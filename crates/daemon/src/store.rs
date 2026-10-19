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

use anyhow::{Context, Result};
use sbxw_core::hosts::HostAlias;
use sbxw_core::SandboxName;
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
    fn sandboxes(&self) -> PathBuf {
        self.state().join("sandboxes")
    }
    pub fn plans(&self) -> PathBuf {
        self.state().join("plans")
    }
    /// `state/kits/<name>-oauth/`: kept as long as the sandbox, because sbx
    /// re-resolves a kit by its original path on every container swap.
    pub fn oauth_kit(&self, name: &str) -> PathBuf {
        self.state().join("kits").join(format!("{name}-oauth"))
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
    /// Documents `sbxw env run` derives for one sandbox.
    pub fn env_dir(&self, name: &str) -> PathBuf {
        self.state().join("env").join(name)
    }
    pub fn chat_workspace(&self, name: &str) -> PathBuf {
        self.chat_root.join(name)
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
    /// The pre-`ANTHROPIC_DEFAULT_MODEL` settings key has been dealt with.
    #[serde(default)]
    pub model_migrated: bool,
}

#[derive(Debug)]
pub struct Store {
    pub paths: Paths,
    lock: Mutex<()>,
    tokens: Mutex<HashMap<String, String>>,
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

    fn record_path(&self, name: &str) -> PathBuf {
        self.paths.sandboxes().join(format!("{name}.json"))
    }

    fn index_tokens(&self) {
        let mut idx = HashMap::new();
        if let Ok(dir) = std::fs::read_dir(self.paths.sandboxes()) {
            for e in dir.flatten() {
                let p = e.path();
                let Some(name) = p.file_stem().and_then(|s| s.to_str()) else { continue };
                if p.extension().is_none_or(|x| x != "json") {
                    continue;
                }
                let rec: SandboxRecord = read_json(&p);
                if let Some(t) = rec.token {
                    idx.insert(t, name.to_string());
                }
            }
        }
        *self.tokens.lock().unwrap() = idx;
    }

    /// The record, rebuilt from the old layout when there is no new one.
    pub fn get(&self, name: &str) -> SandboxRecord {
        let path = self.record_path(name);
        if path.exists() {
            return read_json(&path);
        }
        self.legacy(name)
    }

    pub fn exists(&self, name: &str) -> bool {
        self.record_path(name).exists() || self.paths.state().join(format!("{name}.workspace")).exists()
    }

    /// Change a record and write it back.
    pub fn update<T>(&self, name: &str, f: impl FnOnce(&mut SandboxRecord) -> T) -> Result<T> {
        let _guard = self.lock.lock().unwrap();
        let mut rec = self.get(name);
        let before_token = rec.token.clone();
        let out = f(&mut rec);
        write_atomic(&self.record_path(name), &serde_json::to_vec_pretty(&rec)?)?;
        if rec.token != before_token {
            let mut idx = self.tokens.lock().unwrap();
            if let Some(old) = before_token {
                idx.remove(&old);
            }
            if let Some(new) = &rec.token {
                idx.insert(new.clone(), name.to_string());
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
    pub fn sandbox_for_token(&self, token: &str) -> Option<SandboxName> {
        let lookup = |s: &Self| {
            let idx = s.tokens.lock().unwrap();
            idx.iter()
                .find(|(t, _)| sbxw_core::ids::same_secret(t, token))
                .map(|(_, name)| name.clone())
        };
        let confirmed = |name: &str| {
            self.get(name).token.is_some_and(|t| sbxw_core::ids::same_secret(&t, token))
        };
        let name = match lookup(self) {
            Some(n) if confirmed(&n) => n,
            _ => {
                self.index_tokens();
                lookup(self).filter(|n| confirmed(n))?
            }
        };
        SandboxName::parse(&name).ok()
    }

    /// This sandbox's token, minted on first use.
    pub fn token(&self, name: &str) -> Result<String> {
        if let Some(t) = self.get(name).token {
            return Ok(t);
        }
        self.update(name, |r| r.token.get_or_insert_with(sbxw_core::ids::agent_token).clone())
    }

    pub fn workspace(&self, name: &str) -> Option<PathBuf> {
        self.get(name).workspace
    }

    /// The chat workspace this sandbox owns — its own directory under the chat
    /// root, named after it. A duplicate of a chat sandbox points at the
    /// source's directory and is not a chat any more.
    pub fn chat_workspace(&self, name: &str) -> Option<PathBuf> {
        self.workspace(name)
            .filter(|w| w.starts_with(&self.paths.chat_root))
            .filter(|w| w.file_name().is_some_and(|n| n == name))
    }

    /// Everything kept about `name`, gone. Best effort: the sandbox is already
    /// removed when this runs.
    pub fn forget(&self, name: &str) {
        let _guard = self.lock.lock().unwrap();
        if let Some(t) = self.get(name).token {
            self.tokens.lock().unwrap().remove(&t);
        }
        for dir in [self.paths.oauth_kit(name), self.paths.env_dir(name)] {
            let _ = std::fs::remove_dir_all(dir);
        }
        let s = self.paths.state();
        for file in [
            self.record_path(name),
            s.join(format!("{name}.workspace")),
            s.join(format!("{name}.model-migrated")),
            s.join("threads").join(format!("{name}.json")),
        ] {
            let _ = std::fs::remove_file(file);
        }
    }

    /// Every name that has a record (or an old-layout workspace file).
    pub fn names(&self) -> Vec<String> {
        let mut out: Vec<String> = std::fs::read_dir(self.paths.sandboxes())
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| {
                let p = e.path();
                (p.extension()? == "json").then(|| p.file_stem()?.to_str().map(str::to_string))?
            })
            .collect();
        out.sort();
        out
    }

    /// A record rebuilt from the files an older sbxw wrote.
    fn legacy(&self, name: &str) -> SandboxRecord {
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
            .is_some_and(|w| w.starts_with(&self.paths.chat_root) && w.file_name().is_some_and(|n| n == name));
        SandboxRecord {
            workspace,
            config: None,
            chat,
            token: None,
            threads: threads
                .into_iter()
                .map(|(k, v)| {
                    let rec = match v {
                        Old::Repo(repo) => ThreadRecord { repo, model: None },
                        Old::Full(r) => r,
                    };
                    (k, rec)
                })
                .collect(),
            model_migrated: s.join(format!("{name}.model-migrated")).exists(),
        }
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

    /// Write the OAuth mixin kit for `name` (0600 spec in a 0700 directory),
    /// at the stable path sbx will re-resolve.
    pub fn write_oauth_kit(&self, name: &str, credentials_json: Option<&str>) -> Result<PathBuf> {
        let dir = self.paths.oauth_kit(name);
        write_atomic(&dir.join("spec.yaml"), sbxw_core::kits::oauth_kit_spec(credentials_json).as_bytes())?;
        Ok(dir)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (tempfile::TempDir, Store) {
        let d = tempfile::tempdir().unwrap();
        let s = Store::open(Paths::under(d.path()));
        (d, s)
    }

    #[test]
    fn a_record_round_trips_and_indexes_its_token() {
        let (_d, s) = store();
        s.update("neos", |r| r.workspace = Some("/w".into())).unwrap();
        let t = s.token("neos").unwrap();
        assert_eq!(s.token("neos").unwrap(), t, "minted once");
        assert_eq!(s.sandbox_for_token(&t).unwrap().as_str(), "neos");
        assert!(s.sandbox_for_token("sbxw_nope").is_none());
        let reopened = Store::open(s.paths.clone());
        assert_eq!(reopened.sandbox_for_token(&t).unwrap().as_str(), "neos");
    }

    #[test]
    fn a_token_another_process_minted_or_forgot_is_seen() {
        let (_d, daemon) = store();
        let cli = Store::open(daemon.paths.clone());
        let t = cli.token("neos").unwrap();
        assert_eq!(daemon.sandbox_for_token(&t).unwrap().as_str(), "neos", "minted elsewhere");
        cli.forget("neos");
        assert!(daemon.sandbox_for_token(&t).is_none(), "forgotten elsewhere");
    }

    #[test]
    fn forgetting_removes_every_trace() {
        let (_d, s) = store();
        let t = s.token("neos").unwrap();
        s.write_oauth_kit("neos", Some("{}")).unwrap();
        std::fs::create_dir_all(s.paths.env_dir("neos")).unwrap();
        s.forget("neos");
        assert!(!s.exists("neos"));
        assert!(!s.paths.oauth_kit("neos").exists());
        assert!(!s.paths.env_dir("neos").exists());
        assert!(s.sandbox_for_token(&t).is_none());
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
        let r = s.get("neos");
        assert_eq!(r.workspace.as_deref(), Some(Path::new("/w/neos")));
        assert!(r.model_migrated);
        assert_eq!(r.threads["api"].repo, "backend");
        assert_eq!(r.threads["ui"].model.as_deref(), Some("claude-opus-5-5"));
    }

    #[test]
    fn files_are_private() {
        let (_d, s) = store();
        s.token("neos").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let f = s.record_path("neos");
            assert_eq!(std::fs::metadata(&f).unwrap().permissions().mode() & 0o777, 0o600);
            let d = f.parent().unwrap();
            assert_eq!(std::fs::metadata(d).unwrap().permissions().mode() & 0o777, 0o700);
        }
    }

    #[test]
    fn chat_workspaces_are_their_own_directory() {
        let (_d, s) = store();
        let own = s.paths.chat_workspace("chat-1");
        s.update("chat-1", |r| r.workspace = Some(own.clone())).unwrap();
        s.update("dup", |r| r.workspace = Some(own.clone())).unwrap();
        assert_eq!(s.chat_workspace("chat-1"), Some(own));
        assert_eq!(s.chat_workspace("dup"), None);
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
