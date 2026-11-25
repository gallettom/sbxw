//! Canvas routes. Agents (agent listener, the sandbox is the token's): add
//! shots and diagrams, list their canvases, read the remarks and diagram
//! edits sent to them. The UI (UI listener, UI token): everything the person
//! does. See `docs/canvas.md`, `docs/relay.md` and `desktop/docs/daemon-api.md`.

use super::error::{ok_with, ApiError, ApiResult};
use super::guard::Agent;
use crate::app::App;
use crate::svc::canvas::{DiagramIn, DiagramSaveError, ShotIn};
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
/// A diagram travels as a JSON string inside JSON: escaping can double it.
const DIAGRAM_BODY_MAX: usize = sbxw_core::diagram::MAX_SOURCE_BYTES * 2 + (16 << 10);

pub fn agent_routes() -> Router<Arc<App>> {
    Router::new()
        .route("/api/canvas/shots", post(agent_add).layer(DefaultBodyLimit::max(SHOT_BODY_MAX)))
        .route("/api/canvas/list", post(agent_list))
        .route("/api/canvas/read", post(agent_read))
        .route("/api/canvas/diagrams", post(agent_diagram).layer(DefaultBodyLimit::max(DIAGRAM_BODY_MAX)))
}

pub fn ui_routes() -> Router<Arc<App>> {
    Router::new()
        .route("/api/canvas", get(list).post(create))
        .route("/api/canvas/delete", post(delete))
        .route("/api/canvas/ask", post(ask))
        .route("/api/canvas/:id/ask/cancel", post(ask_cancel))
        .route("/api/canvas/:id", get(manifest))
        .route("/api/canvas/:id/images/:file", get(image))
        .route("/api/canvas/:id/rename", post(rename))
        .route("/api/canvas/:id/edit", post(edit))
        .route("/api/canvas/:id/assets", post(asset).layer(DefaultBodyLimit::max(ASSET_BODY_MAX)))
        .route("/api/canvas/:id/submit", post(submit))
        .route("/api/canvas/:id/diagrams/:did", get(diagram).post(save_diagram).layer(DefaultBodyLimit::max(DIAGRAM_BODY_MAX)))
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

#[derive(Deserialize, Default)]
#[serde(default)]
struct DiagramBody {
    canvas: Option<String>,
    /// The tool call's id (each call is one round).
    batch: String,
    diagram: DiagramIn,
}

async fn agent_diagram(State(app): S, Agent(me): Agent, Json(b): Json<DiagramBody>) -> ApiResult {
    let batch = canvas::clean(&b.batch, 40, false);
    let a = app.canvas_agent_diagram(&me, b.canvas.as_deref(), &batch, b.diagram)?;
    Ok(ok_with(json!(a)))
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
    let r = app.canvas_agent_read_all(&me, b.canvas.as_deref())?;
    let diagrams: Vec<_> = r.diagrams.into_iter().map(|(c, d)| json!({ "canvas": c, "id": d.id, "title": d.title, "rev": d.rev, "lines": d.lines, "json": d.json })).collect();
    Ok(ok_with(json!({ "text": r.text, "diagrams": diagrams })))
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
    let diagram_bytes = c.diagram_bytes();
    Ok(ok_with(json!({ "canvas": c.ui_view(), "bytes": bytes, "diagram_bytes": diagram_bytes })))
}

async fn diagram(State(app): S, Path((id, did)): Path<(String, String)>) -> ApiResult {
    checked(&id)?;
    let d = app.canvas_diagram(&id, &did).ok_or_else(|| ApiError::not_found("no such diagram"))?;
    Ok(ok_with(json!({ "diagram": d })))
}

#[derive(Deserialize)]
struct SaveDiagramBody {
    /// The revision the editor started from (`diagram.rev` as read).
    rev: u32,
    /// The whole diagram, sbxw's schema.
    diagram: serde_json::Value,
}

async fn save_diagram(State(app): S, Path((id, did)): Path<(String, String)>, Json(b): Json<SaveDiagramBody>) -> ApiResult {
    checked(&id)?;
    let json = b.diagram.to_string();
    match app.canvas_save_diagram(&id, &did, b.rev, &json) {
        Ok(d) => Ok(ok_with(json!({ "diagram": d, "rev": d.diagram.rev }))),
        Err(DiagramSaveError::Stale(n)) => Err(ApiError::conflict(format!("the diagram changed since revision {} (it is at {n}): read it again", b.rev))),
        Err(DiagramSaveError::NotFound) => Err(ApiError::not_found("no such diagram")),
        Err(DiagramSaveError::Bad(e)) => Err(ApiError::bad(e)),
    }
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
struct AskBody {
    /// The canvas asked from; none: a new one for `sandbox`.
    #[serde(default)]
    canvas: Option<String>,
    sandbox: String,
    #[serde(default)]
    thread: Option<String>,
    text: String,
}

/// The person asks a session for screenshots from a canvas: the prompt (with
/// the canvas's id under it) is typed into the session, then the canvas waits
/// for the shots and takes the agent's unnamed ones (`canvas_agent_add`).
async fn ask(State(app): S, Json(b): Json<AskBody>) -> ApiResult {
    let sb = SandboxRef::parse(&b.sandbox)?;
    let thread = b.thread.filter(|t| !t.trim().is_empty()).map(|t| sbxw_core::ThreadName::parse(&t)).transpose()?;
    let text = b.text.trim();
    if text.is_empty() {
        return Err(ApiError::bad("empty prompt"));
    }
    let (id, created) = match b.canvas.filter(|c| !c.is_empty()) {
        Some(id) => {
            checked(&id)?;
            let c = app.canvas_get(&id).ok_or_else(|| ApiError::not_found("no such canvas"))?;
            if c.sandbox != sb.to_string() {
                return Err(ApiError::bad("that canvas belongs to another sandbox"));
            }
            (id, false)
        }
        None => (app.canvas_create(&sb, "Screenshots")?.id, true),
    };
    let to = match &thread {
        Some(t) => format!("{sb} · {t}"),
        None => sb.to_string(),
    };
    let prompt = format!("{text}\n\n{}", canvas::ask_line(&id));
    if let Err(e) = app.chat_push(&prompt, Some(sb), false, false, thread).await {
        if created {
            app.canvas_delete(std::slice::from_ref(&id));
        }
        return Err(ApiError::bad(e));
    }
    let c = app.canvas_ask(&id, &to)?;
    Ok(ok_with(json!({ "canvas": c.id, "created": created, "ask": c.ask })))
}

async fn ask_cancel(State(app): S, Path(id): Path<String>) -> ApiResult {
    checked(&id)?;
    app.canvas_ask_cancel(&id)?;
    Ok(ok_with(json!({})))
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

    #[tokio::test]
    async fn diagrams_go_in_by_the_agent_and_are_edited_by_the_person() {
        let (_d, app) = app();
        let alpha = app.store.token(sbxw_core::SandboxRef::parse("alpha").unwrap()).unwrap();
        let beta = app.store.token(sbxw_core::SandboxRef::parse("beta").unwrap()).unwrap();
        let flow = r#"{ "version": 1, "kind": "workflow", "title": "Release", "nodes": [{ "id": "push", "label": "Push" }, { "id": "test", "label": "Test" }], "edges": [{ "from": "push", "to": "test" }] }"#;
        let body = json!({ "batch": "k1", "sandbox": "beta", "diagram": { "json": flow, "kind": "workflow" } });
        assert_eq!(call(agent_router(app.clone()), post("/api/canvas/diagrams", None, body.clone())).await.0, StatusCode::UNAUTHORIZED);
        let (s, v) = call(agent_router(app.clone()), post("/api/canvas/diagrams", Some(&alpha), body.clone())).await;
        assert_eq!(s, StatusCode::OK, "{v}");
        let (id, did) = (v["canvas"].as_str().unwrap().to_string(), v["diagram"].as_str().unwrap().to_string());
        assert_eq!(app.canvas_get(&id).unwrap().sandbox, "alpha", "the token decides");
        assert_eq!((v["kind"].clone(), v["rev"].clone(), v["nodes"].clone()), (json!("workflow"), json!(1), json!(2)));
        // Beta cannot add to alpha's canvas; a wrong kind is refused.
        let mut other = body.clone();
        other["canvas"] = json!(id);
        assert_eq!(call(agent_router(app.clone()), post("/api/canvas/diagrams", Some(&beta), other)).await.0, StatusCode::BAD_REQUEST);
        let wrong = json!({ "batch": "k2", "diagram": { "json": flow, "kind": "sequence" } });
        let (s, v) = call(agent_router(app.clone()), post("/api/canvas/diagrams", Some(&alpha), wrong)).await;
        assert_eq!(s, StatusCode::BAD_REQUEST);
        assert!(v["error"].as_str().unwrap().contains("not a sequence"));
        // Agents have no UI diagram route.
        assert!(matches!(call(agent_router(app.clone()), get(&format!("/api/canvas/{id}/diagrams/{did}"))).await.0, StatusCode::NOT_FOUND | StatusCode::UNAUTHORIZED | StatusCode::METHOD_NOT_ALLOWED));
        // The UI: the manifest lists it (no base copy), the diagram reads, a save needs the revision.
        assert_eq!(call(ui_router(app.clone()), get(&format!("/api/canvas/{id}/diagrams/{did}"))).await.0, StatusCode::UNAUTHORIZED);
        let (_, m) = call(signed_in(app.clone()), get(&format!("/api/canvas/{id}"))).await;
        assert_eq!(m["canvas"]["diagrams"][0]["id"], json!(did));
        assert!(m["diagram_bytes"].as_u64().unwrap() > 0);
        let (s, g) = call(signed_in(app.clone()), get(&format!("/api/canvas/{id}/diagrams/{did}"))).await;
        assert_eq!(s, StatusCode::OK);
        let mut d = g["diagram"]["diagram"].clone();
        d["nodes"][1]["label"] = json!("Lead\u{202e} review");
        let (s, v) = call(signed_in(app.clone()), post(&format!("/api/canvas/{id}/diagrams/{did}"), None, json!({ "rev": 1, "diagram": d }))).await;
        assert_eq!(s, StatusCode::OK, "{v}");
        assert_eq!(v["rev"], 2);
        assert_eq!(v["diagram"]["diagram"]["nodes"][1]["label"], "Lead review", "cleaned");
        let (s, v) = call(signed_in(app.clone()), post(&format!("/api/canvas/{id}/diagrams/{did}"), None, json!({ "rev": 1, "diagram": d }))).await;
        assert_eq!(s, StatusCode::CONFLICT, "{v}");
        assert_eq!(call(signed_in(app.clone()), get(&format!("/api/canvas/{id}/diagrams/d-..%2F..%2Fxx"))).await.0, StatusCode::NOT_FOUND);
        let (_, m) = call(signed_in(app.clone()), get(&format!("/api/canvas/{id}"))).await;
        assert!(m["canvas"]["diagrams"][0].get("base").is_none(), "{}", m["canvas"]["diagrams"][0]);
        // Sent with a round; the agent reads it once.
        assert_eq!(call(signed_in(app.clone()), post(&format!("/api/canvas/{id}/submit"), None, json!({}))).await.0, StatusCode::OK);
        let (_, v) = call(agent_router(app.clone()), post("/api/canvas/read", Some(&alpha), json!({}))).await;
        assert_eq!(v["diagrams"][0]["id"], json!(did));
        assert_eq!(v["diagrams"][0]["canvas"], json!(id));
        assert!(v["diagrams"][0]["lines"].as_array().unwrap().iter().any(|l| l.as_str().unwrap().contains("Lead review")));
        assert!(v["diagrams"][0]["json"].as_str().unwrap().contains("Lead review"));
        let (_, v) = call(agent_router(app.clone()), post("/api/canvas/read", Some(&alpha), json!({}))).await;
        assert_eq!(v["diagrams"], json!([]));
    }


    #[tokio::test]
    async fn asking_from_a_canvas_checks_it_and_leaves_nothing_when_the_prompt_cannot_go() {
        let (_d, app) = app();
        let web = sbxw_core::SandboxRef::parse("web").unwrap();
        let id = app.canvas_create(&web, "Review").unwrap().id;
        let other = app.canvas_create(&sbxw_core::SandboxRef::parse("docs").unwrap(), "Docs").unwrap().id;
        let ask = |b: Value| post("/api/canvas/ask", None, b);
        assert_eq!(call(ui_router(app.clone()), ask(json!({ "canvas": id, "sandbox": "web", "text": "Shots" }))).await.0, StatusCode::UNAUTHORIZED);
        let (s, _) = call(signed_in(app.clone()), ask(json!({ "canvas": id, "sandbox": "web", "text": "  " }))).await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "an empty prompt");
        let (s, _) = call(signed_in(app.clone()), ask(json!({ "canvas": other, "sandbox": "web", "text": "Shots" }))).await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "another sandbox's canvas");
        // No session to type into here: refused, the canvas does not wait, no new canvas is left.
        let before = app.canvas_list().0.len();
        let (s, _) = call(signed_in(app.clone()), ask(json!({ "canvas": id, "sandbox": "web", "text": "Shots" }))).await;
        assert_eq!(s, StatusCode::BAD_REQUEST);
        assert!(app.canvas_get(&id).unwrap().ask.is_none());
        let (s, _) = call(signed_in(app.clone()), ask(json!({ "sandbox": "web", "text": "Shots" }))).await;
        assert_eq!(s, StatusCode::BAD_REQUEST);
        assert_eq!(app.canvas_list().0.len(), before, "the canvas made for the request is gone again");
        // Cancel: the canvas stops waiting.
        app.canvas_ask(&id, "web").unwrap();
        let (s, _) = call(signed_in(app.clone()), post(&format!("/api/canvas/{id}/ask/cancel"), None, json!({}))).await;
        assert_eq!(s, StatusCode::OK);
        assert!(app.canvas_get(&id).unwrap().ask.is_none());
        assert_eq!(call(signed_in(app.clone()), post("/api/canvas/x/ask/cancel", None, json!({}))).await.0, StatusCode::BAD_REQUEST);
    }
}
