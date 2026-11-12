//! Bringing a sandbox up: one pipeline for every way in — `sbxw up`, the
//! create and chat dialogs, Duplicate, the island's ephemeral chat.
//!
//! Steps are announced before the first one runs (a slow image pull must not
//! look wedged), each reported as it starts; `sbx`'s output streams underneath
//! as detail lines. The sink is an argument, not a thread-local: the caller
//! decides where it goes (every browser tab, the terminal that typed
//! `sbxw up`, or nowhere).
//!
//! What must hold, in order, and why:
//! 1. creation carries every kit (`--kit`, credentials first) and the ports
//!    (`-p`): creation is the only moment kits apply whole. A busy host port
//!    409s the create, so it is retried without them;
//! 2. the in-sandbox tooling (token, trust, hooks, statusLine, relay) is one
//!    `sbx exec`, merged and idempotent;
//! 3. the egress policy goes in before any `kit add`, whose startup commands
//!    need it; sbxw's own rule is the **agent** port, never the UI port;
//! 4. host aliases are best effort (the daemon cannot answer sudo) and parked;
//! 5. ports are re-published once the sandbox runs, by a task the daemon owns.

use crate::app::App;
use crate::{bundled, cache};
use crate::bus::{Event, ProvisionEvent};
use crate::store::MountRecord;
use anyhow::{anyhow, bail, Context, Result};
use sbxw_core::artifacts;
use sbxw_core::config::Config;
use sbxw_core::kits;
use sbxw_core::ports::{self, WantedPort};
use sbxw_core::progress::{self, Progress, Sink, Step};
use sbxw_core::remote;
use sbxw_core::templates;
use sbxw_core::tooling::{self, Tooling};
use sbxw_core::SandboxRef;
use sbxw_sbx::CreateOpts;
use crate::carving;
use crate::host::Host;
use crate::remotes::Remote;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

#[derive(Clone, Debug)]
pub struct BringUp {
    pub name: SandboxRef,
    pub workspace: PathBuf,
    pub ro: Vec<String>,
    /// Extra directories mounted writable beside the workspace.
    pub rw: Vec<String>,
    /// Carves the person chose in the create dialog, over the config's own
    /// (`sbxw.toml`'s `[[carve]]`): absolute host paths, no consent needed.
    pub carve: Vec<sbxw_core::config::Carve>,
    /// The project config, remembered for the sandbox so later attaches use the
    /// same `env`.
    pub config: Config,
    pub extra_ports: Vec<WantedPort>,
    pub chat: bool,
    /// Store `ANTHROPIC_API_KEY` as the global `anthropic` secret.
    pub api_key: Option<String>,
    /// What to do about an automatic template (`template = "auto"`).
    pub template: templates::Choice,
    /// Create it for Remote Control: no credentials from sbxw, a claude.ai
    /// login inside instead (`sbxw_core::remote`). Only a new sandbox takes
    /// it; a recorded one keeps its choice.
    pub remote_control: bool,
    /// Start its agent with `--chrome` from now on (Claude in Chrome).
    pub chrome: bool,
    /// Clone this into `workspace` first, on the sandbox's host, when
    /// `workspace` does not exist yet (`sbxw up --git <url>`).
    pub git: Option<String>,
    /// Whether the config's host-access grants are checked here.
    pub consent: Consent,
}

/// How a bring-up's config was reached, which decides whether its
/// host-access grants (`sbxw_core::consent`) are checked against what the
/// person approved for that file. Either way, the lines carried are the ones
/// the person has just said yes to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Consent {
    /// Read from its file for this bring-up (`sbxw up`, a chat): checked here.
    Check(Vec<String>),
    /// Checked by the caller already (the create dialog checks the daemon's
    /// config before adding the folders the person picked), or the recorded
    /// config of an earlier bring-up (Duplicate, Recreate). A remote host's
    /// `sbxw.toml`, which the bring-up reads itself, is still checked.
    Checked(Vec<String>),
}

/// Kit startup: the timeout before an automatic template is given up on, the
/// poll interval, and how long the guest is left to start the commands.
const STARTUP_TIMEOUT: Duration = if cfg!(test) { Duration::from_millis(300) } else { Duration::from_secs(20 * 60) };
const STARTUP_POLL: Duration = if cfg!(test) { Duration::from_millis(10) } else { Duration::from_secs(2) };
const STARTUP_SETTLE: Duration = if cfg!(test) { Duration::ZERO } else { Duration::from_secs(15) };
/// How long a template's builder sandbox is given to start.
/// How long a quiet process list stands in for an end marker that never
/// came (a kit written before the markers, a guest whose `/proc` hides the
/// boot id): the old judgement, held for longer.
const MARKER_GRACE: Duration = if cfg!(test) { Duration::ZERO } else { Duration::from_secs(60) };
/// A mirrored kit no sandbox uses is kept this long after its last use.
const MIRROR_UNUSED_FOR: Duration = Duration::from_secs(7 * 86_400);
const BUILDER_BOOT: Duration = if cfg!(test) { Duration::from_millis(300) } else { Duration::from_secs(60) };

/// A sandbox of the daemon's own, on an empty workspace, that nobody works
/// in: a template's builder, the kit cache's filler.
struct Scratch {
    sb: SandboxRef,
    sbx: sbxw_sbx::Sbx,
    host: Host,
    workspace: PathBuf,
}

/// What the daemon needs to fill the kit cache in a sandbox of its own.
struct CacheFill {
    host: sbxw_core::names::HostName,
    /// The cache, on the sandbox's host.
    dir: String,
    /// The kits that fill it (sbxw's base kit and the caching ones), as
    /// directories on this machine and as `sbx` on the host names them.
    resolved: Vec<String>,
    on_host: Vec<String>,
    cfg: Config,
}

/// What the daemon needs to build an automatic template on its own, once the
/// bring-up that found it missing or out of date is over.
struct TemplateBuild {
    /// The sandbox just created with these kits, when it runs: its startup
    /// installs are waited for first, so the build copies from the kit cache
    /// it filled instead of downloading beside it.
    after: Option<SandboxRef>,
    host: sbxw_core::names::HostName,
    /// The kit list as the project wrote it, base kit first: the template's
    /// identity.
    kits: Vec<String>,
    /// The same kits as directories on this machine: what is fingerprinted
    /// and read for startup commands.
    resolved: Vec<String>,
    /// The same kits as `sbx` on the sandbox's host names them.
    on_host: Vec<String>,
    cfg: Config,
}

/// Each resolved kit's `spec.yaml`, else its reference: what a template's
/// content is judged on.
fn spec_texts(resolved: &[String]) -> Vec<String> {
    resolved.iter().map(|k| kits::content_text(k)).collect()
}

/// The steps this bring-up takes, announced first; the conditional ones match
/// the body's conditions.
pub fn plan(existed: bool, cfg: &Config, template: bool) -> Vec<Step> {
    let mut steps = vec![
        Step::new(progress::WORKSPACE, "Preparing the workspace"),
        Step::new(
            progress::CREATE,
            if existed { "Reusing the existing sandbox" } else { "Creating the sandbox" },
        ),
        Step::new(progress::BOOT, "Waiting for the sandbox to start"),
        Step::new(progress::TOOLING, "Installing the agent tooling"),
    ];
    if !cfg.network_allow.is_empty() || !cfg.network_deny.is_empty() {
        steps.push(Step::new(progress::POLICY, "Applying the network policy"));
    }
    // Past sbxw's base kit, which only creation applies.
    if existed && cfg.kits.len() > 1 {
        steps.push(Step::new(progress::KITS, "Applying kits"));
    }
    steps.push(Step::new(progress::PORTS, "Publishing ports and host aliases"));
    if template {
        steps.push(Step::new(progress::TEMPLATE, "Saving the automatic template"));
    }
    steps
}

struct Run<'a> {
    sink: &'a Sink,
    warnings: Vec<String>,
}

impl Run<'_> {
    fn step(&self, id: &str) {
        (self.sink)(Progress::Started { id: id.to_string() });
    }
    fn info(&self, msg: impl AsRef<str>) {
        tracing::info!("{}", msg.as_ref());
        if let Some(p) = progress::detail(msg.as_ref()) {
            (self.sink)(p);
        }
    }
    fn warn(&mut self, msg: impl Into<String>) {
        let msg = msg.into();
        tracing::warn!("{msg}");
        (self.sink)(Progress::Warning { text: msg.clone() });
        self.warnings.push(msg);
    }
}

impl App {
    /// Run a bring-up with its progress on the bus (every tab sees it) and in
    /// `extra` (the caller's own view, e.g. the CLI's stream), closing with
    /// `Done` whichever way it went.
    pub async fn provision(self: &Arc<Self>, req: BringUp, extra: Option<Sink>) -> Result<Vec<String>> {
        let bus = self.bus.clone();
        let name = req.name.clone();
        let sink: Sink = Arc::new(move |p: Progress| {
            if let Some(f) = &extra {
                f(p.clone());
            }
            bus.send(Event::Provision(ProvisionEvent { sandbox: name.clone(), progress: p }));
        });
        let out = self.bring_up(req, &sink).await;
        sink(Progress::Done {
            error: out.as_ref().err().map(|e| format!("{e:#}")),
        });
        out
    }

    /// Refuse `cfg` while it grants host access the person has not approved
    /// for its file, counting the lines in `approved_now`, which are then
    /// recorded. `key` names the file when it is not `cfg.source` (a remote
    /// host's). The refusal is a [`NeedsApproval`](sbxw_core::consent::NeedsApproval).
    pub fn check_consent(&self, cfg: &Config, key: Option<String>, approved_now: &[String]) -> Result<()> {
        let all = sbxw_core::consent::grants(cfg);
        let key = key.unwrap_or_else(|| match &cfg.source {
            Some(p) => crate::store::grant_key(p),
            None => "(built-in defaults)".into(),
        });
        if all.is_empty() && !self.store.all_approved_grants().contains_key(&key) {
            return Ok(());
        }
        self.store.check_grants(&key, &all, &all, approved_now)
    }

    async fn bring_up(self: &Arc<Self>, mut req: BringUp, sink: &Sink) -> Result<Vec<String>> {
        // A remote host: its link first. The -R tunnel the hooks call home
        // through must be up before they are installed.
        let on_host: Option<Arc<Remote>> = match req.name.is_local() {
            true => None,
            false => Some(self.ready_remote(&req.name).await?),
        };
        let host = self.host_for(&req.name).await?;
        let sbx = self.sbx_for(&req.name).await?;
        if let Some(url) = &req.git {
            clone_if_missing(&host, url, &req.workspace).await?;
        }
        if on_host.is_some() {
            // sbxw.toml is the project's, so it is on the host, beside the code.
            req.config = remote_config(&host, &req.workspace).await?;
        }
        match (&on_host, &req.consent) {
            (Some(_), Consent::Check(ok) | Consent::Checked(ok)) => {
                let key = format!("{}:{}", req.name.host, req.workspace.join("sbxw.toml").display());
                self.check_consent(&req.config, Some(key), ok)?;
            }
            (None, Consent::Check(ok)) => self.check_consent(&req.config, None, ok)?,
            (None, Consent::Checked(_)) => {}
        }
        // What later attaches run with, as the project wrote it (no base kit,
        // no resolved paths): the file is not read again before the next `up`.
        let snapshot = Box::new(req.config.clone());
        // sbxw's own kit (Node.js) first, on every sandbox: everything sbxw
        // runs inside one needs it, and sbx 0.51's image has none.
        req.config.kits = kits::with_base(&req.config.kits);
        // Secrets (the OAuth token, the API key, the kept skills) go only to a
        // host trusted with them.
        let trusted = on_host.as_ref().is_none_or(|r| r.cfg.trust_secrets);
        let name = &req.name;
        let n = &name.name;
        let mut wants_cache = req.config.cache && cache::wanted_by(&req.config.kits);
        // `sbxw:<name>` kits become the stable directories they are written to.
        let cfg = &Config {
            kits: bundled::resolve(&self.store.paths, &req.config.kits)?,
            ..req.config.clone()
        };
        let mut run = Run { sink, warnings: vec![] };
        let workspace = req.workspace.to_string_lossy().into_owned();
        if !is_dir_on(&host, &req.workspace).await? {
            bail!("{workspace} is not a directory{}", on_host.as_ref().map(|r| format!(" on host '{}'", r.cfg.name)).unwrap_or_default());
        }
        self.validate_kits(&req.config.kits)?;

        // Asked first: it decides two of the announced steps.
        let initial = sbx.lookup(n).await;
        let existed = initial.is_some();
        // An existing sandbox keeps its creation workspace: sbx cannot move its
        // mount. Recording another directory would point the Project panel, env
        // and config at the wrong project, so refuse it before anything changes.
        // Read once: nothing below changes it before the update that follows.
        let before = self.store.get(name);
        if existed {
            if let Some(recorded) = before.workspace.clone() {
                let canon = async |p: &Path| host.fs.canonicalize(p).await.unwrap_or_else(|_| p.to_path_buf());
                if canon(&recorded).await != canon(&req.workspace).await {
                    bail!(
                        "'{name}' was created on {}, not {workspace} — run `sbxw up {name}` from {0}, or pass it: `sbxw up {name} {0}`. A sandbox cannot change workspace; for another one, use another name",
                        recorded.display()
                    );
                }
            }
        }
        let stopped = initial.as_ref().is_some_and(|i| i.is_stopped());
        let mut running = Some(initial.as_ref().is_some_and(|i| i.is_running()));
        let (template, rebuild, background) = self.template_for(&req, cfg, existed, &mut run).await;
        // A template holds the kits' installs, so its kits would never fill the
        // cache this sandbox claimed.
        wants_cache &= template.is_none();
        sink(Progress::Plan { steps: plan(existed, cfg, rebuild) });

        // ── Workspace ────────────────────────────────────────────────────
        run.step(progress::WORKSPACE);
        // The credentials kit would come back on every container swap, so
        // only creation can leave it out.
        let recorded_remote = before.remote_control;
        if existed && req.remote_control && !recorded_remote {
            run.warn(format!(
                "'{name}' was created without Remote Control — press Remote in its pane to switch it to a claude.ai login"
            ));
        }
        // sbx 0.53 keeps Remote Control's credentials on the host, behind its
        // `claude.remoteControl` setting: a sandbox signed in the usual way
        // takes /remote-control, and none is created without credentials any
        // more. An older sbx does not know the setting.
        let native_remote = match !existed && req.remote_control {
            true => sbx.setting_bool(remote::SETTING, remote::SINCE).await?,
            false => None,
        };
        let remote = recorded_remote || (!existed && req.remote_control && native_remote.is_none());
        let chrome = cfg.chrome || req.chrome || before.chrome;
        self.store
            .update(name, |r| {
                r.remote_control = remote;
                r.chrome |= req.chrome;
                r.workspace = Some(req.workspace.clone());
                r.config.clone_from(&cfg.source);
                r.config_snapshot = Some(snapshot);
                r.chat = req.chat;
                // Reused, the sandbox keeps the mounts it was created with.
                if !existed {
                    let mount = |p: &String, readonly| MountRecord { path: p.into(), readonly };
                    r.mounts = req.ro.iter().map(|p| mount(p, true)).chain(req.rw.iter().map(|p| mount(p, false))).collect();
                }
            })
            .context("recording the sandbox")?;
        let token = self.store.token(name)?;
        let artifacts_dir = req.workspace.join(artifacts::ARTIFACTS_DIR);
        if !matches!(host.fs.stat(&artifacts_dir).await, Ok(Some(_))) {
            if host.fs.create_dir_all(&artifacts_dir).await.is_ok() {
                let _ = host.fs.write_atomic(&artifacts_dir.join("README.md"), artifacts::README.as_bytes(), 0o644).await;
            } else {
                run.warn(format!("could not create {}", artifacts_dir.display()));
            }
        }
        let wanted = cfg.wanted_ports(&req.extra_ports);
        // Forwarded from the host's 127.0.0.1 over the link: one address per
        // app means nothing there.
        let specs = ports::publish_specs(&wanted, cfg.ip_per_app && on_host.is_none());
        let mut own_allow: Vec<String> = Vec::new();
        if remote {
            run.info("Remote Control: no credentials injected — log in with /login in the agent's pane");
            own_allow.extend(remote::LOGIN_HOSTS.iter().map(|h| h.to_string()));
        }
        match native_remote {
            Some(true) => {
                run.info("Remote Control: sbx keeps its credentials on the host — press Remote in the agent's pane");
                self.watch_remote(name);
            }
            Some(false) => {
                run.warn(remote::setting_advice());
                self.watch_remote(name);
            }
            None => {}
        }
        if chrome {
            match sbx.setting_bool(sbxw_core::chrome::SETTING, sbxw_core::chrome::SINCE).await {
                Ok(Some(true)) => run.info("Claude in Chrome: the agent starts with --chrome"),
                Ok(Some(false)) => run.warn(sbxw_core::chrome::setting_advice()),
                Ok(None) => run.warn(sbxw_core::chrome::NEEDS_SBX),
                Err(e) => run.warn(format!("Claude in Chrome: could not read sbx's `{}` setting: {e:#}", sbxw_core::chrome::SETTING)),
            }
            if req.api_key.as_deref().is_some_and(|k| !k.trim().is_empty()) {
                run.warn(sbxw_core::chrome::NEEDS_SUBSCRIPTION);
            }
        }

        // ── Create, or reuse ─────────────────────────────────────────────
        run.step(progress::CREATE);
        let mut created_with_kits = false;
        // The project's kits as `sbx` on the sandbox's host names them:
        // mirrored there for a remote host, as they are here.
        let (kits_here, mut mirrored) = self.kits_on(on_host.as_deref(), &cfg.kits, &run).await?;
        if existed {
            run.info(format!("sandbox '{name}' already exists — reusing it"));
            // Mounts too: a changed set of carves is a recreate, never applied.
            let had = self.store.get(name).carve.map(|c| c.file).unwrap_or_default();
            let now = sbxw_core::consent::carve_lines(cfg);
            if sorted(&had) != sorted(&now) {
                run.warn(format!(
                    "carves apply when a sandbox is created — '{name}' is unchanged. \
                     `sbxw rm {name}` then `sbxw up {name}` creates it with them."
                ));
            }
            // Block storage exists only at creation: a changed list is a recreate,
            // never applied.
            if !cfg.volumes.is_empty() {
                let wanted = kits::volumes_kit_spec(&cfg.volumes);
                let had = std::fs::read_to_string(self.store.paths.volumes_kit(name).join("spec.yaml")).ok();
                let created_with = self.store.get(name).kits.iter().any(|k| std::path::Path::new(k) == self.store.paths.volumes_kit(name));
                if !created_with || had.as_deref() != Some(wanted.as_str()) {
                    run.warn(format!(
                        "volumes ({}) apply when a sandbox is created — '{name}' is unchanged. \
                         `sbxw rm {name}` then `sbxw up {name}` creates it with them.",
                        cfg.volumes.join(", ")
                    ));
                }
            }
        } else {
            run.info(format!("creating sandbox '{name}' on workspace {workspace}"));
            let mut kit_refs: Vec<String> = Vec::new();
            // sbxw's own kits, written here; mirrored together below.
            for k in &cfg.kits {
                if kits::needs_allowlist(k) {
                    run.warn(format!(
                        "kit '{k}' is a Git URL or non-Docker-Hub registry — allow it with \
                         `sbx settings set kit.allowedSources <prefix>`"
                    ));
                }
            }
            kit_refs.extend(kits_here.iter().cloned());
            let mut own: Vec<String> = Vec::new();
            // Kept skills go to every new sandbox, whatever its `kits` say: they
            // are the person's, not the project's.
            match self.my_skills_kit() {
                Ok(Some(_)) if !trusted => run.info(format!("your kept skills stay here: host '{}' is not trusted with secrets", name.host)),
                Ok(Some(dir)) => own.push(dir.to_string_lossy().into_owned()),
                Ok(None) => {}
                Err(e) => run.warn(format!("your kept skills are left out: {e:#}")),
            }
            if !cfg.volumes.is_empty() {
                let dir = self.store.write_volumes_kit(name, &cfg.volumes)?;
                run.info(format!("persistent volumes: {} (kept if the sandbox is removed)", cfg.volumes.join(", ")));
                own.push(dir.to_string_lossy().into_owned());
            }
            let (own_here, sent) = self.kits_on(on_host.as_deref(), &own, &run).await?;
            mirrored |= sent;
            kit_refs.extend(own_here);
            // Carves: mounts are fixed now, so the plan is made now.
            let all_carves: Vec<sbxw_core::config::Carve> = cfg.carve.iter().chain(req.carve.iter()).cloned().collect();
            let mut primary = workspace.clone();
            let mut workspaces = cfg.workspaces.clone();
            let (mut ro_mounts, mut rw_mounts) = (req.ro.clone(), req.rw.clone());
            let mut carve_record = None;
            if !all_carves.is_empty() {
                let (stub, mask) = self.carve_dirs(name).await?;
                let extras: Vec<sbxw_core::config::Workspace> = cfg
                    .workspaces
                    .iter()
                    .filter_map(|w| sbxw_core::config::parse_workspace(w).ok())
                    .chain(req.ro.iter().map(|p| sbxw_core::config::Workspace { path: p.clone(), target: None, readonly: true }))
                    .chain(req.rw.iter().map(|p| sbxw_core::config::Workspace { path: p.clone(), target: None, readonly: false }))
                    .collect();
                let plan = carving::plan_on(&host, &req.workspace, &extras, &all_carves, &stub, &mask, &cfg.workdir).await?;
                carving::make_helpers(&host, &stub, &mask).await?;
                for w in &plan.warnings {
                    run.warn(w.clone());
                }
                run.info(format!(
                    "{} carve{}: {} mounts{}, {} entries not visible",
                    all_carves.len(),
                    if all_carves.len() == 1 { "" } else { "s" },
                    1 + plan.mounts.len(),
                    if plan.stub { " (the workspace is split)" } else { "" },
                    plan.unshared.len()
                ));
                primary = plan.primary_spec();
                workspaces = plan.specs();
                (ro_mounts, rw_mounts) = (vec![], vec![]);
                let split = match plan.stub {
                    true => Some(canon_string(&host, &req.workspace).await),
                    false => None,
                };
                carve_record = Some(crate::store::CarveRecord { file: sbxw_core::consent::carve_lines(cfg), extra: req.carve.clone(), split });
            }
            let mut env = if remote { remote::without_auth_env(cfg.env_pairs()) } else { cfg.env_pairs() };
            if wants_cache {
                // Read-only, always: a sandbox someone works in never writes
                // what the others will run. See `cache`.
                let (dir, filling) = self.cache_for(name, cfg, &req.config.kits, &cfg.kits, &kits_here).await?;
                if filling {
                    run.info("the shared kit cache is being filled in the background — this sandbox installs from the network, the next ones from the cache");
                }
                env.push(format!("SBXW_CACHE={dir}"));
                ro_mounts.push(dir);
            }
            let opts = CreateOpts {
                workspace: primary,
                ro_mounts,
                kits: kit_refs,
                publish: specs.clone(),
                kit_mounts: cfg.kit_mount_specs(),
                skills: cfg.skills_mode().map(String::from),
                pull: cfg.pull_policy().map(String::from),
                env,
                env_files: cfg.env_files.clone(),
                rw_mounts,
                template,
                workspaces,
                cpus: (cfg.cpus > 0).then_some(cfg.cpus),
                memory: Some(cfg.memory.clone()).filter(|m| !m.is_empty()),
            };
            let applied = opts.kits.clone();
            self.create_with_port_fallback(name, &opts, sink, &mut run).await?;
            created_with_kits = true;
            // A kit that changed was just mirrored to its host under a new
            // directory: the moment the ones it replaces may have become
            // nobody's. After the create, so this sandbox counts as a user.
            if let (true, Some(r)) = (mirrored, &on_host) {
                self.prune_mirrors_later(r.clone());
            }
            // Created by this sbxw: its kits are known, and on this machine
            // the sbx that made it too (a remote host's is asked of it).
            let made_by = name.is_local().then(|| self.versions.sbx.clone().map(|v| format!("v{}", v.trim_start_matches('v')))).flatten();
            let _ = self.store.update(name, |r| {
                r.kits = applied;
                r.created_by_sbx = made_by;
                r.carve = carve_record;
            });
        }

        if cfg.keep_running {
            let up = match running {
                Some(r) => r,
                None => sbx.is_running(n).await,
            };
            if !existed || !up {
                match sbx.run_detached(n).await {
                    Ok(()) => {
                        running = None;
                        run.info(format!("'{name}' kept running until `sbxw stop {name}`"));
                    }
                    Err(e) => run.warn(format!("keep_running: could not hold '{name}' up: {e:#}")),
                }
            }
        }

        // ── Boot ─────────────────────────────────────────────────────────
        run.step(progress::BOOT);
        let up = match running {
            Some(true) => true,
            Some(false) if stopped => false,
            _ => sbx.wait_until_running(n, Duration::from_secs(30)).await,
        };

        // ── Tooling and policy ───────────────────────────────────────────
        // Independent (one a guest script, one a host-side rule), so they run
        // side by side and report in order. The daemon's agent port, whatever
        // the project file says (the hooks must reach the listener); on a remote
        // host, the far end of the link's -R tunnel.
        let agent_port = self.agent_port_for(name).await?;
        // The proxy sees host.docker.internal as localhost:<port>.
        own_allow.push(format!("localhost:{agent_port}"));
        let allow = [own_allow.as_slice(), cfg.network_allow.as_slice()].concat().join(",");
        let batch = Tooling::new()
            .agent_token(&token)
            .trust_workspace(&workspace)
            .artifact_hook()
            .status_hooks(&agent_port)
            .usage_statusline(&agent_port)
            .relay(&agent_port)
            .lead_command();
        if up {
            run.step(progress::TOOLING);
        }
        let (tooled, allowed) = tokio::join!(
            async {
                match up {
                    true => Some(sbx.tooling(n, &batch).await),
                    false => None,
                }
            },
            sbx.policy("allow", Some(n), &allow)
        );
        if let Some(tooled) = tooled {
            match tooled {
                Ok(outcome) => {
                    for (piece, e) in &outcome.failed {
                        run.warn(format!("{}: {e}", tooling::consequence(piece)));
                    }
                }
                Err(e) if kits::node_missing(&format!("{e:#}")) => run.warn(format!(
                    "'{name}' has no Node.js, which sbxw's tooling (token, trust, hooks, statusLine, relay) runs on. \
                     sbxw's base kit installs it at creation: recreate the sandbox (`sbxw rm {name}` then `sbxw up {name}`), \
                     after saving anything outside the workspace.\n{e:#}"
                )),
                Err(e) => run.warn(format!(
                    "could not install the agent tooling (token, trust, hooks, statusLine, relay): {e:#}"
                )),
            }
        } else if stopped {
            run.info(format!(
                "sandbox '{name}' is stopped; its tooling is refreshed on the next bring-up while it runs"
            ));
        } else {
            run.warn(format!("sandbox '{name}' did not come up in time; skipping the agent tooling"));
        }

        if !cfg.network_allow.is_empty() || !cfg.network_deny.is_empty() {
            run.step(progress::POLICY);
        }
        match allowed {
            Ok(()) => {}
            Err(e) if kits::policy_is_governed(&format!("{e:#}")) => {
                run.warn(format!("network allowlist not applied — your organization manages this policy:\n{e:#}"))
            }
            Err(e) if cfg.network_allow.is_empty() => run.warn(format!("could not allow {allow} egress: {e:#}")),
            Err(e) => return Err(e).context("failed to apply the network allowlist"),
        }
        if !cfg.network_deny.is_empty() {
            match sbx.policy("deny", Some(n), &cfg.network_deny.join(",")).await {
                Ok(()) => {}
                Err(e) if kits::policy_is_governed(&format!("{e:#}")) => {
                    run.warn(format!("network denylist not applied — your organization manages this policy:\n{e:#}"))
                }
                Err(e) => return Err(e).context("failed to apply the network denylist"),
            }
        }

        // ── Kits, for a sandbox that already existed ─────────────────────
        // The first kit is sbxw's base kit, applied at creation only.
        if !created_with_kits && cfg.kits.len() > 1 {
            run.step(progress::KITS);
            let record = self.store.get(name);
            let recorded = record.kits;
            // A kit sbx refused to add to an existing sandbox is not offered
            // to it again while neither the kit nor sbx changed: the answer
            // would be the same, after sbx has resolved the whole composition.
            let sbx_version = self.template_versions().0;
            let refusal = |kit: &str| format!("{}@{sbx_version} {kit}", templates::fingerprint(&[kits::content_text(kit)]));
            let mut refused: Vec<String> = Vec::new();
            let pending: Vec<&String> = kits_here.iter().skip(1).filter(|k| !recorded.contains(k)).collect();
            // `sbx inspect` only when sbxw's own record does not settle it.
            let present = match pending.is_empty() {
                true => Vec::new(),
                false => sbx.inspect_kits(n).await.unwrap_or_default(),
            };
            for (kit, wanted) in kits_here.iter().zip(&req.config.kits) {
                // sbxw's base kit is creation-time only: a sandbox made before it
                // has its image's Node.js, and one lacking it is told so by the
                // tooling step.
                if kits::bundled_name(wanted) == Some(kits::BASE) {
                    continue;
                }
                if kits::already_applied(kit, &present, &recorded) {
                    run.info(format!("kit '{kit}' already applied — skipping"));
                    if !recorded.contains(kit) {
                        let k = kit.clone();
                        let _ = self.store.update(name, |r| r.kits.push(k));
                    }
                    continue;
                }
                if record.kits_refused.contains(&refusal(kit)) {
                    refused.push(refusal(kit));
                    run.warn(format!(
                        "kit '{kit}' still waits for '{name}' to be recreated: sbx refused to add it to an existing \
                         sandbox, and neither the kit nor sbx changed since. `sbxw rm {name}` then `sbxw up {name}` \
                         picks it up; anything outside the workspace mount is lost in the process."
                    ));
                    continue;
                }
                run.info(format!("applying kit {kit} (this recreates the container; state is kept)"));
                match self.sbx_for(name).await?.kit_add(n, kit).await {
                    Ok(()) => {
                        let k = kit.clone();
                        let _ = self.store.update(name, |r| r.kits.push(k));
                    }
                    // sbx asks a person to approve the network rules the kit
                    // adds, and has no flag to: hand them the command.
                    Err(e) if kits::add_needs_approval(&format!("{e:#}")) => {
                        {
                            let mut waiting = self.kit_approvals.lock().unwrap();
                            let list = waiting.entry(name.clone()).or_default();
                            if !list.contains(kit) {
                                list.push(kit.clone());
                            }
                        }
                        run.warn(format!(
                            "kit '{kit}' asks for network access '{name}' was not granted, and sbx wants you to approve it: \
                             press ✓ on '{name}' in the sidebar to answer in a pane, or run `sbx kit add {n} {kit}` in a terminal.\n{e:#}"
                        ))
                    }
                    Err(e) if kits::add_needs_recreate(&format!("{e:#}")) => {
                        refused.push(refusal(kit));
                        run.warn(format!(
                            "`sbx kit add` cannot apply kit '{kit}' to an existing sandbox — '{name}' is \
                             unchanged. Recreate it to pick the kit up: `sbxw rm {name}` then `sbxw up {name}`. \
                             Anything outside the workspace mount is lost in the process.\n{e:#}"
                        ))
                    }
                    Err(e) => run.warn(format!("kit '{kit}' failed to apply: {e:#}")),
                }
            }
            // Only the refusals that still stand: a kit dropped from `kits`,
            // edited, or met by another sbx is forgotten.
            if refused != record.kits_refused {
                let _ = self.store.update(name, |r| r.kits_refused = refused);
            }
        }

        if let Some(key) = req.api_key.as_deref().map(str::trim).filter(|k| !k.is_empty() && !remote) {
            if trusted {
                run.info("storing ANTHROPIC_API_KEY as the global `anthropic` secret");
                sbx.secret_set("anthropic", key, None).await.context("failed to store the anthropic secret")?;
            } else {
                run.warn(format!("ANTHROPIC_API_KEY is not sent to host '{}' (trust_secrets = false)", name.host));
            }
        }

        // ── Aliases and ports ────────────────────────────────────────────
        run.step(progress::PORTS);
        // A remote sandbox's ports are reached through the link, on a local
        // port told when it is forwarded, as `<name>.<host>.localhost`: the
        // /etc/hosts aliases name this machine's sandboxes only.
        if on_host.is_none() {
            let aliases = cfg.wanted_aliases(&req.extra_ports);
            let app = self.clone();
            let warnings = tokio::task::spawn_blocking(move || app.hosts.apply(&app.store, &aliases))
                .await
                .unwrap_or_default();
            for w in warnings {
                run.warn(w);
            }
            for p in wanted.iter().filter(|p| !p.alias.is_empty()) {
                run.info(format!("alias ready: http://{}:{} (sandbox :{})", p.alias, p.host_port, p.sandbox_port));
            }
        }
        self.republish_ports(name.clone(), specs, up);

        // ── Automatic template ───────────────────────────────────────────
        // Last, and before anyone attaches: the snapshot holds the kits'
        // installs and no one's work. A failure costs the template, never the
        // sandbox.
        if rebuild {
            run.step(progress::TEMPLATE);
            if !up {
                run.warn(format!("'{name}' is not running — the automatic template was not saved"));
            } else if let Err(e) = self.save_auto_template(name, &req.config.kits, &cfg.kits, cfg.keep_running, &mut run).await {
                run.warn(format!("the automatic template was not saved: {e:#}"));
            }
        }
        if background {
            self.build_template_later(TemplateBuild {
                after: up.then(|| name.clone()),
                host: name.host.clone(),
                kits: req.config.kits.clone(),
                resolved: cfg.kits.clone(),
                on_host: kits_here,
                cfg: cfg.clone(),
            });
        }
        Ok(run.warnings)
    }

    /// The template `sbx create` starts from, and whether this bring-up saves
    /// a new automatic one. Only a sandbox being created is concerned.
    async fn template_for(&self, req: &BringUp, cfg: &Config, existed: bool, run: &mut Run<'_>) -> (Option<String>, bool, bool) {
        if existed {
            if req.template == templates::Choice::Rebuild {
                run.warn(format!(
                    "'{}' already exists — a template is only rebuilt from a sandbox being created, \
                     before anyone has worked in it. Pick a new name to rebuild it",
                    req.name
                ));
            }
            return (None, false, false);
        }
        match req.config.template_mode() {
            templates::Mode::Off => (None, false, false),
            templates::Mode::Tag(t) => {
                run.info(format!("creating from template {t}"));
                (Some(t), false, false)
            }
            templates::Mode::Auto => {
                let (sbx, sbxw) = self.template_versions();
                let content = templates::fingerprint(&spec_texts(&cfg.kits));
                let now = templates::Current { sbx: &sbx, sbxw, content: &content };
                let mut fresh = templates::check(&self.store.templates_of(&req.name.host), &req.config.kits, &now);
                use templates::{Choice, Freshness};
                // Asked of sbx only for a template this bring-up would use.
                let used = match (&fresh, req.template) {
                    (Freshness::Fresh { record }, Choice::Default | Choice::Keep) | (Freshness::Stale { record, .. }, Choice::Keep) => Some(record.tag.clone()),
                    _ => None,
                };
                if let Some(tag) = used {
                    if !self.template_present(&req.name, &tag).await {
                        self.forget_template(&req.name, &tag, run);
                        fresh = Freshness::Missing;
                    }
                }
                match (fresh, req.template) {
                    (_, Choice::Skip) => (None, false, false),
                    (_, Choice::Rebuild) => (None, true, false),
                    (Freshness::Fresh { record }, _) => {
                        self.template_used(&req.name, &record.tag);
                        run.info(format!("creating from the automatic template {} (sbx {}, sbxw {})", record.tag, record.sbx, record.sbxw));
                        (Some(record.tag), false, false)
                    }
                    (Freshness::Stale { record, .. }, Choice::Keep) => {
                        self.template_used(&req.name, &record.tag);
                        run.info(format!("creating from the out-of-date automatic template {}, as asked", record.tag));
                        (Some(record.tag), false, false)
                    }
                    // Nobody waits for a template: this sandbox is created
                    // from sbx's image and opens as soon as it is up, and the
                    // daemon builds the template beside it, from a sandbox of
                    // its own that nobody works in.
                    (Freshness::Stale { changed, .. }, Choice::Default) => {
                        run.info(format!(
                            "the automatic template is out of date ({}) — creating from sbx's image; \
                             a new one is built in the background for the next sandboxes",
                            changed.join(", ")
                        ));
                        (None, false, true)
                    }
                    (Freshness::Missing, _) => {
                        run.info(
                            "no automatic template for these kits yet — creating from sbx's image; \
                             one is built in the background for the next sandboxes",
                        );
                        (None, false, true)
                    }
                }
            }
        }
    }

    /// Note that a sandbox is being created from the automatic template
    /// `tag`: one nobody uses for a month is removed when another is saved.
    fn template_used(&self, sb: &SandboxRef, tag: &str) {
        let mut records = self.store.templates_of(&sb.host);
        if let Some(r) = records.iter_mut().find(|r| r.tag == tag) {
            r.used_at = unix_now();
            let _ = self.store.set_templates_of(&sb.host, &records);
        }
    }

    /// Whether sbx still has template `tag`. A record outlives a template
    /// removed by `sbx template rm` or a Docker prune, and `create --template`
    /// with a missing tag goes to a registry and fails (403), which no retry
    /// mends. A listing sbx refuses proves nothing, so it counts as present.
    async fn template_present(&self, sb: &SandboxRef, tag: &str) -> bool {
        let Ok(sbx) = self.sbx_for(sb).await else { return true };
        match sbx.template_ls().await {
            Ok(list) => list.iter().any(|t| t.tag == tag || t.tag.ends_with(&format!("/{tag}"))),
            Err(_) => true,
        }
    }

    /// Drop the record of an automatic template sbx no longer has, so this
    /// bring-up and the create dialog both see it as missing.
    fn forget_template(&self, sb: &SandboxRef, tag: &str, run: &mut Run<'_>) {
        let mut records = self.store.templates_of(&sb.host);
        records.retain(|r| r.tag != tag);
        match self.store.set_templates_of(&sb.host, &records) {
            Ok(()) => run.warn(format!("the automatic template {tag} is no longer in sbx (removed outside sbxw?) — its record is dropped")),
            Err(e) => run.warn(format!("the automatic template {tag} is no longer in sbx, and its record could not be dropped: {e:#}")),
        }
    }

    /// Build an automatic template in the background, in a sandbox of the
    /// daemon's own: nobody waits for it, and a failure costs the template,
    /// never a sandbox. One build per template at a time; one that failed is
    /// not started again until the daemon restarts, so a kit whose install
    /// never ends does not cost a builder per new sandbox.
    fn build_template_later(self: &Arc<Self>, build: TemplateBuild) {
        let (sbx, sbxw) = self.template_versions();
        let key = format!("{}/{}", build.host, templates::auto_tag(&build.kits, sbxw, &sbx));
        if !self.template_builds.lock().unwrap().insert(key.clone()) {
            return;
        }
        let app = self.clone();
        tokio::spawn(async move {
            match app.build_template(&build).await {
                Ok(()) => {
                    app.template_builds.lock().unwrap().remove(&key);
                }
                Err(e) => tracing::warn!("the automatic template {key} was not built: {e:#}"),
            }
        });
    }

    /// Create a sandbox with `build`'s kits on an empty workspace, wait for
    /// their installs, save it as the template, remove it. It gets no
    /// tooling, no token, no project environment, no port and none of the
    /// person's own kits: what a template holds is the kits' installs, and
    /// everything else is put in again at each bring-up.
    async fn build_template(self: &Arc<Self>, build: &TemplateBuild) -> Result<()> {
        let sink: Sink = Arc::new(|_| {});
        let mut run = Run { sink: &sink, warnings: vec![] };
        let name = sbxw_core::names::SandboxName::parse(&templates::builder_name(&build.kits)).map_err(|e| anyhow!("{e}"))?;
        let builder = SandboxRef::new(build.host.clone(), name);
        if let Some(first) = &build.after {
            // Best effort: a sandbox stopped or removed meanwhile is not waited for.
            let (commands, marked) = self.startup_commands(&build.resolved);
            let _ = self.wait_for_startups(first, &commands, &marked, &run).await;
        }
        let cfg = &build.cfg;
        let (mut env, mut ro_mounts) = (vec![], vec![]);
        if cfg.cache && cache::wanted_by(&build.kits) {
            let (dir, _) = self.cache_for(&builder, cfg, &build.kits, &build.resolved, &build.on_host).await?;
            env.push(format!("SBXW_CACHE={dir}"));
            ro_mounts.push(dir);
        }
        tracing::info!("building the automatic template for {} in '{builder}'", build.kits.join(", "));
        let scratch = self.scratch_up(builder, "template-build", build.on_host.clone(), cfg, env, ro_mounts, vec![]).await?;
        let built = self.save_auto_template(&scratch.sb, &build.kits, &build.resolved, false, &mut run).await;
        self.scratch_down(&scratch).await;
        built
    }

    /// The kit cache as a sandbox with `kits` mounts it, which is read-only
    /// whoever asks, and whether it is being filled. When a kit of the list
    /// has not filled it, or its fill expired, the daemon fills it in a
    /// sandbox of its own ([`App::fill_cache`]) while the sandbox being
    /// created installs from the network. `kits`, `resolved` and `on_host`
    /// are the same kits: as the project names them, as directories here,
    /// and as `sbx` on the sandbox's host names them.
    async fn cache_for(self: &Arc<Self>, sb: &SandboxRef, cfg: &Config, kits: &[String], resolved: &[String], on_host: &[String]) -> Result<(String, bool)> {
        let host = self.host_for(sb).await?;
        let root = self.root_on(sb).await?;
        let (dir, mount) = cache::claim_on(&*host.fs, &root, cfg.cache_ttl(), kits).await?;
        let dir = dir.to_string_lossy().into_owned();
        if mount == cache::Mount::Fill {
            let fills: Vec<usize> = kits.iter().enumerate().filter(|(_, k)| cache::fills(k)).map(|(i, _)| i).collect();
            let pick = |from: &[String]| fills.iter().filter_map(|i| from.get(*i).cloned()).collect::<Vec<_>>();
            let fill = CacheFill { host: sb.host.clone(), dir: dir.clone(), resolved: pick(resolved), on_host: pick(on_host), cfg: cfg.clone() };
            let app = self.clone();
            tokio::spawn(async move {
                match app.fill_cache(&fill).await {
                    Ok(()) => tracing::info!("the kit cache on host '{}' is filled", fill.host),
                    // The claim's lock stands for half an hour: the next
                    // sandbox created after that starts another fill.
                    Err(e) => tracing::warn!("the kit cache on host '{}' was not filled: {e:#}", fill.host),
                }
            });
        }
        Ok((dir, mount == cache::Mount::Fill))
    }

    /// Fill the kit cache from a sandbox of the daemon's own: created with
    /// the caching kits and the cache writable, left to run their startup
    /// installs (which download into it and mark it), then removed. Nobody
    /// works in it and it has no workspace to read, so nothing a person or
    /// an agent does can reach what every later sandbox copies and runs.
    async fn fill_cache(self: &Arc<Self>, fill: &CacheFill) -> Result<()> {
        let sink: Sink = Arc::new(|_| {});
        let run = Run { sink: &sink, warnings: vec![] };
        let name = sbxw_core::names::SandboxName::parse(cache::FILLER).map_err(|e| anyhow!("{e}"))?;
        let filler = SandboxRef::new(fill.host.clone(), name);
        let env = vec![format!("SBXW_CACHE={}", fill.dir), "SBXW_CACHE_FILL=1".to_string()];
        tracing::info!("filling the kit cache on host '{}' in '{filler}'", fill.host);
        let scratch = self.scratch_up(filler, "cache-fill", fill.on_host.clone(), &fill.cfg, env, vec![], vec![fill.dir.clone()]).await?;
        let (commands, marked) = self.startup_commands(&fill.resolved);
        let filled = self.wait_for_startups(&scratch.sb, &commands, &marked, &run).await;
        self.scratch_down(&scratch).await;
        filled
    }

    /// `~/.sbxw` on `sb`'s host.
    async fn root_on(&self, sb: &SandboxRef) -> Result<PathBuf> {
        match sb.is_local() {
            true => Ok(self.store.paths.root.clone()),
            false => self.ready_remote(sb).await?.root().await,
        }
    }

    /// Create `sb` with `kits` on an empty workspace under `~/.sbxw/<folder>/`
    /// and hold it up, with the project's sizing and egress and nothing else
    /// of the project: no tooling, token, environment, port or mount beyond
    /// the ones given. Removed again if it does not come up.
    #[allow(clippy::too_many_arguments)]
    async fn scratch_up(&self, sb: SandboxRef, folder: &str, kits: Vec<String>, cfg: &Config, env: Vec<String>, ro_mounts: Vec<String>, rw_mounts: Vec<String>) -> Result<Scratch> {
        let sbx = self.sbx_for(&sb).await?;
        let host = self.host_for(&sb).await?;
        let n = &sb.name;
        // One left by a daemon that stopped midway.
        if sbx.exists(n).await {
            sbx.rm(std::slice::from_ref(n)).await?;
        }
        let workspace = self.root_on(&sb).await?.join(folder).join(n.as_str());
        host.fs.create_dir_all(&workspace).await.with_context(|| format!("creating {}", workspace.display()))?;
        let opts = CreateOpts {
            workspace: workspace.to_string_lossy().into_owned(),
            ro_mounts,
            kits,
            publish: vec![],
            kit_mounts: cfg.kit_mount_specs(),
            skills: cfg.skills_mode().map(String::from),
            pull: cfg.pull_policy().map(String::from),
            env,
            env_files: vec![],
            rw_mounts,
            template: None,
            workspaces: vec![],
            cpus: (cfg.cpus > 0).then_some(cfg.cpus),
            memory: Some(cfg.memory.clone()).filter(|m| !m.is_empty()),
        };
        sbx.create(n, &opts, None).await.with_context(|| format!("creating '{sb}'"))?;
        let scratch = Scratch { sb, sbx, host, workspace };
        let up = async {
            let (sbx, n) = (&scratch.sbx, &scratch.sb.name);
            // Held up with nobody attached: the kits' startup runs in a
            // running sandbox only.
            sbx.run_detached(n).await?;
            if !sbx.wait_until_running(n, BUILDER_BOOT).await {
                bail!("'{}' did not start", scratch.sb);
            }
            // The project's egress, as its own sandboxes get it: a kit's
            // install may count on it. Best effort, as an organization may
            // manage the policy.
            if !cfg.network_allow.is_empty() {
                if let Err(e) = sbx.policy("allow", Some(n), &cfg.network_allow.join(",")).await {
                    tracing::warn!("'{}': the network allowlist was not applied: {e:#}", scratch.sb);
                }
            }
            if !cfg.network_deny.is_empty() {
                if let Err(e) = sbx.policy("deny", Some(n), &cfg.network_deny.join(",")).await {
                    tracing::warn!("'{}': the network denylist was not applied: {e:#}", scratch.sb);
                }
            }
            Ok(())
        }
        .await;
        match up {
            Ok(()) => Ok(scratch),
            Err(e) => {
                self.scratch_down(&scratch).await;
                Err(e)
            }
        }
    }

    /// Remove a scratch sandbox and its workspace: it is nobody's to keep.
    async fn scratch_down(&self, scratch: &Scratch) {
        let n = &scratch.sb.name;
        if let Err(e) = scratch.sbx.rm(std::slice::from_ref(n)).await {
            tracing::warn!("could not remove '{}' ({e:#}) — `sbx rm {n}` it by hand", scratch.sb);
        }
        let _ = scratch.host.fs.remove(&scratch.workspace).await;
    }

    /// The sbx and sbxw versions a template is recorded against.
    pub fn template_versions(&self) -> (String, &'static str) {
        (self.versions.sbx.clone().unwrap_or_else(|| "unknown".into()), env!("CARGO_PKG_VERSION"))
    }

    /// Snapshot the sandbox just brought up as this kit list's automatic
    /// template, then remove the one it replaces.
    async fn save_auto_template(&self, name: &SandboxRef, kits: &[String], resolved: &[String], keep_running: bool, run: &mut Run<'_>) -> Result<()> {
        let (commands, marked) = self.startup_commands(resolved);
        let unseen: Vec<&String> = resolved.iter().filter(|k| !std::path::Path::new(k.as_str()).join("spec.yaml").is_file()).collect();
        if !unseen.is_empty() {
            run.warn(format!(
                "kits without a local spec are not waited for: {} — if their startup installs \
                 something, the template may not hold it",
                unseen.iter().map(|k| k.as_str()).collect::<Vec<_>>().join(", ")
            ));
        }
        self.wait_for_startups(name, &commands, &marked, run).await?;

        let (sbx, sbxw) = self.template_versions();
        let tag = templates::auto_tag(kits, sbxw, &sbx);
        run.info(format!("saving '{name}' as {tag}"));
        let (saved, warnings) = self.snapshot(name, &tag, keep_running).await;
        warnings.into_iter().for_each(|w| run.warn(w));
        saved?;

        let record = templates::Record {
            tag: tag.clone(),
            sbx,
            sbxw: sbxw.to_string(),
            kits: kits.to_vec(),
            created_at: unix_now(),
            content: templates::fingerprint(&spec_texts(resolved)),
            used_at: unix_now(),
        };
        let mut records = self.store.templates_of(&name.host);
        let old = templates::replace(&mut records, record);
        self.store.set_templates_of(&name.host, &records).context("recording the automatic template")?;
        run.info(format!("new sandboxes with these kits now start from {tag}"));
        if let Some(old) = old.filter(|o| o.tag != tag) {
            match self.sbx_for(name).await?.template_rm(&old.tag).await {
                Ok(()) => run.info(format!("removed the previous template {}", old.tag)),
                Err(e) => run.warn(format!("could not remove the previous template {} ({e:#}) — `sbx template rm` it by hand", old.tag)),
            }
        }
        // The templates of kit lists no sandbox was created with for a month:
        // each is a whole sandbox image, and a missing one is rebuilt in the
        // background the day it is wanted again.
        let (unused, changed) = templates::take_unused(&mut records, unix_now(), &tag);
        if changed {
            self.store.set_templates_of(&name.host, &records).context("recording the automatic templates")?;
        }
        for gone in unused {
            match self.sbx_for(name).await?.template_rm(&gone.tag).await {
                Ok(()) => run.info(format!("removed the template {}, unused for a month", gone.tag)),
                Err(e) => run.warn(format!("could not remove the unused template {} ({e:#}) — `sbx template rm` it by hand", gone.tag)),
            }
        }
        Ok(())
    }

    /// Save a running sandbox as template `tag` without its secrets: taken out,
    /// snapshot taken, put back whatever happened. A save that stopped the
    /// sandbox restarts it, unless `keep_running` is off. Warnings concern
    /// putting things back.
    pub async fn snapshot(&self, sb: &SandboxRef, tag: &str, keep_running: bool) -> (Result<()>, Vec<String>) {
        let mut warnings = Vec::new();
        let sbx = match self.sbx_for(sb).await {
            Ok(s) => s,
            Err(e) => return (Err(e), warnings),
        };
        let name = &sb.name;
        let mut kept: Vec<(&str, Vec<u8>)> = Vec::new();
        for path in templates::SECRETS {
            match sbx.exec(name, &["cat", path], None).await {
                Ok(out) if out.success() => kept.push((path, out.stdout)),
                Ok(_) => {}
                Err(e) => return (Err(e), warnings),
            }
        }
        let scrub: Vec<&str> = ["rm", "-f"].into_iter().chain(templates::SECRETS).collect();
        match sbx.exec(name, &scrub, None).await {
            Ok(out) if out.success() => {}
            Ok(out) => return (Err(anyhow!("could not take the sandbox's secrets out: {}", String::from_utf8_lossy(&out.stderr).trim())), warnings),
            Err(e) => return (Err(e), warnings),
        }

        let saved = sbx.template_save(name, tag).await;

        let stopped = matches!(saved, Ok(true)) || !sbx.is_running(name).await;
        if stopped {
            let back = sbx.run_detached(name).await.is_ok() && sbx.wait_until_running(name, Duration::from_secs(60)).await;
            if !back {
                warnings.push(format!("'{name}' did not come back after the snapshot — its token returns on `sbxw up {name}`"));
                return (saved.map(drop), warnings);
            }
        }
        for (path, data) in &kept {
            if let Err(e) = sbx.write_file(name, path, data, true).await {
                warnings.push(format!("could not put {path} back after the snapshot ({e:#}) — `sbxw up {name}` reinstalls it"));
            }
        }
        if stopped && !keep_running {
            // `run -d` holds it up with nobody attached; give it back its idle stop.
            let _ = sbx.stop(name).await;
        }
        (saved.map(drop), warnings)
    }

    /// The startup commands of the kits at `resolved` (local directories),
    /// and those of them that leave an end marker: the bundled kits', whose
    /// scripts this build wrote.
    fn startup_commands(&self, resolved: &[String]) -> (Vec<String>, Vec<String>) {
        let bundled = self.store.paths.kits();
        let (mut all, mut marked) = (Vec::new(), Vec::new());
        for kit in resolved {
            let Ok(spec) = std::fs::read_to_string(Path::new(kit).join("spec.yaml")) else { continue };
            let commands = templates::startup_commands(&spec);
            if Path::new(kit).starts_with(&bundled) {
                marked.extend(commands.iter().filter(|c| templates::marker_of(c).is_some()).cloned());
            }
            all.extend(commands);
        }
        (all, marked)
    }

    /// Until the kits' startup commands are over in the guest: they start in
    /// the background after `sbx create` returns, and a snapshot taken
    /// mid-install is broken.
    ///
    /// A command in `marked` is over when its end marker for this boot is
    /// there, which is exact: it cannot be mistaken for one that has not
    /// started yet. The others are judged on the process list, as none of
    /// them running, twice in a row, once they have had time to start. That
    /// judgement also stands in for a marker that never comes, after
    /// `MARKER_GRACE` of quiet.
    async fn wait_for_startups(&self, sb: &SandboxRef, commands: &[String], marked: &[String], run: &Run<'_>) -> Result<()> {
        if commands.is_empty() {
            return Ok(());
        }
        let sbx = self.sbx_for(sb).await?;
        let name = &sb.name;
        run.info("waiting for the kits' startup installs to finish before the snapshot");
        let started = tokio::time::Instant::now();
        let deadline = started + STARTUP_TIMEOUT;
        let mut quiet = 0;
        let mut quiet_since: Option<tokio::time::Instant> = None;
        let mut seen = false;
        let mut shown = String::new();
        loop {
            let out = sbx.exec(name, &["sh", "-c", templates::PROCESS_LIST], None).await?;
            if !out.success() {
                bail!("could not list the sandbox's processes: {}", String::from_utf8_lossy(&out.stderr).trim());
            }
            let ps = out.stdout_str();
            let busy = templates::still_running(&ps, commands);
            let finished = templates::finished(&ps, marked);
            let unmarked = marked.iter().filter(|c| !finished.contains(&c.as_str())).count();
            if marked.len() == commands.len() && unmarked == 0 {
                // Every command says so itself.
                return Ok(());
            }
            // Nothing running proves nothing until the commands have had time
            // to start: seen one, or a guest up for longer than that.
            let settled = seen
                || templates::uptime(&ps).is_some_and(|u| u >= STARTUP_SETTLE.as_secs_f64())
                || started.elapsed() >= STARTUP_SETTLE;
            if busy.is_empty() && settled {
                quiet += 1;
                let since = *quiet_since.get_or_insert_with(tokio::time::Instant::now);
                // Twice in a row: one command ending as the next begins is not done.
                if quiet >= 2 && unmarked == 0 {
                    return Ok(());
                }
                if quiet >= 2 && since.elapsed() >= MARKER_GRACE {
                    run.info(format!("{unmarked} startup command(s) left no end marker; judged on the process list"));
                    return Ok(());
                }
            } else if !busy.is_empty() {
                seen = true;
                quiet = 0;
                quiet_since = None;
                let now = format!("still installing: {}", busy.join(", "));
                if now != shown {
                    run.info(&now);
                    shown = now;
                }
            }
            if tokio::time::Instant::now() >= deadline {
                bail!("kit startup still running after {} min: {}", STARTUP_TIMEOUT.as_secs() / 60, busy.join(", "));
            }
            tokio::time::sleep(STARTUP_POLL).await;
        }
    }

    /// `sbx create` with the port mappings, retried without them when the
    /// all-or-nothing `-p` loses the sandbox over one busy host port.
    async fn create_with_port_fallback(&self, sb: &SandboxRef, opts: &CreateOpts, sink: &Sink, run: &mut Run<'_>) -> Result<()> {
        let sbx = self.sbx_for(sb).await?;
        let name = &sb.name;
        let s = sink.clone();
        let lines: Arc<dyn Fn(&str) + Send + Sync> = Arc::new(move |l: &str| {
            if let Some(p) = progress::detail(l) {
                s(p);
            }
        });
        // A host port already taken on this machine fails the whole create,
        // after sbx has built the kits and prepared the image: asked of the
        // OS first, that create is not attempted at all. A remote host's
        // ports cannot be probed from here, so it keeps the retry below.
        let busy = if sb.is_local() && !cfg!(test) { busy_host_ports(&opts.publish) } else { vec![] };
        if !busy.is_empty() {
            run.warn(format!(
                "host port already in use: {} — '{name}' is created without its port mappings; each is then \
                 published on its own once it runs, so a busy port costs that port, not the sandbox",
                busy.join(", ")
            ));
            let bare = CreateOpts { publish: vec![], ..opts.clone() };
            return sbx.create(name, &bare, Some(lines)).await;
        }
        let first = match sbx.create(name, opts, Some(lines.clone())).await {
            Ok(()) => return Ok(()),
            Err(e) if opts.publish.is_empty() => return Err(e),
            // An image sbx cannot find is not a port problem: without ports it
            // would fail the same way, under a message blaming the ports.
            Err(e) if format!("{e:#}").contains("pull failed") => {
                let from = opts.template.as_deref().map_or_else(|| "sbx's image".to_string(), |t| format!("template {t}"));
                return Err(e.context(format!("sbx could not find {from} to create '{name}' from")));
            }
            Err(e) => e,
        };
        if sbx.exists(name).await {
            run.warn(format!(
                "'{name}' was created but its ports could not be published at creation ({first:#}); \
                 they are retried one by one once it runs"
            ));
            return Ok(());
        }
        run.warn(format!(
            "creating '{name}' with its port mappings failed ({first:#}) — retrying without them; each \
             port is then published on its own, so a busy host port costs that port, not the sandbox"
        ));
        let bare = CreateOpts { publish: vec![], ..opts.clone() };
        sbx
            .create(name, &bare, Some(lines))
            .await
            .map_err(|e| anyhow!("{e:#}\n(create with port mappings had failed with: {first:#})"))
    }

    /// Check the person's own local kits before anything is created. Since sbx
    /// 0.51 `claude` is a kit spec v3 workload and a v1/v2 kit does not compose
    /// with it: refuse here, naming the kit, instead of after an image pull.
    /// (`sbx kit validate` reads only v1/v2, so it cannot check this.) Bundled
    /// kits ship v3; remote references are sbx's to resolve.
    fn validate_kits(&self, kits: &[String]) -> Result<()> {
        let broken: Vec<String> = kits
            .iter()
            .filter(|k| kits::bundled_name(k).is_none())
            .filter_map(|k| kits::incompatible_spec(std::path::Path::new(k.as_str())).map(|why| format!("{k}: {why}")))
            .collect();
        if !broken.is_empty() {
            bail!("kit check failed — nothing was created or changed:\n{}", broken.join("\n"));
        }
        Ok(())
    }

    /// Wait for `running`, then (re)publish every mapping: they do not survive
    /// a stop. A busy host port costs that port, reported, never the rest. A
    /// task of the daemon, so it outlives the request.
    fn republish_ports(self: &Arc<Self>, name: SandboxRef, specs: Vec<String>, up: bool) {
        if specs.is_empty() {
            return;
        }
        let app = self.clone();
        tokio::spawn(async move {
            let sbx = match app.sbx_for(&name).await {
                Ok(s) => s,
                Err(e) => return tracing::warn!("ports of '{name}' not published: {e:#}"),
            };
            if !up && !sbx.wait_until_running(&name.name, Duration::from_secs(60)).await {
                tracing::info!("'{name}' is not running; its ports are published on its next start");
                return;
            }
            for spec in &specs {
                match sbx.publish(&name.name, spec).await {
                    Ok(()) => {
                        tracing::info!("published {spec} for '{name}'");
                        app.forward_port(&name, spec).await;
                    }
                    Err(e) => tracing::warn!(
                        "could not publish {spec} for '{name}': {e:#}\nif that host port is taken, free \
                         it (or change host_port) and run `sbxw ports {name}`"
                    ),
                }
            }
        });
    }
}

impl App {
    /// The kits `refs` as `sbx` on `on_host` names them: unchanged here,
    /// mirrored for a remote host (once per content), refusing a kit that
    /// carries secrets to a host not trusted with them.
    async fn kits_on(&self, on_host: Option<&Remote>, refs: &[String], run: &Run<'_>) -> Result<(Vec<String>, bool)> {
        let Some(r) = on_host else { return Ok((refs.to_vec(), false)) };
        if refs.is_empty() {
            return Ok((vec![], false));
        }
        let root = r.root().await?;
        let m = crate::host::kits::mirror_via(&*r.host.fs, Some(&*r.host.exec), &self.store.paths.root, &root, refs, r.cfg.trust_secrets)
            .await
            .with_context(|| format!("copying kits to host '{}'", r.cfg.name))?;
        for c in &m.copied {
            run.info(format!("kit copied to host '{}': {c}", r.cfg.name));
        }
        Ok((m.refs, !m.copied.is_empty()))
    }

    /// Remove, in the background, the kits mirrored to `r`'s host that none
    /// of its sandboxes uses any more (see [`crate::host::kits::prune`]).
    /// What is in use is asked of the host's own sbx, sandbox by sandbox, so
    /// one made by another daemon counts too; a sandbox that cannot be
    /// asked, or names no kit at all, ends the attempt with nothing removed.
    fn prune_mirrors_later(self: &Arc<Self>, r: Arc<Remote>) {
        tokio::spawn(async move {
            let pruned: Result<Vec<String>> = async {
                let sbx = r.sbx().await?;
                let mut in_use = Vec::new();
                for sb in crate::svc::sandboxes::list_strict(&sbx).await? {
                    let name = sbxw_core::names::SandboxName::parse(&sb.name).map_err(|e| anyhow!("{}: {e}", sb.name))?;
                    let kits = sbx.inspect(&name).await?.kits;
                    if kits.is_empty() {
                        bail!("sbx names no kit for '{name}'");
                    }
                    in_use.extend(kits);
                }
                crate::host::kits::prune(&*r.host.fs, &r.root().await?, &in_use, MIRROR_UNUSED_FOR).await
            }
            .await;
            match pruned {
                Ok(removed) if removed.is_empty() => {}
                Ok(removed) => tracing::info!("host '{}': removed {} unused mirrored kit(s): {}", r.cfg.name, removed.len(), removed.join(", ")),
                Err(e) => tracing::info!("host '{}': mirrored kits left as they are: {e:#}", r.cfg.name),
            }
        });
    }

    /// For a remote sandbox, forward the host port of a just-published
    /// `ip:host_port:sandbox_port` here, through its host's link.
    pub(crate) async fn forward_port(&self, sb: &SandboxRef, spec: &str) {
        if sb.is_local() {
            return;
        }
        let Some(port) = spec.split(':').rev().nth(1).and_then(|p| p.parse::<u16>().ok()) else { return };
        let Ok(r) = self.remote(&sb.host).await else { return };
        match r.link.forward(port).await {
            Ok(local) => tracing::info!("'{sb}': port {port} on host '{}' → http://{}.{}.localhost:{local}", sb.host, sb.name, sb.host),
            Err(e) => tracing::warn!("'{sb}': could not forward port {port}: {e:#}"),
        }
    }
}

/// The `IP:HOST:SANDBOX` mappings whose host address something on this
/// machine already listens on. Only "address in use" counts: an address this
/// machine does not have yet (a loopback alias added later in the bring-up)
/// or a port this process may not bind says nothing about sbx's chances.
fn busy_host_ports(specs: &[String]) -> Vec<String> {
    specs
        .iter()
        .filter(|spec| {
            let mut parts = spec.rsplitn(3, ':').skip(1);
            let port = parts.next().and_then(|p| p.parse::<u16>().ok());
            let ip = parts.next().and_then(|ip| ip.parse::<std::net::IpAddr>().ok());
            match (ip, port) {
                (Some(ip), Some(port)) => {
                    matches!(std::net::TcpListener::bind((ip, port)), Err(e) if e.kind() == std::io::ErrorKind::AddrInUse)
                }
                _ => false,
            }
        })
        .cloned()
        .collect()
}

fn unix_now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

fn sorted(v: &[String]) -> Vec<String> {
    let mut v = v.to_vec();
    v.sort();
    v
}

async fn canon_string(host: &Host, p: &Path) -> String {
    host.fs.canonicalize(p).await.unwrap_or_else(|_| p.to_path_buf()).to_string_lossy().into_owned()
}

/// Whether `p` is a directory on `host`.
async fn is_dir_on(host: &Host, p: &std::path::Path) -> Result<bool> {
    Ok(matches!(host.fs.stat(p).await?, Some(m) if m.kind == crate::host::Kind::Dir))
}

/// `git clone <url> <dir>` on `host`, when `dir` is not there yet.
async fn clone_if_missing(host: &Host, url: &str, dir: &std::path::Path) -> Result<()> {
    if host.fs.lstat(dir).await?.is_some() {
        return Ok(());
    }
    if url.starts_with('-') {
        bail!("'{url}' is not a git URL");
    }
    let d = dir.to_string_lossy().into_owned();
    let out = host
        .exec
        .exec(sbxw_sbx::host::HostCmd::new("git", vec!["clone".into(), "--".into(), url.into(), d.clone()]))
        .await?;
    if !out.success() {
        bail!("git clone {url} {d} failed: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(())
}

/// A remote workspace's `sbxw.toml`, read on its host; the defaults when it
/// has none. Relative paths are the host's, against the workspace. Its
/// `source` stays unset: the file is not on this machine to re-read.
async fn remote_config(host: &Host, workspace: &std::path::Path) -> Result<Config> {
    Ok(read_remote_config(host, workspace).await?.unwrap_or_default())
}

/// As [`remote_config`], but `None` when the workspace has no `sbxw.toml`.
pub(crate) async fn read_remote_config(host: &Host, workspace: &std::path::Path) -> Result<Option<Config>> {
    let p = workspace.join("sbxw.toml");
    if host.fs.stat(&p).await?.is_none() {
        return Ok(None);
    }
    let raw = host.fs.read(&p, 1 << 20).await?;
    let mut cfg = Config::parse(&String::from_utf8_lossy(&raw)).with_context(|| format!("{} on host '{}'", p.display(), host.name))?;
    let rel = |v: String| if v.starts_with("./") || v.starts_with("../") { workspace.join(v).to_string_lossy().into_owned() } else { v };
    cfg.kits = std::mem::take(&mut cfg.kits).into_iter().map(rel).collect();
    cfg.env_files = std::mem::take(&mut cfg.env_files)
        .into_iter()
        .map(|f| if std::path::Path::new(&f).is_absolute() { f } else { workspace.join(f).to_string_lossy().into_owned() })
        .collect();
    Ok(Some(cfg))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{app_with, sbx_answer};
    use sbxw_sbx::runner::Output;

    fn with_port() -> Config {
        Config {
            ports: vec![sbxw_core::config::PortMap { alias: "myapp.local".into(), sandbox_port: 4200, host_port: 4200 }],
            ..Config::default()
        }
    }

    #[test]
    fn the_announced_plan_matches_the_steps_taken() {
        let ids = |existed, cfg: &Config| plan(existed, cfg, false).into_iter().map(|s| s.id).collect::<Vec<_>>();
        let bare = Config { network_allow: vec![], network_deny: vec![], kits: vec![], ..Config::default() };
        assert_eq!(ids(false, &bare), ["workspace", "create", "boot", "tooling", "ports"]);
        // As the bring-up sees them: sbxw's base kit first.
        let base_only = Config { kits: vec!["base".into()], ..bare.clone() };
        assert!(!ids(true, &base_only).contains(&"kits".to_string()), "the base kit is creation-only");
        let kits = Config { kits: vec!["base".into(), "k".into()], ..bare };
        assert!(!ids(false, &kits).contains(&"kits".to_string()));
        assert!(ids(true, &kits).contains(&"kits".to_string()));
        assert!(ids(false, &Config::default()).contains(&"policy".to_string()));
    }

    fn auto_request(dir: &std::path::Path, kits: Vec<String>, template: templates::Choice) -> BringUp {
        let ws = dir.join("myapp");
        std::fs::create_dir_all(&ws).unwrap();
        BringUp {
            name: SandboxRef::parse("myapp").unwrap(),
            workspace: ws,
            ro: vec![],
            rw: vec![],
            carve: vec![],
            config: Config { kits, template: "auto".into(), network_allow: vec![], network_deny: vec![], ..Config::default() },
            extra_ports: vec![],
            chat: false,
            api_key: None,
            template,
            remote_control: false,
            chrome: false,
            git: None,
            consent: Consent::Checked(vec![]),
        }
    }

    /// An sbx whose sandboxes exist and run once created (and are gone once
    /// removed), whose kit install is seen running `busy` times, and whose
    /// secrets read back as "secret".
    fn auto_sbx(busy: usize) -> impl Fn(&[String]) -> Output + Send + Sync + 'static {
        let created = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let polls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        move |a: &[String]| {
            let joined = a.join(" ");
            match a.first().map(String::as_str) {
                Some("ls") => {
                    let rows: Vec<(String, &str)> = created.lock().unwrap().iter().map(|n| (n.clone(), "running")).collect();
                    crate::testing::ls(&rows.iter().map(|(n, s)| (n.as_str(), *s)).collect::<Vec<_>>())
                }
                Some("create") => {
                    created.lock().unwrap().push(a[4].clone());
                    Output::ok("")
                }
                Some("rm") => {
                    created.lock().unwrap().retain(|n| !a.contains(n));
                    Output::ok("")
                }
                Some("exec") if joined.contains("/proc/") => {
                    if polls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) < busy {
                        Output::ok("/sbin/init \nbash /k/install.sh \n")
                    } else {
                        Output::ok("/sbin/init \n")
                    }
                }
                Some("exec") if joined.contains(" cat ") => Output::ok("secret"),
                Some("template") if joined.starts_with("template ls") => Output::ok(r#"[{"tag":"sbxw-auto:fresh"}]"#),
                _ => sbx_answer(a),
            }
        }
    }

    /// Until `done`, or two seconds: what a background build is given.
    async fn eventually(done: impl Fn() -> bool) -> bool {
        for _ in 0..200 {
            if done() {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        done()
    }

    fn kit_with_startup(dir: &std::path::Path) -> String {
        let kit = dir.join("kit");
        std::fs::create_dir_all(&kit).unwrap();
        std::fs::write(
            kit.join("spec.yaml"),
            "# syntax=docker/sandbox-kit:3\nschemaVersion: \"3\"\nkind: mixin\ncapabilities:\n  - type: com.docker.sandbox/lifecycle@1\n    config:\n      startup:\n        - command: [\"bash\", \"/k/install.sh\"]\n",
        )
        .unwrap();
        kit.to_string_lossy().into_owned()
    }

    #[tokio::test]
    async fn a_rebuild_waits_for_the_installs_snapshots_without_secrets_and_drops_the_old_template() {
        let (dir, app, fake) = app_with(auto_sbx(3));
        let kits = vec![kit_with_startup(dir.path())];
        let old = templates::Record { tag: "sbxw-auto:old".into(), sbx: "0.47.0".into(), sbxw: "2.1.2".into(), kits: kits::with_base(&kits), created_at: 1, content: String::new(), used_at: 0 };
        app.store.set_templates(&[old]).unwrap();

        let w = app.provision(auto_request(dir.path(), kits.clone(), templates::Choice::Rebuild), None).await.unwrap();
        assert!(w.is_empty(), "{w:?}");
        let log = fake.log();
        let at = |p: &str| log.iter().position(|c| c.starts_with(p)).unwrap_or_else(|| panic!("no `{p}` in {log:?}"));
        assert!(!log[at("create")].contains("--template"), "rebuilt from sbx's image");
        assert_eq!(log.iter().filter(|c| c.contains("/proc/")).count(), 5, "three busy polls, then two quiet ones");
        assert!(at("exec myapp -- rm -f /home/agent/.sbxw/token") < at("template save myapp sbxw-auto:"), "secrets out before the save");
        assert!(at("template save") < at("exec -i myapp -- sh -c"), "and back after it");
        assert!(at("template save") < at("template rm sbxw-auto:old"), "the old one goes once the new one exists");
        let records = app.store.templates();
        assert_eq!(records.len(), 1);
        assert_eq!((records[0].sbxw.as_str(), records[0].sbx.as_str()), (env!("CARGO_PKG_VERSION"), "0.51.0"));
        assert!(log.iter().any(|c| c == &format!("template save myapp {}", records[0].tag)));
    }

    #[tokio::test]
    async fn a_fresh_template_is_used_and_a_stale_one_is_rebuilt_in_the_background() {
        let (dir, app, fake) = app_with(auto_sbx(0));
        let kits = vec!["sbxw:codemap".to_string()];
        let applied = kits::with_base(&kits);
        let fresh = templates::Record { tag: "sbxw-auto:fresh".into(), sbx: "0.51.0".into(), sbxw: env!("CARGO_PKG_VERSION").into(), kits: applied.clone(), created_at: 1, content: crate::bundled::fingerprint(&app.store.paths, &applied).unwrap(), used_at: 0 };
        app.store.set_templates(std::slice::from_ref(&fresh)).unwrap();
        let seen = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let s2 = seen.clone();
        let sink: Sink = Arc::new(move |p| s2.lock().unwrap().push(serde_json::to_string(&p).unwrap()));
        app.provision(auto_request(dir.path(), kits.clone(), templates::Choice::Default), Some(sink)).await.unwrap();
        assert!(fake.log().iter().any(|c| c.starts_with("create") && c.contains("--template sbxw-auto:fresh")));
        assert!(seen.lock().unwrap().iter().any(|l| l.contains("creating from the automatic template sbxw-auto:fresh")), "the card says so");
        assert!(!fake.log().iter().any(|c| c.starts_with("template save")), "a fresh template is left alone");

        let (dir, app, fake) = app_with(auto_sbx(0));
        app.store.set_templates(&[templates::Record { tag: "sbxw-auto:old".into(), sbx: "0.47.0".into(), ..fresh }]).unwrap();
        let w = app.provision(auto_request(dir.path(), kits, templates::Choice::Default), None).await.unwrap();
        assert!(w.is_empty(), "nobody is warned or made to wait: {w:?}");
        let builder = templates::builder_name(&applied);
        assert!(eventually(|| app.store.templates().iter().any(|r| r.sbx == "0.51.0")).await, "{:?}", fake.log());
        assert!(eventually(|| fake.log().iter().any(|c| c.starts_with("rm") && c.contains(&builder))).await, "the builder is removed");
        let log = fake.log();
        let at = |p: &str| log.iter().position(|c| c.starts_with(p)).unwrap_or_else(|| panic!("no `{p}` in {log:?}"));
        assert!(!log[at("create claude")].contains("--template"), "the person's sandbox comes from sbx's image");
        let made = at(&format!("create claude {}", app.store.paths.root.join("template-build").join(&builder).display()));
        assert!(log[made].contains(&format!("--name {builder}")) && !log[made].contains("myapp"), "{}", log[made]);
        assert!(log.iter().any(|c| c == &format!("run -d --name {builder}")), "held up so its kits install");
        assert!(at(&format!("template save {builder} sbxw-auto:")) < at("template rm sbxw-auto:old"), "the old one goes once the new one exists");
        assert!(!log.iter().any(|c| c.starts_with("template save myapp")), "never from the sandbox someone works in");
        assert_eq!(app.store.templates().len(), 1);
        assert!(app.template_builds.lock().unwrap().is_empty(), "built: the next stale one may build again");
    }

    #[tokio::test]
    async fn saving_a_template_removes_the_ones_unused_for_a_month() {
        let (dir, app, fake) = app_with(auto_sbx(0));
        let rec = |tag: &str, kit: &str, used_at: u64| templates::Record { tag: tag.into(), sbx: "0.51.0".into(), sbxw: "1".into(), kits: vec![kit.into()], created_at: 1, content: "c".into(), used_at };
        app.store.set_templates(&[rec("sbxw-auto:lately", "a", unix_now() - 86_400), rec("sbxw-auto:forgotten", "b", unix_now() - 40 * 86_400), rec("sbxw-auto:before", "c", 0)]).unwrap();
        let w = app.provision(auto_request(dir.path(), vec!["sbxw:codemap".into()], templates::Choice::Rebuild), None).await.unwrap();
        assert!(w.is_empty(), "{w:?}");
        assert!(fake.log().iter().any(|c| c == "template rm sbxw-auto:forgotten"), "{:?}", fake.log());
        let kept: Vec<String> = app.store.templates().into_iter().map(|r| r.tag).collect();
        assert_eq!(kept.len(), 3, "{kept:?}");
        assert!(kept.contains(&"sbxw-auto:lately".to_string()) && kept.contains(&"sbxw-auto:before".to_string()), "{kept:?}");
        assert!(!fake.log().iter().any(|c| c == "template rm sbxw-auto:before"), "a record with no last use starts its clock");
    }

    #[tokio::test]
    async fn end_markers_of_this_boot_end_the_wait_at_the_first_look() {
        let listing = "5.0\n/sbin/init \n#sbxw-markers\nboot-1\n\
            _home_agent_.sbxw_codemap_install-memory.mjs.done boot-1\n\
            _home_agent_.sbxw_codemap_install-hooks.mjs.done boot-1\n";
        let inner = auto_sbx(0);
        let (dir, app, fake) = app_with(move |a: &[String]| match a.first().map(String::as_str) {
            Some("exec") if a.join(" ").contains("/proc/") => Output::ok(listing),
            _ => inner(a),
        });
        let w = app.provision(auto_request(dir.path(), vec!["sbxw:codemap".into()], templates::Choice::Rebuild), None).await.unwrap();
        assert!(w.is_empty(), "{w:?}");
        let log = fake.log();
        assert_eq!(log.iter().filter(|c| c.contains("/proc/")).count(), 1, "no second look, no settling: {log:?}");
        assert!(log.iter().any(|c| c.starts_with("template save myapp sbxw-auto:")));
    }

    #[tokio::test]
    async fn a_background_build_that_fails_is_not_started_again() {
        // The builder never shows as running; the sandboxes people create do.
        let made = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let (dir, app, fake) = app_with(move |a: &[String]| match a.first().map(String::as_str) {
            Some("ls") => {
                let made = made.lock().unwrap();
                crate::testing::ls(&made.iter().map(|n| (n.as_str(), "running")).collect::<Vec<_>>())
            }
            Some("create") => {
                if !a[4].starts_with("sbxw-tpl-") {
                    made.lock().unwrap().push(a[4].clone());
                }
                Output::ok("")
            }
            _ => sbx_answer(a),
        });
        let kits = vec!["sbxw:codemap".to_string()];
        let builder = templates::builder_name(&kits::with_base(&kits));
        for name in ["one", "two"] {
            let mut req = auto_request(dir.path(), kits.clone(), templates::Choice::Default);
            req.name = SandboxRef::parse(name).unwrap();
            app.provision(req, None).await.unwrap();
            assert!(eventually(|| fake.log().iter().any(|c| c.starts_with("rm") && c.contains(&builder))).await, "a failed builder is removed too");
        }
        let builds = fake.log().iter().filter(|c| c.starts_with("create") && c.contains(&builder)).count();
        assert_eq!(builds, 1, "{:?}", fake.log());
        assert!(app.store.templates().is_empty());
    }

    #[tokio::test]
    async fn a_kit_that_needs_a_recreate_is_refused_once_not_at_every_bring_up() {
        let (dir, app, fake) = app_with(|a: &[String]| match a.first().map(String::as_str) {
            Some("ls") => crate::testing::ls(&[("myapp", "running")]),
            Some("kit") => Output::fail(1, "kit declares credentials: recreate the sandbox to apply it"),
            _ => sbx_answer(a),
        });
        let ws = dir.path().join("myapp");
        std::fs::create_dir_all(&ws).unwrap();
        let kit = dir.path().join("kit");
        std::fs::create_dir_all(&kit).unwrap();
        let spec = "# syntax=docker/sandbox-kit:3\nschemaVersion: \"3\"\nkind: mixin\n";
        std::fs::write(kit.join("spec.yaml"), spec).unwrap();
        let myapp = SandboxRef::parse("myapp").unwrap();
        app.store.update(&myapp, |r| r.workspace = Some(ws.clone())).unwrap();
        let req = || {
            let mut r = auto_request(dir.path(), vec![kit.to_string_lossy().into_owned()], Default::default());
            r.config.template = String::new();
            r
        };
        let adds = || fake.log().iter().filter(|c| c.starts_with("kit add")).count();
        let w = app.provision(req(), None).await.unwrap();
        assert!(w.iter().any(|w| w.contains("cannot apply kit")), "{w:?}");
        assert_eq!(adds(), 1);
        let w = app.provision(req(), None).await.unwrap();
        assert!(w.iter().any(|w| w.contains("still waits for 'myapp' to be recreated")), "still said: {w:?}");
        assert_eq!(adds(), 1, "sbx is not asked the same question again");
        // An edited kit is another question.
        std::fs::write(kit.join("spec.yaml"), format!("{spec}description: changed\n")).unwrap();
        app.provision(req(), None).await.unwrap();
        assert_eq!(adds(), 2);
        assert_eq!(app.store.get(&myapp).kits_refused.len(), 1, "one refusal per kit, the current one");
    }

    #[test]
    fn a_host_port_someone_listens_on_is_found_before_the_create() {
        let held = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let taken = format!("127.0.0.1:{}:4200", held.local_addr().unwrap().port());
        let free = {
            let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            format!("127.0.0.1:{}:8000", l.local_addr().unwrap().port())
        };
        // An address this machine does not have is not "in use".
        let elsewhere = "203.0.113.7:4300:4300".to_string();
        assert_eq!(busy_host_ports(&[taken.clone(), free, elsewhere, "nonsense".into()]), [taken]);
    }

    #[tokio::test]
    async fn a_template_gone_from_sbx_is_forgotten_not_pulled() {
        let (dir, app, fake) = app_with(auto_sbx(0));
        let kits = vec!["sbxw:codemap".to_string()];
        let applied = kits::with_base(&kits);
        let content = crate::bundled::fingerprint(&app.store.paths, &applied).unwrap();
        let gone = templates::Record { tag: "sbxw-auto:gone".into(), sbx: "0.51.0".into(), sbxw: env!("CARGO_PKG_VERSION").into(), kits: applied, created_at: 1, content, used_at: 0 };
        app.store.set_templates(&[gone]).unwrap();
        let w = app.provision(auto_request(dir.path(), kits, templates::Choice::Default), None).await.unwrap();
        assert!(w.iter().any(|w| w.contains("sbxw-auto:gone is no longer in sbx")), "{w:?}");
        assert!(fake.log().iter().any(|c| c.starts_with("create") && !c.contains("--template")), "created from sbx's image");
        assert!(app.store.templates().is_empty(), "the record is dropped");
    }

    #[tokio::test]
    async fn an_existing_sandbox_brought_up_from_another_directory_is_refused_and_keeps_its_record() {
        let (dir, app, fake) = app_with(|a: &[String]| match a.first().map(String::as_str) {
            Some("ls") => crate::testing::ls(&[("myapp", "running")]),
            _ => sbx_answer(a),
        });
        let (own, other) = (dir.path().join("test"), dir.path().join("sbxw"));
        for d in [&own, &other] {
            std::fs::create_dir_all(d).unwrap();
        }
        let myapp = SandboxRef::parse("myapp").unwrap();
        app.store.update(&myapp, |r| r.workspace = Some(own.clone())).unwrap();
        let req = |ws: &std::path::Path| BringUp {
            name: myapp.clone(),
            workspace: ws.to_path_buf(),
            ro: vec![],
            rw: vec![],
            carve: vec![],
            config: Config { kits: vec![], ..Config::default() },
            extra_ports: vec![],
            chat: false,
            api_key: None,
            template: Default::default(),
            remote_control: false,
            chrome: false,
            git: None,
            consent: Consent::Checked(vec![]),
        };
        let e = format!("{:#}", app.provision(req(&other), None).await.unwrap_err());
        assert!(e.contains("was created on") && e.contains(&own.display().to_string()), "{e}");
        assert_eq!(app.store.workspace(&myapp), Some(own.clone()), "the record is left as it was");
        assert!(fake.log().iter().all(|c| !c.starts_with("kit add") && !c.starts_with("create")), "{:?}", fake.log());
        // From its own directory, written another way, it goes ahead.
        let same = own.join("..").join("test");
        let ok = app.provision(req(&same), None).await;
        assert!(ok.as_ref().err().is_none_or(|e| !format!("{e:#}").contains("was created on")), "{ok:?}");
    }

    /// `sbxw up` with a file that mounts a host folder: refused with the line,
    /// accepted with exactly that line, then not asked again. A different
    /// line approves nothing; removing the mount asks nothing.
    #[tokio::test]
    async fn a_new_mount_needs_the_persons_yes_once() {
        let (dir, app, fake) = app_with(sbx_answer);
        let ws = dir.path().join("myapp");
        std::fs::create_dir_all(&ws).unwrap();
        let file = ws.join("sbxw.toml");
        let write = |body: &str| std::fs::write(&file, body).unwrap();
        let req = |approved: Vec<String>| BringUp {
            name: SandboxRef::parse("myapp").unwrap(),
            workspace: ws.clone(),
            ro: vec![],
            rw: vec![],
            carve: vec![],
            config: Config::load(&file, &ws).unwrap(),
            extra_ports: vec![],
            chat: false,
            api_key: None,
            template: Default::default(),
            remote_control: false,
            chrome: false,
            git: None,
            consent: Consent::Check(approved),
        };
        write("workspaces = [\"/Users/me/data\"]\n");
        let line = "mount (read-write): /Users/me/data".to_string();
        let e = app.provision(req(vec![]), None).await.unwrap_err();
        let n = crate::store::needs_approval(&e).unwrap();
        assert_eq!((n.config.as_str(), n.grants.clone()), (crate::store::grant_key(&file).as_str(), vec![line.clone()]));
        assert!(!fake.log().iter().any(|c| c.starts_with("create")), "nothing ran before the yes");
        let e = app.provision(req(vec!["mount (read-write): /Users/me/other".into()]), None).await.unwrap_err();
        assert!(crate::store::needs_approval(&e).is_some());
        app.provision(req(vec![line.clone()]), None).await.unwrap();
        app.provision(req(vec![]), None).await.unwrap();
        // An edit after the yes is a new line, asked about.
        write("workspaces = [\"/Users/me/data\", \"/Users/me/.ssh\"]\n");
        let e = app.provision(req(vec![]), None).await.unwrap_err();
        assert_eq!(crate::store::needs_approval(&e).unwrap().grants, ["mount (read-write): /Users/me/.ssh"]);
        write("");
        app.provision(req(vec![]), None).await.unwrap();
        assert!(!app.store.paths.approved_grants().starts_with(&ws), "approvals live outside the workspace");
    }

    /// A strict carve of the workspace: sbx gets a stub as its workspace and
    /// the other folders by name, the agent starts in the real folder, a
    /// changed file is a recreate, and dropping the carve asks first.
    #[tokio::test]
    async fn a_strict_carve_splits_the_workspace() {
        let made = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let seen = made.clone();
        let _ = &made;
        let (dir, app, fake) = app_with(move |a: &[String]| match a.first().map(String::as_str) {
            Some("create") => {
                seen.store(true, std::sync::atomic::Ordering::SeqCst);
                sbx_answer(a)
            }
            Some("ls") if seen.load(std::sync::atomic::Ordering::SeqCst) => crate::testing::ls(&[("myapp", "running")]),
            Some("inspect") => Output::ok(r#"{"name":"myapp","kits":[],"created_version":"v0.51.0"}"#),
            _ => sbx_answer(a),
        });
        let ws = std::fs::canonicalize(dir.path()).unwrap().join("myapp");
        for d in ["a", "secret"] {
            std::fs::create_dir_all(ws.join(d)).unwrap();
        }
        std::fs::write(ws.join("README"), "x").unwrap();
        let file = ws.join("sbxw.toml");
        let write = |body: &str| std::fs::write(&file, body).unwrap();
        let req = |approved: Vec<String>| BringUp {
            name: SandboxRef::parse("myapp").unwrap(),
            workspace: ws.clone(),
            ro: vec![],
            rw: vec![],
            carve: vec![],
            config: Config::load(&file, &ws).unwrap(),
            extra_ports: vec![],
            chat: false,
            api_key: None,
            template: Default::default(),
            remote_control: false,
            chrome: false,
            git: None,
            consent: Consent::Check(approved),
        };
        write("workdir = \"web\"\n[[carve]]\npath = \"secret\"\naccess = \"none\"\n");
        let warnings = app.provision(req(vec![]), None).await.unwrap();
        assert!(warnings.iter().any(|w| w.contains("2 files")), "{warnings:?}");
        let create = fake.log().into_iter().find(|c| c.starts_with("create")).expect("created");
        let stub = app.store.paths.carve().join("myapp").join("stub");
        assert!(create.starts_with(&format!("create claude {} ", stub.display())), "{create}");
        assert!(create.contains(&ws.join("a").display().to_string()), "{create}");
        assert!(!create.contains("secret"), "the carved folder never reaches sbx: {create}");
        assert!(stub.is_dir());
        let rec = app.store.get(SandboxRef::parse("myapp").unwrap());
        assert_eq!(rec.attach_workdir("web"), format!("{}/web", ws.display()));
        // Changing the file's carves does not change the sandbox, and says so.
        write("workdir = \"web\"\n[[carve]]\npath = \"secret\"\naccess = \"none\"\n[[carve]]\npath = \"a\"\naccess = \"ro\"\n");
        let w = app.provision(req(vec![]), None).await.unwrap();
        assert!(w.iter().any(|w| w.contains("carves apply when a sandbox is created")), "{w:?}");
        // Dropping the carve gives access back: the person is asked.
        write("workdir = \"web\"\n");
        let e = app.provision(req(vec![]), None).await.unwrap_err();
        let lines = crate::store::needs_approval(&e).unwrap().grants.clone();
        assert_eq!(lines, [format!("access restored: {}/a (was: read-only, strict)", ws.display()), format!("access restored: {}/secret (was: no access, strict)", ws.display())]);
        app.provision(req(lines), None).await.unwrap();
    }

    #[tokio::test]
    async fn a_kit_waiting_for_approval_is_recorded_for_a_pane_to_answer() {
        let (dir, app, fake) = app_with(|a: &[String]| match a.first().map(String::as_str) {
            Some("ls") => crate::testing::ls(&[("myapp", "running")]),
            Some("kit") => Output::fail(1, "warning: this change requests permissions beyond what this sandbox was granted\n  + network.runtime.allow: dl.k8s.io\n\n  error: stdin is not a terminal; run this command in a terminal to answer the prompt"),
            Some("inspect") => Output::ok(r#"{"name":"myapp","kits":[],"created_version":"v0.51.0"}"#),
            _ => sbx_answer(a),
        });
        let ws = dir.path().join("myapp");
        std::fs::create_dir_all(&ws).unwrap();
        let myapp = SandboxRef::parse("myapp").unwrap();
        app.store.update(&myapp, |r| r.workspace = Some(ws.clone())).unwrap();
        let req = BringUp {
            name: myapp.clone(),
            workspace: ws,
            ro: vec![],
            rw: vec![],
            carve: vec![],
            config: Config { kits: vec!["sbxw:k8s-tools".into()], network_allow: vec![], network_deny: vec![], ..Config::default() },
            extra_ports: vec![],
            chat: false,
            api_key: None,
            template: Default::default(),
            remote_control: false,
            chrome: false,
            git: None,
            consent: Consent::Checked(vec![]),
        };
        let w = app.provision(req, None).await.unwrap();
        assert!(w.iter().any(|w| w.contains("press ✓")), "{w:?}");
        let waiting = app.kit_approvals.lock().unwrap().get(&myapp).cloned().unwrap_or_default();
        assert_eq!(waiting.len(), 1);
        assert!(waiting[0].ends_with("/kits/k8s-tools"), "{waiting:?}");
        assert_eq!(app.sandboxes().await.iter().find(|s| s.name == "myapp").unwrap().kits_waiting, 1);
        assert!(fake.log().iter().any(|c| c.starts_with("kit add myapp")));
    }

    #[tokio::test]
    async fn a_v2_local_kit_stops_the_bring_up_before_create() {
        let (dir, app, fake) = app_with(sbx_answer);
        let ws = dir.path().join("myapp");
        let (good, old) = (dir.path().join("good"), dir.path().join("old"));
        for d in [&ws, &good, &old] {
            std::fs::create_dir_all(d).unwrap();
        }
        std::fs::write(good.join("spec.yaml"), "# syntax=docker/sandbox-kit:3\nschemaVersion: \"3\"\nkind: mixin\n").unwrap();
        std::fs::write(old.join("spec.yaml"), "schemaVersion: \"2\"\nkind: mixin\nname: old\n").unwrap();
        let kits = vec![good.to_string_lossy().into_owned(), old.to_string_lossy().into_owned(), "sbxw:codemap".into(), "docker.io/o/k:1".into()];
        let req = BringUp {
            name: SandboxRef::parse("myapp").unwrap(),
            workspace: ws,
            ro: vec![],
            rw: vec![],
            carve: vec![],
            config: Config { kits, ..Config::default() },
            extra_ports: vec![],
            chat: false,
            api_key: None,
            template: Default::default(),
            remote_control: false,
            chrome: false,
            git: None,
            consent: Consent::Checked(vec![]),
        };
        let e = format!("{:#}", app.provision(req, None).await.unwrap_err());
        assert!(e.contains(&format!("{}: kit spec v2", old.display())), "{e}");
        assert!(!e.contains(&good.display().to_string()), "the v3 kit is not named: {e}");
        assert!(fake.log().iter().all(|c| !c.starts_with("create") && !c.starts_with("kit")), "nothing created: {:?}", fake.log());
    }

    #[tokio::test]
    async fn a_fresh_bring_up_creates_with_kits_ports_and_the_agent_port_rule() {
        let created = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let c = created.clone();
        let (dir, app, fake) = app_with(move |a: &[String]| {
            let joined = a.join(" ");
            match a.first().map(String::as_str) {
                Some("ls") if c.load(std::sync::atomic::Ordering::SeqCst) => {
                    crate::testing::ls(&[("myapp", "running")])
                }
                Some("ls") => crate::testing::ls(&[]),
                Some("create") => {
                    c.store(true, std::sync::atomic::Ordering::SeqCst);
                    Output::ok("pulling image\n")
                }
                Some("exec") if joined.contains("node -") => Output::ok("SBXW-TOOLING []\n"),
                _ => sbx_answer(a),
            }
        });
        let ws = dir.path().join("myapp");
        std::fs::create_dir_all(&ws).unwrap();
        let cfg = Config { kits: vec!["/kits/codemap".into()], ..with_port() };
        let seen = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let s2 = seen.clone();
        let sink: Sink = Arc::new(move |p| s2.lock().unwrap().push(serde_json::to_string(&p).unwrap()));
        let warnings = app
            .provision(
                BringUp {
                    name: SandboxRef::parse("myapp").unwrap(),
                    workspace: ws.clone(),
                    ro: vec![],
                    rw: vec![],
                    carve: vec![],
                    config: cfg,
                    extra_ports: vec![],
                    chat: false,
                    api_key: None,
                    template: Default::default(),
                    remote_control: false,
                    chrome: false,
                    git: None,
                    consent: Consent::Checked(vec![]),
                },
                Some(sink),
            )
            .await
            .unwrap();
        assert!(warnings.iter().all(|w| !w.contains("tooling")), "{warnings:?}");
        let log = fake.log();
        let create = log.iter().find(|c| c.starts_with("create")).unwrap();
        assert!(create.contains("--kit /kits/codemap") && !create.contains("-oauth"), "sbx signs Claude in itself: {create}");
        // `4200:4200`, with or without a host IP in front.
        assert!(create.contains("4200:4200 ") || create.ends_with("4200:4200"), "{create}");
        assert!(create.contains("-e ANTHROPIC_DEFAULT_MODEL=claude-sonnet-5-5"));
        let allow = log.iter().find(|c| c.starts_with("policy allow")).unwrap();
        assert!(allow.contains("localhost:7682") && !allow.contains("localhost:7681"), "{allow}");
        assert!(ws.join(".sbxw-artifacts/README.md").exists());
        let rec = app.store.get(sbxw_core::SandboxRef::parse("myapp").unwrap());
        assert!(rec.token.is_some() && rec.workspace.as_deref() == Some(ws.as_path()));
        // What later attaches run with, as the bring-up was given it.
        let snap = rec.config_snapshot.as_deref().expect("a snapshot of the config");
        assert_eq!(snap.kits, vec!["/kits/codemap".to_string()]);
        assert_eq!(snap.ports, with_port().ports);
        let events = seen.lock().unwrap().join("\n");
        assert!(events.contains("\"kind\":\"plan\"") && events.contains("pulling image") && events.contains("\"kind\":\"done\""));
        // The token went into the sandbox on stdin, never on a command line.
        let token = rec.token.unwrap();
        assert!(log.iter().all(|c| !c.contains(&token)));
        assert!(fake.stdins.lock().unwrap().iter().any(|s| String::from_utf8_lossy(s).contains(&token)));
    }

    #[tokio::test]
    async fn a_remote_control_sandbox_is_given_no_credentials_and_keeps_the_choice() {
        let created = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let c = created.clone();
        let (dir, app, fake) = app_with(move |a: &[String]| match a.first().map(String::as_str) {
            Some("ls") if c.load(std::sync::atomic::Ordering::SeqCst) => {
                crate::testing::ls(&[("myapp", "running")])
            }
            Some("ls") => crate::testing::ls(&[]),
            Some("create") => {
                c.store(true, std::sync::atomic::Ordering::SeqCst);
                Output::ok("")
            }
            Some("exec") if a.join(" ").contains("node -") => Output::ok("SBXW-TOOLING []\n"),
            _ => sbx_answer(a),
        });
        let ws = dir.path().join("myapp");
        std::fs::create_dir_all(&ws).unwrap();
        let mut cfg = Config::default();
        cfg.env.insert("CLAUDE_CODE_OAUTH_TOKEN".into(), "from-config".into());
        let req = BringUp {
            name: SandboxRef::parse("myapp").unwrap(),
            workspace: ws,
            ro: vec![],
            rw: vec![],
            carve: vec![],
            config: cfg,
            extra_ports: vec![],
            chat: false,
            api_key: Some("sk-key".into()),
            template: Default::default(),
            remote_control: true,
            chrome: false,
            git: None,
            consent: Consent::Checked(vec![]),
        };
        app.provision(req.clone(), None).await.unwrap();
        let log = fake.log();
        let create = log.iter().find(|c| c.starts_with("create")).unwrap();
        assert!(!create.contains("-oauth") && !create.contains("from-config"), "{create}");
        assert!(log.iter().all(|c| !c.starts_with("secret set")), "{log:?}");
        assert!(log.iter().all(|c| !c.contains(".credentials.json")), "{log:?}");
        let allow = log.iter().find(|c| c.starts_with("policy allow")).unwrap();
        assert!(allow.contains("claude.ai") && allow.contains("platform.claude.com"), "{allow}");
        assert!(app.store.get(sbxw_core::SandboxRef::parse("myapp").unwrap()).remote_control);

        // Brought up again without asking, it still gets no token.
        app.provision(BringUp { remote_control: false, ..req }, None).await.unwrap();
        assert!(fake.log().iter().all(|c| !c.contains(".credentials.json") && !c.contains("kit add")));
        assert!(app.store.get(sbxw_core::SandboxRef::parse("myapp").unwrap()).remote_control);
    }

    /// sbx 0.53 keeps Remote Control's credentials on the host: the sandbox
    /// is created like any other, and Chrome is said once, with what to run.
    #[tokio::test]
    async fn with_sbx_0_53_a_remote_control_sandbox_keeps_its_sign_in_and_chrome_is_recorded() {
        let created = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let c = created.clone();
        let (dir, app, fake) = app_with(move |a: &[String]| match a.first().map(String::as_str) {
            Some("ls") if c.load(std::sync::atomic::Ordering::SeqCst) => crate::testing::ls(&[("myapp", "running")]),
            Some("ls") => crate::testing::ls(&[]),
            Some("create") => {
                c.store(true, std::sync::atomic::Ordering::SeqCst);
                Output::ok("")
            }
            Some("settings") if a.last().map(String::as_str) == Some("claude.remoteControl") => Output::ok("true\n"),
            Some("settings") if a.last().map(String::as_str) == Some("claude.chrome") => Output::ok("false\n"),
            Some("exec") if a.join(" ").contains("node -") => Output::ok("SBXW-TOOLING []\n"),
            _ => sbx_answer(a),
        });
        let ws = dir.path().join("myapp");
        std::fs::create_dir_all(&ws).unwrap();
        let req = BringUp {
            name: SandboxRef::parse("myapp").unwrap(),
            workspace: ws,
            ro: vec![],
            rw: vec![],
            carve: vec![],
            config: Config::default(),
            extra_ports: vec![],
            chat: false,
            api_key: None,
            template: Default::default(),
            remote_control: true,
            chrome: true,
            git: None,
            consent: Consent::Checked(vec![]),
        };
        let warnings = app.provision(req, None).await.unwrap();
        let rec = app.store.get(sbxw_core::SandboxRef::parse("myapp").unwrap());
        assert!(!rec.remote_control, "not a sandbox without credentials");
        assert!(rec.chrome, "asked for once, kept");
        let log = fake.log();
        assert!(log.iter().all(|c| !(c.starts_with("policy allow") && c.contains("platform.claude.com"))), "no /login inside: {log:?}");
        assert!(warnings.iter().any(|w| w.contains("sbx settings set claude.chrome true")), "{warnings:?}");
        assert!(warnings.iter().all(|w| !w.contains("claude.remoteControl")), "it is on: {warnings:?}");
    }

    #[tokio::test]
    async fn a_busy_port_costs_the_port_not_the_sandbox() {
        let made = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let m = made.clone();
        let (dir, app, fake) = app_with(move |a: &[String]| match a.first().map(String::as_str) {
            Some("create") if a.contains(&"-p".to_string()) => Output::fail(1, "409 port 4200 is already allocated"),
            Some("create") => {
                m.store(true, std::sync::atomic::Ordering::SeqCst);
                Output::ok("")
            }
            Some("ls") if m.load(std::sync::atomic::Ordering::SeqCst) => {
                crate::testing::ls(&[("w", "running")])
            }
            Some("ls") => crate::testing::ls(&[]),
            _ => sbx_answer(a),
        });
        let ws = dir.path().join("w");
        std::fs::create_dir_all(&ws).unwrap();
        let w = app
            .provision(
                BringUp {
                    name: SandboxRef::parse("w").unwrap(),
                    workspace: ws,
                    ro: vec![],
                    rw: vec![],
                    carve: vec![],
                    config: with_port(),
                    extra_ports: vec![],
                    chat: false,
                    api_key: None,
                            template: Default::default(),
                    remote_control: false,
                    chrome: false,
                    git: None,
                    consent: Consent::Checked(vec![]),
                },
                None,
            )
            .await
            .unwrap();
        assert!(w.iter().any(|w| w.contains("retrying without them")));
        assert_eq!(fake.log().iter().filter(|c| c.starts_with("create claude") && c.contains("--name w ")).count(), 2);
    }

    #[tokio::test]
    async fn the_daemon_fills_the_cache_from_its_own_sandbox_and_everyone_reads_it() {
        let made = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let m = made.clone();
        let (dir, app, fake) = app_with(move |a: &[String]| match a.first().map(String::as_str) {
            Some("create") => {
                let i = a.iter().position(|x| x == "--name").unwrap();
                m.lock().unwrap().push(a[i + 1].clone());
                Output::ok("")
            }
            Some("ls") => {
                let names = m.lock().unwrap().clone();
                crate::testing::ls(&names.iter().map(|n| (n.as_str(), "running")).collect::<Vec<_>>())
            }
            _ => sbx_answer(a),
        });
        let up = |name: &str| {
            let ws = dir.path().join(name);
            std::fs::create_dir_all(&ws).unwrap();
            BringUp {
                name: SandboxRef::parse(name).unwrap(),
                workspace: ws,
                ro: vec![],
                rw: vec![],
                carve: vec![],
                config: Config::default(),
                extra_ports: vec![],
                chat: false,
                api_key: None,
                    template: Default::default(),
                remote_control: false,
                chrome: false,
                git: None,
                consent: Consent::Checked(vec![]),
            }
        };
        app.provision(up("a"), None).await.unwrap();
        app.provision(up("b"), None).await.unwrap();
        let paths = &app.store.paths;
        let cache = paths.cache().to_string_lossy().into_owned();
        let log = fake.log();
        let create = |n: &str| log.iter().find(|c| c.starts_with("create") && c.contains(&format!("--name {n} "))).unwrap().clone();
        let (a, b) = (create("a"), create("b"));
        for (name, made) in [("a", &a), ("b", &b)] {
            assert!(made.contains(&format!("{cache}:ro")) && made.contains(&format!("-e SBXW_CACHE={cache}")) && !made.contains("FILL"), "{name} only reads: {made}");
            assert!(!made.contains(&format!(" {cache} ")), "never writable where someone works: {made}");
        }
        // One filler, the daemon's own: the cache writable, the kits that fill
        // it and sbxw's base kit, an empty workspace, and gone when done.
        assert!(eventually(|| fake.log().iter().any(|c| c.starts_with("rm") && c.contains(cache::FILLER))).await, "{:?}", fake.log());
        let log = fake.log();
        let fills: Vec<&String> = log.iter().filter(|c| c.starts_with("create") && c.contains(&format!("--name {} ", cache::FILLER))).collect();
        assert_eq!(fills.len(), 1, "the second sandbox finds a fill under way: {log:?}");
        let f = fills[0];
        assert!(f.contains(&format!(" {cache} ")) && f.contains("SBXW_CACHE_FILL=1") && !f.contains(&format!("{cache}:ro")), "the filler writes: {f}");
        assert!(f.starts_with(&format!("create claude {}", paths.root.join("cache-fill").join(cache::FILLER).display())), "{f}");
        for k in ["base", "agent-skills", "md-to-pdf-tools"] {
            assert!(f.contains(&format!("--kit {}", paths.kits().join(k).display())), "{k}: {f}");
        }
        for k in ["codemap", "retro"] {
            assert!(!f.contains(&format!("--kit {}", paths.kits().join(k).display())), "{k} fills nothing: {f}");
        }
        assert!(log.iter().any(|c| c == &format!("run -d --name {}", cache::FILLER)), "held up so its kits install");
        for k in ["codemap", "agent-skills", "md-to-pdf-tools", "retro"] {
            assert!(b.contains(&format!("--kit {}", paths.kits().join(k).display())), "{k}");
        }
        assert!(!b.contains("sbxw:"));
    }

    #[tokio::test]
    async fn volumes_are_a_kit_given_at_creation() {
        let (dir, app, fake) = app_with(sbx_answer);
        let ws = dir.path().join("myapp");
        std::fs::create_dir_all(&ws).unwrap();
        let req = BringUp {
            name: SandboxRef::parse("myapp").unwrap(),
            workspace: ws,
            ro: vec![],
            rw: vec![],
            carve: vec![],
            config: Config { kits: vec![], volumes: vec!["/home/agent/.npm".into()], ..Config::default() },
            extra_ports: vec![],
            chat: false,
            api_key: None,
            template: Default::default(),
            remote_control: false,
            chrome: false,
            git: None,
            consent: Consent::Checked(vec![]),
        };
        app.provision(req, None).await.unwrap();
        let kit = app.store.paths.volumes_kit(sbxw_core::SandboxRef::parse("myapp").unwrap());
        let log = fake.log();
        let create = log.iter().find(|c| c.starts_with("create")).unwrap();
        assert!(create.contains(&format!("--kit {}", kit.display())), "{create}");
        assert!(std::fs::read_to_string(kit.join("spec.yaml")).unwrap().contains("path: '/home/agent/.npm'"));
    }

    #[tokio::test]
    async fn a_kit_folder_picked_in_the_dialog_is_a_mount_at_creation() {
        use crate::svc::sandboxes::CreateForm;
        let (dir, app, fake) = app_with(sbx_answer);
        let ws = dir.path().join("ws");
        let kit = dir.path().join("hist-kit");
        std::fs::create_dir_all(&ws).unwrap();
        std::fs::create_dir_all(&kit).unwrap();
        std::fs::write(
            kit.join("spec.yaml"),
            "schemaVersion: \"3\"\nkind: mixin\ncapabilities:\n  - type: com.docker.sandbox/host-mount@1\n    config: {path: /home/agent/history}\n",
        )
        .unwrap();
        let mut app = Arc::try_unwrap(app).ok().unwrap();
        app.cfg.kits = vec![kit.to_string_lossy().into_owned()];
        // The daemon's kits as read at start (see `App::daemon_kits`).
        *app.daemon_kits.get_mut().unwrap() = app.cfg.kits.clone();
        let app = Arc::new(app);
        let offered = app.declared_host_mounts();
        assert_eq!(offered.len(), 1);
        assert_eq!(offered[0]["target"], "/home/agent/history");

        // The daemon config's local kit is a grant, asked about once; the
        // folder picked in the dialog is the person's own choice, not asked.
        let picked = || [("/home/agent/history".to_string(), "/h/hist".to_string())].into();
        let form = CreateForm { kit_mounts: picked(), ..Default::default() };
        let e = app.create(SandboxRef::parse("km").unwrap(), &ws.to_string_lossy(), form, None).await.unwrap_err();
        let asked = crate::store::needs_approval(&e).unwrap().grants.clone();
        assert_eq!(asked, [format!("local kit: {}", kit.display())]);
        let form = CreateForm { kit_mounts: picked(), approved: asked, ..Default::default() };
        app.create(SandboxRef::parse("km").unwrap(), &ws.to_string_lossy(), form, None).await.unwrap();
        let log = fake.log();
        let create = log.iter().find(|c| c.starts_with("create")).unwrap();
        assert!(create.contains("--mount /h/hist:/home/agent/history"), "{create}");
    }

    #[tokio::test]
    async fn the_dialogs_other_repos_are_mounted_beside_the_workspace() {
        use crate::svc::sandboxes::CreateForm;
        let (dir, app, fake) = app_with(sbx_answer);
        let [ws, docs, lib] = ["ws", "docs", "lib"].map(|n| {
            let p = dir.path().join(n);
            std::fs::create_dir_all(&p).unwrap();
            p.canonicalize().unwrap().to_string_lossy().into_owned()
        });
        let name = SandboxRef::parse("multi").unwrap();
        let mounts = vec![(docs.clone(), true), (lib.clone(), false)];
        app.create(name.clone(), &ws, CreateForm { mounts, ..Default::default() }, None).await.unwrap();
        let log = fake.log();
        let create = log.iter().find(|c| c.starts_with("create")).unwrap();
        assert!(create.starts_with(&format!("create claude {ws} --name multi {lib} ")), "{create}");
        assert!(create.contains(&format!(" {docs}:ro")), "{create}");
        let mounts = app.store.get(sbxw_core::SandboxRef::parse("multi").unwrap()).mounts;
        assert_eq!(mounts.iter().map(|m| (m.path.to_string_lossy().into_owned(), m.readonly)).collect::<Vec<_>>(), [(docs.clone(), true), (lib.clone(), false)]);
        let twice = vec![(ws.clone(), false)];
        let e = app.create(name.clone(), &ws, CreateForm { mounts: twice, ..Default::default() }, None).await.unwrap_err();
        assert!(e.to_string().contains("already mounted"), "{e}");
        // A repo inside the workspace is reached through the workspace's own
        // mount, writable: its `:ro` would be a lie, so it is refused.
        let inner = format!("{ws}/frontend");
        std::fs::create_dir_all(&inner).unwrap();
        let e = app.create(name.clone(), &ws, CreateForm { mounts: vec![(inner, true)], ..Default::default() }, None).await.unwrap_err();
        assert!(e.to_string().contains("nested"), "{e}");
        let outer = dir.path().canonicalize().unwrap().to_string_lossy().into_owned();
        let e = app.create(name, &ws, CreateForm { mounts: vec![(outer, true)], ..Default::default() }, None).await.unwrap_err();
        assert!(e.to_string().contains("nested"), "{e}");
    }
}
