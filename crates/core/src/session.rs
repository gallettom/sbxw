//! What a terminal session *is*, as a type.
//!
//! The old daemon keyed sessions by strings — `"<sb>::claude"`,
//! `"<sb>::thread:x"`, `"<sb>::thread-bash:x"`, `"__host__::monitor"` — and
//! re-parsed them with `split_key` wherever it needed to know which kind it
//! held. Here a key is a struct, and the string forms exist only at the edges
//! where the wire protocol names them (the WebSocket's `mode=` query and the
//! `mode` field the island reads).

use crate::names::{SandboxName, ThreadName};
use serde::Serialize;
use std::fmt;

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum SessionKind {
    /// The sandbox's main agent, attached with `sbx run`.
    Agent,
    /// A shell in the sandbox (`sbx exec -it … bash`, or SSH when stopped).
    Bash,
    /// Another Claude session, in a git worktree of its own.
    Thread(ThreadName),
    /// A shell in a thread's worktree. Not an agent: never counts against the
    /// session ceiling.
    ThreadBash(ThreadName),
    /// `monitor_cmd` on the host. Belongs to no sandbox.
    Monitor,
}

impl SessionKind {
    /// Whether this session is a Claude agent — what `max_agent_sessions`
    /// counts.
    pub fn is_agent(&self) -> bool {
        matches!(self, SessionKind::Agent | SessionKind::Thread(_))
    }

    pub fn thread(&self) -> Option<&ThreadName> {
        match self {
            SessionKind::Thread(t) | SessionKind::ThreadBash(t) => Some(t),
            _ => None,
        }
    }

    /// The `mode` this kind has on the wire (the old key's second half).
    pub fn mode(&self) -> String {
        match self {
            SessionKind::Agent => "claude".into(),
            SessionKind::Bash => "bash".into(),
            SessionKind::Thread(t) => format!("thread:{t}"),
            SessionKind::ThreadBash(t) => format!("thread-bash:{t}"),
            SessionKind::Monitor => "monitor".into(),
        }
    }

    /// The kind a WebSocket asks for: `mode` and `thread` as the page sends
    /// them. Unknown modes are the agent, as before; a thread with a bad name
    /// is refused rather than quietly opening the agent instead.
    pub fn from_query(mode: Option<&str>, thread: Option<&str>) -> Result<Self, &'static str> {
        let thread = thread.filter(|t| !t.is_empty());
        let named = || {
            thread
                .map(ThreadName::parse)
                .transpose()
                .map_err(|e| e.0)
        };
        Ok(match mode {
            Some("bash") => match named()? {
                Some(t) => SessionKind::ThreadBash(t),
                None => SessionKind::Bash,
            },
            Some("monitor") => SessionKind::Monitor,
            Some("thread") => match named()? {
                Some(t) => SessionKind::Thread(t),
                None => return Err(crate::names::INVALID_THREAD_NAME),
            },
            _ => SessionKind::Agent,
        })
    }
}

/// The pseudo-sandbox the monitor is filed under. The underscores are what
/// make it safe: no real sandbox name can contain one.
pub const MONITOR_SANDBOX: &str = "__host__";

/// One terminal session sbxw holds.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SessionKey {
    /// `None` only for the monitor.
    pub sandbox: Option<SandboxName>,
    pub kind: SessionKind,
}

impl SessionKey {
    pub fn new(sandbox: SandboxName, kind: SessionKind) -> Self {
        debug_assert!(kind != SessionKind::Monitor);
        Self {
            sandbox: Some(sandbox),
            kind,
        }
    }

    pub fn agent(sandbox: &SandboxName) -> Self {
        Self::new(sandbox.clone(), SessionKind::Agent)
    }

    pub fn thread(sandbox: &SandboxName, thread: &ThreadName) -> Self {
        Self::new(sandbox.clone(), SessionKind::Thread(thread.clone()))
    }

    pub fn monitor() -> Self {
        Self {
            sandbox: None,
            kind: SessionKind::Monitor,
        }
    }

    pub fn sandbox_str(&self) -> &str {
        self.sandbox.as_deref().unwrap_or(MONITOR_SANDBOX)
    }

    pub fn belongs_to(&self, sandbox: &str) -> bool {
        self.sandbox.as_deref() == Some(sandbox)
    }
}

impl fmt::Display for SessionKey {
    /// The old wire key, `<sandbox>::<mode>`, for logs and `/api/ptys`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}::{}", self.sandbox_str(), self.kind.mode())
    }
}

impl Serialize for SessionKey {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sb(n: &str) -> SandboxName {
        SandboxName::parse(n).unwrap()
    }
    fn th(n: &str) -> ThreadName {
        ThreadName::parse(n).unwrap()
    }

    #[test]
    fn the_wire_modes_are_the_old_ones() {
        assert_eq!(SessionKey::agent(&sb("neos")).to_string(), "neos::claude");
        assert_eq!(SessionKey::thread(&sb("neos"), &th("api")).to_string(), "neos::thread:api");
        assert_eq!(SessionKey::monitor().to_string(), "__host__::monitor");
        assert_eq!(
            SessionKey::new(sb("n"), SessionKind::ThreadBash(th("a"))).to_string(),
            "n::thread-bash:a"
        );
    }

    #[test]
    fn a_websocket_query_names_a_kind() {
        use SessionKind::*;
        assert_eq!(SessionKind::from_query(None, None), Ok(Agent));
        assert_eq!(SessionKind::from_query(Some("weird"), None), Ok(Agent));
        assert_eq!(SessionKind::from_query(Some("bash"), None), Ok(Bash));
        assert_eq!(SessionKind::from_query(Some("bash"), Some("")), Ok(Bash));
        assert_eq!(SessionKind::from_query(Some("bash"), Some("api")), Ok(ThreadBash(th("api"))));
        assert_eq!(SessionKind::from_query(Some("thread"), Some("api")), Ok(Thread(th("api"))));
        assert!(SessionKind::from_query(Some("thread"), None).is_err());
        assert!(SessionKind::from_query(Some("thread"), Some("Bad")).is_err());
        assert!(SessionKind::from_query(Some("bash"), Some("../x")).is_err());
        assert_eq!(SessionKind::from_query(Some("monitor"), None), Ok(Monitor));
    }

    #[test]
    fn only_agents_and_threads_count_against_the_ceiling() {
        assert!(SessionKind::Agent.is_agent());
        assert!(SessionKind::Thread(th("a")).is_agent());
        assert!(!SessionKind::ThreadBash(th("a")).is_agent());
        assert!(!SessionKind::Bash.is_agent());
        assert!(!SessionKind::Monitor.is_agent());
    }
}
