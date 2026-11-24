//! The Project panel's file explorer: a sandbox's own filesystem, read-only.
//!
//! The artifacts listing reads `.sbxw-artifacts` from the host, through the
//! workspace mount, which is all a list of deliverables needs. An explorer needs
//! the rest (`/home/agent`, `/etc`, a tool's cache), which exist only inside the
//! container. So a **running** sandbox is read through `sbx exec`: one `find`
//! per directory, one `cat` per file.
//!
//! A **stopped** sandbox cannot be exec'd into, and starting one to show a
//! directory listing is not a call a file browser gets to make. What is still
//! readable is the workspace, from the host: in direct mode it is mounted at the
//! same absolute path, so the panel's paths are the ones the agent sees. The
//! listing says which of the two it is (`offline`), and anything outside the
//! workspace is refused rather than read off the host's disk under a sandbox's
//! name.
//!
//! It reads, with one exception: `delete`, for the file an agent left where it
//! should not have. Only under the places an agent writes to (the workspace, the
//! read-write mounts, home and `/tmp`), and never one of those itself.

use anyhow::{bail, Context, Result};
use serde::Serialize;
use std::path::{Path, PathBuf};

use crate::host::{Host, Kind, Meta};
use sbxw_core::SandboxName;
use sbxw_sbx::host::HostCmd;
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


/// The largest folder archive (`<folder>.tar.gz`) handed over in one answer:
/// [`MAX_FILE`], for the same reason. The exec path buffers a command's whole
/// output on both sides, so this is a memory bound too.
pub const MAX_ARCHIVE: u64 = MAX_FILE;

/// What a folder holds, counted without reading it: regular files under it
/// (links not followed) and their bytes.
#[derive(Serialize, Debug, PartialEq, Clone, Copy)]
pub struct FolderSize {
    pub files: u64,
    pub bytes: u64,
}

/// `$1` resolved (`d`), then its files (`n`) and their bytes (`s`): GNU `find
/// -printf` in a sandbox, `find | wc` and `du -sk` (an upper bound) where
/// `-printf` is missing, as on a Mac host. `find` never follows a link.
const SIZE_PART: &str = r#"d=$(cd -P -- "$1" 2>/dev/null && pwd -P) || { echo "no such folder, or not readable" >&2; exit 3; }
cd -- "$d" || exit 3
if find . -maxdepth 0 -printf '' >/dev/null 2>&1; then
  set -- $(find . -type f -printf '%s\n' 2>/dev/null | awk '{n++; s+=$1} END {printf "%.0f %.0f\n", n, s}') "$@"
else
  k=$(du -sk . 2>/dev/null | cut -f1)
  set -- "$(find . -type f 2>/dev/null | wc -l | tr -d ' ')" "$(( ${k:-0} * 1024 ))" "$@"
fi
n=$1; s=$2; shift 2"#;

/// A folder's size: `files bytes`.
fn size_script() -> String {
    format!("{SIZE_PART}\necho \"$n $s\"")
}

/// `$1` as a gzip tar holding the folder by its own name, refused when its
/// files hold more than `$2` bytes (the size and the cap said); the archive is
/// cut at `$3` (`$2 + 1`) bytes, which the caller refuses, and tar failing is a
/// failure, never a short archive. Links are stored as links (no `-h`). The
/// path is an argument, never part of the script.
fn archive_script() -> String {
    format!(
        "{SIZE_PART}\n{}",
        r#"[ "$d" != / ] || { echo "the whole filesystem is not something to download" >&2; exit 3; }
[ "$s" -le "$2" ] || { echo "too large to download here: $s bytes ($n files), over the $2 bytes a folder download allows" >&2; exit 4; }
p=$(dirname -- "$d"); b=$(basename -- "$d")
case "$b" in -*) b="./$b" ;; esac
cd -- "$p" || exit 3
exec 4>&1
st=$( { { tar -czf - "$b"; echo $? >&3; } | head -c "$3" >&4; } 3>&1 )
[ "$st" = 0 ] && exit 0
[ "$st" -gt 128 ] 2>/dev/null && { echo "the archive passed the $2 bytes a folder download allows: not downloaded" >&2; exit 4; }
echo "tar could not pack all of it (status $st): a file the agent user cannot read, or one that changed while it was read" >&2
exit 5"#
    )
}

/// The file name a folder downloads under: its last component, `.tar.gz`
/// added, quotes and line breaks dropped (the client cleans it again).
pub fn archive_name(path: &str) -> String {
    let base = path.trim_end_matches('/').rsplit('/').next().unwrap_or("");
    let base: String = base.chars().filter(|c| !matches!(c, '"' | '\\' | '\r' | '\n' | '/')).collect();
    let base = if base.is_empty() || base == "." || base == ".." { "folder".to_string() } else { base };
    format!("{base}.tar.gz")
}

/// Whether an archive error is the cap (the answer is then 413, not 404).
pub fn is_too_large(e: &anyhow::Error) -> bool {
    let m = format!("{e:#}");
    m.contains("too large to download here") || m.contains("a folder download allows")
}

/// Folders a scan for recent files does not descend into: version control,
/// dependencies, build output, tool caches, and the agent's own transcripts,
/// which change every turn. What is left is what someone, usually the agent,
/// chose to write. Names, matched as `find -name` does; a trailing `*` is the
/// only wildcard, and the only one the host-side walk understands.
pub const RECENT_PRUNE: &[&str] = &[
    ".git", "node_modules", "target", ".cache", ".npm", ".cargo", ".rustup", ".nvm",
    ".local", ".claude", ".sbxw", "__pycache__", ".venv", "venv", ".gradle", ".m2",
    ".pnpm-store", ".yarn", ".next", ".turbo", ".sbxw-worktrees",
    // sbxw's and Claude Code's own bookkeeping under /tmp.
    "sbxw-*", "bash-edit-diff",
];

fn pruned(name: &std::ffi::OsStr) -> bool {
    let name = name.to_string_lossy();
    RECENT_PRUNE.iter().any(|p| match p.strip_suffix('*') {
        Some(prefix) => name.starts_with(prefix),
        None => name == *p,
    })
}

/// How far back "recent" reaches, and how many files it lists at most.
pub const RECENT_DAYS: u64 = 7;
pub const RECENT_MAX: usize = 300;

/// A file written lately, anywhere the scan looked.
#[derive(Serialize, Debug, PartialEq)]
pub struct Recent {
    pub path: String,
    pub size: u64,
    /// Unix seconds.
    pub modified: u64,
}

/// Every regular file under the roots (`$4`…) modified in the last `$1`
/// minutes, outside the pruned folders (`$2`, space-separated), newest first,
/// at most `$3`: mtime, size, path, each record NUL-terminated. `sort -n` reads
/// the leading mtime and stops at the tab after it.
const RECENT_SCRIPT: &str = r#"mins=$1; prune=$2; max=$3; shift 3
set -f; p=''; for n in $prune; do p="$p -name $n -o"; done; set +f
for r in "$@"; do [ -d "$r" ] && find "$r" -xdev \( ${p% -o} \) -prune -o -type f -mmin "-$mins" -printf '%T@\t%s\t%p\0' 2>/dev/null; done |
  sort -z -rn | head -z -n "$max"
exit 0"#;

/// Remove `$1` (a file, a link or a whole folder) if it lies strictly inside one
/// of the roots (`$2`…). The check is made on the resolved parent, so a
/// symlinked folder is no way out; the last component is not followed, so
/// deleting a link removes the link.
const DELETE_SCRIPT: &str = r#"f=$1; shift
d=$(cd -P -- "$(dirname -- "$f")" 2>/dev/null && pwd) || { echo "no such folder" >&2; exit 3; }
b=$(basename -- "$f")
real="${d%/}/$b"
[ -e "$real" ] || [ -L "$real" ] || { echo "not found" >&2; exit 3; }
ok=''
for r in "$@"; do
  rr=$(cd -P -- "$r" 2>/dev/null && pwd) || continue
  case "$real" in "${rr%/}"/?*) ok=1 ;; esac
done
[ -n "$ok" ] || { echo "outside the folders files can be deleted from" >&2; exit 4; }
rm -rf -- "$real" || exit 5
exit 0"#;

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
pub async fn list(sbx: &Sbx, host: &Host, sandbox: &SandboxName, workspace: Option<&Path>, dir: &str) -> Result<Listing> {
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
    let path = host_path(host, workspace, &dir).await?;
    let raw = host.fs.read_dir(&path).await.with_context(|| format!("{}: not readable", path.display()))?;
    let mut entries = Vec::with_capacity(raw.len());
    for e in raw {
        entries.push(host_entry(host, &path, e).await);
    }
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

/// Files written lately across `roots` (the workspace, home, `/tmp`, the
/// read-write mounts: wherever an agent may have put something). Running, one
/// `find` inside the sandbox; stopped, a walk of the workspace on the host.
pub async fn recent(sbx: &Sbx, host: &Host, sandbox: &SandboxName, workspace: Option<&Path>, roots: &[String]) -> Result<(Vec<Recent>, bool)> {
    let since = now_secs().saturating_sub(RECENT_DAYS * 86_400);
    if sbx.is_running(sandbox).await {
        let mins = (RECENT_DAYS * 24 * 60).to_string();
        let prune = RECENT_PRUNE.join(" ");
        let max = RECENT_MAX.to_string();
        let mut args = vec!["sh", "-c", RECENT_SCRIPT, "sh", &mins, &prune, &max];
        args.extend(roots.iter().map(String::as_str));
        let out = sbx.exec(sandbox, &args, None).await?;
        if !out.success() {
            return Err(script_error(&out, "recent files"));
        }
        return Ok((dedup_recent(parse_recent(&out.stdout)), false));
    }
    let Some(ws) = workspace else {
        bail!("the sandbox is stopped and has no recorded workspace — start it to see its files");
    };
    let mut found = walk_recent(host, ws, since).await?;
    found.sort_by_key(|r| std::cmp::Reverse(r.modified));
    found.truncate(RECENT_MAX);
    Ok((found, true))
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// The host-side scan of a stopped sandbox's workspace. Symlinks are not
/// followed, so it cannot leave the workspace. The prune list is applied twice:
/// by exact name, to stop the walk descending, and here for the `sbxw-*` prefix
/// the walk cannot express.
async fn walk_recent(host: &Host, dir: &Path, since: u64) -> Result<Vec<Recent>> {
    let exact: Vec<&str> = RECENT_PRUNE.iter().copied().filter(|p| !p.ends_with('*')).collect();
    let all = host.fs.walk(dir, 32, &exact).await.with_context(|| format!("{}: not readable", dir.display()))?;
    Ok(all
        .into_iter()
        .filter(|(rel, m)| m.kind == Kind::File && m.mtime_secs >= since && !rel.split('/').any(|c| pruned(std::ffi::OsStr::new(c))))
        .map(|(rel, m)| Recent { path: dir.join(rel).to_string_lossy().into_owned(), size: m.len, modified: m.mtime_secs })
        .collect())
}

/// Read `RECENT_SCRIPT`'s records. Paths may hold tabs, so the path goes last.
fn parse_recent(raw: &[u8]) -> Vec<Recent> {
    raw.split(|b| *b == 0)
        .filter(|r| !r.is_empty())
        .filter_map(|r| {
            let text = String::from_utf8_lossy(r);
            let mut f = text.splitn(3, '\t');
            let modified = f.next()?.split('.').next()?.parse().ok()?;
            let size = f.next()?.parse().unwrap_or(0);
            let path = f.next()?.to_string();
            Some(Recent { path, size, modified })
        })
        .collect()
}

/// Overlapping roots (a workspace under home) find the same file twice; the
/// first, newest-ordered, copy is kept.
fn dedup_recent(list: Vec<Recent>) -> Vec<Recent> {
    let mut seen = std::collections::HashSet::new();
    list.into_iter().filter(|r| seen.insert(r.path.clone())).collect()
}

/// A file's bytes, from inside the sandbox or — stopped — from the workspace.
///
/// Past `max` bytes the file is refused, unless `head` is set: then its first
/// `max` bytes come back instead, which is all a preview shows.
pub async fn read(
    sbx: &Sbx,
    host: &Host,
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
    let path = host_path(host, workspace, file).await?;
    let meta = host.fs.stat(&path).await?.with_context(|| format!("{file}: not found"))?;
    if meta.kind != Kind::File {
        bail!("{file}: not a file");
    }
    if meta.len > max && !head {
        bail!("{file}: too large to open here ({} bytes)", meta.len);
    }
    if meta.len > max {
        // A preview: the first `max` bytes of a longer file, which `HostFs::read`
        // refuses by contract, so the host's own `head` cuts it.
        let limit = max.to_string();
        let cmd = HostCmd::new("sh", ["-c", HEAD_SCRIPT, "sh", &path.to_string_lossy(), &limit].map(String::from).to_vec());
        let out = host.exec.exec(cmd).await?;
        if !out.success() {
            return Err(script_error(&out, file));
        }
        return Ok(out.stdout);
    }
    host.fs.read(&path, max).await.with_context(|| format!("{file}: not readable"))
}

/// The folder `dir` from the host, for a stopped sandbox: resolved, inside the
/// workspace (as `read` requires), and a folder.
async fn host_folder(host: &Host, workspace: Option<&Path>, dir: &str) -> Result<PathBuf> {
    let path = host_path(host, workspace, dir).await?;
    let meta = host.fs.stat(&path).await?.with_context(|| format!("{dir}: not found"))?;
    if meta.kind != Kind::Dir {
        bail!("{dir}: not a folder");
    }
    Ok(path)
}

/// Run a folder script: inside the sandbox when it runs, else on the host on
/// the resolved workspace path. Exactly `read`'s two branches.
async fn folder_script(
    sbx: &Sbx,
    host: &Host,
    sandbox: &SandboxName,
    workspace: Option<&Path>,
    dir: &str,
    script: &str,
    extra: &[&str],
) -> Result<Vec<u8>> {
    if !dir.starts_with('/') {
        bail!("{dir}: give an absolute path");
    }
    let out = if sbx.is_running(sandbox).await {
        let mut args = vec!["sh", "-c", script, "sh", dir];
        args.extend_from_slice(extra);
        sbx.exec(sandbox, &args, None).await?
    } else {
        let path = host_folder(host, workspace, dir).await?;
        let mut args: Vec<String> = ["-c", script, "sh"].map(String::from).to_vec();
        args.push(path.to_string_lossy().into_owned());
        args.extend(extra.iter().map(|s| s.to_string()));
        host.exec.exec(HostCmd::new("sh", args)).await?
    };
    if !out.success() {
        return Err(script_error(&out, dir));
    }
    Ok(out.stdout)
}

/// How many files a folder holds and their bytes, without reading them.
pub async fn folder_size(sbx: &Sbx, host: &Host, sandbox: &SandboxName, workspace: Option<&Path>, dir: &str) -> Result<FolderSize> {
    let out = folder_script(sbx, host, sandbox, workspace, dir, &size_script(), &[]).await?;
    parse_size(&out).with_context(|| format!("{dir}: could not count its files"))
}

fn parse_size(raw: &[u8]) -> Option<FolderSize> {
    let s = String::from_utf8_lossy(raw);
    let mut it = s.split_whitespace();
    Some(FolderSize { files: it.next()?.parse().ok()?, bytes: it.next()?.parse().ok()? })
}

/// A folder as `.tar.gz` bytes, the folder by its own name inside. Sized first
/// and refused past `max` bytes ([`MAX_ARCHIVE`] at most), with the size in
/// the error; an archive that still grows past `max` is refused, never cut.
pub async fn archive(sbx: &Sbx, host: &Host, sandbox: &SandboxName, workspace: Option<&Path>, dir: &str, max: u64) -> Result<Vec<u8>> {
    let max = max.min(MAX_ARCHIVE);
    let (cap, cut) = (max.to_string(), (max + 1).to_string());
    let out = folder_script(sbx, host, sandbox, workspace, dir, &archive_script(), &[&cap, &cut]).await?;
    if out.len() as u64 > max {
        bail!("{dir}: the archive passed the {max} bytes a folder download allows: not downloaded");
    }
    Ok(out)
}

/// Why `path` may not be deleted, if it may not: it is relative, climbs with
/// `..`, is not inside one of `roots`, or is one of `protected` (a root itself,
/// the deliverables folder, the repository's history).
pub fn refuse_delete(path: &str, roots: &[String], protected: &[String]) -> Option<String> {
    if !path.starts_with('/') {
        return Some(format!("{path}: give an absolute path"));
    }
    if path.split('/').any(|c| c == ".." || c == ".") {
        return Some(format!("{path}: give the path without . or .."));
    }
    let path = path.trim_end_matches('/');
    let under = |root: &str| {
        let root = root.trim_end_matches('/');
        path.len() > root.len() + 1 && path.starts_with(root) && path.as_bytes()[root.len()] == b'/'
    };
    if roots.iter().any(|r| r.trim_end_matches('/') == path) || protected.iter().any(|p| p.trim_end_matches('/') == path) {
        return Some(format!("{path} is not something to delete from here"));
    }
    if !roots.iter().any(|r| under(r)) {
        return Some(format!(
            "{path}: only files under the workspace, a read-write mount, {SANDBOX_HOME} or /tmp can be deleted"
        ));
    }
    None
}

/// Delete `path` (file, link or folder with everything in it) once
/// `refuse_delete` has nothing against it. Running, inside the sandbox, where
/// the script checks again on the resolved path; stopped, on the host, inside
/// the workspace only.
pub async fn delete(
    sbx: &Sbx,
    host: &Host,
    sandbox: &SandboxName,
    workspace: Option<&Path>,
    path: &str,
    roots: &[String],
    protected: &[String],
) -> Result<()> {
    if let Some(why) = refuse_delete(path, roots, protected) {
        bail!(why);
    }
    let path = path.trim_end_matches('/');
    if sbx.is_running(sandbox).await {
        let mut args = vec!["sh", "-c", DELETE_SCRIPT, "sh", path];
        args.extend(roots.iter().map(String::as_str));
        let out = sbx.exec(sandbox, &args, None).await?;
        if !out.success() {
            return Err(script_error(&out, path));
        }
        return Ok(());
    }
    let Some(ws) = workspace else {
        bail!("the sandbox is stopped and has no recorded workspace — start it to delete files");
    };
    let target = host_delete_target(host, ws, path).await?;
    host.fs.lstat(&target).await?.with_context(|| format!("{path}: not found"))?;
    host.fs.remove(&target).await.with_context(|| format!("{path}: could not delete it"))
}

/// The host path a stopped sandbox's `path` names, its parent resolved and
/// checked to be the workspace or inside it. The last component is left alone,
/// so a link is removed rather than its target.
async fn host_delete_target(host: &Host, ws: &Path, path: &str) -> Result<PathBuf> {
    let ws = host
        .fs
        .canonicalize(ws)
        .await
        .with_context(|| format!("{}: workspace not found on the host", ws.display()))?;
    let p = Path::new(path);
    let (Some(parent), Some(name)) = (p.parent(), p.file_name()) else {
        bail!("{path}: not something to delete");
    };
    let parent = host.fs.canonicalize(parent).await.with_context(|| format!("{path}: not found"))?;
    if !parent.starts_with(&ws) {
        bail!("the sandbox is stopped: only files in its workspace can be deleted — start it to delete {path}");
    }
    Ok(parent.join(name))
}

/// `p`, resolved on the host, provided it stays inside the workspace: the one
/// part of a stopped sandbox the host can answer for.
async fn host_path(host: &Host, workspace: Option<&Path>, p: &str) -> Result<PathBuf> {
    let Some(ws) = workspace else {
        bail!("the sandbox is stopped and has no recorded workspace — start it to browse it");
    };
    let ws = host
        .fs
        .canonicalize(ws)
        .await
        .with_context(|| format!("{}: workspace not found on the host", ws.display()))?;
    let resolved = host.fs.canonicalize(Path::new(p)).await.with_context(|| format!("{p}: not found"))?;
    if !resolved.starts_with(&ws) {
        bail!("the sandbox is stopped: only its workspace can be browsed — start it to see {p}");
    }
    Ok(resolved)
}

/// One entry of a stopped sandbox's listing. A link's kind and size are its
/// target's, like `find -printf %Y`; a dangling one is `other`.
async fn host_entry(host: &Host, dir: &Path, e: crate::host::DirEntry) -> Entry {
    let link = e.meta.kind == Kind::Symlink;
    let meta: Option<Meta> = if link { host.fs.stat(&dir.join(&e.name)).await.ok().flatten() } else { Some(e.meta) };
    let kind = match meta.map(|m| m.kind) {
        Some(Kind::Dir) => "dir",
        Some(Kind::File) => "file",
        _ => "other",
    };
    Entry {
        name: e.name,
        kind,
        link,
        size: meta.map_or(0, |m| m.len),
        modified: meta.map_or(0, |m| m.mtime_secs),
    }
}

fn parent_of(path: &str) -> Option<String> {
    Path::new(path)
        .parent()
        .map(|p| p.to_string_lossy().into_owned())
}

/// Folders first, then by name, case-insensitively: how every file manager
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
    use crate::testing::hosts;

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
    fn recent_files_read_mtime_size_path_and_drop_duplicates() {
        let raw = b"1700000005.25\t10\t/w/odd place/new.md\0garbage\x001700000001.0\t3\t/w/a\tb\x001700000001.0\t3\t/w/a\tb\0";
        let list = dedup_recent(parse_recent(raw));
        assert_eq!(list.len(), 2);
        assert_eq!(list[0], Recent { path: "/w/odd place/new.md".into(), size: 10, modified: 1700000005 });
        assert_eq!(list[1].path, "/w/a\tb");
    }

    /// A stopped sandbox: no `sbx exec`, so everything is the host's disk.
    fn stopped() -> Sbx {
        Sbx::new(sbxw_sbx::runner::Fake::new(|_| Output::ok("[]")))
    }

    fn now_secs() -> u64 {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs()
    }

    /// Stopped, the workspace is walked on the host: newest first, pruned
    /// folders skipped, old files left out.
    #[tokio::test]
    async fn a_stopped_sandbox_lists_recent_workspace_files_outside_pruned_folders() {
        for (n, h) in hosts() {
            let ws = tempfile::tempdir().unwrap();
            let ws = ws.path();
            std::fs::create_dir_all(ws.join("deep/er")).unwrap();
            std::fs::create_dir_all(ws.join("node_modules/x")).unwrap();
            std::fs::create_dir_all(ws.join("sbxw-scratch")).unwrap();
            std::fs::write(ws.join("deep/er/stray.txt"), b"hi").unwrap();
            std::fs::write(ws.join("node_modules/x/index.js"), b"").unwrap();
            std::fs::write(ws.join("sbxw-scratch/t"), b"").unwrap();
            let found = walk_recent(&h, ws, now_secs() - 60).await.unwrap();
            let names: Vec<String> = found.iter().map(|r| r.path.clone()).collect();
            assert_eq!(names, [ws.join("deep/er/stray.txt").to_string_lossy().into_owned()], "{n}");
            assert!(walk_recent(&h, ws, now_secs() + 3600).await.unwrap().is_empty(), "{n}");
            let (via_api, offline) = recent(&stopped(), &h, &SandboxName::parse("s").unwrap(), Some(ws), &[]).await.unwrap();
            assert!(offline && via_api.len() == 1, "{n}");
        }
    }

    #[test]
    fn an_empty_directory_is_its_path_and_nothing_else() {
        let (path, entries) = parse_listing(b"/tmp/empty\n\0");
        assert_eq!(path, "/tmp/empty");
        assert!(entries.is_empty());
    }

    #[test]
    fn deleting_is_refused_outside_the_writable_places_and_on_their_roots() {
        let roots = vec!["/w".to_string(), SANDBOX_HOME.to_string(), "/tmp".to_string()];
        let protected = vec!["/w/.sbxw-artifacts".to_string(), "/w/.git".to_string()];
        assert_eq!(refuse_delete("/w/stray.md", &roots, &protected), None);
        assert_eq!(refuse_delete("/w/.sbxw-artifacts/old.html", &roots, &protected), None);
        assert_eq!(refuse_delete("/tmp/x/", &roots, &protected), None);
        for bad in ["/w", "/w/", "/tmp", "/home/agent", "/w/.git", "/w/.sbxw-artifacts",
                    "/etc/passwd", "/wx/file", "/w/../etc", "relative", "/", "/w/./a"] {
            assert!(refuse_delete(bad, &roots, &protected).is_some(), "{bad} should be refused");
        }
    }

    /// Stopped, a file in the workspace goes, a link goes without its target,
    /// and a path whose parent leaves the workspace is refused.
    #[tokio::test]
    async fn a_stopped_sandbox_deletes_inside_its_workspace_only() {
        for (n, h) in hosts() {
            let root = tempfile::tempdir().unwrap();
            let root = std::fs::canonicalize(root.path()).unwrap();
            let ws = root.join("ws");
            std::fs::create_dir_all(ws.join("dir/sub")).unwrap();
            std::fs::write(root.join("outside"), b"x").unwrap();
            std::fs::write(ws.join("dir/sub/f"), b"x").unwrap();
            std::os::unix::fs::symlink(root.join("outside"), ws.join("link")).unwrap();
            let sb = SandboxName::parse("s").unwrap();
            let roots = vec![ws.to_string_lossy().into_owned()];
            let del = |p: PathBuf| {
                let (h, sb, roots, ws) = (h.clone(), sb.clone(), roots.clone(), ws.clone());
                async move { delete(&stopped(), &h, &sb, Some(&ws), p.to_str().unwrap(), &roots, &[]).await }
            };
            del(ws.join("dir")).await.unwrap();
            assert!(!ws.join("dir").exists(), "{n}");
            del(ws.join("link")).await.unwrap();
            assert!(!ws.join("link").exists() && root.join("outside").exists(), "{n}: the link goes, not its target");
            assert!(del(root.join("outside")).await.is_err(), "{n}");
            assert!(root.join("outside").exists(), "{n}");
            assert!(del(ws.join("nope")).await.is_err(), "{n}");
        }
    }

    /// Stopped, only the workspace is answered for — and `..` is no way out.
    #[tokio::test]
    async fn a_stopped_sandbox_shows_its_workspace_and_nothing_beside_it() {
        for (n, h) in hosts() {
            let root = tempfile::tempdir().unwrap();
            let root = std::fs::canonicalize(root.path()).unwrap();
            let ws = root.join("ws");
            std::fs::create_dir_all(ws.join("src")).unwrap();
            std::fs::write(ws.join("src/a.txt"), b"hello world").unwrap();
            std::fs::write(root.join("secret"), b"x").unwrap();
            std::os::unix::fs::symlink(ws.join("src"), ws.join("lnk")).unwrap();
            std::os::unix::fs::symlink(ws.join("gone"), ws.join("dangling")).unwrap();
            assert!(host_path(&h, Some(&ws), ws.join("src").to_str().unwrap()).await.is_ok(), "{n}");
            let escape = format!("{}/../secret", ws.display());
            assert!(host_path(&h, Some(&ws), &escape).await.is_err(), "{n}");
            assert!(host_path(&h, None, ws.to_str().unwrap()).await.is_err(), "{n}");

            let (sbx, sb) = (stopped(), SandboxName::parse("s").unwrap());
            let l = list(&sbx, &h, &sb, Some(&ws), "").await.unwrap();
            assert!(l.offline && l.parent.is_none(), "{n}: the workspace has no parent here");
            let shown: Vec<(&str, &str, bool)> = l.entries.iter().map(|e| (e.name.as_str(), e.kind, e.link)).collect();
            assert_eq!(shown, [("lnk", "dir", true), ("src", "dir", false), ("dangling", "other", true)], "{n}");
            assert!(list(&sbx, &h, &sb, Some(&ws), root.to_str().unwrap()).await.is_err(), "{n}");

            let file = ws.join("src/a.txt");
            let f = file.to_str().unwrap();
            assert_eq!(read(&sbx, &h, &sb, Some(&ws), f, 100, false).await.unwrap(), b"hello world", "{n}");
            assert!(read(&sbx, &h, &sb, Some(&ws), f, 5, false).await.is_err(), "{n}: too large");
            assert_eq!(read(&sbx, &h, &sb, Some(&ws), f, 5, true).await.unwrap(), b"hello", "{n}: a preview is cut");
            assert!(read(&sbx, &h, &sb, Some(&ws), ws.join("src").to_str().unwrap(), 5, false).await.is_err(), "{n}");
            assert!(read(&sbx, &h, &sb, Some(&ws), root.join("secret").to_str().unwrap(), 5, false).await.is_err(), "{n}");
        }
    }

    /// A running sandbox: the folder is packed by `tar` through `sbx exec`,
    /// the path and the caps as arguments after the script, never inside it.
    #[tokio::test]
    async fn a_running_sandbox_packs_a_folder_with_its_path_as_an_argument() {
        let fake = sbxw_sbx::runner::Fake::new(|a| match a.first().map(String::as_str) {
            Some("ls") => Output::ok(r#"[{"name":"s","agent":"claude","status":"running"}]"#),
            _ => Output::ok("GZ"),
        });
        let sbx = Sbx::new(fake.clone());
        let (_, h) = hosts().remove(0);
        let sb = SandboxName::parse("s").unwrap();
        let dir = "/home/agent/my $(x) dir";
        assert_eq!(archive(&sbx, &h, &sb, None, dir, 1000).await.unwrap(), b"GZ");
        let call = fake.calls.lock().unwrap().iter().find(|c| c[0] == "exec").cloned().unwrap();
        assert_eq!(&call[..5], ["exec", "s", "--", "sh", "-c"]);
        assert!(call[5].contains("tar -czf - \"$b\"") && !call[5].contains("my $(x) dir"), "the path is never in the script");
        assert!(!call[5].contains("tar -h") && !call[5].contains("-czhf"), "links are not followed");
        assert_eq!(&call[6..], ["sh", dir, "1000", "1001"]);
        // The cap is MAX_ARCHIVE at most, whatever is asked.
        archive(&sbx, &h, &sb, None, dir, u64::MAX).await.unwrap();
        let last = fake.calls.lock().unwrap().last().cloned().unwrap();
        assert_eq!(last[last.len() - 2], MAX_ARCHIVE.to_string());
        // An answer longer than the cap is refused, never handed over cut.
        let long = sbxw_sbx::runner::Fake::new(|a| match a.first().map(String::as_str) {
            Some("ls") => Output::ok(r#"[{"name":"s","agent":"claude","status":"running"}]"#),
            _ => Output::ok("0123456789"),
        });
        let e = archive(&Sbx::new(long), &h, &sb, None, dir, 5).await.unwrap_err();
        assert!(is_too_large(&e), "{e:#}");
        assert!(archive(&sbx, &h, &sb, None, "relative/dir", 5).await.is_err());
    }

    #[test]
    fn a_folder_downloads_under_its_own_name_with_tar_gz() {
        assert_eq!(archive_name("/home/agent/proj"), "proj.tar.gz");
        assert_eq!(archive_name("/home/agent/proj/"), "proj.tar.gz");
        assert_eq!(archive_name("/w/we\"ird\nname"), "weirdname.tar.gz");
        assert_eq!(archive_name("/"), "folder.tar.gz");
        assert_eq!(parse_size(b"3 1200\n"), Some(FolderSize { files: 3, bytes: 1200 }));
        assert_eq!(parse_size(b""), None);
    }

    fn untar_list(gz: &[u8]) -> String {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("a.tar.gz");
        std::fs::write(&f, gz).unwrap();
        let out = std::process::Command::new("tar").arg("-tvzf").arg(&f).output().unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    /// Stopped, on both kinds of host: the workspace folder is packed by its
    /// name, a link inside it stays a link (its target outside is not packed),
    /// the cap is refused with the size, and what `read` refuses (outside the
    /// workspace, a relative path) this refuses, as it does a file.
    #[tokio::test]
    async fn a_stopped_sandbox_packs_a_workspace_folder_keeping_links_as_links() {
        for (n, h) in hosts() {
            let root = tempfile::tempdir().unwrap();
            let root = std::fs::canonicalize(root.path()).unwrap();
            let ws = root.join("ws");
            std::fs::create_dir_all(ws.join("out/sub")).unwrap();
            std::fs::write(ws.join("out/a.txt"), b"hello world").unwrap();
            std::fs::write(ws.join("out/sub/b.txt"), vec![b'x'; 100]).unwrap();
            std::fs::create_dir_all(root.join("secrets")).unwrap();
            std::fs::write(root.join("secrets/key"), vec![b'k'; 5000]).unwrap();
            std::os::unix::fs::symlink(root.join("secrets"), ws.join("out/escape")).unwrap();
            let (sbx, sb) = (stopped(), SandboxName::parse("s").unwrap());
            let out = ws.join("out");
            let o = out.to_str().unwrap();

            let size = folder_size(&sbx, &h, &sb, Some(&ws), o).await.unwrap();
            assert_eq!(size.files, 2, "{n}: the link's target is not counted");
            assert!(size.bytes >= 111 && size.bytes < 5000, "{n}: {size:?}");

            let gz = archive(&sbx, &h, &sb, Some(&ws), o, 1 << 20).await.unwrap();
            assert_eq!(&gz[..2], [0x1f, 0x8b], "{n}: gzip");
            let listing = untar_list(&gz);
            assert!(listing.contains("out/a.txt") && listing.contains("out/sub/b.txt"), "{n}: {listing}");
            assert!(listing.lines().any(|l| l.starts_with('l') && l.contains("out/escape")), "{n}: a link: {listing}");
            assert!(!listing.contains("key"), "{n}: never followed out: {listing}");

            let e = archive(&sbx, &h, &sb, Some(&ws), o, 50).await.unwrap_err();
            let m = format!("{e:#}");
            assert!(is_too_large(&e) && m.contains(&format!("{} bytes (2 files)", size.bytes)) && m.contains("50 bytes"), "{n}: {m}");

            assert!(archive(&sbx, &h, &sb, Some(&ws), root.join("secrets").to_str().unwrap(), 1 << 20).await.is_err(), "{n}: outside the workspace");
            assert!(archive(&sbx, &h, &sb, Some(&ws), out.join("escape").to_str().unwrap(), 1 << 20).await.is_err(), "{n}: a link out of it");
            assert!(archive(&sbx, &h, &sb, Some(&ws), out.join("a.txt").to_str().unwrap(), 1 << 20).await.is_err(), "{n}: a file");
            assert!(archive(&sbx, &h, &sb, Some(&ws), "out", 1 << 20).await.is_err(), "{n}: relative");
            assert!(archive(&sbx, &h, &sb, None, o, 1 << 20).await.is_err(), "{n}: no workspace");
        }
    }
}
