//! Names that reach shell commands, file names, hostnames and git refs.
//!
//! Each is a newtype validated once, where it enters sbxw (a CLI argument, an
//! HTTP path, a WebSocket query, a plan a lead proposed). Everything downstream
//! takes the type, so nothing has to remember to re-check a `&str` — the old
//! code had a `reject_invalid_name` call at the top of forty handlers, and the
//! one that forgot would have been the bug.

use serde::{Deserialize, Serialize};
use std::fmt;

/// sbx refuses a longer name: it is a DNS label's limit, which is what
/// `<name>.sbx` and the `/etc/hosts` aliases make of it.
pub const MAX_SANDBOX_NAME_LEN: usize = 63;

/// Shared by the CLI and the web UI, so both explain the same rule the same way.
pub const INVALID_SANDBOX_NAME: &str =
    "name must be 1–63 letters, digits and hyphens, and must not end with a hyphen";

/// A thread name becomes a directory and a branch (`sbxw/<name>`) and is shown
/// in a pane bar.
pub const MAX_THREAD_NAME_LEN: usize = 40;

pub const INVALID_THREAD_NAME: &str =
    "a thread name is 1–40 lower-case letters, digits and inner hyphens";

/// A sandbox name, valid by construction.
///
/// The alphabet is sbx's rule narrowed to what is unambiguous in every place a
/// name travels: letters, digits and hyphens, at most 63, not ending in a
/// hyphen (sbx also forbids a trailing period; sbxw allows no period at all).
/// No underscore, which is what lets `__host__` name the monitor pane without
/// ever colliding with a sandbox.
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct SandboxName(String);

impl SandboxName {
    pub fn parse(raw: &str) -> Result<Self, NameError> {
        let raw = raw.trim();
        if is_valid_sandbox_name(raw) {
            Ok(Self(raw.to_string()))
        } else {
            Err(NameError(INVALID_SANDBOX_NAME))
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for SandboxName {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(d)?;
        SandboxName::parse(&raw).map_err(serde::de::Error::custom)
    }
}

/// A thread's name, valid by construction: lower-case only, since a branch
/// `Fix` and a branch `fix` are one directory on a case-insensitive Mac.
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct ThreadName(String);

impl ThreadName {
    pub fn parse(raw: &str) -> Result<Self, NameError> {
        let raw = raw.trim();
        if is_valid_thread_name(raw) {
            Ok(Self(raw.to_string()))
        } else {
            Err(NameError(INVALID_THREAD_NAME))
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The branch a thread works on.
    pub fn branch(&self) -> String {
        format!("sbxw/{}", self.0)
    }
}

impl<'de> Deserialize<'de> for ThreadName {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(d)?;
        ThreadName::parse(&raw).map_err(serde::de::Error::custom)
    }
}

/// A model id as `claude --model` receives it: `^[a-z0-9][a-z0-9.\-]{0,63}$`.
/// It reaches a command line inside the sandbox, so the alphabet is kept to
/// what model ids and aliases use, and nothing that could read as a flag.
#[derive(Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct ModelId(String);

impl ModelId {
    pub fn parse(raw: &str) -> Result<Self, NameError> {
        let raw = raw.trim();
        if is_valid_model_id(raw) {
            Ok(Self(raw.to_string()))
        } else {
            Err(NameError("not a model id"))
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for ModelId {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(d)?;
        ModelId::parse(&raw).map_err(serde::de::Error::custom)
    }
}

macro_rules! string_like {
    ($($t:ty),*) => {$(
        impl fmt::Display for $t {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }
        impl fmt::Debug for $t {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                fmt::Debug::fmt(&self.0, f)
            }
        }
        impl AsRef<str> for $t {
            fn as_ref(&self) -> &str {
                &self.0
            }
        }
        impl std::ops::Deref for $t {
            type Target = str;
            fn deref(&self) -> &str {
                &self.0
            }
        }
        impl PartialEq<str> for $t {
            fn eq(&self, other: &str) -> bool {
                self.0 == other
            }
        }
        impl PartialEq<&str> for $t {
            fn eq(&self, other: &&str) -> bool {
                self.0 == *other
            }
        }
    )*};
}
string_like!(SandboxName, ThreadName, ModelId);

/// Why a name was refused, in words a person can act on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NameError(pub &'static str);

impl fmt::Display for NameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

impl std::error::Error for NameError {}

pub fn is_valid_sandbox_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_SANDBOX_NAME_LEN
        && !name.ends_with('-')
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

pub fn is_valid_thread_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_THREAD_NAME_LEN
        && !name.starts_with('-')
        && !name.ends_with('-')
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

pub fn is_valid_model_id(id: &str) -> bool {
    let b = id.as_bytes();
    !b.is_empty()
        && b.len() <= 64
        && (b[0].is_ascii_lowercase() || b[0].is_ascii_digit())
        && b.iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'.' || *c == b'-')
}

/// Anything outside the sandbox-name alphabet becomes `-`, cut to sbx's length
/// with no trailing hyphen — a derived name is always valid.
pub fn sanitize_sandbox_name(s: &str) -> SandboxName {
    let cleaned: String = s
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '-' })
        .take(MAX_SANDBOX_NAME_LEN)
        .collect();
    let cleaned = cleaned.trim_end_matches('-');
    SandboxName(if cleaned.is_empty() {
        "sandbox".to_string()
    } else {
        cleaned.to_string()
    })
}

/// The `n`th candidate for a name derived from a workspace whose plain name is
/// taken by a different path: `<base>-copy`, `<base>-copy-1`, … The base gives
/// way so the suffix survives sbx's length limit, and never leaves a hyphen
/// where the two meet.
pub fn copy_candidate(base: &SandboxName, clashes: u32) -> SandboxName {
    if clashes == 0 {
        return base.clone();
    }
    let suffix = if clashes == 1 {
        "-copy".to_string()
    } else {
        format!("-copy-{}", clashes - 1)
    };
    let keep = MAX_SANDBOX_NAME_LEN - suffix.len();
    let head = base.0[..base.0.len().min(keep)].trim_end_matches('-');
    SandboxName(format!("{head}{suffix}"))
}

/// A `chat-xxxxxx` name from 24 bits of entropy.
pub fn mint_chat_name(entropy: u32) -> SandboxName {
    SandboxName(format!("chat-{:06x}", entropy & 0xff_ffff))
}

/// Base name of the island's ephemeral chats.
pub const EPHEMERAL_CHAT: &str = "ephemeral-chat";

/// The name a brand-new ephemeral chat should take: `ephemeral-chat`, else the
/// first free `ephemeral-chat-N` counting from 2. By availability, not a
/// high-water mark: a freed name is offered again rather than the counter
/// climbing forever. `taken` holds every sandbox name.
pub fn next_ephemeral_chat<'a>(taken: impl IntoIterator<Item = &'a str> + Clone) -> SandboxName {
    let has = |n: &str| taken.clone().into_iter().any(|t| t == n);
    if !has(EPHEMERAL_CHAT) {
        return SandboxName(EPHEMERAL_CHAT.into());
    }
    (2..)
        .map(|n| format!("{EPHEMERAL_CHAT}-{n}"))
        .find(|c| !has(c))
        .map(SandboxName)
        .unwrap_or_else(|| SandboxName(EPHEMERAL_CHAT.into()))
}

/// The thread a new conversation in `ephemeral-chat` takes: `chat-N`, the
/// first free one counting from 2 (the sandbox's own agent is chat 1). The
/// chat dock's "New chat" is another session in that sandbox, not another
/// sandbox. `taken` holds the sandbox's thread names.
pub fn next_chat_session<'a>(taken: impl IntoIterator<Item = &'a str> + Clone) -> ThreadName {
    let has = |n: &str| taken.clone().into_iter().any(|t| t == n);
    (2..)
        .map(|n| format!("chat-{n}"))
        .find(|c| !has(c))
        .map(ThreadName)
        .expect("an unbounded range has a free name")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sandbox_names_follow_sbx_rule() {
        assert!(SandboxName::parse("neos").is_ok());
        assert!(SandboxName::parse("neos-2").is_ok());
        assert!(SandboxName::parse(" neos ").is_ok(), "trimmed at the door");
        assert!(SandboxName::parse("").is_err());
        assert!(SandboxName::parse("neos-").is_err());
        assert!(SandboxName::parse("ne os").is_err());
        assert!(SandboxName::parse("ne.os").is_err());
        assert!(SandboxName::parse("__host__").is_err(), "the monitor key cannot collide");
        assert!(SandboxName::parse(&"a".repeat(63)).is_ok());
        assert!(SandboxName::parse(&"a".repeat(64)).is_err());
    }

    #[test]
    fn thread_names_are_lower_case_and_trimmed_of_hyphens() {
        assert!(ThreadName::parse("api-auth").is_ok());
        assert!(ThreadName::parse("Api").is_err());
        assert!(ThreadName::parse("-a").is_err());
        assert!(ThreadName::parse("a-").is_err());
        assert!(ThreadName::parse(&"a".repeat(41)).is_err());
        assert_eq!(ThreadName::parse("x").unwrap().branch(), "sbxw/x");
    }

    #[test]
    fn model_ids_are_checked_against_a_narrow_alphabet() {
        for ok in ["claude-sonnet-5-5", "opus", "claude-3.7"] {
            assert!(ModelId::parse(ok).is_ok(), "{ok}");
        }
        for bad in ["", "-x", "Opus", "a b", "a;b", &"a".repeat(65)] {
            assert!(ModelId::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_name_can_only_be_deserialized_when_valid() {
        assert!(serde_json::from_str::<SandboxName>("\"ok\"").is_ok());
        assert!(serde_json::from_str::<SandboxName>("\"no way\"").is_err());
        assert!(serde_json::from_str::<ThreadName>("\"Bad\"").is_err());
    }

    #[test]
    fn sanitizing_always_yields_a_valid_name() {
        assert_eq!(sanitize_sandbox_name("my project!").as_str(), "my-project");
        assert_eq!(sanitize_sandbox_name("___").as_str(), "sandbox");
        let long = sanitize_sandbox_name(&"x".repeat(100));
        assert_eq!(long.len(), MAX_SANDBOX_NAME_LEN);
    }

    #[test]
    fn copies_keep_their_suffix_within_the_limit() {
        let base = sanitize_sandbox_name("neos");
        assert_eq!(copy_candidate(&base, 0).as_str(), "neos");
        assert_eq!(copy_candidate(&base, 1).as_str(), "neos-copy");
        assert_eq!(copy_candidate(&base, 3).as_str(), "neos-copy-2");
        let long = sanitize_sandbox_name(&format!("{}-{}", "a".repeat(57), "bcdef"));
        let c = copy_candidate(&long, 2);
        assert!(c.len() <= MAX_SANDBOX_NAME_LEN && c.ends_with("-copy-1"));
        assert!(is_valid_sandbox_name(&c));
    }

    #[test]
    fn ephemeral_chats_are_numbered_by_availability() {
        assert_eq!(next_ephemeral_chat(["neos"]).as_str(), "ephemeral-chat");
        assert_eq!(
            next_ephemeral_chat(["ephemeral-chat", "ephemeral-chat-2"]).as_str(),
            "ephemeral-chat-3"
        );
        assert_eq!(
            next_ephemeral_chat(["ephemeral-chat", "ephemeral-chat-3"]).as_str(),
            "ephemeral-chat-2",
            "a freed name is reused"
        );
    }

    #[test]
    fn chat_sessions_are_numbered_by_availability() {
        assert_eq!(next_chat_session([]).as_str(), "chat-2");
        assert_eq!(next_chat_session(["chat-2", "t1"]).as_str(), "chat-3");
        assert_eq!(next_chat_session(["chat-3"]).as_str(), "chat-2", "a freed name is reused");
        assert!(is_valid_thread_name(next_chat_session(["chat-2"]).as_str()));
    }

    #[test]
    fn chat_names_are_valid() {
        let n = mint_chat_name(0xdead_beef);
        assert_eq!(n.as_str(), "chat-adbeef");
    }
}

/// A repo path as a thread names it, relative to the workspace: no `..`, no
/// absolute path, no empty parts; `''` is the workspace itself.
pub fn normalize_repo_path(rel: &str) -> Result<String, String> {
    use std::path::{Component, Path};
    let rel = rel.trim();
    if rel.starts_with('/') || rel.starts_with('\\') {
        return Err(format!("repo {rel:?} must be relative to the workspace"));
    }
    let mut parts = Vec::new();
    for c in Path::new(rel).components() {
        match c {
            Component::Normal(p) => parts.push(p.to_string_lossy().into_owned()),
            Component::CurDir => {}
            _ => return Err(format!("repo {rel:?} must stay inside the workspace")),
        }
    }
    Ok(parts.join("/"))
}

#[cfg(test)]
mod repo_path_tests {
    use super::normalize_repo_path as n;

    #[test]
    fn repo_paths_stay_inside_the_workspace() {
        assert_eq!(n("").unwrap(), "");
        assert_eq!(n("./backend/").unwrap(), "backend");
        assert_eq!(n("a/./b").unwrap(), "a/b");
        assert!(n("/abs").is_err());
        assert!(n("../up").is_err());
        assert!(n("a/../../b").is_err());
    }
}
