//! The shared kit cache: what the heavy bundled kits download once.
//!
//! agent-skills and md-to-pdf-tools spend their startup fetching — apt
//! packages, a Chromium build, skill tarballs, Python wheels. With `cache` on,
//! `~/.sbxw/cache` is mounted **read-only** into every new sandbox and the
//! kits copy from it (`SBXW_CACHE`), falling back to the network for anything
//! missing. Read-only, because an agent that could write it would decide what
//! the next sandbox installs.
//!
//! The only writer is the *warmer*: a throwaway sandbox nobody attaches to,
//! created with the cache mounted writable and `SBXW_CACHE_FILL=1`, holding
//! just the caching kits. Each kit drops `.filled-<kit>` under its
//! `<arch>-<codename>` directory when done; the warmer waits for both, then
//! removes the sandbox. One warmer at a time, across processes: a lock file.

use crate::bundled;
use crate::store::Paths;
use anyhow::{bail, Context, Result};
use sbxw_core::SandboxName;
use sbxw_sbx::{CreateOpts, Sbx};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

pub const WARM_SANDBOX: &str = "sbxw-cache-warm";
const LOCK: &str = ".warming";
/// Past this, a lock is a crashed warmer's, and a warm is given up on.
const WARM_TIMEOUT: Duration = Duration::from_secs(25 * 60);

/// One `<arch>-<codename>` directory the kits filled, or are filling.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Flavour {
    pub name: String,
    pub filled: Vec<String>,
    pub bytes: u64,
}

pub fn flavours(paths: &Paths) -> Vec<Flavour> {
    let Ok(dir) = std::fs::read_dir(paths.cache()) else { return vec![] };
    let mut out: Vec<Flavour> = dir
        .flatten()
        .filter(|e| e.path().is_dir() && !e.file_name().to_string_lossy().starts_with('.'))
        .map(|e| {
            let p = e.path();
            Flavour {
                name: e.file_name().to_string_lossy().into_owned(),
                filled: bundled::CACHING
                    .iter()
                    .filter(|k| p.join(format!(".filled-{k}")).exists())
                    .map(|k| k.to_string())
                    .collect(),
                bytes: du(&p),
            }
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// Has a warm completed for some flavour? Sandboxes of one host share an
/// image, so one flavour is the common case.
pub fn is_filled(paths: &Paths) -> bool {
    flavours(paths).iter().any(|f| f.filled.len() == bundled::CACHING.len())
}

/// Is a warmer running (a lock younger than the timeout)?
pub fn is_warming(paths: &Paths) -> bool {
    lock_age(&paths.cache().join(LOCK)).is_some_and(|age| age < WARM_TIMEOUT)
}

/// Does this kit list use the cache at all?
pub fn wanted_by(kits: &[String]) -> bool {
    kits.iter()
        .filter_map(|k| sbxw_core::kits::bundled_name(k))
        .any(|n| bundled::CACHING.contains(&n))
}

/// The directory to mount, created if needed.
pub fn ensure_dir(paths: &Paths) -> Result<PathBuf> {
    let dir = paths.cache();
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    Ok(dir)
}

/// Fill the cache. `force` drops the completion markers first, so every kit
/// downloads again (what was fetched before is overwritten, not reused).
/// `say` gets one line per stage.
pub async fn warm(sbx: &Sbx, paths: &Paths, pull: Option<String>, force: bool, say: &(dyn Fn(&str) + Sync)) -> Result<()> {
    let dir = ensure_dir(paths)?;
    if !force && is_filled(paths) {
        say("the cache is already filled (`sbxw cache warm --force` refreshes it)");
        return Ok(());
    }
    let lock = dir.join(LOCK);
    take_lock(&lock)?;
    let result = warm_locked(sbx, paths, &dir, pull, force, say).await;
    let _ = std::fs::remove_file(&lock);
    result
}

async fn warm_locked(sbx: &Sbx, paths: &Paths, dir: &Path, pull: Option<String>, force: bool, say: &(dyn Fn(&str) + Sync)) -> Result<()> {
    if force {
        for f in flavours(paths) {
            for k in bundled::CACHING {
                let _ = std::fs::remove_file(dir.join(&f.name).join(format!(".filled-{k}")));
            }
            let _ = std::fs::remove_file(dir.join(&f.name).join("apt-lists/.stamp"));
        }
    }
    let name = SandboxName::parse(WARM_SANDBOX).expect("a valid name");
    if sbx.exists(&name).await {
        sbx.rm(std::slice::from_ref(&name), false).await?;
    }
    let ws = paths.root.join(".cache-warm-ws");
    std::fs::create_dir_all(&ws)?;
    let kits = bundled::resolve(paths, &bundled::CACHING.map(|k| format!("sbxw:{k}")))?;
    let cache = dir.to_string_lossy().into_owned();
    let opts = CreateOpts {
        workspace: ws.to_string_lossy().into_owned(),
        rw_mounts: vec![cache.clone()],
        kits,
        pull,
        env: vec![format!("SBXW_CACHE={cache}"), "SBXW_CACHE_FILL=1".into()],
        ..Default::default()
    };
    say(&format!("creating '{WARM_SANDBOX}' to fill {cache} (several minutes)"));
    let outcome = async {
        sbx.create(&name, &opts, None).await.context("creating the warming sandbox")?;
        // Startup commands run when the container starts; nothing attaches
        // to this sandbox, so start it detached if create did not.
        if !sbx.wait_until_running(&name, Duration::from_secs(5)).await {
            sbx.run_detached(&name).await.context("starting the warming sandbox")?;
        }
        say("kits are installing and filling the cache");
        let deadline = tokio::time::Instant::now() + WARM_TIMEOUT;
        loop {
            if is_filled(paths) {
                return Ok(());
            }
            if tokio::time::Instant::now() >= deadline {
                bail!("the cache was not filled within {} minutes", WARM_TIMEOUT.as_secs() / 60);
            }
            tokio::time::sleep(Duration::from_secs(3)).await;
        }
    }
    .await;
    if let Err(e) = sbx.rm(std::slice::from_ref(&name), false).await {
        say(&format!("could not remove '{WARM_SANDBOX}': {e:#}"));
    }
    let _ = std::fs::remove_dir_all(&ws);
    if outcome.is_ok() {
        say("cache filled — new sandboxes install the bundled kits from it");
    }
    outcome
}

/// Everything under the cache, gone. Refused while a warmer runs.
pub fn clear(paths: &Paths) -> Result<()> {
    if is_warming(paths) {
        bail!("a warm is running — wait for it, or remove {} if it crashed", paths.cache().join(LOCK).display());
    }
    match std::fs::remove_dir_all(paths.cache()) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
        _ => Ok(()),
    }
}

fn take_lock(lock: &Path) -> Result<()> {
    match std::fs::OpenOptions::new().write(true).create_new(true).open(lock) {
        Ok(_) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            if lock_age(lock).is_some_and(|age| age < WARM_TIMEOUT) {
                bail!("a warm is already running");
            }
            // A crashed warmer's: take it over.
            std::fs::write(lock, b"")?;
            Ok(())
        }
        Err(e) => Err(e.into()),
    }
}

fn lock_age(lock: &Path) -> Option<Duration> {
    let m = std::fs::metadata(lock).ok()?.modified().ok()?;
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

    #[test]
    fn filled_means_every_caching_kit_marked_in_one_flavour() {
        let d = tempfile::tempdir().unwrap();
        let p = Paths::under(d.path());
        assert!(!is_filled(&p));
        let f = p.cache().join("arm64-bookworm");
        std::fs::create_dir_all(&f).unwrap();
        std::fs::write(f.join(".filled-agent-skills"), "").unwrap();
        assert!(!is_filled(&p));
        std::fs::write(f.join(".filled-md-to-pdf-tools"), "").unwrap();
        std::fs::write(f.join("blob"), "12345").unwrap();
        assert!(is_filled(&p));
        assert_eq!(flavours(&p)[0].bytes, 5);
    }

    #[test]
    fn one_warmer_at_a_time() {
        let d = tempfile::tempdir().unwrap();
        let p = Paths::under(d.path());
        ensure_dir(&p).unwrap();
        let lock = p.cache().join(LOCK);
        take_lock(&lock).unwrap();
        assert!(is_warming(&p));
        assert!(take_lock(&lock).is_err());
        assert!(clear(&p).is_err(), "not under a running warmer");
        std::fs::remove_file(&lock).unwrap();
        clear(&p).unwrap();
        assert!(!p.cache().exists());
    }

    #[test]
    fn only_the_heavy_bundled_kits_want_the_cache() {
        assert!(wanted_by(&["sbxw:agent-skills".into()]));
        assert!(!wanted_by(&["sbxw:codemap".into(), "/x/agent-skills".into()]));
    }
}
