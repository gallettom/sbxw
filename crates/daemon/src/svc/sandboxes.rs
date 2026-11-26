//! Sandboxes as the UI and the CLI act on them: listing, removal with its
//! cleanup, duplicate, chats, threads, ports, policy, the env file export.

use crate::app::App;
use crate::git;
use crate::host::Host;
use crate::provision::{BringUp, Consent};
use anyhow::{anyhow, bail, Context, Result};
use sbxw_core::config::Config;
use sbxw_core::envexport;
use sbxw_core::envfile;
use sbxw_core::hosts::HostAlias;
use sbxw_core::names;
use sbxw_core::policy::{BlockedHost, PolicyView};
use sbxw_core::templates;
use sbxw_core::ports::WantedPort;
use sbxw_core::progress::Sink;
use sbxw_core::session::{SessionKey, SessionKind};
use sbxw_core::{SandboxName, SandboxRef};
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// How long a listing waits for a remote host's link that is still making
/// its first connection; after one failure it does not wait at all.
const LIST_WAIT: std::time::Duration = std::time::Duration::from_secs(5);

#[derive(Serialize, Debug, Clone)]
pub struct SandboxItem {
    pub name: String,
    pub agent: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace: Option<String>,
    /// A genuine chat sandbox (its own throwaway workspace): grouped apart.
    pub chat: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,
    /// Held up with nobody attached (`keep_running`, `sbx run -d`).
    pub detached: bool,
    /// Created for Remote Control: its agent logs in to claude.ai itself.
    pub remote_control: bool,
    /// Requests sbx waits for a person to decide for this sandbox (`sbx policy
    /// approval`): hosts no rule allows, a kit's host port.
    pub approvals: usize,
    /// Kits `sbx kit add` left waiting for the person's approval: a ✓ in the
    /// sidebar opens a pane where they answer sbx's prompt.
    pub kits_waiting: usize,
    /// Made by an sbx older than 0.51: it keeps running, but takes no kit
    /// until it is recreated (`sbx inspect` records no creator version).
    pub old_sbx: bool,
    /// On a remote host that cannot be reached since then (unix seconds):
    /// `status` is the last one seen, and actions are refused.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unreachable_since: Option<u64>,
    /// The model the main agent session runs (an id): what the session
    /// reported, else what sbxw starts it with. Absent when unknown.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// That model's display name, when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_name: Option<String>,
    /// What its agent sessions have cost so far, in US dollars: the sum of
    /// Claude Code's own estimates (see `SessionInfo::cost_usd`), counting
    /// every session since the daemon saw it running, ended ones included,
    /// until it stops. Absent when none reported one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
}

#[derive(Serialize)]
pub struct ThreadsView {
    pub threads: Vec<serde_json::Value>,
    pub open: usize,
    pub max: usize,
}

#[derive(Serialize)]
pub struct PolicyPanel {
    pub ok: bool,
    pub policies: PolicyView,
    pub rules: PolicyView,
    pub log: PolicyView,
    /// Hosts refused recently, one row each, from `policy log --format json`;
    /// empty when this sbx gave a table instead.
    pub blocked: Vec<BlockedHost>,
    pub configured_allow: Vec<String>,
    pub configured_deny: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

pub struct RenderedEnvFile {
    pub yaml: String,
    pub dest: PathBuf,
    pub dir: PathBuf,
}

/// What the create dialog asks for beyond a name and a folder.
#[derive(Default)]
pub struct CreateForm {
    /// Other repos mounted beside the workspace, as `(path, readonly)`.
    pub mounts: Vec<(String, bool)>,
    pub ports: Vec<WantedPort>,
    pub template: templates::Choice,
    pub remote_control: bool,
    /// Start the agent with `--chrome` (Claude in Chrome).
    pub chrome: bool,
    /// Host folders for kit-declared host mounts, over `kit_mounts`.
    pub kit_mounts: std::collections::BTreeMap<String, String>,
    /// The host-access grant lines the person has just approved.
    pub approved: Vec<String>,
    /// Folders kept out of the sandbox or read-only, chosen in the dialog.
    pub carve: Vec<sbxw_core::config::Carve>,
}

/// The create dialog's carves and mounts as the planner sees them, for the
/// preview and for creation.
pub fn carve_extras(cfg: &Config, mounts: &[(String, bool)]) -> Vec<sbxw_core::config::Workspace> {
    cfg.workspaces
        .iter()
        .filter_map(|w| sbxw_core::config::parse_workspace(w).ok())
        .chain(mounts.iter().map(|(p, ro)| sbxw_core::config::Workspace { path: p.clone(), target: None, readonly: *ro }))
        .collect()
}

impl App {
    pub async fn sandboxes(&self) -> Vec<SandboxItem> {
        // Two independent `sbx` calls: run side by side, not one after the other.
        let (listed, pending) = tokio::join!(self.sbx.ls(), self.sbx.approvals());
        // Which sbx made each sandbox sbxw knows, asked of `sbx inspect` once
        // and recorded: it never changes.
        let unknown: Vec<SandboxRef> = listed
            .iter()
            .filter_map(|s| SandboxName::parse(&s.name).ok())
            .map(SandboxRef::from)
            .filter(|n| self.store.exists(n) && self.store.get(n).created_by_sbx.is_none())
            .collect();
        let asked = futures_util::future::join_all(unknown.iter().map(|n| self.sbx.inspect(&n.name))).await;
        for (n, r) in unknown.iter().zip(asked) {
            if let Ok(i) = r {
                let v = i.created_version.unwrap_or_default();
                let _ = self.store.update(n, |rec| rec.created_by_sbx = Some(v));
            }
        }
        let waiting = self.kit_approvals.lock().unwrap().clone();
        let pending = pending.unwrap_or_default();
        let infos = self.session_infos();
        // The daemon's scratch sandboxes (a template's builder, the cache's
        // filler) are nobody's to open, stop or remove while they live.
        let scratches = self.scratches.lock().unwrap().clone();
        // The ticket sync's sandbox is the daemon's too, and runs no agent.
        let scratch = |name: &str| SandboxRef::parse(name).is_ok_and(|r| scratches.contains(&r) || crate::svc::tracker_sandbox::is_sync_name(&r));
        let pending_for = |name: &str| pending.iter().filter(|a| a.sandbox == name).count();
        let waiting_for = |name: &str| SandboxRef::parse(name).ok().and_then(|r| waiting.get(&r).map(Vec::len)).unwrap_or(0);
        listed
            .into_iter()
            .filter(|s| !scratch(&s.name))
            .map(|s| {
                // sbx lists this machine's sandboxes; one whose name sbxw would
                // refuse has no record to look up.
                let rec = SandboxName::parse(&s.name).map(|n| self.store.get(&n)).unwrap_or_default();
                let chat = SandboxName::parse(&s.name).is_ok_and(|n| self.store.chat_workspace(&n).is_some());
                (s, rec, chat)
            })
            .map(|(s, rec, chat)| SandboxItem {
                approvals: pending_for(&s.name),
                kits_waiting: waiting_for(&s.name),
                chat,
                old_sbx: rec.created_by_sbx.as_deref() == Some(""),
                remote_control: rec.remote_control,
                workspace: rec.workspace.map(|w| w.to_string_lossy().into_owned()),
                name: s.name,
                agent: s.agent,
                status: s.status,
                created_at: s.created_at,
                detached: s.detached,
                unreachable_since: None,
                model: None,
                model_name: None,
                cost_usd: None,
            })
            .collect::<Vec<_>>()
            .into_iter()
            .chain(self.remote_sandboxes().await.into_iter().filter(|i| !scratch(&i.name)))
            .map(|mut item| {
                let (model, model_name) = self.main_model(&item.name, &item.agent, &infos);
                item.model = model;
                item.model_name = model_name;
                item.cost_usd = self.sandbox_cost(&item.name);
                item
            })
            .collect()
    }

    /// Every registered remote host's sandboxes, named by ref. A host that
    /// answers is asked (and what it says remembered); one that does not is
    /// listed from those records, marked unreachable.
    async fn remote_sandboxes(&self) -> Vec<SandboxItem> {
        let hosts = crate::remotes::load_hosts(&self.store.paths).unwrap_or_default();
        let mut out = Vec::new();
        for name in hosts.hosts.values().map(|h| h.name.clone()) {
            let Ok(r) = self.remote(&name).await else { continue };
            let known: Vec<SandboxRef> = self.store.names().into_iter().filter(|n| n.host == name).collect();
            let up = r.link.settle(LIST_WAIT).await == crate::host::link::LinkState::Up;
            // Not `Sbx::ls`, which reads a host that stopped answering as
            // "no sandboxes": that is unreachable, not empty.
            let listed = match (up, r.sbx().await) {
                (true, Ok(sbx)) => list_strict(&sbx).await.ok(),
                _ => None,
            };
            match (listed, r.down_since()) {
                (Some(list), _) => {
                    for s in list {
                        let Ok(n) = SandboxName::parse(&s.name) else { continue };
                        let sb = SandboxRef::new(name.clone(), n);
                        let rec = self.store.get(&sb);
                        if known.contains(&sb) && rec.last_status.as_deref() != Some(s.status.as_str()) {
                            let st = s.status.clone();
                            let _ = self.store.update(&sb, |r| r.last_status = Some(st));
                        }
                        out.push(SandboxItem {
                            name: sb.to_string(),
                            agent: s.agent,
                            status: s.status,
                            workspace: rec.workspace.map(|w| w.to_string_lossy().into_owned()),
                            chat: false,
                            created_at: s.created_at,
                            detached: s.detached,
                            remote_control: rec.remote_control,
                            old_sbx: rec.created_by_sbx.as_deref() == Some(""),
                            kits_waiting: self.kit_approvals.lock().unwrap().get(&sb).map(Vec::len).unwrap_or(0),
                            approvals: 0,
                            unreachable_since: None,
                            model: None,
                            model_name: None,
                            cost_usd: None,
                        });
                    }
                }
                (None, since) => {
                    let since = since.unwrap_or_else(|| crate::now_ms() / 1000);
                    for sb in known {
                        let rec = self.store.get(&sb);
                        out.push(SandboxItem {
                            name: sb.to_string(),
                            agent: "claude".into(),
                            status: rec.last_status.clone().unwrap_or_else(|| "unknown".into()),
                            workspace: rec.workspace.map(|w| w.to_string_lossy().into_owned()),
                            chat: false,
                            created_at: None,
                            detached: false,
                            remote_control: rec.remote_control,
                            old_sbx: rec.created_by_sbx.as_deref() == Some(""),
                            kits_waiting: 0,
                            approvals: 0,
                            unreachable_since: Some(since),
                            model: None,
                            model_name: None,
                            cost_usd: None,
                        });
                    }
                }
            }
        }
        out
    }

    /// Remove sandboxes and everything sbxw keeps about them: sessions, record,
    /// kit and, for a chat, its throwaway workspace. Only what sbx reports
    /// removed is forgotten; the rest is the error. Each host's are removed by
    /// that host's sbx.
    pub async fn remove(&self, names: &[SandboxRef]) -> Result<()> {
        let mut by_host: BTreeMap<sbxw_core::names::HostName, Vec<SandboxRef>> = BTreeMap::new();
        for n in names {
            by_host.entry(n.host.clone()).or_default().push(n.clone());
        }
        let mut failed: Vec<String> = Vec::new();
        let mut chats: Vec<PathBuf> = Vec::new();
        // A bring-up's kit-startup watch ends first: no look at a sandbox
        // being removed (`svc::timings`).
        for n in names {
            self.timing_stop_watch(n);
        }
        for refs in by_host.values() {
            let sbx = self.sbx_for(&refs[0]).await?;
            let bare: Vec<SandboxName> = refs.iter().map(|r| r.name.clone()).collect();
            let results = sbx.rm(&bare).await?;
            let gone: Vec<&SandboxRef> = refs.iter().filter(|n| results.iter().any(|r| r.removed && r.name == n.name.as_str())).collect();
            chats.extend(gone.iter().filter_map(|n| self.store.chat_workspace(*n)));
            for n in &gone {
                if crate::svc::jarvis_sandbox::is_jarvis_name(n) {
                    crate::svc::jarvis_sandbox::forget(&self.store.paths);
                }
                self.sessions.drop_sandbox(&n.to_string());
                self.lead_gone(n);
                self.history_forget(n);
                self.browser_forget(n);
                self.store.forget(*n);
            }
            self.forget_remote(&gone);
            failed.extend(
                results
                    .iter()
                    .filter(|r| !r.removed)
                    .map(|r| format!("'{}': {}", r.name, r.error.as_deref().unwrap_or("not removed"))),
            );
        }
        for dir in chats {
            if let Err(e) = std::fs::remove_dir_all(&dir) {
                if e.kind() != std::io::ErrorKind::NotFound {
                    tracing::warn!("could not remove chat workspace {}: {e}", dir.display());
                }
            }
        }
        if !failed.is_empty() {
            bail!("sbx could not remove {}", failed.join("; "));
        }
        Ok(())
    }

    /// A bring-up from the browser: the daemon's config, the dialog's ports,
    /// and the other repos it mounts beside the workspace as `(path, readonly)`.
    pub async fn create(self: &Arc<Self>, name: SandboxRef, path: &str, form: CreateForm, sink: Option<Sink>) -> Result<Vec<String>> {
        let CreateForm { mounts, ports: extra, template, remote_control, chrome, kit_mounts, approved, carve } = form;
        // Before "already exists", which would send the person to remove it.
        if let Some(why) = crate::svc::jarvis_sandbox::reserved(&name) {
            bail!("{why}");
        }
        let path = PathBuf::from(path.trim());
        // The folders are the sandbox's host's, checked there.
        let host = self.host_for(&name).await?;
        if !matches!(host.fs.stat(&path).await?, Some(m) if m.kind == crate::host::Kind::Dir) {
            bail!("path is not a directory");
        }
        // `provision` reuses a sandbox that exists, as `sbxw up` should — but
        // this dialog's folders and modes would then be dropped in silence.
        if self.sbx_for(&name).await?.exists(&name.name).await {
            bail!("a sandbox named '{name}' already exists — pick another name, or remove it first");
        }
        let (ro, rw) = extra_mounts(&host, &path, mounts).await?;
        tracing::info!(
            "web UI: provisioning sandbox '{name}' at {} ({} read-only and {} writable repos beside it, {} extra ports)",
            path.display(),
            ro.len(),
            rw.len(),
            extra.len()
        );
        let mut cfg = self.daemon_config();
        // The file's grants, before the folders the person picked in the
        // dialog join them: those need no second yes.
        self.check_consent(&cfg, None, &approved)?;
        cfg.kit_mounts.extend(kit_mounts.into_iter().filter(|(_, host)| !host.trim().is_empty()));
        cfg.validate()?;
        let mut req = self.bring_up_request(name, path, cfg, extra, false, Consent::Checked(approved));
        req.ro = ro;
        req.rw = rw;
        req.template = template;
        req.remote_control = remote_control;
        req.chrome = chrome;
        req.carve = carve;
        self.provision(req, sink).await
    }

    /// The host mounts the daemon config's kits declare (`host-mount@1`), each
    /// with the host directory `kit_mounts` already gives it: what the create
    /// dialog offers a folder for. A kit that is not a local directory is not
    /// read, so its targets are not offered (a path typed in still works).
    pub fn declared_host_mounts(&self) -> Vec<serde_json::Value> {
        let cfg = self.daemon_config();
        let refs = crate::bundled::resolve(&self.store.paths, &cfg.kits).unwrap_or_default();
        let mut out = Vec::new();
        for (kit, dir) in cfg.kits.iter().zip(refs) {
            let Ok(spec) = std::fs::read_to_string(PathBuf::from(&dir).join("spec.yaml")) else { continue };
            for target in sbxw_core::kits::host_mount_targets(&spec) {
                let host = self.cfg.kit_mounts.get(&target).cloned().unwrap_or_default();
                out.push(serde_json::json!({ "kit": kit, "target": target, "host": host }));
            }
        }
        out
    }

    /// The automatic template as the create dialog sees it: `auto` off, or
    /// fresh, stale or missing for the daemon config's kits.
    pub fn template_status(&self) -> serde_json::Value {
        if self.cfg.template_mode() != templates::Mode::Auto {
            return serde_json::json!({ "auto": false });
        }
        let (sbx, sbxw) = self.template_versions();
        // As the bring-up applies them, sbxw's base kit first.
        let kits = sbxw_core::kits::with_base(&self.daemon_config().kits);
        let content = crate::bundled::fingerprint(&self.store.paths, &kits).unwrap_or_default();
        let now = templates::Current { sbx: &sbx, sbxw, content: &content };
        let state = templates::check(&self.store.templates(), &kits, &now);
        serde_json::json!({ "auto": true, "current": { "sbx": sbx, "sbxw": sbxw }, "template": state })
    }

    /// Every saved template, each marked with what sbxw makes of it: the one
    /// `template` in sbxw.toml names, and the automatic ones it keeps.
    pub async fn templates(&self) -> Result<serde_json::Value> {
        let list = self.sbx.template_ls().await?;
        let records = self.store.templates();
        let configured = match self.cfg.template_mode() {
            templates::Mode::Tag(t) => Some(t),
            _ => None,
        };
        let rows: Vec<serde_json::Value> = list
            .into_iter()
            .map(|t| {
                let auto = records.iter().find(|r| r.tag == t.tag);
                serde_json::json!({
                    "tag": t.tag,
                    "size": t.size,
                    "created": t.created,
                    "configured": configured.as_deref() == Some(t.tag.as_str()),
                    "auto": auto.map(|r| serde_json::json!({ "kits": r.kits, "sbx": r.sbx, "sbxw": r.sbxw, "created_at": r.created_at })),
                })
            })
            .collect();
        Ok(serde_json::json!({ "templates": rows, "configured": configured }))
    }

    /// Snapshot a sandbox as template `tag`, its secrets taken out first
    /// (see [`App::snapshot`]). A stopped sandbox is started for it.
    pub async fn save_template(&self, name: &SandboxRef, tag: &str) -> Result<Vec<String>> {
        let tag = tag.trim();
        if tag.is_empty() || tag.starts_with('-') || tag.contains(char::is_whitespace) {
            bail!("'{tag}' is not a template tag");
        }
        let sbx = self.sbx_for(name).await?;
        if !sbx.exists(&name.name).await {
            bail!("no sandbox named '{name}'");
        }
        let mut warnings = Vec::new();
        if !sbx.is_running(&name.name).await {
            sbx.run_detached(&name.name).await.context("starting the sandbox to take its secrets out")?;
            if !sbx.wait_until_running(&name.name, std::time::Duration::from_secs(60)).await {
                bail!("'{name}' did not come up — nothing was saved");
            }
            warnings.push(format!("'{name}' was started to take its secrets out before the snapshot"));
        }
        tracing::info!("web UI: saving '{name}' as template {tag}");
        let (saved, more) = self.snapshot(name, tag, self.cfg.keep_running).await;
        warnings.extend(more);
        saved?;
        Ok(warnings)
    }

    /// Remove a template, and sbxw's record of it if it was an automatic one:
    /// the next bring-up with those kits then finds none and says so.
    pub async fn remove_template(&self, tag: &str) -> Result<()> {
        tracing::info!("web UI: removing template {tag}");
        self.sbx.template_rm(tag).await?;
        let mut records = self.store.templates();
        let before = records.len();
        records.retain(|r| r.tag != tag);
        if records.len() != before {
            self.store.set_templates(&records).context("forgetting the automatic template")?;
        }
        Ok(())
    }

    /// What a bring-up asked from inside the daemon carries: its credentials
    /// come from the daemon's own environment.
    pub fn bring_up_request(&self, name: SandboxRef, workspace: PathBuf, config: Config, extra_ports: Vec<WantedPort>, chat: bool, consent: Consent) -> BringUp {
        BringUp {
            name,
            workspace,
            ro: vec![],
            rw: vec![],
            carve: vec![],
            config,
            extra_ports,
            chat,
            api_key: self
                .use_api_key
                .then(|| std::env::var("ANTHROPIC_API_KEY").ok())
                .flatten(),
            template: Default::default(),
            remote_control: false,
            chrome: false,
            git: None,
            consent,
        }
    }

    /// A chat's empty workspace is a folder on this machine: chats run here.
    pub fn prepare_chat_workspace(&self, name: &SandboxRef) -> Result<PathBuf> {
        if !name.is_local() {
            bail!("'{name}': chat sandboxes run on this machine only");
        }
        let dir = self.store.paths.chat_workspace(&name.name);
        std::fs::create_dir_all(&dir).with_context(|| format!("could not create chat workspace {}", dir.display()))?;
        Ok(dir)
    }

    /// A chat on the daemon's config, whose grants are checked like any
    /// file's, counting the lines in `approved`.
    pub async fn create_chat(self: &Arc<Self>, name: SandboxRef, approved: Vec<String>) -> Result<Vec<String>> {
        let ws = self.prepare_chat_workspace(&name)?;
        tracing::info!("provisioning chat sandbox '{name}' at {}", ws.display());
        let req = self.bring_up_request(name, ws, self.daemon_config(), vec![], true, Consent::Check(approved));
        self.provision(req, None).await
    }

    /// A new sandbox on the same host workspace (both see each other's edits).
    pub async fn duplicate(self: &Arc<Self>, source: &SandboxRef, new: SandboxRef) -> Result<Vec<String>> {
        let ws = self
            .store
            .workspace(source)
            .ok_or_else(|| anyhow!("no known workspace for '{source}' — it may predate this sbxw version"))?;
        if let Some(why) = crate::svc::jarvis_sandbox::reserved(&new) {
            bail!("{why}");
        }
        if new.host != source.host {
            bail!("'{new}' would be on another host than '{source}', whose workspace it shares");
        }
        if self.sbx_for(&new).await?.exists(&new.name).await {
            bail!("a sandbox named '{new}' already exists");
        }
        let cfg = self.config_for(source).await;
        tracing::info!("duplicating '{source}' as '{new}' (workspace {})", ws.display());
        // The source's recorded config, approved when it was brought up.
        let mut req = self.bring_up_request(new, ws, cfg, vec![], false, Consent::Checked(vec![]));
        req.carve = self.store.get(source).carve.map(|c| c.extra).unwrap_or_default();
        self.provision(req, None).await
    }

    /// Where a sandbox's carve helpers live on its host: the stub that is its
    /// primary folder when the workspace is split, and the empty folder laid
    /// over a soft carve. Not created here.
    pub async fn carve_dirs(&self, name: &SandboxRef) -> Result<(PathBuf, PathBuf)> {
        let base = if name.is_local() {
            self.store.paths.carve()
        } else {
            self.ready_remote(name).await?.root().await?.join("carve")
        };
        Ok((base.join(name.name.to_string()).join("stub"), base.join("empty")))
    }

    /// Remove a sandbox and create it again from what sbxw recorded: workspace,
    /// config, mounts and Remote Control choice. For one an older sbx made,
    /// which takes no kit until then. Anything outside the workspace mount is
    /// lost; the caller asked the person first. A chat is refused: its
    /// workspace goes with it.
    pub async fn recreate(self: &Arc<Self>, name: &SandboxRef) -> Result<Vec<String>> {
        let rec = self.store.get(name);
        if rec.chat {
            bail!("'{name}' is a chat: removing it deletes its workspace — start a new chat instead");
        }
        let ws = rec.workspace.clone().ok_or_else(|| anyhow!("sbxw doesn't know which workspace '{name}' was created on — remove it and create it again by hand"))?;
        let cfg = self.config_for(name).await;
        tracing::info!("recreating '{name}' on {}", ws.display());
        self.remove(std::slice::from_ref(name)).await?;
        let mut req = self.bring_up_request(name.clone(), ws, cfg, vec![], false, Consent::Checked(vec![]));
        for m in &rec.mounts {
            let p = m.path.to_string_lossy().into_owned();
            if m.readonly { req.ro.push(p) } else { req.rw.push(p) }
        }
        req.remote_control = rec.remote_control;
        req.chrome = rec.chrome;
        req.carve = rec.carve.map(|c| c.extra).unwrap_or_default();
        self.provision(req, None).await
    }

    /// The island's composer and the chat dock: type into a chat agent,
    /// creating the sandbox first only when the daemon picked the name.
    /// `session` is the dock's "New chat": another conversation in
    /// `ephemeral-chat` (a `chat-N` thread), the sandbox made only if missing.
    pub async fn chat_push(self: &Arc<Self>, text: &str, name: Option<SandboxRef>, fresh: bool, session: bool, thread: Option<names::ThreadName>) -> Result<(SandboxRef, bool, Option<String>), String> {
        let text = text.trim();
        if text.is_empty() {
            return Err("empty message".into());
        }
        let (name, thread) = if session {
            let name = SandboxRef::local(SandboxName::parse(names::EPHEMERAL_CHAT).expect("valid"));
            if !self.sessions.is_live(&SessionKey::agent(&name)) && !self.sbx.exists(&name.name).await {
                // No chat sandbox yet: its own agent is the first conversation.
                self.create_chat(name.clone(), vec![]).await.map_err(|e| format!("{e:#}"))?;
                self.push_as_person(&name, SessionKind::Agent, text).await?;
                return Ok((name, true, None));
            }
            let mut taken: Vec<String> = self.store.get(&name).threads.into_keys().collect();
            taken.extend(self.sessions.open_threads(&name.to_string()));
            (Some(name), Some(names::next_chat_session(taken.iter().map(String::as_str))))
        } else {
            (name, thread)
        };
        let (name, may_create) = match name {
            Some(n) => (n, false),
            None if fresh => {
                let taken: Vec<String> = self.sbx.ls().await.into_iter().map(|s| s.name).collect();
                (names::next_ephemeral_chat(taken.iter().map(String::as_str)).into(), true)
            }
            None => (SandboxRef::local(SandboxName::parse(names::EPHEMERAL_CHAT).expect("valid")), true),
        };
        if let Some(thread) = thread {
            if may_create {
                return Err("a thread is named together with its sandbox".into());
            }
            // A chat's conversations are opened from here; any other thread
            // is opened in sbxw, where its repo and model are chosen.
            if let Some(ws) = self.store.chat_workspace(&name) {
                git::init_chat_repo(&Host::local(), &ws).await?;
            } else if !self.sessions.is_live(&SessionKey::thread(&name, &thread)) {
                return Err(format!("thread '{thread}' of '{name}' is not running — open it in sbxw first"));
            }
            self.push_as_person(&name, SessionKind::Thread(thread.clone()), text).await?;
            return Ok((name, false, Some(thread.to_string())));
        }
        self.agents
            .lock()
            .unwrap()
            .can_type_into(&name.to_string())
            .map_err(|e| format!("'{name}': {e}"))?;
        // A live session is proof enough that the sandbox exists.
        let exists = self.sessions.is_live(&SessionKey::agent(&name))
            || self.sbx_for(&name).await.map_err(|e| format!("{e:#}"))?.exists(&name.name).await;
        if !exists {
            if !may_create {
                return Err(format!("no sandbox named '{name}' — it may have been removed"));
            }
            self.create_chat(name.clone(), vec![]).await.map_err(|e| format!("{e:#}"))?;
        }
        self.push_as_person(&name, SessionKind::Agent, text).await?;
        Ok((name, !exists, None))
    }

    pub async fn threads(&self, sandbox: &SandboxRef) -> ThreadsView {
        let rec = self.store.get(sandbox);
        let recorded: BTreeMap<String, String> = rec.threads.iter().map(|(k, v)| (k.clone(), v.repo.clone())).collect();
        let found = match &rec.workspace {
            Some(ws) => match self.host_for(sandbox).await {
                Ok(host) => git::thread_worktrees(&host, ws, &recorded).await,
                Err(_) => vec![],
            },
            None => vec![],
        };
        // A shared plan's worktree is not a thread; its threads have no
        // worktree of their own, so they are listed from the plan.
        let (teams, team_threads) = {
            let book = self.plans.lock().unwrap();
            let plans = book.of(&sandbox.to_string());
            let teams: std::collections::HashSet<String> = plans.iter().map(|p| p.team_name()).collect();
            let members: Vec<String> = plans
                .iter()
                .flat_map(|p| p.threads.iter().map(|t| t.name.clone()))
                .filter(|t| book.team_of(&sandbox.to_string(), t).is_some())
                .collect();
            (teams, members)
        };
        let mut threads: BTreeMap<String, (bool, String)> =
            found.into_iter().filter(|(t, _)| !teams.contains(t)).map(|(t, r)| (t, (false, r))).collect();
        let team_members: std::collections::HashSet<String> = team_threads.iter().cloned().collect();
        for t in team_threads {
            let repo = recorded.get(&t).cloned().unwrap_or_default();
            threads.entry(t).or_insert((false, repo));
        }
        // Sessions in the main checkout have no worktree to be found by: they
        // are listed from their record, running or not, until closed.
        for (t, r) in rec.threads.iter().filter(|(_, r)| r.same_worktree) {
            threads.entry(t.clone()).or_insert((false, r.repo.clone()));
        }
        for t in self.sessions.open_threads(&sandbox.to_string()) {
            let repo = recorded.get(&t).cloned().unwrap_or_default();
            threads.entry(t).or_insert((false, repo)).0 = true;
        }
        ThreadsView {
            threads: threads
                .into_iter()
                .map(|(name, (open, repo))| {
                    let model = rec.threads.get(&name).and_then(|r| r.model.clone());
                    // Where it works: the main checkout (`same`), a shared
                    // plan's one worktree (`team`), or a worktree of its own.
                    let worktree = if rec.threads.get(&name).is_some_and(|r| r.same_worktree) {
                        "same"
                    } else if team_members.contains(&name) {
                        "team"
                    } else {
                        "own"
                    };
                    serde_json::json!({ "name": name, "open": open, "repo": repo, "model": model, "worktree": worktree })
                })
                .collect(),
            open: self.sessions.agents_open(&sandbox.to_string()),
            max: self.config_for(sandbox).await.max_sessions(),
        }
    }

    pub async fn worktree_graph(&self, sandbox: &SandboxRef) -> Result<(PathBuf, Vec<git::RepoBranches>), String> {
        let ws = self.store.workspace(sandbox).ok_or_else(|| format!("sbxw has no workspace recorded for '{sandbox}'"))?;
        let rec = self.store.get(sandbox);
        let recorded: BTreeMap<String, String> = rec.threads.iter().map(|(k, v)| (k.clone(), v.repo.clone())).collect();
        let host = self.host_for(sandbox).await.map_err(|e| format!("{e:#}"))?;
        let threads = git::thread_worktrees(&host, &ws, &recorded).await;
        Ok((ws.clone(), git::graph(&host, &ws, &threads).await))
    }

    pub async fn policy_panel(&self, sandbox: &SandboxRef) -> PolicyPanel {
        let cfg = self.config_for(sandbox).await;
        let sbx = match self.sbx_for(sandbox).await {
            Ok(s) => s,
            Err(e) => {
                let none = || PolicyView { error: Some(format!("{e:#}")), ..Default::default() };
                return PolicyPanel {
                    ok: false,
                    policies: none(),
                    rules: none(),
                    log: none(),
                    blocked: vec![],
                    configured_allow: cfg.network_allow,
                    configured_deny: cfg.network_deny,
                    error: Some(format!("{e:#}")),
                };
            }
        };
        let (policies, rules, log) = tokio::join!(
            sbx.policy_ls(&sandbox.name, false),
            sbx.policy_ls(&sandbox.name, true),
            sbx.policy_log(&sandbox.name)
        );
        let (log, egress) = log;
        let ok = policies.error.is_none() || rules.error.is_none() || log.error.is_none();
        let error = (!ok).then(|| policies.error.clone()).flatten();
        PolicyPanel {
            ok,
            policies,
            rules,
            log,
            blocked: egress.map(|e| e.blocked_hosts()).unwrap_or_default(),
            configured_allow: cfg.network_allow,
            configured_deny: cfg.network_deny,
            error,
        }
    }

    /// Publish one mapping from the ports dialog; an alias goes through the
    /// same parking rule as a bring-up's. With no host port, sbx 0.54 picks
    /// one on 127.0.0.1 ([`Sbx::port_publish`](sbxw_sbx::Sbx::port_publish));
    /// another host IP without a host port is refused before anything runs;
    /// an older sbx gets today's default, the sandbox port, and the answer
    /// says so.
    pub async fn publish_port(self: &Arc<Self>, name: &SandboxRef, sandbox_port: u16, host_port: Option<u16>, host_ip: Option<String>, alias: Option<String>) -> Result<Published> {
        let ip = host_ip.map(|i| i.trim().to_string()).filter(|i| !i.is_empty()).unwrap_or_else(|| sbxw_sbx::AUTO_PORT_IP.into());
        // sbx assigns a host port only on 127.0.0.1: refused before anything runs.
        if host_port.is_none() && ip != sbxw_sbx::AUTO_PORT_IP {
            bail!("{}", sbxw_sbx::AUTO_PORT_LOOPBACK_ONLY);
        }
        let alias = alias.map(|a| a.trim().to_string()).filter(|a| !a.is_empty());
        if let Some(a) = &alias {
            if !sbxw_core::hosts::is_hostname(a) {
                bail!("'{a}' is not a hostname");
            }
        }
        let sbx = self.sbx_for(name).await?;
        // Before 0.54 there is no automatic port: today's default, said.
        let (host_port, note) = match host_port {
            None if !sbx.has_port_command().await => (Some(sandbox_port), Some(AUTO_PORT_NEEDS_054.to_string())),
            hp => (hp, None),
        };
        let assigned = host_port.is_none();
        if !name.is_local() {
            // On the host's loopback, reached here through the link.
            let m = sbx.port_publish(&name.name, "127.0.0.1", host_port, sandbox_port).await?;
            let forwarded_port = self.forward_port(name, &m.spec()).await;
            let hosts_warning = alias.map(|_| format!("'{name}' is on another host: its port is reached as {}.{}.localhost, not by an alias", name.name, name.host));
            return Ok(Published { sandbox_port, host_ip: m.host_ip, host_port: m.host_port, assigned, note, forwarded_port, hosts_warning });
        }
        let app = self.clone();
        let ip2 = ip.clone();
        tokio::task::spawn_blocking(move || app.hosts.ensure_loopback(&[HostAlias::new("", ip2)]))
            .await?
            .context("failed to create the loopback alias — run: sudo ifconfig lo0 alias <ip> up")?;
        let m = sbx.port_publish(&name.name, &ip, host_port, sandbox_port).await?;
        let mut out = Published { sandbox_port, host_ip: m.host_ip, host_port: m.host_port, assigned, note, forwarded_port: None, hosts_warning: None };
        let Some(alias) = alias else { return Ok(out) };
        let app = self.clone();
        let w = tokio::task::spawn_blocking(move || app.hosts.apply(&app.store, &[HostAlias::new(alias, ip)])).await?;
        out.hosts_warning = w.into_iter().next();
        Ok(out)
    }

    /// Which rule lets `sandbox` reach `target` (`host` or `host:port`,
    /// checked first): `sbx policy check network` (0.54). Before 0.54,
    /// `supported: false` and a note, never an error. `rule_id` is the rule
    /// again when the sandbox's `policy ls --wide` lists it as a rule id, the
    /// one `policy rm` takes.
    pub async fn policy_check(&self, sandbox: &SandboxRef, target: &str) -> Result<PolicyCheckAnswer, CheckRefused> {
        let target = check_target(target).map_err(CheckRefused::Bad)?;
        let sbx = self.sbx_for(sandbox).await.map_err(CheckRefused::Failed)?;
        if !sbx.known_version().await.is_some_and(|v| v >= sbxw_sbx::POLICY_CHECK_SINCE) {
            return Ok(PolicyCheckAnswer { supported: false, target, note: Some(POLICY_CHECK_NEEDS_054.into()), ..Default::default() });
        }
        let c = sbx.policy_check(Some(&sandbox.name), &target).await.map_err(CheckRefused::Failed)?;
        let rule_id = match &c.rule {
            Some(rule) => {
                let rules = sbx.policy_ls(&sandbox.name, true).await;
                rule_ids(&rules).into_iter().find(|id| id == rule)
            }
            None => None,
        };
        Ok(PolicyCheckAnswer { supported: true, target, allowed: c.allowed, rule: c.rule, rule_id, raw: c.raw, note: None })
    }

    /// Must `sbx setup ssh` run again (sbx 0.54 asks once after the
    /// upgrade)? Only where `~/.ssh/config` has the `*.sbx` block: a person
    /// who never set SSH up is not reminded of it.
    pub async fn ssh_status(&self) -> SshStatus {
        let sbx = self.sbx.known_version().await.map(|(a, b, c)| format!("{a}.{b}.{c}")).or_else(|| self.versions.sbx.clone());
        // `~/.sbxw`'s parent: the home directory (a test's own directory under test).
        let configured = ssh_configured(self.store.paths.root.parent());
        let refreshed_at = self.store.ssh_refreshed_at();
        let due = sbx.as_deref().is_some_and(|now| sbxw_sbx::ssh_refresh_due(refreshed_at.as_deref(), now));
        SshStatus { configured, refresh_due: configured && due, sbx, refreshed_at }
    }

    /// `sbx setup ssh` on the person's click, and the sbx version it ran
    /// with recorded, which ends the reminder.
    pub async fn ssh_setup(&self) -> Result<SshStatus> {
        tracing::info!("web UI: running `sbx setup ssh`");
        self.sbx.setup_ssh().await?;
        if let Some(v) = self.sbx.known_version().await.map(|(a, b, c)| format!("{a}.{b}.{c}")).or_else(|| self.versions.sbx.clone()) {
            self.store.set_ssh_refreshed_at(&v)?;
        }
        Ok(self.ssh_status().await)
    }

    /// A sandbox's MCP mode and connected servers, from `sbx inspect` (sbx
    /// 0.54, local sandboxes). Whatever sbx does not say is `unknown`.
    pub async fn sandbox_mcp(&self, sandbox: &SandboxRef) -> Result<SandboxMcp> {
        if !sandbox.is_local() {
            return Ok(SandboxMcp::unsupported(MCP_LOCAL_ONLY));
        }
        let i = self.sbx.inspect(&sandbox.name).await?;
        Ok(match i.mcp {
            Some(m) => SandboxMcp { supported: true, mode: m.mode, servers: m.servers, note: None },
            None => SandboxMcp::unsupported(MCP_NEEDS_054),
        })
    }

    /// The env file for an existing sandbox — the ports it *has*, the aliases
    /// its config gives them.
    pub async fn render_env_file(&self, name: &SandboxRef, dir: Option<&Path>) -> Result<RenderedEnvFile> {
        let ws = self.store.workspace(name).ok_or_else(|| {
            anyhow!("sbxw doesn't know which workspace '{name}' was created on — bring it up once with sbxw")
        })?;
        let mut cfg = self.config_for(name).await;
        // sbx knows no `sbxw:` kit: the file names the written-out directory,
        // sbxw's base kit first, as the bring-up applies it.
        cfg.kits = crate::bundled::resolve(&self.store.paths, &sbxw_core::kits::with_base(&cfg.kits))?;
        let live = self.sbx_for(name).await?.ports(&name.name).await;
        let ports: Vec<envfile::Port> = if live.is_empty() {
            envexport::ports_from_config(&cfg)
        } else {
            live.iter().map(envfile::Port::from).collect()
        };
        let dir = envexport::export_dir_for(&ws, dir);
        let file = envexport::env_file_for(&name.to_string(), &cfg, &ws, ports, &dir);
        Ok(RenderedEnvFile {
            yaml: file.render(&format!("sbxw {}", self.versions.sbxw)),
            dest: dir.join(envfile::FILE_NAME),
            dir,
        })
    }
}

pub const AUTO_PORT_NEEDS_054: &str = "automatic host port needs sbx 0.54: published on the sandbox port";
pub const POLICY_CHECK_NEEDS_054: &str = "needs sbx 0.54: `sbx policy check` is new there";
pub const MCP_NEEDS_054: &str = "needs sbx 0.54";
pub const MCP_LOCAL_ONLY: &str = "only shown for sandboxes on this Mac";

/// `GET /api/ssh`'s answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SshStatus {
    /// `~/.ssh/config` has a `*.sbx` block (`sbx setup ssh`'s).
    pub configured: bool,
    pub refresh_due: bool,
    /// The running sbx's version.
    pub sbx: Option<String>,
    /// The sbx version sbxw last ran `sbx setup ssh` with.
    pub refreshed_at: Option<String>,
}

/// Is a `*.sbx` host block in `<home>/.ssh/config`?
pub fn ssh_configured(home: Option<&Path>) -> bool {
    home.and_then(|h| std::fs::read_to_string(h.join(".ssh/config")).ok()).is_some_and(|s| s.contains(".sbx"))
}

/// What publishing one port did (`POST .../ports/publish`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Published {
    pub sandbox_port: u16,
    pub host_ip: String,
    pub host_port: u16,
    /// sbx picked `host_port` (0.54, none asked for).
    pub assigned: bool,
    /// Why no automatic port was given, on an older sbx.
    pub note: Option<String>,
    /// A remote sandbox's: the local end of the forward to its host port.
    pub forwarded_port: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hosts_warning: Option<String>,
}

/// `GET .../policy/check`'s answer. Every string is sbx's: text, not markup.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct PolicyCheckAnswer {
    pub supported: bool,
    pub target: String,
    pub allowed: Option<bool>,
    pub rule: Option<String>,
    pub rule_id: Option<String>,
    pub raw: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// Why a policy check was not run: a target that is not one (the caller's),
/// or sbx failing it.
#[derive(Debug)]
pub enum CheckRefused {
    Bad(String),
    Failed(anyhow::Error),
}

/// `host` or `host:port` (1..65535), the host checked as a host name:
/// nothing else reaches a command line. Normalised (lower case).
pub fn check_target(raw: &str) -> Result<String, String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err("not a target: give a host, or host:port".into());
    }
    let (host, port) = match raw.rsplit_once(':') {
        Some((h, p)) => match p.parse::<u16>() {
            Ok(p) if p != 0 => (h, Some(p)),
            _ => return Err(format!("not a target: '{raw}' has no valid port")),
        },
        None => (raw, None),
    };
    let host = sbxw_core::policy::clean_host(host).map_err(|e| format!("not a target: {e}"))?;
    Ok(port.map_or(host.clone(), |p| format!("{host}:{p}")))
}

/// The rule ids a `policy ls --wide` listing shows.
fn rule_ids(v: &PolicyView) -> Vec<String> {
    let Some(i) = v.roles.iter().position(|r| *r == "rule_id") else { return vec![] };
    v.rows.iter().filter_map(|r| r.get(i)).map(|id| id.trim().to_string()).filter(|id| !id.is_empty() && id != "-").collect()
}

/// `GET /api/sandboxes/:name/mcp`'s answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SandboxMcp {
    pub supported: bool,
    /// `static`, `dynamic` or `unknown`.
    pub mode: String,
    pub servers: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

impl SandboxMcp {
    fn unsupported(note: &str) -> Self {
        Self { supported: false, mode: "unknown".into(), servers: vec![], note: Some(note.into()) }
    }
}

/// `sbx ls`, failing when the host cannot be asked instead of answering
/// with an empty list.
pub async fn list_strict(sbx: &sbxw_sbx::Sbx) -> Result<Vec<sbxw_sbx::SandboxInfo>> {
    let args: Vec<String> = ["ls", "--format", "json"].map(String::from).into();
    let out = sbx.raw(sbxw_sbx::runner::Invocation::new(args.clone())).await?;
    if !out.success() {
        return Err(sbxw_sbx::command_error(&args, &out));
    }
    Ok(sbxw_sbx::parse_ls_json(&out.stdout_str()).unwrap_or_default())
}

/// The repos to mount beside `workspace`, split `(read-only, writable)`. Each
/// is canonical (sbx mounts it at the same path inside) and apart from every
/// other: sbx shares a directory once, so a repo inside the workspace, inside
/// another repo, or around one is reached through that mount with its mode, and
/// a `:ro` on it is silently writable.
async fn extra_mounts(host: &Host, workspace: &std::path::Path, mounts: Vec<(String, bool)>) -> Result<(Vec<String>, Vec<String>)> {
    let workspace = host.fs.canonicalize(workspace).await.with_context(|| format!("workspace path does not exist: {}", workspace.display()))?;
    let mut seen = vec![workspace];
    let (mut ro, mut rw) = (vec![], vec![]);
    for (path, readonly) in mounts {
        let shown = path.trim();
        let dir = match host.fs.canonicalize(std::path::Path::new(shown)).await {
            Ok(d) if matches!(host.fs.stat(&d).await, Ok(Some(m)) if m.kind == crate::host::Kind::Dir) => d,
            _ => bail!("{shown} is not a directory"),
        };
        let s = dir.to_string_lossy().into_owned();
        if s.contains(':') || s.contains(',') {
            bail!("{shown}: sbx cannot mount a path containing ':' or ','");
        }
        if let Some(other) = seen.iter().find(|o| dir.starts_with(o) || o.starts_with(&dir)) {
            if *other == dir {
                bail!("{shown} is already mounted in this sandbox");
            }
            let other = other.display();
            bail!(
                "{shown} and {other} are nested: sbx mounts only the outer one, so the inner one gets its mode — \
                 pick sibling folders (or make the outer one the only repo)"
            );
        }
        seen.push(dir);
        if readonly {
            ro.push(s);
        } else {
            rw.push(s);
        }
    }
    Ok((ro, rw))
}

/// `CLAUDE_CODE_OAUTH_TOKEN`, or its legacy alias, when set and non-empty.
pub fn oauth_token_from_env() -> Option<String> {
    ["CLAUDE_CODE_OAUTH_TOKEN", "CLAUDE_OAUTH_TOKEN"]
        .iter()
        .filter_map(|v| std::env::var(v).ok())
        .map(|v| v.trim().to_string())
        .find(|v| !v.is_empty())
}

impl App {
    /// The one writer of uploaded files into a sandbox (the web UI's upload
    /// box, the web chat's attachments, the phone's files): `bytes` under
    /// `/tmp/sbxw-uploads/<at_ms>/<name>`, `name` made safe by
    /// [`sbxw_core::uploads::upload_name`]. The path written.
    pub async fn write_upload(&self, sb: &sbxw_core::SandboxRef, at_ms: u64, name: &str, bytes: &[u8]) -> Result<String> {
        let dest = sbxw_core::uploads::dest(at_ms, &sbxw_core::uploads::upload_name(name));
        self.sbx_for(sb).await?.write_file(&sb.name, &dest, bytes, false).await?;
        Ok(dest)
    }
}

#[cfg(test)]
mod tests {
    use crate::testing::{app_with, sbx_answer};
    use sbxw_core::SandboxRef;
    use sbxw_sbx::runner::Output;

    #[tokio::test]
    async fn a_scratch_sandbox_of_the_daemon_is_left_out_of_the_list_while_it_lives() {
        let (_d, app, _fake) = app_with(|a: &[String]| match a.first().map(String::as_str) {
            Some("ls") => crate::testing::ls(&[("mine", "running"), ("sbxw-tpl-65efab12", "stopped")]),
            _ => sbx_answer(a),
        });
        let names = |items: Vec<super::SandboxItem>| items.into_iter().map(|i| i.name).collect::<Vec<_>>();
        // One a stopped daemon left behind shows, so it can be removed.
        assert_eq!(names(app.sandboxes().await), ["mine", "sbxw-tpl-65efab12"]);
        app.scratches.lock().unwrap().insert(SandboxRef::parse("sbxw-tpl-65efab12").unwrap());
        assert_eq!(names(app.sandboxes().await), ["mine"]);
    }

    #[tokio::test]
    async fn which_sbx_made_a_sandbox_is_asked_once_and_an_old_one_is_marked() {
        let (_d, app, fake) = app_with(|a: &[String]| match a.first().map(String::as_str) {
            Some("ls") => crate::testing::ls(&[("old", "running"), ("new", "running"), ("theirs", "running")]),
            Some("inspect") if a[1] == "new" => Output::ok(r#"{"name":"new","kits":[],"created_version":"v0.51.0"}"#),
            Some("inspect") => Output::ok(r#"{"name":"old","kits":["/k/codemap"]}"#),
            _ => sbx_answer(a),
        });
        for n in ["old", "new"] {
            app.store.update(SandboxRef::parse(n).unwrap(), |r| r.workspace = Some("/w".into())).unwrap();
        }
        let first = app.sandboxes().await;
        let old = |items: &[super::SandboxItem], n: &str| items.iter().find(|i| i.name == n).unwrap().old_sbx;
        assert!(old(&first, "old") && !old(&first, "new"));
        assert!(!old(&first, "theirs"), "a sandbox sbxw has no record of is not asked");
        app.sandboxes().await;
        assert_eq!(fake.log().iter().filter(|c| c.starts_with("inspect")).count(), 2, "once each: {:?}", fake.log());
    }

    #[tokio::test]
    async fn a_chat_is_never_recreated() {
        let (_d, app, fake) = app_with(sbx_answer);
        let n = SandboxRef::parse("chat-1").unwrap();
        app.store.update(&n, |r| {
            r.chat = true;
            r.workspace = Some("/w/chat-1".into());
        })
        .unwrap();
        let e = app.recreate(&n).await.unwrap_err();
        assert!(format!("{e:#}").contains("chat"), "{e:#}");
        assert!(fake.log().iter().all(|c| !c.starts_with("rm")), "nothing removed");
    }

    #[tokio::test]
    async fn the_policy_panel_lists_blocked_hosts_from_the_json_log() {
        let (_d, app, _fake) = app_with(|a: &[String]| {
            if a.starts_with(&["policy".into(), "log".into()]) && a.contains(&"json".to_string()) {
                return Output::ok(
                    r#"{"updated_at":"x","allowed":[],"blocked":[{"host":"registry.npmjs.org:443","rule":"default-deny-all","reason":"no matching allow rule","count":2,"last_seen":"2026-10-26T09:00:00Z"}]}"#,
                );
            }
            sbx_answer(a)
        });
        let p = app.policy_panel(&SandboxRef::parse("myapp").unwrap()).await;
        assert_eq!(p.blocked.len(), 1);
        assert_eq!(p.blocked[0].resource, "registry.npmjs.org");
        assert_eq!(p.log.rows.len(), 1, "the log view is built from the same document");
    }

    #[tokio::test]
    async fn a_partial_removal_forgets_only_what_went() {
        let (_d, app, _fake) = app_with(|a: &[String]| match a.first().map(String::as_str) {
            Some("rm") => Output {
                code: Some(1),
                stdout: br#"[{"action":"removed","name":"a"},{"action":"failed","name":"b","error":"busy"}]"#.to_vec(),
                stderr: vec![],
            },
            _ => sbx_answer(a),
        });
        let (a, b) = (SandboxRef::parse("a").unwrap(), SandboxRef::parse("b").unwrap());
        for n in [&a, &b] {
            app.store.update(n, |r| r.chat = false).unwrap();
        }
        let e = app.remove(&[a.clone(), b.clone()]).await.unwrap_err();
        assert!(format!("{e:#}").contains("'b': busy"), "{e:#}");
        assert!(!app.store.exists(sbxw_core::SandboxRef::parse("a").unwrap()) && app.store.exists(sbxw_core::SandboxRef::parse("b").unwrap()));
    }

    fn on(version: &'static str, rest: impl Fn(&[String]) -> Option<Output> + Send + Sync + 'static) -> impl Fn(&[String]) -> Output + Send + Sync + 'static {
        move |a: &[String]| match a.first().map(String::as_str) {
            Some("--version") => Output::ok(&format!("sbx version {version}\n")),
            _ => rest(a).unwrap_or_else(|| sbx_answer(a)),
        }
    }

    #[tokio::test]
    async fn a_port_with_no_host_port_is_sbx_s_pick_on_0_54_and_the_sandbox_port_before() {
        // What sbx 0.54 printed on the person's Mac, verbatim.
        let said = r#"[{"action":"published","host_ip":"127.0.0.1","host_port":49152,"sandbox_port":3000,"protocol":"tcp4"}]"#;
        let (_d, app, fake) = app_with(on("0.54.0", move |a| (a[0] == "port" && a[1] == "publish").then(|| Output::ok(said))));
        let web = SandboxRef::parse("web").unwrap();
        let p = app.publish_port(&web, 3000, None, None, None).await.unwrap();
        assert_eq!((p.host_ip.as_str(), p.host_port, p.assigned, p.note.as_deref()), ("127.0.0.1", 49152, true, None));
        assert!(fake.log().iter().any(|c| c == "port publish web 3000 --format json"), "{:?}", fake.log());
        let fixed = app.publish_port(&web, 3000, Some(8080), None, None).await.unwrap();
        assert_eq!((fixed.host_port, fixed.assigned), (8080, false));
        assert!(fake.log().iter().any(|c| c == "port publish web 127.0.0.1:8080:3000"), "{:?}", fake.log());
        let fixed = app.publish_port(&web, 3000, Some(8081), Some("127.0.0.2".into()), None).await.unwrap();
        assert_eq!((fixed.host_ip.as_str(), fixed.host_port), ("127.0.0.2", 8081));
        assert!(fake.log().iter().any(|c| c == "port publish web 127.0.0.2:8081:3000"), "{:?}", fake.log());

        // Another host IP with no host port: refused, nothing run.
        let ran = fake.log().len();
        let e = app.publish_port(&web, 3000, None, Some("127.0.0.2".into()), None).await.unwrap_err();
        assert_eq!(format!("{e:#}"), sbxw_sbx::AUTO_PORT_LOOPBACK_ONLY);
        assert_eq!(fake.log().len(), ran, "{:?}", fake.log());

        let (_d, app, fake) = app_with(on("0.53.1", |_| None));
        let p = app.publish_port(&web, 3000, None, None, None).await.unwrap();
        assert_eq!((p.host_port, p.assigned), (3000, false), "today's default");
        assert_eq!(p.note.as_deref(), Some(super::AUTO_PORT_NEEDS_054));
        assert!(!fake.log().iter().any(|c| c.contains("::")), "no automatic spec is sent before 0.54: {:?}", fake.log());
    }

    #[test]
    fn a_policy_check_target_is_a_host_and_maybe_a_port_or_nothing_runs() {
        assert_eq!(super::check_target(" GitHub.com:443 ").unwrap(), "github.com:443");
        assert_eq!(super::check_target("pypi.org").unwrap(), "pypi.org");
        for bad in ["", "-rf", "a.com:0", "a.com:99999", "a.com:x", "a.com;rm -rf /", "--sandbox=x", "a b.com", "localhost:80", "10.0.0.1"] {
            assert!(super::check_target(bad).is_err(), "{bad}");
        }
    }

    #[tokio::test]
    async fn a_policy_check_names_the_rule_and_its_id_on_0_54_only() {
        let (_d, app, fake) = app_with(on("0.54.0", |a| match (a[0].as_str(), a.get(1).map(String::as_str)) {
            ("policy", Some("check")) => Some(Output::ok("ALLOWED pypi.org:443 by rule b-12 (allow network pypi.org)\n")),
            ("policy", Some("ls")) => Some(Output::ok("RULE ID   DECISION   RESOURCE\nb-12      allow      pypi.org\n")),
            _ => None,
        }));
        let web = SandboxRef::parse("web").unwrap();
        let c = app.policy_check(&web, "pypi.org:443").await.unwrap();
        assert!(c.supported);
        assert_eq!(c.rule.as_deref(), Some("b-12"));
        assert_eq!(c.rule_id.as_deref(), Some("b-12"), "removable through policy rm");
        assert!(c.raw.contains("ALLOWED"));
        assert!(fake.log().iter().any(|l| l == "policy check network --sandbox web pypi.org:443"), "{:?}", fake.log());
        assert!(matches!(app.policy_check(&web, "x.com; rm").await, Err(super::CheckRefused::Bad(_))));

        let (_d, app, fake) = app_with(on("0.53.0", |_| None));
        let c = app.policy_check(&web, "pypi.org").await.unwrap();
        assert!(!c.supported && c.note.is_some());
        assert!(!fake.log().iter().any(|l| l.starts_with("policy check")));
    }

    #[tokio::test]
    async fn the_mcp_mode_is_sbx_s_word_or_unknown_never_an_error() {
        let (_d, app, _f) = app_with(|a: &[String]| match a.first().map(String::as_str) {
            Some("inspect") if a[1] == "new" => Output::ok(r#"{"name":"new","kits":[],"mcp":{"mode":"dynamic","servers":["github",{"name":"notion"}]}}"#),
            Some("inspect") => Output::ok(r#"{"name":"old","kits":[]}"#),
            _ => sbx_answer(a),
        });
        let m = app.sandbox_mcp(&SandboxRef::parse("new").unwrap()).await.unwrap();
        assert!(m.supported);
        assert_eq!((m.mode.as_str(), m.servers.as_slice()), ("dynamic", &["github".to_string(), "notion".to_string()][..]));
        let m = app.sandbox_mcp(&SandboxRef::parse("old").unwrap()).await.unwrap();
        assert!(!m.supported);
        assert_eq!((m.mode.as_str(), m.note.as_deref()), ("unknown", Some(super::MCP_NEEDS_054)));
    }

    #[tokio::test]
    async fn the_ssh_reminder_shows_on_0_54_where_ssh_is_set_up_until_setup_runs() {
        let (d, app, fake) = app_with(on("0.54.0", |_| None));
        let s = app.ssh_status().await;
        assert!(!s.configured && !s.refresh_due, "no *.sbx block, no reminder");
        // `Paths::under(dir)`: the test's directory stands for the home.
        let home = app.store.paths.root.parent().unwrap().to_path_buf();
        assert!(home.starts_with(d.path()));
        std::fs::create_dir_all(home.join(".ssh")).unwrap();
        std::fs::write(home.join(".ssh/config"), "Host *.sbx\n  ProxyCommand sbx ssh-proxy %h\n").unwrap();
        assert!(app.ssh_status().await.refresh_due);
        let after = app.ssh_setup().await.unwrap();
        assert!(fake.log().iter().any(|l| l == "setup ssh"));
        assert_eq!((after.refresh_due, after.refreshed_at.as_deref()), (false, Some("0.54.0")));

        let (d2, old, _f) = app_with(on("0.53.0", |_| None));
        let home = old.store.paths.root.parent().unwrap().to_path_buf();
        assert!(home.starts_with(d2.path()));
        std::fs::create_dir_all(home.join(".ssh")).unwrap();
        std::fs::write(home.join(".ssh/config"), "Host *.sbx\n").unwrap();
        assert!(!old.ssh_status().await.refresh_due, "an older sbx asks for nothing");
    }
}
