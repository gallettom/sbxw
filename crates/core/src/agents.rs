//! Claude Code sessions as their hooks report them.
//!
//! Session state comes from Claude Code's own lifecycle hooks, POSTed by a
//! small script sbxw installs in every sandbox — never from scraping a
//! terminal. This module folds those events into one record per session and
//! decides the one question with a safety edge: **may sbxw type an answer into
//! this session's terminal?**
//!
//! | Hook event | State |
//! |---|---|
//! | `SessionStart` | idle |
//! | `UserPromptSubmit` | working (captures the prompt) |
//! | `PreToolUse` `AskUserQuestion` | attention + the structured question |
//! | `PreToolUse` / `PostToolUse` | working (the tool is the activity) |
//! | `Notification` [`SUMMONS_NOTIFICATIONS`] or untyped | attention |
//! | `Notification` `idle_prompt` | idle |
//! | `Notification` any other type | unchanged (a notice) |
//! | a subagent's (`agent_id`) `PreToolUse` / `PostToolUse` | unchanged, but answers a permission prompt |
//! | `Stop` | idle (captures the reply) |
//! | `Stop` listing background work of its own | working, [`Background`] set, until a turn or [`BACKGROUND_MAX_MS`] |
//! | `SessionEnd` | exited, dropped |

use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Default, Debug)]
#[serde(rename_all = "lowercase")]
pub enum SessionState {
    #[default]
    Working,
    Idle,
    Attention,
    Exited,
}

/// Where a session was started from.
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Default, Debug)]
#[serde(rename_all = "lowercase")]
pub enum SessionOrigin {
    /// Attached by sbxw, through the PTY behind the browser terminal.
    Tty,
    /// Driven by a client: Claude Desktop, an editor, a shell on `<name>.sbx`.
    Remote,
    /// The hook could not tell, or is too old to have looked.
    #[default]
    Unknown,
}

/// A prompt the agent is waiting on, from `AskUserQuestion`'s structured input.
#[derive(Clone, PartialEq, Serialize, Debug)]
pub struct Question {
    pub text: String,
    pub options: Vec<String>,
    /// "label — description" per option: the decision table.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub context: Vec<String>,
}

/// The rich payload of `/api/events` and `/api/sessions`. A wire contract with
/// the island and the web UI.
#[derive(Clone, Serialize, Debug, PartialEq)]
pub struct SessionInfo {
    pub sandbox: String,
    /// Always `"claude"` — only agents report hooks.
    pub mode: &'static str,
    pub state: SessionState,
    pub agent: &'static str,
    pub started_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub activity: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_input: Option<String>,
    /// First step of the pending prompt, for islands built before steps.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub question: Option<Question>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub steps: Option<Vec<Question>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reply: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// The thread sbxw started this session as (`SBXW_THREAD`), when it is
    /// one: what tells a shared plan's threads apart, since they share a cwd.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thread: Option<String>,
    /// The model the session runs, as an id (`claude-opus-5-5`). Reported by
    /// the session itself (its status line, or a hook event that names it);
    /// the daemon fills in what sbxw started it with when it has not said yet.
    /// Absent when nothing is known (another agent that reports none).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// The model's display name (`Opus 5.5`), when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_name: Option<String>,
    /// What the session has cost so far, in US dollars: Claude Code's own
    /// estimate of the API-equivalent price of its tokens (the status line's
    /// `cost.total_cost_usd`), not an invoice; under a subscription it is not
    /// what the person pays. Absent until the session's status line reports.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
    /// The session's wall-clock time so far (`cost.total_duration_ms`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    /// Lines the session added and removed (`cost.total_lines_added`,
    /// `cost.total_lines_removed`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lines_added: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lines_removed: Option<u64>,
    pub origin: SessionOrigin,
    pub answerable: bool,
    pub ts: u64,
    /// Who wrote `last_input`: `"person"`, or `"injected"` when the harness
    /// did (a message from another session, a task notification…; see
    /// [`crate::history::classify_user_text`]).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_input_from: Option<&'static str>,
    /// Who injected it, when that can be read (a cross-session message's
    /// sender, `task`, `system`, `hook`…).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_input_sender: Option<String>,
    /// The session this one continues: the one the same agent process ran
    /// before (`/clear`, `/resume` in place), or the one its transcript was
    /// resumed from. Absent when not known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub continues: Option<String>,
    /// Set (`true`) only on a session's last payload: it is over, and no
    /// event of it will follow. `end_reason` says how.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub ended: bool,
    /// `session_end:<Claude Code's reason>`, `replaced` (its process started
    /// another session), `process_gone` (its process no longer runs),
    /// `sandbox_stopped`, `thread_closed`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_reason: Option<String>,
    /// Set only while the session's turn is over but work of its own runs on
    /// and will wake it (a background shell, a background agent, a one-shot
    /// scheduled wakeup): then `state` is `working` and `activity` says what
    /// it waits for. Nobody is asked anything.
    /// Boxed: every bus event carries a `SessionInfo`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub background: Option<Box<Background>>,
    /// `"background"` for a session Claude Code's harness runs with no
    /// terminal of its own (its background sessions and the spares it keeps
    /// ready for them: `CLAUDE_CODE_SESSION_KIND=bg`, or, from a hook older
    /// than v4, an agent process started as a versioned binary). Never
    /// sbxw's terminal's session: it is never answerable and does not count
    /// among the sessions of its place. Absent for any other session.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<RecordKind>,
    /// When this session last reported an event of its own (ms), unlike
    /// `ts`, which is when this payload was built: every event of a sandbox
    /// re-sends all its sessions with the same `ts`. 0 when not known.
    #[serde(skip_serializing_if = "is_zero")]
    pub event_ms: u64,
    /// Why sbxw will not type into this session, when `answerable` is
    /// false and it is not over. Absent for a thread's session (its own
    /// terminal is how sbxw reaches it) and for an answerable one.
    #[serde(skip_serializing_if = "Option::is_none")]
    /// Boxed, like `background`: every bus event carries a `SessionInfo`.
    pub unanswerable: Option<Box<Unanswerable>>,
}

/// What a record is, when it is not an ordinary session ([`SessionInfo::kind`]).
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Debug)]
#[serde(rename_all = "lowercase")]
pub enum RecordKind {
    /// A Claude Code background session: no terminal of its own.
    Background,
}

fn is_zero(n: &u64) -> bool {
    *n == 0
}

/// Why a session cannot be typed into from sbxw, with the facts that say
/// so. Machine-readable; the clients word it.
#[derive(Clone, Copy, Serialize, Debug, PartialEq, Eq)]
pub struct Unanswerable {
    /// `background`: a harness background session, with no terminal.
    /// `no_terminal`: sbxw holds no terminal for the sandbox's main session
    /// (never opened in sbxw, or the daemon restarted since).
    /// `remote`: the session came in over SSH or Claude Desktop, while
    /// sbxw's terminal is another session's.
    /// `old_hook`: several sessions, and one at least was reported by a
    /// hook older than v3, whose dead records cannot be told from live ones.
    /// `several_sessions`: several sessions share the sandbox and none or
    /// more than one could be sbxw's terminal's.
    pub reason: &'static str,
    /// Sessions the daemon counts in that sandbox's main checkout (no
    /// thread, not background), this one included when it is one.
    pub sessions: usize,
    /// Of those, how many look started from a terminal (`origin` tty).
    pub tty: usize,
    /// The oldest hook version among them (0: unknown).
    pub hook_version: u64,
}

/// A session's own pending background work, as its last `Stop` listed it
/// (Claude Code's `background_tasks` and `session_crons`).
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Debug, Default)]
pub struct Background {
    /// Background shell commands (`run_in_background`).
    pub shells: u32,
    /// Background agents: subagents, teammates, workflows, cloud sessions.
    pub agents: u32,
    /// Any other background task (a monitor, an MCP task...).
    pub other: u32,
    /// One-shot scheduled wakeups (`ScheduleWakeup`, a one-shot `CronCreate`).
    pub wakeups: u32,
    /// When the turn ended with this work pending.
    pub since_ms: u64,
    /// When sbxw stops believing it and shows the session idle, had no
    /// event ended it: `since_ms` + [`BACKGROUND_MAX_MS`].
    pub until_ms: u64,
}

/// How long a turn's background work keeps a session shown at work without
/// any event of it. `ScheduleWakeup` waits at most an hour, so a wakeup always
/// fires within it; a background command or agent still running after an
/// hour is most likely a server or a watcher that will not end on its own,
/// and the session is then as good as idle.
pub const BACKGROUND_MAX_MS: u64 = 60 * 60 * 1000;

impl Background {
    /// What the session waits for, in words: its `activity`.
    pub fn words(&self) -> String {
        let one = |n: u32, what: &str, whats: &str| if n == 1 { format!("Waiting for its {what}") } else { format!("Waiting for its {n} {whats}") };
        match (self.shells, self.agents, self.other, self.wakeups) {
            (n, 0, 0, 0) => one(n, "background command", "background commands"),
            (0, n, 0, 0) => one(n, "background agent", "background agents"),
            (0, 0, n, 0) => one(n, "background task", "background tasks"),
            (0, 0, 0, n) => one(n, "scheduled wakeup", "scheduled wakeups"),
            _ => "Waiting for its background work".into(),
        }
    }
}

/// The model a session says it runs: an id, and a display name when given.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct ModelReport {
    pub id: String,
    pub name: Option<String>,
}

impl ModelReport {
    /// From what a caller sent: an empty or blank id is no report. Both are
    /// clipped, since they reach every tab.
    pub fn new(id: Option<&str>, name: Option<&str>) -> Option<Self> {
        let id = id.map(str::trim).filter(|s| !s.is_empty())?;
        let name = name.map(str::trim).filter(|s| !s.is_empty()).map(|n| clip(n, 60));
        Some(Self { id: clip(id, 120), name })
    }
}

/// What a session's status line says it has cost: Claude Code's `cost`
/// object. Only `usd` is required; the rest goes with it when given.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct CostReport {
    pub usd: f64,
    pub duration_ms: Option<u64>,
    pub lines_added: Option<u64>,
    pub lines_removed: Option<u64>,
}

impl CostReport {
    /// From what a caller sent: no report without a finite, non-negative
    /// dollar amount under a million (a broken script is not a fortune).
    pub fn new(usd: Option<f64>, duration_ms: Option<u64>, lines_added: Option<u64>, lines_removed: Option<u64>) -> Option<Self> {
        let usd = usd.filter(|u| u.is_finite() && (0.0..1e6).contains(u))?;
        Some(Self { usd, duration_ms, lines_added, lines_removed })
    }
}

/// At most one `session` event per session this often for a change of cost
/// alone: the status line reports every ten seconds while an agent works,
/// and a cent is not worth waking every tab. A cost that changes with
/// something else goes out at once, with it.
pub const COST_EVENT_MS: u64 = 30_000;

/// One session's identity: its sandbox and Claude Code's `session_id` (empty
/// for a hook script older than the field).
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct AgentKey {
    pub sandbox: String,
    pub session_id: String,
}

#[derive(Default, Debug, Clone)]
struct Status {
    state: SessionState,
    /// The background work the last `Stop` reported, until an event of the
    /// session (or [`BACKGROUND_MAX_MS`]) ends it.
    background: Option<Background>,
    started_ms: u64,
    activity: Option<String>,
    last_input: Option<String>,
    prompt: Vec<Question>,
    reply: Option<String>,
    cwd: Option<String>,
    thread: Option<String>,
    model: Option<ModelReport>,
    cost: Option<CostReport>,
    /// When this session's cost last went out in an event (0: never), and
    /// whether it changed since.
    cost_sent_ms: u64,
    cost_pending: bool,
    origin: SessionOrigin,
    /// The agent process the session runs in (`<pid>:<starttime>`), from a
    /// v3 hook: what tells "the same terminal" for sure.
    proc: Option<String>,
    continues: Option<String>,
    last_input_from: Option<&'static str>,
    last_input_sender: Option<String>,
    /// A harness background session ([`SessionInfo::kind`]). Never unset.
    bg: bool,
    /// The name of the agent process (`claude`, `2.1.328`), from the hook's
    /// ancestry: which `live_procs` lists can speak for it.
    agent_name: Option<String>,
    /// The newest hook version that reported this session.
    hook_version: u64,
    /// When the session last sent an event.
    event_ms: u64,
}

/// What a hook event says about where its session came from.
pub struct OriginEvidence<'a> {
    pub hook_version: u64,
    pub ssh_env: &'a [String],
    pub ancestry: &'a [String],
    pub cwd: Option<&'a str>,
    pub workspace: Option<&'a str>,
    /// `SBXW_THREAD`: the session sbxw started under that name — a thread, or
    /// a session sharing the main checkout. Never the main agent.
    pub thread: Option<&'a str>,
}

/// Is this the session sbxw attached? Any client marker is enough: a client in
/// the ancestry (`server` is Claude Desktop's in-container half, `sshd` a plain
/// `ssh <name>.sbx`) or sshd's environment. The working directory settles the
/// rest. Silence is never evidence: a hook that reported nothing leaves
/// `Unknown`, not `Tty`.
pub fn classify_origin(ev: &OriginEvidence<'_>) -> SessionOrigin {
    let client = ev.ancestry.iter().any(|name| {
        let n = name.trim().to_ascii_lowercase();
        n == "server" || n == "sshd" || n == "ssh" || n.starts_with("sshd:")
    });
    if client || !ev.ssh_env.is_empty() {
        return SessionOrigin::Remote;
    }
    // Named by sbxw, so not its main agent — even when it works in the same
    // checkout, where the cwd below would read it as the tty's.
    if ev.thread.is_some() {
        return SessionOrigin::Remote;
    }
    match classify_by_cwd(ev.cwd, ev.workspace) {
        SessionOrigin::Unknown if ev.hook_version >= 2 => SessionOrigin::Tty,
        other => other,
    }
}

/// The agent sbxw attaches starts in the recorded workspace; a client lands
/// wherever it chose. Weaker than sshd's environment, so checked after it. An
/// SSH session that `cd`s into the workspace reads as the tty's too: the safe
/// misreading, since two `Tty` sessions make a sandbox read-only, not typed into.
fn classify_by_cwd(cwd: Option<&str>, workspace: Option<&str>) -> SessionOrigin {
    match (cwd, workspace) {
        (Some(c), Some(w)) if !c.is_empty() && !w.is_empty() => {
            if c.trim_end_matches('/') == w.trim_end_matches('/') {
                SessionOrigin::Tty
            } else {
                SessionOrigin::Remote
            }
        }
        _ => SessionOrigin::Unknown,
    }
}

/// One hook event, as the in-sandbox script POSTs it.
pub struct HookEvent<'a> {
    pub body: &'a Value,
}

impl<'a> HookEvent<'a> {
    pub fn new(body: &'a Value) -> Self {
        Self { body }
    }
    fn str(&self, k: &str) -> &'a str {
        self.body.get(k).and_then(Value::as_str).unwrap_or("")
    }
    pub fn event(&self) -> &'a str {
        self.str("hook_event_name")
    }
    pub fn tool(&self) -> &'a str {
        self.str("tool_name")
    }
    pub fn session_id(&self) -> &'a str {
        self.str("session_id")
    }
    pub fn cwd(&self) -> Option<&'a str> {
        Some(self.str("cwd")).filter(|s| !s.is_empty())
    }
    /// The thread the session was started as (`SBXW_THREAD`), from the status hook.
    pub fn thread(&self) -> Option<&'a str> {
        Some(self.str("sbxw_thread")).filter(|s| !s.is_empty())
    }
    /// The model a hook event names. Claude Code gives `SessionStart` a
    /// `model` id string; an object with `id` and `display_name` (the status
    /// line's shape) is read too.
    pub fn model(&self) -> Option<ModelReport> {
        match self.body.get("model")? {
            Value::String(id) => ModelReport::new(Some(id), None),
            Value::Object(m) => ModelReport::new(
                m.get("id").and_then(Value::as_str),
                m.get("display_name").and_then(Value::as_str),
            ),
            _ => None,
        }
    }
    /// The agent process the hook runs under, `<pid>:<starttime>` (hook v3).
    pub fn agent_proc(&self) -> Option<&'a str> {
        Some(self.str("agent_proc")).filter(|s| !s.is_empty() && s.len() <= 40)
    }
    /// Every process of the agent's name alive in the sandbox, as
    /// `agent_proc` keys: sent by a v3 hook on `SessionStart`, absent when
    /// `/proc` could not be read (then nothing is retired).
    pub fn live_procs(&self) -> Option<Vec<String>> {
        self.body.get("live_procs")?.as_array()?;
        Some(self.strings("live_procs"))
    }
    /// The session's own background work a `Stop` lists: `background_tasks`
    /// (in flight: running or pending, and backgrounded) and the one-shot
    /// `session_crons` (a recurring cron keeps no turn going: between its
    /// runs the session is idle). `None` when nothing is pending, and from a
    /// Claude Code that sends neither field.
    pub fn background_work(&self, now: u64) -> Option<Background> {
        let mut b = Background { since_ms: now, until_ms: now.saturating_add(BACKGROUND_MAX_MS), ..Background::default() };
        for t in self.body.get("background_tasks").and_then(Value::as_array).into_iter().flatten() {
            if !matches!(t.get("status").and_then(Value::as_str), None | Some("running" | "pending")) {
                continue;
            }
            // Claude Code's display names, and its internal ones in case.
            match t.get("type").and_then(Value::as_str).unwrap_or("") {
                "shell" | "local_bash" => b.shells += 1,
                "subagent" | "teammate" | "workflow" | "cloud session" | "local_agent" | "in_process_teammate" | "local_workflow" | "remote_agent" => b.agents += 1,
                _ => b.other += 1,
            }
        }
        for c in self.body.get("session_crons").and_then(Value::as_array).into_iter().flatten() {
            if !c.get("recurring").and_then(Value::as_bool).unwrap_or(false) {
                b.wakeups += 1;
            }
        }
        (b.shells + b.agents + b.other + b.wakeups > 0).then_some(b)
    }
    pub fn hook_version(&self) -> u64 {
        self.body.get("hook_version").and_then(Value::as_u64).unwrap_or(0)
    }
    /// The agent process's name, as the hook found it: the first of its
    /// ancestors (after the hook itself) that is not a shell. The same
    /// process `agent_proc` names.
    pub fn agent_name(&self) -> Option<String> {
        let names = self.strings("ancestry");
        names.iter().skip(1).find(|n| !LAUNCHERS.contains(&n.as_str())).filter(|n| !n.is_empty()).map(|n| clip(n, 40))
    }
    /// Claude Code's own word for the session (`CLAUDE_CODE_SESSION_KIND`,
    /// forwarded by hook v4): `bg` for its background sessions.
    pub fn session_kind(&self) -> Option<&'a str> {
        Some(self.str("session_kind")).filter(|s| !s.is_empty())
    }
    /// A harness background session: Claude Code says so (hook v4), or its
    /// agent process was started as a versioned binary (`2.1.328`), which is
    /// how Claude Code starts its background sessions and never how a
    /// terminal's `claude` runs. The second is a reading of the evidence
    /// that only ever takes a session out of the answerable ones.
    pub fn harness_background(&self) -> bool {
        self.session_kind() == Some("bg") || self.agent_name().is_some_and(|n| version_shaped(&n))
    }
    /// The hook's `live_procs` covers every Claude-like process (hook v4),
    /// not only those of the agent's own name.
    pub fn live_scope_claude(&self) -> bool {
        self.str("live_scope") == "claude"
    }
    /// A session sbxw started in the background (a code map run): nobody
    /// attached it and no terminal holds it, so it is not the sandbox's agent.
    pub fn is_background(&self) -> bool {
        self.body.get("background").and_then(Value::as_str).is_some()
    }
    /// From within a subagent: Claude Code adds `agent_id` to the hook input
    /// of a subagent's tool calls, under its parent's `session_id`.
    pub fn subagent(&self) -> bool {
        !self.str("agent_id").is_empty()
    }
    pub fn strings(&self, k: &str) -> Vec<String> {
        self.body
            .get(k)
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
            .unwrap_or_default()
    }
}

/// What runs a command for the agent, not the agent (the hook's list).
const LAUNCHERS: &[&str] = &["sh", "bash", "dash", "zsh", "fish", "env", "timeout"];

/// A Claude Code binary started by its versioned path: `2.1.328`.
pub fn version_shaped(name: &str) -> bool {
    let parts: Vec<&str> = name.split('.').collect();
    parts.len() >= 2 && parts.iter().all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
}

/// The process names a v4 hook's `live_procs` covers (`live_scope:
/// "claude"`): `claude` and the versioned binaries.
fn claude_like(name: &str) -> bool {
    name == "claude" || version_shaped(name)
}

/// Trim to one line and at most `max` chars.
pub fn clip(s: &str, max: usize) -> String {
    let one = s.split('\n').next().unwrap_or(s).trim();
    if one.chars().count() <= max {
        one.to_string()
    } else {
        let mut out: String = one.chars().take(max).collect();
        out.push('…');
        out
    }
}

/// Clip prose but keep its lines (the island's accordion shows several).
pub fn clip_lines(s: &str, max_lines: usize, max_chars: usize) -> String {
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

/// Every question of an `AskUserQuestion` call, in tab order. All or nothing:
/// a step that cannot be represented would desync answers matched by position.
pub fn questions_from_ask(body: &Value) -> Vec<Question> {
    let Some(qs) = body
        .get("tool_input")
        .and_then(|i| i.get("questions"))
        .and_then(Value::as_array)
    else {
        return Vec::new();
    };
    let steps: Option<Vec<Question>> = qs.iter().map(question_step).collect();
    steps.unwrap_or_default()
}

fn question_step(q: &Value) -> Option<Question> {
    let text = q.get("question")?.as_str()?.trim().to_string();
    let mut options = Vec::new();
    let mut context = Vec::new();
    for o in q.get("options")?.as_array()? {
        let label = o.get("label").and_then(Value::as_str).unwrap_or("").trim().to_string();
        if label.is_empty() {
            continue;
        }
        if let Some(desc) = o.get("description").and_then(Value::as_str).map(str::trim) {
            if !desc.is_empty() {
                context.push(format!("{label} — {desc}"));
            }
        }
        options.push(label);
    }
    (options.len() >= 2).then_some(Question {
        text,
        options,
        context,
    })
}

/// A short label for the tool a session is running.
pub fn describe_tool(tool: &str, input: Option<&Value>) -> String {
    let field = |k: &str| input.and_then(|i| i.get(k)).and_then(Value::as_str).unwrap_or("");
    let base = |p: &str| p.rsplit('/').next().unwrap_or(p).to_string();
    match tool {
        "Edit" | "Write" | "Read" | "NotebookEdit" => match field("file_path") {
            "" => tool.to_string(),
            p => format!("{tool} {}", base(p)),
        },
        "Bash" => match field("command") {
            "" => "Bash".into(),
            c => format!("$ {}", clip(c, 60)),
        },
        "Grep" | "Glob" => match field("pattern") {
            "" => tool.to_string(),
            p => format!("{tool} {}", clip(p, 40)),
        },
        "" => "Working".into(),
        other => other.to_string(),
    }
}

/// The `Notification` types that mean the session cannot go on without the
/// person: a permission prompt (its own, or a sandboxed worker's), an MCP
/// elicitation dialog, a dialog Claude Code files as "needs your input". From
/// Claude Code 2.1's own list of types; every other type is a notice
/// (`agent_completed`, `elicitation_complete`, `elicitation_response`,
/// `push_notification`, `quota_auto_resume_*`, `computer_use_*`, ...), and so
/// is a type this list has never heard of.
pub const SUMMONS_NOTIFICATIONS: &[&str] = &[
    "permission_prompt",
    "worker_permission_prompt",
    "elicitation_dialog",
    "elicitation_url_dialog",
    "agent_needs_input",
];

/// What a `Notification` says of the session.
#[derive(Debug, PartialEq, Eq)]
enum Notice {
    /// It waits on the person.
    Summons,
    /// The idle reminder: the prompt is open, nothing is asked.
    Idle,
    /// News: changes nothing.
    Info,
}

fn notice(kind: &str) -> Notice {
    match kind {
        "idle_prompt" => Notice::Idle,
        // No type: a Claude Code from before `notification_type`, or one of
        // today's dialogs that names none ("Claude needs your permission", a
        // plan to approve, a paused session). Each of those is a dialog open
        // on the person, so the old reading stands.
        "" => Notice::Summons,
        k if SUMMONS_NOTIFICATIONS.contains(&k) => Notice::Summons,
        _ => Notice::Info,
    }
}

/// Fold an event into a status. `true` when the session ended.
fn apply(event: &HookEvent<'_>, st: &mut Status, now: u64, sbxw_typed: &dyn Fn(&str) -> bool) -> bool {
    let body = event.body;
    // A subagent's tool calls come under its parent's session, marked with
    // `agent_id`. They are not the parent's turn: an idle parent stays idle
    // (a background helper works, the person can type) and its activity stays
    // its own. They only answer a permission prompt (one of theirs, most
    // likely, since the parent waits on them); a pending question stays.
    if event.subagent() && matches!(event.event(), "PreToolUse" | "PostToolUse") && event.tool() != "AskUserQuestion" {
        if st.state == SessionState::Attention && st.prompt.is_empty() {
            st.state = SessionState::Working;
            // Answered while the parent's turn is over: back to waiting on
            // its background work.
            if let Some(b) = st.background {
                st.activity = Some(b.words());
            }
        }
        return false;
    }
    match event.event() {
        "SessionStart" => {
            st.state = SessionState::Idle;
            st.background = None;
            st.started_ms = now;
            st.prompt.clear();
            st.activity = None;
            st.reply = None;
        }
        "UserPromptSubmit" => {
            // A turn: the person's, or the one Claude Code starts itself
            // when background work ends. Waiting on it is over either way.
            st.background = None;
            st.state = SessionState::Working;
            st.prompt.clear();
            st.activity = None;
            // The previous answer is stale the moment a new turn starts.
            st.reply = None;
            if let Some(p) = body.get("prompt").and_then(Value::as_str) {
                let mut said = crate::history::classify_user_text(p);
                if said.kind == crate::history::Kind::UserPrompt && sbxw_typed(&said.text) {
                    said.kind = crate::history::Kind::Injected;
                    said.from = Some(crate::typed::SENDER.to_string());
                }
                let person = said.kind == crate::history::Kind::UserPrompt;
                st.last_input = Some(clip(if person { &said.text } else { p }, 200));
                st.last_input_from = Some(if person { "person" } else { "injected" });
                st.last_input_sender = if person { None } else { said.from.map(|f| clip(&f, 80)) };
            }
        }
        "PreToolUse" if event.tool() == "AskUserQuestion" => {
            st.background = None;
            let steps = questions_from_ask(body);
            match steps.first() {
                Some(first) => {
                    st.activity = Some(clip(&first.text, 80));
                    st.prompt = steps;
                    st.state = SessionState::Attention;
                }
                None => st.state = SessionState::Working,
            }
        }
        "PreToolUse" => {
            st.background = None;
            st.state = SessionState::Working;
            st.prompt.clear();
            st.activity = Some(describe_tool(event.tool(), body.get("tool_input")));
        }
        "PostToolUse" => {
            st.background = None;
            if event.tool() == "AskUserQuestion" {
                st.prompt.clear();
            }
            st.state = SessionState::Working;
        }
        // Claude Code's idle reminder (a minute into an idle prompt) is not a
        // summons: after a `Stop` it repeats what is known. But a turn cut short
        // with Esc fires no `Stop`, so this is the only word that settles it;
        // without it the session reads as working (or, after a rejected
        // permission prompt, as wanting you) until the next prompt.
        "Notification" => match notice(event.str("notification_type")) {
            // Claude Code waits for background agents and wakeups before
            // it reminds, not for background shells: a reminder is not the
            // end of the session's background work.
            Notice::Idle => {
                st.prompt.clear();
                match st.background {
                    Some(b) => {
                        st.state = SessionState::Working;
                        st.activity = Some(b.words());
                    }
                    None => {
                        st.state = SessionState::Idle;
                        st.activity = None;
                    }
                }
            }
            Notice::Summons => {
                st.state = SessionState::Attention;
                if let Some(m) = body.get("message").and_then(Value::as_str) {
                    st.activity = Some(clip(m, 120));
                }
            }
            // A login finishing, a teammate done, an elicitation answered, a
            // push the model sent: news. Above all not a summons while the
            // session works (the reported case: a notice during a foreground
            // `cargo test` lit "wants you" until the tool ended).
            Notice::Info => {}
        },
        "Stop" => {
            st.prompt.clear();
            // The turn is over. With background work of its own still
            // running (a shell, an agent, a wakeup), Claude Code wakes the
            // session itself when it ends: at work, nobody is asked anything.
            st.background = event.background_work(now);
            match st.background {
                Some(b) => {
                    st.state = SessionState::Working;
                    st.activity = Some(b.words());
                }
                None => {
                    st.state = SessionState::Idle;
                    st.activity = None;
                }
            }
            // Claude Code hands the finished turn's prose to `Stop` itself; the
            // transcript lags and is never read.
            if let Some(text) = body.get("last_assistant_message").and_then(Value::as_str) {
                // Long enough for a /grill-me round (the voice concierge reads
                // it whole); bounded, since every tab gets it on each event.
                let text = clip_lines(text, 80, 4000);
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

/// Why sbxw will not type into a sandbox's agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ambiguous {
    pub sessions: usize,
}

impl std::fmt::Display for Ambiguous {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} agent sessions share this sandbox and none is identifiably the one sbxw \
             attached — answer in the session itself",
            self.sessions
        )
    }
}

/// Every agent session the hooks have told us about.
#[derive(Default, Debug)]
pub struct AgentBook {
    statuses: BTreeMap<AgentKey, Status>,
    /// What the status line reported for sessions no hook has announced yet
    /// (the status line and the hook race at start), applied when the
    /// session appears.
    early: BTreeMap<AgentKey, Early>,
    /// Per sandbox, the cost of its sessions that ended since the daemon saw
    /// it running: kept in its total until it stops, so the total never goes
    /// backwards when a session exits (or `/clear` starts a new one).
    retired_cost: BTreeMap<String, f64>,
    /// Sessions that ended with a known agent process, newest last: what a
    /// new session of the same process continues (`/clear` ends one, then
    /// starts the next). Bounded by [`ENDED_MAX`].
    ended: std::collections::VecDeque<(String, String, String)>,
    /// What sbxw typed: a turn with exactly that text is not the person's.
    typed: crate::typed::TypedLog,
}

/// Bound on [`AgentBook::ended`].
const ENDED_MAX: usize = 64;

#[derive(Default, Debug)]
struct Early {
    model: Option<ModelReport>,
    cost: Option<CostReport>,
}

/// Bound on [`AgentBook::early`]: a session that never sends a hook event
/// must not grow it forever.
const EARLY_MAX: usize = 64;

impl AgentBook {
    pub fn new() -> Self {
        Self::default()
    }

    /// Sbxw is about to type `text` into one of `sandbox`'s sessions.
    pub fn note_typed(&mut self, sandbox: &str, text: &str, now: u64) {
        self.typed.note(sandbox, text, now);
    }

    /// Fold one event in. Returns the payloads to broadcast: the session itself
    /// and every sibling in its sandbox, because whether a row is answerable
    /// depends on how many there are. `has_pty` says whether sbxw holds the
    /// sandbox's agent terminal.
    pub fn apply(
        &mut self,
        sandbox: &str,
        event: &HookEvent<'_>,
        origin: SessionOrigin,
        has_pty: bool,
        now: u64,
    ) -> Vec<SessionInfo> {
        let key = AgentKey {
            sandbox: sandbox.to_string(),
            session_id: event.session_id().to_string(),
        };
        let early = self.early.remove(&key).unwrap_or_default();
        let proc = (event.hook_version() >= crate::history::HOOK_VERSION_TRANSCRIPT).then(|| event.agent_proc()).flatten();
        let mut out = Vec::new();
        let mut continues = None;
        if event.event() == "SessionStart" {
            if let Some(p) = proc {
                continues = self.ended.iter().rev().find(|(sb, pr, id)| sb == sandbox && pr == p && *id != key.session_id).map(|e| e.2.clone());
            }
            let live = (event.hook_version() >= crate::history::HOOK_VERSION_TRANSCRIPT).then(|| event.live_procs()).flatten();
            // The names the list was made for: the hook lists the processes
            // of its agent's name (v3), or every Claude-like one (v4). A
            // record of another name is not in it whether it lives or not
            // (a background session's `2.1.328` beside terminals' `claude`).
            let name = event.agent_name();
            let wide = event.live_scope_claude();
            let covers = |theirs: Option<&str>| match theirs {
                Some(t) => name.as_deref() == Some(t) || (wide && claude_like(t)),
                None => false,
            };
            // Records proven over: the same process now runs this session, or
            // their process is no longer alive. A record whose process is not
            // known, or that the list cannot speak for, is never retired: no
            // guess makes a terminal answerable.
            let over: Vec<(AgentKey, &'static str)> = self
                .statuses
                .iter()
                .filter(|(k, _)| k.sandbox == sandbox && **k != key)
                .filter_map(|(k, st)| {
                    let theirs = st.proc.as_deref()?;
                    if Some(theirs) == proc {
                        Some((k.clone(), "replaced"))
                    } else if covers(st.agent_name.as_deref()) && live.as_ref().is_some_and(|l| !l.iter().any(|x| x == theirs)) {
                        Some((k.clone(), "process_gone"))
                    } else {
                        None
                    }
                })
                .collect();
            for (k, reason) in over {
                if reason == "replaced" {
                    continues = Some(k.session_id.clone());
                }
                out.extend(self.evict(now, reason, |kk, _| *kk == k));
            }
        }
        let typed = &self.typed;
        let st = self.statuses.entry(key.clone()).or_default();
        if let Some(p) = proc {
            st.proc = Some(p.to_string());
            st.agent_name = event.agent_name();
        }
        st.bg |= event.harness_background();
        st.hook_version = st.hook_version.max(event.hook_version());
        st.event_ms = now;
        if let Some(c) = continues.filter(|c| *c != key.session_id && !c.is_empty()) {
            st.continues.get_or_insert(c);
        }
        if let Some(m) = early.model {
            st.model.get_or_insert(m);
        }
        if let Some(c) = early.cost {
            st.cost.get_or_insert(c);
        }
        if let Some(mut m) = event.model() {
            // A bare id (`SessionStart`) keeps the name the status line gave it.
            if m.name.is_none() {
                m.name = st.model.as_ref().filter(|old| old.id == m.id).and_then(|old| old.name.clone());
            }
            st.model = Some(m);
        }
        if let Some(cwd) = event.cwd() {
            st.cwd = Some(cwd.to_string());
        }
        if let Some(thread) = event.thread() {
            st.thread = Some(thread.to_string());
        }
        // Only ever upgraded away from Unknown: one unreadable read must not
        // demote a session whose origin was already established.
        if origin != SessionOrigin::Unknown {
            st.origin = origin;
        }
        let ended = apply(event, st, now, &|t| typed.typed(sandbox, t, now));
        out.extend(self.emit(sandbox, has_pty, now));
        if ended {
            let reason = match event.str("reason") {
                "" => "session_end".to_string(),
                r => format!("session_end:{}", clip(r, 40)),
            };
            if let Some(gone) = out.iter_mut().rev().find(|i| i.session_id.as_deref().unwrap_or("") == key.session_id) {
                gone.state = SessionState::Exited;
                gone.ended = true;
                gone.end_reason = Some(reason);
            }
            if let Some(st) = self.statuses.remove(&key) {
                self.retire(&key, &st);
            }
            self.early.remove(&key);
            // A session ending can make its sibling answerable again.
            out.extend(self.emit(sandbox, has_pty, now));
        }
        out
    }

    /// The payloads of `sandbox`'s sessions, to broadcast: their costs are
    /// now out, so a pending cost-only change has nothing left to send.
    fn emit(&mut self, sandbox: &str, has_pty: bool, now: u64) -> Vec<SessionInfo> {
        let out = self.infos_for(sandbox, has_pty, now);
        for (_, st) in self.statuses.iter_mut().filter(|(k, _)| k.sandbox == sandbox) {
            if st.cost.is_some() {
                st.cost_pending = false;
                st.cost_sent_ms = now;
            }
        }
        out
    }

    /// A session leaves: its cost stays in its sandbox's total.
    fn retire(&mut self, key: &AgentKey, st: &Status) {
        if let Some(p) = &st.proc {
            if self.ended.len() >= ENDED_MAX {
                self.ended.pop_front();
            }
            self.ended.push_back((key.sandbox.clone(), p.clone(), key.session_id.clone()));
        }
        if let Some(c) = &st.cost {
            *self.retired_cost.entry(key.sandbox.clone()).or_default() += c.usd;
        }
    }

    /// What `sandbox`'s sessions have cost: the live ones plus those that
    /// ended while it ran. `None` when none of them ever reported one.
    pub fn sandbox_cost(&self, sandbox: &str) -> Option<f64> {
        let live: Vec<f64> = self
            .statuses
            .iter()
            .filter(|(k, _)| k.sandbox == sandbox)
            .filter_map(|(_, st)| st.cost.as_ref().map(|c| c.usd))
            .collect();
        let retired = self.retired_cost.get(sandbox).copied();
        if live.is_empty() && retired.is_none() {
            return None;
        }
        // Rounded to a millionth: a sum of floats should not read 0.30000000000000004.
        Some(((live.iter().sum::<f64>() + retired.unwrap_or(0.0)) * 1e6).round() / 1e6)
    }

    /// Sessions whose cost changed since its last event and whose last event
    /// is [`COST_EVENT_MS`] old: their sandboxes' payloads. For a periodic
    /// tick, so the last cent of a session gone quiet still goes out.
    pub fn flush_costs(&mut self, has_pty: impl Fn(&str) -> bool, now: u64) -> Vec<SessionInfo> {
        let due: std::collections::BTreeSet<String> = self
            .statuses
            .iter()
            .filter(|(_, st)| st.cost_pending && now.saturating_sub(st.cost_sent_ms) >= COST_EVENT_MS)
            .map(|(k, _)| k.sandbox.clone())
            .collect();
        due.into_iter()
            .flat_map(|sb| {
                let pty = has_pty(&sb);
                self.emit(&sb, pty, now)
            })
            .collect()
    }

    /// Sessions shown at work on background work for [`BACKGROUND_MAX_MS`]
    /// with no event since: shown idle now. Their sandboxes' payloads. For a
    /// periodic tick.
    pub fn expire_background(&mut self, has_pty: impl Fn(&str) -> bool, now: u64) -> Vec<SessionInfo> {
        let mut due = std::collections::BTreeSet::new();
        for (k, st) in self.statuses.iter_mut() {
            if st.background.is_some_and(|b| now >= b.until_ms) {
                st.background = None;
                if st.state == SessionState::Working {
                    st.state = SessionState::Idle;
                    st.activity = None;
                    due.insert(k.sandbox.clone());
                }
            }
        }
        due.into_iter()
            .flat_map(|sb| {
                let pty = has_pty(&sb);
                self.emit(&sb, pty, now)
            })
            .collect()
    }

    /// The model a session reports (its status line). Returns the payloads to
    /// broadcast: none when nothing changed, so a report repeated at every
    /// render costs nothing. A session not seen yet keeps it for later.
    pub fn report_model(&mut self, sandbox: &str, session_id: &str, model: ModelReport, has_pty: bool, now: u64) -> Vec<SessionInfo> {
        self.report(sandbox, session_id, Some(model), None, has_pty, now)
    }

    /// What a session's status line reports: its model and its cost, either
    /// absent from an older script. Returns the payloads to broadcast: at
    /// once for a new model or a session's first cost, for a new cost only
    /// when the session's last event that carried one is [`COST_EVENT_MS`] old (else it waits for the next event or
    /// [`Self::flush_costs`]). A cost lower than the last one is a new run of
    /// the same session id (a restart without `SessionEnd`): the old run's
    /// cost is kept in the sandbox's total.
    pub fn report(&mut self, sandbox: &str, session_id: &str, model: Option<ModelReport>, cost: Option<CostReport>, has_pty: bool, now: u64) -> Vec<SessionInfo> {
        let key = AgentKey {
            sandbox: sandbox.to_string(),
            session_id: session_id.to_string(),
        };
        let Some(st) = self.statuses.get_mut(&key) else {
            if model.is_none() && cost.is_none() {
                return Vec::new();
            }
            if self.early.len() >= EARLY_MAX && !self.early.contains_key(&key) {
                self.early.pop_first();
            }
            let e = self.early.entry(key).or_default();
            if model.is_some() {
                e.model = model;
            }
            if cost.is_some() {
                e.cost = cost;
            }
            return Vec::new();
        };
        let model_changed = model.as_ref().is_some_and(|m| st.model.as_ref() != Some(m));
        if model_changed {
            st.model = model;
        }
        let mut restarted = 0.0;
        if let Some(c) = cost.filter(|c| st.cost.as_ref() != Some(c)) {
            if let Some(old) = st.cost.as_ref().filter(|old| c.usd < old.usd) {
                restarted = old.usd;
            }
            st.cost = Some(c);
            st.cost_pending = true;
        }
        let cost_due = st.cost_pending && now.saturating_sub(st.cost_sent_ms) >= COST_EVENT_MS;
        if restarted > 0.0 {
            *self.retired_cost.entry(sandbox.to_string()).or_default() += restarted;
        }
        if model_changed || cost_due {
            self.emit(sandbox, has_pty, now)
        } else {
            Vec::new()
        }
    }

    /// The sessions of `sandbox` that could be its terminal's: every one
    /// but the harness's background sessions.
    fn keys_of(&self, sandbox: &str) -> Vec<&AgentKey> {
        self.statuses.iter().filter(|(k, st)| k.sandbox == sandbox && !st.bg).map(|(k, _)| k).collect()
    }

    /// May sbxw type into the terminal of `key`? Only when sbxw's PTY is
    /// unambiguously that session's: a sandbox with one agent session, or
    /// several of which exactly one was started from sbxw's tty. A
    /// background session is never it, and is not counted. `Err` says why
    /// not.
    fn why_not(&self, key: &AgentKey, has_pty: bool) -> Result<(), Unanswerable> {
        let siblings = self.keys_of(&key.sandbox);
        let main: Vec<&Status> = siblings.iter().filter_map(|k| self.statuses.get(*k)).filter(|s| s.thread.is_none()).collect();
        let tty_keys: Vec<&&AgentKey> = siblings.iter().filter(|k| self.statuses.get(**k).is_some_and(|s| s.origin == SessionOrigin::Tty)).collect();
        let me = self.statuses.get(key);
        let mut why = Unanswerable {
            reason: "several_sessions",
            sessions: main.len(),
            tty: main.iter().filter(|s| s.origin == SessionOrigin::Tty).count(),
            hook_version: main.iter().map(|s| s.hook_version).min().unwrap_or(0),
        };
        if me.is_some_and(|s| s.bg) {
            why.reason = "background";
            return Err(why);
        }
        if !has_pty {
            why.reason = "no_terminal";
            return Err(why);
        }
        if siblings.len() == 1 || (tty_keys.len() == 1 && **tty_keys[0] == *key) {
            return Ok(());
        }
        why.reason = if me.is_some_and(|s| s.origin == SessionOrigin::Remote) && tty_keys.len() == 1 {
            "remote"
        } else if siblings.iter().filter_map(|k| self.statuses.get(*k)).any(|s| s.hook_version < crate::history::HOOK_VERSION_TRANSCRIPT) {
            "old_hook"
        } else {
            "several_sessions"
        };
        Err(why)
    }

    fn is_answerable(&self, key: &AgentKey, has_pty: bool) -> bool {
        self.why_not(key, has_pty).is_ok()
    }

    /// The one session of `sandbox` whose prompt sbxw may answer.
    pub fn answerable(&self, sandbox: &str) -> Result<AgentKey, Ambiguous> {
        let keys = self.keys_of(sandbox);
        let ok: Vec<&&AgentKey> = keys.iter().filter(|k| self.is_answerable(k, true)).collect();
        match ok.as_slice() {
            [one] => Ok((**one).clone()),
            _ => Err(Ambiguous { sessions: keys.len() }),
        }
    }

    /// Before typing a chat message into a sandbox's agent: several sessions
    /// are fine if exactly one is the tty's; zero is fine (it is about to be
    /// brought up). Background sessions do not count.
    pub fn can_type_into(&self, sandbox: &str) -> Result<(), Ambiguous> {
        let keys = self.keys_of(sandbox);
        let tty = keys
            .iter()
            .filter(|k| self.statuses.get(**k).is_some_and(|s| s.origin == SessionOrigin::Tty))
            .count();
        if keys.len() > 1 && tty != 1 {
            return Err(Ambiguous { sessions: keys.len() });
        }
        Ok(())
    }

    /// The session sbxw's terminal types into for `sandbox`, by the rule of
    /// [`Self::can_type_into`]: the one session of the sandbox, or the one of
    /// several that was started from sbxw's tty. `None` when there is none,
    /// or several and none (or more than one) is known to be the tty's.
    /// Background sessions do not count. Whenever this is `Some`,
    /// `can_type_into` is `Ok`.
    pub fn terminal_session(&self, sandbox: &str) -> Option<AgentKey> {
        let keys = self.keys_of(sandbox);
        if let [one] = keys.as_slice() {
            return Some((*one).clone());
        }
        let tty: Vec<&&AgentKey> = keys.iter().filter(|k| self.statuses.get(**k).is_some_and(|s| s.origin == SessionOrigin::Tty)).collect();
        match tty.as_slice() {
            [one] => Some((**one).clone()),
            _ => None,
        }
    }

    /// `key`'s transcript says it was resumed from `prior` (its first
    /// entries carry that session id): its `continues`, unless one is known.
    /// Returns the payloads to broadcast when it changed.
    pub fn resumed_from(&mut self, key: &AgentKey, prior: &str, has_pty: bool, now: u64) -> Vec<SessionInfo> {
        let valid = !prior.is_empty() && prior.len() <= 128 && prior.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
        match self.statuses.get_mut(key) {
            Some(st) if valid && prior != key.session_id && st.continues.is_none() => {
                st.continues = Some(prior.to_string());
                self.emit(&key.sandbox, has_pty, now)
            }
            _ => Vec::new(),
        }
    }

    /// How many options each step of `key`'s pending prompt offers.
    pub fn prompt_sizes(&self, key: &AgentKey) -> Vec<u32> {
        self.statuses
            .get(key)
            .map(|s| s.prompt.iter().map(|q| q.options.len() as u32).collect())
            .unwrap_or_default()
    }

    /// The prompt was answered: clear it and mark the session working.
    pub fn answered(&mut self, key: &AgentKey, has_pty: bool, now: u64) -> Vec<SessionInfo> {
        if let Some(st) = self.statuses.get_mut(key) {
            st.prompt.clear();
            st.state = SessionState::Working;
        }
        self.emit(&key.sandbox, has_pty, now)
    }

    /// Drop every session whose sandbox is not in `running` — a sandbox
    /// stopped out of band sends no `SessionEnd`. Returns their `Exited`
    /// payloads. An empty `running` is a failed `sbx ls`, never "all gone".
    pub fn reconcile(&mut self, running: &[String], now: u64) -> Vec<SessionInfo> {
        if running.is_empty() {
            return Vec::new();
        }
        self.early.retain(|k, _| running.contains(&k.sandbox));
        let out = self.evict(now, "sandbox_stopped", |k, _| !running.contains(&k.sandbox));
        self.ended.retain(|(sb, _, _)| running.contains(sb));
        // A stopped sandbox starts its total again at its next run.
        self.retired_cost.retain(|sb, _| running.contains(sb));
        out
    }

    /// Drop the sessions `gone` picks; their `Exited` payloads, in key order.
    fn evict(&mut self, now: u64, reason: &str, gone: impl Fn(&AgentKey, &Status) -> bool) -> Vec<SessionInfo> {
        let mut out = Vec::new();
        let mut retired = Vec::new();
        self.statuses.retain(|k, st| {
            let go = gone(k, st);
            if go {
                let mut info = build(k, st, Err(Unanswerable { reason: "ended", sessions: 0, tty: 0, hook_version: 0 }), now);
                info.unanswerable = None;
                info.state = SessionState::Exited;
                info.ended = true;
                info.end_reason = Some(reason.to_string());
                out.push(info);
                retired.push((k.clone(), st.clone()));
            }
            !go
        });
        for (k, st) in retired {
            self.retire(&k, &st);
        }
        out
    }

    /// Drop the sessions of `sandbox` running in thread `thread`'s worktree.
    /// Closing a thread kills its agent, which sends no `SessionEnd`. Returns
    /// their `Exited` payloads.
    pub fn forget_thread(&mut self, sandbox: &str, thread: &str, now: u64) -> Vec<SessionInfo> {
        let tail = format!("/.sbxw-worktrees/{thread}");
        self.evict(now, "thread_closed", |k, st| {
            k.sandbox == sandbox
                && match st.thread.as_deref() {
                    Some(t) => t == thread,
                    None => st.cwd.as_deref().is_some_and(|c| c.trim_end_matches('/').ends_with(&tail)),
                }
        })
    }

    fn infos_for(&self, sandbox: &str, has_pty: bool, now: u64) -> Vec<SessionInfo> {
        self.statuses
            .iter()
            .filter(|(k, _)| k.sandbox == sandbox)
            .map(|(k, st)| build(k, st, self.why_not(k, has_pty), now))
            .collect()
    }

    /// Every session, for a snapshot. `has_pty` answers per sandbox.
    pub fn snapshot(&self, has_pty: impl Fn(&str) -> bool, now: u64) -> Vec<SessionInfo> {
        let mut out: Vec<SessionInfo> = self
            .statuses
            .iter()
            .map(|(k, st)| build(k, st, self.why_not(k, has_pty(&k.sandbox)), now))
            .collect();
        out.sort_by(|a, b| {
            a.sandbox
                .cmp(&b.sandbox)
                .then(a.started_ms.cmp(&b.started_ms))
                .then(a.session_id.cmp(&b.session_id))
        });
        out
    }
}

fn build(key: &AgentKey, st: &Status, why: Result<(), Unanswerable>, now: u64) -> SessionInfo {
    let answerable = why.is_ok();
    SessionInfo {
        sandbox: key.sandbox.clone(),
        mode: "claude",
        state: st.state,
        agent: "claude",
        started_ms: st.started_ms,
        activity: st.activity.clone(),
        last_input: st.last_input.clone(),
        question: st.prompt.first().cloned(),
        steps: (!st.prompt.is_empty()).then(|| st.prompt.clone()),
        reply: st.reply.clone(),
        session_id: (!key.session_id.is_empty()).then(|| key.session_id.clone()),
        cwd: st.cwd.clone(),
        thread: st.thread.clone(),
        model: st.model.as_ref().map(|m| m.id.clone()),
        model_name: st.model.as_ref().and_then(|m| m.name.clone()),
        cost_usd: st.cost.as_ref().map(|c| c.usd),
        duration_ms: st.cost.as_ref().and_then(|c| c.duration_ms),
        lines_added: st.cost.as_ref().and_then(|c| c.lines_added),
        lines_removed: st.cost.as_ref().and_then(|c| c.lines_removed),
        origin: st.origin,
        answerable,
        ts: now,
        last_input_from: st.last_input_from,
        last_input_sender: st.last_input_sender.clone(),
        continues: st.continues.clone(),
        ended: false,
        end_reason: None,
        background: st.background.filter(|_| st.state == SessionState::Working).map(Box::new),
        kind: st.bg.then_some(RecordKind::Background),
        event_ms: st.event_ms,
        unanswerable: why.err().filter(|_| st.thread.is_none()).map(Box::new),
    }
}

/// The keystrokes that answer a prompt: per step, `k-1` downs then Return
/// (each tab opens on its first option, and the prompt listens to arrows and
/// Return only — not to the numbers it shows), then a final Return on Submit.
/// An index past what the step offers is clamped, so a stray number cannot
/// spray arrows.
pub fn answer_keystrokes(indices: &[u32], sizes: &[u32]) -> Vec<&'static [u8]> {
    const DOWN: &[u8] = b"\x1b[B";
    const ENTER: &[u8] = b"\r";
    const MAX_MENU: u32 = 20;
    let mut keys = Vec::new();
    for (n, idx) in indices.iter().enumerate() {
        let limit = sizes.get(n).copied().unwrap_or(MAX_MENU).max(1);
        let downs = (*idx).clamp(1, limit) - 1;
        keys.extend(std::iter::repeat_n(DOWN, downs as usize));
        keys.push(ENTER);
    }
    keys.push(ENTER);
    keys
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ev(v: &Value) -> HookEvent<'_> {
        HookEvent::new(v)
    }

    fn ask(steps: &[(&str, &[&str])]) -> Value {
        json!({
            "hook_event_name": "PreToolUse",
            "tool_name": "AskUserQuestion",
            "tool_input": {"questions": steps.iter().map(|(q, opts)| json!({
                "question": q,
                "options": opts.iter().map(|o| json!({"label": o, "description": format!("{o}!")})).collect::<Vec<_>>()
            })).collect::<Vec<_>>()}
        })
    }

    #[test]
    fn a_question_builds_options_and_a_decision_table() {
        let qs = questions_from_ask(&ask(&[("Which?", &["A", "B"])]));
        assert_eq!(qs[0].options, ["A", "B"]);
        assert_eq!(qs[0].context, ["A — A!", "B — B!"]);
        assert!(questions_from_ask(&ask(&[("Which?", &["A"])])).is_empty());
        assert!(
            questions_from_ask(&ask(&[("ok", &["A", "B"]), ("bad", &["A"])])).is_empty(),
            "all or nothing"
        );
    }

    #[test]
    fn hooks_drive_the_state() {
        let mut book = AgentBook::new();
        let start = json!({"hook_event_name": "SessionStart", "session_id": "s1"});
        let prompt = json!({"hook_event_name": "UserPromptSubmit", "session_id": "s1", "prompt": "fix it"});
        let stop = json!({"hook_event_name": "Stop", "session_id": "s1", "last_assistant_message": "Done.\nAll good."});
        book.apply("myapp", &ev(&start), SessionOrigin::Tty, true, 1);
        let out = book.apply("myapp", &ev(&prompt), SessionOrigin::Tty, true, 2);
        assert_eq!(out[0].state, SessionState::Working);
        assert_eq!(out[0].last_input.as_deref(), Some("fix it"));
        let out = book.apply("myapp", &ev(&stop), SessionOrigin::Tty, true, 3);
        assert_eq!(out[0].state, SessionState::Idle);
        assert_eq!(out[0].reply.as_deref(), Some("Done.\nAll good."));
        let out = book.apply("myapp", &ev(&prompt), SessionOrigin::Unknown, true, 4);
        assert_eq!(out[0].reply, None, "a new prompt clears the reply");
        assert_eq!(out[0].origin, SessionOrigin::Tty, "unknown never demotes");
    }

    #[test]
    fn an_idle_reminder_is_not_a_summons() {
        let mut book = AgentBook::new();
        let stop = json!({"hook_event_name": "Stop", "session_id": "s1"});
        let idle = json!({"hook_event_name": "Notification", "session_id": "s1",
            "notification_type": "idle_prompt", "message": "Claude is waiting for your input"});
        let perm = json!({"hook_event_name": "Notification", "session_id": "s1",
            "notification_type": "permission_prompt", "message": "Claude needs your permission to use Bash"});
        let auth = json!({"hook_event_name": "Notification", "session_id": "s1",
            "notification_type": "auth_success", "message": "Authenticated"});
        let tool = json!({"hook_event_name": "PreToolUse", "session_id": "s1", "tool_name": "Bash"});
        book.apply("n", &ev(&stop), SessionOrigin::Tty, true, 1);
        let out = book.apply("n", &ev(&idle), SessionOrigin::Tty, true, 2);
        assert_eq!(out[0].state, SessionState::Idle);
        let out = book.apply("n", &ev(&auth), SessionOrigin::Tty, true, 3);
        assert_eq!(out[0].state, SessionState::Idle, "a login is not a summons");
        let out = book.apply("n", &ev(&perm), SessionOrigin::Tty, true, 4);
        assert_eq!(out[0].state, SessionState::Attention);
        // Esc on the permission prompt: no `Stop`, only the idle reminder.
        let out = book.apply("n", &ev(&idle), SessionOrigin::Tty, true, 5);
        assert_eq!(out[0].state, SessionState::Idle, "an interrupted ask settles");
        // Esc mid-tool: likewise.
        book.apply("n", &ev(&tool), SessionOrigin::Tty, true, 6);
        let out = book.apply("n", &ev(&idle), SessionOrigin::Tty, true, 7);
        assert_eq!(out[0].state, SessionState::Idle, "an interrupted turn settles");
    }

    /// A `Notification` as Claude Code 2.1 posts it (`message`, and
    /// `notification_type` unless `kind` is empty: an older Claude Code, or one
    /// of its dialogs that names no type).
    fn note(kind: &str, message: &str) -> Value {
        let mut v = json!({"hook_event_name": "Notification", "session_id": "s1", "message": message});
        if !kind.is_empty() {
            v["notification_type"] = kind.into();
        }
        v
    }

    fn state_after(book: &mut AgentBook, e: &Value, now: u64) -> SessionInfo {
        book.apply("n", &ev(e), SessionOrigin::Tty, true, now).remove(0)
    }

    /// The reported case: a thread runs `cargo test` in the foreground, a
    /// message from its lead arrives, Claude Code posts notices meanwhile;
    /// none of them asks anything of the person.
    #[test]
    fn a_notice_while_working_is_not_a_summons() {
        let mut book = AgentBook::new();
        let prompt = json!({"hook_event_name": "UserPromptSubmit", "session_id": "s1", "prompt": "run the tests"});
        let bash = json!({"hook_event_name": "PreToolUse", "session_id": "s1", "tool_name": "Bash", "tool_input": {"command": "cargo test"}});
        let done = json!({"hook_event_name": "PostToolUse", "session_id": "s1", "tool_name": "Bash"});
        state_after(&mut book, &prompt, 1);
        state_after(&mut book, &bash, 2);
        let notices = [
            note("agent_completed", "Lead session finished"),
            note("elicitation_complete", "MCP server \"x\" confirmed elicitation 1 complete"),
            note("elicitation_response", "Elicitation response for server \"x\": accept"),
            note("push_notification", "Tests are running"),
            note("plugin_notification", "Plugin says hi"),
            note("computer_use_exit", "Claude is done using your computer"),
            note("quota_auto_resume_fired", "Usage limit reset, Claude is continuing your task"),
            note("model_refusal_fallback", "Retrying on another model"),
            note("auth_storage_failure", "Claude Code login needs attention"),
            note("auth_success", "Claude Code login successful"),
            note("a_type_from_next_year", "Something happened"),
        ];
        for (i, n) in notices.iter().enumerate() {
            let out = state_after(&mut book, n, 3 + i as u64);
            assert_eq!(out.state, SessionState::Working, "{n}");
            assert_eq!(out.activity.as_deref(), Some("$ cargo test"), "a notice does not replace the activity: {n}");
        }
        assert_eq!(state_after(&mut book, &done, 20).state, SessionState::Working);
    }

    #[test]
    fn what_needs_the_person_still_summons_at_once_and_holds_until_answered() {
        let bash = json!({"hook_event_name": "PreToolUse", "session_id": "s1", "tool_name": "Bash", "tool_input": {"command": "rm -rf build"}});
        let done = json!({"hook_event_name": "PostToolUse", "session_id": "s1", "tool_name": "Bash"});
        for n in [
            note("permission_prompt", "Claude needs your permission to use Bash"),
            note("elicitation_dialog", "Claude Code needs your input"),
            note("elicitation_url_dialog", "An MCP server needs your input"),
            note("worker_permission_prompt", "worker needs permission for Bash"),
            note("agent_needs_input", "Teammate setup needs your input"),
            // No type: an older Claude Code, or a dialog of today's that names
            // none ("Claude needs your permission", a plan to approve).
            note("", "Claude needs your permission"),
            note("", "Claude Code needs your approval for the plan"),
        ] {
            let mut book = AgentBook::new();
            state_after(&mut book, &bash, 1);
            let out = state_after(&mut book, &n, 2);
            assert_eq!(out.state, SessionState::Attention, "{n}");
            // A notice meanwhile does not answer it.
            let out = state_after(&mut book, &note("agent_completed", "Lead session finished"), 3);
            assert_eq!(out.state, SessionState::Attention, "a notice does not clear a summons: {n}");
            assert_eq!(state_after(&mut book, &done, 4).state, SessionState::Working, "answered: {n}");
        }
    }

    #[test]
    fn an_idle_reminder_then_a_message_starts_a_turn() {
        let stop = json!({"hook_event_name": "Stop", "session_id": "s1", "last_assistant_message": "Waiting for the lead."});
        let idle = note("idle_prompt", "Claude is waiting for your input");
        let perm = note("permission_prompt", "Claude needs your permission to use Bash");
        let injected = json!({"hook_event_name": "UserPromptSubmit", "session_id": "s1",
            "prompt": "<cross-session-message from=\"Lead session\">Go on with the routes.</cross-session-message>"});
        let tool = json!({"hook_event_name": "PreToolUse", "session_id": "s1", "tool_name": "Read", "tool_input": {"file_path": "/w/a.rs"}});
        // After a finished turn, the reminder is idle, not "wants you".
        let mut book = AgentBook::new();
        state_after(&mut book, &stop, 1);
        assert_eq!(state_after(&mut book, &idle, 2).state, SessionState::Idle);
        let out = state_after(&mut book, &injected, 3);
        assert_eq!(out.state, SessionState::Working, "an injected message starts a turn");
        assert_eq!(out.last_input_from, Some("injected"));
        // A real summons, then the message: the first evidence of work clears it,
        // whether the turn opens with the prompt or straight with a tool.
        for first in [&injected, &tool] {
            let mut book = AgentBook::new();
            state_after(&mut book, &stop, 1);
            assert_eq!(state_after(&mut book, &perm, 2).state, SessionState::Attention);
            assert_eq!(state_after(&mut book, first, 3).state, SessionState::Working, "{first}");
        }
    }

    #[test]
    fn a_question_holds_through_notices_until_answered() {
        let mut book = AgentBook::new();
        let mut q = ask(&[("Which?", &["A", "B"])]);
        q["session_id"] = "s1".into();
        let out = state_after(&mut book, &q, 1);
        assert_eq!(out.state, SessionState::Attention);
        let out = state_after(&mut book, &note("agent_completed", "Lead session finished"), 2);
        assert_eq!(out.state, SessionState::Attention);
        assert_eq!(out.question.as_ref().map(|q| q.text.as_str()), Some("Which?"));
        assert_eq!(out.activity.as_deref(), Some("Which?"), "the question stays the activity");
        let answered = json!({"hook_event_name": "PostToolUse", "session_id": "s1", "tool_name": "AskUserQuestion"});
        let out = state_after(&mut book, &answered, 3);
        assert_eq!(out.state, SessionState::Working);
        assert!(out.question.is_none());
    }

    /// A `Stop` as Claude Code 2.1 sends it: `background_tasks` lists the
    /// in-flight background work of the session (running or pending, and
    /// backgrounded), `session_crons` the session's scheduled crons.
    fn stop_with(tasks: &[(&str, &str)], crons: &[bool]) -> Value {
        json!({"hook_event_name": "Stop", "session_id": "s1", "stop_hook_active": false,
            "last_assistant_message": "The build runs in the background; I will pick it up when it ends.",
            "background_tasks": tasks.iter().enumerate().map(|(n, (kind, status))| json!({
                "id": format!("b{n}"), "type": kind, "status": status, "description": "cargo build", "command": "cargo build"
            })).collect::<Vec<_>>(),
            "session_crons": crons.iter().enumerate().map(|(n, rec)| json!({
                "id": format!("c{n}"), "schedule": "*/5 * * * *", "recurring": rec, "prompt": "check the build"
            })).collect::<Vec<_>>()})
    }

    /// A task notification typed in by Claude Code when a background task
    /// ends: an injected turn.
    fn woken() -> Value {
        json!({"hook_event_name": "UserPromptSubmit", "session_id": "s1",
            "prompt": "<task-notification>\n<task-id>b0</task-id>\n<status>completed</status>\n</task-notification>"})
    }

    /// The reported case: the turn ends while a background shell of the
    /// session runs (`1 shell` in Claude Code's status line). Claude Code
    /// will wake the session itself when it ends: the session is at work,
    /// never waiting for the person, and not plainly idle either.
    #[test]
    fn a_turn_ended_on_a_background_command_is_busy() {
        let mut book = AgentBook::new();
        let prompt = json!({"hook_event_name": "UserPromptSubmit", "session_id": "s1", "prompt": "build it"});
        let bash = json!({"hook_event_name": "PreToolUse", "session_id": "s1", "tool_name": "Bash",
            "tool_input": {"command": "cargo build", "run_in_background": true}});
        let bash_done = json!({"hook_event_name": "PostToolUse", "session_id": "s1", "tool_name": "Bash",
            "tool_input": {"command": "cargo build", "run_in_background": true},
            "tool_response": {"backgroundTaskId": "b0"}});
        state_after(&mut book, &prompt, 1);
        state_after(&mut book, &bash, 2);
        state_after(&mut book, &bash_done, 3);
        let out = state_after(&mut book, &stop_with(&[("shell", "running")], &[]), 10);
        assert_eq!(out.state, SessionState::Working, "busy, not attention, not idle");
        let bg = out.background.expect("its background work");
        assert_eq!((bg.shells, bg.agents, bg.other, bg.wakeups), (1, 0, 0, 0));
        assert_eq!((bg.since_ms, bg.until_ms), (10, 10 + BACKGROUND_MAX_MS));
        assert_eq!(out.activity.as_deref(), Some("Waiting for its background command"));
        assert!(out.reply.as_deref().is_some_and(|r| r.starts_with("The build runs")), "the turn's words are kept");
        // Claude Code's idle reminder fires a minute later for a background
        // shell (it waits only for background agents): still busy.
        let out = state_after(&mut book, &note("idle_prompt", "Claude is waiting for your input"), 60_010);
        assert_eq!(out.state, SessionState::Working, "the reminder does not end the background work");
        assert!(out.background.is_some());
        // A notice is news.
        assert_eq!(state_after(&mut book, &note("agent_completed", "x finished"), 60_020).state, SessionState::Working);
        // The command ends: Claude Code types its notification in, a new turn.
        let out = state_after(&mut book, &woken(), 90_000);
        assert_eq!(out.state, SessionState::Working);
        assert_eq!(out.background, None, "a turn of its own: no longer waiting");
        assert_eq!(out.activity, None);
        assert_eq!(out.last_input_from, Some("injected"));
        let out = state_after(&mut book, &stop_with(&[], &[]), 95_000);
        assert_eq!(out.state, SessionState::Idle, "finished, nothing pending");
        assert_eq!(out.background, None);
    }

    #[test]
    fn background_work_without_any_event_falls_back_to_idle() {
        let mut book = AgentBook::new();
        state_after(&mut book, &stop_with(&[("shell", "running")], &[]), 1_000);
        assert!(book.expire_background(|_| true, 1_000 + BACKGROUND_MAX_MS - 1).is_empty(), "not yet");
        let out = book.expire_background(|_| true, 1_000 + BACKGROUND_MAX_MS);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].state, SessionState::Idle);
        assert_eq!(out[0].background, None);
        assert_eq!(out[0].activity, None);
        assert!(out[0].reply.is_some(), "the turn's words stay");
        assert!(book.expire_background(|_| true, 2_000 + BACKGROUND_MAX_MS).is_empty(), "once");
        // A session idle or at work in a turn is never touched.
        let mut book = AgentBook::new();
        state_after(&mut book, &stop_with(&[], &[]), 1);
        state_after(&mut book, &json!({"hook_event_name": "UserPromptSubmit", "session_id": "t", "prompt": "go"}), 1);
        assert!(book.expire_background(|_| true, 10 * BACKGROUND_MAX_MS).is_empty());
    }

    /// Nothing pending, or a Claude Code from before these fields: idle, as
    /// it always was.
    #[test]
    fn a_turn_ended_with_nothing_pending_is_idle() {
        for stop in [
            stop_with(&[], &[]),
            json!({"hook_event_name": "Stop", "session_id": "s1", "last_assistant_message": "Done."}),
            stop_with(&[("shell", "completed")], &[]),
            stop_with(&[], &[true]),
        ] {
            let mut book = AgentBook::new();
            let out = state_after(&mut book, &stop, 1);
            assert_eq!(out.state, SessionState::Idle, "{stop}");
            assert_eq!(out.background, None, "{stop}");
            assert_eq!(out.activity, None, "{stop}");
        }
    }

    /// A background agent, a one-shot wakeup, several commands: worded so.
    #[test]
    fn background_work_is_worded_by_what_waits() {
        for (tasks, crons, (sh, ag, ot, wk), words) in [
            (&[("subagent", "running")][..], &[][..], (0, 1, 0, 0), "Waiting for its background agent"),
            (&[("teammate", "running"), ("workflow", "pending"), ("cloud session", "running")][..], &[][..], (0, 3, 0, 0), "Waiting for its 3 background agents"),
            (&[("shell", "running"), ("shell", "pending")][..], &[][..], (2, 0, 0, 0), "Waiting for its 2 background commands"),
            (&[][..], &[false][..], (0, 0, 0, 1), "Waiting for its scheduled wakeup"),
            (&[("monitor", "running")][..], &[][..], (0, 0, 1, 0), "Waiting for its background task"),
            (&[("shell", "running"), ("subagent", "running")][..], &[true, false][..], (1, 1, 0, 1), "Waiting for its background work"),
        ] {
            let mut book = AgentBook::new();
            let out = state_after(&mut book, &stop_with(tasks, crons), 1);
            assert_eq!(out.state, SessionState::Working, "{words}");
            let bg = out.background.expect(words);
            assert_eq!((bg.shells, bg.agents, bg.other, bg.wakeups), (sh, ag, ot, wk), "{words}");
            assert_eq!(out.activity.as_deref(), Some(words));
        }
    }

    /// A permission prompt while background work runs (a background agent's
    /// tool, most likely): it wants the person, and once answered the
    /// session is back to waiting on its background work.
    #[test]
    fn a_prompt_during_background_work_wins() {
        let mut book = AgentBook::new();
        state_after(&mut book, &stop_with(&[("subagent", "running")], &[]), 1);
        let out = state_after(&mut book, &note("permission_prompt", "Claude needs your permission to use Write"), 2);
        assert_eq!(out.state, SessionState::Attention);
        assert_eq!(out.background, None, "shown only while at work");
        assert_eq!(out.activity.as_deref(), Some("Claude needs your permission to use Write"));
        let sub = json!({"hook_event_name": "PostToolUse", "session_id": "s1", "agent_id": "a1", "tool_name": "Write"});
        let out = state_after(&mut book, &sub, 3);
        assert_eq!(out.state, SessionState::Working, "answered");
        assert!(out.background.is_some(), "still waiting on its agent");
        assert_eq!(out.activity.as_deref(), Some("Waiting for its background agent"));
        // The agent's own tools change nothing meanwhile.
        let pre = json!({"hook_event_name": "PreToolUse", "session_id": "s1", "agent_id": "a1", "tool_name": "Read", "tool_input": {"file_path": "/w/a"}});
        let out = state_after(&mut book, &pre, 4);
        assert_eq!(out.activity.as_deref(), Some("Waiting for its background agent"));
        // A question asked by the session itself: attention too.
        let mut book = AgentBook::new();
        state_after(&mut book, &stop_with(&[("shell", "running")], &[]), 1);
        let mut q = ask(&[("Which?", &["A", "B"])]);
        q["session_id"] = "s1".into();
        let out = state_after(&mut book, &q, 2);
        assert_eq!(out.state, SessionState::Attention);
        assert_eq!(out.background, None);
    }

    /// Two background commands, one ends: woken, then the next `Stop` lists
    /// the one left. Still busy.
    #[test]
    fn one_of_two_background_commands_ends() {
        let mut book = AgentBook::new();
        let out = state_after(&mut book, &stop_with(&[("shell", "running"), ("shell", "running")], &[]), 1);
        assert_eq!(out.background.map(|b| b.shells), Some(2));
        assert_eq!(state_after(&mut book, &woken(), 2).background, None);
        let out = state_after(&mut book, &stop_with(&[("shell", "running")], &[]), 3);
        assert_eq!(out.state, SessionState::Working);
        assert_eq!(out.background.map(|b| (b.shells, b.since_ms)), Some((1, 3)), "the bound starts again");
        assert_eq!(out.activity.as_deref(), Some("Waiting for its background command"));
    }

    /// A session killed while it waits on background work: retired by the
    /// existing rules (its process gone at the next start, or `SessionEnd`).
    #[test]
    fn a_session_killed_while_busy_is_retired() {
        let v3 = |mut v: Value, sid: &str, p: &str| {
            v["session_id"] = sid.into();
            v["hook_version"] = 3.into();
            v["agent_proc"] = p.into();
            v["ancestry"] = json!(["node", "claude"]);
            v
        };
        let mut book = AgentBook::new();
        book.apply("n", &ev(&v3(json!({"hook_event_name": "SessionStart", "live_procs": ["42:100"]}), "old", "42:100")), SessionOrigin::Tty, true, 1);
        let out = book.apply("n", &ev(&v3(stop_with(&[("shell", "running")], &[]), "old", "42:100")), SessionOrigin::Tty, true, 2);
        assert_eq!(out[0].state, SessionState::Working);
        let out = book.apply("n", &ev(&v3(json!({"hook_event_name": "SessionStart", "live_procs": ["43:200"]}), "new", "43:200")), SessionOrigin::Tty, true, 3);
        let gone = out.iter().find(|i| i.session_id.as_deref() == Some("old")).expect("the old one goes");
        assert_eq!((gone.state, gone.ended, gone.end_reason.as_deref()), (SessionState::Exited, true, Some("process_gone")));
        assert!(book.expire_background(|_| true, 10 * BACKGROUND_MAX_MS).is_empty(), "nothing left to expire");
        let mut book = AgentBook::new();
        state_after(&mut book, &stop_with(&[("shell", "running")], &[]), 1);
        let end = json!({"hook_event_name": "SessionEnd", "session_id": "s1", "reason": "other"});
        let out = book.apply("n", &ev(&end), SessionOrigin::Tty, true, 2);
        assert_eq!(out.last().map(|i| i.state), Some(SessionState::Exited));
        assert!(book.expire_background(|_| true, 10 * BACKGROUND_MAX_MS).is_empty());
    }

    /// The session's own tool call (the person typed a prompt that started
    /// straight with a tool, or a turn resumed): a turn, no longer waiting.
    #[test]
    fn the_session_s_own_tool_ends_the_wait() {
        let mut book = AgentBook::new();
        state_after(&mut book, &stop_with(&[("shell", "running")], &[]), 1);
        let tool = json!({"hook_event_name": "PreToolUse", "session_id": "s1", "tool_name": "Read", "tool_input": {"file_path": "/w/a.rs"}});
        let out = state_after(&mut book, &tool, 2);
        assert_eq!(out.state, SessionState::Working);
        assert_eq!(out.background, None);
        assert_eq!(out.activity.as_deref(), Some("Read a.rs"));
    }

    /// A subagent's tool calls come under its parent's `session_id`, marked
    /// with `agent_id` (Claude Code's hook input: "present only when the hook
    /// fires from within a subagent"). A background subagent goes on after
    /// its parent's `Stop`.
    #[test]
    fn a_subagent_s_tools_do_not_move_its_parent() {
        let sub = |name: &str, tool: &str| json!({"hook_event_name": name, "session_id": "s1", "agent_id": "a1", "agent_type": "Explore", "tool_name": tool, "tool_input": {"file_path": "/w/x.rs"}});
        let stop = json!({"hook_event_name": "Stop", "session_id": "s1"});
        let bash = json!({"hook_event_name": "PreToolUse", "session_id": "s1", "tool_name": "Bash", "tool_input": {"command": "cargo test"}});
        let mut book = AgentBook::new();
        state_after(&mut book, &stop, 1);
        let out = state_after(&mut book, &sub("PreToolUse", "Read"), 2);
        assert_eq!(out.state, SessionState::Idle, "the person can type: a background helper works");
        assert_eq!(out.activity, None);
        state_after(&mut book, &sub("PostToolUse", "Read"), 3);
        assert_eq!(state_after(&mut book, &sub("PostToolUse", "Read"), 3).state, SessionState::Idle);
        let out = state_after(&mut book, &bash, 4);
        assert_eq!(out.activity.as_deref(), Some("$ cargo test"));
        let out = state_after(&mut book, &sub("PreToolUse", "Read"), 5);
        assert_eq!(out.activity.as_deref(), Some("$ cargo test"), "the parent's own tool stays the activity");
        // A subagent's tool waits on a permission, then runs: answered.
        assert_eq!(state_after(&mut book, &note("permission_prompt", "Claude needs your permission to use Write"), 6).state, SessionState::Attention);
        assert_eq!(state_after(&mut book, &sub("PostToolUse", "Write"), 7).state, SessionState::Working);
        // A question pending in the parent is not answered by a helper's tool.
        let mut q = ask(&[("Which?", &["A", "B"])]);
        q["session_id"] = "s1".into();
        state_after(&mut book, &q, 8);
        assert_eq!(state_after(&mut book, &sub("PostToolUse", "Read"), 9).state, SessionState::Attention);
    }

    #[test]
    fn a_replaced_record_s_summons_is_not_carried_over() {
        let mut book = AgentBook::new();
        let v3 = |mut v: Value, sid: &str| {
            v["session_id"] = sid.into();
            v["hook_version"] = 3.into();
            v["agent_proc"] = "42:100".into();
            v
        };
        book.apply("n", &ev(&v3(json!({"hook_event_name": "SessionStart", "live_procs": ["42:100"]}), "old")), SessionOrigin::Tty, true, 1);
        book.apply("n", &ev(&v3(ask(&[("Which?", &["A", "B"])]), "old")), SessionOrigin::Tty, true, 2);
        book.apply("n", &ev(&v3(note("permission_prompt", "Claude needs your permission to use Bash"), "old")), SessionOrigin::Tty, true, 3);
        // `/clear`: the same process starts a new session.
        let out = book.apply("n", &ev(&v3(json!({"hook_event_name": "SessionStart", "live_procs": ["42:100"]}), "new")), SessionOrigin::Tty, true, 4);
        let new = out.iter().rev().find(|i| i.session_id.as_deref() == Some("new")).unwrap();
        assert_eq!(new.state, SessionState::Idle);
        assert!(new.question.is_none() && new.steps.is_none());
        assert_eq!(new.continues.as_deref(), Some("old"));
        let snap = book.snapshot(|_| true, 5);
        assert!(snap.iter().all(|i| i.state != SessionState::Attention), "{snap:?}");
    }

    #[test]
    fn an_ask_enters_attention_and_an_end_removes() {
        let mut book = AgentBook::new();
        let out = book.apply("n", &ev(&ask(&[("Q?", &["A", "B"]), ("R?", &["C", "D"])])), SessionOrigin::Tty, true, 1);
        assert_eq!(out[0].state, SessionState::Attention);
        assert_eq!(out[0].steps.as_ref().unwrap().len(), 2);
        assert_eq!(out[0].question.as_ref().unwrap().text, "Q?");
        let end = json!({"hook_event_name": "SessionEnd"});
        let out = book.apply("n", &ev(&end), SessionOrigin::Tty, true, 2);
        assert_eq!(out[0].state, SessionState::Exited);
        assert!(book.snapshot(|_| true, 3).is_empty());
    }

    #[test]
    fn the_terminal_session_follows_can_type_into() {
        let mut book = AgentBook::new();
        let s = |id: &str| json!({"hook_event_name": "SessionStart", "session_id": id});
        let term = |b: &AgentBook| b.terminal_session("n").map(|k| k.session_id);
        assert_eq!(term(&book), None, "no session");
        book.apply("n", &ev(&s("a")), SessionOrigin::Unknown, true, 1);
        assert_eq!(term(&book).as_deref(), Some("a"), "alone");
        book.apply("n", &ev(&s("b")), SessionOrigin::Remote, true, 2);
        assert_eq!(term(&book), None, "two, none tty");
        assert!(book.can_type_into("n").is_err());
        book.apply("n", &ev(&s("a")), SessionOrigin::Tty, true, 3);
        assert_eq!(term(&book).as_deref(), Some("a"), "the older one is the tty's");
        book.can_type_into("n").unwrap();
        book.apply("n", &ev(&s("b")), SessionOrigin::Tty, true, 4);
        assert_eq!(term(&book), None, "two tty: unknown");
        assert!(book.can_type_into("n").is_err());
    }

    #[test]
    fn answering_needs_one_identifiable_terminal() {
        let mut book = AgentBook::new();
        let s = |id: &str| json!({"hook_event_name": "SessionStart", "session_id": id});
        book.apply("n", &ev(&s("a")), SessionOrigin::Unknown, true, 1);
        assert_eq!(book.answerable("n").unwrap().session_id, "a", "alone: answerable");
        book.apply("n", &ev(&s("b")), SessionOrigin::Remote, true, 2);
        assert!(book.answerable("n").is_err(), "two, none tty");
        assert!(book.can_type_into("n").is_err());
        book.apply("n", &ev(&s("a")), SessionOrigin::Tty, true, 3);
        assert_eq!(book.answerable("n").unwrap().session_id, "a");
        book.can_type_into("n").unwrap();
        book.apply("n", &ev(&s("b")), SessionOrigin::Tty, true, 4);
        assert!(book.answerable("n").is_err(), "two tty: read-only");
        let snap = book.snapshot(|_| false, 5);
        assert!(snap.iter().all(|i| !i.answerable), "no pty, nothing answerable");
    }

    #[test]
    fn origin_follows_the_evidence() {
        let none: Vec<String> = vec![];
        let e = |ver, ssh: &[String], anc: &[String], cwd: Option<&'static str>| {
            classify_origin(&OriginEvidence {
                hook_version: ver,
                ssh_env: ssh,
                ancestry: anc,
                cwd,
                workspace: Some("/w"),
                thread: None,
            })
        };
        assert_eq!(e(2, &none, &["node".into(), "server".into()], Some("/w")), SessionOrigin::Remote);
        assert_eq!(e(2, &["SSH_TTY".into()], &none, Some("/w")), SessionOrigin::Remote);
        assert_eq!(e(2, &none, &["node".into(), "claude".into()], Some("/w/")), SessionOrigin::Tty);
        assert_eq!(e(1, &none, &none, Some("/elsewhere")), SessionOrigin::Remote);
        assert_eq!(e(1, &none, &none, None), SessionOrigin::Unknown);
        assert_eq!(e(2, &none, &none, None), SessionOrigin::Tty);
        let named = classify_origin(&OriginEvidence {
            hook_version: 2,
            ssh_env: &none,
            ancestry: &none,
            cwd: Some("/w"),
            workspace: Some("/w"),
            thread: Some("s1"),
        });
        assert_eq!(named, SessionOrigin::Remote, "a session in main's checkout, started under a name, is not the tty's");
    }

    #[test]
    fn a_closed_thread_forgets_only_its_own_session() {
        let mut book = AgentBook::new();
        let s = |id: &str, cwd: &str| json!({"hook_event_name": "SessionStart", "session_id": id, "cwd": cwd});
        book.apply("c", &ev(&s("main", "/w")), SessionOrigin::Tty, true, 1);
        book.apply("c", &ev(&s("t2", "/w/.sbxw-worktrees/chat-2")), SessionOrigin::Remote, true, 2);
        book.apply("c", &ev(&s("t3", "/w/.sbxw-worktrees/chat-22")), SessionOrigin::Remote, true, 3);
        let gone = book.forget_thread("c", "chat-2", 4);
        assert_eq!(gone.len(), 1);
        assert_eq!(gone[0].state, SessionState::Exited);
        assert_eq!(book.snapshot(|_| true, 5).len(), 2, "the main agent and chat-22 stay");
    }

    #[test]
    fn a_shared_plans_threads_are_told_apart_by_name_not_cwd() {
        let mut book = AgentBook::new();
        let team = "/w/.sbxw-worktrees/team-p-1";
        let s = |id: &str, thread: &str| {
            json!({"hook_event_name": "SessionStart", "session_id": id, "cwd": team, "sbxw_thread": thread})
        };
        book.apply("c", &ev(&s("t1", "api")), SessionOrigin::Remote, true, 1);
        let infos = book.apply("c", &ev(&s("t2", "web")), SessionOrigin::Remote, true, 2);
        assert_eq!(infos.last().and_then(|i| i.thread.as_deref()), Some("web"));
        let gone = book.forget_thread("c", "api", 3);
        assert_eq!(gone.iter().map(|i| i.thread.as_deref()).collect::<Vec<_>>(), [Some("api")]);
        assert!(book.forget_thread("c", "team-p-1", 4).is_empty(), "the shared worktree is no thread");
        assert_eq!(book.snapshot(|_| true, 5).len(), 1, "web stays");
    }

    #[test]
    fn reconcile_drops_stopped_sandboxes_but_not_on_a_failed_ls() {
        let mut book = AgentBook::new();
        book.apply("a", &ev(&json!({"hook_event_name": "SessionStart"})), SessionOrigin::Tty, true, 1);
        assert!(book.reconcile(&[], 2).is_empty());
        let gone = book.reconcile(&["b".into()], 3);
        assert_eq!(gone[0].state, SessionState::Exited);
        assert!(book.snapshot(|_| true, 4).is_empty());
    }

    #[test]
    fn the_model_comes_from_hooks_and_the_status_line() {
        let mut book = AgentBook::new();
        let opus = || ModelReport::new(Some("claude-opus-5-5"), Some("Opus 5.5")).unwrap();
        // Reported before the session's first hook: kept, applied on arrival.
        assert!(book.report_model("n", "s1", opus(), true, 1).is_empty());
        let out = book.apply("n", &ev(&json!({"hook_event_name": "SessionStart", "session_id": "s1"})), SessionOrigin::Tty, true, 2);
        assert_eq!((out[0].model.as_deref(), out[0].model_name.as_deref()), (Some("claude-opus-5-5"), Some("Opus 5.5")));
        // The same again broadcasts nothing; another does.
        assert!(book.report_model("n", "s1", opus(), true, 3).is_empty());
        let haiku = ModelReport::new(Some("claude-haiku-5-5"), Some("Haiku 5.5")).unwrap();
        let out = book.report_model("n", "s1", haiku, true, 4);
        assert_eq!(out[0].model.as_deref(), Some("claude-haiku-5-5"));
        // A hook naming the same id bare keeps the display name; one naming
        // none changes nothing.
        let bare = json!({"hook_event_name": "SessionStart", "session_id": "s1", "model": "claude-haiku-5-5"});
        let out = book.apply("n", &ev(&bare), SessionOrigin::Tty, true, 5);
        assert_eq!(out[0].model_name.as_deref(), Some("Haiku 5.5"));
        let out = book.apply("n", &ev(&json!({"hook_event_name": "Stop", "session_id": "s1"})), SessionOrigin::Tty, true, 6);
        assert_eq!(out[0].model.as_deref(), Some("claude-haiku-5-5"));
        // A hook with the status line's shape, and a new id with no name.
        let obj = json!({"hook_event_name": "Stop", "session_id": "s1", "model": {"id": "claude-sonnet-5-5", "display_name": "Sonnet 5.5"}});
        assert_eq!(book.apply("n", &ev(&obj), SessionOrigin::Tty, true, 7)[0].model_name.as_deref(), Some("Sonnet 5.5"));
        let other = json!({"hook_event_name": "Stop", "session_id": "s1", "model": "claude-x"});
        let out = book.apply("n", &ev(&other), SessionOrigin::Tty, true, 8);
        assert_eq!((out[0].model.as_deref(), out[0].model_name.as_deref()), (Some("claude-x"), None));
    }

    #[test]
    fn no_model_stays_absent() {
        let mut book = AgentBook::new();
        let out = book.apply("n", &ev(&json!({"hook_event_name": "SessionStart", "session_id": "s"})), SessionOrigin::Tty, true, 1);
        assert_eq!((out[0].model.as_ref(), out[0].model_name.as_ref()), (None, None));
        let v = serde_json::to_value(&out[0]).unwrap();
        assert!(v.get("model").is_none() && v.get("model_name").is_none(), "omitted, not null");
        assert!(ModelReport::new(Some("  "), Some("x")).is_none(), "a blank id is no report");
        assert!(ModelReport::new(None, Some("Opus")).is_none());
        let junk = json!({"hook_event_name": "Stop", "session_id": "s", "model": 3});
        assert_eq!(book.apply("n", &ev(&junk), SessionOrigin::Tty, true, 2)[0].model, None);
    }

    #[test]
    fn early_models_are_bounded_and_forgotten_with_their_sandbox() {
        let mut book = AgentBook::new();
        let m = || ModelReport::new(Some("m"), None).unwrap();
        for i in 0..(EARLY_MAX + 10) {
            book.report_model("n", &format!("s{i:03}"), m(), true, 1);
        }
        assert_eq!(book.early.len(), EARLY_MAX);
        book.apply("keep", &ev(&json!({"hook_event_name": "SessionStart", "session_id": "k"})), SessionOrigin::Tty, true, 2);
        book.reconcile(&["keep".into()], 3);
        assert!(book.early.is_empty());
    }

    #[test]
    fn costs_coalesce_and_add_up_per_sandbox() {
        let mut book = AgentBook::new();
        let usd = |u: f64| CostReport::new(Some(u), Some(1000), Some(3), Some(1));
        // Reported before the first hook: applied on arrival.
        assert!(book.report("n", "a", None, usd(0.10), true, 1).is_empty());
        let out = book.apply("n", &ev(&json!({"hook_event_name": "SessionStart", "session_id": "a"})), SessionOrigin::Tty, true, 2);
        assert_eq!((out[0].cost_usd, out[0].lines_added, out[0].duration_ms), (Some(0.10), Some(3), Some(1000)));
        // The event at 2 carried it: a change within 30 s of it waits, and is
        // still read by a snapshot; one 30 s after goes out.
        assert!(book.report("n", "a", None, usd(0.20), true, 3).is_empty());
        assert!(book.report("n", "a", None, usd(0.30), true, 10_000).is_empty());
        assert_eq!(book.snapshot(|_| true, 10_001)[0].cost_usd, Some(0.30));
        assert_eq!(book.report("n", "a", None, usd(0.35), true, 40_000)[0].cost_usd, Some(0.35), "due 30 s after the last event");
        assert!(book.report("n", "a", None, usd(0.40), true, 41_000).is_empty());
        assert_eq!(book.snapshot(|_| true, 41_001)[0].cost_usd, Some(0.40));
        // With a model change it goes out at once.
        let opus = ModelReport::new(Some("claude-opus-5-5"), None);
        let out = book.report("n", "a", opus, usd(0.50), true, 42_000);
        assert_eq!(out[0].cost_usd, Some(0.50));
        // A session gone quiet: the tick sends its last change once.
        assert!(book.report("n", "a", None, usd(0.60), true, 50_000).is_empty());
        assert!(book.flush_costs(|_| true, 60_000).is_empty(), "not 30 s yet");
        assert_eq!(book.flush_costs(|_| true, 72_000)[0].cost_usd, Some(0.60));
        assert!(book.flush_costs(|_| true, 200_000).is_empty(), "nothing pending");
        // A hook event carries the latest cost and clears what was pending.
        assert!(!book.report("n", "a", None, usd(0.70), true, 210_000).is_empty());
        assert!(book.report("n", "a", None, usd(0.80), true, 211_000).is_empty());
        let out = book.apply("n", &ev(&json!({"hook_event_name": "Stop", "session_id": "a"})), SessionOrigin::Tty, true, 212_000);
        assert_eq!(out[0].cost_usd, Some(0.80));
        assert!(book.flush_costs(|_| true, 500_000).is_empty());

        // The sandbox's total: every session, live or ended while it runs.
        book.apply("n", &ev(&json!({"hook_event_name": "SessionStart", "session_id": "b", "sbxw_thread": "t"})), SessionOrigin::Tty, true, 3);
        book.report("n", "b", None, usd(1.25), true, 4);
        assert_eq!(book.sandbox_cost("n"), Some(2.05));
        book.apply("n", &ev(&json!({"hook_event_name": "SessionEnd", "session_id": "b"})), SessionOrigin::Tty, true, 5);
        assert_eq!(book.sandbox_cost("n"), Some(2.05), "an ended session stays counted");
        book.forget_thread("n", "nope", 6);
        // The same session id restarted (no SessionEnd): its old run stays.
        book.report("n", "a", None, usd(0.05), true, 600_000);
        assert_eq!(book.sandbox_cost("n"), Some(2.10), "1.25 ended + 0.80 old run + 0.05");
        // Stopped: forgotten, and the next run starts from nothing.
        book.reconcile(&["other".into()], 7);
        assert_eq!(book.sandbox_cost("n"), None);
    }

    #[test]
    fn no_cost_stays_absent() {
        let mut book = AgentBook::new();
        let out = book.apply("n", &ev(&json!({"hook_event_name": "SessionStart", "session_id": "s"})), SessionOrigin::Tty, true, 1);
        let v = serde_json::to_value(&out[0]).unwrap();
        for k in ["cost_usd", "duration_ms", "lines_added", "lines_removed"] {
            assert!(v.get(k).is_none(), "{k} omitted, not null");
        }
        assert_eq!(book.sandbox_cost("n"), None);
        // An older script: no cost, nothing broadcast, nothing kept.
        assert!(book.report("n", "s", None, None, true, 2).is_empty());
        assert!(book.report("n", "never", None, None, true, 2).is_empty());
        assert!(book.early.is_empty());
        for bad in [f64::NAN, f64::INFINITY, -1.0, 2e6] {
            assert!(CostReport::new(Some(bad), None, None, None).is_none(), "{bad}");
        }
        assert!(CostReport::new(None, Some(5), None, None).is_none(), "no dollars, no report");
    }

    #[test]
    fn tools_are_described_briefly() {
        assert_eq!(describe_tool("Edit", Some(&json!({"file_path": "/a/b.rs"}))), "Edit b.rs");
        assert_eq!(describe_tool("Bash", Some(&json!({"command": "ls -la\nmore"}))), "$ ls -la");
        assert_eq!(describe_tool("", None), "Working");
        assert_eq!(describe_tool("Task", None), "Task");
    }

    #[test]
    fn answers_become_arrows_and_returns() {
        let k = answer_keystrokes(&[2, 1], &[3, 2]);
        assert_eq!(k, [b"\x1b[B" as &[u8], b"\r", b"\r", b"\r"]);
        let clamped = answer_keystrokes(&[9], &[2]);
        assert_eq!(clamped.len(), 3, "one down, return, submit");
    }

    #[test]
    fn clip_lines_keeps_lines_and_bounds_length() {
        assert_eq!(clip_lines("a\nb\nc", 2, 100), "a\nb");
        assert_eq!(clip_lines(&"x".repeat(10), 5, 4), "xxxx…");
    }

    fn start3(id: &str, proc: Option<&str>, live: Option<&[&str]>) -> Value {
        let mut v = json!({"hook_event_name": "SessionStart", "session_id": id, "hook_version": 3, "ancestry": ["node", "claude"]});
        if let Some(p) = proc {
            v["agent_proc"] = json!(p);
        }
        if let Some(l) = live {
            v["live_procs"] = json!(l);
        }
        v
    }

    #[test]
    fn a_restarted_main_session_retires_its_dead_record_and_is_answerable_again() {
        let mut book = AgentBook::new();
        book.apply("a", &ev(&start3("old", Some("100:5"), Some(&["100:5"]))), SessionOrigin::Tty, true, 1);
        // The terminal was killed: no SessionEnd. A new claude starts.
        let out = book.apply("a", &ev(&start3("new", Some("200:9"), Some(&["200:9"]))), SessionOrigin::Tty, true, 2);
        let gone = out.iter().find(|i| i.session_id.as_deref() == Some("old")).unwrap();
        assert!(gone.ended && gone.state == SessionState::Exited);
        assert_eq!(gone.end_reason.as_deref(), Some("process_gone"));
        assert_eq!(book.answerable("a").unwrap().session_id, "new");
        assert!(book.can_type_into("a").is_ok());
    }

    #[test]
    fn a_live_second_terminal_or_an_unknown_process_is_never_retired() {
        let mut book = AgentBook::new();
        book.apply("a", &ev(&start3("one", Some("100:5"), Some(&["100:5"]))), SessionOrigin::Tty, true, 1);
        // An old hook's record: its process is not known.
        book.apply("a", &ev(&json!({"hook_event_name": "SessionStart", "session_id": "legacy", "hook_version": 2})), SessionOrigin::Tty, true, 1);
        // A second terminal, both alive.
        book.apply("a", &ev(&start3("two", Some("200:9"), Some(&["100:5", "200:9"]))), SessionOrigin::Tty, true, 2);
        assert_eq!(book.snapshot(|_| true, 3).len(), 3);
        assert!(book.answerable("a").is_err(), "three tty sessions: still nobody's");
        // /proc unreadable: no list, nothing retired.
        book.apply("a", &ev(&start3("three", Some("300:1"), None)), SessionOrigin::Remote, true, 4);
        assert_eq!(book.snapshot(|_| true, 5).len(), 4);
        // A v2 hook's process and list are not trusted.
        let v2 = json!({"hook_event_name": "SessionStart", "session_id": "four", "hook_version": 2, "agent_proc": "100:5", "live_procs": []});
        book.apply("a", &ev(&v2), SessionOrigin::Remote, true, 6);
        assert_eq!(book.snapshot(|_| true, 7).len(), 5);
    }

    #[test]
    fn clear_in_the_same_process_continues_the_previous_session() {
        let mut book = AgentBook::new();
        book.apply("a", &ev(&start3("s1", Some("100:5"), Some(&["100:5"]))), SessionOrigin::Tty, true, 1);
        // /clear: SessionEnd for s1, then SessionStart for s2, same process.
        let end = json!({"hook_event_name": "SessionEnd", "session_id": "s1", "hook_version": 3, "agent_proc": "100:5", "reason": "clear"});
        let out = book.apply("a", &ev(&end), SessionOrigin::Tty, true, 2);
        let last = out.iter().find(|i| i.session_id.as_deref() == Some("s1")).unwrap();
        assert!(last.ended);
        assert_eq!(last.end_reason.as_deref(), Some("session_end:clear"));
        let out = book.apply("a", &ev(&start3("s2", Some("100:5"), Some(&["100:5"]))), SessionOrigin::Tty, true, 3);
        assert_eq!(out.iter().find(|i| i.session_id.as_deref() == Some("s2")).unwrap().continues.as_deref(), Some("s1"));
        // /resume in place, no SessionEnd: the same process replaces s2.
        let out = book.apply("a", &ev(&start3("s3", Some("100:5"), None)), SessionOrigin::Tty, true, 4);
        let s2 = out.iter().find(|i| i.session_id.as_deref() == Some("s2")).unwrap();
        assert_eq!(s2.end_reason.as_deref(), Some("replaced"));
        assert_eq!(out.iter().find(|i| i.session_id.as_deref() == Some("s3")).unwrap().continues.as_deref(), Some("s2"));
        assert_eq!(book.snapshot(|_| true, 5).len(), 1);
    }

    #[test]
    fn a_hook_never_retires_another_sandboxs_session() {
        let mut book = AgentBook::new();
        book.apply("a", &ev(&start3("a1", Some("100:5"), Some(&["100:5"]))), SessionOrigin::Tty, true, 1);
        book.apply("b", &ev(&start3("b1", Some("100:5"), Some(&["999:1"]))), SessionOrigin::Tty, true, 2);
        assert_eq!(book.snapshot(|_| true, 3).len(), 2, "same pid in another sandbox, and a list without it: untouched");
    }

    #[test]
    fn stopped_sandboxes_and_closed_threads_say_they_ended() {
        let mut book = AgentBook::new();
        book.apply("a", &ev(&json!({"hook_event_name": "SessionStart", "session_id": "s"})), SessionOrigin::Tty, true, 1);
        let out = book.reconcile(&["other".to_string()], 2);
        assert!(out[0].ended);
        assert_eq!(out[0].end_reason.as_deref(), Some("sandbox_stopped"));
    }

    #[test]
    fn a_resumed_transcript_names_what_it_continues_once() {
        let mut book = AgentBook::new();
        book.apply("a", &ev(&json!({"hook_event_name": "SessionStart", "session_id": "new"})), SessionOrigin::Tty, true, 1);
        let key = AgentKey { sandbox: "a".into(), session_id: "new".into() };
        assert!(book.resumed_from(&key, "../x", true, 2).is_empty());
        assert_eq!(book.resumed_from(&key, "old", true, 2)[0].continues.as_deref(), Some("old"));
        assert!(book.resumed_from(&key, "older", true, 3).is_empty(), "the first word stands");
        let other = AgentKey { sandbox: "b".into(), session_id: "new".into() };
        assert!(book.resumed_from(&other, "old", true, 3).is_empty());
    }

    #[test]
    fn the_last_input_says_who_wrote_it() {
        let mut book = AgentBook::new();
        let p = |t: &str| json!({"hook_event_name": "UserPromptSubmit", "session_id": "s", "prompt": t});
        let out = book.apply("a", &ev(&p("Fix the build")), SessionOrigin::Tty, true, 1);
        assert_eq!(out[0].last_input_from, Some("person"));
        assert_eq!(out[0].last_input_sender, None);
        let out = book.apply("a", &ev(&p("<cross-session-message from-name=\"lead\">rebase</cross-session-message>")), SessionOrigin::Tty, true, 2);
        assert_eq!(out[0].last_input_from, Some("injected"));
        assert_eq!(out[0].last_input_sender.as_deref(), Some("lead"));
        let json = serde_json::to_value(&out[0]).unwrap();
        assert!(json.get("ended").is_none() && json.get("continues").is_none(), "absent unless known");
    }

    // ---- Background sessions, retirement by name, the reason on the wire.

    /// A v3 hook's event of a terminal's `claude` (ancestry as sbxw-2's hook
    /// log shows it).
    fn term(event: &str, id: &str, proc: &str) -> Value {
        json!({"hook_event_name": event, "session_id": id, "hook_version": 3, "agent_proc": proc,
            "ancestry": ["MainThread", "claude"], "cwd": "/w"})
    }

    /// A harness background session's start (a spare, or a `/bg` job): its
    /// process is the versioned binary, and the hook listed only processes
    /// of that name.
    fn bg_start(id: &str, proc: &str, live: &[&str]) -> Value {
        json!({"hook_event_name": "SessionStart", "session_id": id, "hook_version": 3, "agent_proc": proc,
            "ancestry": ["MainThread", "2.1.328", "2.1.328", "claude"], "live_procs": live, "cwd": "/w"})
    }

    fn info_of<'a>(infos: &'a [SessionInfo], id: &str) -> &'a SessionInfo {
        infos.iter().find(|i| i.session_id.as_deref() == Some(id)).expect("listed")
    }

    /// The user's sequence on sbxw-2: the daemon restarts; the lead's busy
    /// session and four idle ones are interactive; a background spare starts.
    /// Every interactive record survives the spare's start, the spare is its
    /// own background record, and when the lead is the only tty session it
    /// stays answerable.
    #[test]
    fn a_helper_s_start_after_a_restart_keeps_the_interactive_sessions() {
        let mut book = AgentBook::new();
        // The busy lead reports first (any event: the daemon knew nothing).
        book.apply("sbxw-2", &ev(&term("PreToolUse", "lead", "100:5")), SessionOrigin::Tty, true, 1);
        // Four idle interactive sessions act once each (they had been idle).
        for (n, id) in ["i1", "i2", "i3", "i4"].iter().enumerate() {
            book.apply("sbxw-2", &ev(&term("Stop", id, &format!("{}:6", 200 + n))), SessionOrigin::Tty, true, 2);
        }
        // The spare: the hook lists the `2.1.328` processes only.
        let out = book.apply("sbxw-2", &ev(&bg_start("spare", "300:7", &["300:7", "301:7"])), SessionOrigin::Tty, true, 3);
        assert!(out.iter().all(|i| !i.ended), "nothing retired: {:?}", out.iter().filter(|i| i.ended).map(|i| &i.session_id).collect::<Vec<_>>());
        let snap = book.snapshot(|_| true, 4);
        assert_eq!(snap.len(), 6, "five interactive and the spare");
        let spare = info_of(&snap, "spare");
        assert_eq!(spare.kind, Some(RecordKind::Background));
        assert!(!spare.answerable);
        assert_eq!(spare.unanswerable.as_deref().map(|u| u.reason), Some("background"));
        let lead = info_of(&snap, "lead");
        assert_eq!(lead.kind, None);
        assert_eq!(lead.unanswerable.as_deref().map(|u| (u.reason, u.sessions, u.tty)), Some(("several_sessions", 5, 5)), "five terminals: nobody's, and it says how many");
        assert!(lead.event_ms > 0 && spare.event_ms > lead.event_ms, "each says when it last reported");
        // The lead alone beside the spare: answerable, the spare not counted.
        let mut book = AgentBook::new();
        book.apply("sbxw-2", &ev(&term("PreToolUse", "lead", "100:5")), SessionOrigin::Tty, true, 1);
        book.apply("sbxw-2", &ev(&bg_start("spare", "300:7", &["300:7"])), SessionOrigin::Tty, true, 2);
        let snap = book.snapshot(|_| true, 3);
        assert!(info_of(&snap, "lead").answerable, "the only terminal session");
        assert_eq!(info_of(&snap, "lead").unanswerable, None);
        assert_eq!(book.answerable("sbxw-2").unwrap().session_id, "lead");
        assert!(book.can_type_into("sbxw-2").is_ok());
    }

    /// Claude Code's own word (hook v4) marks a background session whatever
    /// its process is named.
    #[test]
    fn claude_code_s_own_word_marks_a_background_session() {
        let mut book = AgentBook::new();
        let mut v = term("SessionStart", "job", "400:1");
        v["hook_version"] = 4.into();
        v["session_kind"] = "bg".into();
        let out = book.apply("n", &ev(&v), SessionOrigin::Tty, true, 1);
        assert_eq!(out[0].kind, Some(RecordKind::Background));
        assert_eq!(out[0].unanswerable.as_deref().map(|u| u.reason), Some("background"));
        // Sticky: a later event without the word does not make it a terminal's.
        let out = book.apply("n", &ev(&term("Stop", "job", "400:1")), SessionOrigin::Tty, true, 2);
        assert_eq!(out[0].kind, Some(RecordKind::Background));
        // An interactive session's v4 event says nothing of the kind.
        let mut v = term("SessionStart", "me", "401:1");
        v["hook_version"] = 4.into();
        let out = book.apply("n", &ev(&v), SessionOrigin::Tty, true, 3);
        assert_eq!(info_of(&out, "me").kind, None);
        assert!(info_of(&out, "me").answerable, "the job is not counted");
    }

    /// The v3 reading (a versioned process name) is a heuristic in the safe
    /// direction: a real terminal session whose process is named like a
    /// version is shown as background, listed, and never answerable.
    #[test]
    fn a_version_named_terminal_is_shown_as_background_never_answerable() {
        let mut book = AgentBook::new();
        let mut v = term("SessionStart", "odd", "500:1");
        v["ancestry"] = json!(["node", "bash", "2.1.300"]);
        let out = book.apply("n", &ev(&v), SessionOrigin::Tty, true, 1);
        assert_eq!(out.len(), 1, "listed, its own record");
        assert_eq!(out[0].kind, Some(RecordKind::Background));
        assert!(!out[0].answerable);
        assert!(book.answerable("n").is_err());
        assert_eq!(book.can_type_into("n"), Ok(()), "nothing to type into: the main session is brought up");
    }

    #[test]
    fn a_background_session_alone_is_not_answerable() {
        let mut book = AgentBook::new();
        let out = book.apply("n", &ev(&bg_start("spare", "300:7", &["300:7"])), SessionOrigin::Tty, true, 1);
        assert!(!out[0].answerable, "even with sbxw's terminal open, it is not that terminal's");
        let u = out[0].unanswerable.as_deref().copied().unwrap();
        assert_eq!((u.reason, u.sessions, u.tty), ("background", 0, 0));
        assert!(book.answerable("n").is_err());
        let wire = serde_json::to_value(&out[0]).unwrap();
        assert_eq!(wire["kind"], "background");
        assert_eq!(wire["unanswerable"]["reason"], "background");
    }

    #[test]
    fn several_interactive_sessions_say_how_many() {
        let mut book = AgentBook::new();
        for (n, id) in ["a", "b", "c"].iter().enumerate() {
            book.apply("n", &ev(&term("SessionStart", id, &format!("{}:1", 10 + n))), SessionOrigin::Tty, true, 1 + n as u64);
        }
        let snap = book.snapshot(|_| true, 9);
        for i in &snap {
            let u = i.unanswerable.as_deref().copied().expect("a reason");
            assert_eq!((u.reason, u.sessions, u.tty, u.hook_version), ("several_sessions", 3, 3, 3));
        }
        // A thread's session carries none: its own terminal reaches it.
        let mut t = term("SessionStart", "t", "20:1");
        t["sbxw_thread"] = "t1".into();
        let out = book.apply("n", &ev(&t), SessionOrigin::Remote, true, 10);
        assert_eq!(info_of(&out, "t").unanswerable, None);
        assert_eq!(info_of(&out, "a").unanswerable.as_deref().copied().unwrap().sessions, 3, "a thread is not in the main checkout");
    }

    #[test]
    fn no_terminal_is_its_own_reason() {
        let mut book = AgentBook::new();
        book.apply("n", &ev(&term("SessionStart", "a", "10:1")), SessionOrigin::Tty, false, 1);
        let snap = book.snapshot(|_| false, 2);
        let u = snap[0].unanswerable.as_deref().copied().unwrap();
        assert_eq!((u.reason, u.sessions, u.tty), ("no_terminal", 1, 1));
        // The terminal opened: answerable, no reason.
        let snap = book.snapshot(|_| true, 3);
        assert!(snap[0].answerable && snap[0].unanswerable.is_none());
    }

    #[test]
    fn an_old_hook_sandbox_says_so() {
        let mut book = AgentBook::new();
        let s = |id: &str| json!({"hook_event_name": "SessionStart", "session_id": id, "hook_version": 2});
        book.apply("n", &ev(&s("a")), SessionOrigin::Tty, true, 1);
        book.apply("n", &ev(&s("b")), SessionOrigin::Tty, true, 2);
        let snap = book.snapshot(|_| true, 3);
        let u = snap[0].unanswerable.as_deref().copied().unwrap();
        assert_eq!((u.reason, u.sessions, u.hook_version), ("old_hook", 2, 2));
        // A remote session beside the one terminal session says it is remote.
        let mut book = AgentBook::new();
        book.apply("n", &ev(&term("SessionStart", "a", "10:1")), SessionOrigin::Tty, true, 1);
        let out = book.apply("n", &ev(&term("SessionStart", "ssh", "11:1")), SessionOrigin::Remote, true, 2);
        assert_eq!(info_of(&out, "ssh").unanswerable.as_deref().map(|u| u.reason), Some("remote"));
        assert!(info_of(&out, "a").answerable);
    }

    /// Retirement never drops a live interactive session because a list
    /// was made for another name, and a v4 list made for every Claude-like
    /// process still retires the dead ones of either name.
    #[test]
    fn retirement_never_drops_a_live_interactive_session() {
        let mut book = AgentBook::new();
        book.apply("n", &ev(&term("SessionStart", "a", "10:1")), SessionOrigin::Tty, true, 1);
        book.apply("n", &ev(&bg_start("spare", "300:7", &["300:7"])), SessionOrigin::Tty, true, 2);
        // Another spare, the first gone: only the old spare (same name) goes.
        let out = book.apply("n", &ev(&bg_start("spare2", "301:7", &["301:7"])), SessionOrigin::Tty, true, 3);
        let ended: Vec<_> = out.iter().filter(|i| i.ended).map(|i| i.session_id.clone().unwrap()).collect();
        assert_eq!(ended, vec!["spare".to_string()]);
        // A terminal's start lists `claude` processes: the spare is kept.
        let out = book.apply("n", &ev(&start3("b", Some("11:1"), Some(&["10:1", "11:1"]))), SessionOrigin::Tty, true, 4);
        assert!(out.iter().all(|i| !i.ended));
        // A v4 list covers every Claude-like process: the dead spare and a
        // dead terminal go, the live terminal stays.
        let mut v = term("SessionStart", "c", "12:1");
        v["hook_version"] = 4.into();
        v["live_scope"] = "claude".into();
        v["live_procs"] = json!(["10:1", "12:1"]);
        let out = book.apply("n", &ev(&v), SessionOrigin::Tty, true, 5);
        let mut ended: Vec<_> = out.iter().filter(|i| i.ended).map(|i| i.session_id.clone().unwrap()).collect();
        ended.sort();
        assert_eq!(ended, vec!["b".to_string(), "spare2".to_string()]);
        assert!(book.snapshot(|_| true, 6).iter().any(|i| i.session_id.as_deref() == Some("a")), "alive, kept");
    }

    #[test]
    fn version_shapes() {
        assert!(version_shaped("2.1.328") && version_shaped("10.0"));
        assert!(!version_shaped("claude") && !version_shaped("2") && !version_shaped("2..1") && !version_shaped("v2.1"));
    }
}

#[cfg(test)]
mod typed_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_line_sbxw_typed_is_not_the_person_answering() {
        let mut book = AgentBook::new();
        let line = crate::team::main_moved_line(&["a".to_string()], None).unwrap();
        book.note_typed("sb", &line, 1);
        let p = |t: &str| json!({"hook_event_name": "UserPromptSubmit", "session_id": "s", "prompt": t});
        let out = book.apply("sb", &HookEvent::new(&p(&line)), SessionOrigin::Tty, true, 2);
        assert_eq!(out[0].last_input_from, Some("injected"));
        assert_eq!(out[0].last_input_sender.as_deref(), Some("sbxw"));
        let out = book.apply("sb", &HookEvent::new(&p("[sbxw] hello")), SessionOrigin::Tty, true, 3);
        assert_eq!(out[0].last_input_from, Some("person"));
        assert_eq!(out[0].last_input_sender, None);
    }
}
