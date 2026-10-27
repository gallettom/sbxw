//! The git repositories under a workspace, read with git on the **host**.
//!
//! The workspace is mounted at the same path in the sandbox, so a path found
//! here is the one the in-sandbox scripts `cd` into — and every read works on a
//! stopped sandbox. Every call passes `--no-optional-locks`, so a read never
//! holds the index lock a thread needs to commit, and is bounded by a timeout
//! (`kill_on_drop` makes the timeout real).
//!
//! A workspace is often a folder of repos (`~/dev/shop/{backend,frontend}`),
//! so a thread is opened *in a repo*, named relative to the workspace (`''` is
//! the workspace itself), and that choice is kept in the sandbox's record.

use serde::Serialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

const MAX_DEPTH: usize = 3;
const SKIPPED: &[&str] = &["node_modules", "target", "vendor", ".sbxw-worktrees"];
const GIT_TIMEOUT: Duration = Duration::from_secs(5);
const BASE_LOG: usize = 60;
const THREAD_LOG: usize = 20;
const LOG_FORMAT: &str = "--format=%H%x1f%P%x1f%ct%x1f%s";

/// Settings that make git start a program, pinned off for every host read.
/// The repo's `.git` is writable from inside the sandbox, so its config is the
/// sandbox's word: a `core.fsmonitor` set there would run on the host at the
/// next `git status` (sbx 0.50 closed the same door for its own git calls).
/// Command-line `-c` outranks every config file.
const NO_PROGRAMS: &[&str] = &[
    "core.fsmonitor=false",
    "core.hooksPath=/dev/null",
    "core.pager=cat",
    "core.sshCommand=",
    "core.askPass=",
    "diff.external=",
    "credential.helper=",
];

/// Clean/smudge filters are named by the repo, so they are found first —
/// `git config` reads without running anything — then blanked: git runs no
/// filter whose command is empty. `status` is the read that would run one.
async fn filter_overrides(dir: &Path) -> Vec<String> {
    let Some(out) = run_git(dir, &[], &["config", "--includes", "--name-only", "--get-regexp", r"^filter\..*\.(clean|smudge|process)$"]).await
    else {
        return Vec::new();
    };
    out.lines().filter(|k| !k.is_empty()).flat_map(|k| ["-c".to_string(), format!("{k}=")]).collect()
}

async fn run_git(dir: &Path, overrides: &[String], args: &[&str]) -> Option<String> {
    let mut cmd = tokio::process::Command::new("git");
    cmd.arg("--no-optional-locks");
    for kv in NO_PROGRAMS {
        cmd.args(["-c", kv]);
    }
    let child = cmd
        .args(overrides)
        .arg("-C")
        .arg(dir)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .output();
    let out = tokio::time::timeout(GIT_TIMEOUT, child).await.ok()?.ok()?;
    // Lossy: a commit subject is whatever bytes its author typed.
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// `git -C dir args…`, stdout on success; `None` on failure or timeout. Never
/// runs a program the repo's own config names ([`NO_PROGRAMS`]).
pub async fn git(dir: &Path, args: &[&str]) -> Option<String> {
    let filters = if args.first() == Some(&"status") { filter_overrides(dir).await } else { Vec::new() };
    run_git(dir, &filters, args).await
}

fn has_git(dir: &Path) -> bool {
    dir.join(".git").symlink_metadata().is_ok()
}

/// The repo the workspace sits in, which may be above it.
pub fn enclosing_repo(workspace: &Path) -> Option<PathBuf> {
    workspace.ancestors().find(|d| has_git(d)).map(Path::to_path_buf)
}

/// Make a chat workspace a repo with one empty commit, so its extra
/// conversations can be threads: a thread is a worktree, and a worktree needs
/// a commit to branch from. Nothing is ever committed after that. A no-op on
/// a workspace already in a repo.
pub fn init_chat_repo(dir: &Path) -> Result<(), String> {
    if has_git(dir) {
        return Ok(());
    }
    // A chat workspace lives under /tmp, which macOS sweeps; `git -C` into a
    // vanished one fails before the sandbox is even started.
    std::fs::create_dir_all(dir).map_err(|e| format!("could not re-create chat workspace {}: {e}", dir.display()))?;
    for args in [&["init", "-q"][..], &["commit", "-q", "--allow-empty", "-m", "sbxw chat"][..]] {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["-c", "user.email=sbxw@localhost", "-c", "user.name=sbxw", "-c", "init.defaultBranch=main"])
            .args(args)
            .output()
            .map_err(|e| format!("could not run git: {e}"))?;
        if !out.status.success() {
            return Err(format!("git {} in {} failed: {}", args[0], dir.display(), String::from_utf8_lossy(&out.stderr).trim()));
        }
    }
    Ok(())
}

/// Repos under `workspace`, relative: `['']` when it is in one, else its
/// subfolders that are repos, to depth 3, skipping hidden and build folders.
pub fn scan(workspace: &Path) -> Vec<String> {
    if enclosing_repo(workspace).is_some() {
        return vec![String::new()];
    }
    fn walk(dir: &Path, rel: &str, depth: usize, out: &mut Vec<String>) {
        if depth > MAX_DEPTH {
            return;
        }
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') || SKIPPED.contains(&name.as_str()) || !e.file_type().is_ok_and(|t| t.is_dir()) {
                continue;
            }
            let child = if rel.is_empty() { name } else { format!("{rel}/{name}") };
            if has_git(&e.path()) {
                out.push(child);
            } else {
                walk(&e.path(), &child, depth + 1, out);
            }
        }
    }
    let mut out = Vec::new();
    walk(workspace, "", 1, &mut out);
    out.sort();
    out
}

pub fn repo_root(workspace: &Path, rel: &str) -> Option<PathBuf> {
    if rel.is_empty() {
        enclosing_repo(workspace)
    } else {
        Some(workspace.join(rel))
    }
}

/// Where a thread's scripts start: `<workspace>/<repo>`.
pub fn thread_dir(workspace: &Path, rel: &str) -> PathBuf {
    if rel.is_empty() {
        workspace.to_path_buf()
    } else {
        workspace.join(rel)
    }
}

/// `rel` checked against the workspace: normalised, a folder under it (after
/// symlinks), and a repo.
pub fn validate(workspace: &Path, rel: &str) -> Result<String, String> {
    let rel = sbxw_core::names::normalize_repo_path(rel)?;
    let shown = if rel.is_empty() { "the workspace" } else { rel.as_str() };
    let dir = workspace.join(&rel);
    let (Ok(ws), Ok(real)) = (workspace.canonicalize(), dir.canonicalize()) else {
        return Err(format!("{shown} is not a folder of the workspace"));
    };
    if !real.starts_with(&ws) || !real.is_dir() {
        return Err(format!("{shown} is not a folder of the workspace"));
    }
    let is_repo = if rel.is_empty() { enclosing_repo(&real).is_some() } else { has_git(&real) };
    if !is_repo {
        return Err(pick_a_repo(shown, &scan(workspace)));
    }
    Ok(rel)
}

fn pick_a_repo(what: &str, found: &[String]) -> String {
    let hint = if found.is_empty() {
        "no repository was found under it".to_string()
    } else {
        format!("its repositories: {}", found.join(", "))
    };
    format!("{what} is not a git repository - pick a repo for this thread ({hint})")
}

fn shown(rel: &str) -> String {
    if rel.is_empty() {
        "the workspace".into()
    } else {
        format!("'{rel}'")
    }
}

/// Which repo a thread works in: the one asked for, else where it already
/// has a worktree, else its record, else the workspace's only repo. A thread
/// with a worktree cannot move to another repo — its branch, uncommitted work
/// and conversation are in the old one.
pub fn choose(workspace: &Path, recorded: Option<&str>, thread: &str, requested: Option<&str>) -> Result<String, String> {
    let has_worktree = |rel: &str| repo_root(workspace, rel).is_some_and(|r| r.join(".sbxw-worktrees").join(thread).is_dir());
    let requested = requested.map(|r| validate(workspace, r)).transpose()?;
    let current = match recorded {
        Some(r) => has_worktree(r).then(|| r.to_string()),
        None => scan(workspace).into_iter().find(|r| has_worktree(r)),
    };
    match (requested, current) {
        (Some(want), Some(have)) if want != have => Err(format!(
            "thread {thread} already has a worktree in {} - it cannot move to {}. Close it first, \
             or use another thread name.",
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

/// Every thread with a worktree under the workspace, with its repo. A name in
/// two repos is listed in the one its record names, else the first.
pub fn thread_worktrees(workspace: &Path, recorded: &BTreeMap<String, String>) -> Vec<(String, String)> {
    let mut out: BTreeMap<String, String> = BTreeMap::new();
    for repo in scan(workspace) {
        let Some(root) = repo_root(workspace, &repo) else { continue };
        let Ok(dir) = std::fs::read_dir(root.join(".sbxw-worktrees")) else { continue };
        for e in dir.flatten() {
            let n = e.file_name().to_string_lossy().into_owned();
            if sbxw_core::names::is_valid_thread_name(&n) && e.path().is_dir() {
                let mine = recorded.get(&n) == Some(&repo);
                if mine || !out.contains_key(&n) {
                    out.insert(n, repo.clone());
                }
            }
        }
    }
    out.into_iter().collect()
}

// ── Repo status, for the New thread dialog ────────────────────────────────

#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
pub struct RepoInfo {
    pub path: String,
    pub branch: String,
    pub dirty: u32,
    pub ahead: Option<u32>,
    pub behind: Option<u32>,
    pub detached: bool,
}

pub fn parse_status(rel: &str, raw: &str) -> RepoInfo {
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
            info.branch = if info.detached { "HEAD".into() } else { head.to_string() };
        } else if let Some(ab) = line.strip_prefix("# branch.ab ") {
            let mut it = ab.split_whitespace();
            info.ahead = it.next().and_then(|a| a.trim_start_matches('+').parse().ok());
            info.behind = it.next().and_then(|b| b.trim_start_matches('-').parse().ok());
        } else if !line.starts_with('#') && !line.is_empty() {
            info.dirty += 1;
        }
    }
    info
}

/// The branch checked out at `dir`, read from `HEAD` without running git.
pub fn head_branch(dir: &Path) -> Option<(String, bool)> {
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

pub fn branch_of(dir: &Path) -> Option<String> {
    head_branch(dir).and_then(|(b, detached)| (!detached).then_some(b))
}

pub async fn list(workspace: &Path) -> Vec<RepoInfo> {
    let reads = scan(workspace).into_iter().map(|rel| {
        let dir = thread_dir(workspace, &rel);
        async move {
            match git(&dir, &["status", "--porcelain=v2", "--branch"]).await {
                Some(raw) => parse_status(&rel, &raw),
                None => {
                    let (branch, detached) = head_branch(&dir).unwrap_or_default();
                    RepoInfo { path: rel, branch, dirty: 0, ahead: None, behind: None, detached }
                }
            }
        }
    });
    futures_util::future::join_all(reads).await
}

// ── The worktree graph ─────────────────────────────────────────────────────

#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
pub struct Commit {
    pub sha: String,
    pub parents: Vec<String>,
    pub at: i64,
    pub subject: String,
}

#[derive(Serialize, Debug, Clone, PartialEq, Eq, Default)]
pub struct ThreadBranch {
    pub name: String,
    pub branch: String,
    pub worktree: String,
    pub tip: Option<String>,
    pub fork: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    pub dirty: Option<u32>,
    pub commits: Vec<Commit>,
}

#[derive(Serialize, Debug, Clone, PartialEq, Eq, Default)]
pub struct RepoBranches {
    pub path: String,
    pub base: Option<String>,
    pub head: Option<String>,
    pub dirty: Option<u32>,
    pub log: Vec<Commit>,
    pub threads: Vec<ThreadBranch>,
    pub error: Option<String>,
}

fn one_line(out: Option<String>) -> Option<String> {
    out.map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

pub fn parse_counts(raw: &str) -> Option<(u32, u32)> {
    let mut it = raw.split_whitespace();
    Some((it.next()?.parse().ok()?, it.next()?.parse().ok()?))
}

pub fn parse_log(raw: &str) -> Vec<Commit> {
    raw.lines()
        .filter_map(|line| {
            let mut f = line.splitn(4, '\u{1f}');
            let sha = f.next().filter(|s| !s.is_empty())?.to_string();
            let parents = f.next()?.split_whitespace().map(str::to_string).collect();
            let at = f.next()?.parse().unwrap_or(0);
            let subject = f.next().unwrap_or_default().to_string();
            Some(Commit { sha, parents, at, subject })
        })
        .collect()
}

async fn count_status(dir: &Path) -> Option<u32> {
    git(dir, &["status", "--porcelain"]).await.map(|r| r.lines().filter(|l| !l.is_empty()).count() as u32)
}

/// Each thread's branch against the branch its repo's main checkout is on —
/// the lead's, and the one a merge lands in.
pub async fn graph(workspace: &Path, threads: &[(String, String)]) -> Vec<RepoBranches> {
    let mut by_repo: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for (t, repo) in threads {
        by_repo.entry(repo.as_str()).or_default().push(t.as_str());
    }
    futures_util::future::join_all(by_repo.into_iter().map(|(repo, names)| read_repo(workspace, repo, names))).await
}

async fn read_repo(workspace: &Path, rel: &str, names: Vec<&str>) -> RepoBranches {
    let mut out = RepoBranches { path: rel.to_string(), ..Default::default() };
    let Some(root) = repo_root(workspace, rel) else {
        out.error = Some("the workspace is not in a git repository".into());
        return out;
    };
    let Some(head) = one_line(git(&root, &["rev-parse", "--verify", "--quiet", "HEAD"]).await) else {
        out.error = Some("git on the host could not read its HEAD".into());
        return out;
    };
    out.base = branch_of(&root);
    out.head = Some(head);
    let n = format!("-n{BASE_LOG}");
    let log_args = ["log", "--first-parent", n.as_str(), LOG_FORMAT, "HEAD"];
    let (log, dirty, threads) = tokio::join!(
        git(&root, &log_args),
        count_status(&root),
        futures_util::future::join_all(names.iter().map(|t| read_thread(&root, t)))
    );
    out.log = log.map(|t| parse_log(&t)).unwrap_or_default();
    out.dirty = dirty;
    out.threads = threads;
    out
}

async fn read_thread(root: &Path, name: &str) -> ThreadBranch {
    let wt = root.join(".sbxw-worktrees").join(name);
    let mut t = ThreadBranch {
        name: name.to_string(),
        branch: format!("sbxw/{name}"),
        worktree: wt.to_string_lossy().into_owned(),
        dirty: if wt.is_dir() { count_status(&wt).await } else { None },
        ..Default::default()
    };
    let full = format!("refs/heads/{}", t.branch);
    let Some(tip) = one_line(git(root, &["rev-parse", "--verify", "--quiet", &full]).await) else {
        return t;
    };
    t.fork = one_line(git(root, &["merge-base", "HEAD", &tip]).await);
    let range = format!("HEAD...{tip}");
    if let Some((behind, ahead)) = git(root, &["rev-list", "--left-right", "--count", &range]).await.as_deref().and_then(parse_counts) {
        (t.behind, t.ahead) = (behind, ahead);
    }
    if t.ahead > 0 {
        let (n, own) = (format!("-n{THREAD_LOG}"), format!("HEAD..{tip}"));
        t.commits = git(root, &["log", &n, LOG_FORMAT, &own]).await.map(|r| parse_log(&r)).unwrap_or_default();
    }
    t.tip = Some(tip);
    t
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git_ok() -> bool {
        std::process::Command::new("git").arg("--version").output().is_ok()
    }

    fn run(dir: &Path, args: &[&str]) {
        let ok = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["-c", "user.email=t@t", "-c", "user.name=t", "-c", "init.defaultBranch=main"])
            .args(args)
            .output()
            .unwrap()
            .status
            .success();
        assert!(ok, "git {args:?}");
    }

    #[test]
    fn a_chat_workspace_becomes_a_repo_once() {
        if !git_ok() {
            return;
        }
        let d = tempfile::tempdir().unwrap();
        init_chat_repo(d.path()).unwrap();
        assert_eq!(scan(d.path()), [""]);
        init_chat_repo(d.path()).unwrap();
        run(d.path(), &["worktree", "add", "-q", "-b", "sbxw/chat-2", ".sbxw-worktrees/chat-2"]);
    }

    #[test]
    fn a_swept_chat_workspace_is_re_created() {
        if !git_ok() {
            return;
        }
        let d = tempfile::tempdir().unwrap();
        let gone = d.path().join("ephemeral-chat");
        init_chat_repo(&gone).unwrap();
        assert_eq!(scan(&gone), [""]);
    }

    #[test]
    fn scan_finds_nested_repos_and_skips_what_it_should() {
        let d = tempfile::tempdir().unwrap();
        for p in ["backend/.git", "web/app/.git", "node_modules/x/.git", ".hidden/.git"] {
            std::fs::create_dir_all(d.path().join(p)).unwrap();
        }
        assert_eq!(scan(d.path()), ["backend", "web/app"]);
        std::fs::create_dir_all(d.path().join(".git")).unwrap();
        assert_eq!(scan(d.path()), [""]);
    }

    #[test]
    fn status_is_read_from_porcelain_v2() {
        let i = parse_status("b", "# branch.oid abc\n# branch.head main\n# branch.ab +2 -1\n1 .M N... a\n? x\n");
        assert_eq!((i.branch.as_str(), i.ahead, i.behind, i.dirty), ("main", Some(2), Some(1), 2));
        assert!(parse_status("", "# branch.head (detached)\n").detached);
    }

    #[test]
    fn a_worktree_pins_its_repo() {
        let d = tempfile::tempdir().unwrap();
        for p in ["a/.git", "b/.git", "a/.sbxw-worktrees/t1"] {
            std::fs::create_dir_all(d.path().join(p)).unwrap();
        }
        assert_eq!(choose(d.path(), None, "t1", None).unwrap(), "a");
        assert!(choose(d.path(), None, "t1", Some("b")).is_err());
        assert!(choose(d.path(), None, "t2", None).is_err(), "two repos: pick one");
        assert_eq!(choose(d.path(), None, "t2", Some("./b")).unwrap(), "b");
        assert!(choose(d.path(), None, "t2", Some("../x")).is_err());
        assert_eq!(
            thread_worktrees(d.path(), &BTreeMap::new()),
            [("t1".to_string(), "a".to_string())]
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_read_runs_no_program_the_repo_config_names() {
        use std::os::unix::fs::PermissionsExt;
        if !git_ok() {
            return;
        }
        let d = tempfile::tempdir().unwrap();
        let r = d.path().join("repo");
        std::fs::create_dir(&r).unwrap();
        run(&r, &["init", "-q"]);
        std::fs::write(r.join("a.txt"), "1").unwrap();
        run(&r, &["add", "a.txt"]);
        run(&r, &["commit", "-qm", "one"]);
        // What a sandbox could plant: a hook-style fsmonitor and a clean filter.
        let marker = d.path().join("ran");
        let evil = d.path().join("evil.sh");
        std::fs::write(&evil, format!("#!/bin/sh\ntouch '{}'\ncat\n", marker.display())).unwrap();
        std::fs::set_permissions(&evil, std::fs::Permissions::from_mode(0o755)).unwrap();
        let e = evil.to_str().unwrap();
        run(&r, &["config", "core.fsmonitor", e]);
        run(&r, &["config", "filter.x.clean", e]);
        std::fs::write(r.join(".gitattributes"), "*.txt filter=x\n").unwrap();
        std::fs::write(r.join("a.txt"), "changed").unwrap();
        let out = git(&r, &["status", "--porcelain=v2", "--branch"]).await;
        assert!(out.is_some_and(|o| o.contains("a.txt")));
        assert!(!marker.exists(), "a repo-configured program ran on the host");
    }

    #[tokio::test]
    async fn a_thread_is_read_against_its_base() {
        if !git_ok() {
            return;
        }
        let d = tempfile::tempdir().unwrap();
        let r = d.path();
        run(r, &["init", "-q"]);
        std::fs::write(r.join("a"), "1").unwrap();
        run(r, &["add", "a"]);
        run(r, &["commit", "-qm", "one"]);
        run(r, &["branch", "sbxw/t"]);
        run(r, &["checkout", "-q", "sbxw/t"]);
        std::fs::write(r.join("b"), "2").unwrap();
        run(r, &["add", "b"]);
        run(r, &["commit", "-qm", "two"]);
        run(r, &["checkout", "-q", "main"]);
        let g = graph(r, &[("t".into(), "".into())]).await;
        assert_eq!(g[0].base.as_deref(), Some("main"));
        let t = &g[0].threads[0];
        assert_eq!((t.ahead, t.behind), (1, 0));
        assert_eq!(t.commits[0].subject, "two");
        let repos = list(r).await;
        assert_eq!(repos[0].branch, "main");
    }

    #[test]
    fn logs_and_counts_parse() {
        assert_eq!(parse_counts("3\t4\n"), Some((3, 4)));
        let c = parse_log("abc\u{1f}p1 p2\u{1f}10\u{1f}subject with \u{1f} unit\n");
        assert_eq!(c[0].parents.len(), 2);
        assert_eq!(c[0].subject, "subject with \u{1f} unit");
    }
}
