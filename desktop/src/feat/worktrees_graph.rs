//! The branch graph of the Worktrees view: for one repository of a
//! sandbox, the main checkout's branch as one horizontal lane and each
//! UNMERGED thread branch (a tip with commits of its own) as a lane under
//! it, forking from its merge-base and running to its tip. A branch with
//! nothing ahead of the base is not drawn, merged or with no commit yet: it
//! stays in the list, where its row says which (`worktrees::branch_word`).
//!
//! The data mapping is the web UI's (`assets/js/worktrees.js`, `wtLayout`):
//! columns are the base's first-parent commits (`log`, newest first in the
//! answer), oldest first, from just before the earliest fork to HEAD; a
//! thread's own commits (`commits`, newest first) follow its fork (`fork`)
//! on its own row; a column is a commit's place in the history, not its
//! time. Lanes go by fork, the newest first, so a link down from the base
//! never crosses a lane above it, then by name.
//!
//! This module is the layout only, in column and row units, deterministic
//! and width-free; `screens/worktrees.luau` turns it into points (Rune's
//! vector `paint`, `crate::luau::Paint`). Two caps per variant ([`Caps`]):
//! at 720 wide fewer dots, the same lanes. Beyond the cap a lane shows a
//! "+N" gap before its newest commits, and the base starts at an "earlier"
//! edge when older forks are cut.

use serde_json::{json, Value as J};

/// How many dots a graph draws.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Caps {
    /// Commits of the base drawn at most.
    pub base: usize,
    /// Commits of one thread drawn at most (its newest).
    pub thread: usize,
}

/// The overlay 760 pt wide and more.
pub const WIDE: Caps = Caps { base: 24, thread: 10 };
/// Under it (a 720 pt window).
pub const NARROW: Caps = Caps { base: 10, thread: 4 };

/// The lanes' colours, by the thread's name (stable across reads).
pub const PALETTE: &[&str] = &["blue", "purple", "green", "amber"];

/// A thread's lane colour: the same name, the same colour.
pub fn lane_color(name: &str) -> &'static str {
    let h = name.bytes().fold(2166136261u32, |h, b| (h ^ b as u32).wrapping_mul(16777619));
    PALETTE[(h as usize) % PALETTE.len()]
}

/// What the view knows of a thread beyond the answer (its row id, state).
pub struct Extra<'a> {
    pub id: &'a str,
    pub tone: &'a str,
    pub word: &'a str,
}

/// Whether a thread is drawn: a tip and commits of its own. Not whether it
/// is merged: a branch with nothing ahead has no lane to draw, merged or not
/// yet committed to; its row says which (`worktrees::branch_word`). A
/// branch with commits ahead is never "merged" there either.
pub fn unmerged(t: &J) -> bool {
    t["tip"].is_string() && t["ahead"].as_u64().unwrap_or(0) > 0
}

fn short(s: &str) -> &str {
    &s[..s.len().min(7)]
}

/// The graph of one repository (`repo`: one of the answer's `repos`), or
/// `None` when it has no unmerged thread. `extra(name)` gives each
/// thread's row id and state.
pub fn layout<'a>(repo: &J, caps: Caps, extra: &dyn Fn(&str) -> Option<Extra<'a>>) -> Option<J> {
    let threads: Vec<&J> = repo["threads"].as_array().into_iter().flatten().filter(|t| unmerged(t)).collect();
    if threads.is_empty() {
        return None;
    }
    // The base, oldest first.
    let mut fp: Vec<&J> = repo["log"].as_array().into_iter().flatten().collect();
    fp.reverse();
    let at = |sha: Option<&str>| -> Option<usize> { sha.and_then(|s| fp.iter().position(|c| c["sha"] == s)) };
    let last = fp.len().saturating_sub(1);
    let forks: Vec<usize> = threads.iter().filter_map(|t| at(t["fork"].as_str())).collect();
    // From just before the earliest fork, at most `caps.base` commits.
    let mut lo = forks.iter().min().map_or(last.saturating_sub(4), |m| m.saturating_sub(1));
    lo = lo.max((last + 1).saturating_sub(caps.base.max(1)));
    let base_n = if fp.is_empty() { 1 } else { last - lo + 1 };
    // Older commits than drawn, or a fork further back: an "earlier" edge.
    let earlier = lo > 0 || fp.first().is_some_and(|c| c["parents"].as_array().is_some_and(|p| !p.is_empty()));
    let base: Vec<J> = if fp.is_empty() {
        vec![json!({ "col": 0, "sha": repo["head"], "short": repo["head"].as_str().map(short), "subject": "HEAD", "head": true })]
    } else {
        fp[lo..]
            .iter()
            .enumerate()
            .map(|(k, c)| json!({ "col": k, "sha": c["sha"], "short": c["sha"].as_str().map(short), "subject": c["subject"], "at": c["at"], "head": lo + k == last }))
            .collect()
    };
    let mut lanes: Vec<(i64, String, J)> = vec![];
    let mut cols = base_n;
    for t in threads {
        let name = t["name"].as_str().unwrap_or("");
        // Where it forks, in drawn columns: -1 when further back than drawn
        // (or unknown: the answer had no merge-base in the log).
        let fk: i64 = match at(t["fork"].as_str()) {
            Some(f) if f >= lo => (f - lo) as i64,
            _ => -1,
        };
        let ahead = t["ahead"].as_u64().unwrap_or(0) as usize;
        let own: Vec<&J> = t["commits"].as_array().into_iter().flatten().collect();
        // Its newest commits, oldest of them first; the rest counted.
        let shown_n = ahead.min(caps.thread.max(1)).min(own.len().max(1));
        let hidden = ahead.saturating_sub(shown_n);
        let first = fk + 1 + if hidden > 0 { 1 } else { 0 };
        let mut shown: Vec<J> = if own.is_empty() {
            // The daemon listed none of them: the tip alone.
            vec![json!({ "sha": t["tip"], "short": t["tip"].as_str().map(short), "subject": t["tip_subject"], "at": t["tip_at"] })]
        } else {
            own.iter().take(shown_n).map(|c| json!({ "sha": c["sha"], "short": c["sha"].as_str().map(short), "subject": c["subject"], "at": c["at"] })).collect()
        };
        shown.reverse();
        for (j, c) in shown.iter_mut().enumerate() {
            c["col"] = json!(first + j as i64);
        }
        let end = first + shown.len() as i64 - 1;
        cols = cols.max((end + 1).max(0) as usize);
        let x = extra(name);
        lanes.push((
            fk,
            name.to_string(),
            json!({
                "thread": name, "branch": t["branch"], "id": x.as_ref().map(|x| x.id), "tone": x.as_ref().map_or("muted", |x| x.tone), "word": x.as_ref().map_or("", |x| x.word),
                "color": lane_color(name), "fork": fk, "first": first, "end": end, "hidden": hidden, "commits": shown,
                "ahead": ahead, "behind": t["behind"], "dirty": t["dirty"],
            }),
        ));
    }
    lanes.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    let lanes: Vec<J> = lanes
        .into_iter()
        .enumerate()
        .map(|(i, (_, _, mut l))| {
            l["row"] = json!(i + 1);
            l
        })
        .collect();
    Some(json!({
        "repo": repo["path"], "base_name": repo["base"].as_str().unwrap_or("HEAD"), "cols": cols, "rows": lanes.len() + 1,
        "earlier": earlier, "base": base, "lanes": lanes, "caps": { "base": caps.base, "thread": caps.thread },
    }))
}
