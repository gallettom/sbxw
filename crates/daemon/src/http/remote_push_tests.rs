//! Web Push (API.md 2.8, API-push.md): the three `/r/push` routes and
//! `/sw.js` behind the remote guard, the subscription's validation, its
//! deletion on every exit, the one trigger and its limits, the sidecar's
//! pipes (a fake sidecar reads them), and what never leaves the daemon.

use super::remote::{remote_router, ROUTES, REMOTE_PAGE};
use super::remote_tests::{call, hook, leader, on, pair, send, Capture, TN};
use crate::app::App;
use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum::Router;
use sbxw_core::plans::{PlanThread, ThreadState, Variant};
use sbxw_core::remote_access::journal::Kind;
use sbxw_core::remote_access::push::Vapid;
use sbxw_core::remote_access::webauthn::{b64url_decode, b64url_encode};
use sbxw_core::remote_access::COOKIE;
use sbxw_core::SandboxRef;
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};

const EP: &str = "https://web.push.apple.com/QGx-phone_1.a";

/// A phone's P-256 public key (any point will do: a VAPID key's).
fn phone_key(seed: u8) -> String {
    Vapid::from_b64(&b64url_encode(&[seed; 32])).unwrap().public_b64()
}

fn auth() -> String {
    b64url_encode(&[9u8; 16])
}

fn sub_body(endpoint: &str) -> Value {
    json!({ "endpoint": endpoint, "p256dh": phone_key(7), "auth": auth() })
}

async fn device_id(router: &Router, token: &str) -> String {
    call(router, "GET", "/r/me", Some(token), json!({})).await.json()["device"]["id"].as_str().unwrap().to_string()
}

fn push_file(app: &App) -> String {
    std::fs::read_to_string(app.remote_dir_for_tests().join("push.json")).unwrap_or_default()
}

fn journal(app: &App) -> Vec<sbxw_core::remote_access::journal::Entry> {
    app.remote_journal(1000)
}

/// The sidecar's two pipes, from the test's side.
struct Fake {
    jobs: tokio::io::BufReader<tokio::io::DuplexStream>,
    answers: Option<tokio::io::DuplexStream>,
}

impl Fake {
    async fn line(&mut self, wait_ms: u64) -> Option<String> {
        let mut l = String::new();
        match tokio::time::timeout(Duration::from_millis(wait_ms), self.jobs.read_line(&mut l)).await {
            Ok(Ok(n)) if n > 0 => Some(l),
            _ => None,
        }
    }

    async fn answer(&mut self, id: &str, status: &str) {
        let a = self.answers.as_mut().unwrap();
        a.write_all(format!("{{\"id\":\"{id}\",\"status\":\"{status}\"}}\n").as_bytes()).await.unwrap();
        a.flush().await.unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Attach a fake sidecar as if spawned with `--push`; the configuration
/// line is read and checked.
async fn attach(app: &Arc<App>) -> Fake {
    let (stdin, theirs_in) = tokio::io::duplex(1 << 16);
    let (theirs_out, stdout) = tokio::io::duplex(1 << 16);
    let config = app.remote_access.lock().unwrap().push.wanted().expect("a subscription and a key");
    app.remote_push_attach(config, stdin, stdout);
    let mut f = Fake { jobs: tokio::io::BufReader::new(theirs_in), answers: Some(theirs_out) };
    let c = f.line(1000).await.expect("the configuration line first");
    let v: Value = serde_json::from_str(&c).unwrap();
    let keys: BTreeSet<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
    assert_eq!(keys, BTreeSet::from(["vapid_private", "sub"]), "{c}");
    assert_eq!(v["sub"], "https://github.com/gallettom/sbxw", "D5: the project URL");
    let vapid = Vapid::from_b64(v["vapid_private"].as_str().unwrap()).expect("a valid scalar");
    let (key, _) = app.remote_push_key("");
    assert_eq!(vapid.public_b64(), key, "the key the page subscribed with");
    f
}

/// Paired, subscribed through the route: `(router, token, device id)`.
async fn subscribed(app: &Arc<App>, name: &str) -> (Router, String, String) {
    let router = remote_router(app.clone());
    let token = pair(app, &router, name).await;
    let r = call(&router, "POST", "/r/push/subscribe", Some(&token), sub_body(EP)).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    let id = device_id(&router, &token).await;
    (router, token, id)
}

fn ask(app: &App, sb: &SandboxRef) {
    hook(app, sb, "s1", json!({"hook_event_name": "PreToolUse", "tool_name": "AskUserQuestion",
        "tool_input": {"questions": [{"question": "Which way?", "options": [{"label": "Left"}, {"label": "Right"}]}]}}));
}

fn answered(app: &App, sb: &SandboxRef) {
    hook(app, sb, "s1", json!({"hook_event_name": "PostToolUse", "tool_name": "AskUserQuestion"}));
    hook(app, sb, "s1", json!({"hook_event_name": "Stop"}));
}

fn propose(app: &App, sandbox: &str) -> String {
    let id = sbxw_core::ids::plan_id();
    let t = PlanThread { name: "alpha".into(), goal: "secret goal".into(), scope: "crates/x".into(), repo: None, model: None, state: ThreadState::Starting, summary: String::new(), updated_ms: 0, set_by: None, ticket: None };
    app.plans.lock().unwrap().propose(id.clone(), sandbox, "Secret plan title", vec![t], Variant::Split, "why", Some("lead"), 1).unwrap();
    id
}

fn job(line: &str) -> Value {
    let v: Value = serde_json::from_str(line).unwrap();
    let keys: BTreeSet<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
    assert_eq!(keys, BTreeSet::from(["id", "endpoint", "p256dh", "auth", "kind", "badge", "ttl"]), "exactly the contract's fields: {line}");
    v
}

// ── The routes ───────────────────────────────────────────────────────────

/// Every path the remote router held before Web Push. The router now holds
/// exactly these plus the four of API.md 2.8.
const BEFORE: &[&str] = &[
    "/", "/index.html", "/remote-core.js", "/remote.js", "/remote.css", "/manifest.webmanifest", "/icon-192.png", "/icon-512.png", "/apple-touch-icon.png",
    "/r/pair/start", "/r/pair/status", "/r/me", "/r/logout", "/r/leaders", "/r/leaders/:sandbox/history", "/r/events", "/r/leaders/:sandbox/message",
    "/r/leaders/:sandbox/answer", "/r/leaders/:sandbox/interrupt", "/r/voice/transcribe", "/r/leaders/:sandbox/upload", "/r/leaders/:sandbox/upload/:id",
    "/r/leaders/:sandbox/kanban", "/r/leaders/:sandbox/kanban/:ticket", "/r/approvals", "/r/passkey/start", "/r/passkey/status", "/r/passkey/options",
    "/r/passkey/register", "/r/plans", "/r/plans/:id", "/r/plans/:id/approve", "/r/plans/:id/refuse", "/r/merges", "/r/merges/:id", "/r/merges/:id/merge",
    "/r/network", "/r/network/:id", "/r/network/:id/respond", "/r/stopped", "/r/stopped/:sandbox", "/r/stopped/:sandbox/remove", "/r/approvals/challenge",
];

#[test]
fn the_route_list_grew_by_exactly_the_four_push_paths() {
    let now: BTreeSet<&str> = ROUTES.iter().map(|(_, p)| *p).chain(REMOTE_PAGE.iter().map(|(p, _, _)| *p)).collect();
    let before: BTreeSet<&str> = BEFORE.iter().copied().collect();
    let added: BTreeSet<&str> = now.difference(&before).copied().collect();
    // Plus the model and lead routes (API.md 2.9), added after Web Push.
    assert_eq!(added, BTreeSet::from(["/sw.js", "/r/push/key", "/r/push/subscribe", "/r/push/unsubscribe", "/r/models", "/r/leaders/:sandbox/model", "/r/leaders/:sandbox/lead"]));
    assert!(before.is_subset(&now), "nothing removed");
    let methods: Vec<_> = ROUTES.iter().filter(|(_, p)| p.starts_with("/r/push/")).copied().collect();
    assert_eq!(methods, [("GET", "/r/push/key"), ("POST", "/r/push/subscribe"), ("POST", "/r/push/unsubscribe")]);
}

#[tokio::test]
async fn the_push_routes_need_the_cookie_the_origin_and_remote_access_on() {
    let (_d, app) = on();
    let router = remote_router(app.clone());
    let token = pair(&app, &router, "phone").await;
    let routes = [("GET", "/r/push/key", json!({})), ("POST", "/r/push/subscribe", sub_body(EP)), ("POST", "/r/push/unsubscribe", json!({}))];
    for (m, p, body) in &routes {
        // No cookie, the UI token, a made-up cookie: not paired.
        assert_eq!(call(&router, m, p, None, body.clone()).await.status, StatusCode::UNAUTHORIZED, "{m} {p}");
        assert_eq!(call(&router, m, p, Some(&app.ui_token), body.clone()).await.status, StatusCode::UNAUTHORIZED, "{m} {p} with the UI token");
        // Another origin, or none on a write: refused before the cookie.
        for origin in [Some("https://evil.example"), Some("null"), None] {
            let mut b = Request::builder().method(*m).uri(*p).header(header::HOST, TN).header(header::CONTENT_TYPE, "application/json").header(header::COOKIE, format!("{COOKIE}={token}"));
            if let Some(o) = origin {
                b = b.header(header::ORIGIN, o);
            }
            let r = send(&router, b.body(Body::from(body.to_string())).unwrap()).await;
            let want = if *m == "GET" && origin.is_none() { StatusCode::OK } else { StatusCode::FORBIDDEN };
            assert_eq!(r.status, want, "{m} {p} origin {origin:?}");
        }
        // A wrong Host.
        let b = Request::builder().method(*m).uri(*p).header(header::HOST, "evil.example").header(header::ORIGIN, "https://evil.example").header(header::COOKIE, format!("{COOKIE}={token}"));
        assert_eq!(send(&router, b.body(Body::from(body.to_string())).unwrap()).await.status, StatusCode::FORBIDDEN);
        // With both: answered.
        assert_eq!(call(&router, m, p, Some(&token), body.clone()).await.status, StatusCode::OK, "{m} {p}");
    }
    // Remote access off: not found, whatever is presented.
    app.remote_disable().await;
    for (m, p, body) in &routes {
        assert_eq!(call(&router, m, p, Some(&token), body.clone()).await.status, StatusCode::NOT_FOUND, "{m} {p} with remote off");
    }
    assert_eq!(call(&router, "GET", "/sw.js", None, json!({})).await.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn sw_js_is_served_from_the_root_as_javascript_never_cached() {
    let (_d, app) = on();
    let router = remote_router(app.clone());
    let r = call(&router, "GET", "/sw.js", None, json!({})).await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.headers[header::CONTENT_TYPE], "text/javascript");
    assert_eq!(r.headers[header::CACHE_CONTROL], "no-store");
    assert_eq!(r.headers[header::CONTENT_SECURITY_POLICY].to_str().unwrap().split(';').next(), Some("default-src 'self'"));
    assert!(r.headers.get("service-worker-allowed").is_none());
    let file = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/remote/sw.js");
    match std::fs::read(&file) {
        Ok(bytes) => assert_eq!(r.body, bytes, "the file's own bytes"),
        Err(_) => assert_eq!(r.text(), "// sbxw: the service worker is not built into this daemon.\n", "the placeholder"),
    }
    // Only GET (and HEAD).
    assert_eq!(call(&router, "POST", "/sw.js", None, json!({})).await.status, StatusCode::METHOD_NOT_ALLOWED);
}

#[tokio::test]
async fn the_key_is_made_once_kept_private_and_on_says_this_device() {
    let (d, app) = on();
    let router = remote_router(app.clone());
    let a = pair(&app, &router, "a").await;
    let b = pair(&app, &router, "b").await;
    let k1 = call(&router, "GET", "/r/push/key", Some(&a), json!({})).await.json();
    assert_eq!(k1["on"], false);
    let key = k1["key"].as_str().unwrap().to_string();
    let raw = b64url_decode(&key, 65).unwrap();
    assert_eq!((raw.len(), raw[0]), (65, 0x04), "an uncompressed P-256 point");
    assert!(sbxw_core::remote_access::push::Subscription::parse(EP, &key, &auth(), 0).is_ok(), "a point of P-256");
    assert_eq!(call(&router, "GET", "/r/push/key", Some(&b), json!({})).await.json()["key"], key.as_str(), "one key per Mac");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let m = std::fs::metadata(app.remote_dir_for_tests().join("push.json")).unwrap().permissions().mode();
        assert_eq!(m & 0o777, 0o600);
    }
    assert_eq!(call(&router, "POST", "/r/push/subscribe", Some(&a), sub_body(EP)).await.json(), json!({ "ok": true }));
    assert_eq!(call(&router, "GET", "/r/push/key", Some(&a), json!({})).await.json()["on"], true);
    assert_eq!(call(&router, "GET", "/r/push/key", Some(&b), json!({})).await.json()["on"], false, "on is per device");
    // A restart keeps the key and the subscription.
    let again = crate::testing::app_in(d.path());
    let (k2, _) = again.remote_push_key("x");
    assert_eq!(k2, key);
    assert_eq!(again.remote_push_devices(), 1);
    assert!(!format!("{:?}", again.remote_status()).contains("push.apple.com"));
}

#[tokio::test]
async fn a_forged_subscription_is_refused_with_a_reason_word_and_never_stored() {
    let (_d, app) = on();
    let router = remote_router(app.clone());
    let token = pair(&app, &router, "phone").await;
    let key = phone_key(7);
    let mut off = b64url_decode(&key, 65).unwrap();
    off[64] ^= 1;
    let cases = [
        (json!({ "endpoint": "https://evil.example/x", "p256dh": key, "auth": auth() }), "endpoint"),
        (json!({ "endpoint": "http://web.push.apple.com/x", "p256dh": key, "auth": auth() }), "endpoint"),
        (json!({ "endpoint": "https://web.push.apple.com:8443/x", "p256dh": key, "auth": auth() }), "endpoint"),
        (json!({ "endpoint": "https://17.253.1.1/x", "p256dh": key, "auth": auth() }), "endpoint"),
        (json!({ "endpoint": "https://u@web.push.apple.com/x", "p256dh": key, "auth": auth() }), "endpoint"),
        (json!({ "endpoint": "https://web.push.apple.com/x?y", "p256dh": key, "auth": auth() }), "endpoint"),
        (json!({ "endpoint": format!("https://web.push.apple.com/{}", "a".repeat(600)), "p256dh": key, "auth": auth() }), "endpoint"),
        (json!({ "endpoint": EP, "p256dh": b64url_encode(&off), "auth": auth() }), "p256dh"),
        (json!({ "endpoint": EP, "p256dh": b64url_encode(&[4u8; 65]), "auth": auth() }), "p256dh"),
        (json!({ "endpoint": EP, "p256dh": key, "auth": b64url_encode(&[1u8; 15]) }), "auth"),
        (json!({ "endpoint": EP, "p256dh": key, "auth": "not base64url!" }), "auth"),
    ];
    for (body, word) in cases {
        let r = call(&router, "POST", "/r/push/subscribe", Some(&token), body.clone()).await;
        assert_eq!((r.status, r.json()["reason"].as_str()), (StatusCode::BAD_REQUEST, Some(word)), "{body}");
        assert!(!r.text().contains("evil") && !r.text().contains("apple"), "the answer never echoes the endpoint: {}", r.text());
    }
    // Extra fields, missing fields: refused by the body's shape.
    let mut extra = sub_body(EP);
    extra["text"] = "x".into();
    assert!(call(&router, "POST", "/r/push/subscribe", Some(&token), extra).await.status.is_client_error());
    assert!(call(&router, "POST", "/r/push/subscribe", Some(&token), json!({ "endpoint": EP })).await.status.is_client_error());
    assert_eq!(app.remote_push_devices(), 0);
    assert!(!push_file(&app).contains("subscriptions\": {\n"), "nothing stored: {}", push_file(&app));
    assert!(!journal(&app).iter().any(|e| e.kind == Kind::PushOn));
}

// ── Deletion on each exit ────────────────────────────────────────────────

#[tokio::test]
async fn a_subscription_goes_on_every_exit_and_stays_when_remote_access_is_turned_off() {
    let (_d, app) = on();
    let ui = crate::testing::signed_in(app.clone());
    let ui_call = |m: &str, p: String| Request::builder().method(m).uri(p).header(header::HOST, "127.0.0.1:7681").body(Body::empty()).unwrap();

    // Unsubscribe from the phone (twice: idempotent).
    let (router, token, id) = subscribed(&app, "unsub").await;
    assert!(push_file(&app).contains(&id));
    assert_eq!(call(&router, "POST", "/r/push/unsubscribe", Some(&token), json!({})).await.json(), json!({ "ok": true }));
    assert_eq!(call(&router, "POST", "/r/push/unsubscribe", Some(&token), json!({})).await.json(), json!({ "ok": true }));
    assert!(!push_file(&app).contains(&id));
    assert_eq!(journal(&app).iter().filter(|e| e.kind == Kind::PushOff && e.device_id == id).count(), 1, "one push_off");

    // The Mac turns it off (the device stays paired).
    let (router, token, id) = subscribed(&app, "mac").await;
    let devs = send(&ui, ui_call("GET", "/api/remote/devices".into())).await.json();
    assert_eq!(devs.as_array().unwrap().iter().find(|d| d["id"] == id.as_str()).unwrap()["notifications"], true);
    assert_eq!(send(&ui, ui_call("DELETE", format!("/api/remote/devices/{id}/push"))).await.status, StatusCode::OK);
    assert!(!push_file(&app).contains(&id));
    assert_eq!(call(&router, "GET", "/r/me", Some(&token), json!({})).await.status, StatusCode::OK, "still paired");
    assert_eq!(send(&ui, ui_call("DELETE", "/api/remote/devices/nope/push".into())).await.status, StatusCode::NOT_FOUND);
    // The admin route is not on the remote listener.
    assert_eq!(call(&router, "DELETE", &format!("/api/remote/devices/{id}/push"), Some(&token), json!({})).await.status, StatusCode::NOT_FOUND);

    // Revoked on the Mac.
    let (_r, _t, id) = subscribed(&app, "revoked").await;
    assert!(app.remote_revoke(&id, Kind::Revoked));
    assert!(!push_file(&app).contains(&id));

    // Forget this device (logout) on the phone.
    let (router, token, id) = subscribed(&app, "logout").await;
    assert_eq!(call(&router, "POST", "/r/logout", Some(&token), json!({})).await.status, StatusCode::OK);
    assert!(!push_file(&app).contains(&id));

    // 30 days idle.
    let (_r, _t, id) = subscribed(&app, "idle").await;
    app.remote_expire_for_tests(crate::app::now_ms() + sbxw_core::remote_access::DEVICE_IDLE_MS + 60_000);
    assert!(!push_file(&app).contains(&id));
    assert_eq!(app.remote_push_devices(), 0);

    // Turning remote access off keeps it.
    let (_r, _t, id) = subscribed(&app, "kept").await;
    app.remote_disable().await;
    assert!(push_file(&app).contains(&id));
    assert_eq!(app.remote_push_devices(), 1);
    assert_eq!(app.remote_status().push_devices, 1);
}

#[tokio::test]
async fn a_subscription_whose_device_is_gone_is_dropped_at_load() {
    let (d, app) = on();
    let (_r, _t, id) = subscribed(&app, "phone").await;
    let dir = app.remote_dir_for_tests();
    // The device record vanishes behind the daemon's back.
    let state = std::fs::read_to_string(dir.join("state.json")).unwrap();
    let mut v: Value = serde_json::from_str(&state).unwrap();
    v["devices"] = json!([]);
    std::fs::write(dir.join("state.json"), v.to_string()).unwrap();
    drop(app);
    let again = crate::testing::app_in(d.path());
    assert_eq!(again.remote_push_devices(), 0);
    assert!(!push_file(&again).contains(&id));
}

// ── The trigger ──────────────────────────────────────────────────────────

#[tokio::test]
async fn a_session_starting_to_ask_pushes_one_n_job_with_the_badge_and_nothing_else() {
    let logs = Capture::default();
    let _guard = tracing::subscriber::set_default(logs.clone());
    let (d, app) = on();
    let (_router, _token, id) = subscribed(&app, "Marie iPhone").await;
    let mut fake = attach(&app).await;
    let (sb, _) = leader(&app, d.path(), "lead1").await;
    app.remote_push_step(); // the baseline
    assert!(fake.line(200).await.is_none(), "the baseline pushes nothing");
    ask(&app, &sb);
    assert!(app.remote_leaders()[0].asks, "the needs_you test holds");
    app.remote_push_step();
    let line = fake.line(1000).await.expect("a job");
    let j = job(&line);
    assert_eq!((j["kind"].as_str(), j["badge"].as_u64(), j["ttl"].as_u64()), (Some("n"), Some(1), Some(3600)));
    assert_eq!(j["endpoint"], EP);
    assert!(!line.contains("lead1") && !line.contains("Which way") && !line.contains("Marie"), "no sandbox, text or device name: {line}");
    // The same state again: nothing.
    app.remote_push_step();
    assert!(fake.line(200).await.is_none(), "no push on a repeat");
    // The answer.
    fake.answer(j["id"].as_str().unwrap(), "201").await;
    let sent: Vec<_> = journal(&app).into_iter().filter(|e| e.kind == Kind::PushSent).collect();
    assert_eq!(sent.len(), 1);
    assert_eq!((sent[0].device_id.as_str(), sent[0].target.as_deref()), (id.as_str(), Some("n")));
    // Answered, then asking again within 2 minutes: the limit holds.
    answered(&app, &sb);
    app.remote_push_step();
    ask(&app, &sb);
    app.remote_push_step();
    assert!(fake.line(300).await.is_none(), "one push per kind per 2 minutes");
    // Nothing of the subscription in the logs or the journal.
    let logged = logs.0.lock().unwrap().clone();
    let journal_text = std::fs::read_to_string(app.remote_dir_for_tests().join("journal.jsonl")).unwrap();
    for secret in [EP, "push.apple.com", &phone_key(7), &auth(), "vapid_private"] {
        assert!(!logged.contains(secret), "{secret} in the logs:\n{logged}");
        assert!(!journal_text.contains(secret), "{secret} in the journal");
    }
    let (vapid_pub, _) = app.remote_push_key("");
    assert!(!logged.contains(&vapid_pub));
}

#[tokio::test]
async fn more_plans_waiting_pushes_p_and_the_badge_counts_plans_only_with_level_2_on() {
    let (_d, app) = on();
    let (_router, _token, _id) = subscribed(&app, "phone").await;
    let mut fake = attach(&app).await;
    app.remote_push_step();
    propose(&app, "lead1");
    app.remote_push_step();
    let line = fake.line(1000).await.expect("a p job");
    let j = job(&line);
    assert_eq!((j["kind"].as_str(), j["badge"].as_u64()), (Some("p"), Some(0)), "level 2 off: no plan count leaks through the badge");
    assert!(!line.contains("Secret") && !line.contains("secret goal") && !line.contains("lead1"), "{line}");

    // Level 2 on, a second phone: the badge counts the plans.
    app.remote_approvals_enable().unwrap();
    let (_r2, _t2, id2) = subscribed(&app, "second").await;
    let _ = id2;
    // Its own limits: the first phone already had its `p` in this window.
    propose(&app, "lead2");
    app.remote_push_step();
    let line = fake.line(1000).await.expect("the second phone's job");
    let j = job(&line);
    assert_eq!((j["kind"].as_str(), j["badge"].as_u64()), (Some("p"), Some(2)));
    assert!(fake.line(200).await.is_none(), "the first phone is inside its 2 minutes");
    // Fewer plans: no push.
    for p in app.plans_list() {
        let _ = app.plans.lock().unwrap().refuse(&p.id, None, 2);
    }
    app.remote_push_step();
    assert!(fake.line(200).await.is_none());
}

#[tokio::test]
async fn no_push_to_a_device_with_a_stream_open_while_remote_is_off_or_with_no_subscription() {
    let (d, app) = on();
    let (_router, _token, id) = subscribed(&app, "phone").await;
    let mut fake = attach(&app).await;
    let (sb, _) = leader(&app, d.path(), "lead1").await;
    app.remote_push_step();
    // An event stream open: it has the toast.
    let slot = app.remote_stream_slot(&id);
    ask(&app, &sb);
    app.remote_push_step();
    assert!(fake.line(300).await.is_none(), "a device with a stream open gets no push");
    drop(slot);
    answered(&app, &sb);
    app.remote_push_step();
    // No subscription.
    app.remote_push_off(&id);
    ask(&app, &sb);
    app.remote_push_step();
    assert!(fake.line(300).await.is_none(), "no subscription, no push");
    // Remote off (the pipe goes with the sidecar; edges still move).
    let (_r2, _t2, _id2) = subscribed(&app, "again").await;
    let mut fake = attach(&app).await;
    answered(&app, &sb);
    app.remote_push_step();
    app.remote_disable().await;
    ask(&app, &sb);
    app.remote_push_step();
    assert!(fake.line(300).await.is_none(), "remote off: nothing sent");
    assert!(app.remote_access.lock().unwrap().push.wanted().is_some(), "the subscription is kept");
}

#[tokio::test]
async fn a_thread_a_background_session_or_jarvis_asking_never_pushes() {
    let (_d, app) = on();
    let (_router, _token, _id) = subscribed(&app, "phone").await;
    let mut fake = attach(&app).await;
    app.remote_push_step();
    for (name, extra) in [("thread1", json!({"sbxw_thread": "worker"})), ("bg1", json!({"session_kind": "bg"})), (crate::svc::jarvis_sandbox::NAME, json!({}))] {
        let sb = SandboxRef::parse(name).unwrap();
        let mut start = json!({"hook_event_name": "SessionStart"});
        let mut q = json!({"hook_event_name": "PreToolUse", "tool_name": "AskUserQuestion", "tool_input": {"questions": [{"question": "?", "options": [{"label": "a"}, {"label": "b"}]}]}});
        for (k, v) in extra.as_object().unwrap() {
            start[k] = v.clone();
            q[k] = v.clone();
        }
        hook(&app, &sb, "t1", start);
        hook(&app, &sb, "t1", q);
        app.remote_push_step();
        assert!(fake.line(200).await.is_none(), "{name} asking pushed");
    }
}

#[tokio::test]
async fn no_route_causes_a_push() {
    let (d, app) = on();
    let (router, token, _id) = subscribed(&app, "phone").await;
    let mut fake = attach(&app).await;
    let _ = leader(&app, d.path(), "myapp").await;
    app.remote_push_step();
    for (m, p) in ROUTES {
        if *p == "/r/logout" || p.starts_with("/r/push/") {
            continue;
        }
        let concrete = p.replace(":sandbox", "myapp").replace(":id", "p-0123456789abcdef0123456789abcdef").replace(":ticket", "t1");
        let _ = call(&router, m, &concrete, Some(&token), json!({})).await;
        app.remote_push_step();
    }
    assert!(fake.line(300).await.is_none(), "a route pushed");
}

// ── The answers and the pipe ─────────────────────────────────────────────

#[tokio::test]
async fn each_answer_is_handled_and_a_closed_pipe_stops_pushing_with_one_journal_line() {
    let (d, app) = on();
    let (_r, _t, id) = subscribed(&app, "phone").await;
    let mut fake = attach(&app).await;
    let (sb, _) = leader(&app, d.path(), "lead1").await;
    app.remote_push_step();

    // 500: failed, class only.
    ask(&app, &sb);
    app.remote_push_step();
    let j = job(&fake.line(1000).await.unwrap());
    fake.answer(j["id"].as_str().unwrap(), "500").await;
    let last = journal(&app).pop().unwrap();
    assert_eq!((last.kind, last.target.as_deref()), (Kind::PushFailed, Some("5xx")));

    // 429 on a `p`: the device is paused, a later `n` does not go.
    propose(&app, "lead1");
    app.remote_push_step();
    let j = job(&fake.line(1000).await.unwrap());
    assert_eq!(j["kind"], "p");
    fake.answer(j["id"].as_str().unwrap(), "429").await;
    answered(&app, &sb);
    app.remote_push_step();
    ask(&app, &sb);
    app.remote_push_step();
    assert!(fake.line(300).await.is_none(), "paused after 429");

    // An answer for no job, or garbage: ignored.
    fake.answer("j999", "201").await;
    fake.answers.as_mut().unwrap().write_all(b"not json\n").await.unwrap();
    assert!(!journal(&app).iter().any(|e| e.kind == Kind::PushSent));

    // 410: gone, deleted.
    let (_r2, _t2, id2) = subscribed(&app, "second").await;
    answered(&app, &sb);
    app.remote_push_step();
    ask(&app, &sb);
    app.remote_push_step();
    let j = job(&fake.line(1000).await.unwrap());
    fake.answer(j["id"].as_str().unwrap(), "410").await;
    assert!(!push_file(&app).contains(&id2), "410 deletes");
    assert!(journal(&app).iter().any(|e| e.kind == Kind::PushGone && e.device_id == id2));
    assert!(push_file(&app).contains(&id), "the other device keeps its own");

    // The sidecar dies: the answers pipe closes. One journal line, then no push.
    drop(fake.answers.take());
    tokio::time::sleep(Duration::from_millis(200)).await;
    let closed: Vec<_> = journal(&app).into_iter().filter(|e| e.kind == Kind::PushFailed && e.target.as_deref() == Some("sidecar")).collect();
    assert_eq!(closed.len(), 1);
    assert_eq!(closed[0].device_id, "");
    assert!(app.remote_access.lock().unwrap().push.wanted().is_some(), "subscriptions kept");
    assert!(app.remote_enabled(), "remote access still on");
}

#[tokio::test]
async fn refused_by_the_sidecar_deletes_the_subscription() {
    let (d, app) = on();
    let (_r, _t, id) = subscribed(&app, "phone").await;
    let mut fake = attach(&app).await;
    let (sb, _) = leader(&app, d.path(), "lead1").await;
    app.remote_push_step();
    ask(&app, &sb);
    app.remote_push_step();
    let j = job(&fake.line(1000).await.unwrap());
    fake.answer(j["id"].as_str().unwrap(), "refused").await;
    assert!(!push_file(&app).contains(&id));
}

#[tokio::test]
async fn the_task_watches_the_bus_with_no_stream_open() {
    let (d, app) = on();
    let (_r, _t, _id) = subscribed(&app, "phone").await;
    let mut fake = attach(&app).await;
    let (sb, _) = leader(&app, d.path(), "lead1").await;
    app.remote_push_task();
    app.remote_push_task(); // once per daemon
    tokio::time::sleep(Duration::from_millis(100)).await;
    ask(&app, &sb);
    let j = job(&fake.line(2000).await.expect("pushed from the bus event"));
    assert_eq!(j["kind"], "n");
    assert!(fake.line(300).await.is_none(), "one task: one job");
}

// ── A fake sidecar program ───────────────────────────────────────────────

/// The real spawn path: a fake `sbxw-tailnet` that records its argv, reads
/// the configuration line and the jobs from stdin and answers `201`.
#[tokio::test]
async fn a_fake_sidecar_gets_push_on_the_first_subscription_restarted_once() {
    let (d, app) = crate::testing::app();
    let dir = d.path().join("fake");
    std::fs::create_dir_all(&dir).unwrap();
    let fake = d.path().join("fake-tailnet");
    let script = format!(
        "#!/bin/sh\nd='{dir}'\nprintf '%s\\n' \"$*\" >> \"$d/starts\"\ncase \"$*\" in *--push*) ;; *) exec sleep 30;; esac\n\
         IFS= read -r cfg\nprintf '%s\\n' \"$cfg\" > \"$d/config\"\n\
         while IFS= read -r l; do\n  printf '%s\\n' \"$l\" >> \"$d/jobs\"\n  id=$(printf '%s' \"$l\" | sed 's/^{{\"id\":\"\\([^\"]*\\)\".*/\\1/')\n  printf '{{\"id\":\"%s\",\"status\":\"201\"}}\\n' \"$id\"\ndone\nsleep 30\n",
        dir = dir.display()
    );
    std::fs::write(&fake, script).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    {
        let mut rt = app.remote_access.lock().unwrap();
        rt.addr_override = Some("127.0.0.1:0".into());
        rt.sidecar_program = crate::svc::remote_access::Sidecar::Program(fake.clone());
        rt.tailnet_override = Some(TN.into());
        rt.push.restart_delay = Duration::from_millis(100);
    }
    app.remote_enable().await.unwrap();
    let wait_for = |f: &'static str, want: usize| {
        let p = dir.join(f);
        async move {
            for _ in 0..80 {
                let t = std::fs::read_to_string(&p).unwrap_or_default();
                if t.lines().count() >= want {
                    return t;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            std::fs::read_to_string(&p).unwrap_or_default()
        }
    };
    let starts = wait_for("starts", 1).await;
    assert!(!starts.contains("--push"), "no subscription: started without --push: {starts}");

    // Two phones subscribe at once: ONE restart, with --push last.
    let router = remote_router(app.clone());
    let a = pair(&app, &router, "a").await;
    let b = pair(&app, &router, "b").await;
    assert_eq!(call(&router, "POST", "/r/push/subscribe", Some(&a), sub_body(EP)).await.status, StatusCode::OK);
    assert_eq!(call(&router, "POST", "/r/push/subscribe", Some(&b), sub_body("https://web.push.apple.com/other")).await.status, StatusCode::OK);
    let starts = wait_for("starts", 2).await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    let starts = std::fs::read_to_string(dir.join("starts")).unwrap_or(starts);
    assert_eq!(starts.lines().count(), 2, "restarted once, not per subscription: {starts}");
    assert!(starts.lines().nth(1).unwrap().ends_with(" --push"), "{starts}");
    assert_eq!(app.remote_access.lock().unwrap().push.restarts, 1);
    let config = wait_for("config", 1).await;
    assert_eq!(config, app.remote_access.lock().unwrap().push.wanted().unwrap(), "the configuration line, first");

    // A third subscription with the pipe up: no restart.
    let c = pair(&app, &router, "c").await;
    assert_eq!(call(&router, "POST", "/r/push/subscribe", Some(&c), sub_body("https://web.push.apple.com/third")).await.status, StatusCode::OK);

    // A session asks: one job per phone, through the real pipe, answered.
    let (sb, _) = leader(&app, d.path(), "lead1").await;
    app.remote_push_step();
    ask(&app, &sb);
    app.remote_push_step();
    let jobs = wait_for("jobs", 3).await;
    assert_eq!(jobs.lines().count(), 3, "{jobs}");
    for l in jobs.lines() {
        let j = job(l);
        assert_eq!(j["kind"], "n");
        assert!(!l.contains("lead1"));
    }
    for _ in 0..30 {
        if journal(&app).iter().filter(|e| e.kind == Kind::PushSent).count() == 3 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(journal(&app).iter().filter(|e| e.kind == Kind::PushSent).count(), 3);
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(std::fs::read_to_string(dir.join("starts")).unwrap().lines().count(), 2, "no further restart");

    // Off: the sidecar stops quietly (no `sidecar` failure journaled), the
    // subscriptions stay; the next start is with --push at once.
    app.remote_disable().await;
    assert!(!journal(&app).iter().any(|e| e.kind == Kind::PushFailed));
    app.remote_enable().await.unwrap();
    let starts = wait_for("starts", 3).await;
    assert!(starts.lines().nth(2).unwrap().ends_with(" --push"), "{starts}");
    app.remote_disable().await;
}
