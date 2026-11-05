//! The Remote Control button: show whether Remote Control is on in a Claude
//! session, and turn it on once the login is one Remote Control accepts —
//! switching a sandbox sbxw signed in with a token over to a claude.ai login
//! first. Turning it off is left to the session or claude.ai: on, the button
//! is only an indicator.

use crate::app::App;
use anyhow::{bail, Context, Result};
use sbxw_core::remote::{self, Login};
use sbxw_core::session::SessionKind;
use sbxw_core::SandboxRef;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// How long a probe answers for. Every open pane of a sandbox asks at the same
/// moment (the page polls each every 15 s), and each probe is an `sbx ls` and
/// an `sbx exec node`, seconds apiece: they share one, and a person's press
/// always reads afresh.
const PROBE_TTL: Duration = Duration::from_secs(5);

/// A sandbox's last probe: its output or why it failed, and when. The lock
/// around it is held while probing, so the panes that ask meanwhile wait for
/// that answer rather than start their own.
pub(crate) type ProbeSlot = Arc<tokio::sync::Mutex<Option<(Instant, Result<String, String>)>>>;

/// What the button shows: how Claude is signed in, and whether Remote Control
/// is on in the session asked about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RemoteState {
    pub login: Login,
    pub active: bool,
}

impl App {
    /// What the panes poll: a probe at most `PROBE_TTL` old.
    pub async fn remote_state(&self, sandbox: &SandboxRef, kind: &SessionKind) -> Result<RemoteState> {
        self.remote_state_read(sandbox, kind, false).await
    }

    /// The probe's output, shared between the panes of a sandbox. `fresh`
    /// ignores an earlier answer, for a decision a person just asked for.
    async fn remote_probe(&self, sandbox: &SandboxRef, fresh: bool) -> Result<String> {
        let slot = self.remote_probes.lock().unwrap().entry(sandbox.clone()).or_default().clone();
        let mut held = slot.lock().await;
        if !fresh {
            if let Some((at, answer)) = held.as_ref() {
                if at.elapsed() < PROBE_TTL {
                    return answer.clone().map_err(anyhow::Error::msg);
                }
            }
        }
        let answer = self.remote_probe_run(sandbox).await.map_err(|e| format!("{e:#}"));
        *held = Some((Instant::now(), answer.clone()));
        answer.map_err(anyhow::Error::msg)
    }

    /// Read inside the sandbox, so only scopes and session flags come out,
    /// never the token.
    async fn remote_probe_run(&self, sandbox: &SandboxRef) -> Result<String> {
        if !self.sbx_for(sandbox).await?.is_running(&sandbox.name).await {
            bail!("'{sandbox}' is not running — open its Claude pane first");
        }
        let out = self.sbx_for(sandbox).await?.exec(&sandbox.name, &["node", "-e", remote::PROBE_SCRIPT], None).await?;
        Ok(out.stdout_str().to_string())
    }

    async fn remote_state_read(&self, sandbox: &SandboxRef, kind: &SessionKind, fresh: bool) -> Result<RemoteState> {
        let probe = self.remote_probe(sandbox, fresh).await?;
        let active = match self.session_dir(sandbox, kind).await {
            Some(dir) => remote::active_in(&probe, &dir, kind.thread().map(|t| t.as_str())),
            None => false,
        };
        Ok(RemoteState { login: remote::login_state(&probe), active })
    }

    /// The directory Claude Code was started in for `kind`: the workspace (sbx
    /// mounts it at the same path) for the main agent and a session in its
    /// checkout, else the thread's worktree — its own, or its shared plan's.
    async fn session_dir(&self, sandbox: &SandboxRef, kind: &SessionKind) -> Option<String> {
        match kind {
            SessionKind::Thread(t) if self.is_same_worktree(sandbox, t) => {
                self.store.workspace(sandbox).map(|w| w.to_string_lossy().into_owned())
            }
            SessionKind::Thread(t) => {
                let repo = self.settle_thread_repo(sandbox, t, None).await.ok().filter(|d| !d.is_empty())?;
                let folder = self.team_for(sandbox, t).map_or_else(|| t.to_string(), |team| team.worktree.to_string());
                Some(std::path::Path::new(&repo).join(".sbxw-worktrees").join(folder).to_string_lossy().into_owned())
            }
            _ => self.store.workspace(sandbox).map(|w| w.to_string_lossy().into_owned()),
        }
    }

    /// Turn Remote Control on in the main session or `thread`'s. Refused when
    /// it is on already — typed again, `/remote-control` opens a disconnect
    /// dialog — and, with the way out, when the login would make Claude refuse
    /// it: typing it then only leaves an error in the pane.
    pub async fn remote_control(self: &Arc<Self>, sandbox: &SandboxRef, kind: SessionKind) -> Result<()> {
        let state = self.remote_state_read(sandbox, &kind, true).await?;
        tracing::info!(
            "web UI: Remote Control in '{sandbox}' ({}) reads {}",
            self.session_dir(sandbox, &kind).await.unwrap_or_default(),
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
            Login::ApiKeyHelper => bail!("{}", remote::helper_advice(&sandbox.to_string())),
        }
        self.watch_remote(sandbox);
        let typed = self.push_text(sandbox, kind, remote::COMMAND).await.map_err(anyhow::Error::msg);
        // The page looks again in seconds, to see it come on: not at the old answer.
        if let Some(slot) = self.remote_probes.lock().unwrap().get(sandbox).cloned() {
            if let Ok(mut held) = slot.try_lock() {
                *held = None;
            }
        }
        typed
    }

    /// Switch a sandbox signed in with the setup token to a claude.ai login:
    /// sbxw stops injecting the token, and `/login` is typed into the session
    /// for the person to finish.
    ///
    /// The OAuth kit is rewritten without its file rather than left alone:
    /// sbx re-reads a kit from its path on every container swap, so the old
    /// spec would put the token back over the login at the next one.
    pub async fn switch_to_login(self: &Arc<Self>, sandbox: &SandboxRef, kind: SessionKind) -> Result<()> {
        if self.remote_state_read(sandbox, &kind, true).await?.login != Login::TokenOnly {
            bail!("Claude in '{sandbox}' is not signed in with a setup token — nothing to switch");
        }
        tracing::info!("web UI: switching '{sandbox}' to a claude.ai login for Remote Control");
        if self.store.paths.oauth_kit(sandbox).exists() {
            self.store.write_oauth_kit(sandbox, None).context("rewriting the OAuth kit without its token")?;
        }
        self.store.update(sandbox, |r| r.remote_control = true).context("recording the switch")?;
        if let Err(e) = self.sbx_for(sandbox).await?.policy("allow", Some(&sandbox.name), &remote::LOGIN_HOSTS.join(",")).await {
            tracing::warn!("could not allow the login hosts for '{sandbox}': {e:#}");
        }
        self.push_text(sandbox, kind, remote::LOGIN_COMMAND).await.map_err(anyhow::Error::msg)
    }
}

#[cfg(test)]
mod tests {
    use crate::testing::{app_with, sbx_answer};
    use sbxw_core::session::SessionKind;
    use sbxw_core::SandboxRef;
    use sbxw_sbx::runner::Output;

    fn signed_in_with(probe: &'static str) -> impl Fn(&[String]) -> Output + Send + Sync + 'static {
        move |a: &[String]| match a.first().map(String::as_str) {
            Some("ls") => crate::testing::ls(&[("neos", "running")]),
            Some("exec") if a.iter().any(|x| x == "-e") => Output::ok(probe),
            _ => sbx_answer(a),
        }
    }

    #[tokio::test]
    async fn the_panes_of_a_sandbox_share_one_probe_and_a_press_reads_afresh() {
        let (_d, app, fake) = app_with(signed_in_with(r#"{"present":true,"scopes":["user:profile"],"sessions":[]}"#));
        let name = SandboxRef::parse("neos").unwrap();
        let probes = |fake: &sbxw_sbx::runner::Fake| fake.log().iter().filter(|c| c.starts_with("exec")).count();
        let looks: Vec<_> = (0..4)
            .map(|_| {
                let (a, n) = (app.clone(), name.clone());
                tokio::spawn(async move { a.remote_state(&n, &SessionKind::Agent).await })
            })
            .collect();
        for l in looks {
            l.await.unwrap().unwrap();
        }
        assert_eq!(probes(&fake), 1, "four panes, one probe: {:?}", fake.log());
        app.remote_state(&name, &SessionKind::Agent).await.unwrap();
        assert_eq!(probes(&fake), 1, "still fresh");
        // The press decides on what is true now. Typing needs a live session,
        // which the fake has none of; the read comes first.
        let _ = app.remote_control(&name, SessionKind::Agent).await;
        assert_eq!(probes(&fake), 2, "{:?}", fake.log());
        app.remote_state(&name, &SessionKind::Agent).await.unwrap();
        assert_eq!(probes(&fake), 3, "after a press the next look is real");
    }

    #[tokio::test]
    async fn a_stopped_sandbox_is_refused_by_the_shared_probe_too() {
        let (_d, app, fake) = app_with(|a: &[String]| match a.first().map(String::as_str) {
            Some("ls") => crate::testing::ls(&[("neos", "stopped")]),
            _ => sbx_answer(a),
        });
        let name = SandboxRef::parse("neos").unwrap();
        for _ in 0..3 {
            let e = app.remote_state(&name, &SessionKind::Agent).await.unwrap_err();
            assert!(format!("{e:#}").contains("not running"), "{e:#}");
        }
        assert_eq!(fake.log().iter().filter(|c| c.starts_with("ls")).count(), 1, "{:?}", fake.log());
    }

    #[tokio::test]
    async fn a_token_sandbox_is_switched_for_good() {
        let (_d, app, fake) = app_with(signed_in_with(r#"{"present":true,"scopes":["user:inference"]}"#));
        let name = SandboxRef::parse("neos").unwrap();
        app.store.write_oauth_kit(sbxw_core::SandboxRef::parse("neos").unwrap(), Some("{\"claudeAiOauth\":{}}")).unwrap();
        let e = app.remote_control(&name, SessionKind::Agent).await.unwrap_err();
        assert!(format!("{e:#}").contains("switch"), "{e:#}");

        // Typing /login needs a live session, which the fake has none of; what
        // must already hold by then is that the token cannot come back.
        let _ = app.switch_to_login(&name, SessionKind::Agent).await;
        let spec = std::fs::read_to_string(app.store.paths.oauth_kit(sbxw_core::SandboxRef::parse("neos").unwrap()).join("spec.yaml")).unwrap();
        assert!(!spec.contains("credentials.json"), "{spec}");
        assert!(app.store.get(sbxw_core::SandboxRef::parse("neos").unwrap()).remote_control);
        assert!(fake.log().iter().any(|c| c.starts_with("policy allow") && c.contains("platform.claude.com")));
    }

    #[tokio::test]
    async fn the_state_is_the_session_of_this_pane() {
        let (_d, app, _fake) = app_with(signed_in_with(
            r#"{"present":true,"scopes":["user:profile"],"sessions":[{"cwd":"/w/neos","rc":true}]}"#,
        ));
        let name = SandboxRef::parse("neos").unwrap();
        app.store.update(sbxw_core::SandboxRef::parse("neos").unwrap(), |r| r.workspace = Some("/w/neos".into())).unwrap();
        let state = app.remote_state(&name, &SessionKind::Agent).await.unwrap();
        assert!(state.active && state.login == sbxw_core::remote::Login::Full);
        // On already, the button is an indicator: nothing is typed.
        let e = app.remote_control(&name, SessionKind::Agent).await.unwrap_err();
        assert!(format!("{e:#}").contains("already on"), "{e:#}");
        app.store.update(sbxw_core::SandboxRef::parse("neos").unwrap(), |r| r.workspace = Some("/w/other".into())).unwrap();
        assert!(!app.remote_state(&name, &SessionKind::Agent).await.unwrap().active);
    }

    #[tokio::test]
    async fn only_a_token_login_is_switched() {
        let (_d, app, _fake) = app_with(signed_in_with(r#"{"present":false,"scopes":[]}"#));
        let name = SandboxRef::parse("neos").unwrap();
        let e = app.switch_to_login(&name, SessionKind::Agent).await.unwrap_err();
        assert!(format!("{e:#}").contains("nothing to switch"));
        assert!(!app.store.get(sbxw_core::SandboxRef::parse("neos").unwrap()).remote_control);
    }
}
