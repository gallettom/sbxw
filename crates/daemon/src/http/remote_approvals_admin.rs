//! Level 2 from the Mac: on the UI listener, behind the UI token like every
//! UI route. Turning approving from the phone on or off, granting a device
//! one passkey registration (the person types the six digits its phone
//! shows), revoking a passkey. None of these exists on the remote listener;
//! no agent, sandbox or Jarvis holds the UI token.

use super::error::{ok, ApiError, ApiResult};
use crate::app::App;
use axum::extract::{Path, State};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use sbxw_core::remote_access::level2::AllowError;
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

type S = State<Arc<App>>;

/// Every admin route of level 2, `(method, path)`: the route walk checks
/// none answers on the remote listener.
pub const ROUTES: &[(&str, &str)] = &[
    ("GET", "/api/remote/approvals"),
    ("POST", "/api/remote/approvals/enable"),
    ("POST", "/api/remote/approvals/disable"),
    ("POST", "/api/remote/approvals/devices/:id/allow"),
    ("POST", "/api/remote/approvals/devices/:id/refuse"),
    ("DELETE", "/api/remote/approvals/devices/:id/passkey"),
];

pub fn routes() -> Router<Arc<App>> {
    Router::new()
        .route("/api/remote/approvals", get(status))
        .route("/api/remote/approvals/enable", post(enable))
        .route("/api/remote/approvals/disable", post(disable))
        .route("/api/remote/approvals/devices/:id/allow", post(allow))
        .route("/api/remote/approvals/devices/:id/refuse", post(refuse))
        .route("/api/remote/approvals/devices/:id/passkey", delete(revoke))
}

async fn status(State(app): S) -> Json<Value> {
    Json(app.remote_approvals_status())
}

async fn enable(State(app): S) -> ApiResult {
    app.remote_approvals_enable().map_err(ApiError::conflict)?;
    Ok(ok())
}

async fn disable(State(app): S) -> ApiResult {
    app.remote_approvals_disable();
    Ok(ok())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AllowBody {
    /// The six digits the phone shows, typed by the person.
    check: String,
}

async fn allow(State(app): S, Path(id): Path<String>, Json(b): Json<AllowBody>) -> ApiResult {
    match app.remote_passkey_allow(&id, &b.check) {
        Ok(until) => Ok(Json(json!({ "ok": true, "expires_ms": until }))),
        Err(AllowError::Unknown) => Err(ApiError::not_found(format!("device '{id}' has no Face ID request waiting: start it on the phone first"))),
        Err(AllowError::WrongCheck { left }) => Err(ApiError::forbidden(format!("those are not the digits the phone shows ({left} tries left)"))),
        Err(AllowError::Refused) => Err(ApiError::forbidden("wrong digits three times: the request is refused. Start again on the phone")),
    }
}

async fn refuse(State(app): S, Path(id): Path<String>) -> ApiResult {
    if app.remote_passkey_refuse(&id) {
        Ok(ok())
    } else {
        Err(ApiError::not_found(format!("device '{id}' has no Face ID request waiting")))
    }
}

async fn revoke(State(app): S, Path(id): Path<String>) -> ApiResult {
    if app.remote_passkey_revoke(&id) {
        Ok(ok())
    } else {
        Err(ApiError::not_found(format!("device '{id}' has no passkey")))
    }
}
