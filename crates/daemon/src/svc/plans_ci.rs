//! A split plan's CI branch, run by the daemon (see `sbxw_core::plans::ci`):
//! built in a scratch worktree when the lead proposes the merge, tested once
//! through the test queue, rebuilt when main moves (one plan at a time, the
//! oldest Merged to CI first, coalesced on the newest main), and taken to
//! main by the person's click as a fast-forward to exactly the tested tip.

use crate::app::{now_ms, App};
use crate::git;
use crate::svc::plans::MergePins;
use crate::svc::testq::TestAsk;
use sbxw_core::inbox::Kind;
use sbxw_core::messages;
use sbxw_core::plans::{merge_status, Built, CiFailKind, CiFailure, CiSkip, CiStep, CiTests, Merge, MergeResult, MergeState, Plan, PlanState, Variant};
use sbxw_core::session::SessionKind;
use sbxw_core::testq::{JobState, Took};
use sbxw_core::{SandboxRef, ThreadName};
use std::collections::HashSet;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

/// How often main is looked at while a plan of the sandbox has a CI run.
const CI_WATCH_EVERY: Duration = Duration::from_secs(30);
/// One wait on the test queue; the run waits again until the job is done.
const CI_WAIT: Duration = Duration::from_secs(600);

/// The Apps whose watch on main runs (by address: one per App).
fn watching() -> &'static Mutex<HashSet<usize>> {
    static W: OnceLock<Mutex<HashSet<usize>>> = OnceLock::new();
    W.get_or_init(Default::default)
}

fn short(c: &str) -> &str {
    &c[..c.len().min(7)]
}

impl App {
    /// Whether a merge of `order` in `plan` goes through a CI branch: a
    /// split plan with an approved test command whose ordered threads all
    /// work in the workspace's repository, the one the test queue tests.
    /// `Err`: merged as proposed, and why.
    pub(crate) fn ci_applies(&self, plan: &Plan, order: &[String]) -> Result<(), String> {
        if plan.variant != Variant::Split {
            return Err("a shared plan merges its one branch".into());
        }
        if plan.approved_tests.is_none() {
            return Err("the plan has no approved test command".into());
        }
        let Ok(sb) = SandboxRef::parse(&plan.sandbox) else { return Err("no sandbox".into()) };
        let recorded = self.store.get(&sb).threads;
        for name in order {
            let planned = plan.threads.iter().find(|t| &t.name == name).and_then(|t| t.repo.clone());
            let repo = recorded.get(name.as_str()).map(|r| r.repo.clone()).or(planned).unwrap_or_default();
            if !repo.is_empty() && repo != "." {
                return Err(format!("thread {name:?} works in {repo}, not in the workspace's repository the test queue tests"));
            }
        }
        Ok(())
    }

    /// The lead's proposal of a CI merge is recorded: build it.
    pub(crate) fn ci_start(self: &Arc<Self>, plan: &Plan) {
        let Some(c) = plan.ci() else { return };
        let (app, id, run) = (self.clone(), plan.id.clone(), c.run);
        tokio::spawn(async move { app.ci_run(&id, run).await });
    }

    /// Build run `run` of plan `id`'s CI branch, then test its tip; the
    /// lead hears how it went. A step for a run no longer current is
    /// dropped by the book, and this stops there.
    pub(crate) async fn ci_run(self: &Arc<Self>, id: &str, run: u32) {
        let Some(plan) = self.plan(id) else { return };
        let Some(merge) = plan.merge.clone() else { return };
        let Ok(sandbox) = SandboxRef::parse(&plan.sandbox) else { return };
        let fail = |kind, thread: Option<String>, files: Vec<String>, text: String| CiFailure { kind, thread, files, text };
        let built = async {
            let ws = self.store.workspace(&sandbox).ok_or_else(|| format!("sbxw has no workspace recorded for '{sandbox}'"))?;
            let threads: Vec<ThreadName> = merge.order.iter().map(|t| ThreadName::parse(t).map_err(|e| e.to_string())).collect::<Result<_, _>>()?;
            self.ensure_running_for_thread(&sandbox).await.map_err(|e| format!("the sandbox could not be started: {e:#}"))?;
            self.sbx_of(&sandbox).await?.ci_build(&sandbox.name, &ws.to_string_lossy(), id, &threads).await.map_err(|e| format!("{e:#}"))
        }
        .await;
        let step = match built {
            Err(e) => Err(fail(CiFailKind::Error, None, vec![], e)),
            Ok(b) => match (b.conflict, b.error) {
                (Some((t, files)), _) => {
                    let text = format!("sbxw/{t} conflicts in {}", if files.is_empty() { "?".to_string() } else { files.join(", ") });
                    // What merged before it stays on record for the lead's line.
                    let merged = b.merged.iter().map(|(t, tip, c)| CiStep { thread: t.clone(), tip: tip.clone(), commit: c.clone() }).collect();
                    let _ = self.plans.lock().unwrap().ci_partial(id, run, merged, now_ms());
                    Err(fail(CiFailKind::Conflict, Some(t), files, text))
                }
                (None, Some(e)) => Err(fail(CiFailKind::Error, None, vec![], e)),
                (None, None) => Ok(Built {
                    base_branch: b.base_branch,
                    base: b.base,
                    tip: b.tip,
                    merged: b.merged.into_iter().map(|(thread, tip, commit)| CiStep { thread, tip, commit }).collect(),
                    skipped: b.skipped.into_iter().map(|(thread, why)| CiSkip { thread, why }).collect(),
                }),
            },
        };
        let built = match step {
            Err(f) => {
                let r = self.plans.lock().unwrap().ci_failed(id, run, f, now_ms());
                match r {
                    Ok(p) => {
                        let p = self.plan_changed(p);
                        self.ci_tell_lead(&p, messages::ci_failed_line(&p), "failed");
                        self.ci_schedule(&sandbox).await;
                    }
                    Err(_) => self.ci_dropped(id).await,
                }
                return;
            }
            Ok(b) => b,
        };
        let tip = built.tip.clone();
        let r = self.plans.lock().unwrap().ci_built(id, run, built, now_ms());
        let Ok(p) = r else {
            self.ci_dropped(id).await;
            return;
        };
        self.plan_changed(p);
        // The battery, once, through the queue: its lock, its stored result.
        let ask = TestAsk { plan: Some(id), git_ref: Some(&tip), by: Some(sbxw_core::team::SBXW), ..Default::default() };
        let (job, took) = match self.tests_request(&sandbox, ask).await {
            Ok(j) => j,
            Err(e) => {
                let f = CiFailure { kind: CiFailKind::Error, thread: None, files: vec![], text: format!("the tests could not be queued: {e}") };
                let r = self.plans.lock().unwrap().ci_failed(id, run, f, now_ms());
                if let Ok(p) = r {
                    let p = self.plan_changed(p);
                    self.ci_tell_lead(&p, messages::ci_failed_line(&p), "failed");
                }
                return;
            }
        };
        let Ok(p) = self.plans.lock().unwrap().ci_job(id, run, &job.id, now_ms()) else { return };
        self.plan_changed(p);
        self.ci_await(id, run, &sandbox, &job.id, took == Took::Reused).await;
    }

    /// Wait on the CI run's test job and record what it came to.
    async fn ci_await(self: &Arc<Self>, id: &str, run: u32, sandbox: &SandboxRef, job: &str, reused: bool) {
        let job = loop {
            match self.tests_wait(job, sandbox, CI_WAIT).await {
                Ok(j) if j.state == JobState::Done => break j,
                Ok(_) => {}
                // The job is gone from the queue (pruned, a lost state file):
                // tested again, from a fresh build, never left waiting.
                Err(e) => {
                    tracing::info!("plans: {id}: the CI test job {job} is gone ({e}): building and testing again");
                    let (app, id) = (self.clone(), id.to_string());
                    tokio::spawn(app.ci_rebuild_now(&id, run));
                    return;
                }
            }
            // A run dropped meanwhile (a newer main, a re-proposal) stops waiting.
            if self.plan(id).and_then(|p| p.ci().map(|c| c.run)) != Some(run) {
                return;
            }
        };
        let Some(result) = job.result.as_ref() else { return };
        let tests = CiTests::of(&job.id, result, reused || job.reused);
        let Ok(p) = self.plans.lock().unwrap().ci_tested(id, run, tests, now_ms()) else { return };
        let p = self.plan_changed(p);
        if p.merge.as_ref().is_some_and(|m| m.state == MergeState::MergedCi) {
            self.ci_tell_lead(&p, messages::ci_passed_line(&p), "passed");
        } else {
            self.ci_tell_lead(&p, messages::ci_failed_line(&p), "failed");
        }
        if let Ok(sb) = SandboxRef::parse(&p.sandbox) {
            self.ci_schedule(&sb).await;
        }
    }

    fn ci_tell_lead(&self, plan: &Plan, line: Option<String>, step: &str) {
        let (Some(line), Ok(sandbox)) = (line, SandboxRef::parse(&plan.sandbox)) else { return };
        let run = plan.ci().map_or(0, |c| c.run);
        tracing::info!("plans: {}: CI {step} (run {run})", plan.id);
        self.deliver_or_log(&sandbox, &SessionKind::Agent, Kind::Merge, "sbxw", &line, Some(&format!("ci:{}:{run}:{step}", plan.id)));
    }

    /// What the ordered threads' branches hold that the CI branch did not
    /// test, read on the host, in a person's words: a merged thread whose
    /// tip moved since ("sbxw/a has 1 commit the CI branch did not test",
    /// or "was rewritten"), a skipped one that now has commits main lacks.
    /// `None`: what was tested is all there is. A branch gone since is not
    /// untested work (its tested commits are in the CI tip).
    pub(crate) async fn ci_untested(&self, plan: &Plan) -> Option<String> {
        let c = plan.ci()?;
        let base = c.base.as_deref()?;
        let sandbox = SandboxRef::parse(&plan.sandbox).ok()?;
        let ws = self.store.workspace(&sandbox)?;
        let host = self.host_for(&sandbox).await.ok()?;
        let root = git::repo_root(&host, &ws, "").await?;
        let line = |o: Option<String>| o.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
        let mut out = Vec::new();
        let merge = plan.merge.as_ref()?;
        for t in &merge.order {
            let full = format!("refs/heads/sbxw/{t}");
            let Some(tip) = line(git::git(&host, &root, &["rev-parse", "--verify", "--quiet", &full]).await) else { continue };
            let since = match c.merged.iter().find(|s| &s.thread == t) {
                Some(s) if s.tip == tip => continue,
                Some(s) => {
                    if git::git(&host, &root, &["merge-base", "--is-ancestor", &s.tip, &tip]).await.is_none() {
                        out.push(format!("sbxw/{t} was rewritten since the CI branch was built"));
                        continue;
                    }
                    s.tip.clone()
                }
                None => base.to_string(),
            };
            let range = format!("{since}..{tip}");
            let n: u32 = line(git::git(&host, &root, &["rev-list", "--count", &range]).await).and_then(|n| n.parse().ok()).unwrap_or(0);
            if n > 0 {
                out.push(format!("sbxw/{t} has {n} commit{} the CI branch did not test", if n == 1 { "" } else { "s" }));
            }
        }
        (!out.is_empty()).then(|| out.join("; "))
    }

    /// After a CI merge: the threads whose branch now holds commits main
    /// lacks (committed after the check at the click). Their close keeps the
    /// branch, so no work is lost.
    pub(crate) async fn ci_unmerged_branches(&self, plan: &Plan) -> std::collections::HashSet<String> {
        let mut out = std::collections::HashSet::new();
        let (Ok(sandbox), Some(m)) = (SandboxRef::parse(&plan.sandbox), plan.merge.as_ref()) else { return out };
        let (Some(ws), Ok(host)) = (self.store.workspace(&sandbox), self.host_for(&sandbox).await) else { return out };
        let Some(root) = git::repo_root(&host, &ws, "").await else { return out };
        for t in &m.order {
            let full = format!("refs/heads/sbxw/{t}");
            let Some(tip) = git::git(&host, &root, &["rev-parse", "--verify", "--quiet", &full]).await else { continue };
            if git::git(&host, &root, &["merge-base", "--is-ancestor", tip.trim(), "HEAD"]).await.is_none() {
                out.insert(t.clone());
            }
        }
        out
    }

    /// Main's tip in `sandbox`'s workspace repository, read on the host.
    async fn ci_main_tip(&self, sandbox: &SandboxRef) -> Option<String> {
        let ws = self.store.workspace(sandbox)?;
        let host = self.host_for(sandbox).await.ok()?;
        let root = git::repo_root(&host, &ws, "").await?;
        git::git(&host, &root, &["rev-parse", "--verify", "--quiet", "HEAD^{commit}"]).await.map(|s| s.trim().to_string())
    }

    /// Main moved under the CI runs of `sandbox`: a plan Merged to CI on an
    /// older main waits to be rebuilt; a rebuild under way on an older main
    /// is dropped and started once on the newest (its queued job taken
    /// back); then the next rebuild starts if none is at work.
    pub(crate) async fn ci_schedule(self: &Arc<Self>, sandbox: &SandboxRef) {
        let s = sandbox.to_string();
        let ci_plans: Vec<Plan> = self.plans.lock().unwrap().of(&s).into_iter().filter(|p| p.state == PlanState::Approved && p.ci().is_some()).collect();
        if !ci_plans.iter().any(|p| p.merge.as_ref().is_some_and(|m| m.state.ci_busy() || m.state == MergeState::MergedCi)) {
            return;
        }
        let Some(main) = self.ci_main_tip(sandbox).await else { return };
        for p in &ci_plans {
            let (Some(m), Some(c)) = (p.merge.as_ref(), p.ci()) else { continue };
            let Some(base) = c.base.as_deref() else { continue };
            if base == main {
                // Main is where it was; a thread's branch may not be.
                if m.state == MergeState::MergedCi {
                    if let Some(why) = self.ci_untested(p).await {
                        let r = self.plans.lock().unwrap().ci_stale_because(&p.id, &main, Some(&why), now_ms());
                        if let Ok(p) = r {
                            self.plan_changed(p);
                        }
                    }
                }
                continue;
            }
            match m.state {
                MergeState::MergedCi => {
                    let r = self.plans.lock().unwrap().ci_stale(&p.id, &main, now_ms());
                    if let Ok(p) = r {
                        self.plan_changed(p);
                    }
                }
                // Coalesced: a test on an older main is not worth its run.
                MergeState::CiTesting => {
                    if let Some(job) = c.job.as_deref() {
                        if self.tests.lock().unwrap().withdraw(job) {
                            tracing::info!("tests: '{sandbox}' job {job} taken back: main moved under {}", p.id);
                            self.tests_changed();
                        }
                    }
                    self.ci_rebuild(&p.id, &main).await;
                }
                _ => {}
            }
        }
        // Picked and started under one lock: two looks at main at once never
        // start two rebuilds.
        let next = {
            let mut b = self.plans.lock().unwrap();
            b.ci_next_rebuild(&s).and_then(|id| b.ci_rebuild(&id, &main, now_ms()).ok())
        };
        if let Some((p, run)) = next {
            self.ci_rebuilt(p, run);
        }
    }

    async fn ci_rebuild(self: &Arc<Self>, id: &str, main: &str) {
        let r = self.plans.lock().unwrap().ci_rebuild(id, main, now_ms());
        if let Ok((p, run)) = r {
            self.ci_rebuilt(p, run);
        }
    }

    /// A rebuild recorded: the lead told, the build started.
    fn ci_rebuilt(self: &Arc<Self>, p: Plan, run: u32) {
        let id = p.id.clone();
        let p = self.plan_changed(p);
        self.ci_tell_lead(&p, messages::ci_rebuild_line(&p), &format!("rebuild-{run}"));
        let app = self.clone();
        tokio::spawn(async move { app.ci_run(&id, run).await });
    }

    /// After a restart: the CI runs that were building start again; those
    /// testing wait on their job again. Then a watch on main, every
    /// [`CI_WATCH_EVERY`], for the sandboxes with a CI run.
    pub(crate) fn ci_resume(self: &Arc<Self>) {
        let in_flight = self.plans.lock().unwrap().ci_in_flight();
        for (id, state, run) in in_flight {
            let app = self.clone();
            tokio::spawn(async move {
                let Some(p) = app.plan(&id) else { return };
                let Ok(sb) = SandboxRef::parse(&p.sandbox) else { return };
                match (state, p.ci().and_then(|c| c.job.clone())) {
                    (MergeState::CiTesting, Some(job)) => app.ci_await(&id, run, &sb, &job, false).await,
                    // Built but not yet queued, or building: from scratch.
                    _ => app.ci_rebuild_now(&id, run).await,
                }
            });
        }
        let me = Arc::as_ptr(self) as usize;
        if !watching().lock().unwrap().insert(me) {
            return;
        }
        let weak = Arc::downgrade(self);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(CI_WATCH_EVERY).await;
                let Some(app) = weak.upgrade() else { break };
                app.ci_watch(None).await;
            }
            watching().lock().unwrap().remove(&me);
        });
    }

    /// A run cut short by a restart: built again from scratch, as the same
    /// proposal (a building one has no result to keep).
    ///
    /// Boxed as `Send`: it is spawned from inside the wait it leads back
    /// to (`ci_await`), and a named future type ends that cycle.
    fn ci_rebuild_now(self: &Arc<Self>, id: &str, run: u32) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'static>> {
        let (app, id) = (self.clone(), id.to_string());
        Box::pin(async move {
            let reset = app.plans.lock().unwrap().ci_restart(&id, run, now_ms());
            if let Ok(p) = reset {
                let run = p.ci().map_or(run, |c| c.run);
                app.plan_changed(p);
                app.ci_run(&id, run).await;
            }
        })
    }

    /// Look at main for every sandbox (or those named) with a CI run.
    pub(crate) async fn ci_watch(self: &Arc<Self>, sandboxes: Option<&[String]>) {
        let mut seen: Vec<String> = self
            .plans
            .lock()
            .unwrap()
            .list()
            .into_iter()
            .filter(|p| p.state == PlanState::Approved && p.ci().is_some() && sandboxes.is_none_or(|s| s.contains(&p.sandbox)))
            .map(|p| p.sandbox)
            .collect();
        seen.sort();
        seen.dedup();
        for s in seen {
            if let Ok(sb) = SandboxRef::parse(&s) {
                self.ci_schedule(&sb).await;
            }
        }
    }

    /// Remove plan `plan`'s CI worktree and branch, quietly: merged,
    /// abandoned or closed.
    pub(crate) async fn ci_remove(&self, plan: &Plan) {
        let Ok(sandbox) = SandboxRef::parse(&plan.sandbox) else { return };
        let Some(ws) = self.store.workspace(&sandbox) else { return };
        let res = match self.sbx_of(&sandbox).await {
            Ok(sbx) => sbx.ci_remove(&sandbox.name, &ws.to_string_lossy(), &plan.id).await.map_err(|e| format!("{e:#}")),
            Err(e) => Err(e),
        };
        match res {
            Ok(()) => tracing::info!("plans: {}: CI branch and worktree removed", plan.id),
            Err(e) => tracing::warn!("plans: {}: could not remove the CI branch: {e}", plan.id),
        }
    }

    /// The plan was closed (lead mode ended, closed from the Kanban): a CI
    /// run not merged is over and its branch removed.
    pub(crate) async fn ci_close(self: &Arc<Self>, id: &str) {
        let job = self.plan(id).and_then(|p| p.ci().and_then(|c| c.job.clone()));
        let closed = self.plans.lock().unwrap().ci_closed(id, now_ms());
        if let Some(p) = closed {
            let p = self.plan_changed(p);
            if let Some(job) = job {
                if self.tests.lock().unwrap().withdraw(&job) {
                    tracing::info!("tests: job {job} taken back: plan {id} was closed");
                    self.tests_changed();
                }
            }
            self.ci_remove(&p).await;
        }
    }

    /// A step of run `run` was refused by the book: when the plan no longer
    /// wants a CI branch (abandoned, closed while it was built), what the
    /// build left is removed. A newer run (a rebuild) keeps its branch.
    async fn ci_dropped(&self, id: &str) {
        let Some(p) = self.plan(id) else { return };
        let gone = p.closed || p.dismissed || p.state != PlanState::Approved || p.merge.as_ref().is_none_or(|m| m.state == MergeState::Abandoned);
        if gone {
            self.ci_remove(&p).await;
        }
    }

    /// The person's click on a plan Merged to CI: main fast-forwarded to
    /// the tested CI tip, if main is still where the CI branch was built
    /// and the checkout holds no edit on a path the merge changes (as a
    /// thread merge). Pinned (the phone): each thread's approved tip must be
    /// the one the CI branch merged, and the approved base the one it was
    /// built on. A refusal leaves the plan Merged to CI with the reason;
    /// main moved sends it to be rebuilt.
    pub(crate) async fn plan_merge_ci(self: &Arc<Self>, plan: Plan, sandbox: &SandboxRef, close_threads: bool, distillate: Option<Vec<sbxw_core::plans::DistillItem>>, pins: Option<&MergePins>) -> Result<Merge, String> {
        let id = plan.id.clone();
        let merge = plan.merge.clone().ok_or_else(|| format!("plan {id} has no merge proposed"))?;
        let c = merge.ci.clone().ok_or("not a CI merge")?;
        if merge.state != MergeState::MergedCi {
            let st = merge_status(&merge);
            return Err(format!("plan {id} cannot be merged yet: {}", st.why_not.unwrap_or(st.detail)));
        }
        let (Some(tip), Some(base), Some(base_branch)) = (c.tip.clone(), c.base.clone(), c.base_branch.clone()) else {
            return Err(format!("the CI branch of plan {id} has no tip on record: the lead proposes the merge again"));
        };
        if let Some(pins) = pins {
            for s in &c.merged {
                match pins.get(&s.thread) {
                    Some((t, b)) if t == &s.tip && b == &base => {}
                    Some(_) => return Err(format!("changed: sbxw/{} or {base_branch} is not what the CI branch tested", s.thread)),
                    None => return Err(format!("changed: sbxw/{} was not in the approved merge", s.thread)),
                }
            }
        }
        // A thread committed after its CI branch was tested: that commit was
        // never tested, so the click is refused and the CI branch rebuilt
        // with it, as when main moves. Pins or not.
        if let Some(why) = self.ci_untested(&plan).await {
            let r = self.plans.lock().unwrap().ci_stale_because(&id, &base, Some(&why), now_ms());
            if let Ok(p) = r {
                self.plan_changed(p);
            }
            self.ci_schedule(sandbox).await;
            return Err(format!("{why}: sbxw rebuilds the CI branch with it and tests it again; Merge comes back when it passes"));
        }
        let ws = self.store.workspace(sandbox).ok_or_else(|| format!("sbxw has no workspace recorded for '{sandbox}'"))?;
        let p = self.plans.lock().unwrap().start_merge(&id, Some(base_branch.clone()), distillate, now_ms())?;
        self.plan_changed(p);
        tracing::info!("plans: merging {id} for '{sandbox}': fast-forward {base_branch} to {}", short(&tip));
        let outcome = async {
            self.ensure_running_for_thread(sandbox).await.map_err(|e| format!("the sandbox could not be started: {e:#}"))?;
            self.sbx_of(sandbox).await?.ci_fast_forward(&sandbox.name, &ws.to_string_lossy(), &tip, &base, &base_branch).await.map_err(|e| self.with_split_note(sandbox, format!("{e:#}")))
        }
        .await;
        let outcome = match outcome {
            Ok(Ok(o)) => o,
            Ok(Err(now)) => {
                let why = format!("{base_branch} moved since the CI branch was built (now {}): sbxw rebuilds it on the new {base_branch} and tests it again; Merge comes back when it passes", short(&now));
                let r = self.plans.lock().unwrap().ci_click_refused(&id, &why, now_ms());
                if let Ok(p) = r {
                    self.plan_changed(p);
                }
                self.ci_schedule(sandbox).await;
                return Err(why);
            }
            Err(e) => {
                let r = self.plans.lock().unwrap().ci_click_refused(&id, &e, now_ms());
                if let Ok(p) = r {
                    self.plan_changed(p);
                }
                return Err(e);
            }
        };
        let landed = format!("fast-forwarded {base_branch} to the tested CI tip {}", short(&tip));
        let extra = outcome.note().map(|n| format!("; {n}")).unwrap_or_default();
        for t in &merge.order {
            let (commit, note) = match (c.merged.iter().find(|s| &s.thread == t), c.skipped.iter().find(|s| &s.thread == t)) {
                (Some(s), _) => (Some(short(&s.commit).to_string()), format!("{landed}{extra}")),
                (None, Some(k)) => (None, format!("nothing to merge, {}", k.why)),
                (None, None) => (None, format!("nothing to merge: not in {}", c.branch)),
            };
            let r = MergeResult { thread: t.clone(), ok: true, commit, error: None, note: Some(note) };
            let p = self.plans.lock().unwrap().merge_result(&id, r, now_ms())?;
            self.plan_changed(p);
        }
        let p = self.plans.lock().unwrap().finish_merge(&id, now_ms())?;
        let p = self.plan_changed(p);
        self.ci_remove(&p).await;
        let merge = p.merge.clone().unwrap_or(merge);
        let done_before: Vec<String> = vec![];
        let merge = self.merge_wrap_up(sandbox, &id, &p, merge, &done_before, close_threads).await;
        // Main moved: the other plans Merged to CI wait their turn.
        self.ci_schedule(sandbox).await;
        Ok(merge)
    }
}
