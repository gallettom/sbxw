//! What the Kanban screen draws, built here once per answer or press: the
//! columns as the daemon decided them, each card's texts and marks ready,
//! its element key, its colours. The screen does no string work per card
//! (BOARD.md, the render budget): it picks one of two prepared titles by one
//! comparison and lays out.

use serde_json::{json, Value as J};

/// Cards a column draws a page (then "Show n more"): 60 cards of about 14
/// elements stay far under a render's 5,000 with five columns.
pub const CARDS_PAGE: usize = 30;
/// Pages a column draws at most.
pub const MAX_PAGES: usize = 4;
/// Pictures fetched for a ticket's panel (their preview).
pub const PREVIEWS: usize = 4;
/// A picture's bytes read at most for a preview (the daemon's own cap is 20 MB).
pub const PREVIEW_MAX: u64 = 21 << 20;

/// Every column a board may have, in order. `backlog` comes only from a
/// daemon with ticket sources (an older one has the five others).
pub const COLUMNS: &[&str] = &["backlog", "planned", "progress", "test", "ready", "merged"];
/// The columns of a board with no ticket source (an older daemon's).
pub const LOCAL_COLUMNS: &[&str] = &["planned", "progress", "test", "ready", "merged"];

/// A tracker's name as the person reads it.
pub fn tracker_label(t: &str) -> String {
    match t {
        "github" => "GitHub".into(),
        "" => "the tracker".into(),
        t => t.to_string(),
    }
}

/// A card's origin url, only when it is one to open in the browser
/// (http or https, no space or control character). Data from the daemon.
pub fn issue_url(card: &J) -> Option<&str> {
    card["origin"]["url"].as_str().filter(|u| crate::markdown::safe_url(u))
}

/// An imported ticket: its words are edited in the tracker.
pub fn imported(card: &J) -> bool {
    card["origin"].is_object()
}

/// A Backlog card the person may move to Planned.
pub fn can_plan(card: &J) -> bool {
    card["column"] == "backlog" && card["can_plan"] == true
}

/// A card's marks as drawn: `closed-upstream` is a quiet "Closed at source".
fn marks_of(c: &J) -> Vec<J> {
    c["marks"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|m| {
            if m["id"] == "closed-upstream" {
                json!({ "label": "Closed at source", "tone": "neutral", "help": "The issue is closed in the tracker", "quiet": true })
            } else {
                json!({ "label": m["label"], "tone": m["tone"], "help": m["help"] })
            }
        })
        .collect()
}

/// An imported card's origin, ready: its key, its tracker, and whether its
/// link opens (the url itself stays in Rust).
fn origin_of(c: &J, id: &str) -> J {
    if !imported(c) {
        return J::Null;
    }
    let tracker = tracker_label(c["origin"]["tracker"].as_str().unwrap_or(""));
    let url = issue_url(c).is_some();
    json!({
        "key": c["origin"]["key"],
        "tracker": tracker,
        "open": url,
        "open_help": if url { format!("Open the issue in {tracker}, in your browser") } else { "Its link is not a web address: not opened".to_string() },
        "el": format!("{id}-issue"),
    })
}

/// `s` cut to `max` characters with an ellipsis (a title in a sentence).
pub fn clip(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut t: String = s.chars().take(max.saturating_sub(1)).collect();
    t.truncate(t.trim_end().len());
    t.push('…');
    t
}

/// A card's element key: `kanban-row-` and its daemon key made safe
/// (`t:web-app:12` becomes `kanban-row-t-web-app-12`).
pub fn el(key: &str) -> String {
    let s: String = key.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '-' }).collect();
    format!("kanban-row-{s}")
}

/// A ticket card's `(sandbox, n)`, from its daemon card.
pub fn ticket_of(card: &J) -> Option<(String, u32)> {
    let n = card["n"].as_u64()? as u32;
    Some((card["sandbox"].as_str()?.to_string(), n))
}

/// The view of one card.
pub fn card(c: &J, groups: &J, all: bool, focus: Option<&str>) -> J {
    let key = c["key"].as_str().unwrap_or("");
    let id = el(key);
    let sb = c["sandbox"].as_str().unwrap_or("");
    let thread = c["thread"].as_str().unwrap_or("");
    let marks = marks_of(c);
    // Failed or refused work: a red edge (a priority is not a failure).
    let red = c["marks"].as_array().into_iter().flatten().any(|m| matches!(m["id"].as_str(), Some("failed" | "merge_failed" | "refused")));
    let can = &c["can"];
    // The whole title in the tooltip only when the card cuts it (and its sandbox in All).
    let cut = c["title_long"] != c["title"] || c["title_short"] != c["title"];
    let help = match c["title"].as_str() {
        Some(t) if all => format!("{t}\n{sb}"),
        Some(t) if cut => t.to_string(),
        _ => String::new(),
    };
    json!({
        "id": id,
        "key": key,
        "num": c["num"],
        "thread_card": c["n"].is_null(),
        "title": c["title_short"],
        "title_long": c["title_long"],
        "help": if help.is_empty() { J::Null } else { help.into() },
        "thread": if thread.is_empty() { J::Null } else { thread.into() },
        "thread_tone": c["thread_tone"],
        "thread_color": if thread.is_empty() { J::Null } else { crate::feat::worktrees::graph::lane_color(thread).into() },
        // The sandbox's colour mark, in All sandboxes (one sandbox: its scope says it).
        "team_color": if all { groups[sb].clone() } else { J::Null },
        "team_help": if all { sb.into() } else { J::Null },
        // In All sandboxes the sandbox's name too: a colour alone is never the only signal, and a sandbox in no group has none.
        "sandbox_label": if all { sb.into() } else { J::Null },
        "marks": marks,
        "age": c["age"],
        "border": if red { "red-border" } else { "elevated" },
        "selected": focus == Some(id.as_str()),
        "movable": can["reorder"] == true,
        "move_help": if can["reorder"] == true { "Move up or down in Planned (its panel)".to_string() } else { can["reorder_why"].as_str().unwrap_or("").to_string() },
        "origin": origin_of(c, &id),
        "plan": if can_plan(c) { json!({ "el": format!("{id}-plan") }) } else { J::Null },
    })
}

/// The columns, each with its first pages of cards.
pub fn columns(board: &J, groups: &J, pages: &std::collections::BTreeMap<String, usize>, focus: Option<&str>) -> Vec<J> {
    let all = board["scope"] == "all";
    board["columns"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|c| {
            let id = c["id"].as_str().unwrap_or("");
            let cards: Vec<&J> = c["cards"].as_array().into_iter().flatten().collect();
            let p = pages.get(id).copied().unwrap_or(1).clamp(1, MAX_PAGES);
            let drawn = (CARDS_PAGE * p).min(cards.len());
            let left = cards.len() - drawn;
            let more = if left > 0 && p < MAX_PAGES { json!({ "label": format!("Show {} more", left.min(CARDS_PAGE)), "el": format!("kanban-more-{id}") }) } else { J::Null };
            let older = c["older"].as_u64().unwrap_or(0);
            let count = c["count"].as_u64().unwrap_or(0);
            json!({
                "id": id,
                "title": c["title"],
                "tone": c["tone"],
                "help": c["help"],
                "empty": c["empty"],
                "count": count,
                "count_help": format!("{count} · {}", c["help"].as_str().unwrap_or("")),
                "picker_label": format!("{} ({count})", c["title"].as_str().unwrap_or("")),
                "cards": cards[..drawn].iter().map(|x| card(x, groups, all, focus)).collect::<Vec<_>>(),
                "more": more,
                "older": if older > 0 { json!({ "label": format!("Show {older} older"), "el": "kanban-older" }) } else { J::Null },
                "capped": left > 0 && p >= MAX_PAGES,
                "el_head": format!("kanban-head-{id}"),
                "el_lane": format!("kanban-lane-{id}"),
                "el_scroll": format!("kanban-scroll-{id}"),
            })
        })
        .collect()
}

/// Every card's element key, column by column, in drawing order (the keys
/// move along it).
pub fn order(cols: &[J]) -> Vec<Vec<String>> {
    cols.iter().map(|c| c["cards"].as_array().into_iter().flatten().filter_map(|x| x["id"].as_str().map(str::to_string)).collect()).collect()
}

/// A column's id by its label in the History.
pub fn column_label(id: &str) -> (&'static str, &'static str) {
    match id {
        "backlog" => ("Backlog", "neutral"),
        "planned" => ("Planned", "neutral"),
        "progress" => ("In progress", "blue"),
        "test" => ("In test", "purple"),
        "ready" => ("Ready to merge", "amber"),
        "merged" => ("Merged", "green"),
        _ => ("", "neutral"),
    }
}

/// The ticket's panel: the daemon's view (or, for a thread with no ticket,
/// its card), what the person may do and why not.
pub fn panel(card: &J, view: Option<&J>, previews: &std::collections::BTreeSet<String>, held: Option<&str>) -> J {
    let col = card["column"].as_str().unwrap_or("");
    let (label, tone) = column_label(col);
    let can = &card["can"];
    let thread = card["thread"].as_str().filter(|t| !t.is_empty());
    let sb = card["sandbox"].as_str().unwrap_or("");
    let state = card["thread_state"].as_str().unwrap_or("proposed");
    let mut v = json!({
        "id": el(card["key"].as_str().unwrap_or("")),
        "num": card["num"],
        "thread_card": card["n"].is_null(),
        "sandbox": sb,
        "column_label": label,
        "column_tone": tone,
        "title": card["title"],
        "marks": marks_of(card),
        "thread": thread.map(|t| json!({ "name": t, "state": state, "tone": card["thread_tone"], "color": crate::feat::worktrees::graph::lane_color(t) })),
        "editable": editable(card),
        "edit_why": if editable(card) { J::Null } else if imported(card) { format!("Edited in {}", tracker_label(card["origin"]["tracker"].as_str().unwrap_or(""))).into() } else { "Recorded from a thread: nothing to edit".into() },
        "deletable": deletable(card),
        "delete_why": if deletable(card) { "Delete the ticket and its files".to_string() } else if imported(card) { "Imported from the tracker: it goes when its source is disconnected".to_string() } else { can["delete_why"].as_str().filter(|s| !s.is_empty()).unwrap_or("A thread card: it goes with its plan").to_string() },
        "movable": can["reorder"] == true,
        "move_why": can["reorder_why"],
        "validate": if col == "merged" && !card["n"].is_null() { json!({ "on": card["validated"] == true }) } else { J::Null },
        "priority": card["priority"],
        "priorities": if editable(card) && col == "planned" { json!([{ "id": "none", "label": "No priority" }, { "id": "p1", "label": "P1" }, { "id": "p2", "label": "P2" }, { "id": "p3", "label": "P3" }]) } else { J::Null },
        "loading": view.is_none() && !card["n"].is_null(),
        "plan": can_plan(card),
    });
    // An imported ticket: read-only here, one line saying where it is edited.
    if imported(card) {
        let tracker = tracker_label(card["origin"]["tracker"].as_str().unwrap_or(""));
        let url = issue_url(card).is_some();
        v["imported"] = json!({
            "key": card["origin"]["key"],
            "line": format!("Imported from {tracker}: its title and description are edited there."),
            "open": url,
            "open_label": format!("Open in {tracker}"),
            "open_help": if url { format!("Open the issue in {tracker}, in your browser") } else { "Its link is not a web address: not opened".to_string() },
            "closed": card["origin"]["upstream"] == "closed",
        });
    }
    // Send to the lead: only a ticket no thread works on, and not now when
    // something holds the window (the core's dialog, a Jarvis proposal).
    let why = if can["send"] != true { can["send_why"].as_str().filter(|s| !s.is_empty()).unwrap_or("Not a planned ticket").to_string() } else { held.unwrap_or("").to_string() };
    v["send_why"] = if why.is_empty() { J::Null } else { why.into() };
    if let Some(t) = view {
        v["description"] = t["description"].clone();
        v["files_label"] = t["files_label"].clone();
        // Where the files go: one quiet line, so attaching is never a surprise later.
        if t["files"].as_array().is_some_and(|f| !f.is_empty()) {
            v["files_note"] = format!("These files are copied into {sb} when an agent reads this ticket (ticket_read), as the lead does when it takes it.").into();
        }
        v["files"] = t["files"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|f| {
                let id = f["id"].as_str().unwrap_or("");
                let img = image_key(sb, card["n"].as_u64().unwrap_or(0) as u32, id);
                let picture = f["picture"] == true;
                json!({
                    "id": id, "name": f["name"], "size": f["size"], "icon": f["icon"], "picture": picture,
                    "img": if picture && previews.contains(&img) { img.into() } else { J::Null },
                    "el": format!("kanban-file-{id}"),
                    "el_remove": format!("kanban-file-remove-{id}"),
                    "help": if picture { "A picture: its preview is above; press to save it to Downloads and show it" } else { "Save it to your Downloads folder and show it there" },
                })
            })
            .collect::<Vec<_>>()
            .into();
        v["history"] = t["history"].clone();
    } else if card["n"].is_null() {
        v["description"] = "A thread with no ticket: its card shows its plan's goal for it. The plan dialog has the whole goal.".into();
        v["history"] = json!([]);
        v["files"] = json!([]);
    }
    v
}

/// The person edits this ticket here (not a thread's card, not imported).
pub fn editable(card: &J) -> bool {
    card["can"]["edit"] == true && !imported(card)
}

/// The person deletes this ticket here (an imported one goes with its source).
pub fn deletable(card: &J) -> bool {
    card["can"]["delete"] == true && !imported(card)
}

/// The source line in the Kanban's header, for one sandbox: `None` when the
/// daemon has no ticket sources (an older one: the header is as before).
pub fn source_line(src: &J, now_ms: u64) -> J {
    let st = &src["status"];
    let state = st["state"].as_str().unwrap_or("off");
    if src["configured"] != true || state == "off" {
        return json!({
            "on": false,
            "text": "No ticket source",
            "help": "Import the issues of a repository as tickets in Backlog",
            "tone": "neutral",
        });
    }
    let tracker = tracker_label(src["tracker"].as_str().unwrap_or(""));
    let repo = src["repo"].as_str().unwrap_or("");
    let label = src["label"].as_str().filter(|l| !l.is_empty());
    let what = match label {
        Some(l) => format!("{tracker} {repo}, label {l}"),
        None => format!("{tracker} {repo}"),
    };
    let (status, tone) = match state {
        "syncing" => ("syncing".to_string(), "blue"),
        "failed" => {
            let hint = st["hint"].as_str().filter(|h| !h.is_empty());
            let err = st["error"].as_str().filter(|e| !e.is_empty());
            (format!("sync failed: {}", hint.or(err).unwrap_or("the tracker did not answer")), "red")
        }
        _ => match st["last_ok_ms"].as_u64().filter(|t| *t > 0) {
            Some(t) => (format!("synced {}", crate::feat::files::view::ago(t / 1000, now_ms / 1000)), "green"),
            None => ("not synced yet".to_string(), "neutral"),
        },
    };
    json!({
        "on": true,
        "text": format!("{what} · {status}"),
        "help": match (state, st["error"].as_str().filter(|e| !e.is_empty())) {
            ("failed", Some(e)) => format!("The tracker said: {e}"),
            _ => format!("Issues of {repo} come in as Backlog tickets; sbxw writes each ticket's progress back to its issue"),
        },
        "tone": tone,
        "failed": state == "failed",
        "syncing": state == "syncing",
    })
}

/// The registered picture's key for a file's preview.
pub fn image_key(sb: &str, n: u32, file: &str) -> String {
    format!("kanban-img-{}-{n}-{file}", sb.replace(|c: char| !c.is_ascii_alphanumeric() && c != '-', "-"))
}
