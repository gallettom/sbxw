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

/// The dark palette: `(name, 0xRRGGBB)`, in `app.css` order. The app's
/// first theme, the web UI's values; unchanged by the light theme.
pub const DARK: &[(&str, u32)] = &[
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
    // The one filled primary button of a view: its fill, its label, hovered
    // (lighter than the fill, the label still 4.5:1).
    ("primary", 0x238636),
    ("on-primary", 0xffffff),
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
    // terracotta sits between `red-text` and `amber` in hue, Gemini's
    // periwinkle differs from the `blue` state word by hue (232 against 212).
    ("vendor-anthropic", 0xe2875a),
    ("vendor-openai", 0xe6edf3),
    ("vendor-google", 0x9aa8ff),
    // Keyword pills (emphasis.rs): the kind of work, and the filled pill of
    // an irreversible or costly consequence (white text on it, 4.5:1). The
    // other tones are the colour-by-meaning tokens above; each pill's tint
    // and text are derived from the ground it sits on (`emphasis::palette`).
    ("cyan", 0x39c5cf),
    ("pill-danger", 0xb62324),
    // The interaction accent: links, the current tab's mark. In the dark
    // theme it is the blue the app always had; in the light theme the orange
    // (THEME.md).
    ("accent", 0x58a6ff),
    ("accent-tint", 0x0d1f3c),
    ("link", 0x58a6ff),
    // A neutral page tab that is the current one, over the pager's floor.
    ("tab-current", 0x21262d),
    // What dims the window under a dialog ([`SCRIM_ALPHA`]), the shadow under
    // menus, popovers and dialogs ([`SHADOW_ALPHA`]), and the ground behind
    // a picture or a remote screen (black in both themes).
    ("scrim", 0x080b0f),
    ("shadow", 0x000000),
    ("media", 0x000000),
];

/// The light palette: the same names, warm taupe greys and an orange accent
/// (THEME.md has each value with its contrast). Surfaces step from the floor
/// (darkest, behind everything) up to `bg` (the lightest: wells, inputs and
/// the terminal, where text has the most contrast); text steps the other way.
pub const LIGHT: &[(&str, u32)] = &[
    ("bg", 0xfdfcfa),
    ("surface", 0xf7f4ef),
    ("surface-alt", 0xf2eee8),
    ("elevated", 0xebe6de),
    ("border", 0xd9d1c5),
    ("dim", 0xbcb2a4),
    ("text", 0x3a332c),
    ("text-bright", 0x1f1a15),
    ("text-muted", 0x5c5349),
    ("text-soft", 0x5c5349),
    ("text-dim", 0x6b6258),
    ("blue", 0x2b5ea9),
    ("blue-tint", 0xe4ebf5),
    ("green", 0x2a6e35),
    ("green-strong", 0x2f7a3b),
    ("green-deep", 0x23652d),
    ("green-tint", 0xe3eee0),
    ("red", 0xb02c26),
    ("red-border", 0xecc8c2),
    ("red-tint", 0xf7e2de),
    ("amber", 0x7f5a00),
    ("amber-strong", 0x6b4b00),
    ("amber-tint", 0xf6eac2),
    ("purple", 0x6a42ad),
    ("purple-strong", 0x6a3fc0),
    ("purple-alt", 0x7347bd),
    ("purple-tint", 0xece5f6),
    ("group-1", 0xa8336f),
    ("group-2", 0x16717a),
    ("group-3", 0x76602f),
    ("group-4", 0x7347bd),
    ("hover-subtle", 0xf0ece6),
    ("active-bg", 0xe5dfd5),
    ("white", 0xffffff),
    ("floor", 0xe7e1d8),
    ("pane-bar", 0xf7f4ef),
    ("focus", 0xa84a16),
    ("control-border", 0x8a8073),
    ("primary", 0xa84a16),
    ("on-primary", 0xffffff),
    ("primary-hover", 0x8f3d10),
    ("red-text", 0xa3261f),
    ("status-bar", 0xe7e1d8),
    ("status-line", 0xd9d1c5),
    ("vendor-anthropic", 0x763a30),
    ("vendor-openai", 0x1f1a15),
    ("vendor-google", 0x4a52b0),
    ("cyan", 0x17707a),
    ("pill-danger", 0xb02c26),
    ("accent", 0xa84a16),
    ("accent-tint", 0xf6e2d5),
    ("link", 0xa04512),
    ("tab-current", 0xfdfcfa),
    ("scrim", 0x2a231c),
    ("shadow", 0x3a2e22),
    ("media", 0x000000),
];

/// Pure white and black: the ends that "move toward white or black" math
/// aims at (a pill's text, a lifted icon), not a theme's colour.
pub const WHITE: u32 = 0xffffff;
pub const BLACK: u32 = 0x000000;

/// The dark palette under its old name (`app.css` order).
pub const COLORS: &[(&str, u32)] = DARK;

/// Which palette is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Mode {
    Dark,
    Light,
}

impl Mode {
    pub fn palette(self) -> &'static [(&'static str, u32)] {
        match self {
            Mode::Dark => DARK,
            Mode::Light => LIGHT,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Mode::Dark => "dark",
            Mode::Light => "light",
        }
    }
    pub fn is_light(self) -> bool {
        self == Mode::Light
    }
}

// The one place that says which palette is active: the shell sets it from
// the person's choice (and the system's appearance, for "Match the
// system") at the top of every frame and on the toggle; every token read
// goes through it. Per thread: the UI is one thread, and tests that run
// side by side each have their own.
thread_local! {
    static ACTIVE: std::cell::Cell<Mode> = const { std::cell::Cell::new(Mode::Dark) };
    /// Bumped on every change of the active palette, so what caches a
    /// colour (a screen's `theme` table) can tell it is stale.
    static GENERATION: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// The active palette.
pub fn mode() -> Mode {
    ACTIVE.with(|a| a.get())
}

/// Makes `m` the active palette; true when it changed.
pub fn set_mode(m: Mode) -> bool {
    let was = ACTIVE.with(|a| a.replace(m));
    if was != m {
        GENERATION.with(|g| g.set(g.get() + 1));
    }
    was != m
}

/// Counts the changes of the active palette.
pub fn generation() -> u64 {
    GENERATION.with(|g| g.get())
}

/// `f` with `m` as the active palette, then the one before again.
pub fn with_mode<R>(m: Mode, f: impl FnOnce() -> R) -> R {
    struct Back(Mode);
    impl Drop for Back {
        fn drop(&mut self) {
            set_mode(self.0);
        }
    }
    let _back = Back(mode());
    set_mode(m);
    f()
}

/// The person's choice, as the state file keeps it (`theme`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Choice {
    Dark,
    Light,
    /// The system's appearance (Rune's `Cx::system_dark`).
    System,
}

impl Choice {
    pub fn parse(s: &str) -> Option<Choice> {
        match s.trim().to_ascii_lowercase().as_str() {
            "dark" => Some(Choice::Dark),
            "light" => Some(Choice::Light),
            "system" | "auto" => Some(Choice::System),
            _ => None,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Choice::Dark => "dark",
            Choice::Light => "light",
            Choice::System => "system",
        }
    }
    /// The palette this choice draws when the system is (or is not) dark.
    pub fn resolve(self, system_dark: bool) -> Mode {
        match self {
            Choice::Dark => Mode::Dark,
            Choice::Light => Mode::Light,
            Choice::System if system_dark => Mode::Dark,
            Choice::System => Mode::Light,
        }
    }
}

/// A new install's choice: follow the system (Rune reports its appearance
/// on every platform it runs on; headless runs report dark unless
/// `--theme light`).
pub const DEFAULT_CHOICE: Choice = Choice::System;

/// How opaque the scrim under a dialog is, and the shadow under menus and
/// dialogs, in the active theme: a light theme dims less (a warm dark veil
/// at 0.32 keeps the window behind recognisable) and casts a softer shadow.
pub fn scrim_alpha() -> f32 {
    if mode().is_light() { 0.32 } else { 0.6 }
}
pub fn shadow_alpha() -> f32 {
    if mode().is_light() { 0.16 } else { 0x73 as f32 / 255.0 }
}
/// The scrim's colour with its opacity.
pub fn scrim() -> Color {
    c("scrim").alpha(scrim_alpha())
}
/// The shadow's colour with its opacity.
pub fn shadow() -> Color {
    c("shadow").alpha(shadow_alpha())
}

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

/// A token's value in the active palette.
pub fn color_value(name: &str) -> Option<u32> {
    color_in(mode(), name)
}

/// A token's value in `m`'s palette.
pub fn color_in(m: Mode, name: &str) -> Option<u32> {
    m.palette().iter().find(|(n, _)| *n == name).map(|(_, v)| *v)
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
    let colors: serde_json::Map<_, _> = mode().palette().iter().map(|(n, v)| (n.to_string(), format!("#{v:06x}").into())).collect();
    let nums = |t: &[(&str, f32)]| -> serde_json::Map<String, serde_json::Value> { t.iter().map(|(n, v)| (n.to_string(), (*v as f64).into())).collect() };
    serde_json::json!({
        "mode": mode().name(),
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

/// Whether long prose is set in the proportional face (the default);
/// `SBXW_DESKTOP_PROSE=mono` is the way back to monospace everywhere.
pub fn prose_sans() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| !std::env::var("SBXW_DESKTOP_PROSE").is_ok_and(|v| v == "mono"))
}

/// `t` in the prose face: Inter, which Rune embeds and registers itself
/// (its default faces, no font file of ours, so it is there on every
/// system; a glyph Inter lacks falls back to a system face as any text's
/// does). Unchanged under `SBXW_DESKTOP_PROSE=mono`.
pub fn as_prose(t: rune::TextStyle) -> rune::TextStyle {
    if prose_sans() {
        t.family(rune::Family::Named("Inter"))
    } else {
        t
    }
}

/// Long prose: Jarvis's answers and Markdown in screens (Help), a wrapped
/// `text` in a screen (explanations, descriptions), a core dialog's
/// sentences, the empty pane's line. Terminals, code, paths, names,
/// numbers, badges, bars, labels and tooltips stay monospace (tooltips are
/// Rune's, in the app's one UI face).
pub fn prose(size: f32, weight: f32) -> rune::TextStyle {
    as_prose(crate::screen::body(size.max(12.0), weight))
}

/// A sandbox's group colour, as the web UI gives it (`assets/js/sandboxes.js`,
/// `groupColorOf`): chats share one hue, a workspace folder gets
/// `hsl(hash % 360, 55%, 62%)` from its path, a sandbox with neither none.
pub fn group_color(workspace: Option<&str>, chat: bool) -> Option<u32> {
    let g = group_look();
    if chat {
        return Some(hsl(265.0, g.chat.0, g.chat.1));
    }
    let ws = workspace.filter(|w| !w.is_empty())?;
    // JavaScript's `(hash * 31 + charCode) >>> 0` over UTF-16 code units.
    let hash = ws.encode_utf16().fold(0u32, |h, c| h.wrapping_mul(31).wrapping_add(c as u32));
    Some(hsl((hash % 360) as f32, g.saturation, g.lightness[0]))
}

/// Two groups' hues closer than this (degrees) read as the same block.
pub const GROUP_MIN_HUE: f32 = 40.0;
/// Lightness as the second cue when the wheel has no free hue left (the
/// dark theme's; [`group_look`] has the active theme's).
pub const GROUP_LIGHTNESS: &[f32] = &[0.62, 0.72, 0.80];

/// How a theme draws a group's colour: the web UI's hue at this saturation
/// and lightness (then the lighter or darker steps when the wheel is full),
/// the chats' purple, and its tints (the list's block, the pane bar, the
/// page tabs). The hue rule ([`GROUP_MIN_HUE`]) is the same in both themes;
/// on light the colour is deeper (a mark at 3:1 on a light surface) and its
/// tints are pastels of it.
#[derive(Clone, Copy, Debug)]
pub struct GroupLook {
    pub saturation: f32,
    pub lightness: &'static [f32],
    /// The chats' (saturation, lightness), at hue 265.
    pub chat: (f32, f32),
    /// The list's block, and its header hovered.
    pub tint: f32,
    pub tint_hover: f32,
    /// The group's label: its colour moved this far toward white (dark) or
    /// `text-bright`, the darkest text (light).
    pub label: f32,
    /// A pane bar, focused and not.
    pub bar: f32,
    pub bar_unfocused: f32,
    /// A page tab, current and not.
    pub tab_current: f32,
    pub tab: f32,
}

pub const GROUP_DARK: GroupLook = GroupLook {
    saturation: 0.55,
    lightness: GROUP_LIGHTNESS,
    chat: (0.60, 0.68),
    tint: GROUP_TINT,
    tint_hover: GROUP_TINT_HOVER,
    label: GROUP_LABEL_LIGHT,
    bar: BAR_TINT,
    bar_unfocused: BAR_TINT_UNFOCUSED,
    tab_current: TAB_TINT_CURRENT,
    tab: TAB_TINT,
};

/// On light: the hue at 50% saturation, 35% lightness (then 28, 22), so the
/// square and the accent line mark at 3:1 on the surfaces; the block a 10%
/// pastel of it, the pane bar 16% (unfocused 12%), the current tab 16%
/// (others 8%): the same steps as dark, a little lower, since a hue shows
/// more on a light ground.
pub const GROUP_LIGHT: GroupLook = GroupLook {
    saturation: 0.50,
    lightness: &[0.35, 0.28, 0.22],
    chat: (0.45, 0.48),
    tint: 0.10,
    tint_hover: 0.16,
    label: 0.45,
    bar: 0.16,
    bar_unfocused: 0.12,
    tab_current: 0.16,
    tab: 0.08,
};

/// The active theme's [`GroupLook`].
pub fn group_look() -> GroupLook {
    look_of(mode())
}
pub fn look_of(m: Mode) -> GroupLook {
    match m {
        Mode::Dark => GROUP_DARK,
        Mode::Light => GROUP_LIGHT,
    }
}

/// A group's tints as the list draws them: its block, the header hovered,
/// and the label's colour (sandboxes.luau reads these as `app.group_look`).
pub fn group_tints(g: u32) -> (u32, u32, u32) {
    let look = group_look();
    let surface = color_value("surface").unwrap();
    // Toward white on dark (as the web UI), toward the darkest text on light.
    let to = if mode().is_light() { color_value("text-bright").unwrap() } else { WHITE };
    (over(g, look.tint, surface), over(g, look.tint_hover, surface), over(to, look.label, g))
}

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
    let look = group_look();
    let base = |k: &str| -> (f32, f32, f32) {
        if k.is_empty() {
            (265.0, look.chat.0, look.chat.1)
        } else {
            let hash = k.encode_utf16().fold(0u32, |h, c| h.wrapping_mul(31).wrapping_add(c as u32));
            ((hash % 360) as f32, look.saturation, look.lightness[0])
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
                let level = 1 + taken.iter().filter(|(t, _)| hue_dist(h, *t) < GROUP_MIN_HUE).count() % (look.lightness.len() - 1);
                (h, level)
            }
        };
        taken.push((h, level));
        let l = if level == 0 { l0 } else { look.lightness[level] };
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

/// A pane bar's tint of its group's colour (focused, unfocused) over the
/// bar's surface: strong enough to tell a group at a glance (the list's
/// block is 10%).
pub const BAR_TINT: f32 = 0.20;
pub const BAR_TINT_UNFOCUSED: f32 = 0.16;

/// A pane bar's ground: the group's colour over the bar's neutral surface,
/// or that surface for an ungrouped sandbox. The page tabs use the same.
pub fn bar_ground(group: Option<u32>, focused: bool) -> u32 {
    let surface = color_value(if focused { "surface" } else { "surface-alt" }).unwrap();
    let look = group_look();
    match group {
        Some(g) => over(g, if focused { look.bar } else { look.bar_unfocused }, surface),
        None => surface,
    }
}

/// A page tab's tint of its group's colour over the pager's floor: the
/// current page, the others.
pub const TAB_TINT_CURRENT: f32 = 0.28;
pub const TAB_TINT: f32 = 0.14;

/// A page tab's ground: the group's colour over the floor, or neutral (the
/// current one elevated, the others the floor).
pub fn tab_ground(group: Option<u32>, current: bool) -> u32 {
    let floor = color_value("floor").unwrap();
    let look = group_look();
    match group {
        Some(g) => over(g, if current { look.tab_current } else { look.tab }, floor),
        None if current => color_value("tab-current").unwrap(),
        None => floor,
    }
}

/// Secondary text on a tinted ground (a pane bar's session kind, a tab's
/// sandbox prefix): `text-muted`, moved toward `text-bright` until it reads
/// at 4.5:1 there (a yellow or green tint lifts the ground).
pub fn muted_on(ground: u32) -> u32 {
    let (m, b) = (color_value("text-muted").unwrap(), color_value("text-bright").unwrap());
    (0..=10).map(|k| over(b, k as f32 / 10.0, m)).find(|c| contrast(*c, ground) >= Need::Text.min()).unwrap_or(b)
}

/// A tone's icon on a tinted ground: the tone, moved toward `text-bright`
/// (white on dark, the darkest text on light) until it marks at 3:1 there
/// (red on the lightest yellow tint needs it).
pub fn mark_on(fg: u32, ground: u32) -> u32 {
    let to = if luminance(ground) > 0.18 { color_value("text-bright").unwrap() } else { 0xffffff };
    (0..=10).map(|k| over(to, k as f32 / 20.0, fg)).find(|c| contrast(*c, ground) >= Need::Mark.min()).unwrap_or(to)
}

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
    ("on-primary", "primary", Need::Text),
    ("on-primary", "primary-hover", Need::Text),
    // The accent: the focused pane's border, the focus ring, the current
    // tab; links as text on every surface they sit on.
    ("accent", "bg", Need::Mark),
    ("accent", "surface", Need::Mark),
    ("accent", "floor", Need::Mark),
    ("accent", "elevated", Need::Mark),
    ("primary", "surface", Need::Mark),
    ("link", "bg", Need::Text),
    ("link", "surface", Need::Text),
    ("link", "elevated", Need::Text),
    ("text-bright", "accent-tint", Need::Text),
    ("text", "tab-current", Need::Text),
    ("text-muted", "tab-current", Need::Text),
    // Who makes a model: the vendor words on the sidebar's surfaces.
    ("vendor-anthropic", "surface", Need::Text),
    ("vendor-anthropic", "active-bg", Need::Text),
    ("vendor-openai", "surface", Need::Text),
    ("vendor-google", "surface", Need::Text),
    ("vendor-google", "active-bg", Need::Text),
    // Keyword pill kinds and the filled danger pill.
    ("cyan", "surface", Need::Text),
    ("white", "pill-danger", Need::Text),
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

/// `0xRRGGBB` as (hue in degrees, saturation, lightness), CSS's HSL.
pub fn to_hsl(c: u32) -> (f32, f32, f32) {
    let (r, g, b) = (((c >> 16) & 255) as f32 / 255.0, ((c >> 8) & 255) as f32 / 255.0, (c & 255) as f32 / 255.0);
    let (mx, mn) = (r.max(g).max(b), r.min(g).min(b));
    let (d, l) = (mx - mn, (mx + mn) / 2.0);
    if d == 0.0 {
        return (0.0, 0.0, l);
    }
    let s = d / (1.0 - (2.0 * l - 1.0).abs());
    let h = if mx == r { 60.0 * ((g - b) / d).rem_euclid(6.0) } else if mx == g { 60.0 * ((b - r) / d + 2.0) } else { 60.0 * ((r - g) / d + 4.0) };
    (h, s, l)
}

/// The accent, amber and the vendor terracotta: at least this far apart in
/// hue (degrees) and in lightness (HSL points), pairwise, in each theme.
pub const ORANGE_MIN_HUE: f32 = 12.0;
pub const ORANGE_MIN_LIGHT: f32 = 4.0;

#[cfg(test)]
mod tests {
    use super::*;

    /// Every colour token of app.css's `:root` is here with the same value,
    /// except the few the desktop raised for contrast ([`WEB_ADJUSTED`],
    /// whose app.css values are checked so a change there is seen).
    #[test]
    fn colours_match_app_css() {
        with_mode(Mode::Dark, colours_match_app_css_dark);
    }

    fn colours_match_app_css_dark() {
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
        for m in MODES {
            with_mode(m, || {
                println!("\n{m:?}\n\n| Foreground | Background | Kind | Ratio | Needs |\n|---|---|---|---|---|");
                for &(fg, bg, need) in PAIRS {
                    let r = pair_contrast(fg, bg, need);
                    println!("| `{fg}` | `{bg}` | {need:?} | {r:.2} | {} |", need.min());
                    if r < need.min() {
                        bad.push(format!("{m:?}: {fg} on {bg} ({need:?}): {r:.2} < {}", need.min()));
                    }
                }
            });
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
        with_mode(Mode::Dark, group_colours_match_the_web_ui_dark);
        // On light: every hue the hash can give, at every step, marks at 3:1
        // on the surfaces it is drawn on.
        with_mode(Mode::Light, || {
            let look = group_look();
            for &l in look.lightness {
                for h in 0..360 {
                    for ground in ["surface", "surface-alt"] {
                        let g = hsl(h as f32, look.saturation, l);
                        assert!(contrast(g, color_value(ground).unwrap()) >= 3.0, "hue {h} at {l} on {ground}");
                    }
                }
            }
            assert!(contrast(hsl(265.0, look.chat.0, look.chat.1), color_value("surface").unwrap()) >= 3.0);
        });
    }

    fn group_colours_match_the_web_ui_dark() {
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
        for m in MODES {
            with_mode(m, text_and_rails_read_on_every_group_tint_in);
        }
    }

    fn text_and_rails_read_on_every_group_tint_in() {
        let look = group_look();
        println!("\n{:?}", mode());
        let mut worst: Vec<(String, f64, f64)> = vec![];
        let mut note = |what: &str, r: f64, need: f64| match worst.iter_mut().find(|w| w.0 == what) {
            Some(w) => w.1 = w.1.min(r),
            None => worst.push((what.to_string(), r, need)),
        };
        // Every hue at every lightness the distinct-colour rule can give.
        let hues: Vec<u32> = look.lightness.iter().flat_map(|&l| (0..360).map(move |h| hsl(h as f32, look.saturation, l))).chain([hsl(265.0, look.chat.0, look.chat.1)]).collect();
        for g in hues {
            let (tint, hover, label) = group_tints(g);
            for t in ["text-bright", "text", "text-muted", "amber", "blue", "red-text", "vendor-anthropic", "vendor-openai", "vendor-google"] {
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
        with_mode(Mode::Dark, colour_props_dark);
        assert_eq!(with_mode(Mode::Light, || parse_color("text-muted")), Ok(color_in(Mode::Light, "text-muted").unwrap()));
    }

    fn colour_props_dark() {
        assert_eq!(parse_color("text-muted"), Ok(0x9198a1));
        assert_eq!(parse_color("#00ff00"), Ok(0x00ff00));
        assert!(parse_color("#0f0").is_err());
        assert!(parse_color("pink").unwrap_err().contains("unknown colour"));
    }

    /// Every hue a group can take (the wheel at each lightness, and the
    /// chats' purple): on its pane bar (focused and not) and its page tab
    /// (current and not), the labels read at 4.5:1, the icons and dots in
    /// their tones and the focus border at 3:1.
    #[test]
    fn contrast_on_every_group_tint() {
        for m in MODES {
            with_mode(m, contrast_on_every_group_tint_in);
        }
    }

    fn contrast_on_every_group_tint_in() {
        let look = group_look();
        let mut groups: Vec<u32> = look.lightness.iter().flat_map(|l| (0..36).map(move |h| hsl(h as f32 * 10.0, look.saturation, *l))).collect();
        groups.push(hsl(265.0, look.chat.0, look.chat.1));
        let tones = ["blue", "purple", "red", "green", "amber"];
        let mut bad = vec![];
        for g in groups.iter().map(|g| Some(*g)).chain([None]) {
            let grounds = [("bar", bar_ground(g, true)), ("bar unfocused", bar_ground(g, false)), ("tab", tab_ground(g, true)), ("tab other", tab_ground(g, false))];
            for (what, ground) in grounds {
                for (fg, c) in [("text-bright", color_value("text-bright").unwrap()), ("muted", muted_on(ground))] {
                    let r = contrast(c, ground);
                    if r < Need::Text.min() {
                        bad.push(format!("{fg} on {what} of {g:06x?}: {r:.2}"));
                    }
                }
                // Bars draw every tone's icon; tabs only their amber and blue dots.
                let drawn: &[&str] = if what.starts_with("bar") { &tones } else { &["amber", "blue"] };
                for t in drawn.iter().copied().chain(["focus"]) {
                    let c = if what.starts_with("bar") && t != "focus" { mark_on(color_value(t).unwrap(), ground) } else { color_value(t).unwrap() };
                    let r = contrast(c, ground);
                    if r < Need::Mark.min() {
                        bad.push(format!("{t} on {what} of {g:06x?}: {r:.2}"));
                    }
                }
            }
        }
        assert!(bad.is_empty(), "{:?}: {}", mode(), bad.join("\n"));
    }

    const MODES: [Mode; 2] = [Mode::Dark, Mode::Light];

    /// Both palettes name the same tokens, in the same order, each once.
    #[test]
    fn every_token_is_in_both_palettes() {
        fn names(p: &'static [(&'static str, u32)]) -> Vec<&'static str> {
            p.iter().map(|(n, _)| *n).collect()
        }
        assert_eq!(names(DARK), names(LIGHT));
        let mut seen = std::collections::HashSet::new();
        for (n, _) in DARK {
            assert!(seen.insert(*n), "{n} twice");
        }
        // The dark palette is today's: a few anchors of the web UI.
        assert_eq!(color_in(Mode::Dark, "bg"), Some(0x0d1117));
        assert_eq!(color_in(Mode::Dark, "focus"), Some(0x58a6ff));
        assert_eq!(color_in(Mode::Dark, "primary"), color_in(Mode::Dark, "green-deep"));
    }

    /// The active palette is per process, a forced one per thread.
    #[test]
    fn the_active_palette_switches() {
        assert_eq!(with_mode(Mode::Light, || c("bg")), hex(0xfdfcfa));
        assert_eq!(with_mode(Mode::Dark, || c("bg")), hex(0x0d1117));
        let nested = with_mode(Mode::Light, || with_mode(Mode::Dark, || mode()) == Mode::Dark && mode() == Mode::Light);
        assert!(nested);
        assert_eq!(Choice::parse("Light"), Some(Choice::Light));
        assert_eq!(Choice::System.resolve(true), Mode::Dark);
        assert_eq!(Choice::System.resolve(false), Mode::Light);
        assert_eq!(with_mode(Mode::Light, || to_json()["mode"].clone()), "light");
    }

    /// The three oranges that meet on a sidebar row: the accent (the focus
    /// ring, the primary button, links: interaction), amber (needs you: a
    /// state) and Anthropic's terracotta (a vendor word). In each theme every
    /// pair differs in hue by [`ORANGE_MIN_HUE`] degrees and in HSL
    /// lightness by [`ORANGE_MIN_LIGHT`] points (the dark theme has no orange
    /// accent: its accent is the blue, far from both).
    #[test]
    fn the_three_oranges_stay_apart() {
        for m in MODES {
            let three = [("accent", color_in(m, "focus").unwrap()), ("amber", color_in(m, "amber").unwrap()), ("vendor", color_in(m, "vendor-anthropic").unwrap())];
            println!("{m:?}");
            for (n, v) in three {
                let (h, s, l) = to_hsl(v);
                println!("  {n}: #{v:06x} hue {h:.0} sat {:.0} light {:.0}", s * 100.0, l * 100.0);
            }
            for i in 0..3 {
                for j in i + 1..3 {
                    let ((a, x), (b, y)) = (three[i], three[j]);
                    let ((h1, _, l1), (h2, _, l2)) = (to_hsl(x), to_hsl(y));
                    let dh = hue_dist(h1, h2);
                    let dl = (l1 - l2).abs() * 100.0;
                    assert!(dh >= ORANGE_MIN_HUE, "{m:?}: {a} and {b} {dh:.0} degrees apart");
                    assert!(dl >= ORANGE_MIN_LIGHT, "{m:?}: {a} and {b} {dl:.1} points of lightness apart");
                }
            }
        }
    }

    /// No colour is written outside this file: every literal `0xRRGGBB` or
    /// `#rrggbb` in the app's Rust and Luau sources is a token here, except
    /// in tests, fixtures and the few places listed (with why).
    #[test]
    fn no_literal_colour_outside_the_theme() {
        // (file, what may stay): data that is not a colour, and the terminal
        // module, whose ANSI palettes are its own tokens (terminal/theme.rs).
        const ALLOWED: &[(&str, &str)] = &[
            ("src/terminal/theme.rs", "the terminal's palettes: its own tokens, tested there"),
            ("src/feat/threads/standin.rs", "a commit hash, not a colour"),
            ("src/bin/term-demo.rs", "a test tool's banner"),
        ];
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut files = vec![];
        let mut stack = vec![root.join("src"), root.join("screens")];
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(&d).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else if p.extension().is_some_and(|x| x == "rs" || x == "luau") {
                    files.push(p);
                }
            }
        }
        let mut found = vec![];
        for p in files {
            let rel = p.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/");
            let name = p.file_name().unwrap().to_string_lossy();
            if rel == "src/theme.rs" || name.ends_with("tests.rs") || name == "tests.rs" || ALLOWED.iter().any(|(f, _)| *f == rel) {
                continue;
            }
            let src = std::fs::read_to_string(&p).unwrap();
            // Unit tests at the end of a file are fixtures too.
            let body = src.split("#[cfg(test)]").next().unwrap();
            for (n, line) in body.lines().enumerate() {
                let code = line.split("//").next().unwrap();
                let code = if rel.ends_with(".luau") { line.split("--").next().unwrap() } else { code };
                if literal_colour(code) {
                    found.push(format!("{rel}:{}: {}", n + 1, line.trim()));
                }
            }
        }
        assert!(found.is_empty(), "literal colours outside theme.rs:\n{}", found.join("\n"));
    }

    /// `0xRRGGBB` or `#rrggbb` (`#rrggbbaa` too) in a line of code.
    fn literal_colour(code: &str) -> bool {
        let b = code.as_bytes();
        let hex = |i: usize, n: usize| i + n <= b.len() && b[i..i + n].iter().all(u8::is_ascii_hexdigit) && (i + n == b.len() || !b[i + n].is_ascii_alphanumeric());
        (0..b.len()).any(|i| {
            let start = i == 0 || !b[i - 1].is_ascii_alphanumeric();
            (start && b[i] == b'0' && b.get(i + 1) == Some(&b'x') && hex(i + 2, 6)) || (b[i] == b'#' && (hex(i + 1, 6) || hex(i + 1, 8)))
        })
    }
}
