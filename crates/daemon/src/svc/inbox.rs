//! The one way sbxw hands an agent session something of its own accord:
//! [`App::deliver`] files it in the session's persisted inbox
//! (`sbxw_core::inbox`, `state/inbox.json`), and the session's status hook
//! prints it (D15 of the reliability audit). Nothing of a message is typed.
//!
//! In order of preference:
//!
//! 1. **A hook answer.** The status hook posts every event and prints the
//!    daemon's answer. On `PostToolUse` (the main conversation's, never a
//!    subagent's), `UserPromptSubmit` and `SessionStart` the waiting messages
//!    go as `additionalContext`; on `Stop`, as the reason that keeps the
//!    agent going (`decision: "block"`). A busy agent gets them at its next
//!    tool call; one ending its turn, before it goes idle.
//! 2. **One fixed line, only when the session is idle at its prompt** and no
//!    hook event will come: [`NUDGE_LINE`] starts a turn, and that turn's
//!    `UserPromptSubmit` carries the content. The line is sbxw's own and
//!    never holds a message's words. Same idle rule as the phone's
//!    (`remote_can_type`, svc/remote_access.rs).
//! 3. **A session that is not running**: the messages wait for its next
//!    `SessionStart`.
//!
//! A message leaves the inbox only when the hook that printed it says so:
//! a second small call after its stdout is written (`/api/hook/inbox_ack`).
//! Not the next event of the session: an idle session may send none for
//! hours, and an event proves the hook ran, not that it printed. An ack
//! lost on the way costs a second offer (after `REOFFER_MS`), never the
//! message.

use crate::app::{now_ms, App};
use crate::bus::Event;
use crate::pty::PtySession;
use sbxw_core::agents::{classify_origin, HookEvent, OriginEvidence, SessionOrigin, SessionState};
use sbxw_core::inbox::{Book, Kind, Offer, Refused, To};
use sbxw_core::session::{SessionKey, SessionKind};
use sbxw_core::{SandboxRef, ThreadName};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

/// The line typed into an idle session that has messages: sbxw's words
/// only. The turn it starts brings the messages, by its hook.
pub const NUDGE_LINE: &str = "[sbxw] sbxw has messages for you: they come with this turn, from sbxw's hook.";
/// The status hook version that prints an inbox and acknowledges it. An
/// older hook is never offered one: it would print the ack fields too.
pub const MIN_HOOK_VERSION: u64 = 5;
/// The same session is typed [`NUDGE_LINE`] at most this often.
const NUDGE_EVERY_MS: u64 = 60_000;
/// A session's last hook event is this old before it reads as settled.
const SETTLE_MS: u64 = 2_000;
/// How often the waiting inboxes are looked at without a reason to.
const TICK: Duration = Duration::from_secs(15);

/// What the daemon knows of a recipient's hook, from its latest event.
#[derive(Clone, Debug, Default)]
pub(crate) struct Seen {
    pub hook_version: u64,
    pub event_ms: u64,
    pub nudged_ms: u64,
}

/// An offer, with the session it is for: what the hook route answers.
pub struct HookInbox {
    pub session: String,
    pub offer: Offer,
}

pub(crate) fn load(path: &Path) -> Book {
    match std::fs::read_to_string(path) {
        Ok(t) => Book::parse_lenient(&t).unwrap_or_else(|| {
            tracing::warn!("inbox: could not read {}; starting empty", path.display());
            Book::default()
        }),
        Err(_) => Book::default(),
    }
}

/// The recipient a session kind names.
pub fn to_of(kind: &SessionKind) -> Option<To> {
    match kind {
        SessionKind::Agent => Some(To::Main),
        SessionKind::Thread(t) => Some(To::Thread(t.to_string())),
        _ => None,
    }
}

fn key_of(sandbox: &SandboxRef, to: &To) -> Option<SessionKey> {
    match to {
        To::Main => Some(SessionKey::agent(sandbox)),
        To::Thread(t) => ThreadName::parse(t).ok().map(|t| SessionKey::thread(sandbox, &t)),
    }
}

/// The hook's answer: `pieces` of context (another source's, then the
/// inbox's) for event `event`, and the inbox ids the hook acknowledges.
/// `{}` when there is nothing to say.
pub fn hook_answer(event: &str, mut pieces: Vec<String>, inbox: Option<HookInbox>) -> Value {
    if let Some(i) = &inbox {
        pieces.push(i.offer.context.clone());
    }
    pieces.retain(|p| !p.is_empty());
    if pieces.is_empty() {
        return json!({});
    }
    let text = pieces.join("\n\n");
    let mut out = match event {
        "Stop" => json!({ "decision": "block", "reason": text }),
        _ => json!({ "hookSpecificOutput": { "hookEventName": event, "additionalContext": text } }),
    };
    if let Some(i) = inbox {
        let files: Vec<Value> = i.offer.files.iter().map(|(name, text)| json!({ "name": name, "text": text })).collect();
        out["sbxw_inbox"] = json!({ "session_id": i.session, "ids": i.offer.ids, "files": files });
    }
    out
}

impl App {
    pub(crate) fn inbox_file(&self) -> PathBuf {
        self.store.paths.state().join("inbox.json")
    }

    pub(crate) fn persist_inbox(&self) {
        let bytes = serde_json::to_vec_pretty(&*self.inbox.lock().unwrap());
        let path = self.inbox_file();
        if let Err(e) = bytes.map_err(anyhow::Error::from).and_then(|b| crate::store::write_atomic(&path, &b)) {
            tracing::warn!("inbox: could not save {}: {e:#}", path.display());
        }
    }

    /// Hand `text` to the session `to` of `sandbox` (its main session, or a
    /// thread): filed in its inbox, printed by its status hook at its next
    /// tool call, prompt, start or stop; an idle session is typed one fixed
    /// line so that a turn starts. `kind` and `source` (who it is from, in
    /// sbxw's words: `relay request r-4`, `thread api`, `sbxw`) are shown
    /// with it. `key`: a message with the same key and kind still waiting
    /// for the same session is replaced rather than doubled. Returns the
    /// message's id. Refused for a session kind that is not an agent, an
    /// empty text, or a full inbox.
    pub fn deliver(&self, sandbox: &SandboxRef, to: &SessionKind, kind: Kind, source: &str, text: &str, key: Option<&str>) -> Result<String, String> {
        let to = to_of(to).ok_or("only an agent session has an inbox")?;
        let id = self
            .inbox
            .lock()
            .unwrap()
            .add(&sandbox.to_string(), to.clone(), kind, source, text, key, now_ms())
            .map_err(|e| match e {
                Refused::Empty => "nothing to deliver".to_string(),
                Refused::Full => format!("the inbox of the {to} of '{sandbox}' is full"),
            })?;
        self.persist_inbox();
        tracing::info!("inbox: {id} ({}) from {source} for the {to} of '{sandbox}'", kind.label());
        // The idle-session loop (started with the daemon) looks again.
        self.inbox_wake.notify_one();
        Ok(id)
    }

    /// [`App::deliver`], logging a refusal: for callers with nobody to tell.
    pub(crate) fn deliver_or_log(&self, sandbox: &SandboxRef, to: &SessionKind, kind: Kind, source: &str, text: &str, key: Option<&str>) {
        if let Err(e) = self.deliver(sandbox, to, kind, source, text, key) {
            tracing::warn!("inbox: could not deliver a {} to '{sandbox}': {e}", kind.label());
        }
    }

    /// Which inbox a hook event's session reads: a thread's (`sbxw_thread`),
    /// or the main session's when the event comes from the terminal sbxw
    /// attached. Never a subagent's call, a background session's, or a
    /// session driven over SSH: those are not the recipient's conversation.
    pub(crate) fn inbox_recipient(&self, sandbox: &SandboxRef, ev: &Value) -> Option<To> {
        let he = HookEvent::new(ev);
        if he.is_background() || he.harness_background() || he.subagent() {
            return None;
        }
        let thread = ev["sbxw_thread"].as_str().map(str::trim).filter(|t| !t.is_empty());
        if let Some(t) = thread {
            return ThreadName::parse(t).ok().map(|t| To::Thread(t.to_string()));
        }
        let workspace = self.store.workspace(sandbox);
        let origin = classify_origin(&OriginEvidence {
            hook_version: he.hook_version(),
            ssh_env: &he.strings("ssh_env"),
            ancestry: &he.strings("ancestry"),
            cwd: he.cwd(),
            workspace: workspace.as_ref().and_then(|p| p.to_str()),
            thread: None,
        });
        (origin == SessionOrigin::Tty).then_some(To::Main)
    }

    /// Every hook event, before its answer: what the recipient's hook is and
    /// when it last spoke (for the nudge), and a `read_team_notes` call that
    /// came back (the notes it handed are read: svc::team).
    pub(crate) fn inbox_hook_seen(&self, sandbox: &SandboxRef, ev: &Value) {
        let Some(to) = self.inbox_recipient(sandbox, ev) else { return };
        let version = HookEvent::new(ev).hook_version();
        {
            let mut seen = self.inbox_seen.lock().unwrap();
            let s = seen.entry((sandbox.to_string(), to.clone())).or_default();
            s.hook_version = version;
            s.event_ms = now_ms();
        }
        if ev["hook_event_name"] == "PostToolUse" && ev["tool_name"].as_str().is_some_and(|t| t.ends_with("read_team_notes")) {
            self.team_notes_handed(sandbox, &to);
        }
    }

    /// The hook version the recipient last reported, when known.
    pub(crate) fn inbox_hook_version(&self, sandbox: &SandboxRef, to: &To) -> Option<u64> {
        self.inbox_seen.lock().unwrap().get(&(sandbox.to_string(), to.clone())).map(|s| s.hook_version)
    }

    /// What the hook answering `ev` prints of the session's inbox, within
    /// `budget` characters, and the offer recorded. `None` for an event that
    /// cannot carry context, a hook too old to acknowledge, or nothing waiting.
    pub fn inbox_offer(&self, sandbox: &SandboxRef, ev: &Value, budget: usize) -> Option<HookInbox> {
        let event = ev["hook_event_name"].as_str().unwrap_or("");
        if !matches!(event, "PostToolUse" | "UserPromptSubmit" | "SessionStart" | "Stop") {
            return None;
        }
        if HookEvent::new(ev).hook_version() < MIN_HOOK_VERSION {
            return None;
        }
        let to = self.inbox_recipient(sandbox, ev)?;
        let session = ev["session_id"].as_str()?;
        // A Stop that already kept the agent going: only what is new, so an
        // ack that never comes cannot keep it going for ever.
        let fresh_only = event == "Stop" && ev["stop_hook_active"].as_bool().unwrap_or(false);
        let offer = self.inbox.lock().unwrap().offer(&sandbox.to_string(), &to, session, now_ms(), budget.max(2_000), fresh_only)?;
        self.persist_inbox();
        tracing::info!("inbox: offered {} to the {to} of '{sandbox}' on {event}", offer.ids.join(", "));
        Some(HookInbox { session: session.to_string(), offer })
    }

    /// The hook of `session` printed `ids`: they leave the inbox. Only those
    /// offered last to that session of `sandbox`. Returns how many left.
    pub fn inbox_ack(&self, sandbox: &SandboxRef, session: &str, ids: &[String]) -> usize {
        let gone = self.inbox.lock().unwrap().ack(&sandbox.to_string(), session, ids);
        if gone.is_empty() {
            return 0;
        }
        self.persist_inbox();
        for m in &gone {
            tracing::info!("inbox: {} delivered to the {} of '{sandbox}'", m.id, m.to);
            if m.kind == Kind::TeamNote {
                self.team_note_delivered(sandbox, &m.to, m.key.as_deref());
            }
        }
        gone.len()
    }

    /// Start the loop that types [`NUDGE_LINE`] into idle sessions with
    /// messages; once per daemon (a second call does nothing).
    pub(crate) fn inbox_start(self: &Arc<Self>) {
        if self.inbox_running.swap(true, std::sync::atomic::Ordering::SeqCst) {
            return;
        }
        if tokio::runtime::Handle::try_current().is_err() {
            self.inbox_running.store(false, std::sync::atomic::Ordering::SeqCst);
            return;
        }
        let app = self.clone();
        tokio::spawn(async move {
            let mut rx = app.bus.subscribe();
            loop {
                tokio::select! {
                    _ = app.inbox_wake.notified() => {}
                    r = rx.recv() => match r {
                        Ok(Event::Session(_)) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                        Ok(_) => continue,
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
                    },
                    _ = tokio::time::sleep(TICK) => {}
                }
                // Let the event that woke us be answered first: a Stop's own
                // hook may be carrying the messages right now.
                tokio::time::sleep(Duration::from_millis(SETTLE_MS)).await;
                app.inbox_pass();
            }
        });
    }

    /// One look at every inbox with something offerable: an idle session is
    /// typed the fixed line, at most once per [`NUDGE_EVERY_MS`]. A session
    /// whose hook version is unknown (a daemon restart) is typed it once; an
    /// older hook, never.
    fn inbox_pass(self: &Arc<Self>) {
        let now = now_ms();
        let waiting = self.inbox.lock().unwrap().waiting(now);
        for (sb, to) in waiting {
            let Ok(sandbox) = SandboxRef::parse(&sb) else { continue };
            let seen = self.inbox_seen.lock().unwrap().get(&(sb.clone(), to.clone())).cloned();
            match &seen {
                Some(s) if s.hook_version < MIN_HOOK_VERSION && s.event_ms > 0 => continue,
                Some(s) if now.saturating_sub(s.nudged_ms) < NUDGE_EVERY_MS => continue,
                Some(s) if now.saturating_sub(s.event_ms) < SETTLE_MS => continue,
                _ => {}
            }
            let Some(session) = self.inbox_idle_session(&sandbox, &to) else { continue };
            self.inbox_seen.lock().unwrap().entry((sb.clone(), to.clone())).or_default().nudged_ms = now;
            tracing::info!("inbox: the {to} of '{sb}' is idle with messages: typing the fixed line");
            let app = self.clone();
            tokio::spawn(async move {
                app.note_typed(&sandbox.to_string(), NUDGE_LINE);
                if let Err(e) = crate::typing::type_and_submit(&session, NUDGE_LINE, false).await {
                    tracing::warn!("inbox: could not type the fixed line into the {to} of '{sandbox}': {e}");
                }
            });
        }
    }

    /// The terminal of `to`, when typed text would land in its prompt now:
    /// the rule of the phone's `remote_can_type` (svc/remote_access.rs), for
    /// a thread as for the main session. Its one session idle (not working,
    /// not waiting for the person, no question or prompt open), its last
    /// history item neither a prompt nor a tool call under
    /// `TOOL_CALL_QUIET_MS`, its terminal live with bracketed paste on.
    /// Anything unknown: `None`.
    pub(crate) fn inbox_idle_session(&self, sandbox: &SandboxRef, to: &To) -> Option<Arc<PtySession>> {
        let key = key_of(sandbox, to)?;
        let pty = self.sessions.live(&key)?;
        let sb = sandbox.to_string();
        let all = self.agents.lock().unwrap().snapshot(|_| true, now_ms());
        let mine: Vec<_> = all
            .into_iter()
            .filter(|i| i.sandbox == sb && !i.ended)
            .filter(|i| match to {
                To::Main => i.thread.is_none() && i.origin == SessionOrigin::Tty,
                To::Thread(t) => i.thread.as_deref() == Some(t.as_str()),
            })
            .collect();
        let [info] = mine.as_slice() else { return None };
        if info.state != SessionState::Idle || info.question.is_some() || info.steps.as_ref().is_some_and(|s| !s.is_empty()) {
            return None;
        }
        let sid = info.session_id.as_deref()?;
        if let Some(last) = self.history_page(sandbox, sid, None, Some(1)).and_then(|p| p.items.last().cloned()) {
            use sbxw_core::history::Kind as H;
            match last.kind {
                H::Permission | H::Question => return None,
                H::ToolCall if now_ms().saturating_sub(last.at) < crate::svc::remote_access::TOOL_CALL_QUIET_MS => return None,
                _ => {}
            }
        }
        if !sbxw_core::terminal::bracketed_paste_enabled(&pty.replay()) {
            return None;
        }
        Some(pty)
    }
}

#[cfg(test)]
mod tests;
