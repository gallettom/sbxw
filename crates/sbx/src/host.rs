//! Running a command on a host: this machine, or (see [`crate::ssh`]) one
//! reached over SSH. The contract every host-side caller is written against.
//!
//! * [`HostExec`] runs one command and says what came back. A non-zero exit
//!   is an ordinary `Ok(Output)`; only "the host could not be reached" is an
//!   error, and that error is always a [`HostUnreachable`], so callers can
//!   tell a dead link from a failed command with `err.downcast_ref()`.
//! * [`LocalExec`] is this machine. [`OnHost`] turns any `HostExec` plus a
//!   program into a [`Runner`], so the typed `sbx` client runs on a host
//!   unchanged. [`FakeExec`] is the scripted host for tests in every crate.
//!
//! Secrets travel on stdin, never in argv: argv is visible in `ps` on the
//! host and in ssh's command line.

use crate::runner::{drain, Invocation, LineSink, Output, Runner};
use anyhow::{Context, Result};
use async_trait::async_trait;
use std::fmt;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::AsyncWriteExt;

/// One command to run on a host.
#[derive(Clone, Default)]
pub struct HostCmd {
    pub program: String,
    pub args: Vec<String>,
    /// Working directory on the host; `None` = wherever the host starts.
    pub cwd: Option<String>,
    /// Piped to stdin, then closed. Secrets go here.
    pub stdin: Option<Vec<u8>>,
    /// Each line of output as it arrives (stdout and stderr).
    pub on_line: Option<LineSink>,
    /// Kill the command after this long; the result is then `code: None`.
    pub timeout: Option<Duration>,
}

impl HostCmd {
    pub fn new(program: impl Into<String>, args: Vec<String>) -> Self {
        Self { program: program.into(), args, ..Default::default() }
    }

    /// `program args…`, for logs and test assertions.
    pub fn argv_line(&self) -> String {
        std::iter::once(self.program.as_str()).chain(self.args.iter().map(String::as_str)).collect::<Vec<_>>().join(" ")
    }
}

impl fmt::Debug for HostCmd {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // stdin may hold a secret: show its size only.
        f.debug_struct("HostCmd")
            .field("program", &self.program)
            .field("args", &self.args)
            .field("cwd", &self.cwd)
            .field("stdin", &self.stdin.as_ref().map(|s| format!("<{} bytes>", s.len())))
            .field("on_line", &self.on_line.is_some())
            .field("timeout", &self.timeout)
            .finish()
    }
}

/// `async_trait` for the same reason as [`Runner`]: it is used as
/// `Arc<dyn HostExec>`.
#[async_trait]
pub trait HostExec: Send + Sync {
    /// Ok(Output) whenever the command ran, whatever its exit code. Err only when the host
    /// could not be reached; that error must be a `HostUnreachable` (downcastable via anyhow).
    async fn exec(&self, cmd: HostCmd) -> Result<Output>;
    /// argv for a local PTY to run `program args` on the host (local: as is).
    fn tty_argv(&self, program: &str, args: &[String]) -> (String, Vec<String>);
    async fn home(&self) -> Result<PathBuf>;
}

/// The host could not be reached (ssh failed to connect, the local program
/// could not be spawned). Not a failed command: that is an `Ok(Output)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostUnreachable {
    pub host: String,
    pub reason: String,
}

impl fmt::Display for HostUnreachable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "host '{}' is unreachable: {}", self.host, self.reason)
    }
}

impl std::error::Error for HostUnreachable {}

/// Is this error (anywhere in its chain) a [`HostUnreachable`]?
pub fn is_unreachable(e: &anyhow::Error) -> bool {
    e.chain().any(|c| c.is::<HostUnreachable>())
}

/// This machine, through `tokio::process`.
#[derive(Debug, Clone, Copy, Default)]
pub struct LocalExec;

#[async_trait]
impl HostExec for LocalExec {
    async fn exec(&self, cmd: HostCmd) -> Result<Output> {
        tracing::debug!(target: "host", "local: {}", cmd.argv_line());
        let mut c = tokio::process::Command::new(&cmd.program);
        c.args(&cmd.args)
            .stdin(if cmd.stdin.is_some() { Stdio::piped() } else { Stdio::null() })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        if let Some(dir) = &cmd.cwd {
            c.current_dir(dir);
        }
        // Locally, "could not spawn" is the closest thing to unreachable.
        let mut child = c.spawn().map_err(|e| {
            anyhow::Error::new(HostUnreachable { host: "local".into(), reason: format!("could not run `{}`: {e}", cmd.program) })
        })?;
        let stdin = child.stdin.take();
        let out = child.stdout.take();
        let err = child.stderr.take();
        let sink = cmd.on_line.clone();
        let data = cmd.stdin;
        let work = async move {
            // Feed stdin alongside draining: a child that writes before it
            // reads would otherwise deadlock on a full pipe.
            let feed = async move {
                if let (Some(data), Some(mut pipe)) = (data, stdin) {
                    // A child that exits without reading closes the pipe; not our error.
                    let _ = pipe.write_all(&data).await;
                }
            };
            let ((), stdout, stderr) = tokio::join!(feed, drain(out, sink.clone()), drain(err, sink));
            (stdout, stderr)
        };
        let (stdout, stderr, code) = match cmd.timeout {
            None => {
                let (o, e) = work.await;
                (o, e, child.wait().await.context("waiting for the command")?.code())
            }
            Some(t) => match tokio::time::timeout(t, work).await {
                Ok((o, e)) => (o, e, child.wait().await.context("waiting for the command")?.code()),
                Err(_) => {
                    let _ = child.kill().await;
                    (Vec::new(), format!("timed out after {t:?}").into_bytes(), None)
                }
            },
        };
        Ok(Output { code, stdout, stderr })
    }

    fn tty_argv(&self, program: &str, args: &[String]) -> (String, Vec<String>) {
        (program.to_string(), args.to_vec())
    }

    async fn home(&self) -> Result<PathBuf> {
        std::env::var_os("HOME").map(PathBuf::from).context("HOME is not set")
    }
}

/// A [`Runner`] that runs `program <inv.args>` on a host: how the typed
/// `sbx` client drives a remote `sbx`.
#[derive(Clone)]
pub struct OnHost {
    pub exec: Arc<dyn HostExec>,
    pub program: String,
}

#[async_trait]
impl Runner for OnHost {
    async fn run(&self, inv: Invocation) -> Result<Output> {
        self.exec
            .exec(HostCmd { program: self.program.clone(), args: inv.args, stdin: inv.stdin, on_line: inv.on_line, ..Default::default() })
            .await
    }
}

type ExecScript = Box<dyn Fn(&HostCmd) -> Result<Output> + Send + Sync>;

/// A scripted host for tests: records every [`HostCmd`], answers with a
/// closure (which may return a `HostUnreachable` to play a dead link).
/// `pub`, not `cfg(test)`, so every crate's tests can use it.
pub struct FakeExec {
    pub cmds: Mutex<Vec<HostCmd>>,
    pub home: PathBuf,
    answer: ExecScript,
}

impl FakeExec {
    pub fn new(answer: impl Fn(&HostCmd) -> Result<Output> + Send + Sync + 'static) -> Arc<Self> {
        Arc::new(Self { cmds: Mutex::new(Vec::new()), home: PathBuf::from("/home/fake"), answer: Box::new(answer) })
    }

    /// Answers every command with exit 0 and empty output.
    pub fn ok() -> Arc<Self> {
        Self::new(|_| Ok(Output::ok("")))
    }

    /// Every command so far as `program args…`.
    pub fn log(&self) -> Vec<String> {
        self.cmds.lock().unwrap().iter().map(HostCmd::argv_line).collect()
    }

    /// The stdin of every command that had one, in order.
    pub fn stdins(&self) -> Vec<Vec<u8>> {
        self.cmds.lock().unwrap().iter().filter_map(|c| c.stdin.clone()).collect()
    }

    pub fn called(&self, prefix: &str) -> bool {
        self.log().iter().any(|c| c.starts_with(prefix))
    }
}

#[async_trait]
impl HostExec for FakeExec {
    async fn exec(&self, cmd: HostCmd) -> Result<Output> {
        let out = (self.answer)(&cmd);
        if let (Ok(out), Some(sink)) = (&out, &cmd.on_line) {
            for line in String::from_utf8_lossy(&out.stdout).lines() {
                if !line.trim().is_empty() {
                    sink(line.trim());
                }
            }
        }
        self.cmds.lock().unwrap().push(cmd);
        out
    }

    fn tty_argv(&self, program: &str, args: &[String]) -> (String, Vec<String>) {
        (program.to_string(), args.to_vec())
    }

    async fn home(&self) -> Result<PathBuf> {
        Ok(self.home.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[tokio::test]
    async fn local_exec_pipes_stdin_streams_lines_and_keeps_the_exit_code() {
        let seen = Arc::new(Mutex::new(Vec::<String>::new()));
        let sk = seen.clone();
        let out = LocalExec
            .exec(HostCmd {
                program: "sh".into(),
                args: s(&["-c", "echo one; cat; echo err >&2; exit 3"]),
                stdin: Some(b"two\n".to_vec()),
                on_line: Some(Arc::new(move |l: &str| sk.lock().unwrap().push(l.into()))),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(out.code, Some(3));
        assert_eq!(out.stdout_str(), "one\ntwo\n");
        assert_eq!(String::from_utf8_lossy(&out.stderr), "err\n");
        let mut lines = seen.lock().unwrap().clone();
        lines.sort();
        assert_eq!(lines, ["err", "one", "two"]);
    }

    #[tokio::test]
    async fn local_exec_honours_cwd_and_timeout() {
        let out = LocalExec.exec(HostCmd { cwd: Some("/".into()), ..HostCmd::new("pwd", vec![]) }).await.unwrap();
        assert_eq!(out.stdout_str().trim(), "/");
        let slow = HostCmd { timeout: Some(Duration::from_millis(100)), ..HostCmd::new("sleep", s(&["5"])) };
        let t = std::time::Instant::now();
        let out = LocalExec.exec(slow).await.unwrap();
        assert_eq!(out.code, None);
        assert!(t.elapsed() < Duration::from_secs(4));
    }

    #[tokio::test]
    async fn local_exec_cannot_spawn_is_unreachable() {
        let e = LocalExec.exec(HostCmd::new("/no/such/program", vec![])).await.unwrap_err();
        assert!(e.downcast_ref::<HostUnreachable>().is_some());
        assert!(is_unreachable(&e));
    }

    #[tokio::test]
    async fn on_host_is_a_runner() {
        let r = OnHost { exec: Arc::new(LocalExec), program: "sh".into() };
        let out = r
            .run(Invocation { args: s(&["-c", "cat; echo \" $0\""]), stdin: Some(b"hi".to_vec()), on_line: None })
            .await
            .unwrap();
        assert!(out.success());
        assert_eq!(out.stdout_str(), "hi sh\n");
        let out = r.run(Invocation::new(s(&["-c", "exit 7"]))).await.unwrap();
        assert_eq!(out.code, Some(7));
    }

    #[tokio::test]
    async fn fake_exec_records_argv_and_stdin() {
        let f = FakeExec::new(|c| Ok(if c.program == "sbx" { Output::ok("v1\n") } else { Output::fail(1, "no") }));
        let r = OnHost { exec: f.clone(), program: "sbx".into() };
        let out = r.run(Invocation { args: s(&["secret", "set", "github"]), stdin: Some(b"tok".to_vec()), on_line: None }).await.unwrap();
        assert_eq!(out.stdout_str(), "v1\n");
        f.exec(HostCmd::new("git", s(&["status"]))).await.unwrap();
        assert_eq!(f.log(), ["sbx secret set github", "git status"]);
        assert_eq!(f.stdins(), [b"tok".to_vec()]);
        assert!(f.called("git"));
        assert_eq!(f.home().await.unwrap(), PathBuf::from("/home/fake"));
        // The secret never shows in Debug.
        assert!(!format!("{:?}", f.cmds.lock().unwrap()[0]).contains("tok"));
    }

    #[tokio::test]
    async fn host_unreachable_downcasts_through_context() {
        let f = FakeExec::new(|_| Err(HostUnreachable { host: "bb".into(), reason: "timed out".into() }.into()));
        let e = f.exec(HostCmd::new("true", vec![])).await.unwrap_err().context("running true");
        assert!(is_unreachable(&e));
        let u = e.chain().find_map(|c| c.downcast_ref::<HostUnreachable>()).unwrap();
        assert_eq!(u, &HostUnreachable { host: "bb".into(), reason: "timed out".into() });
        assert_eq!(u.to_string(), "host 'bb' is unreachable: timed out");
        assert!(!is_unreachable(&anyhow::anyhow!("other")));
    }
}
