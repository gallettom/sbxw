//! The git repositories under a sandbox's workspace, and which one each thread
//! works in.
//!
//! A workspace is not always a repository. It is often a folder that gathers
//! several (`~/dev/shop/{backend,frontend}`), and a thread needs one for its
//! worktree (`sbx::thread_run_args`). So a thread is opened *in a repo*, named
//! relative to the workspace (`backend`, or `''` for the workspace itself),
//! and that choice is remembered: its worktree lives in that repo, and every
//! later open, its Bash and its close must find it there again.
//!
//! Everything here reads the **host** side of the mount. The workspace is
//! mounted at the same path in the sandbox, so a path found here is the one the
//! scripts in the sandbox `cd` into — and the scan works on a stopped sandbox.
//!
//! The choices are kept in `~/.sbxw/state/threads/<sandbox>.json`
//! (`thread → repo`, or `thread → {repo, model}` once a model was picked for
//! it — see `ThreadRecord`), written like plans are: `0700` directory, temporary file
//! and rename.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// How deep under the workspace repos are looked for: `a/b/c`.
const MAX_DEPTH: usize = 3;

/// Folders never looked into: dependencies and build output, which can be huge
/// and hold someone else's repositories, and sbxw's own worktrees (hidden
/// folders are skipped anyway).
const SKIPPED: &[&str] = &["node_modules", "target", "vendor", ".sbxw-worktrees"];

/// Longest a host git command may take before it is killed and its answer
/// reported as unknown.
const GIT_TIMEOUT: Duration = Duration::from_secs(5);

/// One repository under the workspace, as `GET /api/sandboxes/:name/repos`
/// reports it.
#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
pub(crate) struct RepoInfo {
    /// Relative to the workspace; `''` is the workspace itself.
    pub path: String,
    pub branch: String,
    /// Uncommitted files, untracked included.
    pub dirty: u32,
    /// Against its upstream; `None` without one, or when git could not say.
    pub ahead: Option<u32>,
    pub behind: Option<u32>,
    pub detached: bool,
}

/// Does `dir` hold a `.git` — a directory, or the file a worktree or a
/// submodule has instead?
fn has_git(dir: &Path) -> bool {
    dir.join(".git").symlink_metadata().is_ok()
}

/// The repository root holding the workspace itself: the workspace, or the
/// nearest ancestor with a `.git`.
pub(crate) fn enclosing_repo(workspace: &Path) -> Option<PathBuf> {
    workspace
        .ancestors()
        .find(|d| has_git(d))
        .map(Path::to_path_buf)
}

/// Every repo under `workspace`, relative to it, sorted with `''` first.
///
/// The workspace counts as `''` when it, or an ancestor, is a repo; then
/// nothing under it is looked at, since the inside of a repo is that repo's.
/// Otherwise its subfolders are walked to `MAX_DEPTH`, stopping at each repo
/// found. Symlinks are not followed: a link back up would loop, and one out of
/// the workspace names a repo the sandbox may not see.
pub(crate) fn scan(workspace: &Path) -> Vec<String> {
    if enclosing_repo(workspace).is_some() {
        return vec![String::new()];
    }
    let mut out = Vec::new();
    walk(workspace, "", 1, &mut out);
    out.sort();
    out
}

fn walk(dir: &Path, rel: &str, depth: usize, out: &mut Vec<String>) {
    if depth > MAX_DEPTH {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') || SKIPPED.contains(&name.as_str()) {
            continue;
        }
        if !e.file_type().is_ok_and(|t| t.is_dir()) {
            continue;
        }
        let path = e.path();
        let child = if rel.is_empty() {
            name
        } else {
            format!("{rel}/{name}")
        };
        if has_git(&path) {
            out.push(child);
        } else {
            walk(&path, &child, depth + 1, out);
        }
    }
}

/// `scan` with each repo's status, read in parallel.
pub(crate) fn list(workspace: &Path) -> Vec<RepoInfo> {
    let handles: Vec<_> = scan(workspace)
        .into_iter()
        .map(|rel| {
            let dir = if rel.is_empty() {
                workspace.to_path_buf()
            } else {
                workspace.join(&rel)
            };
            std::thread::spawn(move || status(&rel, &dir))
        })
        .collect();
    handles.into_iter().filter_map(|h| h.join().ok()).collect()
}

/// One repo's status: `git status --porcelain=v2 --branch`, or, when host git
/// is missing or too slow, its branch from `HEAD` and nothing else.
fn status(rel: &str, dir: &Path) -> RepoInfo {
    match git_status(dir) {
        Some(raw) => parse_status(rel, &raw),
        None => {
            let (branch, detached) = head_branch(dir).unwrap_or_default();
            RepoInfo {
                path: rel.to_string(),
                branch,
                dirty: 0,
                ahead: None,
                behind: None,
                detached,
            }
        }
    }
}

/// The porcelain output, or `None` (see `git`).
fn git_status(dir: &Path) -> Option<String> {
    git(dir, &["status", "--porcelain=v2", "--branch"])
}

/// Host git's output for `git -C dir args…`, or `None` if git is missing,
/// fails, or takes longer than `GIT_TIMEOUT` (it is killed then). Output goes
/// to a file rather than a pipe, so a large output cannot block git while this
/// waits on it.
///
/// `--no-optional-locks`: the agents in the sandbox commit in these same
/// repos, and a `git status` that refreshed the index from here would hold
/// `index.lock` under a `git commit` of theirs, which then fails.
pub(crate) fn git(dir: &Path, args: &[&str]) -> Option<String> {
    let out = tempfile_for_git()?;
    let mut child = Command::new("git")
        .arg("--no-optional-locks")
        .arg("-C")
        .arg(dir)
        .args(args)
        .stdin(Stdio::null())
        .stdout(out.0.try_clone().ok()?)
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let deadline = Instant::now() + GIT_TIMEOUT;
    let ok = loop {
        match child.try_wait() {
            Ok(Some(st)) => break st.success(),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(5)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                break false;
            }
        }
    };
    // Lossy: a commit subject is whatever bytes its author typed.
    let text = std::fs::read(&out.1).ok().map(|b| String::from_utf8_lossy(&b).into_owned());
    let _ = std::fs::remove_file(&out.1);
    text.filter(|_| ok)
}

/// A fresh file in the temp dir to collect one git command's output into.
fn tempfile_for_git() -> Option<(std::fs::File, PathBuf)> {
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("sbxw-git-{}-{n}", std::process::id()));
    let f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .ok()?;
    Some((f, path))
}

/// `# branch.head`, `# branch.ab` and one line per changed file.
fn parse_status(rel: &str, raw: &str) -> RepoInfo {
    let mut info = RepoInfo {
        path: rel.to_string(),
        branch: String::new(),
        dirty: 0,
        ahead: None,
        behind: None,
        detached: false,
    };
    for line in raw.lines() {
        if let Some(head) = line.strip_prefix("# branch.head ") {
            info.detached = head == "(detached)";
            info.branch = if info.detached {
                "HEAD".into()
            } else {
                head.to_string()
            };
        } else if let Some(ab) = line.strip_prefix("# branch.ab ") {
            let mut it = ab.split_whitespace();
            info.ahead = it
                .next()
                .and_then(|a| a.trim_start_matches('+').parse().ok());
            info.behind = it
                .next()
                .and_then(|b| b.trim_start_matches('-').parse().ok());
        } else if !line.starts_with('#') && !line.is_empty() {
            info.dirty += 1;
        }
    }
    info
}

/// The branch `HEAD` names, read off disk (following a `.git` file's
/// `gitdir:`), and whether it is detached.
fn head_branch(dir: &Path) -> Option<(String, bool)> {
    let dot = enclosing_repo(dir)?.join(".git");
    let git_dir = if dot.is_file() {
        let text = std::fs::read_to_string(&dot).ok()?;
        let target = PathBuf::from(text.trim().strip_prefix("gitdir:")?.trim());
        if target.is_absolute() {
            target
        } else {
            dot.parent()?.join(target)
        }
    } else {
        dot
    };
    let head = std::fs::read_to_string(git_dir.join("HEAD")).ok()?;
    Some(match head.trim().strip_prefix("ref: refs/heads/") {
        Some(b) => (b.to_string(), false),
        None => ("HEAD".to_string(), true),
    })
}

/// The branch checked out in the repo holding `dir`, or `None` when HEAD is
/// detached or there is no repo — what a merge into it would land on.
pub(crate) fn branch_of(dir: &Path) -> Option<String> {
    head_branch(dir).and_then(|(b, detached)| (!detached).then_some(b))
}

/// A repo path as a thread names it, checked and normalised: relative, no
/// `..`, no `.`, no empty parts. `''` (the workspace itself) is fine.
pub(crate) fn normalize(rel: &str) -> Result<String, String> {
    let rel = rel.trim();
    if rel.starts_with('/') || rel.starts_with('\\') {
        return Err(format!("repo {rel:?} must be relative to the workspace"));
    }
    let mut parts = Vec::new();
    for c in Path::new(rel).components() {
        match c {
            Component::Normal(p) => parts.push(p.to_string_lossy().into_owned()),
            Component::CurDir => {}
            _ => return Err(format!("repo {rel:?} must stay inside the workspace")),
        }
    }
    Ok(parts.join("/"))
}

/// `rel` checked against `workspace`: normalised, a directory under it (after
/// symlinks), and a repo. Gives the normalised path.
pub(crate) fn validate(workspace: &Path, rel: &str) -> Result<String, String> {
    let rel = normalize(rel)?;
    let shown = if rel.is_empty() {
        "the workspace"
    } else {
        rel.as_str()
    };
    let dir = workspace.join(&rel);
    let (Ok(ws), Ok(real)) = (workspace.canonicalize(), dir.canonicalize()) else {
        return Err(format!("{shown} is not a folder of the workspace"));
    };
    if !real.starts_with(&ws) || !real.is_dir() {
        return Err(format!("{shown} is not a folder of the workspace"));
    }
    let is_repo = if rel.is_empty() {
        enclosing_repo(&real).is_some()
    } else {
        has_git(&real)
    };
    if !is_repo {
        return Err(pick_a_repo(shown, &scan(workspace)));
    }
    Ok(rel)
}

/// Why `what` cannot hold a thread, with the repos there are to pick from.
fn pick_a_repo(what: &str, found: &[String]) -> String {
    let hint = if found.is_empty() {
        "no repository was found under it".to_string()
    } else {
        format!("its repositories: {}", found.join(", "))
    };
    format!("{what} is not a git repository - pick a repo for this thread ({hint})")
}

/// The top level of the repo `rel` names: where its `.sbxw-worktrees` is. For
/// `''`, the workspace's enclosing repo, which may sit above it.
pub(crate) fn repo_root(workspace: &Path, rel: &str) -> Option<PathBuf> {
    if rel.is_empty() {
        enclosing_repo(workspace)
    } else {
        Some(workspace.join(rel))
    }
}

/// The directory a thread's scripts start from: `<workspace>/<repo>`.
pub(crate) fn thread_dir(workspace: &Path, rel: &str) -> PathBuf {
    if rel.is_empty() {
        workspace.to_path_buf()
    } else {
        workspace.join(rel)
    }
}

// ── Which repo each thread works in ──────────────────────────────────────────

/// One writer at a time: two threads opened at once must not each write the
/// file without the other's choice.
static LOCK: Mutex<()> = Mutex::new(());

fn file_of(dir: &Path, sandbox: &str) -> PathBuf {
    dir.join(format!("{sandbox}.json"))
}

/// What is remembered about one thread: its repo, and the model it was first
/// opened on.
///
/// On disk a record without a model is the bare repo string, exactly what
/// files held before models could be picked — so those files still load, and a
/// file written now still reads in an older sbxw unless a model was picked.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct ThreadRecord {
    pub repo: String,
    pub model: Option<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(untagged)]
enum StoredRecord {
    Repo(String),
    Full {
        repo: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
    },
}

impl From<StoredRecord> for ThreadRecord {
    fn from(s: StoredRecord) -> Self {
        match s {
            StoredRecord::Repo(repo) => ThreadRecord { repo, model: None },
            StoredRecord::Full { repo, model } => ThreadRecord { repo, model },
        }
    }
}

impl From<&ThreadRecord> for StoredRecord {
    fn from(r: &ThreadRecord) -> Self {
        match &r.model {
            None => StoredRecord::Repo(r.repo.clone()),
            Some(m) => StoredRecord::Full {
                repo: r.repo.clone(),
                model: Some(m.clone()),
            },
        }
    }
}

/// `sandbox`'s records; unreadable reads as none.
pub(crate) fn load_records(dir: &Path, sandbox: &str) -> BTreeMap<String, ThreadRecord> {
    std::fs::read_to_string(file_of(dir, sandbox))
        .ok()
        .and_then(|t| serde_json::from_str::<BTreeMap<String, StoredRecord>>(&t).ok())
        .map(|m| m.into_iter().map(|(k, v)| (k, v.into())).collect())
        .unwrap_or_default()
}

/// `sandbox`'s choices of repo; unreadable reads as none.
pub(crate) fn load(dir: &Path, sandbox: &str) -> BTreeMap<String, String> {
    load_records(dir, sandbox)
        .into_iter()
        .map(|(k, v)| (k, v.repo))
        .collect()
}

/// The repo recorded for `thread`, if any.
pub(crate) fn recorded(dir: &Path, sandbox: &str, thread: &str) -> Option<String> {
    load(dir, sandbox).remove(thread)
}

/// The model recorded for `thread`, if any.
pub(crate) fn recorded_model(dir: &Path, sandbox: &str, thread: &str) -> Option<String> {
    load_records(dir, sandbox).remove(thread).and_then(|r| r.model)
}

/// Record `thread`'s repo, keeping its model, or forget the thread with `None`.
pub(crate) fn record(
    dir: &Path,
    sandbox: &str,
    thread: &str,
    repo: Option<&str>,
) -> std::io::Result<()> {
    update(dir, sandbox, thread, |rec| match repo {
        Some(r) => {
            let mut rec = rec.unwrap_or_default();
            rec.repo = r.to_string();
            Some(rec)
        }
        None => None,
    })
}

/// Record the model `thread` runs on, or clear it with `None`, keeping its
/// repo. A thread with no record yet gets one in `''` — only the case for a
/// sandbox sbxw has no workspace for, where the repo is never read.
pub(crate) fn record_model(
    dir: &Path,
    sandbox: &str,
    thread: &str,
    model: Option<&str>,
) -> std::io::Result<()> {
    update(dir, sandbox, thread, |rec| {
        let mut rec = rec.unwrap_or_default();
        rec.model = model.map(str::to_string);
        Some(rec)
    })
}

/// Change one thread's record under the lock, writing only when it changed.
fn update<F>(dir: &Path, sandbox: &str, thread: &str, f: F) -> std::io::Result<()>
where
    F: FnOnce(Option<ThreadRecord>) -> Option<ThreadRecord>,
{
    let _held = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut map = load_records(dir, sandbox);
    let before = map.clone();
    match f(map.remove(thread)) {
        Some(rec) => map.insert(thread.to_string(), rec),
        None => None,
    };
    if map == before {
        return Ok(());
    }
    let path = file_of(dir, sandbox);
    if map.is_empty() {
        return match std::fs::remove_file(&path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        };
    }
    std::fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    let stored: BTreeMap<&String, StoredRecord> = map.iter().map(|(k, v)| (k, v.into())).collect();
    let tmp = dir.join(format!(".{sandbox}.json.tmp"));
    std::fs::write(&tmp, serde_json::to_vec_pretty(&stored)?)?;
    std::fs::rename(&tmp, &path)
}

/// The repo a thread is to open in, given what was asked (`requested`) and
/// what is known: its record, and where it already has a worktree. With none
/// of those, the workspace when it is a repo (`''`, as before repos could be
/// picked), else the one repo under it; with several, or none, nothing can be
/// guessed and the person is asked to pick.
///
/// A different repo for a thread that already has a worktree is refused: its
/// branch, its uncommitted work and its conversation are all in the old one.
pub(crate) fn choose(
    workspace: &Path,
    recorded: Option<&str>,
    thread: &str,
    requested: Option<&str>,
) -> Result<String, String> {
    let has_worktree = |rel: &str| {
        repo_root(workspace, rel).is_some_and(|r| r.join(".sbxw-worktrees").join(thread).is_dir())
    };
    let requested = match requested {
        Some(r) => Some(validate(workspace, r)?),
        None => None,
    };
    // Where it has a worktree now: its record, or else wherever one is found.
    let current = match recorded {
        Some(r) => has_worktree(r).then(|| r.to_string()),
        None => scan(workspace).into_iter().find(|r| has_worktree(r)),
    };
    match (requested, current) {
        (Some(want), Some(have)) if want != have => Err(format!(
            "thread {thread} already has a worktree in {} - it cannot move to {}. \
             Close it first, or use another thread name.",
            shown(&have),
            shown(&want)
        )),
        (Some(want), _) => Ok(want),
        (None, Some(have)) => Ok(have),
        (None, None) => match recorded {
            Some(r) => Ok(r.to_string()),
            None => match scan(workspace).as_slice() {
                [only] => Ok(only.clone()),
                found => Err(pick_a_repo("the workspace", found)),
            },
        },
    }
}

fn shown(rel: &str) -> String {
    if rel.is_empty() {
        "the workspace".into()
    } else {
        format!("'{rel}'")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh, empty directory for one test.
    fn temp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("sbxw-repos-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn mkrepo(dir: &Path) {
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        std::fs::write(dir.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
    }

    #[test]
    fn scan_finds_nested_repos_and_skips_what_it_should() {
        let ws = temp("scan");
        mkrepo(&ws.join("backend"));
        mkrepo(&ws.join("backend/vendored")); // inside a repo: not listed
        mkrepo(&ws.join("apps/web/frontend")); // depth 3
        mkrepo(&ws.join("a/b/c/d")); // depth 4: too deep
        mkrepo(&ws.join("node_modules/dep"));
        mkrepo(&ws.join("target/x"));
        mkrepo(&ws.join("vendor/y"));
        mkrepo(&ws.join(".hidden/z"));
        mkrepo(&ws.join(".sbxw-worktrees/t"));
        std::fs::create_dir_all(ws.join("tools")).unwrap();
        std::fs::write(ws.join("tools/.git"), "gitdir: ../backend/.git\n").unwrap();
        assert_eq!(scan(&ws), vec!["apps/web/frontend", "backend", "tools"]);

        // A workspace that is a repo (or inside one) is just `''`.
        assert_eq!(scan(&ws.join("backend")), vec![""]);
        std::fs::create_dir_all(ws.join("backend/src")).unwrap();
        assert_eq!(scan(&ws.join("backend/src")), vec![""]);

        // Without host git's help, the branch still comes from HEAD.
        let (b, detached) = head_branch(&ws.join("tools")).unwrap();
        assert_eq!((b.as_str(), detached), ("main", false));
        let _ = std::fs::remove_dir_all(&ws);
    }

    #[test]
    fn list_reads_each_repo_with_host_git() {
        let git_ok = Command::new("git")
            .arg("--version")
            .output()
            .is_ok_and(|o| o.status.success());
        if !git_ok {
            return;
        }
        let ws = temp("list");
        for r in ["one", "two"] {
            let d = ws.join(r);
            std::fs::create_dir_all(&d).unwrap();
            let ok = Command::new("git")
                .args(["init", "-q", "-b", "trunk"])
                .current_dir(&d)
                .status()
                .is_ok_and(|s| s.success());
            assert!(ok);
        }
        std::fs::write(ws.join("two/a.txt"), "x").unwrap();
        let got = list(&ws);
        assert_eq!(
            got.iter().map(|r| r.path.as_str()).collect::<Vec<_>>(),
            ["one", "two"]
        );
        assert_eq!((got[0].branch.as_str(), got[0].dirty), ("trunk", 0));
        assert_eq!(
            (got[1].dirty, got[1].ahead, got[1].detached),
            (1, None, false)
        );
        let _ = std::fs::remove_dir_all(&ws);
    }

    #[test]
    fn porcelain_v2_is_read_into_a_status() {
        let raw = "# branch.oid abc\n# branch.head main\n# branch.upstream origin/main\n\
                   # branch.ab +2 -1\n1 .M N... 100644 100644 100644 a b src/x.rs\n? new.txt\n";
        let s = parse_status("backend", raw);
        assert_eq!(
            s,
            RepoInfo {
                path: "backend".into(),
                branch: "main".into(),
                dirty: 2,
                ahead: Some(2),
                behind: Some(1),
                detached: false
            }
        );
        let d = parse_status("", "# branch.oid abc\n# branch.head (detached)\n");
        assert!(d.detached);
        assert_eq!((d.ahead, d.dirty), (None, 0));
    }

    #[test]
    fn repo_paths_are_validated() {
        assert_eq!(normalize("backend/").unwrap(), "backend");
        assert_eq!(normalize("./a//b").unwrap(), "a/b");
        assert_eq!(normalize("").unwrap(), "");
        assert!(normalize("/etc").is_err());
        assert!(normalize("../other").is_err());
        assert!(normalize("a/../../b").is_err());

        let ws = temp("validate");
        mkrepo(&ws.join("backend"));
        std::fs::create_dir_all(ws.join("docs")).unwrap();
        assert_eq!(validate(&ws, "backend").unwrap(), "backend");
        let e = validate(&ws, "docs").unwrap_err();
        assert!(
            e.contains("pick a repo for this thread") && e.contains("backend"),
            "{e}"
        );
        assert!(validate(&ws, "missing").is_err());
        assert!(
            validate(&ws, "").is_err(),
            "the workspace itself is no repo here"
        );
        #[cfg(unix)]
        {
            let outside = temp("validate-outside");
            mkrepo(&outside);
            std::os::unix::fs::symlink(&outside, ws.join("link")).unwrap();
            assert!(
                validate(&ws, "link").is_err(),
                "a symlink out of the workspace"
            );
            let _ = std::fs::remove_dir_all(&outside);
        }
        let _ = std::fs::remove_dir_all(&ws);
    }

    #[test]
    fn choices_round_trip_and_a_worktree_pins_its_repo() {
        let state = temp("state").join("threads");
        assert_eq!(recorded(&state, "neos", "api"), None);
        record(&state, "neos", "api", Some("backend")).unwrap();
        record(&state, "neos", "ui", Some("frontend")).unwrap();
        assert_eq!(recorded(&state, "neos", "api").as_deref(), Some("backend"));
        assert_eq!(load(&state, "neos").len(), 2);
        assert!(load(&state, "other").is_empty());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&state).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o700);
        }
        record(&state, "neos", "api", None).unwrap();
        record(&state, "neos", "ui", None).unwrap();
        assert!(
            !file_of(&state, "neos").exists(),
            "an empty record is removed"
        );

        let ws = temp("choose");
        mkrepo(&ws.join("backend"));
        mkrepo(&ws.join("frontend"));
        let pick = |rec: Option<&str>, t: &str, req: Option<&str>| choose(&ws, rec, t, req);
        assert_eq!(pick(None, "api", Some("backend")).unwrap(), "backend");
        // Several repos and nothing asked: the person picks.
        let e = pick(None, "api", None).unwrap_err();
        assert!(
            e.contains("pick a repo for this thread") && e.contains("backend, frontend"),
            "{e}"
        );
        std::fs::create_dir_all(ws.join("backend/.sbxw-worktrees/api")).unwrap();
        // Found without a record, kept without a request, refused elsewhere.
        assert_eq!(pick(None, "api", None).unwrap(), "backend");
        assert_eq!(pick(Some("backend"), "api", None).unwrap(), "backend");
        let e = pick(Some("backend"), "api", Some("frontend")).unwrap_err();
        assert!(e.contains("already has a worktree"), "{e}");
        // No worktree yet: the record can still change.
        assert_eq!(
            pick(Some("frontend"), "ui", Some("backend")).unwrap(),
            "backend"
        );
        let _ = std::fs::remove_dir_all(&ws);

        // Nothing recorded or asked: the workspace when it is a repo, else
        // the one repo under it, else nothing to guess from.
        let one = temp("choose-one");
        mkrepo(&one.join("only"));
        assert_eq!(choose(&one, None, "t", None).unwrap(), "only");
        assert_eq!(choose(&one.join("only"), None, "t", None).unwrap(), "");
        let none = temp("choose-none");
        let e = choose(&none, None, "t", None).unwrap_err();
        assert!(
            e.contains("pick a repo") && e.contains("no repository"),
            "{e}"
        );
        let _ = std::fs::remove_dir_all(&one);
        let _ = std::fs::remove_dir_all(&none);
        let _ = std::fs::remove_dir_all(state.parent().unwrap());
    }

    #[test]
    fn a_model_is_kept_beside_the_repo_and_old_files_still_load() {
        let state = temp("state-model").join("threads");
        std::fs::create_dir_all(&state).unwrap();
        // Written before models could be picked.
        std::fs::write(file_of(&state, "neos"), r#"{"api":"backend","ui":""}"#).unwrap();
        assert_eq!(recorded(&state, "neos", "api").as_deref(), Some("backend"));
        assert_eq!(recorded_model(&state, "neos", "api"), None);

        record_model(&state, "neos", "api", Some("claude-opus-5-5")).unwrap();
        assert_eq!(recorded_model(&state, "neos", "api").as_deref(), Some("claude-opus-5-5"));
        // Moving the repo keeps the model.
        record(&state, "neos", "api", Some("frontend")).unwrap();
        let rec = load_records(&state, "neos").remove("api").unwrap();
        assert_eq!(
            rec,
            ThreadRecord { repo: "frontend".into(), model: Some("claude-opus-5-5".into()) }
        );
        // A thread without a model is still written as a bare repo.
        let raw = std::fs::read_to_string(file_of(&state, "neos")).unwrap();
        assert!(raw.contains(r#""ui": """#), "{raw}");
        assert!(raw.contains(r#""model": "claude-opus-5-5""#), "{raw}");
        assert_eq!(load(&state, "neos").get("ui").map(String::as_str), Some(""));

        record(&state, "neos", "api", None).unwrap();
        assert_eq!(recorded_model(&state, "neos", "api"), None);
        let _ = std::fs::remove_dir_all(state.parent().unwrap());
    }
}
