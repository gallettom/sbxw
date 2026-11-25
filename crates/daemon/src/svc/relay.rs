//! The relay around its state machine: parking a sandbox on its request,
//! delivering a question to the routed-to agent, and an outcome to an asker
//! whose wait had already returned.

use crate::app::{now_ms, App};
use crate::bus::Event;
use sbxw_core::messages::{self, Recipient};
use sbxw_core::relay::{self, RelayKind, RelayRequest, Shot};
use sbxw_core::session::SessionKind;
use sbxw_core::{SandboxRef, ThreadName};
use sbxw_core::relay::RelayBook;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::broadcast::error::RecvError;

/// The parked `wait`s on one request, and an outcome settled while they
/// were parked that none of them has returned yet.
#[derive(Default)]
pub struct Parked {
    waits: usize,
    owed: Option<RelayRequest>,
}

/// One parked wait. Dropped however the wait ends, returned or cancelled:
/// the last one out types in an outcome no wait returned.
struct WaitGuard<'a> {
    app: &'a Arc<App>,
    id: &'a str,
    returned_final: bool,
}

impl Drop for WaitGuard<'_> {
    fn drop(&mut self) {
        let unread = {
            let mut parked = self.app.relay_waiters.lock().unwrap();
            let Some(p) = parked.get_mut(self.id) else { return };
            p.waits = p.waits.saturating_sub(1);
            let unread = if self.returned_final {
                // This wait hands the outcome to the asker: nothing is owed.
                p.owed = None;
                None
            } else if p.waits == 0 {
                p.owed.take()
            } else {
                None
            };
            if p.waits == 0 {
                parked.remove(self.id);
            }
            unread
        };
        if let Some(req) = unread {
            if tokio::runtime::Handle::try_current().is_ok() {
                self.app.tell_asker(&req);
            } else {
                tracing::warn!("relay: {} settled while its wait ended with the daemon; not typed in", self.id);
            }
        }
    }
}

/// Ceiling on one `ask`/`wait`: past a few minutes it is a hung tool call.
pub const MAX_WAIT_SECS: u64 = 600;

pub fn wait_bound(requested: Option<u64>) -> Duration {
    Duration::from_secs(requested.unwrap_or(90).clamp(1, MAX_WAIT_SECS))
}

impl App {
    fn relay_changed(&self, req: RelayRequest) -> RelayRequest {
        self.send(Event::Relay(req.clone()));
        self.relay_noted(&req);
        req
    }

    pub fn relay_list(&self) -> Vec<RelayRequest> {
        self.relay.lock().unwrap().list()
    }

    /// A sandbox opens a request (its identity is verified upstream).
    pub fn relay_open(&self, from: &SandboxRef, thread: Option<&ThreadName>, kind: RelayKind, question: &str) -> Result<RelayRequest, String> {
        let question = messages::clip_text(question, relay::MAX_QUESTION);
        if question.is_empty() {
            return Err("empty question".into());
        }
        let req = self
            .relay
            .lock()
            .unwrap()
            .open(&from.to_string(), thread.map(|t| t.as_str()), kind, &question, now_ms());
        tracing::info!("relay: '{from}' opened {} ({kind:?})", req.id);
        Ok(self.relay_changed(req))
    }

    /// Park until the request settles or the bound passes, then return it as
    /// it stands. Waiting is reading, so only the asker may.
    ///
    /// Cancellation-safe: an outcome settled while this wait is parked is
    /// either returned by it, or, if the wait ends without returning it (the
    /// bound passed, the caller's future was dropped), typed into the asker
    /// once the last parked wait is gone. Never both, never neither.
    pub async fn relay_wait(self: &Arc<Self>, id: &str, from: &SandboxRef, bound: Duration) -> Result<RelayRequest, String> {
        // Subscribe before the first look, or a settlement in between is lost.
        let mut rx = self.bus.subscribe();
        // Looked at and registered under the waiters lock, which settling
        // holds too: an outcome is either seen here or owed to this wait.
        let current = {
            let mut parked = self.relay_waiters.lock().unwrap();
            let current = self.relay.lock().unwrap().check_asker(id, &from.to_string(), None)?.clone();
            if current.state.is_final() {
                return Ok(current);
            }
            parked.entry(id.to_string()).or_default().waits += 1;
            current
        };
        let mut guard = WaitGuard { app: self, id, returned_final: false };
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
        // No await from here to the return: what the guard is told is what
        // the caller gets.
        guard.returned_final = latest.state.is_final();
        drop(guard);
        Ok(latest)
    }

    /// Settle request `id` with `settle`, then deliver the outcome once: to
    /// the parked waits if there are any (the last of them to go types it in
    /// if none returned it), else typed into the asking session now.
    fn relay_settle(self: &Arc<Self>, id: &str, settle: impl FnOnce(&mut RelayBook) -> Result<RelayRequest, String>) -> Result<RelayRequest, String> {
        let (req, unattended) = {
            let mut parked = self.relay_waiters.lock().unwrap();
            let req = settle(&mut self.relay.lock().unwrap())?;
            match parked.get_mut(id) {
                Some(p) if p.waits > 0 => {
                    p.owed = Some(req.clone());
                    (req, false)
                }
                _ => (req, true),
            }
        };
        let req = self.relay_changed(req);
        if unattended {
            self.tell_asker(&req);
        }
        Ok(req)
    }

    /// The routed-to sandbox files its answer; a human reviews it.
    pub fn relay_reply(&self, id: &str, from: &SandboxRef, answer: &str) -> Result<(), String> {
        let answer = messages::clip_text(answer, relay::MAX_ANSWER);
        if answer.is_empty() {
            return Err("empty answer".into());
        }
        let req = self.relay.lock().unwrap().reply(id, &from.to_string(), &answer, now_ms())?;
        tracing::info!("relay: '{from}' answered {id}");
        self.relay_changed(req);
        Ok(())
    }

    /// A human sends the question to `to`; it is typed into that agent. A
    /// delivery that fails bounces the request back with the reason.
    pub fn relay_route(self: &Arc<Self>, id: &str, to: &SandboxRef) -> Result<(), String> {
        let req = self.relay.lock().unwrap().route(id, &to.to_string(), now_ms())?;
        tracing::info!("relay: {id} routed to '{to}'");
        let req = self.relay_changed(req);
        let app = self.clone();
        let rid = id.to_string();
        let target = to.clone();
        self.spawn_push(
            to.clone(),
            SessionKind::Agent,
            messages::relay_notice(&req),
            Some(Box::new(move |e: String| {
                let why = format!("could not reach '{target}': {e} — try another sandbox");
                if let Ok(r) = app.relay.lock().unwrap().unroute(&rid, &target.to_string(), &why, now_ms()) {
                    app.send(Event::Relay(r));
                }
            })),
        );
        Ok(())
    }

    /// The routed-to sandbox reads what it was handed: the question framed as
    /// untrusted input, with how to answer. Only that sandbox, only while the
    /// request is with it.
    pub fn relay_show(&self, id: &str, me: &SandboxRef) -> Result<String, String> {
        let book = self.relay.lock().unwrap();
        let req = book.check_target(id, &me.to_string())?;
        Ok(messages::relay_request(req))
    }

    /// A human releases the answer (theirs, or the target's, maybe edited)
    /// and/or images. Settling is what makes every wait return; see
    /// `relay_settle` for who then gets the outcome.
    pub fn relay_approve(self: &Arc<Self>, id: &str, answer: Option<&str>, images: &[String]) -> Result<(), String> {
        // A virtual Mac request is settled by what the yes ran, not by text.
        if self.relay.lock().unwrap().get(id).is_some_and(|r| r.kind == RelayKind::Mac && r.state == sbxw_core::relay::RelayState::Pending) {
            return if self.mac_approved(id) { Ok(()) } else { Err(format!("request {id} is already being carried out")) };
        }
        let edited = answer.map(|a| messages::clip_text(a, relay::MAX_ANSWER)).filter(|a| !a.is_empty());
        let shots: Vec<Shot> = relay::parse_shots(images)?;
        let req = self.relay_settle(id, |book| book.approve(id, edited.as_deref(), shots, now_ms()))?;
        tracing::info!("relay: {id} approved → '{}'", req.from);
        Ok(())
    }

    pub fn relay_deny(self: &Arc<Self>, id: &str, note: Option<&str>) -> Result<(), String> {
        let note = note.map(|n| messages::clip_text(n, 500)).filter(|n| !n.is_empty());
        self.mac_denied(id);
        self.relay_settle(id, |book| book.deny(id, note.as_deref(), now_ms()))?;
        tracing::info!("relay: {id} denied");
        Ok(())
    }

    /// A Mac request the person approved, settled with what came of it.
    pub(crate) fn relay_settle_mac(self: &Arc<Self>, id: &str, outcome: &str) {
        match self.relay_settle(id, |book| book.approve(id, Some(outcome), Vec::new(), now_ms())) {
            Ok(req) => tracing::info!("relay: {id} (virtual Mac) carried out → '{}'", req.from),
            Err(e) => tracing::warn!("relay: settling {id}: {e}"),
        }
    }

    /// The asker's own call came back empty and moved on: typing the outcome
    /// in is the only way it is ever read — into the thread that asked, while
    /// its session is open.
    fn tell_asker(self: &Arc<Self>, req: &RelayRequest) {
        #[cfg(test)]
        self.relay_told.lock().unwrap().push(req.id.clone());
        let Ok(from) = SandboxRef::parse(&req.from) else { return };
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
    use sbxw_core::SandboxRef;
    use std::time::Duration;

    fn sb(n: &str) -> SandboxRef {
        SandboxRef::parse(n).unwrap()
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
    async fn a_routed_round_reaches_the_asker_only_once_the_person_releases_it() {
        let (_d, app) = app();
        let r = app.relay_open(&sb("alpha"), Some(&sbxw_core::ThreadName::parse("t1").unwrap()), RelayKind::Ask, "❓ **Q1 - Port** : which?").unwrap();
        // Routed in the book directly: delivery into a real sandbox is not under test.
        app.relay.lock().unwrap().route(&r.id, "beta", 1).unwrap();
        app.relay_reply(&r.id, &sb("beta"), "Q1 : 8080").unwrap();
        let held = app.relay_wait(&r.id, &sb("alpha"), Duration::from_millis(30)).await.unwrap();
        assert_eq!(held.state, RelayState::Answered, "the draft does not settle the round");
        assert_eq!(sbxw_core::relay::agent_view(&held)["answer"], serde_json::Value::Null);
        app.relay_approve(&r.id, Some("Q1 : 8081"), &[]).unwrap();
        let got = app.relay_wait(&r.id, &sb("alpha"), Duration::from_millis(30)).await.unwrap();
        assert_eq!(got.state, RelayState::Approved);
        assert_eq!(sbxw_core::relay::agent_view(&got)["answer"], "Q1 : 8081");
    }

    #[test]
    fn only_the_routed_sandbox_can_show_a_request() {
        let (_d, app) = app();
        let r = app.relay_open(&sb("alpha"), None, RelayKind::Ask, "❓ **Q1 - Port** : which?").unwrap();
        assert!(app.relay_show(&r.id, &sb("beta")).is_err(), "not routed yet");
        app.relay.lock().unwrap().route(&r.id, "beta", 1).unwrap();
        let text = app.relay_show(&r.id, &sb("beta")).unwrap();
        assert!(text.contains("untrusted input") && text.contains("which?"));
        assert!(app.relay_show(&r.id, &sb("gamma")).is_err());
        assert!(app.relay_show(&r.id, &sb("alpha")).is_err(), "nor the asker");
    }

    #[tokio::test]
    async fn a_timed_out_wait_reports_the_open_state() {
        let (_d, app) = app();
        let r = app.relay_open(&sb("alpha"), None, RelayKind::Screenshot, "show").unwrap();
        let got = app.relay_wait(&r.id, &sb("alpha"), Duration::from_millis(30)).await.unwrap();
        assert_eq!(got.state, RelayState::Pending);
        assert!(app.relay_waiters.lock().unwrap().get(&r.id).is_none(), "the waiter deregistered");
    }

    fn told(app: &crate::app::App, id: &str) -> usize {
        app.relay_told.lock().unwrap().iter().filter(|t| *t == id).count()
    }

    #[tokio::test]
    async fn an_answer_approved_after_the_wait_was_dropped_is_typed_in_once() {
        let (_d, app) = app();
        let r = app.relay_open(&sb("alpha"), None, RelayKind::Question, "q?").unwrap();
        let a = app.clone();
        let id = r.id.clone();
        let waiter = tokio::spawn(async move { a.relay_wait(&id, &sb("alpha"), Duration::from_secs(30)).await });
        tokio::time::sleep(Duration::from_millis(50)).await;
        // The asker's call is cancelled (its client gave up, the connection dropped).
        waiter.abort();
        assert!(waiter.await.unwrap_err().is_cancelled());
        app.relay_approve(&r.id, Some("42"), &[]).unwrap();
        assert_eq!(told(&app, &r.id), 1, "the approved answer is typed into the asker");
        assert!(app.relay_waiters.lock().unwrap().get(&r.id).is_none(), "nothing left parked");
    }

    #[tokio::test]
    async fn an_answer_a_live_wait_returns_is_not_typed_in_too() {
        let (_d, app) = app();
        let r = app.relay_open(&sb("alpha"), None, RelayKind::Question, "q?").unwrap();
        let a = app.clone();
        let id = r.id.clone();
        let waiter = tokio::spawn(async move { a.relay_wait(&id, &sb("alpha"), Duration::from_secs(5)).await });
        tokio::time::sleep(Duration::from_millis(50)).await;
        app.relay_approve(&r.id, Some("42"), &[]).unwrap();
        assert_eq!(waiter.await.unwrap().unwrap().state, RelayState::Approved);
        assert_eq!(told(&app, &r.id), 0, "returned to the waiter, so not typed as well");
    }

    #[tokio::test]
    async fn an_answer_nobody_waits_for_is_typed_in_once() {
        let (_d, app) = app();
        let r = app.relay_open(&sb("alpha"), None, RelayKind::Question, "q?").unwrap();
        app.relay_approve(&r.id, Some("42"), &[]).unwrap();
        assert_eq!(told(&app, &r.id), 1);
        // A later wait reads the settled request; it does not type it again.
        let got = app.relay_wait(&r.id, &sb("alpha"), Duration::from_millis(30)).await.unwrap();
        assert_eq!(got.state, RelayState::Approved);
        assert_eq!(told(&app, &r.id), 1);
    }
}
