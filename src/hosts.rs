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
//!
//! Two rules keep those privileged steps out of the web daemon's way, because a
//! daemon cannot answer a password prompt (see `can_prompt`):
//!   * every privileged step is skipped when the system is already in the
//!     wanted state, so the steady state needs no `sudo` at all;
//!   * off a terminal `sudo` runs with `-n`, so a missing password is an
//!     instant, reportable error instead of a prompt written into a terminal
//!     nobody is watching.

use anyhow::{bail, Context, Result};
use std::io::IsTerminal;
use std::process::{Command, Stdio};

const BEGIN: &str = "# >>> sbxw managed block >>>";
const END: &str = "# <<< sbxw managed block <<<";
const HOSTS: &str = "/etc/hosts";

pub struct HostAlias {
    pub hostname: String,
    pub ip: String,
}

/// Whether this process can put a password prompt in front of somebody.
///
/// Only when stdin is a terminal — i.e. a human typed `sbxw …` and is waiting
/// for it. The web daemon is spawned with stdin on /dev/null yet *keeps the
/// launching shell's controlling terminal* (`cmd_up_background` changes the
/// process group, not the session), so an interactive `sudo` there writes
/// "Password:" into a terminal nobody is reading — blocking the browser request
/// that triggered it until `passwd_timeout` — or, once that window is closed,
/// dies with "no tty present". Either way the caller learns nothing useful.
fn can_prompt() -> bool {
    std::io::stdin().is_terminal()
}

/// `sudo` for one privileged step: non-interactive unless somebody is watching.
fn sudo(args: &[&str]) -> Command {
    let mut cmd = Command::new("sudo");
    if !can_prompt() {
        cmd.arg("-n"); // never prompt: fail now, with a message we can report
    }
    cmd.args(args);
    cmd
}

/// Whether a privileged step could run *right now* without anyone typing
/// anything: we can prompt, or sudo still holds a valid timestamp (or this user
/// needs no password at all).
pub fn can_elevate() -> bool {
    can_prompt()
        || Command::new("sudo")
            .args(["-n", "true"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
}

/// Where a privileged step that could not run gets redone: a terminal is the
/// one place a `sudo` password prompt can be answered.
pub const SYNC_HINT: &str = "run `sbxw hosts sync` in a terminal to apply it";

/// What to tell the user when sudo is out of reach and nothing else can ask.
/// macOS has the authentication panel for that case, so nothing there says it.
#[cfg_attr(target_os = "macos", allow(dead_code))]
pub const NEEDS_SUDO_HINT: &str =
    "sudo has no cached password left and no terminal to ask on — run `sbxw hosts sync` \
     in a terminal to apply it";

/// The one line of a command's stderr worth repeating.
fn first_line(stderr: &str) -> &str {
    stderr
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("no output")
}

/// Whether `stderr` is sudo declining to authenticate, as opposed to the
/// command failing on its own terms.
///
/// The distinction is what makes a second attempt worth anything: a refusal
/// means the write is still possible for whoever *can* answer a prompt (the
/// macOS panel, a terminal), while a read-only /etc or a full disk fails the
/// same way however you ask.
fn sudo_refused(stderr: &str) -> bool {
    let lower = first_line(stderr).to_ascii_lowercase();
    lower.starts_with("sudo:")
        || ["password", "no tty", "terminal", "askpass"]
            .iter()
            .any(|s| lower.contains(s))
}

/// Turn a failed privileged command into one line that says what to do next.
fn explain(stderr: &str) -> String {
    let detail = first_line(stderr);
    if sudo_refused(stderr) {
        format!("{detail} — {SYNC_HINT}")
    } else {
        detail.to_string()
    }
}

/// Run one privileged command, by whichever route can actually authenticate.
///
/// `argv` goes to `sudo` (no shell involved); `admin_form` is the same command
/// as a `/bin/sh` string for the macOS panel. The panel is used when sudo has
/// nothing cached and no terminal to ask on — and *also* when sudo was reachable
/// a moment ago and refused anyway, since `can_elevate` can only report what was
/// true when it looked.
fn run_privileged(argv: &[&str], admin_form: &str) -> Result<()> {
    if !can_elevate() {
        return run_as_admin(admin_form);
    }
    let out = sudo(argv)
        .stdin(Stdio::null())
        .output()
        .with_context(|| format!("failed to run sudo {}", argv.first().unwrap_or(&"")))?;
    if out.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    if sudo_refused(&stderr) && !can_prompt() {
        tracing::info!(
            "sudo declined ({}) — asking macOS instead",
            first_line(&stderr)
        );
        return run_as_admin(admin_form);
    }
    bail!("{}", explain(&stderr))
}

/// Loopback IPs already configured on lo0, so an alias that exists costs no
/// `sudo`. Empty on any parse/exec failure: the caller then just tries.
fn existing_loopback_aliases() -> Vec<String> {
    Command::new("ifconfig")
        .arg("lo0")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .filter_map(|l| {
                    let mut it = l.split_whitespace();
                    (it.next() == Some("inet")).then(|| it.next()).flatten()
                })
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// Ensure each `127.0.0.X` (X != 1) alias exists on lo0 (macOS only).
/// 127.0.0.1 is always present; other addresses are not, by default, on macOS.
pub fn ensure_loopback_aliases(aliases: &[HostAlias]) -> Result<()> {
    // Before anything else, and on every platform: an IP that is not an IPv4
    // literal is a caller's mistake worth naming, and never something to pass
    // on to `ifconfig` — let alone to a shell string (see `run_as_admin`).
    if let Some(bad) = aliases.iter().find(|a| !is_ipv4_literal(&a.ip)) {
        bail!("'{}' is not an IPv4 address", bad.ip);
    }
    if !cfg!(target_os = "macos") {
        return Ok(()); // Linux already routes the whole 127.0.0.0/8.
    }
    // Read lo0 once, and only if there is something that could need adding:
    // every `sbxw up` used to shell out to `sudo ifconfig` per alias, which is
    // what made a long-running daemon hit the password prompt again and again.
    if aliases.iter().all(|a| a.ip == "127.0.0.1") {
        return Ok(());
    }
    let present = existing_loopback_aliases();
    for a in aliases {
        if a.ip == "127.0.0.1" || present.iter().any(|p| p == &a.ip) {
            continue;
        }
        tracing::info!("aliasing loopback {} on lo0 (sudo)", a.ip);
        run_privileged(
            &["ifconfig", "lo0", "alias", &a.ip, "up"],
            &format!("/sbin/ifconfig lo0 alias {} up", a.ip),
        )
        .with_context(|| format!("could not add loopback alias {}", a.ip))?;
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

/// Add `aliases` to the managed block, keeping every entry already in it.
///
/// This is what provisioning uses. `sync_hosts_block` states the block
/// *exactly*, so one sandbox's aliases would evict the previous one's — and
/// each eviction is another privileged write, which is precisely the write that
/// fails once the daemon has outlived sudo's cached password. Merging means the
/// steady state is "nothing to change", and nothing to change needs no sudo.
pub fn merge_hosts_block(aliases: &[HostAlias]) -> Result<()> {
    sync_hosts_block(&merged_entries(read_hosts_block(), aliases))
}

/// `stored` with `wanted` folded in: order preserved, no duplicate hostname,
/// and a hostname that appears in both takes `wanted`'s IP (so an alias that
/// moved is updated in place rather than listed twice).
fn merged_entries(stored: Vec<HostAlias>, wanted: &[HostAlias]) -> Vec<HostAlias> {
    let mut merged: Vec<HostAlias> = stored
        .into_iter()
        .filter(|old| !wanted.iter().any(|new| new.hostname == old.hostname))
        .collect();
    merged.extend(wanted.iter().map(|a| HostAlias {
        hostname: a.hostname.clone(),
        ip: a.ip.clone(),
    }));
    merged
}

/// The subset of `aliases` that is not in /etc/hosts (under the wanted IP).
/// Used to tell the user exactly what is missing when the write could not run.
pub fn missing_aliases(aliases: &[HostAlias]) -> Vec<String> {
    let have = read_hosts_block();
    aliases
        .iter()
        .filter(|a| {
            !a.hostname.is_empty()
                && !have
                    .iter()
                    .any(|h| h.hostname == a.hostname && h.ip == a.ip)
        })
        .map(|a| a.hostname.clone())
        .collect()
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

/// A plain dotted-quad IPv4 literal, e.g. `127.0.0.2`.
///
/// Checked because the alias IP can arrive from an HTTP body (the ports panel's
/// `host_ip`) and, on the macOS escalation path below, ends up inside a shell
/// command string rather than an argv slot.
fn is_ipv4_literal(s: &str) -> bool {
    let mut parts = s.split('.');
    let ok = (0..4).all(|_| parts.next().is_some_and(|p| p.parse::<u8>().is_ok()));
    ok && parts.next().is_none()
}

/// Quote one argument for `/bin/sh`.
fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Escape a string for an AppleScript string literal.
// Only the macOS escalation builds a script, but the escaping is where a
// mistake would be worst, so it stays compiled and tested on every platform.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn applescript_quote(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

/// Run `shell_command` as root through the macOS authentication panel.
///
/// This is the one privileged path a daemon can actually complete: `sudo` needs
/// a terminal to prompt on and the daemon has none, but `do shell script … with
/// administrator privileges` makes the system itself ask — a window the user
/// sees whether they are in the browser, in another app, or nowhere near the
/// terminal sbxw was started from.
#[cfg(target_os = "macos")]
fn run_as_admin(shell_command: &str) -> Result<()> {
    tracing::info!("asking macOS for administrator rights (no terminal for sudo)");
    let script = format!(
        "do shell script {} with administrator privileges",
        applescript_quote(shell_command)
    );
    let out = Command::new("osascript")
        .args(["-e", &script])
        .stdin(Stdio::null())
        .output()
        .context("failed to run osascript")?;
    if out.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    // -128 is "User canceled": their answer, not a failure to report as one.
    if stderr.contains("-128") {
        bail!("the macOS password prompt was dismissed — {SYNC_HINT}");
    }
    bail!("macOS authentication failed: {}", explain(&stderr));
}

#[cfg(not(target_os = "macos"))]
fn run_as_admin(_shell_command: &str) -> Result<()> {
    bail!("{NEEDS_SUDO_HINT}")
}

/// Stage `content` in a file only this user can write, then have root copy it
/// over `path`. `cp` writes *into* the existing file, so /etc/hosts keeps its
/// own owner and mode.
fn write_as_admin(path: &str, content: &str) -> Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;

    let staged = crate::state_dir().join(format!("hosts-staged.{}", std::process::id()));
    let _ = std::fs::remove_file(&staged);
    // `create_new` + 0600: root is about to copy this file, so it must not be
    // one an attacker planted (a symlink here would make O_EXCL fail) or can
    // rewrite between the staging and the copy.
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&staged)
        .with_context(|| {
            format!(
                "could not stage {} for a privileged write",
                staged.display()
            )
        })?
        .write_all(content.as_bytes())?;
    let result = run_as_admin(&format!(
        "/bin/cp {} {}",
        shell_quote(&staged.to_string_lossy()),
        shell_quote(path)
    ));
    let _ = std::fs::remove_file(&staged);
    result
}

/// Write `content` to a root-owned file via `sudo tee`, avoiding the need to run
/// the whole wrapper as root.
fn write_privileged(path: &str, content: &str) -> Result<()> {
    // Checked before spawning: off a terminal a stale sudo timestamp makes
    // `sudo -n` exit before reading its stdin, and the write below would then
    // fail with a bare "broken pipe" that says nothing about why. That is also
    // the case the macOS panel exists for — nobody can type into sudo, but
    // somebody can still answer the system.
    if !can_elevate() {
        return write_as_admin(path, content);
    }
    let Some(stderr) = sudo_tee(path, content)? else {
        return Ok(());
    };
    // sudo was reachable when `can_elevate` looked and refused a moment later:
    // the timestamp expired in between, or it was never this process's to use
    // (sudo keys its timestamp by terminal). The panel can still ask.
    if sudo_refused(&stderr) && !can_prompt() {
        tracing::info!(
            "sudo declined ({}) — asking macOS instead",
            first_line(&stderr)
        );
        return write_as_admin(path, content);
    }
    bail!("`sudo tee {path}` failed: {}", explain(&stderr))
}

/// Feed `content` to `sudo tee path`. `Ok(None)` when it landed, `Ok(Some(…))`
/// with tee's stderr when the command ran and failed — for the caller to decide
/// whether that failure is worth asking somebody about — and `Err` when it could
/// not be run at all.
fn sudo_tee(path: &str, content: &str) -> Result<Option<String>> {
    use std::io::{Read, Write};

    let mut child = sudo(&["tee", path])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .context("failed to spawn `sudo tee`")?;
    // Dropped at the end of the statement, closing tee's stdin.
    let write_err = child
        .stdin
        .take()
        .context("no stdin for sudo tee")?
        .write_all(content.as_bytes())
        .err();
    let mut stderr = String::new();
    if let Some(mut pipe) = child.stderr.take() {
        let _ = pipe.read_to_string(&mut stderr);
    }
    let status = child.wait()?;
    if !status.success() {
        return Ok(Some(stderr));
    }
    if let Some(e) = write_err {
        bail!("`sudo tee {path}` did not take the new content: {e}");
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn alias(hostname: &str, ip: &str) -> HostAlias {
        HostAlias {
            hostname: hostname.into(),
            ip: ip.into(),
        }
    }

    fn pairs(entries: &[HostAlias]) -> Vec<(&str, &str)> {
        entries
            .iter()
            .map(|a| (a.hostname.as_str(), a.ip.as_str()))
            .collect()
    }

    /// The reason merging exists: provisioning one sandbox must not evict the
    /// aliases of the ones provisioned before it. Every eviction is another
    /// privileged write, and a privileged write is exactly what the web daemon
    /// cannot do once sudo's cached password has expired.
    #[test]
    fn merging_keeps_what_other_sandboxes_put_there() {
        let stored = vec![
            alias("neos.local", "127.0.0.1"),
            alias("app-a", "127.0.0.1"),
        ];
        let merged = merged_entries(stored, &[alias("app-b", "127.0.0.1")]);
        assert_eq!(
            pairs(&merged),
            vec![
                ("neos.local", "127.0.0.1"),
                ("app-a", "127.0.0.1"),
                ("app-b", "127.0.0.1"),
            ]
        );
    }

    #[test]
    fn a_hostname_that_moved_is_updated_not_duplicated() {
        let stored = vec![alias("app", "127.0.0.2"), alias("other", "127.0.0.1")];
        let merged = merged_entries(stored, &[alias("app", "127.0.0.3")]);
        assert_eq!(
            pairs(&merged),
            vec![("other", "127.0.0.1"), ("app", "127.0.0.3")]
        );
    }

    /// Re-stating what is already there must produce the identical block:
    /// `sync_hosts_block` compares the rendered file and skips the `sudo` write
    /// when nothing changed, which is what makes the steady state password-free.
    #[test]
    fn re_stating_the_same_aliases_changes_nothing() {
        let wanted = [
            alias("sbxw.localhost", "127.0.0.1"),
            alias("app", "127.0.0.1"),
        ];
        let stored = merged_entries(Vec::new(), &wanted);
        let again = merged_entries(stored, &wanted);
        assert_eq!(
            pairs(&again),
            vec![("sbxw.localhost", "127.0.0.1"), ("app", "127.0.0.1")]
        );
    }

    /// The classification the late fallback turns on: a refusal is worth
    /// asking somebody else about (the macOS panel), a failed write is not.
    #[test]
    fn a_refusal_is_told_apart_from_a_failed_write() {
        assert!(sudo_refused("sudo: a password is required"));
        assert!(sudo_refused(
            "sudo: no tty present and no askpass program specified"
        ));
        assert!(sudo_refused("\n  sudo: 1 incorrect password attempt\n"));
        assert!(!sudo_refused("tee: /etc/hosts: Read-only file system"));
        assert!(!sudo_refused("ifconfig: ioctl (SIOCAIFADDR): File exists"));
        assert!(!sudo_refused(""));
        // The hint mentions a terminal; reading a message we wrote back as a
        // refusal would send every failure to the panel.
        assert!(!sudo_refused(&explain("tee: /etc/hosts: No such file")));
    }

    /// The recovery hint is for one failure only — sudo refusing to
    /// authenticate. Suffixing it onto a read-only /etc or a full disk would
    /// send the user to a terminal to watch the same write fail again.
    #[test]
    fn only_a_sudo_refusal_is_sent_to_a_terminal() {
        assert!(explain("sudo: a password is required").ends_with(SYNC_HINT));
        assert!(
            explain("sudo: no tty present and no askpass program specified").ends_with(SYNC_HINT)
        );
        assert_eq!(
            explain("tee: /etc/hosts: Read-only file system"),
            "tee: /etc/hosts: Read-only file system"
        );
        assert_eq!(explain(""), "no output");
    }

    /// The alias IP reaches `ifconfig` — and, on the macOS escalation path, a
    /// shell command string — from an HTTP body, so nothing that isn't a plain
    /// dotted quad may get through.
    #[test]
    fn only_a_dotted_quad_passes_for_an_ip() {
        assert!(is_ipv4_literal("127.0.0.1"));
        assert!(is_ipv4_literal("127.0.0.255"));
        assert!(!is_ipv4_literal("127.0.0.256"));
        assert!(!is_ipv4_literal("127.0.0"));
        assert!(!is_ipv4_literal("127.0.0.1.2"));
        assert!(!is_ipv4_literal("127.0.0.1 up; rm -rf /"));
        assert!(!is_ipv4_literal("::1"));
        assert!(!is_ipv4_literal(""));
        assert!(ensure_loopback_aliases(&[alias("evil", "$(id)")]).is_err());
    }

    /// Both quoting layers the macOS escalation goes through: the staged path
    /// is interpolated into a `/bin/sh` command, which is itself interpolated
    /// into an AppleScript string literal.
    #[test]
    fn a_path_survives_both_quoting_layers() {
        assert_eq!(shell_quote("/Users/o'brien/x"), "'/Users/o'\\''brien/x'");
        assert_eq!(shell_quote("plain"), "'plain'");
        assert_eq!(
            applescript_quote(r#"/bin/cp 'a"b\c'"#),
            r#""/bin/cp 'a\"b\\c'""#
        );
    }

    #[test]
    fn only_the_managed_block_is_stripped() {
        let file = format!(
            "127.0.0.1\tlocalhost\n{BEGIN}\n127.0.0.1\tapp\n{END}\n255.255.255.255\tbroadcasthost\n"
        );
        assert_eq!(
            strip_block(&file),
            "127.0.0.1\tlocalhost\n255.255.255.255\tbroadcasthost\n"
        );
    }
}
