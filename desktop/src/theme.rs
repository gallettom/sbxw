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
    ("text-muted", 0x9198a1),  // secondary text: labels, hints, ages, placeholders; 4.5:1 on a selected row too (web: #8b949e)
    ("text-soft", 0x9198a1),   // lead paragraphs
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
    // The explorer's file icons by kind: quiet hues, never a meaning
    // colour's (folders are `blue`); a mark at 3:1 on the explorer's grounds.
    ("file-doc", 0x6cb6c8),
    ("file-code", 0xb392f0),
    ("file-image", 0xe38bb5),
    ("file-data", 0x9cc26a),
    ("file-archive", 0xc9a36b),
    // Where the person writes to an agent (the Sessions composer, Jarvis's
    // input, Canvas's ask and remarks, a request's answer): raised a step
    // above the panels it sits in, as surfaces rise toward light here (a
    // modal over the page, a card over a panel). See `compose_stands_out`.
    ("compose", 0x252f3e),
    ("compose-edge", 0x7d8796),
    // The core's notice in the top bar, by kind: the kind's colour mixed
    // onto a raised ground (blue 26 %, amber 26 %, red 30 % over `surface`),
    // lighter than the bar by NOTICE_STEP, with an edge and an icon in the
    // kind's colour. Its text is `text-bright`, its offer too (a link
    // colour falls under 4.5:1 on these).
    ("notice-info", 0x273f5b),
    ("notice-ask", 0x473c22),
    ("notice-error", 0x5a2b2e),
    // The Sessions conversation's message bubbles: the agent's `green` and
    // the person's `blue` at 12 % over `bg` (the fill) and 32 % (the edge).
    // Text stays `text`, `text-bright` or `text-muted` on them.
    ("bubble-agent", 0x13251e),
    ("bubble-agent-edge", 0x1d4729),
    ("bubble-you", 0x162333),
    ("bubble-you-edge", 0x254161),
];

/// Graphite: the same names on neutral dark greys (a ground near #202124,
/// panels #292a2d, raised #35363a, hairlines #3c4043, the range of a
/// browser's night mode) with an orange accent for interaction. Meaning
/// colours are the dark theme's, re-checked on grey (THEME.md lists what
/// moved and why).
pub const GRAPHITE: &[(&str, u32)] = &[
    ("bg", 0x202124),
    ("surface", 0x292a2d),
    ("surface-alt", 0x252629),
    ("elevated", 0x35363a),
    ("border", 0x3c4043),
    ("dim", 0x5f6368),
    ("text", 0xdadce0),
    ("text-bright", 0xf1f3f4),
    ("text-muted", 0xa3a9af),
    ("text-soft", 0xa3a9af),
    ("blue", 0x6cb0ff),
    ("blue-tint", 0x303742),
    ("green", 0x4cc35d),
    ("green-strong", 0x2ea043),
    ("green-deep", 0x238636),
    ("green-tint", 0x2c3932),
    ("red", 0xff7369),
    ("red-border", 0x4b3637),
    ("red-tint", 0x3e3133),
    ("amber", 0xdea42c),
    ("amber-strong", 0xe3b341),
    ("amber-tint", 0x3d372d),
    ("purple", 0xd2a8ff),
    ("purple-strong", 0x8957e5),
    ("purple-alt", 0xa371f7),
    ("purple-tint", 0x3a3742),
    ("group-1", 0xf778ba),
    ("group-2", 0x39c5cf),
    ("group-3", 0xc9b38a),
    ("group-4", 0xa371f7),
    ("hover-subtle", 0x2f3033),
    ("active-bg", 0x3b3c40),
    ("white", 0xffffff),
    ("floor", 0x1b1c1e),
    ("pane-bar", 0x292a2d),
    ("focus", 0xe8833a),
    ("control-border", 0x80868b),
    ("primary", 0xe8833a),
    ("on-primary", 0x1b1c1e),
    ("primary-hover", 0xf0955a),
    ("red-text", 0xff8c84),
    ("status-bar", 0x1b1c1e),
    ("status-line", 0x35363a),
    ("vendor-anthropic", 0xd99a8c),
    ("vendor-openai", 0xf1f3f4),
    ("vendor-google", 0x9aa8ff),
    ("cyan", 0x39c5cf),
    ("pill-danger", 0xb62324),
    ("accent", 0xe8833a),
    ("accent-tint", 0x40352f),
    ("link", 0xf0955a),
    ("tab-current", 0x35363a),
    ("scrim", 0x0e0e0f),
    ("shadow", 0x000000),
    ("media", 0x000000),
    ("file-doc", 0x6cb6c8),
    ("file-code", 0xb392f0),
    ("file-image", 0xe38bb5),
    ("file-data", 0x9cc26a),
    ("file-archive", 0xc9a36b),
    ("compose", 0x3a3e46),
    ("compose-edge", 0x90969c),
    ("notice-info", 0x3a4d64),
    ("notice-ask", 0x584a2d),
    ("notice-error", 0x6a3838),
    // The Sessions conversation's message bubbles: the agent's `green` and
    // the person's `blue` at 12 % over `bg` (the fill) and 32 % (the edge).
    // Text stays `text`, `text-bright` or `text-muted` on them.
    ("bubble-agent", 0x25342b),
    ("bubble-agent-edge", 0x2e5536),
    ("bubble-you", 0x29323e),
    ("bubble-you-edge", 0x384f6a),
];

/// Pure white and black: the ends that "move toward white or black" math
/// aims at (a pill's text, a lifted icon), not a theme's colour.
pub const WHITE: u32 = 0xffffff;
pub const BLACK: u32 = 0x000000;

/// The dark palette under its old name (`app.css` order).
pub const COLORS: &[(&str, u32)] = DARK;

/// The app's themes, by name: each one palette behind the same tokens. A
/// theme is added as one more table and one more entry here (the light
/// variants explored for this app are in THEME.md, not shipped).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Mode {
    /// The web UI's blue-black: the default.
    Dark,
    /// Neutral dark greys with an orange accent.
    Graphite,
}

impl Mode {
    /// Every theme, in the order the switch cycles and its menu lists them.
    pub const ALL: [Mode; 2] = [Mode::Dark, Mode::Graphite];

    pub fn palette(self) -> &'static [(&'static str, u32)] {
        match self {
            Mode::Dark => DARK,
            Mode::Graphite => GRAPHITE,
        }
    }
    /// Its name in the state file and in `set_theme`.
    pub fn name(self) -> &'static str {
        match self {
            Mode::Dark => "dark",
            Mode::Graphite => "graphite",
        }
    }
    /// Its name for a person.
    pub fn label(self) -> &'static str {
        match self {
            Mode::Dark => "Dark",
            Mode::Graphite => "Graphite",
        }
    }
    /// The theme's mark in the status bar and its menu.
    pub fn icon(self) -> &'static str {
        match self {
            Mode::Dark => "moon",
            Mode::Graphite => "appearance",
        }
    }
    /// A name as the state file or an intent spells it; anything else
    /// (an older file's "light" or "system") is `None`.
    pub fn parse(s: &str) -> Option<Mode> {
        let s = s.trim().to_ascii_lowercase();
        Mode::ALL.into_iter().find(|m| m.name() == s)
    }
    /// The next theme in [`Mode::ALL`], round.
    pub fn next(self) -> Mode {
        let i = Mode::ALL.iter().position(|m| *m == self).unwrap_or(0);
        Mode::ALL[(i + 1) % Mode::ALL.len()]
    }
}

// The one place that says which palette is active: the shell sets it from
// the person's choice, at start, on the switch and at the top of every
// frame; every token read goes through it. Per thread: the UI is one
// thread, and tests that run side by side each have their own.
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

/// A new install's theme.
pub const DEFAULT_MODE: Mode = Mode::Dark;

/// How opaque the scrim under a dialog is, and the shadow under menus,
/// popovers and dialogs. Graphite's surfaces sit closer to each other in
/// lightness than the blue-black ones, so its shadow is a little denser.
pub fn scrim_alpha() -> f32 {
    0.6
}
pub fn shadow_alpha() -> f32 {
    match mode() {
        Mode::Dark => 0x73 as f32 / 255.0,
        Mode::Graphite => 0.55,
    }
}
/// The scrim's colour with its opacity.
pub fn scrim() -> Color {
    c("scrim").alpha(scrim_alpha())
}
/// The shadow's colour with its opacity.
pub fn shadow() -> Color {
    c("shadow").alpha(shadow_alpha())
}

/// The tones' colours for a LIGHT ground the app may still meet though no
/// theme is light: a terminal whose `terminal_color` setting is light (or
/// term-demo's `--theme`), where the keyword pills take the darker members
/// of each hue. Keyed by the tones' token names.
pub const ON_LIGHT_GROUND: &[(&str, u32)] = &[
    ("green", 0x1a7f37),
    ("red", 0xcf222e),
    ("amber", 0x9a6700),
    ("blue", 0x0969da),
    ("purple", 0x8250df),
    ("cyan", 0x1b7c83),
    ("pill-danger", 0xcf222e),
];

/// Tokens whose desktop value differs from app.css, with app.css's value:
/// each failed WCAG AA on a surface the desktop draws it on.
pub const WEB_ADJUSTED: &[(&str, u32)] = &[("text-muted", 0x8b949e), ("text-dim", 0x6e7681)];

/// Corner radii, the names screens use: `mark` 2 for the 8 pt identity
/// squares and hairline bars, `box` 4 for the small boxes inside a control
/// (a checkbox, a keycap), 6 for controls (buttons, rows, chips), 10 for
/// tiles, cards and menus, 12 for dialogs, `pill` for dots and chips. The
/// core reads the same values from [`corner`].
pub const RADII: &[(&str, f32)] = &[("mark", corner::MARK), ("box", corner::BOX), ("xs", corner::CONTROL), ("sm", corner::CONTROL), ("md", corner::TILE), ("lg", corner::DIALOG), ("pill", corner::PILL)];

/// [`RADII`] for the core, by role.
pub mod corner {
    pub const MARK: f32 = 2.0;
    pub const BOX: f32 = 4.0;

    pub const CONTROL: f32 = 6.0;
    pub const TILE: f32 = 10.0;
    pub const DIALOG: f32 = 12.0;
    pub const PILL: f32 = 999.0;
}

/// The type scale (pt). Nothing a person must read is under 12: `caption`
/// 12 (second lines, labels, badges, counts), `body` 13 (macOS's own body),
/// `title` 15 (row names, panel titles), `heading` 17 (dialog titles):
/// steps of 1.154 and 1.133 above the body; caption to body stays 1.083,
/// pinned by the 12 pt floor. The older names stay for screens written
/// against them: `xxs`, `xs` and `sm` are 12, `md` 13, `lg` 15, `xl` 17.
/// Hierarchy comes from weight (400, 500, 600) and colour, never from
/// capitals. The core reads the same values from [`font_size`].
pub const TEXT_SIZES: &[(&str, f32)] = &[
    ("caption", font_size::CAPTION),
    ("body", font_size::BODY),
    ("title", font_size::TITLE),
    ("heading", font_size::HEADING),
    ("xxs", font_size::CAPTION),
    ("xs", font_size::CAPTION),
    ("sm", font_size::CAPTION),
    ("md", font_size::BODY),
    ("lg", font_size::TITLE),
    ("xl", font_size::HEADING),
];

/// [`TEXT_SIZES`] for the core.
pub mod font_size {
    pub const CAPTION: f32 = 12.0;
    pub const BODY: f32 = 13.0;
    pub const TITLE: f32 = 15.0;
    pub const HEADING: f32 = 17.0;
    /// Every size the core may set text at (terminals and pictures aside).
    pub const ALL: &[f32] = &[CAPTION, BODY, TITLE, HEADING];
}

/// The chrome's icon sizes (pt), one optical system: `SMALL` inline with
/// 12 to 13 pt text (a row's chevron, a badge's mark, a file in a list),
/// `MEDIUM` every control (buttons, dense or not, the pane bar's actions,
/// the status bar, menus, headers, the palette), `LARGE` the rail's glyphs
/// (in 28 pt targets), `MARK` the top bar's `>_`, the app's signature, a
/// little larger than the rail and a little heavier, not by half. Screens
/// read them as `theme.icon.sm`, `md`, `lg`, `mark`.
pub mod icon {
    pub const SMALL: f32 = 12.0;
    pub const MEDIUM: f32 = 14.0;
    pub const LARGE: f32 = 20.0;
    pub const MARK: f32 = 24.0;
    /// Every size an icon may be drawn at.
    pub const ALL: &[f32] = &[SMALL, MEDIUM, LARGE, MARK];
}

/// Line height of wrapped text, a multiple of the size.
pub const LINE_HEIGHT: f32 = 1.4;

/// The spacing grid (4 pt): `xxs` 2 only between stacked lines of one
/// thing, then 4, 8, 12, 16, 24.
pub const SPACE: &[(&str, f32)] = &[("xxs", 2.0), ("xs", 4.0), ("sm", 8.0), ("md", 12.0), ("lg", 16.0), ("xl", 24.0)];

/// Every spacing value a built-in screen may write as a number.
pub const GRID: &[f32] = &[0.0, 2.0, 4.0, 8.0, 12.0, 16.0, 24.0];

/// [`SPACE`] for the core, plus the two insets inside a control that the
/// grid has no step for, named so they stay the only ones.
pub mod spacing {
    pub const XXS: f32 = 2.0;
    pub const XS: f32 = 4.0;
    pub const SM: f32 = 8.0;
    pub const MD: f32 = 12.0;
    pub const LG: f32 = 16.0;
    pub const XL: f32 = 24.0;
    /// An icon to its label inside a control (a button, a menu's face, a
    /// popover's line).
    pub const INLINE: f32 = 6.0;
    /// A badge's or a count's inset: 6 across, 1 above and below, so a
    /// 12 pt label sits in an 18 pt chip.
    pub const CHIP_X: f32 = 6.0;
    pub const CHIP_Y: f32 = 1.0;
}

/// Elevation, as Rune's shadow `(offset y, blur)` in the theme's
/// [`shadow`] colour: `RAISED` a switch's thumb, `FLOAT` the top bar's
/// notice, `OVERLAY` menus, popovers, the palette and dialogs. Surfaces
/// otherwise rise by lightness (`surface` over `bg`, `elevated` over
/// `surface`), never by a shadow.
pub mod elevation {
    pub const RAISED: (f32, f32) = (1.0, 3.0);
    pub const FLOAT: (f32, f32) = (2.0, 8.0);
    pub const OVERLAY: (f32, f32) = (8.0, 24.0);
}

/// `e` lifted to `level` ([`elevation`]).
pub fn lift(e: rune::El, level: (f32, f32)) -> rune::El {
    e.lift(level)
}

/// [`lift`] as a method, for a builder chain: `.lift(elevation::OVERLAY)`.
pub trait Lift {
    fn lift(self, level: (f32, f32)) -> Self;
}
impl Lift for rune::El {
    fn lift(self, level: (f32, f32)) -> Self {
        self.shadow(level.0, level.1, shadow())
    }
}


/// Motion: two durations (seconds), state and never decoration. `MICRO_IN`
/// and `MICRO_OUT` a fade (menus, popovers, the palette), `SHORT`
/// everything that moves (a fold, a switch's thumb, a picker's face, the
/// status bar's progress). `SPIN` is the busy ring's one turn. Under the
/// system's reduced motion every one of them lands at once.
pub mod motion {
    pub const MICRO_IN: f32 = 0.1;
    pub const MICRO_OUT: f32 = 0.08;
    pub const SHORT: f32 = 0.18;
    pub const SPIN: f32 = 0.9;
}

/// The keyboard focus ring: 2 pt in `focus`, drawn `OUT` outside a control
/// (the default), `NEAR` outside where neighbours sit close (the rail, a
/// field's box), `IN` inside a row that fills its list's width (menu rows).
pub mod focus {
    pub const W: f32 = 2.0;
    pub const OUT: f32 = 2.0;
    pub const NEAR: f32 = 1.0;
    pub const IN: f32 = -2.0;
}

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

/// A token's value in `m`'s palette (an [`ALIASES`] name reads its token).
pub fn color_in(m: Mode, name: &str) -> Option<u32> {
    let name = ALIASES.iter().find(|(old, _)| *old == name).map_or(name, |(_, to)| *to);
    m.palette().iter().find(|(n, _)| *n == name).map(|(_, v)| *v)
}

/// Retired colour names that still resolve, for screens written against
/// them (an override in `~/.sbxw/desktop/screens/` may name one): `text-dim`
/// was a second grey 1 to 4 % from `text-muted`, merged into it. Built-in
/// code never writes them (`retired_colour_names_stay_out_of_built_in_code`).
pub const ALIASES: &[(&str, &str)] = &[("text-dim", "text-muted")];

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
    let mut colors: serde_json::Map<_, _> = mode().palette().iter().map(|(n, v)| (n.to_string(), format!("#{v:06x}").into())).collect();
    for (old, _) in ALIASES {
        colors.insert(old.to_string(), format!("#{:06x}", color_value(old).unwrap()).into());
    }
    let nums = |t: &[(&str, f32)]| -> serde_json::Map<String, serde_json::Value> { t.iter().map(|(n, v)| (n.to_string(), (*v as f64).into())).collect() };
    serde_json::json!({
        "mode": mode().name(),
        "color": colors,
        "radius": nums(RADII),
        "text": nums(TEXT_SIZES),
        "space": nums(SPACE),
        "line_height": LINE_HEIGHT,
        "icon": { "sm": icon::SMALL, "md": icon::MEDIUM, "lg": icon::LARGE, "mark": icon::MARK },
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
    as_prose(crate::screen::body(size.max(font_size::CAPTION), weight))
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

/// Graphite: the dark theme's hues and steps on a grey a little lighter
/// than the blue-black: the colour a step lighter (66%, then 74, 81) so its
/// square still marks at 3:1 on its block, the current tab's tint 22% (the
/// orange focus ring keeps 3:1 on the yellow tabs).
pub const GROUP_GRAPHITE: GroupLook = GroupLook { lightness: &[0.66, 0.74, 0.81], chat: (0.60, 0.70), tab_current: 0.22, ..GROUP_DARK };

/// The active theme's [`GroupLook`].
pub fn group_look() -> GroupLook {
    look_of(mode())
}
pub fn look_of(m: Mode) -> GroupLook {
    match m {
        Mode::Dark => GROUP_DARK,
        Mode::Graphite => GROUP_GRAPHITE,
    }
}

/// A group's tints as the list draws them: its block, the header hovered,
/// and the label's colour (sandboxes.luau reads these as `app.group_look`).
pub fn group_tints(g: u32) -> (u32, u32, u32) {
    let look = group_look();
    let surface = color_value("surface").unwrap();
    // Toward white, as the web UI.
    let to = WHITE;
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

/// A tone's icon on a tinted ground: the tone, moved toward white (black on
/// a light ground) until it marks at 3:1 there (red on the lightest yellow
/// tint needs it).
pub fn mark_on(fg: u32, ground: u32) -> u32 {
    let to = if luminance(ground) > 0.18 { BLACK } else { WHITE };
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
    // `text-muted` above also covers what `text-dim` drew before the merge:
    // placeholders in inputs (bg) and the palette's field (surface), and a
    // selected or hovered row's second line (a Kanban card's age and
    // sandbox, a Board team's time, a kit's description, a Monitor row).
    ("text-muted", "floor", Need::Text),
    ("text-muted", "pane-bar", Need::Text),
    ("text-soft", "active-bg", Need::Text),
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
    // The explorer's file kinds, on its ground, a row's cursor and the selected row.
    ("file-doc", "bg", Need::Mark),
    ("file-code", "bg", Need::Mark),
    ("file-image", "bg", Need::Mark),
    ("file-data", "bg", Need::Mark),
    ("file-archive", "bg", Need::Mark),
    ("file-doc", "active-bg", Need::Mark),
    ("file-code", "active-bg", Need::Mark),
    ("file-image", "active-bg", Need::Mark),
    ("file-data", "active-bg", Need::Mark),
    ("file-archive", "active-bg", Need::Mark),
    ("accent", "active-bg", Need::Mark),
    ("blue", "active-bg", Need::Mark),
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
    // A chat input's ground (`compose`): typed words, the placeholder
    // (`text-muted`), its edge
    // (`compose-edge`) and the focus ring on it (disabled, it is drawn on `bg` at
    // DISABLED_OPACITY: `text-bright` on `bg`, Disabled, below).
    ("text-bright", "compose", Need::Text),
    ("text-muted", "compose", Need::Text),
    ("compose-edge", "compose", Need::Mark),
    ("focus", "compose", Need::Mark),
    // The notice: its text, its offer ("Click to open"), the × and the
    // kind's edge and icon on its ground.
    ("text-bright", "notice-info", Need::Text),
    ("text-bright", "notice-ask", Need::Text),
    ("text-bright", "notice-error", Need::Text),
    ("text", "bubble-agent", Need::Text),
    ("text-bright", "bubble-agent", Need::Text),
    ("text-muted", "bubble-agent", Need::Text),
    ("text", "bubble-you", Need::Text),
    ("text-bright", "bubble-you", Need::Text),
    ("text-muted", "bubble-you", Need::Text),
    ("text", "notice-info", Need::Mark),
    ("text", "notice-ask", Need::Mark),
    ("text", "notice-error", Need::Mark),
    ("blue", "notice-info", Need::Mark),
    ("amber", "notice-ask", Need::Mark),
    ("red", "notice-error", Need::Mark),
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

/// How far a chat input's ground (`compose`) steps from `bg` and from
/// `surface`, as a luminance ratio: enough to read in a squint on a shot
/// (a card over a panel steps about 1.1 here).
pub const COMPOSE_STEP_BG: f64 = 1.3;
pub const COMPOSE_STEP_SURFACE: f64 = 1.15;
/// And from `elevated` and from the conversation's tinted bubbles
/// (`bubble-agent`, `bubble-you`): a field is never taken for a message.
pub const COMPOSE_STEP_ELEVATED: f64 = 1.1;

/// How far a notice's ground steps from the top bar's `surface`, and how far
/// above the views' card tint of the same kind (`blue-tint`...) it sits.
pub const NOTICE_STEP: f64 = 1.4;
pub const NOTICE_OVER_TINT: f64 = 1.2;

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

    /// Every icon the built-in screens and the core draw is one of the
    /// sizes of [`icon`]: a source scan of `icon{ ..., size = N }` in the
    /// screens and of `rune::icon(.., N)` in the core (named constants are
    /// the scale's own). approval.rs is the one exception (its dialogs' ×
    /// at 14, listed: that file's layout is not this change's).
    #[test]
    fn every_icon_is_on_the_scale() {
        let mut off = vec![];
        for (name, src) in crate::screen::all_builtin() {
            for (i, line) in src.lines().enumerate() {
                let mut rest = line;
                while let Some(at) = rest.find("icon{") {
                    rest = &rest[at + 5..];
                    let end = rest.find('}').unwrap_or(rest.len());
                    let body = &rest[..end];
                    if let Some(p) = body.find("size = ") {
                        let num: String = body[p + 7..].chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
                        if let Ok(v) = num.parse::<f32>() {
                            if !icon::ALL.contains(&v) {
                                off.push(format!("{name}.luau:{}: size = {num}", i + 1));
                            }
                        }
                    }
                }
            }
        }
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut stack = vec![root.clone()];
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(&d).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                    continue;
                }
                let rel = p.strip_prefix(&root).unwrap().to_string_lossy().to_string();
                if !rel.ends_with(".rs") || rel.contains("tests") || rel == "approval.rs" || rel.starts_with("bin/") {
                    continue;
                }
                let src = std::fs::read_to_string(&p).unwrap();
                for (i, line) in src.split("#[cfg(test)]").next().unwrap().lines().enumerate() {
                    let mut rest = line;
                    while let Some(at) = rest.find("rune::icon(") {
                        rest = &rest[at + 11..];
                        let end = rest.find(')').map(|e| e + 1).unwrap_or(rest.len());
                        let args = &rest[..end];
                        let last = args.rsplit(',').next().unwrap_or("").trim().trim_end_matches(')');
                        if let Ok(v) = last.trim_end_matches(".0").parse::<f32>() {
                            if !icon::ALL.contains(&v) {
                                off.push(format!("src/{rel}:{}: {}", i + 1, line.trim()));
                            }
                        }
                    }
                }
            }
        }
        assert!(off.is_empty(), "icons off the scale (12, 14, 20, 24): {off:#?}");
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
        // In every theme: every hue the hash can give, at every step, marks
        // at 3:1 on the surfaces it is drawn on.
        for m in MODES {
            with_mode(m, || {
            let look = group_look();
            for &l in look.lightness {
                for h in 0..360 {
                    for ground in ["surface", "surface-alt"] {
                        let g = hsl(h as f32, look.saturation, l);
                        assert!(contrast(g, color_value(ground).unwrap()) >= 3.0, "{m:?}: hue {h} at {l} on {ground}");
                    }
                }
            }
            assert!(contrast(hsl(265.0, look.chat.0, look.chat.1), color_value("surface").unwrap()) >= 3.0);
            });
        }
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
            // A sandbox with several sessions in its group (sandboxes.luau): its bar, the group's colour, on the tint.
            note("a sandbox's session bar on its group's tint (mark)", contrast(g, tint), 3.0);
            note("group label on its tint (text)", contrast(label, tint), 4.5);
            note("group label on its hover tint (text)", contrast(label, hover), 4.5);
            note("group square on its tint (mark)", contrast(g, tint), 3.0);
            note("`text-muted` chevron on the hover tint (mark)", contrast(color_value("text-muted").unwrap(), hover), 3.0);
            // The bubble's number on its fill, and the selected row over a tint (opaque).
            note("`bg` on `blue` (the bubble)", contrast(color_value("bg").unwrap(), color_value("blue").unwrap()), 4.5);
            note("`bg` on `amber` (the bubble)", contrast(color_value("bg").unwrap(), color_value("amber").unwrap()), 4.5);
        }
        // An ungrouped sandbox with several sessions and no colour: its block is `surface-alt`.
        let alt = color_value("surface-alt").unwrap();
        for t in ["text-bright", "text", "text-muted", "amber", "blue", "red-text", "vendor-anthropic", "vendor-openai", "vendor-google"] {
            note(&format!("`{t}` on the neutral session block (text)"), contrast(color_value(t).unwrap(), alt), 4.5);
        }
        for m in ["green", "blue", "amber", "red", "text-muted"] {
            note(&format!("`{m}` on the neutral session block (rail)"), contrast(color_value(m).unwrap(), alt), 3.0);
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
        assert_eq!(with_mode(Mode::Graphite, || parse_color("text-muted")), Ok(color_in(Mode::Graphite, "text-muted").unwrap()));
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

    const MODES: [Mode; 2] = Mode::ALL;

    /// Both palettes name the same tokens, in the same order, each once.
    #[test]
    fn every_token_is_in_both_palettes() {
        fn names(p: &'static [(&'static str, u32)]) -> Vec<&'static str> {
            p.iter().map(|(n, _)| *n).collect()
        }
        for m in Mode::ALL {
            assert_eq!(names(DARK), names(m.palette()), "{m:?}");
        }
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
        assert_eq!(with_mode(Mode::Graphite, || c("bg")), hex(color_in(Mode::Graphite, "bg").unwrap()));
        assert_ne!(with_mode(Mode::Graphite, || c("bg")), with_mode(Mode::Dark, || c("bg")));
        assert_eq!(with_mode(Mode::Dark, || c("bg")), hex(0x0d1117));
        let nested = with_mode(Mode::Graphite, || with_mode(Mode::Dark, || mode()) == Mode::Dark && mode() == Mode::Graphite);
        assert!(nested);
        assert_eq!(Mode::parse("Graphite"), Some(Mode::Graphite));
        assert_eq!(Mode::parse("light"), None, "an older state file's light falls back");
        assert_eq!(Mode::parse("system"), None);
        assert_eq!(Mode::Dark.next(), Mode::Graphite);
        assert_eq!(Mode::Graphite.next(), Mode::Dark);
        assert_eq!(with_mode(Mode::Graphite, || to_json()["mode"].clone()), "graphite");
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

    /// A chat input's ground reads apart from the panels it sits in at rest,
    /// without its focus ring: lighter than `bg` (the views' and panels'
    /// ground) by a luminance ratio of [`COMPOSE_STEP_BG`] and than `surface`
    /// (a request's card) by [`COMPOSE_STEP_SURFACE`] and than `elevated` (a
    /// message bubble) by [`COMPOSE_STEP_ELEVATED`], in both themes, while
    /// its text, placeholder, edge and focus ring keep their own ratios
    /// (`PAIRS`).
    #[test]
    fn compose_stands_out() {
        for m in MODES {
            let v = |n| color_in(m, n).unwrap();
            let (comp, bg, surface) = (v("compose"), v("bg"), v("surface"));
            assert!(luminance(comp) > luminance(surface) && luminance(surface) > luminance(bg), "{m:?}: raised, as surfaces rise");
            let (sb, ss) = (contrast(comp, bg), contrast(comp, surface));
            println!("{m:?}: compose #{comp:06x} {sb:.2}:1 on bg, {ss:.2}:1 on surface");
            assert!(sb >= COMPOSE_STEP_BG, "{m:?}: {sb:.2} on bg");
            assert!(ss >= COMPOSE_STEP_SURFACE, "{m:?}: {ss:.2} on surface");
            let se = contrast(comp, v("elevated"));
            assert!(luminance(comp) > luminance(v("elevated")) && se >= COMPOSE_STEP_ELEVATED, "{m:?}: {se:.2} from a message bubble");
            for b in ["bubble-agent", "bubble-you"] {
                let sb = contrast(comp, v(b));
                println!("{m:?}: compose {sb:.2}:1 from {b}");
                assert!(luminance(comp) > luminance(v(b)) && sb >= COMPOSE_STEP_ELEVATED, "{m:?}: {sb:.2} from {b}");
            }
            for (fg, need) in [("text-bright", 4.5), ("text-muted", 4.5), ("compose-edge", 3.0), ("focus", 3.0)] {
                assert!(contrast(v(fg), comp) >= need, "{m:?}: {fg} on compose");
            }
        }
    }

    /// The notice stands out from the bar it sits in, in both themes.
    #[test]
    fn notice_stands_out_from_the_bar() {
        for m in MODES {
            let s = color_in(m, "surface").unwrap();
            for (kind, hue, tint) in [("notice-info", "blue", "blue-tint"), ("notice-ask", "amber", "amber-tint"), ("notice-error", "red", "red-tint")] {
                let n = color_in(m, kind).unwrap();
                assert!(luminance(n) > luminance(s), "{m:?} {kind}: raised");
                assert!(contrast(n, s) >= NOTICE_STEP, "{m:?} {kind}: {:.2} on the bar", contrast(n, s));
                assert!(contrast(n, color_in(m, tint).unwrap()) >= NOTICE_OVER_TINT, "{m:?} {kind}: too close to the cards' {tint}");
                // Its hue is the kind's (within 25 degrees), and coloured.
                let ((h, sat, _), (kh, _, _)) = (to_hsl(n), to_hsl(color_in(m, hue).unwrap()));
                assert!(hue_dist(h, kh) <= 25.0 && sat >= 0.25, "{m:?} {kind}: hue {h:.0} against {kh:.0}, saturation {sat:.2}");
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

    /// Sizes go through the tokens as colours do: in the core, no text size,
    /// radius, shadow or motion duration written as a number (the type scale,
    /// [`corner`], [`elevation`], [`motion`]); in the screens, no radius as a
    /// number. Excepted, with why: the canvas's zoom-scaled marks (a radius
    /// times the zoom), tests, fixtures and tools.
    #[test]
    fn core_sizes_go_through_the_tokens() {
        const ALLOWED: &[(&str, &str)] = &[

            ("src/surface_canvas.rs", "radius(N * inv): marks drawn at the canvas's zoom"),
        ];
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let num = |rest: &str| rest.trim_start().chars().next().is_some_and(|c| c.is_ascii_digit() || c == '-');
        let mut found = vec![];
        let mut stack = vec![root.join("src"), root.join("screens")];
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(&d).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                    continue;
                }
                let rel = p.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/");
                let name = p.file_name().unwrap().to_string_lossy().to_string();
                if rel == "src/theme.rs" || rel.starts_with("src/bin/") || name.contains("tests") || ALLOWED.iter().any(|(f, _)| *f == rel) {
                    continue;
                }
                let src = std::fs::read_to_string(&p).unwrap();
                let (calls, comment): (&[&str], &str) = if rel.ends_with(".luau") {
                    (&["radius = "], "--")
                } else if rel.ends_with(".rs") {
                    (&["body(", "prose(", "TextStyle::new(", ".radius(", ".shadow(", "Spring::new(", "rotation("], "//")
                } else {
                    continue;
                };
                let body = src.split("#[cfg(test)]\nmod tests").next().unwrap();
                for (n, line) in body.lines().enumerate() {
                    let code = line.split(comment).next().unwrap();
                    for call in calls {
                        let mut rest = code;
                        while let Some(at) = rest.find(call) {
                            let before = rest[..at].chars().last();
                            rest = &rest[at + call.len()..];
                            let word = !call.starts_with('.') && before.is_some_and(|c| c.is_alphanumeric() || c == '_');
                            if !word && num(rest) {
                                found.push(format!("{rel}:{}: {}", n + 1, line.trim()));
                            }
                        }
                    }
                }
            }
        }
        assert!(found.is_empty(), "sizes written as numbers outside theme.rs:\n{}", found.join("\n"));
        // Every text size the core may set is on the scale, and the scale's
        // steps above the body are 1.125 to 1.2.
        for w in font_size::ALL.windows(2).skip(1) {
            let r = w[1] / w[0];
            assert!((1.125..=1.2).contains(&r), "type step {} -> {}: {r:.3}", w[0], w[1]);
        }
    }

    /// A retired colour name ([`ALIASES`]) resolves to its token, in the
    /// core and in `theme.color`, and no built-in code writes it.
    #[test]
    fn retired_colour_names_stay_out_of_built_in_code() {
        for m in Mode::ALL {
            for (old, to) in ALIASES {
                assert!(!m.palette().iter().any(|(n, _)| n == old), "{old} is in the {m:?} palette");
                assert_eq!(color_in(m, old), color_in(m, to), "{m:?}: {old}");
            }
        }
        assert_eq!(to_json()["color"]["text-dim"], to_json()["color"]["text-muted"]);
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut found = vec![];
        let mut stack = vec![root.join("src"), root.join("screens")];
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(&d).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                    continue;
                }
                let rel = p.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/");
                if rel == "src/theme.rs" {
                    continue;
                }
                let Ok(src) = std::fs::read_to_string(&p) else { continue };
                for (old, _) in ALIASES {
                    for q in [format!("\"{old}\""), format!("'{old}'")] {
                        if src.contains(&q) {
                            found.push(format!("{rel}: {old}"));
                        }
                    }
                }
            }
        }
        assert!(found.is_empty(), "retired colour names in built-in code:\n{}", found.join("\n"));
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
