//! Hook events in, session state out; answering a prompt; raw input.

use crate::app::{now_ms, App};
use crate::bus::Event;
use crate::typing;
use sbxw_core::agents::{self, answer_keystrokes, HookEvent, OriginEvidence, SessionInfo};
use sbxw_core::session::{SessionKey, SessionKind};
use sbxw_core::SandboxName;
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
    pub fn ingest_hook(&self, sandbox: &SandboxName, mut body: serde_json::Value) {
        if let Some(obj) = body.as_object_mut() {
            obj.insert("received_ms".into(), now_ms().into());
            obj.insert("sandbox".into(), sandbox.as_str().into());
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
        });
        let has_pty = self.sessions.is_live(&SessionKey::agent(sandbox));
        let infos = self.agents.lock().unwrap().apply(sandbox, &ev, origin, has_pty, now_ms());
        for info in infos {
            self.send(Event::Session(info));
        }
    }

    pub fn hook_log(&self) -> Vec<serde_json::Value> {
        self.hook_log.lock().unwrap().iter().cloned().collect()
    }

    pub fn session_infos(&self) -> Vec<SessionInfo> {
        let sessions = self.sessions.clone();
        self.agents.lock().unwrap().snapshot(
            |sb| {
                SandboxName::parse(sb)
                    .map(|n| sessions.is_live(&SessionKey::agent(&n)))
                    .unwrap_or(false)
            },
            now_ms(),
        )
    }

    /// Answer a pending prompt by replaying the keystrokes a person would
    /// type. Refused (409) unless exactly one session of the sandbox is
    /// identifiably the one sbxw's terminal drives — an island too old to know
    /// would otherwise answer one session's question in another's terminal.
    pub async fn answer(self: &Arc<Self>, sandbox: &SandboxName, kind: SessionKind, indices: Vec<u32>) -> Result<(), AnswerError> {
        let Some(session) = self.sessions.live(&SessionKey::new(sandbox.clone(), kind)) else {
            return Err(AnswerError::NoSession);
        };
        if indices.is_empty() {
            return Err(AnswerError::NoIndices);
        }
        let (key, sizes) = {
            let book = self.agents.lock().unwrap();
            let key = book.answerable(sandbox).map_err(|e| AnswerError::Ambiguous(e.to_string()))?;
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
    pub async fn input(&self, sandbox: &SandboxName, kind: SessionKind, data: &[u8]) -> bool {
        match self.sessions.live(&SessionKey::new(sandbox.clone(), kind)) {
            Some(s) => {
                s.write(&[data]).await;
                true
            }
            None => false,
        }
    }
}
