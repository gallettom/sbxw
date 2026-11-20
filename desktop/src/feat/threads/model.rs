//! What the threads feature reads out of the daemon's answers: the sessions
//! of a sandbox and where each stands, a plan's draft, the session ceiling
//! said in plain words, a merge's rows. Pure functions over JSON, so the
//! screens, the extras and the core's dialogs say the same thing.

use serde_json::{json, Value as J};

/// The daemon's rule for a thread name (`THREAD_NAME` in panes.js):
/// lower-case letters, digits and inner hyphens, 1 to 40 characters.
pub fn valid_name(n: &str) -> bool {
    let b = n.as_bytes();
    !b.is_empty()
        && b.len() <= 40
        && b.iter().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-')
        && b[0] != b'-'
        && b[b.len() - 1] != b'-'
}

/// The longest thread name.
pub const NAME_MAX: usize = 40;

/// A lower-case letter folded to ASCII, for the letters with a diacritic or a
/// stroke of Latin-1 and Latin Extended-A. The web client folds the same way
/// (`normalizeName` in util.js: Unicode decomposition plus a short table); both
/// are tested on `tests/js/name-cases.json`. A letter not listed here (Greek,
/// Cyrillic, CJK...) is treated as any other character: a hyphen.
fn fold(c: char) -> Option<&'static str> {
    Some(match c {
        'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'ā' | 'ă' | 'ą' => "a",
        'ç' | 'ć' | 'ĉ' | 'ċ' | 'č' => "c",
        'ď' | 'đ' | 'ð' => "d",
        'è' | 'é' | 'ê' | 'ë' | 'ē' | 'ĕ' | 'ė' | 'ę' | 'ě' => "e",
        'ĝ' | 'ğ' | 'ġ' | 'ģ' => "g",
        'ĥ' | 'ħ' => "h",
        'ì' | 'í' | 'î' | 'ï' | 'ĩ' | 'ī' | 'ĭ' | 'į' | 'ı' => "i",
        'ĵ' => "j",
        'ķ' => "k",
        'ĺ' | 'ļ' | 'ľ' | 'ŀ' | 'ł' => "l",
        'ñ' | 'ń' | 'ņ' | 'ň' => "n",
        'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' | 'ō' | 'ŏ' | 'ő' => "o",
        'ŕ' | 'ŗ' | 'ř' => "r",
        'ś' | 'ŝ' | 'ş' | 'š' => "s",
        'ţ' | 'ť' | 'ŧ' => "t",
        'ù' | 'ú' | 'û' | 'ü' | 'ũ' | 'ū' | 'ŭ' | 'ů' | 'ű' | 'ų' => "u",
        'ŵ' => "w",
        'ý' | 'ÿ' | 'ŷ' => "y",
        'ź' | 'ż' | 'ž' => "z",
        'ß' => "ss",
        'æ' => "ae",
        'œ' => "oe",
        'þ' => "th",
        _ => return None,
    })
}

/// A name as the dialogs make it from what was typed or pasted. `typing`
/// false: the whole rule. Lower case, accents folded, every run of other
/// characters one hyphen, hyphen runs one, no leading hyphen, cut to 40, no
/// trailing hyphen. `typing` true: the same, but a trailing hyphen stands (the
/// next letter is on its way). Empty stays empty. The web client has the same
/// rule (`normalizeName` in util.js); the daemon's own check stays the last word.
pub fn clean_name(raw: &str, typing: bool) -> String {
    let mut out = String::new();
    for c in raw.chars().flat_map(char::to_lowercase) {
        // Combining marks (an accent typed as a separate character) vanish.
        if ('\u{300}'..='\u{36f}').contains(&c) {
            continue;
        }
        let folded;
        let piece: &str = if c.is_ascii_lowercase() || c.is_ascii_digit() {
            folded = c.to_string();
            &folded
        } else {
            fold(c).unwrap_or("-")
        };
        for p in piece.chars() {
            if p == '-' && (out.is_empty() || out.ends_with('-')) {
                continue;
            }
            out.push(p);
        }
    }
    out.truncate(NAME_MAX);
    if !typing {
        out.truncate(out.trim_end_matches('-').len());
    }
    out
}

/// The whole rule, for a name about to be used.
pub fn normalize_name(raw: &str) -> String {
    clean_name(raw, false)
}

/// One session of a sandbox, as the sidebar, the panel and the pane bars
/// show it.
#[derive(Debug, Clone, PartialEq)]
pub struct Session {
    /// `None`: the main session.
    pub thread: Option<String>,
    /// `same` (main checkout), `own` (its worktree), `team` (a shared plan's).
    pub kind: String,
    /// Its session runs (the daemon's `open`, or a hook report).
    pub running: bool,
    /// `starting`, `working`, `wants`, `done`, `failed`, `idle`, `stopped`.
    pub state: &'static str,
    /// Its plan's report, when a plan holds it.
    pub summary: String,
    pub repo: Option<String>,
    pub model: Option<String>,
}

impl Session {
    pub fn label(&self) -> String {
        match &self.thread {
            None => "main".into(),
            Some(t) if self.kind == "same" => t.clone(),
            Some(t) => t.clone(),
        }
    }
    /// The word said beside it (nothing when plainly idle).
    pub fn word(&self) -> &'static str {
        match self.state {
            "starting" => "starting",
            "working" => "working",
            "wants" => "wants you",
            "done" => "done",
            "failed" => "failed",
            "stopped" => "stopped",
            _ => "",
        }
    }
    pub fn tone(&self) -> &'static str {
        match self.state {
            "working" | "starting" => "green",
            "wants" => "amber",
            "failed" => "red",
            "done" => "blue",
            _ => "muted",
        }
    }
    pub fn to_json(&self) -> J {
        json!({
            "thread": self.thread, "kind": self.kind, "running": self.running, "state": self.state,
            "word": self.word(), "tone": self.tone(), "label": self.label(), "summary": self.summary,
            "repo": self.repo, "model": self.model,
        })
    }
}

/// The thread a session report belongs to: the name sbxw gave it, else the
/// worktree its cwd is in (main.js, `threadOfSession`).
pub fn thread_of(info: &J) -> Option<String> {
    if let Some(t) = info["thread"].as_str().filter(|t| !t.is_empty()) {
        return Some(t.to_string());
    }
    let cwd = info["cwd"].as_str()?;
    let rest = cwd.split("/.sbxw-worktrees/").nth(1)?;
    let name = rest.split('/').next()?;
    (!name.is_empty() && !name.starts_with("team-")).then(|| name.to_string())
}

/// The latest hook report of that session (`GET /api/sessions`).
fn live_state(data: &J, sandbox: &str, thread: Option<&str>) -> Option<String> {
    data["sessions"]
        .as_array()?
        .iter()
        .filter(|i| i["sandbox"] == sandbox && thread_of(i).as_deref() == thread && i["state"] != "exited")
        .max_by_key(|i| i["ts"].as_u64().unwrap_or(0))
        .and_then(|i| i["state"].as_str().map(str::to_string))
}

/// The latest approved plan of `sandbox` that holds `thread`: its row.
pub fn plan_thread<'a>(data: &'a J, sandbox: &str, thread: &str) -> Option<&'a J> {
    data["plans"]
        .as_array()?
        .iter()
        .filter(|p| p["sandbox"] == sandbox && p["state"] == "approved")
        .filter_map(|p| p["threads"].as_array()?.iter().find(|t| t["name"] == thread).map(|t| (p["created_ms"].as_u64().unwrap_or(0), t)))
        .max_by_key(|(c, _)| *c)
        .map(|(_, t)| t)
}

fn running_sandbox(data: &J, sandbox: &str) -> bool {
    data["sandboxes"].as_array().is_some_and(|a| a.iter().any(|s| s["name"] == sandbox && s["status"] == "running"))
}

/// Every session of `sandbox`: the main one first, then the sessions in the
/// main checkout, then the threads on worktrees, each kind by name (the web
/// UI's `sandboxSessions`).
pub fn sessions(data: &J, sandbox: &str) -> Vec<Session> {
    let view = &data["threads"][sandbox];
    let mut names: Vec<(String, J)> = view["threads"].as_array().map(|a| a.iter().filter_map(|t| Some((t["name"].as_str()?.to_string(), t.clone()))).collect()).unwrap_or_default();
    // A thread whose hooks reported, not listed yet (the list is read 300 ms later).
    for i in data["sessions"].as_array().into_iter().flatten().filter(|i| i["sandbox"] == sandbox && i["state"] != "exited") {
        if let Some(t) = thread_of(i) {
            if !names.iter().any(|(n, _)| *n == t) {
                names.push((t, json!({ "open": true })));
            }
        }
    }
    let up = running_sandbox(data, sandbox);
    let mut out = vec![];
    let main_state = if !up {
        "stopped"
    } else {
        match live_state(data, sandbox, None).as_deref() {
            Some("working") => "working",
            Some("attention") => "wants",
            _ => "idle",
        }
    };
    out.push(Session { thread: None, kind: "same".into(), running: up, state: main_state, summary: String::new(), repo: None, model: None });
    let mut rest: Vec<Session> = names
        .into_iter()
        .map(|(name, t)| {
            let live = live_state(data, sandbox, Some(&name));
            let running = up && (t["open"] == true || live.is_some());
            let plan = plan_thread(data, sandbox, &name);
            let state = if !running {
                match plan.and_then(|p| p["state"].as_str()) {
                    Some("done") => "done",
                    Some("failed") => "failed",
                    _ => "stopped",
                }
            } else {
                match (plan.and_then(|p| p["state"].as_str()), live.as_deref()) {
                    (_, Some("attention")) => "wants",
                    (Some("blocked"), _) => "wants",
                    (Some("starting"), _) => "starting",
                    (Some("done"), _) => "done",
                    (Some("failed"), _) => "failed",
                    (_, Some("working")) | (Some("working"), _) => "working",
                    _ => "idle",
                }
            };
            Session {
                kind: t["worktree"].as_str().unwrap_or("own").to_string(),
                running,
                state,
                summary: plan.and_then(|p| p["summary"].as_str()).unwrap_or("").to_string(),
                repo: t["repo"].as_str().map(str::to_string),
                model: t["model"].as_str().map(str::to_string),
                thread: Some(name),
            }
        })
        .collect();
    rest.sort_by(|a, b| (a.kind != "same", &a.thread).cmp(&(b.kind != "same", &b.thread)));
    out.extend(rest);
    out
}

/// Sandboxes whose main session leads: made lead (`/api/leads`), or leading
/// a plan that waits on a person or has a thread still at work (plans.js,
/// `isLeading`).
pub fn leading(data: &J, sandbox: &str) -> bool {
    if data["leads"].as_array().is_some_and(|l| l.iter().any(|n| n == sandbox)) {
        return true;
    }
    data["plans"].as_array().into_iter().flatten().any(|p| {
        p["sandbox"] == sandbox
            && p["closed"] != true
            && (p["state"] == "pending" || (p["state"] == "approved" && p["threads"].as_array().is_some_and(|t| t.iter().any(|t| t["state"] != "done" && t["state"] != "failed"))))
    })
}

pub fn plan<'a>(data: &'a J, id: &str) -> Option<&'a J> {
    data["plans"].as_array()?.iter().find(|p| p["id"] == id)
}

/// The plan of `sandbox` waiting for the person.
pub fn pending_plan<'a>(data: &'a J, sandbox: &str) -> Option<&'a J> {
    data["plans"].as_array()?.iter().filter(|p| p["sandbox"] == sandbox && p["state"] == "pending" && p["closed"] != true).max_by_key(|p| p["created_ms"].as_u64().unwrap_or(0))
}

/// The plan of `sandbox` whose merge waits, runs, or failed (and can be retried).
pub fn merge_plan<'a>(data: &'a J, sandbox: &str) -> Option<&'a J> {
    data["plans"]
        .as_array()?
        .iter()
        .filter(|p| p["sandbox"] == sandbox && p["closed"] != true && ["proposed", "merging", "failed"].contains(&p["merge"]["state"].as_str().unwrap_or("")))
        .max_by_key(|p| p["created_ms"].as_u64().unwrap_or(0))
}

// ---------------------------------------------------------------------------
// The draft of a plan under review

/// A pending plan's draft as the review starts: its threads as proposed,
/// each with the index it had (`from`), and its variant.
pub fn draft_of(plan: &J) -> J {
    let threads: Vec<J> = plan["threads"]
        .as_array()
        .into_iter()
        .flatten()
        .enumerate()
        .map(|(i, t)| {
            let mut d = json!({ "name": t["name"], "goal": t["goal"], "scope": t["scope"].as_str().unwrap_or(""), "from": i });
            if let Some(r) = t["repo"].as_str() {
                d["repo"] = json!(r);
            }
            if let Some(m) = t["model"].as_str() {
                d["model"] = json!(m);
            }
            d
        })
        .collect();
    json!({ "threads": threads, "variant": plan["variant"].as_str().unwrap_or("split") })
}

/// What the approval sends: `{threads, variant}`, `repo` and `model` only
/// where the thread has one (as plans.js `rowThread`).
pub fn approve_body(draft: &J) -> J {
    let threads: Vec<J> = draft["threads"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|t| {
            let mut o = json!({ "name": t["name"].as_str().unwrap_or("").trim(), "goal": t["goal"].as_str().unwrap_or("").trim(), "scope": t["scope"].as_str().unwrap_or("").trim() });
            if let Some(r) = t["repo"].as_str() {
                o["repo"] = json!(r);
            }
            if let Some(m) = t["model"].as_str().filter(|m| !m.is_empty()) {
                o["model"] = json!(m);
            }
            o
        })
        .collect();
    json!({ "threads": threads, "variant": draft["variant"].as_str().unwrap_or("split") })
}

/// What the person changed against the proposal, one line each.
pub fn draft_changes(plan: &J, draft: &J) -> Vec<String> {
    let mut out = vec![];
    let proposed = plan["threads"].as_array().cloned().unwrap_or_default();
    let kept: Vec<&J> = draft["threads"].as_array().map(|a| a.iter().collect()).unwrap_or_default();
    for (i, p) in proposed.iter().enumerate() {
        if !kept.iter().any(|t| t["from"] == i) {
            out.push(format!("Removed: {}", p["name"].as_str().unwrap_or("?")));
        }
    }
    for t in &kept {
        let name = t["name"].as_str().unwrap_or("?");
        match t["from"].as_u64().and_then(|i| proposed.get(i as usize)) {
            None => out.push(format!("Added: {name}")),
            Some(p) => {
                let mut what = vec![];
                if p["name"] != t["name"] {
                    what.push(format!("renamed from {}", p["name"].as_str().unwrap_or("?")));
                }
                if p["goal"].as_str().unwrap_or("").trim() != t["goal"].as_str().unwrap_or("").trim() {
                    what.push("goal edited".to_string());
                }
                if p["scope"].as_str().unwrap_or("").trim() != t["scope"].as_str().unwrap_or("").trim() {
                    what.push("scope edited".to_string());
                }
                if p["repo"] != t["repo"] {
                    what.push(format!("repository {}", repo_label(t["repo"].as_str())));
                }
                if p["model"].as_str().unwrap_or("") != t["model"].as_str().unwrap_or("") {
                    what.push(format!("model {}", t["model"].as_str().filter(|m| !m.is_empty()).unwrap_or("default")));
                }
                if !what.is_empty() {
                    out.push(format!("{name}: {}", what.join(", ")));
                }
            }
        }
    }
    if plan["variant"].as_str().unwrap_or("split") != draft["variant"].as_str().unwrap_or("split") {
        out.push(format!("Variant: {} instead of {}", draft["variant"].as_str().unwrap_or("split"), plan["variant"].as_str().unwrap_or("split")));
    }
    out
}

pub fn repo_label(repo: Option<&str>) -> String {
    match repo {
        None => "default".into(),
        Some("") => "the workspace".into(),
        Some(r) => r.to_string(),
    }
}

/// A goal's opening sentence (plans.js `splitGoal`).
pub fn first_sentence(goal: &str) -> String {
    let g = goal.trim();
    let b: Vec<char> = g.chars().collect();
    for i in 0..b.len() {
        if matches!(b[i], '.' | '!' | '?') && b.get(i + 1).is_some_and(|c| c.is_whitespace()) {
            if let Some(n) = b[i + 1..].iter().find(|c| !c.is_whitespace()) {
                if n.is_uppercase() || matches!(n, '(' | '"' | '\'' | '`') {
                    return b[..=i].iter().collect();
                }
            }
        }
    }
    g.to_string()
}

// ---------------------------------------------------------------------------
// The session ceiling, said in plain words

/// How many sessions a sandbox has open, which, and its ceiling: from
/// `GET /api/sandboxes/:name/threads` (`{open, max, threads}`).
pub struct Budget {
    pub open: u64,
    pub max: u64,
    /// The open sessions by name (`main` first).
    pub names: Vec<String>,
}

impl Budget {
    pub fn of(view: &J) -> Option<Budget> {
        let (open, max) = (view["open"].as_u64()?, view["max"].as_u64()?);
        let threads: Vec<String> = view["threads"].as_array().into_iter().flatten().filter(|t| t["open"] == true).filter_map(|t| t["name"].as_str().map(str::to_string)).collect();
        let mut names = vec![];
        // `open` counts the main agent and the threads: what is left is the main one.
        if open > threads.len() as u64 {
            names.push("main".to_string());
        }
        names.extend(threads);
        Some(Budget { open, max, names })
    }
    pub fn free(&self) -> u64 {
        self.max.saturating_sub(self.open)
    }
}

fn plural(n: u64, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// Why `new` more sessions do not fit, and how to make room: how many are
/// open, which, how many places are free, what to do.
pub fn ceiling_text(sandbox: &str, b: &Budget, new: u64) -> String {
    let which = if b.names.is_empty() { String::new() } else { format!(" ({})", b.names.join(", ")) };
    let free = b.free();
    let over = (b.open + new).saturating_sub(b.max);
    format!(
        "Not started: this plan needs {} and {sandbox} allows {} at once (max_agent_sessions = {}). {} open now{which}, so {} free. \
         To make room: remove {} from the plan, or close a thread you no longer need (Threads panel, Close), \
         or raise max_agent_sessions in sbxw.toml.",
        plural(new, "new Claude session", "new Claude sessions"),
        b.max,
        b.max,
        plural(b.open, "session is", "sessions are"),
        if free == 1 { "1 place is".to_string() } else { format!("{free} places are") },
        plural(over, "thread", "threads"),
    )
}

/// The daemon's refusal (`svc/plans.rs`, `plan_approve`): `"{new} new
/// thread(s) and {open} session(s) already open is more than
/// max_agent_sessions = {max}. ..."`, as `(new, open, max)`.
pub fn parse_ceiling(msg: &str) -> Option<(u64, u64, u64)> {
    if !msg.contains("max_agent_sessions") {
        return None;
    }
    let num = |s: &str| s.trim().split_whitespace().next()?.parse::<u64>().ok();
    let new = num(msg)?;
    let open = num(msg.split(" and ").nth(1)?)?;
    let max = num(msg.split("max_agent_sessions =").nth(1)?.trim_end_matches(|c: char| !c.is_ascii_digit()))?;
    Some((new, open, max))
}

/// The refusal said in full, with the session names when the sandbox's list is known.
pub fn explain_refusal(data: &J, sandbox: &str, msg: &str) -> String {
    match parse_ceiling(msg) {
        Some((new, open, max)) => {
            let mut b = Budget::of(&data["threads"][sandbox]).unwrap_or(Budget { open, max, names: vec![] });
            // The daemon's numbers are the ones it refused with.
            b.open = open;
            b.max = max;
            if b.names.len() as u64 != open {
                b.names.clear();
            }
            ceiling_text(sandbox, &b, new)
        }
        None => format!("The daemon refused the plan: {msg}"),
    }
}

/// Sessions approving `draft` would start: its threads whose session is not already running.
pub fn new_sessions(data: &J, sandbox: &str, draft: &J) -> u64 {
    let open: Vec<&str> = data["threads"][sandbox]["threads"].as_array().into_iter().flatten().filter(|t| t["open"] == true).filter_map(|t| t["name"].as_str()).collect();
    draft["threads"].as_array().into_iter().flatten().filter(|t| !open.contains(&t["name"].as_str().unwrap_or(""))).count() as u64
}

// ---------------------------------------------------------------------------
// Merge

/// One row per thread in merge order: `{name, state, text}`, state `ok`,
/// `failed`, `pending`, `skipped` or `` (not run yet); and the outcome line.
pub fn merge_rows(plan: &J) -> (Vec<J>, String) {
    let m = &plan["merge"];
    let mut order: Vec<String> = m["order"].as_array().into_iter().flatten().filter_map(|t| t.as_str().map(str::to_string)).collect();
    if order.is_empty() {
        order = plan["threads"].as_array().into_iter().flatten().filter_map(|t| t["name"].as_str().map(str::to_string)).collect();
    }
    let results: Vec<&J> = m["results"].as_array().map(|a| a.iter().collect()).unwrap_or_default();
    let st = m["state"].as_str().unwrap_or("");
    let shared = plan["variant"] == "shared";
    let rows = order
        .iter()
        .map(|n| {
            // A shared plan's one result is named after its worktree.
            let r = results.iter().find(|r| r["thread"] == n.as_str() || (shared && r["thread"].as_str().is_some_and(|t| t.starts_with("team-"))));
            let (state, text) = match r {
                Some(r) if r["ok"] == true => ("ok", r["commit"].as_str().map(|c| c.chars().take(7).collect()).unwrap_or_else(|| "merged".to_string())),
                Some(r) => ("failed", r["error"].as_str().unwrap_or("failed").to_string()),
                None if st == "merging" => ("pending", "pending".to_string()),
                None if st == "failed" || st == "merged" => ("skipped", "not merged".to_string()),
                None => ("", String::new()),
            };
            json!({ "name": n, "state": state, "text": text })
        })
        .collect();
    let outcome = match st {
        "merged" => match m["base"].as_str() {
            Some(b) => format!("Merged into {b}."),
            None => "Merged.".to_string(),
        },
        "failed" => {
            let bad = results.iter().find(|r| r["ok"] != true);
            let done: Vec<&str> = results.iter().filter(|r| r["ok"] == true).filter_map(|r| r["thread"].as_str()).collect();
            let rest: Vec<&String> = order.iter().filter(|n| !done.contains(&n.as_str()) && bad.is_none_or(|b| b["thread"] != n.as_str())).collect();
            let mut s = match bad {
                Some(b) => format!("Stopped at {}: {}.", b["thread"].as_str().unwrap_or("?"), b["error"].as_str().unwrap_or("no reason given")),
                None => "The merge failed.".to_string(),
            };
            if !rest.is_empty() {
                s += &format!(" Not merged: {}.", rest.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", "));
            }
            s
        }
        _ => String::new(),
    };
    (rows, outcome)
}

#[cfg(test)]
mod tests {
    /// The table the web client's tests use too (`tests/js/name-cases.json`).
    const CASES: &str = include_str!("../../../../tests/js/name-cases.json");

    #[test]
    fn the_shared_table_of_names() {
        let t: serde_json::Value = serde_json::from_str(CASES).unwrap();
        for (key, typing) in [("full", false), ("typing", true)] {
            let rows = t[key].as_array().unwrap();
            assert!(rows.len() >= 8, "{key}");
            for r in rows {
                let (input, want) = (r[0].as_str().unwrap(), r[1].as_str().unwrap());
                assert_eq!(clean_name(input, typing), want, "{key}: {input:?}");
            }
        }
    }

    #[test]
    fn a_long_paste_is_cut_to_40_without_a_trailing_hyphen() {
        assert_eq!(normalize_name(&"a".repeat(200)).len(), 40);
        let r = normalize_name(&"word ".repeat(40));
        assert_eq!(r.len(), 39);
        assert!(valid_name(&r));
    }

    #[test]
    fn hostile_text_gives_a_valid_name_or_nothing() {
        for h in ["<script>alert(1)</script>", "\"><img src=x onerror=1>", "'; rm -rf / #", "$(reboot)", "\u{0}\u{7}\u{1b}[31m", "😀😀😀", &"x".repeat(4000), &"<b>".repeat(2000), &"é".repeat(4000), "\u{202e}evil", "e\u{301}te\u{301}"] {
            let r = normalize_name(h);
            assert!(r.is_empty() || valid_name(&r), "{h:.20}");
        }
        assert_eq!(normalize_name("e\u{301}te\u{301} 2026"), "ete-2026");
        for [input, _] in serde_json::from_str::<Vec<[String; 2]>>(&serde_json::from_str::<serde_json::Value>(CASES).unwrap()["full"].to_string()).unwrap() {
            let r = normalize_name(&input);
            assert_eq!(normalize_name(&r), r, "idempotent: {input:?}");
        }
    }

    use super::*;

    #[test]
    fn names_follow_the_daemon_rule() {
        for ok in ["t1", "a", "api-tests", "x0-9"] {
            assert!(valid_name(ok), "{ok}");
        }
        for bad in ["", "-a", "a-", "A", "a_b", "a b", &"x".repeat(41)] {
            assert!(!valid_name(bad), "{bad}");
        }
    }

    #[test]
    fn the_ceiling_is_said_with_numbers_names_and_a_way_out() {
        let msg = "3 new thread(s) and 4 session(s) already open is more than max_agent_sessions = 5. Remove threads, or raise the ceiling in sbxw.toml.";
        assert_eq!(parse_ceiling(msg), Some((3, 4, 5)));
        let data = json!({ "threads": { "web": { "ok": true, "open": 4, "max": 5, "threads": [
            { "name": "t1", "open": true }, { "name": "t2", "open": true }, { "name": "t3", "open": true }, { "name": "old", "open": false } ] } } });
        let t = explain_refusal(&data, "web", msg);
        assert!(t.contains("4 sessions are open now (main, t1, t2, t3)"), "{t}");
        assert!(t.contains("1 place is free"), "{t}");
        assert!(t.contains("max_agent_sessions = 5"), "{t}");
        assert!(t.contains("remove 2 threads from the plan"), "{t}");
        assert!(t.contains("close a thread"), "{t}");
        assert!(explain_refusal(&json!({}), "web", "no plan 'x'").contains("no plan 'x'"));
    }

    #[test]
    fn a_goal_is_cut_at_its_first_sentence_only_before_a_capital() {
        assert_eq!(first_sentence("Fix web.rs and e.g. a file. Then test."), "Fix web.rs and e.g. a file.");
        assert_eq!(first_sentence("One line"), "One line");
    }
}
