//! The Team-mode board: the threads' and the lead's calls on the agent
//! listener, the UI's read on the UI listener.
//!
//! A caller names itself by `thread` (its `SBXW_THREAD`); none is the lead,
//! the sandbox's main session. The sandbox itself is proven by its token, so
//! a thread can only ever reach its own sandbox's board.

use super::error::{ok_with, ApiError, ApiResult};
use super::guard::Agent;
use crate::app::App;
use axum::extract::{Path, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use sbxw_core::board::{Kind, LEAD};
use sbxw_core::{SandboxName, ThreadName};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

type S = State<Arc<App>>;

pub fn agent_routes() -> Router<Arc<App>> {
    Router::new()
        .route("/api/board/claim", post(claim))
        .route("/api/board/release", post(release))
        .route("/api/board/extend", post(extend))
        .route("/api/board/note", post(note))
        .route("/api/board/read", post(read))
        .route("/api/board/check", post(check))
        .route("/api/board/after", post(after))
        .route("/api/board/force_release", post(force_release))
        .route("/api/board/approve_extension", post(approve_extension))
}

pub fn ui_routes() -> Router<Arc<App>> {
    Router::new().route("/api/sandboxes/:name/board", get(board))
}

#[derive(Deserialize, Default)]
struct Body {
    #[serde(default)]
    thread: Option<String>,
    #[serde(default)]
    plan: Option<String>,
    #[serde(default)]
    files: Vec<String>,
}

/// Who is calling: a thread by its name, or the lead.
fn session(raw: &Option<String>) -> Result<String, ApiError> {
    match raw.as_deref().map(str::trim).filter(|t| !t.is_empty()) {
        None => Ok(LEAD.to_string()),
        Some(t) => Ok(ThreadName::parse(t)?.to_string()),
    }
}

fn lead_only(raw: &Option<String>) -> Result<(), ApiError> {
    if session(raw)? != LEAD {
        return Err(ApiError::forbidden("only the lead may do this; ask it with SendMessage"));
    }
    Ok(())
}

async fn claim(State(app): S, Agent(me): Agent, Json(b): Json<Body>) -> ApiResult {
    let who = session(&b.thread)?;
    let out = app.board_claim(&me, b.plan.as_deref(), &who, &b.files)?;
    Ok(ok_with(json!({ "granted": out.granted, "taken": out.taken })))
}

async fn release(State(app): S, Agent(me): Agent, Json(b): Json<Body>) -> ApiResult {
    let who = session(&b.thread)?;
    let released = app.board_release(&me, b.plan.as_deref(), &who, &b.files)?;
    Ok(ok_with(json!({ "released": released })))
}

async fn extend(State(app): S, Agent(me): Agent, Json(b): Json<Body>) -> ApiResult {
    let who = session(&b.thread)?;
    let out = app.board_extend(&me, b.plan.as_deref(), &who, &b.files)?;
    Ok(ok_with(json!({ "extended": out.extended, "needs_lead": out.needs_lead, "not_held": out.not_held })))
}

#[derive(Deserialize)]
struct NoteBody {
    #[serde(default)]
    thread: Option<String>,
    #[serde(default)]
    plan: Option<String>,
    kind: Kind,
    text: String,
    #[serde(default)]
    to: Option<String>,
    #[serde(default)]
    files: Vec<String>,
}

async fn note(State(app): S, Agent(me): Agent, Json(b): Json<NoteBody>) -> ApiResult {
    let who = session(&b.thread)?;
    let to = b.to.as_deref().map(str::trim).filter(|t| !t.is_empty());
    let to = match to {
        None => None,
        Some(LEAD) => Some(LEAD.to_string()),
        Some(t) => Some(ThreadName::parse(t)?.to_string()),
    };
    let e = app.board_note(&me, b.plan.as_deref(), &who, b.kind, &b.text, to.as_deref(), &b.files)?;
    Ok(ok_with(json!({ "entry": e })))
}

async fn read(State(app): S, Agent(me): Agent) -> Json<Value> {
    Json(app.board_view(&me))
}

async fn check(State(app): S, Agent(me): Agent, Json(b): Json<Body>) -> ApiResult {
    let who = session(&b.thread)?;
    let taken = app.board_check(&me, b.plan.as_deref(), &who, &b.files)?;
    Ok(ok_with(json!({ "taken": taken })))
}

#[derive(Deserialize)]
struct CommitBody {
    hash: String,
    #[serde(default)]
    subject: String,
    #[serde(default)]
    files: Vec<String>,
}

#[derive(Deserialize)]
struct AfterBody {
    #[serde(default)]
    thread: Option<String>,
    #[serde(default)]
    plan: Option<String>,
    #[serde(default)]
    read: Vec<String>,
    #[serde(default)]
    written: Vec<String>,
    #[serde(default)]
    commit: Option<CommitBody>,
}

async fn after(State(app): S, Agent(me): Agent, Json(b): Json<AfterBody>) -> ApiResult {
    let who = session(&b.thread)?;
    let commit = b.commit.as_ref().map(|c| (c.hash.as_str(), c.subject.as_str(), c.files.as_slice()));
    let (outside, released, news) = app.board_after(&me, b.plan.as_deref(), &who, &b.read, &b.written, commit)?;
    Ok(ok_with(json!({ "outside": outside, "released": released, "news": news })))
}

#[derive(Deserialize)]
struct ForceBody {
    #[serde(default)]
    thread: Option<String>,
    files: Vec<String>,
    #[serde(default)]
    reason: String,
}

async fn force_release(State(app): S, Agent(me): Agent, Json(b): Json<ForceBody>) -> ApiResult {
    lead_only(&b.thread)?;
    let released = app.board_force_release(&me, &b.files, &b.reason)?;
    Ok(ok_with(json!({ "released": released })))
}

async fn approve_extension(State(app): S, Agent(me): Agent, Json(b): Json<Body>) -> ApiResult {
    lead_only(&b.thread)?;
    let extended = app.board_approve_extension(&me, &b.files)?;
    Ok(ok_with(json!({ "extended": extended })))
}

async fn board(State(app): S, Path(name): Path<String>) -> ApiResult {
    let name = SandboxName::parse(&name)?;
    Ok(Json(app.board_view(&name)))
}

#[cfg(test)]
mod tests {
    use crate::http::{agent_router, ui_router};
    use crate::testing::app;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use serde_json::{json, Value};
    use tower::ServiceExt;

    async fn call(router: axum::Router, req: Request<Body>) -> (StatusCode, Value) {
        let res = router.oneshot(req).await.unwrap();
        let status = res.status();
        let bytes = axum::body::to_bytes(res.into_body(), 1 << 20).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    #[tokio::test]
    async fn a_claimed_path_is_refused_to_another_thread_and_the_ui_sees_it() {
        let (_d, app) = app();
        let token = app.store.token("neos").unwrap();
        let agent = |path: &str, body: Value| {
            Request::builder()
                .method("POST")
                .uri(path)
                .header("content-type", "application/json")
                .header("authorization", format!("Bearer {token}"))
                .body(Body::from(body.to_string()))
                .unwrap()
        };
        let (s, v) = call(agent_router(app.clone()), agent("/api/board/claim", json!({"thread": "api", "plan": "p-1", "files": ["src/cart.ts"]}))).await;
        assert_eq!((s, &v["granted"]), (StatusCode::OK, &json!(["src/cart.ts"])));

        let (_, v) = call(agent_router(app.clone()), agent("/api/board/check", json!({"thread": "front", "files": ["./src/cart.ts"]}))).await;
        assert_eq!(v["taken"][0]["owner"], "api");
        let (_, v) = call(agent_router(app.clone()), agent("/api/board/check", json!({"thread": "api", "files": ["src/cart.ts"]}))).await;
        assert_eq!(v["taken"], json!([]));

        let force = json!({"thread": "front", "files": ["src/cart.ts"]});
        assert_eq!(call(agent_router(app.clone()), agent("/api/board/force_release", force)).await.0, StatusCode::FORBIDDEN);

        let get = Request::builder().uri("/api/sandboxes/neos/board").header("host", "127.0.0.1:7681").body(Body::empty()).unwrap();
        let (s, v) = call(ui_router(app.clone()), get).await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(v["plan"], "p-1");
        assert_eq!(v["claims"][0]["file"], "src/cart.ts");
        let kinds: Vec<_> = v["entries"].as_array().unwrap().iter().map(|e| e["kind"].clone()).collect();
        assert_eq!(kinds, [json!("claim"), json!("conflict")]);

        let (s, v) = call(agent_router(app.clone()), agent("/api/board/force_release", json!({"files": ["src/cart.ts"], "reason": "stuck"}))).await;
        assert_eq!((s, &v["released"]), (StatusCode::OK, &json!(["src/cart.ts"])));
    }
}
