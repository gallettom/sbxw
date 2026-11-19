//! The monitor's model: one card per Claude session, kept from the daemon's
//! `session` events one card at a time, and the view the screen draws
//! (`data.monitor`), assembled from each card's JSON as it was last built.
//!
//! A card's JSON is built again only when its session's report changed; its
//! `rev` then moves, and only then. The screen memoises a row by its `key`
//! and `rev` (`screens/monitor.luau`), so an event about one session builds
//! one row again, not the list.

use serde_json::{json, Map, Value as J};
use std::collections::{BTreeMap, HashMap};

/// Rows a page of the list.
pub const PAGE: usize = 50;
/// The sandbox Jarvis lives in: its own panel shows it, not the monitor.
pub const JARVIS: &str = crate::feat::relay::JARVIS;

/// Where a session stands, for the list: who needs the person first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Group {
    You,
    Working,
    Idle,
    Ended,
}

impl Group {
    pub fn of(state: &str) -> Group {
        match state {
            "attention" => Group::You,
            "working" => Group::Working,
            "exited" => Group::Ended,
            _ => Group::Idle,
        }
    }
    pub fn id(self) -> &'static str {
        match self {
            Group::You => "you",
            Group::Working => "working",
            Group::Idle => "idle",
            Group::Ended => "ended",
        }
    }
    pub fn title(self) -> &'static str {
        match self {
            Group::You => "Needs you",
            Group::Working => "Working",
            Group::Idle => "Idle",
            Group::Ended => "Ended",
        }
    }
    pub fn word(self) -> &'static str {
        match self {
            Group::You => "needs you",
            Group::Working => "working",
            Group::Idle => "idle",
            Group::Ended => "ended",
        }
    }
    pub fn tone(self) -> &'static str {
        match self {
            Group::You => "amber",
            Group::Working => "blue",
            Group::Idle | Group::Ended => "neutral",
        }
    }
}

fn s<'a>(v: &'a J, k: &str) -> &'a str {
    v[k].as_str().unwrap_or("")
}

fn opt<'a>(v: &'a J, k: &str) -> Option<&'a str> {
    v[k].as_str().map(str::trim).filter(|t| !t.is_empty())
}

/// The session's thread, when it is one.
pub fn thread_of(info: &J) -> Option<&str> {
    opt(info, "thread")
}

/// `sandbox · thread` or `sandbox · main`.
pub fn who(sandbox: &str, thread: Option<&str>) -> String {
    format!("{sandbox} · {}", thread.unwrap_or("main"))
}

/// The first non-empty line of a text.
pub fn first_line(t: &str) -> Option<&str> {
    t.lines().map(str::trim).find(|l| !l.is_empty())
}

/// The steps of the question a session waits on (`steps`, else the older `question`).
pub fn steps(info: &J) -> Vec<J> {
    match &info["steps"] {
        J::Array(a) if !a.is_empty() => a.clone(),
        _ if info["question"].is_object() => vec![info["question"].clone()],
        _ => vec![],
    }
}

/// One session's card.
#[derive(Debug, Clone)]
pub struct Card {
    /// The daemon's report, as the last event or list gave it.
    pub info: J,
    pub rev: u64,
    /// The list row, built from `info` when it changed.
    pub row: J,
}

/// The whole model, one per app (the UI thread's).
#[derive(Debug, Default)]
pub struct Model {
    pub cards: BTreeMap<String, Card>,
    /// The next `rev` handed out.
    next: u64,
    /// The selected card's key.
    pub sel: Option<String>,
    /// Pages of the list shown.
    pub pages: usize,
    /// Sends that went out, by card: the screen empties its draft when it moves.
    pub sent: HashMap<String, u64>,
    /// Built from the daemon's list at least once (the view was opened).
    pub seeded: bool,
}

impl Model {
    /// The card a report belongs to: the same thread of the same sandbox,
    /// else the same session id, else the sandbox's session without a thread.
    fn key_for(&self, info: &J) -> Option<String> {
        let sb = opt(info, "sandbox")?;
        if sb == JARVIS {
            return None;
        }
        let thread = thread_of(info);
        let sid = opt(info, "session_id");
        let found = self.cards.iter().find(|(_, c)| {
            s(&c.info, "sandbox") == sb
                && match thread {
                    Some(t) => thread_of(&c.info) == Some(t),
                    None => thread_of(&c.info).is_none() && (sid.is_none() || opt(&c.info, "session_id").is_none() || opt(&c.info, "session_id") == sid),
                }
        });
        Some(match found {
            Some((k, _)) => k.clone(),
            None => match (thread, sid) {
                (Some(t), _) => format!("{sb}::{t}"),
                (None, Some(id)) => format!("{sb}:{id}"),
                (None, None) => sb.to_string(),
            },
        })
    }

    /// A `session` event: that one card changes (a whole report replaces
    /// what was known; a partial one, without `started_ms`, is merged into
    /// it). True when something the view shows changed.
    pub fn upsert(&mut self, info: &J) -> bool {
        let Some(key) = self.key_for(info) else { return false };
        let merged = match (self.cards.get(&key), info.get("started_ms").is_some()) {
            (Some(c), false) => {
                let mut m = c.info.clone();
                if let (Some(o), Some(n)) = (m.as_object_mut(), info.as_object()) {
                    for (k, v) in n {
                        o.insert(k.clone(), v.clone());
                    }
                }
                m
            }
            _ => info.clone(),
        };
        self.put(key, merged)
    }

    fn put(&mut self, key: String, info: J) -> bool {
        if self.cards.get(&key).is_some_and(|c| c.info == info) {
            return false;
        }
        self.next += 1;
        let row = row_of(&key, self.next, &info);
        self.cards.insert(key, Card { info, rev: self.next, row });
        true
    }

    /// The daemon's whole list (`data.sessions`): cards it no longer lists
    /// go, the others change only when their report did.
    pub fn seed(&mut self, list: &J) -> bool {
        let mut seen = vec![];
        let mut changed = false;
        for info in list.as_array().map(Vec::as_slice).unwrap_or(&[]) {
            let Some(key) = self.key_for(info) else { continue };
            // Two reports for one card in one list: the later one counts.
            changed |= self.put(key.clone(), info.clone());
            seen.push(key);
        }
        let before = self.cards.len();
        self.cards.retain(|k, _| seen.contains(k));
        changed |= self.cards.len() != before || !self.seeded;
        self.seeded = true;
        if self.sel.as_ref().is_some_and(|k| !self.cards.contains_key(k)) {
            self.sel = None;
        }
        changed
    }

    /// Every card's key, in the list's order: those that need the person
    /// first (the longest waiting at the top), then working, idle and ended
    /// (the latest report first).
    pub fn order(&self) -> Vec<String> {
        let mut v: Vec<(&String, &Card)> = self.cards.iter().collect();
        v.sort_by(|(ka, a), (kb, b)| {
            let (ga, gb) = (Group::of(s(&a.info, "state")), Group::of(s(&b.info, "state")));
            let (ta, tb) = (a.info["ts"].as_u64().unwrap_or(0), b.info["ts"].as_u64().unwrap_or(0));
            ga.cmp(&gb).then_with(|| if ga == Group::You { ta.cmp(&tb) } else { tb.cmp(&ta) }).then_with(|| ka.cmp(kb))
        });
        v.into_iter().map(|(k, _)| k.clone()).collect()
    }

    /// The selected card: the one picked, else the first in the list.
    pub fn selected(&self) -> Option<String> {
        self.sel.clone().filter(|k| self.cards.contains_key(k)).or_else(|| self.order().into_iter().next())
    }

    /// The card `by` rows from the selected one (Up -1, Down +1), stopping at the ends.
    pub fn step(&mut self, by: i64) {
        let order = self.order();
        let Some(cur) = self.selected() else { return };
        let i = order.iter().position(|k| *k == cur).unwrap_or(0) as i64;
        let j = (i + by).clamp(0, order.len() as i64 - 1) as usize;
        self.sel = Some(order[j].clone());
        // Keep the selection on a drawn page.
        while j >= PAGE * self.pages.max(1) {
            self.pages = self.pages.max(1) + 1;
        }
    }

    /// The next card that needs the person after the selected one, round
    /// (the first one when the selection does not need them). False: none does.
    pub fn next_needing(&mut self) -> bool {
        let order = self.order();
        let you: Vec<&String> = order.iter().filter(|k| Group::of(s(&self.cards[*k].info, "state")) == Group::You).collect();
        if you.is_empty() {
            return false;
        }
        let cur = self.selected();
        let at = cur.as_ref().and_then(|c| you.iter().position(|k| *k == c));
        let next = match at {
            Some(i) => you[(i + 1) % you.len()],
            None => you[0],
        };
        self.sel = Some(next.clone());
        let j = order.iter().position(|k| k == next).unwrap_or(0);
        while j >= PAGE * self.pages.max(1) {
            self.pages = self.pages.max(1) + 1;
        }
        true
    }

    /// The sandbox's most urgent card (the sidebar's selection, when the view opens).
    pub fn first_of(&self, sandbox: &str) -> Option<String> {
        self.order().into_iter().find(|k| s(&self.cards[k].info, "sandbox") == sandbox)
    }

    pub fn counts(&self) -> (usize, usize, usize, usize) {
        let mut n = (0, 0, 0, 0);
        for c in self.cards.values() {
            match Group::of(s(&c.info, "state")) {
                Group::You => n.0 += 1,
                Group::Working => n.1 += 1,
                Group::Idle => n.2 += 1,
                Group::Ended => n.3 += 1,
            }
        }
        n
    }

    /// `data.monitor`. `panes`: `data.app.panes`, to say where a session already shows.
    pub fn view(&self, now: u64, panes: &J) -> J {
        let order = self.order();
        let (rows, count) = crate::screen::paged(order.clone(), PAGE, self.pages.max(1), "sessions");
        let (you, working, idle, ended) = self.counts();
        let sel = self.selected();
        json!({
            "now": now,
            "rows": rows.iter().map(|k| self.cards[k].row.clone()).collect::<Vec<_>>(),
            "count": count,
            "counts": { "you": you, "working": working, "idle": idle, "ended": ended, "total": self.cards.len() },
            "sel": sel,
            "detail": sel.as_ref().map(|k| self.detail(k, panes)).unwrap_or(J::Null),
        })
    }

    /// What the middle shows for one card.
    pub fn detail(&self, key: &str, panes: &J) -> J {
        let c = &self.cards[key];
        let i = &c.info;
        let sb = s(i, "sandbox");
        let thread = thread_of(i);
        let g = Group::of(s(i, "state"));
        let st = steps(i);
        let waiting = g == Group::You;
        let answerable = i["answerable"] == true;
        // Typed words go to a thread's own terminal, or to the sandbox's
        // one main session when the daemon can tell which it is.
        let reach = thread.is_some() || answerable;
        let why = if waiting {
            Some("Answer the card above first: typed text would land in its menu.")
        } else if g == Group::Ended {
            Some("This session has ended.")
        } else if !reach {
            Some("sbxw cannot tell which terminal is this session's: answer in its terminal.")
        } else {
            None
        };
        let worktree = opt(i, "cwd").and_then(|c| c.split_once(".sbxw-worktrees/")).map(|(_, w)| w.trim_end_matches('/').to_string());
        let mut strip = vec![match &worktree {
            Some(w) => format!("worktree {w}"),
            None => "main checkout".to_string(),
        }];
        if let Some(a) = i["lines_added"].as_u64() {
            strip.push(format!("+{a} \u{2212}{} lines", i["lines_removed"].as_u64().unwrap_or(0)));
        }
        if let Some(d) = i["duration_ms"].as_u64() {
            strip.push(took(d));
        }
        let pane = pane_of(panes, sb, thread);
        let question = (waiting && !st.is_empty()).then(|| {
            json!({
                "answerable": answerable,
                "steps": st.iter().map(|q| json!({
                    "text": s(q, "text"),
                    "options": q["options"].as_array().map(Vec::as_slice).unwrap_or(&[]).iter().enumerate().map(|(n, o)| {
                        let label = o.as_str().unwrap_or("");
                        // "label, em dash, description" (the daemon's decision table): the description.
                        let desc = q["context"][n].as_str().and_then(|c| c.split_once(" \u{2014} ")).map(|(_, d)| d.trim().to_string());
                        json!({ "label": label, "desc": desc })
                    }).collect::<Vec<_>>(),
                })).collect::<Vec<_>>(),
            })
        });
        json!({
            "key": key,
            "rev": c.rev,
            "sandbox": sb,
            "thread": thread,
            "name": thread.unwrap_or("main"),
            "session": session_key(sb, thread),
            "state": s(i, "state"),
            "word": g.word(),
            "tone": g.tone(),
            "model": opt(i, "model_name").map(str::to_string).or_else(|| opt(i, "model").map(|m| m.trim_start_matches("claude-").to_string())),
            "cost": i["cost_usd"].as_f64().map(|c| format!("${c:.2}")),
            "strip": strip.join("  \u{b7}  "),
            "last_input": opt(i, "last_input"),
            "now": if g == Group::Working { Some(opt(i, "activity").unwrap_or("Working")) } else { None },
            "reply": opt(i, "reply"),
            "ts": i["ts"].as_u64().unwrap_or(0),
            "question": question,
            "permission": (waiting && st.is_empty()).then(|| opt(i, "activity").unwrap_or("Claude needs your permission.")),
            "compose": { "can": why.is_none(), "why": why, "target": who(sb, thread) },
            "sent": self.sent.get(key).copied().unwrap_or(0),
            "worktree": worktree,
            "pane": pane,
        })
    }
}

/// The relay's key for a session (`web-app`, `web-app::t2`): its picks are kept by it.
pub fn session_key(sandbox: &str, thread: Option<&str>) -> String {
    crate::feat::relay::session_key(sandbox, thread)
}

/// The pane (from 1) that shows that session's agent, if one does.
pub fn pane_of(panes: &J, sandbox: &str, thread: Option<&str>) -> Option<u64> {
    panes.as_array()?.iter().find(|p| p["sandbox"] == sandbox && p["mode"] == "agent" && p["thread"].as_str() == thread).and_then(|p| p["index"].as_u64())
}

/// A wall-clock time in words: `12 min`, `2 h 5 min`.
pub fn took(ms: u64) -> String {
    let m = ms / 60_000;
    if m < 60 {
        format!("{m} min")
    } else {
        format!("{} h {} min", m / 60, m % 60)
    }
}

/// One row of the list, from a report.
fn row_of(key: &str, rev: u64, i: &J) -> J {
    let g = Group::of(s(i, "state"));
    let st = steps(i);
    let reply = opt(i, "reply").and_then(first_line);
    let (line, mono) = match g {
        Group::You => (st.first().and_then(|q| opt(q, "text")).or(opt(i, "activity")).unwrap_or("Waiting for you"), false),
        Group::Working => (opt(i, "activity").unwrap_or("Working"), true),
        Group::Ended => ("Ended", false),
        Group::Idle => (reply.or(opt(i, "last_input")).unwrap_or("Idle"), false),
    };
    let mut m = Map::new();
    m.insert("key".into(), key.into());
    m.insert("rev".into(), rev.into());
    m.insert("sandbox".into(), s(i, "sandbox").into());
    m.insert("name".into(), thread_of(i).unwrap_or("main").into());
    m.insert("thread".into(), thread_of(i).is_some().into());
    m.insert("group".into(), g.id().into());
    m.insert("tone".into(), g.tone().into());
    m.insert("line".into(), line.into());
    m.insert("mono".into(), mono.into());
    m.insert("ts".into(), i["ts"].as_u64().unwrap_or(0).into());
    if g == Group::You {
        m.insert("ask".into(), if st.is_empty() { "Wants permission" } else { "Asks you a question" }.into());
    }
    J::Object(m)
}

/// The clock the view's times are counted from: now, or, for a fixture
/// whose reports are all older than a week (fixed times), a minute after
/// the latest of them.
pub fn now_for(model: &Model, wall: u64) -> u64 {
    let latest = model.cards.values().filter_map(|c| c.info["ts"].as_u64()).max().unwrap_or(0);
    if latest > 0 && wall.saturating_sub(latest) > 7 * 24 * 3600 * 1000 {
        latest + 60_000
    } else {
        wall
    }
}
