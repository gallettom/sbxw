//! The person's Merge, with "Close each thread once merged": every thread
//! of the merged order closed (a first merge, a re-proposed one, a late
//! one); a thread with nothing to merge (no commit of its own, its branch
//! gone after a close by hand) settled as merged with nothing instead of
//! failing for ever; a thread with commits its base lacks never settled so.

use crate::app::App;
use crate::testing::{app_with, ls, sbx_answer};
use sbxw_core::plans::{MergeState, PlanThread, ThreadState, Variant};
use sbxw_core::session::SessionKey;
use sbxw_core::{SandboxRef, ThreadName};
use sbxw_sbx::runner::Output;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

fn t(name: &str) -> PlanThread {
    PlanThread {
        name: name.into(),
        goal: "g".into(),
        scope: String::new(),
        repo: None,
        model: None,
        state: ThreadState::Starting,
        summary: String::new(),
        updated_ms: 0,
        set_by: None,
        ticket: None,
    }
}

fn has_git() -> bool {
    std::process::Command::new("git").arg("--version").output().is_ok()
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.email=someone@example.invalid", "-c", "user.name=Ada", "-c", "commit.gpgsign=false", "-c", "init.defaultBranch=main"])
        .args(args)
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// A repo on `main` with one commit, and for each `(thread, commits)` a
/// branch `sbxw/<thread>` holding that many commits of its own (0: the
/// branch is at main, a read-only thread).
fn repo(ws: &Path, branches: &[(&str, usize)]) {
    git(ws, &["init", "-q"]);
    std::fs::write(ws.join("f"), "one").unwrap();
    git(ws, &["add", "f"]);
    git(ws, &["commit", "-q", "-m", "one"]);
    for (b, n) in branches {
        git(ws, &["branch", &format!("sbxw/{b}"), "main"]);
        if *n > 0 {
            git(ws, &["checkout", "-q", &format!("sbxw/{b}")]);
            for i in 0..*n {
                let file = format!("{b}{i}");
                std::fs::write(ws.join(&file), b).unwrap();
                git(ws, &["add", &file]);
                git(ws, &["commit", "-q", "-m", &file]);
            }
            git(ws, &["checkout", "-q", "main"]);
        }
    }
}

/// A plan of `names`, approved, every thread done.
async fn done_plan(app: &Arc<App>, sb: &SandboxRef, names: &[&str]) -> String {
    let threads: Vec<PlanThread> = names.iter().map(|n| t(n)).collect();
    let p = app.plan_propose(sb, "", threads.clone(), Variant::Split, "independent", Some("lead")).await.unwrap();
    app.plans.lock().unwrap().approve(&p.id, threads, None, None, 1).unwrap();
    for n in names {
        app.plans.lock().unwrap().set_thread(&sb.to_string(), n, ThreadState::Done, "", false, 2).unwrap();
    }
    p.id
}

/// The fake sbx: every merge script succeeds, except for the threads named
/// in `fails` (as the real script does for a branch that is gone).
fn fake(fails: &'static [&'static str]) -> impl Fn(&[String]) -> Output + Send + Sync + 'static {
    move |a: &[String]| {
        let has = |x: &str| a.iter().any(|y| y == x);
        match a.first().map(String::as_str) {
            Some("ls") => ls(&[("myapp", "running")]),
            Some("exec") if has("sbxw-thread-merge") && fails.iter().any(|f| has(&format!("={f}"))) => Output::ok("error=conflict in x.rs\n"),
            Some("exec") if has("sbxw-thread-merge") => Output::ok("base=main\ncommit=abc1234\nok=1\n"),
            Some("exec") if has("sbxw-thread-end") => Output::ok("ok=1\n"),
            _ => sbx_answer(a),
        }
    }
}

fn calls(log: &[String], script: &str) -> Vec<String> {
    log.iter().filter(|c| c.contains(script)).cloned().collect()
}

/// `b` has no commit of its own (a read-only study), `c`'s branch is gone
/// (the person closed it by hand after the lead proposed it): the lead can
/// propose them, Merge settles them as merged with nothing, merges only
/// `a`, and closes all three. The merge script never runs for b or c.
#[tokio::test]
async fn a_thread_with_nothing_to_merge_settles_and_is_closed() {
    if !has_git() {
        return;
    }
    let (_d, app, fake) = app_with(fake(&[]));
    let sb = SandboxRef::parse("myapp").unwrap();
    let ws = tempfile::tempdir().unwrap();
    repo(ws.path(), &[("a", 1), ("b", 0), ("c", 1)]);
    app.store.update(&sb, |r| r.workspace = Some(ws.path().to_path_buf())).unwrap();
    let id = done_plan(&app, &sb, &["a", "b", "c"]).await;
    app.plan_propose_merge_checked(&id, &sb, vec!["a".into(), "b".into(), "c".into()], "", vec![]).await.expect("b has nothing to merge: proposed all the same");
    // c is closed by hand: its branch goes with its unmerged commit, as the
    // person chose in the close dialog.
    git(ws.path(), &["branch", "-D", "sbxw/c"]);

    let m = app.plan_merge(&id, true, None).await.unwrap();
    assert_eq!(m.state, MergeState::Merged, "{m:?}");
    let by: Vec<(&str, bool, Option<&str>)> = m.results.iter().map(|r| (r.thread.as_str(), r.ok, r.commit.as_deref())).collect();
    assert_eq!(by, [("a", true, Some("abc1234")), ("b", true, None), ("c", true, None)]);
    let note = |n: &str| m.results.iter().find(|r| r.thread == n).and_then(|r| r.note.clone()).unwrap_or_default();
    assert!(note("b").contains("nothing to merge") && note("b").contains("already in main"), "{}", note("b"));
    assert!(note("c").contains("nothing to merge") && note("c").contains("gone"), "{}", note("c"));
    let log = fake.log();
    let merges = calls(&log, "sbxw-thread-merge");
    assert_eq!(merges.len(), 1, "only a is merged: {merges:?}");
    assert!(merges[0].ends_with("=a"));
    let ends = calls(&log, "sbxw-thread-end");
    for n in ["a", "b", "c"] {
        assert!(ends.iter().any(|c| c.contains(&format!("={n} "))), "{n} is closed: {ends:?}");
    }
}

/// A proposal whose only thread's branch is gone settles at the first click
/// (it used to fail with "there is no branch" at every Retry).
#[tokio::test]
async fn a_proposal_of_only_gone_threads_settles() {
    if !has_git() {
        return;
    }
    let (_d, app, fake) = app_with(fake(&[]));
    let sb = SandboxRef::parse("myapp").unwrap();
    let ws = tempfile::tempdir().unwrap();
    repo(ws.path(), &[("study", 0)]);
    app.store.update(&sb, |r| r.workspace = Some(ws.path().to_path_buf())).unwrap();
    let id = done_plan(&app, &sb, &["study"]).await;
    app.plan_propose_merge_checked(&id, &sb, vec!["study".into()], "", vec![]).await.unwrap();
    git(ws.path(), &["branch", "-D", "sbxw/study"]);
    let m = app.plan_merge(&id, true, None).await.unwrap();
    assert_eq!(m.state, MergeState::Merged, "{m:?}");
    assert!(calls(&fake.log(), "sbxw-thread-merge").is_empty());
}

/// Work its base lacks is never "nothing to merge": a thread with commits
/// of its own goes through the merge script, and when that fails it is
/// failed, not settled, and not closed.
#[tokio::test]
async fn a_thread_with_unmerged_commits_is_never_settled() {
    if !has_git() {
        return;
    }
    let (_d, app, fake) = app_with(fake(&["a"]));
    let sb = SandboxRef::parse("myapp").unwrap();
    let ws = tempfile::tempdir().unwrap();
    repo(ws.path(), &[("a", 2), ("b", 0)]);
    app.store.update(&sb, |r| r.workspace = Some(ws.path().to_path_buf())).unwrap();
    let id = done_plan(&app, &sb, &["a", "b"]).await;
    app.plan_propose_merge_checked(&id, &sb, vec!["a".into(), "b".into()], "", vec![]).await.unwrap();
    let m = app.plan_merge(&id, true, None).await.unwrap();
    assert_eq!(m.state, MergeState::Failed);
    assert_eq!(m.results.iter().map(|r| (r.thread.as_str(), r.ok)).collect::<Vec<_>>(), [("a", false)]);
    assert!(calls(&fake.log(), "sbxw-thread-end").is_empty(), "nothing closed after a failure");
}

/// A repo git cannot read is unknown, not gone: the thread is merged as
/// before (the script says what is wrong), never settled with nothing.
#[tokio::test]
async fn an_unreadable_repo_is_not_a_gone_branch() {
    let (_d, app, fake) = app_with(fake(&[]));
    let sb = SandboxRef::parse("myapp").unwrap();
    let ws = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(ws.path().join("r/.git")).unwrap();
    app.store.update(&sb, |r| r.workspace = Some(ws.path().to_path_buf())).unwrap();
    let threads = vec![PlanThread { repo: Some("r".into()), ..t("a") }];
    let p = app.plan_propose(&sb, "", threads.clone(), Variant::Split, "independent", Some("lead")).await.unwrap();
    app.plans.lock().unwrap().approve(&p.id, threads, None, None, 1).unwrap();
    app.plans.lock().unwrap().set_thread("myapp", "a", ThreadState::Done, "", false, 2).unwrap();
    let plan = app.plan(&p.id).unwrap();
    assert!(app.thread_git(&plan).await.is_empty(), "unknown is left out");
    app.plan_propose_merge(&p.id, &sb, vec!["a".into()], "", vec![]).unwrap();
    let m = app.plan_merge(&p.id, false, None).await.unwrap();
    assert_eq!(m.results[0].commit.as_deref(), Some("abc1234"));
    assert_eq!(calls(&fake.log(), "sbxw-thread-merge").len(), 1);
}

