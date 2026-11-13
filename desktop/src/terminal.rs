//! A terminal: `alacritty_terminal` keeps the grid and parses the bytes, Rune
//! draws it with its terminal element (`rune::grid`), and the bytes come from
//! (and keys go to) a [`TerminalConn`], the daemon's terminal WebSocket.
//!
//! # Public API
//!
//! Building one:
//! * [`TerminalState::new`]`(conn)`: one connection, no reconnect.
//! * [`TerminalState::with_reconnect`]`(conn, reopen)`: reopens the socket after
//!   it drops, with backoff; the daemon replays the session's last 256 KB first
//!   and the terminal starts from a fresh grid, so nothing is duplicated.
//! * [`TerminalState::set_theme`] with a [`Theme`] (the web UI's defaults, or
//!   [`Theme::from_setting_strings`] for sbxw's `terminal_color` settings).
//!
//! Every frame:
//! * [`TerminalState::poll_tx`]`(settle, clock, tx)`: drains the socket,
//!   applies the debounced resize, blinks the cursor, retries a dropped
//!   connection; returns true when the rows changed. [`TerminalState::poll`]
//!   (no `Tx`) still works: no blink, and resizes go out on the next poll.
//! * [`pane`]`(t, lens)`: the whole pane (status line, grid, padding, theme
//!   background, layout to size) with keys, mouse, wheel, selection, paste and
//!   the input method wired; [`view`] is the grid element alone. `lens` finds
//!   the terminal in the app's state.
//!
//! Input, for callers that route events themselves:
//! * [`TerminalState::on_key`] (keys and scrollback keys) and
//!   [`TerminalState::on_key_tx`] (plus copy, paste, select all through Rune's
//!   clipboard), [`TerminalState::on_ime`] (dead keys, CJK, emoji picker: the
//!   app must register it with `rune::App::on_ime`), [`TerminalState::paste`],
//!   [`TerminalState::set_focused`] (focus reports, hollow cursor),
//!   [`TerminalState::fit`] / [`TerminalState::fit_at`] (size in points).
//! * Scrollback: [`TerminalState::scroll_lines`], `scroll_page`,
//!   `scroll_to_bottom`, `scroll_to_top`, `display_offset`, `history_len`.
//! * Selection: `select_start`, `select_update`, `select_all`,
//!   `clear_selection`, `selection_text`.
//!
//! Out to the shell: [`TerminalState::take_events`] gives [`TerminalEvent`]s
//! (the daemon's `attention` frame, bell, title, OSC 52 copy requests,
//! reconnects). Fields kept from the spike: `size`, `status`, `rows`,
//! `cursor`, `echo_ms`, `rebuilt_rows`, `build_time`; and `screen_text`.
//!
//! # Drawing
//!
//! Each screen row becomes one `rune::GridRow` ([`render`]): the row's text
//! with colour spans, background runs and the selection as fills, wide cells
//! pinned to their column, a vector layer for box drawing and fancy
//! underlines. Only rows alacritty reports as damaged, or whose cursor,
//! selection or hovered link changed, are rebuilt; an unchanged row keeps its
//! `Rc<GridRow>`, so Rune neither shapes nor paints it again.

pub mod keys;
pub mod mouse;
pub mod render;
pub mod theme;
mod view;

pub use keys::{key_bytes, key_bytes_with, paste_bytes, KeyOpts, Shortcut};
pub use render::{rgb, CursorDraw};
pub use theme::Theme;
pub use view::{pane, view};

use crate::client::{TermEvent, TermInput, TerminalConn};
use alacritty_terminal::event::{Event as TermEv, EventListener, WindowSize};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Line, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::term::{Config, Term, TermDamage, TermMode};
use alacritty_terminal::vte::ansi::{CursorShape, NamedColor, Processor, Rgb, StdSyncHandler};
use rune::{GridRow, KeyEvent, TextStyle, Tx};
use std::hash::{Hash, Hasher};
use std::rc::Rc;
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

pub const FONT_SIZE: f32 = 14.0;
pub const LINE_HEIGHT: f32 = 18.0;
/// Space between the grid and the element's edges, and the status line above it.
pub const PAD: f32 = 8.0;
pub const STATUS_H: f32 = 20.0;
/// Lines of history, as the web UI's panes (`scrollback: 10000`).
pub const SCROLLBACK: usize = 10_000;
/// How long a size must hold before the daemon is told (a window drag sends
/// one resize, not one a frame).
pub const RESIZE_DEBOUNCE: f64 = 0.08;
/// Half a blink period, in seconds.
pub const BLINK: f64 = 0.53;

/// The web UI's default text and background (see [`Theme`]).
pub const FG: u32 = 0xc9d1d9;
pub const BG: u32 = theme::WEB_BACKGROUND;

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

/// Reopens the terminal's socket (on a background thread): usually
/// `Daemon::open_terminal` with the same request and waker.
pub type Reconnect = Arc<dyn Fn() -> anyhow::Result<TerminalConn> + Send + Sync>;

/// What the terminal tells the shell around it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TerminalEvent {
    /// The daemon's `{"type":"attention"}` frame: the session wants a person.
    Attention,
    /// BEL from the program.
    Bell,
    /// The program set the window title (OSC 0/2).
    Title(String),
    /// The program asked to copy this text (OSC 52). The shell decides
    /// (the web UI copies short text and asks for long text).
    Clipboard(String),
    /// The socket dropped; a reconnect is planned.
    Disconnected(String),
    /// A new socket is open; the daemon's replay follows.
    Reconnected,
}

/// Where input goes: `TerminalConn::input` behind its `send`, whatever its
/// type (the closure is built where the connection is taken apart, so
/// client.rs can change the sender's type without touching this module).
#[derive(Clone)]
struct Input(Arc<dyn Fn(TermInput) + Send + Sync>);

impl Input {
    fn send(&self, i: TermInput) {
        (self.0)(i)
    }
}

#[derive(Clone)]
struct Listener {
    input: Input,
    events: mpsc::Sender<TermEv>,
}
impl EventListener for Listener {
    fn send_event(&self, ev: TermEv) {
        match ev {
            // Answers to the program's queries (cursor position, device attributes).
            TermEv::PtyWrite(s) => {
                self.input.send(TermInput::Bytes(s.into_bytes()));
            }
            TermEv::MouseCursorDirty | TermEv::Wakeup | TermEv::CursorBlinkingChange => {}
            ev => {
                let _ = self.events.send(ev);
            }
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

/// One phase of a pointer gesture on the grid, in the grid's points (what
/// Rune's `Drag` carries).
#[derive(Clone, Copy, Debug)]
pub struct DragInput {
    pub phase: rune::Phase,
    pub x: f32,
    pub y: f32,
    pub start_x: f32,
    pub start_y: f32,
    pub dx: f32,
    pub dy: f32,
    /// 1, 2, 3: single, double, triple click.
    pub clicks: u32,
}

/// A drag or press of the mouse in progress.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Gesture {
    Select,
    Report(mouse::Button),
}

pub struct TerminalState {
    term: Term<Listener>,
    parser: Processor<StdSyncHandler>,
    term_events: mpsc::Receiver<TermEv>,
    input: Input,
    events: mpsc::Receiver<TermEvent>,
    pub size: (usize, usize),
    pub status: String,
    /// The rows as Rune draws them.
    pub rows: Vec<Rc<GridRow>>,
    /// What each row was built from besides its cells (cursor, selection, link).
    row_extra: Vec<u64>,
    /// The cursor's cell on screen, for screen readers (`grid_caret`).
    pub cursor: Option<(u16, u16)>,
    dirty: bool,
    force_all: bool,
    theme: Theme,
    pub keys: KeyOpts,
    /// When the first key still waiting for its echo went out.
    key_sent: Option<Instant>,
    pub echo_ms: Vec<f64>,
    /// Rows rebuilt over the run, rebuilds, and how long building them took.
    pub rebuilt_rows: usize,
    pub rebuilds: usize,
    pub build_time: Duration,
    /// Output bytes parsed over the run.
    pub bytes_in: usize,
    wheel_acc: f32,
    gesture: Option<Gesture>,
    last_report_cell: Option<(usize, usize)>,
    hover_link: Option<(usize, usize, usize, String)>,
    /// The size the daemon has, and when the current one was set (app clock;
    /// `Some(None)`: send on the next poll).
    sent_size: Option<(usize, usize)>,
    resize_at: Option<Option<f64>>,
    reconnect: Option<Reconnect>,
    attempt: Option<mpsc::Receiver<anyhow::Result<TerminalConn>>>,
    retry_at: Option<Instant>,
    backoff: Duration,
    out: Vec<TerminalEvent>,
    focused: bool,
    /// Blink like the web UI's panes (`cursorBlink: true`); programs can ask
    /// for it too (DECSCUSR).
    pub cursor_blink: bool,
    blink_epoch: f64,
    blink_on: bool,
    clock: f64,
    /// The input method's composition, drawn at the cursor.
    pub preedit: Option<String>,
    /// Suffix for the element keys when a window has several terminals
    /// (`grid-<id>`, `terminal-<id>`); empty: `grid`, `terminal`.
    pub id: String,
}

fn new_term(size: (usize, usize), input: &Input, events: &mpsc::Sender<TermEv>) -> Term<Listener> {
    let config = Config { scrolling_history: SCROLLBACK, ..Config::default() };
    Term::new(config, &Size { cols: size.0, rows: size.1 }, Listener { input: input.clone(), events: events.clone() })
}

impl TerminalState {
    pub fn new(conn: TerminalConn) -> Self {
        let TerminalConn { events, input } = conn;
        let input = Input(Arc::new(move |i| {
            let _ = input.send(i);
        }));
        let size = (100, 30);
        let (tev_tx, tev_rx) = mpsc::channel();
        let term = new_term(size, &input, &tev_tx);
        let mut t = Self {
            term,
            parser: Processor::new(),
            term_events: tev_rx,
            input,
            events,
            size,
            status: "connecting".into(),
            rows: Vec::new(),
            row_extra: Vec::new(),
            cursor: None,
            dirty: true,
            force_all: true,
            theme: Theme::default(),
            keys: KeyOpts::default(),
            key_sent: None,
            echo_ms: Vec::new(),
            rebuilt_rows: 0,
            rebuilds: 0,
            build_time: Duration::ZERO,
            bytes_in: 0,
            wheel_acc: 0.0,
            gesture: None,
            last_report_cell: None,
            hover_link: None,
            sent_size: None,
            resize_at: Some(None),
            reconnect: None,
            attempt: None,
            retry_at: None,
            backoff: Duration::from_millis(500),
            out: Vec::new(),
            focused: true,
            cursor_blink: true,
            blink_epoch: 0.0,
            blink_on: true,
            clock: 0.0,
            preedit: None,
            id: String::new(),
        };
        t.flush_resize();
        t.rebuild();
        t
    }

    /// A terminal that reopens its socket with `reopen` when it drops.
    pub fn with_reconnect(conn: TerminalConn, reopen: Reconnect) -> Self {
        let mut t = Self::new(conn);
        t.reconnect = Some(reopen);
        t
    }

    /// An element key for this terminal: `base`, or `base-<id>`.
    pub fn key(&self, base: &str) -> String {
        if self.id.is_empty() {
            base.to_string()
        } else {
            format!("{base}-{}", self.id)
        }
    }

    pub fn theme(&self) -> &Theme {
        &self.theme
    }

    /// New colours; every row is drawn again.
    pub fn set_theme(&mut self, theme: Theme) {
        if theme != self.theme {
            self.theme = theme;
            self.force_all = true;
            self.dirty = true;
        }
    }

    /// The events since the last call.
    pub fn take_events(&mut self) -> Vec<TerminalEvent> {
        std::mem::take(&mut self.out)
    }

    /// [`Self::poll_tx`] without a `Tx`: no blink, no wake-ups, and a pending
    /// resize goes out at once.
    pub fn poll(&mut self, settle: Option<Duration>) -> bool {
        if self.resize_at.is_some() {
            self.flush_resize();
        }
        self.poll_inner(settle, None)
    }

    /// Takes what the socket thread queued. With `settle`, first waits up to
    /// `settle` for output (when a key is waiting for its echo, or nothing
    /// arrived yet), then until the output has been quiet for 100 ms: headless
    /// runs use a virtual clock, so a frame must not be drawn before the shell
    /// answered. `clock` is the app clock (`poll`'s second argument). Asks `tx`
    /// for the wake-ups the debounce, the blink and a reconnect need. Returns
    /// true when the rows changed.
    pub fn poll_tx(&mut self, settle: Option<Duration>, clock: f64, tx: &mut Tx) -> bool {
        self.clock = clock;
        match self.resize_at {
            Some(None) => self.flush_resize(),
            Some(Some(at)) if clock - at >= RESIZE_DEBOUNCE => self.flush_resize(),
            Some(Some(at)) => tx.wake_after(Duration::from_secs_f64(RESIZE_DEBOUNCE - (clock - at))),
            None => {}
        }
        let changed = self.poll_inner(settle, Some(tx));
        // Blink while the cursor shows; it stays lit for a period after activity.
        let blinking = self.cursor_blink || self.term.cursor_style().blinking;
        if blinking && self.focused && self.cursor.is_some() {
            let since = (clock - self.blink_epoch).max(0.0);
            let on = (since / BLINK) as u64 % 2 == 0;
            if on != self.blink_on {
                self.blink_on = on;
                self.dirty = true;
            }
            tx.wake_after(Duration::from_secs_f64(BLINK - since % BLINK + 0.001));
        } else if !self.blink_on {
            self.blink_on = true;
            self.dirty = true;
        }
        if self.dirty {
            self.rebuild();
            return true;
        }
        changed
    }

    fn poll_inner(&mut self, settle: Option<Duration>, tx: Option<&mut Tx>) -> bool {
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
        loop {
            match self.events.try_recv() {
                Ok(ev) => changed |= self.on_event(ev),
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    let closed = self.status.starts_with("closed") || self.status.starts_with("disconnected") || self.status.starts_with("reconnect");
                    if !closed {
                        changed |= self.on_event(TermEvent::Closed("connection lost".into()));
                    }
                    break;
                }
            }
        }
        changed |= self.drain_term_events();
        changed |= self.step_reconnect(tx);
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
                    log::debug!("key to echo: {ms:.1} ms");
                }
                if self.status != "connected" {
                    self.status = "connected".into();
                }
                self.bytes_in += bytes.len();
                self.blink_epoch = self.clock;
                self.parser.advance(&mut self.term, &bytes);
            }
            TermEvent::Notice(t) => self.parser.advance(&mut self.term, t.as_bytes()),
            TermEvent::Attention => {
                self.out.push(TerminalEvent::Attention);
                return false;
            }
            TermEvent::Closed(why) => {
                self.key_sent = None;
                if self.reconnect.is_some() {
                    self.status = format!("disconnected ({why}), reconnecting");
                    self.retry_at = Some(Instant::now() + self.backoff);
                    self.out.push(TerminalEvent::Disconnected(why));
                } else {
                    self.status = format!("closed: {why}");
                }
            }
        }
        true
    }

    /// What the emulator asked of the app (titles, bell, OSC 52, colour and
    /// size queries).
    fn drain_term_events(&mut self) -> bool {
        let mut changed = false;
        while let Ok(ev) = self.term_events.try_recv() {
            match ev {
                TermEv::Title(t) => self.out.push(TerminalEvent::Title(t)),
                TermEv::ResetTitle => self.out.push(TerminalEvent::Title(String::new())),
                TermEv::Bell => self.out.push(TerminalEvent::Bell),
                TermEv::ClipboardStore(_, text) => self.out.push(TerminalEvent::Clipboard(text)),
                // Never hand the clipboard to the sandbox.
                TermEv::ClipboardLoad(..) => {}
                TermEv::ColorRequest(i, f) => {
                    let c = self.term.colors()[i].map(|r| (r.r as u32) << 16 | (r.g as u32) << 8 | r.b as u32).unwrap_or_else(|| match i {
                        i if i < 256 => self.theme.indexed(i as u8),
                        i if i == NamedColor::Background as usize => self.theme.background,
                        i if i == NamedColor::Cursor as usize => self.theme.cursor,
                        _ => self.theme.foreground,
                    });
                    let rgb = Rgb { r: (c >> 16) as u8, g: (c >> 8) as u8, b: c as u8 };
                    self.input.send(TermInput::Bytes(f(rgb).into_bytes()));
                }
                TermEv::TextAreaSizeRequest(f) => {
                    let ws = WindowSize {
                        num_lines: self.size.1 as u16,
                        num_cols: self.size.0 as u16,
                        cell_width: cell_width() as u16,
                        cell_height: LINE_HEIGHT as u16,
                    };
                    self.input.send(TermInput::Bytes(f(ws).into_bytes()));
                }
                _ => {}
            }
            changed = true;
        }
        changed
    }

    /// Retries a dropped socket when it is time, and takes a new one when the
    /// attempt is done: a fresh grid, the size, then the daemon's replay.
    fn step_reconnect(&mut self, tx: Option<&mut Tx>) -> bool {
        let Some(reopen) = self.reconnect.clone() else { return false };
        let mut changed = false;
        if let Some(rx) = &self.attempt {
            match rx.try_recv() {
                Ok(Ok(conn)) => {
                    self.attempt = None;
                    self.adopt(conn);
                    self.out.push(TerminalEvent::Reconnected);
                    changed = true;
                }
                Ok(Err(e)) => {
                    self.attempt = None;
                    self.backoff = (self.backoff * 2).min(Duration::from_secs(10));
                    self.retry_at = Some(Instant::now() + self.backoff);
                    self.status = format!("reconnect failed ({e:#}), retrying in {:.0} s", self.backoff.as_secs_f64());
                    changed = true;
                }
                Err(mpsc::TryRecvError::Empty) => {}
                Err(mpsc::TryRecvError::Disconnected) => self.attempt = None,
            }
        } else if let Some(at) = self.retry_at {
            if Instant::now() >= at {
                self.retry_at = None;
                let (rtx, rrx) = mpsc::channel();
                self.attempt = Some(rrx);
                self.status = "reconnecting".into();
                let spawned = std::thread::Builder::new().name("terminal-reconnect".into()).spawn(move || {
                    let _ = rtx.send(reopen());
                });
                if spawned.is_err() {
                    self.attempt = None;
                    self.retry_at = Some(Instant::now() + self.backoff);
                }
                changed = true;
            }
        }
        if let Some(tx) = tx {
            if self.attempt.is_some() {
                tx.wake_after(Duration::from_millis(30));
            } else if let Some(at) = self.retry_at {
                tx.wake_after(at.saturating_duration_since(Instant::now()));
            }
        }
        changed
    }

    /// Waits (real time, up to `limit`) for a reconnect in progress to finish:
    /// headless runs and tests, whose clock does not wait for threads.
    pub fn settle_reconnect(&mut self, limit: Duration) {
        let deadline = Instant::now() + limit;
        while (self.retry_at.is_some() || self.attempt.is_some()) && Instant::now() < deadline {
            if let Some(at) = self.retry_at {
                std::thread::sleep(at.saturating_duration_since(Instant::now()).min(deadline.saturating_duration_since(Instant::now())));
            }
            if let Some(rx) = &self.attempt {
                match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                    Ok(r) => {
                        let (t, r2) = mpsc::channel();
                        let _ = t.send(r);
                        self.attempt = Some(r2);
                    }
                    Err(_) => break,
                }
            }
            self.step_reconnect(None);
        }
    }

    /// Switches to a new connection: the old grid and history go (the
    /// daemon's replay repaints them), as the web UI's `term.reset()` before
    /// a reconnect.
    fn adopt(&mut self, conn: TerminalConn) {
        let TerminalConn { events, input } = conn;
        self.events = events;
        self.input = Input(Arc::new(move |i| {
            let _ = input.send(i);
        }));
        let (tev_tx, tev_rx) = mpsc::channel();
        self.term = new_term(self.size, &self.input, &tev_tx);
        self.term_events = tev_rx;
        self.parser = Processor::new();
        self.gesture = None;
        self.hover_link = None;
        self.backoff = Duration::from_millis(500);
        self.status = "connected".into();
        self.sent_size = None;
        self.flush_resize();
        self.force_all = true;
        self.dirty = true;
    }

    /// Tells the daemon the current size, if it changed.
    fn flush_resize(&mut self) {
        self.resize_at = None;
        if self.sent_size != Some(self.size) {
            self.sent_size = Some(self.size);
            log::info!("terminal resized to {}x{}", self.size.0, self.size.1);
            self.input.send(TermInput::Resize { cols: self.size.0 as u16, rows: self.size.1 as u16 });
        }
    }

    /// Sends bytes as typed input: the view jumps back to the bottom and the
    /// selection goes, as in xterm.js.
    pub fn send(&mut self, bytes: Vec<u8>) {
        self.key_sent.get_or_insert_with(Instant::now);
        self.blink_epoch = self.clock;
        if self.term.grid().display_offset() != 0 {
            self.term.scroll_display(Scroll::Bottom);
            self.dirty = true;
        }
        if self.term.selection.take().is_some() {
            self.dirty = true;
        }
        self.input.send(TermInput::Bytes(bytes));
    }

    /// Bytes the program asked for (mouse and focus reports): no jump.
    fn send_report(&mut self, bytes: Vec<u8>) {
        self.input.send(TermInput::Bytes(bytes));
    }

    /// The terminal element was laid out at `w` x `h` points. The grid
    /// follows at once; the daemon hears on the next poll.
    pub fn fit(&mut self, w: f32, h: f32) {
        if self.fit_inner(w, h) {
            self.resize_at = Some(None);
        }
    }

    /// [`Self::fit`] at app clock `clock`: the daemon hears once the size has
    /// held for [`RESIZE_DEBOUNCE`] (with [`Self::poll_tx`]).
    pub fn fit_at(&mut self, w: f32, h: f32, clock: f64) {
        if self.fit_inner(w, h) {
            self.resize_at = Some(Some(clock));
        }
    }

    fn fit_inner(&mut self, w: f32, h: f32) -> bool {
        let cols = ((w - 2.0 * PAD) / cell_width()).floor().max(10.0) as usize;
        let rows = ((h - 2.0 * PAD - STATUS_H) / LINE_HEIGHT).floor().max(3.0) as usize;
        self.resize(cols, rows)
    }

    /// Resizes the grid to `cols` x `rows` cells (the daemon hears on the
    /// next poll).
    pub fn resize(&mut self, cols: usize, rows: usize) -> bool {
        if (cols, rows) == self.size {
            return false;
        }
        self.size = (cols, rows);
        self.term.resize(Size { cols, rows });
        self.hover_link = None;
        if self.resize_at.is_none() {
            self.resize_at = Some(None);
        }
        self.dirty = true;
        self.rebuild();
        true
    }

    /// Resizes the daemon has been sent (for tests and measurements).
    pub fn sent_size(&self) -> Option<(usize, usize)> {
        self.sent_size
    }

    pub fn mode(&self) -> TermMode {
        *self.term.mode()
    }

    // ---- keys ----

    /// A key: scrollback keys (Shift+PageUp/PageDown/Home/End) move the view,
    /// everything else goes to the program. Returns true when taken.
    pub fn on_key(&mut self, ev: &KeyEvent) -> bool {
        let mac = self.keys.mac;
        let alt_screen = self.mode().contains(TermMode::ALT_SCREEN);
        match keys::shortcut(ev, mac) {
            // A full-screen program has no history: it gets the keys.
            Some(Shortcut::ScrollPage(_) | Shortcut::ScrollTop | Shortcut::ScrollBottom) if alt_screen => {}
            Some(Shortcut::ScrollPage(n)) => {
                self.scroll_page(n);
                return true;
            }
            Some(Shortcut::ScrollTop) => {
                self.scroll_to_top();
                return true;
            }
            Some(Shortcut::ScrollBottom) => {
                self.scroll_to_bottom();
                return true;
            }
            _ => {}
        }
        let app_cursor = self.mode().contains(TermMode::APP_CURSOR);
        match key_bytes_with(ev, app_cursor, self.keys) {
            Some(b) => {
                log::debug!("key {:?} {:?} -> {:?}", ev.key, ev.mods, String::from_utf8_lossy(&b));
                self.send(b);
                true
            }
            None => false,
        }
    }

    /// [`Self::on_key`] plus the platform's copy, paste and select-all
    /// shortcuts, through Rune's clipboard. `lens` finds this terminal in the
    /// app's state when the clipboard answers.
    pub fn on_key_tx<S: 'static, L>(&mut self, ev: &KeyEvent, tx: &mut Tx, lens: L) -> bool
    where
        L: Fn(&mut S) -> Option<&mut TerminalState> + 'static,
    {
        match keys::shortcut(ev, self.keys.mac) {
            Some(Shortcut::Copy) => {
                let text = self.selection_text();
                log::debug!("copy: {} bytes", text.as_ref().map_or(0, String::len));
                if let Some(text) = text {
                    tx.copy(text);
                }
                return true;
            }
            Some(Shortcut::Paste) => {
                tx.read_clipboard(move |s: &mut S, content: &rune::ClipboardContent, _tx: &mut Tx| {
                    log::debug!("paste: {} bytes", content.text().len());
                    if let Some(t) = lens(s) {
                        t.paste(content.text());
                    }
                });
                return true;
            }
            Some(Shortcut::SelectAll) => {
                self.select_all();
                return true;
            }
            _ => {}
        }
        self.on_key(ev)
    }

    /// Pastes `text` (bracketed when the program asked).
    pub fn paste(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        let bracketed = self.mode().contains(TermMode::BRACKETED_PASTE);
        self.send(paste_bytes(text, bracketed));
    }

    /// The input method (registered with `rune::App::on_ime`): a commit is
    /// typed, a composition is drawn at the cursor.
    pub fn on_ime(&mut self, ime: &rune::Ime) -> bool {
        match ime {
            rune::Ime::Preedit(text, ..) => {
                self.preedit = (!text.is_empty()).then(|| text.clone());
                self.dirty = true;
            }
            rune::Ime::Commit(text) => {
                self.preedit = None;
                self.send(text.as_bytes().to_vec());
            }
            rune::Ime::Disabled => {
                self.preedit = None;
                self.dirty = true;
            }
            _ => return false,
        }
        true
    }

    /// The window (or the terminal) took or lost the keyboard: a focus report
    /// when the program asked for them (DECSET 1004), and a hollow cursor.
    pub fn set_focused(&mut self, focused: bool) {
        if focused == self.focused {
            return;
        }
        self.focused = focused;
        self.term.is_focused = focused;
        if self.mode().contains(TermMode::FOCUS_IN_OUT) {
            self.send_report(if focused { b"\x1b[I".to_vec() } else { b"\x1b[O".to_vec() });
        }
        self.blink_epoch = self.clock;
        self.dirty = true;
    }

    pub fn focused(&self) -> bool {
        self.focused
    }

    // ---- scrollback ----

    /// How many lines the view is scrolled up into the history.
    pub fn display_offset(&self) -> usize {
        self.term.grid().display_offset()
    }

    /// Lines of history kept above the screen.
    pub fn history_len(&self) -> usize {
        self.term.grid().history_size()
    }

    /// Scrolls the view `n` lines up into the history (negative: down).
    pub fn scroll_lines(&mut self, n: i32) {
        let before = self.display_offset();
        self.term.scroll_display(Scroll::Delta(n));
        if self.display_offset() != before {
            self.hover_link = None;
            self.dirty = true;
        }
    }

    /// Scrolls by pages: negative up, positive down.
    pub fn scroll_page(&mut self, pages: i32) {
        let page = self.size.1.saturating_sub(1).max(1) as i32;
        self.scroll_lines(-pages * page);
    }

    pub fn scroll_to_bottom(&mut self) {
        self.scroll_lines(-(self.display_offset() as i32));
    }

    pub fn scroll_to_top(&mut self) {
        self.scroll_lines(self.history_len() as i32);
    }

    // ---- selection ----

    /// The viewport cell under (`x`, `y`) points from the grid's top-left,
    /// as a grid point (history lines are negative) and the half of the cell.
    pub fn point_at(&self, x: f32, y: f32) -> (Point, Side) {
        let cw = cell_width();
        let col = (x / cw).floor().clamp(0.0, self.size.0 as f32 - 1.0) as usize;
        let row = (y / LINE_HEIGHT).floor().clamp(0.0, self.size.1 as f32 - 1.0) as i32;
        let side = if x - col as f32 * cw < cw / 2.0 { Side::Left } else { Side::Right };
        (Point::new(Line(row - self.display_offset() as i32), Column(col)), side)
    }

    /// Starts a selection at (`x`, `y`): one click selects characters, two
    /// words, three lines.
    pub fn select_start(&mut self, x: f32, y: f32, clicks: u32) {
        let (p, side) = self.point_at(x, y);
        let ty = match clicks {
            0 | 1 => SelectionType::Simple,
            2 => SelectionType::Semantic,
            _ => SelectionType::Lines,
        };
        self.term.selection = Some(Selection::new(ty, p, side));
        self.dirty = true;
    }

    /// Extends the selection to (`x`, `y`); past the top or bottom edge the
    /// view scrolls a line.
    pub fn select_update(&mut self, x: f32, y: f32) {
        let h = self.size.1 as f32 * LINE_HEIGHT;
        if y < 0.0 {
            self.scroll_lines(1);
        } else if y >= h {
            self.scroll_lines(-1);
        }
        let (p, side) = self.point_at(x, y);
        if let Some(sel) = &mut self.term.selection {
            sel.update(p, side);
            self.dirty = true;
        }
    }

    /// Selects the whole history and screen.
    pub fn select_all(&mut self) {
        let top = Point::new(Line(-(self.history_len() as i32)), Column(0));
        let bottom = Point::new(Line(self.size.1 as i32 - 1), Column(self.size.0 - 1));
        let mut sel = Selection::new(SelectionType::Simple, top, Side::Left);
        sel.update(bottom, Side::Right);
        self.term.selection = Some(sel);
        self.dirty = true;
    }

    pub fn clear_selection(&mut self) {
        if self.term.selection.take().is_some() {
            self.dirty = true;
        }
    }

    /// The selected text, lines joined with `\n`, wrapped lines rejoined,
    /// without a final line break (a pasted line does not run by itself).
    pub fn selection_text(&self) -> Option<String> {
        let mut s = self.term.selection_to_string()?;
        if s.ends_with('\n') {
            s.pop();
        }
        (!s.is_empty()).then_some(s)
    }

    // ---- mouse ----

    /// The program's mouse reports are on, and Shift is not held (Shift
    /// selects even then, as in xterm).
    pub fn mouse_reporting(&self, shift: bool) -> bool {
        mouse::reporting(self.mode()) && !shift
    }

    fn cell_at(&self, x: f32, y: f32) -> (usize, usize) {
        let col = (x / cell_width()).floor().clamp(0.0, self.size.0 as f32 - 1.0) as usize;
        let row = (y / LINE_HEIGHT).floor().clamp(0.0, self.size.1 as f32 - 1.0) as usize;
        (col, row)
    }

    /// A mouse press, motion or release at (`x`, `y`) for the program.
    pub fn mouse_report(&mut self, button: mouse::Button, action: mouse::Action, x: f32, y: f32, mods: rune::Mods) {
        let (col, row) = self.cell_at(x, y);
        if action == mouse::Action::Motion && self.last_report_cell == Some((col, row)) {
            return;
        }
        self.last_report_cell = Some((col, row));
        let m = mouse::Mods { shift: mods.shift, alt: mods.alt, ctrl: mods.ctrl };
        if let Some(b) = mouse::report(self.mode(), button, action, col, row, m) {
            self.send_report(b);
        }
    }

    /// A drag phase on the grid (Rune's `on_drag`): selection, or reports for
    /// the program. Returns a link to open after a primary-modifier click on
    /// one (⌘ on macOS, Ctrl elsewhere).
    pub fn on_drag(&mut self, d: &rune::Drag, button: rune::PointerButton, mods: rune::Mods) -> Option<String> {
        let d = DragInput { phase: d.phase, x: d.x, y: d.y, start_x: d.start_x, start_y: d.start_y, dx: d.dx, dy: d.dy, clicks: d.clicks };
        self.drag(d, button, mods)
    }

    /// [`Self::on_drag`] from plain values (Rune's `Drag` cannot be built
    /// outside Rune).
    pub fn drag(&mut self, d: DragInput, button: rune::PointerButton, mods: rune::Mods) -> Option<String> {
        use rune::Phase;
        let b = match button {
            rune::PointerButton::Middle => mouse::Button::Middle,
            rune::PointerButton::Secondary => mouse::Button::Right,
            _ => mouse::Button::Left,
        };
        match d.phase {
            Phase::Down => {
                self.blink_epoch = self.clock;
                if self.mouse_reporting(mods.shift) {
                    self.gesture = Some(Gesture::Report(b));
                    self.last_report_cell = None;
                    self.mouse_report(b, mouse::Action::Press, d.x, d.y, mods);
                } else if b == mouse::Button::Left {
                    self.gesture = Some(Gesture::Select);
                    if d.clicks <= 1 {
                        self.clear_selection();
                    } else {
                        self.select_start(d.x, d.y, d.clicks);
                    }
                }
            }
            Phase::Move => match self.gesture {
                Some(Gesture::Report(b)) => self.mouse_report(b, mouse::Action::Motion, d.x, d.y, mods),
                Some(Gesture::Select) => {
                    if self.term.selection.is_none() {
                        // A plain drag starts where the press was.
                        self.select_start(d.start_x, d.start_y, d.clicks);
                    }
                    self.select_update(d.x, d.y);
                }
                None => {}
            },
            Phase::Up => match self.gesture.take() {
                Some(Gesture::Report(b)) => self.mouse_report(b, mouse::Action::Release, d.x, d.y, mods),
                Some(Gesture::Select) => {
                    let moved = d.dx.abs() > 2.0 || d.dy.abs() > 2.0;
                    if !moved && d.clicks <= 1 && mods.cmd {
                        return self.link_at(d.x, d.y).map(|(_, _, _, uri)| uri);
                    }
                }
                None => {}
            },
        }
        None
    }

    /// A secondary (right) click: Rune reports only its release, so the
    /// program hears a press and a release together.
    pub fn on_secondary_click(&mut self, x: f32, y: f32, mods: rune::Mods) -> bool {
        if !self.mouse_reporting(mods.shift) {
            return false;
        }
        self.last_report_cell = None;
        self.mouse_report(mouse::Button::Right, mouse::Action::Press, x, y, mods);
        self.mouse_report(mouse::Button::Right, mouse::Action::Release, x, y, mods);
        true
    }

    /// The pointer moved over the grid with no button down: motion reports
    /// (DECSET 1003) and the hovered link.
    pub fn on_pointer_move(&mut self, x: f32, y: f32, mods: rune::Mods) {
        log::trace!("pointer at {x:.1},{y:.1}");
        if self.gesture.is_none() && self.mode().contains(TermMode::MOUSE_MOTION) {
            self.mouse_report(mouse::Button::None, mouse::Action::Motion, x, y, mods);
        }
        let link = self.link_at(x, y);
        if link.as_ref().map(|l| (l.0, l.1, l.2)) != self.hover_link.as_ref().map(|l| (l.0, l.1, l.2)) {
            self.hover_link = link;
            self.dirty = true;
        }
    }

    /// Whether the pointer is over an OSC 8 link.
    pub fn hovering_link(&self) -> bool {
        self.hover_link.is_some()
    }

    /// The OSC 8 link under (`x`, `y`): its viewport row, its columns on that
    /// row, and its URI (web links only, as the web UI opens).
    fn link_at(&self, x: f32, y: f32) -> Option<(usize, usize, usize, String)> {
        let (col, row) = self.cell_at(x, y);
        let line = &self.term.grid()[Line(row as i32 - self.display_offset() as i32)];
        let link = line[Column(col)].hyperlink()?;
        let uri = link.uri().to_string();
        let lower = uri.to_ascii_lowercase();
        if !(lower.starts_with("http://") || lower.starts_with("https://")) {
            return None;
        }
        let same = |c: usize| line[Column(c)].hyperlink().is_some_and(|h| h == link);
        let mut a = col;
        while a > 0 && same(a - 1) {
            a -= 1;
        }
        let mut b = col + 1;
        while b < self.size.0 && same(b) {
            b += 1;
        }
        Some((row, a, b, uri))
    }

    /// A wheel or trackpad delta of `dy` points (positive: content moves
    /// down, toward older lines) at (`x`, `y`): wheel reports for a program
    /// that asked, arrow keys in a full-screen program that did not (xterm's
    /// alternate scroll), the history otherwise.
    pub fn on_wheel(&mut self, dy: f32, x: f32, y: f32, mods: rune::Mods) {
        self.wheel_acc += dy / LINE_HEIGHT;
        let lines = self.wheel_acc.trunc() as i32;
        if lines == 0 {
            return;
        }
        self.wheel_acc -= lines as f32;
        let mode = self.mode();
        if self.mouse_reporting(mods.shift) {
            let b = if lines > 0 { mouse::Button::WheelUp } else { mouse::Button::WheelDown };
            for _ in 0..lines.unsigned_abs().min(10) {
                self.last_report_cell = None;
                self.mouse_report(b, mouse::Action::Press, x, y, mods);
            }
        } else if mode.contains(TermMode::ALT_SCREEN) && mode.contains(TermMode::ALTERNATE_SCROLL) {
            let app = mode.contains(TermMode::APP_CURSOR);
            let key: &[u8] = match (lines > 0, app) {
                (true, true) => b"\x1bOA",
                (true, false) => b"\x1b[A",
                (false, true) => b"\x1bOB",
                (false, false) => b"\x1b[B",
            };
            let bytes = key.repeat(lines.unsigned_abs().min(10) as usize);
            self.send_report(bytes);
        } else {
            self.scroll_lines(lines);
        }
    }

    // ---- drawing ----

    fn cursor_draw(&self) -> Option<CursorDraw> {
        let style = self.term.cursor_style();
        if style.shape == CursorShape::Hidden {
            return None;
        }
        if !self.focused {
            return Some(CursorDraw::Hollow);
        }
        if !self.blink_on {
            return None;
        }
        Some(match style.shape {
            CursorShape::Underline => CursorDraw::Underline,
            CursorShape::Beam => CursorDraw::Beam,
            CursorShape::HollowBlock => CursorDraw::Hollow,
            _ => CursorDraw::Block,
        })
    }

    /// The shape the cursor is drawn with now (`None`: hidden, blinked off,
    /// scrolled out of view, or an input-method composition covers it).
    pub fn cursor_shape(&self) -> Option<CursorDraw> {
        self.cursor.and(self.preedit.is_none().then(|| self.cursor_draw()).flatten())
    }

    /// Turns the grid into Rune rows, rebuilding only rows that alacritty
    /// reports as damaged or whose cursor, selection or link state changed.
    fn rebuild(&mut self) {
        let start = Instant::now();
        self.dirty = false;
        let (ncols, nrows) = self.size;
        let offset = self.display_offset() as i32;
        let cur = self.term.grid().cursor.point;
        let cur_row = cur.line.0 + offset;
        let cursor_visible = self.mode().contains(TermMode::SHOW_CURSOR) && (0..nrows as i32).contains(&cur_row);
        self.cursor = cursor_visible.then_some((cur.column.0 as u16, cur_row as u16));
        let draw = self.cursor_shape();

        let mut damaged = vec![false; nrows];
        let full = std::mem::take(&mut self.force_all) || self.rows.len() != nrows;
        match self.term.damage() {
            TermDamage::Full => damaged.iter_mut().for_each(|d| *d = true),
            TermDamage::Partial(it) => {
                for d in it {
                    if let Some(x) = damaged.get_mut(d.line) {
                        *x = true;
                    }
                }
            }
        }
        self.term.reset_damage();
        let style = style();
        self.rows.resize_with(nrows, || Rc::new(GridRow::new("", vec![], style)));
        self.row_extra.resize(nrows, u64::MAX);
        let sel = self.term.selection.as_ref().and_then(|s| s.to_range(&self.term));
        let colors = *self.term.colors();
        let grid = self.term.grid();
        for r in 0..nrows.min(grid.screen_lines()) {
            let line = Line(r as i32 - offset);
            let cursor = draw.filter(|_| cur_row == r as i32).map(|d| (cur.column.0, d));
            let selection = sel.as_ref().and_then(|s| {
                if line < s.start.line || line > s.end.line {
                    return None;
                }
                if s.is_block {
                    return Some((s.start.column.0, s.end.column.0 + 1));
                }
                let a = if line == s.start.line { s.start.column.0 } else { 0 };
                let b = if line == s.end.line { s.end.column.0 + 1 } else { ncols };
                Some((a, b))
            });
            let link = self.hover_link.as_ref().filter(|l| l.0 == r).map(|l| (l.1, l.2));
            let mut h = std::collections::hash_map::DefaultHasher::new();
            (cursor, selection, link).hash(&mut h);
            let extra = h.finish();
            if !full && !damaged[r] && self.row_extra[r] == extra {
                continue;
            }
            self.row_extra[r] = extra;
            self.rebuilt_rows += 1;
            let ctx = render::RowCtx {
                cols: ncols,
                colors: &colors,
                theme: &self.theme,
                style,
                cell_w: cell_width(),
                line_h: LINE_HEIGHT,
                font_size: FONT_SIZE,
                cursor,
                selection,
                link,
            };
            self.rows[r] = Rc::new(render::build_row(&grid[line], &ctx));
        }
        self.rebuilds += 1;
        self.build_time += start.elapsed();
    }

    /// The text of the live screen (not the scrolled view), row by row.
    pub fn screen_text(&self) -> String {
        self.text_of(0)
    }

    /// The text of what the view shows now (scrolled into the history or not).
    pub fn visible_text(&self) -> String {
        self.text_of(self.display_offset() as i32)
    }

    fn text_of(&self, offset: i32) -> String {
        let g = self.term.grid();
        (0..g.screen_lines() as i32)
            .map(|l| {
                let row = &g[Line(l - offset)];
                let mut s = String::new();
                for c in 0..g.columns() {
                    let cell = &row[Column(c)];
                    if cell.flags.contains(alacritty_terminal::term::cell::Flags::WIDE_CHAR_SPACER) {
                        continue;
                    }
                    s.push(cell.c);
                    if let Some(z) = cell.zerowidth() {
                        s.extend(z);
                    }
                }
                s.trim_end().to_string()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Feeds bytes as if the daemon sent them (tests, fixtures).
    pub fn feed(&mut self, bytes: &[u8]) {
        self.on_event(TermEvent::Output(bytes.to_vec()));
        self.drain_term_events();
        self.rebuild();
    }
}

#[cfg(test)]
mod tests;
