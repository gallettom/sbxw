//! A terminal: `alacritty_terminal` keeps the grid and parses the bytes, GPUI
//! draws it, and the bytes come from (and keys go to) a [`TerminalConn`].
//!
//! Drawing is one shaped line per row, cells grouped into runs of the same
//! colours. Good enough for a spike; the real element should paint the
//! background rectangles and glyphs itself (as Zed's terminal does) and skip
//! rows that did not change.

use crate::client::{TermEvent, TermInput, TerminalConn};
use alacritty_terminal::event::{Event as TermEv, EventListener};
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line, Point};
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::{Config, Term, TermMode};
use alacritty_terminal::vte::ansi::{Color as AColor, NamedColor, Processor, Rgb, StdSyncHandler};
use futures::StreamExt;
use gpui::*;
use std::sync::mpsc;
use std::time::Instant;

pub const FONT_FAMILY: &str = "DejaVu Sans Mono";
pub const FONT_SIZE: f32 = 14.0;
pub const LINE_HEIGHT: f32 = 18.0;

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

pub struct TerminalView {
    term: Term<Listener>,
    parser: Processor<StdSyncHandler>,
    input: mpsc::Sender<TermInput>,
    focus: FocusHandle,
    size: (usize, usize),
    cell_width: Option<Pixels>,
    status: String,
    /// When the last key went out, to measure key-to-echo latency.
    key_sent: Option<Instant>,
    pub last_echo_ms: Option<f64>,
    _reader: Task<()>,
}

impl TerminalView {
    pub fn new(conn: TerminalConn, cx: &mut Context<Self>) -> Self {
        let TerminalConn { mut events, input } = conn;
        let size = (100, 30);
        let term = Term::new(Config::default(), &Size { cols: size.0, rows: size.1 }, Listener(input.clone()));
        let reader = cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            while let Some(ev) = events.next().await {
                // Take whatever else already arrived, then draw once.
                let mut batch = vec![ev];
                while let Ok(more) = events.try_recv() {
                    batch.push(more);
                }
                let alive = this.update(cx, |v, cx| {
                    for ev in batch {
                        v.on_event(ev);
                    }
                    cx.notify();
                });
                if alive.is_err() {
                    break;
                }
            }
        });
        let _ = input.send(TermInput::Resize { cols: size.0 as u16, rows: size.1 as u16 });
        Self {
            term,
            parser: Processor::new(),
            input,
            focus: cx.focus_handle(),
            size,
            cell_width: None,
            status: "connecting".into(),
            key_sent: None,
            last_echo_ms: None,
            _reader: reader,
        }
    }

    fn on_event(&mut self, ev: TermEvent) {
        match ev {
            TermEvent::Output(bytes) => {
                if let Some(t) = self.key_sent.take() {
                    let ms = t.elapsed().as_secs_f64() * 1000.0;
                    self.last_echo_ms = Some(ms);
                    log::info!("key to echo: {ms:.1} ms");
                }
                self.status = "connected".into();
                self.parser.advance(&mut self.term, &bytes);
            }
            TermEvent::Notice(t) => self.parser.advance(&mut self.term, t.as_bytes()),
            TermEvent::Attention => log::info!("bell"),
            TermEvent::Closed(why) => self.status = format!("closed: {why}"),
        }
    }

    pub fn focus_handle(&self) -> &FocusHandle {
        &self.focus
    }

    pub fn send(&mut self, bytes: Vec<u8>) {
        self.key_sent = Some(Instant::now());
        let _ = self.input.send(TermInput::Bytes(bytes));
    }

    fn resize_to(&mut self, cols: usize, rows: usize) {
        let (cols, rows) = (cols.max(10), rows.max(3));
        if (cols, rows) != self.size {
            self.size = (cols, rows);
            self.term.resize(Size { cols, rows });
            let _ = self.input.send(TermInput::Resize { cols: cols as u16, rows: rows as u16 });
        }
    }

    fn on_key(&mut self, ev: &KeyDownEvent, _w: &mut Window, cx: &mut Context<Self>) {
        let app_cursor = self.term.mode().contains(TermMode::APP_CURSOR);
        if let Some(b) = key_bytes(&ev.keystroke, app_cursor) {
            self.send(b);
            cx.stop_propagation();
        }
    }

    /// The text of the screen, row by row (for tests and the window title).
    pub fn screen_text(&self) -> String {
        let g = self.term.grid();
        (0..g.screen_lines())
            .map(|l| (0..g.columns()).map(|c| g[Line(l as i32)][Column(c)].c).collect::<String>().trim_end().to_string())
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// What a key sends to the program, xterm style.
pub fn key_bytes(k: &Keystroke, app_cursor: bool) -> Option<Vec<u8>> {
    let m = &k.modifiers;
    let arrow = |c: char| Some(if app_cursor { format!("\x1bO{c}") } else { format!("\x1b[{c}") }.into_bytes());
    let fixed: Option<&[u8]> = match k.key.as_str() {
        "enter" => Some(b"\r"),
        "backspace" => Some(b"\x7f"),
        "tab" if m.shift => Some(b"\x1b[Z"),
        "tab" => Some(b"\t"),
        "escape" => Some(b"\x1b"),
        "delete" => Some(b"\x1b[3~"),
        "home" => Some(b"\x1b[H"),
        "end" => Some(b"\x1b[F"),
        "pageup" => Some(b"\x1b[5~"),
        "pagedown" => Some(b"\x1b[6~"),
        _ => None,
    };
    if let Some(f) = fixed {
        return Some(f.to_vec());
    }
    match k.key.as_str() {
        "up" => return arrow('A'),
        "down" => return arrow('B'),
        "right" => return arrow('C'),
        "left" => return arrow('D'),
        _ => {}
    }
    if m.control && k.key.len() == 1 {
        let c = k.key.as_bytes()[0].to_ascii_lowercase();
        return match c {
            b'a'..=b'z' => Some(vec![c - b'a' + 1]),
            b'[' => Some(vec![0x1b]),
            b'\\' => Some(vec![0x1c]),
            b']' => Some(vec![0x1d]),
            b' ' | b'@' => Some(vec![0]),
            _ => None,
        };
    }
    let ch = k.key_char.clone().or_else(|| (k.key == "space").then(|| " ".to_string()))?;
    let mut out = Vec::new();
    if m.alt {
        out.push(0x1b);
    }
    out.extend_from_slice(ch.as_bytes());
    Some(out)
}

const PALETTE: [u32; 16] = [
    0x1e1e2e, 0xf38ba8, 0xa6e3a1, 0xf9e2af, 0x89b4fa, 0xf5c2e7, 0x94e2d5, 0xbac2de, //
    0x585b70, 0xf38ba8, 0xa6e3a1, 0xf9e2af, 0x89b4fa, 0xf5c2e7, 0x94e2d5, 0xa6adc8,
];
const FG: u32 = 0xcdd6f4;
const BG: u32 = 0x181825;

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

impl Focusable for TerminalView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for TerminalView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let font = Font { family: FONT_FAMILY.into(), ..font(FONT_FAMILY) };
        let size = px(FONT_SIZE);
        let cw = *self.cell_width.get_or_insert_with(|| {
            let ts = window.text_system();
            let id = ts.resolve_font(&font);
            ts.advance(id, size, 'm').map(|s| s.width).unwrap_or(px(8.4))
        });
        // The terminal takes what the window leaves after the side panel.
        let vp = window.viewport_size();
        let cols = ((vp.width - px(crate::screen::PANEL_WIDTH) - px(16.)) / cw).floor() as usize;
        let rows = ((vp.height - px(40.)) / px(LINE_HEIGHT)).floor() as usize;
        self.resize_to(cols, rows);

        let content = self.term.renderable_content();
        let colors = content.colors;
        let cursor = content.cursor.point;
        let show_cursor = content.mode.contains(TermMode::SHOW_CURSOR);
        let (ncols, nrows) = self.size;
        let mut lines: Vec<(String, Vec<TextRun>)> = vec![(String::new(), Vec::new()); nrows];
        for ic in content.display_iter {
            let Point { line, column } = ic.point;
            let row = (line.0 + content.display_offset as i32) as usize;
            if row >= nrows || column.0 >= ncols || ic.cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
                continue;
            }
            let mut fg = color(ic.cell.fg, colors, true);
            let mut bg = color(ic.cell.bg, colors, false);
            if ic.cell.flags.contains(Flags::INVERSE) {
                std::mem::swap(&mut fg, &mut bg);
            }
            if show_cursor && ic.point == cursor && self.focus.is_focused(window) {
                fg = BG;
                bg = FG;
            }
            let bold = ic.cell.flags.contains(Flags::BOLD);
            let ch = if ic.cell.c == '\0' { ' ' } else { ic.cell.c };
            let (text, runs) = &mut lines[row];
            let start = text.len();
            text.push(ch);
            let len = text.len() - start;
            let weight = if bold { FontWeight::BOLD } else { FontWeight::NORMAL };
            let bgc = (bg != BG).then(|| rgb(bg).into());
            match runs.last_mut() {
                Some(r) if r.color == Hsla::from(rgb(fg)) && r.background_color == bgc && r.font.weight == weight => r.len += len,
                _ => runs.push(TextRun {
                    len,
                    font: Font { weight, ..font.clone() },
                    color: rgb(fg).into(),
                    background_color: bgc,
                    underline: None,
                    strikethrough: None,
                }),
            }
        }
        let rows_el = lines.into_iter().map(|(text, runs)| {
            let el: AnyElement = if text.is_empty() {
                div().h(px(LINE_HEIGHT)).into_any_element()
            } else {
                div().h(px(LINE_HEIGHT)).whitespace_nowrap().child(StyledText::new(text).with_runs(runs)).into_any_element()
            };
            el
        });
        div()
            .id("terminal")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::on_key))
            .on_mouse_down(MouseButton::Left, cx.listener(|v, _, w, _| v.focus.focus(w)))
            .flex()
            .flex_col()
            .flex_grow()
            .h_full()
            .bg(rgb(BG))
            .p(px(8.))
            .font_family(FONT_FAMILY)
            .text_size(size)
            .line_height(px(LINE_HEIGHT))
            .child(
                div()
                    .text_color(rgb(0x6c7086))
                    .text_size(px(12.))
                    .child(format!("terminal {}x{}  {}", ncols, nrows, self.status)),
            )
            .children(rows_el)
    }
}

#[cfg(test)]
mod tests {
    use super::key_bytes;
    use gpui::{Keystroke, Modifiers};

    fn ks(key: &str, ch: Option<&str>, ctrl: bool) -> Keystroke {
        Keystroke {
            modifiers: Modifiers { control: ctrl, ..Default::default() },
            key: key.into(),
            key_char: ch.map(Into::into),
        }
    }

    #[test]
    fn keys_become_xterm_bytes() {
        assert_eq!(key_bytes(&ks("a", Some("a"), false), false), Some(b"a".to_vec()));
        assert_eq!(key_bytes(&ks("c", None, true), false), Some(vec![3]));
        assert_eq!(key_bytes(&ks("enter", None, false), false), Some(b"\r".to_vec()));
        assert_eq!(key_bytes(&ks("up", None, false), false), Some(b"\x1b[A".to_vec()));
        assert_eq!(key_bytes(&ks("up", None, false), true), Some(b"\x1bOA".to_vec()));
    }
}
