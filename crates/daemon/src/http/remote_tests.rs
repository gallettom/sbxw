//! The remote listener, end to end through its router: the route walk (Q20),
//! pairing, the device token, the guards, what a phone sees and does, the
//! journal and the logs. Every test runs in a temporary sbxw home.

use super::remote::{remote_router, ROUTES, REMOTE_PAGE};
use crate::app::App;
use crate::testing::{app, app_in};
use axum::body::Body;
use axum::http::{header, HeaderMap, Method, Request, StatusCode};
use axum::Router;
use futures_util::StreamExt;
use sbxw_core::remote_access::journal::Kind;
use sbxw_core::remote_access::COOKIE;
use sbxw_core::session::SessionKey;
use sbxw_core::SandboxRef;
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
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

/// A request as the sidecar forwards it: Host the tailnet name, the page's
/// Origin on writes, JSON bodies.
fn req(method: &str, path: &str) -> axum::http::request::Builder {
    let m: Method = method.parse().unwrap();
    let mut b = Request::builder().method(m.clone()).uri(path).header(header::HOST, TN);
    if m != Method::GET && m != Method::HEAD {
        b = b.header(header::ORIGIN, format!("https://{TN}")).header(header::CONTENT_TYPE, "application/json");
    }
    b
}

async fn send(router: &Router, r: Request<Body>) -> Reply {
    let res = router.clone().oneshot(r).await.unwrap();
    let status = res.status();
    let headers = res.headers().clone();
    // A stream (SSE) never ends: its first seconds are enough to judge.
    let body = match tokio::time::timeout(Duration::from_secs(3), axum::body::to_bytes(res.into_body(), 16 << 20)).await {
        Ok(b) => b.unwrap().to_vec(),
        Err(_) => b"(still streaming)".to_vec(),
    };
    Reply { status, headers, body }
}

async fn call(router: &Router, method: &str, path: &str, cookie: Option<&str>, body: Value) -> Reply {
    let mut b = req(method, path);
    if let Some(t) = cookie {
        b = b.header(header::COOKIE, format!("{COOKIE}={t}"));
    }
    let body = if method == "GET" || method == "HEAD" { Body::empty() } else { Body::from(body.to_string()) };
    send(router, b.body(body).unwrap()).await
}

/// Pair a phone through the routes; its token, from the cookie.
async fn pair(app: &Arc<App>, router: &Router, name: &str) -> String {
    let code = app.remote_pairing_code().unwrap().code;
    let r = call(router, "POST", "/r/pair/start", None, json!({ "code": code, "device_name": name })).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    let id = r.json()["request_id"].as_str().unwrap().to_string();
    assert!(app.remote_pairing_approve(&id));
    let r = call(router, "GET", &format!("/r/pair/status?request_id={id}"), None, json!({})).await;
    assert_eq!(r.json()["state"], "paired", "{}", r.text());
    let cookie = r.headers[header::SET_COOKIE].to_str().unwrap().to_string();
    for part in ["HttpOnly", "Secure", "SameSite=Strict", "Path=/", "Max-Age=2592000"] {
        assert!(cookie.contains(part), "{part} in {cookie}");
    }
    assert!(cookie.starts_with(&format!("{COOKIE}=")));
    cookie[COOKIE.len() + 1..].split(';').next().unwrap().to_string()
}

/// The paths a router holds, read from the router itself: axum 0.7's Debug
/// lists them (`RouteId(n): "/path"`). If a future axum prints them another
/// way, the counts below fail loudly rather than the walk passing empty.
fn router_paths(r: &Router) -> BTreeSet<String> {
    let dbg = format!("{r:?}");
    let mut out = BTreeSet::new();
    for chunk in dbg.split("RouteId(").skip(1) {
        if let Some(rest) = chunk.split_once("): \"").map(|(_, r)| r) {
            if let Some((path, _)) = rest.split_once('"') {
                if !path.contains("__private__axum_fallback") {
                    out.insert(path.to_string());
                }
            }
        }
    }
    out
}

fn concrete(path: &str) -> String {
    path.split('/')
        .map(|s| if s.starts_with(':') { "myapp" } else if s.starts_with('*') { "a/b" } else { s })
        .collect::<Vec<_>>()
        .join("/")
}

fn remote_paths() -> BTreeSet<String> {
    ROUTES.iter().map(|(_, p)| p.to_string()).chain(REMOTE_PAGE.iter().map(|(p, _, _)| p.to_string())).collect()
}

// ── 1. The route walk (Q20) ──────────────────────────────────────────────

#[tokio::test]
async fn the_remote_router_holds_exactly_its_list() {
    let (_d, app) = on();
    let got = router_paths(&remote_router(app));
    assert_eq!(got, remote_paths(), "a route was added to or removed from the remote router without updating ROUTES");
}

#[tokio::test]
async fn every_ui_route_is_absent_from_the_remote_listener() {
    let (_d, app) = on();
    let ui = router_paths(&crate::http::ui_router(app.clone()));
    assert!(ui.len() > 150, "the walk must see the whole UI router, saw {}", ui.len());
    for (_, p) in super::remote_admin::ROUTES {
        assert!(ui.contains(*p), "admin route {p} is on the UI router");
    }
    let ours = remote_paths();
    let router = remote_router(app.clone());
    let token = pair(&app, &router, "phone").await;
    let mut walked = 0;
    for path in &ui {
        let p = concrete(path);
        for method in ["GET", "POST", "PUT", "DELETE", "PATCH"] {
            let r = call(&router, method, &p, Some(&token), json!({})).await;
            walked += 1;
            if ours.contains(path.as_str()) {
                // The same path exists on both (the page at `/`): it must be
                // the remote listener's own answer, never the UI's.
                if method == "GET" {
                    let page = REMOTE_PAGE.iter().find(|(rp, _, _)| *rp == path.as_str()).unwrap();
                    assert_eq!(r.body, page.2, "{path} answered with the UI's content");
                } else {
                    assert_eq!(r.status, StatusCode::METHOD_NOT_ALLOWED, "{method} {path}");
                }
                continue;
            }
            assert_eq!(r.status, StatusCode::NOT_FOUND, "{method} {p} (UI route {path}) answered on the remote listener: {}", r.text());
        }
    }
    assert!(walked > 750);
}

#[tokio::test]
async fn every_remote_data_route_refuses_the_ui_token_and_no_token() {
    let (_d, app) = on();
    let router = remote_router(app.clone());
    let ui = app.ui_token.clone();
    for (method, path) in ROUTES.iter().filter(|(_, p)| p.starts_with("/r/") && !p.starts_with("/r/pair/")) {
        let p = concrete(path);
        let tries: Vec<(&str, Option<String>, Option<String>)> = vec![
            ("none", None, None),
            ("bearer", Some(format!("Bearer {ui}")), None),
            ("ui cookie", None, Some(format!("sbxw_ui={ui}"))),
            ("ui token as device cookie", None, Some(format!("{COOKIE}={ui}"))),
            ("agent-style token", Some("Bearer sbxw_0000".into()), None),
        ];
        for (what, auth, cookie) in tries {
            let mut b = req(method, &p);
            if let Some(a) = auth {
                b = b.header(header::AUTHORIZATION, a);
            }
            if let Some(c) = cookie {
                b = b.header(header::COOKIE, c);
            }
            let r = send(&router, b.body(Body::from("{}")).unwrap()).await;
            assert_eq!(r.status, StatusCode::UNAUTHORIZED, "{method} {p} with {what}: {}", r.text());
        }
    }
}

#[tokio::test]
async fn the_ui_listener_refuses_a_device_token() {
    let (_d, app) = on();
    let token = pair(&app, &remote_router(app.clone()), "phone").await;
    let ui = crate::http::ui_router(app.clone());
    for path in ["/api/remote", "/api/sessions", "/api/remote/devices"] {
        for (name, value) in [(header::COOKIE, format!("{COOKIE}={token}")), (header::COOKIE, format!("sbxw_ui={token}")), (header::AUTHORIZATION, format!("Bearer {token}"))] {
            let r = Request::builder().uri(path).header(header::HOST, "127.0.0.1:7681").header(name, value).body(Body::empty()).unwrap();
            assert_eq!(send(&ui, r).await.status, StatusCode::UNAUTHORIZED, "{path}");
        }
    }
}

// ── 2. Pairing and the token ─────────────────────────────────────────────

#[tokio::test]
async fn pairing_codes_are_single_use_and_refused_alike() {
    let (_d, app) = on();
    let router = remote_router(app.clone());
    let start = |code: String, name: &'static str| {
        let router = router.clone();
        async move { call(&router, "POST", "/r/pair/start", None, json!({ "code": code, "device_name": name })).await }
    };
    let wrong = start("0".repeat(48), "x").await;
    assert_eq!(wrong.status, StatusCode::FORBIDDEN);
    let code = app.remote_pairing_code().unwrap().code;
    let first = start(code.clone(), "mine").await;
    assert_eq!(first.status, StatusCode::OK);
    let check = first.json()["check"].as_str().unwrap().to_string();
    assert_eq!(check.len(), 6);
    let replay = start(code.clone(), "theirs").await;
    assert_eq!(replay.status, StatusCode::FORBIDDEN);
    assert_eq!(replay.json(), wrong.json(), "wrong and used codes answer the same");
    let pending = app.remote_pairing_requests();
    assert_eq!(pending.len(), 1, "the racing phone made no request");
    assert_eq!(pending[0].device_name, "mine");
    assert_eq!(pending[0].check, check, "the Mac shows the phone's check");

    let id = first.json()["request_id"].as_str().unwrap().to_string();
    assert!(app.remote_pairing_refuse(&id));
    let r = call(&router, "GET", &format!("/r/pair/status?request_id={id}"), None, json!({})).await;
    assert_eq!(r.json()["state"], "refused");
    assert!(r.headers.get(header::SET_COOKIE).is_none());
    let r = call(&router, "GET", "/r/pair/status?request_id=nope", None, json!({})).await;
    assert_eq!(r.json()["state"], "expired");
    let r = call(&router, "POST", "/r/pair/start", None, json!({ "code": app.remote_pairing_code().unwrap().code, "device_name": "bad\nname" })).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    assert!(app.remote_journal(10).iter().any(|e| e.kind == Kind::Refused));
}

#[tokio::test]
async fn a_pairing_code_needs_remote_access_on_and_a_serving_tailnet() {
    let (_d, app) = app();
    assert!(app.remote_pairing_code().is_err(), "off");
    app.remote_test_on(TN);
    app.remote_access.lock().unwrap().tailnet_override = None;
    let e = app.remote_pairing_code().unwrap_err();
    assert!(e.contains("not serving"), "{e}");
}

#[tokio::test]
async fn pairing_is_rate_limited_per_source_and_in_all() {
    let (_d, app) = on();
    let router = remote_router(app.clone());
    let start = |src: Option<String>| {
        let router = router.clone();
        async move {
            let mut b = req("POST", "/r/pair/start");
            if let Some(s) = src {
                b = b.header("x-forwarded-for", s);
            }
            send(&router, b.body(Body::from(json!({"code": "x", "device_name": "p"}).to_string())).unwrap()).await.status
        }
    };
    for _ in 0..5 {
        assert_eq!(start(Some("100.64.0.1".into())).await, StatusCode::FORBIDDEN);
    }
    assert_eq!(start(Some("100.64.0.1".into())).await, StatusCode::TOO_MANY_REQUESTS, "per source");
    for _ in 0..5 {
        assert_eq!(start(None).await, StatusCode::FORBIDDEN);
    }
    assert_eq!(start(None).await, StatusCode::TOO_MANY_REQUESTS, "no header: one shared source");
    assert_eq!(start(Some("not-an-ip".into())).await, StatusCode::TOO_MANY_REQUESTS, "a malformed header is the shared source");
    // 10 hits counted so far (refusals are not counted): 10 more sources
    // reach the global cap of 20.
    for i in 2..12 {
        assert_eq!(start(Some(format!("100.64.0.{i}"))).await, StatusCode::FORBIDDEN);
    }
    let r = start(Some("100.64.1.1".into())).await;
    assert_eq!(r, StatusCode::TOO_MANY_REQUESTS, "the global cap holds whatever source a caller claims");
}

#[tokio::test]
async fn failed_tokens_are_rate_limited_and_the_global_cap_holds() {
    let (_d, app) = on();
    let router = remote_router(app.clone());
    let token = pair(&app, &router, "phone").await;
    let me = |src: &str, t: &str| {
        let router = router.clone();
        let (src, t) = (src.to_string(), t.to_string());
        async move {
            let b = req("GET", "/r/me").header("x-forwarded-for", src).header(header::COOKIE, format!("{COOKIE}={t}"));
            send(&router, b.body(Body::empty()).unwrap()).await.status
        }
    };
    for _ in 0..10 {
        assert_eq!(me("100.64.0.1", "sbxw_dev_wrong").await, StatusCode::UNAUTHORIZED);
    }
    assert_eq!(me("100.64.0.1", &token).await, StatusCode::TOO_MANY_REQUESTS, "the guessing source is held back, even with a good token");
    assert_eq!(me("100.64.0.2", &token).await, StatusCode::OK);
    for i in 0..20 {
        let _ = me(&format!("100.64.2.{i}"), "sbxw_dev_wrong").await;
    }
    assert_eq!(me("100.64.3.3", "sbxw_dev_wrong").await, StatusCode::TOO_MANY_REQUESTS, "30 failures in all");
}

#[tokio::test]
async fn the_token_is_hashed_at_rest_and_survives_a_restart() {
    let d = tempfile::tempdir().unwrap();
    let app = app_in(d.path());
    app.remote_test_on(TN);
    let token = pair(&app, &remote_router(app.clone()), "phone").await;
    let dir = app.remote_dir_for_tests();
    let state = std::fs::read_to_string(dir.join("state.json")).unwrap();
    assert!(!state.contains(&token) && !state.contains(&token[9..]), "the state file never holds the token");
    assert!(state.contains(&sbxw_core::remote_access::hash_token(&token)));
    let journal = std::fs::read_to_string(dir.join("journal.jsonl")).unwrap();
    assert!(!journal.contains(&token));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = |p: &std::path::Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&dir), 0o700);
        assert_eq!(mode(&dir.join("state.json")), 0o600);
        assert_eq!(mode(&dir.join("journal.jsonl")), 0o600);
    }
    drop(app);

    let again = app_in(d.path());
    assert!(again.remote_enabled(), "on survives a restart");
    again.remote_test_on(TN);
    let r = call(&remote_router(again.clone()), "GET", "/r/me", Some(&token), json!({})).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    assert_eq!(r.json()["device"]["name"], "phone");
    assert!(r.json()["device"].get("token_sha256").is_none());
}

#[tokio::test]
async fn a_device_idle_for_30_days_is_dropped() {
    let d = tempfile::tempdir().unwrap();
    let app = app_in(d.path());
    app.remote_test_on(TN);
    let token = pair(&app, &remote_router(app.clone()), "phone").await;
    let path = app.remote_dir_for_tests().join("state.json");
    drop(app);
    let mut state: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    let old = crate::app::now_ms() - sbxw_core::remote_access::DEVICE_IDLE_MS - 60_000;
    state["devices"][0]["last_seen_ms"] = old.into();
    std::fs::write(&path, state.to_string()).unwrap();
    let again = app_in(d.path());
    again.remote_test_on(TN);
    assert!(again.remote_devices().is_empty());
    assert_eq!(again.remote_journal(10).last().map(|e| e.kind), Some(Kind::Expired));
    let r = call(&remote_router(again.clone()), "GET", "/r/me", Some(&token), json!({})).await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
}

/// Read SSE frames until one contains `want`, or the stream ends (None).
async fn read_until(stream: &mut (impl futures_util::Stream<Item = Result<axum::body::Bytes, axum::Error>> + Unpin), want: &str) -> Option<String> {
    let mut seen = String::new();
    loop {
        match tokio::time::timeout(Duration::from_secs(5), stream.next()).await {
            Ok(Some(Ok(b))) => {
                seen.push_str(&String::from_utf8_lossy(&b));
                if seen.contains(want) {
                    return Some(seen);
                }
            }
            Ok(_) => return None,
            Err(_) => panic!("no {want:?} within 5 s; saw {seen:?}"),
        }
    }
}

#[tokio::test]
async fn revoking_a_device_closes_its_stream_at_once() {
    let (_d, app) = on();
    let router = remote_router(app.clone());
    let token = pair(&app, &router, "phone").await;
    let other = pair(&app, &router, "tablet").await;
    let open = |t: String| {
        let router = router.clone();
        async move {
            let r = req("GET", "/r/events").header(header::COOKIE, format!("{COOKIE}={t}")).body(Body::empty()).unwrap();
            let res = router.oneshot(r).await.unwrap();
            assert_eq!(res.status(), StatusCode::OK);
            res.into_body().into_data_stream()
        }
    };
    let mut mine = open(token.clone()).await;
    let mut theirs = open(other.clone()).await;
    assert!(read_until(&mut mine, "event: leaders").await.is_some());
    assert!(read_until(&mut theirs, "event: leaders").await.is_some());
    let id = app.remote_devices().into_iter().find(|d| d.name == "phone").unwrap().id;
    assert!(app.remote_revoke(&id, Kind::Revoked));
    let bye = read_until(&mut mine, "\"revoked\"").await.expect("a bye frame");
    assert!(bye.contains("event: bye"));
    assert!(tokio::time::timeout(Duration::from_secs(5), mine.next()).await.unwrap().is_none(), "then the stream ends");
    let r = call(&router, "GET", "/r/me", Some(&token), json!({})).await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED, "revocation is immediate");
    assert_eq!(call(&router, "GET", "/r/me", Some(&other), json!({})).await.status, StatusCode::OK, "the other device stays");

    // Turning remote access off closes every stream.
    app.remote_disable().await;
    assert!(read_until(&mut theirs, "\"disabled\"").await.is_some());
}

#[tokio::test]
async fn logout_forgets_the_device_and_clears_the_cookie() {
    let (_d, app) = on();
    let router = remote_router(app.clone());
    let token = pair(&app, &router, "phone").await;
    let r = call(&router, "POST", "/r/logout", Some(&token), json!({})).await;
    assert_eq!(r.status, StatusCode::OK);
    assert!(r.headers[header::SET_COOKIE].to_str().unwrap().contains("Max-Age=0"));
    assert_eq!(call(&router, "GET", "/r/me", Some(&token), json!({})).await.status, StatusCode::UNAUTHORIZED);
    assert_eq!(app.remote_journal(10).last().map(|e| e.kind), Some(Kind::Logout));
}

// ── 3. Guards ────────────────────────────────────────────────────────────

#[tokio::test]
async fn host_origin_methods_and_headers() {
    let (_d, app) = on();
    let router = remote_router(app.clone());
    let token = pair(&app, &router, "phone").await;
    let cookie = format!("{COOKIE}={token}");
    let get = |host: &str, origin: Option<&str>| {
        let mut b = Request::builder().uri("/r/me").header(header::HOST, host).header(header::COOKIE, cookie.clone());
        if let Some(o) = origin {
            b = b.header(header::ORIGIN, o);
        }
        b.body(Body::empty()).unwrap()
    };
    assert_eq!(send(&router, get(TN, None)).await.status, StatusCode::OK);
    assert_eq!(send(&router, get("127.0.0.1:7683", None)).await.status, StatusCode::OK, "loopback, for tests and the sidecar");
    for bad in ["evil.example", "127.0.0.1:7681", "sbxw.tail1.ts.net:443", "SBXW.TAIL1.TS.NET", ""] {
        assert_eq!(send(&router, get(bad, None)).await.status, StatusCode::FORBIDDEN, "Host {bad:?}");
    }
    assert_eq!(send(&router, get(TN, Some("https://evil.example"))).await.status, StatusCode::FORBIDDEN);

    let post = |origin: Option<&str>| {
        let mut b = Request::builder().method("POST").uri("/r/logout").header(header::HOST, TN).header(header::COOKIE, cookie.clone()).header(header::CONTENT_TYPE, "application/json");
        if let Some(o) = origin {
            b = b.header(header::ORIGIN, o);
        }
        b.body(Body::from("{}")).unwrap()
    };
    assert_eq!(send(&router, post(None)).await.status, StatusCode::FORBIDDEN, "Origin required on writes");
    assert_eq!(send(&router, post(Some("http://sbxw.tail1.ts.net"))).await.status, StatusCode::FORBIDDEN, "https only");
    assert_eq!(send(&router, post(Some("null"))).await.status, StatusCode::FORBIDDEN);

    let head = send(&router, Request::builder().method("HEAD").uri("/r/me").header(header::HOST, TN).header(header::COOKIE, cookie.clone()).body(Body::empty()).unwrap()).await;
    assert_eq!(head.status, StatusCode::OK);
    assert!(head.body.is_empty());
    let options = send(&router, req("OPTIONS", "/r/me").header(header::COOKIE, cookie.clone()).header("access-control-request-method", "POST").body(Body::empty()).unwrap()).await;
    assert_eq!(options.status, StatusCode::METHOD_NOT_ALLOWED);
    let r = call(&router, "GET", "/r/me", Some(&token), json!({})).await;
    for res in [&options, &r] {
        assert!(!res.headers.keys().any(|k| k.as_str().starts_with("access-control-")), "never CORS");
        assert_eq!(res.headers[header::CONTENT_SECURITY_POLICY], "default-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'self'");
        assert_eq!(res.headers[header::X_CONTENT_TYPE_OPTIONS], "nosniff");
        assert_eq!(res.headers[header::REFERRER_POLICY], "no-referrer");
        assert_eq!(res.headers[header::X_FRAME_OPTIONS], "DENY");
        assert_eq!(res.headers[header::CACHE_CONTROL], "no-store");
    }
    let page = call(&router, "GET", "/", None, json!({})).await;
    assert_eq!(page.status, StatusCode::OK, "the page needs no token");
    assert_eq!(page.headers[header::X_FRAME_OPTIONS], "DENY");
}

#[tokio::test]
async fn bodies_are_capped_and_json_only() {
    let (_d, app) = on();
    let router = remote_router(app.clone());
    let token = pair(&app, &router, "phone").await;
    let cookie = format!("{COOKIE}={token}");
    let big = json!({ "code": "x".repeat(5000), "device_name": "p" }).to_string();
    let r = send(&router, req("POST", "/r/pair/start").body(Body::from(big)).unwrap()).await;
    assert_eq!(r.status, StatusCode::PAYLOAD_TOO_LARGE);
    let text = json!({ "text": "a".repeat(30 * 1024) }).to_string();
    let r = send(&router, req("POST", "/r/leaders/myapp/message").header(header::COOKIE, cookie.clone()).body(Body::from(text)).unwrap()).await;
    assert_eq!(r.status, StatusCode::PAYLOAD_TOO_LARGE);
    let r = send(
        &router,
        Request::builder()
            .method("POST")
            .uri("/r/logout")
            .header(header::HOST, TN)
            .header(header::ORIGIN, format!("https://{TN}"))
            .header(header::CONTENT_TYPE, "text/plain")
            .header(header::COOKIE, cookie.clone())
            .body(Body::from("{}"))
            .unwrap(),
    )
    .await;
    assert_eq!(r.status, StatusCode::UNSUPPORTED_MEDIA_TYPE);
    let r = send(&router, req("POST", "/r/voice/transcribe").header(header::COOKIE, cookie.clone()).body(Body::from(vec![0u8; 16])).unwrap()).await;
    assert_eq!(r.status, StatusCode::UNSUPPORTED_MEDIA_TYPE, "audio is octet-stream, not JSON");
    let r = send(&router, req("POST", "/r/pair/start").body(Body::from(r#"{"code":"x","device_name":"p","extra":1}"#)).unwrap()).await;
    assert_eq!(r.status, StatusCode::UNPROCESSABLE_ENTITY, "unknown fields refused");
}

#[tokio::test]
async fn path_tricks_find_nothing() {
    let (_d, app) = on();
    let router = remote_router(app.clone());
    let token = pair(&app, &router, "phone").await;
    for p in [
        "/r/../api/sessions",
        "/r/leaders/../../api/sessions",
        "//r/me",
        "/r//me",
        "/R/me",
        "/r/ME",
        "/r/me/",
        "/r/leaders/a%2Fb/history",
        "/r/leaders/%2e%2e/history",
        "/r/leaders/myapp%00/history",
        "/api/sessions",
        "/api/remote",
        "/ws",
        "/r/leaders/myapp/history/../message",
        "/index.html/..",
        "/remote.js/",
        "/r/pair/start/../../api/shutdown",
    ] {
        let r = call(&router, "GET", p, Some(&token), json!({})).await;
        assert_eq!(r.status, StatusCode::NOT_FOUND, "{p}: {}", r.text());
        let r = call(&router, "POST", p, Some(&token), json!({})).await;
        assert_eq!(r.status, StatusCode::NOT_FOUND, "POST {p}: {}", r.text());
    }
    let r = call(&router, "GET", "/r/leaders/myapp/history", Some(&token), json!({})).await;
    assert_eq!(r.status, StatusCode::NOT_FOUND, "not a leader");
}

// ── 4. Data: leaders, history, message, answer, interrupt ────────────────

/// A leader with a fake agent: its PTY runs `cat` into a file, so what is
/// typed into it can be read back; its hooks say what it did.
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
    for _ in 0..50 {
        let got = std::fs::read(file).unwrap_or_default();
        if got.windows(want.len()).any(|w| w == want) {
            return got;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    std::fs::read(file).unwrap_or_default()
}

#[tokio::test]
async fn only_leaders_are_listed_and_their_history_carries_no_tool_detail() {
    let (d, app) = on();
    let router = remote_router(app.clone());
    let token = pair(&app, &router, "phone").await;
    let (sb, _) = leader(&app, d.path(), "lead1").await;
    hook(&app, &sb, "s1", json!({"hook_event_name": "UserPromptSubmit", "prompt": "please read the notes"}));
    hook(&app, &sb, "s1", json!({"hook_event_name": "PreToolUse", "tool_name": "Bash", "tool_input": {"command": "cat /secret/notes.txt"}}));
    hook(&app, &sb, "s1", json!({"hook_event_name": "PostToolUse", "tool_name": "Bash", "tool_input": {"command": "cat /secret/notes.txt"}, "tool_response": {"stdout": "TOP SECRET OUTPUT"}}));
    hook(&app, &sb, "s1", json!({"hook_event_name": "Stop", "stop_hook_active": false, "last_assistant_message": "I read them."}));
    // A thread of the same sandbox, and another sandbox that is not a leader.
    let mut t = json!({"hook_event_name": "UserPromptSubmit", "prompt": "THREAD WORDS"});
    t["sbxw_thread"] = "worker".into();
    hook(&app, &sb, "s9", t);
    let other = SandboxRef::parse("plain").unwrap();
    hook(&app, &other, "s5", json!({"hook_event_name": "UserPromptSubmit", "prompt": "OTHER WORDS"}));

    let r = call(&router, "GET", "/r/leaders", Some(&token), json!({})).await;
    let list = r.json()["leaders"].as_array().unwrap().clone();
    assert_eq!(list.len(), 1, "{list:?}");
    assert_eq!(list[0]["sandbox"], "lead1");
    assert!(!r.text().contains("cat /secret"), "no tool in the activity: {}", r.text());

    let r = call(&router, "GET", "/r/leaders/lead1/history", Some(&token), json!({})).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    let body = r.text();
    for leak in ["cat /secret", "TOP SECRET", "notes.txt", "Bash", "THREAD WORDS", "OTHER WORDS", "\"tool\"", "\"result\""] {
        assert!(!body.contains(leak), "{leak} in {body}");
    }
    assert!(body.contains("please read the notes"), "{body}");
    assert!(body.contains("\"kind\":\"steps\""), "{body}");
    let latest = r.json()["page"]["latest_seq"].as_u64().unwrap();
    let r = call(&router, "GET", &format!("/r/leaders/lead1/history?after={latest}"), Some(&token), json!({})).await;
    assert_eq!(r.json()["page"]["items"].as_array().unwrap().len(), 0);
    assert_eq!(call(&router, "GET", "/r/leaders/plain/history", Some(&token), json!({})).await.status, StatusCode::NOT_FOUND);
    assert_eq!(call(&router, "POST", "/r/leaders/plain/message", Some(&token), json!({"text": "hi"})).await.status, StatusCode::NOT_FOUND);
    assert_eq!(call(&router, "POST", "/r/leaders/plain/interrupt", Some(&token), json!({})).await.status, StatusCode::NOT_FOUND);
}

/// Every log line, as text, while it is the default subscriber.
#[derive(Clone, Default)]
struct Capture(Arc<Mutex<String>>);

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

#[tokio::test(flavor = "current_thread")]
async fn a_message_is_typed_as_the_person_and_never_logged_or_journaled() {
    let logs = Capture::default();
    let _guard = tracing::subscriber::set_default(logs.clone());
    let (d, app) = on();
    let router = remote_router(app.clone());
    let token = pair(&app, &router, "phone").await;
    let (_sb, file) = leader(&app, d.path(), "lead1").await;
    let secret = "PINEAPPLE-77 meet at the usual place";
    let r = call(&router, "POST", "/r/leaders/lead1/message", Some(&token), json!({ "text": secret })).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    let got = typed(&file, b"PINEAPPLE-77").await;
    assert!(String::from_utf8_lossy(&got).contains(secret), "typed into the leader: {:?}", String::from_utf8_lossy(&got));
    // Masked first: an email address never reaches the agent.
    let r = call(&router, "POST", "/r/leaders/lead1/message", Some(&token), json!({ "text": "write to someone@example.org" })).await;
    assert_eq!(r.status, StatusCode::OK);
    assert!(r.json()["masked"].as_u64().unwrap() >= 1);
    let got = typed(&file, b"write to").await;
    assert!(!String::from_utf8_lossy(&got).contains("someone@example.org"));
    // Not recorded as sbxw's own words.
    assert!(app.agents.lock().unwrap().snapshot(|_| true, crate::app::now_ms()).iter().all(|i| i.last_input.as_deref() != Some("[sbxw]")));

    let kinds: Vec<Kind> = app.remote_journal(100).iter().map(|e| e.kind).collect();
    assert!(kinds.contains(&Kind::Message), "{kinds:?}");
    let journal = std::fs::read_to_string(app.remote_dir_for_tests().join("journal.jsonl")).unwrap();
    assert!(!journal.contains("PINEAPPLE") && !journal.contains("someone@"), "{journal}");
    let logged = logs.0.lock().unwrap().clone();
    assert!(logged.contains("sent a message"), "the capture works: {logged}");
    for leak in ["PINEAPPLE", "usual place", "someone@example.org", &token[..]] {
        assert!(!logged.contains(leak), "{leak} in the logs:\n{logged}");
    }
}

#[tokio::test]
async fn a_message_meets_the_same_refusals_as_chat_push() {
    let (d, app) = on();
    let router = remote_router(app.clone());
    let token = pair(&app, &router, "phone").await;
    let (sb, _) = leader(&app, d.path(), "lead1").await;
    // A second session started from the tty: sbxw cannot tell which is the
    // terminal's, so chat_push refuses, and so does the phone.
    hook(&app, &sb, "s2", json!({"hook_event_name": "SessionStart"}));
    let refusal = app.agents.lock().unwrap().can_type_into("lead1").unwrap_err().to_string();
    let r = call(&router, "POST", "/r/leaders/lead1/message", Some(&token), json!({ "text": "hello" })).await;
    assert_eq!(r.status, StatusCode::CONFLICT);
    assert!(r.json()["error"].as_str().unwrap().contains(&refusal), "{}", r.text());
    assert_eq!(app.remote_journal(10).last().map(|e| e.kind), Some(Kind::Refused));
    let r = call(&router, "POST", "/r/leaders/lead1/message", Some(&token), json!({ "text": "   " })).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn a_question_is_answered_a_permission_prompt_never() {
    let (d, app) = on();
    let router = remote_router(app.clone());
    let token = pair(&app, &router, "phone").await;
    let (sb, file) = leader(&app, d.path(), "lead1").await;
    hook(&app, &sb, "s1", json!({"hook_event_name": "PreToolUse", "tool_name": "AskUserQuestion",
        "tool_input": {"questions": [{"question": "Which way?", "options": [{"label": "Left", "description": "reads ~/.ssh"}, {"label": "Right"}]}]}}));
    let r = call(&router, "GET", "/r/leaders", Some(&token), json!({})).await;
    let l = &r.json()["leaders"][0];
    assert_eq!(l["asks"], true);
    assert_eq!(l["answerable"], true, "{l}");
    assert_eq!(l["steps"][0]["options"], json!(["Left", "Right"]));
    assert!(!r.text().contains("reads ~/.ssh"), "no decision table");
    assert_eq!(call(&router, "POST", "/r/leaders/lead1/answer", Some(&token), json!({"indices": [0]})).await.status, StatusCode::BAD_REQUEST, "1-based");
    assert_eq!(call(&router, "POST", "/r/leaders/lead1/answer", Some(&token), json!({"indices": [1, 1]})).await.status, StatusCode::BAD_REQUEST, "one per step");
    let r = call(&router, "POST", "/r/leaders/lead1/answer", Some(&token), json!({"indices": [2]})).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    let got = typed(&file, b"\x1b[B\r").await;
    assert!(got.windows(4).any(|w| w == b"\x1b[B\r"), "{got:?}");

    hook(&app, &sb, "s1", json!({"hook_event_name": "PostToolUse", "tool_name": "AskUserQuestion"}));
    hook(&app, &sb, "s1", json!({"hook_event_name": "PreToolUse", "tool_name": "Bash", "tool_input": {"command": "git push"}}));
    hook(&app, &sb, "s1", json!({"hook_event_name": "Notification", "notification_type": "permission_prompt", "message": "Claude needs your permission to use Bash"}));
    let r = call(&router, "GET", "/r/leaders", Some(&token), json!({})).await;
    let l = &r.json()["leaders"][0];
    assert_eq!(l["asks"], true);
    assert_eq!(l["answerable"], false, "allowing a tool is an approval: not from the phone");
    assert!(!r.text().contains("git push"));
    let before = std::fs::read(&file).unwrap_or_default().len();
    let r = call(&router, "POST", "/r/leaders/lead1/answer", Some(&token), json!({"indices": [1]})).await;
    assert_eq!(r.status, StatusCode::CONFLICT, "{}", r.text());
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(std::fs::read(&file).unwrap_or_default().len(), before, "nothing typed");
}

#[tokio::test]
async fn interrupt_is_one_escape() {
    let (d, app) = on();
    let router = remote_router(app.clone());
    let token = pair(&app, &router, "phone").await;
    let (_sb, file) = leader(&app, d.path(), "lead1").await;
    let r = call(&router, "POST", "/r/leaders/lead1/interrupt", Some(&token), json!({})).await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(typed(&file, b"\x1b").await, b"\x1b");
    assert_eq!(app.remote_journal(10).last().map(|e| e.kind), Some(Kind::Interrupt));
    let r = call(&router, "POST", "/r/leaders/lead1/interrupt", Some(&token), json!({"data": "rm -rf /\r"})).await;
    assert_eq!(r.status, StatusCode::UNPROCESSABLE_ENTITY, "no raw input rides along");
}

#[tokio::test]
async fn events_name_a_leader_that_needs_you() {
    let (d, app) = on();
    let router = remote_router(app.clone());
    let token = pair(&app, &router, "phone").await;
    let (sb, _) = leader(&app, d.path(), "lead1").await;
    let r = req("GET", "/r/events").header(header::COOKIE, format!("{COOKIE}={token}")).body(Body::empty()).unwrap();
    let mut s = router.clone().oneshot(r).await.unwrap().into_body().into_data_stream();
    assert!(read_until(&mut s, "lead1").await.is_some());
    hook(&app, &sb, "s1", json!({"hook_event_name": "PreToolUse", "tool_name": "AskUserQuestion",
        "tool_input": {"questions": [{"question": "SECRET QUESTION?", "options": [{"label": "A"}, {"label": "B"}]}]}}));
    let frames = read_until(&mut s, "event: needs_you").await.unwrap();
    assert!(frames.contains("{\"sandbox\":\"lead1\"}"), "{frames}");
}

// ── 5. Disabled, enable, disable ─────────────────────────────────────────

#[tokio::test]
async fn off_means_no_listener_and_nothing_answers() {
    let (_d, app) = app();
    assert!(!app.remote_enabled(), "off by default");
    let r = call(&remote_router(app.clone()), "GET", "/", None, json!({})).await;
    assert_eq!(r.status, StatusCode::NOT_FOUND, "a connection kept from before gets nothing");
    {
        let mut rt = app.remote_access.lock().unwrap();
        rt.sidecar_program = crate::svc::remote_access::Sidecar::None;
        rt.addr_override = Some("127.0.0.1:0".into());
        rt.tailnet_override = Some(TN.into());
    }
    app.remote_boot().await;
    assert!(app.remote_listen_addr().is_none(), "boot does not start what is off");

    app.remote_enable().await.unwrap();
    let addr = app.remote_listen_addr().expect("bound");
    app.remote_enable().await.unwrap();
    assert_eq!(app.remote_listen_addr(), Some(addr), "enable is idempotent");
    let got = raw_get(addr).await.expect("answers while on");
    assert!(got.starts_with("HTTP/1.1 200"), "{got}");
    let st = app.remote_status();
    assert!(st.enabled && st.listening);

    app.remote_disable().await;
    app.remote_disable().await;
    assert!(app.remote_listen_addr().is_none());
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(raw_get(addr).await.is_none(), "no listener once off");
    let st = app.remote_status();
    assert!(!st.enabled && !st.listening);
    assert_eq!(st.keep_awake, "off");
}

async fn raw_get(addr: std::net::SocketAddr) -> Option<String> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut s = tokio::net::TcpStream::connect(addr).await.ok()?;
    let req = format!("GET / HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nConnection: close\r\n\r\n", addr.port());
    s.write_all(req.as_bytes()).await.ok()?;
    let mut out = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(3), s.read_to_end(&mut out)).await;
    Some(String::from_utf8_lossy(&out).into_owned())
}

#[tokio::test]
async fn disable_voids_codes_and_requests_but_keeps_devices() {
    let (_d, app) = on();
    let router = remote_router(app.clone());
    let _token = pair(&app, &router, "phone").await;
    let code = app.remote_pairing_code().unwrap().code;
    app.remote_disable().await;
    app.remote_test_on(TN);
    let r = call(&router, "POST", "/r/pair/start", None, json!({ "code": code, "device_name": "p" })).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN, "the code died with the switch");
    assert_eq!(app.remote_devices().len(), 1);
}

#[tokio::test]
async fn a_missing_sidecar_fails_enable_plainly() {
    let (_d, app) = app();
    {
        let mut rt = app.remote_access.lock().unwrap();
        rt.addr_override = Some("127.0.0.1:0".into());
        rt.sidecar_program = crate::svc::remote_access::Sidecar::Program("/nonexistent/sbxw-tailnet".into());
    }
    let e = app.remote_enable().await.unwrap_err();
    assert!(e.contains("cannot start sbxw-tailnet"), "{e}");
    assert!(!app.remote_enabled());
    assert!(app.remote_listen_addr().is_none(), "nothing left bound");
}

#[tokio::test]
async fn the_sidecar_is_started_with_a_fixed_argv_and_a_clean_environment() {
    let (d, app) = app();
    let out = d.path().join("argv.txt");
    let fake = d.path().join("fake-tailnet");
    std::fs::write(&fake, format!("#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\nenv >> '{}'\nsleep 30\n", out.display(), out.display())).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    {
        let mut rt = app.remote_access.lock().unwrap();
        rt.addr_override = Some("127.0.0.1:0".into());
        rt.sidecar_program = crate::svc::remote_access::Sidecar::Program(fake.clone());
    }
    std::env::set_var("TS_AUTHKEY", "tskey-should-not-pass");
    app.remote_enable().await.unwrap();
    let mut text = String::new();
    for _ in 0..50 {
        text = std::fs::read_to_string(&out).unwrap_or_default();
        if text.contains("--parent-pid") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    std::env::remove_var("TS_AUTHKEY");
    let addr = app.remote_listen_addr().unwrap().to_string();
    let dir = app.remote_dir_for_tests();
    let want = format!(
        "--forward\n{addr}\n--state-dir\n{}\n--status-file\n{}\n--hostname\nsbxw\n--parent-pid\n{}\n",
        dir.join("tailnet").display(),
        dir.join("tailnet.json").display(),
        std::process::id()
    );
    assert!(text.starts_with(&want), "{text}");
    assert!(!text.contains("TS_AUTHKEY"), "the environment is cleared: {text}");
    // Not serving until the sidecar says so, from its own pid.
    assert!(app.remote_hosts().1.is_none());
    app.remote_disable().await;
}
