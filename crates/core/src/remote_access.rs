//! Remote access: a paired phone talks to a running team leader over a
//! private Tailscale network (see `docs/remote-access.md`).
//!
//! Not Remote Control (`crate::remote`): that is claude.ai driving a session.
//! Here the daemon itself answers a second, reduced listener (`remote_addr`),
//! reached only through the `sbxw-tailnet` sidecar, which terminates TLS on
//! the tailnet and forwards to it.
//!
//! This module holds the rules, with time passed in so tests can move it:
//! * [`devices`]: paired devices, their tokens kept as SHA-256 only.
//! * [`pairing`]: the one-time code, the requests a phone makes with it, the
//!   six-digit check both screens show.
//! * [`journal`]: what each device did, never what it said.
//! * [`limits`]: fixed-window rate limits.
//! * Here: the Host and Origin rules, and the sidecar's status file, read as
//!   untrusted input.
//!
//! The daemon's `svc::remote_access` owns the files and the listener.

pub mod devices;
pub mod journal;
pub mod limits;
pub mod pairing;
pub mod view;

use serde::{Deserialize, Serialize};

/// `remote_addr` in `sbxw.toml`.
pub const DEFAULT_REMOTE_ADDR: &str = "127.0.0.1:7683";

/// The only carrier of a device token. `__Host-` makes a browser refuse it
/// unless it is `Secure`, `Path=/` and has no `Domain`.
pub const COOKIE: &str = "__Host-sbxw_device";

/// A device not seen for this long is dropped and must pair again (Q16).
pub const DEVICE_IDLE_MS: u64 = 30 * 24 * 60 * 60 * 1000;

/// The cookie's `Max-Age`, in seconds: the idle limit.
pub const COOKIE_MAX_AGE_S: u64 = DEVICE_IDLE_MS / 1000;

/// The sidecar must rewrite its status file at least this often; an older
/// file means the sidecar is not serving.
pub const TAILNET_STALE_MS: u64 = 30_000;

/// The status file's size cap.
pub const TAILNET_FILE_MAX: u64 = 8 * 1024;

/// SHA-256 of a token, lower-case hex. Tokens are 256 random bits, so a fast
/// hash is not reversible and needs no salt.
pub fn hash_token(token: &str) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(token.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

/// A loopback listen address: `127.0.0.0/8` or `[::1]`, with a port.
pub fn is_loopback_addr(addr: &str) -> bool {
    match addr.parse::<std::net::SocketAddr>() {
        Ok(a) => a.ip().is_loopback() && a.port() != 0,
        Err(_) => false,
    }
}

/// A plain DNS name: labels of letters, digits and `-`, joined by `.`, at
/// least two labels, lower case, no trailing dot, 253 characters at most.
pub fn valid_tailnet_name(name: &str) -> bool {
    if name.is_empty() || name.len() > 253 || name.ends_with('.') {
        return false;
    }
    let labels: Vec<&str> = name.split('.').collect();
    labels.len() >= 2
        && labels.iter().all(|l| {
            !l.is_empty()
                && l.len() <= 63
                && !l.starts_with('-')
                && !l.ends_with('-')
                && l.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        })
}

/// What the sidecar writes to `tailnet.json` (see `.sbxw-artifacts/remote/API.md`
/// section 6). Unknown fields are refused: the file is another process's.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TailnetStatus {
    pub state: TailnetState,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub login_url: Option<String>,
    #[serde(default)]
    pub detail: Option<String>,
    pub pid: u32,
    pub updated_ms: u64,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TailnetState {
    Starting,
    NeedsLogin,
    Running,
    HttpsOff,
    Stopped,
    Error,
}

impl TailnetStatus {
    /// Parse the file's bytes; `None` for anything off-schema or too large.
    pub fn parse(bytes: &[u8]) -> Option<TailnetStatus> {
        if bytes.len() as u64 > TAILNET_FILE_MAX {
            return None;
        }
        let mut s: TailnetStatus = serde_json::from_slice(bytes).ok()?;
        // Shown on the Mac: short plain text only.
        let clean = |v: Option<String>, max: usize| v.filter(|t| t.len() <= max && !t.chars().any(char::is_control));
        s.detail = clean(s.detail, 500);
        s.login_url = clean(s.login_url, 1000).filter(|u| u.starts_with("https://"));
        s.name = s.name.filter(|n| valid_tailnet_name(n));
        Some(s)
    }

    /// The name the Host and Origin checks may use: only from a running,
    /// fresh sidecar that is the daemon's own child.
    pub fn serving_name(&self, now: u64, child_pid: Option<u32>) -> Option<&str> {
        let fresh = now.saturating_sub(self.updated_ms) <= TAILNET_STALE_MS && self.updated_ms <= now + 5_000;
        (self.state == TailnetState::Running && fresh && child_pid == Some(self.pid)).then_some(self.name.as_deref()).flatten()
    }
}

/// The Host values the remote listener answers: the serving tailnet name
/// (no port: the sidecar is on 443), or the listener's own loopback address.
pub fn allowed_hosts(remote_addr: &str, tailnet: Option<&str>) -> Vec<String> {
    let port = crate::ports::port_of(remote_addr);
    let mut out = vec![format!("127.0.0.1:{port}"), format!("localhost:{port}"), format!("[::1]:{port}")];
    let ip = crate::ports::ip_of(remote_addr);
    let own = if ip.contains(':') { format!("[{ip}]:{port}") } else { format!("{ip}:{port}") };
    if !out.contains(&own) {
        out.push(own);
    }
    if let Some(n) = tailnet {
        out.push(n.to_string());
    }
    out
}

/// Why a request is refused before anything else, if it is: a `Host` not in
/// `allowed`, or an `Origin` that is not that host's own page. `Origin` is
/// required on every method that is not GET or HEAD.
pub fn check_host_origin(allowed: &[String], tailnet: Option<&str>, host: Option<&str>, origin: Option<&str>, safe_method: bool) -> Result<(), &'static str> {
    let host = host.unwrap_or("");
    if host.is_empty() || !allowed.iter().any(|h| h == host) {
        return Err("unknown Host");
    }
    let expected = if Some(host) == tailnet { format!("https://{host}") } else { format!("http://{host}") };
    match origin {
        Some(o) if o == expected => Ok(()),
        Some(_) => Err("cross-origin requests are refused"),
        None if safe_method => Ok(()),
        None => Err("Origin required"),
    }
}

/// The rate-limit source from the sidecar's `X-Forwarded-For`: a hint only
/// (any local process may send one). One IP literal, else the shared bucket.
pub fn source_of(forwarded_for: Option<&str>) -> String {
    match forwarded_for.map(str::trim) {
        Some(v) if v.parse::<std::net::IpAddr>().is_ok() => v.to_string(),
        _ => "shared".to_string(),
    }
}

/// Hold the Mac awake for remote access (Q15): while it is on and the Mac is
/// on mains power (a Mac with no battery is on mains). On battery it may
/// sleep: no setting is changed, and a forgotten phone never drains it.
pub fn keep_awake_wanted(enabled: bool, battery: Option<crate::power::Battery>) -> bool {
    enabled && !battery.is_some_and(|b| b.on_battery)
}

/// `remote-access.json`: whether it is on, and the paired devices.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(default)]
pub struct State {
    pub enabled: bool,
    pub devices: Vec<devices::Device>,
}

/// A device name as a phone sends it: trimmed, 1 to 64 characters, no
/// control or bidi-override character (it is shown on the Mac, next to the
/// check the person compares).
pub fn clean_device_name(raw: &str) -> Option<String> {
    let t = raw.trim();
    let bad = |c: char| c.is_control() || ('\u{202a}'..='\u{202e}').contains(&c) || ('\u{2066}'..='\u{2069}').contains(&c) || c == '\u{200e}' || c == '\u{200f}';
    (!t.is_empty() && t.chars().count() <= 64 && !t.chars().any(bad)).then(|| t.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_is_sha256_hex() {
        // echo -n abc | sha256sum
        assert_eq!(hash_token("abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    }

    #[test]
    fn loopback_only() {
        assert!(is_loopback_addr("127.0.0.1:7683"));
        assert!(is_loopback_addr("127.1.2.3:7683"));
        assert!(is_loopback_addr("[::1]:7683"));
        for bad in ["0.0.0.0:7683", "[::]:7683", "192.168.1.2:7683", "100.64.0.1:7683", "localhost:7683", "127.0.0.1", "127.0.0.1:0"] {
            assert!(!is_loopback_addr(bad), "{bad}");
        }
    }

    #[test]
    fn tailnet_names() {
        assert!(valid_tailnet_name("sbxw.tail1234.ts.net"));
        for bad in ["", "sbxw", "SBXW.ts.net", "sbxw.ts.net.", "a..b", "-a.b", "a.b-", "a b.c", "a.b:443", "a/b.c", "evil.com\r\nx: y"] {
            assert!(!valid_tailnet_name(bad), "{bad:?}");
        }
        assert!(!valid_tailnet_name(&format!("{}.net", "a".repeat(64))));
    }

    fn status(json: &str) -> Option<TailnetStatus> {
        TailnetStatus::parse(json.as_bytes())
    }

    #[test]
    fn the_status_file_is_untrusted() {
        let good = r#"{"state":"running","name":"sbxw.tail1.ts.net","pid":42,"updated_ms":1000}"#;
        let s = status(good).unwrap();
        assert_eq!(s.serving_name(1000, Some(42)), Some("sbxw.tail1.ts.net"));
        assert_eq!(s.serving_name(1000, Some(43)), None, "not our child");
        assert_eq!(s.serving_name(1000, None), None, "no child");
        assert_eq!(s.serving_name(1000 + TAILNET_STALE_MS + 1, Some(42)), None, "stale");
        assert_eq!(s.serving_name(0, Some(42)), Some("sbxw.tail1.ts.net"), "a little clock skew");
        assert!(status(r#"{"state":"running","name":"x.y","pid":1,"updated_ms":1,"extra":1}"#).is_none(), "unknown field");
        assert!(status(r#"{"state":"pwned","pid":1,"updated_ms":1}"#).is_none());
        assert!(status("not json").is_none());
        let big = format!(r#"{{"state":"running","pid":1,"updated_ms":1,"detail":"{}"}}"#, "a".repeat(9000));
        assert!(status(&big).is_none(), "size cap");
        let s = status(r#"{"state":"running","name":"Evil.com:80","pid":1,"updated_ms":1}"#).unwrap();
        assert_eq!(s.name, None, "a name off the pattern is dropped");
        assert_eq!(s.serving_name(1, Some(1)), None);
        let s = status(r#"{"state":"needs_login","name":"a.b","login_url":"javascript:x","pid":1,"updated_ms":1}"#).unwrap();
        assert_eq!(s.login_url, None);
        assert_eq!(s.serving_name(1, Some(1)), None, "not running");
    }

    #[test]
    fn host_and_origin() {
        let tn = Some("sbxw.tail1.ts.net");
        let allowed = allowed_hosts("127.0.0.1:7683", tn);
        let ok = |h: &str, o: Option<&str>, safe: bool| check_host_origin(&allowed, tn, Some(h), o, safe).is_ok();
        assert!(ok("sbxw.tail1.ts.net", Some("https://sbxw.tail1.ts.net"), false));
        assert!(ok("sbxw.tail1.ts.net", None, true));
        assert!(!ok("sbxw.tail1.ts.net", None, false), "Origin required on writes");
        assert!(!ok("sbxw.tail1.ts.net", Some("http://sbxw.tail1.ts.net"), false), "https only");
        assert!(!ok("sbxw.tail1.ts.net", Some("https://evil.ts.net"), true));
        assert!(!ok("sbxw.tail1.ts.net", Some("null"), false));
        assert!(!ok("SBXW.tail1.ts.net", None, true), "exact, lower case");
        assert!(!ok("sbxw.tail1.ts.net:443", None, true));
        assert!(!ok("evil.com", None, true));
        assert!(!ok("127.0.0.1:7681", None, true), "the UI's port is not ours");
        assert!(ok("127.0.0.1:7683", Some("http://127.0.0.1:7683"), false));
        assert!(!ok("127.0.0.1:7683", Some("https://sbxw.tail1.ts.net"), false));
        assert!(check_host_origin(&allowed, tn, None, None, true).is_err());
        let not_serving = allowed_hosts("127.0.0.1:7683", None);
        assert!(check_host_origin(&not_serving, None, Some("sbxw.tail1.ts.net"), None, true).is_err(), "no name, no tailnet Host");
    }

    #[test]
    fn source_is_a_hint() {
        assert_eq!(source_of(Some("100.64.0.7")), "100.64.0.7");
        assert_eq!(source_of(Some("fd7a:115c::1")), "fd7a:115c::1");
        assert_eq!(source_of(None), "shared");
        assert_eq!(source_of(Some("1.2.3.4, 5.6.7.8")), "shared", "one value only");
        assert_eq!(source_of(Some("garbage")), "shared");
    }

    #[test]
    fn awake_only_when_on_and_on_mains() {
        use crate::power::Battery;
        assert!(keep_awake_wanted(true, None));
        assert!(keep_awake_wanted(true, Some(Battery { percent: 50, on_battery: false })));
        assert!(!keep_awake_wanted(true, Some(Battery { percent: 90, on_battery: true })));
        assert!(!keep_awake_wanted(false, None));
    }

    #[test]
    fn device_names() {
        assert_eq!(clean_device_name("  iPhone de T  ").as_deref(), Some("iPhone de T"));
        assert!(clean_device_name("").is_none());
        assert!(clean_device_name("a\nb").is_none());
        assert!(clean_device_name("abc\u{202e}fed").is_none());
        assert!(clean_device_name(&"x".repeat(65)).is_none());
        assert!(clean_device_name(&"é".repeat(64)).is_some());
    }
}
