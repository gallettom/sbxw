//! The bar's git item (key `status-git`): the repository and branch of what
//! is on screen, read only, after the bring-ups on the left.
//!
//! What is on screen ([`App::git_want`]): over the panes, the focused pane's
//! session; the Sessions view's selected session; any other overlay view
//! scoped to a sandbox, that sandbox's main checkout; every other screen
//! (scope All, Settings, Kits, MCP servers...), nothing: the item is absent
//! rather than wrong.
//!
//! The data is the daemon's `GET /api/sandboxes/<sb>/worktrees`, read by
//! this module (never by a screen) for the sandbox on screen only: when the
//! sandbox on screen changes (after [`DEBOUNCE`]), then at most every
//! [`EVERY`] while the window shows; one read at a time; never for a
//! stopped sandbox; a failed read keeps the last value, with no notice.
//! The repository's name is the last segment of the repository's path (the
//! answer's `repos[].path`), else of the workspace folder. A thread's
//! branch is `threads[].branch`, the main checkout's `repos[].base`; a
//! detached head shows its short commit id (`repos[].head`).
//!
//! Names are data: control characters are dropped, the text is drawn as
//! any label, and a long branch shortens with an ellipsis in its middle.

use super::{item_w, App, Duration, Instant, GAP, ICON, ICON_GAP, ITEM_PX, PAD, TEXT, J};
use crate::theme::{c, layout};
use rune::{row, text, El};
use std::collections::HashMap;

/// Reads for one sandbox are at least this far apart.
pub const EVERY: Duration = Duration::from_secs(30);
/// The sandbox on screen must hold this long before it is read.
pub const DEBOUNCE: Duration = Duration::from_millis(300);
pub const KEY_PREFIX: &str = "statusbar-git:";
/// The shortest the branch gets before the item hides (characters kept).
const MIN_KEEP: usize = 4;
const SHORT_ID: usize = 7;

/// What the item is about: a sandbox, and one of its threads (none: the main checkout).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Want {
    pub sandbox: String,
    pub thread: Option<String>,
}

/// What the item says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    pub repo: String,
    /// The branch, or the short commit id of a detached head, or "detached";
    /// `None` while unknown (the repository shows alone).
    pub branch: Option<String>,
    pub detached: bool,
}

/// The bar's git state between frames.
#[derive(Default)]
pub struct Git {
    /// The sandbox on screen and since when (the debounce).
    seen: Option<(String, Instant)>,
    /// Per sandbox: the last good answer, and when a read was last tried.
    cache: HashMap<String, (Option<J>, Instant)>,
    /// The sandbox being read.
    inflight: Option<String>,
    /// The item as last drawn: a change repaints the bar.
    shown: Option<Item>,
}

fn clean(s: &str) -> String {
    s.chars().filter(|c| !c.is_control()).collect::<String>().trim().to_string()
}

fn last_segment(path: &str) -> String {
    clean(path.trim_end_matches(['/', '\\']).rsplit(['/', '\\']).next().unwrap_or(""))
}

/// The item for a sandbox: its workspace folder, the worktrees answer when
/// there is one, the thread. `None` when it would be wrong or empty.
pub fn item(workspace: Option<&str>, answer: Option<&J>, thread: Option<&str>) -> Option<Item> {
    let ws_name = workspace.map(last_segment).filter(|n| !n.is_empty());
    let Some(a) = answer else {
        // Nothing read yet: the repository alone.
        return ws_name.map(|repo| Item { repo, branch: None, detached: false });
    };
    let repos = a["repos"].as_array()?;
    // Several repositories (or none): which one is on screen is not in the data.
    let [r] = repos.as_slice() else { return None };
    let repo = Some(last_segment(r["path"].as_str().unwrap_or(""))).filter(|n| !n.is_empty()).or_else(|| a["workspace"].as_str().map(last_segment).filter(|n| !n.is_empty())).or(ws_name)?;
    let short = |sha: &J| sha.as_str().map(|s| clean(s).chars().take(SHORT_ID).collect::<String>()).filter(|s| !s.is_empty());
    let (branch, detached) = match thread {
        None => match r["base"].as_str().map(clean).filter(|b| !b.is_empty()) {
            Some(b) => (Some(b), false),
            // A head with no branch name: detached, when there is a head.
            None if r["head"].is_string() => (Some(short(&r["head"]).unwrap_or_else(|| "detached".into())), true),
            None => (None, false),
        },
        Some(t) => match r["threads"].as_array().and_then(|ts| ts.iter().find(|x| x["name"].as_str() == Some(t))) {
            Some(x) => match x["branch"].as_str().map(clean).filter(|b| !b.is_empty()) {
                Some(b) => (Some(b), false),
                None => (Some(short(&x["tip"]).unwrap_or_else(|| "detached".into())), true),
            },
            // That thread is not in the list: the repository alone.
            None => (None, false),
        },
    };
    Some(Item { repo, branch, detached })
}

/// `repo/branch`, as it reads at full length.
pub fn full(i: &Item) -> String {
    match &i.branch {
        Some(b) => format!("{}/{b}", i.repo),
        None => i.repo.clone(),
    }
}

/// The entry's width for its text.
fn entry_w(t: &str) -> f32 {
    item_w(t)
}

/// The text that fits `avail` points (the whole entry): everything, else
/// the branch with an ellipsis in its middle (the repository and the end of
/// the branch stay), else `None` (the item hides).
pub fn fit_text(i: &Item, avail: f32) -> Option<String> {
    let whole = full(i);
    if entry_w(&whole) <= avail {
        return Some(whole);
    }
    let b: Vec<char> = i.branch.as_deref()?.chars().collect();
    let with = |keep: usize| {
        let head = keep.div_ceil(3);
        let tail = keep - head;
        format!("{}/{}…{}", i.repo, b[..head].iter().collect::<String>(), b[b.len() - tail..].iter().collect::<String>())
    };
    if b.len() <= MIN_KEEP || entry_w(&with(MIN_KEEP)) > avail {
        return None;
    }
    // The widest that fits (the width grows with what is kept).
    let (mut lo, mut hi) = (MIN_KEEP, b.len() - 1);
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        if entry_w(&with(mid)) <= avail {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    Some(with(lo))
}

/// The tooltip: the names whole.
pub fn help(i: &Item, sandbox: &str) -> String {
    let mut h = format!("Repository: {}", i.repo);
    match (&i.branch, i.detached) {
        (Some(b), true) => h.push_str(&format!("\nDetached head: {b}")),
        (Some(b), false) => h.push_str(&format!("\nBranch: {b}")),
        (None, _) => {}
    }
    h.push_str(&format!("\nSandbox: {}", clean(sandbox)));
    h
}

impl App {
    /// What the bar's git item is about now: see the module's words.
    pub fn git_want(&self) -> Option<Want> {
        let covering = self.feats.overlay.as_ref().map(|o| o.0.as_str()).or(self.feats.base.as_deref());
        match covering {
            Some(crate::feat::monitor::SCREEN) if self.feats.overlay.as_ref().is_none_or(|o| o.0 == crate::feat::monitor::SCREEN) => {
                let d = &self.feats.data.get(crate::feat::monitor::KEY)?["detail"];
                let sandbox = d["sandbox"].as_str().filter(|s| !s.is_empty())?.to_string();
                Some(Want { sandbox, thread: d["thread"].as_str().filter(|t| !t.is_empty()).map(str::to_string) })
            }
            Some(_) => {
                let sc = self.overlay_scope();
                Some(Want { sandbox: sc["sandbox"].as_str().filter(|s| !s.is_empty())?.to_string(), thread: None })
            }
            None => {
                let p = self.panes.get(self.focused)?;
                Some(Want { sandbox: p.sandbox.clone().filter(|s| !s.is_empty())?, thread: p.thread.clone().filter(|t| !t.is_empty()) })
            }
        }
    }

    /// The item for what is on screen, from the cache.
    pub fn git_item(&self) -> Option<Item> {
        let w = self.git_want()?;
        let ws = self.sandboxes.iter().find(|s| s.name == w.sandbox).and_then(|s| s.workspace.as_deref());
        let answer = self.status.git.cache.get(&w.sandbox).and_then(|(a, _)| a.as_ref());
        item(ws, answer, w.thread.as_deref())
    }

    /// The width the item may take: what the fixed parts leave.
    pub(super) fn git_avail(&self, f: super::Fit, cpu: &super::Cpu, cost: &super::Cost) -> f32 {
        let jobs = &self.status.jobs;
        let left = if jobs.is_empty() { 0.0 } else { super::jobs_w(jobs, f.step) + GAP };
        self.window.0 - PAD * 2.0 - GAP - super::right_w(cpu, cost, f) - left - 0.5
    }

    pub(in crate::shell) fn git_data(&mut self, key: &str, r: Result<J, String>, now: Instant) {
        let Some(sb) = key.strip_prefix(KEY_PREFIX) else { return };
        let g = &mut self.status.git;
        if g.inflight.as_deref() == Some(sb) {
            g.inflight = None;
        }
        let e = g.cache.entry(sb.to_string()).or_insert((None, now));
        e.1 = now;
        // A failed read keeps the last value; nothing is said.
        if let Ok(v) = r {
            if v["repos"].is_array() {
                e.0 = Some(v);
            }
        }
    }

    /// Each poll: follow what is on screen, read when due. Whether the item
    /// changed, and when to look again.
    pub(super) fn git_poll(&mut self, now: Instant) -> (bool, Option<Instant>) {
        let want = self.git_want();
        let mut wake = None;
        let sb = want.as_ref().map(|w| w.sandbox.clone());
        match (&sb, &self.status.git.seen) {
            (Some(s), Some((o, _))) if s == o => {}
            (Some(s), _) => self.status.git.seen = Some((s.clone(), now)),
            (None, _) => self.status.git.seen = None,
        }
        if let (Some(sb), Some((_, since))) = (&sb, &self.status.git.seen) {
            let running = self.sandboxes.iter().any(|s| &s.name == sb && s.status == "running");
            let due = self.status.git.cache.get(sb).map_or(*since + DEBOUNCE, |(_, at)| (*at + EVERY).max(*since + DEBOUNCE));
            if running && !self.status.hidden {
                if due <= now && self.status.git.inflight.is_none() {
                    self.git_read(sb.clone(), now);
                } else if due > now {
                    wake = Some(due);
                }
            }
        }
        let item = self.git_item();
        let changed = item != self.status.git.shown;
        self.status.git.shown = item;
        (changed, wake)
    }

    /// Asks for the worktrees of `sb`. A fixture answers at once.
    fn git_read(&mut self, sb: String, now: Instant) {
        let Some(d) = self.daemon.clone() else { return };
        let path = format!("/api/sandboxes/{}/worktrees", crate::client::enc(&sb));
        let key = format!("{KEY_PREFIX}{sb}");
        self.status.git.inflight = Some(sb.clone());
        // Marked as tried now: a failure waits [`EVERY`] too.
        let e = self.status.git.cache.entry(sb).or_insert((None, now));
        e.1 = now;
        if matches!(self.target, crate::client::Target::Fixture(_)) {
            let r = match d.request("GET", &path, &J::Null, Duration::from_secs(5)) {
                Ok((code, v)) if (200..300).contains(&code) => Ok(v),
                Ok((code, _)) => Err(format!("the daemon answered {code}")),
                Err(e) => Err(format!("{e:#}")),
            };
            self.git_data(&key, r, now);
        } else if let Some(l) = &self.link {
            l.fetch(d, key, vec![(None, path)]);
        } else {
            self.status.git.inflight = None;
        }
    }
}

/// The entry (key `status-git`): the branch icon and `text`, read only.
pub fn view(i: &Item, shown: String, sandbox: &str) -> El {
    row()
        .key("status-git")
        .role("status")
        .label(shown.clone())
        .help(help(i, sandbox))
        .shrink()
        .h(layout::DENSE_H)
        .px(ITEM_PX)
        .gap(ICON_GAP)
        .items_center()
        .child(rune::icon(crate::icons::get("branch").unwrap(), ICON).color(c("text-muted")))
        .child(text(shown).font(crate::screen::body(TEXT, 400.0)).color(c("text")).truncate().min_w(0.0))
}

#[cfg(test)]
#[path = "git_tests.rs"]
mod tests;
