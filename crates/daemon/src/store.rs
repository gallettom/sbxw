//! Everything sbxw keeps on the host, in one place.
//!
//! The old layout grew a file per concern — `<name>.workspace`,
//! `<name>.model-migrated`, `threads/<name>.json`, `pending-hosts`, … — and
//! two teardown paths that had to enumerate them by hand (one of them forgot
//! `env/<name>/` once). Here a sandbox is **one record**,
//! `state/sandboxes/<name>.json`, and forgetting it is one call.
//!
//! Every write goes to a temporary file renamed over the target, so a crash
//! leaves the previous version; files are `0600` in `0700` directories, since a
//! record holds the sandbox's agent token and the kits directory an OAuth token.
//!
//! A record missing from the new layout is rebuilt from the old files on first
//! read, so sandboxes created by an older sbxw keep their workspace, threads
//! and model migration.
//!
//! Everything is keyed by a `SandboxRef`. A local sandbox's files are where
//! they always were; a remote host's go under `state/hosts/<host>/`, with the
//! same layout inside (`sandboxes/<name>.json`, `kits/<name>-oauth/`, …).

use anyhow::{Context, Result};
use sbxw_core::hosts::HostAlias;
use sbxw_core::{HostName, SandboxName, SandboxRef};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Where everything lives. Built once, passed down; tests point it at a temp dir.
#[derive(Clone, Debug)]
pub struct Paths {
    /// `~/.sbxw`
    pub root: PathBuf,
    /// Where chat sandboxes get their empty workspace. `/tmp`, not the OS temp
    /// dir: on macOS that is `/var/folders/…`, which Docker Desktop does not
    /// share by default.
    pub chat_root: PathBuf,
}

impl Paths {
    pub fn from_home() -> Self {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        Self {
            root: home.join(".sbxw"),
            chat_root: PathBuf::from("/tmp/sbxw-chat"),
        }
    }

    pub fn under(dir: &Path) -> Self {
        Self {
            root: dir.join(".sbxw"),
            chat_root: dir.join("chat"),
        }
    }

    pub fn state(&self) -> PathBuf {
        self.root.join("state")
    }
    /// The daemon's own files: pid, log.
    pub fn run(&self) -> PathBuf {
        self.root.join("run")
    }
    pub fn daemon_pid(&self) -> PathBuf {
        self.run().join("daemon.pid")
    }
    pub fn daemon_log(&self) -> PathBuf {
        self.run().join("daemon.log")
    }
    /// Where a host's sandboxes are filed: `state/` itself for this machine
    /// (the layout every older sbxw wrote), `state/hosts/<host>/` for another.
    pub fn host_state(&self, host: &HostName) -> PathBuf {
        if host.is_local() {
            self.state()
        } else {
            self.state().join("hosts").join(host.as_str())
        }
    }
    fn hosts(&self) -> PathBuf {
        self.state().join("hosts")
    }
    fn sandboxes(&self, host: &HostName) -> PathBuf {
        self.host_state(host).join("sandboxes")
    }
    pub fn plans(&self) -> PathBuf {
        self.state().join("plans")
    }
    /// `state/kits/<name>-oauth/`: kept as long as the sandbox, because sbx
    /// re-resolves a kit by its original path on every container swap.
    pub fn oauth_kit(&self, sb: impl Into<SandboxRef>) -> PathBuf {
        let sb = sb.into();
        self.host_state(&sb.host).join("kits").join(format!("{}-oauth", sb.name))
    }
    /// `state/kits/<name>-volumes/`: **not** removed with the sandbox. sbx
    /// keeps volume storage after `rm` and reattaches it to the same name
    /// created with the same kit, so the kit has to be there, unchanged.
    pub fn volumes_kit(&self, sb: impl Into<SandboxRef>) -> PathBuf {
        let sb = sb.into();
        self.host_state(&sb.host).join("kits").join(format!("{}-volumes", sb.name))
    }
    /// Where the bundled kits are written out: stable, because sbx keeps a
    /// kit's path and re-resolves it on every container swap.
    pub fn kits(&self) -> PathBuf {
        self.root.join("kits")
    }
    /// Downloads the bundled kits share between sandboxes (read-only in them).
    pub fn cache(&self) -> PathBuf {
        self.root.join("cache")
    }
    /// The skills the person kept from a sandbox, one folder each: data on
    /// the host, installed in new sandboxes by the kit below.
    pub fn my_skills(&self) -> PathBuf {
        self.root.join("skills")
    }
    /// `state/kits/sbxw-my-skills/`: one kit for every sandbox, rewritten when
    /// a skill is kept or dropped. sbx re-reads it on a container swap, so an
    /// older sandbox picks up the new list then too.
    pub fn my_skills_kit(&self) -> PathBuf {
        self.state().join("kits").join(sbxw_core::my_skills::KIT_NAME)
    }
    /// Documents `sbxw env run` derives for one sandbox.
    pub fn env_dir(&self, sb: impl Into<SandboxRef>) -> PathBuf {
        let sb = sb.into();
        self.host_state(&sb.host).join("env").join(sb.name.as_str())
    }
    /// A chat sandbox's empty workspace. Chats are made on this machine.
    pub fn chat_workspace(&self, name: &SandboxName) -> PathBuf {
        self.chat_root.join(name.as_str())
    }
    fn templates(&self) -> PathBuf {
        self.state().join("templates.json")
    }

    /// A host's automatic templates: an image lives on the host that built
    /// it, so each host keeps its own list. For this machine it is
    /// [`Paths::templates`] itself, so local storage is unchanged.
    fn templates_of(&self, host: &HostName) -> PathBuf {
        self.host_state(host).join("templates.json")
    }
    fn favourites(&self) -> PathBuf {
        self.state().join("favourites.json")
    }
    fn pairings(&self) -> PathBuf {
        self.state().join("embed-pairings.json")
    }
    fn pending_hosts(&self) -> PathBuf {
        self.state().join("pending-hosts")
    }
}

/// Which repo a thread works in, and the model it was first opened on.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadRecord {
    pub repo: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// A session in the main checkout, beside the main agent, rather than in
    /// a worktree of its own: no worktree, no branch, the same files.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub same_worktree: bool,
}

/// A directory mounted beside the workspace, at the same path inside.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MountRecord {
    pub path: PathBuf,
    pub readonly: bool,
}

/// Everything sbxw knows about one sandbox.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SandboxRecord {
    /// The host directory it was created on.
    #[serde(default)]
    pub workspace: Option<PathBuf>,
    /// The `sbxw.toml` it was brought up with — its `env`, model and policy.
    #[serde(default)]
    pub config: Option<PathBuf>,
    /// A throwaway chat sandbox: its workspace is deleted with it.
    #[serde(default)]
    pub chat: bool,
    /// The agent token installed in it.
    #[serde(default)]
    pub token: Option<String>,
    #[serde(default)]
    pub threads: BTreeMap<String, ThreadRecord>,
    /// The repos mounted beside the workspace at creation, the only moment
    /// sbx applies them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mounts: Vec<MountRecord>,
    /// The pre-`ANTHROPIC_DEFAULT_MODEL` settings key has been dealt with.
    #[serde(default)]
    pub model_migrated: bool,
    /// Kit references sbxw applied — at creation or by `kit add` — so a
    /// bring-up never re-applies one (each `kit add` recreates the container).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub kits: Vec<String>,
    /// The `localhost:<UI port>` rule an older sbxw added has been looked for.
    /// A sandbox this sbxw created never had it.
    #[serde(default)]
    pub ui_rule_checked: bool,
    /// Created for Remote Control: sbxw never signs its agent in — no OAuth
    /// kit, no auth variables — so it logs in to claude.ai itself.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub remote_control: bool,
}

#[derive(Debug)]
pub struct Store {
    pub paths: Paths,
    lock: Mutex<()>,
    tokens: Mutex<HashMap<String, SandboxRef>>,
}

fn restrict(path: &Path, mode: u32) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode));
    }
    #[cfg(not(unix))]
    let _ = (path, mode);
}

/// Write `bytes` to `path` through a private temporary file and a rename.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let dir = path.parent().context("a file needs a directory")?;
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    restrict(dir, 0o700);
    let tmp = dir.join(format!(
        ".{}.{}.tmp",
        path.file_name().and_then(|n| n.to_str()).unwrap_or("file"),
        std::process::id()
    ));
    std::fs::write(&tmp, bytes).with_context(|| format!("writing {}", tmp.display()))?;
    restrict(&tmp, 0o600);
    std::fs::rename(&tmp, path).with_context(|| format!("replacing {}", path.display()))?;
    Ok(())
}

fn read_json<T: for<'de> Deserialize<'de> + Default>(path: &Path) -> T {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

impl Store {
    pub fn open(paths: Paths) -> Self {
        let store = Self {
            paths,
            lock: Mutex::new(()),
            tokens: Mutex::new(HashMap::new()),
        };
        store.index_tokens();
        store
    }

    fn record_path(&self, sb: &SandboxRef) -> PathBuf {
        self.paths.sandboxes(&sb.host).join(format!("{}.json", sb.name))
    }

    /// The hosts that have a directory of records: this machine, then every
    /// `state/hosts/<host>/` whose name is a valid host.
    fn hosts(&self) -> Vec<HostName> {
        let mut out = vec![HostName::local()];
        let mut remote: Vec<HostName> = std::fs::read_dir(self.paths.hosts())
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| HostName::parse(e.file_name().to_str()?).ok())
            .filter(|h| !h.is_local())
            .collect();
        remote.sort();
        out.extend(remote);
        out
    }

    /// The refs with a record on `host`, by file name.
    fn records_of(&self, host: &HostName) -> Vec<SandboxRef> {
        std::fs::read_dir(self.paths.sandboxes(host))
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| {
                let p = e.path();
                if p.extension()? != "json" {
                    return None;
                }
                let name = SandboxName::parse(p.file_stem()?.to_str()?).ok()?;
                Some(SandboxRef::new(host.clone(), name))
            })
            .collect()
    }

    fn index_tokens(&self) {
        let mut idx = HashMap::new();
        for host in self.hosts() {
            for sb in self.records_of(&host) {
                let rec: SandboxRecord = read_json(&self.record_path(&sb));
                if let Some(t) = rec.token {
                    idx.insert(t, sb);
                }
            }
        }
        *self.tokens.lock().unwrap() = idx;
    }

    /// The record, rebuilt from the old layout when there is no new one.
    pub fn get(&self, sb: impl Into<SandboxRef>) -> SandboxRecord {
        let sb = sb.into();
        let path = self.record_path(&sb);
        if path.exists() {
            return read_json(&path);
        }
        if sb.is_local() {
            return self.legacy(&sb.name);
        }
        SandboxRecord::default()
    }

    pub fn exists(&self, sb: impl Into<SandboxRef>) -> bool {
        let sb = sb.into();
        self.record_path(&sb).exists()
            || (sb.is_local() && self.paths.state().join(format!("{}.workspace", sb.name)).exists())
    }

    /// Change a record and write it back.
    pub fn update<T>(&self, sb: impl Into<SandboxRef>, f: impl FnOnce(&mut SandboxRecord) -> T) -> Result<T> {
        let sb = sb.into();
        let _guard = self.lock.lock().unwrap();
        let mut rec = self.get(&sb);
        let before_token = rec.token.clone();
        let out = f(&mut rec);
        write_atomic(&self.record_path(&sb), &serde_json::to_vec_pretty(&rec)?)?;
        if rec.token != before_token {
            let mut idx = self.tokens.lock().unwrap();
            if let Some(old) = before_token {
                idx.remove(&old);
            }
            if let Some(new) = &rec.token {
                idx.insert(new.clone(), sb.clone());
            }
        }
        Ok(out)
    }

    /// The sandbox a presented agent token belongs to.
    ///
    /// The index is a cache of the records on disk, which another process may
    /// have changed (the CLI mints a token under `--no-web`, `sbxw rm` forgets
    /// one): a hit is confirmed against the record, and a miss or a mismatch
    /// re-reads the records once.
    pub fn sandbox_for_token(&self, token: &str) -> Option<SandboxRef> {
        let lookup = |s: &Self| {
            let idx = s.tokens.lock().unwrap();
            idx.iter()
                .find(|(t, _)| sbxw_core::ids::same_secret(t, token))
                .map(|(_, name)| name.clone())
        };
        let confirmed = |sb: &SandboxRef| {
            self.get(sb).token.is_some_and(|t| sbxw_core::ids::same_secret(&t, token))
        };
        match lookup(self) {
            Some(n) if confirmed(&n) => Some(n),
            _ => {
                self.index_tokens();
                lookup(self).filter(|n| confirmed(n))
            }
        }
    }

    /// This sandbox's token, minted on first use.
    pub fn token(&self, sb: impl Into<SandboxRef>) -> Result<String> {
        let sb = sb.into();
        if let Some(t) = self.get(&sb).token {
            return Ok(t);
        }
        self.update(&sb, |r| r.token.get_or_insert_with(sbxw_core::ids::agent_token).clone())
    }

    pub fn workspace(&self, sb: impl Into<SandboxRef>) -> Option<PathBuf> {
        self.get(sb).workspace
    }

    /// The chat workspace this sandbox owns — its own directory under the chat
    /// root, named after it. A duplicate of a chat sandbox points at the
    /// source's directory and is not a chat any more. Only a local sandbox
    /// has one.
    pub fn chat_workspace(&self, sb: impl Into<SandboxRef>) -> Option<PathBuf> {
        let sb = sb.into();
        if !sb.is_local() {
            return None;
        }
        self.workspace(&sb)
            .filter(|w| w.starts_with(&self.paths.chat_root))
            .filter(|w| w.file_name().is_some_and(|n| n == sb.name.as_str()))
    }

    /// Everything kept about a sandbox, gone. Best effort: the sandbox is
    /// already removed when this runs.
    pub fn forget(&self, sb: impl Into<SandboxRef>) {
        let sb = sb.into();
        let _guard = self.lock.lock().unwrap();
        if let Some(t) = self.get(&sb).token {
            self.tokens.lock().unwrap().remove(&t);
        }
        for dir in [self.paths.oauth_kit(&sb), self.paths.env_dir(&sb)] {
            let _ = std::fs::remove_dir_all(dir);
        }
        let _ = std::fs::remove_file(self.record_path(&sb));
        if sb.is_local() {
            let s = self.paths.state();
            let name = &sb.name;
            for file in [
                s.join(format!("{name}.workspace")),
                s.join(format!("{name}.model-migrated")),
                s.join("threads").join(format!("{name}.json")),
            ] {
                let _ = std::fs::remove_file(file);
            }
        }
    }

    /// Every sandbox that has a record, local ones first, each host sorted.
    pub fn names(&self) -> Vec<SandboxRef> {
        let mut out = Vec::new();
        for host in self.hosts() {
            let mut refs = self.records_of(&host);
            refs.sort();
            out.extend(refs);
        }
        out
    }

    /// A record rebuilt from the files an older sbxw wrote (local only: no
    /// older sbxw knew another host).
    fn legacy(&self, name: &SandboxName) -> SandboxRecord {
        let s = self.paths.state();
        let workspace = std::fs::read_to_string(s.join(format!("{name}.workspace")))
            .ok()
            .map(|w| w.trim().to_string())
            .filter(|w| !w.is_empty())
            .map(PathBuf::from);
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Old {
            Repo(String),
            Full(ThreadRecord),
        }
        let threads: BTreeMap<String, Old> = read_json(&s.join("threads").join(format!("{name}.json")));
        let chat = workspace
            .as_ref()
            .is_some_and(|w| w.starts_with(&self.paths.chat_root) && w.file_name().is_some_and(|n| n == name.as_str()));
        SandboxRecord {
            workspace,
            config: None,
            chat,
            token: None,
            mounts: vec![],
            threads: threads
                .into_iter()
                .map(|(k, v)| {
                    let rec = match v {
                        Old::Repo(repo) => ThreadRecord { repo, ..Default::default() },
                        Old::Full(r) => r,
                    };
                    (k, rec)
                })
                .collect(),
            model_migrated: s.join(format!("{name}.model-migrated")).exists(),
            kits: vec![],
            ui_rule_checked: false,
            remote_control: false,
        }
    }

    /// The automatic templates (`template = "auto"`), one per kit list.
    pub fn templates(&self) -> Vec<sbxw_core::templates::Record> {
        read_json(&self.paths.templates())
    }

    pub fn set_templates(&self, list: &[sbxw_core::templates::Record]) -> Result<()> {
        write_atomic(&self.paths.templates(), &serde_json::to_vec_pretty(list)?)
    }

    /// [`Store::templates`] of one host; `local` is the same list.
    pub fn templates_of(&self, host: &HostName) -> Vec<sbxw_core::templates::Record> {
        read_json(&self.paths.templates_of(host))
    }

    pub fn set_templates_of(&self, host: &HostName, list: &[sbxw_core::templates::Record]) -> Result<()> {
        write_atomic(&self.paths.templates_of(host), &serde_json::to_vec_pretty(list)?)
    }

    pub fn favourites(&self) -> Vec<String> {
        read_json(&self.paths.favourites())
    }

    pub fn set_favourites(&self, list: &[String]) -> Result<()> {
        write_atomic(&self.paths.favourites(), &serde_json::to_vec_pretty(list)?)
    }

    pub fn pairings(&self) -> BTreeMap<String, String> {
        read_json(&self.paths.pairings())
    }

    pub fn set_pairings(&self, p: &BTreeMap<String, String>) -> Result<()> {
        write_atomic(&self.paths.pairings(), &serde_json::to_vec_pretty(p)?)
    }

    /// Aliases a daemon could not write, parked for `sbxw hosts sync`. Same
    /// `<ip>\t<hostname>` lines as the hosts block, readable by eye.
    pub fn pending_hosts(&self) -> Vec<HostAlias> {
        std::fs::read_to_string(self.paths.pending_hosts())
            .unwrap_or_default()
            .lines()
            .filter_map(|l| {
                let mut p = l.split_whitespace();
                Some(HostAlias::new(p.nth(1)?, l.split_whitespace().next()?))
            })
            .collect()
    }

    pub fn park_hosts(&self, aliases: &[HostAlias]) -> Result<()> {
        let mut all = self.pending_hosts();
        for a in aliases.iter().filter(|a| !a.hostname.is_empty()) {
            if !all.contains(a) {
                all.push(a.clone());
            }
        }
        let body: String = all.iter().map(|a| format!("{}\t{}\n", a.ip, a.hostname)).collect();
        write_atomic(&self.paths.pending_hosts(), body.as_bytes())
    }

    pub fn clear_pending_hosts(&self) {
        let _ = std::fs::remove_file(self.paths.pending_hosts());
    }

    /// Write the volumes kit for `name`, rewritten only when its list changed
    /// (a rewrite is another kit to sbx, whose storage it would not find).
    pub fn write_volumes_kit(&self, sb: impl Into<SandboxRef>, paths: &[String]) -> Result<PathBuf> {
        let dir = self.paths.volumes_kit(sb);
        let spec = sbxw_core::kits::volumes_kit_spec(paths);
        let file = dir.join("spec.yaml");
        if std::fs::read_to_string(&file).ok().as_deref() != Some(spec.as_str()) {
            write_atomic(&file, spec.as_bytes())?;
        }
        Ok(dir)
    }

    /// Write the OAuth mixin kit for `name` (0600 spec in a 0700 directory),
    /// at the stable path sbx will re-resolve.
    pub fn write_oauth_kit(&self, sb: impl Into<SandboxRef>, credentials_json: Option<&str>) -> Result<PathBuf> {
        let dir = self.paths.oauth_kit(sb);
        write_atomic(&dir.join("spec.yaml"), sbxw_core::kits::oauth_kit_spec(credentials_json).as_bytes())?;
        Ok(dir)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(raw: &str) -> SandboxRef {
        SandboxRef::parse(raw).unwrap()
    }

    fn store() -> (tempfile::TempDir, Store) {
        let d = tempfile::tempdir().unwrap();
        let s = Store::open(Paths::under(d.path()));
        (d, s)
    }

    #[test]
    fn a_record_round_trips_and_indexes_its_token() {
        let (_d, s) = store();
        s.update(r("neos"), |r| r.workspace = Some("/w".into())).unwrap();
        let t = s.token(r("neos")).unwrap();
        assert_eq!(s.token(r("neos")).unwrap(), t, "minted once");
        assert_eq!(s.sandbox_for_token(&t).unwrap(), r("neos"));
        assert!(s.sandbox_for_token("sbxw_nope").is_none());
        let reopened = Store::open(s.paths.clone());
        assert_eq!(reopened.sandbox_for_token(&t).unwrap(), r("neos"));
    }

    #[test]
    fn a_token_another_process_minted_or_forgot_is_seen() {
        let (_d, daemon) = store();
        let cli = Store::open(daemon.paths.clone());
        let t = cli.token(r("neos")).unwrap();
        assert_eq!(daemon.sandbox_for_token(&t).unwrap(), r("neos"), "minted elsewhere");
        cli.forget(r("neos"));
        assert!(daemon.sandbox_for_token(&t).is_none(), "forgotten elsewhere");
    }

    #[test]
    fn forgetting_removes_every_trace() {
        let (_d, s) = store();
        let t = s.token(r("neos")).unwrap();
        s.write_oauth_kit(r("neos"), Some("{}")).unwrap();
        std::fs::create_dir_all(s.paths.env_dir(r("neos"))).unwrap();
        s.write_volumes_kit(r("neos"), &["/home/agent/.npm".into()]).unwrap();
        s.forget(r("neos"));
        assert!(!s.exists(r("neos")));
        assert!(!s.paths.oauth_kit(r("neos")).exists());
        assert!(!s.paths.env_dir(r("neos")).exists());
        assert!(s.sandbox_for_token(&t).is_none());
        assert!(s.paths.volumes_kit(r("neos")).join("spec.yaml").exists(), "sbx reattaches the volumes to the same kit");
    }

    #[test]
    fn an_old_layout_is_read_back() {
        let (_d, s) = store();
        let st = s.paths.state();
        std::fs::create_dir_all(st.join("threads")).unwrap();
        std::fs::write(st.join("neos.workspace"), "/w/neos\n").unwrap();
        std::fs::write(st.join("neos.model-migrated"), "").unwrap();
        std::fs::write(
            st.join("threads/neos.json"),
            r#"{"api":"backend","ui":{"repo":"","model":"claude-opus-5-5"}}"#,
        )
        .unwrap();
        let r = s.get(r("neos"));
        assert_eq!(r.workspace.as_deref(), Some(Path::new("/w/neos")));
        assert!(r.model_migrated);
        assert_eq!(r.threads["api"].repo, "backend");
        assert_eq!(r.threads["ui"].model.as_deref(), Some("claude-opus-5-5"));
    }

    #[test]
    fn files_are_private() {
        let (_d, s) = store();
        s.token(r("neos")).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let f = s.record_path(&r("neos"));
            assert_eq!(std::fs::metadata(&f).unwrap().permissions().mode() & 0o777, 0o600);
            let d = f.parent().unwrap();
            assert_eq!(std::fs::metadata(d).unwrap().permissions().mode() & 0o777, 0o700);
        }
    }

    #[test]
    fn chat_workspaces_are_their_own_directory() {
        let (_d, s) = store();
        let own = s.paths.chat_workspace(&r("chat-1").name);
        s.update(r("chat-1"), |r| r.workspace = Some(own.clone())).unwrap();
        s.update(r("dup"), |r| r.workspace = Some(own.clone())).unwrap();
        assert_eq!(s.chat_workspace(r("chat-1")), Some(own));
        assert_eq!(s.chat_workspace(r("dup")), None);
    }

    #[test]
    fn a_remote_sandbox_is_filed_under_its_host() {
        let (_d, s) = store();
        let local = r("neos");
        let remote = r("buildbox~neos");
        s.update(&local, |r| r.workspace = Some("/w/local".into())).unwrap();
        s.update(&remote, |r| r.workspace = Some("/w/remote".into())).unwrap();
        let st = s.paths.state();
        assert!(st.join("sandboxes/neos.json").exists(), "local layout unchanged");
        assert!(st.join("hosts/buildbox/sandboxes/neos.json").exists());
        assert_eq!(s.workspace(&local).as_deref(), Some(Path::new("/w/local")));
        assert_eq!(s.workspace(&remote).as_deref(), Some(Path::new("/w/remote")), "same name, separate record");
        assert_eq!(s.names(), vec![local.clone(), remote.clone()], "local first, then each host");

        let t = s.token(&remote).unwrap();
        assert_eq!(s.sandbox_for_token(&t), Some(remote.clone()));
        let reopened = Store::open(s.paths.clone());
        assert_eq!(reopened.sandbox_for_token(&t), Some(remote.clone()), "indexed from the host's directory");

        s.write_oauth_kit(&remote, None).unwrap();
        std::fs::create_dir_all(s.paths.env_dir(&remote)).unwrap();
        assert!(st.join("hosts/buildbox/kits/neos-oauth/spec.yaml").exists());
        assert!(st.join("hosts/buildbox/env/neos").exists());
        assert_eq!(s.paths.oauth_kit(&local), st.join("kits/neos-oauth"), "local kits unchanged");

        s.forget(&remote);
        assert!(!s.exists(&remote));
        assert!(!st.join("hosts/buildbox/kits/neos-oauth").exists());
        assert!(s.sandbox_for_token(&t).is_none());
        assert!(s.exists(&local), "forgetting the remote one leaves the local one");
        assert_eq!(s.chat_workspace(&remote), None);
    }

    #[test]
    fn templates_are_kept_per_host_and_local_is_unchanged() {
        use sbxw_core::templates::Record;
        let (d, s) = store();
        let rec = |tag: &str| Record { tag: tag.into(), sbx: "1".into(), sbxw: "1".into(), kits: vec![], created_at: 0, content: String::new() };
        let box_ = HostName::parse("buildbox").unwrap();
        s.set_templates(&[rec("local")]).unwrap();
        s.set_templates_of(&box_, &[rec("remote")]).unwrap();
        assert_eq!(s.templates()[0].tag, "local");
        assert_eq!(s.templates_of(&HostName::local())[0].tag, "local");
        assert_eq!(s.templates_of(&box_)[0].tag, "remote");
        assert!(d.path().join(".sbxw/state/templates.json").exists());
        assert!(d.path().join(".sbxw/state/hosts/buildbox/templates.json").exists());
        assert!(s.templates_of(&HostName::parse("other").unwrap()).is_empty());
    }

    #[test]
    fn local_tilde_is_the_local_record() {
        let (_d, s) = store();
        s.update(r("local~neos"), |r| r.chat = true).unwrap();
        assert!(s.paths.state().join("sandboxes/neos.json").exists());
        assert!(s.get(r("neos")).chat);
    }

    #[test]
    fn pending_hosts_accumulate_without_duplicates() {
        let (_d, s) = store();
        s.park_hosts(&[HostAlias::new("a.local", "127.0.0.1")]).unwrap();
        s.park_hosts(&[HostAlias::new("a.local", "127.0.0.1"), HostAlias::new("b.local", "127.0.0.2")]).unwrap();
        assert_eq!(s.pending_hosts().len(), 2);
        s.clear_pending_hosts();
        assert!(s.pending_hosts().is_empty());
    }
}
