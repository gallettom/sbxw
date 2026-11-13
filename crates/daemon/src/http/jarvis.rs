//! Jarvis's routes: its proposals on the agent listener, answered for the
//! `sbxw-jarvis` sandbox alone, and the person's half on the UI listener.

use super::error::{ok_with, ApiError, ApiResult};
use super::guard::Agent;
use crate::app::App;
use axum::extract::{Path, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use sbxw_core::jarvis::{self, ApproveError, Change, Via};
use sbxw_core::SandboxRef;
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

type S = State<Arc<App>>;

pub fn agent_routes() -> Router<Arc<App>> {
    Router::new()
        .route("/api/jarvis/state", post(state))
        .route("/api/jarvis/propose", post(propose))
        .route("/api/jarvis/wait", post(wait))
}

pub fn ui_routes() -> Router<Arc<App>> {
    Router::new()
        .route("/api/jarvis", get(list))
        .route("/api/jarvis/status", get(status))
        .route("/api/jarvis/start", post(start))
        .route("/api/jarvis/proposals/:id/approve", post(approve))
        .route("/api/jarvis/proposals/:id/refuse", post(refuse))
}

/// Every agent route: the daemon's own Jarvis sandbox, or 403.
fn only_jarvis(app: &App, me: &SandboxRef) -> Result<(), ApiError> {
    if app.is_jarvis(me) {
        return Ok(());
    }
    Err(ApiError::forbidden(format!("only sbxw's own '{}' sandbox may propose configuration changes", jarvis::SANDBOX)))
}

fn approve_error(e: ApproveError) -> ApiError {
    match e {
        ApproveError::NotFound(m) => ApiError::not_found(m),
        ApproveError::Conflict(m) => ApiError::conflict(m),
    }
}

async fn state(State(app): S, Agent(me): Agent) -> ApiResult {
    only_jarvis(&app, &me)?;
    Ok(Json(app.jarvis_state().await))
}

#[derive(Deserialize)]
struct ProposeBody {
    #[serde(default)]
    summary: String,
    changes: Vec<Value>,
    #[serde(default)]
    timeout: Option<u64>,
}

/// The body read only once the caller is known to be Jarvis: anyone else
/// gets 403, whatever they sent.
fn body<T: serde::de::DeserializeOwned>(v: Value) -> Result<T, ApiError> {
    serde_json::from_value(v).map_err(|e| ApiError::bad(format!("bad request: {e}")))
}

async fn propose(State(app): S, Agent(me): Agent, Json(v): Json<Value>) -> ApiResult {
    only_jarvis(&app, &me)?;
    let b: ProposeBody = body(v)?;
    // Each change parsed on its own, so the error names the one to fix.
    let changes = b
        .changes
        .into_iter()
        .enumerate()
        .map(|(i, c)| serde_json::from_value::<Change>(c).map_err(|e| format!("change {}: {e}", i + 1)))
        .collect::<Result<Vec<_>, _>>()?;
    let p = app.jarvis_propose(&b.summary, changes).await?;
    let p = app.jarvis_wait(&p.id, jarvis::wait_bound(b.timeout)).await?;
    Ok(ok_with(p.agent_view()))
}

#[derive(Deserialize)]
struct WaitBody {
    id: String,
    #[serde(default)]
    timeout: Option<u64>,
}

async fn wait(State(app): S, Agent(me): Agent, Json(v): Json<Value>) -> ApiResult {
    only_jarvis(&app, &me)?;
    let b: WaitBody = body(v)?;
    let p = app.jarvis_wait(&b.id, jarvis::wait_bound(b.timeout)).await.map_err(ApiError::not_found)?;
    Ok(ok_with(p.agent_view()))
}

async fn list(State(app): S) -> Json<Value> {
    let proposals: Vec<Value> = app.jarvis_list().iter().rev().map(|p| p.ui_view()).collect();
    Json(json!({ "ok": true, "proposals": proposals }))
}

async fn status(State(app): S) -> ApiResult {
    Ok(Json(serde_json::to_value(app.jarvis_status().await).unwrap_or_default()))
}

async fn start(State(app): S) -> ApiResult {
    Ok(Json(serde_json::to_value(app.jarvis_start().await).unwrap_or_default()))
}

#[derive(Deserialize)]
struct ApproveBody {
    via: Via,
}

async fn approve(State(app): S, Path(id): Path<String>, Json(b): Json<ApproveBody>) -> ApiResult {
    let p = app.jarvis_approve(&id, b.via).await.map_err(approve_error)?;
    Ok(ok_with(json!({ "proposal": p.ui_view() })))
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RefuseBody {
    reason: Option<String>,
}

async fn refuse(State(app): S, Path(id): Path<String>, body: Option<Json<RefuseBody>>) -> ApiResult {
    let reason = body.and_then(|Json(b)| b.reason);
    let p = app.jarvis_refuse(&id, reason).map_err(approve_error)?;
    Ok(ok_with(json!({ "proposal": p.ui_view() })))
}
