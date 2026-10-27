//! The Remote Control button: show whether Remote Control is on in a Claude
//! session, and turn it on once the login is one Remote Control accepts —
//! switching a sandbox sbxw signed in with a token over to a claude.ai login
//! first. Turning it off is left to the session or claude.ai: on, the button
//! is only an indicator.

use crate::app::App;
use anyhow::{bail, Context, Result};
use sbxw_core::remote::{self, Login};
use sbxw_core::session::SessionKind;
use sbxw_core::SandboxName;
use std::sync::Arc;

/// What the button shows: how Claude is signed in, and whether Remote Control
/// is on in the session asked about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RemoteState {
    pub login: Login,
    pub active: bool,
}

impl App {
    /// Read inside the sandbox, so only scopes and session flags come out,
    /// never the token.
    pub async fn remote_state(&self, sandbox: &SandboxName, kind: &SessionKind) -> Result<RemoteState> {
        if !self.sbx.is_running(sandbox).await {
            bail!("'{sandbox}' is not running — open its Claude pane first");
        }
        let out = self.sbx.exec(sandbox, &["node", "-e", remote::PROBE_SCRIPT], None).await?;
        let probe = out.stdout_str();
        let active = match self.session_dir(sandbox, kind) {
            Some(dir) => remote::active_in(&probe, &dir),
            None => false,
        };
        Ok(RemoteState { login: remote::login_state(&probe), active })
    }

    /// The directory Claude Code was started in for `kind`: the workspace (sbx
    /// mounts it at the same path), or the thread's worktree.
    fn session_dir(&self, sandbox: &SandboxName, kind: &SessionKind) -> Option<String> {
        match kind {
            SessionKind::Thread(t) => self.settle_thread_repo(sandbox, t, None).ok().filter(|d| !d.is_empty()),
            _ => self.store.workspace(sandbox).map(|w| w.to_string_lossy().into_owned()),
        }
    }

    /// Turn Remote Control on in the main session or `thread`'s. Refused when
    /// it is on already — typed again, `/remote-control` opens a disconnect
    /// dialog — and, with the way out, when the login would make Claude refuse
    /// it: typing it then only leaves an error in the pane.
    pub async fn remote_control(self: &Arc<Self>, sandbox: &SandboxName, kind: SessionKind) -> Result<()> {
        let state = self.remote_state(sandbox, &kind).await?;
        tracing::info!(
            "web UI: Remote Control in '{sandbox}' ({}) reads {}",
            self.session_dir(sandbox, &kind).unwrap_or_default(),
            if state.active { "on" } else { "off" },
        );
        if state.active {
            bail!("Remote Control is already on in this session");
        }
        match state.login {
            Login::Full => {}
            Login::Missing => bail!("Claude in '{sandbox}' is not logged in to claude.ai — run /login in its pane, then try again"),
            Login::TokenOnly => bail!(
                "Claude in '{sandbox}' is signed in with a setup token (CLAUDE_CODE_OAUTH_TOKEN), which Remote Control refuses — \
                 switch it to a claude.ai login first"
            ),
        }
        self.watch_remote(sandbox);
        self.push_text(sandbox, kind, remote::COMMAND).await.map_err(anyhow::Error::msg)
    }

    /// Switch a sandbox signed in with the setup token to a claude.ai login:
    /// sbxw stops injecting the token, and `/login` is typed into the session
    /// for the person to finish.
    ///
    /// The OAuth kit is rewritten without its file rather than left alone:
    /// sbx re-reads a kit from its path on every container swap, so the old
    /// spec would put the token back over the login at the next one.
    pub async fn switch_to_login(self: &Arc<Self>, sandbox: &SandboxName, kind: SessionKind) -> Result<()> {
        if self.remote_state(sandbox, &kind).await?.login != Login::TokenOnly {
            bail!("Claude in '{sandbox}' is not signed in with a setup token — nothing to switch");
        }
        tracing::info!("web UI: switching '{sandbox}' to a claude.ai login for Remote Control");
        if self.store.paths.oauth_kit(sandbox).exists() {
            self.store.write_oauth_kit(sandbox, None).context("rewriting the OAuth kit without its token")?;
        }
        self.store.update(sandbox, |r| r.remote_control = true).context("recording the switch")?;
        if let Err(e) = self.sbx.policy("allow", Some(sandbox), &remote::LOGIN_HOSTS.join(",")).await {
            tracing::warn!("could not allow the login hosts for '{sandbox}': {e:#}");
        }
        self.push_text(sandbox, kind, remote::LOGIN_COMMAND).await.map_err(anyhow::Error::msg)
    }
}

#[cfg(test)]
mod tests {
    use crate::testing::{app_with, sbx_answer};
    use sbxw_core::session::SessionKind;
    use sbxw_core::SandboxName;
    use sbxw_sbx::runner::Output;

    fn signed_in_with(probe: &'static str) -> impl Fn(&[String]) -> Output + Send + Sync + 'static {
        move |a: &[String]| match a.first().map(String::as_str) {
            Some("ls") => Output::ok("SANDBOX   AGENT    STATUS\nneos      claude   running\n"),
            Some("exec") if a.iter().any(|x| x == "-e") => Output::ok(probe),
            _ => sbx_answer(a),
        }
    }

    #[tokio::test]
    async fn a_token_sandbox_is_switched_for_good() {
        let (_d, app, fake) = app_with(signed_in_with(r#"{"present":true,"scopes":["user:inference"]}"#));
        let name = SandboxName::parse("neos").unwrap();
        app.store.write_oauth_kit("neos", Some("{\"claudeAiOauth\":{}}")).unwrap();
        let e = app.remote_control(&name, SessionKind::Agent).await.unwrap_err();
        assert!(format!("{e:#}").contains("switch"), "{e:#}");

        // Typing /login needs a live session, which the fake has none of; what
        // must already hold by then is that the token cannot come back.
        let _ = app.switch_to_login(&name, SessionKind::Agent).await;
        let spec = std::fs::read_to_string(app.store.paths.oauth_kit("neos").join("spec.yaml")).unwrap();
        assert!(!spec.contains("credentials.json"), "{spec}");
        assert!(app.store.get("neos").remote_control);
        assert!(fake.log().iter().any(|c| c.starts_with("policy allow") && c.contains("platform.claude.com")));
    }

    #[tokio::test]
    async fn the_state_is_the_session_of_this_pane() {
        let (_d, app, _fake) = app_with(signed_in_with(
            r#"{"present":true,"scopes":["user:profile"],"sessions":[{"cwd":"/w/neos","rc":true}]}"#,
        ));
        let name = SandboxName::parse("neos").unwrap();
        app.store.update("neos", |r| r.workspace = Some("/w/neos".into())).unwrap();
        let state = app.remote_state(&name, &SessionKind::Agent).await.unwrap();
        assert!(state.active && state.login == sbxw_core::remote::Login::Full);
        // On already, the button is an indicator: nothing is typed.
        let e = app.remote_control(&name, SessionKind::Agent).await.unwrap_err();
        assert!(format!("{e:#}").contains("already on"), "{e:#}");
        app.store.update("neos", |r| r.workspace = Some("/w/other".into())).unwrap();
        assert!(!app.remote_state(&name, &SessionKind::Agent).await.unwrap().active);
    }

    #[tokio::test]
    async fn only_a_token_login_is_switched() {
        let (_d, app, _fake) = app_with(signed_in_with(r#"{"present":false,"scopes":[]}"#));
        let name = SandboxName::parse("neos").unwrap();
        let e = app.switch_to_login(&name, SessionKind::Agent).await.unwrap_err();
        assert!(format!("{e:#}").contains("nothing to switch"));
        assert!(!app.store.get("neos").remote_control);
    }
}
