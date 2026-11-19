use crate::http::{agent_router, ui_router};
use crate::testing::{app, signed_in};
use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::{json, Value};
use tower::ServiceExt;

const HOST: &str = "127.0.0.1:7681";

async fn call(router: axum::Router, req: Request<Body>) -> (StatusCode, Value) {
    let res = router.oneshot(req).await.unwrap();
    let status = res.status();
    let bytes = axum::body::to_bytes(res.into_body(), 64 << 20).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

fn post(path: &str, token: Option<&str>, body: Value) -> Request<Body> {
    let mut b = Request::builder().method("POST").uri(path).header("host", HOST).header("content-type", "application/json");
    if let Some(t) = token {
        b = b.header("authorization", format!("Bearer {t}"));
    }
    b.body(Body::from(body.to_string())).unwrap()
}

fn get(path: &str) -> Request<Body> {
    Request::builder().uri(path).header("host", HOST).body(Body::empty()).unwrap()
}

#[tokio::test]
async fn the_ui_writes_tickets_with_its_token_and_agents_only_read_their_own() {
    let (_d, app) = app();
    // No UI token: refused.
    assert_eq!(call(ui_router(app.clone()), get("/api/tickets")).await.0, StatusCode::UNAUTHORIZED);
    let (s, v) = call(signed_in(app.clone()), post("/api/tickets/alpha", None, json!({ "title": "Export as PDF", "description": "With the logo", "priority": "p1" }))).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["ticket"]["num"], "T-1");
    // A file as raw bytes, its name only shown.
    let req = Request::builder().method("POST").uri("/api/tickets/alpha/1/files?name=..%2F..%2F%3Cscript%3E.html").header("host", HOST).body(Body::from("<script>alert(1)</script>")).unwrap();
    let (s, v) = call(signed_in(app.clone()), req).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["ticket"]["files"][0]["name"], "<script>.html");
    let fid = v["ticket"]["files"][0]["id"].as_str().unwrap().to_string();
    // Served as plain text, never as a page.
    let res = signed_in(app.clone()).oneshot(get(&format!("/api/tickets/alpha/1/files/{fid}"))).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(res.headers()["content-type"], "text/plain; charset=utf-8");
    assert_eq!(res.headers()["x-content-type-options"], "nosniff");
    assert!(res.headers()["content-disposition"].to_str().unwrap().starts_with("attachment;"));
    assert!(res.headers()["content-security-policy"].to_str().unwrap().contains("sandbox"));
    assert_eq!(call(signed_in(app.clone()), get("/api/tickets/alpha/1/files/..%2Ftickets.json")).await.0, StatusCode::NOT_FOUND);
    assert_eq!(call(signed_in(app.clone()), get("/api/tickets/..%2F..%2Fetc/1")).await.0, StatusCode::BAD_REQUEST);
    // The board: every column, counts, labels, the card ready to draw.
    let (_, b) = call(signed_in(app.clone()), get("/api/tickets?sandbox=alpha")).await;
    let cols = b["board"]["columns"].as_array().unwrap();
    assert_eq!(cols.iter().map(|c| c["id"].as_str().unwrap()).collect::<Vec<_>>(), ["planned", "progress", "test", "ready", "merged"]);
    let card = &cols[0]["cards"][0];
    assert_eq!((card["num"].as_str(), card["column"].as_str(), card["can"]["reorder"].as_bool()), (Some("T-1"), Some("planned"), Some(true)));
    assert_eq!(card["marks"][0]["label"], "P1");
    assert!(b["board"]["counts_line"].as_str().unwrap().starts_with("1 planned"));
    // Agents: their own sandbox's list, never another's; no agent route writes.
    let alpha = app.store.token(sbxw_core::SandboxRef::parse("alpha").unwrap()).unwrap();
    let beta = app.store.token(sbxw_core::SandboxRef::parse("beta").unwrap()).unwrap();
    assert_eq!(call(agent_router(app.clone()), post("/api/tickets/list", None, json!({}))).await.0, StatusCode::UNAUTHORIZED);
    let (_, v) = call(agent_router(app.clone()), post("/api/tickets/list", Some(&alpha), json!({ "sandbox": "beta" }))).await;
    assert!(v["text"].as_str().unwrap().contains("Export as PDF"));
    let (_, v) = call(agent_router(app.clone()), post("/api/tickets/list", Some(&beta), json!({ "sandbox": "alpha" }))).await;
    assert!(!v["text"].as_str().unwrap().contains("Export"), "{v}");
    let (s, _) = call(agent_router(app.clone()), post("/api/tickets/read", Some(&beta), json!({ "id": "T-1", "sandbox": "alpha" }))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "beta has no T-1");
    for path in ["/api/tickets/alpha", "/api/tickets/alpha/1/edit", "/api/tickets/alpha/1/delete", "/api/tickets/alpha/1/move", "/api/tickets/alpha/1/send", "/api/tickets/alpha/1/files"] {
        let (s, _) = call(agent_router(app.clone()), post(path, Some(&alpha), json!({ "title": "x", "to": "up" }))).await;
        assert!(s == StatusCode::NOT_FOUND || s == StatusCode::METHOD_NOT_ALLOWED, "{path}: {s}");
    }
    assert_eq!(app.ticket_view(&sbxw_core::SandboxRef::parse("alpha").unwrap(), 1).unwrap()["title"], "Export as PDF");
    // Edit, move, delete from the UI.
    let (s, v) = call(signed_in(app.clone()), post("/api/tickets/alpha/1/edit", None, json!({ "title": "Export as PDF, A4" }))).await;
    assert_eq!((s, v["ticket"]["title"].as_str()), (StatusCode::OK, Some("Export as PDF, A4")));
    assert_eq!(call(signed_in(app.clone()), post("/api/tickets/alpha/1/move", None, json!({ "to": "top" }))).await.0, StatusCode::OK);
    let (s, v) = call(signed_in(app.clone()), post("/api/tickets/alpha/1/send", None, json!({ "text": "rm -rf /" }))).await;
    assert_eq!(s, StatusCode::CONFLICT, "no session open: {v}");
    let (s, v) = call(signed_in(app.clone()), post("/api/tickets/alpha/1/delete", None, json!({}))).await;
    assert_eq!((s, v["files"].as_u64()), (StatusCode::OK, Some(1)));
}
