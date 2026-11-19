//! Each session's conversation, for the Monitor: fed by hook events and by
//! the status hook's transcript posts, read page by page from the UI
//! listener. The rules (kinds, bounds, masking) are `sbxw_core::history`'s.
//!
//! In memory only, and never in a log line: what is logged is counts and
//! sizes. Every text is masked on ingest with the person's anonymise rules;
//! rules that do not parse fall back to the built-in shapes (never to
//! nothing).

use crate::app::{now_ms, App};
use crate::bus::{Event, HistoryNudge};
use sbxw_core::agents::{HookEvent, SessionInfo};
use sbxw_core::anonymise::Rules;
use sbxw_core::history::{limits, Page, Posted};
use sbxw_core::SandboxRef;
use serde_json::Value;

/// Why a transcript post was refused.
#[derive(Debug, PartialEq, Eq)]
pub enum PostError {
    /// More items than [`limits::POST_ITEMS_MAX`].
    TooLarge,
    /// No session id.
    NoSession,
}

impl App {
    /// What history texts are masked with: the person's rules, else the shapes.
    fn history_rules(&self) -> Rules {
        self.anonymise_rules().unwrap_or_default()
    }

    fn nudge(&self, sandbox: &str, session_id: &str, seq: Option<u64>) {
        if let Some(seq) = seq {
            self.send(Event::History(HistoryNudge { sandbox: sandbox.to_string(), session_id: session_id.to_string(), seq }));
        }
    }

    /// A hook event, into its session's history (after the session state).
    pub(crate) fn history_event(&self, sandbox: &SandboxRef, body: &Value) {
        let ev = HookEvent::new(body);
        if ev.event().is_empty() || ev.is_background() {
            return;
        }
        let rules = self.history_rules();
        let sb = sandbox.to_string();
        let seq = {
            let mut book = self.history.lock().unwrap();
            book.expire(now_ms());
            book.ingest_event(&sb, &ev, &rules, now_ms())
        };
        self.nudge(&sb, ev.session_id(), seq);
    }

    /// A transcript post from `sandbox`'s hook (its identity, from its
    /// token): accepted up to a byte offset, or out of step.
    pub fn history_post(&self, sandbox: &SandboxRef, post: &Value) -> Result<Posted, PostError> {
        let session_id = post.get("session_id").and_then(Value::as_str).unwrap_or("");
        if session_id.is_empty() || session_id.len() > 128 {
            return Err(PostError::NoSession);
        }
        let n = post.get("items").and_then(Value::as_array).map_or(0, Vec::len);
        if n > limits::POST_ITEMS_MAX {
            return Err(PostError::TooLarge);
        }
        let rules = self.history_rules();
        let sb = sandbox.to_string();
        let (res, total, sessions) = {
            let mut book = self.history.lock().unwrap();
            book.expire(now_ms());
            let res = book.ingest_transcript(&sb, post, &rules, now_ms());
            (res, book.total_bytes(), book.sessions())
        };
        match &res {
            Posted::Accepted { nudge, .. } => {
                tracing::debug!("history: sandbox={sb} items={n} held={total}B in {sessions} sessions");
                self.nudge(&sb, session_id, *nudge);
            }
            Posted::OutOfStep { next } => tracing::debug!("history: sandbox={sb} out of step, asks from {next}"),
        }
        Ok(res)
    }

    /// A page of a session's history; reading it subscribes to its nudges.
    pub fn history_page(&self, sandbox: &SandboxRef, session_id: &str, before: Option<u64>, limit: Option<usize>) -> Option<Page> {
        let mut book = self.history.lock().unwrap();
        book.expire(now_ms());
        book.page(&sandbox.to_string(), session_id, before, limit, now_ms())
    }

    pub fn history_unsubscribe(&self, sandbox: &SandboxRef, session_id: &str) -> bool {
        self.history.lock().unwrap().unsubscribe(&sandbox.to_string(), session_id)
    }

    /// A session's last payload went out: its history expires from now.
    pub(crate) fn history_ended(&self, info: &SessionInfo) {
        let id = info.session_id.as_deref().unwrap_or("");
        self.history.lock().unwrap().end(&info.sandbox, id, now_ms());
    }

    /// A removed sandbox: its sessions' histories go at once.
    pub(crate) fn history_forget(&self, sandbox: &SandboxRef) {
        self.history.lock().unwrap().drop_sandbox(&sandbox.to_string());
    }
}
