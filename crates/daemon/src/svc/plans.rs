//! Plans around their book: persistence, the lead's waits, launching an
//! approved plan's threads, the one-click merge, and lead mode itself.

use crate::app::{now_ms, App};
use crate::bus::{Event, ThreadClosed};
use crate::git;
use crate::store::{write_atomic, LeadRecord, Store, LEAD_UNBOUND_MAX_MS};
use sbxw_core::agents::SessionInfo;
use std::collections::{BTreeMap, HashMap};
use sbxw_core::messages;
use sbxw_core::plans::{DistillItem, MergeResult, MergeState, Plan, PlanState, PlanThread, ThreadState, Variant};
use sbxw_sbx::scripts::Team;
use sbxw_core::session::{SessionKey, SessionKind};
use sbxw_core::tooling::Tooling;
use sbxw_core::{ModelId, SandboxRef, ThreadName};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::broadcast::error::RecvError;

/// The commits a merge approved from the phone must take, per thread (a
/// shared plan: its team branch's name): `(branch tip, base tip)`, full ids.
pub type MergePins = HashMap<String, (String, String)>;

impl App {
    /// [`App::sbx_for`], its error as the plain text these services return.
    async fn sbx_of(&self, sb: &SandboxRef) -> Result<sbxw_sbx::Sbx, String> {
        self.sbx_for(sb).await.map_err(|e| format!("{e:#}"))
    }
}

impl App {
    /// Write `sandbox`'s plans (or remove the file when none are left) and
    /// broadcast. Every change comes through here. A plan id is a capability,
    /// hence the 0700 directory.
    fn plan_changed(&self, plan: Plan) -> Plan {
        self.persist_plans(&plan.sandbox);
        self.send(Event::Plan(plan.clone()));
        self.team_changed(&plan.id);
        plan
    }

    pub(crate) fn persist_plans(&self, sandbox: &str) {
        let list = self.plans.lock().unwrap().of(sandbox);
        let path = self.store.paths.plans().join(format!("{sandbox}.json"));
        let res = if list.is_empty() {
            match std::fs::remove_file(&path) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
                _ => Ok(()),
            }
        } else {
            serde_json::to_vec_pretty(&list)
                .map_err(anyhow::Error::from)
                .and_then(|b| write_atomic(&path, &b))
        };
        if let Err(e) = res {
            tracing::warn!("plans: could not save {}: {e:#}", path.display());
        }
        self.persist_team_logs(sandbox);
    }

    pub fn plans_list(&self) -> Vec<Plan> {
        self.plans.lock().unwrap().list()
    }

    pub fn plan(&self, id: &str) -> Option<Plan> {
        self.plans.lock().unwrap().get(id).cloned()
    }

    /// A plan as the lead sees it, plus whether each thread's session is
    /// running — the one fact a report cannot carry.
    pub fn plan_view(&self, plan: &Plan) -> serde_json::Value {
        let mut v = serde_json::to_value(plan).unwrap_or_default();
        if let Ok(sb) = SandboxRef::parse(&plan.sandbox) {
            if let Some(threads) = v.get_mut("threads").and_then(|t| t.as_array_mut()) {
                for (t, pt) in threads.iter_mut().zip(&plan.threads) {
                    let live = ThreadName::parse(&pt.name)
                        .map(|n| self.sessions.is_live(&SessionKey::thread(&sb, &n)))
                        .unwrap_or(false);
                    t["running"] = live.into();
                }
            }
        }
        v
    }

    /// Every thread's repo resolvable, without recording anything: a plan
    /// whose threads could not start is refused before anything is started.
    async fn check_plan_repos(&self, sandbox: &SandboxRef, threads: &[PlanThread]) -> Result<(), String> {
        let Some(ws) = self.store.workspace(sandbox) else {
            return if threads.iter().any(|t| t.repo.is_some()) {
                Err(format!("sbxw has no workspace recorded for '{sandbox}', so threads cannot name a repo"))
            } else {
                Ok(())
            };
        };
        let rec = self.store.get(sandbox);
        let host = self.host_for(sandbox).await.map_err(|e| format!("{e:#}"))?;
        for t in threads {
            let name = t.name.trim();
            let recorded = rec.threads.get(name).map(|r| r.repo.as_str());
            git::choose(&host, &ws, recorded, name, t.repo.as_deref()).await.map_err(|e| format!("thread {name:?}: {e}"))?;
        }
        Ok(())
    }

    /// Uncommitted changes in the main checkouts the threads' worktrees
    /// will be cut from, summed over the repos they resolve to, read with git
    /// on the host (where the whole repository is). `None` without a
    /// workspace or when one could not be read.
    async fn main_uncommitted(&self, sandbox: &SandboxRef, threads: &[PlanThread]) -> Option<u32> {
        let ws = self.store.workspace(sandbox)?;
        let rec = self.store.get(sandbox);
        let host = self.host_for(sandbox).await.ok()?;
        let mut repos = std::collections::BTreeSet::new();
        for t in threads {
            let recorded = rec.threads.get(t.name.as_str()).map(|r| r.repo.as_str());
            repos.insert(git::choose(&host, &ws, recorded, &t.name, t.repo.as_deref()).await.ok()?);
        }
        let mut n = 0;
        for rel in repos {
            n += git::uncommitted(&host, &git::thread_dir(&ws, &rel)).await?;
        }
        Some(n)
    }

    /// A shared plan's threads all work in one worktree, so in one repo: the
    /// one they resolve to, and the worktree's path in it. `Ok(None)` when the
    /// sandbox has no workspace recorded (the scripts find the repo there).
    async fn shared_worktree(&self, sandbox: &SandboxRef, threads: &[PlanThread], team: &str) -> Result<Option<String>, String> {
        let Some(ws) = self.store.workspace(sandbox) else { return Ok(None) };
        let rec = self.store.get(sandbox);
        let host = self.host_for(sandbox).await.map_err(|e| format!("{e:#}"))?;
        let mut repos = std::collections::BTreeSet::new();
        for t in threads {
            let name = t.name.trim();
            let recorded = rec.threads.get(name).map(|r| r.repo.as_str());
            repos.insert(git::choose(&host, &ws, recorded, name, t.repo.as_deref()).await.map_err(|e| format!("thread {name:?}: {e}"))?);
        }
        match repos.len() {
            1 => {
                let rel = repos.into_iter().next().unwrap_or_default();
                Ok(git::team_worktree(&host, &ws, &rel, team).await.map(|p| p.to_string_lossy().into_owned()))
            }
            _ => Err(format!(
                "a shared plan works in one worktree, so its threads need one repo; these name {}. \
                 Pick one repo, or make the plan split.",
                repos.iter().map(|r| if r.is_empty() { "the workspace".to_string() } else { format!("'{r}'") }).collect::<Vec<_>>().join(", ")
            )),
        }
    }

    /// The shared plan `thread` works under, as its scripts take it.
    pub fn team_for(&self, sandbox: &SandboxRef, thread: &ThreadName) -> Option<Team> {
        let plans = self.plans.lock().unwrap();
        let plan = plans.team_of(&sandbox.to_string(), thread)?;
        Some(Team { plan: plan.id.clone(), worktree: ThreadName::parse(&plan.team_name()).ok()? })
    }

    pub async fn plan_propose(
        &self,
        sandbox: &SandboxRef,
        summary: &str,
        threads: Vec<PlanThread>,
        variant: Variant,
        justification: &str,
        lead: Option<&str>,
    ) -> Result<Plan, String> {
        self.check_plan_repos(sandbox, &threads).await?;
        if variant == Variant::Shared {
            self.shared_worktree(sandbox, &threads, "team").await?;
        }
        let plan = self.plans.lock().unwrap().propose(
            sbxw_core::ids::plan_id(),
            &sandbox.to_string(),
            summary,
            threads,
            variant,
            justification,
            lead,
            now_ms(),
        )?;
        tracing::info!("plans: '{sandbox}' proposed {} ({} thread(s), {:?})", plan.id, plan.threads.len(), plan.variant);
        let n = self.main_uncommitted(sandbox, &plan.threads).await;
        let plan = self.plans.lock().unwrap().note_main_uncommitted(&plan.id, n).unwrap_or(plan);
        Ok(self.plan_changed(plan))
    }

    /// Park until the plan is settled or the bound passes.
    pub async fn plan_wait(&self, id: &str, sandbox: &SandboxRef, bound: Duration) -> Result<Plan, String> {
        let mut rx = self.bus.subscribe();
        let mut latest = self.plans.lock().unwrap().owned(id, &sandbox.to_string())?.clone();
        let deadline = tokio::time::Instant::now() + bound;
        while latest.state == PlanState::Pending {
            let left = deadline.saturating_duration_since(tokio::time::Instant::now());
            if left.is_zero() {
                break;
            }
            match tokio::time::timeout(left, rx.recv()).await {
                Ok(Ok(Event::Plan(p))) if p.id == id => latest = p,
                Ok(Ok(_)) => {}
                Ok(Err(RecvError::Lagged(_))) => latest = self.plans.lock().unwrap().owned(id, &sandbox.to_string())?.clone(),
                Ok(Err(RecvError::Closed)) | Err(_) => break,
            }
        }
        Ok(latest)
    }

    pub fn plan_led(&self, id: &str, sandbox: &SandboxRef) -> Result<Plan, String> {
        self.plans.lock().unwrap().led(id, &sandbox.to_string()).cloned()
    }

    pub fn plan_propose_merge(&self, id: &str, sandbox: &SandboxRef, order: Vec<String>, note: &str, distillate: Vec<DistillItem>) -> Result<Plan, String> {
        let order: Vec<String> = order.iter().map(|t| t.trim().to_string()).collect();
        let note = messages::clip_text(note, sbxw_core::plans::MAX_TEXT);
        let plan = self.plans.lock().unwrap().propose_merge(id, &sandbox.to_string(), order, &note, distillate, now_ms())?;
        tracing::info!("plans: '{sandbox}' proposed merging {id}");
        Ok(self.plan_changed(plan))
    }

    /// The lead's `propose_merge`: [`App::plan_propose_merge`], refused for
    /// a thread git shows is in its base already ([`App::merged_outside`]):
    /// sbxw would merge nothing, and says so rather than wait for it.
    pub async fn plan_propose_merge_checked(&self, id: &str, sandbox: &SandboxRef, order: Vec<String>, note: &str, distillate: Vec<DistillItem>) -> Result<Plan, String> {
        let plan = self.plan_led(id, sandbox)?;
        let outside = self.merged_outside(&plan).await;
        let named: Vec<String> = match plan.variant {
            Variant::Shared => plan.threads.iter().map(|t| t.name.clone()).collect(),
            Variant::Split => order.iter().map(|t| t.trim().to_string()).collect(),
        };
        if let Some((t, base)) = named.iter().find_map(|t| outside.get(t).map(|b| (t, b))) {
            let what = if plan.variant.is_split() { format!("sbxw/{t}") } else { format!("sbxw/{} (thread {t:?})", plan.team_name()) };
            return Err(format!(
                "{what} is already in {base}: merged outside sbxw, or it has no commit of its own, \
                 so there is nothing to merge. Leave it out of the order; the user can close the thread"
            ));
        }
        self.plan_propose_merge(id, sandbox, order, note, distillate)
    }

    /// The lead marks a thread of its plan done ([`PlanBook::mark_done`]).
    pub fn plan_mark_done(&self, id: &str, sandbox: &SandboxRef, thread: &str, note: &str) -> Result<Plan, String> {
        let note = messages::clip_text(note, sbxw_core::plans::MAX_TEXT);
        let plan = self.plans.lock().unwrap().mark_done(id, &sandbox.to_string(), thread.trim(), &note, now_ms())?;
        tracing::info!("plans: the lead of '{sandbox}' marked thread '{}' of {id} done", thread.trim());
        Ok(self.plan_changed(plan))
    }

    /// A thread reports. When the plan has no lead name, blocked and done are
    /// typed into the main session so the lead still hears. Returns the lead's
    /// address for the thread's own `SendMessage`.
    pub fn plan_report(self: &Arc<Self>, sandbox: &SandboxRef, thread: &ThreadName, state: ThreadState, summary: &str) -> Result<Option<String>, String> {
        let summary = messages::clip_text(summary, sbxw_core::plans::MAX_TEXT);
        let plan = self.plans.lock().unwrap().set_thread(&sandbox.to_string(), thread, state, &summary, false, now_ms())?;
        let plan = self.plan_changed(plan);
        if plan.closed {
            return Ok(None);
        }
        if plan.lead.is_none() && self.sessions.is_live(&SessionKey::agent(sandbox)) {
            if let Some(text) = messages::report_to_lead(thread, state, &summary) {
                self.spawn_push(sandbox.clone(), SessionKind::Agent, text, None);
            }
        }
        Ok(plan.lead)
    }

    /// A person approves (maybe after editing, maybe changing the variant):
    /// the daemon records each thread's repo and model, then opens one session
    /// per thread with its briefing as start prompt, in its own worktree or all
    /// in the plan's one worktree when shared. Refused before anything is
    /// recorded when the threads would not fit under the ceiling, or a shared
    /// plan's threads name more than one repo.
    pub async fn plan_approve(self: &Arc<Self>, id: &str, threads: Vec<PlanThread>, variant: Option<Variant>) -> Result<Plan, String> {
        let plan = self.plan(id).ok_or_else(|| format!("no plan '{id}'"))?;
        let sandbox = SandboxRef::parse(&plan.sandbox).map_err(|e| e.to_string())?;
        let cfg = self.config_for(&sandbox).await;
        let open = self.sessions.agents_open(&sandbox.to_string());
        let new = threads
            .iter()
            .filter(|t| {
                ThreadName::parse(&t.name)
                    .map(|n| !self.sessions.is_live(&SessionKey::thread(&sandbox, &n)))
                    .unwrap_or(true)
            })
            .count();
        let max = cfg.max_sessions();
        if open + new > max {
            return Err(format!(
                "{new} new thread(s) and {open} session(s) already open is more than \
                 max_agent_sessions = {max}. Remove threads, or raise the ceiling in sbxw.toml."
            ));
        }
        self.check_plan_repos(&sandbox, &threads).await?;
        let worktree = match variant.unwrap_or(plan.variant) {
            Variant::Split => None,
            Variant::Shared => self.shared_worktree(&sandbox, &threads, &plan.team_name()).await?,
        };
        let plan = self.plans.lock().unwrap().approve(id, threads, variant, worktree, now_ms())?;
        let plan = self.plan_changed(plan);
        tracing::info!("plans: {id} approved for '{sandbox}' ({} thread(s), {:?})", plan.threads.len(), plan.variant);
        for t in &plan.threads {
            let Ok(name) = ThreadName::parse(&t.name) else { continue };
            if let Err(e) = self.settle_thread_repo(&sandbox, &name, t.repo.as_deref()).await {
                tracing::warn!("plans: thread '{name}' of '{sandbox}': {e}");
            } else if let Some(m) = t.model.as_deref().and_then(|m| ModelId::parse(m).ok()) {
                if let Err(e) = self.settle_thread_model(&sandbox, &name, &m, true) {
                    tracing::warn!("plans: {e}");
                }
            }
            self.briefings
                .lock()
                .unwrap()
                .insert(((&sandbox).into(), name.to_string()), messages::thread_briefing(&plan, t));
        }
        for t in &plan.threads {
            let (app, sandbox, name) = (self.clone(), sandbox.clone(), t.name.clone());
            tokio::spawn(async move { app.launch_thread(&sandbox, &name).await });
        }
        Ok(plan)
    }

    async fn launch_thread(self: &Arc<Self>, sandbox: &SandboxRef, name: &str) {
        let Ok(thread) = ThreadName::parse(name) else { return };
        let key = SessionKey::thread(sandbox, &thread);
        let outcome = match self.open_session(&key).await {
            Err(e) => Err(format!("{e:#}")),
            Ok((session, _)) => {
                // Still here: the session was already running and never read its
                // briefing as a start prompt — type it in instead.
                let unread = self.briefings.lock().unwrap().remove(&(sandbox.into(), name.to_string()));
                match unread {
                    Some(b) => {
                        self.note_typed(&sandbox.to_string(), &b);
                        crate::typing::type_and_submit(&session, &b, false).await
                    }
                    None => Ok(()),
                }
            }
        };
        if let Err(e) = outcome {
            self.thread_start_failed(sandbox, &thread, &e);
        }
    }

    /// Thread `thread` could not start: recorded on its plan, if it has one,
    /// with the reason (see `PlanBook::start_failed`).
    pub fn thread_start_failed(&self, sandbox: &SandboxRef, thread: &ThreadName, reason: &str) {
        let changed = self.plans.lock().unwrap().start_failed(&sandbox.to_string(), thread.as_str(), reason, now_ms());
        if let Some(p) = changed {
            tracing::warn!("plans: thread '{thread}' of '{sandbox}' did not start: {reason}");
            self.plan_changed(p);
        }
    }

    /// Thread `thread` started: a start failure recorded earlier is cleared.
    pub fn thread_start_cleared(&self, sandbox: &SandboxRef, thread: &ThreadName) {
        let changed = self.plans.lock().unwrap().start_cleared(&sandbox.to_string(), thread.as_str(), now_ms());
        if let Some(p) = changed {
            self.plan_changed(p);
        }
    }

    pub fn plan_refuse(&self, id: &str, note: Option<String>) -> Result<Plan, String> {
        let plan = self.plans.lock().unwrap().refuse(id, note, now_ms())?;
        Ok(self.plan_changed(plan))
    }

    /// The person clicks Merge: each thread, in order, merged into the branch
    /// its repo's main checkout is on, inside the sandbox (started first),
    /// stopping at the first failure; the plan broadcast after each. With
    /// `close_threads`, once all merged, each is closed, branch and all. The
    /// lead hears how it went in one line. A shared plan has one branch,
    /// merged once ([`App::plan_merge_shared`]).
    pub async fn plan_merge(self: &Arc<Self>, id: &str, close_threads: bool, distillate: Option<Vec<DistillItem>>) -> Result<sbxw_core::plans::Merge, String> {
        self.plan_merge_pinned(id, close_threads, distillate, None).await
    }

    /// [`App::plan_merge`], and with `pins` (the phone's approved merge,
    /// review P2) each branch is merged only if it still points at the
    /// commit approved and its repo's main checkout at the base approved
    /// (then at the commit the previous merge in that repo made), and the
    /// commit id is what is merged; otherwise that thread's result is
    /// `changed: ...` and the merge stops there, as at a conflict. `None`
    /// is the desktop's and web UI's merge, unchanged.
    pub async fn plan_merge_pinned(self: &Arc<Self>, id: &str, close_threads: bool, distillate: Option<Vec<DistillItem>>, pins: Option<&MergePins>) -> Result<sbxw_core::plans::Merge, String> {
        let plan = self.plan(id).ok_or_else(|| format!("no plan '{id}'"))?;
        let merge = plan.merge.clone().ok_or_else(|| format!("plan {id} has no merge proposed"))?;
        let sandbox = SandboxRef::parse(&plan.sandbox).map_err(|e| e.to_string())?;
        if plan.variant == Variant::Shared {
            self.refuse_while_claimed(&plan)?;
            return self.plan_merge_shared(plan, merge, &sandbox, close_threads, distillate, pins).await;
        }
        // Per repo directory, the base the next pinned merge must find.
        let mut pinned_base: HashMap<String, String> = HashMap::new();
        // Where each thread merges, read first so the base is known from the
        // first broadcast. A retry skips the threads already merged.
        let done: std::collections::HashSet<&str> = merge.results.iter().filter(|r| r.ok).map(|r| r.thread.as_str()).collect();
        let done_before: Vec<String> = done.iter().map(|t| t.to_string()).collect();
        let mut dirs: Vec<(String, Result<String, String>)> = Vec::new();
        for t in merge.order.iter().filter(|t| !done.contains(t.as_str())) {
            let planned = plan.threads.iter().find(|p| &p.name == t).and_then(|p| p.repo.clone());
            let dir = match ThreadName::parse(t) {
                Ok(name) => {
                    let recorded = self.store.get(&sandbox).threads.contains_key(t.as_str());
                    self.settle_thread_repo(&sandbox, &name, if recorded { None } else { planned.as_deref() }).await
                }
                Err(e) => Err(e.to_string()),
            };
            dirs.push((t.clone(), dir));
        }
        let host = self.host_for(&sandbox).await.map_err(|e| format!("{e:#}"))?;
        let mut bases: std::collections::BTreeSet<Option<String>> = std::collections::BTreeSet::new();
        for (_, d) in &dirs {
            bases.insert(match d {
                Ok(dir) if !dir.is_empty() => git::branch_of(&host, std::path::Path::new(dir)).await,
                _ => None,
            });
        }
        let base = if bases.len() == 1 { bases.into_iter().next().flatten() } else { None };
        let p = self.plans.lock().unwrap().start_merge(id, base, distillate, now_ms())?;
        self.plan_changed(p);
        tracing::info!("plans: merging {id} for '{sandbox}' ({} thread(s))", merge.order.len());

        let running = self.ensure_running_for_thread(&sandbox).await.map_err(|e| format!("{e:#}"));
        for (thread, dir) in dirs {
            let outcome = match (&running, dir, ThreadName::parse(&thread)) {
                (Err(e), _, _) => Err(format!("the sandbox could not be started: {e}")),
                (Ok(()), Err(e), _) => Err(e),
                (Ok(()), Ok(_), Err(e)) => Err(e.to_string()),
                (Ok(()), Ok(dir), Ok(name)) => match pins {
                    None => self.sbx_of(&sandbox).await?.thread_merge(&sandbox.name, &dir, &name).await.map_err(|e| self.with_split_note(&sandbox, format!("{e:#}"))),
                    Some(pins) => match pins.get(&thread) {
                        None => Err(format!("changed: sbxw/{thread} was not in the approved merge")),
                        Some((tip, base)) => {
                            let want = pinned_base.get(&dir).cloned().unwrap_or_else(|| base.clone());
                            let m = self.sbx_of(&sandbox).await?.thread_merge_pinned(&sandbox.name, &dir, &name, tip, &want).await.map_err(|e| self.with_split_note(&sandbox, format!("{e:#}")));
                            match &m {
                                Ok(o) if !o.head.is_empty() => {
                                    pinned_base.insert(dir.clone(), o.head.clone());
                                }
                                // A script that says nothing of its new head: the next
                                // pinned merge in this repo cannot be checked, so it is
                                // refused rather than guessed.
                                Ok(_) => {
                                    pinned_base.insert(dir.clone(), String::from("unknown"));
                                }
                                Err(_) => {}
                            }
                            m
                        }
                    },
                },
            };
            let ok = outcome.is_ok();
            let result = match outcome {
                Ok(m) => MergeResult { thread: thread.clone(), ok: true, note: m.note(), commit: Some(m.commit).filter(|c| !c.is_empty()), error: None },
                Err(e) => MergeResult { thread: thread.clone(), ok: false, commit: None, error: Some(e), note: None },
            };
            let p = self.plans.lock().unwrap().merge_result(id, result, now_ms())?;
            self.plan_changed(p);
            if !ok {
                break;
            }
        }
        let plan = self.plans.lock().unwrap().finish_merge(id, now_ms())?;
        let plan = self.plan_changed(plan);
        let merge = plan.merge.clone().unwrap_or(merge);
        let mut summary = messages::merge_summary(&merge);
        // The other threads of the sandbox hear that their base moved.
        let all_merged: Vec<String> = merge.results.iter().filter(|r| r.ok).map(|r| r.thread.clone()).collect();
        let merged_now: Vec<String> = all_merged.iter().filter(|t| !done_before.contains(*t)).cloned().collect();
        let told = self.tell_main_moved(&sandbox, &merged_now, &all_merged, merge.base.as_deref());
        if !told.is_empty() {
            summary.push_str(&format!(" Told that {} moved: {}.", merge.base.as_deref().unwrap_or("main"), told.join(", ")));
        }
        if close_threads && merge.state == MergeState::Merged {
            let mut unclosed = Vec::new();
            for t in &merge.order {
                let Ok(name) = ThreadName::parse(t) else { continue };
                if let Err(e) = self.close_thread(&sandbox, &name, false, true).await {
                    tracing::warn!("plans: could not close merged thread '{t}': {e}");
                    unclosed.push(format!("{t} ({e})"));
                }
            }
            if !unclosed.is_empty() {
                summary.push_str(&format!(" Not closed: {}.", unclosed.join(", ")).replace('\n', " "));
            }
        }
        self.tell_lead_merged(&sandbox, id, &summary).await;
        Ok(merge)
    }

    /// A merge refused for files missing from the checkout, with the reason
    /// sbxw knows when it split the workspace itself.
    fn with_split_note(&self, sandbox: &SandboxRef, err: String) -> String {
        match self.split_note(sandbox) {
            Some(note) if err.contains("missing from the main checkout") => format!("{err} {note}"),
            _ => err,
        }
    }

    /// A shared plan's merge waits for its board: files still claimed are
    /// work still in flight. Expired leases do not count.
    fn refuse_while_claimed(&self, plan: &Plan) -> Result<(), String> {
        let now = now_ms();
        let held: Vec<(String, String)> = self.with_board(&plan.sandbox, Some(&plan.id), |b| {
            b.expire(now);
            b.claims.values().map(|c| (c.file.clone(), c.owner.clone())).collect()
        });
        if held.is_empty() {
            return Ok(());
        }
        let list: Vec<String> = held.iter().map(|(f, o)| format!("{f} ({o})")).collect();
        let n = list.len();
        Err(format!(
            "{n} file{} still claimed: {} - release them or force_release first",
            if n == 1 { "" } else { "s" },
            list.join(", ")
        ))
    }

    async fn tell_lead_merged(self: &Arc<Self>, sandbox: &SandboxRef, id: &str, summary: &str) {
        tracing::info!("plans: {id}: {summary}");
        if self.sessions.is_live(&SessionKey::agent(sandbox)) {
            if let Err(e) = self.push_text(sandbox, SessionKind::Agent, summary).await {
                tracing::warn!("plans: could not tell the lead of '{sandbox}' about the merge: {e}");
            }
        }
    }

    /// A shared plan's merge: one `git merge --no-ff sbxw/team-<plan>` into
    /// the branch its repo's main checkout is on, recorded as one result
    /// named after the worktree. Once merged, the approved distillate is
    /// written to the repo's Claude Code auto memory; with `close_threads`,
    /// each thread's session is closed, then the shared worktree and branch.
    async fn plan_merge_shared(
        self: &Arc<Self>,
        plan: Plan,
        merge: sbxw_core::plans::Merge,
        sandbox: &SandboxRef,
        close_threads: bool,
        distillate: Option<Vec<DistillItem>>,
        pins: Option<&MergePins>,
    ) -> Result<sbxw_core::plans::Merge, String> {
        let id = plan.id.as_str();
        let team = ThreadName::parse(&plan.team_name()).map_err(|e| e.to_string())?;
        // Every thread is in the one repo; the first one's settles it.
        let dir = match plan.threads.first() {
            Some(t) => match ThreadName::parse(&t.name) {
                Ok(name) => {
                    let recorded = self.store.get(sandbox).threads.contains_key(t.name.as_str());
                    self.settle_thread_repo(sandbox, &name, if recorded { None } else { t.repo.as_deref() }).await
                }
                Err(e) => Err(e.to_string()),
            },
            None => Err(format!("plan {id} has no thread")),
        };
        let base = match &dir {
            Ok(d) if !d.is_empty() => git::branch_of(&self.host_for(sandbox).await.map_err(|e| format!("{e:#}"))?, std::path::Path::new(d)).await,
            _ => None,
        };
        let p = self.plans.lock().unwrap().start_merge(id, base, distillate, now_ms())?;
        self.plan_changed(p);
        tracing::info!("plans: merging shared {id} for '{sandbox}' (branch sbxw/{team})");
        let outcome = match (self.ensure_running_for_thread(sandbox).await, dir) {
            (Err(e), _) => Err(format!("the sandbox could not be started: {e:#}")),
            (Ok(()), Err(e)) => Err(e),
            (Ok(()), Ok(dir)) => {
                let sbx = self.sbx_of(sandbox).await?;
                let m = match pins {
                    None => sbx.thread_merge(&sandbox.name, &dir, &team).await,
                    Some(pins) => match pins.get(team.as_str()) {
                        Some((tip, base)) => sbx.thread_merge_pinned(&sandbox.name, &dir, &team, tip, base).await,
                        None => Err(anyhow::anyhow!("changed: sbxw/{team} was not in the approved merge")),
                    },
                };
                m.map(|c| (c, dir)).map_err(|e| self.with_split_note(sandbox, format!("{e:#}")))
            }
        };
        let (result, dir) = match outcome {
            Ok((m, dir)) => (MergeResult { thread: team.to_string(), ok: true, note: m.note(), commit: Some(m.commit).filter(|c| !c.is_empty()), error: None }, Some(dir)),
            Err(e) => (MergeResult { thread: team.to_string(), ok: false, commit: None, error: Some(e), note: None }, None),
        };
        self.plans.lock().unwrap().merge_result(id, result, now_ms())?;
        let plan = self.plans.lock().unwrap().finish_merge(id, now_ms())?;
        let plan = self.plan_changed(plan);
        let merge = plan.merge.clone().unwrap_or(merge);
        let mut summary = messages::merge_summary(&merge);
        if let (Some(dir), MergeState::Merged) = (&dir, merge.state) {
            summary = format!("[sbxw] Merged sbxw/{team} (threads {}).", merge.order.join(", "));
            // Threads of the sandbox's other plans hear that their base moved.
            let mine: Vec<String> = plan.threads.iter().map(|t| t.name.clone()).chain(std::iter::once(team.to_string())).collect();
            let told = self.tell_main_moved(sandbox, &[team.to_string()], &mine, merge.base.as_deref());
            if !told.is_empty() {
                summary.push_str(&format!(" Told that {} moved: {}.", merge.base.as_deref().unwrap_or("main"), told.join(", ")));
            }
            if let Some((file, body, index)) = plan.distillate_memory() {
                match self.write_memory(sandbox, dir, &file, &body, &index).await {
                    Ok(path) => summary.push_str(&format!(" Distillate saved to auto memory: {path}.")),
                    Err(e) => {
                        tracing::warn!("plans: could not save the distillate of {id}: {e}");
                        summary.push_str(&format!(" The distillate was not saved ({e}).").replace('\n', " "));
                    }
                }
            }
            if close_threads {
                let mut unclosed = Vec::new();
                for t in &merge.order {
                    let Ok(name) = ThreadName::parse(t) else { continue };
                    if let Err(e) = self.close_thread(sandbox, &name, false, true).await {
                        unclosed.push(format!("{t} ({e})"));
                    }
                }
                if let Err(e) = self.sbx_of(sandbox).await?.thread_close(&sandbox.name, dir, &team, false, true).await {
                    unclosed.push(format!("its worktree ({e:#})"));
                }
                if !unclosed.is_empty() {
                    tracing::warn!("plans: after merging {id}, not closed: {}", unclosed.join(", "));
                    summary.push_str(&format!(" Not closed: {}.", unclosed.join(", ")).replace('\n', " "));
                }
            }
        }
        self.tell_lead_merged(sandbox, id, &summary).await;
        Ok(merge)
    }

    /// One memory into the auto memory of the repo under `dir`, inside the
    /// sandbox; where it went.
    async fn write_memory(&self, sandbox: &SandboxRef, dir: &str, file: &str, body: &str, index: &str) -> Result<String, String> {
        let args = sbxw_sbx::scripts::memory_write_args(dir, file, index);
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        let out = self.sbx_of(sandbox).await?.exec(&sandbox.name, &args, Some(body.as_bytes().to_vec())).await.map_err(|e| format!("{e:#}"))?;
        let fields = sbxw_sbx::scripts::script_fields(&out.stdout_str())?;
        if !out.success() {
            return Err(format!("the memory script failed: {}", String::from_utf8_lossy(&out.stderr).trim()));
        }
        Ok(fields.get("path").cloned().unwrap_or_default())
    }

    /// Close a thread: stop what runs in its worktree, remove the worktree,
    /// the branch if asked. Dirty work is refused without `discard` — the
    /// daemon does not trust a click it did not see.
    pub async fn close_thread(&self, sandbox: &SandboxRef, thread: &ThreadName, discard: bool, delete_branch: bool) -> Result<(), String> {
        // A session in the main checkout has no worktree to remove (its folder
        // is the main agent's, so nothing there is killed by cwd). A shared
        // thread has no worktree of its own: it leaves its plan, and the plan's
        // worktree goes with the last one.
        let mut team_end = None;
        if self.is_same_worktree(sandbox, thread) {
            tracing::info!("closing session '{thread}' of '{sandbox}', in the main checkout");
        } else if let Some(team) = self.team_for(sandbox, thread) {
            let dir = self.settle_thread_repo(sandbox, thread, None).await.ok();
            let left = self.plans.lock().unwrap().drop_team_thread(&sandbox.to_string(), thread.as_str(), now_ms());
            if let Some(plan) = left {
                tracing::info!("closing '{thread}' of '{sandbox}': it leaves shared plan {}", plan.id);
                if plan.state == PlanState::Refused {
                    team_end = dir.map(|d| (d, team.worktree));
                }
                self.plan_changed(plan);
            }
        } else {
            match self.settle_thread_repo(sandbox, thread, None).await {
                Ok(dir) => self
                    .sbx_of(sandbox)
                    .await?
                    .thread_close(&sandbox.name, &dir, thread, discard, delete_branch)
                    .await
                    .map_err(|e| format!("{e:#}"))?,
                // A chat's workspace is throwaway and may have been swept (or
                // never made a repo): no repo, no worktree, nothing to remove —
                // only the session and its record to forget.
                Err(e) if self.store.chat_workspace(sandbox).is_some() => {
                    tracing::info!("closing chat session '{thread}' of '{sandbox}' with no worktree to remove ({e})");
                }
                Err(e) => return Err(e),
            }
        }
        self.sessions.remove(&SessionKey::thread(sandbox, thread));
        self.sessions.remove(&SessionKey::new(sandbox.clone(), SessionKind::ThreadBash(thread.clone())));
        if let Some((dir, team)) = team_end {
            self.end_team_worktree(sandbox, &dir, &team).await;
        }
        let gone = self.agents.lock().unwrap().forget_thread(&sandbox.to_string(), thread.as_str(), now_ms());
        for info in gone {
            self.send(Event::Session(info));
        }
        if let Err(e) = self.store.update(sandbox, |r| {
            r.threads.remove(thread.as_str());
        }) {
            tracing::warn!("threads: could not forget the repo of '{thread}': {e:#}");
        }
        // Last, once the thread is gone for the daemon: every client drops its
        // panes and its row. Every way a thread ends comes through here.
        self.send(Event::ThreadClosed(ThreadClosed { sandbox: sandbox.to_string(), thread: thread.to_string() }));
        Ok(())
    }

    /// The shared worktree of a plan whose every thread was closed: removed
    /// with its branch when it holds nothing — no uncommitted file, no commit
    /// its base lacks. Anything in it is work, and is left for a person.
    async fn end_team_worktree(&self, sandbox: &SandboxRef, dir: &str, team: &ThreadName) {
        let sbx = match self.sbx_of(sandbox).await {
            Ok(s) => s,
            Err(e) => return tracing::warn!("plans: could not read the worktree {team} of '{sandbox}': {e}"),
        };
        match sbx.thread_status(&sandbox.name, dir, team).await {
            Ok(st) if st.dirty == 0 && st.ahead == 0 => {
                if let Err(e) = sbx.thread_close(&sandbox.name, dir, team, false, true).await {
                    tracing::warn!("plans: could not remove the empty worktree {team} of '{sandbox}': {e:#}");
                }
            }
            Ok(st) => tracing::info!(
                "plans: keeping {} of '{sandbox}': {} uncommitted file(s), {} commit(s) not in {}",
                st.worktree, st.dirty, st.ahead, st.base
            ),
            Err(e) => tracing::warn!("plans: could not read the worktree {team} of '{sandbox}': {e:#}"),
        }
    }

    /// ★ Lead: install `/lead` and refresh the relay server (a sandbox from
    /// before plans lacks its tools), type `/lead`, remember *which* session.
    pub async fn lead(self: &Arc<Self>, sandbox: &SandboxRef) -> Result<(), String> {
        let token = self.store.token(sandbox).map_err(|e| format!("{e:#}"))?;
        let batch = Tooling::new()
            .agent_token(&token)
            .relay(&self.agent_port_for(sandbox).await.map_err(|e| format!("{e:#}"))?)
            .lead_command();
        let outcome = self.sbx_of(sandbox).await?.tooling(&sandbox.name, &batch).await.map_err(|e| format!("{e:#}"))?;
        if let Some((piece, e)) = outcome.failed.first() {
            return Err(format!("{piece}: {e}"));
        }
        self.push_text(sandbox, SessionKind::Agent, messages::LEAD_COMMAND).await?;
        if let Some(s) = self.sessions.live(&SessionKey::agent(sandbox)) {
            let now = now_ms();
            // The conversation the person turned the team on in. Unknown when
            // the agent has not reported yet: filled by the first `leading()`
            // while this very terminal is still the main one.
            let session_id = self.main_session_id(sandbox);
            let rec = LeadRecord { since_ms: now, session_id, bound_ms: now };
            self.leads.lock().unwrap().insert(sandbox.clone(), Arc::as_ptr(&s) as usize);
            self.lead_recs.lock().unwrap().insert(sandbox.clone(), rec.clone());
            self.save_lead(sandbox, Some(rec));
            self.send(Event::Lead(sandbox.into()));
            // Plans its last end closed are this sandbox's lead's again,
            // told once, after `/lead`, which plans and where they stand.
            let back = self.plans.lock().unwrap().resume_lead(&sandbox.to_string(), now);
            if !back.is_empty() {
                tracing::info!("plans: '{sandbox}' leads again, {} open plan(s) given back", back.len());
                let text = messages::lead_resumed(&back);
                for p in back {
                    self.plan_changed(p);
                }
                if let Some(text) = text {
                    if let Err(e) = self.push_text(sandbox, SessionKind::Agent, &text).await {
                        tracing::warn!("plans: could not tell '{sandbox}' which plans it follows again: {e}");
                    }
                }
            }
        }
        Ok(())
    }

    /// The gate of the lead's plan routes (propose, wait, status,
    /// propose_merge, mark_done, the lead's read_team_notes): open only while
    /// `sandbox` leads by [`App::leading`]'s rule. The daemon cannot tell
    /// which session of the sandbox posts: within a leading sandbox, the plan
    /// id stays the capability. `id` names the plan in the refusal.
    pub fn require_leading(&self, sandbox: &SandboxRef, id: Option<&str>) -> Result<(), String> {
        let me = sandbox.to_string();
        if self.leading().contains(&me) {
            return Ok(());
        }
        // Another sandbox's id reads as not found, as before the gate.
        if let Some(id) = id {
            self.plans.lock().unwrap().owned(id, &me)?;
        }
        Err(sbxw_core::plans::lead_ended(id))
    }

    /// Done threads of `plan` whose branch git shows is already in the branch
    /// its repo's main checkout is on, though sbxw never merged it: merged by
    /// hand, or with no commit of its own. Thread name to that branch. A
    /// shared plan's one branch counts for all its threads.
    pub async fn merged_outside(&self, plan: &Plan) -> BTreeMap<String, String> {
        let mut out = BTreeMap::new();
        if plan.state != PlanState::Approved || plan.merge.as_ref().is_some_and(|m| m.state == MergeState::Merged) {
            return out;
        }
        let Ok(sandbox) = SandboxRef::parse(&plan.sandbox) else { return out };
        let Some(ws) = self.store.workspace(&sandbox) else { return out };
        let Ok(host) = self.host_for(&sandbox).await else { return out };
        let recorded = self.store.get(&sandbox).threads;
        let by_sbxw: Vec<&str> = plan.merge.iter().flat_map(|m| m.results.iter().filter(|r| r.ok).map(|r| r.thread.as_str())).collect();
        let repo_of = |t: &PlanThread| recorded.get(t.name.as_str()).map(|r| r.repo.clone()).or_else(|| t.repo.clone()).unwrap_or_default();
        let in_base = |rel: String, branch: String| {
            let (host, ws) = (&host, &ws);
            async move {
                let root = git::repo_root(host, ws, &rel).await?;
                let full = format!("refs/heads/{branch}");
                let tip = git::git(host, &root, &["rev-parse", "--verify", "--quiet", &full]).await?;
                git::git(host, &root, &["merge-base", "--is-ancestor", tip.trim(), "HEAD"]).await?;
                Some(git::branch_of(host, &root).await.unwrap_or_else(|| "HEAD".into()))
            }
        };
        let done = |t: &&PlanThread| t.state == ThreadState::Done && !by_sbxw.contains(&t.name.as_str());
        match plan.variant {
            Variant::Shared => {
                let Some(first) = plan.threads.first() else { return out };
                if let Some(base) = in_base(repo_of(first), format!("sbxw/{}", plan.team_name())).await {
                    for t in plan.threads.iter().filter(done) {
                        out.insert(t.name.clone(), base.clone());
                    }
                }
            }
            Variant::Split => {
                for t in plan.threads.iter().filter(done) {
                    if let Some(base) = in_base(repo_of(t), format!("sbxw/{}", t.name)).await {
                        out.insert(t.name.clone(), base);
                    }
                }
            }
        }
        out
    }

    /// [`App::plan_view`], with `merged_outside: <branch>` on each thread
    /// git shows is already in its base ([`App::merged_outside`]).
    pub async fn plan_view_checked(&self, plan: &Plan) -> serde_json::Value {
        let outside = self.merged_outside(plan).await;
        let mut v = self.plan_view(plan);
        if let Some(threads) = v.get_mut("threads").and_then(|t| t.as_array_mut()) {
            for t in threads.iter_mut() {
                if let Some(base) = t["name"].as_str().and_then(|n| outside.get(n)) {
                    t["merged_outside"] = base.clone().into();
                }
            }
        }
        v
    }

    pub async fn lead_end(self: &Arc<Self>, sandbox: &SandboxRef) {
        self.forget_lead(sandbox);
        let ended = self.plans.lock().unwrap().end_lead(&sandbox.to_string(), now_ms());
        tracing::info!("plans: lead mode ended for '{sandbox}' ({} plan(s) refused or closed)", ended.len());
        for p in ended {
            self.plan_changed(p);
        }
        if self.sessions.is_live(&SessionKey::agent(sandbox)) {
            if let Err(e) = self.push_text(sandbox, SessionKind::Agent, messages::LEAD_END).await {
                tracing::warn!("plans: could not tell '{sandbox}' that lead mode ended: {e}");
            }
        }
        self.send(Event::LeadEnd(sandbox.into()));
    }

    /// The lead of `sandbox` is over without a word to its agent: it was
    /// stopped or removed. Bookkeeping only; nothing is typed.
    pub fn lead_gone(&self, sandbox: &SandboxRef) {
        let had = self.leads.lock().unwrap().contains_key(sandbox) || self.lead_recs.lock().unwrap().contains_key(sandbox);
        self.forget_lead(sandbox);
        if had {
            tracing::info!("plans: lead mode of '{sandbox}' dropped, the sandbox stopped or went away");
            self.send(Event::LeadEnd(sandbox.into()));
        }
    }

    fn forget_lead(&self, sandbox: &SandboxRef) {
        self.leads.lock().unwrap().remove(sandbox);
        self.lead_recs.lock().unwrap().remove(sandbox);
        self.save_lead(sandbox, None);
    }

    /// The lead on disk, written only to a sandbox that still has a record
    /// (writing would bring a removed one back).
    fn save_lead(&self, sandbox: &SandboxRef, rec: Option<LeadRecord>) {
        if !self.store.exists(sandbox) || (rec.is_none() && self.store.get(sandbox).lead.is_none()) {
            return;
        }
        if let Err(e) = self.store.update(sandbox, |r| r.lead = rec) {
            tracing::warn!("plans: could not save the lead of '{sandbox}': {e:#}");
        }
    }

    /// The id of the live session the phone shows for `sandbox`.
    fn main_session_id(&self, sandbox: &SandboxRef) -> Option<String> {
        main_session(self, &self.session_infos(), sandbox)
    }

    /// After a hook post or a new main terminal: a lead waiting to be found
    /// again (after a daemon restart) is looked at now, so its `Lead` event
    /// goes out without a client asking.
    pub(crate) fn rebind_lead(&self, sandbox: &SandboxRef) {
        let bound = self.leads.lock().unwrap().contains_key(sandbox);
        let waiting = self.lead_recs.lock().unwrap().get(sandbox).is_some_and(|r| !bound || r.session_id.is_none());
        if waiting {
            self.leading();
        }
    }

    /// Sandboxes whose main session is still the very session made lead.
    ///
    /// While this daemon holds the terminal it was made lead in, that
    /// terminal decides, as before. A lead with no terminal bound (the daemon
    /// restarted, or the terminal closed) is bound again, with a `Lead` event
    /// and nothing typed, only when the main terminal is live and the
    /// session it shows is the recorded one; it is dropped when that terminal
    /// shows another live session (a new conversation is not a leader), when
    /// no session was ever recorded, or after [`LEAD_UNBOUND_MAX_MS`] unbound.
    pub fn leading(&self) -> Vec<String> {
        let now = now_ms();
        let mut saves: Vec<(SandboxRef, Option<LeadRecord>)> = Vec::new();
        let mut rebound: Vec<SandboxRef> = Vec::new();
        let out = {
            let mut leads = self.leads.lock().unwrap();
            let mut recs = self.lead_recs.lock().unwrap();
            leads.retain(|sb, ptr| {
                self.sessions
                    .live(&SessionKey::agent(sb))
                    .is_some_and(|s| Arc::as_ptr(&s) as usize == *ptr)
            });
            let pending = recs.iter().any(|(sb, r)| !leads.contains_key(sb) || r.session_id.is_none());
            let infos = if pending { self.session_infos() } else { Vec::new() };
            // A bound lead whose conversation was not reported at Team on:
            // the one its own terminal shows now.
            for (sb, r) in recs.iter_mut().filter(|(sb, r)| leads.contains_key(*sb) && r.session_id.is_none()) {
                if let Some(sid) = main_session(self, &infos, sb) {
                    r.session_id = Some(sid);
                    saves.push((sb.clone(), Some(r.clone())));
                }
            }
            recs.retain(|sb, r| {
                if leads.contains_key(sb) {
                    return true;
                }
                let Some(want) = r.session_id.clone() else {
                    // Unbound, and nothing ever said which conversation.
                    saves.push((sb.clone(), None));
                    return false;
                };
                let fresh = now.saturating_sub(r.bound_ms) <= LEAD_UNBOUND_MAX_MS;
                let here = self.sessions.live(&SessionKey::agent(sb));
                match (here, main_session(self, &infos, sb)) {
                    (Some(s), Some(sid)) if sid == want => {
                        leads.insert(sb.clone(), Arc::as_ptr(&s) as usize);
                        r.bound_ms = now;
                        saves.push((sb.clone(), Some(r.clone())));
                        rebound.push(sb.clone());
                        true
                    }
                    // Another conversation shows now: not a leader, but the
                    // record waits (as an unbound one does) until
                    // LEAD_UNBOUND_MAX_MS, so a wrong pick is never permanent.
                    // Debug only: `leading()` runs often.
                    (Some(_), Some(_)) if fresh => {
                        tracing::debug!("plans: '{sb}' runs another conversation than the one made lead: waiting for that one");
                        true
                    }
                    (Some(_), Some(_)) => {
                        tracing::info!("plans: '{sb}' runs another conversation than the one made lead: lead mode dropped");
                        saves.push((sb.clone(), None));
                        false
                    }
                    _ if fresh => true,
                    _ => {
                        tracing::info!("plans: the lead of '{sb}' was not found again in time: dropped");
                        saves.push((sb.clone(), None));
                        false
                    }
                }
            });
            let mut out: Vec<String> = leads.keys().map(|r| r.to_string()).collect();
            out.sort();
            out
        };
        for (sb, rec) in saves {
            self.save_lead(&sb, rec);
        }
        for sb in rebound {
            tracing::info!("plans: '{sb}' leads again, its main session is the one made lead");
            self.send(Event::Lead(sb));
        }
        out
    }
}

/// The id of a sandbox's main session, when it is live: the rule of the
/// phone's reach (`sbxw_core::remote_access::reach::main_index`): the session
/// sbxw's terminal types into when the daemon knows it, never a thread's or
/// a harness background session; else the newest such one.
fn main_session(app: &App, infos: &[SessionInfo], sandbox: &SandboxRef) -> Option<String> {
    let cands = app.reach_candidates(infos);
    sbxw_core::remote_access::reach::main_index(&cands, &sandbox.to_string())
        .filter(|&i| !cands[i].exited)
        .and_then(|i| infos[i].session_id.clone())
}

/// The leads on disk, unbound: each waits for its own session.
pub(crate) fn load_leads(store: &Store) -> HashMap<SandboxRef, LeadRecord> {
    store.names().into_iter().filter_map(|sb| store.get(&sb).lead.map(|rec| (sb, rec))).collect()
}

#[cfg(test)]
#[path = "plans_lead_tests.rs"]
mod lead_tests;

#[cfg(test)]
#[path = "plans_reopen_tests.rs"]
mod reopen_tests;

#[cfg(test)]
mod tests {
    use crate::bus::{Event, ThreadClosed};
    use crate::testing::{app, app_with, ls, sbx_answer};
    use sbxw_core::plans::{DistillItem, DistillKind, MergeState, PlanState, PlanThread, ThreadState, Variant};
    use sbxw_sbx::runner::Output;
    use sbxw_core::SandboxRef;
    use std::time::Duration;

    #[tokio::test]
    async fn a_chat_session_without_a_repo_still_closes() {
        let (_d, app) = app();
        let name = SandboxRef::parse("ephemeral-chat").unwrap();
        let ws = app.store.paths.chat_workspace(&name.name);
        std::fs::create_dir_all(&ws).unwrap();
        let thread = sbxw_core::ThreadName::parse("chat-2").unwrap();
        app.store
            .update(&name, |r| {
                r.workspace = Some(ws.clone());
                r.threads.entry("chat-2".into()).or_default();
            })
            .unwrap();
        let mut rx = app.bus.subscribe();
        app.close_thread(&name, &thread, true, true).await.unwrap();
        assert!(!app.store.get(&name).threads.contains_key("chat-2"), "the record is forgotten");
        let mut closed = vec![];
        while let Ok(ev) = rx.try_recv() {
            if let Event::ThreadClosed(c) = ev {
                closed.push(c);
            }
        }
        assert_eq!(closed, vec![ThreadClosed { sandbox: "ephemeral-chat".into(), thread: "chat-2".into() }], "one event, names only");
        assert_eq!(Event::ThreadClosed(closed[0].clone()).name(), "thread-closed");
        assert_eq!(Event::ThreadClosed(closed[0].clone()).data(), r#"{"sandbox":"ephemeral-chat","thread":"chat-2"}"#);

        let other = SandboxRef::parse("myapp").unwrap();
        let dir = tempfile::tempdir().unwrap();
        app.store.update(&other, |r| r.workspace = Some(dir.path().to_path_buf())).unwrap();
        assert!(app.close_thread(&other, &thread, true, true).await.is_err(), "anywhere else it still asks for a repo");
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

    #[tokio::test]
    async fn a_proposal_is_persisted_and_survives_a_restart() {
        let (dir, app) = app();
        let sb = SandboxRef::parse("myapp").unwrap();
        let p = app.plan_propose(&sb, "split", vec![t("api")], Variant::Split, "independent", Some("lead")).await.unwrap();
        let file = app.store.paths.plans().join("myapp.json");
        assert!(file.exists());
        let again = crate::testing::app_in(dir.path());
        assert_eq!(again.plan(&p.id).unwrap().state, PlanState::Pending);
        // Another sandbox cannot wait on it.
        assert!(app.plan_wait(&p.id, &SandboxRef::parse("other").unwrap(), Duration::from_millis(10)).await.is_err());
    }

    #[tokio::test]
    async fn the_lead_can_mark_a_silent_thread_done_and_then_propose_the_merge() {
        let (_d, app) = app();
        let sb = SandboxRef::parse("myapp").unwrap();
        let p = app.plan_propose(&sb, "", vec![t("api")], Variant::Split, "one", Some("lead")).await.unwrap();
        app.plans.lock().unwrap().approve(&p.id, vec![t("api")], None, None, 1).unwrap();
        assert!(app.plan_propose_merge(&p.id, &sb, vec!["api".into()], "", vec![]).unwrap_err().contains("not done"));
        assert!(app.plan_mark_done(&p.id, &SandboxRef::parse("other").unwrap(), "api", "x").is_err(), "another sandbox's lead cannot");
        let plan = app.plan_mark_done(&p.id, &sb, "api", "reviewed sbxw/api").unwrap();
        let view = app.plan_view(&plan);
        assert_eq!((view["threads"][0]["state"].as_str(), view["threads"][0]["set_by"].as_str()), (Some("done"), Some("lead")));
        app.plan_propose_merge(&p.id, &sb, vec!["api".into()], "", vec![]).unwrap();
        let again = crate::testing::app_in(_d.path());
        assert_eq!(again.plan(&p.id).unwrap().threads[0].set_by, Some(sbxw_core::plans::SetBy::Lead), "persisted");
    }

    #[tokio::test]
    async fn a_proposal_counts_the_uncommitted_work_the_worktrees_will_not_have() {
        let (_d, app) = app();
        let sb = SandboxRef::parse("myapp").unwrap();
        let ws = tempfile::tempdir().unwrap();
        let git = |args: &[&str]| {
            let ok = std::process::Command::new("git")
                .arg("-C")
                .arg(ws.path())
                .args(["-c", "user.email=t@t", "-c", "user.name=t", "-c", "commit.gpgsign=false"])
                .args(args)
                .output()
                .is_ok_and(|o| o.status.success());
            assert!(ok, "git {args:?}");
        };
        if std::process::Command::new("git").arg("--version").output().is_err() {
            return;
        }
        git(&["init", "-q"]);
        std::fs::write(ws.path().join("a"), "a").unwrap();
        git(&["add", "a"]);
        git(&["commit", "-q", "-m", "one"]);
        app.store.update(&sb, |r| r.workspace = Some(ws.path().to_path_buf())).unwrap();

        let p = app.plan_propose(&sb, "", vec![t("api")], Variant::Split, "one", None).await.unwrap();
        assert_eq!(p.main_uncommitted, Some(0));
        app.plan_refuse(&p.id, None).unwrap();

        std::fs::write(ws.path().join("a"), "edited").unwrap();
        std::fs::write(ws.path().join("new"), "untracked").unwrap();
        let p = app.plan_propose(&sb, "", vec![t("api")], Variant::Split, "one", None).await.unwrap();
        assert_eq!(p.main_uncommitted, Some(2));
        assert_eq!(serde_json::to_value(app.plan(&p.id).unwrap()).unwrap()["main_uncommitted"], 2, "on the plan both UIs receive");
    }

    #[tokio::test]
    async fn a_refusal_wakes_the_lead() {
        let (_d, app) = app();
        let sb = SandboxRef::parse("myapp").unwrap();
        let p = app.plan_propose(&sb, "", vec![t("api")], Variant::Split, "one thread", None).await.unwrap();
        let a = app.clone();
        let (id, s) = (p.id.clone(), sb.clone());
        let w = tokio::spawn(async move { a.plan_wait(&id, &s, Duration::from_secs(5)).await });
        tokio::time::sleep(Duration::from_millis(30)).await;
        app.plan_refuse(&p.id, Some("no".into())).unwrap();
        assert_eq!(w.await.unwrap().unwrap().state, PlanState::Refused);
    }

    #[tokio::test]
    async fn a_shared_plan_works_in_one_worktree_and_merges_one_branch() {
        let (_d, app, fake) = app_with(|a| {
            let has = |x: &str| a.iter().any(|y| y == x);
            match a.first().map(String::as_str) {
                Some("ls") => ls(&[("myapp", "running")]),
                Some("exec") if has("sbxw-thread-merge") => Output::ok("base=main\ncommit=abc1234\nok=1\n"),
                Some("exec") if has("sbxw-memory") => Output::ok("path=/home/agent/.claude/projects/-w/memory/team.md\nok=1\n"),
                Some("exec") if has("sbxw-thread-end") => Output::ok("ok=1\n"),
                _ => sbx_answer(a),
            }
        });
        let sb = SandboxRef::parse("myapp").unwrap();
        let ws = tempfile::tempdir().unwrap();
        std::fs::create_dir(ws.path().join(".git")).unwrap();
        app.store.update(&sb, |r| r.workspace = Some(ws.path().to_path_buf())).unwrap();

        let p = app.plan_propose(&sb, "cart", vec![t("api"), t("web")], Variant::Shared, "same files", Some("lead")).await.unwrap();
        let worktree = app.shared_worktree(&sb, &p.threads, &p.team_name()).await.unwrap();
        let team = format!("team-{}", p.id);
        assert_eq!(worktree, Some(ws.path().join(".sbxw-worktrees").join(&team).to_string_lossy().into_owned()));
        app.plans.lock().unwrap().approve(&p.id, vec![t("api"), t("web")], None, worktree, 1).unwrap();

        // Each thread opens in the plan's worktree, with the team's env.
        let api = sbxw_core::ThreadName::parse("api").unwrap();
        let tm = app.team_for(&sb, &api).unwrap();
        assert_eq!((tm.plan.as_str(), tm.worktree.as_str()), (p.id.as_str(), team.as_str()));
        let argv = sbxw_sbx::scripts::thread_run_args(&sb.name, "/w", &api, "", None, Some(&tm), sbxw_sbx::scripts::StartMode::New);
        assert_eq!(&argv[argv.len() - 3..], [format!("={team}"), format!("={}", p.id), "=new".to_string()]);
        let listed: Vec<String> = app.threads(&sb).await.threads.iter().map(|v| v["name"].as_str().unwrap().to_string()).collect();
        assert_eq!(listed, ["api", "web"], "its threads are listed though they have no worktree of their own");

        for n in ["api", "web"] {
            app.plans.lock().unwrap().set_thread("myapp", n, ThreadState::Done, "", false, 2).unwrap();
        }
        let d = vec![DistillItem { kind: DistillKind::Decision, text: "prices in cents".into() }];
        app.plan_propose_merge(&p.id, &sb, vec![], "", d).unwrap();
        let mut rx = app.bus.subscribe();
        let merge = app.plan_merge(&p.id, true, None).await.unwrap();
        assert_eq!(merge.state, MergeState::Merged);
        let mut closed = vec![];
        while let Ok(ev) = rx.try_recv() {
            if let Event::ThreadClosed(c) = ev {
                closed.push(c.thread);
            }
        }
        closed.sort();
        assert_eq!(closed, ["api", "web"], "a merge that closes the threads publishes each close once");
        assert_eq!(merge.results.len(), 1);
        assert_eq!(merge.results[0].thread, team);

        let log = fake.log();
        let merges: Vec<&String> = log.iter().filter(|c| c.contains("sbxw-thread-merge")).collect();
        assert_eq!(merges.len(), 1, "one merge, of one branch");
        assert!(merges[0].ends_with(&format!("={team}")));
        assert!(log.iter().any(|c| c.contains("sbxw-memory") && c.contains(&format!("={team}.md"))));
        let stdin = String::from_utf8(fake.stdins.lock().unwrap().last().unwrap().clone()).unwrap();
        assert!(stdin.contains("- **Decision:** prices in cents"));
        assert!(
            log.iter().any(|c| c.contains("sbxw-thread-end") && c.contains(&format!("={team} =close =0 =1"))),
            "the shared worktree is closed, branch and all"
        );
        assert!(app.team_for(&sb, &api).is_none());
    }

    #[tokio::test]
    async fn a_retried_merge_only_merges_the_threads_not_yet_merged() {
        let b_fails = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
        let flag = b_fails.clone();
        let (_d, app, fake) = app_with(move |a| {
            let has = |x: &str| a.iter().any(|y| y == x);
            match a.first().map(String::as_str) {
                Some("ls") => ls(&[("myapp", "running")]),
                Some("exec") if has("sbxw-thread-merge") && a.iter().any(|y| y == "=b") && flag.load(std::sync::atomic::Ordering::SeqCst) => Output::ok("error=conflict in x.rs\n"),
                Some("exec") if has("sbxw-thread-merge") => Output::ok("base=main\ncommit=abc1234\nok=1\n"),
                _ => sbx_answer(a),
            }
        });
        let sb = SandboxRef::parse("myapp").unwrap();
        let ws = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(ws.path().join("r/.git")).unwrap();
        app.store.update(&sb, |r| r.workspace = Some(ws.path().to_path_buf())).unwrap();
        let on = |n: &str| PlanThread { repo: Some("r".into()), ..t(n) };
        let p = app.plan_propose(&sb, "", vec![on("a"), on("b")], Variant::Split, "independent", Some("lead")).await.unwrap();
        app.plans.lock().unwrap().approve(&p.id, vec![on("a"), on("b")], None, None, 1).unwrap();
        for n in ["a", "b"] {
            app.plans.lock().unwrap().set_thread("myapp", n, ThreadState::Done, "", false, 2).unwrap();
        }
        app.plan_propose_merge(&p.id, &sb, vec!["a".into(), "b".into()], "", vec![]).unwrap();
        let first = app.plan_merge(&p.id, false, None).await.unwrap();
        assert_eq!(first.state, MergeState::Failed);
        assert_eq!(first.results.iter().map(|r| r.ok).collect::<Vec<_>>(), [true, false]);

        b_fails.store(false, std::sync::atomic::Ordering::SeqCst);
        let before = fake.log().iter().filter(|c| c.contains("sbxw-thread-merge")).count();
        let retry = app.plan_merge(&p.id, false, None).await.unwrap();
        assert_eq!(retry.state, MergeState::Merged);
        assert_eq!(retry.results.iter().map(|r| (r.thread.as_str(), r.ok)).collect::<Vec<_>>(), [("a", true), ("b", true)]);
        let log = fake.log();
        let merges: Vec<&String> = log.iter().filter(|c| c.contains("sbxw-thread-merge")).skip(before).collect();
        assert_eq!(merges.len(), 1, "only b is merged again: {merges:?}");
        assert!(merges[0].ends_with("=b"));
    }

    #[tokio::test]
    async fn a_shared_plan_needs_one_repo_and_a_split_plan_is_as_before() {
        let (_d, app) = app();
        let sb = SandboxRef::parse("myapp").unwrap();
        let ws = tempfile::tempdir().unwrap();
        for r in ["a/.git", "b/.git"] {
            std::fs::create_dir_all(ws.path().join(r)).unwrap();
        }
        app.store.update(&sb, |r| r.workspace = Some(ws.path().to_path_buf())).unwrap();
        let on = |name: &str, repo: &str| PlanThread { repo: Some(repo.into()), ..t(name) };
        let err = app
            .plan_propose(&sb, "", vec![on("x", "a"), on("y", "b")], Variant::Shared, "live", None)
            .await
            .unwrap_err();
        assert!(err.contains("one repo"), "{err}");
        let p = app.plan_propose(&sb, "", vec![on("x", "a"), on("y", "b")], Variant::Split, "independent", None).await.unwrap();
        let p = app.plans.lock().unwrap().approve(&p.id, vec![on("x", "a"), on("y", "b")], None, None, 1).unwrap();
        assert_eq!((p.variant, p.worktree), (Variant::Split, None));
        assert!(app.team_for(&sb, &sbxw_core::ThreadName::parse("x").unwrap()).is_none());
    }

    #[tokio::test]
    async fn a_shared_plan_is_not_merged_while_files_are_claimed() {
        let (_d, app, _fake) = app_with(|a| {
            let has = |x: &str| a.iter().any(|y| y == x);
            match a.first().map(String::as_str) {
                Some("ls") => ls(&[("myapp", "running")]),
                Some("exec") if has("sbxw-thread-merge") => Output::ok("base=main\ncommit=abc1234\nok=1\n"),
                Some("exec") if has("sbxw-memory") => Output::ok("path=/m/team.md\nok=1\n"),
                Some("exec") if has("sbxw-thread-end") => Output::ok("ok=1\n"),
                _ => sbx_answer(a),
            }
        });
        let sb = SandboxRef::parse("myapp").unwrap();
        let ws = tempfile::tempdir().unwrap();
        std::fs::create_dir(ws.path().join(".git")).unwrap();
        app.store.update(&sb, |r| r.workspace = Some(ws.path().to_path_buf())).unwrap();
        let p = app.plan_propose(&sb, "cart", vec![t("api"), t("web")], Variant::Shared, "same files", Some("lead")).await.unwrap();
        let worktree = app.shared_worktree(&sb, &p.threads, &p.team_name()).await.unwrap();
        app.plans.lock().unwrap().approve(&p.id, vec![t("api"), t("web")], None, worktree, 1).unwrap();
        for n in ["api", "web"] {
            app.plans.lock().unwrap().set_thread("myapp", n, ThreadState::Done, "", false, 2).unwrap();
        }
        app.plan_propose_merge(&p.id, &sb, vec![], "", vec![]).unwrap();

        let files = vec!["a.rs".to_string()];
        app.board_claim("myapp", Some(&p.id), "api", &files).unwrap();
        let err = app.plan_merge(&p.id, false, None).await.unwrap_err();
        assert!(err.contains("1 file still claimed: a.rs (api)") && err.contains("force_release"), "{err}");
        assert_ne!(app.plan(&p.id).unwrap().merge.unwrap().state, MergeState::Merged, "nothing was merged");

        app.board_release("myapp", Some(&p.id), "api", &files).unwrap();
        assert_eq!(app.plan_merge(&p.id, false, None).await.unwrap().state, MergeState::Merged);
    }
}
