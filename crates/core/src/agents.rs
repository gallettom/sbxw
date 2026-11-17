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
//! | `Notification` | attention (`idle_prompt`: idle; `auth_success`: unchanged) |
//! | `Stop` | idle (captures the reply) |
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
    pub mode: String,
    pub state: SessionState,
    pub agent: String,
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
    pub fn hook_version(&self) -> u64 {
        self.body.get("hook_version").and_then(Value::as_u64).unwrap_or(0)
    }
    /// A session sbxw started in the background (a code map run): nobody
    /// attached it and no terminal holds it, so it is not the sandbox's agent.
    pub fn is_background(&self) -> bool {
        self.body.get("background").and_then(Value::as_str).is_some()
    }
    pub fn strings(&self, k: &str) -> Vec<String> {
        self.body
            .get(k)
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
            .unwrap_or_default()
    }
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

/// Fold an event into a status. `true` when the session ended.
fn apply(event: &HookEvent<'_>, st: &mut Status, now: u64) -> bool {
    let body = event.body;
    match event.event() {
        "SessionStart" => {
            st.state = SessionState::Idle;
            st.started_ms = now;
            st.prompt.clear();
            st.activity = None;
            st.reply = None;
        }
        "UserPromptSubmit" => {
            st.state = SessionState::Working;
            st.prompt.clear();
            st.activity = None;
            // The previous answer is stale the moment a new turn starts.
            st.reply = None;
            if let Some(p) = body.get("prompt").and_then(Value::as_str) {
                st.last_input = Some(clip(p, 200));
            }
        }
        "PreToolUse" if event.tool() == "AskUserQuestion" => {
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
            st.state = SessionState::Working;
            st.prompt.clear();
            st.activity = Some(describe_tool(event.tool(), body.get("tool_input")));
        }
        "PostToolUse" => {
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
        "Notification" if event.str("notification_type") == "idle_prompt" => {
            st.state = SessionState::Idle;
            st.prompt.clear();
            st.activity = None;
        }
        // A login finishing is news to nobody watching the strip.
        "Notification" if event.str("notification_type") == "auth_success" => {}
        "Notification" => {
            st.state = SessionState::Attention;
            if let Some(m) = body.get("message").and_then(Value::as_str) {
                st.activity = Some(clip(m, 120));
            }
        }
        "Stop" => {
            st.state = SessionState::Idle;
            st.prompt.clear();
            st.activity = None;
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
}

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
        let st = self.statuses.entry(key.clone()).or_default();
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
        let ended = apply(event, st, now);
        let mut out = self.emit(sandbox, has_pty, now);
        if ended {
            if let Some(gone) = out.iter_mut().find(|i| i.session_id.as_deref().unwrap_or("") == key.session_id) {
                gone.state = SessionState::Exited;
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

    fn keys_of(&self, sandbox: &str) -> Vec<&AgentKey> {
        self.statuses.keys().filter(|k| k.sandbox == sandbox).collect()
    }

    /// May sbxw type into the terminal of `key`? Only when sbxw's PTY is
    /// unambiguously that session's: a sandbox with one agent session, or
    /// several of which exactly one was started from sbxw's tty.
    fn is_answerable(&self, key: &AgentKey, has_pty: bool) -> bool {
        if !has_pty {
            return false;
        }
        let siblings = self.keys_of(&key.sandbox);
        if siblings.len() == 1 {
            return true;
        }
        let tty: Vec<&&AgentKey> = siblings
            .iter()
            .filter(|k| self.statuses.get(**k).is_some_and(|s| s.origin == SessionOrigin::Tty))
            .collect();
        tty.len() == 1 && **tty[0] == *key
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
    /// brought up).
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
        let out = self.evict(now, |k, _| !running.contains(&k.sandbox));
        // A stopped sandbox starts its total again at its next run.
        self.retired_cost.retain(|sb, _| running.contains(sb));
        out
    }

    /// Drop the sessions `gone` picks; their `Exited` payloads, in key order.
    fn evict(&mut self, now: u64, gone: impl Fn(&AgentKey, &Status) -> bool) -> Vec<SessionInfo> {
        let mut out = Vec::new();
        let mut retired = Vec::new();
        self.statuses.retain(|k, st| {
            let go = gone(k, st);
            if go {
                let mut info = build(k, st, false, now);
                info.state = SessionState::Exited;
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
        self.evict(now, |k, st| {
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
            .map(|(k, st)| build(k, st, self.is_answerable(k, has_pty), now))
            .collect()
    }

    /// Every session, for a snapshot. `has_pty` answers per sandbox.
    pub fn snapshot(&self, has_pty: impl Fn(&str) -> bool, now: u64) -> Vec<SessionInfo> {
        let mut out: Vec<SessionInfo> = self
            .statuses
            .iter()
            .map(|(k, st)| build(k, st, self.is_answerable(k, has_pty(&k.sandbox)), now))
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

fn build(key: &AgentKey, st: &Status, answerable: bool, now: u64) -> SessionInfo {
    SessionInfo {
        sandbox: key.sandbox.clone(),
        mode: "claude".into(),
        state: st.state,
        agent: "claude".into(),
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
}
