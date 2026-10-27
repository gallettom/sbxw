//! Remote Control: driving a sandbox's Claude session from claude.ai/code or
//! the Claude app.
//!
//! Remote Control refuses the long-lived token `claude setup-token` makes
//! (`CLAUDE_CODE_OAUTH_TOKEN`, scope `user:inference` only) and an API key: it
//! needs a claude.ai login made with `/login`, whose scopes include the
//! profile and sessions. So a sandbox created for it gets no credentials from
//! sbxw — no OAuth kit, no auth variable in its environment — and its agent
//! asks for a login on first launch, the way Claude Code does anywhere.
//!
//! A sandbox created without the option can be switched over later, but only
//! by rewriting its credentials kit without the file: sbx re-reads the kit
//! from its path on every container swap, and would otherwise put the token
//! back over the login.

/// Typed into a Claude session to start Remote Control in it.
/// Never typed while it is on: it then opens a disconnect dialog instead.
pub const COMMAND: &str = "/remote-control";

/// Typed into a session signed in with the setup token, to replace it with a
/// claude.ai login. Never into one with no credentials: Claude Code is then on
/// its first-launch login screen, not at a prompt.
pub const LOGIN_COMMAND: &str = "/login";

/// Egress a `/login` from inside the sandbox needs: the authorize page is
/// opened in the person's own browser, but the code exchange and the
/// profile lookup leave from the sandbox.
pub const LOGIN_HOSTS: &[&str] = &["claude.ai", "platform.claude.com", "console.anthropic.com"];

/// Variables that would sign the agent in with something Remote Control
/// refuses, and that a Remote Control sandbox is therefore never given.
pub const AUTH_ENV: &[&str] = &["CLAUDE_CODE_OAUTH_TOKEN", "CLAUDE_OAUTH_TOKEN", "ANTHROPIC_API_KEY", "ANTHROPIC_AUTH_TOKEN"];

/// `K=V` pairs without the auth variables.
pub fn without_auth_env(pairs: Vec<String>) -> Vec<String> {
    pairs
        .into_iter()
        .filter(|p| !AUTH_ENV.iter().any(|k| p.split_once('=').map_or(p.as_str(), |(n, _)| n) == *k))
        .collect()
}

/// Run inside the sandbox; prints whether there are credentials and their
/// scopes — never the token, which stays in the sandbox — and, for each live
/// Claude Code session, its directory and whether Remote Control is on in it.
///
/// Claude Code keeps a registry entry per session in `~/.claude/sessions/
/// <pid>.json` and sets `bridgeSessionId` in it while Remote Control is
/// connected (null once disconnected). Entries outlive their process, and a pid
/// is reused across container lifetimes, so an entry is live only when that
/// pid's start time (`/proc/<pid>/stat`, field 22) is its `procStart`.
///
/// It also says whether an `apiKeyHelper` is configured: Claude Code puts a
/// helper's key ahead of any login, and sbx provisions one in Claude
/// sandboxes — before 0.50 even for subscription OAuth — so such a sandbox
/// answers with an API key whatever `/login` did.
pub const PROBE_SCRIPT: &str = r#"const fs = require('fs');
let c = {};
try { c = JSON.parse(fs.readFileSync('/home/agent/.claude/.credentials.json', 'utf8')).claudeAiOauth || {}; } catch (_) {}
const sessions = [];
const dir = '/home/agent/.claude/sessions';
let names = [];
try { names = fs.readdirSync(dir).filter(n => /^\d+\.json$/.test(n)); } catch (_) {}
for (const n of names) {
  try {
    const e = JSON.parse(fs.readFileSync(`${dir}/${n}`, 'utf8'));
    const stat = fs.readFileSync(`/proc/${e.pid}/stat`, 'utf8');
    const start = stat.slice(stat.lastIndexOf(')') + 2).split(' ')[19];
    if (String(e.procStart) === start) sessions.push({ cwd: e.cwd, rc: !!e.bridgeSessionId });
  } catch (_) {}
}
let helper = false;
for (const f of ['/home/agent/.claude/settings.json', '/etc/claude-code/managed-settings.json']) {
  try { if (JSON.parse(fs.readFileSync(f, 'utf8')).apiKeyHelper) helper = true; } catch (_) {}
}
console.log(JSON.stringify({ present: !!c.accessToken, scopes: Array.isArray(c.scopes) ? c.scopes : [], helper, sessions }));"#;

/// Whether Remote Control is on in a live session started in `cwd`, as the
/// probe saw it. Any of them counts: a thread and the main agent never share
/// a directory.
pub fn active_in(probe: &str, cwd: &str) -> bool {
    let want = cwd.trim_end_matches('/');
    serde_json::from_str::<serde_json::Value>(probe.trim())
        .ok()
        .and_then(|v| v["sessions"].as_array().cloned())
        .unwrap_or_default()
        .iter()
        .any(|s| s["rc"] == true && s["cwd"].as_str().is_some_and(|c| c.trim_end_matches('/') == want))
}

/// Whether Remote Control is on in any live session the probe saw.
pub fn any_active(probe: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(probe.trim())
        .ok()
        .and_then(|v| v["sessions"].as_array().cloned())
        .unwrap_or_default()
        .iter()
        .any(|s| s["rc"] == true)
}

/// How the sandbox's Claude is signed in, as far as Remote Control cares.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Login {
    /// No claude.ai credentials: `/login` first.
    Missing,
    /// The setup-token sbxw injects: inference only, refused.
    TokenOnly,
    /// A `/login` session.
    Full,
    /// An `apiKeyHelper` outranks every login: Claude answers with an API key,
    /// which Remote Control refuses. Only recreating the sandbox on sbx 0.50+
    /// (or removing the helper by hand) changes it.
    ApiKeyHelper,
}

/// What to do about [`Login::ApiKeyHelper`]: the helper came with the
/// sandbox, so the sandbox has to go.
pub fn helper_advice(sandbox: &str) -> String {
    format!(
        "Claude in '{sandbox}' takes its key from an apiKeyHelper, which outranks /login, and Remote Control refuses \
         an API key. sbx provisioned the helper at creation (before 0.50 even for subscription OAuth): on sbx 0.50+, \
         recreate it without --use-api-key — `sbxw rm {sandbox}` then `sbxw up {sandbox} --remote-control` \
         (anything outside the workspace is lost)"
    )
}

/// Read the probe's output. Anything unreadable is `Missing`, which only
/// ever asks for a login.
pub fn login_state(probe: &str) -> Login {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(probe.trim()) else {
        return Login::Missing;
    };
    if v["helper"] == true {
        return Login::ApiKeyHelper;
    }
    if v["present"] != true {
        return Login::Missing;
    }
    let scopes: Vec<&str> = v["scopes"].as_array().map(|a| a.iter().filter_map(|s| s.as_str()).collect()).unwrap_or_default();
    if scopes.iter().any(|s| *s != "user:inference") {
        Login::Full
    } else {
        Login::TokenOnly
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_variables_are_dropped_and_nothing_else() {
        let pairs = vec![
            "CLAUDE_CODE_OAUTH_TOKEN=x".to_string(),
            "ANTHROPIC_API_KEY=y".into(),
            "ANTHROPIC_DEFAULT_MODEL=z".into(),
            "CLAUDE_CODE_OAUTH_TOKEN_EXTRA=kept".into(),
        ];
        assert_eq!(without_auth_env(pairs), vec!["ANTHROPIC_DEFAULT_MODEL=z", "CLAUDE_CODE_OAUTH_TOKEN_EXTRA=kept"]);
    }

    #[test]
    fn the_setup_token_is_told_apart_from_a_login() {
        assert_eq!(login_state(r#"{"present":false,"scopes":[]}"#), Login::Missing);
        assert_eq!(login_state(r#"{"present":true,"scopes":["user:inference"]}"#), Login::TokenOnly);
        assert_eq!(login_state(r#"{"present":true,"scopes":[]}"#), Login::TokenOnly);
        assert_eq!(
            login_state(r#"{"present":true,"scopes":["user:inference","user:profile","user:sessions:claude_code"]}"#),
            Login::Full
        );
        assert_eq!(login_state("node: not found"), Login::Missing);
        assert_eq!(
            login_state(r#"{"present":true,"scopes":["user:inference","user:profile"],"helper":true}"#),
            Login::ApiKeyHelper,
            "a helper outranks any login"
        );
    }

    #[test]
    fn remote_control_is_on_only_in_a_live_session_of_that_directory() {
        let probe = r#"{"present":true,"scopes":[],"sessions":[
            {"cwd":"/w/app","rc":true},
            {"cwd":"/w/app/.sbxw-worktrees/t1","rc":false}]}"#;
        assert!(active_in(probe, "/w/app"));
        assert!(active_in(probe, "/w/app/"));
        assert!(!active_in(probe, "/w/app/.sbxw-worktrees/t1"));
        assert!(!active_in(probe, "/w/other"));
        assert!(!active_in("garbage", "/w/app"));
        assert!(any_active(probe));
        assert!(!any_active(r#"{"sessions":[{"cwd":"/w/app","rc":false}]}"#));
        assert!(!any_active("garbage"));
    }
}
