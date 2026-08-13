//! Browser terminal with sandbox switcher sidebar.
//!
//! PTY sessions are persistent — they survive WebSocket disconnects.
//! Refreshing the browser tab replays the last 256 KB of output and
//! resumes the live stream without restarting the agent.
//!
//! Routes:
//!   GET  /                          → HTML (initial_sandbox embedded)
//!   GET  /api/events                → SSE stream of rich session updates (macOS island)
//!   GET  /api/stream                → SSE: session updates + focus requests + open-tab count,
//!                                      multiplexed for the browser UI (see `api_stream`)
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
//!   GET  /api/sandboxes/:name/policy → network rules in force (`sbx policy ls`)
//!   POST /api/sandboxes/:name/policy/rules    → add an allow/deny network rule
//!   POST /api/sandboxes/:name/policy/rules/rm → remove one rule by id
//!   GET  /api/fs?path=<dir>         → directory listing for the folder picker
//!   POST /api/fs/pick               → OS-native folder picker (Finder/Explorer/zenity)
//!   GET  /api/sandboxes/:name/artifacts             → non-code files under .sbxw-artifacts
//!   GET  /api/sandboxes/:name/artifacts/download     → download one of those files
//!   GET  /ws?sandbox=<name>         → WebSocket ↔ persistent PTY

use crate::config::Config;
use crate::hosts::{self, HostAlias};
use crate::sbx;
use crate::ExtraPort;
use anyhow::{bail, Context, Result};
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
use tokio::sync::{broadcast, watch};
use tokio_stream::wrappers::{BroadcastStream, ReceiverStream};

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
    /// Child process handle — kept alive so the process is properly reaped on
    /// exit, and polled by `alive`.
    child: Mutex<Box<dyn portable_pty::Child + Send + Sync>>,
}

impl PtySession {
    /// Whether the PTY's process is still running.
    ///
    /// A session entry outlives its process — nothing removes it from the map
    /// when the agent exits — so "there is a session for this sandbox" is not
    /// the same as "there is something to type into". Anything that treats the
    /// map as evidence about the *sandbox* has to ask this too. Unreadable exit
    /// status counts as alive: the pessimistic answer here is the one that only
    /// costs an extra check, never a lost message.
    fn alive(&self) -> bool {
        self.child
            .lock()
            .unwrap()
            .try_wait()
            .map(|status| status.is_none())
            .unwrap_or(true)
    }
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
    /// Claude Code's own session id. Absent from a session reported by an
    /// in-sandbox hook script older than this field.
    #[serde(skip_serializing_if = "Option::is_none")]
    session_id: Option<String>,
    /// The directory this session runs in. Two agents in one container are
    /// otherwise indistinguishable on screen, and the cwd is usually what tells
    /// them apart — Claude Code also scopes its transcripts by it, so it is the
    /// same string that decides what `/resume` offers.
    #[serde(skip_serializing_if = "Option::is_none")]
    cwd: Option<String>,
    /// Where this session was started from: `tty` (sbxw's own terminal), `ssh`
    /// (Claude Desktop, an editor, a shell on `<name>.sbx`), or `unknown`.
    origin: SessionOrigin,
    /// Can the island answer this session's prompt for you? See `is_answerable`.
    answerable: bool,
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
    /// Claude Code's session id, as reported by its hook payload. Empty for a
    /// hook script that predates it.
    session_id: String,
    /// Working directory of the session, from the same payload.
    cwd: Option<String>,
    /// Where the session was started from, derived from the process ancestry
    /// its hook reported (see `classify_origin`).
    origin: SessionOrigin,
}

type Statuses = Arc<Mutex<HashMap<String, SessionStatus>>>;

/// Unix epoch milliseconds, for event timestamps.
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// Key a session is stored under: the agent ("claude") and a bash shell coexist
/// independently for the same sandbox, so both parts are needed to address one.
fn session_key(sandbox: &str, mode: &str) -> String {
    format!("{sandbox}::{mode}")
}

/// Mode of the host-wide monitor pane, and the pseudo-sandbox it is filed
/// under. The monitor watches *every* sandbox from the host, so it belongs to
/// none of them and needs a key of its own.
///
/// The underscores are what make it safe: `is_valid_sandbox_name` accepts only
/// letters, digits and hyphens, so no real sandbox can ever collide with this
/// key — and every endpoint that takes a name still rejects it, which is why
/// the monitor is reachable through the WebSocket alone and not, say, through
/// `rm`.
const MONITOR_KEY_MODE: &str = "monitor";
const MONITOR_SANDBOX: &str = "__host__";

/// Split a session key "<sandbox>::<mode>" back into its parts.
fn split_key(key: &str) -> (&str, &str) {
    let (sandbox, rest) = key.split_once("::").unwrap_or((key, ""));
    // An agent key carries its Claude Code session id after an `@`; the mode is
    // what precedes it. Sandbox names are `[A-Za-z0-9-]` only (see
    // `crate::is_valid_sandbox_name`), so `@` can never be part of one.
    (sandbox, rest.split('@').next().unwrap_or(rest))
}

/// Key an *agent* session's hook-driven state is stored under.
///
/// Claude Code gives every session a `session_id`, and one container can hold
/// several: the agent sbxw attached through its own PTY, plus anything started
/// over SSH — Claude Desktop, a terminal, an editor. They all read the same
/// `~/.claude/settings.json`, so they all fire sbxw's hooks. Keyed by sandbox
/// alone they overwrote each other and the island showed one incoherent state
/// for two agents; keyed by session they get a row each.
///
/// A hook that carries no session id (an older in-sandbox hook script) keeps
/// the plain `<sandbox>::claude` key, which is exactly what it used to get.
fn agent_status_key(sandbox: &str, session_id: &str) -> String {
    let base = session_key(sandbox, "claude");
    if session_id.is_empty() {
        base
    } else {
        format!("{base}@{session_id}")
    }
}

/// Where a Claude Code session was started from.
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Default, Debug)]
#[serde(rename_all = "lowercase")]
enum SessionOrigin {
    /// Attached by sbxw itself, through the PTY behind the browser terminal.
    Tty,
    /// Driven by an attached client rather than by sbxw: Claude Desktop, an
    /// editor, a shell on `<name>.sbx`. Named for what is observable — the
    /// session is not sbxw's — because the transport is not: Claude Desktop
    /// runs its own `server` process in the container and sets no sshd
    /// environment at all.
    Remote,
    /// The hook could not read its process tree, or predates `ancestry`.
    #[default]
    Unknown,
}

/// Everything a hook event says about where its session came from.
pub struct OriginEvidence<'a> {
    /// What the in-sandbox hook is capable of reporting; 0 for one that
    /// predates the question entirely.
    pub hook_version: u64,
    /// Names of sshd-set variables found in the session's environment.
    pub ssh_env: &'a [String],
    /// Process ancestry, innermost first.
    pub ancestry: &'a [String],
    /// The session's working directory.
    pub cwd: Option<&'a str>,
    /// The workspace sbxw recorded for this sandbox.
    pub workspace: Option<&'a str>,
}

/// Decide whether a session is the one sbxw attached, from the evidence a hook
/// event carries. Any single marker of a client is enough; the working
/// directory settles what is left.
///
///  1. **A client process in the ancestry.** Observed on a real sandbox: sbxw's
///     own session is `node < claude`, while Claude Desktop's is
///     `node < 2.1.222 < server` — it runs its own server in the container, the
///     way an editor's remote extension does. An `sshd` ancestor counts the
///     same way, for a plain `ssh <name>.sbx`.
///  2. **sshd's environment.** `SSH_CONNECTION` / `SSH_CLIENT` / `SSH_TTY`,
///     set per session by sshd and inherited through the agent. Conclusive when
///     present — but Claude Desktop sets none of them, so its absence proves
///     nothing. (`SSH_AUTH_SOCK` is deliberately not among them: sbx forwards an
///     agent into every sandbox, so it is always set — see the hook.)
///  3. **The working directory.** sbxw records the workspace each sandbox was
///     created with and the agent it attaches starts there, while a client
///     lands wherever it chose (`/home/agent/workspace` for Claude Desktop).
///     Needs no hook support, so it covers sandboxes provisioned before any of
///     this — and it is what actually separates the two today.
///
/// A hook that reported nothing at all leaves `Unknown`, never `Tty`: the whole
/// point is to stop assuming a session is sbxw's own, and silence is not
/// evidence. A hook that *looked* has said something, so version 2 and above
/// may conclude `Tty` when every marker is absent.
fn classify_origin(ev: &OriginEvidence<'_>) -> SessionOrigin {
    let client_ancestor = ev.ancestry.iter().any(|name| {
        let n = name.trim().to_ascii_lowercase();
        // `server` is how a remote client's in-container half presents itself;
        // sshd covers someone arriving on `<name>.sbx` by hand.
        n == "server" || n == "sshd" || n == "ssh" || n.starts_with("sshd:")
    });
    if client_ancestor || !ev.ssh_env.is_empty() {
        return SessionOrigin::Remote;
    }
    match classify_origin_by_cwd(ev.cwd, ev.workspace) {
        SessionOrigin::Unknown if ev.hook_version >= 2 => SessionOrigin::Tty,
        other => other,
    }
}

/// Classify from the session's working directory instead, for a sandbox whose
/// in-container hook predates `ancestry`.
///
/// sbxw records the workspace it created each sandbox with, and the agent it
/// attaches starts there — while a session arriving over SSH lands wherever its
/// client put it (`/home/agent/workspace`, a home directory, anywhere). So a
/// cwd equal to the recorded workspace is the session sbxw started.
///
/// Weaker than sshd's environment and deliberately kept behind it: nothing
/// stops an SSH session from `cd`-ing into the workspace, and then both look
/// like the tty's. That misreading is *safe* — `is_answerable` requires exactly
/// one `Tty` session, so two of them make the sandbox read-only rather than
/// making sbxw type into the wrong terminal. The dangerous direction is the one
/// this cannot produce: an SSH session sitting somewhere else never passes for
/// the tty's.
fn classify_origin_by_cwd(cwd: Option<&str>, workspace: Option<&str>) -> SessionOrigin {
    match (cwd, workspace) {
        (Some(cwd), Some(workspace)) if !cwd.is_empty() && !workspace.is_empty() => {
            // Trailing slashes only: these are two recordings of one path, not
            // arbitrary user input to be normalised.
            if cwd.trim_end_matches('/') == workspace.trim_end_matches('/') {
                SessionOrigin::Tty
            } else {
                SessionOrigin::Remote
            }
        }
        _ => SessionOrigin::Unknown,
    }
}

/// Every agent status key currently held for `sandbox`.
fn agent_keys_for(statuses: &HashMap<String, SessionStatus>, sandbox: &str) -> Vec<String> {
    statuses
        .keys()
        .filter(|k| {
            let (s, mode) = split_key(k);
            s == sandbox && mode == "claude"
        })
        .cloned()
        .collect()
}

// ── JSON envelopes ───────────────────────────────────────────────────────────
//
// Every mutating endpoint answers in one shape — `{ok: true, …}` or
// `{ok: false, error: "…"}` — because the frontend branches on `ok` alone. The
// helpers below are what keep that promise from being re-typed (and drifted
// from) at every handler.

/// Bare acknowledgement of success.
fn ok_json() -> Json<serde_json::Value> {
    Json(serde_json::json!({ "ok": true }))
}

/// Success carrying extra fields, e.g. the name of what was just created.
fn ok_json_with(fields: serde_json::Value) -> Json<serde_json::Value> {
    let mut body = serde_json::json!({ "ok": true });
    if let (Some(obj), Some(extra)) = (body.as_object_mut(), fields.as_object()) {
        obj.extend(extra.clone());
    }
    Json(body)
}

/// Failure carrying a human-readable reason. `sbx`'s own stderr reaches the user
/// through here (see `sbx::command_error`), so it must never be swallowed.
fn err_json(msg: impl std::fmt::Display) -> Json<serde_json::Value> {
    Json(serde_json::json!({ "ok": false, "error": msg.to_string() }))
}

/// Run a blocking `sbx`/filesystem operation off the async runtime and render
/// its outcome as the envelope above: `on_ok` shapes the success body, while a
/// returned error — or a panicked task — becomes `{ok: false, error}`.
async fn blocking<T, F, G>(f: F, on_ok: G) -> Json<serde_json::Value>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T> + Send + 'static,
    G: FnOnce(T) -> Json<serde_json::Value>,
{
    match tokio::task::spawn_blocking(f).await {
        Ok(Ok(value)) => on_ok(value),
        Ok(Err(e)) => err_json(format!("{e:#}")),
        Err(_) => err_json("task panic"),
    }
}

/// `blocking` for the common case: nothing to report beyond "it worked".
async fn blocking_ok<F>(f: F) -> Json<serde_json::Value>
where
    F: FnOnce() -> Result<()> + Send + 'static,
{
    blocking(f, |()| ok_json()).await
}

/// `blocking` for use *mid-handler*, where success has to feed the next step:
/// yields the value, or the ready-made error envelope for the caller to return.
async fn try_blocking<T, F>(f: F) -> std::result::Result<T, Json<serde_json::Value>>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T> + Send + 'static,
{
    match tokio::task::spawn_blocking(f).await {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(e)) => Err(err_json(format!("{e:#}"))),
        Err(_) => Err(err_json("task panic")),
    }
}

/// The one rule for a sandbox name, and the one message explaining it — shared
/// so every endpoint that accepts a name rejects the same inputs identically.
fn reject_invalid_name(name: &str) -> Option<Json<serde_json::Value>> {
    (!crate::is_valid_sandbox_name(name)).then(|| err_json(crate::INVALID_NAME_MSG))
}

/// Can the island answer this session's prompt on your behalf?
///
/// Answering means typing arrow keys and Enter into a PTY, so it is only
/// legitimate when sbxw's PTY is unambiguously the terminal that asked. Two
/// situations qualify, and nothing else:
///
///  * the sandbox has a single agent session — there is only one terminal it
///    could be, which is every ordinary sbxw sandbox;
///  * several sessions share the container but exactly one of them was started
///    from the tty sbxw attached (see `classify_origin`), so the PTY it holds
///    is that session's and no other's.
///
/// Everything else is read-only. Two `Tty` sessions, or none, or a chain the
/// hook could not read, all leave sbxw unable to say which terminal asked —
/// and typing an answer into the wrong one answers someone else's question,
/// which is worse than not answering at all. The island still *shows* those
/// rows; it just stops offering a button it cannot honour.
///
/// `has_pty` is passed in rather than looked up so this never holds the
/// `statuses` lock while taking the `sessions` one — the two are taken in the
/// opposite order elsewhere, and a nested pair is how that becomes a deadlock.
fn is_answerable(statuses: &HashMap<String, SessionStatus>, has_pty: bool, key: &str) -> bool {
    let (sandbox, mode) = split_key(key);
    if !has_pty || mode != "claude" {
        return false;
    }
    let siblings = agent_keys_for(statuses, sandbox);
    if siblings.len() == 1 {
        return true;
    }
    // Shared container: only the one session sbxw itself started, and only
    // while it is the only such session.
    let from_tty: Vec<&String> = siblings
        .iter()
        .filter(|k| {
            statuses
                .get(*k)
                .is_some_and(|st| st.origin == SessionOrigin::Tty)
        })
        .collect();
    from_tty.len() == 1 && from_tty[0] == key
}

/// Build the rich payload for a session from its current status.
fn build_info(key: &str, st: &SessionStatus, answerable: bool) -> SessionInfo {
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
        session_id: (!st.session_id.is_empty()).then(|| st.session_id.clone()),
        cwd: st.cwd.clone(),
        origin: st.origin,
        answerable,
        ts: now_ms(),
    }
}

/// Broadcast the current state of `key`, if it still exists.
///
/// Every sibling agent session in the same sandbox is re-emitted too: whether a
/// row can be answered depends on how many of them there are, so the arrival or
/// departure of one changes the others' payload without changing their state.
fn emit_info(
    events: &broadcast::Sender<SessionInfo>,
    statuses: &Statuses,
    sessions: &Sessions,
    key: &str,
) {
    let (sandbox, _) = split_key(key);
    let has_pty = sessions
        .lock()
        .unwrap()
        .contains_key(&session_key(sandbox, "claude"));
    let infos: Vec<SessionInfo> = {
        let map = statuses.lock().unwrap();
        let mut keys = agent_keys_for(&map, sandbox);
        if !keys.iter().any(|k| k == key) {
            keys.push(key.to_string());
        }
        keys.iter()
            .filter_map(|k| {
                let st = map.get(k)?;
                Some(build_info(k, st, is_answerable(&map, has_pty, k)))
            })
            .collect()
    };
    for info in infos {
        // `send` errors only when nobody is listening; nothing to do then.
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
    sessions: Sessions,
    /// Broadcast bus of rich session updates (see `/api/events`).
    events: broadcast::Sender<SessionInfo>,
    /// Broadcast bus of "focus this sandbox" requests from the macOS island,
    /// delivered to open web tabs over `/api/focus-events` so a click reuses an
    /// existing tab instead of spawning a new one.
    focus: broadcast::Sender<String>,
    /// The same bus in the opposite direction: "a browser tab is now watching
    /// this sandbox's terminal", delivered to the island over
    /// `/api/watch-events` so it can retire a notification the user has already
    /// gone and read (see `api_watching`).
    watching: broadcast::Sender<String>,
    /// Current state per session, for the `/api/sessions` snapshot.
    statuses: Statuses,
    /// Recent hook events (POC).
    hook_log: HookLog,
    /// Sandboxes whose in-container hook has been seen reporting no process
    /// ancestry, so the "refresh it" advice is logged once each and not on
    /// every tool call.
    stale_hooks: Arc<Mutex<std::collections::HashSet<String>>>,
    /// Latest subscription usage (see `/api/usage`).
    usage: Arc<Mutex<UsageInfo>>,
    /// Count of open `/api/stream` connections — one per browser tab currently
    /// loaded (see `api_stream`, which folds this into the same connection as
    /// the session/focus SSE feeds). sbxw is built around a single client: two
    /// runtime worker threads, one PTY per sandbox, and a handful of the
    /// browser's six-per-origin HTTP/1.1 connection slots, all shared by
    /// however many tabs attach. A second tab doesn't get its own slice of any
    /// of that, it just contends with the first for all of it. The web UI
    /// shows a warning as soon as this exceeds one.
    client_count: watch::Sender<usize>,
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
    /// "claude" (default) attaches the agent, "bash" opens a shell via sbx exec,
    /// "monitor" runs the host dashboard and ignores `sandbox`.
    mode: Option<String>,
}

const INDEX_HTML_TEMPLATE: &str = include_str!("../assets/index.html");

/// The page's stylesheet and scripts, compiled into the binary alongside the
/// HTML shell that loads them. Split out of `index.html` so each part can be
/// read on its own; the shell is the only templated file, which is why the two
/// injected values live in an inline `<script>` there rather than in a module.
///
/// Served from a table rather than a `/{file}` route: the paths are a closed
/// set known at compile time, so there is nothing to resolve at runtime and no
/// path to traverse.
/// The scripts load in this order and share one global scope, exactly as they
/// did when they were a single inline block — `main.js` last, since it is the
/// one that runs rather than declares. `singleton.js` is the odd one out: it
/// is the only one loaded by a static `<script>` tag in the HTML shell, and
/// it injects the rest of this list itself, once it decides this tab should
/// actually boot (see the duplicate-tab interstitial in index.html).
const JS: &str = "application/javascript; charset=utf-8";
const STATIC_ASSETS: &[(&str, &str, &str)] = &[
    (
        "/app.css",
        "text/css; charset=utf-8",
        include_str!("../assets/app.css"),
    ),
    (
        "/js/singleton.js",
        JS,
        include_str!("../assets/js/singleton.js"),
    ),
    ("/js/util.js", JS, include_str!("../assets/js/util.js")),
    ("/js/panes.js", JS, include_str!("../assets/js/panes.js")),
    (
        "/js/pane-controls.js",
        JS,
        include_str!("../assets/js/pane-controls.js"),
    ),
    (
        "/js/sandboxes.js",
        JS,
        include_str!("../assets/js/sandboxes.js"),
    ),
    ("/js/create.js", JS, include_str!("../assets/js/create.js")),
    ("/js/ports.js", JS, include_str!("../assets/js/ports.js")),
    ("/js/files.js", JS, include_str!("../assets/js/files.js")),
    ("/js/ssh.js", JS, include_str!("../assets/js/ssh.js")),
    (
        "/js/lifecycle.js",
        JS,
        include_str!("../assets/js/lifecycle.js"),
    ),
    ("/js/main.js", JS, include_str!("../assets/js/main.js")),
];

pub async fn serve(
    addr: &str,
    initial_sandbox: String,
    cfg: Arc<Config>,
    use_api_key: bool,
) -> Result<()> {
    // Session state is driven by Claude Code hook events (see `api_hook`), so
    // there is no output-timing scanner: `Idle` comes from a `Stop` event, not
    // a quiet timer.
    let (events, _) = broadcast::channel::<SessionInfo>(256);
    let (focus, _) = broadcast::channel::<String>(16);
    let (watching, _) = broadcast::channel::<String>(16);
    let (client_count, _) = watch::channel::<usize>(0);
    let statuses: Statuses = Arc::new(Mutex::new(HashMap::new()));
    // Hoisted above the reconciler because emitting a session's state now needs
    // to know whether sbxw holds a PTY for its sandbox (see `is_answerable`).
    let sessions: Sessions = Arc::new(Mutex::new(HashMap::new()));

    // Reconcile against reality: a sandbox stopped or removed out-of-band never
    // sends a `SessionEnd` hook, so its status would linger. Every 15 s, drop
    // any status whose sandbox `sbx ls` no longer reports as running — emitting
    // an `Exited` first so subscribers remove it immediately. `sbx ls` is the
    // authority (same source the UI polls); an empty result is treated as a
    // transient failure and skipped, never as "everything is gone".
    {
        let events = events.clone();
        let statuses = statuses.clone();
        let sessions = sessions.clone();
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
                    emit_info(&events, &statuses, &sessions, &key);
                    statuses.lock().unwrap().remove(&key);
                    tracing::info!("reconcile: dropped stale session '{key}' (sandbox gone)");
                }
            }
        });
    }

    let state = Arc::new(AppState {
        initial_sandbox,
        sessions,
        events,
        focus,
        watching,
        statuses,
        hook_log: Arc::new(Mutex::new(VecDeque::new())),
        stale_hooks: Arc::new(Mutex::new(std::collections::HashSet::new())),
        usage: Arc::new(Mutex::new(UsageInfo::default())),
        client_count,
        cfg,
        use_api_key,
    });

    let app = Router::new()
        .route("/", get(index_handler))
        .merge(static_assets())
        .route("/ws", get(ws_handler))
        .route("/api/events", get(api_events))
        .route("/api/focus", post(api_focus))
        .route("/api/watching", post(api_watching))
        .route("/api/watch-events", get(api_watch_events))
        .route("/api/stream", get(api_stream))
        .route("/api/sessions", get(api_sessions))
        .route("/api/ptys", get(api_ptys))
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
        .route("/api/sandboxes/:name/policy", get(api_policy_one))
        .route("/api/sandboxes/:name/policy/rules", post(api_policy_add))
        .route("/api/sandboxes/:name/policy/rules/rm", post(api_policy_rm))
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

/// Routes for `STATIC_ASSETS`. Everything is baked into the binary, so each one
/// is a constant response with its content type.
fn static_assets() -> Router<Arc<AppState>> {
    let mut router = Router::new();
    for (path, content_type, body) in STATIC_ASSETS {
        router = router.route(
            path,
            get(move || async move { ([(header::CONTENT_TYPE, *content_type)], *body) }),
        );
    }
    router
}

async fn index_handler(State(state): State<Arc<AppState>>) -> Html<String> {
    // The monitor command is echoed into the page so the button can label
    // itself with what it will actually run — and stay hidden when nothing is
    // configured. Quotes are stripped rather than escaped: it lands inside a
    // JS string literal, and a command with a quote in it isn't worth a
    // serialiser here.
    let monitor = state.cfg.monitor_cmd.join(" ").replace(['"', '\\'], "");
    Html(
        INDEX_HTML_TEMPLATE
            .replace("__SANDBOX__", &state.initial_sandbox)
            .replace("__MONITOR__", &monitor),
    )
}

/// `GET /api/ptys` — the size every live PTY actually has, straight from
/// `TIOCGWINSZ`.
///
/// Diagnostic: when a TUI draws into a fraction of its pane, the question is
/// whether the browser's terminal and the PTY disagree, and this is the side
/// that cannot be read from the browser. Kept out of `/api/sessions`, which
/// answers from `statuses` — hook-derived agent lifecycle, a different map with
/// different keys (a bash PTY has no status, a status outlives its PTY).
async fn api_ptys(State(state): State<Arc<AppState>>) -> Json<Vec<serde_json::Value>> {
    // Cloned out of the map so the ioctls below don't run under its lock.
    let entries: Vec<(String, Arc<PtySession>)> = state
        .sessions
        .lock()
        .unwrap()
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    // `get_size` and `alive` are syscalls behind a std mutex — off the runtime,
    // as everywhere else in this file that touches one.
    let list = tokio::task::spawn_blocking(move || {
        entries
            .into_iter()
            .map(|(key, s)| {
                let size = s.master.lock().ok().and_then(|m| m.get_size().ok());
                serde_json::json!({
                    "key": key,
                    "cols": size.map(|z| z.cols),
                    "rows": size.map(|z| z.rows),
                    "alive": s.alive(),
                })
            })
            .collect::<Vec<_>>()
    })
    .await
    .unwrap_or_default();
    Json(list)
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

#[derive(Deserialize)]
struct WatchReq {
    sandbox: String,
}

/// `POST /api/watching` — the web UI reporting that the user is now reading
/// this sandbox's terminal (its pane holds the focus, in a focused window).
///
/// Deliberately an *event* and not a stored flag. A flag would need a heartbeat
/// and per-tab bookkeeping to survive a closed tab, and a stale one would go on
/// silencing a sandbox nobody is watching. What consumers actually want is the
/// moment the user arrived: the island turns it into the same acknowledgement
/// its own ✕ produces, which its content keying already retires as soon as the
/// session asks something new.
async fn api_watching(
    State(state): State<Arc<AppState>>,
    Json(req): Json<WatchReq>,
) -> Json<serde_json::Value> {
    let clients = state.watching.receiver_count();
    // `send` errors only when there are no subscribers; nothing to do then.
    let _ = state.watching.send(req.sandbox);
    Json(serde_json::json!({ "clients": clients }))
}

/// Live stream of "this sandbox is being watched" reports (see `api_watching`).
/// The macOS island subscribes here; each event's `data:` payload is the bare
/// sandbox name.
async fn api_watch_events(
    State(state): State<Arc<AppState>>,
) -> Sse<impl Stream<Item = Result<SseEvent, std::convert::Infallible>>> {
    let stream = BroadcastStream::new(state.watching.subscribe()).filter_map(|res| async move {
        // `Err` here is a lagged receiver — skip the gap rather than closing.
        let name = res.ok()?;
        Some(Ok(SseEvent::default().data(name)))
    });
    Sse::new(stream).keep_alive(KeepAlive::default())
}

/// Multiplexes the three SSE feeds a browser tab needs — rich session
/// updates, "the island wants this tab to focus a sandbox", and the open-tab
/// count — onto a single HTTP connection via named SSE events (`session`,
/// `focus`, `clients`).
///
/// Plain HTTP has no multiplexing (no TLS here, so no ALPN, so no h2), and
/// browsers cap concurrent connections to one origin at 6. Three separate
/// long-lived `EventSource`s per tab meant two tabs already claimed 8 of
/// those 6 sockets, so a third tab's very first `GET /` had nowhere to go —
/// it just queued in the browser, forever, since the sockets ahead of it
/// never close. One connection per tab instead of three buys the headroom
/// back. `/api/events` and `/api/watch-events` stay mounted unchanged: the
/// macOS island still reads those directly over its own connection pool,
/// which this doesn't touch.
async fn api_stream(
    State(state): State<Arc<AppState>>,
) -> Sse<impl Stream<Item = Result<SseEvent, std::convert::Infallible>>> {
    let (tx, rx) = tokio::sync::mpsc::channel::<SseEvent>(64);

    state.client_count.send_modify(|n| *n += 1);
    // Sent from inside the pump task too (on every subsequent change), but the
    // very first value has to be pushed explicitly — a `watch` receiver only
    // wakes on *changes*, and this connection arrived after the increment
    // above already happened.
    let _ = tx
        .try_send(SseEvent::default().event("clients").data(state.client_count.borrow().to_string()));

    let mut session_rx = state.events.subscribe();
    let mut focus_rx = state.focus.subscribe();
    let mut clients_rx = state.client_count.subscribe();
    let count_state = state.clone();

    tokio::spawn(async move {
        loop {
            tokio::select! {
                // Fires the moment `rx` below is dropped — i.e. the SSE
                // response itself ended (tab closed, navigated away, socket
                // dropped). Without this branch the loop would only find out
                // via a `send()` failure, which needs *some* session/focus/
                // clients event to come along first — on a quiet server that
                // could be arbitrarily late, leaving the count (and the
                // header warning) stuck wrong indefinitely.
                _ = tx.closed() => break,
                res = session_rx.recv() => match res {
                    Ok(info) => {
                        let ev = SseEvent::default()
                            .event("session")
                            .json_data(&info)
                            .unwrap_or_else(|_| SseEvent::default().event("session"));
                        if tx.send(ev).await.is_err() { break; }
                    }
                    // Lagged: skip the gap rather than closing the connection.
                    Err(broadcast::error::RecvError::Lagged(_)) => {}
                    Err(broadcast::error::RecvError::Closed) => break,
                },
                res = focus_rx.recv() => match res {
                    Ok(name) => {
                        if tx.send(SseEvent::default().event("focus").data(name)).await.is_err() { break; }
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => {}
                    Err(broadcast::error::RecvError::Closed) => break,
                },
                changed = clients_rx.changed() => {
                    if changed.is_err() { break; }
                    let n = *clients_rx.borrow();
                    if tx.send(SseEvent::default().event("clients").data(n.to_string())).await.is_err() { break; }
                }
            }
        }
        // The receiving half (`rx` below) only drops when the SSE response
        // itself does — tab closed, navigated away, connection dropped — so
        // this is exactly "one fewer tab talking to the server".
        count_state.client_count.send_modify(|n| *n = n.saturating_sub(1));
    });

    let stream = ReceiverStream::new(rx).map(Ok::<_, std::convert::Infallible>);
    Sse::new(stream).keep_alive(KeepAlive::default())
}

/// Current state of every live session, so a consumer that connects mid-stream
/// (e.g. the notch app on launch) sees what already exists without waiting for
/// the next transition.
async fn api_sessions(State(state): State<Arc<AppState>>) -> Json<Vec<SessionInfo>> {
    let with_pty: std::collections::HashSet<String> =
        state.sessions.lock().unwrap().keys().cloned().collect();
    let mut out: Vec<SessionInfo> = {
        let map = state.statuses.lock().unwrap();
        map.iter()
            .map(|(key, st)| {
                let has_pty = with_pty.contains(&session_key(split_key(key).0, "claude"));
                build_info(key, st, is_answerable(&map, has_pty, key))
            })
            .collect()
    };
    out.sort_by(|a, b| {
        a.sandbox
            .cmp(&b.sandbox)
            .then(a.mode.cmp(&b.mode))
            .then(a.started_ms.cmp(&b.started_ms))
            .then(a.session_id.cmp(&b.session_id))
    });
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
    // Claude Code stamps every hook event with the session it came from, so two
    // agents sharing a container get a row each instead of overwriting one.
    let session_id = body
        .get("session_id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let cwd = body
        .get("cwd")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string);

    // Who started this session, from the process tree the hook walked inside
    // the container. Only ever *upgraded* away from `Unknown`: the chain is
    // read per event, and one unreadable read must not demote a session whose
    // origin was already established.
    let strings = |field: &str| -> Vec<String> {
        body.get(field)
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()
    };
    let hook_version = body
        .get("hook_version")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);

    // The hook script is written into the container at provisioning time, so a
    // rebuilt sbxw does not reach a sandbox that is merely re-attached — and
    // the symptom (rows that never say where they came from) gives no hint of
    // the cause. Say it once per sandbox rather than on every tool call.
    if hook_version < 2 && state.stale_hooks.lock().unwrap().insert(sandbox.clone()) {
        tracing::warn!(
            "sandbox '{sandbox}' runs a status hook too old to report where its session came \
             from; falling back to the workspace path. Run `sbxw up {sandbox}` to reinstall it."
        );
    }

    let workspace = crate::workspace_for(&sandbox);
    let origin = classify_origin(&OriginEvidence {
        hook_version,
        ssh_env: &strings("ssh_env"),
        ancestry: &strings("ancestry"),
        cwd: cwd.as_deref(),
        workspace: workspace.as_ref().and_then(|p| p.to_str()),
    });

    let key = agent_status_key(&sandbox, &session_id);
    let remove = {
        let mut map = state.statuses.lock().unwrap();
        let st = map.entry(key.clone()).or_default();
        st.session_id = session_id;
        if cwd.is_some() {
            st.cwd = cwd;
        }
        if origin != SessionOrigin::Unknown {
            st.origin = origin;
        }
        apply_hook(&event, &tool, &body, st)
    };
    emit_info(&state.events, &state.statuses, &state.sessions, &key);
    if remove {
        state.statuses.lock().unwrap().remove(&key);
        // A session ending can make its sibling answerable again, so the
        // sandbox's remaining rows need re-emitting after the removal.
        emit_info(&state.events, &state.statuses, &state.sessions, &key);
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
/// Write raw bytes into a PTY. Deliberately *not* guarded the way `api_answer`
/// and `api_chat_push` are: those two infer a session ("the one that asked",
/// "the sandbox's agent") and that inference is what breaks when a container
/// runs several agents. This one infers nothing — the caller names the terminal
/// it wants — so it stays the escape hatch for driving sbxw's own PTY.
async fn api_input(State(state): State<Arc<AppState>>, Json(body): Json<InputBody>) -> StatusCode {
    let key = session_key(&body.sandbox, body.mode.as_deref().unwrap_or("claude"));
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
    let key = session_key(&body.sandbox, body.mode.as_deref().unwrap_or("claude"));
    let session = state.sessions.lock().unwrap().get(&key).cloned();
    let Some(sess) = session else {
        return StatusCode::NOT_FOUND;
    };
    // Which session's prompt is this? Enforced here and not only in the island:
    // answering types into a PTY, and with two agents in one container nothing
    // says which of them sbxw's PTY is driving (see `is_answerable`). An island
    // too old to know that would otherwise answer one session's question in the
    // other's terminal.
    let status_key = {
        let map = state.statuses.lock().unwrap();
        let keys = agent_keys_for(&map, &body.sandbox);
        let answerable: Vec<String> = keys
            .iter()
            .filter(|k| is_answerable(&map, true, k))
            .cloned()
            .collect();
        // `is_answerable` is the single rule, so the endpoint cannot drift from
        // what the island was told: exactly one row may be typed into.
        match answerable.len() {
            1 => answerable.into_iter().next().unwrap_or_default(),
            _ => {
                tracing::warn!(
                    "refusing to answer '{}': {} agent sessions share this sandbox and none is \
                     identifiably the one sbxw attached — answer in its own session",
                    body.sandbox,
                    keys.len()
                );
                return StatusCode::CONFLICT;
            }
        }
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
        .get(&status_key)
        .map(|st| st.prompt.iter().map(|q| q.options.len() as u32).collect())
        .unwrap_or_default();
    tracing::info!(
        "answer '{}': indices={:?}, sizes={:?}",
        body.sandbox,
        answers,
        sizes
    );
    for (n, index) in answers.iter().enumerate() {
        let limit = sizes.get(n).copied().unwrap_or(MAX_MENU_STEPS);
        let downs = (*index).clamp(1, limit.max(1)) - 1;
        if *index > limit {
            tracing::warn!(
                "answer '{}': step {n} index {index} exceeds {limit} known option(s) — clamped to {}",
                body.sandbox,
                downs + 1
            );
        }
        for _ in 0..downs {
            send_key(&sess, KEY_DOWN).await;
        }
        send_key(&sess, KEY_ENTER).await;
    }
    // Every question answered, the form is on its Submit tab.
    send_key(&sess, KEY_ENTER).await;
    // Optimistically clear the prompt and mark the session working again.
    if let Some(st) = state.statuses.lock().unwrap().get_mut(&status_key) {
        st.prompt.clear();
        st.state = SessionState::Working;
    }
    emit_info(&state.events, &state.statuses, &state.sessions, &status_key);
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

/// Payload of `GET /api/sandboxes/:name/policy`: three views of the same policy,
/// from three `sbx` calls, plus what sbxw itself configured.
///
/// Three because no single sbx command answers the question. `policy ls
/// <sandbox>` says which policies govern the sandbox and how many rules each
/// holds; `--wide` names the resources those rules cover; `policy log` says what
/// was actually allowed or blocked. Any one of them can fail on its own without
/// taking the panel down.
#[derive(Serialize)]
struct SandboxPolicy {
    ok: bool,
    /// Overview: one entry per policy governing this sandbox.
    policies: PolicyView,
    /// Rule-level detail (`--wide`) — the view that names domains.
    rules: PolicyView,
    /// Recent allow/deny decisions (`policy log`).
    log: PolicyView,
    /// The allow/deny lists sbxw itself applies on `up`, from `sbxw.toml`.
    /// Shown alongside the live rules because they are the part the user can
    /// actually edit — and because they still explain the sandbox's egress when
    /// `sbx policy` is unavailable entirely.
    configured_allow: Vec<String>,
    configured_deny: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

/// One parsed `sbx policy` listing.
///
/// Carries both the parsed table *and* the raw text: sbx's policy columns aren't
/// pinned by the CLI reference sbxw was built against, so when parsing comes up
/// empty the UI shows what sbx actually said instead of an implied (and
/// dangerously wrong) "no egress rules".
#[derive(Serialize, Default)]
struct PolicyView {
    /// Header names of the parsed listing; empty when it wasn't a table.
    columns: Vec<String>,
    /// What each column holds (`id`, `source`, `applies`, `summary`, `host`,
    /// `action`, …; `""` when unrecognised), so the UI can lay the listing out
    /// by meaning instead of by position. Same length as `columns`.
    roles: Vec<&'static str>,
    /// Only the rows concerning *this* sandbox (see `scope_policy_rows`).
    rows: Vec<Vec<String>>,
    /// How many rows were dropped as belonging to other sandboxes. Reported
    /// rather than silently swallowed: "3 rules" reads very differently when you
    /// know 10 were filtered out.
    other_sandboxes: usize,
    /// Rows beyond the display limit, dropped to keep the panel a panel.
    truncated: usize,
    /// True when sbx accepted the sandbox argument. False means these are the
    /// host-wide rules, which must not be presented as this sandbox's.
    sandbox_scoped: bool,
    raw: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

/// Most rows a view renders. The rule-level and log views are both unbounded in
/// principle — a global policy can hold a couple of hundred rules — and past
/// this the panel stops being glanceable. The UI says how many were dropped.
const VIEW_ROW_LIMIT: usize = 200;

/// Drop the rows of a policy listing that belong to *other* sandboxes, and
/// report how many went. Handles both shapes: `policy ls`'s `APPLIES TO`
/// (`sandbox:<name>` / `all`) and a bare `SANDBOX` column, which is normalised
/// to the former so one rule decides both.
fn scope_policy_rows(
    rows: Vec<Vec<String>>,
    roles: &[&'static str],
    sandbox: &str,
) -> (Vec<Vec<String>>, usize) {
    let col = roles
        .iter()
        .position(|r| *r == "applies")
        .map(|i| (i, false))
        .or_else(|| {
            roles
                .iter()
                .position(|r| *r == "sandbox")
                .map(|i| (i, true))
        });

    let (i, bare) = match col {
        Some(c) => c,
        // Nothing to scope on: show everything rather than guess.
        None => return (rows, 0),
    };

    let total = rows.len();
    let kept: Vec<Vec<String>> = rows
        .into_iter()
        .filter(|r| match r.get(i) {
            None => true,
            Some(cell) if bare && !cell.trim().is_empty() => {
                sbx::policy_applies_to(&format!("sandbox:{}", cell.trim()), sandbox)
            }
            Some(cell) => sbx::policy_applies_to(cell, sandbox),
        })
        .collect();

    let dropped = total - kept.len();
    (kept, dropped)
}

/// Turn one `sbx policy` listing into a `PolicyView`. Never fails outward: a
/// sbx that doesn't support the command yields a view carrying the reason, and
/// the panel omits that section instead of breaking.
fn policy_view(listed: Result<(String, bool)>, sandbox: &str) -> PolicyView {
    let (raw, sandbox_scoped) = match listed {
        Ok(v) => v,
        Err(e) => {
            return PolicyView {
                error: Some(format!("{e:#}")),
                ..Default::default()
            }
        }
    };

    let table = sbx::parse_policy_table(&raw);
    let roles = sbx::policy_column_roles(&table.columns);
    // sbx already scopes the listing when it accepts the sandbox argument; this
    // is what makes the unscoped fallback usable, and it is a no-op otherwise.
    let (mut rows, other_sandboxes) = scope_policy_rows(table.rows, &roles, sandbox);
    let truncated = rows.len().saturating_sub(VIEW_ROW_LIMIT);
    rows.truncate(VIEW_ROW_LIMIT);

    PolicyView {
        columns: table.columns,
        roles,
        rows,
        other_sandboxes,
        truncated,
        sandbox_scoped,
        raw: raw.trim_end().to_string(),
        error: None,
    }
}

/// `GET /api/sandboxes/:name/policy` — the network policy in force for one
/// sandbox: which policies govern it, the rules they hold, and what those rules
/// recently allowed or blocked.
///
/// A failure is not fatal here: whatever the other views returned is still sent,
/// down to just the `sbxw.toml` lists, so the panel always says *something*
/// truthful about this sandbox's egress.
async fn api_policy_one(
    Path(name): Path<String>,
    State(state): State<Arc<AppState>>,
) -> Json<SandboxPolicy> {
    let configured_allow = state.cfg.network_allow.clone();
    let configured_deny = state.cfg.network_deny.clone();

    // This endpoint answers with its own shape, not the `{ok, error}` envelope
    // `reject_invalid_name` produces, so it applies the same rule by hand.
    if !crate::is_valid_sandbox_name(&name) {
        return Json(SandboxPolicy {
            ok: false,
            policies: PolicyView::default(),
            rules: PolicyView::default(),
            log: PolicyView::default(),
            configured_allow,
            configured_deny,
            error: Some(crate::INVALID_NAME_MSG.to_string()),
        });
    }

    // Three shell-outs for one panel, so they share a single blocking thread
    // rather than each holding one of the pool's.
    let queried = tokio::task::spawn_blocking(move || {
        let policies = policy_view(sbx::policy_ls(&name, false), &name);
        let rules = policy_view(sbx::policy_ls(&name, true), &name);
        let log = policy_view(sbx::policy_log(&name), &name);
        (policies, rules, log)
    })
    .await;

    match queried {
        Ok((policies, rules, log)) => {
            // "ok" means the panel has something to show, not that every call
            // succeeded — each view reports its own failure.
            let ok = policies.error.is_none() || rules.error.is_none() || log.error.is_none();
            let error = if ok { None } else { policies.error.clone() };
            Json(SandboxPolicy {
                ok,
                policies,
                rules,
                log,
                configured_allow,
                configured_deny,
                error,
            })
        }
        Err(e) => Json(SandboxPolicy {
            ok: false,
            policies: PolicyView::default(),
            rules: PolicyView::default(),
            log: PolicyView::default(),
            configured_allow,
            configured_deny,
            error: Some(format!("policy lookup task failed: {e}")),
        }),
    }
}

#[derive(Deserialize)]
struct PolicyRuleBody {
    /// Resources in sbx's own syntax: `example.com`, `*.example.com`,
    /// `host:443`, or a comma-separated list of them.
    resources: String,
    /// `"allow"` (default) or `"deny"`.
    decision: Option<String>,
    /// Write the rule to the host-wide policy — every sandbox, present and
    /// future — instead of scoping it to this one. Defaults to false: the
    /// narrower scope is the one you can't regret.
    global: Option<bool>,
}

#[derive(Deserialize)]
struct PolicyRmBody {
    /// A **rule** id from the `--wide` listing, never a policy id.
    rule: String,
}

/// Longest policy argument accepted. A resource list is a handful of hostnames;
/// anything past this is a mistake or an attempt to do something else.
const POLICY_ARG_LIMIT: usize = 512;

/// Clean one policy argument, or explain why it can't be used.
///
/// sbx arguments are passed as argv (never through a shell), so the risk isn't
/// injection — it's a value starting with `-` being parsed as a *flag*, which
/// could turn `policy rm <rule>` into `policy rm --something`. Empty and
/// oversized values are rejected for the same "don't send sbx nonsense" reason.
fn clean_policy_arg(value: &str, what: &str) -> std::result::Result<String, String> {
    let v = value.trim();
    if v.is_empty() {
        return Err(format!("{what} is required"));
    }
    if v.len() > POLICY_ARG_LIMIT {
        return Err(format!(
            "{what} is too long (max {POLICY_ARG_LIMIT} characters)"
        ));
    }
    if v.starts_with('-') {
        return Err(format!(
            "{what} must not start with '-' — sbx would read it as a flag"
        ));
    }
    Ok(v.to_string())
}

/// Normalise a comma-separated resource list: trim each entry, drop empties,
/// and apply `clean_policy_arg` to every one of them. "a.com, b.com" is what a
/// person types; "a.com,b.com" is what sbx wants.
fn clean_resource_list(raw: &str) -> std::result::Result<String, String> {
    let cleaned = clean_policy_arg(raw, "resource")?;
    let parts: Vec<String> = cleaned
        .split(',')
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(|p| clean_policy_arg(p, "resource"))
        .collect::<std::result::Result<_, _>>()?;
    if parts.is_empty() {
        return Err("resource is required".into());
    }
    Ok(parts.join(","))
}

/// `POST /api/sandboxes/:name/policy/rules` — add an allow or deny network rule.
///
/// Scoped to this sandbox unless `global` is set, in which case it lands in the
/// host-wide policy and governs every sandbox. sbx's own refusal (governance,
/// org policy) is passed straight back to the browser.
async fn api_policy_add(
    Path(name): Path<String>,
    Json(body): Json<PolicyRuleBody>,
) -> Json<serde_json::Value> {
    if let Some(err) = reject_invalid_name(&name) {
        return err;
    }
    let resources = match clean_resource_list(&body.resources) {
        Ok(r) => r,
        Err(e) => return err_json(e),
    };
    let deny = match body.decision.as_deref().unwrap_or("allow") {
        "allow" => false,
        "deny" => true,
        other => return err_json(format!("unknown decision '{other}' — use allow or deny")),
    };
    let global = body.global.unwrap_or(false);

    // A rule change is a change to what the sandbox can reach: worth a line in
    // the daemon log whether or not anyone is watching the browser.
    tracing::info!(
        "web UI: adding {} network rule for {} — {resources}",
        if deny { "deny" } else { "allow" },
        if global {
            "ALL sandboxes".into()
        } else {
            format!("sandbox '{name}'")
        },
    );

    blocking_ok(move || {
        let scope = if global { None } else { Some(name.as_str()) };
        if deny {
            sbx::policy_deny_network(scope, &resources)
        } else {
            sbx::policy_allow_network(scope, &resources)
        }
    })
    .await
}

/// `POST /api/sandboxes/:name/policy/rules/rm` — remove one rule by id.
///
/// The id must come from a rule-id column (see `sbx::policy_rm_rule`); the
/// frontend only offers the button when the listing has one.
async fn api_policy_rm(
    Path(name): Path<String>,
    Json(body): Json<PolicyRmBody>,
) -> Json<serde_json::Value> {
    if let Some(err) = reject_invalid_name(&name) {
        return err;
    }
    let rule = match clean_policy_arg(&body.rule, "rule id") {
        Ok(r) => r,
        Err(e) => return err_json(e),
    };
    tracing::info!("web UI: removing policy rule {rule} (from sandbox '{name}' panel)");
    blocking_ok(move || sbx::policy_rm_rule(&rule)).await
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

/// Add or replace `alias` → `ip` in the sbxw /etc/hosts block, returning a
/// warning string if the entry didn't actually land. Reported separately from
/// the publish so a sudo/tty failure doesn't hide the port going live.
fn upsert_host_alias(alias: &str, ip: &str) -> Option<String> {
    let manual = format!("run manually: echo '{ip}\\t{alias}' | sudo tee -a /etc/hosts");
    let mut entries: Vec<HostAlias> = hosts::read_hosts_block()
        .into_iter()
        .filter(|a| a.hostname != alias)
        .collect();
    entries.push(HostAlias {
        hostname: alias.to_string(),
        ip: ip.to_string(),
    });
    if let Err(e) = hosts::sync_hosts_block(&entries) {
        return Some(format!("failed to update /etc/hosts ({e:#}) — {manual}"));
    }
    // `sync_hosts_block` reporting success isn't proof: it writes through `sudo
    // tee`, so read the block back and check the entry is really there.
    hosts::read_hosts_block()
        .iter()
        .all(|a| a.hostname != alias)
        .then(|| format!("/etc/hosts write succeeded but alias not found — {manual}"))
}

async fn api_ports_publish(
    Path(name): Path<String>,
    Json(body): Json<PublishBody>,
) -> Json<serde_json::Value> {
    blocking(
        move || {
            let host_port = body.host_port.unwrap_or(body.sandbox_port);
            let host_ip = body.host_ip.clone().unwrap_or_else(|| "127.0.0.1".into());

            // 1. Ensure the host IP exists on lo0 BEFORE sbx tries to bind to it.
            hosts::ensure_loopback_aliases(&[HostAlias {
                hostname: String::new(),
                ip: host_ip.clone(),
            }])
            .context("failed to create loopback alias — run: sudo ifconfig lo0 alias <ip> up")?;

            // 2. Publish the port now that the IP is bound.
            let spec = format!("{host_ip}:{host_port}:{}", body.sandbox_port);
            sbx::publish_port(&name, &spec)?;

            // 3. If an alias was requested, upsert it in the sbxw /etc/hosts block.
            Ok(body
                .alias
                .as_deref()
                .map(str::trim)
                .filter(|a| !a.is_empty())
                .and_then(|alias| upsert_host_alias(alias, &host_ip)))
        },
        |warning| match warning {
            None => ok_json(),
            Some(warn) => ok_json_with(serde_json::json!({ "hosts_warning": warn })),
        },
    )
    .await
}

async fn api_ports_unpublish(
    Path(name): Path<String>,
    Json(body): Json<PortSpecBody>,
) -> Json<serde_json::Value> {
    blocking_ok(move || sbx::unpublish_port(&name, &body.spec)).await
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
    blocking_ok(move || sbx::stop_sandbox(&name)).await
}

async fn api_rm(Path(name): Path<String>) -> Json<serde_json::Value> {
    // If this is a chat sandbox, resolve its throwaway temp workspace *before*
    // the sandbox goes away — that's what records the mapping — so it can be
    // deleted alongside once the removal succeeds.
    let chat_dir = crate::chat_workspace_of(&name);
    let owner = name.clone();
    blocking(
        move || sbx::rm_sandboxes(&[name.as_str()], false),
        move |()| {
            // Same cleanup as the CLI's `sbxw rm`: the OAuth kit is kept on
            // disk for sbx to re-resolve, so removal is what ends it.
            crate::forget_oauth_kit(&owner);
            if let Some(dir) = chat_dir {
                let _ = std::fs::remove_dir_all(&dir);
            }
            ok_json()
        },
    )
    .await
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
        return err_json("empty image");
    }
    let ext = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(ext_for_mime)
        .unwrap_or("png");
    // Millisecond timestamp keeps names unique and chronologically sortable.
    let dest = format!("/tmp/sbxw-pastes/paste-{}.{ext}", now_ms());
    let data = body.to_vec();
    let dest_ret = dest.clone();
    blocking(
        move || sbx::write_file_stdin(&name, &dest, &data),
        |()| ok_json_with(serde_json::json!({ "path": dest_ret })),
    )
    .await
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

    if let Some(rejected) = reject_invalid_name(&name) {
        return rejected;
    }
    if !std::path::Path::new(&path).is_dir() {
        return err_json("path is not a directory");
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
    blocking_ok(move || {
        crate::provision_sandbox(&name, &path, &[], &cfg, &extra_ports, use_api_key)
    })
    .await
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
    if let Some(rejected) = reject_invalid_name(&name) {
        return rejected;
    }

    let workspace = match crate::prepare_chat_workspace(&name) {
        Ok(w) => w,
        Err(e) => return err_json(format!("{e:#}")),
    };

    let cfg = state.cfg.clone();
    let use_api_key = state.use_api_key;
    let name_ret = name.clone();
    tracing::info!("web UI: provisioning chat sandbox '{name}' at {workspace}");
    blocking(
        move || crate::provision_sandbox(&name, &workspace, &[], &cfg, &[], use_api_key),
        |()| ok_json_with(serde_json::json!({ "name": name_ret })),
    )
    .await
}

/// Base name for the island's "ephemeral chat" sandboxes: a scratch agent
/// that's always one keystroke away, with no project mounted.
///
/// The first one takes this name bare; asking for another mints
/// `ephemeral-chat-2`, `-3`, … (see `next_ephemeral_chat`). Sending *into* an
/// existing chat is a different gesture — it names the sandbox — so a follow-up
/// question still lands in the same conversation.
const EPHEMERAL_CHAT: &str = "ephemeral-chat";

/// How long a *cold* PTY must stay silent before we accept that the agent's TUI
/// has finished drawing and won't swallow what we type into a half-painted
/// screen. Generous, because a first frame arrives in fits and starts.
const CHAT_SETTLE: Duration = Duration::from_millis(900);

/// The same, for a session that is already attached and drawn. Nothing is
/// booting, so this is only guarding against landing mid-redraw — and it is
/// paid on *every* message, which is what made chatting on into an existing
/// sandbox feel sluggish at the cold-start figure.
const CHAT_SETTLE_WARM: Duration = Duration::from_millis(180);

/// Upper bound on waiting for a cold TUI: a cold sandbox has to boot the agent
/// first.
const CHAT_READY_TIMEOUT: Duration = Duration::from_secs(45);

/// How long to wait for the typed message to start echoing back. Short: the
/// bytes are already written, so this is a round-trip through the PTY, not a
/// boot. Capped so a session that echoes nothing at all (a dead PTY) costs a
/// blink rather than `CHAT_READY_TIMEOUT`.
const CHAT_ECHO_TIMEOUT: Duration = Duration::from_secs(3);

/// Silence that ends the echo of a typed message, before Return is sent as its
/// own keystroke.
///
/// Deliberately looser than `CHAT_SETTLE_WARM`: this is the step that breaks
/// quietly. Call it too early and Return joins the paste, which Claude Code
/// inserts into the message instead of sending it — the text sits in the box and
/// nothing happens. A long message echoes in chunks, so the window has to absorb
/// a stutter between them, not just the round-trip.
const CHAT_ECHO_QUIET: Duration = Duration::from_millis(350);

#[derive(Deserialize)]
struct ChatPushBody {
    /// Message to submit to the chat agent.
    text: String,
    /// Sandbox to chat in. Defaults to the shared ephemeral one.
    #[serde(default)]
    name: Option<String>,
    /// Send this into a brand-new ephemeral chat sandbox instead, named by the
    /// daemon (`ephemeral-chat`, `ephemeral-chat-2`, …). Ignored when `name`
    /// says where the message goes.
    #[serde(default)]
    fresh: bool,
}

/// The name a brand-new ephemeral chat should take: `EPHEMERAL_CHAT`, else the
/// first free `ephemeral-chat-N` counting from 2.
///
/// Numbering is by availability, not by a high-water mark: chat sandboxes are
/// disposable, so removing `-2` and asking for another chat should reuse that
/// name rather than climb forever. `taken` holds every sandbox name, not just
/// the chat ones — the answer has to be free for *any* sandbox.
fn next_ephemeral_chat(taken: &std::collections::HashSet<String>) -> String {
    if !taken.contains(EPHEMERAL_CHAT) {
        return EPHEMERAL_CHAT.to_string();
    }
    (2..)
        .map(|n| format!("{EPHEMERAL_CHAT}-{n}"))
        .find(|candidate| !taken.contains(candidate))
        .unwrap_or_else(|| EPHEMERAL_CHAT.to_string())
}

/// Wait until the PTY stops producing output, so a prompt isn't typed into a
/// TUI that is still painting (Claude Code redraws its whole frame on startup
/// and would drop the keystrokes).
///
/// Quiescence is measured on the broadcast channel rather than the replay ring
/// buffer: that buffer is capacity-bounded, so once full its length stops
/// changing and silence becomes indistinguishable from a flood.
///
/// `rx` is passed in rather than subscribed here so a caller can subscribe
/// *before* the write it is about to wait on. Doing it the other way round
/// loses the race: the echo can land between the write and the subscription,
/// leaving `first` waiting for output that has already been and gone.
///
/// `first: Some(bound)` waits for the initial byte (up to `bound`) before
/// timing any silence — for a session that has yet to emit anything, where
/// timing silence from now would "settle" instantly. `None` starts the clock
/// immediately, which is what an already-drawn, possibly perfectly quiet
/// session needs.
async fn settle(rx: &mut broadcast::Receiver<Vec<u8>>, first: Option<Duration>, quiet: Duration) {
    if let Some(bound) = first {
        if tokio::time::timeout(bound, rx.recv()).await.is_err() {
            return;
        }
    }
    let deadline = tokio::time::Instant::now() + CHAT_READY_TIMEOUT;
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(quiet, rx.recv()).await {
            Err(_) => return,      // quiet for long enough — the TUI is ready
            Ok(Ok(_)) => continue, // still drawing
            Ok(Err(_)) => return,  // lagged or closed; don't block on it
        }
    }
}

/// `POST /api/chat/push` — submit a message to a chat agent, creating the
/// sandbox and attaching its session first if they don't exist.
///
/// This is the island composer's one call: it has no sandbox picker and no
/// terminal, so everything between "user typed a question" and "the agent is
/// reading it" has to happen here. Reuses the same provisioning path as the web
/// UI's 💬 button, so a chat started from the island is the same thing as one
/// started from the browser.
///
/// Three shapes of request, and only the first is expected to be slow:
///
///  - `fresh: true` — mint a new `ephemeral-chat[-N]`: provision, boot, type.
///  - `name: "<sandbox>"` — a row's inline composer typing into a sandbox that
///    is, almost always, already attached. This is the hot path.
///  - neither — the legacy island build's shared `ephemeral-chat`.
async fn api_chat_push(
    State(state): State<Arc<AppState>>,
    Json(body): Json<ChatPushBody>,
) -> Json<serde_json::Value> {
    let text = body.text.trim().to_string();
    if text.is_empty() {
        return err_json("empty message");
    }
    let named = body
        .name
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    // Whether this request is allowed to *create* the sandbox it targets — only
    // when the daemon chose the name. "Write to this sandbox" must never be a way
    // to conjure one: a row whose sandbox was removed a moment ago would
    // otherwise silently rebuild it under the same name with an empty throwaway
    // workspace, none of the project it is named after.
    let (name, may_create) = match named {
        Some(n) => (n, false),
        None if body.fresh => {
            let taken = tokio::task::spawn_blocking(crate::sbx::list_sandboxes)
                .await
                .unwrap_or_default()
                .into_iter()
                .map(|s| s.name)
                .collect();
            (next_ephemeral_chat(&taken), true)
        }
        // What an island build predating per-row composers sends.
        None => (EPHEMERAL_CHAT.to_string(), true),
    };
    if let Some(rejected) = reject_invalid_name(&name) {
        return rejected;
    }
    // Same question as answering a prompt (see `is_answerable`): this types
    // into the PTY sbxw holds for the sandbox, so "the sandbox's agent" has to
    // name exactly one session. Several are fine as long as one of them is the
    // tty's — that is the one this PTY drives. Zero is fine too: the ordinary
    // case of a sandbox whose agent this endpoint is about to bring up.
    {
        let map = state.statuses.lock().unwrap();
        let agents = agent_keys_for(&map, &name);
        let from_tty = agents
            .iter()
            .filter(|k| {
                map.get(*k)
                    .is_some_and(|st| st.origin == SessionOrigin::Tty)
            })
            .count();
        if agents.len() > 1 && from_tty != 1 {
            return err_json(format!(
                "'{name}' is running {} agent sessions and none is identifiably sbxw's own, \
                 so sbxw cannot tell which one you mean — type in the session itself",
                agents.len()
            ));
        }
    }
    let key = session_key(&name, "claude");

    // 1. Provision on first use only. Re-provisioning per message would redo
    //    policy, hooks and trust on every keystroke-worth of chat.
    //
    //    An attached, still-running session is proof enough that the sandbox is
    //    there, and it lets the common case skip `sbx ls` — a process spawn that
    //    was the single most expensive step of typing into a chat already up.
    let attached = state
        .sessions
        .lock()
        .unwrap()
        .get(&key)
        .is_some_and(|s| s.alive());
    let exists = attached || {
        let n = name.clone();
        try_blocking(move || crate::sbx::exists(&n))
            .await
            .unwrap_or(false)
    };
    if !exists {
        if !may_create {
            return err_json(format!(
                "no sandbox named '{name}' — it may have been removed"
            ));
        }
        let workspace = match crate::prepare_chat_workspace(&name) {
            Ok(w) => w,
            Err(e) => return err_json(format!("{e:#}")),
        };
        let cfg = state.cfg.clone();
        let use_api_key = state.use_api_key;
        let n = name.clone();
        tracing::info!("island: provisioning ephemeral chat sandbox '{name}' at {workspace}");
        if let Err(rejected) = try_blocking(move || {
            crate::provision_sandbox(&n, &workspace, &[], &cfg, &[], use_api_key)
        })
        .await
        {
            return rejected;
        }
    }

    // 2. Attach the agent. `sbx run --name` also starts a stopped sandbox, so
    //    this covers "the chat sandbox exists but was stopped" for free.
    //
    //    Drop whatever is filed under this key unless it is the live session we
    //    just proved: nothing prunes one whose PTY has exited (see
    //    `PtySession::alive`), and `get_or_create_session` hands back what it
    //    finds. Typing into that corpse loses the message — and costs
    //    `CHAT_READY_TIMEOUT` first, waiting for a first frame that can never
    //    come. Also covers a recycled chat name: `ephemeral-chat-2` removed, then
    //    minted again, with the old session still in the map.
    if !attached {
        state.sessions.lock().unwrap().remove(&key);
    }
    let sessions = state.sessions.clone();
    let cfg = state.cfg.clone();
    let n = name.clone();
    let session =
        match try_blocking(move || get_or_create_session(&n, "claude", &cfg, &sessions)).await {
            Ok(s) => s,
            Err(rejected) => return rejected,
        };

    // 3. Let the TUI finish drawing, then type the message. Subscribe first: the
    //    same receiver carries the echo in step 4, and one subscribed after the
    //    write could miss it (see `settle`).
    let mut rx = session.tx.subscribe();
    if attached {
        settle(&mut rx, None, CHAT_SETTLE_WARM).await;
    } else {
        // Nothing has been drawn yet — wait for the first frame, then for it to
        // stop.
        settle(&mut rx, Some(CHAT_READY_TIMEOUT), CHAT_SETTLE).await;
    }
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
    //
    //    Waiting for the echo to *start* before timing its silence is what keeps
    //    this honest at a short quiet window: without it, a PTY that hasn't
    //    turned the write around yet reads as quiet, and Return joins the paste.
    settle(&mut rx, Some(CHAT_ECHO_TIMEOUT), CHAT_ECHO_QUIET).await;
    {
        let mut w = session.writer.lock().unwrap();
        let _ = w.write_all(KEY_ENTER);
        let _ = w.flush();
    }
    tracing::info!("island: pushed {} chars into '{name}'", text.len());

    ok_json_with(serde_json::json!({ "name": name, "created": !exists }))
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
    if let Some(rejected) = reject_invalid_name(&new_name) {
        return rejected;
    }

    let Some(workspace) = crate::workspace_for(&name) else {
        return err_json(format!(
            "no known workspace for '{name}' — it may predate this sbxw version"
        ));
    };
    let workspace = workspace.to_string_lossy().into_owned();

    match sbx::exists(&new_name) {
        Ok(true) => return err_json(format!("a sandbox named '{new_name}' already exists")),
        Err(e) => return err_json(format!("{e:#}")),
        Ok(false) => {}
    }

    let cfg = state.cfg.clone();
    let use_api_key = state.use_api_key;
    tracing::info!("web UI: duplicating sandbox '{name}' as '{new_name}' (workspace {workspace})");
    blocking_ok(move || {
        crate::provision_sandbox(&new_name, &workspace, &[], &cfg, &[], use_api_key)
    })
    .await
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

fn read_fs_dir(params: FsQuery) -> FsResponse {
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

    FsResponse {
        path: dir.to_string_lossy().into_owned(),
        parent,
        entries,
    }
}

// Directory reads (`canonicalize`, `read_dir`) are synchronous syscalls; run them
// on the blocking pool so they can't stall the runtime's worker threads (only 2 —
// see `#[tokio::main]`) out from under every other client's SSE stream and PTY
// bridge when several web UI tabs are open at once.
async fn api_fs(Query(params): Query<FsQuery>) -> Json<FsResponse> {
    Json(
        tokio::task::spawn_blocking(move || read_fs_dir(params))
            .await
            .unwrap_or(FsResponse {
                path: String::new(),
                parent: None,
                entries: Vec::new(),
            }),
    )
}

/// `POST /api/fs/pick` — pops the OS-native folder picker (Finder on macOS,
/// the Explorer folder browser on Windows, zenity/kdialog on Linux) and
/// returns the chosen absolute path. Runs on a blocking thread since the
/// dialog blocks until the user responds.
async fn api_fs_pick() -> Json<serde_json::Value> {
    blocking(pick_folder_native, |picked| match picked {
        Some(path) => ok_json_with(serde_json::json!({ "path": path })),
        // Dismissing the dialog isn't an error, but it isn't a path either — the
        // frontend distinguishes the two to stay silent on cancel.
        None => Json(serde_json::json!({ "ok": false, "cancelled": true })),
    })
    .await
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
    out.sort_by_key(|e| std::cmp::Reverse(e.modified)); // newest first
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
    // "bash" → shell session; "monitor" → the host dashboard, which ignores the
    // sandbox parameter entirely; anything else → the agent ("claude").
    let mode = match params.mode.as_deref() {
        Some("bash") => "bash",
        Some(MONITOR_KEY_MODE) => MONITOR_KEY_MODE,
        _ => "claude",
    }
    .to_string();
    let sandbox = if mode == MONITOR_KEY_MODE {
        MONITOR_SANDBOX.to_string()
    } else {
        params
            .sandbox
            .unwrap_or_else(|| state.initial_sandbox.clone())
    };
    ws.on_upgrade(move |socket| {
        handle_socket(
            socket,
            sandbox,
            mode,
            state.cfg.clone(),
            state.sessions.clone(),
        )
    })
}

async fn handle_socket(
    socket: WebSocket,
    sandbox: String,
    mode: String,
    cfg: Arc<Config>,
    sessions: Sessions,
) {
    if let Err(e) = bridge(socket, sandbox, mode, cfg, sessions).await {
        tracing::warn!("tty bridge ended: {e:#}");
    }
}

/// Return the existing PTY session for (`sandbox`, `mode`), or create one.
/// Sessions are keyed by "<sandbox>::<mode>" so the agent ("claude") and a
/// bash shell coexist independently for the same sandbox.
///   mode == "bash"   → `sbx exec -it <sandbox> -- bash`, or SSH if it's stopped
///   mode == "claude" → `sbx run --name <sandbox>` (or the configured web_shell via exec)
///   mode == "monitor"→ `cfg.monitor_cmd` on the **host**, under `MONITOR_KEY`
/// The session lives until the PTY process exits.
fn get_or_create_session(
    sandbox: &str,
    mode: &str,
    cfg: &Config,
    sessions: &Sessions,
) -> Result<Arc<PtySession>> {
    let key = session_key(sandbox, mode);

    // Fast path: session already exists.
    if let Some(s) = sessions.lock().unwrap().get(&key) {
        return Ok(s.clone());
    }

    // The monitor runs on the host and watches every sandbox, so it is neither
    // created from nor scoped to one. Everything below that touches `sandbox`
    // would be meaningless (or wrong) for it, hence the early split.
    if mode == MONITOR_KEY_MODE {
        if cfg.monitor_cmd.is_empty() {
            bail!("no monitor command configured — set `monitor_cmd` in sbxw.toml");
        }
        let mut cmd = CommandBuilder::new(&cfg.monitor_cmd[0]);
        cmd.args(&cfg.monitor_cmd[1..]);
        return spawn_pty_session(cmd, key, sessions);
    }

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

    let cmd = if bash_over_ssh {
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
    } else if cfg.web_shell.is_empty() {
        // Re-attach by name. The positional form (`sbx run <name>`) is
        // deprecated; `--name` re-attaches regardless of working directory.
        // Since sbx 0.35 this also works for sandboxes created with a custom
        // --kit (like sbxw's OAuth kit) without re-passing the kit.
        let mut c = CommandBuilder::new("sbx");
        c.args(["run", "--name", sandbox]);
        c
    } else {
        let mut c = CommandBuilder::new("sbx");
        c.args(["exec", "-it", sandbox, "--", &cfg.web_shell]);
        c
    };

    spawn_pty_session(cmd, key, sessions)
}

/// Open a PTY, run `cmd` in it, and register the session under `key`.
///
/// Shared by every mode: the sandbox panes and the host monitor differ only in
/// which command they launch, and everything after that — the replay ring, the
/// broadcast channel, the debounced bell, the reader thread that unregisters
/// the session on exit — is identical.
fn spawn_pty_session(
    mut cmd: CommandBuilder,
    key: String,
    sessions: &Sessions,
) -> Result<Arc<PtySession>> {
    let pty = native_pty_system();
    let pair = pty.openpty(PtySize {
        rows: 30,
        cols: 100,
        pixel_width: 0,
        pixel_height: 0,
    })?;

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
        child: Mutex::new(child),
    });

    sessions
        .lock()
        .unwrap()
        .insert(key.clone(), session.clone());

    // Background reader thread — pure terminal I/O: PTY output → replay buffer →
    // WebSocket broadcast, plus a debounced BEL signal for the browser's
    // "attention" toast. Session *state* is derived from Claude Code hooks
    // (`api_hook`), not from this stream, so there is no scraping here.
    let sessions_ref = sessions.clone();
    let sandbox_key = key.clone();
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
    cfg: Arc<Config>,
    sessions: Sessions,
) -> Result<()> {
    let (mut ws_tx, mut ws_rx) = socket.split();

    // Get or create the session on a blocking thread (PTY setup does syscalls).
    let session = tokio::task::spawn_blocking({
        let sandbox = sandbox.clone();
        let mode = mode.clone();
        let cfg = cfg.clone();
        let sessions = sessions.clone();
        move || get_or_create_session(&sandbox, &mode, &cfg, &sessions)
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
    let key = session_key(&sandbox, &mode);

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
                        let key = key.clone();
                        let _ = tokio::task::spawn_blocking(move || {
                            if let Ok(m) = m.lock() {
                                let before = m.get_size().ok();
                                let res = m.resize(PtySize {
                                    rows,
                                    cols,
                                    pixel_width: 0,
                                    pixel_height: 0,
                                });
                                // The one place the browser's idea of the
                                // terminal size meets the PTY's, so a TUI drawn
                                // to the wrong width can be pinned on one side
                                // or the other (`/api/ptys` reads the same
                                // truth on demand). Only a size that actually
                                // moved is worth a line: clients re-announce
                                // freely, and those are no-ops the kernel does
                                // not even signal.
                                let moved = before.is_none_or(|b| b.cols != cols || b.rows != rows);
                                match res {
                                    Ok(()) if moved => tracing::info!(
                                        "resize {key}: {} → {cols}x{rows}",
                                        before.map_or_else(
                                            || "?".to_string(),
                                            |b| format!("{}x{}", b.cols, b.rows)
                                        )
                                    ),
                                    Ok(()) => {}
                                    Err(e) => {
                                        tracing::warn!("resize {key} to {cols}x{rows} failed: {e}")
                                    }
                                }
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
        let info = build_info("box::claude", &st, true);
        assert_eq!(info.question.expect("first step").text, "Quel thème ?");
        assert_eq!(info.steps.expect("all steps").len(), 2);
    }

    /// One container can hold several agents — sbxw's own PTY session, plus
    /// anything attached over SSH. They must not share a slot.
    #[test]
    fn two_agents_in_one_sandbox_get_a_key_each() {
        let a = agent_status_key("neos", "5f2c-aaaa");
        let b = agent_status_key("neos", "9e10-bbbb");
        assert_ne!(a, b);

        // The sandbox and the mode still read out of both, which is what the
        // reconciler and every consumer index on.
        for key in [&a, &b] {
            assert_eq!(split_key(key), ("neos", "claude"));
        }

        // A hook script too old to send a session id keeps the historical key.
        assert_eq!(agent_status_key("neos", ""), "neos::claude");
        assert_eq!(split_key("neos::claude"), ("neos", "claude"));
        // And a bash session is untouched by any of this.
        assert_eq!(split_key("neos::bash"), ("neos", "bash"));
    }

    /// Where a session came from, decided on evidence rather than assumed.
    /// Every chain below is copied from a real `/api/hook/log`.
    #[test]
    fn a_client_is_recognised_by_the_process_it_runs_in_the_container() {
        let strs = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let ev = |ssh: &[&str], ancestry: &[&str], cwd, workspace| {
            classify_origin(&OriginEvidence {
                hook_version: 2,
                ssh_env: &strs(ssh),
                ancestry: &strs(ancestry),
                cwd,
                workspace,
            })
        };

        let ws = Some("/Users/thomas/Desktop");

        // The agent sbxw attached: the claude CLI, in the workspace.
        assert_eq!(ev(&[], &["node", "claude"], ws, ws), SessionOrigin::Tty);

        // Claude Desktop: it runs its own server in the container and sets no
        // sshd environment whatsoever, so `ssh_env` is empty for both — the
        // ancestry is what gives it away.
        assert_eq!(
            ev(
                &[],
                &["node", "2.1.222", "server"],
                Some("/home/agent/workspace"),
                ws
            ),
            SessionOrigin::Remote
        );
        // …and it stays recognised even sitting in the workspace, which the cwd
        // test alone could not have caught.
        assert_eq!(
            ev(&[], &["node", "2.1.222", "server"], ws, ws),
            SessionOrigin::Remote
        );

        // Someone arriving by hand on <name>.sbx: sshd, either way it shows.
        assert_eq!(
            ev(&["SSH_CONNECTION"], &["node", "claude"], ws, ws),
            SessionOrigin::Remote
        );
        assert_eq!(
            ev(&[], &["node", "claude", "sshd"], ws, ws),
            SessionOrigin::Remote
        );

        // ssh-agent forwards keys, it does not start sessions — and sbx sets
        // SSH_AUTH_SOCK in every sandbox, which is why it is not in `ssh_env`.
        assert_eq!(
            ev(&[], &["node", "claude", "ssh-agent"], ws, ws),
            SessionOrigin::Tty
        );

        // The pre-`hook_version` sandboxes in the same log, carrying no markers
        // at all: the working directory still separates them.
        let old = |ancestry: &[&str], cwd| {
            classify_origin(&OriginEvidence {
                hook_version: 0,
                ssh_env: &[],
                ancestry: &strs(ancestry),
                cwd,
                workspace: ws,
            })
        };
        assert_eq!(old(&["node", "claude"], ws), SessionOrigin::Tty);
        assert_eq!(
            old(&[], Some("/home/agent/workspace")),
            SessionOrigin::Remote
        );
    }

    /// Silence is not evidence: a hook too old to look must not be read as
    /// "not over SSH", or every foreign session is promoted to answerable.
    #[test]
    fn a_hook_that_could_not_look_says_unknown_not_tty() {
        let none: Vec<String> = vec![];
        let old = OriginEvidence {
            hook_version: 0,
            ssh_env: &none,
            ancestry: &none,
            cwd: None,
            workspace: None,
        };
        assert_eq!(classify_origin(&old), SessionOrigin::Unknown);

        // A hook that looked and found nothing has said something.
        let looked = OriginEvidence {
            hook_version: 2,
            ..old
        };
        assert_eq!(classify_origin(&looked), SessionOrigin::Tty);
    }

    /// Sandboxes provisioned before the hook reported its ancestry — which is
    /// every sandbox already running when this shipped — are classified by
    /// where the session sits instead. Taken from a real `/api/hook/log`:
    /// sbxw's own agent starts in the workspace the sandbox was created with.
    #[test]
    fn a_hook_without_ancestry_falls_back_to_the_recorded_workspace() {
        let workspace = "/Users/thomas/Downloads/sbxw 2";
        assert_eq!(
            classify_origin_by_cwd(Some(workspace), Some(workspace)),
            SessionOrigin::Tty
        );
        // A trailing slash is the same directory recorded twice, not a
        // different one.
        assert_eq!(
            classify_origin_by_cwd(
                Some("/Users/thomas/Desktop/"),
                Some("/Users/thomas/Desktop")
            ),
            SessionOrigin::Tty
        );
        // Where Claude Desktop's SSH session landed in the real log.
        assert_eq!(
            classify_origin_by_cwd(Some("/home/agent/workspace"), Some(workspace)),
            SessionOrigin::Remote
        );
        // Nothing to compare against says nothing.
        assert_eq!(
            classify_origin_by_cwd(None, Some(workspace)),
            SessionOrigin::Unknown
        );
        assert_eq!(
            classify_origin_by_cwd(Some(workspace), None),
            SessionOrigin::Unknown
        );
        assert_eq!(
            classify_origin_by_cwd(Some(""), Some("")),
            SessionOrigin::Unknown
        );
    }

    /// The fallback's unsafe direction must be impossible: an SSH session can
    /// only ever be *mistaken for the tty's* by sitting in the workspace, and
    /// then both do, which `is_answerable` refuses outright.
    #[test]
    fn an_ssh_session_that_cds_into_the_workspace_locks_the_sandbox_instead() {
        let workspace = "/Users/thomas/Downloads/sbxw 2";
        let mut map: HashMap<String, SessionStatus> = HashMap::new();
        for (sid, cwd) in [("aaaa", workspace), ("bbbb", workspace)] {
            map.insert(
                agent_status_key("neos", sid),
                SessionStatus {
                    origin: classify_origin_by_cwd(Some(cwd), Some(workspace)),
                    ..Default::default()
                },
            );
        }
        for sid in ["aaaa", "bbbb"] {
            assert!(!is_answerable(&map, true, &agent_status_key("neos", sid)));
        }
    }

    /// Answering types into a PTY, so it is offered only when sbxw's PTY is
    /// unambiguously the terminal that asked.
    #[test]
    fn a_prompt_is_answerable_only_when_one_agent_owns_the_terminal() {
        let mut map: HashMap<String, SessionStatus> = HashMap::new();
        let mine = agent_status_key("neos", "aaaa");
        map.insert(mine.clone(), SessionStatus::default());

        // The ordinary case: one agent, one PTY.
        assert!(is_answerable(&map, true, &mine));
        // No PTY (the browser terminal is closed): nothing to type into.
        assert!(!is_answerable(&map, false, &mine));

        // Claude Desktop attaches over SSH and starts a second agent. Both
        // origins are known, so the tty's row keeps its buttons and the SSH
        // one — whose terminal sbxw does not hold — does not.
        let theirs = agent_status_key("neos", "bbbb");
        map.insert(
            mine.clone(),
            SessionStatus {
                origin: SessionOrigin::Tty,
                ..Default::default()
            },
        );
        map.insert(
            theirs.clone(),
            SessionStatus {
                origin: SessionOrigin::Remote,
                ..Default::default()
            },
        );
        assert!(is_answerable(&map, true, &mine));
        assert!(!is_answerable(&map, true, &theirs));

        // Without that evidence the pair is unreadable again, and *neither*
        // may be answered: an unknown origin never stands in for the tty.
        map.insert(mine.clone(), SessionStatus::default());
        assert!(!is_answerable(&map, true, &mine));
        assert!(!is_answerable(&map, true, &theirs));

        // Two sessions both claiming the tty is equally unusable — sbxw holds
        // one PTY and cannot hand it to two.
        for k in [&mine, &theirs] {
            map.insert(
                k.to_string(),
                SessionStatus {
                    origin: SessionOrigin::Tty,
                    ..Default::default()
                },
            );
        }
        assert!(!is_answerable(&map, true, &mine));
        assert!(!is_answerable(&map, true, &theirs));

        // A single session is answerable whatever its origin: there is only one
        // terminal it could be.
        map.remove(&theirs);
        map.insert(mine.clone(), SessionStatus::default());
        assert!(is_answerable(&map, true, &mine));

        // A sandbox next door is a different question entirely.
        map.insert(agent_status_key("other", "cccc"), SessionStatus::default());
        assert!(is_answerable(&map, true, &mine));

        // A shell is never answerable: there is no prompt to answer.
        map.insert(session_key("neos", "bash"), SessionStatus::default());
        assert!(!is_answerable(&map, true, &session_key("neos", "bash")));
        assert!(is_answerable(&map, true, &mine));
    }

    /// The island needs to tell two rows of one sandbox apart, and the cwd is
    /// what does it — the same string Claude Code scopes its transcripts by.
    #[test]
    fn a_session_carries_its_identity_to_the_island() {
        let st = SessionStatus {
            session_id: "5f2c-aaaa".into(),
            cwd: Some("/Users/you/src/neos".into()),
            ..Default::default()
        };
        let info = build_info(&agent_status_key("neos", "5f2c-aaaa"), &st, false);
        assert_eq!(info.sandbox, "neos");
        assert_eq!(info.mode, "claude");
        assert_eq!(info.session_id.as_deref(), Some("5f2c-aaaa"));
        assert_eq!(info.cwd.as_deref(), Some("/Users/you/src/neos"));
        assert!(!info.answerable);

        // An older in-sandbox hook sends no session id; the field is then
        // absent from the wire rather than present and empty.
        let plain = build_info("neos::claude", &SessionStatus::default(), true);
        assert!(plain.session_id.is_none());
        assert!(plain.answerable);
        let json = serde_json::to_value(&plain).unwrap();
        assert!(json.get("session_id").is_none(), "{json}");
        assert_eq!(json.get("answerable").and_then(|v| v.as_bool()), Some(true));
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

    /// A resource starting with `-` would be parsed by sbx as a flag, so it is
    /// refused rather than passed through — before and after the comma split.
    #[test]
    fn policy_args_reject_flag_lookalikes_and_empties() {
        assert!(clean_policy_arg("--sandbox", "rule id").is_err());
        assert!(clean_policy_arg("   ", "rule id").is_err());
        assert!(clean_policy_arg(&"x".repeat(POLICY_ARG_LIMIT + 1), "rule id").is_err());
        assert_eq!(clean_policy_arg(" r-0005 ", "rule id").unwrap(), "r-0005");

        assert!(clean_resource_list("github.com,--force").is_err());
        assert!(clean_resource_list(" , , ").is_err());
    }

    /// What a person types versus what sbx wants.
    #[test]
    fn resource_lists_are_normalised_for_sbx() {
        assert_eq!(
            clean_resource_list(" github.com , *.npmjs.org ,, host:443 ").unwrap(),
            "github.com,*.npmjs.org,host:443"
        );
    }

    /// The frontend branches on `ok` alone, so every envelope must carry it —
    /// and a success with extra fields must not lose them to the merge.
    #[test]
    fn json_envelopes_keep_their_shape() {
        assert_eq!(ok_json().0, json!({ "ok": true }));
        assert_eq!(
            ok_json_with(json!({ "name": "chat-01", "created": true })).0,
            json!({ "ok": true, "name": "chat-01", "created": true })
        );
        assert_eq!(
            err_json("path is not a directory").0,
            json!({ "ok": false, "error": "path is not a directory" })
        );
    }

    /// `sbx`'s stderr reaches the user only if the whole context chain is
    /// rendered — `to_string()` would show just the outermost layer and drop the
    /// policy/governance detail `sbx::command_error` went to the trouble of
    /// keeping.
    #[test]
    fn err_json_renders_the_whole_anyhow_context_chain() {
        let err = anyhow::anyhow!("Blocked by network policy: domain example.com")
            .context("failed to apply network allowlist");
        let body = err_json(format!("{err:#}")).0;
        let msg = body["error"].as_str().expect("error string");
        assert!(msg.contains("failed to apply network allowlist"), "{msg}");
        assert!(msg.contains("Blocked by network policy"), "{msg}");
    }

    #[test]
    fn reject_invalid_name_accepts_valid_and_explains_invalid() {
        assert!(reject_invalid_name("neos-2").is_none());
        let rejected = reject_invalid_name("bad name!").expect("rejected");
        assert_eq!(rejected.0["ok"], json!(false));
        assert_eq!(rejected.0["error"], json!(crate::INVALID_NAME_MSG));
    }

    #[test]
    fn session_keys_round_trip() {
        let key = session_key("neos", "bash");
        assert_eq!(key, "neos::bash");
        assert_eq!(split_key(&key), ("neos", "bash"));
    }

    #[test]
    fn clip_truncates_and_single_lines() {
        assert_eq!(clip("hello", 10), "hello");
        assert_eq!(clip("first line\nsecond", 40), "first line");
        assert_eq!(clip("abcdefghij", 5), "abcde…");
    }

    fn taken(names: &[&str]) -> std::collections::HashSet<String> {
        names.iter().map(|n| n.to_string()).collect()
    }

    #[test]
    fn next_ephemeral_chat_numbers_from_two() {
        assert_eq!(next_ephemeral_chat(&taken(&[])), "ephemeral-chat");
        // Unrelated sandboxes don't push the numbering along.
        assert_eq!(next_ephemeral_chat(&taken(&["neos"])), "ephemeral-chat");
        assert_eq!(
            next_ephemeral_chat(&taken(&["ephemeral-chat"])),
            "ephemeral-chat-2"
        );
        assert_eq!(
            next_ephemeral_chat(&taken(&["ephemeral-chat", "ephemeral-chat-2"])),
            "ephemeral-chat-3"
        );
    }

    /// Chat sandboxes are disposable, so a name freed by `sbx rm` is offered
    /// again instead of the counter climbing past it forever.
    #[test]
    fn next_ephemeral_chat_reuses_a_freed_name() {
        assert_eq!(
            next_ephemeral_chat(&taken(&["ephemeral-chat", "ephemeral-chat-3"])),
            "ephemeral-chat-2"
        );
    }

    /// Every minted name has to survive the same validation the endpoint applies
    /// to a caller-supplied one.
    #[test]
    fn next_ephemeral_chat_names_are_valid_sandbox_names() {
        let mut used = taken(&[]);
        for _ in 0..12 {
            let name = next_ephemeral_chat(&used);
            assert!(crate::is_valid_sandbox_name(&name), "{name}");
            assert!(used.insert(name), "minted a name twice");
        }
    }
}
