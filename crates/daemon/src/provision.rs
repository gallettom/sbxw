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
use crate::bus::{Event, ProvisionEvent};
use anyhow::{anyhow, bail, Context, Result};
use sbxw_core::artifacts;
use sbxw_core::config::Config;
use sbxw_core::kits;
use sbxw_core::ports::{self, WantedPort};
use sbxw_core::progress::{self, Progress, Sink, Step};
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
}

/// The steps this bring-up will take, announced first. Two are conditional in
/// exactly the way the body is.
pub fn plan(existed: bool, cfg: &Config) -> Vec<Step> {
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
        let cfg = &req.config;
        let mut run = Run { sink, warnings: vec![] };
        let workspace = req.workspace.to_string_lossy().into_owned();
        if !req.workspace.is_dir() {
            bail!("{workspace} is not a directory");
        }

        // Asked first: it decides two of the announced steps.
        let initial = self.sbx.lookup(name).await;
        let existed = initial.is_some();
        let stopped = initial.as_ref().is_some_and(|i| i.is_stopped());
        let mut running = Some(initial.as_ref().is_some_and(|i| i.is_running()));
        sink(Progress::Plan { steps: plan(existed, cfg) });

        // ── Workspace ────────────────────────────────────────────────────
        run.step(progress::WORKSPACE);
        self.store
            .update(name, |r| {
                r.workspace = Some(req.workspace.clone());
                r.config = cfg.source.clone();
                r.chat = req.chat;
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
            .map(|t| kits::oauth_credentials_json(t, &cfg.claude_subscription));
        let wanted = cfg.wanted_ports(&req.extra_ports);
        let specs = ports::publish_specs(&wanted, cfg.ip_per_app);
        let mut own_allow: Vec<String> = Vec::new();

        // ── Create, or reuse ─────────────────────────────────────────────
        run.step(progress::CREATE);
        let mut created_with_kits = false;
        if existed {
            run.info(format!("sandbox '{name}' already exists — reusing it"));
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
                        run.warn(format!("OAuth kit add failed (use /login in-session instead): {e:#}"));
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
            let opts = CreateOpts {
                workspace: workspace.clone(),
                ro_mounts: req.ro.clone(),
                kits: kit_refs,
                publish: specs.clone(),
                skills: cfg.skills_mode().map(String::from),
                pull: cfg.pull_policy().map(String::from),
                env: cfg.env_pairs(),
                env_files: cfg.env_files.clone(),
            };
            self.create_with_port_fallback(name, &opts, sink, &mut run).await?;
            created_with_kits = true;
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

        // ── Tooling ──────────────────────────────────────────────────────
        let agent_port = cfg_port(self, cfg);
        if up {
            run.step(progress::TOOLING);
            let migrate = !cfg.claude_model.is_empty() && !self.store.get(name).model_migrated;
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
            match self.sbx.tooling(name, &batch).await {
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
        // The proxy sees host.docker.internal as localhost:<port>.
        own_allow.push(format!("localhost:{agent_port}"));

        // ── Policy ───────────────────────────────────────────────────────
        if !cfg.network_allow.is_empty() || !cfg.network_deny.is_empty() {
            run.step(progress::POLICY);
        }
        let allow = [own_allow.as_slice(), cfg.network_allow.as_slice()].concat().join(",");
        match self.sbx.policy("allow", Some(name), &allow).await {
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
        self.revoke_ui_port(name, &mut run).await;

        // ── Kits, for a sandbox that already existed ─────────────────────
        if !created_with_kits && !cfg.kits.is_empty() {
            run.step(progress::KITS);
            let present = kits::inspect_kits_section(&self.sbx.inspect(name).await.unwrap_or_default());
            for kit in &cfg.kits {
                let spec = std::fs::read_to_string(std::path::Path::new(kit).join("spec.yaml")).ok();
                let shown = kits::display_name(kit, spec.as_deref());
                if shown.len() >= 3 && present.contains(&shown) {
                    run.info(format!("kit '{kit}' already applied — skipping"));
                    continue;
                }
                let recreate = |run: &mut Run, said: Option<String>| {
                    run.warn(format!(
                        "kit '{kit}' declares startup commands, which `sbx kit add` cannot apply to an \
                         existing sandbox — '{name}' is unchanged. Recreate it to pick the kit up: \
                         `sbxw rm {name}` then `sbxw up {name}`. Anything outside the workspace mount \
                         is lost in the process.{}",
                        said.map(|s| format!("\n{s}")).unwrap_or_default()
                    ))
                };
                if spec.as_deref().is_some_and(kits::declares_startup) {
                    recreate(&mut run, None);
                    continue;
                }
                run.info(format!("applying kit {kit} (this recreates the container; state is kept)"));
                match self.kit_add_repairing(name, kit, creds.as_deref()).await {
                    Ok(()) => {}
                    Err(e) if kits::add_needs_recreate(&format!("{e:#}")) => recreate(&mut run, Some(format!("{e:#}"))),
                    Err(e) => run.warn(format!("kit '{kit}' failed to apply: {e:#}")),
                }
            }
        }

        if let Some(key) = req.api_key.as_deref().map(str::trim).filter(|k| !k.is_empty()) {
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
        Ok(run.warnings)
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

    #[test]
    fn the_announced_plan_matches_the_steps_taken() {
        let ids = |existed, cfg: &Config| plan(existed, cfg).into_iter().map(|s| s.id).collect::<Vec<_>>();
        let bare = Config { network_allow: vec![], network_deny: vec![], kits: vec![], ..Config::default() };
        assert_eq!(ids(false, &bare), ["workspace", "create", "boot", "tooling", "ports"]);
        let kits = Config { kits: vec!["k".into()], ..bare.clone() };
        assert!(!ids(false, &kits).contains(&"kits".to_string()));
        assert!(ids(true, &kits).contains(&"kits".to_string()));
        assert!(ids(false, &Config::default()).contains(&"policy".to_string()));
    }

    #[tokio::test]
    async fn a_fresh_bring_up_creates_with_kits_ports_and_the_agent_port_rule() {
        let created = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let c = created.clone();
        let (dir, app, fake) = app_with(move |a: &[String]| {
            let joined = a.join(" ");
            match a.first().map(String::as_str) {
                Some("ls") if c.load(std::sync::atomic::Ordering::SeqCst) => {
                    Output::ok("SANDBOX   AGENT    STATUS\nneos      claude   running\n")
                }
                Some("ls") => Output::ok("SANDBOX   AGENT    STATUS\n"),
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
        let cfg = Config { kits: vec!["/kits/codemap".into()], ..Config::default() };
        let seen = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let s2 = seen.clone();
        let sink: Sink = Arc::new(move |p| s2.lock().unwrap().push(serde_json::to_string(&p).unwrap()));
        let warnings = app
            .provision(
                BringUp {
                    name: SandboxName::parse("neos").unwrap(),
                    workspace: ws.clone(),
                    ro: vec![],
                    config: cfg,
                    extra_ports: vec![],
                    chat: false,
                    api_key: None,
                    oauth_token: Some("tok".into()),
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
                Output::ok("SANDBOX   AGENT    STATUS\nw         claude   running\n")
            }
            Some("ls") => Output::ok("SANDBOX   AGENT    STATUS\n"),
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
                    config: Config::default(),
                    extra_ports: vec![],
                    chat: false,
                    api_key: None,
                    oauth_token: None,
                },
                None,
            )
            .await
            .unwrap();
        assert!(w.iter().any(|w| w.contains("retrying without them")));
        assert_eq!(fake.log().iter().filter(|c| c.starts_with("create")).count(), 2);
    }
}
