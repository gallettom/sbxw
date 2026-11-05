//! The virtual Mac's two halves: what a sandbox may call (agent listener),
//! and what the person drives from the UI and the CLI (UI listener).
//!
//! Installing and booting take minutes — a first boot downloads macOS — so
//! the UI's calls start them and return; the panel follows `busy` in
//! `GET /api/mac` and the `mac` event.

use super::error::{ok_with, ApiError, ApiResult};
use super::guard::Agent;
use crate::app::App;
use crate::svc::relay::wait_bound;
use axum::extract::State;
use axum::routing::{get, post};
use axum::{Json, Router};
use sbxw_core::ThreadName;
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

type S = State<Arc<App>>;

pub fn agent_routes() -> Router<Arc<App>> {
    Router::new()
        .route("/api/mac/start", post(start))
        .route("/api/mac/open_port", post(open_port))
        .route("/api/mac/action", post(action))
        .route("/api/mac/status", post(status))
        .route("/api/mac/release", post(release))
}

pub fn ui_routes() -> Router<Arc<App>> {
    Router::new()
        .route("/api/mac", get(panel))
        .route("/api/mac/install", post(install))
        .route("/api/mac/up", post(up))
        .route("/api/mac/stop", post(stop))
        .route("/api/mac/force-off", post(force_off))
        .route("/api/mac/pause", post(pause))
        .route("/api/mac/show", post(show))
        .route("/api/mac/take-back", post(take_back))
}

#[derive(Deserialize, Default)]
struct MacBody {
    #[serde(default)]
    thread: Option<String>,
    #[serde(default)]
    reason: String,
    #[serde(default)]
    port: Option<f64>,
    #[serde(default)]
    timeout: Option<u64>,
    #[serde(default)]
    action: String,
    #[serde(default)]
    args: Value,
}

fn thread_of(raw: &Option<String>) -> Result<Option<ThreadName>, ApiError> {
    Ok(raw.as_deref().map(str::trim).filter(|t| !t.is_empty()).map(ThreadName::parse).transpose()?)
}

fn text(t: String) -> ApiResult {
    Ok(ok_with(json!({ "text": t })))
}

async fn start(State(app): S, Agent(me): Agent, Json(b): Json<MacBody>) -> ApiResult {
    let thread = thread_of(&b.thread)?;
    text(app.mac_start_for(&me, thread.as_ref(), &b.reason, wait_bound(b.timeout)).await?)
}

async fn open_port(State(app): S, Agent(me): Agent, Json(b): Json<MacBody>) -> ApiResult {
    let thread = thread_of(&b.thread)?;
    let port = b.port.filter(|p| p.fract() == 0.0 && *p >= 0.0).ok_or_else(|| ApiError::bad("'port' is a whole number"))?;
    text(app.mac_open_port_for(&me, thread.as_ref(), port as u64, &b.reason, wait_bound(b.timeout)).await?)
}

async fn action(State(app): S, Agent(me): Agent, Json(b): Json<MacBody>) -> ApiResult {
    let v = app.mac_action_for(&me, &b.action, &b.args).await?;
    Ok(ok_with(v))
}

async fn status(State(app): S, Agent(me): Agent) -> ApiResult {
    text(app.mac_status_text(&me).await)
}

async fn release(State(app): S, Agent(me): Agent) -> ApiResult {
    text(app.mac_release_for(&me))
}

// ── The person's half ─────────────────────────────────────────────────────

async fn panel(State(app): S) -> ApiResult {
    Ok(ok_with(json!({ "mac": app.mac_status().await })))
}

/// Run a long job in the background; its outcome shows in the panel.
fn spawn_job<F>(app: Arc<App>, what: &'static str, job: F)
where
    F: std::future::Future<Output = anyhow::Result<String>> + Send + 'static,
{
    tokio::spawn(async move {
        match job.await {
            Ok(m) => tracing::info!("mac: {m}"),
            Err(e) => {
                tracing::warn!("mac: {what} failed: {e:#}");
                app.mac.lock().unwrap().error = Some(format!("{e:#}"));
                app.send(crate::bus::Event::Mac);
            }
        }
    });
}

async fn install(State(app): S) -> ApiResult {
    let a = app.clone();
    spawn_job(app, "install", async move { a.mac_install().await });
    Ok(ok_with(json!({ "started": true })))
}

async fn up(State(app): S) -> ApiResult {
    let a = app.clone();
    spawn_job(app, "boot", async move { a.mac_boot().await });
    Ok(ok_with(json!({ "started": true })))
}

async fn stop(State(app): S) -> ApiResult {
    text(app.mac_stop().await?)
}

async fn force_off(State(app): S) -> ApiResult {
    text(app.mac_force_off().await?)
}

async fn pause(State(app): S) -> ApiResult {
    text(app.mac_pause().await?)
}

async fn show(State(app): S) -> ApiResult {
    text(app.mac_show().await?)
}

async fn take_back(State(app): S) -> ApiResult {
    let who = app.mac_force_release();
    text(match who {
        Some(w) => format!("taken back from '{w}'"),
        None => "nobody held it".into(),
    })
}
