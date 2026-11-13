//! Jarvis's proposals around their book ([`sbxw_core::jarvis`]): what the
//! daemon reads for them, the waits, and applying one once the person
//! approved it.
//!
//! Only the `sbxw-jarvis` sandbox proposes (the agent routes check it); only
//! the UI listener approves. Approval plans every change again against the
//! files as they are then: when a diff differs from the one the person read,
//! nothing is written. Then the changes apply in order, through the same
//! writers the Kits and MCP dialogs use, stopping at the first failure.

use crate::app::{now_ms, App};
use crate::bus::Event;
use crate::store::write_atomic;
use sbxw_core::config::Config;
use sbxw_core::jarvis::{self, ApproveError, Change, ChangeResult, JarvisKit, KitFileIn, Op, Proposal, State, TomlFile, Via, World};
use sbxw_core::my_skills::KitFile;
use sbxw_core::kits;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::sync::broadcast::error::RecvError;

/// Every Jarvis kit under `root`, by name. A directory sbxw would not have
/// named so is not one of them.
fn read_kits(root: &Path) -> BTreeMap<String, JarvisKit> {
    let mut out = BTreeMap::new();
    for e in std::fs::read_dir(root).into_iter().flatten().flatten() {
        let Some(name) = e.file_name().to_str().map(String::from) else { continue };
        if !jarvis::valid_kit(&name) || !e.file_type().is_ok_and(|t| t.is_dir()) {
            continue;
        }
        let dir = e.path();
        let (description, context) = jarvis::parse_spec(&std::fs::read_to_string(dir.join("spec.yaml")).unwrap_or_default());
        let mut files = Vec::new();
        fn walk(base: &Path, at: &Path, out: &mut Vec<KitFileIn>) {
            for e in std::fs::read_dir(at).into_iter().flatten().flatten() {
                let Ok(t) = e.file_type() else { continue };
                let p = e.path();
                if t.is_dir() {
                    walk(base, &p, out);
                } else if t.is_file() {
                    let rel = p.strip_prefix(base).map(|r| r.to_string_lossy().replace('\\', "/")).unwrap_or_default();
                    if let Ok(content) = std::fs::read_to_string(&p) {
                        out.push(KitFileIn { path: rel, content });
                    }
                }
            }
        }
        let home = dir.join("files/home/agent");
        walk(&home, &home, &mut files);
        files.sort_by(|a, b| a.path.cmp(&b.path));
        out.insert(name, JarvisKit { description, context, files });
    }
    out
}

/// Write a kit's directory: files rewritten only when changed (sbx's build
/// cache stays warm), files under `files/` it no longer names removed, every
/// file `0644`: data on this machine, never executable.
fn write_kit_tree(dir: &Path, files: &[KitFile]) -> anyhow::Result<()> {
    for f in files {
        let p = dir.join(&f.rel);
        if std::fs::read(&p).ok().as_deref() != Some(f.content.as_bytes()) {
            write_atomic(&p, f.content.as_bytes())?;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o644))?;
        }
    }
    fn prune(at: &Path, dir: &Path, keep: &[KitFile]) {
        for e in std::fs::read_dir(at).into_iter().flatten().flatten() {
            let p = e.path();
            if e.file_type().is_ok_and(|t| t.is_dir()) {
                prune(&p, dir, keep);
                let _ = std::fs::remove_dir(&p);
            } else {
                let rel = p.strip_prefix(dir).map(|r| r.to_string_lossy().replace('\\', "/")).unwrap_or_default();
                if !keep.iter().any(|f| f.rel == rel) {
                    let _ = std::fs::remove_file(&p);
                }
            }
        }
    }
    prune(&dir.join("files"), dir, files);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fn open(d: &Path) {
            let _ = std::fs::set_permissions(d, std::fs::Permissions::from_mode(0o755));
            for e in std::fs::read_dir(d).into_iter().flatten().flatten() {
                if e.file_type().is_ok_and(|t| t.is_dir()) {
                    open(&e.path());
                }
            }
        }
        open(dir);
    }
    Ok(())
}

fn read_or_empty(path: &Path) -> Result<String, String> {
    match std::fs::read_to_string(path) {
        Ok(raw) => Ok(raw),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(format!("{} is not readable: {e}", path.display())),
    }
}

/// Saved proposals; a file that cannot be read costs the history, nothing else.
pub(crate) fn load(path: &Path) -> Vec<Proposal> {
    std::fs::read_to_string(path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

impl App {
    pub(crate) fn jarvis_file(&self) -> PathBuf {
        self.store.paths.state().join("jarvis.json")
    }

    /// `state/kits/`, where every Jarvis kit's directory is.
    pub fn jarvis_kit_root(&self) -> PathBuf {
        self.store.paths.state().join("kits")
    }

    fn jarvis_changed(&self, p: Proposal) -> Proposal {
        let list = self.jarvis.lock().unwrap().list();
        if let Err(e) = serde_json::to_vec_pretty(&list).map_err(anyhow::Error::from).and_then(|b| write_atomic(&self.jarvis_file(), &b)) {
            tracing::warn!("jarvis: could not save the proposals: {e:#}");
        }
        self.send(Event::Jarvis(p.clone()));
        p
    }

    /// The sbxw.toml files sbxw uses, by `kits_line` key: the daemon's, and
    /// each sandbox's own.
    fn jarvis_toml_paths(&self) -> BTreeMap<String, PathBuf> {
        let mut out = BTreeMap::new();
        if let Some(p) = &self.cfg.source {
            out.insert(jarvis::DAEMON_FILE.to_string(), p.clone());
        }
        for name in self.store.names() {
            if let Some(p) = self.store.get(&name).config {
                out.insert(name.to_string(), p);
            }
        }
        out
    }

    /// What the planner reads. The MCP servers only when `with_mcp`: a
    /// proposal without an MCP change does not depend on sbx answering.
    async fn jarvis_world(&self, with_mcp: bool) -> Result<World, String> {
        let mut files = BTreeMap::new();
        for (key, path) in self.jarvis_toml_paths() {
            files.insert(key, TomlFile { path: path.to_string_lossy().into_owned(), raw: read_or_empty(&path)? });
        }
        let mcp = if with_mcp {
            Some(self.sbx.mcp_ls().await.map_err(|e| format!("the MCP servers could not be listed: {e:#}"))?.into_iter().map(|s| s.name).collect())
        } else {
            None
        };
        Ok(World {
            files,
            kits: read_kits(&self.jarvis_kit_root()),
            kit_root: self.jarvis_kit_root().to_string_lossy().into_owned(),
            mcp,
            models: self.cfg.models.iter().map(|m| m.id.clone()).collect(),
        })
    }

    /// The read-only view Jarvis plans from. No secret, token or env value.
    pub async fn jarvis_state(&self) -> Value {
        let world = self.jarvis_world(false).await.unwrap_or_default();
        let files: Vec<Value> = world
            .files
            .iter()
            .map(|(key, f)| {
                let kits = kits::read_kits_line(&f.raw);
                json!({
                    "file": key,
                    "path": f.path,
                    "kits": kits.as_ref().ok().cloned().flatten(),
                    "defaults_apply": kits.as_ref().is_ok_and(|k| k.is_none()),
                    "error": kits.err().map(|e| format!("not valid TOML: {e}")),
                })
            })
            .collect();
        let bundled: Vec<Value> = kits::BUNDLED.iter().map(|b| json!({ "ref": format!("sbxw:{b}"), "about": kits::about(b) })).collect();
        let jarvis_kits: Vec<Value> = world
            .kits
            .iter()
            .map(|(name, k)| json!({ "kit": name, "ref": world.kit_ref(name), "description": k.description, "context": k.context, "files": k.files }))
            .collect();
        let daemon_cfg = world.files.get(jarvis::DAEMON_FILE).and_then(|f| Config::parse(&f.raw).ok());
        let shown = daemon_cfg.clone().unwrap_or_else(|| self.cfg.clone());
        let settings: Vec<Value> = jarvis::SETTINGS
            .iter()
            .map(|(k, _)| json!({ "key": k, "value": jarvis::current_value(&shown, k), "allowed": jarvis::allowed_values(k, &world.models) }))
            .collect();
        let mcp: Vec<Value> = match self.sbx.mcp_ls().await {
            Ok(list) => list.into_iter().map(|s| json!({ "name": s.name, "type": s.kind })).collect(),
            Err(_) => vec![],
        };
        let running: Vec<String> = self.sbx.ls().await.into_iter().filter(|s| s.is_running()).map(|s| s.name).collect();
        json!({
            "ok": true,
            "files": files,
            "bundled": bundled,
            "default_kits": kits::DEFAULT_KITS,
            "jarvis_kits": jarvis_kits,
            "settings": settings,
            "network_allow": shown.network_allow,
            "mcp_servers": mcp,
            "running_sandboxes": running,
            "daemon_file": daemon_cfg.is_some() || world.files.contains_key(jarvis::DAEMON_FILE),
            "limits": { "changes": jarvis::MAX_CHANGES, "kit_files": sbxw_core::my_skills::MAX_FILES, "kit_bytes": sbxw_core::my_skills::MAX_BYTES },
        })
    }

    pub fn jarvis_list(&self) -> Vec<Proposal> {
        self.jarvis.lock().unwrap().list()
    }

    /// Check every change, then record the proposal. Nothing is written.
    pub async fn jarvis_propose(&self, summary: &str, changes: Vec<Change>) -> Result<Proposal, String> {
        let with_mcp = changes.iter().any(|c| matches!(c, Change::McpServer { .. } | Change::McpRemove { .. }));
        if let Some(p) = self.jarvis.lock().unwrap().pending() {
            return Err(format!("proposal {} is still waiting for the person: wait on it before proposing again", p.id));
        }
        let world = self.jarvis_world(with_mcp).await?;
        let planned = jarvis::plan(&world, &changes)?;
        let views = planned.into_iter().map(|p| p.view).collect();
        let id = format!("j-{}", sbxw_core::ids::random_hex(8));
        let p = self.jarvis.lock().unwrap().propose(id, summary, changes, views, now_ms())?;
        tracing::info!("jarvis: proposed {} ({} change(s), {:?} risk)", p.id, p.changes.len(), p.risk);
        Ok(self.jarvis_changed(p))
    }

    /// Park until the proposal is settled or the bound passes.
    pub async fn jarvis_wait(&self, id: &str, bound: Duration) -> Result<Proposal, String> {
        let get = || self.jarvis.lock().unwrap().get(id).cloned().ok_or_else(|| format!("no proposal '{id}'"));
        let mut rx = self.bus.subscribe();
        let mut latest = get()?;
        let deadline = tokio::time::Instant::now() + bound;
        while latest.state == State::Pending {
            let left = deadline.saturating_duration_since(tokio::time::Instant::now());
            if left.is_zero() {
                break;
            }
            match tokio::time::timeout(left, rx.recv()).await {
                Ok(Ok(Event::Jarvis(p))) if p.id == id => latest = p,
                Ok(Ok(_)) => {}
                Ok(Err(RecvError::Lagged(_))) => latest = get()?,
                Ok(Err(RecvError::Closed)) | Err(_) => break,
            }
        }
        Ok(latest)
    }

    pub fn jarvis_refuse(&self, id: &str, reason: Option<String>) -> Result<Proposal, ApproveError> {
        let p = self.jarvis.lock().unwrap().refuse(id, reason, now_ms())?;
        tracing::info!("jarvis: {id} refused");
        Ok(self.jarvis_changed(p))
    }

    /// The person approved: plan again against the files as they are now,
    /// refuse when what would be written is not what they read, then apply
    /// in order and stop at the first failure.
    pub async fn jarvis_approve(&self, id: &str, via: Via) -> Result<Proposal, ApproveError> {
        let p = self.jarvis.lock().unwrap().begin(id, via)?;
        let finish = |results: Vec<ChangeResult>, error: Option<String>| {
            let done = self.jarvis.lock().unwrap().finish(id, via, results, error, now_ms());
            done.map(|d| self.jarvis_changed(d)).ok_or_else(|| ApproveError::NotFound(format!("no proposal '{id}'")))
        };
        let with_mcp = p.changes.iter().any(|c| matches!(c, Change::McpServer { .. } | Change::McpRemove { .. }));
        let planned = match self.jarvis_world(with_mcp).await.and_then(|w| jarvis::plan(&w, &p.changes)) {
            Ok(planned) => planned,
            Err(e) => return finish(vec![], Some(format!("not applied, the configuration changed since the proposal: {e}"))),
        };
        if let Some((i, now)) = planned.iter().map(|x| &x.view).zip(&p.views).position(|(now, then)| now != then).map(|i| (i, &planned[i].view)) {
            return finish(
                vec![],
                Some(format!("not applied: change {} ({}) would no longer write what you read, the file changed since. Refuse this proposal and ask Jarvis again.", i + 1, now.title)),
            );
        }
        tracing::info!("jarvis: applying {id} ({} change(s), by {via:?})", planned.len());
        let mut results = Vec::new();
        for (i, step) in planned.into_iter().enumerate() {
            let r = self.jarvis_apply(step.op).await;
            let failed = !r.ok;
            if failed {
                tracing::warn!("jarvis: {id} change {} failed: {}", i + 1, r.message);
            }
            results.push(r);
            if failed {
                break;
            }
        }
        finish(results, None)
    }

    async fn jarvis_apply(&self, op: Op) -> ChangeResult {
        let ok = |message: String, restart: bool| ChangeResult { ok: true, message, restart };
        let err = |message: String| ChangeResult { ok: false, message, restart: false };
        match op {
            Op::WriteKit { kit, tree } => match write_kit_tree(&self.jarvis_kit_root().join(&kit), &tree) {
                Ok(()) => ok(format!("{kit} written; add it to a kits line for new sandboxes to get it"), false),
                Err(e) => err(format!("writing {kit}: {e:#}")),
            },
            Op::RemoveKit { kit } => match std::fs::remove_dir_all(self.jarvis_kit_root().join(&kit)) {
                Ok(()) => ok(format!("{kit} removed"), false),
                Err(e) => err(format!("removing {kit}: {e}")),
            },
            Op::KitsLine { file, path, list } => match self.set_kits(Path::new(&path), Some(list)) {
                Ok(warnings) => {
                    let mut m = format!("kits line of {file} written");
                    for w in warnings {
                        m.push_str(&format!("; {w}"));
                    }
                    ok(m, false)
                }
                Err(e) => err(format!("{e:#}")),
            },
            Op::WriteToml { path, text, network_added } => {
                let p = Path::new(&path);
                let before = read_or_empty(p).unwrap_or_default();
                if let Err(e) = crate::svc::kits::write_in_place(p, &text) {
                    return err(format!("{e:#}"));
                }
                if !network_added.is_empty() {
                    // The person approved these hosts in the dialog: their
                    // grant lines are recorded, so the next bring-up does not
                    // ask again. Only the lines this write added.
                    let lines = |raw: &str| Config::parse(raw).map(|c| sbxw_core::consent::grants(&c)).unwrap_or_default();
                    let added: Vec<String> = sbxw_core::consent::new_grants(&lines(&text), &lines(&before)).into_iter().filter(|g| g.starts_with("network")).collect();
                    if !added.is_empty() {
                        let recorded = Config::load(p, Path::new("/"))
                            .map_err(|e| format!("{e:#}"))
                            .and_then(|cfg| self.store.check_grants(&crate::store::grant_key(p), &sbxw_core::consent::grants(&cfg), &added, &added).map_err(|e| format!("{e:#}")));
                        if let Err(e) = recorded {
                            return ok(format!("{path} written, but the approval was not recorded ({e}): the next bring-up asks again"), true);
                        }
                    }
                }
                ok(format!("{path} written; it takes effect when the daemon restarts"), true)
            }
            Op::McpAdd { replace, spec } => {
                if replace {
                    if let Err(e) = self.sbx.mcp_rm(&spec.name).await {
                        return err(format!("removing the old {}: {e:#}", spec.name));
                    }
                }
                match self.sbx.mcp_add(&spec).await {
                    Ok(()) => {
                        let keys = spec.placeholders();
                        let mut m = format!("MCP server {} added", spec.name);
                        if !keys.is_empty() {
                            m.push_str(&format!("; add its secrets ({}) in the MCP dialog", keys.join(", ")));
                        }
                        ok(m, false)
                    }
                    Err(e) if replace => err(format!("{} was removed, but adding it again failed: {e:#}", spec.name)),
                    Err(e) => err(format!("{e:#}")),
                }
            }
            Op::McpRm { name } => match self.sbx.mcp_rm(&name).await {
                Ok(()) => ok(format!("MCP server {name} removed"), false),
                Err(e) => err(format!("{e:#}")),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::testing::{app_with, sbx_answer};
    use sbxw_core::jarvis::{Change, State, Via};
    use serde_json::json;

    fn changes(v: serde_json::Value) -> Vec<Change> {
        serde_json::from_value(v).unwrap()
    }

    #[tokio::test]
    async fn a_kit_write_round_trip_produces_a_v3_kit_directory() {
        let (_dir, app, _) = app_with(sbx_answer);
        let p = app
            .jarvis_propose(
                "A hello kit",
                changes(json!([{"kind": "kit_write", "kit": "jarvis-hello", "description": "Hello", "files": [
                    {"path": ".claude/commands/hello.md", "content": "Say hello\n"},
                    {"path": "bin/hi.sh", "content": "#!/bin/sh\necho hi\n"}]}])),
            )
            .await
            .unwrap();
        assert_eq!(p.state, State::Pending);
        let root = app.jarvis_kit_root().join("jarvis-hello");
        assert!(!root.exists(), "nothing is written before approval");
        let done = app.jarvis_approve(&p.id, Via::Click).await.unwrap();
        assert_eq!(done.state, State::Applied, "{:?}", done.error);
        assert!(sbxw_core::kits::is_v3_kit(&root));
        assert_eq!(std::fs::read_to_string(root.join("files/home/agent/bin/hi.sh")).unwrap(), "#!/bin/sh\necho hi\n");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(root.join("files/home/agent/bin/hi.sh")).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o644, "never executable on the host");
        }

        // Read back and updated: the file removed leaves the tree.
        let state = app.jarvis_state().await;
        assert_eq!(state["jarvis_kits"][0]["kit"], "jarvis-hello");
        assert_eq!(state["jarvis_kits"][0]["description"], "Hello");
        let p = app.jarvis_propose("drop the script", changes(json!([{"kind": "kit_write", "kit": "jarvis-hello", "remove": ["bin/hi.sh"]}]))).await.unwrap();
        assert!(p.views[0].diff.contains("+++ /dev/null"), "{}", p.views[0].diff);
        assert_eq!(app.jarvis_approve(&p.id, Via::Click).await.unwrap().state, State::Applied);
        assert!(!root.join("files/home/agent/bin").exists());
        assert!(root.join("files/home/agent/.claude/commands/hello.md").exists());
    }

    #[tokio::test]
    async fn voice_cannot_approve_a_high_risk_proposal() {
        let (_dir, app, _) = app_with(sbx_answer);
        let p = app.jarvis_propose("kit", changes(json!([{"kind": "kit_write", "kit": "jarvis-a", "files": [{"path": "a.md", "content": "a"}]}]))).await.unwrap();
        assert!(matches!(app.jarvis_approve(&p.id, Via::Voice).await, Err(sbxw_core::jarvis::ApproveError::Conflict(_))));
        assert_eq!(app.jarvis_list()[0].state, State::Pending, "still waiting for a click");
        assert!(app.jarvis_propose("another", changes(json!([{"kind": "kit_remove", "kit": "jarvis-a"}]))).await.unwrap_err().contains("still waiting"));
        assert_eq!(app.jarvis_refuse(&p.id, Some("no".into())).unwrap().state, State::Refused);
    }

    #[tokio::test]
    async fn a_file_changed_since_the_proposal_is_not_written() {
        let d = tempfile::tempdir().unwrap();
        let file = d.path().join("sbxw.toml");
        std::fs::write(&file, "# mine\nkits = [\"sbxw:codemap\"]\n").unwrap();
        let mut cfg = sbxw_core::config::Config::load(&file, std::path::Path::new("/")).unwrap();
        cfg.source = Some(file.clone());
        let app = crate::app::AppBuilder {
            paths: crate::store::Paths::under(d.path()),
            cfg,
            sbx: sbxw_sbx::Sbx::new(sbxw_sbx::runner::Fake::new(sbx_answer)),
            hosts: crate::hostsys::for_tests(&d.path().join("hosts")),
            use_api_key: true,
            sbx_version: Some("0.51.0".into()),
        }
        .build();

        // A low-risk proposal approved by voice: written, and the daemon's kits follow.
        let p = app.jarvis_propose("retro", changes(json!([{"kind": "kits_line", "file": "daemon", "add": ["sbxw:retro"]}, {"kind": "setting", "key": "keep_awake", "value": "always"}]))).await.unwrap();
        let done = app.jarvis_approve(&p.id, Via::Voice).await.unwrap();
        assert_eq!(done.state, State::Applied, "{:?}", done.error);
        assert!(done.results[1].restart);
        let text = std::fs::read_to_string(&file).unwrap();
        assert!(text.starts_with("# mine\n") && text.contains("\"sbxw:retro\"") && text.contains("keep_awake = \"always\""), "{text}");
        assert_eq!(app.daemon_config().kits, vec!["sbxw:codemap".to_string(), "sbxw:retro".to_string()]);

        // Changed by hand after the proposal: refused, the hand edit kept.
        let p = app.jarvis_propose("more", changes(json!([{"kind": "setting", "key": "battery_warn_below", "value": 40}]))).await.unwrap();
        std::fs::write(&file, "battery_warn_below = 10\n").unwrap();
        let done = app.jarvis_approve(&p.id, Via::Click).await.unwrap();
        assert_eq!(done.state, State::Failed);
        assert!(done.error.as_deref().unwrap().contains("changed since"), "{:?}", done.error);
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "battery_warn_below = 10\n");
    }
}
