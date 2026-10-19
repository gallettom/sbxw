//! Between `sbxw.toml` and an `sbxenv.yaml`: exporting one, and folding one
//! back onto the config when running from it.
//!
//! Export rules: the file lands *outside* the workspace (the agent can rewrite
//! anything inside it, and this file decides the next sandbox), every path is
//! relative to the file (sbx resolves them that way, so the file travels), and
//! everything the format cannot express is written in its header with the
//! command that reproduces it — an export that silently dropped the allowlist
//! would describe a sandbox with *broader* egress than sbxw builds.

use crate::config::{Config, PortMap};
use crate::envfile::{self, EnvFile, Port};
use crate::ports;
use std::path::{Component, Path, PathBuf};

/// The directory an export goes in: `explicit` when it strictly contains the
/// workspace, else the workspace's parent — never the workspace itself.
pub fn export_dir_for(workspace: &Path, explicit: Option<&Path>) -> PathBuf {
    if let Some(d) = explicit.filter(|d| workspace.starts_with(d) && *d != workspace) {
        return d.to_path_buf();
    }
    workspace.parent().map(Path::to_path_buf).unwrap_or_else(|| workspace.to_path_buf())
}

/// `path` as a file in `dir` should spell it: relative (`./neos`,
/// `../kits/x`), or absolute when the two share nothing but the root.
pub fn path_relative_to(dir: &Path, path: &Path) -> String {
    let d: Vec<Component> = dir.components().collect();
    let p: Vec<Component> = path.components().collect();
    let common = d.iter().zip(&p).take_while(|(a, b)| a == b).count();
    let only_root = d
        .iter()
        .take(common)
        .all(|c| matches!(c, Component::RootDir | Component::Prefix(_)));
    if only_root {
        return path.to_string_lossy().into_owned();
    }
    let mut rel = PathBuf::new();
    for _ in common..d.len() {
        rel.push("..");
    }
    for c in &p[common..] {
        rel.push(c);
    }
    let rel = rel.to_string_lossy().into_owned();
    if rel.is_empty() {
        ".".into()
    } else if rel.starts_with("..") {
        rel
    } else {
        format!("./{rel}")
    }
}

/// Ports as the file spells them, from the config.
pub fn ports_from_config(cfg: &Config) -> Vec<Port> {
    cfg.wanted_ports(&[])
        .iter()
        .enumerate()
        .map(|(i, p)| Port {
            sandbox: p.sandbox_port,
            host: Some(p.host_port),
            host_ip: cfg.ip_per_app.then(|| ports::host_ip_for(true, i)),
            protocol: None,
        })
        .collect()
}

/// The alias `sbxw.toml` gives a sandbox port — matched on the sandbox side,
/// which does not move when a host port is renegotiated.
pub fn alias_for(cfg: &Config, sandbox_port: u16) -> String {
    cfg.ports
        .iter()
        .find(|c| c.sandbox_port == sandbox_port)
        .map(|c| c.alias.clone())
        .unwrap_or_default()
}

/// The one place an `EnvFile` is assembled, for the CLI and the browser.
pub fn env_file_for(name: &str, cfg: &Config, workspace: &Path, ports: Vec<Port>, dir: &Path) -> EnvFile {
    let aliases: Vec<(u16, String)> = ports
        .iter()
        .map(|p| (p.host.unwrap_or(p.sandbox), alias_for(cfg, p.sandbox)))
        .collect();
    EnvFile {
        name: name.to_string(),
        agent: "claude".into(),
        workspace: path_relative_to(dir, workspace),
        skills: cfg.skills_mode().map(str::to_string),
        kits: cfg
            .kits
            .iter()
            .map(|k| {
                let p = Path::new(k);
                if p.is_absolute() {
                    path_relative_to(dir, p)
                } else {
                    k.clone()
                }
            })
            .collect(),
        env: cfg.effective_env(),
        ports,
        notes: notes(cfg, &aliases),
    }
}

/// Header comments: every part of sbxw's pipeline the format has no field for.
pub fn notes(cfg: &Config, aliases: &[(u16, String)]) -> Vec<String> {
    let mut notes = Vec::new();
    if !cfg.network_allow.is_empty() || !cfg.network_deny.is_empty() {
        let mut note = String::from(
            "the egress allowlist — policy is `sbx policy`, not a file field.\n\
             Run once per sandbox, on the host:",
        );
        if !cfg.network_allow.is_empty() {
            note.push_str(&format!("\n  sbx policy allow network \"{}\"", cfg.network_allow.join(",")));
        }
        if !cfg.network_deny.is_empty() {
            note.push_str(&format!("\n  sbx policy deny network \"{}\"", cfg.network_deny.join(",")));
        }
        note.push_str(
            "\nWithout them the sandbox falls back to the host's default policy,\n\
             which may be broader than what sbxw would have given it.",
        );
        notes.push(note);
    }
    notes.push(
        "OAuth credentials. sbxw injects CLAUDE_CODE_OAUTH_TOKEN as a kit at creation;\n\
         `sbx env run` does not. Run /login inside the sandbox, or add a secrets:\n\
         entry whose `command:` resolves the token on the host — by absolute path\n\
         or by name on PATH: since sbx 0.46 it runs from a fresh temporary directory."
            .into(),
    );
    if cfg.keep_running {
        notes.push(
            "keep_running — staying up with nobody attached is `sbx run -d`, not a file\n\
             field. After `sbx env run`, run once: sbx run -d --name <this file's name:>"
                .into(),
        );
    }
    if let Some(policy) = cfg.pull_policy() {
        notes.push(format!(
            "pull = \"{policy}\" — the image pull policy is `sbx create --pull`, not a file\n\
             field, so `sbx env run` pulls with sbx's default (always)."
        ));
    }
    let named: Vec<String> = aliases
        .iter()
        .filter(|(_, a)| !a.is_empty())
        .map(|(host, a)| format!("{a}:{host}"))
        .collect();
    if !named.is_empty() {
        notes.push(format!(
            "the /etc/hosts aliases ({}) — sbxw writes those on the host.\n\
             The ports below are published either way; only the names are missing.",
            named.join(", ")
        ));
    }
    notes.push(
        "the browser terminal, the artifacts panel and the network-policy panel.\n\
         Those are sbxw itself — `sbx env run` attaches the agent to your terminal."
            .into(),
    );
    notes
}

/// Fold an environment file's spec onto `sbxw.toml`: **the file wins, the
/// config is the fallback**, field by field, and the report says what was
/// borrowed — two files describing one sandbox is exactly where somebody edits
/// the wrong one.
pub fn config_from_spec(spec: &envfile::Spec, mut cfg: Config) -> (Config, Vec<String>) {
    let mut report = Vec::new();
    if spec.ports.is_empty() {
        if !cfg.ports.is_empty() {
            report.push(format!(
                "ports ({}) — the environment file declares none",
                cfg.ports
                    .iter()
                    .map(|p| format!("{}:{}", p.host_port, p.sandbox_port))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
    } else {
        let mut borrowed = Vec::new();
        cfg.ports = spec
            .ports
            .iter()
            .map(|p| {
                let alias = alias_for(&cfg, p.sandbox);
                if !alias.is_empty() {
                    borrowed.push(format!("{alias} → :{}", p.host.unwrap_or(p.sandbox)));
                }
                PortMap {
                    alias,
                    sandbox_port: p.sandbox,
                    host_port: p.host.unwrap_or(p.sandbox),
                }
            })
            .collect();
        if !borrowed.is_empty() {
            report.push(format!("/etc/hosts aliases ({})", borrowed.join(", ")));
        }
    }
    if !spec.env.is_empty() {
        let overridden: Vec<&str> = spec
            .env
            .keys()
            .filter(|k| cfg.env.contains_key(*k))
            .map(String::as_str)
            .collect();
        if !overridden.is_empty() {
            report.push(format!("env — the environment file overrides {}", overridden.join(", ")));
        }
        for (k, v) in &spec.env {
            cfg.env.insert(k.clone(), v.clone());
        }
    }
    if !spec.kits.is_empty() {
        cfg.kits = spec.kits.clone();
    } else if !cfg.kits.is_empty() {
        report.push(format!("kits ({})", cfg.kits.join(", ")));
    }
    if !cfg.network_allow.is_empty() {
        report.push(format!(
            "the egress allowlist ({} rules) — an environment file has no field for it",
            cfg.network_allow.len()
        ));
    }
    if !cfg.network_deny.is_empty() {
        report.push(format!("the egress denylist ({} rules)", cfg.network_deny.len()));
    }
    if !cfg.claude_model.is_empty() && !spec.env.contains_key("ANTHROPIC_DEFAULT_MODEL") {
        report.push(format!(
            "claude_model ({}) — passed as ANTHROPIC_DEFAULT_MODEL",
            cfg.claude_model
        ));
    }
    if cfg.ip_per_app {
        report.push("ip_per_app (one loopback IP per app)".into());
    }
    (cfg, report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exports_land_outside_the_workspace() {
        let ws = Path::new("/home/u/dev/neos");
        assert_eq!(export_dir_for(ws, None), Path::new("/home/u/dev"));
        assert_eq!(export_dir_for(ws, Some(Path::new("/home/u"))), Path::new("/home/u"));
        assert_eq!(export_dir_for(ws, Some(ws)), Path::new("/home/u/dev"), "never inside");
        assert_eq!(export_dir_for(ws, Some(Path::new("/elsewhere"))), Path::new("/home/u/dev"));
    }

    #[test]
    fn paths_are_relative_to_the_file_unless_only_root_is_shared() {
        let d = Path::new("/home/u/dev");
        assert_eq!(path_relative_to(d, Path::new("/home/u/dev/neos")), "./neos");
        assert_eq!(path_relative_to(d, Path::new("/home/u/kits/x")), "../kits/x");
        assert_eq!(path_relative_to(d, d), ".");
        assert_eq!(path_relative_to(d, Path::new("/opt/kit")), "/opt/kit");
    }

    fn neos() -> Config {
        Config {
            ports: vec![PortMap { alias: "neos.local".into(), sandbox_port: 4200, host_port: 4200 }],
            ..Config::default()
        }
    }

    #[test]
    fn a_rendered_export_carries_its_omissions() {
        let cfg = neos();
        let f = env_file_for("neos", &cfg, Path::new("/d/neos"), ports_from_config(&cfg), Path::new("/d"));
        let yaml = f.render("sbxw test");
        assert!(yaml.contains("workspace: ./neos"));
        assert!(yaml.contains("sbx policy allow network"));
        assert!(yaml.contains("neos.local:4200"));
    }

    #[test]
    fn the_file_wins_and_the_config_fills_in() {
        let spec = envfile::Spec {
            ports: vec![Port { sandbox: 4200, host: Some(4300), host_ip: None, protocol: None }],
            ..Default::default()
        };
        let (cfg, report) = config_from_spec(&spec, neos());
        assert_eq!(cfg.ports[0].host_port, 4300);
        assert_eq!(cfg.ports[0].alias, "neos.local", "the alias survives the merge");
        assert!(report.iter().any(|r| r.contains("neos.local → :4300")));
        assert!(report.iter().any(|r| r.contains("egress allowlist")));
    }
}
