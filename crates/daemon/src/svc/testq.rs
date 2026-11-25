//! The test queue: an agent names a declared suite (`[[tests]]`) and a
//! commit; the daemon answers a stored result for the same code at once, or
//! runs the suite inside the sandbox, one job at a time per sandbox.
//!
//! * The suites come from the config snapshot of the last bring-up
//!   (`App::config_for`), never from a fresh read of the agent-writable
//!   file, so a suite's command is one a person approved at `sbxw up`.
//! * Everything runs through `sbx_for`: the probe of the commit (its hash,
//!   the git objects of the covered paths, the toolchain version) and the run
//!   itself, so a remote host's sandbox is served the same way. Only a tail
//!   of the output crosses back.
//! * Jobs and results live in `state/tests.json`: a job id outlives a daemon
//!   restart, a job that was running runs again, and a result stays reusable.
//!   See `sbxw_core::testq` for the rules.

use crate::app::{now_ms, App};
use sbxw_core::plans::{Plan, PlanState, Variant};
use sbxw_core::testq::{self, Ask, Book, Job, JobState, Outcome, Suite, TestResult, Took};
use sbxw_core::{SandboxRef, ThreadName};
use sbxw_sbx::runner::Invocation;
use sbxw_sbx::scripts::{tests_probe_args, tests_run_args, TESTS_ERROR_MARK, TESTS_LOG_MARK};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// The probe reads git and runs a version command: seconds, normally.
const PROBE_MAX: Duration = Duration::from_secs(120);
/// The daemon's own bound on a run, past the suite's limit (which `timeout`
/// enforces inside the sandbox): a wedged `sbx exec` cannot hold the queue.
const BACKSTOP: Duration = Duration::from_secs(120);

/// One request, as the relay sends it.
#[derive(Default)]
pub struct TestAsk<'a> {
    pub thread: Option<&'a ThreadName>,
    /// Empty: the team suite of the caller's plan.
    pub suite: Option<&'a str>,
    /// Empty: the team's code for the team suite of a shared plan, else `head`.
    pub git_ref: Option<&'a str>,
    /// The caller's `HEAD` commit, as its relay server read it.
    pub head: Option<&'a str>,
    /// A lead names its plan; a thread's is found by its name.
    pub plan: Option<&'a str>,
    pub rerun: bool,
}

pub(crate) fn load(path: &Path) -> Book {
    match std::fs::read_to_string(path) {
        Ok(t) => match serde_json::from_str::<Book>(&t) {
            Ok(b) => b.loaded(),
            Err(e) => {
                tracing::warn!("tests: could not read {}: {e}; starting with no stored results", path.display());
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

    /// The suites `sb` was brought up with.
    pub async fn tests_suites(&self, sb: &SandboxRef) -> Vec<Suite> {
        self.config_for(sb).await.tests
    }

    /// A team suite a lead names must be one the sandbox declares.
    pub async fn tests_check_team_suite(&self, sb: &SandboxRef, name: Option<&str>) -> Result<Option<String>, String> {
        let Some(name) = nonempty(name) else { return Ok(None) };
        let suites = self.tests_suites(sb).await;
        if suites.iter().any(|s| s.name == name) {
            Ok(Some(name.to_string()))
        } else {
            Err(format!("team_suite {name:?} is not a declared test suite of this sandbox ({})", declared(&suites)))
        }
    }

    /// The plan a request counts under: a thread's own, or the one a lead
    /// names, or the sandbox's latest approved plan still open.
    fn tests_plan(&self, sb: &SandboxRef, thread: Option<&ThreadName>, plan: Option<&str>) -> Option<Plan> {
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

    /// Queue a suite on a commit, join the job already at work on the same
    /// code, or answer the stored result. Refused: no suites, a name not
    /// declared, a ref that is not a plain revision or not a commit of the
    /// workspace's repository.
    pub async fn tests_request(self: &Arc<Self>, sb: &SandboxRef, ask: TestAsk<'_>) -> Result<(Job, Took), String> {
        let suites = self.tests_suites(sb).await;
        if suites.is_empty() {
            return Err(format!(
                "'{sb}' declares no test suites: they are `[[tests]]` tables in its sbxw.toml, applied when a person runs `sbxw up`"
            ));
        }
        let plan = self.tests_plan(sb, ask.thread, ask.plan);
        let team_suite = plan.as_ref().and_then(|p| p.team_suite.clone());
        let name = match nonempty(ask.suite) {
            Some(n) => n.to_string(),
            None => team_suite.clone().ok_or_else(|| format!("name a suite: {}", declared(&suites)))?,
        };
        let Some(suite) = suites.iter().find(|s| s.name == name) else {
            return Err(format!("{name:?} is not a declared test suite ({})", declared(&suites)));
        };
        // The team's code: a shared plan's branch tip, unless a ref is named.
        let team_branch = plan
            .as_ref()
            .filter(|p| p.variant == Variant::Shared && team_suite.as_deref() == Some(name.as_str()))
            .map(|p| format!("sbxw/{}", p.team_name()));
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
        let paths = testq::covered_paths(&suites, &name);
        let args = tests_probe_args(&sb.name, &ws.to_string_lossy(), &git_ref, &suite.version_cmd, &paths);
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
        let key = testq::cache_key(&suites, &name, &probe);
        let by = match (ask.thread, &plan) {
            (Some(t), _) => t.to_string(),
            (None, Some(_)) => "lead".to_string(),
            (None, None) => "main".to_string(),
        };
        let sandbox = sb.to_string();
        let plan_id = plan.as_ref().map(|p| p.id.clone());
        let (job, took) = self.tests.lock().unwrap().request(
            testq::job_id(),
            &Ask { sandbox: &sandbox, suite: &name, key: &key, commit: &probe.commit, plan: plan_id.as_deref(), by: &by, rerun: ask.rerun },
            now_ms(),
        );
        tracing::info!("tests: '{sb}' {by} asked for {name} at {} ({took:?}, job {})", short(&probe.commit), job.id);
        self.tests_changed();
        if took == Took::Queued {
            self.tests_kick(&sandbox);
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

    /// The jobs queued or running in `sb`, and the latest result per suite.
    pub fn tests_jobs(&self, sb: &SandboxRef) -> (Vec<Job>, Vec<Job>) {
        let b = self.tests.lock().unwrap();
        let s = sb.to_string();
        (b.active(&s).into_iter().cloned().collect(), b.latest(&s).into_values().cloned().collect())
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
                job.suite,
                short(&job.commit),
                result.outcome.word(),
                result.duration_ms / 1000,
                if result.retried() { " (retried)" } else { "" }
            );
            self.tests.lock().unwrap().finish(&job.id, result);
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
            log: String::new(),
            finished_ms: now_ms(),
        };
        let Ok(sb) = SandboxRef::parse(&job.sandbox) else { return error(format!("'{}' is not a sandbox name", job.sandbox)) };
        let suites = self.tests_suites(&sb).await;
        let Some(suite) = suites.iter().find(|s| s.name == job.suite) else {
            return error(format!("the suite {:?} is no longer declared", job.suite));
        };
        let Some(ws) = self.store.workspace(&sb) else { return error(format!("sbxw has no workspace recorded for '{sb}'")) };
        let first = self.tests_attempt(&sb, &ws.to_string_lossy(), suite, &job.commit).await;
        let retry = suite.retries > 0 && matches!(first.outcome, Outcome::Failed | Outcome::TimedOut);
        if !retry {
            return first;
        }
        tracing::info!("tests: '{sb}' {} {} at {}; running it once more", suite.name, first.outcome.word(), short(&job.commit));
        let second = self.tests_attempt(&sb, &ws.to_string_lossy(), suite, &job.commit).await;
        TestResult { attempts: 2, first_outcome: Some(first.outcome), duration_ms: first.duration_ms + second.duration_ms, ..second }
    }

    async fn tests_attempt(&self, sb: &SandboxRef, ws: &str, suite: &Suite, commit: &str) -> TestResult {
        let limit = Duration::from_secs(u64::from(suite.timeout_minutes) * 60);
        let args = tests_run_args(&sb.name, ws, &suite.name, commit, &suite.command, limit.as_secs());
        let started = Instant::now();
        let run = async { self.sbx_for(sb).await?.raw(Invocation::new(args)).await };
        let out = tokio::time::timeout(limit + BACKSTOP, run).await;
        let mut log = String::new();
        let (outcome, tail) = match out {
            Err(_) => (Outcome::TimedOut, format!("still running after {} minutes: stopped by sbxw", suite.timeout_minutes)),
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
                        (Outcome::TimedOut, format!("over its limit of {} minutes, stopped.\n{}", suite.timeout_minutes, testq::tail(rest.as_bytes())))
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
            log,
            finished_ms: now_ms(),
        }
    }
}

fn declared(suites: &[Suite]) -> String {
    if suites.is_empty() {
        "none is declared".into()
    } else {
        format!("declared: {}", suites.iter().map(|s| s.name.as_str()).collect::<Vec<_>>().join(", "))
    }
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
    }
    v
}
