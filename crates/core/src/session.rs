//! What a terminal session *is*, as a type.
//!
//! The old daemon keyed sessions by strings — `"<sb>::claude"`,
//! `"<sb>::thread:x"`, `"<sb>::thread-bash:x"`, `"__host__::monitor"` — and
//! re-parsed them with `split_key` wherever it needed to know which kind it
//! held. Here a key is a struct, and the string forms exist only at the edges
//! where the wire protocol names them (the WebSocket's `mode=` query and the
//! `mode` field the island reads).

use crate::names::{SandboxRef, ThreadName};
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
    /// `sbx kit add` on the host, for a kit sbx wants a person to approve
    /// (0.51: new network rules) and that the daemon recorded as waiting.
    /// The person answers sbx's prompt in the pane.
    KitAdd,
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
            SessionKind::KitAdd => "kit-add".into(),
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
            Some("kit-add") => SessionKind::KitAdd,
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
    pub sandbox: Option<SandboxRef>,
    pub kind: SessionKind,
}

impl SessionKey {
    pub fn new(sandbox: impl Into<SandboxRef>, kind: SessionKind) -> Self {
        debug_assert!(kind != SessionKind::Monitor);
        Self {
            sandbox: Some(sandbox.into()),
            kind,
        }
    }

    pub fn agent(sandbox: impl Into<SandboxRef>) -> Self {
        Self::new(sandbox, SessionKind::Agent)
    }

    pub fn thread(sandbox: impl Into<SandboxRef>, thread: &ThreadName) -> Self {
        Self::new(sandbox, SessionKind::Thread(thread.clone()))
    }

    pub fn monitor() -> Self {
        Self {
            sandbox: None,
            kind: SessionKind::Monitor,
        }
    }

    /// The sandbox's ref in text form (`neos`, `buildbox~neos`), or the
    /// monitor's pseudo-sandbox.
    pub fn sandbox_str(&self) -> String {
        self.sandbox.as_ref().map_or_else(|| MONITOR_SANDBOX.to_string(), |s| s.to_string())
    }

    /// Whether this is a session of `sandbox`, given as a ref's text form.
    pub fn belongs_to(&self, sandbox: &str) -> bool {
        self.sandbox.as_ref().is_some_and(|s| s == sandbox)
    }

    pub fn belongs_to_ref(&self, sandbox: &SandboxRef) -> bool {
        self.sandbox.as_ref() == Some(sandbox)
    }
}

impl fmt::Display for SessionKey {
    /// The old wire key, `<sandbox>::<mode>`, for logs and `/api/ptys`; a
    /// remote sandbox's is `<host>~<sandbox>::<mode>`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}::{}", self.sandbox_str(), self.kind.mode())
    }
}

impl SessionKey {
    /// The key back from its wire form. The kind's thread name is checked
    /// like any other.
    pub fn parse(raw: &str) -> Result<Self, &'static str> {
        let (sb, mode) = raw.rsplit_once("::").ok_or("a session key is <sandbox>::<mode>")?;
        let kind = match mode {
            "claude" => SessionKind::Agent,
            "bash" => SessionKind::Bash,
            "monitor" => SessionKind::Monitor,
            m => match m.split_once(':') {
                Some(("thread", t)) => SessionKind::Thread(ThreadName::parse(t).map_err(|e| e.0)?),
                Some(("thread-bash", t)) => SessionKind::ThreadBash(ThreadName::parse(t).map_err(|e| e.0)?),
                _ => return Err("unknown session mode"),
            },
        };
        if kind == SessionKind::Monitor {
            return if sb == MONITOR_SANDBOX { Ok(Self::monitor()) } else { Err("only the host has a monitor") };
        }
        Ok(Self::new(SandboxRef::parse(sb).map_err(|e| e.0)?, kind))
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

    fn sb(n: &str) -> SandboxRef {
        SandboxRef::parse(n).unwrap()
    }
    fn th(n: &str) -> ThreadName {
        ThreadName::parse(n).unwrap()
    }

    #[test]
    fn the_wire_modes_are_the_old_ones() {
        assert_eq!(SessionKey::agent(sb("neos")).to_string(), "neos::claude");
        assert_eq!(SessionKey::thread(sb("neos"), &th("api")).to_string(), "neos::thread:api");
        assert_eq!(SessionKey::monitor().to_string(), "__host__::monitor");
        assert_eq!(
            SessionKey::new(sb("n"), SessionKind::ThreadBash(th("a"))).to_string(),
            "n::thread-bash:a"
        );
    }

    #[test]
    fn a_remote_key_names_its_host_and_round_trips() {
        let local = SessionKey::agent(sb("local~neos"));
        assert_eq!(local.to_string(), "neos::claude", "local is byte-identical to today");
        let remote = SessionKey::agent(sb("buildbox~neos"));
        assert_eq!(remote.to_string(), "buildbox~neos::claude");
        assert_ne!(local, remote);
        assert!(remote.belongs_to("buildbox~neos") && !remote.belongs_to("neos"));
        for key in [
            local,
            remote,
            SessionKey::thread(sb("buildbox~neos"), &th("api")),
            SessionKey::new(sb("buildbox~n"), SessionKind::ThreadBash(th("a"))),
            SessionKey::new(sb("n"), SessionKind::Bash),
            SessionKey::monitor(),
        ] {
            assert_eq!(SessionKey::parse(&key.to_string()), Ok(key.clone()), "{key}");
        }
        assert!(SessionKey::parse("neos").is_err());
        assert!(SessionKey::parse("neos::weird").is_err());
        assert!(SessionKey::parse("neos::monitor").is_err());
        assert!(SessionKey::parse("a~b~c::claude").is_err());
        assert!(SessionKey::parse("neos::thread:Bad").is_err());
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
