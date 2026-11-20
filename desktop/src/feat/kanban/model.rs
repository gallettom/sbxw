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

pub const COLUMNS: &[&str] = &["planned", "progress", "test", "ready", "merged"];

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
    let marks: Vec<J> = c["marks"].as_array().into_iter().flatten().map(|m| json!({ "label": m["label"], "tone": m["tone"], "help": m["help"] })).collect();
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
        "marks": card["marks"].as_array().cloned().unwrap_or_default().iter().map(|m| json!({ "label": m["label"], "tone": m["tone"], "help": m["help"] })).collect::<Vec<_>>(),
        "thread": thread.map(|t| json!({ "name": t, "state": state, "tone": card["thread_tone"], "color": crate::feat::worktrees::graph::lane_color(t) })),
        "editable": can["edit"] == true,
        "edit_why": if can["edit"] == true { J::Null } else { "Recorded from a thread: nothing to edit".into() },
        "deletable": can["delete"] == true,
        "delete_why": if can["delete"] == true { "Delete the ticket and its files".to_string() } else { can["delete_why"].as_str().filter(|s| !s.is_empty()).unwrap_or("A thread card: it goes with its plan").to_string() },
        "movable": can["reorder"] == true,
        "move_why": can["reorder_why"],
        "validate": if col == "merged" && !card["n"].is_null() { json!({ "on": card["validated"] == true }) } else { J::Null },
        "priority": card["priority"],
        "priorities": if can["edit"] == true && col == "planned" { json!([{ "id": "none", "label": "No priority" }, { "id": "p1", "label": "P1" }, { "id": "p2", "label": "P2" }, { "id": "p3", "label": "P3" }]) } else { J::Null },
        "loading": view.is_none() && !card["n"].is_null(),
    });
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

/// The registered picture's key for a file's preview.
pub fn image_key(sb: &str, n: u32, file: &str) -> String {
    format!("kanban-img-{}-{n}-{file}", sb.replace(|c: char| !c.is_ascii_alphanumeric() && c != '-', "-"))
}
