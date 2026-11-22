//! The scope picker: which sandbox a screen shows, or every one. One
//! component, drawn by the core for every screen that asks for it with
//! `select{ ..., scope = "sandbox" }` (docs/screens.md, "Scope"), so every
//! scoped screen draws and behaves the same and no screen can drift.
//!
//! - **Closed**: a capsule in the sandbox's group colour (the colour the
//!   sidebar's group square and the pane header use: [`look`]), its status
//!   dot, its name; "All sandboxes" is neutral with a grid glyph, so the
//!   absence of a filter is seen too. A second level (a thread's worktree, a
//!   branch) follows it as a chip with its own clear cross.
//! - **Open**: "All sandboxes" first (only where the screen offers it), then
//!   a heading per group in the sidebar's tint, a row per sandbox: its group
//!   bar, status dot, name, state, agent, sessions, a check on the current
//!   one. A filter field above [`FILTER_FROM`] sandboxes. Up and Down,
//!   Enter, Escape (the list closes, the view stays), a press outside closes.
//! - **Over an overlay** ([`frame`]): the overlay's top edge, its title band
//!   and its edge take the scope's colour.
//!
//! The rows come from the core's own sandbox data (`data.sandboxes`,
//! `data.app.groups`, `data.sessions`), never from a screen's labels: the
//! screen's `items` only say which sandboxes it offers. Names are data:
//! text only, clipped, the whole name in the tooltip.

use crate::luau::{Item, Node};
use crate::theme::{self, c, hex};
use rune::{col, el, row, text, text_input, Cursor, El, TextEdit};
use serde_json::Value as J;
use std::rc::Rc;

/// The scope kinds the core knows. A `scope` with another kind is drawn as
/// the plain select it is.
pub const KINDS: &[&str] = &["sandbox"];

/// From this many sandboxes on, the open list has a filter field.
pub const FILTER_FROM: usize = 8;

/// The capsule's fill: the group colour at this opacity over `surface`, a
/// step stronger than the pane header's ([`theme::BAR_TINT`]) so it stands
/// as a control on the title band that has the header's tint.
pub const CAPSULE_TINT: f32 = 0.32;
/// The overlay's edge when scoped: the group colour at this opacity over `bg`.
pub const EDGE_TINT: f32 = 0.55;
/// The strip along a scoped overlay's top edge, in points.
pub const STRIP_H: f32 = 3.0;
/// The overlay's title band: the scope's strip along the top edge
/// (`scope::STRIP_H`), then the same room over the title row as under it,
/// so the row (icon, title, "in", the capsule, the cross: all centred on
/// `CONTROL_H`) sits in the middle of the band below the strip.
pub const BAND_PAD: f32 = 8.0;
/// Where the title row starts: under the strip, then [`BAND_PAD`].
pub const HEAD_TOP: f32 = STRIP_H + BAND_PAD;
/// The title band's height (the wash): the strip, the row, its room both sides.
pub const BAND_H: f32 = HEAD_TOP + theme::layout::CONTROL_H + BAND_PAD;

/// Under this overlay width the title drops its "in" ("Files [web-app]").
pub const PHRASE_MIN: f32 = 560.0;
/// The open list: at least this wide (names read in full), at most [`LIST_MAX_W`].
pub const LIST_MIN_W: f32 = 300.0;
pub const LIST_MAX_W: f32 = 420.0;
/// The name in the capsule clips past this width (the tooltip has it whole).
pub const NAME_MAX_W: f32 = 220.0;
/// The second level's name clips past this width.
pub const NARROW_MAX_W: f32 = 160.0;
pub const ROW_H: f32 = 30.0;
pub const HEAD_H: f32 = 22.0;
/// The group's name in a row of a list ordered by count clips past this.
pub const GROUP_W: f32 = 90.0;
/// The open list's panel is never taller than this (and never more than the
/// window's room beside the capsule, 8 pt in: `anchored_fit`). Its rows
/// scroll inside it; the filter and "All sandboxes" stay pinned above them.
pub const LIST_MAX_H: f32 = 420.0;
/// The fade at the rows' scrolling edges, and the room a revealed row keeps from it.
pub const LIST_FADE: f32 = 16.0;

/// What a screen asked for: `scope = "sandbox"` or `scope = { kind =
/// "sandbox", narrow = "thread-a", narrow_icon = "branch", clear = "action" }`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Spec {
    pub kind: String,
    /// The second level (a thread's worktree, a branch), drawn after the capsule.
    pub narrow: Option<String>,
    pub narrow_icon: String,
    /// The screen's action that clears the second level (its cross).
    pub clear: Option<String>,
    /// `all = true`: "All sandboxes" is offered (as an item with id "").
    pub all: bool,
    /// `running = true`: only the running sandboxes (when `items` names none).
    pub running: bool,
    /// `counts = { [sandbox] = n }`: a number per sandbox the screen holds
    /// (Worktrees: its worktrees), shown in each row's chip in place of the
    /// open sessions, its tooltip "<n> <noun>s". None: the sessions.
    pub counts: Option<std::collections::BTreeMap<String, u64>>,
    /// `noun = "worktree"`: what a count counts (singular), for the tooltip.
    pub noun: String,
    /// `only = "counted"`: only the sandboxes whose count is above 0 (the
    /// current one stays, marked, so the capsule never names a missing row).
    pub only_counted: bool,
    /// `order = "count"`: the largest count first, ties by name, no group
    /// headings (each row keeps its group bar and names its group quietly).
    pub by_count: bool,
    /// `order = "name"`: the daemon's order (by name), grouped by workspace,
    /// in place of the sidebar's tiers (the default). No screen asks today.
    pub by_name: bool,
    /// `counting = true`: the counts are still being read (the empty list
    /// says so instead of "none").
    pub counting: bool,
}

/// The `scope` prop read, when its kind is one the core knows.
pub fn spec(v: &J) -> Option<Spec> {
    let (kind, t) = match v {
        J::String(k) => (k.clone(), None),
        J::Object(o) => (o.get("kind").and_then(J::as_str).unwrap_or("").to_string(), Some(o)),
        _ => return None,
    };
    if !KINDS.contains(&kind.as_str()) {
        return None;
    }
    let s = |k: &str| t.and_then(|o| o.get(k)).and_then(J::as_str).filter(|x| !x.is_empty()).map(str::to_string);
    let icon = s("narrow_icon").filter(|i| crate::icons::get(i).is_some()).unwrap_or_else(|| "branch".into());
    let b = |k: &str| t.and_then(|o| o.get(k)).and_then(J::as_bool).unwrap_or(false);
    // `counts`: a table of names to numbers (an empty Lua table may come as `[]`).
    let counts = t.and_then(|o| o.get("counts")).and_then(|c| match c {
        J::Object(m) => Some(m.iter().filter_map(|(k, v)| Some((k.clone(), v.as_f64().filter(|n| *n >= 0.0)? as u64))).collect()),
        J::Array(a) if a.is_empty() => Some(Default::default()),
        _ => None,
    });
    Some(Spec {
        kind,
        narrow: s("narrow"),
        narrow_icon: icon,
        clear: s("clear"),
        all: b("all"),
        running: b("running"),
        noun: s("noun").unwrap_or_else(|| "item".into()),
        only_counted: s("only").as_deref() == Some("counted") && counts.is_some(),
        by_count: s("order").as_deref() == Some("count") && counts.is_some(),
        by_name: s("order").as_deref() == Some("name"),
        counts,
        counting: b("counting"),
    })
}

/// One choice of the list: "All" (`id` empty) or a sandbox the core knows.
#[derive(Debug, Clone, PartialEq)]
pub struct Choice {
    pub id: String,
    pub status: String,
    /// The group's name: the workspace folder's last part, "Chats", or none.
    pub group: Option<String>,
    /// The sidebar's block it sits in ([`block_of`]): its heading in the
    /// list in the sidebar's order. None: ungrouped.
    pub block: Option<String>,
    /// The group's colour, as the sidebar draws it (`data.app.groups`).
    pub color: Option<u32>,
    pub agent: String,
    pub sessions: usize,
    /// The screen's count for it (`counts`), when it passed them.
    pub count: Option<u64>,
}

impl Choice {
    pub fn all(&self) -> bool {
        self.id.is_empty()
    }
}

/// `#rrggbb` as `0xRRGGBB`.
pub fn parse_hex(s: &str) -> Option<u32> {
    let h = s.strip_prefix('#')?;
    (h.len() == 6).then(|| u32::from_str_radix(h, 16).ok()).flatten()
}

/// A sandbox's group name, as the sidebar heads its block.
pub fn group_name(sb: &J) -> Option<String> {
    if sb["chat"] == true {
        return Some("Chats".into());
    }
    let ws = sb["workspace"].as_str()?.trim_end_matches('/');
    ws.rsplit('/').next().filter(|s| !s.is_empty()).map(str::to_string)
}

// ---------------------------------------------------------------------------
// The order: the Sandboxes sidebar's tiers, one definition for the core

/// A session's words that say it is over.
const ENDED: &[&str] = &["done", "exited", "failed", "stopped", "ended"];

/// A sandbox's tier, the one rule the sidebar (sandboxes.luau reads it as
/// `data.app.tiers`) and the scope list both order by. From slow facts only
/// (a start, a stop, a session opening or ending), never from working, idle
/// or wants you, which flicker:
///   1  running, with at least one live session (not ended; a session busy
///      on its own background work counts, whatever its state word)
///   2  running without one, or on its way up ("starting…")
///   3  stopped, stopping, failed, unreachable
pub fn tier(sb: &J, sessions: &[J]) -> u8 {
    if !sb["unreachable_since"].is_null() && sb["unreachable_since"] != false {
        return 3;
    }
    let st = sb["status"].as_str().unwrap_or("");
    if st == "running" {
        let name = sb["name"].as_str();
        let live = sessions.iter().any(|s| {
            s["sandbox"].as_str() == name && s["ended"] != true && (s["background"] == true || !ENDED.contains(&s["state"].as_str().unwrap_or("")))
        });
        return if live { 1 } else { 2 };
    }
    if st == "stopped" || st.starts_with("stop") {
        return 3;
    }
    if st.ends_with('\u{2026}') || st.ends_with("...") || st.ends_with("ing") {
        return 2;
    }
    3
}

/// Every sandbox's [`tier`], by name (`data.app.tiers`).
pub fn tiers(data: &J) -> std::collections::HashMap<String, u8> {
    let sessions = data["sessions"].as_array().map(Vec::as_slice).unwrap_or(&[]);
    data["sandboxes"].as_array().into_iter().flatten().filter_map(|sb| Some((sb["name"].as_str()?.to_string(), tier(sb, sessions)))).collect()
}

/// [`tiers`] as the screens read it, `{ [sandbox] = 1 | 2 | 3 }`.
pub fn tiers_json(data: &J) -> J {
    tiers(data).into_iter().map(|(n, t)| (n, J::from(t))).collect::<serde_json::Map<_, _>>().into()
}

/// The sidebar's block of a sandbox: "Chats", a workspace with more than one
/// sandbox (its folder's last part), or none (ungrouped: a workspace of one
/// is not a group, as in the web UI). `sbs`: every sandbox the core knows.
pub fn block_of(sb: &J, sbs: &[J]) -> Option<String> {
    if sb["chat"] == true {
        return Some("Chats".into());
    }
    let ws = sb["workspace"].as_str().filter(|w| !w.is_empty())?;
    let n = sbs.iter().filter(|o| o["chat"] != true && o["workspace"].as_str() == Some(ws)).count();
    (n > 1).then(|| group_name(sb)).flatten()
}

/// The choices in the sidebar's order: by tier; a group ranks by its best
/// member and comes before an ungrouped sandbox of the same tier; groups of
/// one tier in the sidebar's order (Chats, then by folder name); inside a
/// group, and among ungrouped sandboxes, by tier then the daemon's order.
/// All stays first. `tier`: the tier of each (one not known ranks 3).
pub fn by_tier(list: Vec<Choice>, data: &J, tier: &dyn Fn(&str) -> u8) -> Vec<Choice> {
    let sbs = data["sandboxes"].as_array().map(Vec::as_slice).unwrap_or(&[]);
    let index = |id: &str| sbs.iter().position(|s| s["name"].as_str() == Some(id)).unwrap_or(usize::MAX);
    let (all, mut rest): (Vec<Choice>, Vec<Choice>) = list.into_iter().partition(Choice::all);
    rest.sort_by_key(|c| (tier(&c.id), index(&c.id)));
    // The groups' own order: Chats first, then by the folder's name.
    let group_at = |b: &str| (b != "Chats", b.to_string());
    let best = |b: &str, rest: &[Choice]| rest.iter().filter(|c| c.block.as_deref() == Some(b)).map(|c| tier(&c.id)).min().unwrap_or(3);
    let snapshot = rest.clone();
    rest.sort_by_cached_key(|c| match &c.block {
        Some(b) => (best(b, &snapshot), 0, group_at(b), 0, 0),
        None => (tier(&c.id), 1, (false, String::new()), index(&c.id), 0),
    });
    all.into_iter().chain(rest).collect()
}

// The order an open list keeps, by `<screen>/<select id>`: set when the
// list opens, so rows never move under the pointer or the keyboard while
// it is open (their status still updates); dropped when it closes.
thread_local! {
    static HELD: std::cell::RefCell<std::collections::HashMap<String, Vec<String>>> = Default::default();
}

/// `list` in the order held for `at` (taken from `list` the first time):
/// a sandbox that left is gone, one that came goes last.
pub fn hold(at: &str, list: Vec<Choice>) -> Vec<Choice> {
    HELD.with(|h| {
        let mut h = h.borrow_mut();
        let order = h.entry(at.to_string()).or_insert_with(|| list.iter().map(|c| c.id.clone()).collect());
        let mut list = list;
        list.sort_by_key(|c| order.iter().position(|o| o == &c.id).unwrap_or(usize::MAX));
        for c in &list {
            if !order.contains(&c.id) {
                order.push(c.id.clone());
            }
        }
        list
    })
}

/// The list at `at` is closed: the next open orders afresh.
pub fn release(at: &str) {
    HELD.with(|h| {
        h.borrow_mut().remove(at);
    });
}

/// Whether the picker offers All: `all = true`, or an item with id "".
pub fn offers_all(sp: &Spec, items: &[Item]) -> bool {
    sp.all || items.iter().any(|i| i.id.is_empty() && !i.divider)
}

/// The choices a screen offers, from the core's data: every sandbox the
/// core knows (only the running ones with `running = true`) when the
/// screen's `items` name none, else those items' sandboxes in their order
/// (one the core does not know is left out). All first when offered.
/// Grouped: the groups in the order their first sandbox comes.
pub fn choices(data: &J, items: &[Item], sp: &Spec) -> Vec<Choice> {
    let sbs = data["sandboxes"].as_array().cloned().unwrap_or_default();
    let named = items.iter().any(|i| !i.id.is_empty() && !i.divider);
    let every: Vec<Item>;
    let items: &[Item] = if named {
        items
    } else {
        every = offers_all(sp, items).then(|| Item::plain("", "", None)).into_iter()
            .chain(sbs.iter().filter(|s| !sp.running || s["status"] == "running").filter_map(|s| s["name"].as_str()).map(|n| Item::plain(n, n, None)))
            .collect();
        &every
    };
    let groups = &data["app"]["groups"];
    let sessions = data["sessions"].as_array();
    let mut all = None;
    let mut list: Vec<Choice> = vec![];
    for it in items.iter().filter(|i| !i.divider) {
        if it.id.is_empty() && !offers_all(sp, items) {
            continue;
        }
        if it.id.is_empty() {
            all.get_or_insert(Choice { id: String::new(), status: String::new(), group: None, block: None, color: None, agent: String::new(), sessions: 0, count: None });
            continue;
        }
        let Some(sb) = sbs.iter().find(|s| s["name"].as_str() == Some(it.id.as_str())) else { continue };
        if list.iter().any(|c| c.id == it.id) {
            continue;
        }
        let n = sessions.map_or(0, |l| l.iter().filter(|x| x["sandbox"].as_str() == Some(it.id.as_str()) && x["ended"] != true).count());
        list.push(Choice {
            id: it.id.clone(),
            status: sb["status"].as_str().unwrap_or("").to_string(),
            group: group_name(sb),
            block: block_of(sb, &sbs),
            color: groups[it.id.as_str()].as_str().and_then(parse_hex),
            agent: sb["agent"].as_str().unwrap_or("").to_string(),
            sessions: n,
            count: sp.counts.as_ref().map(|m| m.get(&it.id).copied().unwrap_or(0)),
        });
    }
    // Stable grouping: by the first index of each group.
    let first = |g: &Option<String>, list: &[Choice]| list.iter().position(|c| &c.group == g).unwrap_or(0);
    let snapshot = list.clone();
    list.sort_by_key(|c| first(&c.group, &snapshot));
    all.into_iter().chain(list).collect()
}

/// The choices the list offers for `value` (the current one): [`choices`],
/// then `only = "counted"` (those counted above 0, and the current one
/// always), in the sidebar's order ([`by_tier`]), or with `order = "count"`
/// the largest count first, ties by tier then name, or with `order =
/// "name"` the daemon's order grouped by workspace ([`choices`]' own).
pub fn offered(data: &J, items: &[Item], sp: &Spec, value: &str) -> Vec<Choice> {
    let mut list = choices(data, items, sp);
    if sp.only_counted {
        list.retain(|c| c.all() || c.count.unwrap_or(0) > 0 || c.id == value);
    }
    let t = tiers(data);
    let tier = |id: &str| t.get(id).copied().unwrap_or(3);
    if sp.by_count {
        // All stays first (its count is none: rank it above every number).
        list.sort_by(|a, b| {
            b.all().cmp(&a.all()).then(b.count.unwrap_or(0).cmp(&a.count.unwrap_or(0))).then(tier(&a.id).cmp(&tier(&b.id))).then_with(|| a.id.cmp(&b.id))
        });
    } else if !sp.by_name {
        list = by_tier(list, data, &tier);
    }
    list
}

/// The list's headings follow the sidebar's blocks (the default order).
fn by_block(sp: &Spec) -> bool {
    !sp.by_count && !sp.by_name
}

/// "4 worktrees", "1 worktree".
pub fn count_words(n: u64, noun: &str) -> String {
    format!("{n} {noun}{}", if n == 1 { "" } else { "s" })
}

/// The choices whose name (or group's name) holds `q`, any case. All stays.
pub fn filter<'a>(list: &'a [Choice], q: &str) -> Vec<&'a Choice> {
    let q = q.trim().to_lowercase();
    list.iter().filter(|c| c.all() || q.is_empty() || c.id.to_lowercase().contains(&q) || c.group.as_deref().is_some_and(|g| g.to_lowercase().contains(&q))).collect()
}

/// The status dot's tone (THEME.md's status dots): running green, working
/// blue, failed red, stopped a ring.
pub fn status_tone(status: &str) -> &'static str {
    match status {
        "running" => "green",
        "stopped" | "" => "neutral",
        "failed" | "error" => "red",
        _ => "blue",
    }
}

/// The capsule's colours: its fill, its border, its text and secondary text
/// (both at 4.5:1 on the fill), and its status marks' ground.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Look {
    pub fill: u32,
    pub border: u32,
    pub text: u32,
    pub muted: u32,
}

/// The capsule's look for a sandbox of group colour `color` (none: neutral,
/// as "All sandboxes" and a sandbox in no group).
pub fn look(color: Option<u32>) -> Look {
    let v = |n| theme::color_value(n).unwrap();
    match color {
        Some(g) => {
            let fill = theme::over(g, CAPSULE_TINT, v("surface"));
            Look { fill, border: g, text: v("text-bright"), muted: theme::muted_on(fill) }
        }
        None => Look { fill: v("surface"), border: v("control-border"), text: v("text-bright"), muted: theme::muted_on(v("surface")) },
    }
}

/// [`look`] as a screen reads it (`#rrggbb` strings): for a picker a screen
/// draws itself (the Board's team picker, two lines a row).
pub fn look_json(color: Option<u32>) -> J {
    let l = look(color);
    let h = |c: u32| J::String(format!("#{c:06x}"));
    serde_json::json!({ "fill": h(l.fill), "border": h(l.border), "text": h(l.text), "muted": h(l.muted) })
}

/// A scoped overlay's frame: the strip along its top edge, the wash of its
/// title band, its edge. One function, so the frame can be on (the
/// "coloured frame"), or off (the capsule alone: [`FRAME`] false).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Frame {
    pub strip: Option<u32>,
    pub wash: Option<u32>,
    pub edge: u32,
}

/// Whether a scoped overlay carries its scope's colour on its frame.
pub const FRAME: bool = true;

/// The frame of an overlay scoped to a sandbox of group colour `color`
/// (`None`: not scoped, All, or no group: today's neutral frame). The title
/// band has the pane header's tint ([`theme::bar_ground`], focused), so the
/// overlay and that sandbox's terminal read as one.
pub fn frame(color: Option<u32>) -> Frame {
    let v = |n| theme::color_value(n).unwrap();
    match color.filter(|_| FRAME) {
        Some(g) => Frame { strip: Some(g), wash: Some(theme::bar_ground(Some(g), true)), edge: theme::over(g, EDGE_TINT, v("bg")) },
        None => Frame { strip: None, wash: None, edge: v("elevated") },
    }
}

/// The scope select in a screen's tree (the first `select` with a known
/// `scope`, at any depth) inside its `title-slot` row: its spec and value.
pub fn in_slot(n: &Node) -> Option<(Spec, String)> {
    fn find(n: &Node, slot: bool) -> Option<(Spec, String)> {
        let slot = slot || n.key.as_deref() == Some(crate::screen::TITLE_SLOT);
        if slot && n.kind == crate::luau::Kind::Select {
            if let Some(s) = spec(&n.scope) {
                return Some((s, n.value.as_str().unwrap_or("").to_string()));
            }
        }
        n.children.iter().find_map(|k| find(k, slot))
    }
    find(n, false)
}

// ---------------------------------------------------------------------------
// Drawing

/// The status mark: a dot in its tone, a ring when stopped.
fn status_mark(status: &str, ground: u32) -> El {
    let tone = status_tone(status);
    let fg = theme::mark_on(crate::screen::tone(tone).0, ground);
    if tone == "neutral" {
        el().w(8.0).h(8.0).radius(theme::corner::PILL).border(1.5, hex(theme::muted_on(ground)))
    } else {
        el().w(8.0).h(8.0).radius(theme::corner::PILL).fill(hex(fg))
    }
}

fn icon(name: &str, size: f32, color: u32) -> El {
    rune::icon(crate::icons::get(name).expect("known icon"), size).color(hex(color))
}

/// What the capsule says to a screen reader and in its tooltip.
pub fn describe(ch: Option<&Choice>, value: &str) -> String {
    match ch {
        Some(c) if c.all() => "All sandboxes: pick one sandbox".into(),
        Some(c) => {
            let mut s = format!("{} ({}", c.id, if c.status.is_empty() { "unknown" } else { &c.status });
            if let Some(g) = &c.group {
                s.push_str(&format!(", group {g}"));
            }
            s.push_str("): pick another sandbox");
            s
        }
        None if value.is_empty() => "Pick a sandbox".into(),
        None => format!("{value}: pick another sandbox"),
    }
}

/// The closed capsule. `key` is the select's key (scripts and tests press
/// it); a change of `value` plays one short pop of its face (none under
/// reduced motion): the person sees the scope changed.
/// `empty`: what an empty value says, and whether it is All (the grid
/// glyph) or no sandbox at all (a screen with none to offer: its placeholder).
pub fn capsule<S: 'static>(key: &str, value: &str, ch: Option<&Choice>, empty: (&str, bool), open: bool, disabled: bool, toggle: impl Fn(&mut S) + 'static) -> El {
    let lk = look(ch.and_then(|c| c.color));
    let all = value.is_empty();
    let (all_label, is_all) = empty;
    let mut face = row().key(format!("{key}-face-{value}")).gap(8.0).items_center().shrink();
    if all && is_all {
        face = face.child(icon("grid", theme::icon::MEDIUM, lk.muted)).child(text(all_label.to_string()).font(crate::screen::body(crate::theme::font_size::BODY, 500.0)).color(hex(lk.text)));
    } else if all {
        face = face.child(text(all_label.to_string()).font(crate::screen::body(crate::theme::font_size::BODY, 400.0)).color(hex(lk.muted)));
    } else {
        face = face.child(status_mark(ch.map_or("", |c| c.status.as_str()), lk.fill)).child(text(value.to_string()).font(crate::screen::body(crate::theme::font_size::BODY, 500.0)).color(hex(lk.text)).truncate().shrink().max_w(NAME_MAX_W));
    }
    if !crate::screen::reduced_motion() {
        face = face.transition(rune::Transition::scale(0.9).combined(rune::Transition::opacity()).springs(rune::SpringPair::new(rune::Spring::new(theme::motion::SHORT, 0.0), rune::Spring::new(theme::motion::MICRO_OUT, 0.0))));
    }
    let name = if all { all_label.to_string() } else { value.to_string() };
    let mut t = row()
        .key(key.to_string())
        .role("combobox")
        .label(match ch.filter(|c| !c.all() && !c.status.is_empty()) {
            Some(c) => format!("Scope: {name} ({})", c.status),
            None => format!("Scope: {name}"),
        })
        .help(describe(ch, value))
        .expanded(open)
        .h(theme::layout::CONTROL_H)
        .px(12.0)
        .gap(8.0)
        .items_center()
        .shrink()
        .radius(theme::radius("pill").unwrap())
        .fill(hex(lk.fill))
        .border(if open { 2.0 } else { 1.0 }, hex(lk.border))
        .focus_ring(crate::theme::focus::W, crate::theme::focus::OUT, c("focus"))
        .child(face)
        .child(icon("chevron-down", theme::icon::SMALL, lk.muted));
    if disabled {
        return t.opacity(theme::DISABLED_OPACITY).disabled(true);
    }
    t = t.cursor(Cursor::Pointer).on_press(toggle);
    t
}

/// The second level after the capsule: "› ⑂ thread-a ×".
pub fn narrow_chip<S: 'static>(key: &str, sp: &Spec, clear: Option<Box<dyn Fn(&mut S)>>) -> Option<El> {
    let name = sp.narrow.clone()?;
    let muted = theme::color_value("text-muted").unwrap();
    let mut r = row().key(format!("{key}-narrow")).gap(4.0).items_center().shrink().help(name.clone())
        .child(icon("chevron-right", theme::icon::SMALL, muted))
        .child(icon(&sp.narrow_icon, theme::icon::SMALL, muted))
        .child(text(name.clone()).font(crate::screen::body(crate::theme::font_size::BODY, 500.0)).color(c("text-bright")).truncate().shrink().max_w(NARROW_MAX_W));
    if let Some(f) = clear {
        r = r.child(
            el().key(format!("{key}-narrow-clear")).role("button").label(format!("Clear {name}")).help(format!("Back to the whole sandbox, not only {name}"))
                .w(theme::layout::DENSE_H).h(theme::layout::DENSE_H).items_center().justify_center().radius(theme::radius("xs").unwrap())
                .hover(c("elevated")).focus_ring(crate::theme::focus::W, crate::theme::focus::IN, c("focus")).cursor(Cursor::Pointer).on_press(move |s: &mut S| f(s))
                .child(icon("close", theme::icon::SMALL, muted)),
        );
    }
    Some(r)
}

/// The list's width: the longest name in full, between the bounds.
pub fn list_width(list: &[Choice], sp: &Spec) -> f32 {
    (list_width_of(list) + if sp.by_count { GROUP_W } else { 0.0 }).clamp(LIST_MIN_W, LIST_MAX_W)
}

fn list_width_of(list: &[Choice]) -> f32 {
    let longest = list.iter().map(|c| c.id.chars().count().max(c.group.as_deref().map_or(0, |g| g.chars().count()))).max().unwrap_or(0) as f32;
    // name at 13 pt (0.6 em a character), mark, state, agent, sessions, check.
    longest * 13.0 * 0.6 + 8.0 + 3.0 + 8.0 + 8.0 + 64.0 + 56.0 + 28.0 + 20.0 + 16.0 + 40.0
}

/// What the open list needs from the screen: its keys, the filter's edit
/// (core-held), and what a pick and a close do.
pub struct ListCtx<S> {
    pub key: String,
    pub value: String,
    pub all_label: String,
    /// The window's width: the list never leaves it.
    pub window_w: f32,
    pub filter: Option<TextEdit>,
    /// What the screen asked for: counts, `only`, `order`.
    pub spec: Spec,
    pub edit: Rc<dyn Fn(&mut S) -> &mut TextEdit>,
    pub pick: Rc<dyn Fn(&mut S, &str)>,
    pub close: Rc<dyn Fn(&mut S)>,
}

/// The key of a choice's row: `<select key>-<sandbox>`, and `<key>-` for All
/// (as the core's select rows, `<id>-<item>`). A script waits on `<id>-list`, then taps a row.
pub fn row_key(key: &str, c: &Choice) -> String {
    format!("{key}-{}", c.id)
}

/// The open list: anchored under the capsule, on the menus' surface.
pub fn list<S: 'static>(cx: ListCtx<S>, choices: &[Choice]) -> El {
    let q = cx.filter.as_ref().map(|e| e.text.clone()).unwrap_or_default();
    let shown = filter(choices, &q);
    let keys: Vec<String> = shown.iter().map(|c| row_key(&cx.key, c)).collect();
    let filter_key = format!("{}-filter", cx.key);
    let w = list_width(choices, &cx.spec).min((cx.window_w - 16.0).max(200.0));
    // Modal while open: the keyboard starts in it (the filter, else the
    // current row), Escape closes only the list, then the keyboard goes
    // back to the capsule. A press outside closes it and does nothing else.
    let close0 = cx.close.clone();
    // Sized to the window (`anchored_fit`): at most LIST_MAX_H, at most the
    // room under (or over) the capsule; the rows' viewport shrinks and scrolls.
    let mut l = crate::screen::menu_surface(format!("{}-list", cx.key), cx.key.clone(), rune::Align::Start, w)
        .anchored_fit(cx.key.clone(), rune::Side::Bottom, rune::Align::Start, 4.0)
        .gap(2.0)
        .max_h(LIST_MAX_H)
        .modal(true, move |s: &mut S| close0(s))
        .focus_return(cx.key.clone());
    // The field, when there are many: typed text narrows the rows; Down goes
    // to the first row, Enter picks the first match, Escape closes.
    if let Some(edit) = &cx.filter {
        let (e2, first, pick, close) = (cx.edit.clone(), keys.first().cloned(), cx.pick.clone(), cx.close.clone());
        let first_id = shown.iter().find(|c| !c.all() || shown.len() == 1).map(|c| c.id.clone());
        let input = text_input(edit, crate::screen::body(crate::theme::font_size::BODY, 400.0))
            .single_line()
            .key(filter_key.clone())
            .label("Filter sandboxes")
            .placeholder("Filter sandboxes", c("text-muted"))
            .color(c("text-bright"))
            .autofocus()
            .w_full()
            .on_edit(move |s: &mut S| e2(s))
            .on_input_key(move |s: &mut S, ev: &rune::KeyEvent, tx: &mut rune::Tx| match ev.key {
                rune::Key::Down => {
                    if let Some(k) = &first {
                        tx.focus(k.clone(), true);
                        tx.scroll_into_view(k.clone());
                    }
                    true
                }
                rune::Key::Enter => {
                    if let Some(id) = &first_id {
                        pick(s, id);
                    }
                    true
                }
                rune::Key::Escape => {
                    close(s);
                    true
                }
                _ => false,
            });
        l = l.child(el().w_full().h(theme::layout::CONTROL_H).px(8.0).items_center().fill(c("bg")).border(1.0, c("control-border")).radius(theme::radius("sm").unwrap()).focus_within_ring(crate::theme::focus::W, crate::theme::focus::NEAR, c("focus")).child(input));
    }
    // The rows scroll inside the panel (it never grows past LIST_MAX_H or the
    // window): `shrink` and `min_h(0)` let the viewport be shorter than its
    // content. "All sandboxes" stays pinned above it, as the filter does.
    let mut rows = col().key(format!("{}-rows", cx.key)).w_full().gap(2.0).shrink().min_h(0.0)
        .scroll(rune::ScrollSpec { stick_bottom: false }).scroll_edge_fade(LIST_FADE).scroll_margin(LIST_FADE);
    // The current row in view on open (the keyboard may be in the filter):
    // on the viewport's first layout, and again only if its size changes.
    if let Some(cur) = shown.iter().find(|c| !c.all() && c.id == cx.value) {
        let k = row_key(&cx.key, cur);
        rows = rows.on_layout_tx(move |_s: &mut S, _r: [f32; 4], tx: &mut rune::Tx| tx.scroll_into_view(k.clone()));
    }
    let mut last_group: Option<Option<String>> = None;
    let mut runs = 0;
    for (i, ch) in shown.iter().enumerate() {
        // Ordered by count, no headings: they would break the order (each
        // row keeps its group bar and names its group). In the sidebar's
        // order a heading per group block, as the sidebar's; ungrouped
        // sandboxes after a group get a hairline, not a heading (the
        // sidebar's plain block).
        let g = if by_block(&cx.spec) { &ch.block } else { &ch.group };
        if !ch.all() && !cx.spec.by_count && last_group.as_ref() != Some(g) {
            if g.is_some() || !by_block(&cx.spec) {
                rows = rows.child(heading(&cx.key, g, ch.color));
            } else if last_group.is_some() {
                runs += 1;
                rows = rows.child(el().key(format!("{}-ungrouped-{runs}", cx.key)).w_full().py(theme::spacing::XS).child(el().w_full().h(1.0).fill(c("elevated"))));
            }
            last_group = Some(g.clone());
        }
        let prev = if i == 0 { cx.filter.as_ref().map(|_| filter_key.clone()) } else { Some(keys[i - 1].clone()) };
        let next = keys.get(i + 1).cloned();
        let focus_me = cx.filter.is_none() && (ch.id == cx.value || (i == 0 && !shown.iter().any(|c| c.id == cx.value)));
        let r = choice_row(&cx, ch, &keys[i], prev, next, focus_me);
        if ch.all() {
            l = l.child(r);
        } else {
            rows = rows.child(r);
        }
    }
    if shown.iter().all(|c| c.all()) && q.trim().is_empty() && cx.spec.only_counted {
        let line = if cx.spec.counting { format!("Counting the {}s\u{2026}", cx.spec.noun) } else { format!("No sandbox has a {}", cx.spec.noun) };
        rows = rows.child(el().key(format!("{}-none", cx.key)).w_full().px(8.0).py(6.0).child(text(line).font(crate::screen::body(crate::theme::font_size::CAPTION, 400.0)).color(c("text-muted")).truncate().min_w(0.0)));
    }
    if shown.iter().all(|c| c.all()) && !q.trim().is_empty() {
        rows = rows.child(el().w_full().px(8.0).py(6.0).child(text(format!("No sandbox matches \u{201c}{}\u{201d}", q.trim())).font(crate::screen::body(crate::theme::font_size::CAPTION, 400.0)).color(c("text-muted")).truncate().min_w(0.0)));
    }
    l.child(rows)
}

/// A group's heading in the list: its tint and label colour, as the sidebar heads its block.
fn heading(key: &str, group: &Option<String>, color: Option<u32>) -> El {
    let name = group.clone().unwrap_or_else(|| "No group".into());
    let color = color.filter(|_| group.is_some());
    let (tint, label) = match color {
        Some(g) => {
            let (t, _, l) = theme::group_tints(g);
            (t, l)
        }
        None => (theme::color_value("surface").unwrap(), theme::color_value("text-muted").unwrap()),
    };
    let sq = el().w(8.0).h(8.0).radius(theme::corner::MARK);
    let sq = match color {
        Some(g) => sq.fill(hex(g)),
        None => sq.border(1.0, c("text-muted")),
    };
    row().key(format!("{key}-group-{name}")).w_full().h(HEAD_H).px(8.0).gap(crate::theme::spacing::INLINE).items_center().radius(theme::radius("xs").unwrap()).fill(hex(tint)).help(name.clone())
        .child(sq)
        .child(text(name).font(crate::screen::body(crate::theme::font_size::CAPTION, 600.0)).color(hex(label)).truncate().min_w(0.0))
}

/// A row's count: the screen's count (`counts`: "4 worktrees") or, without,
/// its open sessions. None at 0, except the current row of an `only =
/// "counted"` list, which says it has none any more.
fn chip(sp: &Spec, ch: &Choice, key: &str) -> Option<El> {
    let k = format!("{key}-count");
    let (n, tip) = match ch.count {
        Some(n) => (n, count_words(n, &sp.noun)),
        None => (ch.sessions as u64, count_words(ch.sessions as u64, "session")),
    };
    if n == 0 {
        return (sp.only_counted && ch.count.is_some()).then(|| {
            text(format!("no {}", sp.noun)).key(k).font(crate::screen::body(crate::theme::font_size::CAPTION, 400.0)).color(c("text-muted")).help(format!("No {} any more: shown because it is the current one", sp.noun))
        });
    }
    Some(el().key(k).px(crate::theme::spacing::CHIP_X).py(crate::theme::spacing::CHIP_Y).radius(theme::radius("xs").unwrap()).fill(c("elevated")).help(tip).child(text(n.to_string()).font(crate::screen::body(crate::theme::font_size::CAPTION, 500.0)).color(c("text"))))
}

fn choice_row<S: 'static>(cx: &ListCtx<S>, ch: &Choice, key: &str, prev: Option<String>, next: Option<String>, focus_me: bool) -> El {
    let current = ch.id == cx.value;
    let surface = theme::color_value("surface").unwrap();
    let muted = theme::color_value("text-muted").unwrap();
    let mut r = row().key(key.to_string()).w_full().h(ROW_H).px(8.0).gap(8.0).items_center().radius(theme::radius("xs").unwrap()).role("menuitem").focusable(true);
    if ch.all() {
        r = r.label(cx.all_label.clone()).help("Every sandbox: no filter")
            .child(icon("grid", theme::icon::MEDIUM, muted))
            .child(text(cx.all_label.clone()).font(crate::screen::body(crate::theme::font_size::BODY, if current { 600.0 } else { 500.0 })).color(c("text-bright")).grow(1.0).min_w(0.0).truncate())
            .child(text("no filter").font(crate::screen::body(crate::theme::font_size::CAPTION, 400.0)).color(c("text-muted")));
    } else {
        let bar = el().w(3.0).h(18.0).radius(theme::corner::MARK);

        let bar = match ch.color {
            Some(g) => bar.fill(hex(g)),
            None => bar.fill(c("elevated")),
        };
        let mut tip = format!("{} ({})", ch.id, if ch.status.is_empty() { "unknown" } else { &ch.status });
        if !ch.agent.is_empty() {
            tip.push_str(&format!(", {}", ch.agent));
        }
        if let Some(g) = ch.group.as_ref().filter(|_| cx.spec.by_count) {
            tip.push_str(&format!(", group {g}"));
        }
        if let Some(n) = ch.count {
            tip.push_str(&format!(", {}", count_words(n, &cx.spec.noun)));
        }
        r = r.label(format!("{}, {}", ch.id, if ch.status.is_empty() { "unknown" } else { &ch.status })).help(tip)
            .child(bar)
            .child(status_mark(&ch.status, surface))
            .child(text(ch.id.clone()).font(crate::screen::body(crate::theme::font_size::BODY, if current { 600.0 } else { 500.0 })).color(c("text-bright")).grow(1.0).min_w(0.0).truncate())
            // In the order of the count, the group is named here (no headings).
            .children(cx.spec.by_count.then(|| {
                text(ch.group.clone().unwrap_or_default()).font(crate::screen::body(crate::theme::font_size::CAPTION, 400.0)).color(c("text-muted")).truncate().shrink().max_w(GROUP_W)
            }))
            .children(chip(&cx.spec, ch, &key))
            .child(el().w(64.0).child(text(ch.status.clone()).font(crate::screen::body(crate::theme::font_size::CAPTION, 400.0)).color(c("text-muted")).truncate().min_w(0.0)))
            .child(el().w(56.0).child(text(ch.agent.clone()).font(crate::screen::body(crate::theme::font_size::CAPTION, 400.0)).color(c("text-muted")).truncate().min_w(0.0)));
    }
    r = r.child(el().w(12.0).items_center().justify_center().children(current.then(|| icon("check", theme::icon::SMALL, theme::color_value("accent").unwrap()))));
    if current {
        r = r.fill(c("active-bg")).focus_selected(true);
    }
    if focus_me {
        r = r.autofocus();
    }
    let (pick, id, me) = (cx.pick.clone(), ch.id.clone(), key.to_string());
    let (pick2, id2) = (cx.pick.clone(), ch.id.clone());
    r.hover(c("elevated")).focus_highlight(c("elevated")).focus_ring(crate::theme::focus::W, crate::theme::focus::IN, c("focus")).cursor(Cursor::Pointer)
        .on_press(move |s: &mut S| pick(s, &id))
        .on_key(move |s: &mut S, ev: &rune::KeyEvent, tx: &mut rune::Tx| {
            if tx.focused() != Some(me.as_str()) || ev.mods.cmd || ev.mods.ctrl || ev.mods.alt || ev.mods.logo {
                return false;
            }
            match ev.key {
                rune::Key::Up | rune::Key::Down => {
                    if let Some(k) = if ev.key == rune::Key::Up { &prev } else { &next } {
                        tx.focus(k.clone(), true);
                        // The focused row stays in the scrolling part's view.
                        tx.scroll_into_view(k.clone());
                    }
                    true
                }
                rune::Key::Enter => {
                    pick2(s, &id2);
                    true
                }
                _ => false,
            }
        })
}

#[cfg(test)]
#[path = "scope_tests.rs"]
mod tests;
