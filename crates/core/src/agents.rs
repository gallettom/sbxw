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
//! | `Notification` | attention |
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
    pub origin: SessionOrigin,
    pub answerable: bool,
    pub ts: u64,
}

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
    origin: SessionOrigin,
}

/// What a hook event says about where its session came from.
pub struct OriginEvidence<'a> {
    pub hook_version: u64,
    pub ssh_env: &'a [String],
    pub ancestry: &'a [String],
    pub cwd: Option<&'a str>,
    pub workspace: Option<&'a str>,
}

/// Is this session the one sbxw attached? Any marker of a client is enough —
/// a client process in the ancestry (`server` is Claude Desktop's in-container
/// half, `sshd` a plain `ssh <name>.sbx`), sshd's environment — and the working
/// directory settles the rest. Silence is never evidence: a hook that reported
/// nothing leaves `Unknown`, not `Tty`.
pub fn classify_origin(ev: &OriginEvidence<'_>) -> SessionOrigin {
    let client = ev.ancestry.iter().any(|name| {
        let n = name.trim().to_ascii_lowercase();
        n == "server" || n == "sshd" || n == "ssh" || n.starts_with("sshd:")
    });
    if client || !ev.ssh_env.is_empty() {
        return SessionOrigin::Remote;
    }
    match classify_by_cwd(ev.cwd, ev.workspace) {
        SessionOrigin::Unknown if ev.hook_version >= 2 => SessionOrigin::Tty,
        other => other,
    }
}

/// The agent sbxw attaches starts in the recorded workspace; a client lands
/// wherever it chose. Weaker than sshd's environment and kept behind it: an
/// SSH session that `cd`s into the workspace reads as the tty's too — and
/// that misreading is the safe one, since two `Tty` sessions make a sandbox
/// read-only rather than typed into.
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
}

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
        let st = self.statuses.entry(key.clone()).or_default();
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
        let mut out = self.infos_for(sandbox, has_pty, now);
        if ended {
            if let Some(gone) = out.iter_mut().find(|i| i.session_id.as_deref().unwrap_or("") == key.session_id) {
                gone.state = SessionState::Exited;
            }
            self.statuses.remove(&key);
            // A session ending can make its sibling answerable again.
            out.extend(self.infos_for(sandbox, has_pty, now));
        }
        out
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
        self.infos_for(&key.sandbox, has_pty, now)
    }

    /// Drop every session whose sandbox is not in `running` — a sandbox
    /// stopped out of band sends no `SessionEnd`. Returns their `Exited`
    /// payloads. An empty `running` is a failed `sbx ls`, never "all gone".
    pub fn reconcile(&mut self, running: &[String], now: u64) -> Vec<SessionInfo> {
        if running.is_empty() {
            return Vec::new();
        }
        let gone: Vec<AgentKey> = self
            .statuses
            .keys()
            .filter(|k| !running.contains(&k.sandbox))
            .cloned()
            .collect();
        gone.into_iter()
            .filter_map(|k| {
                let st = self.statuses.remove(&k)?;
                let mut info = build(&k, &st, false, now);
                info.state = SessionState::Exited;
                Some(info)
            })
            .collect()
    }

    /// Drop the sessions of `sandbox` running in thread `thread`'s worktree.
    /// Closing a thread kills its agent, which sends no `SessionEnd`. Returns
    /// their `Exited` payloads.
    pub fn forget_thread(&mut self, sandbox: &str, thread: &str, now: u64) -> Vec<SessionInfo> {
        let tail = format!("/.sbxw-worktrees/{thread}");
        let gone: Vec<AgentKey> = self
            .statuses
            .iter()
            .filter(|(k, st)| {
                k.sandbox == sandbox
                    && match st.thread.as_deref() {
                        Some(t) => t == thread,
                        None => st.cwd.as_deref().is_some_and(|c| c.trim_end_matches('/').ends_with(&tail)),
                    }
            })
            .map(|(k, _)| k.clone())
            .collect();
        gone.into_iter()
            .filter_map(|k| {
                let st = self.statuses.remove(&k)?;
                let mut info = build(&k, &st, false, now);
                info.state = SessionState::Exited;
                Some(info)
            })
            .collect()
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
        book.apply("neos", &ev(&start), SessionOrigin::Tty, true, 1);
        let out = book.apply("neos", &ev(&prompt), SessionOrigin::Tty, true, 2);
        assert_eq!(out[0].state, SessionState::Working);
        assert_eq!(out[0].last_input.as_deref(), Some("fix it"));
        let out = book.apply("neos", &ev(&stop), SessionOrigin::Tty, true, 3);
        assert_eq!(out[0].state, SessionState::Idle);
        assert_eq!(out[0].reply.as_deref(), Some("Done.\nAll good."));
        let out = book.apply("neos", &ev(&prompt), SessionOrigin::Unknown, true, 4);
        assert_eq!(out[0].reply, None, "a new prompt clears the reply");
        assert_eq!(out[0].origin, SessionOrigin::Tty, "unknown never demotes");
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
            })
        };
        assert_eq!(e(2, &none, &["node".into(), "server".into()], Some("/w")), SessionOrigin::Remote);
        assert_eq!(e(2, &["SSH_TTY".into()], &none, Some("/w")), SessionOrigin::Remote);
        assert_eq!(e(2, &none, &["node".into(), "claude".into()], Some("/w/")), SessionOrigin::Tty);
        assert_eq!(e(1, &none, &none, Some("/elsewhere")), SessionOrigin::Remote);
        assert_eq!(e(1, &none, &none, None), SessionOrigin::Unknown);
        assert_eq!(e(2, &none, &none, None), SessionOrigin::Tty);
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
