//! The two routers, driven as a client would: the security boundaries first.

use super::{agent_router, ui_router};
use crate::testing::signed_in;
use crate::testing::app;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::{json, Value};
use tower::ServiceExt;

const HOST: &str = "127.0.0.1:7681";
/// The one name a browser signs in at, on a loopback daemon.
const SIGN_IN_HOST: &str = "sbxw.localhost:7681";

async fn call(router: axum::Router, req: Request<Body>) -> (StatusCode, Value) {
    let res = router.oneshot(req).await.unwrap();
    let status = res.status();
    let bytes = axum::body::to_bytes(res.into_body(), 1 << 20).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

fn post(path: &str, body: Value) -> axum::http::request::Builder {
    let _ = body;
    Request::builder().method("POST").uri(path).header("content-type", "application/json")
}

fn json_req(b: axum::http::request::Builder, body: Value) -> Request<Body> {
    b.body(Body::from(body.to_string())).unwrap()
}

#[tokio::test]
async fn the_ui_refuses_foreign_hosts_and_origins() {
    let (_d, app) = app();
    let get = |host: &str, origin: Option<&str>| {
        let mut b = Request::builder().uri("/api/health").header("host", host);
        if let Some(o) = origin {
            b = b.header("origin", o);
        }
        b.body(Body::empty()).unwrap()
    };
    let (s, v) = call(signed_in(app.clone()), get(HOST, None)).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["ok"], true);
    assert_eq!(call(signed_in(app.clone()), get("sbxw.localhost:7681", Some("http://sbxw.localhost:7681"))).await.0, StatusCode::OK);
    assert_eq!(call(signed_in(app.clone()), get("attacker.example:7681", None)).await.0, StatusCode::FORBIDDEN);
    assert_eq!(call(signed_in(app.clone()), get(HOST, Some("https://evil.example"))).await.0, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn a_cross_site_websocket_is_refused_before_it_opens() {
    let (_d, app) = app();
    let req = Request::builder()
        .uri("/ws?mode=monitor")
        .header("host", HOST)
        .header("origin", "https://evil.example")
        .header("connection", "upgrade")
        .header("upgrade", "websocket")
        .header("sec-websocket-version", "13")
        .header("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ==")
        .body(Body::empty())
        .unwrap();
    assert_eq!(call(signed_in(app), req).await.0, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn an_agent_needs_a_valid_token() {
    let (_d, app) = app();
    let hook = json!({"hook_event_name": "SessionStart", "session_id": "s"});
    let no_token = json_req(post("/api/hook", hook.clone()), hook.clone());
    assert_eq!(call(agent_router(app.clone()), no_token).await.0, StatusCode::UNAUTHORIZED);
    let wrong = json_req(post("/api/hook", hook.clone()).header("authorization", "Bearer sbxw_nope"), hook.clone());
    assert_eq!(call(agent_router(app.clone()), wrong).await.0, StatusCode::UNAUTHORIZED);

    let token = app.store.token(sbxw_core::SandboxRef::parse("myapp").unwrap()).unwrap();
    let ok = json_req(post("/api/hook", hook.clone()).header("authorization", format!("Bearer {token}")), hook);
    assert_eq!(call(agent_router(app.clone()), ok).await.0, StatusCode::OK);
    assert_eq!(app.session_infos()[0].sandbox, "myapp", "identity comes from the token");
}

#[tokio::test]
async fn a_sandbox_cannot_claim_another_identity_or_reach_human_decisions() {
    let (_d, app) = app();
    let alpha = app.store.token(sbxw_core::SandboxRef::parse("alpha").unwrap()).unwrap();
    let beta = app.store.token(sbxw_core::SandboxRef::parse("beta").unwrap()).unwrap();
    // alpha asks (with a lying `from`, which is ignored), parks briefly.
    let ask = json!({"from": "beta", "question": "q?", "timeout": 1});
    let (s, v) = call(
        agent_router(app.clone()),
        json_req(post("/api/relay/ask", ask.clone()).header("authorization", format!("Bearer {alpha}")), ask),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let id = v["id"].as_str().unwrap().to_string();
    assert_eq!(app.relay_list()[0].from, "alpha");
    // beta cannot wait on it, nor answer it (it was not routed to beta).
    let wait = json!({"id": id, "timeout": 1});
    let (s, _) = call(
        agent_router(app.clone()),
        json_req(post("/api/relay/wait", wait.clone()).header("authorization", format!("Bearer {beta}")), wait),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    // The human's half does not exist on the agent port at all.
    let approve = json!({"answer": "leak"});
    let (s, _) = call(
        agent_router(app.clone()),
        json_req(post(&format!("/api/relay/{id}/approve"), approve.clone()).header("authorization", format!("Bearer {alpha}")), approve),
    )
    .await;
    assert_eq!(s, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_code_map_report_is_for_the_callers_own_sandbox() {
    let (_d, app) = app();
    let alpha = app.store.token(sbxw_core::SandboxRef::parse("alpha").unwrap()).unwrap();
    let req = Request::builder()
        .method("POST")
        .uri(format!("/api/sandboxes/beta/codemap/done?token={alpha}"))
        .body(Body::empty())
        .unwrap();
    assert_eq!(call(agent_router(app.clone()), req).await.0, StatusCode::FORBIDDEN);
    let req = Request::builder()
        .method("POST")
        .uri(format!("/api/sandboxes/alpha/codemap/done?token={alpha}"))
        .body(Body::empty())
        .unwrap();
    let (s, v) = call(agent_router(app), req).await;
    assert_eq!((s, v["tracked"].as_bool()), (StatusCode::OK, Some(false)));
}

#[tokio::test]
async fn old_tooling_on_the_ui_port_is_told_how_to_fix_it() {
    let (_d, app) = app();
    let body = json!({"hook_event_name": "Stop"});
    let req = json_req(post("/api/hook", body.clone()).header("host", HOST), body);
    let (s, v) = call(signed_in(app), req).await;
    assert_eq!(s, StatusCode::GONE);
    assert!(v["error"].as_str().unwrap().contains("sbxw up"));
}

#[tokio::test]
async fn failures_carry_a_status_and_the_old_envelope() {
    let (_d, app) = app();
    let body = json!({"name": "bad name!", "path": "/"});
    let req = json_req(post("/api/sandboxes/create", body.clone()).header("host", HOST), body);
    let (s, v) = call(signed_in(app), req).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_eq!(v["ok"], false);
    assert!(v["error"].as_str().unwrap().contains("letters, digits and hyphens"));
}

#[tokio::test]
async fn the_index_carries_its_versions_and_the_state_is_one_call() {
    let (_d, app) = app();
    let res = signed_in(app.clone())
        .oneshot(Request::builder().uri("/").header("host", HOST).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let html = String::from_utf8(axum::body::to_bytes(res.into_body(), 1 << 22).await.unwrap().to_vec()).unwrap();
    assert!(html.contains(&format!("const SBXW_VERSION = \"{}\"", env!("CARGO_PKG_VERSION"))));
    assert!(!html.contains("__SBX_VERSION__"));
    let (s, v) = call(signed_in(app), Request::builder().uri("/api/state").header("host", HOST).body(Body::empty()).unwrap()).await;
    assert_eq!(s, StatusCode::OK);
    for k in ["sessions", "relay", "plans", "codemap", "leads", "usage", "models"] {
        assert!(v.get(k).is_some(), "{k}");
    }
    assert_eq!(v["ssh_refresh_due"], false, "a bool, false where SSH is not set up");
    assert_eq!(v["terminal_colors"], serde_json::json!({ "background": "", "bar": "", "text": "" }), "sbxw.toml's terminal colours, empty when unset");
}

#[tokio::test]
async fn a_policy_check_refuses_a_target_that_is_not_a_host_before_sbx_hears_of_it() {
    let (_d, app, fake) = crate::testing::app_with(crate::testing::sbx_answer);
    for target in ["", "%2Drf", "a.com%3Brm", "a.com%3A0", "127.0.0.1"] {
        let req = Request::builder().uri(format!("/api/sandboxes/web/policy/check?target={target}")).header("host", HOST).body(Body::empty()).unwrap();
        let (s, v) = call(signed_in(app.clone()), req).await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "{target}: {v}");
        assert_eq!(v["ok"], false);
    }
    assert!(!fake.log().iter().any(|l| l.starts_with("policy")), "{:?}", fake.log());
    // sbx 0.51 (the fake's): not supported, said, not an error.
    let req = Request::builder().uri("/api/sandboxes/web/policy/check?target=pypi.org").header("host", HOST).body(Body::empty()).unwrap();
    let (s, v) = call(signed_in(app.clone()), req).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!((v["ok"].clone(), v["supported"].clone()), (json!(true), json!(false)));
    let req = Request::builder().uri("/api/sandboxes/web/mcp").header("host", HOST).body(Body::empty()).unwrap();
    let (s, v) = call(signed_in(app.clone()), req).await;
    assert_eq!(s, StatusCode::BAD_GATEWAY, "inspect prints nothing in this fake: {v}");
    let (s, v) = call(signed_in(app), Request::builder().uri("/api/ssh").header("host", HOST).body(Body::empty()).unwrap()).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["refresh_due"], false);
}

#[test]
fn an_upload_keeps_its_name_as_one_safe_component() {
    use sbxw_core::uploads::upload_name;
    assert_eq!(upload_name("shot 1 (final).png"), "shot_1__final_.png");
    assert_eq!(upload_name("../../etc/passwd"), "passwd");
    assert_eq!(upload_name("C:\\Users\\me\\clip.mov"), "clip.mov");
    assert_eq!(upload_name(".env"), "env");
    assert_eq!(upload_name("..."), "upload");
    assert_eq!(upload_name("été.jpg"), "_t_.jpg");
}

#[tokio::test]
async fn a_sandbox_on_an_unregistered_host_is_not_found_and_its_hooks_are_its_own() {
    let (_d, app) = app();
    let get = |path: &str| Request::builder().uri(path).header("host", HOST).body(Body::empty()).unwrap();
    let (s, v) = call(signed_in(app.clone()), get("/api/sandboxes/buildbox~myapp/ports")).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    assert!(v["error"].as_str().unwrap().contains("no remote host named 'buildbox'"), "{v}");
    let stop = json_req(post("/api/sandboxes/buildbox~myapp/stop", json!({})).header("host", HOST), json!({}));
    assert_eq!(call(signed_in(app.clone()), stop).await.0, StatusCode::NOT_FOUND);
    let (s, _) = call(signed_in(app.clone()), get("/api/sandboxes/a~b~c/ports")).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "a malformed ref is a bad request");
    assert!(!app.store.paths.state().join("hosts").exists(), "asking writes nothing");

    // A remote sandbox's hooks come home through its link with its token,
    // and its sessions are filed under its ref.
    let token = app.store.token(sbxw_core::SandboxRef::parse("buildbox~myapp").unwrap()).unwrap();
    let hook = json!({"hook_event_name": "SessionStart", "session_id": "s"});
    let req = json_req(post("/api/hook", hook.clone()).header("authorization", format!("Bearer {token}")), hook);
    assert_eq!(call(agent_router(app.clone()), req).await.0, StatusCode::OK);
    assert!(app.session_infos().iter().any(|i| i.sandbox == "buildbox~myapp"));
}

#[tokio::test]
async fn the_initial_sandbox_may_be_a_remote_ref() {
    let (_d, app) = app();
    let req = json_req(post("/api/initial", json!({})).header("host", HOST), json!({"sandbox": "local~myapp"}));
    assert_eq!(call(signed_in(app.clone()), req).await.0, StatusCode::OK);
    assert_eq!(*app.initial_sandbox.lock().unwrap(), "myapp", "normalised");
    let req = json_req(post("/api/initial", json!({})).header("host", HOST), json!({"sandbox": "buildbox~myapp"}));
    assert_eq!(call(signed_in(app.clone()), req).await.0, StatusCode::OK);
    assert_eq!(*app.initial_sandbox.lock().unwrap(), "buildbox~myapp");
}

#[tokio::test]
async fn the_virtual_mac_is_lent_by_token_and_steered_only_from_the_ui() {
    let (_d, app) = app();
    let alpha = app.store.token(sbxw_core::SandboxRef::parse("alpha").unwrap()).unwrap();
    let shot = json!({"action": "screenshot", "args": {}});
    let no_token = json_req(post("/api/mac/action", shot.clone()), shot.clone());
    assert_eq!(call(agent_router(app.clone()), no_token).await.0, StatusCode::UNAUTHORIZED);
    // Without terminal-mac the call fails with what to do, not a crash.
    let (s, v) = call(
        agent_router(app.clone()),
        json_req(post("/api/mac/action", shot.clone()).header("authorization", format!("Bearer {alpha}")), shot),
    )
    .await;
    assert_ne!(s, StatusCode::OK);
    assert!(v["error"].as_str().unwrap_or("").len() > 10, "{v}");
    // Booting it, or taking it back, is the person's: not on the agent port.
    for path in ["/api/mac/up", "/api/mac/install", "/api/mac/take-back", "/api/mac/stop", "/api/mac/force-off"] {
        let (s, _) = call(
            agent_router(app.clone()),
            json_req(post(path, json!({})).header("authorization", format!("Bearer {alpha}")), json!({})),
        )
        .await;
        assert_eq!(s, StatusCode::NOT_FOUND, "{path}");
    }
}

#[tokio::test]
async fn the_voice_panel_reads_its_models_and_refuses_what_it_cannot_do() {
    let (_d, app) = app();
    let get = Request::builder().uri("/api/voice").header("host", HOST).body(Body::empty()).unwrap();
    let (s, v) = call(signed_in(app.clone()), get).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["ready"], false, "nothing is downloaded on its own");
    assert_eq!(v["compiled"], cfg!(feature = "voice"));
    let models = if sbxw_core::voice::speech_supported() { 2 } else { 1 };
    assert_eq!(v["models"].as_array().unwrap().len(), models, "the recogniser, and the voice where MLX runs");
    assert_eq!(v["speech"]["supported"], sbxw_core::voice::speech_supported());
    assert_eq!(v["hint"], "sbxw voice install");

    let (s, _) = call(signed_in(app.clone()), json_req(post("/api/voice/cancel", json!({})).header("host", HOST), json!({}))).await;
    assert_eq!(s, StatusCode::CONFLICT, "nothing to cancel");

    let pcm: Vec<u8> = [0.0f32; 1600].iter().flat_map(|f| f.to_le_bytes()).collect();
    let req = Request::builder()
        .method("POST")
        .uri("/api/voice/transcribe")
        .header("host", HOST)
        .header("content-type", "application/octet-stream")
        .body(Body::from(pcm))
        .unwrap();
    let (s, v) = call(signed_in(app.clone()), req).await;
    assert_eq!(s, StatusCode::SERVICE_UNAVAILABLE, "no build or no models: {v}");
    assert!(v["error"].as_str().unwrap().contains("voice"));

    // Anonymisation: shapes without a file, names with one, nothing through a broken one.
    let hide = |text: &str| json_req(post("/api/voice/anonymise", json!({})).header("host", HOST), json!({ "text": text }));
    let (s, v) = call(signed_in(app.clone()), hide("Jeanne Martin, a@b.ch")).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!((v["text"].as_str().unwrap(), v["masked"].as_u64().unwrap()), ("Jeanne Martin, [email]", 1));
    let file = app.store.paths.anonymise();
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, "[replace]\n\"Jeanne Martin\" = \"Personne A\"\n").unwrap();
    let (_, v) = call(signed_in(app.clone()), hide("jeanne martin, a@b.ch")).await;
    assert_eq!(v["text"], "Personne A, [email]", "the file is read at once, no restart");
    let (_, v) = call(signed_in(app.clone()), Request::builder().uri("/api/voice").header("host", HOST).body(Body::empty()).unwrap()).await;
    assert_eq!(v["anonymise"]["terms"], 1);
    std::fs::write(&file, "[remplace]\n").unwrap();
    let (s, v) = call(signed_in(app.clone()), hide("jeanne martin")).await;
    assert_eq!(s, StatusCode::SERVICE_UNAVAILABLE, "a typo in the file must not let words through: {v}");
    assert!(v["error"].as_str().unwrap().contains("anonymise.toml"));
    std::fs::remove_file(&file).unwrap();

    for gone in ["/api/voice/chat", "/api/voice/conversations"] {
        let (s, _) = call(signed_in(app.clone()), json_req(post(gone, json!({})).header("host", HOST), json!({}))).await;
        assert_eq!(s, StatusCode::NOT_FOUND, "{gone}: no language model, nothing kept");
    }

    let empty = json!({"text": "  "});
    let (s, _) = call(signed_in(app.clone()), json_req(post("/api/voice/speak", empty.clone()).header("host", HOST), empty)).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    let said = json!({"text": "Bonjour."});
    let (s, _) = call(signed_in(app.clone()), json_req(post("/api/voice/speak", said.clone()).header("host", HOST), said)).await;
    assert_eq!(s, StatusCode::SERVICE_UNAVAILABLE);
}

// ── The UI token ─────────────────────────────────────────────────────────

fn ws_upgrade(b: axum::http::request::Builder) -> Request<Body> {
    b.uri("/ws?mode=monitor")
        .header("host", HOST)
        .header("connection", "upgrade")
        .header("upgrade", "websocket")
        .header("sec-websocket-version", "13")
        .header("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ==")
        .body(Body::empty())
        .unwrap()
}

#[tokio::test]
async fn without_the_ui_token_nothing_but_the_sign_in_answers() {
    let (_d, app) = app();
    let get = |path: &str| Request::builder().uri(path).header("host", HOST).body(Body::empty()).unwrap();
    for path in ["/api/state", "/api/sessions", "/api/stats", "/app.css", "/js/main.js", "/vendor/xterm.js"] {
        let res = ui_router(app.clone()).oneshot(get(path)).await.unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED, "{path}");
        let body = axum::body::to_bytes(res.into_body(), 1 << 16).await.unwrap();
        let body = String::from_utf8(body.to_vec()).unwrap();
        assert!(body.contains("sbxw web") && !body.contains(&app.ui_token), "{path}: {body}");
    }
    let post_it = json_req(post("/api/sandboxes/create", json!({})).header("host", HOST), json!({"name": "x", "path": "/"}));
    assert_eq!(call(ui_router(app.clone()), post_it).await.0, StatusCode::UNAUTHORIZED);
    assert_eq!(call(ui_router(app.clone()), ws_upgrade(Request::builder())).await.0, StatusCode::UNAUTHORIZED, "/ws");

    // `/` and `/sidepanel` say how to sign in, and nothing more.
    for path in ["/", "/sidepanel"] {
        let res = ui_router(app.clone()).oneshot(get(path)).await.unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED, "{path}");
        let html = String::from_utf8(axum::body::to_bytes(res.into_body(), 1 << 16).await.unwrap().to_vec()).unwrap();
        assert!(!html.contains("<script") && !html.contains("SBXW_VERSION"), "{path}: {html}");
    }
    let h = ui_router(app.clone()).oneshot(get("/sidepanel")).await.unwrap().headers().clone();
    assert!(h[axum::http::header::CONTENT_SECURITY_POLICY].to_str().unwrap().contains("chrome-extension://"), "the extension may frame it");
    // Health: no version, no addresses.
    let (s, v) = call(ui_router(app.clone()), get("/api/health")).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v, json!({"ok": true, "proof": null}));
    // The Host check still runs first.
    let foreign = Request::builder().uri("/api/state").header("host", "evil.example:7681").header("authorization", format!("Bearer {}", app.ui_token)).body(Body::empty()).unwrap();
    assert_eq!(call(ui_router(app.clone()), foreign).await.0, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn the_bearer_or_the_cookie_passes_a_wrong_token_does_not() {
    let (_d, app) = app();
    let with = |name: &str, value: String| Request::builder().uri("/api/state").header("host", SIGN_IN_HOST).header(name, value).body(Body::empty()).unwrap();
    let t = app.ui_token.clone();
    assert_eq!(call(ui_router(app.clone()), with("authorization", format!("Bearer {t}"))).await.0, StatusCode::OK);
    assert_eq!(call(ui_router(app.clone()), with("cookie", format!("theme=dark; sbxw_ui={t}"))).await.0, StatusCode::OK);
    let wrong = crate::http::token::mint();
    assert_eq!(wrong.len(), t.len());
    assert_eq!(call(ui_router(app.clone()), with("authorization", format!("Bearer {wrong}"))).await.0, StatusCode::UNAUTHORIZED);
    assert_eq!(call(ui_router(app.clone()), with("cookie", format!("sbxw_ui={wrong}"))).await.0, StatusCode::UNAUTHORIZED);
    assert_eq!(call(ui_router(app.clone()), with("cookie", format!("xsbxw_ui={t}"))).await.0, StatusCode::UNAUTHORIZED);
    assert_eq!(call(ui_router(app.clone()), with("authorization", format!("Bearer {}", app.store.token(sbxw_core::SandboxRef::parse("alpha").unwrap()).unwrap()))).await.0, StatusCode::UNAUTHORIZED, "an agent token is not a UI token");
    // The WebSocket with the cookie gets past the guard (the upgrade itself
    // needs a real connection, which a oneshot is not).
    let ws = ws_upgrade(Request::builder().header("cookie", format!("sbxw_ui={t}")).header("host", SIGN_IN_HOST));
    assert_ne!(call(ui_router(app.clone()), ws).await.0, StatusCode::UNAUTHORIZED);
    // The cookie counts only at the name it is set for: one that reached a
    // `127.0.0.1` or `localhost` address was not set by sbxw.
    for host in [HOST, "localhost:7681"] {
        let req = Request::builder().uri("/api/state").header("host", host).header("cookie", format!("sbxw_ui={t}")).body(Body::empty()).unwrap();
        assert_eq!(call(ui_router(app.clone()), req).await.0, StatusCode::UNAUTHORIZED, "{host}");
    }
}

#[tokio::test]
async fn the_sign_in_address_sets_the_cookie_and_drops_the_token() {
    let (_d, app) = app();
    let t = app.ui_token.clone();
    let get = |uri: String| Request::builder().uri(uri).header("host", SIGN_IN_HOST).body(Body::empty()).unwrap();
    let res = ui_router(app.clone()).oneshot(get(format!("/?t={t}"))).await.unwrap();
    assert_eq!(res.status(), StatusCode::SEE_OTHER);
    let h = res.headers();
    assert_eq!(h["location"], "/");
    assert_eq!(h["cache-control"], "no-store");
    assert_eq!(h["set-cookie"].to_str().unwrap(), format!("sbxw_ui={t}; HttpOnly; SameSite=Strict; Path=/"));

    let res = ui_router(app.clone()).oneshot(get(format!("/?a=1&t={t}&b=2"))).await.unwrap();
    assert_eq!(res.headers()["location"], "/?a=1&b=2", "other parameters stay");

    let res = ui_router(app.clone()).oneshot(get(format!("/?t={}", crate::http::token::mint()))).await.unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    assert!(res.headers().get("set-cookie").is_none());

    // At 127.0.0.1 or localhost, where sandbox apps are published: no
    // cookie, only the same sign-in at sbxw.localhost.
    for host in [HOST, "localhost:7681"] {
        let req = Request::builder().uri(format!("/?t={t}&a=1")).header("host", host).body(Body::empty()).unwrap();
        let res = ui_router(app.clone()).oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::SEE_OTHER, "{host}");
        assert!(res.headers().get("set-cookie").is_none(), "{host}");
        assert_eq!(res.headers()["location"].to_str().unwrap(), format!("http://sbxw.localhost:7681/?t={t}&a=1"));
        assert_eq!(res.headers()["cache-control"], "no-store");
    }

    // Signed in, the page itself.
    let page = Request::builder().uri("/").header("host", SIGN_IN_HOST).header("cookie", format!("sbxw_ui={t}")).body(Body::empty()).unwrap();
    let res = ui_router(app.clone()).oneshot(page).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let html = String::from_utf8(axum::body::to_bytes(res.into_body(), 1 << 22).await.unwrap().to_vec()).unwrap();
    assert!(html.contains("const SBXW_VERSION"));
}

#[tokio::test]
async fn health_proves_the_daemon_knows_the_token_without_showing_it() {
    let (_d, app) = app();
    let nonce = sbxw_core::ids::random_hex(16);
    let get = |uri: String| Request::builder().uri(uri).header("host", HOST).body(Body::empty()).unwrap();
    let (s, v) = call(ui_router(app.clone()), get(format!("/api/health?nonce={nonce}"))).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["proof"], crate::http::token::proof(&app.ui_token, &nonce));
    assert!(v.get("version").is_none() && v.get("web_addr").is_none(), "{v}");
    assert!(!v.to_string().contains(&app.ui_token));
    let (_, v) = call(ui_router(app.clone()), get("/api/health?nonce=short".into())).await;
    assert!(v["proof"].is_null(), "a nonce must be 16 to 128 letters or digits");
    // Signed in, the full answer.
    let (_, v) = call(signed_in(app.clone()), get(format!("/api/health?nonce={nonce}"))).await;
    assert_eq!(v["version"], env!("CARGO_PKG_VERSION"));
}

#[tokio::test]
async fn off_loopback_the_cookie_is_set_for_the_name_the_browser_used() {
    let d = tempfile::tempdir().unwrap();
    let cfg = sbxw_core::config::Config { web_addr: "0.0.0.0:7681".into(), ..Default::default() };
    let app = crate::testing::app_with_config(d.path(), cfg);
    let t = app.ui_token.clone();
    let req = Request::builder().uri(format!("/?t={t}")).header("host", "192.168.1.9:7681").body(Body::empty()).unwrap();
    let res = ui_router(app.clone()).oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::SEE_OTHER);
    assert!(res.headers()["set-cookie"].to_str().unwrap().starts_with("sbxw_ui="));
    let req = Request::builder().uri("/api/state").header("host", "192.168.1.9:7681").header("cookie", format!("sbxw_ui={t}")).body(Body::empty()).unwrap();
    assert_eq!(call(ui_router(app), req).await.0, StatusCode::OK);
}

#[test]
fn the_cookie_name_is_sbxw_localhost_on_loopback_only() {
    use super::guard::cookie_host;
    assert_eq!(cookie_host("127.0.0.1:7681").as_deref(), Some("sbxw.localhost:7681"));
    assert_eq!(cookie_host("127.0.0.1:7700").as_deref(), Some("sbxw.localhost:7700"));
    assert_eq!(cookie_host("0.0.0.0:7681"), None);
    assert_eq!(cookie_host("192.168.1.9:7681"), None);
}

/// Jarvis's routes answer the daemon's own `sbxw-jarvis` sandbox alone: not
/// another sandbox, not one of that name on a remote host, not one of that
/// name the daemon did not create. The person's half is on the UI listener.
#[tokio::test]
async fn only_the_jarvis_sandbox_may_propose_and_voice_cannot_approve_high_risk() {
    use sbxw_core::SandboxRef;
    let (_d, app) = app();
    let jarvis = SandboxRef::parse(sbxw_core::jarvis::SANDBOX).unwrap();
    let ws = crate::svc::jarvis_sandbox::workspace(&app.store.paths);
    app.store.update(jarvis.clone(), |r| r.workspace = Some(ws.clone())).unwrap();
    let remote = SandboxRef::parse(&format!("bb~{}", sbxw_core::jarvis::SANDBOX)).unwrap();
    app.store.update(remote.clone(), |r| r.workspace = Some(ws.clone())).unwrap();
    let as_ = |sb: &SandboxRef, path: &str, body: Value| {
        let token = app.store.token(sb.clone()).unwrap();
        json_req(post(path, body.clone()).header("authorization", format!("Bearer {token}")), body)
    };
    let kit = json!({"summary": "a kit", "timeout": 1, "changes": [{"kind": "kit_write", "kit": "jarvis-a", "files": [{"path": "a.md", "content": "a"}]}]});

    for who in ["myapp", "sbxw-jarvis-2"] {
        let sb = SandboxRef::parse(who).unwrap();
        for path in ["/api/jarvis/state", "/api/jarvis/propose", "/api/jarvis/wait", "/api/jarvis/desktop/screen", "/api/jarvis/desktop/docs"] {
            assert_eq!(call(agent_router(app.clone()), as_(&sb, path, kit.clone())).await.0, StatusCode::FORBIDDEN, "{who} {path}");
        }
    }
    assert_eq!(call(agent_router(app.clone()), as_(&remote, "/api/jarvis/propose", kit.clone())).await.0, StatusCode::FORBIDDEN, "a remote host's sandbox of that name");
    let impostor = SandboxRef::parse("sbxw-jarvis").unwrap();
    app.store.update(impostor.clone(), |r| r.workspace = Some("/elsewhere".into())).unwrap();
    assert_eq!(call(agent_router(app.clone()), as_(&impostor, "/api/jarvis/state", json!({}))).await.0, StatusCode::FORBIDDEN, "not created by the daemon");
    app.store.update(jarvis.clone(), |r| r.workspace = Some(ws.clone())).unwrap();

    let (s, v) = call(agent_router(app.clone()), as_(&jarvis, "/api/jarvis/state", json!({}))).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert!(v["settings"].as_array().unwrap().iter().any(|s| s["key"] == "voice.tts_voice"));
    assert!(!v.to_string().contains("sbxw_"), "no token in the view: {v}");
    assert_eq!(v["desktop"]["app_ran"], false, "{v}");
    // The desktop app never ran here: its routes say so, in words.
    let (s, v) = call(agent_router(app.clone()), as_(&jarvis, "/api/jarvis/desktop/screen", json!({"name": "topbar"}))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert!(v["error"].as_str().unwrap().contains("has not run on this machine"), "{v}");
    let (s, v) = call(agent_router(app.clone()), as_(&jarvis, "/api/jarvis/desktop/docs", json!({}))).await;
    assert!(s == StatusCode::BAD_REQUEST && v["error"].as_str().unwrap().contains("has not run"), "{v}");

    let bad = json!({"summary": "s", "changes": [{"kind": "setting", "key": "web_addr", "value": "0.0.0.0:1"}]});
    let (s, v) = call(agent_router(app.clone()), as_(&jarvis, "/api/jarvis/propose", bad)).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert!(v["error"].as_str().unwrap().contains("not a setting Jarvis may change"), "{v}");

    let (s, v) = call(agent_router(app.clone()), as_(&jarvis, "/api/jarvis/propose", kit)).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!((v["state"].as_str(), v["risk"].as_str()), (Some("pending"), Some("high")));
    let id = v["id"].as_str().unwrap().to_string();

    let ui = |path: String, body: Value| json_req(post(&path, body.clone()).header("host", HOST), body);
    let (s, v) = call(signed_in(app.clone()), Request::builder().uri("/api/jarvis").header("host", HOST).body(Body::empty()).unwrap()).await;
    assert_eq!(s, StatusCode::OK);
    assert!(v["proposals"][0]["changes"][0]["diff"].as_str().unwrap().contains("+++ jarvis-a/spec.yaml"), "{v}");
    let (s, _) = call(signed_in(app.clone()), ui(format!("/api/jarvis/proposals/{id}/approve"), json!({"via": "voice"}))).await;
    assert_eq!(s, StatusCode::CONFLICT, "voice never approves a high-risk proposal");
    let (s, v) = call(signed_in(app.clone()), ui(format!("/api/jarvis/proposals/{id}/approve"), json!({"via": "click"}))).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["proposal"]["state"], "applied");
    let (s, v) = call(agent_router(app.clone()), as_(&jarvis, "/api/jarvis/wait", json!({"id": id, "timeout": 1}))).await;
    assert_eq!((s, v["state"].as_str()), (StatusCode::OK, Some("applied")));
    assert_eq!(call(signed_in(app.clone()), ui(format!("/api/jarvis/proposals/{id}/refuse"), json!({}))).await.0, StatusCode::CONFLICT);
}

/// A stdio MCP server runs on this machine: the approve route answers 409
/// until the person acknowledged that command, and a remote one needs none.
#[tokio::test]
async fn a_host_command_must_be_acknowledged_to_approve() {
    let answer = |a: &[String]| match a.first().map(String::as_str) {
        Some("mcp") if a.get(1).map(String::as_str) == Some("ls") => sbxw_sbx::runner::Output::ok("[]"),
        _ => crate::testing::sbx_answer(a),
    };
    let (_d, app, _fake) = crate::testing::app_with(answer);
    let ui = |path: String, body: Value| json_req(post(&path, body.clone()).header("host", HOST), body);
    let stdio = serde_json::from_value(json!([{"kind": "mcp_server", "spec": {"name": "pw", "kind": "stdio", "command": "npx", "args": ["-y", "@playwright/mcp@latest"]}}])).unwrap();
    let p = app.jarvis_propose("pw", stdio).await.unwrap();
    let (_, v) = call(signed_in(app.clone()), Request::builder().uri("/api/jarvis").header("host", HOST).body(Body::empty()).unwrap()).await;
    assert_eq!(v["proposals"][0]["needs_ack"], true);
    assert_eq!(v["proposals"][0]["changes"][0]["host_command"], "npx -y @playwright/mcp@latest");
    let (s, v) = call(signed_in(app.clone()), ui(format!("/api/jarvis/proposals/{}/approve", p.id), json!({"via": "click"}))).await;
    assert_eq!(s, StatusCode::CONFLICT, "{v}");
    assert!(v["error"].as_str().unwrap().contains("acknowledged"));
    let (s, v) = call(signed_in(app.clone()), ui(format!("/api/jarvis/proposals/{}/approve", p.id), json!({"via": "click", "ack_host_command": true}))).await;
    assert_eq!((s, v["proposal"]["state"].as_str()), (StatusCode::OK, Some("applied")), "{v}");

    let remote = serde_json::from_value(json!([{"kind": "mcp_server", "spec": {"name": "r", "kind": "remote", "url": "https://mcp.example.com/mcp"}}])).unwrap();
    let p = app.jarvis_propose("r", remote).await.unwrap();
    let (s, v) = call(signed_in(app.clone()), ui(format!("/api/jarvis/proposals/{}/approve", p.id), json!({"via": "click"}))).await;
    assert_eq!((s, v["proposal"]["state"].as_str()), (StatusCode::OK, Some("applied")), "a remote server needs no ack: {v}");
}

/// The model a session runs: reported by its status line (or a hook event
/// that names it), else what sbxw started it with; in both payloads.
#[tokio::test]
async fn a_sessions_model_reaches_sessions_and_sandboxes() {
    use sbxw_core::SandboxRef;
    let (_d, app, _fake) = crate::testing::app_with(|a| match a.first().map(String::as_str) {
        Some("ls") => sbxw_sbx::runner::Output::ok(
            &json!([
                {"name": "myapp", "agent": "claude", "status": "running"},
                {"name": "tuned", "agent": "claude", "status": "stopped"},
                {"name": "cx", "agent": "codex", "status": "running"},
            ])
            .to_string(),
        ),
        _ => crate::testing::sbx_answer(a),
    });
    let myapp = SandboxRef::parse("myapp").unwrap();
    let token = app.store.token(myapp.clone()).unwrap();
    let agent = |path: &str, body: Value| {
        json_req(post(path, body.clone()).header("authorization", format!("Bearer {token}")), body)
    };
    let get = |path: &str| Request::builder().uri(path).header("host", HOST).body(Body::empty()).unwrap();
    let session = |v: &Value, id: &str| v.as_array().unwrap().iter().find(|s| s["session_id"] == id).cloned().unwrap();
    let item = |v: &Value, name: &str| v.as_array().unwrap().iter().find(|s| s["name"] == name).cloned().unwrap();

    // Not reported yet: what sbxw starts it with, the configured default.
    let start = json!({"hook_event_name": "SessionStart", "session_id": "s1", "hook_version": 2});
    assert_eq!(call(agent_router(app.clone()), agent("/api/hook", start)).await.0, StatusCode::OK);
    let (_, v) = call(signed_in(app.clone()), get("/api/sessions")).await;
    assert_eq!((session(&v, "s1")["model"].as_str(), session(&v, "s1")["model_name"].as_str()), (Some("claude-sonnet-5-5"), Some("Sonnet 5.5")));

    // The status line says which: broadcast once, not at every render.
    let mut bus = app.bus.subscribe();
    let report = |model: &str, name: &str| {
        json!({"five_hour_pct": 12.0, "seven_day_pct": null, "session_id": "s1", "model": model, "model_name": name, "sandbox": "myapp"})
    };
    assert_eq!(call(agent_router(app.clone()), agent("/api/usage", report("claude-opus-5-5", "Opus 5.5"))).await.0, StatusCode::OK);
    match bus.try_recv() {
        Ok(crate::bus::Event::Session(i)) => assert_eq!(i.model.as_deref(), Some("claude-opus-5-5")),
        other => panic!("expected a session event, got {other:?}"),
    }
    call(agent_router(app.clone()), agent("/api/usage", report("claude-opus-5-5", "Opus 5.5"))).await;
    assert!(bus.try_recv().is_err(), "the same model again changes nothing");
    assert_eq!(app.usage().five_hour_pct, Some(12.0), "the usage reading still lands");
    let (_, v) = call(signed_in(app.clone()), get("/api/sessions")).await;
    assert_eq!(session(&v, "s1")["model_name"], "Opus 5.5");
    let (_, v) = call(signed_in(app.clone()), get("/api/sandboxes")).await;
    assert_eq!((item(&v, "myapp")["model"].as_str(), item(&v, "myapp")["model_name"].as_str()), (Some("claude-opus-5-5"), Some("Opus 5.5")));

    // `/model`: a later report carries another.
    call(agent_router(app.clone()), agent("/api/usage", report("claude-haiku-5-5", "Haiku 5.5"))).await;
    let (_, v) = call(signed_in(app.clone()), get("/api/sandboxes")).await;
    assert_eq!(item(&v, "myapp")["model"], "claude-haiku-5-5");

    // An older status line sends no model: nothing changes.
    let old = json!({"five_hour_pct": 20.0, "seven_day_pct": null, "five_hour_resets_at": null, "seven_day_resets_at": null, "sandbox": "myapp"});
    assert_eq!(call(agent_router(app.clone()), agent("/api/usage", old)).await.0, StatusCode::OK);
    let (_, v) = call(signed_in(app.clone()), get("/api/sessions")).await;
    assert_eq!(session(&v, "s1")["model"], "claude-haiku-5-5");
    // A hook event without one neither.
    call(agent_router(app.clone()), agent("/api/hook", json!({"hook_event_name": "Stop", "session_id": "s1"}))).await;
    let (_, v) = call(signed_in(app.clone()), get("/api/sessions")).await;
    assert_eq!(session(&v, "s1")["model"], "claude-haiku-5-5");

    // A thread started on a model: that one, until it says otherwise.
    app.store
        .update(myapp.clone(), |r| {
            r.threads.insert("t1".into(), crate::store::ThreadRecord { repo: ".".into(), model: Some("claude-opus-5-5".into()), same_worktree: false })
        })
        .unwrap();
    let thread = json!({"hook_event_name": "SessionStart", "session_id": "t1s", "sbxw_thread": "t1", "hook_version": 2});
    call(agent_router(app.clone()), agent("/api/hook", thread)).await;
    let (_, v) = call(signed_in(app.clone()), get("/api/sessions")).await;
    assert_eq!(session(&v, "t1s")["model"], "claude-opus-5-5");
    let (_, v) = call(signed_in(app.clone()), get("/api/sandboxes")).await;
    assert_eq!(item(&v, "myapp")["model"], "claude-haiku-5-5", "the main agent's, not the thread's");

    // A sandbox with no session: its own `claude_model`; a model outside the
    // list has no display name.
    let tuned = SandboxRef::parse("tuned").unwrap();
    app.store
        .update(tuned, |r| {
            r.config_snapshot = Some(Box::new(sbxw_core::config::Config { claude_model: "claude-next-1".into(), ..Default::default() }))
        })
        .unwrap();
    let (_, v) = call(signed_in(app.clone()), get("/api/sandboxes")).await;
    assert_eq!(item(&v, "tuned")["model"], "claude-next-1");
    assert!(item(&v, "tuned").get("model_name").is_none());
    // Another agent that reports none: left out.
    assert!(item(&v, "cx").get("model").is_none() && item(&v, "cx").get("model_name").is_none());
}

/// A session's cost: reported by its status line, on the session and summed
/// on its sandbox; a change of cost alone is held back, not broadcast.
#[tokio::test]
async fn a_sessions_cost_reaches_sessions_and_sandboxes() {
    use sbxw_core::SandboxRef;
    let (_d, app, _fake) = crate::testing::app_with(|a| match a.first().map(String::as_str) {
        Some("ls") => sbxw_sbx::runner::Output::ok(&json!([{"name": "myapp", "agent": "claude", "status": "running"}]).to_string()),
        _ => crate::testing::sbx_answer(a),
    });
    let myapp = SandboxRef::parse("myapp").unwrap();
    let token = app.store.token(myapp.clone()).unwrap();
    let agent = |path: &str, body: Value| {
        json_req(post(path, body.clone()).header("authorization", format!("Bearer {token}")), body)
    };
    let get = |path: &str| Request::builder().uri(path).header("host", HOST).body(Body::empty()).unwrap();
    let session = |v: &Value, id: &str| v.as_array().unwrap().iter().find(|s| s["session_id"] == id).cloned().unwrap();
    let item = |v: &Value| v.as_array().unwrap().iter().find(|s| s["name"] == "myapp").cloned().unwrap();
    let report = |id: &str, usd: f64| json!({"five_hour_pct": null, "seven_day_pct": null, "session_id": id, "cost_usd": usd, "duration_ms": 1000, "lines_added": 7, "lines_removed": 2, "sandbox": "myapp"});

    for id in ["s1", "s2"] {
        let start = json!({"hook_event_name": "SessionStart", "session_id": id, "hook_version": 2});
        call(agent_router(app.clone()), agent("/api/hook", start)).await;
    }
    // Before any report: absent on both.
    let (_, v) = call(signed_in(app.clone()), get("/api/sessions")).await;
    assert!(session(&v, "s1").get("cost_usd").is_none());
    let (_, v) = call(signed_in(app.clone()), get("/api/sandboxes")).await;
    assert!(item(&v).get("cost_usd").is_none());

    // The first cost of a session goes out at once; the next within 30 s
    // is read at once but not broadcast.
    let mut bus = app.bus.subscribe();
    assert_eq!(call(agent_router(app.clone()), agent("/api/usage", report("s1", 1.0))).await.0, StatusCode::OK);
    call(agent_router(app.clone()), agent("/api/usage", report("s2", 0.25))).await;
    assert!(std::iter::from_fn(|| bus.try_recv().ok()).count() > 0);
    call(agent_router(app.clone()), agent("/api/usage", report("s1", 1.5))).await;
    assert!(bus.try_recv().is_err(), "a cost-only change is coalesced");
    let (_, v) = call(signed_in(app.clone()), get("/api/sessions")).await;
    let s1 = session(&v, "s1");
    assert_eq!((s1["cost_usd"].as_f64(), s1["duration_ms"].as_u64(), s1["lines_added"].as_u64(), s1["lines_removed"].as_u64()), (Some(1.5), Some(1000), Some(7), Some(2)));
    let (_, v) = call(signed_in(app.clone()), get("/api/sandboxes")).await;
    assert_eq!(item(&v)["cost_usd"].as_f64(), Some(1.75), "both sessions");

    // A model change takes the cost along at once.
    let mut with_model = report("s1", 1.6);
    with_model["model"] = json!("claude-opus-5-5");
    call(agent_router(app.clone()), agent("/api/usage", with_model)).await;
    let costs: Vec<_> = std::iter::from_fn(|| bus.try_recv().ok()).filter_map(|e| match e {
        crate::bus::Event::Session(i) if i.session_id.as_deref() == Some("s1") => i.cost_usd,
        _ => None,
    }).collect();
    assert_eq!(costs, vec![1.6]);

    // An older script sends none: nothing changes.
    let old = json!({"five_hour_pct": 20.0, "seven_day_pct": null, "sandbox": "myapp", "session_id": "s1"});
    call(agent_router(app.clone()), agent("/api/usage", old)).await;
    assert!(bus.try_recv().is_err());
    let (_, v) = call(signed_in(app.clone()), get("/api/sessions")).await;
    assert_eq!(session(&v, "s1")["cost_usd"].as_f64(), Some(1.6));

    // A session that ends stays in its sandbox's total.
    call(agent_router(app.clone()), agent("/api/hook", json!({"hook_event_name": "SessionEnd", "session_id": "s2", "hook_version": 2}))).await;
    let (_, v) = call(signed_in(app.clone()), get("/api/sandboxes")).await;
    assert_eq!(item(&v)["cost_usd"].as_f64(), Some(1.85));
}

/// A fake `ps`: logs each run to `log`, then runs `body`.
fn fake_ps(dir: &std::path::Path, name: &str, body: &str) -> (std::path::PathBuf, std::path::PathBuf) {
    use std::os::unix::fs::PermissionsExt;
    let (script, log) = (dir.join(name), dir.join(format!("{name}.log")));
    std::fs::write(&script, format!("#!/bin/sh\necho run >> '{}'\n{body}\n", log.display())).unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    (script, log)
}

fn runs(log: &std::path::Path) -> usize {
    std::fs::read_to_string(log).map(|s| s.lines().count()).unwrap_or(0)
}

/// CPU and memory: each sandbox from its status line's `/proc` counters, the
/// daemon from `ps`; unknown is `null`, never 0.
#[tokio::test]
async fn stats_come_from_the_sandboxes_reports_and_the_daemons_cpu_time() {
    use crate::svc::stats::CpuProbe;
    use sbxw_core::SandboxRef;
    let (d, app, _fake) = crate::testing::app_with(|a| match a.first().map(String::as_str) {
        Some("ls") => sbxw_sbx::runner::Output::ok(&json!([{"name": "myapp", "agent": "claude", "status": "running"}, {"name": "cx", "agent": "codex", "status": "running"}]).to_string()),
        _ => crate::testing::sbx_answer(a),
    });
    let (ps, log) = fake_ps(d.path(), "ps-ok", "echo ' 25:00.00'");
    *app.cpu_probe.lock().unwrap() = CpuProbe::Command(ps);
    app.stats.lock().unwrap().set_running(&["myapp".into(), "cx".into()]);
    let token = app.store.token(SandboxRef::parse("myapp").unwrap()).unwrap();
    let agent = |body: Value| json_req(post("/api/usage", body.clone()).header("authorization", format!("Bearer {token}")), body);
    let get = || Request::builder().uri("/api/stats").header("host", HOST).body(Body::empty()).unwrap();
    // Fixtures recorded in an sbx sandbox: /proc/stat's first line 10 s
    // apart, and /proc/meminfo's two lines.
    let mem = "MemTotal:       12417904 kB\nMemAvailable:    8297264 kB";
    let report = |cpu: &str| json!({"five_hour_pct": null, "seven_day_pct": null, "proc_stat_cpu": cpu, "cpu_cores": 10, "proc_meminfo": mem, "sandbox": "myapp"});

    // Nothing reported, first reading of the daemon's CPU: all unknown.
    let (s, v) = call(signed_in(app.clone()), get()).await;
    assert_eq!(s, StatusCode::OK);
    assert!(v["daemon_cpu_pct"].is_null() && v["total_cpu_pct"].is_null(), "{v}");
    assert_eq!((v["sandboxes_counted"].as_u64(), v["sandboxes_unknown"].as_u64()), (Some(0), Some(2)));
    assert_eq!(runs(&log), 1);

    call(agent_router(app.clone()), agent(report("cpu  9617884 113451 1415149 177022282 68855 0 474660 0 0 0"))).await;
    call(agent_router(app.clone()), agent(report("cpu  9619884 113451 1415649 177030282 68855 0 474760 0 0 0"))).await;
    // The daemon used 1,500 s of CPU in the 2,000 s since a reading of 0:
    // 75 %, and the time this test takes moves that by a few thousandths.
    app.own_cpu.lock().unwrap().update(Some(std::time::Duration::ZERO), crate::app::now_ms() - 2_000_000);
    let (_, v) = call(signed_in(app.clone()), get()).await;
    let pct = v["daemon_cpu_pct"].as_f64().unwrap();
    assert!((70.0..=80.0).contains(&pct), "{v}");
    let myapp = v["sandboxes"].as_array().unwrap().iter().find(|s| s["name"] == "myapp").unwrap().clone();
    assert_eq!((myapp["cpu_pct"].as_f64(), myapp["cpu_cores"].as_u64(), myapp["mem_bytes"].as_u64()), (Some(245.3), Some(10), Some((12417904 - 8297264) * 1024)));
    let cx = v["sandboxes"].as_array().unwrap().iter().find(|s| s["name"] == "cx").unwrap().clone();
    assert!(cx["cpu_pct"].is_null() && cx["mem_bytes"].is_null(), "a sandbox that never reported: {cx}");
    assert_eq!((v["sandboxes_counted"].as_u64(), v["sandboxes_unknown"].as_u64()), (Some(1), Some(1)));
    assert!((v["total_cpu_pct"].as_f64().unwrap() - (pct + 245.3)).abs() < 0.11);
    // A burst of requests reads once.
    call(signed_in(app.clone()), get()).await;
    assert_eq!(runs(&log), 2);

    // `ps` failing, or too slow: unknown, never 0.
    for (name, body) in [("ps-fail", "exit 1"), ("ps-junk", "echo nope"), ("ps-slow", "sleep 5; echo 0:01.00")] {
        let (ps, _) = fake_ps(d.path(), name, body);
        *app.cpu_probe.lock().unwrap() = CpuProbe::Command(ps);
        *app.own_cpu.lock().unwrap() = Default::default();
        app.own_cpu.lock().unwrap().update(Some(std::time::Duration::ZERO), 0);
        let t = std::time::Instant::now();
        let (_, v) = call(signed_in(app.clone()), get()).await;
        assert!(v["daemon_cpu_pct"].is_null(), "{name}: {v}");
        assert!(t.elapsed() < crate::svc::stats::PROBE_TIMEOUT + std::time::Duration::from_secs(1), "{name} is bounded");
    }
}

/// The `stats` event runs only while a `/api/stream` is open.
#[tokio::test]
async fn no_stats_are_read_while_nobody_listens() {
    use crate::svc::stats::CpuProbe;
    let (d, app) = app();
    let (ps, log) = fake_ps(d.path(), "ps-count", "echo 0:00.10");
    *app.cpu_probe.lock().unwrap() = CpuProbe::Command(ps);
    let mut bus = app.bus.subscribe();
    app.start_stats_ticker();
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    assert_eq!(runs(&log), 0, "no tab: nothing read");
    assert!(bus.try_recv().is_err());

    app.bus.clients.send_modify(|n| *n += 1);
    let ev = tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            if let Ok(crate::bus::Event::Stats(s)) = bus.recv().await {
                return s;
            }
        }
    })
    .await
    .expect("a stats event once a tab is open");
    assert_eq!(crate::bus::Event::Stats(ev.clone()).name(), "stats");
    assert!(ev.daemon_cpu_pct.is_none(), "first reading");
    assert_eq!(runs(&log), 1);

    // The tab closes: after the tick in flight, nothing more.
    app.bus.clients.send_modify(|n| *n -= 1);
    tokio::time::sleep(crate::svc::stats::TICK + std::time::Duration::from_millis(500)).await;
    let after = runs(&log);
    tokio::time::sleep(crate::svc::stats::TICK + std::time::Duration::from_millis(500)).await;
    assert_eq!(runs(&log), after, "no tab: nothing read");
    assert!(after <= 2);
}

/// The real probe on this OS reads something (Linux: `/proc/self/schedstat`;
/// macOS: `ps`).
#[tokio::test]
async fn the_daemons_own_cpu_is_read_from_the_os() {
    let (_d, app) = app();
    app.own_cpu.lock().unwrap().update(Some(std::time::Duration::ZERO), crate::app::now_ms() - 2000);
    let pct = app.stats().await.daemon_cpu_pct;
    assert!(pct.is_some_and(|p| p >= 0.0), "{pct:?}");
}

/// The commit routes: limits and names checked before git runs; no
/// workspace, a folder that is not a repo: "not available", not an error;
/// a real repo: names only, never an email, and the detail found by hash.
#[tokio::test]
async fn commits_check_their_arguments_and_say_when_history_is_not_available() {
    use sbxw_core::SandboxRef;
    if std::process::Command::new("git").arg("--version").output().is_err() {
        return;
    }
    let (_d, app) = app();
    let get = |path: &str| Request::builder().uri(path).header("host", HOST).body(Body::empty()).unwrap();
    for bad in ["limit=0", "limit=201", "before=--all", "before=HEAD", "ref=--output=x", "ref=a..b", "ref=-x", "ref=$(id)"] {
        let (s, v) = call(signed_in(app.clone()), get(&format!("/api/sandboxes/web/commits?{bad}"))).await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "{bad}: {v}");
    }
    assert_eq!(call(signed_in(app.clone()), get("/api/sandboxes/web/commits/--stat")).await.0, StatusCode::BAD_REQUEST);
    assert_eq!(call(signed_in(app.clone()), get("/api/sandboxes/web/commits/HEAD")).await.0, StatusCode::BAD_REQUEST);
    // No workspace recorded.
    let (s, v) = call(signed_in(app.clone()), get("/api/sandboxes/web/commits")).await;
    assert_eq!((s, v["available"].clone()), (StatusCode::OK, json!(false)), "{v}");
    assert!(v["reason"].as_str().unwrap().contains("no workspace"));
    // A folder that is not a repository.
    let plain = tempfile::tempdir().unwrap();
    let web = SandboxRef::parse("web").unwrap();
    app.store.update(web.clone(), |r| r.workspace = Some(plain.path().to_path_buf())).unwrap();
    let (s, v) = call(signed_in(app.clone()), get("/api/sandboxes/web/commits")).await;
    assert_eq!((s, v["available"].clone()), (StatusCode::OK, json!(false)), "{v}");
    // A repository.
    let repo = tempfile::tempdir().unwrap();
    let git = |args: &[&str]| {
        let ok = std::process::Command::new("git").arg("-C").arg(repo.path()).args(["-c", "user.name=Ada Lovelace", "-c", "user.email=ada@example.org", "-c", "init.defaultBranch=main"]).args(args).output().unwrap().status.success();
        assert!(ok, "{args:?}");
    };
    git(&["init", "-q"]);
    std::fs::write(repo.path().join("a.txt"), "a\n").unwrap();
    git(&["add", "."]);
    git(&["commit", "-q", "--no-verify", "-m", "Hello"]);
    app.store.update(web.clone(), |r| r.workspace = Some(repo.path().to_path_buf())).unwrap();
    let (s, v) = call(signed_in(app.clone()), get("/api/sandboxes/web/commits?limit=5")).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["available"], true);
    let c = &v["commits"][0];
    assert_eq!((c["subject"].as_str(), c["author"].as_str(), c["merged"].as_bool(), c["branch"].as_str()), (Some("Hello"), Some("Ada Lovelace"), Some(true), Some("main")));
    assert!(!v.to_string().contains("ada@example.org"), "no email");
    let short = c["short"].as_str().unwrap().to_string();
    let (s, d) = call(signed_in(app.clone()), get(&format!("/api/sandboxes/web/commits/{short}"))).await;
    assert_eq!(s, StatusCode::OK, "{d}");
    assert_eq!(d["commit"]["files"][0]["path"], "a.txt");
    assert!(d["commit"]["files"][0]["patch"].as_str().unwrap().contains("+a"));
    assert!(!d.to_string().contains("ada@example.org"));
    assert_eq!(d["limits"]["file"], sbxw_core::commits::PATCH_FILE_MAX);
    assert_eq!(call(signed_in(app.clone()), get("/api/sandboxes/web/commits/abcdef0")).await.0, StatusCode::NOT_FOUND);
}

/// A session's history: posted by the hook of the sandbox the token proves,
/// bounded, masked before it is stored, read only on the UI listener.
/// Every text is invented; the token-shaped strings are fakes.
#[tokio::test]
async fn a_sessions_history_is_its_own_sandboxs_bounded_masked_and_ui_only() {
    use sbxw_core::SandboxRef;
    let (_d, app) = app();
    let token_of = |n: &str| app.store.token(SandboxRef::parse(n).unwrap()).unwrap();
    let (shop, other) = (token_of("shop"), token_of("other"));
    let agent = |path: &str, token: &str, body: Value| {
        json_req(post(path, body.clone()).header("authorization", format!("Bearer {token}")), body)
    };
    let get = |path: &str| Request::builder().uri(path).header("host", HOST).body(Body::empty()).unwrap();
    let fake = "ghp_FAKEfake0123456789abcdefABCDEF0000";

    let start = json!({"hook_event_name": "SessionStart", "session_id": "s1", "hook_version": 3, "history": "transcript"});
    assert_eq!(call(agent_router(app.clone()), agent("/api/hook", &shop, start)).await.0, StatusCode::OK);
    let items = json!([
        {"t": "user", "text": format!("deploy with {fake}")},
        {"t": "thinking", "text": "never"},
        {"t": "tool_use", "id": "t1", "name": "Bash", "input": {"command": "ls"}},
    ]);
    let body = json!({"session_id": "s1", "from": 0, "to": 300, "items": items, "sandbox": "other"});
    let (s, v) = call(agent_router(app.clone()), agent("/api/hook/history", &shop, body)).await;
    assert_eq!((s, v["next"].as_u64()), (StatusCode::OK, Some(300)));

    // The UI reads it, masked; never on the agent listener, never without the UI token.
    let path = "/api/sessions/shop/s1/history";
    let (s, v) = call(signed_in(app.clone()), get(path)).await;
    assert_eq!(s, StatusCode::OK);
    let all = v.to_string();
    assert!(!all.contains(fake) && all.contains("[secret]") && !all.contains("never"), "{all}");
    assert_eq!(v["page"]["items"].as_array().unwrap().len(), 3, "the start notice, the prompt, the call");
    assert_eq!(v["page"]["partial"], false);
    assert_eq!(call(agent_router(app.clone()), agent(path, &shop, json!({}))).await.0, StatusCode::NOT_FOUND);
    assert_eq!(call(crate::http::ui_router(app.clone()), get(path)).await.0, StatusCode::UNAUTHORIZED);

    // A post cannot speak for another sandbox: the "sandbox" field above was
    // ignored, and another sandbox's post for the same id is its own.
    assert_eq!(call(signed_in(app.clone()), get("/api/sessions/other/s1/history")).await.0, StatusCode::NOT_FOUND);
    let theirs = json!({"session_id": "s1", "from": 300, "to": 400, "items": [{"t": "user", "text": "hijack"}]});
    assert_eq!(call(agent_router(app.clone()), agent("/api/hook/history", &other, theirs)).await.0, StatusCode::CONFLICT);
    let (_, v) = call(signed_in(app.clone()), get(path)).await;
    assert!(!v.to_string().contains("hijack"));

    // Out of step: told where to start again.
    let dup = json!({"session_id": "s1", "from": 100, "to": 400, "items": []});
    let (s, v) = call(agent_router(app.clone()), agent("/api/hook/history", &shop, dup)).await;
    assert_eq!((s, v["next"].as_u64()), (StatusCode::CONFLICT, Some(300)));

    // Too large: by items, and by bytes.
    let many: Vec<Value> = (0..sbxw_core::history::limits::POST_ITEMS_MAX + 1).map(|_| json!({"t": "user", "text": "x"})).collect();
    let big = json!({"session_id": "s1", "from": 300, "to": 400, "items": many});
    assert_eq!(call(agent_router(app.clone()), agent("/api/hook/history", &shop, big)).await.0, StatusCode::PAYLOAD_TOO_LARGE);
    let huge = json!({"session_id": "s1", "from": 300, "to": 400, "items": [{"t": "user", "text": "x".repeat(sbxw_core::history::limits::POST_MAX_BYTES)}]});
    assert_eq!(call(agent_router(app.clone()), agent("/api/hook/history", &shop, huge)).await.0, StatusCode::PAYLOAD_TOO_LARGE);
    let (_, v) = call(signed_in(app.clone()), get(path)).await;
    assert_eq!(v["page"]["latest_seq"], 3, "nothing of the refused posts was stored");

    // The nudge: while subscribed (the GETs above), and not after the DELETE.
    let mut bus = app.bus.subscribe();
    let next = json!({"session_id": "s1", "from": 300, "to": 350, "items": [{"t": "assistant", "text": "ok"}]});
    call(agent_router(app.clone()), agent("/api/hook/history", &shop, next)).await;
    match bus.try_recv() {
        Ok(crate::bus::Event::History(n)) => {
            assert_eq!((n.sandbox.as_str(), n.session_id.as_str(), n.seq), ("shop", "s1", 4));
            assert_eq!(crate::bus::Event::History(n).data(), r#"{"sandbox":"shop","session_id":"s1","seq":4}"#);
        }
        other => panic!("expected a history nudge, got {other:?}"),
    }
    let del = Request::builder().method("DELETE").uri("/api/sessions/shop/s1/history/subscription").header("host", HOST).body(Body::empty()).unwrap();
    assert_eq!(call(signed_in(app.clone()), del).await.0, StatusCode::OK);
    let after = json!({"session_id": "s1", "from": 350, "to": 360, "items": [{"t": "assistant", "text": "again"}]});
    call(agent_router(app.clone()), agent("/api/hook/history", &shop, after)).await;
    assert!(!std::iter::from_fn(|| bus.try_recv().ok()).any(|e| matches!(e, crate::bus::Event::History(_))));

    // An old hook: events only, and the route says so.
    let old = json!({"hook_event_name": "UserPromptSubmit", "session_id": "o1", "hook_version": 2, "prompt": "hello"});
    call(agent_router(app.clone()), agent("/api/hook", &other, old)).await;
    let (_, v) = call(signed_in(app.clone()), get("/api/sessions/other/o1/history")).await;
    assert_eq!(v["page"]["partial"], true);
    assert!(v["page"]["partial_reason"].as_str().unwrap().starts_with("old_hook"));
    assert_eq!(v["page"]["items"][0]["kind"], "user_prompt");

    // A session's end starts its expiry; its sandbox's removal is not
    // exercised here (sbx rm), see the core tests for `drop_sandbox`.
    let end = json!({"hook_event_name": "SessionEnd", "session_id": "s1", "hook_version": 3, "reason": "other"});
    call(agent_router(app.clone()), agent("/api/hook", &shop, end)).await;
    let (_, v) = call(signed_in(app.clone()), get(path)).await;
    assert_eq!(v["page"]["ended"], true);
}

/// A hook of one sandbox retires nothing in another, and a restarted main
/// session is answerable again once its dead record is retired.
#[tokio::test]
async fn retiring_dead_records_stays_within_the_posting_sandbox() {
    use sbxw_core::SandboxRef;
    let (_d, app) = app();
    let token_of = |n: &str| app.store.token(SandboxRef::parse(n).unwrap()).unwrap();
    let (a, b) = (token_of("alpha"), token_of("beta"));
    let hook = |token: &str, body: Value| json_req(post("/api/hook", body.clone()).header("authorization", format!("Bearer {token}")), body);
    let start = |id: &str, proc: &str, live: &[&str]| {
        json!({"hook_event_name": "SessionStart", "session_id": id, "hook_version": 3, "agent_proc": proc, "live_procs": live,
            "ancestry": ["node", "claude"]})
    };
    call(agent_router(app.clone()), hook(&a, start("a1", "10:1", &["10:1"]))).await;
    call(agent_router(app.clone()), hook(&b, start("b1", "20:1", &[]))).await;
    let ids = |app: &std::sync::Arc<crate::app::App>| app.session_infos().iter().filter_map(|i| i.session_id.clone()).collect::<Vec<_>>();
    assert_eq!(ids(&app), vec!["a1", "b1"], "beta's empty list retires nothing of alpha's");
    call(agent_router(app.clone()), hook(&a, start("a2", "11:1", &["11:1"]))).await;
    assert_eq!(ids(&app), vec!["a2", "b1"]);
}
