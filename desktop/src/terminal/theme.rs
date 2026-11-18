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
/// web UI's default, unchanged).
pub fn for_mode(m: crate::theme::Mode) -> Theme {
    match m {
        crate::theme::Mode::Dark => Theme::default(),
        crate::theme::Mode::Graphite => GRAPHITE,
    }
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
