//! The only door to the `sbx` CLI. sbxw never calls `docker sandbox`.
//!
//! Written for sbx 0.48 and newer, with no fallbacks below it: since 0.39 an
//! unrecognised command or flag is an *error*, so a command that succeeded
//! really ran, and a fallback path would be code nobody runs. 0.48 rewrote
//! every human-readable table, so what sbxw parses it reads as `--format json`.
//!
//! ```text
//! sbx create claude PATH --name N [PATH:ro]… [--kit K]… [-p SPEC]…
//!            [--skills=off|readonly|readwrite] [--pull=always|missing|never]
//!            [--env-file F]… [-e K=V]…        (creation: the only moment kits apply whole;
//!                                              -p is all-or-nothing)
//! sbx run --name N [--env-file F]… [-e K=V]…  (attach; -e applies to the agent session)
//! sbx run -d --name N                         (keep running with nobody attached)
//! sbx ls --format json                        (a bare array; table read by headers as a fallback)
//! sbx exec [-i|-it] N -- cmd…                 (only a *running* sandbox; -d fails)
//! sbx ports N --format json · sbx ports N --publish|--unpublish SPEC
//! sbx policy allow|deny network [--sandbox N] RESOURCES
//! sbx policy ls [N] [--wide] · sbx policy log [N]   (N positional here)
//! sbx policy rm network --id RULE_ID [--sandbox N] --force
//! sbx secret set [--sandbox N] SERVICE [--force]  (value on stdin)
//! sbx kit add N REF                           (recreates the container; startup commands since 0.48)
//! sbx env create PATH… --name N [--env-arg K=V]…   (shows a plan and asks)
//! sbx prune [--dry-run] [--filter until=D] --force
//! sbx rm --force --format json N…            (one result per target)
//! sbx stop N
//! sbx policy log [N] --format json · sbx policy refresh
//! sbx mcp ls --format json · sbx mcp unload SERVER --sandbox N
//! sbx kit validate REF --offline              (local dir or ZIP; no daemon, no sign-in)
//! sbx template save N TAG                    (--description is cloud-only: refused locally)
//! sbx template ls · sbx template rm TAG · create --template TAG
//! sbx diagnose --format json · sbx skills import · sbx setup ssh
//! ```
//!
//! Facts the methods below are built on:
//! * every removal asks for confirmation since 0.45, and a prompt with no
//!   terminal is a refusal (since 0.48 even a piped `y`) — sbxw passes
//!   `--force` and does its own asking;
//! * a sandbox made with `sbx create` stops by itself once idle;
//! * a sandbox whose guest stopped responding is not running;
//! * `policy allow|deny` refused with "managed by your organization" is a
//!   governed host, not a broken one.

pub mod runner;
pub mod scripts;

use anyhow::{anyhow, bail, Result};
use runner::{Invocation, Output, Runner};
use sbxw_core::policy::{parse_mcp_ls, EgressLog, McpServer, PolicyView};
use sbxw_core::ports::PortMapping;
use sbxw_core::table;
use sbxw_core::tooling::{Tooling, ToolingOutcome};
use sbxw_core::{SandboxName, ThreadName};
use serde::Serialize;
use std::sync::Arc;
use std::time::Duration;

pub use scripts::ThreadStatus;

/// Oldest sbx sbxw works against, and the one it is written for.
pub const MIN_VERSION: (u32, u32, u32) = (0, 48, 0);
pub const SKIP_VERSION_CHECK_ENV: &str = "SBXW_SKIP_SBX_VERSION_CHECK";

/// Longest tail of a failing command's stderr folded into the error: sbx's
/// policy denials carry a multi-line explanation and an org support message.
const STDERR_TAIL_LIMIT: usize = 2000;

/// A failed invocation as an error carrying *what sbx said* (its tail: sbx
/// prints context first and the reason last).
pub fn command_error(args: &[String], out: &Output) -> anyhow::Error {
    let shown = args.iter().take(4).cloned().collect::<Vec<_>>().join(" ");
    let status = out.code.map_or_else(|| "a signal".to_string(), |c| format!("exit status {c}"));
    let msg = String::from_utf8_lossy(&out.stderr);
    let msg = msg.trim();
    if msg.is_empty() {
        return anyhow!("`sbx {shown}` exited with {status}");
    }
    let tail = match msg.char_indices().nth_back(STDERR_TAIL_LIMIT) {
        Some((i, _)) => format!("…{}", &msg[i..]),
        None => msg.to_string(),
    };
    anyhow!("`sbx {shown}` exited with {status}:\n{tail}")
}

/// One row of `sbx ls`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SandboxInfo {
    pub name: String,
    pub agent: String,
    pub status: String,
    /// RFC 3339, from the 0.48 JSON; absent from the table fallback.
    pub created_at: Option<String>,
    /// Running with nobody attached (`sbx run -d`).
    pub detached: bool,
}

impl SandboxInfo {
    /// Up *and answering*: a hung guest reported as running is not (0.47 said
    /// "unresponsive", 0.48 "not responding").
    pub fn is_running(&self) -> bool {
        let s = self.status.to_ascii_lowercase();
        self.state() == "running" && !s.contains("unresponsive") && !s.contains("not responding")
    }
    pub fn is_stopped(&self) -> bool {
        self.state() == "stopped"
    }
    /// The first word of the status, past any state mark (0.48 prints `● running`).
    fn state(&self) -> String {
        self.status
            .split_whitespace()
            .find(|w| w.chars().any(char::is_alphanumeric))
            .unwrap_or("")
            .to_ascii_lowercase()
    }
}

/// `sbx ls --format json` (0.48+: a bare array of snake_case rows); `None`
/// when `raw` is not that, so the caller can read it as a table.
pub fn parse_ls_json(raw: &str) -> Option<Vec<SandboxInfo>> {
    let rows: Vec<serde_json::Value> = serde_json::from_str(raw.trim()).ok()?;
    let text = |v: &serde_json::Value, keys: &[&str]| {
        keys.iter().find_map(|k| v.get(*k).and_then(|x| x.as_str())).unwrap_or("").to_string()
    };
    Some(
        rows.iter()
            .filter_map(|v| {
                let name = text(v, &["name", "sandbox"]);
                if name.is_empty() {
                    return None;
                }
                let mut status = text(v, &["status", "state"]);
                // A hung guest may be a flag beside a "running" status.
                let hung = ["not_responding", "unresponsive"].iter().any(|k| v.get(*k).and_then(|x| x.as_bool()) == Some(true))
                    || v.get("responsive").and_then(|x| x.as_bool()) == Some(false);
                if hung && !status.to_ascii_lowercase().contains("respon") {
                    status.push_str(" (not responding)");
                }
                Some(SandboxInfo {
                    name,
                    agent: text(v, &["agent"]),
                    status: if status.is_empty() { "unknown".into() } else { status },
                    created_at: Some(text(v, &["created_at"])).filter(|c| !c.is_empty()),
                    detached: v.get("detached").and_then(|x| x.as_bool()).unwrap_or(false),
                })
            })
            .collect(),
    )
}

/// One saved template, as `sbx template ls` lists it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TemplateInfo {
    /// What `create --template` and `template rm` take.
    pub tag: String,
    pub size: Option<String>,
    pub created: Option<String>,
}

/// `sbx template ls`, as JSON (an array, bare or under `templates`) or by its
/// headers: a tag column (`TAG`, `NAME`, `TEMPLATE`, `REFERENCE`), or
/// `REPOSITORY` + `TAG` joined as docker does. Not sbx's own format to rely
/// on: every shape it might take is read, and anything else lists nothing.
pub fn parse_template_ls(raw: &str) -> Vec<TemplateInfo> {
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(raw.trim()) {
        let rows = v.as_array().or_else(|| v.get("templates").and_then(|t| t.as_array())).cloned().unwrap_or_default();
        let text = |v: &serde_json::Value, keys: &[&str]| keys.iter().find_map(|k| v.get(*k).and_then(|x| x.as_str())).filter(|s| !s.is_empty()).map(String::from);
        return rows
            .iter()
            .filter_map(|v| {
                let tag = match (text(v, &["repository", "repo"]), text(v, &["tag"])) {
                    (Some(r), Some(t)) if !t.contains(':') => format!("{r}:{t}"),
                    _ => text(v, &["tag", "name", "template", "reference", "ref"])?,
                };
                Some(TemplateInfo { tag, size: text(v, &["size"]), created: text(v, &["created_at", "created"]) })
            })
            .collect();
    }
    let t = table::parse(raw);
    let col = |names: &[&str]| t.columns.iter().position(|h| names.iter().any(|n| h.eq_ignore_ascii_case(n)));
    let (repo, tag) = (col(&["REPOSITORY"]), col(&["TAG", "NAME", "TEMPLATE", "REFERENCE"]));
    let (size, created) = (col(&["SIZE"]), col(&["CREATED", "CREATED AT"]));
    let Some(tag) = tag.or(repo) else {
        return Vec::new();
    };
    let cell = |row: &Vec<String>, i: Option<usize>| i.and_then(|i| row.get(i)).filter(|c| !c.is_empty()).cloned();
    t.rows
        .iter()
        .filter_map(|row| {
            let t = cell(row, Some(tag))?;
            let tag = match cell(row, repo) {
                Some(r) if Some(tag) != repo && !t.contains(':') => format!("{r}:{t}"),
                _ => t,
            };
            Some(TemplateInfo { tag, size: cell(row, size), created: cell(row, created) })
        })
        .collect()
}

/// What `sbx rm` did with one target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RmResult {
    pub name: String,
    /// `removed` or `removing`: sbxw may forget it.
    pub removed: bool,
    pub error: Option<String>,
}

/// `sbx rm --format json` (0.48+): one result per target, missing and failed
/// ones included. `None` when `raw` is not that.
pub fn parse_rm_json(raw: &str) -> Option<Vec<RmResult>> {
    let rows: Vec<serde_json::Value> = serde_json::from_str(raw.trim()).ok()?;
    Some(
        rows.iter()
            .filter_map(|v| {
                let name = v.get("name")?.as_str()?.to_string();
                let action = v.get("action").and_then(|a| a.as_str()).unwrap_or("").to_ascii_lowercase();
                let error = v.get("error").and_then(|e| e.as_str()).filter(|e| !e.is_empty()).map(String::from);
                Some(RmResult {
                    name,
                    removed: matches!(action.as_str(), "removed" | "removing") && error.is_none(),
                    error,
                })
            })
            .collect(),
    )
}

/// `sbx ls`, by its headers (`STATUS`/`STATE`, `AGENT`, name first); a layout
/// without readable headers falls back to `NAME AGENT STATUS`.
pub fn parse_ls(raw: &str) -> Vec<SandboxInfo> {
    let mut lines = raw.lines().filter(|l| !l.trim().is_empty());
    let Some(header) = lines.next() else {
        return Vec::new();
    };
    let spans = table::header_spans(header);
    let find = |names: &[&str]| spans.iter().position(|(h, _)| names.iter().any(|n| h.eq_ignore_ascii_case(n)));
    let (agent_col, status_col) = (find(&["AGENT"]), find(&["STATUS", "STATE"]));
    match status_col {
        Some(status_col) if spans.len() >= 2 => {
            let starts: Vec<usize> = spans.iter().map(|(_, at)| *at).collect();
            lines
                .filter_map(|line| {
                    let cells = table::slice_at(line, &starts);
                    // Past a leading state mark, as in the status column.
                    let name = cells.first()?.split_whitespace().find(|w| w.chars().any(char::is_alphanumeric))?.to_string();
                    let cell = |i: Option<usize>| i.and_then(|i| cells.get(i)).cloned().unwrap_or_default();
                    let status = cell(Some(status_col));
                    Some(SandboxInfo {
                        name,
                        agent: cell(agent_col),
                        status: if status.is_empty() { "unknown".into() } else { status },
                        created_at: None,
                        detached: false,
                    })
                })
                .collect()
        }
        _ => lines
            .filter_map(|line| {
                let mut cols = line.split_whitespace();
                Some(SandboxInfo {
                    name: cols.next()?.to_string(),
                    agent: cols.next().unwrap_or("").to_string(),
                    status: cols.next().unwrap_or("unknown").to_string(),
                    created_at: None,
                    detached: false,
                })
            })
            .collect(),
    }
}

/// The first `MAJOR.MINOR[.PATCH]` in `raw`, however it is framed; a digit run
/// inside a longer token ("sha256") or a four-digit year is not one.
pub fn parse_version(raw: &str) -> Option<(u32, u32, u32)> {
    let chars: Vec<char> = raw.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if !chars[i].is_ascii_digit() {
            i += 1;
            continue;
        }
        if i > 0 && chars[i - 1].is_ascii_alphanumeric() && !matches!(chars[i - 1], 'v' | 'V') {
            while i < chars.len() && chars[i].is_ascii_digit() {
                i += 1;
            }
            continue;
        }
        let start = i;
        let mut nums: Vec<u32> = Vec::new();
        loop {
            let from = i;
            while i < chars.len() && chars[i].is_ascii_digit() {
                i += 1;
            }
            if from == i {
                break;
            }
            match chars[from..i].iter().collect::<String>().parse::<u32>() {
                Ok(n) => nums.push(n),
                Err(_) => break,
            }
            if nums.len() == 3 || i >= chars.len() || chars[i] != '.' {
                break;
            }
            i += 1;
        }
        if nums.len() >= 2 && nums[0] < 1000 {
            return Some((nums[0], nums[1], *nums.get(2).unwrap_or(&0)));
        }
        if i == start {
            i += 1;
        }
    }
    None
}

/// Everything `create` needs beyond the name.
#[derive(Clone, Default, Debug)]
pub struct CreateOpts {
    pub workspace: String,
    pub ro_mounts: Vec<String>,
    /// Extra directories mounted writable (the sandbox that fills the kit cache).
    pub rw_mounts: Vec<String>,
    /// Every kit, credentials kit first: creation applies them whole.
    pub kits: Vec<String>,
    /// `-p` specs: all-or-nothing, a busy host port 409s the whole create.
    pub publish: Vec<String>,
    pub skills: Option<String>,
    pub pull: Option<String>,
    pub env: Vec<String>,
    pub env_files: Vec<String>,
    /// A saved template (`sbx template save`): a local snapshot needs no
    /// registry round-trip, so creation skips the image pull.
    pub template: Option<String>,
}

pub fn create_args(name: &SandboxName, o: &CreateOpts) -> Vec<String> {
    let mut a: Vec<String> = vec!["create".into(), "claude".into(), o.workspace.clone(), "--name".into(), name.to_string()];
    if let Some(t) = &o.template {
        a.extend(["--template".into(), t.clone()]);
    }
    a.extend(o.rw_mounts.iter().cloned());
    a.extend(o.ro_mounts.iter().map(|m| format!("{m}:ro")));
    for k in &o.kits {
        a.extend(["--kit".into(), k.clone()]);
    }
    for p in &o.publish {
        a.extend(["-p".into(), p.clone()]);
    }
    if let Some(m) = &o.skills {
        a.push(format!("--skills={m}"));
    }
    if let Some(p) = &o.pull {
        a.push(format!("--pull={p}"));
    }
    // Files first, then -e: sbx's precedence is -e over any file whatever the
    // order, and a log line then reads the way the rule does.
    for f in &o.env_files {
        a.extend(["--env-file".into(), f.clone()]);
    }
    for e in &o.env {
        a.extend(["-e".into(), e.clone()]);
    }
    a
}

/// Re-attach the agent. The env flags apply to the agent session, which is
/// how an edited `env` reaches a sandbox that already exists.
pub fn attach_args(name: &SandboxName, env: &[String], env_files: &[String]) -> Vec<String> {
    let mut a = vec!["run".to_string(), "--name".into(), name.to_string()];
    for f in env_files {
        a.extend(["--env-file".into(), f.clone()]);
    }
    for e in env {
        a.extend(["-e".into(), e.clone()]);
    }
    a
}

pub fn bash_args(name: &SandboxName) -> Vec<String> {
    vec!["exec".into(), "-it".into(), name.to_string(), "--".into(), "bash".into()]
}

pub fn shell_args(name: &SandboxName, shell: &str) -> Vec<String> {
    vec!["exec".into(), "-it".into(), name.to_string(), "--".into(), shell.into()]
}

pub fn env_create_args(paths: &[String], name: &SandboxName, env_args: &[String]) -> Vec<String> {
    let mut a = vec!["env".to_string(), "create".into()];
    a.extend(paths.iter().cloned());
    a.extend(["--name".into(), name.to_string()]);
    for e in env_args {
        a.extend(["--env-arg".into(), e.clone()]);
    }
    a
}

pub fn policy_rule_args(decision: &str, sandbox: Option<&SandboxName>, resources: &str) -> Vec<String> {
    let mut a = vec!["policy".to_string(), decision.into(), "network".into()];
    if let Some(n) = sandbox {
        a.extend(["--sandbox".into(), n.to_string()]);
    }
    a.push(resources.into());
    a
}

pub fn policy_rm_args(rule: &str, sandbox: Option<&SandboxName>) -> Vec<String> {
    let mut a: Vec<String> = ["policy", "rm", "network", "--id", rule].map(String::from).to_vec();
    if let Some(n) = sandbox {
        a.extend(["--sandbox".into(), n.to_string()]);
    }
    a.push("--force".into());
    a
}

/// The scope of `sbx secret set`: global says nothing, a sandbox is
/// `--sandbox N` (the positional and `-g` spellings print deprecation warnings).
pub fn secret_args(service: &str, sandbox: Option<&SandboxName>) -> Vec<String> {
    let mut a = vec!["secret".to_string(), "set".into()];
    if let Some(n) = sandbox {
        a.extend(["--sandbox".into(), n.to_string()]);
    }
    a.push(service.into());
    a
}

/// Snapshot a sandbox into a local template. No `--description`: sbx 0.49
/// takes it only with `--cloud` and refuses a local save that carries it.
pub fn template_save_args(name: &SandboxName, tag: &str) -> Vec<String> {
    vec!["template".to_string(), "save".into(), name.to_string(), tag.into()]
}

pub fn prune_args(until: Option<&str>, dry_run: bool) -> Vec<String> {
    let mut a = vec!["prune".to_string(), if dry_run { "--dry-run" } else { "--force" }.into()];
    if let Some(u) = until {
        a.extend(["--filter".into(), format!("until={u}")]);
    }
    a
}

/// Did sbx refuse because it wanted a yes it could not ask for?
pub fn wants_confirmation(err: &str) -> bool {
    let e = err.to_lowercase();
    e.contains("--force") || e.contains("confirm") || e.contains("not a terminal") || e.contains("non-interactive")
}

/// The typed client.
#[derive(Clone)]
pub struct Sbx {
    runner: Arc<dyn Runner>,
}

fn s(args: &[&str]) -> Vec<String> {
    args.iter().map(|a| a.to_string()).collect()
}

impl Sbx {
    pub fn new(runner: Arc<dyn Runner>) -> Self {
        Self { runner }
    }

    pub fn process() -> Self {
        Self::new(Arc::new(runner::Process::default()))
    }

    /// Run; fail with sbx's own words.
    async fn checked(&self, inv: Invocation) -> Result<Output> {
        let args = inv.args.clone();
        let out = self.runner.run(inv).await?;
        if !out.success() {
            return Err(command_error(&args, &out));
        }
        Ok(out)
    }

    /// Run and capture. Some commands write to stderr off a TTY, so an empty
    /// stdout falls back to stderr.
    async fn capture(&self, args: Vec<String>) -> Result<String> {
        let out = self.checked(Invocation::new(args)).await?;
        if !out.stdout.is_empty() {
            return Ok(out.stdout_str());
        }
        Ok(String::from_utf8_lossy(&out.stderr).into_owned())
    }

    pub async fn raw(&self, inv: Invocation) -> Result<Output> {
        self.runner.run(inv).await
    }

    /// `sbx version`, parsed.
    pub async fn version(&self) -> Result<Option<(u32, u32, u32)>> {
        Ok(parse_version(&self.capture(s(&["version"])).await?))
    }

    /// Refuse an sbx older than the floor. An unreadable version is a warning,
    /// never a refusal: sbxw does not get to veto on output format.
    pub async fn check_version(&self, skip: bool) -> Result<Option<String>> {
        let v = self.version().await.map_err(|e| {
            anyhow!("`sbx version` failed — install the standalone sbx binary and put it on PATH ({e:#})")
        })?;
        let shown = v.map(|(a, b, c)| format!("{a}.{b}.{c}"));
        if let Some(found) = v {
            if found < MIN_VERSION && !skip {
                let (a, b, c) = found;
                let (x, y, z) = MIN_VERSION;
                bail!(
                    "this sbx is {a}.{b}.{c}, but sbxw needs {x}.{y}.{z} or newer. Upgrade sbx, or \
                     set {SKIP_VERSION_CHECK_ENV}=1 to run anyway — expect failures rather than \
                     degraded behaviour: sbxw carries no fallbacks for older releases."
                );
            }
        } else {
            tracing::warn!("could not read a version out of `sbx version` — continuing");
        }
        Ok(shown)
    }

    /// Every sandbox; empty when `sbx ls` fails (callers treat empty as
    /// "could not tell", never as "all gone"). JSON, because 0.48 reshaped the
    /// table with state marks and a creation-time column.
    pub async fn ls(&self) -> Vec<SandboxInfo> {
        match self.capture(s(&["ls", "--format", "json"])).await {
            Ok(raw) => parse_ls_json(&raw).unwrap_or_else(|| parse_ls(&raw)),
            Err(_) => self.capture(s(&["ls"])).await.map(|r| parse_ls(&r)).unwrap_or_default(),
        }
    }

    pub async fn lookup(&self, name: &SandboxName) -> Option<SandboxInfo> {
        self.ls().await.into_iter().find(|i| i.name == name.as_str())
    }

    pub async fn exists(&self, name: &SandboxName) -> bool {
        self.lookup(name).await.is_some()
    }

    pub async fn is_running(&self, name: &SandboxName) -> bool {
        self.lookup(name).await.is_some_and(|i| i.is_running())
    }

    /// Poll until running, the interval doubling from 100 ms to 500 ms.
    pub async fn wait_until_running(&self, name: &SandboxName, timeout: Duration) -> bool {
        let deadline = tokio::time::Instant::now() + timeout;
        let mut pause = Duration::from_millis(100);
        loop {
            if self.is_running(name).await {
                return true;
            }
            let now = tokio::time::Instant::now();
            if now >= deadline {
                return false;
            }
            tokio::time::sleep(pause.min(deadline - now)).await;
            pause = (pause * 2).min(Duration::from_millis(500));
        }
    }

    /// `sbx create`, its output forwarded line by line to `on_line`.
    pub async fn create(&self, name: &SandboxName, opts: &CreateOpts, on_line: Option<runner::LineSink>) -> Result<()> {
        let tail = Arc::new(std::sync::Mutex::new(std::collections::VecDeque::<String>::new()));
        let t = tail.clone();
        let sink: runner::LineSink = Arc::new(move |line: &str| {
            {
                let mut t = t.lock().unwrap();
                if t.len() == 12 {
                    t.pop_front();
                }
                t.push_back(line.to_string());
            }
            if let Some(f) = &on_line {
                f(line);
            }
        });
        let args = create_args(name, opts);
        let out = self
            .runner
            .run(Invocation {
                args: args.clone(),
                stdin: None,
                on_line: Some(sink),
            })
            .await?;
        if !out.success() {
            let said = tail.lock().unwrap().iter().cloned().collect::<Vec<_>>().join("\n");
            let out = Output { stderr: said.into_bytes(), ..out };
            return Err(command_error(&args, &out));
        }
        Ok(())
    }

    pub async fn run_detached(&self, name: &SandboxName) -> Result<()> {
        self.capture(s(&["run", "-d", "--name", name.as_str()])).await.map(drop)
    }

    pub async fn stop(&self, name: &SandboxName) -> Result<()> {
        self.checked(Invocation::new(s(&["stop", name.as_str()]))).await.map(drop)
    }

    /// `--force`: sbxw's own daemon keeps a session attached, and an unforced
    /// rm refuses a sandbox in use. One result per target, so a partial
    /// failure still lets the caller forget the sandboxes that did go; an
    /// error only when sbx gave no per-target answer.
    pub async fn rm(&self, names: &[SandboxName]) -> Result<Vec<RmResult>> {
        let mut a = s(&["rm", "--force", "--format", "json"]);
        a.extend(names.iter().map(|n| n.to_string()));
        let out = self.runner.run(Invocation::new(a.clone())).await?;
        if let Some(results) = parse_rm_json(&out.stdout_str()).filter(|r| !r.is_empty()) {
            return Ok(results);
        }
        if !out.success() {
            return Err(command_error(&a, &out));
        }
        Ok(names
            .iter()
            .map(|n| RmResult { name: n.to_string(), removed: true, error: None })
            .collect())
    }

    pub async fn prune(&self, until: Option<&str>, dry_run: bool) -> Result<String> {
        self.capture(prune_args(until, dry_run)).await
    }

    /// The live mappings, from the documented 0.48 JSON rows (the table, if
    /// the output is not JSON).
    pub async fn ports(&self, name: &SandboxName) -> Vec<PortMapping> {
        self.capture(s(&["ports", name.as_str(), "--format", "json"]))
            .await
            .map(|r| sbxw_core::ports::parse_ports_json(&r).unwrap_or_else(|| sbxw_core::ports::parse_ports(&r)))
            .unwrap_or_default()
    }

    pub async fn ports_raw(&self, name: &SandboxName) -> Result<String> {
        self.capture(s(&["ports", name.as_str()])).await
    }

    pub async fn publish(&self, name: &SandboxName, spec: &str) -> Result<()> {
        self.checked(Invocation::new(s(&["ports", name.as_str(), "--publish", spec]))).await.map(drop)
    }

    pub async fn unpublish(&self, name: &SandboxName, spec: &str) -> Result<()> {
        self.checked(Invocation::new(s(&["ports", name.as_str(), "--unpublish", spec]))).await.map(drop)
    }

    /// `None` writes a host-wide rule — every sandbox, present and future.
    pub async fn policy(&self, decision: &str, sandbox: Option<&SandboxName>, resources: &str) -> Result<()> {
        self.checked(Invocation::new(policy_rule_args(decision, sandbox, resources))).await.map(drop)
    }

    /// Remove one rule by **rule** id: tried in the sandbox's policy first,
    /// then the global one (the panel lists both). A rule id is a UUID, so the
    /// second attempt cannot hit a different rule.
    pub async fn policy_rm(&self, rule: &str, sandbox: Option<&SandboxName>) -> Result<()> {
        let first = self.checked(Invocation::new(policy_rm_args(rule, sandbox))).await;
        match (first, sandbox) {
            (Ok(_), _) => Ok(()),
            (Err(scoped), Some(n)) => self
                .checked(Invocation::new(policy_rm_args(rule, None)))
                .await
                .map(drop)
                .map_err(|global| anyhow!("{scoped:#}\n\nnor in the global policy (tried after sandbox '{n}'s): {global:#}")),
            (Err(e), None) => Err(e),
        }
    }

    /// `policy ls`/`log` for a sandbox (positional), retried unscoped so the
    /// panel degrades to host-wide rules — flagged as such — not to an error.
    async fn policy_view(&self, scoped: Vec<String>, unscoped: Vec<String>, sandbox: &SandboxName) -> PolicyView {
        match self.capture(scoped).await {
            Ok(raw) => PolicyView::build(&raw, true, sandbox),
            Err(scoped_err) => match self.capture(unscoped).await {
                Ok(raw) => PolicyView::build(&raw, false, sandbox),
                Err(_) => PolicyView::failed(format!("{scoped_err:#}")),
            },
        }
    }

    pub async fn policy_ls(&self, sandbox: &SandboxName, wide: bool) -> PolicyView {
        let mut scoped = s(&["policy", "ls", sandbox.as_str()]);
        let mut unscoped = s(&["policy", "ls"]);
        if wide {
            scoped.push("--wide".into());
            unscoped.push("--wide".into());
        }
        self.policy_view(scoped, unscoped, sandbox).await
    }

    /// What the sandbox reached and was refused, from the 0.48 JSON document;
    /// `None` beside a table view when sbx gave one instead.
    pub async fn policy_log(&self, sandbox: &SandboxName) -> (PolicyView, Option<EgressLog>) {
        if let Ok(raw) = self.capture(s(&["policy", "log", sandbox.as_str(), "--format", "json"])).await {
            if let Some(log) = EgressLog::parse(&raw) {
                return (log.view(), Some(log));
            }
        }
        (self.policy_view(s(&["policy", "log", sandbox.as_str()]), s(&["policy", "log"]), sandbox).await, None)
    }

    /// `sbx policy refresh` (0.49): re-read policy now, e.g. after an AI
    /// Governance change, instead of at the next poll. Connections stay open.
    pub async fn policy_refresh(&self) -> Result<()> {
        self.checked(Invocation::new(s(&["policy", "refresh"]))).await.map(drop)
    }

    /// The MCP servers registered on this host.
    pub async fn mcp_ls(&self) -> Result<Vec<McpServer>> {
        let raw = self.capture(s(&["mcp", "ls", "--format", "json"])).await?;
        parse_mcp_ls(&raw).ok_or_else(|| anyhow!("`sbx mcp ls --format json` gave no JSON array:\n{}", raw.trim()))
    }

    /// Detach a loaded MCP server from one sandbox without restarting its agent (0.48).
    pub async fn mcp_unload(&self, server: &str, sandbox: &SandboxName) -> Result<()> {
        self.checked(Invocation::new(s(&["mcp", "unload", server, "--sandbox", sandbox.as_str()]))).await.map(drop)
    }

    /// Check a local kit (directory or ZIP) without the daemon or a sign-in
    /// (0.49), so a broken kit fails in a second rather than mid-create.
    pub async fn kit_validate_offline(&self, kit: &str) -> Result<()> {
        self.checked(Invocation::new(s(&["kit", "validate", kit, "--offline"]))).await.map(drop)
    }

    /// Snapshot a sandbox from the daemon, where no terminal can answer a
    /// prompt: a save refused for want of one (sbx asking to stop the
    /// sandbox first) is retried after stopping it. True when it was stopped.
    pub async fn template_save(&self, name: &SandboxName, tag: &str) -> Result<bool> {
        let args = template_save_args(name, tag);
        let Err(first) = self.checked(Invocation::new(args.clone())).await else {
            return Ok(false);
        };
        let said = format!("{first:#}").to_lowercase();
        if !(wants_confirmation(&said) || said.contains("stop") || said.contains("running")) {
            return Err(first);
        }
        self.stop(name).await?;
        self.checked(Invocation::new(args))
            .await
            .map(|_| true)
            .map_err(|e| anyhow!("{e:#}\n(retried after stopping '{name}'; first attempt: {first:#})"))
    }

    /// The saved templates. `--format json` first; an sbx that refuses the
    /// flag is asked for its table.
    pub async fn template_ls(&self) -> Result<Vec<TemplateInfo>> {
        let raw = match self.capture(s(&["template", "ls", "--format", "json"])).await {
            Ok(raw) => raw,
            Err(_) => self.capture(s(&["template", "ls"])).await?,
        };
        Ok(parse_template_ls(&raw))
    }

    /// Remove a template; a refusal for want of a confirmation is retried
    /// with `--force`, as every sbx removal asks since 0.45.
    pub async fn template_rm(&self, tag: &str) -> Result<()> {
        let Err(first) = self.checked(Invocation::new(s(&["template", "rm", tag]))).await else {
            return Ok(());
        };
        if !wants_confirmation(&format!("{first:#}")) {
            return Err(first);
        }
        self.checked(Invocation::new(s(&["template", "rm", tag, "--force"]))).await.map(drop)
    }

    /// `sbx diagnose --format json`, unparsed: its shape is sbx's to change.
    pub async fn diagnose(&self) -> Result<String> {
        self.capture(s(&["diagnose", "--format", "json"])).await
    }

    /// A secret by stdin, never argv. Replacing one may ask to confirm, and
    /// since 0.48 no terminal is a refusal: that refusal is retried with
    /// `--force` — the caller asked to store this value.
    pub async fn secret_set(&self, service: &str, value: &str, sandbox: Option<&SandboxName>) -> Result<()> {
        let inv = |args: Vec<String>| Invocation {
            args,
            stdin: Some(value.as_bytes().to_vec()),
            on_line: None,
        };
        let Err(first) = self.checked(inv(secret_args(service, sandbox))).await else {
            return Ok(());
        };
        if !wants_confirmation(&format!("{first:#}")) {
            return Err(first);
        }
        let mut forced = secret_args(service, sandbox);
        forced.push("--force".into());
        self.checked(inv(forced))
            .await
            .map(drop)
            .map_err(|e| anyhow!("{e:#}\n(retried with --force after: {first:#})"))
    }

    pub async fn kit_add(&self, name: &SandboxName, kit: &str) -> Result<()> {
        self.checked(Invocation::new(s(&["kit", "add", name.as_str(), kit]))).await.map(drop)
    }

    pub async fn inspect(&self, name: &SandboxName) -> Result<String> {
        self.capture(s(&["inspect", name.as_str()])).await
    }

    /// Run `args` in the sandbox; the raw output whatever the status.
    pub async fn exec(&self, name: &SandboxName, args: &[&str], stdin: Option<Vec<u8>>) -> Result<Output> {
        let mut a = vec!["exec".to_string()];
        if stdin.is_some() {
            // -i without -t: a PTY would translate newlines and corrupt data.
            a.push("-i".into());
        }
        a.extend([name.to_string(), "--".into()]);
        a.extend(args.iter().map(|x| x.to_string()));
        self.runner.run(Invocation { args: a, stdin, on_line: None }).await
    }

    /// Write `data` to `dest` in the sandbox; the path is `$1`, never script.
    pub async fn write_file(&self, name: &SandboxName, dest: &str, data: &[u8], private: bool) -> Result<()> {
        let script = if private {
            r#"umask 077 && mkdir -p "$(dirname "$1")" && cat > "$1" && chmod 600 "$1""#
        } else {
            r#"mkdir -p "$(dirname "$1")" && cat > "$1""#
        };
        let out = self.exec(name, &["sh", "-c", script, "sh", dest], Some(data.to_vec())).await?;
        if !out.success() {
            bail!("`sbx exec` write to {dest} failed: {}", String::from_utf8_lossy(&out.stderr).trim());
        }
        Ok(())
    }

    pub async fn file_exists(&self, name: &SandboxName, path: &str) -> bool {
        self.exec(name, &["test", "-f", path], None).await.is_ok_and(|o| o.success())
    }

    /// Fetch a URL *from inside* the sandbox: its proxy authenticates the
    /// request with the account's own credentials at full scope.
    pub async fn curl(&self, name: &SandboxName, url: &str) -> Result<String> {
        let out = self.exec(name, &["curl", "-s", "--max-time", "10", url], None).await?;
        if !out.success() {
            bail!("curl in '{name}' failed: {}", String::from_utf8_lossy(&out.stderr).trim());
        }
        Ok(out.stdout_str())
    }

    /// Run a tooling batch in one `sbx exec`.
    pub async fn tooling(&self, name: &SandboxName, batch: &Tooling) -> Result<ToolingOutcome> {
        let out = self.exec(name, &["node", "-"], Some(batch.script().into_bytes())).await?;
        match batch.parse_report(&out.stdout_str()) {
            Some(outcome) if out.success() => Ok(outcome),
            _ => Err(command_error(&s(&["exec", name.as_str(), "--", "node", "-"]), &out)),
        }
    }

    async fn thread_script(&self, args: Vec<String>) -> Result<std::collections::HashMap<String, String>> {
        let out = self.runner.run(Invocation::new(args.clone())).await?;
        let fields = scripts::script_fields(&out.stdout_str()).map_err(|e| anyhow!(e))?;
        if !out.success() {
            return Err(command_error(&args, &out));
        }
        Ok(fields)
    }

    pub async fn thread_status(&self, name: &SandboxName, dir: &str, thread: &ThreadName) -> Result<ThreadStatus> {
        let f = self.thread_script(scripts::thread_end_args(name, dir, thread, "status", false, false)).await?;
        Ok(ThreadStatus::from_fields(&f))
    }

    pub async fn thread_close(&self, name: &SandboxName, dir: &str, thread: &ThreadName, discard: bool, delete_branch: bool) -> Result<()> {
        self.thread_script(scripts::thread_end_args(name, dir, thread, "close", discard, delete_branch)).await.map(drop)
    }

    /// Merge a thread's branch into its repo's checked-out branch; the commit.
    pub async fn thread_merge(&self, name: &SandboxName, dir: &str, thread: &ThreadName) -> Result<String> {
        let f = self.thread_script(scripts::thread_merge_args(name, dir, thread)).await?;
        Ok(f.get("commit").cloned().unwrap_or_default())
    }

    pub async fn setup_ssh(&self) -> Result<()> {
        self.checked(Invocation::new(s(&["setup", "ssh"]))).await.map(drop)
    }
}

/// Run `sbx args…` on this process's own terminal — for the commands that are
/// a terminal session (`sbxw bash`, `--no-web`, `sbx env create`'s question).
pub fn interactive(args: &[String]) -> Result<()> {
    let status = std::process::Command::new("sbx")
        .args(args)
        .status()
        .map_err(|e| anyhow!("failed to spawn `sbx` — is it on your PATH? ({e})"))?;
    if !status.success() {
        bail!("`sbx {}` exited with {status}", args.iter().take(3).cloned().collect::<Vec<_>>().join(" "));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use runner::Fake;

    fn sb(n: &str) -> SandboxName {
        SandboxName::parse(n).unwrap()
    }

    #[test]
    fn version_is_found_however_sbx_frames_it() {
        assert_eq!(parse_version("sbx version 0.47.1"), Some((0, 47, 1)));
        assert_eq!(parse_version("Version: v0.50"), Some((0, 50, 0)));
        assert_eq!(parse_version("built 2026-01-02 sha256 abc, 1.2.3-rc1"), Some((1, 2, 3)));
        assert_eq!(parse_version("nothing"), None);
    }

    #[test]
    fn the_ls_table_is_read_by_its_headers() {
        let raw = "SANDBOX   AGENT    STATUS                 WORKSPACE\n\
                   neos      claude   running                /w\n\
                   old       claude   running (unresponsive) /x\n\
                   gone      claude   stopped                /y\n";
        let l = parse_ls(raw);
        assert_eq!(l.len(), 3);
        assert!(l[0].is_running());
        assert!(!l[1].is_running(), "a hung guest is not running");
        assert!(l[2].is_stopped());
        assert_eq!(parse_ls("x\na b c\n")[0].name, "a", "positional fallback");
        let marked = parse_ls("  SANDBOX   AGENT    STATUS              CREATED\n● neos      claude   ● running           2h ago\n");
        assert_eq!(marked[0].name, "neos", "0.48 state marks are not names");
        assert!(marked[0].is_running());
    }

    #[test]
    fn the_ls_json_is_a_bare_array() {
        let raw = r#"[{"name":"neos","agent":"claude","status":"running","created_at":"2026-10-20T10:00:00Z","detached":false},
                     {"name":"hung","agent":"claude","status":"running (not responding)"},
                     {"name":"flagged","agent":"claude","status":"running","responsive":false},
                     {"name":"off","agent":"claude","state":"stopped"}]"#;
        let l = parse_ls_json(raw).unwrap();
        assert_eq!(l.len(), 4);
        assert!(l[0].is_running());
        assert!(!l[1].is_running() && !l[2].is_running(), "a guest that stopped answering is not running");
        assert!(l[3].is_stopped());
        assert!(parse_ls_json("SANDBOX  AGENT  STATUS\n").is_none(), "a table is not JSON");
    }

    #[test]
    fn template_ls_is_read_as_json_or_by_its_headers() {
        let json = r#"[{"tag":"me/base:1","size":"1.2GB","created_at":"2026-10-20T10:00:00Z"},{"repository":"sbxw-auto","tag":"abc-2.1.3-0.49.1"}]"#;
        let l = parse_template_ls(json);
        assert_eq!(l.iter().map(|t| t.tag.as_str()).collect::<Vec<_>>(), ["me/base:1", "sbxw-auto:abc-2.1.3-0.49.1"]);
        assert_eq!(l[0].size.as_deref(), Some("1.2GB"));
        let table = "REPOSITORY   TAG               SIZE    CREATED\nme/base      1                 1.2GB   2 days ago\n";
        let l = parse_template_ls(table);
        assert_eq!(l, [TemplateInfo { tag: "me/base:1".into(), size: Some("1.2GB".into()), created: Some("2 days ago".into()) }]);
        assert_eq!(parse_template_ls("TAG          SIZE\nme/base:1    1GB\n")[0].tag, "me/base:1");
        assert!(parse_template_ls("No templates.\n").is_empty());
        assert!(parse_template_ls("[]").is_empty());
    }

    #[test]
    fn rm_results_keep_the_failures() {
        let raw = r#"[{"action":"removed","resource":"sandbox","name":"a"},
                     {"action":"failed","resource":"sandbox","name":"b","error":"no such sandbox"}]"#;
        let r = parse_rm_json(raw).unwrap();
        assert!(r[0].removed);
        assert!(!r[1].removed);
        assert_eq!(r[1].error.as_deref(), Some("no such sandbox"));
    }

    #[tokio::test]
    async fn a_partial_rm_still_names_what_went() {
        let fake = Fake::new(|_| Output {
            code: Some(1),
            stdout: br#"[{"action":"removed","name":"a"},{"action":"failed","name":"b","error":"busy"}]"#.to_vec(),
            stderr: vec![],
        });
        let r = Sbx::new(fake.clone()).rm(&[sb("a"), sb("b")]).await.unwrap();
        assert_eq!(r.iter().filter(|r| r.removed).count(), 1);
        assert_eq!(fake.log(), ["rm --force --format json a b"]);
    }

    #[test]
    fn create_argv_has_every_flag_in_order() {
        let a = create_args(
            &sb("neos"),
            &CreateOpts {
                workspace: "/w".into(),
                ro_mounts: vec!["/ro".into()],
                kits: vec!["/k".into()],
                publish: vec!["4200:4200".into()],
                skills: Some("off".into()),
                pull: Some("missing".into()),
                env: vec!["A=1".into()],
                env_files: vec!["/e".into()],
                rw_mounts: vec!["/rw".into()],
                template: Some("me/base:1".into()),
            },
        );
        assert_eq!(
            a.join(" "),
            "create claude /w --name neos --template me/base:1 /rw /ro:ro --kit /k -p 4200:4200 --skills=off --pull=missing --env-file /e -e A=1"
        );
    }

    #[test]
    fn small_argvs() {
        assert_eq!(attach_args(&sb("n"), &["A=1".into()], &[]).join(" "), "run --name n -e A=1");
        assert_eq!(policy_rule_args("deny", Some(&sb("n")), "x.com").join(" "), "policy deny network --sandbox n x.com");
        assert_eq!(policy_rm_args("r-1", None).join(" "), "policy rm network --id r-1 --force");
        assert_eq!(secret_args("anthropic", None).join(" "), "secret set anthropic");
        assert_eq!(template_save_args(&sb("n"), "me/base:1").join(" "), "template save n me/base:1");
        assert_eq!(prune_args(Some("168h"), false).join(" "), "prune --force --filter until=168h");
        assert_eq!(env_create_args(&["/d".into()], &sb("n"), &["p=1".into()]).join(" "), "env create /d --name n --env-arg p=1");
    }

    #[tokio::test]
    async fn a_failure_carries_what_sbx_said() {
        let fake = Fake::new(|_| Output::fail(1, "ERROR: Blocked by network policy: rule x"));
        let sbx = Sbx::new(fake.clone());
        let e = sbx.stop(&sb("n")).await.unwrap_err();
        assert!(format!("{e:#}").contains("Blocked by network policy"));
    }

    #[tokio::test]
    async fn policy_rm_falls_back_to_the_global_policy() {
        let fake = Fake::new(|a| {
            if a.contains(&"--sandbox".to_string()) {
                Output::fail(1, "no such rule")
            } else {
                Output::ok("")
            }
        });
        Sbx::new(fake.clone()).policy_rm("r-1", Some(&sb("n"))).await.unwrap();
        assert_eq!(fake.log().len(), 2);
    }

    #[tokio::test]
    async fn the_policy_view_degrades_to_unscoped_and_says_so() {
        let fake = Fake::new(|a| {
            if a.len() == 3 {
                Output::fail(1, "unknown sandbox")
            } else {
                Output::ok("POLICY   SOURCE   APPLIES TO   SUMMARY\np1       local    all          1 rule\n")
            }
        });
        let v = Sbx::new(fake).policy_ls(&sb("n"), false).await;
        assert!(!v.sandbox_scoped);
        assert_eq!(v.rows.len(), 1);
    }

    #[tokio::test]
    async fn secrets_travel_on_stdin() {
        let fake = Fake::new(|_| Output::ok(""));
        Sbx::new(fake.clone()).secret_set("anthropic", "sk-123", None).await.unwrap();
        assert_eq!(fake.log(), ["secret set anthropic"]);
        assert_eq!(fake.stdins.lock().unwrap()[0], b"sk-123");
    }

    #[tokio::test]
    async fn a_replaced_secret_is_confirmed_with_force() {
        let fake = Fake::new(|a| {
            if a.contains(&"--force".to_string()) {
                Output::ok("")
            } else {
                Output::fail(1, "secret exists; confirmation required (stdin is not a terminal) — use --force")
            }
        });
        Sbx::new(fake.clone()).secret_set("anthropic", "sk-123", None).await.unwrap();
        assert_eq!(fake.log(), ["secret set anthropic", "secret set anthropic --force"]);
        assert_eq!(fake.stdins.lock().unwrap()[1], b"sk-123", "the value travels on stdin both times");
    }

    #[tokio::test]
    async fn the_version_floor_is_enforced() {
        let old = Sbx::new(Fake::new(|_| Output::ok("sbx 0.47.9")));
        assert!(old.check_version(false).await.is_err());
        assert!(old.check_version(true).await.is_ok());
        let new = Sbx::new(Fake::new(|_| Output::ok("sbx 0.48.0")));
        assert_eq!(new.check_version(false).await.unwrap().as_deref(), Some("0.48.0"));
    }

    #[tokio::test]
    async fn a_thread_script_error_line_wins() {
        let fake = Fake::new(|_| Output { code: Some(4), stdout: b"error=has 2 uncommitted file(s)\n".to_vec(), stderr: vec![] });
        let e = Sbx::new(fake)
            .thread_close(&sb("n"), "/w", &ThreadName::parse("a").unwrap(), false, false)
            .await
            .unwrap_err();
        assert_eq!(format!("{e}"), "has 2 uncommitted file(s)");
    }
}
