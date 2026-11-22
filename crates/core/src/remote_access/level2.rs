//! Level 2 of remote access: approving from the phone with a passkey. The
//! rules, with time passed in (see `.sbxw-artifacts/remote/API-level2.md`).
//!
//! * [`Passkey`]: one per device, stored beside the devices (public data:
//!   a credential id, a public key, a counter, the RP ID it was made for).
//! * [`Enrolment`]: a phone asks to register; the Mac grants it by typing
//!   the six digits the phone shows; the grant is single use, 5 minutes.
//! * [`Challenges`]: ONE outstanding challenge per device, bound to what it
//!   is for (registration, or an action on a target with a digest), 60 s or
//!   2 minutes, taken (removed) by the first attempt that names its device.
//! * [`digest`]: SHA-256 of canonical JSON, so what was shown is what is
//!   approved.
//! * [`option_meaning`]: what a network-request option does, in the words
//!   the phone's confirmation shows, and whether it may leave a lasting rule.
//! * [`check_models`]: the model changes a phone may make to a plan.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const ENROL_TTL_MS: u64 = 5 * 60 * 1000;
pub const GRANT_TTL_MS: u64 = 5 * 60 * 1000;
pub const CREATE_TTL_MS: u64 = 2 * 60 * 1000;
pub const ACT_TTL_MS: u64 = 60 * 1000;
pub const MAX_CHECK_TRIES: u32 = 3;
/// Random bytes of a challenge (256 bits; the floor is 128).
pub const CHALLENGE_BYTES: usize = 32;

/// What a level 2 action does; also its journal kind's name.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    PlanApprove,
    PlanRefuse,
    Merge,
    NetworkRespond,
    SandboxRemove,
}

impl Action {
    pub fn code(self) -> &'static str {
        match self {
            Action::PlanApprove => "plan_approve",
            Action::PlanRefuse => "plan_refuse",
            Action::Merge => "merge",
            Action::NetworkRespond => "network_respond",
            Action::SandboxRemove => "sandbox_remove",
        }
    }
}

/// A device's passkey, as stored (`passkeys.json`, 0600).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Passkey {
    pub device_id: String,
    /// base64url.
    pub credential_id: String,
    /// base64url of the uncompressed SEC1 point.
    pub public_key: String,
    pub counter: u32,
    pub rp_id: String,
    pub created_ms: u64,
}

/// The passkeys file.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PasskeyFile {
    pub passkeys: Vec<Passkey>,
}

impl PasskeyFile {
    pub fn get(&self, device_id: &str) -> Option<&Passkey> {
        self.passkeys.iter().find(|p| p.device_id == device_id)
    }

    /// One per device: a new one replaces the device's earlier one.
    pub fn put(&mut self, p: Passkey) {
        self.passkeys.retain(|o| o.device_id != p.device_id);
        self.passkeys.push(p);
    }

    pub fn drop_device(&mut self, device_id: &str) -> bool {
        let n = self.passkeys.len();
        self.passkeys.retain(|p| p.device_id != device_id);
        n != self.passkeys.len()
    }

    /// Keep only passkeys whose device still exists; true when one went.
    pub fn retain_devices(&mut self, exists: impl Fn(&str) -> bool) -> bool {
        let n = self.passkeys.len();
        self.passkeys.retain(|p| exists(&p.device_id));
        n != self.passkeys.len()
    }

    pub fn set_counter(&mut self, device_id: &str, counter: u32) {
        if let Some(p) = self.passkeys.iter_mut().find(|p| p.device_id == device_id) {
            p.counter = counter;
        }
    }
}

// ── Enrolment: the phone asks, the Mac grants ────────────────────────────

#[derive(Clone, Debug, PartialEq, Eq)]
enum EnrolState {
    Pending { tries: u32 },
    Granted { until_ms: u64 },
    Refused,
}

#[derive(Clone, Debug)]
struct EnrolRequest {
    id: String,
    device_id: String,
    check: String,
    created_ms: u64,
    expires_ms: u64,
    state: EnrolState,
}

/// What the Mac lists of a device's request (never its digits).
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct EnrolView {
    pub id: String,
    pub created_ms: u64,
    pub expires_ms: u64,
}

/// What a phone is told about its own request.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EnrolStatus {
    None,
    Pending,
    Granted,
    Refused,
}

#[derive(Debug, PartialEq, Eq)]
pub enum AllowError {
    /// No pending request for that device.
    Unknown,
    WrongCheck { left: u32 },
    /// The last try was wrong: the request is refused.
    Refused,
}

#[derive(Debug, Default)]
pub struct Enrolment {
    requests: Vec<EnrolRequest>,
}

/// Six random digits.
fn six_digits() -> String {
    let mut b = [0u8; 8];
    getrandom::getrandom(&mut b).expect("the OS has no randomness to give");
    format!("{:06}", u64::from_be_bytes(b) % 1_000_000)
}

impl Enrolment {
    fn sweep(&mut self, now: u64) {
        self.requests.retain(|r| match r.state {
            EnrolState::Granted { until_ms } => now <= until_ms,
            _ => now <= r.expires_ms,
        });
    }

    /// A phone asks: a new request for its device, voiding the device's
    /// earlier request and any unused grant. `(request id, digits, expiry)`.
    pub fn start(&mut self, device_id: &str, now: u64) -> (String, String, u64) {
        self.requests.retain(|r| r.device_id != device_id);
        let r = EnrolRequest { id: crate::ids::random_hex(16), device_id: device_id.into(), check: six_digits(), created_ms: now, expires_ms: now + ENROL_TTL_MS, state: EnrolState::Pending { tries: 0 } };
        let out = (r.id.clone(), r.check.clone(), r.expires_ms);
        self.requests.push(r);
        out
    }

    /// The device's ONE request, if `request_id` is its id (compared in
    /// constant time). Another holder of the device's cookie never learns
    /// the id, so it can neither read this request's state nor spend it.
    fn mine(&self, device_id: &str, request_id: &str) -> Option<&EnrolRequest> {
        self.requests.iter().find(|r| r.device_id == device_id).filter(|r| crate::ids::same_secret(&r.id, request_id))
    }

    /// The state of the request `request_id` of this device: `None` for any
    /// other id.
    pub fn status(&mut self, device_id: &str, request_id: &str, now: u64) -> (EnrolStatus, Option<u64>) {
        self.sweep(now);
        match self.mine(device_id, request_id) {
            None => (EnrolStatus::None, None),
            Some(r) => match r.state {
                EnrolState::Pending { .. } => (EnrolStatus::Pending, Some(r.expires_ms)),
                EnrolState::Granted { until_ms } => (EnrolStatus::Granted, Some(until_ms)),
                EnrolState::Refused => (EnrolStatus::Refused, None),
            },
        }
    }

    /// The device's pending request, for the Mac.
    pub fn pending(&mut self, device_id: &str, now: u64) -> Option<EnrolView> {
        self.sweep(now);
        self.requests
            .iter()
            .find(|r| r.device_id == device_id && matches!(r.state, EnrolState::Pending { .. }))
            .map(|r| EnrolView { id: r.id.clone(), created_ms: r.created_ms, expires_ms: r.expires_ms })
    }

    /// The live grant of request `request_id`, if any: until when.
    pub fn granted(&mut self, device_id: &str, request_id: &str, now: u64) -> Option<u64> {
        self.sweep(now);
        match self.mine(device_id, request_id)?.state {
            EnrolState::Granted { until_ms } => Some(until_ms),
            _ => None,
        }
    }

    /// The device's live grant, if any: until when (for the Mac's status).
    pub fn granted_until(&mut self, device_id: &str, now: u64) -> Option<u64> {
        self.sweep(now);
        self.requests.iter().find_map(|r| match r.state {
            EnrolState::Granted { until_ms } if r.device_id == device_id => Some(until_ms),
            _ => None,
        })
    }

    /// The Mac grants with the digits the phone shows. Ok: the grant's end.
    pub fn allow(&mut self, device_id: &str, check: &str, now: u64) -> Result<u64, AllowError> {
        self.sweep(now);
        let r = self.requests.iter_mut().find(|r| r.device_id == device_id && matches!(r.state, EnrolState::Pending { .. })).ok_or(AllowError::Unknown)?;
        let EnrolState::Pending { tries } = r.state else { return Err(AllowError::Unknown) };
        if crate::ids::same_secret(&r.check, check.trim()) {
            let until = now + GRANT_TTL_MS;
            r.state = EnrolState::Granted { until_ms: until };
            return Ok(until);
        }
        let tries = tries + 1;
        if tries >= MAX_CHECK_TRIES {
            r.state = EnrolState::Refused;
            return Err(AllowError::Refused);
        }
        r.state = EnrolState::Pending { tries };
        Err(AllowError::WrongCheck { left: MAX_CHECK_TRIES - tries })
    }

    pub fn refuse(&mut self, device_id: &str, now: u64) -> bool {
        self.sweep(now);
        match self.requests.iter_mut().find(|r| r.device_id == device_id && matches!(r.state, EnrolState::Pending { .. })) {
            Some(r) => {
                r.state = EnrolState::Refused;
                true
            }
            None => false,
        }
    }

    /// Spend the grant of request `request_id` (at registration, whatever
    /// its outcome). Another id spends nothing.
    pub fn take_grant(&mut self, device_id: &str, request_id: &str, now: u64) -> bool {
        self.sweep(now);
        let had = self.mine(device_id, request_id).is_some_and(|r| matches!(r.state, EnrolState::Granted { .. }));
        if had {
            self.requests.retain(|r| r.device_id != device_id);
        }
        had
    }

    pub fn drop_device(&mut self, device_id: &str) {
        self.requests.retain(|r| r.device_id != device_id);
    }

    pub fn clear(&mut self) {
        self.requests.clear();
    }
}

// ── Challenges ───────────────────────────────────────────────────────────

/// What a challenge was made for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Purpose {
    /// A registration, for the enrolment request with this id.
    Register { request: String },
    Act {
        action: Action,
        target: String,
        digest: String,
        option: Option<String>,
        /// A plan approval: the model the phone chose per thread (thread
        /// name to model id); empty for the proposed models and any other
        /// action. The act must send exactly these.
        models: BTreeMap<String, String>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Challenge {
    pub bytes: Vec<u8>,
    pub purpose: Purpose,
    pub expires_ms: u64,
}

/// One outstanding challenge per device.
#[derive(Debug, Default)]
pub struct Challenges {
    by_device: BTreeMap<String, Challenge>,
}

impl Challenges {
    /// A new challenge for `device_id`, voiding its last one.
    pub fn issue(&mut self, device_id: &str, purpose: Purpose, now: u64) -> Challenge {
        let mut bytes = vec![0u8; CHALLENGE_BYTES];
        getrandom::getrandom(&mut bytes).expect("the OS has no randomness to give");
        let ttl = if matches!(purpose, Purpose::Register { .. }) { CREATE_TTL_MS } else { ACT_TTL_MS };
        let c = Challenge { bytes, purpose, expires_ms: now + ttl };
        self.by_device.insert(device_id.to_string(), c.clone());
        c
    }

    /// Take the device's challenge only if it is a registration challenge
    /// for request `request_id` (compared in constant time): a call that
    /// does not know the id leaves it in place.
    pub fn take_register(&mut self, device_id: &str, request_id: &str, now: u64) -> Option<Challenge> {
        let fits = matches!(self.by_device.get(device_id), Some(Challenge { purpose: Purpose::Register { request }, .. }) if crate::ids::same_secret(request, request_id));
        if fits {
            self.take(device_id, now)
        } else {
            None
        }
    }

    /// Take (remove) the device's challenge: every attempt consumes it,
    /// whatever follows. `None` when it has none or it expired.
    pub fn take(&mut self, device_id: &str, now: u64) -> Option<Challenge> {
        self.by_device.remove(device_id).filter(|c| now <= c.expires_ms)
    }

    pub fn drop_device(&mut self, device_id: &str) {
        self.by_device.remove(device_id);
    }

    pub fn clear(&mut self) {
        self.by_device.clear();
    }

    pub fn len(&self) -> usize {
        self.by_device.len()
    }

    /// Tests: move a device's challenge's expiry.
    #[cfg(any(test, feature = "test-authenticator"))]
    pub fn age_for_tests(&mut self, device_id: &str, expires_ms: u64) {
        if let Some(c) = self.by_device.get_mut(device_id) {
            c.expires_ms = expires_ms;
        }
    }

    pub fn is_empty(&self) -> bool {
        self.by_device.is_empty()
    }
}

/// Whether a taken act challenge was made for exactly this action, target
/// and option.
pub fn challenge_fits(c: &Challenge, action: Action, target: &str, option: Option<&str>) -> bool {
    match &c.purpose {
        Purpose::Act { action: a, target: t, option: o, .. } => *a == action && t == target && o.as_deref() == option,
        Purpose::Register { .. } => false,
    }
}

// ── Digests ──────────────────────────────────────────────────────────────

/// SHA-256 (hex) of `v` as JSON. `serde_json` keeps object keys sorted
/// (no `preserve_order`), so the text is canonical for one value.
pub fn digest(v: &serde_json::Value) -> String {
    use sha2::{Digest, Sha256};
    let bytes = serde_json::to_vec(v).expect("a JSON value serialises");
    Sha256::digest(&bytes).iter().map(|b| format!("{b:02x}")).collect()
}

/// A digest as a phone sends it back: 64 lower-case hex.
pub fn valid_digest(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

// ── Network options ──────────────────────────────────────────────────────

/// What answering a network request with one option does, as the phone's
/// confirmation says it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct OptionMeaning {
    /// It may leave a rule behind that outlives this request: the sheet
    /// flags it.
    pub lasting: bool,
    /// One plain sentence.
    pub effect: String,
}

/// What option `id` of a network request of `sandbox` for `host` does.
/// A phone may answer with any option sbx offers (each behind Face ID);
/// this only words it. sbx 0.51 offers two, checked against its real
/// output: `dismiss` closes the request and writes no rule; `allow` writes
/// a LASTING rule. Evidence (sbx 0.51, verified on the person's Mac,
/// 2026-11-22): after allowing index.crates.io, static.crates.io,
/// crates.io and proxy.golang.org, `sbx policy inspect` showed a `local`
/// policy applying to `sandbox:sbxw-2` with one `allow <host>:443 network
/// tcp active` rule each, editable, and removable with `sbx policy rm
/// network --id <rule-id> --sandbox sbxw-2`. By id, exactly (review P4:
/// never a word found inside an id or a label); any other id is sbx's own,
/// worded cautiously and flagged as lasting (fail safe).
pub fn option_meaning(id: &str, label: &str, sandbox: &str, host: &str) -> OptionMeaning {
    match id {
        "dismiss" => OptionMeaning { lasting: false, effect: format!("Closes this request and writes no rule: {sandbox} stays unable to reach {host}, and sbx asks again if it tries again.") },
        "allow" => OptionMeaning {
            lasting: true,
            effect: format!("Adds a lasting network rule to {sandbox}: {sandbox} may reach {host} from now on, until you remove the rule on your Mac (sbx policy rm network --id <rule id> --sandbox {sandbox})."),
        },
        _ => {
            let label = if label.is_empty() { id } else { label };
            OptionMeaning { lasting: true, effect: format!("sbx's own option \u{201c}{label}\u{201d} for {sandbox} and {host}. sbxw does not know what it leaves behind: it may add a lasting network rule to {sandbox}.") }
        }
    }
}

// ── Models of a plan approved from the phone ─────────────────────────────

/// Model changes a phone sends with a plan approval, at most.
pub const MAX_MODEL_CHANGES: usize = 32;

/// Check a phone's model changes for a plan: each names one of the plan's
/// `threads` and a model id of `allowed` (the configured list, the one
/// `GET /r/models` serves). Only the model of a thread changes, nothing else.
pub fn check_models(models: &BTreeMap<String, String>, threads: &[&str], allowed: &[&str]) -> Result<(), String> {
    if models.len() > MAX_MODEL_CHANGES {
        return Err("models: too many".into());
    }
    for (t, m) in models {
        if !threads.contains(&t.as_str()) {
            return Err(format!("models: \u{201c}{t}\u{201d} is not a thread of this plan"));
        }
        if !allowed.contains(&m.as_str()) {
            return Err(format!("models: \u{201c}{m}\u{201d} is not in the list of models"));
        }
    }
    Ok(())
}

/// A plan id as the routes take it: `p-` and 32 hex.
pub fn valid_plan_id(s: &str) -> bool {
    s.len() == 34 && s.starts_with("p-") && s[2..].bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// A network request id as the routes take it: `network:` and 1 to 64 of
/// `[A-Za-z0-9_-]`.
pub fn valid_network_id(s: &str) -> bool {
    s.strip_prefix("network:").is_some_and(|r| !r.is_empty() && r.len() <= 64 && r.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_grant_needs_the_phones_digits_and_is_single_use() {
        let mut e = Enrolment::default();
        assert_eq!(e.allow("d1", "000000", 0), Err(AllowError::Unknown), "no request, no grant");
        let (rid, check, exp) = e.start("d1", 0);
        assert_eq!(exp, ENROL_TTL_MS);
        assert_eq!(e.status("d1", &rid, 1).0, EnrolStatus::Pending);
        assert_eq!(e.status("d1", "0".repeat(32).as_str(), 1).0, EnrolStatus::None, "another id learns nothing");
        assert_eq!(e.allow("d2", &check, 1), Err(AllowError::Unknown), "another device's request");
        let wrong = if check == "000000" { "111111" } else { "000000" };
        assert_eq!(e.allow("d1", wrong, 1), Err(AllowError::WrongCheck { left: 2 }));
        assert_eq!(e.allow("d1", &check, 2), Ok(2 + GRANT_TTL_MS));
        assert_eq!(e.status("d1", &rid, 3).0, EnrolStatus::Granted);
        assert_eq!(e.granted("d1", "f".repeat(32).as_str(), 3), None, "the grant is the request's, not the device's");
        assert!(!e.take_grant("d1", "f".repeat(32).as_str(), 3), "another id spends nothing");
        assert!(!e.take_grant("d2", &rid, 3));
        assert!(e.take_grant("d1", &rid, 3));
        assert!(!e.take_grant("d1", &rid, 3), "single use");
        assert_eq!(e.status("d1", &rid, 3).0, EnrolStatus::None);
    }

    #[test]
    fn grants_and_requests_expire_and_three_wrong_tries_refuse() {
        let mut e = Enrolment::default();
        let (_, check, _) = e.start("d1", 0);
        assert_eq!(e.allow("d1", &check, ENROL_TTL_MS + 1), Err(AllowError::Unknown), "expired request");
        let (rid, check, _) = e.start("d1", 0);
        e.allow("d1", &check, 10).unwrap();
        assert!(!e.take_grant("d1", &rid, 10 + GRANT_TTL_MS + 1), "expired grant");
        let (rid, check, _) = e.start("d1", 0);
        let wrong = if check == "000000" { "111111" } else { "000000" };
        let _ = e.allow("d1", wrong, 1);
        let _ = e.allow("d1", wrong, 1);
        assert_eq!(e.allow("d1", wrong, 1), Err(AllowError::Refused));
        assert_eq!(e.allow("d1", &check, 1), Err(AllowError::Unknown), "refused for good");
        assert_eq!(e.status("d1", &rid, 1).0, EnrolStatus::Refused);
    }

    #[test]
    fn a_new_request_voids_the_old_one_and_its_grant() {
        let mut e = Enrolment::default();
        let (r1, c1, _) = e.start("d1", 0);
        e.allow("d1", &c1, 1).unwrap();
        let (_r2, c2, _) = e.start("d1", 2);
        assert!(!e.take_grant("d1", &r1, 3), "the grant went with the old request");
        if c1 != c2 {
            assert!(e.allow("d1", &c1, 3).is_err(), "old digits");
        }
        assert!(e.allow("d1", &c2, 3).is_ok());
    }

    #[test]
    fn challenges_are_one_per_device_taken_once_and_expire() {
        let mut c = Challenges::default();
        let act = |t: &str| Purpose::Act { action: Action::Merge, target: t.into(), digest: "d".into(), option: None, models: BTreeMap::new() };
        let a = c.issue("d1", act("p1"), 0);
        assert_eq!(a.bytes.len(), CHALLENGE_BYTES);
        let b = c.issue("d1", act("p2"), 0);
        assert_ne!(a.bytes, b.bytes);
        assert_eq!(c.len(), 1, "a new challenge voids the last");
        assert!(c.take("d2", 1).is_none(), "another device's");
        let got = c.take("d1", 1).unwrap();
        assert!(challenge_fits(&got, Action::Merge, "p2", None));
        assert!(!challenge_fits(&got, Action::Merge, "p1", None), "another target");
        assert!(!challenge_fits(&got, Action::PlanApprove, "p2", None), "another action");
        assert!(!challenge_fits(&got, Action::Merge, "p2", Some("x")), "another option");
        assert!(c.take("d1", 1).is_none(), "taken once");
        c.issue("d1", act("p1"), 0);
        assert!(c.take("d1", ACT_TTL_MS + 1).is_none(), "expired, and gone");
        let r = c.issue("d1", Purpose::Register { request: "a".repeat(32) }, 0);
        assert_eq!(r.expires_ms, CREATE_TTL_MS);
        assert!(!challenge_fits(&r, Action::Merge, "p1", None));
        assert!(c.take_register("d1", &"b".repeat(32), 1).is_none(), "another request id takes nothing");
        assert_eq!(c.len(), 1, "and leaves it in place");
        assert!(c.take_register("d1", &"a".repeat(32), 1).is_some());
    }

    #[test]
    fn digests_are_canonical() {
        let a = serde_json::json!({"b": 1, "a": [1, 2]});
        let b: serde_json::Value = serde_json::from_str(r#"{"a":[1,2],"b":1}"#).unwrap();
        assert_eq!(digest(&a), digest(&b));
        assert_ne!(digest(&a), digest(&serde_json::json!({"b": 1, "a": [2, 1]})));
        assert!(valid_digest(&digest(&a)));
        assert!(!valid_digest("ABC"));
        assert!(!valid_digest(&digest(&a).to_uppercase()));
    }

    #[test]
    fn each_option_is_worded_by_its_exact_id_and_unknown_ones_are_lasting() {
        let d = option_meaning("dismiss", "Dismiss", "sbxw-2", "storage.googleapis.com:443");
        assert!(!d.lasting);
        assert!(d.effect.contains("writes no rule") && d.effect.contains("sbxw-2"));
        let a = option_meaning("allow", "Allow", "sbxw-2", "storage.googleapis.com:443");
        assert!(a.lasting, "sbx 0.51's Allow writes a lasting rule");
        assert!(a.effect.starts_with("Adds a lasting network rule to sbxw-2:") && a.effect.contains("storage.googleapis.com:443") && a.effect.contains("sbx policy rm"));
        assert!(!a.effect.contains("may add"), "no hedge for a checked id");
        for (id, label) in [("allow-always", "Allow always"), ("Dismiss", "Dismiss"), ("dismiss ", "Dismiss"), ("x", "dismiss"), ("deny", "Deny")] {
            assert!(option_meaning(id, label, "s", "h").lasting, "{id:?} {label:?}: unknown, so flagged");
        }
        assert!(option_meaning("x", "", "s", "h").effect.contains("\u{201c}x\u{201d}"), "no label: the id is shown");
    }

    #[test]
    fn model_changes_name_a_thread_of_the_plan_and_a_listed_model() {
        let m = |pairs: &[(&str, &str)]| pairs.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect::<BTreeMap<_, _>>();
        let threads = ["alpha", "beta"];
        let allowed = ["claude-opus-5-5", "claude-sonnet-5-5"];
        assert!(check_models(&m(&[]), &threads, &allowed).is_ok());
        assert!(check_models(&m(&[("alpha", "claude-sonnet-5-5")]), &threads, &allowed).is_ok());
        assert!(check_models(&m(&[("gamma", "claude-sonnet-5-5")]), &threads, &allowed).is_err(), "not a thread");
        assert!(check_models(&m(&[("alpha", "gpt-x")]), &threads, &allowed).is_err(), "not listed");
        assert!(check_models(&m(&[("alpha", "claude-sonnet-5-5 ")]), &threads, &allowed).is_err(), "exactly");
    }

    #[test]
    fn ids_as_routes_take_them() {
        assert!(valid_plan_id(&crate::ids::plan_id()));
        for bad in ["p-", "p-xyz", "P-0123456789abcdef0123456789abcdef", "p-0123456789ABCDEF0123456789abcdef", "p-0123456789abcdef0123456789abcde"] {
            assert!(!valid_plan_id(bad), "{bad}");
        }
        assert!(valid_network_id("network:85TMuE6"));
        for bad in ["network:", "port:abc", "network:a/b", "network:a b", "network:..", "network:%2F"] {
            assert!(!valid_network_id(bad), "{bad}");
        }
    }

    #[test]
    fn passkeys_are_one_per_device_and_follow_their_device() {
        let p = |d: &str, c: &str| Passkey { device_id: d.into(), credential_id: c.into(), public_key: "k".into(), counter: 0, rp_id: "a.b".into(), created_ms: 0 };
        let mut f = PasskeyFile::default();
        f.put(p("d1", "c1"));
        f.put(p("d1", "c2"));
        f.put(p("d2", "c3"));
        assert_eq!(f.passkeys.len(), 2);
        assert_eq!(f.get("d1").unwrap().credential_id, "c2");
        assert!(f.retain_devices(|d| d == "d2"));
        assert!(f.get("d1").is_none());
        assert!(serde_json::from_str::<PasskeyFile>(r#"{"passkeys":[],"secret":"x"}"#).is_err(), "unknown fields refused");
    }
}
