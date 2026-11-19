//! A session's conversation, as the daemon keeps it (`GET
//! /api/sessions/:sandbox/:session_id/history`, docs/session-history.md):
//! its pages merged by `seq`, and the rows Sessions draws from them, in the
//! terminal's reading order (oldest at the top, opening at the newest).
//!
//! * **Only the selected session is read.** [`Store::sub`] is the one
//!   session whose history is fetched and whose subscription is renewed
//!   (each read renews it for 120 s); another selected, the view left, its
//!   subscription is deleted. A few recently viewed logs stay in memory
//!   ([`LRU`]) so going back to one draws at once; older ones are dropped.
//! * **Nothing is written anywhere.** The texts are the daemon's, already
//!   masked: kept in memory only, never logged, never on disk.
//! * **Agent text is data.** Every row carries text for the screen to draw
//!   as text (and the prose face with pills); the only things a row does
//!   are its own fold and unfold.

use serde_json::{json, Value as J};
use std::collections::{BTreeMap, HashSet, VecDeque};
use std::hash::{Hash, Hasher};

/// Items a page, the first one and each older one.
pub const PAGE: u64 = 100;
/// Logs kept in memory besides the selected one (recently viewed).
pub const LRU: usize = 3;
/// Rows drawn from the newest; "Show earlier" draws this many more.
pub const WINDOW: usize = 60;
/// An injected message longer than this is folded to its head.
pub const FOLD_CHARS: usize = 360;
pub const FOLD_LINES: usize = 5;
/// A step's result head, drawn when the step is open, at most.
pub const HEAD_CHARS: usize = 2000;
/// A read older than this is renewed by the session's next event (the
/// daemon's subscription lasts 120 s).
pub const RENEW_MS: u64 = 90_000;

/// Which session: `(sandbox, session_id)`.
pub type Sid = (String, String);

/// One session's log as far as it was read.
#[derive(Debug, Default, Clone)]
pub struct Hist {
    pub items: BTreeMap<u64, J>,
    /// `before` for the next older page; `None`: the log's start is here (or nothing was read yet).
    pub next_before: Option<u64>,
    pub loaded: bool,
    /// A read under way: the newest page, an older one.
    pub loading: bool,
    pub older_loading: bool,
    /// A nudge came while a read ran: read the newest page again after it.
    pub again: bool,
    pub truncated: bool,
    /// `partial_reason`'s code (`old_hook`, `no_transcript`, `catching_up`) while the log is partial.
    pub partial: Option<String>,
    pub ended: bool,
    /// The daemon holds nothing for it (404).
    pub missing: bool,
    /// The last read failed (what is shown stays).
    pub error: Option<String>,
    /// Rows drawn from the newest.
    pub window: usize,
    /// Rows unfolded by the person (a step group, a step, a long message).
    pub open: HashSet<String>,
    /// Moves whenever the rows would change.
    pub rev: u64,
    /// When the last read was asked (ms), to renew the subscription.
    pub read_ms: u64,
    built: Option<(u64, u64, J)>,
}

/// The logs in memory and the session followed.
#[derive(Debug, Default)]
pub struct Store {
    pub logs: BTreeMap<Sid, Hist>,
    /// The most recent first; at most `1 + LRU`.
    pub recent: VecDeque<Sid>,
    /// The session whose history is read and subscribed to.
    pub sub: Option<Sid>,
}

impl Store {
    /// That session's log, made the most recent; the least recent beyond
    /// the bound are dropped.
    pub fn touch(&mut self, id: &Sid) -> &mut Hist {
        self.recent.retain(|x| x != id);
        self.recent.push_front(id.clone());
        while self.recent.len() > 1 + LRU {
            if let Some(gone) = self.recent.pop_back() {
                self.logs.remove(&gone);
            }
        }
        self.logs.entry(id.clone()).or_insert_with(|| Hist { window: WINDOW, ..Hist::default() })
    }

    pub fn get(&self, id: &Sid) -> Option<&Hist> {
        self.logs.get(id)
    }
}

fn seq(i: &J) -> u64 {
    i["seq"].as_u64().unwrap_or(0)
}

/// The code of a `partial_reason` (`old_hook: ...` gives `old_hook`).
pub fn reason_code(r: &str) -> String {
    let c: String = r.split(':').next().unwrap_or("").trim().chars().filter(|c| c.is_ascii_alphanumeric() || *c == '_').take(32).collect();
    if c.is_empty() { "unknown".into() } else { c }
}

impl Hist {
    /// A page the daemon answered (`newest`: the first page, read again on
    /// each nudge; else an older one). Seqs never repeat, items can go: a
    /// provisional reply is replaced, a reset restarts the log.
    pub fn merge(&mut self, page: &J, newest: bool) {
        let items: Vec<J> = page["items"].as_array().cloned().unwrap_or_default();
        let lo = items.first().map(seq);
        if newest {
            let latest = page["latest_seq"].as_u64().unwrap_or(0);
            let ours = self.items.keys().next_back().copied().unwrap_or(0);
            let reset = latest < ours;
            match lo {
                // What the page covers is the page's: an item there it no longer has is gone.
                Some(lo) if !reset => {
                    self.items.retain(|s, _| *s < lo);
                    // Ours end before the page starts and more lies between: start from the page.
                    if page["next_before"].is_u64() && !self.items.is_empty() && self.items.keys().next_back().is_some_and(|m| m + 1 < lo) {
                        self.items.clear();
                    }
                }
                _ => self.items.clear(),
            }
            if self.items.is_empty() {
                self.next_before = page["next_before"].as_u64();
            }
        } else {
            self.next_before = page["next_before"].as_u64();
        }
        for i in items {
            self.items.insert(seq(&i), i);
        }
        self.loaded = true;
        self.missing = false;
        self.error = None;
        self.truncated = page["truncated_before"] == true;
        self.partial = (page["partial"] == true).then(|| reason_code(page["partial_reason"].as_str().unwrap_or("")));
        self.ended = page["ended"] == true;
        self.rev += 1;
    }

    /// Nothing held for that session (404): an empty log.
    pub fn none(&mut self) {
        self.items.clear();
        self.next_before = None;
        self.loaded = true;
        self.missing = true;
        self.error = None;
        self.rev += 1;
    }

    pub fn fail(&mut self, why: String) {
        self.error = Some(why);
        self.rev += 1;
    }

    /// A row unfolded or folded again.
    pub fn toggle(&mut self, id: &str) {
        if !self.open.remove(id) {
            self.open.insert(id.to_string());
        }
        self.rev += 1;
    }

    /// The rows to draw (only the newest `window`), built again only when
    /// the log or what is open changed. `now_s`: for the day separators.
    pub fn rows(&mut self, now_s: u64) -> J {
        let day = now_s / 86_400;
        if let Some((rev, d, v)) = &self.built {
            if *rev == self.rev && *d == day {
                return v.clone();
            }
        }
        let all = build(self.items.values(), &self.open, now_s);
        let hidden = all.len().saturating_sub(self.window);
        let v = json!({ "rows": all[hidden..].to_vec(), "hidden": hidden, "total": all.len() });
        self.built = Some((self.rev, day, v.clone()));
        v
    }
}

/// The icon of a tool's family.
pub fn tool_icon(name: &str) -> &'static str {
    match name {
        "Bash" | "BashOutput" | "KillShell" => "terminal",
        "Read" => "book",
        "Glob" | "Grep" | "LS" => "search",
        "Edit" | "MultiEdit" | "Write" | "NotebookEdit" => "file",
        "WebFetch" | "WebSearch" => "globe",
        "Task" | "Agent" => "people",
        "TodoWrite" => "check",
        n if n.starts_with("mcp__") => "plug",
        _ => "gear",
    }
}

/// Text cut to `n` characters, and whether it was.
fn cut(t: &str, chars: usize, lines: usize) -> (String, bool) {
    let mut s: String = t.lines().take(lines).collect::<Vec<_>>().join("\n");
    let mut cutted = t.lines().count() > lines;
    if let Some((i, _)) = s.char_indices().nth(chars) {
        s.truncate(i);
        cutted = true;
    }
    (s, cutted)
}

fn sig_of(v: &J) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    v.to_string().hash(&mut h);
    h.finish()
}

fn with_sig(mut v: J) -> J {
    let s = sig_of(&v);
    v["sig"] = s.to_string().into();
    v
}

/// One step: a tool call and its result (same `id`), or a result alone.
fn step(call: Option<&J>, result: Option<&J>, open: &HashSet<String>) -> J {
    let first = call.or(result).unwrap();
    let id = format!("s{}", seq(first));
    let name = call.and_then(|c| c["tool"]["name"].as_str()).unwrap_or("");
    let summary = call.and_then(|c| c["tool"]["summary"].as_str()).filter(|s| !s.trim().is_empty()).map(str::to_string).unwrap_or_else(|| if name.is_empty() { "A step".into() } else { name.to_string() });
    let r = result.map(|r| &r["result"]);
    let status = match r.and_then(|r| r["status"].as_str()) {
        Some("error") => "error",
        Some(_) => "ok",
        None => "running",
    };
    let is_open = open.contains(&id);
    let head = r.and_then(|r| r["head"].as_str()).unwrap_or("");
    let mut v = json!({
        "t": "step", "id": id, "icon": tool_icon(name), "summary": summary, "status": status,
        "size": r.and_then(|r| r["size"].as_u64()).map(crate::feat::files::view::size_label),
        "has_head": !head.is_empty(), "open": is_open, "at": first["at"].as_u64().unwrap_or(0),
    });
    if is_open && !head.is_empty() {
        v["head"] = cut(head, HEAD_CHARS, 400).0.into();
    }
    with_sig(v)
}

enum Entry<'a> {
    Step(Option<&'a J>, Option<&'a J>),
    One(&'a J),
}

/// The rows from a log's items, oldest first: day separators, the
/// person's prompts, messages typed in by another session, Claude's text,
/// steps (consecutive ones folded into a group), system lines.
pub fn build<'a>(items: impl Iterator<Item = &'a J>, open: &HashSet<String>, now_s: u64) -> Vec<J> {
    let items: Vec<&J> = items.collect();
    // A question the session asked is drawn from its `question` item: its tool call is not a step.
    let asked = items.iter().any(|i| i["kind"] == "question");
    let mut entries: Vec<Entry> = vec![];
    let mut calls: BTreeMap<String, usize> = BTreeMap::new();
    let mut hidden_ids: HashSet<String> = HashSet::new();
    for i in &items {
        match i["kind"].as_str().unwrap_or("") {
            "tool_call" => {
                let tid = i["tool"]["id"].as_str().unwrap_or("").to_string();
                if asked && i["tool"]["name"] == "AskUserQuestion" {
                    hidden_ids.insert(tid);
                    continue;
                }
                if !tid.is_empty() {
                    calls.insert(tid, entries.len());
                }
                entries.push(Entry::Step(Some(i), None));
            }
            "tool_result" => {
                let tid = i["result"]["id"].as_str().unwrap_or("");
                if hidden_ids.contains(tid) {
                    continue;
                }
                match calls.get(tid).copied() {
                    Some(at) if !tid.is_empty() => {
                        if let Entry::Step(c, None) = entries[at] {
                            entries[at] = Entry::Step(c, Some(i));
                        } else {
                            entries.push(Entry::Step(None, Some(i)));
                        }
                    }
                    _ => entries.push(Entry::Step(None, Some(i))),
                }
            }
            _ => entries.push(Entry::One(i)),
        }
    }
    let mut out: Vec<J> = vec![];
    let mut last_day: Option<u64> = None;
    let mut k = 0;
    let at_of = |e: &Entry| match e {
        Entry::Step(c, r) => c.or(*r).map(|x| x["at"].as_u64().unwrap_or(0)).unwrap_or(0),
        Entry::One(i) => i["at"].as_u64().unwrap_or(0),
    };
    while k < entries.len() {
        let at = at_of(&entries[k]);
        let d = at / 1000 / 86_400;
        if at > 0 && last_day != Some(d) {
            out.push(with_sig(json!({ "t": "day", "id": format!("day-{d}"), "label": crate::feat::board::model::day_label(at, now_s) })));
            last_day = Some(d);
        }
        match &entries[k] {
            Entry::Step(..) => {
                // The run of steps from here (a day change ends it).
                let mut j = k;
                let mut run = vec![];
                while j < entries.len() {
                    let Entry::Step(c, r) = &entries[j] else { break };
                    if j > k && at_of(&entries[j]) / 1000 / 86_400 != d && at_of(&entries[j]) > 0 {
                        break;
                    }
                    run.push(step(*c, *r, open));
                    j += 1;
                }
                if run.len() == 1 {
                    out.push(run.pop().unwrap());
                } else {
                    let id = format!("g{}", run[0]["id"].as_str().unwrap_or("").trim_start_matches('s'));
                    let last = run.last().unwrap();
                    let status = if run.iter().any(|s| s["status"] == "error") { "error" } else { last["status"].as_str().unwrap_or("ok") };
                    let is_open = open.contains(&id);
                    let mut g = json!({ "t": "group", "id": id, "n": run.len(), "summary": last["summary"], "icon": last["icon"], "status": status, "errors": run.iter().filter(|s| s["status"] == "error").count(), "open": is_open, "at": last["at"] });
                    if is_open {
                        g["steps"] = J::Array(run);
                    }
                    out.push(with_sig(g));
                }
                k = j;
                continue;
            }
            Entry::One(i) => {
                let id = seq(i).to_string();
                let text = i["text"].as_str().unwrap_or("");
                let row = match i["kind"].as_str().unwrap_or("") {
                    "user_prompt" => Some(json!({ "t": "you", "id": id, "text": text, "at": at })),
                    "injected" => {
                        // Typed in by another session or the harness: never "You", never the raw tag.
                        let body = super::model::injected(text).map(|(_, b)| b).unwrap_or_else(|| text.to_string());
                        let name: String = i["from"].as_str().map(|f| f.chars().filter(|c| !c.is_control()).take(60).collect()).filter(|f: &String| !f.trim().is_empty()).unwrap_or_else(|| "another session".into());
                        let is_open = open.contains(&id);
                        let (head, long) = cut(&body, FOLD_CHARS, FOLD_LINES);
                        Some(json!({ "t": "from", "id": id, "name": name, "text": if is_open || !long { body.clone() } else { head }, "long": long, "open": is_open, "at": at }))
                    }
                    "assistant_text" => Some(json!({ "t": "claude", "id": id, "text": text, "at": at, "provisional": i["provisional"] == true })),
                    "compact_marker" => Some(json!({ "t": "system", "id": id, "icon": "history", "text": "Conversation compacted: what came before is summarised for Claude", "at": at })),
                    "notice" => Some(json!({ "t": "system", "id": id, "icon": "info", "text": if text.is_empty() { "A notice" } else { text }, "at": at })),
                    "question" => {
                        let q = i["steps"][0]["text"].as_str().or(Some(text)).filter(|t| !t.is_empty()).unwrap_or("a question");
                        Some(json!({ "t": "system", "id": id, "icon": "ask", "text": format!("Asked: {q}"), "at": at }))
                    }
                    "permission" => Some(json!({ "t": "system", "id": id, "icon": "warning", "text": if text.is_empty() { "Asked for a permission".to_string() } else { text.to_string() }, "at": at })),
                    _ => None,
                };
                if let Some(r) = row {
                    out.push(with_sig(r));
                }
            }
        }
        k += 1;
    }
    out
}

/// The line under a partial log: why, and what to do.
pub fn partial_line(code: &str, sandbox: &str) -> String {
    match code {
        "old_hook" => format!("Only the last prompt and reply: this sandbox's status hook is older and does not read the conversation. Run sbxw up {sandbox} to get it."),
        "no_transcript" => "Only the last prompt and reply: the hook could not read this session's transcript, so sbxw keeps its events only.".into(),
        "catching_up" => "The conversation is on its way: the hook has not sent this session's transcript yet.".into(),
        _ => "Only the last prompt and reply: sbxw does not have the whole conversation of this session.".into(),
    }
}
