//! Level 2 on the remote listener: approving with a passkey
//! (`.sbxw-artifacts/remote/API-level2.md`). These handlers sit behind the
//! remote guard like every `/r/` route (Host, Origin, path, device cookie);
//! `http::remote` lists them in its `ROUTES`. Agent-written text in what
//! they return is data, masked by the person's anonymise rules.

use crate::app::App;
use crate::bus::Event;
use crate::svc::remote_access::DeviceCtx;
use crate::svc::remote_approvals::{Assertion, L2Error};
use axum::extract::{Path, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::sse::Event as Sse;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use sbxw_core::remote_access::level2::Action;
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;

type S = State<Arc<App>>;

/// `POST /r/passkey/register`: an attestation object is larger than the
/// listener's 4 KiB.
pub const REGISTER_BODY_MAX: usize = 8 * 1024;

pub fn refuse(e: L2Error) -> Response {
    let status = StatusCode::from_u16(e.status).unwrap_or(StatusCode::FORBIDDEN);
    let mut res = (status, Json(json!({ "ok": false, "error": e.error, "reason": e.reason }))).into_response();
    if let Some(ms) = e.retry_ms {
        if let Ok(v) = HeaderValue::from_str(&ms.div_ceil(1000).max(1).to_string()) {
            res.headers_mut().insert(header::RETRY_AFTER, v);
        }
    }
    res
}

fn answer(r: Result<Value, L2Error>) -> Response {
    match r {
        Ok(v) => Json(v).into_response(),
        Err(e) => refuse(e),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Empty {}

// ── Overview and registration ────────────────────────────────────────────

pub async fn approvals(State(app): S, Extension(d): Extension<DeviceCtx>) -> Response {
    Json(app.remote_approvals_overview(&d).await).into_response()
}

pub async fn passkey_start(State(app): S, Extension(d): Extension<DeviceCtx>, Json(_): Json<Empty>) -> Response {
    answer(app.remote_passkey_start(&d).map(|(id, check, exp)| json!({ "request_id": id, "check": check, "expires_ms": exp })))
}

pub async fn passkey_status(State(app): S, Extension(d): Extension<DeviceCtx>) -> Response {
    answer(app.remote_passkey_status(&d))
}

pub async fn passkey_options(State(app): S, Extension(d): Extension<DeviceCtx>, Json(_): Json<Empty>) -> Response {
    answer(app.remote_passkey_options(&d))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Register {
    id: String,
    client_data_json: String,
    attestation_object: String,
}

pub async fn passkey_register(State(app): S, Extension(d): Extension<DeviceCtx>, Json(b): Json<Register>) -> Response {
    answer(app.remote_passkey_register(&d, &b.id, &b.client_data_json, &b.attestation_object).map(|()| json!({ "ok": true })))
}

// ── Lists and details ────────────────────────────────────────────────────

pub async fn plans(State(app): S, Extension(_d): Extension<DeviceCtx>) -> Response {
    answer(app.remote_plans())
}

pub async fn merges(State(app): S, Extension(_d): Extension<DeviceCtx>) -> Response {
    answer(app.remote_merges())
}

pub async fn network(State(app): S, Extension(_d): Extension<DeviceCtx>) -> Response {
    answer(app.remote_network().await)
}

pub async fn stopped(State(app): S, Extension(_d): Extension<DeviceCtx>) -> Response {
    answer(app.remote_stopped().await)
}

async fn detail(app: &Arc<App>, action: Action, target: &str) -> Response {
    let r = async {
        app.l2_on()?;
        match app.remote_item(action, target).await? {
            Some(s) => {
                let mut v = s.view;
                v["digest"] = json!(s.digest);
                Ok(v)
            }
            None => Err(L2Error::not_found()),
        }
    };
    answer(r.await)
}

pub async fn plan(State(app): S, Extension(_d): Extension<DeviceCtx>, Path(id): Path<String>) -> Response {
    detail(&app, Action::PlanApprove, &id).await
}

pub async fn merge(State(app): S, Extension(_d): Extension<DeviceCtx>, Path(id): Path<String>) -> Response {
    detail(&app, Action::Merge, &id).await
}

pub async fn network_one(State(app): S, Extension(_d): Extension<DeviceCtx>, Path(id): Path<String>) -> Response {
    detail(&app, Action::NetworkRespond, &id).await
}

pub async fn stopped_one(State(app): S, Extension(_d): Extension<DeviceCtx>, Path(name): Path<String>) -> Response {
    detail(&app, Action::SandboxRemove, &name).await
}

// ── The challenge and the actions ────────────────────────────────────────

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChallengeBody {
    kind: Action,
    target: String,
    digest: String,
    #[serde(default)]
    option: Option<String>,
}

pub async fn challenge(State(app): S, Extension(d): Extension<DeviceCtx>, Json(b): Json<ChallengeBody>) -> Response {
    if b.target.len() > 128 || b.option.as_ref().is_some_and(|o| o.len() > 64) {
        return refuse(L2Error { status: 400, reason: "bad", error: "target or option too long".into(), retry_ms: None });
    }
    answer(app.remote_challenge(&d, b.kind, &b.target, &b.digest, b.option.as_deref()).await)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActBody {
    assertion: Assertion,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RespondBody {
    option: String,
    assertion: Assertion,
}

pub async fn plan_approve(State(app): S, Extension(d): Extension<DeviceCtx>, Path(id): Path<String>, Json(b): Json<ActBody>) -> Response {
    answer(app.remote_act(&d, Action::PlanApprove, &id, None, &b.assertion).await)
}

pub async fn plan_refuse(State(app): S, Extension(d): Extension<DeviceCtx>, Path(id): Path<String>, Json(b): Json<ActBody>) -> Response {
    answer(app.remote_act(&d, Action::PlanRefuse, &id, None, &b.assertion).await)
}

pub async fn merge_run(State(app): S, Extension(d): Extension<DeviceCtx>, Path(id): Path<String>, Json(b): Json<ActBody>) -> Response {
    answer(app.remote_act(&d, Action::Merge, &id, None, &b.assertion).await)
}

pub async fn network_respond(State(app): S, Extension(d): Extension<DeviceCtx>, Path(id): Path<String>, Json(b): Json<RespondBody>) -> Response {
    answer(app.remote_act(&d, Action::NetworkRespond, &id, Some(&b.option), &b.assertion).await)
}

pub async fn sandbox_remove(State(app): S, Extension(d): Extension<DeviceCtx>, Path(name): Path<String>, Json(b): Json<ActBody>) -> Response {
    answer(app.remote_act(&d, Action::SandboxRemove, &name, None, &b.assertion).await)
}

// ── The badge on the event stream ────────────────────────────────────────

/// How often the counts are looked at again (network requests have no event).
pub const BADGE_EVERY: Duration = Duration::from_secs(15);

/// `event: approvals` on a phone's stream while level 2 is on: numbers only,
/// at connect and when a count changes. Holds only a weak sender: the stream
/// ends when its own task ends (a `bye`), whatever this task is doing.
pub fn spawn_badge(app: Arc<App>, d: DeviceCtx, tx: &tokio::sync::mpsc::Sender<Sse>) {
    let weak = tx.downgrade();
    let mut bus = app.bus.subscribe();
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(BADGE_EVERY);
        let mut last: Option<Value> = None;
        loop {
            tokio::select! {
                _ = tick.tick() => {}
                ev = bus.recv() => match ev {
                    Ok(Event::Plan(_)) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                    Ok(_) => continue,
                    Err(_) => break,
                },
            }
            let on = app.remote_access.lock().unwrap().level2.enabled;
            if !on || app.remote_device(&d.id).is_none() {
                if weak.upgrade().is_none() {
                    break;
                }
                last = None;
                continue;
            }
            let counts = app.remote_approval_counts().await;
            if last.as_ref() == Some(&counts) {
                continue;
            }
            let Some(tx) = weak.upgrade() else { break };
            if tx.send(Sse::default().event("approvals").data(counts.to_string())).await.is_err() {
                break;
            }
            last = Some(counts);
        }
    });
}
