//! Host-side alias management for "easy access to bound apps".
//!
//! Two modes (see `config::PortMap.host_ip`):
//!   * shared-loopback (default): everything binds 127.0.0.1 on distinct host
//!     ports, and /etc/hosts maps `<alias> -> 127.0.0.1`. You reach the app at
//!     `http://<alias>:<host_port>`.
//!   * ip-per-app: each app gets its own loopback alias IP (127.0.0.X). On macOS
//!     these must be created with `ifconfig lo0 alias`. /etc/hosts maps
//!     `<alias> -> 127.0.0.X`, so you reach the app at `http://<alias>:<port>`
//!     with its *natural* port (no remapping needed).
//!
//! All /etc/hosts edits live inside a single delimited block so they are trivial
//! to inspect and remove. Privileged steps are executed via `sudo` and will
//! prompt; nothing is done silently.

use anyhow::{bail, Context, Result};
use std::process::Command;

const BEGIN: &str = "# >>> sbxw managed block >>>";
const END: &str = "# <<< sbxw managed block <<<";
const HOSTS: &str = "/etc/hosts";

pub struct HostAlias {
    pub hostname: String,
    pub ip: String,
}

/// Ensure each `127.0.0.X` (X != 1) alias exists on lo0 (macOS only).
/// 127.0.0.1 is always present; other addresses are not, by default, on macOS.
pub fn ensure_loopback_aliases(aliases: &[HostAlias]) -> Result<()> {
    if !cfg!(target_os = "macos") {
        return Ok(()); // Linux already routes the whole 127.0.0.0/8.
    }
    for a in aliases {
        if a.ip == "127.0.0.1" {
            continue;
        }
        tracing::info!("aliasing loopback {} on lo0 (sudo)", a.ip);
        let status = Command::new("sudo")
            .args(["ifconfig", "lo0", "alias", &a.ip, "up"])
            .status()
            .context("failed to run sudo ifconfig")?;
        if !status.success() {
            bail!("could not add loopback alias {} (ifconfig)", a.ip);
        }
    }
    Ok(())
}

/// Rewrite the sbxw block in /etc/hosts to exactly match `aliases`.
/// Idempotent: removes any previous sbxw block first, then appends the new one.
pub fn sync_hosts_block(aliases: &[HostAlias]) -> Result<()> {
    let current = std::fs::read_to_string(HOSTS).unwrap_or_default();
    let stripped = strip_block(&current);

    let mut block = String::new();
    block.push_str(BEGIN);
    block.push('\n');
    for a in aliases {
        block.push_str(&format!("{}\t{}\n", a.ip, a.hostname));
    }
    block.push_str(END);
    block.push('\n');

    let mut next = stripped;
    if !next.ends_with('\n') {
        next.push('\n');
    }
    next.push_str(&block);

    if next == current {
        tracing::debug!("/etc/hosts already up to date");
        return Ok(());
    }

    tracing::info!("updating {} (sudo)", HOSTS);
    write_privileged(HOSTS, &next).context("failed to update /etc/hosts")
}

/// Read the current aliases from the sbxw-managed block in /etc/hosts.
pub fn read_hosts_block() -> Vec<HostAlias> {
    let content = std::fs::read_to_string(HOSTS).unwrap_or_default();
    let mut aliases = Vec::new();
    let mut in_block = false;
    for line in content.lines() {
        if line.trim() == BEGIN {
            in_block = true;
            continue;
        }
        if line.trim() == END {
            in_block = false;
            continue;
        }
        if in_block {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 2 {
                aliases.push(HostAlias {
                    ip: parts[0].to_string(),
                    hostname: parts[1].to_string(),
                });
            }
        }
    }
    aliases
}

/// Remove the sbxw-managed block from /etc/hosts (leaves everything else).
pub fn clear_hosts_block() -> Result<()> {
    let current = std::fs::read_to_string(HOSTS).unwrap_or_default();
    let stripped = strip_block(&current);
    if stripped == current {
        return Ok(());
    }
    write_privileged(HOSTS, &stripped)
}

fn strip_block(content: &str) -> String {
    let mut out = String::new();
    let mut in_block = false;
    for line in content.lines() {
        if line.trim() == BEGIN {
            in_block = true;
            continue;
        }
        if line.trim() == END {
            in_block = false;
            continue;
        }
        if !in_block {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

/// Write `content` to a root-owned file via `sudo tee`, avoiding the need to run
/// the whole wrapper as root.
fn write_privileged(path: &str, content: &str) -> Result<()> {
    use std::io::Write;
    use std::process::Stdio;

    let mut child = Command::new("sudo")
        .args(["tee", path])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()
        .context("failed to spawn `sudo tee`")?;
    child
        .stdin
        .take()
        .context("no stdin for sudo tee")?
        .write_all(content.as_bytes())?;
    let status = child.wait()?;
    if !status.success() {
        bail!("`sudo tee {path}` failed: {status}");
    }
    Ok(())
}
