//! A split plan's CI branch: what the lead's `propose_merge` starts when the
//! plan has an approved test command.
//!
//! The daemon builds `sbxw/ci-<plan>` from main's tip, merging the thread
//! branches in the lead's order, runs the plan's battery once on its tip
//! through the test queue, and the person's click then fast-forwards main
//! to exactly the tree that was tested. States, in the plan's merge:
//!
//! ```text
//! propose_merge ─▶ ci_building ─┬─ conflict / error ─▶ ci_failed
//!                               └▶ ci_testing ─┬─ passed ─▶ merged_ci ("Merged to CI")
//!                                              └─ failed ─▶ ci_failed
//! merged_ci ── main moved ─▶ ci_stale ── its turn ─▶ ci_building (rebuilt on the new main)
//! merged_ci ── the click ─▶ merging ─┬─ fast-forward ─▶ merged ("Merged to main")
//!                                    └─ refused ─▶ merged_ci (the reason kept)
//! ```
//!
//! Every build has a run number: a result that comes back for an older run
//! (a re-proposal or a rebuild started since) is dropped. The book is pure,
//! as the rest of `plans`: the daemon runs git and the queue.

use super::{Merge, MergeState, Plan, PlanBook, PlanState};
use crate::team::{self, EventKind};
use crate::testq::{Attempt, Outcome, TestResult};
use serde::{Deserialize, Serialize};
use serde_json::json;

/// The words the person reads for the two ends of a CI merge.
pub const MERGED_TO_CI: &str = "Merged to CI";
pub const MERGED_TO_MAIN: &str = "Merged to main";

/// A plan's CI branch, `sbxw/ci-<plan>`.
pub fn ci_branch(plan_id: &str) -> String {
    format!("sbxw/ci-{plan_id}")
}

/// Its scratch worktree, under the repository's `.sbxw-worktrees/` (a dot
/// name, so never a thread's; excluded like the test queue's `.tests`).
pub fn ci_worktree(plan_id: &str) -> String {
    format!(".sbxw-worktrees/.ci-{plan_id}")
}

/// One thread merged into the CI branch.
#[derive(Clone, Serialize, Deserialize, Debug, PartialEq, Eq)]
pub struct CiStep {
    pub thread: String,
    /// The thread's branch tip that was merged (full id).
    pub tip: String,
    /// The merge commit it made in the CI branch (full id): after the
    /// fast-forward, the same commit is in main.
    pub commit: String,
}

/// A thread of the order with nothing to merge: its branch already in main,
/// or gone.
#[derive(Clone, Serialize, Deserialize, Debug, PartialEq, Eq)]
pub struct CiSkip {
    pub thread: String,
    pub why: String,
}

#[derive(Clone, Copy, Serialize, Deserialize, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CiFailKind {
    /// A thread's branch conflicted while the CI branch was built.
    Conflict,
    /// The battery failed, timed out or could not run.
    Tests,
    /// The CI branch could not be built (sandbox, repository, git).
    Error,
}

/// Why nothing reached main.
#[derive(Clone, Serialize, Deserialize, Debug, PartialEq, Eq)]
pub struct CiFailure {
    pub kind: CiFailKind,
    /// The thread whose branch conflicted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread: Option<String>,
    /// The conflicting files (five at most).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<String>,
    pub text: String,
}

/// The battery's result on the CI tip, each attempt with its tail and log.
#[derive(Clone, Serialize, Deserialize, Debug, PartialEq, Eq)]
pub struct CiTests {
    pub job: String,
    pub outcome: Outcome,
    /// Answered from a stored result for the same tree.
    #[serde(default)]
    pub reused: bool,
    /// The commit it actually ran on (a reused result: the first).
    pub commit: String,
    /// Oldest first: one, or two when the first failed and it ran again.
    pub attempts: Vec<Attempt>,
}

impl CiTests {
    pub fn of(job: &str, result: &TestResult, reused: bool) -> CiTests {
        let last = Attempt { outcome: result.outcome, duration_ms: result.duration_ms.saturating_sub(result.first.as_ref().map_or(0, |f| f.duration_ms)), log: result.log.clone(), tail: result.tail.clone() };
        let attempts = result.first.iter().cloned().chain(std::iter::once(last)).collect();
        CiTests { job: job.to_string(), outcome: result.outcome, reused, commit: result.commit.clone(), attempts }
    }
}

/// A merge's CI run.
#[derive(Clone, Serialize, Deserialize, Debug, PartialEq, Eq, Default)]
pub struct Ci {
    pub branch: String,
    pub worktree: String,
    /// The build this is: every new build (a re-proposal is a new `Ci`, a
    /// rebuild bumps it) gets the next one; results of an older one are
    /// dropped.
    pub run: u32,
    /// Builds of this proposal: 1, plus one per rebuild after main moved.
    pub builds: u32,
    /// The branch the main checkout is on, and the commit of it the CI
    /// branch was built on (full id).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<String>,
    /// The CI branch's tip once built (full id): what is tested, and what
    /// main is fast-forwarded to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tip: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub merged: Vec<CiStep>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skipped: Vec<CiSkip>,
    /// Why this build is a rebuild: "main moved from abc1234 to def5678".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rebuilt_because: Option<String>,
    /// Main's tip when it was seen moved ([`MergeState::CiStale`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub main_now: Option<String>,
    /// The test job on the tip.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub job: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tests: Option<CiTests>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure: Option<CiFailure>,
    /// When the battery passed: the order rebuilds take (oldest first).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub passed_ms: Option<u64>,
    /// The last click that was refused (an edit in the main checkout on a
    /// path the merge changes): the plan stays Merged to CI.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub click_refused: Option<String>,
}

/// The CI branch as built, for [`PlanBook::ci_built`].
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Built {
    pub base_branch: String,
    pub base: String,
    pub tip: String,
    pub merged: Vec<CiStep>,
    pub skipped: Vec<CiSkip>,
}

/// What the plan view says of a merge, for a screen to draw as it is.
#[derive(Clone, Serialize, Debug, PartialEq, Eq)]
pub struct MergeStatus {
    /// The status, in the person's words.
    pub word: &'static str,
    /// One line on it.
    pub detail: String,
    /// The click (Merge) may be offered now.
    pub can_merge: bool,
    /// Why it is not offered, when the merge is not over.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub why_not: Option<String>,
    /// The merge runs through a CI branch.
    pub ci: bool,
}

fn short(c: &str) -> &str {
    &c[..c.len().min(7)]
}

/// The status of `m`, in words a screen and the lead read as they are.
pub fn merge_status(m: &Merge) -> MergeStatus {
    let ci = m.ci.as_ref();
    let s = |word, detail: String, can_merge, why_not: Option<&str>| MergeStatus { word, detail, can_merge, why_not: why_not.map(str::to_string), ci: ci.is_some() };
    let branch = ci.map_or(String::new(), |c| c.branch.clone());
    let base = ci.and_then(|c| c.base_branch.clone()).unwrap_or_else(|| "main".into());
    match m.state {
        MergeState::Proposed => s("Merge proposed", "The lead proposed the merge: it waits for your click".into(), true, None),
        MergeState::Failed => s("Merge failed", "The merge stopped: the results say why. It can be retried".into(), true, None),
        MergeState::Abandoned => s("Merge abandoned", "You abandoned this merge: nothing was merged".into(), false, None),
        MergeState::Merging => s("Merging", format!("Merging into {base}"), false, Some("the merge is running")),
        MergeState::Merged if ci.is_some() => s(MERGED_TO_MAIN, format!("{base} is now the tested tip of {branch}"), false, None),
        MergeState::Merged => s(MERGED_TO_MAIN, format!("Merged into {}", m.base.as_deref().unwrap_or("main")), false, None),
        MergeState::CiBuilding => {
            let why = match ci.and_then(|c| c.rebuilt_because.as_deref()) {
                Some(r) => format!("{r}: sbxw is rebuilding {branch} on the new {base} and will test it again; Merge comes back when it passes"),
                None => format!("sbxw is building {branch} from {base} with the threads' branches; the tests run next, then Merge is offered"),
            };
            s("Building the CI branch", format!("Merging the threads into {branch}"), false, Some(&why))
        }
        MergeState::CiTesting => {
            let tip = ci.and_then(|c| c.tip.as_deref()).map(short).unwrap_or("?");
            let why = match ci.and_then(|c| c.rebuilt_because.as_deref()) {
                Some(r) => format!("{r}: the rebuilt {branch} is being tested; Merge comes back when it passes"),
                None => format!("the plan's tests run on {branch}; Merge is offered when they pass"),
            };
            s("Testing the CI branch", format!("The plan's tests run on {branch} at {tip}"), false, Some(&why))
        }
        MergeState::MergedCi => {
            let tip = ci.and_then(|c| c.tip.as_deref()).map(short).unwrap_or("?");
            let detail = match ci.and_then(|c| c.click_refused.as_deref()) {
                Some(e) => format!("Tests passed on {branch} at {tip}. The last Merge was refused: {e}"),
                None => format!("Tests passed on {branch} at {tip}: Merge fast-forwards {base} to exactly that commit"),
            };
            s(MERGED_TO_CI, detail, true, None)
        }
        MergeState::CiStale => {
            let why = format!(
                "{base} moved since {branch} was built: sbxw rebuilds it on the new {base} and tests it again before Merge is offered. \
                 One plan rebuilds at a time, the oldest first, so this one waits its turn"
            );
            s("Main moved: CI rebuild waits", format!("{MERGED_TO_CI} on an older {base}"), false, Some(&why))
        }
        MergeState::CiFailed => {
            let f = ci.and_then(|c| c.failure.as_ref());
            let detail = match f {
                Some(f) if f.kind == CiFailKind::Conflict => format!(
                    "sbxw/{} conflicts in {}: nothing reached {base}, no tests ran",
                    f.thread.as_deref().unwrap_or("?"),
                    if f.files.is_empty() { "?".to_string() } else { f.files.join(", ") }
                ),
                Some(f) if f.kind == CiFailKind::Tests => format!("The tests {} on {branch}: nothing reached {base}", f.text),
                Some(f) => format!("{branch} could not be built: {}", f.text),
                None => format!("{branch} failed: nothing reached {base}"),
            };
            s("CI failed", detail, false, Some("the lead has the threads fix it, then proposes the merge again, which rebuilds and retests the CI branch"))
        }
    }
}

impl MergeState {
    /// A CI run is at work on this merge (built or tested, or waiting to be
    /// rebuilt).
    pub fn ci_busy(self) -> bool {
        matches!(self, MergeState::CiBuilding | MergeState::CiTesting | MergeState::CiStale)
    }
}

impl Plan {
    /// The CI run of the plan's current merge, if it has one.
    pub fn ci(&self) -> Option<&Ci> {
        self.merge.as_ref().and_then(|m| m.ci.as_ref())
    }
}

impl PlanBook {
    /// A fresh CI run for a merge just proposed: the next run number of the
    /// plan (above any earlier merge's).
    pub(super) fn new_ci(plan: &Plan) -> Ci {
        let last = plan.earlier_merges.iter().chain(plan.merge.iter()).filter_map(|m| m.ci.as_ref()).map(|c| c.run).max().unwrap_or(0);
        Ci { branch: ci_branch(&plan.id), worktree: ci_worktree(&plan.id), run: last + 1, builds: 1, ..Default::default() }
    }

    /// The CI run `run` of plan `id`, in state `want`, to change with `f`.
    fn ci_step<F>(&mut self, id: &str, run: u32, want: &[MergeState], now: u64, f: F) -> Result<Plan, String>
    where
        F: FnOnce(&mut Merge) -> Result<(), String>,
    {
        self.mutate(id, now, |p| {
            if p.state != PlanState::Approved {
                return Err(format!("plan {id} is not approved"));
            }
            let m = p.merge.as_mut().ok_or_else(|| format!("plan {id} has no merge"))?;
            match &m.ci {
                Some(c) if c.run == run && want.contains(&m.state) => f(m),
                Some(_) => Err(format!("the CI run {run} of plan {id} is no longer the current one")),
                None => Err(format!("the merge of plan {id} has no CI branch")),
            }
        })
    }

    fn ci_note(&mut self, id: &str, now: u64, step: &str, text: &str, mut data: serde_json::Value) {
        data["step"] = step.into();
        if let Some(m) = self.get(id).and_then(|p| p.merge.as_ref()) {
            data["state"] = json!(m.state);
            data["status"] = merge_status(m).word.into();
        }
        self.note(id, now, EventKind::CiStep, team::SBXW, None, Some(text), data);
    }

    /// The CI branch is built: on to the tests.
    pub fn ci_built(&mut self, id: &str, run: u32, built: Built, now: u64) -> Result<Plan, String> {
        let p = self.ci_step(id, run, &[MergeState::CiBuilding], now, |m| {
            let c = m.ci.as_mut().expect("ci");
            c.base_branch = Some(built.base_branch.clone());
            c.base = Some(built.base.clone());
            c.tip = Some(built.tip.clone());
            c.merged = built.merged.clone();
            c.skipped = built.skipped.clone();
            m.base = Some(built.base_branch.clone());
            m.state = MergeState::CiTesting;
            Ok(())
        })?;
        let c = p.ci().expect("ci");
        let text = format!("{} built at {} on {}", c.branch, short(&built.tip), short(&built.base));
        self.ci_note(id, now, "built", &text, json!({ "run": run, "tip": built.tip, "base": built.base }));
        Ok(p)
    }

    /// The test job on the CI tip.
    pub fn ci_job(&mut self, id: &str, run: u32, job: &str, now: u64) -> Result<Plan, String> {
        self.ci_step(id, run, &[MergeState::CiTesting], now, |m| {
            m.ci.as_mut().expect("ci").job = Some(job.to_string());
            Ok(())
        })
    }

    /// The CI branch could not be built (a conflict or an error), or its
    /// tests failed: nothing reached main.
    pub fn ci_failed(&mut self, id: &str, run: u32, failure: CiFailure, now: u64) -> Result<Plan, String> {
        let text = failure.text.clone();
        let data = json!({ "run": run, "kind": failure.kind, "thread": failure.thread, "files": failure.files });
        let p = self.ci_step(id, run, &[MergeState::CiBuilding, MergeState::CiTesting], now, |m| {
            m.ci.as_mut().expect("ci").failure = Some(failure);
            m.state = MergeState::CiFailed;
            Ok(())
        })?;
        self.ci_note(id, now, "failed", &text, data);
        Ok(p)
    }

    /// The battery came back on the CI tip: passed is Merged to CI, anything
    /// else is CI failed.
    pub fn ci_tested(&mut self, id: &str, run: u32, tests: CiTests, now: u64) -> Result<Plan, String> {
        let passed = tests.outcome == Outcome::Passed;
        let word = tests.outcome.word();
        let data = json!({ "run": run, "job": tests.job, "outcome": tests.outcome, "reused": tests.reused });
        let p = self.ci_step(id, run, &[MergeState::CiTesting], now, |m| {
            let c = m.ci.as_mut().expect("ci");
            if passed {
                c.passed_ms = Some(now);
                c.failure = None;
                m.state = MergeState::MergedCi;
            } else {
                c.failure = Some(CiFailure { kind: CiFailKind::Tests, thread: None, files: vec![], text: word.to_string() });
                m.state = MergeState::CiFailed;
            }
            c.tests = Some(tests);
            Ok(())
        })?;
        let c = p.ci().expect("ci");
        let text = if passed { format!("{MERGED_TO_CI}: tests passed on {} at {}", c.branch, c.tip.as_deref().map(short).unwrap_or("?")) } else { format!("CI failed: the tests {word} on {}", c.branch) };
        self.ci_note(id, now, if passed { "passed" } else { "failed" }, &text, data);
        Ok(p)
    }

    /// Main is at `main_now`, no longer the commit a Merged to CI plan was
    /// built on: it waits its turn to be rebuilt.
    pub fn ci_stale(&mut self, id: &str, main_now: &str, now: u64) -> Result<Plan, String> {
        let run = self.get(id).and_then(Plan::ci).map(|c| c.run).unwrap_or(0);
        let p = self.ci_step(id, run, &[MergeState::MergedCi], now, |m| {
            m.ci.as_mut().expect("ci").main_now = Some(main_now.to_string());
            m.state = MergeState::CiStale;
            Ok(())
        })?;
        self.ci_note(id, now, "stale", "main moved: the CI branch waits to be rebuilt", json!({ "run": run, "main": main_now }));
        Ok(p)
    }

    /// A rebuild on main at `main_now`: from a stale plan's turn, or a
    /// rebuild under way when main moved again (coalesced: that run is
    /// dropped, one starts on the newest main). Returns the new run.
    pub fn ci_rebuild(&mut self, id: &str, main_now: &str, now: u64) -> Result<(Plan, u32), String> {
        let run = self.get(id).and_then(Plan::ci).map(|c| c.run).unwrap_or(0);
        let p = self.ci_step(id, run, &[MergeState::CiStale, MergeState::CiBuilding, MergeState::CiTesting], now, |m| {
            let c = m.ci.as_mut().expect("ci");
            let was = c.base.as_deref().map(short).unwrap_or("?").to_string();
            let fresh = Ci {
                branch: c.branch.clone(),
                worktree: c.worktree.clone(),
                run: c.run + 1,
                builds: c.builds + 1,
                rebuilt_because: Some(format!("main moved from {was} to {}", short(main_now))),
                base_branch: c.base_branch.clone(),
                passed_ms: c.passed_ms,
                ..Default::default()
            };
            *c = fresh;
            m.state = MergeState::CiBuilding;
            Ok(())
        })?;
        let c = p.ci().expect("ci");
        let (new_run, why) = (c.run, c.rebuilt_because.clone().unwrap_or_default());
        self.ci_note(id, now, "rebuild", &format!("{why}: rebuilding {}", c.branch), json!({ "run": new_run, "main": main_now }));
        Ok((p, new_run))
    }

    /// The person's click was refused before anything moved (an edit in
    /// the main checkout on a path the merge changes, main moved): the
    /// plan is Merged to CI again, the reason kept.
    pub fn ci_click_refused(&mut self, id: &str, why: &str, now: u64) -> Result<Plan, String> {
        let run = self.get(id).and_then(Plan::ci).map(|c| c.run).unwrap_or(0);
        self.ci_step(id, run, &[MergeState::Merging], now, |m| {
            m.ci.as_mut().expect("ci").click_refused = Some(why.to_string());
            m.results.clear();
            m.state = MergeState::MergedCi;
            Ok(())
        })
    }

    /// The next plan of `sandbox` to rebuild after main moved: none while
    /// one rebuild is at work; else the stale plan that passed first.
    pub fn ci_next_rebuild(&self, sandbox: &str) -> Option<String> {
        let mine: Vec<&Plan> = self.plans.values().filter(|p| p.sandbox == sandbox && p.state == PlanState::Approved && !p.closed && !p.dismissed).collect();
        let rebuilding = mine.iter().any(|p| {
            p.merge.as_ref().is_some_and(|m| matches!(m.state, MergeState::CiBuilding | MergeState::CiTesting) && m.ci.as_ref().is_some_and(|c| c.rebuilt_because.is_some()))
        });
        if rebuilding {
            return None;
        }
        mine.into_iter()
            .filter(|p| p.merge.as_ref().is_some_and(|m| m.state == MergeState::CiStale))
            .min_by_key(|p| (p.ci().and_then(|c| c.passed_ms).unwrap_or(u64::MAX), p.id.clone()))
            .map(|p| p.id.clone())
    }

    /// Plans of `sandbox` whose CI run is at work or waits on its tests:
    /// what the daemon picks up after a restart.
    pub fn ci_in_flight(&self) -> Vec<(String, MergeState, u32)> {
        self.plans
            .values()
            .filter(|p| p.state == PlanState::Approved)
            .filter_map(|p| {
                let m = p.merge.as_ref()?;
                let c = m.ci.as_ref()?;
                matches!(m.state, MergeState::CiBuilding | MergeState::CiTesting).then(|| (p.id.clone(), m.state, c.run))
            })
            .collect()
    }
}

#[cfg(test)]
mod tests;
