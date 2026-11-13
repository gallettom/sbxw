//! One screen row of `alacritty_terminal` cells to one `rune::GridRow`: text
//! runs as spans (colour, bold, italic, single underline), backgrounds and the
//! selection as fills, wide cells pinned to their columns, and a vector layer
//! for what a font does not draw edge to edge: box drawing, block elements,
//! Powerline triangles, double, curly, dotted and dashed underlines,
//! strikethrough, link underlines and the non-block cursor shapes.

use super::theme::Theme;
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
/// then the theme.
pub fn resolve(c: AColor, colors: &Colors, theme: &Theme, fg: bool) -> u32 {
    match c {
        AColor::Spec(r) => rgb_of(r),
        AColor::Indexed(i) => colors[i as usize].map(rgb_of).unwrap_or_else(|| theme.indexed(i)),
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

/// Builds the row. `row` holds at least `ctx.cols` cells.
pub fn build_row(row: &Row<Cell>, ctx: &RowCtx) -> GridRow {
    let theme = ctx.theme;
    let mut text = String::new();
    let mut spans: Vec<Span> = Vec::new();
    let mut fills: Vec<(u16, u16, Color, u32)> = Vec::new();
    let mut wide: Vec<(u16, String, Color, bool)> = Vec::new();
    let mut p = Painter { shape: Shape::new(), any: false };
    let (cw, lh) = (ctx.cell_w, ctx.line_h);
    let light = (ctx.font_size / 14.0).round().max(1.0);
    let ul_y = (lh / 2.0 + ctx.font_size * 0.45).min(lh - light).floor();
    let ncols = ctx.cols.min(row.len());
    let mut c = 0;
    while c < ncols {
        let cell = &row[Column(c)];
        if cell.flags.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER) {
            text.push(' ');
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
        if bg != theme.background {
            let col = rgb(bg);
            match fills.last_mut() {
                Some((_, end, _, b)) if *end == c as u16 && *b == bg => *end += width as u16,
                _ => fills.push((c as u16, (c + width) as u16, col, bg)),
            }
        }
        let x0 = c as f32 * cw;
        let x1 = x0 + width as f32 * cw;
        let fgc = rgb(fg);
        // Lines a font cannot draw edge to edge.
        let ch = if cell.c == '\0' || cell.c == '\t' { ' ' } else { cell.c };
        let drawn = !is_wide && cell.zerowidth().is_none() && draw_glyph(&mut p, ch, x0, cw, lh, light, fgc);
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
        let at = text.len();
        text.push_str(&glyph);
        let end = text.len();
        let ul = underline.then_some(ul_color);
        let same = |s: &Span| s.end as usize == at && s.color == Some(fgc) && s.weight == bold.then_some(700.0) && s.italic == italic && s.underline == ul;
        match spans.last_mut() {
            Some(s) if same(s) => s.end = end as u32,
            _ => {
                let mut s = Span::new(at, end).color(fgc);
                if bold {
                    s = s.weight(700.0);
                }
                s.italic = italic;
                s.underline = ul;
                spans.push(s);
            }
        }
        c += 1;
    }
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
    if p.any {
        row_el = row_el.shape(p.shape);
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
fn draw_glyph(p: &mut Painter, ch: char, x0: f32, cw: f32, lh: f32, light: f32, c: Color) -> bool {
    let x1 = x0 + cw;
    let u = ch as u32;
    // Rounded corners: a quarter curve from one edge's middle to the other's.
    if (0x256d..=0x2570).contains(&u) {
        let (cx, cy) = (x0 + (cw / 2.0).floor() + light / 2.0, (lh / 2.0).floor() + light / 2.0);
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
        let cx = x0 + (cw / 2.0).floor();
        let cy = (lh / 2.0).floor();
        let heavy = light * 2.0;
        let gap = light;
        let th = |k: u8| if k == 2 { heavy } else { light };
        // Up, right, down, left.
        for (dir, &k) in w.iter().enumerate() {
            if k == 0 {
                continue;
            }
            let offsets: &[f32] = if k == 3 { &[-gap - light / 2.0, gap + light / 2.0] } else { &[0.0] };
            for &o in offsets {
                let t = th(k);
                let (a, b) = (-t / 2.0 + o, t / 2.0 + o);
                // Reach a bit past the centre so joints close.
                let reach = heavy / 2.0 + if k == 3 { gap + light } else { 0.0 };
                match dir {
                    0 => p.rect(cx + light / 2.0 + a, 0.0, cx + light / 2.0 + b, cy + light / 2.0 + reach, c),
                    1 => p.rect(cx + light / 2.0 - reach, cy + light / 2.0 + a, x1, cy + light / 2.0 + b, c),
                    2 => p.rect(cx + light / 2.0 + a, cy + light / 2.0 - reach, cx + light / 2.0 + b, lh, c),
                    _ => p.rect(x0, cy + light / 2.0 + a, cx + light / 2.0 + reach, cy + light / 2.0 + b, c),
                }
            }
        }
        return true;
    }
    // Block elements.
    let (hx, hy) = (x0 + cw / 2.0, lh / 2.0);
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
    use super::box_lines;

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
