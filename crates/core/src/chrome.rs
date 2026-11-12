//! Claude in Chrome (sbx 0.53): a sandboxed Claude Code working with tabs of
//! the host's browser, through the Claude extension installed there.
//!
//! Three things have to hold, and only the last is sbxw's:
//! * sbx's `claude.chrome` setting is on (off by default; the person's own
//!   switch, on the host, which sbxw reads and never sets);
//! * Claude is signed in with a subscription, the same account as the
//!   extension: sbx authenticates the browser connection with the login it
//!   keeps on the host, and an API key alone is refused;
//! * the agent is started with [`FLAG`], which sbxw passes after `--` when the
//!   sandbox asks for it (`chrome = true`, the create dialog, `sbxw up
//!   --chrome`) *and* the setting is on, so a sandbox that asks in vain starts
//!   as it always did.
//!
//! Turning the setting off blocks new connections from running sandboxes; an
//! established one lasts until the Claude browser session is ended.

/// sbx's setting, read with `sbx settings get`.
pub const SETTING: &str = "claude.chrome";
/// The sbx release that added the setting: an older one refuses the key.
pub const SINCE: (u32, u32, u32) = (0, 53, 0);

/// What the agent is started with.
pub const FLAG: &str = "--chrome";

pub const NEEDS_SBX: &str = "Claude in Chrome needs sbx 0.53 or newer — the agent starts without --chrome";

pub const NEEDS_SUBSCRIPTION: &str =
    "Claude in Chrome needs a Claude subscription login, the same account as the browser extension: an API key alone is refused";

/// What to run when the sandbox asks for Chrome and the setting is off.
pub fn setting_advice() -> String {
    format!(
        "Claude in Chrome is off in sbx, so the agent starts without --chrome: run `sbx settings set {SETTING} true` on the \
         host, install the Claude extension in your browser signed in to the same account, then reopen the agent"
    )
}

/// The flags the agent is started with: [`FLAG`] when the sandbox asks for
/// Chrome and sbx's setting is on (`None`: an sbx that has no such setting).
pub fn agent_flags(wanted: bool, setting: Option<bool>) -> &'static [&'static str] {
    match wanted && setting == Some(true) {
        true => &[FLAG],
        false => &[],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_flag_is_passed_only_when_asked_for_and_switched_on() {
        assert_eq!(agent_flags(true, Some(true)), ["--chrome"]);
        assert!(agent_flags(true, Some(false)).is_empty(), "sbx would refuse the connection");
        assert!(agent_flags(true, None).is_empty(), "an older sbx reads arguments after -- differently");
        assert!(agent_flags(false, Some(true)).is_empty(), "the setting alone opens nothing");
    }
}
