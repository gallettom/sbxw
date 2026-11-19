//! Teams for the UIs: a plan with its sandbox, lead and threads, and its
//! timeline (`sbxw_core::team`). Read-only: nothing here acts on a plan.
//!
//! The plan book logs its own transitions; this file logs the relay
//! requests a team opened, persists the logs beside the plans (the same
//! 0700 directory, pruned with them), reads the team's commits from git,
//! and tells a listening UI when a timeline grew.

use crate::app::{now_ms, App};
use crate::bus::Event;
use crate::store::write_atomic;
use sbxw_core::plans::{MergeState, Plan, Variant};
use sbxw_core::relay::RelayRequest;
use sbxw_core::team::{self, Filter, TeamCommit};
use sbxw_core::session::{SessionKey, SessionKind};
use sbxw_core::{SandboxRef, ThreadName};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// Commits read for a team stay this long before git is asked again.
const COMMITS_FRESH: Duration = Duration::from_secs(10);
/// Commits per range, and in a timeline.
const COMMITS_PER_RANGE: usize = 300;
const COMMITS_MAX: usize = 1000;

type CommitCache = Mutex<HashMap<String, (Instant, Result<Vec<TeamCommit>, String>)>>;

fn commit_cache() -> &'static CommitCache {
    static C: OnceLock<CommitCache> = OnceLock::new();
    C.get_or_init(Default::default)
}

impl App {
    /// A team's timeline grew: a UI listening re-reads it. Nobody listening,
    /// nothing is sent.
    pub(crate) fn team_changed(&self, plan: &str) {
        if *self.bus.clients.borrow() > 0 {
            self.send(Event::Team(plan.to_string()));
        }
    }

    /// `plans/team/<sandbox>.json`: the sandbox's plans' logs (removed with
    /// the last of them).
    pub(crate) fn persist_team_logs(&self, sandbox: &str) {
        let logs = self.plans.lock().unwrap().logs_of(sandbox);
        let dir = self.store.paths.plans().join("team");
        let path = dir.join(format!("{sandbox}.json"));
        let res = if logs.is_empty() {
            match std::fs::remove_file(&path) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
                _ => Ok(()),
            }
        } else {
            std::fs::create_dir_all(&dir)
                .map_err(anyhow::Error::from)
                .and_then(|_| serde_json::to_vec(&logs).map_err(anyhow::Error::from))
                .and_then(|b| write_atomic(&path, &b))
        };
        if let Err(e) = res {
            tracing::warn!("team logs: could not save {}: {e:#}", path.display());
        }
    }

    /// A relay request moved: logged on the plan of the sandbox (and thread)
    /// that opened it, if it has one. Its text only per
    /// [`team::KEEP_RELAY_TEXT`]; never an image.
    pub(crate) fn relay_noted(&self, req: &RelayRequest) {
        let Some((kind, from, to, text)) = team::relay_step(req) else { return };
        let id = {
            let mut plans = self.plans.lock().unwrap();
            let Some(id) = plans.plan_for_request(&req.from, req.from_thread.as_deref()) else { return };
            if plans.log_of(&id).has_relay(&req.id, req.created_ms, kind) {
                return;
            }
            plans.note(&id, req.updated_ms, kind, &from, to.as_deref(), text.as_deref(), team::relay_data(req));
            id
        };
        let sandbox = self.plan(&id).map(|p| p.sandbox).unwrap_or_default();
        self.persist_team_logs(&sandbox);
        self.team_changed(&id);
    }

    /// Every team, the latest active first.
    pub fn teams(&self) -> Vec<Value> {
        let plans = self.plans_list();
        let mut rows: Vec<(u64, Value)> = {
            let book = self.plans.lock().unwrap();
            plans
                .iter()
                .map(|p| {
                    let last = book.log_of(&p.id).entries.last().map(|e| e.at);
                    let row = team::team_row(p, last);
                    (row["last_ms"].as_u64().unwrap_or(0), row)
                })
                .collect()
        };
        rows.sort_by_key(|(t, _)| std::cmp::Reverse(*t));
        rows.into_iter().map(|(_, r)| r).collect()
    }

    /// The ranges of the team's commits: each thread's branch (or, merged,
    /// its recorded merge commit's other side), or a shared plan's one branch.
    fn commit_ranges(plan: &Plan) -> Vec<(Option<String>, String, bool)> {
        let merged_by: HashMap<&str, &str> = plan
            .merge
            .iter()
            .flat_map(|m| m.results.iter())
            .filter(|r| r.ok)
            .filter_map(|r| r.commit.as_deref().map(|c| (r.thread.as_str(), c)))
            .collect();
        let range = |name: &str, thread: Option<String>| match merged_by.get(name) {
            Some(c) => (thread, format!("{c}^1..{c}^2"), true),
            None => (thread, format!("HEAD..sbxw/{name}"), false),
        };
        match plan.variant {
            Variant::Split => plan.threads.iter().map(|t| range(&t.name, Some(t.name.clone()))).collect(),
            Variant::Shared => vec![range(&plan.team_name(), None)],
        }
    }

    async fn team_commits(&self, plan: &Plan, board: &[sbxw_core::board::Entry]) -> Result<Vec<TeamCommit>, String> {
        let fresh = {
            let c = commit_cache().lock().unwrap();
            c.get(&plan.id).filter(|(t, _)| t.elapsed() < COMMITS_FRESH).map(|(_, v)| v.clone())
        };
        if let Some(v) = fresh {
            return v;
        }
        let read = async {
            let sb = SandboxRef::parse(&plan.sandbox).map_err(|e| e.to_string())?;
            let ws = self.store.workspace(&sb).ok_or_else(|| format!("sbxw has no workspace recorded for '{sb}'"))?;
            let host = self.host_for(&sb).await.map_err(|e| format!("the host of '{sb}' cannot be reached: {e:#}"))?;
            let rows = crate::git::range_commits(&host, &ws, &Self::commit_ranges(plan), COMMITS_PER_RANGE).await?;
            // A shared branch's commits are the threads': the board says whose.
            let by: HashMap<&str, &str> = board
                .iter()
                .filter(|e| e.kind == sbxw_core::board::Kind::Commit)
                .filter_map(|e| e.hash.as_deref().map(|h| (h, e.by.as_str())))
                .collect();
            Ok(rows
                .into_iter()
                .take(COMMITS_MAX)
                .map(|r| {
                    let thread = r.thread.clone().or_else(|| {
                        by.iter().find(|(h, _)| r.hash.starts_with(**h) || h.starts_with(r.hash.as_str())).map(|(_, b)| b.to_string())
                    });
                    TeamCommit {
                        hash: r.hash,
                        short: r.short,
                        subject: r.subject,
                        author: r.author,
                        at: r.at,
                        thread,
                        branch: r.branch,
                        merged: r.merged,
                        files: r.files,
                        added: r.added,
                        removed: r.removed,
                        repo: r.repo,
                    }
                })
                .collect())
        };
        let v: Result<Vec<TeamCommit>, String> = read.await;
        commit_cache().lock().unwrap().insert(plan.id.clone(), (Instant::now(), v.clone()));
        v
    }

    /// A page of team `id`'s timeline, newest first, with the team, the
    /// board's decisions and warnings that still stand, and (on the first
    /// page) its commits per thread.
    pub async fn team_timeline(&self, id: &str, filter: &Filter<'_>, before: Option<&str>, limit: usize) -> Result<Value, String> {
        let plan = self.plan(id).ok_or_else(|| format!("no team '{id}'"))?;
        let log = self.plans.lock().unwrap().log_of(id);
        let board = self.with_board_read(&plan.sandbox).unwrap_or_default();
        let relay = self.relay_list();
        let commits = self.team_commits(&plan, &board).await;
        let now = now_ms();
        let all = team::timeline(&plan, &log, &board, &relay, commits.as_deref().unwrap_or(&[]), now);
        let (events, more) = team::page(&all, filter, before, limit);
        let mut out = json!({
            "team": team::team_row(&plan, log.entries.last().map(|e| e.at)),
            "events": events,
            "more": more,
            "standing": team::standing_notes(&plan, &board, &log, now),
            "relay_text_kept": team::KEEP_RELAY_TEXT,
        });
        if before.is_none() {
            let list = commits.as_deref().unwrap_or(&[]);
            let mut totals: std::collections::BTreeMap<String, (u64, u64, u64, u64)> = Default::default();
            for c in list {
                let t = totals.entry(c.thread.clone().unwrap_or_else(|| "team".into())).or_default();
                *t = (t.0 + 1, t.1 + c.files as u64, t.2 + c.added, t.3 + c.removed);
            }
            let totals: Vec<Value> = totals
                .into_iter()
                .map(|(t, (n, f, a, r))| json!({ "thread": t, "commits": n, "files": f, "added": a, "removed": r }))
                .collect();
            let merged = plan.merge.as_ref().is_some_and(|m| m.state == MergeState::Merged);
            out["commits"] = json!({
                "available": commits.is_ok(), "why": commits.as_ref().err(), "list": list, "totals": totals, "merged": merged,
            });
        }
        Ok(out)
    }
}

/// The last team note each reader was handed, by (plan, reader). In memory:
/// after a restart a reader is handed its notes again once.
type NoteCursors = Mutex<HashMap<(String, String), u64>>;

fn note_cursors() -> &'static NoteCursors {
    static C: OnceLock<NoteCursors> = OnceLock::new();
    C.get_or_init(Default::default)
}

impl App {
    /// `team_note`: an agent of a team (`thread`, or the lead) writes to
    /// `to` (`lead`, a thread of its plan, or `team`). Recorded on the
    /// team's timeline, then each addressee whose session is open is typed
    /// one line of sbxw's own that says a note waits (never the note's
    /// words), as a relay question is announced; it reads the note with
    /// `read_team_notes`. Returns the note's line on the log and who was told.
    pub fn team_note(self: &std::sync::Arc<Self>, sandbox: &SandboxRef, thread: Option<&ThreadName>, to: &str, kind: &str, text: &str) -> Result<(u64, Vec<String>), String> {
        if !team::NOTE_KINDS.contains(&kind) {
            return Err(format!("kind is one of {}", team::NOTE_KINDS.join(", ")));
        }
        let text = sbxw_core::messages::clip_text(text, team::MAX_TEXT);
        if text.is_empty() {
            return Err("a note needs text".into());
        }
        let me = thread.map_or(team::LEAD.to_string(), |t| t.to_string());
        let to = to.trim();
        let (id, seq, sessions) = {
            let mut plans = self.plans.lock().unwrap();
            let id = plans
                .plan_for_request(&sandbox.to_string(), thread.map(|t| t.as_str()))
                .ok_or("you are in no team: team_note is for a lead and its threads")?;
            let plan = plans.get(&id).cloned().ok_or("no plan")?;
            let names: Vec<String> = plan.threads.iter().map(|t| t.name.clone()).collect();
            let valid = to == "team" || to == team::LEAD || names.iter().any(|n| n == to);
            if !valid || to == me {
                return Err(format!("to is \"lead\", \"team\" or a thread of your plan ({}), not yourself", names.join(", ")));
            }
            plans.note(&id, now_ms(), team::EventKind::TeamNote, &me, Some(to), Some(&text), json!({ "kind": kind }));
            let seq = plans.log_of(&id).next;
            let mut who: Vec<String> = match to {
                "team" => std::iter::once(team::LEAD.to_string()).chain(names).collect(),
                t => vec![t.to_string()],
            };
            who.retain(|w| w != &me);
            (id, seq, who)
        };
        self.persist_team_logs(&sandbox.to_string());
        self.team_changed(&id);
        let mut told = vec![];
        for w in sessions {
            let kind_of = if w == team::LEAD { Some(SessionKind::Agent) } else { ThreadName::parse(&w).ok().map(SessionKind::Thread) };
            let Some(k) = kind_of else { continue };
            let key = match &k {
                SessionKind::Thread(t) => SessionKey::thread(sandbox, t),
                _ => SessionKey::agent(sandbox),
            };
            if !self.sessions.is_live(&key) {
                continue;
            }
            let line = format!("[sbxw team note · {seq}] A {kind} from {me} for you: read it with the read_team_notes tool (MCP sbxw-relay).");
            self.spawn_push(sandbox.clone(), k, line, None);
            told.push(w);
        }
        tracing::info!("team: {me} of '{sandbox}' wrote note {seq} ({kind}) to {to}");
        Ok((seq, told))
    }

    /// `read_team_notes`: the notes for the caller not handed yet, oldest
    /// first, each `{from, kind, text, at, to}`.
    pub fn team_notes_read(&self, sandbox: &SandboxRef, thread: Option<&ThreadName>) -> Result<Vec<Value>, String> {
        let me = thread.map_or(team::LEAD.to_string(), |t| t.to_string());
        let plans = self.plans.lock().unwrap();
        let id = plans.plan_for_request(&sandbox.to_string(), thread.map(|t| t.as_str())).ok_or("you are in no team")?;
        let log = plans.log_of(&id);
        let mut cur = note_cursors().lock().unwrap();
        let after = cur.get(&(id.clone(), me.clone())).copied().unwrap_or(0);
        let notes = team::notes_for(&log, &me, after);
        if let Some(last) = notes.last() {
            cur.insert((id.clone(), me.clone()), last.seq);
        }
        Ok(notes.iter().map(|e| json!({ "from": e.from, "to": e.to, "kind": e.data["kind"], "text": e.text, "at": e.at })).collect())
    }
}

#[cfg(test)]
mod tests;
