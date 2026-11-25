//! Lead states: the Team click waits for an idle agent, `/lead` is
//! confirmed by the session's `lead_ack`, the lead follows its terminal's new
//! conversation, and every fall back to off says so.

use super::{LeadPhase, ACK_REFUSED, ACK_THREAD, ACK_WAITING, LEAD_ACK_MAX_MS};
use crate::app::{now_ms, App};
use crate::bus::Event;
use crate::pty::PtySession;
use crate::store::{LeadRecord, LEAD_UNBOUND_MAX_MS};
use crate::testing::{app, app_in};
use axum::body::Body;
use axum::http::{Request, StatusCode};
use sbxw_core::session::SessionKey;
use sbxw_core::SandboxRef;
use serde_json::{json, Value};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tower::ServiceExt;

/// A main terminal whose fake agent reads what is typed into `typed`, with
/// bracketed paste on, as Claude Code at its prompt.
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

fn hook(app: &App, sb: &SandboxRef, body: Value) {
    app.ingest_hook(sb, body);
}

fn ev(sid: &str, event: &str) -> Value {
    json!({"hook_event_name": event, "session_id": sid, "hook_version": 2})
}

/// A sandbox with a token and a main terminal whose session `s1` is idle.
async fn ready(app: &Arc<App>, dir: &Path, name: &str) -> SandboxRef {
    let sb = SandboxRef::parse(name).unwrap();
    app.store.update(&sb, |r| r.token = Some(format!("tok-{name}"))).unwrap();
    main_pty(app, &sb, &dir.join(format!("{name}.typed"))).await;
    hook(app, &sb, ev("s1", "SessionStart"));
    sb
}

fn state(app: &App, name: &str) -> Option<LeadPhase> {
    app.lead_states().get(name).map(|s| s.state)
}

fn typed(dir: &Path, name: &str) -> String {
    String::from_utf8_lossy(&std::fs::read(dir.join(format!("{name}.typed"))).unwrap_or_default()).into_owned()
}

async fn typed_soon(dir: &Path, name: &str, what: &str) -> bool {
    for _ in 0..250 {
        if typed(dir, name).contains(what) {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    false
}

/// Every event of the bus since `rx` subscribed, as (name, sandbox).
fn events(rx: &mut tokio::sync::broadcast::Receiver<Event>) -> Vec<(&'static str, String)> {
    let mut out = Vec::new();
    while let Ok(ev) = rx.try_recv() {
        match ev {
            Event::Lead(s) => out.push(("lead", s.to_string())),
            Event::LeadEnd(s) => out.push(("lead-end", s.to_string())),
            _ => {}
        }
    }
    out
}

fn ends(rx: &mut tokio::sync::broadcast::Receiver<Event>, name: &str) -> usize {
    events(rx).iter().filter(|(e, s)| *e == "lead-end" && s == name).count()
}

async fn agent_post(app: &Arc<App>, sb: &str, path: &str, body: Value) -> (StatusCode, Value) {
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

#[tokio::test]
async fn a_click_at_an_idle_agent_types_lead_and_leads_only_once_confirmed() {
    let (d, app) = app();
    let sb = ready(&app, d.path(), "la").await;
    let mut rx = app.bus.subscribe();
    app.lead_request(&sb).await.unwrap();
    assert!(typed_soon(d.path(), "la", "/lead").await, "typed at once: {}", typed(d.path(), "la"));
    assert_eq!(state(&app, "la"), Some(LeadPhase::Activating));
    assert!(app.leading().is_empty(), "/api/leads lists only a confirmed lead");
    assert_eq!(app.store.get(&sb).lead, None, "not on disk before the ack");
    assert_eq!(events(&mut rx), vec![("lead", "la".to_string())]);
    // Pressed again while activating: nothing more.
    app.lead_request(&sb).await.unwrap();
    // The ack, through the agent route.
    let (st, v) = agent_post(&app, "la", "/api/lead/ack", json!({})).await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!(state(&app, "la"), Some(LeadPhase::Leading));
    assert_eq!(app.leading(), vec!["la".to_string()]);
    assert_eq!(app.store.get(&sb).lead.and_then(|r| r.session_id).as_deref(), Some("s1"));
    assert_eq!(events(&mut rx), vec![("lead", "la".to_string())]);
    assert_eq!(typed(d.path(), "la").matches("/lead").count(), 1, "typed once");
    // The snapshot's shape.
    let v = serde_json::to_value(app.lead_states()).unwrap();
    assert_eq!(v["la"]["state"], "leading");
    assert!(v["la"]["since_ms"].as_u64().unwrap() > 0);
    assert_eq!(v["la"]["reason"], "");
}

#[tokio::test]
async fn a_click_while_busy_waits_then_types_at_idle() {
    let (d, app) = app();
    let sb = ready(&app, d.path(), "lb").await;
    hook(&app, &sb, ev("s1", "UserPromptSubmit"));
    let mut rx = app.bus.subscribe();
    app.lead_request(&sb).await.unwrap();
    assert_eq!(state(&app, "lb"), Some(LeadPhase::Waiting));
    assert_eq!(events(&mut rx), vec![("lead", "lb".to_string())]);
    app.lead_tick().await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(!typed(d.path(), "lb").contains("/lead"), "nothing typed into a busy agent");
    assert_eq!(state(&app, "lb"), Some(LeadPhase::Waiting));
    // A hand-typed `/lead` meanwhile is not confirmed.
    assert_eq!(app.lead_ack(&sb, None).unwrap_err(), ACK_WAITING);
    // The turn ends: the next tick types it.
    hook(&app, &sb, ev("s1", "Stop"));
    app.lead_tick().await;
    assert!(typed_soon(d.path(), "lb", "/lead").await, "typed at idle");
    assert_eq!(state(&app, "lb"), Some(LeadPhase::Activating));
    app.lead_ack(&sb, None).unwrap();
    assert_eq!(app.leading(), vec!["lb".to_string()]);
}

#[tokio::test]
async fn a_waiting_or_activating_lead_can_be_cancelled() {
    let (d, app) = app();
    let sb = ready(&app, d.path(), "lc").await;
    hook(&app, &sb, ev("s1", "UserPromptSubmit"));
    app.lead_request(&sb).await.unwrap();
    let mut rx = app.bus.subscribe();
    app.lead_cancel(&sb).unwrap();
    assert_eq!(state(&app, "lc"), None, "off is absent");
    assert_eq!(ends(&mut rx, "lc"), 1);
    hook(&app, &sb, ev("s1", "Stop"));
    app.lead_tick().await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(!typed(d.path(), "lc").contains("/lead"), "a cancelled wait types nothing");
    assert!(app.lead_cancel(&sb).is_err(), "nothing to cancel");
    // Activating, cancelled: a later ack is refused.
    app.lead_request(&sb).await.unwrap();
    assert_eq!(state(&app, "lc"), Some(LeadPhase::Activating));
    app.lead_cancel(&sb).unwrap();
    assert_eq!(app.lead_ack(&sb, None).unwrap_err(), ACK_REFUSED);
    assert!(app.leading().is_empty());
}

#[tokio::test]
async fn a_wait_past_its_limit_falls_back_to_off_with_a_reason() {
    let (d, app) = app();
    let sb = ready(&app, d.path(), "ld").await;
    hook(&app, &sb, ev("s1", "UserPromptSubmit"));
    app.lead_request(&sb).await.unwrap();
    app.lead_pending.lock().unwrap().get_mut(&sb).unwrap().until_ms = now_ms() - 1;
    let mut rx = app.bus.subscribe();
    app.lead_tick().await;
    assert_eq!(ends(&mut rx, "ld"), 1);
    let s = app.lead_states().get("ld").cloned().expect("off, with its reason");
    assert_eq!(s.state, LeadPhase::Off);
    assert!(s.reason.contains("not idle within 10 minutes"), "{}", s.reason);
}

#[tokio::test]
async fn no_ack_in_time_falls_back_to_off_with_a_reason() {
    let (d, app) = app();
    let sb = ready(&app, d.path(), "le").await;
    app.lead_request(&sb).await.unwrap();
    assert_eq!(state(&app, "le"), Some(LeadPhase::Activating));
    let until = app.lead_pending_of(&sb).unwrap().until_ms;
    assert!(until <= now_ms() + LEAD_ACK_MAX_MS);
    app.lead_pending.lock().unwrap().get_mut(&sb).unwrap().until_ms = now_ms() - 1;
    let mut rx = app.bus.subscribe();
    app.lead_tick().await;
    assert_eq!(ends(&mut rx, "le"), 1);
    let s = app.lead_states().get("le").cloned().unwrap();
    assert_eq!(s.state, LeadPhase::Off);
    assert!(s.reason.contains("did not confirm"), "{}", s.reason);
    assert!(app.leading().is_empty());
    assert_eq!(app.store.get(&sb).lead, None);
    assert_eq!(app.lead_ack(&sb, None).unwrap_err(), ACK_REFUSED, "a late ack is refused");
    // Pressed again: the reason goes.
    app.lead_request(&sb).await.unwrap();
    assert_eq!(app.lead_states()["le"].reason, "");
}

#[tokio::test]
async fn a_hand_typed_lead_is_refused_by_lead_ack() {
    let (d, app) = app();
    let sb = ready(&app, d.path(), "lf").await;
    let (st, v) = agent_post(&app, "lf", "/api/lead/ack", json!({})).await;
    assert_eq!(st, StatusCode::CONFLICT);
    assert!(v.to_string().contains("press Team in sbxw"), "{v}");
    assert!(app.leading().is_empty(), "no lead created");
    assert_eq!(state(&app, "lf"), None);
    assert_eq!(app.store.get(&sb).lead, None);
    // A thread's ack, even while the main session activates, is refused.
    app.lead_request(&sb).await.unwrap();
    let (st, v) = agent_post(&app, "lf", "/api/lead/ack", json!({ "thread": "api" })).await;
    assert_eq!(st, StatusCode::CONFLICT);
    assert!(v.to_string().contains(ACK_THREAD), "{v}");
    assert_eq!(state(&app, "lf"), Some(LeadPhase::Activating));
}

#[tokio::test]
async fn a_clear_in_the_bound_terminal_gives_the_lead_back_until_the_ack() {
    let (d, app) = app();
    let sb = ready(&app, d.path(), "lg").await;
    app.lead_request(&sb).await.unwrap();
    app.lead_ack(&sb, None).unwrap();
    let mut rx = app.bus.subscribe();
    // `/clear`: a new conversation in the same terminal.
    hook(&app, &sb, ev("s1", "SessionEnd"));
    let start = json!({"hook_event_name": "SessionStart", "session_id": "s2", "source": "clear", "hook_version": 2});
    hook(&app, &sb, start.clone());
    let ctx = app.session_start_context(&sb, &start).expect("the new conversation is told");
    assert!(ctx.contains("lead_ack"), "{ctx}");
    assert!(ctx.contains("lead instructions"), "{ctx}");
    assert_eq!(state(&app, "lg"), Some(LeadPhase::Activating));
    assert!(app.leading().is_empty(), "not leading until it confirms");
    assert_eq!(events(&mut rx), vec![("lead", "lg".to_string())]);
    app.lead_ack(&sb, None).unwrap();
    assert_eq!(app.leading(), vec!["lg".to_string()]);
    assert_eq!(app.store.get(&sb).lead.and_then(|r| r.session_id).as_deref(), Some("s2"), "the record follows");
    // Through the hook route, a compaction (same session id) does the same.
    let (st, v) = agent_post(&app, "lg", "/api/hook", json!({"hook_event_name": "SessionStart", "session_id": "s2", "source": "compact", "hook_version": 2})).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(v["hookSpecificOutput"]["hookEventName"], "SessionStart");
    assert!(v["hookSpecificOutput"]["additionalContext"].as_str().unwrap().contains("lead_ack"), "{v}");
    assert_eq!(state(&app, "lg"), Some(LeadPhase::Activating));
    // A resume of the same conversation with no compaction says nothing.
    app.lead_ack(&sb, None).unwrap();
    let again = json!({"hook_event_name": "SessionStart", "session_id": "s2", "source": "resume", "hook_version": 2});
    assert_eq!(app.session_start_context(&sb, &again), None);
}

#[tokio::test]
async fn a_new_conversation_never_creates_a_lead() {
    let (d, app) = app();
    let sb = ready(&app, d.path(), "lh").await;
    let start = json!({"hook_event_name": "SessionStart", "session_id": "s2", "source": "clear", "hook_version": 2});
    hook(&app, &sb, start.clone());
    assert_eq!(app.session_start_context(&sb, &start), None, "no lead, nothing given");
    assert_eq!(state(&app, "lh"), None);
    // A thread starting in a leading sandbox is given nothing either.
    app.lead_request(&sb).await.unwrap();
    app.lead_ack(&sb, None).unwrap();
    let thread = json!({"hook_event_name": "SessionStart", "session_id": "t1", "source": "startup", "sbxw_thread": "api", "hook_version": 2});
    hook(&app, &sb, thread.clone());
    assert_eq!(app.session_start_context(&sb, &thread), None);
    assert_eq!(app.leading(), vec!["lh".to_string()]);
}

#[tokio::test]
async fn a_dead_terminal_sends_lead_end() {
    let (d, app) = app();
    let sb = ready(&app, d.path(), "li").await;
    app.lead_request(&sb).await.unwrap();
    app.lead_ack(&sb, None).unwrap();
    let mut rx = app.bus.subscribe();
    app.sessions.live(&SessionKey::agent(&sb)).unwrap().kill();
    for _ in 0..250 {
        if app.sessions.live(&SessionKey::agent(&sb)).is_none() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(app.leading().is_empty());
    assert_eq!(ends(&mut rx, "li"), 1, "the drop is told");
    assert!(app.leading().is_empty());
    assert_eq!(ends(&mut rx, "li"), 0, "once");
    assert!(app.store.get(&sb).lead.is_some(), "the record waits for its conversation");
}

#[tokio::test]
async fn an_activating_lead_whose_terminal_dies_is_off() {
    let (d, app) = app();
    let sb = ready(&app, d.path(), "lj").await;
    app.lead_request(&sb).await.unwrap();
    let mut rx = app.bus.subscribe();
    app.sessions.live(&SessionKey::agent(&sb)).unwrap().kill();
    for _ in 0..250 {
        if app.sessions.live(&SessionKey::agent(&sb)).is_none() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    app.lead_tick().await;
    assert_eq!(ends(&mut rx, "lj"), 1);
    assert_eq!(state(&app, "lj"), Some(LeadPhase::Off));
    assert_eq!(app.lead_ack(&sb, None).unwrap_err(), ACK_REFUSED);
}

#[tokio::test]
async fn every_dropped_record_sends_lead_end() {
    let (d, app) = app();
    // No session id ever recorded, expired, another conversation past the limit.
    let old = now_ms() - LEAD_UNBOUND_MAX_MS - 1000;
    let cases = [
        ("lk", LeadRecord { since_ms: 1, session_id: None, bound_ms: now_ms() }),
        ("ll", LeadRecord { since_ms: old, session_id: Some("s1".into()), bound_ms: old }),
        ("lm", LeadRecord { since_ms: old, session_id: Some("s1".into()), bound_ms: old }),
    ];
    for (name, rec) in &cases {
        let sb = SandboxRef::parse(name).unwrap();
        app.store.update(&sb, |r| r.lead = Some(rec.clone())).unwrap();
    }
    let again = app_in(d.path());
    let lm = SandboxRef::parse("lm").unwrap();
    again.store.update(&lm, |r| r.token = Some("tok-lm".into())).unwrap();
    main_pty(&again, &lm, &d.path().join("lm.typed")).await;
    let mut rx = again.bus.subscribe();
    hook(&again, &lm, ev("s2", "SessionStart"));
    assert!(again.leading().is_empty());
    for (name, _) in &cases {
        assert_eq!(again.store.get(&SandboxRef::parse(name).unwrap()).lead, None, "{name} dropped");
    }
    let got = events(&mut rx);
    for (name, _) in &cases {
        assert_eq!(got.iter().filter(|(e, s)| *e == "lead-end" && s == name).count(), 1, "{name}: {got:?}");
        assert_eq!(again.lead_states()[*name].state, LeadPhase::Off, "{name} has its reason");
    }
}

#[tokio::test]
async fn the_phone_path_still_types_only_at_idle_and_needs_the_ack() {
    let (d, app) = app();
    let sb = ready(&app, d.path(), "ln").await;
    // `App::lead` is what the phone calls once its own gate passed.
    app.lead(&sb).await.unwrap();
    assert!(typed_soon(d.path(), "ln", "/lead").await);
    assert_eq!(state(&app, "ln"), Some(LeadPhase::Activating));
    app.lead(&sb).await.unwrap();
    assert_eq!(typed(d.path(), "ln").matches("/lead").count(), 1, "idempotent");
    // Ending a waiting lead types nothing; ending an activating one says so.
    app.lead_end(&sb).await;
    assert_eq!(state(&app, "ln"), None);
}
