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
    Some(Spec { kind, narrow: s("narrow"), narrow_icon: icon, clear: s("clear"), all: b("all"), running: b("running") })
}

/// One choice of the list: "All" (`id` empty) or a sandbox the core knows.
#[derive(Debug, Clone, PartialEq)]
pub struct Choice {
    pub id: String,
    pub status: String,
    /// The group's name: the workspace folder's last part, "Chats", or none.
    pub group: Option<String>,
    /// The group's colour, as the sidebar draws it (`data.app.groups`).
    pub color: Option<u32>,
    pub agent: String,
    pub sessions: usize,
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
            all.get_or_insert(Choice { id: String::new(), status: String::new(), group: None, color: None, agent: String::new(), sessions: 0 });
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
            color: groups[it.id.as_str()].as_str().and_then(parse_hex),
            agent: sb["agent"].as_str().unwrap_or("").to_string(),
            sessions: n,
        });
    }
    // Stable grouping: by the first index of each group.
    let first = |g: &Option<String>, list: &[Choice]| list.iter().position(|c| &c.group == g).unwrap_or(0);
    let snapshot = list.clone();
    list.sort_by_key(|c| first(&c.group, &snapshot));
    all.into_iter().chain(list).collect()
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
        el().w(8.0).h(8.0).radius(4.0).border(1.5, hex(theme::muted_on(ground)))
    } else {
        el().w(8.0).h(8.0).radius(4.0).fill(hex(fg))
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
        face = face.child(icon("grid", theme::icon::MEDIUM, lk.muted)).child(text(all_label.to_string()).font(crate::screen::body(13.0, 500.0)).color(hex(lk.text)));
    } else if all {
        face = face.child(text(all_label.to_string()).font(crate::screen::body(13.0, 400.0)).color(hex(lk.muted)));
    } else {
        face = face.child(status_mark(ch.map_or("", |c| c.status.as_str()), lk.fill)).child(text(value.to_string()).font(crate::screen::body(13.0, 500.0)).color(hex(lk.text)).truncate().shrink().max_w(NAME_MAX_W));
    }
    if !crate::screen::reduced_motion() {
        face = face.transition(rune::Transition::scale(0.9).combined(rune::Transition::opacity()).springs(rune::SpringPair::new(rune::Spring::new(0.22, 0.0), rune::Spring::new(0.08, 0.0))));
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
        .focus_ring(2.0, 2.0, c("focus"))
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
        .child(text(name.clone()).font(crate::screen::body(13.0, 500.0)).color(c("text-bright")).truncate().shrink().max_w(NARROW_MAX_W));
    if let Some(f) = clear {
        r = r.child(
            el().key(format!("{key}-narrow-clear")).role("button").label(format!("Clear {name}")).help(format!("Back to the whole sandbox, not only {name}"))
                .w(theme::layout::DENSE_H).h(theme::layout::DENSE_H).items_center().justify_center().radius(theme::radius("xs").unwrap())
                .hover(c("elevated")).focus_ring(2.0, -2.0, c("focus")).cursor(Cursor::Pointer).on_press(move |s: &mut S| f(s))
                .child(icon("close", theme::icon::SMALL, muted)),
        );
    }
    Some(r)
}

/// The list's width: the longest name in full, between the bounds.
pub fn list_width(list: &[Choice]) -> f32 {
    let longest = list.iter().map(|c| c.id.chars().count().max(c.group.as_deref().map_or(0, |g| g.chars().count()))).max().unwrap_or(0) as f32;
    // name at 13 pt (0.6 em a character), mark, state, agent, sessions, check.
    (longest * 13.0 * 0.6 + 8.0 + 3.0 + 8.0 + 8.0 + 64.0 + 56.0 + 28.0 + 20.0 + 16.0 + 40.0).clamp(LIST_MIN_W, LIST_MAX_W)
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
    pub edit: Rc<dyn Fn(&mut S) -> &mut TextEdit>,
    pub pick: Rc<dyn Fn(&mut S, &str)>,
    pub close: Rc<dyn Fn(&mut S)>,
}

/// The key of a choice's row: `<select key>-<sandbox>`, and `<key>-` for All
/// (as the core's select rows, `<id>-<item>`, which `wait:` marks know).
pub fn row_key(key: &str, c: &Choice) -> String {
    format!("{key}-{}", c.id)
}

/// The open list: anchored under the capsule, on the menus' surface.
pub fn list<S: 'static>(cx: ListCtx<S>, choices: &[Choice]) -> El {
    let q = cx.filter.as_ref().map(|e| e.text.clone()).unwrap_or_default();
    let shown = filter(choices, &q);
    let keys: Vec<String> = shown.iter().map(|c| row_key(&cx.key, c)).collect();
    let filter_key = format!("{}-filter", cx.key);
    let w = list_width(choices).min((cx.window_w - 16.0).max(200.0));
    // Modal while open: the keyboard starts in it (the filter, else the
    // current row), Escape closes only the list, then the keyboard goes
    // back to the capsule. A press outside closes it and does nothing else.
    let close0 = cx.close.clone();
    let mut l = crate::screen::menu_surface(format!("{}-list", cx.key), cx.key.clone(), rune::Align::Start, w)
        .gap(2.0)
        .max_h(480.0)
        .modal(true, move |s: &mut S| close0(s))
        .focus_return(cx.key.clone());
    // The field, when there are many: typed text narrows the rows; Down goes
    // to the first row, Enter picks the first match, Escape closes.
    if let Some(edit) = &cx.filter {
        let (e2, first, pick, close) = (cx.edit.clone(), keys.first().cloned(), cx.pick.clone(), cx.close.clone());
        let first_id = shown.iter().find(|c| !c.all() || shown.len() == 1).map(|c| c.id.clone());
        let input = text_input(edit, crate::screen::body(13.0, 400.0))
            .single_line()
            .key(filter_key.clone())
            .label("Filter sandboxes")
            .placeholder("Filter sandboxes", c("text-dim"))
            .color(c("text-bright"))
            .autofocus()
            .w_full()
            .on_edit(move |s: &mut S| e2(s))
            .on_input_key(move |s: &mut S, ev: &rune::KeyEvent, tx: &mut rune::Tx| match ev.key {
                rune::Key::Down => {
                    if let Some(k) = &first {
                        tx.focus(k.clone(), true);
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
        l = l.child(el().w_full().h(theme::layout::CONTROL_H).px(8.0).items_center().fill(c("bg")).border(1.0, c("control-border")).radius(theme::radius("sm").unwrap()).focus_within_ring(2.0, 1.0, c("focus")).child(input));
    }
    let mut rows = col().key(format!("{}-rows", cx.key)).w_full().gap(2.0).min_h(0.0).scroll(rune::ScrollSpec { stick_bottom: false });
    let mut last_group: Option<Option<String>> = None;
    for (i, ch) in shown.iter().enumerate() {
        if !ch.all() && last_group.as_ref() != Some(&ch.group) {
            rows = rows.child(heading(ch));
            last_group = Some(ch.group.clone());
        }
        let prev = if i == 0 { cx.filter.as_ref().map(|_| filter_key.clone()) } else { Some(keys[i - 1].clone()) };
        let next = keys.get(i + 1).cloned();
        let focus_me = cx.filter.is_none() && (ch.id == cx.value || (i == 0 && !shown.iter().any(|c| c.id == cx.value)));
        rows = rows.child(choice_row(&cx, ch, &keys[i], prev, next, focus_me));
    }
    if shown.iter().all(|c| c.all()) && !q.trim().is_empty() {
        rows = rows.child(el().w_full().px(8.0).py(6.0).child(text(format!("No sandbox matches \u{201c}{}\u{201d}", q.trim())).font(crate::screen::body(12.0, 400.0)).color(c("text-muted")).truncate().min_w(0.0)));
    }
    l.child(rows)
}

/// A group's heading in the list: its tint and label colour, as the sidebar heads its block.
fn heading(ch: &Choice) -> El {
    let name = ch.group.clone().unwrap_or_else(|| "No group".into());
    let (tint, label) = match ch.color {
        Some(g) => {
            let (t, _, l) = theme::group_tints(g);
            (t, l)
        }
        None => (theme::color_value("surface").unwrap(), theme::color_value("text-muted").unwrap()),
    };
    let sq = el().w(8.0).h(8.0).radius(2.0);
    let sq = match ch.color {
        Some(g) => sq.fill(hex(g)),
        None => sq.border(1.0, c("text-dim")),
    };
    row().w_full().h(HEAD_H).px(8.0).gap(6.0).items_center().radius(theme::radius("xs").unwrap()).fill(hex(tint)).help(name.clone())
        .child(sq)
        .child(text(name).font(crate::screen::body(12.0, 600.0)).color(hex(label)).truncate().min_w(0.0))
}

fn choice_row<S: 'static>(cx: &ListCtx<S>, ch: &Choice, key: &str, prev: Option<String>, next: Option<String>, focus_me: bool) -> El {
    let current = ch.id == cx.value;
    let surface = theme::color_value("surface").unwrap();
    let muted = theme::color_value("text-muted").unwrap();
    let mut r = row().key(key.to_string()).w_full().h(ROW_H).px(8.0).gap(8.0).items_center().radius(theme::radius("xs").unwrap()).role("menuitem").focusable(true);
    if ch.all() {
        r = r.label(cx.all_label.clone()).help("Every sandbox: no filter")
            .child(icon("grid", theme::icon::MEDIUM, muted))
            .child(text(cx.all_label.clone()).font(crate::screen::body(13.0, if current { 600.0 } else { 500.0 })).color(c("text-bright")).grow(1.0).min_w(0.0).truncate())
            .child(text("no filter").font(crate::screen::body(12.0, 400.0)).color(c("text-muted")));
    } else {
        let bar = el().w(3.0).h(18.0).radius(2.0);
        let bar = match ch.color {
            Some(g) => bar.fill(hex(g)),
            None => bar.fill(c("elevated")),
        };
        let mut tip = format!("{} ({})", ch.id, if ch.status.is_empty() { "unknown" } else { &ch.status });
        if !ch.agent.is_empty() {
            tip.push_str(&format!(", {}", ch.agent));
        }
        r = r.label(format!("{}, {}", ch.id, if ch.status.is_empty() { "unknown" } else { &ch.status })).help(tip)
            .child(bar)
            .child(status_mark(&ch.status, surface))
            .child(text(ch.id.clone()).font(crate::screen::body(13.0, if current { 600.0 } else { 500.0 })).color(c("text-bright")).grow(1.0).min_w(0.0).truncate())
            .children((ch.sessions > 0).then(|| {
                el().px(6.0).py(1.0).radius(theme::radius("xs").unwrap()).fill(c("elevated")).help(format!("{} session{}", ch.sessions, if ch.sessions == 1 { "" } else { "s" }))
                    .child(text(ch.sessions.to_string()).font(crate::screen::body(12.0, 500.0)).color(c("text")))
            }))
            .child(el().w(64.0).child(text(ch.status.clone()).font(crate::screen::body(12.0, 400.0)).color(c("text-muted")).truncate().min_w(0.0)))
            .child(el().w(56.0).child(text(ch.agent.clone()).font(crate::screen::body(12.0, 400.0)).color(c("text-dim")).truncate().min_w(0.0)));
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
    r.hover(c("elevated")).focus_highlight(c("elevated")).focus_ring(2.0, -2.0, c("focus")).cursor(Cursor::Pointer)
        .on_press(move |s: &mut S| pick(s, &id))
        .on_key(move |s: &mut S, ev: &rune::KeyEvent, tx: &mut rune::Tx| {
            if tx.focused() != Some(me.as_str()) || ev.mods.cmd || ev.mods.ctrl || ev.mods.alt || ev.mods.logo {
                return false;
            }
            match ev.key {
                rune::Key::Up | rune::Key::Down => {
                    if let Some(k) = if ev.key == rune::Key::Up { &prev } else { &next } {
                        tx.focus(k.clone(), true);
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
