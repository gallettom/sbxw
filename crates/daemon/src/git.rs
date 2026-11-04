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

use crate::host::{Host, Kind};
use sbxw_sbx::host::HostCmd;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

const MAX_DEPTH: usize = 3;
const SKIPPED: &[&str] = &["node_modules", "target", "vendor", ".sbxw-worktrees"];
const GIT_TIMEOUT: Duration = Duration::from_secs(5);
/// `.git` as a file, and `HEAD`, are a line long; this only bounds a hostile one.
const GIT_FILE_MAX: u64 = 64 * 1024;
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
async fn filter_overrides(host: &Host, dir: &Path) -> Vec<String> {
    let Some(out) = run_git(host, dir, &[], &["config", "--includes", "--name-only", "--get-regexp", r"^filter\..*\.(clean|smudge|process)$"]).await
    else {
        return Vec::new();
    };
    out.lines().filter(|k| !k.is_empty()).flat_map(|k| ["-c".to_string(), format!("{k}=")]).collect()
}

async fn run_git(host: &Host, dir: &Path, overrides: &[String], args: &[&str]) -> Option<String> {
    let mut argv = vec!["--no-optional-locks".to_string()];
    for kv in NO_PROGRAMS {
        argv.extend(["-c".to_string(), kv.to_string()]);
    }
    argv.extend(overrides.iter().cloned());
    argv.extend(["-C".to_string(), dir.to_string_lossy().into_owned()]);
    argv.extend(args.iter().map(|a| a.to_string()));
    // No stdin (git must never wait for input) and a timeout, which the host
    // enforces by killing the command. An unreachable host reads as a failed git.
    let cmd = HostCmd { timeout: Some(GIT_TIMEOUT), ..HostCmd::new("git", argv) };
    let out = host.exec.exec(cmd).await.ok()?;
    // Lossy: a commit subject is whatever bytes its author typed.
    out.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// `git -C dir args…` on the host, stdout on success; `None` on failure or
/// timeout. Never runs a program the repo's own config names ([`NO_PROGRAMS`]).
pub async fn git(host: &Host, dir: &Path, args: &[&str]) -> Option<String> {
    let filters = if args.first() == Some(&"status") { filter_overrides(host, dir).await } else { Vec::new() };
    run_git(host, dir, &filters, args).await
}

async fn has_git(host: &Host, dir: &Path) -> bool {
    host.fs.lstat(&dir.join(".git")).await.is_ok_and(|m| m.is_some())
}

async fn is_dir(host: &Host, p: &Path) -> bool {
    host.fs.stat(p).await.ok().flatten().is_some_and(|m| m.kind == Kind::Dir)
}

/// The repo the workspace sits in, which may be above it.
pub async fn enclosing_repo(host: &Host, workspace: &Path) -> Option<PathBuf> {
    for d in workspace.ancestors() {
        if has_git(host, d).await {
            return Some(d.to_path_buf());
        }
    }
    None
}

/// Make a chat workspace a repo with one empty commit, so its extra
/// conversations can be threads: a thread is a worktree, and a worktree needs
/// a commit to branch from. Nothing is ever committed after that. A no-op on
/// a workspace already in a repo.
pub async fn init_chat_repo(host: &Host, dir: &Path) -> Result<(), String> {
    if has_git(host, dir).await {
        return Ok(());
    }
    // A chat workspace lives under /tmp, which macOS sweeps; `git -C` into a
    // vanished one fails before the sandbox is even started.
    host.fs.create_dir_all(dir).await.map_err(|e| format!("could not re-create chat workspace {}: {e:#}", dir.display()))?;
    for args in [&["init", "-q"][..], &["commit", "-q", "--allow-empty", "-m", "sbxw chat"][..]] {
        let argv = ["-C", &dir.to_string_lossy(), "-c", "user.email=sbxw@localhost", "-c", "user.name=sbxw", "-c", "init.defaultBranch=main"]
            .into_iter()
            .chain(args.iter().copied())
            .map(String::from)
            .collect();
        let out = host.exec.exec(HostCmd { timeout: Some(GIT_TIMEOUT * 6), ..HostCmd::new("git", argv) }).await.map_err(|e| format!("could not run git: {e:#}"))?;
        if !out.success() {
            return Err(format!("git {} in {} failed: {}", args[0], dir.display(), String::from_utf8_lossy(&out.stderr).trim()));
        }
    }
    Ok(())
}

/// Repos under `workspace`, relative: `['']` when it is in one, else its
/// subfolders that are repos, to depth 3, skipping hidden and build folders.
pub async fn scan(host: &Host, workspace: &Path) -> Vec<String> {
    if enclosing_repo(host, workspace).await.is_some() {
        return vec![String::new()];
    }
    // One walk, four levels deep: a repo at depth 3 shows its `.git` at 4.
    // The walk cannot prune "hidden", only named folders, so hidden ones are
    // dropped below; a repo is not descended into, so a repo inside one is not
    // a repo of the workspace.
    let Ok(all) = host.fs.walk(workspace, MAX_DEPTH + 1, SKIPPED).await else { return Vec::new() };
    let repos: BTreeSet<&str> = all
        .iter()
        .filter_map(|(p, _)| p.strip_suffix("/.git").or_else(|| (p == ".git").then_some("")))
        .filter(|r| !r.is_empty() && !r.split('/').any(|c| c.starts_with('.')))
        .collect();
    let dirs: BTreeSet<&str> = all.iter().filter(|(_, m)| m.kind == Kind::Dir).map(|(p, _)| p.as_str()).collect();
    repos
        .iter()
        .filter(|r| dirs.contains(*r))
        .filter(|r| !repos.iter().any(|outer| r.len() > outer.len() && r.starts_with(*outer) && r.as_bytes()[outer.len()] == b'/'))
        .map(|r| r.to_string())
        .collect()
}

pub async fn repo_root(host: &Host, workspace: &Path, rel: &str) -> Option<PathBuf> {
    if rel.is_empty() {
        enclosing_repo(host, workspace).await
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

/// A shared plan's worktree in repo `rel`: `<repo>/.sbxw-worktrees/<team>`,
/// where its threads' scripts make it.
pub async fn team_worktree(host: &Host, workspace: &Path, rel: &str, team: &str) -> Option<PathBuf> {
    repo_root(host, workspace, rel).await.map(|r| r.join(".sbxw-worktrees").join(team))
}

/// `rel` checked against the workspace: normalised, a folder under it (after
/// symlinks), and a repo.
pub async fn validate(host: &Host, workspace: &Path, rel: &str) -> Result<String, String> {
    let rel = sbxw_core::names::normalize_repo_path(rel)?;
    let shown = if rel.is_empty() { "the workspace" } else { rel.as_str() };
    let dir = workspace.join(&rel);
    let (Ok(ws), Ok(real)) = (host.fs.canonicalize(workspace).await, host.fs.canonicalize(&dir).await) else {
        return Err(format!("{shown} is not a folder of the workspace"));
    };
    if !real.starts_with(&ws) || !is_dir(host, &real).await {
        return Err(format!("{shown} is not a folder of the workspace"));
    }
    let is_repo = if rel.is_empty() { enclosing_repo(host, &real).await.is_some() } else { has_git(host, &real).await };
    if !is_repo {
        return Err(pick_a_repo(shown, &scan(host, workspace).await));
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
pub async fn choose(host: &Host, workspace: &Path, recorded: Option<&str>, thread: &str, requested: Option<&str>) -> Result<String, String> {
    let has_worktree = |rel: String| async move {
        match repo_root(host, workspace, &rel).await {
            Some(r) => is_dir(host, &r.join(".sbxw-worktrees").join(thread)).await,
            None => false,
        }
    };
    let requested = match requested {
        Some(r) => Some(validate(host, workspace, r).await?),
        None => None,
    };
    let current = match recorded {
        Some(r) => has_worktree(r.to_string()).await.then(|| r.to_string()),
        None => {
            let mut found = None;
            for r in scan(host, workspace).await {
                if has_worktree(r.clone()).await {
                    found = Some(r);
                    break;
                }
            }
            found
        }
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
            None => match scan(host, workspace).await.as_slice() {
                [only] => Ok(only.clone()),
                found => Err(pick_a_repo("the workspace", found)),
            },
        },
    }
}

/// Every thread with a worktree under the workspace, with its repo. A name in
/// two repos is listed in the one its record names, else the first.
pub async fn thread_worktrees(host: &Host, workspace: &Path, recorded: &BTreeMap<String, String>) -> Vec<(String, String)> {
    let mut out: BTreeMap<String, String> = BTreeMap::new();
    for repo in scan(host, workspace).await {
        let Some(root) = repo_root(host, workspace, &repo).await else { continue };
        let wts = root.join(".sbxw-worktrees");
        let Ok(entries) = host.fs.read_dir(&wts).await else { continue };
        for e in entries {
            // `stat`, not the entry's own kind: a link to a folder counts.
            if sbxw_core::names::is_valid_thread_name(&e.name) && is_dir(host, &wts.join(&e.name)).await {
                let mine = recorded.get(&e.name) == Some(&repo);
                if mine || !out.contains_key(&e.name) {
                    out.insert(e.name, repo.clone());
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
pub async fn head_branch(host: &Host, dir: &Path) -> Option<(String, bool)> {
    let dot = enclosing_repo(host, dir).await?.join(".git");
    let git_dir = if is_file(host, &dot).await {
        let text = String::from_utf8(host.fs.read(&dot, GIT_FILE_MAX).await.ok()?).ok()?;
        let target = PathBuf::from(text.trim().strip_prefix("gitdir:")?.trim());
        if target.is_absolute() {
            target
        } else {
            dot.parent()?.join(target)
        }
    } else {
        dot
    };
    let head = String::from_utf8(host.fs.read(&git_dir.join("HEAD"), GIT_FILE_MAX).await.ok()?).ok()?;
    Some(match head.trim().strip_prefix("ref: refs/heads/") {
        Some(b) => (b.to_string(), false),
        None => ("HEAD".to_string(), true),
    })
}

async fn is_file(host: &Host, p: &Path) -> bool {
    host.fs.stat(p).await.ok().flatten().is_some_and(|m| m.kind == Kind::File)
}

pub async fn branch_of(host: &Host, dir: &Path) -> Option<String> {
    head_branch(host, dir).await.and_then(|(b, detached)| (!detached).then_some(b))
}

pub async fn list(host: &Host, workspace: &Path) -> Vec<RepoInfo> {
    let reads = scan(host, workspace).await.into_iter().map(|rel| {
        let dir = thread_dir(workspace, &rel);
        async move {
            match git(host, &dir, &["status", "--porcelain=v2", "--branch"]).await {
                Some(raw) => parse_status(&rel, &raw),
                None => {
                    let (branch, detached) = head_branch(host, &dir).await.unwrap_or_default();
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

async fn count_status(host: &Host, dir: &Path) -> Option<u32> {
    git(host, dir, &["status", "--porcelain"]).await.map(|r| r.lines().filter(|l| !l.is_empty()).count() as u32)
}

/// Each thread's branch against the branch its repo's main checkout is on —
/// the lead's, and the one a merge lands in.
pub async fn graph(host: &Host, workspace: &Path, threads: &[(String, String)]) -> Vec<RepoBranches> {
    let mut by_repo: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for (t, repo) in threads {
        by_repo.entry(repo.as_str()).or_default().push(t.as_str());
    }
    futures_util::future::join_all(by_repo.into_iter().map(|(repo, names)| read_repo(host, workspace, repo, names))).await
}

async fn read_repo(host: &Host, workspace: &Path, rel: &str, names: Vec<&str>) -> RepoBranches {
    let mut out = RepoBranches { path: rel.to_string(), ..Default::default() };
    let Some(root) = repo_root(host, workspace, rel).await else {
        out.error = Some("the workspace is not in a git repository".into());
        return out;
    };
    let Some(head) = one_line(git(host, &root, &["rev-parse", "--verify", "--quiet", "HEAD"]).await) else {
        out.error = Some("git on the host could not read its HEAD".into());
        return out;
    };
    out.base = branch_of(host, &root).await;
    out.head = Some(head);
    let n = format!("-n{BASE_LOG}");
    let log_args = ["log", "--first-parent", n.as_str(), LOG_FORMAT, "HEAD"];
    let (log, dirty, threads) = tokio::join!(
        git(host, &root, &log_args),
        count_status(host, &root),
        futures_util::future::join_all(names.iter().map(|t| read_thread(host, &root, t)))
    );
    out.log = log.map(|t| parse_log(&t)).unwrap_or_default();
    out.dirty = dirty;
    out.threads = threads;
    out
}

async fn read_thread(host: &Host, root: &Path, name: &str) -> ThreadBranch {
    let wt = root.join(".sbxw-worktrees").join(name);
    let mut t = ThreadBranch {
        name: name.to_string(),
        branch: format!("sbxw/{name}"),
        worktree: wt.to_string_lossy().into_owned(),
        dirty: if is_dir(host, &wt).await { count_status(host, &wt).await } else { None },
        ..Default::default()
    };
    let full = format!("refs/heads/{}", t.branch);
    let Some(tip) = one_line(git(host, root, &["rev-parse", "--verify", "--quiet", &full]).await) else {
        return t;
    };
    t.fork = one_line(git(host, root, &["merge-base", "HEAD", &tip]).await);
    let range = format!("HEAD...{tip}");
    if let Some((behind, ahead)) = git(host, root, &["rev-list", "--left-right", "--count", &range]).await.as_deref().and_then(parse_counts) {
        (t.behind, t.ahead) = (behind, ahead);
    }
    if t.ahead > 0 {
        let (n, own) = (format!("-n{THREAD_LOG}"), format!("HEAD..{tip}"));
        t.commits = git(host, root, &["log", &n, LOG_FORMAT, &own]).await.map(|r| parse_log(&r)).unwrap_or_default();
    }
    t.tip = Some(tip);
    t
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::hosts;

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

    #[tokio::test]
    async fn a_chat_workspace_becomes_a_repo_once() {
        if !git_ok() {
            return;
        }
        for (n, h) in hosts() {
            let d = tempfile::tempdir().unwrap();
            init_chat_repo(&h, d.path()).await.unwrap();
            assert_eq!(scan(&h, d.path()).await, [""], "{n}");
            init_chat_repo(&h, d.path()).await.unwrap();
            run(d.path(), &["worktree", "add", "-q", "-b", "sbxw/chat-2", ".sbxw-worktrees/chat-2"]);
        }
    }

    #[tokio::test]
    async fn a_swept_chat_workspace_is_re_created() {
        if !git_ok() {
            return;
        }
        for (n, h) in hosts() {
            let d = tempfile::tempdir().unwrap();
            let gone = d.path().join("ephemeral-chat");
            init_chat_repo(&h, &gone).await.unwrap();
            assert_eq!(scan(&h, &gone).await, [""], "{n}");
        }
    }

    #[tokio::test]
    async fn scan_finds_nested_repos_and_skips_what_it_should() {
        for (n, h) in hosts() {
            let d = tempfile::tempdir().unwrap();
            for p in ["backend/.git", "web/app/.git", "web/app/vendor/x/.git", "deep/a/b/c/.git", "deep/a/b/.git/x/.git", "node_modules/x/.git", ".hidden/.git", "inner/.git/modules/y/.git"] {
                std::fs::create_dir_all(d.path().join(p)).unwrap();
            }
            std::fs::create_dir_all(d.path().join("backend/sub/.git")).unwrap();
            // A `.git` that is a file (a worktree's) counts too.
            std::fs::create_dir_all(d.path().join("wt")).unwrap();
            std::fs::write(d.path().join("wt/.git"), "gitdir: /x").unwrap();
            assert_eq!(scan(&h, d.path()).await, ["backend", "deep/a/b", "inner", "web/app", "wt"], "{n}");
            std::fs::create_dir_all(d.path().join(".git")).unwrap();
            assert_eq!(scan(&h, d.path()).await, [""], "{n}");
        }
    }

    #[test]
    fn status_is_read_from_porcelain_v2() {
        let i = parse_status("b", "# branch.oid abc\n# branch.head main\n# branch.ab +2 -1\n1 .M N... a\n? x\n");
        assert_eq!((i.branch.as_str(), i.ahead, i.behind, i.dirty), ("main", Some(2), Some(1), 2));
        assert!(parse_status("", "# branch.head (detached)\n").detached);
    }

    #[tokio::test]
    async fn a_worktree_pins_its_repo() {
        for (n, h) in hosts() {
            let d = tempfile::tempdir().unwrap();
            for p in ["a/.git", "b/.git", "a/.sbxw-worktrees/t1"] {
                std::fs::create_dir_all(d.path().join(p)).unwrap();
            }
            let w = d.path();
            assert_eq!(choose(&h, w, None, "t1", None).await.unwrap(), "a", "{n}");
            assert!(choose(&h, w, None, "t1", Some("b")).await.is_err(), "{n}");
            assert!(choose(&h, w, None, "t2", None).await.is_err(), "{n}: two repos: pick one");
            assert_eq!(choose(&h, w, None, "t2", Some("./b")).await.unwrap(), "b", "{n}");
            assert!(choose(&h, w, None, "t2", Some("../x")).await.is_err(), "{n}");
            assert!(choose(&h, w, None, "t2", Some("c")).await.is_err(), "{n}: not a folder");
            assert_eq!(thread_worktrees(&h, w, &BTreeMap::new()).await, [("t1".to_string(), "a".to_string())], "{n}");
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_repo_cannot_be_chosen_through_a_link_out_of_the_workspace() {
        for (n, h) in hosts() {
            let d = tempfile::tempdir().unwrap();
            let ws = d.path().join("ws");
            std::fs::create_dir_all(d.path().join("elsewhere/.git")).unwrap();
            std::fs::create_dir_all(ws.join("a/.git")).unwrap();
            std::os::unix::fs::symlink(d.path().join("elsewhere"), ws.join("out")).unwrap();
            assert!(validate(&h, &ws, "out").await.is_err(), "{n}");
            assert_eq!(validate(&h, &ws, "a").await.unwrap(), "a", "{n}");
        }
    }

    #[tokio::test]
    async fn the_branch_is_read_from_head_without_git() {
        for (n, h) in hosts() {
            let d = tempfile::tempdir().unwrap();
            let r = d.path().join("r");
            std::fs::create_dir_all(r.join(".git")).unwrap();
            std::fs::write(r.join(".git/HEAD"), "ref: refs/heads/feat/x\n").unwrap();
            assert_eq!(branch_of(&h, &r).await.as_deref(), Some("feat/x"), "{n}");
            // A linked worktree: `.git` is a file naming the real git dir.
            let wt = d.path().join("wt");
            std::fs::create_dir_all(&wt).unwrap();
            let gd = d.path().join("gd");
            std::fs::create_dir_all(&gd).unwrap();
            std::fs::write(gd.join("HEAD"), "0123abc\n").unwrap();
            std::fs::write(wt.join(".git"), format!("gitdir: {}\n", gd.display())).unwrap();
            assert_eq!(head_branch(&h, &wt).await, Some(("HEAD".to_string(), true)), "{n}");
            assert_eq!(branch_of(&h, &wt).await, None, "{n}");
            assert_eq!(head_branch(&h, &d.path().join("nowhere")).await, None, "{n}");
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_read_runs_no_program_the_repo_config_names() {
        use std::os::unix::fs::PermissionsExt;
        if !git_ok() {
            return;
        }
        for (n, h) in hosts() {
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
            let out = git(&h, &r, &["status", "--porcelain=v2", "--branch"]).await;
            assert!(out.is_some_and(|o| o.contains("a.txt")), "{n}");
            assert!(!marker.exists(), "{n}: a repo-configured program ran on the host");
        }
    }

    #[tokio::test]
    async fn a_thread_is_read_against_its_base() {
        if !git_ok() {
            return;
        }
        for (n, h) in hosts() {
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
            let g = graph(&h, r, &[("t".into(), "".into())]).await;
            assert_eq!(g[0].base.as_deref(), Some("main"), "{n}");
            let t = &g[0].threads[0];
            assert_eq!((t.ahead, t.behind), (1, 0), "{n}");
            assert_eq!(t.commits[0].subject, "two", "{n}");
            let repos = list(&h, r).await;
            assert_eq!(repos[0].branch, "main", "{n}");
        }
    }

    #[test]
    fn logs_and_counts_parse() {
        assert_eq!(parse_counts("3\t4\n"), Some((3, 4)));
        let c = parse_log("abc\u{1f}p1 p2\u{1f}10\u{1f}subject with \u{1f} unit\n");
        assert_eq!(c[0].parents.len(), 2);
        assert_eq!(c[0].subject, "subject with \u{1f} unit");
    }
}
