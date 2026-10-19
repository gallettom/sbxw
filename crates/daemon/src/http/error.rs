//! Errors with a status code and the body the UI already reads.
//!
//! The old daemon answered every failure `200 {ok:false, error}`. The body is
//! kept — the page branches on `ok` — and the status now says what happened,
//! so the island, a proxy or `curl -f` can tell without parsing.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::{json, Value};

#[derive(Debug)]
pub struct ApiError {
    pub status: StatusCode,
    pub message: String,
}

impl ApiError {
    pub fn new(status: StatusCode, message: impl Into<String>) -> Self {
        Self { status, message: message.into() }
    }
    pub fn bad(m: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, m)
    }
    pub fn not_found(m: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, m)
    }
    pub fn conflict(m: impl Into<String>) -> Self {
        Self::new(StatusCode::CONFLICT, m)
    }
    pub fn unauthorized(m: impl Into<String>) -> Self {
        Self::new(StatusCode::UNAUTHORIZED, m)
    }
    pub fn forbidden(m: impl Into<String>) -> Self {
        Self::new(StatusCode::FORBIDDEN, m)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.status, Json(json!({ "ok": false, "error": self.message }))).into_response()
    }
}

/// A failure from below (sbx, the filesystem): the whole context chain, since
/// sbx's own words are what the person can act on.
impl From<anyhow::Error> for ApiError {
    fn from(e: anyhow::Error) -> Self {
        Self::new(StatusCode::BAD_GATEWAY, format!("{e:#}"))
    }
}

/// A refusal from a service: the request cannot be honoured as asked.
impl From<String> for ApiError {
    fn from(e: String) -> Self {
        Self::bad(e)
    }
}

impl From<sbxw_core::names::NameError> for ApiError {
    fn from(e: sbxw_core::names::NameError) -> Self {
        Self::bad(e.0)
    }
}

pub type ApiResult<T = Json<Value>> = Result<T, ApiError>;

pub fn ok() -> Json<Value> {
    Json(json!({ "ok": true }))
}

/// `{ok: true, …fields}`.
pub fn ok_with(fields: Value) -> Json<Value> {
    let mut body = json!({ "ok": true });
    if let (Some(obj), Some(extra)) = (body.as_object_mut(), fields.as_object()) {
        obj.extend(extra.clone());
    }
    Json(body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn envelopes_keep_their_shape() {
        assert_eq!(ok().0, json!({"ok": true}));
        assert_eq!(ok_with(json!({"name": "x"})).0, json!({"ok": true, "name": "x"}));
        let e: ApiError = anyhow::anyhow!("outer").context("ctx").into();
        assert_eq!(e.status, StatusCode::BAD_GATEWAY);
        assert_eq!(e.message, "ctx: outer");
    }
}
