//! The remote hosts the daemon drives (EXPERIMENTAL: never run against a
//! real KVM host yet). Per host: its [`Link`] (one SSH master, carrying the
//! `-R` tunnel the sandboxes' hooks come home through), a [`Host`] over that
//! link for commands and files, and the typed `sbx` client there.
//!
//! Built lazily, the first time a ref on that host is acted on, from
//! `~/.sbxw/hosts.toml`: a daemon with no remote hosts never runs ssh.
//!
//! A host whose link is down is "unreachable": its sandboxes stay listed with
//! the state last seen, and every action on them is refused with
//! [`HostUnreachable`] (HTTP 503) instead of waiting on ssh each time. The
//! link keeps retrying with backoff on its own.

use crate::app::App;
use crate::host::exec_fs::ExecFs;
use crate::host::link::{Link, LinkConfig, LinkState};
use crate::host::Host;
use anyhow::{Context, Result};
use sbxw_core::names::HostName;
use sbxw_core::remote_hosts::{RemoteHost, RemoteHosts};
use sbxw_core::SandboxRef;
use sbxw_sbx::host::{HostExec, HostUnreachable};
use sbxw_sbx::Sbx;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

/// How long the first action on a host waits for its link to come up. A
/// host that has already failed once is refused at once, not after this.
const FIRST_CONNECT: Duration = Duration::from_secs(20);

/// One remote host, as the daemon holds it.
pub struct Remote {
    pub cfg: RemoteHost,
    pub link: Link,
    pub host: Host,
    sbx: tokio::sync::OnceCell<Sbx>,
    /// The host's `~/.sbxw`, asked once.
    root: tokio::sync::OnceCell<PathBuf>,
}

impl Remote {
    fn start(cfg: RemoteHost, paths: &crate::store::Paths, agent_addr: &str, ssh: PathBuf) -> Self {
        let home = paths.root.parent().unwrap_or(Path::new("/"));
        let control = sbxw_sbx::ssh::control_path(home, &cfg.name);
        let mut lc = LinkConfig::new(cfg.clone(), control, agent_addr);
        lc.ssh = ssh;
        let link = Link::start(lc);
        let exec: Arc<dyn HostExec> = link.exec();
        let host = Host { name: cfg.name.clone(), exec: exec.clone(), fs: Arc::new(ExecFs::new(exec)) };
        Self { cfg, link, host, sbx: Default::default(), root: Default::default() }
    }

    /// `Some(since)` (unix seconds) while the link is down.
    pub fn down_since(&self) -> Option<u64> {
        match *self.link.state().borrow() {
            LinkState::Up => None,
            LinkState::Down { since } => Some(since),
        }
    }

    /// Ok when the link is up, after giving a link that has never tried its
    /// first connection the chance to; else the host is unreachable.
    pub async fn ready(&self) -> Result<()> {
        match self.link.settle(FIRST_CONNECT).await {
            LinkState::Up => Ok(()),
            LinkState::Down { since } => Err(HostUnreachable {
                host: self.cfg.name.to_string(),
                reason: format!("its SSH link has been down since {} (retrying in the background)", clock(since)),
            }
            .into()),
        }
    }

    /// The `sbx` client on this host (its `sbx` found once, on first use).
    pub async fn sbx(&self) -> Result<Sbx> {
        self.sbx
            .get_or_try_init(|| async { Ok::<_, anyhow::Error>(Sbx::new(Arc::new(self.link.exec().on_host().await?))) })
            .await
            .cloned()
    }

    /// `~/.sbxw` on the host: where kits are mirrored and the kit cache lives.
    pub async fn root(&self) -> Result<PathBuf> {
        self.root.get_or_try_init(|| async { Ok::<_, anyhow::Error>(self.host.exec.home().await?.join(".sbxw")) }).await.cloned()
    }
}

/// A ref names a host that is not in `hosts.toml` (HTTP 404).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownHost(pub HostName);

impl std::fmt::Display for UnknownHost {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "no remote host named '{0}' — add it with `sbxw host add {0}`", self.0)
    }
}

impl std::error::Error for UnknownHost {}

/// `HH:MM:SS` UTC, enough to tell "just now" from "an hour ago".
fn clock(unix: u64) -> String {
    let s = unix % 86_400;
    format!("{:02}:{:02}:{:02} UTC", s / 3600, (s / 60) % 60, s % 60)
}

pub fn hosts_file(paths: &crate::store::Paths) -> PathBuf {
    paths.root.join("hosts.toml")
}

/// The registry as on disk; none at all is an empty one.
pub fn load_hosts(paths: &crate::store::Paths) -> Result<RemoteHosts> {
    let p = hosts_file(paths);
    match std::fs::read_to_string(&p) {
        Ok(t) => RemoteHosts::parse(&t).with_context(|| p.display().to_string()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(RemoteHosts::default()),
        Err(e) => Err(e).with_context(|| p.display().to_string()),
    }
}

impl App {
    /// The remote host `name`, its link started if this is the first ask.
    pub async fn remote(&self, name: &HostName) -> Result<Arc<Remote>> {
        if let Some(r) = self.remotes.lock().unwrap().get(name) {
            return Ok(r.clone());
        }
        let hosts = load_hosts(&self.store.paths)?;
        let cfg = hosts
            .get(name.as_str())
            .cloned()
            .ok_or_else(|| anyhow::Error::new(UnknownHost(name.clone())))?;
        let mut map = self.remotes.lock().unwrap();
        // Two first asks at once: the one that got here first keeps its link.
        let r = map.entry(name.clone()).or_insert_with(|| {
            tracing::info!(target: "host", "{name}: starting the SSH link (remote hosts are experimental)");
            let ssh = self.ssh_program.lock().unwrap().clone();
            Arc::new(Remote::start(cfg, &self.store.paths, &self.cfg.agent_addr, ssh))
        });
        Ok(r.clone())
    }

    /// Remote hosts already started, without starting any.
    pub fn started_remotes(&self) -> Vec<Arc<Remote>> {
        let mut v: Vec<Arc<Remote>> = self.remotes.lock().unwrap().values().cloned().collect();
        v.sort_by(|a, b| a.cfg.name.cmp(&b.cfg.name));
        v
    }

    /// A ref's remote host, ready to act on (see [`Remote::ready`]).
    pub async fn ready_remote(&self, sb: &SandboxRef) -> Result<Arc<Remote>> {
        let r = self.remote(&sb.host).await?;
        r.ready().await?;
        Ok(r)
    }

    /// The `sbx` that manages `sb`: this machine's, or its host's over SSH.
    pub async fn sbx_for(&self, sb: &SandboxRef) -> Result<Sbx> {
        if sb.is_local() {
            return Ok(self.sbx.clone());
        }
        self.ready_remote(sb).await?.sbx().await
    }

    /// The machine `sb`'s workspace, git checkouts and kits are on.
    pub async fn host_for(&self, sb: &SandboxRef) -> Result<Host> {
        if sb.is_local() {
            return Ok(Host::local());
        }
        Ok(self.ready_remote(sb).await?.host.clone())
    }

    /// The port the in-sandbox hooks call on `host.docker.internal`: the
    /// daemon's agent port here, the far end of the `-R` tunnel on a host.
    pub async fn agent_port_for(&self, sb: &SandboxRef) -> Result<String> {
        if sb.is_local() {
            return Ok(self.cfg.agent_port().to_string());
        }
        Ok(self.remote(&sb.host).await?.cfg.agent_port.to_string())
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_clock_reads_utc() {
        assert_eq!(super::clock(3600 + 61), "01:01:01 UTC");
    }
}
