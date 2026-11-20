//! Pairing a phone: a one-time code the Mac shows, a request the phone makes
//! with it, a six-digit check both screens show, the person's yes on the Mac.
//!
//! * One code at a time: a new one voids the last. 192 bits, 5 minutes,
//!   consumed by the first `start` that presents it, whatever happens next.
//! * A request lives 5 minutes from `start`. Approved, it is spent by the
//!   phone's next status poll, which is when the device and its token are
//!   made: a token never waits in memory for a phone that went away.
//! * Every failure a phone can see is one answer ("not valid", "expired"),
//!   so nothing tells a wrong code from a used one.

use serde::Serialize;

pub const CODE_TTL_MS: u64 = 5 * 60 * 1000;
pub const REQUEST_TTL_MS: u64 = 5 * 60 * 1000;
/// Requests pending at once; past it `start` is refused.
pub const MAX_PENDING: usize = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
enum ReqState {
    Pending,
    Approved,
    Refused,
}

#[derive(Clone, Debug)]
struct Request {
    id: String,
    device_name: String,
    check: String,
    created_ms: u64,
    expires_ms: u64,
    state: ReqState,
}

/// What the Mac lists.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct RequestView {
    pub id: String,
    pub device_name: String,
    pub check: String,
    pub created_ms: u64,
    pub expires_ms: u64,
}

/// What a phone is told when it polls.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    Pending,
    Refused,
    Expired,
    /// The person said yes: make the device now (the request is spent).
    Approved { device_name: String },
}

#[derive(Debug, PartialEq, Eq)]
pub enum StartError {
    /// Wrong, used, expired or superseded: one answer for all.
    InvalidCode,
    TooMany,
}

#[derive(Debug, Default)]
pub struct PairingBook {
    code: Option<(String, u64)>,
    requests: Vec<Request>,
}

/// The six digits both screens show: SHA-256 of the request id and the code,
/// reduced. Not a secret (it is on the phone's screen); it lets the person see
/// that the request on the Mac is the one from the phone in their hand.
pub fn check_of(request_id: &str, code: &str) -> String {
    use sha2::{Digest, Sha256};
    let d = Sha256::digest(format!("sbxw-pair-check\0{request_id}\0{code}").as_bytes());
    let n = u64::from_be_bytes(d[..8].try_into().expect("8 bytes")) % 1_000_000;
    format!("{n:06}")
}

impl PairingBook {
    pub fn new() -> Self {
        Self::default()
    }

    fn sweep(&mut self, now: u64) {
        if self.code.as_ref().is_some_and(|(_, exp)| now > *exp) {
            self.code = None;
        }
        self.requests.retain(|r| now <= r.expires_ms);
    }

    /// A new code, voiding the last; returns it and when it expires.
    pub fn new_code(&mut self, now: u64) -> (String, u64) {
        let code = crate::ids::random_hex(24);
        let exp = now + CODE_TTL_MS;
        self.code = Some((code.clone(), exp));
        (code, exp)
    }

    /// A phone presents `code`. The code is spent on a match.
    pub fn start(&mut self, code: &str, device_name: &str, now: u64) -> Result<RequestView, StartError> {
        self.sweep(now);
        let matches = self.code.as_ref().is_some_and(|(c, _)| crate::ids::same_secret(c, code));
        if !matches {
            return Err(StartError::InvalidCode);
        }
        let (code, _) = self.code.take().expect("matched");
        if self.requests.iter().filter(|r| r.state == ReqState::Pending).count() >= MAX_PENDING {
            return Err(StartError::TooMany);
        }
        let id = crate::ids::random_hex(16);
        let r = Request { check: check_of(&id, &code), id, device_name: device_name.to_string(), created_ms: now, expires_ms: now + REQUEST_TTL_MS, state: ReqState::Pending };
        let view = view(&r);
        self.requests.push(r);
        Ok(view)
    }

    /// A phone polls its request.
    pub fn status(&mut self, id: &str, now: u64) -> Status {
        self.sweep(now);
        let Some(i) = self.requests.iter().position(|r| crate::ids::same_secret(&r.id, id)) else {
            return Status::Expired;
        };
        match self.requests[i].state {
            ReqState::Pending => Status::Pending,
            ReqState::Refused => Status::Refused,
            ReqState::Approved => {
                let r = self.requests.remove(i);
                Status::Approved { device_name: r.device_name }
            }
        }
    }

    /// The person approves on the Mac; false when there is no such pending one.
    pub fn approve(&mut self, id: &str, now: u64) -> bool {
        self.decide(id, now, ReqState::Approved).is_some()
    }

    /// The person refuses on the Mac; the device name, for the journal.
    pub fn refuse(&mut self, id: &str, now: u64) -> Option<String> {
        self.decide(id, now, ReqState::Refused)
    }

    fn decide(&mut self, id: &str, now: u64, to: ReqState) -> Option<String> {
        self.sweep(now);
        let r = self.requests.iter_mut().find(|r| r.id == id && r.state == ReqState::Pending)?;
        r.state = to;
        Some(r.device_name.clone())
    }

    pub fn pending(&mut self, now: u64) -> Vec<RequestView> {
        self.sweep(now);
        self.requests.iter().filter(|r| r.state == ReqState::Pending).map(view).collect()
    }

    /// Remote access was turned off: no code, no request survives.
    pub fn clear(&mut self) {
        self.code = None;
        self.requests.clear();
    }
}

fn view(r: &Request) -> RequestView {
    RequestView { id: r.id.clone(), device_name: r.device_name.clone(), check: r.check.clone(), created_ms: r.created_ms, expires_ms: r.expires_ms }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_code_is_long_single_use_and_expires() {
        let mut b = PairingBook::new();
        let (code, exp) = b.new_code(0);
        assert_eq!(code.len(), 48, "192 bits");
        assert_eq!(exp, CODE_TTL_MS);
        assert_eq!(b.start("wrong", "p", 1), Err(StartError::InvalidCode));
        let r = b.start(&code, "phone", 1).unwrap();
        assert_eq!(b.start(&code, "phone", 2), Err(StartError::InvalidCode), "replay");
        assert_eq!(r.check, check_of(&r.id, &code));
        assert_eq!(r.check.len(), 6);
        assert!(r.check.bytes().all(|c| c.is_ascii_digit()));

        let (code, _) = b.new_code(0);
        assert_eq!(b.start(&code, "late", CODE_TTL_MS + 1), Err(StartError::InvalidCode), "expired");

        let (old, _) = b.new_code(0);
        let (new, _) = b.new_code(0);
        assert_eq!(b.start(&old, "p", 1), Err(StartError::InvalidCode), "superseded");
        assert!(b.start(&new, "p", 1).is_ok());
    }

    #[test]
    fn two_phones_racing_one_wins() {
        let mut b = PairingBook::new();
        let (code, _) = b.new_code(0);
        assert!(b.start(&code, "mine", 1).is_ok());
        assert_eq!(b.start(&code, "theirs", 1), Err(StartError::InvalidCode));
        assert_eq!(b.pending(1).len(), 1);
        assert_eq!(b.pending(1)[0].device_name, "mine");
    }

    #[test]
    fn approve_is_spent_by_one_poll() {
        let mut b = PairingBook::new();
        let (code, _) = b.new_code(0);
        let r = b.start(&code, "phone", 1).unwrap();
        assert_eq!(b.status(&r.id, 2), Status::Pending);
        assert_eq!(b.status("unknown", 2), Status::Expired);
        assert!(b.approve(&r.id, 3));
        assert!(!b.approve(&r.id, 3), "once");
        assert_eq!(b.status(&r.id, 4), Status::Approved { device_name: "phone".into() });
        assert_eq!(b.status(&r.id, 5), Status::Expired, "the token is minted once");
    }

    #[test]
    fn refuse_and_request_expiry() {
        let mut b = PairingBook::new();
        let (code, _) = b.new_code(0);
        let r = b.start(&code, "phone", 1).unwrap();
        assert_eq!(b.refuse(&r.id, 2).as_deref(), Some("phone"));
        assert!(!b.approve(&r.id, 2), "a refusal is final");
        assert_eq!(b.status(&r.id, 3), Status::Refused);
        assert_eq!(b.status(&r.id, 1 + REQUEST_TTL_MS + 1), Status::Expired);

        let (code, _) = b.new_code(0);
        let r = b.start(&code, "phone", 1).unwrap();
        assert!(!b.approve(&r.id, 1 + REQUEST_TTL_MS + 1), "too late to approve");
        assert_eq!(b.status(&r.id, 1 + REQUEST_TTL_MS + 1), Status::Expired);
    }

    #[test]
    fn clear_voids_everything() {
        let mut b = PairingBook::new();
        let (code, _) = b.new_code(0);
        let r = b.start(&code, "p", 1).unwrap();
        let (code2, _) = b.new_code(0);
        b.clear();
        assert_eq!(b.status(&r.id, 2), Status::Expired);
        assert_eq!(b.start(&code2, "p", 2), Err(StartError::InvalidCode));
    }

    #[test]
    fn pending_requests_are_capped() {
        let mut b = PairingBook::new();
        for _ in 0..MAX_PENDING {
            let (code, _) = b.new_code(0);
            b.start(&code, "p", 1).unwrap();
        }
        let (code, _) = b.new_code(0);
        assert_eq!(b.start(&code, "p", 1), Err(StartError::TooMany));
    }

    #[test]
    fn the_check_depends_on_request_and_code() {
        assert_ne!(check_of("a", "c"), check_of("b", "c"));
        assert_ne!(check_of("a", "c"), check_of("a", "d"));
        assert_eq!(check_of("a", "c"), check_of("a", "c"));
    }
}
