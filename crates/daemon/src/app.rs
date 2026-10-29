//! The daemon's state, all of it, in one value.
//!
//! No globals: everything a request can touch hangs off `App`, built from a
//! `Paths` and a `Config`, and shared by `Arc`. The services are `impl App`
//! blocks in `svc/`, one file per concern, so a handler is a call and a test
//! builds an `App` on a temp dir with a fake `sbx`.

use crate::bus::{Bus, Event};
use crate::hostsys::HostSys;
use crate::sessions::Sessions;
use crate::store::{Paths, Store};
use sbxw_core::agents::AgentBook;
use sbxw_core::codemap::RunBook;
use sbxw_core::config::Config;
use sbxw_core::plans::{Plan, PlanBook};
use sbxw_core::relay::RelayBook;
use sbxw_core::usage::Usage;
use sbxw_core::SandboxName;
use sbxw_sbx::Sbx;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

pub struct Versions {
    pub sbxw: String,
    pub sbx: Option<String>,
}

pub struct App {
    pub store: Store,
    pub sbx: Sbx,
    pub bus: Bus,
    pub sessions: Arc<Sessions>,
    pub hosts: HostSys,
    /// The config the daemon was started with: its listeners, the monitor,
    /// the models offered, and the defaults for sandboxes without their own.
    pub cfg: Config,
    pub versions: Versions,
    pub use_api_key: bool,
    /// The sandbox the page opens on (`sbxw web <name>`).
    pub initial_sandbox: Mutex<String>,

    pub(crate) agents: Mutex<AgentBook>,
    pub(crate) relay: Mutex<RelayBook>,
    /// Parked `wait`s per request: an approval with none is typed into the
    /// asking session instead, or it is never read.
    pub(crate) relay_waiters: Mutex<HashMap<String, usize>>,
    pub(crate) plans: Mutex<PlanBook>,
    pub(crate) boards: Mutex<HashMap<String, sbxw_core::board::Board>>,
    pub(crate) codemap: Mutex<RunBook>,
    pub(crate) usage: Mutex<Usage>,
    /// Sandboxes whose main session was made lead, and *which* session (its
    /// address): a restarted agent never read `/lead`.
    pub(crate) leads: Mutex<HashMap<String, usize>>,
    pub(crate) hook_log: Mutex<VecDeque<serde_json::Value>>,
    pub(crate) stale_hooks: Mutex<HashSet<String>>,
    /// An approved plan's briefing, for a thread whose session has not
    /// started yet — handed to `open_session`, never typed into a boot.
    pub(crate) briefings: Mutex<HashMap<(String, String), String>>,
    /// `caffeinate`, the battery and who is on Remote Control.
    pub(crate) awake: Mutex<crate::svc::awake::Awake>,
}

pub struct AppBuilder {
    pub paths: Paths,
    pub cfg: Config,
    pub sbx: Sbx,
    pub hosts: HostSys,
    pub use_api_key: bool,
    pub sbx_version: Option<String>,
}

impl AppBuilder {
    pub fn build(self) -> Arc<App> {
        let store = Store::open(self.paths);
        let plans = load_plans(&store.paths.plans());
        Arc::new(App {
            store,
            sbx: self.sbx,
            bus: Bus::new(),
            sessions: Sessions::new(),
            hosts: self.hosts,
            cfg: self.cfg,
            versions: Versions {
                sbxw: env!("CARGO_PKG_VERSION").into(),
                sbx: self.sbx_version,
            },
            use_api_key: self.use_api_key,
            initial_sandbox: Mutex::new(String::new()),
            agents: Mutex::new(AgentBook::new()),
            relay: Mutex::new(RelayBook::new()),
            relay_waiters: Mutex::new(HashMap::new()),
            plans: Mutex::new(PlanBook::load(plans)),
            boards: Mutex::new(HashMap::new()),
            codemap: Mutex::new(RunBook::new()),
            usage: Mutex::new(Usage::default()),
            leads: Mutex::new(HashMap::new()),
            hook_log: Mutex::new(VecDeque::new()),
            stale_hooks: Mutex::new(HashSet::new()),
            briefings: Mutex::new(HashMap::new()),
            awake: Mutex::new(Default::default()),
        })
    }
}

/// Plans written by this daemon or an older one (same file format). A file
/// that cannot be read costs its chips, never a thread.
fn load_plans(dir: &Path) -> Vec<Plan> {
    let mut out = Vec::new();
    for p in std::fs::read_dir(dir).into_iter().flatten().flatten().map(|e| e.path()) {
        if p.extension().is_none_or(|x| x != "json") {
            continue;
        }
        match std::fs::read_to_string(&p)
            .map_err(|e| e.to_string())
            .and_then(|t| serde_json::from_str::<Vec<Plan>>(&t).map_err(|e| e.to_string()))
        {
            Ok(list) => out.extend(list),
            Err(e) => tracing::warn!("plans: skipping {}: {e}", p.display()),
        }
    }
    out
}

impl App {
    /// The config a sandbox was brought up with, or the daemon's. Read each
    /// time: an edited `env` reaches the agent on its next attach.
    pub fn config_for(&self, name: &str) -> Config {
        let path = self.store.get(name).config.filter(|p| p.exists());
        match path {
            Some(p) => Config::load(&p, Path::new("/")).unwrap_or_else(|e| {
                tracing::warn!("{}: {e:#} — using the daemon's config", p.display());
                self.cfg.clone()
            }),
            None => self.cfg.clone(),
        }
    }

    pub fn send(&self, ev: Event) {
        self.bus.send(ev);
    }

    /// Spawn the slow loops: relay/plan pruning, session reconciliation, the
    /// usage poller, keep-awake.
    pub fn start_background(self: &Arc<Self>) {
        let app = self.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(60));
            loop {
                tick.tick().await;
                app.prune();
            }
        });
        let app = self.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(15));
            loop {
                tick.tick().await;
                app.reconcile_sessions().await;
            }
        });
        if !self.use_api_key {
            let app = self.clone();
            tokio::spawn(async move { app.poll_usage().await });
        }
        self.start_keep_awake();
        self.start_board_listener();
    }

    fn prune(&self) {
        let now = now_ms();
        let dropped = self.relay.lock().unwrap().prune(now);
        if dropped > 0 {
            tracing::info!("relay: pruned {dropped} stale request(s)");
        }
        let touched = self.plans.lock().unwrap().prune(now, sbxw_core::plans::PRUNE_AFTER_MS);
        for sb in touched {
            self.persist_plans(&sb);
        }
    }

    /// Hook state of sandboxes `sbx ls` no longer reports running is dropped,
    /// with an `Exited` first — a sandbox stopped out of band sends no
    /// `SessionEnd` — and so is their Remote Control, for keep-awake.
    async fn reconcile_sessions(&self) {
        let running: Vec<String> = self.sbx.ls().await.into_iter().filter(|s| s.is_running()).map(|s| s.name).collect();
        self.keep_remote_running(&running);
        let gone = self.agents.lock().unwrap().reconcile(&running, now_ms());
        for info in gone {
            tracing::info!("reconcile: dropped stale session of '{}' (sandbox gone)", info.sandbox);
            self.send(Event::Session(info));
        }
    }

    pub fn workspace(&self, name: &SandboxName) -> Option<std::path::PathBuf> {
        self.store.workspace(name)
    }
}
