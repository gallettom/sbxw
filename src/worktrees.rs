//! Where each thread's branch stands against the branch the lead works on:
//! what the Worktrees view of the web UI draws
//! (`GET /api/sandboxes/:name/worktrees`).
//!
//! The lead is the sandbox's main session, in the main checkout of each repo.
//! A thread works in `<repo>/.sbxw-worktrees/<thread>` on `sbxw/<thread>`
//! (`sbx::thread_run_args`), and is merged into whatever branch that main
//! checkout is on (`sbx::thread_merge`). That branch is the *base* here, and
//! for each thread this reads how far apart the two are: the commits a merge
//! would bring (`ahead`), the commits the base gained since the thread forked
//! (`behind`, the ones a merge may conflict with), and the work not committed
//! yet.
//!
//! Read on the host side of the mount, like `repos`: it works on a stopped
//! sandbox and costs no `sbx exec`. Every git call goes through `repos::git`,
//! which never takes the index lock the agents need to commit.

use serde::Serialize;
use std::collections::BTreeMap;
use std::path::Path;

use crate::repos;

/// How much of the base's history is read: its first-parent commits, newest
/// first. A thread forked further back than this is shown as forked earlier.
const BASE_LOG: usize = 60;

/// How many of a thread's own commits are read, newest first. `ahead` still
/// counts them all.
const THREAD_LOG: usize = 20;

/// One `git log` line: full hash, parents, committer time, subject, split by
/// the ASCII unit separator, which no subject holds.
const LOG_FORMAT: &str = "--format=%H%x1f%P%x1f%ct%x1f%s";

#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
pub(crate) struct Commit {
    pub sha: String,
    pub parents: Vec<String>,
    /// Committer time, Unix seconds.
    pub at: i64,
    pub subject: String,
}

/// One thread's branch, against the base of its repo.
#[derive(Serialize, Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct ThreadBranch {
    pub name: String,
    /// `sbxw/<name>`.
    pub branch: String,
    /// `<repo>/.sbxw-worktrees/<name>`, the same path in the sandbox.
    pub worktree: String,
    /// The branch's commit; `None` without a branch (deleted by hand).
    pub tip: Option<String>,
    /// Where the branch and the base last met (`git merge-base`): the commit
    /// it forked from, or, once merged, its own tip.
    pub fork: Option<String>,
    /// Commits the branch has that the base lacks: what a merge would bring.
    pub ahead: u32,
    /// Commits the base gained since the fork: what a merge may conflict with.
    pub behind: u32,
    /// Files changed in the worktree and not committed; `None` when git could
    /// not say, or there is no worktree.
    pub dirty: Option<u32>,
    /// `base..branch`, newest first, at most `THREAD_LOG`.
    pub commits: Vec<Commit>,
}

/// One repo holding threads: the branch its main checkout is on, which is the
/// lead's and the one the threads merge into, and each thread against it.
#[derive(Serialize, Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct RepoBranches {
    /// Relative to the workspace; `''` is the workspace itself.
    pub path: String,
    /// The main checkout's branch; `None` on a detached HEAD.
    pub base: Option<String>,
    pub head: Option<String>,
    /// Uncommitted files in the main checkout; `None` when git could not say.
    pub dirty: Option<u32>,
    /// The base's first-parent history, newest first, at most `BASE_LOG`.
    pub log: Vec<Commit>,
    pub threads: Vec<ThreadBranch>,
    /// Why nothing could be read: no git on the host, no commit yet, or git
    /// too slow.
    pub error: Option<String>,
}

/// Every repo holding one of `threads` (`(thread, repo)`, as
/// `web::thread_worktrees` lists them), sorted by path. Repos are read in
/// parallel, and so are the threads in each.
pub(crate) fn read(workspace: &Path, threads: &[(String, String)]) -> Vec<RepoBranches> {
    let mut by_repo: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for (thread, repo) in threads {
        by_repo.entry(repo).or_default().push(thread);
    }
    std::thread::scope(|s| {
        let handles: Vec<_> = by_repo
            .iter()
            .map(|(repo, names)| s.spawn(move || read_repo(workspace, repo, names)))
            .collect();
        handles.into_iter().filter_map(|h| h.join().ok()).collect()
    })
}

fn read_repo(workspace: &Path, rel: &str, names: &[&str]) -> RepoBranches {
    let mut out = RepoBranches {
        path: rel.to_string(),
        ..Default::default()
    };
    let Some(root) = repos::repo_root(workspace, rel) else {
        out.error = Some("the workspace is not in a git repository".into());
        return out;
    };
    let Some(head) = one_line(repos::git(&root, &["rev-parse", "--verify", "--quiet", "HEAD"])) else {
        out.error = Some("git on the host could not read its HEAD".into());
        return out;
    };
    out.base = repos::branch_of(&root);
    out.head = Some(head);
    let root = root.as_path();
    let n = format!("-n{BASE_LOG}");
    std::thread::scope(|s| {
        let log = s.spawn(|| repos::git(root, &["log", "--first-parent", &n, LOG_FORMAT, "HEAD"]));
        let dirty = s.spawn(|| count_status(root));
        let threads: Vec<_> = names
            .iter()
            .map(|name| s.spawn(move || read_thread(root, name)))
            .collect();
        out.log = log.join().ok().flatten().map(|t| parse_log(&t)).unwrap_or_default();
        out.dirty = dirty.join().ok().flatten();
        out.threads = threads.into_iter().filter_map(|h| h.join().ok()).collect();
    });
    out
}

fn read_thread(root: &Path, name: &str) -> ThreadBranch {
    let wt = root.join(".sbxw-worktrees").join(name);
    let mut t = ThreadBranch {
        name: name.to_string(),
        branch: format!("sbxw/{name}"),
        worktree: wt.to_string_lossy().into_owned(),
        dirty: wt.is_dir().then(|| count_status(&wt)).flatten(),
        ..Default::default()
    };
    let full = format!("refs/heads/{}", t.branch);
    let Some(tip) = one_line(repos::git(root, &["rev-parse", "--verify", "--quiet", &full])) else {
        return t;
    };
    t.fork = one_line(repos::git(root, &["merge-base", "HEAD", &tip]));
    let range = format!("HEAD...{tip}");
    if let Some((behind, ahead)) = repos::git(root, &["rev-list", "--left-right", "--count", &range])
        .as_deref()
        .and_then(parse_counts)
    {
        (t.behind, t.ahead) = (behind, ahead);
    }
    if t.ahead > 0 {
        let (n, own) = (format!("-n{THREAD_LOG}"), format!("HEAD..{tip}"));
        t.commits = repos::git(root, &["log", &n, LOG_FORMAT, &own])
            .map(|raw| parse_log(&raw))
            .unwrap_or_default();
    }
    t.tip = Some(tip);
    t
}

/// Changed and untracked files, as `git status --porcelain` lists them.
fn count_status(dir: &Path) -> Option<u32> {
    repos::git(dir, &["status", "--porcelain"])
        .map(|raw| raw.lines().filter(|l| !l.is_empty()).count() as u32)
}

fn one_line(out: Option<String>) -> Option<String> {
    out.map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

/// `git rev-list --left-right --count`: `<left>\t<right>`.
fn parse_counts(raw: &str) -> Option<(u32, u32)> {
    let mut it = raw.split_whitespace();
    Some((it.next()?.parse().ok()?, it.next()?.parse().ok()?))
}

/// Lines of `LOG_FORMAT`.
fn parse_log(raw: &str) -> Vec<Commit> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::process::{Command, Stdio};

    fn temp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("sbxw-worktrees-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn git_ok() -> bool {
        Command::new("git")
            .arg("--version")
            .output()
            .is_ok_and(|o| o.status.success())
    }

    /// `git args…` in `dir`, as nobody in particular, signing nothing.
    fn run(dir: &Path, args: &[&str]) {
        let ok = Command::new("git")
            .args(["-c", "user.name=t", "-c", "user.email=t@example.com", "-c", "commit.gpgsign=false"])
            .args(args)
            .current_dir(dir)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success());
        assert!(ok, "git {args:?} in {}", dir.display());
    }

    fn commit(dir: &Path, file: &str, msg: &str) {
        std::fs::write(dir.join(file), msg).unwrap();
        run(dir, &["add", file]);
        run(dir, &["commit", "-q", "-m", msg]);
    }

    fn rev(dir: &Path, what: &str) -> String {
        one_line(repos::git(dir, &["rev-parse", what])).unwrap()
    }

    #[test]
    fn a_thread_is_read_against_the_branch_of_the_main_checkout() {
        if !git_ok() {
            return;
        }
        let ws = temp("read");
        run(&ws, &["init", "-q", "-b", "trunk"]);
        // As `sbx::thread_run_args` does, so the worktrees are no change.
        std::fs::write(ws.join(".git/info/exclude"), "/.sbxw-worktrees/\n").unwrap();
        commit(&ws, "a.txt", "first");
        let forked = rev(&ws, "HEAD");
        let wt = ws.join(".sbxw-worktrees/api");
        run(&ws, &["worktree", "add", "-q", "-b", "sbxw/api", wt.to_str().unwrap()]);
        commit(&wt, "b.txt", "api one");
        commit(&wt, "c.txt", "api two");
        std::fs::write(wt.join("wip.txt"), "x").unwrap();
        commit(&ws, "d.txt", "lead moved on");

        let threads = [("api".to_string(), String::new())];
        let got = read(&ws, &threads);
        assert_eq!(got.len(), 1);
        let repo = &got[0];
        assert_eq!(repo.error, None);
        assert_eq!(repo.base.as_deref(), Some("trunk"));
        assert_eq!(repo.dirty, Some(0), "the worktrees folder is no change of the lead's");
        assert_eq!(
            repo.log.iter().map(|c| c.subject.as_str()).collect::<Vec<_>>(),
            ["lead moved on", "first"]
        );
        let t = &repo.threads[0];
        assert_eq!((t.ahead, t.behind, t.dirty), (2, 1, Some(1)));
        assert_eq!(t.fork.as_deref(), Some(forked.as_str()));
        assert_eq!(t.branch, "sbxw/api");
        assert_eq!(
            t.commits.iter().map(|c| c.subject.as_str()).collect::<Vec<_>>(),
            ["api two", "api one"]
        );

        // Merged as `propose_merge` does: nothing left to bring, and its tip
        // is the second parent of the base's merge commit.
        std::fs::remove_file(wt.join("wip.txt")).unwrap();
        run(&ws, &["merge", "-q", "--no-ff", "--no-edit", "sbxw/api"]);
        let t = &read(&ws, &threads)[0].threads[0];
        assert_eq!((t.ahead, t.dirty), (0, Some(0)));
        assert_eq!(t.fork, t.tip);
        assert!(t.commits.is_empty());
        let head = &read(&ws, &threads)[0].log[0];
        assert_eq!(head.parents.get(1), t.tip.as_ref());
        let _ = std::fs::remove_dir_all(&ws);
    }

    #[test]
    fn a_thread_without_a_branch_or_a_repo_without_commits_reads_as_unknown() {
        if !git_ok() {
            return;
        }
        let ws = temp("empty");
        run(&ws, &["init", "-q", "-b", "trunk"]);
        let got = read(&ws, &[("api".to_string(), String::new())]);
        assert!(got[0].error.is_some(), "no commit yet, no HEAD");
        assert!(got[0].threads.is_empty());

        commit(&ws, "a.txt", "first");
        let t = &read(&ws, &[("api".to_string(), String::new())])[0].threads[0];
        assert_eq!((t.tip.as_ref(), t.dirty, t.ahead), (None, None, 0));
        let _ = std::fs::remove_dir_all(&ws);
    }

    #[test]
    fn log_lines_and_counts_are_parsed() {
        let raw = "aaa\u{1f}bbb ccc\u{1f}1700000000\u{1f}Merge branch 'sbxw/api'\n\
                   bbb\u{1f}\u{1f}1690000000\u{1f}a subject \u{1f} with a separator\n\n";
        let got = parse_log(raw);
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].parents, ["bbb", "ccc"]);
        assert_eq!(got[0].at, 1_700_000_000);
        assert!(got[1].parents.is_empty(), "a root commit");
        assert_eq!(got[1].subject, "a subject \u{1f} with a separator");
        assert_eq!(parse_counts("3\t2\n"), Some((3, 2)));
        assert_eq!(parse_counts(""), None);
    }
}
