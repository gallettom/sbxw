//! Canvas routes. Agents (agent listener, the sandbox is the token's): add
//! shots, list their canvases, read the remarks sent to them. The UI (UI
//! listener, UI token): everything the person does. See `docs/canvas.md`.

use super::error::{ok_with, ApiError, ApiResult};
use super::guard::Agent;
use crate::app::App;
use crate::svc::canvas::ShotIn;
use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use sbxw_core::canvas::{self, Edit};
use sbxw_core::SandboxRef;
use serde::Deserialize;
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;

type S = State<Arc<App>>;

/// A shot travels as base64 in JSON: 4/3 of the picture, and room for its texts.
const SHOT_BODY_MAX: usize = canvas::MAX_IMAGE_BYTES / 3 * 4 + (64 << 10);
/// An asset travels as raw bytes.
const ASSET_BODY_MAX: usize = canvas::MAX_IMAGE_BYTES + 1024;

pub fn agent_routes() -> Router<Arc<App>> {
    Router::new()
        .route("/api/canvas/shots", post(agent_add).layer(DefaultBodyLimit::max(SHOT_BODY_MAX)))
        .route("/api/canvas/list", post(agent_list))
        .route("/api/canvas/read", post(agent_read))
}

pub fn ui_routes() -> Router<Arc<App>> {
    Router::new()
        .route("/api/canvas", get(list).post(create))
        .route("/api/canvas/delete", post(delete))
        .route("/api/canvas/:id", get(manifest))
        .route("/api/canvas/:id/images/:file", get(image))
        .route("/api/canvas/:id/rename", post(rename))
        .route("/api/canvas/:id/edit", post(edit))
        .route("/api/canvas/:id/assets", post(asset).layer(DefaultBodyLimit::max(ASSET_BODY_MAX)))
        .route("/api/canvas/:id/submit", post(submit))
}

fn checked(id: &str) -> Result<(), ApiError> {
    if canvas::valid_id(id) {
        Ok(())
    } else {
        Err(ApiError::bad("not a canvas id"))
    }
}

// ── Agents ────────────────────────────────────────────────────────────────

#[derive(Deserialize, Default)]
#[serde(default)]
struct AddBody {
    canvas: Option<String>,
    /// A new canvas's title.
    title: String,
    /// The tool call's id: its shots form one round.
    batch: String,
    shot: ShotIn,
}

async fn agent_add(State(app): S, Agent(me): Agent, Json(b): Json<AddBody>) -> ApiResult {
    let batch = canvas::clean(&b.batch, 40, false);
    let a = app.canvas_agent_add(&me, b.canvas.as_deref(), &b.title, &batch, b.shot)?;
    Ok(ok_with(json!({ "canvas": a.canvas, "title": a.title, "round": a.round, "shot": a.shot })))
}

async fn agent_list(State(app): S, Agent(me): Agent) -> ApiResult {
    Ok(ok_with(json!({ "canvases": app.canvas_agent_list(&me) })))
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct ReadBody {
    canvas: Option<String>,
}

async fn agent_read(State(app): S, Agent(me): Agent, Json(b): Json<ReadBody>) -> ApiResult {
    let text = app.canvas_agent_read(&me, b.canvas.as_deref())?;
    Ok(ok_with(json!({ "text": text })))
}

// ── The UI ────────────────────────────────────────────────────────────────

async fn list(State(app): S) -> ApiResult {
    let (canvases, total) = app.canvas_list();
    Ok(ok_with(json!({ "canvases": canvases, "total_bytes": total, "max_canvas_bytes": canvas::MAX_CANVAS_BYTES })))
}

async fn manifest(State(app): S, Path(id): Path<String>) -> ApiResult {
    checked(&id)?;
    let c = app.canvas_get(&id).ok_or_else(|| ApiError::not_found("no such canvas"))?;
    let bytes = c.bytes();
    Ok(ok_with(json!({ "canvas": c, "bytes": bytes })))
}

async fn image(State(app): S, Path((id, file)): Path<(String, String)>) -> Result<Response, ApiError> {
    checked(&id)?;
    let (bytes, mime) = app.canvas_image(&id, &file).map_err(ApiError::not_found)?;
    Ok((
        StatusCode::OK,
        [(header::CONTENT_TYPE, mime), (header::CACHE_CONTROL, "private, max-age=31536000, immutable"), (header::X_CONTENT_TYPE_OPTIONS, "nosniff")],
        bytes,
    )
        .into_response())
}

#[derive(Deserialize)]
struct CreateBody {
    sandbox: String,
    #[serde(default)]
    title: String,
}

async fn create(State(app): S, Json(b): Json<CreateBody>) -> ApiResult {
    let sb = SandboxRef::parse(&b.sandbox)?;
    let c = app.canvas_create(&sb, if b.title.trim().is_empty() { "Review" } else { &b.title })?;
    Ok(ok_with(json!({ "canvas": c })))
}

#[derive(Deserialize)]
struct RenameBody {
    title: String,
}

async fn rename(State(app): S, Path(id): Path<String>, Json(b): Json<RenameBody>) -> ApiResult {
    checked(&id)?;
    let c = app.canvas_rename(&id, &b.title)?;
    Ok(ok_with(json!({ "title": c.title })))
}

async fn edit(State(app): S, Path(id): Path<String>, Json(e): Json<Edit>) -> ApiResult {
    checked(&id)?;
    let c = app.canvas_edit(&id, e)?;
    Ok(ok_with(json!({ "updated_ms": c.updated_ms, "draft": c.draft() })))
}

/// `?name=<the file's name, shown only>&x=&y=`; the body is the picture.
async fn asset(State(app): S, Path(id): Path<String>, Query(q): Query<HashMap<String, String>>, body: Bytes) -> ApiResult {
    checked(&id)?;
    let num = |k: &str| q.get(k).and_then(|v| v.parse::<f32>().ok()).filter(|v| v.is_finite());
    let at = num("x").zip(num("y"));
    let c = app.canvas_add_asset(&id, q.get("name").map(String::as_str).unwrap_or("picture"), &body, at)?;
    let a = c.assets.last().cloned();
    Ok(ok_with(json!({ "asset": a })))
}

#[derive(Deserialize)]
struct SubmitBody {
    /// The session it goes to, as shown (for the history).
    #[serde(default)]
    session: String,
    /// Each sent asset's id and its path in the sandbox (the upload's answer).
    #[serde(default)]
    assets: HashMap<String, String>,
}

async fn submit(State(app): S, Path(id): Path<String>, Json(b): Json<SubmitBody>) -> ApiResult {
    checked(&id)?;
    let paths: Vec<(String, String)> = b.assets.into_iter().collect();
    let (n, c) = app.canvas_submit(&id, &b.session, &paths)?;
    Ok(ok_with(json!({ "review": n, "line": canvas::ready_line(&c.id, n) })))
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct DeleteBody {
    ids: Vec<String>,
    /// Every canvas last changed before this time (ms since 1970).
    before_ms: Option<u64>,
}

async fn delete(State(app): S, Json(b): Json<DeleteBody>) -> ApiResult {
    let mut ids: Vec<String> = b.ids.into_iter().filter(|i| canvas::valid_id(i)).collect();
    if let Some(t) = b.before_ms {
        ids.extend(app.canvas_older_than(t));
    }
    if ids.is_empty() {
        return Err(ApiError::bad("no canvas named"));
    }
    let (n, bytes) = app.canvas_delete(&ids);
    Ok(ok_with(json!({ "deleted": n, "bytes": bytes })))
}

#[cfg(test)]
mod tests {
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

    fn png() -> String {
        let mut b = b"\x89PNG\r\n\x1a\n".to_vec();
        b.extend(13u32.to_be_bytes());
        b.extend(b"IHDR");
        b.extend(390u32.to_be_bytes());
        b.extend(844u32.to_be_bytes());
        b.extend([8, 6, 0, 0, 0, 0, 0, 0, 0]);
        b.extend(0u32.to_be_bytes());
        b.extend(b"IDAT");
        sbxw_core::mac::base64(&b)
    }

    #[tokio::test]
    async fn agents_reach_their_own_canvases_only_and_the_ui_needs_its_token() {
        let (_d, app) = app();
        let alpha = app.store.token(sbxw_core::SandboxRef::parse("alpha").unwrap()).unwrap();
        let beta = app.store.token(sbxw_core::SandboxRef::parse("beta").unwrap()).unwrap();
        let shot = json!({ "batch": "b1", "shot": { "b64": png(), "title": "Phone home", "width": 390 }, "sandbox": "beta" });
        // No token: refused.
        assert_eq!(call(agent_router(app.clone()), post("/api/canvas/shots", None, shot.clone())).await.0, StatusCode::UNAUTHORIZED);
        // A `sandbox` field in the body is ignored: the token decides.
        let (s, v) = call(agent_router(app.clone()), post("/api/canvas/shots", Some(&alpha), shot.clone())).await;
        assert_eq!(s, StatusCode::OK, "{v}");
        let id = v["canvas"].as_str().unwrap().to_string();
        assert_eq!(app.canvas_get(&id).unwrap().sandbox, "alpha");
        // Beta can neither add to it, read it nor list it.
        let mut other = shot.clone();
        other["canvas"] = json!(id);
        assert_eq!(call(agent_router(app.clone()), post("/api/canvas/shots", Some(&beta), other)).await.0, StatusCode::BAD_REQUEST);
        assert_eq!(call(agent_router(app.clone()), post("/api/canvas/read", Some(&beta), json!({ "canvas": id }))).await.0, StatusCode::BAD_REQUEST);
        let (_, v) = call(agent_router(app.clone()), post("/api/canvas/list", Some(&beta), json!({}))).await;
        assert_eq!(v["canvases"], json!([]));
        // No agent route deletes, edits or submits.
        for path in ["/api/canvas/delete", &format!("/api/canvas/{id}/edit"), &format!("/api/canvas/{id}/submit")] {
            let (s, _) = call(agent_router(app.clone()), post(path, Some(&alpha), json!({ "ids": [id] }))).await;
            assert!(s == StatusCode::NOT_FOUND || s == StatusCode::METHOD_NOT_ALLOWED, "{path}: {s}");
        }
        assert!(app.canvas_get(&id).is_some());
        // The UI: nothing without its token; with it, the list, the manifest, the picture.
        assert_eq!(call(ui_router(app.clone()), get("/api/canvas")).await.0, StatusCode::UNAUTHORIZED);
        let (s, v) = call(signed_in(app.clone()), get("/api/canvas")).await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(v["canvases"][0]["shots"], 1);
        let (_, m) = call(signed_in(app.clone()), get(&format!("/api/canvas/{id}"))).await;
        let file = m["canvas"]["shots"][0]["file"].as_str().unwrap().to_string();
        let res = signed_in(app.clone()).oneshot(get(&format!("/api/canvas/{id}/images/{file}"))).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(res.headers()["content-type"], "image/png");
        assert_eq!(call(signed_in(app.clone()), get(&format!("/api/canvas/{id}/images/..%2Fcanvas.json"))).await.0, StatusCode::NOT_FOUND);
        assert_eq!(call(signed_in(app.clone()), get("/api/canvas/..%2F..%2Fetc/images/passwd")).await.0, StatusCode::BAD_REQUEST);
        // Delete from the UI.
        let (s, v) = call(signed_in(app.clone()), post("/api/canvas/delete", None, json!({ "ids": [id] }))).await;
        assert_eq!((s, v["deleted"].clone()), (StatusCode::OK, json!(1)));
    }

    #[tokio::test]
    async fn the_person_adds_an_asset_pins_and_submits_then_the_agent_reads() {
        let (_d, app) = app();
        let alpha = app.store.token(sbxw_core::SandboxRef::parse("alpha").unwrap()).unwrap();
        let (_, v) = call(agent_router(app.clone()), post("/api/canvas/shots", Some(&alpha), json!({ "title": "Mobile review", "batch": "b", "shot": { "b64": png(), "title": "Home", "width": 390 } }))).await;
        let id = v["canvas"].as_str().unwrap().to_string();
        let shot = app.canvas_get(&id).unwrap().shots[0].id.clone();
        // An asset as raw bytes.
        let raw = sbxw_core::canvas::unbase64(&png(), 1 << 20).unwrap();
        let req = Request::builder().method("POST").uri(format!("/api/canvas/{id}/assets?name=..%2F..%2Fevil.png&x=500&y=0")).header("host", HOST).body(Body::from(raw)).unwrap();
        let (s, v) = call(signed_in(app.clone()), req).await;
        assert_eq!(s, StatusCode::OK, "{v}");
        let asset = v["asset"]["id"].as_str().unwrap().to_string();
        assert_eq!(v["asset"]["name"], "../../evil.png", "a name is shown, never used as a path");
        assert!(sbxw_core::canvas::valid_file(v["asset"]["file"].as_str().unwrap()));
        let edit = json!({ "pins": [{ "n": 1, "on": shot, "fx": 0.5, "fy": 0.9, "text": "Bigger tap target" }] });
        assert_eq!(call(signed_in(app.clone()), post(&format!("/api/canvas/{id}/edit"), None, edit)).await.0, StatusCode::OK);
        let (s, v) = call(signed_in(app.clone()), post(&format!("/api/canvas/{id}/submit"), None, json!({ "session": "alpha · main", "assets": { asset: "/tmp/sbxw-uploads/1/evil.png" } }))).await;
        assert_eq!(s, StatusCode::OK, "{v}");
        assert_eq!(v["review"], 1);
        assert!(v["line"].as_str().unwrap().contains("canvas_read"));
        let (_, v) = call(agent_router(app.clone()), post("/api/canvas/read", Some(&alpha), json!({}))).await;
        let t = v["text"].as_str().unwrap();
        assert!(t.contains("Pin 1 on shot \"Home\"") && t.contains("> Bigger tap target") && t.contains("/tmp/sbxw-uploads/1/evil.png"), "{t}");
    }
}
