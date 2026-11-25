//! What a session is told as it starts, through the status hook's answer to
//! its `SessionStart` (`additionalContext`): the one way sbxw hands a new
//! conversation text without typing into it.
//!
//! [`App::session_start_context`] gathers what each source has pending for
//! the session that just started. Today one source: the lead's instructions
//! after a `/clear` or a compaction in the lead's terminal (D3 of the
//! reliability audit). The session's inbox (svc::inbox: team notes, merge
//! results, relay answers) is added by the hook route after it, within what
//! is left of [`CONTEXT_MAX_CHARS`].

use crate::app::App;
use sbxw_core::plans::PlanState;
use sbxw_core::SandboxRef;
use serde_json::Value;

/// Claude Code keeps a hook's `additionalContext` whole up to this size (an
/// assumption: Claude Code's documented cap on hook output in context is
/// taken to be 10,000 characters, not verified from this sandbox); the
/// lead's instructions go by their path when they would not fit, and an
/// inbox message by its head and a file.
pub const CONTEXT_MAX_CHARS: usize = 9_500;

impl App {
    /// The context a session that just started in `sandbox` must be given,
    /// from the hook event `ev` (after `ingest_hook` filed it), or `None`.
    /// Never fails: a source with nothing to say adds nothing.
    pub fn session_start_context(&self, sandbox: &SandboxRef, ev: &Value) -> Option<String> {
        if ev["hook_event_name"] != "SessionStart" || ev.get("background").is_some_and(|b| !b.is_null()) {
            return None;
        }
        let pieces: Vec<String> = [self.lead_start_context(sandbox, ev)].into_iter().flatten().collect();
        (!pieces.is_empty()).then(|| pieces.join("\n\n"))
    }

    /// The lead's main session started a new conversation in its own
    /// terminal: the lead follows it ([`App::lead_follow_conversation`]),
    /// and the conversation gets lead.md, the open plans and the order to
    /// call `lead_ack`. A thread's session (`SBXW_THREAD` set) never.
    fn lead_start_context(&self, sandbox: &SandboxRef, ev: &Value) -> Option<String> {
        if ev.get("sbxw_thread").and_then(Value::as_str).is_some_and(|t| !t.is_empty()) {
            return None;
        }
        let sid = ev["session_id"].as_str().filter(|s| !s.is_empty())?;
        let compacted = ev["source"] == "compact";
        if !self.lead_follow_conversation(sandbox, sid, compacted) {
            return None;
        }
        let me = sandbox.to_string();
        let plans: Vec<_> = self
            .plans
            .lock()
            .unwrap()
            .list()
            .into_iter()
            .filter(|p| p.sandbox == me && !p.closed && matches!(p.state, PlanState::Pending | PlanState::Approved))
            .collect();
        Some(sbxw_core::messages::lead_context(
            &plans,
            sbxw_core::tooling::LEAD_COMMAND,
            sbxw_core::tooling::LEAD_COMMAND_PATH,
            CONTEXT_MAX_CHARS,
        ))
    }
}
