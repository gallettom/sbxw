//! Ticket routes (the Kanban). The UI (UI listener, UI token): the board,
//! a ticket, everything the person does. Agents (agent listener, the
//! sandbox is the token's): read only, `tickets_list` and `ticket_read`.
//! See `docs/tickets.md`.

use super::error::{ok_with, ApiError, ApiResult};
use super::guard::Agent;
use crate::app::App;
use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use sbxw_core::tickets::{self, Draft, Move};
use sbxw_core::SandboxRef;
use serde::Deserialize;
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;

type S = State<Arc<App>>;

/// A file travels as raw bytes.
const FILE_BODY_MAX: usize = tickets::MAX_FILE_BYTES as usize + 1024;

pub fn agent_routes() -> Router<Arc<App>> {
    Router::new().route("/api/tickets/list", post(agent_list)).route("/api/tickets/read", post(agent_read))
}

pub fn ui_routes() -> Router<Arc<App>> {
    Router::new()
        .route("/api/tickets", get(board))
        .route("/api/tickets/:sandbox", post(create))
        .route("/api/tickets/:sandbox/:n", get(one))
        .route("/api/tickets/:sandbox/:n/edit", post(edit))
        .route("/api/tickets/:sandbox/:n/files", post(attach).layer(DefaultBodyLimit::max(FILE_BODY_MAX)))
        .route("/api/tickets/:sandbox/:n/files/:file", get(file))
        .route("/api/tickets/:sandbox/:n/files/:file/remove", post(remove_file))
        .route("/api/tickets/:sandbox/:n/move", post(move_ticket))
        .route("/api/tickets/:sandbox/:n/delete", post(delete))
        .route("/api/tickets/:sandbox/:n/send", post(send))
}

fn sandbox(raw: &str) -> Result<SandboxRef, ApiError> {
    Ok(SandboxRef::parse(raw)?)
}

fn number(raw: &str) -> Result<u32, ApiError> {
    tickets::parse_num(raw).ok_or_else(|| ApiError::bad("not a ticket number"))
}

// ── Agents ────────────────────────────────────────────────────────────────

async fn agent_list(State(app): S, Agent(me): Agent) -> ApiResult {
    Ok(ok_with(json!({ "text": app.tickets_agent_list(&me) })))
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct ReadBody {
    id: serde_json::Value,
}

async fn agent_read(State(app): S, Agent(me): Agent, Json(b): Json<ReadBody>) -> ApiResult {
    let id = match &b.id {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Number(n) => n.to_string(),
        _ => return Err(ApiError::bad("id is a ticket number (T-12)")),
    };
    let text = app.tickets_agent_read(&me, &id).await?;
    Ok(ok_with(json!({ "text": text })))
}

// ── The UI ────────────────────────────────────────────────────────────────

/// `?sandbox=<name>|all&older=1`.
async fn board(State(app): S, Query(q): Query<HashMap<String, String>>) -> ApiResult {
    let sb = match q.get("sandbox").map(String::as_str) {
        None | Some("") | Some("all") => None,
        Some(s) => Some(sandbox(s)?),
    };
    let older = q.get("older").is_some_and(|v| v == "1" || v == "true");
    app.plans_ext_refresh(sb.as_ref().map(|s| vec![s.to_string()]).as_deref()).await;
    let b = app.tickets_board(sb.as_ref(), older);
    Ok(ok_with(json!({ "board": b })))
}

async fn one(State(app): S, Path((sb, n)): Path<(String, String)>) -> ApiResult {
    let sb = sandbox(&sb)?;
    let t = app.ticket_view(&sb, number(&n)?).ok_or_else(|| ApiError::not_found("no such ticket"))?;
    Ok(ok_with(json!({ "ticket": t })))
}

async fn create(State(app): S, Path(sb): Path<String>, Json(d): Json<Draft>) -> ApiResult {
    let sb = sandbox(&sb)?;
    let t = app.ticket_create(&sb, d)?;
    Ok(ok_with(json!({ "ticket": t })))
}

#[derive(Deserialize, Default)]
struct EditBody {
    #[serde(flatten)]
    draft: Draft,
    #[serde(default)]
    validated: Option<bool>,
}

async fn edit(State(app): S, Path((sb, n)): Path<(String, String)>, Json(b): Json<EditBody>) -> ApiResult {
    let sb = sandbox(&sb)?;
    let t = app.ticket_edit(&sb, number(&n)?, b.draft, b.validated)?;
    Ok(ok_with(json!({ "ticket": t })))
}

/// `?name=<the file's name, shown only>`; the body is the file.
async fn attach(State(app): S, Path((sb, n)): Path<(String, String)>, Query(q): Query<HashMap<String, String>>, body: Bytes) -> ApiResult {
    let sb = sandbox(&sb)?;
    let t = app.ticket_attach(&sb, number(&n)?, q.get("name").map(String::as_str).unwrap_or("file"), &body)?;
    Ok(ok_with(json!({ "ticket": t })))
}

/// A file's bytes: the type sniffed when it was attached (text as plain
/// text, never HTML), `nosniff`, a sandboxing CSP and `attachment`, so no
/// browser renders it as a page.
async fn file(State(app): S, Path((sb, n, f)): Path<(String, String, String)>) -> Result<Response, ApiError> {
    let sb = sandbox(&sb)?;
    let (bytes, mime, name) = app.ticket_file(&sb, number(&n)?, &f).map_err(ApiError::not_found)?;
    let disposition = format!("attachment; filename=\"{}\"", tickets::safe_name(&name));
    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, mime.to_string()),
            (header::CONTENT_DISPOSITION, disposition),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff".to_string()),
            (header::CONTENT_SECURITY_POLICY, "sandbox; default-src 'none'".to_string()),
            (header::CACHE_CONTROL, "private, no-store".to_string()),
        ],
        bytes,
    )
        .into_response())
}

async fn remove_file(State(app): S, Path((sb, n, f)): Path<(String, String, String)>) -> ApiResult {
    let sb = sandbox(&sb)?;
    let t = app.ticket_remove_file(&sb, number(&n)?, &f)?;
    Ok(ok_with(json!({ "ticket": t })))
}

#[derive(Deserialize)]
struct MoveBody {
    to: Move,
}

async fn move_ticket(State(app): S, Path((sb, n)): Path<(String, String)>, Json(b): Json<MoveBody>) -> ApiResult {
    let sb = sandbox(&sb)?;
    app.ticket_move(&sb, number(&n)?, b.to)?;
    Ok(ok_with(json!({})))
}

async fn delete(State(app): S, Path((sb, n)): Path<(String, String)>) -> ApiResult {
    let sb = sandbox(&sb)?;
    let (files, bytes) = app.ticket_delete(&sb, number(&n)?)?;
    Ok(ok_with(json!({ "deleted": 1, "files": files, "bytes": bytes })))
}

/// "Send to the lead": the app asked the person first. The line is the
/// daemon's own (`tickets::send_line`): nothing of the body is typed.
async fn send(State(app): S, Path((sb, n)): Path<(String, String)>) -> ApiResult {
    let sb = sandbox(&sb)?;
    let n = number(&n)?;
    let sent = app.ticket_send(&sb, n).map_err(ApiError::conflict)?;
    Ok(ok_with(json!({ "sent": sent, "line": tickets::send_line(n) })))
}

#[cfg(test)]
mod tests;
