//! Requests an agent cannot settle from inside its own sandbox, with a human
//! on every hop.
//!
//! A **question** is about a workspace this agent cannot see: a person picks
//! which other sandbox — if any — is asked, that sandbox's agent answers, and
//! the same person decides whether the answer is released. A **screenshot** is
//! about what only the person at the keyboard can see, so it is never routed:
//! it is answered, or refused, by the human it interrupted.
//!
//! Two rules shape everything:
//!
//! * **No transition happens on its own.** No timeout routes a question, and no
//!   answer flows onward without an explicit approval.
//! * **A sandbox may only touch its own side of a request.** The asker waits on
//!   what it opened; the routed-to sandbox answers what it was handed. With
//!   agent tokens (see the daemon), "a sandbox" is an identity the daemon
//!   verified, not one the request claims.
//!
//! This is the state machine only: no clock, no channels, no I/O. Every
//! transition takes `now` and returns the request as it now stands, or why not.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Debug)]
#[serde(rename_all = "lowercase")]
pub enum RelayState {
    /// Open; nobody asked yet.
    Pending,
    /// Sent to `to`, which has not answered.
    Routed,
    /// `to` answered; held for review, *not* released.
    Answered,
    /// A human released an answer to the asker. Final.
    Approved,
    /// A human refused. Final.
    Denied,
}

impl RelayState {
    pub fn is_final(self) -> bool {
        matches!(self, RelayState::Approved | RelayState::Denied)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Debug, Default)]
#[serde(rename_all = "lowercase")]
pub enum RelayKind {
    #[default]
    Question,
    Screenshot,
    /// A sandbox asks to boot the virtual Mac or open a port to it: a yes or
    /// a no from the person, never routed (see `mac`).
    Mac,
    /// Questions for the person themselves (a /grill-me round), answered on
    /// one form in the relay dialog. The person may route the round to another
    /// sandbox for a draft, which they review and release like any routed
    /// answer. The answers come back as the asking tool's result: nothing is
    /// typed into a terminal.
    Ask,
}

/// One attached image, as the base64 the browser sent. Never decoded here:
/// the daemon is a courier, and the sandbox that asked decodes it anyway.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Shot {
    pub mime: String,
    pub b64: String,
}

fn shot_count<S: serde::Serializer>(shots: &[Shot], s: S) -> Result<S::Ok, S::Error> {
    s.serialize_u64(shots.len() as u64)
}

/// A request and everything that has happened to it. Serialized whole for the
/// browser and the island — images as a count, never as pixels.
#[derive(Clone, Serialize, Debug, PartialEq)]
pub struct RelayRequest {
    pub id: String,
    pub from: String,
    /// The thread of `from` that asked, when one did: an outcome is typed into
    /// the session that asked, not the agent beside it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from_thread: Option<String>,
    pub kind: RelayKind,
    /// Untrusted text either way: shown to a person, typed into another agent.
    pub question: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub answer: Option<String>,
    #[serde(rename = "shot_count", serialize_with = "shot_count")]
    pub shots: Vec<Shot>,
    pub state: RelayState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    pub created_ms: u64,
    pub updated_ms: u64,
    /// What the asker's relay server made for this one tool call: a retried
    /// open with the same key is the same request, not a second one.
    #[serde(skip)]
    pub client_key: Option<String>,
    /// The final outcome is the asker's: its relay server acknowledged the
    /// tool result, an old client read it, or it was handed to delivery.
    #[serde(skip)]
    pub delivered: bool,
    /// When a wait last returned the final outcome to a client that will
    /// acknowledge it; past `ACK_LIMIT_MS` without that, it is owed.
    #[serde(skip)]
    pub returned_ms: Option<u64>,
    /// The person was reminded of it once (`REMIND_AFTER_MS`).
    #[serde(skip)]
    pub reminded: bool,
}

/// The whole request as `state/relay.json` keeps it, images included: what
/// the browser's form leaves out is what a restart must not lose.
#[derive(Serialize, Deserialize)]
struct Stored {
    id: String,
    from: String,
    #[serde(default)]
    from_thread: Option<String>,
    #[serde(default)]
    kind: RelayKind,
    question: String,
    #[serde(default)]
    to: Option<String>,
    #[serde(default)]
    answer: Option<String>,
    #[serde(default)]
    shots: Vec<Shot>,
    state: RelayState,
    #[serde(default)]
    note: Option<String>,
    created_ms: u64,
    updated_ms: u64,
    #[serde(default)]
    client_key: Option<String>,
    #[serde(default)]
    delivered: bool,
    #[serde(default)]
    returned_ms: Option<u64>,
    #[serde(default)]
    reminded: bool,
}

impl From<&RelayRequest> for Stored {
    fn from(r: &RelayRequest) -> Self {
        Stored {
            id: r.id.clone(),
            from: r.from.clone(),
            from_thread: r.from_thread.clone(),
            kind: r.kind,
            question: r.question.clone(),
            to: r.to.clone(),
            answer: r.answer.clone(),
            shots: r.shots.clone(),
            state: r.state,
            note: r.note.clone(),
            created_ms: r.created_ms,
            updated_ms: r.updated_ms,
            client_key: r.client_key.clone(),
            delivered: r.delivered,
            returned_ms: r.returned_ms,
            reminded: r.reminded,
        }
    }
}

impl From<Stored> for RelayRequest {
    fn from(s: Stored) -> Self {
        RelayRequest {
            id: s.id,
            from: s.from,
            from_thread: s.from_thread,
            kind: s.kind,
            question: s.question,
            to: s.to,
            answer: s.answer,
            shots: s.shots,
            state: s.state,
            note: s.note,
            created_ms: s.created_ms,
            updated_ms: s.updated_ms,
            client_key: s.client_key,
            delivered: s.delivered,
            returned_ms: s.returned_ms,
            reminded: s.reminded,
        }
    }
}

/// What happened to a request, for whoever tells the person (the desktop,
/// the phone): one `relay-note` event per step, with the request's id and
/// creation time, which together name it for good.
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Debug)]
#[serde(rename_all = "lowercase")]
pub enum RelayStep {
    /// A sandbox opened it: it waits for the person.
    Opened,
    /// The person sent it to another sandbox.
    Routed,
    /// Delivery to that sandbox failed: back with the person.
    Unrouted,
    /// The routed-to sandbox answered: it waits for the person's review.
    Answered,
    /// Approved or denied: final.
    Settled,
    /// It has waited for the person `REMIND_AFTER_MS` with nothing done.
    /// Once per request.
    Reminder,
    /// Pruned after `OPEN_TTL_MS` still open; the asker was told.
    Expired,
    /// A final outcome the asker's tool call never acknowledged, handed to
    /// delivery instead.
    Owed,
}

#[derive(Clone, Serialize, Debug, PartialEq)]
pub struct RelayNote {
    pub step: RelayStep,
    pub id: String,
    pub kind: RelayKind,
    pub state: RelayState,
    pub from: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from_thread: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
    pub created_ms: u64,
    /// When this step happened.
    pub at_ms: u64,
}

impl RelayNote {
    pub fn of(step: RelayStep, req: &RelayRequest, at_ms: u64) -> Self {
        RelayNote {
            step,
            id: req.id.clone(),
            kind: req.kind,
            state: req.state,
            from: req.from.clone(),
            from_thread: req.from_thread.clone(),
            to: req.to.clone(),
            created_ms: req.created_ms,
            at_ms,
        }
    }
}

impl RelayRequest {
    /// Waiting on the person: to be routed or answered, or a draft to review.
    pub fn waits_for_person(&self) -> bool {
        matches!(self.state, RelayState::Pending | RelayState::Answered)
    }

    /// How many parts the answer takes for an agent (at least 1).
    pub fn answer_parts(&self) -> usize {
        self.answer.as_deref().map(|a| answer_part(a, usize::MAX).1).unwrap_or(1)
    }
}

/// How long a settled request is kept for a late `wait` (30 min).
pub const SETTLED_TTL_MS: u64 = 30 * 60 * 1000;
/// How long an open request waits for a human (6 h).
pub const OPEN_TTL_MS: u64 = 6 * 60 * 60 * 1000;
pub const MAX_QUESTION: usize = 4000;
/// A round of questions for the person: up to 12 questions with their
/// recommendations, which 4000 characters did not hold.
pub const MAX_ROUND: usize = 32000;
/// What is kept of an answer. An agent reads it in `PART_CHARS` parts.
pub const MAX_ANSWER: usize = 200_000;
/// One part of an answer in a tool result: what fits comfortably in an
/// agent's context at once. A longer answer comes in parts, never cut.
pub const PART_CHARS: usize = 12_000;
/// A final outcome a wait returned and the asker's relay server did not
/// acknowledge within this is owed: handed to delivery.
pub const ACK_LIMIT_MS: u64 = 30_000;
/// A request waiting on the person this long with nothing done is
/// reminded, once (decision D17).
pub const REMIND_AFTER_MS: u64 = 10 * 60 * 1000;

/// Part `n` (1-based, clamped) of `answer` in `PART_CHARS` characters, cut
/// at a line break when there is one in the part's last quarter, and how
/// many parts there are.
pub fn answer_part(answer: &str, n: usize) -> (&str, usize) {
    let mut cuts = vec![0usize];
    let mut start = 0;
    while let Some((hard, _)) = answer[start..].char_indices().nth(PART_CHARS) {
        let hard = start + hard;
        let window = &answer[start..hard];
        let soft = window
            .rfind('\n')
            .filter(|&i| i >= window.len() * 3 / 4)
            .map(|i| start + i + 1)
            .unwrap_or(hard);
        cuts.push(soft);
        start = soft;
    }
    cuts.push(answer.len());
    let parts = cuts.len() - 1;
    let k = n.clamp(1, parts);
    (&answer[cuts[k - 1]..cuts[k]], parts)
}
/// One image's base64: a backstop, the browser downscales first.
pub const MAX_SHOT_B64: usize = 6 * 1024 * 1024;
/// Images per request: every one lands in an agent's context.
pub const MAX_SHOTS: usize = 6;
/// All images of one request together.
pub const MAX_SHOTS_B64: usize = 16 * 1024 * 1024;
pub const SHOT_MIMES: &[&str] = &["image/png", "image/jpeg", "image/webp"];

/// What a *sandbox* may see of a request it opened. The answer and the images
/// exist on the server from the moment they are written, but belong to the
/// asker only once a human approved — serializing the request wholesale would
/// make the review decorative.
pub fn agent_view(req: &RelayRequest) -> serde_json::Value {
    agent_view_part(req, 1)
}

/// `agent_view` with part `part` of a long answer: `answer_part` and
/// `answer_parts` say where it stands, so nothing is lost to a cut.
pub fn agent_view_part(req: &RelayRequest, part: usize) -> serde_json::Value {
    let released = req.state == RelayState::Approved;
    let images: Vec<serde_json::Value> = if released {
        req.shots
            .iter()
            .map(|s| serde_json::json!({ "mime": s.mime, "b64": s.b64 }))
            .collect()
    } else {
        Vec::new()
    };
    let (answer, part, parts) = match req.answer.as_deref().filter(|_| released) {
        Some(a) => {
            let (text, parts) = answer_part(a, part);
            (Some(text.to_string()), part.clamp(1, parts), parts)
        }
        None => (None, 1, 1),
    };
    serde_json::json!({
        "id": req.id,
        "kind": req.kind,
        "state": req.state,
        "to": req.to,
        "note": req.note,
        "answer": answer,
        "answer_part": part,
        "answer_parts": parts,
        "images": images,
        // The first again, alone, for a relay CLI from before images were a list.
        "image": images.first(),
    })
}

/// Every live request.
///
/// Kept in `state/relay.json` by the daemon, the counter with it: an id
/// names one request for good, across restarts (`r-3` is never reused).
#[derive(Default, Debug)]
pub struct RelayBook {
    requests: HashMap<String, RelayRequest>,
    seq: u64,
    /// Changed since the last `take_dirty`: the daemon writes the file then.
    dirty: bool,
}

#[derive(Serialize, Deserialize)]
struct StoredBook {
    seq: u64,
    #[serde(default)]
    requests: Vec<serde_json::Value>,
}

/// Longest client key kept: a UUID is 36.
pub const MAX_CLIENT_KEY: usize = 80;

impl RelayBook {
    pub fn new() -> Self {
        Self::default()
    }

    /// The book as `state/relay.json` holds it.
    pub fn to_json(&self) -> String {
        let book = StoredBook {
            seq: self.seq,
            requests: self.list().iter().filter_map(|r| serde_json::to_value(Stored::from(r)).ok()).collect(),
        };
        serde_json::to_string(&book).unwrap_or_default()
    }

    /// Read back leniently: a request that no longer parses is dropped, the
    /// others kept. `None` when the file is not a book at all. The counter
    /// never goes below an id it finds.
    pub fn parse_lenient(text: &str) -> Option<RelayBook> {
        let book: StoredBook = serde_json::from_str(text).ok()?;
        let mut seq = book.seq;
        let mut requests = HashMap::new();
        for v in book.requests {
            let Ok(s) = serde_json::from_value::<Stored>(v) else { continue };
            if let Some(n) = s.id.strip_prefix("r-").and_then(|n| n.parse::<u64>().ok()) {
                seq = seq.max(n);
            }
            requests.insert(s.id.clone(), RelayRequest::from(s));
        }
        Some(RelayBook { requests, seq, dirty: false })
    }

    /// Whether the book changed since the last call.
    pub fn take_dirty(&mut self) -> bool {
        std::mem::take(&mut self.dirty)
    }

    pub fn open(&mut self, from: &str, from_thread: Option<&str>, kind: RelayKind, question: &str, now: u64) -> RelayRequest {
        self.open_keyed(from, from_thread, kind, question, None, now).0
    }

    /// Open a request, or, when `from` already opened one with `client_key`,
    /// return that one (`false`): a retried open is not a second request.
    pub fn open_keyed(
        &mut self,
        from: &str,
        from_thread: Option<&str>,
        kind: RelayKind,
        question: &str,
        client_key: Option<&str>,
        now: u64,
    ) -> (RelayRequest, bool) {
        let key = client_key.map(str::trim).filter(|k| !k.is_empty()).map(|k| {
            let end = k.char_indices().nth(MAX_CLIENT_KEY).map(|(i, _)| i).unwrap_or(k.len());
            k[..end].to_string()
        });
        if let Some(k) = key.as_deref() {
            if let Some(r) = self.requests.values().find(|r| r.from == from && r.client_key.as_deref() == Some(k)) {
                return (r.clone(), false);
            }
        }
        self.seq += 1;
        self.dirty = true;
        let req = RelayRequest {
            id: format!("r-{}", self.seq),
            from: from.to_string(),
            from_thread: from_thread.map(str::to_string),
            kind,
            question: question.to_string(),
            to: None,
            answer: None,
            shots: Vec::new(),
            state: RelayState::Pending,
            note: None,
            created_ms: now,
            updated_ms: now,
            client_key: key,
            delivered: false,
            returned_ms: None,
            reminded: false,
        };
        self.requests.insert(req.id.clone(), req.clone());
        (req, true)
    }

    pub fn get(&self, id: &str) -> Option<&RelayRequest> {
        self.requests.get(id)
    }

    /// Oldest first.
    pub fn list(&self) -> Vec<RelayRequest> {
        let mut out: Vec<RelayRequest> = self.requests.values().cloned().collect();
        out.sort_by(|a, b| a.created_ms.cmp(&b.created_ms).then(a.id.cmp(&b.id)));
        out
    }

    fn mutate<F>(&mut self, id: &str, now: u64, open_only: bool, f: F) -> Result<RelayRequest, String>
    where
        F: FnOnce(&mut RelayRequest) -> Result<(), String>,
    {
        let req = self
            .requests
            .get_mut(id)
            .ok_or_else(|| format!("no request '{id}' — it may have been answered long ago"))?;
        if open_only && req.state.is_final() {
            return Err(format!("request {id} is already settled"));
        }
        f(req)?;
        req.updated_ms = now;
        self.dirty = true;
        Ok(req.clone())
    }

    /// A human sends the question to `to` — also how it is re-routed; an
    /// earlier answer is dropped, it spoke for another sandbox.
    pub fn route(&mut self, id: &str, to: &str, now: u64) -> Result<RelayRequest, String> {
        self.mutate(id, now, true, |req| {
            if req.kind == RelayKind::Screenshot {
                return Err(format!(
                    "request {id} asks for a screenshot — no other sandbox can see this screen"
                ));
            }
            if req.kind == RelayKind::Mac {
                return Err(format!("request {id} asks you to approve something on the virtual Mac — only you can"));
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

    /// A person approved a virtual Mac request and sbxw is carrying it out:
    /// off every "waiting on you" list (it reads as routed, to sbxw) until
    /// `approve` settles it with what came of it.
    pub fn carry(&mut self, id: &str, now: u64) -> Result<RelayRequest, String> {
        self.mutate(id, now, true, |req| {
            if req.kind != RelayKind::Mac || req.state != RelayState::Pending {
                return Err(format!("request {id} is not a virtual Mac request waiting on you"));
            }
            req.to = Some("sbxw".to_string());
            req.state = RelayState::Routed;
            Ok(())
        })
    }

    /// Delivery to `to` failed: back to the human, saying why. Only while
    /// it is still routed to `to`: a late failure for an earlier target
    /// does not undo a later routing.
    pub fn unroute(&mut self, id: &str, to: &str, why: &str, now: u64) -> Result<RelayRequest, String> {
        self.mutate(id, now, false, |req| {
            if req.state != RelayState::Routed {
                return Err(format!("request {id} is no longer being routed"));
            }
            if req.to.as_deref() != Some(to) {
                return Err(format!("request {id} was routed elsewhere since"));
            }
            req.to = None;
            req.state = RelayState::Pending;
            req.note = Some(why.to_string());
            Ok(())
        })
    }

    /// The routed-to sandbox answers — only the one it was handed.
    pub fn reply(&mut self, id: &str, from: &str, answer: &str, now: u64) -> Result<RelayRequest, String> {
        self.mutate(id, now, true, |req| {
            if req.to.as_deref() != Some(from) {
                return Err(format!("request {id} was not sent to '{from}'"));
            }
            req.answer = Some(answer.to_string());
            req.state = RelayState::Answered;
            Ok(())
        })
    }

    /// A human releases what the asker gets: an answer, the images, or both.
    /// Something has to go out. `shots` is the whole set as the human left it.
    pub fn approve(&mut self, id: &str, answer: Option<&str>, shots: Vec<Shot>, now: u64) -> Result<RelayRequest, String> {
        self.mutate(id, now, true, |req| {
            if let Some(text) = answer {
                req.answer = Some(text.to_string());
            }
            if !shots.is_empty() {
                req.shots = shots;
            }
            if req.answer.is_none() && req.shots.is_empty() {
                return Err("there is nothing to send yet".to_string());
            }
            req.state = RelayState::Approved;
            Ok(())
        })
    }

    /// A human refuses: a full stop, and everything held is dropped.
    pub fn deny(&mut self, id: &str, note: Option<&str>, now: u64) -> Result<RelayRequest, String> {
        self.mutate(id, now, true, |req| {
            req.answer = None;
            req.shots.clear();
            req.state = RelayState::Denied;
            req.note = note.map(str::to_string);
            Ok(())
        })
    }

    /// Waiting is reading: only the asker may, the sandbox and, when
    /// `thread` is given (`Some(None)`: the sandbox's own agent), the
    /// session that asked. `None` checks the sandbox only, for a client from
    /// before threads were sent with a wait.
    pub fn check_asker(&self, id: &str, from: &str, thread: Option<Option<&str>>) -> Result<&RelayRequest, String> {
        let req = self
            .requests
            .get(id)
            .ok_or_else(|| format!("no request '{id}' — it may have expired, or never existed"))?;
        if req.from != from {
            return Err(format!("request {id} was not opened by '{from}'"));
        }
        if let Some(t) = thread {
            if req.from_thread.as_deref() != t {
                return Err(match req.from_thread.as_deref() {
                    Some(owner) => format!("request {id} was opened by thread '{owner}', not by this session"),
                    None => format!("request {id} was opened by this sandbox's agent, not by this thread"),
                });
            }
        }
        Ok(req)
    }

    /// A wait returned the final outcome to a client that acknowledges.
    pub fn mark_returned(&mut self, id: &str, now: u64) {
        if let Some(r) = self.requests.get_mut(id).filter(|r| r.state.is_final() && !r.delivered) {
            r.returned_ms = Some(now);
            self.dirty = true;
        }
    }

    /// The asker has the outcome. `true` the first time.
    pub fn mark_delivered(&mut self, id: &str) -> bool {
        match self.requests.get_mut(id).filter(|r| r.state.is_final() && !r.delivered) {
            Some(r) => {
                r.delivered = true;
                self.dirty = true;
                true
            }
            None => false,
        }
    }

    /// Final outcomes the asker does not have: returned and not acknowledged
    /// within `ACK_LIMIT_MS`, or never returned while nobody waits for them
    /// (`parked` says who does), as after a restart. Each is marked
    /// delivered here: the caller hands it to delivery, once.
    pub fn take_owed(&mut self, now: u64, parked: impl Fn(&str) -> bool) -> Vec<RelayRequest> {
        let mut out = Vec::new();
        for r in self.requests.values_mut() {
            if !r.state.is_final() || r.delivered {
                continue;
            }
            let owed = match r.returned_ms {
                Some(t) => now.saturating_sub(t) >= ACK_LIMIT_MS,
                None => !parked(&r.id) && now.saturating_sub(r.updated_ms) >= ACK_LIMIT_MS,
            };
            if owed {
                r.delivered = true;
                out.push(r.clone());
            }
        }
        if !out.is_empty() {
            self.dirty = true;
        }
        out.sort_by(|a, b| a.created_ms.cmp(&b.created_ms));
        out
    }

    /// Requests that have waited for the person `REMIND_AFTER_MS` since
    /// their last change, not reminded before: marked, once per request.
    pub fn take_reminders(&mut self, now: u64) -> Vec<RelayRequest> {
        let mut out = Vec::new();
        for r in self.requests.values_mut() {
            if !r.reminded && r.waits_for_person() && now.saturating_sub(r.updated_ms) >= REMIND_AFTER_MS {
                r.reminded = true;
                out.push(r.clone());
            }
        }
        if !out.is_empty() {
            self.dirty = true;
        }
        out.sort_by(|a, b| a.created_ms.cmp(&b.created_ms));
        out
    }

    /// Reading a routed request: only the sandbox it was handed, and only
    /// while it waits on that sandbox's answer. Anyone else, the asker
    /// included, gets the refusal an unknown id gets, so the id's existence
    /// is not revealed.
    pub fn check_target(&self, id: &str, me: &str) -> Result<&RelayRequest, String> {
        self.requests
            .get(id)
            .filter(|r| r.state == RelayState::Routed && r.to.as_deref() == Some(me))
            .ok_or_else(|| format!("no request '{id}' for you — it may have expired, or never existed"))
    }

    /// Forget requests nobody can act on any more. Returns how many.
    pub fn prune(&mut self, now: u64) -> usize {
        self.prune_taken(now).len()
    }

    /// `prune`, returning what it dropped: an open one among them expired,
    /// and its asker is owed being told.
    pub fn prune_taken(&mut self, now: u64) -> Vec<RelayRequest> {
        let gone: Vec<String> = self
            .requests
            .values()
            .filter(|r| now.saturating_sub(r.updated_ms) >= if r.state.is_final() { SETTLED_TTL_MS } else { OPEN_TTL_MS })
            .map(|r| r.id.clone())
            .collect();
        let mut out: Vec<RelayRequest> = gone.iter().filter_map(|id| self.requests.remove(id)).collect();
        if !out.is_empty() {
            self.dirty = true;
        }
        out.sort_by(|a, b| a.created_ms.cmp(&b.created_ms));
        out
    }
}

/// A screenshot out of the `data:` URL the browser built. Strict about the
/// envelope, incurious about the contents.
pub fn parse_shot(data_url: &str) -> Result<Shot, String> {
    let rest = data_url
        .trim()
        .strip_prefix("data:")
        .ok_or("that is not an image the browser could read")?;
    let (mime, b64) = rest.split_once(";base64,").ok_or("expected base64 image data")?;
    let mime = mime.trim().to_ascii_lowercase();
    if !SHOT_MIMES.contains(&mime.as_str()) {
        return Err(format!("sbxw does not pass on {mime} images"));
    }
    if b64.len() > MAX_SHOT_B64 {
        return Err("that image is too large to send — try a window rather than the whole screen".into());
    }
    if b64.is_empty()
        || !b64
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'+' || c == b'/' || c == b'=')
    {
        return Err("that image did not arrive intact".into());
    }
    Ok(Shot {
        mime,
        b64: b64.to_string(),
    })
}

/// The whole set a person attached, checked by count and by weight together.
pub fn parse_shots(urls: &[String]) -> Result<Vec<Shot>, String> {
    let wanted: Vec<&String> = urls.iter().filter(|u| !u.trim().is_empty()).collect();
    if wanted.len() > MAX_SHOTS {
        return Err(format!("that is more than {MAX_SHOTS} images — send the ones that show it best"));
    }
    let mut total = 0;
    let mut out = Vec::with_capacity(wanted.len());
    for url in wanted {
        let shot = parse_shot(url)?;
        total += shot.b64.len();
        if total > MAX_SHOTS_B64 {
            return Err(
                "those images are too large to send together — remove one, or capture windows \
                 rather than whole screens"
                    .into(),
            );
        }
        out.push(shot);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shot(b64: &str) -> Shot {
        Shot {
            mime: "image/png".into(),
            b64: b64.into(),
        }
    }

    #[test]
    fn an_answer_only_reaches_the_asker_once_released() {
        let mut b = RelayBook::new();
        let r = b.open("alpha", None, RelayKind::Question, "q?", 1);
        b.route(&r.id, "beta", 2).unwrap();
        let answered = b.reply(&r.id, "beta", "42", 3).unwrap();
        assert_eq!(answered.state, RelayState::Answered);
        assert_eq!(agent_view(&answered)["answer"], serde_json::Value::Null);
        let approved = b.approve(&r.id, None, vec![], 4).unwrap();
        assert_eq!(agent_view(&approved)["answer"], "42");
    }

    #[test]
    fn only_the_routed_sandbox_answers_and_rerouting_drops_its_answer() {
        let mut b = RelayBook::new();
        let r = b.open("alpha", None, RelayKind::Question, "q?", 1);
        b.route(&r.id, "beta", 2).unwrap();
        assert!(b.reply(&r.id, "gamma", "x", 3).is_err());
        b.reply(&r.id, "beta", "x", 3).unwrap();
        let rerouted = b.route(&r.id, "gamma", 4).unwrap();
        assert_eq!(rerouted.answer, None);
        assert!(b.route(&r.id, "alpha", 5).is_err(), "not to the asker");
    }

    #[test]
    fn denial_is_terminal_and_discards() {
        let mut b = RelayBook::new();
        let r = b.open("a", None, RelayKind::Question, "q", 1);
        b.route(&r.id, "b", 2).unwrap();
        b.reply(&r.id, "b", "secret", 3).unwrap();
        let d = b.deny(&r.id, Some("no"), 4).unwrap();
        assert_eq!(d.answer, None);
        assert!(b.approve(&r.id, Some("x"), vec![], 5).is_err());
    }

    #[test]
    fn a_screenshot_is_never_routed_and_needs_a_payload() {
        let mut b = RelayBook::new();
        let r = b.open("a", Some("t1"), RelayKind::Screenshot, "show me", 1);
        assert!(b.route(&r.id, "b", 2).is_err());
        assert!(b.approve(&r.id, None, vec![], 2).is_err());
        let ok = b.approve(&r.id, None, vec![shot("AAAA"), shot("BBBB")], 3).unwrap();
        let v = agent_view(&ok);
        assert_eq!(v["images"].as_array().unwrap().len(), 2);
        assert_eq!(v["image"]["b64"], "AAAA");
        let wire = serde_json::to_value(&ok).unwrap();
        assert_eq!(wire["shot_count"], 2, "the browser gets a count, not pixels");
        assert_eq!(wire["from_thread"], "t1");
    }

    #[test]
    fn a_mac_request_is_carried_once_then_settled() {
        let mut b = RelayBook::new();
        let r = b.open("a", None, RelayKind::Mac, "boot?", 1);
        assert!(b.route(&r.id, "b", 2).is_err(), "a Mac request is never routed");
        let carried = b.carry(&r.id, 2).unwrap();
        assert_eq!((carried.state, carried.to.as_deref()), (RelayState::Routed, Some("sbxw")));
        assert!(b.carry(&r.id, 3).is_err(), "once");
        let done = b.approve(&r.id, Some("booted"), vec![], 4).unwrap();
        assert_eq!(agent_view(&done)["answer"], "booted");
        let q = b.open("a", None, RelayKind::Question, "q", 5);
        assert!(b.carry(&q.id, 6).is_err(), "only Mac requests");
    }

    #[test]
    fn a_round_for_the_person_is_answered_by_them_or_routed_for_a_draft() {
        let mut b = RelayBook::new();
        let own = b.open("a", None, RelayKind::Ask, "Q1?", 1);
        assert_eq!(b.approve(&own.id, Some("Q1 : oui"), vec![], 2).unwrap().state, RelayState::Approved);

        let r = b.open("a", Some("t1"), RelayKind::Ask, "Q1? Q2?", 3);
        assert!(b.route(&r.id, "a", 4).is_err(), "not to the asker");
        assert_eq!(b.route(&r.id, "b", 4).unwrap().state, RelayState::Routed);
        let drafted = b.reply(&r.id, "b", "Q1 : yes\nQ2 : no", 5).unwrap();
        assert_eq!(drafted.state, RelayState::Answered);
        assert_eq!(agent_view(&drafted)["answer"], serde_json::Value::Null, "the draft waits for the person");
        let released = b.approve(&r.id, Some("Q1 : yes\nQ2 : maybe"), vec![], 6).unwrap();
        assert_eq!(agent_view(&released)["answer"], "Q1 : yes\nQ2 : maybe");
    }

    #[test]
    fn only_the_sandbox_it_was_handed_reads_a_routed_request() {
        let mut b = RelayBook::new();
        let r = b.open("a", None, RelayKind::Ask, "Q1?", 1);
        assert!(b.check_target(&r.id, "b").is_err(), "not routed yet");
        b.route(&r.id, "b", 2).unwrap();
        assert!(b.check_target(&r.id, "b").is_ok());
        assert!(b.check_target(&r.id, "c").is_err(), "another sandbox");
        assert!(b.check_target(&r.id, "a").is_err(), "nor the asker");
        let unknown = b.check_target("r-999", "c").unwrap_err().replace("r-999", "ID");
        assert_eq!(b.check_target(&r.id, "c").unwrap_err().replace(&r.id, "ID"), unknown, "same refusal as an unknown id");
        b.reply(&r.id, "b", "Q1 : x", 3).unwrap();
        assert!(b.check_target(&r.id, "b").is_err(), "answered: in review, no longer readable");
        b.route(&r.id, "c", 4).unwrap();
        assert!(b.check_target(&r.id, "b").is_err(), "re-routed away");
        assert!(b.check_target(&r.id, "c").is_ok());
        b.approve(&r.id, Some("Q1 : y"), vec![], 5).unwrap();
        assert!(b.check_target(&r.id, "c").is_err(), "settled");
        let d = b.open("a", None, RelayKind::Question, "q", 6);
        b.route(&d.id, "b", 7).unwrap();
        b.deny(&d.id, None, 8).unwrap();
        assert!(b.check_target(&d.id, "b").is_err(), "denied");
    }

    #[test]
    fn waiting_is_for_the_asker() {
        let mut b = RelayBook::new();
        let r = b.open("a", None, RelayKind::Question, "q", 1);
        assert!(b.check_asker(&r.id, "a", None).is_ok());
        assert!(b.check_asker(&r.id, "a", Some(None)).is_ok());
        assert!(b.check_asker(&r.id, "b", None).is_err());
        let t = b.open("a", Some("t1"), RelayKind::Question, "q", 2);
        assert!(b.check_asker(&t.id, "a", Some(Some("t1"))).is_ok());
        assert!(b.check_asker(&t.id, "a", Some(Some("t2"))).is_err(), "another thread");
        assert!(b.check_asker(&t.id, "a", Some(None)).is_err(), "the agent beside it");
        assert!(b.check_asker(&r.id, "a", Some(Some("t1"))).is_err(), "a thread reading the agent's");
    }

    #[test]
    fn pruning_outlives_a_late_pickup_but_not_an_abandoned_request() {
        let mut b = RelayBook::new();
        let open = b.open("a", None, RelayKind::Question, "q", 0);
        let done = b.open("a", None, RelayKind::Question, "q", 0);
        b.deny(&done.id, None, 0).unwrap();
        assert_eq!(b.prune(SETTLED_TTL_MS - 1), 0);
        assert_eq!(b.prune(SETTLED_TTL_MS), 1);
        assert!(b.get(&open.id).is_some());
        let expired = b.prune_taken(OPEN_TTL_MS);
        assert_eq!(expired.len(), 1);
        assert_eq!((expired[0].id.as_str(), expired[0].state), (open.id.as_str(), RelayState::Pending), "the open one, returned to be told");
    }

    #[test]
    fn a_book_read_back_keeps_its_open_requests_and_never_reuses_an_id() {
        let mut b = RelayBook::new();
        let a = b.open("a", Some("t1"), RelayKind::Ask, "Q1?", 1);
        let s = b.open("a", None, RelayKind::Screenshot, "show", 2);
        b.approve(&s.id, Some("see"), vec![shot("AAAA")], 3).unwrap();
        b.mark_returned(&s.id, 4);
        assert!(b.take_dirty());
        let mut back = RelayBook::parse_lenient(&b.to_json()).unwrap();
        assert!(!back.take_dirty());
        assert_eq!(back.list(), b.list(), "everything, the images and the delivery state included");
        assert!(back.check_asker(&a.id, "a", Some(Some("t1"))).is_ok(), "the waiting thread finds its request");
        let next = back.open("a", None, RelayKind::Question, "q", 5);
        assert!(next.id != a.id && next.id != s.id, "{} reused", next.id);
        assert_eq!(next.id, "r-3");
        // A file whose counter was lost still never goes below an id it holds.
        let lost = b.to_json().replace("\"seq\":2", "\"seq\":0");
        assert_eq!(RelayBook::parse_lenient(&lost).unwrap().open("a", None, RelayKind::Question, "q", 6).id, "r-3");
        // One unreadable request is dropped, the rest kept.
        let mut v: serde_json::Value = serde_json::from_str(&b.to_json()).unwrap();
        v["requests"][0]["state"] = "weird".into();
        assert_eq!(RelayBook::parse_lenient(&v.to_string()).unwrap().list().len(), 1);
        assert!(RelayBook::parse_lenient("not json").is_none());
    }

    #[test]
    fn a_retried_open_with_the_same_key_is_the_same_request() {
        let mut b = RelayBook::new();
        let (r1, new1) = b.open_keyed("a", None, RelayKind::Question, "q", Some("k-1"), 1);
        let (r2, new2) = b.open_keyed("a", None, RelayKind::Question, "q", Some("k-1"), 2);
        assert!(new1 && !new2);
        assert_eq!(r1.id, r2.id);
        let (other, _) = b.open_keyed("b", None, RelayKind::Question, "q", Some("k-1"), 3);
        assert_ne!(other.id, r1.id, "a key is the asker's own");
        let (r3, new3) = b.open_keyed("a", None, RelayKind::Question, "q", Some("k-2"), 4);
        assert!(new3 && r3.id != r1.id);
        assert_eq!(b.list().len(), 3);
        let survived = RelayBook::parse_lenient(&b.to_json()).unwrap().open_keyed("a", None, RelayKind::Question, "q", Some("k-1"), 5);
        assert_eq!((survived.0.id, survived.1), (r1.id, false), "across a restart too");
    }

    #[test]
    fn a_late_failure_for_one_target_does_not_undo_a_routing_to_another() {
        let mut b = RelayBook::new();
        let r = b.open("a", None, RelayKind::Question, "q", 1);
        b.route(&r.id, "x", 2).unwrap();
        b.route(&r.id, "y", 3).unwrap();
        assert!(b.unroute(&r.id, "x", "could not reach x", 4).is_err());
        assert_eq!(b.get(&r.id).unwrap().to.as_deref(), Some("y"));
        let back = b.unroute(&r.id, "y", "could not reach y", 5).unwrap();
        assert_eq!((back.state, back.note.as_deref()), (RelayState::Pending, Some("could not reach y")));
    }

    #[test]
    fn an_outcome_is_owed_when_not_acknowledged_in_time_and_only_once() {
        let mut b = RelayBook::new();
        let r = b.open("a", None, RelayKind::Question, "q", 0);
        b.approve(&r.id, Some("42"), vec![], 10).unwrap();
        assert!(b.take_owed(10 + ACK_LIMIT_MS, |_| true).is_empty(), "a wait is parked on it");
        b.mark_returned(&r.id, 20);
        assert!(b.take_owed(20 + ACK_LIMIT_MS - 1, |_| false).is_empty(), "still within the limit");
        assert_eq!(b.take_owed(20 + ACK_LIMIT_MS, |_| false).len(), 1);
        assert!(b.take_owed(u64::MAX, |_| false).is_empty(), "once");
        assert!(!b.mark_delivered(&r.id), "already handed on");

        let acked = b.open("a", None, RelayKind::Question, "q", 0);
        b.approve(&acked.id, Some("x"), vec![], 1).unwrap();
        b.mark_returned(&acked.id, 2);
        assert!(b.mark_delivered(&acked.id));
        assert!(b.take_owed(u64::MAX, |_| false).is_empty(), "acknowledged: nothing owed");

        let open = b.open("a", None, RelayKind::Question, "q", 0);
        b.mark_returned(&open.id, 1);
        assert!(!b.mark_delivered(&open.id), "an open request is not delivered");
        assert!(b.take_owed(u64::MAX, |_| false).is_empty());
    }

    #[test]
    fn a_request_waiting_on_the_person_is_reminded_once() {
        let mut b = RelayBook::new();
        let r = b.open("a", None, RelayKind::Question, "q", 0);
        let routed = b.open("a", None, RelayKind::Question, "q", 0);
        b.route(&routed.id, "b", 0).unwrap();
        assert!(b.take_reminders(REMIND_AFTER_MS - 1).is_empty());
        let due = b.take_reminders(REMIND_AFTER_MS);
        assert_eq!(due.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), vec![r.id.as_str()], "not one with another sandbox");
        assert!(b.take_reminders(10 * REMIND_AFTER_MS).is_empty(), "once");
        b.reply(&routed.id, "b", "draft", 1).unwrap();
        assert_eq!(b.take_reminders(1 + REMIND_AFTER_MS).len(), 1, "a draft waiting for review");
        assert!(RelayBook::parse_lenient(&b.to_json()).unwrap().take_reminders(u64::MAX).is_empty(), "once, across a restart");
    }

    #[test]
    fn a_long_answer_comes_in_parts_and_nothing_is_lost() {
        let line = "Q1 : ".to_string() + &"x".repeat(95) + "\n";
        let long: String = line.repeat(300); // 30 000 characters
        let (first, parts) = answer_part(&long, 1);
        assert_eq!(parts, 3);
        assert!(first.ends_with('\n'), "cut at a line break");
        let whole: String = (1..=parts).map(|n| answer_part(&long, n).0).collect();
        assert_eq!(whole, long);
        assert_eq!(answer_part("short", 1), ("short", 1));
        assert_eq!(answer_part("short", 9), ("short", 1), "clamped");
        let wide = "é".repeat(PART_CHARS + 5);
        assert_eq!(answer_part(&wide, 2), ("ééééé", 2), "counted in characters, cut on one");

        let mut b = RelayBook::new();
        let r = b.open("a", None, RelayKind::Ask, "Q1?", 1);
        let done = b.approve(&r.id, Some(&long), vec![], 2).unwrap();
        let v = agent_view_part(&done, 2);
        assert_eq!((v["answer_part"].as_u64(), v["answer_parts"].as_u64()), (Some(2), Some(3)));
        assert_eq!(v["answer"], answer_part(&long, 2).0);
        assert_eq!(agent_view(&done)["answer_part"], 1);
        assert_eq!(done.answer_parts(), 3);
    }

    #[test]
    fn shots_are_checked_at_the_envelope() {
        assert!(parse_shot("data:image/png;base64,AAAA").is_ok());
        assert!(parse_shot("data:image/gif;base64,AAAA").is_err());
        assert!(parse_shot("data:image/png;base64,<script>").is_err());
        assert!(parse_shot("http://x").is_err());
        let seven: Vec<String> = (0..7).map(|_| "data:image/png;base64,AA".to_string()).collect();
        assert!(parse_shots(&seven).is_err());
        let big = format!("data:image/png;base64,{}", "A".repeat(MAX_SHOT_B64));
        assert!(parse_shots(&[big.clone(), big.clone(), big]).is_err(), "too heavy together");
    }
}
