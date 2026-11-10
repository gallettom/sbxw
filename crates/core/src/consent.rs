//! What an `sbxw.toml` asks of the host, as lines a person reads and approves.
//!
//! The file usually sits in the workspace, where the agent (or a repository
//! just cloned) can write it. So the part of it that reaches the host or
//! widens the sandbox (mounts, host files, local kits, network, a host
//! command, the listeners, `/etc/hosts` names) takes effect only once a person
//! has seen it and said yes, as with `direnv allow`.
//!
//! * [`grants`] turns a config into sorted, canonical lines. A value equal to
//!   the built-in default produces none, so a config with nothing special asks
//!   nothing.
//! * [`new_grants`] is what the current lines add to an approved list.
//!   Removing a grant needs no approval.
//! * The lines themselves are what is stored and compared: an approval names
//!   the exact lines shown, so an edit between showing and approving is caught.

use crate::config::Config;
use std::collections::BTreeSet;

/// Every host-access grant in `cfg`, sorted and deduplicated.
///
/// Fields left out, and why (checked by `every_field_is_classified` below):
/// `web_shell`, `workdir`, `env`, `volumes`, `claude_model`, `models`,
/// `pull`, `keep_running`, `network_deny`, `cpus`, `memory`, `fuse`,
/// `root_size`, `docker_size`, `max_agent_sessions`, `keep_awake*`,
/// `battery_warn_below`, `terminal_*_color`, `voice`, the other `[mac]`
/// timings and `cache`. See docs/security.md for the reasons.
pub fn grants(cfg: &Config) -> Vec<String> {
    let mut out: BTreeSet<String> = daemon_grants(cfg).into_iter().collect();
    let d = Config::default();

    for w in &cfg.workspaces {
        out.insert(match crate::config::parse_workspace(w) {
            Ok(ws) => {
                let mode = if ws.readonly { "read-only" } else { "read-write" };
                match &ws.target {
                    Some(t) => format!("mount ({mode}): {} at {t}", ws.path),
                    None => format!("mount ({mode}): {}", ws.path),
                }
            }
            // `validate` refuses these; shown raw rather than dropped.
            Err(_) => format!("mount: {w}"),
        });
    }
    for (target, host) in &cfg.kit_mounts {
        out.insert(format!("mount (read-write): {host} at {target}"));
    }
    for f in &cfg.env_files {
        out.insert(format!("env file: {f}"));
    }
    for k in cfg.kits.iter().filter(|k| !d.kits.contains(k)) {
        out.insert(if crate::kits::bundled_name(k).is_some() {
            format!("kit: {k}")
        } else if is_local_ref(k) {
            format!("local kit: {k}")
        } else {
            format!("remote kit: {k}")
        });
    }
    for r in cfg.network_allow.iter().filter(|r| !d.network_allow.contains(r)) {
        out.insert(format!("network: {r}"));
    }
    for p in &cfg.ports {
        out.insert(format!("published port: host {} -> sandbox {}", p.host_port, p.sandbox_port));
        if !p.alias.is_empty() {
            out.insert(format!("hosts alias: {}", p.alias));
        }
    }
    if cfg.ip_per_app {
        out.insert("loopback addresses: one 127.0.0.X per app (ip_per_app)".into());
    }
    if cfg.skills_mode() == Some("readwrite") {
        out.insert("skills: readwrite (the agent may add skills other sandboxes load)".into());
    }
    if let crate::templates::Mode::Tag(t) = cfg.template_mode() {
        out.insert(format!("template: {t}"));
    }
    if cfg.cache_ttl_days > d.cache_ttl_days {
        out.insert(format!("kit cache kept {} days (default {})", cfg.cache_ttl_days, d.cache_ttl_days));
    }
    out.into_iter().collect()
}

/// The grants of the keys the daemon reads when it starts: its listeners, the
/// Monitor pane's host command and the virtual Mac. The CLI checks these
/// before it spawns a daemon with the file.
pub fn daemon_grants(cfg: &Config) -> Vec<String> {
    let d = Config::default();
    let mut out = BTreeSet::new();
    if cfg.web_addr != d.web_addr {
        out.insert(format!("listener: web_addr = {}", cfg.web_addr));
    }
    if cfg.agent_addr != d.agent_addr {
        out.insert(format!("listener: agent_addr = {}", cfg.agent_addr));
    }
    if cfg.monitor_cmd != d.monitor_cmd && !cfg.monitor_cmd.is_empty() {
        out.insert(format!("host command (Monitor pane): {}", shell_words(&cfg.monitor_cmd)));
    }
    if cfg.mac.vm != d.mac.vm {
        out.insert(format!("virtual Mac: VM {}", cfg.mac.vm));
    }
    if cfg.mac.guest_host_ip != d.mac.guest_host_ip {
        out.insert(format!("virtual Mac: ports published to it on host address {}", cfg.mac.guest_host_ip));
    }
    out.into_iter().collect()
}

/// The lines of `current` not in `approved`, in order.
pub fn new_grants(current: &[String], approved: &[String]) -> Vec<String> {
    current.iter().filter(|g| !approved.contains(g)).cloned().collect()
}

/// A kit reference that is a path on the host (resolved at load), not an
/// OCI image or a git URL.
fn is_local_ref(k: &str) -> bool {
    k.starts_with('/') || k.starts_with("./") || k.starts_with("../") || k.starts_with("~/")
}

/// argv as a person would type it: each word quoted when it needs to be, so
/// `["sh", "-c", "a b"]` reads `sh -c 'a b'`.
fn shell_words(argv: &[String]) -> String {
    argv.iter()
        .map(|a| {
            let plain = !a.is_empty() && a.chars().all(|c| c.is_ascii_alphanumeric() || "-_./=:@%+,".contains(c));
            if plain { a.clone() } else { format!("'{}'", a.replace('\'', r"'\''")) }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// A bring-up refused because its config asks for host access the person has
/// not approved for that file. Carries the new lines, so the CLI and the web
/// UI can show them and send them back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NeedsApproval {
    /// The config file the lines are approved for (its canonical path).
    pub config: String,
    pub grants: Vec<String>,
}

impl std::fmt::Display for NeedsApproval {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "{} asks for host access you have not approved yet:", self.config)?;
        for g in &self.grants {
            writeln!(f, "  {g}")?;
        }
        write!(f, "Approve it with `sbxw up` in a terminal, or with Allow in the web UI.")
    }
}

impl std::error::Error for NeedsApproval {}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(toml: &str) -> Config {
        let c: Config = toml::from_str(toml).unwrap();
        c.validate().unwrap();
        c
    }

    fn lines(toml: &str) -> Vec<String> {
        grants(&cfg(toml))
    }

    #[test]
    fn the_defaults_and_the_example_grant_nothing() {
        assert!(grants(&Config::default()).is_empty());
        let example: Config = toml::from_str(include_str!("../../../sbxw.toml.example")).unwrap();
        assert_eq!(grants(&example), Vec::<String>::new());
        // Writing a default out in full is still the default.
        assert!(lines("network_allow = [\"github.com\"]\nkits = [\"sbxw:retro\"]\nmonitor_cmd = [\"sbx\"]\ncache_ttl_days = 3").is_empty());
    }

    #[test]
    fn mounts_and_host_files() {
        assert_eq!(lines("workspaces = [\"/data\", \"/docs:/w/docs:ro\"]"), [
            "mount (read-only): /docs at /w/docs",
            "mount (read-write): /data",
        ]);
        assert_eq!(lines("[kit_mounts]\n\"/home/agent/d\" = \"/Users/me/d\""), ["mount (read-write): /Users/me/d at /home/agent/d"]);
        assert_eq!(lines("env_files = [\"/Users/me/.aws/credentials\"]"), ["env file: /Users/me/.aws/credentials"]);
    }

    #[test]
    fn kits_beyond_the_defaults() {
        assert_eq!(lines("kits = [\"sbxw:codemap\", \"sbxw:k8s-tools\", \"/abs/kit\", \"ghcr.io/o/k:1\"]"), [
            "kit: sbxw:k8s-tools",
            "local kit: /abs/kit",
            "remote kit: ghcr.io/o/k:1",
        ]);
        assert!(lines("kits = []").is_empty(), "fewer kits grant nothing");
    }

    #[test]
    fn network_ports_and_names() {
        assert_eq!(lines("network_allow = [\"github.com\", \"api.example.com\"]"), ["network: api.example.com"]);
        assert!(lines("network_deny = [\"github.com\"]").is_empty());
        assert_eq!(lines("ip_per_app = true\n[[ports]]\nalias = \"app.local\"\nsandbox_port = 4200\nhost_port = 4201"), [
            "hosts alias: app.local",
            "loopback addresses: one 127.0.0.X per app (ip_per_app)",
            "published port: host 4201 -> sandbox 4200",
        ]);
    }

    #[test]
    fn the_daemon_keys() {
        let c = cfg("web_addr = \"0.0.0.0:7681\"\nagent_addr = \"127.0.0.1:9000\"\nmonitor_cmd = [\"sh\", \"-c\", \"curl x | sh\"]\n[mac]\nvm = \"personal\"\nguest_host_ip = \"192.168.1.2\"");
        let want = [
            "host command (Monitor pane): sh -c 'curl x | sh'",
            "listener: agent_addr = 127.0.0.1:9000",
            "listener: web_addr = 0.0.0.0:7681",
            "virtual Mac: VM personal",
            "virtual Mac: ports published to it on host address 192.168.1.2",
        ];
        assert_eq!(daemon_grants(&c), want);
        assert_eq!(grants(&c), want);
        assert!(lines("monitor_cmd = []").is_empty(), "hiding the pane grants nothing");
    }

    #[test]
    fn sharing_between_sandboxes() {
        assert_eq!(lines("skills = \"readwrite\""), ["skills: readwrite (the agent may add skills other sandboxes load)"]);
        assert!(lines("skills = \"readonly\"").is_empty());
        assert_eq!(lines("template = \"other:base\""), ["template: other:base"]);
        assert!(lines("template = \"auto\"").is_empty());
        assert_eq!(lines("cache_ttl_days = 365"), ["kit cache kept 365 days (default 7)"]);
    }

    /// Every other key, set away from its default, grants nothing.
    #[test]
    fn every_field_is_classified() {
        let harmless = "web_shell = \"bash\"\nworkdir = \"web\"\nvolumes = [\"/home/agent/.npm\"]\nclaude_model = \"opus\"\n\
            models = [{ id = \"claude-opus-5-5\", label = \"O\" }]\npull = \"never\"\nkeep_running = true\nnetwork_deny = [\"x.com\"]\n\
            cpus = 8\nmemory = \"16g\"\nfuse = false\nroot_size = \"40g\"\ndocker_size = \"60g\"\nmax_agent_sessions = 9\n\
            keep_awake = \"always\"\nkeep_awake_min_battery = 1\nbattery_warn_below = 50\nterminal_color = \"#000000\"\n\
            terminal_bar_color = \"#111111\"\nterminal_text_color = \"auto\"\ncache = false\nskills = \"off\"\n\
            [env]\nHTTP_PROXY = \"http://x\"\n[mac]\nidle_release_minutes = 1\npause_after_minutes = 2\nstop_after_minutes = 3\nforce_stop_after_minutes = 4\n\
            [voice]\npush_to_talk_key = \"F7\"\n";
        assert!(lines(harmless).is_empty(), "{:?}", lines(harmless));
        // A field added to Config later must be classified here and in `grants`:
        // this fails when the default serialises a key the list does not name.
        let known = [
            "web_addr", "agent_addr", "web_shell", "network_allow", "network_deny", "ip_per_app", "ports", "kits", "kit_mounts",
            "workdir", "workspaces", "cpus", "memory", "fuse", "root_size", "docker_size", "volumes", "cache", "cache_ttl_days",
            "claude_model", "models", "skills", "pull", "template", "keep_running", "max_agent_sessions", "keep_awake",
            "keep_awake_min_battery", "battery_warn_below", "monitor_cmd", "terminal_color", "terminal_bar_color",
            "terminal_text_color", "env", "env_files", "mac", "voice",
        ];
        let v = toml::Value::try_from(Config { fuse: Some(true), ..Config::default() }).unwrap();
        for key in v.as_table().unwrap().keys() {
            assert!(known.contains(&key.as_str()), "classify the new key `{key}` in consent::grants");
        }
    }

    #[test]
    fn only_new_lines_need_approval() {
        let approved = vec!["a".to_string(), "b".into()];
        assert_eq!(new_grants(&["a".into(), "c".into()], &approved), ["c"]);
        assert!(new_grants(&["a".into()], &approved).is_empty(), "a removal asks nothing");
    }

    #[test]
    fn the_refusal_lists_the_lines() {
        let e = NeedsApproval { config: "/p/sbxw.toml".into(), grants: vec!["network: x.com".into()] };
        let s = e.to_string();
        assert!(s.contains("/p/sbxw.toml") && s.contains("  network: x.com"), "{s}");
    }
}
