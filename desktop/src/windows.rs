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
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::PathBuf;
use std::rc::Rc;

/// A window of the app: the main one, the canvas window, or a page window
/// (shell/pagewin.rs) by its number.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Win {
    Main,
    Canvas,
    Page(u32),
}

impl std::fmt::Display for Win {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Win::Main => write!(f, "main"),
            Win::Canvas => write!(f, "canvas"),
            Win::Page(n) => write!(f, "page-{n}"),
        }
    }
}

/// The canvas window's key (one canvas window: the Canvas view has one
/// current canvas and one surface, `feat::canvas::SURF`).
pub const CANVAS: Win = Win::Canvas;
/// The screen the canvas window shows.
pub const CANVAS_SCREEN: &str = crate::feat::canvas::SCREEN;
/// The canvas window's size when it opens, and its smallest, in points.
pub const CANVAS_SIZE: (f32, f32) = (1200.0, 800.0);
pub const CANVAS_MIN: (f32, f32) = (640.0, 420.0);

/// Something a page window asks the primary to run on the app, inside that
/// window's page (`App::in_window`): a pane bar's press, a menu item, a
/// drop, a pasted picture, a program's copy. Only through [`Hub::post`],
/// which refuses it while a gate holds the window (when it is input).
#[derive(Clone)]
pub struct RunFn(pub Rc<dyn Fn(&mut App)>);

impl std::fmt::Debug for RunFn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "RunFn")
    }
}

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
    /// The window closed (its close button, ⌘W, its menu item): its view or
    /// its page goes back to the main window.
    Closed,
    /// The answer to the confirmation this window shows (its Confirm, or
    /// Cancel, ×, Escape, a press outside): `dialog` is its identity
    /// (`Dialog::identity`), so it answers that one or nothing.
    Answer { confirm: bool, dialog: String },
    /// The answer to the copy offer this window shows (Copy, or Not now).
    CopyAnswer { copy: bool },
    /// A page window's action on the app ([`RunFn`]); `input`: the person's
    /// (a press, a drop), which a gate holds, else the window's own news
    /// (a bar laid out, a program's copy).
    Run { f: RunFn, input: bool },
    /// What a page window shows changed on its side (a pane's focus, a
    /// title, a connection): the primary draws its bars again.
    Changed,
}

impl ToPrimary {
    /// Whether it is the person's input to the view, which a gate holds
    /// (an answer is the gate's own input: see [`Hub::post`]).
    pub fn is_input(&self) -> bool {
        match self {
            ToPrimary::Resized | ToPrimary::Closed | ToPrimary::Answer { .. } | ToPrimary::CopyAnswer { .. } | ToPrimary::Changed => false,
            ToPrimary::Run { input, .. } => *input,
            _ => true,
        }
    }
}

/// What the hub asks of the platform (Rune), in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Native {
    /// Open the window of that key.
    Open(Win),
    /// Bring that window to the front.
    Focus(Win),
    /// Close that window.
    Close(Win),
    /// Bring the main window to the front.
    FocusPrimary,
}

/// Where a gate is drawn and answered, and what the other windows say about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Gate {
    /// The window that draws it, and alone answers it: [`PRIMARY`], or the
    /// window whose input raised it (a confirmation, a copy offer).
    pub owner: Win,
    /// One line naming what waits, said in the other windows.
    pub what: String,
}

/// The owner of every gate not raised in another window.
pub const PRIMARY: Win = Win::Main;

/// The hub's model: which windows are open, what goes to the primary, the
/// gates, and what the platform must do.
#[derive(Debug, Default)]
pub struct Hub {
    canvas_open: bool,
    /// The canvas window's content size (points), once laid out.
    canvas_size: Option<[f32; 2]>,
    to_primary: Vec<(Win, ToPrimary)>,
    native: Vec<Native>,
    /// Every gate up, in order, each with the window that draws it.
    gates: Vec<Gate>,
    /// The confirmation up (its identity) and the window it belongs to.
    dialog: Option<(String, Win)>,
    /// The copy offer up (its identity) and the window it belongs to.
    copy: Option<(String, Win)>,
    /// The page windows open, by number, with their titles.
    pages: BTreeMap<u32, String>,
    next_page: u32,
    /// The main window holds the keyboard for Jarvis's proposal
    /// (`App::held_focus`): no terminal of any window takes a key.
    keys_held: bool,
    /// The windows that must draw again.
    dirty: BTreeSet<Win>,
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
        self.dirty.insert(CANVAS);
        self.native.push(Native::Open(CANVAS));
        Opened::New
    }

    /// Brings the canvas window to the front, if it is open.
    pub fn focus_canvas(&mut self) -> bool {
        self.focus(CANVAS)
    }

    /// Brings window `w` to the front, if it is open (the main one always is).
    pub fn focus(&mut self, w: Win) -> bool {
        match w {
            Win::Main => self.native.push(Native::FocusPrimary),
            w if self.is_open(w) => self.native.push(Native::Focus(w)),
            _ => return false,
        }
        true
    }

    /// The view goes back to the main window: the canvas window closes.
    /// Its `on_close` then reports [`Self::window_closed`], which finds it
    /// closed already and says nothing more. A confirmation it showed is
    /// cancelled (never confirmed).
    pub fn close_canvas(&mut self) -> bool {
        let was = std::mem::take(&mut self.canvas_open);
        if was {
            self.cancel_gates_of(CANVAS);
            self.native.push(Native::Close(CANVAS));
            self.canvas_size = None;
        }
        was
    }

    /// A new page window (its page is moved there by the caller): its number.
    pub fn open_page(&mut self, title: String) -> u32 {
        self.next_page += 1;
        let n = self.next_page;
        self.pages.insert(n, title);
        self.dirty.insert(Win::Page(n));
        self.native.push(Native::Open(Win::Page(n)));
        n
    }

    /// The page goes back to the main window by the main window's own
    /// doing (its menu item, the palette): the window closes, and what it
    /// showed of a gate is cancelled. Its `on_close` then says nothing more.
    pub fn close_page(&mut self, n: u32) -> bool {
        let was = self.pages.remove(&n).is_some();
        if was {
            self.cancel_gates_of(Win::Page(n));
            self.dirty.remove(&Win::Page(n));
            self.native.push(Native::Close(Win::Page(n)));
        }
        was
    }

    /// The page windows open, by number.
    pub fn page_windows(&self) -> Vec<u32> {
        self.pages.keys().copied().collect()
    }

    pub fn page_title(&self, n: u32) -> Option<&str> {
        self.pages.get(&n).map(String::as_str)
    }

    /// A page window's title follows what its page shows.
    pub fn set_page_title(&mut self, n: u32, title: String) {
        if let Some(t) = self.pages.get_mut(&n) {
            *t = title;
        }
    }

    /// A window closed by the person (its close button, ⌘W, the menu): a
    /// confirmation or copy offer it showed is cancelled, and the primary
    /// hears it once and puts the view (or the page) back.
    pub fn window_closed(&mut self, key: Win) {
        let was = match key {
            Win::Canvas => {
                let was = std::mem::take(&mut self.canvas_open);
                if was {
                    self.canvas_size = None;
                }
                was
            }
            Win::Page(n) => self.pages.remove(&n).is_some(),
            Win::Main => false,
        };
        if was {
            self.dirty.remove(&key);
            self.cancel_gates_of(key);
            self.to_primary.push((key, ToPrimary::Closed));
        }
    }

    /// The confirmation and the copy offer of window `key` are cancelled:
    /// their answers are queued (the primary runs `cancel_click`, or drops
    /// the offer), and they belong to it until then, so the answer is taken.
    fn cancel_gates_of(&mut self, key: Win) {
        if let Some((id, owner)) = &self.dialog {
            if *owner == key {
                self.to_primary.push((key, ToPrimary::Answer { confirm: false, dialog: id.clone() }));
            }
        }
        if self.copy.as_ref().is_some_and(|(_, o)| *o == key) {
            self.to_primary.push((key, ToPrimary::CopyAnswer { copy: false }));
        }
    }

    /// A message from window `from` to the primary. Returns whether it was
    /// queued. An answer is taken only from the window the confirmation (or
    /// the copy offer) belongs to, for that one; the view's input only from
    /// an open window, and only while no gate is up anywhere (the main
    /// window's, or one raised in a window, which is modal there).
    pub fn post(&mut self, from: Win, msg: ToPrimary) -> bool {
        let ok = match &msg {
            ToPrimary::Answer { dialog, .. } => self.dialog.as_ref().is_some_and(|(id, owner)| *owner == from && id == dialog),
            ToPrimary::CopyAnswer { .. } => self.copy.as_ref().is_some_and(|(_, owner)| *owner == from),
            m if m.is_input() => self.gates.is_empty() && self.dialog.is_none() && self.copy.is_none() && from != PRIMARY && self.is_open(from),
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
    pub fn dialog_raised(&mut self, id: String, from: Win) {
        let owner = if self.is_open(from) { from } else { PRIMARY };
        self.dialog = Some((id, owner));
        self.mark_all();
    }

    /// The confirmation up at the end of a primary turn (its identity, or
    /// none): one this hub has not seen came from the main window itself
    /// (its own press, its own paths), and belongs to it.
    pub fn note_dialog(&mut self, id: Option<String>) {
        let next = Self::noted(self.dialog.take(), id, |o| o == PRIMARY || self.is_open(o));
        if next != self.dialog {
            self.mark_all();
        }
        self.dialog = next;
    }

    /// A copy offer came up while the primary ran what window `from` asked
    /// (its program's copy): it is drawn and answered there.
    pub fn copy_raised(&mut self, id: String, from: Win) {
        let owner = if self.is_open(from) { from } else { PRIMARY };
        self.copy = Some((id, owner));
        self.mark_all();
    }

    /// The copy offer up at the end of a primary turn, as [`Self::note_dialog`].
    pub fn note_copy(&mut self, id: Option<String>) {
        let was = self.copy.clone();
        let next = Self::noted(self.copy.take(), id, |o| o == PRIMARY || self.is_open(o));
        if next != was {
            self.mark_all();
        }
        self.copy = next;
    }

    fn noted(seen: Option<(String, Win)>, id: Option<String>, open: impl Fn(Win) -> bool) -> Option<(String, Win)> {
        match id {
            None => None,
            Some(id) => match seen {
                Some((s, owner)) if s == id && open(owner) => Some((s, owner)),
                _ => Some((id, PRIMARY)),
            },
        }
    }

    /// The window the confirmation up belongs to.
    pub fn dialog_owner(&self) -> Option<Win> {
        self.dialog.as_ref().map(|(_, o)| *o)
    }

    /// The window the copy offer up belongs to.
    pub fn copy_owner(&self) -> Option<Win> {
        self.copy.as_ref().map(|(_, o)| *o)
    }

    /// The primary takes what the other windows asked, in order.
    pub fn take_for_primary(&mut self) -> Vec<(Win, ToPrimary)> {
        std::mem::take(&mut self.to_primary)
    }

    pub fn pending_for_primary(&self) -> bool {
        !self.to_primary.is_empty()
    }

    /// The main window's own gate, alone (`None`: none). See [`Self::set_gates`].
    pub fn set_gate(&mut self, gate: Option<Gate>) {
        self.set_gates(gate.into_iter().collect());
    }

    /// Every gate up, each with its owner ([`gates`]): each other window
    /// draws its scrim and refuses input while one is up.
    pub fn set_gates(&mut self, gates: Vec<Gate>) {
        if gates != self.gates {
            self.gates = gates;
            self.mark_all();
        }
    }

    /// The first gate up, wherever it is drawn.
    pub fn gate(&self) -> Option<&Gate> {
        self.gates.first()
    }

    /// The gate that holds window `w`: the first one drawn in another window.
    pub fn gate_for(&self, w: Win) -> Option<&Gate> {
        self.gates.iter().find(|g| g.owner != w)
    }

    /// The main window holds the keyboard for Jarvis's proposal.
    pub fn set_keys_held(&mut self, held: bool) {
        if held != self.keys_held {
            self.keys_held = held;
            self.mark_all();
        }
    }

    pub fn keys_held(&self) -> bool {
        self.keys_held
    }

    /// The person asked for the main window (a scrim's button).
    pub fn show_primary(&mut self) {
        self.native.push(Native::FocusPrimary);
    }

    pub fn is_open(&self, key: Win) -> bool {
        match key {
            Win::Main => true,
            Win::Canvas => self.canvas_open,
            Win::Page(n) => self.pages.contains_key(&n),
        }
    }

    /// The words for a window, in a sentence ("the Canvas window").
    pub fn name(&self, w: Win) -> String {
        match w {
            Win::Main => "the main window".into(),
            Win::Canvas => "the Canvas window".into(),
            Win::Page(n) => match self.pages.get(&n).filter(|t| !t.is_empty()) {
                Some(t) => format!("the page window “{t}”"),
                None => format!("page window {n}"),
            },
        }
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
            self.dirty.insert(CANVAS);
        }
    }

    /// Every open window but the main one must draw again (a gate moved).
    pub fn mark_all(&mut self) {
        if self.canvas_open {
            self.dirty.insert(CANVAS);
        }
        for n in self.pages.keys() {
            self.dirty.insert(Win::Page(*n));
        }
    }

    /// Window `w` must draw again.
    pub fn mark(&mut self, w: Win) {
        if w != PRIMARY && self.is_open(w) {
            self.dirty.insert(w);
        }
    }

    /// Whether window `w` must draw again (and clears it).
    pub fn take_dirty(&mut self, w: Win) -> bool {
        self.dirty.remove(&w)
    }

    /// Whether the canvas window must draw again (and clears it).
    pub fn take_canvas_dirty(&mut self) -> bool {
        self.take_dirty(CANVAS)
    }

    fn dirty_windows(&self) -> Vec<Win> {
        self.dirty.iter().copied().filter(|w| self.is_open(*w)).collect()
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
    /// The confirmation between turns while it belongs to another window
    /// than the main one, with that window.
    static DIALOG: RefCell<Option<(Win, Dialog)>> = const { RefCell::new(None) };
    static PLATFORM: RefCell<Option<Box<dyn Platform>>> = RefCell::new(None);
    /// The operations run with no platform installed (tests).
    static DONE: RefCell<Vec<Native>> = const { RefCell::new(Vec::new()) };
    /// Each window's Rune waker, once it is set up.
    static WAKERS: RefCell<HashMap<Win, Waker>> = RefCell::new(HashMap::new());
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
    set_waker(PRIMARY, w);
}

/// Window `w`'s waker (its `setup`).
pub fn set_waker(w: Win, waker: Waker) {
    WAKERS.with(|s| s.borrow_mut().insert(w, waker));
}

/// Window `w`'s waker, once it is set up.
pub fn waker(w: Win) -> Option<Waker> {
    WAKERS.with(|s| s.borrow().get(&w).cloned())
}

/// A new hub on this thread, its slots empty (tests: one app per run).
pub fn reset() {
    with(|h| *h = Hub::default());
    SLOT.with(|s| s.borrow_mut().take());
    DIALOG.with(|s| s.borrow_mut().take());
    DONE.with(|d| d.borrow_mut().clear());
    WAKERS.with(|s| s.borrow_mut().clear());
    crate::shell::pagewin::reset();
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

/// Wakes window `w` (its `poll` runs), once it is set up.
pub fn wake(w: Win) {
    if let Some(w) = waker(w) {
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
    focus_window(CANVAS)
}

/// A scrim's button: window `w` to the front.
pub fn focus_window(w: Win) -> bool {
    let open = with(|h| h.focus(w));
    run_native();
    open
}

/// Every gate up, each with the window that draws and answers it: the
/// core's confirmation and the copy offer (in the window they belong to,
/// [`Hub::dialog_owner`], [`Hub::copy_owner`]), and in the main window an
/// approval, Jarvis's proposal, a modal, the palette.
pub fn gates(app: &App) -> Vec<Gate> {
    let mut v = vec![];
    let main = |what: &str| Gate { owner: PRIMARY, what: what.into() };
    if !app.approvals.is_empty() {
        v.push(main("An approval waits for you in the main window."));
    }
    if app.feats.dialog.is_some() {
        match with(|h| h.dialog_owner().filter(|o| *o != PRIMARY && h.is_open(*o)).map(|o| (o, h.name(o)))) {
            Some((o, name)) => v.push(Gate { owner: o, what: format!("A confirmation waits for you in {name}.") }),
            None => v.push(main("A confirmation waits for you in the main window.")),
        }
    }
    if app.copy_offer.is_some() {
        match with(|h| h.copy_owner().filter(|o| *o != PRIMARY && h.is_open(*o)).map(|o| (o, h.name(o)))) {
            Some((o, name)) => v.push(Gate { owner: o, what: format!("A copy to the clipboard waits for your answer in {name}.") }),
            None => v.push(main("A copy to the clipboard waits for your answer in the main window.")),
        }
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

/// The window other than the main one the confirmation up belongs to (the
/// main window then draws [`elsewhere_scrim`], never the confirmation).
pub fn dialog_elsewhere_in() -> Option<Win> {
    with(|h| h.dialog_owner().filter(|o| *o != PRIMARY && h.is_open(*o)))
}

/// Whether the confirmation up belongs to another window than the main one.
pub fn dialog_elsewhere() -> bool {
    dialog_elsewhere_in().is_some()
}

/// The window other than the main one the copy offer up belongs to.
pub fn copy_elsewhere_in() -> Option<Win> {
    with(|h| h.copy_owner().filter(|o| *o != PRIMARY && h.is_open(*o)))
}

/// A copy offer's identity, to know it again turn after turn.
pub fn copy_id(o: &crate::approval::CopyOffer) -> String {
    format!("{}:{}:{:?}", o.sandbox, o.bytes, o.until)
}

/// The start of a primary turn: the Canvas screen comes back to
/// `App::screens`, a confirmation another window held to `feats.dialog`,
/// and the page windows' pages to `App::windowed`.
pub fn primary_enter(app: &mut App) {
    if let Some((i, sc)) = SLOT.with(|s| s.borrow_mut().take()) {
        let i = i.min(app.screens.len());
        app.screens.insert(i, sc);
    }
    if let Some((w, d)) = DIALOG.with(|s| s.borrow_mut().take()) {
        if app.feats.dialog.is_none() {
            app.feats.dialog = Some(d);
        } else {
            log::warn!("windows: two confirmations at once; the {w} window's is dropped");
        }
    }
    crate::shell::pagewin::primary_enter(app);
}

/// The end of a primary turn: the gates are published, the Canvas screen
/// (and a confirmation that belongs to another window) move out while
/// that window is open, the page windows' pages too (their bars drawn
/// again when what they show changed), the platform runs what was asked,
/// and each window whose view changed is woken.
pub fn primary_leave(app: &mut App) {
    with(|h| {
        h.note_dialog(app.feats.dialog.as_ref().map(Dialog::identity));
        h.note_copy(app.copy_offer.as_ref().map(copy_id));
    });
    let gates = gates(app);
    let held = app.keys_held_for_windows();
    with(|h| {
        h.set_gates(gates);
        h.set_keys_held(held);
    });
    if let Some(w) = dialog_elsewhere_in() {
        if let Some(d) = app.feats.dialog.take() {
            DIALOG.with(|s| *s.borrow_mut() = Some((w, d)));
        }
    }
    if canvas_open() {
        if let Some(i) = app.screens.iter().position(|s| s.name == CANVAS_SCREEN) {
            let renders = app.screens[i].renders;
            with(|h| h.note_drawn_from(renders, crate::surface_canvas::generation()));
            let sc = app.screens.remove(i);
            SLOT.with(|s| *s.borrow_mut() = Some((i, sc)));
        }
    }
    crate::shell::pagewin::primary_leave(app);
    run_native();
    for w in with(|h| h.dirty_windows()) {
        wake(w);
    }
}

/// The confirmation that belongs to window `w`, for its turn.
pub fn take_dialog_of(w: Win) -> Option<Dialog> {
    DIALOG.with(|s| {
        let mut s = s.borrow_mut();
        if s.as_ref().is_some_and(|(o, _)| *o == w) {
            s.take().map(|(_, d)| d)
        } else {
            None
        }
    })
}

/// Window `w`'s turn ends: its confirmation goes back.
pub fn put_dialog_of(w: Win, d: Dialog) {
    DIALOG.with(|s| *s.borrow_mut() = Some((w, d)));
}

/// A secondary window's turn ends: the platform runs what it asked, and
/// the primary is woken when something waits for it.
pub fn secondary_leave() {
    run_native();
    if with(|h| h.pending_for_primary()) {
        wake(PRIMARY);
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
        s.dialog = take_dialog_of(CANVAS);
    }
    s.gate = with(|h| h.gate_for(CANVAS).cloned());
}

/// The canvas window's turn ends: the screen and its confirmation go back,
/// and the primary is woken when something waits for it.
pub fn canvas_leave(s: &mut CanvasWin) {
    if let Some(sc) = s.screen.take() {
        SLOT.with(|sl| *sl.borrow_mut() = Some(sc));
    }
    if let Some(d) = s.dialog.take() {
        put_dialog_of(CANVAS, d);
    }
    secondary_leave();
}

pub fn canvas_setup(_s: &mut CanvasWin, waker: Waker) {
    set_waker(CANVAS, waker);
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

pub(crate) fn small(size: f32, weight: f32) -> rune::TextStyle {
    screen::body(size, weight)
}

/// A plain button of the window's chrome.
pub(crate) fn button<S: 'static>(key: &str, label: &str, help: &str, primary: bool, press: impl Fn(&mut S, &mut Tx) + 'static) -> El {
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
        .hover(if primary { c("blue").alpha(0.85) } else { c("hover-subtle") })
        .cursor(Cursor::Pointer)
        .focusable(true)
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
        root = root.child(gate_scrim::<CanvasWin>(g));
    }
    root
}

pub(crate) fn full_screen_help() -> &'static str {
    if cfg!(target_os = "macos") {
        "Full screen (⌃⌘F, or the green button)"
    } else {
        "Full screen (F11)"
    }
}

/// Over a secondary window (the canvas window, a page window) while a gate
/// drawn in another window is up: modal (no press, key or Tab reaches what
/// is under it), saying where to answer, with a button that brings that
/// window to the front.
pub fn gate_scrim<S: 'static>(g: &Gate) -> El {
    let (name, owner) = (with(|h| h.name(g.owner)), g.owner);
    let title = format!("Answer in {name}");
    let card = col()
        .key("cwin-gate")
        .role("alertdialog")
        .label(title.clone())
        .w(420.0)
        .padding(16.0)
        .gap(10.0)
        .radius(radius("md").unwrap())
        .fill(c("surface"))
        .border(1.0, c("border"))
        .child(text(title).font(small(theme::font_size::TITLE, 600.0)).color(c("text-bright")))
        .child(text(g.what.clone()).font(small(theme::font_size::BODY, 400.0)).color(c("text")).wrap().w_full())
        .child(text("Nothing here can answer it: this window waits until it is answered there.").font(small(theme::font_size::CAPTION, 400.0)).color(c("text-muted")).wrap().w_full())
        .child(row().w_full().child(el().grow(1.0)).child(button(
            "cwin-gate-show",
            &format!("Show {name}"),
            "Brings that window to the front",
            true,
            move |_s: &mut S, _tx: &mut Tx| {
                focus_window(owner);
            },
        )));
    el().key("cwin-scrim")
        .pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
        .fill(theme::scrim())
        .modal(false, |_s: &mut S| {})
        .on_press(|_s: &mut S| {})
        .on_wheel(|_s: &mut S, _ev: rune::WheelEvent, _tx: &mut Tx| true)
        .on_drop_files(|_s: &mut S, _files: &[PathBuf], _tx| with(|h| h.refused += 1))
        .items_center()
        .justify_center()
        .child(card)
}

/// Over the main window while a confirmation (or a copy offer) raised in
/// window `owner` waits there: a press, the wheel or a drop on it reaches
/// nothing and answers nothing; its button brings that window to the front.
pub fn elsewhere_scrim(owner: Win, copy: bool) -> El {
    let name = with(|h| h.name(owner));
    let short = match owner {
        Win::Canvas => "Show the Canvas window".to_string(),
        Win::Page(_) => "Show the page window".to_string(),
        Win::Main => "Show the main window".to_string(),
    };
    let card = col()
        .key("cwin-elsewhere")
        .role("alertdialog")
        .label(format!("Answer in {name}"))
        .w(420.0)
        .padding(16.0)
        .gap(10.0)
        .radius(radius("md").unwrap())
        .fill(c("surface"))
        .border(1.0, c("border"))
        .on_press(|_s: &mut App| {})
        .child(text(format!("Answer in {name}")).font(small(theme::font_size::TITLE, 600.0)).color(c("text-bright")))
        .child(text(if copy { format!("A copy to the clipboard, asked by a program in {name}, waits there for your answer. It is answered there only.") } else { format!("A confirmation you asked for in {name} waits there. It is answered there only.") }).font(small(theme::font_size::BODY, 400.0)).color(c("text")).wrap().w_full())
        .child(row().w_full().child(el().grow(1.0)).child(button("cwin-elsewhere-show", &short, "Brings that window to the front", true, move |_s: &mut App, _tx: &mut Tx| {
            focus_window(owner);
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
    rune::WindowSpec::from_app(CANVAS.to_string(), app)
}

/// Rune's side: the main window's handle, and each other window's once
/// open (kept, so a later stage can read and restore its placement).
pub struct RunePlatform {
    pub primary: rune::AppHandle<App>,
    pub canvas: Option<rune::WindowHandle>,
    pub pages: HashMap<u32, rune::WindowHandle>,
}

impl RunePlatform {
    fn handle(&self, w: Win) -> Option<&rune::WindowHandle> {
        match w {
            Win::Canvas => self.canvas.as_ref(),
            Win::Page(n) => self.pages.get(&n),
            Win::Main => None,
        }
    }
}

impl Platform for RunePlatform {
    fn run(&mut self, op: &Native) {
        match op {
            Native::Open(CANVAS) => self.canvas = Some(rune::open_window(canvas_spec())),
            Native::Open(Win::Page(n)) => {
                let h = rune::open_window(crate::shell::pagewin::page_spec(*n));
                self.pages.insert(*n, h);
            }
            Native::Open(k) => log::warn!("windows: no window of key {k}"),
            Native::Focus(w) => {
                if let Some(h) = self.handle(*w) {
                    h.open();
                    h.focus();
                }
            }
            Native::Close(w) => {
                let h = match w {
                    Win::Canvas => self.canvas.take(),
                    Win::Page(n) => self.pages.remove(n),
                    Win::Main => None,
                };
                if let Some(h) = h {
                    h.close();
                }
            }
            Native::FocusPrimary => self.primary.open(),
        }
    }
}

#[cfg(test)]
#[path = "windows_tests.rs"]
mod tests;
