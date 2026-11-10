//! The two routers, driven as a client would: the security boundaries first.

use super::{agent_router, ui_router};
use crate::testing::app;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::{json, Value};
use tower::ServiceExt;

const HOST: &str = "127.0.0.1:7681";

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
    let (s, v) = call(ui_router(app.clone()), get(HOST, None)).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["ok"], true);
    assert_eq!(call(ui_router(app.clone()), get("sbxw.localhost:7681", Some("http://sbxw.localhost:7681"))).await.0, StatusCode::OK);
    assert_eq!(call(ui_router(app.clone()), get("attacker.example:7681", None)).await.0, StatusCode::FORBIDDEN);
    assert_eq!(call(ui_router(app.clone()), get(HOST, Some("https://evil.example"))).await.0, StatusCode::FORBIDDEN);
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
    assert_eq!(call(ui_router(app), req).await.0, StatusCode::FORBIDDEN);
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
    let (s, v) = call(ui_router(app), req).await;
    assert_eq!(s, StatusCode::GONE);
    assert!(v["error"].as_str().unwrap().contains("sbxw up"));
}

#[tokio::test]
async fn failures_carry_a_status_and_the_old_envelope() {
    let (_d, app) = app();
    let body = json!({"name": "bad name!", "path": "/"});
    let req = json_req(post("/api/sandboxes/create", body.clone()).header("host", HOST), body);
    let (s, v) = call(ui_router(app), req).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_eq!(v["ok"], false);
    assert!(v["error"].as_str().unwrap().contains("letters, digits and hyphens"));
}

#[tokio::test]
async fn the_index_carries_its_versions_and_the_state_is_one_call() {
    let (_d, app) = app();
    let res = ui_router(app.clone())
        .oneshot(Request::builder().uri("/").header("host", HOST).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let html = String::from_utf8(axum::body::to_bytes(res.into_body(), 1 << 22).await.unwrap().to_vec()).unwrap();
    assert!(html.contains(&format!("const SBXW_VERSION = \"{}\"", env!("CARGO_PKG_VERSION"))));
    assert!(!html.contains("__SBX_VERSION__"));
    let (s, v) = call(ui_router(app), Request::builder().uri("/api/state").header("host", HOST).body(Body::empty()).unwrap()).await;
    assert_eq!(s, StatusCode::OK);
    for k in ["sessions", "relay", "plans", "codemap", "leads", "usage", "models"] {
        assert!(v.get(k).is_some(), "{k}");
    }
}

#[test]
fn an_upload_keeps_its_name_as_one_safe_component() {
    use super::ui::upload_name;
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
    let (s, v) = call(ui_router(app.clone()), get("/api/sandboxes/buildbox~myapp/ports")).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    assert!(v["error"].as_str().unwrap().contains("no remote host named 'buildbox'"), "{v}");
    let stop = json_req(post("/api/sandboxes/buildbox~myapp/stop", json!({})).header("host", HOST), json!({}));
    assert_eq!(call(ui_router(app.clone()), stop).await.0, StatusCode::NOT_FOUND);
    let (s, _) = call(ui_router(app.clone()), get("/api/sandboxes/a~b~c/ports")).await;
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
    assert_eq!(call(ui_router(app.clone()), req).await.0, StatusCode::OK);
    assert_eq!(*app.initial_sandbox.lock().unwrap(), "myapp", "normalised");
    let req = json_req(post("/api/initial", json!({})).header("host", HOST), json!({"sandbox": "buildbox~myapp"}));
    assert_eq!(call(ui_router(app.clone()), req).await.0, StatusCode::OK);
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
    let (s, v) = call(ui_router(app.clone()), get).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["ready"], false, "nothing is downloaded on its own");
    assert_eq!(v["compiled"], cfg!(feature = "voice"));
    let models = if sbxw_core::voice::speech_supported() { 2 } else { 1 };
    assert_eq!(v["models"].as_array().unwrap().len(), models, "the recogniser, and the voice where MLX runs");
    assert_eq!(v["speech"]["supported"], sbxw_core::voice::speech_supported());
    assert_eq!(v["hint"], "sbxw voice install");

    let (s, _) = call(ui_router(app.clone()), json_req(post("/api/voice/cancel", json!({})).header("host", HOST), json!({}))).await;
    assert_eq!(s, StatusCode::CONFLICT, "nothing to cancel");

    let pcm: Vec<u8> = [0.0f32; 1600].iter().flat_map(|f| f.to_le_bytes()).collect();
    let req = Request::builder()
        .method("POST")
        .uri("/api/voice/transcribe")
        .header("host", HOST)
        .header("content-type", "application/octet-stream")
        .body(Body::from(pcm))
        .unwrap();
    let (s, v) = call(ui_router(app.clone()), req).await;
    assert_eq!(s, StatusCode::SERVICE_UNAVAILABLE, "no build or no models: {v}");
    assert!(v["error"].as_str().unwrap().contains("voice"));

    // Anonymisation: shapes without a file, names with one, nothing through a broken one.
    let hide = |text: &str| json_req(post("/api/voice/anonymise", json!({})).header("host", HOST), json!({ "text": text }));
    let (s, v) = call(ui_router(app.clone()), hide("Jeanne Martin, a@b.ch")).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!((v["text"].as_str().unwrap(), v["masked"].as_u64().unwrap()), ("Jeanne Martin, [email]", 1));
    let file = app.store.paths.anonymise();
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, "[replace]\n\"Jeanne Martin\" = \"Personne A\"\n").unwrap();
    let (_, v) = call(ui_router(app.clone()), hide("jeanne martin, a@b.ch")).await;
    assert_eq!(v["text"], "Personne A, [email]", "the file is read at once, no restart");
    let (_, v) = call(ui_router(app.clone()), Request::builder().uri("/api/voice").header("host", HOST).body(Body::empty()).unwrap()).await;
    assert_eq!(v["anonymise"]["terms"], 1);
    std::fs::write(&file, "[remplace]\n").unwrap();
    let (s, v) = call(ui_router(app.clone()), hide("jeanne martin")).await;
    assert_eq!(s, StatusCode::SERVICE_UNAVAILABLE, "a typo in the file must not let words through: {v}");
    assert!(v["error"].as_str().unwrap().contains("anonymise.toml"));
    std::fs::remove_file(&file).unwrap();

    for gone in ["/api/voice/chat", "/api/voice/conversations"] {
        let (s, _) = call(ui_router(app.clone()), json_req(post(gone, json!({})).header("host", HOST), json!({}))).await;
        assert_eq!(s, StatusCode::NOT_FOUND, "{gone}: no language model, nothing kept");
    }

    let empty = json!({"text": "  "});
    let (s, _) = call(ui_router(app.clone()), json_req(post("/api/voice/speak", empty.clone()).header("host", HOST), empty)).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    let said = json!({"text": "Bonjour."});
    let (s, _) = call(ui_router(app.clone()), json_req(post("/api/voice/speak", said.clone()).header("host", HOST), said)).await;
    assert_eq!(s, StatusCode::SERVICE_UNAVAILABLE);
}
