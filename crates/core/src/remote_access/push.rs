//! Web Push to a paired phone (remote-access-spec.md, "Notifications: the
//! person's answers"; the pipe is `.sbxw-artifacts/remote/API-push.md`).
//!
//! The rules, with time passed in: what a subscription may be (the same
//! allow-list as the sidecar's `checkEndpoint`), the Mac's VAPID key, the
//! pipe's lines (the configuration line, a job, an answer), the edges that
//! cause a push and the per-device limits. Nothing here does I/O.
//!
//! Never logged, never journaled, never in a `Debug` output: the endpoint,
//! the phone's keys, the VAPID private key.

use super::webauthn::{b64url_decode, b64url_encode};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap, VecDeque};

/// The `sub` claim of every VAPID token (D5): the public project URL, the
/// same for every install, so no personal address goes to Apple.
pub const SUB_URL: &str = "https://github.com/gallettom/sbxw";

/// Seconds Apple may hold a push for an offline phone (study: 3600).
pub const TTL_S: u64 = 3600;

/// Endpoint path, bytes (the sidecar's `pushPathMax`).
pub const PATH_MAX: usize = 512;
/// Whole endpoint URL, bytes (the sidecar's `pushEndpointMax`).
pub const ENDPOINT_MAX: usize = 1024;

/// One push per kind per device within this window.
pub const PER_KIND_MS: u64 = 2 * 60_000;
/// At most this many pushes per device per hour.
pub const PER_HOUR: usize = 20;
const HOUR_MS: u64 = 3_600_000;
/// After Apple answers 429 for a device: wait this long.
pub const BACKOFF_429_MS: u64 = 10 * 60_000;

/// The badge is clamped to this (the sidecar's own cap).
pub const BADGE_MAX: usize = 99;

/// Why a subscription is refused: one short word, the `400`'s `reason`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refused {
    Endpoint,
    P256dh,
    Auth,
}

impl Refused {
    pub fn word(self) -> &'static str {
        match self {
            Refused::Endpoint => "endpoint",
            Refused::P256dh => "p256dh",
            Refused::Auth => "auth",
        }
    }
}

/// The sidecar's allow-list (API-push.md section 6, `checkEndpoint`), the
/// same table tested on both sides: `https://<label>.push.apple.com[:443]/<path>`,
/// no user info, query, fragment, percent, IP literal or trailing dot, a
/// path of at most 512 bytes of `[A-Za-z0-9_\-/.]` with no dot segment.
pub fn check_endpoint(raw: &str) -> Result<(), Refused> {
    let bad = Err(Refused::Endpoint);
    if raw.len() > ENDPOINT_MAX {
        return bad;
    }
    let Some(rest) = raw.strip_prefix("https://") else { return bad };
    if rest.contains(['?', '#', '@', '\\', '%']) {
        return bad;
    }
    let Some(slash) = rest.find('/') else { return bad };
    let (authority, path) = rest.split_at(slash);
    // An IP literal in brackets (IPv6) is refused before any port parsing.
    if authority.starts_with('[') {
        return bad;
    }
    let host = match authority.split_once(':') {
        Some((h, "443")) => h,
        Some(_) => return bad,
        None => authority,
    };
    let host = host.to_ascii_lowercase();
    if host.parse::<std::net::IpAddr>().is_ok() || host.len() > 253 || host.ends_with('.') {
        return bad;
    }
    let Some(prefix) = host.strip_suffix(".push.apple.com") else { return bad };
    let label_ok = |l: &str| {
        let b = l.as_bytes();
        !b.is_empty() && b.len() <= 63 && b.iter().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-') && b[0] != b'-' && b[b.len() - 1] != b'-'
    };
    if prefix.is_empty() || !prefix.split('.').all(label_ok) {
        return bad;
    }
    if path.len() > PATH_MAX || !path.bytes().all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-' | b'/' | b'.')) {
        return bad;
    }
    if path.split('/').any(|s| s == "." || s == "..") {
        return bad;
    }
    Ok(())
}

/// base64url from a phone: strict, unpadded; trailing `=` are accepted (a
/// browser may pad), then the strict decoder decides.
fn decode_key(s: &str, len: usize) -> Option<Vec<u8>> {
    let t = s.strip_suffix("==").or_else(|| s.strip_suffix('=')).unwrap_or(s);
    b64url_decode(t, len).ok().filter(|b| b.len() == len)
}

/// One device's subscription. Its `Debug` shows nothing of it: the
/// endpoint addresses this phone, the keys encrypt to it.
#[derive(Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Subscription {
    pub endpoint: String,
    /// base64url, unpadded: the phone's P-256 public key, 65 bytes.
    pub p256dh: String,
    /// base64url, unpadded: 16 bytes.
    pub auth: String,
    pub created_ms: u64,
}

impl std::fmt::Debug for Subscription {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Subscription(..)")
    }
}

impl Subscription {
    /// What `POST /r/push/subscribe` sends, checked: the endpoint against
    /// the allow-list, `p256dh` a point of P-256 (65 bytes, uncompressed),
    /// `auth` 16 bytes. Stored re-encoded, unpadded.
    pub fn parse(endpoint: &str, p256dh: &str, auth: &str, now: u64) -> Result<Self, Refused> {
        check_endpoint(endpoint)?;
        let key = decode_key(p256dh, 65).ok_or(Refused::P256dh)?;
        if key[0] != 0x04 || p256::PublicKey::from_sec1_bytes(&key).is_err() {
            return Err(Refused::P256dh);
        }
        let secret = decode_key(auth, 16).ok_or(Refused::Auth)?;
        Ok(Subscription { endpoint: endpoint.to_string(), p256dh: b64url_encode(&key), auth: b64url_encode(&secret), created_ms: now })
    }
}

/// The Mac's VAPID key: a P-256 scalar of its own, used for nothing else.
/// Its `Debug` shows nothing.
#[derive(Clone, PartialEq, Eq)]
pub struct Vapid {
    secret: p256::SecretKey,
}

impl std::fmt::Debug for Vapid {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Vapid(..)")
    }
}

impl Vapid {
    /// A fresh key from the OS's randomness (a draw outside the curve's
    /// order, about 2^-32, is drawn again).
    pub fn generate() -> Self {
        loop {
            let mut b = [0u8; 32];
            getrandom::getrandom(&mut b).expect("the OS has no randomness to give");
            if let Ok(secret) = p256::SecretKey::from_slice(&b) {
                return Vapid { secret };
            }
        }
    }

    pub fn from_b64(s: &str) -> Option<Self> {
        let b = decode_key(s, 32)?;
        p256::SecretKey::from_slice(&b).ok().map(|secret| Vapid { secret })
    }

    /// The private scalar, base64url: for `push.json` and the sidecar's
    /// configuration line only.
    pub fn private_b64(&self) -> String {
        b64url_encode(&self.secret.to_bytes())
    }

    /// The public key the page gives `pushManager.subscribe`: the
    /// uncompressed point, 65 bytes, base64url.
    pub fn public_b64(&self) -> String {
        use p256::elliptic_curve::sec1::ToEncodedPoint;
        b64url_encode(self.secret.public_key().to_encoded_point(false).as_bytes())
    }
}

/// `<sbxw home>/remote/push.json` (`0600`): the VAPID key and one
/// subscription per paired device, by device id.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PushFile {
    pub vapid: Option<Vapid>,
    pub subscriptions: std::collections::BTreeMap<String, Subscription>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct PushFileRaw {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    vapid_private: Option<String>,
    #[serde(default)]
    subscriptions: std::collections::BTreeMap<String, Subscription>,
}

impl PushFile {
    /// A file that does not parse, or a key that is not a scalar, reads as
    /// nothing: no push until a phone subscribes again.
    pub fn parse(bytes: &[u8]) -> Option<Self> {
        let raw: PushFileRaw = serde_json::from_slice(bytes).ok()?;
        let vapid = match raw.vapid_private {
            Some(k) => Some(Vapid::from_b64(&k)?),
            None => None,
        };
        // A subscription stored by hand that breaks the rules is dropped.
        let subscriptions = raw.subscriptions.into_iter().filter(|(_, s)| Subscription::parse(&s.endpoint, &s.p256dh, &s.auth, s.created_ms).is_ok()).collect();
        Some(PushFile { vapid, subscriptions })
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let raw = PushFileRaw { vapid_private: self.vapid.as_ref().map(Vapid::private_b64), subscriptions: self.subscriptions.clone() };
        serde_json::to_vec_pretty(&raw).expect("serialisable")
    }
}

/// What a push says, as the worker reads it: `p` a plan or a merge waits,
/// `n` a session needs the person.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
pub enum PushKind {
    #[serde(rename = "p")]
    Plan,
    #[serde(rename = "n")]
    Needs,
}

impl PushKind {
    pub fn letter(self) -> &'static str {
        match self {
            PushKind::Plan => "p",
            PushKind::Needs => "n",
        }
    }
}

/// The first line on the sidecar's stdin (API-push.md section 3).
pub fn config_line(vapid: &Vapid) -> String {
    #[derive(Serialize)]
    struct Line<'a> {
        vapid_private: String,
        sub: &'a str,
    }
    let mut s = serde_json::to_string(&Line { vapid_private: vapid.private_b64(), sub: SUB_URL }).expect("serialisable");
    s.push('\n');
    s
}

/// A job line (API-push.md section 4). No field for a text, a title, a
/// sandbox or a URL exists: the sidecar builds the request itself.
#[derive(Serialize)]
pub struct Job<'a> {
    pub id: &'a str,
    pub endpoint: &'a str,
    pub p256dh: &'a str,
    pub auth: &'a str,
    pub kind: PushKind,
    pub badge: usize,
    pub ttl: u64,
}

impl Job<'_> {
    pub fn line(&self) -> String {
        let mut s = serde_json::to_string(self).expect("serialisable");
        s.push('\n');
        s
    }
}

/// A job's badge: the current total, clamped.
pub fn badge(total: usize) -> usize {
    total.min(BADGE_MAX)
}

/// One answer line from the sidecar (API-push.md section 5).
#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Answer {
    pub id: String,
    pub status: String,
}

/// What the daemon does with an answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// `201`: journal `push_sent`.
    Sent,
    /// `404`, `410` or `refused`: delete the subscription, journal `push_gone`.
    Gone,
    /// `429`: back off for this device; journal `push_failed`.
    Backoff,
    /// Anything else: journal `push_failed` with this class.
    Failed(&'static str),
}

/// The status class for the journal: never a body, never the endpoint.
pub fn outcome(status: &str) -> Outcome {
    match status {
        "201" => Outcome::Sent,
        "404" | "410" | "refused" => Outcome::Gone,
        "429" => Outcome::Backoff,
        "rate" => Outcome::Failed("rate"),
        "timeout" => Outcome::Failed("timeout"),
        "network" => Outcome::Failed("network"),
        "bad_job" => Outcome::Failed("bad_job"),
        "bad_config" => Outcome::Failed("bad_config"),
        "stopped" => Outcome::Failed("stopped"),
        s if s.len() == 3 && s.bytes().all(|b| b.is_ascii_digit()) => Outcome::Failed(match s.as_bytes()[0] {
            b'2' => "2xx",
            b'3' => "3xx",
            b'4' => "4xx",
            b'5' => "5xx",
            _ => "other",
        }),
        _ => Outcome::Failed("other"),
    }
}

/// The edges that cause a push, watched by ONE daemon task: a sandbox the
/// phone reaches starts to ask for the person (`n`, the test of the
/// `needs_you` event), or the number of plans and merges waiting goes up
/// (`p`). The first look sets the baseline and pushes nothing.
#[derive(Debug, Default)]
pub struct Edges {
    seen: Option<(BTreeSet<String>, usize)>,
}

impl Edges {
    pub fn step(&mut self, asking: BTreeSet<String>, waiting: usize) -> Vec<PushKind> {
        let mut out = Vec::new();
        if let Some((was_asking, was_waiting)) = &self.seen {
            if asking.iter().any(|s| !was_asking.contains(s)) {
                out.push(PushKind::Needs);
            }
            if waiting > *was_waiting {
                out.push(PushKind::Plan);
            }
        }
        self.seen = Some((asking, waiting));
        out
    }
}

/// Per-device limits: one push per kind per [`PER_KIND_MS`], at most
/// [`PER_HOUR`] an hour, and a pause after Apple's `429`.
#[derive(Debug, Default)]
pub struct Limits {
    per: HashMap<String, DeviceLimit>,
}

#[derive(Debug, Default)]
struct DeviceLimit {
    last: HashMap<PushKind, u64>,
    hour: VecDeque<u64>,
    paused_until: u64,
}

impl Limits {
    /// Whether a push of `kind` to `device` may go now; counts it if so.
    pub fn take(&mut self, device: &str, kind: PushKind, now: u64) -> bool {
        let d = self.per.entry(device.to_string()).or_default();
        if now < d.paused_until {
            return false;
        }
        if d.last.get(&kind).is_some_and(|t| now.saturating_sub(*t) < PER_KIND_MS) {
            return false;
        }
        while d.hour.front().is_some_and(|t| now.saturating_sub(*t) >= HOUR_MS) {
            d.hour.pop_front();
        }
        if d.hour.len() >= PER_HOUR {
            return false;
        }
        d.last.insert(kind, now);
        d.hour.push_back(now);
        true
    }

    pub fn pause(&mut self, device: &str, now: u64) {
        self.per.entry(device.to_string()).or_default().paused_until = now + BACKOFF_429_MS;
    }

    pub fn forget(&mut self, device: &str) {
        self.per.remove(device);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The sidecar's table (`tailnet/push_test.go`, TestPushEndpointAllowList),
    /// case for case.
    #[test]
    fn the_endpoint_allow_list_matches_the_sidecars_table() {
        let long = format!("https://web.push.apple.com/{}", "a".repeat(512));
        let ok_long = format!("https://web.push.apple.com/{}", "a".repeat(511));
        let too_long = format!("https://web.push.apple.com/{}", "a".repeat(1100));
        let cases: &[(&str, &str, bool)] = &[
            ("apple web push", "https://web.push.apple.com/QGx-abc_DEF.9/x", true),
            ("other apple push host", "https://api.push.apple.com/abc", true),
            ("explicit 443", "https://web.push.apple.com:443/abc", true),
            ("upper case host", "https://WEB.PUSH.APPLE.COM/abc", true),
            ("path of 512", &ok_long, true),
            ("http", "http://web.push.apple.com/abc", false),
            ("upper case scheme", "HTTPS://web.push.apple.com/abc", false),
            ("bare push.apple.com", "https://push.apple.com/abc", false),
            ("suffix trick", "https://push.apple.com.evil.example/abc", false),
            ("no dot before push", "https://evilpush.apple.com/abc", false),
            ("trailing dot", "https://web.push.apple.com./abc", false),
            ("user info", "https://user@web.push.apple.com/abc", false),
            ("user and password", "https://u:p@web.push.apple.com/abc", false),
            ("port 8443", "https://web.push.apple.com:8443/abc", false),
            ("port 80", "https://web.push.apple.com:80/abc", false),
            ("empty port", "https://web.push.apple.com:/abc", false),
            ("ipv4 literal", "https://17.0.0.1/abc", false),
            ("ipv6 literal", "https://[::1]/abc", false),
            ("ipv6 literal 443", "https://[2001:db8::1]:443/abc", false),
            ("path over 512", &long, false),
            ("path with percent", "https://web.push.apple.com/a%2Fb", false),
            ("path with space", "https://web.push.apple.com/a b", false),
            ("path with tilde", "https://web.push.apple.com/a~b", false),
            ("path with colon", "https://web.push.apple.com/a:b", false),
            ("dot dot segment", "https://web.push.apple.com/a/../b", false),
            ("query", "https://web.push.apple.com/abc?x=1", false),
            ("empty query", "https://web.push.apple.com/abc?", false),
            ("fragment", "https://web.push.apple.com/abc#x", false),
            ("backslash", "https://web.push.apple.com\\@evil.example/abc", false),
            ("percent in host", "https://web%2epush.apple.com/abc", false),
            ("underscore label", "https://we_b.push.apple.com/abc", false),
            ("empty", "", false),
            ("no scheme", "web.push.apple.com/abc", false),
            ("opaque", "https:web.push.apple.com/abc", false),
            ("too long", &too_long, false),
            ("control char", "https://web.push.apple.com/a\nb", false),
            ("empty label", "https://web..push.apple.com/abc", false),
            // Beyond the sidecar's table: shapes a hand parser could miss.
            ("no path", "https://web.push.apple.com", false),
            ("dot segment", "https://web.push.apple.com/a/./b", false),
            ("label starting with a dash", "https://-web.push.apple.com/abc", false),
            ("two colons", "https://web.push.apple.com:443:443/abc", false),
            ("unicode host", "https://wéb.push.apple.com/abc", false),
        ];
        for (name, url, ok) in cases {
            assert_eq!(check_endpoint(url).is_ok(), *ok, "{name}: {url}");
        }
    }

    fn phone_key() -> String {
        let k = p256::SecretKey::from_slice(&[7u8; 32]).unwrap();
        use p256::elliptic_curve::sec1::ToEncodedPoint;
        b64url_encode(k.public_key().to_encoded_point(false).as_bytes())
    }

    #[test]
    fn a_subscription_needs_a_point_of_p256_and_16_bytes_of_auth() {
        let ep = "https://web.push.apple.com/QGx";
        let key = phone_key();
        let auth = b64url_encode(&[1u8; 16]);
        let s = Subscription::parse(ep, &key, &auth, 5).unwrap();
        assert_eq!((s.endpoint.as_str(), s.created_ms), (ep, 5));
        assert_eq!(Subscription::parse(ep, &format!("{key}="), &format!("{auth}=="), 5).unwrap(), s, "padding is accepted and dropped");
        assert_eq!(format!("{s:?}"), "Subscription(..)", "Debug shows nothing");
        // A point off the curve, compressed, short, long, not base64url.
        let mut off = b64url_decode(&key, 65).unwrap();
        off[64] ^= 1;
        let mut compressed = b64url_decode(&key, 65).unwrap()[..33].to_vec();
        compressed[0] = 0x02;
        for bad in [b64url_encode(&off), b64url_encode(&compressed), b64url_encode(&[4u8; 64]), b64url_encode(&[4u8; 66]), "not base64!".into(), String::new(), b64url_encode(&[0u8; 65])] {
            assert_eq!(Subscription::parse(ep, &bad, &auth, 0), Err(Refused::P256dh), "{bad}");
        }
        for bad in [b64url_encode(&[1u8; 15]), b64url_encode(&[1u8; 17]), "***".into(), String::new()] {
            assert_eq!(Subscription::parse(ep, &key, &bad, 0), Err(Refused::Auth), "{bad}");
        }
        assert_eq!(Subscription::parse("https://evil.example/x", &key, &auth, 0), Err(Refused::Endpoint));
        assert_eq!(Refused::Endpoint.word(), "endpoint");
    }

    #[test]
    fn the_vapid_key_round_trips_and_never_shows() {
        let v = Vapid::generate();
        assert_ne!(v, Vapid::generate());
        assert_eq!(b64url_decode(&v.public_b64(), 65).unwrap()[0], 0x04);
        assert_eq!(b64url_decode(&v.private_b64(), 32).unwrap().len(), 32);
        assert_eq!(Vapid::from_b64(&v.private_b64()).unwrap(), v);
        assert_eq!(format!("{v:?}"), "Vapid(..)");
        assert!(Vapid::from_b64(&b64url_encode(&[0u8; 32])).is_none(), "zero is not a scalar");
        assert!(Vapid::from_b64(&b64url_encode(&[0xffu8; 32])).is_none(), "above the order");
        let mut f = PushFile { vapid: Some(v.clone()), ..Default::default() };
        f.subscriptions.insert("d1".into(), Subscription::parse("https://web.push.apple.com/a", &phone_key(), &b64url_encode(&[2u8; 16]), 1).unwrap());
        let text = String::from_utf8(f.to_bytes()).unwrap();
        assert!(!format!("{f:?}").contains(&v.private_b64()), "Debug of the file shows no key");
        assert!(!format!("{f:?}").contains("push.apple.com"), "nor an endpoint");
        assert_eq!(PushFile::parse(text.as_bytes()).unwrap(), f);
        assert!(PushFile::parse(b"{\"other\":1}").is_none(), "unknown fields refused");
        let forged = text.replace("https://web.push.apple.com/a", "https://evil.example/a");
        assert!(PushFile::parse(forged.as_bytes()).unwrap().subscriptions.is_empty(), "a forged stored endpoint is dropped");
    }

    #[test]
    fn the_config_line_and_a_job_are_exactly_the_contracts_shape() {
        let v = Vapid::generate();
        let c = config_line(&v);
        assert_eq!(c, format!("{{\"vapid_private\":\"{}\",\"sub\":\"https://github.com/gallettom/sbxw\"}}\n", v.private_b64()));
        let j = Job { id: "j1", endpoint: "https://web.push.apple.com/x", p256dh: "BP", auth: "x2", kind: PushKind::Needs, badge: badge(150), ttl: TTL_S }.line();
        assert_eq!(j, "{\"id\":\"j1\",\"endpoint\":\"https://web.push.apple.com/x\",\"p256dh\":\"BP\",\"auth\":\"x2\",\"kind\":\"n\",\"badge\":99,\"ttl\":3600}\n");
        assert!(j.len() < 4096);
    }

    #[test]
    fn each_answer_maps_to_one_outcome() {
        assert_eq!(outcome("201"), Outcome::Sent);
        for s in ["404", "410", "refused"] {
            assert_eq!(outcome(s), Outcome::Gone, "{s}");
        }
        assert_eq!(outcome("429"), Outcome::Backoff);
        assert_eq!(outcome("403"), Outcome::Failed("4xx"));
        assert_eq!(outcome("500"), Outcome::Failed("5xx"));
        assert_eq!(outcome("301"), Outcome::Failed("3xx"));
        assert_eq!(outcome("200"), Outcome::Failed("2xx"));
        for w in ["rate", "timeout", "network", "bad_job", "bad_config", "stopped"] {
            assert_eq!(outcome(w), Outcome::Failed(w));
        }
        assert_eq!(outcome("<script>"), Outcome::Failed("other"), "an unknown word is never passed on");
        assert!(serde_json::from_str::<Answer>(r#"{"id":"a","status":"201","x":1}"#).is_err());
    }

    #[test]
    fn edges_fire_on_the_rise_only() {
        let set = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<BTreeSet<_>>();
        let mut e = Edges::default();
        assert!(e.step(set(&["a"]), 3).is_empty(), "the baseline pushes nothing");
        assert!(e.step(set(&["a"]), 3).is_empty(), "no change, no push");
        assert_eq!(e.step(set(&["a", "b"]), 3), [PushKind::Needs]);
        assert!(e.step(set(&["b"]), 2).is_empty(), "a fall pushes nothing");
        assert_eq!(e.step(set(&["a", "b"]), 4), [PushKind::Needs, PushKind::Plan]);
        assert!(e.step(set(&[]), 4).is_empty());
        assert_eq!(e.step(set(&["a"]), 4), [PushKind::Needs], "asking again after an answer is a new edge");
    }

    #[test]
    fn limits_hold_per_kind_per_hour_and_after_429() {
        let mut l = Limits::default();
        assert!(l.take("d", PushKind::Needs, 0));
        assert!(!l.take("d", PushKind::Needs, PER_KIND_MS - 1), "one per kind per 2 minutes");
        assert!(l.take("d", PushKind::Plan, 1), "the other kind is its own");
        assert!(l.take("e", PushKind::Needs, 1), "another device is its own");
        assert!(l.take("d", PushKind::Needs, PER_KIND_MS));
        let mut t = PER_KIND_MS;
        let mut n = 3;
        while n < PER_HOUR {
            t += PER_KIND_MS;
            assert!(l.take("d", PushKind::Needs, t), "push {n}");
            n += 1;
        }
        t += PER_KIND_MS;
        assert!(!l.take("d", PushKind::Needs, t), "21st in the hour refused");
        assert!(!l.take("d", PushKind::Plan, t));
        assert!(l.take("d", PushKind::Needs, HOUR_MS + PER_KIND_MS * 2), "the hour slides");
        l.pause("e", 10);
        assert!(!l.take("e", PushKind::Plan, 10 + BACKOFF_429_MS - 1));
        assert!(l.take("e", PushKind::Plan, 10 + BACKOFF_429_MS));
        l.forget("e");
        assert!(l.take("e", PushKind::Plan, 10 + BACKOFF_429_MS + 1), "forgotten with its device");
    }
}
