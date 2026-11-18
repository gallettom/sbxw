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

    #[tokio::test]
    async fn local_host_runs_and_reads_here() {
        let h = Host::local();
        assert!(h.is_local());
        let out = h.exec.exec(sbxw_sbx::host::HostCmd::new("echo", vec!["hi".into()])).await.unwrap();
        assert_eq!(out.stdout_str(), "hi\n");
        assert_eq!(h.fs.stat(Path::new("/")).await.unwrap().unwrap().kind, Kind::Dir);
    }
}
