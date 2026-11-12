//! The shared kit cache: what the heavy bundled kits download once.
//!
//! agent-skills, md-to-pdf-tools and k8s-tools spend their startup fetching
//! apt packages, a Chromium build, skill tarballs, Python wheels and
//! binaries. With `cache` on, `~/.sbxw/cache` is mounted into every new
//! sandbox and the kits copy from it (`SBXW_CACHE`), falling back to the
//! network for anything missing. sbxw's base kit does not: its Node.js
//! install is an install hook, which sbx runs without the sandbox's
//! environment, so it cannot find the cache.
//!
//! The daemon fills it, from a sandbox of its own: when a kit of the sandbox
//! being created has not marked the cache, or its mark expired, a filler
//! sandbox ([`FILLER`]) is created with those kits and the cache *writable*
//! (`SBXW_CACHE_FILL=1`); they download into it and drop `.filled-<kit>`
//! under their `<arch>-<codename>` directory, and the filler is removed.
//! Every sandbox a person or an agent works in gets the cache read-only,
//! the one whose creation started the fill included: what one sandbox could
//! write, every later one would copy and run.
//!
//! What a refill drops depends on what goes stale. apt's lists and packages
//! do, so they go after `cache_ttl_days`. What a kit pins (a tarball by
//! commit, Chromium by Playwright version, wheels, binaries by hash) does
//! not, so it is kept and the refill costs no download; everything goes once
//! the cache is [`KEEP_PINNED`] times older than that, which bounds how long
//! anything a filler wrote survives (a second line of defence now that the
//! filler is nobody's to work in).
//!
//! One filler at a time: a lock file taken at creation, never released, only
//! outlived. The fill is done once every wanted marker is newer than the
//! lock, and a lock older than `FILL_TIMEOUT` is a fill that failed.

use crate::bundled;
use crate::host::{HostFs, Kind, LocalFs};
use crate::store::Paths;
use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const LOCK: &str = ".filling";
/// Written when an empty cache is first filled: the age of its oldest content.
const BORN: &str = ".born";
const MARKER: &str = ".filled-";
const FILL_TIMEOUT: Duration = Duration::from_secs(30 * 60);
/// Pinned downloads outlive this many `cache_ttl_days` before a full wipe.
pub const KEEP_PINNED: u32 = 4;
/// What goes stale under a flavour: apt's packages and its lists.
const STALE: [&str; 2] = ["apt", "apt-lists"];

/// The sandbox the daemon fills the cache from, one per host at a time (the
/// lock sees to that).
pub const FILLER: &str = "sbxw-cache-fill";

/// What a claim found. Either way the sandbox being created reads the cache.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mount {
    /// Filled and fresh, or being filled: copy what is there.
    ReadOnly,
    /// Missing or expired for a kit of this sandbox, and the lock is now the
    /// caller's: it must have the cache filled.
    Fill,
}

/// Does this kit go into the filler sandbox: sbxw's base kit (Node.js, which
/// the others run on, and its apt packages) or one that fills the cache.
pub fn fills(kit: &str) -> bool {
    sbxw_core::kits::bundled_name(kit).is_some_and(|n| n == sbxw_core::kits::BASE || bundled::CACHING.contains(&n))
}

/// One `<arch>-<codename>` directory the kits filled, or are filling.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Flavour {
    pub name: String,
    pub filled: Vec<String>,
    /// Age of the oldest marker; `None` while no kit has marked it.
    pub age: Option<Duration>,
    pub bytes: u64,
}

pub fn flavours(paths: &Paths) -> Vec<Flavour> {
    let Ok(dir) = std::fs::read_dir(paths.cache()) else { return vec![] };
    let mut out: Vec<Flavour> = dir
        .flatten()
        .filter(|e| e.path().is_dir() && !e.file_name().to_string_lossy().starts_with('.'))
        .map(|e| {
            let p = e.path();
            let ages: Vec<(String, Duration)> = bundled::CACHING
                .iter()
                .filter_map(|k| Some((k.to_string(), age(&p.join(format!("{MARKER}{k}")))?)))
                .collect();
            Flavour {
                name: e.file_name().to_string_lossy().into_owned(),
                age: ages.iter().map(|(_, a)| *a).max(),
                filled: ages.into_iter().map(|(k, _)| k).collect(),
                bytes: du(&p),
            }
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// Is a fill under way: a lock younger than the timeout, and no marker
/// written since it was taken?
pub fn is_filling(paths: &Paths) -> bool {
    let Some(lock) = age(&paths.cache().join(LOCK)).filter(|a| *a < FILL_TIMEOUT) else {
        return false;
    };
    !flavours(paths).iter().any(|f| f.age.is_some_and(|a| a <= lock))
}

/// The bundled kits of this list that use the cache.
fn caching(kits: &[String]) -> Vec<&'static str> {
    bundled::CACHING
        .iter()
        .copied()
        .filter(|c| kits.iter().filter_map(|k| sbxw_core::kits::bundled_name(k)).any(|n| n == *c))
        .collect()
}

/// Does this kit list use the cache at all?
pub fn wanted_by(kits: &[String]) -> bool {
    !caching(kits).is_empty()
}

/// The cache as a claim reads it: modification times in seconds, `now` too.
#[derive(Debug, Default)]
struct Seen {
    now: u64,
    /// Per flavour directory, its markers: (kit, mtime).
    flavours: Vec<(String, Vec<(String, u64)>)>,
    lock: Option<u64>,
    born: Option<u64>,
}

/// What a filler removes before it starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Refill {
    /// Nothing expired: a kit never marked the cache, so it only adds to it.
    TopUp,
    /// Expired: apt's lists and packages go, what the kits pin stays.
    Stale,
    /// Expired, and older than the pinned content may get: everything goes.
    All,
}

/// `None` when the sandbox reads the cache, else what it drops to fill it.
fn decide(seen: &Seen, ttl: Duration, wanted: &[&str]) -> Option<Refill> {
    let age = |t: u64| seen.now.saturating_sub(t);
    let has = |marks: &[(String, u64)], ok: &dyn Fn(u64) -> bool| wanted.iter().all(|k| marks.iter().any(|(n, t)| n == k && ok(*t)));
    if seen.flavours.iter().any(|(_, m)| has(m, &|t| age(t) < ttl.as_secs())) {
        return None;
    }
    // Being filled: a lock younger than the timeout, and nothing completed since.
    if let Some(lock) = seen.lock.filter(|l| age(*l) < FILL_TIMEOUT.as_secs()) {
        if !seen.flavours.iter().any(|(_, m)| has(m, &|t| t >= lock)) {
            return None;
        }
    }
    let expired = seen.flavours.iter().flat_map(|(_, m)| m).any(|(_, t)| age(*t) >= ttl.as_secs());
    Some(match expired {
        false => Refill::TopUp,
        // A cache from before `.born` has content of unknown age: all of it.
        true if seen.born.is_some_and(|b| age(b) < ttl.as_secs().saturating_mul(u64::from(KEEP_PINNED))) => Refill::Stale,
        true => Refill::All,
    })
}

async fn seen(fs: &dyn HostFs, dir: &Path) -> Result<Seen> {
    // 0 = the host cannot say: as old as can be, so it is refilled, not trusted.
    let mut out = Seen { now: SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs()), ..Seen::default() };
    for e in fs.read_dir(dir).await? {
        match (e.meta.kind, e.name.as_str()) {
            (Kind::File, LOCK) => out.lock = Some(e.meta.mtime_secs),
            (Kind::File, BORN) => out.born = Some(e.meta.mtime_secs),
            (Kind::Dir, name) if !name.starts_with('.') => {
                let marks = fs
                    .read_dir(&dir.join(name))
                    .await?
                    .into_iter()
                    .filter_map(|m| Some((m.name.strip_prefix(MARKER)?.to_string(), m.meta.mtime_secs)))
                    .collect();
                out.flavours.push((name.to_string(), marks));
            }
            _ => {}
        }
    }
    Ok(out)
}

/// One claim at a time per daemon: [`HostFs`] has no exclusive create, so the
/// lock file alone cannot arbitrate between two bring-ups.
static CLAIMING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// What the sandbox being created, with `kits`, finds of this machine's cache.
pub async fn claim(paths: &Paths, ttl: Duration, kits: &[String]) -> Result<(PathBuf, Mount)> {
    claim_on(&LocalFs, &paths.root, ttl, kits).await
}

/// [`claim`] on a host's filesystem: the cache is `<root>/cache`, `root`
/// being that host's `~/.sbxw`. The filler takes the lock and drops what
/// expired first. Ages are the host's mtimes judged by this machine's clock,
/// so a TTL is off by the clock skew, no more. Two daemons claiming the same
/// host at once could both fill: a duplicate download, not corruption, as
/// every fill writes only its own files.
pub async fn claim_on(fs: &dyn HostFs, root: &Path, ttl: Duration, kits: &[String]) -> Result<(PathBuf, Mount)> {
    let _one = CLAIMING.lock().await;
    let dir = root.join("cache");
    fs.create_dir_all(&dir).await.with_context(|| format!("creating {}", dir.display()))?;
    let seen = seen(fs, &dir).await?;
    let Some(refill) = decide(&seen, ttl, &caching(kits)) else {
        return Ok((dir, Mount::ReadOnly));
    };
    fs.write_atomic(&dir.join(LOCK), b"", 0o644).await?;
    match refill {
        Refill::TopUp => {}
        Refill::Stale => {
            for (flavour, marks) in &seen.flavours {
                let at = dir.join(flavour);
                for name in STALE.iter().map(|s| s.to_string()).chain(marks.iter().map(|(k, _)| format!("{MARKER}{k}"))) {
                    fs.remove(&at.join(name)).await?;
                }
            }
        }
        Refill::All => {
            for e in fs.read_dir(&dir).await? {
                if e.name != LOCK {
                    fs.remove(&dir.join(&e.name)).await?;
                }
            }
        }
    }
    if refill == Refill::All || seen.flavours.is_empty() {
        fs.write_atomic(&dir.join(BORN), b"", 0o644).await?;
    }
    Ok((dir, Mount::Fill))
}

/// Everything under the cache, gone: the next sandbox refills it.
pub fn clear(paths: &Paths) -> Result<()> {
    if is_filling(paths) {
        bail!(
            "a sandbox is filling the cache — wait for it, or remove {} if it failed",
            paths.cache().join(LOCK).display()
        );
    }
    match std::fs::remove_dir_all(paths.cache()) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
        _ => Ok(()),
    }
}

fn age(p: &Path) -> Option<Duration> {
    let m = std::fs::metadata(p).ok()?.modified().ok()?;
    Some(SystemTime::now().duration_since(m).unwrap_or_default())
}

fn du(p: &Path) -> u64 {
    let Ok(dir) = std::fs::read_dir(p) else { return 0 };
    dir.flatten()
        .map(|e| match e.file_type() {
            Ok(t) if t.is_dir() => du(&e.path()),
            Ok(t) if t.is_file() => e.metadata().map(|m| m.len()).unwrap_or(0),
            _ => 0,
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: Duration = Duration::from_secs(86_400);

    fn kits() -> Vec<String> {
        bundled::CACHING.iter().map(|k| format!("sbxw:{k}")).collect()
    }

    fn fill(p: &Paths) {
        let f = p.cache().join("arm64-bookworm");
        std::fs::create_dir_all(f.join("apt")).unwrap();
        std::fs::create_dir_all(f.join("tarballs")).unwrap();
        std::fs::write(f.join("apt/x.deb"), "12").unwrap();
        std::fs::write(f.join("tarballs/pinned.tar.gz"), "345").unwrap();
        for k in bundled::CACHING {
            std::fs::write(f.join(format!(".filled-{k}")), "").unwrap();
        }
    }

    fn seen_at(now: u64, marks: &[(&str, u64)], lock: Option<u64>, born: Option<u64>) -> Seen {
        let marks: Vec<(String, u64)> = marks.iter().map(|(k, t)| (k.to_string(), *t)).collect();
        Seen { now, flavours: if marks.is_empty() { vec![] } else { vec![("f".into(), marks)] }, lock, born }
    }

    #[tokio::test]
    async fn the_first_claim_fills_the_next_ones_read() {
        let d = tempfile::tempdir().unwrap();
        let p = Paths::under(d.path());
        assert_eq!(claim(&p, DAY, &kits()).await.unwrap().1, Mount::Fill);
        assert_eq!(claim(&p, DAY, &kits()).await.unwrap().1, Mount::ReadOnly, "one filler at a time");
        assert!(clear(&p).is_err(), "not under a running fill");
        fill(&p);
        assert!(!is_filling(&p), "done once the markers follow the lock");
        assert_eq!(claim(&p, DAY, &kits()).await.unwrap().1, Mount::ReadOnly, "fresh");
        assert_eq!(flavours(&p)[0].bytes, 5);
        clear(&p).unwrap();
        assert_eq!(claim(&p, DAY, &kits()).await.unwrap().1, Mount::Fill, "cleared, so refilled");
    }

    #[tokio::test]
    async fn an_expired_cache_keeps_what_is_pinned_and_drops_apt() {
        let d = tempfile::tempdir().unwrap();
        let p = Paths::under(d.path());
        assert_eq!(claim(&p, DAY, &kits()).await.unwrap().1, Mount::Fill, "an empty cache is born here");
        fill(&p);
        // A zero TTL is past the pinned content's age too: see `decide` below
        // for the kept case, which needs a clock.
        assert_eq!(claim(&p, Duration::ZERO, &kits()).await.unwrap().1, Mount::Fill);
        assert!(!p.cache().join("arm64-bookworm").exists(), "past its whole life, nothing survives");
        assert!(is_filling(&p));
    }

    #[test]
    fn what_a_refill_drops() {
        let ttl = 7 * DAY;
        let day = DAY.as_secs();
        let now = 100 * day;
        let both = ["agent-skills", "md-to-pdf-tools"];
        let fresh = [("agent-skills", now - day), ("md-to-pdf-tools", now - day)];
        assert_eq!(decide(&seen_at(now, &fresh, None, Some(now - day)), ttl, &both), None, "fresh: read");
        assert_eq!(decide(&seen_at(now, &[], None, None), ttl, &both), Some(Refill::TopUp), "empty: filled");
        // A kit that never marked the cache adds to it; nothing is dropped.
        assert_eq!(decide(&seen_at(now, &fresh[..1], None, Some(now - day)), ttl, &both), Some(Refill::TopUp));
        assert_eq!(decide(&seen_at(now, &fresh[..1], None, Some(now - day)), ttl, &both[..1]), None, "and a sandbox without that kit reads");
        let old = [("agent-skills", now - 8 * day), ("md-to-pdf-tools", now - 8 * day)];
        assert_eq!(decide(&seen_at(now, &old, None, Some(now - 8 * day)), ttl, &both), Some(Refill::Stale), "expired: apt goes, pins stay");
        assert_eq!(decide(&seen_at(now, &old, None, Some(now - 29 * day)), ttl, &both), Some(Refill::All), "pins too, past four TTLs");
        assert_eq!(decide(&seen_at(now, &old, None, None), ttl, &both), Some(Refill::All), "content of unknown age");
        // A fill under way is read, until it times out.
        assert_eq!(decide(&seen_at(now, &old, Some(now - 60), Some(now)), ttl, &both), None);
        assert_eq!(decide(&seen_at(now, &old, Some(now - 3600), Some(now - day)), ttl, &both), Some(Refill::Stale));
    }

    #[tokio::test]
    async fn a_stale_refill_removes_apt_and_the_markers_only() {
        let d = tempfile::tempdir().unwrap();
        let fs = LocalFs;
        let dir = d.path().join("cache");
        let p = Paths { root: d.path().to_path_buf(), chat_root: PathBuf::new() };
        fill(&p);
        let old = filetime_days_ago(8);
        for k in bundled::CACHING {
            set_mtime(&dir.join(format!("arm64-bookworm/.filled-{k}")), old);
        }
        std::fs::write(dir.join(BORN), "").unwrap();
        set_mtime(&dir.join(BORN), old);
        assert_eq!(claim_on(&fs, d.path(), 7 * DAY, &kits()).await.unwrap().1, Mount::Fill);
        let f = dir.join("arm64-bookworm");
        assert!(f.join("tarballs/pinned.tar.gz").exists(), "what a kit pins is kept");
        assert!(!f.join("apt").exists() && !f.join(".filled-agent-skills").exists(), "apt and the markers go");
        assert_eq!(claim_on(&fs, d.path(), 7 * DAY, &kits()).await.unwrap().1, Mount::ReadOnly, "one filler at a time");
    }

    fn filetime_days_ago(days: u64) -> SystemTime {
        SystemTime::now() - Duration::from_secs(days * 86_400)
    }

    fn set_mtime(p: &Path, t: SystemTime) {
        std::fs::File::options().write(true).open(p).unwrap().set_modified(t).unwrap();
    }

    #[test]
    fn only_the_heavy_bundled_kits_want_the_cache() {
        assert!(wanted_by(&["sbxw:agent-skills".into()]));
        assert!(wanted_by(&["sbxw:k8s-tools".into()]));
        assert!(!wanted_by(&["sbxw:codemap".into(), "/x/agent-skills".into()]));
    }
}
