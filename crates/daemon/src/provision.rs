//! Bringing a sandbox up: one pipeline for every way in — `sbxw up`, the
//! create and chat dialogs, Duplicate, the island's ephemeral chat.
//!
//! The steps are announced before the first one runs (a slow image pull must
//! not look like a wedged bring-up), each is reported as it starts, and
//! `sbx`'s own output streams underneath as detail lines. The sink is an
//! argument, not a thread-local: whoever asked decides where it goes — every
//! browser tab, the terminal that typed `sbxw up`, or nowhere.
//!
//! What must hold, in order, and why:
//! 1. creation carries every kit (`--kit`, credentials first) and the ports
//!    (`-p`), because creation is the only moment kits apply whole; a busy
//!    host port 409s the create, so it is retried without them;
//! 2. the in-sandbox tooling — token, trust, hooks, statusLine, relay — is one
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
use crate::host::Host;
use crate::remotes::Remote;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

/// One bring-up, as asked.
#[derive(Clone, Debug)]
pub struct BringUp {
    pub name: SandboxRef,
    pub workspace: PathBuf,
    pub ro: Vec<String>,
    /// Extra directories mounted writable beside the workspace.
    pub rw: Vec<String>,
    /// The project config — remembered for the sandbox, so its later attaches
    /// use the same `env`.
    pub config: Config,
    pub extra_ports: Vec<WantedPort>,
    pub chat: bool,
    /// Store `ANTHROPIC_API_KEY` as the global `anthropic` secret.
    pub api_key: Option<String>,
    /// What to do about an automatic template (`template = "auto"`).
    pub template: templates::Choice,
    /// Create it for Remote Control: no credentials from sbxw, a claude.ai
    /// login inside instead (`sbxw_core::remote`). Only a new sandbox takes
    /// it; one already recorded with it keeps it.
    pub remote_control: bool,
    /// Clone this into `workspace` first, on the sandbox's host, when
    /// `workspace` does not exist yet (`sbxw up --git <url>`).
    pub git: Option<String>,
}

/// How long a kit's startup commands may run before an automatic template is
/// given up on, how often the guest is looked at, and how long it is left to
/// start them at all.
const STARTUP_TIMEOUT: Duration = if cfg!(test) { Duration::from_millis(300) } else { Duration::from_secs(20 * 60) };
const STARTUP_POLL: Duration = if cfg!(test) { Duration::from_millis(10) } else { Duration::from_secs(2) };
const STARTUP_SETTLE: Duration = if cfg!(test) { Duration::ZERO } else { Duration::from_secs(15) };

/// Each resolved kit's `spec.yaml`, else its reference: what a template's
/// content is judged on.
fn spec_texts(resolved: &[String]) -> Vec<String> {
    resolved.iter().map(|k| kits::content_text(k)).collect()
}

/// The steps this bring-up will take, announced first. Three are conditional
/// in exactly the way the body is.
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

    async fn bring_up(self: &Arc<Self>, mut req: BringUp, sink: &Sink) -> Result<Vec<String>> {
        // A remote host: its link first. The -R tunnel the hooks call home
        // through must be up before they are installed.
        let on_host: Option<Arc<Remote>> = match req.name.is_local() {
            true => None,
            false => Some(self.ready_remote(&req.name).await?),
        };
        let host = self.host_for(&req.name).await?;
        let sbx = self.sbx_for(&req.name).await?;
        if let Some(url) = req.git.clone() {
            clone_if_missing(&host, &url, &req.workspace).await?;
        }
        if on_host.is_some() {
            // sbxw.toml is the project's, so it is on the host, beside the code.
            req.config = remote_config(&host, &req.workspace).await?;
        }
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
        self.validate_kits(&req.config.kits).await?;

        // Asked first: it decides two of the announced steps.
        let initial = sbx.lookup(n).await;
        let existed = initial.is_some();
        // An existing sandbox keeps the workspace it was created on: sbx
        // cannot move its mount. A bring-up from another directory used to
        // record that directory anyway — and the Project panel, the env and
        // the config then all pointed at the wrong project. Refuse it here,
        // before anything is changed.
        if existed {
            if let Some(recorded) = self.store.get(name).workspace.clone() {
                let canon = |p: PathBuf| async { host.fs.canonicalize(&p).await.unwrap_or(p) };
                if canon(recorded.clone()).await != canon(req.workspace.clone()).await {
                    bail!(
                        "'{name}' was created on {}, not {workspace} — run `sbxw up {name}` from {0}, or pass it: `sbxw up {name} {0}`. A sandbox cannot change workspace; for another one, use another name",
                        recorded.display()
                    );
                }
            }
        }
        let stopped = initial.as_ref().is_some_and(|i| i.is_stopped());
        let mut running = Some(initial.as_ref().is_some_and(|i| i.is_running()));
        let (template, rebuild) = self.template_for(&req, cfg, existed, &mut run).await;
        // A template holds the kits' installs: its kits find everything in
        // place and would never fill the cache this sandbox claimed.
        wants_cache &= template.is_none();
        sink(Progress::Plan { steps: plan(existed, cfg, rebuild) });

        // ── Workspace ────────────────────────────────────────────────────
        run.step(progress::WORKSPACE);
        // The credentials kit would come back on every container swap, so
        // only creation can leave it out.
        let recorded_remote = self.store.get(name).remote_control;
        if existed && req.remote_control && !recorded_remote {
            run.warn(format!(
                "'{name}' was created without Remote Control — press Remote in its pane to switch it to a claude.ai login"
            ));
        }
        let remote = recorded_remote || (!existed && req.remote_control);
        self.store
            .update(name, |r| {
                r.remote_control = remote;
                r.workspace = Some(req.workspace.clone());
                r.config = cfg.source.clone();
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

        // ── Create, or reuse ─────────────────────────────────────────────
        run.step(progress::CREATE);
        let mut created_with_kits = false;
        // The project's kits as `sbx` on the sandbox's host names them:
        // mirrored there for a remote host, as they are here.
        let kits_here = self.kits_on(on_host.as_deref(), &cfg.kits, &mut run).await?;
        if existed {
            run.info(format!("sandbox '{name}' already exists — reusing it"));
            // Block storage exists only from creation: a list that is not the
            // one this sandbox was created with is a recreate, never applied.
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
            // The skills the person kept from a sandbox: every new one gets
            // them, whatever its `kits` say — they are the person's, not the project's.
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
            kit_refs.extend(self.kits_on(on_host.as_deref(), &own, &mut run).await?);
            let mut ro_mounts = req.ro.clone();
            let mut env = if remote { remote::without_auth_env(cfg.env_pairs()) } else { cfg.env_pairs() };
            let mut rw_mounts = req.rw.clone();
            if wants_cache {
                // Writable only for the sandbox that fills it: see `cache`.
                let (dir, mount) = match &on_host {
                    None => cache::claim(&self.store.paths, cfg.cache_ttl())?,
                    Some(r) => cache::claim_on(&*host.fs, &r.root().await?, cfg.cache_ttl()).await?,
                };
                let dir = dir.to_string_lossy().into_owned();
                env.push(format!("SBXW_CACHE={dir}"));
                if mount == cache::Mount::Fill {
                    run.info("this sandbox fills the shared kit cache — the next ones install from it");
                    env.push("SBXW_CACHE_FILL=1".into());
                    rw_mounts.push(dir);
                } else {
                    ro_mounts.push(dir);
                }
            }
            let opts = CreateOpts {
                workspace: workspace.clone(),
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
            };
            let applied = opts.kits.clone();
            self.create_with_port_fallback(name, &opts, sink, &mut run).await?;
            created_with_kits = true;
            // Created by this sbxw: its kits are known, and the old UI-port
            // rule cannot be there.
            let _ = self.store.update(name, |r| {
                r.kits = applied;
                r.ui_rule_checked = true;
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
        // Independent of each other — one is a script in the guest, the other
        // a host-side rule — so they run side by side and are reported in order.
        // The daemon's agent port, whatever the project file says (the hooks
        // must reach the process that listens); on a remote host, the far
        // end of the link's -R tunnel.
        let agent_port = self.agent_port_for(name).await?;
        // The proxy sees host.docker.internal as localhost:<port>.
        own_allow.push(format!("localhost:{agent_port}"));
        let allow = [own_allow.as_slice(), cfg.network_allow.as_slice()].concat().join(",");
        let migrate = up && !cfg.claude_model.is_empty() && !self.store.get(name).model_migrated;
        let mut batch = Tooling::new()
            .agent_token(&token)
            .trust_workspace(&workspace)
            .artifact_hook()
            .status_hooks(&agent_port)
            .usage_statusline(&agent_port)
            .relay(&agent_port)
            .lead_command();
        if migrate {
            batch = batch.drop_stale_model(&cfg.claude_model);
        }
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
                    if migrate && outcome.error(tooling::piece::STALE_MODEL).is_none() {
                        let _ = self.store.update(name, |r| r.model_migrated = true);
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
        if !self.store.get(name).ui_rule_checked {
            self.revoke_ui_port(name, &mut run).await;
            let _ = self.store.update(name, |r| r.ui_rule_checked = true);
        }

        // ── Kits, for a sandbox that already existed ─────────────────────
        // The first kit is sbxw's base kit, applied at creation only.
        if !created_with_kits && cfg.kits.len() > 1 {
            run.step(progress::KITS);
            let recorded = self.store.get(name).kits;
            let pending: Vec<&String> = kits_here.iter().skip(1).filter(|k| !recorded.contains(k)).collect();
            // `sbx inspect` only when sbxw's own record does not settle it.
            let present = match pending.is_empty() {
                true => String::new(),
                false => kits::inspect_kits_section(&sbx.inspect(n).await.unwrap_or_default()),
            };
            for ((kit, local), wanted) in kits_here.iter().zip(&cfg.kits).zip(&req.config.kits) {
                // sbxw's base kit is a creation-time kit: a sandbox made before
                // it existed has the Node.js of the image it came from, and one
                // that lacks it is told so by the tooling step below.
                if kits::bundled_name(wanted) == Some(kits::BASE) {
                    continue;
                }
                let spec = std::fs::read_to_string(std::path::Path::new(local).join("spec.yaml")).ok();
                if kits::already_applied(kit, spec.as_deref(), &present, &recorded) {
                    run.info(format!("kit '{kit}' already applied — skipping"));
                    if !recorded.contains(kit) {
                        let k = kit.clone();
                        let _ = self.store.update(name, |r| r.kits.push(k));
                    }
                    continue;
                }
                // sbx 0.48+ takes kits with startup commands here too.
                run.info(format!("applying kit {kit} (this recreates the container; state is kept)"));
                match self.sbx_for(name).await?.kit_add(n, kit).await {
                    Ok(()) => {
                        let k = kit.clone();
                        let _ = self.store.update(name, |r| r.kits.push(k));
                    }
                    // sbx asks a person to approve the network rules the kit
                    // adds, and has no flag to: hand them the command.
                    Err(e) if kits::add_needs_approval(&format!("{e:#}")) => run.warn(format!(
                        "kit '{kit}' asks for network access '{name}' was not granted, and sbx wants you to approve it. \
                         Run in a terminal: `sbx kit add {n} {kit}` — the next `sbxw up` sees it applied.\n{e:#}"
                    )),
                    Err(e) if kits::add_needs_recreate(&format!("{e:#}")) => run.warn(format!(
                        "`sbx kit add` cannot apply kit '{kit}' to an existing sandbox — '{name}' is \
                         unchanged. Recreate it to pick the kit up: `sbxw rm {name}` then `sbxw up {name}`. \
                         Anything outside the workspace mount is lost in the process.\n{e:#}"
                    )),
                    Err(e) => run.warn(format!("kit '{kit}' failed to apply: {e:#}")),
                }
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
        Ok(run.warnings)
    }

    /// The template `sbx create` starts from, and whether this bring-up saves
    /// a new automatic one. Only a sandbox being created is concerned.
    async fn template_for(&self, req: &BringUp, cfg: &Config, existed: bool, run: &mut Run<'_>) -> (Option<String>, bool) {
        if existed {
            if req.template == templates::Choice::Rebuild {
                run.warn(format!(
                    "'{}' already exists — a template is only rebuilt from a sandbox being created, \
                     before anyone has worked in it. Pick a new name to rebuild it",
                    req.name
                ));
            }
            return (None, false);
        }
        match req.config.template_mode() {
            templates::Mode::Off => (None, false),
            templates::Mode::Tag(t) => {
                run.info(format!("creating from template {t}"));
                (Some(t), false)
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
                    (_, Choice::Skip) => (None, false),
                    (_, Choice::Rebuild) => (None, true),
                    (Freshness::Fresh { record }, _) => {
                        run.info(format!("creating from the automatic template {} (sbx {}, sbxw {})", record.tag, record.sbx, record.sbxw));
                        (Some(record.tag), false)
                    }
                    (Freshness::Stale { record, .. }, Choice::Keep) => {
                        run.info(format!("creating from the out-of-date automatic template {}, as asked", record.tag));
                        (Some(record.tag), false)
                    }
                    (Freshness::Stale { changed, .. }, Choice::Default) => {
                        run.warn(format!(
                            "the automatic template is out of date ({}) — creating from sbx's image. \
                             Rebuild it from the web UI's create dialog, or with `sbxw up --rebuild-template`",
                            changed.join(", ")
                        ));
                        (None, false)
                    }
                    (Freshness::Missing, _) => {
                        run.info("no automatic template for these kits yet — creating from sbx's image");
                        (None, false)
                    }
                }
            }
        }
    }

    /// Whether sbx still has template `tag`. A record outlives a template
    /// removed with `sbx template rm` or a Docker prune, and `create
    /// --template` with a tag sbx lacks goes looking for it in a registry and
    /// fails there (403), which no retry mends. A listing sbx refuses proves
    /// nothing, so it counts as present.
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

    /// The sbx and sbxw versions a template is recorded against.
    pub fn template_versions(&self) -> (String, &'static str) {
        (self.versions.sbx.clone().unwrap_or_else(|| "unknown".into()), env!("CARGO_PKG_VERSION"))
    }

    /// Snapshot the sandbox just brought up as this kit list's automatic
    /// template, then remove the one it replaces.
    async fn save_auto_template(&self, name: &SandboxRef, kits: &[String], resolved: &[String], keep_running: bool, run: &mut Run<'_>) -> Result<()> {
        let commands: Vec<String> = resolved
            .iter()
            .filter_map(|k| std::fs::read_to_string(std::path::Path::new(k).join("spec.yaml")).ok())
            .flat_map(|spec| templates::startup_commands(&spec))
            .collect();
        let unseen: Vec<&String> = resolved.iter().filter(|k| !std::path::Path::new(k.as_str()).join("spec.yaml").is_file()).collect();
        if !unseen.is_empty() {
            run.warn(format!(
                "kits without a local spec are not waited for: {} — if their startup installs \
                 something, the template may not hold it",
                unseen.iter().map(|k| k.as_str()).collect::<Vec<_>>().join(", ")
            ));
        }
        self.wait_for_startups(name, &commands, run).await?;

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
            created_at: std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0),
            content: templates::fingerprint(&spec_texts(resolved)),
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
        Ok(())
    }

    /// Save a running sandbox as template `tag` without its secrets: they are
    /// taken out, the snapshot taken, and they are put back whatever it did.
    /// A save that had to stop the sandbox leaves it running again, unless
    /// `keep_running` is off. The warnings are about putting things back.
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

    /// Until none of the kits' startup commands is running in the guest:
    /// they run in the background after `sbx create` returns, and a snapshot
    /// taken mid-install is a broken template.
    async fn wait_for_startups(&self, sb: &SandboxRef, commands: &[String], run: &mut Run<'_>) -> Result<()> {
        if commands.is_empty() {
            return Ok(());
        }
        let sbx = self.sbx_for(sb).await?;
        let name = &sb.name;
        run.info("waiting for the kits' startup installs to finish before the snapshot");
        let started = tokio::time::Instant::now();
        let deadline = started + STARTUP_TIMEOUT;
        let mut quiet = 0;
        let mut seen = false;
        let mut shown = String::new();
        loop {
            let out = sbx.exec(name, &["sh", "-c", templates::PROCESS_LIST], None).await?;
            if !out.success() {
                bail!("could not list the sandbox's processes: {}", String::from_utf8_lossy(&out.stderr).trim());
            }
            let ps = out.stdout_str();
            let busy = templates::still_running(&ps, commands);
            // Nothing running proves nothing until the commands have had time
            // to start: seen one, or a guest up for longer than that.
            let settled = seen
                || templates::uptime(&ps).is_some_and(|u| u >= STARTUP_SETTLE.as_secs_f64())
                || started.elapsed() >= STARTUP_SETTLE;
            if busy.is_empty() && settled {
                quiet += 1;
                // Twice in a row: one command ending as the next begins is not done.
                if quiet >= 2 {
                    return Ok(());
                }
            } else if !busy.is_empty() {
                seen = true;
                quiet = 0;
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
    /// 0.51 `claude` is a kit spec v3 workload, and a v1/v2 kit does not
    /// compose with it: refused here in a second, naming the kit, rather than
    /// after an image pull. (`sbx kit validate` reads only v1/v2 specs, so it
    /// has nothing left to check.) Bundled kits ship v3; remote references
    /// are sbx's to resolve.
    async fn validate_kits(&self, kits: &[String]) -> Result<()> {
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

    /// An older sbxw gave sandboxes `localhost:<ui port>`, through which they
    /// could reach the browser's endpoints. Remove that rule if it is there.
    async fn revoke_ui_port(&self, sb: &SandboxRef, run: &mut Run<'_>) {
        let ui = format!("localhost:{}", self.cfg.web_port());
        let Ok(sbx) = self.sbx_for(sb).await else { return };
        let name = &sb.name;
        let view = sbx.policy_ls(name, true).await;
        let found = sbxw_core::policy::rule_ids_for(&view, &ui, name);
        for id in found.mine {
            match sbx.policy_rm(&id, Some(name)).await {
                Ok(()) => run.info(format!("revoked {ui} (the UI port) from '{name}' — sandboxes reach the agent port only")),
                Err(e) => run.warn(format!("could not revoke the stale {ui} rule {id}: {e:#}")),
            }
        }
        for id in found.unsure {
            run.warn(format!(
                "a rule allows {ui} (the UI port) and the listing does not say whose it is; if it is \
                 '{name}'s, remove it: `sbx policy rm network --id {id} --sandbox {name} --force`"
            ));
        }
    }

    /// Wait for `running`, then (re)publish every mapping: they do not survive
    /// a stop. A busy host port costs that port, reported, never the rest. A
    /// task of the daemon, so it outlives the request that started it.
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
    async fn kits_on(&self, on_host: Option<&Remote>, refs: &[String], run: &mut Run<'_>) -> Result<Vec<String>> {
        let Some(r) = on_host else { return Ok(refs.to_vec()) };
        if refs.is_empty() {
            return Ok(vec![]);
        }
        let root = r.root().await?;
        let m = crate::host::kits::mirror_via(&*r.host.fs, Some(&*r.host.exec), &self.store.paths.root, &root, refs, r.cfg.trust_secrets)
            .await
            .with_context(|| format!("copying kits to host '{}'", r.cfg.name))?;
        for c in &m.copied {
            run.info(format!("kit copied to host '{}': {c}", r.cfg.name));
        }
        Ok(m.refs)
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
            ports: vec![sbxw_core::config::PortMap { alias: "neos.local".into(), sandbox_port: 4200, host_port: 4200 }],
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
        let kits = Config { kits: vec!["base".into(), "k".into()], ..bare.clone() };
        assert!(!ids(false, &kits).contains(&"kits".to_string()));
        assert!(ids(true, &kits).contains(&"kits".to_string()));
        assert!(ids(false, &Config::default()).contains(&"policy".to_string()));
    }

    fn auto_request(dir: &std::path::Path, kits: Vec<String>, template: templates::Choice) -> BringUp {
        let ws = dir.join("neos");
        std::fs::create_dir_all(&ws).unwrap();
        BringUp {
            name: SandboxRef::parse("neos").unwrap(),
            workspace: ws,
            ro: vec![],
            rw: vec![],
            config: Config { kits, template: "auto".into(), network_allow: vec![], network_deny: vec![], ..Config::default() },
            extra_ports: vec![],
            chat: false,
            api_key: None,
            template,
            remote_control: false,
            git: None,
        }
    }

    /// An sbx whose sandbox exists once created, whose kit install is seen
    /// running `busy` times, and whose secrets read back as "secret".
    fn auto_sbx(busy: usize) -> impl Fn(&[String]) -> Output + Send + Sync + 'static {
        let created = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let polls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        move |a: &[String]| {
            let joined = a.join(" ");
            match a.first().map(String::as_str) {
                Some("ls") if created.load(std::sync::atomic::Ordering::SeqCst) => {
                    Output::ok(r#"[{"name":"neos","agent":"claude","status":"running"}]"#)
                }
                Some("create") => {
                    created.store(true, std::sync::atomic::Ordering::SeqCst);
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
        let old = templates::Record { tag: "sbxw-auto:old".into(), sbx: "0.47.0".into(), sbxw: "2.1.2".into(), kits: kits::with_base(&kits), created_at: 1, content: String::new() };
        app.store.set_templates(&[old]).unwrap();

        let w = app.provision(auto_request(dir.path(), kits.clone(), templates::Choice::Rebuild), None).await.unwrap();
        assert!(w.is_empty(), "{w:?}");
        let log = fake.log();
        let at = |p: &str| log.iter().position(|c| c.starts_with(p)).unwrap_or_else(|| panic!("no `{p}` in {log:?}"));
        assert!(!log[at("create")].contains("--template"), "rebuilt from sbx's image");
        assert_eq!(log.iter().filter(|c| c.contains("/proc/")).count(), 5, "three busy polls, then two quiet ones");
        assert!(at("exec neos -- rm -f /home/agent/.sbxw/token") < at("template save neos sbxw-auto:"), "secrets out before the save");
        assert!(at("template save") < at("exec -i neos -- sh -c"), "and back after it");
        assert!(at("template save") < at("template rm sbxw-auto:old"), "the old one goes once the new one exists");
        let records = app.store.templates();
        assert_eq!(records.len(), 1);
        assert_eq!((records[0].sbxw.as_str(), records[0].sbx.as_str()), (env!("CARGO_PKG_VERSION"), "0.51.0"));
        assert!(log.iter().any(|c| c == &format!("template save neos {}", records[0].tag)));
    }

    #[tokio::test]
    async fn a_fresh_template_is_used_and_a_stale_one_only_warns() {
        let (dir, app, fake) = app_with(auto_sbx(0));
        let kits = vec!["sbxw:codemap".to_string()];
        let applied = kits::with_base(&kits);
        let fresh = templates::Record { tag: "sbxw-auto:fresh".into(), sbx: "0.51.0".into(), sbxw: env!("CARGO_PKG_VERSION").into(), kits: applied.clone(), created_at: 1, content: crate::bundled::fingerprint(&app.store.paths, &applied).unwrap() };
        app.store.set_templates(std::slice::from_ref(&fresh)).unwrap();
        let seen = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let s2 = seen.clone();
        let sink: Sink = Arc::new(move |p| s2.lock().unwrap().push(serde_json::to_string(&p).unwrap()));
        app.provision(auto_request(dir.path(), kits.clone(), templates::Choice::Default), Some(sink)).await.unwrap();
        assert!(fake.log().iter().any(|c| c.starts_with("create") && c.contains("--template sbxw-auto:fresh")));
        assert!(seen.lock().unwrap().iter().any(|l| l.contains("creating from the automatic template sbxw-auto:fresh")), "the card says so");

        let (dir, app, fake) = app_with(auto_sbx(0));
        app.store.set_templates(&[templates::Record { sbx: "0.47.0".into(), ..fresh }]).unwrap();
        let w = app.provision(auto_request(dir.path(), kits, templates::Choice::Default), None).await.unwrap();
        assert!(w.iter().any(|w| w.contains("out of date (sbx 0.47.0 → 0.51.0)")), "{w:?}");
        let log = fake.log();
        assert!(log.iter().any(|c| c.starts_with("create") && !c.contains("--template")));
        assert!(!log.iter().any(|c| c.starts_with("template")), "nothing saved or removed unasked");
    }

    #[tokio::test]
    async fn a_template_gone_from_sbx_is_forgotten_not_pulled() {
        let (dir, app, fake) = app_with(auto_sbx(0));
        let kits = vec!["sbxw:codemap".to_string()];
        let applied = kits::with_base(&kits);
        let content = crate::bundled::fingerprint(&app.store.paths, &applied).unwrap();
        let gone = templates::Record { tag: "sbxw-auto:gone".into(), sbx: "0.51.0".into(), sbxw: env!("CARGO_PKG_VERSION").into(), kits: applied, created_at: 1, content };
        app.store.set_templates(&[gone]).unwrap();
        let w = app.provision(auto_request(dir.path(), kits, templates::Choice::Default), None).await.unwrap();
        assert!(w.iter().any(|w| w.contains("sbxw-auto:gone is no longer in sbx")), "{w:?}");
        assert!(fake.log().iter().any(|c| c.starts_with("create") && !c.contains("--template")), "created from sbx's image");
        assert!(app.store.templates().is_empty(), "the record is dropped");
    }

    #[tokio::test]
    async fn an_existing_sandbox_brought_up_from_another_directory_is_refused_and_keeps_its_record() {
        let (dir, app, fake) = app_with(|a: &[String]| match a.first().map(String::as_str) {
            Some("ls") => crate::testing::ls(&[("neos", "running")]),
            _ => sbx_answer(a),
        });
        let (own, other) = (dir.path().join("test"), dir.path().join("sbxw"));
        for d in [&own, &other] {
            std::fs::create_dir_all(d).unwrap();
        }
        let neos = SandboxRef::parse("neos").unwrap();
        app.store.update(&neos, |r| r.workspace = Some(own.clone())).unwrap();
        let req = |ws: &std::path::Path| BringUp {
            name: neos.clone(),
            workspace: ws.to_path_buf(),
            ro: vec![],
            rw: vec![],
            config: Config { kits: vec![], ..Config::default() },
            extra_ports: vec![],
            chat: false,
            api_key: None,
            template: Default::default(),
            remote_control: false,
            git: None,
        };
        let e = format!("{:#}", app.provision(req(&other), None).await.unwrap_err());
        assert!(e.contains("was created on") && e.contains(&own.display().to_string()), "{e}");
        assert_eq!(app.store.workspace(&neos), Some(own.clone()), "the record is left as it was");
        assert!(fake.log().iter().all(|c| !c.starts_with("kit add") && !c.starts_with("create")), "{:?}", fake.log());
        // From its own directory, written another way, it goes ahead.
        let same = own.join("..").join("test");
        let ok = app.provision(req(&same), None).await;
        assert!(ok.as_ref().err().is_none_or(|e| !format!("{e:#}").contains("was created on")), "{ok:?}");
    }

    #[tokio::test]
    async fn a_v2_local_kit_stops_the_bring_up_before_create() {
        let (dir, app, fake) = app_with(sbx_answer);
        let ws = dir.path().join("neos");
        let (good, old) = (dir.path().join("good"), dir.path().join("old"));
        for d in [&ws, &good, &old] {
            std::fs::create_dir_all(d).unwrap();
        }
        std::fs::write(good.join("spec.yaml"), "# syntax=docker/sandbox-kit:3\nschemaVersion: \"3\"\nkind: mixin\n").unwrap();
        std::fs::write(old.join("spec.yaml"), "schemaVersion: \"2\"\nkind: mixin\nname: old\n").unwrap();
        let kits = vec![good.to_string_lossy().into_owned(), old.to_string_lossy().into_owned(), "sbxw:codemap".into(), "docker.io/o/k:1".into()];
        let req = BringUp {
            name: SandboxRef::parse("neos").unwrap(),
            workspace: ws,
            ro: vec![],
            rw: vec![],
            config: Config { kits, ..Config::default() },
            extra_ports: vec![],
            chat: false,
            api_key: None,
            template: Default::default(),
            remote_control: false,
            git: None,
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
                    crate::testing::ls(&[("neos", "running")])
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
        let ws = dir.path().join("neos");
        std::fs::create_dir_all(&ws).unwrap();
        let cfg = Config { kits: vec!["/kits/codemap".into()], ..with_port() };
        let seen = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let s2 = seen.clone();
        let sink: Sink = Arc::new(move |p| s2.lock().unwrap().push(serde_json::to_string(&p).unwrap()));
        let warnings = app
            .provision(
                BringUp {
                    name: SandboxRef::parse("neos").unwrap(),
                    workspace: ws.clone(),
                    ro: vec![],
                    rw: vec![],
                    config: cfg,
                    extra_ports: vec![],
                    chat: false,
                    api_key: None,
                    template: Default::default(),
                    remote_control: false,
                    git: None,
                },
                Some(sink),
            )
            .await
            .unwrap();
        assert!(warnings.iter().all(|w| !w.contains("tooling")), "{warnings:?}");
        let log = fake.log();
        let create = log.iter().find(|c| c.starts_with("create")).unwrap();
        assert!(create.contains("--kit /kits/codemap") && !create.contains("-oauth"), "sbx signs Claude in itself: {create}");
        assert!(create.contains("-p 4200:4200"));
        assert!(create.contains("-e ANTHROPIC_DEFAULT_MODEL=claude-sonnet-5-5"));
        let allow = log.iter().find(|c| c.starts_with("policy allow")).unwrap();
        assert!(allow.contains("localhost:7682") && !allow.contains("localhost:7681"), "{allow}");
        assert!(ws.join(".sbxw-artifacts/README.md").exists());
        let rec = app.store.get(sbxw_core::SandboxRef::parse("neos").unwrap());
        assert!(rec.token.is_some() && rec.workspace.as_deref() == Some(ws.as_path()));
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
                crate::testing::ls(&[("neos", "running")])
            }
            Some("ls") => crate::testing::ls(&[]),
            Some("create") => {
                c.store(true, std::sync::atomic::Ordering::SeqCst);
                Output::ok("")
            }
            Some("exec") if a.join(" ").contains("node -") => Output::ok("SBXW-TOOLING []\n"),
            _ => sbx_answer(a),
        });
        let ws = dir.path().join("neos");
        std::fs::create_dir_all(&ws).unwrap();
        let mut cfg = Config::default();
        cfg.env.insert("CLAUDE_CODE_OAUTH_TOKEN".into(), "from-config".into());
        let req = BringUp {
            name: SandboxRef::parse("neos").unwrap(),
            workspace: ws,
            ro: vec![],
            rw: vec![],
            config: cfg,
            extra_ports: vec![],
            chat: false,
            api_key: Some("sk-key".into()),
            template: Default::default(),
            remote_control: true,
            git: None,
        };
        app.provision(req.clone(), None).await.unwrap();
        let log = fake.log();
        let create = log.iter().find(|c| c.starts_with("create")).unwrap();
        assert!(!create.contains("-oauth") && !create.contains("from-config"), "{create}");
        assert!(log.iter().all(|c| !c.starts_with("secret set")), "{log:?}");
        assert!(log.iter().all(|c| !c.contains(".credentials.json")), "{log:?}");
        let allow = log.iter().find(|c| c.starts_with("policy allow")).unwrap();
        assert!(allow.contains("claude.ai") && allow.contains("platform.claude.com"), "{allow}");
        assert!(app.store.get(sbxw_core::SandboxRef::parse("neos").unwrap()).remote_control);

        // Brought up again without asking, it still gets no token.
        app.provision(BringUp { remote_control: false, ..req }, None).await.unwrap();
        assert!(fake.log().iter().all(|c| !c.contains(".credentials.json") && !c.contains("kit add")));
        assert!(app.store.get(sbxw_core::SandboxRef::parse("neos").unwrap()).remote_control);
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
                    config: with_port(),
                    extra_ports: vec![],
                    chat: false,
                    api_key: None,
                            template: Default::default(),
                    remote_control: false,
                    git: None,
                },
                None,
            )
            .await
            .unwrap();
        assert!(w.iter().any(|w| w.contains("retrying without them")));
        assert_eq!(fake.log().iter().filter(|c| c.starts_with("create claude") && c.contains("--name w ")).count(), 2);
    }

    #[tokio::test]
    async fn the_first_sandbox_fills_the_cache_the_next_read_it() {
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
                config: Config::default(),
                extra_ports: vec![],
                chat: false,
                api_key: None,
                    template: Default::default(),
                remote_control: false,
                git: None,
            }
        };
        app.provision(up("a"), None).await.unwrap();
        app.provision(up("b"), None).await.unwrap();
        let paths = &app.store.paths;
        let cache = paths.cache().to_string_lossy().into_owned();
        let log = fake.log();
        let create = |n: &str| log.iter().find(|c| c.starts_with("create") && c.contains(&format!("--name {n} "))).unwrap().clone();
        let (a, b) = (create("a"), create("b"));
        assert!(a.contains(&format!(" {cache} ")) && a.contains("SBXW_CACHE_FILL=1"), "the filler writes: {a}");
        assert!(b.contains(&format!("{cache}:ro")) && b.contains(&format!("-e SBXW_CACHE={cache}")) && !b.contains("FILL"), "{b}");
        for k in ["codemap", "agent-skills", "md-to-pdf-tools", "retro"] {
            assert!(b.contains(&format!("--kit {}", paths.kits().join(k).display())), "{k}");
        }
        assert!(!b.contains("sbxw:"));
    }

    #[tokio::test]
    async fn volumes_are_a_kit_given_at_creation() {
        let (dir, app, fake) = app_with(sbx_answer);
        let ws = dir.path().join("neos");
        std::fs::create_dir_all(&ws).unwrap();
        let req = BringUp {
            name: SandboxRef::parse("neos").unwrap(),
            workspace: ws,
            ro: vec![],
            rw: vec![],
            config: Config { kits: vec![], volumes: vec!["/home/agent/.npm".into()], ..Config::default() },
            extra_ports: vec![],
            chat: false,
            api_key: None,
            template: Default::default(),
            remote_control: false,
            git: None,
        };
        app.provision(req, None).await.unwrap();
        let kit = app.store.paths.volumes_kit(sbxw_core::SandboxRef::parse("neos").unwrap());
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
        let app = Arc::new(app);
        let offered = app.declared_host_mounts();
        assert_eq!(offered.len(), 1);
        assert_eq!(offered[0]["target"], "/home/agent/history");

        let form = CreateForm { kit_mounts: [("/home/agent/history".to_string(), "/h/hist".to_string())].into(), ..Default::default() };
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
