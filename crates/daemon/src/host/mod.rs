//! A host the daemon acts on, this machine or a remote one: a name, a way to
//! run commands there ([`HostExec`]) and a way to touch its files
//! ([`HostFs`]). Everything that touches a sandbox's workspace, git checkout or
//! kit goes through these, so one code path serves `local` and SSH hosts.
//!
//! * [`LocalFs`] is `std::fs`, inside `spawn_blocking`;
//! * `exec_fs` is `HostFs` over any `HostExec` (POSIX sh on the host);
//! * `kits` mirrors kits to a host; `link` holds one SSH connection per host.
//!
//! Paths are host paths: absolute, as the host spells them.

pub mod exec_fs;
pub mod kits;
pub mod link;

use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use sbxw_core::names::HostName;
use sbxw_sbx::host::{HostExec, LocalExec};
use std::fmt;
use std::io::{ErrorKind, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::UNIX_EPOCH;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    File,
    Dir,
    Symlink,
    Other,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Meta {
    pub kind: Kind,
    pub len: u64,
    /// Seconds since the Unix epoch; 0 when the host cannot say.
    pub mtime_secs: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DirEntry {
    pub name: String,
    /// Of the entry itself: a symlink is `Kind::Symlink`.
    pub meta: Meta,
}

/// A host's filesystem. `async_trait` because it is used as `Arc<dyn HostFs>`.
#[async_trait]
#[allow(clippy::double_must_use)]
pub trait HostFs: Send + Sync {
    /// Follows symlinks; `None` = absent (including a dangling symlink).
    async fn stat(&self, p: &Path) -> Result<Option<Meta>>;
    /// Does not follow symlinks; `None` = absent.
    async fn lstat(&self, p: &Path) -> Result<Option<Meta>>;
    /// Entries sorted by name, without `.` and `..`, each with its `lstat`.
    async fn read_dir(&self, p: &Path) -> Result<Vec<DirEntry>>;
    /// The whole file; an error past `max` bytes, never a truncation.
    async fn read(&self, p: &Path, max: u64) -> Result<Vec<u8>>;
    /// Temporary file beside `p`, `mode` set, renamed over `p`: a reader sees
    /// the old file or the new one, never half. The parent must exist.
    async fn write_atomic(&self, p: &Path, data: &[u8], mode: u32) -> Result<()>;
    async fn create_dir_all(&self, p: &Path) -> Result<()>;
    /// Like `rm -rf`: a file, a symlink (not its target) or a whole tree.
    /// Absent is not an error.
    async fn remove(&self, p: &Path) -> Result<()>;
    async fn canonicalize(&self, p: &Path) -> Result<PathBuf>;
    /// Everything under `root` down to `depth` levels (1 = its direct
    /// children), as (`a/b` path relative to root, `lstat`), sorted by path.
    /// An entry whose name is in `skip` is left out and not descended into;
    /// symlinks are listed, never followed. One round trip on a remote host.
    async fn walk(&self, root: &Path, depth: usize, skip: &[&str]) -> Result<Vec<(String, Meta)>>;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct LocalFs;

fn meta_of(m: &std::fs::Metadata) -> Meta {
    let t = m.file_type();
    let kind = if t.is_symlink() {
        Kind::Symlink
    } else if t.is_dir() {
        Kind::Dir
    } else if t.is_file() {
        Kind::File
    } else {
        Kind::Other
    };
    let mtime_secs = m.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs());
    Meta { kind, len: m.len(), mtime_secs }
}

fn absent_is_none(r: std::io::Result<std::fs::Metadata>, p: &Path) -> Result<Option<Meta>> {
    match r {
        Ok(m) => Ok(Some(meta_of(&m))),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| p.display().to_string()),
    }
}

/// Run blocking filesystem work off the async threads.
async fn blocking<T: Send + 'static>(f: impl FnOnce() -> Result<T> + Send + 'static) -> Result<T> {
    tokio::task::spawn_blocking(f).await.context("filesystem task panicked")?
}

fn walk_into(dir: &Path, rel: &str, depth: usize, skip: &[String], out: &mut Vec<(String, Meta)>) -> Result<()> {
    if depth == 0 {
        return Ok(());
    }
    for e in std::fs::read_dir(dir).with_context(|| dir.display().to_string())? {
        let e = e?;
        let name = e.file_name().to_string_lossy().into_owned();
        if skip.contains(&name) {
            continue;
        }
        let meta = meta_of(&e.metadata()?);
        let path = if rel.is_empty() { name } else { format!("{rel}/{name}") };
        if meta.kind == Kind::Dir {
            walk_into(&e.path(), &path, depth - 1, skip, out)?;
        }
        out.push((path, meta));
    }
    Ok(())
}

#[async_trait]
impl HostFs for LocalFs {
    async fn stat(&self, p: &Path) -> Result<Option<Meta>> {
        let p = p.to_path_buf();
        blocking(move || absent_is_none(std::fs::metadata(&p), &p)).await
    }

    async fn lstat(&self, p: &Path) -> Result<Option<Meta>> {
        let p = p.to_path_buf();
        blocking(move || absent_is_none(std::fs::symlink_metadata(&p), &p)).await
    }

    async fn read_dir(&self, p: &Path) -> Result<Vec<DirEntry>> {
        let p = p.to_path_buf();
        blocking(move || {
            let mut v = Vec::new();
            for e in std::fs::read_dir(&p).with_context(|| p.display().to_string())? {
                let e = e?;
                v.push(DirEntry { name: e.file_name().to_string_lossy().into_owned(), meta: meta_of(&e.metadata()?) });
            }
            v.sort_by(|a, b| a.name.cmp(&b.name));
            Ok(v)
        })
        .await
    }

    async fn read(&self, p: &Path, max: u64) -> Result<Vec<u8>> {
        let p = p.to_path_buf();
        blocking(move || {
            let f = std::fs::File::open(&p).with_context(|| p.display().to_string())?;
            let mut buf = Vec::new();
            // One byte past the limit tells "exactly max" from "longer".
            f.take(max.saturating_add(1)).read_to_end(&mut buf).with_context(|| p.display().to_string())?;
            if buf.len() as u64 > max {
                bail!("{} is longer than {max} bytes", p.display());
            }
            Ok(buf)
        })
        .await
    }

    async fn write_atomic(&self, p: &Path, data: &[u8], mode: u32) -> Result<()> {
        let p = p.to_path_buf();
        let data = data.to_vec();
        blocking(move || {
            let dir = p.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(Path::new("."));
            let name = p.file_name().with_context(|| format!("{}: no file name", p.display()))?;
            let tmp = dir.join(format!(".{}.tmp-{}", name.to_string_lossy(), std::process::id()));
            let write = || -> Result<()> {
                let mut f = std::fs::File::create(&tmp)?;
                f.write_all(&data)?;
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    // set_permissions, unlike the create mode, ignores the umask.
                    f.set_permissions(std::fs::Permissions::from_mode(mode))?;
                }
                f.sync_all()?;
                std::fs::rename(&tmp, &p)?;
                Ok(())
            };
            write().inspect_err(|_| {
                let _ = std::fs::remove_file(&tmp);
            })
            .with_context(|| p.display().to_string())
        })
        .await
    }

    async fn create_dir_all(&self, p: &Path) -> Result<()> {
        let p = p.to_path_buf();
        blocking(move || std::fs::create_dir_all(&p).with_context(|| p.display().to_string())).await
    }

    async fn remove(&self, p: &Path) -> Result<()> {
        let p = p.to_path_buf();
        blocking(move || {
            let r = match std::fs::symlink_metadata(&p) {
                Err(e) if e.kind() == ErrorKind::NotFound => return Ok(()),
                Err(e) => Err(e),
                Ok(m) if m.is_dir() => std::fs::remove_dir_all(&p),
                Ok(_) => std::fs::remove_file(&p),
            };
            match r {
                Err(e) if e.kind() != ErrorKind::NotFound => Err(e).with_context(|| p.display().to_string()),
                _ => Ok(()),
            }
        })
        .await
    }

    async fn canonicalize(&self, p: &Path) -> Result<PathBuf> {
        let p = p.to_path_buf();
        blocking(move || std::fs::canonicalize(&p).with_context(|| p.display().to_string())).await
    }

    async fn walk(&self, root: &Path, depth: usize, skip: &[&str]) -> Result<Vec<(String, Meta)>> {
        let root = root.to_path_buf();
        let skip: Vec<String> = skip.iter().map(|s| s.to_string()).collect();
        blocking(move || {
            let mut out = Vec::new();
            walk_into(&root, "", depth, &skip, &mut out)?;
            out.sort_by(|a, b| a.0.cmp(&b.0));
            Ok(out)
        })
        .await
    }
}

#[derive(Clone)]
pub struct Host {
    pub name: HostName,
    pub exec: Arc<dyn HostExec>,
    pub fs: Arc<dyn HostFs>,
}

impl Host {
    pub fn local() -> Self {
        Self { name: HostName::local(), exec: Arc::new(LocalExec), fs: Arc::new(LocalFs) }
    }

    pub fn is_local(&self) -> bool {
        self.name.is_local()
    }
}

impl fmt::Debug for Host {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Host").field("name", &self.name).finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fs() -> LocalFs {
        LocalFs
    }

    #[tokio::test]
    async fn stat_and_lstat_follow_or_not_and_absent_is_none() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("f");
        std::fs::write(&f, b"hello").unwrap();
        std::os::unix::fs::symlink(&f, d.path().join("l")).unwrap();
        std::os::unix::fs::symlink(d.path().join("gone"), d.path().join("dangling")).unwrap();

        let m = fs().stat(&f).await.unwrap().unwrap();
        assert_eq!((m.kind, m.len), (Kind::File, 5));
        assert!(m.mtime_secs > 0);
        assert_eq!(fs().stat(&d.path().join("l")).await.unwrap().unwrap().kind, Kind::File);
        assert_eq!(fs().lstat(&d.path().join("l")).await.unwrap().unwrap().kind, Kind::Symlink);
        assert_eq!(fs().stat(d.path()).await.unwrap().unwrap().kind, Kind::Dir);
        assert_eq!(fs().stat(&d.path().join("nope")).await.unwrap(), None);
        assert_eq!(fs().lstat(&d.path().join("nope")).await.unwrap(), None);
        assert_eq!(fs().stat(&d.path().join("dangling")).await.unwrap(), None);
        assert_eq!(fs().lstat(&d.path().join("dangling")).await.unwrap().unwrap().kind, Kind::Symlink);
    }

    #[tokio::test]
    async fn read_dir_is_sorted_with_lstat_kinds() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("b"), b"").unwrap();
        std::fs::create_dir(d.path().join("a")).unwrap();
        std::os::unix::fs::symlink("b", d.path().join("c")).unwrap();
        let v = fs().read_dir(d.path()).await.unwrap();
        let got: Vec<(&str, Kind)> = v.iter().map(|e| (e.name.as_str(), e.meta.kind)).collect();
        assert_eq!(got, [("a", Kind::Dir), ("b", Kind::File), ("c", Kind::Symlink)]);
        assert!(fs().read_dir(&d.path().join("nope")).await.is_err());
    }

    #[tokio::test]
    async fn read_refuses_past_max() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("f");
        std::fs::write(&f, b"12345").unwrap();
        assert_eq!(fs().read(&f, 5).await.unwrap(), b"12345");
        assert_eq!(fs().read(&f, 100).await.unwrap(), b"12345");
        assert!(fs().read(&f, 4).await.is_err());
        assert!(fs().read(&d.path().join("nope"), 10).await.is_err());
    }

    #[tokio::test]
    async fn write_atomic_replaces_and_sets_mode() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("f");
        fs().write_atomic(&f, b"one", 0o600).await.unwrap();
        assert_eq!(std::fs::read(&f).unwrap(), b"one");
        assert_eq!(std::fs::metadata(&f).unwrap().permissions().mode() & 0o7777, 0o600);
        fs().write_atomic(&f, b"two", 0o755).await.unwrap();
        assert_eq!(std::fs::read(&f).unwrap(), b"two");
        assert_eq!(std::fs::metadata(&f).unwrap().permissions().mode() & 0o7777, 0o755);
        // No temporary file left behind.
        assert_eq!(std::fs::read_dir(d.path()).unwrap().count(), 1);
        assert!(fs().write_atomic(&d.path().join("no/parent"), b"x", 0o644).await.is_err());
        assert_eq!(std::fs::read_dir(d.path()).unwrap().count(), 1);
    }

    #[tokio::test]
    async fn create_dir_all_and_remove() {
        let d = tempfile::tempdir().unwrap();
        let deep = d.path().join("a/b/c");
        fs().create_dir_all(&deep).await.unwrap();
        fs().create_dir_all(&deep).await.unwrap();
        std::fs::write(deep.join("f"), b"x").unwrap();
        let target = d.path().join("keep");
        std::fs::write(&target, b"x").unwrap();
        std::os::unix::fs::symlink(&target, d.path().join("link")).unwrap();

        fs().remove(&d.path().join("link")).await.unwrap();
        assert!(target.exists(), "removing a symlink keeps its target");
        fs().remove(&target).await.unwrap();
        assert!(!target.exists());
        fs().remove(&d.path().join("a")).await.unwrap();
        assert!(!d.path().join("a").exists());
        fs().remove(&d.path().join("nope")).await.unwrap();
    }

    #[tokio::test]
    async fn canonicalize_resolves_links() {
        let d = tempfile::tempdir().unwrap();
        let real = std::fs::canonicalize(d.path()).unwrap();
        std::fs::create_dir(real.join("x")).unwrap();
        std::os::unix::fs::symlink(real.join("x"), real.join("l")).unwrap();
        assert_eq!(fs().canonicalize(&real.join("l/../x")).await.unwrap(), real.join("x"));
        assert!(fs().canonicalize(&real.join("nope")).await.is_err());
    }

    #[tokio::test]
    async fn walk_respects_depth_and_skip_and_does_not_follow_links() {
        let d = tempfile::tempdir().unwrap();
        let r = d.path();
        std::fs::create_dir_all(r.join("src/deep")).unwrap();
        std::fs::create_dir_all(r.join(".git/objects")).unwrap();
        std::fs::create_dir_all(r.join("lib/node_modules/x")).unwrap();
        std::fs::write(r.join("src/main.rs"), b"fn main(){}").unwrap();
        std::fs::write(r.join("src/deep/a.rs"), b"").unwrap();
        std::fs::write(r.join("README"), b"hi").unwrap();
        std::os::unix::fs::symlink(r.join("src"), r.join("ln")).unwrap();

        let names = |v: Vec<(String, Meta)>| v.into_iter().map(|(p, _)| p).collect::<Vec<_>>();
        let one = fs().walk(r, 1, &[".git"]).await.unwrap();
        assert_eq!(names(one), ["README", "lib", "ln", "src"]);

        let all = fs().walk(r, 10, &[".git", "node_modules"]).await.unwrap();
        let kinds: std::collections::BTreeMap<_, _> = all.iter().map(|(p, m)| (p.clone(), m.kind)).collect();
        assert_eq!(kinds["ln"], Kind::Symlink);
        assert_eq!(kinds["src/main.rs"], Kind::File);
        assert_eq!(all.iter().find(|(p, _)| p == "src/main.rs").unwrap().1.len, 11);
        assert_eq!(names(all), ["README", "lib", "ln", "src", "src/deep", "src/deep/a.rs", "src/main.rs"]);

        let two = fs().walk(r, 2, &[]).await.unwrap();
        assert!(names(two.clone()).contains(&".git/objects".to_string()));
        assert!(!names(two).contains(&"src/deep/a.rs".to_string()));
        assert!(fs().walk(r, 0, &[]).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn local_host_runs_and_reads_here() {
        let h = Host::local();
        assert!(h.is_local());
        let out = h.exec.exec(sbxw_sbx::host::HostCmd::new("echo", vec!["hi".into()])).await.unwrap();
        assert_eq!(out.stdout_str(), "hi\n");
        assert_eq!(h.fs.stat(Path::new("/")).await.unwrap().unwrap().kind, Kind::Dir);
    }
}
