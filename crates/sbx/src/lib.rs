//! The only door to the `sbx` CLI. sbxw never calls `docker sandbox`.
//!
//! Written for sbx 0.47 and newer, with no fallbacks below it: since 0.39 an
//! unrecognised command or flag is an *error*, so a command that succeeded
//! really ran, and a fallback path would be code nobody runs.
//!
//! ```text
//! sbx create claude PATH --name N [PATH:ro]… [--kit K]… [-p SPEC]…
//!            [--skills=off|readonly|readwrite] [--pull=always|missing|never]
//!            [--env-file F]… [-e K=V]…        (creation: the only moment kits apply whole;
//!                                              -p is all-or-nothing)
//! sbx run --name N [--env-file F]… [-e K=V]…  (attach; -e applies to the agent session)
//! sbx run -d --name N                         (keep running with nobody attached)
//! sbx ls                                      (read by column headers)
//! sbx exec [-i|-it] N -- cmd…                 (only a *running* sandbox; -d fails)
//! sbx ports N [--publish|--unpublish SPEC]
//! sbx policy allow|deny network [--sandbox N] RESOURCES
//! sbx policy ls [N] [--wide] · sbx policy log [N]   (N positional here)
//! sbx policy rm network --id RULE_ID [--sandbox N] --force
//! sbx secret set [--sandbox N] SERVICE        (value on stdin)
//! sbx kit add N REF                           (recreates the container)
//! sbx env create PATH… --name N [--env-arg K=V]…   (shows a plan and asks)
//! sbx prune [--dry-run] [--filter until=D] --force
//! sbx rm --force (N…|--all) · sbx stop N · sbx skills import · sbx setup ssh
//! ```
//!
//! Facts the methods below are built on:
//! * every removal asks for confirmation since 0.45, and a prompt with no
//!   terminal is a refusal — sbxw passes `--force` and does its own asking;
//! * a sandbox made with `sbx create` stops by itself once idle;
//! * a sandbox whose guest stopped responding is not running;
//! * `policy allow|deny` refused with "managed by your organization" is a
//!   governed host, not a broken one.

pub mod runner;
pub mod scripts;

use anyhow::{anyhow, bail, Result};
use runner::{Invocation, Output, Runner};
use sbxw_core::policy::PolicyView;
use sbxw_core::ports::PortMapping;
use sbxw_core::table;
use sbxw_core::tooling::{Tooling, ToolingOutcome};
use sbxw_core::{SandboxName, ThreadName};
use serde::Serialize;
use std::sync::Arc;
use std::time::Duration;

pub use scripts::ThreadStatus;

/// Oldest sbx sbxw works against, and the one it is written for.
pub const MIN_VERSION: (u32, u32, u32) = (0, 47, 0);
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
}

impl SandboxInfo {
    /// Up *and answering*: a hung guest reported as running is not.
    pub fn is_running(&self) -> bool {
        let s = self.status.to_ascii_lowercase();
        s.split_whitespace().next() == Some("running") && !s.contains("unresponsive")
    }
    pub fn is_stopped(&self) -> bool {
        self.status.to_ascii_lowercase().split_whitespace().next() == Some("stopped")
    }
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
                    let name = cells.first()?.split_whitespace().next()?.to_string();
                    let cell = |i: Option<usize>| i.and_then(|i| cells.get(i)).cloned().unwrap_or_default();
                    let status = cell(Some(status_col));
                    Some(SandboxInfo {
                        name,
                        agent: cell(agent_col),
                        status: if status.is_empty() { "unknown".into() } else { status },
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
    /// Every kit, credentials kit first: creation applies them whole.
    pub kits: Vec<String>,
    /// `-p` specs: all-or-nothing, a busy host port 409s the whole create.
    pub publish: Vec<String>,
    pub skills: Option<String>,
    pub pull: Option<String>,
    pub env: Vec<String>,
    pub env_files: Vec<String>,
}

pub fn create_args(name: &SandboxName, o: &CreateOpts) -> Vec<String> {
    let mut a: Vec<String> = vec!["create".into(), "claude".into(), o.workspace.clone(), "--name".into(), name.to_string()];
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

pub fn prune_args(until: Option<&str>, dry_run: bool) -> Vec<String> {
    let mut a = vec!["prune".to_string(), if dry_run { "--dry-run" } else { "--force" }.into()];
    if let Some(u) = until {
        a.extend(["--filter".into(), format!("until={u}")]);
    }
    a
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
    /// "could not tell", never as "all gone").
    pub async fn ls(&self) -> Vec<SandboxInfo> {
        self.capture(s(&["ls"])).await.map(|r| parse_ls(&r)).unwrap_or_default()
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
    pub async fn create(&self, name: &SandboxName, opts: &CreateOpts, on_line: Option<Arc<dyn Fn(&str) + Send + Sync>>) -> Result<()> {
        let tail = Arc::new(std::sync::Mutex::new(std::collections::VecDeque::<String>::new()));
        let t = tail.clone();
        let sink: Arc<dyn Fn(&str) + Send + Sync> = Arc::new(move |line: &str| {
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
    /// rm refuses a sandbox in use.
    pub async fn rm(&self, names: &[SandboxName], all: bool) -> Result<()> {
        let mut a = s(&["rm", "--force"]);
        if all {
            a.push("--all".into());
        } else {
            a.extend(names.iter().map(|n| n.to_string()));
        }
        self.checked(Invocation::new(a)).await.map(drop)
    }

    pub async fn prune(&self, until: Option<&str>, dry_run: bool) -> Result<String> {
        self.capture(prune_args(until, dry_run)).await
    }

    pub async fn ports(&self, name: &SandboxName) -> Vec<PortMapping> {
        self.ports_raw(name)
            .await
            .map(|r| sbxw_core::ports::parse_ports(&r))
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

    pub async fn policy_log(&self, sandbox: &SandboxName) -> PolicyView {
        self.policy_view(s(&["policy", "log", sandbox.as_str()]), s(&["policy", "log"]), sandbox).await
    }

    /// A secret by stdin, never argv.
    pub async fn secret_set(&self, service: &str, value: &str, sandbox: Option<&SandboxName>) -> Result<()> {
        self.checked(Invocation {
            args: secret_args(service, sandbox),
            stdin: Some(value.as_bytes().to_vec()),
            on_line: None,
        })
        .await
        .map(drop)
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
            },
        );
        assert_eq!(
            a.join(" "),
            "create claude /w --name neos /ro:ro --kit /k -p 4200:4200 --skills=off --pull=missing --env-file /e -e A=1"
        );
    }

    #[test]
    fn small_argvs() {
        assert_eq!(attach_args(&sb("n"), &["A=1".into()], &[]).join(" "), "run --name n -e A=1");
        assert_eq!(policy_rule_args("deny", Some(&sb("n")), "x.com").join(" "), "policy deny network --sandbox n x.com");
        assert_eq!(policy_rm_args("r-1", None).join(" "), "policy rm network --id r-1 --force");
        assert_eq!(secret_args("anthropic", None).join(" "), "secret set anthropic");
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
    async fn the_version_floor_is_enforced() {
        let old = Sbx::new(Fake::new(|_| Output::ok("sbx 0.46.9")));
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
