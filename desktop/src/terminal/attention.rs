//! A pane's "wants you": its OWN session only, and only while it holds.
//!
//! Two signals, both per session (never per sandbox):
//! * the session's hook state (`GET /api/sessions`, refreshed on each
//!   `session` event): `attention` means it waits for a person;
//! * the daemon's `attention` frame on this terminal's socket: a real BEL of
//!   this PTY (the daemon no longer counts the BEL that ends a window title).
//!
//! A pane with an agent session shows the badge while that session's state
//! is `attention` (reported after the person last typed in the pane), or
//! while its bell rang and the session has not reported working since. A pane
//! with no session (a plain shell, or no hook reports) keeps the bell rule:
//! lit by a bell, cleared by typing. A new session in the pane is a new
//! [`TerminalState`](super::TerminalState), so nothing carries over.

use serde_json::Value as J;

/// The latest live hook report of one session: its state and `ts`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionSeen {
    pub state: String,
    pub ts: u64,
}

/// The session a pane shows, in `GET /api/sessions` (`sessions`): same
/// sandbox, same thread (`None`: the main session), not exited, the latest
/// report. Only agent panes have one (`agent`).
pub fn session_for(sessions: Option<&J>, sandbox: Option<&str>, thread: Option<&str>, agent: bool) -> Option<SessionSeen> {
    if !agent {
        return None;
    }
    let sandbox = sandbox?;
    sessions?
        .as_array()?
        .iter()
        .filter(|i| i["sandbox"] == sandbox && i["state"] != "exited" && crate::feat::threads::model::thread_of(i).as_deref() == thread)
        .max_by_key(|i| i["ts"].as_u64().unwrap_or(0))
        .map(|i| SessionSeen { state: i["state"].as_str().unwrap_or("").to_string(), ts: i["ts"].as_u64().unwrap_or(0) })
}

/// What one terminal remembers to answer "does it want you?".
#[derive(Clone, Debug, Default)]
pub(super) struct Attention {
    /// The bell rang; the session's `ts` then (0: none seen).
    bell: Option<u64>,
    /// The person typed; the session's `ts` then.
    typed: Option<u64>,
    /// The latest session `ts` seen.
    seen: u64,
}

impl Attention {
    pub(super) fn bell(&mut self) {
        self.bell = Some(self.seen);
    }

    pub(super) fn typed(&mut self) {
        self.bell = None;
        self.typed = Some(self.seen);
    }

    pub(super) fn wants_you(&mut self, session: Option<&SessionSeen>) -> bool {
        let Some(s) = session else { return self.bell.is_some() };
        self.seen = self.seen.max(s.ts);
        // Working again since the bell: the bell is answered.
        if s.state == "working" && self.bell.is_some_and(|b| s.ts > b) {
            self.bell = None;
        }
        let asks = s.state == "attention" && self.typed.is_none_or(|t| s.ts > t);
        asks || self.bell.is_some()
    }
}
