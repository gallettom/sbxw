//! The git repositories under a workspace, read with git on the **host**.
//!
//! The workspace is mounted at the same path in the sandbox, so a path found
//! here is the one the in-sandbox scripts `cd` into, and every read works on a
//! stopped sandbox. Every call passes `--no-optional-locks`, so a read never
//! holds the index lock a commit needs, and is bounded by a timeout
//! (`kill_on_drop` makes it real).
//!
//! A workspace is often a folder of repos (`~/dev/shop/{backend,frontend}`), so
//! a thread opens *in a repo*, named relative to the workspace (`''` is the
//! workspace itself), and that choice is kept in the sandbox's record.

use crate::host::{Host, Kind};
use sbxw_sbx::host::HostCmd;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

const MAX_DEPTH: usize = 3;
const SKIPPED: &[&str] = &["node_modules", "target", "vendor", ".sbxw-worktrees"];
const GIT_TIMEOUT: Duration = Duration::from_secs(5);
/// A `.git` file and `HEAD` are one line; this caps a hostile one.
const GIT_FILE_MAX: u64 = 64 * 1024;
const BASE_LOG: usize = 60;
const THREAD_LOG: usize = 20;
const LOG_FORMAT: &str = "--format=%H%x1f%P%x1f%ct%x1f%s";

/// Settings that make git run a program, pinned off for every host read. The
/// repo's `.git` is writable from the sandbox, so a `core.fsmonitor` set there
/// would run on the host at the next `git status` (sbx 0.50 closed the same
/// door for its own calls). `-c` on the command line outranks every config file.
const NO_PROGRAMS: &[&str] = &[
    "core.fsmonitor=false",
    "core.hooksPath=/dev/null",
    "core.pager=cat",
    "core.sshCommand=",
    "core.askPass=",
    "diff.external=",
    "credential.helper=",
];

/// Clean/smudge filters are named by the repo: find them first (`git config`
/// runs nothing), then blank them, since git runs no filter with an empty
/// command. `status` is the read that would run one.
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
    // No stdin (git must never wait for input); the host enforces the timeout
    // by killing the command. An unreachable host reads as a failed git.
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

/// The URL `origin` points at in the repo at or above `workspace`
/// (`git remote get-url origin`), trimmed. Pre-fills a tracker source only.
pub async fn origin_url(host: &Host, workspace: &Path) -> Option<String> {
    let repo = enclosing_repo(host, workspace).await?;
    let out = git(host, &repo, &["remote", "get-url", "origin"]).await?;
    let u = out.trim();
    (!u.is_empty()).then(|| u.to_string())
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

/// Make a chat workspace a repo with one empty commit: a thread is a worktree,
/// and a worktree needs a commit to branch from. Nothing is committed after
/// that. A no-op inside a repo already.
pub async fn init_chat_repo(host: &Host, dir: &Path) -> Result<(), String> {
    if has_git(host, dir).await {
        return Ok(());
    }
    // Chat workspaces live under /tmp, which macOS sweeps; `git -C` into a
    // vanished one fails before the sandbox starts.
    host.fs.create_dir_all(dir).await.map_err(|e| format!("could not re-create chat workspace {}: {e:#}", dir.display()))?;
    for args in [CHAT_INIT, CHAT_COMMIT] {
        chat_git(host, dir, args).await?;
    }
    Ok(())
}

const CHAT_INIT: &[&str] = &["init", "-q"];
/// `--no-verify`: the directory is the sandbox's to write, so a hook planted
/// between `init` and `commit` must not run (hooksPath is pinned off too).
const CHAT_COMMIT: &[&str] = &["commit", "-q", "--no-verify", "--allow-empty", "-m", "sbxw chat"];

/// One git call of [`init_chat_repo`], with [`NO_PROGRAMS`] pinned like every
/// other host git call.
async fn chat_git(host: &Host, dir: &Path, args: &[&str]) -> Result<(), String> {
    let mut argv: Vec<String> = NO_PROGRAMS.iter().flat_map(|kv| ["-c".to_string(), kv.to_string()]).collect();
    argv.extend(
        ["-C", &dir.to_string_lossy(), "-c", "user.email=sbxw@localhost", "-c", "user.name=sbxw", "-c", "init.defaultBranch=main"]
            .into_iter()
            .chain(args.iter().copied())
            .map(String::from),
    );
    let out = host.exec.exec(HostCmd { timeout: Some(GIT_TIMEOUT * 6), ..HostCmd::new("git", argv) }).await.map_err(|e| format!("could not run git: {e:#}"))?;
    if !out.success() {
        return Err(format!("git {} in {} failed: {}", args[0], dir.display(), String::from_utf8_lossy(&out.stderr).trim()));
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
    // The walk prunes only named folders, so hidden ones are dropped below. A
    // repo is not descended into, so a repo inside one is not a repo of the
    // workspace.
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
/// with a worktree cannot move: its branch, uncommitted work and conversation
/// are in the old repo.
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
    /// The tip's subject and commit date (unix), for a view of the
    /// worktrees; `None` without a branch.
    pub tip_subject: Option<String>,
    pub tip_at: Option<i64>,
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

/// Uncommitted changes in the checkout at `dir`, untracked files included.
pub async fn uncommitted(host: &Host, dir: &Path) -> Option<u32> {
    count_status(host, dir).await
}

async fn count_status(host: &Host, dir: &Path) -> Option<u32> {
    git(host, dir, &["status", "--porcelain"]).await.map(|r| r.lines().filter(|l| !l.is_empty()).count() as u32)
}

/// Each thread's branch against the branch checked out in its repo's main
/// checkout, where a merge lands.
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
    let last = match t.commits.first() {
        Some(c) => Some(c.clone()),
        None => git(host, root, &["log", "-n1", LOG_FORMAT, "--end-of-options", &tip]).await.and_then(|r| parse_log(&r).into_iter().next()),
    };
    if let Some(c) = last {
        (t.tip_subject, t.tip_at) = (Some(c.subject), Some(c.at));
    }
    t.tip = Some(tip);
    t
}

// ── Commit history (read-only, `GET /api/sandboxes/:name/commits`) ─────────

use sbxw_core::commits::{self as hist, CommitRow, FileChange};

/// A page of commits across the workspace's repositories, newest first.
#[derive(Serialize, Debug, Clone, PartialEq, Eq, Default)]
pub struct CommitPage {
    pub repos: Vec<String>,
    pub commits: Vec<CommitRow>,
    /// More commits follow this page (ask again with `before` = the last hash).
    pub more: bool,
}

/// What a client asks for: one branch (`None`: every local branch and the
/// main checkout's HEAD), after a hash, a page size. Both names are checked
/// by the caller ([`hist::valid_branch`], [`hist::valid_hash`]).
pub struct CommitAsk<'a> {
    pub branch: Option<&'a str>,
    pub before: Option<&'a str>,
    pub limit: usize,
}

/// The walk of one repo: `(hash, time, ref, repo index)` and its merged set.
async fn walk_repo(host: &Host, root: &Path, branch: Option<&str>) -> (Vec<(String, i64, String)>, BTreeSet<String>) {
    let n = format!("-n{}", hist::WALK_MAX);
    let full = branch.map(|b| format!("refs/heads/{b}"));
    let mut args = vec!["log", "--date-order", "--source", n.as_str(), hist::WALK_FORMAT];
    match &full {
        Some(r) => args.extend(["--end-of-options", r.as_str()]),
        None => args.extend(["--branches", "HEAD"]),
    }
    let rev = ["rev-list", n.as_str(), "HEAD"];
    let (walk, merged) = tokio::join!(git(host, root, &args), git(host, root, &rev));
    let walk = walk.map(|w| hist::parse_walk(&w)).unwrap_or_default();
    let merged = merged.map(|m| m.lines().map(str::to_string).collect()).unwrap_or_default();
    (walk, merged)
}

/// Rows for these hashes of one repo, in that order, with their files count.
async fn rows_of(host: &Host, root: &Path, hashes: &[String]) -> Vec<CommitRow> {
    if hashes.is_empty() {
        return vec![];
    }
    let base = ["log", "--no-walk=unsorted", "--shortstat", "--no-renames", "--no-color", hist::LIST_FORMAT];
    let with = |extra: Option<&'static str>| {
        let mut a: Vec<&str> = base.to_vec();
        a.extend(extra);
        a.push("--end-of-options");
        a.extend(hashes.iter().map(String::as_str));
        a
    };
    // A merge's files against its first parent; an older git without
    // `--diff-merges` reads it without (its merges then count no files).
    let raw = match git(host, root, &with(Some("--diff-merges=first-parent"))).await {
        Some(r) => Some(r),
        None => git(host, root, &with(None)).await,
    };
    raw.map(|r| hist::parse_list(&r)).unwrap_or_default()
}

/// The repos of the workspace with their roots.
async fn repo_roots(host: &Host, workspace: &Path) -> Vec<(String, PathBuf)> {
    let mut out = vec![];
    for rel in scan(host, workspace).await {
        if let Some(root) = repo_root(host, workspace, &rel).await {
            out.push((rel, root));
        }
    }
    out
}

/// A page of the workspace's commits: every repo under it, each with the
/// branches of its worktrees, newest first by commit date.
pub async fn commits(host: &Host, workspace: &Path, ask: &CommitAsk<'_>) -> Result<CommitPage, String> {
    let roots = repo_roots(host, workspace).await;
    if roots.is_empty() {
        return Err("the workspace is not a git repository, or git on the host cannot read it".into());
    }
    let walks = futures_util::future::join_all(roots.iter().map(|(_, root)| walk_repo(host, root, ask.branch))).await;
    // One list over every repo: (hash, time, ref, repo index), newest first.
    let mut all: Vec<(String, i64, String, usize)> = vec![];
    for (i, (walk, _)) in walks.iter().enumerate() {
        all.extend(walk.iter().map(|(h, t, r)| (h.clone(), *t, r.clone(), i)));
    }
    all.sort_by_key(|e| std::cmp::Reverse(e.1));
    let (page, more) = hist::page(&all, |e| e.0.as_str(), ask.before, ask.limit);
    let mut rows: Vec<CommitRow> = vec![];
    let reads = (0..roots.len()).map(|i| {
        let hashes: Vec<String> = page.iter().filter(|e| e.3 == i).map(|e| e.0.clone()).collect();
        let root = roots[i].1.clone();
        async move { rows_of(host, &root, &hashes).await }
    });
    let per_repo = futures_util::future::join_all(reads).await;
    for (h, _, source, i) in &page {
        let Some(mut r) = per_repo[*i].iter().find(|r| &r.hash == h).cloned() else { continue };
        r.branch = hist::branch_shown(source);
        r.thread = hist::thread_of(&r.branch);
        r.merged = walks[*i].1.contains(h);
        r.repo = roots[*i].0.clone();
        rows.push(r);
    }
    Ok(CommitPage { repos: roots.into_iter().map(|(r, _)| r).collect(), commits: rows, more })
}

/// A team's commits: each `(thread, revision range, merged)` read in every
/// repository of the workspace (a range names its own repo's refs; the
/// others answer nothing), newest first, at most `limit` per range. The
/// ranges come from sbxw's own names (`HEAD..sbxw/<thread>`, a recorded
/// merge commit's `<hash>^1..<hash>^2`), each checked here again.
pub async fn range_commits(host: &Host, workspace: &Path, ranges: &[(Option<String>, String, bool)], limit: usize) -> Result<Vec<CommitRow>, String> {
    let roots = repo_roots(host, workspace).await;
    if roots.is_empty() {
        return Err("the workspace is not a git repository, or git on the host cannot read it".into());
    }
    let n = format!("-n{}", limit.clamp(1, hist::WALK_MAX));
    let mut out: Vec<CommitRow> = vec![];
    for (thread, range, merged) in ranges {
        if !valid_range(range) {
            continue;
        }
        for (rel, root) in &roots {
            let args = ["log", "--no-merges", "--shortstat", "--no-renames", "--no-color", n.as_str(), hist::LIST_FORMAT, "--end-of-options", range.as_str()];
            let Some(raw) = git(host, root, &args).await else { continue };
            for mut r in hist::parse_list(&raw) {
                if out.iter().any(|o| o.hash == r.hash) {
                    continue;
                }
                r.thread = thread.clone();
                r.merged = *merged;
                r.branch = thread.as_ref().map(|t| format!("sbxw/{t}")).unwrap_or_default();
                r.repo = rel.clone();
                out.push(r);
            }
        }
    }
    out.sort_by_key(|r| std::cmp::Reverse(r.at));
    Ok(out)
}

/// `A..B` where each side is `HEAD`, a valid branch, or a hash with `^1`
/// or `^2`: nothing an option or a path could be read from.
fn valid_range(range: &str) -> bool {
    let Some((a, b)) = range.split_once("..") else { return false };
    let side = |x: &str| {
        x == "HEAD"
            || hist::valid_branch(x).is_ok()
            || x.strip_suffix("^1").or_else(|| x.strip_suffix("^2")).is_some_and(|h| hist::valid_hash(h).is_ok())
    };
    !b.contains("..") && side(a) && side(b)
}

/// One commit with its files and its patch, capped.
#[derive(Serialize, Debug, Clone, PartialEq, Eq, Default)]
pub struct CommitDetail {
    pub hash: String,
    pub short: String,
    pub author: String,
    pub at: i64,
    pub subject: String,
    /// The whole body, up to [`DETAIL_BODY_MAX`] bytes.
    pub body: String,
    pub repo: String,
    pub files: Vec<FileChange>,
    /// Files past [`hist::FILES_MAX`] (not listed).
    pub files_more: usize,
    /// Something was cut: a file's patch, the total, the file list.
    pub truncated: bool,
}

/// A commit's body in the detail.
pub const DETAIL_BODY_MAX: usize = 16 * 1024;

/// The commit `hash` (checked by the caller) in one of the workspace's repos.
pub async fn commit_detail(host: &Host, workspace: &Path, hash: &str) -> Result<Option<CommitDetail>, String> {
    let roots = repo_roots(host, workspace).await;
    if roots.is_empty() {
        return Err("the workspace is not a git repository, or git on the host cannot read it".into());
    }
    let obj = format!("{hash}^{{commit}}");
    for (rel, root) in &roots {
        let Some(full) = one_line(git(host, root, &["rev-parse", "--verify", "--quiet", "--end-of-options", &obj]).await) else { continue };
        if !hist::is_full_hash(&full) {
            continue;
        }
        let meta_args = ["log", "-n1", "--no-color", hist::LIST_FORMAT, "--end-of-options", full.as_str()];
        let num_args = ["show", "--format=", "--numstat", "--no-renames", "--no-color", "--diff-merges=first-parent", "--end-of-options", full.as_str()];
        let (meta, num) = tokio::join!(git(host, root, &meta_args), git(host, root, &num_args));
        let num = match num {
            Some(n) => Some(n),
            None => git(host, root, &["show", "--format=", "--numstat", "--no-renames", "--no-color", "--end-of-options", full.as_str()]).await,
        };
        let mut d = CommitDetail { repo: rel.clone(), ..Default::default() };
        if let Some(raw) = &meta {
            // The full body, not the list's first lines.
            let rec = raw.split('\u{1e}').next().unwrap_or("");
            let f: Vec<&str> = rec.splitn(6, '\u{1f}').collect();
            if f.len() == 6 {
                d.hash = f[0].trim().to_string();
                d.short = f[1].to_string();
                d.author = f[2].to_string();
                d.at = f[3].parse().unwrap_or(0);
                d.subject = f[4].to_string();
                let (b, cut) = hist::cut_at_line(f[5].trim(), DETAIL_BODY_MAX);
                d.body = b.to_string();
                d.truncated |= cut;
            }
        }
        if d.hash.is_empty() {
            d.hash = full.clone();
        }
        let mut files = num.map(|n| hist::parse_numstat(&n)).unwrap_or_default();
        if files.len() > hist::FILES_MAX {
            d.files_more = files.len() - hist::FILES_MAX;
            files.truncate(hist::FILES_MAX);
            d.truncated = true;
        }
        // The patch, asked only for text files of a sane size, by the paths
        // git itself listed (never a client's), literal pathspecs. A path git
        // printed quoted (odd characters) is listed without its patch.
        let mut budget: u64 = (hist::PATCH_TOTAL_MAX as u64) / 24;
        let mut wanted = vec![];
        for f in files.iter_mut() {
            let lines = f.added.unwrap_or(0) + f.removed.unwrap_or(0);
            if f.binary || f.path.starts_with('"') {
                continue;
            }
            if lines > hist::FILE_LINES_MAX || lines > budget {
                f.truncated = true;
                d.truncated = true;
                continue;
            }
            budget -= lines;
            wanted.push(format!(":(literal){}", f.path));
        }
        if !wanted.is_empty() {
            let mut args = vec!["show", "--format=", "--patch", "--no-color", "--no-ext-diff", "--no-textconv", "--no-renames", "--diff-merges=first-parent", "--end-of-options", full.as_str(), "--"];
            args.extend(wanted.iter().map(String::as_str));
            if let Some(p) = git(host, root, &args).await {
                d.truncated |= hist::attach_patch(&mut files, &p);
            }
        }
        d.files = files;
        return Ok(Some(d));
    }
    Ok(None)
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
    /// What a sandbox could plant between `git init` and `git commit` in its
    /// chat workspace: a pre-commit hook, a hooksPath, an fsmonitor.
    #[tokio::test]
    async fn the_chat_commit_runs_no_program_the_sandbox_planted() {
        use std::os::unix::fs::PermissionsExt;
        if !git_ok() {
            return;
        }
        for (n, h) in hosts() {
            let d = tempfile::tempdir().unwrap();
            let r = d.path().join("chat");
            std::fs::create_dir(&r).unwrap();
            chat_git(&h, &r, CHAT_INIT).await.unwrap();
            let marker = d.path().join("ran");
            let evil = d.path().join("evil.sh");
            std::fs::write(&evil, format!("#!/bin/sh\ntouch '{}'\n", marker.display())).unwrap();
            std::fs::set_permissions(&evil, std::fs::Permissions::from_mode(0o755)).unwrap();
            let hooks = d.path().join("hooks");
            std::fs::create_dir(&hooks).unwrap();
            for dir in [&hooks, &r.join(".git/hooks")] {
                std::fs::copy(&evil, dir.join("pre-commit")).unwrap();
            }
            run(&r, &["config", "core.hooksPath", hooks.to_str().unwrap()]);
            run(&r, &["config", "core.fsmonitor", evil.to_str().unwrap()]);
            chat_git(&h, &r, CHAT_COMMIT).await.unwrap();
            assert!(!marker.exists(), "{n}: a planted program ran on the host");
            assert_eq!(scan(&h, &r).await, [""], "{n}");
        }
    }

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

    fn commit_as(dir: &Path, who: &str, msg: &str) {
        let ok = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["-c", &format!("user.name={who}"), "-c", "user.email=secret.person@example.org", "commit", "-q", "--no-verify", "-am", msg])
            .output()
            .unwrap()
            .status
            .success();
        assert!(ok, "commit {msg}");
    }

    /// A repo with a main line, a thread's worktree ahead of it and one
    /// merged into it: the list holds both branches, newest first, names
    /// the thread, marks what main holds, pages, filters to a branch, and
    /// never says an email; the detail lists files and caps the patch.
    #[tokio::test]
    async fn commits_list_both_branches_and_the_detail_caps_its_patch() {
        if !git_ok() {
            return;
        }
        for (n, h) in hosts() {
            let d = tempfile::tempdir().unwrap();
            let w = d.path();
            run(w, &["init", "-q"]);
            std::fs::write(w.join("a.txt"), "one\n").unwrap();
            std::fs::write(w.join("logo.png"), [0u8, 159, 146, 150, 0, 1, 2]).unwrap();
            run(w, &["add", "."]);
            commit_as(w, "Ada", "First commit\n\nWhy it exists.");
            run(w, &["worktree", "add", "-q", "-b", "sbxw/views", ".sbxw-worktrees/views"]);
            let wt = w.join(".sbxw-worktrees/views");
            std::fs::write(wt.join("a.txt"), "one\ntwo\n").unwrap();
            std::fs::write(wt.join("logo.png"), [0u8, 159, 146, 150, 9, 9, 9, 9]).unwrap();
            let big: String = (0..3000).map(|i| format!("line {i:05} of a long file that fills the patch quickly\n")).collect();
            std::fs::write(wt.join("big.txt"), big).unwrap();
            run(&wt, &["add", "."]);
            std::thread::sleep(std::time::Duration::from_millis(1100));
            commit_as(&wt, "Bo", "Thread work");
            std::thread::sleep(std::time::Duration::from_millis(1100));
            std::fs::write(w.join("a.txt"), "one\nmain\n").unwrap();
            commit_as(w, "Ada", "Main work");

            let all = commits(&h, w, &CommitAsk { branch: None, before: None, limit: 50 }).await.unwrap();
            let subjects: Vec<&str> = all.commits.iter().map(|c| c.subject.as_str()).collect();
            assert_eq!(subjects, ["Main work", "Thread work", "First commit"], "{n}");
            let t = &all.commits[1];
            assert_eq!((t.branch.as_str(), t.thread.as_deref(), t.merged, t.files, t.author.as_str()), ("sbxw/views", Some("views"), false, 3, "Bo"), "{n}");
            assert!(all.commits[0].merged && all.commits[2].merged, "{n}: main holds them");
            assert_eq!(all.commits[2].body, "Why it exists.");
            let json = serde_json::to_string(&all).unwrap();
            assert!(!json.contains("secret.person") && !json.contains('@'), "{n}: no email");

            // Pages: one at a time, then after the first.
            let p1 = commits(&h, w, &CommitAsk { branch: None, before: None, limit: 1 }).await.unwrap();
            assert!(p1.more && p1.commits.len() == 1);
            let p2 = commits(&h, w, &CommitAsk { branch: None, before: Some(&p1.commits[0].short), limit: 1 }).await.unwrap();
            assert_eq!(p2.commits[0].subject, "Thread work");
            // One branch.
            let th = commits(&h, w, &CommitAsk { branch: Some("sbxw/views"), before: None, limit: 50 }).await.unwrap();
            assert_eq!(th.commits.iter().map(|c| c.subject.as_str()).collect::<Vec<_>>(), ["Thread work", "First commit"]);
            assert!(commits(&h, w, &CommitAsk { branch: Some("nope"), before: None, limit: 50 }).await.unwrap().commits.is_empty());

            // The detail: the binary named, not dumped; the big file cut.
            let dt = commit_detail(&h, w, &t.short).await.unwrap().expect("found");
            assert_eq!((dt.subject.as_str(), dt.author.as_str(), dt.files.len()), ("Thread work", "Bo", 3));
            let f = |p: &str| dt.files.iter().find(|f| f.path == p).unwrap().clone();
            assert!(f("a.txt").patch.contains("+two") && !f("a.txt").truncated);
            assert!(f("logo.png").binary && f("logo.png").patch.is_empty());
            let b = f("big.txt");
            assert!(b.truncated && b.patch.len() <= sbxw_core::commits::PATCH_FILE_MAX, "{n}: {}", b.patch.len());
            assert!(dt.truncated);
            assert!(!serde_json::to_string(&dt).unwrap().contains("secret.person"));
            assert!(commit_detail(&h, w, "abcdef0").await.unwrap().is_none(), "{n}: unknown");

            // The worktree view's numbers: the tip's subject and date.
            let g = graph(&h, w, &[("views".to_string(), String::new())]).await;
            let th = &g[0].threads[0];
            assert_eq!((th.tip_subject.as_deref(), th.ahead, th.behind, th.dirty), (Some("Thread work"), 1, 1, Some(0)), "{n}");
            assert!(th.tip_at.is_some());
            // Not a repo: said, not a crash.
            let empty = tempfile::tempdir().unwrap();
            assert!(commits(&h, empty.path(), &CommitAsk { branch: None, before: None, limit: 5 }).await.is_err());
        }
    }

}
