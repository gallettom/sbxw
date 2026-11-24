//! The terminal's colours, passed in by the caller: the web UI's defaults
//! (`assets/js/term-color.js`, `termTheme()`, and xterm.js's own ANSI
//! palette, which the web UI does not override), so sbxw's `terminal_color`
//! and `terminal_text_color` settings can apply here the same way.

/// Colours as 0xRRGGBB.
#[derive(Clone, Debug, PartialEq)]
pub struct Theme {
    pub background: u32,
    pub foreground: u32,
    pub cursor: u32,
    /// The selection's colour, drawn over the cells' backgrounds at `selection_alpha`.
    pub selection: u32,
    pub selection_alpha: f32,
    /// ANSI 0-15.
    pub palette: [u32; 16],
}

/// xterm.js's default ANSI colours (its `DEFAULT_ANSI_COLORS`, Tango), which
/// the web UI's panes use: `termTheme()` sets only the background, the
/// foreground, the cursor and the selection.
pub const XTERM_PALETTE: [u32; 16] = [
    0x2e3436, 0xcc0000, 0x4e9a06, 0xc4a000, 0x3465a4, 0x75507b, 0x06989a, 0xd3d7cf, //
    0x555753, 0xef2929, 0x8ae234, 0xfce94f, 0x729fcf, 0xad7fa8, 0x34e2e2, 0xeeeeec,
];

/// The web UI's built-in terminal background (`--c-bg`, "Midnight").
pub const WEB_BACKGROUND: u32 = 0x0d1117;
/// Its text (`--c-text`).
pub const WEB_FOREGROUND: u32 = 0xc9d1d9;

/// Graphite's ANSI colours: xterm's set (as the dark theme's) on the grey
/// ground [`GRAPHITE_BACKGROUND`], with what fell under 4.5:1 there lifted
/// (red, blue, magenta) and bright black lifted to 3:1; black stays the
/// ground's own dark (it is the background role, never text on it).
/// Tested in `graphite_ansi_reads_on_its_ground`.
pub const GRAPHITE_PALETTE: [u32; 16] = [
    0x2e3436, 0xf0544b, 0x4e9a06, 0xc4a000, 0x5e93d9, 0xb07fb5, 0x06989a, 0xd3d7cf, //
    0x707478, 0xff7b72, 0x8ae234, 0xfce94f, 0x8ab4f0, 0xd4a2cf, 0x34e2e2, 0xeeeeec,
];

/// Graphite's terminal ground and text: the palette's `bg` and `text`.
pub const GRAPHITE_BACKGROUND: u32 = 0x202124;
pub const GRAPHITE_FOREGROUND: u32 = 0xdadce0;

/// The terminal in Graphite: its ground and text, the orange accent as the
/// cursor, the selection the dark theme's blue (a selection is not an
/// action).
pub const GRAPHITE: Theme = Theme {
    background: GRAPHITE_BACKGROUND,
    foreground: GRAPHITE_FOREGROUND,
    cursor: 0xe8833a,
    selection: 0x58a6ff,
    selection_alpha: 0x4d as f32 / 255.0,
    palette: GRAPHITE_PALETTE,
};

/// The terminal's theme for one of the app's themes (the dark one is the
/// web UI's default, unchanged), with the person's terminal colours
/// ([`chosen`]) over it.
pub fn for_mode(m: crate::theme::Mode) -> Theme {
    chosen().theme(builtin_for_mode(m))
}

/// The app theme's own terminal colours, before any choice.
pub fn builtin_for_mode(m: crate::theme::Mode) -> Theme {
    match m {
        crate::theme::Mode::Dark => Theme::default(),
        crate::theme::Mode::Graphite => GRAPHITE,
    }
}

// The terminal colours picker (the web UI's `assets/js/term-color.js`,
// `TERM_COLOR_SLOTS`): three slots, each a list of presets and a custom
// colour. The lists are the web UI's, value for value
// (`the_presets_are_the_web_uis` reads that file).

/// The terminal's background.
pub const BACKGROUND_PRESETS: &[(&str, &str)] = &[
    ("#0d1117", "Midnight (built-in)"),
    ("#000000", "Black"),
    ("#1a1b26", "Night"),
    ("#002b36", "Solarized"),
    ("#0e1f17", "Phosphor"),
    ("#1f1530", "Aubergine"),
    ("#2a1d10", "Amber"),
    ("#fdf6e3", "Paper"),
];

/// The pane bar: dark on purpose, its labels and buttons are light text.
pub const BAR_PRESETS: &[(&str, &str)] = &[
    ("#2f4f4f", "Slate (built-in)"),
    ("#21262d", "Graphite"),
    ("#1c2f4a", "Navy"),
    ("#163a3a", "Teal"),
    ("#2b3a1f", "Moss"),
    ("#3a2b4f", "Plum"),
    ("#4a2a2a", "Oxblood"),
    ("#4a3a1a", "Ochre"),
];

/// The text: `auto` follows the background's lightness.
pub const TEXT_PRESETS: &[(&str, &str)] = &[
    ("auto", "Auto (follows the background)"),
    ("#ffffff", "White"),
    ("#c9d1d9", "Silver"),
    ("#8b949e", "Grey"),
    ("#e6d5b8", "Cream"),
    ("#ffb000", "Amber"),
    ("#33ff66", "Phosphor"),
    ("#7dcfff", "Ice"),
];

/// One of the picker's three slots.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Slot {
    Background,
    Bar,
    Text,
}

impl Slot {
    pub const ALL: [Slot; 3] = [Slot::Background, Slot::Bar, Slot::Text];

    /// Its key in the app's state file and in the daemon's
    /// `terminal_colors` (`GET /api/state`).
    pub fn key(self) -> &'static str {
        match self {
            Slot::Background => "background",
            Slot::Bar => "bar",
            Slot::Text => "text",
        }
    }

    /// Its heading in the picker (the web UI's zone names).
    pub fn heading(self) -> &'static str {
        match self {
            Slot::Background => "Background",
            Slot::Bar => "Top bar",
            Slot::Text => "Text",
        }
    }

    pub fn presets(self) -> &'static [(&'static str, &'static str)] {
        match self {
            Slot::Background => BACKGROUND_PRESETS,
            Slot::Bar => BAR_PRESETS,
            Slot::Text => TEXT_PRESETS,
        }
    }

    /// The web UI's built-in for the slot (its first preset). The app keeps
    /// its theme's own colours when nothing is picked and sbxw.toml says
    /// nothing.
    pub fn builtin(self) -> &'static str {
        self.presets()[0].0
    }

    /// A value of this slot, as spelled in sbxw.toml or typed: `#rrggbb`
    /// (any case), and `auto` for the text. Anything else is no colour.
    pub fn parse(self, s: &str) -> Option<Pick> {
        let s = s.trim().to_ascii_lowercase();
        if self == Slot::Text && s == "auto" {
            return Some(Pick::Auto);
        }
        let h = s.strip_prefix('#')?;
        (h.len() == 6 && h.bytes().all(|b| b.is_ascii_hexdigit())).then(|| Pick::Color(u32::from_str_radix(h, 16).unwrap()))
    }

    /// The preset's name for a value, if it is one.
    pub fn preset_name(self, p: Pick) -> Option<&'static str> {
        self.presets().iter().find(|(v, _)| self.parse(v) == Some(p)).map(|(_, n)| *n)
    }
}

/// A slot's value: a colour, or (the text only) Auto.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pick {
    Color(u32),
    Auto,
}

impl Pick {
    /// As sbxw.toml spells it.
    pub fn spelled(self) -> String {
        match self {
            Pick::Color(c) => format!("#{c:06x}"),
            Pick::Auto => "auto".into(),
        }
    }
}

/// What the terminals and the pane bars use now, per slot: the value that
/// won (the person's pick, else sbxw.toml's), `None` for the app theme's own.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Chosen {
    pub background: Option<u32>,
    pub bar: Option<u32>,
    pub text: Option<Pick>,
}

impl Chosen {
    /// `base` (the app theme's terminal) with these colours over it. A
    /// background makes the theme the web UI's for it ([`Theme::from_settings`]:
    /// a light one takes the light cursor and selection, and the pills the
    /// light ground's tones through [`crate::emphasis`], the path sbxw.toml's
    /// light `terminal_color` takes); a text colour alone replaces the
    /// foreground and ANSI white and bright white of `base`.
    pub fn theme(&self, base: Theme) -> Theme {
        let text = match self.text {
            Some(Pick::Color(t)) => Some(t),
            _ => None,
        };
        match (self.background, text) {
            (Some(bg), text) => Theme::from_settings(bg, text),
            (None, Some(t)) => {
                let mut th = base;
                th.foreground = t;
                th.palette[7] = t;
                th.palette[15] = t;
                th
            }
            (None, None) => base,
        }
    }
}

thread_local! {
    // The UI is one thread (every window's terminals read it there); tests
    // that run side by side each have their own.
    static CHOSEN: std::cell::Cell<Chosen> = const { std::cell::Cell::new(Chosen { background: None, bar: None, text: None }) };
    static CHOSEN_GENERATION: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// The terminal colours in force.
pub fn chosen() -> Chosen {
    CHOSEN.with(|c| c.get())
}

/// Puts `c` in force; true when it changed. Every terminal follows on its
/// next poll ([`for_mode`]), in every window.
pub fn set_chosen(c: Chosen) -> bool {
    let was = CHOSEN.with(|x| x.replace(c));
    if was != c {
        CHOSEN_GENERATION.with(|g| g.set(g.get() + 1));
    }
    was != c
}

/// Counts the changes of [`chosen`] (what caches a bar can tell it is stale).
pub fn chosen_generation() -> u64 {
    CHOSEN_GENERATION.with(|g| g.get())
}

impl Default for Theme {
    fn default() -> Self {
        Theme::from_settings(WEB_BACKGROUND, None)
    }
}

impl Theme {
    /// The theme the web UI computes from its two settings: the background
    /// (`terminal_color`) and the text (`terminal_text_color`, `None` for
    /// `auto`, which follows the background's lightness). A picked text
    /// colour also replaces ANSI white and bright white, as in the web UI.
    pub fn from_settings(background: u32, text: Option<u32>) -> Theme {
        let light = is_light(background);
        let foreground = text.unwrap_or(if light { 0x24292f } else { 0xc9d1d9 });
        let mut palette = XTERM_PALETTE;
        if let Some(t) = text {
            palette[7] = t;
            palette[15] = t;
        }
        Theme {
            background,
            foreground,
            cursor: if light { 0x0969da } else { 0x58a6ff },
            selection: if light { 0x0969da } else { 0x58a6ff },
            // '#0969da40' and '#58a6ff4d' in the web UI.
            selection_alpha: if light { 0x40 as f32 / 255.0 } else { 0x4d as f32 / 255.0 },
            palette,
        }
    }

    /// Parses the settings as sbxw.toml spells them: `#rrggbb`, and `auto`
    /// (or empty) for the text. Anything else falls back to the default.
    pub fn from_setting_strings(background: &str, text: &str) -> Theme {
        let bg = parse_hex(background).unwrap_or(WEB_BACKGROUND);
        Theme::from_settings(bg, parse_hex(text))
    }

    /// The colour of an xterm 256-colour index.
    pub fn indexed(&self, i: u8) -> u32 {
        match i {
            0..=15 => self.palette[i as usize],
            16..=231 => {
                let i = i - 16;
                let v = |x: u8| if x == 0 { 0 } else { 55 + x as u32 * 40 };
                v(i / 36) << 16 | v((i / 6) % 6) << 8 | v(i % 6)
            }
            _ => {
                let g = 8 + (i as u32 - 232) * 10;
                g << 16 | g << 8 | g
            }
        }
    }

    /// The dim variant of a colour (SGR 2): two thirds of the way to the background.
    pub fn dim(&self, c: u32) -> u32 {
        mix(c, self.background, 0.34)
    }
}

pub fn parse_hex(s: &str) -> Option<u32> {
    let h = s.trim().strip_prefix('#')?;
    (h.len() == 6).then(|| u32::from_str_radix(h, 16).ok()).flatten()
}

/// WCAG relative luminance above 0.4, as `isLightColor` in term-color.js.
pub fn is_light(c: u32) -> bool {
    let lin = |v: u32| {
        let v = v as f64 / 255.0;
        if v <= 0.03928 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * lin(c >> 16 & 0xff) + 0.7152 * lin(c >> 8 & 0xff) + 0.0722 * lin(c & 0xff) > 0.4
}

/// `a` moved toward `b` by `t` (0 keeps `a`).
pub fn mix(a: u32, b: u32, t: f32) -> u32 {
    let ch = |s: u32| {
        let (x, y) = ((a >> s & 0xff) as f32, (b >> s & 0xff) as f32);
        ((x + (y - x) * t).round() as u32) << s
    };
    ch(16) | ch(8) | ch(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every ANSI colour of Graphite reads on its ground: the normal eight
    /// (black apart: the background role) as text at 4.5:1, the bright
    /// eight at 3:1; the text, the dim text, the cursor and the selection
    /// too. Prints the swatch table.
    #[test]
    fn graphite_ansi_reads_on_its_ground() {
        use crate::theme::contrast;
        let t = GRAPHITE;
        let mut bad = vec![];
        println!("| ANSI | Colour | On #{:06x} | Needs |\n|---|---|---|---|", t.background);
        for (i, c) in t.palette.iter().enumerate() {
            let need = match i {
                0 => 1.0,
                1..=7 => 4.5,
                _ => 3.0,
            };
            let r = contrast(*c, t.background);
            println!("| {i} | #{c:06x} | {r:.2} | {need} |");
            if r < need {
                bad.push(format!("ANSI {i} #{c:06x}: {r:.2} < {need}"));
            }
        }
        for (what, c, need) in [("text", t.foreground, 4.5), ("dim text", t.dim(t.foreground), 3.0), ("cursor", t.cursor, 3.0)] {
            let r = contrast(c, t.background);
            println!("| {what} | #{c:06x} | {r:.2} | {need} |");
            if r < need {
                bad.push(format!("{what}: {r:.2}"));
            }
        }
        // The selection is seen, and the text under it still reads.
        let sel = crate::theme::over(t.selection, t.selection_alpha, t.background);
        assert!(contrast(sel, t.background) >= 1.3, "selection {:.2}", contrast(sel, t.background));
        assert!(contrast(t.foreground, sel) >= 4.5);
        assert!(bad.is_empty(), "{bad:#?}");
        // The ground and the text are the palette's.
        let m = crate::theme::Mode::Graphite;
        assert_eq!(crate::theme::color_in(m, "bg"), Some(GRAPHITE_BACKGROUND));
        assert_eq!(crate::theme::color_in(m, "text"), Some(GRAPHITE_FOREGROUND));
        assert_eq!(crate::theme::color_in(m, "accent"), Some(t.cursor));
        assert!(!is_light(GRAPHITE_BACKGROUND));
    }

    /// The three slots' presets equal the web UI's (`TERM_COLOR_SLOTS` in
    /// assets/js/term-color.js), value, name and order.
    #[test]
    fn the_presets_are_the_web_uis() {
        let js = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../assets/js/term-color.js"));
        for (slot, block) in [(Slot::Background, "  term: {"), (Slot::Bar, "  bar: {"), (Slot::Text, "  text: {")] {
            let from = js.find(block).unwrap_or_else(|| panic!("{block} in term-color.js"));
            let rest = &js[from..];
            let body = &rest[rest.find("presets: [").unwrap()..rest.find("],\n  },").unwrap()];
            let web: Vec<(String, String)> = body
                .lines()
                .filter_map(|l| {
                    let l = l.trim().strip_prefix("['")?.strip_suffix("'],")?;
                    let (v, n) = l.split_once("', '")?;
                    Some((v.to_string(), n.to_string()))
                })
                .collect();
            let ours: Vec<(String, String)> = slot.presets().iter().map(|(v, n)| (v.to_string(), n.to_string())).collect();
            assert_eq!(ours, web, "{slot:?}");
            assert_eq!(ours.len(), 8);
            let b = &rest[rest.find("builtin: '").unwrap() + 10..];
            assert_eq!(&b[..b.find('\'').unwrap()], slot.builtin(), "{slot:?}'s built-in");
        }
    }

    /// Auto text reads at 4.5:1 on every background preset.
    #[test]
    fn auto_text_reads_on_every_background() {
        use crate::theme::contrast;
        for (v, name) in BACKGROUND_PRESETS {
            let Some(Pick::Color(bg)) = Slot::Background.parse(v) else { panic!("{v}") };
            let t = Chosen { background: Some(bg), bar: None, text: Some(Pick::Auto) }.theme(Theme::default());
            let r = contrast(t.foreground, bg);
            assert!(r >= 4.5, "{name}: {r:.2}");
            assert!(contrast(t.cursor, bg) >= 3.0, "{name}'s cursor");
        }
    }

    #[test]
    fn a_slot_takes_only_its_values() {
        assert_eq!(Slot::Background.parse("#1A1b26"), Some(Pick::Color(0x1a1b26)));
        assert_eq!(Slot::Text.parse(" AUTO "), Some(Pick::Auto));
        for bad in ["auto", "1a1b26", "#1a1b2", "#1a1b266", "#gggggg", "red", "", "#12 456", "#+1a1b2"] {
            assert_eq!(Slot::Background.parse(bad), None, "{bad}");
        }
        assert_eq!(Slot::Text.parse("#ffb000").map(Pick::spelled).as_deref(), Some("#ffb000"));
        assert_eq!(Slot::Background.preset_name(Pick::Color(0xfdf6e3)), Some("Paper"));
        // Nothing chosen: the app theme's own, Graphite included.
        assert_eq!(Chosen::default().theme(GRAPHITE), GRAPHITE);
        // A text colour alone keeps the app theme's ground.
        let t = Chosen { text: Some(Pick::Color(0xffb000)), ..Chosen::default() }.theme(GRAPHITE);
        assert_eq!((t.background, t.foreground, t.palette[7], t.palette[15]), (GRAPHITE_BACKGROUND, 0xffb000, 0xffb000, 0xffb000));
    }

    #[test]
    fn web_defaults() {
        let t = Theme::default();
        assert_eq!((t.background, t.foreground, t.cursor), (0x0d1117, 0xc9d1d9, 0x58a6ff));
        let paper = Theme::from_setting_strings("#fdf6e3", "auto");
        assert_eq!(paper.foreground, 0x24292f);
        let amber = Theme::from_setting_strings("#000000", "#ffb000");
        assert_eq!((amber.foreground, amber.palette[7], amber.palette[15]), (0xffb000, 0xffb000, 0xffb000));
        assert_eq!(Theme::from_setting_strings("nope", "").background, WEB_BACKGROUND);
        assert_eq!(t.indexed(208), 0xff8700);
        assert_eq!(t.indexed(244), 0x808080);
    }
}
