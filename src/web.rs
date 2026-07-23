//! Browser terminal with sandbox switcher sidebar.
//!
//! PTY sessions are persistent — they survive WebSocket disconnects.
//! Refreshing the browser tab replays the last 256 KB of output and
//! resumes the live stream without restarting the agent.
//!
//! Routes:
//!   GET  /                          → HTML (initial_sandbox embedded)
//!   GET  /api/events                → SSE stream of rich session updates
//!   GET  /api/sessions              → snapshot of current session info
//!   POST /api/input                 → write raw bytes into a session's PTY
//!   POST /api/answer                → answer a session's numbered prompt
//!   POST /api/hook                  → ingest a Claude Code hook event (session state)
//!   GET  /api/hook/log              → recent hook events (inspection)
//!   GET/POST /api/usage             → subscription usage % (5h / weekly)
//!   GET  /api/sandboxes             → JSON list from `sbx ls`
//!   POST /api/sandboxes/create      → create a new sandbox
//!   POST /api/sandboxes/:name/duplicate → create a new sandbox on the same workspace
//!   POST /api/sandboxes/:name/stop  → `sbx stop <name>`
//!   GET  /api/fs?path=<dir>         → directory listing for the folder picker
//!   POST /api/fs/pick               → OS-native folder picker (Finder/Explorer/zenity)
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
    response::{
        sse::{Event as SseEvent, KeepAlive, Sse},
        Html, IntoResponse, Response,
    },
    routing::{get, post},
    Json, Router,
};
use futures_util::{SinkExt, Stream, StreamExt};
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, VecDeque},
    io::Write,
    sync::{Arc, Mutex},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::sync::broadcast;
use tokio_stream::wrappers::BroadcastStream;

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

/// Coarse lifecycle state of a session, surfaced to the macOS notch companion
/// (and any other consumer) over the `/api/events` stream.
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Default, Debug)]
#[serde(rename_all = "lowercase")]
enum SessionState {
    /// The agent is running a turn (thinking or using tools).
    #[default]
    Working,
    /// The agent finished its turn and is waiting for the next prompt.
    Idle,
    /// The agent needs the user: a question, a permission prompt, or an idle
    /// notification.
    Attention,
    /// The Claude Code session ended; the session is gone.
    Exited,
}

/// A prompt the agent is waiting on. Built from the trusted, structured
/// `AskUserQuestion` tool input carried by a `PreToolUse` hook event — no
/// terminal scraping.
#[derive(Clone, PartialEq, Serialize)]
struct Question {
    /// The question line (e.g. "Which deployment target?").
    text: String,
    /// The choices, in order (option 1 first).
    options: Vec<String>,
    /// One "label — description" line per option: the decision table Claude
    /// offers to help the user choose.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    context: Vec<String>,
}

/// Full, rich description of a session — the payload for both `/api/events`
/// (one per change) and the `/api/sessions` snapshot.
#[derive(Clone, Serialize)]
struct SessionInfo {
    sandbox: String,
    /// "claude" (agent) or "bash" (shell).
    mode: String,
    state: SessionState,
    /// Agent running in the sandbox (from `sbx ls`), e.g. "claude".
    agent: String,
    /// Unix epoch ms when this session's PTY was created.
    started_ms: u64,
    /// Current activity — the tool the agent is running or a notification
    /// message, from the latest hook event.
    #[serde(skip_serializing_if = "Option::is_none")]
    activity: Option<String>,
    /// The user's last submitted prompt (from `UserPromptSubmit`).
    #[serde(skip_serializing_if = "Option::is_none")]
    last_input: Option<String>,
    /// Pending question, when `state == attention` and the agent invoked the
    /// `AskUserQuestion` tool.
    #[serde(skip_serializing_if = "Option::is_none")]
    question: Option<Question>,
    /// Unix epoch ms of this event.
    ts: u64,
}

/// Current, mutable state of a session, driven entirely by Claude Code hook
/// events (see `apply_hook`). Keyed by "<sandbox>::claude".
#[derive(Default)]
struct SessionStatus {
    state: SessionState,
    agent: String,
    started_ms: u64,
    activity: Option<String>,
    last_input: Option<String>,
    question: Option<Question>,
}

type Statuses = Arc<Mutex<HashMap<String, SessionStatus>>>;

/// Unix epoch milliseconds, for event timestamps.
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// Split a session key "<sandbox>::<mode>" back into its parts.
fn split_key(key: &str) -> (&str, &str) {
    key.split_once("::").unwrap_or((key, ""))
}

/// Build the rich payload for a session from its current status.
fn build_info(key: &str, st: &SessionStatus) -> SessionInfo {
    let (sandbox, mode) = split_key(key);
    SessionInfo {
        sandbox: sandbox.to_string(),
        mode: mode.to_string(),
        state: st.state,
        agent: st.agent.clone(),
        started_ms: st.started_ms,
        activity: st.activity.clone(),
        last_input: st.last_input.clone(),
        question: st.question.clone(),
        ts: now_ms(),
    }
}

/// Broadcast the current state of `key`, if it still exists.
fn emit_info(events: &broadcast::Sender<SessionInfo>, statuses: &Statuses, key: &str) {
    let info = statuses
        .lock()
        .unwrap()
        .get(key)
        .map(|st| build_info(key, st));
    if let Some(info) = info {
        let _ = events.send(info);
    }
}

/// Trim a string to one line and at most `max` chars, for compact display.
fn clip(s: &str, max: usize) -> String {
    let one_line = s.split('\n').next().unwrap_or(s).trim();
    if one_line.chars().count() <= max {
        one_line.to_string()
    } else {
        let mut out: String = one_line.chars().take(max).collect();
        out.push('…');
        out
    }
}

/// Build the interactive prompt from an `AskUserQuestion` tool input, which
/// carries the questions and their options as structured JSON — so no parsing
/// of the terminal is needed. Uses the first question (the island shows one
/// card); its options become the choices and their descriptions the decision
/// table shown above them.
fn question_from_ask(body: &serde_json::Value) -> Option<Question> {
    let questions = body.get("tool_input")?.get("questions")?.as_array()?;
    let q = questions.first()?;
    let text = q
        .get("question")
        .and_then(|v| v.as_str())?
        .trim()
        .to_string();
    let opts = q.get("options")?.as_array()?;
    let mut options = Vec::new();
    let mut context = Vec::new();
    for o in opts {
        let label = o
            .get("label")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        if label.is_empty() {
            continue;
        }
        if let Some(desc) = o.get("description").and_then(|v| v.as_str()) {
            let desc = desc.trim();
            if !desc.is_empty() {
                context.push(format!("{label} — {desc}"));
            }
        }
        options.push(label);
    }
    if options.len() < 2 {
        return None;
    }
    Some(Question {
        text,
        options,
        context,
    })
}

/// A short human label for the tool a `PreToolUse`/`PostToolUse` event is
/// about, used as the session's "current activity".
fn describe_tool(tool: &str, input: Option<&serde_json::Value>) -> String {
    let field = |name: &str| {
        input
            .and_then(|i| i.get(name))
            .and_then(|v| v.as_str())
            .unwrap_or("")
    };
    let basename = |p: &str| p.rsplit('/').next().unwrap_or(p).to_string();
    match tool {
        "Edit" | "Write" | "Read" | "NotebookEdit" => {
            let path = field("file_path");
            if path.is_empty() {
                tool.to_string()
            } else {
                format!("{tool} {}", basename(path))
            }
        }
        "Bash" => {
            let cmd = field("command");
            if cmd.is_empty() {
                "Bash".to_string()
            } else {
                format!("$ {}", clip(cmd, 60))
            }
        }
        "Grep" | "Glob" => {
            let pat = field("pattern");
            if pat.is_empty() {
                tool.to_string()
            } else {
                format!("{tool} {}", clip(pat, 40))
            }
        }
        "" => "Working".to_string(),
        other => other.to_string(),
    }
}

/// Fold a Claude Code hook event into a session's status. Returns `true` when
/// the session ended and its entry should be dropped. This is the single source
/// of truth for session state — replacing the old terminal-scraping heuristics.
fn apply_hook(event: &str, tool: &str, body: &serde_json::Value, st: &mut SessionStatus) -> bool {
    match event {
        "SessionStart" => {
            st.state = SessionState::Idle;
            st.started_ms = now_ms();
            st.question = None;
            st.activity = None;
        }
        "UserPromptSubmit" => {
            st.state = SessionState::Working;
            st.question = None;
            st.activity = None;
            if let Some(p) = body.get("prompt").and_then(|v| v.as_str()) {
                st.last_input = Some(clip(p, 200));
            }
        }
        "PreToolUse" => {
            if tool == "AskUserQuestion" {
                if let Some(q) = question_from_ask(body) {
                    st.activity = Some(clip(&q.text, 80));
                    st.question = Some(q);
                    st.state = SessionState::Attention;
                } else {
                    st.state = SessionState::Working;
                }
            } else {
                st.state = SessionState::Working;
                st.question = None;
                st.activity = Some(describe_tool(tool, body.get("tool_input")));
            }
        }
        "PostToolUse" => {
            if tool == "AskUserQuestion" {
                st.question = None;
            }
            st.state = SessionState::Working;
        }
        "Notification" => {
            // Claude needs the user: a permission prompt or an idle nudge. There
            // is no structured question here, so the island shows the message and
            // a "go to browser" affordance.
            st.state = SessionState::Attention;
            if let Some(m) = body.get("message").and_then(|v| v.as_str()) {
                st.activity = Some(clip(m, 120));
            }
        }
        "Stop" => {
            st.state = SessionState::Idle;
            st.question = None;
            st.activity = None;
        }
        "SessionEnd" => {
            st.state = SessionState::Exited;
            st.question = None;
            return true;
        }
        _ => {}
    }
    false
}

/// Ring buffer of the most recent Claude Code hook events received at
/// `POST /api/hook` — for the trusted-events POC (inspect via `/api/hook/log`).
type HookLog = Arc<Mutex<VecDeque<serde_json::Value>>>;

/// Account-wide Claude subscription usage, reported by the in-sandbox
/// `statusLine` script (`POST /api/usage`). The percentages are the same
/// numbers Claude Code's `/usage` shows; since every sandbox shares one account
/// this is a single latest-wins value, not a per-session sum.
#[derive(Clone, Serialize, Default)]
struct UsageInfo {
    /// 5-hour rolling window utilization, 0–100 (absent until the first API
    /// response of a session, and only on Pro/Max — not API-key auth).
    #[serde(skip_serializing_if = "Option::is_none")]
    five_hour_pct: Option<f64>,
    /// 7-day (weekly) window utilization, 0–100.
    #[serde(skip_serializing_if = "Option::is_none")]
    seven_day_pct: Option<f64>,
    /// Unix epoch seconds when the 5-hour window resets.
    #[serde(skip_serializing_if = "Option::is_none")]
    five_hour_resets_at: Option<i64>,
    /// Unix epoch seconds when the weekly window resets.
    #[serde(skip_serializing_if = "Option::is_none")]
    seven_day_resets_at: Option<i64>,
    /// Unix epoch ms when this was last refreshed.
    updated_ms: u64,
}

#[derive(Clone)]
struct AppState {
    initial_sandbox: String,
    shell: String,
    sessions: Sessions,
    /// Broadcast bus of rich session updates (see `/api/events`).
    events: broadcast::Sender<SessionInfo>,
    /// Broadcast bus of "focus this sandbox" requests from the macOS island,
    /// delivered to open web tabs over `/api/focus-events` so a click reuses an
    /// existing tab instead of spawning a new one.
    focus: broadcast::Sender<String>,
    /// Current state per session, for the `/api/sessions` snapshot.
    statuses: Statuses,
    /// Recent hook events (POC).
    hook_log: HookLog,
    /// Latest subscription usage (see `/api/usage`).
    usage: Arc<Mutex<UsageInfo>>,
    cfg: Arc<Config>,
    use_api_key: bool,
}

#[derive(Serialize)]
struct SandboxItem {
    name: String,
    agent: String,
    status: String,
    /// Host workspace directory, if known (see `workspace_for`). Lets the
    /// frontend visually group sandboxes that share the same workspace.
    #[serde(skip_serializing_if = "Option::is_none")]
    workspace: Option<String>,
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
    // Session state is driven by Claude Code hook events (see `api_hook`), so
    // there is no output-timing scanner: `Idle` comes from a `Stop` event, not
    // a quiet timer.
    let (events, _) = broadcast::channel::<SessionInfo>(256);
    let (focus, _) = broadcast::channel::<String>(16);
    let statuses: Statuses = Arc::new(Mutex::new(HashMap::new()));

    // Reconcile against reality: a sandbox stopped or removed out-of-band never
    // sends a `SessionEnd` hook, so its status would linger. Every 15 s, drop
    // any status whose sandbox `sbx ls` no longer reports as running — emitting
    // an `Exited` first so subscribers remove it immediately. `sbx ls` is the
    // authority (same source the UI polls); an empty result is treated as a
    // transient failure and skipped, never as "everything is gone".
    {
        let events = events.clone();
        let statuses = statuses.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(15));
            loop {
                tick.tick().await;
                let running: std::collections::HashSet<String> =
                    tokio::task::spawn_blocking(sbx::list_sandboxes)
                        .await
                        .unwrap_or_default()
                        .into_iter()
                        .filter(|s| s.status.to_lowercase().contains("running"))
                        .map(|s| s.name)
                        .collect();
                if running.is_empty() {
                    continue;
                }
                let gone: Vec<String> = {
                    let map = statuses.lock().unwrap();
                    map.keys()
                        .filter(|k| !running.contains(split_key(k).0))
                        .cloned()
                        .collect()
                };
                for key in gone {
                    if let Some(st) = statuses.lock().unwrap().get_mut(&key) {
                        st.state = SessionState::Exited;
                    }
                    emit_info(&events, &statuses, &key);
                    statuses.lock().unwrap().remove(&key);
                    tracing::info!("reconcile: dropped stale session '{key}' (sandbox gone)");
                }
            }
        });
    }

    let state = Arc::new(AppState {
        initial_sandbox,
        shell,
        sessions: Arc::new(Mutex::new(HashMap::new())),
        events,
        focus,
        statuses,
        hook_log: Arc::new(Mutex::new(VecDeque::new())),
        usage: Arc::new(Mutex::new(UsageInfo::default())),
        cfg,
        use_api_key,
    });

    let app = Router::new()
        .route("/", get(index_handler))
        .route("/ws", get(ws_handler))
        .route("/api/events", get(api_events))
        .route("/api/focus", post(api_focus))
        .route("/api/focus-events", get(api_focus_events))
        .route("/api/sessions", get(api_sessions))
        .route("/api/input", post(api_input))
        .route("/api/answer", post(api_answer))
        .route("/api/hook", post(api_hook))
        .route("/api/hook/log", get(api_hook_log))
        .route("/api/usage", get(api_usage_get).post(api_usage_post))
        .route("/api/sandboxes", get(api_list))
        .route("/api/sandboxes/create", post(api_create))
        .route("/api/sandboxes/:name/duplicate", post(api_duplicate))
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
        .route("/api/fs/pick", post(api_fs_pick))
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

/// Live stream of session state transitions (Server-Sent Events). The macOS
/// notch companion subscribes here to keep its "island" in sync; each event's
/// `data:` payload is a JSON `SessionInfo`.
async fn api_events(
    State(state): State<Arc<AppState>>,
) -> Sse<impl Stream<Item = Result<SseEvent, std::convert::Infallible>>> {
    let stream = BroadcastStream::new(state.events.subscribe()).filter_map(|res| async move {
        // `Err` here is a lagged receiver — skip the gap rather than closing.
        let ev = res.ok()?;
        Some(Ok(SseEvent::default()
            .json_data(&ev)
            .unwrap_or_else(|_| SseEvent::default())))
    });
    Sse::new(stream).keep_alive(KeepAlive::default())
}

#[derive(Deserialize)]
struct FocusReq {
    sandbox: String,
}

/// The macOS island (or any client) asks every open web UI to switch to a
/// sandbox. We broadcast the name to all `/api/focus-events` subscribers and
/// report how many received it: the caller uses `clients > 0` to decide whether
/// a browser tab is already open (reuse it) or one must be cold-started with a
/// `#sandbox=` deep link. This is what makes an island click focus the existing
/// tab instead of spawning a new page.
async fn api_focus(
    State(state): State<Arc<AppState>>,
    Json(req): Json<FocusReq>,
) -> Json<serde_json::Value> {
    let clients = state.focus.receiver_count();
    // `send` errors only when there are no subscribers; nothing to do then.
    let _ = state.focus.send(req.sandbox);
    Json(serde_json::json!({ "clients": clients }))
}

/// Live stream of "focus this sandbox" requests (see `api_focus`). The web UI
/// subscribes here and switches its focused pane to the named sandbox; each
/// event's `data:` payload is the bare sandbox name.
async fn api_focus_events(
    State(state): State<Arc<AppState>>,
) -> Sse<impl Stream<Item = Result<SseEvent, std::convert::Infallible>>> {
    let stream = BroadcastStream::new(state.focus.subscribe()).filter_map(|res| async move {
        // `Err` here is a lagged receiver — skip the gap rather than closing.
        let name = res.ok()?;
        Some(Ok(SseEvent::default().data(name)))
    });
    Sse::new(stream).keep_alive(KeepAlive::default())
}

/// Current state of every live session, so a consumer that connects mid-stream
/// (e.g. the notch app on launch) sees what already exists without waiting for
/// the next transition.
async fn api_sessions(State(state): State<Arc<AppState>>) -> Json<Vec<SessionInfo>> {
    let mut out: Vec<SessionInfo> = {
        let map = state.statuses.lock().unwrap();
        map.iter().map(|(key, st)| build_info(key, st)).collect()
    };
    out.sort_by(|a, b| a.sandbox.cmp(&b.sandbox).then(a.mode.cmp(&b.mode)));
    Json(out)
}

const HOOK_LOG_MAX: usize = 100;

/// Receive a Claude Code hook event — the trusted, structured source of session
/// state. The in-sandbox hook script (see `assets/status-hook.js`) POSTs each
/// lifecycle event here, adding the `sandbox` field. We fold it into the
/// session's status (`apply_hook`), broadcast the update on `/api/events`, and
/// keep a copy in the ring buffer for inspection via `/api/hook/log`.
async fn api_hook(
    State(state): State<Arc<AppState>>,
    Json(mut body): Json<serde_json::Value>,
) -> StatusCode {
    let event = body
        .get("hook_event_name")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let sandbox = body
        .get("sandbox")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let tool = body
        .get("tool_name")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    tracing::info!("hook: sandbox={sandbox} event={event} tool={tool}");

    // Stamp receipt time and store in the ring buffer for inspection.
    if let Some(obj) = body.as_object_mut() {
        obj.insert("received_ms".into(), serde_json::json!(now_ms()));
    }
    {
        let mut log = state.hook_log.lock().unwrap();
        if log.len() >= HOOK_LOG_MAX {
            log.pop_front();
        }
        log.push_back(body.clone());
    }

    // Hooks only fire for the agent session; nothing to do without a sandbox.
    if sandbox.is_empty() || event.is_empty() {
        return StatusCode::OK;
    }
    let key = format!("{sandbox}::claude");
    let remove = {
        let mut map = state.statuses.lock().unwrap();
        let st = map.entry(key.clone()).or_default();
        apply_hook(&event, &tool, &body, st)
    };
    emit_info(&state.events, &state.statuses, &key);
    if remove {
        state.statuses.lock().unwrap().remove(&key);
    }
    StatusCode::OK
}

/// Inspect recently received hook events:
/// `curl -s http://sbxw.localhost:7681/api/hook/log | jq`.
async fn api_hook_log(State(state): State<Arc<AppState>>) -> Json<Vec<serde_json::Value>> {
    Json(state.hook_log.lock().unwrap().iter().cloned().collect())
}

#[derive(Deserialize)]
struct UsageBody {
    five_hour_pct: Option<f64>,
    seven_day_pct: Option<f64>,
    five_hour_resets_at: Option<i64>,
    seven_day_resets_at: Option<i64>,
}

/// Receive a subscription-usage report from an in-sandbox `statusLine` script
/// (see `assets/usage-statusline.js`). Latest-wins: the percentages are
/// account-wide, so any sandbox's report refreshes the shared value.
async fn api_usage_post(
    State(state): State<Arc<AppState>>,
    Json(body): Json<UsageBody>,
) -> StatusCode {
    let mut u = state.usage.lock().unwrap();
    // Only overwrite a percentage when the report actually carries one, so a
    // session that hasn't seen its first API response yet (no `rate_limits`)
    // doesn't wipe a good value from another session.
    if body.five_hour_pct.is_some() {
        u.five_hour_pct = body.five_hour_pct;
        u.five_hour_resets_at = body.five_hour_resets_at;
    }
    if body.seven_day_pct.is_some() {
        u.seven_day_pct = body.seven_day_pct;
        u.seven_day_resets_at = body.seven_day_resets_at;
    }
    u.updated_ms = now_ms();
    StatusCode::OK
}

/// Latest account-wide subscription usage (the `/usage` percentages), for the
/// island. `null`-ish fields when unknown (e.g. API-key auth, or before the
/// first API response). Poll: `curl -s http://sbxw.localhost:7681/api/usage`.
async fn api_usage_get(State(state): State<Arc<AppState>>) -> Json<UsageInfo> {
    Json(state.usage.lock().unwrap().clone())
}

#[derive(Deserialize)]
struct InputBody {
    sandbox: String,
    mode: Option<String>,
    /// Raw bytes to write to the session's PTY (e.g. "y\r").
    data: String,
}

/// Write raw input into a live session's PTY. Used by the notch companion to
/// answer prompts without opening the browser. Local-only (the daemon binds
/// loopback).
async fn api_input(State(state): State<Arc<AppState>>, Json(body): Json<InputBody>) -> StatusCode {
    let mode = body.mode.as_deref().unwrap_or("claude");
    let key = format!("{}::{}", body.sandbox, mode);
    let session = state.sessions.lock().unwrap().get(&key).cloned();
    match session {
        Some(sess) => {
            let mut w = sess.writer.lock().unwrap();
            let _ = w.write_all(body.data.as_bytes());
            let _ = w.flush();
            StatusCode::OK
        }
        None => StatusCode::NOT_FOUND,
    }
}

#[derive(Deserialize)]
struct AnswerBody {
    sandbox: String,
    mode: Option<String>,
    /// 1-based option number to select in the pending numbered menu.
    index: u32,
}

/// Answer a session's pending numbered menu by writing "<index>\r" to its PTY,
/// then clear the stored question so the island's prompt card dismisses.
async fn api_answer(
    State(state): State<Arc<AppState>>,
    Json(body): Json<AnswerBody>,
) -> StatusCode {
    let mode = body.mode.as_deref().unwrap_or("claude");
    let key = format!("{}::{}", body.sandbox, mode);
    let session = state.sessions.lock().unwrap().get(&key).cloned();
    let Some(sess) = session else {
        return StatusCode::NOT_FOUND;
    };
    {
        let mut w = sess.writer.lock().unwrap();
        let _ = w.write_all(format!("{}\r", body.index).as_bytes());
        let _ = w.flush();
    }
    // Optimistically clear the prompt and mark the session working again.
    if let Some(st) = state.statuses.lock().unwrap().get_mut(&key) {
        st.question = None;
        st.state = SessionState::Working;
    }
    emit_info(&state.events, &state.statuses, &key);
    StatusCode::OK
}

async fn api_list() -> Json<Vec<SandboxItem>> {
    let items = tokio::task::spawn_blocking(sbx::list_sandboxes)
        .await
        .unwrap_or_default();
    Json(
        items
            .into_iter()
            .map(|s| {
                let workspace =
                    crate::workspace_for(&s.name).map(|p| p.to_string_lossy().into_owned());
                SandboxItem {
                    name: s.name,
                    agent: s.agent,
                    status: s.status,
                    workspace,
                }
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

#[derive(Deserialize)]
struct DuplicateBody {
    new_name: String,
}

/// `POST /api/sandboxes/:name/duplicate` — provision a brand-new sandbox
/// pointed at the *same* host workspace directory as `name` (bind-mounted,
/// so both sandboxes see each other's file changes live). Just a name
/// wizard on the frontend: no path/port picking, since everything else is
/// inherited from the source sandbox's workspace + sbxw.toml.
async fn api_duplicate(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
    Json(body): Json<DuplicateBody>,
) -> Json<serde_json::Value> {
    let new_name = body.new_name.trim().to_string();
    if new_name.is_empty()
        || !new_name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-')
    {
        return Json(serde_json::json!({
            "ok": false,
            "error": "name must be non-empty and contain only letters, digits, and hyphens"
        }));
    }

    let Some(workspace) = crate::workspace_for(&name) else {
        return Json(serde_json::json!({
            "ok": false,
            "error": format!("no known workspace for '{name}' — it may predate this sbxw version")
        }));
    };
    let workspace = workspace.to_string_lossy().into_owned();

    match sbx::exists(&new_name) {
        Ok(true) => {
            return Json(serde_json::json!({
                "ok": false,
                "error": format!("a sandbox named '{new_name}' already exists")
            }))
        }
        Err(e) => return Json(serde_json::json!({ "ok": false, "error": e.to_string() })),
        Ok(false) => {}
    }

    let cfg = state.cfg.clone();
    let use_api_key = state.use_api_key;
    tracing::info!("web UI: duplicating sandbox '{name}' as '{new_name}' (workspace {workspace})");
    match tokio::task::spawn_blocking(move || {
        crate::provision_sandbox(&new_name, &workspace, &[], &cfg, &[], use_api_key)
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

/// `POST /api/fs/pick` — pops the OS-native folder picker (Finder on macOS,
/// the Explorer folder browser on Windows, zenity/kdialog on Linux) and
/// returns the chosen absolute path. Runs on a blocking thread since the
/// dialog blocks until the user responds.
async fn api_fs_pick() -> Json<serde_json::Value> {
    match tokio::task::spawn_blocking(pick_folder_native).await {
        Ok(Ok(Some(path))) => Json(serde_json::json!({ "ok": true, "path": path })),
        Ok(Ok(None)) => Json(serde_json::json!({ "ok": false, "cancelled": true })),
        Ok(Err(e)) => Json(serde_json::json!({ "ok": false, "error": e.to_string() })),
        Err(_) => Json(serde_json::json!({ "ok": false, "error": "task panic" })),
    }
}

/// Blocks the calling thread until the user picks a folder or dismisses the
/// dialog. Returns `Ok(None)` on cancel, `Err` if no native picker is
/// available on this platform.
fn pick_folder_native() -> Result<Option<String>> {
    use std::process::Command;

    #[cfg(target_os = "macos")]
    {
        let out = Command::new("osascript")
            .arg("-e")
            .arg(r#"POSIX path of (choose folder with prompt "Select workspace folder")"#)
            .output()
            .context("failed to launch Finder folder picker")?;
        if !out.status.success() {
            // AppleScript exits non-zero when the user clicks Cancel.
            return Ok(None);
        }
        let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
        return Ok(if path.is_empty() { None } else { Some(path) });
    }

    #[cfg(target_os = "windows")]
    {
        const SCRIPT: &str = r#"
Add-Type -AssemblyName System.Windows.Forms
$dialog = New-Object System.Windows.Forms.FolderBrowserDialog
$dialog.Description = "Select workspace folder"
if ($dialog.ShowDialog() -eq [System.Windows.Forms.DialogResult]::OK) {
    Write-Output $dialog.SelectedPath
}
"#;
        let out = Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command", SCRIPT])
            .output()
            .context("failed to launch Explorer folder picker")?;
        let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
        return Ok(if path.is_empty() { None } else { Some(path) });
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        if let Ok(out) = Command::new("zenity")
            .args([
                "--file-selection",
                "--directory",
                "--title=Select workspace folder",
            ])
            .output()
        {
            let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
            // zenity exits non-zero on cancel; treat that as "no selection"
            // rather than "picker unavailable".
            return Ok(if out.status.success() && !path.is_empty() {
                Some(path)
            } else {
                None
            });
        }
        if let Ok(out) = Command::new("kdialog")
            .arg("--getexistingdirectory")
            .arg(std::env::var("HOME").unwrap_or_else(|_| "/".into()))
            .output()
        {
            let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
            return Ok(if out.status.success() && !path.is_empty() {
                Some(path)
            } else {
                None
            });
        }
        anyhow::bail!("no native folder picker found — install zenity or kdialog")
    }
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
        // Since sbx 0.35 this also works for sandboxes created with a custom
        // --kit (like sbxw's OAuth kit) without re-passing the kit.
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

    // Background reader thread — pure terminal I/O: PTY output → replay buffer →
    // WebSocket broadcast, plus a debounced BEL signal for the browser's
    // "attention" toast. Session *state* is derived from Claude Code hooks
    // (`api_hook`), not from this stream, so there is no scraping here.
    let sessions_ref = sessions.clone();
    let sandbox_key = session_key.clone();
    std::thread::spawn(move || {
        let mut buf = [0u8; 8192];
        // Debounces the bell signal — agents can emit several BELs in a row
        // for one prompt, and we only want one notification out of that burst.
        let mut last_bell = Instant::now() - Duration::from_secs(3);
        loop {
            match std::io::Read::read(&mut reader, &mut buf) {
                Ok(0) => break, // PTY closed (process exited)
                Ok(n) => {
                    let chunk = buf[..n].to_vec();
                    let has_bell = chunk.contains(&0x07);
                    if has_bell && last_bell.elapsed() >= Duration::from_secs(3) {
                        last_bell = Instant::now();
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
        // PTY process exited — drop the session so the next connect spawns fresh.
        // Session state (Exited) is announced by the SessionEnd hook.
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
                // Raw keystrokes go straight to the PTY. The user's prompt text
                // is captured from the trusted `UserPromptSubmit` hook, not by
                // reconstructing it from keystrokes here.
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ask_body(question: &str, opts: &[(&str, &str)]) -> serde_json::Value {
        let options: Vec<_> = opts
            .iter()
            .map(|(l, d)| json!({ "label": l, "description": d }))
            .collect();
        json!({
            "hook_event_name": "PreToolUse",
            "tool_name": "AskUserQuestion",
            "tool_input": { "questions": [ { "question": question, "options": options } ] }
        })
    }

    #[test]
    fn question_from_ask_builds_options_and_decision_table() {
        let body = ask_body(
            "Which deployment target?",
            &[("Production", "The live site"), ("Staging", "The test env")],
        );
        let q = question_from_ask(&body).expect("a question");
        assert_eq!(q.text, "Which deployment target?");
        assert_eq!(q.options, vec!["Production", "Staging"]);
        assert_eq!(
            q.context,
            vec![
                "Production — The live site".to_string(),
                "Staging — The test env".to_string()
            ]
        );
    }

    #[test]
    fn question_from_ask_rejects_single_option() {
        let body = ask_body("Only one?", &[("Yes", "")]);
        assert!(question_from_ask(&body).is_none());
    }

    #[test]
    fn describe_tool_summarizes_common_tools() {
        assert_eq!(
            describe_tool("Edit", Some(&json!({ "file_path": "/a/b/main.rs" }))),
            "Edit main.rs"
        );
        assert_eq!(
            describe_tool("Bash", Some(&json!({ "command": "cargo build" }))),
            "$ cargo build"
        );
        assert_eq!(describe_tool("", None), "Working");
    }

    #[test]
    fn apply_hook_prompt_submit_sets_working_and_input() {
        let mut st = SessionStatus::default();
        let body = json!({ "hook_event_name": "UserPromptSubmit", "prompt": "refais en un" });
        let remove = apply_hook("UserPromptSubmit", "", &body, &mut st);
        assert!(!remove);
        assert_eq!(st.state, SessionState::Working);
        assert_eq!(st.last_input.as_deref(), Some("refais en un"));
        assert!(st.question.is_none());
    }

    #[test]
    fn apply_hook_ask_question_enters_attention_with_question() {
        let mut st = SessionStatus::default();
        let body = ask_body("Pick one?", &[("A", "first"), ("B", "second")]);
        apply_hook("PreToolUse", "AskUserQuestion", &body, &mut st);
        assert_eq!(st.state, SessionState::Attention);
        let q = st.question.expect("a question");
        assert_eq!(q.text, "Pick one?");
        assert_eq!(q.options, vec!["A", "B"]);
    }

    #[test]
    fn apply_hook_other_tool_is_working_and_clears_question() {
        let mut st = SessionStatus {
            state: SessionState::Attention,
            question: Some(Question {
                text: "old?".into(),
                options: vec!["a".into(), "b".into()],
                context: vec![],
            }),
            ..Default::default()
        };
        let body = json!({
            "hook_event_name": "PreToolUse",
            "tool_name": "Edit",
            "tool_input": { "file_path": "/x/y.rs" }
        });
        apply_hook("PreToolUse", "Edit", &body, &mut st);
        assert_eq!(st.state, SessionState::Working);
        assert!(st.question.is_none());
        assert_eq!(st.activity.as_deref(), Some("Edit y.rs"));
    }

    #[test]
    fn apply_hook_notification_enters_attention() {
        let mut st = SessionStatus::default();
        let body = json!({
            "hook_event_name": "Notification",
            "message": "Claude needs your permission to use Bash"
        });
        apply_hook("Notification", "", &body, &mut st);
        assert_eq!(st.state, SessionState::Attention);
        assert_eq!(
            st.activity.as_deref(),
            Some("Claude needs your permission to use Bash")
        );
    }

    #[test]
    fn apply_hook_stop_goes_idle_and_clears_question() {
        let mut st = SessionStatus {
            state: SessionState::Attention,
            question: Some(Question {
                text: "q?".into(),
                options: vec!["a".into(), "b".into()],
                context: vec![],
            }),
            ..Default::default()
        };
        apply_hook("Stop", "", &json!({ "hook_event_name": "Stop" }), &mut st);
        assert_eq!(st.state, SessionState::Idle);
        assert!(st.question.is_none());
    }

    #[test]
    fn apply_hook_session_end_marks_exited_and_removes() {
        let mut st = SessionStatus::default();
        let remove = apply_hook(
            "SessionEnd",
            "",
            &json!({ "hook_event_name": "SessionEnd" }),
            &mut st,
        );
        assert!(remove);
        assert_eq!(st.state, SessionState::Exited);
    }

    #[test]
    fn clip_truncates_and_single_lines() {
        assert_eq!(clip("hello", 10), "hello");
        assert_eq!(clip("first line\nsecond", 40), "first line");
        assert_eq!(clip("abcdefghij", 5), "abcde…");
    }
}
