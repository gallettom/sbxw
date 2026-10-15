//! Which sandbox serves the page in a browser tab — the question the browser
//! side panel asks before it can chat with anyone.
//!
//! The panel sits beside the app being tested, in its own top-level tab, so it
//! knows the tab's URL and nothing else. Two answers, in order:
//!
//!   * a **pairing** the developer chose by hand, keyed by the URL's `host`
//!     (`app.test`, `localhost:4200`) and kept host-side under
//!     `~/.sbxw/state/embed-pairings.json`. It wins because it is the one
//!     answer a person gave on purpose, and it is the only one for an app sbxw
//!     did not route itself (company DNS, a hand-written `/etc/hosts` line, a
//!     reverse proxy on the host);
//!   * the **published ports**: the hostname becomes an IP through sbxw's own
//!     `/etc/hosts` block (or is one already), and the sandbox that publishes
//!     that IP and port is the one serving the page. In ip-per-app mode the IP
//!     tells the apps apart; on the shared loopback the port does.
//!
//! Anything else is no answer, and the panel asks the developer to choose —
//! once, since the choice becomes a pairing.
//!
//! Everything here is pure over its inputs (`sbx ls`, `sbx ports`, the hosts
//! block and the pairing file are read by the caller), so the rules can be
//! tested without a daemon or a sandbox.

use crate::hosts::HostAlias;
use crate::sbx::PortMapping;
use anyhow::{bail, Context, Result};
use std::collections::BTreeMap;
use std::path::Path;

/// The parts of a tab's URL that say where the page came from.
#[derive(Debug, PartialEq)]
pub struct Origin {
    /// What a pairing is keyed by: the URL's `host` as a browser reports it —
    /// hostname, plus the port when it isn't the scheme's default.
    pub host: String,
    /// Hostname alone, lowercased, IPv6 brackets removed.
    pub hostname: String,
    /// Port the browser connects to, the scheme's default filled in.
    pub port: u16,
}

/// Parse an `http(s)://` URL down to its `Origin`. Anything else — `chrome://`,
/// `file://`, a new tab — is `None`: there is no app behind it to chat about.
///
/// Hand-rolled because the daemon has no URL crate and needs only this much:
/// scheme, then the authority up to the first `/`, `?` or `#`, minus any
/// `user:password@`.
pub fn parse_origin(url: &str) -> Option<Origin> {
    let (scheme, rest) = url.trim().split_once("://")?;
    let default_port = match scheme.to_ascii_lowercase().as_str() {
        "http" => 80,
        "https" => 443,
        _ => return None,
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let authority = authority.rsplit_once('@').map_or(authority, |(_, a)| a);
    let authority = authority.to_ascii_lowercase();

    // `[::1]:8080` — the colon that starts the port is the one after `]`.
    let (hostname, port) = if let Some(after) = authority.strip_prefix('[') {
        let (inside, tail) = after.split_once(']')?;
        (inside.to_string(), tail.strip_prefix(':'))
    } else {
        match authority.rsplit_once(':') {
            Some((h, p)) => (h.to_string(), Some(p)),
            None => (authority.clone(), None),
        }
    };
    let port = match port {
        None | Some("") => default_port,
        Some(p) => p.parse().ok()?,
    };
    if hostname.is_empty() {
        return None;
    }
    let shown = if hostname.contains(':') { format!("[{hostname}]") } else { hostname.clone() };
    let host = if port == default_port { shown } else { format!("{shown}:{port}") };
    Some(Origin { host, hostname, port })
}

/// Is `host` something a pairing may be keyed by? A browser's `URL.host`
/// alphabet, bounded: the key ends up in a JSON file and in the web UI.
pub fn valid_host(host: &str) -> bool {
    !host.is_empty()
        && host.len() <= 255
        && host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | ':' | '[' | ']'))
}

/// How a sandbox was found for a tab.
#[derive(Debug, PartialEq, Clone, Copy)]
pub enum Via {
    Pairing,
    Port,
}

impl Via {
    pub fn as_str(self) -> &'static str {
        match self {
            Via::Pairing => "pairing",
            Via::Port => "port",
        }
    }
}

/// The answer for one tab.
#[derive(Debug, PartialEq)]
pub struct Resolution {
    /// The sandbox to chat with, when exactly one qualifies.
    pub sandbox: Option<String>,
    pub via: Option<Via>,
    /// Sandboxes that publish the tab's address, when more than one does —
    /// the developer picks among these first.
    pub candidates: Vec<String>,
    /// A pairing that names a sandbox `sbx ls` no longer lists. Ignored for
    /// the answer, reported so the panel can say why it is asking again.
    pub stale_pairing: Option<String>,
}

/// Find the sandbox serving `origin`.
///
/// `sandboxes` is `sbx ls`; an empty list means it could not be read, and a
/// pairing is then trusted rather than declared stale on no evidence.
/// `ports` is every sandbox's `sbx ports`, as `(sandbox, mapping)`.
pub fn resolve(
    origin: &Origin,
    pairings: &BTreeMap<String, String>,
    aliases: &[HostAlias],
    ports: &[(String, PortMapping)],
    sandboxes: &[String],
) -> Resolution {
    let mut stale_pairing = None;
    if let Some(paired) = pairings.get(&origin.host) {
        if sandboxes.is_empty() || sandboxes.contains(paired) {
            return Resolution {
                sandbox: Some(paired.clone()),
                via: Some(Via::Pairing),
                candidates: Vec::new(),
                stale_pairing: None,
            };
        }
        stale_pairing = Some(paired.clone());
    }

    let ips = addresses_of(&origin.hostname, aliases);
    let mut candidates: Vec<String> = ports
        .iter()
        .filter(|(_, m)| {
            m.host_port == origin.port
                && m.proto.to_ascii_lowercase().starts_with("tcp")
                && (is_wildcard(&m.host_ip) || ips.iter().any(|ip| same_ip(ip, &m.host_ip)))
        })
        .map(|(name, _)| name.clone())
        .collect();
    candidates.sort();
    candidates.dedup();

    if candidates.len() == 1 {
        Resolution {
            sandbox: candidates.pop(),
            via: Some(Via::Port),
            candidates,
            stale_pairing,
        }
    } else {
        Resolution { sandbox: None, via: None, candidates, stale_pairing }
    }
}

/// The IPs a hostname reaches, as far as sbxw knows: its own `/etc/hosts`
/// block first (that is how an alias gets its 127.0.0.X), then an IP literal
/// as itself, then `localhost` and `*.localhost`, which browsers send to the
/// loopback without asking DNS. Any other name is outside what sbxw routes.
fn addresses_of(hostname: &str, aliases: &[HostAlias]) -> Vec<String> {
    let from_block: Vec<String> = aliases
        .iter()
        .filter(|a| a.hostname.eq_ignore_ascii_case(hostname))
        .map(|a| a.ip.clone())
        .collect();
    if !from_block.is_empty() {
        return from_block;
    }
    if hostname.parse::<std::net::IpAddr>().is_ok() {
        return vec![hostname.to_string()];
    }
    if hostname == "localhost" || hostname.ends_with(".localhost") {
        return vec!["127.0.0.1".into()];
    }
    Vec::new()
}

/// A binding that answers on every address of the host.
fn is_wildcard(ip: &str) -> bool {
    matches!(ip, "" | "*" | "0.0.0.0" | "::" | "[::]")
}

/// `list_ports_parsed` keeps the IPv4 row of an IPv4/IPv6 pair, so a page
/// reached over `[::1]` is served by the `127.0.0.1` binding.
fn same_ip(a: &str, b: &str) -> bool {
    let norm = |ip: &str| match ip.trim_start_matches('[').trim_end_matches(']') {
        "::1" => "127.0.0.1".to_string(),
        other => other.to_string(),
    };
    norm(a) == norm(b)
}

/// The pairings in `file`. Unreadable or corrupt reads as none: the panel
/// still works, it just asks again.
pub fn load_pairings(file: &Path) -> BTreeMap<String, String> {
    std::fs::read_to_string(file)
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

/// Pair `host` with `sandbox`, or forget its pairing when `sandbox` is `None`,
/// returning the pairings as they now stand.
pub fn set_pairing(
    file: &Path,
    host: &str,
    sandbox: Option<&str>,
) -> Result<BTreeMap<String, String>> {
    let host = host.trim().to_ascii_lowercase();
    if !valid_host(&host) {
        bail!("'{host}' is not a host a browser tab could be on");
    }
    let mut pairings = load_pairings(file);
    match sandbox {
        Some(name) => {
            if !crate::is_valid_sandbox_name(name) {
                bail!(crate::INVALID_NAME_MSG);
            }
            pairings.insert(host, name.to_string());
        }
        None => {
            pairings.remove(&host);
        }
    }
    let body = serde_json::to_string_pretty(&pairings)?;
    std::fs::write(file, body).with_context(|| format!("could not write {}", file.display()))?;
    Ok(pairings)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mapping(host_ip: &str, host_port: u16) -> PortMapping {
        PortMapping {
            sandbox_port: host_port,
            proto: "tcp".into(),
            host_ip: host_ip.into(),
            host_port,
        }
    }

    fn alias(hostname: &str, ip: &str) -> HostAlias {
        HostAlias { hostname: hostname.into(), ip: ip.into() }
    }

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn an_origin_keeps_only_what_says_where_the_page_came_from() {
        let o = parse_origin("https://user:pw@App.Test/orders?id=3#top").unwrap();
        assert_eq!(o, Origin { host: "app.test".into(), hostname: "app.test".into(), port: 443 });

        let o = parse_origin("http://localhost:4200/").unwrap();
        assert_eq!((o.host.as_str(), o.port), ("localhost:4200", 4200));

        let o = parse_origin("http://[::1]:8080/x").unwrap();
        assert_eq!((o.host.as_str(), o.hostname.as_str(), o.port), ("[::1]:8080", "::1", 8080));

        // An explicit default port is dropped, as `URL.host` drops it.
        assert_eq!(parse_origin("https://app.test:443").unwrap().host, "app.test");
    }

    #[test]
    fn only_web_pages_have_an_origin() {
        for url in ["chrome://extensions", "file:///tmp/x.html", "about:blank", "http://", "http://a:99999"] {
            assert_eq!(parse_origin(url), None, "{url}");
        }
    }

    #[test]
    fn an_alias_in_ip_per_app_mode_finds_its_sandbox_by_ip() {
        let origin = parse_origin("https://neos.local/login").unwrap();
        let aliases = [alias("neos.local", "127.0.0.2"), alias("shop.local", "127.0.0.3")];
        let ports = [
            ("neos".to_string(), mapping("127.0.0.2", 443)),
            ("shop".to_string(), mapping("127.0.0.3", 443)),
        ];
        let r = resolve(&origin, &BTreeMap::new(), &aliases, &ports, &names(&["neos", "shop"]));
        assert_eq!(r.sandbox.as_deref(), Some("neos"));
        assert_eq!(r.via, Some(Via::Port));
    }

    #[test]
    fn on_the_shared_loopback_the_port_tells_the_apps_apart() {
        let ports = [
            ("front".to_string(), mapping("127.0.0.1", 4200)),
            ("back".to_string(), mapping("127.0.0.1", 8000)),
        ];
        let sandboxes = names(&["front", "back"]);
        for (url, want) in [("http://localhost:8000/api", "back"), ("http://[::1]:4200", "front")] {
            let r = resolve(&parse_origin(url).unwrap(), &BTreeMap::new(), &[], &ports, &sandboxes);
            assert_eq!(r.sandbox.as_deref(), Some(want), "{url}");
        }
    }

    #[test]
    fn a_pairing_wins_and_a_stale_one_falls_back_to_the_ports() {
        let origin = parse_origin("http://localhost:4200").unwrap();
        let ports = [("front".to_string(), mapping("127.0.0.1", 4200))];
        let mut pairings = BTreeMap::new();
        pairings.insert("localhost:4200".to_string(), "other".to_string());

        let r = resolve(&origin, &pairings, &[], &ports, &names(&["front", "other"]));
        assert_eq!((r.sandbox.as_deref(), r.via), (Some("other"), Some(Via::Pairing)));

        let r = resolve(&origin, &pairings, &[], &ports, &names(&["front"]));
        assert_eq!((r.sandbox.as_deref(), r.via), (Some("front"), Some(Via::Port)));
        assert_eq!(r.stale_pairing.as_deref(), Some("other"));

        // `sbx ls` unreadable: no evidence the pairing is stale, so it holds.
        let r = resolve(&origin, &pairings, &[], &ports, &[]);
        assert_eq!(r.sandbox.as_deref(), Some("other"));
    }

    #[test]
    fn two_sandboxes_on_one_address_are_candidates_not_an_answer() {
        let origin = parse_origin("http://localhost:3000").unwrap();
        let ports = [
            ("b".to_string(), mapping("0.0.0.0", 3000)),
            ("a".to_string(), mapping("127.0.0.1", 3000)),
        ];
        let r = resolve(&origin, &BTreeMap::new(), &[], &ports, &names(&["a", "b"]));
        assert_eq!(r.sandbox, None);
        assert_eq!(r.candidates, names(&["a", "b"]));
    }

    #[test]
    fn a_name_sbxw_does_not_route_has_no_answer() {
        let origin = parse_origin("https://app.company.dev").unwrap();
        let ports = [("neos".to_string(), mapping("127.0.0.1", 443))];
        let r = resolve(&origin, &BTreeMap::new(), &[], &ports, &names(&["neos"]));
        assert_eq!(r, Resolution { sandbox: None, via: None, candidates: vec![], stale_pairing: None });
    }

    #[test]
    fn pairings_are_set_and_forgotten_in_their_file() {
        let dir = std::env::temp_dir().join(format!("sbxw-embed-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("embed-pairings.json");
        let _ = std::fs::remove_file(&file);

        let p = set_pairing(&file, " App.Test ", Some("neos")).unwrap();
        assert_eq!(p.get("app.test").map(String::as_str), Some("neos"));
        assert_eq!(load_pairings(&file), p);

        assert!(set_pairing(&file, "app.test", Some("Not A Name!")).is_err());
        assert!(set_pairing(&file, "app test/../x", Some("neos")).is_err());

        let p = set_pairing(&file, "app.test", None).unwrap();
        assert!(p.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
