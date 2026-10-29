//! Plans around their book: persistence, the lead's waits, launching an
//! approved plan's threads, the one-click merge, and lead mode itself.

use crate::app::{now_ms, App};
use crate::bus::Event;
use crate::git;
use crate::store::write_atomic;
use sbxw_core::messages;
use sbxw_core::plans::{DistillItem, MergeResult, MergeState, Plan, PlanState, PlanThread, ThreadState, Variant};
use sbxw_sbx::scripts::Team;
use sbxw_core::session::{SessionKey, SessionKind};
use sbxw_core::tooling::Tooling;
use sbxw_core::{ModelId, SandboxName, ThreadName};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::broadcast::error::RecvError;

impl App {
    /// Write `sandbox`'s plans (or remove the file when none are left) and
    /// broadcast. Every change comes through here. The directory is 0700: a
    /// plan id is a capability.
    fn plan_changed(&self, plan: Plan) -> Plan {
        self.persist_plans(&plan.sandbox);
        self.send(Event::Plan(plan.clone()));
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
        if let Ok(sb) = SandboxName::parse(&plan.sandbox) {
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
    fn check_plan_repos(&self, sandbox: &SandboxName, threads: &[PlanThread]) -> Result<(), String> {
        let Some(ws) = self.store.workspace(sandbox) else {
            return if threads.iter().any(|t| t.repo.is_some()) {
                Err(format!("sbxw has no workspace recorded for '{sandbox}', so threads cannot name a repo"))
            } else {
                Ok(())
            };
        };
        let rec = self.store.get(sandbox);
        for t in threads {
            let name = t.name.trim();
            let recorded = rec.threads.get(name).map(|r| r.repo.as_str());
            git::choose(&ws, recorded, name, t.repo.as_deref()).map_err(|e| format!("thread {name:?}: {e}"))?;
        }
        Ok(())
    }

    /// A shared plan's threads all work in one worktree, so in one repo: the
    /// one they resolve to, and the worktree's path in it. `Ok(None)` when the
    /// sandbox has no workspace recorded (the scripts find the repo there).
    fn shared_worktree(&self, sandbox: &SandboxName, threads: &[PlanThread], team: &str) -> Result<Option<String>, String> {
        let Some(ws) = self.store.workspace(sandbox) else { return Ok(None) };
        let rec = self.store.get(sandbox);
        let mut repos = std::collections::BTreeSet::new();
        for t in threads {
            let name = t.name.trim();
            let recorded = rec.threads.get(name).map(|r| r.repo.as_str());
            repos.insert(git::choose(&ws, recorded, name, t.repo.as_deref()).map_err(|e| format!("thread {name:?}: {e}"))?);
        }
        match repos.len() {
            1 => {
                let rel = repos.into_iter().next().unwrap_or_default();
                Ok(git::team_worktree(&ws, &rel, team).map(|p| p.to_string_lossy().into_owned()))
            }
            _ => Err(format!(
                "a shared plan works in one worktree, so its threads need one repo; these name {}. \
                 Pick one repo, or make the plan split.",
                repos.iter().map(|r| if r.is_empty() { "the workspace".to_string() } else { format!("'{r}'") }).collect::<Vec<_>>().join(", ")
            )),
        }
    }

    /// The shared plan `thread` works under, as its scripts take it.
    pub fn team_for(&self, sandbox: &SandboxName, thread: &ThreadName) -> Option<Team> {
        let plans = self.plans.lock().unwrap();
        let plan = plans.team_of(sandbox, thread)?;
        Some(Team { plan: plan.id.clone(), worktree: ThreadName::parse(&plan.team_name()).ok()? })
    }

    pub fn plan_propose(
        &self,
        sandbox: &SandboxName,
        summary: &str,
        threads: Vec<PlanThread>,
        variant: Variant,
        justification: &str,
        lead: Option<&str>,
    ) -> Result<Plan, String> {
        self.check_plan_repos(sandbox, &threads)?;
        if variant == Variant::Shared {
            self.shared_worktree(sandbox, &threads, "team")?;
        }
        let plan = self.plans.lock().unwrap().propose(
            sbxw_core::ids::plan_id(),
            sandbox,
            summary,
            threads,
            variant,
            justification,
            lead,
            now_ms(),
        )?;
        tracing::info!("plans: '{sandbox}' proposed {} ({} thread(s), {:?})", plan.id, plan.threads.len(), plan.variant);
        Ok(self.plan_changed(plan))
    }

    /// Park until the plan is settled or the bound passes.
    pub async fn plan_wait(&self, id: &str, sandbox: &SandboxName, bound: Duration) -> Result<Plan, String> {
        let mut rx = self.bus.subscribe();
        let mut latest = self.plans.lock().unwrap().owned(id, sandbox)?.clone();
        let deadline = tokio::time::Instant::now() + bound;
        while latest.state == PlanState::Pending {
            let left = deadline.saturating_duration_since(tokio::time::Instant::now());
            if left.is_zero() {
                break;
            }
            match tokio::time::timeout(left, rx.recv()).await {
                Ok(Ok(Event::Plan(p))) if p.id == id => latest = p,
                Ok(Ok(_)) => {}
                Ok(Err(RecvError::Lagged(_))) => latest = self.plans.lock().unwrap().owned(id, sandbox)?.clone(),
                Ok(Err(RecvError::Closed)) | Err(_) => break,
            }
        }
        Ok(latest)
    }

    pub fn plan_led(&self, id: &str, sandbox: &SandboxName) -> Result<Plan, String> {
        self.plans.lock().unwrap().led(id, sandbox).cloned()
    }

    pub fn plan_propose_merge(&self, id: &str, sandbox: &SandboxName, order: Vec<String>, note: &str, distillate: Vec<DistillItem>) -> Result<Plan, String> {
        let order: Vec<String> = order.iter().map(|t| t.trim().to_string()).collect();
        let note = messages::clip_text(note, sbxw_core::plans::MAX_TEXT);
        let plan = self.plans.lock().unwrap().propose_merge(id, sandbox, order, &note, distillate, now_ms())?;
        tracing::info!("plans: '{sandbox}' proposed merging {id}");
        Ok(self.plan_changed(plan))
    }

    /// A thread reports. When the plan has no lead name, blocked and done are
    /// typed into the main session so the lead still hears. Returns the lead's
    /// address for the thread's own `SendMessage`.
    pub fn plan_report(self: &Arc<Self>, sandbox: &SandboxName, thread: &ThreadName, state: ThreadState, summary: &str) -> Result<Option<String>, String> {
        let summary = messages::clip_text(summary, sbxw_core::plans::MAX_TEXT);
        let plan = self.plans.lock().unwrap().set_thread(sandbox, thread, state, &summary, false, now_ms())?;
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
    /// per thread with its briefing as its start prompt — in its own worktree,
    /// or all in the plan's one worktree when it is shared. Refused, before
    /// anything is recorded, when the threads would not fit under the ceiling
    /// or a shared plan's threads name more than one repo.
    pub async fn plan_approve(self: &Arc<Self>, id: &str, threads: Vec<PlanThread>, variant: Option<Variant>) -> Result<Plan, String> {
        let plan = self.plan(id).ok_or_else(|| format!("no plan '{id}'"))?;
        let sandbox = SandboxName::parse(&plan.sandbox).map_err(|e| e.to_string())?;
        let cfg = self.config_for(&sandbox);
        let open = self.sessions.agents_open(&sandbox);
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
        self.check_plan_repos(&sandbox, &threads)?;
        let worktree = match variant.unwrap_or(plan.variant) {
            Variant::Split => None,
            Variant::Shared => self.shared_worktree(&sandbox, &threads, &plan.team_name())?,
        };
        let plan = self.plans.lock().unwrap().approve(id, threads, variant, worktree, now_ms())?;
        let plan = self.plan_changed(plan);
        tracing::info!("plans: {id} approved for '{sandbox}' ({} thread(s), {:?})", plan.threads.len(), plan.variant);
        for t in &plan.threads {
            let Ok(name) = ThreadName::parse(&t.name) else { continue };
            if let Err(e) = self.settle_thread_repo(&sandbox, &name, t.repo.as_deref()) {
                tracing::warn!("plans: thread '{name}' of '{sandbox}': {e}");
            } else if let Some(m) = t.model.as_deref().and_then(|m| ModelId::parse(m).ok()) {
                if let Err(e) = self.settle_thread_model(&sandbox, &name, &m, true) {
                    tracing::warn!("plans: {e}");
                }
            }
            self.briefings
                .lock()
                .unwrap()
                .insert((sandbox.to_string(), name.to_string()), messages::thread_briefing(&plan, t));
        }
        for t in plan.threads.clone() {
            let app = self.clone();
            let sandbox = sandbox.clone();
            tokio::spawn(async move { app.launch_thread(&sandbox, &t.name).await });
        }
        Ok(plan)
    }

    async fn launch_thread(self: &Arc<Self>, sandbox: &SandboxName, name: &str) {
        let Ok(thread) = ThreadName::parse(name) else { return };
        let key = SessionKey::thread(sandbox, &thread);
        let outcome = match self.open_session(&key).await {
            Err(e) => Err(format!("{e:#}")),
            Ok((session, _)) => {
                // Still here: the session was already running and never read its
                // briefing as a start prompt — type it in instead.
                let unread = self.briefings.lock().unwrap().remove(&(sandbox.to_string(), name.to_string()));
                match unread {
                    Some(b) => crate::typing::type_and_submit(&session, &b, false).await,
                    None => Ok(()),
                }
            }
        };
        if let Err(e) = outcome {
            tracing::warn!("plans: thread '{name}' of '{sandbox}' did not start: {e}");
            if let Ok(p) = self.plans.lock().unwrap().set_thread(sandbox, name, ThreadState::Failed, &e, true, now_ms()) {
                self.send(Event::Plan(p.clone()));
                self.persist_plans(&p.sandbox);
            }
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
        let plan = self.plan(id).ok_or_else(|| format!("no plan '{id}'"))?;
        let merge = plan.merge.clone().ok_or_else(|| format!("plan {id} has no merge proposed"))?;
        let sandbox = SandboxName::parse(&plan.sandbox).map_err(|e| e.to_string())?;
        if plan.variant == Variant::Shared {
            self.refuse_while_claimed(&plan)?;
            return self.plan_merge_shared(plan, merge, &sandbox, close_threads, distillate).await;
        }
        // Where each thread merges — read first, so the base is known from the
        // first broadcast on.
        let dirs: Vec<(String, Result<String, String>)> = merge
            .order
            .iter()
            .map(|t| {
                let planned = plan.threads.iter().find(|p| &p.name == t).and_then(|p| p.repo.clone());
                let dir = ThreadName::parse(t).map_err(|e| e.to_string()).and_then(|name| {
                    let recorded = self.store.get(&sandbox).threads.contains_key(t.as_str());
                    self.settle_thread_repo(&sandbox, &name, if recorded { None } else { planned.as_deref() })
                });
                (t.clone(), dir)
            })
            .collect();
        let bases: std::collections::BTreeSet<Option<String>> = dirs
            .iter()
            .map(|(_, d)| match d {
                Ok(dir) if !dir.is_empty() => git::branch_of(std::path::Path::new(dir)),
                _ => None,
            })
            .collect();
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
                (Ok(()), Ok(dir), Ok(name)) => self.sbx.thread_merge(&sandbox, &dir, &name).await.map_err(|e| format!("{e:#}")),
            };
            let ok = outcome.is_ok();
            let result = match outcome {
                Ok(commit) => MergeResult { thread: thread.clone(), ok: true, commit: Some(commit).filter(|c| !c.is_empty()), error: None },
                Err(e) => MergeResult { thread: thread.clone(), ok: false, commit: None, error: Some(e) },
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

    async fn tell_lead_merged(self: &Arc<Self>, sandbox: &SandboxName, id: &str, summary: &str) {
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
        sandbox: &SandboxName,
        close_threads: bool,
        distillate: Option<Vec<DistillItem>>,
    ) -> Result<sbxw_core::plans::Merge, String> {
        let id = plan.id.as_str();
        let team = ThreadName::parse(&plan.team_name()).map_err(|e| e.to_string())?;
        // Every thread is in the one repo; the first one's settles it.
        let dir = match plan.threads.first() {
            Some(t) => ThreadName::parse(&t.name).map_err(|e| e.to_string()).and_then(|name| {
                let recorded = self.store.get(sandbox).threads.contains_key(t.name.as_str());
                self.settle_thread_repo(sandbox, &name, if recorded { None } else { t.repo.as_deref() })
            }),
            None => Err(format!("plan {id} has no thread")),
        };
        let base = match &dir {
            Ok(d) if !d.is_empty() => git::branch_of(std::path::Path::new(d)),
            _ => None,
        };
        let p = self.plans.lock().unwrap().start_merge(id, base, distillate, now_ms())?;
        self.plan_changed(p);
        tracing::info!("plans: merging shared {id} for '{sandbox}' (branch sbxw/{team})");
        let outcome = match (self.ensure_running_for_thread(sandbox).await, dir) {
            (Err(e), _) => Err(format!("the sandbox could not be started: {e:#}")),
            (Ok(()), Err(e)) => Err(e),
            (Ok(()), Ok(dir)) => self.sbx.thread_merge(sandbox, &dir, &team).await.map(|c| (c, dir)).map_err(|e| format!("{e:#}")),
        };
        let (result, dir) = match outcome {
            Ok((commit, dir)) => (MergeResult { thread: team.to_string(), ok: true, commit: Some(commit).filter(|c| !c.is_empty()), error: None }, Some(dir)),
            Err(e) => (MergeResult { thread: team.to_string(), ok: false, commit: None, error: Some(e) }, None),
        };
        self.plans.lock().unwrap().merge_result(id, result, now_ms())?;
        let plan = self.plans.lock().unwrap().finish_merge(id, now_ms())?;
        let plan = self.plan_changed(plan);
        let merge = plan.merge.clone().unwrap_or(merge);
        let mut summary = messages::merge_summary(&merge);
        if let (Some(dir), MergeState::Merged) = (&dir, merge.state) {
            summary = format!("[sbxw] Merged sbxw/{team} (threads {}).", merge.order.join(", "));
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
                if let Err(e) = self.sbx.thread_close(sandbox, dir, &team, false, true).await {
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
    async fn write_memory(&self, sandbox: &SandboxName, dir: &str, file: &str, body: &str, index: &str) -> Result<String, String> {
        let args = sbxw_sbx::scripts::memory_write_args(dir, file, index);
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        let out = self.sbx.exec(sandbox, &args, Some(body.as_bytes().to_vec())).await.map_err(|e| format!("{e:#}"))?;
        let fields = sbxw_sbx::scripts::script_fields(&out.stdout_str())?;
        if !out.success() {
            return Err(format!("the memory script failed: {}", String::from_utf8_lossy(&out.stderr).trim()));
        }
        Ok(fields.get("path").cloned().unwrap_or_default())
    }

    /// Close a thread: stop what runs in its worktree, remove the worktree,
    /// the branch if asked. Dirty work is refused without `discard` — the
    /// daemon does not trust a click it did not see.
    pub async fn close_thread(&self, sandbox: &SandboxName, thread: &ThreadName, discard: bool, delete_branch: bool) -> Result<(), String> {
        // A session in the main checkout has no worktree to remove, and its
        // folder is the main agent's: nothing there is killed by cwd.
        // A shared plan's thread has no worktree of its own either: it leaves
        // its plan, and the plan's worktree goes with the last of them.
        let mut team_end = None;
        if self.is_same_worktree(sandbox, thread) {
            tracing::info!("closing session '{thread}' of '{sandbox}', in the main checkout");
        } else if let Some(team) = self.team_for(sandbox, thread) {
            let dir = self.settle_thread_repo(sandbox, thread, None).ok();
            let left = self.plans.lock().unwrap().drop_team_thread(sandbox.as_str(), thread.as_str(), now_ms());
            if let Some(plan) = left {
                tracing::info!("closing '{thread}' of '{sandbox}': it leaves shared plan {}", plan.id);
                if plan.state == PlanState::Refused {
                    team_end = dir.map(|d| (d, team.worktree));
                }
                self.plan_changed(plan);
            }
        } else {
            match self.settle_thread_repo(sandbox, thread, None) {
                Ok(dir) => self
                    .sbx
                    .thread_close(sandbox, &dir, thread, discard, delete_branch)
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
        let gone = self.agents.lock().unwrap().forget_thread(sandbox.as_str(), thread.as_str(), now_ms());
        for info in gone {
            self.send(Event::Session(info));
        }
        if let Err(e) = self.store.update(sandbox, |r| {
            r.threads.remove(thread.as_str());
        }) {
            tracing::warn!("threads: could not forget the repo of '{thread}': {e:#}");
        }
        Ok(())
    }

    /// The shared worktree of a plan whose every thread was closed: removed
    /// with its branch when it holds nothing — no uncommitted file, no commit
    /// its base lacks. Anything in it is work, and is left for a person.
    async fn end_team_worktree(&self, sandbox: &SandboxName, dir: &str, team: &ThreadName) {
        match self.sbx.thread_status(sandbox, dir, team).await {
            Ok(st) if st.dirty == 0 && st.ahead == 0 => {
                if let Err(e) = self.sbx.thread_close(sandbox, dir, team, false, true).await {
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
    pub async fn lead(self: &Arc<Self>, sandbox: &SandboxName) -> Result<(), String> {
        let token = self.store.token(sandbox).map_err(|e| format!("{e:#}"))?;
        let batch = Tooling::new()
            .agent_token(&token)
            .relay(self.cfg.agent_port())
            .lead_command();
        let outcome = self.sbx.tooling(sandbox, &batch).await.map_err(|e| format!("{e:#}"))?;
        if let Some((piece, e)) = outcome.failed.first() {
            return Err(format!("{piece}: {e}"));
        }
        self.push_text(sandbox, SessionKind::Agent, messages::LEAD_COMMAND).await?;
        if let Some(s) = self.sessions.live(&SessionKey::agent(sandbox)) {
            self.leads.lock().unwrap().insert(sandbox.to_string(), Arc::as_ptr(&s) as usize);
            self.send(Event::Lead(sandbox.to_string()));
        }
        Ok(())
    }

    pub async fn lead_end(self: &Arc<Self>, sandbox: &SandboxName) {
        self.leads.lock().unwrap().remove(sandbox.as_str());
        let ended = self.plans.lock().unwrap().end_lead(sandbox, now_ms());
        tracing::info!("plans: lead mode ended for '{sandbox}' ({} plan(s) refused or closed)", ended.len());
        for p in ended {
            self.plan_changed(p);
        }
        if self.sessions.is_live(&SessionKey::agent(sandbox)) {
            if let Err(e) = self.push_text(sandbox, SessionKind::Agent, messages::LEAD_END).await {
                tracing::warn!("plans: could not tell '{sandbox}' that lead mode ended: {e}");
            }
        }
        self.send(Event::LeadEnd(sandbox.to_string()));
    }

    /// Sandboxes whose main session is still the very session made lead.
    pub fn leading(&self) -> Vec<String> {
        let mut leads = self.leads.lock().unwrap();
        leads.retain(|name, ptr| {
            SandboxName::parse(name)
                .ok()
                .and_then(|n| self.sessions.live(&SessionKey::agent(&n)))
                .is_some_and(|s| Arc::as_ptr(&s) as usize == *ptr)
        });
        let mut out: Vec<String> = leads.keys().cloned().collect();
        out.sort();
        out
    }
}

#[cfg(test)]
mod tests {
    use crate::testing::{app, app_with, ls, sbx_answer};
    use sbxw_core::plans::{DistillItem, DistillKind, MergeState, PlanState, PlanThread, ThreadState, Variant};
    use sbxw_sbx::runner::Output;
    use sbxw_core::SandboxName;
    use std::time::Duration;

    #[tokio::test]
    async fn a_chat_session_without_a_repo_still_closes() {
        let (_d, app) = app();
        let name = SandboxName::parse("ephemeral-chat").unwrap();
        let ws = app.store.paths.chat_workspace(name.as_str());
        std::fs::create_dir_all(&ws).unwrap();
        let thread = sbxw_core::ThreadName::parse("chat-2").unwrap();
        app.store
            .update(&name, |r| {
                r.workspace = Some(ws.clone());
                r.threads.entry("chat-2".into()).or_default();
            })
            .unwrap();
        app.close_thread(&name, &thread, true, true).await.unwrap();
        assert!(!app.store.get(&name).threads.contains_key("chat-2"), "the record is forgotten");

        let other = SandboxName::parse("neos").unwrap();
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
        }
    }

    #[tokio::test]
    async fn a_proposal_is_persisted_and_survives_a_restart() {
        let (dir, app) = app();
        let sb = SandboxName::parse("neos").unwrap();
        let p = app.plan_propose(&sb, "split", vec![t("api")], Variant::Split, "independent", Some("lead")).unwrap();
        let file = app.store.paths.plans().join("neos.json");
        assert!(file.exists());
        let again = crate::testing::app_in(dir.path());
        assert_eq!(again.plan(&p.id).unwrap().state, PlanState::Pending);
        // Another sandbox cannot wait on it.
        assert!(app.plan_wait(&p.id, &SandboxName::parse("other").unwrap(), Duration::from_millis(10)).await.is_err());
    }

    #[tokio::test]
    async fn a_refusal_wakes_the_lead() {
        let (_d, app) = app();
        let sb = SandboxName::parse("neos").unwrap();
        let p = app.plan_propose(&sb, "", vec![t("api")], Variant::Split, "one thread", None).unwrap();
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
                Some("ls") => ls(&[("neos", "running")]),
                Some("exec") if has("sbxw-thread-merge") => Output::ok("base=main\ncommit=abc1234\nok=1\n"),
                Some("exec") if has("sbxw-memory") => Output::ok("path=/home/agent/.claude/projects/-w/memory/team.md\nok=1\n"),
                Some("exec") if has("sbxw-thread-end") => Output::ok("ok=1\n"),
                _ => sbx_answer(a),
            }
        });
        let sb = SandboxName::parse("neos").unwrap();
        let ws = tempfile::tempdir().unwrap();
        std::fs::create_dir(ws.path().join(".git")).unwrap();
        app.store.update(&sb, |r| r.workspace = Some(ws.path().to_path_buf())).unwrap();

        let p = app.plan_propose(&sb, "cart", vec![t("api"), t("web")], Variant::Shared, "same files", Some("lead")).unwrap();
        let worktree = app.shared_worktree(&sb, &p.threads, &p.team_name()).unwrap();
        let team = format!("team-{}", p.id);
        assert_eq!(worktree, Some(ws.path().join(".sbxw-worktrees").join(&team).to_string_lossy().into_owned()));
        app.plans.lock().unwrap().approve(&p.id, vec![t("api"), t("web")], None, worktree, 1).unwrap();

        // Each thread opens in the plan's worktree, with the team's env.
        let api = sbxw_core::ThreadName::parse("api").unwrap();
        let tm = app.team_for(&sb, &api).unwrap();
        assert_eq!((tm.plan.as_str(), tm.worktree.as_str()), (p.id.as_str(), team.as_str()));
        let argv = sbxw_sbx::scripts::thread_run_args(&sb, "/w", &api, "", None, Some(&tm));
        assert_eq!(&argv[argv.len() - 2..], [format!("={team}"), format!("={}", p.id)]);
        let listed: Vec<String> = app.threads(&sb).await.threads.iter().map(|v| v["name"].as_str().unwrap().to_string()).collect();
        assert_eq!(listed, ["api", "web"], "its threads are listed though they have no worktree of their own");

        for n in ["api", "web"] {
            app.plans.lock().unwrap().set_thread("neos", n, ThreadState::Done, "", false, 2).unwrap();
        }
        let d = vec![DistillItem { kind: DistillKind::Decision, text: "prices in cents".into() }];
        app.plan_propose_merge(&p.id, &sb, vec![], "", d).unwrap();
        let merge = app.plan_merge(&p.id, true, None).await.unwrap();
        assert_eq!(merge.state, MergeState::Merged);
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
    async fn a_shared_plan_needs_one_repo_and_a_split_plan_is_as_before() {
        let (_d, app) = app();
        let sb = SandboxName::parse("neos").unwrap();
        let ws = tempfile::tempdir().unwrap();
        for r in ["a/.git", "b/.git"] {
            std::fs::create_dir_all(ws.path().join(r)).unwrap();
        }
        app.store.update(&sb, |r| r.workspace = Some(ws.path().to_path_buf())).unwrap();
        let on = |name: &str, repo: &str| PlanThread { repo: Some(repo.into()), ..t(name) };
        let err = app
            .plan_propose(&sb, "", vec![on("x", "a"), on("y", "b")], Variant::Shared, "live", None)
            .unwrap_err();
        assert!(err.contains("one repo"), "{err}");
        let p = app.plan_propose(&sb, "", vec![on("x", "a"), on("y", "b")], Variant::Split, "independent", None).unwrap();
        let p = app.plans.lock().unwrap().approve(&p.id, vec![on("x", "a"), on("y", "b")], None, None, 1).unwrap();
        assert_eq!((p.variant, p.worktree), (Variant::Split, None));
        assert!(app.team_for(&sb, &sbxw_core::ThreadName::parse("x").unwrap()).is_none());
    }

    #[tokio::test]
    async fn a_shared_plan_is_not_merged_while_files_are_claimed() {
        let (_d, app, _fake) = app_with(|a| {
            let has = |x: &str| a.iter().any(|y| y == x);
            match a.first().map(String::as_str) {
                Some("ls") => ls(&[("neos", "running")]),
                Some("exec") if has("sbxw-thread-merge") => Output::ok("base=main\ncommit=abc1234\nok=1\n"),
                Some("exec") if has("sbxw-memory") => Output::ok("path=/m/team.md\nok=1\n"),
                Some("exec") if has("sbxw-thread-end") => Output::ok("ok=1\n"),
                _ => sbx_answer(a),
            }
        });
        let sb = SandboxName::parse("neos").unwrap();
        let ws = tempfile::tempdir().unwrap();
        std::fs::create_dir(ws.path().join(".git")).unwrap();
        app.store.update(&sb, |r| r.workspace = Some(ws.path().to_path_buf())).unwrap();
        let p = app.plan_propose(&sb, "cart", vec![t("api"), t("web")], Variant::Shared, "same files", Some("lead")).unwrap();
        let worktree = app.shared_worktree(&sb, &p.threads, &p.team_name()).unwrap();
        app.plans.lock().unwrap().approve(&p.id, vec![t("api"), t("web")], None, worktree, 1).unwrap();
        for n in ["api", "web"] {
            app.plans.lock().unwrap().set_thread("neos", n, ThreadState::Done, "", false, 2).unwrap();
        }
        app.plan_propose_merge(&p.id, &sb, vec![], "", vec![]).unwrap();

        let files = vec!["a.rs".to_string()];
        app.board_claim("neos", Some(&p.id), "api", &files).unwrap();
        let err = app.plan_merge(&p.id, false, None).await.unwrap_err();
        assert!(err.contains("1 file still claimed: a.rs (api)") && err.contains("force_release"), "{err}");
        assert_ne!(app.plan(&p.id).unwrap().merge.unwrap().state, MergeState::Merged, "nothing was merged");

        app.board_release("neos", Some(&p.id), "api", &files).unwrap();
        assert_eq!(app.plan_merge(&p.id, false, None).await.unwrap().state, MergeState::Merged);
    }
}
