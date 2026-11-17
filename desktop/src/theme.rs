//! The look, in one table: the web UI's design tokens (`assets/app.css`,
//! `:root`, read only) plus the sizes its layout uses. The core draws its
//! chrome from here, and screens get the same table as the read-only global
//! `theme` and may name any colour, radius or size by its token name.
//!
//! Names drop the CSS prefix: `--c-text-muted` is `text-muted`, `--r-sm` is
//! `sm`. A colour prop also takes `#rrggbb`.
//!
//! The desktop departs from the web UI's values where a pair fails WCAG AA
//! ([`WEB_ADJUSTED`], checked by `contrast_of_every_pair_used`), and its
//! sizes follow its own scale: a 4 pt spacing grid, type from 12 pt (nothing
//! a person reads is smaller), radii 6 (controls), 10 (tiles), 12 (dialogs).

use rune::Color;

/// `(name, 0xRRGGBB)`, in `app.css` order.
pub const COLORS: &[(&str, u32)] = &[
    ("bg", 0x0d1117),          // page canvas, wells, inputs
    ("surface", 0x161b22),     // header, sidebar, modals, popovers
    ("surface-alt", 0x12171e), // cards inside a surface
    ("elevated", 0x21262d),    // hover backgrounds, chips, subtle dividers
    ("border", 0x30363d),      // default card/input border
    ("dim", 0x484f58),         // faint icons, hover borders
    ("text", 0xc9d1d9),        // base body text
    ("text-bright", 0xe6edf3), // headings, emphasized text
    ("text-muted", 0x9198a1),  // secondary text, field labels (web: #8b949e)
    ("text-soft", 0x9198a1),   // lead paragraphs
    ("text-dim", 0x8b949e),    // placeholders and hints only, never information (web: #6e7681)
    ("blue", 0x58a6ff),
    ("blue-tint", 0x0d1f3c),
    ("green", 0x3fb950),
    ("green-strong", 0x2ea043),
    ("green-deep", 0x238636),
    ("green-tint", 0x1c2b1c),
    ("red", 0xf85149),
    ("red-border", 0x2d1618),
    ("red-tint", 0x3d1c1c),
    ("amber", 0xd29922),
    ("amber-strong", 0xe3b341),
    ("amber-tint", 0x3c2d0d),
    ("purple", 0xd2a8ff),
    ("purple-strong", 0x8957e5),
    ("purple-alt", 0xa371f7),
    ("purple-tint", 0x22163c),
    ("group-1", 0xf778ba),
    ("group-2", 0x39c5cf),
    ("group-3", 0xc9b38a),
    ("group-4", 0xa371f7),
    ("hover-subtle", 0x1c2128),
    ("active-bg", 0x282e36),
    ("white", 0xffffff),
    // Not tokens in app.css but literals of its layout: the floor the
    // header, sidebar and panes stand on (`body`, `.layout`). The pane bar
    // was the web UI's darkslategrey slab; here it is a neutral surface with
    // a 2 pt accent line (shell.rs), and the token stays for screens.
    ("floor", 0x080b0f),
    ("pane-bar", 0x161b22),
    // The desktop's own: the keyboard focus ring (2 pt, 2 pt outside), and
    // the edge of a control a person types into or picks from (inputs,
    // selects, checkboxes), 3:1 on every surface as WCAG 1.4.11 asks.
    ("focus", 0x58a6ff),
    ("control-border", 0x6e7681),
    // The primary button hovered: lighter than green-deep, white still 4.5:1.
    ("primary-hover", 0x2a7f3a),
    // Red as words on a group's tint (the list's "unreachable", a menu's
    // destructive label): `red` falls under 4.5:1 on the lighter tints.
    ("red-text", 0xff7b72),
    // The status bar at the window's foot: the app's quietest surface (the
    // floor itself) under one hairline.
    ("status-bar", 0x080b0f),
    ("status-line", 0x21262d),
    // Who makes a session's model (the sidebar's model words): text colour
    // only, regular weight, never a fill. Not colour by meaning: the
    // terracotta sits between `red-text` and `amber` in hue, Gemini's blue
    // is lighter than the `blue` state word beside it.
    ("vendor-anthropic", 0xe2875a),
    ("vendor-openai", 0xe6edf3),
    ("vendor-google", 0x8ab4f8),
    // Keyword pills (emphasis.rs): the kind of work, and the filled pill of
    // an irreversible or costly consequence (white text on it, 4.5:1). The
    // other tones are the colour-by-meaning tokens above; each pill's tint
    // and text are derived from the ground it sits on (`emphasis::palette`).
    ("cyan", 0x39c5cf),
    ("pill-danger", 0xb62324),
];

/// Tokens whose desktop value differs from app.css, with app.css's value:
/// each failed WCAG AA on a surface the desktop draws it on.
pub const WEB_ADJUSTED: &[(&str, u32)] = &[("text-muted", 0x8b949e), ("text-dim", 0x6e7681)];

/// Corner radii: 6 for controls (buttons, rows, chips), 10 for tiles, cards
/// and menus, 12 for dialogs.
pub const RADII: &[(&str, f32)] = &[("xs", 6.0), ("sm", 6.0), ("md", 10.0), ("lg", 12.0), ("pill", 999.0)];

/// The type scale (pt). Nothing a person must read is under 12: `caption`
/// 12 (second lines, labels, badges), `body` 13, `title` 14 (row names,
/// panel titles), `heading` 16 (dialog titles). The older names stay for
/// screens written against them: `xxs`, `xs` and `sm` are 12, `md` 13,
/// `lg` 14, `xl` 16. Hierarchy comes from weight (400, 500, 600) and
/// colour, never from capitals.
pub const TEXT_SIZES: &[(&str, f32)] = &[
    ("caption", 12.0),
    ("body", 13.0),
    ("title", 14.0),
    ("heading", 16.0),
    ("xxs", 12.0),
    ("xs", 12.0),
    ("sm", 12.0),
    ("md", 13.0),
    ("lg", 14.0),
    ("xl", 16.0),
];

/// Line height of wrapped text, a multiple of the size.
pub const LINE_HEIGHT: f32 = 1.4;

/// The spacing grid (4 pt): `xxs` 2 only between stacked lines of one
/// thing, then 4, 8, 12, 16, 24.
pub const SPACE: &[(&str, f32)] = &[("xxs", 2.0), ("xs", 4.0), ("sm", 8.0), ("md", 12.0), ("lg", 16.0), ("xl", 24.0)];

/// Every spacing value a built-in screen may write as a number.
pub const GRID: &[f32] = &[0.0, 2.0, 4.0, 8.0, 12.0, 16.0, 24.0];

/// Layout constants of the shell, from `app.css`.
pub mod layout {
    /// The top bar: 28 pt controls with 8 pt above and below.
    pub const TOP_BAR_H: f32 = 44.0;
    /// The gutter between the top bar, the sidebar, the panes and panels.
    pub const GUTTER: f32 = 8.0;
    /// The sidebar's width by default; the person drags its edge between
    /// [`SIDEBAR_MIN`] and [`SIDEBAR_MAX`] (remembered).
    pub const SIDEBAR_W: f32 = 264.0;
    pub const SIDEBAR_MIN: f32 = 200.0;
    pub const SIDEBAR_MAX: f32 = 420.0;
    /// The sidebar's inner padding.
    pub const SIDEBAR_PAD: f32 = 12.0;
    /// A pane's bar: 24 pt controls with 4 pt above and below.
    pub const PANE_BAR_H: f32 = 32.0;
    /// Control heights: the top bar's, a pane bar's (dense), and the least
    /// size of anything pressable.
    pub const CONTROL_H: f32 = 28.0;
    pub const DENSE_H: f32 = 24.0;
    /// The left rail (shell/rail.rs): always shown, left of the sidebar.
    pub const RAIL_W: f32 = 44.0;
    /// The pager strip above the panes (shell/pager.rs).
    pub const PAGER_H: f32 = 28.0;
    /// The status bar across the window's foot (shell/statusbar.rs): 24 pt
    /// targets, a hairline above.
    pub const STATUS_BAR_H: f32 = 26.0;
    /// Room for macOS's close / minimise / zoom buttons, which the window draws
    /// over the content's top-left corner (the window has no title bar).
    pub const TRAFFIC_LIGHTS_W: f32 = if cfg!(target_os = "macos") { 78.0 } else { 0.0 };
}

pub fn hex(c: u32) -> Color {
    rune::color::hex(&format!("#{c:06x}")).expect("hex colour")
}

/// A token's colour; panics on a name not in [`COLORS`] (core code only).
pub fn c(name: &str) -> Color {
    hex(color_value(name).unwrap_or_else(|| panic!("no colour token {name}")))
}

pub fn color_value(name: &str) -> Option<u32> {
    COLORS.iter().find(|(n, _)| *n == name).map(|(_, v)| *v)
}

/// A colour prop: a token name or `#rrggbb`.
pub fn parse_color(s: &str) -> Result<u32, String> {
    if let Some(h) = s.strip_prefix('#') {
        if h.len() == 6 {
            if let Ok(v) = u32::from_str_radix(h, 16) {
                return Ok(v);
            }
        }
        return Err(format!("bad colour '{s}' (expected #rrggbb or a theme colour)"));
    }
    color_value(s).ok_or_else(|| format!("unknown colour '{s}' (see theme.color)"))
}

fn lookup(table: &[(&str, f32)], what: &str, s: &str) -> Result<f32, String> {
    table.iter().find(|(n, _)| *n == s).map(|(_, v)| *v).ok_or_else(|| format!("unknown {what} '{s}'"))
}

pub fn radius(s: &str) -> Result<f32, String> {
    lookup(RADII, "radius", s)
}
pub fn text_size(s: &str) -> Result<f32, String> {
    lookup(TEXT_SIZES, "text size", s)
}
pub fn space(s: &str) -> Result<f32, String> {
    lookup(SPACE, "space", s)
}

/// The table screens see as `theme`.
pub fn to_json() -> serde_json::Value {
    let colors: serde_json::Map<_, _> = COLORS.iter().map(|(n, v)| (n.to_string(), format!("#{v:06x}").into())).collect();
    let nums = |t: &[(&str, f32)]| -> serde_json::Map<String, serde_json::Value> { t.iter().map(|(n, v)| (n.to_string(), (*v as f64).into())).collect() };
    serde_json::json!({
        "color": colors,
        "radius": nums(RADII),
        "text": nums(TEXT_SIZES),
        "space": nums(SPACE),
        "line_height": LINE_HEIGHT,
        "layout": {
            "top_bar_h": layout::TOP_BAR_H, "gutter": layout::GUTTER, "sidebar_w": layout::SIDEBAR_W,
            "pane_bar_h": layout::PANE_BAR_H, "traffic_lights_w": layout::TRAFFIC_LIGHTS_W,
            "rail_w": layout::RAIL_W, "pager_h": layout::PAGER_H, "control_h": layout::CONTROL_H, "dense_h": layout::DENSE_H, "sidebar_min": layout::SIDEBAR_MIN, "sidebar_max": layout::SIDEBAR_MAX,
        },
    })
}

/// Long prose (Jarvis's answers, Markdown in screens): the app's monospace
/// by default, the product's voice. `SBXW_DESKTOP_PROSE=sans` sets it in
/// Inter, the proportional face Rune bundles (no font file of ours), to
/// compare the two; nothing else changes.
pub fn prose(size: f32, weight: f32) -> rune::TextStyle {
    let t = crate::screen::body(size.max(12.0), weight);
    if std::env::var("SBXW_DESKTOP_PROSE").is_ok_and(|v| v == "sans") {
        t.family(rune::Family::Named("Inter"))
    } else {
        t
    }
}

/// A sandbox's group colour, as the web UI gives it (`assets/js/sandboxes.js`,
/// `groupColorOf`): chats share one hue, a workspace folder gets
/// `hsl(hash % 360, 55%, 62%)` from its path, a sandbox with neither none.
pub fn group_color(workspace: Option<&str>, chat: bool) -> Option<u32> {
    if chat {
        return Some(hsl(265.0, 0.60, 0.68));
    }
    let ws = workspace.filter(|w| !w.is_empty())?;
    // JavaScript's `(hash * 31 + charCode) >>> 0` over UTF-16 code units.
    let hash = ws.encode_utf16().fold(0u32, |h, c| h.wrapping_mul(31).wrapping_add(c as u32));
    Some(hsl((hash % 360) as f32, 0.55, 0.62))
}

/// Two groups' hues closer than this (degrees) read as the same block.
pub const GROUP_MIN_HUE: f32 = 40.0;
/// Lightness as the second cue when the wheel has no free hue left.
pub const GROUP_LIGHTNESS: &[f32] = &[0.62, 0.72, 0.80];

fn hue_dist(a: f32, b: f32) -> f32 {
    let d = (a - b).rem_euclid(360.0);
    d.min(360.0 - d)
}

/// Every group's colour, distinct: the web UI's hue first (so colours
/// mostly match it), then, in a stable order (chats first, then workspaces
/// by path, whatever the list's order or filter), a hue within
/// [`GROUP_MIN_HUE`] of one already taken moves to the nearest free hue.
/// With more groups than the wheel holds apart, the hue the furthest from
/// the others, at the next lightness of [`GROUP_LIGHTNESS`]. Input: each
/// sandbox's `(name, workspace, chat)`; output: its group's colour, by name
/// (a sandbox in no group has none). A tint can differ from the web UI's.
pub fn group_colors<'a>(sandboxes: impl IntoIterator<Item = (&'a str, Option<&'a str>, bool)>) -> std::collections::HashMap<String, u32> {
    let list: Vec<(&str, Option<&str>, bool)> = sandboxes.into_iter().collect();
    // The groups: "" for chats, else the workspace path.
    let mut keys: Vec<String> = list.iter().filter_map(|(_, ws, chat)| if *chat { Some(String::new()) } else { ws.filter(|w| !w.is_empty()).map(str::to_string) }).collect();
    keys.sort();
    keys.dedup();
    let base = |k: &str| -> (f32, f32, f32) {
        if k.is_empty() {
            (265.0, 0.60, 0.68)
        } else {
            let hash = k.encode_utf16().fold(0u32, |h, c| h.wrapping_mul(31).wrapping_add(c as u32));
            ((hash % 360) as f32, 0.55, 0.62)
        }
    };
    let mut taken: Vec<(f32, usize)> = vec![];
    let mut by_key = std::collections::HashMap::new();
    for k in &keys {
        let (h0, s, l0) = base(k);
        let free = |h: f32, taken: &[(f32, usize)]| taken.iter().all(|(t, _)| hue_dist(h, *t) >= GROUP_MIN_HUE);
        let found = (0..=180).flat_map(|d| [h0 + d as f32, h0 - d as f32]).map(|h| h.rem_euclid(360.0)).find(|h| free(*h, &taken));
        let (h, level) = match found {
            Some(h) => (h, 0),
            None => {
                // The hue the furthest from the others, the next lightness.
                let h = (0..360).map(|d| (h0 + d as f32).rem_euclid(360.0)).max_by(|a, b| {
                    let m = |h: f32| taken.iter().map(|(t, _)| hue_dist(h, *t)).fold(f32::MAX, f32::min);
                    m(*a).total_cmp(&m(*b))
                }).unwrap_or(h0);
                let level = 1 + taken.iter().filter(|(t, _)| hue_dist(h, *t) < GROUP_MIN_HUE).count() % (GROUP_LIGHTNESS.len() - 1);
                (h, level)
            }
        };
        taken.push((h, level));
        let l = if level == 0 { l0 } else { GROUP_LIGHTNESS[level] };
        by_key.insert(k.clone(), hsl(h, s, l));
    }
    list.iter()
        .filter_map(|(name, ws, chat)| {
            let k = if *chat { String::new() } else { ws.filter(|w| !w.is_empty())?.to_string() };
            by_key.get(&k).map(|c| (name.to_string(), *c))
        })
        .collect()
}

/// `hsl(h, s, l)` as `0xRRGGBB` (CSS's conversion).
pub fn hsl(h: f32, s: f32, l: f32) -> u32 {
    let a = s * l.min(1.0 - l);
    let f = |n: f32| {
        let k = (n + h / 30.0) % 12.0;
        let v = l - a * (k - 3.0).min(9.0 - k).clamp(-1.0, 1.0);
        (v * 255.0).round().clamp(0.0, 255.0) as u32
    };
    f(0.0) << 16 | f(8.0) << 8 | f(4.0)
}

// ---------------------------------------------------------------------------
// Contrast (WCAG 2.2)

fn lin(c: u32) -> f64 {
    let c = c as f64 / 255.0;
    if c <= 0.03928 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

/// Relative luminance of `0xRRGGBB`.
pub fn luminance(c: u32) -> f64 {
    0.2126 * lin(c >> 16 & 255) + 0.7152 * lin(c >> 8 & 255) + 0.0722 * lin(c & 255)
}

/// The contrast ratio of two colours, 1 to 21.
pub fn contrast(a: u32, b: u32) -> f64 {
    let (x, y) = (luminance(a), luminance(b));
    (x.max(y) + 0.05) / (x.min(y) + 0.05)
}

/// A group's tint behind its block in the list (sandboxes.luau): its colour
/// at this opacity over the sidebar's surface; hovered (the group's header)
/// a little more. Its label is the colour lightened toward white.
pub const GROUP_TINT: f32 = 0.10;
pub const GROUP_TINT_HOVER: f32 = 0.16;
pub const GROUP_LABEL_LIGHT: f32 = 0.40;

/// `a` drawn at `alpha` over `b`.
pub fn over(a: u32, alpha: f32, b: u32) -> u32 {
    let ch = |s: u32| {
        let (x, y) = ((a >> s & 255) as f32, (b >> s & 255) as f32);
        ((x * alpha + y * (1.0 - alpha)).round() as u32) << s
    };
    ch(16) | ch(8) | ch(0)
}

/// The opacity a disabled control is drawn at (screen.rs, shell.rs).
pub const DISABLED_OPACITY: f32 = 0.65;

/// What a pair must reach.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Need {
    /// Text under 18 pt (everything here): 4.5:1.
    Text,
    /// Icons, rails, dots, focus rings, a control's edge: 3:1.
    Mark,
    /// A disabled control's label (drawn at [`DISABLED_OPACITY`] over its
    /// surface): 3:1. WCAG exempts inactive controls; the app still keeps
    /// them readable, because a person needs to read what they cannot do yet.
    Disabled,
}

impl Need {
    pub fn min(self) -> f64 {
        match self {
            Need::Text => 4.5,
            Need::Mark | Need::Disabled => 3.0,
        }
    }
}

/// Every (foreground, background) pair the app draws, by token, with what it
/// must reach: the core's chrome and elements (screen.rs, shell.rs,
/// approval.rs) and the colour pairs the built-in screens write. A new pair
/// goes here; `contrast_of_every_pair_used` holds them all.
pub const PAIRS: &[(&str, &str, Need)] = &[
    // Body, labels and secondary text on every surface they sit on.
    ("text", "bg", Need::Text),
    ("text", "surface", Need::Text),
    ("text", "surface-alt", Need::Text),
    ("text", "elevated", Need::Text),
    ("text", "hover-subtle", Need::Text),
    ("text", "active-bg", Need::Text),
    ("text-bright", "bg", Need::Text),
    ("text-bright", "surface", Need::Text),
    ("text-bright", "surface-alt", Need::Text),
    ("text-bright", "elevated", Need::Text),
    ("text-bright", "active-bg", Need::Text),
    ("text-bright", "red-tint", Need::Text),
    ("text-bright", "amber-tint", Need::Text),
    ("text-bright", "blue-tint", Need::Text),
    ("text-muted", "bg", Need::Text),
    ("text-muted", "surface", Need::Text),
    ("text-muted", "surface-alt", Need::Text),
    ("text-muted", "elevated", Need::Text),
    ("text-muted", "hover-subtle", Need::Text),
    ("text-muted", "active-bg", Need::Text),
    ("text-soft", "bg", Need::Text),
    ("text-soft", "surface", Need::Text),
    // Placeholders: in inputs (bg) and in the palette's field (surface).
    ("text-dim", "bg", Need::Text),
    ("text-dim", "surface", Need::Text),
    ("text-dim", "elevated", Need::Text),
    // Tones as text: badges on their tint, coloured labels on surfaces.
    ("blue", "blue-tint", Need::Text),
    ("green", "green-tint", Need::Text),
    ("amber", "amber-tint", Need::Text),
    ("amber-strong", "amber-tint", Need::Text),
    ("red", "red-tint", Need::Text),
    ("purple", "purple-tint", Need::Text),
    ("blue", "bg", Need::Text),
    ("blue", "surface", Need::Text),
    ("blue", "elevated", Need::Text),
    ("green", "bg", Need::Text),
    ("green", "surface", Need::Text),
    ("green", "elevated", Need::Text),
    ("amber", "bg", Need::Text),
    ("amber", "surface", Need::Text),
    ("amber", "elevated", Need::Text),
    ("red", "bg", Need::Text),
    ("red", "surface", Need::Text),
    ("red", "elevated", Need::Text),
    ("red-text", "surface", Need::Text),
    ("red-text", "elevated", Need::Text),
    ("red-text", "red-tint", Need::Text),
    ("purple", "surface", Need::Text),
    ("purple", "elevated", Need::Text),
    // Colour by meaning on the controls' own grounds: a toned button's
    // label on its frame (elevated) and its hover or focus tint; the
    // compact row buttons' face; the network bubble's number on its fill.
    ("purple", "purple-tint", Need::Mark),
    ("green", "elevated", Need::Mark),
    ("red", "elevated", Need::Mark),
    ("blue", "elevated", Need::Mark),
    ("purple", "elevated", Need::Mark),
    ("bg", "blue", Need::Text),
    ("bg", "amber", Need::Text),
    // The primary button's label, at rest and hovered.
    ("white", "green-deep", Need::Text),
    ("white", "primary-hover", Need::Text),
    // Marks: rails, dots, icons, focus rings, control edges.
    ("green", "surface", Need::Mark),
    ("blue", "surface", Need::Mark),
    ("amber", "surface", Need::Mark),
    ("red", "surface", Need::Mark),
    ("text-muted", "surface", Need::Mark),
    ("text-muted", "elevated", Need::Mark),
    ("focus", "bg", Need::Mark),
    ("focus", "surface", Need::Mark),
    ("focus", "elevated", Need::Mark),
    ("focus", "floor", Need::Mark),
    // The status bar: numbers, words and their icons on its ground.
    ("text", "status-bar", Need::Text),
    ("text-muted", "status-bar", Need::Text),
    ("text-muted", "hover-subtle", Need::Mark),
    ("blue", "status-bar", Need::Text),
    ("green", "status-bar", Need::Text),
    ("amber", "status-bar", Need::Text),
    ("red-text", "status-bar", Need::Text),
    ("blue", "border", Need::Mark),
    ("red", "border", Need::Mark),
    ("amber", "border", Need::Mark),
    ("focus", "status-bar", Need::Mark),
    ("control-border", "bg", Need::Mark),
    ("control-border", "surface", Need::Mark),
    ("group-1", "surface", Need::Mark),
    ("group-2", "surface", Need::Mark),
    ("group-3", "surface", Need::Mark),
    ("group-4", "surface", Need::Mark),
    // Disabled labels (see Need::Disabled).
    ("text", "elevated", Need::Disabled),
    ("text-muted", "surface", Need::Disabled),
    ("text-bright", "bg", Need::Disabled),
];

/// The contrast of a [`PAIRS`] entry as drawn: a disabled label is the
/// colour at [`DISABLED_OPACITY`] over its surface.
pub fn pair_contrast(fg: &str, bg: &str, need: Need) -> f64 {
    let (f, b) = (color_value(fg).expect(fg), color_value(bg).expect(bg));
    let f = if need == Need::Disabled { over(f, DISABLED_OPACITY, b) } else { f };
    contrast(f, b)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every colour token of app.css's `:root` is here with the same value,
    /// except the few the desktop raised for contrast ([`WEB_ADJUSTED`],
    /// whose app.css values are checked so a change there is seen).
    #[test]
    fn colours_match_app_css() {
        let css = include_str!("../../assets/app.css");
        let root = &css[css.find(":root {").unwrap()..];
        let root = &root[..root.find("\n  }").unwrap()];
        let mut n = 0;
        for line in root.lines() {
            let l = line.trim();
            let Some(rest) = l.strip_prefix("--c-") else { continue };
            let Some((name, v)) = rest.split_once(':') else { continue };
            let v = v.split_whitespace().next().unwrap_or("").trim_end_matches(';');
            if !v.starts_with('#') || name.ends_with("-rgb") {
                continue;
            }
            let v = if v.len() == 4 { v[1..].chars().flat_map(|c| [c, c]).collect::<String>() } else { v[1..].to_string() };
            let web = u32::from_str_radix(&v, 16).unwrap();
            match WEB_ADJUSTED.iter().find(|(n, _)| *n == name) {
                Some((_, was)) => {
                    assert_eq!(web, *was, "--c-{name} changed in app.css: check the desktop's value");
                    assert_ne!(color_value(name), Some(web), "{name} is listed as adjusted");
                }
                None => assert_eq!(color_value(name), Some(web), "--c-{name}"),
            }
            n += 1;
        }
        assert!(n >= 30, "read {n} tokens");
        assert_eq!(WEB_ADJUSTED.len(), 2);
    }

    /// Every pair the app draws reaches WCAG AA: 4.5:1 for text (all of it
    /// is under 18 pt), 3:1 for marks and for disabled labels. Prints the
    /// table (`cargo test contrast -- --nocapture`).
    #[test]
    fn contrast_of_every_pair_used() {
        let mut bad = vec![];
        println!("| Foreground | Background | Kind | Ratio | Needs |\n|---|---|---|---|---|");
        for &(fg, bg, need) in PAIRS {
            let r = pair_contrast(fg, bg, need);
            println!("| `{fg}` | `{bg}` | {need:?} | {r:.2} | {} |", need.min());
            if r < need.min() {
                bad.push(format!("{fg} on {bg} ({need:?}): {r:.2} < {}", need.min()));
            }
        }
        assert!(bad.is_empty(), "{bad:#?}");
        // The ratio itself, against WCAG's own examples.
        assert!((contrast(0xffffff, 0x000000) - 21.0).abs() < 0.01);
        assert!((contrast(0x777777, 0xffffff) - 4.48).abs() < 0.01);
    }

    /// Built-in screens space things on the 4 pt grid: every number given
    /// to gap, pad, px or py is a grid step (or a theme name).
    #[test]
    fn screens_space_on_the_grid() {
        let mut off = vec![];
        for (name, src) in crate::screen::all_builtin() {
            for (i, line) in src.lines().enumerate() {
                for prop in ["gap", "pad", "px", "py"] {
                    let mut rest = line;
                    while let Some(at) = rest.find(&format!("{prop} = ")) {
                        let before = rest[..at].chars().last();
                        rest = &rest[at + prop.len() + 3..];
                        if before.is_some_and(|c| c.is_alphanumeric() || c == '_') {
                            continue;
                        }
                        let num: String = rest.chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
                        if let Ok(v) = num.parse::<f32>() {
                            if !GRID.contains(&v) {
                                off.push(format!("{name}.luau:{}: {prop} = {num}", i + 1));
                            }
                        }
                    }
                }
            }
        }
        assert!(off.is_empty(), "off the 4 pt grid: {off:#?}");
    }

    #[test]
    fn group_colours_match_the_web_ui() {
        // hsl(265, 60%, 68%) and hsl(120, 100%, 50%), as a browser computes them.
        assert_eq!(group_color(None, true), Some(0xa57cde));
        assert_eq!(hsl(120.0, 1.0, 0.5), 0x00ff00);
        assert_eq!(group_color(None, false), None);
        let a = group_color(Some("/Users/me/dev/shop"), false).unwrap();
        assert_eq!(Some(a), group_color(Some("/Users/me/dev/shop"), false));
        assert_ne!(Some(a), group_color(Some("/Users/me/dev/sbxw"), false));
        // Every hue at 55% / 62% is a mark on the bars' surface (3:1).
        for h in 0..360 {
            assert!(contrast(hsl(h as f32, 0.55, 0.62), color_value("surface").unwrap()) >= 3.0, "hue {h}");
        }
    }

    /// Over every group tint (every hue the web UI's hash can give, and the
    /// chats' colour), what the list draws there stays readable: the row's
    /// words and the group's label 4.5:1, the rails and the group's square
    /// 3:1; on the header's hover tint too. Prints the worst of each.
    #[test]
    fn text_and_rails_read_on_every_group_tint() {
        let surface = color_value("surface").unwrap();
        let mut worst: Vec<(String, f64, f64)> = vec![];
        let mut note = |what: &str, r: f64, need: f64| match worst.iter_mut().find(|w| w.0 == what) {
            Some(w) => w.1 = w.1.min(r),
            None => worst.push((what.to_string(), r, need)),
        };
        // Every hue at every lightness the distinct-colour rule can give.
        let hues: Vec<u32> = GROUP_LIGHTNESS.iter().flat_map(|&l| (0..360).map(move |h| hsl(h as f32, 0.55, l))).chain([hsl(265.0, 0.60, 0.68)]).collect();
        for g in hues {
            let tint = over(g, GROUP_TINT, surface);
            let hover = over(g, GROUP_TINT_HOVER, surface);
            let label = over(0xffffff, GROUP_LABEL_LIGHT, g);
            for t in ["text-bright", "text", "text-muted", "amber", "blue", "red-text"] {
                note(&format!("`{t}` on a tint (text)"), contrast(color_value(t).unwrap(), tint), 4.5);
            }
            for m in ["green", "blue", "amber", "red", "text-muted"] {
                note(&format!("`{m}` on a tint (rail)"), contrast(color_value(m).unwrap(), tint), 3.0);
            }
            note("group label on its tint (text)", contrast(label, tint), 4.5);
            note("group label on its hover tint (text)", contrast(label, hover), 4.5);
            note("group square on its tint (mark)", contrast(g, tint), 3.0);
            note("`text-muted` chevron on the hover tint (mark)", contrast(color_value("text-muted").unwrap(), hover), 3.0);
            // The bubble's number on its fill, and the selected row over a tint (opaque).
            note("`bg` on `blue` (the bubble)", contrast(color_value("bg").unwrap(), color_value("blue").unwrap()), 4.5);
            note("`bg` on `amber` (the bubble)", contrast(color_value("bg").unwrap(), color_value("amber").unwrap()), 4.5);
        }
        println!("| Over every group tint | Worst | Needs |\n|---|---|---|");
        for (w, r, need) in &worst {
            println!("| {w} | {r:.2} | {need} |");
        }
        let bad: Vec<_> = worst.iter().filter(|(_, r, n)| r < n).collect();
        assert!(bad.is_empty(), "{bad:#?}");
    }

    /// Two groups never share a background: same-hash workspaces, one
    /// hashing to the chats' hue, stable under filtering, many groups.
    #[test]
    fn groups_get_distinct_colours() {
        let hue = |c: u32| {
            let (r, g, b) = (((c >> 16) & 255) as f32 / 255.0, ((c >> 8) & 255) as f32 / 255.0, (c & 255) as f32 / 255.0);
            let (mx, mn) = (r.max(g).max(b), r.min(g).min(b));
            let d = mx - mn;
            if d == 0.0 { 0.0 } else if mx == r { 60.0 * (((g - b) / d).rem_euclid(6.0)) } else if mx == g { 60.0 * ((b - r) / d + 2.0) } else { 60.0 * ((r - g) / d + 4.0) }
        };
        let web = |p: &str| (p.encode_utf16().fold(0u32, |h, c| h.wrapping_mul(31).wrapping_add(c as u32)) % 360) as f32;
        // Find two paths with the same web hue, and one at the chats' 265.
        let paths: Vec<String> = (0..5000).map(|i| format!("/Users/me/dev/p{i}")).collect();
        let (a, b) = paths.iter().enumerate().find_map(|(i, p)| paths[i + 1..].iter().find(|q| web(q) == web(p)).map(|q| (p.clone(), q.clone()))).unwrap();
        let near_chat = paths.iter().find(|p| web(p) == 265.0).unwrap().clone();
        let sbs = vec![("c1", None, true), ("c2", None, true), ("a1", Some(a.as_str()), false), ("a2", Some(a.as_str()), false), ("b1", Some(b.as_str()), false), ("n1", Some(near_chat.as_str()), false)];
        let m = group_colors(sbs.iter().map(|(n, w, c)| (*n, *w, *c)));
        assert_eq!(m["c1"], m["c2"], "one group, one colour");
        assert_eq!(m["a1"], m["a2"]);
        assert_ne!(m["a1"], m["b1"], "same hash, different groups");
        assert!(hue_dist(hue(m["a1"]), hue(m["b1"])) >= GROUP_MIN_HUE - 2.0);
        assert!(hue_dist(hue(m["n1"]), hue(m["c1"])) >= GROUP_MIN_HUE - 2.0, "a workspace at 265 moves off the chats' hue");
        assert_eq!(m["c1"], hsl(265.0, 0.60, 0.68), "the chats keep the web UI's colour");
        // Stable: the same set in another order, and a filtered list, give the same colours.
        let rev = group_colors(sbs.iter().rev().map(|(n, w, c)| (*n, *w, *c)));
        assert_eq!(m, rev);
        // Many groups: still pairwise different.
        let many: Vec<(String, String)> = (0..14).map(|i| (format!("s{i}"), format!("/Users/me/dev/many{i}"))).collect();
        let mm = group_colors(many.iter().map(|(n, w)| (n.as_str(), Some(w.as_str()), false)).chain([("chat", None, true)]));
        let mut colours: Vec<u32> = mm.values().copied().collect();
        colours.sort();
        colours.dedup();
        assert_eq!(colours.len(), 15, "15 groups, 15 colours");
    }

    #[test]
    fn colour_props() {
        assert_eq!(parse_color("text-muted"), Ok(0x9198a1));
        assert_eq!(parse_color("#00ff00"), Ok(0x00ff00));
        assert!(parse_color("#0f0").is_err());
        assert!(parse_color("pink").unwrap_err().contains("unknown colour"));
    }
}
