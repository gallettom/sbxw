//! The Project panel's file explorer: a sandbox's own filesystem, read-only.
//!
//! The artifacts listing reads `.sbxw-artifacts` from the
//! host, through the workspace mount, which is all a list of deliverables needs.
//! An explorer needs the rest — `/home/agent`, `/etc`, a tool's cache — and those
//! exist only inside the container. So a **running** sandbox is read through
//! `sbx exec`: one `find` per directory, one `cat` per file.
//!
//! A **stopped** sandbox cannot be exec'd into, and starting one to show a
//! directory listing is not a call a file browser gets to make. What is still
//! readable then is the workspace, from the host: in direct mode it is mounted
//! at the same absolute path, so the paths the panel shows stay the ones the
//! agent sees. The listing says which of the two it is (`offline`), and
//! anything outside the workspace is refused rather than read off the host's own
//! disk under a sandbox's name.

use anyhow::{bail, Context, Result};
use serde::Serialize;
use std::path::{Path, PathBuf};

use sbxw_core::SandboxName;
use sbxw_sbx::{runner::Output, Sbx};

/// Where the agent lives in every sandbox; the explorer's "Home" favourite.
pub const SANDBOX_HOME: &str = "/home/agent";

/// Largest file the panel will hand over in one response. Files are read into
/// memory on both sides of `sbx exec`, so this is a memory bound, not a policy.
pub const MAX_FILE: u64 = 256 * 1024 * 1024;

#[derive(Serialize, Debug, PartialEq)]
pub struct Entry {
    pub name: String,
    /// `dir`, `file` or `other` — of the *target*, for a symlink, so a link to
    /// a directory opens like one. A dangling link is `other`.
    pub kind: &'static str,
    pub link: bool,
    pub size: u64,
    /// Unix seconds.
    pub modified: u64,
}

#[derive(Serialize, Debug)]
pub struct Listing {
    /// The directory, resolved (symlinks followed) — what the breadcrumb shows.
    pub path: String,
    pub parent: Option<String>,
    pub entries: Vec<Entry>,
    /// Read from the host because the sandbox is stopped: workspace only.
    pub offline: bool,
}

/// One directory: canonical path, NUL, then one NUL-terminated record per
/// entry — `%Y` target type, `%y` own type, size, mtime, name. Names may hold
/// anything but NUL and `/`, so the name goes last and the record ends at NUL.
const LIST_SCRIPT: &str = r#"cd -- "$1" 2>/dev/null || { echo "no such directory, or not readable" >&2; exit 3; }
pwd -P; printf '\0'
find . -mindepth 1 -maxdepth 1 -printf '%Y\t%y\t%s\t%T@\t%f\0' 2>/dev/null
exit 0"#;

/// A file's bytes, refused past `$2` bytes. `stat -L` follows a link, as `cat`
/// will.
const READ_SCRIPT: &str = r#"[ -f "$1" ] || { echo "not a file" >&2; exit 3; }
[ -r "$1" ] || { echo "not readable by the agent user" >&2; exit 3; }
s=$(stat -Lc %s -- "$1") || exit 3
[ "$s" -le "$2" ] || { echo "too large to open here ($s bytes)" >&2; exit 4; }
exec cat -- "$1""#;

/// The first `$2` bytes of a file, whatever its size — what a preview reads.
const HEAD_SCRIPT: &str = r#"[ -f "$1" ] || { echo "not a file" >&2; exit 3; }
[ -r "$1" ] || { echo "not readable by the agent user" >&2; exit 3; }
exec head -c "$2" -- "$1""#;


/// The reason a script gave on stderr, or a generic one: the `sbx exec` command
/// line is noise to someone browsing files.
fn script_error(out: &Output, what: &str) -> anyhow::Error {
    let err = String::from_utf8_lossy(&out.stderr);
    let last = err.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("");
    if last.is_empty() {
        anyhow::anyhow!("could not read {what}")
    } else {
        anyhow::anyhow!("{what}: {}", last.trim())
    }
}

/// List `dir` (absolute; empty means the workspace, or home without one).
pub async fn list(sbx: &Sbx, sandbox: &SandboxName, workspace: Option<&Path>, dir: &str) -> Result<Listing> {
    let dir = if dir.trim().is_empty() {
        workspace
            .map(|w| w.to_string_lossy().into_owned())
            .unwrap_or_else(|| SANDBOX_HOME.to_string())
    } else {
        dir.trim().to_string()
    };
    if !dir.starts_with('/') {
        bail!("{dir}: give an absolute path");
    }
    if sbx.is_running(sandbox).await {
        let out = sbx.exec(sandbox, &["sh", "-c", LIST_SCRIPT, "sh", &dir], None).await?;
        if !out.success() {
            return Err(script_error(&out, &dir));
        }
        let (path, entries) = parse_listing(&out.stdout);
        let path = if path.is_empty() { dir } else { path };
        return Ok(Listing {
            parent: parent_of(&path),
            path,
            entries,
            offline: false,
        });
    }
    let path = host_path(workspace, &dir)?;
    let mut entries: Vec<Entry> = std::fs::read_dir(&path)
        .with_context(|| format!("{}: not readable", path.display()))?
        .flatten()
        .map(|e| host_entry(&e))
        .collect();
    sort_entries(&mut entries);
    let path = path.to_string_lossy().into_owned();
    // The workspace's own parent is outside what a stopped sandbox can show.
    let parent = (Some(path.as_str()) != workspace.and_then(|w| w.to_str()))
        .then(|| parent_of(&path))
        .flatten();
    Ok(Listing {
        path,
        parent,
        entries,
        offline: true,
    })
}

/// A file's bytes, from inside the sandbox or — stopped — from the workspace.
///
/// Past `max` bytes the file is refused, unless `head` is set: then its first
/// `max` bytes come back instead, which is all a preview shows.
pub async fn read(
    sbx: &Sbx,
    sandbox: &SandboxName,
    workspace: Option<&Path>,
    file: &str,
    max: u64,
    head: bool,
) -> Result<Vec<u8>> {
    if !file.starts_with('/') {
        bail!("{file}: give an absolute path");
    }
    let max = max.min(MAX_FILE);
    if sbx.is_running(sandbox).await {
        let limit = max.to_string();
        let script = if head { HEAD_SCRIPT } else { READ_SCRIPT };
        let out = sbx.exec(sandbox, &["sh", "-c", script, "sh", file, &limit], None).await?;
        if !out.success() {
            return Err(script_error(&out, file));
        }
        return Ok(out.stdout);
    }
    let path = host_path(workspace, file)?;
    let meta = std::fs::metadata(&path).with_context(|| format!("{file}: not found"))?;
    if !meta.is_file() {
        bail!("{file}: not a file");
    }
    if meta.len() > max && !head {
        bail!("{file}: too large to open here ({} bytes)", meta.len());
    }
    let mut data = Vec::new();
    std::io::Read::read_to_end(
        &mut std::io::Read::take(
            std::fs::File::open(&path).with_context(|| format!("{file}: not readable"))?,
            max,
        ),
        &mut data,
    )?;
    Ok(data)
}

/// `p`, resolved on the host, provided it stays inside the workspace — the one
/// part of a stopped sandbox the host can answer for.
fn host_path(workspace: Option<&Path>, p: &str) -> Result<PathBuf> {
    let Some(ws) = workspace else {
        bail!("the sandbox is stopped and has no recorded workspace — start it to browse it");
    };
    let ws = ws
        .canonicalize()
        .with_context(|| format!("{}: workspace not found on the host", ws.display()))?;
    let resolved = Path::new(p)
        .canonicalize()
        .with_context(|| format!("{p}: not found"))?;
    if !resolved.starts_with(&ws) {
        bail!("the sandbox is stopped: only its workspace can be browsed — start it to see {p}");
    }
    Ok(resolved)
}

fn host_entry(e: &std::fs::DirEntry) -> Entry {
    let link = e.file_type().map(|t| t.is_symlink()).unwrap_or(false);
    // `metadata` follows links, like `find -printf %Y`.
    let meta = std::fs::metadata(e.path()).ok();
    let kind = match &meta {
        Some(m) if m.is_dir() => "dir",
        Some(m) if m.is_file() => "file",
        _ => "other",
    };
    let modified = meta.as_ref().map_or(0, mtime_secs);
    Entry {
        name: e.file_name().to_string_lossy().into_owned(),
        kind,
        link,
        size: meta.map(|m| m.len()).unwrap_or(0),
        modified,
    }
}

/// When a file was last modified, in Unix seconds; 0 when the platform cannot say.
pub fn mtime_secs(meta: &std::fs::Metadata) -> u64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn parent_of(path: &str) -> Option<String> {
    Path::new(path)
        .parent()
        .map(|p| p.to_string_lossy().into_owned())
}

/// Folders first, then by name, case-insensitively — how every file manager
/// orders a directory.
fn sort_entries(entries: &mut [Entry]) {
    entries.sort_by(|a, b| {
        (a.kind != "dir")
            .cmp(&(b.kind != "dir"))
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
}

/// Read `LIST_SCRIPT`'s output: the resolved directory, then its entries.
fn parse_listing(raw: &[u8]) -> (String, Vec<Entry>) {
    let mut records = raw.split(|b| *b == 0);
    let path = records
        .next()
        .map(|p| String::from_utf8_lossy(p).trim().to_string())
        .unwrap_or_default();
    let mut entries: Vec<Entry> = records
        .filter(|r| !r.is_empty())
        .filter_map(|r| {
            let text = String::from_utf8_lossy(r);
            let mut f = text.splitn(5, '\t');
            let target = f.next()?;
            let own = f.next()?;
            let size = f.next()?.parse().unwrap_or(0);
            let modified = f.next()?.split('.').next()?.parse().unwrap_or(0);
            let name = f.next()?.to_string();
            let kind = match target {
                "d" => "dir",
                "f" => "file",
                _ => "other",
            };
            Some(Entry {
                name,
                kind,
                link: own == "l",
                size,
                modified,
            })
        })
        .collect();
    sort_entries(&mut entries);
    (path, entries)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_listing_reads_path_then_records_with_folders_first() {
        let raw = b"/home/agent\n\0f\tf\t12\t1700000000.5\tnotes.txt\0d\td\t4096\t1700000001.0\t.cache\0d\tl\t9\t1700000002.0\tlinked dir\0N\tl\t7\t1700000003.0\tdangling\0f\tf\t3\t1700000004.0\twith\ttab\0";
        let (path, entries) = parse_listing(raw);
        assert_eq!(path, "/home/agent");
        let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, [".cache", "linked dir", "dangling", "notes.txt", "with\ttab"]);
        let linked = &entries[1];
        assert_eq!((linked.kind, linked.link), ("dir", true));
        assert_eq!(entries[2].kind, "other");
        assert_eq!(entries[3].modified, 1700000000);
        assert_eq!(entries[3].size, 12);
    }

    #[test]
    fn an_empty_directory_is_its_path_and_nothing_else() {
        let (path, entries) = parse_listing(b"/tmp/empty\n\0");
        assert_eq!(path, "/tmp/empty");
        assert!(entries.is_empty());
    }

    /// Stopped, only the workspace is answered for — and `..` is no way out.
    #[test]
    fn a_stopped_sandbox_shows_its_workspace_and_nothing_beside_it() {
        let root = std::env::temp_dir().join(format!("sbxw-fs-{}", std::process::id()));
        let ws = root.join("ws");
        std::fs::create_dir_all(ws.join("src")).unwrap();
        std::fs::write(root.join("secret"), b"x").unwrap();
        assert!(host_path(Some(&ws), ws.join("src").to_str().unwrap()).is_ok());
        let escape = format!("{}/../secret", ws.display());
        assert!(host_path(Some(&ws), &escape).is_err());
        assert!(host_path(None, ws.to_str().unwrap()).is_err());
        let _ = std::fs::remove_dir_all(&root);
    }
}
