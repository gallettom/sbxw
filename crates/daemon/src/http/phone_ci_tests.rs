//! The phone and a split plan's CI merge (`plans::ci`): a plan Merged to CI
//! is listed and merged from the phone like a proposed merge, through the CI
//! path (a fast-forward to the tested tip); a thread tip that moved since the
//! CI branch was built is refused as changed; a CI run still building,
//! testing or failed is never offered.

use super::*;
use sbxw_core::plans::{Built, CiFailKind, CiFailure, CiStep, CiTests, MergeState};
use sbxw_core::testq::{Attempt, Outcome};

struct Ci {
    ws: PathBuf,
    id: String,
    tip: String,
    base: String,
}

/// A repo with `sbxw/alpha` one commit ahead, plan `alpha` done, its merge
/// proposed as a CI merge and its CI branch built (by hand, as the script
/// would), left in `state`.
fn ci_plan(w: &World, state: MergeState) -> Ci {
    let ws = w._dir.path().join("ws");
    std::fs::create_dir_all(&ws).unwrap();
    git(&ws, &["init", "-q", "-b", "main"]);
    std::fs::write(ws.join("a.txt"), "one\n").unwrap();
    git(&ws, &["add", "."]);
    git(&ws, &["commit", "-qm", "base"]);
    git(&ws, &["checkout", "-q", "-b", "sbxw/alpha"]);
    std::fs::write(ws.join("b.rs"), "thread\n").unwrap();
    git(&ws, &["add", "."]);
    git(&ws, &["commit", "-qm", "thread work"]);
    let alpha = git(&ws, &["rev-parse", "HEAD"]);
    git(&ws, &["checkout", "-q", "-b", "sbxw/ci-x", "main"]);
    git(&ws, &["merge", "-q", "--no-ff", "-m", "Merge branch 'sbxw/alpha'", &alpha]);
    let tip = git(&ws, &["rev-parse", "HEAD"]);
    git(&ws, &["checkout", "-q", "main"]);
    let base = git(&ws, &["rev-parse", "main"]);
    let sb = sbxw_core::SandboxRef::parse("myapp").unwrap();
    w.app.store.update(&sb, |r| r.workspace = Some(ws.clone())).unwrap();
    let id = pending_plan(w, "myapp", "ci me");
    let mut book = w.app.plans.lock().unwrap();
    let threads = book.get(&id).unwrap().threads.clone();
    book.approve(&id, threads, None, None, 2).unwrap();
    book.set_thread("myapp", "alpha", ThreadState::Done, "done", false, 3).unwrap();
    book.propose_merge_with(&id, "myapp", vec!["alpha".into()], "Merge alpha", vec![], true, 4).unwrap();
    if state != MergeState::CiBuilding {
        let built = Built { base_branch: "main".into(), base: base.clone(), tip: tip.clone(), merged: vec![CiStep { thread: "alpha".into(), tip: alpha, commit: tip.clone() }], skipped: vec![] };
        book.ci_built(&id, 1, built, 5).unwrap();
    }
    let attempt = |o| Attempt { outcome: o, duration_ms: 1, log: String::new(), tail: String::new() };
    match state {
        MergeState::MergedCi => {
            book.ci_tested(&id, 1, CiTests { job: "j".into(), outcome: Outcome::Passed, reused: false, commit: tip.clone(), attempts: vec![attempt(Outcome::Passed)] }, 6).unwrap();
        }
        MergeState::CiFailed => {
            book.ci_failed(&id, 1, CiFailure { kind: CiFailKind::Tests, thread: None, files: vec![], text: "failed".into() }, 6).unwrap();
        }
        _ => {}
    }
    assert_eq!(book.get(&id).unwrap().merge.as_ref().unwrap().state, state);
    drop(book);
    Ci { ws, id, tip, base }
}

fn listed(r: &Reply, id: &str) -> bool {
    r.json()["merges"].as_array().is_some_and(|l| l.iter().any(|m| m.to_string().contains(id)))
}

#[tokio::test]
async fn checked_ci_a_plan_merged_to_ci_is_listed_and_the_phone_fast_forwards_main_to_the_tested_tip() {
    let w = world();
    let mut p = enrolled(&w, "phone").await;
    let c = ci_plan(&w, MergeState::MergedCi);
    let l = call(&w.router, "GET", "/r/merges", Some(&p.token), json!({})).await;
    assert!(listed(&l, &c.id), "{}", l.text());
    let ch = challenge(&w, &p, "merge", &c.id, &format!("/r/merges/{}", c.id)).await;
    let a = assertion(&mut p, &ch);
    let r = call(&w.router, "POST", &format!("/r/merges/{}/merge", c.id), Some(&p.token), json!({ "assertion": a })).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    assert_eq!(git(&c.ws, &["rev-parse", "main"]), c.tip, "main is exactly the tested commit");
    assert_eq!(w.app.plan(&c.id).unwrap().merge.unwrap().state, MergeState::Merged);
    assert!(!w.fake.log().iter().any(|x| x.contains("sbxw-thread-merge")), "no per-branch merge");
}

#[tokio::test]
async fn checked_ci_a_thread_tip_moved_since_the_ci_branch_was_built_is_refused_as_changed() {
    let w = world();
    let mut p = enrolled(&w, "phone").await;
    let c = ci_plan(&w, MergeState::MergedCi);
    // The thread commits again after its CI branch was tested: the phone
    // shows the new tip, which is not what was tested.
    git(&c.ws, &["checkout", "-q", "sbxw/alpha"]);
    std::fs::write(c.ws.join("late.rs"), "untested\n").unwrap();
    git(&c.ws, &["add", "."]);
    git(&c.ws, &["commit", "-qm", "late"]);
    git(&c.ws, &["checkout", "-q", "main"]);
    let ch = challenge(&w, &p, "merge", &c.id, &format!("/r/merges/{}", c.id)).await;
    let a = assertion(&mut p, &ch);
    let r = call(&w.router, "POST", &format!("/r/merges/{}/merge", c.id), Some(&p.token), json!({ "assertion": a })).await;
    assert!(!r.status.is_success() && r.text().contains("changed"), "{} {}", r.status, r.text());
    assert_eq!(git(&c.ws, &["rev-parse", "main"]), c.base, "main untouched");
    assert_eq!(w.app.plan(&c.id).unwrap().merge.unwrap().state, MergeState::MergedCi, "still Merged to CI");
}

#[tokio::test]
async fn checked_ci_a_ci_run_building_testing_or_failed_is_not_offered_to_the_phone() {
    for state in [MergeState::CiBuilding, MergeState::CiTesting, MergeState::CiFailed] {
        let w = world();
        let p = enrolled(&w, "phone").await;
        let c = ci_plan(&w, state);
        let l = call(&w.router, "GET", "/r/merges", Some(&p.token), json!({})).await;
        assert!(!listed(&l, &c.id), "{state:?}: {}", l.text());
        let d = call(&w.router, "GET", &format!("/r/merges/{}", c.id), Some(&p.token), json!({})).await;
        assert_ne!(d.status, StatusCode::OK, "{state:?}: {}", d.text());
    }
}
