//! Jarvis's sandbox, `sbxw-jarvis`: a sandbox of the daemon's own on this
//! machine, with one Claude session the person talks to by voice or text,
//! which proposes changes to sbxw's configuration through the `sbxw-config`
//! MCP server (`assets/jarvis/`). The daemon validates and applies them once
//! the person approves (`svc::jarvis`, `http::jarvis`).
//!
//! Like the kit cache's filler (`cache::FILLER`) it is the daemon's, not a
//! project's; unlike it, it is long-lived and someone works in it, so it goes
//! through the ordinary bring-up (token, hooks, relay, a record, a row in the
//! sidebar, sign-in as any sandbox) on a config the daemon writes itself.
//! What must hold, and why:
//!
//! * **Light.** Its only job is sbxw's configuration, so it gets sbxw's base
//!   kit and the jarvis kit, and nothing else: not the daemon's `kits` line
//!   nor the defaults (agent-skills alone pulls Chromium and FFmpeg), no kept
//!   skills, no host-secret kit (ssh-agent, git-identity), no kit cache (it
//!   must never start a fill), no template, no volumes, small fixed CPUs and
//!   memory, one Claude session.
//! * **Nothing to edit.** Its workspace is an empty directory under
//!   `~/.sbxw/state/jarvis/`: never a project, never sbxw's repository, never
//!   `~/.sbxw` itself. No carve, no extra mount, no ports.
//! * **The name is the daemon's.** The agent listener gives Jarvis's routes
//!   to `sbxw-jarvis` alone, so a person (or an agent) must never get a
//!   project sandbox under that name, on any host: [`reserved`] is checked by
//!   every bring-up but the daemon's own ([`App::provision_owned`]).
//! * **Removable.** `sbxw rm sbxw-jarvis` is an ordinary removal; its state is
//!   read back from sbx, so the next start creates it again.
//! * **Ready to talk to.** A start opens its agent session the way a terminal
//!   does, which also starts a stopped sandbox: one path, not a second.

use crate::app::App;
use crate::provision::Consent;
use crate::store::Paths;
use anyhow::Result;
use sbxw_core::config::Config;
use sbxw_core::kits;
use sbxw_core::session::SessionKey;
use sbxw_core::{SandboxName, SandboxRef};
use serde::Serialize;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

/// The sandbox's name, on this machine.
pub const NAME: &str = kits::JARVIS_SANDBOX;

/// Enough for one Claude Code session and its MCP servers, and no more: the
/// sandbox runs no builds, no browser, no containers. Not inherited from the
/// daemon's config, whose sizing is for project work. sbx's own floor is not
/// documented; sbxw's config check sets none.
pub const CPUS: u32 = 2;
pub const MEMORY: &str = "2g";

/// Jarvis's sandbox on this machine.
pub fn sandbox() -> SandboxRef {
    SandboxRef::local(SandboxName::parse(NAME).expect("sbxw-jarvis is a valid sandbox name"))
}

/// Is `sb` Jarvis's sandbox by name: on this machine, named [`NAME`].
pub fn is_jarvis_name(sb: &SandboxRef) -> bool {
    sb.is_local() && sb.name.as_str() == NAME
}

/// Why a person cannot create, duplicate to or bring up `sb`: its name is
/// Jarvis's, on whichever host. `None` for every other name.
pub fn reserved(sb: &SandboxRef) -> Option<String> {
    (sb.name.as_str() == NAME).then(|| {
        format!(
            "'{NAME}' is reserved for Jarvis, sbxw's assistant, which the daemon creates itself — pick another name. \
             To recreate Jarvis, `sbxw rm {NAME}` and start Jarvis again"
        )
    })
}

/// Its empty workspace: under sbxw's state, not `~/.sbxw` itself, so the
/// sandbox sees none of sbxw's files.
pub fn workspace(paths: &Paths) -> PathBuf {
    paths.state().join("jarvis").join("workspace")
}

/// The kits as `kits` lists them; the bring-up puts sbxw's base kit first.
pub fn kit_list() -> Vec<String> {
    vec![format!("sbxw:{}", kits::JARVIS)]
}

/// The config Jarvis is brought up on. Built from sbxw's defaults, not the
/// daemon's file: only the listeners and the network policy (the daemon's
/// default for every sandbox) and the model are the daemon's; nothing else
/// of it (kits, env, mounts, ports, sizing, carves, volumes, Chrome) leaks in.
pub fn config(daemon: &Config) -> Config {
    Config {
        web_addr: daemon.web_addr.clone(),
        agent_addr: daemon.agent_addr.clone(),
        network_allow: daemon.network_allow.clone(),
        network_deny: daemon.network_deny.clone(),
        claude_model: daemon.claude_model.clone(),
        models: daemon.models.clone(),
        pull: daemon.pull.clone(),
        kits: kit_list(),
        cpus: CPUS,
        memory: MEMORY.into(),
        // No kit of it fills the cache, and none may start a fill.
        cache: false,
        template: String::new(),
        keep_running: false,
        chrome: false,
        // sbx's shared skills store is the person's: not Jarvis's to read.
        skills: "off".into(),
        // One session, no threads, no lead.
        max_agent_sessions: 1,
        ..Config::default()
    }
}

/// What `/api/jarvis/status` answers. `starting` is a stopped sandbox whose
/// agent session is being opened, which starts it.
#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
#[serde(tag = "state", rename_all = "lowercase")]
pub enum Status {
    Absent,
    Creating,
    Starting,
    Running,
    Stopped,
    Failed { error: String },
}

/// A creation or start in progress, or the last one's failure, per daemon
/// (keyed by its `~/.sbxw`, so tests on their own directories do not meet).
/// Whether the sandbox exists is always asked of sbx, never remembered here.
#[derive(Clone)]
enum Phase {
    Creating,
    Starting,
    Failed(String),
}

static PHASES: Mutex<Option<HashMap<PathBuf, Phase>>> = Mutex::new(None);

fn phase(paths: &Paths) -> Option<Phase> {
    PHASES.lock().unwrap().as_ref().and_then(|m| m.get(&paths.root).cloned())
}

fn set_phase(paths: &Paths, p: Option<Phase>) {
    let mut g = PHASES.lock().unwrap();
    let m = g.get_or_insert_with(HashMap::new);
    match p {
        Some(p) => m.insert(paths.root.clone(), p),
        None => m.remove(&paths.root),
    };
}

/// Take a creation or a start for this daemon: `false` when one is already
/// under way.
fn begin(paths: &Paths, p: Phase) -> bool {
    let mut g = PHASES.lock().unwrap();
    let m = g.get_or_insert_with(HashMap::new);
    if matches!(m.get(&paths.root), Some(Phase::Creating | Phase::Starting)) {
        return false;
    }
    m.insert(paths.root.clone(), p);
    true
}

/// Forget a failure once the sandbox is removed: the next start begins anew.
pub(crate) fn forget(paths: &Paths) {
    let mut g = PHASES.lock().unwrap();
    if let Some(m) = g.as_mut() {
        if matches!(m.get(&paths.root), Some(Phase::Failed(_))) {
            m.remove(&paths.root);
        }
    }
}

impl App {
    /// Is `sb` the Jarvis sandbox the daemon made: the name, on this machine,
    /// created on Jarvis's workspace. A sandbox made under that name before
    /// it was reserved is not taken for it.
    pub fn is_jarvis(&self, sb: &SandboxRef) -> bool {
        is_jarvis_name(sb) && self.store.workspace(sb).is_some_and(|w| w == workspace(&self.store.paths))
    }

    /// Where Jarvis's sandbox stands.
    pub async fn jarvis_status(&self) -> Status {
        let now = phase(&self.store.paths);
        match now {
            Some(Phase::Creating) => return Status::Creating,
            Some(Phase::Starting) => return Status::Starting,
            _ => {}
        }
        let failed = match now {
            Some(Phase::Failed(error)) => Some(Status::Failed { error }),
            _ => None,
        };
        let sb = sandbox();
        match self.sbx.lookup(&sb.name).await {
            Some(_) if !self.is_jarvis(&sb) => Status::Failed { error: foreign() },
            Some(i) if i.is_running() => Status::Running,
            Some(_) => failed.unwrap_or(Status::Stopped),
            None => failed.unwrap_or(Status::Absent),
        }
    }

    /// Make Jarvis ready to talk to, and say where it stands: create its
    /// sandbox when there is none, start it when it is stopped, then open its
    /// agent session, through the path a terminal opens one by
    /// ([`App::open_session`]), so the person can speak to it without opening
    /// a terminal first. Idempotent: a running one, a creation or a start
    /// under way are returned as they are, and a sandbox of that name the
    /// daemon did not make is refused. The work runs in the background, a
    /// creation with its progress on the bus like any sandbox's;
    /// `jarvis_status` follows it.
    pub async fn jarvis_start(self: &Arc<Self>) -> Status {
        let create = match self.jarvis_status().await {
            Status::Failed { error } if error == foreign() => return Status::Failed { error },
            Status::Absent => true,
            Status::Stopped => false,
            Status::Failed { .. } => !self.sbx.exists(&sandbox().name).await,
            other => return other,
        };
        let (claim, shown) = match create {
            true => (Phase::Creating, Status::Creating),
            false => (Phase::Starting, Status::Starting),
        };
        if !begin(&self.store.paths, claim) {
            return self.jarvis_status().await;
        }
        let app = self.clone();
        tokio::spawn(async move {
            let out = async {
                if create {
                    app.jarvis_bring_up().await?;
                    set_phase(&app.store.paths, Some(Phase::Starting));
                }
                app.open_session(&SessionKey::agent(sandbox())).await.map(|_| ())
            }
            .await;
            if let Err(e) = &out {
                tracing::warn!("Jarvis is not ready: {e:#}");
            }
            set_phase(&app.store.paths, out.err().map(|e| Phase::Failed(format!("{e:#}"))));
        });
        shown
    }

    async fn jarvis_bring_up(self: &Arc<Self>) -> Result<Vec<String>> {
        let sb = sandbox();
        // One left over from before the name was reserved is not Jarvis.
        if self.sbx.exists(&sb.name).await && !self.is_jarvis(&sb) {
            anyhow::bail!("{}", foreign());
        }
        let ws = workspace(&self.store.paths);
        std::fs::create_dir_all(&ws).map_err(|e| anyhow::anyhow!("creating {}: {e}", ws.display()))?;
        tracing::info!("creating Jarvis's sandbox '{sb}' on {}", ws.display());
        let cfg = config(&self.daemon_config());
        // Built here, with no host-access grant to approve.
        let req = self.bring_up_request(sb, ws, cfg, vec![], false, Consent::Checked(vec![]));
        self.provision_owned(req, None).await
    }
}

fn foreign() -> String {
    format!("a sandbox named '{NAME}' exists that sbxw did not create for Jarvis — `sbxw rm {NAME}`, then start Jarvis again")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{app_with, ls, sbx_answer};

    #[test]
    fn the_name_is_reserved_on_every_host() {
        assert!(reserved(&SandboxRef::parse(NAME).unwrap()).is_some());
        let remote = SandboxRef::new(sbxw_core::names::HostName::parse("box").unwrap(), SandboxName::parse(NAME).unwrap());
        assert!(reserved(&remote).is_some() && !is_jarvis_name(&remote), "reserved there too, never Jarvis");
        assert!(reserved(&SandboxRef::parse("myapp").unwrap()).is_none());
        assert!(reserved(&SandboxRef::parse("sbxw-jarvis2").unwrap()).is_none());
        assert!(is_jarvis_name(&sandbox()));
    }

    #[test]
    fn the_kit_list_is_base_and_jarvis_only() {
        let daemon = Config {
            kits: vec!["sbxw:agent-skills".into(), "sbxw:ssh-agent".into(), "sbxw:git-identity".into(), "/my/kit".into()],
            ..Config::default()
        };
        let cfg = config(&daemon);
        assert_eq!(cfg.kits, ["sbxw:jarvis"]);
        assert_eq!(kits::with_base(&cfg.kits), ["sbxw:base", "sbxw:jarvis"], "as the bring-up applies them");
        assert_eq!(config(&Config::default()).kits, ["sbxw:jarvis"], "the defaults do not leak in either");
    }

    #[test]
    fn nothing_of_the_daemon_s_config_but_its_policy_and_listeners_is_inherited() {
        let mut daemon = Config {
            cpus: 16,
            memory: "32g".into(),
            root_size: "100g".into(),
            docker_size: "200g".into(),
            volumes: vec!["/home/agent/.npm".into()],
            workspaces: vec!["/other:/w:ro".into()],
            carve: vec![sbxw_core::config::Carve { path: "secret".into(), access: sbxw_core::config::CarveAccess::Hidden, enforce: Default::default() }],
            env_files: vec!["/e".into()],
            chrome: true,
            keep_running: true,
            template: "auto".into(),
            network_allow: vec!["example.org".into()],
            network_deny: vec!["bad.example".into()],
            agent_addr: "127.0.0.1:9999".into(),
            max_agent_sessions: 9,
            ..Config::default()
        };
        daemon.env.insert("TOKENISH".into(), "x".into());
        daemon.kit_mounts.insert("/home/agent/x".into(), "/host".into());
        let cfg = config(&daemon);
        assert_eq!((cfg.cpus, cfg.memory.as_str()), (CPUS, MEMORY));
        assert!(cfg.root_size.is_empty() && cfg.docker_size.is_empty());
        assert!(cfg.volumes.is_empty() && cfg.workspaces.is_empty() && cfg.carve.is_empty());
        assert!(cfg.env.is_empty() && cfg.env_files.is_empty() && cfg.kit_mounts.is_empty() && cfg.ports.is_empty());
        assert!(!cfg.chrome && !cfg.keep_running && !cfg.cache);
        assert_eq!(cfg.template_mode(), sbxw_core::templates::Mode::Off);
        assert_eq!(cfg.max_agent_sessions, 1);
        assert_eq!(cfg.network_allow, ["example.org"]);
        assert_eq!(cfg.network_deny, ["bad.example"]);
        assert_eq!(cfg.agent_addr, "127.0.0.1:9999");
    }

    #[test]
    fn the_workspace_is_empty_state_never_sbxw_s_root() {
        let d = tempfile::tempdir().unwrap();
        let p = Paths::under(d.path());
        let ws = workspace(&p);
        assert_eq!(ws, p.state().join("jarvis/workspace"));
        assert_ne!(ws, p.root);
        assert!(ws.starts_with(p.state()));
    }

    #[tokio::test]
    async fn a_person_cannot_bring_up_a_sandbox_under_jarvis_s_name() {
        let (dir, app, fake) = app_with(sbx_answer);
        let ws = dir.path().join("ws");
        std::fs::create_dir_all(&ws).unwrap();
        let req = app.bring_up_request(sandbox(), ws.clone(), Config::default(), vec![], false, Consent::Checked(vec![]));
        let e = format!("{:#}", app.provision(req, None).await.unwrap_err());
        assert!(e.contains("reserved for Jarvis"), "{e}");
        let form = crate::svc::sandboxes::CreateForm::default();
        let e = format!("{:#}", app.create(sandbox(), &ws.to_string_lossy(), form, None).await.unwrap_err());
        assert!(e.contains("reserved for Jarvis"), "{e}");
        assert!(!fake.log().iter().any(|c| c.starts_with("create")), "{:?}", fake.log());
    }

    async fn settled(app: &Arc<App>) -> Status {
        for _ in 0..500 {
            match app.jarvis_status().await {
                Status::Creating | Status::Starting => tokio::time::sleep(std::time::Duration::from_millis(10)).await,
                s => return s,
            }
        }
        panic!("still creating or starting");
    }

    #[tokio::test]
    async fn start_creates_it_light_once_and_after_a_removal_again() {
        let made = Arc::new(Mutex::new(false));
        let m = made.clone();
        let (_dir, app, fake) = app_with(move |a| match a.first().map(String::as_str) {
            Some("create") => {
                *m.lock().unwrap() = true;
                sbxw_sbx::runner::Output::ok("")
            }
            Some("rm") => {
                *m.lock().unwrap() = false;
                sbxw_sbx::runner::Output::ok("")
            }
            Some("ls") if *m.lock().unwrap() => ls(&[(NAME, "running")]),
            _ => sbx_answer(a),
        });
        assert_eq!(app.jarvis_status().await, Status::Absent);
        assert_eq!(app.jarvis_start().await, Status::Creating);
        assert_eq!(app.jarvis_start().await, Status::Creating, "not started twice");
        assert_eq!(settled(&app).await, Status::Running);
        assert_eq!(app.jarvis_start().await, Status::Running, "idempotent");
        let log = fake.log();
        let creates: Vec<&String> = log.iter().filter(|c| c.starts_with("create")).collect();
        assert_eq!(creates.len(), 1, "{log:?}");
        let c = creates[0];
        let paths = &app.store.paths;
        assert!(c.starts_with(&format!("create claude {}", workspace(paths).display())), "{c}");
        assert!(c.contains(&format!("--name {NAME}")), "{c}");
        let kits_given: Vec<&str> = c.split(" --kit ").skip(1).map(|s| s.split(' ').next().unwrap()).collect();
        let want = [paths.kits().join("base"), paths.kits().join("jarvis")].map(|p| p.display().to_string());
        assert_eq!(kits_given, want, "base and jarvis only: {c}");
        assert!(c.contains("--cpus 2") && c.contains("--memory 2g"), "{c}");
        assert!(c.contains("--skills=off"), "{c}");
        assert!(!c.contains("SBXW_CACHE") && !c.contains(" -p "), "no cache, no port: {c}");
        assert!(!log.iter().any(|c| c.contains(crate::cache::FILLER) || c.contains("sbxw-tpl-")), "no fill, no template: {log:?}");
        assert!(app.is_jarvis(&sandbox()));
        // `sbxw rm` is an ordinary removal; the next start makes it again.
        app.remove(&[sandbox()]).await.unwrap();
        assert_eq!(app.jarvis_status().await, Status::Absent);
        assert_eq!(app.jarvis_start().await, Status::Creating);
        assert_eq!(settled(&app).await, Status::Running);
        assert_eq!(fake.log().iter().filter(|c| c.starts_with("create")).count(), 2);
    }

    #[tokio::test]
    async fn a_stopped_jarvis_is_started_by_opening_its_agent_session() {
        let (_dir, app, fake) = app_with(|a| match a.first().map(String::as_str) {
            Some("ls") => ls(&[(NAME, "stopped")]),
            _ => sbx_answer(a),
        });
        let ws = workspace(&app.store.paths);
        app.store.update(sandbox(), |r| r.workspace = Some(ws)).unwrap();
        assert_eq!(app.jarvis_status().await, Status::Stopped);
        assert_eq!(app.jarvis_start().await, Status::Starting);
        assert_eq!(app.jarvis_start().await, Status::Starting, "not started twice");
        let after = settled(&app).await;
        // The terminal's own path ran: the agent session is open, or opening
        // it failed with its error (no `sbx` binary under `cargo test`).
        let opened = app.sessions.keys_of(&sandbox().to_string()).iter().any(|k| k.kind.is_agent());
        assert!(opened || matches!(after, Status::Failed { .. }), "{after:?}");
        assert!(!fake.log().iter().any(|c| c.starts_with("create")), "started, never created: {:?}", fake.log());
        if let Status::Failed { .. } = after {
            assert_eq!(app.jarvis_start().await, Status::Starting, "a failed start is tried again");
            settled(&app).await;
        }
    }

    #[tokio::test]
    async fn one_the_daemon_did_not_make_is_not_taken_for_jarvis() {
        let (_dir, app, fake) = app_with(|a| match a.first().map(String::as_str) {
            Some("ls") => ls(&[(NAME, "running")]),
            _ => sbx_answer(a),
        });
        assert!(!app.is_jarvis(&sandbox()));
        assert!(matches!(app.jarvis_status().await, Status::Failed { .. }));
        assert!(matches!(app.jarvis_start().await, Status::Failed { .. }));
        assert!(!fake.log().iter().any(|c| c.starts_with("create")));
    }

    #[test]
    fn the_status_reads_as_the_contract_says() {
        assert_eq!(serde_json::to_value(Status::Running).unwrap(), serde_json::json!({ "state": "running" }));
        assert_eq!(serde_json::to_value(Status::Starting).unwrap(), serde_json::json!({ "state": "starting" }));
        assert_eq!(serde_json::to_value(Status::Failed { error: "x".into() }).unwrap(), serde_json::json!({ "state": "failed", "error": "x" }));
    }
}
