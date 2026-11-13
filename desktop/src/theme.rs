//! The look, in one table: the web UI's design tokens (`assets/app.css`,
//! `:root`, read only) plus the sizes its layout uses. The core draws its
//! chrome from here, and screens get the same table as the read-only global
//! `theme` and may name any colour, radius or size by its token name.
//!
//! Names drop the CSS prefix: `--c-text-muted` is `text-muted`, `--r-sm` is
//! `sm`. A colour prop also takes `#rrggbb`.

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
    ("text-muted", 0x8b949e),  // secondary text, field labels
    ("text-soft", 0x9198a1),   // lead paragraphs
    ("text-dim", 0x6e7681),    // tertiary/placeholder text
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
    // header, sidebar and panes stand on (`body`, `.layout`), and the pane
    // bar's default (`--c-pane-bar`, darkslategrey, which the web UI lets the
    // person change).
    ("floor", 0x080b0f),
    ("pane-bar", 0x2f4f4f),
];

/// Corner radii, `--r-*`.
pub const RADII: &[(&str, f32)] = &[("xs", 6.0), ("sm", 8.0), ("md", 12.0), ("lg", 16.0), ("pill", 999.0)];

/// Text sizes the web UI uses (px = pt here). `md` is the body (13px).
pub const TEXT_SIZES: &[(&str, f32)] = &[("xxs", 9.5), ("xs", 10.5), ("sm", 11.0), ("md", 13.0), ("lg", 14.0), ("xl", 16.0)];

/// Spacing steps, from the paddings and gaps of `app.css` (.15rem .. 1.1rem at 16px).
pub const SPACE: &[(&str, f32)] = &[("xxs", 2.0), ("xs", 4.0), ("sm", 6.0), ("md", 8.0), ("lg", 12.0), ("xl", 16.0)];

/// Layout constants of the shell, from `app.css`.
pub mod layout {
    /// `header { height: 48px; margin: 6px 6px 0 }`.
    pub const TOP_BAR_H: f32 = 48.0;
    /// The gutter between the header, sidebar and panes (`.main { gap: 6px; padding: 6px }`).
    pub const GUTTER: f32 = 6.0;
    /// `.sidebar { width: 205px }`; wider here because the screen shows more per row.
    pub const SIDEBAR_W: f32 = 240.0;
    /// `.pane-bar { height: 32px }`.
    pub const PANE_BAR_H: f32 = 32.0;
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
        "layout": {
            "top_bar_h": layout::TOP_BAR_H, "gutter": layout::GUTTER, "sidebar_w": layout::SIDEBAR_W,
            "pane_bar_h": layout::PANE_BAR_H, "traffic_lights_w": layout::TRAFFIC_LIGHTS_W,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every colour token of app.css's `:root` is here with the same value.
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
            assert_eq!(color_value(name), Some(u32::from_str_radix(&v, 16).unwrap()), "--c-{name}");
            n += 1;
        }
        assert!(n >= 30, "read {n} tokens");
        for (r, v) in [("xs", 6.0), ("sm", 8.0), ("md", 12.0), ("lg", 16.0)] {
            assert!(root.contains(&format!("--r-{r}: {v}px")), "--r-{r}");
            assert_eq!(radius(r), Ok(v));
        }
    }

    #[test]
    fn colour_props() {
        assert_eq!(parse_color("text-muted"), Ok(0x8b949e));
        assert_eq!(parse_color("#00ff00"), Ok(0x00ff00));
        assert!(parse_color("#0f0").is_err());
        assert!(parse_color("pink").unwrap_err().contains("unknown colour"));
    }
}
