//! Thin, typed wrappers around the `sbx` CLI.
//!
//! Every command here maps to a *confirmed* `sbx` 0.35 subcommand. We never call
//! `docker sandbox` — only the standalone `sbx` binary, as requested.
//!
//! Confirmed surface (docs.docker.com/reference/cli/sbx, v0.35):
//!   sbx create <agent> [PATH...] --name <name>
//!   sbx run    <agent> [PATH...] [--name <name>] [-- AGENT_ARGS...]   (no --env flag)
//!   sbx run    --name <name>   (re-attach; since 0.35 also works for sandboxes
//!                               created with a custom --kit, without re-passing it)
//!   sbx ls
//!   sbx inspect SANDBOX        (since 0.35: lists kits, injected secrets, info)
//!   sbx exec   [-it|-d] [-u user] SANDBOX -- cmd...
//!   sbx ports  SANDBOX [--publish [[HOST_IP:]HOST_PORT:]SANDBOX_PORT[/PROTO]]
//!   sbx policy allow|deny network [--sandbox NAME] RESOURCES  (comma list, *.dom, dom:443, **)
//!   sbx policy init <posture>                      (was `set-default`, kept as deprecated alias)
//!   sbx secret set [-g | SANDBOX] <service>        (service-keyed, via stdin)
//!
//! Behaviour changes in 0.35 this module accounts for:
//!   * `sbx kit add` RECREATES the sandbox container (state preserved) and
//!     composes the kit's network allow/deny rules into the live policy.
//!   * `sbx rm` refuses to delete a sandbox with an active session unless
//!     `--force` is passed (we always pass it — see `rm_sandboxes`).
//!   * Host env vars are no longer auto-injected at runtime; secrets must go
//!     through `sbx secret set` / `sbx secret import` (we already use `set`).
//!
//! Surface added from the newer release notes, NOT yet verified against a live
//! `sbx --help` — check these before depending on them:
//!   sbx create … -p/--publish SPEC        (publish at creation; see create_claude)
//!   sbx create … --no-share-skills        (opt out of the shared skill store)
//!   sbx skills import [--dry-run|--force] (import host agent skills into the store)
//!   sbx setup ssh                         (managed `Host *.sbx` block => `ssh <name>.sbx`)

use anyhow::{bail, Context, Result};
use std::io::Write;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const BIN: &str = "sbx";

/// Longest tail of a failing command's stderr we fold into the error message.
/// Generous on purpose: sbx's policy denials carry a multi-line explanation
/// (rule, origin, detail) and, since the 2026 releases, an organisation's
/// configured support message — truncating those defeats the point.
const STDERR_TAIL_LIMIT: usize = 2000;

/// Turn a failed `sbx` invocation into an error that carries *what sbx said*.
///
/// Without this, every failure collapsed into "`sbx …` exited with exit status: 1"
/// and the actual diagnosis was dropped on the floor — which mattered most for
/// the messages users can act on: `Blocked by network policy` blocks (rule /
/// origin / detail) and governance denials carrying an org support message
/// telling you who to contact. Those surface in the web UI, where nobody sees
/// the daemon's stderr.
fn command_error(args: &[&str], status: std::process::ExitStatus, stderr: &[u8]) -> anyhow::Error {
    let msg = String::from_utf8_lossy(stderr);
    let msg = msg.trim();
    if msg.is_empty() {
        return anyhow::anyhow!("`sbx {}` exited with {status}", args.join(" "));
    }
    // Keep the *tail*: sbx prints context first and the reason last.
    let tail = match msg.char_indices().nth_back(STDERR_TAIL_LIMIT) {
        Some((i, _)) => format!("…{}", &msg[i..]),
        None => msg.to_string(),
    };
    anyhow::anyhow!("`sbx {}` exited with {status}:\n{tail}", args.join(" "))
}

/// Run `sbx <args...>`, inheriting stdio (for interactive / long, chatty steps
/// such as `create` and `kit add`, where live progress output matters).
///
/// The child's stderr goes straight to this process's stderr, so a failure is
/// already visible to whoever is watching the terminal or the daemon log — it
/// just can't be folded into the returned error. Use `run_checked` instead for
/// short commands whose error is surfaced programmatically (web UI, `Result`).
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

/// Run a short, non-interactive `sbx <args...>` whose failure needs to be
/// *reportable*: stdout is inherited (these commands print little), stderr is
/// captured so it can be folded into the error via `command_error`.
///
/// `wait_with_output` drains the stderr pipe before reaping the child, so a
/// command that writes more than one pipe buffer's worth can't deadlock.
fn run_checked(args: &[&str]) -> Result<()> {
    tracing::debug!(target: "sbx", "sbx {}", args.join(" "));
    let out = Command::new(BIN)
        .args(args)
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("failed to spawn `{BIN}` — is it on your PATH?"))?
        .wait_with_output()?;
    if !out.status.success() {
        return Err(command_error(args, out.status, &out.stderr));
    }
    // sbx routes progress to stderr when stdout isn't a TTY; on success that's
    // noise for the caller but useful when debugging a pipeline step.
    let noise = String::from_utf8_lossy(&out.stderr);
    if !noise.trim().is_empty() {
        tracing::debug!(target: "sbx", "{}", noise.trim());
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
        return Err(command_error(args, out.status, &out.stderr));
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

/// Everything `create_claude` needs beyond the sandbox name.
#[derive(Clone, Copy)]
pub struct CreateOpts<'a> {
    /// Host path the agent edits *in place* (bidirectional sync).
    pub workspace: &'a str,
    /// Extra directories mounted read-only (a ":ro" suffix is added per the sbx spec).
    pub ro_mounts: &'a [String],
    /// Forwarded as `--kit`; applied before the agent starts, so env vars it
    /// sets are visible from the first process.
    pub kit_path: Option<&'a str>,
    /// Port specs (`[[HOST_IP:]HOST_PORT:]SANDBOX_PORT[/PROTO]`) forwarded as
    /// `-p`. See the note on `create_claude`.
    pub publish: &'a [String],
    /// When false, pass `--no-share-skills` to keep the host's shared skill
    /// store out of this sandbox.
    pub share_skills: bool,
}

/// `sbx create claude <workspace> --name <name> [--kit K] [-p SPEC…] [--no-share-skills]`.
///
/// Ports are published *at creation* rather than only by the provisioning
/// thread: that thread has to wait for the sandbox to report `running` before
/// `sbx ports --publish` will work, which leaves a window where the agent is
/// already serving but `neos.local:4200` refuses connections. `-p` closes it —
/// the mapping exists from first boot. The thread still re-publishes, because
/// mappings do not survive a stop/restart.
///
/// Note that `-p` makes creation **all-or-nothing**: sbx 409s the whole request
/// if any one host port is already bound. Callers that would rather have a
/// sandbox without its ports than no sandbox at all should go through
/// `crate::create_with_port_fallback`.
pub fn create_claude(name: &str, opts: &CreateOpts<'_>) -> Result<()> {
    let mut args: Vec<String> = vec![
        "create".into(),
        "claude".into(),
        opts.workspace.into(),
        "--name".into(),
        name.into(),
    ];
    for m in opts.ro_mounts {
        args.push(format!("{m}:ro"));
    }
    if let Some(kit) = opts.kit_path {
        args.push("--kit".into());
        args.push(kit.into());
    }
    for spec in opts.publish {
        args.push("-p".into());
        args.push(spec.clone());
    }
    if !opts.share_skills {
        args.push("--no-share-skills".into());
    }
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    run_inherit(&refs)
}

/// `sbx skills import [--dry-run] [--force]` — discover skills from supported
/// host agents and copy them into the persistent store that sandboxes mount.
///
/// A pure passthrough: the store is sbx-managed and shared across sandboxes, so
/// sbxw has nothing of its own to add beyond saving you the context switch.
/// Imported skills outlive the sandboxes that used them.
pub fn skills_import(dry_run: bool, force: bool) -> Result<()> {
    let mut args = vec!["skills", "import"];
    if dry_run {
        args.push("--dry-run");
    }
    if force {
        args.push("--force");
    }
    run_inherit(&args)
}

/// `sbx setup ssh` — install the managed `Host *.sbx` block in the user's SSH
/// config, making every existing sandbox reachable as `<name>.sbx`.
pub fn setup_ssh() -> Result<()> {
    run_checked(&["setup", "ssh"])
}

/// `sbx kit add SANDBOX REFERENCE` — apply a kit to an existing sandbox.
///
/// Since sbx 0.35 this RECREATES the sandbox container with the augmented kit
/// set (state is preserved) and composes the kit's network allow/deny rules
/// into the sandbox policy. Recreation kills anything attached to a running
/// sandbox (agent/bash PTY sessions), so callers should prefer `sbx exec`
/// paths on running sandboxes and reserve `kit add` for stopped ones or for
/// changes that genuinely need the kit machinery.
pub fn kit_add(sandbox: &str, kit_path: &str) -> Result<()> {
    run_inherit(&["kit", "add", sandbox, kit_path])
}

/// `sbx inspect SANDBOX` — raw text output. Since sbx 0.35 this lists the
/// sandbox's kits, injected secrets, and general sandbox information; sbxw
/// uses it to skip re-applying kits that are already present (each re-apply
/// would recreate the container). Best-effort: callers must tolerate errors
/// and unknown formats (older sbx versions may not list kits at all).
pub fn inspect_raw(name: &str) -> Result<String> {
    run_capture(&["inspect", name])
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
    run_checked(&["stop", name])
}

/// `sbx rm --force [--all | SANDBOX...]` — remove sandboxes permanently.
///
/// `--force` is load-bearing since sbx 0.35: `sbx rm` now refuses to delete a
/// sandbox with an active session without it, and sbxw's own web daemon keeps
/// an `sbx run --name` session attached — a non-forced rm would always fail
/// from the web UI. Removal therefore proceeds even mid-session, by design.
pub fn rm_sandboxes(names: &[&str], all: bool) -> Result<()> {
    let mut args = vec!["rm", "--force"];
    if all {
        args.push("--all");
    } else {
        args.extend_from_slice(names);
    }
    run_checked(&args)
}

/// `sbx ports <name> --publish <spec>` where spec = [[HOST_IP:]HOST_PORT:]SANDBOX_PORT[/PROTO].
/// sbx restores published ports on restart; we still re-publish as a belt-and-suspenders
/// guard against conflict recovery choosing a different host port.
pub fn publish_port(name: &str, spec: &str) -> Result<()> {
    run_checked(&["ports", name, "--publish", spec])
}

/// `sbx ports <name> --unpublish <spec>` — remove a published port mapping.
pub fn unpublish_port(name: &str, spec: &str) -> Result<()> {
    run_checked(&["ports", name, "--unpublish", spec])
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
///
/// Uses `run_checked` so a refusal reaches the caller intact: a governance
/// denial can carry an organisation-configured support message (who to contact)
/// that is useless if it only ever lands in the daemon log.
pub fn policy_allow_network(sandbox: &str, resources: &str) -> Result<()> {
    run_checked(&[
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
    run_checked(&["policy", "deny", "network", "--sandbox", sandbox, resources])
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

/// Refresh Claude Code's OAuth credentials in a *running* sandbox by writing
/// `~/.claude/.credentials.json` directly over `sbx exec`. This replaces the
/// old `sbx kit add` path for running sandboxes: since sbx 0.35 `kit add`
/// recreates the sandbox container, which would kill live agent/bash sessions
/// attached through the web terminal.
pub fn write_oauth_credentials(sandbox: &str, credentials_json: &str) -> Result<()> {
    const DEST: &str = "/home/agent/.claude/.credentials.json";
    write_file_stdin(sandbox, DEST, credentials_json.as_bytes())?;
    exec_run(sandbox, &["chmod", "600", DEST])
}

/// Run a non-interactive command inside `sandbox` via `sbx exec`, inheriting stdio.
fn exec_run(sandbox: &str, args: &[&str]) -> Result<()> {
    let mut full: Vec<&str> = vec!["exec", sandbox, "--"];
    full.extend_from_slice(args);
    run_checked(&full)
}

/// Path (inside the sandbox) of Claude Code's user-level settings file — the one
/// every helper below merges into.
const SETTINGS_PATH: &str = "/home/agent/.claude/settings.json";

/// Run a one-shot Node script inside `sandbox`: write it to `tmp_path`, run it,
/// then remove it. The removal is best-effort and never masks the script's own
/// outcome, which is what the caller cares about.
fn run_node_script(sandbox: &str, tmp_path: &str, script: &str) -> Result<()> {
    write_file_stdin(sandbox, tmp_path, script.as_bytes())?;
    let result = exec_run(sandbox, &["node", tmp_path]);
    let _ = exec_run(sandbox, &["rm", "-f", tmp_path]);
    result
}

/// Wrap `body` — JavaScript mutating the parsed settings object `d` — in the
/// read-modify-write boilerplate every settings helper needs.
///
/// Merging rather than overwriting is load-bearing: that single file also holds
/// the model, hook and statusLine settings installed by the *other* helpers here,
/// plus whatever the user configured in-session. A missing or corrupt file parses
/// as `{}`, so a first run and a repeat run behave identically.
fn settings_merge_script(body: &str) -> String {
    format!(
        "const fs=require('fs');const p='{SETTINGS_PATH}';\
         let d={{}};try{{d=JSON.parse(fs.readFileSync(p,'utf8'))}}catch(e){{}}\
         {body}\
         fs.writeFileSync(p,JSON.stringify(d));"
    )
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
    // Not `settings_merge_script`: trust lives in `.claude.json`, a different
    // file from the `settings.json` every other helper here merges into.
    let script = format!(
        "const fs=require('fs');const p='/home/agent/.claude.json';const w={};\
         let d={{}};try{{d=JSON.parse(fs.readFileSync(p,'utf8'))}}catch(e){{}}\
         d.projects=d.projects||{{}};\
         d.projects[w]=Object.assign({{}},d.projects[w],{{hasTrustDialogAccepted:true}});\
         fs.writeFileSync(p,JSON.stringify(d));",
        serde_json::to_string(workspace)?
    );
    run_node_script(sandbox, "/tmp/.sbxw-trust.js", &script)
}

/// Set the default model in the sandbox's user-level Claude Code settings
/// (`/home/agent/.claude/settings.json`). Merges into whatever `settings.json`
/// already exists rather than overwriting it, since that file also holds the
/// permission/hook settings installed elsewhere. Safe to call repeatedly
/// (e.g. on every `sbxw up`); sbxw.toml's `claude_model` is the source of
/// truth for the default.
pub fn set_default_model(sandbox: &str, model: &str) -> Result<()> {
    let script = settings_merge_script(&format!("d.model={};", serde_json::to_string(model)?));
    run_node_script(sandbox, "/tmp/.sbxw-model.js", &script)
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

    let merge_script = settings_merge_script(&format!(
        "const hookPath={};\
         d.hooks=d.hooks||{{}};\
         d.hooks.PreToolUse=(d.hooks.PreToolUse||[]).filter(e=>\
           !(e.hooks||[]).some(h=>(h.args||[]).includes(hookPath)));\
         d.hooks.PreToolUse.push({{matcher:'Write',hooks:[{{type:'command',command:'node',args:[hookPath]}}]}});",
        serde_json::to_string(ARTIFACT_HOOK_PATH)?
    ));
    run_node_script(sandbox, "/tmp/.sbxw-hook-install.js", &merge_script)
}

/// Path (inside the sandbox) the status-forwarding hook script is installed at.
const STATUS_HOOK_PATH: &str = "/home/agent/.sbxw/status-hook.js";

/// Claude Code hook events forwarded to the daemon for trusted session state.
// The Claude Code lifecycle events the island derives session state from.
// (There is no "PermissionRequest" event — permission prompts surface through
// `Notification`, and structured questions through a `PreToolUse` for the
// `AskUserQuestion` tool.)
const STATUS_HOOK_EVENTS: &[&str] = &[
    "SessionStart",
    "SessionEnd",
    "UserPromptSubmit",
    "PreToolUse",
    "PostToolUse",
    "Notification",
    "Stop",
];

/// Install a user-level hook that POSTs every relevant Claude Code lifecycle
/// event to the sbxw daemon (`http://host.docker.internal:<port>/api/hook`), so
/// the island can derive session state from *trusted, structured* events rather
/// than scraping the terminal. `web_port` is the daemon's port.
///
/// Fire-and-forget: the script never blocks Claude Code or influences a tool
/// decision (see `assets/status-hook.js`). Merges into `settings.json` and
/// removes any prior copy of this hook first, like `install_artifact_hook`.
pub fn install_status_hooks(sandbox: &str, web_port: &str) -> Result<()> {
    const HOOK_SCRIPT: &str = include_str!("../assets/status-hook.js");
    let script = HOOK_SCRIPT.replace("__PORT__", web_port);
    write_file_stdin(sandbox, STATUS_HOOK_PATH, script.as_bytes())?;

    let merge_script = settings_merge_script(&format!(
        "const hookPath={path};const events={events};\
         d.hooks=d.hooks||{{}};\
         for(const ev of events){{\
           d.hooks[ev]=(d.hooks[ev]||[]).filter(e=>\
             !(e.hooks||[]).some(h=>(h.args||[]).includes(hookPath)));\
           d.hooks[ev].push({{hooks:[{{type:'command',command:'node',args:[hookPath]}}]}});\
         }}",
        path = serde_json::to_string(STATUS_HOOK_PATH)?,
        events = serde_json::to_string(STATUS_HOOK_EVENTS)?,
    ));
    run_node_script(sandbox, "/tmp/.sbxw-status-hook-install.js", &merge_script)
}

/// Path (inside the sandbox) the usage statusLine script is installed at.
const USAGE_STATUSLINE_PATH: &str = "/home/agent/.sbxw/usage-statusline.js";

/// Install a Claude Code `statusLine` command that prints a compact status line
/// and (throttled) forwards the subscription rate limits to the daemon
/// (`http://host.docker.internal:<port>/api/usage`). Claude Code fetches those
/// numbers itself per the statusLine contract, so no OAuth token is reused
/// out-of-band. Only Pro/Max sessions carry `rate_limits`; others degrade to
/// model/cost only. A user-configured `statusLine` is preserved (we only set
/// ours if none exists, or if the existing one is already ours).
pub fn install_usage_statusline(sandbox: &str, web_port: &str) -> Result<()> {
    const SCRIPT: &str = include_str!("../assets/usage-statusline.js");
    let script = SCRIPT.replace("__PORT__", web_port);
    write_file_stdin(sandbox, USAGE_STATUSLINE_PATH, script.as_bytes())?;

    let command = format!("node {USAGE_STATUSLINE_PATH}");
    let merge_script = settings_merge_script(&format!(
        "const cmd={cmd};\
         if(!d.statusLine||((d.statusLine.command||'').includes('usage-statusline.js'))){{\
           d.statusLine={{type:'command',command:cmd}};\
         }}",
        cmd = serde_json::to_string(&command)?,
    ));
    run_node_script(sandbox, "/tmp/.sbxw-usage-install.js", &merge_script)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Any exit status will do; we only ever format it.
    fn failed_status() -> std::process::ExitStatus {
        std::process::Command::new("sh")
            .args(["-c", "exit 1"])
            .status()
            .expect("sh should be runnable")
    }

    #[test]
    fn command_error_keeps_the_reason_sbx_printed() {
        let stderr = b"Blocked by network policy: domain example.com\n  \
                       origin: corporate policy\n  Ask platform@acme.example.";
        let err = command_error(&["policy", "allow", "network"], failed_status(), stderr);
        let msg = err.to_string();
        assert!(msg.contains("sbx policy allow network"), "{msg}");
        assert!(msg.contains("origin: corporate policy"), "{msg}");
        assert!(msg.contains("Ask platform@acme.example"), "{msg}");
    }

    #[test]
    fn command_error_falls_back_to_the_exit_status_when_sbx_said_nothing() {
        let err = command_error(&["stop", "neos"], failed_status(), b"   \n  ");
        assert_eq!(
            err.to_string(),
            "`sbx stop neos` exited with exit status: 1"
        );
    }

    /// The wrapper must *merge*: read the existing settings, apply the body, and
    /// write back. Overwriting instead would make each helper clobber the ones
    /// that ran before it (model vs. hooks vs. statusLine, all in one file).
    #[test]
    fn settings_merge_script_reads_modifies_then_writes() {
        let script = settings_merge_script("d.model=\"claude-sonnet-5\";");
        let read_at = script.find("readFileSync").expect("reads the current file");
        let body_at = script.find("d.model=").expect("applies the body");
        let write_at = script.find("writeFileSync").expect("writes it back");
        assert!(read_at < body_at && body_at < write_at, "{script}");
        // A missing or corrupt settings.json must parse as `{}`, not throw.
        assert!(script.contains("catch(e){}"), "{script}");
        assert!(script.contains(SETTINGS_PATH), "{script}");
    }

    #[test]
    fn command_error_truncates_from_the_front_keeping_the_tail() {
        // sbx prints context first and the actionable reason last, so an
        // over-long stderr must lose its head, not its tail.
        let mut stderr = "noise\n".repeat(2000);
        stderr.push_str("THE ACTUAL REASON");
        let msg = command_error(&["ls"], failed_status(), stderr.as_bytes()).to_string();
        assert!(msg.ends_with("THE ACTUAL REASON"), "{msg}");
        assert!(msg.contains('…'), "expected an ellipsis marker: {msg}");
        // The kept slice is bounded by the limit, not by how much sbx printed.
        let kept = msg.split_once('…').expect("ellipsis").1;
        assert_eq!(kept.chars().count(), STDERR_TAIL_LIMIT + 1);
    }
}
