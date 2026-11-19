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

/// Text the harness typed into a session, not the person: a message from
/// another session (`<cross-session-message from-name="lead">...`), a task
/// notification, a system reminder, a slash command's echo. `Some((from,
/// body))`: who sent it, when it says, and its words without the tags.
pub fn injected(text: &str) -> Option<(Option<String>, String)> {
    const TAGS: &[&str] = &["cross-session-message", "task-notification", "system-reminder", "local-command-stdout", "local-command-caveat", "command-name", "command-message", "user-prompt-submit-hook"];
    let t = text.trim_start();
    let rest = t.strip_prefix('<')?;
    let rest = rest.strip_prefix('\\').unwrap_or(rest);
    let tag = TAGS.iter().find(|g| rest.starts_with(**g) && rest[g.len()..].starts_with(|c: char| c == ' ' || c == '>' || c == '\n'))?;
    let head_end = rest.find('>').unwrap_or(rest.len());
    let head = &rest[..head_end];
    let from = head.split("from-name=\"").nth(1).and_then(|x| x.split('"').next()).map(|x| x.chars().filter(|c| !c.is_control()).take(60).collect::<String>()).filter(|x| !x.is_empty());
    let body = rest.get(head_end + 1..).unwrap_or("");
    let body = body.split(&format!("</{tag}")).next().unwrap_or("").split(&format!("<\\/{tag}")).next().unwrap_or("").trim().to_string();
    Some((from.or_else(|| (*tag != "cross-session-message").then(|| "sbxw".to_string())), body))
}

/// Who typed a report's `last_input` when it was not the person: the
/// daemon's word when it gives it (`last_input_from`, `last_input_sender`),
/// else the app's reading of the text ([`injected`]). `(name, words)`.
pub fn from_of(info: &J) -> Option<(String, String)> {
    let text = opt(info, "last_input")?;
    let sniffed = injected(text);
    match opt(info, "last_input_from") {
        Some("person") => None,
        Some("injected") => {
            let name = opt(info, "last_input_sender").map(|n| n.chars().filter(|c| !c.is_control()).take(60).collect::<String>())
                .or_else(|| sniffed.as_ref().and_then(|(f, _)| f.clone())).unwrap_or_else(|| "another session".into());
            Some((name, sniffed.map(|(_, b)| b).unwrap_or_else(|| text.to_string())))
        }
        _ => sniffed.map(|(f, b)| (f.unwrap_or_else(|| "another session".into()), b)),
    }
}

/// A report the daemon marks `ended: true` (when it serves that field) is an end.
fn ended_as_exited(info: &J) -> J {
    let mut i = info.clone();
    if i["ended"] == true {
        i["state"] = "exited".into();
    }
    i
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
    /// Files attached for each card, by its key ([`super::attach`]).
    pub att: HashMap<String, Vec<super::attach::Att>>,
    /// The next attachment's id, and pasted pictures so far (their names).
    pub att_seq: u64,
    pub pastes: u64,
    /// The card whose Attach opened the file dialog.
    pub pick_for: Option<String>,
    /// Sends with files under way, by number (`attach:<n>`).
    pub jobs: BTreeMap<u64, super::attach::Job>,
    pub job_seq: u64,
    /// The team leads filter while it is on ([`Model::sync_leads`]).
    pub leads: Option<Leads>,
}

/// The rail's Team leads, applied to Sessions: the core's one state
/// (`data.app.leads`, shell/leads.rs), so the terminals and Sessions are
/// filtered together.
#[derive(Debug, Clone, Default)]
pub struct Leads {
    /// The running sandboxes that lead a team, in the core's order (its
    /// rule: `App::leaders`, the same as the terminals' view).
    pub leaders: Vec<String>,
    /// The selection before the filter went on, put back when it goes off.
    pub before: Option<String>,
}

impl Model {
    /// The card a report belongs to: the same thread of the same sandbox,
    /// else the same session id, else the sandbox's session without a thread.
    /// The card a report belongs to, and whether it is news for it (a
    /// report older than what the card shows, from another session in the
    /// same place, is not). One card per place, as the sidebar folds them: a
    /// sandbox's thread, or its main checkout. A new session in the same
    /// place (resumed, restarted, a session the daemon never saw end) takes
    /// that card over: its key, so the selection, the draft and the attached
    /// files stay with the place. Only two sessions both live (working or
    /// waiting on the person) in one main checkout make two cards.
    fn card_for(&self, info: &J) -> Option<(String, bool)> {
        let sb = opt(info, "sandbox")?;
        if sb == JARVIS {
            return None;
        }
        let thread = thread_of(info);
        let sid = opt(info, "session_id");
        let ts = info["ts"].as_u64().unwrap_or(0);
        let live = |i: &J| matches!(s(i, "state"), "working" | "attention");
        let place: Vec<(&String, &Card)> = self.cards.iter().filter(|(_, c)| s(&c.info, "sandbox") == sb && thread_of(&c.info) == thread).collect();
        if let Some((k, _)) = place.iter().find(|(_, c)| sid.is_some() && opt(&c.info, "session_id") == sid) {
            return Some(((*k).clone(), true));
        }
        // The daemon says which session this one continues (`continues`, when it serves it).
        if let Some(prev) = opt(info, "continues") {
            if let Some((k, _)) = self.cards.iter().find(|(_, c)| s(&c.info, "sandbox") == sb && opt(&c.info, "session_id") == Some(prev)) {
                return Some((k.clone(), true));
            }
        }
        // The end of a session no card shows (an older one of the place): nothing to change.
        if sid.is_some() && s(info, "state") == "exited" {
            return place.first().map(|(k, _)| ((*k).clone(), false));
        }
        let fresh = |_: &str| match (thread, sid) {
            (Some(t), _) => format!("{sb}::{t}"),
            (None, Some(id)) => format!("{sb}:{id}"),
            (None, None) => sb.to_string(),
        };
        // Two live sessions in one main checkout: two terminals, two cards.
        if thread.is_none() && live(info) && !place.is_empty() && place.iter().all(|(_, c)| live(&c.info)) {
            return Some((fresh(""), true));
        }
        match place.iter().max_by_key(|(_, c)| c.info["ts"].as_u64().unwrap_or(0)) {
            // An end that names no session ends the place's.
            Some((k, c)) => Some(((*k).clone(), ts >= c.info["ts"].as_u64().unwrap_or(0) || (sid.is_none() && s(info, "state") == "exited"))),
            None => Some((fresh(""), true)),
        }
    }

    /// A `session` event: that one card changes (a whole report replaces
    /// what was known; a partial one, without `started_ms`, is merged into
    /// it). An ended session's card leaves when another card holds its
    /// place; the place's last card stays, ended, for the next session there.
    pub fn upsert(&mut self, info: &J) -> bool {
        let info = &ended_as_exited(info);
        let Some((key, news)) = self.card_for(info) else { return false };
        if !news {
            return false;
        }
        if s(info, "state") == "exited" {
            let (sb, thread) = (s(info, "sandbox").to_string(), thread_of(info).map(str::to_string));
            let others = self.cards.iter().filter(|(k, c)| **k != key && s(&c.info, "sandbox") == sb && thread_of(&c.info) == thread.as_deref()).count();
            if others > 0 && opt(&self.cards.get(&key).map(|c| c.info.clone()).unwrap_or(J::Null), "session_id") == opt(info, "session_id") {
                self.cards.remove(&key);
                if self.sel.as_deref() == Some(key.as_str()) {
                    self.sel = None;
                }
                self.next += 1;
                return true;
            }
        }
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
        // The oldest first: in each place, the latest report is the one shown.
        let mut list: Vec<&J> = list.as_array().map(|v| v.iter().collect()).unwrap_or_default();
        list.sort_by_key(|i| i["ts"].as_u64().unwrap_or(0));
        for info in list {
            let info = &ended_as_exited(info);
            if info["state"] == "exited" {
                continue;
            }
            let Some((key, news)) = self.card_for(info) else { continue };
            if news {
                changed |= self.put(key.clone(), info.clone());
            }
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

    /// The core's team leads state (`data.app.leads`: `{on, leaders}`),
    /// followed: on, the list keeps the leaders' sessions, each followed by
    /// its threads' (a leader with its team), and a selection filtered out
    /// moves to the first leader; off, the selection from before comes back.
    pub fn sync_leads(&mut self, app: &J) {
        let on = app["leads"]["on"] == true;
        match (on, self.leads.is_some()) {
            (true, _) => {
                let leaders: Vec<String> = app["leads"]["leaders"].as_array().map(|l| l.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default();
                let before = match self.leads.take() {
                    Some(l) => l.before,
                    None => self.sel.clone(),
                };
                self.leads = Some(Leads { leaders, before });
                let order = self.order();
                if !self.sel.as_ref().is_some_and(|k| order.contains(k)) {
                    self.sel = order.first().cloned();
                }
                self.pages = 1;
            }
            (false, true) => {
                let before = self.leads.take().and_then(|l| l.before);
                if before.as_ref().is_some_and(|k| self.cards.contains_key(k)) {
                    self.sel = before;
                }
                // The selection kept on a drawn page.
                let order = self.order();
                if let Some(j) = self.sel.as_ref().and_then(|k| order.iter().position(|o| o == k)) {
                    self.pages = self.pages.max(j / PAGE + 1);
                }
            }
            (false, false) => {}
        }
    }

    /// A card the team leads filter keeps: a leader's main session, or one
    /// of its threads (`Some(true)`: a thread, drawn under its leader).
    fn led(&self, key: &str) -> Option<bool> {
        let l = self.leads.as_ref()?;
        let c = self.cards.get(key)?;
        l.leaders.iter().any(|x| x == s(&c.info, "sandbox")).then(|| thread_of(&c.info).is_some())
    }

    /// Every card's key, in the list's order: those that need the person
    /// first (the longest waiting at the top), then working, idle and ended
    /// (the latest report first). With the team leads filter on: each
    /// leader (the core's order), its main session first, then its threads.
    pub fn order(&self) -> Vec<String> {
        let all = self.order_all();
        let Some(l) = &self.leads else { return all };
        let mut out = vec![];
        for sb in &l.leaders {
            let of = |thread: bool| all.iter().filter(|k| s(&self.cards[*k].info, "sandbox") == sb && thread_of(&self.cards[*k].info).is_some() == thread).cloned().collect::<Vec<_>>();
            let mains = of(false);
            // A leader shows only while its main session is listed.
            if mains.is_empty() {
                continue;
            }
            out.extend(mains);
            out.extend(of(true));
        }
        out
    }

    fn order_all(&self) -> Vec<String> {
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
        self.sel.clone().filter(|k| self.cards.contains_key(k) && (self.leads.is_none() || self.led(k).is_some())).or_else(|| self.order().into_iter().next())
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
            "rows": rows.iter().map(|k| {
                let mut r = self.cards[k].row.clone();
                if let Some(n) = self.told_apart(k) {
                    r["name"] = n.into();
                }
                // Team leads on: a thread under its leader, one step in.
                if self.led(k) == Some(true) {
                    r["indent"] = true.into();
                }
                r
            }).collect::<Vec<_>>(),
            "leads": self.leads.as_ref().map(|l| json!({ "on": true, "leaders": order.iter().filter(|k| self.led(k) == Some(false)).count(), "running": l.leaders.len() })).unwrap_or(json!({ "on": false })),
            "count": count,
            "counts": { "you": you, "working": working, "idle": idle, "ended": ended, "total": self.cards.len() },
            "sel": sel,
            "detail": sel.as_ref().map(|k| self.detail(k, panes)).unwrap_or(J::Null),
        })
    }

    /// Two live sessions in one main checkout: each row's name says which
    /// (`main · 3f2a`, the end of its session id).
    fn told_apart(&self, key: &str) -> Option<String> {
        let c = self.cards.get(key)?;
        if thread_of(&c.info).is_some() {
            return None;
        }
        let sb = s(&c.info, "sandbox");
        let n = self.cards.values().filter(|o| s(&o.info, "sandbox") == sb && thread_of(&o.info).is_none()).count();
        let sid: Vec<char> = opt(&c.info, "session_id")?.chars().filter(|ch| ch.is_ascii_alphanumeric()).collect();
        (n > 1).then(|| format!("main \u{b7} {}", sid[sid.len().saturating_sub(4)..].iter().collect::<String>()))
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
            "name": self.told_apart(key).unwrap_or_else(|| thread.unwrap_or("main").to_string()),
            "session": session_key(sb, thread),
            "state": s(i, "state"),
            "word": g.word(),
            "tone": g.tone(),
            "model": opt(i, "model_name").map(str::to_string).or_else(|| opt(i, "model").map(|m| m.trim_start_matches("claude-").to_string())),
            "cost": i["cost_usd"].as_f64().map(|c| format!("${c:.2}")),
            "strip": strip.join("  \u{b7}  "),
            // The person's last prompt only: a message another session or
            // the harness typed in is shown as that, quietly, never as "You".
            "last_input": opt(i, "last_input").filter(|_| from_of(i).is_none()),
            "from": from_of(i).map(|(from, body)| json!({ "name": from, "text": crate::feat::monitor::attach::clipped_text(&body, 600) })),
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
    let prompt: Option<String> = opt(i, "last_input").map(|t| match from_of(i) {
        Some((from, _)) => format!("From {from}"),
        None => t.to_string(),
    });
    let (line, mono) = match g {
        Group::You => (st.first().and_then(|q| opt(q, "text")).or(opt(i, "activity")).unwrap_or("Waiting for you"), false),
        Group::Working => (opt(i, "activity").unwrap_or("Working"), true),
        Group::Ended => ("Ended", false),
        Group::Idle => (reply.or(prompt.as_deref()).unwrap_or("Idle"), false),
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
