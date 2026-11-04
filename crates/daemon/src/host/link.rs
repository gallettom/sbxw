//! One SSH ControlMaster per host, shared by every command sent there, plus
//! its tunnels: `-R` so sandboxes on the host reach this daemon's agent
//! listener, `-L` for ports published on the host.
//!
//! A [`Link`] owns a supervisor task that runs the master
//! (`ssh -M -N -S <ControlPath> -R 127.0.0.1:<agent_port>:<agent_addr> …`),
//! says when it is up (`ssh -O check` answers), and restarts it with backoff
//! when it dies: a laptop that sleeps or changes network gets its link back
//! without anyone asking. Commands do not wait for it: the [`SshExec`] from
//! [`Link::exec`] rides the master when there is one and connects on its own
//! when there is not.
//!
//! `-L` forwards are added to the running master with `ssh -O forward`, and
//! remembered, so a restarted master gets them back on the same local ports.

use anyhow::{bail, Result};
use sbxw_core::remote_hosts::RemoteHost;
use sbxw_sbx::host::{HostCmd, HostExec, HostUnreachable, LocalExec};
use sbxw_sbx::runner::Output;
use sbxw_sbx::ssh::{SshExec, SSH_UNREACHABLE};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::watch;
use tokio::task::JoinHandle;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkState {
    /// The master is running and answers `-O check`.
    Up,
    /// Not connected since `since` (unix seconds): never was, or dropped then.
    Down { since: u64 },
}

/// Ports tried after the one asked for, when it is taken locally.
const PORT_TRIES: u16 = 100;
/// How long a fresh master gets to answer `-O check`.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
/// How long a control command (`-O …`) may take: it talks to a local socket.
const CONTROL_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Clone, Debug)]
pub struct LinkConfig {
    pub host: RemoteHost,
    /// From `sbxw_sbx::ssh::control_path`.
    pub control_path: PathBuf,
    /// This daemon's agent listener, `addr:port`, the far side of `-R`.
    pub agent_addr: String,
    /// The ssh program; tests put `scripts/fake-ssh` here.
    pub ssh: PathBuf,
    pub backoff_min: Duration,
    pub backoff_max: Duration,
}

impl LinkConfig {
    pub fn new(host: RemoteHost, control_path: PathBuf, agent_addr: impl Into<String>) -> Self {
        Self {
            host,
            control_path,
            agent_addr: agent_addr.into(),
            ssh: "ssh".into(),
            backoff_min: Duration::from_secs(1),
            backoff_max: Duration::from_secs(30),
        }
    }
}

struct Shared {
    cfg: LinkConfig,
    state: watch::Sender<LinkState>,
    /// local port → remote port, for `-O cancel` and for replay on restart.
    forwards: Mutex<BTreeMap<u16, u16>>,
}

/// The link to one host. Dropping it stops the master.
pub struct Link {
    shared: Arc<Shared>,
    exec: Arc<SshExec>,
    task: JoinHandle<()>,
}

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn l_spec(local: u16, remote: u16) -> String {
    format!("127.0.0.1:{local}:127.0.0.1:{remote}")
}

impl Link {
    /// Start the supervisor (needs a tokio runtime). Down until the master answers.
    pub fn start(cfg: LinkConfig) -> Self {
        let exec = Arc::new(SshExec::with_ssh(cfg.host.clone(), cfg.control_path.clone(), cfg.ssh.clone()));
        let (state, _) = watch::channel(LinkState::Down { since: now_secs() });
        let shared = Arc::new(Shared { cfg, state, forwards: Mutex::new(BTreeMap::new()) });
        let task = tokio::spawn(supervise(shared.clone()));
        Self { shared, exec, task }
    }

    pub fn state(&self) -> watch::Receiver<LinkState> {
        self.shared.state.subscribe()
    }

    /// Commands on this host, through this link's ControlPath.
    pub fn exec(&self) -> Arc<SshExec> {
        self.exec.clone()
    }

    /// Forward `127.0.0.1:<L>` here to `127.0.0.1:<remote_port>` on the host,
    /// L being `remote_port` when free locally, else the next free one.
    /// Asking again for the same remote port returns the same L.
    pub async fn forward(&self, remote_port: u16) -> Result<u16> {
        if let Some((&l, _)) = self.shared.forwards.lock().unwrap().iter().find(|(_, &r)| r == remote_port) {
            return Ok(l);
        }
        for local in (remote_port..=remote_port.saturating_add(PORT_TRIES)).filter(|&p| p != 0) {
            if self.shared.forwards.lock().unwrap().contains_key(&local) {
                continue;
            }
            // Free now; ssh binds it a moment later. A race with another
            // program is possible and shows as ssh's refusal below.
            if std::net::TcpListener::bind(("127.0.0.1", local)).is_err() {
                continue;
            }
            let out = control(&self.shared, "forward", &["-L".into(), l_spec(local, remote_port)]).await?;
            if !out.success() {
                bail!("could not forward port {remote_port} from host '{}': {}", self.shared.cfg.host.name.as_str(), stderr_line(&out));
            }
            self.shared.forwards.lock().unwrap().insert(local, remote_port);
            return Ok(local);
        }
        bail!("no free local port from {remote_port} to {}", remote_port.saturating_add(PORT_TRIES))
    }

    /// Stop forwarding `local_port`. Unknown ports are Ok; so is a link that
    /// is down (the forward went with the master).
    pub async fn cancel(&self, local_port: u16) -> Result<()> {
        let Some(remote) = self.shared.forwards.lock().unwrap().remove(&local_port) else {
            return Ok(());
        };
        match control(&self.shared, "cancel", &["-L".into(), l_spec(local_port, remote)]).await {
            Ok(out) if !out.success() && out.code != Some(SSH_UNREACHABLE) => {
                bail!("could not cancel forward of port {local_port}: {}", stderr_line(&out))
            }
            Err(e) if !sbxw_sbx::host::is_unreachable(&e) => Err(e),
            _ => Ok(()),
        }
    }

    /// The live forwards, `(local, remote)`, by local port.
    pub fn forwards(&self) -> Vec<(u16, u16)> {
        self.shared.forwards.lock().unwrap().iter().map(|(&l, &r)| (l, r)).collect()
    }
}

impl Drop for Link {
    fn drop(&mut self) {
        // The master is spawned kill_on_drop: aborting the task kills it.
        self.task.abort();
    }
}

fn stderr_line(out: &Output) -> String {
    let s = String::from_utf8_lossy(&out.stderr);
    s.lines().map(str::trim).rfind(|l| !l.is_empty()).unwrap_or("no reason given").to_string()
}

/// `ssh -S <ControlPath> -O <op> [extra] <ssh_argv()>`, against our master.
async fn control(shared: &Shared, op: &str, extra: &[String]) -> Result<Output> {
    let cfg = &shared.cfg;
    let mut args = vec!["-S".to_string(), cfg.control_path.to_string_lossy().into_owned(), "-O".into(), op.into()];
    args.extend_from_slice(extra);
    args.extend(cfg.host.ssh_argv());
    let cmd = HostCmd { timeout: Some(CONTROL_TIMEOUT), ..HostCmd::new(cfg.ssh.to_string_lossy(), args) };
    LocalExec.exec(cmd).await.map_err(|e| HostUnreachable { host: cfg.host.name.as_str().into(), reason: format!("{e:#}") }.into())
}

async fn master_answers(shared: &Shared) -> bool {
    matches!(control(shared, "check", &[]).await, Ok(o) if o.success())
}

fn set_down(shared: &Shared) {
    shared.state.send_if_modified(|s| {
        if *s == LinkState::Up {
            *s = LinkState::Down { since: now_secs() };
            true
        } else {
            false
        }
    });
}

async fn supervise(shared: Arc<Shared>) {
    let cfg = &shared.cfg;
    let name = cfg.host.name.as_str().to_string();
    let mut backoff = cfg.backoff_min;
    loop {
        let up_for = run_master(&shared).await;
        set_down(&shared);
        // Reset only after a link that held: one that drops right after it
        // comes up (a refused -R, a flaky network) must still back off.
        if up_for.is_some_and(|d| d >= cfg.backoff_max) {
            backoff = cfg.backoff_min;
        }
        tracing::info!(target: "host", "{name}: link down, retrying in {backoff:?}");
        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(cfg.backoff_max);
    }
}

/// One master's life. `Some(how long it was up)`, or `None` if it never came up.
async fn run_master(shared: &Shared) -> Option<Duration> {
    let cfg = &shared.cfg;
    let name = cfg.host.name.as_str();
    let cp = &cfg.control_path;
    if let Some(dir) = cp.parent() {
        use std::os::unix::fs::DirBuilderExt;
        if let Err(e) = std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir) {
            tracing::warn!(target: "host", "{name}: cannot create {}: {e}", dir.display());
            return None;
        }
    }
    // A master left by an earlier daemon would keep its own -R; replace it.
    // A socket with nobody behind it (a master killed hard) would make the
    // new `ssh -M` give up multiplexing, so it goes too.
    if master_answers(shared).await {
        let _ = control(shared, "exit", &[]).await;
    }
    let _ = std::fs::remove_file(cp);

    let mut args: Vec<String> = vec!["-M".into(), "-N".into(), "-S".into(), cp.to_string_lossy().into_owned()];
    args.extend(["-o", "ExitOnForwardFailure=yes", "-o", "ServerAliveInterval=15", "-R"].map(String::from));
    args.push(format!("127.0.0.1:{}:{}", cfg.host.agent_port, cfg.agent_addr));
    args.extend(cfg.host.ssh_argv());
    let mut child = match tokio::process::Command::new(&cfg.ssh)
        .args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(target: "host", "{name}: cannot run {}: {e}", cfg.ssh.display());
            return None;
        }
    };
    if let Some(err) = child.stderr.take() {
        let name = name.to_string();
        tokio::spawn(async move {
            let mut lines = BufReader::new(err).lines();
            while let Ok(Some(l)) = lines.next_line().await {
                tracing::info!(target: "host", "{name}: ssh: {l}");
            }
        });
    }

    // ssh -M says nothing when it is ready; ask the socket until it answers.
    let deadline = Instant::now() + CONNECT_TIMEOUT;
    loop {
        tokio::select! {
            status = child.wait() => {
                tracing::info!(target: "host", "{name}: ssh master exited before connecting ({status:?})");
                return None;
            }
            _ = tokio::time::sleep(Duration::from_millis(100)) => {}
        }
        if master_answers(shared).await {
            break;
        }
        if Instant::now() > deadline {
            tracing::warn!(target: "host", "{name}: ssh master did not come up in {CONNECT_TIMEOUT:?}");
            return None; // dropping `child` kills it
        }
    }

    // Forwards the last master had, on the same local ports, before saying Up:
    // to a caller, Up means what it set up is there again.
    let forwards: Vec<(u16, u16)> = shared.forwards.lock().unwrap().iter().map(|(&l, &r)| (l, r)).collect();
    for (local, remote) in forwards {
        match control(shared, "forward", &["-L".into(), l_spec(local, remote)]).await {
            Ok(o) if o.success() => {}
            Ok(o) => tracing::warn!(target: "host", "{name}: could not restore forward {local}→{remote}: {}", stderr_line(&o)),
            Err(e) => tracing::warn!(target: "host", "{name}: could not restore forward {local}→{remote}: {e:#}"),
        }
    }
    let up = Instant::now();
    shared.state.send_replace(LinkState::Up);
    tracing::info!(target: "host", "{name}: link up");
    let status = child.wait().await;
    tracing::info!(target: "host", "{name}: ssh master exited ({status:?})");
    Some(up.elapsed())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;

    /// Written by a child `sh`, not by this process: a test on another thread
    /// may fork while we hold the file open for writing, and that child keeps
    /// the fd until it execs, so running the script fails with ETXTBSY.
    fn write_exe(p: &Path, body: &str) {
        use std::io::Write;
        let mut c = std::process::Command::new("sh")
            .args(["-c", "cat > \"$1\" && chmod 755 \"$1\"", "sh"])
            .arg(p)
            .stdin(Stdio::piped())
            .spawn()
            .unwrap();
        c.stdin.take().unwrap().write_all(body.as_bytes()).unwrap();
        assert!(c.wait().unwrap().success());
    }

    struct World {
        _dir: tempfile::TempDir,
        home: PathBuf,
        log: PathBuf,
        down: PathBuf,
        ssh: PathBuf,
        ctl: PathBuf,
    }

    impl World {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let d = dir.path().to_path_buf();
            let home = d.join("home");
            std::fs::create_dir_all(&home).unwrap();
            let fake = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/fake-ssh").canonicalize().unwrap();
            let (log, down, ssh, ctl) = (d.join("ssh.log"), d.join("down"), d.join("ssh"), d.join("run/ssh/0badc0de"));
            // Per-test environment in a wrapper: tests run in parallel threads.
            write_exe(
                &ssh,
                &format!(
                    "#!/bin/sh\nFAKE_SSH_LOG='{}' FAKE_SSH_HOME='{}' FAKE_SSH_DOWN='{}' exec '{}' \"$@\"\n",
                    log.display(),
                    home.display(),
                    down.display(),
                    fake.display()
                ),
            );
            Self { _dir: dir, home, log, down, ssh, ctl }
        }

        fn link(&self) -> Link {
            let host = RemoteHost::new("bb", Some("bb.example".into()), None, None, None, None, Some(7700), false).unwrap();
            let mut cfg = LinkConfig::new(host, self.ctl.clone(), "127.0.0.1:7681");
            cfg.ssh = self.ssh.clone();
            cfg.backoff_min = Duration::from_millis(100);
            cfg.backoff_max = Duration::from_millis(400);
            Link::start(cfg)
        }

        fn log(&self) -> String {
            std::fs::read_to_string(&self.log).unwrap_or_default()
        }

        fn master_pid(&self) -> String {
            std::fs::read_to_string(&self.ctl).unwrap().trim().to_string()
        }
    }

    async fn wait_for(rx: &mut watch::Receiver<LinkState>, want: impl Fn(&LinkState) -> bool) -> LinkState {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        loop {
            let s = *rx.borrow_and_update();
            if want(&s) {
                return s;
            }
            tokio::time::timeout_at(deadline, rx.changed()).await.expect("link state timed out").unwrap();
        }
    }

    fn is_up(s: &LinkState) -> bool {
        *s == LinkState::Up
    }

    #[tokio::test]
    async fn goes_up_down_when_the_master_dies_and_up_again() {
        let w = World::new();
        let link = w.link();
        let mut rx = link.state();
        assert!(matches!(*rx.borrow(), LinkState::Down { .. }));
        wait_for(&mut rx, is_up).await;

        let log = w.log();
        assert!(
            log.contains("-M -N -S ")
                && log.contains("-o ExitOnForwardFailure=yes -o ServerAliveInterval=15 -R 127.0.0.1:7700:127.0.0.1:7681"),
            "{log}"
        );
        assert!(log.contains("-O check"), "{log}");
        // The master's mode is private: the socket gives a shell on the host.
        let mode = std::fs::metadata(w.ctl.parent().unwrap()).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700);

        let first = w.master_pid();
        let before = now_secs();
        std::process::Command::new("kill").arg(&first).status().unwrap();
        let down = wait_for(&mut rx, |s| !is_up(s)).await;
        assert!(matches!(down, LinkState::Down { since } if since >= before), "{down:?}");
        wait_for(&mut rx, is_up).await;
        assert_ne!(w.master_pid(), first);
        assert_eq!(w.log().matches("-M -N").count(), 2);
    }

    #[tokio::test]
    async fn forward_takes_the_next_port_when_the_first_is_taken_and_survives_a_restart() {
        let w = World::new();
        let link = w.link();
        let mut rx = link.state();
        wait_for(&mut rx, is_up).await;

        let taken = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let p = taken.local_addr().unwrap().port();
        let l = link.forward(p).await.unwrap();
        assert!(l > p, "{l} after {p}");
        assert!(w.log().contains(&format!("-O forward -L 127.0.0.1:{l}:127.0.0.1:{p}")), "{}", w.log());
        assert_eq!(link.forward(p).await.unwrap(), l, "asking again reuses the forward");
        assert_eq!(link.forwards(), [(l, p)]);

        // A new master gets the forward back before it reports Up.
        std::process::Command::new("kill").arg(w.master_pid()).status().unwrap();
        wait_for(&mut rx, |s| !is_up(s)).await;
        wait_for(&mut rx, is_up).await;
        assert_eq!(w.log().matches(&format!("-O forward -L 127.0.0.1:{l}:127.0.0.1:{p}")).count(), 2, "{}", w.log());

        link.cancel(l).await.unwrap();
        assert!(w.log().contains(&format!("-O cancel -L 127.0.0.1:{l}:127.0.0.1:{p}")));
        assert!(link.forwards().is_empty());
        link.cancel(l).await.unwrap();
    }

    #[tokio::test]
    async fn a_down_host_stays_down_and_keeps_retrying() {
        let w = World::new();
        std::fs::write(&w.down, "").unwrap();
        let link = w.link();
        let mut rx = link.state();
        tokio::time::sleep(Duration::from_millis(700)).await;
        assert!(matches!(*rx.borrow(), LinkState::Down { .. }));
        assert!(w.log().matches("-M -N").count() >= 2, "{}", w.log());
        // A forward with no master is the host being unreachable.
        let free = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        assert!(link.forward(free).await.is_err());

        std::fs::remove_file(&w.down).unwrap();
        wait_for(&mut rx, is_up).await;
        // Commands go through the same ControlPath.
        let out = link.exec().exec(HostCmd::new("pwd", vec![])).await.unwrap();
        assert_eq!(out.stdout_str().trim(), w.home.canonicalize().unwrap().display().to_string());
        assert!(w.log().contains(&format!("-S {}", w.ctl.display())));
    }

    #[tokio::test]
    async fn dropping_the_link_stops_the_master() {
        let w = World::new();
        let link = w.link();
        let mut rx = link.state();
        wait_for(&mut rx, is_up).await;
        let pid = w.master_pid();
        drop(link);
        let deadline = Instant::now() + Duration::from_secs(5);
        while std::process::Command::new("kill").args(["-0", &pid]).stderr(Stdio::null()).status().unwrap().success() {
            assert!(Instant::now() < deadline, "master {pid} still running");
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
}
