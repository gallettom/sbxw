//! Third pass of the remote access review (thread remote-review). Same convention: finding_* passes while the weakness exists.

use super::remote::remote_router;
use crate::app::App;
use crate::testing::app;
use axum::body::Body;
use axum::http::{header, HeaderMap, Method, Request, StatusCode};
use axum::Router;
use sbxw_core::remote_access::COOKIE;
use sbxw_core::session::SessionKey;
use sbxw_core::SandboxRef;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tower::ServiceExt;

const TN: &str = "sbxw.tail1.ts.net";

fn on() -> (tempfile::TempDir, Arc<App>) {
    let (d, app) = app();
    app.remote_test_on(TN);
    (d, app)
}

struct Reply {
    status: StatusCode,
    #[allow(dead_code)]
    headers: HeaderMap,
    body: Vec<u8>,
}

impl Reply {
    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

/// A request as the sidecar forwards it, from tailnet peer `src`.
fn req(method: &str, path: &str, src: &str, cookie: Option<&str>) -> axum::http::request::Builder {
    let m: Method = method.parse().unwrap();
    let mut b = Request::builder().method(m.clone()).uri(path).header(header::HOST, TN).header("x-forwarded-for", src);
    if m != Method::GET && m != Method::HEAD {
        b = b.header(header::ORIGIN, format!("https://{TN}")).header(header::CONTENT_TYPE, "application/json");
    }
    if let Some(t) = cookie {
        b = b.header(header::COOKIE, format!("{COOKIE}={t}"));
    }
    b
}

async fn send(router: &Router, r: Request<Body>) -> Reply {
    let res = router.clone().oneshot(r).await.unwrap();
    let status = res.status();
    let headers = res.headers().clone();
    let body = match tokio::time::timeout(Duration::from_secs(3), axum::body::to_bytes(res.into_body(), 16 << 20)).await {
        Ok(b) => b.unwrap().to_vec(),
        Err(_) => b"(still streaming)".to_vec(),
    };
    Reply { status, headers, body }
}

async fn call(router: &Router, method: &str, path: &str, src: &str, cookie: Option<&str>, body: Value) -> Reply {
    let b = req(method, path, src, cookie);
    let body = if method == "GET" || method == "HEAD" { Body::empty() } else { Body::from(body.to_string()) };
    send(router, b.body(body).unwrap()).await
}

const PHONE: &str = "100.64.0.10";

async fn pair(app: &Arc<App>, router: &Router, name: &str) -> String {
    let code = app.remote_pairing_code().unwrap().code;
    let r = call(router, "POST", "/r/pair/start", PHONE, None, json!({ "code": code, "device_name": name })).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    let id = r.json()["request_id"].as_str().unwrap().to_string();
    let check = r.json()["check"].as_str().unwrap().to_string();
    app.remote_pairing_approve(&id, &check).unwrap();
    let r = call(router, "GET", &format!("/r/pair/status?request_id={id}"), PHONE, None, json!({})).await;
    assert_eq!(r.json()["state"], "paired", "{}", r.text());
    let cookie = r.headers[header::SET_COOKIE].to_str().unwrap().to_string();
    cookie[COOKIE.len() + 1..].split(';').next().unwrap().to_string()
}

/// A leader whose "terminal" records every byte typed into it.
async fn leader(app: &Arc<App>, dir: &std::path::Path, name: &str) -> (SandboxRef, PathBuf) {
    let sb = SandboxRef::parse(name).unwrap();
    let file = dir.join(format!("{name}.typed"));
    // Claude Code turns bracketed paste on at its prompt; so does the fake.
    let script = format!("printf '\\033[?2004h'; stty raw -echo 2>/dev/null; exec cat > '{}'", file.display());
    let (s, _) = app
        .sessions
        .get_or_open(&SessionKey::agent(&sb), 0, || async move { Ok(crate::pty::Spawn { program: "sh".into(), args: vec!["-c".into(), script], env: vec![] }) })
        .await
        .unwrap();
    for _ in 0..100 {
        if sbxw_core::terminal::bracketed_paste_enabled(&s.replay()) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    app.leads.lock().unwrap().insert(sb.clone(), Arc::as_ptr(&s) as usize);
    hook(app, &sb, "s1", json!({"hook_event_name": "SessionStart"}));
    (sb, file)
}

fn hook(app: &App, sb: &SandboxRef, sid: &str, mut ev: Value) {
    ev["session_id"] = sid.into();
    ev["hook_version"] = 2.into();
    app.ingest_hook(sb, ev);
}

async fn typed(file: &std::path::Path, want: &[u8]) -> Vec<u8> {
    for _ in 0..80 {
        let got = std::fs::read(file).unwrap_or_default();
        if got.windows(want.len()).any(|w| w == want) {
            return got;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    std::fs::read(file).unwrap_or_default()
}


/// T1. Interrupt now waits for the session's typing lock: while a phone
/// message (or any sbxw line, up to 45 s for a cold agent) is being typed,
/// the phone's Escape is held back until that writer is done.
#[tokio::test]
async fn finding_t1_interrupt_waits_behind_a_message_being_typed() {
    let (d, app) = on();
    let router = remote_router(app.clone());
    let token = pair(&app, &router, "phone").await;
    let (_sb, _file) = leader(&app, d.path(), "lead1").await;
    let r2 = router.clone();
    let t2 = token.clone();
    let msg = tokio::spawn(async move { call(&r2, "POST", "/r/leaders/lead1/message", PHONE, Some(&t2), json!({"text": "hello"})).await.status });
    tokio::time::sleep(Duration::from_millis(150)).await;
    let t0 = std::time::Instant::now();
    let r = call(&router, "POST", "/r/leaders/lead1/interrupt", PHONE, Some(&token), json!({})).await;
    let waited = t0.elapsed();
    assert_eq!(r.status, StatusCode::OK);
    let _ = msg.await;
    assert!(waited >= Duration::from_millis(1500), "FIXED? the Escape went out after {waited:?}");
}
