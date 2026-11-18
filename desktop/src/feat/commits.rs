//! Commits: the most recent commits across the sandboxes' workspaces, an
//! overlay over the pane area opened from the rail (browse zone), a pane's
//! "⋯" menu (filtered to that sandbox) or the Worktrees view (filtered to a
//! branch). Read only: no git action leaves the app.
//!
//! Data: `GET /api/sandboxes/:name/commits?limit=&before=&ref=` per sandbox
//! (at most [`PARALLEL`] at once), and `.../commits/:hash` for a row pressed
//! open (its files and patch, capped by the daemon). Nothing is read in the
//! background: only while the view is open, and again on Refresh.
//!
//! Why an overlay and not a docked panel: a commit's diff wants the width
//! (long lines, side margins), and a docked panel narrows every terminal and
//! resizes them; the overlay leaves them running underneath, untouched, and
//! Escape gives them back at once.
//!
//! Commit messages are data from git: shown as plain text (no keyword
//! pills, no Markdown, no links made from them).

use super::{Action, Asked, Call, Ctx, Extra, Feature, Place, ScreenDecl, Slot, Standin, Zone};
use crate::client::enc;
use crate::intent::{Arg, Risk, Spec};
use serde_json::{json, Value as J};
use std::collections::{BTreeMap, BTreeSet};

pub const SCREEN: &str = "commits";
/// `data.commits_ui`: what the screen draws.
pub const UI: &str = "commits_ui";
/// Commits asked of each sandbox at a time, and rows a page of the view.
pub const PAGE: usize = 50;
/// Sandboxes read at once.
pub const PARALLEL: usize = 6;
/// Lines of one file's diff drawn (the daemon caps the bytes; this caps the
/// elements: a render holds 5000).
pub const DIFF_LINES: usize = 600;

const KEYS: &[&str] = &["up", "down", "enter"];

pub const FEATURE: Feature = Feature {
    name: "commits",
    screens: &[ScreenDecl { name: SCREEN, src: include_str!("../../screens/commits.luau"), slot: Slot::Overlay { title: "Commits", icon: "history" } }],
    intents: INTENTS,
    act,
    on_answer: Some(on_answer),
    extras: Some(extras),
    standin: Some(standin),
    ..Feature::EMPTY
};

pub const INTENTS: &[Spec] = &[
    Spec { name: "commits_show", risk: Risk::Direct, args: &[("sandbox", Arg::Text, false), ("branch", Arg::Text, false)], doc: "Opens the Commits view over the panes: every sandbox, or that one (`sandbox`), or one branch of it (`branch`, a thread's `sbxw/<thread>`). Read only." },
    Spec { name: "commits_sandbox", risk: Risk::Direct, args: &[("sandbox", Arg::Text, false)], doc: "Shows every sandbox's commits (no `sandbox`, or empty), or one sandbox's." },
    Spec { name: "commits_filter", risk: Risk::Direct, args: &[("text", Arg::Text, false)], doc: "Filters the commits by subject and author; give it on every edit." },
    Spec { name: "commits_unmerged", risk: Risk::Direct, args: &[("on", Arg::Bool, true)], doc: "Only the commits the main checkout's branch does not hold yet." },
    Spec { name: "commits_branch_clear", risk: Risk::Direct, args: &[], doc: "Every branch again (after the view was opened on one)." },
    Spec { name: "commits_expand", risk: Risk::Direct, args: &[("id", Arg::Str, true)], doc: "Opens a commit in place (its body and changed files), or folds it back (`id`: a row's id)." },
    Spec { name: "commits_file", risk: Risk::Direct, args: &[("path", Arg::Str, true)], doc: "Shows a changed file's diff under it in the open commit, or hides it (`path`: as the commit lists it)." },
    Spec { name: "commits_more", risk: Risk::Direct, args: &[], doc: "Shows 50 more commits (reads the next page where needed)." },
    Spec { name: "commits_copy", risk: Risk::Direct, args: &[("id", Arg::Str, true)], doc: "Puts a commit's full hash on the clipboard (a row's id)." },
    Spec { name: "commits_refresh", risk: Risk::Direct, args: &[], doc: "Reads the commits again." },
    Spec { name: "commits_key", risk: Risk::Direct, args: &[("key", Arg::OneOf(KEYS), true)], doc: "The view's keys: Up and Down move the selection, Enter opens or folds the selected commit." },
];

#[derive(Debug, Default, Clone)]
struct Per {
    rows: Vec<J>,
    more: bool,
    loading: bool,
    /// The daemon could not read it (its words).
    unavailable: Option<String>,
    error: Option<String>,
}

#[derive(Debug, Default)]
pub struct State {
    /// One sandbox, or every one (`None`).
    sandbox: Option<String>,
    branch: Option<String>,
    filter: String,
    unmerged: bool,
    /// Pages of [`PAGE`] rows drawn.
    pages: usize,
    /// A read generation: answers of an older one are dropped.
    gen: u64,
    per: BTreeMap<String, Per>,
    queue: Vec<(String, Option<String>)>,
    inflight: usize,
    /// The row open: (sandbox, hash), and its detail.
    open: Option<(String, String)>,
    detail: J,
    detail_error: Option<String>,
    files_open: BTreeSet<String>,
    /// The keyboard's row, by id.
    cursor: Option<String>,
}

crate::feat::relay::per_thread!(STATE, State);

fn act(cx: &Ctx, a: &Asked) -> Result<Vec<Action>, String> {
    STATE().lock().unwrap().act(cx, a)
}
fn on_answer(cx: &Ctx, call: &Call, r: Result<(u16, J), String>) -> Vec<Action> {
    STATE().lock().unwrap().on_answer(cx, call, r)
}

/// A row's id: `<sandbox>:<hash>` (a sandbox name holds no `:`).
pub fn row_id(sandbox: &str, hash: &str) -> String {
    format!("{sandbox}:{hash}")
}

fn split_id(id: &str) -> Option<(&str, &str)> {
    let (s, h) = id.rsplit_once(':')?;
    (!s.is_empty() && h.len() >= 7 && h.bytes().all(|b| b.is_ascii_hexdigit())).then_some((s, h))
}

fn sandboxes(cx: &Ctx) -> Vec<String> {
    cx.get("sandboxes").as_array().into_iter().flatten().filter_map(|s| s["name"].as_str().map(str::to_string)).collect()
}

/// A diff as groups of lines of one kind (`add`, `del`, `hunk`, `meta`,
/// `ctx`), so a file's diff is a handful of text elements, at most
/// [`DIFF_LINES`] lines drawn; and the lines left out.
pub fn diff_groups(patch: &str) -> (Vec<J>, usize) {
    let mut out: Vec<(String, String)> = vec![];
    let total = patch.lines().count();
    for l in patch.lines().take(DIFF_LINES) {
        let kind = if l.starts_with("@@") {
            "hunk"
        } else if l.starts_with("+++") || l.starts_with("---") || l.starts_with("diff --git") || l.starts_with("index ") || l.starts_with("new file") || l.starts_with("deleted file") || l.starts_with("old mode") || l.starts_with("new mode") || l.starts_with("similarity") || l.starts_with("rename ") {
            "meta"
        } else if l.starts_with('+') {
            "add"
        } else if l.starts_with('-') {
            "del"
        } else {
            "ctx"
        };
        // The header lines say what the row already says: not drawn.
        if kind == "meta" {
            continue;
        }
        match out.last_mut() {
            Some((k, t)) if k == kind => {
                t.push('\n');
                t.push_str(l);
            }
            _ => out.push((kind.to_string(), l.to_string())),
        }
    }
    (out.into_iter().map(|(k, t)| json!({ "kind": k, "text": t })).collect(), total.saturating_sub(DIFF_LINES))
}

/// How long ago, from unix seconds.
fn ago(at: i64, now: u64) -> String {
    if at <= 0 {
        return String::new();
    }
    crate::feat::files::view::ago(at as u64, now)
}

impl State {
    fn reset_reads(&mut self) {
        self.gen += 1;
        self.per.clear();
        self.queue.clear();
        self.inflight = 0;
        self.pages = 1;
        self.open = None;
        self.detail = J::Null;
        self.detail_error = None;
        self.files_open.clear();
        self.cursor = None;
    }

    /// Reads the first page of every sandbox shown.
    fn read_all(&mut self, cx: &Ctx) -> Vec<Action> {
        self.reset_reads();
        let names = match &self.sandbox {
            Some(s) => vec![s.clone()],
            None => sandboxes(cx),
        };
        for n in names {
            self.per.insert(n.clone(), Per { loading: true, ..Per::default() });
            self.queue.push((n, None));
        }
        self.pump()
    }

    /// Sends the queued reads, [`PARALLEL`] at a time.
    fn pump(&mut self) -> Vec<Action> {
        let mut out = vec![];
        while self.inflight < PARALLEL && !self.queue.is_empty() {
            let (sb, before) = self.queue.remove(0);
            let mut q = format!("?limit={PAGE}");
            if let Some(b) = &before {
                q.push_str(&format!("&before={}", enc(b)));
            }
            if let Some(b) = &self.branch {
                q.push_str(&format!("&ref={}", enc(b)));
            }
            self.inflight += 1;
            out.push(Action::Call(Call::get(format!("/api/sandboxes/{}/commits{q}", enc(&sb)), format!("read {sb}'s commits")).tag(&format!("list:{}:{sb}", self.gen)).quiet()));
        }
        out
    }

    /// Every row read, newest first, through the filters (all of them, before
    /// any page is cut).
    fn filtered(&self) -> Vec<J> {
        let f = self.filter.trim().to_lowercase();
        let mut v: Vec<J> = self
            .per
            .iter()
            .filter(|(n, _)| self.sandbox.as_ref().is_none_or(|s| s == *n))
            .flat_map(|(_, p)| p.rows.iter().cloned())
            .filter(|r| !self.unmerged || r["merged"] != true)
            .filter(|r| f.is_empty() || r["subject"].as_str().unwrap_or("").to_lowercase().contains(&f) || r["author"].as_str().unwrap_or("").to_lowercase().contains(&f))
            .collect();
        v.sort_by_key(|r| std::cmp::Reverse(r["at"].as_i64().unwrap_or(0)));
        v
    }

    /// The rows drawn, with the count line.
    fn page(&self) -> (Vec<J>, J) {
        crate::screen::paged(self.filtered(), PAGE, self.pages.max(1), "commits")
    }

    pub fn act(&mut self, cx: &Ctx, a: &Asked) -> Result<Vec<Action>, String> {
        let mut out = vec![];
        match a.name {
            "commits_show" => {
                let sb = a.s("sandbox").filter(|s| !s.is_empty()).map(str::to_string);
                if let Some(s) = &sb {
                    cx.sandbox(s).ok_or_else(|| format!("{s} is not a sandbox sbxw knows"))?;
                }
                let branch = a.s("branch").filter(|s| !s.is_empty()).map(str::to_string);
                if let Some(b) = &branch {
                    if b.starts_with('-') || b.contains("..") || b.contains(char::is_whitespace) {
                        return Err(format!("{b} is not a branch name"));
                    }
                }
                self.sandbox = sb;
                self.branch = branch;
                self.filter.clear();
                out.push(Action::OpenPanel(SCREEN.into()));
                out.extend(self.read_all(cx));
            }
            "commits_sandbox" => {
                let sb = a.s("sandbox").filter(|s| !s.is_empty()).map(str::to_string);
                if let Some(s) = &sb {
                    cx.sandbox(s).ok_or_else(|| format!("{s} is not a sandbox sbxw knows"))?;
                }
                if sb != self.sandbox {
                    self.sandbox = sb;
                    self.branch = None;
                    out.extend(self.read_all(cx));
                }
            }
            "commits_branch_clear" => {
                self.branch = None;
                out.extend(self.read_all(cx));
            }
            "commits_filter" => {
                self.filter = a.s("text").unwrap_or("").to_string();
                self.pages = 1;
            }
            "commits_unmerged" => {
                self.unmerged = a.b("on").unwrap_or(false);
                self.pages = 1;
            }
            "commits_refresh" => out.extend(self.read_all(cx)),
            "commits_more" => {
                self.pages += 1;
                let next: Vec<(String, Option<String>)> = self
                    .per
                    .iter()
                    .filter(|(_, p)| p.more && !p.loading)
                    .map(|(n, p)| (n.clone(), p.rows.last().and_then(|r| r["hash"].as_str()).map(str::to_string)))
                    .collect();
                for (n, before) in next {
                    if let Some(p) = self.per.get_mut(&n) {
                        p.loading = true;
                    }
                    self.queue.push((n, before));
                }
                out.extend(self.pump());
            }
            "commits_expand" => out.extend(self.expand(cx, a.req("id"))?),
            "commits_file" => {
                let p = a.req("path").to_string();
                let listed = self.detail["commit"]["files"].as_array().into_iter().flatten().any(|f| f["path"] == p.as_str());
                if !listed {
                    return Err(format!("{p} is not a file of the open commit"));
                }
                if !self.files_open.remove(&p) {
                    self.files_open.insert(p);
                }
            }
            "commits_copy" => {
                let id = a.req("id");
                let row = self.find(id).ok_or_else(|| format!("{id} is not a commit shown"))?;
                let hash = row["hash"].as_str().unwrap_or("").to_string();
                out.push(Action::Intent { name: "copy_text".into(), args: json!({ "text": hash }) });
                out.push(Action::notice(format!("Copied {}", &hash[..hash.len().min(12)])));
            }
            "commits_key" => {
                let (rows, _) = self.page();
                let ids: Vec<String> = rows.iter().filter_map(|r| r["id"].as_str().map(str::to_string)).collect();
                let at = self.cursor.as_ref().and_then(|c| ids.iter().position(|i| i == c));
                match a.req("key") {
                    "down" => self.cursor = ids.get(at.map_or(0, |i| (i + 1).min(ids.len().saturating_sub(1)))).cloned(),
                    "up" => self.cursor = ids.get(at.map_or(0, |i| i.saturating_sub(1))).cloned(),
                    _ => {
                        if let Some(c) = self.cursor.clone() {
                            out.extend(self.expand(cx, &c)?);
                        }
                    }
                }
            }
            other => return Err(format!("{other}: not a commits intent")),
        }
        out.push(self.ui(cx));
        Ok(out)
    }

    fn find(&self, id: &str) -> Option<J> {
        let (sb, h) = split_id(id)?;
        self.per.get(sb)?.rows.iter().find(|r| r["hash"].as_str().is_some_and(|x| x == h)).cloned()
    }

    fn expand(&mut self, _cx: &Ctx, id: &str) -> Result<Vec<Action>, String> {
        let row = self.find(id).ok_or_else(|| format!("{id} is not a commit shown"))?;
        let (sb, _) = split_id(id).unwrap();
        let hash = row["hash"].as_str().unwrap_or("").to_string();
        self.cursor = Some(id.to_string());
        if self.open.as_ref().is_some_and(|(s, h)| s == sb && *h == hash) {
            self.open = None;
            self.detail = J::Null;
            self.files_open.clear();
            return Ok(vec![]);
        }
        self.open = Some((sb.to_string(), hash.clone()));
        self.detail = J::Null;
        self.detail_error = None;
        self.files_open.clear();
        Ok(vec![Action::Call(Call::get(format!("/api/sandboxes/{}/commits/{hash}", enc(sb)), format!("read commit {}", &hash[..7.min(hash.len())])).tag(&format!("detail:{sb}:{hash}")).quiet())])
    }

    pub fn on_answer(&mut self, cx: &Ctx, call: &Call, r: Result<(u16, J), String>) -> Vec<Action> {
        let mut out = vec![];
        if let Some(rest) = call.tag.strip_prefix("list:") {
            let Some((gen, sb)) = rest.split_once(':') else { return out };
            if gen != self.gen.to_string() {
                return out;
            }
            self.inflight = self.inflight.saturating_sub(1);
            let colour = cx.get("app")["groups"][sb].as_str().map(str::to_string);
            let p = self.per.entry(sb.to_string()).or_default();
            p.loading = false;
            match r {
                Ok((200, b)) if b["available"] == false => p.unavailable = Some(b["reason"].as_str().unwrap_or("not available").to_string()),
                Ok((200, b)) => {
                    for c in b["commits"].as_array().into_iter().flatten() {
                        let mut c = c.clone();
                        c["sandbox"] = sb.into();
                        c["color"] = colour.clone().map_or(J::Null, J::String);
                        c["id"] = row_id(sb, c["hash"].as_str().unwrap_or("")).into();
                        if !p.rows.iter().any(|x| x["hash"] == c["hash"]) {
                            p.rows.push(c);
                        }
                    }
                    p.more = b["more"] == true;
                }
                Ok((code, b)) => p.error = Some(b["error"].as_str().map(str::to_string).unwrap_or_else(|| format!("the daemon answered {code}"))),
                Err(e) => p.error = Some(e),
            }
            out.extend(self.pump());
        } else if let Some(rest) = call.tag.strip_prefix("detail:") {
            let Some((sb, hash)) = rest.split_once(':') else { return out };
            if self.open.as_ref() != Some(&(sb.to_string(), hash.to_string())) {
                return out;
            }
            match r {
                Ok((200, b)) if b["available"] == false => self.detail_error = Some(b["reason"].as_str().unwrap_or("not available").to_string()),
                Ok((200, b)) => self.detail = b,
                Ok((code, b)) => self.detail_error = Some(b["error"].as_str().map(str::to_string).unwrap_or_else(|| format!("the daemon answered {code}"))),
                Err(e) => self.detail_error = Some(e),
            }
        } else {
            return out;
        }
        out.push(self.ui(cx));
        out
    }

    /// What the screen draws (`data.commits_ui`).
    fn ui(&self, cx: &Ctx) -> Action {
        let now = crate::feat::files::view::now_secs();
        let (rows, count) = self.page();
        let open_id = self.open.as_ref().map(|(s, h)| row_id(s, h));
        let rows: Vec<J> = rows
            .into_iter()
            .map(|mut r| {
                let id = r["id"].as_str().unwrap_or("").to_string();
                r["when"] = ago(r["at"].as_i64().unwrap_or(0), now).into();
                r["expanded"] = (open_id.as_deref() == Some(id.as_str())).into();
                r["selected"] = (self.cursor.as_deref() == Some(id.as_str())).into();
                // A thread's branch reads as the thread's name.
                r["branch_label"] = match r["thread"].as_str() {
                    Some(t) => t.into(),
                    None => r["branch"].clone(),
                };
                r
            })
            .collect();
        let detail = if self.open.is_some() {
            let c = &self.detail["commit"];
            let files: Vec<J> = c["files"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|f| {
                    let path = f["path"].as_str().unwrap_or("");
                    let open = self.files_open.contains(path);
                    let (groups, left) = if open { diff_groups(f["patch"].as_str().unwrap_or("")) } else { (vec![], 0) };
                    let note = if f["binary"] == true {
                        "Binary file: named, not shown".to_string()
                    } else if f["truncated"] == true && f["patch"].as_str().unwrap_or("").is_empty() {
                        "Too large to show here: truncated".to_string()
                    } else if f["truncated"] == true || left > 0 {
                        format!("Truncated{}", if left > 0 { format!(": {left} more lines not drawn") } else { String::new() })
                    } else {
                        String::new()
                    };
                    json!({ "path": path, "added": f["added"], "removed": f["removed"], "binary": f["binary"], "open": open, "groups": groups, "note": note })
                })
                .collect();
            json!({
                "loading": self.detail.is_null() && self.detail_error.is_none(),
                "error": self.detail_error,
                "body": c["body"], "files": files, "files_more": c["files_more"],
                "truncated": self.detail["truncated"] == true,
            })
        } else {
            J::Null
        };
        let mut notes = vec![];
        for (n, p) in &self.per {
            if self.sandbox.as_ref().is_some_and(|s| s != n) {
                continue;
            }
            if let Some(u) = &p.unavailable {
                notes.push(json!({ "sandbox": n, "text": format!("{n}: not available ({u})"), "tone": "muted" }));
            }
            if let Some(e) = &p.error {
                notes.push(json!({ "sandbox": n, "text": format!("{n}: {e}"), "tone": "red" }));
            }
        }
        let choices: Vec<J> = sandboxes(cx).iter().map(|n| json!({ "id": n, "label": n, "color": cx.get("app")["groups"][n.as_str()] })).collect();
        Action::Set {
            key: UI.into(),
            value: json!({
                "sandbox": self.sandbox, "branch": self.branch, "filter": self.filter, "unmerged": self.unmerged,
                "sandboxes": choices, "rows": rows, "count": count, "detail": detail, "notes": notes,
                "loading": self.per.values().any(|p| p.loading),
                "read": self.per.len(),
            }),
        }
    }
}

/// The rail entry (browse zone, first) and a pane's "⋯" menu item
/// (`bar::MENU_ONLY`) that opens the view on that pane's sandbox.
fn extras(cx: &Ctx) -> Vec<Extra> {
    let mut v = vec![Extra {
        place: Place::Rail { zone: Zone::Browse, order: 10 },
        id: SCREEN.into(),
        label: "Commits".into(),
        icon: Some("history".into()),
        tone: "blue".into(),
        help: "The latest commits of every sandbox, read only".into(),
        count: None,
        intent: Some(("commits_show".into(), json!({}))),
    }];
    for n in sandboxes(cx) {
        v.push(Extra { place: Place::Pane(n.clone()), id: "commits".into(), label: "Commits".into(), icon: Some("history".into()), tone: "blue".into(), help: format!("{n}'s latest commits, read only"), count: None, intent: Some(("commits_show".into(), json!({ "sandbox": n }))) });
    }
    v
}

/// The stand-in daemon's history: the fixture's `commits` (by sandbox, the
/// list answer; `commit_details` by hash), else the built-in one
/// (`fixtures/commits/standin.json`). Paged and filtered by branch as the
/// daemon does; a sandbox it has no history for is "not available".
fn standin(st: &mut Standin, method: &str, path: &str, _body: &J) -> Option<(u16, J)> {
    if method != "GET" {
        return None;
    }
    let builtin: J = serde_json::from_str(include_str!("../../fixtures/commits/standin.json")).unwrap_or(J::Null);
    let data = if st.data["commits"].is_object() { st.data.clone() } else { builtin["daemon"].clone() };
    if let Some(c) = crate::feat::match_path("/api/sandboxes/{name}/commits/{hash}", path) {
        let hash = &c[1].1;
        if !(7..=64).contains(&hash.len()) || !hash.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) {
            return Some((400, json!({ "ok": false, "error": "a commit is named by 7 to 64 lowercase hex digits" })));
        }
        let found = data["commit_details"].as_object().and_then(|m| m.iter().find(|(k, _)| k.starts_with(hash.as_str())).map(|(_, v)| v.clone()));
        return Some(match found {
            Some(d) => (200, json!({ "ok": true, "available": true, "commit": d, "truncated": d["truncated"] })),
            None => (404, json!({ "ok": false, "error": format!("no commit {hash}") })),
        });
    }
    let c = crate::feat::match_path("/api/sandboxes/{name}/commits", path)?;
    let sb = &c[0].1;
    let Some(list) = data["commits"][sb.as_str()].as_array() else {
        return Some((200, json!({ "ok": true, "available": false, "reason": "the workspace is not a git repository", "commits": [], "more": false })));
    };
    let limit: usize = crate::feat::query(path, "limit").and_then(|l| l.parse().ok()).unwrap_or(PAGE);
    let branch = crate::feat::query(path, "ref");
    let rows: Vec<&J> = list.iter().filter(|r| branch.as_ref().is_none_or(|b| r["branch"] == b.as_str() || r["merged"] == true)).collect();
    let start = match crate::feat::query(path, "before") {
        Some(b) => rows.iter().position(|r| r["hash"].as_str().is_some_and(|h| h.starts_with(&b))).map_or(rows.len(), |i| i + 1),
        None => 0,
    };
    let end = (start + limit).min(rows.len());
    Some((200, json!({ "ok": true, "available": true, "commits": rows[start..end], "more": end < rows.len() })))
}

#[cfg(test)]
#[path = "commits_tests.rs"]
mod tests;
