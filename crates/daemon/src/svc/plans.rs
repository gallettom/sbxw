//! Plans around their book: persistence, the lead's waits, launching an
//! approved plan's threads, the one-click merge, and lead mode itself.

use crate::app::{now_ms, App};
use crate::bus::Event;
use crate::git;
use crate::store::write_atomic;
use sbxw_core::messages;
use sbxw_core::plans::{MergeResult, MergeState, Plan, PlanState, PlanThread, ThreadState};
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

    pub fn plan_propose(&self, sandbox: &SandboxName, summary: &str, threads: Vec<PlanThread>, lead: Option<&str>) -> Result<Plan, String> {
        self.check_plan_repos(sandbox, &threads)?;
        let plan = self
            .plans
            .lock()
            .unwrap()
            .propose(sbxw_core::ids::plan_id(), sandbox, summary, threads, lead, now_ms())?;
        tracing::info!("plans: '{sandbox}' proposed {} ({} thread(s))", plan.id, plan.threads.len());
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

    pub fn plan_propose_merge(&self, id: &str, sandbox: &SandboxName, order: Vec<String>, note: &str) -> Result<Plan, String> {
        let order: Vec<String> = order.iter().map(|t| t.trim().to_string()).collect();
        let note = messages::clip_text(note, sbxw_core::plans::MAX_TEXT);
        let plan = self.plans.lock().unwrap().propose_merge(id, sandbox, order, &note, now_ms())?;
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

    /// A person approves (maybe after editing): the daemon records each
    /// thread's repo and model, then opens one session per thread with its
    /// briefing as its start prompt. Refused, before anything is recorded,
    /// when the threads would not fit under the ceiling.
    pub async fn plan_approve(self: &Arc<Self>, id: &str, threads: Vec<PlanThread>) -> Result<Plan, String> {
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
        let plan = self.plans.lock().unwrap().approve(id, threads, now_ms())?;
        let plan = self.plan_changed(plan);
        tracing::info!("plans: {id} approved for '{sandbox}' ({} thread(s))", plan.threads.len());
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
    /// lead hears how it went in one line.
    pub async fn plan_merge(self: &Arc<Self>, id: &str, close_threads: bool) -> Result<sbxw_core::plans::Merge, String> {
        let plan = self.plan(id).ok_or_else(|| format!("no plan '{id}'"))?;
        let merge = plan.merge.clone().ok_or_else(|| format!("plan {id} has no merge proposed"))?;
        let sandbox = SandboxName::parse(&plan.sandbox).map_err(|e| e.to_string())?;
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
        let p = self.plans.lock().unwrap().start_merge(id, base, now_ms())?;
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
        tracing::info!("plans: {id}: {summary}");
        if self.sessions.is_live(&SessionKey::agent(&sandbox)) {
            if let Err(e) = self.push_text(&sandbox, SessionKind::Agent, &summary).await {
                tracing::warn!("plans: could not tell the lead of '{sandbox}' about the merge: {e}");
            }
        }
        Ok(merge)
    }

    /// Close a thread: stop what runs in its worktree, remove the worktree,
    /// the branch if asked. Dirty work is refused without `discard` — the
    /// daemon does not trust a click it did not see.
    pub async fn close_thread(&self, sandbox: &SandboxName, thread: &ThreadName, discard: bool, delete_branch: bool) -> Result<(), String> {
        let dir = self.settle_thread_repo(sandbox, thread, None)?;
        self.sbx
            .thread_close(sandbox, &dir, thread, discard, delete_branch)
            .await
            .map_err(|e| format!("{e:#}"))?;
        self.sessions.remove(&SessionKey::thread(sandbox, thread));
        self.sessions.remove(&SessionKey::new(sandbox.clone(), SessionKind::ThreadBash(thread.clone())));
        if let Err(e) = self.store.update(sandbox, |r| {
            r.threads.remove(thread.as_str());
        }) {
            tracing::warn!("threads: could not forget the repo of '{thread}': {e:#}");
        }
        Ok(())
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
    use crate::testing::app;
    use sbxw_core::plans::{PlanState, PlanThread, ThreadState};
    use sbxw_core::SandboxName;
    use std::time::Duration;

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
        let p = app.plan_propose(&sb, "split", vec![t("api")], Some("lead")).unwrap();
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
        let p = app.plan_propose(&sb, "", vec![t("api")], None).unwrap();
        let a = app.clone();
        let (id, s) = (p.id.clone(), sb.clone());
        let w = tokio::spawn(async move { a.plan_wait(&id, &s, Duration::from_secs(5)).await });
        tokio::time::sleep(Duration::from_millis(30)).await;
        app.plan_refuse(&p.id, Some("no".into())).unwrap();
        assert_eq!(w.await.unwrap().unwrap().state, PlanState::Refused);
    }
}
