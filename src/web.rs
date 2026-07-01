//! Browser terminal with sandbox switcher sidebar.
//!
//! PTY sessions are persistent — they survive WebSocket disconnects.
//! Refreshing the browser tab replays the last 256 KB of output and
//! resumes the live stream without restarting the agent.
//!
//! Routes:
//!   GET  /                          → HTML (initial_sandbox embedded)
//!   GET  /api/sandboxes             → JSON list from `sbx ls`
//!   POST /api/sandboxes/create      → create a new sandbox
//!   POST /api/sandboxes/:name/stop  → `sbx stop <name>`
//!   GET  /api/fs?path=<dir>         → directory listing for the folder picker
//!   GET  /api/sandboxes/:name/artifacts             → non-code files under .sbxw-artifacts
//!   GET  /api/sandboxes/:name/artifacts/download     → download one of those files
//!   GET  /ws?sandbox=<name>         → WebSocket ↔ persistent PTY

use crate::config::Config;
use crate::hosts::{self, HostAlias};
use crate::sbx;
use crate::ExtraPort;
use anyhow::{Context, Result};
use axum::{
    body::Bytes,
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        DefaultBodyLimit, Path, Query, State,
    },
    http::{header, HeaderMap, StatusCode},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use futures_util::{SinkExt, StreamExt};
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, VecDeque},
    io::Write,
    sync::{Arc, Mutex},
};
use tokio::sync::broadcast;

/// Output bytes kept per sandbox for replay on reconnect (256 KB).
const REPLAY_BYTES: usize = 256 * 1024;

/// Persistent PTY state shared across all WebSocket connections to the same sandbox.
struct PtySession {
    /// Broadcast sender: every connected WebSocket subscribes to this.
    tx: broadcast::Sender<Vec<u8>>,
    /// PTY input writer — shared across connections.
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    /// PTY master kept for resize operations.
    master: Arc<Mutex<Box<dyn portable_pty::MasterPty + Send>>>,
    /// Ring buffer for replaying output to newly connected WebSockets.
    replay: Arc<Mutex<VecDeque<u8>>>,
    /// Fires whenever the PTY emits a BEL (0x07) — the agent's "I need you" signal.
    bell_tx: broadcast::Sender<()>,
    /// Child process handle — kept alive so the process is properly reaped on exit.
    _child: Mutex<Box<dyn portable_pty::Child + Send + Sync>>,
}

type Sessions = Arc<Mutex<HashMap<String, Arc<PtySession>>>>;

#[derive(Clone)]
struct AppState {
    initial_sandbox: String,
    shell: String,
    sessions: Sessions,
    cfg: Arc<Config>,
    use_api_key: bool,
}

#[derive(Serialize)]
struct SandboxItem {
    name: String,
    agent: String,
    status: String,
}

#[derive(Deserialize)]
struct WsQuery {
    sandbox: Option<String>,
    /// "claude" (default) attaches the agent; "bash" opens a shell via sbx exec.
    mode: Option<String>,
}

const INDEX_HTML_TEMPLATE: &str = include_str!("../assets/index.html");

pub async fn serve(
    addr: &str,
    initial_sandbox: String,
    shell: String,
    cfg: Arc<Config>,
    use_api_key: bool,
) -> Result<()> {
    let state = Arc::new(AppState {
        initial_sandbox,
        shell,
        sessions: Arc::new(Mutex::new(HashMap::new())),
        cfg,
        use_api_key,
    });

    let app = Router::new()
        .route("/", get(index_handler))
        .route("/ws", get(ws_handler))
        .route("/api/sandboxes", get(api_list))
        .route("/api/sandboxes/create", post(api_create))
        .route("/api/sandboxes/:name/ports", get(api_ports_one))
        .route(
            "/api/sandboxes/:name/ports/publish",
            post(api_ports_publish),
        )
        .route(
            "/api/sandboxes/:name/ports/unpublish",
            post(api_ports_unpublish),
        )
        .route("/api/hosts", get(api_hosts_read))
        .route("/api/sandboxes/:name/stop", post(api_stop))
        .route("/api/sandboxes/:name/rm", post(api_rm))
        .route(
            "/api/sandboxes/:name/paste-image",
            // Screenshots are easily a few MB; lift the 2 MB default body cap.
            post(api_paste_image).layer(DefaultBodyLimit::max(32 * 1024 * 1024)),
        )
        .route("/api/fs", get(api_fs))
        .route("/api/sandboxes/:name/artifacts", get(api_artifacts))
        .route(
            "/api/sandboxes/:name/artifacts/download",
            get(api_artifact_download),
        )
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!("web TTY listening on http://{addr}");
    axum::serve(listener, app).await?;
    Ok(())
}

async fn index_handler(State(state): State<Arc<AppState>>) -> Html<String> {
    Html(INDEX_HTML_TEMPLATE.replace("__SANDBOX__", &state.initial_sandbox))
}

async fn api_list() -> Json<Vec<SandboxItem>> {
    let items = tokio::task::spawn_blocking(sbx::list_sandboxes)
        .await
        .unwrap_or_default();
    Json(
        items
            .into_iter()
            .map(|s| SandboxItem {
                name: s.name,
                agent: s.agent,
                status: s.status,
            })
            .collect(),
    )
}

#[derive(Serialize)]
struct PortMappingJson {
    sandbox_port: u16,
    proto: String,
    host_ip: String,
    host_port: u16,
    spec: String,
}

#[derive(Serialize)]
struct SandboxPorts {
    ports: Vec<PortMappingJson>,
}

async fn api_ports_one(Path(name): Path<String>) -> Json<SandboxPorts> {
    let ports = tokio::task::spawn_blocking(move || sbx::list_ports_parsed(&name))
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|p| PortMappingJson {
            spec: p.spec(),
            sandbox_port: p.sandbox_port,
            proto: p.proto,
            host_ip: p.host_ip,
            host_port: p.host_port,
        })
        .collect();
    Json(SandboxPorts { ports })
}

#[derive(Deserialize)]
struct PortSpecBody {
    spec: String,
}

#[derive(Deserialize)]
struct PublishBody {
    sandbox_port: u16,
    host_port: Option<u16>,
    /// Bind the host side to this IP (e.g. "127.0.0.2"). Defaults to 127.0.0.1.
    host_ip: Option<String>,
    /// If set, add/update this hostname → host_ip in the sbxw /etc/hosts block.
    alias: Option<String>,
}

async fn api_ports_publish(
    Path(name): Path<String>,
    Json(body): Json<PublishBody>,
) -> Json<serde_json::Value> {
    match tokio::task::spawn_blocking(move || {
        let host_port = body.host_port.unwrap_or(body.sandbox_port);
        let host_ip = body.host_ip.clone().unwrap_or_else(|| "127.0.0.1".into());

        // 1. Ensure the host IP exists on lo0 BEFORE sbx tries to bind to it.
        let lo_entry = HostAlias {
            hostname: String::new(),
            ip: host_ip.clone(),
        };
        hosts::ensure_loopback_aliases(&[lo_entry])
            .context("failed to create loopback alias — run: sudo ifconfig lo0 alias <ip> up")?;

        // 2. Publish the port now that the IP is bound.
        let spec = format!("{host_ip}:{host_port}:{}", body.sandbox_port);
        sbx::publish_port(&name, &spec)?;

        // 3. If an alias was requested, upsert it in the sbxw /etc/hosts block.
        //    Reported separately so a sudo/tty failure doesn't hide the publish success.
        let hosts_result: Option<String> = if let Some(ref alias) = body.alias {
            let alias = alias.trim();
            if !alias.is_empty() {
                let new_entry = HostAlias {
                    hostname: alias.to_string(),
                    ip: host_ip.clone(),
                };
                let mut entries: Vec<HostAlias> = hosts::read_hosts_block()
                    .into_iter()
                    .filter(|a| a.hostname != new_entry.hostname)
                    .collect();
                entries.push(new_entry);
                match hosts::sync_hosts_block(&entries) {
                    Ok(()) => {
                        // Verify the write actually landed.
                        let written = hosts::read_hosts_block();
                        if written.iter().any(|a| a.hostname == alias) {
                            None // success
                        } else {
                            Some(format!(
                                "/etc/hosts write succeeded but alias not found — \
                                 run manually: echo '{host_ip}\\t{alias}' | sudo tee -a /etc/hosts"
                            ))
                        }
                    }
                    Err(e) => Some(format!(
                        "failed to update /etc/hosts ({e:#}) — \
                         run manually: echo '{host_ip}\\t{alias}' | sudo tee -a /etc/hosts"
                    )),
                }
            } else {
                None
            }
        } else {
            None
        };

        Ok::<_, anyhow::Error>(hosts_result)
    })
    .await
    {
        Ok(Ok(None)) => Json(serde_json::json!({ "ok": true })),
        Ok(Ok(Some(warn))) => Json(serde_json::json!({ "ok": true, "hosts_warning": warn })),
        Ok(Err(e)) => Json(serde_json::json!({ "ok": false, "error": e.to_string() })),
        Err(_) => Json(serde_json::json!({ "ok": false, "error": "task panic" })),
    }
}

async fn api_ports_unpublish(
    Path(name): Path<String>,
    Json(body): Json<PortSpecBody>,
) -> Json<serde_json::Value> {
    let spec = body.spec.clone();
    match tokio::task::spawn_blocking(move || sbx::unpublish_port(&name, &spec)).await {
        Ok(Ok(())) => Json(serde_json::json!({ "ok": true })),
        Ok(Err(e)) => Json(serde_json::json!({ "ok": false, "error": e.to_string() })),
        Err(_) => Json(serde_json::json!({ "ok": false, "error": "task panic" })),
    }
}

// ── /etc/hosts aliases ────────────────────────────────────────────────────────

#[derive(Serialize)]
struct HostEntry {
    hostname: String,
    ip: String,
}

async fn api_hosts_read() -> Json<Vec<HostEntry>> {
    Json(
        tokio::task::spawn_blocking(hosts::read_hosts_block)
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|a| HostEntry {
                hostname: a.hostname,
                ip: a.ip,
            })
            .collect(),
    )
}

async fn api_stop(Path(name): Path<String>) -> Json<serde_json::Value> {
    match tokio::task::spawn_blocking(move || sbx::stop_sandbox(&name)).await {
        Ok(Ok(())) => Json(serde_json::json!({ "ok": true })),
        Ok(Err(e)) => Json(serde_json::json!({ "ok": false, "error": e.to_string() })),
        Err(_) => Json(serde_json::json!({ "ok": false, "error": "task panic" })),
    }
}

async fn api_rm(Path(name): Path<String>) -> Json<serde_json::Value> {
    match tokio::task::spawn_blocking(move || {
        let n = name.clone();
        sbx::rm_sandboxes(&[n.as_str()], false)
    })
    .await
    {
        Ok(Ok(())) => Json(serde_json::json!({ "ok": true })),
        Ok(Err(e)) => Json(serde_json::json!({ "ok": false, "error": e.to_string() })),
        Err(_) => Json(serde_json::json!({ "ok": false, "error": "task panic" })),
    }
}

// ── Pasted image upload ───────────────────────────────────────────────────────

/// Map an image MIME type to a file extension. Defaults to `png` for anything
/// unrecognised (clipboard screenshots are almost always PNG).
fn ext_for_mime(mime: &str) -> &'static str {
    match mime.split(';').next().unwrap_or("").trim() {
        "image/jpeg" => "jpg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "image/bmp" => "bmp",
        "image/svg+xml" => "svg",
        _ => "png",
    }
}

/// `POST /api/sandboxes/:name/paste-image` — write a clipboard image into the
/// sandbox and return its in-sandbox path. The browser then types that path
/// into the terminal so the agent can read the file.
async fn api_paste_image(
    Path(name): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Json<serde_json::Value> {
    if body.is_empty() {
        return Json(serde_json::json!({ "ok": false, "error": "empty image" }));
    }
    let ext = headers
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(ext_for_mime)
        .unwrap_or("png");
    // Millisecond timestamp keeps names unique and chronologically sortable.
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let dest = format!("/tmp/sbxw-pastes/paste-{ts}.{ext}");
    let data = body.to_vec();
    let dest_ret = dest.clone();
    match tokio::task::spawn_blocking(move || sbx::write_file_stdin(&name, &dest, &data)).await {
        Ok(Ok(())) => Json(serde_json::json!({ "ok": true, "path": dest_ret })),
        Ok(Err(e)) => Json(serde_json::json!({ "ok": false, "error": e.to_string() })),
        Err(_) => Json(serde_json::json!({ "ok": false, "error": "task panic" })),
    }
}

// ── Sandbox creation ──────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct PortEntry {
    sandbox_port: u16,
    host_port: Option<u16>,
    #[serde(default)]
    alias: Option<String>,
}

#[derive(Deserialize)]
struct CreateBody {
    name: String,
    path: String,
    #[serde(default)]
    ports: Vec<PortEntry>,
}

async fn api_create(
    State(state): State<Arc<AppState>>,
    Json(body): Json<CreateBody>,
) -> Json<serde_json::Value> {
    // Validate: name must be non-empty alphanumeric/hyphens, path must exist.
    let name = body.name.trim().to_string();
    let path = body.path.trim().to_string();

    if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return Json(serde_json::json!({
            "ok": false,
            "error": "name must be non-empty and contain only letters, digits, and hyphens"
        }));
    }
    if !std::path::Path::new(&path).is_dir() {
        return Json(serde_json::json!({ "ok": false, "error": "path is not a directory" }));
    }

    let cfg = state.cfg.clone();
    let use_api_key = state.use_api_key;
    let extra_ports: Vec<ExtraPort> = body
        .ports
        .into_iter()
        .map(|pe| ExtraPort {
            sandbox_port: pe.sandbox_port,
            host_port: pe.host_port.unwrap_or(pe.sandbox_port),
            alias: pe.alias.unwrap_or_default(),
        })
        .collect();
    tracing::info!(
        "web UI: provisioning sandbox '{name}' at {path} ({} extra ports)",
        extra_ports.len()
    );
    match tokio::task::spawn_blocking(move || {
        crate::provision_sandbox(&name, &path, &[], &cfg, &extra_ports, use_api_key)
    })
    .await
    {
        Ok(Ok(())) => Json(serde_json::json!({ "ok": true })),
        Ok(Err(e)) => Json(serde_json::json!({ "ok": false, "error": e.to_string() })),
        Err(_) => Json(serde_json::json!({ "ok": false, "error": "task panic" })),
    }
}

// ── Filesystem browser ────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct FsQuery {
    path: Option<String>,
}

#[derive(Serialize)]
struct FsEntry {
    name: String,
    path: String,
}

#[derive(Serialize)]
struct FsResponse {
    path: String,
    parent: Option<String>,
    entries: Vec<FsEntry>,
}

async fn api_fs(Query(params): Query<FsQuery>) -> Json<FsResponse> {
    let base = params
        .path
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::env::var("HOME")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|_| "/".into())
        });
    // Canonicalize prevents path traversal and resolves symlinks.
    let dir = base.canonicalize().unwrap_or(base);

    let parent = dir.parent().map(|p| p.to_string_lossy().into_owned());

    let mut entries: Vec<FsEntry> = std::fs::read_dir(&dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .filter(|e| !e.file_name().to_string_lossy().starts_with('.'))
        .map(|e| FsEntry {
            name: e.file_name().to_string_lossy().into_owned(),
            path: e.path().to_string_lossy().into_owned(),
        })
        .collect();

    entries.sort_by(|a, b| a.name.cmp(&b.name));

    Json(FsResponse {
        path: dir.to_string_lossy().into_owned(),
        parent,
        entries,
    })
}

// ── Generated-files ("artifacts") panel ───────────────────────────────────────
//
// Convention, not enforcement: sbxw just lists and serves whatever non-code
// files (by extension) it finds under `<workspace>/.sbxw-artifacts`. Since the
// workspace is bind-mounted straight from the host, this needs no `sbx exec`
// round-trip — it reads the host side of the mount directly.

const ARTIFACT_EXTENSIONS: &[&str] = &[
    "md", "markdown", "pdf", "png", "jpg", "jpeg", "gif", "svg", "webp", "docx", "pptx", "xlsx",
    "csv", "html", "txt",
];

const MAX_ARTIFACT_DEPTH: u32 = 6;

#[derive(Serialize)]
struct ArtifactEntry {
    /// Path relative to `.sbxw-artifacts`, forward-slash separated.
    path: String,
    name: String,
    size: u64,
    /// Unix seconds.
    modified: u64,
}

#[derive(Serialize)]
struct ArtifactsResponse {
    dir: String,
    entries: Vec<ArtifactEntry>,
}

fn has_allowed_extension(path: &std::path::Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| ARTIFACT_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

fn walk_artifacts(
    root: &std::path::Path,
    dir: &std::path::Path,
    out: &mut Vec<ArtifactEntry>,
    depth: u32,
) {
    if depth > MAX_ARTIFACT_DEPTH {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in rd.flatten() {
        let path = entry.path();
        let file_name = entry.file_name().to_string_lossy().into_owned();
        if file_name.starts_with('.') {
            continue;
        }
        let Ok(ft) = entry.file_type() else {
            continue;
        };
        if ft.is_dir() {
            walk_artifacts(root, &path, out, depth + 1);
        } else if ft.is_file() && has_allowed_extension(&path) {
            let Ok(meta) = entry.metadata() else {
                continue;
            };
            let modified = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let rel = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            out.push(ArtifactEntry {
                path: rel,
                name: file_name,
                size: meta.len(),
                modified,
            });
        }
    }
}

fn collect_artifacts(dir: &std::path::Path) -> Vec<ArtifactEntry> {
    let mut out = Vec::new();
    if dir.is_dir() {
        walk_artifacts(dir, dir, &mut out, 0);
    }
    out.sort_by(|a, b| b.modified.cmp(&a.modified));
    out
}

async fn api_artifacts(Path(name): Path<String>) -> Json<ArtifactsResponse> {
    let Some(workspace) = crate::workspace_for(&name) else {
        return Json(ArtifactsResponse {
            dir: String::new(),
            entries: Vec::new(),
        });
    };
    let dir = workspace.join(crate::ARTIFACTS_DIR);
    let dir_str = dir.to_string_lossy().into_owned();
    let entries = tokio::task::spawn_blocking(move || collect_artifacts(&dir))
        .await
        .unwrap_or_default();
    Json(ArtifactsResponse {
        dir: dir_str,
        entries,
    })
}

fn guess_mime(filename: &str) -> &'static str {
    let ext = filename
        .rsplit('.')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        "md" | "markdown" => "text/markdown",
        "pdf" => "application/pdf",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        "webp" => "image/webp",
        "csv" => "text/csv",
        "html" => "text/html",
        "txt" => "text/plain",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        _ => "application/octet-stream",
    }
}

#[derive(Deserialize)]
struct ArtifactDownloadQuery {
    path: String,
}

/// Streams a single file back from `<workspace>/.sbxw-artifacts`. The
/// requested `path` is resolved and canonicalized, then checked to still be
/// inside the artifacts directory — this is what actually blocks `../`
/// traversal, not the string itself.
async fn api_artifact_download(
    Path(name): Path<String>,
    Query(params): Query<ArtifactDownloadQuery>,
) -> Response {
    let Some(workspace) = crate::workspace_for(&name) else {
        return (StatusCode::NOT_FOUND, "unknown sandbox").into_response();
    };
    let dir = workspace.join(crate::ARTIFACTS_DIR);
    let Ok(dir_canon) = dir.canonicalize() else {
        return (StatusCode::NOT_FOUND, "no artifacts directory").into_response();
    };
    let candidate = dir.join(&params.path);
    let Ok(candidate_canon) = candidate.canonicalize() else {
        return (StatusCode::NOT_FOUND, "file not found").into_response();
    };
    if !candidate_canon.starts_with(&dir_canon) || !candidate_canon.is_file() {
        return (StatusCode::FORBIDDEN, "invalid path").into_response();
    }
    let read_path = candidate_canon.clone();
    let data = match tokio::task::spawn_blocking(move || std::fs::read(read_path)).await {
        Ok(Ok(d)) => d,
        _ => return (StatusCode::NOT_FOUND, "file not found").into_response(),
    };
    let filename = candidate_canon
        .file_name()
        .map(|n| n.to_string_lossy().replace('"', ""))
        .unwrap_or_else(|| "download".into());
    let mime = guess_mime(&filename);
    (
        [
            (header::CONTENT_TYPE, mime.to_string()),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{filename}\""),
            ),
        ],
        data,
    )
        .into_response()
}

async fn ws_handler(
    ws: WebSocketUpgrade,
    Query(params): Query<WsQuery>,
    State(state): State<Arc<AppState>>,
) -> axum::response::Response {
    let sandbox = params
        .sandbox
        .unwrap_or_else(|| state.initial_sandbox.clone());
    // "bash" → shell session; anything else → the agent ("claude").
    let mode = match params.mode.as_deref() {
        Some("bash") => "bash",
        _ => "claude",
    }
    .to_string();
    ws.on_upgrade(move |socket| {
        handle_socket(
            socket,
            sandbox,
            mode,
            state.shell.clone(),
            state.sessions.clone(),
        )
    })
}

async fn handle_socket(
    socket: WebSocket,
    sandbox: String,
    mode: String,
    shell: String,
    sessions: Sessions,
) {
    if let Err(e) = bridge(socket, sandbox, mode, shell, sessions).await {
        tracing::warn!("tty bridge ended: {e:#}");
    }
}

/// Return the existing PTY session for (`sandbox`, `mode`), or create one.
/// Sessions are keyed by "<sandbox>::<mode>" so the agent ("claude") and a
/// bash shell coexist independently for the same sandbox.
///   mode == "bash"  → `sbx exec -it <sandbox> -- bash`
///   mode == "claude"→ `sbx run --name <sandbox>` (or the configured web_shell via exec)
/// The session lives until the PTY process exits.
fn get_or_create_session(
    sandbox: &str,
    mode: &str,
    shell: &str,
    sessions: &Sessions,
) -> Result<Arc<PtySession>> {
    let session_key = format!("{sandbox}::{mode}");

    // Fast path: session already exists.
    if let Some(s) = sessions.lock().unwrap().get(&session_key) {
        return Ok(s.clone());
    }

    // Slow path: spin up a new PTY.
    let pty = native_pty_system();
    let pair = pty.openpty(PtySize {
        rows: 30,
        cols: 100,
        pixel_width: 0,
        pixel_height: 0,
    })?;

    let mut cmd = CommandBuilder::new("sbx");
    if mode == "bash" {
        cmd.args(["exec", "-it", sandbox, "--", "bash"]);
    } else if shell.is_empty() {
        // Re-attach by name. The positional form (`sbx run <name>`) is
        // deprecated; `--name` re-attaches regardless of working directory.
        cmd.args(["run", "--name", sandbox]);
    } else {
        cmd.args(["exec", "-it", sandbox, "--", shell]);
    }
    cmd.env("TERM", "xterm-256color");

    let child = pair.slave.spawn_command(cmd)?;
    drop(pair.slave); // slave fd no longer needed on the host side

    let mut reader = pair.master.try_clone_reader()?;
    let writer = Arc::new(Mutex::new(pair.master.take_writer()?));
    let master = Arc::new(Mutex::new(pair.master));
    let replay: Arc<Mutex<VecDeque<u8>>> =
        Arc::new(Mutex::new(VecDeque::with_capacity(REPLAY_BYTES)));

    // Broadcast channel capacity: 256 chunks. Slow receivers are warned, not killed.
    let (tx, _) = broadcast::channel::<Vec<u8>>(256);
    let (bell_tx, _) = broadcast::channel::<()>(16);

    let session = Arc::new(PtySession {
        tx: tx.clone(),
        writer,
        master,
        replay: replay.clone(),
        bell_tx: bell_tx.clone(),
        _child: Mutex::new(child),
    });

    sessions
        .lock()
        .unwrap()
        .insert(session_key.clone(), session.clone());

    // Background reader thread: PTY output → replay buffer → broadcast.
    let sessions_ref = sessions.clone();
    let sandbox_key = session_key.clone();
    std::thread::spawn(move || {
        let mut buf = [0u8; 8192];
        // Debounces the bell signal — agents can emit several BELs in a row
        // for one prompt, and we only want one notification out of that burst.
        let mut last_bell = std::time::Instant::now() - std::time::Duration::from_secs(3);
        loop {
            match std::io::Read::read(&mut reader, &mut buf) {
                Ok(0) => break, // PTY closed (process exited)
                Ok(n) => {
                    let chunk = buf[..n].to_vec();
                    if chunk.contains(&0x07)
                        && last_bell.elapsed() >= std::time::Duration::from_secs(3)
                    {
                        last_bell = std::time::Instant::now();
                        let _ = bell_tx.send(());
                    }
                    // Append to replay ring buffer.
                    {
                        let mut r = replay.lock().unwrap();
                        for &b in &chunk {
                            if r.len() >= REPLAY_BYTES {
                                r.pop_front();
                            }
                            r.push_back(b);
                        }
                    }
                    // Broadcast to all live WebSockets.
                    let _ = tx.send(chunk);
                }
                Err(_) => break,
            }
        }
        // PTY process exited — remove session so the next connect spawns fresh.
        sessions_ref.lock().unwrap().remove(&sandbox_key);
        tracing::info!("PTY session '{sandbox_key}' ended");
    });

    Ok(session)
}

async fn bridge(
    socket: WebSocket,
    sandbox: String,
    mode: String,
    shell: String,
    sessions: Sessions,
) -> Result<()> {
    let (mut ws_tx, mut ws_rx) = socket.split();

    // Get or create the session on a blocking thread (PTY setup does syscalls).
    let session = tokio::task::spawn_blocking({
        let sandbox = sandbox.clone();
        let mode = mode.clone();
        let shell = shell.clone();
        let sessions = sessions.clone();
        move || get_or_create_session(&sandbox, &mode, &shell, &sessions)
    })
    .await??;

    // Subscribe BEFORE reading the replay buffer so we don't miss output
    // produced in the window between snapshot and subscription.
    let mut rx = session.tx.subscribe();
    let mut bell_rx = session.bell_tx.subscribe();

    // Send replay buffer → the client sees the terminal history.
    // Clone out of the lock before awaiting (MutexGuard is not Send).
    let replay_snapshot: Vec<u8> = {
        let r = session.replay.lock().unwrap();
        r.iter().cloned().collect()
    };
    if !replay_snapshot.is_empty() {
        ws_tx.send(Message::Binary(replay_snapshot)).await.ok();
    }

    // Forward live PTY output, and "attention" events (BEL → the agent is
    // waiting on the user), to this WebSocket.
    let sandbox_for_pump = sandbox.clone();
    let pump = tokio::spawn(async move {
        loop {
            tokio::select! {
                res = rx.recv() => {
                    match res {
                        Ok(chunk) => {
                            if ws_tx.send(Message::Binary(chunk)).await.is_err() {
                                break;
                            }
                        }
                        Err(broadcast::error::RecvError::Lagged(n)) => {
                            tracing::warn!("WebSocket lagged, dropped {n} PTY frames");
                        }
                        Err(broadcast::error::RecvError::Closed) => break,
                    }
                }
                res = bell_rx.recv() => {
                    match res {
                        Ok(()) => {
                            let msg = serde_json::json!({
                                "type": "attention",
                                "sandbox": sandbox_for_pump,
                            }).to_string();
                            if ws_tx.send(Message::Text(msg)).await.is_err() {
                                break;
                            }
                        }
                        Err(broadcast::error::RecvError::Lagged(_)) => {}
                        Err(broadcast::error::RecvError::Closed) => break,
                    }
                }
            }
        }
    });

    // Forward WebSocket input → PTY, and handle resize messages.
    let writer = session.writer.clone();
    let master = session.master.clone();

    while let Some(Ok(msg)) = ws_rx.next().await {
        match msg {
            Message::Binary(data) => {
                let w = writer.clone();
                let _ = tokio::task::spawn_blocking(move || {
                    if let Ok(mut w) = w.lock() {
                        let _ = w.write_all(&data);
                        let _ = w.flush();
                    }
                })
                .await;
            }
            Message::Text(txt) => {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&txt) {
                    if v.get("type").and_then(|t| t.as_str()) == Some("resize") {
                        let cols = v.get("cols").and_then(|c| c.as_u64()).unwrap_or(100) as u16;
                        let rows = v.get("rows").and_then(|r| r.as_u64()).unwrap_or(30) as u16;
                        let m = master.clone();
                        let _ = tokio::task::spawn_blocking(move || {
                            if let Ok(m) = m.lock() {
                                let _ = m.resize(PtySize {
                                    rows,
                                    cols,
                                    pixel_width: 0,
                                    pixel_height: 0,
                                });
                            }
                        })
                        .await;
                    }
                }
            }
            Message::Close(_) => break,
            _ => {}
        }
    }

    // WebSocket closed — do NOT kill the PTY. The session stays alive for reconnects.
    pump.abort();
    Ok(())
}
