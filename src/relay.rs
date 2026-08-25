//! Requests an agent cannot settle from inside its own sandbox, with a human on
//! every hop.
//!
//! Two things are asked for here, and they differ only in who can possibly
//! answer. A **question** (`RelayKind::Question`) is about a workspace this
//! agent cannot see: a person in the web UI decides which *other* sandbox — if
//! any — is asked, that sandbox's agent answers, and the same person decides
//! whether the answer is released. A **screenshot** (`RelayKind::Screenshot`) is
//! about something only the person at the keyboard can see — what the change
//! actually looks like on screen — so it is never handed to another sandbox at
//! all; it is answered, or refused, by the human it interrupted.
//!
//! Both travel the same queue, the same popup and the same approval, because
//! the interesting part is identical: an agent is blocked on a person, and
//! nothing crosses until that person says so.
//!
//! The two rules that shape everything here:
//!
//!  - **No transition happens on its own.** There is no timeout that routes a
//!    question, and no state where an answer flows onwards without an explicit
//!    approval. A request nobody attends to simply stays open until it is
//!    pruned, and the asking agent is told exactly that.
//!  - **A sandbox may only touch its own side of a request.** The asker can
//!    wait on requests it opened; the routed-to sandbox can answer the one it
//!    was handed. Neither can enumerate, read, or answer anything else — which
//!    is what keeps a compromised or merely over-eager agent from turning this
//!    into a general-purpose bus between sandboxes.
//!
//! This module is the state machine and nothing else: no HTTP, no PTY. `web.rs`
//! owns the endpoints, the SSE fan-out, and the typing of messages into the
//! target session.
//!
//! A note on what a request *carries*. An answer is text and a screenshot is an
//! image, but neither is read here — a question is data for another agent, an
//! answer is data for the asking one, and an image is base64 the daemon never
//! decodes. Everything in this file moves payloads between states; nothing in
//! it looks inside one.

use serde::Serialize;
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicU64, Ordering},
        Mutex,
    },
    time::Duration,
};
use tokio::sync::broadcast;

/// Where a request stands. `Approved` and `Denied` are terminal: nothing moves
/// a request out of them, and they are the only two states in which the asking
/// agent stops being told to come back later.
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Debug)]
#[serde(rename_all = "lowercase")]
pub(crate) enum RelayState {
    /// Open, and nobody has been asked yet.
    Pending,
    /// A human sent it to `to`, which has not answered.
    Routed,
    /// `to` answered; the answer is held for review and has *not* been released.
    Answered,
    /// A human released an answer to the asker.
    Approved,
    /// A human refused; nothing is released, now or later.
    Denied,
}

impl RelayState {
    /// Whether the request has settled for good — what `wait` returns on, and
    /// what the browser UI stops offering buttons for.
    pub(crate) fn is_final(self) -> bool {
        matches!(self, RelayState::Approved | RelayState::Denied)
    }
}

/// What is being asked for, and therefore who could possibly supply it.
///
/// This is not a label on an otherwise identical request: it decides whether
/// `Routed` and `Answered` are reachable at all. Nobody but the person at the
/// keyboard can see the screen, so a `Screenshot` that got routed to another
/// sandbox would be a question no agent there is able to answer — which is why
/// the transitions below refuse it outright rather than leaving the UI to know
/// better.
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Debug, Default)]
#[serde(rename_all = "lowercase")]
pub(crate) enum RelayKind {
    /// Information from another sandbox's workspace.
    #[default]
    Question,
    /// An image of what is on the human's screen.
    Screenshot,
}

/// An image a human attached to a screenshot request.
///
/// Held as the base64 the browser sent rather than as decoded bytes. The daemon
/// is a courier for this payload exactly as it is for a question's text: every
/// byte it does not interpret is an image decoder it does not run on data a
/// browser handed it, and the sandbox that asked has to decode the picture
/// anyway. What is checked at the door is the envelope — see
/// `web::parse_shot`.
#[derive(Clone, Debug)]
pub(crate) struct Shot {
    /// `image/png`, `image/jpeg` or `image/webp`.
    pub(crate) mime: String,
    /// The image itself, base64, without the `data:` prefix.
    pub(crate) b64: String,
}

/// How an attached image is serialized: as *whether there is one*.
///
/// The browser and the island are the only readers of a whole `RelayRequest`,
/// and neither needs the pixels — the popup already holds the copy it just
/// pasted, and the notch has nothing to do with it. Sending presence instead
/// also keeps a megabyte of base64 out of every SSE frame this request will
/// ever produce, on a channel that repaints on each transition.
fn shot_presence<S: serde::Serializer>(shot: &Option<Shot>, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_bool(shot.is_some())
}

/// One question and everything that has happened to it.
#[derive(Clone, Serialize, Debug)]
pub(crate) struct RelayRequest {
    pub(crate) id: String,
    /// Sandbox that asked.
    pub(crate) from: String,
    /// What is being asked for. Fixed at `open` — a request never changes what
    /// it is, only where it stands.
    pub(crate) kind: RelayKind,
    /// The question, as the asking agent wrote it — or, for a screenshot, why
    /// it wants one and what it hopes to see. Untrusted text either way: it is
    /// shown to a person and typed into another agent's session, never
    /// interpreted here.
    pub(crate) question: String,
    /// Sandbox a human routed it to, once one has been chosen.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) to: Option<String>,
    /// The answer — held while `Answered`, released only by `Approved`. On a
    /// screenshot request this is the human's caption, if they wrote one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) answer: Option<String>,
    /// The image a human attached, under exactly the same rule as `answer`:
    /// present on the server from the moment it is pasted, the asker's only
    /// once approval says so.
    #[serde(rename = "has_shot", serialize_with = "shot_presence")]
    pub(crate) shot: Option<Shot>,
    pub(crate) state: RelayState,
    /// Why a request was denied, or what went wrong delivering it. Shown to
    /// both humans and agents, so it says what to do rather than what failed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) note: Option<String>,
    pub(crate) created_ms: u64,
    pub(crate) updated_ms: u64,
    /// How many `wait` calls are parked on this request right now.
    ///
    /// Not state so much as a delivery hint: on approval, an answer with a
    /// waiter is picked up by that call's return, while one with none has to be
    /// typed into the asking session or it is never read at all — the agent's
    /// bounded call already came back empty and it moved on.
    #[serde(skip)]
    waiters: usize,
}

/// Longest a settled request is kept around, so a `wait` that comes back late
/// (or a person re-reading the popup) still finds the answer.
const SETTLED_TTL: Duration = Duration::from_secs(30 * 60);

/// Longest an *open* request is kept. Generous: the whole point is that it
/// waits for a human, who may well be at lunch. Past this it is dropped, and
/// the asking agent is told the request is unknown rather than left waiting.
const OPEN_TTL: Duration = Duration::from_secs(6 * 60 * 60);

/// Cap on a question's length. Long enough for a paragraph of context, short
/// enough that no single request can fill the popup — or the target session's
/// prompt — with an agent's entire scrollback.
pub(crate) const MAX_QUESTION: usize = 4000;

/// Cap on an answer's length, same reasoning from the other direction.
pub(crate) const MAX_ANSWER: usize = 16000;

/// Cap on the base64 of an attached screenshot — a little over 4 MB of actual
/// image.
///
/// A backstop, not a budget: the browser downscales what it uploads (see
/// `assets/js/relay.js`), so a screenshot that arrives anywhere near this is one
/// that skipped that path. It has to be generous enough for a full retina
/// window and small enough that the JSON it rides in stays a message rather
/// than a transfer.
pub(crate) const MAX_SHOT_B64: usize = 6 * 1024 * 1024;

/// Image types a screenshot may be. Short on purpose: an agent has to decode
/// whatever comes out the other end, and this is the list every one of them
/// reads without being told.
pub(crate) const SHOT_MIMES: &[&str] = &["image/png", "image/jpeg", "image/webp"];

/// Every live request, plus the bus the UI watches.
pub(crate) struct Relay {
    requests: Mutex<HashMap<String, RelayRequest>>,
    updates: broadcast::Sender<RelayRequest>,
    seq: AtomicU64,
}

impl Relay {
    pub(crate) fn new() -> Self {
        let (updates, _) = broadcast::channel(256);
        Self {
            requests: Mutex::new(HashMap::new()),
            updates,
            seq: AtomicU64::new(1),
        }
    }

    pub(crate) fn subscribe(&self) -> broadcast::Receiver<RelayRequest> {
        self.updates.subscribe()
    }

    /// Announce a change. Errors only when nobody is listening (no browser tab
    /// open), which is not a problem worth reporting: the state is in the map,
    /// and a tab that connects later seeds itself from `list`.
    fn announce(&self, req: &RelayRequest) {
        let _ = self.updates.send(req.clone());
    }

    /// Open a request. Always succeeds — deciding whether a question deserves
    /// asking, or an interruption deserves a screenshot, is the human's job and
    /// not this function's.
    pub(crate) fn open(
        &self,
        from: &str,
        kind: RelayKind,
        question: &str,
        now: u64,
    ) -> RelayRequest {
        let id = format!("r-{}", self.seq.fetch_add(1, Ordering::Relaxed));
        let req = RelayRequest {
            id: id.clone(),
            from: from.to_string(),
            kind,
            question: question.to_string(),
            to: None,
            answer: None,
            shot: None,
            state: RelayState::Pending,
            note: None,
            created_ms: now,
            updated_ms: now,
            waiters: 0,
        };
        self.requests.lock().unwrap().insert(id, req.clone());
        self.announce(&req);
        req
    }

    pub(crate) fn get(&self, id: &str) -> Option<RelayRequest> {
        self.requests.lock().unwrap().get(id).cloned()
    }

    /// Every request, oldest first — what a browser tab seeds itself from.
    pub(crate) fn list(&self) -> Vec<RelayRequest> {
        let mut out: Vec<RelayRequest> = self.requests.lock().unwrap().values().cloned().collect();
        out.sort_by(|a, b| a.created_ms.cmp(&b.created_ms).then(a.id.cmp(&b.id)));
        out
    }

    /// Apply `f` to a live request, then announce whatever it made of it.
    ///
    /// Every transition below goes through here so that no path can change a
    /// request without the UI hearing about it, and so the lock is never held
    /// across the broadcast.
    fn mutate<F>(&self, id: &str, now: u64, f: F) -> Result<RelayRequest, String>
    where
        F: FnOnce(&mut RelayRequest) -> Result<(), String>,
    {
        let updated = {
            let mut map = self.requests.lock().unwrap();
            let req = map
                .get_mut(id)
                .ok_or_else(|| format!("no request '{id}' — it may have been answered long ago"))?;
            f(req)?;
            req.updated_ms = now;
            req.clone()
        };
        self.announce(&updated);
        Ok(updated)
    }

    /// A human sends the question to `to`. Also the way a request is *re*-routed
    /// after a target went quiet: the earlier answer, if any, is dropped rather
    /// than carried over, since it answered on behalf of a different sandbox.
    pub(crate) fn route(&self, id: &str, to: &str, now: u64) -> Result<RelayRequest, String> {
        self.mutate(id, now, |req| {
            if req.state.is_final() {
                return Err(format!("request {id} is already settled"));
            }
            if req.kind == RelayKind::Screenshot {
                return Err(format!(
                    "request {id} asks for a screenshot — no other sandbox can see this screen"
                ));
            }
            if req.from == to {
                return Err(format!("'{to}' is the sandbox that asked"));
            }
            req.to = Some(to.to_string());
            req.answer = None;
            req.note = None;
            req.state = RelayState::Routed;
            Ok(())
        })
    }

    /// Delivery into the target session failed — put the request back where it
    /// was so the human can pick someone else, and say why.
    pub(crate) fn unroute(&self, id: &str, why: &str, now: u64) -> Result<RelayRequest, String> {
        self.mutate(id, now, |req| {
            if req.state != RelayState::Routed {
                return Err(format!("request {id} is no longer being routed"));
            }
            req.to = None;
            req.state = RelayState::Pending;
            req.note = Some(why.to_string());
            Ok(())
        })
    }

    /// The routed-to sandbox answers. `from` is checked against the routing: a
    /// sandbox can only answer the question it was actually handed.
    ///
    /// No `RelayKind` check is needed, and adding one would be a second rule
    /// saying the same thing: a screenshot request cannot be routed, so it has
    /// no `to` for any sandbox's name to match.
    pub(crate) fn reply(
        &self,
        id: &str,
        from: &str,
        answer: &str,
        now: u64,
    ) -> Result<RelayRequest, String> {
        self.mutate(id, now, |req| {
            if req.state.is_final() {
                return Err(format!("request {id} is already settled"));
            }
            match req.to.as_deref() {
                Some(target) if target == from => {}
                _ => return Err(format!("request {id} was not sent to '{from}'")),
            }
            req.answer = Some(answer.to_string());
            req.state = RelayState::Answered;
            Ok(())
        })
    }

    /// A human releases what the asker gets — the answer under review, their
    /// own text, an image they attached, or a caption alongside it. This is also
    /// how a question gets answered without involving a second sandbox at all,
    /// and it is the *only* way a screenshot request ever settles in the asker's
    /// favour.
    ///
    /// One rule covers both kinds: something has to be going out. Which of the
    /// two payloads it is stays deliberately untyped here — a screenshot with a
    /// caption and a question answered with a picture are both perfectly sensible
    /// things for a person to send, and refusing them would be this function
    /// second-guessing the human it exists to serve.
    pub(crate) fn approve(
        &self,
        id: &str,
        answer: Option<&str>,
        shot: Option<Shot>,
        now: u64,
    ) -> Result<RelayRequest, String> {
        self.mutate(id, now, |req| {
            if req.state.is_final() {
                return Err(format!("request {id} is already settled"));
            }
            if let Some(text) = answer {
                req.answer = Some(text.to_string());
            }
            if let Some(image) = shot {
                req.shot = Some(image);
            }
            if req.answer.is_none() && req.shot.is_none() {
                return Err("there is nothing to send yet".to_string());
            }
            req.state = RelayState::Approved;
            Ok(())
        })
    }

    /// A human refuses. Terminal on purpose: "not this time" has to be a full
    /// stop, or an agent will simply ask again.
    pub(crate) fn deny(
        &self,
        id: &str,
        note: Option<&str>,
        now: u64,
    ) -> Result<RelayRequest, String> {
        self.mutate(id, now, |req| {
            if req.state.is_final() {
                return Err(format!("request {id} is already settled"));
            }
            // The held answer never reaches the asker, so it is dropped here
            // rather than kept where an approval could later release it. The
            // image is cleared alongside it so that "denied" is a statement
            // about the whole request rather than about the one field that
            // happens to be fillable before approval today.
            req.answer = None;
            req.shot = None;
            req.state = RelayState::Denied;
            req.note = note.map(str::to_string);
            Ok(())
        })
    }

    /// Whether an approval still needs typing into the asking session, i.e. no
    /// `wait` call is parked on it to carry the answer back.
    pub(crate) fn is_unattended(&self, id: &str) -> bool {
        self.requests
            .lock()
            .unwrap()
            .get(id)
            .is_none_or(|req| req.waiters == 0)
    }

    /// Park until request `id` settles, or `timeout` elapses — whichever comes
    /// first — and return it as it stands either way.
    ///
    /// `from` must be the sandbox that opened the request: waiting is also
    /// *reading*, since a settled request carries the answer.
    pub(crate) async fn wait(
        &self,
        id: &str,
        from: &str,
        timeout: Duration,
    ) -> Result<RelayRequest, String> {
        // Subscribe before the first look. The other order loses the race: a
        // request that settles between the read and the subscription would
        // leave this parked for the full timeout on news that already happened.
        let mut rx = self.subscribe();

        let current = {
            let mut map = self.requests.lock().unwrap();
            let req = map.get_mut(id).ok_or_else(|| {
                format!("no request '{id}' — it may have expired, or never existed")
            })?;
            if req.from != from {
                return Err(format!("request {id} was not opened by '{from}'"));
            }
            if req.state.is_final() {
                return Ok(req.clone());
            }
            req.waiters += 1;
            req.clone()
        };

        let outcome = self.wait_for_settle(&mut rx, id, timeout, current).await;
        if let Some(req) = self.requests.lock().unwrap().get_mut(id) {
            req.waiters = req.waiters.saturating_sub(1);
        }
        Ok(outcome)
    }

    /// The parked half of `wait`, split out so the waiter count is decremented
    /// on every path back — including the timeout.
    async fn wait_for_settle(
        &self,
        rx: &mut broadcast::Receiver<RelayRequest>,
        id: &str,
        timeout: Duration,
        mut latest: RelayRequest,
    ) -> RelayRequest {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let left = deadline.saturating_duration_since(tokio::time::Instant::now());
            if left.is_zero() {
                return latest;
            }
            match tokio::time::timeout(left, rx.recv()).await {
                Ok(Ok(req)) => {
                    if req.id != id {
                        continue;
                    }
                    if req.state.is_final() {
                        return req;
                    }
                    latest = req;
                }
                // Lagged: the state we want is still in the map, so re-read it
                // rather than treating a dropped message as an answer.
                Ok(Err(broadcast::error::RecvError::Lagged(_))) => {
                    if let Some(req) = self.get(id) {
                        if req.state.is_final() {
                            return req;
                        }
                        latest = req;
                    }
                }
                Ok(Err(broadcast::error::RecvError::Closed)) => return latest,
                Err(_) => return latest,
            }
        }
    }

    /// Forget requests nobody can act on any more (see `SETTLED_TTL` /
    /// `OPEN_TTL`). Returns how many were dropped, for the log.
    pub(crate) fn prune(&self, now: u64) -> usize {
        let settled = SETTLED_TTL.as_millis() as u64;
        let open = OPEN_TTL.as_millis() as u64;
        let mut map = self.requests.lock().unwrap();
        let before = map.len();
        map.retain(|_, req| {
            let age = now.saturating_sub(req.updated_ms);
            if req.state.is_final() {
                age < settled
            } else {
                age < open
            }
        });
        before - map.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn relay() -> Relay {
        Relay::new()
    }

    /// The happy path, stated as a whole: nothing reaches the asker before the
    /// last step, and the answer that reaches it is the approved one.
    #[test]
    fn an_answer_only_exists_once_a_human_has_released_it() {
        let r = relay();
        let req = r.open(
            "alpha",
            RelayKind::Question,
            "what shape is /v1/orders?",
            1_000,
        );
        assert_eq!(req.state, RelayState::Pending);
        assert!(req.answer.is_none());

        let routed = r.route(&req.id, "beta", 2_000).unwrap();
        assert_eq!(routed.state, RelayState::Routed);
        assert_eq!(routed.to.as_deref(), Some("beta"));

        let answered = r.reply(&req.id, "beta", "{ id, total }", 3_000).unwrap();
        assert_eq!(answered.state, RelayState::Answered);
        // Held, not released: `Answered` is not final, so a waiting `ask` keeps
        // waiting rather than returning what is on the table.
        assert!(!answered.state.is_final());

        let approved = r.approve(&req.id, None, None, 4_000).unwrap();
        assert_eq!(approved.state, RelayState::Approved);
        assert_eq!(approved.answer.as_deref(), Some("{ id, total }"));
    }

    /// The human's edit is the answer — not a note attached to the agent's.
    #[test]
    fn approving_with_text_replaces_what_the_target_wrote() {
        let r = relay();
        let req = r.open("alpha", RelayKind::Question, "the staging URL?", 1_000);
        r.route(&req.id, "beta", 2_000).unwrap();
        r.reply(
            &req.id,
            "beta",
            "https://staging.internal, token hunter2",
            3_000,
        )
        .unwrap();
        let approved = r
            .approve(&req.id, Some("https://staging.internal"), None, 4_000)
            .unwrap();
        assert_eq!(approved.answer.as_deref(), Some("https://staging.internal"));
    }

    /// A human can answer from their own head, with no second sandbox involved.
    #[test]
    fn a_human_can_answer_a_pending_request_themselves() {
        let r = relay();
        let req = r.open(
            "alpha",
            RelayKind::Question,
            "which region do we deploy to?",
            1_000,
        );
        let approved = r.approve(&req.id, Some("eu-west-1"), None, 2_000).unwrap();
        assert_eq!(approved.state, RelayState::Approved);
        assert_eq!(approved.answer.as_deref(), Some("eu-west-1"));
        // …but not out of thin air: with nothing written, there is nothing to
        // release.
        let bare = r.open("alpha", RelayKind::Question, "and the account id?", 3_000);
        assert!(r.approve(&bare.id, None, None, 4_000).is_err());
    }

    /// The rule that keeps this from being a bus: answering is scoped to the
    /// routing, not to being a sandbox.
    #[test]
    fn only_the_routed_sandbox_can_answer() {
        let r = relay();
        let req = r.open("alpha", RelayKind::Question, "?", 1_000);
        assert!(r.reply(&req.id, "beta", "…", 2_000).is_err());
        r.route(&req.id, "beta", 3_000).unwrap();
        assert!(r.reply(&req.id, "gamma", "…", 4_000).is_err());
        assert!(r.reply(&req.id, "beta", "…", 5_000).is_ok());
    }

    /// Re-routing after a silent target must not carry the old answer over: it
    /// was written by a sandbox that is no longer the one being asked.
    #[test]
    fn rerouting_drops_the_previous_answer() {
        let r = relay();
        let req = r.open("alpha", RelayKind::Question, "?", 1_000);
        r.route(&req.id, "beta", 2_000).unwrap();
        r.reply(&req.id, "beta", "beta's take", 3_000).unwrap();
        let rerouted = r.route(&req.id, "gamma", 4_000).unwrap();
        assert_eq!(rerouted.state, RelayState::Routed);
        assert!(rerouted.answer.is_none());
        // And beta, no longer the target, can no longer speak into it.
        assert!(r.reply(&req.id, "beta", "second thoughts", 5_000).is_err());
    }

    /// Denial is a full stop, and it takes the held answer with it — otherwise
    /// a later approval could release text a human already refused.
    #[test]
    fn denial_is_terminal_and_discards_the_answer() {
        let r = relay();
        let req = r.open("alpha", RelayKind::Question, "the prod credentials?", 1_000);
        r.route(&req.id, "beta", 2_000).unwrap();
        r.reply(&req.id, "beta", "hunter2", 3_000).unwrap();
        let denied = r
            .deny(&req.id, Some("not over this channel"), 4_000)
            .unwrap();
        assert_eq!(denied.state, RelayState::Denied);
        assert!(denied.answer.is_none());
        assert!(r.approve(&req.id, None, None, 5_000).is_err());
        assert!(r.route(&req.id, "gamma", 6_000).is_err());
        assert!(r.reply(&req.id, "beta", "hunter2", 7_000).is_err());
    }

    /// A sandbox must not be able to read a request it did not open, since
    /// waiting on a settled one hands back the answer.
    #[tokio::test]
    async fn waiting_is_scoped_to_the_sandbox_that_asked() {
        let r = relay();
        let req = r.open("alpha", RelayKind::Question, "?", 1_000);
        r.approve(&req.id, Some("released"), None, 2_000).unwrap();
        assert!(r
            .wait(&req.id, "beta", Duration::from_millis(10))
            .await
            .is_err());
        let mine = r
            .wait(&req.id, "alpha", Duration::from_millis(10))
            .await
            .unwrap();
        assert_eq!(mine.answer.as_deref(), Some("released"));
    }

    /// A `wait` that times out reports the request as it stands — it never
    /// invents a settlement, and it leaves nothing behind that would make the
    /// answer look attended to.
    #[tokio::test]
    async fn a_timed_out_wait_reports_the_open_state_and_deregisters() {
        let r = relay();
        let req = r.open("alpha", RelayKind::Question, "?", 1_000);
        r.route(&req.id, "beta", 2_000).unwrap();
        let out = r
            .wait(&req.id, "alpha", Duration::from_millis(20))
            .await
            .unwrap();
        assert_eq!(out.state, RelayState::Routed);
        assert!(out.answer.is_none());
        assert!(r.is_unattended(&req.id));
    }

    /// Approval reaching a parked `wait` is the whole delivery mechanism, so it
    /// is worth pinning that the waiter actually wakes on it.
    #[tokio::test]
    async fn an_approval_wakes_the_waiting_ask() {
        let r = std::sync::Arc::new(relay());
        let req = r.open("alpha", RelayKind::Question, "?", 1_000);
        let waiter = {
            let r = r.clone();
            let id = req.id.clone();
            tokio::spawn(async move { r.wait(&id, "alpha", Duration::from_secs(5)).await })
        };
        // Give the task time to park before settling the request, so this
        // exercises the broadcast rather than the pre-check in `wait`.
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!r.is_unattended(&req.id), "the waiter should be registered");
        r.approve(&req.id, Some("here you go"), None, 2_000)
            .unwrap();

        let out = waiter.await.unwrap().unwrap();
        assert_eq!(out.state, RelayState::Approved);
        assert_eq!(out.answer.as_deref(), Some("here you go"));
    }

    /// Pruning keeps a settled request readable for a while (a late `wait` still
    /// collects its answer) but lets an abandoned one go.
    #[test]
    fn pruning_outlives_a_late_pickup_but_not_an_abandoned_request() {
        let r = relay();
        let fresh = r.open("alpha", RelayKind::Question, "?", 0);
        let settled = r.open("alpha", RelayKind::Question, "?", 0);
        r.approve(&settled.id, Some("x"), None, 0).unwrap();

        let hour = 60 * 60 * 1000;
        assert_eq!(r.prune(10 * 60 * 1000), 0, "nothing is stale after 10 min");
        assert_eq!(r.prune(hour), 1, "the settled one goes after 30 min");
        assert!(r.get(&settled.id).is_none());
        assert!(
            r.get(&fresh.id).is_some(),
            "an open request waits for its human"
        );
        assert_eq!(r.prune(7 * hour), 1, "…but not forever");
    }

    fn shot() -> Shot {
        Shot {
            mime: "image/png".to_string(),
            b64: "iVBORw0KGgo=".to_string(),
        }
    }

    /// The short life of a screenshot request: opened, answered by the person it
    /// interrupted, released. No sandbox is involved at any point.
    #[test]
    fn a_screenshot_request_is_settled_by_the_human_alone() {
        let r = relay();
        let req = r.open(
            "alpha",
            RelayKind::Screenshot,
            "how does the header look?",
            1_000,
        );
        assert_eq!(req.state, RelayState::Pending);

        let approved = r
            .approve(&req.id, Some("mobile width"), Some(shot()), 2_000)
            .unwrap();
        assert_eq!(approved.state, RelayState::Approved);
        assert_eq!(approved.shot.as_ref().unwrap().mime, "image/png");
        // The caption rides along with the image rather than replacing it.
        assert_eq!(approved.answer.as_deref(), Some("mobile width"));
    }

    /// The rule that makes the kind more than a label. Routing one of these
    /// would hand a person's screen to an agent that cannot see it, and the
    /// human would be left waiting on a sandbox that has nothing to say.
    #[test]
    fn a_screenshot_request_cannot_be_sent_to_another_sandbox() {
        let r = relay();
        let req = r.open(
            "alpha",
            RelayKind::Screenshot,
            "what does it look like?",
            1_000,
        );
        assert!(r.route(&req.id, "beta", 2_000).is_err());
        // And with no routing there is nothing for a sandbox to answer into.
        assert!(r.reply(&req.id, "beta", "looks fine to me", 3_000).is_err());
        assert_eq!(r.get(&req.id).unwrap().state, RelayState::Pending);
    }

    /// Approval needs something to send, in whichever form. A screenshot request
    /// answered in words is a human choosing to describe rather than show — a
    /// perfectly good outcome, and not one this layer overrules.
    #[test]
    fn approving_needs_a_payload_but_not_a_particular_one() {
        let r = relay();
        let empty = r.open("alpha", RelayKind::Screenshot, "?", 1_000);
        assert!(r.approve(&empty.id, None, None, 2_000).is_err());

        let described = r.open("alpha", RelayKind::Screenshot, "?", 3_000);
        let approved = r
            .approve(
                &described.id,
                Some("the header wraps at 400px"),
                None,
                4_000,
            )
            .unwrap();
        assert_eq!(approved.state, RelayState::Approved);
        assert!(approved.shot.is_none());
    }

    /// Refusing is a full stop for pixels too: "not this window" must not be
    /// something a later approval can walk back by attaching an image to the
    /// same request.
    #[test]
    fn denial_closes_the_door_on_a_screenshot() {
        let r = relay();
        let req = r.open("alpha", RelayKind::Screenshot, "?", 1_000);
        let denied = r
            .deny(&req.id, Some("that window has customer data"), 2_000)
            .unwrap();
        assert_eq!(denied.state, RelayState::Denied);
        assert!(denied.shot.is_none());
        assert!(r.approve(&req.id, None, Some(shot()), 3_000).is_err());
    }

    /// What the SSE stream and the popup are told about an image: that there is
    /// one. The bytes travel only in the reply to the sandbox that asked.
    #[test]
    fn the_broadcast_carries_presence_not_pixels() {
        let r = relay();
        let req = r.open("alpha", RelayKind::Screenshot, "?", 1_000);
        let approved = r.approve(&req.id, None, Some(shot()), 2_000).unwrap();
        let json = serde_json::to_string(&approved).unwrap();
        assert!(json.contains("\"has_shot\":true"));
        assert!(json.contains("\"kind\":\"screenshot\""));
        assert!(!json.contains("iVBORw0KGgo"));
    }
}
