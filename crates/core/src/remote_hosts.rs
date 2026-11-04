//! The registry of remote hosts: `~/.sbxw/hosts.toml`.
//!
//! (Not `hosts.rs`, which is the `/etc/hosts` block.) Pure: it parses,
//! validates and renders the file and builds the ssh argv for a host. Running
//! ssh and touching the disk is the CLI's job.
//!
//! ```toml
//! [hosts.buildbox]
//! ssh = "buildbox"        # an alias from ~/.ssh/config, OR host/user/port/key
//! agent_port = 7682       # default 7682
//! trust_secrets = false   # default false
//! sbx_path = "/opt/sbx/bin/sbx"   # default: found with `sh -lc 'command -v sbx'`
//! ```
//!
//! `key` is a PATH to an identity file; this module never holds a key or a
//! password.

use crate::names::{HostName, LOCAL_HOST};
use anyhow::{bail, Result};
use serde::Deserialize;
use std::collections::BTreeMap;

pub const DEFAULT_AGENT_PORT: u16 = 7682;

/// How to reach a host.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    /// An alias from `~/.ssh/config`.
    Alias(String),
    /// Spelled out: host, and optionally user, port and identity file.
    Direct {
        host: String,
        user: Option<String>,
        port: Option<u16>,
        key: Option<String>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteHost {
    pub name: HostName,
    pub target: Target,
    pub agent_port: u16,
    pub trust_secrets: bool,
    /// The `sbx` to run there. `None` = detect it: when the SSH link to the
    /// host connects, the transport runs `sh -lc 'command -v sbx'` (a login
    /// shell, since a non-interactive ssh PATH often lacks it) and caches the
    /// answer for that link. Set it when detection finds the wrong one.
    pub sbx_path: Option<String>,
}

/// The whole registry, ordered by name.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RemoteHosts {
    pub hosts: BTreeMap<String, RemoteHost>,
}

#[derive(Deserialize)]
struct RawFile {
    #[serde(default)]
    hosts: BTreeMap<String, RawHost>,
}

#[derive(Deserialize)]
struct RawHost {
    ssh: Option<String>,
    host: Option<String>,
    user: Option<String>,
    port: Option<u16>,
    key: Option<String>,
    agent_port: Option<u16>,
    trust_secrets: Option<bool>,
    sbx_path: Option<String>,
}

/// A value that ends up as one ssh argument: non-empty, no whitespace or
/// control characters, and not starting with `-` (it would read as an option).
fn check_word(what: &str, v: &str) -> Result<()> {
    if v.is_empty() {
        bail!("{what} is empty");
    }
    if v.starts_with('-') {
        bail!("{what} must not start with '-'");
    }
    if v.chars().any(|c| c.is_whitespace() || c.is_control()) {
        bail!("{what} must not contain spaces or control characters");
    }
    Ok(())
}

/// Validate a host name: `local` is refused (it is this machine), and the rest
/// must be a valid `HostName` spelled exactly as given (no surrounding spaces).
pub fn check_name(name: &str) -> Result<HostName> {
    if name == LOCAL_HOST {
        bail!("'{LOCAL_HOST}' is reserved for this machine");
    }
    match HostName::parse(name) {
        Ok(h) if h.as_str() == name => Ok(h),
        _ => bail!("'{name}' is not a valid host name (letters, digits and '-', not ending in '-')"),
    }
}

impl RemoteHost {
    /// Build from loose parts (the flags of `sbxw host add`), enforcing the rules.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        name: &str,
        ssh: Option<String>,
        host: Option<String>,
        user: Option<String>,
        port: Option<u16>,
        key: Option<String>,
        agent_port: Option<u16>,
        trust_secrets: bool,
    ) -> Result<Self> {
        let name = check_name(name)?;
        let target = match (ssh, host) {
            (Some(_), Some(_)) => bail!("host '{name}': set either ssh or host, not both"),
            (None, None) => bail!("host '{name}': set ssh (an alias) or host"),
            (Some(alias), None) => {
                if user.is_some() || port.is_some() || key.is_some() {
                    bail!("host '{name}': user, port and key only go with host, not ssh");
                }
                check_word("ssh", &alias)?;
                Target::Alias(alias)
            }
            (None, Some(host)) => {
                check_word("host", &host)?;
                if let Some(u) = &user {
                    check_word("user", u)?;
                }
                if let Some(k) = &key {
                    check_word("key", k)?;
                }
                if port == Some(0) {
                    bail!("host '{name}': port must be 1 to 65535");
                }
                Target::Direct { host, user, port, key }
            }
        };
        let agent_port = agent_port.unwrap_or(DEFAULT_AGENT_PORT);
        if agent_port == 0 {
            bail!("host '{name}': agent_port must be 1 to 65535");
        }
        Ok(Self { name, target, agent_port, trust_secrets, sbx_path: None })
    }

    /// Set (or clear) the `sbx` to run on the host, checked like the other
    /// words that reach ssh's command line.
    pub fn with_sbx_path(mut self, sbx_path: Option<String>) -> Result<Self> {
        if let Some(p) = &sbx_path {
            check_word("sbx_path", p).map_err(|e| anyhow::anyhow!("host '{}': {e}", self.name))?;
        }
        self.sbx_path = sbx_path;
        Ok(self)
    }

    /// The program to run for `sbx` on this host: `sbx_path`, or a bare `sbx`
    /// when unset (the SSH transport replaces that with what it detects).
    pub fn sbx_program(&self) -> &str {
        self.sbx_path.as_deref().unwrap_or("sbx")
    }

    /// The name ssh connects to (alias or address), for display and ssh-keyscan.
    pub fn address(&self) -> &str {
        match &self.target {
            Target::Alias(a) => a,
            Target::Direct { host, .. } => host,
        }
    }

    /// `ssh` arguments to reach this host, ending with the destination; append
    /// the remote command after it. Never prompts, never trusts a new key.
    pub fn ssh_argv(&self) -> Vec<String> {
        let mut a: Vec<String> = ["-o", "BatchMode=yes", "-o", "StrictHostKeyChecking=yes"]
            .map(String::from)
            .into();
        match &self.target {
            Target::Alias(alias) => a.push(alias.clone()),
            Target::Direct { host, user, port, key } => {
                if let Some(p) = port {
                    a.extend(["-p".into(), p.to_string()]);
                }
                if let Some(u) = user {
                    a.extend(["-l".into(), u.clone()]);
                }
                if let Some(k) = key {
                    a.extend(["-i".into(), k.clone()]);
                }
                a.push(host.clone());
            }
        }
        a
    }
}

impl RemoteHosts {
    pub fn parse(text: &str) -> Result<Self> {
        let raw: RawFile = toml::from_str(text).map_err(|e| anyhow::anyhow!("hosts.toml: {e}"))?;
        let mut hosts = BTreeMap::new();
        for (name, r) in raw.hosts {
            let h = RemoteHost::new(&name, r.ssh, r.host, r.user, r.port, r.key, r.agent_port, r.trust_secrets.unwrap_or(false))?
                .with_sbx_path(r.sbx_path)?;
            hosts.insert(name, h);
        }
        Ok(Self { hosts })
    }

    /// Stable text: hosts by name, a fixed key order, only known keys.
    pub fn render(&self) -> String {
        let mut out = String::new();
        for (i, h) in self.hosts.values().enumerate() {
            if i > 0 {
                out.push('\n');
            }
            out.push_str(&format!("[hosts.{}]\n", h.name));
            match &h.target {
                Target::Alias(a) => out.push_str(&format!("ssh = {}\n", q(a))),
                Target::Direct { host, user, port, key } => {
                    out.push_str(&format!("host = {}\n", q(host)));
                    if let Some(u) = user {
                        out.push_str(&format!("user = {}\n", q(u)));
                    }
                    if let Some(p) = port {
                        out.push_str(&format!("port = {p}\n"));
                    }
                    if let Some(k) = key {
                        out.push_str(&format!("key = {}\n", q(k)));
                    }
                }
            }
            out.push_str(&format!("agent_port = {}\n", h.agent_port));
            out.push_str(&format!("trust_secrets = {}\n", h.trust_secrets));
            if let Some(p) = &h.sbx_path {
                out.push_str(&format!("sbx_path = {}\n", q(p)));
            }
        }
        out
    }

    pub fn get(&self, name: &str) -> Option<&RemoteHost> {
        self.hosts.get(name)
    }
}

/// A TOML basic string.
fn q(s: &str) -> String {
    let mut o = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

#[cfg(test)]
mod tests {
    use super::*;

    fn alias(name: &str, a: &str) -> Result<RemoteHost> {
        RemoteHost::new(name, Some(a.into()), None, None, None, None, None, false)
    }

    #[test]
    fn names_follow_sandbox_rules_and_local_is_reserved() {
        assert!(alias("buildbox", "bb").is_ok());
        assert!(alias("local", "bb").is_err());
        assert!(alias("bad name", "bb").is_err());
        assert!(alias("bad-", "bb").is_err());
        assert!(alias("", "bb").is_err());
    }

    #[test]
    fn exactly_one_of_ssh_or_host() {
        let n = |s: Option<&str>, h: Option<&str>| {
            RemoteHost::new("x", s.map(Into::into), h.map(Into::into), None, None, None, None, false)
        };
        assert!(n(Some("a"), Some("b")).is_err());
        assert!(n(None, None).is_err());
        assert!(n(Some("a"), None).is_ok());
        assert!(n(None, Some("b")).is_ok());
    }

    #[test]
    fn user_port_key_only_with_host() {
        let with = |u: Option<&str>, p: Option<u16>, k: Option<&str>| {
            RemoteHost::new("x", Some("a".into()), None, u.map(Into::into), p, k.map(Into::into), None, false)
        };
        assert!(with(Some("tom"), None, None).is_err());
        assert!(with(None, Some(22), None).is_err());
        assert!(with(None, None, Some("~/k")).is_err());
        assert!(RemoteHost::new("x", None, Some("h".into()), Some("tom".into()), Some(22), Some("~/k".into()), None, false).is_ok());
    }

    #[test]
    fn rejects_option_lookalikes_and_zero_ports() {
        assert!(alias("x", "-oProxyCommand=evil").is_err());
        assert!(alias("x", "a b").is_err());
        assert!(RemoteHost::new("x", None, Some("h".into()), Some("-l".into()), None, None, None, false).is_err());
        assert!(RemoteHost::new("x", None, Some("h".into()), None, Some(0), None, None, false).is_err());
        assert!(RemoteHost::new("x", Some("a".into()), None, None, None, None, Some(0), false).is_err());
    }

    #[test]
    fn defaults() {
        let h = alias("x", "a").unwrap();
        assert_eq!(h.agent_port, 7682);
        assert!(!h.trust_secrets);
        assert_eq!(h.sbx_path, None);
        assert_eq!(h.sbx_program(), "sbx");
    }

    #[test]
    fn sbx_path_is_checked_like_other_words() {
        let h = alias("x", "a").unwrap().with_sbx_path(Some("/opt/sbx/bin/sbx".into())).unwrap();
        assert_eq!(h.sbx_program(), "/opt/sbx/bin/sbx");
        assert_eq!(h.clone().with_sbx_path(None).unwrap().sbx_path, None);
        for bad in ["", "-x", "a b", "a\nb"] {
            assert!(alias("x", "a").unwrap().with_sbx_path(Some(bad.into())).is_err(), "{bad:?}");
        }
        assert!(RemoteHosts::parse("[hosts.a]\nssh = \"x\"\nsbx_path = \"-oops\"\n").is_err());
    }

    #[test]
    fn argv_for_alias_and_direct() {
        let base = ["-o", "BatchMode=yes", "-o", "StrictHostKeyChecking=yes"];
        assert_eq!(alias("x", "bb").unwrap().ssh_argv(), [base.as_slice(), &["bb"]].concat());
        let h = RemoteHost::new("x", None, Some("10.0.0.5".into()), Some("tom".into()), Some(2222), Some("~/.ssh/k".into()), None, false).unwrap();
        assert_eq!(
            h.ssh_argv(),
            [base.as_slice(), &["-p", "2222", "-l", "tom", "-i", "~/.ssh/k", "10.0.0.5"]].concat()
        );
        let bare = RemoteHost::new("x", None, Some("h".into()), None, None, None, None, false).unwrap();
        assert_eq!(bare.ssh_argv(), [base.as_slice(), &["h"]].concat());
    }

    const SAMPLE: &str = r#"
[hosts.zed]
host = "10.0.0.5"
user = "tom"
port = 2222
key = "~/.ssh/id_ed25519"
agent_port = 9000
trust_secrets = true
sbx_path = "/opt/sbx/bin/sbx"
unknown = "dropped"

[hosts.buildbox]
ssh = "buildbox"
"#;

    #[test]
    fn parse_render_round_trip_is_stable_and_drops_unknown_keys() {
        let a = RemoteHosts::parse(SAMPLE).unwrap();
        assert_eq!(a.hosts.len(), 2);
        let text = a.render();
        assert!(!text.contains("unknown"));
        assert!(text.find("buildbox").unwrap() < text.find("zed").unwrap());
        let b = RemoteHosts::parse(&text).unwrap();
        assert_eq!(a, b);
        assert_eq!(b.get("zed").unwrap().sbx_path.as_deref(), Some("/opt/sbx/bin/sbx"));
        assert_eq!(b.get("buildbox").unwrap().sbx_path, None);
        assert!(!text[..text.find("[hosts.zed]").unwrap()].contains("sbx_path"));
        assert_eq!(text, b.render());
    }

    #[test]
    fn parse_applies_defaults_and_reports_rule_breaks() {
        let h = RemoteHosts::parse("[hosts.a]\nssh = \"x\"\n").unwrap();
        assert_eq!(h.get("a").unwrap().agent_port, 7682);
        assert!(RemoteHosts::parse("[hosts.local]\nssh = \"x\"\n").is_err());
        assert!(RemoteHosts::parse("[hosts.\"a b\"]\nssh = \"x\"\n").is_err());
        assert!(RemoteHosts::parse("[hosts.a]\nssh = \"x\"\nhost = \"y\"\n").is_err());
        assert!(RemoteHosts::parse("[hosts.a]\nssh = \"x\"\nuser = \"y\"\n").is_err());
        assert!(RemoteHosts::parse("not toml [").is_err());
        assert!(RemoteHosts::parse("").unwrap().hosts.is_empty());
    }

    #[test]
    fn render_escapes_quotes() {
        let h = RemoteHost::new("x", None, Some("h".into()), None, None, Some("a\"b\\c".into()), None, false).unwrap();
        let mut r = RemoteHosts::default();
        r.hosts.insert("x".into(), h);
        assert_eq!(RemoteHosts::parse(&r.render()).unwrap(), r);
    }
}
