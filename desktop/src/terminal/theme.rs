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
