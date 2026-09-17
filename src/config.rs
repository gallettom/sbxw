//! Project configuration (`sbxw.toml`), with Angular(4200)+Symfony(8000)
//! defaults tuned for the NEOS-style stack.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Deserialize, Serialize)]
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
    /// Model new sessions in the sandbox start on, passed as
    /// `ANTHROPIC_DEFAULT_MODEL`. Accepts a full model id ("claude-sonnet-5")
    /// or an alias ("sonnet", "opus"). Empty => don't set it at all.
    ///
    /// A *default*, and now genuinely one. sbxw used to write `model` into the
    /// sandbox's `settings.json`, which sits **above** a saved `/model` choice
    /// in Claude Code's precedence — so every `sbxw up` silently undid whatever
    /// the user had picked in-session. `ANTHROPIC_DEFAULT_MODEL` sits *below*
    /// it: Claude Code starts on this model only when nothing else selects one,
    /// so a `/model` choice now survives.
    ///
    /// Requires Claude Code **2.1.236+** inside the sandbox, which comes from
    /// the sbx template rather than from sbxw. On an older one the variable is
    /// ignored in silence and sessions start on Claude Code's own default.
    pub claude_model: String,
    /// How sandboxes sbxw creates see sbx's shared skill store (populated by
    /// `sbxw skills import`): `"off"`, `"readonly"` or `"readwrite"`, passed as
    /// `sbx create --skills=…` (sbx 0.43). Empty leaves it to sbx, whose
    /// default is `"readonly"` — and whose `skills.defaultMode` setting can
    /// change that for the whole host, which is the better place for a
    /// preference that isn't about this project.
    ///
    /// `"off"` means the agent sees only the skills its kits and workspace
    /// provide; `"readwrite"` lets it add to the store every other sandbox
    /// mounts, so it is a choice about the *other* sandboxes as much as this one.
    ///
    /// Only read at *creation*: changing it has no effect on existing sandboxes.
    pub skills: String,
    /// The spelling before `skills`: `false` means `skills = "off"`. Kept so an
    /// existing sbxw.toml goes on meaning what it said — sbx kept its own old
    /// flag as an alias for the same reason. `skills` wins when both are set.
    pub share_skills: bool,
    /// Command the web UI's **Monitor** pane runs, as argv — no shell, so no
    /// quoting rules and no injection surface. It runs on the *host*, outside
    /// any sandbox, which is why it is one fixed configured command rather than
    /// a free-form "run anything here" box.
    ///
    /// Empty disables the pane (the sidebar button disappears).
    ///
    /// The default is bare `sbx`: with no subcommand the CLI opens its own
    /// all-sandboxes dashboard.
    pub monitor_cmd: Vec<String>,
    /// Environment variables put in the sandbox at creation, forwarded as
    /// `sbx create -e KEY=VALUE`.
    ///
    /// A `BTreeMap` so the argv is stable run to run — a diff of two `sbxw up`
    /// logs should show what changed in the config, not how a hash map felt.
    ///
    /// Read at **creation only**, like `share_skills`: sbx has no way to set a
    /// variable on a sandbox that already exists, so editing this and re-running
    /// `up` changes nothing until the sandbox is recreated. `provision_sandbox`
    /// says so out loud rather than leaving you to discover it.
    ///
    /// Not a place for secrets: the value lands in `sbx create`'s argv, visible
    /// in the host process list, and in a file most projects commit. Tokens go
    /// through `sbx secret set` (which is how sbxw already handles the Anthropic
    /// one) or an `sbxenv.yaml` `secrets:` entry that resolves on the host.
    pub env: std::collections::BTreeMap<String, String>,
    /// Files of `KEY=VALUE` lines forwarded as `sbx create --env-file`,
    /// in order. Relative paths resolve against `sbxw.toml`'s directory.
    ///
    /// sbx settles the precedence itself: `env` above beats every file, and a
    /// later file beats an earlier one. sbxw merges nothing — a project's `.env`
    /// is read by sbx, at creation, on the host.
    pub env_files: Vec<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
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
                PortMap {
                    alias: "neos.local".into(),
                    sandbox_port: 4200,
                    host_port: 4200,
                },
                PortMap {
                    alias: "api.neos.local".into(),
                    sandbox_port: 8000,
                    host_port: 8000,
                },
            ],
            kits: vec![],
            claude_subscription: "pro".into(),
            claude_model: "claude-sonnet-5".into(),
            skills: String::new(),
            share_skills: true,
            monitor_cmd: vec!["sbx".into()],
            env: Default::default(),
            env_files: vec![],
        }
    }
}

/// The values `skills` accepts, in sbx's own words.
pub const SKILL_MODES: [&str; 3] = ["off", "readonly", "readwrite"];

impl Config {
    /// The `--skills` mode to ask for, or `None` for sbx's default.
    pub fn skills_mode(&self) -> Option<&str> {
        match self.skills.trim() {
            "" => (!self.share_skills).then_some("off"),
            mode => Some(mode),
        }
    }

    /// `env` as the `KEY=VALUE` arguments `sbx`'s `-e` takes, in the map's
    /// (sorted) order.
    ///
    /// Values are passed through untouched — quoting, spaces and `=` signs
    /// included, since sbx splits on the first `=` only.
    ///
    /// One `-e` form a TOML map cannot reach: a bare `KEY`, which tells sbx to
    /// copy the value from the host's environment. `KEY = ""` is an *empty*
    /// value, not that. Pass such variables through `env_files`, or give them
    /// their value here.
    pub fn env_pairs(&self) -> Vec<String> {
        self.effective_env()
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect()
    }

    /// `env`, plus the variables sbxw derives from other settings.
    ///
    /// Only `claude_model` for now, as `ANTHROPIC_DEFAULT_MODEL`. Deriving it
    /// here rather than at each call site is what keeps the model wherever the
    /// environment goes — baked in at creation, re-applied on every attach, and
    /// carried into an exported `sbxenv.yaml` — from one definition.
    ///
    /// An explicit `ANTHROPIC_DEFAULT_MODEL` in `[env]` wins: it is the more
    /// specific way to say the same thing, and silently overruling it would be
    /// the surprise this whole change is meant to remove.
    pub fn effective_env(&self) -> std::collections::BTreeMap<String, String> {
        const MODEL_VAR: &str = "ANTHROPIC_DEFAULT_MODEL";
        let mut env = self.env.clone();
        if !self.claude_model.is_empty() {
            env.entry(MODEL_VAR.into())
                .or_insert_with(|| self.claude_model.clone());
        }
        env
    }

    /// Render back to TOML, for the merged config `sbxw env run` leaves on disk
    /// so the web daemon — a separate process that re-reads a file — sees the
    /// same merge the run computed.
    ///
    /// Not a plain `toml::to_string`: TOML requires every bare key to come
    /// before the first table, and serde emits fields in declaration order,
    /// where `ports` and `env` sit in the middle. Going through `toml::Value`
    /// (a sorted map) lets the writer place the tables last, which is the
    /// difference between a file that round-trips and one that fails to parse
    /// with "values must be emitted before tables".
    pub fn to_toml(&self) -> Result<String> {
        let value = toml::Value::try_from(self).context("encoding the config")?;
        toml::to_string(&value).context("rendering the config as TOML")
    }

    pub fn load_or_default(path: &Path) -> Result<Self> {
        if path.exists() {
            let raw = std::fs::read_to_string(path)
                .with_context(|| format!("reading {}", path.display()))?;
            let cfg: Config =
                toml::from_str(&raw).with_context(|| format!("parsing {}", path.display()))?;
            // Checked here rather than left to sbx: a value it refuses would
            // fail `sbx create` — the sandbox, not the setting — and only once
            // somebody asked for a new one.
            if let Some(mode) = cfg.skills_mode() {
                if !SKILL_MODES.contains(&mode) {
                    anyhow::bail!(
                        "{}: skills = \"{mode}\" — expected one of {}",
                        path.display(),
                        SKILL_MODES.join(", ")
                    );
                }
            }
            Ok(cfg)
        } else {
            Ok(Config::default())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The merged config `sbxw env run` writes is read back by a *different
    /// process* (the web daemon), so a round trip that loses or reorders a
    /// field is a silent misprovision rather than an error anyone sees.
    ///
    /// TOML's rule that bare keys precede tables is the trap here: `ports` and
    /// `env` are declared in the middle of `Config`, and serde's field order
    /// would put every key after them inside the last table.
    #[test]
    fn a_config_survives_being_written_and_read_again() {
        let mut cfg = Config {
            claude_model: "claude-opus-5".into(),
            skills: "readwrite".into(),
            share_skills: false,
            ip_per_app: true,
            env_files: vec!["/p/.env".into()],
            ..Config::default()
        };
        cfg.env.insert("NODE_ENV".into(), "test".into());
        cfg.env
            .insert("TRICKY".into(), "a = b # not a comment".into());

        let text = cfg.to_toml().expect("render");
        let back: Config = toml::from_str(&text).expect("re-parse");

        assert_eq!(back.claude_model, cfg.claude_model);
        assert!(!back.share_skills);
        assert_eq!(back.skills_mode(), Some("readwrite"));
        assert!(back.ip_per_app);
        assert_eq!(back.env, cfg.env);
        assert_eq!(back.env_files, cfg.env_files);
        assert_eq!(back.network_allow, cfg.network_allow);
        assert_eq!(back.ports.len(), cfg.ports.len());
        assert_eq!(back.ports[0].alias, "neos.local");
        assert_eq!(back.ports[0].host_port, 4200);
        assert_eq!(back.monitor_cmd, cfg.monitor_cmd);
    }

    /// `share_skills = false` still means what it meant; `skills` is the newer,
    /// more specific spelling and wins; and neither says nothing at all.
    #[test]
    fn the_old_share_skills_switch_is_an_alias_for_off() {
        let cfg = |skills: &str, share_skills: bool| Config {
            skills: skills.into(),
            share_skills,
            ..Config::default()
        };
        assert_eq!(cfg("", true).skills_mode(), None);
        assert_eq!(cfg("", false).skills_mode(), Some("off"));
        assert_eq!(cfg("readwrite", false).skills_mode(), Some("readwrite"));
        assert_eq!(cfg("readonly", true).skills_mode(), Some("readonly"));
    }

    #[test]
    fn an_unknown_skills_mode_is_refused_on_load() {
        let dir = std::env::temp_dir().join(format!("sbxw-config-skills-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("sbxw.toml");
        std::fs::write(&f, "skills = \"rw\"\n").unwrap();
        let err = Config::load_or_default(&f).unwrap_err().to_string();
        assert!(err.contains("readwrite"), "{err}");
        std::fs::write(&f, "skills = \"readwrite\"\n").unwrap();
        assert!(Config::load_or_default(&f).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A bare `KEY` (inherit from the host) is not reachable from a TOML map,
    /// and pretending otherwise would put an empty value in the sandbox.
    #[test]
    fn env_pairs_are_key_equals_value_in_a_stable_order() {
        let mut cfg = Config {
            claude_model: String::new(),
            ..Config::default()
        };
        cfg.env.insert("B".into(), "2".into());
        cfg.env.insert("A".into(), "1".into());
        cfg.env.insert("EMPTY".into(), String::new());
        assert_eq!(cfg.env_pairs(), vec!["A=1", "B=2", "EMPTY="]);
    }

    /// The model reaches the sandbox as one more environment variable, so it
    /// travels wherever the environment does — creation, attach, export — from
    /// a single definition.
    #[test]
    fn the_model_rides_along_as_anthropic_default_model() {
        let cfg = Config {
            claude_model: "claude-opus-5".into(),
            ..Config::default()
        };
        assert!(cfg
            .env_pairs()
            .contains(&"ANTHROPIC_DEFAULT_MODEL=claude-opus-5".to_string()));

        // Empty means "say nothing", not "set it to an empty string" — which
        // Claude Code would read as a model name and reject.
        let none = Config {
            claude_model: String::new(),
            ..Config::default()
        };
        assert!(!none.effective_env().contains_key("ANTHROPIC_DEFAULT_MODEL"));
    }

    /// Spelling the variable out in `[env]` is the more specific way to say the
    /// same thing; overruling it from `claude_model` would be exactly the kind
    /// of silent override this move was meant to remove.
    #[test]
    fn an_explicit_variable_in_env_beats_claude_model() {
        let mut cfg = Config {
            claude_model: "claude-sonnet-5".into(),
            ..Config::default()
        };
        cfg.env
            .insert("ANTHROPIC_DEFAULT_MODEL".into(), "haiku".into());
        assert_eq!(
            cfg.effective_env()
                .get("ANTHROPIC_DEFAULT_MODEL")
                .map(String::as_str),
            Some("haiku")
        );
    }
}
