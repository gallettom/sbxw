//! What sbxw itself typed into a sandbox's sessions.
//!
//! A line sbxw types (a team note, "main moved", a relay notice, a briefing…)
//! reaches the agent as a plain user turn: Claude Code writes it in the
//! transcript like the person's own words, with no tag. The only reliable
//! way to tell them apart is for the daemon to record, when it types, that
//! the line is its own; the history and the session info then match the
//! turn they read against that record.
//!
//! The match is exact (the whole text, whitespace folded), never a prefix
//! test: a person or an agent whose text merely starts with "[sbxw]" keeps
//! their words. Limits, by design: the record is per sandbox (the daemon
//! knows the sandbox and the kind of session it typed into, not the agent's
//! session id), it lives in memory (a daemon restart forgets it) and an
//! entry expires after [`TTL_MS`].

use std::collections::{BTreeMap, VecDeque};

/// Who an entry of the history is from when sbxw typed it.
pub const SENDER: &str = "sbxw";

/// How long a recorded line is remembered: long enough for a transcript
/// posted late (a hook catching up) to still match.
pub const TTL_MS: u64 = 2 * 60 * 60 * 1000;

/// Lines kept per sandbox.
pub const PER_SANDBOX_MAX: usize = 64;

/// Sandboxes kept.
pub const SANDBOXES_MAX: usize = 256;

#[derive(Debug, Default, Clone)]
pub struct TypedLog {
    by_sandbox: BTreeMap<String, VecDeque<(u64, String)>>,
}

/// The text with every run of whitespace folded to one space: the form two
/// copies of the same line compare in (a terminal may re-wrap, a transcript
/// may trim).
fn fold(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

impl TypedLog {
    pub fn new() -> Self {
        Self::default()
    }

    /// Sbxw is about to type `text` into one of `sandbox`'s sessions.
    pub fn note(&mut self, sandbox: &str, text: &str, now: u64) {
        let text = fold(&crate::terminal::sanitize_typed(text));
        if text.is_empty() {
            return;
        }
        if !self.by_sandbox.contains_key(sandbox) && self.by_sandbox.len() >= SANDBOXES_MAX {
            let oldest = self.by_sandbox.iter().min_by_key(|(_, v)| v.back().map_or(0, |e| e.0)).map(|(k, _)| k.clone());
            if let Some(k) = oldest {
                self.by_sandbox.remove(&k);
            }
        }
        let q = self.by_sandbox.entry(sandbox.to_string()).or_default();
        q.retain(|(at, _)| now.saturating_sub(*at) < TTL_MS);
        q.push_back((now, text));
        while q.len() > PER_SANDBOX_MAX {
            q.pop_front();
        }
    }

    /// Did sbxw type exactly `text` into `sandbox` (lately)? Not consumed:
    /// the hook event and the transcript both read the same turn.
    pub fn typed(&self, sandbox: &str, text: &str, now: u64) -> bool {
        let Some(q) = self.by_sandbox.get(sandbox) else { return false };
        let text = fold(text);
        !text.is_empty() && q.iter().any(|(at, t)| now.saturating_sub(*at) < TTL_MS && *t == text)
    }

    /// Forget a sandbox (it was removed).
    pub fn drop_sandbox(&mut self, sandbox: &str) {
        self.by_sandbox.remove(sandbox);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_exact_line_matches_a_prefix_does_not() {
        let mut t = TypedLog::new();
        t.note("sb", "[sbxw] main moved: a merged.", 10);
        assert!(t.typed("sb", "[sbxw] main moved: a merged.", 20));
        assert!(t.typed("sb", "  [sbxw] main   moved: a\nmerged. ", 20), "whitespace folds");
        assert!(!t.typed("sb", "[sbxw] hello there", 20));
        assert!(!t.typed("sb", "[sbxw] main moved: a merged. And more", 20));
        assert!(!t.typed("other", "[sbxw] main moved: a merged.", 20));
        assert!(!t.typed("sb", "", 20));
    }

    #[test]
    fn a_line_expires_and_the_record_is_bounded() {
        let mut t = TypedLog::new();
        t.note("sb", "x", 0);
        assert!(t.typed("sb", "x", TTL_MS - 1));
        assert!(!t.typed("sb", "x", TTL_MS));
        for i in 0..PER_SANDBOX_MAX + 5 {
            t.note("sb", &format!("line {i}"), 1);
        }
        assert!(!t.typed("sb", "line 0", 2));
        assert!(t.typed("sb", &format!("line {}", PER_SANDBOX_MAX + 4), 2));
        t.drop_sandbox("sb");
        assert!(!t.typed("sb", "line 70", 2));
    }
}
