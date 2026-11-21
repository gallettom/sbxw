//! The remote listener (`remote_addr`): what a paired phone may do, and
//! nothing else. The contract is `.sbxw-artifacts/remote/API.md`; the threat
//! model is `docs/security.md#remote-access`.
//!
//! The router is built here from an explicit list ([`ROUTES`] is that list,
//! for the route-walk test): nothing of the UI router is mounted, merged or
//! shared. Its guard, in order: remote access is on; the `Host` is the
//! tailnet name the sidecar serves (or this listener's loopback address);
//! the `Origin` is that host's page (required on writes); the path has no
//! trick in it; a rate limit; the device cookie on every data route but the
//! two pairing ones. The UI token, the UI cookie and an agent token are never
//! read here.

use super::error::ApiError;
use super::remote_approvals as l2;
use crate::app::App;
use crate::svc::remote_access::{AuthError, DeviceCtx, Kick, Reason, Refusal};
use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Path, Query, Request, State};
use axum::http::{header, HeaderMap, HeaderValue, Method, StatusCode};
use axum::middleware::Next;
use axum::response::sse::{Event as Sse, KeepAlive};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Extension, Json, Router};
use sbxw_core::remote_access::journal::Kind;
use sbxw_core::remote_access::pairing::Status;
use sbxw_core::remote_access::{self as rules, COOKIE};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;

include!(concat!(env!("OUT_DIR"), "/remote_page.rs"));

type S = State<Arc<App>>;

/// JSON bodies, at most.
pub const BODY_MAX: usize = 4 * 1024;
/// A message's body: 16 KiB of text plus its JSON.
pub const MESSAGE_BODY_MAX: usize = 20 * 1024;
/// A message's text.
pub const MESSAGE_MAX: usize = 16 * 1024;
/// Audio: the UI route's cap.
pub const AUDIO_BODY_MAX: usize = crate::svc::voice::MAX_SAMPLES * 4 + 1024;

/// Every route of this listener, `(method, path)`. The route-walk test holds
/// the router to this list; `/index.html` and the page's files are below.
pub const ROUTES: &[(&str, &str)] = &[
    ("GET", "/index.html"),
    ("POST", "/r/pair/start"),
    ("GET", "/r/pair/status"),
    ("GET", "/r/me"),
    ("POST", "/r/logout"),
    ("GET", "/r/leaders"),
    ("GET", "/r/leaders/:sandbox/history"),
    ("GET", "/r/events"),
    ("POST", "/r/leaders/:sandbox/message"),
    ("POST", "/r/leaders/:sandbox/answer"),
    ("POST", "/r/leaders/:sandbox/interrupt"),
    ("GET", "/r/models"),
    ("POST", "/r/leaders/:sandbox/model"),
    ("POST", "/r/leaders/:sandbox/lead"),
    ("POST", "/r/voice/transcribe"),
    ("POST", "/r/leaders/:sandbox/upload"),
    ("DELETE", "/r/leaders/:sandbox/upload/:id"),
    ("GET", "/r/leaders/:sandbox/kanban"),
    ("GET", "/r/leaders/:sandbox/kanban/:ticket"),
    ("POST", "/r/leaders/:sandbox/kanban"),
    // Level 2 (API-level2.md): approving with a passkey.
    ("GET", "/r/approvals"),
    ("POST", "/r/passkey/start"),
    ("GET", "/r/passkey/status"),
    ("POST", "/r/passkey/options"),
    ("POST", "/r/passkey/register"),
    ("GET", "/r/plans"),
    ("GET", "/r/plans/:id"),
    ("POST", "/r/plans/:id/approve"),
    ("POST", "/r/plans/:id/refuse"),
    ("GET", "/r/merges"),
    ("GET", "/r/merges/:id"),
    ("POST", "/r/merges/:id/merge"),
    ("GET", "/r/network"),
    ("GET", "/r/network/:id"),
    ("POST", "/r/network/:id/respond"),
    ("GET", "/r/stopped"),
    ("GET", "/r/stopped/:sandbox"),
    ("POST", "/r/stopped/:sandbox/remove"),
    ("POST", "/r/approvals/challenge"),
    // Web Push (API.md 2.8): level 1, the device cookie and the Origin.
    ("GET", "/r/push/key"),
    ("POST", "/r/push/subscribe"),
    ("POST", "/r/push/unsubscribe"),
];

/// Data routes a phone may call before it is paired.
const UNPAIRED: &[&str] = &["/r/pair/start", "/r/pair/status"];

pub fn remote_router(app: Arc<App>) -> Router {
    let mut r = Router::new();
    for (path, _, _) in REMOTE_PAGE {
        r = r.route(path, get(page));
    }
    r.route("/index.html", get(page))
        .route("/r/pair/start", post(pair_start))
        .route("/r/pair/status", get(pair_status))
        .route("/r/me", get(me))
        .route("/r/logout", post(logout))
        .route("/r/leaders", get(leaders))
        .route("/r/leaders/:sandbox/history", get(history))
        .route("/r/events", get(events))
        .route("/r/leaders/:sandbox/message", post(message).layer(DefaultBodyLimit::max(MESSAGE_BODY_MAX)))
        .route("/r/leaders/:sandbox/answer", post(answer))
        .route("/r/leaders/:sandbox/interrupt", post(interrupt))
        .route("/r/models", get(models))
        .route("/r/leaders/:sandbox/model", post(set_model))
        .route("/r/leaders/:sandbox/lead", post(set_lead))
        .route("/r/voice/transcribe", post(transcribe).layer(DefaultBodyLimit::max(AUDIO_BODY_MAX)))
        // The body is streamed and counted by `remote_upload` itself
        // (25 MiB); no extractor buffers it.
        .route("/r/leaders/:sandbox/upload", post(upload))
        .route("/r/leaders/:sandbox/upload/:id", delete(upload_drop))
        .route("/r/leaders/:sandbox/kanban", get(super::remote_kanban::board).post(super::remote_kanban::create).layer(DefaultBodyLimit::max(super::remote_kanban::BODY_MAX)))
        .route("/r/leaders/:sandbox/kanban/:ticket", get(super::remote_kanban::ticket))
        // Level 2 (API-level2.md): approving with a passkey.
        .route("/r/approvals", get(l2::approvals))
        .route("/r/passkey/start", post(l2::passkey_start))
        .route("/r/passkey/status", get(l2::passkey_status))
        .route("/r/passkey/options", post(l2::passkey_options))
        .route("/r/passkey/register", post(l2::passkey_register).layer(DefaultBodyLimit::max(l2::REGISTER_BODY_MAX)))
        .route("/r/plans", get(l2::plans))
        .route("/r/plans/:id", get(l2::plan))
        .route("/r/plans/:id/approve", post(l2::plan_approve))
        .route("/r/plans/:id/refuse", post(l2::plan_refuse))
        .route("/r/merges", get(l2::merges))
        .route("/r/merges/:id", get(l2::merge))
        .route("/r/merges/:id/merge", post(l2::merge_run))
        .route("/r/network", get(l2::network))
        .route("/r/network/:id", get(l2::network_one))
        .route("/r/network/:id/respond", post(l2::network_respond))
        .route("/r/stopped", get(l2::stopped))
        .route("/r/stopped/:sandbox", get(l2::stopped_one))
        .route("/r/stopped/:sandbox/remove", post(l2::sandbox_remove))
        .route("/r/approvals/challenge", post(l2::challenge))
        .route("/r/push/key", get(push_key))
        .route("/r/push/subscribe", post(push_subscribe))
        .route("/r/push/unsubscribe", post(push_unsubscribe))
        .fallback(not_found)
        .layer(DefaultBodyLimit::max(BODY_MAX))
        .layer(axum::middleware::from_fn_with_state(app.clone(), remote_guard))
        .with_state(app)
}

async fn not_found() -> ApiError {
    ApiError::not_found("not found")
}

pub(super) fn refusal(r: Refusal) -> Response {
    match r {
        Refusal::NotFound => ApiError::not_found("not found").into_response(),
        Refusal::Reason(r) => {
            let status = StatusCode::from_u16(r.http_status()).unwrap_or(StatusCode::BAD_REQUEST);
            (status, Json(json!({ "ok": false, "error": r.sentence(), "reason": r.code() }))).into_response()
        }
        Refusal::Conflict(m) => ApiError::conflict(m).into_response(),
        Refusal::Forbidden(m) => ApiError::forbidden(m).into_response(),
        Refusal::Unavailable(m) => ApiError::new(StatusCode::SERVICE_UNAVAILABLE, m).into_response(),
        Refusal::Bad(m) => ApiError::bad(m).into_response(),
        Refusal::Limited(ms) => limited(ms),
        Refusal::Failed(m) => ApiError::new(StatusCode::BAD_GATEWAY, m).into_response(),
    }
}

fn limited(retry_ms: u64) -> Response {
    let secs = retry_ms.div_ceil(1000).max(1).to_string();
    let mut res = ApiError::new(StatusCode::TOO_MANY_REQUESTS, "too many requests, try again later").into_response();
    if let Ok(v) = HeaderValue::from_str(&secs) {
        res.headers_mut().insert(header::RETRY_AFTER, v);
    }
    res
}

/// The device cookie, the only token carrier on this listener.
fn device_cookie(h: &HeaderMap) -> Option<String> {
    h.get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .find_map(|kv| kv.trim().strip_prefix(COOKIE)?.strip_prefix('=').map(str::to_string))
}

fn source(h: &HeaderMap) -> String {
    rules::source_of(h.get("x-forwarded-for").and_then(|v| v.to_str().ok()))
}

/// A path this listener could be tricked by: an encoded byte, a dot
/// segment, an empty segment. None of its routes needs one.
fn tricky(path: &str) -> bool {
    path.contains('%') || path.contains("//") || path.split('/').any(|s| s == "." || s == "..") || path.contains('\\')
}

pub async fn remote_guard(State(app): S, mut req: Request, next: Next) -> Response {
    let path = req.uri().path().to_string();
    let data = path.starts_with("/r/");
    let res = match guarded(&app, &mut req, &path, data).await {
        Ok(()) => next.run(req).await,
        Err(r) => *r,
    };
    with_headers(res, data)
}

/// `Err` is the refusal to send, boxed (a response is large).
async fn guarded(app: &Arc<App>, req: &mut Request, path: &str, data: bool) -> Result<(), Box<Response>> {
    if !app.remote_enabled() {
        return Err(Box::new(ApiError::not_found("not found").into_response()));
    }
    let h = req.headers();
    let host = h.get(header::HOST).and_then(|v| v.to_str().ok());
    let origin = h.get(header::ORIGIN).and_then(|v| v.to_str().ok());
    let safe = req.method() == Method::GET || req.method() == Method::HEAD;
    let (allowed, tailnet) = app.remote_hosts();
    if let Err(why) = rules::check_host_origin(&allowed, tailnet.as_deref(), host, origin, safe) {
        let shown: String = host.unwrap_or("").chars().take(80).collect();
        tracing::info!("remote access: refused {} (host {shown:?}): {why}", req.method());
        return Err(Box::new(ApiError::forbidden(why).into_response()));
    }
    if tricky(path) {
        return Err(Box::new(ApiError::not_found("not found").into_response()));
    }
    if !data || UNPAIRED.contains(&path) {
        return Ok(());
    }
    let src = source(h);
    match app.remote_auth(&src, device_cookie(h).as_deref()) {
        Ok(d) => {
            req.extensions_mut().insert(d);
            Ok(())
        }
        Err(AuthError::Limited(ms)) => Err(Box::new(limited(ms))),
        Err(AuthError::NotPaired) => Err(Box::new(ApiError::unauthorized("not paired").into_response())),
    }
}

/// Every answer of this listener: a strict policy for the page, no framing,
/// no sniffing, no referrer, nothing cached on `/r/`, never a CORS header.
fn with_headers(mut res: Response, data: bool) -> Response {
    let h = res.headers_mut();
    for (name, value) in [
        (header::CONTENT_SECURITY_POLICY, "default-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'self'"),
        (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        (header::REFERRER_POLICY, "no-referrer"),
        (header::X_FRAME_OPTIONS, "DENY"),
    ] {
        h.insert(name, HeaderValue::from_static(value));
    }
    if data {
        h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    }
    let cors: Vec<_> = h.keys().filter(|k| k.as_str().starts_with("access-control-")).cloned().collect();
    for k in cors {
        h.remove(k);
    }
    res
}

// ── The page ─────────────────────────────────────────────────────────────

async fn page(uri: axum::http::Uri) -> Response {
    let path = if uri.path() == "/index.html" { "/" } else { uri.path() };
    // The service worker is never cached: a new daemon's worker replaces
    // the old one at the next visit.
    let cache = if path == "/sw.js" { "no-store" } else { "no-cache" };
    match REMOTE_PAGE.iter().find(|(p, _, _)| *p == path) {
        Some((_, ctype, bytes)) => ([(header::CONTENT_TYPE, *ctype), (header::CACHE_CONTROL, cache)], *bytes).into_response(),
        None => ApiError::not_found("not found").into_response(),
    }
}

// ── Pairing ──────────────────────────────────────────────────────────────

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PairStart {
    code: String,
    device_name: String,
}

async fn pair_start(State(app): S, headers: HeaderMap, Json(b): Json<PairStart>) -> Response {
    if b.code.len() > 128 {
        return refusal(Refusal::Forbidden("this pairing code is not valid".into()));
    }
    match app.remote_pair_start(&source(&headers), &b.code, &b.device_name) {
        Ok(r) => Json(json!({ "request_id": r.id, "check": r.check, "expires_ms": r.expires_ms })).into_response(),
        Err(e) => refusal(e),
    }
}

#[derive(Deserialize)]
struct StatusQuery {
    request_id: String,
}

async fn pair_status(State(app): S, headers: HeaderMap, Query(q): Query<StatusQuery>) -> Response {
    if q.request_id.len() > 64 {
        return Json(json!({ "state": "expired" })).into_response();
    }
    match app.remote_pair_status(&source(&headers), &q.request_id) {
        Ok((Status::Approved { .. }, Some((d, token)))) => {
            let cookie = format!("{COOKIE}={token}; HttpOnly; Secure; SameSite=Strict; Path=/; Max-Age={}", rules::COOKIE_MAX_AGE_S);
            let mut res = Json(json!({ "state": "paired", "device": { "id": d.id, "name": d.name } })).into_response();
            if let Ok(v) = HeaderValue::from_str(&cookie) {
                res.headers_mut().insert(header::SET_COOKIE, v);
            }
            res
        }
        Ok((s, _)) => {
            let state = match s {
                Status::Pending => "pending",
                Status::Refused => "refused",
                _ => "expired",
            };
            Json(json!({ "state": state })).into_response()
        }
        Err(e) => refusal(e),
    }
}

// ── Session ──────────────────────────────────────────────────────────────

async fn me(State(app): S, Extension(d): Extension<DeviceCtx>) -> Response {
    match app.remote_device(&d.id) {
        Some(v) => Json(json!({ "device": v })).into_response(),
        None => ApiError::unauthorized("not paired").into_response(),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Empty {}

async fn logout(State(app): S, Extension(d): Extension<DeviceCtx>, Json(_): Json<Empty>) -> Response {
    app.remote_revoke(&d.id, Kind::Logout);
    let clear = format!("{COOKIE}=; HttpOnly; Secure; SameSite=Strict; Path=/; Max-Age=0");
    let mut res = Json(json!({ "ok": true })).into_response();
    if let Ok(v) = HeaderValue::from_str(&clear) {
        res.headers_mut().insert(header::SET_COOKIE, v);
    }
    res
}

// ── Leaders ──────────────────────────────────────────────────────────────

async fn leaders(State(app): S, Extension(_d): Extension<DeviceCtx>) -> Json<Value> {
    Json(json!({ "leaders": app.remote_leaders() }))
}

#[derive(Deserialize)]
struct HistoryQuery {
    before: Option<u64>,
    after: Option<u64>,
    limit: Option<usize>,
}

async fn history(State(app): S, Extension(_d): Extension<DeviceCtx>, Path(sandbox): Path<String>, Query(q): Query<HistoryQuery>) -> Response {
    let sb = match app.remote_target(&sandbox) {
        Ok(sb) => sb,
        Err(e) => return refusal(e),
    };
    match app.remote_history(&sb, q.before, q.after, q.limit) {
        Ok((sid, page)) => Json(json!({ "sandbox": sb.to_string(), "session_id": sid, "page": page })).into_response(),
        Err(e) => refusal(e),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MessageBody {
    #[serde(default)]
    text: String,
    /// Ids from `upload` (API.md 2.6.1); the daemon resolves them.
    #[serde(default)]
    attachments: Vec<String>,
}

async fn message(State(app): S, Extension(d): Extension<DeviceCtx>, Path(sandbox): Path<String>, Json(b): Json<MessageBody>) -> Response {
    if b.text.len() > MESSAGE_MAX {
        return refusal(Refusal::Bad(format!("a message is at most {MESSAGE_MAX} bytes")));
    }
    if b.attachments.len() > sbxw_core::remote_access::uploads::WAITING_MAX {
        return refusal(Refusal::Reason(Reason::TooManyIds));
    }
    let sb = match app.remote_target(&sandbox) {
        Ok(sb) => sb,
        Err(e) => return refusal(e),
    };
    match app.remote_message(&d, &sb, &b.text, &b.attachments).await {
        Ok(masked) => Json(json!({ "ok": true, "masked": masked })).into_response(),
        Err(e) => refusal(e),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AnswerBody {
    indices: Vec<u32>,
}

async fn answer(State(app): S, Extension(d): Extension<DeviceCtx>, Path(sandbox): Path<String>, Json(b): Json<AnswerBody>) -> Response {
    let sb = match app.remote_target(&sandbox) {
        Ok(sb) => sb,
        Err(e) => return refusal(e),
    };
    match app.remote_answer(&d, &sb, b.indices).await {
        Ok(()) => Json(json!({ "ok": true })).into_response(),
        Err(e) => refusal(e),
    }
}

async fn interrupt(State(app): S, Extension(d): Extension<DeviceCtx>, Path(sandbox): Path<String>, Json(_): Json<Empty>) -> Response {
    let sb = match app.remote_target(&sandbox) {
        Ok(sb) => sb,
        Err(e) => return refusal(e),
    };
    match app.remote_interrupt(&d, &sb).await {
        Ok(()) => Json(json!({ "ok": true })).into_response(),
        Err(e) => refusal(e),
    }
}

async fn models(State(app): S, Extension(_d): Extension<DeviceCtx>) -> Json<Value> {
    Json(app.remote_models())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ModelBody {
    id: String,
}

/// An id of the configured list, never text: the daemon types `/model <id>`.
async fn set_model(State(app): S, Extension(d): Extension<DeviceCtx>, Path(sandbox): Path<String>, Json(b): Json<ModelBody>) -> Response {
    let sb = match app.remote_target(&sandbox) {
        Ok(sb) => sb,
        Err(e) => return refusal(e),
    };
    match app.remote_set_model(&d, &sb, &b.id).await {
        Ok(()) => Json(json!({ "ok": true })).into_response(),
        Err(e) => refusal(e),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LeadBody {
    on: bool,
}

async fn set_lead(State(app): S, Extension(d): Extension<DeviceCtx>, Path(sandbox): Path<String>, Json(b): Json<LeadBody>) -> Response {
    let sb = match app.remote_target(&sandbox) {
        Ok(sb) => sb,
        Err(e) => return refusal(e),
    };
    match app.remote_set_lead(&d, &sb, b.on).await {
        Ok(()) => Json(json!({ "ok": true })).into_response(),
        Err(e) => refusal(e),
    }
}

async fn transcribe(State(app): S, Extension(d): Extension<DeviceCtx>, headers: HeaderMap, body: Bytes) -> Response {
    let octets = headers.get(header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).is_some_and(|c| c == "application/octet-stream");
    if !octets {
        return ApiError::new(StatusCode::UNSUPPORTED_MEDIA_TYPE, "audio is sent as application/octet-stream").into_response();
    }
    match app.remote_transcribe(&d, &body).await {
        Ok(safe) => Json(json!({ "text": safe.text, "masked": safe.count })).into_response(),
        Err(e) => refusal(e),
    }
}

#[derive(Deserialize)]
struct UploadQuery {
    #[serde(default)]
    name: String,
}

/// A file from the phone: the raw bytes, streamed (API.md 2.6.1). Never a
/// path in the answer.
async fn upload(State(app): S, Extension(d): Extension<DeviceCtx>, Path(sandbox): Path<String>, Query(q): Query<UploadQuery>, req: Request) -> Response {
    let headers = req.headers();
    let octets = headers.get(header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).is_some_and(|c| c == "application/octet-stream");
    if !octets {
        return ApiError::new(StatusCode::UNSUPPORTED_MEDIA_TYPE, "a file is sent as application/octet-stream").into_response();
    }
    let declared = headers.get(header::CONTENT_LENGTH).and_then(|v| v.to_str().ok()).and_then(|v| v.parse::<u64>().ok());
    let sb = match app.remote_target(&sandbox) {
        Ok(sb) => sb,
        Err(e) => return refusal(e),
    };
    let body = req.into_body().into_data_stream();
    match app.remote_upload(&d, &sb, &q.name, declared, body).await {
        Ok(v) => Json(json!(v)).into_response(),
        Err(e) => refusal(e),
    }
}

async fn upload_drop(State(app): S, Extension(d): Extension<DeviceCtx>, Path((sandbox, id)): Path<(String, String)>) -> Response {
    let sb = match app.remote_target(&sandbox) {
        Ok(sb) => sb,
        Err(e) => return refusal(e),
    };
    if id.len() != 32 || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
        return refusal(Refusal::NotFound);
    }
    match app.remote_upload_drop(&d, &sb, &id) {
        Ok(()) => Json(json!({ "ok": true })).into_response(),
        Err(e) => refusal(e),
    }
}

// ── Web Push (API.md 2.8) ────────────────────────────────────────────────

/// `{"key", "on"}`: the Mac's VAPID public key, and whether THIS device
/// has notifications on.
async fn push_key(State(app): S, Extension(d): Extension<DeviceCtx>) -> Json<Value> {
    let (key, on) = app.remote_push_key(&d.id);
    Json(json!({ "key": key, "on": on }))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SubscribeBody {
    endpoint: String,
    p256dh: String,
    auth: String,
}

/// A forged endpoint or key: `400` with one reason word, nothing stored.
async fn push_subscribe(State(app): S, Extension(d): Extension<DeviceCtx>, Json(b): Json<SubscribeBody>) -> Response {
    use crate::svc::remote_push::SubscribeError;
    match app.remote_push_subscribe(&d.id, &b.endpoint, &b.p256dh, &b.auth) {
        Ok(()) => Json(json!({ "ok": true })).into_response(),
        Err(SubscribeError::Refused(r)) => (StatusCode::BAD_REQUEST, Json(json!({ "ok": false, "error": "this subscription is refused", "reason": r.word() }))).into_response(),
        Err(SubscribeError::Gone) => ApiError::unauthorized("not paired").into_response(),
    }
}

async fn push_unsubscribe(State(app): S, Extension(d): Extension<DeviceCtx>, Json(_): Json<Empty>) -> Json<Value> {
    app.remote_push_off(&d.id);
    Json(json!({ "ok": true }))
}

// ── Events ───────────────────────────────────────────────────────────────

/// How often an open stream renews its leaders' history subscriptions and
/// checks its device is still paired.
const RENEW: Duration = Duration::from_secs(60);

async fn events(State(app): S, Extension(d): Extension<DeviceCtx>) -> Response {
    use crate::bus::Event;
    use futures_util::StreamExt;
    let (tx, rx) = tokio::sync::mpsc::channel::<Sse>(32);
    let mut bus = app.bus.subscribe();
    let mut kicks = app.remote_kicks();
    let leaders_frame = |l: &Vec<crate::svc::remote_access::LeaderView>| Sse::default().event("leaders").data(json!({ "leaders": l }).to_string());
    let mut last = app.remote_leaders();
    let mut kanban_seen = super::remote_kanban::baseline(&app, &last);
    let _ = tx.try_send(leaders_frame(&last));
    l2::spawn_badge(app.clone(), d.clone(), &tx);
    let (app_for_cap, d_for_cap) = (app.clone(), d.id.clone());
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(RENEW);
        loop {
            let bye = |reason: &str| Sse::default().event("bye").data(json!({ "reason": reason }).to_string());
            tokio::select! {
                _ = tx.closed() => break,
                k = kicks.recv() => match k {
                    Ok(Kick::All) | Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        let _ = tx.send(bye("disabled")).await;
                        break;
                    }
                    Ok(Kick::Device(id)) if id == d.id => {
                        let _ = tx.send(bye("revoked")).await;
                        break;
                    }
                    // Kicks were missed: look again rather than trust the gap.
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                        if !app.remote_enabled() || app.remote_device(&d.id).is_none() {
                            let _ = tx.send(bye("revoked")).await;
                            break;
                        }
                    }
                    Ok(Kick::Device(_)) => {}
                },
                _ = tick.tick() => {
                    if !app.remote_enabled() || app.remote_device(&d.id).is_none() {
                        let _ = tx.send(bye("revoked")).await;
                        break;
                    }
                    for l in &last {
                        if let Ok(sb) = sbxw_core::SandboxRef::parse(&l.sandbox) {
                            if let Some(sid) = app.remote_leader_session(&sb) {
                                let _ = app.history_page(&sb, &sid, None, Some(1));
                            }
                        }
                    }
                }
                ev = bus.recv() => match ev {
                    Ok(Event::Session(_) | Event::Lead(_) | Event::LeadEnd(_)) => {
                        let now = app.remote_leaders();
                        if now != last {
                            for l in &now {
                                let was = last.iter().find(|o| o.sandbox == l.sandbox).is_some_and(|o| o.asks);
                                if l.asks && !was {
                                    let _ = tx.send(Sse::default().event("needs_you").data(json!({ "sandbox": l.sandbox }).to_string())).await;
                                }
                            }
                            if tx.send(leaders_frame(&now)).await.is_err() {
                                break;
                            }
                            last = now;
                        }
                    }
                    Ok(Event::History(n)) => {
                        let ours = last.iter().any(|l| l.sandbox == n.sandbox)
                            && sbxw_core::SandboxRef::parse(&n.sandbox).ok().and_then(|sb| app.remote_leader_session(&sb)).as_deref() == Some(n.session_id.as_str());
                        if ours && tx.send(Sse::default().event("history").data(json!({ "sandbox": n.sandbox, "seq": n.seq }).to_string())).await.is_err() {
                            break;
                        }
                    }
                    Ok(ev @ (Event::Tickets(_) | Event::Plan(_))) => {
                        if let Some(f) = super::remote_kanban::changed(&app, &last, &ev, &mut kanban_seen) {
                            if tx.send(f).await.is_err() {
                                break;
                            }
                        }
                    }
                    Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                    Err(_) => break,
                },
            }
        }
    });
    let stream = tokio_stream::wrappers::ReceiverStream::new(rx).map(Ok::<_, std::convert::Infallible>);
    let stream = l2::capped(&app_for_cap, &d_for_cap, stream);
    axum::response::Sse::new(stream).keep_alive(KeepAlive::new().interval(Duration::from_secs(15))).into_response()
}
