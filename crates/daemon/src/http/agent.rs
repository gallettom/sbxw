//! The agent listener: what a sandbox may call, and nothing else.
//!
//! Every handler takes [`Agent`], the sandbox proven by its token. A `from`
//! in a body (older relay tools still send one) is ignored: identity is never
//! a field a caller fills in.

use super::error::{ok, ok_with, ApiError, ApiResult};
use super::guard::Agent;
use crate::app::App;
use crate::svc::relay::wait_bound;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::post;
use axum::{Json, Router};
use sbxw_core::plans::{DistillItem, PlanThread, ThreadState, Variant};
use sbxw_core::relay::{agent_view, RelayKind};
use sbxw_core::usage::Reading;
use sbxw_core::{SandboxName, ThreadName};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

type S = State<Arc<App>>;

pub fn routes() -> Router<Arc<App>> {
    Router::new()
        .route("/api/hook", post(hook))
        .route("/api/usage", post(usage))
        .route("/api/relay/ask", post(ask))
        .route("/api/relay/shot", post(shot))
        .route("/api/relay/wait", post(wait))
        .route("/api/relay/reply", post(reply))
        .route("/api/plans/propose", post(propose))
        .route("/api/plans/wait", post(plan_wait))
        .route("/api/plans/status", post(plan_status))
        .route("/api/plans/propose_merge", post(propose_merge))
        .route("/api/plans/report", post(report))
        .route("/api/sandboxes/:name/codemap/done", post(codemap_done))
}

/// Answered with what the hook should print: on a prompt from a Remote
/// Control session, a low-battery note for the agent's context. Empty
/// otherwise — a hook printing nothing changes nothing.
async fn hook(State(app): S, Agent(me): Agent, Json(body): Json<Value>) -> Json<Value> {
    let prompt = body["hook_event_name"] == "UserPromptSubmit";
    app.ingest_hook(&me, body);
    match prompt.then(|| app.battery_notice(&me)).flatten() {
        Some(note) => Json(json!({
            "hookSpecificOutput": { "hookEventName": "UserPromptSubmit", "additionalContext": note }
        })),
        None => Json(json!({})),
    }
}

async fn usage(State(app): S, Agent(_): Agent, Json(r): Json<Reading>) -> StatusCode {
    app.report_usage(r);
    StatusCode::OK
}

fn thread_of(raw: Option<String>) -> Result<Option<ThreadName>, ApiError> {
    Ok(raw.map(|t| t.trim().to_string()).filter(|t| !t.is_empty()).map(|t| ThreadName::parse(&t)).transpose()?)
}

#[derive(Deserialize)]
struct AskBody {
    #[serde(default)]
    thread: Option<String>,
    question: String,
    #[serde(default)]
    timeout: Option<u64>,
}

async fn open_and_park(app: Arc<App>, me: SandboxName, b: AskBody, kind: RelayKind) -> ApiResult {
    let thread = thread_of(b.thread)?;
    let req = app.relay_open(&me, thread.as_ref(), kind, &b.question)?;
    let got = app.relay_wait(&req.id, &me, wait_bound(b.timeout)).await?;
    Ok(Json(agent_view(&got)))
}

async fn ask(State(app): S, Agent(me): Agent, Json(b): Json<AskBody>) -> ApiResult {
    open_and_park(app, me, b, RelayKind::Question).await
}

async fn shot(State(app): S, Agent(me): Agent, Json(b): Json<AskBody>) -> ApiResult {
    open_and_park(app, me, b, RelayKind::Screenshot).await
}

#[derive(Deserialize)]
struct WaitBody {
    id: String,
    #[serde(default)]
    timeout: Option<u64>,
}

async fn wait(State(app): S, Agent(me): Agent, Json(b): Json<WaitBody>) -> ApiResult {
    let got = app.relay_wait(&b.id, &me, wait_bound(b.timeout)).await?;
    Ok(Json(agent_view(&got)))
}

#[derive(Deserialize)]
struct ReplyBody {
    id: String,
    answer: String,
}

async fn reply(State(app): S, Agent(me): Agent, Json(b): Json<ReplyBody>) -> ApiResult {
    app.relay_reply(&b.id, &me, &b.answer)?;
    Ok(ok())
}

#[derive(Deserialize)]
struct ProposeBody {
    #[serde(default)]
    summary: String,
    threads: Vec<PlanThread>,
    #[serde(default)]
    variant: Variant,
    #[serde(default)]
    justification: String,
    #[serde(default)]
    lead: Option<String>,
    #[serde(default)]
    timeout: Option<u64>,
}

async fn propose(State(app): S, Agent(me): Agent, Json(b): Json<ProposeBody>) -> ApiResult {
    let plan = app.plan_propose(&me, &b.summary, b.threads, b.variant, &b.justification, b.lead.as_deref())?;
    let plan = app.plan_wait(&plan.id, &me, wait_bound(b.timeout)).await?;
    Ok(ok_with(json!({ "plan": app.plan_view(&plan) })))
}

#[derive(Deserialize)]
struct PlanIdBody {
    id: String,
    #[serde(default)]
    timeout: Option<u64>,
}

async fn plan_wait(State(app): S, Agent(me): Agent, Json(b): Json<PlanIdBody>) -> ApiResult {
    app.plan_led(&b.id, &me)?;
    let plan = app.plan_wait(&b.id, &me, wait_bound(b.timeout)).await?;
    Ok(ok_with(json!({ "plan": app.plan_view(&plan) })))
}

async fn plan_status(State(app): S, Agent(me): Agent, Json(b): Json<PlanIdBody>) -> ApiResult {
    let plan = app.plan_led(&b.id, &me)?;
    Ok(ok_with(json!({ "plan": app.plan_view(&plan) })))
}

#[derive(Deserialize)]
struct MergeProposeBody {
    id: String,
    /// A shared plan's may be empty: it merges every thread.
    #[serde(default)]
    order: Vec<String>,
    #[serde(default)]
    note: String,
    #[serde(default)]
    distillate: Vec<DistillItem>,
}

async fn propose_merge(State(app): S, Agent(me): Agent, Json(b): Json<MergeProposeBody>) -> ApiResult {
    let plan = app.plan_propose_merge(&b.id, &me, b.order, &b.note, b.distillate)?;
    Ok(ok_with(json!({ "plan": app.plan_view(&plan) })))
}

#[derive(Deserialize)]
struct ReportBody {
    thread: String,
    state: ThreadState,
    #[serde(default)]
    summary: String,
}

async fn report(State(app): S, Agent(me): Agent, Json(b): Json<ReportBody>) -> ApiResult {
    let thread = ThreadName::parse(&b.thread)?;
    let lead = app.plan_report(&me, &thread, b.state, &b.summary)?;
    Ok(ok_with(json!({ "lead": lead })))
}

#[derive(Deserialize, Default)]
struct DoneBody {
    ok: Option<bool>,
    note: Option<String>,
}

/// The agent says the document is written. The path names a sandbox, and it
/// has to be the caller's own.
async fn codemap_done(State(app): S, Agent(me): Agent, Path(name): Path<String>, body: Option<Json<DoneBody>>) -> ApiResult {
    if me.as_str() != name {
        return Err(ApiError::forbidden(format!("'{me}' cannot report for '{name}'")));
    }
    let b = body.map(|Json(b)| b).unwrap_or_default();
    let ok = b.ok.unwrap_or(true);
    let tracked = app.codemap_finish(&me, ok, b.note.as_deref()).is_some();
    tracing::info!("codemap: '{me}' reports {} (run in flight: {tracked})", if ok { "it is written" } else { "it gave up" });
    Ok(ok_with(json!({ "tracked": tracked })))
}
