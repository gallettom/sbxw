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
//!   sbx policy ls [SANDBOX] [--wide] [--json] [--type|--source|--decision …]
//!                                                  (SANDBOX is POSITIONAL here,
//!                                                  unlike allow/deny's --sandbox;
//!                                                  --wide is the rule-level view)
//!   sbx policy log [SANDBOX]                       (recent allow/deny decisions)
//!   sbx policy inspect <policy-or-rule>            (full detail on one entry)
//!   sbx policy init <posture>                      (was `set-default`, kept as deprecated alias)
//!   sbx secret set [-g | SANDBOX] <service>        (service-keyed, via stdin;
//!                                                  see `secret_set_stdin` — 0.38
//!                                                  replaced both scope forms)
//!
//! Behaviour changes in 0.35 this module accounts for:
//!   * `sbx kit add` RECREATES the sandbox container (state preserved) and
//!     composes the kit's network allow/deny rules into the live policy.
//!   * `sbx rm` refuses to delete a sandbox with an active session unless
//!     `--force` is passed (we always pass it — see `rm_sandboxes`).
//!   * Host env vars are no longer auto-injected at runtime; secrets must go
//!     through `sbx secret set` / `sbx secret import` (we already use `set`).
//!
//! Surface added after 0.35, which is why `MIN_SBX_VERSION` is 0.37 and not the
//! 0.35 the core pipeline strictly needs. Individually still unverified against
//! a live `sbx --help` — but each one *degrades* rather than failing loudly, so
//! the version floor is what keeps their absence from going unnoticed:
//!   sbx create … -p/--publish SPEC        (publish at creation; see create_claude)
//!   sbx create … --no-share-skills        (opt out of the shared skill store)
//!   sbx skills import [--dry-run|--force] (import host agent skills into the store)
//!   sbx setup ssh                         (managed `Host *.sbx` block => `ssh <name>.sbx`)
//!
//! Changes in 0.38 this module accounts for, each behind a version gate rather
//! than a floor bump, so an 0.37 host keeps working (see `version_at_least`):
//!   * `secret set` scopes global by *default*; sandbox scope moved to
//!     `--sandbox NAME`, and the old positional / `-g` forms now warn.
//!   * kit spec **v2** (`schemaVersion: "2"`) — v1 still loads, via a legacy
//!     path. sbxw writes its own kits in whichever grammar the host understands.
//!   * `policy allow|deny network` refuses with "managed by your organization"
//!     when org governance owns the rule; that is a governed host, not a broken
//!     one, and bring-up treats it as such.
//!   * `inspect` also reports the sandbox's custom secrets, which widens what a
//!     bare substring search for a kit name can collide with.

use anyhow::{bail, Context, Result};
use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::OnceLock;
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

/// Oldest `sbx` sbxw is known to work against.
///
/// Two tiers of requirement sit behind this one number, and the higher one
/// wins, because the check exists to prevent surprises and not merely crashes:
///
/// * **0.35, or sbxw misprovisions.** `kit add` recreating the container (why
///   sbxw skips kits `inspect` already lists), `rm` refusing an attached session
///   without `--force` (which sbxw always passes), `run --name` re-attaching a
///   sandbox created with a custom kit. Older than that, these don't fail
///   cleanly: a kit is silently re-applied on every `up`, an `rm` refuses with
///   no explanation.
/// * **0.37, or half of sbxw quietly isn't there.** `create -p` (ports live from
///   first boot), `create --no-share-skills`, `skills import`, `setup ssh`, and
///   the policy panel's `policy ls [SANDBOX] --wide` / `policy log`. Every one of
///   those either degrades or is opt-in — `create_with_port_fallback` even
///   retries bare when `-p` is rejected — which is exactly the problem. Between
///   0.35 and 0.37 you get a working sandbox and a tool missing features it says
///   it has, explained only by scattered warnings.
pub const MIN_SBX_VERSION: (u32, u32, u32) = (0, 37, 0);

/// There is deliberately **no** upper bound. Newer sbx releases have so far
/// added surface rather than moved it, and the parts that did move (the policy
/// panel's `ls --wide` / `log`) already degrade view by view. Refusing to run
/// against a version merely newer than this file would be the "déconvenue" the
/// check exists to prevent.
const SKIP_VERSION_CHECK_ENV: &str = "SBXW_SKIP_SBX_VERSION_CHECK";

/// The sbx release sbxw is *current* with, as opposed to the oldest it accepts.
///
/// 0.38 is where `secret set` moved its scope onto `--sandbox`, kits grew spec
/// v2, and `policy allow` learned to say an organisation owns a rule. None of
/// that is required — every one of them is gated below and falls back to the
/// 0.37 shape — so this is documentation with a value, not a second floor.
pub const CURRENT_SBX_VERSION: (u32, u32, u32) = (0, 38, 0);

/// 0.38 scopes `secret set` globally by default and takes `--sandbox NAME` for
/// the other case. The positional-sandbox and `-g`/`--global` forms sbxw used
/// still work but print deprecation warnings, so they are worth leaving behind
/// before they are removed outright.
const SECRET_SCOPE_FLAGS_SINCE: (u32, u32, u32) = (0, 38, 0);

/// 0.38 introduced kit spec v2. v1 keeps loading through a legacy path, which
/// is why sbxw can still emit it for an older host rather than raising a floor.
pub const KIT_SPEC_V2_SINCE: (u32, u32, u32) = (0, 38, 0);

/// Parsed `sbx version`, read at most once per process.
static SBX_VERSION: OnceLock<Option<(u32, u32, u32)>> = OnceLock::new();

/// The running sbx's version, or `None` if it could not be run or parsed.
///
/// `assert_available` seeds this, so the common path costs nothing; the lazy
/// branch exists for the callers that never go through the startup check.
pub fn sbx_version() -> Option<(u32, u32, u32)> {
    *SBX_VERSION.get_or_init(|| {
        run_capture(&["version"])
            .ok()
            .as_deref()
            .and_then(parse_version)
    })
}

/// Is the running sbx at least `want`?
///
/// A version sbxw could not read answers **false**, deliberately: every gate
/// here chooses between a current form and a still-supported older one, so
/// "unknown" has to mean the form that works on both. Guessing the other way
/// would turn an unreadable `sbx version` into a hard failure, which is exactly
/// what `assert_available` refuses to do.
pub fn version_at_least(want: (u32, u32, u32)) -> bool {
    at_least(sbx_version(), want)
}

/// `version_at_least` without the process-global cache, so the "unknown reads
/// as old" rule is testable on its own.
fn at_least(found: Option<(u32, u32, u32)>, want: (u32, u32, u32)) -> bool {
    found.is_some_and(|found| found >= want)
}

/// Is `sbx` reachable, and recent enough?
///
/// A version we can't parse is a warning, never a refusal: the output format of
/// `sbx version` is not something sbxw should get to veto on.
pub fn assert_available() -> Result<()> {
    let raw = run_capture(&["version"]).context(
        "`sbx version` failed — install the standalone sbx binary and ensure it is on PATH",
    )?;

    // Seed the cache even when the floor check is skipped: the feature gates
    // still need to know which sbx they are talking to.
    let parsed = parse_version(&raw);
    let _ = SBX_VERSION.set(parsed);

    if std::env::var_os(SKIP_VERSION_CHECK_ENV).is_some() {
        tracing::debug!("sbx version check skipped via {SKIP_VERSION_CHECK_ENV}");
        return Ok(());
    }

    let (min_a, min_b, min_c) = MIN_SBX_VERSION;
    match parsed {
        Some(found) if found < MIN_SBX_VERSION => {
            let (a, b, c) = found;
            bail!(
                "this sbx is {a}.{b}.{c}, but sbxw needs {min_a}.{min_b}.{min_c} or newer.\n\
                 Upgrade sbx, or set {SKIP_VERSION_CHECK_ENV}=1 to run anyway — below \
                 0.37 you lose ports published at creation, SSH, shared skills and parts \
                 of the network-policy panel; below 0.35 `kit add`, `rm` and `run --name` \
                 behave differently and sbxw misprovisions rather than failing loudly."
            );
        }
        Some((a, b, c)) => {
            tracing::debug!("sbx {a}.{b}.{c} (needs >= {min_a}.{min_b}.{min_c})");
            if (a, b, c) < CURRENT_SBX_VERSION {
                let (c_a, c_b, c_c) = CURRENT_SBX_VERSION;
                tracing::debug!(
                    "sbx {a}.{b}.{c} is older than {c_a}.{c_b}.{c_c}: kits are written in the \
                     legacy v1 grammar and secrets are scoped with the pre-0.38 flags"
                );
            }
        }
        None => tracing::warn!(
            "could not read a version out of `sbx version` — continuing, but sbxw is built \
             for sbx {min_a}.{min_b}.{min_c} or newer"
        ),
    }
    Ok(())
}

/// First `MAJOR.MINOR[.PATCH]` in `raw`, so the check doesn't depend on how
/// `sbx version` frames it (`sbx version 0.35.0`, `Version: v0.40.1`, a table…).
///
/// A missing patch reads as `.0`, and any suffix (`-rc1`, `+build`) is ignored:
/// the comparison is a floor, and a release candidate for 0.36 is not older
/// than 0.35. Four-digit leading numbers are skipped so a build date printed
/// before the version can't be mistaken for one.
fn parse_version(raw: &str) -> Option<(u32, u32, u32)> {
    let bytes: Vec<char> = raw.chars().collect();
    let mut i = 0;

    while i < bytes.len() {
        if !bytes[i].is_ascii_digit() {
            i += 1;
            continue;
        }
        // A digit run only starts a candidate if it isn't the tail of a longer
        // token (the "256" in "sha256") — except after a `v`, which is how half
        // the CLIs in existence print a version.
        if i > 0 && bytes[i - 1].is_ascii_alphanumeric() && !matches!(bytes[i - 1], 'v' | 'V') {
            while i < bytes.len() && bytes[i].is_ascii_digit() {
                i += 1;
            }
            continue;
        }
        let start = i;
        let mut nums: Vec<u32> = Vec::new();
        loop {
            let from = i;
            while i < bytes.len() && bytes[i].is_ascii_digit() {
                i += 1;
            }
            if from == i {
                break;
            }
            match bytes[from..i].iter().collect::<String>().parse::<u32>() {
                Ok(n) => nums.push(n),
                Err(_) => break, // absurdly long digit run: not a version
            }
            if nums.len() == 3 || i >= bytes.len() || bytes[i] != '.' {
                break;
            }
            i += 1; // consume the dot and read the next component
        }
        // "0.35" and "0.35.0" are both versions; a bare "12" is not, and a
        // four-digit lead is a year.
        if nums.len() >= 2 && nums[0] < 1000 {
            return Some((nums[0], nums[1], *nums.get(2).unwrap_or(&0)));
        }
        if i == start {
            i += 1;
        }
    }
    None
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
    /// Forwarded as one `--kit` per entry, in order, and applied before the
    /// agent starts, so env vars they set are visible from the first process.
    ///
    /// Every kit the sandbox is meant to have belongs here rather than in a
    /// later `sbx kit add`: since 0.38 the recreate path behind `kit add`
    /// **refuses a kit that declares startup commands** ("does not yet apply"),
    /// telling you to `sbx rm` + `sbx create --kit` instead. Creation is the
    /// only moment a kit is applied whole.
    pub kits: &'a [String],
    /// Port specs (`[[HOST_IP:]HOST_PORT:]SANDBOX_PORT[/PROTO]`) forwarded as
    /// `-p`. See the note on `create_claude`.
    pub publish: &'a [String],
    /// When false, pass `--no-share-skills` to keep the host's shared skill
    /// store out of this sandbox.
    pub share_skills: bool,
}

/// `sbx create claude <workspace> --name <name> [--kit K…] [-p SPEC…] [--no-share-skills]`.
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
    for kit in opts.kits {
        args.push("--kit".into());
        args.push(kit.clone());
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
/// `run_checked`, not `run_inherit`, since 0.38: the failure mode that matters
/// here names a *different* kit than the one being added — a stale reference
/// re-resolved during the container swap — and callers cannot diagnose or
/// repair that from an exit status alone. stdout is still inherited, so 0.38's
/// live kit-install progress reaches the terminal.
pub fn kit_add(sandbox: &str, kit_path: &str) -> Result<()> {
    run_checked(&["kit", "add", sandbox, kit_path])
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

/// `sbx policy allow network [--sandbox <sandbox>] <resources>`.
///
/// `sandbox: None` writes a rule that applies to **every** sandbox on the host —
/// sbx's global local policy. Callers that mean "this project" must pass
/// `Some(name)`.
///
/// Uses `run_checked` so a refusal reaches the caller intact: a governance
/// denial can carry an organisation-configured support message (who to contact)
/// that is useless if it only ever lands in the daemon log.
pub fn policy_allow_network(sandbox: Option<&str>, resources: &str) -> Result<()> {
    run_checked(&policy_rule_args("allow", sandbox, resources))
}

/// `sbx policy deny network [--sandbox <sandbox>] <resources>`. See
/// `policy_allow_network` for what `None` means.
pub fn policy_deny_network(sandbox: Option<&str>, resources: &str) -> Result<()> {
    run_checked(&policy_rule_args("deny", sandbox, resources))
}

/// Argv for an allow/deny rule. Split out so both decisions place `--sandbox`
/// identically and the ordering is covered by one test.
fn policy_rule_args<'a>(
    decision: &'a str,
    sandbox: Option<&'a str>,
    resources: &'a str,
) -> Vec<&'a str> {
    let mut args = vec!["policy", decision, "network"];
    if let Some(name) = sandbox {
        args.push("--sandbox");
        args.push(name);
    }
    args.push(resources);
    args
}

/// `sbx policy rm <rule>` — drop a single rule from a local policy.
///
/// Takes the **rule** id from `policy ls --wide`, never a policy id: `sbx policy
/// inspect` accepts either, and handing a policy id to a command documented as
/// "Remove a policy rule" is how you delete far more than you meant to. The
/// caller is responsible for sourcing the id from a rule-id column.
///
/// Not verified against a live `sbx policy rm --help` — the argument shape is
/// inferred from the command's description. A mismatch fails loudly with sbx's
/// own message rather than removing the wrong thing.
pub fn policy_rm_rule(rule: &str) -> Result<()> {
    run_checked(&["policy", "rm", rule])
}

/// `sbx policy ls [SANDBOX] [--wide]` — the policy rules in force, including the
/// ones a kit composed in.
///
/// The sandbox is **positional**, not a `--sandbox` flag (confirmed against
/// `sbx policy ls --help`), and it changes what the command reports: without it
/// you get one overview row per policy *for every sandbox on the host*, with it
/// a summary of the rules that apply to that one. `wide` switches to the
/// rule-level table — the only view that names the resources (domains) a rule
/// covers rather than counting them.
///
/// If the scoped call fails (older sbx, unknown sandbox) we retry unscoped so
/// the caller degrades to the host-wide listing instead of an error. The
/// returned bool says which ran: "the rules for *this* sandbox" and "the rules
/// for *every* sandbox" must never look alike in the UI. On total failure the
/// scoped error wins — it is the one that explains the refusal.
pub fn policy_ls(sandbox: &str, wide: bool) -> Result<(String, bool)> {
    let mut scoped = vec!["policy", "ls", sandbox];
    let mut unscoped = vec!["policy", "ls"];
    if wide {
        scoped.push("--wide");
        unscoped.push("--wide");
    }
    match run_capture(&scoped) {
        Ok(out) => Ok((out, true)),
        Err(scoped_err) => match run_capture(&unscoped) {
            Ok(out) => Ok((out, false)),
            Err(_) => Err(scoped_err),
        },
    }
}

/// A parsed `sbx policy ls` table: its header names, and one entry per column
/// per row.
///
/// Deliberately schema-less. Unlike `sbx ports`, the policy columns are not
/// pinned by the CLI reference we built against, and sbx has already renamed
/// commands in this area (`set-default` → `init`). Mapping them onto named
/// fields would silently drop a column a future release adds, so we carry
/// whatever sbx printed and let the UI render it.
#[derive(Default)]
pub struct PolicyTable {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<String>>,
}

/// Parse a columnar `sbx policy ls` listing, e.g.
///
/// ```text
/// TYPE      RESOURCE            ACTION   ORIGIN            STATUS
/// domain    *.npmjs.org         allow    local policy      active
/// domain    telemetry.acme.com  deny     corporate policy  active
/// ```
///
/// Columns are located by where their headers start, not by splitting each row
/// on whitespace: policy values contain spaces ("corporate policy", "inactive —
/// superseded") and headers can too, so per-row splitting would shred them.
///
/// Returns an empty table for anything that isn't such a listing (a "no rules"
/// notice, a future `--format` output, a prose error). Callers are expected to
/// fall back to showing the raw text rather than claiming there are no rules.
pub fn parse_policy_table(raw: &str) -> PolicyTable {
    let mut lines = raw.lines().skip_while(|l| l.trim().is_empty());
    let header = match lines.next() {
        Some(h) => h,
        None => return PolicyTable::default(),
    };

    let spans = header_spans(header);
    if spans.len() < 2 {
        return PolicyTable::default();
    }

    let columns: Vec<String> = spans.iter().map(|(name, _)| name.clone()).collect();
    let starts: Vec<usize> = spans.iter().map(|(_, start)| *start).collect();

    let rows = lines
        .filter(|l| !l.trim().is_empty())
        .map(|line| slice_at(line, &starts))
        // A row that lands entirely in the first column is a separator or a
        // trailing note, not data.
        .filter(|cells| cells.iter().skip(1).any(|c| !c.is_empty()))
        .collect();

    PolicyTable { columns, rows }
}

/// `sbx policy log [SANDBOX]` — the connections sbx recently allowed or blocked,
/// with the rule and reason behind each decision.
///
/// This is the only view that reports what actually happened rather than what is
/// configured, so it answers "why was that request refused?". The sandbox
/// argument is assumed positional, like `policy ls`'s; the unscoped retry covers
/// the case where it isn't.
pub fn policy_log(sandbox: &str) -> Result<(String, bool)> {
    match run_capture(&["policy", "log", sandbox]) {
        Ok(out) => Ok((out, true)),
        Err(scoped_err) => match run_capture(&["policy", "log"]) {
            Ok(out) => Ok((out, false)),
            Err(_) => Err(scoped_err),
        },
    }
}

/// What a policy column holds, for the ones we know how to present. Anything
/// else maps to `""` and is rendered as plain text, so a column sbx adds shows
/// up instead of vanishing.
///
/// Covers both listings: `policy ls` (`POLICY  SOURCE  APPLIES TO  SUMMARY`,
/// confirmed against real output) and `policy log`, whose columns are described
/// as host / rule / reason / last-seen but not pinned — hence the aliases.
pub fn policy_column_roles(columns: &[String]) -> Vec<&'static str> {
    columns
        .iter()
        .map(|c| {
            let c = c.trim().to_ascii_uppercase();
            match () {
                // Must precede the generic id arm: `policy rm` takes a *rule*
                // id, and a policy id reaching it would delete a whole policy
                // instead of one rule. The two are told apart here or nowhere.
                _ if c == "RULE ID" || c == "RULE_ID" || c == "RULEID" => "rule_id",
                _ if c == "POLICY" || c == "ID" || c.ends_with(" ID") => "id",
                _ if c == "SOURCE" || c == "ORIGIN" => "source",
                _ if c.starts_with("APPLIES") || c == "SCOPE" || c == "TARGET" => "applies",
                _ if c == "SUMMARY" || c == "RULES" => "summary",
                _ if c == "SANDBOX" => "sandbox",
                _ if c == "HOST" || c == "DOMAIN" || c == "RESOURCE" => "host",
                _ if c == "RULE" => "rule",
                _ if c == "REASON" || c == "DETAIL" => "reason",
                _ if c == "ACTION" || c == "DECISION" => "action",
                _ if c == "TYPE" => "type",
                _ if c == "STATUS" || c == "STATE" => "status",
                _ if c.contains("SEEN") || c == "TIME" || c == "WHEN" => "when",
                _ => "",
            }
        })
        .collect()
}

/// Does a policy whose "applies to" cell reads `applies` govern `sandbox`?
///
/// sbx scopes a policy either to every sandbox (`all`) or to one by name
/// (`sandbox:<name>`), and `policy ls` lists the policies of *all* sandboxes —
/// so without this the panel for one sandbox shows a dozen rules that have
/// nothing to do with it.
///
/// An unrecognised scope counts as applying. Showing one row too many is a
/// cosmetic annoyance; hiding a rule that does govern the sandbox would make
/// the panel lie about what egress is allowed.
pub fn policy_applies_to(applies: &str, sandbox: &str) -> bool {
    let applies = applies.trim();
    match applies.split_once(':') {
        Some(("sandbox", target)) => target.trim() == sandbox,
        _ => true,
    }
}

/// Header names and their starting character offsets, or empty if `line` does
/// not look like a table header. Fields are separated by two or more spaces so
/// that a multi-word header ("RULE NAME", "LAST SEEN") stays one column.
fn header_spans(line: &str) -> Vec<(String, usize)> {
    let chars: Vec<char> = line.chars().collect();
    let mut out: Vec<(String, usize)> = Vec::new();
    let mut i = 0;

    while i < chars.len() {
        if chars[i] == ' ' {
            i += 1;
            continue;
        }
        let start = i;
        let mut end = i;
        while i < chars.len() {
            if chars[i] == ' ' {
                // Two spaces in a row close the field; a single one is part of
                // a multi-word header.
                if chars.get(i + 1).is_none_or(|c| *c == ' ') {
                    break;
                }
            } else {
                end = i + 1;
            }
            i += 1;
        }
        out.push((chars[start..end].iter().collect(), start));
    }

    // Only an ALL-CAPS row is a header. Without this check the first *data*
    // line of a non-table output would be mistaken for one.
    let caps = out.iter().all(|(name, _)| {
        name.chars().any(|c| c.is_ascii_uppercase())
            && !name.contains(|c: char| c.is_ascii_lowercase())
    });
    if caps {
        out
    } else {
        Vec::new()
    }
}

/// Cut `line` at the given character offsets, trimming each cell. A value wider
/// than its column bleeds into the next one; we accept that over dropping it,
/// since sbx pads its tables to fit.
fn slice_at(line: &str, starts: &[usize]) -> Vec<String> {
    let chars: Vec<char> = line.chars().collect();
    starts
        .iter()
        .enumerate()
        .map(|(i, &start)| {
            let end = starts.get(i + 1).copied().unwrap_or(chars.len());
            let start = start.min(chars.len());
            let end = end.min(chars.len()).max(start);
            chars[start..end]
                .iter()
                .collect::<String>()
                .trim()
                .to_string()
        })
        .collect()
}

/// The scope arguments for `sbx secret set`, in the grammar `modern` selects.
///
/// sbx 0.38 made **global the default** and moved sandbox scope onto
/// `--sandbox NAME`; the forms sbxw used until now — a bare positional sandbox,
/// and `-g` for global — still work there but print a deprecation warning on
/// every call, and a warning nobody can act on is one people learn to ignore.
///
/// So: on 0.38+ a global secret passes *nothing at all* and a sandbox-scoped
/// one passes the flag; below that, the old spellings, which is the only thing
/// those releases understand. Both branches mean the same two scopes.
fn secret_scope_args(modern: bool, global: bool, sandbox: Option<&str>) -> Vec<String> {
    match (modern, global, sandbox) {
        // 0.38+: global is what you get when you say nothing.
        (true, false, Some(name)) => vec!["--sandbox".into(), name.into()],
        (true, _, _) => vec![],
        (false, true, _) => vec!["-g".into()],
        (false, false, Some(name)) => vec![name.into()],
        (false, false, None) => vec![],
    }
}

/// Store a service-scoped secret by piping the value on stdin (keeps it out of
/// argv / shell history). `service` must be one of sbx's known services
/// (anthropic, openai, github, ...). For a global secret pass `global = true`.
///
/// The scope is spelled differently either side of sbx 0.38 — see
/// `secret_scope_args`.
pub fn secret_set_stdin(
    service: &str,
    value: &str,
    global: bool,
    sandbox: Option<&str>,
) -> Result<()> {
    let mut args: Vec<String> = vec!["secret".into(), "set".into()];
    args.extend(secret_scope_args(
        version_at_least(SECRET_SCOPE_FLAGS_SINCE),
        global,
        sandbox,
    ));
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

/// Path (inside the sandbox) the cross-sandbox relay CLI is installed at.
const RELAY_TOOL_PATH: &str = "/home/agent/.sbxw/relay.js";

/// The same relay, wrapped as an MCP server so it appears in the agent's *tool
/// list* rather than only in prose it read at startup. Sits beside the CLI
/// because it `require`s it (`./relay.js`).
const RELAY_MCP_PATH: &str = "/home/agent/.sbxw/relay-mcp.js";

/// Name the MCP server is registered under, and therefore the prefix its tools
/// appear with. Kept short: it is read on every turn.
const RELAY_MCP_NAME: &str = "sbxw-relay";

/// Claude Code's user-level memory file — read at the start of every session in
/// this sandbox, whatever project is open. Where the relay is *documented*,
/// since a tool nothing mentions is a tool no agent ever runs.
const USER_MEMORY_PATH: &str = "/home/agent/.claude/CLAUDE.md";

/// Fences around sbxw's section of `USER_MEMORY_PATH`. That file also belongs to
/// the user and to the agent (`#` memories land there), so every install
/// replaces what sits *between* these and leaves the rest alone.
const RELAY_DOC_BEGIN: &str = "<!-- sbxw:relay:begin -->";
const RELAY_DOC_END: &str = "<!-- sbxw:relay:end -->";

/// What the agent is told about the relay, verbatim, in its user memory.
///
/// This is the *trigger*, not the manual — the tool's own description (see
/// `assets/relay-mcp.js`) explains how to use it, and the message the daemon
/// types into a target session carries the answering side's instructions. What
/// memory adds is recognising the moment, because the moment does not announce
/// itself: it looks like an ordinary dead end, and the reflex it has to beat is
/// "tell the user I can't see it" — which reads as diligence, not as a miss.
///
/// Deliberately short and unhedged. An earlier version led with "use it
/// sparingly", and a session that had established the code it needed was in an
/// unmounted repo went on to offer the user a codebase search instead of asking.
/// A brake reads the same whether or not the case in front of it is the one
/// worth spending on, so the restraint moved into the tool description, where it
/// is read *while deciding to call it* rather than while deciding whether to
/// consider it at all.
fn relay_doc() -> String {
    format!(
        "{RELAY_DOC_BEGIN}\n\
         ## When the answer isn't in this workspace\n\
         \n\
         Other sandboxes run on this machine, each with its own agent and its own\n\
         project, and you can ask them — a human routes the question and approves\n\
         the answer. Use the **`ask_other_sandbox`** tool (MCP server\n\
         `{RELAY_MCP_NAME}`) for this.\n\
         \n\
         The moment to reach for it is easy to miss, so name it explicitly: you\n\
         have searched, and what you need is in a repo, service or project that\n\
         is not mounted here. **Ask before you tell the user you cannot see it,\n\
         and before you offer to go looking elsewhere.** Finding the boundary of\n\
         your workspace is not the answer to a question — it is the point at\n\
         which to ask one.\n\
         \n\
         Same thing from a shell, if the tool is unavailable:\n\
         `node {RELAY_TOOL_PATH} ask \"your question\"` (then `wait <id>`).\n\
         \n\
         The question is read by a human and then by an agent that knows nothing\n\
         of this conversation, so make it self-contained. Never send secrets or\n\
         file contents you were not asked to share.\n\
         {RELAY_DOC_END}\n"
    )
}

/// Install the cross-sandbox relay, in three parts: the CLI, the MCP server
/// that puts it in the agent's tool list, and the paragraph in the agent's user
/// memory that says when to reach for it. `web_port` is the daemon's port — the
/// same one the status hook already reaches, so no extra network rule is needed.
///
/// All three, because they answer different failures. The CLI is the thing that
/// works; the MCP server is what makes it *considered* (an agent weighs its
/// tools every turn and its memory only sometimes); the memory paragraph is what
/// names the moment, which otherwise looks like an ordinary dead end.
///
/// The memory file is rewritten around sbxw's fenced section rather than
/// overwritten: Claude Code writes `#` memories into that same file, and the
/// user may have put their own standing instructions there. `.claude.json` is
/// merged for the same reason.
pub fn install_relay_tool(sandbox: &str, web_port: &str) -> Result<()> {
    const TOOL: &str = include_str!("../assets/relay-tool.js");
    const MCP: &str = include_str!("../assets/relay-mcp.js");
    let script = TOOL.replace("__PORT__", web_port);
    write_file_stdin(sandbox, RELAY_TOOL_PATH, script.as_bytes())?;
    // The MCP wrapper reads the daemon's port from the CLI it requires, so only
    // one copy of it is ever templated in.
    write_file_stdin(sandbox, RELAY_MCP_PATH, MCP.as_bytes())?;

    // Register the server at *user* scope — every project in this sandbox, not
    // just the one that happens to be open. That lives at the top level of
    // `.claude.json` under `mcpServers` (the same file `trust_workspace`
    // merges into), which is why this is a merge and not a write: that file
    // also holds the trust flags, onboarding state and the agent's own config.
    let mcp_script = format!(
        "const fs=require('fs');const p='/home/agent/.claude.json';\
         let d={{}};try{{d=JSON.parse(fs.readFileSync(p,'utf8'))}}catch(e){{}}\
         d.mcpServers=d.mcpServers||{{}};\
         d.mcpServers[{name}]={{type:'stdio',command:'node',args:[{path}],env:{{}}}};\
         fs.writeFileSync(p,JSON.stringify(d));",
        name = serde_json::to_string(RELAY_MCP_NAME)?,
        path = serde_json::to_string(RELAY_MCP_PATH)?,
    );
    run_node_script(sandbox, "/tmp/.sbxw-relay-mcp-install.js", &mcp_script)?;

    let merge_script = format!(
        "const fs=require('fs');const p={path};\
         const begin={begin},end={end},doc={doc};\
         let cur='';try{{cur=fs.readFileSync(p,'utf8')}}catch(e){{}}\
         const from=cur.indexOf(begin),to=cur.indexOf(end);\
         let rest=(from>=0&&to>from)?cur.slice(0,from)+cur.slice(to+end.length):cur;\
         rest=rest.replace(/\\n{{3,}}/g,'\\n\\n').trim();\
         fs.mkdirSync(require('path').dirname(p),{{recursive:true}});\
         fs.writeFileSync(p,(rest?rest+'\\n\\n':'')+doc);",
        path = serde_json::to_string(USER_MEMORY_PATH)?,
        begin = serde_json::to_string(RELAY_DOC_BEGIN)?,
        end = serde_json::to_string(RELAY_DOC_END)?,
        doc = serde_json::to_string(&relay_doc())?,
    );
    run_node_script(sandbox, "/tmp/.sbxw-relay-install.js", &merge_script)
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

    /// `sbx version`'s exact framing isn't pinned by anything sbxw controls, so
    /// the floor check must survive it changing.
    #[test]
    fn version_is_found_however_sbx_frames_it() {
        assert_eq!(parse_version("sbx version 0.35.0"), Some((0, 35, 0)));
        assert_eq!(parse_version("v0.40.1\n"), Some((0, 40, 1)));
        assert_eq!(parse_version("Version:  1.2\n"), Some((1, 2, 0)));
        // A pre-release of 0.36 is not older than 0.35 — the suffix is noise.
        assert_eq!(parse_version("sbx version 0.36.0-rc1"), Some((0, 36, 0)));
        assert_eq!(
            parse_version("sbx CLI\n  Version: 0.41.2\n  Commit: abc123\n"),
            Some((0, 41, 2))
        );
        // A build date printed first must not be read as the version.
        assert_eq!(
            parse_version("built 2026.07.30\nsbx version 0.35.1"),
            Some((0, 35, 1))
        );
        // Digits inside a word aren't a version, and neither is a bare number.
        assert_eq!(parse_version("sha256 checksum only"), None);
        assert_eq!(parse_version("sbx build 12"), None);
        assert_eq!(parse_version(""), None);
    }

    /// Tuple ordering is the whole comparison, so it is worth stating once.
    #[test]
    fn version_floor_compares_component_wise() {
        assert!(parse_version("0.34.9").unwrap() < MIN_SBX_VERSION);
        // The tier that only *degrades* is still below the floor: silently
        // missing features is what this check is for.
        assert!(parse_version("0.36.9").unwrap() < MIN_SBX_VERSION);
        assert!(parse_version("0.37.0").unwrap() >= MIN_SBX_VERSION);
        assert!(parse_version("0.37").unwrap() >= MIN_SBX_VERSION);
        assert!(parse_version("0.100.0").unwrap() > MIN_SBX_VERSION);
        assert!(parse_version("1.0.0").unwrap() > MIN_SBX_VERSION);
    }

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

    /// 0.38 turned both spellings sbxw used into deprecation warnings: global
    /// stopped needing `-g` and sandbox scope moved onto `--sandbox`.
    #[test]
    fn secret_scope_follows_the_grammar_of_the_running_sbx() {
        // 0.38+: global says nothing at all, sandbox scope takes the flag.
        assert!(secret_scope_args(true, true, None).is_empty());
        assert!(secret_scope_args(true, true, Some("neos")).is_empty());
        assert_eq!(
            secret_scope_args(true, false, Some("neos")),
            vec!["--sandbox".to_string(), "neos".to_string()]
        );

        // Below it, the older spellings — the only ones those releases parse.
        assert_eq!(secret_scope_args(false, true, None), vec!["-g".to_string()]);
        assert_eq!(
            secret_scope_args(false, false, Some("neos")),
            vec!["neos".to_string()]
        );
    }

    /// A version sbxw cannot read must select the grammar that works on *both*
    /// sides of the gate, never the newer one.
    #[test]
    fn an_unreadable_version_is_not_at_least_anything() {
        assert!(!at_least(None, KIT_SPEC_V2_SINCE));
        assert!(!at_least(None, (0, 1, 0)));

        assert!(at_least(Some((0, 38, 0)), KIT_SPEC_V2_SINCE));
        assert!(at_least(Some((0, 41, 2)), KIT_SPEC_V2_SINCE));
        assert!(!at_least(Some((0, 37, 9)), KIT_SPEC_V2_SINCE));
    }

    /// `relay-mcp.js` reaches its transport with `require("./relay.js")`, so the
    /// two are only ever one edit away from a server that starts, connects,
    /// lists no tools nobody notices are missing, and fails on the first call.
    /// Nothing else in the build would catch that: they are opaque strings here.
    #[test]
    fn the_relay_mcp_server_is_installed_beside_the_cli_it_requires() {
        let dir_of = |p: &str| p.rsplit_once('/').expect("absolute path").0.to_string();
        assert_eq!(dir_of(RELAY_MCP_PATH), dir_of(RELAY_TOOL_PATH));
        assert!(RELAY_TOOL_PATH.ends_with("/relay.js"), "{RELAY_TOOL_PATH}");

        let mcp = include_str!("../assets/relay-mcp.js");
        assert!(mcp.contains(r#"require("./relay.js")"#), "sibling require");
        // The CLI has to stay requirable — it grew a `main()` first, and running
        // it on import would make the MCP server ask a question at startup.
        let cli = include_str!("../assets/relay-tool.js");
        assert!(cli.contains("require.main === module"), "main is guarded");
        assert!(cli.contains("module.exports"), "the transport is exported");
    }

    /// The agent's memory is where the *moment* is named — the tool description
    /// covers everything else. This pins the phrasing that earns its place:
    /// without it, a session that has just found the edge of its workspace
    /// reports that edge to the user instead of asking past it.
    #[test]
    fn the_memory_block_names_the_moment_rather_than_the_mechanism() {
        let doc = relay_doc();
        assert!(doc.starts_with(RELAY_DOC_BEGIN), "fenced for re-install");
        assert!(doc.trim_end().ends_with(RELAY_DOC_END));
        assert!(doc.contains("ask_other_sandbox"), "names the tool");
        assert!(
            doc.contains("before you tell the user you cannot see it"),
            "names the moment it is for: {doc}"
        );
        // The shell fallback has to keep working when MCP doesn't.
        assert!(doc.contains(RELAY_TOOL_PATH), "keeps the CLI fallback");
        // And it must not re-introduce the brake that suppressed it: restraint
        // belongs in the tool description, read while deciding to *call* it.
        assert!(
            !doc.to_lowercase().contains("sparingly"),
            "the memory block must not discourage the tool it exists to trigger"
        );
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
    fn policy_table_keeps_values_that_contain_spaces() {
        // "corporate policy" and "inactive — superseded" are single cells; a
        // split_whitespace parse would tear them apart and shift every column.
        let raw = "\
TYPE     RESOURCE            ACTION  ORIGIN            STATUS
domain   *.npmjs.org         allow   local policy      active
domain   telemetry.acme.com  deny    corporate policy  inactive — superseded
";
        let table = parse_policy_table(raw);
        assert_eq!(
            table.columns,
            ["TYPE", "RESOURCE", "ACTION", "ORIGIN", "STATUS"]
        );
        assert_eq!(
            table.rows[0],
            ["domain", "*.npmjs.org", "allow", "local policy", "active"]
        );
        assert_eq!(table.rows[1][3], "corporate policy");
        assert_eq!(table.rows[1][4], "inactive — superseded");
    }

    #[test]
    fn policy_table_keeps_multi_word_headers_in_one_column() {
        let raw = "\
RULE NAME   RESOURCE       LAST SEEN
dev-allow   github.com     2 minutes ago
";
        let table = parse_policy_table(raw);
        assert_eq!(table.columns, ["RULE NAME", "RESOURCE", "LAST SEEN"]);
        assert_eq!(table.rows[0], ["dev-allow", "github.com", "2 minutes ago"]);
    }

    /// The listing sbx actually prints: one row per *policy document*, scoped
    /// to one sandbox or to all of them.
    #[test]
    fn policy_table_parses_the_real_sbx_listing() {
        let raw = "\
POLICY                                SOURCE  APPLIES TO       SUMMARY
057fdbc3-50f9-41dc-8ca5-9365f52962a0  kit     sandbox:test-1   network: 4 allow
25a42ef4-e948-47a2-952b-b4d7aad8fbc1  local   sandbox:sbxw-2   network: 23 allow
local-policy                          local   all              network: 159 allow; filesystem read: 1 allow
";
        let table = parse_policy_table(raw);
        assert_eq!(
            policy_column_roles(&table.columns),
            ["id", "source", "applies", "summary"]
        );
        assert_eq!(table.rows.len(), 3);
        assert_eq!(table.rows[2][0], "local-policy");
        // The multi-clause summary must survive intact — it is the only place
        // the listing says anything about what the policy actually does.
        assert_eq!(
            table.rows[2][3],
            "network: 159 allow; filesystem read: 1 allow"
        );
    }

    /// The scope flag decides whether a rule governs one sandbox or every one
    /// of them, so its presence and placement are pinned by a test rather than
    /// left to a reading of the call site.
    #[test]
    fn policy_rule_args_place_the_scope_flag() {
        assert_eq!(
            policy_rule_args("allow", Some("neos"), "github.com"),
            [
                "policy",
                "allow",
                "network",
                "--sandbox",
                "neos",
                "github.com"
            ]
        );
        // No `--sandbox` at all — anything else would silently scope a rule the
        // caller meant to apply host-wide, or vice versa.
        assert_eq!(
            policy_rule_args("deny", None, "telemetry.example"),
            ["policy", "deny", "network", "telemetry.example"]
        );
    }

    /// A rule id and a policy id are different things to `policy rm`; only the
    /// former may reach it, so they must not share a role.
    #[test]
    fn rule_id_and_policy_id_are_distinct_roles() {
        let columns: Vec<String> = ["POLICY", "RULE ID", "RESOURCE", "DECISION"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(
            policy_column_roles(&columns),
            ["id", "rule_id", "host", "action"]
        );
    }

    #[test]
    fn policy_scope_keeps_this_sandbox_and_the_global_ones() {
        assert!(policy_applies_to("sandbox:sbxw-2", "sbxw-2"));
        assert!(policy_applies_to("all", "sbxw-2"));
        assert!(!policy_applies_to("sandbox:test-1", "sbxw-2"));
        // A near-miss on the name must not slip through.
        assert!(!policy_applies_to("sandbox:sbxw-22", "sbxw-2"));
        // A scope syntax we don't know is shown, never hidden.
        assert!(policy_applies_to("workspace:/src/app", "sbxw-2"));
        assert!(policy_applies_to("", "sbxw-2"));
    }

    #[test]
    fn policy_table_rejects_output_that_is_not_a_table() {
        // The caller shows the raw text in this case; claiming an empty table
        // would read as "no rules in force", which is the opposite of true.
        assert!(parse_policy_table("No policy rules configured.\n")
            .columns
            .is_empty());
        assert!(parse_policy_table("").columns.is_empty());
        assert!(parse_policy_table("{\n  \"rules\": []\n}\n")
            .columns
            .is_empty());
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
