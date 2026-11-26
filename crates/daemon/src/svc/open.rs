//! Opening a terminal session: which command each kind runs, and the rules
//! around threads (their repo, their model, the session ceiling, a stopped
//! sandbox).

use crate::app::{now_ms, App};
use crate::bus::Event;
use crate::git;
use crate::svc::timings;
use crate::pty::{PtySession, Spawn};
use crate::typing;
use anyhow::{anyhow, bail, Result, Context};
use futures_util::future::BoxFuture;
use sbxw_core::agents::SessionState;
use sbxw_core::config::Config;
use sbxw_core::{ports, remote};
use sbxw_core::session::{SessionKey, SessionKind};
use sbxw_core::{ModelId, SandboxRef, ThreadName};
use sbxw_sbx::host::HostCmd;
use sbxw_sbx::scripts;
use std::sync::Arc;
use std::time::Duration;

/// How long a new thread session is watched for an early end.
const THREAD_START_WATCH: Duration = Duration::from_secs(90);

/// The last line a session printed, escapes and `[sbxw] ` taken off.
fn last_line(replay: &[u8]) -> Option<String> {
    let text = sbxw_sbx::runner::strip_ansi(&String::from_utf8_lossy(replay));
    text.lines()
        .map(|l| l.trim().trim_start_matches("[sbxw]").trim())
        .rfind(|l| !l.is_empty())
        .map(|l| l.chars().take(500).collect())
}

/// A stopped sandbox starts its container, then boots Claude Code.
const COLD_START_TIMEOUT: Duration = Duration::from_secs(120);

/// Is a host port held by someone else? Either a connect answers or a bind
/// fails. Both are needed: std binds with `SO_REUSEADDR`, which on macOS lets a
/// wildcard bind succeed beside a listener on one address.
fn host_port_busy(ip: &str, port: u16) -> bool {
    let ip: std::net::IpAddr = ip.parse().unwrap_or(std::net::Ipv4Addr::LOCALHOST.into());
    let probe = if ip.is_unspecified() { std::net::Ipv4Addr::LOCALHOST.into() } else { ip };
    let addr = std::net::SocketAddr::new(probe, port);
    std::net::TcpStream::connect_timeout(&addr, Duration::from_millis(200)).is_ok()
        || std::net::TcpListener::bind(std::net::SocketAddr::new(ip, port)).is_err()
}

impl App {
    /// [`host_port_busy`] on the sandbox's host: here, or asked over its
    /// link (`ss` lists the host's listeners; a host without it says free).
    async fn port_busy_for(&self, sb: &SandboxRef, ip: &str, port: u16) -> bool {
        if sb.is_local() {
            return host_port_busy(ip, port);
        }
        let Ok(host) = self.host_for(sb).await else { return false };
        let script = r#"ss -Hltn 2>/dev/null | awk '{print $4}' | grep -Eq "[:.]$1\$""#;
        let cmd = HostCmd::new("sh", vec!["-c".into(), script.into(), "sh".into(), port.to_string()]);
        matches!(host.exec.exec(cmd).await, Ok(o) if o.success())
    }

    /// The session under `key`, opened if it is not running. Returns whether
    /// this call started it.
    ///
    /// Boxed, because opening a thread on a stopped sandbox opens its agent
    /// first — the one recursion in here.
    pub fn open_session<'a>(self: &'a Arc<Self>, key: &'a SessionKey) -> BoxFuture<'a, Result<(Arc<PtySession>, bool)>> {
        Box::pin(async move {
            let app = self.clone();
            let k = key.clone();
            // The attach a bring-up is followed by, timed on its timeline
            // (`svc::timings`): only a main agent session this call starts.
            let attach = match (&key.sandbox, &key.kind) {
                (Some(sb), SessionKind::Agent) if !self.sessions.is_live(key) => self.timing_begin(sb, timings::step::ATTACH),
                _ => None,
            };
            let opened = self
                .sessions
                .get_or_open(key, now_ms(), move || async move { app.spawn_for(&k).await })
                .await;
            if let Some(late) = attach {
                match &opened {
                    Ok((_, true)) => self.timing_end(late, Some("sbx run spawned".into())),
                    _ => self.timing_drop(late),
                }
            }
            let opened = opened?;
            // A stopped sandbox starts with its agent: its kits' startups are
            // watched from its session's start.
            if let (true, Some(sb), SessionKind::Agent) = (opened.1, &key.sandbox, &key.kind) {
                if let Some((id, start, resolved)) = self.timing_watch_at_attach(sb) {
                    self.watch_kit_startups(sb.clone(), resolved, id, start, true);
                }
            }
            match (opened.1, &key.sandbox, &key.kind) {
                (true, Some(sb), SessionKind::Thread(t)) => self.watch_thread_start(sb.clone(), t.clone(), opened.0.clone()),
                // A new main terminal: a leader from before a restart may be in it.
                (true, Some(sb), SessionKind::Agent) => self.rebind_lead(sb),
                _ => {}
            }
            Ok(opened)
        })
    }

    /// A thread's session that ends before the thread ever reported (its
    /// script refused, `claude` would not start) leaves its plan thread at
    /// "starting" for good: the end is recorded as a failed start, with the
    /// last line it printed as the reason.
    fn watch_thread_start(self: &Arc<Self>, sandbox: SandboxRef, thread: ThreadName, session: Arc<PtySession>) {
        let app = self.clone();
        tokio::spawn(async move {
            let deadline = tokio::time::Instant::now() + THREAD_START_WATCH;
            while session.alive() && tokio::time::Instant::now() < deadline {
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
            if session.alive() {
                return;
            }
            let starting = app
                .plans
                .lock()
                .unwrap()
                .approved_threads(&sandbox.to_string())
                .iter()
                .any(|(_, t)| t.name == thread.as_str() && t.state == sbxw_core::plans::ThreadState::Starting);
            if starting {
                let reason = match last_line(&session.replay()) {
                    Some(l) => format!("its session ended before the thread reported: {l}"),
                    None => "its session ended before the thread reported".to_string(),
                };
                app.thread_start_failed(&sandbox, &thread, &reason);
            }
        });
    }

    async fn spawn_for(self: &Arc<Self>, key: &SessionKey) -> Result<Spawn> {
        let spawn = self.spawn_here(key).await?;
        match &key.sandbox {
            Some(sb) if !sb.is_local() && spawn.program == "sbx" => self.over_link(sb, spawn).await,
            _ => Ok(spawn),
        }
    }

    /// A remote sandbox's `sbx …` as a local PTY runs it: `ssh -tt` through
    /// its host's link, with the host's own `sbx`.
    async fn over_link(&self, sb: &SandboxRef, spawn: Spawn) -> Result<Spawn> {
        let r = self.ready_remote(sb).await?;
        let exec = r.link.exec();
        let sbx = exec.sbx_program().await?;
        let (program, args) = sbxw_sbx::host::HostExec::tty_argv(&*exec, &sbx, &spawn.args);
        Ok(Spawn { program, args, env: spawn.env })
    }

    async fn spawn_here(self: &Arc<Self>, key: &SessionKey) -> Result<Spawn> {
        let Some(sandbox) = key.sandbox.clone() else {
            let cmd = &self.cfg.monitor_cmd;
            let Some((program, args)) = cmd.split_first() else {
                bail!("no monitor command configured — set `monitor_cmd` in sbxw.toml");
            };
            return Ok(Spawn { program: program.clone(), args: args.to_vec(), env: vec![] });
        };
        let sbx = self.sbx_for(&sandbox).await?;
        // A chat workspace under /tmp may have been swept away; `sbx run` then
        // refuses to start the sandbox. It is empty by definition: re-create it.
        if let Some(dir) = self.store.chat_workspace(&sandbox) {
            if !dir.exists() {
                let _ = std::fs::create_dir_all(&dir);
                tracing::info!("re-created vanished chat workspace {}", dir.display());
            }
        }
        let cfg = self.config_for(&sandbox).await;
        // The two kinds that start a stopped sandbox: `sbx run` and `sbx exec`
        // (threads release ports in `ensure_running_for_thread`).
        let late = matches!(key.kind, SessionKind::Agent).then(|| self.timing_begin(&sandbox, timings::step::ATTACH_LS)).flatten();
        let running = match key.kind {
            SessionKind::Agent | SessionKind::Bash => Some(sbx.is_running(&sandbox.name).await),
            _ => None,
        };
        if let Some(late) = late {
            self.timing_end(late, None);
        }
        if running == Some(false) {
            self.release_busy_ports(&sandbox, &cfg).await;
        }
        // Jarvis's role and tools, written before its Claude session starts,
        // which reads them only then: how one an older daemon made is repaired.
        if matches!(key.kind, SessionKind::Agent) && self.is_jarvis(&sandbox) {
            self.jarvis_setup(running == Some(true)).await;
        }
        Ok(match &key.kind {
            SessionKind::Monitor => unreachable!("the monitor has no sandbox"),
            // Only a kit the daemon recorded: the pane never names one.
            SessionKind::KitAdd => {
                let kit = self.kit_approvals.lock().unwrap().get_mut(&sandbox).filter(|v| !v.is_empty()).map(|v| v.remove(0));
                let Some(kit) = kit else {
                    bail!("no kit is waiting for your approval in '{sandbox}' — bring it up again to retry");
                };
                Spawn::sbx(sbxw_sbx::Sbx::kit_add_args(&sandbox.name, &kit))
            }
            SessionKind::Agent if cfg.web_shell.is_empty() => {
                let env = if self.store.get(&sandbox).remote_control {
                    remote::without_auth_env(cfg.env_pairs())
                } else {
                    cfg.env_pairs()
                };
                // Claude in Chrome: only with sbx's own setting on, read now
                // so switching it off takes effect at the next attach.
                let rec = self.store.get(&sandbox);
                let chrome = match cfg.chrome || rec.chrome {
                    // Not read: the agent starts without --chrome, and says why.
                    true => {
                        let late = self.timing_begin(&sandbox, timings::step::ATTACH_SETTING);
                        let read = sbx.setting_bool(sbxw_core::chrome::SETTING, sbxw_core::chrome::SINCE).await.unwrap_or_else(|e| {
                            tracing::warn!("'{sandbox}' starts without --chrome, sbx's setting could not be read: {e:#}");
                            None
                        });
                        if let Some(late) = late {
                            self.timing_end(late, None);
                        }
                        read
                    }
                    false => None,
                };
                let flags = sbxw_core::chrome::agent_flags(cfg.chrome || rec.chrome, chrome);
                Spawn::sbx(sbxw_sbx::attach_args(&sandbox.name, &env, &cfg.env_files, &rec.attach_workdir(&cfg.workdir), flags))
            }
            SessionKind::Agent => Spawn::sbx(sbxw_sbx::shell_args(&sandbox.name, &cfg.web_shell)),
            // `sbx exec` starts a stopped sandbox first (0.50).
            SessionKind::Bash => Spawn::sbx(sbxw_sbx::bash_args(&sandbox.name)),
            // A session in the main checkout has no worktree: its shell is the
            // sandbox's own.
            SessionKind::ThreadBash(thread) if self.is_same_worktree(&sandbox, thread) => {
                Spawn::sbx(sbxw_sbx::bash_args(&sandbox.name))
            }
            SessionKind::ThreadBash(thread) => {
                self.ensure_running_for_thread(&sandbox).await?;
                let dir = self.settle_thread_repo(&sandbox, thread, None).await.map_err(|e| anyhow!(e))?;
                Spawn::sbx(scripts::thread_bash_args(&sandbox.name, &dir, thread, self.team_for(&sandbox, thread).as_ref()))
            }
            SessionKind::Thread(thread) => {
                // Whatever stops a plan's thread from starting is recorded on
                // it, with the reason, rather than left at "starting".
                let spawn = self.spawn_thread(&sandbox, thread, &cfg).await;
                match &spawn {
                    Err(e) => self.thread_start_failed(&sandbox, thread, &format!("{e:#}")),
                    Ok(_) => self.thread_start_cleared(&sandbox, thread),
                }
                spawn?
            }
        })
    }

    /// The command for thread `thread`'s session, its worktree made first.
    async fn spawn_thread(self: &Arc<Self>, sandbox: &SandboxRef, thread: &ThreadName, cfg: &Config) -> Result<Spawn> {
        let sandbox = sandbox.clone();
        // A shared plan's worktree is named like a thread but is not one:
        // a session opened under its name would find no briefing and
        // resume whichever conversation that folder holds.
        if self.plans.lock().unwrap().of(&sandbox.to_string()).iter().any(|p| p.team_name() == thread.as_str()) {
            bail!("'{thread}' is the worktree a shared plan's threads share, not a thread: open one of its threads instead");
        }
        self.ensure_running_for_thread(&sandbox).await?;
        // The ceiling refuses a *new* thread only; the agent never.
        let open = self.sessions.agents_open(&sandbox.to_string());
        let max = cfg.max_sessions();
        if open >= max {
            bail!(
                "'{sandbox}' already has {open} Claude session{} open, and max_agent_sessions \
                 is {max} (sbxw.toml). Close a thread first, or raise the ceiling.",
                if open == 1 { "" } else { "s" }
            );
        }
        let dir = self.settle_thread_repo(&sandbox, thread, None).await.map_err(|e| anyhow!(e))?;
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
        if self.is_same_worktree(&sandbox, thread) {
            return Ok(Spawn::sbx(scripts::same_worktree_run_args(&sandbox.name, &dir, thread, model.as_ref())));
        }
        let team = self.team_for(&sandbox, thread);
        // The worktree first, on its own: a failure here is a reason (a root
        // the agent cannot write, git refusing), not a pane that dies.
        self.sbx_for(&sandbox)
            .await?
            .thread_prepare(&sandbox.name, &dir, thread, team.as_ref())
            .await
            .map_err(|e| match self.split_note(&sandbox) {
                Some(note) => anyhow!("{e:#} {note}"),
                None => e,
            })
            .with_context(|| format!("thread {thread}: its worktree could not be made"))?;
        // The briefing an approval left is taken only now: a start that
        // failed above keeps it for the retry. Without one, this is a reopen:
        // the script resumes the thread's conversation, and only failing that
        // starts it with its plan's goal, said as a restart, never blank.
        let pending = self.briefings.lock().unwrap().remove(&(sandbox.clone(), thread.to_string()));
        let (prompt, mode) = match pending {
            Some(b) => (b, scripts::StartMode::New),
            None => {
                let plans = self.plans.lock().unwrap();
                let restart = plans
                    .thread_plan(&sandbox.to_string(), thread.as_str())
                    .map(|(p, t)| sbxw_core::messages::thread_restart_briefing(p, t))
                    .unwrap_or_default();
                (restart, scripts::StartMode::Reopen)
            }
        };
        // Passed to Claude Code as its first prompt: sbxw's words, not the person's.
        if !prompt.is_empty() {
            self.note_typed(&sandbox.to_string(), &prompt);
        }
        Ok(Spawn::sbx(scripts::thread_run_args(&sandbox.name, &dir, thread, &prompt, model.as_ref(), team.as_ref(), mode)))
    }

    /// A stopped sandbox keeps the mappings it was created with (`-p`), and
    /// sbx refuses to start it while any of their host ports is taken. A port
    /// is never a reason not to start: each busy one is unpublished first, and
    /// said. sbx's own list is the truth; the config's specs back it up in
    /// case it is empty while stopped (a needless unpublish fails harmlessly).
    async fn release_busy_ports(&self, sandbox: &SandboxRef, cfg: &Config) {
        let Ok(sbx) = self.sbx_for(sandbox).await else { return };
        let mut candidates: Vec<(String, u16, String)> =
            sbx.ports(&sandbox.name).await.into_iter().map(|m| (m.host_ip.clone(), m.host_port, m.spec())).collect();
        let wanted = cfg.wanted_ports(&[]);
        for (i, (p, spec)) in wanted.iter().zip(ports::publish_specs(&wanted, cfg.ip_per_app)).enumerate() {
            if !candidates.iter().any(|(_, port, _)| *port == p.host_port) {
                candidates.push((ports::host_ip_for(cfg.ip_per_app, i), p.host_port, spec));
            }
        }
        for (ip, port, spec) in candidates {
            if !self.port_busy_for(sandbox, &ip, port).await {
                continue;
            }
            match sbx.unpublish(&sandbox.name, &spec).await {
                Ok(()) => tracing::warn!(
                    "host port {port} is taken: starting '{sandbox}' without its {spec} mapping; free the \
                     port and run `sbxw ports {sandbox}` to publish it again"
                ),
                Err(e) => tracing::debug!("could not unpublish {spec} from '{sandbox}' before starting it: {e:#}"),
            }
        }
    }

    /// A thread runs through `sbx exec`, which starts a stopped sandbox itself
    /// (0.50), so there is no main session to open. It is started here rather
    /// than by the thread's own exec, so busy host ports are released first and
    /// a failure is an error, not a pane that dies.
    pub async fn ensure_running_for_thread(self: &Arc<Self>, sandbox: &SandboxRef) -> Result<()> {
        let sbx = self.sbx_for(sandbox).await?;
        if sbx.is_running(&sandbox.name).await {
            return Ok(());
        }
        tracing::info!("'{sandbox}' is stopped — starting it for a thread");
        self.release_busy_ports(sandbox, &self.config_for(sandbox).await).await;
        sbx.start(&sandbox.name).await.with_context(|| format!("starting '{sandbox}' for a thread"))
    }

    /// Which repo `thread` works in, settled and recorded, as the directory
    /// its scripts start in. Every thread path goes through here (its
    /// session, its Bash, status, close, merge) so they cannot disagree.
    pub async fn settle_thread_repo(&self, sandbox: &SandboxRef, thread: &ThreadName, requested: Option<&str>) -> Result<String, String> {
        // A session in the main checkout starts where the main agent does.
        if self.is_same_worktree(sandbox, thread) {
            return Ok(self.store.workspace(sandbox).map(|w| w.to_string_lossy().into_owned()).unwrap_or_default());
        }
        let Some(ws) = self.store.workspace(sandbox) else {
            return match requested {
                Some(_) => Err(format!("sbxw has no workspace recorded for '{sandbox}', so it cannot pick a repo in it")),
                None => Ok(String::new()),
            };
        };
        let recorded = self.store.get(sandbox).threads.get(thread.as_str()).map(|r| r.repo.clone());
        let host = self.host_for(sandbox).await.map_err(|e| format!("{e:#}"))?;
        let chosen = git::choose(&host, &ws, recorded.as_deref(), thread, requested).await?;
        if recorded.as_deref() != Some(chosen.as_str()) {
            self.store
                .update(sandbox, |r| r.threads.entry(thread.to_string()).or_default().repo = chosen.clone())
                .map_err(|e| format!("could not record the repo of thread {thread}: {e:#}"))?;
        }
        Ok(git::thread_dir(&ws, &chosen).to_string_lossy().into_owned())
    }

    /// When sbxw itself created `sandbox` with its main workspace split by a
    /// strict carve (only some of its folders mounted, the root a folder
    /// nothing can be created in), a sentence saying so: the cause of a
    /// worktree that cannot be made and of tracked files missing from the
    /// checkout, read from what sbxw recorded at creation, not guessed.
    pub fn split_note(&self, sandbox: &SandboxRef) -> Option<String> {
        let rec = self.store.get(sandbox);
        let main = rec.carve.as_ref()?.split.as_deref()?.to_string();
        Some(format!(
            "sbxw created this sandbox with a strict carve that splits its workspace {main}: only some of \
             its folders are mounted, the files beside them are not, and nothing can be created at its \
             root. For a repository, make that carve enforce = \"soft\" in sbxw.toml, then recreate the \
             sandbox (sbxw rm, then sbxw up)."
        ))
    }

    /// Is `thread` a session in the main checkout rather than in a worktree?
    pub fn is_same_worktree(&self, sandbox: &SandboxRef, thread: &ThreadName) -> bool {
        self.store.get(sandbox).threads.get(thread.as_str()).is_some_and(|r| r.same_worktree)
    }

    /// Record that `thread` is a session in the main checkout, on its first
    /// open. A name that already has a worktree is refused: it would start a
    /// second session in a folder the thread's own worktree does not use.
    pub async fn settle_same_worktree(&self, sandbox: &SandboxRef, thread: &ThreadName) -> Result<(), String> {
        let rec = self.store.get(sandbox);
        match rec.threads.get(thread.as_str()) {
            Some(r) if r.same_worktree => return Ok(()),
            Some(_) => return Err(format!("'{thread}' is already a thread with a worktree of its own: pick another name")),
            None => {}
        }
        if let Some(ws) = &rec.workspace {
            let host = self.host_for(sandbox).await.map_err(|e| format!("{e:#}"))?;
            if git::thread_worktrees(&host, ws, &Default::default()).await.iter().any(|(t, _)| t == thread.as_str()) {
                return Err(format!("'{thread}' is already a thread with a worktree of its own: pick another name"));
            }
        }
        self.store
            .update(sandbox, |r| r.threads.entry(thread.to_string()).or_default().same_worktree = true)
            .map_err(|e| format!("could not record session {thread}: {e:#}"))
    }

    /// Record a thread's model the first time one is asked for: it keeps the
    /// model it was opened on, as it keeps its repo. `force` (a plan says so)
    /// replaces one recorded earlier.
    pub fn settle_thread_model(&self, sandbox: &SandboxRef, thread: &ThreadName, model: &ModelId, force: bool) -> Result<(), String> {
        self.store
            .update(sandbox, |r| {
                let t = r.threads.entry(thread.to_string()).or_default();
                if force || t.model.is_none() {
                    t.model = Some(model.to_string());
                }
            })
            .map_err(|e| format!("could not record the model of thread {thread}: {e:#}"))
    }

    /// Type a line sbxw wrote itself into a session and submit it, opening the
    /// session first if it is not running. The line is recorded as sbxw's, so
    /// the conversation does not show it as the person's.
    pub async fn push_text(self: &Arc<Self>, sandbox: &SandboxRef, kind: SessionKind, text: &str) -> Result<(), String> {
        self.note_typed(&sandbox.to_string(), text);
        self.push_as_person(sandbox, kind, text).await
    }

    /// Type text into a session and submit it, recording nothing: the
    /// person's own words (the composer), or a caller that recorded already.
    pub async fn push_as_person(self: &Arc<Self>, sandbox: &SandboxRef, kind: SessionKind, text: &str) -> Result<(), String> {
        let key = SessionKey::new(sandbox.clone(), kind.clone());
        let cold = kind == SessionKind::Agent
            && !self.sessions.is_live(&key)
            && !self.sbx_for(sandbox).await.map_err(|e| format!("{e:#}"))?.is_running(&sandbox.name).await;
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

    /// The agent of a stopped sandbox: `sbx run` goes quiet while the
    /// container starts, before Claude Code boots — a silence typing would
    /// read as a drawn prompt, losing the message. Claude Code's
    /// `SessionStart` hook says when it is up; its first frame is then let
    /// settle. The output is watched from the start, so a frame drawn before
    /// the hook arrives is not waited for again.
    async fn await_cold_agent(&self, sandbox: &SandboxRef, session: &Arc<PtySession>, events: &mut tokio::sync::broadcast::Receiver<Event>) {
        let mut output = session.subscribe();
        tracing::info!("'{sandbox}' is stopped — waiting for its agent to start before typing");
        let started = async {
            loop {
                match events.recv().await {
                    Ok(Event::Session(info)) if info.sandbox == sandbox.to_string() && info.state == SessionState::Idle => return true,
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
    pub fn spawn_push(self: &Arc<Self>, sandbox: SandboxRef, kind: SessionKind, text: String, on_fail: Option<Box<dyn FnOnce(String) + Send>>) {
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
    use sbxw_core::SandboxRef;
    use sbxw_sbx::runner::Output;

    fn plan_thread(name: &str) -> sbxw_core::plans::PlanThread {
        sbxw_core::plans::PlanThread {
            name: name.into(),
            goal: "do the api".into(),
            scope: "src/api".into(),
            repo: None,
            model: None,
            state: sbxw_core::plans::ThreadState::Starting,
            summary: String::new(),
            updated_ms: 0,
            set_by: None,
            ticket: None,
        }
    }

    #[tokio::test]
    async fn a_thread_that_cannot_make_its_worktree_fails_with_the_reason_and_a_retry_clears_it() {
        use sbxw_core::plans::{SetBy, ThreadState};
        let refuse = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
        let flag = refuse.clone();
        let (_d, app, _fake) = app_with(move |a: &[String]| match a.first().map(String::as_str) {
            Some("ls") => crate::testing::ls(&[("myapp", "running")]),
            Some("exec") if a.iter().any(|x| x == "sbxw-thread-prepare") && flag.load(std::sync::atomic::Ordering::SeqCst) => {
                Output::ok("error=cannot create /w/.sbxw-worktrees: /w is not writable by agent in this sandbox (owner root).\n")
            }
            Some("exec") if a.iter().any(|x| x == "sbxw-thread-prepare") => Output::ok("worktree=/w/.sbxw-worktrees/api\nok=1\n"),
            _ => sbx_answer(a),
        });
        let sb = SandboxRef::parse("myapp").unwrap();
        let ws = tempfile::tempdir().unwrap();
        std::fs::create_dir(ws.path().join(".git")).unwrap();
        app.store.update(&sb, |r| r.workspace = Some(ws.path().to_path_buf())).unwrap();
        let p = app
            .plan_propose(&sb, "s", vec![plan_thread("api")], sbxw_core::plans::Variant::Split, "one", Some("lead"))
            .await
            .unwrap();
        app.plans.lock().unwrap().approve(&p.id, vec![plan_thread("api")], None, None, 1).unwrap();
        app.briefings.lock().unwrap().insert((sb.clone(), "api".into()), "the briefing".into());
        let key = sbxw_core::session::SessionKey::thread(&sb, &sbxw_core::ThreadName::parse("api").unwrap());

        let err = format!("{:#}", app.open_session(&key).await.err().expect("refused"));
        assert!(err.contains("not writable by agent"), "{err}");
        assert!(!err.contains("strict carve"), "said only when sbxw split the workspace: {err}");
        let t = app.plan(&p.id).unwrap().threads[0].clone();
        assert_eq!((t.state, t.set_by), (ThreadState::Failed, Some(SetBy::Sbxw)));
        assert!(t.summary.contains("its worktree could not be made") && t.summary.contains("not writable"), "{}", t.summary);
        let view = app.plan_view(&app.plan(&p.id).unwrap());
        assert_eq!(view["threads"][0]["state"], "failed");
        assert_eq!(view["threads"][0]["set_by"], "sbxw");
        assert!(app.briefings.lock().unwrap().contains_key(&(sb.clone(), "api".into())), "the briefing waits for the retry");

        app.store
            .update(&sb, |r| r.carve = Some(crate::store::CarveRecord { file: vec![], extra: vec![], split: Some("/w".into()) }))
            .unwrap();
        let err = format!("{:#}", app.open_session(&key).await.err().expect("refused"));
        assert!(err.contains("strict carve that splits its workspace /w") && err.contains("enforce = \"soft\""), "{err}");
        app.store.update(&sb, |r| r.carve = None).unwrap();

        refuse.store(false, std::sync::atomic::Ordering::SeqCst);
        let spawn = app.spawn_for(&key).await.expect("the retry starts it");
        assert!(spawn.args.iter().any(|a| a == "=the briefing"), "{:?}", spawn.args);
        let t = app.plan(&p.id).unwrap().threads[0].clone();
        assert_eq!((t.state, t.set_by, t.summary.as_str()), (ThreadState::Starting, None, ""), "cleared");
        assert_eq!(spawn.args.last().map(String::as_str), Some("=new"));
    }

    #[tokio::test]
    async fn a_reopened_plan_thread_gets_its_goal_said_as_a_restart_never_nothing() {
        // The pane after a daemon restart: the approval's briefing is gone
        // (it lived in memory), the plan is not.
        let (_d, app, _fake) = app_with(|a: &[String]| match a.first().map(String::as_str) {
            Some("ls") => crate::testing::ls(&[("myapp", "running")]),
            _ => sbx_answer(a),
        });
        let sb = SandboxRef::parse("myapp").unwrap();
        let ws = tempfile::tempdir().unwrap();
        std::fs::create_dir(ws.path().join(".git")).unwrap();
        app.store.update(&sb, |r| r.workspace = Some(ws.path().to_path_buf())).unwrap();
        let p = app
            .plan_propose(&sb, "the whole plan", vec![plan_thread("api")], sbxw_core::plans::Variant::Split, "one", Some("lead"))
            .await
            .unwrap();
        app.plans.lock().unwrap().approve(&p.id, vec![plan_thread("api")], None, None, 1).unwrap();
        let key = sbxw_core::session::SessionKey::thread(&sb, &sbxw_core::ThreadName::parse("api").unwrap());
        let spawn = app.spawn_for(&key).await.unwrap();
        let n = spawn.args.len();
        assert_eq!(spawn.args[n - 1], "=reopen");
        let prompt = &spawn.args[n - 5];
        assert!(prompt.starts_with("=[sbxw] This is a restart of thread \"api\""), "{prompt}");
        assert!(prompt.contains("Your goal: do the api") && prompt.contains("report_thread_status") && prompt.contains("\"lead\""), "{prompt}");

        // A thread of no plan reopens as before: nothing to say, resume if it can.
        let solo = sbxw_core::session::SessionKey::thread(&sb, &sbxw_core::ThreadName::parse("solo").unwrap());
        let spawn = app.spawn_for(&solo).await.unwrap();
        let n = spawn.args.len();
        assert_eq!((spawn.args[n - 5].as_str(), spawn.args[n - 1].as_str()), ("=", "=reopen"));
    }

    #[tokio::test]
    async fn a_second_open_of_a_running_thread_attaches_and_starts_or_stops_nothing() {
        // Web and desktop on one thread, or a pane closed and reopened: the
        // daemon still holds the thread's PTY, so the open is that PTY. No
        // script runs, so nothing in the sandbox is stopped or started.
        let (_d, app, fake) = app_with(sbx_answer);
        let sb = SandboxRef::parse("myapp").unwrap();
        let key = sbxw_core::session::SessionKey::thread(&sb, &sbxw_core::ThreadName::parse("api").unwrap());
        let sh = crate::pty::Spawn { program: "sh".into(), args: vec!["-c".into(), "sleep 5".into()], env: vec![] };
        let (first, _) = app.sessions.get_or_open(&key, 0, || async { Ok(sh) }).await.unwrap();
        let before = fake.log().len();
        for _ in 0..2 {
            let (again, fresh) = app.open_session(&key).await.unwrap();
            assert!(std::sync::Arc::ptr_eq(&first, &again) && !fresh, "the same terminal");
        }
        assert_eq!(fake.log().len(), before, "no sbx call at all: {:?}", &fake.log()[before..]);
        app.sessions.drop_sandbox("myapp");
    }

    #[test]
    fn the_reason_of_an_early_end_is_the_last_line_printed() {
        let out = b"\x1b[2m[sbxw] thread api - worktree /w\x1b[0m\r\n\x1b[31m[sbxw] cannot create /w/.sbxw-worktrees: denied\x1b[0m\r\n\r\n";
        assert_eq!(super::last_line(out).as_deref(), Some("cannot create /w/.sbxw-worktrees: denied"));
        assert_eq!(super::last_line(b"\r\n"), None);
    }

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
        app.release_busy_ports(&SandboxRef::parse("chat").unwrap(), &Config::default()).await;
        assert!(fake.called(&format!("ports chat --unpublish 127.0.0.1:{busy}:8000")), "{:?}", fake.log());
        assert!(!fake.log().iter().any(|c| c.contains(&format!(":{free}:3000"))), "a free port stays published");
    }

    #[tokio::test]
    async fn a_thread_starts_a_stopped_sandbox_itself_ports_first() {
        let held = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let busy = held.local_addr().unwrap().port();
        let rows = format!(r#"[{{"host_ip":"127.0.0.1","host_port":{busy},"sandbox_port":8000,"protocol":"tcp"}}]"#);
        let (_d, app, fake) = app_with(move |a: &[String]| match a.first().map(String::as_str) {
            Some("ls") => crate::testing::ls(&[("myapp", "stopped")]),
            Some("ports") if a.get(2).map(String::as_str) == Some("--format") => Output::ok(&rows),
            _ => sbx_answer(a),
        });
        app.ensure_running_for_thread(&SandboxRef::parse("myapp").unwrap()).await.unwrap();
        let log = fake.log();
        let at = |p: &str| log.iter().position(|c| c.starts_with(p)).unwrap_or_else(|| panic!("no `{p}` in {log:?}"));
        assert!(at("ports myapp --unpublish") < at("exec myapp -- true"), "{log:?}");
        assert!(!log.iter().any(|c| c.starts_with("run ")), "no main session opened for it: {log:?}");
    }

    #[tokio::test]
    async fn a_thread_on_a_missing_sandbox_is_an_error() {
        let (_d, app, _fake) = app_with(|a: &[String]| match a.first().map(String::as_str) {
            Some("exec") => Output::fail(sbxw_sbx::EXEC_NOT_RUN, "sandbox 'gone' not found"),
            _ => sbx_answer(a),
        });
        let e = app.ensure_running_for_thread(&SandboxRef::parse("gone").unwrap()).await.unwrap_err();
        assert!(format!("{e:#}").contains("not found"), "{e:#}");
    }

    /// The agent may rewrite the workspace's `sbxw.toml` at any time; an
    /// attach runs with what the last bring-up recorded, never that file.
    #[tokio::test]
    async fn an_attach_runs_with_the_bring_up_snapshot_not_the_edited_file() {
        use sbxw_core::session::SessionKey;
        let (d, app, _fake) = app_with(sbx_answer);
        let toml = d.path().join("sbxw.toml");
        std::fs::write(&toml, "env_files = [\"/Users/me/.aws/credentials\"]\nworkdir = \"/etc\"\nweb_shell = \"\"\n").unwrap();
        let sb = SandboxRef::parse("myapp").unwrap();
        let snap = Config { env_files: vec!["/ok/.env".into()], ..Config::default() };
        app.store
            .update(&sb, |r| {
                r.config = Some(toml.clone());
                r.config_snapshot = Some(Box::new(snap));
            })
            .unwrap();
        let spawn = app.spawn_here(&SessionKey::agent(&sb)).await.unwrap();
        let args = spawn.args.join(" ");
        assert!(args.contains("--env-file /ok/.env"), "{args}");
        assert!(!args.contains(".aws") && !args.contains("/etc"), "{args}");
        assert_eq!(app.config_for(&sb).await.source.as_deref(), Some(toml.as_path()));

        // A record from before snapshots: the daemon's config, not the file.
        app.store.update(&sb, |r| r.config_snapshot = None).unwrap();
        let args = app.spawn_here(&SessionKey::agent(&sb)).await.unwrap().args.join(" ");
        assert!(!args.contains(".aws") && !args.contains("--env-file"), "{args}");
    }

    /// Claude in Chrome: the flag follows the sandbox's wish *and* sbx's own
    /// setting, read at each attach.
    #[tokio::test]
    async fn the_agent_starts_with_chrome_only_while_sbx_allows_it() {
        use sbxw_core::session::SessionKey;
        let on = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
        let o = on.clone();
        let (_d, app, fake) = app_with(move |a: &[String]| match a.first().map(String::as_str) {
            Some("settings") => Output::ok(if o.load(std::sync::atomic::Ordering::SeqCst) { "true\n" } else { "false\n" }),
            _ => sbx_answer(a),
        });
        let sb = SandboxRef::parse("myapp").unwrap();
        let args = app.spawn_here(&SessionKey::agent(&sb)).await.unwrap().args.join(" ");
        assert!(!args.contains("--chrome"), "{args}");
        assert!(!fake.log().iter().any(|c| c.starts_with("settings")), "not asked for, not looked up");

        app.store.update(&sb, |r| r.chrome = true).unwrap();
        let args = app.spawn_here(&SessionKey::agent(&sb)).await.unwrap().args.join(" ");
        assert!(args.ends_with("-- --chrome"), "{args}");

        on.store(false, std::sync::atomic::Ordering::SeqCst);
        let args = app.spawn_here(&SessionKey::agent(&sb)).await.unwrap().args.join(" ");
        assert!(!args.contains("--chrome"), "switched off on the host: {args}");
    }

    /// `sbx run` on a stopped sandbox: a line, a long silence while the
    /// container starts, then Claude Code. Typing must wait for the second.
    #[tokio::test]
    async fn a_cold_agent_is_typed_into_only_once_claude_code_is_up() {
        use crate::pty::{PtySession, Spawn};
        use std::time::{Duration, Instant};
        let (_d, app, _fake) = app_with(sbx_answer);
        let name = SandboxRef::parse("ephemeral-chat").unwrap();
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
