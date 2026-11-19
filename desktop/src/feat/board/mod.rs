//! Board: a team's conversation, an overlay over the pane area opened from
//! the rail (activity zone) or a sandbox's row. A team is a plan with its
//! sandbox, lead and threads; its timeline (the daemon's `GET
//! /api/teams/:id/timeline`) is drawn as a conversation in time order: the
//! person's acts on the right, the lead and threads on the left, sbxw's own
//! events in the middle; the board's decisions and warnings stand out, and
//! a thread's consecutive commits fold into one row. Beside it, when wide:
//! the team, the decisions and warnings that still stand, and the commits
//! per thread with a lane graph and a commit's diff in place.
//!
//! Read only. Acting stays where it is done: the plan dialog (approve,
//! merge), the Requests panel (answers), a thread's terminal. The Board
//! links there. It shows only what went through sbxw: the lead's and
//! threads' own `SendMessage`s and terminal conversations never reach it.
//!
//! Data, only while it is open: `GET /api/teams`, a team's timeline (100
//! events a page, newest first, `before` for older ones), a commit's detail
//! (`/api/sandboxes/:name/commits/:hash`, the Commits view's route). The
//! `team` stream event (a timeline grew) reads the newest page again.

pub mod model;

use super::{Action, Asked, Call, Ctx, Extra, Feature, Place, ScreenDecl, Slot, Standin, Zone};
use crate::client::enc;
use crate::intent::{Arg, Risk, Spec};
use serde_json::{json, Value as J};
use std::collections::{BTreeMap, BTreeSet};

pub const SCREEN: &str = "board";
pub const UI: &str = "board_ui";
/// Events a timeline page asks for.
pub const PAGE: usize = 100;
/// Teams read at once for "All teams", and events each.
pub const ALL_TEAMS: usize = 8;
pub const ALL_PAGE: usize = 40;

const GROUPS: &[&str] = &["all", "notes", "questions", "messages", "commits", "status"];
const TABS: &[&str] = &["conversation", "notes", "commits"];
const KEYS: &[&str] = &["up", "down", "enter", "escape"];
/// Teams the picker lists a page (then "Show more").
pub const PICKER_PAGE: usize = 20;
const LINKS: &[&str] = &["terminal", "monitor", "commits", "worktrees", "plan", "requests", "threads"];

pub const FEATURE: Feature = Feature {
    name: "board",
    screens: &[ScreenDecl { name: SCREEN, src: include_str!("../../../screens/board.luau"), slot: Slot::Overlay { title: "Board", icon: "board" } }],
    events: &["team", "plan"],
    intents: INTENTS,
    act,
    on_event: Some(on_event),
    on_answer: Some(on_answer),
    extras: Some(extras),
    standin: Some(standin),
    ..Feature::EMPTY
};

const fn d(name: &'static str, args: &'static [(&'static str, Arg, bool)], doc: &'static str) -> Spec {
    Spec { name, risk: Risk::Direct, args, doc }
}

pub const INTENTS: &[Spec] = &[
    d("board_show", &[("sandbox", Arg::Text, false), ("team", Arg::Str, false)], "Opens the Board over the panes: a team's conversation between you, its lead and its threads (what went through sbxw), its notes and its commits. On a team (`team`, a plan id; `all` for every team), else on that sandbox's current team, else the latest. Read only."),
    d("board_team", &[("id", Arg::Str, true)], "Shows that team's conversation (`all`: every team's latest events)."),
    d("board_filter", &[("group", Arg::OneOf(GROUPS), false), ("thread", Arg::Text, false), ("text", Arg::Text, false)], "Filters the conversation: by kind (all, notes: decisions and warnings, questions, messages: the agents' direct messages, commits, status), by thread (empty: every one), by words; give the words on every edit."),
    d("board_more", &[], "Reads and shows older events of the team's conversation."),
    d("board_expand", &[("id", Arg::Str, true)], "Opens or folds a row in place: a long message, a plan's goals, a thread's commits. View state only."),
    d("board_commit", &[("hash", Arg::Str, true)], "Opens a commit of the team in the Commits column, its files and their diff, or folds it."),
    d("board_file", &[("path", Arg::Str, true)], "Shows or hides a changed file's diff in the open commit (`path`: as the commit lists it)."),
    d("board_tab", &[("tab", Arg::OneOf(TABS), true)], "On a narrow window, which column shows: the conversation, the team and its notes, or the commits."),
    d("board_open", &[("what", Arg::OneOf(LINKS), true), ("thread", Arg::Text, false), ("relay", Arg::Str, false)], "Goes where one acts or reads more: a thread's terminal, its session in Sessions, its commits or worktrees, the plan dialog, the Requests panel (on a request), the Threads panel."),
    d("board_refresh", &[], "Reads the teams and the conversation again."),
    d("board_picker", &[("open", Arg::Bool, false), ("more", Arg::Bool, false)], "Opens or closes the team picker's list (`open`; none: toggles), or lists more teams in it (`more`). View state only."),
    d("board_key", &[("key", Arg::OneOf(KEYS), true)], "The Board's keys while its team picker is open: Up and Down move through the teams, Enter shows the one marked, Escape closes the list."),
];

#[derive(Debug, Default, Clone)]
struct Timeline {
    /// Oldest first, each once.
    events: Vec<J>,
    more: bool,
    team: J,
    standing: Vec<J>,
    commits: J,
    loading: bool,
    error: Option<String>,
}

impl Timeline {
    /// Adds a page (newest first on the wire), keeping each event once.
    fn merge(&mut self, page: &[J]) {
        let have: BTreeSet<String> = self.events.iter().filter_map(|e| e["id"].as_str().map(str::to_string)).collect();
        let mut add: Vec<J> = page.iter().filter(|e| e["id"].as_str().is_some_and(|i| !have.contains(i))).cloned().collect();
        if add.is_empty() {
            return;
        }
        add.reverse();
        self.events.extend(add);
        // Pages arrive newest or oldest; one order: time, then the daemon's.
        let rank: BTreeMap<String, usize> = self.events.iter().enumerate().filter_map(|(i, e)| e["id"].as_str().map(|s| (s.to_string(), i))).collect();
        let key = |e: &J| (e["at"].as_u64().unwrap_or(0), rank.get(e["id"].as_str().unwrap_or("")).copied().unwrap_or(0));
        self.events.sort_by_key(key);
    }
}

#[derive(Debug, Default)]
pub struct State {
    gen: u64,
    teams: Vec<J>,
    teams_loading: bool,
    teams_error: Option<String>,
    /// The team shown: a plan id, or `None` for every team.
    team: Option<String>,
    timelines: BTreeMap<String, Timeline>,
    group: String,
    thread: String,
    text: String,
    expanded: BTreeSet<String>,
    /// Pages of [`model::ROWS_PAGE`] rows drawn.
    pages: usize,
    tab: String,
    /// The commit open in the Commits column, and its detail.
    commit: Option<(String, String)>,
    detail: J,
    detail_error: Option<String>,
    files_open: BTreeSet<String>,
    /// The team picker's list: open, the row marked by the keys (0: All
    /// teams), the pages of teams it lists.
    picker: bool,
    picker_hl: usize,
    picker_pages: usize,
}

crate::feat::relay::per_thread!(STATE, State);

fn act(cx: &Ctx, a: &Asked) -> Result<Vec<Action>, String> {
    STATE().lock().unwrap().act(cx, a)
}
fn on_answer(cx: &Ctx, call: &Call, r: Result<(u16, J), String>) -> Vec<Action> {
    STATE().lock().unwrap().on_answer(cx, call, r)
}
fn on_event(cx: &Ctx, name: &str, data: &J) -> Vec<Action> {
    STATE().lock().unwrap().on_event(cx, name, data)
}

fn shown(cx: &Ctx) -> bool {
    cx.get("app")["overlay"]["screen"] == SCREEN
}

/// The sandbox's current team, from the plans the app already reads: its
/// latest plan pending or at work, else its latest.
pub fn current_team(cx: &Ctx, sandbox: &str) -> Option<String> {
    let plans: Vec<&J> = cx.get("plans").as_array().into_iter().flatten().filter(|p| p["sandbox"] == sandbox).collect();
    let at_work = |p: &J| p["state"] == "pending" || (p["state"] == "approved" && p["closed"] != true && p["merge"]["state"] != "merged");
    let latest = |v: Vec<&J>| v.into_iter().max_by_key(|p| p["created_ms"].as_u64().unwrap_or(0)).and_then(|p| p["id"].as_str().map(str::to_string));
    latest(plans.iter().copied().filter(|p| at_work(p)).collect()).or_else(|| latest(plans))
}

fn timeline_path(id: &str, before: Option<&str>, limit: usize, group: &str, thread: &str, text: &str) -> String {
    let mut p = format!("/api/teams/{}/timeline?limit={limit}", enc(id));
    let g = if group.is_empty() || group == "all" { "conversation" } else { group };
    p.push_str(&format!("&group={}", enc(g)));
    if let Some(b) = before {
        p.push_str(&format!("&before={}", enc(b)));
    }
    if !thread.is_empty() {
        p.push_str(&format!("&thread={}", enc(thread)));
    }
    if !text.trim().is_empty() {
        p.push_str(&format!("&q={}", enc(text.trim())));
    }
    p
}

impl State {
    fn read_teams(&mut self) -> Action {
        self.teams_loading = true;
        Action::Call(Call::get("/api/teams", "read the teams").tag(&format!("teams:{}", self.gen)).quiet())
    }

    /// The first page of the team(s) shown, again (a new team, a filter).
    fn read_shown(&mut self) -> Vec<Action> {
        self.gen += 1;
        self.timelines.clear();
        self.pages = 1;
        let ids: Vec<String> = match &self.team {
            Some(id) => vec![id.clone()],
            None => self.teams.iter().take(ALL_TEAMS).filter_map(|t| t["id"].as_str().map(str::to_string)).collect(),
        };
        let limit = if self.team.is_some() { PAGE } else { ALL_PAGE };
        ids.into_iter().map(|id| self.read(&id, None, limit, "first")).collect()
    }

    fn read(&mut self, id: &str, before: Option<&str>, limit: usize, mode: &str) -> Action {
        self.timelines.entry(id.to_string()).or_default().loading = true;
        let path = timeline_path(id, before, limit, &self.group, &self.thread, &self.text);
        Action::Call(Call::get(path, "read the team's conversation").tag(&format!("tl:{}:{mode}:{id}", self.gen)).quiet())
    }

    fn team_row(&self, id: &str) -> Option<&J> {
        self.teams.iter().find(|t| t["id"] == id)
    }

    fn threads_of(&self) -> Vec<String> {
        let Some(id) = &self.team else { return vec![] };
        let t = self.timelines.get(id).map(|t| &t.team).filter(|t| !t.is_null()).or_else(|| self.team_row(id));
        t.and_then(|t| t["threads"].as_array()).into_iter().flatten().filter_map(|t| t["name"].as_str().map(str::to_string)).collect()
    }

    pub fn act(&mut self, cx: &Ctx, a: &Asked) -> Result<Vec<Action>, String> {
        let mut out = vec![];
        match a.name {
            "board_show" => {
                let team = a.s("team").filter(|t| !t.is_empty()).map(str::to_string);
                let picked = match team.as_deref() {
                    Some("all") => None,
                    Some(t) => Some(t.to_string()),
                    // From a sandbox: its team. From the rail: the team last
                    // shown, else the selected sandbox's, else every team.
                    None => match a.s("sandbox").filter(|s| !s.is_empty()) {
                        Some(s) => current_team(cx, s),
                        None => self.team.clone().or_else(|| cx.get("app")["selected"].as_str().and_then(|s| current_team(cx, s))),
                    },
                };
                self.team = picked;
                self.thread.clear();
                self.text.clear();
                self.group.clear();
                self.commit = None;
                self.picker = false;
                out.push(Action::OpenPanel(SCREEN.into()));
                out.push(self.read_teams());
                out.extend(self.read_shown());
            }
            "board_team" => {
                let id = a.req("id");
                if id != "all" && !self.teams.iter().any(|t| t["id"] == id) && !self.teams.is_empty() {
                    return Err(format!("{id} is not a team sbxw lists"));
                }
                self.picker = false;
                self.team = (id != "all").then(|| id.to_string());
                self.thread.clear();
                self.commit = None;
                out.extend(self.read_shown());
            }
            "board_filter" => {
                if let Some(g) = a.s("group") {
                    self.group = if g == "all" { String::new() } else { g.to_string() };
                }
                if let Some(t) = a.s("thread") {
                    if !t.is_empty() && !self.threads_of().iter().any(|x| x == t) && t != "lead" {
                        return Err(format!("{t} is not a thread of this team"));
                    }
                    self.thread = t.to_string();
                }
                if let Some(t) = a.s("text") {
                    self.text = t.chars().take(200).collect();
                }
                out.extend(self.read_shown());
            }
            "board_more" => {
                // Rows already read but not drawn come first; then an older page.
                let drawn = model::ROWS_PAGE * self.pages.max(1);
                let rows = self.rows(cx).len();
                self.pages += 1;
                if rows <= drawn + model::ROWS_PAGE {
                    if let Some(id) = self.team.clone() {
                        let tl = self.timelines.get(&id);
                        if let Some(oldest) = tl.filter(|t| t.more).and_then(|t| t.events.first()).and_then(|e| e["id"].as_str()).map(str::to_string) {
                            out.push(self.read(&id, Some(&oldest), PAGE, "older"));
                        }
                    }
                }
            }
            "board_expand" => {
                let id = a.req("id").to_string();
                if !self.expanded.remove(&id) {
                    self.expanded.insert(id);
                }
            }
            "board_commit" => {
                let hash = a.req("hash").to_string();
                if self.commit.as_ref().is_some_and(|(_, h)| *h == hash) {
                    self.commit = None;
                } else {
                    let sb = self.commit_sandbox(&hash).ok_or_else(|| format!("{hash} is not a commit of this team"))?;
                    self.detail = json!({ "loading": true });
                    self.detail_error = None;
                    self.files_open.clear();
                    out.push(Action::Call(Call::get(format!("/api/sandboxes/{}/commits/{}", enc(&sb), enc(&hash)), "read the commit").tag(&format!("cd:{hash}")).quiet()));
                    self.commit = Some((sb, hash));
                }
            }
            "board_file" => {
                let p = a.req("path").to_string();
                if !self.detail["files"].as_array().into_iter().flatten().any(|f| f["path"] == p.as_str()) {
                    return Err(format!("{p} is not a file of the open commit"));
                }
                if !self.files_open.remove(&p) {
                    self.files_open.insert(p);
                }
            }
            "board_tab" => self.tab = a.req("tab").to_string(),
            "board_open" => return self.open_link(a),
            "board_refresh" => {
                out.push(self.read_teams());
                out.extend(self.read_shown());
            }
            "board_picker" => {
                if a.b("more") == Some(true) {
                    self.picker_pages = self.picker_pages.max(1) + 1;
                } else {
                    let open = a.b("open").unwrap_or(!self.picker);
                    if open && !self.picker {
                        // Marked: the team shown.
                        self.picker_hl = self.picker_ids().iter().position(|i| Some(i.as_str()) == self.team.as_deref().or(Some("all"))).unwrap_or(0);
                        self.picker_pages = 1;
                    }
                    self.picker = open;
                    if open {
                        // The keyboard on the marked row: Up and Down move
                        // through the list (its rows are one), Enter picks.
                        let id = self.picker_ids().get(self.picker_hl).cloned().unwrap_or_else(|| "all".into());
                        out.push(self.ui(cx));
                        out.push(Action::Focus(format!("board-row-team-{id}")));
                        return Ok(out);
                    }
                    // Closed: the keyboard back on its button, as a menu does.
                    out.push(self.ui(cx));
                    out.push(Action::Focus("board-row-picker".into()));
                    return Ok(out);
                }
            }
            "board_key" => {
                if !self.picker {
                    return Ok(vec![]);
                }
                let ids = self.picker_ids();
                match a.req("key") {
                    "up" => self.picker_hl = self.picker_hl.saturating_sub(1),
                    "down" => self.picker_hl = (self.picker_hl + 1).min(ids.len().saturating_sub(1)),
                    "enter" => {
                        let id = ids.get(self.picker_hl).cloned().unwrap_or_else(|| "all".into());
                        return self.act(cx, &Asked { name: "board_team", args: json!({ "id": id }).as_object().cloned().unwrap_or_default() });
                    }
                    _ => {
                        self.picker = false;
                        out.push(self.ui(cx));
                        out.push(Action::Focus("board-row-picker".into()));
                        return Ok(out);
                    }
                }
            }
            other => return Err(format!("{other}: not a board intent")),
        }
        out.push(self.ui(cx));
        Ok(out)
    }

    /// The picker's rows, in order: "all", then the teams it lists now.
    fn picker_ids(&self) -> Vec<String> {
        let n = PICKER_PAGE * self.picker_pages.max(1);
        std::iter::once("all".to_string()).chain(self.teams.iter().take(n).filter_map(|t| t["id"].as_str().map(str::to_string))).collect()
    }

    /// The sandbox a commit of the team shown is in (from the daemon's answer).
    fn commit_sandbox(&self, hash: &str) -> Option<String> {
        self.timelines.values().find(|t| t.commits["list"].as_array().into_iter().flatten().any(|c| c["hash"] == hash)).and_then(|t| t.team["sandbox"].as_str().map(str::to_string))
    }

    fn open_link(&mut self, a: &Asked) -> Result<Vec<Action>, String> {
        let id = self.team.clone().ok_or("pick a team first")?;
        let team = self.timelines.get(&id).map(|t| t.team.clone()).filter(|t| !t.is_null()).or_else(|| self.team_row(&id).cloned()).ok_or("the team is not read yet")?;
        let sb = team["sandbox"].as_str().unwrap_or("").to_string();
        let thread = a.s("thread").filter(|t| !t.is_empty()).map(str::to_string);
        if let Some(t) = &thread {
            if !self.threads_of().contains(t) {
                return Err(format!("{t} is not a thread of this team"));
            }
        }
        let close = vec![Action::ClosePanel(SCREEN.into())];
        let go = |name: &str, args: J| Action::Intent { name: name.into(), args };
        Ok(match a.req("what") {
            "terminal" => [close, vec![go("open_terminal", json!({ "sandbox": sb, "thread": thread, "pane": "spare" }))]].concat(),
            "monitor" => {
                // Sessions (the monitor feature), when the app has it.
                if crate::feat::intent("monitor_open").is_none() {
                    return Err("Sessions is not in this build".into());
                }
                let key = match &thread {
                    Some(t) => format!("{sb}::{t}"),
                    None => sb.clone(),
                };
                vec![go("monitor_open", json!({})), go("monitor_select", json!({ "key": key }))]
            }
            "commits" => vec![go("commits_show", json!({ "sandbox": sb, "branch": thread.map(|t| format!("sbxw/{t}")) }))],
            "worktrees" => vec![go("worktrees_show", json!({}))],
            "plan" => vec![go("plan_review", json!({ "id": id }))],
            "requests" => vec![go("relay_open", json!({ "id": a.s("relay") }))],
            "threads" => vec![go("threads_show", json!({ "sandbox": sb }))],
            w => return Err(format!("{w}: not a place the Board links to")),
        })
    }

    pub fn on_event(&mut self, cx: &Ctx, name: &str, data: &J) -> Vec<Action> {
        if !shown(cx) {
            return vec![];
        }
        match name {
            "team" => {
                let Some(id) = data["plan"].as_str() else { return vec![] };
                let ours = match &self.team {
                    Some(t) => t == id,
                    None => self.timelines.contains_key(id),
                };
                if !ours {
                    return vec![self.read_teams()];
                }
                let limit = if self.team.is_some() { PAGE } else { ALL_PAGE };
                vec![self.read(id, None, limit, "live")]
            }
            "plan" => vec![self.read_teams()],
            _ => vec![],
        }
    }

    pub fn on_answer(&mut self, cx: &Ctx, call: &Call, r: Result<(u16, J), String>) -> Vec<Action> {
        let body = match r {
            Ok((200, b)) => Ok(b),
            Ok((code, b)) => Err(b["error"].as_str().map(str::to_string).unwrap_or_else(|| format!("the daemon answered {code}"))),
            Err(e) => Err(e),
        };
        if let Some(hash) = call.tag.strip_prefix("cd:") {
            if self.commit.as_ref().is_none_or(|(_, h)| h != hash) {
                return vec![];
            }
            match body {
                Ok(b) => {
                    self.detail = b["commit"].clone();
                    self.detail_error = None;
                }
                Err(e) => {
                    self.detail = J::Null;
                    self.detail_error = Some(e);
                }
            }
            return vec![self.ui(cx)];
        }
        if call.tag.starts_with("teams:") {
            self.teams_loading = false;
            match body {
                Ok(b) => {
                    self.teams = b["teams"].as_array().cloned().unwrap_or_default();
                    self.teams_error = None;
                    // A team the list no longer has (pruned since): the latest one.
                    if let Some(id) = self.team.clone() {
                        if !self.teams.is_empty() && !self.teams.iter().any(|t| t["id"] == id.as_str()) {
                            self.team = self.teams[0]["id"].as_str().map(str::to_string);
                            let mut out = self.read_shown();
                            out.push(self.ui(cx));
                            return out;
                        }
                    }
                    // Every team, opened before the list was read: read them now.
                    if self.team.is_none() && self.timelines.is_empty() && !self.teams.is_empty() {
                        let mut out = self.read_shown();
                        out.push(self.ui(cx));
                        return out;
                    }
                }
                Err(e) => self.teams_error = Some(e),
            }
            return vec![self.ui(cx)];
        }
        let Some(rest) = call.tag.strip_prefix("tl:") else { return vec![] };
        let mut parts = rest.splitn(3, ':');
        let (Some(gen), Some(mode), Some(id)) = (parts.next(), parts.next(), parts.next()) else { return vec![] };
        if gen != self.gen.to_string() {
            return vec![];
        }
        let tl = self.timelines.entry(id.to_string()).or_default();
        tl.loading = false;
        match body {
            Ok(b) => {
                let page = b["events"].as_array().cloned().unwrap_or_default();
                tl.merge(&page);
                // A live read is the newest page: it says nothing of older ones.
                if mode != "live" {
                    tl.more = b["more"].as_bool().unwrap_or(false);
                }
                tl.team = b["team"].clone();
                tl.standing = b["standing"].as_array().cloned().unwrap_or_default();
                if !b["commits"].is_null() {
                    tl.commits = b["commits"].clone();
                }
                tl.error = None;
            }
            Err(e) => tl.error = Some(e),
        }
        vec![self.ui(cx)]
    }

    /// Every event shown, oldest first; for every team, each tagged with its team.
    fn events(&self) -> Vec<J> {
        let lead_of = |id: &str, t: &Timeline| -> J {
            let team = if t.team.is_null() { self.team_row(id).cloned().unwrap_or(J::Null) } else { t.team.clone() };
            team["lead"].clone()
        };
        if let Some(id) = &self.team {
            let Some(t) = self.timelines.get(id) else { return vec![] };
            let lead = lead_of(id, t);
            return t.events.iter().map(|e| {
                let mut e = e.clone();
                e["lead_name"] = lead.clone();
                e
            }).collect();
        }
        let mut all: Vec<J> = vec![];
        for (id, t) in &self.timelines {
            let row = self.team_row(id).cloned().unwrap_or_else(|| t.team.clone());
            let (lead, names) = (lead_of(id, t), model::team_names(&row));
            for e in &t.events {
                let mut e = e.clone();
                e["team"] = id.as_str().into();
                // Named by its threads, the summary in the tooltip: never a cut sentence.
                e["team_label"] = names.clone().into();
                e["team_help"] = row["summary"].clone();
                e["team_sandbox"] = row["sandbox"].clone();
                e["lead_name"] = lead.clone();
                all.push(e);
            }
        }
        all.sort_by_key(|e| e["at"].as_u64().unwrap_or(0));
        all
    }

    fn waiting(cx: &Ctx) -> BTreeSet<String> {
        let reqs = cx.get("relay");
        let list = reqs["requests"].as_array().or_else(|| reqs.as_array());
        list.into_iter().flatten().filter(|r| r["state"] == "pending" || r["state"] == "answered").filter_map(|r| r["id"].as_str().map(str::to_string)).collect()
    }

    /// Characters a line of a bubble holds at the overlay's width now: the
    /// fold counts lines as drawn.
    fn cpl(&self, cx: &Ctx) -> usize {
        let ov = &cx.get("app")["overlay"];
        let Some(ow) = ov["w"].as_f64().map(|w| w as f32) else { return 0 };
        let conv = model::conv_w(ow, self.team.is_some());
        model::chars_per_line(model::bubble_max(conv, ow))
    }

    fn rows(&self, cx: &Ctx) -> Vec<J> {
        let waiting = Self::waiting(cx);
        let o = model::Opts { expanded: Some(&self.expanded), all: self.team.is_none(), now_s: crate::feat::files::view::now_secs(), waiting: Some(&waiting), cpl: self.cpl(cx) };
        let groups = &cx.get("app")["groups"];
        let events = self.events();
        let mut rows = model::rows(&events, &o);
        if self.team.is_none() {
            // Each row's team's sandbox colour (a commit group's id is its own).
            let sb_of: BTreeMap<&str, &str> = self.timelines.keys().filter_map(|id| self.team_row(id).and_then(|t| t["sandbox"].as_str()).map(|sb| (id.as_str(), sb))).collect();
            for r in rows.iter_mut() {
                if let Some(sb) = r["team"].as_str().and_then(|t| sb_of.get(t)) {
                    r["team_color"] = groups[*sb].clone();
                }
            }
        }
        rows
    }

    /// `data.board_ui`: what the screen draws.
    fn ui(&self, cx: &Ctx) -> Action {
        let groups = cx.get("app")["groups"].clone();
        let now = crate::feat::files::view::now_secs();
        let listed = PICKER_PAGE * self.picker_pages.max(1);
        let teams: Vec<J> = self
            .teams
            .iter()
            .take(listed)
            .map(|t| {
                let sb = t["sandbox"].as_str().unwrap_or("");
                let state = t["state"].as_str().unwrap_or("");
                let last = t["last_ms"].as_u64().or(t["updated_ms"].as_u64()).or(t["created_ms"].as_u64());
                json!({ "id": t["id"], "names": model::team_names(t), "summary": t["summary"].as_str().filter(|s| !s.is_empty()).map(model::breakable).unwrap_or_else(|| "(no summary)".into()),
                    "sandbox": sb, "color": groups[sb], "state": state, "tone": model::team_tone(state), "when": last.map(|l| crate::feat::files::view::ago(l / 1000, now)).unwrap_or_default() })
            })
            .collect();
        let current = self.team.clone().unwrap_or_else(|| "all".into());
        let picker = json!({ "open": self.picker, "hl": self.picker_hl, "more": self.teams.len().saturating_sub(listed), "current": current });
        let rows = self.rows(cx);
        let keep = model::ROWS_PAGE * self.pages.max(1);
        let total = rows.len();
        let cut = total.saturating_sub(keep);
        let tl = self.team.as_ref().and_then(|id| self.timelines.get(id));
        let older = cut > 0 || tl.is_some_and(|t| t.more);
        let loading = self.teams_loading || self.timelines.values().any(|t| t.loading);
        let errors: Vec<String> = self.timelines.values().filter_map(|t| t.error.clone()).chain(self.teams_error.clone()).collect();
        let mut v = json!({
            "teams": teams, "team": current, "all": self.team.is_none(), "picker": picker, "popup": self.picker,
            "rows": rows[cut..], "older": older, "hidden": cut, "total": total,
            "group": if self.group.is_empty() { "all" } else { self.group.as_str() }, "thread": self.thread, "text": self.text,
            "tab": if self.tab.is_empty() { "conversation" } else { self.tab.as_str() },
            "loading": loading, "errors": errors, "monitor": crate::feat::intent("monitor_open").is_some(),
        });
        if let Some(t) = tl {
            v["side"] = self.side(t, &groups);
        }
        Action::Set { key: UI.into(), value: v }
    }

    fn side(&self, t: &Timeline, groups: &J) -> J {
        let team = &t.team;
        let sb = team["sandbox"].as_str().unwrap_or("");
        let threads: Vec<J> = team["threads"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|th| {
                let n = th["name"].as_str().unwrap_or("");
                let (w, tone) = model::state_word(th["state"].as_str().unwrap_or(""));
                let meta: Vec<String> = [
                    Some(if team["variant"] == "shared" { "shared worktree".to_string() } else { format!(".sbxw-worktrees/{n}") }),
                    th["model"].as_str().map(str::to_string),
                    th["set_by"].as_str().map(|s| format!("set by {s}")),
                ]
                .into_iter()
                .flatten()
                .collect();
                json!({ "name": n, "color": crate::feat::worktrees::graph::lane_color(n), "state": w, "tone": tone, "meta": meta.join(" · "), "selected": self.thread == n })
            })
            .collect();
        let notes: Vec<J> = t
            .standing
            .iter()
            .map(|n| {
                let warn = n["kind"] == "warning";
                // A team note's pinned decisions and warnings say where they came from.
                let (who, color) = model::who(n["by"].as_str().unwrap_or(""));
                json!({ "id": n["id"], "label": if warn { "Warning" } else { "Decision" }, "tone": if warn { "amber" } else { "green" }, "who": who, "who_color": color, "text": n["text"], "files": n["files"], "guess": n["guess"], "via": if n["team_note"] == true { "team note" } else { "board" } })
            })
            .collect();
        let list: Vec<J> = t.commits["list"].as_array().cloned().unwrap_or_default();
        let names: Vec<String> = threads.iter().filter_map(|x| x["name"].as_str().map(str::to_string)).collect();
        let mut per: Vec<J> = vec![];
        let mut lane_names = names.clone();
        if list.iter().any(|c| c["thread"].is_null()) {
            lane_names.push("team".into());
        }
        for n in &lane_names {
            let mine: Vec<J> = list
                .iter()
                .filter(|c| c["thread"].as_str().unwrap_or("team") == n.as_str())
                .map(|c| {
                    let h = c["hash"].as_str().unwrap_or("");
                    let open = self.commit.as_ref().is_some_and(|(_, x)| x == h);
                    let mut row = json!({ "hash": h, "short": c["short"], "subject": c["subject"], "files": c["files"], "added": c["added"], "removed": c["removed"], "merged": c["merged"], "open": open, "repo": c["repo"] });
                    if open {
                        row["detail"] = self.detail_view();
                    }
                    row
                })
                .collect();
            let tot = t.commits["totals"].as_array().into_iter().flatten().find(|x| x["thread"] == n.as_str()).cloned().unwrap_or(json!({ "commits": 0, "files": 0, "added": 0, "removed": 0 }));
            let merged = !mine.is_empty() && mine.iter().all(|c| c["merged"] == true);
            per.push(json!({ "thread": n, "color": crate::feat::worktrees::graph::lane_color(n), "commits": mine, "totals": tot, "merged": merged }));
        }
        let lanes_w = model::lanes(&lane_names, &list, 320.0);
        let lanes_n = model::lanes(&lane_names, &list, 260.0);
        json!({
            "id": team["id"], "sandbox": sb, "color": groups[sb], "summary": team["summary"], "names": model::team_names(team), "state": team["state"], "tone": model::team_tone(team["state"].as_str().unwrap_or("")),
            "lead": team["lead"], "variant": team["variant"], "threads": threads, "notes": notes,
            "commits": per, "commits_available": t.commits["available"].as_bool().unwrap_or(t.commits.is_null()), "commits_why": t.commits["why"],
            "lanes": { "wide": lanes_w, "narrow": lanes_n },
            "loading": t.loading,
        })
    }

    /// The open commit's files, each with its diff when shown (the Commits
    /// view's groups, `commits::diff_groups`).
    fn detail_view(&self) -> J {
        if let Some(e) = &self.detail_error {
            return json!({ "error": e });
        }
        if self.detail["loading"] == true {
            return json!({ "loading": true });
        }
        let files: Vec<J> = self.detail["files"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|f| {
                let p = f["path"].as_str().unwrap_or("");
                let open = self.files_open.contains(p);
                let mut v = json!({ "path": p, "added": f["added"], "removed": f["removed"], "binary": f["binary"], "open": open, "note": f["note"] });
                if open {
                    let (groups, cut) = crate::feat::commits::diff_groups(f["patch"].as_str().unwrap_or(""));
                    v["groups"] = J::Array(groups);
                    if cut > 0 {
                        v["note"] = format!("{cut} more lines not drawn here: the Commits view or git show has them").into();
                    }
                }
                v
            })
            .collect();
        json!({ "body": self.detail["body"], "files": files, "files_more": self.detail["files_more"], "truncated": self.detail["truncated"] })
    }
}

fn extras(cx: &Ctx) -> Vec<Extra> {
    let plans = cx.get("plans").as_array().cloned().unwrap_or_default();
    let at_work = plans.iter().filter(|p| p["state"] == "approved" && p["closed"] != true && p["merge"]["state"] != "merged").count();
    let mut out = vec![Extra {
        place: Place::Rail { zone: Zone::Activity, order: 35 },
        id: SCREEN.into(),
        label: "Board".into(),
        icon: Some("board".into()),
        tone: "purple".into(),
        help: match at_work {
            0 => "The teams' conversations: plans, reports, notes, questions and commits".into(),
            1 => "The teams' conversations: 1 team at work".into(),
            n => format!("The teams' conversations: {n} teams at work"),
        },
        count: None,
        intent: Some(("board_show".into(), json!({}))),
    }];
    let mut sbs: Vec<&str> = plans.iter().filter_map(|p| p["sandbox"].as_str()).collect();
    sbs.sort();
    sbs.dedup();
    for sb in sbs {
        out.push(Extra {
            place: Place::Sandbox(sb.to_string()),
            id: "board".into(),
            label: "Board".into(),
            icon: Some("board".into()),
            tone: "purple".into(),
            help: format!("The conversation of {sb}'s team"),
            count: None,
            intent: Some(("board_show".into(), json!({ "sandbox": sb }))),
        });
    }
    out
}

/// The stand-in's teams: the fixture's `board_teams` (the list) and
/// `board_timelines` (by plan id, every event newest first, with `team`,
/// `standing` and `commits`), paged and filtered as the daemon does.
fn standin(st: &mut Standin, method: &str, path: &str, body: &J) -> Option<(u16, J)> {
    // The stand-in only (demos, tests): an event joins a team's timeline and
    // the `team` event goes out, as the daemon's does when it grows.
    if method == "POST" {
        let c = crate::feat::match_path("/api/teams/{id}/standin-event", path)?;
        let id = c[0].1.clone();
        let list = st.data["board_timelines"][id.as_str()]["events"].as_array_mut()?;
        list.insert(0, body.clone());
        (st.publish)("team", json!({ "plan": id }));
        return Some((200, json!({ "ok": true })));
    }
    if method != "GET" {
        return None;
    }
    if crate::feat::match_path("/api/teams", path).is_some() {
        return Some((200, json!({ "ok": true, "teams": st.data["board_teams"].as_array().cloned().unwrap_or_default() })));
    }
    let c = crate::feat::match_path("/api/teams/{id}/timeline", path)?;
    let id = &c[0].1;
    let tl = st.data["board_timelines"][id.as_str()].clone();
    if tl.is_null() {
        return Some((404, json!({ "ok": false, "error": format!("no team '{id}'") })));
    }
    let q = |k: &str| crate::feat::query(path, k).filter(|v| !v.is_empty());
    let group = q("group");
    let thread = q("thread");
    let text = q("q").map(|t| t.to_lowercase());
    let all: Vec<&J> = tl["events"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|e| match group.as_deref() {
            None => true,
            Some("conversation") => e["group"] != "claims",
            Some(g) => e["group"] == g,
        })
        .filter(|e| thread.as_ref().is_none_or(|t| e["from"] == t.as_str() || e["to"] == t.as_str() || e["data"]["thread"] == t.as_str()))
        .filter(|e| text.as_ref().is_none_or(|t| e["text"].as_str().unwrap_or("").to_lowercase().contains(t)))
        .collect();
    let limit: usize = q("limit").and_then(|l| l.parse().ok()).unwrap_or(PAGE);
    let start = match q("before") {
        Some(b) => all.iter().position(|e| e["id"] == b.as_str()).map_or(all.len(), |i| i + 1),
        None => 0,
    };
    let end = (start + limit).min(all.len());
    let mut out = json!({ "ok": true, "team": tl["team"], "events": all[start..end], "more": end < all.len(), "standing": tl["standing"] });
    if q("before").is_none() {
        out["commits"] = tl["commits"].clone();
    }
    Some((200, out))
}

#[cfg(test)]
mod tests;
