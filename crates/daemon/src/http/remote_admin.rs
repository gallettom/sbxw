//! Remote access, from the Mac: on the UI listener, behind the UI token like
//! every UI route (`guard::ui_guard`). For the CLI now and the desktop later.
//! None of these exists on the remote listener.

use super::error::{ok, ApiError, ApiResult};
use crate::app::App;
use axum::extract::{Path, Query, State};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use sbxw_core::remote_access::journal::Kind;
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

type S = State<Arc<App>>;

/// Every admin route, `(method, path)`: the route-walk test checks that none
/// answers on the remote listener.
pub const ROUTES: &[(&str, &str)] = &[
    ("GET", "/api/remote"),
    ("POST", "/api/remote/enable"),
    ("POST", "/api/remote/disable"),
    ("POST", "/api/remote/pairing"),
    ("GET", "/api/remote/pairing/requests"),
    ("POST", "/api/remote/pairing/requests/:id/approve"),
    ("POST", "/api/remote/pairing/requests/:id/refuse"),
    ("GET", "/api/remote/devices"),
    ("DELETE", "/api/remote/devices/:id"),
    ("DELETE", "/api/remote/devices/:id/push"),
    ("GET", "/api/remote/journal"),
    ("POST", "/api/remote/tailnet/forget"),
];

pub fn routes() -> Router<Arc<App>> {
    Router::new()
        .route("/api/remote", get(status))
        .route("/api/remote/enable", post(enable))
        .route("/api/remote/disable", post(disable))
        .route("/api/remote/pairing", post(pairing))
        .route("/api/remote/pairing/requests", get(requests))
        .route("/api/remote/pairing/requests/:id/approve", post(approve))
        .route("/api/remote/pairing/requests/:id/refuse", post(refuse))
        .route("/api/remote/devices", get(devices))
        .route("/api/remote/devices/:id", delete(revoke))
        .route("/api/remote/devices/:id/push", delete(push_off))
        .route("/api/remote/journal", get(journal))
        .route("/api/remote/tailnet/forget", post(forget))
}

async fn status(State(app): S) -> Json<Value> {
    Json(json!(app.remote_status()))
}

async fn enable(State(app): S) -> ApiResult {
    let st = app.remote_enable().await.map_err(ApiError::conflict)?;
    Ok(Json(json!({ "ok": true, "status": st })))
}

async fn disable(State(app): S) -> Json<Value> {
    let st = app.remote_disable().await;
    Json(json!({ "ok": true, "status": st }))
}

async fn pairing(State(app): S) -> ApiResult {
    let p = app.remote_pairing_code().map_err(ApiError::conflict)?;
    Ok(Json(json!({ "ok": true, "code": p.code, "code_id": p.code_id, "url": p.url, "expires_ms": p.expires_ms })))
}

/// Pending requests WITHOUT their check digits: the Mac never shows them,
/// so the person reads them on the phone and types them to approve.
async fn requests(State(app): S) -> Json<Value> {
    let list: Vec<Value> = app
        .remote_pairing_requests()
        .into_iter()
        .map(|r| json!({ "id": r.id, "code_id": r.code_id, "device_name": r.device_name, "created_ms": r.created_ms, "expires_ms": r.expires_ms }))
        .collect();
    Json(json!(list))
}

#[derive(Deserialize)]
struct ApproveBody {
    /// The six digits the phone shows, typed by the person.
    check: String,
}

/// The Mac's yes binds to the phone in hand: it carries the digits the
/// phone shows. Three wrong tries refuse the request.
async fn approve(State(app): S, Path(id): Path<String>, Json(b): Json<ApproveBody>) -> ApiResult {
    use sbxw_core::remote_access::pairing::ApproveError;
    match app.remote_pairing_approve(&id, &b.check) {
        Ok(()) => Ok(ok()),
        Err(ApproveError::Unknown) => Err(ApiError::not_found("no pending pairing request with that id (it may have expired or been replaced by a new code)")),
        Err(ApproveError::WrongCheck { left }) => Err(ApiError::forbidden(format!("those are not the digits the phone shows ({left} tries left)"))),
        Err(ApproveError::Refused) => Err(ApiError::forbidden("wrong digits three times: the request is refused. Start pairing again")),
    }
}

async fn refuse(State(app): S, Path(id): Path<String>) -> ApiResult {
    if app.remote_pairing_refuse(&id) {
        Ok(ok())
    } else {
        Err(ApiError::not_found("no pending pairing request with that id (it may have expired)"))
    }
}

/// Each device, with `notifications`: whether it has a push subscription.
async fn devices(State(app): S) -> Json<Value> {
    let list: Vec<Value> = app
        .remote_devices()
        .into_iter()
        .map(|d| {
            let on = app.remote_push_on(&d.id);
            let mut v = json!(d);
            v["notifications"] = on.into();
            v
        })
        .collect();
    Json(json!(list))
}

/// The Mac turns a device's notifications off (D6). The device stays
/// paired; it can turn them on again from the phone.
async fn push_off(State(app): S, Path(id): Path<String>) -> ApiResult {
    if app.remote_device(&id).is_none() {
        return Err(ApiError::not_found(format!("no paired device '{id}'")));
    }
    app.remote_push_off(&id);
    Ok(ok())
}

async fn revoke(State(app): S, Path(id): Path<String>) -> ApiResult {
    if app.remote_revoke(&id, Kind::Revoked) {
        Ok(ok())
    } else {
        Err(ApiError::not_found(format!("no paired device '{id}'")))
    }
}

#[derive(Deserialize)]
struct JournalQuery {
    limit: Option<usize>,
}

async fn journal(State(app): S, Query(q): Query<JournalQuery>) -> Json<Value> {
    Json(json!(app.remote_journal(q.limit.unwrap_or(200).min(sbxw_core::remote_access::journal::MAX_ENTRIES))))
}

async fn forget(State(app): S) -> ApiResult {
    app.remote_forget_node().await.map_err(ApiError::conflict)?;
    Ok(ok())
}
