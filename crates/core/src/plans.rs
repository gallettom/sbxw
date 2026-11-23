//! Plans: a lead session splits work into threads, and a person approves the
//! split before anything starts.
//!
//! The lead (the sandbox's main agent after `/lead`) proposes threads: name,
//! goal, scope, optionally repo and model. The plan waits here as `Pending`
//! until someone approves (maybe after editing) or refuses it in the web UI.
//! Only an approved plan starts anything. Threads report working, blocked or
//! done; once all are done the lead may propose a merge, which a person clicks.
//!
//! Lead-to-thread talk is Claude Code's `SendMessage`, by session name, within
//! one sandbox. The book is pure: the daemon persists it, broadcasts it, and
//! launches the sessions.

use crate::names::{is_valid_model_id, is_valid_thread_name, normalize_repo_path};
use crate::team::{self, EventKind, TeamLog};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap, HashSet};

pub const MAX_THREADS: usize = 8;
pub const MAX_TEXT: usize = 4000;
/// What the lead's plan tools answer once the user ended lead mode: about
/// plan `id`, or about leading at all.
pub fn lead_ended(id: Option<&str>) -> String {
    match id {
        Some(id) => format!(
            "the user ended lead mode, so plan {id} is no longer yours to follow. \
             Stop leading: no new plan and no follow-up of its threads."
        ),
        None => "the user ended lead mode (or never turned it on in this sandbox). \
                 Stop leading: no new plan and no follow-up of its threads."
            .into(),
    }
}

/// Goal and scope are a brief, not a note: over this a proposal is refused,
/// not cut, since a cut goal reaches the thread looking whole.
pub const MAX_BRIEF: usize = 16000;
/// Settled plans with all threads finished are dropped this long after their
/// last change.
pub const PRUNE_AFTER_MS: u64 = 24 * 3600 * 1000;

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Debug)]
#[serde(rename_all = "lowercase")]
pub enum PlanState {
    Pending,
    Approved,
    Refused,
}

/// How threads share code (Team mode). Set at proposal, editable in the
/// approval dialog, fixed once approved.
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Debug, Default)]
#[serde(rename_all = "lowercase")]
pub enum Variant {
    /// A worktree and branch per thread, each merged at the end.
    #[default]
    Split,
    /// One worktree and one branch for every thread, merged once.
    Shared,
}

impl Variant {
    pub fn is_split(&self) -> bool {
        *self == Variant::Split
    }
}

/// Kept once a shared plan merges: what still holds, written to the sandbox's
/// Claude Code auto memory.
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Debug)]
#[serde(rename_all = "lowercase")]
pub enum DistillKind {
    Decision,
    Warning,
}

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq, Eq)]
pub struct DistillItem {
    pub kind: DistillKind,
    pub text: String,
}

pub const MAX_DISTILLATE: usize = 40;

/// Distillate items trimmed and clipped, empty ones dropped, at most
/// `MAX_DISTILLATE`.
pub fn clean_distillate(items: Vec<DistillItem>) -> Result<Vec<DistillItem>, String> {
    let items: Vec<DistillItem> = items
        .into_iter()
        .map(|i| DistillItem { kind: i.kind, text: clip(&i.text) })
        .filter(|i| !i.text.is_empty())
        .collect();
    if items.len() > MAX_DISTILLATE {
        return Err(format!("a distillate has at most {MAX_DISTILLATE} items"));
    }
    Ok(items)
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Debug, Default)]
#[serde(rename_all = "lowercase")]
pub enum ThreadState {
    #[default]
    Starting,
    Working,
    Blocked,
    Done,
    Failed,
}

impl ThreadState {
    /// What a thread may report about itself; `Starting` and `Failed` are the
    /// daemon's.
    pub fn reportable(self) -> bool {
        matches!(self, ThreadState::Working | ThreadState::Blocked | ThreadState::Done)
    }
    pub fn is_finished(self) -> bool {
        matches!(self, ThreadState::Done | ThreadState::Failed)
    }
}

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq)]
pub struct PlanThread {
    pub name: String,
    pub goal: String,
    #[serde(default)]
    pub scope: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default)]
    pub state: ThreadState,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub updated_ms: u64,
    /// Who set `state` and `summary` when it was not the thread itself:
    /// sbxw (a start that failed, with its reason) or the lead (a thread it
    /// marked done). Absent for the thread's own reports.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub set_by: Option<SetBy>,
    /// The person's ticket this thread works on (`T-<n>` of the plan's
    /// sandbox), given by the lead in `propose_plan`. Additive: a plan without
    /// tickets is as before. See `tickets.rs`.
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "crate::tickets::de_ticket")]
    pub ticket: Option<u32>,
}

/// Who set a thread's state, when not the thread.
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Debug)]
#[serde(rename_all = "lowercase")]
pub enum SetBy {
    /// The daemon: the thread could not start, `summary` says why.
    Sbxw,
    /// The lead marked it done after reviewing it (`mark_thread_done`).
    Lead,
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Debug)]
#[serde(rename_all = "lowercase")]
pub enum MergeState {
    Proposed,
    Merging,
    Merged,
    Failed,
    /// The person gave the proposal up (Abandon merge): nothing was merged
    /// by it after that, nothing was closed or deleted. Kept as the plan's
    /// merge, with `abandoned_ms`, until the lead proposes again, which is
    /// a fresh proposal. Not a merge to do: no Merge button offers it.
    Abandoned,
}

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq, Eq)]
pub struct MergeResult {
    pub thread: String,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// A merge that went through, with something left alone in the checkout
    /// (tracked files absent from it, edits the merge did not touch).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq)]
pub struct Merge {
    pub order: Vec<String>,
    pub note: String,
    pub state: MergeState,
    #[serde(default)]
    pub base: Option<String>,
    #[serde(default)]
    pub results: Vec<MergeResult>,
    /// Proposed by the lead, then as the person approved it at Merge.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub distillate: Vec<DistillItem>,
    /// When the person abandoned it ([`MergeState::Abandoned`]). Only the
    /// person can, from the plan dialog; the plan's log says so too.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub abandoned_ms: Option<u64>,
}

/// Where git shows a done thread's branch, when sbxw never merged it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ext {
    /// The branch is already in its base: merged by hand, or empty.
    Outside,
    /// The branch no longer exists.
    Gone,
}

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq)]
pub struct Plan {
    pub id: String,
    pub sandbox: String,
    pub summary: String,
    pub threads: Vec<PlanThread>,
    pub state: PlanState,
    #[serde(default)]
    pub variant: Variant,
    /// The lead's one line on why this variant.
    #[serde(default)]
    pub justification: String,
    /// A shared plan's worktree, set when it is approved.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// The lead's Claude Code session name — its threads' `SendMessage` address.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lead: Option<String>,
    /// Team mode ended: threads run on, reports no longer reach the lead.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub closed: bool,
    /// The person closed this plan from the Kanban: its unmerged threads
    /// leave the review columns. Marks the plan only; never touches git.
    /// Not `closed`: that one means the lead ended, and Team on again
    /// gives such a plan back.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub dismissed: bool,
    /// What git showed the daemon about threads sbxw never merged, by
    /// thread name; set by the daemon before the board is built, never saved.
    #[serde(skip)]
    pub ext: HashMap<String, Ext>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub merge: Option<Merge>,
    /// Merges before `merge`, oldest first: a split plan merged, then a
    /// thread finished later and was proposed in a merge of its own. Their
    /// results stay the plan's history, and their threads are never merged
    /// again.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub earlier_merges: Vec<Merge>,
    /// Uncommitted changes (tracked and untracked) in the main checkout of
    /// the repo(s) the threads work in, read on the host when the plan was
    /// proposed: work the threads' worktrees, cut from `HEAD`, will not
    /// have. The host holds the whole repository, so files a sandbox merely
    /// does not mount are not counted. `None` when it could not be read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub main_uncommitted: Option<u32>,
    pub created_ms: u64,
    pub updated_ms: u64,
}

impl Plan {
    /// A shared plan's worktree under `.sbxw-worktrees/`, branch
    /// `sbxw/team-<plan id>`: a valid thread name, so thread scripts treat it
    /// as one.
    pub fn team_name(&self) -> String {
        team_name(&self.id)
    }

    /// Every merge result that went through, in this merge and the earlier
    /// ones: the threads sbxw merged (a shared plan: its one branch).
    pub fn merged_results(&self) -> impl Iterator<Item = &MergeResult> {
        self.earlier_merges.iter().chain(self.merge.iter()).flat_map(|m| m.results.iter()).filter(|r| r.ok)
    }

    /// The threads sbxw merged, in any of the plan's merges.
    pub fn merged_threads(&self) -> HashSet<&str> {
        self.merged_results().map(|r| r.thread.as_str()).collect()
    }
}

impl Plan {
    /// The distillate as one auto memory: file name, contents, and `MEMORY.md`
    /// index line. `None` when nothing was kept.
    pub fn distillate_memory(&self) -> Option<(String, String, String)> {
        let items = &self.merge.as_ref()?.distillate;
        if items.is_empty() {
            return None;
        }
        let file = format!("{}.md", self.team_name());
        let one_line = |s: &str, n: usize| {
            let s: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
            match s.char_indices().nth(n) {
                Some((i, _)) => format!("{}…", &s[..i]),
                None => s,
            }
        };
        let what = if self.summary.is_empty() { "a team plan".to_string() } else { one_line(&self.summary, 100) };
        let mut body = format!(
            "---\nname: {team}\ndescription: {description}\n\
             metadata:\n  type: project\n---\n\n\
             Kept from sbxw team plan {id} when its branch sbxw/{team} was merged. The plan: {what}\n\n",
            team = self.team_name(),
            id = self.id,
            // JSON's string is a YAML double-quoted scalar: a summary with
            // `: ` in it stays one value.
            description = serde_json::to_string(&format!("Decisions and warnings that still hold after {what}")).unwrap_or_default(),
        );
        for i in items {
            let kind = match i.kind {
                DistillKind::Decision => "Decision",
                DistillKind::Warning => "Warning",
            };
            body.push_str(&format!("- **{kind}:** {}\n", one_line(&i.text, MAX_TEXT)));
        }
        let index = format!("- [Team plan {}]({file}) — decisions and warnings from: {}", self.id, one_line(&what, 80));
        Some((file, body, index))
    }
}

pub fn team_name(plan_id: &str) -> String {
    format!("team-{plan_id}")
}

pub fn clip(s: &str) -> String {
    let s = s.trim();
    match s.char_indices().nth(MAX_TEXT) {
        Some((i, _)) => format!("{}\n…[truncated by sbxw]", &s[..i]),
        None => s.to_string(),
    }
}

/// `s` trimmed, or an error naming `what` when it is over `max` chars.
fn brief(s: &str, max: usize, what: &str) -> Result<String, String> {
    let s = s.trim();
    let n = s.chars().count();
    if n > max {
        return Err(format!("{what} is {n} characters, over the limit of {max}: shorten it (or put the detail in a file in the repo and point to it)"));
    }
    Ok(s.to_string())
}

/// The threads of a proposal or of an edited plan: names a worktree can be
/// made from, none twice, each with a goal, repo and model well-formed.
pub fn validate(threads: Vec<PlanThread>) -> Result<Vec<PlanThread>, String> {
    if threads.is_empty() {
        return Err("a plan needs at least one thread".into());
    }
    if threads.len() > MAX_THREADS {
        return Err(format!("a plan has at most {MAX_THREADS} threads"));
    }
    let mut seen = HashSet::new();
    threads
        .into_iter()
        .map(|t| {
            let name = t.name.trim().to_string();
            if !is_valid_thread_name(&name) {
                return Err(format!(
                    "thread name {name:?}: 1–40 lower-case letters, digits and inner hyphens"
                ));
            }
            if !seen.insert(name.clone()) {
                return Err(format!("thread name {name:?} is used twice"));
            }
            let goal = brief(&t.goal, MAX_BRIEF, &format!("the goal of thread {name:?}"))?;
            let scope = brief(&t.scope, MAX_BRIEF, &format!("the scope of thread {name:?}"))?;
            if goal.is_empty() {
                return Err(format!("thread {name:?} has no goal"));
            }
            let repo = t
                .repo
                .as_deref()
                .map(|r| normalize_repo_path(r).map_err(|e| format!("thread {name:?}: {e}")))
                .transpose()?;
            let model = match t.model.as_deref().map(str::trim) {
                None | Some("") => None,
                Some(m) if is_valid_model_id(m) => Some(m.to_string()),
                Some(m) => return Err(format!("thread {name:?}: {m:?} is not a model id")),
            };
            Ok(PlanThread {
                name,
                goal,
                scope,
                repo,
                model,
                state: ThreadState::Starting,
                summary: String::new(),
                updated_ms: 0,
                set_by: None,
                ticket: t.ticket,
            })
        })
        .collect()
}

/// A lead's session name as threads will be told it: one printable line.
/// Anything else is dropped, and the plan falls back to the daemon telling it.
pub fn lead_address(name: Option<&str>) -> Option<String> {
    let name = name?.trim();
    let ok = !name.is_empty() && name.chars().count() <= 100 && !name.chars().any(char::is_control);
    ok.then(|| name.to_string())
}

#[derive(Default, Debug)]
pub struct PlanBook {
    plans: HashMap<String, Plan>,
    /// Each plan's history, for its timeline (`team`): kept apart from the
    /// plan, which reaches the lead's tools whole.
    logs: HashMap<String, TeamLog>,
}

impl PlanBook {
    pub fn new() -> Self {
        Self::default()
    }

    /// Start from plans read back from disk.
    pub fn load(plans: impl IntoIterator<Item = Plan>) -> Self {
        Self {
            plans: plans.into_iter().map(|p| (p.id.clone(), p)).collect(),
            logs: HashMap::new(),
        }
    }

    /// The logs read back from disk; those of plans no longer here are dropped.
    pub fn load_logs(&mut self, logs: impl IntoIterator<Item = (String, TeamLog)>) {
        for (id, log) in logs {
            if self.plans.contains_key(&id) {
                self.logs.insert(id, log);
            }
        }
    }

    /// Plan `id`'s log (empty for a plan from before logs).
    pub fn log_of(&self, id: &str) -> TeamLog {
        self.logs.get(id).cloned().unwrap_or_default()
    }

    /// `sandbox`'s logs, by plan id: what is written to its log file.
    pub fn logs_of(&self, sandbox: &str) -> BTreeMap<String, TeamLog> {
        self.logs
            .iter()
            .filter(|(id, _)| self.plans.get(*id).is_some_and(|p| p.sandbox == sandbox))
            .map(|(id, l)| (id.clone(), l.clone()))
            .collect()
    }

    /// A line on plan `id`'s log.
    #[allow(clippy::too_many_arguments)]
    pub fn note(&mut self, id: &str, at: u64, kind: EventKind, from: &str, to: Option<&str>, text: Option<&str>, data: Value) {
        if self.plans.contains_key(id) {
            self.logs.entry(id.to_string()).or_default().push(at, kind, from, to, text, data);
        }
    }

    /// The plan a relay request of `sandbox` belongs to: its thread's, or for
    /// the lead, the sandbox's latest plan still pending or at work.
    pub fn plan_for_request(&self, sandbox: &str, thread: Option<&str>) -> Option<String> {
        if let Some(t) = thread {
            return self.plan_of_thread(sandbox, t);
        }
        self.plans
            .values()
            .filter(|p| p.sandbox == sandbox && !p.closed)
            .filter(|p| match p.state {
                PlanState::Pending => true,
                PlanState::Approved => !p.merge.as_ref().is_some_and(|m| m.state == MergeState::Merged),
                PlanState::Refused => false,
            })
            .max_by_key(|p| p.created_ms)
            .map(|p| p.id.clone())
    }

    pub fn list(&self) -> Vec<Plan> {
        let mut out: Vec<Plan> = self.plans.values().cloned().collect();
        out.sort_by(|a, b| a.created_ms.cmp(&b.created_ms).then(a.id.cmp(&b.id)));
        out
    }

    pub fn get(&self, id: &str) -> Option<&Plan> {
        self.plans.get(id)
    }

    /// `sandbox`'s plans, oldest first — what is written to its file.
    pub fn of(&self, sandbox: &str) -> Vec<Plan> {
        let mut out: Vec<Plan> = self.plans.values().filter(|p| p.sandbox == sandbox).cloned().collect();
        out.sort_by_key(|p| p.created_ms);
        out
    }

    /// The plan `id`, if it is `sandbox`'s. Not found and someone else's read
    /// the same.
    pub fn owned(&self, id: &str, sandbox: &str) -> Result<&Plan, String> {
        self.plans
            .get(id)
            .filter(|p| p.sandbox == sandbox)
            .ok_or_else(|| format!("no plan '{id}' for '{sandbox}'"))
    }

    /// `owned`, for the lead's tools: a closed plan says so.
    pub fn led(&self, id: &str, sandbox: &str) -> Result<&Plan, String> {
        let plan = self.owned(id, sandbox)?;
        if plan.closed {
            return Err(lead_ended(Some(id)));
        }
        Ok(plan)
    }

    /// One pending plan per sandbox, saying why its variant.
    #[allow(clippy::too_many_arguments)]
    pub fn propose(
        &mut self,
        id: String,
        sandbox: &str,
        summary: &str,
        threads: Vec<PlanThread>,
        variant: Variant,
        justification: &str,
        lead: Option<&str>,
        now: u64,
    ) -> Result<Plan, String> {
        let threads = validate(threads)?;
        let justification = clip(justification);
        if justification.is_empty() {
            return Err(format!(
                "say in one line why this plan is {:?} (justification): shared when threads touch \
                 overlapping files, must integrate live or share a dev server; split when the work \
                 streams are independent, long or risky",
                variant
            )
            .to_lowercase());
        }
        if let Some(p) = self
            .plans
            .values()
            .find(|p| p.sandbox == sandbox && p.state == PlanState::Pending)
        {
            return Err(format!(
                "plan {} is still waiting for the user; wait for it with check_plan",
                p.id
            ));
        }
        let plan = Plan {
            id: id.clone(),
            sandbox: sandbox.to_string(),
            summary: clip(summary),
            threads,
            state: PlanState::Pending,
            variant,
            justification,
            worktree: None,
            note: None,
            lead: lead_address(lead),
            closed: false,
            dismissed: false,
            ext: HashMap::new(),
            merge: None,
            earlier_merges: vec![],
            main_uncommitted: None,
            created_ms: now,
            updated_ms: now,
        };
        self.plans.insert(id.clone(), plan.clone());
        self.note(&id, now, EventKind::PlanProposed, team::LEAD, Some(team::USER), Some(&plan.summary), team::proposal_data(&plan));
        Ok(plan)
    }

    /// Record what the main checkout held when plan `id` was proposed.
    pub fn note_main_uncommitted(&mut self, id: &str, n: Option<u32>) -> Option<Plan> {
        let p = self.plans.get_mut(id)?;
        p.main_uncommitted = n;
        Some(p.clone())
    }

    fn mutate<F>(&mut self, id: &str, now: u64, f: F) -> Result<Plan, String>
    where
        F: FnOnce(&mut Plan) -> Result<(), String>,
    {
        let plan = self.plans.get_mut(id).ok_or_else(|| format!("no plan '{id}'"))?;
        f(plan)?;
        plan.updated_ms = now;
        Ok(plan.clone())
    }

    fn mutate_pending<F>(&mut self, id: &str, now: u64, f: F) -> Result<Plan, String>
    where
        F: FnOnce(&mut Plan) -> Result<(), String>,
    {
        self.mutate(id, now, |p| {
            if p.state != PlanState::Pending {
                return Err(format!("plan {id} was already settled"));
            }
            f(p)
        })
    }

    /// Approves with the threads as edited in the dialog. A thread still running
    /// under another approved plan of the sandbox is refused: two plans must not
    /// write one worktree. `variant` is the dialog's choice, else the proposal's.
    /// A shared plan's worktree is recorded.
    pub fn approve(
        &mut self,
        id: &str,
        threads: Vec<PlanThread>,
        variant: Option<Variant>,
        worktree: Option<String>,
        now: u64,
    ) -> Result<Plan, String> {
        let mut threads = validate(threads)?;
        let sandbox = self.plans.get(id).map(|p| p.sandbox.clone()).unwrap_or_default();
        // A dialog that does not know tickets sends threads without them:
        // each keeps the proposal's link by name.
        if let Some(p) = self.plans.get(id) {
            for t in threads.iter_mut().filter(|t| t.ticket.is_none()) {
                t.ticket = p.threads.iter().find(|o| o.name == t.name).and_then(|o| o.ticket);
            }
        }
        let taken: HashSet<String> = self
            .approved_threads(&sandbox)
            .into_iter()
            .filter(|(_, t)| !t.state.is_finished())
            .map(|(_, t)| t.name)
            .collect();
        if let Some(t) = threads.iter().find(|t| taken.contains(&t.name)) {
            return Err(format!(
                "thread {:?} already belongs to an approved plan that is still running",
                t.name
            ));
        }
        self.mutate_pending(id, now, |p| {
            p.threads = threads;
            for t in &mut p.threads {
                t.updated_ms = now;
            }
            if let Some(v) = variant {
                p.variant = v;
            }
            p.worktree = if p.variant.is_split() { None } else { worktree };
            p.state = PlanState::Approved;
            Ok(())
        })
        .inspect(|p| {
            let names: Vec<&str> = p.threads.iter().map(|t| t.name.as_str()).collect();
            self.note(id, now, EventKind::PlanApproved, team::USER, Some(team::LEAD), None, json!({ "variant": p.variant, "threads": names }));
        })
    }

    pub fn refuse(&mut self, id: &str, note: Option<String>, now: u64) -> Result<Plan, String> {
        self.mutate_pending(id, now, |p| {
            p.state = PlanState::Refused;
            p.note = note.map(|n| clip(&n)).filter(|n| !n.is_empty());
            Ok(())
        })
        .inspect(|p| self.note(id, now, EventKind::PlanRefused, team::USER, Some(team::LEAD), p.note.as_deref(), Value::Null))
    }

    /// Team mode ended: the pending plan is refused, approved ones closed.
    pub fn end_lead(&mut self, sandbox: &str, now: u64) -> Vec<Plan> {
        let ended: Vec<Plan> = self
            .plans
            .values_mut()
            .filter(|p| p.sandbox == sandbox)
            .filter_map(|p| {
                match p.state {
                    PlanState::Pending => {
                        p.state = PlanState::Refused;
                        p.note = Some("lead mode ended".into());
                    }
                    PlanState::Approved if !p.closed => p.closed = true,
                    _ => return None,
                }
                p.updated_ms = now;
                Some(p.clone())
            })
            .collect();
        for p in &ended {
            match p.state {
                PlanState::Refused => self.note(&p.id, now, EventKind::PlanRefused, team::SBXW, Some(team::LEAD), p.note.as_deref(), Value::Null),
                _ => self.note(&p.id, now, EventKind::TeamClosed, team::USER, Some(team::LEAD), Some("Team mode ended"), Value::Null),
            }
        }
        ended
    }

    /// The person closes a settled plan from the Kanban. Refused while a
    /// thread still works or a merge is proposed or running. Only marks
    /// the plan; git is never touched.
    pub fn dismiss(&mut self, id: &str, now: u64) -> Result<Plan, String> {
        self.mutate(id, now, |p| {
            if p.state != PlanState::Approved {
                return Err(format!("plan {id} is not a running plan"));
            }
            if p.threads.iter().any(|t| matches!(t.state, ThreadState::Starting | ThreadState::Working | ThreadState::Blocked)) {
                return Err("a thread of this plan still works".into());
            }
            if p.merge.as_ref().is_some_and(|m| matches!(m.state, MergeState::Proposed | MergeState::Merging)) {
                return Err("a merge of this plan is proposed or running".into());
            }
            p.dismissed = true;
            Ok(())
        })
    }

    /// Team mode on again in `sandbox`: the approved plans its end closed come
    /// back to the sandbox's lead, oldest first. Only `end_lead` sets `closed`,
    /// so a closed plan is always one whose lead ended, never one the person
    /// settled: refused plans and merged ones stay as they are.
    pub fn resume_lead(&mut self, sandbox: &str, now: u64) -> Vec<Plan> {
        let mut out: Vec<Plan> = self
            .plans
            .values_mut()
            .filter(|p| p.sandbox == sandbox && p.closed && p.state == PlanState::Approved)
            .filter(|p| !p.merge.as_ref().is_some_and(|m| m.state == MergeState::Merged))
            .map(|p| {
                p.closed = false;
                p.updated_ms = now;
                p.clone()
            })
            .collect();
        out.sort_by(|a, b| a.created_ms.cmp(&b.created_ms).then(a.id.cmp(&b.id)));
        out
    }

    /// The lead proposes merging finished threads, in order. A shared plan has
    /// one branch, so all threads merge at once and must all be done; an empty
    /// order means all of them.
    pub fn propose_merge(
        &mut self,
        id: &str,
        sandbox: &str,
        order: Vec<String>,
        note: &str,
        distillate: Vec<DistillItem>,
        now: u64,
    ) -> Result<Plan, String> {
        let plan = self.led(id, sandbox)?;
        if plan.state != PlanState::Approved {
            return Err(format!("plan {id} is not approved, so there is nothing to merge"));
        }
        let distillate = clean_distillate(distillate)?;
        let order = match plan.variant {
            Variant::Split => order,
            Variant::Shared => {
                let all: Vec<String> = plan.threads.iter().map(|t| t.name.clone()).collect();
                if let Some(n) = order.iter().find(|n| !all.contains(n)) {
                    return Err(format!("{n:?} is not a thread of plan {id}"));
                }
                all
            }
        };
        if order.is_empty() {
            return Err("name the threads to merge, in order".into());
        }
        let mut seen = HashSet::new();
        for name in &order {
            if !seen.insert(name.as_str()) {
                return Err(format!("thread {name:?} is named twice"));
            }
            match plan.threads.iter().find(|t| &t.name == name) {
                None => return Err(format!("{name:?} is not a thread of plan {id}")),
                Some(t) if t.state != ThreadState::Done => {
                    return Err(format!(
                        "thread {name:?} is not done (it is {:?}); propose the merge once it is",
                        t.state
                    )
                    .to_lowercase())
                }
                Some(_) => {}
            }
        }
        // A split plan merged (or partly merged) takes a new proposal for the
        // threads that finished since: never one it merged already. A shared
        // plan has one branch, merged once.
        let merged = plan.merged_threads();
        if let Some(m) = &plan.merge {
            let late = plan.variant.is_split() && m.state != MergeState::Merging;
            if !late && matches!(m.state, MergeState::Merging | MergeState::Merged) {
                return Err(format!("plan {id} is already {:?}", m.state).to_lowercase());
            }
        }
        if plan.variant.is_split() {
            if let Some(name) = order.iter().find(|n| merged.contains(n.as_str())) {
                return Err(format!("thread {name:?} is already merged; leave it out of the order"));
            }
        }
        self.mutate(id, now, |p| {
            // An earlier merge that merged something stays in the history.
            if let Some(m) = p.merge.take().filter(|m| m.results.iter().any(|r| r.ok)) {
                p.earlier_merges.push(m);
            }
            p.merge = Some(Merge {
                order,
                note: clip(note),
                state: MergeState::Proposed,
                base: None,
                results: vec![],
                distillate,
                abandoned_ms: None,
            });
            Ok(())
        })
        .inspect(|p| {
            let m = p.merge.as_ref().expect("just proposed");
            self.note(id, now, EventKind::MergeProposed, team::LEAD, Some(team::USER), Some(&m.note), json!({ "order": m.order, "distillate": m.distillate.len() }));
        })
    }

    /// The person abandons the merge proposed (or failed) for plan `id`:
    /// the plan no longer has a merge to do. Refused while it is merging,
    /// and once merged. Merges nothing, closes nothing, deletes nothing: the
    /// merge keeps its results (a thread a failed merge already merged
    /// stays merged), and the lead may propose again.
    pub fn abandon_merge(&mut self, id: &str, now: u64) -> Result<Plan, String> {
        self.mutate(id, now, |p| {
            if p.state != PlanState::Approved {
                return Err(format!("plan {id} is not approved"));
            }
            let m = p.merge.as_mut().ok_or_else(|| format!("plan {id} has no merge proposed"))?;
            match m.state {
                MergeState::Proposed | MergeState::Failed => {}
                MergeState::Merging => return Err(format!("plan {id} is merging: wait for the merge to end")),
                MergeState::Merged => return Err(format!("plan {id} is already merged")),
                MergeState::Abandoned => return Err(format!("the merge of plan {id} is already abandoned")),
            }
            m.state = MergeState::Abandoned;
            m.abandoned_ms = Some(now);
            Ok(())
        })
        .inspect(|p| {
            let m = p.merge.as_ref().expect("abandoned");
            self.note(id, now, EventKind::MergeAbandoned, team::USER, Some(team::LEAD), Some("The user abandoned the proposed merge: nothing was merged"), json!({ "order": m.order }));
        })
    }

    /// A person clicked Merge, keeping `distillate` (`None`: as proposed).
    pub fn start_merge(&mut self, id: &str, base: Option<String>, distillate: Option<Vec<DistillItem>>, now: u64) -> Result<Plan, String> {
        let distillate = distillate.map(clean_distillate).transpose()?;
        self.mutate(id, now, |p| {
            if p.state != PlanState::Approved {
                return Err(format!("plan {id} is not approved"));
            }
            let m = p.merge.as_mut().ok_or_else(|| format!("plan {id} has no merge proposed"))?;
            match m.state {
                MergeState::Proposed | MergeState::Failed => {}
                s => return Err(format!("plan {id} is already {s:?}").to_lowercase()),
            }
            m.state = MergeState::Merging;
            m.base = base;
            // A retry keeps merged threads (skipped): they may be closed since,
            // and merging twice is never wanted.
            m.results.retain(|r| r.ok);
            if let Some(d) = distillate {
                m.distillate = d;
            }
            Ok(())
        })
        .inspect(|p| {
            let m = p.merge.as_ref().expect("merging");
            self.note(id, now, EventKind::MergeStarted, team::USER, None, None, json!({ "order": m.order, "base": m.base }));
        })
    }

    pub fn merge_result(&mut self, id: &str, result: MergeResult, now: u64) -> Result<Plan, String> {
        let data = team::merge_result_data(&result);
        let text = result.error.clone().or(result.note.clone());
        self.mutate(id, now, |p| {
            p.merge
                .as_mut()
                .ok_or_else(|| format!("plan {id} has no merge"))?
                .results
                .push(result);
            Ok(())
        })
        .inspect(|_| self.note(id, now, EventKind::MergeResult, team::SBXW, None, text.as_deref(), data))
    }

    /// Merged when every result is in and ok: one per thread, or the one
    /// branch of a shared plan.
    pub fn finish_merge(&mut self, id: &str, now: u64) -> Result<Plan, String> {
        self.mutate(id, now, |p| {
            let expected = if p.variant.is_split() { p.merge.as_ref().map_or(0, |m| m.order.len()) } else { 1 };
            let m = p.merge.as_mut().ok_or_else(|| format!("plan {id} has no merge"))?;
            let all = m.results.len() == expected && m.results.iter().all(|r| r.ok);
            m.state = if all { MergeState::Merged } else { MergeState::Failed };
            Ok(())
        })
        .inspect(|p| {
            let m = p.merge.as_ref().expect("merged");
            self.note(id, now, EventKind::MergeFinished, team::SBXW, None, None, json!({ "state": m.state, "order": m.order }));
        })
    }

    pub fn approved_threads(&self, sandbox: &str) -> Vec<(String, PlanThread)> {
        self.plans
            .values()
            .filter(|p| p.sandbox == sandbox && p.state == PlanState::Approved)
            .flat_map(|p| p.threads.iter().map(|t| (p.id.clone(), t.clone())))
            .collect()
    }

    /// The shared plan `thread` works under, if any: the latest approved plan
    /// holding its name, when that plan is shared and not merged yet. Its
    /// sessions open in the plan's worktree.
    pub fn team_of(&self, sandbox: &str, thread: &str) -> Option<&Plan> {
        self.plans
            .values()
            .filter(|p| p.sandbox == sandbox && p.state == PlanState::Approved)
            .filter(|p| p.threads.iter().any(|t| t.name == thread))
            .max_by_key(|p| p.created_ms)
            .filter(|p| p.variant == Variant::Shared)
            .filter(|p| !p.merge.as_ref().is_some_and(|m| m.state == MergeState::Merged))
    }

    /// A person closed `thread` of a shared plan: it leaves the plan, so the
    /// thread list stops offering it (shared threads have no worktree to be
    /// found by). A plan losing its last thread is refused, taking its board
    /// with it. `None` when `thread` is in no shared plan at work.
    pub fn drop_team_thread(&mut self, sandbox: &str, thread: &str, now: u64) -> Option<Plan> {
        let id = self.team_of(sandbox, thread)?.id.clone();
        let p = self.plans.get_mut(&id)?;
        p.threads.retain(|t| t.name != thread);
        if p.threads.is_empty() {
            p.state = PlanState::Refused;
            p.note = Some("every thread was closed".into());
        }
        p.updated_ms = now;
        let p = p.clone();
        self.note(&id, now, EventKind::ThreadClosed, team::USER, None, None, json!({ "thread": thread }));
        Some(p)
    }

    /// The sandbox's shared plan at work: latest approved, shared, not merged.
    /// Its board belongs to it.
    pub fn team_plan(&self, sandbox: &str) -> Option<&Plan> {
        self.plans
            .values()
            .filter(|p| p.sandbox == sandbox && p.state == PlanState::Approved && p.variant == Variant::Shared)
            .filter(|p| !p.merge.as_ref().is_some_and(|m| m.state == MergeState::Merged))
            .max_by_key(|p| p.created_ms)
    }

    /// Where a thread stands. A thread reports only the reportable states; the
    /// daemon may set any. The latest approved plan holding the name wins.
    pub fn set_thread(
        &mut self,
        sandbox: &str,
        thread: &str,
        state: ThreadState,
        summary: &str,
        by_daemon: bool,
        now: u64,
    ) -> Result<Plan, String> {
        if !by_daemon && !state.reportable() {
            return Err("a thread reports working, blocked or done".into());
        }
        let id = self
            .plans
            .values()
            .filter(|p| p.sandbox == sandbox && p.state == PlanState::Approved)
            .filter(|p| p.threads.iter().any(|t| t.name == thread))
            .max_by_key(|p| p.created_ms)
            .map(|p| p.id.clone())
            .ok_or_else(|| format!("'{thread}' is not a thread of an approved plan in '{sandbox}'"))?;
        self.mutate(&id, now, |p| {
            if let Some(t) = p.threads.iter_mut().find(|t| t.name == thread) {
                t.state = state;
                t.summary = clip(summary);
                t.updated_ms = now;
                t.set_by = by_daemon.then_some(SetBy::Sbxw);
            }
            Ok(())
        })
        .inspect(|p| {
            let text = p.threads.iter().find(|t| t.name == thread).map(|t| t.summary.clone()).unwrap_or_default();
            let data = json!({ "thread": thread, "state": state });
            if by_daemon {
                self.note(&p.id, now, EventKind::ThreadFailed, team::SBXW, None, Some(&text), data);
            } else {
                self.note(&p.id, now, EventKind::ThreadStatus, thread, Some(team::LEAD), Some(&text), data);
            }
        })
    }

    /// The approved plan `thread` belongs to (the latest holding its name).
    fn plan_of_thread(&self, sandbox: &str, thread: &str) -> Option<String> {
        self.plans
            .values()
            .filter(|p| p.sandbox == sandbox && p.state == PlanState::Approved)
            .filter(|p| p.threads.iter().any(|t| t.name == thread))
            .max_by_key(|p| p.created_ms)
            .map(|p| p.id.clone())
    }

    /// The approved plan `thread` works under, and its entry there: the
    /// latest approved plan holding the name, as `set_thread` picks it.
    pub fn thread_plan(&self, sandbox: &str, thread: &str) -> Option<(&Plan, &PlanThread)> {
        let p = self.plans.get(&self.plan_of_thread(sandbox, thread)?)?;
        Some((p, p.threads.iter().find(|t| t.name == thread)?))
    }

    /// `thread` could not start (its worktree, its branch, its session):
    /// failed, with the reason, set by sbxw. A thread already done keeps its
    /// state: the failure is a later reopen's, and the work is finished.
    /// `None` when it is in no approved plan, or nothing changed.
    pub fn start_failed(&mut self, sandbox: &str, thread: &str, reason: &str, now: u64) -> Option<Plan> {
        let id = self.plan_of_thread(sandbox, thread)?;
        let t = self.plans.get(&id)?.threads.iter().find(|t| t.name == thread)?;
        let reason = clip(reason);
        if t.state == ThreadState::Done || (t.state == ThreadState::Failed && t.summary == reason) {
            return None;
        }
        self.set_thread(sandbox, thread, ThreadState::Failed, &reason, true, now).ok()
    }

    /// `thread` started after all: a failure sbxw recorded is cleared (back to
    /// starting, until the thread reports). `None` when there was none.
    pub fn start_cleared(&mut self, sandbox: &str, thread: &str, now: u64) -> Option<Plan> {
        let id = self.plan_of_thread(sandbox, thread)?;
        let t = self.plans.get(&id)?.threads.iter().find(|t| t.name == thread)?;
        if !(t.state == ThreadState::Failed && t.set_by == Some(SetBy::Sbxw)) {
            return None;
        }
        self.mutate(&id, now, |p| {
            if let Some(t) = p.threads.iter_mut().find(|t| t.name == thread) {
                t.state = ThreadState::Starting;
                t.summary.clear();
                t.set_by = None;
                t.updated_ms = now;
            }
            Ok(())
        })
        .ok()
    }

    /// The lead marks `thread` of its plan `id` done, having reviewed its
    /// work, when the thread will not say so itself (its session is gone, or
    /// it reported to the lead only). This only lets the lead propose the
    /// merge, which a person still approves; the plan records that the lead
    /// set it, and the dialog says so. `note` (what the lead checked) is
    /// required. A thread that reported done itself is left as it is.
    pub fn mark_done(&mut self, id: &str, sandbox: &str, thread: &str, note: &str, now: u64) -> Result<Plan, String> {
        let plan = self.led(id, sandbox)?;
        if plan.state != PlanState::Approved {
            return Err(format!("plan {id} is not approved"));
        }
        let t = plan
            .threads
            .iter()
            .find(|t| t.name == thread)
            .ok_or_else(|| format!("{thread:?} is not a thread of plan {id}"))?;
        if t.state == ThreadState::Done && t.set_by.is_none() {
            return Ok(plan.clone());
        }
        // A split plan already merged still takes a late thread's done.
        match plan.merge.as_ref().map(|m| m.state) {
            Some(MergeState::Merging) => return Err(format!("plan {id} is merging")),
            Some(MergeState::Merged) if !plan.variant.is_split() => return Err(format!("plan {id} is already merged")),
            _ if plan.merged_threads().contains(thread) => return Err(format!("thread {thread:?} is already merged")),
            _ => {}
        }
        let note = clip(note);
        if note.is_empty() {
            return Err("say what you reviewed (note): it is shown to the user as the reason the thread is done".into());
        }
        self.mutate(id, now, |p| {
            if let Some(t) = p.threads.iter_mut().find(|t| t.name == thread) {
                t.state = ThreadState::Done;
                t.summary = note;
                t.set_by = Some(SetBy::Lead);
                t.updated_ms = now;
            }
            Ok(())
        })
        .inspect(|p| {
            let text = p.threads.iter().find(|t| t.name == thread).map(|t| t.summary.clone());
            self.note(id, now, EventKind::MarkedDone, team::LEAD, Some(thread), text.as_deref(), json!({ "thread": thread, "state": ThreadState::Done }));
        })
    }

    /// Forget old settled plans whose threads all finished. Returns the
    /// sandboxes whose files changed.
    pub fn prune(&mut self, now: u64, max_age_ms: u64) -> Vec<String> {
        let mut touched = Vec::new();
        self.plans.retain(|_, p| {
            let old = now.saturating_sub(p.updated_ms) > max_age_ms;
            let finished = p.state != PlanState::Approved || p.threads.iter().all(|t| t.state.is_finished());
            let keep = !(old && finished);
            if !keep {
                touched.push(p.sandbox.clone());
            }
            keep
        });
        let plans = &self.plans;
        self.logs.retain(|id, _| plans.contains_key(id));
        touched.sort();
        touched.dedup();
        touched
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    fn proposed(book: &mut PlanBook, sb: &str, names: &[&str]) -> Plan {
        book.propose(format!("p-{sb}"), sb, "s", names.iter().map(|n| t(n)).collect(), Variant::Split, "why", Some("lead-1"), 1)
            .unwrap()
    }

    #[test]
    fn the_lead_marks_a_reviewed_thread_done_and_the_plan_says_so() {
        let mut b = PlanBook::new();
        let p = b.propose("p".into(), "n", "", vec![t("a"), t("b")], Variant::Shared, "x", Some("lead"), 1).unwrap();
        b.approve(&p.id, vec![t("a"), t("b")], None, None, 2).unwrap();
        b.set_thread("n", "a", ThreadState::Done, "mine", false, 3).unwrap();
        let err = b.propose_merge("p", "n", vec![], "", vec![], 4).unwrap_err();
        assert!(err.contains("\"b\" is not done (it is starting)"), "{err}");
        assert!(b.mark_done("p", "n", "b", "  ", 5).unwrap_err().contains("say what you reviewed"));
        assert!(b.mark_done("p", "other", "b", "x", 5).is_err(), "only the plan's own sandbox");
        assert!(b.mark_done("p", "n", "c", "x", 5).is_err());
        let p = b.mark_done("p", "n", "b", "reviewed sbxw/team-p: tests pass", 6).unwrap();
        let tb = &p.threads[1];
        assert_eq!((tb.state, tb.set_by, tb.summary.as_str()), (ThreadState::Done, Some(SetBy::Lead), "reviewed sbxw/team-p: tests pass"));
        let unchanged = b.mark_done("p", "n", "a", "x", 7).unwrap();
        assert_eq!((unchanged.threads[0].summary.as_str(), unchanged.threads[0].set_by), ("mine", None), "a thread's own done stands");
        b.propose_merge("p", "n", vec![], "", vec![], 8).unwrap();
        // The thread reporting again takes its mark back.
        let p = b.set_thread("n", "b", ThreadState::Working, "still on it", false, 9).unwrap();
        assert_eq!(p.threads[1].set_by, None);
        b.end_lead("n", 10);
        assert!(b.mark_done("p", "n", "b", "x", 11).unwrap_err().contains("ended lead mode"));
    }

    #[test]
    fn a_failed_start_is_recorded_with_its_reason_and_cleared_by_a_start() {
        let mut b = PlanBook::new();
        let p = b.propose("p".into(), "n", "", vec![t("api")], Variant::Split, "x", None, 1).unwrap();
        b.approve(&p.id, vec![t("api")], None, None, 2).unwrap();
        let p = b.start_failed("n", "api", "cannot create /w/.sbxw-worktrees", 3).unwrap();
        assert_eq!((p.threads[0].state, p.threads[0].set_by), (ThreadState::Failed, Some(SetBy::Sbxw)));
        assert!(b.start_failed("n", "api", "cannot create /w/.sbxw-worktrees", 4).is_none(), "the same reason twice is no change");
        let p = b.start_cleared("n", "api", 5).unwrap();
        assert_eq!((p.threads[0].state, p.threads[0].set_by), (ThreadState::Starting, None));
        assert!(b.start_cleared("n", "api", 6).is_none());
        // A thread's own report is never cleared, and done is never undone.
        b.set_thread("n", "api", ThreadState::Done, "finished", false, 7).unwrap();
        assert!(b.start_failed("n", "api", "a later reopen failed", 8).is_none());
        assert!(b.start_failed("n", "other", "x", 8).is_none());
    }

    #[test]
    fn a_plan_starts_nothing_until_approved_and_one_pends_per_sandbox() {
        let mut b = PlanBook::new();
        let p = proposed(&mut b, "myapp", &["api"]);
        assert_eq!(p.state, PlanState::Pending);
        assert!(b.propose("p-2".into(), "myapp", "s", vec![t("x")], Variant::Split, "why", None, 2).is_err());
        assert!(b.set_thread("myapp", "api", ThreadState::Working, "", false, 3).is_err());
        b.approve(&p.id, vec![t("api")], None, None, 4).unwrap();
        b.set_thread("myapp", "api", ThreadState::Working, "on it", false, 5).unwrap();
        assert!(b.approve(&p.id, vec![t("api")], None, None, 6).is_err(), "settled once");
    }

    #[test]
    fn proposals_are_checked() {
        let mut b = PlanBook::new();
        let bad = |b: &mut PlanBook, th: Vec<PlanThread>| b.propose("p".into(), "n", "", th, Variant::Split, "why", None, 1).is_err();
        assert!(bad(&mut b, vec![]));
        assert!(bad(&mut b, vec![t("Bad")]));
        assert!(bad(&mut b, vec![t("a"), t("a")]));
        assert!(bad(&mut b, vec![PlanThread { goal: " ".into(), ..t("a") }]));
        assert!(bad(&mut b, vec![PlanThread { repo: Some("../x".into()), ..t("a") }]));
        assert!(bad(&mut b, vec![PlanThread { model: Some("Opus!".into()), ..t("a") }]));
        assert!(bad(&mut b, (0..9).map(|i| t(&format!("t{i}"))).collect()));
    }

    #[test]
    fn a_goal_over_the_cap_is_refused_not_cut() {
        let mut b = PlanBook::new();
        let mut propose = |goal: String, scope: String| b.propose("p".into(), "n", "", vec![PlanThread { goal, scope, ..t("api") }], Variant::Split, "why", None, 1);
        let long = "é".repeat(MAX_BRIEF + 1);
        let e = propose(long.clone(), String::new()).unwrap_err();
        assert!(e.contains("\"api\"") && e.contains("goal") && e.contains(&MAX_BRIEF.to_string()), "{e}");
        let e = propose("ok".into(), long).unwrap_err();
        assert!(e.contains("scope") && e.contains("\"api\""), "{e}");
        let whole = "x".repeat(MAX_BRIEF);
        let p = propose(whole.clone(), String::new()).unwrap();
        assert_eq!(p.threads[0].goal, whole, "a brief at the cap arrives whole");
    }

    #[test]
    fn other_texts_say_when_they_are_cut() {
        let c = clip(&"x".repeat(MAX_TEXT + 5));
        assert!(c.ends_with("[truncated by sbxw]"), "{c}");
    }

    #[test]
    fn a_retry_keeps_the_threads_that_merged() {
        let mut b = PlanBook::new();
        let p = proposed(&mut b, "n", &["a", "b"]);
        b.approve(&p.id, vec![t("a"), t("b")], None, None, 3).unwrap();
        b.set_thread("n", "a", ThreadState::Done, "ok", false, 4).unwrap();
        b.set_thread("n", "b", ThreadState::Done, "ok", false, 5).unwrap();
        b.propose_merge(&p.id, "n", vec!["a".into(), "b".into()], "", vec![], 6).unwrap();
        b.start_merge(&p.id, None, None, 7).unwrap();
        b.merge_result(&p.id, MergeResult { thread: "a".into(), ok: true, commit: Some("abc".into()), error: None, note: None }, 8).unwrap();
        b.merge_result(&p.id, MergeResult { thread: "b".into(), ok: false, commit: None, error: Some("conflict".into()), note: None }, 9).unwrap();
        assert_eq!(b.finish_merge(&p.id, 10).unwrap().merge.unwrap().state, MergeState::Failed);
        let m = b.start_merge(&p.id, None, None, 11).unwrap().merge.unwrap();
        assert_eq!(m.results.len(), 1, "only a's ok result is kept");
        assert_eq!(m.results[0].thread, "a");
        b.merge_result(&p.id, MergeResult { thread: "b".into(), ok: true, commit: Some("def".into()), error: None, note: None }, 12).unwrap();
        let m = b.finish_merge(&p.id, 13).unwrap().merge.unwrap();
        assert_eq!(m.state, MergeState::Merged);
        assert!(m.results.iter().all(|r| r.ok) && m.results.len() == 2);
    }

    /// Abandon merge: allowed when proposed or failed, refused while it
    /// runs, once merged, with no merge, and twice. The merge is kept as
    /// abandoned (when, and the log says the person), its results with it;
    /// Merge is refused on it; a later proposal is a fresh one, the earlier
    /// merge kept as history only when it merged something.
    #[test]
    fn abandoning_a_merge_leaves_no_merge_to_do_and_a_fresh_proposal_works() {
        let mut b = PlanBook::new();
        let p = proposed(&mut b, "n", &["a", "b"]);
        assert!(b.abandon_merge(&p.id, 2).unwrap_err().contains("not approved"));
        b.approve(&p.id, vec![t("a"), t("b")], None, None, 3).unwrap();
        assert!(b.abandon_merge(&p.id, 3).unwrap_err().contains("no merge proposed"));
        b.set_thread("n", "a", ThreadState::Done, "ok", false, 4).unwrap();
        b.set_thread("n", "b", ThreadState::Done, "ok", false, 5).unwrap();
        // Proposed, then abandoned.
        b.propose_merge(&p.id, "n", vec!["a".into(), "b".into()], "", vec![], 6).unwrap();
        let q = b.abandon_merge(&p.id, 7).unwrap();
        let m = q.merge.as_ref().unwrap();
        assert_eq!((m.state, m.abandoned_ms, m.results.len()), (MergeState::Abandoned, Some(7), 0));
        assert!(q.threads.iter().all(|t| t.state == ThreadState::Done), "no thread touched");
        assert!(b.abandon_merge(&p.id, 8).unwrap_err().contains("already abandoned"));
        assert!(b.start_merge(&p.id, None, None, 8).unwrap_err().contains("abandoned"), "Merge is refused on it");
        let log = b.log_of(&p.id);
        let e = log.entries.iter().find(|e| e.kind == EventKind::MergeAbandoned).expect("logged");
        assert_eq!((e.from.as_str(), e.to.as_deref(), e.at), (team::USER, Some(team::LEAD), 7));
        // The lead proposes again: a fresh proposal, nothing kept of the
        // abandoned one (it merged nothing).
        let q = b.propose_merge(&p.id, "n", vec!["a".into(), "b".into()], "again", vec![], 9).unwrap();
        let m = q.merge.as_ref().unwrap();
        assert_eq!((m.state, m.abandoned_ms, q.earlier_merges.len()), (MergeState::Proposed, None, 0));
        // Failed after merging a: abandoned, a stays merged; re-proposing b
        // keeps the abandoned merge as history.
        b.start_merge(&p.id, None, None, 10).unwrap();
        assert!(b.abandon_merge(&p.id, 10).unwrap_err().contains("is merging"), "refused while it runs");
        b.merge_result(&p.id, MergeResult { thread: "a".into(), ok: true, commit: Some("abc".into()), error: None, note: None }, 11).unwrap();
        b.merge_result(&p.id, MergeResult { thread: "b".into(), ok: false, commit: None, error: Some("conflict".into()), note: None }, 12).unwrap();
        b.finish_merge(&p.id, 13).unwrap();
        let q = b.abandon_merge(&p.id, 14).unwrap();
        assert_eq!(q.merge.as_ref().unwrap().state, MergeState::Abandoned);
        assert!(q.merged_threads().contains("a") && !q.merged_threads().contains("b"));
        assert!(b.propose_merge(&p.id, "n", vec!["a".into()], "", vec![], 15).unwrap_err().contains("already merged"));
        let q = b.propose_merge(&p.id, "n", vec!["b".into()], "", vec![], 15).unwrap();
        assert_eq!((q.merge.as_ref().unwrap().state, q.earlier_merges.len()), (MergeState::Proposed, 1));
        assert_eq!(q.earlier_merges[0].state, MergeState::Abandoned);
        // Merged: nothing to abandon.
        b.start_merge(&p.id, None, None, 16).unwrap();
        b.merge_result(&p.id, MergeResult { thread: "b".into(), ok: true, commit: Some("def".into()), error: None, note: None }, 17).unwrap();
        b.finish_merge(&p.id, 18).unwrap();
        assert!(b.abandon_merge(&p.id, 19).unwrap_err().contains("already merged"));
        // A plan whose merge was abandoned can be closed from the Kanban.
        let s = proposed(&mut b, "m", &["x"]);
        b.approve(&s.id, vec![t("x")], None, None, 20).unwrap();
        b.set_thread("m", "x", ThreadState::Done, "ok", false, 21).unwrap();
        b.propose_merge(&s.id, "m", vec!["x".into()], "", vec![], 22).unwrap();
        assert!(b.dismiss(&s.id, 23).is_err());
        b.abandon_merge(&s.id, 24).unwrap();
        assert!(b.dismiss(&s.id, 25).unwrap().dismissed);
    }

    #[test]
    fn a_late_thread_gets_a_merge_of_its_own_and_a_shared_plan_does_not() {
        let mut b = PlanBook::new();
        let p = proposed(&mut b, "n", &["a", "b"]);
        b.approve(&p.id, vec![t("a"), t("b")], None, None, 3).unwrap();
        b.set_thread("n", "a", ThreadState::Done, "ok", false, 4).unwrap();
        b.propose_merge(&p.id, "n", vec!["a".into()], "", vec![], 5).unwrap();
        b.start_merge(&p.id, None, None, 6).unwrap();
        b.merge_result(&p.id, MergeResult { thread: "a".into(), ok: true, commit: Some("abc".into()), error: None, note: None }, 7).unwrap();
        b.finish_merge(&p.id, 8).unwrap();
        b.mark_done(&p.id, "n", "b", "reviewed sbxw/b", 9).unwrap();
        assert!(b.propose_merge(&p.id, "n", vec!["a".into(), "b".into()], "", vec![], 10).unwrap_err().contains("already merged"));
        let q = b.propose_merge(&p.id, "n", vec!["b".into()], "", vec![], 10).unwrap();
        assert_eq!((q.merge.as_ref().unwrap().state, q.earlier_merges.len()), (MergeState::Proposed, 1));

        let s = b.propose("p-s".into(), "m", "", vec![t("x")], Variant::Shared, "why", None, 11).unwrap();
        b.approve(&s.id, vec![t("x")], None, None, 12).unwrap();
        b.set_thread("m", "x", ThreadState::Done, "ok", false, 13).unwrap();
        b.propose_merge(&s.id, "m", vec![], "", vec![], 14).unwrap();
        b.start_merge(&s.id, None, None, 15).unwrap();
        b.merge_result(&s.id, MergeResult { thread: s.team_name(), ok: true, commit: None, error: None, note: None }, 16).unwrap();
        b.finish_merge(&s.id, 17).unwrap();
        assert!(b.propose_merge(&s.id, "m", vec![], "", vec![], 18).unwrap_err().contains("already merged"), "one branch, one merge");
    }

    #[test]
    fn plans_are_scoped_to_their_sandbox() {
        let mut b = PlanBook::new();
        let p = proposed(&mut b, "myapp", &["api"]);
        assert!(b.owned(&p.id, "myapp").is_ok());
        assert!(b.owned(&p.id, "other").is_err());
    }

    #[test]
    fn a_running_thread_cannot_be_approved_twice() {
        let mut b = PlanBook::new();
        let p = proposed(&mut b, "n", &["api"]);
        b.approve(&p.id, vec![t("api")], None, None, 2).unwrap();
        let q = b.propose("p-2".into(), "n", "", vec![t("api")], Variant::Split, "why", None, 3).unwrap();
        assert!(b.approve(&q.id, vec![t("api")], None, None, 4).is_err());
    }

    #[test]
    fn a_merge_needs_a_led_approved_plan_of_done_threads() {
        let mut b = PlanBook::new();
        let p = proposed(&mut b, "n", &["a", "b"]);
        assert!(b.propose_merge(&p.id, "n", vec!["a".into()], "", vec![], 2).is_err(), "pending");
        b.approve(&p.id, vec![t("a"), t("b")], None, None, 3).unwrap();
        assert!(b.propose_merge(&p.id, "n", vec!["a".into()], "", vec![], 4).is_err(), "not done");
        b.set_thread("n", "a", ThreadState::Done, "ok", false, 5).unwrap();
        assert!(b.propose_merge(&p.id, "x", vec!["a".into()], "", vec![], 6).is_err(), "not its plan");
        assert!(b.propose_merge(&p.id, "n", vec!["a".into(), "a".into()], "", vec![], 6).is_err());
        b.propose_merge(&p.id, "n", vec!["a".into()], "go", vec![], 7).unwrap();
        b.start_merge(&p.id, Some("main".into()), None, 8).unwrap();
        assert!(b.start_merge(&p.id, None, None, 9).is_err(), "a second click");
        b.merge_result(&p.id, MergeResult { thread: "a".into(), ok: true, commit: Some("abc".into()), error: None, note: None }, 10).unwrap();
        let done = b.finish_merge(&p.id, 11).unwrap();
        assert_eq!(done.merge.unwrap().state, MergeState::Merged);
    }

    #[test]
    fn ending_lead_refuses_the_pending_and_closes_the_approved() {
        let mut b = PlanBook::new();
        let p = proposed(&mut b, "n", &["a"]);
        b.approve(&p.id, vec![t("a")], None, None, 2).unwrap();
        let q = b.propose("p-q".into(), "n", "", vec![t("b")], Variant::Split, "why", None, 3).unwrap();
        let changed = b.end_lead("n", 4);
        assert_eq!(changed.len(), 2);
        assert_eq!(b.get(&q.id).unwrap().state, PlanState::Refused);
        assert!(b.get(&p.id).unwrap().closed);
        assert!(b.led(&p.id, "n").is_err());
    }

    #[test]
    fn team_on_again_gives_back_only_the_plans_its_end_closed() {
        let mut b = PlanBook::new();
        // Open at Team off: comes back.
        let open = proposed(&mut b, "n", &["a"]);
        b.approve(&open.id, vec![t("a")], None, None, 2).unwrap();
        // Merged before Team off: stays merged and closed.
        let merged = b.propose("p-m".into(), "n", "", vec![t("m")], Variant::Split, "why", None, 3).unwrap();
        b.approve(&merged.id, vec![t("m")], None, None, 3).unwrap();
        b.set_thread("n", "m", ThreadState::Done, "ok", false, 3).unwrap();
        b.propose_merge(&merged.id, "n", vec!["m".into()], "", vec![], 3).unwrap();
        b.start_merge(&merged.id, None, None, 3).unwrap();
        b.merge_result(&merged.id, MergeResult { thread: "m".into(), ok: true, commit: None, error: None, note: None }, 3).unwrap();
        b.finish_merge(&merged.id, 3).unwrap();
        // Refused by the person: stays refused.
        let refused = b.propose("p-r".into(), "n", "", vec![t("r")], Variant::Split, "why", None, 4).unwrap();
        b.refuse(&refused.id, Some("no".into()), 4).unwrap();
        // Pending at Team off: refused by the end, stays refused.
        let pending = b.propose("p-p".into(), "n", "", vec![t("p")], Variant::Split, "why", None, 5).unwrap();
        // Another sandbox's closed plan: not this sandbox's to reopen.
        let other = b.propose("p-o".into(), "o", "", vec![t("a")], Variant::Split, "why", None, 5).unwrap();
        b.approve(&other.id, vec![t("a")], None, None, 5).unwrap();
        b.end_lead("o", 5);

        b.end_lead("n", 6);
        assert!(b.led(&open.id, "n").unwrap_err().contains("no longer yours"), "lead mode off still refuses");
        let back = b.resume_lead("n", 7);
        assert_eq!(back.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(), vec![open.id.as_str()]);
        assert!(b.led(&open.id, "n").is_ok());
        assert_eq!(b.plan_for_request("n", None).as_deref(), Some(open.id.as_str()), "read_team_notes finds it again");
        assert_eq!(b.get(&refused.id).unwrap().state, PlanState::Refused);
        assert_eq!(b.get(&pending.id).unwrap().state, PlanState::Refused);
        assert!(b.get(&other.id).unwrap().closed);
        assert!(b.resume_lead("n", 8).is_empty(), "nothing twice");
    }

    #[test]
    fn the_lead_address_is_one_printable_line() {
        assert_eq!(lead_address(Some(" lead ")), Some("lead".into()));
        assert_eq!(lead_address(Some("a\nb")), None);
        assert_eq!(lead_address(Some(&"x".repeat(101))), None);
        assert_eq!(lead_address(None), None);
    }

    #[test]
    fn old_plan_files_without_newer_fields_still_load() {
        let raw = r#"[{"id":"p-1","sandbox":"n","summary":"","threads":[{"name":"a","goal":"g"}],
                       "state":"approved","created_ms":1,"updated_ms":1}]"#;
        let plans: Vec<Plan> = serde_json::from_str(raw).unwrap();
        let b = PlanBook::load(plans);
        let p = b.get("p-1").unwrap();
        assert_eq!(p.threads[0].state, ThreadState::Starting);
        assert_eq!((p.variant, p.justification.as_str(), p.worktree.as_deref()), (Variant::Split, "", None));
        let raw = r#"{"order":["a"],"note":"","state":"merged"}"#;
        assert!(serde_json::from_str::<Merge>(raw).unwrap().distillate.is_empty());
    }

    #[test]
    fn a_proposal_says_why_its_variant() {
        let mut b = PlanBook::new();
        assert!(b.propose("p".into(), "n", "", vec![t("a")], Variant::Shared, " ", None, 1).is_err());
        let p = b.propose("p".into(), "n", "", vec![t("a")], Variant::Shared, "same files", None, 1).unwrap();
        assert_eq!(p.variant, Variant::Shared);
        let v = serde_json::to_value(&p).unwrap();
        assert_eq!((v["variant"].as_str(), v["justification"].as_str()), (Some("shared"), Some("same files")));
    }

    #[test]
    fn closing_a_shared_plans_threads_empties_and_then_ends_it() {
        let mut b = PlanBook::new();
        let p = b.propose("p-s".into(), "n", "", vec![t("a"), t("b")], Variant::Shared, "live", Some("l"), 1).unwrap();
        b.approve(&p.id, vec![t("a"), t("b")], None, Some("/w/.sbxw-worktrees/team-p-s".into()), 2).unwrap();
        assert!(b.drop_team_thread("n", "zz", 3).is_none(), "not in a shared plan");
        let p = b.drop_team_thread("n", "a", 3).unwrap();
        assert_eq!(p.threads.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(), ["b"]);
        assert_eq!(p.state, PlanState::Approved);
        assert!(b.team_of("n", "a").is_none(), "a is listed no more");
        assert!(b.team_plan("n").is_some(), "b still works on its board");
        let p = b.drop_team_thread("n", "b", 4).unwrap();
        assert_eq!(p.state, PlanState::Refused, "no thread left: the plan is over");
        assert!(b.team_plan("n").is_none(), "and its board with it");
    }

    #[test]
    fn a_shared_plan_merges_its_one_branch_once_all_threads_are_done() {
        let mut b = PlanBook::new();
        let p = b.propose("p-s".into(), "n", "", vec![t("a"), t("b")], Variant::Shared, "live", Some("l"), 1).unwrap();
        let p = b.approve(&p.id, vec![t("a"), t("b")], None, Some("/w/.sbxw-worktrees/team-p-s".into()), 2).unwrap();
        assert_eq!(p.worktree.as_deref(), Some("/w/.sbxw-worktrees/team-p-s"));
        assert_eq!(p.team_name(), "team-p-s");
        assert_eq!(b.team_of("n", "a").map(|p| p.id.as_str()), Some("p-s"));
        assert!(b.team_of("n", "zz").is_none());
        assert_eq!(b.team_plan("n").map(|p| p.id.as_str()), Some("p-s"));
        assert!(b.team_plan("other").is_none());
        b.set_thread("n", "a", ThreadState::Done, "", false, 3).unwrap();
        assert!(b.propose_merge(&p.id, "n", vec![], "", vec![], 4).is_err(), "b is not done");
        b.set_thread("n", "b", ThreadState::Done, "", false, 5).unwrap();
        let d = vec![
            DistillItem { kind: DistillKind::Decision, text: "prices in cents".into() },
            DistillItem { kind: DistillKind::Warning, text: " ".into() },
        ];
        let m = b.propose_merge(&p.id, "n", vec![], "", d, 6).unwrap().merge.unwrap();
        assert_eq!(m.order, ["a", "b"], "every thread goes in");
        assert_eq!(m.distillate.len(), 1, "the empty item is dropped");
        let kept = vec![DistillItem { kind: DistillKind::Warning, text: "no float money".into() }];
        let m = b.start_merge(&p.id, None, Some(kept.clone()), 7).unwrap().merge.unwrap();
        assert_eq!(m.distillate, kept, "the person's version wins");
        b.merge_result(&p.id, MergeResult { thread: "team-p-s".into(), ok: true, commit: None, error: None, note: None }, 8).unwrap();
        let merged = b.finish_merge(&p.id, 9).unwrap();
        assert_eq!(merged.merge.as_ref().unwrap().state, MergeState::Merged);
        let (file, body, index) = merged.distillate_memory().unwrap();
        assert_eq!(file, "team-p-s.md");
        assert!(body.starts_with("---\nname: team-p-s\n") && body.contains("- **Warning:** no float money\n"));
        assert!(index.starts_with("- [Team plan p-s](team-p-s.md) — "));
        assert!(b.team_of("n", "a").is_none(), "merged: the threads are on their own again");
        assert!(b.team_plan("n").is_none(), "merged: no board to show");
    }

    #[test]
    fn a_split_plan_has_no_shared_worktree() {
        let mut b = PlanBook::new();
        let p = proposed(&mut b, "n", &["a"]);
        let p = b.approve(&p.id, vec![t("a")], None, Some("/w".into()), 2).unwrap();
        assert_eq!(p.worktree, None);
        assert!(b.team_of("n", "a").is_none());
        assert!(b.team_plan("n").is_none());
    }

    #[test]
    fn prune_keeps_running_work() {
        let mut b = PlanBook::new();
        let p = proposed(&mut b, "n", &["a"]);
        b.approve(&p.id, vec![t("a")], None, None, 0).unwrap();
        assert!(b.prune(PRUNE_AFTER_MS * 2, PRUNE_AFTER_MS).is_empty());
        b.set_thread("n", "a", ThreadState::Failed, "", true, 0).unwrap();
        assert_eq!(b.prune(PRUNE_AFTER_MS * 2, PRUNE_AFTER_MS), ["n"]);
    }
}
