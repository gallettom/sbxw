//! A split plan's CI branch against a real git repository: the fake
//! `sbx exec` runs the CI, merge and test scripts with `bash` here, as the
//! sandbox would, with `HOME` in a temp dir. The test command passes when
//! the tree's `verdict` file says `pass`.

use crate::app::App;
use crate::testing::{app_in_with, ls, sbx_answer};
use sbxw_core::plans::{CiFailKind, MergeState, Plan, PlanThread, TestsChoice, ThreadState, Variant, MERGED_TO_CI, MERGED_TO_MAIN};
use sbxw_core::testq::{Outcome, PlanTests};
use sbxw_core::SandboxRef;
use sbxw_sbx::runner::{Fake, Output};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

const COMMAND: &str = "test \"$(cat verdict)\" = pass";

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

fn commit(ws: &Path, file: &str, text: &str) -> String {
    std::fs::write(ws.join(file), text).unwrap();
    git(ws, &["add", "-A"]);
    git(ws, &["commit", "-q", "-m", file]);
    git(ws, &["rev-parse", "HEAD"])
}

/// One more commit on `sbxw/<thread>`, main checked out again after.
fn branch_commit(ws: &Path, thread: &str, file: &str, text: &str) {
    git(ws, &["checkout", "-q", &format!("sbxw/{thread}")]);
    commit(ws, file, text);
    git(ws, &["checkout", "-q", "main"]);
}

/// Branch `sbxw/<thread>` from main with `file` set to `text`.
fn branch(ws: &Path, thread: &str, file: &str, text: &str) {
    git(ws, &["checkout", "-q", "-b", &format!("sbxw/{thread}"), "main"]);
    commit(ws, file, text);
    git(ws, &["checkout", "-q", "main"]);
}

/// A commit a thread makes on its branch at the moment the fast-forward
/// runs: `(repo, thread)`, taken once.
type Late = Arc<std::sync::Mutex<Option<(PathBuf, String)>>>;

/// `sbx exec <name> -- bash -lc SCRIPT $0 args…` of sbxw's own scripts, run
/// here (the thread close too); the rest answered as a running sandbox
/// would.
fn local_exec(home: PathBuf, late: Late) -> impl Fn(&[String]) -> Output + Send + Sync + 'static {
    move |a: &[String]| match a.first().map(String::as_str) {
        Some("ls") => ls(&[("myapp", "running")]),
        Some("exec") if a.get(3).is_some_and(|x| x == "bash") && a.get(6).is_some_and(|x| x.starts_with("sbxw-tests-") || x.starts_with("sbxw-ci-") || x == "sbxw-thread-merge" || x == "sbxw-thread-end") => {
            if a[6] == "sbxw-ci-ff" {
                if let Some((ws, t)) = late.lock().unwrap().take() {
                    branch_commit(&ws, &t, "late.txt", "made after the check");
                }
            }
            let out = std::process::Command::new("bash")
                .env("HOME", &home)
                .env("GIT_AUTHOR_NAME", "Ada")
                .env("GIT_AUTHOR_EMAIL", "someone@example.invalid")
                .env("GIT_COMMITTER_NAME", "Ada")
                .env("GIT_COMMITTER_EMAIL", "someone@example.invalid")
                .args(["-c", &a[5], &a[6]])
                .args(&a[7..])
                .output()
                .unwrap();
            Output { code: out.status.code(), stdout: out.stdout, stderr: out.stderr }
        }
        _ => sbx_answer(a),
    }
}

struct Rig {
    _state: tempfile::TempDir,
    ws: tempfile::TempDir,
    app: Arc<App>,
    fake: Arc<Fake>,
    sb: SandboxRef,
    late: Late,
}

fn t(name: &str) -> PlanThread {
    PlanThread { name: name.into(), goal: "g".into(), scope: String::new(), repo: None, model: None, state: ThreadState::Starting, summary: String::new(), updated_ms: 0, set_by: None, ticket: None }
}

/// main with `verdict` = pass and two files; sandbox `myapp` on it.
fn rig() -> Rig {
    let state = tempfile::tempdir().unwrap();
    let ws = tempfile::tempdir().unwrap();
    git(ws.path(), &["init", "-q"]);
    commit(ws.path(), "verdict", "pass");
    commit(ws.path(), "a.txt", "a");
    commit(ws.path(), "b.txt", "b");
    let home = state.path().join("sandbox-home");
    std::fs::create_dir_all(&home).unwrap();
    let late: Late = Default::default();
    let (app, fake) = app_in_with(state.path(), local_exec(home, late.clone()));
    let sb = SandboxRef::parse("myapp").unwrap();
    app.store.update(&sb, |r| r.workspace = Some(ws.path().to_path_buf())).unwrap();
    Rig { _state: state, ws, app, fake, sb, late }
}

impl Rig {
    /// An approved split plan `id` of `names`, all done, with the test
    /// command (or none).
    fn plan(&self, id: &str, names: &[&str], command: Option<&str>) {
        let mut b = self.app.plans.lock().unwrap();
        let threads: Vec<PlanThread> = names.iter().map(|n| t(n)).collect();
        b.propose(id.into(), &self.sb.to_string(), "s", threads.clone(), Variant::Split, "independent", Some("lead"), 1).unwrap();
        b.note_proposed_tests(id, PlanTests::proposed(command, Some(5)).unwrap());
        b.approve_with(id, threads, None, None, TestsChoice::Approve, 2).unwrap();
        for n in names {
            b.set_thread(&self.sb.to_string(), n, ThreadState::Done, "done", false, 3).unwrap();
        }
    }

    async fn propose(&self, id: &str, order: &[&str]) -> Plan {
        self.app.plan_propose_merge_checked(id, &self.sb, order.iter().map(|s| s.to_string()).collect(), "note", vec![]).await.unwrap()
    }

    fn state(&self, id: &str) -> MergeState {
        self.app.plan(id).unwrap().merge.unwrap().state
    }

    /// Wait until plan `id`'s merge is in `want`.
    async fn until(&self, id: &str, want: MergeState) -> Plan {
        for _ in 0..800 {
            if self.state(id) == want {
                return self.app.plan(id).unwrap();
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        panic!("plan {id} never reached {want:?}: {:?}", self.app.plan(id).unwrap().merge);
    }

    fn runs(&self) -> usize {
        self.fake.log().iter().filter(|c| c.contains("sbxw-tests-run")).count()
    }

    fn main(&self) -> String {
        git(self.ws.path(), &["rev-parse", "main"])
    }

    fn lead_line(&self, what: &str) -> String {
        self.app.inbox.lock().unwrap().pending(&self.sb.to_string(), &sbxw_core::inbox::To::Main).iter().map(|m| m.text.clone()).find(|t| t.contains(what)).unwrap_or_default()
    }

    fn has_ci_branch(&self, id: &str) -> bool {
        !git(self.ws.path(), &["branch", "--list", &format!("sbxw/ci-{id}")]).is_empty() || self.ws.path().join(format!(".sbxw-worktrees/.ci-{id}")).exists()
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_threads_build_the_ci_branch_in_order_test_it_once_and_the_click_fast_forwards_main_to_it() {
    if !has_tools() {
        return;
    }
    let r = rig();
    branch(r.ws.path(), "a", "a.txt", "a's work");
    branch(r.ws.path(), "b", "b.txt", "b's work");
    r.plan("p1", &["a", "b"], Some(COMMAND));
    let before = r.main();
    let p = r.propose("p1", &["b", "a"]).await;
    assert!(p.ci().is_some(), "a CI merge");
    let p = r.until("p1", MergeState::MergedCi).await;
    let c = p.ci().unwrap();
    assert_eq!(c.merged.iter().map(|s| s.thread.as_str()).collect::<Vec<_>>(), ["b", "a"], "the lead's order");
    assert_eq!(c.base.as_deref(), Some(before.as_str()));
    let tip = c.tip.clone().unwrap();
    assert_eq!(git(r.ws.path(), &["rev-parse", "sbxw/ci-p1"]), tip);
    assert_eq!(c.tests.as_ref().map(|t| t.outcome), Some(Outcome::Passed));
    assert_eq!(r.runs(), 1, "the battery ran once");
    assert_eq!(r.main(), before, "nothing reached main before the click");
    let v = r.app.plan_view(&p);
    assert_eq!((v["merge"]["state"].as_str(), v["merge_status"]["word"].as_str(), v["merge_status"]["can_merge"].as_bool()), (Some("merged_ci"), Some(MERGED_TO_CI), Some(true)));
    assert!(r.lead_line("[sbxw] Merged to CI: plan p1").contains(&tip[..7]), "the lead hears of the pass");
    // An untracked file and an edit the merge does not touch stay.
    std::fs::write(r.ws.path().join("notes.md"), "mine").unwrap();
    std::fs::write(r.ws.path().join("verdict"), "pass\n# local").unwrap();
    let m = r.app.plan_merge("p1", false, None).await.unwrap();
    assert_eq!(m.state, MergeState::Merged);
    assert_eq!(r.main(), tip, "main is exactly the tested commit");
    assert!(m.results.iter().all(|x| x.ok && x.note.as_deref().unwrap_or("").contains("fast-forwarded main to the tested CI tip")), "{m:?}");
    assert_eq!(std::fs::read_to_string(r.ws.path().join("notes.md")).unwrap(), "mine");
    assert_eq!(sbxw_core::plans::merge_status(&m).word, MERGED_TO_MAIN);
    assert!(!r.has_ci_branch("p1"), "the CI branch and worktree are removed once merged");
    assert!(r.lead_line("[sbxw] Merged to main: b, a").contains("fast-forwarded"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_conflict_names_the_thread_and_the_files_and_runs_no_battery() {
    if !has_tools() {
        return;
    }
    let r = rig();
    branch(r.ws.path(), "a", "a.txt", "a's");
    branch(r.ws.path(), "b", "a.txt", "b's own a");
    r.plan("p1", &["a", "b"], Some(COMMAND));
    let before = r.main();
    r.propose("p1", &["a", "b"]).await;
    let p = r.until("p1", MergeState::CiFailed).await;
    let f = p.ci().unwrap().failure.clone().unwrap();
    assert_eq!((f.kind, f.thread.as_deref(), f.files.as_slice()), (CiFailKind::Conflict, Some("b"), ["a.txt".to_string()].as_slice()));
    assert_eq!(r.runs(), 0, "no battery");
    assert_eq!(r.main(), before);
    let line = r.lead_line("[sbxw] CI failed for plan p1");
    assert!(line.contains("sbxw/b conflicts in a.txt") && line.contains("after merging a"), "{line}");
    assert!(r.app.plan_merge("p1", false, None).await.unwrap_err().contains("cannot be merged yet"));
    assert!(r.has_ci_branch("p1"), "kept after a failure: the thing to rebuild");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_failed_battery_leaves_main_untouched_reports_both_attempts_and_a_fix_retests() {
    if !has_tools() {
        return;
    }
    let r = rig();
    branch(r.ws.path(), "a", "a.txt", "a's");
    branch(r.ws.path(), "b", "verdict", "fail");
    r.plan("p1", &["a", "b"], Some(COMMAND));
    let before = r.main();
    r.propose("p1", &["a", "b"]).await;
    let p = r.until("p1", MergeState::CiFailed).await;
    let t = p.ci().unwrap().tests.clone().unwrap();
    assert_eq!((t.outcome, t.attempts.len()), (Outcome::Failed, 2), "{t:?}");
    assert!(t.attempts.iter().all(|a| a.outcome == Outcome::Failed && !a.log.is_empty()));
    assert_eq!(r.main(), before);
    let line = r.lead_line("[sbxw] CI failed for plan p1");
    assert!(line.contains("=== Attempt 1 of 2 ===") && line.contains("=== Attempt 2 of 2 ===") && line.contains(&t.attempts[0].log), "{line}");
    // The lead sends b back: proposing again waits for b's done.
    r.app.plans.lock().unwrap().set_thread("myapp", "b", ThreadState::Working, "fixing", false, 10).unwrap();
    let e = r.app.plan_propose_merge_checked("p1", &r.sb, vec!["a".into(), "b".into()], "", vec![]).await.unwrap_err();
    assert!(e.contains("\"b\" is not done"), "{e}");
    git(r.ws.path(), &["checkout", "-q", "sbxw/b"]);
    commit(r.ws.path(), "verdict", "pass");
    git(r.ws.path(), &["checkout", "-q", "main"]);
    r.app.plans.lock().unwrap().set_thread("myapp", "b", ThreadState::Done, "fixed", false, 11).unwrap();
    r.propose("p1", &["a", "b"]).await;
    let p = r.until("p1", MergeState::MergedCi).await;
    assert_eq!(p.ci().unwrap().run, 2, "rebuilt from scratch");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn main_moving_rebuilds_and_retests_before_the_click_and_a_refused_click_stays_merged_to_ci() {
    if !has_tools() {
        return;
    }
    let r = rig();
    branch(r.ws.path(), "a", "a.txt", "a's");
    r.plan("p1", &["a"], Some(COMMAND));
    r.propose("p1", &["a"]).await;
    let p = r.until("p1", MergeState::MergedCi).await;
    let first_tip = p.ci().unwrap().tip.clone().unwrap();
    // The click refused for an edit on a path the merge changes.
    std::fs::write(r.ws.path().join("a.txt"), "my edit").unwrap();
    let e = r.app.plan_merge("p1", false, None).await.unwrap_err();
    assert!(e.contains("a.txt"), "{e}");
    assert_eq!(r.state("p1"), MergeState::MergedCi, "a refused click is not a CI failure");
    git(r.ws.path(), &["checkout", "-q", "--", "a.txt"]);
    // Main moves (a merge outside this plan): the click is refused, the CI
    // branch is rebuilt on the new main and tested again.
    let moved = commit(r.ws.path(), "c.txt", "elsewhere");
    let e = r.app.plan_merge("p1", false, None).await.unwrap_err();
    assert!(e.contains("main moved since the CI branch was built"), "{e}");
    let p = r.until("p1", MergeState::MergedCi).await;
    let c = p.ci().unwrap();
    assert_eq!((c.base.as_deref(), c.builds), (Some(moved.as_str()), 2));
    assert_ne!(c.tip.as_deref(), Some(first_tip.as_str()));
    assert_eq!(r.runs(), 2, "tested again on the new tree");
    assert!(r.lead_line("[sbxw] Plan p1").contains("main moved from"), "the lead hears of the rebuild");
    r.app.plan_merge("p1", false, None).await.unwrap();
    assert_eq!(r.main(), c.tip.clone().unwrap());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn main_moving_under_two_plans_rebuilds_one_at_a_time_and_the_watch_sees_it() {
    if !has_tools() {
        return;
    }
    let r = rig();
    branch(r.ws.path(), "a", "a.txt", "a's");
    branch(r.ws.path(), "b", "b.txt", "b's");
    r.plan("p1", &["a"], Some(COMMAND));
    r.plan("p2", &["b"], Some(COMMAND));
    r.propose("p1", &["a"]).await;
    r.until("p1", MergeState::MergedCi).await;
    r.propose("p2", &["b"]).await;
    r.until("p2", MergeState::MergedCi).await;
    commit(r.ws.path(), "c.txt", "elsewhere");
    r.app.ci_watch(None).await;
    // p1 passed first: it rebuilds, p2 waits its turn, then follows.
    assert_ne!(r.state("p1"), MergeState::CiStale);
    assert_eq!(r.state("p2"), MergeState::CiStale);
    let why = r.app.plan_view(&r.app.plan("p2").unwrap())["merge_status"]["why_not"].as_str().unwrap_or("").to_string();
    assert!(why.contains("waits its turn"), "{why}");
    r.until("p1", MergeState::MergedCi).await;
    r.until("p2", MergeState::MergedCi).await;
    assert_eq!(r.runs(), 4);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn no_test_command_merges_as_today_and_abandon_removes_the_ci_branch() {
    if !has_tools() {
        return;
    }
    let r = rig();
    branch(r.ws.path(), "a", "a.txt", "a's");
    r.plan("p0", &["a"], None);
    let p = r.propose("p0", &["a"]).await;
    assert_eq!((p.merge.as_ref().unwrap().state, p.ci().is_none()), (MergeState::Proposed, true));
    let m = r.app.plan_merge("p0", false, None).await.unwrap();
    assert_eq!(m.state, MergeState::Merged);
    assert_eq!(git(r.ws.path(), &["rev-list", "--count", "--merges", "main"]), "1", "a --no-ff merge, as today");
    assert_eq!(r.runs(), 0);

    branch(r.ws.path(), "b", "b.txt", "b's");
    r.plan("p1", &["b"], Some(COMMAND));
    r.propose("p1", &["b"]).await;
    r.until("p1", MergeState::MergedCi).await;
    assert!(r.has_ci_branch("p1"));
    r.app.plan_abandon_merge("p1").await.unwrap();
    assert_eq!(r.state("p1"), MergeState::Abandoned);
    assert!(!r.has_ci_branch("p1"), "abandoned: the CI branch and worktree are removed");
}

/// A Merge click can arrive while the CI branch builds, tests or failed
/// (a screen that offers Merge for any state): refused in words a person
/// reads, nothing run, main untouched.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_click_before_merged_to_ci_is_refused_with_the_reason() {
    if !has_tools() {
        return;
    }
    let r = rig();
    branch(r.ws.path(), "a", "a.txt", "a's");
    r.plan("p1", &["a"], Some(COMMAND));
    let before = r.main();
    // Proposed in the book only: nothing builds it, it stays ci_building.
    r.app.plans.lock().unwrap().propose_merge_with("p1", "myapp", vec!["a".into()], "", vec![], true, 4).unwrap();
    let e = r.app.plan_merge("p1", false, None).await.unwrap_err();
    assert!(e.contains("cannot be merged yet") && e.contains("is building sbxw/ci-p1"), "{e}");
    let built = sbxw_core::plans::Built { base_branch: "main".into(), base: before.clone(), tip: before.clone(), merged: vec![], skipped: vec![] };
    r.app.plans.lock().unwrap().ci_built("p1", 1, built, 5).unwrap();
    let e = r.app.plan_merge("p1", false, None).await.unwrap_err();
    assert!(e.contains("cannot be merged yet") && e.contains("the plan's tests run on sbxw/ci-p1"), "{e}");
    let f = sbxw_core::plans::CiFailure { kind: CiFailKind::Tests, thread: None, files: vec![], text: "failed".into() };
    r.app.plans.lock().unwrap().ci_failed("p1", 1, f, 6).unwrap();
    let e = r.app.plan_merge("p1", false, None).await.unwrap_err();
    assert!(e.contains("cannot be merged yet") && e.contains("proposes the merge again"), "{e}");
    assert_eq!(r.main(), before);
    assert!(!r.fake.log().iter().any(|c| c.contains("sbxw-ci-ff") || c.contains("sbxw-thread-merge")), "nothing ran");
}

/// Plan `id`'s CI branch built for real (as `ci_run` would) and recorded as
/// built, so it is left in ci_testing: what a daemon stopped mid-battery
/// leaves behind.
async fn built_and_testing(r: &Rig, id: &str, order: &[&str]) -> String {
    r.app.plans.lock().unwrap().propose_merge_with(id, "myapp", order.iter().map(|s| s.to_string()).collect(), "", vec![], true, 4).unwrap();
    let threads: Vec<sbxw_core::ThreadName> = order.iter().map(|t| sbxw_core::ThreadName::parse(t).unwrap()).collect();
    let b = r.app.sbx_of(&r.sb).await.unwrap().ci_build(&r.sb.name, &r.ws.path().to_string_lossy(), id, &threads).await.unwrap();
    let built = sbxw_core::plans::Built {
        base_branch: b.base_branch,
        base: b.base,
        tip: b.tip.clone(),
        merged: b.merged.into_iter().map(|(thread, tip, commit)| sbxw_core::plans::CiStep { thread, tip, commit }).collect(),
        skipped: vec![],
    };
    r.app.plans.lock().unwrap().ci_built(id, 1, built, 5).unwrap();
    b.tip
}

/// A job queued in the book and never started (no worker): as a queue
/// that had not reached it.
fn queued_job(r: &Rig, id: &str, tip: &str) -> String {
    let tests = PlanTests { command: COMMAND.into(), timeout_minutes: 5 };
    let ask = sbxw_core::testq::Ask { sandbox: "myapp", tests: &tests, key: "k-queued", commit: tip, plan: Some(id), by: "sbxw", rerun: false };
    let (job, took) = r.app.tests.lock().unwrap().request("t-queued".into(), &ask, 6);
    assert_eq!(took, sbxw_core::testq::Took::Queued);
    r.app.plans.lock().unwrap().ci_job(id, 1, &job.id, 7).unwrap();
    job.id
}

/// After a restart: a run left building is built again; one left testing
/// waits on its job again; one whose job is gone from the queue is built
/// and tested again. None is left stuck, none is merged.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_restart_picks_up_every_ci_run_and_merges_none() {
    if !has_tools() {
        return;
    }
    let r = rig();
    branch(r.ws.path(), "a", "a.txt", "a's");
    branch(r.ws.path(), "b", "b.txt", "b's");
    branch(r.ws.path(), "c", "c.txt", "c's");
    r.plan("p1", &["a"], Some(COMMAND));
    r.plan("p2", &["b"], Some(COMMAND));
    r.plan("p3", &["c"], Some(COMMAND));
    let before = r.main();
    // p1: left building (nothing recorded but the proposal).
    r.app.plans.lock().unwrap().propose_merge_with("p1", "myapp", vec!["a".into()], "", vec![], true, 4).unwrap();
    // p2: left testing, its job finished meanwhile (a run the queue resumed).
    let tip2 = built_and_testing(&r, "p2", &["b"]).await;
    let (job2, _) = r.app.tests_request(&r.sb, crate::svc::testq::TestAsk { plan: Some("p2"), git_ref: Some(&tip2), by: Some("sbxw"), ..Default::default() }).await.unwrap();
    r.app.tests_wait(&job2.id, &r.sb, Duration::from_secs(60)).await.unwrap();
    r.app.plans.lock().unwrap().ci_job("p2", 1, &job2.id, 7).unwrap();
    // p3: left testing on a job the queue no longer has.
    built_and_testing(&r, "p3", &["c"]).await;
    r.app.plans.lock().unwrap().ci_job("p3", 1, "t-lost", 7).unwrap();
    let runs_before = r.runs();

    r.app.tests_resume();
    let p1 = r.until("p1", MergeState::MergedCi).await;
    let p2 = r.until("p2", MergeState::MergedCi).await;
    let p3 = r.until("p3", MergeState::MergedCi).await;
    assert_eq!(p1.ci().unwrap().run, 2, "p1 built again");
    assert_eq!((p2.ci().unwrap().run, p2.ci().unwrap().tests.as_ref().map(|t| t.job.clone())), (1, Some(job2.id.clone())), "p2 kept its job");
    assert_eq!(p3.ci().unwrap().run, 2, "p3 built and tested again");
    assert_eq!(r.runs(), runs_before + 2, "p1 and p3 tested, p2 not again");
    assert_eq!(r.main(), before, "nothing merged without the click");
}

/// Closing a plan while its CI branch is built or tested (lead mode ended,
/// Close plan on the Kanban): the run ends as abandoned, its queued job is
/// taken back, its CI branch and worktree are gone.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn closing_the_plan_while_ci_runs_ends_it_and_removes_the_ci_branch() {
    if !has_tools() {
        return;
    }
    let r = rig();
    branch(r.ws.path(), "a", "a.txt", "a's");
    branch(r.ws.path(), "b", "b.txt", "b's");
    // Close plan on the Kanban while testing, its job still queued.
    r.plan("p1", &["a"], Some(COMMAND));
    let tip = built_and_testing(&r, "p1", &["a"]).await;
    let job = queued_job(&r, "p1", &tip);
    assert!(r.has_ci_branch("p1"));
    r.app.plan_dismiss("p1").unwrap();
    let p = r.until("p1", MergeState::Abandoned).await;
    assert!(p.merge.unwrap().abandoned_ms.is_some());
    for _ in 0..200 {
        if !r.has_ci_branch("p1") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert!(!r.has_ci_branch("p1"), "the CI branch and worktree are gone");
    assert!(r.app.tests.lock().unwrap().get(&job, "myapp").is_err(), "the queued job was taken back");
    // Lead mode ended while building: the run ends, its branch goes.
    r.plan("p2", &["b"], Some(COMMAND));
    r.app.plans.lock().unwrap().propose_merge_with("p2", "myapp", vec!["b".into()], "", vec![], true, 4).unwrap();
    let threads = [sbxw_core::ThreadName::parse("b").unwrap()];
    r.app.sbx_of(&r.sb).await.unwrap().ci_build(&r.sb.name, &r.ws.path().to_string_lossy(), "p2", &threads).await.unwrap();
    assert!(r.has_ci_branch("p2"));
    r.app.lead_end(&r.sb).await;
    assert_eq!(r.state("p2"), MergeState::Abandoned);
    assert!(!r.has_ci_branch("p2"));
    // A build still at work when the plan closed would land after it: its
    // step is refused and what it built is removed.
    r.app.ci_run("p2", 1).await;
    assert!(!r.has_ci_branch("p2"), "a late build leaves nothing behind");
    assert_eq!(r.runs(), 0, "no battery ran for a closed plan");
}

/// A thread commits after its CI branch passed: the watch says so before
/// the click ("CI rebuild waits", with the reason); a click from the
/// desktop or the web UI (no pins) is refused with it too; the CI branch is
/// rebuilt with the commit, tested again, and the click then lands it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_commit_the_ci_did_not_test_is_refused_at_the_click_and_rebuilt() {
    if !has_tools() {
        return;
    }
    let r = rig();
    branch(r.ws.path(), "a", "a.txt", "a's");
    r.plan("p1", &["a"], Some(COMMAND));
    r.propose("p1", &["a"]).await;
    r.until("p1", MergeState::MergedCi).await;
    let before = r.main();
    branch_commit(r.ws.path(), "a", "more.txt", "after the pass");
    // The click, from the desktop or the web UI: no pins.
    let e = r.app.plan_merge("p1", false, None).await.unwrap_err();
    assert!(e.starts_with("sbxw/a has 1 commit the CI branch did not test"), "{e}");
    assert_eq!(r.main(), before, "nothing reached main");
    let p = r.until("p1", MergeState::MergedCi).await;
    let c = p.ci().unwrap();
    assert_eq!((c.builds, c.rebuilt_because.as_deref()), (2, Some("sbxw/a has 1 commit the CI branch did not test")));
    assert!(r.lead_line("[sbxw] Plan p1").contains("did not test"), "the lead hears of the rebuild");
    r.app.plan_merge("p1", false, None).await.unwrap();
    assert!(r.ws.path().join("more.txt").exists(), "the commit landed once tested");
}

/// The watch sees a thread's new commit before the click: the plan waits
/// for its rebuild, the reason in `why_not`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_watch_sees_a_thread_commit_before_the_click() {
    if !has_tools() {
        return;
    }
    let r = rig();
    branch(r.ws.path(), "a", "a.txt", "a's");
    branch(r.ws.path(), "b", "b.txt", "b's");
    r.plan("p1", &["a"], Some(COMMAND));
    r.plan("p2", &["b"], Some(COMMAND));
    r.propose("p1", &["a"]).await;
    r.until("p1", MergeState::MergedCi).await;
    r.propose("p2", &["b"]).await;
    r.until("p2", MergeState::MergedCi).await;
    branch_commit(r.ws.path(), "a", "more.txt", "x");
    branch_commit(r.ws.path(), "b", "more-b.txt", "y");
    r.app.ci_watch(None).await;
    // Both flagged; p1 (passed first) rebuilds, p2 waits, with its reason.
    assert_ne!(r.state("p1"), MergeState::MergedCi);
    let v = r.app.plan_view(&r.app.plan("p2").unwrap());
    assert_eq!((v["merge"]["state"].as_str(), v["merge_status"]["word"].as_str()), (Some("ci_stale"), Some("CI rebuild waits")));
    assert!(v["merge_status"]["why_not"].as_str().unwrap().starts_with("sbxw/b has 1 commit the CI branch did not test"), "{v}");
    r.until("p1", MergeState::MergedCi).await;
    r.until("p2", MergeState::MergedCi).await;
}

/// A commit lands on a thread's branch between the click's check and the
/// fast-forward: main takes the tested tip, and the close keeps that
/// thread's branch (its result says so), while the other's is deleted.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_branch_with_an_untested_commit_survives_the_close() {
    if !has_tools() {
        return;
    }
    let r = rig();
    branch(r.ws.path(), "a", "a.txt", "a's");
    branch(r.ws.path(), "b", "b.txt", "b's");
    r.plan("p1", &["a", "b"], Some(COMMAND));
    r.propose("p1", &["a", "b"]).await;
    r.until("p1", MergeState::MergedCi).await;
    *r.late.lock().unwrap() = Some((r.ws.path().to_path_buf(), "a".into()));
    let m = r.app.plan_merge("p1", true, None).await.unwrap();
    assert_eq!(m.state, MergeState::Merged);
    let late = git(r.ws.path(), &["rev-parse", "sbxw/a"]);
    assert!(git(r.ws.path(), &["log", "-1", "--format=%s", &late]).contains("late.txt"), "a's branch kept, with the late commit");
    assert!(git(r.ws.path(), &["branch", "--list", "sbxw/b"]).is_empty(), "b's branch, all merged, is deleted");
    let p = r.app.plan("p1").unwrap();
    let note = p.merge.unwrap().results.iter().find(|x| x.thread == "a").and_then(|x| x.note.clone()).unwrap_or_default();
    assert!(note.contains("its branch sbxw/a was kept"), "{note}");
    assert!(r.lead_line("Merged to main").contains("Kept the branch sbxw/a"));
}

/// A thread with nothing to merge (its branch gone before the build) is
/// said before the click: in the status the screens draw and in the lead's
/// Merged to CI line.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_skipped_thread_is_shown_before_the_click() {
    if !has_tools() {
        return;
    }
    let r = rig();
    branch(r.ws.path(), "a", "a.txt", "a's");
    r.plan("p1", &["a", "gone"], Some(COMMAND));
    r.propose("p1", &["a", "gone"]).await;
    let p = r.until("p1", MergeState::MergedCi).await;
    let v = r.app.plan_view(&p);
    let detail = v["merge_status"]["detail"].as_str().unwrap();
    assert!(detail.contains("Nothing to merge for: gone (sbxw/gone is gone)"), "{detail}");
    assert!(r.lead_line("[sbxw] Merged to CI: plan p1").contains("Nothing to merge for: gone (sbxw/gone is gone)."));
}

/// sbxw cannot read the thread branches (the workspace gone from the host's
/// view): the click is refused, saying so, and the plan stays Merged to CI;
/// a close after a CI merge keeps every ordered thread's branch.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn when_the_branches_cannot_be_read_the_click_is_refused_and_every_branch_kept() {
    if !has_tools() {
        return;
    }
    let r = rig();
    branch(r.ws.path(), "a", "a.txt", "a's");
    branch(r.ws.path(), "b", "b.txt", "b's");
    r.plan("p1", &["a", "b"], Some(COMMAND));
    r.propose("p1", &["a", "b"]).await;
    r.until("p1", MergeState::MergedCi).await;
    let before = r.main();
    let gone = r._state.path().join("no-such-repo");
    r.app.store.update(&r.sb, |x| x.workspace = Some(gone.clone())).unwrap();
    let e = r.app.plan_merge("p1", false, None).await.unwrap_err();
    assert!(e.starts_with("sbxw could not read the thread branches"), "{e}");
    assert_eq!(r.state("p1"), MergeState::MergedCi, "not ci_stale: nothing is known to be untested");
    assert_eq!(r.main(), before);
    // The close side, on the same unreadable workspace: every branch kept.
    let (keep, why) = r.app.ci_unmerged_branches(&r.app.plan("p1").unwrap()).await;
    assert_eq!(keep, ["a".to_string(), "b".to_string()].into_iter().collect());
    assert!(why.unwrap().contains("no-such-repo"));
}
