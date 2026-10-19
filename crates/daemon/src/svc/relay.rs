//! The relay around its state machine: parking a sandbox on its request,
//! delivering a question to the routed-to agent, and an outcome to the asker
//! whose own wait had already come back.

use crate::app::{now_ms, App};
use crate::bus::Event;
use sbxw_core::messages::{self, Recipient};
use sbxw_core::relay::{self, RelayKind, RelayRequest, Shot};
use sbxw_core::session::SessionKind;
use sbxw_core::{SandboxName, ThreadName};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::broadcast::error::RecvError;

/// Ceiling on one `ask`/`wait`: past a few minutes it is a hung tool call.
pub const MAX_WAIT_SECS: u64 = 600;

pub fn wait_bound(requested: Option<u64>) -> Duration {
    Duration::from_secs(requested.unwrap_or(90).clamp(1, MAX_WAIT_SECS))
}

impl App {
    fn relay_changed(&self, req: RelayRequest) -> RelayRequest {
        self.send(Event::Relay(req.clone()));
        req
    }

    pub fn relay_list(&self) -> Vec<RelayRequest> {
        self.relay.lock().unwrap().list()
    }

    /// A sandbox opens a request (its identity is verified upstream).
    pub fn relay_open(&self, from: &SandboxName, thread: Option<&ThreadName>, kind: RelayKind, question: &str) -> Result<RelayRequest, String> {
        let question = messages::clip_text(question, relay::MAX_QUESTION);
        if question.is_empty() {
            return Err("empty question".into());
        }
        let req = self
            .relay
            .lock()
            .unwrap()
            .open(from, thread.map(|t| t.as_str()), kind, &question, now_ms());
        tracing::info!("relay: '{from}' opened {} ({kind:?})", req.id);
        Ok(self.relay_changed(req))
    }

    /// Park until the request settles or the bound passes, then return it as
    /// it stands. Waiting is reading, so only the asker may.
    pub async fn relay_wait(&self, id: &str, from: &SandboxName, bound: Duration) -> Result<RelayRequest, String> {
        // Subscribe before the first look, or a settlement in between is lost.
        let mut rx = self.bus.subscribe();
        let current = {
            let book = self.relay.lock().unwrap();
            book.check_asker(id, from)?.clone()
        };
        if current.state.is_final() {
            return Ok(current);
        }
        *self.relay_waiters.lock().unwrap().entry(id.to_string()).or_default() += 1;
        let mut latest = current;
        let deadline = tokio::time::Instant::now() + bound;
        loop {
            let left = deadline.saturating_duration_since(tokio::time::Instant::now());
            if left.is_zero() {
                break;
            }
            match tokio::time::timeout(left, rx.recv()).await {
                Ok(Ok(Event::Relay(r))) if r.id == id => {
                    let done = r.state.is_final();
                    latest = r;
                    if done {
                        break;
                    }
                }
                Ok(Ok(_)) => {}
                Ok(Err(RecvError::Lagged(_))) => {
                    if let Some(r) = self.relay.lock().unwrap().get(id).cloned() {
                        let done = r.state.is_final();
                        latest = r;
                        if done {
                            break;
                        }
                    }
                }
                Ok(Err(RecvError::Closed)) | Err(_) => break,
            }
        }
        if let Some(n) = self.relay_waiters.lock().unwrap().get_mut(id) {
            *n = n.saturating_sub(1);
        }
        Ok(latest)
    }

    fn relay_unattended(&self, id: &str) -> bool {
        self.relay_waiters.lock().unwrap().get(id).copied().unwrap_or(0) == 0
    }

    /// The routed-to sandbox files its answer; a human reviews it.
    pub fn relay_reply(&self, id: &str, from: &SandboxName, answer: &str) -> Result<(), String> {
        let answer = messages::clip_text(answer, relay::MAX_ANSWER);
        if answer.is_empty() {
            return Err("empty answer".into());
        }
        let req = self.relay.lock().unwrap().reply(id, from, &answer, now_ms())?;
        tracing::info!("relay: '{from}' answered {id}");
        self.relay_changed(req);
        Ok(())
    }

    /// A human sends the question to `to`; it is typed into that agent. A
    /// delivery that fails bounces the request back with the reason.
    pub fn relay_route(self: &Arc<Self>, id: &str, to: &SandboxName) -> Result<(), String> {
        let req = self.relay.lock().unwrap().route(id, to, now_ms())?;
        tracing::info!("relay: {id} routed to '{to}'");
        let req = self.relay_changed(req);
        let app = self.clone();
        let rid = id.to_string();
        let target = to.clone();
        self.spawn_push(
            to.clone(),
            SessionKind::Agent,
            messages::relay_request(&req),
            Some(Box::new(move |e: String| {
                let why = format!("could not reach '{target}': {e} — try another sandbox");
                if let Ok(r) = app.relay.lock().unwrap().unroute(&rid, &why, now_ms()) {
                    app.send(Event::Relay(r));
                }
            })),
        );
        Ok(())
    }

    /// A human releases the answer (theirs, or the target's, maybe edited)
    /// and/or images. Whether anyone is parked is read *before*: settling is
    /// what makes every wait return.
    pub fn relay_approve(self: &Arc<Self>, id: &str, answer: Option<&str>, images: &[String]) -> Result<(), String> {
        let edited = answer.map(|a| messages::clip_text(a, relay::MAX_ANSWER)).filter(|a| !a.is_empty());
        let shots: Vec<Shot> = relay::parse_shots(images)?;
        let unattended = self.relay_unattended(id);
        let req = self.relay.lock().unwrap().approve(id, edited.as_deref(), shots, now_ms())?;
        tracing::info!("relay: {id} approved → '{}'", req.from);
        let req = self.relay_changed(req);
        if unattended {
            self.tell_asker(&req);
        }
        Ok(())
    }

    pub fn relay_deny(self: &Arc<Self>, id: &str, note: Option<&str>) -> Result<(), String> {
        let note = note.map(|n| messages::clip_text(n, 500)).filter(|n| !n.is_empty());
        let unattended = self.relay_unattended(id);
        let req = self.relay.lock().unwrap().deny(id, note.as_deref(), now_ms())?;
        tracing::info!("relay: {id} denied");
        let req = self.relay_changed(req);
        if unattended {
            self.tell_asker(&req);
        }
        Ok(())
    }

    /// The asker's own call came back empty and moved on: typing the outcome
    /// in is the only way it is ever read — into the thread that asked, while
    /// its session is open.
    fn tell_asker(self: &Arc<Self>, req: &RelayRequest) {
        let Ok(from) = SandboxName::parse(&req.from) else { return };
        let sessions = self.sessions.clone();
        let (to, text) = messages::relay_delivery(req, |t| {
            ThreadName::parse(t)
                .map(|t| sessions.is_live(&sbxw_core::session::SessionKey::thread(&from, &t)))
                .unwrap_or(false)
        });
        let kind = match to {
            Recipient::Agent => SessionKind::Agent,
            Recipient::Thread(t) => match ThreadName::parse(&t) {
                Ok(t) => SessionKind::Thread(t),
                Err(_) => SessionKind::Agent,
            },
        };
        self.spawn_push(from, kind, text, None);
    }
}

#[cfg(test)]
mod tests {
    use crate::testing::app;
    use sbxw_core::relay::{RelayKind, RelayState};
    use sbxw_core::SandboxName;
    use std::time::Duration;

    fn sb(n: &str) -> SandboxName {
        SandboxName::parse(n).unwrap()
    }

    #[tokio::test]
    async fn an_approval_wakes_the_parked_asker_with_the_answer() {
        let (_d, app) = app();
        let r = app.relay_open(&sb("alpha"), None, RelayKind::Question, "q?").unwrap();
        let a = app.clone();
        let id = r.id.clone();
        let waiter = tokio::spawn(async move { a.relay_wait(&id, &sb("alpha"), Duration::from_secs(5)).await });
        tokio::time::sleep(Duration::from_millis(50)).await;
        app.relay_approve(&r.id, Some("42"), &[]).unwrap();
        let got = waiter.await.unwrap().unwrap();
        assert_eq!(got.state, RelayState::Approved);
        assert_eq!(got.answer.as_deref(), Some("42"));
    }

    #[tokio::test]
    async fn another_sandbox_cannot_wait_on_or_answer_a_request() {
        let (_d, app) = app();
        let r = app.relay_open(&sb("alpha"), None, RelayKind::Question, "q?").unwrap();
        assert!(app.relay_wait(&r.id, &sb("beta"), Duration::from_millis(10)).await.is_err());
        assert!(app.relay_reply(&r.id, &sb("beta"), "x").is_err(), "not routed to it");
    }

    #[tokio::test]
    async fn a_timed_out_wait_reports_the_open_state() {
        let (_d, app) = app();
        let r = app.relay_open(&sb("alpha"), None, RelayKind::Screenshot, "show").unwrap();
        let got = app.relay_wait(&r.id, &sb("alpha"), Duration::from_millis(30)).await.unwrap();
        assert_eq!(got.state, RelayState::Pending);
        assert!(app.relay_unattended(&r.id), "the waiter deregistered");
    }
}
