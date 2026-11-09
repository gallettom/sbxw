//! The UI listener's handlers. Thin on purpose: parse, call a service, shape
//! the answer the page already reads.

use super::error::{ok, ok_with, ApiError, ApiResult};
use crate::app::App;
use crate::host::{Host, Kind};
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
use sbxw_core::mcp;
use sbxw_core::plans::{DistillItem, PlanThread, Variant};
use sbxw_core::policy;
use sbxw_core::ports::WantedPort;
use sbxw_core::progress::Progress;
use sbxw_core::session::{SessionKey, SessionKind};
use sbxw_core::{names, ModelId, SandboxName, SandboxRef, ThreadName};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

type S = State<Arc<App>>;

/// A sandbox named on the wire, as a ref (`neos`, `buildbox~neos`). Each
/// service routes it to its host: `App::sbx_for`, `App::host_for`.
fn sandbox(raw: &str) -> Result<SandboxRef, ApiError> {
    Ok(SandboxRef::parse(raw)?)
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
        .route("/api/sandboxes/:name/recreate", post(recreate))
        .route("/api/sandboxes/:name/stop", post(stop))
        .route("/api/sandboxes/:name/rm", post(rm))
        .route("/api/sandboxes/:name/ports", get(ports))
        .route("/api/sandboxes/:name/ports/publish", post(publish))
        .route("/api/sandboxes/:name/ports/unpublish", post(unpublish))
        .route("/api/sandboxes/:name/policy", get(policy_panel))
        .route("/api/sandboxes/:name/policy/rules", post(policy_add))
        .route("/api/sandboxes/:name/policy/rules/rm", post(policy_rm))
        .route("/api/policy/refresh", post(policy_refresh))
        .route("/api/github-token", get(github_token).post(github_token_set))
        .route("/api/github-token/rm", post(github_token_rm))
        .route("/api/github-token/gh", post(github_token_gh))
        .route("/api/approvals", get(approvals_list))
        .route("/api/approvals/respond", post(approval_respond))
        .route("/api/secrets", get(secrets_list))
        .route("/api/secrets/custom", post(custom_secret_set))
        .route("/api/secrets/rm", post(secret_rm))
        .route("/api/template/status", get(template_status))
        .route("/api/kit-mounts", get(kit_mounts))
        .route("/api/templates", get(templates_list))
        .route("/api/templates/save", post(template_save))
        .route("/api/templates/rm", post(template_rm))
        .route("/api/kits", get(kits_panel).post(kits_set))
        .route("/api/my-skills", get(my_skills_list))
        .route("/api/my-skills/rm", post(my_skills_rm))
        .route("/api/sandboxes/:name/skills", get(sandbox_skills))
        .route("/api/sandboxes/:name/skills/keep", post(keep_skill))
        .route("/api/mcp", get(mcp_list).post(mcp_save))
        .route("/api/mcp/:server", get(mcp_inspect))
        .route("/api/mcp/:server/rm", post(mcp_rm))
        .route("/api/mcp/:server/auth", post(mcp_auth))
        .route("/api/sandboxes/:name/mcp/load", post(mcp_load))
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
        // Remote hosts (EXPERIMENTAL): `~/.sbxw/hosts.toml`. Not `/api/hosts`,
        // which is the `/etc/hosts` block.
        .route("/api/remote-hosts", get(remote_hosts))
        .route("/api/remote-hosts/key", post(remote_host_key))
        .route("/api/remote-hosts/add", post(remote_host_add))
        .route("/api/remote-hosts/reload", post(remote_hosts_reload))
        .route("/api/remote-hosts/:host/edit", post(remote_host_edit))
        .route("/api/remote-hosts/:host/rm", post(remote_host_rm))
        .route("/api/remote-hosts/:host/test", post(remote_host_test))
        .route("/api/remote-hosts/:host/clone", post(remote_host_clone))
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
        // The voice concierge.
        .route("/api/voice", get(voice_status))
        .route("/api/voice/install", post(voice_install))
        .route("/api/voice/cancel", post(voice_cancel))
        .route("/api/voice/rm", post(voice_rm))
        .route("/api/voice/chat", post(voice_chat))
        .route("/api/voice/speak", post(voice_speak))
        .route(
            "/api/voice/transcribe",
            post(voice_transcribe).layer(DefaultBodyLimit::max(crate::svc::voice::MAX_SAMPLES * 4 + 1024)),
        )
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
    fn key(&self) -> Result<(SandboxRef, SessionKind), ApiError> {
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

/// The sbxw.toml files the Kits dialog may edit, with their `kits` lines.
async fn kits_panel(State(app): S) -> ApiResult {
    Ok(ok_with(app.kits_panel()))
}

#[derive(Deserialize)]
struct KitsBody {
    path: std::path::PathBuf,
    /// `null` removes the line: the defaults apply.
    kits: Option<Vec<String>>,
}

async fn kits_set(State(app): S, Json(b): Json<KitsBody>) -> ApiResult {
    Ok(warnings(app.set_kits(&b.path, b.kits)?))
}

/// The skills kept from sandboxes, which every new sandbox gets.
async fn my_skills_list(State(app): S) -> ApiResult {
    Ok(ok_with(app.my_skills_list()))
}

#[derive(Deserialize)]
struct SkillNameBody {
    name: String,
}

async fn my_skills_rm(State(app): S, Json(b): Json<SkillNameBody>) -> ApiResult {
    app.forget_skill(&b.name)?;
    Ok(ok())
}

/// The skills a sandbox has that could be kept.
async fn sandbox_skills(State(app): S, Path(name): Path<String>) -> ApiResult {
    Ok(ok_with(app.sandbox_skills(&sandbox(&name)?).await?))
}

#[derive(Deserialize)]
struct KeepSkillBody {
    origin: sbxw_core::my_skills::Origin,
    name: String,
    #[serde(default)]
    replace: bool,
}

/// Keep one: on the UI listener only, since it reaches every later sandbox.
async fn keep_skill(State(app): S, Path(name): Path<String>, Json(b): Json<KeepSkillBody>) -> ApiResult {
    Ok(warnings(app.keep_skill(&sandbox(&name)?, b.origin, &b.name, b.replace).await?))
}

#[derive(Deserialize)]
struct ChatBody {
    #[serde(default)]
    name: Option<String>,
}

async fn chat(State(app): S, Json(b): Json<ChatBody>) -> ApiResult {
    let name = match non_blank(b.name) {
        Some(n) => sandbox(&n)?,
        None => names::mint_chat_name(u32::from_str_radix(&sbxw_core::ids::random_hex(4), 16).unwrap_or(0)).into(),
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
    extra_ports: Vec<WantedPort>,
    #[serde(default)]
    template: sbxw_core::templates::Choice,
    #[serde(default)]
    remote_control: bool,
    /// Clone this into `path` first when it does not exist yet.
    #[serde(default)]
    git: Option<String>,
}

async fn up(State(app): S, Path(name): Path<String>, Json(b): Json<UpBody>) -> Result<Response, ApiError> {
    let name = sandbox(&name)?;
    let config = match &b.config {
        Some(p) => Config::load(p, std::path::Path::new("/"))?,
        None => app.daemon_config(),
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
        template: b.template,
        remote_control: b.remote_control,
        git: b.git,
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
    app.sessions.drop_sandbox(&sandbox(&name)?.to_string());
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

async fn recreate(State(app): S, Path(name): Path<String>) -> ApiResult {
    Ok(warnings(app.recreate(&sandbox(&name)?).await?))
}

async fn stop(State(app): S, Path(name): Path<String>) -> ApiResult {
    let sb = sandbox(&name)?;
    app.sbx_for(&sb).await?.stop(&sb.name).await?;
    Ok(ok())
}

async fn rm(State(app): S, Path(name): Path<String>) -> ApiResult {
    app.remove(&[sandbox(&name)?]).await?;
    Ok(ok())
}

async fn ports(State(app): S, Path(name): Path<String>) -> ApiResult {
    let sb = sandbox(&name)?;
    let list: Vec<Value> = app
        .sbx_for(&sb)
        .await?
        .ports(&sb.name)
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
    let sb = sandbox(&name)?;
    app.sbx_for(&sb).await?.unpublish(&sb.name, &spec).await?;
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
    app.sbx_for(&sb).await?.policy_http(decision, (!global).then_some(&sb.name), &resources, &http).await?;
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
    app.sbx_for(&sb).await?.policy_rm(&rule, Some(&sb.name)).await?;
    Ok(ok())
}

/// The GitHub token the sandbox proxy injects into git/gh traffic: sbx's
/// global `github` secret. Only whether it is set is ever read back.
async fn github_token(State(app): S) -> ApiResult {
    let gh = sbxw_core::sysbin::gh().map(|p| p.to_string_lossy().into_owned());
    Ok(ok_with(json!({ "set": app.sbx.secret_has("github").await?, "gh": gh })))
}

/// The global `github` secret as `gh auth token` on this machine, which sbx
/// runs and refreshes itself: nothing is pasted, nothing goes stale.
async fn github_token_gh(State(app): S) -> ApiResult {
    let gh = sbxw_core::sysbin::gh().ok_or_else(|| {
        ApiError::bad(format!("no GitHub CLI found in {} — install it, run `gh auth login`, then try again", sbxw_core::sysbin::GH_DIRS.join(", ")))
    })?;
    tracing::info!("web UI: storing the global `github` secret as `{} auth token`", gh.display());
    app.sbx.secret_set_command("github", &sbxw_core::sysbin::gh_token_command(&gh)).await?;
    Ok(ok())
}

#[derive(Deserialize)]
struct TokenBody {
    token: String,
}

async fn github_token_set(State(app): S, Json(b): Json<TokenBody>) -> ApiResult {
    let token = b.token.trim();
    if token.is_empty() || token.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(ApiError::bad("a GitHub token is one word with no spaces — paste it again"));
    }
    tracing::info!("web UI: storing the global `github` secret");
    app.sbx.secret_set("github", token, None).await?;
    Ok(ok())
}

async fn github_token_rm(State(app): S) -> ApiResult {
    tracing::info!("web UI: removing the global `github` secret");
    app.sbx.secret_rm("github").await?;
    Ok(ok())
}

/// What sbx waits for a person to decide, on this machine.
async fn approvals_list(State(app): S) -> ApiResult {
    Ok(ok_with(json!({ "approvals": app.sbx.approvals().await? })))
}

#[derive(Deserialize)]
struct ApprovalBody {
    id: String,
    option: String,
}

/// Only an approval sbx still lists, with an option it offers: a stale page
/// (a port request lapses after 45 s) gets told so rather than sbx's error.
async fn approval_respond(State(app): S, Json(b): Json<ApprovalBody>) -> ApiResult {
    let pending = app.sbx.approvals().await?;
    let Some(a) = pending.iter().find(|a| a.id == b.id) else {
        return Err(ApiError::bad("this request is no longer pending — it was answered, or it lapsed"));
    };
    if !a.offers(&b.option) {
        return Err(ApiError::bad(format!("“{}” is not one of this request's options", b.option)));
    }
    tracing::info!("web UI: approval {} for '{}' ({}) → {}", a.id, a.sandbox, a.title, b.option);
    app.sbx.approval_respond(&a.id, &b.option).await?;
    Ok(ok())
}

/// sbx's stored secrets, names and scopes only.
async fn secrets_list(State(app): S) -> ApiResult {
    Ok(ok_with(json!({ "secrets": app.sbx.secrets().await? })))
}

#[derive(Deserialize)]
struct CustomSecretBody {
    hosts: Vec<String>,
    env: String,
    value: String,
}

async fn custom_secret_set(State(app): S, Json(b): Json<CustomSecretBody>) -> ApiResult {
    let hosts: Vec<String> = b.hosts.iter().map(|h| h.trim().to_string()).filter(|h| !h.is_empty()).collect();
    if hosts.is_empty() {
        return Err(ApiError::bad("name at least one host the secret is for (api.example.com, *.example.com)"));
    }
    if let Some(h) = hosts.iter().find(|h| h.starts_with('-') || h.chars().any(|c| c.is_whitespace() || c == '/' || c == ',')) {
        return Err(ApiError::bad(format!("{h:?} is not a host: one name or pattern per entry, no scheme or path")));
    }
    let env = b.env.trim();
    let valid = env.chars().next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_') && env.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    if !valid {
        return Err(ApiError::bad("the variable name is letters, digits and _, not starting with a digit"));
    }
    if b.value.is_empty() {
        return Err(ApiError::bad("paste the secret's value"));
    }
    tracing::info!("web UI: storing a custom secret for {} as ${env}", hosts.join(", "));
    app.sbx.secret_set_custom(&hosts, env, &b.value).await?;
    Ok(ok())
}

#[derive(Deserialize)]
struct SecretRmBody {
    name: String,
}

/// Forget a global secret by the name `sbx secret ls` gives it.
async fn secret_rm(State(app): S, Json(b): Json<SecretRmBody>) -> ApiResult {
    let name = b.name.trim();
    if name.is_empty() || name.starts_with('-') || name.chars().any(|c| c.is_whitespace()) {
        return Err(ApiError::bad("not a secret name"));
    }
    tracing::info!("web UI: removing the global secret {name}");
    app.sbx.secret_rm(name).await?;
    Ok(ok())
}

async fn policy_refresh(State(app): S) -> ApiResult {
    tracing::info!("web UI: refreshing network policy");
    app.sbx.policy_refresh().await?;
    Ok(ok())
}

/// The host's registered MCP servers. They are the host's, not a sandbox's:
/// a sandbox only scopes load and unload.
async fn mcp_list(State(app): S) -> ApiResult {
    Ok(ok_with(json!({ "servers": app.sbx.mcp_ls().await? })))
}

async fn mcp_inspect(State(app): S, Path(server): Path<String>) -> ApiResult {
    let server = mcp::check_name(&server)?;
    Ok(ok_with(json!({ "text": app.sbx.mcp_inspect(&server).await? })))
}

#[derive(Deserialize)]
struct McpSaveBody {
    spec: mcp::McpSpec,
    /// The name being edited: removed once the new spec has been checked.
    replace: Option<String>,
    /// Header placeholder → value; an empty value keeps the stored secret.
    #[serde(default)]
    secrets: BTreeMap<String, String>,
}

/// Create a server, or edit one: sbx has no edit, so it is `rm` then `add`.
async fn mcp_save(State(app): S, Json(b): Json<McpSaveBody>) -> ApiResult {
    b.spec.add_args()?;
    let name = mcp::check_name(&b.spec.name)?;
    let keys = b.spec.placeholders();
    if let Some(old) = &b.replace {
        let old = mcp::check_name(old)?;
        tracing::info!("web UI: replacing MCP server {old} with {name}");
        app.sbx.mcp_rm(&old).await?;
    } else {
        tracing::info!("web UI: adding MCP server {name}");
    }
    app.sbx.mcp_add(&b.spec).await.map_err(|e| match &b.replace {
        Some(old) => anyhow::anyhow!("{old} was removed, but adding {name} failed: {e:#}"),
        None => e,
    })?;
    for (key, value) in b.secrets.iter().filter(|(k, v)| !v.is_empty() && keys.contains(k)) {
        app.sbx.secret_set(&mcp::secret_service(&name, key), value, None).await?;
    }
    Ok(ok())
}

async fn mcp_rm(State(app): S, Path(server): Path<String>) -> ApiResult {
    let server = mcp::check_name(&server)?;
    tracing::info!("web UI: removing MCP server {server}");
    app.sbx.mcp_rm(&server).await?;
    Ok(ok())
}

async fn mcp_auth(State(app): S, Path(server): Path<String>) -> ApiResult {
    let server = mcp::check_name(&server)?;
    tracing::info!("web UI: signing in to MCP server {server}");
    let text = app.sbx.mcp_auth(&server).await?;
    Ok(ok_with(json!({ "text": text })))
}

#[derive(Deserialize)]
struct McpServerBody {
    server: String,
}

async fn mcp_load(State(app): S, Path(name): Path<String>, Json(b): Json<McpServerBody>) -> ApiResult {
    let sb = sandbox(&name)?;
    let server = mcp::check_name(&b.server)?;
    tracing::info!("web UI: loading MCP server {server} into '{sb}'");
    app.sbx_for(&sb).await?.mcp_load(&server, &sb.name).await?;
    Ok(ok())
}

async fn mcp_unload(State(app): S, Path(name): Path<String>, Json(b): Json<McpServerBody>) -> ApiResult {
    let sb = sandbox(&name)?;
    let server = mcp::check_name(&b.server)?;
    tracing::info!("web UI: unloading MCP server {server} from '{sb}'");
    app.sbx_for(&sb).await?.mcp_unload(&server, &sb.name).await?;
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
        // Beside the workspace, so on the sandbox's host.
        let host = app.host_for(&sb).await?;
        if host.fs.lstat(&r.dest).await?.is_some() && !b.force.unwrap_or(false) {
            return Err(ApiError::conflict(format!("{} already exists — tick overwrite to replace it", r.dest.display())));
        }
        host.fs.write_atomic(&r.dest, r.yaml.as_bytes(), 0o644).await.map_err(|e| ApiError::bad(format!("writing {}: {e:#}", r.dest.display())))?;
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
    app.sbx_for(&sb).await?.write_file(&sb.name, &dest, &body, false).await?;
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
    app.sbx_for(&sb).await?.write_file(&sb.name, &dest, &body, false).await?;
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

// ── Remote hosts ─────────────────────────────────────────────────────────

/// A refusal of a hosts.toml change (a rule broken, a key not confirmed, a
/// host with sandboxes) is the request's fault: 400. A host that cannot be
/// reached stays 503, an unknown one 404, as everywhere else.
fn host_refusal(e: anyhow::Error) -> ApiError {
    let mapped = ApiError::from(e);
    if mapped.status == StatusCode::BAD_GATEWAY {
        ApiError::bad(mapped.message)
    } else {
        mapped
    }
}

fn host_name(raw: &str) -> Result<names::HostName, ApiError> {
    let h = names::HostName::parse(raw)?;
    if h.is_local() {
        return Err(ApiError::bad("'local' is this machine, not a remote host"));
    }
    Ok(h)
}

async fn remote_hosts(State(app): S) -> ApiResult {
    let hosts = app.remote_hosts_view()?;
    // The provision command is run on this machine, by this very binary.
    let sbxw = std::env::current_exe().map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|_| "sbxw".into());
    Ok(ok_with(json!({ "hosts": hosts, "sbxw": sbxw })))
}

#[derive(Deserialize)]
struct HostBody {
    #[serde(default)]
    name: String,
    #[serde(flatten)]
    form: crate::svc::hosts::HostForm,
    /// The fingerprints the person confirmed, from `/api/remote-hosts/key`.
    fingerprints: Option<Vec<String>>,
}

async fn remote_host_key(State(app): S, Json(b): Json<HostBody>) -> ApiResult {
    let h = b.form.to_host(b.name.trim()).map_err(host_refusal)?;
    let key = app.host_key(&h).await.map_err(host_refusal)?;
    Ok(ok_with(json!(key)))
}

async fn remote_host_add(State(app): S, Json(b): Json<HostBody>) -> ApiResult {
    app.host_add(b.name.trim(), &b.form, b.fingerprints.as_deref()).await.map_err(host_refusal)?;
    Ok(ok())
}

async fn remote_host_edit(State(app): S, Path(host): Path<String>, Json(b): Json<HostBody>) -> ApiResult {
    let h = host_name(&host)?;
    app.host_edit(h.as_str(), &b.form, b.fingerprints.as_deref()).await.map_err(host_refusal)?;
    Ok(ok())
}

#[derive(Deserialize)]
struct HostRmBody {
    #[serde(default)]
    force: bool,
}

async fn remote_host_rm(State(app): S, Path(host): Path<String>, Json(b): Json<HostRmBody>) -> ApiResult {
    let h = host_name(&host)?;
    app.host_rm(h.as_str(), b.force).await.map_err(host_refusal)?;
    Ok(ok())
}

async fn remote_host_test(State(app): S, Path(host): Path<String>) -> ApiResult {
    let t = app.host_test(&host_name(&host)?).await?;
    Ok(ok_with(json!(t)))
}

/// hosts.toml edited by hand (or by `sbxw host`): pick it up without a restart.
async fn remote_hosts_reload(State(app): S) -> ApiResult {
    app.reload_remotes().map_err(host_refusal)?;
    Ok(ok())
}

#[derive(Deserialize)]
struct CloneBody {
    url: String,
}

async fn remote_host_clone(State(app): S, Path(host): Path<String>, Json(b): Json<CloneBody>) -> ApiResult {
    let path = app.host_clone(&host_name(&host)?, &b.url).await?;
    Ok(ok_with(json!({ "path": path })))
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
    let repos = crate::git::list(&app.host_for(&sb).await?, &ws).await;
    Ok(ok_with(json!({ "workspace": ws, "repos": repos })))
}

async fn worktrees(State(app): S, Path(name): Path<String>) -> ApiResult {
    let (ws, repos) = app.worktree_graph(&sandbox(&name)?).await.map_err(ApiError::not_found)?;
    Ok(ok_with(json!({ "workspace": ws, "repos": repos })))
}

async fn thread_status(State(app): S, Path((name, thread)): Path<(String, String)>) -> ApiResult {
    let (sb, t) = (sandbox(&name)?, ThreadName::parse(&thread)?);
    let dir = app.settle_thread_repo(&sb, &t, None).await?;
    // A session in the main checkout has no worktree or branch to lose:
    // closing it only ends the session.
    if app.is_same_worktree(&sb, &t) {
        return Ok(ok_with(json!({ "status": {
            "worktree": dir, "branch": "", "base": "", "has_branch": false, "dirty": 0, "ahead": 0,
            "same_worktree": true,
        } })));
    }
    // A shared plan's thread leaves its plan: the shared worktree is not its
    // to remove, unless it is the last thread (and the worktree holds
    // nothing — the daemon checks again when it closes).
    if let Some(team) = app.team_for(&sb, &t) {
        let last = app.plans.lock().unwrap().team_of(&sb.to_string(), t.as_str()).is_some_and(|p| p.threads.len() == 1);
        return Ok(ok_with(json!({ "status": {
            "worktree": format!("{}/.sbxw-worktrees/{}", dir.trim_end_matches('/'), team.worktree),
            "branch": "", "base": "", "has_branch": false, "dirty": 0, "ahead": 0,
            "team": team.plan, "last": last,
        } })));
    }
    let st = app.sbx_for(&sb).await?.thread_status(&sb.name, &dir, &t).await?;
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
    /// A remote host's folders instead of this machine's.
    #[serde(default)]
    host: Option<String>,
}

/// Host folders for the create dialog's picker: this machine's, or with
/// `host=<name>` a remote host's, through the same `HostFs`.
async fn fs_browse(State(app): S, Query(q): Query<PathQuery>) -> ApiResult {
    let host = match non_blank(q.host) {
        None => Host::local(),
        Some(h) => {
            let h = sbxw_core::names::HostName::parse(&h)?;
            if h.is_local() {
                Host::local()
            } else {
                let r = app.remote(&h).await?;
                r.ready().await?;
                r.host.clone()
            }
        }
    };
    let base = match q.path {
        Some(p) => PathBuf::from(p),
        None => host.exec.home().await.unwrap_or_else(|_| "/".into()),
    };
    let dir = host.fs.canonicalize(&base).await.unwrap_or(base);
    let mut entries: Vec<Value> = Vec::new();
    for e in host.fs.read_dir(&dir).await.unwrap_or_default() {
        if e.name.starts_with('.') {
            continue;
        }
        // A link to a folder is a folder here, as it was with `is_dir`.
        let path = dir.join(&e.name);
        let is_dir = match e.meta.kind {
            Kind::Dir => true,
            Kind::Symlink => host.fs.stat(&path).await.ok().flatten().is_some_and(|m| m.kind == Kind::Dir),
            _ => false,
        };
        if is_dir {
            entries.push(json!({ "name": e.name, "path": path }));
        }
    }
    entries.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
    Ok(Json(json!({ "path": dir, "parent": dir.parent(), "entries": entries })))
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

/// Starred folders are this machine's: they feed its picker.
async fn favourites_payload(list: Vec<String>) -> Value {
    let fs = Host::local().fs;
    let mut dirs = std::collections::HashSet::new();
    for p in &list {
        if fs.stat(std::path::Path::new(p)).await.ok().flatten().is_some_and(|m| m.kind == Kind::Dir) {
            dirs.insert(p.clone());
        }
    }
    json!({ "favourites": favourites::payload(list, |p| dirs.contains(p)) })
}

async fn favourites_get(State(app): S) -> Json<Value> {
    Json(favourites_payload(app.store.favourites()).await)
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
    let fs = Host::local().fs;
    let canonical = fs.canonicalize(std::path::Path::new(raw)).await.ok();
    if b.favourite {
        let c = canonical.ok_or_else(|| ApiError::bad(format!("cannot star '{raw}'")))?;
        if !fs.stat(&c).await.ok().flatten().is_some_and(|m| m.kind == Kind::Dir) {
            return Err(ApiError::bad(format!("'{raw}' is not a folder")));
        }
        favourites::star(&mut list, c.to_string_lossy().into_owned())?;
    } else {
        let c = canonical.map(|c| c.to_string_lossy().into_owned());
        favourites::unstar(&mut list, raw, c.as_deref());
    }
    app.store.set_favourites(&list)?;
    let mut payload = favourites_payload(list).await;
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
    let run = app.codemap_run(&sb.to_string());
    let Some(ws) = app.store.workspace(&sb) else {
        return Ok(Json(json!({ "dir": "", "entries": [], "codemap": null, "lenses": [], "run": run })));
    };
    let dir = ws.join(artifacts::ARTIFACTS_DIR);
    let entries = walk_artifacts(&app.host_for(&sb).await?, &dir).await;
    let (entries, codemap, lenses) = artifacts::fold(entries);
    Ok(Json(json!({ "dir": dir, "entries": entries, "codemap": codemap, "lenses": lenses, "run": run })))
}

/// The deliverables under `root`: allowed extensions, no hidden file or
/// folder, links not followed. One walk of the host, filtered here because a
/// walk prunes by exact name and "hidden" is a prefix.
async fn walk_artifacts(host: &Host, root: &std::path::Path) -> Vec<artifacts::Entry> {
    if !host.fs.stat(root).await.ok().flatten().is_some_and(|m| m.kind == Kind::Dir) {
        return Vec::new();
    }
    let all = host.fs.walk(root, artifacts::MAX_DEPTH as usize + 1, &[]).await.unwrap_or_default();
    all.into_iter()
        .filter(|(rel, m)| m.kind == Kind::File && !rel.split('/').any(|c| c.starts_with('.')))
        .filter_map(|(rel, m)| {
            let name = rel.rsplit('/').next().unwrap_or(&rel).to_string();
            artifacts::has_allowed_extension(&name).then_some(artifacts::Entry { path: rel, name, size: m.len, modified: m.mtime_secs })
        })
        .collect()
}

/// One file of `.sbxw-artifacts`, canonicalised and checked to stay inside.
async fn artifact_download(State(app): S, Path(name): Path<String>, Query(q): Query<PathQuery>) -> Result<Response, ApiError> {
    let sb = sandbox(&name)?;
    let ws = app.store.workspace(&sb).ok_or_else(|| ApiError::not_found("unknown sandbox"))?;
    let host = app.host_for(&sb).await?;
    let root = host.fs.canonicalize(&ws.join(artifacts::ARTIFACTS_DIR)).await.map_err(|_| ApiError::not_found("no artifacts directory"))?;
    let file = host
        .fs
        .canonicalize(&root.join(q.path.unwrap_or_default()))
        .await
        .map_err(|_| ApiError::not_found("file not found"))?;
    if !file.starts_with(&root) || !host.fs.stat(&file).await.ok().flatten().is_some_and(|m| m.kind == Kind::File) {
        return Err(ApiError::forbidden("invalid path"));
    }
    let data = host.fs.read(&file, sandbox_fs::MAX_FILE).await.map_err(|_| ApiError::not_found("file not found"))?;
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
    let (sbx, host) = (app.sbx_for(&sb).await?, app.host_for(&sb).await?);
    let l = sandbox_fs::list(&sbx, &host, &sb.name, ws.as_deref(), &q.path).await.map_err(|e| ApiError::not_found(format!("{e:#}")))?;
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
async fn thread_worktrees(app: &App, sb: &SandboxRef) -> Vec<Value> {
    let Some(ws) = app.store.workspace(sb) else { return vec![] };
    let recorded: std::collections::BTreeMap<String, String> =
        app.store.get(sb).threads.iter().map(|(k, v)| (k.clone(), v.repo.clone())).collect();
    let Ok(host) = app.host_for(sb).await else { return vec![] };
    let mut out = Vec::new();
    for (thread, repo) in crate::git::thread_worktrees(&host, &ws, &recorded).await {
        let Some(root) = crate::git::repo_root(&host, &ws, &repo).await else { continue };
        let dir = root.join(".sbxw-worktrees").join(&thread);
        let arts = dir.join(artifacts::ARTIFACTS_DIR);
        let has = host.fs.stat(&arts).await.ok().flatten().is_some_and(|m| m.kind == Kind::Dir);
        out.push(json!({ "thread": thread, "repo": repo, "path": dir, "artifacts": has.then_some(arts) }));
    }
    out
}

/// What was written lately, wherever: the workspace, the read-write mounts,
/// home and `/tmp` — an agent's file is not always where it was asked to go.
async fn sandbox_fs_recent(State(app): S, Path(name): Path<String>) -> ApiResult {
    let sb = sandbox(&name)?;
    let ws = app.store.workspace(&sb);
    let ws_s = ws.as_ref().map(|w| w.to_string_lossy().into_owned());
    let roots = writable_roots(&app, &sb, ws_s.as_deref());
    let (sbx, host) = (app.sbx_for(&sb).await?, app.host_for(&sb).await?);
    let (files, offline) = sandbox_fs::recent(&sbx, &host, &sb.name, ws.as_deref(), &roots)
        .await
        .map_err(|e| ApiError::not_found(format!("{e:#}")))?;
    Ok(ok_with(json!({
        "files": files, "offline": offline, "roots": roots, "days": sandbox_fs::RECENT_DAYS,
        "workspace": ws_s, "home": sandbox_fs::SANDBOX_HOME,
    })))
}

/// Where an agent writes: the workspace, the read-write mounts, home and
/// `/tmp`. What Recent scans, and the only places a file may be deleted from.
fn writable_roots(app: &App, sb: &SandboxRef, ws: Option<&str>) -> Vec<String> {
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
    let (sbx, host) = (app.sbx_for(&sb).await?, app.host_for(&sb).await?);
    sandbox_fs::delete(&sbx, &host, &sb.name, ws.as_deref(), &b.path, &roots, &protected)
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
    let (sbx, host) = (app.sbx_for(&sb).await?, app.host_for(&sb).await?);
    let data = sandbox_fs::read(&sbx, &host, &sb.name, ws.as_deref(), &q.path, max, q.head.is_some())
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
    // Any ref: the page opens on it, and a remote one's pane says what is missing.
    let sb = SandboxRef::parse(&b.sandbox)?;
    *app.initial_sandbox.lock().unwrap() = sb.to_string();
    Ok(ok())
}

/// `sbxw down`: stop every session, then exit.
async fn shutdown(State(app): S) -> Json<Value> {
    for (key, s) in app.sessions.all() {
        tracing::info!("shutting down: closing {key}");
        s.kill();
    }
    // Before exit(): llama.cpp's Metal backend asserts in its static
    // destructors when a model is still loaded, which turns a clean stop
    // into a crash report.
    app.voice_unload();
    tokio::spawn(async {
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        std::process::exit(0);
    });
    ok()
}

/// The thread a WebSocket asks for may choose its repo and model on first open.
pub async fn settle_ws_thread(
    app: &App,
    sb: &SandboxRef,
    kind: &SessionKind,
    repo: Option<&str>,
    model: Option<&str>,
    checkout: bool,
) -> Result<(), String> {
    if let SessionKind::Thread(t) = kind {
        if checkout {
            app.settle_same_worktree(sb, t).await?;
        }
        if !checkout && (repo.is_some() || model.is_some()) {
            app.settle_thread_repo(sb, t, repo).await?;
        }
        if let Some(m) = model.filter(|m| !m.is_empty()) {
            let m = ModelId::parse(m).map_err(|e| e.to_string())?;
            app.settle_thread_model(sb, t, &m, false)?;
        }
    }
    let _ = SessionKey::new(sb.clone(), kind.clone());
    Ok(())
}

// ── Voice ────────────────────────────────────────────────────────────────

async fn voice_status(State(app): S) -> Json<Value> {
    ok_with(app.voice_status())
}

async fn voice_install(State(app): S) -> ApiResult {
    app.voice_install().map_err(ApiError::conflict)?;
    Ok(ok_with(app.voice_status()))
}

async fn voice_cancel(State(app): S) -> ApiResult {
    app.voice_cancel().map_err(ApiError::conflict)?;
    Ok(ok())
}

async fn voice_rm(State(app): S) -> ApiResult {
    app.voice_remove().map_err(ApiError::conflict)?;
    Ok(ok_with(app.voice_status()))
}

/// The body is 16 kHz mono little-endian f32 PCM — the page resamples, so
/// no audio container is parsed here.
async fn voice_transcribe(State(app): S, body: Bytes) -> ApiResult {
    let samples = crate::svc::voice::samples_of(&body)?;
    let secs = samples.len() as f32 / crate::svc::voice::SAMPLE_RATE as f32;
    let t = std::time::Instant::now();
    let text = app
        .voice_transcribe(samples)
        .await
        .map_err(|e| ApiError::new(StatusCode::SERVICE_UNAVAILABLE, e))?;
    Ok(ok_with(json!({ "text": text.trim(), "audio_s": secs, "ms": t.elapsed().as_millis() as u64 })))
}

#[derive(Deserialize)]
struct VoiceChatBody {
    turns: Vec<crate::svc::voice::Turn>,
}

/// The reply as NDJSON, one `ChatEvent` a line, written as it is generated:
/// the page shows (and, later, speaks) the first words before the last exist.
async fn voice_chat(State(app): S, Json(b): Json<VoiceChatBody>) -> ApiResult<Response> {
    use futures_util::StreamExt;
    let rx = app
        .voice_chat(b.turns)
        .await
        .map_err(|e| if e.contains("turn") || e.contains("conversation") { ApiError::bad(e) } else { ApiError::new(StatusCode::SERVICE_UNAVAILABLE, e) })?;
    let lines = tokio_stream::wrappers::ReceiverStream::new(rx).map(|ev| {
        let mut line = serde_json::to_string(&ev).unwrap_or_default();
        line.push('\n');
        Ok::<_, std::convert::Infallible>(Bytes::from(line))
    });
    Ok(([(header::CONTENT_TYPE, "application/x-ndjson")], Body::from_stream(lines)).into_response())
}

#[derive(Deserialize)]
struct VoiceSpeakBody {
    text: String,
}

async fn voice_speak(State(app): S, Json(b): Json<VoiceSpeakBody>) -> ApiResult<Response> {
    crate::svc::voice::check_speech(&b.text)?;
    let wav = app.voice_speak(b.text).await.map_err(|e| ApiError::new(StatusCode::SERVICE_UNAVAILABLE, e))?;
    Ok(([(header::CONTENT_TYPE, "audio/wav")], wav).into_response())
}
