//! Hook events in, session state out; answering a prompt; raw input.

use crate::app::{now_ms, App};
use crate::bus::Event;
use crate::typing;
use sbxw_core::agents::{self, answer_keystrokes, CostReport, HookEvent, ModelReport, OriginEvidence, SessionInfo, SessionOrigin};
use sbxw_core::session::{SessionKey, SessionKind};
use sbxw_core::{SandboxName, SandboxRef};
use std::sync::Arc;

const HOOK_LOG_MAX: usize = 100;

#[derive(Debug)]
pub enum AnswerError {
    NoSession,
    Ambiguous(String),
    NoIndices,
}

impl App {
    /// One hook event from `sandbox` (its identity, from its token).
    pub fn ingest_hook(&self, sandbox: &SandboxRef, mut body: serde_json::Value) {
        if let Some(obj) = body.as_object_mut() {
            obj.insert("received_ms".into(), now_ms().into());
            obj.insert("sandbox".into(), sandbox.to_string().into());
        }
        {
            let mut log = self.hook_log.lock().unwrap();
            if log.len() >= HOOK_LOG_MAX {
                log.pop_front();
            }
            log.push_back(body.clone());
        }
        let ev = HookEvent::new(&body);
        tracing::debug!("hook: sandbox={sandbox} event={} tool={}", ev.event(), ev.tool());
        if ev.event().is_empty() || ev.is_background() {
            return;
        }
        let hook_version = ev.hook_version();
        if hook_version < 2 && self.stale_hooks.lock().unwrap().insert(sandbox.to_string()) {
            tracing::warn!(
                "sandbox '{sandbox}' runs a status hook too old to report where its session came \
                 from; falling back to the workspace path. Run `sbxw up {sandbox}` to reinstall it."
            );
        }
        let workspace = self.store.workspace(sandbox);
        let origin = agents::classify_origin(&OriginEvidence {
            hook_version,
            ssh_env: &ev.strings("ssh_env"),
            ancestry: &ev.strings("ancestry"),
            cwd: ev.cwd(),
            workspace: workspace.as_ref().and_then(|p| p.to_str()),
            thread: ev.thread(),
        });
        // A lead's or thread's `SendMessage` to its team: on the team's timeline.
        self.direct_message_noted(sandbox, &body);
        let has_pty = self.sessions.is_live(&SessionKey::agent(sandbox));
        let infos = self.agents.lock().unwrap().apply(&sandbox.to_string(), &ev, origin, has_pty, now_ms());
        for info in infos {
            self.send(Event::Session(info));
        }
        self.history_event(sandbox, &body);
    }

    /// The model a session's status line reports. Broadcast only when it
    /// changes (the person switched with `/model`, or the first report).
    pub fn report_session_model(&self, sandbox: &SandboxRef, session_id: &str, model: ModelReport) {
        self.report_session(sandbox, session_id, Some(model), None);
    }

    /// What a session's status line reports, model and cost (either absent
    /// from an older script). A new model is broadcast at once; a change of
    /// cost alone at most every `COST_EVENT_MS` per session (see
    /// [`agents::AgentBook::report`]), the rest by [`Self::flush_session_costs`].
    pub fn report_session(&self, sandbox: &SandboxRef, session_id: &str, model: Option<ModelReport>, cost: Option<CostReport>) {
        let has_pty = self.sessions.is_live(&SessionKey::agent(sandbox));
        let infos = self.agents.lock().unwrap().report(&sandbox.to_string(), session_id, model, cost, has_pty, now_ms());
        for info in infos {
            self.send(Event::Session(info));
        }
    }

    /// Send the cost changes held back for 30 s: the last cent of a session
    /// that went quiet. On the background tick.
    pub fn flush_session_costs(&self) {
        let sessions = self.sessions.clone();
        let infos = self.agents.lock().unwrap().flush_costs(
            |sb| SandboxName::parse(sb).map(|n| sessions.is_live(&SessionKey::agent(&n))).unwrap_or(false),
            now_ms(),
        );
        for info in infos {
            self.send(Event::Session(info));
        }
    }

    /// What a sandbox's sessions have cost (see `AgentBook::sandbox_cost`).
    pub fn sandbox_cost(&self, sandbox: &str) -> Option<f64> {
        self.agents.lock().unwrap().sandbox_cost(sandbox)
    }

    /// What sbxw started a session with, for one that has not reported its
    /// model yet: the model its thread was opened on, else the sandbox's
    /// `claude_model`, else the daemon's, else the first of the models list.
    /// Only for Claude: sbxw starts no other agent on a model it chose.
    pub fn fill_model(&self, info: &mut SessionInfo) {
        if info.model.is_some() || info.agent != "claude" {
            return;
        }
        let Ok(sb) = SandboxRef::parse(&info.sandbox) else { return };
        let rec = self.store.get(sb);
        let thread = info.thread.as_ref().and_then(|t| rec.threads.get(t)).and_then(|r| r.model.clone());
        let (id, name) = self.configured_model(rec.config_snapshot.as_deref(), thread);
        info.model = id;
        if info.model_name.is_none() {
            info.model_name = name;
        }
    }

    /// The model a sandbox's main agent runs: what its session reported, else
    /// what sbxw starts it with. `(id, display name)`; both absent for an agent
    /// other than Claude that reported nothing.
    pub fn main_model(&self, sandbox: &str, agent: &str, infos: &[SessionInfo]) -> (Option<String>, Option<String>) {
        // The thread-less session that is not a client's: the tty's first.
        let main = infos
            .iter()
            .filter(|i| i.sandbox == sandbox && i.thread.is_none() && i.origin != SessionOrigin::Remote && i.model.is_some())
            .min_by_key(|i| (i.origin != SessionOrigin::Tty, i.started_ms));
        if let Some(i) = main {
            return (i.model.clone(), i.model_name.clone());
        }
        if agent != "claude" {
            return (None, None);
        }
        let snapshot = SandboxRef::parse(sandbox).ok().and_then(|sb| self.store.get(sb).config_snapshot);
        self.configured_model(snapshot.as_deref(), None)
    }

    /// `preferred`, else the configured default; named from the models lists.
    fn configured_model(&self, snapshot: Option<&sbxw_core::config::Config>, preferred: Option<String>) -> (Option<String>, Option<String>) {
        let nonempty = |s: &str| Some(s.trim().to_string()).filter(|s| !s.is_empty());
        let id = preferred
            .and_then(|m| nonempty(&m))
            .or_else(|| snapshot.and_then(|c| nonempty(&c.claude_model)))
            .or_else(|| nonempty(&self.cfg.claude_model))
            .or_else(|| self.cfg.models.first().map(|m| m.id.clone()));
        let name = id.as_ref().and_then(|id| {
            snapshot
                .map(|c| c.models.as_slice())
                .unwrap_or_default()
                .iter()
                .chain(self.cfg.models.iter())
                .find(|m| &m.id == id)
                .map(|m| m.label.clone())
        });
        (id, name)
    }

    pub fn hook_log(&self) -> Vec<serde_json::Value> {
        self.hook_log.lock().unwrap().iter().cloned().collect()
    }

    pub fn session_infos(&self) -> Vec<SessionInfo> {
        let sessions = self.sessions.clone();
        let mut infos = self.agents.lock().unwrap().snapshot(
            |sb| {
                SandboxName::parse(sb)
                    .map(|n| sessions.is_live(&SessionKey::agent(&n)))
                    .unwrap_or(false)
            },
            now_ms(),
        );
        for info in &mut infos {
            self.fill_model(info);
        }
        infos
    }

    /// Answer a pending prompt by replaying the keystrokes a person would
    /// type. Refused (409) unless exactly one session of the sandbox is
    /// identifiably the one sbxw's terminal drives — an island too old to know
    /// would otherwise answer one session's question in another's terminal.
    pub async fn answer(self: &Arc<Self>, sandbox: &SandboxRef, kind: SessionKind, indices: Vec<u32>) -> Result<(), AnswerError> {
        let Some(session) = self.sessions.live(&SessionKey::new(sandbox.clone(), kind)) else {
            return Err(AnswerError::NoSession);
        };
        if indices.is_empty() {
            return Err(AnswerError::NoIndices);
        }
        let (key, sizes) = {
            let book = self.agents.lock().unwrap();
            let key = book.answerable(&sandbox.to_string()).map_err(|e| AnswerError::Ambiguous(e.to_string()))?;
            let sizes = book.prompt_sizes(&key);
            (key, sizes)
        };
        tracing::info!("answer '{sandbox}': indices={indices:?}, sizes={sizes:?}");
        typing::keys(&session, &answer_keystrokes(&indices, &sizes)).await;
        let infos = self.agents.lock().unwrap().answered(&key, true, now_ms());
        for i in infos {
            self.send(Event::Session(i));
        }
        Ok(())
    }

    /// Raw bytes into a terminal the caller names: no inference, so no guard.
    pub async fn input(&self, sandbox: &SandboxRef, kind: SessionKind, data: &[u8]) -> bool {
        match self.sessions.live(&SessionKey::new(sandbox.clone(), kind)) {
            Some(s) => {
                s.write(&[data]).await;
                true
            }
            None => false,
        }
    }
}
