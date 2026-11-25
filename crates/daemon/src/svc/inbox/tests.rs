//! The inbox: delivery by hook answer and never by typing into a busy
//! session, the fixed line for an idle one, a stopped session served at its
//! start, re-offers, persistence, long messages, who may read what, and the
//! team note cursor.

use super::{hook_answer, NUDGE_LINE};
use crate::app::App;
use crate::testing::{app, app_in};
use sbxw_core::inbox::{Kind, To};
use sbxw_core::plans::{PlanThread, Variant};
use sbxw_core::session::{SessionKey, SessionKind};
use sbxw_core::{SandboxRef, ThreadName};
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;

const BUDGET: usize = crate::svc::session_context::CONTEXT_MAX_CHARS;

fn sb() -> SandboxRef {
    SandboxRef::parse("inboxapp").unwrap()
}

fn thread(t: &str) -> SessionKind {
    SessionKind::Thread(ThreadName::parse(t).unwrap())
}

/// A hook event as status hook v5 posts it, from thread `t` (none: the
/// main session), filed as the hook route files it.
fn ev(t: Option<&str>, name: &str) -> Value {
    let mut e = json!({ "hook_event_name": name, "session_id": format!("sid-{}", t.unwrap_or("main")), "hook_version": 5, "tool_name": "Bash" });
    if let Some(t) = t {
        e["sbxw_thread"] = t.into();
    }
    e
}

fn hook(app: &Arc<App>, sb: &SandboxRef, e: &Value) {
    app.ingest_hook(sb, e.clone());
    app.inbox_hook_seen(sb, e);
}

/// What the hook route would answer `e` with, inbox included.
fn answer(app: &Arc<App>, sb: &SandboxRef, e: &Value) -> Value {
    hook(app, sb, e);
    let inbox = app.inbox_offer(sb, e, BUDGET);
    hook_answer(e["hook_event_name"].as_str().unwrap(), vec![], inbox)
}

fn context(v: &Value) -> String {
    v["hookSpecificOutput"]["additionalContext"].as_str().or(v["reason"].as_str()).unwrap_or("").to_string()
}

fn ids(v: &Value) -> Vec<String> {
    v["sbxw_inbox"]["ids"].as_array().map(|a| a.iter().map(|i| i.as_str().unwrap().to_string()).collect()).unwrap_or_default()
}

/// A thread session that echoes what is typed into it, with bracketed paste on.
async fn echo_session(app: &Arc<App>, sb: &SandboxRef, t: &str) -> Arc<crate::pty::PtySession> {
    let key = SessionKey::thread(sb, &ThreadName::parse(t).unwrap());
    let (s, _) = app
        .sessions
        .get_or_open(&key, 0, || async {
            Ok(crate::pty::Spawn { program: "sh".into(), args: vec!["-c".into(), "stty -echo 2>/dev/null; printf '\\033[?2004h> '; cat".into()], env: vec![] })
        })
        .await
        .unwrap();
    s
}

fn shown(s: &crate::pty::PtySession) -> String {
    String::from_utf8_lossy(&s.replay()).into_owned()
}

async fn until(what: impl Fn() -> bool) -> bool {
    for _ in 0..200 {
        if what() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    false
}

#[tokio::test]
async fn a_busy_session_gets_its_message_at_its_next_tool_call_and_nothing_is_typed() {
    let (_d, app) = app();
    let sb = sb();
    let pty = echo_session(&app, &sb, "a").await;
    hook(&app, &sb, &ev(Some("a"), "SessionStart"));
    hook(&app, &sb, &ev(Some("a"), "UserPromptSubmit"));
    hook(&app, &sb, &ev(Some("a"), "PreToolUse"));
    app.inbox_start();
    let id = app.deliver(&sb, &thread("a"), Kind::Answer, "relay request r-1", "The answer is 42.", None).unwrap();
    tokio::time::sleep(Duration::from_millis(2_600)).await;
    assert!(!shown(&pty).contains("sbxw"), "nothing typed into a working session: {:?}", shown(&pty));
    let v = answer(&app, &sb, &ev(Some("a"), "PostToolUse"));
    assert_eq!(v["hookSpecificOutput"]["hookEventName"], "PostToolUse");
    assert!(context(&v).contains("The answer is 42.") && context(&v).contains("· answer · from relay request r-1"), "{v}");
    assert_eq!(ids(&v), [id.clone()]);
    assert_eq!(app.inbox_ack(&sb, "sid-a", &[id]), 1);
    assert_eq!(answer(&app, &sb, &ev(Some("a"), "PostToolUse")), json!({}), "delivered once");
    pty.kill();
}

#[tokio::test]
async fn a_session_ending_its_turn_gets_its_messages_before_it_idles() {
    let (_d, app) = app();
    let sb = sb();
    hook(&app, &sb, &ev(Some("a"), "SessionStart"));
    app.deliver(&sb, &thread("a"), Kind::TeamNote, "team note 3 from lead", "Rebase on main.", None).unwrap();
    let v = answer(&app, &sb, &ev(Some("a"), "Stop"));
    assert_eq!(v["decision"], "block", "{v}");
    assert!(v["reason"].as_str().unwrap().contains("Rebase on main."));
    // Kept going once and still not acked: a second Stop lets it go idle.
    let mut again = ev(Some("a"), "Stop");
    again["stop_hook_active"] = true.into();
    assert_eq!(answer(&app, &sb, &again), json!({}), "no loop on a lost ack");
    // A message filed meanwhile still keeps it going.
    app.deliver(&sb, &thread("a"), Kind::Notice, "sbxw", "One more.", None).unwrap();
    let v = answer(&app, &sb, &again);
    assert!(v["reason"].as_str().unwrap().contains("One more.") && !v["reason"].as_str().unwrap().contains("Rebase"), "{v}");
}

#[tokio::test]
async fn an_idle_session_is_typed_the_fixed_line_and_gets_the_content_by_its_hook() {
    let (_d, app) = app();
    let sb = sb();
    let pty = echo_session(&app, &sb, "a").await;
    hook(&app, &sb, &ev(Some("a"), "SessionStart"));
    hook(&app, &sb, &ev(Some("a"), "Stop"));
    tokio::time::sleep(Duration::from_millis(2_100)).await;
    app.inbox_start();
    app.deliver(&sb, &thread("a"), Kind::Request, "relay request r-2", "SECRET-WORDS from another agent", None).unwrap();
    assert!(until(|| shown(&pty).contains(NUDGE_LINE)).await, "typed: {:?}", shown(&pty));
    assert!(!shown(&pty).contains("SECRET-WORDS"), "the message's words are never typed");
    let v = answer(&app, &sb, &ev(Some("a"), "UserPromptSubmit"));
    assert!(context(&v).contains("SECRET-WORDS"), "{v}");
    // Not typed again before the next nudge window, even unacked.
    tokio::time::sleep(Duration::from_millis(2_600)).await;
    assert_eq!(shown(&pty).matches(NUDGE_LINE).count(), 1);
    pty.kill();
}

#[tokio::test]
async fn a_session_waiting_for_the_person_or_with_an_old_hook_is_not_typed_into() {
    let (_d, app) = app();
    let sb = sb();
    let asked = echo_session(&app, &sb, "asked").await;
    hook(&app, &sb, &ev(Some("asked"), "SessionStart"));
    hook(&app, &sb, &json!({ "hook_event_name": "PreToolUse", "session_id": "sid-asked", "hook_version": 5, "sbxw_thread": "asked",
        "tool_name": "AskUserQuestion", "tool_input": { "questions": [{ "question": "Which?", "options": [{ "label": "A" }, { "label": "B" }] }] } }));
    let old = echo_session(&app, &sb, "old").await;
    let mut e = ev(Some("old"), "Stop");
    e["hook_version"] = 4.into();
    hook(&app, &sb, &e);
    tokio::time::sleep(Duration::from_millis(2_100)).await;
    app.inbox_start();
    app.deliver(&sb, &thread("asked"), Kind::Notice, "sbxw", "x", None).unwrap();
    app.deliver(&sb, &thread("old"), Kind::Notice, "sbxw", "y", None).unwrap();
    tokio::time::sleep(Duration::from_millis(2_600)).await;
    assert!(!shown(&asked).contains("sbxw"), "a question is open");
    assert!(!shown(&old).contains("sbxw"), "an old hook would not carry it");
    let mut e = ev(Some("old"), "PostToolUse");
    e["hook_version"] = 4.into();
    assert!(app.inbox_offer(&sb, &e, BUDGET).is_none(), "an old hook is never offered an inbox");
    asked.kill();
    old.kill();
}

#[tokio::test]
async fn a_stopped_session_gets_its_messages_at_its_start_and_the_inbox_survives_a_restart() {
    let (d, app) = app();
    let sb = sb();
    let id = app.deliver(&sb, &SessionKind::Agent, Kind::Merge, "sbxw", "Merged api into main.", None).unwrap();
    drop(app);
    let app = app_in(d.path());
    assert_eq!(app.inbox.lock().unwrap().pending(&sb.to_string(), &To::Main).len(), 1, "kept across a restart");
    let v = answer(&app, &sb, &ev(None, "SessionStart"));
    assert_eq!(v["hookSpecificOutput"]["hookEventName"], "SessionStart");
    assert!(context(&v).contains("Merged api into main."), "{v}");
    // No ack: offered again to the next conversation, and kept on disk.
    let next = json!({ "hook_event_name": "SessionStart", "session_id": "sid-2", "hook_version": 5 });
    let v = answer(&app, &sb, &next);
    assert_eq!(ids(&v), [id.clone()]);
    assert_eq!(app.inbox_ack(&sb, "sid-main", &[id.clone()]), 0, "an ack from a session it was not last offered to");
    assert_eq!(app.inbox_ack(&sb, "sid-2", &[id]), 1);
    let app = app_in(d.path());
    assert!(app.inbox.lock().unwrap().all().is_empty(), "the ack is on disk too");
}

#[tokio::test]
async fn a_long_message_is_given_as_its_head_and_a_file() {
    let (_d, app) = app();
    let sb = sb();
    let text = format!("START {} END", "word ".repeat(6_000));
    app.deliver(&sb, &thread("a"), Kind::Briefing, "sbxw", &text, None).unwrap();
    let v = answer(&app, &sb, &ev(Some("a"), "SessionStart"));
    let ctx = context(&v);
    assert!(ctx.chars().count() <= BUDGET, "{}", ctx.chars().count());
    assert!(ctx.contains("START") && !ctx.contains("END") && ctx.contains("/tmp/sbxw-inbox/sid-a/m-1.txt"));
    let files = v["sbxw_inbox"]["files"].as_array().unwrap();
    assert_eq!(files[0]["name"], "m-1.txt");
    assert!(files[0]["text"].as_str().unwrap().contains("END"));
}

#[tokio::test]
async fn a_thread_s_message_never_reaches_another_session() {
    let (_d, app) = app();
    let sb = sb();
    app.deliver(&sb, &thread("a"), Kind::Answer, "relay request r-3", "for a only", None).unwrap();
    for e in [ev(Some("b"), "PostToolUse"), ev(None, "PostToolUse"), ev(None, "SessionStart")] {
        assert_eq!(answer(&app, &sb, &e), json!({}), "{e}");
    }
    // A subagent of a, a background session of a, and a over SSH: not its conversation.
    let mut sub = ev(Some("a"), "PostToolUse");
    sub["agent_id"] = "x1".into();
    let mut bg = ev(Some("a"), "PostToolUse");
    bg["session_kind"] = "bg".into();
    for e in [sub, bg] {
        assert!(app.inbox_offer(&sb, &e, BUDGET).is_none(), "{e}");
    }
    let mut ssh = ev(None, "PostToolUse");
    ssh["ssh_env"] = json!(["SSH_CONNECTION"]);
    assert!(app.inbox_recipient(&sb, &ssh).is_none(), "an SSH session is not the main session");
    // Another sandbox's thread a.
    let other = SandboxRef::parse("inboxother").unwrap();
    assert_eq!(answer(&app, &other, &ev(Some("a"), "PostToolUse")), json!({}));
    assert!(context(&answer(&app, &sb, &ev(Some("a"), "PostToolUse"))).contains("for a only"));
    // Never to the lead what was for a thread, nor to a thread what was for
    // the lead; and an ack from another session removes nothing.
    assert_eq!(app.inbox_ack(&sb, "sid-main", &["m-1".into()]), 0);
    assert_eq!(app.inbox_ack(&other, "sid-a", &["m-1".into()]), 0);
    assert!(app.deliver(&sb, &SessionKind::Bash, Kind::Notice, "sbxw", "x", None).is_err(), "a shell has no inbox");
}

fn t(name: &str) -> PlanThread {
    PlanThread {
        name: name.into(),
        goal: "g".into(),
        scope: "s".into(),
        repo: None,
        model: None,
        state: sbxw_core::plans::ThreadState::Starting,
        summary: String::new(),
        updated_ms: 0,
        set_by: None,
        ticket: None,
    }
}

#[tokio::test]
async fn a_team_note_is_delivered_by_the_inbox_and_its_cursor_survives_a_lost_response() {
    let (d, app) = app();
    let sb = SandboxRef::parse("inboxteam").unwrap();
    let p = app.plan_propose(&sb, "Notes", vec![t("a"), t("b")], Variant::Split, "x", Some("Lead")).await.unwrap();
    app.plans.lock().unwrap().approve(&p.id, vec![t("a"), t("b")], None, None, crate::app::now_ms()).unwrap();
    let a = ThreadName::parse("a").unwrap();
    hook(&app, &sb, &ev(Some("a"), "SessionStart"));
    let (_, told, unsure) = app.team_note(&sb, None, "a", "info", "Fixtures moved.").unwrap();
    assert_eq!(told, ["a"]);
    assert!(unsure.is_empty(), "a's hook prints an inbox");
    // read_team_notes answers, but the answer is lost: handed again.
    assert_eq!(app.team_notes_read(&sb, Some(&a)).unwrap().len(), 1);
    assert_eq!(app.team_notes_read(&sb, Some(&a)).unwrap().len(), 1, "not read until its call came back");
    // The call came back (its PostToolUse): read, and gone from the inbox.
    let mut back = ev(Some("a"), "PostToolUse");
    back["tool_name"] = "mcp__sbxw-relay__read_team_notes".into();
    hook(&app, &sb, &back);
    assert!(app.team_notes_read(&sb, Some(&a)).unwrap().is_empty());
    assert!(app.inbox.lock().unwrap().pending(&sb.to_string(), &To::Thread("a".into())).is_empty(), "not delivered twice");
    // A second note goes by the inbox: its ack moves the cursor.
    app.team_note(&sb, None, "a", "warning", "Do not touch money.rs.").unwrap();
    let v = answer(&app, &sb, &ev(Some("a"), "PostToolUse"));
    assert!(context(&v).contains("Do not touch money.rs.") && context(&v).contains("team note"), "{v}");
    app.inbox_ack(&sb, "sid-a", &ids(&v));
    assert!(app.team_notes_read(&sb, Some(&a)).unwrap().is_empty(), "read by the inbox");
    // A restart keeps the cursor: nothing handed twice.
    app.persist_plans(&sb.to_string());
    app.forget_note_cursors();
    let again = app_in(d.path());
    assert!(again.team_notes_read(&sb, Some(&a)).unwrap().is_empty());
    assert!(std::fs::read_to_string(again.store.paths.state().join("team_cursors.json")).unwrap().contains(&p.id));
    // b never heard from: told, but named so the sender can resend.
    let (_, _, unsure) = app.team_note(&sb, None, "b", "info", "Hello b.").unwrap();
    assert_eq!(unsure, ["b"]);
}

#[tokio::test]
async fn a_plan_decided_with_no_lead_waiting_goes_to_the_lead_s_inbox() {
    let (_d, app) = app();
    let sb = SandboxRef::parse("inboxplan").unwrap();
    let p = app.plan_propose(&sb, "One", vec![t("a")], Variant::Split, "x", Some("Lead")).await.unwrap();
    // The lead's propose_plan came back pending: nobody waits. Refused.
    app.plan_refuse(&p.id, Some("too big".into())).unwrap();
    let pending = app.inbox.lock().unwrap().pending(&sb.to_string(), &To::Main).iter().map(|m| (m.kind, m.text.clone())).collect::<Vec<_>>();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].0, Kind::Plan);
    assert!(pending[0].1.contains(&p.id) && pending[0].1.contains("refused") && pending[0].1.contains("too big"));
    // A lead parked on its plan hears it there, not in its inbox.
    let q = app.plan_propose(&sb, "Two", vec![t("b")], Variant::Split, "x", Some("Lead")).await.unwrap();
    let (app2, id) = (app.clone(), q.id.clone());
    let waiting = tokio::spawn(async move { app2.plan_wait(&id, &SandboxRef::parse("inboxplan").unwrap(), Duration::from_secs(5)).await });
    tokio::time::sleep(Duration::from_millis(100)).await;
    app.plan_refuse(&q.id, None).unwrap();
    waiting.await.unwrap().unwrap();
    assert_eq!(app.inbox.lock().unwrap().pending(&sb.to_string(), &To::Main).len(), 1, "only the first");
}

async fn agent_post(app: &Arc<App>, sb: &str, path: &str, body: Value) -> (axum::http::StatusCode, Value) {
    use tower::ServiceExt;
    let token = app.store.token(SandboxRef::parse(sb).unwrap()).unwrap();
    let req = axum::http::Request::builder()
        .method("POST")
        .uri(path)
        .header("content-type", "application/json")
        .header("authorization", format!("Bearer {token}"))
        .body(axum::body::Body::from(body.to_string()))
        .unwrap();
    let res = crate::http::agent_router(app.clone()).oneshot(req).await.unwrap();
    let status = res.status();
    let bytes = axum::body::to_bytes(res.into_body(), 1 << 20).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

#[tokio::test]
async fn the_hook_route_answers_the_inbox_and_its_ack_route_takes_it_out() {
    let (_d, app) = app();
    let sb = sb();
    app.deliver(&sb, &thread("a"), Kind::Answer, "relay request r-9", "Use port 8080.", None).unwrap();
    let (st, v) = agent_post(&app, "inboxapp", "/api/hook", ev(Some("a"), "PostToolUse")).await;
    assert_eq!(st, axum::http::StatusCode::OK);
    assert!(context(&v).contains("Use port 8080."), "{v}");
    assert_eq!(v["sbxw_inbox"]["session_id"], "sid-a");
    // Another sandbox's token cannot ack it.
    let (_, w) = agent_post(&app, "inboxother", "/api/hook/inbox_ack", json!({ "session_id": "sid-a", "ids": ids(&v) })).await;
    assert_eq!(w["acked"], 0);
    let (st, w) = agent_post(&app, "inboxapp", "/api/hook/inbox_ack", json!({ "session_id": "sid-a", "ids": ids(&v) })).await;
    assert_eq!((st, w["acked"].as_u64()), (axum::http::StatusCode::OK, Some(1)));
    // A PreToolUse never carries it, and a deny stays a deny.
    app.deliver(&sb, &thread("a"), Kind::Notice, "sbxw", "later", None).unwrap();
    let (_, v) = agent_post(&app, "inboxapp", "/api/hook", ev(Some("a"), "PreToolUse")).await;
    assert_eq!(v, json!({}));
}

fn old(t: &str, name: &str) -> Value {
    let mut e = ev(Some(t), name);
    e["hook_version"] = 4.into();
    e
}

fn warnings(app: &Arc<App>, sb: &SandboxRef) -> Vec<String> {
    app.with_board_read(&sb.to_string())
        .unwrap_or_default()
        .into_iter()
        .filter(|e| e.kind == sbxw_core::board::Kind::Warning && e.by == "sbxw")
        .map(|e| e.text.unwrap_or_default())
        .collect()
}

fn hook_refreshes(fake: &sbxw_sbx::runner::Fake) -> usize {
    fake.stdins.lock().unwrap().iter().filter(|s| String::from_utf8_lossy(s).contains("const HOOK_VERSION = 5;")).count()
}

#[tokio::test]
async fn an_old_hook_with_messages_waiting_is_refreshed_then_its_next_event_gets_them() {
    let (_d, app, fake) = crate::testing::app_with(crate::testing::sbx_answer);
    let sb = SandboxRef::parse("inboxold").unwrap();
    // Nothing waits: an old hook alone refreshes nothing.
    hook(&app, &sb, &old("a", "PostToolUse"));
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(hook_refreshes(&fake), 0);
    app.deliver(&sb, &thread("a"), Kind::TeamNote, "team note 1 from lead", "Hello a.", None).unwrap();
    let e = old("a", "PostToolUse");
    hook(&app, &sb, &e);
    assert!(app.inbox_offer(&sb, &e, BUDGET).is_none(), "the old hook would print the ack part");
    assert!(until(|| hook_refreshes(&fake) == 1).await, "its status hooks are reinstalled");
    assert!(until(|| app.inbox_refreshed(&sb)).await);
    // An event of the old script still in flight: no second refresh, no warning.
    hook(&app, &sb, &old("a", "PostToolUse"));
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(hook_refreshes(&fake), 1);
    assert!(warnings(&app, &sb).is_empty());
    // The next event runs the new script: delivered.
    let v = answer(&app, &sb, &ev(Some("a"), "PostToolUse"));
    assert!(context(&v).contains("Hello a."), "{v}");
    // Another sandbox where the session keeps the old one after its refresh:
    // the person is told once.
    let sb2 = SandboxRef::parse("inboxold2").unwrap();
    app.deliver(&sb2, &thread("a"), Kind::Notice, "sbxw", "x", None).unwrap();
    hook(&app, &sb2, &old("a", "Stop"));
    assert!(until(|| app.inbox_refreshed(&sb2)).await);
    app.inbox_refresh_age(&sb2, 61_000);
    hook(&app, &sb2, &old("a", "PostToolUse"));
    hook(&app, &sb2, &old("a", "PostToolUse"));
    let w = warnings(&app, &sb2);
    assert_eq!(w.len(), 1, "{w:?}");
    assert!(w[0].contains("sbxw up inboxold2") && w[0].contains("still runs the old one"), "{}", w[0]);
    assert_eq!(app.inbox.lock().unwrap().pending("inboxold2", &To::Thread("a".into())).len(), 1, "kept for its next start");
}

#[tokio::test]
async fn a_refresh_that_fails_tells_the_person_once_and_keeps_the_messages() {
    let (_d, app, fake) = crate::testing::app_with(|a: &[String]| match a.first().map(String::as_str) {
        Some("exec") if a.iter().any(|x| x == "node") => sbxw_sbx::runner::Output::ok("SBXW-TOOLING [[\"status-hooks\",\"EACCES: permission denied\"]]\n"),
        _ => crate::testing::sbx_answer(a),
    });
    let sb = SandboxRef::parse("inboxfail").unwrap();
    app.deliver(&sb, &SessionKind::Agent, Kind::Merge, "sbxw", "Merged.", None).unwrap();
    let mut e = ev(None, "PostToolUse");
    e["hook_version"] = 4.into();
    hook(&app, &sb, &e);
    assert!(until(|| !warnings(&app, &sb).is_empty()).await, "told");
    let w = warnings(&app, &sb);
    assert!(w[0].contains("sbxw up inboxfail") && w[0].contains("EACCES"), "{}", w[0]);
    hook(&app, &sb, &e);
    hook(&app, &sb, &e);
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(warnings(&app, &sb).len(), 1, "once");
    assert_eq!(hook_refreshes(&fake), 1, "not retried at every event");
    assert_eq!(app.inbox.lock().unwrap().pending("inboxfail", &To::Main).len(), 1);
}
