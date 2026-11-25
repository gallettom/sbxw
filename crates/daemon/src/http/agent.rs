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
use sbxw_core::relay::{agent_view, agent_view_part, RelayKind};
use sbxw_core::usage::Reading;
use sbxw_core::{SandboxRef, ThreadName};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

type S = State<Arc<App>>;

pub fn routes() -> Router<Arc<App>> {
    Router::new()
        .route("/api/hook", post(hook))
        .route("/api/hook/inbox_ack", post(inbox_ack))
        .route(
            "/api/hook/history",
            post(hook_history).layer(axum::extract::DefaultBodyLimit::max(sbxw_core::history::limits::POST_MAX_BYTES)),
        )
        .route("/api/usage", post(usage))
        .route("/api/relay/ask", post(ask))
        .route("/api/relay/shot", post(shot))
        .route("/api/relay/questions", post(questions))
        .route("/api/relay/open", post(open))
        .route("/api/relay/wait", post(wait))
        .route("/api/relay/ack", post(ack))
        .route("/api/relay/reply", post(reply))
        .route("/api/relay/show", post(show))
        .route("/api/plans/propose", post(propose))
        .route("/api/plans/wait", post(plan_wait))
        .route("/api/plans/status", post(plan_status))
        .route("/api/plans/propose_merge", post(propose_merge))
        .route("/api/plans/report", post(report))
        .route("/api/plans/mark_done", post(mark_done))
        .route("/api/lead/ack", post(lead_ack))
        .route("/api/tests/run", post(tests_run))
        .route("/api/tests/check", post(tests_check))
        .route("/api/tests/list", post(tests_list))
        .route("/api/sandboxes/:name/codemap/done", post(codemap_done))
        .merge(super::board::agent_routes())
        .merge(super::jarvis::agent_routes())
        .merge(super::mac::agent_routes())
        .merge(super::browser::agent_routes())
        .merge(super::canvas::agent_routes())
        .merge(super::tickets::agent_routes())
}

/// Answered with what the hook should print: on a thread's Bash call that
/// is its plan's whole test command, a deny; otherwise what sbxw holds for
/// the session as context (svc::inbox: its inbox on PostToolUse,
/// UserPromptSubmit, SessionStart and, as the reason to go on, Stop; the
/// lead's instructions on a new conversation; a low-battery note for a
/// Remote Control session's prompt). The inbox part (`sbxw_inbox`) is not
/// printed: it tells the hook what to acknowledge. Empty otherwise: a hook
/// printing nothing changes nothing.
async fn hook(State(app): S, Agent(me): Agent, Json(body): Json<Value>) -> Json<Value> {
    use crate::svc::session_context::CONTEXT_MAX_CHARS;
    // A thread about to run its plan's whole test command is sent to the
    // queue (svc::testq). The hook prints this answer; a daemon that does
    // not answer within the hook's limit lets the command run.
    let refused = app.tests_gate(&me, &body);
    let event = body["hook_event_name"].as_str().unwrap_or("").to_string();
    let ev = body.clone();
    app.ingest_hook(&me, body);
    app.inbox_hook_seen(&me, &ev);
    if let Some(reason) = refused {
        return Json(json!({
            "hookSpecificOutput": { "hookEventName": "PreToolUse", "permissionDecision": "deny", "permissionDecisionReason": reason }
        }));
    }
    let mut pieces = vec![];
    // A new conversation is given what sbxw holds for it (svc::session_context).
    if event == "SessionStart" {
        pieces.extend(app.session_start_context(&me, &ev));
    }
    if event == "UserPromptSubmit" {
        pieces.extend(app.battery_notice(&me));
    }
    // What is left of the context the harness keeps whole, for the inbox.
    let used: usize = pieces.iter().map(|p: &String| p.chars().count() + 2).sum();
    let inbox = app.inbox_offer(&me, &ev, CONTEXT_MAX_CHARS.saturating_sub(used));
    Json(crate::svc::inbox::hook_answer(&event, pieces, inbox))
}

#[derive(Deserialize)]
struct InboxAckBody {
    session_id: String,
    #[serde(default)]
    ids: Vec<String>,
}

/// The status hook printed these inbox messages for its session: they
/// leave the inbox. Only messages of the token's sandbox last offered to
/// that session; anything else is left as it was.
async fn inbox_ack(State(app): S, Agent(me): Agent, Json(b): Json<InboxAckBody>) -> Json<Value> {
    let ids: Vec<String> = b.ids.into_iter().take(sbxw_core::inbox::MAX_PER_RECIPIENT).collect();
    let n = app.inbox_ack(&me, b.session_id.trim(), &ids);
    Json(json!({ "ok": true, "acked": n }))
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

/// The request routes of a relay client from before `open` and `wait`
/// were two steps: open, park, answer once. Kept for a sandbox whose relay
/// files `sbxw up` has not refreshed; its caller has the outcome once it is
/// returned (it cannot acknowledge).
async fn open_and_park(app: Arc<App>, me: SandboxRef, b: AskBody, kind: RelayKind) -> ApiResult {
    let thread = thread_of(b.thread)?;
    let req = app.relay_open(&me, thread.as_ref(), kind, &b.question)?;
    let got = app.relay_wait_as(&req.id, &me, Some(thread.as_ref()), false, wait_bound(b.timeout)).await?;
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
struct OpenBody {
    kind: RelayKind,
    #[serde(default)]
    thread: Option<String>,
    question: String,
    /// Made by the relay server for one tool call: an open retried with it
    /// answers the same request.
    #[serde(default)]
    client_key: Option<String>,
}

/// Open a request and answer its id at once; `wait` parks on it. A Mac
/// request is opened by its own route, never here.
async fn open(State(app): S, Agent(me): Agent, Json(b): Json<OpenBody>) -> ApiResult {
    if b.kind == RelayKind::Mac {
        return Err(ApiError::bad("a virtual Mac request is opened with the mac tools"));
    }
    let thread = thread_of(b.thread)?;
    let req = app.relay_open_keyed(&me, thread.as_ref(), b.kind, &b.question, b.client_key.as_deref())?;
    Ok(Json(agent_view(&req)))
}

#[derive(Deserialize)]
struct WaitBody {
    id: String,
    #[serde(default)]
    timeout: Option<u64>,
    /// The asking session (empty: the sandbox's agent). Absent, from a
    /// client that predates it: the sandbox is checked alone.
    #[serde(default)]
    thread: Option<String>,
    /// The client acknowledges a final outcome with `ack` once it wrote it.
    #[serde(default)]
    ack: bool,
    /// Which part of a long answer (1-based).
    #[serde(default)]
    part: Option<usize>,
}

async fn wait(State(app): S, Agent(me): Agent, Json(b): Json<WaitBody>) -> ApiResult {
    let thread = b.thread.map(|t| thread_of(Some(t))).transpose()?;
    let got = app.relay_wait_as(b.id.trim(), &me, thread.as_ref().map(|t| t.as_ref()), b.ack, wait_bound(b.timeout)).await?;
    Ok(Json(agent_view_part(&got, b.part.unwrap_or(1))))
}

#[derive(Deserialize)]
struct AckBody {
    id: String,
    #[serde(default)]
    thread: Option<String>,
}

/// The relay server wrote the tool result with the outcome of `id`: it is
/// delivered, and nothing is handed on for it.
async fn ack(State(app): S, Agent(me): Agent, Json(b): Json<AckBody>) -> ApiResult {
    let thread = b.thread.map(|t| thread_of(Some(t))).transpose()?;
    let first = app.relay_ack(b.id.trim(), &me, thread.as_ref().map(|t| t.as_ref()))?;
    Ok(ok_with(json!({ "delivered": first })))
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
    /// The project's test command, proposed with the plan; run by the test
    /// queue only once the person approves the plan (see `svc::testq`).
    #[serde(default)]
    test_command: Option<String>,
    #[serde(default)]
    test_timeout_minutes: Option<u32>,
    #[serde(default)]
    timeout: Option<u64>,
}

async fn propose(State(app): S, Agent(me): Agent, Json(b): Json<ProposeBody>) -> ApiResult {
    app.require_leading(&me, None)?;
    let tests = sbxw_core::testq::PlanTests::proposed(b.test_command.as_deref(), b.test_timeout_minutes)?;
    let plan = app.plan_propose_with_tests(&me, &b.summary, b.threads, b.variant, &b.justification, b.lead.as_deref(), tests).await?;
    let plan = app.plan_wait(&plan.id, &me, wait_bound(b.timeout)).await?;
    Ok(ok_with(json!({ "plan": app.plan_view(&plan), "ticket_notes": app.ticket_link_notes(&plan) })))
}

/// `run_tests`: the caller's plan's approved command on a ref (empty: a
/// shared plan's team branch, or `head`, the caller's HEAD). Never a
/// command: a field that would be one is not read.
#[derive(Deserialize)]
struct TestsRunBody {
    #[serde(default)]
    thread: Option<String>,
    #[serde(default, rename = "ref")]
    git_ref: Option<String>,
    #[serde(default)]
    head: Option<String>,
    #[serde(default)]
    plan: Option<String>,
    #[serde(default)]
    rerun: bool,
    #[serde(default)]
    timeout: Option<u64>,
}

async fn tests_run(State(app): S, Agent(me): Agent, Json(b): Json<TestsRunBody>) -> ApiResult {
    let thread = thread_of(b.thread)?;
    let ask = crate::svc::testq::TestAsk {
        thread: thread.as_ref(),
        git_ref: b.git_ref.as_deref(),
        head: b.head.as_deref(),
        plan: b.plan.as_deref(),
        rerun: b.rerun,
    };
    let (job, took) = app.tests_request(&me, ask).await?;
    let took = match took {
        sbxw_core::testq::Took::Queued => "queued",
        sbxw_core::testq::Took::Joined => "joined",
        sbxw_core::testq::Took::Reused => "reused",
    };
    let job = app.tests_wait(&job.id, &me, wait_bound(b.timeout)).await?;
    Ok(ok_with(json!({ "job": crate::svc::testq::job_view(&job), "took": took })))
}

async fn tests_check(State(app): S, Agent(me): Agent, Json(b): Json<WaitBody>) -> ApiResult {
    let job = app.tests_wait(b.id.trim(), &me, wait_bound(b.timeout)).await?;
    Ok(ok_with(json!({ "job": crate::svc::testq::job_view(&job) })))
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct TestsListBody {
    thread: Option<String>,
    plan: Option<String>,
}

async fn tests_list(State(app): S, Agent(me): Agent, Json(b): Json<TestsListBody>) -> ApiResult {
    let thread = thread_of(b.thread)?;
    let plan = app.tests_plan(&me, thread.as_ref(), b.plan.as_deref());
    let (active, latest) = app.tests_jobs(&me);
    let view = |v: Vec<sbxw_core::testq::Job>| v.iter().map(crate::svc::testq::job_view).collect::<Vec<_>>();
    Ok(ok_with(json!({ "plan": crate::svc::testq::plan_tests_view(plan.as_ref()), "active": view(active), "latest": view(latest) })))
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
