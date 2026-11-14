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
use sbxw_core::tooling::{piece, Tooling, ToolingOutcome};
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

/// What the daemon wrote into Jarvis's sandbox the last time a Claude session
/// was about to start there: its role in `~/.claude/CLAUDE.md` and the
/// `sbxw-config` registration in `~/.claude.json`. What the commands did, not
/// what the session read. `None` for both: no session started there since
/// this daemon started.
#[derive(Serialize, Debug, Clone, PartialEq, Eq, Default)]
pub struct Setup {
    pub role_written: Option<bool>,
    pub tools_registered: Option<bool>,
    /// In plain words, when something is missing or nothing was checked.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// Written by a bring-up that a session open follows at once: that open
    /// need not write it all again.
    #[serde(skip)]
    fresh: bool,
}

/// What a new session needs for a repair to reach it. A running session
/// read `CLAUDE.md` and loaded its MCP servers when it started.
const RESTART: &str = "Stop Jarvis's sandbox (`sbxw stop sbxw-jarvis`), then start Jarvis again: \
                       its next Claude session reads its role and loads its tools.";

impl Setup {
    /// What a tooling batch run for Jarvis says, in the status's words.
    pub fn from_outcome(outcome: &Result<ToolingOutcome>, fresh: bool) -> Self {
        let recreate = format!("Recreate it to repair this: `sbxw rm {NAME}`, then start Jarvis again.");
        match outcome {
            Ok(o) => {
                let role = o.error(piece::JARVIS_ROLE);
                let tools = o.error(piece::CONFIG_MCP);
                let mut notes = Vec::new();
                if let Some(e) = role {
                    notes.push(format!("Jarvis's role could not be written into its CLAUDE.md, so it may not know it is Jarvis: {e}."));
                }
                if let Some(e) = tools {
                    notes.push(format!("Jarvis's sbxw-config tools could not be registered, so it cannot propose changes: {e}. {recreate}"));
                }
                Setup { role_written: Some(role.is_none()), tools_registered: Some(tools.is_none()), note: (!notes.is_empty()).then(|| notes.join(" ")), fresh }
            }
            Err(e) => {
                let e = format!("{e:#}");
                let fix = if kits::node_missing(&e) { format!(" It has no Node.js. {recreate}") } else { String::new() };
                Setup {
                    role_written: Some(false),
                    tools_registered: Some(false),
                    note: Some(format!("sbxw could not set up Jarvis's sandbox, so Jarvis may not know its role nor have its tools: {e}.{fix}")),
                    fresh: false,
                }
            }
        }
    }

    fn complete(&self) -> bool {
        self.role_written == Some(true) && self.tools_registered == Some(true)
    }
}

/// `/api/jarvis/status` and `/api/jarvis/start`: the state, and what the
/// last setup wrote.
#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
pub struct Report {
    #[serde(flatten)]
    pub status: Status,
    #[serde(flatten)]
    pub setup: Setup,
}

static SETUPS: Mutex<Option<HashMap<PathBuf, Setup>>> = Mutex::new(None);

/// Keep what the last setup of Jarvis's sandbox did, for this daemon.
pub(crate) fn record_setup(paths: &Paths, setup: Setup) {
    if let Some(n) = &setup.note {
        tracing::warn!("Jarvis: {n}");
    }
    SETUPS.lock().unwrap().get_or_insert_with(HashMap::new).insert(paths.root.clone(), setup);
}

pub(crate) fn setup(paths: &Paths) -> Setup {
    SETUPS.lock().unwrap().as_ref().and_then(|m| m.get(&paths.root).cloned()).unwrap_or_default()
}

/// Was everything written by the bring-up just before? Then the session open
/// that follows it skips its own pass, once.
fn take_fresh(paths: &Paths) -> bool {
    let mut g = SETUPS.lock().unwrap();
    match g.as_mut().and_then(|m| m.get_mut(&paths.root)) {
        Some(s) if s.fresh && s.complete() => {
            s.fresh = false;
            true
        }
        Some(s) => {
            s.fresh = false;
            false
        }
        None => false,
    }
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

    /// [`App::jarvis_status`], with what the last setup wrote. A running
    /// Jarvis with nothing recorded started before this daemon did: its
    /// session may predate the role and the tools, so the note says how to
    /// get a new one.
    pub async fn jarvis_report(&self) -> Report {
        let status = self.jarvis_status().await;
        let mut setup = setup(&self.store.paths);
        if status == Status::Running && setup.role_written.is_none() {
            setup.note = Some(format!(
                "Not checked: Jarvis's session started before sbxw's daemon did, maybe without its role or its tools. {RESTART}"
            ));
        }
        Report { status, setup }
    }

    /// Write Jarvis's role into its `CLAUDE.md` and register its
    /// `sbxw-config` server, before a Claude session starts there: the
    /// session reads both only when it starts. Runs at every session open on
    /// Jarvis's sandbox, so one made by an older daemon is repaired at its
    /// next start; `sbx exec` starts a stopped sandbox first. Skipped once
    /// right after a bring-up that wrote both. Best-effort: the outcome is
    /// recorded for `jarvis_status`, and the session opens either way.
    pub(crate) async fn jarvis_setup(&self) {
        let paths = &self.store.paths;
        if take_fresh(paths) {
            return;
        }
        let sb = sandbox();
        let outcome = async {
            let port = self.agent_port_for(&sb).await?;
            self.sbx.tooling(&sb.name, &jarvis_tooling(&port)).await
        }
        .await;
        record_setup(paths, Setup::from_outcome(&outcome, false));
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

/// What only Jarvis's sandbox gets from the tooling batch: its role and its
/// proposal tools. Part of its bring-up's batch, and on its own before each
/// session ([`App::jarvis_setup`]).
pub fn jarvis_tooling(agent_port: &str) -> Tooling {
    with_jarvis_tooling(Tooling::new(), agent_port)
}

pub fn with_jarvis_tooling(batch: Tooling, agent_port: &str) -> Tooling {
    batch.jarvis_role().config_mcp(agent_port)
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

    /// The tooling scripts piped into `sbx exec <name> -- node -`, in order.
    fn batches(fake: &sbxw_sbx::runner::Fake, name: &str) -> Vec<String> {
        let calls = fake.calls.lock().unwrap();
        let stdins = fake.stdins.lock().unwrap();
        // Every call with stdin, in order, is one stdin.
        let with_stdin: Vec<&Vec<String>> = calls.iter().filter(|c| c.first().map(String::as_str) == Some("exec") && c.get(1).map(String::as_str) == Some("-i")).collect();
        with_stdin
            .iter()
            .zip(stdins.iter())
            .filter(|(c, _)| c.join(" ") == format!("exec -i {name} -- node -"))
            .map(|(_, s)| String::from_utf8_lossy(s).into_owned())
            .collect()
    }

    fn writes_role_and_tools(script: &str) -> bool {
        use sbxw_core::tooling as t;
        script.contains(&serde_json::to_string(&t::jarvis_doc()).unwrap()) && script.contains(&format!("{:?}", t::CONFIG_MCP_NAME))
    }

    #[tokio::test]
    async fn creation_writes_the_role_and_the_tools_once_and_says_so() {
        let made = Arc::new(Mutex::new(false));
        let m = made.clone();
        let (_dir, app, fake) = app_with(move |a| match a.first().map(String::as_str) {
            Some("create") => {
                *m.lock().unwrap() = true;
                sbxw_sbx::runner::Output::ok("")
            }
            Some("ls") if *m.lock().unwrap() => ls(&[(NAME, "running")]),
            _ => sbx_answer(a),
        });
        assert_eq!(app.jarvis_report().await.setup, Setup::default(), "nothing to say before a start");
        app.jarvis_start().await;
        settled(&app).await;
        let ran = batches(&fake, NAME);
        // The bring-up's batch carries them; the session open after it does
        // not run them a second time.
        assert_eq!(ran.len(), 1, "{ran:?}");
        assert!(writes_role_and_tools(&ran[0]));
        assert!(ran[0].contains(sbxw_core::tooling::RELAY_DOC_BEGIN), "beside the relay");
        let r = app.jarvis_report().await;
        assert_eq!((r.setup.role_written, r.setup.tools_registered, r.setup.note.as_deref()), (Some(true), Some(true), None));
        let v = serde_json::to_value(&r).unwrap();
        assert_eq!(v["role_written"], true);
        assert_eq!(v["tools_registered"], true);
        assert!(v.get("note").is_none() && v["state"].is_string(), "{v}");
    }

    #[tokio::test]
    async fn an_existing_jarvis_is_repaired_at_its_next_start_before_its_session() {
        let (_dir, app, fake) = app_with(|a| match a.first().map(String::as_str) {
            Some("ls") => ls(&[(NAME, "stopped")]),
            _ => sbx_answer(a),
        });
        // One an older daemon made: a record and a workspace, nothing written.
        app.store.update(sandbox(), |r| r.workspace = Some(workspace(&app.store.paths))).unwrap();
        app.jarvis_start().await;
        settled(&app).await;
        let ran = batches(&fake, NAME);
        assert_eq!(ran.len(), 1, "{:?}", fake.log());
        assert!(writes_role_and_tools(&ran[0]));
        assert!(!ran[0].contains(sbxw_core::tooling::RELAY_DOC_BEGIN) && !ran[0].contains("SBXW_TEAM"), "Jarvis's pieces only");
        let s = app.jarvis_report().await.setup;
        assert_eq!((s.role_written, s.tools_registered), (Some(true), Some(true)));
        // And again at the start after: each start, not the first only.
        app.sessions.remove(&SessionKey::agent(sandbox()));
        app.jarvis_start().await;
        settled(&app).await;
        assert_eq!(batches(&fake, NAME).len(), 2);
    }

    #[tokio::test]
    async fn what_could_not_be_written_is_said_in_plain_words() {
        let (_dir, app, _fake) = app_with(|a| match a.first().map(String::as_str) {
            Some("ls") => ls(&[(NAME, "running")]),
            Some("exec") if a.iter().any(|x| x == "node") => {
                sbxw_sbx::runner::Output::ok("SBXW-TOOLING [[\"config-mcp\",\"/home/agent/.sbxw/jarvis/config-mcp.js is missing\"]]\n")
            }
            _ => sbx_answer(a),
        });
        app.store.update(sandbox(), |r| r.workspace = Some(workspace(&app.store.paths))).unwrap();
        // Running, and this daemon never set it up: its session predates it.
        let r = app.jarvis_report().await;
        assert_eq!((r.status.clone(), r.setup.role_written), (Status::Running, None));
        assert!(r.setup.note.as_deref().unwrap().contains("sbxw stop sbxw-jarvis"), "{r:?}");
        app.jarvis_setup().await;
        let s = app.jarvis_report().await.setup;
        assert_eq!((s.role_written, s.tools_registered), (Some(true), Some(false)));
        let note = s.note.unwrap();
        assert!(note.contains("cannot propose changes") && note.contains("sbxw rm sbxw-jarvis"), "{note}");
    }

    #[tokio::test]
    async fn another_sandbox_s_session_gets_none_of_it() {
        let (_dir, app, fake) = app_with(|a| match a.first().map(String::as_str) {
            Some("ls") => ls(&[("myapp", "running")]),
            _ => sbx_answer(a),
        });
        let other = SandboxRef::parse("myapp").unwrap();
        app.store.update(other.clone(), |r| r.workspace = Some(workspace(&app.store.paths))).unwrap();
        let _ = app.open_session(&SessionKey::agent(other)).await;
        assert!(batches(&fake, "myapp").is_empty(), "{:?}", fake.log());
        assert_eq!(setup(&app.store.paths), Setup::default());
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
