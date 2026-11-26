//! The sync sandbox, `sbxw-sync`: a sandbox of the daemon's own on this
//! machine where the ticket sync script (`sbxw_core::tracker::SCRIPT`) runs
//! with `node`, through sbx's MCP gateway, with the tracker's MCP server
//! loaded. The daemon holds no token and has no HTTPS client; this sandbox
//! is how it reaches a tracker. Modelled on Jarvis's (`svc::jarvis_sandbox`),
//! with less:
//!
//! * **No agent, ever.** Nobody opens a session there ([`App::spawn_here`]
//!   refuses it); it is held up with `sbx run --detached` and only `sbx exec`
//!   runs in it, always the one script.
//! * **Nothing of a project.** Its workspace is an empty directory under
//!   `~/.sbxw/state/tracker-sync/`: no project, no mount, no port, no
//!   environment, no kit but sbxw's base kit (Node.js), no skills store.
//! * **Small**: 1 CPU, 1 GB.
//! * **Hidden**: not in the sandbox list (`svc::sandboxes`); its name is
//!   reserved on every host, as Jarvis's is ([`crate::svc::jarvis_sandbox::reserved`]).
//! * **On while a source is.** Started when at least one sandbox has a
//!   tracker source on, its tracker's MCP server loaded after every start
//!   (whether a load survives a stop is not known), stopped when none is.
//!   Project sandboxes never get the tracker's MCP server.

use crate::app::App;
use crate::store::Paths;
use sbxw_core::kits;
use sbxw_core::tracker::{ErrorKind, Failure, Tracker};
use sbxw_core::{SandboxName, SandboxRef};
use sbxw_sbx::CreateOpts;
use std::collections::HashSet;
use std::path::PathBuf;
use std::time::Duration;

pub const NAME: &str = kits::SYNC_SANDBOX;
pub const CPUS: u32 = 1;
pub const MEMORY: &str = "1g";
/// How long a start may take before the run is a failure.
const BOOT: Duration = Duration::from_secs(90);

pub fn sandbox() -> SandboxRef {
    SandboxRef::local(SandboxName::parse(NAME).expect("sbxw-sync is a valid sandbox name"))
}

/// Is `sb` the sync sandbox by name (on this machine).
pub fn is_sync_name(sb: &SandboxRef) -> bool {
    sb.is_local() && sb.name.as_str() == NAME
}

/// Its empty workspace, under sbxw's state.
pub fn workspace(paths: &Paths) -> PathBuf {
    paths.state().join("tracker-sync").join("workspace")
}

/// Written once the daemon created the sandbox: one of that name made
/// otherwise (before the name was reserved) is never used.
fn marker(paths: &Paths) -> PathBuf {
    paths.state().join("tracker-sync").join("created")
}

/// What the daemon knows of the sync sandbox while it runs: which trackers'
/// servers were loaded since its last start.
#[derive(Default)]
pub struct SyncBox {
    loaded: HashSet<Tracker>,
}

fn sandbox_err(e: impl std::fmt::Display) -> Failure {
    Failure::new(ErrorKind::Sandbox, e.to_string())
}

impl App {
    /// The sync sandbox exists, runs, and has `t`'s MCP server loaded. The
    /// server must be registered in Setup, MCP: one missing is a
    /// `missing_server` failure, asked of sbx before anything starts.
    pub(crate) async fn sync_sandbox_up(&self, t: Tracker) -> Result<SandboxName, Failure> {
        let sb = sandbox();
        let n = sb.name.clone();
        let servers = self.sbx.mcp_ls().await.map_err(|e| sandbox_err(format!("sbx mcp ls: {e:#}")))?;
        if !servers.iter().any(|s| s.name == t.mcp_server()) {
            return Err(Failure::new(ErrorKind::MissingServer, format!("no MCP server named {}", t.mcp_server())));
        }
        let paths = &self.store.paths;
        match self.sbx.lookup(&n).await {
            Some(_) if !marker(paths).exists() => {
                return Err(sandbox_err(format!("a sandbox named '{NAME}' exists that sbxw did not create: `sbxw rm {NAME}`, then press Refresh")));
            }
            Some(i) if i.is_running() => {}
            Some(_) => {
                self.sync_box.lock().unwrap().loaded.clear();
                self.sbx.run_detached(&n).await.map_err(|e| sandbox_err(format!("{e:#}")))?;
                if !self.sbx.wait_until_running(&n, BOOT).await {
                    return Err(sandbox_err(format!("'{NAME}' did not start")));
                }
            }
            None => {
                self.sync_box.lock().unwrap().loaded.clear();
                self.sync_sandbox_create(t).await?;
            }
        }
        if !self.sync_box.lock().unwrap().loaded.contains(&t) {
            self.sbx.mcp_load(t.mcp_server(), &n).await.map_err(|e| sandbox_err(format!("loading the {} MCP server: {e:#}", t.title())))?;
            self.sync_box.lock().unwrap().loaded.insert(t);
        }
        Ok(n)
    }

    async fn sync_sandbox_create(&self, t: Tracker) -> Result<(), Failure> {
        let sb = sandbox();
        let n = &sb.name;
        let paths = &self.store.paths;
        let ws = workspace(paths);
        std::fs::create_dir_all(&ws).map_err(|e| sandbox_err(format!("creating {}: {e}", ws.display())))?;
        let kits = crate::bundled::resolve(paths, &[format!("sbxw:{}", kits::BASE)]).map_err(|e| sandbox_err(format!("{e:#}")))?;
        let opts = CreateOpts {
            workspace: ws.to_string_lossy().into_owned(),
            kits,
            // The person's skills store is not this sandbox's to read.
            skills: Some("off".into()),
            cpus: Some(CPUS),
            memory: Some(MEMORY.into()),
            ..Default::default()
        };
        tracing::info!("creating the ticket sync sandbox '{sb}'");
        self.sbx.create(n, &opts, None).await.map_err(|e| sandbox_err(format!("creating '{NAME}': {e:#}")))?;
        let _ = crate::store::write_atomic(&marker(paths), b"sbxw-sync\n");
        self.sbx.run_detached(n).await.map_err(|e| sandbox_err(format!("{e:#}")))?;
        if !self.sbx.wait_until_running(n, BOOT).await {
            return Err(sandbox_err(format!("'{NAME}' did not start")));
        }
        // Where the script may go besides the gateway: the tracker's own
        // hosts, for attachments. Best effort, as an organization may manage
        // the policy.
        if let Err(e) = self.sbx.policy("allow", Some(n), &t.hosts().join(",")).await {
            tracing::warn!("'{NAME}': the network allowlist was not applied: {e:#}");
        }
        Ok(())
    }

    /// Stops the sync sandbox when it runs (no source is on). Kept, not
    /// removed: the next source starts it again.
    pub(crate) async fn sync_sandbox_down(&self) {
        let n = sandbox().name;
        if !marker(&self.store.paths).exists() {
            return;
        }
        if self.sbx.lookup(&n).await.is_some_and(|i| i.is_running()) {
            tracing::info!("stopping the ticket sync sandbox '{NAME}': no tracker source is on");
            if let Err(e) = self.sbx.stop(&n).await {
                tracing::warn!("could not stop '{NAME}': {e:#}");
            }
        }
        self.sync_box.lock().unwrap().loaded.clear();
    }

    /// Runs the sync script once with `request` (JSON) as its one argument,
    /// bounded by `bound`. Its answer is data.
    pub(crate) async fn sync_script(&self, n: &SandboxName, request: &serde_json::Value, bound: Duration) -> Result<serde_json::Value, Failure> {
        let args = sbxw_sbx::scripts::tracker_args(&request.to_string());
        let argv: Vec<&str> = args.iter().map(String::as_str).collect();
        let run = self.sbx.exec(n, &argv, Some(sbxw_core::tracker::SCRIPT.as_bytes().to_vec()));
        match tokio::time::timeout(bound, run).await {
            Err(_) => Err(Failure::new(ErrorKind::Timeout, "the script did not answer in time")),
            Ok(Err(e)) => Err(sandbox_err(format!("{e:#}"))),
            Ok(Ok(out)) => sbxw_core::tracker::parse_answer(&out.stdout_str()),
        }
    }

    /// A file the script fetched, read out of the sync sandbox (at most
    /// `max + 1` bytes, so one too big shows) and removed there.
    pub(crate) async fn sync_read_out(&self, n: &SandboxName, path: &str, max: u64) -> Result<Vec<u8>, String> {
        if !sbxw_core::tracker::fetch_path_ok(path) {
            return Err("not a path the script writes".into());
        }
        let limit = (max + 1).to_string();
        let read = tokio::time::timeout(Duration::from_secs(60), self.sbx.exec(n, &["head", "-c", &limit, "--", path], None)).await;
        let _ = tokio::time::timeout(Duration::from_secs(20), self.sbx.exec(n, &["rm", "-f", "--", path], None)).await;
        match read {
            Err(_) => Err("reading the file took too long".into()),
            Ok(Err(e)) => Err(format!("{e:#}")),
            Ok(Ok(out)) if !out.success() => Err("the file could not be read".into()),
            Ok(Ok(out)) if out.stdout.len() as u64 > max => Err(format!("over {} MB", max >> 20)),
            Ok(Ok(out)) => Ok(out.stdout),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_name_is_reserved_and_the_workspace_is_empty_state() {
        let sb = sandbox();
        assert!(is_sync_name(&sb));
        assert!(crate::svc::jarvis_sandbox::reserved(&sb).is_some());
        let remote = SandboxRef::new(sbxw_core::names::HostName::parse("box").unwrap(), SandboxName::parse(NAME).unwrap());
        assert!(crate::svc::jarvis_sandbox::reserved(&remote).is_some() && !is_sync_name(&remote));
        let d = tempfile::tempdir().unwrap();
        let p = Paths::under(d.path());
        assert!(workspace(&p).starts_with(p.state()) && workspace(&p) != p.root);
    }
}
