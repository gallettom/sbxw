//! Opening a terminal session: which command each kind runs, and the rules
//! around threads (their repo, their model, the session ceiling, a stopped
//! sandbox).

use crate::app::{now_ms, App};
use crate::bus::Event;
use crate::git;
use crate::pty::{PtySession, Spawn};
use crate::typing;
use anyhow::{anyhow, bail, Result, Context};
use futures_util::future::BoxFuture;
use sbxw_core::agents::SessionState;
use sbxw_core::config::Config;
use sbxw_core::{ports, remote};
use sbxw_core::session::{SessionKey, SessionKind};
use sbxw_core::{ModelId, SandboxName, ThreadName};
use sbxw_sbx::scripts;
use std::sync::Arc;
use std::time::Duration;

/// A stopped sandbox starts its container, then boots Claude Code.
const COLD_START_TIMEOUT: Duration = Duration::from_secs(120);

/// Is a host port held by someone else? Something answering on it is
/// enough; a failed bind catches a listener that does not answer. Both,
/// because std binds with `SO_REUSEADDR`, which on macOS lets a wildcard bind
/// succeed beside a listener on one address.
fn host_port_busy(ip: &str, port: u16) -> bool {
    let ip: std::net::IpAddr = ip.parse().unwrap_or(std::net::Ipv4Addr::LOCALHOST.into());
    let probe = if ip.is_unspecified() { std::net::Ipv4Addr::LOCALHOST.into() } else { ip };
    let addr = std::net::SocketAddr::new(probe, port);
    std::net::TcpStream::connect_timeout(&addr, Duration::from_millis(200)).is_ok()
        || std::net::TcpListener::bind(std::net::SocketAddr::new(ip, port)).is_err()
}

impl App {
    /// The session under `key`, opened if it is not running. Returns whether
    /// this call started it.
    ///
    /// Boxed, because opening a thread on a stopped sandbox opens its agent
    /// first — the one recursion in here.
    pub fn open_session<'a>(self: &'a Arc<Self>, key: &'a SessionKey) -> BoxFuture<'a, Result<(Arc<PtySession>, bool)>> {
        Box::pin(async move {
            let app = self.clone();
            let k = key.clone();
            self.sessions
                .get_or_open(key, now_ms(), move || async move { app.spawn_for(&k).await })
                .await
        })
    }

    async fn spawn_for(self: &Arc<Self>, key: &SessionKey) -> Result<Spawn> {
        let Some(sandbox) = key.sandbox.clone() else {
            let cmd = &self.cfg.monitor_cmd;
            let Some((program, args)) = cmd.split_first() else {
                bail!("no monitor command configured — set `monitor_cmd` in sbxw.toml");
            };
            return Ok(Spawn { program: program.clone(), args: args.to_vec(), env: vec![] });
        };
        // A chat workspace under /tmp may have been swept away; `sbx run` then
        // refuses to start the sandbox. It is empty by definition: re-create it.
        if let Some(dir) = self.store.chat_workspace(&sandbox) {
            if !dir.exists() {
                let _ = std::fs::create_dir_all(&dir);
                tracing::info!("re-created vanished chat workspace {}", dir.display());
            }
        }
        let cfg = self.config_for(&sandbox);
        // The two kinds that start a stopped sandbox: `sbx run` and `sbx exec`
        // (threads release ports in `ensure_running_for_thread`).
        if matches!(key.kind, SessionKind::Agent | SessionKind::Bash) && !self.sbx.is_running(&sandbox).await {
            self.release_busy_ports(&sandbox, &cfg).await;
        }
        Ok(match &key.kind {
            SessionKind::Monitor => unreachable!("the monitor has no sandbox"),
            SessionKind::Agent if cfg.web_shell.is_empty() => {
                let env = if self.store.get(&sandbox).remote_control {
                    remote::without_auth_env(cfg.env_pairs())
                } else {
                    cfg.env_pairs()
                };
                Spawn::sbx(sbxw_sbx::attach_args(&sandbox, &env, &cfg.env_files))
            }
            SessionKind::Agent => Spawn::sbx(sbxw_sbx::shell_args(&sandbox, &cfg.web_shell)),
            // `sbx exec` starts a stopped sandbox first (0.50).
            SessionKind::Bash => Spawn::sbx(sbxw_sbx::bash_args(&sandbox)),
            SessionKind::ThreadBash(thread) => {
                self.ensure_running_for_thread(&sandbox).await?;
                let dir = self.settle_thread_repo(&sandbox, thread, None).map_err(|e| anyhow!(e))?;
                Spawn::sbx(scripts::thread_bash_args(&sandbox, &dir, thread, self.team_for(&sandbox, thread).as_ref()))
            }
            SessionKind::Thread(thread) => {
                self.ensure_running_for_thread(&sandbox).await?;
                // The ceiling refuses a *new* thread only; the agent never.
                let open = self.sessions.agents_open(&sandbox);
                let max = cfg.max_sessions();
                if open >= max {
                    bail!(
                        "'{sandbox}' already has {open} Claude session{} open, and max_agent_sessions \
                         is {max} (sbxw.toml). Close a thread first, or raise the ceiling.",
                        if open == 1 { "" } else { "s" }
                    );
                }
                let dir = self.settle_thread_repo(&sandbox, thread, None).map_err(|e| anyhow!(e))?;
                let briefing = self
                    .briefings
                    .lock()
                    .unwrap()
                    .remove(&(sandbox.to_string(), thread.to_string()))
                    .unwrap_or_default();
                // Spelled out even for the default: `sbx exec` carries the
                // environment the sandbox was *created* with, which may name a
                // model since retired.
                let model = self
                    .store
                    .get(&sandbox)
                    .threads
                    .get(thread.as_str())
                    .and_then(|r| r.model.clone())
                    .or_else(|| Some(cfg.claude_model.clone()))
                    .and_then(|m| ModelId::parse(&m).ok());
                let team = self.team_for(&sandbox, thread);
                Spawn::sbx(scripts::thread_run_args(&sandbox, &dir, thread, &briefing, model.as_ref(), team.as_ref()))
            }
        })
    }

    /// A stopped sandbox keeps the mappings it was created with (`-p`), and
    /// sbx refuses to start it while any of their host ports is taken. A port
    /// is a convenience, never a reason not to start: each busy one is
    /// unpublished first, and said. sbx's own list is the truth; the config's
    /// specs back it up in case that list is empty while stopped, and an
    /// unpublish of something not published is a harmless failure.
    async fn release_busy_ports(&self, sandbox: &SandboxName, cfg: &Config) {
        let mut candidates: Vec<(String, u16, String)> =
            self.sbx.ports(sandbox).await.into_iter().map(|m| (m.host_ip.clone(), m.host_port, m.spec())).collect();
        let wanted = cfg.wanted_ports(&[]);
        for (i, (p, spec)) in wanted.iter().zip(ports::publish_specs(&wanted, cfg.ip_per_app)).enumerate() {
            if !candidates.iter().any(|(_, port, _)| *port == p.host_port) {
                candidates.push((ports::host_ip_for(cfg.ip_per_app, i), p.host_port, spec));
            }
        }
        for (ip, port, spec) in candidates {
            if !host_port_busy(&ip, port) {
                continue;
            }
            match self.sbx.unpublish(sandbox, &spec).await {
                Ok(()) => tracing::warn!(
                    "host port {port} is taken: starting '{sandbox}' without its {spec} mapping; free the \
                     port and run `sbxw ports {sandbox}` to publish it again"
                ),
                Err(e) => tracing::debug!("could not unpublish {spec} from '{sandbox}' before starting it: {e:#}"),
            }
        }
    }

    /// A thread runs through `sbx exec`, which starts a stopped sandbox
    /// itself (0.50) — no main session to open and wait for. Started here,
    /// rather than by the thread's own exec, so busy host ports are released
    /// first (sbx refuses to start while one is taken) and a failure is an
    /// error, not a pane that dies.
    pub async fn ensure_running_for_thread(self: &Arc<Self>, sandbox: &SandboxName) -> Result<()> {
        if self.sbx.is_running(sandbox).await {
            return Ok(());
        }
        tracing::info!("'{sandbox}' is stopped — starting it for a thread");
        self.release_busy_ports(sandbox, &self.config_for(sandbox)).await;
        self.sbx.start(sandbox).await.with_context(|| format!("starting '{sandbox}' for a thread"))
    }

    /// Which repo `thread` works in, settled and recorded, as the directory
    /// its scripts start in. Every thread path goes through here (its
    /// session, its Bash, status, close, merge) so they cannot disagree.
    pub fn settle_thread_repo(&self, sandbox: &SandboxName, thread: &ThreadName, requested: Option<&str>) -> Result<String, String> {
        let Some(ws) = self.store.workspace(sandbox) else {
            return match requested {
                Some(_) => Err(format!("sbxw has no workspace recorded for '{sandbox}', so it cannot pick a repo in it")),
                None => Ok(String::new()),
            };
        };
        let recorded = self.store.get(sandbox).threads.get(thread.as_str()).map(|r| r.repo.clone());
        let chosen = git::choose(&ws, recorded.as_deref(), thread, requested)?;
        if recorded.as_deref() != Some(chosen.as_str()) {
            self.store
                .update(sandbox, |r| r.threads.entry(thread.to_string()).or_default().repo = chosen.clone())
                .map_err(|e| format!("could not record the repo of thread {thread}: {e:#}"))?;
        }
        Ok(git::thread_dir(&ws, &chosen).to_string_lossy().into_owned())
    }

    /// Record a thread's model the first time one is asked for: it keeps the
    /// model it was opened on, as it keeps its repo. `force` (a plan says so)
    /// replaces one recorded earlier.
    pub fn settle_thread_model(&self, sandbox: &SandboxName, thread: &ThreadName, model: &ModelId, force: bool) -> Result<(), String> {
        self.store
            .update(sandbox, |r| {
                let t = r.threads.entry(thread.to_string()).or_default();
                if force || t.model.is_none() {
                    t.model = Some(model.to_string());
                }
            })
            .map_err(|e| format!("could not record the model of thread {thread}: {e:#}"))
    }

    /// Type `text` into a session and submit it, opening the session first if
    /// it is not running.
    pub async fn push_text(self: &Arc<Self>, sandbox: &SandboxName, kind: SessionKind, text: &str) -> Result<(), String> {
        let key = SessionKey::new(sandbox.clone(), kind.clone());
        let cold = kind == SessionKind::Agent && !self.sessions.is_live(&key) && !self.sbx.is_running(sandbox).await;
        let mut events = self.bus.subscribe();
        let (session, fresh) = self.open_session(&key).await.map_err(|e| format!("{e:#}"))?;
        if cold && fresh {
            self.await_cold_agent(sandbox, &session, &mut events).await;
        }
        // A cold agent's first frame is settled already: not waited for twice.
        typing::type_and_submit(&session, text, fresh && !cold).await.map_err(|e| match &kind {
            SessionKind::Thread(t) => format!("thread '{t}' of '{sandbox}' is not running"),
            _ => format!("'{sandbox}' has no live agent session — {e}"),
        })
    }

    /// The agent of a stopped sandbox: `sbx run` starts the container and goes
    /// quiet for as long as that takes, before Claude Code even boots — a
    /// silence typing would read as a drawn prompt, and the message would be
    /// lost. Claude Code's own `SessionStart` hook says when it is up; its
    /// first frame is then let settle. The output is watched from the start,
    /// so a frame drawn before the hook arrives is not waited for again.
    async fn await_cold_agent(&self, sandbox: &SandboxName, session: &Arc<PtySession>, events: &mut tokio::sync::broadcast::Receiver<Event>) {
        let mut output = session.subscribe();
        tracing::info!("'{sandbox}' is stopped — waiting for its agent to start before typing");
        let started = async {
            loop {
                match events.recv().await {
                    Ok(Event::Session(info)) if info.sandbox == sandbox.as_str() && info.state == SessionState::Idle => return true,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => return false,
                    _ => {}
                }
            }
        };
        let dead = async {
            while session.alive() {
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
        };
        tokio::select! {
            r = tokio::time::timeout(COLD_START_TIMEOUT, started) => {
                if !matches!(r, Ok(true)) {
                    tracing::warn!("'{sandbox}': no SessionStart within {}s — typing anyway", COLD_START_TIMEOUT.as_secs());
                }
            }
            _ = dead => return,
        }
        // Drained, then timed: what was drawn already counts as activity.
        loop {
            match tokio::time::timeout(typing::SETTLE_COLD, output.recv()).await {
                Err(_) | Ok(Err(tokio::sync::broadcast::error::RecvError::Closed)) => break,
                Ok(_) => {}
            }
        }
    }

    /// `push_text` in the background, for callers owed an immediate answer.
    pub fn spawn_push(self: &Arc<Self>, sandbox: SandboxName, kind: SessionKind, text: String, on_fail: Option<Box<dyn FnOnce(String) + Send>>) {
        let app = self.clone();
        tokio::spawn(async move {
            if let Err(e) = app.push_text(&sandbox, kind, &text).await {
                tracing::warn!("could not deliver to '{sandbox}': {e}");
                if let Some(f) = on_fail {
                    f(e);
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use crate::testing::{app_with, sbx_answer};
    use sbxw_core::config::Config;
    use sbxw_core::SandboxName;
    use sbxw_sbx::runner::Output;

    #[tokio::test]
    async fn a_busy_host_port_is_unpublished_before_a_start() {
        let held = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let busy = held.local_addr().unwrap().port();
        let free = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let rows = format!(
            r#"[{{"host_ip":"127.0.0.1","host_port":{busy},"sandbox_port":8000,"protocol":"tcp"}},
               {{"host_ip":"127.0.0.1","host_port":{free},"sandbox_port":3000,"protocol":"tcp"}}]"#
        );
        let (_d, app, fake) = app_with(move |a: &[String]| match a.get(2).map(String::as_str) {
            Some("--format") if a[0] == "ports" => Output::ok(&rows),
            _ => sbx_answer(a),
        });
        app.release_busy_ports(&SandboxName::parse("chat").unwrap(), &Config::default()).await;
        assert!(fake.called(&format!("ports chat --unpublish 127.0.0.1:{busy}:8000")), "{:?}", fake.log());
        assert!(!fake.log().iter().any(|c| c.contains(&format!(":{free}:3000"))), "a free port stays published");
    }

    #[tokio::test]
    async fn a_thread_starts_a_stopped_sandbox_itself_ports_first() {
        let held = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let busy = held.local_addr().unwrap().port();
        let rows = format!(r#"[{{"host_ip":"127.0.0.1","host_port":{busy},"sandbox_port":8000,"protocol":"tcp"}}]"#);
        let (_d, app, fake) = app_with(move |a: &[String]| match a.first().map(String::as_str) {
            Some("ls") => crate::testing::ls(&[("neos", "stopped")]),
            Some("ports") if a.get(2).map(String::as_str) == Some("--format") => Output::ok(&rows),
            _ => sbx_answer(a),
        });
        app.ensure_running_for_thread(&SandboxName::parse("neos").unwrap()).await.unwrap();
        let log = fake.log();
        let at = |p: &str| log.iter().position(|c| c.starts_with(p)).unwrap_or_else(|| panic!("no `{p}` in {log:?}"));
        assert!(at("ports neos --unpublish") < at("exec neos -- true"), "{log:?}");
        assert!(!log.iter().any(|c| c.starts_with("run ")), "no main session opened for it: {log:?}");
    }

    #[tokio::test]
    async fn a_thread_on_a_missing_sandbox_is_an_error() {
        let (_d, app, _fake) = app_with(|a: &[String]| match a.first().map(String::as_str) {
            Some("exec") => Output::fail(sbxw_sbx::EXEC_NOT_RUN, "sandbox 'gone' not found"),
            _ => sbx_answer(a),
        });
        let e = app.ensure_running_for_thread(&SandboxName::parse("gone").unwrap()).await.unwrap_err();
        assert!(format!("{e:#}").contains("not found"), "{e:#}");
    }

    /// `sbx run` on a stopped sandbox: a line, a long silence while the
    /// container starts, then Claude Code. Typing must wait for the second.
    #[tokio::test]
    async fn a_cold_agent_is_typed_into_only_once_claude_code_is_up() {
        use crate::pty::{PtySession, Spawn};
        use std::time::{Duration, Instant};
        let (_d, app, _fake) = app_with(sbx_answer);
        let name = SandboxName::parse("ephemeral-chat").unwrap();
        let s = PtySession::spawn(
            Spawn {
                program: "sh".into(),
                args: vec!["-c".into(), "printf 'starting…'; sleep 2; printf 'claude> '; sleep 5".into()],
                env: vec![],
            },
            0,
            || {},
        )
        .unwrap();
        let mut events = app.bus.subscribe();
        let hook = {
            let app = app.clone();
            let name = name.clone();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(2100)).await;
                app.ingest_hook(&name, serde_json::json!({"hook_event_name": "SessionStart", "session_id": "s1"}));
            })
        };
        let t = Instant::now();
        app.await_cold_agent(&name, &s, &mut events).await;
        hook.await.unwrap();
        // Past the container's silence, and the frame's settle after it.
        assert!(t.elapsed() >= Duration::from_millis(2900), "{:?}", t.elapsed());
        assert!(s.replay().ends_with(b"claude> "));
    }
}
