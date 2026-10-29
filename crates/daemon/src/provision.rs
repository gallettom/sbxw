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
use sbxw_core::SandboxName;
use sbxw_sbx::CreateOpts;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

/// One bring-up, as asked.
#[derive(Clone, Debug)]
pub struct BringUp {
    pub name: SandboxName,
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
    /// `CLAUDE_CODE_OAUTH_TOKEN`, from whoever asked (the CLI's environment,
    /// not the daemon's, which may be older).
    pub oauth_token: Option<String>,
    /// What to do about an automatic template (`template = "auto"`).
    pub template: templates::Choice,
    /// Create it for Remote Control: no credentials from sbxw, a claude.ai
    /// login inside instead (`sbxw_core::remote`). Only a new sandbox takes
    /// it; one already recorded with it keeps it.
    pub remote_control: bool,
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
    resolved
        .iter()
        .map(|k| std::fs::read_to_string(std::path::Path::new(k).join("spec.yaml")).unwrap_or_else(|_| k.clone()))
        .collect()
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
    if existed && !cfg.kits.is_empty() {
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
        let name = req.name.to_string();
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

    async fn bring_up(self: &Arc<Self>, req: BringUp, sink: &Sink) -> Result<Vec<String>> {
        let name = &req.name;
        let mut wants_cache = req.config.cache && cache::wanted_by(&req.config.kits);
        // `sbxw:<name>` kits become the stable directories they are written to.
        let cfg = &Config {
            kits: bundled::resolve(&self.store.paths, &req.config.kits)?,
            ..req.config.clone()
        };
        let mut run = Run { sink, warnings: vec![] };
        let workspace = req.workspace.to_string_lossy().into_owned();
        if !req.workspace.is_dir() {
            bail!("{workspace} is not a directory");
        }
        self.validate_kits(&req.config.kits).await?;

        // Asked first: it decides two of the announced steps.
        let initial = self.sbx.lookup(name).await;
        let existed = initial.is_some();
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
        if !artifacts_dir.exists() {
            if std::fs::create_dir_all(&artifacts_dir).is_ok() {
                let _ = std::fs::write(artifacts_dir.join("README.md"), artifacts::README);
            } else {
                run.warn(format!("could not create {}", artifacts_dir.display()));
            }
        }
        let creds = req
            .oauth_token
            .as_deref()
            .filter(|_| !remote)
            .map(|t| kits::oauth_credentials_json(t, &cfg.claude_subscription));
        let wanted = cfg.wanted_ports(&req.extra_ports);
        let specs = ports::publish_specs(&wanted, cfg.ip_per_app);
        let mut own_allow: Vec<String> = Vec::new();
        if remote {
            run.info("Remote Control: no credentials injected — log in with /login in the agent's pane");
            own_allow.extend(remote::LOGIN_HOSTS.iter().map(|h| h.to_string()));
        }

        // ── Create, or reuse ─────────────────────────────────────────────
        run.step(progress::CREATE);
        let mut created_with_kits = false;
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
            if let Some(creds) = &creds {
                if running == Some(true) {
                    // `kit add` would recreate the container under live sessions.
                    match self
                        .sbx
                        .write_file(name, "/home/agent/.claude/.credentials.json", creds.as_bytes(), true)
                        .await
                    {
                        Ok(()) => own_allow.push("claude.ai".into()),
                        Err(e) => run.warn(format!("OAuth credential refresh failed (use /login in-session instead): {e:#}")),
                    }
                } else {
                    let dir = self.store.write_oauth_kit(name, Some(creds))?;
                    running = None;
                    if let Err(e) = self.kit_add_repairing(name, &dir.to_string_lossy(), Some(creds)).await {
                        // sbx refuses `kit add` for a kit with `setup.files`
                        // (the OAuth kit) — after starting the sandbox. Started,
                        // it takes the file the way a running one does above.
                        let wrote = self.sbx.wait_until_running(name, Duration::from_secs(30)).await
                            && self
                                .sbx
                                .write_file(name, "/home/agent/.claude/.credentials.json", creds.as_bytes(), true)
                                .await
                                .is_ok();
                        if wrote {
                            own_allow.push("claude.ai".into());
                        } else {
                            run.warn(format!("OAuth kit add failed (use /login in-session instead): {e:#}"));
                        }
                    }
                }
            }
        } else {
            run.info(format!("creating sandbox '{name}' on workspace {workspace}"));
            let mut kit_refs: Vec<String> = Vec::new();
            if let Some(creds) = &creds {
                kit_refs.push(self.store.write_oauth_kit(name, Some(creds))?.to_string_lossy().into_owned());
            }
            for k in &cfg.kits {
                if kits::needs_allowlist(k) {
                    run.warn(format!(
                        "kit '{k}' is a Git URL or non-Docker-Hub registry — allow it with \
                         `sbx settings set kit.allowedSources <prefix>`"
                    ));
                }
            }
            kit_refs.extend(cfg.kits.iter().cloned());
            // The skills the person kept from a sandbox: every new one gets
            // them, whatever its `kits` say — they are the person's, not the project's.
            match self.my_skills_kit() {
                Ok(Some(dir)) => kit_refs.push(dir.to_string_lossy().into_owned()),
                Ok(None) => {}
                Err(e) => run.warn(format!("your kept skills are left out: {e:#}")),
            }
            if !cfg.volumes.is_empty() {
                let dir = self.store.write_volumes_kit(name, &cfg.volumes)?;
                run.info(format!("persistent volumes: {} (kept if the sandbox is removed)", cfg.volumes.join(", ")));
                kit_refs.push(dir.to_string_lossy().into_owned());
            }
            let mut ro_mounts = req.ro.clone();
            let mut env = if remote { remote::without_auth_env(cfg.env_pairs()) } else { cfg.env_pairs() };
            let mut rw_mounts = req.rw.clone();
            if wants_cache {
                // Writable only for the sandbox that fills it: see `cache`.
                let (dir, mount) = cache::claim(&self.store.paths, cfg.cache_ttl())?;
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
                None => self.sbx.is_running(name).await,
            };
            if !existed || !up {
                match self.sbx.run_detached(name).await {
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
            _ => self.sbx.wait_until_running(name, Duration::from_secs(30)).await,
        };

        // ── Tooling and policy ───────────────────────────────────────────
        // Independent of each other — one is a script in the guest, the other
        // a host-side rule — so they run side by side and are reported in order.
        let agent_port = cfg_port(self, cfg);
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
                    true => Some(self.sbx.tooling(name, &batch).await),
                    false => None,
                }
            },
            self.sbx.policy("allow", Some(name), &allow)
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
            match self.sbx.policy("deny", Some(name), &cfg.network_deny.join(",")).await {
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
        if !created_with_kits && !cfg.kits.is_empty() {
            run.step(progress::KITS);
            let recorded = self.store.get(name).kits;
            let pending: Vec<&String> = cfg.kits.iter().filter(|k| !recorded.contains(k)).collect();
            // `sbx inspect` only when sbxw's own record does not settle it.
            let present = match pending.is_empty() {
                true => String::new(),
                false => kits::inspect_kits_section(&self.sbx.inspect(name).await.unwrap_or_default()),
            };
            for kit in &cfg.kits {
                let spec = std::fs::read_to_string(std::path::Path::new(kit).join("spec.yaml")).ok();
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
                match self.kit_add_repairing(name, kit, creds.as_deref()).await {
                    Ok(()) => {
                        let k = kit.clone();
                        let _ = self.store.update(name, |r| r.kits.push(k));
                    }
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
            run.info("storing ANTHROPIC_API_KEY as the global `anthropic` secret");
            self.sbx.secret_set("anthropic", key, None).await.context("failed to store the anthropic secret")?;
        }

        // ── Aliases and ports ────────────────────────────────────────────
        run.step(progress::PORTS);
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
                let mut fresh = templates::check(&self.store.templates(), &req.config.kits, &now);
                use templates::{Choice, Freshness};
                // Asked of sbx only for a template this bring-up would use.
                let used = match (&fresh, req.template) {
                    (Freshness::Fresh { record }, Choice::Default | Choice::Keep) | (Freshness::Stale { record, .. }, Choice::Keep) => Some(record.tag.clone()),
                    _ => None,
                };
                if let Some(tag) = used {
                    if !self.template_present(&tag).await {
                        self.forget_template(&tag, run);
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
    async fn template_present(&self, tag: &str) -> bool {
        match self.sbx.template_ls().await {
            Ok(list) => list.iter().any(|t| t.tag == tag || t.tag.ends_with(&format!("/{tag}"))),
            Err(_) => true,
        }
    }

    /// Drop the record of an automatic template sbx no longer has, so this
    /// bring-up and the create dialog both see it as missing.
    fn forget_template(&self, tag: &str, run: &mut Run<'_>) {
        let mut records = self.store.templates();
        records.retain(|r| r.tag != tag);
        match self.store.set_templates(&records) {
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
    async fn save_auto_template(&self, name: &SandboxName, kits: &[String], resolved: &[String], keep_running: bool, run: &mut Run<'_>) -> Result<()> {
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
        let mut records = self.store.templates();
        let old = templates::replace(&mut records, record);
        self.store.set_templates(&records).context("recording the automatic template")?;
        run.info(format!("new sandboxes with these kits now start from {tag}"));
        if let Some(old) = old.filter(|o| o.tag != tag) {
            match self.sbx.template_rm(&old.tag).await {
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
    pub async fn snapshot(&self, name: &SandboxName, tag: &str, keep_running: bool) -> (Result<()>, Vec<String>) {
        let mut warnings = Vec::new();
        let mut kept: Vec<(&str, Vec<u8>)> = Vec::new();
        for path in templates::SECRETS {
            match self.sbx.exec(name, &["cat", path], None).await {
                Ok(out) if out.success() => kept.push((path, out.stdout)),
                Ok(_) => {}
                Err(e) => return (Err(e), warnings),
            }
        }
        let scrub: Vec<&str> = ["rm", "-f"].into_iter().chain(templates::SECRETS).collect();
        match self.sbx.exec(name, &scrub, None).await {
            Ok(out) if out.success() => {}
            Ok(out) => return (Err(anyhow!("could not take the sandbox's secrets out: {}", String::from_utf8_lossy(&out.stderr).trim())), warnings),
            Err(e) => return (Err(e), warnings),
        }

        let saved = self.sbx.template_save(name, tag).await;

        let stopped = matches!(saved, Ok(true)) || !self.sbx.is_running(name).await;
        if stopped {
            let back = self.sbx.run_detached(name).await.is_ok() && self.sbx.wait_until_running(name, Duration::from_secs(60)).await;
            if !back {
                warnings.push(format!("'{name}' did not come back after the snapshot — its token returns on `sbxw up {name}`"));
                return (saved.map(drop), warnings);
            }
        }
        for (path, data) in &kept {
            if let Err(e) = self.sbx.write_file(name, path, data, true).await {
                warnings.push(format!("could not put {path} back after the snapshot ({e:#}) — `sbxw up {name}` reinstalls it"));
            }
        }
        if stopped && !keep_running {
            // `run -d` holds it up with nobody attached; give it back its idle stop.
            let _ = self.sbx.stop(name).await;
        }
        (saved.map(drop), warnings)
    }

    /// Until none of the kits' startup commands is running in the guest:
    /// they run in the background after `sbx create` returns, and a snapshot
    /// taken mid-install is a broken template.
    async fn wait_for_startups(&self, name: &SandboxName, commands: &[String], run: &mut Run<'_>) -> Result<()> {
        if commands.is_empty() {
            return Ok(());
        }
        run.info("waiting for the kits' startup installs to finish before the snapshot");
        let started = tokio::time::Instant::now();
        let deadline = started + STARTUP_TIMEOUT;
        let mut quiet = 0;
        let mut seen = false;
        let mut shown = String::new();
        loop {
            let out = self.sbx.exec(name, &["sh", "-c", templates::PROCESS_LIST], None).await?;
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
    async fn create_with_port_fallback(&self, name: &SandboxName, opts: &CreateOpts, sink: &Sink, run: &mut Run<'_>) -> Result<()> {
        let s = sink.clone();
        let lines: Arc<dyn Fn(&str) + Send + Sync> = Arc::new(move |l: &str| {
            if let Some(p) = progress::detail(l) {
                s(p);
            }
        });
        let first = match self.sbx.create(name, opts, Some(lines.clone())).await {
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
        if self.sbx.exists(name).await {
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
        self.sbx
            .create(name, &bare, Some(lines))
            .await
            .map_err(|e| anyhow!("{e:#}\n(create with port mappings had failed with: {first:#})"))
    }

    /// Check the person's own local kits offline before anything is created: a
    /// broken kit fails here in a second, not after the image pull. Bundled
    /// kits ship tested; remote references cannot be checked offline, and
    /// neither can a v3 kit, which `kit validate` cannot read ([`kits::is_v3_kit`]).
    async fn validate_kits(&self, kits: &[String]) -> Result<()> {
        let local: Vec<&String> = kits
            .iter()
            .filter(|k| {
                let path = std::path::Path::new(k.as_str());
                kits::bundled_name(k).is_none() && path.exists() && !kits::is_v3_kit(path)
            })
            .collect();
        let checks = local.iter().map(|k| self.sbx.kit_validate_offline(k));
        let mut broken = Vec::new();
        for (kit, r) in local.iter().zip(futures_util::future::join_all(checks).await) {
            match r {
                Ok(()) => {}
                Err(e) => broken.push(format!("{kit}: {e:#}")),
            }
        }
        if !broken.is_empty() {
            bail!("kit validation failed — nothing was created or changed:\n{}", broken.join("\n"));
        }
        Ok(())
    }

    /// `sbx kit add`, repairing once a stale OAuth kit reference an older
    /// sbxw left in a temp dir (sbx names the *wrong* kit when it fails).
    async fn kit_add_repairing(&self, name: &SandboxName, kit: &str, creds: Option<&str>) -> Result<()> {
        let Err(first) = self.sbx.kit_add(name, kit).await else {
            return Ok(());
        };
        let Some(stale) = kits::stale_oauth_kit_path(&format!("{first:#}")) else {
            return Err(first);
        };
        tracing::warn!("restoring a stale OAuth kit at {} so '{name}' can be recomposed", stale.display());
        crate::store::write_atomic(&stale.join("spec.yaml"), kits::oauth_kit_spec(creds).as_bytes())?;
        self.sbx
            .kit_add(name, kit)
            .await
            .map_err(|e| anyhow!("{e:#}\n(retried after restoring {}; first attempt: {first:#})", stale.display()))
    }

    /// An older sbxw gave sandboxes `localhost:<ui port>`, through which they
    /// could reach the browser's endpoints. Remove that rule if it is there.
    async fn revoke_ui_port(&self, name: &SandboxName, run: &mut Run<'_>) {
        let ui = format!("localhost:{}", self.cfg.web_port());
        let view = self.sbx.policy_ls(name, true).await;
        let found = sbxw_core::policy::rule_ids_for(&view, &ui, name);
        for id in found.mine {
            match self.sbx.policy_rm(&id, Some(name)).await {
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
    fn republish_ports(self: &Arc<Self>, name: SandboxName, specs: Vec<String>, up: bool) {
        if specs.is_empty() {
            return;
        }
        let app = self.clone();
        tokio::spawn(async move {
            if !up && !app.sbx.wait_until_running(&name, Duration::from_secs(60)).await {
                tracing::info!("'{name}' is not running; its ports are published on its next start");
                return;
            }
            for spec in &specs {
                match app.sbx.publish(&name, spec).await {
                    Ok(()) => tracing::info!("published {spec} for '{name}'"),
                    Err(e) => tracing::warn!(
                        "could not publish {spec} for '{name}': {e:#}\nif that host port is taken, free \
                         it (or change host_port) and run `sbxw ports {name}`"
                    ),
                }
            }
        });
    }
}

fn cfg_port(app: &App, _project: &Config) -> String {
    // The agent port is the daemon's, whatever the project file says: the
    // hooks have to reach the process that is actually listening.
    app.cfg.agent_port().to_string()
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
        let kits = Config { kits: vec!["k".into()], ..bare.clone() };
        assert!(!ids(false, &kits).contains(&"kits".to_string()));
        assert!(ids(true, &kits).contains(&"kits".to_string()));
        assert!(ids(false, &Config::default()).contains(&"policy".to_string()));
    }

    fn auto_request(dir: &std::path::Path, kits: Vec<String>, template: templates::Choice) -> BringUp {
        let ws = dir.join("neos");
        std::fs::create_dir_all(&ws).unwrap();
        BringUp {
            name: SandboxName::parse("neos").unwrap(),
            workspace: ws,
            ro: vec![],
            rw: vec![],
            config: Config { kits, template: "auto".into(), network_allow: vec![], network_deny: vec![], ..Config::default() },
            extra_ports: vec![],
            chat: false,
            api_key: None,
            oauth_token: None,
            template,
            remote_control: false,
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
        std::fs::write(kit.join("spec.yaml"), "schemaVersion: \"2\"\nkind: mixin\nname: kit\nsetup:\n  startup:\n    - command: [\"bash\", \"/k/install.sh\"]\n").unwrap();
        kit.to_string_lossy().into_owned()
    }

    #[tokio::test]
    async fn a_rebuild_waits_for_the_installs_snapshots_without_secrets_and_drops_the_old_template() {
        let (dir, app, fake) = app_with(auto_sbx(3));
        let kits = vec![kit_with_startup(dir.path())];
        let old = templates::Record { tag: "sbxw-auto:old".into(), sbx: "0.47.0".into(), sbxw: "2.1.2".into(), kits: kits.clone(), created_at: 1, content: String::new() };
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
        assert_eq!((records[0].sbxw.as_str(), records[0].sbx.as_str()), (env!("CARGO_PKG_VERSION"), "0.50.0"));
        assert!(log.iter().any(|c| c == &format!("template save neos {}", records[0].tag)));
    }

    #[tokio::test]
    async fn a_fresh_template_is_used_and_a_stale_one_only_warns() {
        let (dir, app, fake) = app_with(auto_sbx(0));
        let kits = vec!["sbxw:codemap".to_string()];
        let fresh = templates::Record { tag: "sbxw-auto:fresh".into(), sbx: "0.50.0".into(), sbxw: env!("CARGO_PKG_VERSION").into(), kits: kits.clone(), created_at: 1, content: crate::bundled::fingerprint(&app.store.paths, &kits).unwrap() };
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
        assert!(w.iter().any(|w| w.contains("out of date (sbx 0.47.0 → 0.50.0)")), "{w:?}");
        let log = fake.log();
        assert!(log.iter().any(|c| c.starts_with("create") && !c.contains("--template")));
        assert!(!log.iter().any(|c| c.starts_with("template")), "nothing saved or removed unasked");
    }

    #[tokio::test]
    async fn a_template_gone_from_sbx_is_forgotten_not_pulled() {
        let (dir, app, fake) = app_with(auto_sbx(0));
        let kits = vec!["sbxw:codemap".to_string()];
        let content = crate::bundled::fingerprint(&app.store.paths, &kits).unwrap();
        let gone = templates::Record { tag: "sbxw-auto:gone".into(), sbx: "0.50.0".into(), sbxw: env!("CARGO_PKG_VERSION").into(), kits: kits.clone(), created_at: 1, content };
        app.store.set_templates(&[gone]).unwrap();
        let w = app.provision(auto_request(dir.path(), kits, templates::Choice::Default), None).await.unwrap();
        assert!(w.iter().any(|w| w.contains("sbxw-auto:gone is no longer in sbx")), "{w:?}");
        assert!(fake.log().iter().any(|c| c.starts_with("create") && !c.contains("--template")), "created from sbx's image");
        assert!(app.store.templates().is_empty(), "the record is dropped");
    }

    #[tokio::test]
    async fn a_broken_local_kit_stops_the_bring_up_before_create() {
        let (dir, app, fake) = app_with(|a: &[String]| match a.first().map(String::as_str) {
            Some("kit") if a.iter().any(|x| x.ends_with("broken")) => Output::fail(1, "spec.yaml: unknown field `setpu`"),
            _ => sbx_answer(a),
        });
        let ws = dir.path().join("neos");
        let (good, broken) = (dir.path().join("good"), dir.path().join("broken"));
        for d in [&ws, &good, &broken] {
            std::fs::create_dir_all(d).unwrap();
        }
        let kits = vec![good.to_string_lossy().into_owned(), broken.to_string_lossy().into_owned(), "sbxw:codemap".into(), "docker.io/o/k:1".into()];
        let req = BringUp {
            name: SandboxName::parse("neos").unwrap(),
            workspace: ws,
            ro: vec![],
            rw: vec![],
            config: Config { kits, ..Config::default() },
            extra_ports: vec![],
            chat: false,
            api_key: None,
            oauth_token: None,
            template: Default::default(),
            remote_control: false,
        };
        let e = app.provision(req, None).await.unwrap_err();
        assert!(format!("{e:#}").contains("unknown field `setpu`"), "{e:#}");
        let log = fake.log();
        assert_eq!(log.iter().filter(|c| c.starts_with("kit validate")).count(), 2, "local, non-bundled kits only: {log:?}");
        assert!(log.iter().all(|c| !c.starts_with("create")), "nothing created: {log:?}");
    }

    #[tokio::test]
    async fn a_v3_kit_is_left_to_create() {
        let (dir, app, fake) = app_with(|a: &[String]| match a.first().map(String::as_str) {
            // What sbx 0.50's `kit validate` says of every v3 spec.
            Some("kit") => Output::fail(1, "field capabilities not found in type spec.SpecFile"),
            _ => sbx_answer(a),
        });
        let ws = dir.path().join("neos");
        let kit = dir.path().join("k");
        std::fs::create_dir_all(&ws).unwrap();
        std::fs::create_dir_all(&kit).unwrap();
        std::fs::write(kit.join("spec.yaml"), "schemaVersion: \"3\"\nkind: mixin\n").unwrap();
        let req = BringUp {
            name: SandboxName::parse("neos").unwrap(),
            workspace: ws,
            ro: vec![],
            rw: vec![],
            config: Config { kits: vec![kit.to_string_lossy().into_owned()], ..Config::default() },
            extra_ports: vec![],
            chat: false,
            api_key: None,
            oauth_token: None,
            template: Default::default(),
            remote_control: false,
        };
        let _ = app.provision(req, None).await;
        let log = fake.log();
        assert!(!log.iter().any(|c| c.starts_with("kit validate")), "{log:?}");
        assert!(log.iter().any(|c| c.starts_with("create")), "{log:?}");
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
                    name: SandboxName::parse("neos").unwrap(),
                    workspace: ws.clone(),
                    ro: vec![],
                    rw: vec![],
                    config: cfg,
                    extra_ports: vec![],
                    chat: false,
                    api_key: None,
                    oauth_token: Some("tok".into()),
                    template: Default::default(),
                    remote_control: false,
                },
                Some(sink),
            )
            .await
            .unwrap();
        assert!(warnings.iter().all(|w| !w.contains("tooling")), "{warnings:?}");
        let log = fake.log();
        let create = log.iter().find(|c| c.starts_with("create")).unwrap();
        assert!(create.contains("--kit") && create.contains("-oauth") && create.contains("--kit /kits/codemap"));
        assert!(create.contains("-p 4200:4200"));
        assert!(create.contains("-e ANTHROPIC_DEFAULT_MODEL=claude-sonnet-5-5"));
        let allow = log.iter().find(|c| c.starts_with("policy allow")).unwrap();
        assert!(allow.contains("localhost:7682") && !allow.contains("localhost:7681"), "{allow}");
        assert!(ws.join(".sbxw-artifacts/README.md").exists());
        let rec = app.store.get("neos");
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
            name: SandboxName::parse("neos").unwrap(),
            workspace: ws,
            ro: vec![],
            rw: vec![],
            config: cfg,
            extra_ports: vec![],
            chat: false,
            api_key: Some("sk-key".into()),
            oauth_token: Some("tok".into()),
            template: Default::default(),
            remote_control: true,
        };
        app.provision(req.clone(), None).await.unwrap();
        let log = fake.log();
        let create = log.iter().find(|c| c.starts_with("create")).unwrap();
        assert!(!create.contains("-oauth") && !create.contains("from-config"), "{create}");
        assert!(log.iter().all(|c| !c.starts_with("secret set")), "{log:?}");
        assert!(log.iter().all(|c| !c.contains(".credentials.json")), "{log:?}");
        let allow = log.iter().find(|c| c.starts_with("policy allow")).unwrap();
        assert!(allow.contains("claude.ai") && allow.contains("platform.claude.com"), "{allow}");
        assert!(app.store.get("neos").remote_control);

        // Brought up again without asking, it still gets no token.
        app.provision(BringUp { remote_control: false, ..req }, None).await.unwrap();
        assert!(fake.log().iter().all(|c| !c.contains(".credentials.json") && !c.contains("kit add")));
        assert!(app.store.get("neos").remote_control);
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
                    name: SandboxName::parse("w").unwrap(),
                    workspace: ws,
                    ro: vec![],
                    rw: vec![],
                    config: with_port(),
                    extra_ports: vec![],
                    chat: false,
                    api_key: None,
                    oauth_token: None,
                    template: Default::default(),
                    remote_control: false,
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
                name: SandboxName::parse(name).unwrap(),
                workspace: ws,
                ro: vec![],
                rw: vec![],
                config: Config::default(),
                extra_ports: vec![],
                chat: false,
                api_key: None,
                oauth_token: None,
                template: Default::default(),
                remote_control: false,
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
            name: SandboxName::parse("neos").unwrap(),
            workspace: ws,
            ro: vec![],
            rw: vec![],
            config: Config { kits: vec![], volumes: vec!["/home/agent/.npm".into()], ..Config::default() },
            extra_ports: vec![],
            chat: false,
            api_key: None,
            oauth_token: None,
            template: Default::default(),
            remote_control: false,
        };
        app.provision(req, None).await.unwrap();
        let kit = app.store.paths.volumes_kit("neos");
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
        app.create(SandboxName::parse("km").unwrap(), &ws.to_string_lossy(), form, None).await.unwrap();
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
        let name = SandboxName::parse("multi").unwrap();
        let mounts = vec![(docs.clone(), true), (lib.clone(), false)];
        app.create(name.clone(), &ws, CreateForm { mounts, ..Default::default() }, None).await.unwrap();
        let log = fake.log();
        let create = log.iter().find(|c| c.starts_with("create")).unwrap();
        assert!(create.starts_with(&format!("create claude {ws} --name multi {lib} ")), "{create}");
        assert!(create.contains(&format!(" {docs}:ro")), "{create}");
        let mounts = app.store.get("multi").mounts;
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
