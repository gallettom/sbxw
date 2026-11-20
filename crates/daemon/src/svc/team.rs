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
use sbxw_core::agents::SessionState;
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

fn session_kind(who: &str) -> Option<SessionKind> {
    if who == team::LEAD {
        Some(SessionKind::Agent)
    } else {
        ThreadName::parse(who).ok().map(SessionKind::Thread)
    }
}

type Recent = Mutex<HashMap<(String, String, String, String), u64>>;

fn recent_messages() -> &'static Recent {
    static C: OnceLock<Recent> = OnceLock::new();
    C.get_or_init(Default::default)
}

/// The last team note each reader was handed, by (plan, reader). In memory:
/// after a restart a reader is handed its notes again once.
type NoteCursors = Mutex<HashMap<(String, String), u64>>;

fn note_cursors() -> &'static NoteCursors {
    static C: OnceLock<NoteCursors> = OnceLock::new();
    C.get_or_init(Default::default)
}

/// The tool a note's addressee reads it with.
pub const READ_NOTES_TOOL: &str = "read_team_notes";
/// A relay server says hello at start and every 2 minutes: one not heard
/// from for longer is taken as gone.
const HELLO_FRESH_MS: u64 = 6 * 60 * 1000;
/// What a hello may carry.
pub const HELLO_MAX_TOOLS: usize = 200;
const HELLO_MAX_NAME: usize = 80;

/// The agent process each session of a sandbox ran in at its latest hook
/// event, by (sandbox, `lead` or thread); and the relay servers that said
/// hello, by (sandbox, agent process): whether they offer `read_team_notes`,
/// and when they last said so. In memory: after a daemon restart a session
/// is unknown until its next hook event and its server's next hello.
#[derive(Default)]
struct Readers {
    procs: HashMap<(String, String), String>,
    hellos: HashMap<(String, String), (bool, u64)>,
}

fn readers() -> &'static Mutex<Readers> {
    static C: OnceLock<Mutex<Readers>> = OnceLock::new();
    C.get_or_init(Default::default)
}

fn valid_proc(p: &str) -> bool {
    !p.is_empty() && p.len() <= 40 && p.bytes().all(|b| b.is_ascii_digit() || b == b':')
}

impl App {
    /// A hook event's agent process, for [`App::can_read_notes`]: the
    /// session `lead` (no thread) or the thread it names runs in it now.
    pub(crate) fn note_reader_seen(&self, sandbox: &SandboxRef, thread: Option<&str>, proc: Option<&str>) {
        let Some(proc) = proc.filter(|p| valid_proc(p)) else { return };
        let who = thread.map(str::trim).filter(|t| !t.is_empty()).unwrap_or(team::LEAD).to_string();
        readers().lock().unwrap().procs.insert((sandbox.to_string(), who), proc.to_string());
    }

    /// `/api/relay/hello`: the relay server of the agent process `proc` in
    /// `sandbox` (from its token, never from the body) offers `tools`. Only
    /// whether `read_team_notes` is among them is kept. Returns whether the
    /// hello was taken.
    pub fn relay_hello(&self, sandbox: &SandboxRef, proc: &str, tools: &[String]) -> bool {
        if !valid_proc(proc) || tools.len() > HELLO_MAX_TOOLS || tools.iter().any(|t| t.len() > HELLO_MAX_NAME) {
            return false;
        }
        let reads = tools.iter().any(|t| t == READ_NOTES_TOOL);
        let now = now_ms();
        let n = {
            let mut r = readers().lock().unwrap();
            r.hellos.retain(|_, (_, at)| now.saturating_sub(*at) < HELLO_FRESH_MS);
            r.hellos.insert((sandbox.to_string(), proc.to_string()), (reads, now));
            r.hellos.len()
        };
        tracing::debug!("relay: hello from '{sandbox}' ({} tool(s)); {n} relay server(s) known", tools.len());
        true
    }

    /// Whether `who`'s session can read a team note: its agent process (from
    /// its hooks) has a relay server that said, lately, it offers
    /// `read_team_notes`. False when anything is unknown: a session started
    /// before its relay could say hello never does.
    pub(crate) fn can_read_notes(&self, sandbox: &SandboxRef, who: &str) -> bool {
        let r = readers().lock().unwrap();
        let Some(proc) = r.procs.get(&(sandbox.to_string(), who.to_string())) else { return false };
        r.hellos
            .get(&(sandbox.to_string(), proc.clone()))
            .is_some_and(|(reads, at)| *reads && now_ms().saturating_sub(*at) < HELLO_FRESH_MS)
    }

    /// `team_note`: an agent of a team (`thread`, or the lead) writes to
    /// `to` (`lead`, a thread of its plan, or `team`). Recorded on the
    /// team's timeline, then each addressee whose session is open is typed
    /// one line of sbxw's own that says a note waits (never the note's
    /// words), as a relay question is announced; it reads the note with
    /// `read_team_notes`. An addressee whose session sbxw cannot tell has
    /// that tool (see [`App::can_read_notes`]) is told, in the same line, to
    /// ask the sender to resend by `SendMessage` if it has not; it is also
    /// named in the third value, so the sender can send it that way at once.
    /// Returns the note's line on the log, who was told, and who of them may
    /// not be able to read it.
    pub fn team_note(self: &std::sync::Arc<Self>, sandbox: &SandboxRef, thread: Option<&ThreadName>, to: &str, kind: &str, text: &str) -> Result<(u64, Vec<String>, Vec<String>), String> {
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
        let line = team::note_line(seq, kind, &me, true);
        let unsure_line = team::note_line(seq, kind, &me, false);
        let mut told = vec![];
        let mut unsure = vec![];
        for (w, d) in self.note_targets(sandbox, &sessions, |k| self.sessions.is_live(k)) {
            let Some(k) = session_kind(&w) else { continue };
            let line = if d != team::Delivery::Drop && !self.can_read_notes(sandbox, &w) {
                unsure.push(w.clone());
                unsure_line.clone()
            } else {
                line.clone()
            };
            match d {
                team::Delivery::Type => self.spawn_push(sandbox.clone(), k, line.clone(), None),
                team::Delivery::Hold => self.hold_note_line(sandbox.clone(), w.clone(), line.clone()),
                team::Delivery::Drop => continue,
            }
            told.push(w);
        }
        tracing::info!("team: {me} of '{sandbox}' wrote note {seq} ({kind}) to {to}; {} told, {} may not read it", told.len(), unsure.len());
        Ok((seq, told, unsure))
    }

    /// The state its hooks last reported of `who` (`lead`: the sandbox's
    /// main session; a thread: its own), when one is known.
    fn session_state(&self, sandbox: &SandboxRef, who: &str) -> Option<SessionState> {
        let sb = sandbox.to_string();
        let all = self.agents.lock().unwrap().snapshot(|_| true, now_ms());
        let mine: Vec<&sbxw_core::agents::SessionInfo> = all
            .iter()
            .filter(|i| i.sandbox == sb)
            .filter(|i| if who == team::LEAD { i.thread.is_none() } else { i.thread.as_deref() == Some(who) })
            .collect();
        // The lead's sessions: one waiting on a question holds the line.
        if mine.iter().any(|i| i.state == SessionState::Attention) {
            return Some(SessionState::Attention);
        }
        mine.iter().map(|i| i.state).find(|s| matches!(s, SessionState::Idle | SessionState::Working)).or(mine.first().map(|i| i.state))
    }

    /// For each addressee: whether the line is typed now, held, or dropped
    /// (its session not open, or ended).
    pub(crate) fn note_targets(&self, sandbox: &SandboxRef, who: &[String], live: impl Fn(&SessionKey) -> bool) -> Vec<(String, team::Delivery)> {
        who.iter()
            .map(|w| {
                let key = match session_kind(w) {
                    Some(SessionKind::Thread(t)) => SessionKey::thread(sandbox, &t),
                    _ => SessionKey::agent(sandbox),
                };
                let d = if live(&key) { team::note_delivery(self.session_state(sandbox, w)) } else { team::Delivery::Drop };
                (w.clone(), d)
            })
            .collect()
    }

    /// After a merge into `base`: each thread of `sandbox` with an open
    /// session, other than those in `exclude` (every thread the plan merged,
    /// now or before), is told in one fixed line that `base` moved, under a
    /// team note's delivery rule (typed when idle or working, held while it
    /// waits on a question or a prompt, dropped when ended). Threads of
    /// another sandbox are never looked at. Returns who was told or will be.
    pub(crate) fn tell_main_moved(self: &std::sync::Arc<Self>, sandbox: &SandboxRef, merged: &[String], exclude: &[String], base: Option<&str>) -> Vec<String> {
        let Some(line) = team::main_moved_line(merged, base) else { return vec![] };
        let mut others: Vec<String> = self.sessions.open_threads(&sandbox.to_string()).into_iter().filter(|t| !exclude.contains(t) && !merged.contains(t)).collect();
        others.sort();
        others.dedup();
        let mut told = vec![];
        for (w, d) in self.note_targets(sandbox, &others, |k| self.sessions.is_live(k)) {
            let Some(k @ SessionKind::Thread(_)) = session_kind(&w) else { continue };
            match d {
                team::Delivery::Type => self.spawn_push(sandbox.clone(), k, line.clone(), None),
                team::Delivery::Hold => self.hold_note_line(sandbox.clone(), w.clone(), line.clone()),
                team::Delivery::Drop => continue,
            }
            told.push(w);
        }
        tracing::info!("team: '{sandbox}' main moved, {} other thread(s) told", told.len());
        told
    }

    /// The line waits while `who` waits on a question or a prompt, and is
    /// typed once it is idle or at work again; dropped if it ends, or after
    /// 30 minutes (the note stays for `read_team_notes`).
    pub(crate) fn hold_note_line(self: &std::sync::Arc<Self>, sandbox: SandboxRef, who: String, line: String) {
        let app = self.clone();
        tokio::spawn(async move {
            let mut rx = app.bus.subscribe();
            let deadline = tokio::time::Instant::now() + Duration::from_secs(30 * 60);
            loop {
                let key = match session_kind(&who) {
                    Some(SessionKind::Thread(t)) => SessionKey::thread(&sandbox, &t),
                    _ => SessionKey::agent(&sandbox),
                };
                let live = app.sessions.is_live(&key);
                match if live { team::note_delivery(app.session_state(&sandbox, &who)) } else { team::Delivery::Drop } {
                    team::Delivery::Type => {
                        if let Some(k) = session_kind(&who) {
                            app.spawn_push(sandbox.clone(), k, line.clone(), None);
                        }
                        return;
                    }
                    team::Delivery::Drop => return,
                    team::Delivery::Hold => {}
                }
                let left = deadline.saturating_duration_since(tokio::time::Instant::now());
                if left.is_zero() {
                    return;
                }
                match tokio::time::timeout(left, rx.recv()).await {
                    Ok(Ok(Event::Session(i))) if i.sandbox == sandbox.to_string() => {}
                    Ok(Ok(_)) | Ok(Err(tokio::sync::broadcast::error::RecvError::Lagged(_))) => continue,
                    _ => return,
                }
            }
        });
    }

    /// A `SendMessage` the status hook reported: recorded on the team's
    /// timeline as a direct message when its sender and its addressee are of
    /// one team. Only the addressee and the words, clipped; nothing else of
    /// the call, nothing in the log above debug.
    pub(crate) fn direct_message_noted(&self, sandbox: &SandboxRef, event: &Value) {
        let Some((to, text)) = team::sent_message(event) else { return };
        let thread = event["sbxw_thread"].as_str().map(str::trim).filter(|t| !t.is_empty());
        let me = thread.unwrap_or(team::LEAD).to_string();
        let text = sbxw_core::messages::clip_text(&text, team::MAX_TEXT);
        let id = {
            let mut plans = self.plans.lock().unwrap();
            let Some(id) = plans.plan_for_request(&sandbox.to_string(), thread) else { return };
            let Some(plan) = plans.get(&id).cloned() else { return };
            if thread.is_some_and(|t| !plan.threads.iter().any(|x| x.name == t)) {
                return;
            }
            let Some(addressee) = team::member(&plan, &to) else { return };
            if addressee == me || text.is_empty() {
                return;
            }
            // One message, once: a hook fired twice inside 10 s is the same send.
            let now = now_ms();
            let key = (id.clone(), me.clone(), addressee.clone(), text.clone());
            {
                let mut seen = recent_messages().lock().unwrap();
                seen.retain(|_, at| now.saturating_sub(*at) < 10_000);
                if seen.insert(key, now).is_some() {
                    return;
                }
            }
            plans.note(&id, now, team::EventKind::DirectMessage, &me, Some(&addressee), Some(&text), Value::Null);
            id
        };
        tracing::debug!("team: a direct message on {id}'s timeline");
        self.persist_team_logs(&sandbox.to_string());
        self.team_changed(&id);
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
