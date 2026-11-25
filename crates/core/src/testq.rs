//! The test queue's rules: the test command a plan carries, the key a
//! result is stored under, and the book of jobs and results.
//!
//! * The command belongs to the plan: the lead proposes it with the plan,
//!   the person sees it in the plan dialog and approves it with the plan.
//!   It is fixed from then on, and no request ever carries a command: an
//!   agent asks for its plan's command on a commit.
//! * A job tests a commit. Its key is the approved command's text and the
//!   git tree of the commit's root (`<commit>^{tree}`): the same command on
//!   the same tree is the same run, so the stored result is answered at
//!   once, marked as reused, with the commit it was first run on, across
//!   threads and plans of the sandbox.
//! * One job runs at a time per sandbox; a request for a key already queued
//!   or running joins that job instead of queueing a second one.
//! * Only a run that went to its end (passed or failed) is stored for reuse.
//!   A time-out or a run that could not start is answered, never reused.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// The default time limit of a plan's tests, in minutes.
pub const DEFAULT_TIMEOUT_MINUTES: u32 = 60;
/// The longest a plan's tests may be given.
pub const MAX_TIMEOUT_MINUTES: u32 = 180;
/// The longest command a plan may carry, in bytes.
pub const MAX_COMMAND_BYTES: usize = 1000;
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
/// Finished jobs `list_tests` shows.
pub const LIST_DONE_JOBS: usize = 5;

/// A plan's test command: one shell command, run with `bash -c` inside the
/// sandbox in the queue's checkout of the commit, with a time limit. It
/// gets `SBXW_TEST_TARGET` (a build directory kept from job to job),
/// `SBXW_TEST_CHECKOUT`, `SBXW_TEST_SUITE` (the plan's id) and
/// `SBXW_TEST_COMMIT`.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct PlanTests {
    pub command: String,
    pub timeout_minutes: u32,
}

impl PlanTests {
    /// What the lead proposed: `None` for no command (empty or absent);
    /// refused when it is not one line. The time limit defaults to
    /// [`DEFAULT_TIMEOUT_MINUTES`] and is capped at [`MAX_TIMEOUT_MINUTES`].
    pub fn proposed(command: Option<&str>, timeout_minutes: Option<u32>) -> Result<Option<PlanTests>, String> {
        let Some(command) = command.map(str::trim).filter(|c| !c.is_empty()) else { return Ok(None) };
        if command.chars().any(|c| c.is_control()) {
            return Err("test_command: one command on one line, no control characters".into());
        }
        if command.len() > MAX_COMMAND_BYTES {
            return Err(format!("test_command: at most {MAX_COMMAND_BYTES} bytes; put a longer battery in a script of the repository"));
        }
        let timeout_minutes = match timeout_minutes {
            None | Some(0) => DEFAULT_TIMEOUT_MINUTES,
            Some(m) => m.min(MAX_TIMEOUT_MINUTES),
        };
        Ok(Some(PlanTests { command: command.to_string(), timeout_minutes }))
    }
}

/// What the sandbox said about a commit: its full hash and the hash of its
/// root tree.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Probe {
    pub commit: String,
    pub tree: String,
}

/// The probe script's output: `commit=<sha>` and `tree=<sha>` lines. `None`
/// unless both are full hashes.
pub fn parse_probe(raw: &str) -> Option<Probe> {
    let mut p = Probe::default();
    for line in raw.lines() {
        if let Some(c) = line.strip_prefix("commit=") {
            p.commit = c.trim().to_string();
        } else if let Some(t) = line.strip_prefix("tree=") {
            p.tree = t.trim().to_string();
        }
    }
    (is_full_hash(&p.commit) && is_full_hash(&p.tree)).then_some(p)
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

/// The key of a command on a commit: SHA-256 of the command's text and the
/// commit's root tree. Nothing else: the tree is the whole code.
pub fn cache_key(command: &str, tree: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(b"sbxw-tests-2\0");
    h.update((command.len() as u64).to_be_bytes());
    h.update(command.as_bytes());
    h.update(b"\0tree\0");
    h.update(tree.as_bytes());
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

/// What a run came to.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct TestResult {
    pub outcome: Outcome,
    /// The commit it actually ran on (for a reused result: the first).
    pub commit: String,
    pub duration_ms: u64,
    pub tail: String,
    /// 1, or 2 when the first run failed and it ran once more.
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
    /// The plan's approved command, as it ran (no request carries one).
    pub command: String,
    pub timeout_minutes: u32,
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

/// One request for a plan's command at a commit.
pub struct Ask<'a> {
    pub sandbox: &'a str,
    pub tests: &'a PlanTests,
    pub key: &'a str,
    pub commit: &'a str,
    pub plan: Option<&'a str>,
    pub by: &'a str,
    /// Run again even when a result is stored (a failure thought flaky).
    pub rerun: bool,
}

impl Book {
    /// `tests.json` as read back, entry by entry: a job, a result or a
    /// count that does not parse (a file of an earlier format: jobs that
    /// named a suite) is left out rather than costing the whole book.
    /// `None` when the text is not a JSON object at all.
    pub fn parse_lenient(text: &str) -> Option<Book> {
        fn each<T: serde::de::DeserializeOwned>(v: Option<&serde_json::Value>) -> BTreeMap<String, T> {
            let Some(m) = v.and_then(|v| v.as_object()) else { return BTreeMap::new() };
            m.iter().filter_map(|(k, v)| serde_json::from_value(v.clone()).ok().map(|t| (k.clone(), t))).collect()
        }
        let v: serde_json::Value = serde_json::from_str(text).ok()?;
        let o = v.as_object()?;
        Some(Book { jobs: each(o.get("jobs")), results: each(o.get("results")), plans: each(o.get("plans")) })
    }

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
            command: ask.tests.command.clone(),
            timeout_minutes: ask.tests.timeout_minutes,
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
    /// when it ran to its end.
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

    /// The last `n` finished jobs in `sandbox`, newest first.
    pub fn latest(&self, sandbox: &str, n: usize) -> Vec<&Job> {
        let mut v: Vec<&Job> = self.jobs.values().filter(|j| j.sandbox == sandbox && j.state == JobState::Done).collect();
        v.sort_by_key(|j| std::cmp::Reverse((j.queued_ms, j.id.clone())));
        v.truncate(n);
        v
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

#[cfg(test)]
mod tests {
    use super::*;

    fn tests() -> &'static PlanTests {
        static T: std::sync::OnceLock<PlanTests> = std::sync::OnceLock::new();
        T.get_or_init(|| PlanTests { command: "npm test".into(), timeout_minutes: 5 })
    }

    fn ask<'a>(key: &'a str, plan: Option<&'a str>) -> Ask<'a> {
        Ask { sandbox: "sb", tests: tests(), key, commit: "c1", plan, by: "t1", rerun: false }
    }

    #[test]
    fn a_proposed_command_is_one_line_with_a_bounded_limit() {
        assert_eq!(PlanTests::proposed(None, Some(5)), Ok(None));
        assert_eq!(PlanTests::proposed(Some("  "), None), Ok(None));
        let t = PlanTests::proposed(Some(" npm test "), None).unwrap().unwrap();
        assert_eq!((t.command.as_str(), t.timeout_minutes), ("npm test", DEFAULT_TIMEOUT_MINUTES));
        assert_eq!(PlanTests::proposed(Some("x"), Some(0)).unwrap().unwrap().timeout_minutes, DEFAULT_TIMEOUT_MINUTES);
        assert_eq!(PlanTests::proposed(Some("x"), Some(999)).unwrap().unwrap().timeout_minutes, MAX_TIMEOUT_MINUTES);
        assert_eq!(PlanTests::proposed(Some("x"), Some(20)).unwrap().unwrap().timeout_minutes, 20);
        for bad in ["a\nb", "a\rb", "a\0b"] {
            assert!(PlanTests::proposed(Some(bad), None).unwrap_err().contains("one line"), "{bad:?}");
        }
        assert!(PlanTests::proposed(Some(&"x".repeat(MAX_COMMAND_BYTES + 1)), None).is_err());
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
    fn the_key_is_the_command_and_the_root_tree_only() {
        let (t1, t2) = ("1".repeat(40), "2".repeat(40));
        let k = cache_key("npm test", &t1);
        assert_eq!(cache_key("npm test", &t1), k, "the same command on the same tree");
        assert_ne!(cache_key("npm test", &t2), k, "another tree");
        assert_ne!(cache_key("npm run test", &t1), k, "another command");
        assert_ne!(cache_key("npm test\0tree\0", &t1), cache_key("npm test", &format!("\0tree\0{t1}")), "the parts are kept apart");
    }

    #[test]
    fn the_probe_is_parsed() {
        let (c, t) = ("c".repeat(40), "d".repeat(40));
        let p = parse_probe(&format!("commit={c}\ntree={t}\n")).unwrap();
        assert_eq!((p.commit.as_str(), p.tree.as_str()), (c.as_str(), t.as_str()));
        assert!(parse_probe(&format!("commit={c}\n")).is_none(), "no tree");
        assert!(parse_probe(&format!("commit=nothex\ntree={t}\n")).is_none());
    }

    #[test]
    fn a_book_of_the_earlier_format_loads_what_parses() {
        // Jobs that named a suite, with no command: left out; the rest kept.
        let old = r#"{"jobs":{"t-1":{"id":"t-1","sandbox":"sb","suite":"crates","key":"k","commit":"c","by":"lead","state":"done","queued_ms":1}},
            "results":{"sb k":{"outcome":"passed","commit":"c","duration_ms":1,"tail":"","attempts":1,"finished_ms":1},"sb bad":{"outcome":"weird"}},
            "plans":{"p-1":{"ran":2,"reused":1}}}"#;
        let b = Book::parse_lenient(old).unwrap();
        assert!(b.jobs.is_empty());
        assert_eq!(b.results.len(), 1);
        assert_eq!(b.counts("p-1"), Counts { ran: 2, reused: 1 });
        assert!(Book::parse_lenient("not json").is_none());
        assert!(Book::parse_lenient("[1]").is_none());
        let mut nb = Book::default();
        nb.request("j1".into(), &ask("k", Some("p")), 1);
        assert_eq!(Book::parse_lenient(&serde_json::to_string(&nb).unwrap()).unwrap(), nb, "the current format round-trips");
    }

    #[test]
    fn the_latest_finished_jobs_come_newest_first() {
        let mut b = Book::default();
        for (i, k) in ["k1", "k2", "k3"].iter().enumerate() {
            let id = format!("j{i}");
            b.request(id.clone(), &ask(k, None), i as u64);
            b.start_next("sb", 10);
            b.finish(&id, result(Outcome::Passed, 11));
        }
        let ids: Vec<&str> = b.latest("sb", 2).iter().map(|j| j.id.as_str()).collect();
        assert_eq!(ids, ["j2", "j1"]);
        assert_eq!(b.latest("sb", 2)[0].command, "npm test");
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
