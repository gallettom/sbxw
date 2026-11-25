//! The relay around its state machine: parking a sandbox on its request,
//! delivering a question to the routed-to agent, and an outcome to an asker
//! whose wait had already returned.
//!
//! * The book is kept in `state/relay.json` (written at each change, read
//!   leniently at start): a restart loses no request and reuses no id.
//! * An outcome is the asker's once its relay server acknowledged the tool
//!   result (`relay_ack`). One returned and not acknowledged within
//!   `ACK_LIMIT_MS`, or settled while nobody waits, is owed: handed to
//!   delivery (`tell_asker`, into the session's inbox), once.
//! * Every step goes out as a `relay-note` event for whoever tells the
//!   person, with a reminder after `REMIND_AFTER_MS` waiting on them.

use crate::app::{now_ms, App};
use crate::bus::Event;
use sbxw_core::messages::{self, Recipient};
use sbxw_core::relay::{self, RelayKind, RelayNote, RelayRequest, RelayStep, Shot};
use sbxw_core::inbox::Kind;
use sbxw_core::session::SessionKind;
use sbxw_core::{SandboxRef, ThreadName};
use sbxw_core::relay::RelayBook;
use std::path::{Path, PathBuf};
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
    /// The client acknowledges what it is handed (`relay_ack`): a returned
    /// outcome is only marked returned, not delivered.
    acks: bool,
}

impl Drop for WaitGuard<'_> {
    fn drop(&mut self) {
        let unread = {
            let mut parked = self.app.relay_waiters.lock().unwrap();
            let Some(p) = parked.get_mut(self.id) else { return };
            p.waits = p.waits.saturating_sub(1);
            let unread = if self.returned_final {
                // This wait hands the outcome to the asker: nothing is owed
                // to the others, and the book says how far it got.
                p.owed = None;
                mark_handed(&mut self.app.relay.lock().unwrap(), self.id, self.acks);
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
            self.app.tell_asker(&req);
        }
    }
}

/// A wait handed the final outcome over: to a client that acknowledges, it
/// is returned (owed if no acknowledgement follows); to one that cannot,
/// delivered.
fn mark_handed(book: &mut RelayBook, id: &str, acks: bool) {
    if acks {
        book.mark_returned(id, now_ms());
    } else {
        book.mark_delivered(id);
    }
}

/// Ceiling on one `ask`/`wait`: past a few minutes it is a hung tool call.
pub const MAX_WAIT_SECS: u64 = 600;

pub fn wait_bound(requested: Option<u64>) -> Duration {
    Duration::from_secs(requested.unwrap_or(90).clamp(1, MAX_WAIT_SECS))
}

/// The book as `state/relay.json` holds it; empty when there is none, or
/// when it cannot be read (said once, and the file is kept aside).
pub(crate) fn load(path: &Path) -> RelayBook {
    match std::fs::read_to_string(path) {
        Ok(t) => match RelayBook::parse_lenient(&t) {
            Some(b) => b,
            None => {
                let aside = path.with_extension("json.unreadable");
                let _ = std::fs::rename(path, &aside);
                tracing::warn!("relay: could not read {}; kept as {} and starting with no request", path.display(), aside.display());
                RelayBook::new()
            }
        },
        Err(_) => RelayBook::new(),
    }
}

/// How the asker's session is told about a long answer it is handed in
/// text: the first part, and how to read the rest.
fn first_part(req: &RelayRequest) -> RelayRequest {
    let mut out = req.clone();
    if let Some(a) = req.answer.as_deref() {
        let (first, parts) = relay::answer_part(a, 1);
        if parts > 1 {
            out.answer = Some(format!(
                "{first}\n\n[sbxw relay: this answer is in {parts} parts; this was part 1. Read the next with \
                 check_sandbox_question, request_id \"{id}\", part 2 (or node ~/.sbxw/relay.js wait {id} --part 2).]",
                id = req.id
            ));
        }
    }
    out
}

/// What the asker of a request pruned while still open is told.
fn expired_text(req: &RelayRequest) -> String {
    let gist: String = req.question.chars().take(80).collect();
    let gist = gist.replace(['\n', '\r'], " ");
    let hours = relay::OPEN_TTL_MS / 3_600_000;
    format!(
        "[sbxw relay · request {id}] Nobody acted on your request (\"{gist}\") within {hours} hours, so sbxw \
         closed it. Nothing was shared. Carry on without it, or ask the person directly if you still need it; \
         check_sandbox_question no longer knows this id.",
        id = req.id
    )
}

impl App {
    pub(crate) fn relay_file(&self) -> PathBuf {
        self.store.paths.state().join("relay.json")
    }

    /// Write the book if it changed. Serialized under the lock, written
    /// outside it.
    pub(crate) fn persist_relay(&self) {
        let text = {
            let mut book = self.relay.lock().unwrap();
            if !book.take_dirty() {
                return;
            }
            book.to_json()
        };
        let path = self.relay_file();
        if let Err(e) = crate::store::write_atomic(&path, text.as_bytes()) {
            tracing::warn!("relay: could not save {}: {e:#}", path.display());
        }
    }

    fn relay_note(&self, step: RelayStep, req: &RelayRequest) {
        self.send(Event::RelayNote(RelayNote::of(step, req, now_ms())));
    }

    fn relay_changed(&self, req: RelayRequest) -> RelayRequest {
        self.persist_relay();
        self.send(Event::Relay(req.clone()));
        self.relay_noted(&req);
        req
    }

    pub fn relay_list(&self) -> Vec<RelayRequest> {
        self.relay.lock().unwrap().list()
    }

    /// A sandbox opens a request (its identity is verified upstream).
    pub fn relay_open(&self, from: &SandboxRef, thread: Option<&ThreadName>, kind: RelayKind, question: &str) -> Result<RelayRequest, String> {
        self.relay_open_keyed(from, thread, kind, question, None)
    }

    /// `relay_open` with the asker's key for this one tool call: an open
    /// retried with the same key answers the request already open.
    pub fn relay_open_keyed(
        &self,
        from: &SandboxRef,
        thread: Option<&ThreadName>,
        kind: RelayKind,
        question: &str,
        client_key: Option<&str>,
    ) -> Result<RelayRequest, String> {
        let max = if kind == RelayKind::Ask { relay::MAX_ROUND } else { relay::MAX_QUESTION };
        let question = messages::clip_text(question, max);
        if question.is_empty() {
            return Err("empty question".into());
        }
        let (req, new) = self.relay.lock().unwrap().open_keyed(
            &from.to_string(),
            thread.map(|t| t.as_str()),
            kind,
            &question,
            client_key,
            now_ms(),
        );
        if !new {
            tracing::info!("relay: '{from}' opened {} again (same client key): the same request", req.id);
            return Ok(req);
        }
        tracing::info!("relay: '{from}' opened {} ({kind:?})", req.id);
        let req = self.relay_changed(req);
        self.relay_note(RelayStep::Opened, &req);
        Ok(req)
    }

    /// Park until the request settles or the bound passes, then return it as
    /// it stands. Waiting is reading, so only the asker may. For the
    /// daemon's own callers: the sandbox is checked, not the thread, and the
    /// caller is taken to have the outcome when it is returned.
    pub async fn relay_wait(self: &Arc<Self>, id: &str, from: &SandboxRef, bound: Duration) -> Result<RelayRequest, String> {
        self.relay_wait_as(id, from, None, false, bound).await
    }

    /// `relay_wait` for a client: `thread` (`Some(None)`: the sandbox's own
    /// agent) is checked against the session that asked; with `acks`, a
    /// final outcome returned is owed until `relay_ack`.
    ///
    /// Cancellation-safe: an outcome settled while this wait is parked is
    /// either returned by it, or, if the wait ends without returning it (the
    /// bound passed, the caller's future was dropped), handed to delivery
    /// once the last parked wait is gone. Never both, never neither.
    pub async fn relay_wait_as(
        self: &Arc<Self>,
        id: &str,
        from: &SandboxRef,
        thread: Option<Option<&ThreadName>>,
        acks: bool,
        bound: Duration,
    ) -> Result<RelayRequest, String> {
        let thread = thread.map(|t| t.map(|t| t.as_str()));
        // Subscribe before the first look, or a settlement in between is lost.
        let mut rx = self.bus.subscribe();
        // Looked at and registered under the waiters lock, which settling
        // holds too: an outcome is either seen here or owed to this wait.
        let current = {
            let mut parked = self.relay_waiters.lock().unwrap();
            let mut book = self.relay.lock().unwrap();
            let current = book.check_asker(id, &from.to_string(), thread)?.clone();
            if current.state.is_final() {
                mark_handed(&mut book, id, acks);
                return Ok(current);
            }
            parked.entry(id.to_string()).or_default().waits += 1;
            current
        };
        let mut guard = WaitGuard { app: self, id, returned_final: false, acks };
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
        if latest.state.is_final() {
            self.persist_relay();
        }
        Ok(latest)
    }

    /// The asker's relay server wrote the tool result holding the outcome:
    /// it is delivered. `false` when there was nothing left to deliver (an
    /// open request, or one acknowledged or handed on already).
    pub fn relay_ack(&self, id: &str, from: &SandboxRef, thread: Option<Option<&ThreadName>>) -> Result<bool, String> {
        let thread = thread.map(|t| t.map(|t| t.as_str()));
        let first = {
            let mut book = self.relay.lock().unwrap();
            book.check_asker(id, &from.to_string(), thread)?;
            book.mark_delivered(id)
        };
        if first {
            tracing::info!("relay: '{from}' has {id}");
            self.persist_relay();
        }
        Ok(first)
    }

    /// Settle request `id` with `settle`, then deliver the outcome once: to
    /// the parked waits if there are any (the last of them to go hands it
    /// on if none returned it), else to the asking session now.
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
        self.relay_note(RelayStep::Settled, &req);
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
        let req = self.relay_changed(req);
        self.relay_note(RelayStep::Answered, &req);
        Ok(())
    }

    /// A human sends the question to `to`; a notice goes to that agent's
    /// inbox (`App::deliver`). A delivery refused bounces the request back
    /// with the reason, unless it was routed elsewhere since.
    pub fn relay_route(self: &Arc<Self>, id: &str, to: &SandboxRef) -> Result<(), String> {
        let req = self.relay.lock().unwrap().route(id, &to.to_string(), now_ms())?;
        tracing::info!("relay: {id} routed to '{to}'");
        let req = self.relay_changed(req);
        self.relay_note(RelayStep::Routed, &req);
        // Into the target agent's inbox, never typed while it works; a
        // retried routing to the same target replaces the waiting notice.
        let key = format!("relay:{id}");
        if let Err(e) = self.deliver(to, &SessionKind::Agent, Kind::Request, &format!("relay request {id}"), &messages::relay_notice(&req), Some(&key)) {
            self.relay_route_failed(id, to, &e);
        }
        Ok(())
    }

    /// Delivery of request `id` to `target` failed: back to the person,
    /// unless it moved on since (re-routed, answered, settled).
    fn relay_route_failed(&self, id: &str, target: &SandboxRef, e: &str) {
        let why = format!("could not reach '{target}': {e} — try another sandbox");
        let res = self.relay.lock().unwrap().unroute(id, &target.to_string(), &why, now_ms());
        match res {
            Ok(r) => {
                tracing::warn!("relay: {id} back with the person: {why}");
                let r = self.relay_changed(r);
                self.relay_note(RelayStep::Unrouted, &r);
            }
            Err(left) => tracing::info!("relay: delivery of {id} to '{target}' failed ({e}), left as is: {left}"),
        }
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

    /// Once a minute (with `prune`): outcomes owed to an asker that never
    /// acknowledged them are handed to delivery, the person is reminded of
    /// requests that have waited on them, and the book is written if any of
    /// that, or anything else, changed it.
    pub(crate) fn relay_tick(self: &Arc<Self>) {
        let now = now_ms();
        let owed = {
            let parked = self.relay_waiters.lock().unwrap();
            self.relay.lock().unwrap().take_owed(now, |id| parked.get(id).is_some_and(|p| p.waits > 0))
        };
        let due = self.relay.lock().unwrap().take_reminders(now);
        self.persist_relay();
        for req in owed {
            tracing::info!("relay: {} was not acknowledged by '{}'; handing it to delivery", req.id, req.from);
            self.relay_note(RelayStep::Owed, &req);
            self.tell_asker(&req);
        }
        for req in due {
            tracing::info!("relay: {} has waited on the person for {} min; reminding", req.id, relay::REMIND_AFTER_MS / 60_000);
            // The request as now served carries `reminded_ms`: re-read.
            self.send(Event::Relay(req.clone()));
            self.relay_note(RelayStep::Reminder, &req);
        }
    }

    /// Forget requests nobody can act on any more; the asker of one that
    /// expired still open is told, and the person's views hear of it.
    /// Returns how many were dropped.
    pub(crate) fn relay_prune(self: &Arc<Self>, now: u64) -> usize {
        let gone = self.relay.lock().unwrap().prune_taken(now);
        if gone.is_empty() {
            return 0;
        }
        self.persist_relay();
        for req in gone.iter().filter(|r| !r.state.is_final()) {
            tracing::info!("relay: {} expired still open; telling '{}'", req.id, req.from);
            self.relay_note(RelayStep::Expired, req);
            self.tell_asker_text(req, expired_text(req));
        }
        gone.len()
    }

    /// Hand the outcome to the asking session, whose own call came back
    /// empty or moved on: into the inbox (`App::deliver`, never typed into a
    /// busy terminal) of the thread that asked while its session is open,
    /// else of the sandbox's agent. A long answer goes as its first part,
    /// saying how to read the rest.
    fn tell_asker(self: &Arc<Self>, req: &RelayRequest) {
        self.relay.lock().unwrap().mark_delivered(&req.id);
        self.persist_relay();
        let text = messages::relay_outcome(&first_part(req));
        self.tell_asker_text(req, text);
    }

    fn tell_asker_text(self: &Arc<Self>, req: &RelayRequest, text: String) {
        #[cfg(test)]
        self.relay_told.lock().unwrap().push(req.id.clone());
        let Ok(from) = SandboxRef::parse(&req.from) else {
            tracing::warn!("relay: {} names an asker '{}' that is not a sandbox; not delivered", req.id, req.from);
            return;
        };
        let sessions = self.sessions.clone();
        let mut ask = req.clone();
        let (to, _) = messages::relay_delivery(&ask, |t| {
            ThreadName::parse(t)
                .map(|t| sessions.is_live(&sbxw_core::session::SessionKey::thread(&from, &t)))
                .unwrap_or(false)
        });
        let text = match (&to, ask.from_thread.take()) {
            (Recipient::Agent, Some(t)) => format!(
                "[sbxw relay] This was asked by thread \"{t}\", whose session is closed, so it comes to you instead.\n\n{text}"
            ),
            _ => text,
        };
        let kind = match to {
            Recipient::Agent => SessionKind::Agent,
            Recipient::Thread(t) => match ThreadName::parse(&t) {
                Ok(t) => SessionKind::Thread(t),
                Err(_) => SessionKind::Agent,
            },
        };
        tracing::info!("relay: handing {} to '{from}' ({kind:?})", req.id);
        let key = format!("relay:{}", req.id);
        self.deliver_or_log(&from, &kind, Kind::Answer, &format!("relay request {}", req.id), &text, Some(&key));
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

    fn notes(rx: &mut tokio::sync::broadcast::Receiver<crate::bus::Event>) -> Vec<(sbxw_core::relay::RelayStep, String)> {
        let mut out = Vec::new();
        while let Ok(ev) = rx.try_recv() {
            if let crate::bus::Event::RelayNote(n) = ev {
                out.push((n.step, n.id));
            }
        }
        out
    }

    #[tokio::test]
    async fn after_a_restart_open_requests_are_open_their_asker_finds_them_and_no_id_is_reused() {
        let d = tempfile::tempdir().unwrap();
        let t1 = sbxw_core::ThreadName::parse("t1").unwrap();
        let (open, done) = {
            let app = crate::testing::app_in(d.path());
            let open = app.relay_open(&sb("alpha"), Some(&t1), RelayKind::Ask, "Q1?").unwrap();
            let done = app.relay_open(&sb("alpha"), None, RelayKind::Question, "q").unwrap();
            app.relay_approve(&done.id, Some("42"), &[]).unwrap();
            (open, done)
        };
        let app = crate::testing::app_in(d.path());
        assert_eq!(app.relay_list().len(), 2, "nothing lost");
        let back = app.relay_wait_as(&open.id, &sb("alpha"), Some(Some(&t1)), true, Duration::from_millis(20)).await.unwrap();
        assert_eq!((back.state, back.created_ms), (RelayState::Pending, open.created_ms), "the waiting thread finds its own request");
        assert!(app.relay_wait_as(&open.id, &sb("alpha"), Some(None), true, Duration::from_millis(20)).await.is_err(), "not the agent beside it");
        let next = app.relay_open(&sb("beta"), None, RelayKind::Question, "q").unwrap();
        assert!(next.id != open.id && next.id != done.id, "{} names another request", next.id);
        let settled = app.relay_wait(&done.id, &sb("alpha"), Duration::from_millis(20)).await.unwrap();
        assert_eq!(settled.answer.as_deref(), Some("42"));
    }

    #[test]
    fn a_retried_open_returns_the_same_request() {
        let (_d, app) = app();
        let mut rx = app.bus.subscribe();
        let a = app.relay_open_keyed(&sb("alpha"), None, RelayKind::Question, "q?", Some("k-1")).unwrap();
        let b = app.relay_open_keyed(&sb("alpha"), None, RelayKind::Question, "q?", Some("k-1")).unwrap();
        assert_eq!(a.id, b.id);
        assert_eq!(app.relay_list().len(), 1);
        assert_eq!(notes(&mut rx), vec![(sbxw_core::relay::RelayStep::Opened, a.id.clone())], "opened once");
    }

    #[tokio::test]
    async fn an_outcome_returned_but_never_acknowledged_is_owed_once() {
        let (_d, app) = app();
        let r = app.relay_open(&sb("alpha"), None, RelayKind::Question, "q?").unwrap();
        app.relay_approve(&r.id, Some("42"), &[]).unwrap();
        assert_eq!(told(&app, &r.id), 1, "nobody waited: handed on at once");

        let r = app.relay_open(&sb("alpha"), None, RelayKind::Question, "q?").unwrap();
        let a = app.clone();
        let id = r.id.clone();
        let waiter = tokio::spawn(async move { a.relay_wait_as(&id, &sb("alpha"), Some(None), true, Duration::from_secs(5)).await });
        tokio::time::sleep(Duration::from_millis(50)).await;
        app.relay_approve(&r.id, Some("42"), &[]).unwrap();
        assert_eq!(waiter.await.unwrap().unwrap().state, RelayState::Approved);
        app.relay_tick();
        assert_eq!(told(&app, &r.id), 0, "within the limit");
        // The tool call was cancelled after the answer left: no ack comes.
        app.relay.lock().unwrap().mark_returned(&r.id, 0);
        let mut rx = app.bus.subscribe();
        app.relay_tick();
        assert_eq!(told(&app, &r.id), 1, "owed, handed on");
        assert_eq!(notes(&mut rx), vec![(sbxw_core::relay::RelayStep::Owed, r.id.clone())]);
        app.relay_tick();
        assert_eq!(told(&app, &r.id), 1, "once");
        assert_eq!(app.relay_ack(&r.id, &sb("alpha"), Some(None)).unwrap(), false, "a late ack finds it handed on");
    }

    #[tokio::test]
    async fn an_acknowledged_outcome_is_not_handed_on() {
        let (_d, app) = app();
        let r = app.relay_open(&sb("alpha"), None, RelayKind::Question, "q?").unwrap();
        let a = app.clone();
        let id = r.id.clone();
        let waiter = tokio::spawn(async move { a.relay_wait_as(&id, &sb("alpha"), Some(None), true, Duration::from_secs(5)).await });
        tokio::time::sleep(Duration::from_millis(50)).await;
        app.relay_approve(&r.id, Some("42"), &[]).unwrap();
        waiter.await.unwrap().unwrap();
        assert!(app.relay_ack(&r.id, &sb("beta"), None).is_err(), "only the asker acknowledges");
        assert!(app.relay_ack(&r.id, &sb("alpha"), Some(None)).unwrap());
        app.relay.lock().unwrap().mark_returned(&r.id, 0);
        app.relay_tick();
        assert_eq!(told(&app, &r.id), 0);
    }

    #[tokio::test]
    async fn a_cancelled_wait_then_an_answer_is_owed_not_lost() {
        let (_d, app) = app();
        let r = app.relay_open(&sb("alpha"), None, RelayKind::Ask, "Q1?").unwrap();
        let a = app.clone();
        let id = r.id.clone();
        let waiter = tokio::spawn(async move { a.relay_wait_as(&id, &sb("alpha"), Some(None), true, Duration::from_secs(30)).await });
        tokio::time::sleep(Duration::from_millis(50)).await;
        waiter.abort();
        let _ = waiter.await;
        app.relay_approve(&r.id, Some("Q1 : yes"), &[]).unwrap();
        assert_eq!(told(&app, &r.id), 1);
        app.relay_tick();
        assert_eq!(told(&app, &r.id), 1, "handed on once, not owed again");
    }

    #[tokio::test]
    async fn a_long_answer_is_read_in_parts() {
        let (_d, app) = app();
        let long = "Q1 : ".to_string() + &"word ".repeat(6000);
        let r = app.relay_open(&sb("alpha"), None, RelayKind::Ask, "Q1?").unwrap();
        app.relay_approve(&r.id, Some(&long), &[]).unwrap();
        let got = app.relay_wait(&r.id, &sb("alpha"), Duration::from_millis(20)).await.unwrap();
        assert_eq!(got.answer.as_deref(), Some(long.trim()), "kept whole");
        let parts = got.answer_parts();
        assert!(parts >= 3, "{parts}");
        let whole: String = (1..=parts).map(|n| sbxw_core::relay::agent_view_part(&got, n)["answer"].as_str().unwrap().to_string()).collect();
        assert_eq!(whole, long.trim());
        let typed = super::first_part(&got).answer.unwrap();
        assert!(typed.contains("part 2") && typed.len() < long.len(), "the typed form says how to read the rest");
    }

    #[test]
    fn a_late_failure_for_an_earlier_target_leaves_the_new_routing() {
        let (_d, app) = app();
        let r = app.relay_open(&sb("alpha"), None, RelayKind::Question, "q?").unwrap();
        app.relay.lock().unwrap().route(&r.id, "beta", 1).unwrap();
        app.relay.lock().unwrap().route(&r.id, "gamma", 2).unwrap();
        app.relay_route_failed(&r.id, &sb("beta"), "not running");
        let now = app.relay.lock().unwrap().get(&r.id).cloned().unwrap();
        assert_eq!((now.state, now.to.as_deref()), (RelayState::Routed, Some("gamma")));
        app.relay_route_failed(&r.id, &sb("gamma"), "not running");
        assert_eq!(app.relay.lock().unwrap().get(&r.id).unwrap().state, RelayState::Pending);
    }

    #[test]
    fn the_person_is_reminded_once_of_a_request_waiting_on_them() {
        let (_d, app) = app();
        let old = app.relay.lock().unwrap().open("alpha", None, RelayKind::Question, "q", 0);
        let fresh = app.relay_open(&sb("alpha"), None, RelayKind::Question, "q").unwrap();
        let mut rx = app.bus.subscribe();
        app.relay_tick();
        assert_eq!(notes(&mut rx), vec![(sbxw_core::relay::RelayStep::Reminder, old.id.clone())], "not the fresh {}", fresh.id);
        assert!(app.relay_list().iter().find(|r| r.id == old.id).unwrap().reminded_ms.is_some(), "served with the request");
        app.relay_tick();
        assert!(notes(&mut rx).is_empty(), "once");
    }

    #[tokio::test]
    async fn an_open_request_pruned_tells_its_asker_and_the_person() {
        let (_d, app) = app();
        let r = app.relay.lock().unwrap().open("alpha", None, RelayKind::Question, "q", 0);
        let mut rx = app.bus.subscribe();
        assert_eq!(app.relay_prune(sbxw_core::relay::OPEN_TTL_MS), 1);
        assert_eq!(told(&app, &r.id), 1, "the asker is told");
        assert_eq!(notes(&mut rx), vec![(sbxw_core::relay::RelayStep::Expired, r.id.clone())]);
        assert!(super::expired_text(&r).contains("closed"));
    }

    #[test]
    fn an_outcome_and_a_routed_request_go_to_an_inbox_not_a_terminal() {
        let (_d, app) = app();
        let long = "Q1 : ".to_string() + &"word ".repeat(6000);
        let r = app.relay_open(&sb("alpha"), None, RelayKind::Ask, "Q1?").unwrap();
        app.relay_approve(&r.id, Some(&long), &[]).unwrap();
        let q = app.relay_open(&sb("alpha"), None, RelayKind::Question, "where?").unwrap();
        app.relay_route(&q.id, &sb("beta")).unwrap();
        app.relay_route(&q.id, &sb("beta")).unwrap();
        let inbox = app.inbox.lock().unwrap();
        let all = inbox.all();
        let answer: Vec<_> = all.iter().filter(|m| m.kind == sbxw_core::inbox::Kind::Answer).collect();
        assert_eq!(answer.len(), 1);
        assert_eq!((answer[0].sandbox.as_str(), answer[0].key.clone()), ("alpha", Some(format!("relay:{}", r.id))));
        assert!(answer[0].text.contains("part 2") && answer[0].text.len() < long.len(), "the first part, and how to read the rest");
        let request: Vec<_> = all.iter().filter(|m| m.kind == sbxw_core::inbox::Kind::Request).collect();
        assert_eq!(request.len(), 1, "routed twice to the same agent: one notice waits");
        assert_eq!(request[0].sandbox, "beta");
        assert!(request[0].text.contains(&q.id) && !request[0].text.contains("where?"), "the id, never the asker's words");
    }
}
