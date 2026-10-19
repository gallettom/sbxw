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

/// Parse `sbx ports <name>`.
///
/// Confirmed format (four columns, one header row):
/// ```text
/// HOST IP     HOST PORT   SANDBOX PORT   PROTOCOL
/// 127.0.0.1   3000        3000           tcp
/// ::1         3000        3000           tcp
/// ```
/// with a fallback for Docker's arrow style `3000/tcp -> 0.0.0.0:3000`.
///
/// The IPv6 mirror sbx adds for each IPv4 publish is dropped; every distinct
/// IPv4 binding is kept — including the extra loopbacks of `ip_per_app`.
pub fn parse_ports(raw: &str) -> Vec<PortMapping> {
    let mut lines = raw.lines().filter(|l| !l.trim().is_empty());
    let Some(header) = lines.next().map(str::trim) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    if header.contains("HOST IP") && header.contains("SANDBOX PORT") {
        for line in lines {
            let c: Vec<&str> = line.split_whitespace().collect();
            if c.len() < 3 {
                continue;
            }
            let (Ok(host_port), Ok(sandbox_port)) = (c[1].parse(), c[2].parse()) else {
                continue;
            };
            out.push(PortMapping {
                sandbox_port,
                proto: c.get(3).unwrap_or(&"tcp").to_string(),
                host_ip: c[0].to_string(),
                host_port,
            });
        }
    } else {
        for line in std::iter::once(header).chain(lines) {
            let line = line.trim();
            if line.split_whitespace().all(|w| w == w.to_uppercase()) {
                continue; // a header row
            }
            let (left, right) = match line.split_once("->") {
                Some((l, r)) => (l.trim(), r.trim()),
                None => match line.split_once(char::is_whitespace) {
                    Some((l, r)) => (l.trim(), r.trim()),
                    None => continue,
                },
            };
            let (port, proto) = left.split_once('/').unwrap_or((left, "tcp"));
            let Ok(sandbox_port) = port.parse() else { continue };
            let (host_ip, host_port) = match right.rsplit_once(':') {
                Some((ip, p)) => match p.parse() {
                    Ok(p) => (ip.to_string(), p),
                    Err(_) => continue,
                },
                None => match right.parse() {
                    Ok(p) => ("0.0.0.0".to_string(), p),
                    Err(_) => continue,
                },
            };
            out.push(PortMapping {
                sandbox_port,
                proto: proto.to_string(),
                host_ip,
                host_port,
            });
        }
    }
    let mut seen = std::collections::HashSet::new();
    out.retain(|p| {
        !p.host_ip.contains(':')
            && seen.insert((p.host_ip.clone(), p.host_port, p.sandbox_port, p.proto.clone()))
    });
    out
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
    fn the_columnar_listing_is_read_and_ipv6_mirrors_dropped() {
        let raw = "HOST IP     HOST PORT   SANDBOX PORT   PROTOCOL\n\
                   127.0.0.1   3000        3000           tcp\n\
                   ::1         3000        3000           tcp\n\
                   127.0.0.2   4200        4200           tcp4\n";
        let p = parse_ports(raw);
        assert_eq!(p.len(), 2);
        assert_eq!(p[1].spec(), "127.0.0.2:4200:4200");
        assert_eq!(p[1].proto, "tcp4");
    }

    #[test]
    fn the_arrow_style_is_read_too() {
        let p = parse_ports("3000/tcp -> 0.0.0.0:3001\n");
        assert_eq!(
            p,
            [PortMapping {
                sandbox_port: 3000,
                proto: "tcp".into(),
                host_ip: "0.0.0.0".into(),
                host_port: 3001
            }]
        );
        assert!(parse_ports("").is_empty());
    }
}
