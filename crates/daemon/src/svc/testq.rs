//! The test queue: an agent asks for its plan's approved test command on a
//! commit; the daemon answers a stored result for the same command on the
//! same tree at once, or runs the command inside the sandbox, one job at a
//! time per sandbox.
//!
//! * The command is the plan's `approved_tests`: proposed by the lead with
//!   the plan, approved by the person in the plan dialog, fixed from then
//!   on. No request carries a command; a plan without one has no queue.
//! * Everything runs through `sbx_for`: the probe of the commit (its hash
//!   and its root tree) and the run itself, so a remote host's sandbox is
//!   served the same way. Only a tail of the output crosses back.
//! * Each attempt keeps its own log in the sandbox,
//!   `~/.sbxw/tests/logs/<plan>/<job>-<attempt>.log`, for the plan's last
//!   [`testq::KEEP_LOG_JOBS`] jobs; a retried result carries its first
//!   attempt (outcome, duration, log, tail) beside the second.
//! * Jobs and results live in `state/tests.json`: a job id outlives a daemon
//!   restart, a job that was running runs again, and a result stays reusable.
//!   See `sbxw_core::testq` for the rules.

use crate::app::{now_ms, App};
use sbxw_core::plans::{Plan, PlanState, Variant};
use sbxw_core::team;
use sbxw_core::testq::{self, Ask, Attempt, Book, Job, JobState, Outcome, TestResult, Took};
use sbxw_core::{SandboxRef, ThreadName};
use sbxw_sbx::runner::Invocation;
use sbxw_sbx::scripts::{tests_probe_args, tests_run_args, TESTS_ERROR_MARK, TESTS_LOG_MARK};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// The probe reads git: seconds, normally.
const PROBE_MAX: Duration = Duration::from_secs(120);
/// The daemon's own bound on a run, past the plan's limit (which `timeout`
/// enforces inside the sandbox): a wedged `sbx exec` cannot hold the queue.
const BACKSTOP: Duration = Duration::from_secs(120);

/// One request, as the relay sends it.
#[derive(Default)]
pub struct TestAsk<'a> {
    pub thread: Option<&'a ThreadName>,
    /// Empty: a shared plan's team branch tip, else `head`.
    pub git_ref: Option<&'a str>,
    /// The caller's `HEAD` commit, as its relay server read it.
    pub head: Option<&'a str>,
    /// A lead names its plan; a thread's is found by its name.
    pub plan: Option<&'a str>,
    pub rerun: bool,
    /// Who asks when it is neither a thread nor the lead: `sbxw` for a CI
    /// branch's tests.
    pub by: Option<&'a str>,
}

pub(crate) fn load(path: &Path) -> Book {
    match std::fs::read_to_string(path) {
        Ok(t) => match Book::parse_lenient(&t) {
            Some(b) => b.loaded(),
            None => {
                tracing::warn!("tests: could not read {}; starting with no stored results", path.display());
                Book::default()
            }
        },
        Err(_) => Book::default(),
    }
}

fn nonempty(s: Option<&str>) -> Option<&str> {
    s.map(str::trim).filter(|s| !s.is_empty())
}

impl App {
    pub(crate) fn tests_file(&self) -> PathBuf {
        self.store.paths.state().join("tests.json")
    }

    fn persist_tests(&self) {
        let bytes = serde_json::to_vec_pretty(&*self.tests.lock().unwrap());
        let path = self.tests_file();
        if let Err(e) = bytes.map_err(anyhow::Error::from).and_then(|b| crate::store::write_atomic(&path, &b)) {
            tracing::warn!("tests: could not save {}: {e:#}", path.display());
        }
    }

    fn tests_changed(&self) {
        self.persist_tests();
        self.tests_tick.send_modify(|n| *n = n.wrapping_add(1));
    }

    /// The plan a request runs under: a thread's own, or the one a lead
    /// names, or the sandbox's latest approved plan still open.
    pub fn tests_plan(&self, sb: &SandboxRef, thread: Option<&ThreadName>, plan: Option<&str>) -> Option<Plan> {
        let plans = self.plans.lock().unwrap();
        let s = sb.to_string();
        if let Some(t) = thread {
            return plans.thread_plan(&s, t.as_str()).map(|(p, _)| p.clone());
        }
        if let Some(id) = nonempty(plan) {
            return plans.owned(id, &s).ok().cloned();
        }
        plans
            .list()
            .into_iter()
            .filter(|p| p.sandbox == s && p.state == PlanState::Approved && !p.closed && !p.dismissed)
            .max_by_key(|p| p.created_ms)
    }

    /// Queue the plan's approved command on a commit, join the job already
    /// at work on the same tree, or answer the stored result. Refused: no
    /// plan with an approved test command, a ref that is not a plain
    /// revision or not a commit of the workspace's repository.
    pub async fn tests_request(self: &Arc<Self>, sb: &SandboxRef, ask: TestAsk<'_>) -> Result<(Job, Took), String> {
        let plan = self.tests_plan(sb, ask.thread, ask.plan);
        let Some((plan, tests)) = plan.and_then(|p| p.approved_tests.clone().map(|t| (p, t))) else {
            return Err(NO_COMMAND.to_string());
        };
        // The team's code: a shared plan's branch tip, unless a ref is named.
        let team_branch = (plan.variant == Variant::Shared).then(|| format!("sbxw/{}", plan.team_name()));
        let git_ref = match (nonempty(ask.git_ref), team_branch, nonempty(ask.head)) {
            (Some(r), _, _) => r.to_string(),
            (None, Some(b), _) => b,
            (None, None, Some(h)) => h.to_string(),
            (None, None, None) => return Err("name the commit to test (ref): your HEAD could not be read".into()),
        };
        if let Some(why) = testq::ref_error(&git_ref) {
            return Err(format!("ref {git_ref:?} refused: {why}"));
        }
        let ws = self.store.workspace(sb).ok_or_else(|| format!("sbxw has no workspace recorded for '{sb}', so it has no repository to test"))?;
        let args = tests_probe_args(&sb.name, &ws.to_string_lossy(), &git_ref);
        let probe = async { self.sbx_for(sb).await?.raw(Invocation::new(args)).await };
        let out = match tokio::time::timeout(PROBE_MAX, probe).await {
            Err(_) => return Err(format!("reading {git_ref} in '{sb}' took over {} s", PROBE_MAX.as_secs())),
            Ok(Err(e)) => return Err(format!("could not read {git_ref} in '{sb}': {e:#}")),
            Ok(Ok(o)) => o,
        };
        let text = out.stdout_str();
        if let Some(e) = text.lines().find_map(|l| l.strip_prefix("error=")) {
            return Err(e.trim().to_string());
        }
        let probe = testq::parse_probe(&text).ok_or_else(|| {
            let why = testq::tail(if out.stderr.is_empty() { &out.stdout } else { &out.stderr });
            format!("could not read {git_ref} in '{sb}' (exit {}): {why}", out.code.unwrap_or(-1))
        })?;
        let key = testq::cache_key(&tests.command, &probe.tree);
        let by = ask.by.map(str::to_string).unwrap_or_else(|| ask.thread.map_or_else(|| "lead".to_string(), |t| t.to_string()));
        let sandbox = sb.to_string();
        let (job, took) = self.tests.lock().unwrap().request(
            testq::job_id(),
            &Ask { sandbox: &sandbox, tests: &tests, key: &key, commit: &probe.commit, plan: Some(&plan.id), by: &by, rerun: ask.rerun },
            now_ms(),
        );
        tracing::info!("tests: '{sb}' {by} asked for {}'s tests at {} ({took:?}, job {})", plan.id, short(&probe.commit), job.id);
        self.tests_changed();
        match took {
            Took::Queued => self.tests_kick(&sandbox),
            Took::Reused => self.test_job_noted(Some(&plan.id), &job, &by, Some(team::REUSE_STORED)),
            // A join is logged when the job finishes, with its result.
            Took::Joined => {}
        }
        Ok((job, took))
    }

    /// Park until the job is done or the bound passes; the job as it stands.
    pub async fn tests_wait(&self, id: &str, sb: &SandboxRef, bound: Duration) -> Result<Job, String> {
        let mut rx = self.tests_tick.subscribe();
        let deadline = tokio::time::Instant::now() + bound;
        loop {
            let job = self.tests.lock().unwrap().get(id, &sb.to_string())?.clone();
            let left = deadline.saturating_duration_since(tokio::time::Instant::now());
            if job.state == JobState::Done || left.is_zero() {
                return Ok(job);
            }
            if tokio::time::timeout(left, rx.changed()).await.map_or(true, |r| r.is_err()) {
                return Ok(self.tests.lock().unwrap().get(id, &sb.to_string())?.clone());
            }
        }
    }

    /// The jobs queued or running in `sb`, and the last ones finished.
    pub fn tests_jobs(&self, sb: &SandboxRef) -> (Vec<Job>, Vec<Job>) {
        let b = self.tests.lock().unwrap();
        let s = sb.to_string();
        (b.active(&s).into_iter().cloned().collect(), b.latest(&s, testq::LIST_DONE_JOBS).into_iter().cloned().collect())
    }

    /// The plans `list_tests` shows the caller: a thread's own; for any
    /// other session, the open approved plans of the sandbox it leads
    /// (`session`, the lead's session name, as plans record it) or names
    /// (`plan`), else every open approved plan of the sandbox. Newest first.
    pub fn tests_plans_of(&self, sb: &SandboxRef, thread: Option<&ThreadName>, plan: Option<&str>, session: Option<&str>) -> Vec<Plan> {
        if thread.is_some() {
            return self.tests_plan(sb, thread, None).into_iter().collect();
        }
        let plans = self.plans.lock().unwrap();
        let s = sb.to_string();
        let mut open: Vec<Plan> = plans
            .list()
            .into_iter()
            .filter(|p| p.sandbox == s && p.state == PlanState::Approved && !p.closed && !p.dismissed)
            .collect();
        open.sort_by_key(|p| std::cmp::Reverse((p.created_ms, p.id.clone())));
        let (plan, session) = (nonempty(plan), nonempty(session));
        let mine: Vec<Plan> =
            open.iter().filter(|p| Some(p.id.as_str()) == plan || (session.is_some() && p.lead.as_deref() == session)).cloned().collect();
        if mine.is_empty() {
            open
        } else {
            mine
        }
    }

    /// What `list_tests` shows of each plan: its command, what is queued
    /// or running for it, and its last results, each with its attempts'
    /// logs.
    pub fn tests_list(&self, sb: &SandboxRef, plans: &[Plan]) -> Vec<serde_json::Value> {
        let b = self.tests.lock().unwrap();
        let s = sb.to_string();
        plans
            .iter()
            .map(|p| {
                let active: Vec<_> = b.plan_active(&s, &p.id).into_iter().map(job_view).collect();
                let latest: Vec<_> = b.plan_latest(&s, &p.id, testq::LIST_DONE_JOBS).into_iter().map(job_view).collect();
                serde_json::json!({ "plan": plan_tests_view(Some(p)), "active": active, "latest": latest })
            })
            .collect()
    }

    /// A test request answered: a line on the timeline of the plan it was
    /// made under (none: nothing), for the Board.
    pub(crate) fn test_job_noted(&self, plan: Option<&str>, job: &Job, asker: &str, reuse: Option<&str>) {
        let Some(plan) = plan else { return };
        let Some((text, data)) = team::test_job_entry(job, asker, reuse) else { return };
        let sandbox = {
            let mut plans = self.plans.lock().unwrap();
            let Some(sandbox) = plans.get(plan).map(|p| p.sandbox.clone()) else { return };
            plans.note(plan, now_ms(), team::EventKind::TestJob, asker, None, Some(&text), data);
            sandbox
        };
        self.persist_team_logs(&sandbox);
        self.team_changed(plan);
    }

    /// The test gate (audit D10), for the hook route: a thread's Bash
    /// command that runs its plan's whole approved test command is refused,
    /// with the reason to print; the refusal is logged on the plan (the
    /// thread, never the command). `None`, the command passes: any other
    /// event, tool or command, the lead's or the person's sessions (no
    /// thread), a thread whose plan has no approved command.
    pub fn tests_gate(&self, sandbox: &SandboxRef, event: &serde_json::Value) -> Option<String> {
        if event["hook_event_name"] != "PreToolUse" || event["tool_name"] != "Bash" {
            return None;
        }
        let thread = nonempty(event["sbxw_thread"].as_str())?;
        let command = event["tool_input"]["command"].as_str()?;
        let (id, approved) = {
            let plans = self.plans.lock().unwrap();
            let (p, _) = plans.thread_plan(&sandbox.to_string(), thread)?;
            if p.state != PlanState::Approved || p.closed {
                return None;
            }
            (p.id.clone(), p.approved_tests.as_ref()?.command.clone())
        };
        if !testq::runs_plan_command(command, &approved) {
            return None;
        }
        self.plans.lock().unwrap().note(
            &id,
            now_ms(),
            team::EventKind::TestRefused,
            thread,
            None,
            Some("ran the plan's whole test command itself: sent to the test queue"),
            serde_json::json!({ "thread": thread }),
        );
        tracing::info!("tests: '{sandbox}' thread {thread} ran {id}'s whole test command: refused, sent to the queue");
        self.persist_team_logs(&sandbox.to_string());
        self.team_changed(&id);
        Some(GATE_REASON.to_string())
    }

    pub fn tests_counts(&self, plan: &str) -> testq::Counts {
        self.tests.lock().unwrap().counts(plan)
    }

    /// Start a worker for every sandbox with a job waiting: after a restart.
    pub fn tests_resume(self: &Arc<Self>) {
        let waiting = self.tests.lock().unwrap().waiting_sandboxes();
        for sb in waiting {
            self.tests_kick(&sb);
        }
        // The CI runs at work before the restart, and the watch on main.
        self.ci_resume();
    }

    pub(crate) fn tests_prune(&self) {
        if self.tests.lock().unwrap().prune(now_ms()) {
            self.tests_changed();
        }
    }

    /// One worker per sandbox: started when a job is queued and none runs.
    fn tests_kick(self: &Arc<Self>, sandbox: &str) {
        if !self.tests_workers.lock().unwrap().insert(sandbox.to_string()) {
            return;
        }
        let app = self.clone();
        let sandbox = sandbox.to_string();
        tokio::spawn(async move { app.tests_worker(sandbox).await });
    }

    /// Runs the sandbox's jobs one after the other, oldest first. Gives up its
    /// slot under the book's lock, so a job queued meanwhile is never left
    /// without a worker (`tests_request` queues, then kicks).
    async fn tests_worker(self: Arc<Self>, sandbox: String) {
        loop {
            let job = {
                let mut book = self.tests.lock().unwrap();
                match book.start_next(&sandbox, now_ms()) {
                    Some(j) => j,
                    None => {
                        self.tests_workers.lock().unwrap().remove(&sandbox);
                        return;
                    }
                }
            };
            self.tests_changed();
            let result = self.tests_run_job(&job).await;
            tracing::info!(
                "tests: '{sandbox}' {} at {} {} in {} s{}",
                job.plan.as_deref().unwrap_or("-"),
                short(&job.commit),
                result.outcome.word(),
                result.duration_ms / 1000,
                if result.retried() { " (retried)" } else { "" }
            );
            let done = self.tests.lock().unwrap().finish(&job.id, result);
            // On the timeline before the waiters are told it is done.
            if let Some(done) = done {
                self.test_job_noted(done.plan.as_deref(), &done, &done.by, None);
                for j in &done.joiners {
                    self.test_job_noted(j.plan.as_deref(), &done, &j.by, Some(team::REUSE_JOINED));
                }
            }
            self.tests_changed();
        }
    }

    async fn tests_run_job(&self, job: &Job) -> TestResult {
        let error = |why: String| TestResult {
            outcome: Outcome::Error,
            commit: job.commit.clone(),
            duration_ms: 0,
            tail: why,
            attempts: 1,
            first_outcome: None,
            first: None,
            log: String::new(),
            finished_ms: now_ms(),
        };
        let Ok(sb) = SandboxRef::parse(&job.sandbox) else { return error(format!("'{}' is not a sandbox name", job.sandbox)) };
        let Some(ws) = self.store.workspace(&sb) else { return error(format!("sbxw has no workspace recorded for '{sb}'")) };
        let first = self.tests_attempt(&sb, &ws.to_string_lossy(), job, 1).await;
        // One retry, of a failure only: a time-out already took the whole limit.
        if first.outcome != Outcome::Failed {
            return first;
        }
        tracing::info!("tests: '{sb}' {} failed at {}; running it once more", job.id, short(&job.commit));
        let second = self.tests_attempt(&sb, &ws.to_string_lossy(), job, 2).await;
        let kept = Attempt { outcome: first.outcome, duration_ms: first.duration_ms, log: first.log, tail: first.tail };
        TestResult {
            attempts: 2,
            first_outcome: Some(kept.outcome),
            duration_ms: kept.duration_ms + second.duration_ms,
            first: Some(kept),
            ..second
        }
    }

    /// Attempt `n` of `job`, logged to its own file (`<job>-<n>.log`).
    async fn tests_attempt(&self, sb: &SandboxRef, ws: &str, job: &Job, n: u8) -> TestResult {
        let (commit, minutes) = (job.commit.as_str(), job.timeout_minutes);
        let limit = Duration::from_secs(u64::from(minutes) * 60);
        let label = job.plan.as_deref().unwrap_or("tests");
        let run = format!("{}-{n}", job.id);
        let args = tests_run_args(&sb.name, ws, label, commit, &job.command, &run, testq::KEEP_LOG_JOBS, limit.as_secs());
        let started = Instant::now();
        let run = async { self.sbx_for(sb).await?.raw(Invocation::new(args)).await };
        let out = tokio::time::timeout(limit + BACKSTOP, run).await;
        let mut log = String::new();
        let (outcome, tail) = match out {
            Err(_) => (Outcome::TimedOut, format!("still running after {} minutes: stopped by sbxw", minutes)),
            Ok(Err(e)) => (Outcome::Error, format!("could not run: {e:#}")),
            Ok(Ok(o)) => {
                let text = o.stdout_str();
                let mut rest = String::new();
                let mut error = None;
                for line in text.lines() {
                    if let Some(p) = line.strip_prefix(TESTS_LOG_MARK) {
                        log = p.trim().to_string();
                    } else if let Some(e) = line.strip_prefix(TESTS_ERROR_MARK) {
                        error = Some(e.trim().to_string());
                    } else {
                        rest.push_str(line);
                        rest.push('\n');
                    }
                }
                match (error, o.code) {
                    (Some(e), _) => (Outcome::Error, e),
                    (None, Some(0)) => (Outcome::Passed, testq::tail(rest.as_bytes())),
                    (None, Some(124)) => {
                        (Outcome::TimedOut, format!("over its limit of {} minutes, stopped.\n{}", minutes, testq::tail(rest.as_bytes())))
                    }
                    (None, _) if rest.trim().is_empty() && log.is_empty() => {
                        // Not the script's output: sbx itself failed.
                        (Outcome::Error, format!("sbx exec failed (exit {}): {}", o.code.unwrap_or(-1), testq::tail(&o.stderr)))
                    }
                    (None, _) => (Outcome::Failed, testq::tail(rest.as_bytes())),
                }
            }
        };
        TestResult {
            outcome,
            commit: commit.to_string(),
            duration_ms: started.elapsed().as_millis() as u64,
            tail,
            attempts: 1,
            first_outcome: None,
            first: None,
            log,
            finished_ms: now_ms(),
        }
    }
}

/// What a thread is told when it runs the plan's whole test command itself.
pub const GATE_REASON: &str = "sbxw: this is your plan's whole test command, which a thread does not run. \
Run only the tests of what you touched (one test, one package: fine to run yourself), commit, and report done: \
the whole battery runs once, on the plan's CI branch, after the lead proposes the merge. \
For a risky change, commit and call run_tests (MCP sbxw-relay): it answers a stored result at once when the code did not change.";

/// The refusal outside any plan with an approved test command.
pub const NO_COMMAND: &str = "no test command: run_tests runs the test command of an approved plan, and this plan has none. \
The lead proposes the project's test command with the plan (propose_plan's test_command), and the person approves it with the plan; \
until then, run the tests as the repository says.";

/// What `list_tests` says of the caller's plan: its approved command, or
/// none and why.
pub fn plan_tests_view(plan: Option<&Plan>) -> serde_json::Value {
    serde_json::json!({
        "plan": plan.map(|p| p.id.clone()),
        "tests": plan.and_then(|p| p.approved_tests.as_ref()),
        "not_approved": plan.and_then(|p| p.tests_not_approved.clone()),
    })
}

fn short(commit: &str) -> &str {
    commit.get(..10).unwrap_or(commit)
}

/// A job as an agent reads it.
pub fn job_view(job: &Job) -> serde_json::Value {
    let mut v = serde_json::to_value(job).unwrap_or_default();
    if let Some(o) = v.as_object_mut() {
        o.remove("key");
        o.remove("sandbox");
        o.remove("joiners");
    }
    v
}

#[cfg(test)]
#[path = "testq_tests.rs"]
mod tests;
