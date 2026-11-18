//! Worktrees: the active worktrees of every sandbox, an overlay over the
//! pane area opened from the rail (browse zone). One block per sandbox (its
//! colour), one row per worktree: its path (short), its branch, the thread
//! that owns it and that thread's state, commits ahead of and behind the
//! main checkout's branch, uncommitted changes, the last commit and whether
//! it is merged. Read only: a row opens its terminal, its commits (the
//! Commits view on its branch) or its files (the explorer at its folder).
//!
//! Data: `GET /api/sandboxes/:name/worktrees` (the daemon's worktree graph,
//! with `tip_subject` and `tip_at`) per sandbox, [`PARALLEL`] at once, when
//! the view opens and on Refresh; nothing in the background.
//!
//! A graph (the web UI's `assets/js/worktrees.js` draws one) is not drawn
//! here: the app has no canvas element; it would need lanes per branch from
//! `log`/`commits`/`fork` (all in the answer) and a vector or canvas element
//! in Rune to draw them. The list says the same numbers.

use super::{Action, Asked, Call, Ctx, Extra, Feature, Place, ScreenDecl, Slot, Zone};
use crate::client::enc;
use crate::intent::{Arg, Risk, Spec};
use serde_json::{json, Value as J};
use std::collections::BTreeMap;

pub const SCREEN: &str = "worktrees";
pub const UI: &str = "worktrees_ui";
pub const PARALLEL: usize = 6;
/// Worktree rows a page of the view.
pub const PAGE: usize = 100;

pub const FEATURE: Feature = Feature {
    name: "worktrees",
    screens: &[ScreenDecl { name: SCREEN, src: include_str!("../../screens/worktrees.luau"), slot: Slot::Overlay { title: "Worktrees", icon: "branch" } }],
    intents: INTENTS,
    act,
    on_answer: Some(on_answer),
    extras: Some(extras),
    ..Feature::EMPTY
};

pub const INTENTS: &[Spec] = &[
    Spec { name: "worktrees_show", risk: Risk::Direct, args: &[], doc: "Opens the Worktrees view over the panes: every sandbox's active worktrees, read again now." },
    Spec { name: "worktrees_refresh", risk: Risk::Direct, args: &[], doc: "Reads the worktrees again." },
    Spec { name: "worktrees_filter", risk: Risk::Direct, args: &[("text", Arg::Text, false)], doc: "Filters the worktrees by sandbox, thread, branch or path." },
    Spec { name: "worktrees_more", risk: Risk::Direct, args: &[], doc: "Shows the next worktrees of a long list." },
    Spec { name: "worktrees_terminal", risk: Risk::Direct, args: &[("id", Arg::Str, true)], doc: "Opens the terminal of the thread that owns that worktree (a row's id)." },
    Spec { name: "worktrees_commits", risk: Risk::Direct, args: &[("id", Arg::Str, true)], doc: "The Commits view on that worktree's branch." },
    Spec { name: "worktrees_files", risk: Risk::Direct, args: &[("id", Arg::Str, true)], doc: "The file explorer at that worktree." },
];

#[derive(Debug, Default, Clone)]
struct Per {
    answer: J,
    loading: bool,
    error: Option<String>,
}

#[derive(Debug, Default)]
pub struct State {
    gen: u64,
    per: BTreeMap<String, Per>,
    queue: Vec<String>,
    inflight: usize,
    filter: String,
    pages: usize,
}

crate::feat::relay::per_thread!(STATE, State);

fn act(cx: &Ctx, a: &Asked) -> Result<Vec<Action>, String> {
    STATE().lock().unwrap().act(cx, a)
}
fn on_answer(cx: &Ctx, call: &Call, r: Result<(u16, J), String>) -> Vec<Action> {
    STATE().lock().unwrap().on_answer(cx, call, r)
}

fn sandboxes(cx: &Ctx) -> Vec<J> {
    cx.get("sandboxes").as_array().cloned().unwrap_or_default()
}

/// A worktree's path, short: under the workspace relative to it, else with
/// the home folder as `~` (as the sidebar shows it).
pub fn short_path(path: &str, workspace: Option<&str>) -> String {
    if let Some(ws) = workspace.filter(|w| !w.is_empty()) {
        if let Some(r) = path.strip_prefix(&format!("{}/", ws.trim_end_matches('/'))) {
            return r.to_string();
        }
    }
    for home in ["/Users/", "/home/"] {
        if let Some(r) = path.strip_prefix(home) {
            if let Some((_, rest)) = r.split_once('/') {
                return format!("~/{rest}");
            }
        }
    }
    path.to_string()
}

impl State {
    fn read_all(&mut self, cx: &Ctx) -> Vec<Action> {
        self.gen += 1;
        self.per.clear();
        self.queue.clear();
        self.inflight = 0;
        self.pages = 1;
        for s in sandboxes(cx) {
            if let Some(n) = s["name"].as_str() {
                self.per.insert(n.to_string(), Per { loading: true, ..Per::default() });
                self.queue.push(n.to_string());
            }
        }
        self.pump()
    }

    fn pump(&mut self) -> Vec<Action> {
        let mut out = vec![];
        while self.inflight < PARALLEL && !self.queue.is_empty() {
            let sb = self.queue.remove(0);
            self.inflight += 1;
            out.push(Action::Call(Call::get(format!("/api/sandboxes/{}/worktrees", enc(&sb)), format!("read {sb}'s worktrees")).tag(&format!("wt:{}:{sb}", self.gen)).quiet()));
        }
        out
    }

    /// Every worktree read, as rows, in the sidebar's sandbox order, through the filter.
    fn rows(&self, cx: &Ctx) -> Vec<J> {
        let now = crate::feat::files::view::now_secs();
        let f = self.filter.trim().to_lowercase();
        let mut out = vec![];
        for s in sandboxes(cx) {
            let Some(sb) = s["name"].as_str() else { continue };
            let Some(p) = self.per.get(sb) else { continue };
            let ws = p.answer["workspace"].as_str().or(s["workspace"].as_str());
            let sessions = crate::feat::threads::model::sessions(cx.data, sb);
            let colour = cx.get("app")["groups"][sb].clone();
            for repo in p.answer["repos"].as_array().into_iter().flatten() {
                for t in repo["threads"].as_array().into_iter().flatten() {
                    let name = t["name"].as_str().unwrap_or("");
                    let path = t["worktree"].as_str().unwrap_or("");
                    let short = short_path(path, ws);
                    let branch = t["branch"].as_str().unwrap_or("");
                    let hay = format!("{sb} {name} {branch} {short}").to_lowercase();
                    if !f.is_empty() && !hay.contains(&f) {
                        continue;
                    }
                    let session = sessions.iter().find(|x| x.thread.as_deref() == Some(name));
                    let has_tip = t["tip"].is_string();
                    out.push(json!({
                        "id": format!("{sb}:{name}"),
                        "sandbox": sb, "color": colour, "thread": name, "branch": branch,
                        "path": short, "full": path, "repo": repo["path"], "base": repo["base"],
                        "state": session.map(|x| x.state).unwrap_or(""), "word": session.map(|x| x.word()).unwrap_or(""), "tone": session.map(|x| x.tone()).unwrap_or("muted"),
                        "has_session": session.is_some(),
                        "ahead": t["ahead"], "behind": t["behind"], "dirty": t["dirty"],
                        "subject": t["tip_subject"], "when": t["tip_at"].as_i64().filter(|a| *a > 0).map(|a| crate::feat::files::view::ago(a as u64, now)),
                        "merged": has_tip && t["ahead"].as_u64() == Some(0),
                        "has_branch": has_tip,
                    }));
                }
            }
        }
        out
    }

    pub fn act(&mut self, cx: &Ctx, a: &Asked) -> Result<Vec<Action>, String> {
        let mut out = vec![];
        match a.name {
            "worktrees_show" => {
                out.push(Action::OpenPanel(SCREEN.into()));
                out.extend(self.read_all(cx));
            }
            "worktrees_refresh" => out.extend(self.read_all(cx)),
            "worktrees_filter" => {
                self.filter = a.s("text").unwrap_or("").to_string();
                self.pages = 1;
            }
            "worktrees_more" => self.pages += 1,
            "worktrees_terminal" | "worktrees_commits" | "worktrees_files" => {
                let id = a.req("id");
                let row = self.rows(&Ctx { data: cx.data }).into_iter().find(|r| r["id"] == id).ok_or_else(|| format!("{id} is not a worktree shown"))?;
                let (sb, t) = (row["sandbox"].as_str().unwrap_or("").to_string(), row["thread"].as_str().unwrap_or("").to_string());
                // Each goes through the intent that owns it, checked as any.
                let (name, args) = match a.name {
                    "worktrees_terminal" => ("thread_open", json!({ "sandbox": sb, "name": t })),
                    "worktrees_commits" => ("commits_show", json!({ "sandbox": sb, "branch": row["branch"] })),
                    _ => ("files_open", json!({ "sandbox": sb, "worktree": t })),
                };
                // The view gives way to what the row opens (a terminal shows under it).
                if a.name == "worktrees_terminal" {
                    out.push(Action::ClosePanel(SCREEN.into()));
                }
                out.push(Action::Intent { name: name.into(), args });
                return Ok(out);
            }
            other => return Err(format!("{other}: not a worktrees intent")),
        }
        out.push(self.ui(cx));
        Ok(out)
    }

    pub fn on_answer(&mut self, cx: &Ctx, call: &Call, r: Result<(u16, J), String>) -> Vec<Action> {
        let Some(rest) = call.tag.strip_prefix("wt:") else { return vec![] };
        let Some((gen, sb)) = rest.split_once(':') else { return vec![] };
        if gen != self.gen.to_string() {
            return vec![];
        }
        self.inflight = self.inflight.saturating_sub(1);
        let p = self.per.entry(sb.to_string()).or_default();
        p.loading = false;
        match r {
            Ok((200, b)) => p.answer = b,
            Ok((code, b)) => p.error = Some(b["error"].as_str().map(str::to_string).unwrap_or_else(|| format!("the daemon answered {code}"))),
            Err(e) => p.error = Some(e),
        }
        let mut out = self.pump();
        out.push(self.ui(cx));
        out
    }

    /// `data.worktrees_ui`: blocks by sandbox over the page of rows.
    fn ui(&self, cx: &Ctx) -> Action {
        let (rows, count) = crate::screen::paged(self.rows(cx), PAGE, self.pages.max(1), "worktrees");
        let mut blocks: Vec<J> = vec![];
        for r in rows {
            let sb = r["sandbox"].as_str().unwrap_or("").to_string();
            match blocks.last_mut() {
                Some(b) if b["sandbox"] == sb.as_str() => b["rows"].as_array_mut().unwrap().push(r),
                _ => blocks.push(json!({ "sandbox": sb, "color": r["color"], "rows": [r] })),
            }
        }
        let mut notes = vec![];
        let mut none = vec![];
        for (n, p) in &self.per {
            if let Some(e) = &p.error {
                notes.push(json!({ "text": format!("{n}: {e}"), "tone": "red" }));
            }
            for repo in p.answer["repos"].as_array().into_iter().flatten() {
                if let Some(e) = repo["error"].as_str() {
                    notes.push(json!({ "text": format!("{n}: {e}"), "tone": "muted" }));
                }
            }
            let has = p.answer["repos"].as_array().into_iter().flatten().any(|r| r["threads"].as_array().is_some_and(|t| !t.is_empty()));
            if !p.loading && p.error.is_none() && !has {
                none.push(n.clone());
            }
        }
        Action::Set {
            key: UI.into(),
            value: json!({ "blocks": blocks, "count": count, "filter": self.filter, "notes": notes, "none": none, "loading": self.per.values().any(|p| p.loading) }),
        }
    }
}

fn extras(_cx: &Ctx) -> Vec<Extra> {
    vec![Extra {
        place: Place::Rail { zone: Zone::Browse, order: 20 },
        id: SCREEN.into(),
        label: "Worktrees".into(),
        icon: Some("branch".into()),
        tone: "blue".into(),
        help: "The active worktrees of every sandbox, read only".into(),
        count: None,
        intent: Some(("worktrees_show".into(), json!({}))),
    }]
}

#[cfg(test)]
#[path = "worktrees_tests.rs"]
mod tests;
