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
    /// First step of the pending prompt. Redundant with `steps[0]`, kept on the
    /// wire so an island built before multi-step support still shows the card it
    /// always showed instead of going blank.
    #[serde(skip_serializing_if = "Option::is_none")]
    question: Option<Question>,
    /// Every step of the pending prompt, in tab order. `AskUserQuestion` can
    /// carry several questions in one call — the terminal walks them as tabs
    /// and submits once at the end, and so does the island.
    #[serde(skip_serializing_if = "Option::is_none")]
    steps: Option<Vec<Question>>,
    /// Claude's last reply (multi-line, clipped). Absent until a turn ends, and
    /// cleared the moment the next prompt is submitted.
    #[serde(skip_serializing_if = "Option::is_none")]
    reply: Option<String>,
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
    /// Steps of the pending `AskUserQuestion` prompt; empty when none is open.
    prompt: Vec<Question>,
    /// What Claude last *said*, taken from the `last_assistant_message` Claude
    /// Code puts on its `Stop` hook event. Newlines are kept — the island shows
    /// the first sentence on the row and the first lines in its hover accordion.
    reply: Option<String>,
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
        question: st.prompt.first().cloned(),
        steps: (!st.prompt.is_empty()).then(|| st.prompt.clone()),
        reply: st.reply.clone(),
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
/// Clip prose while *keeping* its line structure — unlike `clip`, which reduces
/// everything to the first line. The island needs several lines for the
/// accordion it opens when you hover a row.
fn clip_lines(s: &str, max_lines: usize, max_chars: usize) -> String {
    let mut out = String::new();
    for line in s.trim().lines().take(max_lines) {
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(line.trim_end());
        if out.chars().count() >= max_chars {
            break;
        }
    }
    if out.chars().count() > max_chars {
        out = out.chars().take(max_chars).collect();
        out.push('…');
    }
    out
}

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
/// of the terminal is needed. Every question becomes a step, in the order the
/// terminal lays them out as tabs; each one's options become the choices and
/// their descriptions the decision table shown above them.
///
/// All-or-nothing: a step we can't represent (no text, fewer than two options)
/// would desync the island's answers from the terminal's tabs, since the two
/// are matched by position. Better to show no card at all and let the user
/// answer in the browser.
fn questions_from_ask(body: &serde_json::Value) -> Vec<Question> {
    let Some(questions) = body
        .get("tool_input")
        .and_then(|i| i.get("questions"))
        .and_then(|q| q.as_array())
    else {
        return Vec::new();
    };
    let mut steps = Vec::new();
    for q in questions {
        let Some(step) = question_step(q) else {
            return Vec::new();
        };
        steps.push(step);
    }
    steps
}

/// One question of an `AskUserQuestion` call, or `None` if it is unusable.
fn question_step(q: &serde_json::Value) -> Option<Question> {
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
            st.prompt.clear();
            st.activity = None;
            st.reply = None;
        }
        "UserPromptSubmit" => {
            st.state = SessionState::Working;
            st.prompt.clear();
            st.activity = None;
            // The previous answer is stale the moment a new turn starts; leaving
            // it up would caption "working" with what Claude said last time.
            st.reply = None;
            if let Some(p) = body.get("prompt").and_then(|v| v.as_str()) {
                st.last_input = Some(clip(p, 200));
            }
        }
        "PreToolUse" => {
            if tool == "AskUserQuestion" {
                let steps = questions_from_ask(body);
                if let Some(first) = steps.first() {
                    st.activity = Some(clip(&first.text, 80));
                    st.prompt = steps;
                    st.state = SessionState::Attention;
                } else {
                    st.state = SessionState::Working;
                }
            } else {
                st.state = SessionState::Working;
                st.prompt.clear();
                st.activity = Some(describe_tool(tool, body.get("tool_input")));
            }
        }
        "PostToolUse" => {
            if tool == "AskUserQuestion" {
                st.prompt.clear();
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
            st.prompt.clear();
            st.activity = None;
            // Claude Code hands the finished turn's prose to the `Stop` hook
            // itself, as `last_assistant_message`. Two things follow: we never
            // read the transcript (which lags — at `Stop` the turn being ended
            // is not flushed yet, so the last entry on disk is the *previous*
            // answer), and the field reaches us through any hook script, even
            // one installed in a sandbox long before this feature existed.
            if let Some(text) = body.get("last_assistant_message").and_then(|v| v.as_str()) {
                let text = clip_lines(text, 12, 700);
                if !text.is_empty() {
                    st.reply = Some(text);
                }
            }
        }
        "SessionEnd" => {
            st.state = SessionState::Exited;
            st.prompt.clear();
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
    /// Whether this is a chat sandbox (its workspace is a throwaway directory
    /// under `chat_workspace_root()`). Every chat sandbox has a *different*
    /// workspace, so workspace grouping can't catch them — the frontend groups
    /// them on this flag instead.
    chat: bool,
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
        .route("/api/sandboxes/chat", post(api_chat))
        .route("/api/chat/push", post(api_chat_push))
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
    /// 1-based option number to select in the pending numbered menu. Kept for
    /// single-step prompts (and older islands); `indices` supersedes it.
    index: Option<u32>,
    /// One 1-based option number per step of a multi-question prompt, in tab
    /// order.
    indices: Option<Vec<u32>>,
}

/// Pause between the keystrokes that answer a prompt, so the TUI has a frame to
/// redraw — move its cursor, switch tab — before the next one lands.
const ANSWER_KEY_DELAY: Duration = Duration::from_millis(60);

/// Down arrow and Return, the only keys the prompt actually listens to.
const KEY_DOWN: &[u8] = b"\x1b[B";
const KEY_ENTER: &[u8] = b"\r";

/// Guard against an answer index far past the end of a menu we have no record
/// of: at worst we walk to the bottom of the list rather than spraying arrows.
const MAX_MENU_STEPS: u32 = 20;

/// Send one keystroke into a session's PTY, then let the TUI redraw.
async fn send_key(sess: &Arc<PtySession>, key: &[u8]) {
    {
        let mut w = sess.writer.lock().unwrap();
        let _ = w.write_all(key);
        let _ = w.flush();
    }
    tokio::time::sleep(ANSWER_KEY_DELAY).await;
}

/// Answer a session's pending prompt by replaying the keystrokes a user would
/// type, then clear the stored prompt so the island's card dismisses.
///
/// The prompt is a tab per question followed by a Submit tab, and it only
/// answers to arrows and Return — *not* to the option numbers it displays. So
/// picking option k means k-1 downs (each tab opens on its first option) then
/// Return, which selects and moves to the next tab; once every question is
/// answered the form sits on Submit, which one last Return confirms.
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
    let answers: Vec<u32> = match (body.indices, body.index) {
        (Some(list), _) if !list.is_empty() => list,
        (_, Some(index)) => vec![index],
        _ => return StatusCode::BAD_REQUEST,
    };
    // How many options each step offers, so a stray index can't run away.
    let sizes: Vec<u32> = state
        .statuses
        .lock()
        .unwrap()
        .get(&key)
        .map(|st| st.prompt.iter().map(|q| q.options.len() as u32).collect())
        .unwrap_or_default();
    for (n, index) in answers.iter().enumerate() {
        let limit = sizes.get(n).copied().unwrap_or(MAX_MENU_STEPS);
        for _ in 1..(*index).clamp(1, limit.max(1)) {
            send_key(&sess, KEY_DOWN).await;
        }
        send_key(&sess, KEY_ENTER).await;
    }
    // Every question answered, the form is on its Submit tab.
    send_key(&sess, KEY_ENTER).await;
    // Optimistically clear the prompt and mark the session working again.
    if let Some(st) = state.statuses.lock().unwrap().get_mut(&key) {
        st.prompt.clear();
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
                let chat = crate::chat_workspace_of(&s.name).is_some();
                let workspace =
                    crate::workspace_for(&s.name).map(|p| p.to_string_lossy().into_owned());
                SandboxItem {
                    name: s.name,
                    agent: s.agent,
                    status: s.status,
                    workspace,
                    chat,
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
    // If this is a chat sandbox, remember its throwaway temp workspace so we can
    // delete it once the sandbox itself is gone.
    let chat_dir = crate::chat_workspace_of(&name);
    match tokio::task::spawn_blocking({
        let name = name.clone();
        move || sbx::rm_sandboxes(&[name.as_str()], false)
    })
    .await
    {
        Ok(Ok(())) => {
            if let Some(dir) = chat_dir {
                let _ = std::fs::remove_dir_all(&dir);
            }
            Json(serde_json::json!({ "ok": true }))
        }
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

    if !crate::is_valid_sandbox_name(&name) {
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
struct ChatBody {
    #[serde(default)]
    name: Option<String>,
}

/// `POST /api/sandboxes/chat` — the web UI's 💬 button. Provisions a sandbox on
/// a fresh empty workspace, so the agent has none of your code to read or edit.
/// The CLI's `sbxw chat` shares the helpers used here; see the chat section of
/// `main.rs` for what a chat sandbox does and doesn't inherit.
async fn api_chat(
    State(state): State<Arc<AppState>>,
    Json(body): Json<ChatBody>,
) -> Json<serde_json::Value> {
    // Use the caller's name, or mint a unique `chat-xxxxxx`.
    let name = match body
        .name
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        Some(n) => n.to_string(),
        None => crate::mint_chat_name(),
    };
    if !crate::is_valid_sandbox_name(&name) {
        return Json(serde_json::json!({
            "ok": false,
            "error": "name must be non-empty and contain only letters, digits, and hyphens"
        }));
    }

    let workspace = match crate::prepare_chat_workspace(&name) {
        Ok(w) => w,
        Err(e) => return Json(serde_json::json!({ "ok": false, "error": e.to_string() })),
    };

    let cfg = state.cfg.clone();
    let use_api_key = state.use_api_key;
    let name_ret = name.clone();
    tracing::info!("web UI: provisioning chat sandbox '{name}' at {workspace}");
    match tokio::task::spawn_blocking(move || {
        crate::provision_sandbox(&name, &workspace, &[], &cfg, &[], use_api_key)
    })
    .await
    {
        Ok(Ok(())) => Json(serde_json::json!({ "ok": true, "name": name_ret })),
        Ok(Err(e)) => Json(serde_json::json!({ "ok": false, "error": e.to_string() })),
        Err(_) => Json(serde_json::json!({ "ok": false, "error": "task panic" })),
    }
}

/// The shared "ephemeral chat" sandbox the island's composer talks to. One
/// sandbox, reused: the point is a scratch agent that's always one keystroke
/// away, not a new sandbox per message — so the second question lands in the
/// same conversation as the first.
const EPHEMERAL_CHAT: &str = "ephemeral-chat";

/// How long the PTY must stay silent before we accept that the agent's TUI has
/// finished drawing and won't swallow what we type into a half-painted screen.
const CHAT_SETTLE: Duration = Duration::from_millis(900);

/// Upper bound on waiting for that: a cold sandbox has to boot the agent first.
const CHAT_READY_TIMEOUT: Duration = Duration::from_secs(45);

#[derive(Deserialize)]
struct ChatPushBody {
    /// Message to submit to the chat agent.
    text: String,
    /// Sandbox to chat in. Defaults to the shared ephemeral one.
    #[serde(default)]
    name: Option<String>,
}

/// Wait until `sess` stops producing output, so a prompt isn't typed into a TUI
/// that is still painting (Claude Code redraws its whole frame on startup and
/// would drop the keystrokes).
///
/// Quiescence is measured on the broadcast channel rather than the replay ring
/// buffer: that buffer is capacity-bounded, so once full its length stops
/// changing and silence becomes indistinguishable from a flood.
async fn wait_until_settled(sess: &Arc<PtySession>, fresh: bool) {
    let mut rx = sess.tx.subscribe();
    if fresh {
        // A cold session emits nothing until the agent boots; timing the
        // silence from now would "settle" instantly, before anything is
        // listening. Wait for the first output, then for it to stop.
        if tokio::time::timeout(CHAT_READY_TIMEOUT, rx.recv())
            .await
            .is_err()
        {
            return;
        }
    }
    let deadline = tokio::time::Instant::now() + CHAT_READY_TIMEOUT;
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(CHAT_SETTLE, rx.recv()).await {
            Err(_) => return,      // quiet for CHAT_SETTLE — the TUI is ready
            Ok(Ok(_)) => continue, // still drawing
            Ok(Err(_)) => return,  // lagged or closed; don't block on it
        }
    }
}

/// `POST /api/chat/push` — submit a message to the ephemeral chat agent,
/// creating the sandbox and attaching its session first if they don't exist.
///
/// This is the island composer's one call: it has no sandbox picker and no
/// terminal, so everything between "user typed a question" and "the agent is
/// reading it" has to happen here. Reuses the same provisioning path as the web
/// UI's 💬 button, so a chat started from the island is the same thing as one
/// started from the browser.
async fn api_chat_push(
    State(state): State<Arc<AppState>>,
    Json(body): Json<ChatPushBody>,
) -> Json<serde_json::Value> {
    let text = body.text.trim().to_string();
    if text.is_empty() {
        return Json(serde_json::json!({ "ok": false, "error": "empty message" }));
    }
    let name = body
        .name
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(EPHEMERAL_CHAT)
        .to_string();
    if !crate::is_valid_sandbox_name(&name) {
        return Json(serde_json::json!({
            "ok": false,
            "error": "name must be non-empty and contain only letters, digits, and hyphens"
        }));
    }

    // 1. Provision on first use only. Re-provisioning per message would redo
    //    policy, hooks and trust on every keystroke-worth of chat.
    let exists = {
        let n = name.clone();
        tokio::task::spawn_blocking(move || crate::sbx::exists(&n))
            .await
            .ok()
            .and_then(Result::ok)
            .unwrap_or(false)
    };
    if !exists {
        let workspace = match crate::prepare_chat_workspace(&name) {
            Ok(w) => w,
            Err(e) => return Json(serde_json::json!({ "ok": false, "error": e.to_string() })),
        };
        let cfg = state.cfg.clone();
        let use_api_key = state.use_api_key;
        let n = name.clone();
        tracing::info!("island: provisioning ephemeral chat sandbox '{name}' at {workspace}");
        match tokio::task::spawn_blocking(move || {
            crate::provision_sandbox(&n, &workspace, &[], &cfg, &[], use_api_key)
        })
        .await
        {
            Ok(Ok(())) => {}
            Ok(Err(e)) => return Json(serde_json::json!({ "ok": false, "error": e.to_string() })),
            Err(_) => return Json(serde_json::json!({ "ok": false, "error": "task panic" })),
        }
    }

    // 2. Attach the agent. `sbx run --name` also starts a stopped sandbox, so
    //    this covers "the chat sandbox exists but was stopped" for free.
    let key = format!("{name}::claude");
    let fresh = !state.sessions.lock().unwrap().contains_key(&key);
    let sessions = state.sessions.clone();
    let shell = state.shell.clone();
    let n = name.clone();
    let session = match tokio::task::spawn_blocking(move || {
        get_or_create_session(&n, "claude", &shell, &sessions)
    })
    .await
    {
        Ok(Ok(s)) => s,
        Ok(Err(e)) => return Json(serde_json::json!({ "ok": false, "error": e.to_string() })),
        Err(_) => return Json(serde_json::json!({ "ok": false, "error": "task panic" })),
    };

    // 3. Let the TUI finish drawing, then type the message.
    wait_until_settled(&session, fresh).await;
    {
        let mut w = session.writer.lock().unwrap();
        let _ = w.write_all(text.as_bytes());
        let _ = w.flush();
    }

    // 4. Submit — but only once the message has stopped echoing.
    //
    //    Return has to arrive as its own keystroke. Claude Code reads a burst of
    //    closely-spaced bytes as a *paste*, and a newline inside a paste is
    //    inserted into the message rather than sending it: the text landed in
    //    the box and simply sat there. `api_answer`'s 60 ms is enough between
    //    two isolated arrow keys, nowhere near enough after a block of text.
    //    Waiting for the echo to go quiet also scales with the machine, which a
    //    fixed guess would not.
    wait_until_settled(&session, false).await;
    {
        let mut w = session.writer.lock().unwrap();
        let _ = w.write_all(KEY_ENTER);
        let _ = w.flush();
    }
    tracing::info!("island: pushed {} chars into '{name}'", text.len());

    Json(serde_json::json!({ "ok": true, "name": name, "created": !exists }))
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
///   mode == "bash"  → `sbx exec -it <sandbox> -- bash`, or SSH if it's stopped
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

    // A chat sandbox's throwaway workspace lives under `/tmp` and may have been
    // swept away since it was created; `sbx run`/`exec` would then fail to start
    // the runtime with a 422. Re-create the empty directory first (no-op for a
    // normal sandbox, or a chat one still present) so attaching just works.
    crate::ensure_chat_workspace(sandbox);

    // `sbx exec` only reaches a *running* sandbox, while `sbx run` (the agent
    // pane) starts a stopped one as a side effect. That asymmetry made the Bash
    // button a dead end on a stopped sandbox: it failed, and the only way out
    // was to attach the agent first just to boot the thing. An SSH connection
    // brings up the daemon and the sandbox on demand, so use it for exactly
    // that case — the running case stays on `sbx exec`, which needs no SSH
    // setup at all.
    let bash_over_ssh = mode == "bash" && !crate::sbx::is_running(sandbox).unwrap_or(true);

    let mut cmd = if bash_over_ssh {
        tracing::info!("'{sandbox}' is stopped — opening the Bash pane over SSH to start it");
        // The banner names the one prerequisite *before* ssh can fail on it;
        // `exec` keeps the shell from lingering as a useless parent process.
        //
        // The sandbox name is passed as a positional argument (`$1`) rather
        // than interpolated into the script: it arrives from a WebSocket query
        // parameter, so folding it into a `sh -c` string would be a command
        // injection. Same reasoning as `sbx::write_file_stdin`. The printf
        // format is single-quoted so no `$`/backtick in it is ever expanded.
        const SSH_BANNER: &str = r#"printf '\033[2m[sbxw] %s is stopped - connecting over SSH to start it.\r\n[sbxw] If this fails, run: sbxw ssh --setup\033[0m\r\n' "$1"; exec ssh -t "$1.sbx""#;
        let mut c = CommandBuilder::new("sh");
        c.args(["-c", SSH_BANNER, "sh", sandbox]);
        c
    } else if mode == "bash" {
        let mut c = CommandBuilder::new("sbx");
        c.args(["exec", "-it", sandbox, "--", "bash"]);
        c
    } else if shell.is_empty() {
        // Re-attach by name. The positional form (`sbx run <name>`) is
        // deprecated; `--name` re-attaches regardless of working directory.
        // Since sbx 0.35 this also works for sandboxes created with a custom
        // --kit (like sbxw's OAuth kit) without re-passing the kit.
        let mut c = CommandBuilder::new("sbx");
        c.args(["run", "--name", sandbox]);
        c
    } else {
        let mut c = CommandBuilder::new("sbx");
        c.args(["exec", "-it", sandbox, "--", shell]);
        c
    };
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

    fn ask_json(question: &str, opts: &[(&str, &str)]) -> serde_json::Value {
        let options: Vec<_> = opts
            .iter()
            .map(|(l, d)| json!({ "label": l, "description": d }))
            .collect();
        json!({ "question": question, "options": options })
    }

    fn ask_body(question: &str, opts: &[(&str, &str)]) -> serde_json::Value {
        ask_body_multi(&[(question, opts)])
    }

    /// A hook body carrying several questions, the way the terminal lays them
    /// out as tabs.
    fn ask_body_multi(steps: &[(&str, &[(&str, &str)])]) -> serde_json::Value {
        let questions: Vec<_> = steps.iter().map(|(q, o)| ask_json(q, o)).collect();
        json!({
            "hook_event_name": "PreToolUse",
            "tool_name": "AskUserQuestion",
            "tool_input": { "questions": questions }
        })
    }

    #[test]
    fn questions_from_ask_builds_options_and_decision_table() {
        let body = ask_body(
            "Which deployment target?",
            &[("Production", "The live site"), ("Staging", "The test env")],
        );
        let steps = questions_from_ask(&body);
        assert_eq!(steps.len(), 1);
        let q = &steps[0];
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
    fn questions_from_ask_keeps_every_step_in_tab_order() {
        let body = ask_body_multi(&[
            ("Quel thème ?", &[("Foot", "le ballon rond"), ("Ciné", "")]),
            ("Quelle difficulté ?", &[("Facile", ""), ("Moyen", "")]),
        ]);
        let steps = questions_from_ask(&body);
        assert_eq!(steps.len(), 2);
        assert_eq!(steps[0].text, "Quel thème ?");
        assert_eq!(steps[1].text, "Quelle difficulté ?");
        assert_eq!(steps[1].options, vec!["Facile", "Moyen"]);
    }

    #[test]
    fn questions_from_ask_rejects_single_option() {
        let body = ask_body("Only one?", &[("Yes", "")]);
        assert!(questions_from_ask(&body).is_empty());
    }

    /// Dropping just the bad step would shift every later answer onto the wrong
    /// tab, so an unusable step voids the whole prompt.
    #[test]
    fn questions_from_ask_rejects_the_lot_when_a_step_is_unusable() {
        let body = ask_body_multi(&[
            ("Fine?", &[("A", ""), ("B", "")]),
            ("Only one?", &[("Yes", "")]),
        ]);
        assert!(questions_from_ask(&body).is_empty());
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
        assert!(st.prompt.is_empty());
    }

    #[test]
    fn apply_hook_ask_question_enters_attention_with_question() {
        let mut st = SessionStatus::default();
        let body = ask_body("Pick one?", &[("A", "first"), ("B", "second")]);
        apply_hook("PreToolUse", "AskUserQuestion", &body, &mut st);
        assert_eq!(st.state, SessionState::Attention);
        assert_eq!(st.prompt.len(), 1);
        assert_eq!(st.prompt[0].text, "Pick one?");
        assert_eq!(st.prompt[0].options, vec!["A", "B"]);
    }

    #[test]
    fn apply_hook_ask_question_keeps_all_steps() {
        let mut st = SessionStatus::default();
        let body = ask_body_multi(&[
            ("Quel thème ?", &[("Foot", ""), ("Ciné", "")]),
            ("Quelle difficulté ?", &[("Facile", ""), ("Moyen", "")]),
        ]);
        apply_hook("PreToolUse", "AskUserQuestion", &body, &mut st);
        assert_eq!(st.state, SessionState::Attention);
        assert_eq!(st.prompt.len(), 2);
        // The activity line summarises the prompt with its first step.
        assert_eq!(st.activity.as_deref(), Some("Quel thème ?"));
        // Both steps reach the island; `question` stays the first one.
        let info = build_info("box::claude", &st);
        assert_eq!(info.question.expect("first step").text, "Quel thème ?");
        assert_eq!(info.steps.expect("all steps").len(), 2);
    }

    #[test]
    fn apply_hook_other_tool_is_working_and_clears_question() {
        let mut st = SessionStatus {
            state: SessionState::Attention,
            prompt: vec![Question {
                text: "old?".into(),
                options: vec!["a".into(), "b".into()],
                context: vec![],
            }],
            ..Default::default()
        };
        let body = json!({
            "hook_event_name": "PreToolUse",
            "tool_name": "Edit",
            "tool_input": { "file_path": "/x/y.rs" }
        });
        apply_hook("PreToolUse", "Edit", &body, &mut st);
        assert_eq!(st.state, SessionState::Working);
        assert!(st.prompt.is_empty());
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
            prompt: vec![Question {
                text: "q?".into(),
                options: vec!["a".into(), "b".into()],
                context: vec![],
            }],
            ..Default::default()
        };
        apply_hook("Stop", "", &json!({ "hook_event_name": "Stop" }), &mut st);
        assert_eq!(st.state, SessionState::Idle);
        assert!(st.prompt.is_empty());
    }

    #[test]
    fn apply_hook_stop_captures_the_reply_and_a_new_prompt_clears_it() {
        let mut st = SessionStatus::default();
        apply_hook(
            "Stop",
            "",
            &json!({
                "hook_event_name": "Stop",
                "last_assistant_message": "Done — the port is free now.\nI also bumped the version.",
            }),
            &mut st,
        );
        let reply = st.reply.clone().expect("reply captured");
        assert!(reply.starts_with("Done — the port is free now."));
        // Line structure survives: the island's accordion needs more than one.
        assert_eq!(reply.lines().count(), 2);

        // Submitting again makes it stale, so it must not caption the new turn.
        apply_hook(
            "UserPromptSubmit",
            "",
            &json!({ "hook_event_name": "UserPromptSubmit", "prompt": "next" }),
            &mut st,
        );
        assert_eq!(st.reply, None);
    }

    #[test]
    fn apply_hook_stop_without_a_reply_keeps_the_previous_one() {
        // A turn ending on a tool call has no prose; Claude Code then sends no
        // `last_assistant_message` and the last real answer stays on the row.
        let mut st = SessionStatus {
            reply: Some("earlier answer".into()),
            ..Default::default()
        };
        apply_hook("Stop", "", &json!({ "hook_event_name": "Stop" }), &mut st);
        assert_eq!(st.reply.as_deref(), Some("earlier answer"));
    }

    #[test]
    fn clip_lines_keeps_lines_and_bounds_length() {
        let long = "one\ntwo\nthree\nfour\nfive";
        assert_eq!(clip_lines(long, 3, 100), "one\ntwo\nthree");
        let clipped = clip_lines(&"x".repeat(50), 3, 10);
        assert_eq!(clipped.chars().count(), 11); // 10 + the ellipsis
        assert!(clipped.ends_with('…'));
        // Blank input must not become a lone newline.
        assert_eq!(clip_lines("  \n \n ", 3, 100), "");
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
