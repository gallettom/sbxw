//! The test queue against a real git repository: the fake `sbx exec` runs
//! the probe and run scripts with `bash` on this machine, as the sandbox
//! would, with `HOME` in a temp dir.

use super::TestAsk;
use crate::app::App;
use crate::testing::{app_in_with, ls, sbx_answer};
use sbxw_core::config::Config;
use sbxw_core::plans::{PlanThread, ThreadState, Variant};
use sbxw_core::testq::{JobState, Outcome, Suite, Took};
use sbxw_core::{SandboxRef, ThreadName};
use sbxw_sbx::runner::{Fake, Output};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

fn has_tools() -> bool {
    ["git", "timeout"].iter().all(|t| std::process::Command::new(t).arg("--version").output().is_ok_and(|o| o.status.success()))
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.email=someone@example.invalid", "-c", "user.name=Ada", "-c", "commit.gpgsign=false", "-c", "init.defaultBranch=main"])
        .args(args)
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn commit_file(ws: &Path, file: &str, text: &str) -> String {
    let p = ws.join(file);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(&p, text).unwrap();
    git(ws, &["add", "-A"]);
    git(ws, &["commit", "-q", "-m", file]);
    git(ws, &["rev-parse", "HEAD"])
}

/// `sbx exec <name> -- bash -lc SCRIPT $0 args…`, run here with `bash -c`;
/// a run script whose command holds `sleep 30` gets a 1 s limit, so a
/// time-out takes a second.
fn local_exec(home: PathBuf) -> impl Fn(&[String]) -> Output + Send + Sync + 'static {
    move |a: &[String]| match a.first().map(String::as_str) {
        Some("ls") => ls(&[("myapp", "running")]),
        Some("exec") if a.get(3).is_some_and(|x| x == "bash") && a.get(6).is_some_and(|x| x.starts_with("sbxw-tests-")) => {
            let mut rest: Vec<String> = a[7..].to_vec();
            if a[6] == "sbxw-tests-run" && rest.get(3).is_some_and(|c| c.contains("sleep 30")) {
                *rest.last_mut().unwrap() = "=1".into();
            }
            let out = std::process::Command::new("bash").env("HOME", &home).args(["-c", &a[5], &a[6]]).args(&rest).output().unwrap();
            Output { code: out.status.code(), stdout: out.stdout, stderr: out.stderr }
        }
        _ => sbx_answer(a),
    }
}

struct Rig {
    state: tempfile::TempDir,
    ws: tempfile::TempDir,
    app: Arc<App>,
    fake: Arc<Fake>,
    sb: SandboxRef,
}

fn suite(name: &str, paths: &[&str], command: &str) -> Suite {
    Suite {
        name: name.into(),
        command: command.into(),
        paths: paths.iter().map(|s| s.to_string()).collect(),
        depends: vec![],
        timeout_minutes: 5,
        retries: 0,
        version_cmd: "echo tool-1".into(),
    }
}

/// A repo with `crates/a`, `crates/b` and `docs`, a sandbox `myapp` on it
/// brought up with `suites` (made by `make`, which gets a path prefix for
/// scratch files outside the repo).
fn rig(make: impl FnOnce(&dyn Fn(&str) -> String) -> Vec<Suite>) -> Rig {
    let state = tempfile::tempdir().unwrap();
    let ws = tempfile::tempdir().unwrap();
    git(ws.path(), &["init", "-q"]);
    commit_file(ws.path(), "crates/a/x", "1");
    commit_file(ws.path(), "crates/b/y", "1");
    commit_file(ws.path(), "docs/z", "1");
    let home = state.path().join("sandbox-home");
    std::fs::create_dir_all(&home).unwrap();
    let (app, fake) = app_in_with(state.path(), local_exec(home));
    let sb = SandboxRef::parse("myapp").unwrap();
    let scratch = |f: &str| state.path().join(f).to_string_lossy().into_owned();
    let suites = make(&scratch);
    configure(&app, &sb, ws.path(), suites);
    Rig { state, ws, app, fake, sb }
}

fn configure(app: &Arc<App>, sb: &SandboxRef, ws: &Path, suites: Vec<Suite>) {
    app.store
        .update(sb, |r| {
            r.workspace = Some(ws.to_path_buf());
            r.config_snapshot = Some(Box::new(Config { tests: suites, ..Config::default() }));
        })
        .unwrap();
}

fn ask<'a>(suite: &'a str, git_ref: &'a str) -> TestAsk<'a> {
    TestAsk { suite: Some(suite), git_ref: Some(git_ref), ..Default::default() }
}

async fn run(app: &Arc<App>, sb: &SandboxRef, a: TestAsk<'_>) -> (sbxw_core::testq::Job, Took) {
    let (job, took) = app.tests_request(sb, a).await.unwrap();
    (app.tests_wait(&job.id, sb, Duration::from_secs(60)).await.unwrap(), took)
}

fn lines(path: &str) -> usize {
    std::fs::read_to_string(path).map(|t| t.lines().count()).unwrap_or(0)
}

async fn until(what: &str, f: impl Fn() -> bool) {
    for _ in 0..400 {
        if f() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("timed out waiting for {what}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_undeclared_suite_and_a_command_shaped_argument_are_refused() {
    if !has_tools() {
        return;
    }
    let r = rig(|_| vec![suite("a", &["crates/a"], "true")]);
    for (s, rf, want) in [
        ("nope", "HEAD", "not a declared test suite"),
        ("a; rm -rf /", "HEAD", "not a declared test suite"),
        ("a", "$(touch /tmp/sbxw-pwned)", "refused"),
        ("a", "HEAD; id", "refused"),
        ("a", "--output=/tmp/x", "refused"),
        ("a", "no-such-branch", "is not a commit"),
    ] {
        let e = r.app.tests_request(&r.sb, ask(s, rf)).await.unwrap_err();
        assert!(e.contains(want), "{s} {rf}: {e}");
    }
    // Only the last one reached the sandbox, and only to be probed.
    let probes = r.fake.log().iter().filter(|c| c.contains("sbxw-tests-")).count();
    assert_eq!(probes, 1, "{:?}", r.fake.log());
    // A sandbox with no suites.
    configure(&r.app, &r.sb, r.ws.path(), vec![]);
    assert!(r.app.tests_request(&r.sb, ask("a", "HEAD")).await.unwrap_err().contains("declares no test suites"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_result_is_reused_until_a_covered_path_changes() {
    if !has_tools() {
        return;
    }
    let r = rig(|p| vec![suite("a", &["crates/a"], &format!("echo ran >> {}; echo checkout=$SBXW_TEST_CHECKOUT", p("count")))]);
    let count = r.state.path().join("count").to_string_lossy().into_owned();
    let first = git(r.ws.path(), &["rev-parse", "HEAD"]);
    let (j1, t1) = run(&r.app, &r.sb, ask("a", "HEAD")).await;
    assert_eq!(t1, Took::Queued);
    let res = j1.result.clone().unwrap();
    assert_eq!((res.outcome, j1.reused, res.commit.as_str()), (Outcome::Passed, false, first.as_str()));
    assert!(res.tail.contains("checkout=") && res.tail.contains(".sbxw-worktrees/.tests"), "{}", res.tail);
    assert!(res.log.ends_with("/.sbxw/tests/logs/a.log"), "{}", res.log);
    // An uncovered path changes: the stored result, marked reused, first commit.
    let second = commit_file(r.ws.path(), "docs/z", "2");
    let (j2, t2) = run(&r.app, &r.sb, ask("a", &second)).await;
    assert_eq!(t2, Took::Reused);
    assert!(j2.reused && j2.commit == second);
    assert_eq!(j2.result.unwrap().commit, first, "reused from the commit it first ran on");
    assert_eq!(lines(&count), 1);
    // A covered path changes: it runs again.
    commit_file(r.ws.path(), "crates/a/x", "2");
    let (j3, t3) = run(&r.app, &r.sb, ask("a", "main")).await;
    assert_eq!((t3, j3.reused), (Took::Queued, false));
    assert_eq!(lines(&count), 2);
    // rerun runs the same code again.
    let (_, t4) = run(&r.app, &r.sb, TestAsk { rerun: true, ..ask("a", "main") }).await;
    assert_eq!(t4, Took::Queued);
    assert_eq!(lines(&count), 3);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn one_job_at_a_time_and_a_duplicate_joins() {
    if !has_tools() {
        return;
    }
    let r = rig(|p| {
        vec![
            suite("slow", &["crates/a"], &format!("while [ ! -e {} ]; do sleep 0.05; done; echo slow-done", p("gate"))),
            suite("other", &["crates/b"], &format!("echo other-ran >> {}", p("order"))),
        ]
    });
    let gate = r.state.path().join("gate");
    let (slow, t) = r.app.tests_request(&r.sb, ask("slow", "HEAD")).await.unwrap();
    assert_eq!(t, Took::Queued);
    let app = r.app.clone();
    let sb = r.sb.to_string();
    let id = slow.id.clone();
    until("the slow job to run", || app.tests.lock().unwrap().get(&id, &sb).unwrap().state == JobState::Running).await;
    let (dup, t) = r.app.tests_request(&r.sb, ask("slow", "main")).await.unwrap();
    assert_eq!((t, dup.id.as_str()), (Took::Joined, slow.id.as_str()), "the same code joins the job at work");
    let (other, t) = r.app.tests_request(&r.sb, ask("other", "HEAD")).await.unwrap();
    assert_eq!(t, Took::Queued);
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(r.app.tests.lock().unwrap().get(&other.id, &sb).unwrap().state, JobState::Queued, "the second waits");
    let (active, _) = r.app.tests_jobs(&r.sb);
    assert_eq!(active.len(), 2);
    std::fs::write(&gate, "").unwrap();
    let s = r.app.tests_wait(&slow.id, &r.sb, Duration::from_secs(30)).await.unwrap();
    let o = r.app.tests_wait(&other.id, &r.sb, Duration::from_secs(30)).await.unwrap();
    assert_eq!(s.result.as_ref().unwrap().outcome, Outcome::Passed);
    assert_eq!(o.result.as_ref().unwrap().outcome, Outcome::Passed);
    assert!(o.started_ms.unwrap() >= s.result.unwrap().finished_ms, "the second started once the first was over");
    assert_eq!(r.app.tests.lock().unwrap().jobs[&slow.id].joined, 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_timeout_is_answered_and_not_stored() {
    if !has_tools() {
        return;
    }
    let r = rig(|_| vec![suite("hang", &["crates/a"], "echo started; sleep 30")]);
    let (j, _) = run(&r.app, &r.sb, ask("hang", "HEAD")).await;
    let res = j.result.unwrap();
    assert_eq!(res.outcome, Outcome::TimedOut, "{}", res.tail);
    assert!(res.tail.contains("over its limit") && res.tail.contains("started"), "{}", res.tail);
    assert!(r.app.tests.lock().unwrap().results.is_empty(), "a time-out is never reused");
    let (_, t) = run(&r.app, &r.sb, ask("hang", "HEAD")).await;
    assert_eq!(t, Took::Queued);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_failure_is_retried_once_and_the_retry_recorded() {
    if !has_tools() {
        return;
    }
    let r = rig(|p| {
        let flag = p("flag");
        vec![
            Suite { retries: 1, ..suite("flaky", &["crates/a"], &format!("if [ -e {flag} ]; then echo second; else touch {flag}; echo first; exit 1; fi")) },
            Suite { retries: 1, ..suite("broken", &["crates/b"], "echo nope; exit 2") },
            suite("once", &["docs"], "exit 3"),
        ]
    });
    let (j, _) = run(&r.app, &r.sb, ask("flaky", "HEAD")).await;
    let res = j.result.unwrap();
    assert_eq!((res.outcome, res.attempts, res.first_outcome), (Outcome::Passed, 2, Some(Outcome::Failed)));
    assert!(res.retried() && res.tail.contains("second"), "{}", res.tail);
    let (j, _) = run(&r.app, &r.sb, ask("broken", "HEAD")).await;
    let res = j.result.unwrap();
    assert_eq!((res.outcome, res.attempts), (Outcome::Failed, 2));
    assert!(res.tail.contains("nope"));
    let (j, _) = run(&r.app, &r.sb, ask("once", "HEAD")).await;
    let res = j.result.unwrap();
    assert_eq!((res.outcome, res.attempts, res.first_outcome), (Outcome::Failed, 1, None));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn results_and_jobs_survive_a_daemon_restart() {
    if !has_tools() {
        return;
    }
    let r = rig(|p| {
        vec![
            suite("a", &["crates/a"], "echo a"),
            suite("slow", &["crates/b"], &format!("while [ ! -e {} ]; do sleep 0.05; done", p("gate"))),
        ]
    });
    let (done, _) = run(&r.app, &r.sb, ask("a", "HEAD")).await;
    let (slow, _) = r.app.tests_request(&r.sb, ask("slow", "HEAD")).await.unwrap();
    let (app, sb) = (r.app.clone(), r.sb.to_string());
    let id = slow.id.clone();
    until("the slow job to run", || app.tests.lock().unwrap().get(&id, &sb).unwrap().state == JobState::Running).await;
    // The daemon restarts: a new App on the same state directory.
    let home = r.state.path().join("sandbox-home");
    let (app2, _) = app_in_with(r.state.path(), local_exec(home));
    configure(&app2, &r.sb, r.ws.path(), r.app.tests_suites(&r.sb).await);
    assert_eq!(app2.tests.lock().unwrap().get(&slow.id, &sb).unwrap().state, JobState::Queued, "what ran runs again");
    let again = app2.tests_wait(&done.id, &r.sb, Duration::from_secs(1)).await.unwrap();
    assert_eq!(again.result.unwrap().outcome, Outcome::Passed, "a job id outlives the restart");
    let (j, t) = run(&app2, &r.sb, ask("a", "HEAD")).await;
    assert_eq!((t, j.reused), (Took::Reused, true), "a stored result outlives it");
    std::fs::write(r.state.path().join("gate"), "").unwrap();
    app2.tests_resume();
    let s = app2.tests_wait(&slow.id, &r.sb, Duration::from_secs(30)).await.unwrap();
    assert_eq!(s.result.unwrap().outcome, Outcome::Passed);
}

fn thread(name: &str) -> PlanThread {
    PlanThread {
        name: name.into(),
        goal: "g".into(),
        scope: String::new(),
        repo: None,
        model: None,
        state: ThreadState::Starting,
        summary: String::new(),
        updated_ms: 0,
        set_by: None,
        ticket: None,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_plan_names_its_team_suite_and_counts_its_jobs() {
    if !has_tools() {
        return;
    }
    let r = rig(|_| vec![suite("a", &["crates/a"], "echo a"), suite("b", &["crates/b"], "echo b")]);
    let e = r
        .app
        .plan_propose_with_suite(&r.sb, "s", vec![thread("api")], Variant::Split, "one", Some("lead"), Some("nope"))
        .await
        .unwrap_err();
    assert!(e.contains("not a declared test suite") && e.contains("a, b"), "{e}");
    assert!(r.app.plans_list().is_empty(), "nothing proposed");
    let p = r.app.plan_propose_with_suite(&r.sb, "s", vec![thread("api")], Variant::Split, "one", Some("lead"), Some("a")).await.unwrap();
    assert_eq!(p.team_suite.as_deref(), Some("a"));
    r.app.plans.lock().unwrap().approve(&p.id, vec![thread("api")], None, None, 1).unwrap();
    assert_eq!(r.app.plan(&p.id).unwrap().team_suite.as_deref(), Some("a"), "approval keeps it");
    // A thread asks with no suite: the team's, on its HEAD.
    let api = ThreadName::parse("api").unwrap();
    let head = git(r.ws.path(), &["rev-parse", "HEAD"]);
    let (j, _) = run(&r.app, &r.sb, TestAsk { thread: Some(&api), head: Some(&head), ..Default::default() }).await;
    assert_eq!((j.suite.as_str(), j.plan.as_deref(), j.by.as_str()), ("a", Some(p.id.as_str()), "api"));
    // Another suite on its own commit, then the team suite again: reused.
    run(&r.app, &r.sb, TestAsk { thread: Some(&api), suite: Some("b"), head: Some(&head), ..Default::default() }).await;
    let (_, t) = run(&r.app, &r.sb, TestAsk { thread: Some(&api), head: Some(&head), ..Default::default() }).await;
    assert_eq!(t, Took::Reused);
    let view = r.app.plan_view(&r.app.plan(&p.id).unwrap());
    assert_eq!(view["team_suite"], "a");
    assert_eq!((view["tests"]["ran"].as_u64(), view["tests"]["reused"].as_u64()), (Some(2), Some(1)));
    // No team suite and no name: refused with the list.
    let p2 = r.app.plan_propose(&r.sb, "s", vec![thread("web")], Variant::Split, "one", Some("lead")).await;
    assert!(p2.is_ok());
    let none = TestAsk { thread: Some(&ThreadName::parse("web").unwrap()), head: Some(&head), ..Default::default() };
    assert!(r.app.tests_request(&r.sb, none).await.unwrap_err().contains("name a suite"));
}
