//! Port mappings and the host addresses they live on.
//!
//! Two vocabularies meet here. sbxw's own — a sandbox port, a host port, an
//! optional `/etc/hosts` alias, and the `ip_per_app` choice of one loopback
//! address per app — and sbx's, a publish spec
//! `[[HOST_IP:]HOST_PORT:]SANDBOX_PORT[/PROTO]` and the rows `sbx ports` prints.

use crate::hosts::HostAlias;
use serde::{Deserialize, Serialize};

/// One mapping sbxw wants published: from `sbxw.toml`'s `[[ports]]`, or added
/// from the web UI's create dialog.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WantedPort {
    pub host_port: u16,
    pub sandbox_port: u16,
    /// Hostname for `/etc/hosts`; empty for none.
    #[serde(default)]
    pub alias: String,
}

/// One mapping as `sbx ports <name>` reports it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PortMapping {
    pub sandbox_port: u16,
    pub proto: String,
    pub host_ip: String,
    pub host_port: u16,
}

impl PortMapping {
    /// The unpublish spec for exactly this binding: the host IP is included so
    /// unpublishing one alias (127.0.0.2) leaves another on the same port pair.
    pub fn spec(&self) -> String {
        format!("{}:{}:{}", self.host_ip, self.host_port, self.sandbox_port)
    }
}

/// Host IP an app binds to: a distinct loopback per app (`ip_per_app`, from
/// 127.0.0.2 in declaration order), else 127.0.0.1.
pub fn host_ip_for(ip_per_app: bool, index: usize) -> String {
    if ip_per_app {
        format!("127.0.0.{}", 2 + index)
    } else {
        "127.0.0.1".into()
    }
}

/// `sbx ports --publish` / `sbx create -p` spec for each mapping. The order is
/// the order of `ports`, which decides each app's loopback IP in `ip_per_app`
/// mode — callers must keep it.
pub fn publish_specs(ports: &[WantedPort], ip_per_app: bool) -> Vec<String> {
    ports
        .iter()
        .enumerate()
        .map(|(i, p)| {
            if ip_per_app {
                format!("{}:{}:{}", host_ip_for(true, i), p.host_port, p.sandbox_port)
            } else {
                format!("{}:{}", p.host_port, p.sandbox_port)
            }
        })
        .collect()
}

/// `/etc/hosts` entries for the mappings that name a hostname.
pub fn host_aliases(ports: &[WantedPort], ip_per_app: bool) -> Vec<HostAlias> {
    ports
        .iter()
        .enumerate()
        .filter(|(_, p)| !p.alias.is_empty())
        .map(|(i, p)| HostAlias {
            hostname: p.alias.clone(),
            ip: host_ip_for(ip_per_app, i),
        })
        .collect()
}

/// Port half of an address ("127.0.0.1:7681" → "7681"). `rsplit` so an IPv6
/// literal yields the port, not the first hextet.
pub fn port_of(addr: &str) -> &str {
    addr.rsplit(':').next().unwrap_or("")
}

/// Host half of an address, brackets off ("[::1]:7681" → "::1").
pub fn ip_of(addr: &str) -> &str {
    match addr.rsplit_once(':') {
        Some((host, _)) => host.trim_start_matches('[').trim_end_matches(']'),
        None => addr,
    }
}

/// The loopback name of the web UI. Only an address on 127.0.0.0/8 earns it:
/// `sbxw.localhost` pointing at a LAN IP would be a claim about another
/// machine.
pub const WEB_HOSTNAME: &str = "sbxw.localhost";

pub fn web_alias(web_addr: &str) -> Option<HostAlias> {
    let ip = ip_of(web_addr);
    ip.starts_with("127.").then(|| HostAlias {
        hostname: WEB_HOSTNAME.into(),
        ip: ip.to_string(),
    })
}

/// `sbx ports N --format json`: rows of `host_ip`, `host_port`,
/// `sandbox_port`, `protocol`; `None` when `raw` is not that. The IPv6
/// mirror sbx adds for each IPv4 publish is dropped; every distinct IPv4
/// binding is kept — including the extra loopbacks of `ip_per_app`.
pub fn parse_ports_json(raw: &str) -> Option<Vec<PortMapping>> {
    let rows: Vec<serde_json::Value> = serde_json::from_str(raw.trim()).ok()?;
    let port = |v: &serde_json::Value, k: &str| {
        let x = v.get(k)?;
        x.as_u64().and_then(|n| u16::try_from(n).ok()).or_else(|| x.as_str()?.parse().ok())
    };
    let mut seen = std::collections::HashSet::new();
    Some(
        rows.iter()
            .filter_map(|v| {
                Some(PortMapping {
                    sandbox_port: port(v, "sandbox_port")?,
                    host_port: port(v, "host_port")?,
                    host_ip: v.get("host_ip").and_then(|x| x.as_str()).filter(|s| !s.is_empty()).unwrap_or("0.0.0.0").to_string(),
                    proto: v.get("protocol").and_then(|x| x.as_str()).filter(|s| !s.is_empty()).unwrap_or("tcp").to_string(),
                })
            })
            .filter(|p| !p.host_ip.contains(':') && seen.insert((p.host_ip.clone(), p.host_port, p.sandbox_port, p.proto.clone())))
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wp(host: u16, sbox: u16, alias: &str) -> WantedPort {
        WantedPort {
            host_port: host,
            sandbox_port: sbox,
            alias: alias.into(),
        }
    }

    #[test]
    fn specs_carry_an_ip_only_per_app() {
        let ports = [wp(4200, 4200, "neos.local"), wp(8000, 8000, "")];
        assert_eq!(publish_specs(&ports, false), ["4200:4200", "8000:8000"]);
        assert_eq!(
            publish_specs(&ports, true),
            ["127.0.0.2:4200:4200", "127.0.0.3:8000:8000"]
        );
    }

    #[test]
    fn only_named_ports_get_an_alias_on_their_own_ip() {
        let ports = [wp(1, 1, ""), wp(2, 2, "b.local")];
        assert_eq!(
            host_aliases(&ports, true),
            [HostAlias {
                hostname: "b.local".into(),
                ip: "127.0.0.3".into()
            }]
        );
        assert_eq!(host_aliases(&ports, false)[0].ip, "127.0.0.1");
    }

    #[test]
    fn addresses_split_at_the_last_colon() {
        assert_eq!(port_of("127.0.0.1:7681"), "7681");
        assert_eq!(port_of("[::1]:7681"), "7681");
        assert_eq!(ip_of("[::1]:7681"), "::1");
        assert_eq!(ip_of("127.0.0.1:7681"), "127.0.0.1");
    }

    #[test]
    fn the_web_name_is_for_loopback_only() {
        assert!(web_alias("127.0.0.1:7681").is_some());
        assert!(web_alias("[::1]:7681").is_none());
        assert!(web_alias("192.168.1.2:7681").is_none());
    }

    #[test]
    fn the_ports_json_uses_the_documented_fields() {
        let raw = r#"[{"host_ip":"127.0.0.2","host_port":4200,"sandbox_port":4200,"protocol":"tcp4"},
                     {"host_ip":"::1","host_port":4200,"sandbox_port":4200,"protocol":"tcp6"}]"#;
        let p = parse_ports_json(raw).unwrap();
        assert_eq!(p.len(), 1, "IPv6 bindings dropped");
        assert_eq!(p[0].spec(), "127.0.0.2:4200:4200");
        assert_eq!(parse_ports_json("[]").unwrap(), vec![]);
        assert!(parse_ports_json("HOST IP  HOST PORT  SANDBOX PORT\n").is_none());
    }
}
