//! The test queue against a real git repository: the fake `sbx exec` runs
//! the probe and run scripts with `bash` on this machine, as the sandbox
//! would, with `HOME` in a temp dir.

use super::TestAsk;
use crate::app::App;
use crate::testing::{app_in_with, ls, sbx_answer};
use sbxw_core::plans::{PlanThread, TestsChoice, ThreadState, Variant};
use sbxw_core::testq::{JobState, Outcome, PlanTests, Took};
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

impl Rig {
    /// A scratch file outside the repository.
    fn scratch(&self, f: &str) -> String {
        self.state.path().join(f).to_string_lossy().into_owned()
    }

    /// An approved split plan with thread `thread` and test command
    /// `command` (none: `None`), as the person approved it.
    fn plan(&self, thread: &str, command: Option<&str>, choice: TestsChoice) -> String {
        let mut b = self.app.plans.lock().unwrap();
        let id = format!("p-{thread}");
        b.propose(id.clone(), &self.sb.to_string(), "s", vec![t(thread)], Variant::Split, "one", Some("lead"), 1).unwrap();
        b.note_proposed_tests(&id, PlanTests::proposed(command, Some(5)).unwrap());
        b.approve_with(&id, vec![t(thread)], None, None, choice, 2).unwrap();
        id
    }

    /// A thread's request on `git_ref`.
    fn ask<'a>(&self, thread: &'a ThreadName, git_ref: &'a str) -> TestAsk<'a> {
        TestAsk { thread: Some(thread), git_ref: Some(git_ref), ..Default::default() }
    }
}

fn t(name: &str) -> PlanThread {
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

fn name(n: &str) -> ThreadName {
    ThreadName::parse(n).unwrap()
}

/// A repo with `crates/a`, `crates/b` and `docs`, and a sandbox `myapp` on it.
fn rig() -> Rig {
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
    configure(&app, &sb, ws.path());
    Rig { state, ws, app, fake, sb }
}

fn configure(app: &Arc<App>, sb: &SandboxRef, ws: &Path) {
    app.store.update(sb, |r| r.workspace = Some(ws.to_path_buf())).unwrap();
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

fn probes(fake: &Fake) -> usize {
    fake.log().iter().filter(|c| c.contains("sbxw-tests-")).count()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn outside_a_plan_with_an_approved_command_run_tests_is_refused() {
    if !has_tools() {
        return;
    }
    let r = rig();
    // No plan at all, from the lead or a thread.
    let e = r.app.tests_request(&r.sb, TestAsk { git_ref: Some("HEAD"), ..Default::default() }).await.unwrap_err();
    assert!(e.contains("no test command") && e.contains("lead proposes the project's test command with the plan"), "{e}");
    let loose = name("loose");
    assert!(r.app.tests_request(&r.sb, r.ask(&loose, "HEAD")).await.unwrap_err().contains("no test command"));
    // A plan proposed with none, and one whose command the person cleared.
    r.plan("bare", None, TestsChoice::Approve);
    r.plan("cleared", Some("echo should-not-run"), TestsChoice::Clear);
    for th in ["bare", "cleared"] {
        let th = name(th);
        assert!(r.app.tests_request(&r.sb, r.ask(&th, "HEAD")).await.unwrap_err().contains("no test command"), "{th}");
    }
    let (_, latest) = r.app.tests_jobs(&r.sb);
    assert!(latest.is_empty());
    assert_eq!(probes(&r.fake), 0, "nothing reached the sandbox: {:?}", r.fake.log());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_ref_that_is_not_a_revision_is_refused_before_the_sandbox() {
    if !has_tools() {
        return;
    }
    let r = rig();
    r.plan("api", Some("true"), TestsChoice::Approve);
    let api = name("api");
    for (rf, want) in [
        ("$(touch /tmp/sbxw-pwned)", "refused"),
        ("HEAD; id", "refused"),
        ("--output=/tmp/x", "refused"),
        ("no-such-branch", "is not a commit"),
    ] {
        let e = r.app.tests_request(&r.sb, r.ask(&api, rf)).await.unwrap_err();
        assert!(e.contains(want), "{rf}: {e}");
    }
    assert_eq!(probes(&r.fake), 1, "only the last one was probed: {:?}", r.fake.log());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_result_is_reused_for_the_same_command_on_the_same_tree() {
    if !has_tools() {
        return;
    }
    let r = rig();
    let count = r.scratch("count");
    let cmd = format!("echo ran >> {count}; echo checkout=$SBXW_TEST_CHECKOUT");
    let plan = r.plan("api", Some(&cmd), TestsChoice::Approve);
    let api = name("api");
    let first = git(r.ws.path(), &["rev-parse", "HEAD"]);
    let (j1, t1) = run(&r.app, &r.sb, r.ask(&api, "HEAD")).await;
    assert_eq!(t1, Took::Queued);
    let res = j1.result.clone().unwrap();
    assert_eq!((res.outcome, j1.reused, res.commit.as_str()), (Outcome::Passed, false, first.as_str()));
    assert_eq!((j1.command.as_str(), j1.plan.as_deref(), j1.by.as_str()), (cmd.as_str(), Some(plan.as_str()), "api"));
    assert!(res.tail.contains("checkout=") && res.tail.contains(".sbxw-worktrees/.tests"), "{}", res.tail);
    assert!(res.log.ends_with(&format!("/.sbxw/tests/logs/{plan}.log")), "{}", res.log);
    // Another commit with the same root tree: the stored result, marked
    // reused, with the commit it first ran on.
    git(r.ws.path(), &["commit", "-q", "--allow-empty", "-m", "empty"]);
    let second = git(r.ws.path(), &["rev-parse", "HEAD"]);
    assert_ne!(second, first);
    let (j2, t2) = run(&r.app, &r.sb, r.ask(&api, &second)).await;
    assert_eq!(t2, Took::Reused);
    assert!(j2.reused && j2.commit == second);
    assert_eq!(j2.result.unwrap().commit, first, "reused from the commit it first ran on");
    // Another thread of another plan, same command, same tree: reused too.
    r.plan("web", Some(&cmd), TestsChoice::Approve);
    let web = name("web");
    let (_, t) = run(&r.app, &r.sb, r.ask(&web, "HEAD")).await;
    assert_eq!(t, Took::Reused);
    assert_eq!(lines(&count), 1);
    // Any file changes, docs included: the tree is new, it runs again.
    commit_file(r.ws.path(), "docs/z", "2");
    let (j3, t3) = run(&r.app, &r.sb, r.ask(&api, "main")).await;
    assert_eq!((t3, j3.reused), (Took::Queued, false));
    assert_eq!(lines(&count), 2);
    // rerun runs the same code again.
    let (_, t4) = run(&r.app, &r.sb, TestAsk { rerun: true, ..r.ask(&api, "main") }).await;
    assert_eq!(t4, Took::Queued);
    assert_eq!(lines(&count), 3);
    // The plan counts what ran and what was answered without running.
    let view = r.app.plan_view(&r.app.plan(&plan).unwrap());
    assert_eq!((view["tests"]["ran"].as_u64(), view["tests"]["reused"].as_u64()), (Some(3), Some(1)));
    assert_eq!(view["approved_tests"]["command"], cmd.as_str());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_different_command_on_the_same_tree_runs_again() {
    if !has_tools() {
        return;
    }
    let r = rig();
    let count = r.scratch("count");
    r.plan("api", Some(&format!("echo a >> {count}")), TestsChoice::Approve);
    r.plan("web", Some(&format!("echo b >> {count}")), TestsChoice::Approve);
    let (api, web) = (name("api"), name("web"));
    let (_, t1) = run(&r.app, &r.sb, r.ask(&api, "HEAD")).await;
    let (j2, t2) = run(&r.app, &r.sb, r.ask(&web, "HEAD")).await;
    assert_eq!((t1, t2, j2.reused), (Took::Queued, Took::Queued, false));
    assert_eq!(lines(&count), 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn one_job_at_a_time_and_a_duplicate_joins() {
    if !has_tools() {
        return;
    }
    let r = rig();
    let gate = r.scratch("gate");
    r.plan("slow", Some(&format!("while [ ! -e {gate} ]; do sleep 0.05; done; echo slow-done")), TestsChoice::Approve);
    r.plan("other", Some("echo other-ran"), TestsChoice::Approve);
    let (slow_t, other_t) = (name("slow"), name("other"));
    let (slow, t) = r.app.tests_request(&r.sb, r.ask(&slow_t, "HEAD")).await.unwrap();
    assert_eq!(t, Took::Queued);
    let app = r.app.clone();
    let sb = r.sb.to_string();
    let id = slow.id.clone();
    until("the slow job to run", || app.tests.lock().unwrap().get(&id, &sb).unwrap().state == JobState::Running).await;
    let (dup, t) = r.app.tests_request(&r.sb, r.ask(&slow_t, "main")).await.unwrap();
    assert_eq!((t, dup.id.as_str()), (Took::Joined, slow.id.as_str()), "the same code joins the job at work");
    let (other, t) = r.app.tests_request(&r.sb, r.ask(&other_t, "HEAD")).await.unwrap();
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
async fn a_timeout_is_answered_not_retried_and_not_stored() {
    if !has_tools() {
        return;
    }
    let r = rig();
    r.plan("hang", Some("echo started; sleep 30"), TestsChoice::Approve);
    let hang = name("hang");
    let (j, _) = run(&r.app, &r.sb, r.ask(&hang, "HEAD")).await;
    let res = j.result.unwrap();
    assert_eq!((res.outcome, res.attempts), (Outcome::TimedOut, 1), "{}", res.tail);
    assert!(res.tail.contains("over its limit") && res.tail.contains("started"), "{}", res.tail);
    assert!(r.app.tests.lock().unwrap().results.is_empty(), "a time-out is never reused");
    let (_, t) = run(&r.app, &r.sb, r.ask(&hang, "HEAD")).await;
    assert_eq!(t, Took::Queued);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_failure_is_retried_once_and_the_retry_recorded() {
    if !has_tools() {
        return;
    }
    let r = rig();
    let flag = r.scratch("flag");
    r.plan("flaky", Some(&format!("if [ -e {flag} ]; then echo second; else touch {flag}; echo first; exit 1; fi")), TestsChoice::Approve);
    r.plan("broken", Some("echo nope; exit 2"), TestsChoice::Approve);
    let (j, _) = run(&r.app, &r.sb, r.ask(&name("flaky"), "HEAD")).await;
    let res = j.result.unwrap();
    assert_eq!((res.outcome, res.attempts, res.first_outcome), (Outcome::Passed, 2, Some(Outcome::Failed)));
    assert!(res.retried() && res.tail.contains("second"), "{}", res.tail);
    let (j, _) = run(&r.app, &r.sb, r.ask(&name("broken"), "HEAD")).await;
    let res = j.result.unwrap();
    assert_eq!((res.outcome, res.attempts), (Outcome::Failed, 2));
    assert!(res.tail.contains("nope"));
    // A failure that ran to its end is stored.
    let (_, t) = run(&r.app, &r.sb, r.ask(&name("broken"), "HEAD")).await;
    assert_eq!(t, Took::Reused);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn results_and_jobs_survive_a_daemon_restart() {
    if !has_tools() {
        return;
    }
    let r = rig();
    let gate = r.scratch("gate");
    r.plan("a", Some("echo a"), TestsChoice::Approve);
    r.plan("slow", Some(&format!("while [ ! -e {gate} ]; do sleep 0.05; done")), TestsChoice::Approve);
    let (done, _) = run(&r.app, &r.sb, r.ask(&name("a"), "HEAD")).await;
    let (slow, _) = r.app.tests_request(&r.sb, r.ask(&name("slow"), "HEAD")).await.unwrap();
    let (app, sb) = (r.app.clone(), r.sb.to_string());
    let id = slow.id.clone();
    until("the slow job to run", || app.tests.lock().unwrap().get(&id, &sb).unwrap().state == JobState::Running).await;
    // The daemon restarts: a new App on the same state directory.
    r.app.persist_plans(&r.sb.to_string());
    let home = r.state.path().join("sandbox-home");
    let (app2, _) = app_in_with(r.state.path(), local_exec(home));
    configure(&app2, &r.sb, r.ws.path());
    assert_eq!(app2.tests.lock().unwrap().get(&slow.id, &sb).unwrap().state, JobState::Queued, "what ran runs again");
    let again = app2.tests_wait(&done.id, &r.sb, Duration::from_secs(1)).await.unwrap();
    assert_eq!(again.result.unwrap().outcome, Outcome::Passed, "a job id outlives the restart");
    let (j, t) = run(&app2, &r.sb, r.ask(&name("a"), "HEAD")).await;
    assert_eq!((t, j.reused), (Took::Reused, true), "a stored result outlives it");
    std::fs::write(&gate, "").unwrap();
    app2.tests_resume();
    let s = app2.tests_wait(&slow.id, &r.sb, Duration::from_secs(30)).await.unwrap();
    assert_eq!(s.result.unwrap().outcome, Outcome::Passed);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_results_file_of_the_earlier_format_does_not_stop_the_daemon() {
    let state = tempfile::tempdir().unwrap();
    let file = app_in_with(state.path(), sbx_answer).0.tests_file();
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    let old = r#"{"jobs":{"t-1":{"id":"t-1","sandbox":"myapp","suite":"crates","key":"k","commit":"c","by":"lead","state":"running","queued_ms":1}},"results":{},"plans":{"p-1":{"ran":1,"reused":0}}}"#;
    std::fs::write(&file, old).unwrap();
    let (app, _) = app_in_with(state.path(), sbx_answer);
    assert!(app.tests.lock().unwrap().jobs.is_empty(), "a job naming a suite is left out");
    assert_eq!(app.tests_counts("p-1").ran, 1, "what parses is kept");
    std::fs::write(&file, "{ not json").unwrap();
    let (app, _) = app_in_with(state.path(), sbx_answer);
    assert!(app.tests.lock().unwrap().jobs.is_empty());
}

/// The agent route, as the relay calls it: an extra argument shaped like a
/// command (or the old suite name) is not read; the plan's command runs.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn run_tests_never_takes_a_command_from_the_request() {
    use tower::ServiceExt;
    if !has_tools() {
        return;
    }
    let r = rig();
    let pwned = r.scratch("pwned");
    r.plan("api", Some("echo plan-command"), TestsChoice::Approve);
    let token = r.app.store.token(r.sb.clone()).unwrap();
    let body = serde_json::json!({
        "thread": "api", "ref": "HEAD", "timeout": 30,
        "command": format!("touch {pwned}"), "test_command": format!("touch {pwned}"), "suite": format!("x; touch {pwned}"),
    });
    let req = axum::http::Request::builder()
        .method("POST")
        .uri("/api/tests/run")
        .header("content-type", "application/json")
        .header("authorization", format!("Bearer {token}"))
        .body(axum::body::Body::from(body.to_string()))
        .unwrap();
    let res = crate::http::agent_router(r.app.clone()).oneshot(req).await.unwrap();
    let status = res.status();
    let v: serde_json::Value = serde_json::from_slice(&axum::body::to_bytes(res.into_body(), 1 << 20).await.unwrap()).unwrap();
    assert!(status.is_success(), "{status} {v}");
    assert_eq!(v["job"]["command"], "echo plan-command", "{v}");
    assert!(v["job"]["result"]["tail"].as_str().unwrap_or("").contains("plan-command"), "{v}");
    assert!(!Path::new(&pwned).exists(), "the request's command never ran");
}

/// `list_tests` shows the caller's plan command, or that there is none.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_plan_a_request_runs_under_is_the_callers() {
    if !has_tools() {
        return;
    }
    let r = rig();
    let api = r.plan("api", Some("npm test"), TestsChoice::Approve);
    r.plan("web", Some("npm test"), TestsChoice::Clear);
    let p = r.app.tests_plan(&r.sb, Some(&name("api")), None).unwrap();
    assert_eq!(p.id, api);
    let v = super::plan_tests_view(Some(&p));
    assert_eq!((v["plan"].as_str(), v["tests"]["command"].as_str(), v["tests"]["timeout_minutes"].as_u64()), (Some(api.as_str()), Some("npm test"), Some(5)));
    let w = super::plan_tests_view(r.app.tests_plan(&r.sb, Some(&name("web")), None).as_ref());
    assert!(w["tests"].is_null() && w["not_approved"].as_str().unwrap().contains("cleared"), "{w}");
    assert!(super::plan_tests_view(None)["plan"].is_null());
    // The lead, naming no plan: the latest approved plan still open.
    assert_eq!(r.app.tests_plan(&r.sb, None, Some(&api)).unwrap().id, api);
    assert!(r.app.tests_plan(&r.sb, None, None).is_some());
}
