//! The UI listener's handlers. Thin on purpose: parse, call a service, shape
//! the answer the page already reads.

use super::error::{ok, ok_with, ApiError, ApiResult};
use crate::app::App;
use crate::provision::BringUp;
use crate::sandbox_fs;
use axum::body::{Body, Bytes};
use axum::extract::{DefaultBodyLimit, Path, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use sbxw_core::artifacts;
use sbxw_core::config::Config;
use sbxw_core::favourites;
use sbxw_core::plans::{DistillItem, PlanThread, Variant};
use sbxw_core::policy;
use sbxw_core::ports::WantedPort;
use sbxw_core::progress::Progress;
use sbxw_core::session::{SessionKey, SessionKind};
use sbxw_core::{names, ModelId, SandboxName, ThreadName};
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Arc;

type S = State<Arc<App>>;

fn sandbox(name: &str) -> Result<SandboxName, ApiError> {
    Ok(SandboxName::parse(name)?)
}

fn non_blank(s: Option<String>) -> Option<String> {
    s.map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

pub fn routes() -> Router<Arc<App>> {
    Router::new()
        // Sessions, hooks' view, terminals.
        .route("/api/sessions", get(sessions))
        .route("/api/ptys", get(ptys))
        .route("/api/input", post(input))
        .route("/api/answer", post(answer))
        .route("/api/hook/log", get(hook_log))
        .route("/api/usage", get(usage))
        .route("/api/models", get(models))
        .route("/api/leads", get(leads))
        // Sandboxes.
        .route("/api/sandboxes", get(list))
        .route("/api/sandboxes/create", post(create))
        .route("/api/sandboxes/chat", post(chat))
        .route("/api/chat/push", post(chat_push))
        .route("/api/sandboxes/:name/up", post(up))
        .route("/api/sandboxes/:name/detach", post(detach))
        .route("/api/sandboxes/:name/duplicate", post(duplicate))
        .route("/api/sandboxes/:name/stop", post(stop))
        .route("/api/sandboxes/:name/rm", post(rm))
        .route("/api/sandboxes/:name/ports", get(ports))
        .route("/api/sandboxes/:name/ports/publish", post(publish))
        .route("/api/sandboxes/:name/ports/unpublish", post(unpublish))
        .route("/api/sandboxes/:name/policy", get(policy_panel))
        .route("/api/sandboxes/:name/policy/rules", post(policy_add))
        .route("/api/sandboxes/:name/policy/rules/rm", post(policy_rm))
        .route("/api/policy/refresh", post(policy_refresh))
        .route("/api/template/status", get(template_status))
        .route("/api/kit-mounts", get(kit_mounts))
        .route("/api/templates", get(templates_list))
        .route("/api/templates/save", post(template_save))
        .route("/api/templates/rm", post(template_rm))
        .route("/api/sandboxes/:name/mcp", get(mcp_list))
        .route("/api/sandboxes/:name/mcp/unload", post(mcp_unload))
        .route("/api/sandboxes/:name/envfile", get(envfile).post(envfile))
        .route(
            "/api/sandboxes/:name/paste-image",
            post(paste_image).layer(DefaultBodyLimit::max(32 * 1024 * 1024)),
        )
        .route(
            "/api/sandboxes/:name/upload",
            post(upload).layer(DefaultBodyLimit::max(UPLOAD_MAX)),
        )
        .route("/api/hosts", get(hosts))
        // Threads and lead.
        .route("/api/sandboxes/:name/threads", get(threads))
        .route("/api/sandboxes/:name/repos", get(repos))
        .route("/api/sandboxes/:name/worktrees", get(worktrees))
        .route("/api/sandboxes/:name/threads/:thread", get(thread_status))
        .route("/api/sandboxes/:name/threads/:thread/close", post(thread_close))
        .route("/api/sandboxes/:name/lead", post(lead))
        .route("/api/sandboxes/:name/remote-control", get(remote_state).post(remote_control))
        .route("/api/sandboxes/:name/remote-control/login", post(remote_login))
        .route("/api/sandboxes/:name/lead/end", post(lead_end))
        // Plans (the browser's half).
        .route("/api/plans", get(plans))
        .route("/api/plans/:id/approve", post(plan_approve))
        .route("/api/plans/:id/refuse", post(plan_refuse))
        .route("/api/plans/:id/merge", post(plan_merge))
        // Relay (the human's half).
        .route("/api/relay", get(relay_list))
        .route("/api/relay/:id/route", post(relay_route))
        .route(
            "/api/relay/:id/approve",
            post(relay_approve).layer(DefaultBodyLimit::max(24 * 1024 * 1024)),
        )
        .route("/api/relay/:id/deny", post(relay_deny))
        // Files.
        .route("/api/fs", get(fs_browse))
        .route("/api/fs/pick", post(fs_pick))
        .route("/api/fs/favourites", get(favourites_get).post(favourites_set))
        .route("/api/embed/resolve", get(embed_resolve))
        .route("/api/embed/pairings", get(pairings).post(pair))
        .route("/api/embed/pairings/rm", post(unpair))
        .route("/api/sandboxes/:name/artifacts", get(artifacts_list))
        .route("/api/sandboxes/:name/artifacts/download", get(artifact_download))
        .route("/api/sandboxes/:name/fs", get(sandbox_fs_list))
        .route("/api/sandboxes/:name/fs/file", get(sandbox_fs_file))
        .route("/api/sandboxes/:name/fs/recent", get(sandbox_fs_recent))
        .route("/api/sandboxes/:name/fs/delete", post(sandbox_fs_delete))
        // Code map runs.
        .route("/api/sandboxes/:name/codemap", post(codemap_start))
        .route("/api/sandboxes/:name/codemap/lens", post(codemap_lens))
        .route("/api/codemap", get(codemap_runs))
        .merge(super::board::ui_routes())
        // The daemon itself.
        .route("/api/health", get(health))
        .route("/api/state", get(state))
        .route("/api/initial", post(set_initial))
        .route("/api/shutdown", post(shutdown))
}

// ── Sessions ─────────────────────────────────────────────────────────────

async fn sessions(State(app): S) -> Json<Value> {
    Json(json!(app.session_infos()))
}

async fn ptys(State(app): S) -> Json<Value> {
    let list: Vec<Value> = app
        .sessions
        .all()
        .into_iter()
        .map(|(key, s)| {
            let size = s.size();
            json!({ "key": key.to_string(), "cols": size.map(|z| z.0), "rows": size.map(|z| z.1), "alive": s.alive() })
        })
        .collect();
    Json(json!(list))
}

#[derive(Deserialize)]
struct Addressed {
    sandbox: String,
    mode: Option<String>,
}

impl Addressed {
    fn key(&self) -> Result<(SandboxName, SessionKind), ApiError> {
        let sb = sandbox(&self.sandbox)?;
        let (mode, thread) = match self.mode.as_deref() {
            Some(m) if m.starts_with("thread:") => (Some("thread"), m.strip_prefix("thread:")),
            Some(m) if m.starts_with("thread-bash:") => (Some("bash"), m.strip_prefix("thread-bash:")),
            m => (m, None),
        };
        let kind = SessionKind::from_query(mode, thread).map_err(ApiError::bad)?;
        Ok((sb, kind))
    }
}

#[derive(Deserialize)]
struct InputBody {
    #[serde(flatten)]
    at: Addressed,
    data: String,
}

/// Raw bytes into a terminal the caller names.
async fn input(State(app): S, Json(b): Json<InputBody>) -> StatusCode {
    let Ok((sb, kind)) = b.at.key() else { return StatusCode::BAD_REQUEST };
    if app.input(&sb, kind, b.data.as_bytes()).await {
        StatusCode::OK
    } else {
        StatusCode::NOT_FOUND
    }
}

#[derive(Deserialize)]
struct AnswerBody {
    #[serde(flatten)]
    at: Addressed,
    index: Option<u32>,
    indices: Option<Vec<u32>>,
}

async fn answer(State(app): S, Json(b): Json<AnswerBody>) -> StatusCode {
    use crate::svc::agents::AnswerError;
    let Ok((sb, kind)) = b.at.key() else { return StatusCode::BAD_REQUEST };
    let indices = match (b.indices, b.index) {
        (Some(l), _) if !l.is_empty() => l,
        (_, Some(i)) => vec![i],
        _ => vec![],
    };
    match app.answer(&sb, kind, indices).await {
        Ok(()) => StatusCode::OK,
        Err(AnswerError::NoSession) => StatusCode::NOT_FOUND,
        Err(AnswerError::NoIndices) => StatusCode::BAD_REQUEST,
        Err(AnswerError::Ambiguous(why)) => {
            tracing::warn!("refusing to answer '{sb}': {why}");
            StatusCode::CONFLICT
        }
    }
}

async fn hook_log(State(app): S) -> Json<Value> {
    Json(json!(app.hook_log()))
}

async fn usage(State(app): S) -> Json<Value> {
    Json(json!(app.usage()))
}

async fn models(State(app): S) -> Json<Value> {
    ok_with(json!({ "default": app.cfg.claude_model, "models": app.cfg.models }))
}

async fn leads(State(app): S) -> Json<Value> {
    Json(json!(app.leading()))
}

// ── Sandboxes ────────────────────────────────────────────────────────────

async fn list(State(app): S) -> Json<Value> {
    Json(json!(app.sandboxes().await))
}

fn warnings(w: Vec<String>) -> Json<Value> {
    if w.is_empty() {
        ok()
    } else {
        ok_with(json!({ "warnings": w }))
    }
}

#[derive(Deserialize)]
struct PortEntry {
    sandbox_port: u16,
    host_port: Option<u16>,
    #[serde(default)]
    alias: Option<String>,
}

impl PortEntry {
    fn wanted(self) -> Result<WantedPort, ApiError> {
        let alias = self.alias.unwrap_or_default().trim().to_string();
        if !alias.is_empty() && !sbxw_core::hosts::is_hostname(&alias) {
            return Err(ApiError::bad(format!("'{alias}' is not a hostname")));
        }
        Ok(WantedPort {
            sandbox_port: self.sandbox_port,
            host_port: self.host_port.unwrap_or(self.sandbox_port),
            alias,
        })
    }
}

/// A repo mounted beside the workspace, read-only unless asked otherwise.
#[derive(Deserialize)]
struct MountEntry {
    path: String,
    #[serde(default = "yes")]
    readonly: bool,
}

fn yes() -> bool {
    true
}

#[derive(Deserialize)]
struct CreateBody {
    name: String,
    path: String,
    #[serde(default)]
    ports: Vec<PortEntry>,
    #[serde(default)]
    mounts: Vec<MountEntry>,
    #[serde(default)]
    template: sbxw_core::templates::Choice,
    #[serde(default)]
    remote_control: bool,
    /// Sandbox target → host folder, for the kits' declared host mounts.
    #[serde(default)]
    kit_mounts: std::collections::BTreeMap<String, String>,
}

async fn create(State(app): S, Json(b): Json<CreateBody>) -> ApiResult {
    let name = sandbox(&b.name)?;
    let form = crate::svc::sandboxes::CreateForm {
        ports: b.ports.into_iter().map(PortEntry::wanted).collect::<Result<Vec<_>, _>>()?,
        mounts: b.mounts.into_iter().map(|m| (m.path, m.readonly)).collect(),
        template: b.template,
        remote_control: b.remote_control,
        kit_mounts: b.kit_mounts,
    };
    Ok(warnings(app.create(name, &b.path, form, None).await?))
}

async fn kit_mounts(State(app): S) -> Json<Value> {
    Json(json!({ "mounts": app.declared_host_mounts() }))
}

/// Whether the create dialog should ask about the automatic template.
async fn template_status(State(app): S) -> ApiResult {
    Ok(Json(json!(app.template_status())))
}

/// The saved templates, for the sidebar's Templates dialog.
async fn templates_list(State(app): S) -> ApiResult {
    Ok(ok_with(app.templates().await?))
}

#[derive(Deserialize)]
struct TemplateSaveBody {
    sandbox: String,
    tag: String,
}

async fn template_save(State(app): S, Json(b): Json<TemplateSaveBody>) -> ApiResult {
    let name = sandbox(&b.sandbox)?;
    let tag = policy::clean_arg(&b.tag, "tag")?;
    Ok(warnings(app.save_template(&name, &tag).await?))
}

#[derive(Deserialize)]
struct TemplateRmBody {
    tag: String,
}

async fn template_rm(State(app): S, Json(b): Json<TemplateRmBody>) -> ApiResult {
    let tag = policy::clean_arg(&b.tag, "tag")?;
    app.remove_template(&tag).await?;
    Ok(ok())
}

#[derive(Deserialize)]
struct ChatBody {
    #[serde(default)]
    name: Option<String>,
}

async fn chat(State(app): S, Json(b): Json<ChatBody>) -> ApiResult {
    let name = match non_blank(b.name) {
        Some(n) => sandbox(&n)?,
        None => names::mint_chat_name(u32::from_str_radix(&sbxw_core::ids::random_hex(4), 16).unwrap_or(0)),
    };
    let w = app.create_chat(name.clone()).await?;
    Ok(ok_with(json!({ "name": name, "warnings": w })))
}

#[derive(Deserialize)]
struct ChatPushBody {
    text: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    fresh: bool,
    #[serde(default)]
    session: bool,
    #[serde(default)]
    thread: Option<String>,
}

async fn chat_push(State(app): S, Json(b): Json<ChatPushBody>) -> ApiResult {
    let name = non_blank(b.name).map(|n| sandbox(&n)).transpose()?;
    let thread = non_blank(b.thread).map(|t| ThreadName::parse(&t)).transpose()?;
    let (name, created, thread) = app.chat_push(&b.text, name, b.fresh, b.session, thread).await?;
    Ok(ok_with(json!({ "name": name, "created": created, "thread": thread })))
}

/// The CLI's bring-up: the project's config, its credentials, streamed back
/// as NDJSON — every progress event, then one closing line.
#[derive(Deserialize)]
struct UpBody {
    path: PathBuf,
    #[serde(default)]
    ro: Vec<String>,
    #[serde(default)]
    rw: Vec<String>,
    #[serde(default)]
    config: Option<PathBuf>,
    #[serde(default)]
    chat: bool,
    #[serde(default)]
    api_key: Option<String>,
    #[serde(default)]
    oauth_token: Option<String>,
    #[serde(default)]
    extra_ports: Vec<WantedPort>,
    #[serde(default)]
    template: sbxw_core::templates::Choice,
    #[serde(default)]
    remote_control: bool,
}

async fn up(State(app): S, Path(name): Path<String>, Json(b): Json<UpBody>) -> Result<Response, ApiError> {
    let name = sandbox(&name)?;
    let config = match &b.config {
        Some(p) => Config::load(p, std::path::Path::new("/"))?,
        None => app.cfg.clone(),
    };
    let req = BringUp {
        name,
        workspace: b.path,
        ro: b.ro,
        rw: b.rw,
        config,
        extra_ports: b.extra_ports,
        chat: b.chat,
        api_key: b.api_key,
        oauth_token: b.oauth_token,
        template: b.template,
        remote_control: b.remote_control,
    };
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    let line = |v: Value| format!("{v}\n");
    let progress_tx = tx.clone();
    let sink: sbxw_core::progress::Sink = Arc::new(move |p: Progress| {
        let _ = progress_tx.send(line(json!({ "progress": p })));
    });
    tokio::spawn(async move {
        let out = app.provision(req, Some(sink)).await;
        let last = match out {
            Ok(w) => json!({ "done": true, "ok": true, "warnings": w }),
            Err(e) => json!({ "done": true, "ok": false, "error": format!("{e:#}") }),
        };
        let _ = tx.send(line(last));
    });
    let stream = tokio_stream::wrappers::UnboundedReceiverStream::new(rx).map(Ok::<_, std::convert::Infallible>);
    Ok(([(header::CONTENT_TYPE, "application/x-ndjson")], Body::from_stream(stream)).into_response())
}

use futures_util::StreamExt;

/// Close every terminal sbxw holds for a sandbox (`sbxw down <name>`).
async fn detach(State(app): S, Path(name): Path<String>) -> ApiResult {
    app.sessions.drop_sandbox(&sandbox(&name)?);
    Ok(ok())
}

#[derive(Deserialize)]
struct DuplicateBody {
    new_name: String,
}

async fn duplicate(State(app): S, Path(name): Path<String>, Json(b): Json<DuplicateBody>) -> ApiResult {
    let (src, new) = (sandbox(&name)?, sandbox(&b.new_name)?);
    Ok(warnings(app.duplicate(&src, new).await?))
}

async fn stop(State(app): S, Path(name): Path<String>) -> ApiResult {
    app.sbx.stop(&sandbox(&name)?).await?;
    Ok(ok())
}

async fn rm(State(app): S, Path(name): Path<String>) -> ApiResult {
    app.remove(&[sandbox(&name)?]).await?;
    Ok(ok())
}

async fn ports(State(app): S, Path(name): Path<String>) -> ApiResult {
    let list: Vec<Value> = app
        .sbx
        .ports(&sandbox(&name)?)
        .await
        .into_iter()
        .map(|p| json!({ "spec": p.spec(), "sandbox_port": p.sandbox_port, "proto": p.proto, "host_ip": p.host_ip, "host_port": p.host_port }))
        .collect();
    Ok(Json(json!({ "ports": list })))
}

#[derive(Deserialize)]
struct PublishBody {
    sandbox_port: u16,
    host_port: Option<u16>,
    host_ip: Option<String>,
    alias: Option<String>,
}

async fn publish(State(app): S, Path(name): Path<String>, Json(b): Json<PublishBody>) -> ApiResult {
    let w = app.publish_port(&sandbox(&name)?, b.sandbox_port, b.host_port, b.host_ip, b.alias).await?;
    Ok(match w {
        None => ok(),
        Some(w) => ok_with(json!({ "hosts_warning": w })),
    })
}

#[derive(Deserialize)]
struct SpecBody {
    spec: String,
}

async fn unpublish(State(app): S, Path(name): Path<String>, Json(b): Json<SpecBody>) -> ApiResult {
    let spec = policy::clean_arg(&b.spec, "spec")?;
    app.sbx.unpublish(&sandbox(&name)?, &spec).await?;
    Ok(ok())
}

async fn policy_panel(State(app): S, Path(name): Path<String>) -> ApiResult {
    Ok(Json(json!(app.policy_panel(&sandbox(&name)?).await)))
}

#[derive(Deserialize)]
struct RuleBody {
    resources: String,
    decision: Option<String>,
    global: Option<bool>,
    /// HTTP narrowing, as typed: comma-separated methods and `name=value`
    /// query pairs, a path glob.
    #[serde(default)]
    methods: String,
    #[serde(default)]
    path: String,
    #[serde(default)]
    query: String,
}

async fn policy_add(State(app): S, Path(name): Path<String>, Json(b): Json<RuleBody>) -> ApiResult {
    let sb = sandbox(&name)?;
    let resources = policy::clean_resources(&b.resources)?;
    let decision = match b.decision.as_deref().unwrap_or("allow") {
        d @ ("allow" | "deny") => d,
        other => return Err(ApiError::bad(format!("unknown decision '{other}' — use allow or deny"))),
    };
    let global = b.global.unwrap_or(false);
    let http = policy::HttpMatch::parse(&b.methods, &b.path, &b.query).map_err(ApiError::bad)?;
    tracing::info!(
        "web UI: adding {decision} network rule for {} — {resources} {}",
        if global { "ALL sandboxes".to_string() } else { format!("sandbox '{sb}'") },
        http.args().join(" ")
    );
    app.sbx.policy_http(decision, (!global).then_some(&sb), &resources, &http).await?;
    Ok(ok())
}

#[derive(Deserialize)]
struct RuleRmBody {
    rule: String,
}

async fn policy_rm(State(app): S, Path(name): Path<String>, Json(b): Json<RuleRmBody>) -> ApiResult {
    let sb = sandbox(&name)?;
    let rule = policy::clean_arg(&b.rule, "rule id")?;
    tracing::info!("web UI: removing policy rule {rule} (from sandbox '{sb}' panel)");
    app.sbx.policy_rm(&rule, Some(&sb)).await?;
    Ok(ok())
}

async fn policy_refresh(State(app): S) -> ApiResult {
    tracing::info!("web UI: refreshing network policy");
    app.sbx.policy_refresh().await?;
    Ok(ok())
}

/// The host's registered MCP servers; the sandbox only scopes the unload.
async fn mcp_list(State(app): S, Path(name): Path<String>) -> ApiResult {
    sandbox(&name)?;
    Ok(ok_with(json!({ "servers": app.sbx.mcp_ls().await? })))
}

#[derive(Deserialize)]
struct McpUnloadBody {
    server: String,
}

async fn mcp_unload(State(app): S, Path(name): Path<String>, Json(b): Json<McpUnloadBody>) -> ApiResult {
    let sb = sandbox(&name)?;
    let server = policy::clean_arg(&b.server, "server")?;
    tracing::info!("web UI: unloading MCP server {server} from '{sb}'");
    app.sbx.mcp_unload(&server, &sb).await?;
    Ok(ok())
}

#[derive(Deserialize, Default)]
struct EnvFileBody {
    #[serde(alias = "root")]
    dir: Option<String>,
    save: Option<bool>,
    force: Option<bool>,
}

async fn envfile(State(app): S, Path(name): Path<String>, body: Option<Json<EnvFileBody>>) -> ApiResult {
    let sb = sandbox(&name)?;
    let b = body.map(|Json(b)| b).unwrap_or_default();
    let dir = non_blank(b.dir).map(PathBuf::from);
    let r = app.render_env_file(&sb, dir.as_deref()).await?;
    let mut written = None;
    if b.save.unwrap_or(false) {
        if r.dest.exists() && !b.force.unwrap_or(false) {
            return Err(ApiError::conflict(format!("{} already exists — tick overwrite to replace it", r.dest.display())));
        }
        std::fs::write(&r.dest, &r.yaml).map_err(|e| ApiError::bad(format!("writing {}: {e}", r.dest.display())))?;
        tracing::info!("web UI: wrote environment file {}", r.dest.display());
        written = Some(r.dest.to_string_lossy().into_owned());
    }
    Ok(ok_with(json!({ "yaml": r.yaml, "dest": r.dest, "dir": r.dir, "written": written })))
}

async fn paste_image(State(app): S, Path(name): Path<String>, headers: HeaderMap, body: Bytes) -> ApiResult {
    let sb = sandbox(&name)?;
    if body.is_empty() {
        return Err(ApiError::bad("empty image"));
    }
    let ext = match headers.get(header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).unwrap_or("").split(';').next().unwrap_or("").trim() {
        "image/jpeg" => "jpg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "image/bmp" => "bmp",
        "image/svg+xml" => "svg",
        _ => "png",
    };
    let dest = format!("/tmp/sbxw-pastes/paste-{}.{ext}", crate::now_ms());
    app.sbx.write_file(&sb, &dest, &body, false).await?;
    Ok(ok_with(json!({ "path": dest })))
}

/// Largest file the sidebar's upload box takes. It is held in memory on both
/// sides of `sbx exec`, like a file the explorer opens.
const UPLOAD_MAX: usize = 128 * 1024 * 1024;

#[derive(Deserialize)]
struct UploadQuery {
    #[serde(default)]
    name: String,
}

/// A file from the sidebar's upload box, kept under its own name so the path
/// typed into the prompt after it still says what it is.
async fn upload(State(app): S, Path(name): Path<String>, Query(q): Query<UploadQuery>, body: Bytes) -> ApiResult {
    let sb = sandbox(&name)?;
    if body.is_empty() {
        return Err(ApiError::bad("empty file"));
    }
    let dest = format!("/tmp/sbxw-uploads/{}/{}", crate::now_ms(), upload_name(&q.name));
    app.sbx.write_file(&sb, &dest, &body, false).await?;
    tracing::info!("web UI: uploaded {} bytes to '{sb}' at {dest}", body.len());
    Ok(ok_with(json!({ "path": dest })))
}

/// A browser's file name as one safe path component. The path is typed into a
/// prompt, where a space would split it in two, so anything but `[A-Za-z0-9._-]`
/// becomes `_`; leading dots go, so no `..` and no hidden file.
pub(super) fn upload_name(raw: &str) -> String {
    let base = raw.rsplit(['/', '\\']).next().unwrap_or("");
    let safe: String = base
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') { c } else { '_' })
        .collect();
    let safe: String = safe.trim_start_matches('.').chars().take(120).collect();
    if safe.is_empty() {
        "upload".into()
    } else {
        safe
    }
}

async fn hosts(State(app): S) -> Json<Value> {
    let app2 = app.clone();
    let list = tokio::task::spawn_blocking(move || app2.hosts.read_block()).await.unwrap_or_default();
    Json(json!(list))
}

// ── Threads, lead ────────────────────────────────────────────────────────

async fn threads(State(app): S, Path(name): Path<String>) -> ApiResult {
    let v = app.threads(&sandbox(&name)?).await;
    Ok(ok_with(json!(v)))
}

async fn repos(State(app): S, Path(name): Path<String>) -> ApiResult {
    let sb = sandbox(&name)?;
    let ws = app
        .store
        .workspace(&sb)
        .ok_or_else(|| ApiError::not_found(format!("sbxw has no workspace recorded for '{sb}'")))?;
    let repos = crate::git::list(&ws).await;
    Ok(ok_with(json!({ "workspace": ws, "repos": repos })))
}

async fn worktrees(State(app): S, Path(name): Path<String>) -> ApiResult {
    let (ws, repos) = app.worktree_graph(&sandbox(&name)?).await.map_err(ApiError::not_found)?;
    Ok(ok_with(json!({ "workspace": ws, "repos": repos })))
}

async fn thread_status(State(app): S, Path((name, thread)): Path<(String, String)>) -> ApiResult {
    let (sb, t) = (sandbox(&name)?, ThreadName::parse(&thread)?);
    let dir = app.settle_thread_repo(&sb, &t, None)?;
    let st = app.sbx.thread_status(&sb, &dir, &t).await?;
    Ok(ok_with(json!({ "status": st })))
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct CloseBody {
    discard: bool,
    delete_branch: bool,
}

async fn thread_close(State(app): S, Path((name, thread)): Path<(String, String)>, Json(b): Json<CloseBody>) -> ApiResult {
    let (sb, t) = (sandbox(&name)?, ThreadName::parse(&thread)?);
    app.close_thread(&sb, &t, b.discard, b.delete_branch).await?;
    Ok(ok())
}

async fn lead(State(app): S, Path(name): Path<String>) -> ApiResult {
    app.lead(&sandbox(&name)?).await?;
    Ok(ok())
}

async fn lead_end(State(app): S, Path(name): Path<String>) -> ApiResult {
    app.lead_end(&sandbox(&name)?).await;
    Ok(ok())
}

/// The session to start Remote Control in: a thread's, else the main one.
#[derive(Deserialize, Default)]
struct RemoteControlBody {
    #[serde(default)]
    thread: Option<String>,
}

impl RemoteControlBody {
    fn kind(b: Option<Json<Self>>) -> Result<SessionKind, ApiError> {
        Ok(match non_blank(b.map(|Json(b)| b).unwrap_or_default().thread) {
            Some(t) => SessionKind::Thread(ThreadName::parse(&t)?),
            None => SessionKind::Agent,
        })
    }
}

/// What the button shows and decides from: how Claude is signed in, and
/// whether Remote Control is on in this pane's session.
#[derive(Deserialize)]
struct RemoteStateQuery {
    #[serde(default)]
    thread: Option<String>,
}

async fn remote_state(State(app): S, Path(name): Path<String>, Query(q): Query<RemoteStateQuery>) -> ApiResult {
    let kind = RemoteControlBody::kind(Some(Json(RemoteControlBody { thread: q.thread })))?;
    let state = app.remote_state(&sandbox(&name)?, &kind).await?;
    let login = match state.login {
        sbxw_core::remote::Login::Full => "full",
        sbxw_core::remote::Login::TokenOnly => "token",
        sbxw_core::remote::Login::Missing => "missing",
        sbxw_core::remote::Login::ApiKeyHelper => "helper",
    };
    let advice = (state.login == sbxw_core::remote::Login::ApiKeyHelper).then(|| sbxw_core::remote::helper_advice(&name));
    Ok(ok_with(json!({ "login": login, "active": state.active, "advice": advice })))
}

async fn remote_control(State(app): S, Path(name): Path<String>, b: Option<Json<RemoteControlBody>>) -> ApiResult {
    app.remote_control(&sandbox(&name)?, RemoteControlBody::kind(b)?).await?;
    Ok(ok())
}

async fn remote_login(State(app): S, Path(name): Path<String>, b: Option<Json<RemoteControlBody>>) -> ApiResult {
    app.switch_to_login(&sandbox(&name)?, RemoteControlBody::kind(b)?).await?;
    Ok(ok())
}

// ── Plans ────────────────────────────────────────────────────────────────

async fn plans(State(app): S) -> Json<Value> {
    Json(json!(app.plans_list()))
}

#[derive(Deserialize)]
struct ApproveBody {
    threads: Vec<PlanThread>,
    /// The dialog's choice; the proposal's when absent.
    #[serde(default)]
    variant: Option<Variant>,
}

async fn plan_approve(State(app): S, Path(id): Path<String>, Json(b): Json<ApproveBody>) -> ApiResult {
    let plan = app.plan_approve(&id, b.threads, b.variant).await?;
    Ok(ok_with(json!({ "plan": plan })))
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RefuseBody {
    note: Option<String>,
}

async fn plan_refuse(State(app): S, Path(id): Path<String>, Json(b): Json<RefuseBody>) -> ApiResult {
    let plan = app.plan_refuse(&id, b.note)?;
    Ok(ok_with(json!({ "plan": plan })))
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct MergeBody {
    close_threads: bool,
    /// The distillate items the person approved; as proposed when absent.
    distillate: Option<Vec<DistillItem>>,
}

async fn plan_merge(State(app): S, Path(id): Path<String>, Json(b): Json<MergeBody>) -> ApiResult {
    let merge = app.plan_merge(&id, b.close_threads, b.distillate).await?;
    Ok(ok_with(json!({ "merge": merge })))
}

// ── Relay ────────────────────────────────────────────────────────────────

async fn relay_list(State(app): S) -> Json<Value> {
    Json(json!({ "requests": app.relay_list() }))
}

#[derive(Deserialize)]
struct RouteBody {
    to: String,
}

async fn relay_route(State(app): S, Path(id): Path<String>, Json(b): Json<RouteBody>) -> ApiResult {
    app.relay_route(&id, &sandbox(&b.to)?)?;
    Ok(ok())
}

#[derive(Deserialize)]
struct RelayApproveBody {
    #[serde(default)]
    answer: Option<String>,
    #[serde(default)]
    images: Vec<String>,
}

async fn relay_approve(State(app): S, Path(id): Path<String>, Json(b): Json<RelayApproveBody>) -> ApiResult {
    app.relay_approve(&id, b.answer.as_deref(), &b.images)?;
    Ok(ok())
}

#[derive(Deserialize, Default)]
struct DenyBody {
    #[serde(default)]
    note: Option<String>,
}

async fn relay_deny(State(app): S, Path(id): Path<String>, Json(b): Json<DenyBody>) -> ApiResult {
    app.relay_deny(&id, b.note.as_deref())?;
    Ok(ok())
}

// ── Files ────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct PathQuery {
    path: Option<String>,
}

/// Host folders for the create dialog's picker.
async fn fs_browse(Query(q): Query<PathQuery>) -> Json<Value> {
    let out = tokio::task::spawn_blocking(move || {
        let base = q
            .path
            .map(PathBuf::from)
            .unwrap_or_else(|| std::env::var("HOME").map(PathBuf::from).unwrap_or_else(|_| "/".into()));
        let dir = base.canonicalize().unwrap_or(base);
        let mut entries: Vec<Value> = std::fs::read_dir(&dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
            .filter(|e| !e.file_name().to_string_lossy().starts_with('.'))
            .map(|e| json!({ "name": e.file_name().to_string_lossy(), "path": e.path() }))
            .collect();
        entries.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
        json!({ "path": dir, "parent": dir.parent(), "entries": entries })
    })
    .await
    .unwrap_or_else(|_| json!({ "path": "", "parent": null, "entries": [] }));
    Json(out)
}

/// The OS-native folder picker. Dismissing it is not an error, nor a path.
async fn fs_pick() -> ApiResult {
    let picked = tokio::task::spawn_blocking(pick_folder_native)
        .await
        .map_err(|e| ApiError::bad(e.to_string()))??;
    Ok(match picked {
        Some(path) => ok_with(json!({ "path": path })),
        None => Json(json!({ "ok": false, "cancelled": true })),
    })
}

fn pick_folder_native() -> anyhow::Result<Option<String>> {
    use std::process::Command;
    let read = |out: std::process::Output| {
        let p = String::from_utf8_lossy(&out.stdout).trim().to_string();
        (out.status.success() && !p.is_empty()).then_some(p)
    };
    if cfg!(target_os = "macos") {
        let out = Command::new(sbxw_core::sysbin::sys_bin("osascript"))
            .args(["-e", r#"POSIX path of (choose folder with prompt "Select workspace folder")"#])
            .output()?;
        return Ok(read(out));
    }
    if let Ok(out) = Command::new(sbxw_core::sysbin::sys_bin("zenity")).args(["--file-selection", "--directory", "--title=Select workspace folder"]).output() {
        return Ok(read(out));
    }
    if let Ok(out) = Command::new(sbxw_core::sysbin::sys_bin("kdialog"))
        .arg("--getexistingdirectory")
        .arg(std::env::var("HOME").unwrap_or_else(|_| "/".into()))
        .output()
    {
        return Ok(read(out));
    }
    anyhow::bail!("no native folder picker found — install zenity or kdialog")
}

fn favourites_payload(list: Vec<String>) -> Value {
    json!({ "favourites": favourites::payload(list, |p| std::path::Path::new(p).is_dir()) })
}

async fn favourites_get(State(app): S) -> Json<Value> {
    Json(favourites_payload(app.store.favourites()))
}

#[derive(Deserialize)]
struct FavouriteBody {
    path: String,
    favourite: bool,
}

async fn favourites_set(State(app): S, Json(b): Json<FavouriteBody>) -> ApiResult {
    let raw = b.path.trim();
    if raw.is_empty() {
        return Err(ApiError::bad("no folder given"));
    }
    let mut list = app.store.favourites();
    let canonical = std::fs::canonicalize(raw).ok();
    if b.favourite {
        let c = canonical.ok_or_else(|| ApiError::bad(format!("cannot star '{raw}'")))?;
        if !c.is_dir() {
            return Err(ApiError::bad(format!("'{raw}' is not a folder")));
        }
        favourites::star(&mut list, c.to_string_lossy().into_owned())?;
    } else {
        let c = canonical.map(|c| c.to_string_lossy().into_owned());
        favourites::unstar(&mut list, raw, c.as_deref());
    }
    app.store.set_favourites(&list)?;
    let mut payload = favourites_payload(list);
    payload["ok"] = true.into();
    Ok(Json(payload))
}

#[derive(Deserialize)]
struct UrlQuery {
    url: String,
}

/// Which sandbox serves the page in a browser tab (the side panel's question).
async fn embed_resolve(State(app): S, Query(q): Query<UrlQuery>) -> ApiResult {
    let origin = sbxw_core::embed::parse_origin(&q.url)
        .ok_or_else(|| ApiError::bad("not a web page: the side panel works on http:// and https:// tabs"))?;
    let all = app.sbx.ls().await;
    let names: Vec<String> = all.iter().map(|s| s.name.clone()).collect();
    // The panel offers only these: a stopped sandbox serves no page to test.
    let running: Vec<&str> = all.iter().filter(|s| s.is_running()).map(|s| s.name.as_str()).collect();
    let reads = names.iter().filter_map(|n| SandboxName::parse(n).ok()).map(|n| {
        let app = app.clone();
        async move { app.sbx.ports(&n).await.into_iter().map(move |m| (n.to_string(), m)).collect::<Vec<_>>() }
    });
    let ports: Vec<_> = futures_util::future::join_all(reads).await.into_iter().flatten().collect();
    let aliases = app.hosts.read_block();
    let r = sbxw_core::embed::resolve(&origin, &app.store.pairings(), &aliases, &ports, &names);
    Ok(ok_with(json!({
        "host": origin.host,
        "sandbox": r.sandbox,
        "via": r.via.map(|v| v.as_str()),
        "candidates": r.candidates,
        "stale_pairing": r.stale_pairing,
        "sandboxes": names,
        "running": running,
    })))
}

fn pairings_payload(p: std::collections::BTreeMap<String, String>) -> Json<Value> {
    let rows: Vec<Value> = p.into_iter().map(|(host, sandbox)| json!({ "host": host, "sandbox": sandbox })).collect();
    ok_with(json!({ "pairings": rows }))
}

async fn pairings(State(app): S) -> Json<Value> {
    pairings_payload(app.store.pairings())
}

#[derive(Deserialize)]
struct PairBody {
    host: String,
    #[serde(default)]
    sandbox: Option<String>,
}

async fn pair(State(app): S, Json(b): Json<PairBody>) -> ApiResult {
    let sb = non_blank(b.sandbox).ok_or_else(|| ApiError::bad("no sandbox given"))?;
    let mut p = app.store.pairings();
    sbxw_core::embed::set_pairing(&mut p, &b.host, Some(&sb))?;
    app.store.set_pairings(&p)?;
    Ok(pairings_payload(p))
}

async fn unpair(State(app): S, Json(b): Json<PairBody>) -> ApiResult {
    let mut p = app.store.pairings();
    sbxw_core::embed::set_pairing(&mut p, &b.host, None)?;
    app.store.set_pairings(&p)?;
    Ok(pairings_payload(p))
}

/// `.sbxw-artifacts`, read on the host side of the mount.
async fn artifacts_list(State(app): S, Path(name): Path<String>) -> ApiResult {
    let sb = sandbox(&name)?;
    let run = app.codemap_run(&sb);
    let Some(ws) = app.store.workspace(&sb) else {
        return Ok(Json(json!({ "dir": "", "entries": [], "codemap": null, "lenses": [], "run": run })));
    };
    let dir = ws.join(artifacts::ARTIFACTS_DIR);
    let d = dir.clone();
    let entries = tokio::task::spawn_blocking(move || walk_artifacts(&d)).await.unwrap_or_default();
    let (entries, codemap, lenses) = artifacts::fold(entries);
    Ok(Json(json!({ "dir": dir, "entries": entries, "codemap": codemap, "lenses": lenses, "run": run })))
}

fn walk_artifacts(root: &std::path::Path) -> Vec<artifacts::Entry> {
    fn walk(root: &std::path::Path, dir: &std::path::Path, out: &mut Vec<artifacts::Entry>, depth: u32) {
        if depth > artifacts::MAX_DEPTH {
            return;
        }
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            let Ok(ft) = e.file_type() else { continue };
            if name.starts_with('.') {
                continue;
            }
            if ft.is_dir() {
                walk(root, &e.path(), out, depth + 1);
            } else if ft.is_file() && artifacts::has_allowed_extension(&name) {
                let Ok(meta) = e.metadata() else { continue };
                out.push(artifacts::Entry {
                    path: e.path().strip_prefix(root).unwrap_or(&e.path()).to_string_lossy().replace('\\', "/"),
                    name,
                    size: meta.len(),
                    modified: sandbox_fs::mtime_secs(&meta),
                });
            }
        }
    }
    let mut out = Vec::new();
    if root.is_dir() {
        walk(root, root, &mut out, 0);
    }
    out
}

/// One file of `.sbxw-artifacts`, canonicalised and checked to stay inside.
async fn artifact_download(State(app): S, Path(name): Path<String>, Query(q): Query<PathQuery>) -> Result<Response, ApiError> {
    let sb = sandbox(&name)?;
    let ws = app.store.workspace(&sb).ok_or_else(|| ApiError::not_found("unknown sandbox"))?;
    let root = ws.join(artifacts::ARTIFACTS_DIR).canonicalize().map_err(|_| ApiError::not_found("no artifacts directory"))?;
    let file = root
        .join(q.path.unwrap_or_default())
        .canonicalize()
        .map_err(|_| ApiError::not_found("file not found"))?;
    if !file.starts_with(&root) || !file.is_file() {
        return Err(ApiError::forbidden("invalid path"));
    }
    let data = tokio::fs::read(&file).await.map_err(|_| ApiError::not_found("file not found"))?;
    let filename = file.file_name().map(|n| n.to_string_lossy().replace('"', "")).unwrap_or_else(|| "download".into());
    Ok((
        [
            (header::CONTENT_TYPE, artifacts::guess_mime(&filename).to_string()),
            (header::CONTENT_DISPOSITION, format!("attachment; filename=\"{filename}\"")),
            (header::CONTENT_SECURITY_POLICY, "sandbox".to_string()),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff".to_string()),
        ],
        data,
    )
        .into_response())
}

#[derive(Deserialize)]
struct FsQuery {
    #[serde(default)]
    path: String,
    #[serde(default)]
    download: Option<String>,
    #[serde(default)]
    max: Option<u64>,
    #[serde(default)]
    head: Option<String>,
}

async fn sandbox_fs_list(State(app): S, Path(name): Path<String>, Query(q): Query<FsQuery>) -> ApiResult {
    let sb = sandbox(&name)?;
    let ws = app.store.workspace(&sb);
    let l = sandbox_fs::list(&app.sbx, &sb, ws.as_deref(), &q.path).await.map_err(|e| ApiError::not_found(format!("{e:#}")))?;
    let ws_s = ws.map(|w| w.to_string_lossy().into_owned());
    let artifacts = ws_s.as_ref().map(|w| format!("{}/{}", w.trim_end_matches('/'), artifacts::ARTIFACTS_DIR));
    let mounts = app.store.get(&sb).mounts;
    let worktrees = thread_worktrees(&app, &sb).await;
    Ok(ok_with(json!({
        "path": l.path, "parent": l.parent, "entries": l.entries, "offline": l.offline,
        "workspace": ws_s, "artifacts": artifacts, "home": sandbox_fs::SANDBOX_HOME, "mounts": mounts,
        "worktrees": worktrees,
    })))
}

/// Each thread's worktree, and its `.sbxw-artifacts` when it has one: what a
/// thread delivers sits on its branch, under `.sbxw-worktrees/<thread>`, until
/// it is merged — a dot-folder nobody finds by browsing. Paths are the same in
/// the sandbox and on the host (the workspace is mounted at its own path).
async fn thread_worktrees(app: &App, sb: &SandboxName) -> Vec<Value> {
    let Some(ws) = app.store.workspace(sb) else { return vec![] };
    let recorded: std::collections::BTreeMap<String, String> =
        app.store.get(sb).threads.iter().map(|(k, v)| (k.clone(), v.repo.clone())).collect();
    tokio::task::spawn_blocking(move || {
        crate::git::thread_worktrees(&ws, &recorded)
            .into_iter()
            .filter_map(|(thread, repo)| {
                let dir = crate::git::repo_root(&ws, &repo)?.join(".sbxw-worktrees").join(&thread);
                let arts = dir.join(artifacts::ARTIFACTS_DIR);
                Some(json!({
                    "thread": thread, "repo": repo, "path": dir,
                    "artifacts": arts.is_dir().then_some(arts),
                }))
            })
            .collect()
    })
    .await
    .unwrap_or_default()
}

/// What was written lately, wherever: the workspace, the read-write mounts,
/// home and `/tmp` — an agent's file is not always where it was asked to go.
async fn sandbox_fs_recent(State(app): S, Path(name): Path<String>) -> ApiResult {
    let sb = sandbox(&name)?;
    let ws = app.store.workspace(&sb);
    let ws_s = ws.as_ref().map(|w| w.to_string_lossy().into_owned());
    let roots = writable_roots(&app, &sb, ws_s.as_deref());
    let (files, offline) = sandbox_fs::recent(&app.sbx, &sb, ws.as_deref(), &roots)
        .await
        .map_err(|e| ApiError::not_found(format!("{e:#}")))?;
    Ok(ok_with(json!({
        "files": files, "offline": offline, "roots": roots, "days": sandbox_fs::RECENT_DAYS,
        "workspace": ws_s, "home": sandbox_fs::SANDBOX_HOME,
    })))
}

/// Where an agent writes: the workspace, the read-write mounts, home and
/// `/tmp`. What Recent scans, and the only places a file may be deleted from.
fn writable_roots(app: &App, sb: &SandboxName, ws: Option<&str>) -> Vec<String> {
    let mut roots: Vec<String> = ws.map(str::to_string).into_iter().collect();
    roots.extend(app.store.get(sb).mounts.iter().filter(|m| !m.readonly).map(|m| m.path.to_string_lossy().into_owned()));
    roots.extend([sandbox_fs::SANDBOX_HOME.to_string(), "/tmp".to_string()]);
    roots
}

#[derive(Deserialize)]
struct DeleteBody {
    path: String,
}

/// Delete one file or folder — the stray one an agent left somewhere. Not the
/// roots themselves, nor the deliverables folder or the repository's `.git`
/// (see `sandbox_fs::refuse_delete`).
async fn sandbox_fs_delete(State(app): S, Path(name): Path<String>, Json(b): Json<DeleteBody>) -> ApiResult {
    let sb = sandbox(&name)?;
    let ws = app.store.workspace(&sb);
    let ws_s = ws.as_ref().map(|w| w.to_string_lossy().into_owned());
    let roots = writable_roots(&app, &sb, ws_s.as_deref());
    let protected: Vec<String> = ws_s
        .iter()
        .flat_map(|w| {
            let w = w.trim_end_matches('/');
            [format!("{w}/{}", artifacts::ARTIFACTS_DIR), format!("{w}/.git"), format!("{w}/.sbxw-worktrees")]
        })
        // A thread's worktree goes with the thread (Close thread), not from here.
        .chain(thread_worktrees(&app, &sb).await.iter().filter_map(|w| w["path"].as_str().map(str::to_string)))
        .collect();
    if let Some(why) = sandbox_fs::refuse_delete(&b.path, &roots, &protected) {
        return Err(ApiError::forbidden(why));
    }
    sandbox_fs::delete(&app.sbx, &sb, ws.as_deref(), &b.path, &roots, &protected)
        .await
        .map_err(|e| ApiError::bad(format!("{e:#}")))?;
    Ok(ok_with(json!({ "path": b.path })))
}

/// A file from inside the sandbox, always under `CSP: sandbox` + `nosniff`:
/// an HTML file an agent wrote must never run with the daemon's origin.
async fn sandbox_fs_file(State(app): S, Path(name): Path<String>, Query(q): Query<FsQuery>) -> Result<Response, ApiError> {
    let sb = sandbox(&name)?;
    let ws = app.store.workspace(&sb);
    let max = q.max.unwrap_or(sandbox_fs::MAX_FILE);
    let data = sandbox_fs::read(&app.sbx, &sb, ws.as_deref(), &q.path, max, q.head.is_some())
        .await
        .map_err(|e| ApiError::not_found(format!("{e:#}")))?;
    let filename = q.path.rsplit('/').next().unwrap_or("download").replace(['"', '\\', '\r', '\n'], "");
    let disposition = if q.download.is_some() { format!("attachment; filename=\"{filename}\"") } else { "inline".into() };
    Ok((
        [
            (header::CONTENT_TYPE, artifacts::guess_mime(&filename).to_string()),
            (header::CONTENT_DISPOSITION, disposition),
            (header::CONTENT_SECURITY_POLICY, "sandbox".to_string()),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff".to_string()),
        ],
        data,
    )
        .into_response())
}

// ── Code map ─────────────────────────────────────────────────────────────

async fn codemap_start(State(app): S, Path(name): Path<String>) -> ApiResult {
    let (run, dir) = app.codemap_start(&sandbox(&name)?, None).await?;
    Ok(ok_with(json!({ "run": run, "dir": dir })))
}

#[derive(Deserialize)]
struct LensBody {
    objective: String,
}

async fn codemap_lens(State(app): S, Path(name): Path<String>, Json(b): Json<LensBody>) -> ApiResult {
    let (run, dir) = app.codemap_start(&sandbox(&name)?, Some(&b.objective)).await?;
    Ok(ok_with(json!({ "run": run, "dir": dir })))
}

async fn codemap_runs(State(app): S) -> Json<Value> {
    Json(json!(app.codemap_runs()))
}

// ── The daemon ───────────────────────────────────────────────────────────

async fn health(State(app): S) -> Json<Value> {
    ok_with(json!({
        "version": app.versions.sbxw,
        "sbx": app.versions.sbx,
        "pid": std::process::id(),
        "web_addr": app.cfg.web_addr,
        "agent_addr": app.cfg.agent_addr,
    }))
}

/// Everything a tab needs to draw itself, in one call. `/api/stream` carries every change from then on.
async fn state(State(app): S) -> Json<Value> {
    Json(json!({
        "sessions": app.session_infos(),
        "relay": app.relay_list(),
        "plans": app.plans_list(),
        "codemap": app.codemap_runs(),
        "leads": app.leading(),
        "usage": app.usage(),
        "models": { "default": app.cfg.claude_model, "models": app.cfg.models },
        "versions": { "sbxw": app.versions.sbxw, "sbx": app.versions.sbx },
    }))
}

#[derive(Deserialize)]
struct InitialBody {
    sandbox: String,
}

/// The sandbox a freshly opened page shows (`sbxw web <name>`).
async fn set_initial(State(app): S, Json(b): Json<InitialBody>) -> ApiResult {
    let sb = sandbox(&b.sandbox)?;
    *app.initial_sandbox.lock().unwrap() = sb.to_string();
    Ok(ok())
}

/// `sbxw down`: stop every session, then exit.
async fn shutdown(State(app): S) -> Json<Value> {
    for (key, s) in app.sessions.all() {
        tracing::info!("shutting down: closing {key}");
        s.kill();
    }
    tokio::spawn(async {
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        std::process::exit(0);
    });
    ok()
}

/// The thread a WebSocket asks for may choose its repo and model on first open.
pub fn settle_ws_thread(app: &App, sb: &SandboxName, kind: &SessionKind, repo: Option<&str>, model: Option<&str>) -> Result<(), String> {
    if let SessionKind::Thread(t) = kind {
        if repo.is_some() || model.is_some() {
            app.settle_thread_repo(sb, t, repo)?;
        }
        if let Some(m) = model.filter(|m| !m.is_empty()) {
            let m = ModelId::parse(m).map_err(|e| e.to_string())?;
            app.settle_thread_model(sb, t, &m, false)?;
        }
    }
    let _ = SessionKey::new(sb.clone(), kind.clone());
    Ok(())
}
