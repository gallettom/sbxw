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
use sbxw_core::jarvis::{self, ApproveError, Change, ChangeResult, Desktop, JarvisKit, KitFileIn, Op, Proposal, State, TomlFile, Via, World};
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
    Ok(read_toml(path)?.raw)
}

/// The file as the planner sees it: missing is an empty file to create.
fn read_toml(path: &Path) -> Result<TomlFile, String> {
    let path_s = path.to_string_lossy().into_owned();
    match std::fs::read_to_string(path) {
        Ok(raw) => Ok(TomlFile { path: path_s, raw, missing: false }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(TomlFile { path: path_s, raw: String::new(), missing: true }),
        Err(e) => Err(format!("{} is not readable: {e}", path.display())),
    }
}

/// A file sbxw may create: its folder is there already. sbxw does not make
/// folders on the person's machine.
fn creatable(path: &Path) -> Result<(), String> {
    if path.exists() {
        return Ok(());
    }
    match path.parent() {
        Some(dir) if dir.is_dir() => Ok(()),
        Some(dir) => Err(format!("{} cannot be created: its folder {} does not exist", path.display(), dir.display())),
        None => Err(format!("{} cannot be created", path.display())),
    }
}

/// The sbxw.toml an op writes, if any.
fn op_toml(op: &Op) -> Option<&str> {
    match op {
        Op::KitsLine { path, .. } | Op::WriteToml { path, .. } | Op::VoiceWords { path, .. } | Op::Emphasis { path, .. } => Some(path),
        _ => None,
    }
}

/// The `<name>.luau` files of `dir` whose name is a screen name, by name.
/// A file that is not UTF-8 text is left out.
fn read_screens(dir: &Path) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let Some(file) = e.file_name().to_str().map(String::from) else { continue };
        let Some(name) = file.strip_suffix(".luau") else { continue };
        if !jarvis::valid_screen_name(name) || !e.file_type().is_ok_and(|t| t.is_file()) {
            continue;
        }
        if let Ok(text) = std::fs::read_to_string(e.path()) {
            out.insert(name.to_string(), text);
        }
    }
    out
}

/// What the desktop app left in `<sbxw home>/desktop/`: `builtin/` is the
/// app's to write at each start (read only here), `screens/` the override
/// folder sbxw writes on approval.
pub(crate) fn read_desktop(root: &Path) -> Desktop {
    let builtin_dir = root.join("builtin");
    let present = builtin_dir.join("screens").is_dir();
    let (intents, intents_error) = if present {
        match std::fs::read_to_string(builtin_dir.join("intents.json")) {
            Ok(raw) => match jarvis::parse_intents(&raw) {
                Ok(list) => (list, None),
                Err(e) => (vec![], Some(e)),
            },
            Err(e) => (vec![], Some(format!("{} is not readable: {e}", builtin_dir.join("intents.json").display()))),
        }
    } else {
        (vec![], None)
    };
    Desktop {
        present,
        builtin_dir: builtin_dir.to_string_lossy().into_owned(),
        screens_dir: root.join("screens").to_string_lossy().into_owned(),
        builtin: if present { read_screens(&builtin_dir.join("screens")) } else { BTreeMap::new() },
        overrides: read_screens(&root.join("screens")),
        intents,
        intents_error,
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

    /// `<sbxw home>/desktop/`: the desktop app's `builtin/` and the
    /// override folder `screens/`.
    pub fn desktop_root(&self) -> PathBuf {
        self.store.paths.root.join("desktop")
    }

    /// The override file of screen `name`, built from the name alone.
    fn screen_file(&self, name: &str) -> Result<PathBuf, String> {
        if !jarvis::valid_screen_name(name) {
            return Err(format!("'{name}' is not a screen name"));
        }
        Ok(self.desktop_root().join("screens").join(format!("{name}.luau")))
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

    /// The sbxw.toml files sbxw uses, by `kits_line` key: the daemon's (the
    /// one it was asked to read, even when it does not exist yet), and each
    /// sandbox's own.
    fn jarvis_toml_paths(&self) -> BTreeMap<String, PathBuf> {
        let mut out = BTreeMap::new();
        if let Some(p) = self.cfg.source.as_ref().or(self.cfg.asked.as_ref()) {
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
            files.insert(key, read_toml(&path)?);
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
            desktop: read_desktop(&self.desktop_root()),
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
                    "exists": !f.missing,
                    "note": f.missing.then_some("this file does not exist yet: a change to it creates it"),
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
        // The file's table when it parses, else what the voice uses now.
        let configured = daemon_cfg.as_ref().map(|c| c.voice.words.clone()).unwrap_or_else(|| self.voice_words());
        let voice_words: Vec<Value> = sbxw_core::spoken::word_table(&configured)
            .into_iter()
            .map(|w| json!({ "word": w.word, "fr": w.said.fr, "en": w.said.en, "origin": w.origin }))
            .collect();
        // The status pills as the daemon's file has them (else as they are now).
        let em = daemon_cfg.as_ref().map(|c| c.emphasis.clone()).unwrap_or_else(|| self.emphasis_settings());
        let emphasis = json!({
            "enabled": em.enabled(),
            "guidance": em.guidance(),
            "configured": em.words.is_some(),
            "families": sbxw_core::emphasis::families(&em.vocabulary()),
            "tones": sbxw_core::emphasis::TONES,
            "sandboxes": self.emphasis_contract().sandboxes,
        });
        let mcp: Vec<Value> = match self.sbx.mcp_ls().await {
            Ok(list) => list.into_iter().map(|s| json!({ "name": s.name, "type": s.kind })).collect(),
            Err(_) => vec![],
        };
        let running: Vec<String> = self.sbx.ls().await.into_iter().filter(|s| s.is_running()).map(|s| s.name).collect();
        let d = &world.desktop;
        let desktop = json!({
            "app_ran": d.present,
            "note": (!d.present).then(|| format!("sbxw's desktop app has not run on this machine yet ({} does not exist): there is no screen to change until the person starts it once", d.builtin_dir)),
            "override_folder": d.screens_dir,
            "screens": d.builtin.keys().map(|n| json!({ "name": n, "override": d.overrides.contains_key(n) })).collect::<Vec<_>>(),
            "overrides_without_builtin": d.overrides.keys().filter(|n| d.present && !d.builtin.contains_key(*n)).collect::<Vec<_>>(),
            "intents": d.intents,
            "intents_error": d.intents_error,
        });
        json!({
            "ok": true,
            "files": files,
            "bundled": bundled,
            "default_kits": kits::DEFAULT_KITS,
            "jarvis_kits": jarvis_kits,
            "settings": settings,
            "voice_words": voice_words,
            "emphasis": emphasis,
            "network_allow": shown.network_allow,
            "mcp_servers": mcp,
            "running_sandboxes": running,
            "desktop": desktop,
            "daemon_file": daemon_cfg.is_some() || world.files.contains_key(jarvis::DAEMON_FILE),
            "limits": { "voice_words": sbxw_core::spoken::MAX_WORDS, "voice_word_chars": sbxw_core::spoken::MAX_WORD, "voice_said_chars": sbxw_core::spoken::MAX_SAID, "emphasis_word_chars": sbxw_core::emphasis::MAX_WORD, "emphasis_words": sbxw_core::emphasis::MAX_WORDS, "emphasis_families": sbxw_core::emphasis::MAX_FAMILIES, "emphasis_use_chars": sbxw_core::emphasis::MAX_USE, "changes": jarvis::MAX_CHANGES, "kit_files": sbxw_core::my_skills::MAX_FILES, "kit_bytes": sbxw_core::my_skills::MAX_BYTES, "screen_bytes": jarvis::MAX_SCREEN },
        })
    }

    /// One desktop screen as the app uses it now: its text, whether it is
    /// the person's override, and the built-in it replaces.
    pub fn jarvis_desktop_screen(&self, name: &str) -> Result<Value, String> {
        let d = read_desktop(&self.desktop_root());
        d.screen(name)?;
        let (content, is_override) = d.effective(name).unwrap_or_default();
        Ok(json!({
            "ok": true,
            "name": name,
            "override": is_override,
            "path": d.override_path(name),
            "content": content,
            "builtin": d.builtin.get(name),
        }))
    }

    /// The desktop app's own documentation of what a screen may use
    /// (`screens.md`) and its intents (`intents.json`), as it wrote them.
    pub fn jarvis_desktop_docs(&self) -> Result<Value, String> {
        let d = read_desktop(&self.desktop_root());
        if !d.present {
            return Err(d.screen("x").unwrap_err());
        }
        let dir = Path::new(&d.builtin_dir);
        let read = |f: &str| std::fs::read_to_string(dir.join(f)).map_err(|e| format!("{} is not readable: {e}", dir.join(f).display()));
        let intents = read("intents.json").and_then(|raw| serde_json::from_str::<Value>(&raw).map_err(|e| format!("intents.json is not valid JSON: {e}")));
        Ok(json!({
            "ok": true,
            "screens_md": read("screens.md").unwrap_or_else(|e| e),
            "intents": intents.unwrap_or_else(Value::String),
            "screens": d.builtin.keys().collect::<Vec<_>>(),
        }))
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
        for path in planned.iter().filter_map(|p| op_toml(&p.op)) {
            creatable(Path::new(path))?;
        }
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
    pub async fn jarvis_approve(&self, id: &str, via: Via, ack_host_command: bool) -> Result<Proposal, ApproveError> {
        let p = self.jarvis.lock().unwrap().begin(id, via, ack_host_command)?;
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
        if let Some(path) = op_toml(&op) {
            if let Err(e) = creatable(Path::new(path)) {
                return err(e);
            }
        }
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
                let created = !p.exists();
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
                ok(format!("{path} {}; it takes effect when the daemon restarts", if created { "created" } else { "written" }), true)
            }
            Op::VoiceWords { path, text, words } => {
                let p = Path::new(&path);
                let created = !p.exists();
                if let Err(e) = crate::svc::kits::write_in_place(p, &text) {
                    return err(format!("{e:#}"));
                }
                self.set_voice_words(words);
                ok(format!("{path} {}; the voice says it this way from the next sentence it reads", if created { "created" } else { "written" }), false)
            }
            Op::Emphasis { path, text, live } => {
                let p = Path::new(&path);
                let created = !p.exists();
                if let Err(e) = crate::svc::kits::write_in_place(p, &text) {
                    return err(format!("{e:#}"));
                }
                let done = if created { "created" } else { "written" };
                match live {
                    Some(settings) => {
                        self.set_emphasis(settings);
                        ok(format!("{path} {done}; the terminals redraw the status pills now, and each sandbox's agent reads the new guidance after its next bring-up, in a new session"), false)
                    }
                    None => ok(format!("{path} {done}; it takes effect at that sandbox's next bring-up (sbxw up)"), false),
                }
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
            // The path is the daemon's, from the name alone. The folder is
            // sbxw's own, under its home: created when missing (unlike the
            // folder of an sbxw.toml, which is the person's).
            Op::WriteScreen { name, content } => {
                let path = match self.screen_file(&name) {
                    Ok(p) => p,
                    Err(e) => return err(e),
                };
                match write_atomic(&path, content.as_bytes()) {
                    Ok(()) => ok(format!("your {name} screen written ({}); the desktop app reloads it at once, and if it does not load, keeps the last good view and shows the error", path.display()), false),
                    Err(e) => err(format!("writing {}: {e:#}", path.display())),
                }
            }
            Op::RemoveScreen { name } => {
                let path = match self.screen_file(&name) {
                    Ok(p) => p,
                    Err(e) => return err(e),
                };
                match std::fs::remove_file(&path) {
                    Ok(()) => ok(format!("your {name} screen removed; the desktop app is back on its built-in one"), false),
                    Err(e) => err(format!("removing {}: {e}", path.display())),
                }
            }
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
        let done = app.jarvis_approve(&p.id, Via::Click, false).await.unwrap();
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
        assert_eq!(app.jarvis_approve(&p.id, Via::Click, false).await.unwrap().state, State::Applied);
        assert!(!root.join("files/home/agent/bin").exists());
        assert!(root.join("files/home/agent/.claude/commands/hello.md").exists());
    }

    /// A stdio server runs on this machine: applied only with the command
    /// acknowledged, and then exactly that command reaches sbx.
    #[tokio::test]
    async fn a_host_command_is_applied_only_once_acknowledged() {
        let answer = |a: &[String]| match a.first().map(String::as_str) {
            Some("mcp") if a.get(1).map(String::as_str) == Some("ls") => sbxw_sbx::runner::Output::ok("[]"),
            _ => sbx_answer(a),
        };
        let (_dir, app, fake) = app_with(answer);
        let p = app
            .jarvis_propose("playwright", changes(json!([{"kind": "mcp_server", "spec": {"name": "pw", "kind": "stdio", "command": "npx", "args": ["-y", "@playwright/mcp@latest"]}}])))
            .await
            .unwrap();
        assert_eq!(p.views[0].host_command.as_deref(), Some("npx -y @playwright/mcp@latest"));
        assert!(matches!(app.jarvis_approve(&p.id, Via::Click, false).await, Err(sbxw_core::jarvis::ApproveError::Conflict(_))));
        assert!(!fake.called("mcp add"), "nothing applied without the ack");
        let done = app.jarvis_approve(&p.id, Via::Click, true).await.unwrap();
        assert_eq!(done.state, State::Applied, "{:?}", done.error);
        assert!(fake.log().iter().any(|l| l.contains("mcp add pw --command=npx --args=-y,@playwright/mcp@latest")), "{:?}", fake.log());
    }

    #[tokio::test]
    async fn voice_cannot_approve_a_high_risk_proposal() {
        let (_dir, app, _) = app_with(sbx_answer);
        let p = app.jarvis_propose("kit", changes(json!([{"kind": "kit_write", "kit": "jarvis-a", "files": [{"path": "a.md", "content": "a"}]}]))).await.unwrap();
        assert!(matches!(app.jarvis_approve(&p.id, Via::Voice, false).await, Err(sbxw_core::jarvis::ApproveError::Conflict(_))));
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
        let done = app.jarvis_approve(&p.id, Via::Voice, false).await.unwrap();
        assert_eq!(done.state, State::Applied, "{:?}", done.error);
        assert!(done.results[1].restart);
        let text = std::fs::read_to_string(&file).unwrap();
        assert!(text.starts_with("# mine\n") && text.contains("\"sbxw:retro\"") && text.contains("keep_awake = \"always\""), "{text}");
        assert_eq!(app.daemon_config().kits, vec!["sbxw:codemap".to_string(), "sbxw:retro".to_string()]);

        // Changed by hand after the proposal: refused, the hand edit kept.
        let p = app.jarvis_propose("more", changes(json!([{"kind": "setting", "key": "battery_warn_below", "value": 40}]))).await.unwrap();
        std::fs::write(&file, "battery_warn_below = 10\n").unwrap();
        let done = app.jarvis_approve(&p.id, Via::Click, false).await.unwrap();
        assert_eq!(done.state, State::Failed);
        assert!(done.error.as_deref().unwrap().contains("changed since"), "{:?}", done.error);
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "battery_warn_below = 10\n");
    }

    fn app_at(d: &std::path::Path, file: &std::path::Path) -> std::sync::Arc<crate::app::App> {
        crate::app::AppBuilder {
            paths: crate::store::Paths::under(d),
            cfg: sbxw_core::config::Config::load(file, std::path::Path::new("/")).unwrap(),
            sbx: sbxw_sbx::Sbx::new(sbxw_sbx::runner::Fake::new(sbx_answer)),
            hosts: crate::hostsys::for_tests(&d.join("hosts")),
            use_api_key: true,
            sbx_version: Some("0.51.0".into()),
        }
        .build()
    }

    /// Started where there is no sbxw.toml: the voice can still be changed,
    /// by creating the file the daemon was asked to read.
    #[tokio::test]
    async fn a_setting_creates_the_missing_daemon_file() {
        let d = tempfile::tempdir().unwrap();
        let file = d.path().join("sbxw.toml");
        let app = app_at(d.path(), &file);
        let state = app.jarvis_state().await;
        assert_eq!(state["daemon_file"], true);
        let daemon = state["files"].as_array().unwrap().iter().find(|f| f["file"] == "daemon").unwrap().clone();
        assert_eq!((daemon["path"].as_str(), &daemon["exists"]), (file.to_str(), &json!(false)), "{daemon}");

        let p = app.jarvis_propose("voice", changes(json!([{"kind": "setting", "key": "voice.tts_voice", "value": "de_female"}]))).await.unwrap();
        assert_eq!(p.risk, sbxw_core::jarvis::Risk::Low);
        assert!(p.views[0].title.contains(&format!("creating {}", file.display())), "{}", p.views[0].title);
        assert!(p.views[0].diff.starts_with("--- /dev/null\n"), "{}", p.views[0].diff);
        assert!(!file.exists(), "nothing is written before approval");
        let done = app.jarvis_approve(&p.id, Via::Voice, false).await.unwrap();
        assert_eq!(done.state, State::Applied, "{:?}", done.error);
        assert!(done.results[0].message.contains("created"), "{}", done.results[0].message);
        let cfg = sbxw_core::config::Config::load(&file, std::path::Path::new("/")).unwrap();
        assert_eq!(sbxw_core::jarvis::current_value(&cfg, "voice.tts_voice"), json!("de_female"));

        // The file appears between proposal and approval: nothing written.
        let other = tempfile::tempdir().unwrap();
        let file = other.path().join("sbxw.toml");
        let app = app_at(other.path(), &file);
        let p = app.jarvis_propose("voice", changes(json!([{"kind": "setting", "key": "keep_awake", "value": "always"}]))).await.unwrap();
        std::fs::write(&file, "battery_warn_below = 10\n").unwrap();
        let done = app.jarvis_approve(&p.id, Via::Click, false).await.unwrap();
        assert_eq!(done.state, State::Failed);
        assert!(done.error.as_deref().unwrap().contains("changed since"), "{:?}", done.error);
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "battery_warn_below = 10\n");

        // No folder to create it in: refused, with the reason.
        let gone = d.path().join("nowhere/sbxw.toml");
        let app = app_at(d.path(), &gone);
        let e = app.jarvis_propose("voice", changes(json!([{"kind": "setting", "key": "keep_awake", "value": "always"}]))).await.unwrap_err();
        assert!(e.contains("does not exist") && e.contains("nowhere"), "{e}");
    }

    /// Status pills: a switch is low risk and live for the UIs; a new word
    /// is high (its text reaches every agent's CLAUDE.md), so voice cannot
    /// approve it.
    #[tokio::test]
    async fn approving_an_emphasis_change_writes_it_and_the_uis_hear_it() {
        let d = tempfile::tempdir().unwrap();
        let file = d.path().join("sbxw.toml");
        std::fs::write(&file, "# mine\nkeep_awake = \"off\"\n").unwrap();
        let app = app_at(d.path(), &file);
        let state = app.jarvis_state().await;
        assert_eq!(state["emphasis"]["enabled"], json!(true));
        assert_eq!(state["emphasis"]["families"][0]["word"], json!("done"));

        let mut rx = app.bus.subscribe();
        let p = app.jarvis_propose("pills off", changes(json!([{"kind": "emphasis_switch", "enabled": false}]))).await.unwrap();
        assert_eq!(p.risk, sbxw_core::jarvis::Risk::Low);
        let done = app.jarvis_approve(&p.id, Via::Voice, false).await.unwrap();
        assert_eq!(done.state, State::Applied, "{:?}", done.error);
        assert!(done.results[0].message.contains("redraw"), "{}", done.results[0].message);
        assert!(std::fs::read_to_string(&file).unwrap().contains("[emphasis]\nenabled = false"));
        assert!(!app.emphasis_settings().enabled());
        let mut heard = false;
        while let Ok(ev) = rx.try_recv() {
            heard |= ev.name() == "emphasis" && ev.data().contains("\"enabled\":false");
        }
        assert!(heard, "the UIs are told");

        let p = app.jarvis_propose("a word", changes(json!([{"kind": "emphasis_word", "word": "Shipped", "tone": "green"}]))).await.unwrap();
        assert_eq!(p.risk, sbxw_core::jarvis::Risk::High);
        assert!(app.jarvis_approve(&p.id, Via::Voice, false).await.is_err(), "a spoken yes does not approve it");
        let done = app.jarvis_approve(&p.id, Via::Click, false).await.unwrap();
        assert_eq!(done.state, State::Applied, "{:?}", done.error);
        assert_eq!(app.emphasis_contract().words["shipped"], "green");
        assert!(app.emphasis_guidance(&sbxw_core::SandboxRef::parse("any").unwrap()).is_none(), "no guidance while the pills are off");
    }

    /// A word's pronunciation: low risk, so a spoken yes approves it; the
    /// file gets `[voice.words]` and the voice takes it at once.
    #[tokio::test]
    async fn approving_a_voice_word_writes_it_and_the_voice_uses_it() {
        let d = tempfile::tempdir().unwrap();
        let file = d.path().join("sbxw.toml");
        std::fs::write(&file, "# mine\nkeep_awake = \"off\"\n").unwrap();
        let app = app_at(d.path(), &file);
        let state = app.jarvis_state().await;
        assert_eq!(state["voice_words"], json!([{"word": "sbxw", "fr": "ece bé ixe double vé", "en": "S B X W", "origin": "built_in"}]));

        let p = app.jarvis_propose("dictionary", changes(json!([{"kind": "voice_word", "word": "nginx", "en": "engine X"}]))).await.unwrap();
        assert_eq!(p.risk, sbxw_core::jarvis::Risk::Low);
        assert!(app.voice_words().is_empty(), "nothing changes before approval");
        let done = app.jarvis_approve(&p.id, Via::Voice, false).await.unwrap();
        assert_eq!(done.state, State::Applied, "{:?}", done.error);
        assert!(done.results[0].message.contains("from the next sentence") && !done.results[0].restart, "{:?}", done.results[0]);
        let text = std::fs::read_to_string(&file).unwrap();
        assert_eq!(text, "# mine\nkeep_awake = \"off\"\n\n[voice.words]\nnginx = { en = \"engine X\" }\n");
        assert_eq!(app.voice_words()["sbxw"].fr.as_deref(), "live, no restart");
        let state = app.jarvis_state().await;
        let origins: Vec<(String, String)> = state["voice_words"].as_array().unwrap().iter().map(|w| (w["word"].as_str().unwrap().into(), w["origin"].as_str().unwrap().into())).collect();
        assert_eq!(origins, [("nginx".to_string(), "configured".to_string()), ("sbxw".into(), "replaces_built_in".into())]);
    }

    /// The desktop app's builtin/ as it writes it at each start (copies in
    /// crates/core/testdata/desktop-builtin).
    fn desktop_fixture(app: &crate::app::App) {
        let b = app.desktop_root().join("builtin");
        std::fs::create_dir_all(b.join("screens")).unwrap();
        for (f, text) in [
            ("screens/topbar.luau", include_str!("../../../core/testdata/desktop-builtin/screens/topbar.luau")),
            ("screens/sandboxes.luau", include_str!("../../../core/testdata/desktop-builtin/screens/sandboxes.luau")),
            ("screens/jarvis.luau", include_str!("../../../core/testdata/desktop-builtin/screens/jarvis.luau")),
            ("screens.md", include_str!("../../../core/testdata/desktop-builtin/screens.md")),
            ("intents.json", include_str!("../../../core/testdata/desktop-builtin/intents.json")),
        ] {
            std::fs::write(b.join(f), text).unwrap();
        }
    }

    /// A desktop screen: nothing until the app has run; then a click writes
    /// the override under sbxw's home (the folder created), a removal
    /// deletes it, and a built-in that moved since the proposal refuses it.
    #[tokio::test]
    async fn approving_a_desktop_screen_writes_the_override_and_removal_deletes_it() {
        let (_dir, app, _) = app_with(sbx_answer);
        let next = "return { actions = { go = function() return intent(\"stop_sandbox\", { sandbox = \"x\" }) end }, render = function() return text{ \"Open\" } end }\n";
        let screen = |content: &str| changes(json!([{"kind": "desktop_screen", "name": "topbar", "content": content}]));

        let state = app.jarvis_state().await;
        assert_eq!(state["desktop"]["app_ran"], false);
        assert!(state["desktop"]["note"].as_str().unwrap().contains("has not run"), "{}", state["desktop"]);
        assert!(app.jarvis_propose("top bar", screen(next)).await.unwrap_err().contains("has not run on this machine"));
        assert!(app.jarvis_desktop_docs().unwrap_err().contains("has not run"));

        desktop_fixture(&app);
        let state = app.jarvis_state().await;
        assert_eq!(state["desktop"]["screens"], json!([{"name": "jarvis", "override": false}, {"name": "sandboxes", "override": false}, {"name": "topbar", "override": false}]));
        assert_eq!(state["desktop"]["intents"][0], json!({"name": "approve_proposal", "risk": "approval"}));
        let docs = app.jarvis_desktop_docs().unwrap();
        assert_eq!((docs["screens_md"].as_str(), &docs["intents"]["intents"][0]["name"]), (Some(include_str!("../../../core/testdata/desktop-builtin/screens.md")), &json!("open_terminal")));
        let read = app.jarvis_desktop_screen("topbar").unwrap();
        assert_eq!((&read["override"], read["content"] == read["builtin"]), (&json!(false), true));
        assert!(app.jarvis_desktop_screen("nope").unwrap_err().contains("no 'nope' screen"));

        let p = app.jarvis_propose("top bar", screen(next)).await.unwrap();
        assert_eq!(p.risk, sbxw_core::jarvis::Risk::High);
        assert!(p.views[0].diff.contains("stop_sandbox (risk: direct), NEW"), "{}", p.views[0].diff);
        let file = app.desktop_root().join("screens/topbar.luau");
        assert!(!file.parent().unwrap().exists(), "nothing written, not even the folder, before approval");
        assert!(matches!(app.jarvis_approve(&p.id, Via::Voice, false).await, Err(sbxw_core::jarvis::ApproveError::Conflict(_))));
        let done = app.jarvis_approve(&p.id, Via::Click, false).await.unwrap();
        assert_eq!(done.state, State::Applied, "{:?}", done.error);
        assert_eq!(std::fs::read_to_string(&file).unwrap(), next);
        assert_eq!(app.jarvis_state().await["desktop"]["screens"][2], json!({"name": "topbar", "override": true}));
        let read = app.jarvis_desktop_screen("topbar").unwrap();
        assert_eq!((&read["override"], read["content"].as_str()), (&json!(true), Some(next)));

        // The built-in moves between proposal and approval: nothing written.
        let p = app.jarvis_propose("again", screen("return {}\n")).await.unwrap();
        std::fs::write(app.desktop_root().join("builtin/screens/topbar.luau"), "return { render = function() return text{ \"v2\" } end }\n").unwrap();
        let done = app.jarvis_approve(&p.id, Via::Click, false).await.unwrap();
        assert_eq!(done.state, State::Failed);
        assert!(done.error.as_deref().unwrap().contains("changed since"), "{:?}", done.error);
        assert_eq!(std::fs::read_to_string(&file).unwrap(), next);

        // Removal: the file goes, the built-in is back.
        let p = app.jarvis_propose("back to the built-in", changes(json!([{"kind": "desktop_screen_remove", "name": "topbar"}]))).await.unwrap();
        assert!(p.views[0].title.contains("restores the built-in"), "{}", p.views[0].title);
        let done = app.jarvis_approve(&p.id, Via::Click, false).await.unwrap();
        assert_eq!(done.state, State::Applied, "{:?}", done.error);
        assert!(!file.exists());
        assert_eq!(app.jarvis_desktop_screen("topbar").unwrap()["override"], false);
    }
}
