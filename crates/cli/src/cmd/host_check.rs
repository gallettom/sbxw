//! `sbxw host check <name>`: exercises the remote-host transport against a
//! real machine without creating a sandbox. There is no KVM host to test
//! on, but everything below the sandbox (ssh quoting, sbx detection, the
//! ControlMaster link, `-L`/`-R`, the host's disk, kit copies, git) needs only
//! ssh and a shell, so a plain Ubuntu box with `sbx` installed can check it.
//!
//! One line per step, `✓`, `-` (skipped, with the reason) or `✗`. The check
//! never touches a sandbox, the host's `~/.sbxw` or the daemon's own link: it
//! works in a `mktemp -d` directory on the host and uses its own ControlPath,
//! so a running daemon keeps its connection.

use anyhow::{Context, Result};
use sbxw_core::remote_hosts::RemoteHost;
use sbxw_daemon::git;
use sbxw_daemon::host::exec_fs::ExecFs;
use sbxw_daemon::host::kits;
use sbxw_daemon::host::link::{Link, LinkConfig, LinkState};
use sbxw_daemon::host::{Host, HostFs, Kind};
use sbxw_sbx::host::{HostCmd, HostExec, LocalExec};
use sbxw_sbx::ssh::{control_path, SshExec};
use sbxw_sbx::{parse_ls_json, parse_version, MIN_VERSION};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

/// What a throwaway listener says to whoever connects, so a tunnel is proven
/// by bytes that came out of the far end and not only by an open port.
const BANNER: &str = "sbxw-check-ok";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Pass(String),
    /// Not run, and why. Counts as fine: a host without python3 is not broken.
    Skip(String),
    Fail(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    pub title: String,
    pub outcome: Outcome,
}

impl Step {
    /// `  ✓ title: detail` / `  - title: skipped, reason` / `  ✗ title: reason`.
    pub fn line(&self) -> String {
        match &self.outcome {
            Outcome::Pass(d) if d.is_empty() => format!("  ✓ {}", self.title),
            Outcome::Pass(d) => format!("  ✓ {}: {d}", self.title),
            Outcome::Skip(why) => format!("  - {}: skipped, {why}", self.title),
            Outcome::Fail(why) => format!("  ✗ {}: {why}", self.title),
        }
    }
}

#[derive(Debug, Default)]
pub struct Report {
    pub steps: Vec<Step>,
}

impl Report {
    pub fn passed(&self) -> usize {
        self.steps.iter().filter(|s| matches!(s.outcome, Outcome::Pass(_))).count()
    }
    pub fn skipped(&self) -> usize {
        self.steps.iter().filter(|s| matches!(s.outcome, Outcome::Skip(_))).count()
    }
    pub fn failed(&self) -> usize {
        self.steps.iter().filter(|s| matches!(s.outcome, Outcome::Fail(_))).count()
    }
    /// Exit 0 only when every step passed or was skipped with a reason.
    pub fn ok(&self) -> bool {
        self.failed() == 0
    }
    pub fn verdict(&self) -> String {
        if self.ok() {
            format!("PASS: {} passed, {} skipped, 0 failed", self.passed(), self.skipped())
        } else {
            format!("FAIL: {} passed, {} skipped, {} failed", self.passed(), self.skipped(), self.failed())
        }
    }
}

pub struct Opts {
    /// Leave the temp directory on the host (its path is printed).
    pub keep: bool,
    /// The ssh program; tests put `scripts/fake-ssh` here.
    pub ssh: PathBuf,
    /// This machine's home: the ControlPath lives under its `.sbxw/run/ssh`.
    pub home: PathBuf,
    /// How long the link gets to come up.
    pub link_wait: Duration,
}

/// Collects steps and hands each to `emit` as it is recorded, so a slow step
/// shows what came before it.
struct Rec<'a> {
    report: Report,
    emit: &'a mut dyn FnMut(&Step),
}

impl Rec<'_> {
    fn add(&mut self, title: &str, outcome: Outcome) -> bool {
        let ok = !matches!(outcome, Outcome::Fail(_));
        let step = Step { title: title.to_string(), outcome };
        (self.emit)(&step);
        self.report.steps.push(step);
        ok
    }
    fn pass(&mut self, title: &str, detail: impl Into<String>) -> bool {
        self.add(title, Outcome::Pass(detail.into()))
    }
    fn skip(&mut self, title: &str, why: impl Into<String>) {
        self.add(title, Outcome::Skip(why.into()));
    }
    fn fail(&mut self, title: &str, why: impl Into<String>) -> bool {
        self.add(title, Outcome::Fail(why.into()))
    }
}

fn last_line(b: &[u8]) -> String {
    let s = String::from_utf8_lossy(b);
    s.lines().map(str::trim).rfind(|l| !l.is_empty()).unwrap_or("no reason given").to_string()
}

/// Why a command did not work: ssh's last line, or its exit code.
fn why(out: &sbxw_sbx::runner::Output) -> String {
    match out.code {
        None => "timed out".into(),
        Some(c) if out.stderr.iter().all(u8::is_ascii_whitespace) => format!("exit {c}"),
        Some(c) => format!("exit {c}: {}", last_line(&out.stderr)),
    }
}

fn err(e: &anyhow::Error) -> String {
    format!("{e:#}")
}

/// Words that break a shell that is handed them unquoted.
const HOSTILE: &[&str] = &["$(id)", "`id`", "it's", "'", "a  b", "line\nbreak", "$HOME", ";touch x", "-n", "", "\"q\"", "\\", "é"];

/// Written on the host to serve one connection: it says its port, waits up to
/// a minute for one client, sends the banner and exits. Nothing to clean up.
const LISTENER_PY: &str = r#"import socket
s = socket.socket()
s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
s.bind(("127.0.0.1", 0))
s.listen(1)
print(s.getsockname()[1], flush=True)
s.settimeout(60)
try:
    c, _ = s.accept()
    c.sendall(b"sbxw-check-ok\n")
    c.close()
except Exception:
    pass
"#;

/// Start the listener detached (its output goes to files so the ssh channel
/// closes) and print its port.
const LISTENER_SH: &str = r#"d=$1
command -v python3 >/dev/null 2>&1 || { echo "python3 not found" >&2; exit 127; }
(python3 -c "$2" >"$d/port" 2>"$d/err" </dev/null &)
i=0
while [ ! -s "$d/port" ] && [ "$i" -lt 50 ]; do sleep 0.1; i=$((i + 1)); done
[ -s "$d/port" ] || { cat "$d/err" >&2; exit 1; }
cat "$d/port""#;

/// Connect to 127.0.0.1:$1 from the host with bash's /dev/tcp and print the
/// first line the other side sends.
const CLIENT_SH: &str = r#"command -v bash >/dev/null 2>&1 || { echo "bash not found" >&2; exit 127; }
exec bash -c 'exec 3<>/dev/tcp/127.0.0.1/$1 && read -r -t 10 l <&3 && printf %s "$l"' bash "$1""#;

/// Connect to a local port and return the first line it sends.
async fn banner_from(port: u16) -> Result<String> {
    let work = async {
        let s = tokio::net::TcpStream::connect(("127.0.0.1", port)).await?;
        let mut line = String::new();
        BufReader::new(s).read_line(&mut line).await?;
        anyhow::Ok(line.trim().to_string())
    };
    tokio::time::timeout(Duration::from_secs(10), work).await.context("no answer in 10s")?
}

/// A local listener that says [`BANNER`] to each client: the far end of `-R`.
async fn banner_listener() -> Result<(std::net::SocketAddr, tokio::task::JoinHandle<()>)> {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let addr = l.local_addr()?;
    let task = tokio::spawn(async move {
        while let Ok((mut s, _)) = l.accept().await {
            let _ = s.write_all(format!("{BANNER}\n").as_bytes()).await;
        }
    });
    Ok((addr, task))
}

fn tmp_local() -> Result<PathBuf> {
    let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let d = std::env::temp_dir().join(format!("sbxw-host-check-{}-{n}", std::process::id()));
    {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new().recursive(true).mode(0o700).create(&d)?;
    }
    Ok(d)
}

/// Run every step against `host`. Never creates a sandbox.
pub async fn run_checks(host: &RemoteHost, opts: &Opts, emit: &mut dyn FnMut(&Step)) -> Report {
    let mut r = Rec { report: Report::default(), emit };
    // Its own ControlPath: the daemon's link to this host keeps its master.
    let real = control_path(&opts.home, &host.name);
    let cp = real.with_file_name(format!("c{}", real.file_name().and_then(|n| n.to_str()).unwrap_or("check")));
    if let Some(dir) = cp.parent() {
        use std::os::unix::fs::DirBuilderExt;
        let _ = std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir);
    }
    steps(host, opts, &cp, &mut r).await;
    let _ = std::fs::remove_file(&cp);
    r.report
}

async fn steps(host: &RemoteHost, opts: &Opts, cp: &Path, r: &mut Rec<'_>) {
    let exec = Arc::new(SshExec::with_ssh(host.clone(), cp.to_path_buf(), &opts.ssh));

    // 1. ssh itself: no master yet, so this is a plain connection.
    const T1: &str = "ssh: connects, `--` before the command";
    match exec.exec(HostCmd::new("echo", vec!["ok".into()])).await {
        Ok(o) if o.success() && o.stdout_str().trim() == "ok" => {
            r.pass(T1, "echo ok came back");
        }
        Ok(o) => {
            r.fail(T1, why(&o));
            return;
        }
        Err(e) => {
            r.fail(T1, err(&e));
            return;
        }
    }
    const T2: &str = "ssh: hostile words come back unchanged";
    let args: Vec<String> = std::iter::once("%s\\0".to_string()).chain(HOSTILE.iter().map(|w| w.to_string())).collect();
    match exec.exec(HostCmd::new("printf", args)).await {
        Ok(o) if o.success() => {
            let text = String::from_utf8_lossy(&o.stdout).into_owned();
            let mut got: Vec<&str> = text.split('\0').collect();
            got.pop(); // after the last terminator
            if got == HOSTILE {
                r.pass(T2, format!("{} words, byte for byte", HOSTILE.len()));
            } else {
                r.fail(T2, format!("sent {HOSTILE:?}, got {got:?}"));
            }
        }
        Ok(o) => {
            r.fail(T2, why(&o));
        }
        Err(e) => {
            r.fail(T2, err(&e));
        }
    }
    match exec.home().await {
        Ok(h) => {
            r.pass("ssh: the host's home", h.display().to_string());
        }
        Err(e) => {
            r.fail("ssh: the host's home", err(&e));
        }
    }

    // 2. sbx.
    let sbx = match exec.sbx_program().await {
        Ok(p) => {
            r.pass("sbx: found", p.clone());
            Some(p)
        }
        Err(e) => {
            r.fail("sbx: found", err(&e));
            None
        }
    };
    if let Some(sbx) = &sbx {
        const TV: &str = "sbx: version";
        let (a, b, c) = MIN_VERSION;
        match exec.exec(HostCmd { timeout: Some(Duration::from_secs(60)), ..HostCmd::new(sbx, vec!["version".into()]) }).await {
            Ok(o) if o.success() => {
                let text = if o.stdout.is_empty() { String::from_utf8_lossy(&o.stderr).into_owned() } else { o.stdout_str() };
                match parse_version(&text) {
                    Some(v) if v >= MIN_VERSION => {
                        r.pass(TV, format!("{}.{}.{} (needs {a}.{b}.{c} or newer)", v.0, v.1, v.2));
                    }
                    Some(v) => {
                        r.fail(TV, format!("{}.{}.{} is older than {a}.{b}.{c}: upgrade sbx on the host", v.0, v.1, v.2));
                    }
                    None => {
                        r.fail(TV, format!("no version in {:?}", text.trim()));
                    }
                }
            }
            Ok(o) => {
                r.fail(TV, why(&o));
            }
            Err(e) => {
                r.fail(TV, err(&e));
            }
        }
        // Stdin is closed, so sbx's first-run network policy question fails
        // here instead of hanging; the message says what to do.
        const TL: &str = "sbx: `ls --format json` parses";
        match exec
            .exec(HostCmd { timeout: Some(Duration::from_secs(60)), ..HostCmd::new(sbx, vec!["ls".into(), "--format".into(), "json".into()]) })
            .await
        {
            Ok(o) if o.success() => match parse_ls_json(&o.stdout_str()) {
                Some(v) => {
                    r.pass(TL, format!("{} sandbox(es)", v.len()));
                }
                None => {
                    r.fail(TL, format!("not JSON: {:?}", o.stdout_str().chars().take(80).collect::<String>()));
                }
            },
            Ok(o) => {
                r.fail(
                    TL,
                    format!("{} (if sbx asks for a network policy, run `sbx ls` once on the host and answer it)", why(&o)),
                );
            }
            Err(e) => {
                r.fail(TL, err(&e));
            }
        }
    } else {
        r.skip("sbx: version", "sbx was not found");
        r.skip("sbx: `ls --format json` parses", "sbx was not found");
    }

    // 3 and 4. The link. The far end of -R is a throwaway local listener, not
    // the daemon's agent.
    let (agent_addr, listener) = match banner_listener().await {
        Ok(x) => x,
        Err(e) => {
            r.fail("link: local listener for -R", err(&e));
            return;
        }
    };
    let mut cfg = LinkConfig::new(host.clone(), cp.to_path_buf(), agent_addr.to_string());
    cfg.ssh = opts.ssh.clone();
    let link = Link::start(cfg);
    let mut state = link.state();
    const TU: &str = "link: ControlMaster comes up";
    let up = tokio::time::timeout(opts.link_wait, state.wait_for(|s| *s == LinkState::Up)).await.is_ok_and(|x| x.is_ok());
    if !up {
        r.fail(
            TU,
            format!(
                "not up after {}s. Is a daemon already linked to this host? Its -R holds port {} on the host (`sbxw down` first). \
                 Or the host key changed.",
                opts.link_wait.as_secs(),
                host.agent_port
            ),
        );
    } else {
        r.pass(TU, "");
        link_steps(host, opts, cp, &link, r).await;
        fs_steps(host, opts, &link, r).await;
    }
    drop(link);
    listener.abort();
}

async fn link_steps(host: &RemoteHost, opts: &Opts, cp: &Path, link: &Link, r: &mut Rec<'_>) {
    // `-O check`, the way the link itself asks.
    const TC: &str = "link: `ssh -O check` answers";
    let mut args = vec!["-S".to_string(), cp.to_string_lossy().into_owned(), "-O".into(), "check".into()];
    args.extend(host.ssh_argv());
    match LocalExec.exec(HostCmd { timeout: Some(Duration::from_secs(10)), ..HostCmd::new(opts.ssh.to_string_lossy(), args) }).await {
        Ok(o) if o.success() => {
            r.pass(TC, last_line(&o.stderr));
        }
        Ok(o) => {
            r.fail(TC, why(&o));
        }
        Err(e) => {
            r.fail(TC, err(&e));
        }
    }

    // -L: a listener on the host, reached through a forwarded local port.
    const TF: &str = "link: -L forward reaches a service on the host";
    let exec = link.exec();
    let dir = match mktemp(&*exec).await {
        Ok(d) => d,
        Err(e) => {
            r.skip(TF, format!("no temp dir on the host ({e})"));
            return;
        }
    };
    let port = exec
        .exec(HostCmd {
            timeout: Some(Duration::from_secs(20)),
            ..HostCmd::new("sh", vec!["-c".into(), LISTENER_SH.into(), "sh".into(), dir.clone(), LISTENER_PY.into()])
        })
        .await;
    match port {
        Ok(o) if o.code == Some(127) => r.skip(TF, last_line(&o.stderr)),
        Ok(o) if !o.success() => {
            r.fail(TF, format!("could not start a listener on the host: {}", why(&o)));
        }
        Err(e) => {
            r.fail(TF, err(&e));
        }
        Ok(o) => match o.stdout_str().trim().parse::<u16>() {
            Err(_) => {
                r.fail(TF, format!("the host's listener said {:?}", o.stdout_str().trim()));
            }
            Ok(remote) => match link.forward(remote).await {
                Err(e) => {
                    r.fail(TF, err(&e));
                }
                Ok(local) => {
                    match banner_from(local).await {
                        Ok(b) if b == BANNER => {
                            r.pass(TF, format!("127.0.0.1:{local} -> host port {remote}"));
                        }
                        Ok(b) => {
                            r.fail(TF, format!("127.0.0.1:{local} answered {b:?}"));
                        }
                        Err(e) => {
                            r.fail(TF, format!("127.0.0.1:{local} -> host port {remote}: {}", err(&e)));
                        }
                    }
                    const TX: &str = "link: -L forward is cancelled";
                    match link.cancel(local).await {
                        Err(e) => {
                            r.fail(TX, err(&e));
                        }
                        Ok(()) if !link.forwards().is_empty() => {
                            r.fail(TX, "the link still lists it");
                        }
                        Ok(()) => {
                            // Give ssh a moment to close its listener.
                            tokio::time::sleep(Duration::from_millis(300)).await;
                            match tokio::net::TcpStream::connect(("127.0.0.1", local)).await {
                                Err(_) => {
                                    r.pass(TX, format!("127.0.0.1:{local} is closed"));
                                }
                                Ok(_) => {
                                    r.fail(TX, format!("127.0.0.1:{local} still accepts connections"));
                                }
                            }
                        }
                    }
                }
            },
        },
    }
    let _ = exec.exec(HostCmd::new("rm", vec!["-rf".into(), dir])).await;

    // -R: from the host to this machine. Only this direction is covered; a
    // sandbox on the host reaching the host's loopback needs a real sandbox.
    const TR: &str = "link: -R tunnel host -> this machine (sandbox -> host is NOT covered)";
    match exec.exec(HostCmd { timeout: Some(Duration::from_secs(20)), ..HostCmd::new("sh", vec!["-c".into(), CLIENT_SH.into(), "sh".into(), host.agent_port.to_string()]) }).await {
        Ok(o) if o.code == Some(127) => r.skip(TR, last_line(&o.stderr)),
        Ok(o) if o.success() && o.stdout_str().trim() == BANNER => {
            r.pass(TR, format!("host 127.0.0.1:{} reached a throwaway listener here", host.agent_port));
        }
        Ok(o) if o.success() => {
            r.fail(TR, format!("host 127.0.0.1:{} answered {:?}", host.agent_port, o.stdout_str().trim()));
        }
        Ok(o) => {
            r.fail(TR, format!("host 127.0.0.1:{} did not answer: {}", host.agent_port, why(&o)));
        }
        Err(e) => {
            r.fail(TR, err(&e));
        }
    }
}

async fn mktemp(exec: &dyn HostExec) -> Result<String> {
    let o = exec
        .exec(HostCmd::new("sh", vec!["-c".into(), "mktemp -d \"${TMPDIR:-/tmp}/sbxw-check.XXXXXX\"".into()]))
        .await?;
    let d = o.stdout_str().trim().to_string();
    if !o.success() || !d.starts_with('/') {
        anyhow::bail!("mktemp -d: {}", why(&o));
    }
    Ok(d)
}

/// ExecFs, kit mirror and git, in a temp dir on the host.
async fn fs_steps(host: &RemoteHost, opts: &Opts, link: &Link, r: &mut Rec<'_>) {
    let exec = link.exec();
    let dir = match mktemp(&*exec).await {
        Ok(d) => d,
        Err(e) => {
            r.fail("host: temp dir (mktemp -d)", err(&e));
            for t in ["fs: write, read, walk, remove", "kits: mirror copies once", "git: init and scan"] {
                r.skip(t, "no temp dir on the host");
            }
            return;
        }
    };
    r.pass("host: temp dir (mktemp -d)", dir.clone());
    let root = PathBuf::from(&dir);
    let fs = ExecFs::new(exec.clone());

    const TF: &str = "fs: write, read, walk, remove";
    let work = root.join("fs");
    let data = b"sbxw check \x00\xff\n'quote' $(id)\n".to_vec();
    let res: Result<String> = async {
        fs.create_dir_all(&work.join("sub")).await?;
        fs.write_atomic(&work.join("a.txt"), &data, 0o640).await?;
        let back = fs.read(&work.join("a.txt"), 1 << 20).await?;
        anyhow::ensure!(back == data, "read back {} bytes, wrote {}", back.len(), data.len());
        let meta = fs.lstat(&work.join("a.txt")).await?.context("a.txt vanished")?;
        anyhow::ensure!(meta.kind == Kind::File && meta.len == data.len() as u64, "stat says {meta:?}");
        let walk = fs.walk(&work, 2, &[]).await?;
        let names: Vec<&str> = walk.iter().map(|(p, _)| p.as_str()).collect();
        anyhow::ensure!(names == ["a.txt", "sub"], "walk found {names:?}");
        fs.remove(&work).await?;
        anyhow::ensure!(fs.lstat(&work).await?.is_none(), "still there after remove");
        Ok(format!("{} bytes, binary safe", data.len()))
    }
    .await;
    match res {
        Ok(d) => {
            r.pass(TF, d);
        }
        Err(e) => {
            r.fail(TF, err(&e));
        }
    }

    // A throwaway kit under a throwaway local root, mirrored into the temp dir.
    const TK: &str = "kits: mirror copies once, a second run copies nothing";
    match tmp_local() {
        Err(e) => {
            r.fail(TK, err(&e));
        }
        Ok(local) => {
            let res: Result<String> = async {
                let kit = local.join("kits").join("check-kit");
                std::fs::create_dir_all(kit.join("bin"))?;
                std::fs::write(kit.join("spec.yaml"), "name: sbxw-host-check\n")?;
                std::fs::write(kit.join("bin").join("run.sh"), "#!/bin/sh\necho ok\n")?;
                let k = vec![kit.to_string_lossy().into_owned()];
                let first = kits::mirror(&fs, &local, &root, &k, false).await?;
                anyhow::ensure!(first.copied.len() == 1, "first run copied {:?}", first.copied);
                let dest = PathBuf::from(&first.refs[0]);
                let spec = fs.read(&dest.join("spec.yaml"), 4096).await?;
                anyhow::ensure!(spec == b"name: sbxw-host-check\n", "spec.yaml differs on the host");
                let again = kits::mirror(&fs, &local, &root, &k, false).await?;
                anyhow::ensure!(again.copied.is_empty(), "second run copied {:?}", again.copied);
                anyhow::ensure!(again.refs == first.refs, "second run chose another directory");
                Ok(format!("sent once to {}", dest.display()))
            }
            .await;
            let _ = std::fs::remove_dir_all(&local);
            match res {
                Ok(d) => {
                    r.pass(TK, d);
                }
                Err(e) => {
                    r.fail(TK, err(&e));
                }
            }
        }
    }

    // git through the Host, the way the daemon reads a workspace.
    const TG: &str = "git: init on the host, scan finds the repo";
    let h = Host { name: host.name.clone(), exec: exec.clone(), fs: Arc::new(fs.clone()) };
    let ws = root.join("ws");
    let repo = ws.join("proj");
    let res: Result<String> = async {
        h.fs.create_dir_all(&repo).await?;
        let init = git::git(&h, &repo, &["init", "-q"]).await;
        anyhow::ensure!(init.is_some(), "`git init` failed on the host (is git installed?)");
        let found = git::scan(&h, &ws).await;
        anyhow::ensure!(found == ["proj"], "scan found {found:?}");
        Ok("found proj".into())
    }
    .await;
    match res {
        Ok(d) => {
            r.pass(TG, d);
        }
        Err(e) => {
            r.fail(TG, err(&e));
        }
    }

    if opts.keep {
        r.pass("host: temp dir kept", format!("{dir} (remove it yourself)"));
    } else {
        match fs.remove(&root).await {
            Ok(()) => {
                r.pass("host: temp dir removed", "");
            }
            Err(e) => {
                r.fail("host: temp dir removed", format!("{} left behind: {}", dir, err(&e)));
            }
        }
    }
}

/// What this check cannot see, printed under every verdict.
pub const NOT_COVERED: &str = "Not covered (needs a host with KVM): creating a sandbox, `ssh -tt <host> sbx run`, \
and the sandbox -> host -> this machine leg of the -R tunnel.";

/// The command: run the steps, print them as they come, then the verdict.
pub fn run(name: &str, hosts: &sbxw_core::remote_hosts::RemoteHosts, keep: bool) -> Result<()> {
    let host = hosts.get(name).with_context(|| format!("no host '{name}'"))?;
    let home = std::env::var_os("HOME").map(PathBuf::from).context("HOME is not set")?;
    let opts = Opts { keep, ssh: "ssh".into(), home, link_wait: Duration::from_secs(25) };
    println!("Checking host '{name}' (no sandbox is created; EXPERIMENTAL)");
    let rt = super::runtime();
    let report = rt.block_on(run_checks(host, &opts, &mut |s| println!("{}", s.line())));
    println!("\n{}\n{NOT_COVERED}", report.verdict());
    if report.ok() {
        Ok(())
    } else {
        anyhow::bail!("host check failed")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn step(o: Outcome) -> Step {
        Step { title: "t".into(), outcome: o }
    }

    #[test]
    fn lines_say_what_happened() {
        assert_eq!(step(Outcome::Pass("".into())).line(), "  ✓ t");
        assert_eq!(step(Outcome::Pass("d".into())).line(), "  ✓ t: d");
        assert_eq!(step(Outcome::Skip("no python3".into())).line(), "  - t: skipped, no python3");
        assert_eq!(step(Outcome::Fail("boom".into())).line(), "  ✗ t: boom");
    }

    #[test]
    fn skips_pass_and_failures_fail() {
        let mut r = Report { steps: vec![step(Outcome::Pass("".into())), step(Outcome::Skip("x".into()))] };
        assert!(r.ok());
        assert_eq!(r.verdict(), "PASS: 1 passed, 1 skipped, 0 failed");
        r.steps.push(step(Outcome::Fail("y".into())));
        assert!(!r.ok());
        assert_eq!(r.verdict(), "FAIL: 1 passed, 1 skipped, 1 failed");
    }

    #[test]
    fn steps_are_emitted_as_they_are_recorded() {
        let mut seen = Vec::new();
        let mut sink = |s: &Step| seen.push(s.title.clone());
        let mut r = Rec { report: Report::default(), emit: &mut sink };
        r.pass("a", "");
        r.skip("b", "why");
        assert!(!r.fail("c", "bad"));
        assert_eq!(r.report.steps.len(), 3);
        drop(r);
        assert_eq!(seen, ["a", "b", "c"]);
    }

    fn write_exe(p: &Path, body: &str) {
        use std::io::Write;
        let mut c = std::process::Command::new("sh")
            .args(["-c", "cat > \"$1\" && chmod 755 \"$1\"", "sh"])
            .arg(p)
            .stdin(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        c.stdin.take().unwrap().write_all(body.as_bytes()).unwrap();
        assert!(c.wait().unwrap().success());
    }

    /// Stands in for the two tunnels fake-ssh only pretends to open: a python
    /// relay per `-L`/`-R`, which ends when the master (the pid in the
    /// ControlPath file) is gone.
    const RELAY_PY: &str = r#"import os, socket, sys, threading, time
listen, host, port, ctl = int(sys.argv[1]), sys.argv[2], int(sys.argv[3]), sys.argv[4]
s = socket.socket()
s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
s.bind(("127.0.0.1", listen))
s.listen(8)
s.settimeout(0.2)
open(ctl + ".ready." + str(listen), "w").close()
seen = False
def pump(a, b):
    try:
        while True:
            d = a.recv(4096)
            if not d: break
            b.sendall(d)
    except Exception: pass
    try: b.shutdown(socket.SHUT_WR)
    except Exception: pass
end = time.time() + 120
while time.time() < end:
    try:
        pid = int(open(ctl).read())
        os.kill(pid, 0)
        seen = True
    except Exception:
        if seen: break
    try:
        c, _ = s.accept()
    except socket.timeout:
        continue
    u = socket.create_connection((host, port))
    threading.Thread(target=pump, args=(c, u), daemon=True).start()
    threading.Thread(target=pump, args=(u, c), daemon=True).start()
"#;

    const WRAPPER: &str = r#"#!/bin/bash
# fake-ssh plus real relays for -L (via -O forward/cancel) and -R (via -M).
args=("$@"); ctl=""; op=""; lspec=""; rspec=""
for ((i = 0; i < ${#args[@]}; i++)); do
  case "${args[$i]}" in
    -S) ctl="${args[$((i + 1))]}" ;;
    -O) op="${args[$((i + 1))]}" ;;
    -L) lspec="${args[$((i + 1))]}" ;;
    -R) rspec="${args[$((i + 1))]}" ;;
  esac
done
relay() { # listen-port target-host target-port
  nohup python3 '@RELAY@' "$1" "$2" "$3" "$ctl" >/dev/null 2>&1 </dev/null &
  echo $! > "$ctl.relay.$1"
  # ssh has the port open by the time it returns; wait for the same.
  for _ in $(seq 100); do [ -e "$ctl.ready.$1" ] && break; sleep 0.05; done
}
case "$op" in
  forward) IFS=: read -r _ l _ r <<< "$lspec"; relay "$l" 127.0.0.1 "$r" ;;
  cancel) IFS=: read -r _ l _ r <<< "$lspec"; kill "$(cat "$ctl.relay.$l")" 2>/dev/null; sleep 0.3 ;;
esac
if [ -z "$op" ] && [ -n "$rspec" ]; then
  IFS=: read -r _ a h p <<< "$rspec"; relay "$a" "$h" "$p"
fi
exec '@FAKE@' "$@"
"#;

    struct World {
        dir: PathBuf,
        ssh: PathBuf,
    }

    impl World {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("sbxw-hc-{}-{tag}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            let home = dir.join("home");
            std::fs::create_dir_all(home.join("bin")).unwrap();
            let fake = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/fake-ssh").canonicalize().unwrap();
            let relay = dir.join("relay.py");
            std::fs::write(&relay, RELAY_PY).unwrap();
            let ssh = dir.join("ssh");
            let body = WRAPPER.replace("@FAKE@", &fake.display().to_string()).replace("@RELAY@", &relay.display().to_string());
            let env = format!(
                "#!/bin/sh\nFAKE_SSH_LOG='{}' FAKE_SSH_HOME='{}' exec '{}' \"$@\"\n",
                dir.join("ssh.log").display(),
                home.display(),
                dir.join("ssh-relay").display()
            );
            write_exe(&dir.join("ssh-relay"), &body);
            write_exe(&ssh, &env);
            let mut p = std::fs::metadata(&ssh).unwrap().permissions();
            p.set_mode(0o755);
            Self { dir, ssh }
        }
    }

    impl Drop for World {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn free_port() -> u16 {
        std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
    }

    fn have(prog: &str) -> bool {
        std::process::Command::new("sh").args(["-c", &format!("command -v {prog}")]).output().is_ok_and(|o| o.status.success())
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_whole_check_passes_against_fake_ssh() {
        // The -L and -R steps need python3 and bash on the "host" (this machine).
        if !have("python3") || !have("bash") {
            eprintln!("skipped: python3 or bash missing");
            return;
        }
        let w = World::new("pass");
        // `sbx` on the fake host: answers the two commands the check asks.
        let sbx = w.dir.join("sbx");
        write_exe(
            &sbx,
            "#!/bin/sh\ncase \"$1\" in version) echo 'sbx version v0.51.0 abc123';; ls) echo '[]';; *) exit 1;; esac\n",
        );
        let host = RemoteHost::new("fakebox", Some("fake.example".into()), None, None, None, None, Some(free_port()), false)
            .unwrap()
            .with_sbx_path(Some(sbx.to_string_lossy().into_owned()))
            .unwrap();
        let opts = Opts { keep: false, ssh: w.ssh.clone(), home: w.dir.clone(), link_wait: Duration::from_secs(20) };
        let mut lines = Vec::new();
        let report = run_checks(&host, &opts, &mut |s| lines.push(s.line())).await;
        let text = lines.join("\n");
        assert!(report.ok(), "{text}");
        assert_eq!(report.skipped(), 0, "{text}");
        for want in ["hostile words", "ControlMaster", "-O check", "-L forward reaches", "cancelled", "-R tunnel", "write, read, walk", "mirror copies once", "scan finds", "removed"] {
            assert!(text.contains(want), "no step for {want:?}:\n{text}");
        }
        assert!(report.verdict().starts_with("PASS"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_host_that_is_down_fails_at_the_first_step_and_stops() {
        let w = World::new("down");
        std::fs::write(w.dir.join("down"), "").unwrap();
        // The wrapper does not forward FAKE_SSH_DOWN; use a second one that does.
        let ssh = w.dir.join("ssh-down");
        write_exe(&ssh, &format!("#!/bin/sh\nFAKE_SSH_DOWN='{}' exec '{}' \"$@\"\n", w.dir.join("down").display(), w.ssh.display()));
        let host = RemoteHost::new("fakebox", Some("fake.example".into()), None, None, None, None, None, false).unwrap();
        let opts = Opts { keep: false, ssh, home: w.dir.clone(), link_wait: Duration::from_secs(2) };
        let report = run_checks(&host, &opts, &mut |_| {}).await;
        assert!(!report.ok());
        assert_eq!(report.steps.len(), 1);
        assert!(matches!(&report.steps[0].outcome, Outcome::Fail(m) if m.contains("unreachable")), "{:?}", report.steps);
    }
}
