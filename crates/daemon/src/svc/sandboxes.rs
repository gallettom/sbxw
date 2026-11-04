//! Sandboxes as the UI and the CLI act on them: listing, removal with its
//! cleanup, duplicate, chats, threads, ports, policy, the env file export.

use crate::app::App;
use crate::git;
use crate::host::Host;
use crate::provision::BringUp;
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
    /// On a remote host that cannot be reached since then (unix seconds):
    /// `status` is the last one seen, and actions are refused.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unreachable_since: Option<u64>,
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
    /// Host folders for kit-declared host mounts, over `kit_mounts`.
    pub kit_mounts: std::collections::BTreeMap<String, String>,
}

impl App {
    pub async fn sandboxes(&self) -> Vec<SandboxItem> {
        self.sbx
            .ls()
            .await
            .into_iter()
            .map(|s| {
                // sbx lists this machine's sandboxes; one whose name sbxw would
                // refuse has no record to look up.
                let rec = SandboxName::parse(&s.name).map(|n| self.store.get(&n)).unwrap_or_default();
                let chat = SandboxName::parse(&s.name).is_ok_and(|n| self.store.chat_workspace(&n).is_some());
                (s, rec, chat)
            })
            .map(|(s, rec, chat)| SandboxItem {
                chat,
                remote_control: rec.remote_control,
                workspace: rec.workspace.map(|w| w.to_string_lossy().into_owned()),
                name: s.name,
                agent: s.agent,
                status: s.status,
                created_at: s.created_at,
                detached: s.detached,
                unreachable_since: None,
            })
            .collect::<Vec<_>>()
            .into_iter()
            .chain(self.remote_sandboxes().await)
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
                            unreachable_since: None,
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
                            unreachable_since: Some(since),
                        });
                    }
                }
            }
        }
        out
    }

    /// Remove sandboxes and everything sbxw keeps about them: their sessions,
    /// record and kit, and — a chat's — its throwaway workspace. Only what sbx
    /// reports removed is forgotten; the rest is the error. Each host's are
    /// removed by that host's sbx.
    pub async fn remove(&self, names: &[SandboxRef]) -> Result<()> {
        let mut by_host: BTreeMap<sbxw_core::names::HostName, Vec<SandboxRef>> = BTreeMap::new();
        for n in names {
            by_host.entry(n.host.clone()).or_default().push(n.clone());
        }
        let mut failed: Vec<String> = Vec::new();
        let mut chats: Vec<PathBuf> = Vec::new();
        for refs in by_host.values() {
            let sbx = self.sbx_for(&refs[0]).await?;
            let bare: Vec<SandboxName> = refs.iter().map(|r| r.name.clone()).collect();
            let results = sbx.rm(&bare).await?;
            let gone: Vec<&SandboxRef> = refs.iter().filter(|n| results.iter().any(|r| r.removed && r.name == n.name.as_str())).collect();
            chats.extend(gone.iter().filter_map(|n| self.store.chat_workspace(*n)));
            for n in &gone {
                self.sessions.drop_sandbox(&n.to_string());
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
        let CreateForm { mounts, ports: extra, template, remote_control, kit_mounts } = form;
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
        cfg.kit_mounts.extend(kit_mounts.into_iter().filter(|(_, host)| !host.trim().is_empty()));
        cfg.validate()?;
        let mut req = self.bring_up_request(name, path, cfg, extra, false);
        req.ro = ro;
        req.rw = rw;
        req.template = template;
        req.remote_control = remote_control;
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
        let kits = self.daemon_config().kits;
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
    pub fn bring_up_request(&self, name: SandboxRef, workspace: PathBuf, config: Config, extra_ports: Vec<WantedPort>, chat: bool) -> BringUp {
        BringUp {
            name,
            workspace,
            ro: vec![],
            rw: vec![],
            config,
            extra_ports,
            chat,
            api_key: self
                .use_api_key
                .then(|| std::env::var("ANTHROPIC_API_KEY").ok())
                .flatten(),
            oauth_token: oauth_token_from_env(),
            template: Default::default(),
            remote_control: false,
            git: None,
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

    pub async fn create_chat(self: &Arc<Self>, name: SandboxRef) -> Result<Vec<String>> {
        let ws = self.prepare_chat_workspace(&name)?;
        tracing::info!("provisioning chat sandbox '{name}' at {}", ws.display());
        let req = self.bring_up_request(name, ws, self.daemon_config(), vec![], true);
        self.provision(req, None).await
    }

    /// A new sandbox on the same host workspace (both see each other's edits).
    pub async fn duplicate(self: &Arc<Self>, source: &SandboxRef, new: SandboxRef) -> Result<Vec<String>> {
        let ws = self
            .store
            .workspace(source)
            .ok_or_else(|| anyhow!("no known workspace for '{source}' — it may predate this sbxw version"))?;
        if new.host != source.host {
            bail!("'{new}' would be on another host than '{source}', whose workspace it shares");
        }
        if self.sbx_for(&new).await?.exists(&new.name).await {
            bail!("a sandbox named '{new}' already exists");
        }
        let cfg = self.config_for(source);
        tracing::info!("duplicating '{source}' as '{new}' (workspace {})", ws.display());
        let req = self.bring_up_request(new, ws, cfg, vec![], false);
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
                self.create_chat(name.clone()).await.map_err(|e| format!("{e:#}"))?;
                self.push_text(&name, SessionKind::Agent, text).await?;
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
            self.push_text(&name, SessionKind::Thread(thread.clone()), text).await?;
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
            self.create_chat(name.clone()).await.map_err(|e| format!("{e:#}"))?;
        }
        self.push_text(&name, SessionKind::Agent, text).await?;
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
            max: self.config_for(sandbox).max_sessions(),
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
        let cfg = self.config_for(sandbox);
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
    /// same parking rule as a bring-up's. A hosts warning rides along.
    pub async fn publish_port(self: &Arc<Self>, name: &SandboxRef, sandbox_port: u16, host_port: Option<u16>, host_ip: Option<String>, alias: Option<String>) -> Result<Option<String>> {
        let host_port = host_port.unwrap_or(sandbox_port);
        let ip = host_ip.unwrap_or_else(|| "127.0.0.1".into());
        let alias = alias.map(|a| a.trim().to_string()).filter(|a| !a.is_empty());
        if let Some(a) = &alias {
            if !sbxw_core::hosts::is_hostname(a) {
                bail!("'{a}' is not a hostname");
            }
        }
        if !name.is_local() {
            // On the host's loopback, reached here through the link.
            let spec = format!("127.0.0.1:{host_port}:{sandbox_port}");
            self.sbx_for(name).await?.publish(&name.name, &spec).await?;
            self.forward_port(name, &spec).await;
            return Ok(alias.map(|_| format!("'{name}' is on another host: its port is reached as {}.{}.localhost, not by an alias", name.name, name.host)));
        }
        let app = self.clone();
        let ip2 = ip.clone();
        tokio::task::spawn_blocking(move || app.hosts.ensure_loopback(&[HostAlias::new("", ip2)]))
            .await?
            .context("failed to create the loopback alias — run: sudo ifconfig lo0 alias <ip> up")?;
        self.sbx.publish(&name.name, &format!("{ip}:{host_port}:{sandbox_port}")).await?;
        let Some(alias) = alias else { return Ok(None) };
        let app = self.clone();
        let w = tokio::task::spawn_blocking(move || app.hosts.apply(&app.store, &[HostAlias::new(alias, ip)])).await?;
        Ok(w.into_iter().next())
    }

    /// The env file for an existing sandbox — the ports it *has*, the aliases
    /// its config gives them.
    pub async fn render_env_file(&self, name: &SandboxRef, dir: Option<&Path>) -> Result<RenderedEnvFile> {
        let ws = self.store.workspace(name).ok_or_else(|| {
            anyhow!("sbxw doesn't know which workspace '{name}' was created on — bring it up once with sbxw")
        })?;
        let mut cfg = self.config_for(name);
        // sbx knows no `sbxw:` kit: the file names the written-out directory.
        cfg.kits = crate::bundled::resolve(&self.store.paths, &cfg.kits)?;
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
/// is canonical — sbx mounts it at the same path inside — and apart from every
/// other: sbx shares a directory once, so a repo inside the workspace (or inside
/// another repo, or around one) is reached through that mount, with its mode —
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

#[cfg(test)]
mod tests {
    use crate::testing::{app_with, sbx_answer};
    use sbxw_core::SandboxRef;
    use sbxw_sbx::runner::Output;

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
        let p = app.policy_panel(&SandboxRef::parse("neos").unwrap()).await;
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
}
