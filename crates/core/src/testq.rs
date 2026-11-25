//! The test queue's rules: the `[[tests]]` suites of `sbxw.toml`, the key a
//! result is stored under, and the book of jobs and results.
//!
//! * A suite is declared by the person (the file is agent-writable, so its
//!   commands are approved at `sbxw up`, see `consent::grants`) and only
//!   ever NAMED by an agent. No route takes a command string.
//! * A job tests a commit. Its key is the suite's declaration, the git object
//!   of every path the suite and the suites it depends on cover, at that
//!   commit, and the toolchain version its `version_cmd` prints. The same key
//!   is the same code under the same command: the stored result is answered
//!   at once, marked as reused, with the commit it was first run on.
//! * One job runs at a time per sandbox; a request for a key already queued
//!   or running joins that job instead of queueing a second one.
//! * Only a suite that ran to its end (passed or failed) is stored for reuse.
//!   A time-out or a run that could not start is answered, never reused.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// The default time limit of a suite, in minutes.
pub const DEFAULT_TIMEOUT_MINUTES: u32 = 30;
/// The longest a suite may be given.
pub const MAX_TIMEOUT_MINUTES: u32 = 240;
/// Lines of output kept with a result.
pub const TAIL_LINES: usize = 40;
/// Bytes of output kept with a result, at most.
pub const TAIL_BYTES: usize = 6000;
/// Stored results older than this are dropped.
pub const RESULT_TTL_MS: u64 = 30 * 24 * 3600 * 1000;
/// Stored results per sandbox, at most (the oldest go first).
pub const MAX_RESULTS: usize = 100;
/// Finished jobs are kept this long, so `check_tests` can still read them.
pub const JOB_TTL_MS: u64 = 7 * 24 * 3600 * 1000;
/// Finished jobs per sandbox, at most.
pub const MAX_DONE_JOBS: usize = 200;

/// One `[[tests]]` table.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct Suite {
    /// What an agent names: lower-case letters, digits and hyphens.
    pub name: String,
    /// Run with `bash -c` inside the sandbox, in the queue's checkout of the
    /// commit. It gets `SBXW_TEST_TARGET` (a build directory kept from job to
    /// job), `SBXW_TEST_CHECKOUT`, `SBXW_TEST_SUITE` and `SBXW_TEST_COMMIT`.
    pub command: String,
    /// Paths the suite covers, relative to the repository root; `.` is the
    /// whole repository. Lock files the suite depends on belong here too.
    pub paths: Vec<String>,
    /// Suites whose paths this one also covers (a crate and the crates it
    /// depends on).
    #[serde(default)]
    pub depends: Vec<String>,
    #[serde(default = "default_timeout")]
    pub timeout_minutes: u32,
    /// 0 or 1: run a failed suite once more. The result says it was retried.
    #[serde(default)]
    pub retries: u8,
    /// A command printing the toolchain's version (`rustc --version`): part
    /// of the key, so a new toolchain runs the suite again. Empty: none.
    #[serde(default)]
    pub version_cmd: String,
}

fn default_timeout() -> u32 {
    DEFAULT_TIMEOUT_MINUTES
}

fn one_line_command(what: &str, s: &str) -> Result<(), String> {
    if s.contains('\0') || s.contains('\n') || s.contains('\r') {
        return Err(format!("{what}: one line, no control characters"));
    }
    Ok(())
}

/// `.`, `crates/core`, `Cargo.lock`: relative, inside the repository, one
/// line, never read as a git option.
fn path_error(p: &str) -> Option<&'static str> {
    if p.trim().is_empty() {
        Some("an empty path")
    } else if p.starts_with('/') {
        Some("paths are relative to the repository root")
    } else if p.starts_with('-') {
        Some("a path may not start with '-'")
    } else if p.chars().any(|c| c.is_control()) {
        Some("one path, on one line")
    } else if p.split('/').any(|seg| seg == "..") {
        Some("no `..` in a path")
    } else if p.contains(':') {
        Some("no ':' in a path (git would read it as a revision)")
    } else {
        None
    }
}

/// A path as the key reads it: no `./`, no trailing `/`; the root is `.`.
pub fn normal_path(p: &str) -> String {
    let mut s = p.trim().trim_end_matches('/');
    while let Some(rest) = s.strip_prefix("./") {
        s = rest;
    }
    if s.is_empty() { ".".into() } else { s.to_string() }
}

/// Every check `Config::validate` makes of `[[tests]]`.
pub fn validate(suites: &[Suite]) -> Result<(), String> {
    let mut names = BTreeSet::new();
    for s in suites {
        if !crate::names::is_valid_command_name(&s.name) {
            return Err(format!("tests: {:?} is not a suite name (lower-case letters, digits and hyphens, at most 64)", s.name));
        }
        if !names.insert(s.name.as_str()) {
            return Err(format!("tests: the suite {:?} is declared twice", s.name));
        }
        if s.command.trim().is_empty() {
            return Err(format!("tests: suite {:?} has no command", s.name));
        }
        one_line_command(&format!("tests: suite {:?} command", s.name), &s.command)?;
        one_line_command(&format!("tests: suite {:?} version_cmd", s.name), &s.version_cmd)?;
        if s.paths.is_empty() {
            return Err(format!("tests: suite {:?} names no paths; `.` covers the whole repository", s.name));
        }
        for p in &s.paths {
            if let Some(why) = path_error(p) {
                return Err(format!("tests: suite {:?} path {p:?} — {why}", s.name));
            }
        }
        if s.timeout_minutes == 0 || s.timeout_minutes > MAX_TIMEOUT_MINUTES {
            return Err(format!("tests: suite {:?} timeout_minutes = {} — expected 1 to {MAX_TIMEOUT_MINUTES}", s.name, s.timeout_minutes));
        }
        if s.retries > 1 {
            return Err(format!("tests: suite {:?} retries = {} — expected 0 or 1", s.name, s.retries));
        }
    }
    for s in suites {
        for d in &s.depends {
            if d == &s.name {
                return Err(format!("tests: suite {:?} depends on itself", s.name));
            }
            if !names.contains(d.as_str()) {
                return Err(format!("tests: suite {:?} depends on {d:?}, which is not declared", s.name));
            }
        }
    }
    for s in suites {
        if closure_names(suites, &s.name).is_err() {
            return Err(format!("tests: suite {:?} is part of a dependency cycle", s.name));
        }
    }
    Ok(())
}

/// `name` and every suite it depends on, directly or not; `Err` on a cycle.
fn closure_names(suites: &[Suite], name: &str) -> Result<Vec<String>, ()> {
    fn walk(suites: &[Suite], name: &str, path: &mut Vec<String>, out: &mut BTreeSet<String>) -> Result<(), ()> {
        if path.iter().any(|p| p == name) {
            return Err(());
        }
        if !out.insert(name.to_string()) {
            return Ok(());
        }
        path.push(name.to_string());
        if let Some(s) = suites.iter().find(|s| s.name == name) {
            for d in &s.depends {
                walk(suites, d, path, out)?;
            }
        }
        path.pop();
        Ok(())
    }
    let mut out = BTreeSet::new();
    walk(suites, name, &mut vec![], &mut out)?;
    Ok(out.into_iter().collect())
}

/// The paths a suite's key covers: its own and its dependencies', each once,
/// sorted.
pub fn covered_paths(suites: &[Suite], name: &str) -> Vec<String> {
    let names = closure_names(suites, name).unwrap_or_else(|_| vec![name.to_string()]);
    let set: BTreeSet<String> = suites
        .iter()
        .filter(|s| names.contains(&s.name))
        .flat_map(|s| s.paths.iter().map(|p| normal_path(p)))
        .collect();
    set.into_iter().collect()
}

/// What the sandbox said about a commit: its full hash, the git object of
/// each covered path there (`-` when absent), and the toolchain version.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Probe {
    pub commit: String,
    pub objects: BTreeMap<String, String>,
    pub version: String,
}

/// The key-script's output: `commit=<sha>`, `obj=<hash> <path>` lines, then
/// `version=<text>` (the rest of the output, one line). `None` without a
/// commit.
pub fn parse_probe(raw: &str) -> Option<Probe> {
    let mut p = Probe::default();
    for line in raw.lines() {
        if let Some(c) = line.strip_prefix("commit=") {
            p.commit = c.trim().to_string();
        } else if let Some(rest) = line.strip_prefix("obj=") {
            let (hash, path) = rest.split_once(' ')?;
            p.objects.insert(path.to_string(), hash.trim().to_string());
        } else if let Some(v) = line.strip_prefix("version=") {
            p.version = v.trim().to_string();
        }
    }
    (is_full_hash(&p.commit)).then_some(p)
}

/// 40 (SHA-1) or 64 (SHA-256) hex digits.
pub fn is_full_hash(s: &str) -> bool {
    (s.len() == 40 || s.len() == 64) && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// What an agent may name as the commit to test: a hash, a branch, a tag,
/// `HEAD~2`. Never anything a shell or git could read as more than a
/// revision: it goes to the script as an argument all the same.
pub fn ref_error(r: &str) -> Option<&'static str> {
    if r.is_empty() {
        Some("an empty ref")
    } else if r.len() > 200 {
        Some("a ref is at most 200 characters")
    } else if r.starts_with('-') {
        Some("a ref may not start with '-'")
    } else if r.contains("..") || r.contains("@{") {
        Some("one commit, not a range or a reflog entry")
    } else if !r.bytes().all(|b| b.is_ascii_alphanumeric() || b"/._-~^".contains(&b)) {
        Some("a ref is letters, digits and / . _ - ~ ^ only")
    } else {
        None
    }
}

/// The key of a suite at a commit: SHA-256 of its declaration (and its
/// dependencies'), the objects of the covered paths and the version.
pub fn cache_key(suites: &[Suite], name: &str, probe: &Probe) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    let names = closure_names(suites, name).unwrap_or_else(|_| vec![name.to_string()]);
    h.update(b"sbxw-tests-1\0");
    h.update(name.as_bytes());
    for s in suites.iter().filter(|s| names.contains(&s.name)) {
        // serde_json of the declaration: every field, in a fixed order.
        h.update(b"\0suite\0");
        h.update(serde_json::to_string(s).unwrap_or_default().as_bytes());
    }
    for p in covered_paths(suites, name) {
        h.update(b"\0path\0");
        h.update(p.as_bytes());
        h.update(b"\0");
        h.update(probe.objects.get(&p).map(String::as_str).unwrap_or("-").as_bytes());
    }
    h.update(b"\0version\0");
    h.update(probe.version.as_bytes());
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// Colour codes out: the tail is read as text.
fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c != '\x1b' {
            out.push(c);
            continue;
        }
        if let Some('[') = chars.next() {
            for c in chars.by_ref() {
                if ('@'..='~').contains(&c) {
                    break;
                }
            }
        }
    }
    out
}

/// The last lines of a run's output, bounded in lines and bytes.
pub fn tail(raw: &[u8]) -> String {
    let text = strip_ansi(&String::from_utf8_lossy(raw));
    let lines: Vec<&str> = text.lines().collect();
    let mut out = lines[lines.len().saturating_sub(TAIL_LINES)..].join("\n");
    if out.len() > TAIL_BYTES {
        let mut cut = out.len() - TAIL_BYTES;
        while !out.is_char_boundary(cut) {
            cut += 1;
        }
        out = format!("…{}", &out[cut..]);
    }
    out.trim().to_string()
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Passed,
    Failed,
    /// Over its time limit, stopped.
    TimedOut,
    /// Could not run at all (the sandbox stopped, the checkout failed).
    Error,
}

impl Outcome {
    pub fn word(self) -> &'static str {
        match self {
            Outcome::Passed => "passed",
            Outcome::Failed => "failed",
            Outcome::TimedOut => "timed out",
            Outcome::Error => "could not run",
        }
    }
    /// Ran to its end: worth storing for the same code.
    pub fn reusable(self) -> bool {
        matches!(self, Outcome::Passed | Outcome::Failed)
    }
}

/// What a suite's run came to.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct TestResult {
    pub outcome: Outcome,
    /// The commit the suite actually ran on (for a reused result: the first).
    pub commit: String,
    pub duration_ms: u64,
    pub tail: String,
    /// 1, or 2 when the first run failed and the suite retries once.
    pub attempts: u8,
    /// The first attempt's outcome, when there was a second.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_outcome: Option<Outcome>,
    /// The full output of the last attempt, inside the sandbox.
    #[serde(default)]
    pub log: String,
    pub finished_ms: u64,
}

impl TestResult {
    pub fn retried(&self) -> bool {
        self.attempts > 1
    }
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum JobState {
    Queued,
    Running,
    Done,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct Job {
    pub id: String,
    pub sandbox: String,
    pub suite: String,
    pub key: String,
    /// The commit asked for, resolved to its hash.
    pub commit: String,
    /// The plan the request was made under, for the plan's counts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan: Option<String>,
    /// Who asked first: a thread's name, or `lead` / `main`.
    pub by: String,
    pub state: JobState,
    /// Answered from a stored result, without running.
    #[serde(default)]
    pub reused: bool,
    /// Requests that joined this job while it was queued or running.
    #[serde(default)]
    pub joined: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<TestResult>,
    pub queued_ms: u64,
    #[serde(default)]
    pub started_ms: Option<u64>,
}

/// Jobs that really ran, and requests answered without running (a stored
/// result, or a job already queued), per plan.
#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct Counts {
    pub ran: u32,
    pub reused: u32,
}

/// How a request was answered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Took {
    Queued,
    Joined,
    Reused,
}

/// Results by sandbox and key, plus the jobs: what `tests.json` holds.
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
pub struct Book {
    #[serde(default)]
    pub jobs: BTreeMap<String, Job>,
    /// `(sandbox, key)` → result, as `"<sandbox> <key>"`.
    #[serde(default)]
    pub results: BTreeMap<String, TestResult>,
    #[serde(default)]
    pub plans: BTreeMap<String, Counts>,
}

fn result_key(sandbox: &str, key: &str) -> String {
    format!("{sandbox} {key}")
}

/// One request for a suite at a commit.
pub struct Ask<'a> {
    pub sandbox: &'a str,
    pub suite: &'a str,
    pub key: &'a str,
    pub commit: &'a str,
    pub plan: Option<&'a str>,
    pub by: &'a str,
    /// Run again even when a result is stored (a failure thought flaky).
    pub rerun: bool,
}

impl Book {
    /// A book read back after a restart: what was running died with the
    /// daemon, so it waits to run again.
    pub fn loaded(mut self) -> Self {
        for j in self.jobs.values_mut() {
            if j.state == JobState::Running {
                j.state = JobState::Queued;
                j.started_ms = None;
            }
        }
        self
    }

    /// Queue `ask`, join the job already queued or running for its key, or
    /// answer the stored result.
    pub fn request(&mut self, id: String, ask: &Ask, now: u64) -> (Job, Took) {
        if let Some(j) = self
            .jobs
            .values_mut()
            .find(|j| j.sandbox == ask.sandbox && j.key == ask.key && j.state != JobState::Done)
        {
            j.joined += 1;
            let j = j.clone();
            self.count(ask.plan, false);
            return (j, Took::Joined);
        }
        let stored = if ask.rerun { None } else { self.results.get(&result_key(ask.sandbox, ask.key)).cloned() };
        let reused = stored.is_some();
        let job = Job {
            id: id.clone(),
            sandbox: ask.sandbox.to_string(),
            suite: ask.suite.to_string(),
            key: ask.key.to_string(),
            commit: ask.commit.to_string(),
            plan: ask.plan.map(str::to_string),
            by: ask.by.to_string(),
            state: if reused { JobState::Done } else { JobState::Queued },
            reused,
            joined: 0,
            result: stored,
            queued_ms: now,
            started_ms: None,
        };
        self.jobs.insert(id, job.clone());
        if reused {
            self.count(ask.plan, false);
        }
        (job, if reused { Took::Reused } else { Took::Queued })
    }

    fn count(&mut self, plan: Option<&str>, ran: bool) {
        if let Some(p) = plan {
            let c = self.plans.entry(p.to_string()).or_default();
            if ran {
                c.ran += 1;
            } else {
                c.reused += 1;
            }
        }
    }

    pub fn get(&self, id: &str, sandbox: &str) -> Result<&Job, String> {
        self.jobs
            .get(id)
            .filter(|j| j.sandbox == sandbox)
            .ok_or_else(|| format!("no test job {id:?} in this sandbox (finished jobs are kept {} days)", JOB_TTL_MS / 86_400_000))
    }

    pub fn running(&self, sandbox: &str) -> Option<&Job> {
        self.jobs.values().find(|j| j.sandbox == sandbox && j.state == JobState::Running)
    }

    /// The next job to run in `sandbox`, now running; `None` while one runs
    /// or when nothing waits. Oldest first.
    pub fn start_next(&mut self, sandbox: &str, now: u64) -> Option<Job> {
        if self.running(sandbox).is_some() {
            return None;
        }
        let id = self
            .jobs
            .values()
            .filter(|j| j.sandbox == sandbox && j.state == JobState::Queued)
            .min_by_key(|j| (j.queued_ms, j.id.clone()))?
            .id
            .clone();
        let j = self.jobs.get_mut(&id)?;
        j.state = JobState::Running;
        j.started_ms = Some(now);
        Some(j.clone())
    }

    /// The sandboxes with a job waiting: where a worker must run.
    pub fn waiting_sandboxes(&self) -> Vec<String> {
        let set: BTreeSet<String> = self.jobs.values().filter(|j| j.state == JobState::Queued).map(|j| j.sandbox.clone()).collect();
        set.into_iter().collect()
    }

    /// A running job is over: its result is kept, and stored for its key
    /// when the suite ran to its end.
    pub fn finish(&mut self, id: &str, result: TestResult) -> Option<Job> {
        let j = self.jobs.get_mut(id)?;
        if j.state == JobState::Done {
            return None;
        }
        j.state = JobState::Done;
        j.result = Some(result.clone());
        let j = j.clone();
        if result.outcome.reusable() {
            self.results.insert(result_key(&j.sandbox, &j.key), result);
        }
        self.count(j.plan.as_deref(), true);
        Some(j)
    }

    /// Jobs queued or running in `sandbox`, oldest first.
    pub fn active(&self, sandbox: &str) -> Vec<&Job> {
        let mut v: Vec<&Job> = self.jobs.values().filter(|j| j.sandbox == sandbox && j.state != JobState::Done).collect();
        v.sort_by_key(|j| j.queued_ms);
        v
    }

    /// The latest finished job of each suite in `sandbox`.
    pub fn latest(&self, sandbox: &str) -> BTreeMap<String, &Job> {
        let mut out: BTreeMap<String, &Job> = BTreeMap::new();
        for j in self.jobs.values().filter(|j| j.sandbox == sandbox && j.state == JobState::Done) {
            let newer = out.get(&j.suite).is_none_or(|o| o.queued_ms <= j.queued_ms);
            if newer {
                out.insert(j.suite.clone(), j);
            }
        }
        out
    }

    pub fn counts(&self, plan: &str) -> Counts {
        self.plans.get(plan).copied().unwrap_or_default()
    }

    /// Drop old results and finished jobs. True when anything went.
    pub fn prune(&mut self, now: u64) -> bool {
        let before = (self.jobs.len(), self.results.len());
        self.results.retain(|_, r| now.saturating_sub(r.finished_ms) < RESULT_TTL_MS);
        self.jobs.retain(|_, j| j.state != JobState::Done || now.saturating_sub(j.queued_ms) < JOB_TTL_MS);
        let sandboxes: BTreeSet<String> = self.results.keys().filter_map(|k| k.split_once(' ').map(|(s, _)| s.to_string())).collect();
        for sb in sandboxes {
            let mut mine: Vec<(u64, String)> = self
                .results
                .iter()
                .filter(|(k, _)| k.split_once(' ').is_some_and(|(s, _)| s == sb))
                .map(|(k, r)| (r.finished_ms, k.clone()))
                .collect();
            if mine.len() > MAX_RESULTS {
                mine.sort();
                for (_, k) in &mine[..mine.len() - MAX_RESULTS] {
                    self.results.remove(k);
                }
            }
        }
        let sandboxes: BTreeSet<String> = self.jobs.values().map(|j| j.sandbox.clone()).collect();
        for sb in sandboxes {
            let mut done: Vec<(u64, String)> =
                self.jobs.values().filter(|j| j.sandbox == sb && j.state == JobState::Done).map(|j| (j.queued_ms, j.id.clone())).collect();
            if done.len() > MAX_DONE_JOBS {
                done.sort();
                for (_, id) in &done[..done.len() - MAX_DONE_JOBS] {
                    self.jobs.remove(id);
                }
            }
        }
        before != (self.jobs.len(), self.results.len())
    }
}

/// A test job id: unguessable, like a plan id.
pub fn job_id() -> String {
    format!("t-{}", crate::ids::random_hex(12))
}

/// The grant lines of the suites, for `consent::grants`: each command the
/// sandbox runs when an agent asks, and the version command.
pub fn grant_lines(suites: &[Suite]) -> Vec<String> {
    let mut out = vec![];
    for s in suites {
        out.push(format!("test suite {}: an agent may make sbxw run `{}` in the sandbox (tests)", s.name, s.command));
        if !s.version_cmd.trim().is_empty() {
            out.push(format!("test suite {}: its version is read with `{}` in the sandbox (tests)", s.name, s.version_cmd));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn suite(name: &str, paths: &[&str], depends: &[&str]) -> Suite {
        Suite {
            name: name.into(),
            command: format!("scripts/run-suite.sh {name}"),
            paths: paths.iter().map(|s| s.to_string()).collect(),
            depends: depends.iter().map(|s| s.to_string()).collect(),
            timeout_minutes: DEFAULT_TIMEOUT_MINUTES,
            retries: 0,
            version_cmd: String::new(),
        }
    }

    fn probe(objs: &[(&str, &str)]) -> Probe {
        Probe {
            commit: "a".repeat(40),
            objects: objs.iter().map(|(p, h)| (p.to_string(), h.to_string())).collect(),
            version: "rustc 1.90".into(),
        }
    }

    #[test]
    fn declarations_are_checked() {
        assert!(validate(&[suite("crates", &["crates", "Cargo.lock"], &[]), suite("all", &["."], &["crates"])]).is_ok());
        let bad = |s: Suite| validate(&[s]).unwrap_err();
        assert!(bad(suite("Crates", &["."], &[])).contains("not a suite name"));
        assert!(bad(suite("a b", &["."], &[])).contains("not a suite name"));
        assert!(bad(suite("x", &[], &[])).contains("names no paths"));
        for p in ["/etc", "../up", "-x", "a\nb", "HEAD:x", ""] {
            assert!(validate(&[suite("x", &[p], &[])]).is_err(), "{p:?}");
        }
        assert!(bad(Suite { command: " ".into(), ..suite("x", &["."], &[]) }).contains("no command"));
        assert!(bad(Suite { command: "a\nb".into(), ..suite("x", &["."], &[]) }).contains("one line"));
        assert!(bad(Suite { retries: 2, ..suite("x", &["."], &[]) }).contains("0 or 1"));
        assert!(bad(Suite { timeout_minutes: 0, ..suite("x", &["."], &[]) }).contains("timeout_minutes"));
        assert!(bad(Suite { timeout_minutes: 999, ..suite("x", &["."], &[]) }).contains("timeout_minutes"));
        assert!(bad(suite("x", &["."], &["nope"])).contains("not declared"));
        assert!(bad(suite("x", &["."], &["x"])).contains("itself"));
        assert!(validate(&[suite("x", &["."], &[]), suite("x", &["."], &[])]).unwrap_err().contains("twice"));
        assert!(validate(&[suite("a", &["."], &["b"]), suite("b", &["."], &["a"])]).unwrap_err().contains("cycle"));
    }

    #[test]
    fn refs_are_revisions_and_nothing_else() {
        for ok in ["HEAD", "HEAD~2", "main", "sbxw/team-p-1", "v1.2.3", "abc123^"] {
            assert_eq!(ref_error(ok), None, "{ok}");
        }
        for bad in ["", "-x", "--output=/tmp/x", "a..b", "HEAD@{1}", "$(rm -rf /)", "x;ls", "a b", "`id`", "x|y"] {
            assert!(ref_error(bad).is_some(), "{bad:?}");
        }
    }

    #[test]
    fn covered_paths_follow_dependencies() {
        let s = [suite("core", &["crates/core/", "./Cargo.lock"], &[]), suite("daemon", &["crates/daemon"], &["core"]), suite("all", &["."], &[])];
        assert_eq!(covered_paths(&s, "daemon"), ["Cargo.lock", "crates/core", "crates/daemon"]);
        assert_eq!(covered_paths(&s, "all"), ["."]);
    }

    #[test]
    fn the_key_moves_with_covered_code_only() {
        let s = [suite("core", &["crates/core", "Cargo.lock"], &[]), suite("daemon", &["crates/daemon"], &["core"])];
        let base = probe(&[("Cargo.lock", "l1"), ("crates/core", "c1"), ("crates/daemon", "d1"), ("docs", "x1")]);
        let k = cache_key(&s, "daemon", &base);
        // An uncovered path, or another commit with the same trees: the same key.
        let mut other = base.clone();
        other.objects.insert("docs".into(), "x2".into());
        other.commit = "b".repeat(40);
        assert_eq!(cache_key(&s, "daemon", &other), k);
        // A dependency's path, a lock file, the version, the command: a new key.
        for (path, h) in [("crates/core", "c2"), ("Cargo.lock", "l2"), ("crates/daemon", "d2")] {
            let mut p = base.clone();
            p.objects.insert(path.into(), h.into());
            assert_ne!(cache_key(&s, "daemon", &p), k, "{path}");
        }
        assert_ne!(cache_key(&s, "daemon", &Probe { version: "rustc 1.91".into(), ..base.clone() }), k);
        let mut s2 = s.clone();
        s2[1].command = "other".into();
        assert_ne!(cache_key(&s2, "daemon", &base), k);
        assert_ne!(cache_key(&s, "core", &base), k, "the suite name is in the key");
    }

    #[test]
    fn the_probe_is_parsed() {
        let c = "c".repeat(40);
        let p = parse_probe(&format!("commit={c}\nobj=abc crates/core\nobj=- missing dir\nversion=rustc 1.90.0 (x)\n")).unwrap();
        assert_eq!(p.commit, c);
        assert_eq!(p.objects["crates/core"], "abc");
        assert_eq!(p.objects["missing dir"], "-");
        assert_eq!(p.version, "rustc 1.90.0 (x)");
        assert!(parse_probe("commit=nothex\n").is_none());
    }

    fn ask<'a>(key: &'a str, plan: Option<&'a str>) -> Ask<'a> {
        Ask { sandbox: "sb", suite: "core", key, commit: "c1", plan, by: "t1", rerun: false }
    }

    fn result(outcome: Outcome, now: u64) -> TestResult {
        TestResult { outcome, commit: "c1".into(), duration_ms: 5, tail: "ok".into(), attempts: 1, first_outcome: None, log: String::new(), finished_ms: now }
    }

    #[test]
    fn one_job_at_a_time_and_a_duplicate_joins() {
        let mut b = Book::default();
        let (j1, t1) = b.request("j1".into(), &ask("k1", Some("p")), 1);
        assert_eq!(t1, Took::Queued);
        let (_, t2) = b.request("j2".into(), &ask("k2", Some("p")), 2);
        assert_eq!(t2, Took::Queued);
        let (j3, t3) = b.request("j3".into(), &ask("k1", Some("p")), 3);
        assert_eq!((t3, j3.id.as_str()), (Took::Joined, "j1"));
        assert_eq!(b.start_next("sb", 4).unwrap().id, "j1");
        assert!(b.start_next("sb", 5).is_none(), "the second waits");
        // A join while it runs.
        assert_eq!(b.request("j4".into(), &ask("k1", None), 6).1, Took::Joined);
        b.finish(&j1.id, result(Outcome::Passed, 7)).unwrap();
        assert_eq!(b.start_next("sb", 8).unwrap().id, "j2");
        // Stored: answered at once, marked reused, the first commit.
        let (j5, t5) = b.request("j5".into(), &Ask { commit: "c9", ..ask("k1", Some("p")) }, 9);
        assert_eq!(t5, Took::Reused);
        assert!(j5.reused && j5.state == JobState::Done);
        assert_eq!(j5.result.unwrap().commit, "c1");
        assert_eq!(b.counts("p"), Counts { ran: 1, reused: 2 });
        // rerun skips the stored result.
        assert_eq!(b.request("j6".into(), &Ask { rerun: true, ..ask("k1", None) }, 10).1, Took::Queued);
    }

    #[test]
    fn only_a_run_to_its_end_is_stored() {
        let mut b = Book::default();
        for (i, o) in [Outcome::TimedOut, Outcome::Error].into_iter().enumerate() {
            let id = format!("j{i}");
            b.request(id.clone(), &ask("k", None), 1);
            b.start_next("sb", 2);
            b.finish(&id, result(o, 3));
            assert!(b.results.is_empty());
        }
        b.request("jf".into(), &ask("k", None), 4);
        b.start_next("sb", 5);
        b.finish("jf", result(Outcome::Failed, 6));
        assert_eq!(b.request("jx".into(), &ask("k", None), 7).1, Took::Reused);
    }

    #[test]
    fn a_running_job_runs_again_after_a_restart() {
        let mut b = Book::default();
        b.request("j1".into(), &ask("k", None), 1);
        b.start_next("sb", 2);
        let b: Book = serde_json::from_str::<Book>(&serde_json::to_string(&b).unwrap()).unwrap().loaded();
        assert_eq!(b.jobs["j1"].state, JobState::Queued);
        assert_eq!(b.waiting_sandboxes(), ["sb"]);
    }

    #[test]
    fn old_results_and_jobs_are_pruned() {
        let mut b = Book::default();
        b.request("j1".into(), &ask("k", None), 1);
        b.start_next("sb", 2);
        b.finish("j1", result(Outcome::Passed, 3));
        assert!(!b.prune(10));
        assert!(b.prune(3 + RESULT_TTL_MS + JOB_TTL_MS));
        assert!(b.results.is_empty() && b.jobs.is_empty());
    }

    #[test]
    fn the_tail_is_bounded() {
        let raw: String = (0..500).map(|i| format!("line {i} {}\n", "x".repeat(300))).collect();
        let t = tail(raw.as_bytes());
        assert!(t.len() <= TAIL_BYTES + 4, "{}", t.len());
        assert!(t.ends_with(&format!("line 499 {}", "x".repeat(300))));
        assert_eq!(tail(b"\x1b[31mred\x1b[0m\nok\n"), "red\nok");
    }
}
