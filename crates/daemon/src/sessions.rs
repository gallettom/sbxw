//! The terminal sessions sbxw holds, by typed key.
//!
//! Two rules the old global statics enforced, kept here as structure:
//!
//! * **One creator per key.** An approved plan's launch and the pane the
//!   browser opens for the same thread can both miss a lookup at once; without
//!   a per-key lock each would start a `claude` in one worktree.
//! * **A dead entry is no session.** An entry whose process exited is dropped
//!   on the way through, so a recycled name never types into a corpse.

use crate::pty::{PtySession, Spawn};
use sbxw_core::session::{SessionKey, SessionKind};
use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, Mutex, Weak};

#[derive(Default)]
pub struct Sessions {
    map: Mutex<HashMap<SessionKey, Arc<PtySession>>>,
    creating: Mutex<HashMap<SessionKey, Arc<tokio::sync::Mutex<()>>>>,
}

impl Sessions {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// The live session under `key`.
    pub fn live(&self, key: &SessionKey) -> Option<Arc<PtySession>> {
        let mut map = self.map.lock().unwrap();
        match map.get(key) {
            Some(s) if s.alive() => Some(s.clone()),
            Some(_) => {
                map.remove(key);
                None
            }
            None => None,
        }
    }

    pub fn is_live(&self, key: &SessionKey) -> bool {
        self.live(key).is_some()
    }

    /// Agents (main + threads) of `sandbox` alive right now — what
    /// `max_agent_sessions` counts.
    pub fn agents_open(&self, sandbox: &str) -> usize {
        self.map
            .lock()
            .unwrap()
            .iter()
            .filter(|(k, s)| k.belongs_to(sandbox) && k.kind.is_agent() && s.alive())
            .count()
    }

    /// Every live key of `sandbox`.
    pub fn keys_of(&self, sandbox: &str) -> Vec<SessionKey> {
        self.map
            .lock()
            .unwrap()
            .iter()
            .filter(|(k, s)| k.belongs_to(sandbox) && s.alive())
            .map(|(k, _)| k.clone())
            .collect()
    }

    pub fn all(&self) -> Vec<(SessionKey, Arc<PtySession>)> {
        self.map.lock().unwrap().iter().map(|(k, s)| (k.clone(), s.clone())).collect()
    }

    pub fn remove(&self, key: &SessionKey) -> Option<Arc<PtySession>> {
        self.map.lock().unwrap().remove(key)
    }

    /// Forget and stop every session of `sandbox` (it was removed).
    pub fn drop_sandbox(&self, sandbox: &str) {
        let gone: Vec<Arc<PtySession>> = {
            let mut map = self.map.lock().unwrap();
            let keys: Vec<SessionKey> = map.keys().filter(|k| k.belongs_to(sandbox)).cloned().collect();
            keys.iter().filter_map(|k| map.remove(k)).collect()
        };
        for s in gone {
            s.kill();
        }
    }

    /// The live session under `key`, or one made by `make` — at most one
    /// `make` per key at a time; a second caller waits and takes the first's.
    /// `make` decides the command (and may refuse, with the reason shown in
    /// the pane). Returns whether this call created it.
    pub async fn get_or_open<F, Fut>(self: &Arc<Self>, key: &SessionKey, now_ms: u64, make: F) -> anyhow::Result<(Arc<PtySession>, bool)>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = anyhow::Result<Spawn>>,
    {
        if let Some(s) = self.live(key) {
            return Ok((s, false));
        }
        let gate = self.creating.lock().unwrap().entry(key.clone()).or_default().clone();
        let _held = gate.lock().await;
        if let Some(s) = self.live(key) {
            return Ok((s, false));
        }
        let spawn = make().await?;
        let weak: Weak<Self> = Arc::downgrade(self);
        let k = key.clone();
        let session = PtySession::spawn(spawn, now_ms, move || {
            if let Some(me) = weak.upgrade() {
                let mut map = me.map.lock().unwrap();
                // Only if it is still this session filed there (not a newer one).
                if map.get(&k).is_some_and(|s| !s.alive()) {
                    map.remove(&k);
                }
            }
            tracing::info!("PTY session '{k}' ended");
        })?;
        self.map.lock().unwrap().insert(key.clone(), session.clone());
        Ok((session, true))
    }

    /// Thread names with a live session in `sandbox`.
    pub fn open_threads(&self, sandbox: &str) -> Vec<String> {
        self.keys_of(sandbox)
            .into_iter()
            .filter_map(|k| match k.kind {
                SessionKind::Thread(t) => Some(t.to_string()),
                _ => None,
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sbxw_core::{SandboxName, ThreadName};

    fn sh(script: &str) -> Spawn {
        Spawn {
            program: "sh".into(),
            args: vec!["-c".into(), script.into()],
            env: vec![],
        }
    }

    #[tokio::test]
    async fn one_creator_per_key_and_agents_are_counted() {
        let s = Sessions::new();
        let sb = SandboxName::parse("n").unwrap();
        let key = SessionKey::agent(&sb);
        let made = std::sync::atomic::AtomicUsize::new(0);
        let open = || async {
            made.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            Ok(sh("sleep 5"))
        };
        let (a, b) = tokio::join!(s.get_or_open(&key, 0, open), s.get_or_open(&key, 0, open));
        assert!(Arc::ptr_eq(&a.unwrap().0, &b.unwrap().0));
        assert_eq!(made.load(std::sync::atomic::Ordering::SeqCst), 1);
        let t = SessionKey::thread(&sb, &ThreadName::parse("t").unwrap());
        s.get_or_open(&t, 0, || async { Ok(sh("sleep 5")) }).await.unwrap();
        let bash = SessionKey::new(sb.clone(), SessionKind::Bash);
        s.get_or_open(&bash, 0, || async { Ok(sh("sleep 5")) }).await.unwrap();
        assert_eq!(s.agents_open("n"), 2, "bash does not count");
        assert_eq!(s.open_threads("n"), ["t"]);
        s.drop_sandbox("n");
        assert!(s.keys_of("n").is_empty());
    }

    #[tokio::test]
    async fn a_refusal_opens_nothing_and_a_dead_session_is_dropped() {
        let s = Sessions::new();
        let key = SessionKey::agent(&SandboxName::parse("n").unwrap());
        assert!(s.get_or_open(&key, 0, || async { anyhow::bail!("no") }).await.is_err());
        let (first, _) = s.get_or_open(&key, 0, || async { Ok(sh("exit 0")) }).await.unwrap();
        for _ in 0..50 {
            if !first.alive() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        assert!(s.live(&key).is_none());
        let (_, created) = s.get_or_open(&key, 0, || async { Ok(sh("sleep 5")) }).await.unwrap();
        assert!(created, "a fresh one replaces the corpse");
        s.drop_sandbox("n");
    }
}
