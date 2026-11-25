//! The agent listener: what a sandbox may call, and nothing else.
//!
//! Every handler takes [`Agent`]: the sandbox proven by its token. A `from` in
//! a body (older relay tools still send one) is ignored; identity is never a
//! caller-filled field.

use super::error::{ok, ok_with, ApiError, ApiResult};
use super::guard::Agent;
use crate::app::App;
use crate::svc::relay::wait_bound;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::post;
use axum::{Json, Router};
use sbxw_core::agents::{CostReport, ModelReport};
use sbxw_core::plans::{DistillItem, PlanThread, ThreadState, Variant};
use sbxw_core::relay::{agent_view, RelayKind};
use sbxw_core::usage::Reading;
use sbxw_core::{SandboxRef, ThreadName};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

type S = State<Arc<App>>;

pub fn routes() -> Router<Arc<App>> {
    Router::new()
        .route("/api/hook", post(hook))
        .route(
            "/api/hook/history",
            post(hook_history).layer(axum::extract::DefaultBodyLimit::max(sbxw_core::history::limits::POST_MAX_BYTES)),
        )
        .route("/api/usage", post(usage))
        .route("/api/relay/ask", post(ask))
        .route("/api/relay/shot", post(shot))
        .route("/api/relay/questions", post(questions))
        .route("/api/relay/wait", post(wait))
        .route("/api/relay/reply", post(reply))
        .route("/api/relay/show", post(show))
        .route("/api/plans/propose", post(propose))
        .route("/api/plans/wait", post(plan_wait))
        .route("/api/plans/status", post(plan_status))
        .route("/api/plans/propose_merge", post(propose_merge))
        .route("/api/plans/report", post(report))
        .route("/api/plans/mark_done", post(mark_done))
        .route("/api/lead/ack", post(lead_ack))
        .route("/api/sandboxes/:name/codemap/done", post(codemap_done))
        .merge(super::board::agent_routes())
        .merge(super::jarvis::agent_routes())
        .merge(super::mac::agent_routes())
        .merge(super::browser::agent_routes())
        .merge(super::canvas::agent_routes())
        .merge(super::tickets::agent_routes())
}

/// Answered with what the hook should print: on a prompt from a Remote
/// Control session, a low-battery note for the agent's context. Empty
/// otherwise — a hook printing nothing changes nothing.
async fn hook(State(app): S, Agent(me): Agent, Json(body): Json<Value>) -> Json<Value> {
    let prompt = body["hook_event_name"] == "UserPromptSubmit";
    let start = (body["hook_event_name"] == "SessionStart").then(|| body.clone());
    app.ingest_hook(&me, body);
    // A new conversation is given what sbxw holds for it (svc::session_context).
    if let Some(ctx) = start.and_then(|ev| app.session_start_context(&me, &ev)) {
        return Json(json!({
            "hookSpecificOutput": { "hookEventName": "SessionStart", "additionalContext": ctx }
        }));
    }
    match prompt.then(|| app.battery_notice(&me)).flatten() {
        Some(note) => Json(json!({
            "hookSpecificOutput": { "hookEventName": "UserPromptSubmit", "additionalContext": note }
        })),
        None => Json(json!({})),
    }
}

/// The status hook's summarised transcript tail, for its own session (the
/// sandbox is the token's). 200 `{next}`: accepted up to that byte offset;
/// 409 `{next}`: out of step, start again from there (0: from the tail, with
/// `reset`); 413: too large (also past the body limit).
async fn hook_history(State(app): S, Agent(me): Agent, Json(body): Json<Value>) -> (StatusCode, Json<Value>) {
    use crate::svc::history::PostError;
    use sbxw_core::history::Posted;
    match app.history_post(&me, &body) {
        Ok(Posted::Accepted { next, .. }) => (StatusCode::OK, Json(json!({ "ok": true, "next": next }))),
        Ok(Posted::OutOfStep { next }) => (StatusCode::CONFLICT, Json(json!({ "ok": false, "next": next }))),
        Err(PostError::TooLarge) => (StatusCode::PAYLOAD_TOO_LARGE, Json(json!({ "ok": false, "error": "too many items" }))),
        Err(PostError::NoSession) => (StatusCode::BAD_REQUEST, Json(json!({ "ok": false, "error": "no session_id" }))),
    }
}

/// What the status line POSTs: the account's usage windows and, from a
/// script that knows them, the reporting session and the model it runs.
#[derive(Deserialize)]
struct UsageBody {
    #[serde(flatten)]
    reading: Reading,
    #[serde(default)]
    session_id: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    model_name: Option<String>,
    /// The session's `cost` object, flattened: dollars, wall-clock time and
    /// lines changed so far (see `SessionInfo::cost_usd`).
    #[serde(default)]
    cost_usd: Option<f64>,
    #[serde(default)]
    duration_ms: Option<u64>,
    #[serde(default)]
    lines_added: Option<u64>,
    #[serde(default)]
    lines_removed: Option<u64>,
    /// The sandbox's own counters, raw: the first line of its `/proc/stat`,
    /// its core count, its `/proc/meminfo` `MemTotal` and `MemAvailable`
    /// lines (see `sbxw_core::stats`).
    #[serde(default)]
    proc_stat_cpu: Option<String>,
    #[serde(default)]
    cpu_cores: Option<u32>,
    #[serde(default)]
    proc_meminfo: Option<String>,
}

/// A raw counter line is short: anything past 512 bytes is not one.
fn clip512(s: Option<&str>) -> Option<&str> {
    s.map(|s| s.get(..s.len().min(512)).unwrap_or(""))
}

async fn usage(State(app): S, Agent(me): Agent, Json(b): Json<UsageBody>) -> StatusCode {
    let model = ModelReport::new(b.model.as_deref(), b.model_name.as_deref());
    let cost = CostReport::new(b.cost_usd, b.duration_ms, b.lines_added, b.lines_removed);
    app.report_session(&me, b.session_id.as_deref().unwrap_or(""), model, cost);
    app.report_sandbox_stats(&me, clip512(b.proc_stat_cpu.as_deref()), b.cpu_cores, clip512(b.proc_meminfo.as_deref()));
    app.report_usage(b.reading);
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

async fn open_and_park(app: Arc<App>, me: SandboxRef, b: AskBody, kind: RelayKind) -> ApiResult {
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

/// Questions for the person (a /grill-me round), answered in the relay dialog.
async fn questions(State(app): S, Agent(me): Agent, Json(b): Json<AskBody>) -> ApiResult {
    open_and_park(app, me, b, RelayKind::Ask).await
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
struct ShowBody {
    id: String,
}

/// The routed-to sandbox fetches the question it was told about: only a
/// one-line notice is typed into its terminal.
async fn show(State(app): S, Agent(me): Agent, Json(b): Json<ShowBody>) -> ApiResult {
    let text = app.relay_show(&b.id, &me)?;
    Ok(ok_with(json!({ "text": text })))
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
    app.require_leading(&me, None)?;
    let plan = app.plan_propose(&me, &b.summary, b.threads, b.variant, &b.justification, b.lead.as_deref()).await?;
    let plan = app.plan_wait(&plan.id, &me, wait_bound(b.timeout)).await?;
    Ok(ok_with(json!({ "plan": app.plan_view(&plan), "ticket_notes": app.ticket_link_notes(&plan) })))
}

#[derive(Deserialize)]
struct PlanIdBody {
    id: String,
    #[serde(default)]
    timeout: Option<u64>,
}

async fn plan_wait(State(app): S, Agent(me): Agent, Json(b): Json<PlanIdBody>) -> ApiResult {
    app.require_leading(&me, Some(&b.id))?;
    app.plan_led(&b.id, &me)?;
    let plan = app.plan_wait(&b.id, &me, wait_bound(b.timeout)).await?;
    Ok(ok_with(json!({ "plan": app.plan_view(&plan), "ticket_notes": app.ticket_link_notes(&plan) })))
}

async fn plan_status(State(app): S, Agent(me): Agent, Json(b): Json<PlanIdBody>) -> ApiResult {
    app.require_leading(&me, Some(&b.id))?;
    let plan = app.plan_led(&b.id, &me)?;
    Ok(ok_with(json!({ "plan": app.plan_view_checked(&plan).await })))
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
    app.require_leading(&me, Some(&b.id))?;
    let plan = app.plan_propose_merge_checked(&b.id, &me, b.order, &b.note, b.distillate).await?;
    Ok(ok_with(json!({ "plan": app.plan_view(&plan) })))
}

#[derive(Deserialize)]
struct MarkDoneBody {
    id: String,
    thread: String,
    #[serde(default)]
    note: String,
}

/// The lead marks a thread it reviewed done: the plan id is its capability,
/// as for `propose_merge`.
async fn mark_done(State(app): S, Agent(me): Agent, Json(b): Json<MarkDoneBody>) -> ApiResult {
    app.require_leading(&me, Some(&b.id))?;
    let plan = app.plan_mark_done(&b.id, &me, &b.thread, &b.note)?;
    Ok(ok_with(json!({ "plan": app.plan_view(&plan) })))
}

#[derive(Deserialize, Default)]
struct LeadAckBody {
    /// `SBXW_THREAD` of the calling session: a thread is refused.
    #[serde(default)]
    thread: Option<String>,
}

/// The main session says it took the lead: `activating` becomes `leading`
/// (see `svc::lead`). The sandbox is the token's; the body carries no text
/// the daemon acts on. A refusal is 409 with a fixed text for the agent.
async fn lead_ack(State(app): S, Agent(me): Agent, body: Option<Json<LeadAckBody>>) -> ApiResult {
    let b = body.map(|Json(b)| b).unwrap_or_default();
    let text = app.lead_ack(&me, b.thread.as_deref()).map_err(ApiError::conflict)?;
    Ok(ok_with(json!({ "text": text })))
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
    if SandboxRef::parse(&name).ok() != Some(me.clone()) {
        return Err(ApiError::forbidden(format!("'{me}' cannot report for '{name}'")));
    }
    let b = body.map(|Json(b)| b).unwrap_or_default();
    let ok = b.ok.unwrap_or(true);
    let tracked = app.codemap_finish(&me.to_string(), ok, b.note.as_deref()).is_some();
    tracing::info!("codemap: '{me}' reports {} (run in flight: {tracked})", if ok { "it is written" } else { "it gave up" });
    Ok(ok_with(json!({ "tracked": tracked })))
}
