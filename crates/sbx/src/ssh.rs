//! The SSH [`HostExec`]: runs a [`HostCmd`] on a remote host through `ssh`
//! (the argv from `RemoteHost::ssh_argv()`), quoting each argument for the
//! remote shell, sending secrets on stdin, and reporting a failed connection
//! (ssh's exit 255) as a [`HostUnreachable`].
//!
//! Every call goes through `-S <ControlPath>`: when the host's link (the
//! daemon's `host::link`) holds a ControlMaster there, the command rides that
//! connection and costs no handshake; when it does not, ssh connects on its
//! own, so an `SshExec` works with or without a link.

use crate::host::{HostCmd, HostExec, HostUnreachable, LocalExec, OnHost};
use crate::runner::Output;
use anyhow::{bail, Result};
use async_trait::async_trait;
use sbxw_core::names::HostName;
use sbxw_core::remote_hosts::RemoteHost;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::OnceCell;

/// ssh's own exit code for "could not connect" (or the connection dropped).
/// A remote command that itself exits 255 reads the same; nothing in sbxw does.
pub const SSH_UNREACHABLE: i32 = 255;

/// One word, single-quoted for a POSIX shell: sshd joins the command's words
/// with spaces and hands the line to the user's shell, so each must survive it
/// unchanged. Inside single quotes only `'` is special, written `'\''`.
/// Always quoted, needed or not, so there is one rule to get right.
pub fn quote(word: &str) -> String {
    let mut q = String::with_capacity(word.len() + 2);
    q.push('\'');
    for c in word.chars() {
        if c == '\'' {
            q.push_str("'\\''");
        } else {
            q.push(c);
        }
    }
    q.push('\'');
    q
}

/// The remote command line for `program args…`, run in `cwd` when given.
pub fn remote_words(program: &str, args: &[String], cwd: Option<&str>) -> Vec<String> {
    let mut w = Vec::with_capacity(args.len() + 4);
    if let Some(dir) = cwd {
        w.extend([quote("cd"), quote(dir), "&&".to_string()]);
    }
    w.push(quote(program));
    w.extend(args.iter().map(|a| quote(a)));
    w
}

/// 32-bit FNV-1a: tiny, and unlike `DefaultHasher` guaranteed the same on
/// every build, so a ControlPath outlives an upgrade of sbxw.
fn fnv1a(s: &str) -> u32 {
    s.bytes().fold(0x811c_9dc5u32, |h, b| (h ^ b as u32).wrapping_mul(0x0100_0193))
}

/// `<home>/.sbxw/run/ssh/<8 hex>`: short and stable per host name. macOS caps
/// a unix socket path at 104 bytes and ssh appends a temporary suffix while
/// it sets the master up, so the host name itself stays out of the path.
pub fn control_path(home: &Path, host: &HostName) -> PathBuf {
    home.join(".sbxw/run/ssh").join(format!("{:08x}", fnv1a(host.as_str())))
}

/// A remote host, reached through `ssh`.
pub struct SshExec {
    host: RemoteHost,
    /// The ssh program; a path so tests can put `scripts/fake-ssh` there.
    ssh: PathBuf,
    control_path: PathBuf,
    home: OnceCell<PathBuf>,
    sbx: OnceCell<String>,
}

impl SshExec {
    pub fn new(host: RemoteHost, control_path: PathBuf) -> Self {
        Self::with_ssh(host, control_path, "ssh")
    }

    pub fn with_ssh(host: RemoteHost, control_path: PathBuf, ssh: impl Into<PathBuf>) -> Self {
        Self { host, ssh: ssh.into(), control_path, home: OnceCell::new(), sbx: OnceCell::new() }
    }

    pub fn host(&self) -> &RemoteHost {
        &self.host
    }

    pub fn control_path(&self) -> &Path {
        &self.control_path
    }

    fn ssh_str(&self) -> String {
        self.ssh.to_string_lossy().into_owned()
    }

    /// `-S <ControlPath> <ssh_argv()>`: options and destination every call shares.
    fn base_args(&self) -> Vec<String> {
        let mut a = vec!["-S".to_string(), self.control_path.to_string_lossy().into_owned()];
        a.extend(self.host.ssh_argv());
        a
    }

    fn unreachable(&self, reason: impl Into<String>) -> anyhow::Error {
        HostUnreachable { host: self.host.name.as_str().to_string(), reason: reason.into() }.into()
    }

    /// The `sbx` to run on the host: `sbx_path` when set, else what
    /// `sh -lc 'command -v sbx'` finds (a login shell, since a
    /// non-interactive ssh PATH often lacks it), asked once and cached.
    pub async fn sbx_program(&self) -> Result<String> {
        if let Some(p) = &self.host.sbx_path {
            return Ok(p.clone());
        }
        self.sbx
            .get_or_try_init(|| async {
                let out = self.exec(HostCmd::new("sh", vec!["-lc".into(), "command -v sbx".into()])).await?;
                let found = out.stdout_str().lines().map(str::trim).find(|l| !l.is_empty()).map(String::from);
                match found {
                    Some(p) if out.success() => Ok(p),
                    _ => bail!("sbx was not found on host '{}' (set sbx_path with `sbxw host edit`)", self.host.name.as_str()),
                }
            })
            .await
            .cloned()
    }

    /// A [`Runner`](crate::runner::Runner) for the typed `sbx` client on this
    /// host, with the detected (or configured) `sbx`.
    pub async fn on_host(self: &Arc<Self>) -> Result<OnHost> {
        Ok(OnHost { program: self.sbx_program().await?, exec: self.clone() })
    }
}

#[async_trait]
impl HostExec for SshExec {
    async fn exec(&self, cmd: HostCmd) -> Result<Output> {
        tracing::debug!(target: "host", "{}: {}", self.host.name.as_str(), cmd.argv_line());
        let mut args = self.base_args();
        args.push("--".into());
        args.extend(remote_words(&cmd.program, &cmd.args, cmd.cwd.as_deref()));
        // ssh's stdin is closed when the command has none, so a remote prompt
        // (sbx's first-run network policy question) sees EOF and fails
        // instead of hanging.
        let local = HostCmd { program: self.ssh_str(), args, cwd: None, stdin: cmd.stdin, on_line: cmd.on_line, timeout: cmd.timeout };
        let out = LocalExec.exec(local).await.map_err(|e| self.unreachable(format!("{e:#}")))?;
        if out.code == Some(SSH_UNREACHABLE) {
            let why = String::from_utf8_lossy(&out.stderr);
            let why = why.lines().map(str::trim).rfind(|l| !l.is_empty()).unwrap_or("ssh exited 255");
            return Err(self.unreachable(why.to_string()));
        }
        Ok(out)
    }

    fn tty_argv(&self, program: &str, args: &[String]) -> (String, Vec<String>) {
        // The whole ssh_argv, not just the destination: without a master on
        // the ControlPath ssh connects itself and needs port, user and key.
        let mut a = vec!["-S".to_string(), self.control_path.to_string_lossy().into_owned(), "-tt".into()];
        a.extend(self.host.ssh_argv());
        a.push("--".into());
        a.extend(remote_words(program, args, None));
        (self.ssh_str(), a)
    }

    async fn home(&self) -> Result<PathBuf> {
        self.home
            .get_or_try_init(|| async {
                let out = self.exec(HostCmd::new("sh", vec!["-c".into(), "printf %s \"$HOME\"".into()])).await?;
                let home = out.stdout_str();
                if !out.success() || !home.starts_with('/') {
                    bail!("host '{}' did not say where its home is (got {home:?})", self.host.name.as_str());
                }
                Ok(PathBuf::from(home))
            })
            .await
            .cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host(alias: &str) -> RemoteHost {
        RemoteHost::new("bb", Some(alias.into()), None, None, None, None, None, false).unwrap()
    }

    /// What a POSIX shell makes of a quoted word: the word, unchanged.
    fn through_sh(words: &[&str]) -> Vec<String> {
        let line = std::iter::once("printf '%s\\0'".to_string()).chain(words.iter().map(|w| quote(w))).collect::<Vec<_>>().join(" ");
        let out = std::process::Command::new("sh").args(["-c", &line]).output().unwrap();
        assert!(out.status.success(), "{line}");
        let s = String::from_utf8(out.stdout).unwrap();
        s.split_terminator('\0').map(String::from).collect()
    }

    const HOSTILE: &[&str] =
        &["$(id)", "`id`", "it's", "'", "''", "\"q\"", "a b  c", "line\nbreak", "", "-rf", "--", "*", "~", "a;b|c&d", "$HOME", "\\", "!x", "é"];

    #[test]
    fn quoting_keeps_hostile_words_whole() {
        assert_eq!(through_sh(HOSTILE), HOSTILE);
    }

    #[test]
    fn quoting_is_single_quotes_with_escaped_quote() {
        assert_eq!(quote(""), "''");
        assert_eq!(quote("a b"), "'a b'");
        assert_eq!(quote("it's"), "'it'\\''s'");
        assert_eq!(quote("-n"), "'-n'");
        assert_eq!(quote("$(id)"), "'$(id)'");
    }

    #[test]
    fn cwd_becomes_cd_and() {
        let w = remote_words("git", &["status".into()], Some("/w/it's here"));
        assert_eq!(w.join(" "), "'cd' '/w/it'\\''s here' && 'git' 'status'");
    }

    #[test]
    fn control_path_is_short_and_stable() {
        let a = control_path(Path::new("/Users/someone"), &HostName::parse("buildbox").unwrap());
        let b = control_path(Path::new("/Users/someone"), &HostName::parse("buildbox").unwrap());
        let c = control_path(Path::new("/Users/someone"), &HostName::parse("other").unwrap());
        assert_eq!(a, b);
        assert_ne!(a, c);
        let name = a.file_name().unwrap().to_str().unwrap();
        assert_eq!(name.len(), 8);
        assert!(name.chars().all(|c| c.is_ascii_hexdigit()));
        assert!(a.starts_with("/Users/someone/.sbxw/run/ssh"));
        // Pinned: a change would orphan every running master.
        assert_eq!(name, "5e996e64");
    }

    #[test]
    fn tty_argv_goes_through_the_master_with_a_tty() {
        let x = SshExec::new(host("bb.example"), "/c/p".into());
        let (prog, args) = x.tty_argv("sbx", &["run".into(), "--name".into(), "my box".into()]);
        assert_eq!(prog, "ssh");
        assert_eq!(
            args,
            ["-S", "/c/p", "-tt", "-o", "BatchMode=yes", "-o", "StrictHostKeyChecking=yes", "-o", "ConnectTimeout=2", "bb.example", "--", "'sbx'", "'run'", "'--name'", "'my box'"]
        );
    }
}
