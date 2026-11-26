use super::*;
use crate::plans::{PlanThread, TestsChoice, ThreadState, Variant};
use crate::testq::PlanTests;

fn t(name: &str) -> PlanThread {
    PlanThread {
        name: name.into(),
        goal: format!("do {name}"),
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

/// An approved split plan `id` in sandbox `sb` with threads a and b done
/// and a test command.
fn done_plan(b: &mut PlanBook, id: &str, sb: &str) {
    b.propose(id.into(), sb, "s", vec![t("a"), t("b")], Variant::Split, "why", Some("lead"), 1).unwrap();
    b.note_proposed_tests(id, PlanTests::proposed(Some("make test"), Some(5)).unwrap());
    b.approve_with(id, vec![t("a"), t("b")], None, None, TestsChoice::Approve, 2).unwrap();
    for n in ["a", "b"] {
        b.set_thread(sb, n, ThreadState::Done, "ok", false, 3).unwrap();
    }
}

fn built(base: &str, tip: &str) -> Built {
    Built {
        base_branch: "main".into(),
        base: base.into(),
        tip: tip.into(),
        merged: vec![CiStep { thread: "a".into(), tip: "a1".into(), commit: "m1".into() }, CiStep { thread: "b".into(), tip: "b1".into(), commit: tip.into() }],
        skipped: vec![],
    }
}

fn tests(outcome: Outcome) -> CiTests {
    let a = |o| Attempt { outcome: o, duration_ms: 1000, log: format!("/l/{o:?}.log"), tail: format!("tail {o:?}") };
    let attempts = if outcome == Outcome::Passed { vec![a(outcome)] } else { vec![a(Outcome::Failed), a(outcome)] };
    CiTests { job: "j1".into(), outcome, reused: false, commit: "tip1".into(), attempts }
}

fn state(b: &PlanBook, id: &str) -> MergeState {
    b.get(id).unwrap().merge.as_ref().unwrap().state
}

#[test]
fn a_ci_merge_is_built_then_tested_then_merged_to_ci_and_the_click_takes_it() {
    let mut b = PlanBook::new();
    done_plan(&mut b, "p1", "sb");
    let p = b.propose_merge_with("p1", "sb", vec!["a".into(), "b".into()], "n", vec![], true, 4).unwrap();
    let m = p.merge.as_ref().unwrap();
    assert_eq!(m.state, MergeState::CiBuilding);
    let c = m.ci.as_ref().unwrap();
    assert_eq!((c.branch.as_str(), c.worktree.as_str(), c.run, c.builds), ("sbxw/ci-p1", ".sbxw-worktrees/.ci-p1", 1, 1));
    let st = merge_status(m);
    assert!(!st.can_merge && st.ci, "{st:?}");
    assert!(b.start_merge("p1", None, None, 5).unwrap_err().contains("cannot be merged yet"));
    // A re-proposal waits for the build.
    assert!(b.propose_merge_with("p1", "sb", vec!["a".into()], "n", vec![], true, 5).unwrap_err().contains("being built"));
    // An old run's answer is dropped.
    assert!(b.ci_built("p1", 9, built("base1", "tip1"), 5).is_err());
    b.ci_built("p1", 1, built("base1", "tip1"), 5).unwrap();
    assert_eq!(state(&b, "p1"), MergeState::CiTesting);
    b.ci_job("p1", 1, "j1", 6).unwrap();
    let p = b.ci_tested("p1", 1, tests(Outcome::Passed), 7).unwrap();
    let m = p.merge.as_ref().unwrap();
    assert_eq!(m.state, MergeState::MergedCi);
    assert_eq!(serde_json::to_value(m.state).unwrap(), "merged_ci");
    let st = merge_status(m);
    assert_eq!((st.word, st.can_merge), (MERGED_TO_CI, true));
    // The click; refused once (an edit in the checkout): back to Merged to CI.
    b.start_merge("p1", Some("main".into()), None, 8).unwrap();
    let p = b.ci_click_refused("p1", "uncommitted edits to f", 9).unwrap();
    assert_eq!(p.merge.as_ref().unwrap().state, MergeState::MergedCi);
    assert!(merge_status(p.merge.as_ref().unwrap()).detail.contains("uncommitted edits to f"));
    b.start_merge("p1", Some("main".into()), None, 10).unwrap();
    for n in ["a", "b"] {
        b.merge_result("p1", crate::plans::MergeResult { thread: n.into(), ok: true, commit: Some("c".into()), error: None, note: None }, 11).unwrap();
    }
    let p = b.finish_merge("p1", 12).unwrap();
    let m = p.merge.as_ref().unwrap();
    assert_eq!((m.state, merge_status(m).word), (MergeState::Merged, MERGED_TO_MAIN));
    assert!(crate::messages::merge_summary(m).starts_with("[sbxw] Merged to main: a, b. main fast-forwarded to the tested tip of sbxw/ci-p1, tip1"), "{}", crate::messages::merge_summary(m));
}

#[test]
fn a_failed_ci_names_the_conflict_or_each_attempt_and_a_re_proposal_needs_the_threads_done_again() {
    let mut b = PlanBook::new();
    done_plan(&mut b, "p1", "sb");
    b.propose_merge_with("p1", "sb", vec!["a".into(), "b".into()], "n", vec![], true, 4).unwrap();
    let f = CiFailure { kind: CiFailKind::Conflict, thread: Some("b".into()), files: vec!["x.rs".into(), "y.rs".into()], text: "conflict in x.rs y.rs".into() };
    let p = b.ci_failed("p1", 1, f, 5).unwrap();
    assert_eq!(p.merge.as_ref().unwrap().state, MergeState::CiFailed);
    let line = crate::messages::ci_failed_line(&p).unwrap();
    assert!(line.contains("sbxw/b conflicts in x.rs, y.rs") && line.contains("no tests ran"), "{line}");
    assert!(b.start_merge("p1", None, None, 6).is_err(), "nothing to click after a failure");
    // The lead sends b back: b reports working, the proposal is refused.
    b.set_thread("sb", "b", ThreadState::Working, "fixing", false, 7).unwrap();
    let e = b.propose_merge_with("p1", "sb", vec!["a".into(), "b".into()], "n", vec![], true, 8).unwrap_err();
    assert!(e.contains("\"b\" is not done"), "{e}");
    b.set_thread("sb", "b", ThreadState::Done, "fixed", false, 9).unwrap();
    let p = b.propose_merge_with("p1", "sb", vec!["a".into(), "b".into()], "n", vec![], true, 10).unwrap();
    let c = p.ci().unwrap();
    assert_eq!((p.merge.as_ref().unwrap().state, c.run, c.builds), (MergeState::CiBuilding, 2, 1), "a fresh run");
    // The tests fail on both attempts: the line has both, tails and logs.
    b.ci_built("p1", 2, built("base1", "tip1"), 11).unwrap();
    let p = b.ci_tested("p1", 2, tests(Outcome::Failed), 12).unwrap();
    assert_eq!(p.merge.as_ref().unwrap().state, MergeState::CiFailed);
    let line = crate::messages::ci_failed_line(&p).unwrap();
    for want in ["the tests failed on sbxw/ci-p1", "=== Attempt 1 of 2 ===", "=== Attempt 2 of 2 ===", "/l/Failed.log", "> tail Failed"] {
        assert!(line.contains(want), "{want}: {line}");
    }
}

#[test]
fn main_moving_rebuilds_one_plan_at_a_time_oldest_first_and_coalesces() {
    let mut b = PlanBook::new();
    for (id, at) in [("p1", 7), ("p2", 5)] {
        done_plan(&mut b, id, "sb");
        b.propose_merge_with(id, "sb", vec!["a".into(), "b".into()], "n", vec![], true, 4).unwrap();
        b.ci_built(id, 1, built("base1", "tip1"), 5).unwrap();
        b.ci_tested(id, 1, tests(Outcome::Passed), at).unwrap();
    }
    assert_eq!(b.ci_next_rebuild("sb"), None, "nothing stale");
    for id in ["p1", "p2"] {
        let p = b.ci_stale(id, "base2", 10).unwrap();
        let st = merge_status(p.merge.as_ref().unwrap());
        assert!(!st.can_merge && st.why_not.as_deref().unwrap().contains("moved since"), "{st:?}");
    }
    // p2 passed first: it rebuilds first, alone.
    assert_eq!(b.ci_next_rebuild("sb").as_deref(), Some("p2"));
    let (p, run) = b.ci_rebuild("p2", "base2", 11).unwrap();
    assert_eq!((run, p.ci().unwrap().builds), (2, 2));
    assert!(crate::messages::ci_rebuild_line(&p).unwrap().contains("main moved from base1 to base2"));
    assert_eq!(b.ci_next_rebuild("sb"), None, "one rebuild at a time");
    // Main moves again while p2 rebuilds: coalesced on the newest main.
    let (_, run) = b.ci_rebuild("p2", "base3", 12).unwrap();
    assert_eq!(run, 3);
    assert!(b.ci_built("p2", 2, built("base2", "t2"), 13).is_err(), "the dropped run's answer is ignored");
    b.ci_built("p2", 3, built("base3", "t3"), 13).unwrap();
    assert_eq!(b.ci_next_rebuild("sb"), None);
    b.ci_tested("p2", 3, tests(Outcome::Passed), 14).unwrap();
    assert_eq!(b.ci_next_rebuild("sb").as_deref(), Some("p1"));
    // Abandoned while stale: no longer a rebuild to do.
    b.abandon_merge("p1", 15).unwrap();
    assert_eq!(b.ci_next_rebuild("sb"), None);
}

#[test]
fn a_merge_without_ci_is_as_before() {
    let mut b = PlanBook::new();
    done_plan(&mut b, "p1", "sb");
    let p = b.propose_merge("p1", "sb", vec!["a".into()], "n", vec![], 4).unwrap();
    let m = p.merge.as_ref().unwrap();
    assert_eq!((m.state, m.ci.is_none()), (MergeState::Proposed, true));
    assert!(serde_json::to_value(m).unwrap().get("ci").is_none());
    assert!(b.start_merge("p1", None, None, 5).is_ok());
}
