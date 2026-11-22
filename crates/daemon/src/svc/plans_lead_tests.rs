//! Lead mode across a daemon restart: the lead is kept in the sandbox's
//! record and bound again only to the agent session it was turned on in.

use crate::app::App;
use crate::bus::Event;
use crate::pty::PtySession;
use crate::store::{LeadRecord, SandboxRecord, LEAD_UNBOUND_MAX_MS};
use crate::testing::{app, app_in};
use axum::body::Body;
use axum::http::{header, Request, StatusCode};
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

fn hook(app: &App, sb: &SandboxRef, sid: &str, event: &str) {
    app.ingest_hook(sb, json!({"hook_event_name": event, "session_id": sid, "hook_version": 2}));
}

/// A sandbox with a record and a token, its main session `s1` made lead
/// through `App::lead`, as the Team button does.
async fn led(app: &Arc<App>, dir: &Path, name: &str) -> SandboxRef {
    let sb = SandboxRef::parse(name).unwrap();
    app.store.update(&sb, |r| r.token = Some(format!("tok-{name}"))).unwrap();
    main_pty(app, &sb, &dir.join(format!("{name}.typed"))).await;
    hook(app, &sb, "s1", "SessionStart");
    app.lead(&sb).await.unwrap();
    assert_eq!(app.leading(), vec![name.to_string()]);
    sb
}

fn leads_again(rx: &mut tokio::sync::broadcast::Receiver<Event>, name: &str) -> bool {
    while let Ok(ev) = rx.try_recv() {
        if matches!(&ev, Event::Lead(s) if *s == *name) {
            return true;
        }
    }
    false
}

/// Only bytes the fake agent read: what this change must never add to.
fn typed(dir: &Path, name: &str) -> Vec<u8> {
    std::fs::read(dir.join(format!("{name}.typed"))).unwrap_or_default()
}

/// Waits, up to 5 s, until what the fake agent of `name` read satisfies `done`.
async fn typed_until(dir: &Path, name: &str, done: impl Fn(&[u8]) -> bool) -> Vec<u8> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let got = typed(dir, name);
        if done(&got) || tokio::time::Instant::now() >= deadline {
            return got;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[tokio::test]
async fn the_same_session_leads_again_after_a_restart_and_says_so() {
    let (d, app) = app();
    let sb = led(&app, d.path(), "lead1").await;
    let rec = app.store.get(&sb).lead.expect("kept on disk");
    assert_eq!(rec.session_id.as_deref(), Some("s1"));
    // `/lead` and its Return were written to the old terminal; its agent
    // may read them later. Seen landed, then that terminal goes, as in a
    // real restart, so nothing of it reaches the file truncated below.
    let first = typed_until(d.path(), "lead1", |b| b.ends_with(b"\r")).await;
    assert!(first.ends_with(b"\r"), "the old terminal read /lead: {first:?}");
    app.sessions.live(&SessionKey::agent(&sb)).expect("the old terminal").kill();

    // The daemon restarts: the store is read again, the terminals are new.
    let again = app_in(d.path());
    let mut rx = again.bus.subscribe();
    assert!(again.leading().is_empty(), "no terminal yet: not a leader");
    std::fs::write(d.path().join("lead1.typed"), b"").unwrap();
    main_pty(&again, &sb, &d.path().join("lead1.typed")).await;
    assert!(again.leading().is_empty(), "its session has not reported yet");
    hook(&again, &sb, "s1", "UserPromptSubmit");
    assert!(leads_again(&mut rx, "lead1"), "the Lead event goes out on the hook post");
    assert_eq!(again.leading(), vec!["lead1".to_string()]);
    // A marker typed now reaches the agent after anything the re-binding
    // typed (one pty, in order): once it is read, nothing else may be.
    let s = again.sessions.live(&SessionKey::agent(&sb)).expect("the new terminal");
    s.write(&[b"MARK"]).await;
    let got = typed_until(d.path(), "lead1", |b| b.ends_with(b"MARK")).await;
    assert_eq!(got, b"MARK", "re-binding types nothing");
    assert!(again.store.get(&sb).lead.unwrap().bound_ms >= rec.bound_ms);
}

#[tokio::test]
async fn another_conversation_after_a_restart_is_not_a_leader() {
    let (d, app) = app();
    let sb = led(&app, d.path(), "lead1").await;
    let again = app_in(d.path());
    let mut rx = again.bus.subscribe();
    main_pty(&again, &sb, &d.path().join("lead1.typed")).await;
    hook(&again, &sb, "s2", "SessionStart");
    assert!(again.leading().is_empty());
    assert!(!leads_again(&mut rx, "lead1"));
    // Changed by plan phone-sandboxes (lead's ruling): the record waits,
    // unbound, until LEAD_UNBOUND_MAX_MS, so a wrong pick of the main session
    // is never permanent. Only the recorded session leads again.
    assert_eq!(again.store.get(&sb).lead.and_then(|r| r.session_id).as_deref(), Some("s1"), "kept, still for s1 only");
    hook(&again, &sb, "s2", "UserPromptSubmit");
    assert!(again.leading().is_empty(), "s2 is still not the one made lead");
    // The recorded session showing again as the main one: it leads again.
    hook(&again, &sb, "s2", "SessionEnd");
    hook(&again, &sb, "s1", "UserPromptSubmit");
    assert_eq!(again.leading(), vec!["lead1".to_string()]);
}

#[tokio::test]
async fn a_lead_kept_for_another_conversation_still_expires() {
    let (d, app) = app();
    let sb = led(&app, d.path(), "lead1").await;
    let old = crate::app::now_ms() - LEAD_UNBOUND_MAX_MS - 1000;
    app.store.update(&sb, |r| r.lead = Some(LeadRecord { since_ms: old, session_id: Some("s1".into()), bound_ms: old })).unwrap();
    let again = app_in(d.path());
    main_pty(&again, &sb, &d.path().join("lead1.typed")).await;
    hook(&again, &sb, "s2", "SessionStart");
    assert!(again.leading().is_empty());
    assert_eq!(again.store.get(&sb).lead, None, "past LEAD_UNBOUND_MAX_MS it is dropped");
}

#[tokio::test]
async fn a_lead_ended_before_the_restart_stays_ended() {
    let (d, app) = app();
    let sb = led(&app, d.path(), "lead1").await;
    app.lead_end(&sb).await;
    assert!(app.leading().is_empty());
    assert_eq!(app.store.get(&sb).lead, None);
    let again = app_in(d.path());
    main_pty(&again, &sb, &d.path().join("lead1.typed")).await;
    hook(&again, &sb, "s1", "UserPromptSubmit");
    assert!(again.leading().is_empty());
}

#[tokio::test]
async fn a_stopped_or_removed_sandbox_leads_no_more() {
    let (d, app) = app();
    let sb = led(&app, d.path(), "lead1").await;
    let mut rx = app.bus.subscribe();
    app.lead_gone(&sb);
    assert!(app.leading().is_empty());
    assert!(matches!(rx.try_recv(), Ok(Event::LeadEnd(s)) if s == "lead1"));
    assert_eq!(app.store.get(&sb).lead, None);
    // A removed sandbox's record is not written back by its lead.
    app.store.forget(&sb);
    app.lead_gone(&sb);
    assert!(!app.store.exists(&sb));
}

#[tokio::test]
async fn an_old_state_file_loads_with_no_leader() {
    let rec: SandboxRecord = serde_json::from_str(r#"{"token":"t","threads":{}}"#).unwrap();
    assert_eq!(rec.lead, None);
    let (d, app) = app();
    let sb = SandboxRef::parse("old").unwrap();
    app.store.update(&sb, |r| r.token = Some("t".into())).unwrap();
    let again = app_in(d.path());
    main_pty(&again, &sb, &d.path().join("old.typed")).await;
    hook(&again, &sb, "s1", "SessionStart");
    assert!(again.leading().is_empty());
}

#[tokio::test]
async fn a_forged_post_from_another_sandbox_never_binds_a_lead() {
    let (d, app) = app();
    let sb = led(&app, d.path(), "lead1").await;
    let again = app_in(d.path());
    main_pty(&again, &sb, &d.path().join("lead1.typed")).await;
    // Sandbox `other` (its token proves it is `other`) claims lead1's session.
    let other = SandboxRef::parse("other").unwrap();
    again.store.update(&other, |r| r.token = Some("tok-other".into())).unwrap();
    main_pty(&again, &other, &d.path().join("other.typed")).await;
    hook(&again, &other, "s1", "UserPromptSubmit");
    assert!(again.leading().is_empty(), "neither lead1 nor other leads");
    assert_eq!(again.store.get(&other).lead, None);
    // lead1's own session still binds it.
    hook(&again, &sb, "s1", "UserPromptSubmit");
    assert_eq!(again.leading(), vec!["lead1".to_string()]);
}

#[tokio::test]
async fn a_lead_with_no_session_known_is_not_restored() {
    let (d, app) = app();
    let sb = SandboxRef::parse("lead1").unwrap();
    app.store.update(&sb, |r| r.lead = Some(LeadRecord { since_ms: 1, session_id: None, bound_ms: crate::app::now_ms() })).unwrap();
    let again = app_in(d.path());
    main_pty(&again, &sb, &d.path().join("lead1.typed")).await;
    hook(&again, &sb, "s1", "SessionStart");
    assert!(again.leading().is_empty(), "fail closed");
    assert_eq!(again.store.get(&sb).lead, None);
}

#[tokio::test]
async fn a_lead_never_found_again_expires() {
    let (d, app) = app();
    let sb = SandboxRef::parse("lead1").unwrap();
    let old = crate::app::now_ms() - LEAD_UNBOUND_MAX_MS - 1;
    app.store.update(&sb, |r| r.lead = Some(LeadRecord { since_ms: old, session_id: Some("s1".into()), bound_ms: old })).unwrap();
    let again = app_in(d.path());
    assert!(again.leading().is_empty());
    assert_eq!(again.store.get(&sb).lead, None);
}

#[tokio::test]
async fn a_session_unknown_at_team_on_is_recorded_once_it_reports() {
    let (d, app) = app();
    let sb = SandboxRef::parse("lead1").unwrap();
    app.store.update(&sb, |r| r.token = Some("tok".into())).unwrap();
    main_pty(&app, &sb, &d.path().join("lead1.typed")).await;
    app.lead(&sb).await.unwrap();
    assert_eq!(app.store.get(&sb).lead.unwrap().session_id, None);
    hook(&app, &sb, "s7", "UserPromptSubmit");
    assert_eq!(app.store.get(&sb).lead.unwrap().session_id.as_deref(), Some("s7"));
    assert_eq!(app.leading(), vec!["lead1".to_string()]);
}

// ── The phone's leaders, after a restart ─────────────────────────────────

const TN: &str = "sbxw.tail1.ts.net";

async fn phone(router: &axum::Router, method: &str, path: &str, cookie: Option<&str>, body: Value) -> (StatusCode, Value, axum::http::HeaderMap) {
    let mut b = Request::builder().method(method).uri(path).header(header::HOST, TN);
    if method != "GET" {
        b = b.header(header::ORIGIN, format!("https://{TN}")).header(header::CONTENT_TYPE, "application/json");
    }
    if let Some(c) = cookie {
        b = b.header(header::COOKIE, format!("{}={c}", sbxw_core::remote_access::COOKIE));
    }
    let body = if method == "GET" { Body::empty() } else { Body::from(body.to_string()) };
    let res = router.clone().oneshot(b.body(body).unwrap()).await.unwrap();
    let (status, headers) = (res.status(), res.headers().clone());
    let bytes = axum::body::to_bytes(res.into_body(), 1 << 20).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null), headers)
}

async fn pair(app: &Arc<App>, router: &axum::Router) -> String {
    let code = app.remote_pairing_code().unwrap().code;
    let (_, r, _) = phone(router, "POST", "/r/pair/start", None, json!({ "code": code, "device_name": "phone" })).await;
    let id = r["request_id"].as_str().unwrap().to_string();
    app.remote_pairing_approve(&id, r["check"].as_str().unwrap()).unwrap();
    let (_, _, h) = phone(router, "GET", &format!("/r/pair/status?request_id={id}"), None, json!({})).await;
    let cookie = h[header::SET_COOKIE].to_str().unwrap();
    let prefix = format!("{}=", sbxw_core::remote_access::COOKIE);
    cookie[prefix.len()..].split(';').next().unwrap().to_string()
}

#[tokio::test]
async fn the_phone_lists_the_leader_after_a_restart() {
    let (d, app) = app();
    let sb = led(&app, d.path(), "lead1").await;
    let again = app_in(d.path());
    again.remote_test_on(TN);
    let router = crate::http::remote::remote_router(again.clone());
    let token = pair(&again, &router).await;
    main_pty(&again, &sb, &d.path().join("lead1.typed")).await;
    let (_, r, _) = phone(&router, "GET", "/r/leaders", Some(&token), json!({})).await;
    assert_eq!(r["leaders"].as_array().map(Vec::len), Some(0), "not before its session reports: {r}");
    hook(&again, &sb, "s1", "UserPromptSubmit");
    let (status, r, _) = phone(&router, "GET", "/r/leaders", Some(&token), json!({})).await;
    assert_eq!(status, StatusCode::OK);
    let list = r["leaders"].as_array().unwrap();
    assert_eq!(list.len(), 1, "{r}");
    assert_eq!(list[0]["sandbox"], "lead1");
}
