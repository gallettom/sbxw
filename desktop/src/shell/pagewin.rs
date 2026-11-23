//! A page of panes in its own window, and back (stage 3 of the windows
//! study, design A3: `.sbxw-artifacts/reports/windows-canvas-study.md`).
//!
//! * **Out**: "Move Page to New Window" (the pager's menus, the palette,
//!   ⌘⇧M / Ctrl+Shift+M) takes the current page off the main window's pages,
//!   with its panes, terminals included ([`App::move_page_out`]). Nothing is
//!   copied and no socket reopens: each terminal shows in exactly one
//!   window, and its PTY size follows the one tile that shows it (its grid's
//!   layout in that window). The main window keeps its other pages (an
//!   empty one when that was its only page).
//! * **Its window** ([`PageWin`]): the page's tiling with each pane's bar
//!   (the same bar as in the main window, `pane_bar`), the keyboard and
//!   focus of its own, no pager, no sidebar, no Jarvis. A header names the
//!   page and offers Full screen and "Move page back".
//! * **Back**: closing the window (its close button, ⌘W), its menu item
//!   "Move Page Back to Main Window", its header button or the same chord
//!   appends the page to the main window, a new page at the end, its focused
//!   pane remembered; the main window keeps its own page and focus
//!   ([`App::move_page_back`]). A window close never closes a terminal or a
//!   session.
//! * **Who owns what.** The primary keeps the model: between turns the
//!   page ([`WinPage`]: its tiling, panes and focus) waits in a slot here;
//!   the window takes it for its own turn (its turn scope) and gives it
//!   back, and the primary takes it back for its turns (`App::windowed`),
//!   as the canvas window's screen does (windows.rs). The window polls its
//!   terminals itself, with its own `Tx`: its output, redraws and wakes are
//!   its own ([`crate::client::WakeRoute`]: a moved pane's socket wakes this
//!   window, never the main one).
//! * **Its bars' presses** run on the app in the primary, inside the
//!   window's page (`App::in_window`: for that action the window's page is
//!   the app's only page, so every pane action is the main window's own
//!   code), through the hub ([`crate::windows::Hub::post`]), which refuses
//!   them while a gate holds the window. Its bars are drawn by the primary
//!   at the end of each of its turns, again only when what they show
//!   changed ([`App::chrome_fingerprint`]).
//! * **Gates** (the person's rule, as for the canvas window): a core
//!   confirmation or a copy offer raised by what was done in this window is
//!   drawn and answered here only; everything else (approvals, Jarvis,
//!   plans, what the main window or the daemon raised) stays in the main
//!   window, and this window shows a scrim saying where it is, refuses its
//!   input and its terminals take no key. While the main window holds the
//!   keyboard for a proposal, no terminal here takes a key either.
//!   Closing the window cancels its own gate, never confirms it.
//! * **Dictation** stays the main window's: a page window's microphone is
//!   drawn disabled, "Dictation works in the main window."

use super::*;
use crate::windows::{self as w, Gate, RunFn, ToPrimary, Win};
use std::cell::RefCell;
use std::collections::{BTreeMap, VecDeque};

/// A page window's size when it opens, and its smallest, in points.
pub const SIZE: (f32, f32) = (1100.0, 720.0);
pub const MIN: (f32, f32) = (480.0, 320.0);

/// A page in its own window: its tiling, its panes (terminals included)
/// and its focus. Owned by the primary's model; lent to the window for
/// each of its turns.
pub struct WinPage {
    /// The window's number (`Win::Page`).
    pub id: u32,
    pub page: pages::Page,
    /// The page's panes, in reading order (keys `pane-<n>` in the window).
    pub panes: Vec<Pane>,
    /// The pane that has the keyboard here.
    pub focused: usize,
    /// The pane whose bar's menu is open.
    pub bar_menu: Option<usize>,
    /// Rune's keyboard focus moves to the focused pane's grid next poll.
    pub refocus: bool,
    /// The window's content size (points), once laid out.
    pub size: (f32, f32),
    /// Each pane's session as last read (its "wants you"), and its keyword
    /// pills, by pane: the primary's data, read for the window.
    pub sessions: Vec<Option<terminal::SessionSeen>>,
    pub pills: Vec<Option<Rc<crate::emphasis::Vocab>>>,
}

impl WinPage {
    /// Its panes' terminals wake this window (once it has a waker).
    pub fn route_wakes(&self) {
        let Some(wk) = w::waker(Win::Page(self.id)) else { return };
        for r in self.panes.iter().filter_map(|p| p.wake.as_ref()) {
            if !r.targets(&wk) {
                r.set(wk.clone());
            }
        }
    }

    fn focused_uid(&self) -> Option<Uid> {
        self.panes.get(self.focused).map(|p| p.uid)
    }
}

/// What the primary draws for a page window (its turns have the app): each
/// pane's bar with its ground and group colour, the open bar menu, the copy
/// offer that belongs to the window, the title and the zoom.
pub struct Chrome {
    pub title: String,
    pub bars: HashMap<Uid, (El, u32, Option<u32>)>,
    pub menu: Option<El>,
    pub copy: Option<approval::CopyOffer>,
    pub zoom: f32,
}

thread_local! {
    /// The page windows' pages between turns, by number.
    static PAGES: RefCell<BTreeMap<u32, WinPage>> = const { RefCell::new(BTreeMap::new()) };
    /// Each page window's bars, and what they were drawn from.
    static CHROME: RefCell<HashMap<u32, (String, Rc<Chrome>)>> = RefCell::new(HashMap::new());
    /// The numbers of the windows opened and not yet made (Rune's `init`
    /// takes no argument): taken in order.
    static INIT: RefCell<VecDeque<u32>> = const { RefCell::new(VecDeque::new()) };
}

/// Empty slots (tests: one app per run; `windows::reset`).
pub fn reset() {
    PAGES.with(|p| p.borrow_mut().clear());
    CHROME.with(|c| c.borrow_mut().clear());
    INIT.with(|q| q.borrow_mut().clear());
}

/// The start of a primary turn: the page windows' pages come back.
pub fn primary_enter(app: &mut App) {
    let back: Vec<WinPage> = PAGES.with(|p| std::mem::take(&mut *p.borrow_mut())).into_values().collect();
    app.windowed.extend(back);
    app.windowed.sort_by_key(|w| w.id);
}

/// The end of a primary turn: each page window's bars are drawn again when
/// what they show changed (that window is then woken), its title follows
/// its page, and the pages go to their slots for the windows' turns.
pub fn primary_leave(app: &mut App) {
    let ids: Vec<u32> = app.windowed.iter().map(|w| w.id).collect();
    for id in ids {
        let seen = CHROME.with(|c| c.borrow().get(&id).map(|(fp, _)| fp.clone()));
        let got = app.in_window(id, |a| {
            let fp = a.chrome_fingerprint(id);
            let chrome = (seen.as_deref() != Some(fp.as_str())).then(|| a.page_chrome(id));
            let sessions = a.panes.iter().map(|p| terminal::session_for(a.feats.data.get("sessions"), p.sandbox.as_deref(), p.thread.as_deref(), p.mode == Mode::Agent)).collect::<Vec<_>>();
            let pills = a.panes.iter().map(|p| crate::feat::emphasis::for_pane(a.feats.data.get("emphasis"), &a.store.get("features")["emphasis"], p.sandbox.as_deref())).collect::<Vec<_>>();
            (fp, chrome, sessions, pills, a.page_title())
        });
        let Some((fp, chrome, sessions, pills, title)) = got else { continue };
        if let Some(wp) = app.windowed.iter_mut().find(|w| w.id == id) {
            wp.sessions = sessions;
            wp.pills = pills;
        }
        w::with(|h| h.set_page_title(id, title));
        if let Some(c) = chrome {
            CHROME.with(|m| m.borrow_mut().insert(id, (fp, Rc::new(c))));
            w::with(|h| h.mark(Win::Page(id)));
        }
    }
    let out: Vec<WinPage> = std::mem::take(&mut app.windowed);
    PAGES.with(|p| {
        let mut p = p.borrow_mut();
        for wp in out {
            p.insert(wp.id, wp);
        }
    });
}

impl App {
    /// The waker a terminal opened now must wake: its page window's, while
    /// a page window's action runs (`in_window`), else the main window's.
    pub(crate) fn window_waker(&self, primary: Waker) -> crate::client::Wake {
        self.swapped.and_then(|n| w::waker(Win::Page(n))).unwrap_or(primary)
    }

    /// The main window holds the keyboard for Jarvis's proposal: no page
    /// window's terminal takes a key (the same gate as `terminal_may_take_keys`).
    pub(crate) fn keys_held_for_windows(&self) -> bool {
        self.held_focus.is_some() || self.proposal_up()
    }

    /// The page windows open, by number.
    pub fn page_windows(&self) -> Vec<u32> {
        self.windowed.iter().map(|w| w.id).collect()
    }

    /// What the current page shows, as a window's title: its sessions' names.
    fn page_title(&self) -> String {
        let names: Vec<String> = self.panes.iter().filter(|p| p.sandbox.is_some()).map(pane_name).collect();
        if names.is_empty() {
            "Empty page".into()
        } else {
            names.join(", ")
        }
    }

    /// "Move Page to New Window": page `k` (from 0) and its panes leave the
    /// main window for a window of their own. Refused in Lead Mode (its view
    /// is the main window's) and for a page with nothing open. Returns the
    /// window's number.
    pub fn move_page_out(&mut self, k: usize) -> Option<u32> {
        if self.swapped.is_some() {
            return None;
        }
        if self.leads.is_some() {
            self.notify("Turn Lead Mode off first: a page moves to its own window from the usual layout", false);
            return None;
        }
        let page = self.pages.list.get(k)?.clone();
        let uids = page.tile.order();
        if uids.iter().all(|u| self.pane_by_uid(*u).is_none_or(|p| p.sandbox.is_none())) {
            self.notify("This page shows no terminal: nothing to move", false);
            return None;
        }
        let title = {
            let names: Vec<String> = uids.iter().filter_map(|u| self.pane_by_uid(*u)).filter(|p| p.sandbox.is_some()).map(pane_name).collect();
            names.join(", ")
        };
        // The main window's focus stays where it was, unless it was on this page.
        let keep = self.focused_uid().filter(|u| !uids.contains(u));
        let fresh = self.fresh();
        self.pages.close_page(k, fresh);
        let mut by: HashMap<Uid, Pane> = std::mem::take(&mut self.panes).into_iter().map(|p| (p.uid, p)).collect();
        let mut panes: Vec<Pane> = uids.iter().map(|u| by.remove(u).unwrap_or_else(|| Pane::empty(*u))).collect();
        self.panes = by.into_values().collect();
        for (j, p) in panes.iter_mut().enumerate() {
            if let Some(t) = p.term.as_mut() {
                // Keys `grid-<n>`, `terminal-<n>` in its window.
                t.id = (j + 1).to_string();
            }
        }
        let focused = page.focus.and_then(|f| uids.iter().position(|u| *u == f)).unwrap_or(0);
        let id = w::with(|h| h.open_page(title));
        let size = self.window;
        self.windowed.push(WinPage { id, page, panes, focused, bar_menu: None, refocus: true, size, sessions: vec![], pills: vec![] });
        self.bar_menu = None;
        // The main window's panes follow its pages again (and the layout is saved, the moved page with it).
        self.sync_panes(keep);
        log::info!("windows: page {} moved to page window {id}", k + 1);
        self.push_data();
        Some(id)
    }

    /// The page of window `id` comes back to the main window: a new page at
    /// the end, its panes and terminals as they are, its focused pane
    /// remembered; the main window keeps its current page and its focus
    /// (an empty only page gives way to it). Its window closes if it is
    /// still open. Nothing is closed and nothing reconnects.
    pub fn move_page_back(&mut self, id: u32) -> bool {
        let Some(at) = self.windowed.iter().position(|w| w.id == id) else { return false };
        let wp = self.windowed.remove(at);
        w::with(|h| h.close_page(id));
        if let Some(pw) = self.waker.clone() {
            for r in wp.panes.iter().filter_map(|p| p.wake.as_ref()) {
                r.set(pw.clone());
            }
        }
        let mut page = wp.page;
        page.focus = wp.panes.get(wp.focused).map(|p| p.uid);
        let back_focus = page.focus;
        // The main window showing nothing at all (its only page had moved): this page takes its place.
        let only_empty = self.pages.list.len() == 1 && self.panes.iter().all(|p| p.sandbox.is_none());
        let keep = if only_empty {
            self.pages.list.clear();
            self.panes.clear();
            self.pages.current = 0;
            back_focus
        } else {
            self.focused_uid()
        };
        self.pages.list.push(page);
        self.panes.extend(wp.panes);
        self.sync_panes(keep);
        log::info!("windows: page window {id} moved back to the main window");
        self.push_data();
        true
    }

    /// Runs `f` with window `id`'s page as the app's only page (its panes
    /// as `panes`, its focus as `focused`, its size as the window's): every
    /// pane action is then the main window's own code, by index. Afterwards
    /// the page goes back to the window, and a page the action added (a
    /// terminal opened where the window had no room) joins the main window.
    /// None when there is no such window.
    pub(crate) fn in_window<R>(&mut self, id: u32, f: impl FnOnce(&mut App) -> R) -> Option<R> {
        if self.swapped.is_some() {
            return None;
        }
        let at = self.windowed.iter().position(|w| w.id == id)?;
        let mut wp = self.windowed.remove(at);
        let per = self.pages.per.max(wp.page.tile.count());
        let own = Pages { list: vec![wp.page.clone()], current: 0, per, grid_per: self.pages.grid_per };
        let main_pages = std::mem::replace(&mut self.pages, own);
        let main_panes = std::mem::replace(&mut self.panes, std::mem::take(&mut wp.panes));
        let main_focused = std::mem::replace(&mut self.focused, wp.focused);
        let main_menu = std::mem::replace(&mut self.bar_menu, wp.bar_menu);
        let main_refocus = std::mem::replace(&mut self.refocus, false);
        let main_window = std::mem::replace(&mut self.window, wp.size);
        let main_drop = self.rects.pane_drop.take();
        let main_drag = self.drag.take();
        let main_leads = self.leads.take();
        let main_grid = self.grid_saved.take();
        self.swapped = Some(id);
        let r = f(self);
        self.swapped = None;
        let mut pages = std::mem::replace(&mut self.pages, main_pages);
        let panes = std::mem::replace(&mut self.panes, main_panes);
        let focus = panes.get(self.focused).map(|p| p.uid);
        self.focused = main_focused;
        wp.bar_menu = std::mem::replace(&mut self.bar_menu, main_menu);
        wp.refocus |= std::mem::replace(&mut self.refocus, main_refocus);
        wp.size = std::mem::replace(&mut self.window, main_window);
        self.rects.pane_drop = main_drop;
        self.drag = main_drag;
        self.leads = main_leads;
        self.grid_saved = main_grid;
        let mut by: HashMap<Uid, Pane> = panes.into_iter().map(|p| (p.uid, p)).collect();
        let first = pages.list.remove(0);
        wp.panes = first.tile.order().into_iter().map(|u| by.remove(&u).unwrap_or_else(|| Pane::empty(u))).collect();
        wp.page = first;
        wp.focused = focus.and_then(|u| wp.panes.iter().position(|p| p.uid == u)).unwrap_or(0);
        for (j, p) in wp.panes.iter_mut().enumerate() {
            if let Some(t) = p.term.as_mut() {
                t.id = (j + 1).to_string();
            }
        }
        wp.route_wakes();
        // A page the action made (no room in the window): the main window's now.
        let extra = !pages.list.is_empty();
        for p in pages.list {
            for u in p.tile.order() {
                if let Some(pane) = by.remove(&u) {
                    if let (Some(pw), Some(r)) = (self.waker.clone(), pane.wake.as_ref()) {
                        r.set(pw);
                    }
                    self.panes.push(pane);
                }
            }
            self.pages.list.push(p);
        }
        // What no page holds any more closes (its terminal's socket with it), as `sync_panes` does.
        drop(by);
        self.windowed.insert(at, wp);
        if extra {
            let keep = self.focused_uid();
            self.sync_panes(keep);
        }
        Some(r)
    }

    /// What a page window's bars are drawn from (inside `in_window`): when
    /// it is the same as last time, they are not drawn again and the window
    /// is not woken.
    pub(crate) fn chrome_fingerprint(&self, id: u32) -> String {
        let colors = self.group_colors();
        let copy = (w::copy_elsewhere_in() == Some(Win::Page(id))).then(|| self.copy_offer.as_ref().map(w::copy_id)).flatten();
        let panes: Vec<String> = (0..self.panes.len())
            .map(|i| {
                let p = &self.panes[i];
                format!(
                    "{:?} {:?}",
                    (
                        p.uid,
                        &p.sandbox,
                        p.mode.name(),
                        &p.thread,
                        &p.title,
                        p.attention,
                        p.status(),
                        &p.error,
                    ),
                    (
                        self.pane_fit(i),
                        self.pane_dictate_state(i),
                        p.sandbox.as_deref().and_then(|n| colors.get(n)),
                        self.pane_leads(p),
                        self.agent_label(i),
                    )
                )
            })
            .collect();
        format!("{:?}", (theme::generation(), self.zoom(), self.focused, self.bar_menu, (self.window.0 as i32, self.window.1 as i32), copy, &self.feats.panel, self.feats.split, &self.feats.panels, panes))
    }

    /// The state file holds the page windows' pages too, after the main
    /// window's: a quit with page windows open loses no pane (they come
    /// back in the main window at the next start; restoring the windows
    /// themselves is a later stage's).
    pub(crate) fn windowed_layout(&self, pages: &mut Pages, panes: &mut Vec<J>, idx: &mut HashMap<Uid, Uid>) {
        for wp in &self.windowed {
            for p in &wp.panes {
                idx.insert(p.uid, panes.len() as Uid);
                panes.push(serde_json::json!({ "sandbox": p.sandbox, "mode": p.mode.name(), "thread": p.thread }));
            }
            pages.list.push(wp.page.clone());
        }
    }

    /// A page window's bars, its open menu and its copy offer (inside `in_window`).
    pub(crate) fn page_chrome(&self, id: u32) -> Chrome {
        let bars = (0..self.panes.len()).map(|i| (self.panes[i].uid, pane_bar::<PageWin>(self, i))).collect();
        let copy = (w::copy_elsewhere_in() == Some(Win::Page(id)))
            .then(|| self.copy_offer.as_ref().map(|o| approval::CopyOffer { sandbox: o.sandbox.clone(), text: o.text.clone(), bytes: o.bytes, until: o.until }))
            .flatten();
        Chrome { title: self.page_title(), bars, menu: self.bar_menu_view_as::<PageWin>(), copy, zoom: self.zoom() }
    }

    /// The palette's and the menu's "Move Page Back to Main Window" for
    /// window `id`, from the main window.
    pub fn page_window_back(&mut self, id: u32) {
        // Its own confirmation or copy offer is cancelled first, never confirmed.
        let win = Win::Page(id);
        if self.feats.dialog.is_some() && w::with(|h| h.dialog_owner()) == Some(win) {
            approval::ConfirmAnswers::cancel_click(self);
        }
        if self.copy_offer.is_some() && w::with(|h| h.copy_owner()) == Some(win) {
            self.copy_offer = None;
        }
        self.move_page_back(id);
    }
}

// ---------------------------------------------------------------------------
// The window

/// A page window's state: its page during its turns, and what it shows of
/// the gates. Nothing else of the app.
pub struct PageWin {
    pub id: u32,
    pub page: Option<WinPage>,
    chrome: Option<Rc<Chrome>>,
    /// The confirmation that belongs to this window, during its turns.
    dialog: Option<Dialog>,
    /// The gate drawn in another window that holds this one.
    gate: Option<Gate>,
    /// The main window holds the keyboard for a proposal.
    held: bool,
    /// This window has the keyboard (its terminal's cursor is solid).
    focused_window: bool,
    /// What was last told to the primary: each bar's width.
    bar_w: HashMap<Uid, f32>,
    /// Polls that found something to draw (tests: an idle window has none).
    pub drawn: u64,
}

impl PageWin {
    pub fn new(id: u32) -> PageWin {
        PageWin { id, page: None, chrome: None, dialog: None, gate: None, held: false, focused_window: false, bar_w: HashMap::new(), drawn: 0 }
    }

    /// Rune's `init`: the next window opened.
    fn init() -> PageWin {
        PageWin::new(INIT.with(|q| q.borrow_mut().pop_front()).unwrap_or(0))
    }

    pub fn win(&self) -> Win {
        Win::Page(self.id)
    }

    fn post(&mut self, m: ToPrimary) -> bool {
        w::with(|h| h.post(Win::Page(self.id), m))
    }

    pub fn shown_gate(&self) -> Option<&Gate> {
        self.gate.as_ref()
    }

    pub fn dialog(&self) -> Option<&Dialog> {
        self.dialog.as_ref()
    }

    pub fn copy_offer(&self) -> Option<&approval::CopyOffer> {
        self.chrome.as_ref().and_then(|c| c.copy.as_ref())
    }

    /// Something over its panes: another window's gate, its own
    /// confirmation or copy offer, or the main window holding the keyboard.
    /// No terminal here takes input then (by its grid's handlers, `lens`, or
    /// by the window's keys).
    pub fn covered(&self) -> bool {
        self.gate.is_some() || self.dialog.is_some() || self.copy_offer().is_some() || self.held
    }

    /// The window's keyboard gate (the main window's `terminal_may_take_keys`).
    pub fn terminal_may_take_keys(&self, focus: Option<&str>) -> bool {
        !self.covered() && focus.is_some_and(|k| k.starts_with("grid"))
    }

    fn term(&mut self, i: usize) -> Option<&mut TerminalState> {
        self.page.as_mut()?.panes.get_mut(i)?.term.as_mut()
    }

    /// The page goes back to the main window (its menu item, its button,
    /// the chord). Not while another window's gate holds this one.
    pub fn back(&mut self) {
        if self.gate.is_some() {
            w::with(|h| h.refused += 1);
            return;
        }
        // Its own confirmation and copy offer are cancelled first, then the page goes.
        w::with(|h| h.close_page(self.id));
        self.post(ToPrimary::Closed);
    }

    /// Its confirmation's answer, to the primary, which runs its own
    /// `confirm_click` or `cancel_click` for that confirmation alone.
    fn answer(&mut self, confirm: bool) {
        if let Some(id) = self.dialog.as_ref().map(Dialog::identity) {
            self.post(ToPrimary::Answer { confirm, dialog: id });
        }
    }

    fn focus_local(&mut self, i: usize) {
        if let Some(p) = self.page.as_mut() {
            if i < p.panes.len() {
                if p.focused != i {
                    p.focused = i;
                    self.post(ToPrimary::Changed);
                }
                if let Some(p) = self.page.as_mut() {
                    p.refocus = true;
                }
            }
        }
    }

    /// A key the focused control did not take: full screen, the way back,
    /// a pane's focus, then the focused terminal through the keyboard gate.
    pub fn on_key(&mut self, ev: &KeyEvent, tx: &mut Tx) -> bool {
        if w::full_screen_key(ev) {
            tx.toggle_full_screen();
            return true;
        }
        let terminal = tx.focused().is_some_and(|k| k.starts_with("grid"));
        let chord = palette::chord(ev, terminal);
        if chord == Some('m') && ev.mods.shift {
            self.back();
            return true;
        }
        // Its own gate is modal (the scrim takes Escape, Tab and its
        // buttons' keys); a gate of another window: no key here answers it.
        if self.covered() {
            w::with(|h| h.refused += 1);
            return true;
        }
        if ev.key == Key::Escape && self.page.as_ref().is_some_and(|p| p.bar_menu.is_some()) {
            if let Some(p) = self.page.as_mut() {
                p.bar_menu = None;
            }
            self.post(ToPrimary::Changed);
            return true;
        }
        if let Some(c @ '1'..='9') = chord {
            self.focus_local(c as usize - '1' as usize);
            return true;
        }
        if !self.terminal_may_take_keys(tx.focused()) {
            return false;
        }
        let Some(p) = self.page.as_mut() else { return false };
        let f = p.focused;
        let was = p.panes.get_mut(f).is_some_and(|pane| std::mem::take(&mut pane.attention));
        if was {
            self.post(ToPrimary::Changed);
        }
        match self.term(f) {
            Some(t) => t.on_key_tx(ev, tx, lens(f)),
            None => false,
        }
    }

    /// The input method (dead keys, CJK, emoji): to the focused pane, through the gate.
    pub fn on_ime(&mut self, ime: &rune::Ime, focus: Option<&str>) -> bool {
        if !self.terminal_may_take_keys(focus) {
            return false;
        }
        let f = self.page.as_ref().map_or(0, |p| p.focused);
        self.term(f).is_some_and(|t| t.on_ime(ime))
    }

    /// Takes in its terminals' output and events with its own `Tx` (its own
    /// wake-ups, blink and debounces); what concerns the app goes to the
    /// primary. True when something must be drawn again.
    pub fn poll(&mut self, clock: f64, tx: &mut Tx) -> bool {
        let dirty = w::with(|h| h.take_dirty(Win::Page(self.id)));
        let Some(p) = self.page.as_mut() else { return dirty };
        p.route_wakes();
        let mut changed = dirty;
        let mut news = false;
        if std::mem::take(&mut p.refocus) {
            if let Some(t) = p.panes.get(p.focused).and_then(|x| x.term.as_ref()) {
                tx.focus(t.key("grid"), false);
            }
        } else if let Some(k) = tx.focused().map(str::to_string) {
            if let Some(i) = p.panes.iter().position(|x| x.term.as_ref().is_some_and(|t| t.key("grid") == k)) {
                if i != p.focused {
                    p.focused = i;
                    news = true;
                    changed = true;
                }
            }
        }
        let mut runs: Vec<(bool, RunFn)> = vec![];
        let now = Instant::now();
        let focused = p.focused;
        for (i, pane) in p.panes.iter_mut().enumerate() {
            let Some(t) = &mut pane.term else { continue };
            t.set_focused(self.focused_window && i == focused);
            t.set_pills(p.pills.get(i).cloned().flatten());
            let before = (pane.title.clone(), pane.attention, pane.reconnecting.clone(), t.status.clone());
            let mut ch = t.poll_tx(None, clock, tx);
            let uid = pane.uid;
            let name = pane.sandbox.clone().unwrap_or_default();
            for ev in t.take_events() {
                ch = true;
                match ev {
                    TerminalEvent::Attention => pane.attention = true,
                    TerminalEvent::Bell => log::info!("page window {}: pane {}: bell", self.id, i + 1),
                    TerminalEvent::Title(title) => pane.title = Some(title).filter(|t| !t.is_empty()),
                    // OSC 52, by size as in the main window: a short copy is
                    // made here, a long one offered here (its offer belongs
                    // to this window), a too long one dropped with a notice.
                    TerminalEvent::Clipboard(text) => match approval::osc_tier(text.len()) {
                        approval::OscTier::Copy if !text.is_empty() => {
                            let bytes = text.len();
                            tx.copy(text);
                            let who = if name.is_empty() { "the sandbox".to_string() } else { name.clone() };
                            runs.push((false, RunFn(Rc::new(move |a: &mut App| a.notify(format!("Copied {} from {who}", approval::size_label(bytes)), false)))));
                        }
                        approval::OscTier::Copy => {}
                        _ => {
                            let (name, text) = (name.clone(), Rc::new(text));
                            runs.push((
                                false,
                                RunFn(Rc::new(move |a: &mut App| {
                                    if let Some(i) = a.index_of(uid) {
                                        a.osc_copy(i, &name, (*text).clone(), now, &mut |_| {});
                                    }
                                })),
                            ));
                        }
                    },
                    TerminalEvent::Disconnected(why) => pane.reconnecting = Some(why),
                    TerminalEvent::Reconnected => pane.reconnecting = None,
                }
            }
            // A file path clicked with the modifier (terminal/filelink.rs).
            if let Some(path) = t.take_file_click() {
                runs.push((true, RunFn(Rc::new(move |a: &mut App| {
                    if let Some(i) = a.index_of(uid) {
                        a.pane_file_open(i, path.clone());
                    }
                }))));
            }
            // A picture pasted with no text (the key's clipboard read, off macOS).
            if let Some(png) = t.take_pasted_image() {
                runs.push((true, RunFn(Rc::new(move |a: &mut App| {
                    if let Some(i) = a.index_of(uid) {
                        a.pane_paste_image(i, &png);
                    }
                }))));
            }
            let wants = t.wants_you(p.sessions.get(i).and_then(|s| s.as_ref()));
            ch |= std::mem::replace(&mut pane.attention, wants) != wants;
            news |= before != (pane.title.clone(), pane.attention, pane.reconnecting.clone(), t.status.clone());
            changed |= ch;
        }
        for (input, f) in runs {
            self.post(ToPrimary::Run { f, input });
        }
        if news {
            self.post(ToPrimary::Changed);
        }
        if changed {
            self.drawn += 1;
        }
        changed
    }
}

impl WinSide for PageWin {
    const MAIN: bool = false;
    fn act(&mut self, input: bool, f: Rc<dyn Fn(&mut App)>) {
        self.post(ToPrimary::Run { f: RunFn(f), input });
    }
    fn bar_width(&mut self, uid: Uid, width: f32) {
        let was = self.bar_w.insert(uid, width);
        if was.is_none_or(|x| (x - width).abs() > 0.5) {
            self.act(false, Rc::new(move |a: &mut App| {
                a.bar_w.insert(uid, width);
            }));
        }
    }
}

/// The core's confirmation, drawn in this window by `approval::confirm_view`:
/// the toggles and secret fields act on the confirmation itself, Confirm
/// and Cancel go to the primary as this window's answer.
impl approval::ConfirmAnswers for PageWin {
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
        &mut self.dialog.as_mut().expect("the page window's confirmation").secrets[i]
    }
}

/// Pane `i`'s terminal for its grid's handlers: none while anything covers
/// this window's panes ([`PageWin::covered`]).
fn lens(i: usize) -> impl Fn(&mut PageWin) -> Option<&mut TerminalState> + Clone + 'static {
    move |s: &mut PageWin| if s.covered() { None } else { s.page.as_mut()?.panes.get_mut(i)?.term.as_mut() }
}

/// Pane `i`'s terminal for its layout (fitting it to its tile): kept under
/// a gate, as the main window's is under a dialog.
fn layout_lens(i: usize) -> impl Fn(&mut PageWin) -> Option<&mut TerminalState> + Clone + 'static {
    move |s: &mut PageWin| s.page.as_mut()?.panes.get_mut(i)?.term.as_mut()
}

/// The window's turn starts: its page from the slot, its bars, its
/// confirmation when it has one, and the gates as they are.
pub fn page_enter(s: &mut PageWin) {
    if s.page.is_none() {
        s.page = PAGES.with(|p| p.borrow_mut().remove(&s.id));
    }
    s.chrome = CHROME.with(|c| c.borrow().get(&s.id).map(|(_, c)| c.clone()));
    if s.dialog.is_none() {
        s.dialog = w::take_dialog_of(s.win());
    }
    let win = s.win();
    s.gate = w::with(|h| h.gate_for(win).cloned());
    s.held = w::with(|h| h.keys_held());
}

/// The window's turn ends: the page and its confirmation go back, and the
/// primary is woken when something waits for it.
pub fn page_leave(s: &mut PageWin) {
    if let Some(p) = s.page.take() {
        PAGES.with(|m| m.borrow_mut().insert(p.id, p));
    }
    if let Some(d) = s.dialog.take() {
        w::put_dialog_of(s.win(), d);
    }
    w::secondary_leave();
}

pub fn page_setup(s: &mut PageWin, waker: Waker) {
    w::set_waker(s.win(), waker);
}

pub fn page_poll(s: &mut PageWin, clock: f64, tx: &mut Tx) -> bool {
    s.poll(clock, tx)
}

/// Its close button, ⌘W, or the app quitting: the page goes back to the
/// main window (the primary hears it once).
pub fn page_closed(s: &mut PageWin, _tx: &mut Tx) {
    w::with(|h| h.window_closed(Win::Page(s.id)));
}

fn tile_view(s: &PageWin, p: &WinPage, t: &Tile) -> El {
    match t {
        Tile::Leaf(u) => match p.panes.iter().position(|x| x.uid == *u) {
            Some(i) => pane_view(s, p, i),
            None => el().grow(1.0),
        },
        Tile::Split { dir, ratio, a, b } => {
            let vertical = *dir == tiling::Dir::Row;
            let first = tile_view(s, p, a).grow(*ratio).min_w(0.0).min_h(0.0);
            let second = tile_view(s, p, b).grow(1.0 - *ratio).min_w(0.0).min_h(0.0);
            let gap = if vertical { el().w(layout::GUTTER).h_full() } else { el().h(layout::GUTTER).w_full() };
            let base = if vertical { row() } else { col() };
            base.w_full().h_full().min_w(0.0).min_h(0.0).items_stretch().child(first).child(gap).child(second)
        }
    }
}

/// Pane `i` of the window: its top line, its bar (drawn by the primary),
/// and its terminal (drawn here, from the terminal this window holds).
fn pane_view(s: &PageWin, p: &WinPage, i: usize) -> El {
    let pane = &p.panes[i];
    let uid = pane.uid;
    let n = i + 1;
    let focused = i == p.focused;
    let chrome = s.chrome.as_ref().and_then(|c| c.bars.get(&uid));
    let (bar, group) = match chrome {
        Some((b, _, gr)) => (b.clone(), *gr),
        None => (row().key(format!("pane-{n}-bar")).h(layout::PANE_BAR_H).w_full().fill(c("surface")), None),
    };
    let body = match pane {
        Pane { term: Some(t), .. } => row().w_full().grow(1.0).min_h(0.0).child(terminal::pane_held(t, lens(i), layout_lens(i), false)),
        Pane { sandbox: None, .. } => col()
            .key(format!("pane-{n}-empty"))
            .grow(1.0)
            .w_full()
            .padding(24.0)
            .gap(8.0)
            .items_center()
            .justify_center()
            .child(rune::icon(crate::icons::get("terminal").unwrap(), theme::icon::LARGE).color(c("text-muted")))
            .child(text("Nothing open in this pane").font(small(theme::font_size::HEADING, 600.0)).color(c("text-bright")))
            .child(text("Move the page back to the main window to open a sandbox here.").font(theme::prose(theme::font_size::BODY, 400.0)).color(c("text-muted")).wrap().max_w(360.0)),
        Pane { sandbox: Some(sb), error, .. } => {
            let msg = match error {
                Some(e) => e.clone(),
                None => format!("{sb}: {}", pane.status()),
            };
            col().grow(1.0).w_full().padding(16.0).child(text(msg).font(small(theme::font_size::BODY, 400.0)).color(c(if error.is_some() { "red" } else { "text-muted" })).wrap().w_full())
        }
    };
    let accent = group.map(theme::hex).unwrap_or_else(|| c("border"));
    let (bw, border) = if focused { (1.5, c("accent")) } else { (1.0, c("elevated")) };
    let md = theme::radius("md").unwrap();
    col()
        .key(format!("pane-{n}"))
        .w_full()
        .h_full()
        .min_w(0.0)
        .min_h(0.0)
        .fill(c("bg"))
        .border(bw, border)
        .radius(md)
        .clip_rounded(md)
        // A press anywhere on a pane gives its terminal the keyboard.
        .on_press(move |s: &mut PageWin| s.focus_local(i))
        // Files dropped on a pane: to the feature that takes them, through the hub (a gate refuses them).
        .on_drop_files(move |s: &mut PageWin, files: &[PathBuf], _tx| {
            if s.covered() {
                w::with(|h| h.refused += 1);
                return;
            }
            let files = files.to_vec();
            s.post(ToPrimary::Run {
                f: RunFn(Rc::new(move |a: &mut App| {
                    if let Some(i) = a.index_of(uid) {
                        a.pane_drop(i, &files);
                    }
                })),
                input: true,
            });
        })
        .child(el().key(format!("pane-{n}-accent")).h(JOIN_W).w_full().fill(accent))
        .child(bar)
        .child(el().h(1.0).w_full().fill(c("elevated")))
        .child(body)
}

/// The page window's view: a header (the page's names, full screen, back),
/// the page's tiling, an open bar menu, and over everything its own
/// confirmation or copy offer, or another window's gate.
pub fn page_view(s: &PageWin, cx: &mut Cx<PageWin>) -> El {
    cx.web_focus();
    cx.focus_ring(crate::theme::focus::W, crate::theme::focus::OUT, c("focus"));
    if let Some(ch) = &s.chrome {
        cx.zoom(ch.zoom);
    }
    cx.help_look(help_look());
    let mut root = col()
        .key("pwin")
        .w_full()
        .h_full()
        .fill(c("floor"))
        .padding(layout::GUTTER)
        .gap(layout::GUTTER)
        .on_layout(|s: &mut PageWin, r: [f32; 4]| {
            if let Some(p) = s.page.as_mut() {
                p.size = (r[2], r[3]);
            }
        })
        .on_window_focus(|s: &mut PageWin, focused: bool, _tx: &mut Tx| s.focused_window = focused);
    // ⌘V on macOS (Rune hands the root the clipboard): to the focused
    // terminal through the keyboard gate; a picture alone to its sandbox.
    if window_paste() {
        root = root.on_paste_content(|s: &mut PageWin, content: &rune::ClipboardContent, tx: &mut Tx| {
            if !s.terminal_may_take_keys(tx.focused()) {
                if tx.focused().is_some_and(|k| k.starts_with("grid")) {
                    w::with(|h| h.refused += 1);
                }
                return;
            }
            let f = s.page.as_ref().map_or(0, |p| p.focused);
            if !content.text().is_empty() {
                if let Some(t) = s.term(f) {
                    t.paste(content.text());
                }
            } else if let Some(img) = content.image() {
                let (png, uid) = (img.png().to_vec(), s.page.as_ref().and_then(WinPage::focused_uid).unwrap_or(0));
                s.post(ToPrimary::Run {
                    f: RunFn(Rc::new(move |a: &mut App| {
                        if let Some(i) = a.index_of(uid) {
                            a.pane_paste_image(i, &png);
                        }
                    })),
                    input: true,
                });
            }
        });
    }
    let title = s.chrome.as_ref().map_or_else(|| "Page".to_string(), |c| c.title.clone());
    let head = row()
        .key("pwin-head")
        .w_full()
        .h(layout::CONTROL_H)
        .gap(8.0)
        .items_center()
        .child(rune::icon(crate::icons::get("single").unwrap(), theme::icon::MEDIUM).color(c("text-bright")))
        .child(text(title).font(small(theme::font_size::TITLE, 600.0)).color(c("text-bright")).truncate().shrink().min_w(0.0))
        .child(el().grow(1.0).min_w(0.0))
        .child(w::button("pwin-full-screen", "Full screen", w::full_screen_help(), false, |_s: &mut PageWin, tx: &mut Tx| tx.toggle_full_screen()))
        .child(w::button("pwin-back", "Move page back", &format!("Moves this page back to the main window, as its last page ({})", back_keys().join("")), false, |s: &mut PageWin, _tx: &mut Tx| s.back()));
    let area = match &s.page {
        Some(p) => col().key("pwin-area").grow(1.0).w_full().min_h(0.0).child(tile_view(s, p, &p.page.tile)),
        // Between hand-overs (the main window holds the page for a moment).
        None => col().key("pwin-wait").grow(1.0).w_full().items_center().justify_center().child(text("The page is in the main window.").font(small(theme::font_size::BODY, 400.0)).color(c("text-muted"))),
    };
    root = root.child(head).child(area);
    if let Some(m) = s.chrome.as_ref().and_then(|c| c.menu.clone()) {
        if s.page.as_ref().is_some_and(|p| p.bar_menu.is_some()) {
            root = root.child(screen::menu_scrim("pane-more-scrim".into(), |s: &mut PageWin| {
                if let Some(p) = s.page.as_mut() {
                    p.bar_menu = None;
                }
                s.post(ToPrimary::Changed);
            }));
            root = root.child(m);
        }
    }
    let size = s.page.as_ref().map_or(SIZE, |p| p.size);
    if let Some(d) = &s.dialog {
        let top = layout::GUTTER * 2.0 + 24.0;
        let dialog = approval::confirm_view::<PageWin>(&d.state((size.0.max(MIN.0), size.1.max(MIN.1)), top));
        root = root.child(scrim("cf-scrim", dialog, |s: &mut PageWin| approval::ConfirmAnswers::cancel_click(s)));
    } else if let Some(o) = s.copy_offer() {
        root = root.child(approval::copy_offer_view::<PageWin>(
            o,
            |s: &mut PageWin, _tx: &mut Tx| {
                s.post(ToPrimary::CopyAnswer { copy: true });
            },
            |s: &mut PageWin| {
                s.post(ToPrimary::CopyAnswer { copy: false });
            },
        ));
    } else if let Some(g) = &s.gate {
        root = root.child(w::gate_scrim::<PageWin>(g));
    }
    root
}

/// "Move Page to New Window" and back: ⌘⇧M on macOS, Ctrl+Shift+M elsewhere.
pub fn back_keys() -> Vec<String> {
    if cfg!(target_os = "macos") {
        vec!["⌘".into(), "⇧".into(), "M".into()]
    } else {
        vec!["Ctrl".into(), "Shift".into(), "M".into()]
    }
}

/// The menu bar's items while the window is the key window.
fn menu_back(s: &mut PageWin, _tx: &mut Tx) {
    s.back();
}

fn menu_full_screen(_s: &mut PageWin, tx: &mut Tx) {
    tx.toggle_full_screen();
}

/// Page window `id`'s spec: its own state (the page moved in for its
/// turns), a standard frame, its keys and its menu items.
pub fn page_spec(id: u32) -> rune::WindowSpec<PageWin> {
    INIT.with(|q| q.borrow_mut().push_back(id));
    let spec = rune::AppSpec::<PageWin> {
        setup: Some(page_setup),
        poll: Some(page_poll),
        on_key: Some(|s: &mut PageWin, ev: &KeyEvent, tx: &mut Tx| s.on_key(ev, tx)),
        min_size: Some(MIN),
        families: (crate::terminal::MONO_FAMILIES, crate::terminal::MONO_FAMILIES),
        ..rune::AppSpec::new("sbxw page", SIZE, PageWin::init, page_view)
    };
    let mut back = rune::MenuItem::new("Move Page Back to Main Window", menu_back).id("sbxw-page-back").menu("Window");
    let mut full = rune::MenuItem::new("Toggle Full Screen", menu_full_screen).id("sbxw-page-full-screen").menu("Window");
    if cfg!(target_os = "macos") {
        if let Ok(k) = "cmd+shift+m".parse::<rune::Shortcut>() {
            back = back.key_equivalent(k);
        }
        if let Ok(k) = "ctrl+cmd+f".parse::<rune::Shortcut>() {
            full = full.key_equivalent(k);
        }
    }
    let app = rune::App::new(spec)
        .turn_scope(page_enter, page_leave)
        .on_close(page_closed)
        .on_ime(|s: &mut PageWin, ime: &rune::Ime, tx: &mut Tx| s.on_ime(ime, tx.focused()))
        .menu_item(back)
        .menu_item(full);
    rune::WindowSpec::from_app(Win::Page(id).to_string(), app)
}

fn small(size: f32, weight: f32) -> TextStyle {
    screen::body(size, weight)
}
