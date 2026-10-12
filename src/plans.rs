//! Plans: a lead session splits a piece of work into threads, and a person
//! approves the split before anything starts.
//!
//! The lead is the sandbox's main agent session, asked to lead with the
//! `/lead` command. It does not code. It proposes a plan — threads, each with a
//! name, a goal and a scope — and the plan waits here, `Pending`, until someone
//! looks at it in the web UI, edits it if they want, and approves or refuses
//! it. Only an approved plan starts anything: `web.rs` opens one session per
//! thread, in its own worktree, and types its goal into it.
//!
//! After approval, three things move without a click, and only inside the
//! sandbox the plan belongs to:
//!
//!  - a thread reports where it stands (`report`): working, blocked, done;
//!  - the lead is told when one of its threads becomes blocked or done;
//!  - the lead can write to a thread of one of its approved plans.
//!
//! That is the one exception to "nothing moves between agents without a
//! person" that the relay holds to, and it is bounded on purpose: same sandbox,
//! threads a person approved by name, nothing that reaches another sandbox.
//!
//! **The plan id is the capability.** The daemon cannot tell which sandbox an
//! HTTP call came from (see `relay`), so a call naming a plan is honoured only
//! because the id is 128 random bits that only the proposing session and the
//! browser ever see. Another sandbox cannot guess one, so it cannot list or
//! write to threads that are not its own. Reports carry no id — a thread only
//! knows its own name — so a forged report is possible and harmless: it changes
//! a label, never what runs.
//!
//! Unlike `relay` and `codemap`, plans are written to disk
//! (`~/.sbxw/state/plans/<sandbox>.json`) on every change and read back at
//! start: an approved plan is the map of what each thread is for, and its
//! threads' worktrees, branches and conversations outlive a daemon restart.
//! A plan id is a capability (see above), so the directory is `0700`.
//!
//! `web.rs` owns the endpoints and the sessions.

use serde::{Deserialize, Serialize};
use std::{collections::HashMap, io::Read, path::PathBuf, sync::Mutex, time::Duration};
use tokio::sync::broadcast;

/// Most threads one plan may ask for. Far above the session ceiling, which is
/// what actually limits a launch; this only stops a runaway proposal.
pub(crate) const MAX_THREADS: usize = 8;
/// Longest goal, scope or summary kept, in characters.
pub(crate) const MAX_TEXT: usize = 4000;

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Debug)]
#[serde(rename_all = "lowercase")]
pub(crate) enum PlanState {
    /// Waiting on a person.
    Pending,
    /// Approved and launched.
    Approved,
    /// Refused. Final: nothing from it ever starts.
    Refused,
}

/// Where a thread of an approved plan stands. Serialized to the browser and to
/// the lead, so the spellings are a wire contract.
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Debug, Default)]
#[serde(rename_all = "lowercase")]
pub(crate) enum ThreadState {
    /// Its session is being opened and its goal typed in.
    #[default]
    Starting,
    Working,
    /// It needs something — from the lead or from a person.
    Blocked,
    Done,
    /// Its session could not be started, or its goal not delivered.
    Failed,
}

impl ThreadState {
    /// The states a thread may report about itself. `Starting` and `Failed`
    /// are the daemon's to set.
    pub(crate) fn reportable(self) -> bool {
        matches!(self, ThreadState::Working | ThreadState::Blocked | ThreadState::Done)
    }
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub(crate) struct PlanThread {
    pub name: String,
    pub goal: String,
    /// The files or areas it owns, so two threads do not edit the same ones.
    #[serde(default)]
    pub scope: String,
    #[serde(default)]
    pub state: ThreadState,
    /// Its latest report, or why it failed.
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub updated_ms: u64,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub(crate) struct Plan {
    pub id: String,
    pub sandbox: String,
    /// What the whole plan is for, in the lead's words.
    pub summary: String,
    pub threads: Vec<PlanThread>,
    pub state: PlanState,
    /// Why it was refused, when a person said.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    pub created_ms: u64,
    pub updated_ms: u64,
}

pub(crate) struct Plans {
    plans: Mutex<HashMap<String, Plan>>,
    updates: broadcast::Sender<Plan>,
    /// Where plans are kept between runs; `None` keeps them in memory only.
    dir: Option<PathBuf>,
}

/// 128 random bits, hex. From the OS: a plan id is a capability, so it must
/// not be predictable from the time or a counter.
fn random_id() -> String {
    let mut buf = [0u8; 16];
    if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
        let _ = f.read_exact(&mut buf);
    }
    let hex: String = buf.iter().map(|b| format!("{b:02x}")).collect();
    format!("p-{hex}")
}

fn clip(s: &str) -> String {
    let s = s.trim();
    match s.char_indices().nth(MAX_TEXT) {
        Some((i, _)) => format!("{}…", &s[..i]),
        None => s.to_string(),
    }
}

/// The threads of a proposal or of an edited plan, checked: names a worktree
/// can be made from, none twice, each with a goal.
fn validate(threads: Vec<PlanThread>) -> Result<Vec<PlanThread>, String> {
    if threads.is_empty() {
        return Err("a plan needs at least one thread".into());
    }
    if threads.len() > MAX_THREADS {
        return Err(format!("a plan has at most {MAX_THREADS} threads"));
    }
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::with_capacity(threads.len());
    for t in threads {
        let name = t.name.trim().to_string();
        if !crate::is_valid_thread_name(&name) {
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
        out.push(PlanThread {
            name,
            goal,
            scope: clip(&t.scope),
            state: ThreadState::Starting,
            summary: String::new(),
            updated_ms: 0,
        });
    }
    Ok(out)
}

impl Plans {
    /// Plans in memory only.
    #[cfg(test)]
    pub(crate) fn new() -> Self {
        Self::in_dir(None)
    }

    /// Plans kept in `dir`, one file per sandbox, starting from what is there.
    /// A file that cannot be read is skipped with a warning: a lost plan costs
    /// its chips and its lead's tools, never a thread or its worktree.
    pub(crate) fn in_dir(dir: Option<PathBuf>) -> Self {
        let (updates, _) = broadcast::channel(256);
        let mut map = HashMap::new();
        if let Some(dir) = &dir {
            if let Ok(entries) = std::fs::read_dir(dir) {
                for path in entries.flatten().map(|e| e.path()) {
                    if path.extension().is_none_or(|x| x != "json") {
                        continue;
                    }
                    match std::fs::read_to_string(&path)
                        .map_err(|e| e.to_string())
                        .and_then(|t| serde_json::from_str::<Vec<Plan>>(&t).map_err(|e| e.to_string()))
                    {
                        Ok(list) => map.extend(list.into_iter().map(|p| (p.id.clone(), p))),
                        Err(e) => tracing::warn!("plans: skipping {}: {e}", path.display()),
                    }
                }
            }
        }
        Self {
            plans: Mutex::new(map),
            updates,
            dir,
        }
    }

    /// Write `sandbox`'s plans to its file, or remove the file when it has
    /// none left. Through a temporary file and a rename, so a crash mid-write
    /// leaves the previous version rather than half of this one.
    fn persist(&self, sandbox: &str) {
        let Some(dir) = &self.dir else { return };
        let mut list: Vec<Plan> = self
            .plans
            .lock()
            .unwrap()
            .values()
            .filter(|p| p.sandbox == sandbox)
            .cloned()
            .collect();
        list.sort_by_key(|p| p.created_ms);
        let path = dir.join(format!("{sandbox}.json"));
        let res = if list.is_empty() {
            match std::fs::remove_file(&path) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
                _ => Ok(()),
            }
        } else {
            (|| {
                std::fs::create_dir_all(dir)?;
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
                }
                let tmp = dir.join(format!(".{sandbox}.json.tmp"));
                std::fs::write(&tmp, serde_json::to_vec_pretty(&list)?)?;
                std::fs::rename(&tmp, &path)
            })()
        };
        if let Err(e) = res {
            tracing::warn!("plans: could not save {}: {e}", path.display());
        }
    }

    pub(crate) fn subscribe(&self) -> broadcast::Receiver<Plan> {
        self.updates.subscribe()
    }

    /// Save and broadcast a change. Every change comes through here.
    fn announce(&self, plan: &Plan) {
        self.persist(&plan.sandbox);
        let _ = self.updates.send(plan.clone());
    }

    /// Every plan, oldest first — what a browser tab seeds itself from.
    pub(crate) fn list(&self) -> Vec<Plan> {
        let mut out: Vec<Plan> = self.plans.lock().unwrap().values().cloned().collect();
        out.sort_by(|a, b| a.created_ms.cmp(&b.created_ms).then(a.id.cmp(&b.id)));
        out
    }

    pub(crate) fn get(&self, id: &str) -> Option<Plan> {
        self.plans.lock().unwrap().get(id).cloned()
    }

    /// The plan `id`, if it belongs to `sandbox` — how a call from a sandbox
    /// is checked. Not finding it and it being someone else's read the same.
    pub(crate) fn owned(&self, id: &str, sandbox: &str) -> Result<Plan, String> {
        self.get(id)
            .filter(|p| p.sandbox == sandbox)
            .ok_or_else(|| format!("no plan '{id}' for '{sandbox}'"))
    }

    /// A lead proposes a plan. One pending plan per sandbox: a second would
    /// put two dialogs about the same work in front of a person.
    pub(crate) fn propose(
        &self,
        sandbox: &str,
        summary: &str,
        threads: Vec<PlanThread>,
        now: u64,
    ) -> Result<Plan, String> {
        let threads = validate(threads)?;
        let plan = {
            let mut map = self.plans.lock().unwrap();
            if let Some(p) = map
                .values()
                .find(|p| p.sandbox == sandbox && p.state == PlanState::Pending)
            {
                return Err(format!(
                    "plan {} is still waiting for the user; wait for it with check_plan",
                    p.id
                ));
            }
            let plan = Plan {
                id: random_id(),
                sandbox: sandbox.to_string(),
                summary: clip(summary),
                threads,
                state: PlanState::Pending,
                note: None,
                created_ms: now,
                updated_ms: now,
            };
            map.insert(plan.id.clone(), plan.clone());
            plan
        };
        self.announce(&plan);
        Ok(plan)
    }

    fn mutate<F>(&self, id: &str, now: u64, f: F) -> Result<Plan, String>
    where
        F: FnOnce(&mut Plan) -> Result<(), String>,
    {
        let plan = {
            let mut map = self.plans.lock().unwrap();
            let plan = map.get_mut(id).ok_or_else(|| format!("no plan '{id}'"))?;
            f(plan)?;
            plan.updated_ms = now;
            plan.clone()
        };
        self.announce(&plan);
        Ok(plan)
    }

    /// The threads a person approved, as they left them in the dialog. A thread
    /// already approved in another plan of the same sandbox is refused: two
    /// plans writing to one worktree is the thing worktrees exist to prevent.
    /// The launch is the caller's; this only records the decision.
    pub(crate) fn approve(
        &self,
        id: &str,
        threads: Vec<PlanThread>,
        now: u64,
    ) -> Result<Plan, String> {
        let threads = validate(threads)?;
        let sandbox = self.get(id).map(|p| p.sandbox).unwrap_or_default();
        let taken: Vec<String> = self
            .approved_threads(&sandbox)
            .into_iter()
            .filter(|(_, t)| t.state != ThreadState::Done && t.state != ThreadState::Failed)
            .map(|(_, t)| t.name)
            .collect();
        if let Some(t) = threads.iter().find(|t| taken.contains(&t.name)) {
            return Err(format!(
                "thread {:?} already belongs to an approved plan that is still running",
                t.name
            ));
        }
        self.mutate(id, now, |p| {
            if p.state != PlanState::Pending {
                return Err(format!("plan {id} was already settled"));
            }
            p.threads = threads;
            for t in &mut p.threads {
                t.updated_ms = now;
            }
            p.state = PlanState::Approved;
            Ok(())
        })
    }

    pub(crate) fn refuse(&self, id: &str, note: Option<String>, now: u64) -> Result<Plan, String> {
        self.mutate(id, now, |p| {
            if p.state != PlanState::Pending {
                return Err(format!("plan {id} was already settled"));
            }
            p.state = PlanState::Refused;
            p.note = note.map(|n| clip(&n)).filter(|n| !n.is_empty());
            Ok(())
        })
    }

    /// Every thread of every approved plan of `sandbox`, with its plan's id.
    pub(crate) fn approved_threads(&self, sandbox: &str) -> Vec<(String, PlanThread)> {
        self.plans
            .lock()
            .unwrap()
            .values()
            .filter(|p| p.sandbox == sandbox && p.state == PlanState::Approved)
            .flat_map(|p| p.threads.iter().map(|t| (p.id.clone(), t.clone())))
            .collect()
    }

    /// Set where a thread stands. `by_daemon` may set any state; a thread's own
    /// report only the reportable ones. The latest approved plan holding the
    /// name wins, which is the one a re-proposed thread is running under.
    pub(crate) fn set_thread(
        &self,
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
        let id = {
            let map = self.plans.lock().unwrap();
            map.values()
                .filter(|p| p.sandbox == sandbox && p.state == PlanState::Approved)
                .filter(|p| p.threads.iter().any(|t| t.name == thread))
                .max_by_key(|p| p.created_ms)
                .map(|p| p.id.clone())
                .ok_or_else(|| format!("'{thread}' is not a thread of an approved plan in '{sandbox}'"))?
        };
        self.mutate(&id, now, |p| {
            if let Some(t) = p.threads.iter_mut().find(|t| t.name == thread) {
                t.state = state;
                t.summary = clip(summary);
                t.updated_ms = now;
            }
            Ok(())
        })
    }

    /// Park until plan `id` is settled or `timeout` passes, then return it as
    /// it stands. Subscribe before looking, as `relay::wait` does, so a
    /// decision made in between is not missed.
    pub(crate) async fn wait(&self, id: &str, sandbox: &str, timeout: Duration) -> Result<Plan, String> {
        let mut rx = self.subscribe();
        let mut latest = self.owned(id, sandbox)?;
        let deadline = tokio::time::Instant::now() + timeout;
        while latest.state == PlanState::Pending {
            let left = deadline.saturating_duration_since(tokio::time::Instant::now());
            if left.is_zero() {
                break;
            }
            match tokio::time::timeout(left, rx.recv()).await {
                Ok(Ok(p)) if p.id == id => latest = p,
                Ok(Ok(_)) => {}
                Ok(Err(broadcast::error::RecvError::Lagged(_))) => latest = self.owned(id, sandbox)?,
                Ok(Err(broadcast::error::RecvError::Closed)) | Err(_) => break,
            }
        }
        Ok(latest)
    }

    /// Forget settled plans older than `max_age_ms` whose threads have all
    /// finished, and pending ones nobody looked at for as long.
    pub(crate) fn prune(&self, now: u64, max_age_ms: u64) -> usize {
        let mut dropped: Vec<String> = Vec::new();
        {
            let mut map = self.plans.lock().unwrap();
            map.retain(|_, p| {
                let old = now.saturating_sub(p.updated_ms) > max_age_ms;
                let finished = p.state != PlanState::Approved
                    || p.threads
                        .iter()
                        .all(|t| matches!(t.state, ThreadState::Done | ThreadState::Failed));
                let keep = !(old && finished);
                if !keep {
                    dropped.push(p.sandbox.clone());
                }
                keep
            });
        }
        let n = dropped.len();
        dropped.sort();
        dropped.dedup();
        for sandbox in dropped {
            self.persist(&sandbox);
        }
        n
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(name: &str, goal: &str) -> PlanThread {
        PlanThread {
            name: name.into(),
            goal: goal.into(),
            scope: String::new(),
            state: ThreadState::Working,
            summary: "ignored".into(),
            updated_ms: 0,
        }
    }

    #[test]
    fn a_plan_starts_nothing_until_a_person_approves_it() {
        let plans = Plans::new();
        let p = plans.propose("neos", "split the login work", vec![t("api", "the endpoint")], 1).unwrap();
        assert_eq!(p.state, PlanState::Pending);
        assert!(plans.approved_threads("neos").is_empty());
        // A thread of a plan nobody approved cannot report itself into one.
        assert!(plans.set_thread("neos", "api", ThreadState::Done, "", false, 2).is_err());

        let p = plans.approve(&p.id, vec![t("api", "the endpoint, edited")], 3).unwrap();
        assert_eq!(p.state, PlanState::Approved);
        assert_eq!(p.threads[0].goal, "the endpoint, edited");
        // What a proposal claimed about state is never trusted.
        assert_eq!(p.threads[0].state, ThreadState::Starting);
        assert!(plans.approve(&p.id, vec![t("api", "again")], 4).is_err(), "settled is final");
    }

    #[test]
    fn plan_ids_are_unguessable_and_scoped_to_their_sandbox() {
        let plans = Plans::new();
        let a = plans.propose("neos", "", vec![t("a", "g")], 1).unwrap();
        let b = plans.propose("other", "", vec![t("a", "g")], 1).unwrap();
        assert_ne!(a.id, b.id);
        assert_eq!(a.id.len(), 2 + 32, "{}", a.id);
        assert!(plans.owned(&a.id, "neos").is_ok());
        assert!(plans.owned(&a.id, "other").is_err());
    }

    #[test]
    fn a_proposal_is_checked_before_anyone_sees_it() {
        let plans = Plans::new();
        for bad in [
            vec![],
            vec![t("Bad", "g")],
            vec![t("a", "g"), t("a", "g")],
            vec![t("a", "  ")],
        ] {
            assert!(plans.propose("neos", "", bad, 1).is_err());
        }
        plans.propose("neos", "", vec![t("a", "g")], 1).unwrap();
        assert!(
            plans.propose("neos", "", vec![t("b", "g")], 2).is_err(),
            "one pending plan per sandbox"
        );
    }

    #[test]
    fn a_running_thread_cannot_be_approved_twice() {
        let plans = Plans::new();
        let p = plans.propose("neos", "", vec![t("a", "g")], 1).unwrap();
        plans.approve(&p.id, vec![t("a", "g")], 2).unwrap();
        let q = plans.propose("neos", "", vec![t("a", "g2")], 3).unwrap();
        assert!(plans.approve(&q.id, vec![t("a", "g2")], 4).is_err());
        // Once it is done, the name is free again.
        plans.set_thread("neos", "a", ThreadState::Done, "shipped", false, 5).unwrap();
        plans.approve(&q.id, vec![t("a", "g2")], 6).unwrap();
    }

    #[test]
    fn plans_survive_a_restart_and_a_finished_one_leaves_no_file() {
        let dir = std::env::temp_dir().join(format!("sbxw-plans-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let id = {
            let plans = Plans::in_dir(Some(dir.clone()));
            let p = plans.propose("neos", "split", vec![t("a", "g")], 1).unwrap();
            plans.approve(&p.id, vec![t("a", "g")], 2).unwrap();
            plans.set_thread("neos", "a", ThreadState::Blocked, "which table?", false, 3).unwrap();
            p.id
        };
        let again = Plans::in_dir(Some(dir.clone()));
        let p = again.owned(&id, "neos").unwrap();
        assert_eq!(p.state, PlanState::Approved);
        assert_eq!(p.threads[0].state, ThreadState::Blocked);
        assert_eq!(p.threads[0].summary, "which table?");

        again.set_thread("neos", "a", ThreadState::Done, "", false, 4).unwrap();
        assert_eq!(again.prune(10_000, 1), 1);
        assert!(!dir.join("neos.json").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_thread_reports_only_what_a_thread_can_know() {
        let plans = Plans::new();
        let p = plans.propose("neos", "", vec![t("a", "g")], 1).unwrap();
        plans.approve(&p.id, vec![t("a", "g")], 2).unwrap();
        assert!(plans.set_thread("neos", "a", ThreadState::Failed, "", false, 3).is_err());
        assert!(plans.set_thread("neos", "a", ThreadState::Starting, "", false, 3).is_err());
        let p = plans.set_thread("neos", "a", ThreadState::Blocked, "which table?", false, 3).unwrap();
        assert_eq!(p.threads[0].state, ThreadState::Blocked);
        assert_eq!(p.threads[0].summary, "which table?");
        plans.set_thread("neos", "a", ThreadState::Failed, "no session", true, 4).unwrap();
    }
}
