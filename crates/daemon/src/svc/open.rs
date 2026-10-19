//! Opening a terminal session: which command each kind runs, and the rules
//! around threads (their repo, their model, the session ceiling, a stopped
//! sandbox).

use crate::app::{now_ms, App};
use crate::git;
use crate::pty::{PtySession, Spawn};
use crate::typing;
use anyhow::{anyhow, bail, Result};
use futures_util::future::BoxFuture;
use sbxw_core::session::{SessionKey, SessionKind};
use sbxw_core::{ModelId, SandboxName, ThreadName};
use sbxw_sbx::scripts;
use std::sync::Arc;
use std::time::Duration;

/// Shown in a Bash pane on a stopped sandbox, before `ssh` (which starts it)
/// can fail on its one prerequisite. The name is `$1`, never script text.
const SSH_BANNER: &str = r#"printf '\033[2m[sbxw] %s is stopped - connecting over SSH to start it.\r\n[sbxw] If this fails, run: sbxw ssh --setup\033[0m\r\n' "$1"; exec ssh -t "$1.sbx""#;

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
        Ok(match &key.kind {
            SessionKind::Monitor => unreachable!("the monitor has no sandbox"),
            SessionKind::Agent if cfg.web_shell.is_empty() => {
                Spawn::sbx(sbxw_sbx::attach_args(&sandbox, &cfg.env_pairs(), &cfg.env_files))
            }
            SessionKind::Agent => Spawn::sbx(sbxw_sbx::shell_args(&sandbox, &cfg.web_shell)),
            SessionKind::Bash => {
                // `sbx exec` only reaches a running sandbox; SSH starts one.
                if self.sbx.is_running(&sandbox).await {
                    Spawn::sbx(sbxw_sbx::bash_args(&sandbox))
                } else {
                    tracing::info!("'{sandbox}' is stopped — opening the Bash pane over SSH to start it");
                    Spawn {
                        program: "sh".into(),
                        args: vec!["-c".into(), SSH_BANNER.into(), "sh".into(), sandbox.to_string()],
                        env: vec![],
                    }
                }
            }
            SessionKind::ThreadBash(thread) => {
                self.ensure_running_for_thread(&sandbox).await?;
                let dir = self.settle_thread_repo(&sandbox, thread, None).map_err(|e| anyhow!(e))?;
                Spawn::sbx(scripts::thread_bash_args(&sandbox, &dir, thread))
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
                Spawn::sbx(scripts::thread_run_args(&sandbox, &dir, thread, &briefing, model.as_ref()))
            }
        })
    }

    /// A thread runs through `sbx exec`, which only reaches a running sandbox.
    /// sbx has no plain "start": attaching the main session (`sbx run`) is
    /// what starts it — not `run -d`, which is `keep_running`'s call.
    pub async fn ensure_running_for_thread(self: &Arc<Self>, sandbox: &SandboxName) -> Result<()> {
        if self.sbx.is_running(sandbox).await {
            return Ok(());
        }
        tracing::info!("'{sandbox}' is stopped — starting it through its main session for a thread");
        self.open_session(&SessionKey::agent(sandbox)).await?;
        if self.sbx.wait_until_running(sandbox, Duration::from_secs(90)).await {
            Ok(())
        } else {
            bail!("'{sandbox}' did not start within 90 s; open its main pane and try the thread again")
        }
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
        let (session, fresh) = self.open_session(&key).await.map_err(|e| format!("{e:#}"))?;
        typing::type_and_submit(&session, text, fresh).await.map_err(|e| match &kind {
            SessionKind::Thread(t) => format!("thread '{t}' of '{sandbox}' is not running"),
            _ => format!("'{sandbox}' has no live agent session — {e}"),
        })
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
