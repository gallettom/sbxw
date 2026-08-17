//! Cross-sandbox information requests, with a human on every hop.
//!
//! An agent in one sandbox asks a question (`assets/relay-tool.js`); a person
//! in the web UI decides which *other* sandbox — if any — is asked; that
//! sandbox's agent answers; the same person decides whether the answer is
//! released. Only then does the asking side learn anything.
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

/// One question and everything that has happened to it.
#[derive(Clone, Serialize, Debug)]
pub(crate) struct RelayRequest {
    pub(crate) id: String,
    /// Sandbox that asked.
    pub(crate) from: String,
    /// The question, as the asking agent wrote it. Untrusted text: it is shown
    /// to a person and typed into another agent's session, never interpreted
    /// here.
    pub(crate) question: String,
    /// Sandbox a human routed it to, once one has been chosen.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) to: Option<String>,
    /// The answer — held while `Answered`, released only by `Approved`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) answer: Option<String>,
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
    /// asking is the human's job, not this function's.
    pub(crate) fn open(&self, from: &str, question: &str, now: u64) -> RelayRequest {
        let id = format!("r-{}", self.seq.fetch_add(1, Ordering::Relaxed));
        let req = RelayRequest {
            id: id.clone(),
            from: from.to_string(),
            question: question.to_string(),
            to: None,
            answer: None,
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

    /// A human releases an answer to the asker — either the one under review,
    /// or their own text, which is also how a question gets answered without
    /// involving a second sandbox at all.
    pub(crate) fn approve(
        &self,
        id: &str,
        answer: Option<&str>,
        now: u64,
    ) -> Result<RelayRequest, String> {
        self.mutate(id, now, |req| {
            if req.state.is_final() {
                return Err(format!("request {id} is already settled"));
            }
            if let Some(text) = answer {
                req.answer = Some(text.to_string());
            }
            if req.answer.is_none() {
                return Err("there is no answer to approve yet".to_string());
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
            // rather than kept where an approval could later release it.
            req.answer = None;
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
        let req = r.open("alpha", "what shape is /v1/orders?", 1_000);
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

        let approved = r.approve(&req.id, None, 4_000).unwrap();
        assert_eq!(approved.state, RelayState::Approved);
        assert_eq!(approved.answer.as_deref(), Some("{ id, total }"));
    }

    /// The human's edit is the answer — not a note attached to the agent's.
    #[test]
    fn approving_with_text_replaces_what_the_target_wrote() {
        let r = relay();
        let req = r.open("alpha", "the staging URL?", 1_000);
        r.route(&req.id, "beta", 2_000).unwrap();
        r.reply(
            &req.id,
            "beta",
            "https://staging.internal, token hunter2",
            3_000,
        )
        .unwrap();
        let approved = r
            .approve(&req.id, Some("https://staging.internal"), 4_000)
            .unwrap();
        assert_eq!(approved.answer.as_deref(), Some("https://staging.internal"));
    }

    /// A human can answer from their own head, with no second sandbox involved.
    #[test]
    fn a_human_can_answer_a_pending_request_themselves() {
        let r = relay();
        let req = r.open("alpha", "which region do we deploy to?", 1_000);
        let approved = r.approve(&req.id, Some("eu-west-1"), 2_000).unwrap();
        assert_eq!(approved.state, RelayState::Approved);
        assert_eq!(approved.answer.as_deref(), Some("eu-west-1"));
        // …but not out of thin air: with nothing written, there is nothing to
        // release.
        let bare = r.open("alpha", "and the account id?", 3_000);
        assert!(r.approve(&bare.id, None, 4_000).is_err());
    }

    /// The rule that keeps this from being a bus: answering is scoped to the
    /// routing, not to being a sandbox.
    #[test]
    fn only_the_routed_sandbox_can_answer() {
        let r = relay();
        let req = r.open("alpha", "?", 1_000);
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
        let req = r.open("alpha", "?", 1_000);
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
        let req = r.open("alpha", "the prod credentials?", 1_000);
        r.route(&req.id, "beta", 2_000).unwrap();
        r.reply(&req.id, "beta", "hunter2", 3_000).unwrap();
        let denied = r
            .deny(&req.id, Some("not over this channel"), 4_000)
            .unwrap();
        assert_eq!(denied.state, RelayState::Denied);
        assert!(denied.answer.is_none());
        assert!(r.approve(&req.id, None, 5_000).is_err());
        assert!(r.route(&req.id, "gamma", 6_000).is_err());
        assert!(r.reply(&req.id, "beta", "hunter2", 7_000).is_err());
    }

    /// A sandbox must not be able to read a request it did not open, since
    /// waiting on a settled one hands back the answer.
    #[tokio::test]
    async fn waiting_is_scoped_to_the_sandbox_that_asked() {
        let r = relay();
        let req = r.open("alpha", "?", 1_000);
        r.approve(&req.id, Some("released"), 2_000).unwrap();
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
        let req = r.open("alpha", "?", 1_000);
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
        let req = r.open("alpha", "?", 1_000);
        let waiter = {
            let r = r.clone();
            let id = req.id.clone();
            tokio::spawn(async move { r.wait(&id, "alpha", Duration::from_secs(5)).await })
        };
        // Give the task time to park before settling the request, so this
        // exercises the broadcast rather than the pre-check in `wait`.
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!r.is_unattended(&req.id), "the waiter should be registered");
        r.approve(&req.id, Some("here you go"), 2_000).unwrap();

        let out = waiter.await.unwrap().unwrap();
        assert_eq!(out.state, RelayState::Approved);
        assert_eq!(out.answer.as_deref(), Some("here you go"));
    }

    /// Pruning keeps a settled request readable for a while (a late `wait` still
    /// collects its answer) but lets an abandoned one go.
    #[test]
    fn pruning_outlives_a_late_pickup_but_not_an_abandoned_request() {
        let r = relay();
        let fresh = r.open("alpha", "?", 0);
        let settled = r.open("alpha", "?", 0);
        r.approve(&settled.id, Some("x"), 0).unwrap();

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
}
