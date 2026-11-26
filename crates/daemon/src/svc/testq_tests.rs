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
    assert!(res.log.ends_with(&format!("/.sbxw/tests/logs/{plan}/{}-1.log", j1.id)), "{}", res.log);
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
    let flaky = r.app.tests_plan(&r.sb, Some(&name("flaky")), None).unwrap().id;
    let (j, _) = run(&r.app, &r.sb, r.ask(&name("flaky"), "HEAD")).await;
    let res = j.result.clone().unwrap();
    assert_eq!((res.outcome, res.attempts, res.first_outcome), (Outcome::Passed, 2, Some(Outcome::Failed)));
    assert!(res.retried() && res.tail.contains("second"), "{}", res.tail);
    // Each attempt kept its own log, and the result carries the first.
    let first = res.first.clone().expect("the first attempt is kept");
    assert_eq!(first.outcome, Outcome::Failed);
    assert!(first.log.ends_with(&format!("/logs/{flaky}/{}-1.log", j.id)), "{}", first.log);
    assert!(res.log.ends_with(&format!("/logs/{flaky}/{}-2.log", j.id)), "{}", res.log);
    assert!(first.tail.contains("first") && !first.tail.contains("second"), "{}", first.tail);
    assert!(!res.tail.contains("first"), "{}", res.tail);
    assert_eq!(std::fs::read_to_string(&first.log).unwrap().trim(), "first");
    assert_eq!(std::fs::read_to_string(&res.log).unwrap().trim(), "second");
    assert!(res.duration_ms >= first.duration_ms);
    // Counted once it has run, as one job; the Board's line names the first log.
    let view = r.app.plan_view(&r.app.plan(&flaky).unwrap());
    assert_eq!((view["tests"]["ran"].as_u64(), view["numbers"]["tests"]["run"].as_u64()), (Some(1), Some(1)));
    let l = job_lines(&r.app, &flaky);
    let text = l[0]["text"].as_str().unwrap();
    assert!(text.contains("retried once") && text.contains(&first.log), "{text}");
    assert_eq!((l[0]["first_outcome"].as_str(), l[0]["first_log"].as_str()), (Some("failed"), Some(first.log.as_str())));
    assert!(!l[0].to_string().contains("tail"), "never the output: {l:?}");
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

/// The plan dialog's Approve: the proposed command is approved with the
/// plan, unless the person cleared it, and then `run_tests` is refused.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_dialog_approves_the_command_or_clears_it() {
    use tower::ServiceExt;
    if !has_tools() {
        return;
    }
    let r = rig();
    for (thread, clear) in [("kept", false), ("dropped", true)] {
        let id = format!("p-{thread}");
        {
            let mut b = r.app.plans.lock().unwrap();
            b.propose(id.clone(), &r.sb.to_string(), "s", vec![t(thread)], Variant::Split, "one", Some("lead"), 1).unwrap();
            b.note_proposed_tests(&id, PlanTests::proposed(Some("echo ok"), None).unwrap());
        }
        assert!(r.app.plan(&id).unwrap().approved_tests.is_none(), "not before the approval");
        let body = serde_json::json!({ "threads": [t(thread)], "clear_tests": clear });
        let req = axum::http::Request::builder()
            .method("POST")
            .uri(format!("/api/plans/{id}/approve"))
            .header("host", "127.0.0.1:7681")
            .header("content-type", "application/json")
            .body(axum::body::Body::from(body.to_string()))
            .unwrap();
        let res = crate::testing::signed_in(r.app.clone()).oneshot(req).await.unwrap();
        assert!(res.status().is_success(), "{}", res.status());
        let plan = r.app.plan(&id).unwrap();
        assert_eq!(plan.approved_tests.is_some(), !clear, "{thread}");
        let asked = r.app.tests_request(&r.sb, r.ask(&name(thread), "HEAD")).await;
        if clear {
            assert!(asked.unwrap_err().contains("no test command"));
            assert!(plan.tests_not_approved.unwrap().contains("cleared"));
        } else {
            assert!(asked.is_ok());
        }
    }
}

/// The plan's test-job lines, oldest first.
fn job_lines(app: &Arc<App>, plan: &str) -> Vec<serde_json::Value> {
    let log = app.plans.lock().unwrap().log_of(plan);
    log.entries
        .iter()
        .filter(|e| e.kind == sbxw_core::team::EventKind::TestJob)
        .map(|e| {
            let mut d = e.data.clone();
            d["from"] = e.from.clone().into();
            d["text"] = e.text.clone().into();
            d
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn each_answered_request_is_a_line_on_its_plans_timeline() {
    if !has_tools() {
        return;
    }
    let r = rig();
    let api = r.plan("api", Some("echo ok"), TestsChoice::Approve);
    let head = git(r.ws.path(), &["rev-parse", "HEAD"]);
    // Passed, then the same tree again: a stored result.
    run(&r.app, &r.sb, r.ask(&name("api"), "HEAD")).await;
    run(&r.app, &r.sb, r.ask(&name("api"), "HEAD")).await;
    let l = job_lines(&r.app, &api);
    assert_eq!(l.len(), 2, "{l:?}");
    assert_eq!((l[0]["outcome"].as_str(), l[0]["reused"].as_bool(), l[0]["retried"].as_bool()), (Some("passed"), Some(false), Some(false)));
    assert_eq!((l[0]["from"].as_str(), l[0]["asker"].as_str(), l[0]["thread"].as_str()), (Some("api"), Some("api"), Some("api")));
    assert_eq!(l[0]["short"].as_str(), Some(&head[..7]));
    assert!(l[0]["duration_ms"].is_u64() && l[0]["job"].as_str().is_some_and(|j| j.starts_with("t-")));
    assert_eq!((l[1]["reused"].as_bool(), l[1]["reuse"].as_str()), (Some(true), Some("stored")));
    assert!(l.iter().all(|d| !d.to_string().contains("tail")), "no output on the timeline: {l:?}");
    // Failed after a retry, and timed out.
    let broken = r.plan("broken", Some("echo nope; exit 2"), TestsChoice::Approve);
    run(&r.app, &r.sb, r.ask(&name("broken"), "HEAD")).await;
    let l = job_lines(&r.app, &broken);
    assert_eq!((l[0]["outcome"].as_str(), l[0]["retried"].as_bool()), (Some("failed"), Some(true)));
    assert!(l[0]["text"].as_str().unwrap().contains("failed") && l[0]["text"].as_str().unwrap().contains("retried once"), "{l:?}");
    let hang = r.plan("hang", Some("sleep 30"), TestsChoice::Approve);
    run(&r.app, &r.sb, r.ask(&name("hang"), "HEAD")).await;
    assert_eq!(job_lines(&r.app, &hang)[0]["outcome"].as_str(), Some("timed_out"));
    // A join of a job at work: a line for the first asker and one for the
    // joiner (the lead naming the plan), marked joined, when it finishes.
    let gate = r.scratch("gate");
    let slow = r.plan("slow", Some(&format!("while [ ! -e {gate} ]; do sleep 0.05; done")), TestsChoice::Approve);
    let (job, _) = r.app.tests_request(&r.sb, r.ask(&name("slow"), "HEAD")).await.unwrap();
    let (j2, took) = r.app.tests_request(&r.sb, TestAsk { git_ref: Some("HEAD"), plan: Some(&slow), ..Default::default() }).await.unwrap();
    assert_eq!((took, j2.id.as_str()), (Took::Joined, job.id.as_str()));
    assert!(job_lines(&r.app, &slow).is_empty(), "nothing until it finishes");
    std::fs::write(&gate, "").unwrap();
    r.app.tests_wait(&job.id, &r.sb, Duration::from_secs(30)).await.unwrap();
    let l = job_lines(&r.app, &slow);
    assert_eq!(l.len(), 2, "{l:?}");
    assert_eq!((l[0]["from"].as_str(), l[0]["reused"].as_bool()), (Some("slow"), Some(false)));
    assert_eq!((l[1]["from"].as_str(), l[1]["reuse"].as_str(), l[1]["thread"].as_str()), (Some("lead"), Some("joined"), None));
    // The counts and the numbers agree.
    let n = r.app.plan_view(&r.app.plan(&api).unwrap())["numbers"].clone();
    assert_eq!((n["tests"]["run"].as_u64(), n["tests"]["reused"].as_u64(), n["tests"]["refused"].as_u64()), (Some(1), Some(1), Some(0)));
}

async fn hook_post(app: &Arc<App>, sb: &SandboxRef, body: serde_json::Value) -> serde_json::Value {
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;
    let token = app.store.token(sb.clone()).unwrap();
    let req = Request::builder()
        .method("POST")
        .uri("/api/hook")
        .header("content-type", "application/json")
        .header("authorization", format!("Bearer {token}"))
        .body(Body::from(body.to_string()))
        .unwrap();
    let res = crate::http::agent_router(app.clone()).oneshot(req).await.unwrap();
    let bytes = axum::body::to_bytes(res.into_body(), 1 << 20).await.unwrap();
    serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null)
}

fn bash(thread: Option<&str>, command: &str) -> serde_json::Value {
    let mut ev = serde_json::json!({
        "hook_event_name": "PreToolUse", "session_id": "s1", "hook_version": 4,
        "tool_name": "Bash", "tool_input": { "command": command },
    });
    if let Some(t) = thread {
        ev["sbxw_thread"] = t.into();
    }
    ev
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_thread_running_the_plans_whole_test_command_is_sent_to_the_queue() {
    let r = rig();
    let plan = r.plan("api", Some("tools/check   all"), TestsChoice::Approve);
    r.plan("bare", None, TestsChoice::Approve);
    let denied = |v: &serde_json::Value| v["hookSpecificOutput"]["permissionDecision"] == "deny";
    for cmd in ["tools/check all", "cd /w/app && tools/check all", "  tools/check \t all  ", "git commit -qm x; tools/check all 2>&1 | tail -40"] {
        let v = hook_post(&r.app, &r.sb, bash(Some("api"), cmd)).await;
        assert!(denied(&v), "{cmd:?}: {v}");
        assert_eq!(v["hookSpecificOutput"]["hookEventName"], "PreToolUse");
        let why = v["hookSpecificOutput"]["permissionDecisionReason"].as_str().unwrap();
        assert!(why.contains("run_tests") && why.contains("CI branch") && why.contains("Run only the tests of what you touched"), "{why}");
    }
    // A narrower command, the lead (no thread), a plan with no command, another tool.
    for (thread, cmd) in [(Some("api"), "tools/check core"), (Some("api"), "tools/check all --only x"), (None, "tools/check all"), (Some("bare"), "tools/check all")] {
        let v = hook_post(&r.app, &r.sb, bash(thread, cmd)).await;
        assert!(!denied(&v), "{thread:?} {cmd:?}: {v}");
    }
    let mut read = bash(Some("api"), "tools/check all");
    read["tool_name"] = "Read".into();
    assert!(!denied(&hook_post(&r.app, &r.sb, read).await));
    // Each refusal counted and on the timeline: the thread, not the command.
    let log = r.app.plans.lock().unwrap().log_of(&plan);
    let refused: Vec<_> = log.entries.iter().filter(|e| e.kind == sbxw_core::team::EventKind::TestRefused).collect();
    assert_eq!(refused.len(), 4);
    assert!(refused.iter().all(|e| e.from == "api" && e.data["thread"] == "api"));
    assert!(refused.iter().all(|e| !serde_json::to_string(e).unwrap().contains("tools/check")), "never the command");
    let n = r.app.plan_view(&r.app.plan(&plan).unwrap())["numbers"].clone();
    assert_eq!(n["tests"]["refused"].as_u64(), Some(4));
    let tl = r.app.team_timeline(&plan, &Default::default(), None, 100).await.unwrap();
    assert_eq!(tl["numbers"]["tests"]["refused"].as_u64(), Some(4));
    assert!(tl["events"].as_array().unwrap().iter().any(|e| e["kind"] == "test_refused" && e["group"] == "status"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_later_job_never_overwrites_an_earlier_jobs_logs_and_old_ones_are_pruned() {
    if !has_tools() {
        return;
    }
    let r = rig();
    let count = r.scratch("count");
    let plan = r.plan("api", Some(&format!("echo ran >> {count}; echo run-$(wc -l < {count})")), TestsChoice::Approve);
    let api = name("api");
    // The one-file log of before goes with the first run.
    let logs = r.state.path().join("sandbox-home/.sbxw/tests/logs");
    std::fs::create_dir_all(&logs).unwrap();
    std::fs::write(logs.join(format!("{plan}.log")), "old").unwrap();
    let mut jobs = vec![];
    for _ in 0..KEEP + 2 {
        let (j, _) = run(&r.app, &r.sb, TestAsk { rerun: true, ..r.ask(&api, "HEAD") }).await;
        jobs.push(j);
    }
    assert!(!logs.join(format!("{plan}.log")).exists(), "the one-file log is gone");
    let log = |j: &sbxw_core::testq::Job| j.result.as_ref().unwrap().log.clone();
    // The first two jobs' logs are pruned; each kept one still holds its own run.
    for (i, j) in jobs.iter().enumerate() {
        let path = log(j);
        assert!(path.ends_with(&format!("/logs/{plan}/{}-1.log", j.id)), "{path}");
        if i < 2 {
            assert!(!Path::new(&path).exists(), "job {i} pruned");
        } else {
            assert_eq!(std::fs::read_to_string(&path).unwrap().trim(), format!("run-{}", i + 1), "job {i} kept its own output");
        }
    }
    let kept = std::fs::read_dir(logs.join(&plan)).unwrap().count();
    assert_eq!(kept, KEEP);
}

const KEEP: usize = sbxw_core::testq::KEEP_LOG_JOBS;

/// The lead reads a job a thread asked for: `check_tests` by id, and
/// `list_tests` for every open plan it leads, with each attempt's log.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_session_that_did_not_ask_reads_the_result() {
    use tower::ServiceExt;
    if !has_tools() {
        return;
    }
    let r = rig();
    let flag = r.scratch("flag");
    let a = r.plan("api", Some(&format!("if [ -e {flag} ]; then echo second; else touch {flag}; echo first; exit 1; fi")), TestsChoice::Approve);
    let b = r.plan("web", Some("echo web"), TestsChoice::Approve);
    let (j, _) = run(&r.app, &r.sb, r.ask(&name("api"), "HEAD")).await;
    assert_eq!(j.by, "api");
    // The lead's check_tests, as the relay calls it: no thread, the job id.
    let token = r.app.store.token(r.sb.clone()).unwrap();
    let post = |uri: &str, body: serde_json::Value| {
        axum::http::Request::builder()
            .method("POST")
            .uri(uri)
            .header("content-type", "application/json")
            .header("authorization", format!("Bearer {token}"))
            .body(axum::body::Body::from(body.to_string()))
            .unwrap()
    };
    let res = crate::http::agent_router(r.app.clone()).oneshot(post("/api/tests/check", serde_json::json!({ "id": j.id, "timeout": 1 }))).await.unwrap();
    let v: serde_json::Value = serde_json::from_slice(&axum::body::to_bytes(res.into_body(), 1 << 20).await.unwrap()).unwrap();
    assert_eq!((v["job"]["state"].as_str(), v["job"]["by"].as_str()), (Some("done"), Some("api")), "{v}");
    assert_eq!(v["job"]["result"]["attempts"].as_u64(), Some(2));
    assert!(v["job"]["result"]["first"]["tail"].as_str().unwrap().contains("first"), "{v}");
    assert!(v["job"]["result"]["first"]["log"].as_str().unwrap().ends_with("-1.log"), "{v}");
    // The lead's list_tests: both open plans it leads, whoever asked.
    let res = crate::http::agent_router(r.app.clone()).oneshot(post("/api/tests/list", serde_json::json!({ "lead": "lead", "plan": b }))).await.unwrap();
    let v: serde_json::Value = serde_json::from_slice(&axum::body::to_bytes(res.into_body(), 1 << 20).await.unwrap()).unwrap();
    let plans = v["plans"].as_array().unwrap();
    let ids: Vec<&str> = plans.iter().map(|p| p["plan"]["plan"].as_str().unwrap()).collect();
    assert_eq!(ids, [b.as_str(), a.as_str()], "{v}");
    let latest = plans[1]["latest"].as_array().unwrap();
    assert_eq!((latest[0]["id"].as_str(), latest[0]["by"].as_str()), (Some(j.id.as_str()), Some("api")));
    assert!(latest[0]["result"]["first"]["log"].as_str().is_some() && latest[0]["result"]["log"].as_str().is_some());
    assert!(plans[0]["latest"].as_array().unwrap().is_empty(), "a plan's list has its own jobs only");
    // A session that leads none of them (the person's own) sees every open plan;
    // a thread sees its own.
    assert_eq!(r.app.tests_plans_of(&r.sb, None, None, Some("someone-else")).len(), 2);
    let own = r.app.tests_plans_of(&r.sb, Some(&name("web")), None, None);
    assert_eq!(own.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(), [b.as_str()]);
}
