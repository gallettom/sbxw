//! The two privileged things sbxw does on the host: its `/etc/hosts` block
//! and, in `ip_per_app` mode on macOS, loopback aliases on `lo0`.
//!
//! Who may be asked for a password is an *argument* here ([`Elevation`]), not
//! a probe of stdin: the CLI runs in a terminal where `sudo` can prompt, the
//! daemon never does. Off a terminal `sudo -n` fails fast, and on macOS the
//! system's own authentication panel is the one prompt a process without a
//! terminal can still put in front of the user. Whatever cannot be written is
//! parked for `sbxw hosts sync` — never fatal.
//!
//! The steady state needs no privilege at all: a write is skipped when the
//! block already says what it should (see `sbxw_core::hosts::merge_into`).

use crate::store::Store;
use anyhow::{bail, Context, Result};
use sbxw_core::hosts::{self, HostAlias, SYNC_HINT};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Elevation {
    /// A person is at a terminal: `sudo` may ask.
    Interactive,
    /// The daemon: `sudo -n`, then the macOS panel.
    Background,
}

pub struct HostSys {
    pub hosts_file: PathBuf,
    pub elevation: Elevation,
    /// Off in tests: nothing privileged is ever attempted.
    pub privileged: bool,
}

impl HostSys {
    /// The real `/etc/hosts` — or, when `SBXW_HOSTS_FILE` names another file,
    /// that one, written directly (the end-to-end tests, and anyone who keeps
    /// a hosts file they own).
    pub fn system(elevation: Elevation) -> Self {
        if let Some(file) = std::env::var_os("SBXW_HOSTS_FILE").filter(|f| !f.is_empty()) {
            return Self {
                hosts_file: PathBuf::from(file),
                elevation,
                privileged: false,
            };
        }
        Self {
            hosts_file: PathBuf::from("/etc/hosts"),
            elevation,
            privileged: true,
        }
    }

    pub fn read_block(&self) -> Vec<HostAlias> {
        hosts::parse_block(&std::fs::read_to_string(&self.hosts_file).unwrap_or_default())
    }

    pub fn missing(&self, wanted: &[HostAlias]) -> Vec<String> {
        hosts::missing(&self.read_block(), wanted)
    }

    /// Merge `wanted` into the block (and lo0), returning one warning per step
    /// that did not work; parks what is still missing.
    pub fn apply(&self, store: &Store, wanted: &[HostAlias]) -> Vec<String> {
        let mut warnings = Vec::new();
        if let Err(e) = self.ensure_loopback(wanted) {
            warnings.push(format!("{e:#}"));
        }
        if let Err(e) = self.merge(wanted) {
            warnings.push(format!("/etc/hosts not updated: {e:#}"));
        }
        let missing = self.missing(wanted);
        if missing.is_empty() {
            return warnings;
        }
        if let Err(e) = store.park_hosts(wanted) {
            tracing::warn!("could not park pending aliases: {e:#}");
        }
        warnings.push(format!(
            "these hostnames won't resolve yet: {} ({SYNC_HINT})",
            missing.join(", ")
        ));
        warnings
    }

    pub fn merge(&self, wanted: &[HostAlias]) -> Result<()> {
        let current = std::fs::read_to_string(&self.hosts_file).unwrap_or_default();
        match hosts::merge_into(&current, wanted) {
            None => Ok(()),
            Some(next) => self.write(&next),
        }
    }

    pub fn clear(&self) -> Result<()> {
        let current = std::fs::read_to_string(&self.hosts_file).unwrap_or_default();
        let stripped = hosts::strip_block(&current);
        if stripped == current {
            return Ok(());
        }
        self.write(&stripped)
    }

    fn can_elevate(&self) -> bool {
        self.elevation == Elevation::Interactive
            || Command::new(sbxw_core::sysbin::sys_bin("sudo"))
                .args(["-n", "true"])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .is_ok_and(|s| s.success())
    }

    fn sudo(&self, args: &[&str]) -> Command {
        let mut c = Command::new(sbxw_core::sysbin::sys_bin("sudo"));
        if self.elevation == Elevation::Background {
            c.arg("-n");
        }
        c.args(args);
        c
    }

    fn write(&self, content: &str) -> Result<()> {
        if !self.privileged {
            // Tests write to a plain file they own.
            return std::fs::write(&self.hosts_file, content).context("writing hosts file");
        }
        let path = self.hosts_file.to_string_lossy().into_owned();
        tracing::info!("updating {path}");
        if !self.can_elevate() {
            return self.write_as_admin(&path, content);
        }
        match sudo_tee(self.sudo(&["tee", &path]), content)? {
            None => Ok(()),
            Some(stderr) if hosts::sudo_refused(&stderr) && self.elevation == Elevation::Background => {
                tracing::info!("sudo declined — asking macOS instead");
                self.write_as_admin(&path, content)
            }
            Some(stderr) => bail!("`sudo tee {path}` failed: {}", hosts::explain(&stderr)),
        }
    }

    /// Stage in a private file (opened `O_EXCL`: a planted path fails rather
    /// than being followed), then have root `cp` it in place, which keeps the
    /// target's own owner and mode.
    fn write_as_admin(&self, path: &str, content: &str) -> Result<()> {
        use std::io::Write;
        #[cfg(unix)]
        use std::os::unix::fs::OpenOptionsExt;
        let staged = std::env::temp_dir().join(format!("sbxw-hosts-staged.{}", std::process::id()));
        let _ = std::fs::remove_file(&staged);
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        opts.mode(0o600);
        opts.open(&staged)
            .with_context(|| format!("could not stage {}", staged.display()))?
            .write_all(content.as_bytes())?;
        let result = run_as_admin(&format!(
            "/bin/cp {} {}",
            hosts::shell_quote(&staged.to_string_lossy()),
            hosts::shell_quote(path)
        ));
        let _ = std::fs::remove_file(&staged);
        result
    }

    /// Each `127.0.0.X` (X ≠ 1) on lo0 — macOS only; Linux routes all of
    /// 127.0.0.0/8. Read once; an alias that exists costs nothing.
    pub fn ensure_loopback(&self, aliases: &[HostAlias]) -> Result<()> {
        if let Some(bad) = aliases.iter().find(|a| !hosts::is_ipv4_literal(&a.ip)) {
            bail!("'{}' is not an IPv4 address", bad.ip);
        }
        if !cfg!(target_os = "macos") || !self.privileged || aliases.iter().all(|a| a.ip == "127.0.0.1") {
            return Ok(());
        }
        let present = Command::new(sbxw_core::sysbin::sys_bin("ifconfig"))
            .arg("lo0")
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| hosts::parse_loopback_addresses(&String::from_utf8_lossy(&o.stdout)))
            .unwrap_or_default();
        for a in aliases {
            if a.ip == "127.0.0.1" || present.contains(&a.ip) {
                continue;
            }
            tracing::info!("aliasing loopback {} on lo0", a.ip);
            let argv = ["ifconfig", "lo0", "alias", a.ip.as_str(), "up"];
            let admin = format!("/sbin/ifconfig lo0 alias {} up", a.ip);
            if !self.can_elevate() {
                run_as_admin(&admin)?;
                continue;
            }
            let out = self.sudo(&argv).stdin(Stdio::null()).output()?;
            if !out.status.success() {
                let stderr = String::from_utf8_lossy(&out.stderr);
                if hosts::sudo_refused(&stderr) && self.elevation == Elevation::Background {
                    run_as_admin(&admin)?;
                } else {
                    bail!("could not add loopback alias {}: {}", a.ip, hosts::explain(&stderr));
                }
            }
        }
        Ok(())
    }
}

/// Feed `content` to a `sudo tee`. `Ok(None)` when it landed, `Ok(Some(stderr))`
/// when tee ran and failed.
fn sudo_tee(mut cmd: Command, content: &str) -> Result<Option<String>> {
    use std::io::{Read, Write};
    let mut child = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .context("failed to spawn `sudo tee`")?;
    let write_err = child.stdin.take().context("no stdin")?.write_all(content.as_bytes()).err();
    let mut stderr = String::new();
    if let Some(mut p) = child.stderr.take() {
        let _ = p.read_to_string(&mut stderr);
    }
    if !child.wait()?.success() {
        return Ok(Some(stderr));
    }
    if let Some(e) = write_err {
        bail!("`sudo tee` did not take the new content: {e}");
    }
    Ok(None)
}

#[cfg(target_os = "macos")]
fn run_as_admin(shell_command: &str) -> Result<()> {
    tracing::info!("asking macOS for administrator rights (no terminal for sudo)");
    let script = format!(
        "do shell script {} with administrator privileges",
        hosts::applescript_quote(shell_command)
    );
    let out = Command::new(sbxw_core::sysbin::sys_bin("osascript"))
        .args(["-e", &script])
        .stdin(Stdio::null())
        .output()
        .context("failed to run osascript")?;
    if out.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    if stderr.contains("-128") {
        bail!("the macOS password prompt was dismissed — {SYNC_HINT}");
    }
    bail!("macOS authentication failed: {}", hosts::explain(&stderr));
}

#[cfg(not(target_os = "macos"))]
fn run_as_admin(_shell_command: &str) -> Result<()> {
    bail!("sudo has no cached password left and no terminal to ask on — {SYNC_HINT}")
}

/// A hosts file a test owns.
pub fn for_tests(path: &Path) -> HostSys {
    HostSys {
        hosts_file: path.to_path_buf(),
        elevation: Elevation::Background,
        privileged: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Paths;

    #[test]
    fn applying_merges_and_a_second_time_changes_nothing() {
        let d = tempfile::tempdir().unwrap();
        let file = d.path().join("hosts");
        std::fs::write(&file, "127.0.0.1\tlocalhost\n").unwrap();
        let sys = for_tests(&file);
        let store = Store::open(Paths::under(d.path()));
        let want = [HostAlias::new("neos.local", "127.0.0.1")];
        assert!(sys.apply(&store, &want).is_empty());
        let once = std::fs::read_to_string(&file).unwrap();
        assert!(sys.apply(&store, &want).is_empty());
        assert_eq!(std::fs::read_to_string(&file).unwrap(), once);
        sys.clear().unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "127.0.0.1\tlocalhost\n");
    }

    #[test]
    fn a_bad_ip_never_reaches_a_command() {
        let d = tempfile::tempdir().unwrap();
        let sys = for_tests(&d.path().join("h"));
        assert!(sys.ensure_loopback(&[HostAlias::new("x", "$(id)")]).is_err());
    }
}
