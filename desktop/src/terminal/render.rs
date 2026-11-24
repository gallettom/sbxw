//! One screen row of `alacritty_terminal` cells to one `rune::GridRow`: text
//! runs as spans (colour, bold, italic, single underline), backgrounds and the
//! selection as fills, wide cells pinned to their columns, and a vector layer
//! for what a font does not draw edge to edge: box drawing, block elements,
//! Powerline triangles, double, curly, dotted and dashed underlines,
//! strikethrough, link underlines and the non-block cursor shapes.

use super::pills::{Part, Piece};
use super::theme::Theme;
use crate::emphasis::{self, Pill};
use alacritty_terminal::grid::Row;
use alacritty_terminal::index::Column;
use alacritty_terminal::term::cell::{Cell, Flags};
use alacritty_terminal::term::color::Colors;
use alacritty_terminal::vte::ansi::{Color as AColor, NamedColor, Rgb};
use rune::{Color, GridRow, Shape, Span, TextStyle};

/// How the cursor is drawn on its cell.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CursorDraw {
    Block,
    /// The window or the terminal does not have the keyboard.
    Hollow,
    Underline,
    Beam,
}

/// Everything a row is drawn from besides its cells.
pub struct RowCtx<'a> {
    pub cols: usize,
    pub colors: &'a Colors,
    pub theme: &'a Theme,
    pub style: TextStyle,
    pub cell_w: f32,
    pub line_h: f32,
    pub font_size: f32,
    /// The cursor's column on this row, and its shape.
    pub cursor: Option<(usize, CursorDraw)>,
    /// Selected columns on this row, `start..end`.
    pub selection: Option<(usize, usize)>,
    /// Columns of a hovered hyperlink on this row, underlined.
    pub link: Option<(usize, usize)>,
    /// Keyword pills on this row ([`super::pills`]), left to right, and the
    /// colours of each tone.
    pub pills: &'a [Piece],
    pub pill_colors: &'a [Pill; 7],
}

/// A 0xRRGGBB (sRGB) colour as Rune's (linear) colour.
pub fn rgb(c: u32) -> Color {
    thread_local!(static LUT: [f32; 256] = std::array::from_fn(|i| rune::color::srgb_decode(i as f32 / 255.0)));
    LUT.with(|l| Color { r: l[(c >> 16 & 0xff) as usize], g: l[(c >> 8 & 0xff) as usize], b: l[(c & 0xff) as usize], a: 1.0 })
}

fn rgb_of(r: Rgb) -> u32 {
    (r.r as u32) << 16 | (r.g as u32) << 8 | r.b as u32
}

/// A cell colour as 0xRRGGBB: the program's overrides (OSC 4, 10, 11) first,
/// then the theme. Under a picked colour ([`Theme::remap`]) an explicit
/// neutral colour (24-bit, or 256-colour 16 and up) follows the pick.
pub fn resolve(c: AColor, colors: &Colors, theme: &Theme, fg: bool) -> u32 {
    match c {
        AColor::Spec(r) => match theme.remap {
            Some(m) => m.apply(rgb_of(r), fg),
            None => rgb_of(r),
        },
        AColor::Indexed(i) => {
            let v = colors[i as usize].map(rgb_of).unwrap_or_else(|| theme.indexed(i));
            match theme.remap {
                Some(m) if i >= 16 => m.apply(v, fg),
                _ => v,
            }
        }
        AColor::Named(n) => {
            if let Some(r) = colors[n] {
                return rgb_of(r);
            }
            let i = n as usize;
            match n {
                NamedColor::Foreground | NamedColor::BrightForeground => theme.foreground,
                NamedColor::Background => theme.background,
                NamedColor::Cursor => theme.cursor,
                NamedColor::DimForeground => theme.dim(theme.foreground),
                _ if i < 16 => theme.palette[i],
                _ if (NamedColor::DimBlack as usize..=NamedColor::DimWhite as usize).contains(&i) => {
                    theme.dim(theme.palette[i - NamedColor::DimBlack as usize])
                }
                _ if fg => theme.foreground,
                _ => theme.background,
            }
        }
    }
}

/// The foreground of a cell, before inversion: bold makes ANSI 0-7 bright,
/// as xterm.js does by default; dim moves it toward the background.
fn cell_fg(cell: &Cell, ctx: &RowCtx) -> u32 {
    let mut c = cell.fg;
    if cell.flags.contains(Flags::BOLD) {
        if let AColor::Named(n) = c {
            if (n as usize) < 8 {
                c = AColor::Indexed(n as u8 + 8);
            }
        }
    }
    let v = resolve(c, ctx.colors, ctx.theme, true);
    if cell.flags.contains(Flags::DIM) {
        ctx.theme.dim(v)
    } else {
        v
    }
}

struct Painter {
    shape: Shape,
    any: bool,
}

impl Painter {
    fn rect(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, c: Color) {
        if x1 <= x0 || y1 <= y0 {
            return;
        }
        let s = std::mem::take(&mut self.shape);
        self.shape = s.fill(vec![[x0, y0], [x1, y0], [x1, y1], [x0, y1]], c);
        self.any = true;
    }
    fn poly(&mut self, pts: Vec<[f32; 2]>, c: Color) {
        let s = std::mem::take(&mut self.shape);
        self.shape = s.fill(pts, c);
        self.any = true;
    }
    fn stroke(&mut self, pts: Vec<[f32; 2]>, w: f32, c: Color) {
        let s = std::mem::take(&mut self.shape);
        self.shape = s.stroke(pts, w, c);
        self.any = true;
    }
}

/// Appends `glyph` to the row's text, extending the last span when it has
/// the same look.
#[allow(clippy::too_many_arguments)]
fn push_span(text: &mut String, spans: &mut Vec<Span>, glyph: &str, fgc: Color, bold: bool, italic: bool, ul: Option<Color>, bg: Option<Color>) {
    let at = text.len();
    text.push_str(glyph);
    let end = text.len();
    let same = |s: &Span| s.end as usize == at && s.color == Some(fgc) && s.weight == bold.then_some(700.0) && s.italic == italic && s.underline == ul && s.bg == bg;
    match spans.last_mut() {
        Some(s) if same(s) => s.end = end as u32,
        _ => {
            let mut s = Span::new(at, end).color(fgc);
            if bold {
                s = s.weight(700.0);
            }
            s.italic = italic;
            s.underline = ul;
            s.bg = bg;
            spans.push(s);
        }
    }
}

/// What a pill sits on: the background of its first cell (a diff line's
/// band, a status bar), else the terminal's.
fn pill_ground(piece: &Piece, row: &Row<Cell>, ctx: &RowCtx) -> u32 {
    let cell = &row[Column(piece.start as usize)];
    let mut bg = resolve(cell.bg, ctx.colors, ctx.theme, false);
    if cell.flags.contains(Flags::INVERSE) {
        bg = cell_fg(cell, ctx);
    }
    bg
}

/// A pill's colours over the background it sits on: the tone tinted over
/// that cell background, so it blends into a coloured line.
fn pill_colors_on(piece: &Piece, row: &Row<Cell>, ctx: &RowCtx) -> Pill {
    let g = pill_ground(piece, row, ctx);
    if g == ctx.theme.background {
        return ctx.pill_colors[emphasis::index(piece.tone)];
    }
    emphasis::palette(piece.tone, g, ctx.theme.foreground)
}

/// A pill's capsule over its cells' fill: the corners outside each round
/// end (a half-disc as tall as the pill, square where the wrap cuts it) and
/// [`emphasis::AIR`] above and below, painted in `under`; a hairline round
/// it when the colours have one.
fn pill_shape(p: &mut Painter, piece: &Piece, under: u32, colors: Pill, cw: f32, lh: f32) {
    let air = emphasis::AIR.min(lh / 6.0);
    let (x0, x1) = ((piece.start as f32 * cw).round(), (piece.end as f32 * cw).round());
    let (top, bot) = (air, lh - air);
    let r = ((bot - top) / 2.0).min((x1 - x0) / 2.0);
    let cy = (top + bot) / 2.0;
    let g = rgb(under);
    p.rect(x0, 0.0, x1, top, g);
    p.rect(x0, bot, x1, lh, g);
    let arc = |cx: f32, from: f32, to: f32| -> Vec<[f32; 2]> {
        (0..=10).map(|i| {
            let a = from + (to - from) * i as f32 / 10.0;
            [cx + r * a.cos(), cy + r * a.sin()]
        }).collect()
    };
    use std::f32::consts::{FRAC_PI_2, PI};
    if !piece.cut_left {
        // From the top, round the left end, to the bottom; then the cell's corner side.
        let mut pts = vec![[x0, top]];
        pts.extend(arc(x0 + r, -FRAC_PI_2, -FRAC_PI_2 - PI));
        pts.push([x0, bot]);
        p.poly(pts, g);
    }
    if !piece.cut_right {
        let mut pts = vec![[x1, top]];
        pts.extend(arc(x1 - r, -FRAC_PI_2, FRAC_PI_2));
        pts.push([x1, bot]);
        p.poly(pts, g);
    }
    if colors.line != 0 {
        let mut ring = vec![];
        if piece.cut_left { ring.extend([[x0, bot], [x0, top]]); } else { ring.extend(arc(x0 + r, FRAC_PI_2, FRAC_PI_2 + PI)); }
        if piece.cut_right { ring.extend([[x1, top], [x1, bot]]); } else { ring.extend(arc(x1 - r, -FRAC_PI_2, FRAC_PI_2)); }
        let s = std::mem::take(&mut p.shape);
        p.shape = s.stroke_with(ring, true, 1.0, rune::LineCap::Round, rgb(colors.line));
        p.any = true;
    }
}

/// The row's text, spans, fills, pinned cells and vectors.
pub fn row_parts(row: &Row<Cell>, ctx: &RowCtx) -> RowParts {
    let theme = ctx.theme;
    let mut text = String::new();
    let mut spans: Vec<Span> = Vec::new();
    let mut fills: Vec<(u16, u16, Color, u32)> = Vec::new();
    let mut wide: Vec<(u16, String, Color, bool)> = Vec::new();
    let mut p = Painter { shape: Shape::new(), any: false };
    let mut drawn: Vec<(u16, u16, bool, bool)> = Vec::new();
    let (cw, lh) = (ctx.cell_w, ctx.line_h);
    let light = (ctx.font_size / 14.0).round().max(1.0);
    let ul_y = (lh / 2.0 + ctx.font_size * 0.45).min(lh - light).floor();
    let ncols = ctx.cols.min(row.len());
    let mut c = 0;
    while c < ncols {
        let cell = &row[Column(c)];
        if cell.flags.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER) {
            match ctx.pills.iter().find(|p| p.covers(c)) {
                // Inside a pill, the space keeps the pill's background.
                Some(p) => {
                    let colors = pill_colors_on(p, row, ctx);
                    let word = p.part(c) != Part::Detail;
                    push_span(&mut text, &mut spans, " ", rgb(if word { colors.strong } else { colors.detail }), false, false, None, None);
                }
                None => text.push(' '),
            }
            c += 1;
            continue;
        }
        let is_wide = cell.flags.contains(Flags::WIDE_CHAR);
        let width = if is_wide { 2 } else { 1 };
        let mut fg = cell_fg(cell, ctx);
        let mut bg = resolve(cell.bg, ctx.colors, theme, false);
        if cell.flags.contains(Flags::INVERSE) {
            std::mem::swap(&mut fg, &mut bg);
        }
        if cell.flags.contains(Flags::HIDDEN) {
            fg = bg;
        }
        let cursor_here = ctx.cursor.filter(|(cc, _)| *cc == c).map(|(_, d)| d);
        if cursor_here == Some(CursorDraw::Block) {
            fg = theme.background;
            bg = theme.cursor;
        }
        // A pill's cell: the pill's colours replace the program's, the
        // brackets are padding, and the cell's own decorations are not drawn.
        // The pill is a capsule (`pill_shape`): its fill under the text, the
        // corners outside its half-disc ends and the air above and below
        // painted back in the background under it.
        let pill = ctx.pills.iter().find(|p| p.covers(c)).map(|p| (p, pill_colors_on(p, row, ctx), p.part(c)));
        if let Some((piece, colors, part)) = pill {
            let x0 = (c as f32 * cw).round();
            let x1 = ((c + width) as f32 * cw).round();
            if c == piece.start as usize {
                let under = pill_ground(piece, row, ctx);
                fills.push((piece.start, piece.end, rgb(colors.bg), colors.bg));
                pill_shape(&mut p, piece, under, colors, cw, lh);
                drawn.push((piece.start, piece.end, !piece.cut_left, !piece.cut_right));
            }
            let fgv = match part {
                Part::Word => colors.strong,
                Part::Detail => colors.detail,
                Part::Pad => colors.strong,
            };
            let fgc = rgb(fgv);
            let ch = if part == Part::Pad || cell.c == '\0' || cell.c == '\t' { ' ' } else { cell.c };
            let drawn = !is_wide && cell.zerowidth().is_none() && draw_glyph(&mut p, ch, x0, x1, lh, light, fgc);
            let mut glyph = String::new();
            glyph.push(if drawn { ' ' } else { ch });
            if let (Some(zw), false) = (cell.zerowidth(), part == Part::Pad) {
                glyph.extend(zw);
            }
            // Regular weight, the word and the detail alike (views2).
            if is_wide {
                wide.push((c as u16, glyph, fgc, false));
                glyph = " ".into();
            }
            push_span(&mut text, &mut spans, &glyph, fgc, false, false, None, None);
            c += 1;
            continue;
        }
        if bg != theme.background {
            let col = rgb(bg);
            match fills.last_mut() {
                Some((_, end, _, b)) if *end == c as u16 && *b == bg => *end += width as u16,
                _ => fills.push((c as u16, (c + width) as u16, col, bg)),
            }
        }
        // Cell edges on whole points, so neighbouring cells' vectors meet with
        // no seam (cell widths are fractional).
        let x0 = (c as f32 * cw).round();
        let x1 = ((c + width) as f32 * cw).round();
        let fgc = rgb(fg);
        // Lines a font cannot draw edge to edge.
        let ch = if cell.c == '\0' || cell.c == '\t' { ' ' } else { cell.c };
        let drawn = !is_wide && cell.zerowidth().is_none() && draw_glyph(&mut p, ch, x0, x1, lh, light, fgc);
        // Underlines, strikethrough.
        let ul_color = cell.underline_color().map(|u| rgb(resolve(u, ctx.colors, theme, true))).unwrap_or(fgc);
        let f = cell.flags;
        if f.contains(Flags::DOUBLE_UNDERLINE) {
            p.rect(x0, ul_y - light * 2.0, x1, ul_y - light, ul_color);
            p.rect(x0, ul_y + light * 0.5, x1, ul_y + light * 1.5, ul_color);
        } else if f.contains(Flags::UNDERCURL) {
            let pts: Vec<[f32; 2]> = (0..=8).map(|i| {
                let t = i as f32 / 8.0;
                [x0 + t * (x1 - x0), ul_y + (t * std::f32::consts::TAU).sin() * 1.5]
            }).collect();
            p.stroke(pts, light, ul_color);
        } else if f.contains(Flags::DOTTED_UNDERLINE) {
            let mut x = x0;
            while x < x1 {
                p.rect(x, ul_y, (x + light).min(x1), ul_y + light, ul_color);
                x += light * 2.0;
            }
        } else if f.contains(Flags::DASHED_UNDERLINE) {
            let dash = (cw / 2.0).floor().max(2.0);
            p.rect(x0, ul_y, x0 + dash, ul_y + light, ul_color);
        } else if ctx.link.is_some_and(|(a, b)| c >= a && c < b) && !f.contains(Flags::UNDERLINE) {
            p.rect(x0, ul_y, x1, ul_y + light, fgc);
        }
        if f.contains(Flags::STRIKEOUT) {
            let y = (lh / 2.0).floor();
            p.rect(x0, y, x1, y + light, fgc);
        }
        match cursor_here {
            Some(CursorDraw::Beam) => p.rect(x0, 1.0, x0 + light * 2.0, lh - 1.0, rgb(theme.cursor)),
            Some(CursorDraw::Underline) => p.rect(x0, lh - light * 2.0, x1, lh, rgb(theme.cursor)),
            Some(CursorDraw::Hollow) => {
                let cc = rgb(theme.cursor);
                p.rect(x0, 0.0, x1, light, cc);
                p.rect(x0, lh - light, x1, lh, cc);
                p.rect(x0, 0.0, x0 + light, lh, cc);
                p.rect(x1 - light, 0.0, x1, lh, cc);
            }
            _ => {}
        }
        let mut glyph = String::new();
        glyph.push(if drawn { ' ' } else { ch });
        if let Some(zw) = cell.zerowidth() {
            glyph.extend(zw);
        }
        let bold = f.contains(Flags::BOLD);
        let italic = f.contains(Flags::ITALIC);
        let underline = f.contains(Flags::UNDERLINE);
        if is_wide {
            // Pinned to its two columns; the line keeps a space for each.
            wide.push((c as u16, glyph, fgc, bold));
            text.push(' ');
            c += 1;
            continue;
        }
        push_span(&mut text, &mut spans, &glyph, fgc, bold, italic, underline.then_some(ul_color), None);
        c += 1;
    }
    RowParts { text, spans, fills, wide, shape: p.any.then_some(p.shape), pills: drawn }
}

/// A row before it becomes a `GridRow` (tests read it: Rune's row is opaque).
pub struct RowParts {
    /// One character per narrow cell (a space for a wide cell's columns).
    pub text: String,
    pub spans: Vec<Span>,
    /// Background runs: columns, colour, 0xRRGGBB.
    pub fills: Vec<(u16, u16, Color, u32)>,
    pub wide: Vec<(u16, String, Color, bool)>,
    pub shape: Option<Shape>,
    /// Each pill drawn on the row: its columns and whether each end is round
    /// (square at a cut by the wrap).
    pub pills: Vec<(u16, u16, bool, bool)>,
}

/// Builds the row. `row` holds at least `ctx.cols` cells.
pub fn build_row(row: &Row<Cell>, ctx: &RowCtx) -> GridRow {
    into_row(row_parts(row, ctx), ctx)
}

/// The parts as Rune's row.
pub fn into_row(parts: RowParts, ctx: &RowCtx) -> GridRow {
    let theme = ctx.theme;
    let RowParts { text, spans, fills, wide, shape, .. } = parts;
    let mut row_el = GridRow::new(text, spans, ctx.style);
    for (a, b, col, _) in fills {
        row_el = row_el.fill(a, b, col);
    }
    if let Some((a, b)) = ctx.selection {
        row_el = row_el.fill(a as u16, b.min(ctx.cols) as u16, rgb(theme.selection).alpha(theme.selection_alpha));
    }
    for (col, g, color, bold) in wide {
        let st = TextStyle { weight: if bold { 700.0 } else { ctx.style.weight }, ..ctx.style };
        row_el = row_el.cell(col, 2, g, st, color);
    }
    if let Some(shape) = shape {
        row_el = row_el.shape(shape);
    }
    row_el
}

/// Box drawing (U+2500 to U+257F) as [up, right, down, left] weights:
/// 0 none, 1 light, 2 heavy, 3 double.
fn box_lines(ch: char) -> Option<[u8; 4]> {
    let i = (ch as u32).checked_sub(0x2500)? as usize;
    const T: [[u8; 4]; 0x80] = [
        [0, 1, 0, 1], [0, 2, 0, 2], [1, 0, 1, 0], [2, 0, 2, 0], [0, 1, 0, 1], [0, 2, 0, 2], [1, 0, 1, 0], [2, 0, 2, 0], // 2500
        [0, 1, 0, 1], [0, 2, 0, 2], [1, 0, 1, 0], [2, 0, 2, 0], [0, 1, 1, 0], [0, 2, 1, 0], [0, 1, 2, 0], [0, 2, 2, 0], // 2508
        [0, 0, 1, 1], [0, 0, 1, 2], [0, 0, 2, 1], [0, 0, 2, 2], [1, 1, 0, 0], [1, 2, 0, 0], [2, 1, 0, 0], [2, 2, 0, 0], // 2510
        [1, 0, 0, 1], [1, 0, 0, 2], [2, 0, 0, 1], [2, 0, 0, 2], [1, 1, 1, 0], [1, 2, 1, 0], [2, 1, 1, 0], [1, 1, 2, 0], // 2518
        [2, 1, 2, 0], [2, 2, 1, 0], [1, 2, 2, 0], [2, 2, 2, 0], [1, 0, 1, 1], [1, 0, 1, 2], [2, 0, 1, 1], [1, 0, 2, 1], // 2520
        [2, 0, 2, 1], [2, 0, 1, 2], [1, 0, 2, 2], [2, 0, 2, 2], [0, 1, 1, 1], [0, 1, 1, 2], [0, 2, 1, 1], [0, 2, 1, 2], // 2528
        [0, 1, 2, 1], [0, 1, 2, 2], [0, 2, 2, 1], [0, 2, 2, 2], [1, 1, 0, 1], [1, 1, 0, 2], [1, 2, 0, 1], [1, 2, 0, 2], // 2530
        [2, 1, 0, 1], [2, 1, 0, 2], [2, 2, 0, 1], [2, 2, 0, 2], [1, 1, 1, 1], [1, 1, 1, 2], [1, 2, 1, 1], [1, 2, 1, 2], // 2538
        [2, 1, 1, 1], [1, 1, 2, 1], [2, 1, 2, 1], [2, 1, 1, 2], [2, 2, 1, 1], [1, 1, 2, 2], [1, 2, 2, 1], [2, 2, 1, 2], // 2540
        [1, 2, 2, 2], [2, 1, 2, 2], [2, 2, 2, 1], [2, 2, 2, 2], [0, 1, 0, 1], [0, 2, 0, 2], [1, 0, 1, 0], [2, 0, 2, 0], // 2548
        [0, 3, 0, 3], [3, 0, 3, 0], [0, 3, 1, 0], [0, 1, 3, 0], [0, 3, 3, 0], [0, 0, 1, 3], [0, 0, 3, 1], [0, 0, 3, 3], // 2550
        [1, 3, 0, 0], [3, 1, 0, 0], [3, 3, 0, 0], [1, 0, 0, 3], [3, 0, 0, 1], [3, 0, 0, 3], [1, 3, 1, 0], [3, 1, 3, 0], // 2558
        [3, 3, 3, 0], [1, 0, 1, 3], [3, 0, 3, 1], [3, 0, 3, 3], [0, 3, 1, 3], [0, 1, 3, 1], [0, 3, 3, 3], [1, 3, 0, 3], // 2560
        [3, 1, 0, 1], [3, 3, 0, 3], [1, 3, 1, 3], [3, 1, 3, 1], [3, 3, 3, 3], [0, 1, 1, 0], [0, 0, 1, 1], [1, 0, 0, 1], // 2568
        [1, 1, 0, 0], [0, 0, 0, 0], [0, 0, 0, 0], [0, 0, 0, 0], [0, 0, 0, 1], [1, 0, 0, 0], [0, 1, 0, 0], [0, 0, 1, 0], // 2570
        [0, 0, 0, 2], [2, 0, 0, 0], [0, 2, 0, 0], [0, 0, 2, 0], [0, 2, 0, 1], [1, 0, 2, 0], [0, 1, 0, 2], [2, 0, 1, 0], // 2578
    ];
    T.get(i).copied().filter(|w| w.iter().any(|&x| x > 0))
}

/// Draws `ch` as vectors when it is a line or block glyph; returns false for
/// everything the font draws.
fn draw_glyph(p: &mut Painter, ch: char, x0: f32, x1: f32, lh: f32, light: f32, c: Color) -> bool {
    let cw = x1 - x0;
    let u = ch as u32;
    // The light line's left edge and top edge, on whole points; its centre.
    let mid = ((x0 + x1) / 2.0).floor();
    let (cx, cy) = (mid + light / 2.0, (lh / 2.0).floor() + light / 2.0);
    // Rounded corners: a quarter curve from one edge's middle to the other's.
    if (0x256d..=0x2570).contains(&u) {
        let (a, b) = match u {
            0x256d => ([cx, lh], [x1, cy]),
            0x256e => ([x0, cy], [cx, lh]),
            0x256f => ([cx, 0.0], [x0, cy]),
            _ => ([x1, cy], [cx, 0.0]),
        };
        let r = (cw / 2.0).min(lh / 2.0);
        let toward = |from: [f32; 2], d: f32| {
            let (dx, dy) = (from[0] - cx, from[1] - cy);
            let len = (dx * dx + dy * dy).sqrt().max(1e-3);
            [cx + dx / len * d, cy + dy / len * d]
        };
        let (pa, pb) = (toward(a, r), toward(b, r));
        let mut pts = vec![a, pa];
        for i in 1..8 {
            let t = i as f32 / 8.0;
            let q = |k: usize| (1.0 - t) * (1.0 - t) * pa[k] + 2.0 * (1.0 - t) * t * [cx, cy][k] + t * t * pb[k];
            pts.push([q(0), q(1)]);
        }
        pts.push(pb);
        pts.push(b);
        p.stroke(pts, light, c);
        return true;
    }
    if (0x2571..=0x2573).contains(&u) {
        if u != 0x2572 {
            p.stroke(vec![[x1, 0.0], [x0, lh]], light, c);
        }
        if u != 0x2571 {
            p.stroke(vec![[x0, 0.0], [x1, lh]], light, c);
        }
        return true;
    }
    if let Some(w) = box_lines(ch) {
        let heavy = light * 2.0;
        // Double strokes sit this far either side of the centre.
        let d = light * 2.0;
        let th = |k: u8| if k == 2 { heavy } else { light };
        // A stroke of thickness `t` around centre `m`, on whole points.
        let lo = |m: f32| m - light / 2.0;
        // How far a crossing segment reaches from the centre, toward the
        // cell's top or left (`neg`) and bottom or right (`pos`): a heavy
        // stroke is the light one widened on its bottom or right side.
        let neg = |k: u8| match k {
            1 | 2 => light / 2.0,
            3 => d + light / 2.0,
            _ => 0.0,
        };
        let pos = |k: u8| match k {
            1 => light / 2.0,
            2 => heavy - light / 2.0,
            3 => d + light / 2.0,
            _ => 0.0,
        };
        let [up, right, down, left] = w;
        let (h_neg, h_pos) = (neg(left).max(neg(right)), pos(left).max(pos(right)));
        let (v_neg, v_pos) = (neg(up).max(neg(down)), pos(up).max(pos(down)));
        let (hdouble, vdouble) = (left == 3 || right == 3, up == 3 || down == 3);
        for (dir, &k) in w.iter().enumerate() {
            match k {
                1 | 2 => {
                    let t = th(k);
                    match dir {
                        0 => {
                            let end = if hdouble && down == 0 { cy - d + light / 2.0 } else { cy + h_pos.max(light / 2.0) };
                            p.rect(lo(cx), 0.0, lo(cx) + t, end, c);
                        }
                        2 => {
                            let start = if hdouble && up == 0 { cy + d - light / 2.0 } else { cy - h_neg.max(light / 2.0) };
                            p.rect(lo(cx), start, lo(cx) + t, lh, c);
                        }
                        1 => {
                            let start = if vdouble && left == 0 { cx + d - light / 2.0 } else { cx - v_neg.max(light / 2.0) };
                            p.rect(start, lo(cy), x1, lo(cy) + t, c);
                        }
                        _ => {
                            let end = if vdouble && right == 0 { cx - d + light / 2.0 } else { cx + v_pos.max(light / 2.0) };
                            p.rect(x0, lo(cy), end, lo(cy) + t, c);
                        }
                    }
                }
                3 => {
                    // Two strokes; at a corner the outer one runs to the far
                    // stroke of the crossing pair and the inner one stops at its near one.
                    for s in [-1.0f32, 1.0] {
                        let (side, other) = match (dir, s < 0.0) {
                            (0 | 2, true) => (left, right),
                            (0 | 2, false) => (right, left),
                            (_, true) => (up, down),
                            (_, false) => (down, up),
                        };
                        let reach = |centre: f32, toward_far: f32| {
                            if side == 3 {
                                centre - toward_far * (d - light / 2.0)
                            } else if other == 3 {
                                centre + toward_far * (d + light / 2.0)
                            } else {
                                centre + toward_far * light / 2.0
                            }
                        };
                        match dir {
                            0 => p.rect(lo(cx + s * d), 0.0, lo(cx + s * d) + light, reach(cy, 1.0), c),
                            2 => p.rect(lo(cx + s * d), reach(cy, -1.0), lo(cx + s * d) + light, lh, c),
                            1 => p.rect(reach(cx, -1.0), lo(cy + s * d), x1, lo(cy + s * d) + light, c),
                            _ => p.rect(x0, lo(cy + s * d), reach(cx, 1.0), lo(cy + s * d) + light, c),
                        }
                    }
                }
                _ => {}
            }
        }
        return true;
    }
    // Block elements.
    let (hx, hy) = (((x0 + x1) / 2.0).round(), lh / 2.0);
    match u {
        0x2580 => p.rect(x0, 0.0, x1, hy, c),
        0x2581..=0x2588 => {
            let eighths = (u - 0x2580) as f32;
            p.rect(x0, lh - lh * eighths / 8.0, x1, lh, c)
        }
        0x2589..=0x258f => {
            let eighths = (0x2590 - u) as f32;
            p.rect(x0, 0.0, x0 + cw * eighths / 8.0, lh, c)
        }
        0x2590 => p.rect(hx, 0.0, x1, lh, c),
        0x2591..=0x2593 => p.rect(x0, 0.0, x1, lh, c.alpha(0.25 * (u - 0x2590) as f32)),
        0x2594 => p.rect(x0, 0.0, x1, lh / 8.0, c),
        0x2595 => p.rect(x1 - cw / 8.0, 0.0, x1, lh, c),
        0x2596..=0x259f => {
            // Quadrants: upper left, upper right, lower left, lower right.
            let q: [bool; 4] = match u {
                0x2596 => [false, false, true, false],
                0x2597 => [false, false, false, true],
                0x2598 => [true, false, false, false],
                0x2599 => [true, false, true, true],
                0x259a => [true, false, false, true],
                0x259b => [true, true, true, false],
                0x259c => [true, true, false, true],
                0x259d => [false, true, false, false],
                0x259e => [false, true, true, false],
                _ => [false, true, true, true],
            };
            let boxes = [(x0, 0.0, hx, hy), (hx, 0.0, x1, hy), (x0, hy, hx, lh), (hx, hy, x1, lh)];
            for (on, (a, b, cc, d)) in q.iter().zip(boxes) {
                if *on {
                    p.rect(a, b, cc, d, c);
                }
            }
        }
        // Powerline: solid and outline triangles.
        0xe0b0 => p.poly(vec![[x0, 0.0], [x1, hy], [x0, lh]], c),
        0xe0b2 => p.poly(vec![[x1, 0.0], [x0, hy], [x1, lh]], c),
        0xe0b1 => p.stroke(vec![[x0, 0.0], [x1, hy], [x0, lh]], light, c),
        0xe0b3 => p.stroke(vec![[x1, 0.0], [x0, hy], [x1, lh]], light, c),
        _ => return false,
    }
    true
}

#[cfg(test)]
mod tests {
    use super::{box_lines, resolve};
    use crate::terminal::theme::{self as tt, Chosen, Pick, Slot, Theme};
    use crate::theme::contrast;
    use alacritty_terminal::term::color::Colors;
    use alacritty_terminal::vte::ansi::{Color as AColor, NamedColor, Rgb};

    fn spec(c: u32) -> AColor {
        AColor::Spec(Rgb { r: (c >> 16) as u8, g: (c >> 8) as u8, b: c as u8 })
    }

    fn picked(bg: Option<u32>, text: Option<Pick>, base: Theme) -> Theme {
        Chosen { background: bg, bar: None, text }.theme(base)
    }

    // Claude Code's dark theme: its text, its "inactive" grey, its message
    // band and its bash band.
    const WHITE: u32 = 0xffffff;
    const INACTIVE: u32 = 0x999999;
    const BAND: u32 = 0x373737;
    const BASH_BAND: u32 = 0x413c41;
    const NIGHT: u32 = 0x1a1b26;
    const PAPER: u32 = 0xfdf6e3;
    const CREAM: u32 = 0xe6d5b8;

    /// With nothing picked every colour is drawn as today: the program's
    /// 24-bit and 256 colours, foreground and background, in both app themes.
    #[test]
    fn nothing_picked_leaves_every_colour_unchanged() {
        let colors = Colors::default();
        for base in [Theme::default(), tt::GRAPHITE] {
            let th = picked(None, None, base.clone());
            assert_eq!(th, base);
            assert_eq!(th.remap, None);
            for fg in [true, false] {
                for c in (0..=0xffffffu32).step_by(0x010307) {
                    assert_eq!(resolve(spec(c), &colors, &th, fg), c);
                }
                for i in 0..=255u8 {
                    assert_eq!(resolve(AColor::Indexed(i), &colors, &th, fg), th.indexed(i));
                }
            }
            // Auto text alone is nothing picked.
            assert_eq!(picked(None, Some(Pick::Auto), base.clone()), base);
        }
    }

    #[test]
    fn a_picked_text_remaps_white_and_ramp_greys_and_keeps_colours() {
        let colors = Colors::default();
        let th = picked(None, Some(Pick::Color(CREAM)), Theme::default());
        let r = |c: AColor| resolve(c, &colors, &th, true);
        assert_eq!(r(spec(WHITE)), CREAM);
        assert_eq!(r(AColor::Indexed(231)), CREAM, "the cube's white");
        // A ramp grey (250, #bcbcbc) is a tone between the ground and the pick.
        let g = r(AColor::Indexed(250));
        assert_eq!(g, tt::mix(th.background, CREAM, 0xbc as f32 / 255.0));
        assert!(contrast(g, th.background) < contrast(CREAM, th.background));
        // Dim stays dimmer than full, and faint marks stay.
        assert!(contrast(r(spec(INACTIVE)), th.background) < contrast(CREAM, th.background));
        assert_eq!(r(spec(0x505050)), 0x505050);
        // Saturated colours stay as the program chose, 24-bit and 256.
        for c in [0xcc0000, 0x4e9a06, 0xff8700, 0x58a6ff, 0xe5c07b, 0xa0d0a0] {
            assert_eq!(r(spec(c)), c, "{c:06x}");
        }
        assert_eq!(r(AColor::Indexed(196)), 0xff0000);
        // Named colours keep their own path (ANSI white is the pick).
        assert_eq!(r(AColor::Named(NamedColor::White)), CREAM);
        assert_eq!(r(AColor::Named(NamedColor::Red)), th.palette[1]);
    }

    #[test]
    fn a_picked_background_remaps_a_near_black_band() {
        let colors = Colors::default();
        let th = picked(Some(NIGHT), Some(Pick::Color(CREAM)), Theme::default());
        let bg = |c: u32| resolve(spec(c), &colors, &th, false);
        // Black is the ground; a band is the ground a step toward the text.
        assert_eq!(bg(0x000000), NIGHT);
        let band = bg(BAND);
        assert_ne!(band, NIGHT);
        assert!(contrast(band, NIGHT) > 1.3, "still a band: {:.2}", contrast(band, NIGHT));
        assert!(contrast(CREAM, band) >= 4.5);
        assert_eq!(bg(BASH_BAND) != bg(BAND), true);
        // A coloured band (a diff line) and a light one stay.
        assert_eq!(bg(0x225c2b), 0x225c2b);
        assert_eq!(bg(0xf0f0f0), 0xf0f0f0);
        // A background picked with Auto text: the text follows too.
        let auto = picked(Some(NIGHT), Some(Pick::Auto), Theme::default());
        assert_eq!(resolve(spec(WHITE), &colors, &auto, true), auto.foreground);
        println!("Cream on Night: text #{CREAM:06x} on band #{band:06x} (ground #{NIGHT:06x}), dim #{:06x}", resolve(spec(INACTIVE), &colors, &th, true));
    }

    /// On Paper a band is a light grey band and the text on it dark.
    #[test]
    fn paper_stays_readable() {
        let colors = Colors::default();
        for text in [Some(Pick::Auto), None, Some(Pick::Color(0x24292f))] {
            let th = picked(Some(PAPER), text, Theme::default());
            let band = resolve(spec(BAND), &colors, &th, false);
            let fg = resolve(spec(WHITE), &colors, &th, true);
            assert!(tt::is_light(band), "#{band:06x}");
            assert!(!tt::is_light(fg), "#{fg:06x}");
            assert!(contrast(fg, band) >= 4.5, "{:.2}", contrast(fg, band));
            assert!(contrast(band, PAPER) > 1.1);
            let dim = resolve(spec(INACTIVE), &colors, &th, true);
            assert!(contrast(dim, PAPER) >= 3.0, "dim #{dim:06x}");
            println!("Paper ({text:?}): text #{fg:06x} on band #{band:06x}, dim #{dim:06x}");
        }
    }

    /// Every pair the picker offers (each background preset or none, each
    /// text preset or none, over both app themes) whose own text reads on
    /// its own ground at 4.5:1 keeps that on Claude Code's bands: white on
    /// the message band and on the bash band.
    #[test]
    fn remapped_text_reads_on_remapped_bands_for_every_preset_pair() {
        let colors = Colors::default();
        let bgs: Vec<Option<u32>> = std::iter::once(None).chain(Slot::Background.presets().iter().map(|(v, _)| tt::parse_hex(v))).collect();
        let texts: Vec<Option<Pick>> = std::iter::once(None).chain(Slot::Text.presets().iter().map(|(v, _)| Slot::Text.parse(v))).collect();
        let (mut checked, mut bad) = (0, vec![]);
        for base in [Theme::default(), tt::GRAPHITE] {
            for bg in &bgs {
                for text in &texts {
                    let th = picked(*bg, *text, base.clone());
                    if th.remap.is_none() || contrast(th.foreground, th.background) < 4.5 {
                        continue;
                    }
                    let fg = resolve(spec(WHITE), &colors, &th, true);
                    assert_eq!(fg, th.foreground);
                    for b in [BAND, BASH_BAND, 0x2d2d2d, 0x505050] {
                        let band = resolve(spec(b), &colors, &th, false);
                        checked += 1;
                        if contrast(fg, band) < 4.5 {
                            bad.push(format!("{bg:?} {text:?} band {b:06x} -> {band:06x}: {:.2}", contrast(fg, band)));
                        }
                    }
                }
            }
        }
        assert!(checked > 200, "{checked}");
        assert!(bad.is_empty(), "{bad:#?}");
    }

    #[test]
    fn the_remap_is_cached_per_colour_and_per_pick() {
        let a = tt::Remap { text: CREAM, ground: NIGHT };
        let b = tt::Remap { text: 0xffb000, ground: NIGHT };
        let h0 = tt::remap_cache_hits();
        let first = a.apply(BAND, false);
        assert_eq!(tt::remap_cache_hits(), h0);
        assert_eq!(a.apply(BAND, false), first);
        assert_eq!(tt::remap_cache_hits(), h0 + 1);
        // Another pick is not served the first one's answer.
        assert_ne!(b.apply(BAND, false), first);
        assert_eq!(tt::remap_cache_hits(), h0 + 1);
        // The foreground and background of one grey are two entries.
        assert_ne!(a.apply(0x606060, true), a.apply(0x606060, false));
        // Saturated colours never reach the cache.
        assert_eq!(a.apply(0xcc0000, true), 0xcc0000);
        assert_eq!(tt::remap_cache_hits(), h0 + 1);
    }

    /// The cost of the remap where colours are resolved: a 200 x 50 screen
    /// of Claude Code's greys and some syntax colours, both roles per cell,
    /// a hundred times, with and without a pick. Prints the numbers.
    #[test]
    fn remap_cost() {
        let colors = Colors::default();
        let plain = Theme::default();
        let th = picked(Some(NIGHT), Some(Pick::Color(CREAM)), Theme::default());
        let cells: Vec<(AColor, AColor)> = (0..200 * 50)
            .map(|i| match i % 5 {
                0 => (spec(WHITE), spec(BAND)),
                1 => (spec(INACTIVE), spec(0)),
                2 => (spec(0xe5c07b), spec(BASH_BAND)),
                3 => (AColor::Indexed(250), AColor::Named(NamedColor::Background)),
                _ => (AColor::Named(NamedColor::Foreground), spec(0x225c2b)),
            })
            .collect();
        let time = |th: &Theme| {
            let t0 = std::time::Instant::now();
            let mut acc = 0u32;
            for _ in 0..100 {
                for (f, b) in &cells {
                    acc ^= resolve(*f, &colors, th, true) ^ resolve(*b, &colors, th, false);
                }
            }
            (t0.elapsed(), acc)
        };
        let (base, _) = time(&plain);
        let (remap, _) = time(&th);
        let per = |d: std::time::Duration| d.as_nanos() as f64 / (100.0 * cells.len() as f64 * 2.0);
        println!("resolve: {:.1} ns/colour plain, {:.1} ns/colour picked; one 200x50 screen {:.0} us vs {:.0} us", per(base), per(remap), base.as_micros() as f64 / 100.0, remap.as_micros() as f64 / 100.0);
        // A debug build: generous, it is the per-frame order that matters.
        assert!(remap.as_millis() / 100 < 20, "{remap:?} for 100 screens");
    }

    #[test]
    fn box_table_matches_the_characters() {
        assert_eq!(box_lines('─'), Some([0, 1, 0, 1]));
        assert_eq!(box_lines('┼'), Some([1, 1, 1, 1]));
        assert_eq!(box_lines('┏'), Some([0, 2, 2, 0]));
        assert_eq!(box_lines('╋'), Some([2, 2, 2, 2]));
        assert_eq!(box_lines('╔'), Some([0, 3, 3, 0]));
        assert_eq!(box_lines('╬'), Some([3, 3, 3, 3]));
        assert_eq!(box_lines('╰'), Some([1, 1, 0, 0]));
        assert_eq!(box_lines('╿'), Some([2, 0, 1, 0]));
        assert_eq!(box_lines('a'), None);
    }
}
