//! What fits in a bar: the pane bars' extras and the top bar's.
//!
//! A bar keeps its fixed parts (a pane's state dot, name, session kind and
//! close button; the top bar's layout switch, Jarvis and the daemon state),
//! and the features' extras take the room left, in priority order: what needs
//! the person first (an amber tone: sbx holding something, a session that
//! wants you, a plan waiting), then what carries a number or is ready to act
//! on (a count, a green tone), then the rest, each group in its own order.
//! What does not fit goes into one overflow menu ("…" with how many), which
//! lists them with their label, icon, tone and count and runs the same
//! intents. Widths are measured with Rune's own text measure, so the fit is
//! the one the runtime lays out.

use crate::feat::Extra;
use rune::TextStyle;
use serde_json::Value as J;

/// How urgent an extra is: 0 needs the person, 1 has a number or is ready, 2 the rest.
pub fn rank(tone: &str, count: Option<u64>) -> u8 {
    match tone {
        "amber" => 0,
        _ if count.is_some() || tone == "green" => 1,
        _ => 2,
    }
}

/// The colour an extra means when it is quiet, by its id: what it goes to
/// (network, requests, threads, the project, the browser) blue, the team
/// (a lead, Jarvis) purple, an interrupt red, anything else neutral.
pub fn meaning(id: &str) -> &'static str {
    match id {
        "approvals" | "relay" | "threads" | "project" | "browser" | "network" | "plan" | "codemap" | "remote" | "envfile" => "blue",
        "lead" | "jarvis" | "team" => "purple",
        "interrupt" => "red",
        _ => "neutral",
    }
}

/// The zone of a pane bar an extra sits in, so the eye finds a zone before
/// a button: 0 the session's state and what waits (sbx asks, questions, a
/// plan, the model), 1 things it opens (Browser, Project, Env), 2 the team
/// (Team, Threads), 3 controlling the session (Esc, Remote, closing it).
/// Zones are drawn in this order, a hairline between two.
pub fn zone(id: &str) -> u8 {
    match id {
        "attention" | "status" | "approvals" | "relay" | "state" | "model" | "merge" | "plan" => 0,
        "lead" | "threads" => 2,
        "interrupt" | "remote" | "close" => 3,
        _ => 1,
    }
}

/// The hairline between two zones: drawn in the gap before a zone's first
/// chip, so it takes no room of its own.
pub const SEP_W: f32 = 1.0;

/// The panel an extra opens, when it opens one (its chip is drawn active
/// while that panel shows for this pane).
pub fn panel_of(id: &str) -> Option<&'static str> {
    match id {
        "browser" => Some("browser"),
        "project" => Some("files"),
        "codemap" => Some("codemap"),
        "threads" => Some("threads"),
        "relay" => Some("relay"),
        "approvals" => Some("network"),
        _ => None,
    }
}

/// `items` in the order the bar draws them: by zone, stable.
pub fn by_zone(mut items: Vec<Item>) -> Vec<Item> {
    items.sort_by_key(|i| zone(&i.id));
    items
}

/// How wide `items` are drawn in a bar, each after a `gap`.
pub fn chips_w(items: &[Item], gap: f32) -> f32 {
    items.iter().map(|i| gap + chip_w(i)).sum::<f32>()
}

/// The tone an item's menu row takes: its own when it has one, else the
/// colour its chip's icon has by meaning, so it looks the same inline and in
/// the overflow menu.
pub fn menu_tone(it: &Item) -> &str {
    match it.tone.as_str() {
        "" | "muted" => meaning(&it.id),
        t => t,
    }
}

/// One thing a bar may show inline or in its overflow menu.
#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    /// The key's suffix: an extra's id, or the core's `attention` / `status`.
    pub id: String,
    pub label: String,
    pub icon: Option<String>,
    pub tone: String,
    pub help: String,
    pub count: Option<u64>,
    pub intent: Option<(String, J)>,
    /// The core's own (attention, status): keyed `pane-<n>-<id>`, not `pane-<n>-x-<id>`.
    pub core: bool,
}

impl Item {
    pub fn from_extra(e: &Extra) -> Item {
        Item { id: e.id.clone(), label: e.label.clone(), icon: e.icon.clone(), tone: e.tone.clone(), help: e.help.clone(), count: e.count, intent: e.intent.clone(), core: false }
    }

    pub fn rank(&self) -> u8 {
        rank(&self.tone, self.count)
    }

    /// The text shown: the label with its count.
    pub fn text(&self) -> String {
        match self.count {
            Some(n) if self.label.is_empty() => n.to_string(),
            Some(n) => format!("{} {n}", self.label),
            None => self.label.clone(),
        }
    }

    /// Its element key in pane `n`'s bar (the same inline and in the menu).
    pub fn key(&self, n: usize) -> String {
        if self.core { format!("pane-{n}-{}", self.id) } else { format!("pane-{n}-x-{}", self.id) }
    }
}

/// Extras that always sit in the "⋯" menu, never inline: a view setting
/// of the pane (keyword pills), not something to act on at a glance.
pub const MENU_ONLY: &[&str] = &["pills"];

/// Sorted by [`rank`], stable: the order the bar fills in.
pub fn by_priority(mut items: Vec<Item>) -> Vec<Item> {
    items.sort_by_key(Item::rank);
    items
}

pub fn measure(t: &str, size: f32, weight: f32) -> f32 {
    if t.is_empty() {
        return 0.0;
    }
    rune::text::measure(t, &[], TextStyle::new(size, weight), None).0
}

/// A pane bar chip (shell.rs, `bar_chip`): 24 pt high, px 8, icon 14, gap 6, text 12/500.
pub const CHIP_PX: f32 = 8.0;
pub const CHIP_GAP: f32 = 6.0;
pub const CHIP_ICON: f32 = 14.0;
pub const CHIP_TEXT: f32 = 12.0;
/// The pane's name in its bar.
pub const NAME_TEXT: f32 = 13.0;
/// The overflow button's gap and tone dot.
pub const MORE_GAP: f32 = 4.0;
pub const MORE_DOT: f32 = 6.0;

/// A pane bar chip's width, as laid out (at least 24 pt square).
pub fn chip_w(it: &Item) -> f32 {
    let t = it.text();
    let icon = it.icon.is_some() as u8 as f32 * CHIP_ICON;
    let gap = if it.icon.is_some() && !t.is_empty() { CHIP_GAP } else { 0.0 };
    (CHIP_PX * 2.0 + icon + gap + measure(&t, CHIP_TEXT, 500.0)).max(24.0)
}

/// How many of `widths` (in priority order) show inline in `avail` points,
/// after `fixed`, each preceded by `gap`, keeping room for the overflow
/// button (`more`, plus its gap) whenever one is left out.
#[cfg(test)]
pub fn fit(avail: f32, fixed: f32, widths: &[f32], gap: f32, more: f32) -> usize {
    let n = widths.len();
    let mut used = fixed;
    for (k, w) in widths.iter().enumerate() {
        let tail = if k + 1 < n { gap + more } else { 0.0 };
        if used + gap + w + tail > avail + 0.5 {
            return k;
        }
        used += gap + w;
    }
    n
}

/// A pane bar's parts, measured.
pub struct PaneFit {
    /// The mode switch shows inline (else it is the menu's "Switch to ...",
    /// a core item `mode` in `hidden`: one of the last things to go).
    pub switch: bool,
    /// Shown inline, in priority order.
    pub shown: Vec<Item>,
    /// In the overflow menu, in priority order.
    pub hidden: Vec<Item>,
    /// The name's least width (points): it never goes under it.
    pub name_min: f32,
}

impl PaneFit {
    /// How many extras did not fit (the menu-only ones are not counted:
    /// they live in the menu whatever the width).
    pub fn overflowed(&self) -> usize {
        self.hidden.iter().filter(|i| !MENU_ONLY.contains(&i.id.as_str())).count()
    }
}

/// The pane bar's paddings and fixed parts (see `shell::pane_view`).
pub const PANE_PAD: f32 = 8.0;
pub const PANE_GAP: f32 = 8.0;
pub const DOT_W: f32 = 8.0;
/// The grip a pane is dragged by (its bar's empty part drags it too).
pub const GRIP_W: f32 = 10.0;
pub const CLOSE_W: f32 = 24.0;
/// Room the name keeps before any extra: up to this much of it shows.
pub const NAME_WANT: f32 = 120.0;
/// The name's floor: about eight characters, then an ellipsis (its tooltip has it whole).
pub const NAME_MIN: f32 = 56.0;

/// The overflow button: the `more` icon, the count, maybe a dot.
pub fn more_w(hidden: usize) -> f32 {
    CHIP_PX * 2.0 + CHIP_ICON + MORE_GAP + measure(&hidden.max(1).to_string(), CHIP_TEXT, 500.0) + MORE_GAP + MORE_DOT
}

/// The mode switch's segments: px, icon, gap; the switch's padding and gap.
pub const SEG_PX: f32 = 6.0;
pub const SEG_ICON: f32 = 12.0;
pub const SEG_GAP: f32 = 4.0;
pub const SWITCH_PAD: f32 = 1.0;
pub const SWITCH_GAP: f32 = 2.0;
/// The shell's segment.
pub const BASH: &str = "Bash";

/// One segment's width (its icon and word).
pub fn seg_w(label: &str) -> f32 {
    SEG_PX * 2.0 + SEG_ICON + SEG_GAP + measure(label, CHIP_TEXT, 500.0)
}

/// The mode switch right after the name: the agent's segment (`agent`:
/// "Claude", "Codex", "Thread") and "Bash".
pub fn switch_w(agent: &str) -> f32 {
    SWITCH_PAD * 2.0 + seg_w(agent) + SWITCH_GAP + seg_w(BASH)
}

/// What shows in a pane bar `width` points wide. `agent`: the mode
/// switch's agent segment ("Claude", "Thread"...), `None` for a pane with no
/// switch (a kit's prompt: its kind word instead). `menu`: the "⋯" button is
/// always there (it also holds the pane's moves), so its room is kept.
pub fn pane_fit(width: f32, name: &str, agent: Option<&str>, items: Vec<Item>, menu: bool) -> PaneFit {
    let (only_menu, items): (Vec<Item>, Vec<Item>) = items.into_iter().partition(|i| menu && MENU_ONLY.contains(&i.id.as_str()));
    let items = by_priority(items);
    let name_w = measure(name, NAME_TEXT, 500.0);
    let name_min = name_w.min(NAME_MIN);
    let name_keep = name_w.min(NAME_WANT);
    // grip, dot | name, switch, extras…, more | close
    let base = PANE_PAD * 2.0 + GRIP_W + PANE_GAP + DOT_W + PANE_GAP + name_keep + PANE_GAP + CLOSE_W + if menu { PANE_GAP + more_w(items.len()) } else { 0.0 };
    let sw = agent.map_or(0.0, |a| switch_w(a) + PANE_GAP);
    // The switch goes into the menu only when even it alone does not fit.
    let switch = agent.is_some() && base + sw <= width + 0.5;
    let fixed = base + if switch { sw } else { 0.0 };
    // The most items, in priority order, whose chips (drawn by zone) fit
    // after the fixed parts.
    let more = if menu { 0.0 } else { more_w(items.len()) };
    let n = items.len();
    let k = if agent.is_some() && !switch { 0 } else { (0..=n).rev().find(|&k| fixed + chips_w(&items[..k], PANE_GAP) + if k < n { PANE_GAP + more } else { 0.0 } <= width + 0.5).unwrap_or(0) };
    let mut shown = items;
    let mut hidden = shown.split_off(k);
    hidden.extend(only_menu);
    PaneFit { switch, shown: by_zone(shown), hidden, name_min }
}

/// The overflow button's mark for what it hides: the most urgent tone among
/// them that asks for the eye (amber, then any with a count), or none.
pub fn more_tone(hidden: &[Item]) -> Option<&'static str> {
    if hidden.iter().any(|i| i.tone == "amber") {
        Some("amber")
    } else if hidden.iter().any(|i| i.tone == "red" && i.count.is_some()) {
        Some("red")
    } else if hidden.iter().any(|i| i.count.is_some()) {
        Some("blue")
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// The top bar (screens/topbar.luau draws it; the core says what fits).

/// Under this width the top bar goes compact: no sbxw mark.
pub const TOP_COMPACT: f32 = 720.0;
/// Between the bar's groups (left, centre, right, and the spacers between them).
pub const TOP_GAP: f32 = 16.0;
/// Inside the centre group and the right one.
pub const TOP_CENTRE_GAP: f32 = 8.0;
pub const TOP_RIGHT_GAP: f32 = 4.0;

/// A built-in `button`'s width, as screen.rs lays it out: 28 pt square at
/// least; a label pads 10 each side, an icon is 16 with a gap of 6, the
/// label 13/500.
pub fn button_w(label: &str, icon: bool) -> f32 {
    if label.is_empty() {
        return 28.0;
    }
    (20.0 + if icon { 16.0 + 6.0 } else { 0.0 } + measure(label, 13.0, 500.0)).max(28.0)
}

/// A built-in `badge`: px 6 each side, text 12/500.
pub fn badge_w(label: &str) -> f32 {
    12.0 + measure(label, 12.0, 500.0)
}

/// A built-in `menu`/`select` trigger: px 8, icon 16, gaps 6, label 13/400,
/// a 12 pt chevron (none on a ghost menu with no label), 28 pt at least.
pub fn menu_w(label: &str, icon: bool, chevron: bool) -> f32 {
    let mut w = 16.0;
    let mut parts = 0;
    if icon {
        w += 16.0;
        parts += 1;
    }
    if !label.is_empty() {
        w += measure(label, 13.0, 400.0);
        parts += 1;
    }
    if chevron {
        w += 12.0;
        parts += 1;
    }
    (w + 6.0 * (parts.max(1) - 1) as f32).max(28.0)
}

/// The daemon's state at the bar's right end: px 8, dot 8, gap 8, a word 12/400.
pub fn state_w(word: &str) -> f32 {
    16.0 + 8.0 + 8.0 + measure(word, 12.0, 400.0)
}

/// One top bar extra, as the core sees it before the screen draws it.
#[derive(Clone, Debug)]
pub struct TopExtra {
    /// The label with its count.
    pub label: String,
    pub icon: bool,
    /// Drawn as a button (it has an intent), else as a badge.
    pub button: bool,
    /// In the right group (usage, Setup, Help, Mac), else the centre.
    pub tool: bool,
    /// Drawn some other way, this wide (the usage chip with its bars).
    pub drawn_w: Option<f32>,
}

impl TopExtra {
    pub fn width(&self) -> f32 {
        if let Some(w) = self.drawn_w {
            return w;
        }
        if self.button { button_w(&self.label, self.icon) } else { badge_w(&self.label) }
    }
    fn gap(&self) -> f32 {
        if self.tool { TOP_RIGHT_GAP } else { TOP_CENTRE_GAP }
    }
}

/// How the top bar is drawn at `width` (the screen's own column), as
/// topbar.luau lays it out: `[sbxw] · [centre extras] · [tools, …,
/// Jarvis, state]` (`jarvis`: its button's label, when the bar draws it).
/// Compact or not, and how many of `extras` (in priority order) show
/// inline; the rest go into its menu. Exact: every part is measured the way
/// the element draws it (tested against the laid out bar).
pub fn top_fit(width: f32, jarvis: Option<&str>, state_word: &str, extras: &[TopExtra]) -> (bool, usize) {
    // The sbxw mark (topbar.luau, `app-mark`): 22 pt square.
    let left = |compact: bool| if compact { 0.0 } else { 22.0 };
    // left, spacer, centre, spacer, right: four gaps; inside the right group
    // Jarvis and the state, and the gap before Jarvis when a tool precedes it.
    let fixed = |compact: bool| left(compact) + TOP_GAP * 4.0 + jarvis.map_or(0.0, |j| button_w(j, true) + TOP_RIGHT_GAP) + state_w(state_word);
    let need = |compact: bool, k: usize| -> f32 {
        let shown = &extras[..k];
        let mut w = fixed(compact);
        let (c, t): (Vec<&TopExtra>, Vec<&TopExtra>) = shown.iter().partition(|e| !e.tool);
        w += c.iter().map(|e| e.width()).sum::<f32>() + TOP_CENTRE_GAP * c.len().saturating_sub(1) as f32;
        w += t.iter().map(|e| e.width() + e.gap()).sum::<f32>();
        if k < extras.len() {
            // The "…" menu (its count, a dot) in the right group.
            w += menu_w(&(extras.len() - k).to_string(), true, true) + TOP_RIGHT_GAP + 4.0 + 8.0 + TOP_RIGHT_GAP;
        }
        w
    };
    let compact = width < TOP_COMPACT || need(false, extras.len()) > width;
    let k = (0..=extras.len()).rev().find(|&k| need(compact, k) <= width + 0.5).unwrap_or(0);
    (compact, k)
}

// ---------------------------------------------------------------------------
// The usage chip (topbar.luau `usage_chip`): each window's label, a small
// bar filled to its share, its number.

/// A usage bar: 40 by 4 pt.
pub const USAGE_BAR_W: f32 = 40.0;
/// Inside a window (label, bar, number) and between the two windows.
pub const USAGE_GAP: f32 = 4.0;
pub const USAGE_WINDOW_GAP: f32 = 12.0;

/// The bars' colour by meaning: blue under 70%, amber from 70%, red from 90%
/// (topbar.luau `usage_tone` draws it; this is the rule its test holds).
#[cfg(test)]
pub fn usage_tone(pct: f64) -> &'static str {
    if pct >= 90.0 {
        "red"
    } else if pct >= 70.0 {
        "amber"
    } else {
        "blue"
    }
}

/// The windows the daemon reports: label and percentage.
pub fn usage_windows(u: &J) -> Vec<(&'static str, f64)> {
    [("5h", &u["five_hour_pct"]), ("7d", &u["seven_day_pct"])].into_iter().filter_map(|(l, v)| v.as_f64().map(|p| (l, p.clamp(0.0, 100.0)))).collect()
}

/// The chip with its bars, as wide as topbar.luau draws it (8 pt sides,
/// 4 pt gaps, 12 between the windows, 12/500 text).
pub fn usage_bars_w(u: &J) -> Option<f32> {
    let w = usage_windows(u);
    if w.is_empty() {
        return None;
    }
    let parts: f32 = w.iter().map(|(l, p)| measure(l, 12.0, 500.0) + USAGE_GAP + USAGE_BAR_W + USAGE_GAP + measure(&format!("{}%", p.round()), 12.0, 500.0)).sum();
    Some(16.0 + parts + USAGE_WINDOW_GAP * (w.len() - 1) as f32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::feat::Place;

    fn ex(id: &str, label: &str, tone: &str, count: Option<u64>) -> Item {
        Item::from_extra(&Extra { place: Place::Pane("s".into()), id: id.into(), label: label.into(), icon: Some("flag".into()), tone: tone.into(), help: String::new(), count, intent: None })
    }

    /// What the user's screenshot had on one pane bar of test-sbx.
    fn many() -> Vec<Item> {
        vec![
            ex("browser", "Browser", "muted", None),
            ex("env", "Env", "muted", None),
            ex("project", "Project", "muted", None),
            ex("approvals", "sbx asks", "muted", Some(2)),
            ex("lead", "Team", "muted", None),
            ex("interrupt", "Esc", "muted", None),
            ex("remote", "Remote", "muted", None),
            ex("plan", "Review plan", "amber", None),
            ex("threads", "Threads", "muted", None),
        ]
    }

    fn ids(v: &[Item]) -> Vec<&str> {
        v.iter().map(|i| i.id.as_str()).collect()
    }

    /// A chip's colour by meaning is carried into its overflow menu row.
    #[test]
    fn a_chip_keeps_its_colour_in_the_overflow_menu() {
        let tones: Vec<(String, String)> = many().iter().map(|i| (i.id.clone(), menu_tone(i).to_string())).collect();
        let get = |id: &str| tones.iter().find(|(i, _)| *i == id).unwrap().1.clone();
        assert_eq!((get("interrupt"), get("remote"), get("threads"), get("lead"), get("browser"), get("project")), ("red".into(), "blue".into(), "blue".into(), "purple".into(), "blue".into(), "blue".into()));
        assert_eq!(get("plan"), "amber", "a toned extra keeps its own tone");
        assert_eq!(get("env"), "neutral");
    }

    #[test]
    fn what_needs_the_person_goes_first() {
        let order = by_priority(many());
        assert_eq!(ids(&order), ["plan", "approvals", "browser", "env", "project", "lead", "interrupt", "remote", "threads"]);
        assert_eq!(rank("red", None), 2, "red is danger (Close thread), not attention");
        assert_eq!(rank("green", None), 1);
    }

    #[test]
    fn fit_keeps_room_for_the_overflow_button() {
        // 3 items of 10, each after a gap of 1, the button 5 (after its gap), nothing fixed.
        assert_eq!(fit(33.0, 0.0, &[10.0, 10.0, 10.0], 1.0, 5.0), 3, "all fit, no button needed");
        assert_eq!(fit(32.0, 0.0, &[10.0, 10.0, 10.0], 1.0, 5.0), 2, "two, then the button");
        assert_eq!(fit(28.0, 0.0, &[10.0, 10.0, 10.0], 1.0, 5.0), 2);
        assert_eq!(fit(27.0, 0.0, &[10.0, 10.0, 10.0], 1.0, 5.0), 1);
        assert_eq!(fit(17.0, 0.0, &[10.0, 10.0, 10.0], 1.0, 5.0), 1);
        assert_eq!(fit(16.0, 0.0, &[10.0, 10.0, 10.0], 1.0, 5.0), 0);
        assert_eq!(fit(100.0, 0.0, &[], 1.0, 5.0), 0);
    }

    /// The bar never holds more than its width, at every width the layouts
    /// give (1 pane at 1400 down to 4 panes at the 720 x 480 window), and
    /// everything is either inline or in the menu, once.
    #[test]
    fn pane_bars_fit_at_every_width() {
        for width in [1100.0, 700.0, 560.0, 450.0, 330.0, 230.0, 200.0, 160.0] {
            for n in [0, 1, 3, 9] {
                let items: Vec<Item> = many().into_iter().take(n).collect();
                let f = pane_fit(width, "test-sbx", Some("Claude"), items.clone(), true);
                assert_eq!(f.shown.len() + f.hidden.len(), n, "{width} {n}");
                let name = measure("test-sbx", 12.0, 500.0).min(NAME_WANT);
                // The "⋯" menu is always there (it holds the pane's moves).
                let sw = if f.switch { switch_w("Claude") + PANE_GAP } else { 0.0 };
                let mut used = PANE_PAD * 2.0 + GRIP_W + PANE_GAP + DOT_W + PANE_GAP + name + sw + PANE_GAP + CLOSE_W + PANE_GAP + more_w(n);
                used += chips_w(&f.shown, PANE_GAP);
                let floor = PANE_PAD * 2.0 + GRIP_W + PANE_GAP + DOT_W + PANE_GAP + name + sw + PANE_GAP + CLOSE_W + PANE_GAP + more_w(n);
                // The switch is one of the last to go: shown whenever extras show.
                assert!(f.switch || f.shown.is_empty(), "{width} {n}");
                assert!(used <= width + 0.5 || f.shown.is_empty() && used <= floor + 0.5, "{width} {n}: {used}");
                // Priority: nothing hidden ranks before something shown.
                if let (Some(s), Some(h)) = (f.shown.iter().map(Item::rank).max(), f.hidden.first()) {
                    assert!(s <= h.rank(), "{width} {n}");
                }
                // Drawn by zone.
                assert!(f.shown.windows(2).all(|w| zone(&w[0].id) <= zone(&w[1].id)), "{width} {n}");
            }
        }
    }

    #[test]
    fn widths_decide_what_overflows() {
        let wide = pane_fit(1100.0, "test-sbx", Some("Claude"), many(), true);
        assert!(wide.switch && wide.hidden.len() <= 1, "one pane at 1400: the switch and all but one inline: {:?}", ids(&wide.hidden));
        let two = pane_fit(560.0, "test-sbx", Some("Claude"), many(), true);
        assert_eq!(ids(&two.shown)[..2], ["plan", "approvals"], "two panes at 1400: what needs you stays inline");
        assert!(!two.hidden.is_empty());
        let narrow = pane_fit(210.0, "test-sbx", Some("Claude"), many(), true);
        assert!(narrow.shown.len() <= 1, "4 panes at 720: {:?}", ids(&narrow.shown));
        assert_eq!(narrow.hidden.len() + narrow.shown.len(), 9);
        // The name keeps its floor even with nothing else drawn.
        let long = pane_fit(210.0, "a-very-long-sandbox-name-for-tests", Some("Claude"), many(), true);
        assert_eq!(long.name_min, NAME_MIN);
    }

    #[test]
    fn the_overflow_button_shows_what_it_hides() {
        assert_eq!(more_tone(&[ex("a", "A", "muted", None)]), None);
        assert_eq!(more_tone(&[ex("a", "A", "muted", Some(2))]), Some("blue"));
        assert_eq!(more_tone(&[ex("a", "A", "muted", Some(2)), ex("p", "P", "amber", None)]), Some("amber"));
    }

    #[test]
    fn usage_bars_take_their_colour_from_the_share() {
        assert_eq!((usage_tone(11.0), usage_tone(69.9), usage_tone(70.0), usage_tone(89.0), usage_tone(90.0), usage_tone(100.0)), ("blue", "blue", "amber", "amber", "red", "red"));
        let u = serde_json::json!({ "five_hour_pct": 11.0, "seven_day_pct": 78.0 });
        assert_eq!(usage_windows(&u), [("5h", 11.0), ("7d", 78.0)]);
        assert!(usage_bars_w(&u).unwrap() > badge_w("5h 11% · 7d 78%"), "the bars take more room than the words");
        assert_eq!(usage_bars_w(&serde_json::json!({})), None);
    }

    #[test]
    fn the_top_bar_goes_compact_then_overflows() {
        let x = |l: &str, tool: bool| TopExtra { label: l.into(), icon: true, button: true, tool, drawn_w: None };
        let extras = vec![x("New", false), x("sbx asks 88", false), x("Plan waiting", false), x("", true), x("", true), x("Mac", true), x("Requests 3", false)];
        let (c, k) = top_fit(1300.0, Some("Jarvis"), "connected", &extras);
        assert!(!c && k == extras.len(), "wide: all inline ({c}, {k})");
        let (c, k) = top_fit(700.0, Some("Jarvis"), "connected", &extras);
        assert!(c && k < extras.len(), "narrow: compact and an overflow ({c}, {k})");
        let (c, k) = top_fit(330.0, Some("Jarvis · 2 to review"), "connected", &extras);
        assert!(c && k == 0, "the smallest window with a notice: everything in the menu ({c}, {k})");
    }
}
