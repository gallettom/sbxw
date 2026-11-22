//! Plans across Team off then Team on: the sandbox's lead follows its open
//! plans again, told once which; nothing acts on a plan while Team is off;
//! another sandbox never does; and a thread merged by hand reads as such.

use crate::app::App;
use crate::pty::PtySession;
use crate::testing::app;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use sbxw_core::plans::{PlanThread, ThreadState, Variant};
use sbxw_core::session::SessionKey;
use sbxw_core::SandboxRef;
use serde_json::{json, Value};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tower::ServiceExt;

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

async fn main_pty(app: &Arc<App>, sb: &SandboxRef, typed: &Path) -> Arc<PtySession> {
    let script = format!("printf '\\033[?2004h'; stty raw -echo 2>/dev/null; exec cat >> '{}'", typed.display());
    let (s, _) = app
        .sessions
        .get_or_open(&SessionKey::agent(sb), 0, || async move { Ok(crate::pty::Spawn { program: "sh".into(), args: vec!["-c".into(), script], env: vec![] }) })
        .await
        .unwrap();
    for _ in 0..100 {
        if sbxw_core::terminal::bracketed_paste_enabled(&s.replay()) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    s
}

/// A sandbox with a token, its main session made lead as the Team button does.
async fn led(app: &Arc<App>, dir: &Path, name: &str) -> SandboxRef {
    let sb = SandboxRef::parse(name).unwrap();
    app.store.update(&sb, |r| r.token = Some(format!("tok-{name}"))).unwrap();
    main_pty(app, &sb, &dir.join(format!("{name}.typed"))).await;
    app.ingest_hook(&sb, json!({"hook_event_name": "SessionStart", "session_id": "s1", "hook_version": 2}));
    app.lead(&sb).await.unwrap();
    assert_eq!(app.leading(), vec![name.to_string()]);
    sb
}

/// A post to the agent listener with `sb`'s token: what the relay server of
/// any session in `sb` sends, whoever wrote the body.
async fn post(app: &Arc<App>, sb: &str, path: &str, body: Value) -> (StatusCode, Value) {
    let token = app.store.token(SandboxRef::parse(sb).unwrap()).unwrap();
    let req = Request::builder()
        .method("POST")
        .uri(path)
        .header("content-type", "application/json")
        .header("authorization", format!("Bearer {token}"))
        .body(Body::from(body.to_string()))
        .unwrap();
    let res = crate::http::agent_router(app.clone()).oneshot(req).await.unwrap();
    let status = res.status();
    let bytes = axum::body::to_bytes(res.into_body(), 1 << 20).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

fn text(v: &Value) -> String {
    v.to_string()
}

/// An approved plan of `sb` whose two threads are done.
async fn approved_done(app: &Arc<App>, sb: &SandboxRef) -> String {
    let p = app.plan_propose(sb, "two parts", vec![t("a"), t("b")], Variant::Split, "independent", Some("Lead")).await.unwrap();
    app.plans.lock().unwrap().approve(&p.id, vec![t("a"), t("b")], None, None, crate::app::now_ms()).unwrap();
    for n in ["a", "b"] {
        app.plans.lock().unwrap().set_thread(&sb.to_string(), n, ThreadState::Done, "finished", false, crate::app::now_ms()).unwrap();
    }
    p.id
}

fn typed(dir: &Path, name: &str) -> String {
    String::from_utf8_lossy(&std::fs::read(dir.join(format!("{name}.typed"))).unwrap_or_default()).into_owned()
}

async fn typed_soon(dir: &Path, name: &str, what: &str) -> bool {
    for _ in 0..100 {
        if typed(dir, name).contains(what) {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    false
}

/// The person's scenario: three threads at work, Team off, Team on again a
/// few minutes later, the threads finish, the lead proposes their merge.
#[tokio::test]
async fn after_team_off_and_on_the_bound_lead_follows_and_merges_its_open_plans() {
    let (d, app) = app();
    let sb = led(&app, d.path(), "reopen-e2e").await;
    let id = approved_done(&app, &sb).await;
    assert_eq!(post(&app, "reopen-e2e", "/api/plans/status", json!({ "id": id })).await.0, StatusCode::OK);

    app.lead_end(&sb).await;
    assert!(app.plan(&id).unwrap().closed);
    for path in ["/api/plans/status", "/api/plans/propose_merge", "/api/plans/mark_done", "/api/plans/wait"] {
        let (st, v) = post(&app, "reopen-e2e", path, json!({ "id": id, "order": ["a", "b"], "thread": "a", "timeout": 0 })).await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{path}");
        assert!(text(&v).contains("the user ended lead mode, so plan"), "{path}: the message stays: {v}");
    }

    app.lead(&sb).await.unwrap();
    assert!(!app.plan(&id).unwrap().closed, "Team on gives it back");
    assert!(typed_soon(d.path(), "reopen-e2e", &format!("plan {id} (a done, b done)")).await, "told once which plans: {}", typed(d.path(), "reopen-e2e"));
    let (st, v) = post(&app, "reopen-e2e", "/api/plans/status", json!({ "id": id })).await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!(v["plan"]["closed"], Value::Null);
    let (st, _) = post(&app, "reopen-e2e", "/api/team/notes", json!({})).await;
    assert_eq!(st, StatusCode::OK, "read_team_notes finds the team again");
    let (st, v) = post(&app, "reopen-e2e", "/api/plans/propose_merge", json!({ "id": id, "order": ["a", "b"] })).await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!(v["plan"]["merge"]["state"], "proposed", "the person still clicks Merge");

    // A third Team on gives nothing back twice, and types no second notice.
    let before = typed(d.path(), "reopen-e2e").matches("Lead mode is on again").count();
    app.lead_end(&sb).await;
    app.lead(&sb).await.unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(before, 1);
    assert_eq!(typed(d.path(), "reopen-e2e").matches("Lead mode is on again").count(), 2, "each Team on tells once the plans it gives back");
}

#[tokio::test]
async fn while_team_is_off_every_gated_route_refuses_the_lead_and_a_thread_alike() {
    let (d, app) = app();
    let sb = led(&app, d.path(), "reopen-off").await;
    let id = approved_done(&app, &sb).await;
    app.lead_end(&sb).await;
    // These posts carry nothing that tells the caller: the lead's own session
    // and a thread send the same bytes, and both are refused.
    for path in ["/api/plans/status", "/api/plans/propose_merge", "/api/plans/mark_done", "/api/plans/wait"] {
        let (st, v) = post(&app, "reopen-off", path, json!({ "id": id, "order": ["a"], "thread": "a", "timeout": 0 })).await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{path}: {v}");
        assert!(text(&v).contains("the user ended lead mode"), "{path}: {v}");
    }
    let (st, v) = post(&app, "reopen-off", "/api/plans/propose", json!({ "threads": [t("c")], "justification": "x", "timeout": 0 })).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    assert!(text(&v).contains("the user ended lead mode"), "{v}");
    let (st, _) = post(&app, "reopen-off", "/api/team/notes", json!({})).await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "the lead's notes");
    // A thread goes on working: its own report and notes still answer.
    let (st, v) = post(&app, "reopen-off", "/api/plans/report", json!({ "thread": "a", "state": "done", "summary": "again" })).await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!(post(&app, "reopen-off", "/api/team/notes", json!({ "thread": "a" })).await.0, StatusCode::OK);
    // The person still sees the plan.
    assert!(app.plans_list().iter().any(|p| p.id == id));
}

#[tokio::test]
async fn a_sandbox_that_never_led_cannot_propose_or_follow_a_plan() {
    let (_d, app) = app();
    let sb = SandboxRef::parse("reopen-never").unwrap();
    app.store.update(&sb, |r| r.token = Some("tok-never".into())).unwrap();
    let id = approved_done(&app, &sb).await;
    let (st, _) = post(&app, "reopen-never", "/api/plans/propose", json!({ "threads": [t("c")], "justification": "x", "timeout": 0 })).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    assert_eq!(post(&app, "reopen-never", "/api/plans/status", json!({ "id": id })).await.0, StatusCode::BAD_REQUEST);
}

/// The gap left open (see docs/threads-and-lead.md, "Trust"): while a
/// sandbox leads, the daemon cannot tell its lead from a thread, so any
/// session of it holding a plan id can follow and propose the merge.
/// Nothing merges without the person's click.
#[tokio::test]
async fn known_gap_while_the_sandbox_leads_a_thread_with_the_id_acts_as_the_lead() {
    let (d, app) = app();
    let sb = led(&app, d.path(), "reopen-gap").await;
    let id = approved_done(&app, &sb).await;
    // A thread's relay server offers no such tool, but its token is the sandbox's.
    assert_eq!(post(&app, "reopen-gap", "/api/plans/status", json!({ "id": id, "thread": "a" })).await.0, StatusCode::OK);
    assert_eq!(post(&app, "reopen-gap", "/api/plans/propose_merge", json!({ "id": id, "order": ["a", "b"], "thread": "a" })).await.0, StatusCode::OK);
    assert_eq!(app.plan(&id).unwrap().merge.unwrap().state, sbxw_core::plans::MergeState::Proposed, "proposed, never merged");
}

#[tokio::test]
async fn another_sandbox_is_refused_whatever_it_posts_and_gets_no_plan_back() {
    let (d, app) = app();
    let sb = led(&app, d.path(), "reopen-mine").await;
    let id = approved_done(&app, &sb).await;
    app.lead_end(&sb).await;
    // Another sandbox, leading itself, turns Team on: it reopens nothing of reopen-mine.
    let other = led(&app, d.path(), "reopen-other").await;
    assert!(app.plan(&id).unwrap().closed);
    app.lead(&sb).await.unwrap();
    for path in ["/api/plans/status", "/api/plans/propose_merge", "/api/plans/mark_done", "/api/plans/wait"] {
        let (st, v) = post(&app, "reopen-other", path, json!({ "id": id, "order": ["a"], "thread": "a", "timeout": 0 })).await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{path}: {v}");
        assert!(text(&v).contains("no plan"), "{path}: {v}");
    }
    let _ = other;
}

#[tokio::test]
async fn a_refused_plan_stays_out_when_team_comes_back() {
    let (d, app) = app();
    let sb = led(&app, d.path(), "reopen-refused").await;
    let p = app.plan_propose(&sb, "no", vec![t("a")], Variant::Split, "x", Some("Lead")).await.unwrap();
    app.plan_refuse(&p.id, Some("not now".into())).unwrap();
    let q = app.plan_propose(&sb, "pending", vec![t("b")], Variant::Split, "x", Some("Lead")).await.unwrap();
    app.lead_end(&sb).await;
    app.lead(&sb).await.unwrap();
    for id in [&p.id, &q.id] {
        assert_eq!(app.plan(id).unwrap().state, sbxw_core::plans::PlanState::Refused);
    }
    let (st, v) = post(&app, "reopen-refused", "/api/plans/propose_merge", json!({ "id": p.id, "order": ["a"] })).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    assert!(text(&v).contains("not approved"), "{v}");
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(!typed(d.path(), "reopen-refused").contains("Lead mode is on again"), "nothing given back, nothing typed");
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

#[tokio::test]
async fn a_thread_merged_by_hand_reads_as_merged_outside_sbxw() {
    if std::process::Command::new("git").arg("--version").output().is_err() {
        return;
    }
    let (d, app) = app();
    let sb = led(&app, d.path(), "reopen-hand").await;
    let ws = tempfile::tempdir().unwrap();
    git(ws.path(), &["init", "-q"]);
    std::fs::write(ws.path().join("f"), "one").unwrap();
    git(ws.path(), &["add", "f"]);
    git(ws.path(), &["commit", "-q", "-m", "one"]);
    for (b, file) in [("a", "fa"), ("b", "fb")] {
        git(ws.path(), &["checkout", "-q", "-b", &format!("sbxw/{b}"), "main"]);
        std::fs::write(ws.path().join(file), b).unwrap();
        git(ws.path(), &["add", file]);
        git(ws.path(), &["commit", "-q", "-m", b]);
    }
    git(ws.path(), &["checkout", "-q", "main"]);
    // The person merged a by hand; b is not merged.
    git(ws.path(), &["merge", "-q", "--no-ff", "--no-edit", "sbxw/a"]);
    app.store.update(&sb, |r| r.workspace = Some(ws.path().to_path_buf())).unwrap();
    let id = approved_done(&app, &sb).await;

    let (st, v) = post(&app, "reopen-hand", "/api/plans/status", json!({ "id": id })).await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!(v["plan"]["threads"][0]["merged_outside"], "main");
    assert_eq!(v["plan"]["threads"][1]["merged_outside"], Value::Null, "b is not in main");
    assert_eq!(v["plan"]["threads"][0]["state"], "done", "no state invented");
    let (st, v) = post(&app, "reopen-hand", "/api/plans/propose_merge", json!({ "id": id, "order": ["a", "b"] })).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    assert!(text(&v).contains("sbxw/a is already in main: merged outside sbxw"), "{v}");
    let (st, v) = post(&app, "reopen-hand", "/api/plans/propose_merge", json!({ "id": id, "order": ["b"] })).await;
    assert_eq!(st, StatusCode::OK, "{v}");
}

/// The Kanban of a plan merged by hand: no thread waits in In test for work
/// that is in main, a thread whose branch is gone is marked, one whose branch
/// is unmerged stays, and closing the plan moves it without touching git.
#[tokio::test]
async fn the_board_places_threads_merged_by_hand_or_gone() {
    if std::process::Command::new("git").arg("--version").output().is_err() {
        return;
    }
    let (d, app) = app();
    let sb = led(&app, d.path(), "board-hand").await;
    let ws = tempfile::tempdir().unwrap();
    git(ws.path(), &["init", "-q"]);
    std::fs::write(ws.path().join("f"), "one").unwrap();
    git(ws.path(), &["add", "f"]);
    git(ws.path(), &["commit", "-q", "-m", "one"]);
    for (b, file) in [("a", "fa"), ("b", "fb")] {
        git(ws.path(), &["checkout", "-q", "-b", &format!("sbxw/{b}"), "main"]);
        std::fs::write(ws.path().join(file), b).unwrap();
        git(ws.path(), &["add", file]);
        git(ws.path(), &["commit", "-q", "-m", b]);
    }
    git(ws.path(), &["checkout", "-q", "main"]);
    git(ws.path(), &["merge", "-q", "--no-ff", "--no-edit", "sbxw/a"]);
    app.store.update(&sb, |r| r.workspace = Some(ws.path().to_path_buf())).unwrap();
    let threads = vec![t("a"), t("b"), t("c")];
    let p = app.plan_propose(&sb, "three parts", threads.clone(), Variant::Split, "independent", Some("Lead")).await.unwrap();
    app.plans.lock().unwrap().approve(&p.id, threads, None, None, crate::app::now_ms()).unwrap();
    for n in ["a", "b", "c"] {
        app.plans.lock().unwrap().set_thread(&sb.to_string(), n, ThreadState::Done, "finished", false, crate::app::now_ms()).unwrap();
    }
    let place = |app: &Arc<App>| -> std::collections::BTreeMap<String, (String, Vec<String>)> {
        let b = serde_json::to_value(app.tickets_board(Some(&sb), true)).unwrap();
        let mut out = std::collections::BTreeMap::new();
        for col in b["columns"].as_array().unwrap() {
            for c in col["cards"].as_array().unwrap() {
                let marks = c["marks"].as_array().unwrap().iter().map(|m| m["label"].as_str().unwrap().to_string()).collect();
                out.insert(c["thread"].as_str().unwrap().to_string(), (col["id"].as_str().unwrap().to_string(), marks));
            }
        }
        out
    };
    // Before git is asked: all three wait in test.
    assert_eq!(place(&app)["a"].0, "test");
    app.plans_ext_refresh(None).await;
    let got = place(&app);
    assert_eq!(got["a"], ("merged".into(), vec!["merged outside sbxw".into()]));
    assert_eq!(got["b"], ("test".into(), vec![]));
    assert_eq!(got["c"], ("merged".into(), vec!["branch gone".into()]));

    // The person closes the plan: b leaves test, git is as it was.
    let head = git(ws.path(), &["rev-parse", "sbxw/b"]);
    app.plan_dismiss(&p.id).unwrap();
    assert_eq!(place(&app)["b"], ("merged".into(), vec!["plan closed".into()]));
    assert_eq!(git(ws.path(), &["rev-parse", "sbxw/b"]), head);
}

#[tokio::test]
async fn closing_a_plan_is_refused_while_a_thread_works() {
    let (d, app) = app();
    let sb = led(&app, d.path(), "board-close").await;
    let p = app.plan_propose(&sb, "one", vec![t("a")], Variant::Split, "x", Some("Lead")).await.unwrap();
    app.plans.lock().unwrap().approve(&p.id, vec![t("a")], None, None, crate::app::now_ms()).unwrap();
    assert!(app.plan_dismiss(&p.id).unwrap_err().contains("still works"));
}
