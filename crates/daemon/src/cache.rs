//! The shared kit cache: what the heavy bundled kits download once.
//!
//! agent-skills and md-to-pdf-tools spend their startup fetching — apt
//! packages, a Chromium build, skill tarballs, Python wheels. With `cache` on,
//! `~/.sbxw/cache` is mounted into every new sandbox and the kits copy from it
//! (`SBXW_CACHE`), falling back to the network for anything missing.
//!
//! **The first sandbox fills it.** When the cache is empty or expired, the
//! sandbox being created gets it *writable* with `SBXW_CACHE_FILL=1`; its kits
//! download into it and drop `.filled-<kit>` under their `<arch>-<codename>`
//! directory. Every other sandbox gets it read-only. The filler stays able to
//! write it for its whole life — the price of not running a separate warmer —
//! so the cache expires after `cache_ttl_days` and is refilled from scratch,
//! which bounds how long anything it wrote survives.
//!
//! One filler at a time: a lock file taken at creation. It is never released,
//! only outlived — the fill is done once every marker is newer than the lock,
//! and a lock older than [`FILL_TIMEOUT`] is a fill that failed.

use crate::bundled;
use crate::host::{HostFs, Kind};
use crate::store::Paths;
use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const LOCK: &str = ".filling";
const FILL_TIMEOUT: Duration = Duration::from_secs(30 * 60);

/// How a new sandbox gets the cache.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mount {
    /// Filled and fresh, or being filled: copy what is there.
    ReadOnly,
    /// Empty or expired: this sandbox fills it.
    Fill,
}

/// One `<arch>-<codename>` directory the kits filled, or are filling.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Flavour {
    pub name: String,
    pub filled: Vec<String>,
    /// Age of the oldest marker, once every caching kit has marked it.
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
                .filter_map(|k| Some((k.to_string(), age(&p.join(format!(".filled-{k}")))?)))
                .collect();
            Flavour {
                name: e.file_name().to_string_lossy().into_owned(),
                age: (ages.len() == bundled::CACHING.len()).then(|| ages.iter().map(|(_, a)| *a).max().unwrap_or_default()),
                filled: ages.into_iter().map(|(k, _)| k).collect(),
                bytes: du(&p),
            }
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// Filled within the TTL, for some flavour? Sandboxes of one host share an
/// image, so one flavour is the common case.
pub fn is_fresh(paths: &Paths, ttl: Duration) -> bool {
    flavours(paths).iter().any(|f| f.age.is_some_and(|a| a < ttl))
}

/// Is a fill under way: a lock younger than the timeout, and no flavour
/// completed since it was taken?
pub fn is_filling(paths: &Paths) -> bool {
    let Some(lock) = age(&paths.cache().join(LOCK)).filter(|a| *a < FILL_TIMEOUT) else {
        return false;
    };
    !flavours(paths).iter().any(|f| f.age.is_some_and(|a| a <= lock))
}

/// Does this kit list use the cache at all?
pub fn wanted_by(kits: &[String]) -> bool {
    kits.iter()
        .filter_map(|k| sbxw_core::kits::bundled_name(k))
        .any(|n| bundled::CACHING.contains(&n))
}

/// Decide how the sandbox being created mounts the cache — and when it is the
/// filler, take the lock and drop the expired content first.
pub fn claim(paths: &Paths, ttl: Duration) -> Result<(PathBuf, Mount)> {
    let dir = paths.cache();
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    if is_fresh(paths, ttl) || is_filling(paths) {
        return Ok((dir, Mount::ReadOnly));
    }
    let lock = dir.join(LOCK);
    match std::fs::OpenOptions::new().write(true).create_new(true).open(&lock) {
        Ok(_) => {}
        // Another bring-up claimed it a moment ago.
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists && is_filling(paths) => {
            return Ok((dir, Mount::ReadOnly));
        }
        // A failed fill's: take it over.
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => std::fs::write(&lock, b"")?,
        Err(e) => return Err(e.into()),
    }
    for entry in std::fs::read_dir(&dir)?.flatten() {
        if entry.file_name() != LOCK {
            let p = entry.path();
            let _ = if p.is_dir() { std::fs::remove_dir_all(&p) } else { std::fs::remove_file(&p) };
        }
    }
    Ok((dir, Mount::Fill))
}

/// One claim at a time per daemon: [`HostFs`] has no exclusive create, so the
/// lock file below cannot arbitrate between two bring-ups of the same process
/// the way `create_new` does locally.
static CLAIMING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// [`claim`] on a host's filesystem: the cache is `<root>/cache`, `root`
/// being that host's `~/.sbxw`. Ages come from the host's mtimes judged by
/// this machine's clock, so a host whose clock is days off misjudges a TTL
/// of days by that much, no more. Two daemons claiming the same host at the
/// same instant could both fill: the cost is a duplicate download, not
/// corruption, as every fill writes only its own files.
pub async fn claim_on(fs: &dyn HostFs, root: &Path, ttl: Duration) -> Result<(PathBuf, Mount)> {
    let _one = CLAIMING.lock().await;
    let dir = root.join("cache");
    fs.create_dir_all(&dir).await.with_context(|| format!("creating {}", dir.display()))?;
    let flavours = flavours_on(fs, &dir).await?;
    let lock_age = age_on(fs, &dir.join(LOCK)).await?;
    if flavours.iter().any(|f| f.is_some_and(|a| a < ttl)) {
        return Ok((dir, Mount::ReadOnly));
    }
    // Filling: a lock younger than the timeout, and nothing completed since.
    if lock_age.is_some_and(|l| l < FILL_TIMEOUT && !flavours.iter().any(|f| f.is_some_and(|a| a <= l))) {
        return Ok((dir, Mount::ReadOnly));
    }
    fs.write_atomic(&dir.join(LOCK), b"", 0o644).await?;
    for e in fs.read_dir(&dir).await? {
        if e.name != LOCK {
            fs.remove(&dir.join(&e.name)).await?;
        }
    }
    Ok((dir, Mount::Fill))
}

/// Per `<arch>-<codename>` directory: the age of its oldest marker once every
/// caching kit has marked it, else `None`.
async fn flavours_on(fs: &dyn HostFs, dir: &Path) -> Result<Vec<Option<Duration>>> {
    let mut out = Vec::new();
    for e in fs.read_dir(dir).await? {
        if e.meta.kind != Kind::Dir || e.name.starts_with('.') {
            continue;
        }
        let mut ages = Vec::new();
        for k in bundled::CACHING {
            if let Some(a) = age_on(fs, &dir.join(&e.name).join(format!(".filled-{k}"))).await? {
                ages.push(a);
            }
        }
        out.push((ages.len() == bundled::CACHING.len()).then(|| ages.into_iter().max().unwrap_or_default()));
    }
    Ok(out)
}

async fn age_on(fs: &dyn HostFs, p: &Path) -> Result<Option<Duration>> {
    let Some(m) = fs.stat(p).await? else { return Ok(None) };
    // 0 = the host cannot say: as old as can be, so it is refilled, not trusted.
    let mtime = if m.mtime_secs == 0 { UNIX_EPOCH } else { UNIX_EPOCH + Duration::from_secs(m.mtime_secs) };
    Ok(Some(SystemTime::now().duration_since(mtime).unwrap_or_default()))
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

    fn fill(p: &Paths) {
        let f = p.cache().join("arm64-bookworm");
        std::fs::create_dir_all(&f).unwrap();
        std::fs::write(f.join("blob"), "12345").unwrap();
        for k in bundled::CACHING {
            std::fs::write(f.join(format!(".filled-{k}")), "").unwrap();
        }
    }

    #[test]
    fn the_first_claim_fills_the_next_ones_read() {
        let d = tempfile::tempdir().unwrap();
        let p = Paths::under(d.path());
        assert_eq!(claim(&p, DAY).unwrap().1, Mount::Fill);
        assert_eq!(claim(&p, DAY).unwrap().1, Mount::ReadOnly, "one filler at a time");
        assert!(clear(&p).is_err(), "not under a running fill");
        fill(&p);
        assert!(is_fresh(&p, DAY));
        assert!(!is_filling(&p), "done once the markers follow the lock");
        assert_eq!(flavours(&p)[0].bytes, 5);
        clear(&p).unwrap();
        assert_eq!(claim(&p, DAY).unwrap().1, Mount::Fill, "cleared, so refilled");
    }

    #[test]
    fn an_expired_cache_is_emptied_and_refilled() {
        let d = tempfile::tempdir().unwrap();
        let p = Paths::under(d.path());
        fill(&p);
        assert!(!is_fresh(&p, Duration::ZERO));
        assert_eq!(claim(&p, Duration::ZERO).unwrap().1, Mount::Fill);
        assert!(!p.cache().join("arm64-bookworm").exists(), "nothing old survives a refill");
        assert!(is_filling(&p));
    }

    #[test]
    fn a_partial_fill_is_not_fresh() {
        let d = tempfile::tempdir().unwrap();
        let p = Paths::under(d.path());
        fill(&p);
        std::fs::remove_file(p.cache().join("arm64-bookworm/.filled-agent-skills")).unwrap();
        assert!(!is_fresh(&p, DAY));
        assert_eq!(flavours(&p)[0].filled, ["md-to-pdf-tools"]);
    }

    #[tokio::test]
    async fn claim_on_a_host_behaves_like_claim() {
        let d = tempfile::tempdir().unwrap();
        let fs = crate::host::LocalFs;
        let root = d.path();
        assert_eq!(claim_on(&fs, root, DAY).await.unwrap().1, Mount::Fill);
        assert_eq!(claim_on(&fs, root, DAY).await.unwrap().1, Mount::ReadOnly, "one filler at a time");
        let f = root.join("cache/arm64-bookworm");
        std::fs::create_dir_all(&f).unwrap();
        std::fs::write(f.join("blob"), "x").unwrap();
        for k in bundled::CACHING {
            std::fs::write(f.join(format!(".filled-{k}")), "").unwrap();
        }
        assert_eq!(claim_on(&fs, root, DAY).await.unwrap().1, Mount::ReadOnly, "fresh");
        assert_eq!(claim_on(&fs, root, Duration::ZERO).await.unwrap().1, Mount::Fill, "expired");
        assert!(!f.exists(), "nothing old survives a refill");
        assert!(root.join("cache/.filling").exists());
    }

    #[test]
    fn only_the_heavy_bundled_kits_want_the_cache() {
        assert!(wanted_by(&["sbxw:agent-skills".into()]));
        assert!(!wanted_by(&["sbxw:codemap".into(), "/x/agent-skills".into()]));
    }
}
