//! Thin, typed wrappers around the `sbx` CLI.
//!
//! Every command here maps to a *confirmed* `sbx` 0.30 subcommand. We never call
//! `docker sandbox` — only the standalone `sbx` binary, as requested.
//!
//! Confirmed surface (docs.docker.com/reference/cli/sbx, v0.30):
//!   sbx create <agent> [PATH...] --name <name>
//!   sbx run    <agent> [PATH...] [--name <name>] [-- AGENT_ARGS...]   (no --env flag)
//!   sbx run    --name <name>                     (re-attach to existing sandbox)
//!   sbx ls
//!   sbx exec   [-it|-d] [-u user] SANDBOX -- cmd...
//!   sbx ports  SANDBOX [--publish [[HOST_IP:]HOST_PORT:]SANDBOX_PORT[/PROTO]]
//!   sbx policy allow|deny network [--sandbox NAME] RESOURCES  (comma list, *.dom, dom:443, **)
//!   sbx policy init <posture>                      (was `set-default`, kept as deprecated alias)
//!   sbx secret set [-g | SANDBOX] <service>        (service-keyed, via stdin)

use anyhow::{bail, Context, Result};
use std::io::Write;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const BIN: &str = "sbx";

/// Run `sbx <args...>`, inheriting stdio (for interactive-ish steps / logging).
fn run_inherit(args: &[&str]) -> Result<()> {
    tracing::debug!(target: "sbx", "sbx {}", args.join(" "));
    let status = Command::new(BIN)
        .args(args)
        .status()
        .with_context(|| format!("failed to spawn `{BIN}` — is it on your PATH?"))?;
    if !status.success() {
        bail!("`sbx {}` exited with {}", args.join(" "), status);
    }
    Ok(())
}

/// Run `sbx <args...>` and capture its output as a String.
/// Some sbx commands (e.g. `sbx ls`) write to stderr instead of stdout when
/// stdout is not a TTY, so we capture both and fall back to stderr when stdout
/// is empty.
fn run_capture(args: &[&str]) -> Result<String> {
    tracing::debug!(target: "sbx", "sbx {} (capture)", args.join(" "));
    let out = Command::new(BIN)
        .args(args)
        .output()
        .with_context(|| format!("failed to spawn `{BIN}`"))?;
    if !out.status.success() {
        bail!("`sbx {}` exited with {}", args.join(" "), out.status);
    }
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    if !stdout.is_empty() {
        return Ok(stdout);
    }
    Ok(String::from_utf8_lossy(&out.stderr).into_owned())
}

/// Is `sbx` reachable at all?
pub fn assert_available() -> Result<()> {
    run_capture(&["version"]).context(
        "`sbx version` failed — install the standalone sbx binary and ensure it is on PATH",
    )?;
    Ok(())
}

/// Return true if a sandbox with this exact name already exists (any state).
pub fn exists(name: &str) -> Result<bool> {
    // `sbx ls` prints a table whose first column is the sandbox name.
    let table = run_capture(&["ls"]).unwrap_or_default();
    Ok(table
        .lines()
        .skip(1) // header
        .filter_map(|l| l.split_whitespace().next())
        .any(|n| n == name))
}

/// Return true if the sandbox is currently running.
pub fn is_running(name: &str) -> Result<bool> {
    let table = run_capture(&["ls"]).unwrap_or_default();
    for line in table.lines().skip(1) {
        let cols: Vec<&str> = line.split_whitespace().collect();
        // Layout observed in docs: NAME AGENT STATUS PORTS WORKSPACE
        if cols.first() == Some(&name) {
            return Ok(cols
                .get(2)
                .map(|s| s.eq_ignore_ascii_case("running"))
                .unwrap_or(false));
        }
    }
    Ok(false)
}

/// Poll `sbx ls` until `name` reports running, or `timeout` elapses.
/// Returns whether it came up in time.
pub fn wait_until_running(name: &str, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if is_running(name).unwrap_or(false) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    false
}

/// `sbx create claude <workspace> --name <name> [--kit <kit_path>]`.
/// `workspace` is the host path the agent edits *in place* (bidirectional sync).
/// Extra read-only mounts can be appended with a ":ro" suffix per the sbx spec.
/// If `kit_path` is given it is forwarded as `--kit`; the kit is applied before
/// the agent starts, so env vars it sets are visible from the first process.
pub fn create_claude(
    name: &str,
    workspace: &str,
    ro_mounts: &[String],
    kit_path: Option<&str>,
) -> Result<()> {
    let mut args: Vec<String> = vec![
        "create".into(),
        "claude".into(),
        workspace.into(),
        "--name".into(),
        name.into(),
    ];
    for m in ro_mounts {
        args.push(format!("{m}:ro"));
    }
    if let Some(kit) = kit_path {
        args.push("--kit".into());
        args.push(kit.into());
    }
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    run_inherit(&refs)
}

/// `sbx kit add SANDBOX REFERENCE` — apply a kit to an already-running sandbox.
pub fn kit_add(sandbox: &str, kit_path: &str) -> Result<()> {
    run_inherit(&["kit", "add", sandbox, kit_path])
}

/// Parsed row from `sbx ls`.
pub struct SandboxInfo {
    pub name: String,
    pub agent: String,
    pub status: String,
}

/// Parse `sbx ls` into sandbox info. Returns an empty list on error.
pub fn list_sandboxes() -> Vec<SandboxInfo> {
    let table = run_capture(&["ls"]).unwrap_or_default();
    table
        .lines()
        .skip(1) // header row
        .filter_map(|line| {
            let mut cols = line.split_whitespace();
            let name = cols.next()?.to_string();
            let agent = cols.next().unwrap_or("").to_string();
            let status = cols.next().unwrap_or("unknown").to_string();
            Some(SandboxInfo {
                name,
                agent,
                status,
            })
        })
        .collect()
}

/// `sbx stop SANDBOX` — stop without removing.
pub fn stop_sandbox(name: &str) -> Result<()> {
    run_inherit(&["stop", name])
}

/// `sbx rm --force [--all | SANDBOX...]` — remove sandboxes permanently.
pub fn rm_sandboxes(names: &[&str], all: bool) -> Result<()> {
    let mut args = vec!["rm", "--force"];
    if all {
        args.push("--all");
    } else {
        args.extend_from_slice(names);
    }
    run_inherit(&args)
}

/// `sbx ports <name> --publish <spec>` where spec = [[HOST_IP:]HOST_PORT:]SANDBOX_PORT[/PROTO].
/// sbx restores published ports on restart; we still re-publish as a belt-and-suspenders
/// guard against conflict recovery choosing a different host port.
pub fn publish_port(name: &str, spec: &str) -> Result<()> {
    run_inherit(&["ports", name, "--publish", spec])
}

/// `sbx ports <name> --unpublish <spec>` — remove a published port mapping.
pub fn unpublish_port(name: &str, spec: &str) -> Result<()> {
    run_inherit(&["ports", name, "--unpublish", spec])
}

/// `sbx ports <name>` — list currently published ports (raw text).
pub fn list_ports(name: &str) -> Result<String> {
    run_capture(&["ports", name])
}

/// A single parsed port mapping from `sbx ports <name>`.
#[derive(Clone)]
pub struct PortMapping {
    pub sandbox_port: u16,
    pub proto: String,
    pub host_ip: String,
    pub host_port: u16,
}

impl PortMapping {
    /// Reconstruct the unpublish spec: `host_ip:host_port:sandbox_port`.
    /// The host IP is included so unpublish targets this exact binding and
    /// doesn't nuke a different alias (e.g. 127.0.0.2) on the same port pair.
    pub fn spec(&self) -> String {
        format!("{}:{}:{}", self.host_ip, self.host_port, self.sandbox_port)
    }
}

/// Parse the output of `sbx ports <name>` into structured mappings.
///
/// Confirmed sbx format (4 whitespace-separated columns, 1 header row):
///   HOST IP     HOST PORT   SANDBOX PORT   PROTOCOL
///   127.0.0.1   3000        3000           tcp
///   ::1         3000        3000           tcp
///
/// Fallback: Docker arrow style "3000/tcp -> 0.0.0.0:3000".
///
/// IPv4/IPv6 duplicates for the same (sandbox_port, host_port) are collapsed;
/// the IPv4 binding is kept since it's what we publish via sbxw.
pub fn list_ports_parsed(name: &str) -> Vec<PortMapping> {
    let raw = list_ports(name).unwrap_or_default();
    let mut lines = raw.lines().peekable();

    // Consume blank lines and detect format from the first non-empty line.
    let header = loop {
        match lines.next() {
            None => return vec![],
            Some(l) if l.trim().is_empty() => continue,
            Some(l) => break l.trim(),
        }
    };

    let mut out: Vec<PortMapping> = Vec::new();

    if header.contains("HOST IP") && header.contains("SANDBOX PORT") {
        // sbx columnar format: HOST IP  HOST PORT  SANDBOX PORT  PROTOCOL
        for line in lines {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let c: Vec<&str> = line.split_whitespace().collect();
            if c.len() < 3 {
                continue;
            }
            let host_ip = c[0].to_string();
            let host_port: u16 = match c[1].parse() {
                Ok(p) => p,
                _ => continue,
            };
            let sandbox_port: u16 = match c[2].parse() {
                Ok(p) => p,
                _ => continue,
            };
            let proto = c.get(3).unwrap_or(&"tcp").to_string();
            out.push(PortMapping {
                sandbox_port,
                proto,
                host_ip,
                host_port,
            });
        }
    } else {
        // Fallback: Docker arrow "3000/tcp -> 0.0.0.0:3000" or bare table.
        // Re-include the header line in case it's a data line in this format.
        for line in std::iter::once(header).chain(lines) {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            // Skip all-uppercase header rows.
            if line.split_whitespace().all(|w| w == w.to_uppercase()) {
                continue;
            }

            let (left, right) = if let Some((l, r)) = line.split_once("->") {
                (l.trim(), r.trim())
            } else {
                let mut parts = line.splitn(2, |c: char| c.is_whitespace());
                match (parts.next(), parts.next()) {
                    (Some(l), Some(r)) => (l.trim(), r.trim()),
                    _ => continue,
                }
            };

            let (port_str, proto) = left
                .split_once('/')
                .map(|(p, pr)| (p, pr.to_string()))
                .unwrap_or((left, "tcp".to_string()));
            let sandbox_port: u16 = match port_str.parse() {
                Ok(p) => p,
                _ => continue,
            };

            let (host_ip, host_port) = if let Some((ip, p)) = right.rsplit_once(':') {
                match p.parse::<u16>() {
                    Ok(hp) => (ip.to_string(), hp),
                    _ => continue,
                }
            } else {
                match right.parse::<u16>() {
                    Ok(hp) => ("0.0.0.0".to_string(), hp),
                    _ => continue,
                }
            };

            out.push(PortMapping {
                sandbox_port,
                proto,
                host_ip,
                host_port,
            });
        }
    }

    // Drop the IPv6 mirror entries (sbx auto-adds an ::1 binding for each IPv4
    // publish) but keep EVERY distinct IPv4 binding — including extra loopback
    // aliases like 127.0.0.2 created by sbxw's ip_per_app mode. Only exact
    // duplicates (same ip+ports+proto) are collapsed.
    let mut seen: std::collections::HashSet<(String, u16, u16, String)> =
        std::collections::HashSet::new();
    out.retain(|p| {
        if p.host_ip.contains(':') {
            return false; // IPv6 mirror — hidden (sbxw publishes on IPv4)
        }
        seen.insert((
            p.host_ip.clone(),
            p.host_port,
            p.sandbox_port,
            p.proto.clone(),
        ))
    });

    out
}

/// `sbx policy allow network --sandbox <sandbox> <resources>` (sandbox-scoped).
pub fn policy_allow_network(sandbox: &str, resources: &str) -> Result<()> {
    run_inherit(&[
        "policy",
        "allow",
        "network",
        "--sandbox",
        sandbox,
        resources,
    ])
}

/// `sbx policy deny network --sandbox <sandbox> <resources>` (sandbox-scoped).
pub fn policy_deny_network(sandbox: &str, resources: &str) -> Result<()> {
    run_inherit(&["policy", "deny", "network", "--sandbox", sandbox, resources])
}

/// Store a service-scoped secret by piping the value on stdin (keeps it out of
/// argv / shell history). `service` must be one of sbx's known services
/// (anthropic, openai, github, ...). For a global secret pass `global = true`.
pub fn secret_set_stdin(
    service: &str,
    value: &str,
    global: bool,
    sandbox: Option<&str>,
) -> Result<()> {
    let mut args: Vec<String> = vec!["secret".into(), "set".into()];
    if global {
        args.push("-g".into());
    } else if let Some(s) = sandbox {
        args.push(s.into());
    }
    args.push(service.into());

    tracing::debug!(target: "sbx", "sbx {} (secret via stdin)", args.join(" "));
    let mut child = Command::new(BIN)
        .args(&args)
        .stdin(Stdio::piped())
        .spawn()
        .with_context(|| format!("failed to spawn `{BIN} secret set`"))?;
    child
        .stdin
        .take()
        .context("no stdin handle for sbx secret set")?
        .write_all(value.as_bytes())?;
    let status = child.wait()?;
    if !status.success() {
        bail!("`sbx secret set {service}` failed: {status}");
    }
    Ok(())
}

/// Write `data` to `dest` inside the sandbox by piping it over stdin to
/// `sbx exec`. Used by the web UI to drop a pasted image into the sandbox
/// filesystem so the agent can read it.
///
/// `-i` (no `-t`) is deliberate: a PTY would perform newline translation and
/// corrupt binary data. The destination is passed as a positional argument to
/// `sh -c` (`$1`) so an arbitrary path can't break out into shell syntax, and
/// the parent directory is created on demand.
pub fn write_file_stdin(sandbox: &str, dest: &str, data: &[u8]) -> Result<()> {
    let script = r#"mkdir -p "$(dirname "$1")" && cat > "$1""#;
    let mut child = Command::new(BIN)
        .args(["exec", "-i", sandbox, "--", "sh", "-c", script, "sh", dest])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("failed to spawn `{BIN} exec` to write {dest}"))?;
    child
        .stdin
        .take()
        .context("no stdin handle for sbx exec")?
        .write_all(data)?;
    let out = child.wait_with_output()?;
    if !out.status.success() {
        bail!(
            "`sbx exec` write to {dest} failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(())
}

/// Run a non-interactive command inside `sandbox` via `sbx exec`, inheriting stdio.
fn exec_run(sandbox: &str, args: &[&str]) -> Result<()> {
    let mut full: Vec<&str> = vec!["exec", sandbox, "--"];
    full.extend_from_slice(args);
    run_inherit(&full)
}

/// Pre-seed `/home/agent/.claude.json` so Claude Code considers `workspace`
/// already trusted the first time it starts in this sandbox. Without this,
/// a fresh sandbox shows the "workspace has not been trusted" banner and
/// ignores every `permissions.allow` entry from `.claude/settings.local.json`
/// until someone accepts the trust dialog interactively.
///
/// This merges into whatever `.claude.json` already exists (via a small
/// Node script run inside the sandbox) rather than overwriting it, since
/// Claude Code also keeps onboarding/account state in that same file.
/// Safe to call repeatedly (e.g. on every `sbxw up`).
pub fn trust_workspace(sandbox: &str, workspace: &str) -> Result<()> {
    let script = format!(
        "const fs=require('fs');const p='/home/agent/.claude.json';const w={};\
         let d={{}};try{{d=JSON.parse(fs.readFileSync(p,'utf8'))}}catch(e){{}}\
         d.projects=d.projects||{{}};\
         d.projects[w]=Object.assign({{}},d.projects[w],{{hasTrustDialogAccepted:true}});\
         fs.writeFileSync(p,JSON.stringify(d));",
        serde_json::to_string(workspace)?
    );
    write_file_stdin(sandbox, "/tmp/.sbxw-trust.js", script.as_bytes())?;
    let result = exec_run(sandbox, &["node", "/tmp/.sbxw-trust.js"]);
    let _ = exec_run(sandbox, &["rm", "-f", "/tmp/.sbxw-trust.js"]);
    result
}

/// Path (inside the sandbox) the enforcement hook script is installed at.
const ARTIFACT_HOOK_PATH: &str = "/home/agent/.sbxw/enforce-artifacts.js";

/// Install a user-level (`/home/agent/.claude/settings.json`) `PreToolUse`
/// hook that blocks Claude from *creating* new non-code deliverables (docs,
/// wireframes... by extension) anywhere outside the `.sbxw-artifacts/`
/// convention folder. Editing a file that already exists is never blocked —
/// only brand-new files with a matching extension trip it, and Claude gets a
/// `permissionDecisionReason` back telling it where to retry. See
/// `assets/enforce-artifacts.js` for the actual matching logic.
///
/// Installed at the user level (not the project's own `.claude/settings.json`)
/// so it applies automatically to every sbxw sandbox without touching the
/// user's own repo config. Merges into whatever `settings.json` already
/// exists — removing any previous copy of this exact hook first — rather
/// than overwriting it, since that file can also hold model/permission
/// settings the user configured in-session. Safe to call repeatedly.
pub fn install_artifact_hook(sandbox: &str) -> Result<()> {
    const HOOK_SCRIPT: &str = include_str!("../assets/enforce-artifacts.js");
    write_file_stdin(sandbox, ARTIFACT_HOOK_PATH, HOOK_SCRIPT.as_bytes())?;

    let merge_script = format!(
        "const fs=require('fs');const p='/home/agent/.claude/settings.json';const hookPath={};\
         let d={{}};try{{d=JSON.parse(fs.readFileSync(p,'utf8'))}}catch(e){{}}\
         d.hooks=d.hooks||{{}};\
         d.hooks.PreToolUse=(d.hooks.PreToolUse||[]).filter(e=>\
           !(e.hooks||[]).some(h=>(h.args||[]).includes(hookPath)));\
         d.hooks.PreToolUse.push({{matcher:'Write',hooks:[{{type:'command',command:'node',args:[hookPath]}}]}});\
         fs.writeFileSync(p,JSON.stringify(d));",
        serde_json::to_string(ARTIFACT_HOOK_PATH)?
    );
    write_file_stdin(
        sandbox,
        "/tmp/.sbxw-hook-install.js",
        merge_script.as_bytes(),
    )?;
    let result = exec_run(sandbox, &["node", "/tmp/.sbxw-hook-install.js"]);
    let _ = exec_run(sandbox, &["rm", "-f", "/tmp/.sbxw-hook-install.js"]);
    result
}
