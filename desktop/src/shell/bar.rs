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

/// A pane bar chip's width (`shell::features`' chip: px 5, border 1, icon 11, gap 4, text 10.5/500).
pub fn chip_w(it: &Item) -> f32 {
    let t = it.text();
    let icon = it.icon.is_some() as u8 as f32 * 11.0;
    let gap = if it.icon.is_some() && !t.is_empty() { 4.0 } else { 0.0 };
    12.0 + icon + gap + measure(&t, 10.5, 500.0)
}

/// How many of `widths` (in priority order) show inline in `avail` points,
/// after `fixed`, each preceded by `gap`, keeping room for the overflow
/// button (`more`, plus its gap) whenever one is left out.
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
    /// Shown inline, in priority order.
    pub shown: Vec<Item>,
    /// In the overflow menu, in priority order.
    pub hidden: Vec<Item>,
    /// The name's least width (points): it never goes under it.
    pub name_min: f32,
}

/// The pane bar's paddings and fixed parts (see `shell::pane_view`).
pub const PANE_PAD: f32 = 10.0;
pub const PANE_GAP: f32 = 6.0;
pub const DOT_W: f32 = 7.0;
pub const CLOSE_W: f32 = 18.0;
/// Room the name keeps before any extra: up to this much of it shows.
pub const NAME_WANT: f32 = 120.0;
/// The name's floor: about eight characters, then an ellipsis (its tooltip has it whole).
pub const NAME_MIN: f32 = 56.0;

/// The overflow button: the `more` icon, the count, maybe a dot.
pub fn more_w(hidden: usize) -> f32 {
    12.0 + 11.0 + 4.0 + measure(&hidden.max(1).to_string(), 10.5, 500.0) + 4.0 + 6.0
}

/// The kind badge (`agent`, `bash`): px 5, text 10.5/400.
pub fn kind_w(kind: &str) -> f32 {
    10.0 + measure(kind, 10.5, 400.0)
}

/// What shows in a pane bar `width` points wide.
pub fn pane_fit(width: f32, name: &str, kind: &str, items: Vec<Item>) -> PaneFit {
    let items = by_priority(items);
    let name_w = measure(name, 12.0, 500.0);
    let name_min = name_w.min(NAME_MIN);
    let name_keep = name_w.min(NAME_WANT);
    // dot | name, kind, extras…, more | close
    let fixed = PANE_PAD * 2.0 + DOT_W + PANE_GAP + name_keep + PANE_GAP + kind_w(kind) + PANE_GAP + CLOSE_W;
    let widths: Vec<f32> = items.iter().map(chip_w).collect();
    let k = fit(width, fixed, &widths, PANE_GAP, more_w(items.len()));
    let mut shown = items;
    let hidden = shown.split_off(k);
    PaneFit { shown, hidden, name_min }
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

/// Under this width the top bar's own parts go compact: no "sbxw" title, the
/// layout switch as an icon menu.
pub const TOP_COMPACT: f32 = 720.0;
pub const TOP_GAP: f32 = 14.0;
pub const TOP_EXTRA_GAP: f32 = 8.0;
/// The daemon state keeps at least its dot and a word.
pub const TOP_STATE_MIN: f32 = 96.0;

/// A built-in `button`'s width (screen.rs: px 7 or 4, border 1, icon 12, gap 4, text 11/400).
pub fn button_w(label: &str, icon: bool) -> f32 {
    let t = measure(label, 11.0, 400.0);
    let px = if label.is_empty() { 4.0 } else { 7.0 };
    px * 2.0 + 2.0 + if icon { 12.0 } else { 0.0 } + if icon && !label.is_empty() { 4.0 } else { 0.0 } + t
}

/// A built-in `menu`/`select` trigger (px 8, border 1, icon 12, gaps 6, text 12/400, chevron 10).
pub fn menu_w(label: &str, icon: bool) -> f32 {
    let t = measure(label, 12.0, 400.0);
    let mut w = 16.0 + 2.0 + 10.0;
    if icon {
        w += 12.0 + 6.0;
    }
    if !label.is_empty() {
        w += t + 6.0;
    }
    w
}

/// How the top bar is drawn at `width` (the screen's own column): compact
/// or not, and how many of `extras` (each `(label with count, has icon)`,
/// in priority order) show inline; the rest go into its menu.
pub fn top_fit(width: f32, jarvis_label: &str, layout_label: &str, extras: &[(String, bool)]) -> (bool, usize) {
    let full = measure("sbxw", 14.0, 700.0) + TOP_GAP + menu_w(layout_label, true);
    let compact_parts = menu_w("", true);
    let own = |compact: bool| {
        // [title,] layout, spacer, extras row, jarvis, state: four gaps after the layout
        let parts = if compact { compact_parts } else { full };
        parts + TOP_GAP * 4.0 + button_w(jarvis_label, true) + TOP_STATE_MIN
    };
    let widths: Vec<f32> = extras.iter().map(|(l, i)| button_w(l, *i)).collect();
    // The menu ("…" and how many) and the dot of what it hides.
    let more = menu_w("9", true) + TOP_EXTRA_GAP + 8.0;
    let compact = width < TOP_COMPACT || own(false) + widths.iter().sum::<f32>() + TOP_EXTRA_GAP * widths.len() as f32 > width;
    // The extras row's own gaps: the first extra has none before it.
    let k = fit(width + TOP_EXTRA_GAP, own(compact), &widths, TOP_EXTRA_GAP, more);
    (compact, k)
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
                let f = pane_fit(width, "test-sbx", "agent", items.clone());
                assert_eq!(f.shown.len() + f.hidden.len(), n, "{width} {n}");
                let name = measure("test-sbx", 12.0, 500.0).min(NAME_WANT);
                let mut used = PANE_PAD * 2.0 + DOT_W + PANE_GAP + name + PANE_GAP + kind_w("agent") + PANE_GAP + CLOSE_W;
                used += f.shown.iter().map(|i| PANE_GAP + chip_w(i)).sum::<f32>();
                if !f.hidden.is_empty() {
                    used += PANE_GAP + more_w(n);
                }
                let floor = PANE_PAD * 2.0 + DOT_W + PANE_GAP + name + PANE_GAP + kind_w("agent") + PANE_GAP + CLOSE_W + PANE_GAP + more_w(n);
                assert!(used <= width + 0.5 || f.shown.is_empty() && used <= floor + 0.5, "{width} {n}: {used}");
                // Priority: nothing hidden ranks before something shown.
                if let (Some(s), Some(h)) = (f.shown.last(), f.hidden.first()) {
                    assert!(s.rank() <= h.rank(), "{width} {n}");
                }
            }
        }
    }

    #[test]
    fn widths_decide_what_overflows() {
        let wide = pane_fit(1100.0, "test-sbx", "agent", many());
        assert!(wide.hidden.is_empty(), "one pane at 1400: all inline: {:?}", ids(&wide.hidden));
        let two = pane_fit(560.0, "test-sbx", "agent", many());
        assert_eq!(ids(&two.shown)[..2], ["plan", "approvals"], "two panes at 1400: what needs you stays inline");
        assert!(!two.hidden.is_empty());
        let narrow = pane_fit(210.0, "test-sbx", "agent", many());
        assert!(narrow.shown.len() <= 1, "4 panes at 720: {:?}", ids(&narrow.shown));
        assert_eq!(narrow.hidden.len() + narrow.shown.len(), 9);
        // The name keeps its floor even with nothing else drawn.
        let long = pane_fit(210.0, "a-very-long-sandbox-name-for-tests", "agent", many());
        assert_eq!(long.name_min, NAME_MIN);
    }

    #[test]
    fn the_overflow_button_shows_what_it_hides() {
        assert_eq!(more_tone(&[ex("a", "A", "muted", None)]), None);
        assert_eq!(more_tone(&[ex("a", "A", "muted", Some(2))]), Some("blue"));
        assert_eq!(more_tone(&[ex("a", "A", "muted", Some(2)), ex("p", "P", "amber", None)]), Some("amber"));
    }

    #[test]
    fn the_top_bar_goes_compact_then_overflows() {
        let extras: Vec<(String, bool)> = ["New", "sbx asks 88", "Plan waiting", "", "", "Mac", "Requests 3"].iter().map(|l| (l.to_string(), true)).collect();
        let (c, k) = top_fit(1300.0, "Jarvis", "2 side by side", &extras);
        assert!(!c && k == extras.len(), "wide: all inline ({c}, {k})");
        let (c, k) = top_fit(700.0, "Jarvis", "2 side by side", &extras);
        assert!(c && k < extras.len(), "narrow: compact and an overflow ({c}, {k})");
        let (c, k) = top_fit(330.0, "Jarvis · 2 to review", "2 side by side", &extras);
        assert!(c && k == 0, "the smallest window with a notice: everything in the menu ({c}, {k})");
    }
}
