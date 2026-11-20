//! Who a paired phone can reach: the ONE rule (API.md 2.4).
//!
//! 1. Every running team leader: the main session of a sandbox that leads a
//!    team (the daemon's `App::leading`), kind [`ReachKind::Leader`].
//! 2. The main session of every running sandbox that leads no team, kind
//!    [`ReachKind::Sandbox`].
//!
//! A sandbox's main session is its newest session that is not a thread's and
//! not a harness background session. Never reached: a thread (it is reached
//! through its leader), a background session, a stopped sandbox (its
//! sessions are gone or exited), and the Jarvis sandbox, by name on any host
//! (it has its own channel, decided by the person). Every route of the
//! remote listener that takes a `:sandbox` resolves through this list, so a
//! sandbox outside it answers exactly as an unknown one.

use crate::agents::{RecordKind, SessionInfo, SessionState};
use crate::SandboxRef;
use serde::Serialize;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ReachKind {
    Leader,
    Sandbox,
}

/// What the rule needs to know of one session.
#[derive(Clone, Copy, Debug)]
pub struct Candidate<'a> {
    pub sandbox: &'a str,
    pub thread: bool,
    pub background: bool,
    pub exited: bool,
    pub started_ms: u64,
}

impl<'a> From<&'a SessionInfo> for Candidate<'a> {
    fn from(i: &'a SessionInfo) -> Self {
        Candidate {
            sandbox: &i.sandbox,
            thread: i.thread.is_some(),
            background: i.kind == Some(RecordKind::Background),
            exited: i.state == SessionState::Exited,
            started_ms: i.started_ms,
        }
    }
}

/// One reachable sandbox: its name, its kind, and which candidate is the
/// session the phone shows and types into.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reach {
    pub sandbox: String,
    pub kind: ReachKind,
    pub index: usize,
}

/// Never reached from a phone, whatever runs in it: the Jarvis sandbox (by
/// name, on any host: the name is reserved everywhere) and a name that does
/// not parse.
pub fn excluded(sandbox: &str) -> bool {
    match SandboxRef::parse(sandbox) {
        Ok(sb) => sb.name.as_str() == crate::jarvis::SANDBOX,
        Err(_) => true,
    }
}

/// The main session of `sandbox`: the newest one that is not a thread's and
/// not a background session, a live one before an exited one.
pub fn main_index(cands: &[Candidate], sandbox: &str) -> Option<usize> {
    cands
        .iter()
        .enumerate()
        .filter(|(_, c)| c.sandbox == sandbox && !c.thread && !c.background)
        .max_by_key(|(_, c)| (!c.exited, c.started_ms))
        .map(|(i, _)| i)
}

/// Every sandbox a phone reaches: the leaders (in `leading`'s order) first,
/// then the other running sandboxes by name.
pub fn reachable(cands: &[Candidate], leading: &[String]) -> Vec<Reach> {
    let mut out: Vec<Reach> = Vec::new();
    for name in leading {
        if excluded(name) || out.iter().any(|r| &r.sandbox == name) {
            continue;
        }
        if let Some(index) = main_index(cands, name) {
            out.push(Reach { sandbox: name.clone(), kind: ReachKind::Leader, index });
        }
    }
    let mut names: Vec<&str> = cands.iter().map(|c| c.sandbox).collect();
    names.sort_unstable();
    names.dedup();
    for name in names {
        if excluded(name) || leading.iter().any(|l| l == name) {
            continue;
        }
        // Running: its main session has not exited (a stopped sandbox's
        // sessions are evicted, with an exit, by the daemon's reconcile).
        if let Some(index) = main_index(cands, name).filter(|&i| !cands[i].exited) {
            out.push(Reach { sandbox: name.to_string(), kind: ReachKind::Sandbox, index });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(sandbox: &str, started_ms: u64) -> Candidate<'_> {
        Candidate { sandbox, thread: false, background: false, exited: false, started_ms }
    }

    fn names(r: &[Reach]) -> Vec<(&str, ReachKind)> {
        r.iter().map(|r| (r.sandbox.as_str(), r.kind)).collect()
    }

    #[test]
    fn leaders_first_then_running_sandboxes_by_name() {
        let cands = [c("zeta", 1), c("lead", 2), c("alpha", 3)];
        let r = reachable(&cands, &["lead".to_string()]);
        assert_eq!(names(&r), vec![("lead", ReachKind::Leader), ("alpha", ReachKind::Sandbox), ("zeta", ReachKind::Sandbox)]);
        assert_eq!(r[0].index, 1);
    }

    #[test]
    fn a_thread_and_a_background_session_are_never_a_main_session() {
        let thread = Candidate { thread: true, ..c("app", 9) };
        let bg = Candidate { background: true, ..c("app", 10) };
        let cands = [c("app", 1), thread, bg];
        let r = reachable(&cands, &[]);
        assert_eq!(names(&r), vec![("app", ReachKind::Sandbox)]);
        assert_eq!(r[0].index, 0, "the main session, not the newer thread or background one");
        // A sandbox with only a thread's or a background session: not reached.
        let only = [Candidate { thread: true, ..c("t", 1) }, Candidate { background: true, ..c("b", 1) }];
        assert!(reachable(&only, &[]).is_empty());
        // The same for a leader: its shown session is never a background one.
        let cands = [c("lead", 1), Candidate { background: true, ..c("lead", 5) }];
        assert_eq!(reachable(&cands, &["lead".into()])[0].index, 0);
    }

    #[test]
    fn a_stopped_sandbox_is_not_reached() {
        let cands = [Candidate { exited: true, ..c("gone", 1) }];
        assert!(reachable(&cands, &[]).is_empty());
        // A live session wins over an older exited one.
        let cands = [Candidate { exited: true, ..c("app", 5) }, c("app", 1)];
        assert_eq!(reachable(&cands, &[])[0].index, 1);
    }

    #[test]
    fn jarvis_is_excluded_as_a_sandbox_and_as_a_leader() {
        let j = crate::jarvis::SANDBOX;
        let cands = [c(j, 1), c("app", 1)];
        assert_eq!(names(&reachable(&cands, &[j.to_string()])), vec![("app", ReachKind::Sandbox)]);
        assert!(excluded(j));
        assert!(excluded("not a name!"));
        assert!(!excluded("app"));
    }

    #[test]
    fn a_leader_with_no_main_session_is_not_listed_and_a_leader_is_listed_once() {
        let cands = [c("app", 1)];
        let r = reachable(&cands, &["ghost".into(), "app".into(), "app".into()]);
        assert_eq!(names(&r), vec![("app", ReachKind::Leader)]);
    }
}
