//! A terminal: `alacritty_terminal` keeps the grid and parses the bytes, Rune
//! draws it with its terminal element (`rune::grid`), and the bytes come from
//! (and keys go to) a [`TerminalConn`].
//!
//! Each screen row becomes one `rune::GridRow`: the row's text with colour
//! spans, background runs as fills, wide cells pinned to their column. A row
//! whose cells did not change keeps the same `Rc<GridRow>`, so Rune neither
//! shapes nor paints it again.

use crate::client::{TermEvent, TermInput, TerminalConn};
use alacritty_terminal::event::{Event as TermEv, EventListener};
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::{Config, Term, TermMode};
use alacritty_terminal::vte::ansi::{Color as AColor, NamedColor, Processor, Rgb, StdSyncHandler};
use rune::{Color, GridRow, Key, KeyEvent, Span, TextStyle};
use std::hash::{Hash, Hasher};
use std::rc::Rc;
use std::sync::mpsc;
use std::time::{Duration, Instant};

pub const FONT_SIZE: f32 = 14.0;
pub const LINE_HEIGHT: f32 = 18.0;
/// Space between the grid and the element's edges, and the status line above it.
pub const PAD: f32 = 8.0;
pub const STATUS_H: f32 = 20.0;

/// The terminal's text style: Rune's mono slot, which `main` sets to
/// [`MONO_FAMILIES`].
pub fn style() -> TextStyle {
    TextStyle::new(FONT_SIZE, 400.0).family(rune::Family::Mono).lh(LINE_HEIGHT)
}

/// The mono slot: the platform's monospace font (fontconfig's `monospace` on
/// Linux, SF Mono on macOS, Cascadia Mono or Consolas on Windows), then CJK
/// faces for wide cells, which Rune does not find on its own on Linux.
pub const MONO_FAMILIES: &str = "ui-monospace, Noto Sans Mono CJK JP, Noto Sans CJK JP, Droid Sans Fallback, Hiragino Sans, Microsoft YaHei";

/// The width of one cell, measured once from the font.
pub fn cell_width() -> f32 {
    thread_local!(static W: f32 = rune::text::advance(&"m".repeat(100), style()) / 100.0);
    W.with(|w| *w)
}

#[derive(Clone)]
struct Listener(mpsc::Sender<TermInput>);
impl EventListener for Listener {
    fn send_event(&self, ev: TermEv) {
        // Answers to the program's queries (cursor position, device attributes).
        if let TermEv::PtyWrite(s) = ev {
            let _ = self.0.send(TermInput::Bytes(s.into_bytes()));
        }
    }
}

struct Size {
    cols: usize,
    rows: usize,
}
impl Dimensions for Size {
    fn total_lines(&self) -> usize {
        self.rows
    }
    fn screen_lines(&self) -> usize {
        self.rows
    }
    fn columns(&self) -> usize {
        self.cols
    }
}

pub struct TerminalState {
    term: Term<Listener>,
    parser: Processor<StdSyncHandler>,
    input: mpsc::Sender<TermInput>,
    events: mpsc::Receiver<TermEvent>,
    pub size: (usize, usize),
    pub status: String,
    /// The rows as Rune draws them, and a hash of what each was built from.
    pub rows: Vec<Rc<GridRow>>,
    row_keys: Vec<u64>,
    pub cursor: Option<(u16, u16)>,
    dirty: bool,
    /// When the last key went out, to measure key-to-echo latency.
    key_sent: Option<Instant>,
    pub echo_ms: Vec<f64>,
    /// Rows rebuilt over the run, and how long building them took.
    pub rebuilt_rows: usize,
    pub build_time: Duration,
}

impl TerminalState {
    pub fn new(conn: TerminalConn) -> Self {
        let TerminalConn { events, input } = conn;
        let size = (100, 30);
        let term = Term::new(Config::default(), &Size { cols: size.0, rows: size.1 }, Listener(input.clone()));
        let _ = input.send(TermInput::Resize { cols: size.0 as u16, rows: size.1 as u16 });
        let mut t = Self {
            term,
            parser: Processor::new(),
            input,
            events,
            size,
            status: "connecting".into(),
            rows: Vec::new(),
            row_keys: Vec::new(),
            cursor: None,
            dirty: true,
            key_sent: None,
            echo_ms: Vec::new(),
            rebuilt_rows: 0,
            build_time: Duration::ZERO,
        };
        t.rebuild();
        t
    }

    /// Takes what the socket thread queued. With `settle`, first waits up to
    /// `settle` for output (when a key is waiting for its echo, or nothing
    /// arrived yet), then until the output has been quiet for 100 ms: headless
    /// runs use a virtual clock, so a frame must not be drawn before the shell
    /// answered. Returns true when the screen changed.
    pub fn poll(&mut self, settle: Option<Duration>) -> bool {
        let mut changed = false;
        if let Some(limit) = settle {
            let deadline = Instant::now() + limit;
            let waiting = self.key_sent.is_some() || self.status == "connecting";
            let mut got = false;
            if waiting {
                if let Ok(ev) = self.events.recv_timeout(limit) {
                    changed |= self.on_event(ev);
                    got = true;
                }
            }
            while let Ok(ev) = self.events.try_recv() {
                changed |= self.on_event(ev);
                got = true;
            }
            // Then until the output has been quiet for 100 ms (capped).
            while got {
                let left = deadline.saturating_duration_since(Instant::now()).min(Duration::from_millis(100));
                match self.events.recv_timeout(left) {
                    Ok(ev) => changed |= self.on_event(ev),
                    Err(_) => break,
                }
            }
        }
        while let Ok(ev) = self.events.try_recv() {
            changed |= self.on_event(ev);
        }
        if changed || self.dirty {
            self.rebuild();
            return true;
        }
        false
    }

    fn on_event(&mut self, ev: TermEvent) -> bool {
        match ev {
            TermEvent::Output(bytes) => {
                if let Some(t) = self.key_sent.take() {
                    let ms = t.elapsed().as_secs_f64() * 1000.0;
                    self.echo_ms.push(ms);
                    log::info!("key to echo: {ms:.1} ms");
                }
                self.status = "connected".into();
                self.parser.advance(&mut self.term, &bytes);
            }
            TermEvent::Notice(t) => self.parser.advance(&mut self.term, t.as_bytes()),
            TermEvent::Attention => log::info!("bell"),
            TermEvent::Closed(why) => self.status = format!("closed: {why}"),
        }
        true
    }

    pub fn send(&mut self, bytes: Vec<u8>) {
        self.key_sent.get_or_insert_with(Instant::now);
        let _ = self.input.send(TermInput::Bytes(bytes));
    }

    /// The terminal element was laid out at `w` x `h` points.
    pub fn fit(&mut self, w: f32, h: f32) {
        let cols = ((w - 2.0 * PAD) / cell_width()).floor().max(10.0) as usize;
        let rows = ((h - 2.0 * PAD - STATUS_H) / LINE_HEIGHT).floor().max(3.0) as usize;
        if (cols, rows) != self.size {
            log::info!("terminal resized to {cols}x{rows}");
            self.size = (cols, rows);
            self.term.resize(Size { cols, rows });
            let _ = self.input.send(TermInput::Resize { cols: cols as u16, rows: rows as u16 });
            self.dirty = true;
            self.rebuild();
        }
    }

    pub fn on_key(&mut self, ev: &KeyEvent) -> bool {
        let app_cursor = self.term.mode().contains(TermMode::APP_CURSOR);
        match key_bytes(ev, app_cursor) {
            Some(b) => {
                log::debug!("key {:?} {:?} -> {:?}", ev.key, ev.mods, String::from_utf8_lossy(&b));
                self.send(b);
                true
            }
            None => false,
        }
    }

    /// Turns the grid into Rune rows, reusing every row that did not change.
    fn rebuild(&mut self) {
        let start = Instant::now();
        self.dirty = false;
        let (ncols, nrows) = self.size;
        let colors = *self.term.colors();
        let mode = *self.term.mode();
        let cur = self.term.grid().cursor.point;
        let show_cursor = mode.contains(TermMode::SHOW_CURSOR);
        self.cursor = show_cursor.then_some((cur.column.0 as u16, cur.line.0.max(0) as u16));
        self.rows.resize_with(nrows, || Rc::new(GridRow::new("", vec![], style())));
        self.row_keys.resize(nrows, u64::MAX);
        let grid = self.term.grid();
        let style = style();
        for r in 0..nrows.min(grid.screen_lines()) {
            let line = &grid[Line(r as i32)];
            let cursor_col = (show_cursor && cur.line.0 == r as i32).then_some(cur.column.0);
            // What the row is built from: cells, colours, flags, the cursor.
            let mut h = std::collections::hash_map::DefaultHasher::new();
            for c in 0..ncols.min(grid.columns()) {
                let cell = &line[Column(c)];
                (cell.c, cell.flags.bits()).hash(&mut h);
                format!("{:?}{:?}", cell.fg, cell.bg).hash(&mut h);
            }
            cursor_col.hash(&mut h);
            let key = h.finish();
            if self.row_keys[r] == key {
                continue;
            }
            self.row_keys[r] = key;
            self.rebuilt_rows += 1;

            let mut text = String::new();
            let mut spans: Vec<Span> = Vec::new();
            let mut fills: Vec<(u16, u16, Color)> = Vec::new();
            let mut wide: Vec<(u16, char, Color)> = Vec::new();
            for c in 0..ncols.min(grid.columns()) {
                let cell = &line[Column(c)];
                if cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
                    text.push(' ');
                    continue;
                }
                let mut fg = color(cell.fg, &colors, true);
                let mut bg = color(cell.bg, &colors, false);
                if cell.flags.contains(Flags::INVERSE) {
                    std::mem::swap(&mut fg, &mut bg);
                }
                if cursor_col == Some(c) {
                    // A block cursor: the cell drawn in reverse.
                    fg = BG;
                    bg = CURSOR;
                }
                let ch = if cell.c == '\0' { ' ' } else { cell.c };
                let bold = cell.flags.contains(Flags::BOLD);
                let italic = cell.flags.contains(Flags::ITALIC);
                let underline = cell.flags.contains(Flags::UNDERLINE);
                if bg != BG {
                    match fills.last_mut() {
                        Some((_, end, col)) if *end == c as u16 && *col == rgb(bg) => *end += 1,
                        _ => fills.push((c as u16, c as u16 + 1, rgb(bg))),
                    }
                }
                if cell.flags.contains(Flags::WIDE_CHAR) {
                    // Pinned to its two columns; the line keeps a space there.
                    wide.push((c as u16, ch, rgb(fg)));
                    text.push(' ');
                    continue;
                }
                let at = text.len();
                text.push(ch);
                let end = text.len();
                let same = |s: &Span| {
                    s.end as usize == at && s.color == Some(rgb(fg)) && s.weight == bold.then_some(700.0) && s.italic == italic && s.underline.is_some() == underline
                };
                match spans.last_mut() {
                    Some(s) if same(s) => s.end = end as u32,
                    _ => {
                        let mut s = Span::new(at, end).color(rgb(fg));
                        if bold {
                            s = s.weight(700.0);
                        }
                        if italic {
                            s.italic = true;
                        }
                        if underline {
                            s.underline = Some(rgb(fg));
                        }
                        spans.push(s);
                    }
                }
            }
            let mut row = GridRow::new(text, spans, style);
            for (a, b, c) in fills {
                row = row.fill(a, b, c);
            }
            for (col, ch, c) in wide {
                row = row.cell(col, 2, ch.to_string(), style, c);
            }
            self.rows[r] = Rc::new(row);
        }
        self.build_time += start.elapsed();
    }

    /// The text of the screen, row by row (for tests and logs).
    pub fn screen_text(&self) -> String {
        let g = self.term.grid();
        (0..g.screen_lines())
            .map(|l| (0..g.columns()).map(|c| g[Line(l as i32)][Column(c)].c).collect::<String>().trim_end().to_string())
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// xterm's modifier parameter: 1 + shift + 2 alt + 4 ctrl.
fn mod_param(m: rune::Mods) -> u8 {
    1 + m.shift as u8 + 2 * m.alt as u8 + 4 * m.ctrl as u8
}

fn ctrl_byte(c: char) -> Option<u8> {
    match c.to_ascii_lowercase() {
        c @ 'a'..='z' => Some(c as u8 - b'a' + 1),
        '[' => Some(0x1b),
        '\\' => Some(0x1c),
        ']' => Some(0x1d),
        '^' | '6' => Some(0x1e),
        '_' | '-' => Some(0x1f),
        ' ' | '@' | '2' => Some(0),
        _ => None,
    }
}

/// What a key sends to the program, xterm style.
pub fn key_bytes(ev: &KeyEvent, app_cursor: bool) -> Option<Vec<u8>> {
    let m = ev.mods;
    let p = mod_param(m);
    let csi = |final_: char| {
        Some(if p > 1 {
            format!("\x1b[1;{p}{final_}")
        } else if app_cursor {
            format!("\x1bO{final_}")
        } else {
            format!("\x1b[{final_}")
        })
        .map(String::into_bytes)
    };
    let tilde = |n: u8| Some(if p > 1 { format!("\x1b[{n};{p}~") } else { format!("\x1b[{n}~") }.into_bytes());
    let alt = |mut b: Vec<u8>| {
        if m.alt {
            b.insert(0, 0x1b);
        }
        Some(b)
    };
    match &ev.key {
        Key::Enter => alt(b"\r".to_vec()),
        Key::Backspace if m.ctrl => alt(vec![0x08]),
        Key::Backspace => alt(vec![0x7f]),
        Key::Tab if m.shift => Some(b"\x1b[Z".to_vec()),
        Key::Tab => alt(b"\t".to_vec()),
        Key::Escape => Some(vec![0x1b]),
        Key::Up => csi('A'),
        Key::Down => csi('B'),
        Key::Right => csi('C'),
        Key::Left => csi('D'),
        Key::Home => csi('H'),
        Key::End => csi('F'),
        Key::Insert => tilde(2),
        Key::Delete => tilde(3),
        Key::PageUp => tilde(5),
        Key::PageDown => tilde(6),
        Key::Function(n @ 1..=4) => {
            let f = (b'P' + n - 1) as char;
            Some(if p > 1 { format!("\x1b[1;{p}{f}") } else { format!("\x1bO{f}") }.into_bytes())
        }
        Key::Function(n @ 5..=12) => tilde([15, 17, 18, 19, 20, 21, 23, 24][(*n - 5) as usize]),
        // Ctrl+<key> off macOS (Rune reports Ctrl as the primary modifier there).
        Key::Command(c) if m.ctrl => alt(vec![ctrl_byte(*c)?]),
        // ⌘ shortcuts on macOS belong to the app, not the shell.
        Key::Command(_) => None,
        Key::Text(s) if m.ctrl && s.chars().count() == 1 => alt(vec![ctrl_byte(s.chars().next()?)?]),
        Key::Text(s) => alt(s.as_bytes().to_vec()),
        _ => None,
    }
}

const PALETTE: [u32; 16] = [
    0x1e1e2e, 0xf38ba8, 0xa6e3a1, 0xf9e2af, 0x89b4fa, 0xf5c2e7, 0x94e2d5, 0xbac2de, //
    0x585b70, 0xf38ba8, 0xa6e3a1, 0xf9e2af, 0x89b4fa, 0xf5c2e7, 0x94e2d5, 0xa6adc8,
];
pub const FG: u32 = 0xcdd6f4;
pub const BG: u32 = 0x181825;
const CURSOR: u32 = 0xf5e0dc;

/// A 0xRRGGBB colour as Rune's colour.
pub fn rgb(c: u32) -> Color {
    rune::color::hex(&format!("#{c:06x}")).expect("hex colour")
}

fn rgb_of(r: Rgb) -> u32 {
    (r.r as u32) << 16 | (r.g as u32) << 8 | r.b as u32
}

fn indexed(i: u8) -> u32 {
    match i {
        0..=15 => PALETTE[i as usize],
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

fn color(c: AColor, colors: &alacritty_terminal::term::color::Colors, fg: bool) -> u32 {
    match c {
        AColor::Spec(r) => rgb_of(r),
        AColor::Indexed(i) => colors[i as usize].map(rgb_of).unwrap_or_else(|| indexed(i)),
        AColor::Named(n) => {
            if let Some(r) = colors[n] {
                return rgb_of(r);
            }
            match n {
                NamedColor::Foreground | NamedColor::BrightForeground => FG,
                NamedColor::Background => BG,
                NamedColor::DimForeground => 0xa6adc8,
                n if (n as usize) < 16 => PALETTE[n as usize],
                n if (n as usize) >= NamedColor::DimBlack as usize && (n as usize) <= NamedColor::DimWhite as usize => {
                    PALETTE[n as usize - NamedColor::DimBlack as usize]
                }
                _ if fg => FG,
                _ => BG,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::key_bytes;
    use rune::{Key, KeyEvent, Mods};

    fn k(key: Key, f: impl FnOnce(&mut Mods)) -> KeyEvent {
        let mut mods = Mods::default();
        f(&mut mods);
        KeyEvent { key, mods }
    }

    #[test]
    fn keys_become_xterm_bytes() {
        let none = |_: &mut Mods| {};
        assert_eq!(key_bytes(&k(Key::Text("a".into()), none), false), Some(b"a".to_vec()));
        assert_eq!(key_bytes(&k(Key::Text("A".into()), |m| m.shift = true), false), Some(b"A".to_vec()));
        assert_eq!(key_bytes(&k(Key::Command('c'), |m| (m.ctrl, m.cmd) = (true, true)), false), Some(vec![3]));
        assert_eq!(key_bytes(&k(Key::Command('c'), |m| (m.logo, m.cmd) = (true, true)), false), None);
        assert_eq!(key_bytes(&k(Key::Enter, none), false), Some(b"\r".to_vec()));
        assert_eq!(key_bytes(&k(Key::Up, none), false), Some(b"\x1b[A".to_vec()));
        assert_eq!(key_bytes(&k(Key::Up, none), true), Some(b"\x1bOA".to_vec()));
        assert_eq!(key_bytes(&k(Key::Right, |m| m.ctrl = true), false), Some(b"\x1b[1;5C".to_vec()));
        assert_eq!(key_bytes(&k(Key::Tab, |m| m.shift = true), false), Some(b"\x1b[Z".to_vec()));
        assert_eq!(key_bytes(&k(Key::Text("b".into()), |m| m.alt = true), false), Some(b"\x1bb".to_vec()));
    }

    /// Full-screen redraw cost, CPU side: every row of a 111x42 screen changes
    /// each frame (colours on every cell, like `htop` or a `cat` of coloured
    /// output), then Rune lays out and builds the frame's instances. GPU
    /// raster and presentation are not included.
    /// `cargo test --release --lib full_screen -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn full_screen_redraw_cost() {
        use super::*;
        use crate::client::{TermEvent, TerminalConn};
        use rune::{Cx, El, Env, Runtime};
        use std::time::Instant;

        struct S(TerminalState);
        fn view(s: &S, _: &mut Cx<S>) -> El {
            let (c, r) = s.0.size;
            rune::grid(c as u16, cell_width(), LINE_HEIGHT, s.0.rows.clone()).w(c as f32 * cell_width()).h(r as f32 * LINE_HEIGHT)
        }
        let (ev_tx, ev_rx) = mpsc::channel();
        let (in_tx, _in_rx) = mpsc::channel();
        let mut t = TerminalState::new(TerminalConn { events: ev_rx, input: in_tx });
        t.fit(PAD * 2.0 + 111.0 * cell_width() + 0.5, PAD * 2.0 + STATUS_H + 42.0 * LINE_HEIGHT + 0.5);
        assert_eq!(t.size, (111, 42));
        let frame_bytes = |k: usize| {
            let mut out = String::from("\x1b[H");
            for r in 0..42 {
                for c in 0..111 {
                    let col = 31 + (r + c + k) % 7;
                    out += &format!("\x1b[{col}m{}", (b'a' + ((r * 7 + c + k) % 26) as u8) as char);
                }
                if r < 41 {
                    out += "\r\n";
                }
            }
            out.into_bytes()
        };
        let mut rt = Runtime::new(S(t), view, (960.0, 800.0), Env::new(true, 2.0));
        rt.frame(0.0);
        let (mut feed, mut rebuild, mut frame) = (vec![], vec![], vec![]);
        for k in 0..120 {
            let bytes = frame_bytes(k);
            let a = Instant::now();
            ev_tx.send(TermEvent::Output(bytes)).unwrap();
            let mut changed = false;
            rt.update(|s, _| changed = s.0.poll(None));
            let b = Instant::now();
            rt.frame(1.0 / 60.0);
            let c = Instant::now();
            assert!(changed);
            feed.push((b - a).as_secs_f64() * 1e3);
            frame.push((c - b).as_secs_f64() * 1e3);
            rebuild.push(0.0);
        }
        let med = |v: &mut Vec<f64>| {
            v.sort_by(|a, b| a.partial_cmp(b).unwrap());
            (v[v.len() / 2], v[v.len() * 95 / 100])
        };
        let _ = &mut rebuild;
        let (f50, f95) = med(&mut feed);
        let (r50, r95) = med(&mut frame);
        println!("111x42, every row changed, 120 frames, scale 2:");
        println!("  parse + rebuild rows: p50 {f50:.2} ms, p95 {f95:.2} ms");
        println!("  Rune layout + paint (CPU): p50 {r50:.2} ms, p95 {r95:.2} ms");
        println!("  total p50 {:.2} ms", f50 + r50);
        println!("  instances last frame: {}", rt.instances.len());
    }
}
