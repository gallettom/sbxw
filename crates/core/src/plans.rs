//! Plans: a lead session splits work into threads, and a person approves the
//! split before anything starts.
//!
//! The lead is the sandbox's main agent after `/lead`. It proposes threads —
//! name, goal, scope, optionally repo and model — and the plan waits here,
//! `Pending`, until someone approves (maybe after editing) or refuses it in the
//! web UI. Only an approved plan starts anything. Threads then report working,
//! blocked or done; once they are done the lead may propose a merge, which a
//! person clicks.
//!
//! The conversation between the lead and its threads is Claude Code's own
//! `SendMessage`, by session name, inside one sandbox; nothing here reaches
//! another sandbox. The book is pure: the daemon persists it, broadcasts it,
//! and launches the sessions.

use crate::names::{is_valid_model_id, is_valid_thread_name, normalize_repo_path};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

pub const MAX_THREADS: usize = 8;
pub const MAX_TEXT: usize = 4000;
/// Settled plans whose threads all finished are dropped this long after their
/// last change.
pub const PRUNE_AFTER_MS: u64 = 24 * 3600 * 1000;

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Debug)]
#[serde(rename_all = "lowercase")]
pub enum PlanState {
    Pending,
    Approved,
    Refused,
}

/// How a plan's threads share the code (Team mode). Chosen when the plan is
/// proposed, maybe changed in the approval dialog, fixed once approved.
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

/// What the lead proposes to keep once a shared plan is merged: what still
/// holds, written to the sandbox's Claude Code auto memory.
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
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Debug)]
#[serde(rename_all = "lowercase")]
pub enum MergeState {
    Proposed,
    Merging,
    Merged,
    Failed,
}

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq, Eq)]
pub struct MergeResult {
    pub thread: String,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub merge: Option<Merge>,
    pub created_ms: u64,
    pub updated_ms: u64,
}

impl Plan {
    /// A shared plan's worktree folder under `.sbxw-worktrees/`, and its
    /// branch after `sbxw/`: `team-<plan id>`, a valid thread name, so the
    /// thread scripts handle it as one.
    pub fn team_name(&self) -> String {
        team_name(&self.id)
    }
}

impl Plan {
    /// The approved distillate as one Claude Code auto memory: its file name,
    /// its contents, and its line for the `MEMORY.md` index. `None` when
    /// nothing was kept.
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
        Some((i, _)) => format!("{}…", &s[..i]),
        None => s.to_string(),
    }
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
            let goal = clip(&t.goal);
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
                scope: clip(&t.scope),
                repo,
                model,
                state: ThreadState::Starting,
                summary: String::new(),
                updated_ms: 0,
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
}

impl PlanBook {
    pub fn new() -> Self {
        Self::default()
    }

    /// Start from plans read back from disk.
    pub fn load(plans: impl IntoIterator<Item = Plan>) -> Self {
        Self {
            plans: plans.into_iter().map(|p| (p.id.clone(), p)).collect(),
        }
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
            return Err(format!(
                "the user ended lead mode, so plan {id} is no longer yours to follow. \
                 Stop leading: no new plan and no follow-up of its threads."
            ));
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
            merge: None,
            created_ms: now,
            updated_ms: now,
        };
        self.plans.insert(id, plan.clone());
        Ok(plan)
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

    /// The threads a person approved, as left in the dialog. A thread still
    /// running under another approved plan of the sandbox is refused: two plans
    /// writing to one worktree is what worktrees exist to prevent.
    /// `variant`, when given, is the dialog's choice; the proposal's otherwise.
    /// A shared plan's worktree is recorded with it.
    pub fn approve(
        &mut self,
        id: &str,
        threads: Vec<PlanThread>,
        variant: Option<Variant>,
        worktree: Option<String>,
        now: u64,
    ) -> Result<Plan, String> {
        let threads = validate(threads)?;
        let sandbox = self.plans.get(id).map(|p| p.sandbox.clone()).unwrap_or_default();
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
    }

    pub fn refuse(&mut self, id: &str, note: Option<String>, now: u64) -> Result<Plan, String> {
        self.mutate_pending(id, now, |p| {
            p.state = PlanState::Refused;
            p.note = note.map(|n| clip(&n)).filter(|n| !n.is_empty());
            Ok(())
        })
    }

    /// Team mode ended: the pending plan is refused, approved ones closed.
    pub fn end_lead(&mut self, sandbox: &str, now: u64) -> Vec<Plan> {
        self.plans
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
            .collect()
    }

    /// The lead proposes merging finished threads, in order. A shared plan has
    /// one branch: every thread is merged at once, so all must be done, and an
    /// empty order means all of them.
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
        if let Some(m) = &plan.merge {
            if matches!(m.state, MergeState::Merging | MergeState::Merged) {
                return Err(format!("plan {id} is already {:?}", m.state).to_lowercase());
            }
        }
        self.mutate(id, now, |p| {
            p.merge = Some(Merge {
                order,
                note: clip(note),
                state: MergeState::Proposed,
                base: None,
                results: vec![],
                distillate,
            });
            Ok(())
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
            m.results.clear();
            if let Some(d) = distillate {
                m.distillate = d;
            }
            Ok(())
        })
    }

    pub fn merge_result(&mut self, id: &str, result: MergeResult, now: u64) -> Result<Plan, String> {
        self.mutate(id, now, |p| {
            p.merge
                .as_mut()
                .ok_or_else(|| format!("plan {id} has no merge"))?
                .results
                .push(result);
            Ok(())
        })
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
    }

    pub fn approved_threads(&self, sandbox: &str) -> Vec<(String, PlanThread)> {
        self.plans
            .values()
            .filter(|p| p.sandbox == sandbox && p.state == PlanState::Approved)
            .flat_map(|p| p.threads.iter().map(|t| (p.id.clone(), t.clone())))
            .collect()
    }

    /// The shared plan `thread` works under, if it does: the latest approved
    /// plan holding its name, when that plan is shared and not merged yet. Its
    /// sessions open in the plan's worktree rather than one of their own.
    pub fn team_of(&self, sandbox: &str, thread: &str) -> Option<&Plan> {
        self.plans
            .values()
            .filter(|p| p.sandbox == sandbox && p.state == PlanState::Approved)
            .filter(|p| p.threads.iter().any(|t| t.name == thread))
            .max_by_key(|p| p.created_ms)
            .filter(|p| p.variant == Variant::Shared)
            .filter(|p| !p.merge.as_ref().is_some_and(|m| m.state == MergeState::Merged))
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
            }
            Ok(())
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
        }
    }

    fn proposed(book: &mut PlanBook, sb: &str, names: &[&str]) -> Plan {
        book.propose(format!("p-{sb}"), sb, "s", names.iter().map(|n| t(n)).collect(), Variant::Split, "why", Some("lead-1"), 1)
            .unwrap()
    }

    #[test]
    fn a_plan_starts_nothing_until_approved_and_one_pends_per_sandbox() {
        let mut b = PlanBook::new();
        let p = proposed(&mut b, "neos", &["api"]);
        assert_eq!(p.state, PlanState::Pending);
        assert!(b.propose("p-2".into(), "neos", "s", vec![t("x")], Variant::Split, "why", None, 2).is_err());
        assert!(b.set_thread("neos", "api", ThreadState::Working, "", false, 3).is_err());
        b.approve(&p.id, vec![t("api")], None, None, 4).unwrap();
        b.set_thread("neos", "api", ThreadState::Working, "on it", false, 5).unwrap();
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
    fn plans_are_scoped_to_their_sandbox() {
        let mut b = PlanBook::new();
        let p = proposed(&mut b, "neos", &["api"]);
        assert!(b.owned(&p.id, "neos").is_ok());
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
        b.merge_result(&p.id, MergeResult { thread: "a".into(), ok: true, commit: Some("abc".into()), error: None }, 10).unwrap();
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
    fn a_shared_plan_merges_its_one_branch_once_all_threads_are_done() {
        let mut b = PlanBook::new();
        let p = b.propose("p-s".into(), "n", "", vec![t("a"), t("b")], Variant::Shared, "live", Some("l"), 1).unwrap();
        let p = b.approve(&p.id, vec![t("a"), t("b")], None, Some("/w/.sbxw-worktrees/team-p-s".into()), 2).unwrap();
        assert_eq!(p.worktree.as_deref(), Some("/w/.sbxw-worktrees/team-p-s"));
        assert_eq!(p.team_name(), "team-p-s");
        assert_eq!(b.team_of("n", "a").map(|p| p.id.as_str()), Some("p-s"));
        assert!(b.team_of("n", "zz").is_none());
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
        b.merge_result(&p.id, MergeResult { thread: "team-p-s".into(), ok: true, commit: None, error: None }, 8).unwrap();
        let merged = b.finish_merge(&p.id, 9).unwrap();
        assert_eq!(merged.merge.as_ref().unwrap().state, MergeState::Merged);
        let (file, body, index) = merged.distillate_memory().unwrap();
        assert_eq!(file, "team-p-s.md");
        assert!(body.starts_with("---\nname: team-p-s\n") && body.contains("- **Warning:** no float money\n"));
        assert!(index.starts_with("- [Team plan p-s](team-p-s.md) — "));
        assert!(b.team_of("n", "a").is_none(), "merged: the threads are on their own again");
    }

    #[test]
    fn a_split_plan_has_no_shared_worktree() {
        let mut b = PlanBook::new();
        let p = proposed(&mut b, "n", &["a"]);
        let p = b.approve(&p.id, vec![t("a")], None, Some("/w".into()), 2).unwrap();
        assert_eq!(p.worktree, None);
        assert!(b.team_of("n", "a").is_none());
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
