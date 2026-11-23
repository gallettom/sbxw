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

/// A stand-in session under `key`: a shell that runs until killed.
async fn session(app: &Arc<App>, key: &SessionKey, script: String) -> Arc<crate::pty::PtySession> {
    let (s, _) = app
        .sessions
        .get_or_open(key, 0, || async move { Ok(crate::pty::Spawn { program: "sh".into(), args: vec!["-c".into(), script], env: vec![] }) })
        .await
        .unwrap();
    s
}

/// The lead's terminal: what sbxw types into it lands in `typed`.
async fn lead_terminal(app: &Arc<App>, sb: &SandboxRef, typed: &Path) {
    let s = session(app, &SessionKey::agent(sb), format!("printf '\\033[?2004h'; stty raw -echo 2>/dev/null; exec cat >> '{}'", typed.display())).await;
    for _ in 0..100 {
        if sbxw_core::terminal::bracketed_paste_enabled(&s.replay()) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

async fn typed_soon(typed: &Path, what: &str) -> String {
    for _ in 0..150 {
        let s = String::from_utf8_lossy(&std::fs::read(typed).unwrap_or_default()).into_owned();
        if s.contains(what) {
            return s;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    String::from_utf8_lossy(&std::fs::read(typed).unwrap_or_default()).into_owned()
}

/// The lead proposed a merge of one thread, then replaced it by a proposal
/// of both (a re-proposal). Merge with close: both are closed, their
/// sessions ended (not only forgotten), a pane reconnecting at once does
/// not start them again, and the lead's line says which were closed.
#[tokio::test]
async fn a_re_proposed_merge_closes_and_ends_every_thread() {
    if !has_git() {
        return;
    }
    let (d, app, fake) = app_with(fake(&[]));
    let sb = SandboxRef::parse("myapp").unwrap();
    let ws = tempfile::tempdir().unwrap();
    repo(ws.path(), &[("a", 1), ("b", 1)]);
    app.store.update(&sb, |r| r.workspace = Some(ws.path().to_path_buf())).unwrap();
    let typed = d.path().join("lead.typed");
    lead_terminal(&app, &sb, &typed).await;
    let id = done_plan(&app, &sb, &["a", "b"]).await;
    let mut running = vec![];
    for n in ["a", "b"] {
        let name = ThreadName::parse(n).unwrap();
        app.store.update(&sb, |r| r.threads.entry(n.into()).or_default().repo = String::new()).unwrap();
        running.push(session(&app, &SessionKey::thread(&sb, &name), "exec sleep 60".into()).await);
    }
    app.plan_propose_merge_checked(&id, &sb, vec!["a".into()], "", vec![]).await.unwrap();
    app.plan_propose_merge_checked(&id, &sb, vec!["a".into(), "b".into()], "both", vec![]).await.unwrap();

    let m = app.plan_merge(&id, true, None).await.unwrap();
    assert_eq!(m.state, MergeState::Merged);
    assert_eq!(m.results.iter().map(|r| r.thread.as_str()).collect::<Vec<_>>(), ["a", "b"]);
    let ends = calls(&fake.log(), "sbxw-thread-end");
    for (n, s) in ["a", "b"].iter().zip(&running) {
        assert!(ends.iter().any(|c| c.contains(&format!("={n} "))), "{n} is closed: {ends:?}");
        for _ in 0..100 {
            if !s.alive() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(!s.alive(), "{n}'s session is ended");
        assert!(!app.sessions.open_threads("myapp").contains(&n.to_string()));
        let key = SessionKey::thread(&sb, &ThreadName::parse(n).unwrap());
        let err = app.open_session(&key).await.err().map(|e| format!("{e:#}")).unwrap_or_default();
        assert!(err.contains("was closed, so a reconnect"), "{n}: a reconnect does not start it again: {err:?}");
    }
    let line = typed_soon(&typed, "Closed:").await;
    assert!(line.contains("Merged: a, b.") && line.contains("Closed: a, b."), "{line}");
}

/// A close that fails is named in the merge's line to the lead, and the
/// thread's session is left as it was.
#[tokio::test]
async fn a_close_that_fails_is_named_to_the_lead() {
    if !has_git() {
        return;
    }
    let (d, app, _fake) = app_with(|a: &[String]| {
        let has = |x: &str| a.iter().any(|y| y == x);
        match a.first().map(String::as_str) {
            Some("ls") => ls(&[("myapp", "running")]),
            Some("exec") if has("sbxw-thread-merge") => Output::ok("base=main\ncommit=abc1234\nok=1\n"),
            Some("exec") if has("sbxw-thread-end") && has("=b") => Output::fail(4, "error=/w/.sbxw-worktrees/b has 2 uncommitted file(s)\n"),
            Some("exec") if has("sbxw-thread-end") => Output::ok("ok=1\n"),
            _ => sbx_answer(a),
        }
    });
    let sb = SandboxRef::parse("myapp").unwrap();
    let ws = tempfile::tempdir().unwrap();
    repo(ws.path(), &[("a", 1), ("b", 1)]);
    app.store.update(&sb, |r| r.workspace = Some(ws.path().to_path_buf())).unwrap();
    let typed = d.path().join("lead.typed");
    lead_terminal(&app, &sb, &typed).await;
    let id = done_plan(&app, &sb, &["a", "b"]).await;
    let b = session(&app, &SessionKey::thread(&sb, &ThreadName::parse("b").unwrap()), "exec sleep 60".into()).await;
    app.plan_propose_merge_checked(&id, &sb, vec!["a".into(), "b".into()], "", vec![]).await.unwrap();
    assert_eq!(app.plan_merge(&id, true, None).await.unwrap().state, MergeState::Merged);
    let line = typed_soon(&typed, "Not closed:").await;
    assert!(line.contains("Closed: a.") && line.contains("Not closed: b (") && line.contains("uncommitted"), "{line}");
    assert!(b.alive(), "b's session runs on");
}

/// Merged and closed with the Close box: thread `x` of a plan, in a repo
/// on main. Returns its key.
async fn merged_and_closed(app: &Arc<App>, sb: &SandboxRef, ws: &Path) -> SessionKey {
    repo(ws, &[("x", 1)]);
    app.store.update(sb, |r| r.workspace = Some(ws.to_path_buf())).unwrap();
    let id = done_plan(app, sb, &["x"]).await;
    let key = SessionKey::thread(sb, &ThreadName::parse("x").unwrap());
    session(app, &key, "exec sleep 60".into()).await;
    app.plan_propose_merge_checked(&id, sb, vec!["x".into()], "", vec![]).await.unwrap();
    assert_eq!(app.plan_merge(&id, true, None).await.unwrap().state, MergeState::Merged);
    key
}

/// A pane left open on the closed thread reconnects as panes do, with no
/// choices: refused, with a sentence for the pane, and no worktree or
/// branch is made for it. A pane's first open of a New thread of that name
/// carries its choices (its repo, as the desktop and the web UI send it):
/// it starts the thread again at once.
#[tokio::test]
async fn a_stale_pane_is_refused_and_a_new_thread_of_that_name_starts() {
    if !has_git() {
        return;
    }
    let (_d, app, fake) = app_with(fake(&[]));
    let sb = SandboxRef::parse("myapp").unwrap();
    let ws = tempfile::tempdir().unwrap();
    let key = merged_and_closed(&app, &sb, ws.path()).await;
    let prepares = || calls(&fake.log(), "sbxw-thread-prepare").len();
    let before = prepares();

    crate::http::ui::settle_ws_thread(&app, &sb, &key.kind, None, None, false).await.unwrap();
    let err = app.open_session(&key).await.err().map(|e| format!("{e:#}")).unwrap_or_default();
    assert!(err.contains("a reconnect does not open it again: close this pane"), "{err:?}");
    assert_eq!(prepares(), before, "no worktree, no branch");
    assert!(!app.store.get(&sb).threads.contains_key("x"), "no record made");

    crate::http::ui::settle_ws_thread(&app, &sb, &key.kind, Some(""), None, false).await.unwrap();
    let opened = app.open_session(&key).await.err().map(|e| format!("{e:#}")).unwrap_or_default();
    assert!(!opened.contains("reconnect"), "{opened:?}");
    assert_eq!(prepares(), before + 1, "its worktree is made: the thread starts");
}

/// The lead plans the same name again right after the merge, and the
/// person approves it: the thread starts, not refused as a reconnect.
#[tokio::test]
async fn a_plan_approved_with_a_closed_name_starts_it() {
    if !has_git() {
        return;
    }
    let (_d, app, fake) = app_with(fake(&[]));
    let sb = SandboxRef::parse("myapp").unwrap();
    let ws = tempfile::tempdir().unwrap();
    let key = merged_and_closed(&app, &sb, ws.path()).await;
    assert!(app.sessions.just_ended(&key));
    let before = calls(&fake.log(), "sbxw-thread-prepare").len();
    let p = app.plan_propose(&sb, "again", vec![t("x")], Variant::Split, "one", Some("lead")).await.unwrap();
    app.plan_approve(&p.id, vec![t("x")], None).await.unwrap();
    for _ in 0..150 {
        if calls(&fake.log(), "sbxw-thread-prepare").len() > before {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(calls(&fake.log(), "sbxw-thread-prepare").len(), before + 1, "x is started again");
    assert!(!app.sessions.just_ended(&key));
    let reason = app.plan(&p.id).unwrap().threads[0].summary.clone();
    assert!(!reason.contains("reconnect"), "{reason}");
}
