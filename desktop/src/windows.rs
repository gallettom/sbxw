//! The window hub: the one place that owns the app's windows besides the
//! main one, and routes what happens in them to the right window (design
//! A3 of the windows study, `.sbxw-artifacts/reports/windows-canvas-study.md`).
//!
//! * **The primary window** keeps the whole [`App`] as its state: the
//!   model, the features, approvals, Jarvis, the palette. Nothing of that
//!   is copied or shared.
//! * **A secondary window** holds one detached thing. Today there is one:
//!   the **canvas window** (key [`CANVAS`]), which shows the Canvas view
//!   (its pickers, tools, surface, remarks, Ask and Send) instead of the
//!   main window's overlay. One place at a time: while it is open, the
//!   main window's Canvas overlay shows [`placeholder`] instead.
//! * **The screen moves, it is not copied.** The Canvas's Luau screen (its
//!   [`ScreenState`]) lives in the primary's `App::screens`. While the
//!   canvas window is open it is moved out at the end of each primary turn
//!   ([`primary_leave`]) into a slot here, taken by the canvas window for
//!   its own turn ([`canvas_enter`]) and handed back ([`canvas_leave`]),
//!   then taken back at the start of the next primary turn
//!   ([`primary_enter`]). Rune's `App::turn_scope` runs these hooks; the
//!   windows of one event loop take turns, never two at once.
//! * **Everything the canvas window asks goes to the primary** as a
//!   [`ToPrimary`] message: the screen's intents (with the screen's
//!   `editing` flag, so a typed change is never taken for a press), the
//!   surface's events, its keys, dropped files and a pasted picture. The
//!   primary runs them by its own paths ([`App::windows_drain`]), exactly
//!   as for its overlay.
//! * **Gates across windows** (the person's decision, 2026-11-23): a gate
//!   is drawn in exactly one window, and only that window's input answers
//!   it ([`gates`]).
//!   - The core's confirmation (`feats.dialog`: Send remarks, Ask the
//!     agent, Delete, any feature's Confirm) raised by input from the
//!     canvas window belongs to that window ([`Hub::dialog_raised`]): the
//!     `Dialog` moves into the canvas window for its turns, as the screen
//!     does, and is drawn there by the same `approval::confirm_view` in the
//!     same scrim (same text, lines, default, keys). Its Confirm and Cancel
//!     go back to the primary as [`ToPrimary::Answer`] naming that very
//!     confirmation, and the primary runs its own `confirm_click` or
//!     `cancel_click` only for the owner window's answer. The main window
//!     meanwhile shows [`elsewhere_scrim`] and its Escape leaves the
//!     confirmation alone. Closing the canvas window cancels it.
//!   - Every other gate (a confirmation raised in the main window, the
//!     copy offer, Jarvis's proposal, a modal, the palette) stays in the
//!     main window as before; the canvas window then shows a modal scrim
//!     saying where to answer, with a button that brings the main window
//!     to the front, and the hub refuses its input ([`Hub::post`]). The
//!     primary checks the gates again before running each message.
//! * **Wakes.** Each window has its own Rune waker. A message for the
//!   primary wakes it; a primary turn that changed what the canvas window
//!   draws (the screen rendered, the surface's scene or pictures, the gate)
//!   wakes the canvas window.
//!
//! The model ([`Hub`]) is plain data and tested as such; what touches Rune
//! ([`Platform`]) is installed by `main.rs`, so tests see the operations the
//! hub asked for instead of windows.

use crate::approval::ConfirmAnswers;
use crate::intent::Intent;
use crate::shell::Dialog;
use crate::screen::{self, Host, ScreenState};
use crate::shell::App;
use crate::surface::SurfaceEvent;
use crate::theme::{self, c, layout, radius};
use rune::{col, el, row, text, Cursor, Cx, El, KeyEvent, Tx, Waker};
use serde_json::{json, Value as J};
use std::cell::RefCell;
use std::path::PathBuf;

/// The canvas window's key (one canvas window: the Canvas view has one
/// current canvas and one surface, `feat::canvas::SURF`).
pub const CANVAS: &str = "canvas";
/// The screen the canvas window shows.
pub const CANVAS_SCREEN: &str = crate::feat::canvas::SCREEN;
/// The canvas window's size when it opens, and its smallest, in points.
pub const CANVAS_SIZE: (f32, f32) = (1200.0, 800.0);
pub const CANVAS_MIN: (f32, f32) = (640.0, 420.0);

/// What a secondary window hands the primary.
#[derive(Debug)]
pub enum ToPrimary {
    /// A screen's action returned these; `editing`: it ran from an input's
    /// per-keystroke change, not from a press (`App::pressed`).
    Intents { screen: String, intents: Vec<Intent>, editing: bool },
    /// An event on the canvas surface.
    Surface(SurfaceEvent),
    /// A key for the screen's `<name>_key` intent (`{key, text?}`).
    Key(J),
    /// Files dropped on the window.
    Files(Vec<PathBuf>),
    /// A picture pasted in the window (PNG bytes).
    Png(Vec<u8>),
    /// The window's content size changed (its screen lays out by it).
    Resized,
    /// The window closed (its close button, ⌘W): the view goes back to the main window.
    Closed,
    /// The answer to the confirmation this window shows (its Confirm, or
    /// Cancel, ×, Escape, a press outside): `dialog` is its identity
    /// (`Dialog::identity`), so it answers that one or nothing.
    Answer { confirm: bool, dialog: String },
}

impl ToPrimary {
    /// Whether it is the person's input to the view, which a gate holds
    /// (an answer is the gate's own input: see [`Hub::post`]).
    pub fn is_input(&self) -> bool {
        !matches!(self, ToPrimary::Resized | ToPrimary::Closed | ToPrimary::Answer { .. })
    }
}

/// What the hub asks of the platform (Rune), in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Native {
    /// Open the window of that key.
    Open(&'static str),
    /// Bring that window to the front.
    Focus(&'static str),
    /// Close that window.
    Close(&'static str),
    /// Bring the main window to the front.
    FocusPrimary,
}

/// Where a gate is drawn and answered, and what the other window says about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Gate {
    /// The window that draws it, and alone answers it: [`PRIMARY`], or
    /// [`CANVAS`] for a confirmation raised there.
    pub owner: &'static str,
    /// One line naming what waits, said in the other window.
    pub what: String,
}

/// The owner of every gate.
pub const PRIMARY: &str = "main";

/// The hub's model: which windows are open, what goes to the primary, the
/// gate, and what the platform must do.
#[derive(Debug, Default)]
pub struct Hub {
    canvas_open: bool,
    /// The canvas window's content size (points), once laid out.
    canvas_size: Option<[f32; 2]>,
    to_primary: Vec<(&'static str, ToPrimary)>,
    native: Vec<Native>,
    /// The main window's own gate, which holds the canvas window.
    gate: Option<Gate>,
    /// The confirmation up (its identity) and the window it belongs to.
    dialog: Option<(String, &'static str)>,
    /// The canvas window must draw again.
    canvas_dirty: bool,
    /// What the canvas window last drew from: the screen's renders, the
    /// surface's generation.
    seen: (u64, u64),
    /// Input refused because of a gate (for the log and tests).
    pub refused: u64,
}

/// What [`Hub::open_canvas`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Opened {
    New,
    Focused,
}

impl Hub {
    pub fn canvas_open(&self) -> bool {
        self.canvas_open
    }

    /// Opens the canvas window, or brings it to the front when it is open
    /// (one canvas window, never two).
    pub fn open_canvas(&mut self) -> Opened {
        if self.canvas_open {
            self.native.push(Native::Focus(CANVAS));
            return Opened::Focused;
        }
        self.canvas_open = true;
        self.canvas_dirty = true;
        self.native.push(Native::Open(CANVAS));
        Opened::New
    }

    /// Brings the canvas window to the front, if it is open.
    pub fn focus_canvas(&mut self) -> bool {
        if self.canvas_open {
            self.native.push(Native::Focus(CANVAS));
        }
        self.canvas_open
    }

    /// The view goes back to the main window: the canvas window closes.
    /// Its `on_close` then reports [`Self::window_closed`], which finds it
    /// closed already and says nothing more. A confirmation it showed is
    /// cancelled (never confirmed).
    pub fn close_canvas(&mut self) -> bool {
        let was = std::mem::take(&mut self.canvas_open);
        if was {
            self.cancel_dialog_of(CANVAS);
            self.native.push(Native::Close(CANVAS));
            self.canvas_size = None;
        }
        was
    }

    /// A window closed by the person (its close button, ⌘W, the menu): a
    /// confirmation it showed is cancelled, and the primary hears it once
    /// and puts the view back in its overlay.
    pub fn window_closed(&mut self, key: &'static str) {
        if key == CANVAS && std::mem::take(&mut self.canvas_open) {
            self.canvas_size = None;
            self.cancel_dialog_of(CANVAS);
            self.to_primary.push((key, ToPrimary::Closed));
        }
    }

    /// The confirmation of window `key` is cancelled: its answer is queued
    /// (the primary runs `cancel_click` for it), and it belongs to the main
    /// window until then, so the main window draws it if it is still up.
    fn cancel_dialog_of(&mut self, key: &'static str) {
        if let Some((id, owner)) = &mut self.dialog {
            if *owner == key {
                self.to_primary.push((key, ToPrimary::Answer { confirm: false, dialog: id.clone() }));
            }
        }
    }

    /// A message from window `from` to the primary. Returns whether it was
    /// queued. An answer is taken only from the window the confirmation
    /// belongs to; the view's input only while no gate holds that window
    /// (the main window's own gate, or its own confirmation, which is
    /// modal there).
    pub fn post(&mut self, from: &'static str, msg: ToPrimary) -> bool {
        let ok = match &msg {
            ToPrimary::Answer { dialog, .. } => self.dialog.as_ref().is_some_and(|(id, owner)| *owner == from && id == dialog),
            m if m.is_input() => self.gate.is_none() && self.dialog_owner() != Some(from) && (from != CANVAS || self.canvas_open),
            _ => true,
        };
        if !ok {
            self.refused += 1;
            log::info!("windows: input from the {from} window refused: a gate holds it");
            return false;
        }
        self.to_primary.push((from, msg));
        true
    }

    /// A confirmation came up while the primary ran input from window
    /// `from`: it is drawn and answered there.
    pub fn dialog_raised(&mut self, id: String, from: &'static str) {
        let owner = if self.is_open(from) { from } else { PRIMARY };
        self.dialog = Some((id, owner));
        self.canvas_dirty = true;
    }

    /// The confirmation up at the end of a primary turn (its identity, or
    /// none): one this hub has not seen came from the main window itself
    /// (its own press, its own paths), and belongs to it.
    pub fn note_dialog(&mut self, id: Option<String>) {
        let next = match id {
            None => None,
            Some(id) => match self.dialog.take() {
                Some((seen, owner)) if seen == id && (owner == PRIMARY || self.is_open(owner)) => Some((seen, owner)),
                _ => Some((id, PRIMARY)),
            },
        };
        if next != self.dialog {
            self.canvas_dirty = true;
        }
        self.dialog = next;
    }

    /// The window the confirmation up belongs to.
    pub fn dialog_owner(&self) -> Option<&'static str> {
        self.dialog.as_ref().map(|(_, o)| *o)
    }

    /// The primary takes what the other windows asked, in order.
    pub fn take_for_primary(&mut self) -> Vec<(&'static str, ToPrimary)> {
        std::mem::take(&mut self.to_primary)
    }

    pub fn pending_for_primary(&self) -> bool {
        !self.to_primary.is_empty()
    }

    /// The main window's own gate, which holds the other windows (`None`: none).
    pub fn set_gate(&mut self, gate: Option<Gate>) {
        if gate != self.gate {
            self.gate = gate;
            self.canvas_dirty = true;
        }
    }

    pub fn gate(&self) -> Option<&Gate> {
        self.gate.as_ref()
    }

    /// The person asked for the main window (the scrim's button).
    pub fn show_primary(&mut self) {
        self.native.push(Native::FocusPrimary);
    }

    pub fn is_open(&self, key: &str) -> bool {
        key == CANVAS && self.canvas_open
    }

    /// What the platform must do now.
    pub fn take_native(&mut self) -> Vec<Native> {
        std::mem::take(&mut self.native)
    }

    pub fn canvas_size(&self) -> Option<[f32; 2]> {
        self.canvas_size
    }

    /// The canvas window laid out at that size: true when it changed.
    pub fn set_canvas_size(&mut self, size: [f32; 2]) -> bool {
        let changed = self.canvas_size.is_none_or(|s| (s[0] - size[0]).abs() > 1.0 || (s[1] - size[1]).abs() > 1.0);
        if changed {
            self.canvas_size = Some(size);
        }
        changed
    }

    /// What the canvas window draws from changed (renders, surface generation).
    pub fn note_drawn_from(&mut self, renders: u64, surface: u64) {
        if self.seen != (renders, surface) {
            self.seen = (renders, surface);
            self.canvas_dirty = true;
        }
    }

    /// Whether the canvas window must draw again (and clears it).
    pub fn take_canvas_dirty(&mut self) -> bool {
        std::mem::take(&mut self.canvas_dirty)
    }

    fn canvas_dirty(&self) -> bool {
        self.canvas_dirty
    }
}

// ---------------------------------------------------------------------------
// The UI thread's hub, and the platform

/// What touches the windowing system. `main.rs` installs Rune's; tests
/// install none and read [`take_done`].
pub trait Platform {
    fn run(&mut self, op: &Native);
}

thread_local! {
    static HUB: RefCell<Hub> = RefCell::new(Hub::default());
    /// The Canvas screen between turns while the canvas window is open:
    /// (its index in `App::screens`, the screen).
    static SLOT: RefCell<Option<(usize, ScreenState)>> = const { RefCell::new(None) };
    /// The confirmation between turns while it belongs to the canvas window.
    static DIALOG: RefCell<Option<Dialog>> = const { RefCell::new(None) };
    static PLATFORM: RefCell<Option<Box<dyn Platform>>> = RefCell::new(None);
    /// The operations run with no platform installed (tests).
    static DONE: RefCell<Vec<Native>> = const { RefCell::new(Vec::new()) };
    static PRIMARY_WAKE: RefCell<Option<Waker>> = const { RefCell::new(None) };
    static CANVAS_WAKE: RefCell<Option<Waker>> = const { RefCell::new(None) };
}

/// The UI thread's hub.
pub fn with<R>(f: impl FnOnce(&mut Hub) -> R) -> R {
    HUB.with(|h| f(&mut h.borrow_mut()))
}

/// The windowing system (once, from `main`).
pub fn install(p: Box<dyn Platform>) {
    PLATFORM.with(|s| *s.borrow_mut() = Some(p));
}

/// The primary's waker (its `setup`).
pub fn set_primary_waker(w: Waker) {
    PRIMARY_WAKE.with(|s| *s.borrow_mut() = Some(w));
}

/// The operations the hub asked for with no platform installed (tests).
pub fn take_done() -> Vec<Native> {
    DONE.with(|d| std::mem::take(&mut *d.borrow_mut()))
}

/// Runs what the hub asked of the platform.
pub fn run_native() {
    let ops = with(Hub::take_native);
    if ops.is_empty() {
        return;
    }
    PLATFORM.with(|p| match &mut *p.borrow_mut() {
        Some(p) => ops.iter().for_each(|op| p.run(op)),
        None => DONE.with(|d| d.borrow_mut().extend(ops)),
    });
}

fn wake(slot: &'static std::thread::LocalKey<RefCell<Option<Waker>>>) {
    let w = slot.with(|s| s.borrow().clone());
    if let Some(w) = w {
        w();
    }
}

/// Whether the canvas window is open (the Canvas view is there).
pub fn canvas_open() -> bool {
    with(|h| h.canvas_open())
}

/// The canvas window's width, once laid out.
pub fn canvas_width() -> Option<f32> {
    with(|h| h.canvas_size().map(|s| s[0]))
}

/// Whether the main window must not draw screen `name` because a window
/// shows it (one place per view): it draws [`placeholder`] instead.
pub fn shows_screen(name: &str) -> bool {
    name == CANVAS_SCREEN && canvas_open()
}

/// The canvas feature opens its window (`canvas_window {to = "window"}`).
pub fn open_canvas() -> Opened {
    with(Hub::open_canvas)
}

/// The canvas feature takes its view back (`canvas_window {to = "main"}`).
pub fn close_canvas() -> bool {
    with(Hub::close_canvas)
}

/// The placeholder's button: the canvas window to the front.
pub fn focus_canvas() -> bool {
    let open = with(Hub::focus_canvas);
    run_native();
    open
}

/// Every gate up, each with the window that draws and answers it: the
/// core's confirmation (in the window it belongs to, [`Hub::dialog_owner`]),
/// and in the main window an approval, the copy offer, Jarvis's proposal, a
/// modal, the palette.
pub fn gates(app: &App) -> Vec<Gate> {
    let mut v = vec![];
    let main = |what: &str| Gate { owner: PRIMARY, what: what.into() };
    if !app.approvals.is_empty() {
        v.push(main("An approval waits for you in the main window."));
    }
    if app.feats.dialog.is_some() {
        match with(|h| h.dialog_owner().filter(|o| *o != PRIMARY && h.is_open(o))) {
            Some(o) => v.push(Gate { owner: o, what: "A confirmation waits for you in the Canvas window.".into() }),
            None => v.push(main("A confirmation waits for you in the main window.")),
        }
    }
    if app.copy_offer.is_some() {
        v.push(main("A copy to the clipboard waits for your answer in the main window."));
    }
    if app.proposal_up() {
        v.push(main("A Jarvis proposal waits for your answer in the main window."));
    }
    if app.feats.modal.is_some() || app.palette.is_some() {
        v.push(main("Something is open in the main window (a dialog or the palette)."));
    }
    v
}

/// The first gate up, wherever it is drawn: no window's view input runs then.
pub fn gate_of(app: &App) -> Option<Gate> {
    gates(app).into_iter().next()
}

/// Whether the confirmation up belongs to another window than the main one
/// (the main window then draws [`elsewhere_scrim`], never the confirmation).
pub fn dialog_elsewhere() -> bool {
    with(|h| h.dialog_owner().is_some_and(|o| o != PRIMARY && h.is_open(o)))
}

/// The start of a primary turn: the Canvas screen comes back to
/// `App::screens`, and a confirmation the canvas window held to `feats.dialog`.
pub fn primary_enter(app: &mut App) {
    if let Some((i, sc)) = SLOT.with(|s| s.borrow_mut().take()) {
        let i = i.min(app.screens.len());
        app.screens.insert(i, sc);
    }
    if let Some(d) = DIALOG.with(|s| s.borrow_mut().take()) {
        if app.feats.dialog.is_none() {
            app.feats.dialog = Some(d);
        } else {
            log::warn!("windows: two confirmations at once; the canvas window's is dropped");
        }
    }
}

/// The end of a primary turn: the gates are published, the Canvas screen
/// (and a confirmation that belongs to the canvas window) move out while
/// its window is open, the platform runs what was asked, and the canvas
/// window is woken when what it draws changed.
pub fn primary_leave(app: &mut App) {
    with(|h| h.note_dialog(app.feats.dialog.as_ref().map(Dialog::identity)));
    let gate = gates(app).into_iter().find(|g| g.owner == PRIMARY);
    with(|h| h.set_gate(gate));
    if dialog_elsewhere() {
        let d = app.feats.dialog.take();
        DIALOG.with(|s| *s.borrow_mut() = d);
    }
    if canvas_open() {
        if let Some(i) = app.screens.iter().position(|s| s.name == CANVAS_SCREEN) {
            let renders = app.screens[i].renders;
            with(|h| h.note_drawn_from(renders, crate::surface_canvas::generation()));
            let sc = app.screens.remove(i);
            SLOT.with(|s| *s.borrow_mut() = Some((i, sc)));
        }
    }
    run_native();
    if canvas_open() && with(|h| h.canvas_dirty()) {
        wake(&CANVAS_WAKE);
    }
}

// ---------------------------------------------------------------------------
// The canvas window

/// The canvas window's state: the Canvas screen during its turns, nothing
/// else of the app.
#[derive(Default)]
pub struct CanvasWin {
    screen: Option<(usize, ScreenState)>,
    /// The confirmation that belongs to this window, during its turns.
    dialog: Option<Dialog>,
    /// The gate shown (copied at the turn's start).
    gate: Option<Gate>,
    /// The size last laid out.
    size: [f32; 2],
}

impl CanvasWin {
    pub fn has_screen(&self) -> bool {
        self.screen.is_some()
    }

    pub fn shown_gate(&self) -> Option<&Gate> {
        self.gate.as_ref()
    }

    pub fn dialog(&self) -> Option<&Dialog> {
        self.dialog.as_ref()
    }

    /// Its confirmation's answer, to the primary, which runs its own
    /// `confirm_click` or `cancel_click` for that confirmation alone.
    fn answer(&mut self, confirm: bool) {
        if let Some(id) = self.dialog.as_ref().map(Dialog::identity) {
            self.post(ToPrimary::Answer { confirm, dialog: id });
        }
    }

    fn post(&mut self, msg: ToPrimary) {
        with(|h| h.post(CANVAS, msg));
    }

    /// A key the focused control did not take. While a gate is up every
    /// key is taken here and goes nowhere: the scrim is modal, and no key
    /// pressed in this window may answer the main window's gate.
    pub fn on_key(&mut self, ev: &KeyEvent, tx: &mut Tx) -> bool {
        if full_screen_key(ev) {
            tx.toggle_full_screen();
            return true;
        }
        // Its own confirmation is modal (Rune's scrim takes Escape, Tab and
        // its buttons' keys); what it passes on goes nowhere.
        if self.gate.is_some() || self.dialog.is_some() {
            with(|h| h.refused += 1);
            return true;
        }
        let Some(k) = screen_key(ev) else { return false };
        // Idle: nothing of the view's own has the keyboard, or one of its rows does.
        let idle = tx.focused().is_none_or(|f| f.starts_with("canvas-row-") || f.starts_with("cwin"));
        if !idle {
            return false;
        }
        let mut args = json!({ "key": k });
        if let rune::Key::Text(t) = &ev.key {
            args["text"] = t.clone().into();
        }
        self.post(ToPrimary::Key(args));
        true
    }
}

/// ⌃⌘F on macOS (its standard), F11 elsewhere.
pub fn full_screen_key(ev: &KeyEvent) -> bool {
    let m = ev.mods;
    let f = matches!(&ev.key, rune::Key::Text(t) if t.eq_ignore_ascii_case("f")) || matches!(&ev.key, rune::Key::Command(c) if c.eq_ignore_ascii_case(&'f'));
    if cfg!(target_os = "macos") {
        f && m.ctrl && m.logo && !m.alt && !m.shift
    } else {
        ev.key == rune::Key::Function(11) && !m.ctrl && !m.alt && !m.shift
    }
}

/// The Canvas's keys (`canvas_key`), as the main window's overlay takes them.
fn screen_key(ev: &KeyEvent) -> Option<&'static str> {
    let k = match &ev.key {
        rune::Key::Up => "up",
        rune::Key::Down => "down",
        rune::Key::Enter => "enter",
        rune::Key::Left => "left",
        rune::Key::Right => "right",
        rune::Key::Backspace => "backspace",
        rune::Key::Escape => "escape",
        rune::Key::Text(t) if !t.is_empty() && t.chars().all(|c| !c.is_control()) => "type",
        _ => return None,
    };
    let plain = ev.mods == rune::Mods::default() || (k == "type" && !(ev.mods.cmd || ev.mods.ctrl || ev.mods.alt || ev.mods.logo));
    let takes = crate::intent::spec("canvas_key").is_some_and(|sp| sp.args.iter().any(|(n, a, _)| *n == "key" && matches!(a, crate::intent::Arg::OneOf(l) if l.contains(&k))));
    (plain && takes).then_some(k)
}

/// The core's confirmation, drawn in this window by `approval::confirm_view`:
/// the toggles and secret fields act on the confirmation itself (the same
/// `Dialog` methods the main window's use), Confirm and Cancel go to the
/// primary as this window's answer.
impl ConfirmAnswers for CanvasWin {
    fn confirm_click(&mut self) {
        self.answer(true);
    }
    fn cancel_click(&mut self) {
        self.answer(false);
    }
    fn toggle_confirm_ack(&mut self) {
        if let Some(d) = self.dialog.as_mut() {
            d.toggle_ack();
        }
    }
    fn toggle_confirm_dont_ask(&mut self) {
        if let Some(d) = self.dialog.as_mut() {
            d.toggle_dont_ask();
        }
    }
    fn secret_edit(&mut self, i: usize) -> &mut rune::TextEdit {
        &mut self.dialog.as_mut().expect("the canvas window's confirmation").secrets[i]
    }
}

impl Host for CanvasWin {
    fn screen(&mut self, name: &str) -> &mut ScreenState {
        match &mut self.screen {
            Some((_, sc)) if sc.name == name => sc,
            _ => panic!("the canvas window holds no screen {name}"),
        }
    }

    fn intents(&mut self, screen: &str, intents: Vec<Intent>) {
        let editing = self.screen.as_ref().is_some_and(|(_, sc)| sc.ui.editing);
        self.post(ToPrimary::Intents { screen: screen.to_string(), intents, editing });
    }

    fn surface_event(&mut self, ev: SurfaceEvent, _tx: &mut Tx) {
        self.post(ToPrimary::Surface(ev));
    }
}

/// The canvas window's turn starts: the Canvas screen comes from the slot,
/// and its confirmation when it has one.
pub fn canvas_enter(s: &mut CanvasWin) {
    if s.screen.is_none() {
        s.screen = SLOT.with(|sl| sl.borrow_mut().take());
    }
    if s.dialog.is_none() {
        s.dialog = DIALOG.with(|sl| sl.borrow_mut().take());
    }
    s.gate = with(|h| h.gate().cloned());
}

/// The canvas window's turn ends: the screen and its confirmation go back,
/// and the primary is woken when something waits for it.
pub fn canvas_leave(s: &mut CanvasWin) {
    if let Some(sc) = s.screen.take() {
        SLOT.with(|sl| *sl.borrow_mut() = Some(sc));
    }
    if let Some(d) = s.dialog.take() {
        DIALOG.with(|sl| *sl.borrow_mut() = Some(d));
    }
    run_native();
    if with(|h| h.pending_for_primary()) {
        wake(&PRIMARY_WAKE);
    }
}

pub fn canvas_setup(_s: &mut CanvasWin, waker: Waker) {
    CANVAS_WAKE.with(|w| *w.borrow_mut() = Some(waker));
}

/// Draws again when a primary turn changed what it shows.
pub fn canvas_poll(_s: &mut CanvasWin, _clock: f64, _tx: &mut Tx) -> bool {
    with(Hub::take_canvas_dirty)
}

/// Its close button, ⌘W, or the app quitting: the view goes back to the main window.
pub fn canvas_closed(_s: &mut CanvasWin, _tx: &mut Tx) {
    with(|h| h.window_closed(CANVAS));
}

/// The menu bar's item while the canvas window is the key window.
pub fn canvas_menu_full_screen(_s: &mut CanvasWin, tx: &mut Tx) {
    tx.toggle_full_screen();
}

fn small(size: f32, weight: f32) -> rune::TextStyle {
    screen::body(size, weight)
}

/// A plain button of the window's chrome.
fn button<S: 'static>(key: &str, label: &str, help: &str, primary: bool, press: impl Fn(&mut S, &mut Tx) + 'static) -> El {
    let (fill, fg) = if primary { (c("blue"), c("bg")) } else { (c("elevated"), c("text-bright")) };
    el().key(key.to_string())
        .role("button")
        .label(label.to_string())
        .help(help.to_string())
        .h(layout::CONTROL_H)
        .px(10.0)
        .items_center()
        .justify_center()
        .radius(radius("sm").unwrap())
        .fill(fill)
        .hover(if primary { c("blue").alpha(0.85) } else { c("hover") })
        .cursor(Cursor::Pointer)
        .focusable()
        .on_press_tx(press)
        .child(text(label.to_string()).font(small(theme::font_size::BODY, 500.0)).color(fg))
}

/// The canvas window's view: a header (the Canvas's title and pickers,
/// full screen, back to the main window), the Canvas screen, its open
/// menus, and the gate's scrim over everything while one is up.
pub fn canvas_view(s: &CanvasWin, _cx: &mut Cx<CanvasWin>) -> El {
    let mut root = col()
        .key("cwin")
        .w_full()
        .h_full()
        .fill(c("bg"))
        .on_layout(|s: &mut CanvasWin, r: [f32; 4]| {
            s.size = [r[2], r[3]];
            if with(|h| h.set_canvas_size([r[2], r[3]])) {
                s.post(ToPrimary::Resized);
            }
        })
        .on_drop_files(|s: &mut CanvasWin, files: &[PathBuf], _tx| s.post(ToPrimary::Files(files.to_vec())));
    // A picture pasted while no field has the keyboard (a field takes its own paste).
    if crate::shell::window_paste() {
        root = root.on_paste_content(|s: &mut CanvasWin, content: &rune::ClipboardContent, _tx: &mut Tx| {
            if let (Some(img), true) = (content.image(), content.text().is_empty()) {
                s.post(ToPrimary::Png(img.png().to_vec()));
            }
        });
    }
    let mut overlays = vec![];
    let body = match &s.screen {
        Some((_, sc)) => {
            let (panel, more, slot) = screen::view_slot::<CanvasWin>(sc);
            overlays = more;
            let mut head = row().key("cwin-head").w_full().h(layout::CONTROL_H).gap(8.0).items_center();
            if let Some(i) = crate::icons::get("canvas") {
                head = head.child(rune::icon(i, theme::icon::MEDIUM).color(c("text-bright")));
            }
            head = head.child(text("Canvas").font(small(theme::font_size::TITLE, 600.0)).color(c("text-bright")));
            if let Some(e) = slot {
                head = head.child(el().w(4.0)).child(e.shrink());
            }
            head = head
                .child(el().grow(1.0).min_w(0.0))
                .child(button("cwin-full-screen", "Full screen", full_screen_help(), false, |_s: &mut CanvasWin, tx: &mut Tx| tx.toggle_full_screen()))
                .child(button("cwin-back", "Back to the main window", "Closes this window: the Canvas shows over the panes of the main window again", false, |s: &mut CanvasWin, _tx: &mut Tx| {
                    if s.gate.is_none() {
                        s.post(ToPrimary::Closed);
                        with(|h| h.close_canvas());
                    }
                }));
            col().key("cwin-body").w_full().h_full().min_h(0.0).padding(12.0).gap(8.0).child(head).child(col().grow(1.0).w_full().min_h(0.0).child(panel))
        }
        // Between hand-overs (the main window holds the screen for a moment).
        None => col().key("cwin-wait").w_full().h_full().items_center().justify_center().child(text("The Canvas is in the main window.").font(small(theme::font_size::BODY, 400.0)).color(c("text-muted"))),
    };
    root = root.child(body);
    for o in overlays {
        root = root.child(o);
    }
    // Its own confirmation: the core's, in the core's scrim (a press
    // outside, Escape and × cancel), at the top of this window. Else the
    // main window's gate, said here.
    if let Some(d) = &s.dialog {
        let top = layout::GUTTER * 2.0 + 24.0;
        let size = (s.size[0].max(CANVAS_MIN.0), s.size[1].max(CANVAS_MIN.1));
        let dialog = crate::approval::confirm_view::<CanvasWin>(&d.state(size, top));
        root = root.child(crate::shell::scrim("cf-scrim", dialog, |s: &mut CanvasWin| ConfirmAnswers::cancel_click(s)));
    } else if let Some(g) = &s.gate {
        root = root.child(gate_scrim(g));
    }
    root
}

fn full_screen_help() -> &'static str {
    if cfg!(target_os = "macos") {
        "Full screen (⌃⌘F, or the green button)"
    } else {
        "Full screen (F11)"
    }
}

/// Over the canvas window while the main window has a gate up: modal (no
/// press, key or Tab reaches what is under it), saying where to answer.
fn gate_scrim(g: &Gate) -> El {
    let card = col()
        .key("cwin-gate")
        .role("alertdialog")
        .label("Answer in the main window")
        .w(420.0)
        .padding(16.0)
        .gap(10.0)
        .radius(radius("md").unwrap())
        .fill(c("panel"))
        .border(1.0, c("border"))
        .child(text("Answer in the main window").font(small(theme::font_size::TITLE, 600.0)).color(c("text-bright")))
        .child(text(g.what.clone()).font(small(theme::font_size::BODY, 400.0)).color(c("text")).wrap().w_full())
        .child(text("Nothing here can answer it: this window waits until it is answered there.").font(small(theme::font_size::CAPTION, 400.0)).color(c("text-muted")).wrap().w_full())
        .child(row().w_full().child(el().grow(1.0)).child(button("cwin-gate-show", "Show the main window", "Brings the main window to the front", true, |_s: &mut CanvasWin, _tx: &mut Tx| {
            with(Hub::show_primary);
        })));
    el().key("cwin-scrim")
        .pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
        .fill(theme::scrim())
        .modal(false, |_s: &mut CanvasWin| {})
        .on_press(|_s: &mut CanvasWin| {})
        .on_wheel(|_s: &mut CanvasWin, _ev: rune::WheelEvent, _tx: &mut Tx| true)
        .on_drop_files(|_s: &mut CanvasWin, _files: &[PathBuf], _tx| with(|h| h.refused += 1))
        .items_center()
        .justify_center()
        .child(card)
}

/// Over the main window while a confirmation raised in the canvas window
/// waits there: a press, the wheel or a drop on it reaches nothing and
/// answers nothing; its button brings the Canvas window to the front.
pub fn elsewhere_scrim() -> El {
    let card = col()
        .key("cwin-elsewhere")
        .role("alertdialog")
        .label("Answer in the Canvas window")
        .w(420.0)
        .padding(16.0)
        .gap(10.0)
        .radius(radius("md").unwrap())
        .fill(c("panel"))
        .border(1.0, c("border"))
        .on_press(|_s: &mut App| {})
        .child(text("Answer in the Canvas window").font(small(theme::font_size::TITLE, 600.0)).color(c("text-bright")))
        .child(text("A confirmation you asked for in the Canvas window waits there. It is answered there only.").font(small(theme::font_size::BODY, 400.0)).color(c("text")).wrap().w_full())
        .child(row().w_full().child(el().grow(1.0)).child(button("cwin-elsewhere-show", "Show the Canvas window", "Brings the Canvas window to the front", true, |_s: &mut App, _tx: &mut Tx| {
            focus_canvas();
        })));
    el().key("cwin-elsewhere-scrim")
        .pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
        .fill(theme::scrim())
        .on_press(|_s: &mut App| {})
        .on_secondary_press_local(|_s: &mut App, _p: rune::PointerEvent, _tx: &mut Tx| {})
        .on_wheel(|_s: &mut App, _ev: rune::WheelEvent, _tx: &mut Tx| true)
        .on_drop_files(|_s: &mut App, _files: &[PathBuf], _tx| {})
        .items_center()
        .justify_center()
        .child(card)
}

/// In the main window's Canvas overlay while the Canvas is in its own
/// window: where it is, a button that brings it to the front, and one that
/// brings it back here. Never a second live copy.
pub fn placeholder() -> El {
    col()
        .key("canvas-in-window")
        .w_full()
        .h_full()
        .items_center()
        .justify_center()
        .gap(10.0)
        .child(text("The Canvas is open in its own window").font(small(theme::font_size::TITLE, 600.0)).color(c("text-bright")))
        .child(text("It shows in one place at a time. Close that window, or bring it back here, to see it over the panes again.").font(small(theme::font_size::BODY, 400.0)).color(c("text-muted")).wrap().max_w(460.0))
        .child(
            row()
                .gap(8.0)
                .child(button("canvas-in-window-show", "Open in its window", "Brings the Canvas window to the front", true, |_s: &mut App, _tx: &mut Tx| {
                    focus_canvas();
                }))
                .child(button("canvas-in-window-back", "Bring it back here", "Closes the Canvas window and shows the Canvas here", false, |s: &mut App, _tx: &mut Tx| {
                    if let Ok(list) = crate::intent::parse(&json!({ "kind": "intent", "name": "canvas_window", "args": { "to": "main" } })) {
                        s.intents("canvas", list);
                    }
                })),
        )
}

/// The canvas window's spec: its own state, the Canvas screen moved in for
/// its turns, a standard frame, and its menu item for full screen.
pub fn canvas_spec() -> rune::WindowSpec<CanvasWin> {
    let spec = rune::AppSpec::<CanvasWin> {
        setup: Some(canvas_setup),
        poll: Some(canvas_poll),
        on_key: Some(|s: &mut CanvasWin, ev: &KeyEvent, tx: &mut Tx| s.on_key(ev, tx)),
        min_size: Some(CANVAS_MIN),
        families: (crate::terminal::MONO_FAMILIES, crate::terminal::MONO_FAMILIES),
        ..rune::AppSpec::new("Canvas", CANVAS_SIZE, CanvasWin::default, canvas_view)
    };
    let mut item = rune::MenuItem::new("Toggle Full Screen", canvas_menu_full_screen).id("sbxw-canvas-full-screen").menu("Window");
    if cfg!(target_os = "macos") {
        if let Ok(k) = "ctrl+cmd+f".parse::<rune::Shortcut>() {
            item = item.key_equivalent(k);
        }
    }
    let app = rune::App::new(spec).turn_scope(canvas_enter, canvas_leave).on_close(canvas_closed).menu_item(item);
    rune::WindowSpec::from_app(CANVAS, app)
}

/// Rune's side: the main window's handle, and the canvas window's once open.
pub struct RunePlatform {
    pub primary: rune::AppHandle<App>,
    pub canvas: Option<rune::WindowHandle>,
}

impl Platform for RunePlatform {
    fn run(&mut self, op: &Native) {
        match op {
            Native::Open(CANVAS) => self.canvas = Some(rune::open_window(canvas_spec())),
            Native::Open(k) => log::warn!("windows: no window of key {k}"),
            Native::Focus(_) => {
                if let Some(h) = &self.canvas {
                    h.open();
                    h.focus();
                }
            }
            Native::Close(_) => {
                if let Some(h) = self.canvas.take() {
                    h.close();
                }
            }
            Native::FocusPrimary => self.primary.open(),
        }
    }
}
