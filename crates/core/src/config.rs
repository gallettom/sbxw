//! `sbxw.toml`.
//!
//! * `agent_addr` is the daemon's second listener, the only one sandboxes
//!   reach (why two: see the daemon's `http` module).
//! * A loaded config remembers **where it came from** (`source`): the daemon
//!   keeps one per sandbox, so a sandbox from project A keeps A's `env` even
//!   if the daemon started in project B.
//! * Relative paths (`kits`, `env_files`, `kit_mounts`, `workspaces`) resolve
//!   against the file's directory at load.

use crate::hosts::HostAlias;
use crate::kits::resolve_kit_ref;
use crate::ports::{self, WantedPort};
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub const DEFAULT_WEB_ADDR: &str = "127.0.0.1:7681";
pub const DEFAULT_AGENT_ADDR: &str = "127.0.0.1:7682";

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(default)]
pub struct Config {
    /// The UI listener: browser, island, extension.
    pub web_addr: String,
    /// The agent listener: the one address sandboxes are allowed to reach.
    pub agent_addr: String,
    /// Attach this instead of the agent in the main pane (`sbx exec -it … <cmd>`).
    pub web_shell: String,
    pub network_allow: Vec<String>,
    pub network_deny: Vec<String>,
    /// One loopback IP per app (clean hostnames on natural ports).
    pub ip_per_app: bool,
    pub ports: Vec<PortMap>,
    /// Kit references, applied at creation (`sbx create --kit`).
    pub kits: Vec<String>,
    /// Host dirs for kit-declared mounts (`host-mount@1`, sbx 0.50): sandbox
    /// target → host dir, passed as `--mount` at creation. sbx refuses a
    /// target no kit declares.
    pub kit_mounts: BTreeMap<String, String>,
    /// Where the main agent starts, inside the workspace (`sbx run --workdir`,
    /// 0.51): relative to the workspace, or absolute. Empty is the workspace.
    pub workdir: String,
    /// Extra host dirs mounted at creation, `PATH[:TARGET][:ro]` (0.51):
    /// `TARGET` absolute in the sandbox, else the host path; `:ro` read-only.
    /// `PATH` is relative to this file, or `~/`.
    pub workspaces: Vec<String>,
    /// CPUs and memory at creation (`--cpus`, `--memory`); 0 and empty are
    /// sbx's defaults.
    pub cpus: u32,
    pub memory: String,
    /// Only in an environment file (`sbxw env export`): `sandboxOptions` for
    /// FUSE and disk sizes. Unset is sbx's.
    pub fuse: Option<bool>,
    pub root_size: String,
    pub docker_size: String,
    /// Sandbox paths on persistent block storage (`volume@1`, sbx 0.50): caches
    /// and toolchains outside the workspace. They survive `sbxw rm` and
    /// reattach when a sandbox of the same name is recreated. Creation only.
    pub volumes: Vec<String>,
    /// Mount `~/.sbxw/cache` into new sandboxes, so bundled kits download once.
    pub cache: bool,
    /// The cache is emptied after this many days, so the next sandbox refills
    /// it with fresh packages and a bound on what a previous filler left.
    pub cache_ttl_days: u32,
    /// Passed as `ANTHROPIC_DEFAULT_MODEL`: a default, below a saved `/model`.
    pub claude_model: String,
    /// Models a thread may be started on.
    pub models: Vec<ModelChoice>,
    /// `off` | `readonly` | `readwrite`; empty leaves it to sbx.
    pub skills: String,
    /// `always` | `missing` | `never`; empty leaves it to sbx.
    pub pull: String,
    /// A saved template (`sbxw template save`) new sandboxes start from,
    /// `auto` for one sbxw keeps and rebuilds itself; empty is sbx's image.
    pub template: String,
    /// Hold the sandbox up with nobody attached (`sbx run -d`).
    pub keep_running: bool,
    /// Agent + threads one sandbox may hold open.
    pub max_agent_sessions: usize,
    /// macOS: hold the host awake with `caffeinate` — `remote` while Remote
    /// Control is on somewhere, `always`, or `off`.
    pub keep_awake: crate::power::KeepAwake,
    /// On battery at or below this percentage, the host is let go to sleep.
    pub keep_awake_min_battery: u8,
    /// On battery at or below this, Remote Control agents are told.
    pub battery_warn_below: u8,
    /// The host monitor pane's argv; empty hides it.
    pub monitor_cmd: Vec<String>,
    /// Terminal background, `#rrggbb`; empty keeps the built-in one. The UI's
    /// colour picker overrides it per browser.
    pub terminal_color: String,
    /// The bar above each terminal, `#rrggbb`; empty keeps darkslategrey.
    pub terminal_bar_color: String,
    /// Terminal text (default foreground and ANSI white), `#rrggbb` or `auto`;
    /// empty is `auto`, which follows the background's lightness.
    pub terminal_text_color: String,
    /// Environment baked in at creation and re-applied on every attach.
    pub env: BTreeMap<String, String>,
    /// `--env-file`s, in order.
    pub env_files: Vec<String>,
    /// The virtual Mac (`sbxw:terminal-mac`): read from the daemon's config.
    pub mac: crate::mac::MacSettings,
    /// The voice concierge (`[voice]`): read from the daemon's config.
    pub voice: crate::voice::VoiceSettings,

    /// The file this was read from, absolute. `None` for built-in defaults.
    #[serde(skip)]
    pub source: Option<PathBuf>,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
pub struct ModelChoice {
    pub id: String,
    pub label: String,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
pub struct PortMap {
    #[serde(default)]
    pub alias: String,
    pub sandbox_port: u16,
    pub host_port: u16,
}

pub fn default_models() -> Vec<ModelChoice> {
    [
        ("claude-sonnet-5-5", "Sonnet 5.5"),
        ("claude-opus-5-5", "Opus 5.5"),
        ("claude-haiku-5-5", "Haiku 5.5"),
    ]
    .into_iter()
    .map(|(id, label)| ModelChoice {
        id: id.into(),
        label: label.into(),
    })
    .collect()
}

impl Default for Config {
    fn default() -> Self {
        Self {
            web_addr: DEFAULT_WEB_ADDR.into(),
            agent_addr: DEFAULT_AGENT_ADDR.into(),
            web_shell: String::new(),
            // Restrictive-but-usable local-dev allowlist. NOT "**".
            network_allow: [
                "*.npmjs.org",
                "registry.npmjs.org",
                "*.yarnpkg.com",
                "pypi.org",
                "*.pythonhosted.org",
                "repo.packagist.org",
                "*.packagist.org",
                "getcomposer.org",
                "github.com",
                "*.githubusercontent.com",
                "codeload.github.com",
                "*.docker.io",
                "*.docker.com",
                "ghcr.io",
                "api.anthropic.com",
            ]
            .map(String::from)
            .to_vec(),
            network_deny: vec![],
            ip_per_app: false,
            // Empty: a project publishes only what its own sbxw.toml names; a
            // default would hand every sandbox another project's ports and
            // hostnames (and a sudo prompt).
            ports: vec![],
            kits: crate::kits::DEFAULT_KITS.map(String::from).to_vec(),
            kit_mounts: BTreeMap::new(),
            volumes: vec![],
            workdir: String::new(),
            workspaces: vec![],
            cpus: 0,
            memory: String::new(),
            fuse: None,
            root_size: String::new(),
            docker_size: String::new(),
            cache: true,
            cache_ttl_days: 7,
            claude_model: "claude-sonnet-5-5".into(),
            models: default_models(),
            skills: String::new(),
            pull: String::new(),
            template: String::new(),
            keep_running: false,
            max_agent_sessions: 4,
            keep_awake: crate::power::KeepAwake::Remote,
            keep_awake_min_battery: 5,
            battery_warn_below: 20,
            monitor_cmd: vec!["sbx".into()],
            terminal_color: String::new(),
            terminal_bar_color: String::new(),
            terminal_text_color: String::new(),
            env: BTreeMap::new(),
            env_files: vec![],
            mac: Default::default(),
            voice: Default::default(),
            source: None,
        }
    }
}

/// One `workspaces` entry, `PATH[:TARGET][:ro]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Workspace {
    pub path: String,
    pub target: Option<String>,
    pub readonly: bool,
}

impl Workspace {
    /// The argument `sbx create` takes for it.
    pub fn spec(&self) -> String {
        let mut s = self.path.clone();
        if let Some(t) = &self.target {
            s.push(':');
            s.push_str(t);
        }
        if self.readonly {
            s.push_str(":ro");
        }
        s
    }
}

/// `PATH[:TARGET][:ro]`. A path holding `:` cannot be told from its target,
/// so it is refused rather than guessed at.
pub fn parse_workspace(raw: &str) -> Result<Workspace, String> {
    let mut parts: Vec<&str> = raw.trim().split(':').collect();
    let readonly = parts.len() > 1 && parts.last() == Some(&"ro");
    if readonly {
        parts.pop();
    }
    let (path, target) = match parts.as_slice() {
        [p] => (p.to_string(), None),
        [p, t] => (p.to_string(), Some(t.to_string())),
        _ => return Err("expected PATH[:TARGET][:ro], with no ':' in either path".into()),
    };
    if path.is_empty() {
        return Err("the host path is empty".into());
    }
    if let Some(t) = &target {
        if !t.starts_with('/') {
            return Err("TARGET must be an absolute path inside the sandbox".into());
        }
    }
    Ok(Workspace { path, target, readonly })
}

/// `512m`, `8g`, `20G`, `1024`: what sbx reads as a size.
fn is_size(v: &str) -> bool {
    let v = v.trim();
    let digits = v.trim_end_matches(|c: char| "kKmMgGtT".contains(c));
    !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit()) && v.len() - digits.len() <= 1
}

pub const SKILL_MODES: [&str; 3] = ["off", "readonly", "readwrite"];
pub const PULL_POLICIES: [&str; 3] = ["always", "missing", "never"];

/// `<IPv4>:<port>` or `[<IPv6>]:<port>`, numeric, nothing else: the IP ends up
/// in `/etc/hosts` and in sbx network rules.
fn is_listen_addr(s: &str) -> bool {
    !s.chars().any(|c| c.is_whitespace() || c.is_control())
        && s.parse::<std::net::SocketAddr>().is_ok()
        && (s.starts_with('[') || crate::hosts::is_ipv4_literal(ports::ip_of(s)))
}

/// Why one `network_allow` / `network_deny` entry is refused, if it is: each
/// is joined with `,` into one `sbx policy` argument, so a comma, a space or a
/// leading `-` would change what sbx is told.
fn network_rule_error(rule: &str) -> Option<&'static str> {
    if rule.is_empty() {
        Some("an empty rule")
    } else if rule == "*" || rule == "**" {
        Some("a wildcard that matches every host; name the hosts instead")
    } else if rule.starts_with('-') {
        Some("a rule may not start with '-' (sbx would read it as an option)")
    } else if rule.chars().any(|c| c.is_whitespace() || c.is_control() || c == ',') {
        Some("a rule may not hold whitespace, a comma or a control character")
    } else {
        None
    }
}

/// `#rrggbb` only: the value is templated into the page as a JS string, so
/// nothing else may pass.
fn is_hex_color(s: &str) -> bool {
    s.len() == 7 && s.starts_with('#') && s[1..].bytes().all(|b| b.is_ascii_hexdigit())
}
const MODEL_VAR: &str = "ANTHROPIC_DEFAULT_MODEL";

/// Host dir from config: `~/` is home; relative paths are against the config's
/// directory.
fn resolve_host_dir(dir: &Path, host: &str) -> String {
    if let Some(rest) = host.strip_prefix("~/") {
        if let Some(home) = std::env::var_os("HOME") {
            return Path::new(&home).join(rest).to_string_lossy().into_owned();
        }
    }
    if Path::new(host).is_absolute() {
        host.to_string()
    } else {
        dir.join(host).to_string_lossy().into_owned()
    }
}

impl Config {
    /// `--mount HOST:TARGET` arguments for the kit-declared host mounts.
    pub fn kit_mount_specs(&self) -> Vec<String> {
        self.kit_mounts.iter().map(|(target, host)| format!("{host}:{target}")).collect()
    }

    /// Parses and validates `raw` without resolving paths; a rewritten file
    /// must pass this before it replaces the one on disk.
    pub fn parse(raw: &str) -> Result<Self> {
        let cfg: Config = toml::from_str(raw)?;
        cfg.validate()?;
        Ok(cfg)
    }

    /// Read `path`, or the defaults when it does not exist; validate; resolve
    /// relative paths against the file's directory.
    pub fn load(path: &Path, cwd: &Path) -> Result<Self> {
        let abs = if path.is_absolute() {
            path.to_path_buf()
        } else {
            cwd.join(path)
        };
        if !abs.exists() {
            return Ok(Config::default());
        }
        let raw = std::fs::read_to_string(&abs).with_context(|| format!("reading {}", abs.display()))?;
        let mut cfg: Config =
            toml::from_str(&raw).with_context(|| format!("parsing {}", abs.display()))?;
        cfg.validate().with_context(|| abs.display().to_string())?;
        let dir = abs.parent().unwrap_or(Path::new("/")).to_path_buf();
        cfg.kits = std::mem::take(&mut cfg.kits)
            .into_iter()
            .map(|k| resolve_kit_ref(&dir, k))
            .collect();
        cfg.env_files = std::mem::take(&mut cfg.env_files)
            .into_iter()
            .map(|f| {
                if Path::new(&f).is_absolute() {
                    f
                } else {
                    dir.join(f).to_string_lossy().into_owned()
                }
            })
            .collect();
        cfg.kit_mounts = std::mem::take(&mut cfg.kit_mounts)
            .into_iter()
            .map(|(target, host)| (target, resolve_host_dir(&dir, &host)))
            .collect();
        cfg.workspaces = std::mem::take(&mut cfg.workspaces)
            .into_iter()
            .map(|w| match parse_workspace(&w) {
                Ok(ws) => Workspace { path: resolve_host_dir(&dir, &ws.path), ..ws }.spec(),
                Err(_) => w,
            })
            .collect();
        cfg.source = Some(abs);
        Ok(cfg)
    }

    /// Checked here, not left to sbx: sbx would fail the sandbox, not the
    /// setting, and only when a new one is created.
    pub fn validate(&self) -> Result<()> {
        self.mac.validate().map_err(anyhow::Error::msg)?;
        self.voice.validate().map_err(anyhow::Error::msg)?;
        if let Some(mode) = self.skills_mode() {
            if !SKILL_MODES.contains(&mode) {
                bail!("skills = \"{mode}\" — expected one of {}", SKILL_MODES.join(", "));
            }
        }
        if let Some(p) = self.pull_policy() {
            if !PULL_POLICIES.contains(&p) {
                bail!("pull = \"{p}\" — expected one of {}", PULL_POLICIES.join(", "));
            }
        }
        for (key, v) in [("web_addr", &self.web_addr), ("agent_addr", &self.agent_addr)] {
            if !is_listen_addr(v) {
                bail!("{key} = {v:?} — expected IP:PORT, such as 127.0.0.1:7681 or [::1]:7681");
            }
        }
        if ports::port_of(&self.web_addr) == ports::port_of(&self.agent_addr)
            && ports::ip_of(&self.web_addr) == ports::ip_of(&self.agent_addr)
        {
            bail!(
                "web_addr and agent_addr are both {} — sandboxes must reach the agent \
                 listener and nothing else, so they need two ports",
                self.web_addr
            );
        }
        for (key, rules) in [("network_allow", &self.network_allow), ("network_deny", &self.network_deny)] {
            for r in rules {
                if let Some(why) = network_rule_error(r) {
                    bail!("{key}: {r:?} — {why}");
                }
            }
        }
        let web_port = self.web_port();
        for r in &self.network_allow {
            let opens_ui = ["localhost", "127.0.0.1", "host.docker.internal", "[::1]"]
                .iter()
                .any(|host| r.eq_ignore_ascii_case(&format!("{host}:{web_port}")));
            if opens_ui {
                bail!(
                    "network_allow: {r:?} — that is the UI listener (web_addr), which no sandbox may \
                     reach: it approves and releases everything. Sandboxes talk to agent_addr"
                );
            }
        }
        for k in &self.kits {
            if let Some(name) = crate::kits::bundled_name(k) {
                if !crate::kits::BUNDLED.contains(&name) {
                    bail!("kits: {k:?} — the bundled kits are {}", crate::kits::BUNDLED.map(|b| format!("sbxw:{b}")).join(", "));
                }
            }
        }
        for w in &self.workspaces {
            parse_workspace(w).map_err(|e| anyhow::anyhow!("workspaces: {w:?} — {e}"))?;
        }
        for (key, v) in [("memory", &self.memory), ("root_size", &self.root_size), ("docker_size", &self.docker_size)] {
            if !v.is_empty() && !is_size(v) {
                bail!("{key}: {v:?} — a size in binary units, like 512m or 8g");
            }
        }
        if self.workdir.contains('\n') || self.workdir.contains('\0') {
            bail!("workdir: one path, on one line");
        }
        for (target, host) in &self.kit_mounts {
            if !target.starts_with('/') {
                bail!("kit_mounts: {target:?} — the sandbox target must be an absolute path, as the kit declares it");
            }
            if host.trim().is_empty() || host.contains(':') || target.contains(':') {
                bail!("kit_mounts: {target:?} = {host:?} — expected a host directory, with no ':'");
            }
        }
        for v in &self.volumes {
            if !v.starts_with('/') || v.trim_end_matches('/').is_empty() {
                bail!("volumes: {v:?} — expected an absolute path inside the sandbox, such as /home/agent/.npm");
            }
        }
        for m in &self.models {
            if !crate::names::is_valid_model_id(&m.id) {
                bail!("models: {:?} is not a model id", m.id);
            }
        }
        for (key, v) in [("terminal_color", &self.terminal_color), ("terminal_bar_color", &self.terminal_bar_color)] {
            if !v.is_empty() && !is_hex_color(v) {
                bail!("{key} = {v:?} — expected #rrggbb");
            }
        }
        let t = &self.terminal_text_color;
        if !t.is_empty() && t != "auto" && !is_hex_color(t) {
            bail!("terminal_text_color = {t:?} — expected #rrggbb or auto");
        }
        for (key, v) in [("keep_awake_min_battery", self.keep_awake_min_battery), ("battery_warn_below", self.battery_warn_below)] {
            if v > 100 {
                bail!("{key} = {v} — expected a percentage, 0 to 100");
            }
        }
        for p in &self.ports {
            if !p.alias.is_empty() && !crate::hosts::is_hostname(&p.alias) {
                bail!("ports: alias {:?} is not a hostname", p.alias);
            }
        }
        Ok(())
    }

    pub fn skills_mode(&self) -> Option<&str> {
        match self.skills.trim() {
            "" => None,
            mode => Some(mode),
        }
    }

    pub fn cache_ttl(&self) -> std::time::Duration {
        std::time::Duration::from_secs(u64::from(self.cache_ttl_days) * 86_400)
    }

    pub fn pull_policy(&self) -> Option<&str> {
        Some(self.pull.trim()).filter(|p| !p.is_empty())
    }

    pub fn template_mode(&self) -> crate::templates::Mode {
        crate::templates::mode(&self.template)
    }

    /// `env`, plus `ANTHROPIC_DEFAULT_MODEL` from `claude_model` unless `env`
    /// spells it itself.
    pub fn effective_env(&self) -> BTreeMap<String, String> {
        let mut env = self.env.clone();
        if !self.claude_model.is_empty() {
            env.entry(MODEL_VAR.into())
                .or_insert_with(|| self.claude_model.clone());
        }
        env
    }

    /// `KEY=VALUE` for sbx's `-e`, in sorted order (stable argv, diffable logs).
    pub fn env_pairs(&self) -> Vec<String> {
        self.effective_env()
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect()
    }

    /// The configured mappings, then `extra` (from the web UI), in that order —
    /// which decides each app's loopback IP in `ip_per_app` mode.
    pub fn wanted_ports(&self, extra: &[WantedPort]) -> Vec<WantedPort> {
        self.ports
            .iter()
            .map(|p| WantedPort {
                host_port: p.host_port,
                sandbox_port: p.sandbox_port,
                alias: p.alias.clone(),
            })
            .chain(extra.iter().cloned())
            .collect()
    }

    /// Every hostname this config wants in `/etc/hosts`: its port aliases and
    /// the web UI's own name when it is on loopback.
    pub fn wanted_aliases(&self, extra: &[WantedPort]) -> Vec<HostAlias> {
        let mut out = ports::host_aliases(&self.wanted_ports(extra), self.ip_per_app);
        out.extend(ports::web_alias(&self.web_addr));
        out
    }

    pub fn web_port(&self) -> &str {
        ports::port_of(&self.web_addr)
    }

    pub fn agent_port(&self) -> &str {
        ports::port_of(&self.agent_addr)
    }

    pub fn max_sessions(&self) -> usize {
        self.max_agent_sessions.max(1)
    }

    /// Back to TOML, every table after every bare key (serde would emit them in
    /// declaration order, which TOML refuses).
    pub fn to_toml(&self) -> Result<String> {
        let value = toml::Value::try_from(self).context("encoding the config")?;
        toml::to_string(&value).context("rendering the config as TOML")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_config_survives_being_written_and_read_again() {
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = Config::default();
        cfg.env.insert("A".into(), "x=y z".into());
        cfg.env_files = vec!["/abs/.env".into()];
        let path = dir.path().join("sbxw.toml");
        std::fs::write(&path, cfg.to_toml().unwrap()).unwrap();
        let back = Config::load(&path, dir.path()).unwrap();
        assert_eq!(back.env, cfg.env);
        assert_eq!(back.ports, cfg.ports);
        assert_eq!(back.source.as_deref(), Some(path.as_path()));
    }

    #[test]
    fn a_missing_file_is_the_defaults() {
        let cfg = Config::load(Path::new("/nonexistent/sbxw.toml"), Path::new("/")).unwrap();
        assert_eq!(cfg.web_addr, DEFAULT_WEB_ADDR);
        assert_eq!(cfg.agent_addr, DEFAULT_AGENT_ADDR);
        assert!(cfg.source.is_none());
    }

    #[test]
    fn relative_paths_resolve_against_the_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("kit")).unwrap();
        std::fs::write(
            dir.path().join("sbxw.toml"),
            "kits = [\"./kit\", \"docker.io/sbx/k:1\"]\nenv_files = [\".env\"]\n",
        )
        .unwrap();
        let cfg = Config::load(Path::new("sbxw.toml"), dir.path()).unwrap();
        let canon = std::fs::canonicalize(dir.path().join("kit")).unwrap();
        assert_eq!(cfg.kits[0], canon.to_string_lossy());
        assert_eq!(cfg.kits[1], "docker.io/sbx/k:1");
        assert_eq!(cfg.env_files[0], dir.path().join(".env").to_string_lossy());
    }

    #[test]
    fn kit_mounts_resolve_and_become_mount_specs() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("sbxw.toml"), "[kit_mounts]\n\"/home/agent/data\" = \"./data\"\n").unwrap();
        let cfg = Config::load(Path::new("sbxw.toml"), dir.path()).unwrap();
        assert_eq!(cfg.kit_mount_specs(), [format!("{}:/home/agent/data", dir.path().join("./data").display())]);
        for bad in ["[kit_mounts]\n\"data\" = \"/x\"\n", "[kit_mounts]\n\"/d\" = \"/x:ro\"\n"] {
            assert!(toml::from_str::<Config>(bad).unwrap().validate().is_err(), "{bad}");
        }
    }

    #[test]
    fn the_example_file_parses() {
        let raw = include_str!("../../../sbxw.toml.example");
        let cfg: Config = toml::from_str(raw).unwrap();
        cfg.validate().unwrap();
        assert_eq!(cfg.models, default_models());
    }

    #[test]
    fn unknown_modes_and_a_shared_port_are_refused() {
        let bad = |s: &str| toml::from_str::<Config>(s).unwrap().validate().is_err();
        assert!(bad("skills = \"sometimes\""));
        assert!(bad("pull = \"weekly\""));
        assert!(bad("agent_addr = \"127.0.0.1:7681\""));
        assert!(bad("models = [{ id = \"Bad Id\", label = \"x\" }]"));
        assert!(!bad("skills = \"off\""));
    }

    #[test]
    fn listen_addresses_are_an_ip_and_a_port() {
        let bad = |s: &str| toml::from_str::<Config>(s).unwrap().validate().is_err();
        for addr in [
            "127.0.0.1\\n93.184.216.34 github.com:7681",
            "127.0.0.1 x:7681",
            "localhost:7681",
            "127.0.0.1",
            "127.0.0.1:http",
            "::1:7681",
            "",
        ] {
            assert!(bad(&format!("web_addr = \"{addr}\"")), "web_addr {addr:?}");
            assert!(bad(&format!("agent_addr = \"{addr}\"")), "agent_addr {addr:?}");
        }
        assert!(!bad("web_addr = \"[::1]:7681\""));
        assert!(!bad("web_addr = \"192.168.1.2:7681\"\nagent_addr = \"127.0.0.1:7690\""));
    }

    #[test]
    fn network_rules_are_single_hosts_and_never_the_ui_port() {
        Config::default().validate().unwrap();
        let err = |s: &str| toml::from_str::<Config>(s).unwrap().validate().err().map(|e| e.to_string());
        for rule in ["*", "**", "-x", "a b", "a,b", "a\\nb", "a\\u001bb", ""] {
            assert!(err(&format!("network_allow = [\"{rule}\"]")).is_some(), "allow {rule:?}");
            assert!(err(&format!("network_deny = [\"{rule}\"]")).is_some(), "deny {rule:?}");
        }
        for ui in ["localhost:7681", "127.0.0.1:7681", "host.docker.internal:7681", "[::1]:7681", "LOCALHOST:7681"] {
            let e = err(&format!("network_allow = [\"{ui}\"]")).expect(ui);
            assert!(e.contains("UI listener"), "{e}");
        }
        assert!(err("network_allow = [\"localhost:7682\", \"*.npmjs.org\"]").is_none());
        assert!(err("web_addr = \"127.0.0.1:9000\"\nnetwork_allow = [\"localhost:9000\"]").is_some());
        assert!(err("network_deny = [\"localhost:7681\"]").is_none());
    }

    #[test]
    fn terminal_color_is_a_hex_colour_or_nothing() {
        let mut cfg = Config::default();
        assert!(cfg.validate().is_ok());
        cfg.terminal_color = "#1A1b26".into();
        assert!(cfg.validate().is_ok());
        for bad in ["navy", "#fff", "#12345g", "\"#000000"] {
            cfg.terminal_color = bad.into();
            assert!(cfg.validate().is_err(), "{bad} was accepted");
            cfg.terminal_color = String::new();
            cfg.terminal_bar_color = bad.into();
            assert!(cfg.validate().is_err(), "{bad} was accepted as a bar colour");
            cfg.terminal_bar_color = String::new();
            cfg.terminal_text_color = bad.into();
            assert!(cfg.validate().is_err(), "{bad} was accepted as a text colour");
            cfg.terminal_text_color = String::new();
        }
        for good in ["auto", "#FFFFFF"] {
            cfg.terminal_text_color = good.into();
            assert!(cfg.validate().is_ok(), "{good} was refused as a text colour");
        }
    }

    #[test]
    fn workspaces_are_path_target_and_ro() {
        assert_eq!(parse_workspace("../docs").unwrap(), Workspace { path: "../docs".into(), target: None, readonly: false });
        let w = parse_workspace("/data:/workspace/data:ro").unwrap();
        assert_eq!((w.target.as_deref(), w.readonly), (Some("/workspace/data"), true));
        assert_eq!(w.spec(), "/data:/workspace/data:ro");
        assert_eq!(parse_workspace("/a:ro").unwrap().spec(), "/a:ro");
        assert!(parse_workspace("/a:rel").is_err(), "a target is absolute");
        assert!(parse_workspace("/a:/b:/c").is_err());
        let bad: Config = toml::from_str("memory = \"lots\"").unwrap();
        assert!(bad.validate().is_err());
        let ok: Config = toml::from_str("memory = \"8g\"\ncpus = 4\nworkspaces = [\"/d:/w/d:ro\"]\nworkdir = \"web\"").unwrap();
        assert!(ok.validate().is_ok());
    }

    #[test]
    fn the_model_rides_in_env_unless_env_names_it() {
        let cfg = Config::default();
        assert!(cfg.env_pairs().contains(&"ANTHROPIC_DEFAULT_MODEL=claude-sonnet-5-5".to_string()));
        let mut cfg = Config::default();
        cfg.env.insert(MODEL_VAR.into(), "opus".into());
        assert_eq!(cfg.effective_env()[MODEL_VAR], "opus");
    }

    #[test]
    fn bundled_kits_are_on_by_default_and_checked() {
        assert_eq!(Config::default().kits, ["sbxw:codemap", "sbxw:agent-skills", "sbxw:md-to-pdf-tools", "sbxw:retro"]);
        let bad: Config = toml::from_str("kits = [\"sbxw:nope\"]").unwrap();
        assert!(bad.validate().is_err());
        let ok: Config = toml::from_str("kits = [\"sbxw:k8s-tools\", \"/abs/kit\"]").unwrap();
        ok.validate().unwrap();
    }

    #[test]
    fn keep_awake_is_a_mode_and_its_floors_are_percentages() {
        assert_eq!(Config::default().keep_awake, crate::power::KeepAwake::Remote);
        let cfg: Config = toml::from_str("keep_awake = \"always\"\nkeep_awake_min_battery = 10").unwrap();
        assert_eq!(cfg.keep_awake, crate::power::KeepAwake::Always);
        cfg.validate().unwrap();
        assert!(toml::from_str::<Config>("keep_awake = \"sometimes\"").is_err());
        let bad: Config = toml::from_str("battery_warn_below = 120").unwrap();
        assert!(bad.validate().is_err());
    }

    #[test]
    fn without_a_config_nothing_is_published_but_the_web_name() {
        let cfg = Config::default();
        assert!(cfg.ports.is_empty());
        let names: Vec<String> = cfg.wanted_aliases(&[]).into_iter().map(|a| a.hostname).collect();
        assert_eq!(names, ["sbxw.localhost"]);
    }

    #[test]
    fn wanted_aliases_add_the_web_name_on_loopback() {
        let cfg: Config = toml::from_str(
            "[[ports]]\nalias = \"myapp.local\"\nsandbox_port = 4200\nhost_port = 4200\n",
        )
        .unwrap();
        let names: Vec<String> = cfg.wanted_aliases(&[]).into_iter().map(|a| a.hostname).collect();
        assert_eq!(names, ["myapp.local", "sbxw.localhost"]);
    }
}
