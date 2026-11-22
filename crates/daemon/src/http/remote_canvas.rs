//! The canvas on the phone, on the remote listener. The contract is
//! `.sbxw-artifacts/remote/API-canvas.md`; the rules are
//! `svc::remote_canvas`'s. Routes only: each resolves its sandbox or canvas
//! there, and every failure to find one is the same 404.

use super::remote::refusal;
use crate::app::App;
use crate::svc::remote_access::{DeviceCtx, Refusal};
use crate::svc::remote_canvas::{RemarkIn, SubmitError};
use axum::extract::{Path, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;

type S = State<Arc<App>>;

/// A remarks body: 2,000 characters of text (up to 4 bytes each) and its JSON.
pub const REMARK_BODY_MAX: usize = 16 * 1024;
/// A sandbox name as the phone sends it, at most.
const NAME_MAX: usize = 96;

pub async fn list(State(app): S, Extension(_d): Extension<DeviceCtx>, Path(sandbox): Path<String>) -> Response {
    if sandbox.len() > NAME_MAX {
        return refusal(Refusal::NotFound);
    }
    match app.remote_target(&sandbox) {
        Ok(sb) => Json(app.remote_canvas_list(&sb)).into_response(),
        Err(e) => refusal(e),
    }
}

pub async fn manifest(State(app): S, Extension(d): Extension<DeviceCtx>, Path(id): Path<String>) -> Response {
    match app.remote_canvas_view(&d, &id) {
        Ok(v) => Json(v).into_response(),
        Err(e) => refusal(e),
    }
}

/// The picture: never cached, never sniffed.
pub async fn image(State(app): S, Extension(_d): Extension<DeviceCtx>, Path((id, file)): Path<(String, String)>) -> Response {
    match app.remote_canvas_image(&id, &file) {
        Ok((bytes, mime)) => (StatusCode::OK, [(header::CONTENT_TYPE, mime), (header::CACHE_CONTROL, "no-store, private"), (header::X_CONTENT_TYPE_OPTIONS, "nosniff")], bytes).into_response(),
        Err(e) => refusal(e),
    }
}

pub async fn remarks(State(app): S, Extension(d): Extension<DeviceCtx>, Path(id): Path<String>, Json(b): Json<RemarkIn>) -> Response {
    match app.remote_canvas_remark(&d, &id, b) {
        Ok(v) => Json(v).into_response(),
        Err(e) => refusal(e),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Empty {}

pub async fn submit(State(app): S, Extension(d): Extension<DeviceCtx>, Path(id): Path<String>, Json(_): Json<Empty>) -> Response {
    match app.remote_canvas_submit(&d, &id).await {
        Ok(n) => Json(json!({ "ok": true, "review": n })).into_response(),
        Err(SubmitError::Refused(r)) => refusal(r),
        Err(SubmitError::Examples) => (
            StatusCode::CONFLICT,
            Json(json!({ "ok": false, "reason": "examples", "error": "This round holds example pictures added on the Mac: send it from the Mac." })),
        )
            .into_response(),
        Err(SubmitError::NotSubmitted(n)) => (
            StatusCode::CONFLICT,
            Json(json!({
                "ok": false, "reason": "not_submitted", "recorded": true, "review": n,
                "error": "The round is recorded (the agent can read it with canvas_read), but the session started asking at the Mac while its line was typed: the line waits, unsent, in the session's input.",
            })),
        )
            .into_response(),
    }
}
