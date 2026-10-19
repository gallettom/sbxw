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
//!   agent tokens (see the daemon) "a sandbox" is now an identity the daemon
//!   verified, not one the request claims.
//!
//! This is the state machine only: no clock, no channels, no I/O. Every
//! transition takes `now` and returns the request as it now stands, or why not.

use serde::Serialize;
use std::collections::HashMap;

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Debug)]
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

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Debug, Default)]
#[serde(rename_all = "lowercase")]
pub enum RelayKind {
    #[default]
    Question,
    Screenshot,
}

/// One attached image, as the base64 the browser sent. Never decoded here:
/// the daemon is a courier, and the sandbox that asked decodes it anyway.
#[derive(Clone, Debug, PartialEq)]
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
}

/// How long a settled request is kept for a late `wait` (30 min).
pub const SETTLED_TTL_MS: u64 = 30 * 60 * 1000;
/// How long an open request waits for a human (6 h).
pub const OPEN_TTL_MS: u64 = 6 * 60 * 60 * 1000;
pub const MAX_QUESTION: usize = 4000;
pub const MAX_ANSWER: usize = 16000;
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
    let released = req.state == RelayState::Approved;
    let images: Vec<serde_json::Value> = if released {
        req.shots
            .iter()
            .map(|s| serde_json::json!({ "mime": s.mime, "b64": s.b64 }))
            .collect()
    } else {
        Vec::new()
    };
    serde_json::json!({
        "id": req.id,
        "kind": req.kind,
        "state": req.state,
        "to": req.to,
        "note": req.note,
        "answer": released.then(|| req.answer.clone()).flatten(),
        "images": images,
        // The first again, alone, for a relay CLI from before images were a list.
        "image": images.first(),
    })
}

/// Every live request.
#[derive(Default, Debug)]
pub struct RelayBook {
    requests: HashMap<String, RelayRequest>,
    seq: u64,
}

impl RelayBook {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn open(&mut self, from: &str, from_thread: Option<&str>, kind: RelayKind, question: &str, now: u64) -> RelayRequest {
        self.seq += 1;
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
        };
        self.requests.insert(req.id.clone(), req.clone());
        req
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

    /// Delivery failed: back to the human, saying why.
    pub fn unroute(&mut self, id: &str, why: &str, now: u64) -> Result<RelayRequest, String> {
        self.mutate(id, now, false, |req| {
            if req.state != RelayState::Routed {
                return Err(format!("request {id} is no longer being routed"));
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

    /// A human releases what the asker gets. Something has to be going out;
    /// which of the two payloads is the human's business. `shots` is the whole
    /// set as they left it.
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

    /// Waiting is reading: only the asker may.
    pub fn check_asker(&self, id: &str, from: &str) -> Result<&RelayRequest, String> {
        let req = self
            .requests
            .get(id)
            .ok_or_else(|| format!("no request '{id}' — it may have expired, or never existed"))?;
        if req.from != from {
            return Err(format!("request {id} was not opened by '{from}'"));
        }
        Ok(req)
    }

    /// Forget requests nobody can act on any more. Returns how many.
    pub fn prune(&mut self, now: u64) -> usize {
        let before = self.requests.len();
        self.requests.retain(|_, r| {
            let age = now.saturating_sub(r.updated_ms);
            age < if r.state.is_final() { SETTLED_TTL_MS } else { OPEN_TTL_MS }
        });
        before - self.requests.len()
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
    fn waiting_is_for_the_asker() {
        let mut b = RelayBook::new();
        let r = b.open("a", None, RelayKind::Question, "q", 1);
        assert!(b.check_asker(&r.id, "a").is_ok());
        assert!(b.check_asker(&r.id, "b").is_err());
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
        assert_eq!(b.prune(OPEN_TTL_MS), 1);
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
