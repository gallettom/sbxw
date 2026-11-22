//! The Board's view model: a team's timeline (the daemon's events, oldest
//! first) as rows of a conversation, the side column (the team, the notes
//! that still stand, the commits per thread and their lane graph). Pure:
//! JSON in, JSON out, so it is tested without a window.
//!
//! Every text here is data an agent, a person or git wrote: the screen
//! draws it in the prose face (with pills for agents' words), never as a
//! control. File paths and hashes only come from the daemon's answers.

use crate::feat::files::view::{ago, date};
use crate::feat::worktrees::graph::lane_color;
use serde_json::{json, Value as J};
use std::collections::BTreeSet;

/// Same author, same addressee, within this many seconds: one author line.
pub const GROUP_SECS: u64 = 5 * 60;
/// A message drawn on more than `FOLD_DRAWN` lines (as drawn at the bubble's
/// width, a blank line counting as one) is folded with "Show more"; folded,
/// it shows its first `SHOW_DRAWN`.
pub const FOLD_DRAWN: usize = 8;
pub const SHOW_DRAWN: usize = 6;
/// The prose face at 12 pt: about this many points a character (measured: 5.7 at 12 pt, 6.2 at 13 pt).
pub const CHAR_W: f32 = 6.0;
/// A bubble's widest in one column (under 760 pt), and in two: the share of the
/// conversation column, at least `BUBBLE_MIN` and at most `BUBBLE_CAP`
/// (mirrored in screens/board.luau, which draws it).
pub const BUBBLE_MIN: f32 = 520.0;
pub const BUBBLE_CAP: f32 = 720.0;
pub const BUBBLE_SHARE: f32 = 0.62;
/// The width the fold counts with when the view does not say (a bubble at 520 pt).
pub const DEFAULT_CPL: usize = 82;
/// Characters a line of the narrowest bubble holds (about 264 pt): a text
/// that would not fold there never gets a fold.
pub const FOLD_CPL: usize = 40;

/// The conversation column's width in an overlay `ow` pt wide: the whole
/// overlay less its padding (one column; the summary sits above it).
pub fn conv_w(ow: f32) -> f32 {
    ow - 24.0
}

/// The Commits filter's lane graph: the conversation's width, 260 to 960 pt
/// (past that the dots spread too far apart to read as one history).
pub fn lanes_w(ow: f32) -> f32 {
    (conv_w(ow) - 16.0).clamp(260.0, 960.0)
}

/// A bubble's widest in a conversation column `conv` pt wide in an overlay
/// `ow` wide; never past the column less its gutters.
pub fn bubble_max(conv: f32, ow: f32) -> f32 {
    let w = if ow >= 760.0 { (conv * BUBBLE_SHARE).floor().clamp(BUBBLE_MIN, BUBBLE_CAP) } else { BUBBLE_MIN };
    w.min(conv - 24.0).max(120.0)
}

/// Characters of prose a line of such a bubble holds (its 12 pt padding off).
pub fn chars_per_line(bubble: f32) -> usize {
    (((bubble - 24.0) / CHAR_W).floor() as usize).max(16)
}
/// Conversation rows drawn a page (the newest first kept), and commits a lane draws.
pub const ROWS_PAGE: usize = 80;
/// Pages drawn at most (240 rows, about 3,000 elements of the 5,000 a render
/// may return): older rows are found with a filter or a search.
pub const MAX_PAGES: usize = 3;
pub const LANE_DOTS: usize = 12;

/// Who an author is, as the view names and colours them.
pub fn who(from: &str) -> (String, &'static str) {
    match from {
        "user" => ("You".into(), "text-bright"),
        "lead" => ("lead".into(), "purple"),
        "sbxw" => ("sbxw".into(), "text-muted"),
        "team" => ("team".into(), "text-muted"),
        t => (t.to_string(), lane_color(t)),
    }
}

/// An author as the conversation names them: the name, its colour, and a
/// role chip only when the name does not already say it (the lead named
/// "lead" is "lead" once, never "lead lead"). `lead` is the team's lead's
/// name when the daemon knows one.
pub fn author(from: &str, lead: Option<&str>) -> (String, &'static str, Option<&'static str>) {
    if from == "lead" {
        let name = lead.map(str::trim).filter(|n| !n.is_empty()).unwrap_or("lead");
        let role = (!name.to_lowercase().contains("lead")).then_some("lead");
        return (name.to_string(), "purple", role);
    }
    let (n, c) = who(from);
    (n, c, None)
}

/// An addressee as the author line says it ("to the team", "to you").
pub fn addressee(to: &str, lead: Option<&str>) -> String {
    match to {
        "team" => "the team".into(),
        "user" => "you".into(),
        t => author(t, lead).0,
    }
}

/// A run of characters without a space longer than this gets break
/// opportunities (zero-width spaces) every [`BREAK_EVERY`] characters, so a
/// path, a hash or 200 characters without a space wrap inside their bubble
/// instead of running out of it. Drawing only: the record is untouched.
pub const BREAK_RUN: usize = 32;
pub const BREAK_EVERY: usize = 24;

pub fn breakable(text: &str) -> String {
    let long = text.split_whitespace().any(|w| w.chars().count() > BREAK_RUN);
    if !long {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len() + text.len() / 8);
    let mut run = 0;
    let mut pill = false;
    let mut prev = ' ';
    for ch in text.chars() {
        // Not inside a keyword pill (`[[Word: detail]]`): its brackets must stay whole.
        if prev == '[' && ch == '[' {
            pill = true;
        } else if prev == ']' && ch == ']' {
            pill = false;
        }
        if ch.is_whitespace() || ch == '\u{200b}' {
            run = 0;
        } else {
            if run >= BREAK_EVERY && !pill && ch != ']' && ch != '[' {
                out.push('\u{200b}');
                run = 0;
            }
            run += 1;
        }
        out.push(ch);
        prev = ch;
    }
    out
}

/// A team named by its threads ("api-routes + web-page"), the concise thing
/// sbxw has; past three, "+ n more". Its sandbox when it has no thread.
pub fn team_names(team: &J) -> String {
    let names: Vec<&str> = team["threads"].as_array().into_iter().flatten().filter_map(|t| t["name"].as_str()).collect();
    match names.len() {
        0 => team["sandbox"].as_str().unwrap_or("team").to_string(),
        1..=3 => names.join(" + "),
        n => format!("{} + {} more", names[..2].join(" + "), n - 2),
    }
}

/// Where a message sits: the person's acts on the right, sbxw's events in
/// the middle, the agents on the left.
pub fn side(ev: &J) -> &'static str {
    let kind = ev["kind"].as_str().unwrap_or("");
    match ev["from"].as_str().unwrap_or("") {
        "user" => "right",
        "sbxw" => "center",
        _ if matches!(kind, "claim" | "release") => "center",
        _ => "left",
    }
}

/// A thread state's word and tone (the sidebar's).
pub fn state_word(state: &str) -> (&'static str, &'static str) {
    match state {
        "working" => ("working", "blue"),
        "starting" => ("starting", "blue"),
        "blocked" => ("blocked", "amber"),
        "done" => ("done", "green"),
        "failed" => ("failed", "red"),
        _ => ("unknown", "neutral"),
    }
}

/// Where a team's state puts it in a list: what still needs the person
/// first (proposed: waits for their approval; running; done: waits for its
/// merge), then the finished ones (merged, closed), refused last. From
/// [`FINISHED`] on, a team is finished.
pub fn team_rank(state: &str) -> u8 {
    match state {
        "proposed" => 0,
        "running" => 1,
        "done" => 2,
        "merged" => 3,
        "closed" => 4,
        "refused" => 5,
        _ => 6,
    }
}
pub const FINISHED: u8 = 3;

/// The teams as every Board list shows them: by [`team_rank`], then the most
/// recent first.
pub fn order_teams(teams: &mut [J]) {
    let last = |t: &J| t["last_ms"].as_u64().or(t["updated_ms"].as_u64()).or(t["created_ms"].as_u64()).unwrap_or(0);
    teams.sort_by_key(|t| (team_rank(t["state"].as_str().unwrap_or("")), std::cmp::Reverse(last(t))));
}

/// A team's state's tone.
pub fn team_tone(state: &str) -> &'static str {
    match state {
        "proposed" => "amber",
        "running" => "blue",
        "done" | "merged" => "green",
        "refused" => "red",
        _ => "neutral",
    }
}

/// `14:03 UTC`, `2026-11-19 14:03 UTC`, the day's label.
pub fn clock(ms: u64) -> String {
    let s = ms / 1000;
    format!("{:02}:{:02}", (s / 3600) % 24, (s / 60) % 60)
}
pub fn exact(ms: u64) -> String {
    format!("{} {} UTC", date(ms / 1000), clock(ms))
}
pub fn day_label(ms: u64, now_s: u64) -> String {
    let (d, today) = (ms / 1000 / 86_400, now_s / 86_400);
    match today.saturating_sub(d) {
        0 => "Today".into(),
        1 => "Yesterday".into(),
        _ => date(ms / 1000),
    }
}

/// Lines a text takes drawn `cpl` characters wide: each of its lines (split
/// on `\n`, as the screen splits) wraps on `cpl`, an empty one counts as one.
fn drawn_lines(text: &str, cpl: usize) -> usize {
    text.split('\n').map(|l| l.chars().count().div_ceil(cpl.max(1)).max(1)).sum()
}

/// [`drawn_lines`], for the tests that hold the screen to the model.
#[cfg(test)]
pub fn drawn_lines_at(text: &str, cpl: usize) -> usize {
    drawn_lines(text, cpl)
}

fn folded(text: &str, cpl: usize) -> bool {
    drawn_lines(text, cpl) > FOLD_DRAWN
}

/// Bytes a cut looks back for a space, at most (the head ends on a word).
pub const HEAD_BACK: usize = 48;

/// The first part of a long text, its first `SHOW_DRAWN` drawn lines at
/// `cpl` characters a line, cut on a word when a space is near. The screen
/// cuts the same (board.luau's `head`, from [`drawing`]'s `lines`, in
/// constant steps; a test holds the two together).
pub fn head(text: &str, cpl: usize) -> String {
    let mut end = 0;
    let mut start = 0;
    let mut left = SHOW_DRAWN;
    for l in text.split('\n') {
        let n = l.chars().count().div_ceil(cpl).max(1);
        if n > left {
            let mut i = l.char_indices().nth(left * cpl).map_or(l.len(), |(i, _)| i);
            let low = (i / 2 + 1).max(i.saturating_sub(HEAD_BACK));
            if let Some(p) = (low..i).rev().find(|p| l.as_bytes()[*p] == b' ') {
                i = p;
            }
            end = start + i;
            break;
        }
        end = start + l.len();
        start += l.len() + 1;
        left -= n;
        if left == 0 {
            break;
        }
    }
    format!("{}…", text[..end].trim_end_matches([' ', '\t', '\n', '\r', '\x0b', '\x0c']))
}

/// What drawing a text takes, computed once per event when it arrives (the
/// screen never walks a text): the text with its long runs breakable, and
/// when it could fold (at [`FOLD_CPL`]) its `fold`: `cpl`, the fewest
/// characters a line at which it no longer folds (the screen folds it when
/// the line it draws holds fewer: one comparison), and `lines`, its first
/// `SHOW_DRAWN` lines as [byte start (from 1, as Luau counts), bytes,
/// characters], all the screen's `head` reads.
pub fn drawing(text: &str) -> J {
    let t = breakable(text);
    if !folded(&t, FOLD_CPL) {
        return json!({ "text": t });
    }
    let longest = t.split('\n').map(|l| l.chars().count()).max().unwrap_or(1).max(1);
    // Folded at every width: more lines than fold, however wide.
    let cpl = if folded(&t, longest) {
        usize::MAX >> 12
    } else {
        // drawn_lines falls as cpl grows: the first cpl that does not fold.
        let (mut lo, mut hi) = (FOLD_CPL, longest);
        while lo < hi {
            let mid = (lo + hi) / 2;
            if folded(&t, mid) { lo = mid + 1 } else { hi = mid }
        }
        lo
    };
    let mut lines = vec![];
    let mut b = 0;
    for l in t.split('\n').take(SHOW_DRAWN) {
        lines.push(json!([b + 1, l.len(), l.chars().count()]));
        b += l.len() + 1;
    }
    json!({ "text": t, "fold": { "cpl": cpl, "lines": lines } })
}

/// [`drawing`] of an event's own text: the one computed when it arrived
/// (`_draw`, [`prepare`]), else now.
fn drawing_of(ev: &J, text: &str) -> J {
    match &ev["_draw"] {
        d if !d.is_null() && ev["text"].as_str() == Some(text) => d.clone(),
        _ => drawing(text),
    }
}

/// An event as it arrives: what drawing its text takes, once (`_draw`,
/// never sent to the screen as such).
pub fn prepare(ev: &mut J) {
    if let Some(t) = ev["text"].as_str().filter(|t| !t.trim().is_empty()) {
        ev["_draw"] = drawing(t);
    }
}

/// What a kind says, its label and its tone, for notes and system lines.
fn label(kind: &str) -> (&'static str, &'static str) {
    match kind {
        "board_decision" => ("Decision", "green"),
        "board_warning" => ("Warning", "amber"),
        "board_question" => ("Question", "blue"),
        "board_handoff" => ("Handoff", "neutral"),
        "conflict" => ("Conflict", "red"),
        "plan_proposed" => ("Plan", "purple"),
        "plan_approved" => ("Approved the plan", "green"),
        "plan_refused" => ("Refused the plan", "red"),
        "thread_failed" => ("Could not start", "red"),
        "marked_done" => ("Marked done", "green"),
        "thread_closed" => ("Closed the thread", "neutral"),
        "team_closed" => ("Ended Team mode", "neutral"),
        "relay_question" => ("Asked another sandbox", "blue"),
        "relay_routed" => ("Sent the question", "blue"),
        "relay_drafted" => ("Answered, held for you", "blue"),
        "relay_answer" => ("Released the answer", "green"),
        "user_question" => ("Questions for you", "amber"),
        "user_answer" => ("Answered", "green"),
        "screenshot_asked" => ("Asked for a screenshot", "amber"),
        "screenshot_given" => ("Gave a screenshot", "green"),
        "relay_refused" => ("Refused the request", "red"),
        "merge_proposed" => ("Proposed the merge", "purple"),
        "merge_started" => ("Pressed Merge", "green"),
        "merge_result" => ("Merge", "neutral"),
        "merge_finished" => ("Merge", "neutral"),
        "claim" => ("Claimed", "neutral"),
        "release" => ("Released", "neutral"),
        "thread_status" => ("Status", "neutral"),
        _ => ("", "neutral"),
    }
}

/// The view's choices that shape rows.
#[derive(Default, Clone, Debug)]
pub struct Opts<'a> {
    pub expanded: Option<&'a BTreeSet<String>>,
    /// Showing every team: each row says whose it is.
    pub all: bool,
    pub now_s: u64,
    /// Pending relay requests still waiting (ids), for "answer it in Requests".
    pub waiting: Option<&'a BTreeSet<String>>,
    /// The decisions and warnings that still stand (their ids): marked in the flow.
    pub standing: Option<&'a BTreeSet<String>>,
}

impl Opts<'_> {
    fn open(&self, id: &str) -> bool {
        self.expanded.is_some_and(|e| e.contains(id))
    }
}

/// The base of every row: its id, side, author and times.
fn base(ev: &J, o: &Opts) -> J {
    let from = ev["from"].as_str().unwrap_or("");
    let lead = ev["lead_name"].as_str();
    let (name, color, role) = author(from, lead);
    let at = ev["at"].as_u64();
    let to = ev["to"].as_str().filter(|t| !t.is_empty()).map(|t| addressee(t, lead));
    json!({
        "id": ev["id"], "kind": ev["kind"], "side": side(ev), "who": name, "who_color": color, "role": role,
        "is_lead": from == "lead", "is_user": from == "user", "to": to,
        "when": at.map(|a| ago(a / 1000, o.now_s)).unwrap_or_else(|| "time unknown".into()),
        "exact": at.map(exact).unwrap_or_else(|| "sbxw did not record when (a plan from before this version)".into()),
        "guess": ev["guess"].as_bool().unwrap_or(false),
        "team": ev["team"], "team_label": ev["team_label"], "team_help": ev["team_help"], "team_color": ev["team_color"],
    })
}

/// The whole text, always (breakable); `fold` when it could fold at the
/// narrowest bubble drawn (`FOLD_CPL`), with what the screen needs to fold
/// it ([`drawing`]). Whether it folds is the screen's to say, at the width it
/// draws now, by one comparison: a width known only when the data was built
/// would refold other bubbles at the next press.
fn with_text(row: &mut J, ev: &J, id: &str, text: Option<&str>, o: &Opts) {
    let Some(t) = text.filter(|t| !t.trim().is_empty()) else { return };
    let mut d = drawing_of(ev, t);
    row["text"] = d["text"].take();
    if let J::Object(mut f) = d["fold"].take() {
        f.insert("open".into(), o.open(id).into());
        f.insert("key".into(), id.into());
        row["fold"] = J::Object(f);
    }
}

/// One event as one row (commits are grouped by [`rows`]).
pub fn row(ev: &J, o: &Opts) -> J {
    let id = ev["id"].as_str().unwrap_or("").to_string();
    let kind = ev["kind"].as_str().unwrap_or("");
    let data = &ev["data"];
    let mut r = base(ev, o);
    let (lab, tone) = label(kind);
    r["label"] = lab.into();
    r["tone"] = tone.into();
    let text = ev["text"].as_str();
    match kind {
        "plan_proposed" => {
            r["type"] = "card".into();
            with_text(&mut r, ev, &id, text, o);
            r["why"] = data["justification"].clone();
            r["variant"] = data["variant"].clone();
            let threads: Vec<J> = data["threads"].as_array().into_iter().flatten().map(|t| {
                let n = t["name"].as_str().unwrap_or("");
                json!({ "name": n, "color": lane_color(n), "goal": t["goal"], "model": t["model"] })
            }).collect();
            r["threads"] = J::Array(threads);
            r["goals"] = json!({ "open": o.open(&format!("{id}:goals")), "key": format!("{id}:goals") });
        }
        "thread_status" => {
            let (w, t) = state_word(data["state"].as_str().unwrap_or(""));
            r["state"] = json!({ "word": w, "tone": t });
            with_text(&mut r, ev, &id, text, o);
            if r["text"].is_null() {
                // A bare change of state is not a message: one quiet line.
                r["type"] = "transition".into();
                r["count"] = 1.into();
                r["trail"] = w.into();
            } else {
                // Done is green, blocked amber, failed red; the rest neutral.
                r["type"] = "status".into();
                r["tint"] = match w {
                    "done" => "green",
                    "blocked" => "amber",
                    "failed" => "red",
                    _ => "",
                }
                .into();
            }
        }
        "board_decision" | "board_warning" | "board_question" | "board_handoff" | "conflict" => {
            r["type"] = "note".into();
            with_text(&mut r, ev, &id, text, o);
            r["files"] = files(&data["files"]);
            r["standing"] = o.standing.is_some_and(|s| s.contains(&id)).into();
        }
        "direct_message" => {
            // Claude Code's own `SendMessage` between the team's sessions, as
            // sbxw's hook saw it: not approved by anyone, said so.
            r["type"] = "message".into();
            r["label"] = "direct message".into();
            with_text(&mut r, ev, &id, text, o);
        }
        "team_note" => {
            // Sent through sbxw (`team_note`): labelled by what it says it is.
            r["type"] = "note".into();
            let (lab, tone, look) = match data["kind"].as_str() {
                Some("warning") => ("Warning", "amber", "board_warning"),
                Some("decision") => ("Decision", "green", "board_decision"),
                _ => ("Info", "blue", "board_question"),
            };
            r["label"] = lab.into();
            r["tone"] = tone.into();
            r["look"] = look.into();
            r["note_via"] = "team note".into();
            r["standing"] = o.standing.is_some_and(|s| s.contains(&id)).into();
            with_text(&mut r, ev, &id, text, o);
        }
        "user_question" | "user_answer" | "relay_question" | "relay_drafted" | "relay_answer" | "screenshot_asked" | "screenshot_given" | "relay_refused" | "relay_routed" | "board_question_reply" => {
            r["type"] = "question".into();
            with_text(&mut r, ev, &id, text, o);
            if let Some(qs) = data["questions"].as_array() {
                r["questions"] = J::Array(qs.iter().map(|q| {
                    let mut q = q.clone();
                    for k in ["text", "answer", "recommendation"] {
                        if let Some(t) = q[k].as_str() {
                            q[k] = breakable(t).into();
                        }
                    }
                    q
                }).collect());
            }
            if kind == "screenshot_given" {
                let n = data["shots"].as_u64().unwrap_or(0);
                r["text"] = format!("{n} screenshot{} given (the picture stays with the agent that asked)", if n == 1 { "" } else { "s" }).into();
            }
            if kind == "relay_routed" {
                r["text"] = "For its agent to answer; you read the answer before it is released.".into();
            }
            if text.is_none() && data["live"] != true && !matches!(kind, "relay_routed" | "screenshot_given") {
                r["hidden_text"] = "The text is not kept once the request is settled and gone from memory.".into();
            }
            let waiting = data["relay"].as_str().is_some_and(|rid| o.waiting.is_some_and(|w| w.contains(rid)));
            if waiting && matches!(kind, "user_question" | "relay_question" | "screenshot_asked" | "relay_drafted") {
                r["link"] = json!({ "label": "Answer in Requests", "what": "requests", "relay": data["relay"] });
            }
        }
        "merge_proposed" => {
            r["type"] = "merge".into();
            with_text(&mut r, ev, &id, text, o);
            r["order"] = data["order"].clone();
        }
        "merge_started" => {
            r["type"] = "status".into();
            r["text"] = format!("Merge, in order: {}", join(&data["order"])).into();
        }
        "merge_result" => {
            r["type"] = "system".into();
            let ok = data["ok"].as_bool().unwrap_or(false);
            let t = data["thread"].as_str().unwrap_or("");
            r["tone"] = (if ok { "green" } else { "red" }).into();
            r["text"] = match (ok, text) {
                (true, Some(n)) => format!("{t} merged ({n})"),
                (true, None) => format!("{t} merged"),
                (false, Some(e)) => format!("{t} did not merge: {e}"),
                (false, None) => format!("{t} did not merge"),
            }
            .into();
            r["hash"] = data["commit"].clone();
        }
        "merge_finished" => {
            r["type"] = "system".into();
            let merged = data["state"] == "merged";
            r["tone"] = (if merged { "green" } else { "red" }).into();
            r["text"] = (if merged { format!("Merged: {}", join(&data["order"])) } else { "The merge stopped: retry it from the plan dialog".to_string() }).into();
            r["link"] = json!({ "label": "Plan dialog", "what": "plan" });
        }
        "thread_failed" => {
            r["type"] = "system".into();
            r["text"] = format!("{} could not start: {}", data["thread"].as_str().unwrap_or("a thread"), text.unwrap_or("no reason given")).into();
            r["link"] = json!({ "label": "Its terminal", "what": "terminal", "thread": data["thread"] });
        }
        "claim" | "release" => {
            r["type"] = "system".into();
            let files: Vec<&str> = data["files"].as_array().into_iter().flatten().filter_map(J::as_str).collect();
            r["text"] = format!("{} {} {}", who(ev["from"].as_str().unwrap_or("")).0, if kind == "claim" { "claimed" } else { "released" }, files.join(", ")).into();
        }
        _ => {
            // Approvals, refusals, marked done, closings, the person's other acts.
            r["type"] = if r["side"] == "center" { "system" } else { "status" }.into();
            with_text(&mut r, ev, &id, text, o);
            if kind == "plan_refused" && text.is_none() {
                r["text"] = "No reason given".into();
            }
        }
    }
    // The person's act with no words of theirs: what they did, as its words.
    if r["side"] == "right" && r["text"].is_null() && r["questions"].is_null() {
        r["text"] = r["label"].clone();
    }
    // A text made here (not the event's own, already breakable).
    if let Some(t) = r["text"].as_str().filter(|t| ev["_draw"]["text"].as_str() != Some(*t)) {
        r["text"] = breakable(t).into();
    }
    r
}

fn files(v: &J) -> J {
    J::Array(v.as_array().into_iter().flatten().filter_map(J::as_str).map(|f| J::from(breakable(f))).collect())
}

fn join(v: &J) -> String {
    v.as_array().into_iter().flatten().filter_map(J::as_str).collect::<Vec<_>>().join(", ")
}

/// The conversation: day separators, then each event as a row, a thread's
/// consecutive commits folded into one ("t1 made 9 commits"). Oldest first.
pub fn rows(events: &[J], o: &Opts) -> Vec<J> {
    let mut out: Vec<J> = vec![];
    let mut last_day: Option<u64> = None;
    let mut i = 0;
    while i < events.len() {
        let ev = &events[i];
        if let Some(at) = ev["at"].as_u64() {
            let d = at / 1000 / 86_400;
            if last_day != Some(d) {
                out.push(json!({ "type": "day", "id": format!("day-{d}"), "label": day_label(at, o.now_s) }));
                last_day = Some(d);
            }
        }
        if ev["kind"] == "commit" {
            let from = ev["from"].as_str().unwrap_or("");
            let mut j = i;
            while j < events.len() && events[j]["kind"] == "commit" && events[j]["from"] == from && events[j]["team"] == ev["team"] {
                j += 1;
            }
            out.push(commit_group(&events[i..j], o));
            i = j;
            continue;
        }
        let r = row(ev, o);
        // Bare changes of state one after the other by one agent: one line,
        // the last state, the trail in its tooltip and a count.
        if r["type"] == "transition" {
            if let Some(last) = out.last_mut().filter(|l| l["type"] == "transition" && l["who"] == r["who"] && l["team"] == r["team"]) {
                let n = last["count"].as_u64().unwrap_or(1) + 1;
                let trail = format!("{} → {}", last["trail"].as_str().unwrap_or(""), r["trail"].as_str().unwrap_or(""));
                let first = last["first_when"].as_str().or(last["when"].as_str()).unwrap_or("").to_string();
                *last = r;
                last["count"] = n.into();
                last["trail"] = trail.into();
                last["first_when"] = first.into();
                i += 1;
                continue;
            }
        }
        out.push(group(r, ev, out.last(), events.get(i.wrapping_sub(1))));
        i += 1;
    }
    out
}

/// Kinds that are messages with an author line above their bubble.
fn speaks(t: &J) -> bool {
    matches!(t.as_str(), Some("status" | "note" | "card" | "question" | "message" | "merge"))
}

/// A message following one by the same author, to the same addressee, on
/// the same side, within [`GROUP_SECS`]: it shares the author line above
/// (`cont`), its bubble stacked under the previous one.
fn group(mut r: J, ev: &J, prev_row: Option<&J>, prev_ev: Option<&J>) -> J {
    let (Some(p), Some(pe)) = (prev_row, prev_ev) else { return r };
    let near = match (ev["at"].as_u64(), pe["at"].as_u64()) {
        (Some(a), Some(b)) => a.saturating_sub(b) <= GROUP_SECS * 1000,
        _ => false,
    };
    let same = p["who"] == r["who"] && p["to"] == r["to"] && p["side"] == r["side"] && p["team"] == r["team"] && p["id"] == pe["id"];
    if near && same && speaks(&r["type"]) && speaks(&p["type"]) {
        r["cont"] = true.into();
    }
    r
}

fn commit_line(ev: &J) -> J {
    let d = &ev["data"];
    json!({ "id": ev["id"], "hash": d["hash"], "short": d["short"], "subject": ev["text"], "files": d["files"], "added": d["added"], "removed": d["removed"], "merged": d["merged"], "repo": d["repo"] })
}

fn commit_group(group: &[J], o: &Opts) -> J {
    let first = &group[0];
    let mut r = base(group.last().unwrap_or(first), o);
    r["type"] = "commits".into();
    let n = group.len();
    let (added, removed): (u64, u64) = group.iter().fold((0, 0), |(a, b), e| (a + e["data"]["added"].as_u64().unwrap_or(0), b + e["data"]["removed"].as_u64().unwrap_or(0)));
    let key = format!("commits-{}", first["id"].as_str().unwrap_or(""));
    r["id"] = key.clone().into();
    r["count"] = n.into();
    r["added"] = added.into();
    r["removed"] = removed.into();
    r["lines"] = J::Array(group.iter().map(commit_line).collect());
    r["open"] = (n == 1 || o.open(&key)).into();
    r["key"] = key.into();
    r["text"] = (if n == 1 { first["text"].as_str().unwrap_or("").to_string() } else { format!("{} made {n} commits", r["who"].as_str().unwrap_or("")) }).into();
    r
}

/// A lane graph of the team's branches against main: one lane per thread
/// (its colour), a dot per commit in time order across the team (the last
/// [`LANE_DOTS`] a lane), a merged lane joining main again. `w` wide.
pub fn lanes(threads: &[String], commits: &[J], w: f32) -> J {
    const ROW: f32 = 20.0;
    const X0: f32 = 96.0;
    let main_y = 10.0;
    let mut times: Vec<i64> = commits.iter().filter_map(|c| c["at"].as_i64()).collect();
    times.sort();
    times.dedup();
    let span = (w - X0 - 16.0).max(40.0);
    let x_of = |at: i64| -> f32 {
        let rank = times.iter().position(|t| *t == at).unwrap_or(0) as f32;
        let n = (times.len().max(2) - 1) as f32;
        X0 + 16.0 + span * (rank / n)
    };
    let mut strokes = vec![json!({ "points": [X0, main_y, w - 4.0, main_y], "width": 2, "color": "text-muted" })];
    let mut dots = vec![];
    let mut labels = vec![json!({ "name": "main", "y": main_y, "color": "text-muted" })];
    for (i, t) in threads.iter().enumerate() {
        let y = main_y + ROW * (i as f32 + 1.0);
        let color = lane_color(t);
        let mut mine: Vec<&J> = commits.iter().filter(|c| c["thread"].as_str().unwrap_or("team") == t.as_str()).collect();
        mine.sort_by_key(|c| c["at"].as_i64().unwrap_or(0));
        let shown: Vec<&J> = mine.iter().rev().take(LANE_DOTS).rev().copied().collect();
        let xs: Vec<f32> = shown.iter().map(|c| x_of(c["at"].as_i64().unwrap_or(0))).collect();
        let start = xs.first().copied().unwrap_or(X0 + 16.0) - 8.0;
        let end = xs.last().copied().unwrap_or(start + 8.0).max(start + 8.0);
        // Down off main where its first commit is, then along its lane.
        strokes.push(json!({ "points": [start, main_y, start, y, end, y], "width": 2, "color": color }));
        let merged = !mine.is_empty() && mine.iter().all(|c| c["merged"] == true);
        if merged {
            let back = (end + 8.0).min(w - 4.0);
            strokes.push(json!({ "points": [end, y, back, y, back, main_y], "width": 2, "color": color }));
        }
        for x in &xs {
            dots.push(json!({ "x": x, "y": y, "r": 3.5, "color": color }));
        }
        labels.push(json!({ "name": t, "y": y, "color": color, "merged": merged, "count": mine.len() }));
    }
    let h = main_y + ROW * (threads.len() as f32) + 10.0;
    json!({ "w": w, "h": h, "paint": { "strokes": strokes, "dots": dots }, "labels": labels, "x0": X0 })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(id: &str, at: u64, kind: &str, from: &str, text: &str, data: J) -> J {
        json!({ "id": id, "at": at, "kind": kind, "from": from, "text": text, "data": data })
    }

    #[test]
    fn sides_follow_who_acted() {
        let o = Opts { now_s: 2_000_000, ..Default::default() };
        let r = rows(
            &[
                ev("l1", 1_000_000, "plan_proposed", "lead", "Do it", json!({ "threads": [{ "name": "t1", "goal": "g" }] })),
                ev("l2", 1_000_100, "plan_approved", "user", "", json!({})),
                ev("l3", 1_000_200, "thread_failed", "sbxw", "no worktree", json!({ "thread": "t1" })),
            ],
            &o,
        );
        assert_eq!(r[0]["type"], "day");
        assert_eq!((r[1]["side"].as_str(), r[1]["type"].as_str(), r[1]["is_lead"].as_bool()), (Some("left"), Some("card"), Some(true)));
        assert_eq!(r[2]["side"], "right");
        assert_eq!((r[3]["side"].as_str(), r[3]["type"].as_str()), (Some("center"), Some("system")));
    }

    #[test]
    fn a_threads_consecutive_commits_fold_into_one_row() {
        let c = |id: &str, from: &str| ev(id, 5_000, "commit", from, "subject", json!({ "hash": "a".repeat(40), "added": 2, "removed": 1 }));
        let o = Opts::default();
        let r = rows(&[c("c1", "t1"), c("c2", "t1"), c("c3", "t1"), c("c4", "t2")], &o);
        let groups: Vec<&J> = r.iter().filter(|x| x["type"] == "commits").collect();
        assert_eq!(groups.len(), 2);
        assert_eq!((groups[0]["count"].as_u64(), groups[0]["open"].as_bool(), groups[0]["added"].as_u64()), (Some(3), Some(false), Some(6)));
        assert_eq!(groups[0]["text"], "t1 made 3 commits");
        assert_eq!((groups[1]["count"].as_u64(), groups[1]["open"].as_bool()), (Some(1), Some(true)));
        let mut e = BTreeSet::new();
        e.insert(groups[0]["key"].as_str().unwrap().to_string());
        let open = rows(&[c("c1", "t1"), c("c2", "t1")], &Opts { expanded: Some(&e), ..Default::default() });
        assert_eq!(open.iter().find(|x| x["type"] == "commits").unwrap()["open"], true);
    }

    #[test]
    fn long_texts_fold_and_unknown_times_say_so() {
        let long = "word ".repeat(200);
        let mut e = json!({ "id": "s1", "at": null, "kind": "board_warning", "from": "t1", "text": long, "data": {} });
        let r = row(&e, &Opts::default());
        assert_eq!(r["fold"]["open"], false);
        assert_eq!(r["text"].as_str().unwrap(), long.as_str(), "the whole text: the screen folds it at its width");
        assert_eq!(r["when"], "time unknown");
        e["text"] = "short".into();
        assert!(row(&e, &Opts::default())["fold"].is_null());
    }

    #[test]
    fn the_lane_graph_has_a_lane_per_thread_and_joins_main_when_merged() {
        let c = |t: &str, at: i64, merged: bool| json!({ "thread": t, "at": at, "merged": merged });
        let g = lanes(&["a".into(), "b".into()], &[c("a", 1, true), c("a", 2, true), c("b", 3, false)], 300.0);
        assert_eq!(g["labels"].as_array().unwrap().len(), 3);
        assert_eq!(g["paint"]["dots"].as_array().unwrap().len(), 3);
        // main, a's lane, a's join, b's lane.
        assert_eq!(g["paint"]["strokes"].as_array().unwrap().len(), 4);
        for d in g["paint"]["dots"].as_array().unwrap() {
            assert!(d["x"].as_f64().unwrap() <= 300.0);
        }
    }

    #[test]
    fn teams_list_by_what_needs_the_person_then_the_most_recent() {
        let t = |id: &str, state: &str, last: u64| json!({ "id": id, "state": state, "last_ms": last });
        let mut v = vec![t("m-new", "merged", 900), t("r-old", "running", 100), t("x", "refused", 999), t("p", "proposed", 50), t("r-new", "running", 800), t("d", "done", 700), t("m-old", "merged", 10), t("c", "closed", 950)];
        order_teams(&mut v);
        let ids: Vec<&str> = v.iter().map(|t| t["id"].as_str().unwrap()).collect();
        assert_eq!(ids, ["p", "r-new", "r-old", "d", "m-new", "m-old", "c", "x"]);
    }

    #[test]
    fn a_bubble_follows_its_column_between_520_and_the_cap() {
        // (column, overlay): one column under 760 pt, the side column beside a team.
        let w = |conv: f32, ow: f32| bubble_max(conv, ow);
        assert_eq!(w(396.0, 420.0), 372.0, "a narrow column: less its gutters");
        assert_eq!(w(536.0, 560.0), 512.0);
        assert_eq!(w(696.0, 720.0), 520.0, "one column under 760: today's 520");
        assert_eq!(w(360.0, 760.0), 336.0, "760 with its side column: the column less its gutters");
        assert_eq!(w(760.0, 1150.0), 520.0, "62 percent would be under 520");
        assert_eq!(w(1000.0, 1400.0), 620.0, "62 percent of the column");
        assert_eq!(w(1284.0, 1700.0), BUBBLE_CAP);
        assert_eq!(w(2000.0, 2024.0), BUBBLE_CAP, "a wide window never grows past the cap");
        assert_eq!(conv_w(2000.0), 1976.0);
        assert_eq!(conv_w(1284.0), 1260.0);
        assert_eq!(conv_w(720.0), 696.0);
        assert_eq!((lanes_w(420.0), lanes_w(1000.0), lanes_w(2000.0)), (380.0, 960.0, 960.0));
    }

    #[test]
    fn a_text_folds_by_the_lines_it_is_drawn_on() {
        let words = |n: usize| "word ".repeat(n).trim_end().to_string();
        // 500 characters: nine lines at 62 a line, five at 116.
        let t = words(100);
        assert!(folded(&t, chars_per_line(396.0)));
        assert!(!folded(&t, chars_per_line(720.0)), "fits a few lines at the wide width");
        assert!(!folded(&words(60), DEFAULT_CPL));
        // Truly long ones fold at any width, and show the first lines drawn.
        let long = words(600);
        assert!(folded(&long, chars_per_line(720.0)));
        let h = head(&long, chars_per_line(720.0));
        assert!(h.ends_with('…') && drawn_lines(&h, chars_per_line(720.0)) <= SHOW_DRAWN, "{}", h.len());
        assert!(h.chars().count() > 400, "six wide lines, not a stub");
        // Paragraphs count: blank lines are drawn.
        assert!(folded(&"a\n\n".repeat(5), 100));
        assert!(!folded("a\n\nb\n\nc", 100));
    }

    #[test]
    fn a_text_s_fold_width_is_the_first_that_does_not_fold() {
        for t in ["word ".repeat(100), "word ".repeat(600), "a\n\n".repeat(5), format!("{}\n{}", "x".repeat(300), "y ".repeat(200))] {
            let d = drawing(&t);
            let Some(cpl) = d["fold"]["cpl"].as_u64() else {
                assert!(!folded(&t, FOLD_CPL));
                continue;
            };
            for c in 16..200usize {
                assert_eq!(folded(d["text"].as_str().unwrap(), c), (c as u64) < cpl, "{c} against {cpl}");
            }
            assert!(d["fold"]["lines"].as_array().unwrap().len() <= SHOW_DRAWN);
        }
    }
}
