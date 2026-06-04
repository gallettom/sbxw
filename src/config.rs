//! Project configuration (`sbxw.toml`), with Angular(4200)+Symfony(8000)
//! defaults tuned for the NEOS-style stack.

use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::Path;

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Web TTY bind address (host side).
    pub web_addr: String,
    /// When attaching the web terminal, run this instead of the agent if set.
    /// Empty => attach the Claude agent via `sbx run <name>`.
    pub web_shell: String,
    /// Network allowlist applied via `sbx policy allow network`.
    pub network_allow: Vec<String>,
    /// Optional explicit denylist applied via `sbx policy deny network`.
    pub network_deny: Vec<String>,
    /// Use one loopback IP per app (clean hostnames on natural ports).
    /// false => share 127.0.0.1 and reach apps at <alias>:<host_port>.
    pub ip_per_app: bool,
    /// Port mappings to publish + alias.
    pub ports: Vec<PortMap>,
    /// Kit files (paths or sbx kit references) applied via `sbx kit add` after
    /// the sandbox is created or on every `sbxw up`. Applied in order.
    /// Paths relative to sbxw.toml are resolved before passing to sbx.
    pub kits: Vec<String>,
    /// Subscription tier written into the injected OAuth credentials
    /// (`subscriptionType`). Match your actual plan: "pro", "max", "team",
    /// "enterprise", or "free". Wrong values mislabel the plan in-session.
    pub claude_subscription: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct PortMap {
    /// Friendly hostname written to /etc/hosts, e.g. "neos.local".
    pub alias: String,
    /// Port the service listens on *inside* the sandbox (bind 0.0.0.0!).
    pub sandbox_port: u16,
    /// Host port to expose. In ip_per_app mode this is usually == sandbox_port.
    pub host_port: u16,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            web_addr: "127.0.0.1:7681".into(),
            web_shell: String::new(),
            // Restrictive-but-usable local-dev allowlist. NOT "**".
            // Covers npm, pypi, packagist/composer, github, docker registries,
            // and the Anthropic API the agent itself needs.
            network_allow: vec![
                "*.npmjs.org".into(),
                "registry.npmjs.org".into(),
                "*.yarnpkg.com".into(),
                "pypi.org".into(),
                "*.pythonhosted.org".into(),
                "repo.packagist.org".into(),
                "*.packagist.org".into(),
                "getcomposer.org".into(),
                "github.com".into(),
                "*.githubusercontent.com".into(),
                "codeload.github.com".into(),
                "*.docker.io".into(),
                "*.docker.com".into(),
                "ghcr.io".into(),
                "api.anthropic.com".into(),
            ],
            network_deny: vec![],
            ip_per_app: false,
            ports: vec![
                PortMap { alias: "neos.local".into(), sandbox_port: 4200, host_port: 4200 },
                PortMap { alias: "api.neos.local".into(), sandbox_port: 8000, host_port: 8000 },
            ],
            kits: vec![],
            claude_subscription: "pro".into(),
        }
    }
}

impl Config {
    pub fn load_or_default(path: &Path) -> Result<Self> {
        if path.exists() {
            let raw = std::fs::read_to_string(path)
                .with_context(|| format!("reading {}", path.display()))?;
            let cfg: Config = toml::from_str(&raw)
                .with_context(|| format!("parsing {}", path.display()))?;
            Ok(cfg)
        } else {
            Ok(Config::default())
        }
    }
}
