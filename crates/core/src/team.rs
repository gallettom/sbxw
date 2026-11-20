//! A team's timeline: one ordered list of what happened between the person,
//! the lead, the threads and sbxw, for the UIs to show as a conversation.
//!
//! A team is a plan, with its sandbox, lead and threads. Its timeline joins:
//!
//! * the plan's **log**, written by [`crate::plans::PlanBook`] at each
//!   transition (proposed, approved, refused, a status report, a failed
//!   start, marked done, the merge) and by the daemon for relay requests the
//!   team opened. The plan itself keeps only the latest state of each thread;
//!   the log keeps the history, with times and authors.
//! * the sandbox's **board** entries (decisions, warnings, questions,
//!   handoffs by agents; claims, releases, commits, conflicts by sbxw),
//!   attributed by their plan id, or for entries older than that field by
//!   time window and author (marked `guess`).
//! * **commits** of the team's branches, read from git by the daemon.
//! * for plans from before the log, events **synthesized** from the plan's
//!   current state; those whose time is not known have `at: null`.
//!
//! What never reaches sbxw is not here: messages the lead and threads send
//! each other with Claude Code's own `SendMessage`, and terminal
//! conversations. The book is pure: the daemon persists and serves it.

use crate::board::{Entry as BoardEntry, Kind as BoardKind};
use crate::plans::{MergeState, Plan, PlanState, SetBy, ThreadState};
use crate::relay::{RelayKind, RelayRequest, RelayState};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

/// A plan's log keeps this many entries; older ones are dropped.
pub const MAX_LOG: usize = 2000;
/// Characters of one event's text (a thread's goal in a proposal too).
pub const MAX_TEXT: usize = 2000;
/// Events one timeline page carries by default, and at most.
pub const PAGE_DEFAULT: usize = 100;
pub const PAGE_MAX: usize = 500;
/// Whether the text of relay requests (questions, answers) is written to the
/// log on disk. On (the person's choice, BOARD.md): the text, clipped to
/// [`MAX_TEXT`], is kept with the plan and pruned with it. Off: only who
/// asked whom, the outcome, the times and the image count, the text shown
/// while the relay still holds the request. Images are never kept.
pub const KEEP_RELAY_TEXT: bool = true;
/// What a team note (`team_note`) says it is.
pub const NOTE_KINDS: &[&str] = &["info", "warning", "decision"];

/// The person, the lead and sbxw as authors; a thread is its name.
pub const USER: &str = "user";
pub const LEAD: &str = "lead";
pub const SBXW: &str = "sbxw";

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Debug, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    PlanProposed,
    PlanApproved,
    PlanRefused,
    ThreadStatus,
    ThreadFailed,
    MarkedDone,
    ThreadClosed,
    TeamClosed,
    BoardDecision,
    BoardWarning,
    BoardQuestion,
    BoardHandoff,
    Claim,
    Release,
    Conflict,
    /// A question to another sandbox (`ask_other_sandbox`).
    RelayQuestion,
    /// The person sent a request to a sandbox for an answer or a draft.
    RelayRouted,
    /// The routed-to sandbox answered; held for the person.
    RelayDrafted,
    /// The person released an answer to a relay question.
    RelayAnswer,
    /// Questions for the person (`ask_user_questions`).
    UserQuestion,
    UserAnswer,
    ScreenshotAsked,
    ScreenshotGiven,
    /// The person refused a request (any kind).
    RelayRefused,
    MergeProposed,
    /// The person clicked Merge.
    MergeStarted,
    MergeResult,
    MergeFinished,
    Commit,
    /// A note an agent of the team sent another, or the whole team, through
    /// sbxw (`team_note`): `data.kind` is info, warning or decision.
    TeamNote,
    /// A message a session of the team sent another with Claude Code's own
    /// `SendMessage`, as the status hook reported the call (sender side,
    /// delivered). Only its addressee and words are kept.
    DirectMessage,
}

impl EventKind {
    /// The filter groups of the UIs: `notes` (decisions, warnings, handoffs,
    /// conflicts), `questions`, `commits`, `status` (reports, plan and merge
    /// steps), `claims`.
    pub fn group(self) -> &'static str {
        use EventKind::*;
        match self {
            BoardDecision | BoardWarning | BoardHandoff | Conflict | TeamNote => "notes",
            BoardQuestion | RelayQuestion | RelayRouted | RelayDrafted | RelayAnswer | UserQuestion | UserAnswer | ScreenshotAsked | ScreenshotGiven | RelayRefused => "questions",
            Commit => "commits",
            DirectMessage => "messages",
            Claim | Release => "claims",
            _ => "status",
        }
    }
}

/// One line of a plan's log, as it is kept on disk.
#[derive(Clone, Serialize, Deserialize, Debug, PartialEq)]
pub struct LogEntry {
    pub seq: u64,
    pub at: u64,
    pub kind: EventKind,
    pub from: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub data: Value,
}

/// A plan's log.
#[derive(Clone, Serialize, Deserialize, Debug, Default, PartialEq)]
pub struct TeamLog {
    #[serde(default)]
    pub entries: Vec<LogEntry>,
    #[serde(default)]
    pub next: u64,
}

impl TeamLog {
    pub fn push(&mut self, at: u64, kind: EventKind, from: &str, to: Option<&str>, text: Option<&str>, data: Value) {
        self.next += 1;
        self.entries.push(LogEntry {
            seq: self.next,
            at,
            kind,
            from: from.to_string(),
            to: to.map(str::to_string).filter(|t| !t.is_empty()),
            text: text.map(clip).filter(|t| !t.is_empty()),
            data,
        });
        if self.entries.len() > MAX_LOG {
            let drop = self.entries.len() - MAX_LOG;
            self.entries.drain(..drop);
        }
    }

    fn has(&self, kind: EventKind) -> bool {
        self.entries.iter().any(|e| e.kind == kind)
    }

    /// Whether a relay request's step is already logged (a request passes
    /// through each state once; the daemon may see it twice).
    pub fn has_relay(&self, id: &str, created_ms: u64, kind: EventKind) -> bool {
        self.entries.iter().any(|e| e.kind == kind && e.data["relay"] == id && e.data["relay_created"] == created_ms)
    }
}

pub fn clip(s: &str) -> String {
    let s = s.trim();
    match s.char_indices().nth(MAX_TEXT) {
        Some((i, _)) => format!("{}…", &s[..i]),
        None => s.to_string(),
    }
}

/// A plan's state as the UIs name it.
pub fn team_state(p: &Plan) -> &'static str {
    match p.state {
        PlanState::Pending => "proposed",
        PlanState::Refused => "refused",
        PlanState::Approved => {
            if p.merge.as_ref().is_some_and(|m| m.state == MergeState::Merged) {
                "merged"
            } else if p.closed {
                "closed"
            } else if !p.threads.is_empty() && p.threads.iter().all(|t| t.state.is_finished()) {
                "done"
            } else {
                "running"
            }
        }
    }
}

/// One row of `GET /api/teams`.
pub fn team_row(p: &Plan, last_at: Option<u64>) -> Value {
    let threads: Vec<Value> = p
        .threads
        .iter()
        .map(|t| json!({ "name": t.name, "state": t.state, "model": t.model, "repo": t.repo, "set_by": t.set_by }))
        .collect();
    json!({
        "id": p.id, "sandbox": p.sandbox, "summary": short(&p.summary, 120), "state": team_state(p),
        "variant": p.variant, "lead": p.lead, "threads": threads, "worktree": p.worktree,
        "created_ms": p.created_ms, "updated_ms": p.updated_ms, "last_ms": last_at.unwrap_or(p.updated_ms).max(p.updated_ms),
    })
}

fn short(s: &str, n: usize) -> String {
    let line = s.lines().next().unwrap_or("").trim();
    match line.char_indices().nth(n) {
        Some((i, _)) => format!("{}…", &line[..i]),
        None => line.to_string(),
    }
}

/// What the daemon logs when a relay request the team opened moves. `None`
/// for a step not worth a line (a virtual Mac request, a bounce).
pub fn relay_step(req: &RelayRequest) -> Option<(EventKind, String, Option<String>, Option<String>)> {
    if req.kind == RelayKind::Mac {
        return None;
    }
    let asker = req.from_thread.clone().unwrap_or_else(|| LEAD.to_string());
    let keep = |t: &Option<String>| if KEEP_RELAY_TEXT { t.clone() } else { None };
    Some(match req.state {
        RelayState::Pending if req.created_ms == req.updated_ms => {
            let kind = match req.kind {
                RelayKind::Ask => EventKind::UserQuestion,
                RelayKind::Screenshot => EventKind::ScreenshotAsked,
                _ => EventKind::RelayQuestion,
            };
            let to = match req.kind {
                RelayKind::Question => None,
                _ => Some(USER.to_string()),
            };
            (kind, asker, to, keep(&Some(req.question.clone())))
        }
        RelayState::Pending => return None,
        RelayState::Routed => (EventKind::RelayRouted, USER.into(), req.to.clone(), None),
        RelayState::Answered => (EventKind::RelayDrafted, req.to.clone().unwrap_or_else(|| SBXW.into()), Some(USER.into()), keep(&req.answer)),
        RelayState::Approved => {
            let kind = match req.kind {
                RelayKind::Ask => EventKind::UserAnswer,
                RelayKind::Screenshot => EventKind::ScreenshotGiven,
                _ => EventKind::RelayAnswer,
            };
            let text = if req.kind == RelayKind::Screenshot { None } else { keep(&req.answer) };
            (kind, USER.into(), Some(asker), text)
        }
        RelayState::Denied => (EventKind::RelayRefused, USER.into(), Some(asker), keep(&req.note)),
    })
}

/// The data a relay step keeps: the request's id and creation time (ids are
/// reused after a restart, so both name it), its kind, state, the sandbox
/// routed to, and the image count. Never an image.
pub fn relay_data(req: &RelayRequest) -> Value {
    json!({
        "relay": req.id, "relay_created": req.created_ms, "relay_kind": req.kind, "state": req.state,
        "to_sandbox": req.to, "shots": req.shots.len(),
    })
}

/// A commit of the team, as the daemon read it from git.
#[derive(Clone, Serialize, Debug, PartialEq, Eq, Default)]
pub struct TeamCommit {
    pub hash: String,
    pub short: String,
    pub subject: String,
    /// The author's name (never an email).
    pub author: String,
    /// Unix seconds.
    pub at: i64,
    /// The thread it belongs to; `None` for a shared branch's commit no
    /// board entry names.
    pub thread: Option<String>,
    pub branch: String,
    pub merged: bool,
    pub files: u32,
    pub added: u64,
    pub removed: u64,
    pub repo: String,
}

/// One event of a timeline, as the UIs read it.
#[derive(Clone, Serialize, Debug, PartialEq)]
pub struct Event {
    /// Stable across reads: `l<seq>` (the log), `b<id>` (the board),
    /// `c<hash>` (a commit), `s<n>` (synthesized).
    pub id: String,
    /// Unix milliseconds; `None`: not known (a plan from before the log).
    pub at: Option<u64>,
    pub kind: EventKind,
    pub group: &'static str,
    pub from: String,
    pub to: Option<String>,
    pub text: Option<String>,
    pub data: Value,
    /// The team of a board entry from before entries named their plan,
    /// inferred from its time and author.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub guess: bool,
    /// Where it sorts: `at`, or for an unknown time the best place known.
    #[serde(skip)]
    pub sort: u64,
}

/// The plan's own events: its log, and for what happened before the log
/// existed, events made from its state.
fn plan_events(p: &Plan, log: &TeamLog) -> Vec<Event> {
    let mut out: Vec<Event> = log
        .entries
        .iter()
        .map(|e| Event {
            id: format!("l{}", e.seq),
            at: Some(e.at),
            kind: e.kind,
            group: e.kind.group(),
            from: e.from.clone(),
            to: e.to.clone(),
            text: e.text.clone(),
            data: e.data.clone(),
            guess: false,
            sort: e.at,
        })
        .collect();
    let mut n = 0;
    let mut synth = |at: Option<u64>, sort: u64, kind: EventKind, from: &str, to: Option<String>, text: Option<String>, data: Value| {
        n += 1;
        Event { id: format!("s{n}"), at, kind, group: kind.group(), from: from.into(), to, text, data, guess: false, sort }
    };
    let mut add = vec![];
    if !log.has(EventKind::PlanProposed) {
        add.push(synth(Some(p.created_ms), p.created_ms, EventKind::PlanProposed, LEAD, Some(USER.into()), Some(p.summary.clone()), proposal_data(p)));
    }
    match p.state {
        PlanState::Approved if !log.has(EventKind::PlanApproved) => {
            add.push(synth(None, p.created_ms + 1, EventKind::PlanApproved, USER, Some(LEAD.into()), None, json!({ "variant": p.variant, "threads": names(p) })))
        }
        PlanState::Refused if !log.has(EventKind::PlanRefused) => {
            add.push(synth(None, p.updated_ms, EventKind::PlanRefused, USER, Some(LEAD.into()), p.note.clone(), Value::Null))
        }
        _ => {}
    }
    let logged: BTreeSet<&str> = log
        .entries
        .iter()
        .filter(|e| matches!(e.kind, EventKind::ThreadStatus | EventKind::ThreadFailed | EventKind::MarkedDone))
        .filter_map(|e| e.data["thread"].as_str())
        .collect();
    for t in p.threads.iter().filter(|t| t.state != ThreadState::Starting && !logged.contains(t.name.as_str())) {
        let (kind, from, to) = match t.set_by {
            Some(SetBy::Sbxw) => (EventKind::ThreadFailed, SBXW.to_string(), None),
            Some(SetBy::Lead) => (EventKind::MarkedDone, LEAD.to_string(), Some(t.name.clone())),
            None => (EventKind::ThreadStatus, t.name.clone(), Some(LEAD.to_string())),
        };
        // The latest report's time is known: it is the thread's own.
        let at = (t.updated_ms > 0).then_some(t.updated_ms);
        add.push(synth(at, t.updated_ms.max(p.created_ms + 2), kind, &from, to, Some(t.summary.clone()), json!({ "thread": t.name, "state": t.state })));
    }
    if let Some(m) = &p.merge {
        let late = p.updated_ms;
        if !log.has(EventKind::MergeProposed) {
            add.push(synth(None, late, EventKind::MergeProposed, LEAD, Some(USER.into()), Some(m.note.clone()), json!({ "order": m.order })));
        }
        if !log.has(EventKind::MergeResult) {
            for r in &m.results {
                add.push(synth(None, late, EventKind::MergeResult, SBXW, None, r.error.clone().or(r.note.clone()), merge_result_data(r)));
            }
        }
        if matches!(m.state, MergeState::Merged | MergeState::Failed) && !log.has(EventKind::MergeFinished) {
            add.push(synth(None, late, EventKind::MergeFinished, SBXW, None, None, json!({ "state": m.state, "order": m.order })));
        }
    }
    out.extend(add);
    out
}

fn names(p: &Plan) -> Vec<String> {
    p.threads.iter().map(|t| t.name.clone()).collect()
}

/// A proposal's payload: the variant and why, and each thread's brief, clipped.
pub fn proposal_data(p: &Plan) -> Value {
    let threads: Vec<Value> = p
        .threads
        .iter()
        .map(|t| json!({ "name": t.name, "goal": clip(&t.goal), "scope": clip(&t.scope), "model": t.model, "repo": t.repo }))
        .collect();
    json!({ "variant": p.variant, "justification": clip(&p.justification), "threads": threads })
}

pub fn merge_result_data(r: &crate::plans::MergeResult) -> Value {
    json!({ "thread": r.thread, "ok": r.ok, "commit": r.commit })
}

/// Whether a board entry from before entries named their plan belongs to
/// `p`: written while the plan was at work, by its lead, one of its threads,
/// or sbxw about one of them.
fn board_guess(p: &Plan, e: &BoardEntry, end: u64) -> bool {
    if e.at < p.created_ms || e.at > end {
        return false;
    }
    let team = |n: &str| p.threads.iter().any(|t| t.name == n);
    match e.by.as_str() {
        crate::board::LEAD => e.to.as_deref().is_none_or(team),
        crate::board::SBXW => e.to.as_deref().is_none_or(team),
        by => team(by),
    }
}

fn board_event(e: &BoardEntry, guess: bool) -> Option<Event> {
    let kind = match e.kind {
        BoardKind::Decision => EventKind::BoardDecision,
        BoardKind::Warning => EventKind::BoardWarning,
        BoardKind::Question => EventKind::BoardQuestion,
        BoardKind::Handoff => EventKind::BoardHandoff,
        BoardKind::Claim => EventKind::Claim,
        BoardKind::Release => EventKind::Release,
        BoardKind::Conflict => EventKind::Conflict,
        // A commit is read from git; the board only says whose it is.
        BoardKind::Commit => return None,
    };
    Some(Event {
        id: format!("b{}", e.id),
        at: Some(e.at),
        kind,
        group: kind.group(),
        from: e.by.clone(),
        to: e.to.clone(),
        text: e.text.clone(),
        data: json!({ "files": e.files }),
        guess,
        sort: e.at,
    })
}

/// The board entries of plan `p` (`end`: when it stopped being at work).
pub fn board_entries<'a>(p: &Plan, entries: &'a [BoardEntry], end: u64) -> Vec<(&'a BoardEntry, bool)> {
    entries
        .iter()
        .filter_map(|e| match &e.plan {
            Some(id) => (id == &p.id).then_some((e, false)),
            None => board_guess(p, e, end).then_some((e, true)),
        })
        .collect()
}

/// The questions of a round (`ask_user_questions`) and their answers, read
/// from the round's text (`❓ **Q1 - Title** : text`, `➡️ recommendation`)
/// and the released answer's `Q1 : …` lines. The questions are open: an
/// answer is what the person wrote, the recommendation what the agent offered.
pub fn round_questions(question: &str, answer: Option<&str>) -> Vec<Value> {
    let mut answers: BTreeMap<String, String> = BTreeMap::new();
    let mut last: Option<String> = None;
    for line in answer.unwrap_or("").lines() {
        let head = line.split_once(':').map(|(k, v)| (k.trim(), v.trim()));
        match head {
            Some((k, v)) if !k.is_empty() && k.len() <= 12 && k.starts_with('Q') && k[1..].chars().all(|c| c.is_ascii_alphanumeric()) => {
                answers.insert(k.to_string(), v.to_string());
                last = Some(k.to_string());
            }
            _ => {
                if let Some(k) = &last {
                    let a = answers.entry(k.clone()).or_default();
                    a.push('\n');
                    a.push_str(line);
                }
            }
        }
    }
    let mut out = vec![];
    for block in question.split("\n---\n") {
        let Some(start) = block.find("❓ **") else { continue };
        let rest = &block[start + "❓ **".len()..];
        let Some((head, body)) = rest.split_once("**") else { continue };
        let (id, title) = match head.split_once(" - ") {
            Some((i, t)) => (i.trim(), t.trim()),
            None => (head.trim(), ""),
        };
        let body = body.trim_start().trim_start_matches(':').trim();
        let (text, rec) = match body.split_once("➡️") {
            Some((t, r)) => (t.trim(), r.trim()),
            None => (body, ""),
        };
        out.push(json!({
            "id": id, "title": title, "text": clip(text), "recommendation": clip(rec),
            "answer": answers.get(id).map(|a| clip(a)),
        }));
    }
    out
}

/// Fills a relay step's text from the request while the relay still holds
/// it (the same id and creation time), when the log kept none.
fn fill_relay(ev: &mut Event, live: &[RelayRequest]) {
    let (Some(id), Some(created)) = (ev.data["relay"].as_str(), ev.data["relay_created"].as_u64()) else { return };
    let Some(req) = live.iter().find(|r| r.id == id && r.created_ms == created) else { return };
    if ev.text.is_none() {
        ev.text = match ev.kind {
            EventKind::RelayQuestion | EventKind::UserQuestion | EventKind::ScreenshotAsked => Some(clip(&req.question)),
            EventKind::RelayDrafted | EventKind::RelayAnswer | EventKind::UserAnswer => req.answer.as_deref().map(clip),
            EventKind::RelayRefused => req.note.as_deref().map(clip),
            _ => None,
        };
    }
    if req.kind == RelayKind::Ask && matches!(ev.kind, EventKind::UserQuestion | EventKind::UserAnswer) {
        let released = (req.state == RelayState::Approved).then_some(req.answer.as_deref()).flatten();
        ev.data["questions"] = json!(round_questions(&req.question, released));
    }
    ev.data["live"] = true.into();
}

fn commit_event(c: &TeamCommit) -> Event {
    let at = (c.at.max(0) as u64) * 1000;
    Event {
        id: format!("c{}", c.hash),
        at: Some(at),
        kind: EventKind::Commit,
        group: "commits",
        from: c.thread.clone().unwrap_or_else(|| "team".into()),
        to: None,
        text: Some(clip(&c.subject)),
        data: json!({
            "hash": c.hash, "short": c.short, "author": c.author, "branch": c.branch, "merged": c.merged,
            "files": c.files, "added": c.added, "removed": c.removed, "repo": c.repo, "thread": c.thread,
        }),
        guess: false,
        sort: at,
    }
}

/// The whole timeline of plan `p`, oldest first.
pub fn timeline(p: &Plan, log: &TeamLog, board: &[BoardEntry], relay_live: &[RelayRequest], commits: &[TeamCommit], now: u64) -> Vec<Event> {
    let mut out = plan_events(p, log);
    for ev in out.iter_mut() {
        fill_relay(ev, relay_live);
    }
    let end = if team_state(p) == "merged" || p.state == PlanState::Refused { p.updated_ms } else { now.max(p.updated_ms) };
    out.extend(board_entries(p, board, end).into_iter().filter_map(|(e, g)| board_event(e, g)));
    out.extend(commits.iter().map(commit_event));
    out.sort_by(|a, b| a.sort.cmp(&b.sort).then_with(|| order(a).cmp(&order(b))));
    out
}

/// Ties in time: the log's own order, then the board's, then commits.
fn order(e: &Event) -> (u8, u64) {
    let n = |s: &str| s.parse::<u64>().unwrap_or(0);
    match e.id.split_at(1) {
        ("s", r) => (0, n(r)),
        ("l", r) => (1, n(r)),
        ("b", r) => (2, n(r)),
        _ => (3, 0),
    }
}

/// What a UI asks for: a group (`notes`, `questions`, `commits`, `status`,
/// `claims`, or `conversation`: all but claims; none: all), one author or
/// addressee, words in the text.
#[derive(Default, Debug, Clone)]
pub struct Filter<'a> {
    pub group: Option<&'a str>,
    pub thread: Option<&'a str>,
    pub q: Option<&'a str>,
}

impl Filter<'_> {
    pub fn keeps(&self, e: &Event) -> bool {
        match self.group {
            // Everything people and agents said and did: claims and releases
            // are the board's bookkeeping, shown apart.
            Some("conversation") if e.group == "claims" => return false,
            Some("conversation") | None => {}
            Some(g) if e.group != g => return false,
            Some(_) => {}
        }
        if let Some(t) = self.thread {
            let about = e.data["thread"].as_str() == Some(t) || e.data["order"].as_array().is_some_and(|o| o.iter().any(|x| x == t));
            if e.from != t && e.to.as_deref() != Some(t) && !about {
                return false;
            }
        }
        if let Some(q) = self.q.map(str::trim).filter(|q| !q.is_empty()) {
            let q = q.to_lowercase();
            let hay = format!("{} {} {}", e.text.as_deref().unwrap_or(""), e.from, e.data["short"].as_str().unwrap_or("")).to_lowercase();
            if !hay.contains(&q) {
                return false;
            }
        }
        true
    }
}

/// One page, newest first: the events before `before` (an event id; none:
/// from the newest), at most `limit`, and whether older ones remain.
pub fn page(all: &[Event], filter: &Filter, before: Option<&str>, limit: usize) -> (Vec<Event>, bool) {
    let kept: Vec<&Event> = all.iter().filter(|e| filter.keeps(e)).collect();
    let end = match before {
        Some(id) => kept.iter().position(|e| e.id == id).unwrap_or(kept.len()),
        None => kept.len(),
    };
    let start = end.saturating_sub(limit);
    let page: Vec<Event> = kept[start..end].iter().rev().map(|e| (*e).clone()).collect();
    (page, start > 0)
}

/// The decisions and warnings that still stand for plan `p`, from the board
/// and from team notes, newest first, for the pinned notes.
pub fn standing_notes(p: &Plan, board: &[BoardEntry], log: &TeamLog, now: u64) -> Vec<Value> {
    let end = if team_state(p) == "merged" { p.updated_ms } else { now.max(p.updated_ms) };
    let mut out: Vec<(u64, Value)> = board_entries(p, board, end)
        .into_iter()
        .filter(|(e, _)| matches!(e.kind, BoardKind::Decision | BoardKind::Warning) && e.by != crate::board::SBXW)
        .map(|(e, g)| (e.at, json!({ "id": format!("b{}", e.id), "at": e.at, "kind": e.kind, "by": e.by, "to": e.to, "text": e.text, "files": e.files, "guess": g })))
        .collect();
    out.extend(
        log.entries
            .iter()
            .filter(|e| e.kind == EventKind::TeamNote && matches!(e.data["kind"].as_str(), Some("warning" | "decision")))
            .map(|e| (e.at, json!({ "id": format!("l{}", e.seq), "at": e.at, "kind": e.data["kind"], "by": e.from, "to": e.to, "text": e.text, "files": [], "guess": false, "team_note": true }))),
    );
    out.sort_by_key(|(at, _)| std::cmp::Reverse(*at));
    out.truncate(50);
    out.into_iter().map(|(_, v)| v).collect()
}

/// What becomes of the line telling a session a team note waits, by the
/// state its hooks last reported: typed now, held until it is idle or at
/// work again, or dropped (its session ended: it reads its notes with
/// `read_team_notes` when it is back, its briefing says so).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Delivery {
    Type,
    Hold,
    Drop,
}

/// The line typed into an addressee's session when a team note waits for it:
/// sbxw's words, never the note's. When sbxw cannot tell the session has the
/// tool to read it (`known` false), the line says what to do without it.
pub fn note_line(seq: u64, kind: &str, from: &str, known: bool) -> String {
    let mut line = format!("[sbxw team note · {seq}] A {kind} from {from} for you: read it with the read_team_notes tool (MCP sbxw-relay).");
    if !known {
        line.push_str(&format!(" If your session has no such tool, ask {from} to resend it by SendMessage."));
    }
    line
}

/// The line typed into a thread's session when other threads were merged
/// into `base` (sbxw's own words: thread names and a branch name, nothing an
/// agent wrote). `None` when nothing was merged.
pub fn main_moved_line(merged: &[String], base: Option<&str>) -> Option<String> {
    if merged.is_empty() {
        return None;
    }
    let base = base.filter(|b| !b.is_empty() && !b.contains(char::is_whitespace)).unwrap_or("main");
    Some(format!(
        "[sbxw] {base} moved: {} merged. Check `git merge-tree --write-tree {base} HEAD` before you go on.",
        merged.join(", ")
    ))
}

/// A session waiting on a question or a permission prompt would take a typed
/// line as an answer (a menu's pick): never typed then.
pub fn note_delivery(state: Option<crate::agents::SessionState>) -> Delivery {
    use crate::agents::SessionState::*;
    match state {
        Some(Idle | Working) => Delivery::Type,
        Some(Attention) => Delivery::Hold,
        Some(Exited) | None => Delivery::Drop,
    }
}

/// A `SendMessage` the status hook reported (`PostToolUse`): the addressee
/// and the words, when the send went through. `None` for any other tool, a
/// failed send, or nothing to say. Nothing else of the call is read.
pub fn sent_message(event: &Value) -> Option<(String, String)> {
    if event["hook_event_name"] != "PostToolUse" || event["tool_name"] != "SendMessage" {
        return None;
    }
    let resp = &event["tool_response"];
    let parsed;
    let resp = match resp.as_str() {
        Some(s) => {
            parsed = serde_json::from_str::<Value>(s).unwrap_or(Value::Null);
            &parsed
        }
        None => resp,
    };
    if resp["success"] != true {
        return None;
    }
    let to = event["tool_input"]["to"].as_str()?.trim();
    let text = event["tool_input"]["message"].as_str()?.trim();
    if to.is_empty() || text.is_empty() {
        return None;
    }
    // A reply to a `from` address (`uds:...`) names no session: the
    // delivery's own words say whom it reached (`→ <name> (`).
    let to = if to.starts_with("uds:") {
        let m = resp["message"].as_str()?;
        let after = m.split_once("→ ")?.1;
        after.split_once(" (").map(|(n, _)| n).unwrap_or(after).trim().to_string()
    } else {
        // `name [ref]`: the name.
        match to.rsplit_once(" [") {
            Some((n, r)) if r.ends_with(']') => n.trim().to_string(),
            _ => to.to_string(),
        }
    };
    Some((to, text.to_string()))
}

/// Who a session name is in plan `p`: `lead` (its lead's name), a thread, or
/// nobody of the team.
pub fn member(p: &Plan, name: &str) -> Option<String> {
    if p.lead.as_deref() == Some(name) {
        return Some(LEAD.to_string());
    }
    p.threads.iter().find(|t| t.name == name).map(|t| t.name.clone())
}

/// The team notes addressed to `me` (by name, or to the whole team, not its
/// own) after the log line `after`, oldest first.
pub fn notes_for<'a>(log: &'a TeamLog, me: &str, after: u64) -> Vec<&'a LogEntry> {
    log.entries
        .iter()
        .filter(|e| e.kind == EventKind::TeamNote && e.from != me)
        .filter(|e| matches!(e.to.as_deref(), Some(t) if t == me || t == "team"))
        .filter(|e| e.seq > after)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plans::{PlanBook, PlanThread, Variant};

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

    #[test]
    fn a_round_reads_as_questions_with_their_answers() {
        let q = "Two choices.\n\n❓ **Q1 - Port** : which port?\n\n➡️ 8080\n\n---\n\n❓ **Q2 - Name** : what name?";
        let a = "Q1 : 9090\nQ2 : orders\nand more";
        let v = round_questions(q, Some(a));
        assert_eq!(v.len(), 2);
        assert_eq!(v[0]["id"], "Q1");
        assert_eq!(v[0]["title"], "Port");
        assert_eq!(v[0]["text"], "which port?");
        assert_eq!(v[0]["recommendation"], "8080");
        assert_eq!(v[0]["answer"], "9090");
        assert_eq!(v[1]["answer"], "orders\nand more");
        assert_eq!(round_questions(q, None)[1]["answer"], Value::Null);
    }

    #[test]
    fn a_plan_from_before_the_log_is_synthesized_with_unknown_times() {
        let mut b = PlanBook::new();
        let p = b.propose("p-1".into(), "sb", "Do it", vec![t("a"), t("b")], Variant::Split, "independent", Some("Lead"), 1000).unwrap();
        b.approve(&p.id, vec![t("a"), t("b")], None, None, 2000).unwrap();
        let p = b.set_thread("sb", "a", ThreadState::Working, "on it", false, 3000).unwrap();
        // As an old state file would load: no log at all.
        let ev = timeline(&p, &TeamLog::default(), &[], &[], &[], 5000);
        let kinds: Vec<EventKind> = ev.iter().map(|e| e.kind).collect();
        assert_eq!(kinds, vec![EventKind::PlanProposed, EventKind::PlanApproved, EventKind::ThreadStatus]);
        assert_eq!(ev[0].at, Some(1000));
        assert_eq!(ev[1].at, None, "approval time unknown");
        assert_eq!(ev[2].at, Some(3000));
        assert_eq!(ev[2].from, "a");
    }

    #[test]
    fn pages_go_newest_first_and_filters_apply_before_paging() {
        let mut log = TeamLog::default();
        for i in 0..10 {
            log.push(100 + i, EventKind::ThreadStatus, if i % 2 == 0 { "a" } else { "b" }, Some(LEAD), Some(&format!("step {i}")), json!({ "thread": if i % 2 == 0 { "a" } else { "b" } }));
        }
        let mut b = PlanBook::new();
        let p = b.propose("p-1".into(), "sb", "Do it", vec![t("a"), t("b")], Variant::Split, "x", None, 50).unwrap();
        let all = timeline(&p, &log, &[], &[], &[], 1000);
        let (p1, more) = page(&all, &Filter::default(), None, 4);
        assert!(more);
        assert_eq!(p1[0].text.as_deref(), Some("step 9"));
        let (p2, _) = page(&all, &Filter::default(), Some(&p1[3].id), 4);
        assert_eq!(p2[0].text.as_deref(), Some("step 5"));
        let (only_a, _) = page(&all, &Filter { thread: Some("a"), ..Default::default() }, None, 100);
        assert_eq!(only_a.len(), 5);
        let (found, _) = page(&all, &Filter { q: Some("STEP 3"), ..Default::default() }, None, 100);
        assert_eq!(found.len(), 1);
        log.push(200, EventKind::Claim, "a", None, None, json!({ "files": ["x.rs"] }));
        let all = timeline(&p, &log, &[], &[], &[], 1000);
        let (talk, _) = page(&all, &Filter { group: Some("conversation"), ..Default::default() }, None, 100);
        assert!(talk.iter().all(|e| e.kind != EventKind::Claim) && talk.len() == all.len() - 1, "all but claims");
    }

    #[test]
    fn the_main_moved_line_names_the_threads_and_the_base_only() {
        assert_eq!(main_moved_line(&[], Some("main")), None);
        let l = main_moved_line(&["a".into(), "b".into()], Some("dev")).unwrap();
        assert_eq!(l, "[sbxw] dev moved: a, b merged. Check `git merge-tree --write-tree dev HEAD` before you go on.");
        assert!(main_moved_line(&["a".into()], None).unwrap().starts_with("[sbxw] main moved: a merged."));
        assert!(main_moved_line(&["a".into()], Some("x y")).unwrap().contains("--write-tree main HEAD"));
        assert!(!l.contains('\n'));
    }

    #[test]
    fn a_note_line_never_carries_the_note_and_says_what_to_do_without_the_tool() {
        let known = note_line(4, "warning", "api", true);
        assert_eq!(known, "[sbxw team note · 4] A warning from api for you: read it with the read_team_notes tool (MCP sbxw-relay).");
        let unsure = note_line(4, "warning", "api", false);
        assert!(unsure.starts_with(&known) && unsure.ends_with("ask api to resend it by SendMessage."));
        assert!(!unsure.contains('\n'));
    }
}
