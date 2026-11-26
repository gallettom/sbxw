//! A page of panes in its own window and back (pagewin.rs), with the main
//! window and each page window as headless runtimes driven turn by turn the
//! way Rune drives them (each window's turn scope around its frames).

use super::pagewin::{self as pw, PageWin};
use super::*;
use crate::client::{TermEvent, TermInput, TerminalConn, Wake, WakeRoute};
use crate::windows::{self as w, Native, RunFn, ToPrimary, Win};
use rune::{KeyEvent, Mods, Runtime};
use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};
use std::sync::mpsc;

/// The main window and the page windows opened, with each terminal's input
/// and event sender by its sandbox's name, and each window's wake count.
struct Rig {
    rt: Runtime<App>,
    wins: Vec<Runtime<PageWin>>,
    inputs: HashMap<String, mpsc::Receiver<TermInput>>,
    events: HashMap<String, mpsc::Sender<TermEvent>>,
    wakes: HashMap<Win, Arc<AtomicUsize>>,
    clock: f64,
}

fn counter() -> (Arc<AtomicUsize>, Wake) {
    let n = Arc::new(AtomicUsize::new(0));
    let m = n.clone();
    (n, Arc::new(move || {
        m.fetch_add(1, SeqCst);
    }))
}

fn key(t: &str) -> KeyEvent {
    KeyEvent { key: Key::Text(t.into()), mods: Mods::default() }
}

fn bytes(rx: &mpsc::Receiver<TermInput>) -> Vec<u8> {
    rx.try_iter().flat_map(|i| if let TermInput::Bytes(b) = i { b } else { vec![] }).collect()
}

fn sizes(rx: &mpsc::Receiver<TermInput>) -> Vec<(u16, u16)> {
    rx.try_iter().filter_map(|i| if let TermInput::Resize { cols, rows } = i { Some((cols, rows)) } else { None }).collect()
}

/// A picture to paste (the drops dialog takes it).
/// A file to drop on a pane: the drops dialog is the confirmation these tests
/// raise from a page window (a pasted picture asks nothing any more).
fn dropped(tag: &str) -> Vec<std::path::PathBuf> {
    let f = std::env::temp_dir().join(format!("sbxw-pagewin-{tag}-{}.md", std::process::id()));
    std::fs::write(&f, b"# Brief").unwrap();
    vec![f]
}

fn b64(data: &[u8]) -> String {
    const T: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for c in data.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        for k in 0..4 {
            if k <= c.len() {
                out.push(T[(n >> (18 - 6 * k) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

impl Rig {
    /// Live terminals named `names`, `per` to a page, in a window 1284×816.
    fn new(names: &[&str], per: usize) -> Rig {
        w::reset();
        let fixture = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/files/standin.json");
        let mut a = App::new(Args { fixture: Some(fixture.into()), ..Args::default() });
        a.store = store::Store::memory();
        super::tests::set_panes(&mut a, &names.iter().map(|n| Some(*n)).collect::<Vec<_>>());
        let (main_n, main_w) = counter();
        a.waker = Some(main_w.clone());
        w::set_primary_waker(main_w.clone());
        let (mut inputs, mut events) = (HashMap::new(), HashMap::new());
        for (i, n) in names.iter().enumerate() {
            let (ev_tx, ev_rx) = mpsc::channel();
            let (in_tx, in_rx) = mpsc::channel();
            let mut t = TerminalState::new(TerminalConn { events: ev_rx, input: in_tx });
            t.id = (i + 1).to_string();
            a.panes[i].term = Some(t);
            a.panes[i].wake = Some(WakeRoute::new(main_w.clone()));
            inputs.insert(n.to_string(), in_rx);
            events.insert(n.to_string(), ev_tx);
        }
        a.run_intent("test", Intent::Pages(PageOp::Per(per)));
        let mut rt = Runtime::new(a, view, (1284.0, 816.0), rune::Env::new(true, 1.0));
        rt.on_key = Some(|s: &mut App, ev: &KeyEvent, tx: &mut Tx| s.on_key(ev, tx));
        rt.use_clipboard(&rune::clipboard::memory());
        let mut r = Rig { rt, wins: vec![], inputs, events, wakes: HashMap::from([(Win::Main, main_n)]), clock: 0.0 };
        r.settle();
        r
    }

    /// A turn of the main window.
    fn main<R>(&mut self, f: impl FnOnce(&mut Runtime<App>) -> R) -> R {
        self.rt.update(|s, _| w::primary_enter(s));
        let r = f(&mut self.rt);
        self.rt.update(|s, _| w::primary_leave(s));
        r
    }

    /// A turn of page window `k` (in the order opened).
    fn win<R>(&mut self, k: usize, f: impl FnOnce(&mut Runtime<PageWin>) -> R) -> R {
        let pt = &mut self.wins[k];
        pt.update(|s, _| pw::page_enter(s));
        let r = f(pt);
        pt.update(|s, _| pw::page_leave(s));
        r
    }

    /// Frames of the main window, its per-frame work as main.rs runs it.
    fn main_frames(&mut self, n: usize) {
        for _ in 0..n {
            self.clock += 0.03;
            let clock = self.clock;
            self.main(|rt| {
                rt.frame(0.03);
                rt.update(|s, tx| {
                    s.poll(clock, tx);
                });
            });
        }
    }

    /// Frames of page window `k`; whether one of them had something to draw.
    fn win_frames(&mut self, k: usize, n: usize) -> bool {
        let mut any = false;
        for _ in 0..n {
            self.clock += 0.03;
            let clock = self.clock;
            any |= self.win(k, |pt| {
                let due = pt.frame(0.03);
                let mut ch = false;
                pt.update(|s, tx| ch = pw::page_poll(s, clock, tx));
                due || ch
            });
        }
        any
    }

    /// Every window, in turns, about a second of the clock.
    fn settle(&mut self) {
        for _ in 0..12 {
            self.main_frames(1);
            for k in 0..self.wins.len() {
                self.win_frames(k, 1);
            }
        }
        self.main_frames(1);
    }

    /// "Move Page to New Window" on page `k` of the main window, and the
    /// window Rune opens for it (its waker counted).
    fn move_out(&mut self, k: usize) -> u32 {
        let id = self.main(|rt| {
            let mut id = None;
            rt.update(|s, _| id = s.move_page_out(k));
            id.expect("the page moved")
        });
        assert!(w::take_done().contains(&Native::Open(Win::Page(id))), "its window opened");
        let mut pt = Runtime::new(PageWin::new(id), pw::page_view, pw::SIZE, rune::Env::new(true, 1.0));
        pt.on_key = Some(|s: &mut PageWin, ev: &KeyEvent, tx: &mut Tx| s.on_key(ev, tx));
        pt.use_clipboard(&rune::clipboard::memory());
        let (n, wk) = counter();
        pt.update(|s, _| pw::page_setup(s, wk));
        self.wakes.insert(Win::Page(id), n);
        self.wins.push(pt);
        self.settle();
        id
    }

    /// Reads the app inside a main window turn (between turns, the page
    /// windows' pages and their confirmation are in the hub's slots).
    fn app<R>(&mut self, f: impl FnOnce(&App) -> R) -> R {
        self.main(|rt| f(&rt.state))
    }

    fn wakes(&self, win: Win) -> usize {
        self.wakes[&win].load(SeqCst)
    }

    fn tap_win(&mut self, k: usize, key: &str) {
        self.win(k, |pt| {
            pt.frame(0.03);
            let (x, y) = pt.find(key).unwrap_or_else(|| panic!("{key} drawn in the page window"));
            pt.pointer_down(x, y);
            pt.pointer_up(x, y);
            pt.frame(0.03);
        });
        self.settle();
    }

    fn tap_main(&mut self, key: &str) {
        self.main(|rt| {
            rt.frame(0.03);
            let (x, y) = rt.find(key).unwrap_or_else(|| panic!("{key} drawn in the main window"));
            rt.pointer_down(x, y);
            rt.pointer_up(x, y);
            rt.frame(0.03);
        });
        self.settle();
    }

    fn drawn_main(&mut self, key: &str) -> bool {
        self.main(|rt| {
            rt.frame(0.03);
            rt.find(key).is_some()
        })
    }

    fn drawn_win(&mut self, k: usize, key: &str) -> bool {
        self.win(k, |pt| {
            pt.frame(0.03);
            pt.find(key).is_some()
        })
    }

    /// The main window's sandboxes, page by page.
    fn main_names(&self) -> Vec<Vec<String>> {
        let s = &self.rt.state;
        s.pages.list.iter().map(|p| p.tile.order().iter().map(|u| s.pane_by_uid(*u).and_then(|p| p.sandbox.clone()).unwrap_or_default()).collect()).collect()
    }

    /// A page window's sandboxes, between turns (in its slot).
    fn win_names(&mut self, k: usize) -> Vec<String> {
        self.win(k, |pt| pt.state.page.as_ref().map(|p| p.panes.iter().map(|x| x.sandbox.clone().unwrap_or_default()).collect()).unwrap_or_default())
    }

    /// Each terminal's identity (a new terminal has a new one).
    fn generations(&mut self) -> HashMap<String, String> {
        let gen = |p: &Pane| p.term.as_ref().map(|t| (p.sandbox.clone().unwrap_or_default(), t.fit_key().rsplit_once("-g").unwrap().1.to_string()));
        let mut v: HashMap<String, String> = self.rt.state.panes.iter().filter_map(gen).collect();
        for k in 0..self.wins.len() {
            let more: Vec<(String, String)> = self.win(k, |pt| pt.state.page.as_ref().map(|p| p.panes.iter().filter_map(gen).collect()).unwrap_or_default());
            v.extend(more);
        }
        v
    }

    /// Types `t` into page window `k`'s focused terminal (its grid has the keyboard).
    fn type_win(&mut self, k: usize, t: &str) {
        self.win(k, |pt| {
            pt.update(|s, tx| {
                let f = s.page.as_ref().map_or(0, |p| p.focused);
                tx.focus(&format!("grid-{}", f + 1), false);
            });
            pt.key(key(t));
            pt.frame(0.03);
        });
    }

    /// A page window's action on pane 1, as its bar or its paste would ask.
    fn run_from(&mut self, id: u32, f: impl Fn(&mut App) + 'static) -> bool {
        w::with(|h| h.post(Win::Page(id), ToPrimary::Run { f: RunFn(Rc::new(f)), input: true }))
    }
}

/// Out: the page's panes leave the main window with their terminals (the
/// same ones, open), the main window keeps its other page and its focus,
/// the window draws the page's panes with their bars, and its keys reach its
/// own focused terminal. Back (its button): a new page at the end of the
/// main window, the same terminals again, the main window's focus kept and
/// the page's own focus remembered.
#[test]
fn a_page_moves_out_and_back_with_its_panes_focus_and_terminals() {
    let mut r = Rig::new(&["web-app", "api", "docs"], 2);
    assert_eq!(r.main_names(), [vec!["web-app", "api"], vec!["docs"]]);
    let before = r.generations();
    // Page 2 shows in the main window, its pane focused; page 1 moves out.
    r.main(|rt| rt.update(|s, _| s.run_intent("test", Intent::Pages(PageOp::Show(1)))));
    r.main(|rt| {
        rt.update(|s, _| {
            s.pages.list[0].focus = s.panes.get(1).map(|p| p.uid);
        })
    });
    r.settle();
    let id = r.move_out(0);
    assert_eq!(r.main_names(), [vec!["docs"]], "the moved page left the main window's pages");
    assert_eq!(r.rt.state.panes.len(), 1);
    assert_eq!(r.win_names(0), ["web-app", "api"]);
    assert_eq!(r.generations(), before, "no terminal recreated, none copied");
    assert!(r.drawn_win(0, "pane-1-bar") && r.drawn_win(0, "pane-2-bar") && r.drawn_win(0, "grid-1") && r.drawn_win(0, "grid-2"));
    assert!(!r.drawn_win(0, "pager"), "no pager in a page window");
    assert!(r.drawn_win(0, "pane-1-close") && r.drawn_win(0, "pwin-back"));
    // The page's remembered focus (api) has the window's keyboard.
    assert_eq!(r.win(0, |pt| pt.state.page.as_ref().unwrap().focused), 1);
    r.type_win(0, "x");
    assert_eq!(bytes(&r.inputs["api"]), b"x", "the window's key reached its focused terminal");
    assert!(bytes(&r.inputs["web-app"]).is_empty() && bytes(&r.inputs["docs"]).is_empty());
    // A press on pane 1 of the window gives it the keyboard there.
    r.tap_win(0, "grid-1");
    r.settle();
    assert_eq!(r.win(0, |pt| pt.state.page.as_ref().unwrap().focused), 0);
    r.type_win(0, "y");
    assert_eq!(bytes(&r.inputs["web-app"]), b"y");
    // The main window's own focus never moved.
    assert_eq!(r.rt.state.panes[r.rt.state.focused].sandbox.as_deref(), Some("docs"));
    // Back, from the window's button.
    r.tap_win(0, "pwin-back");
    r.settle();
    assert!(!w::with(|h| h.is_open(Win::Page(id))));
    assert!(w::take_done().contains(&Native::Close(Win::Page(id))));
    assert_eq!(r.main_names(), [vec!["docs"], vec!["web-app", "api"]], "a new page at the end");
    assert_eq!(r.rt.state.pages.current, 0, "the main window keeps its page");
    assert_eq!(r.rt.state.panes[r.rt.state.focused].sandbox.as_deref(), Some("docs"), "and its focus");
    let back = &r.rt.state.pages.list[1];
    assert_eq!(back.focus.and_then(|u| r.rt.state.pane_by_uid(u)).and_then(|p| p.sandbox.clone()).as_deref(), Some("web-app"), "the page remembers its focus");
    assert_eq!(r.generations(), before, "the same terminals, back");
    assert!(r.rt.state.panes.iter().all(|p| p.term.as_ref().is_some_and(|t| !t.status.starts_with("closed"))));
    assert!(r.app(|a| a.windowed.is_empty()));
    // The layout saved has every pane, in the main window's pages.
    let saved = r.rt.state.store.get("layout");
    assert_eq!(saved["panes"].as_array().map(Vec::len), Some(3), "{saved}");
}

/// Closing the window (its close button, ⌘W) is the same way back, and
/// closes no terminal; the main window's only page moving out leaves it an
/// empty page, which the page takes again on its way back.
#[test]
fn closing_the_window_brings_the_page_back_and_closes_no_terminal() {
    let mut r = Rig::new(&["web-app", "api"], 4);
    let before = r.generations();
    let id = r.move_out(0);
    assert_eq!(r.main_names(), [vec![String::new()]], "an empty page stays in the main window");
    r.win(0, |pt| pt.update(|s, tx| pw::page_closed(s, tx)));
    r.settle();
    assert!(!w::with(|h| h.is_open(Win::Page(id))));
    assert_eq!(r.main_names(), [vec!["web-app", "api"]], "the page took the empty one's place");
    assert_eq!(r.generations(), before);
    for n in ["web-app", "api"] {
        r.events[n].send(TermEvent::Output(b"still here".to_vec())).unwrap();
    }
    r.settle();
    assert!(r.rt.state.panes.iter().all(|p| p.term.as_ref().unwrap().visible_text().contains("still here")), "the sockets stayed open");
}

/// The PTY size follows the one tile that shows the terminal: moved, it
/// fits its tile in the page window (one size); the main window's frames
/// then send it none. Back, it fits the main window's tile again.
#[test]
fn the_pty_size_follows_the_window_that_shows_the_terminal() {
    let mut r = Rig::new(&["web-app", "docs"], 1);
    let main_size = r.rt.state.panes[0].term.as_ref().unwrap().size;
    for n in ["web-app", "docs"] {
        sizes(&r.inputs[n]);
    }
    r.move_out(0);
    let moved = sizes(&r.inputs["web-app"]);
    assert_eq!(moved.len(), 1, "one size, the window's tile: {moved:?}");
    let win_size = r.win(0, |pt| pt.state.page.as_ref().unwrap().panes[0].term.as_ref().unwrap().size);
    assert_ne!(win_size, main_size, "the window's tile is not the main window's");
    assert_eq!(moved[0], (win_size.0 as u16, win_size.1 as u16));
    r.main_frames(20);
    assert!(sizes(&r.inputs["web-app"]).is_empty(), "the main window sends a moved terminal no size");
    r.win(0, |pt| pt.state.back());
    r.settle();
    assert!(sizes(&r.inputs["web-app"]).is_empty(), "back on a page not shown: no size");
    r.main(|rt| rt.update(|s, _| s.run_intent("test", Intent::Pages(PageOp::Show(1)))));
    r.settle();
    let back = sizes(&r.inputs["web-app"]);
    assert_eq!(back.len(), 1, "shown in the main window: its tile there: {back:?}");
    assert_eq!(r.rt.state.panes.iter().find(|p| p.sandbox.as_deref() == Some("web-app")).unwrap().term.as_ref().unwrap().size, main_size);
}

/// Two page windows: each holds its own page, its keys reach its own
/// terminals only, and each goes back on its own.
#[test]
fn two_page_windows_each_hold_their_own_page() {
    let mut r = Rig::new(&["web-app", "api", "docs"], 1);
    let a = r.move_out(0);
    let b = r.move_out(0);
    assert_ne!(a, b);
    assert_eq!(r.main_names(), [vec!["docs"]]);
    assert_eq!((r.win_names(0), r.win_names(1)), (vec!["web-app".to_string()], vec!["api".to_string()]));
    r.type_win(1, "b");
    r.type_win(0, "a");
    assert_eq!((bytes(&r.inputs["web-app"]), bytes(&r.inputs["api"]), bytes(&r.inputs["docs"])), (b"a".to_vec(), b"b".to_vec(), vec![]));
    r.win(1, |pt| pt.state.back());
    r.settle();
    assert_eq!(r.main_names(), [vec!["docs"], vec!["api"]]);
    assert!(w::with(|h| h.is_open(Win::Page(a)) && !h.is_open(Win::Page(b))));
    // The palette's way back for the other one.
    r.main(|rt| rt.update(|s, _| s.run_cmd(palette::Run::PageBack(a))));
    r.settle();
    assert_eq!(r.main_names(), [vec!["docs"], vec!["api"], vec!["web-app"]]);
    assert!(w::with(|h| h.page_windows().is_empty()));
}

/// A moved pane's output wakes its window and only it, and is drawn there
/// once; the main window does not take it in. Back, the main window again.
#[test]
fn a_moved_panes_output_wakes_the_window_that_shows_it() {
    let mut r = Rig::new(&["web-app", "docs"], 1);
    let id = r.move_out(0);
    let page = Win::Page(id);
    // The route points at the window's waker (set at its first poll).
    let route = r.win(0, |pt| pt.state.page.as_ref().unwrap().panes[0].wake.clone().unwrap());
    let (m0, p0) = (r.wakes(Win::Main), r.wakes(page));
    route.wake()();
    assert_eq!((r.wakes(Win::Main) - m0, r.wakes(page) - p0), (0, 1), "the socket thread's wake reaches the page window only");
    // Its output: drawn by the window's poll (once), never by the main window's.
    r.events["web-app"].send(TermEvent::Output(b"hello window".to_vec())).unwrap();
    r.main_frames(3);
    let in_window = r.win(0, |pt| pt.state.page.as_ref().unwrap().panes[0].term.as_ref().unwrap().visible_text());
    assert!(!in_window.contains("hello window"), "the main window did not take it in");
    let mut ch = false;
    r.win(0, |pt| pt.update(|s, tx| ch = s.poll(1.0, tx)));
    assert!(ch, "the window draws it");
    let mut again = true;
    r.win(0, |pt| pt.update(|s, tx| again = s.poll(1.1, tx)));
    assert!(!again, "and only once");
    assert!(r.win(0, |pt| pt.state.page.as_ref().unwrap().panes[0].term.as_ref().unwrap().visible_text()).contains("hello window"));
    // The main window's own pane still wakes the main window.
    let main_route = r.rt.state.panes[0].wake.clone().unwrap();
    let (m1, p1) = (r.wakes(Win::Main), r.wakes(page));
    main_route.wake()();
    assert_eq!((r.wakes(Win::Main) - m1, r.wakes(page) - p1), (1, 0));
    // Back: the main window hears it.
    r.win(0, |pt| pt.state.back());
    r.settle();
    let (m2, p2) = (r.wakes(Win::Main), r.wakes(page));
    route.wake()();
    assert_eq!((r.wakes(Win::Main) - m2, r.wakes(page) - p2), (1, 0));
}

/// An idle page window costs no frames: nothing due, nothing to draw, and
/// a main window turn that changed nothing of it neither wakes nor dirties it.
#[test]
fn an_idle_page_window_costs_no_frames() {
    let mut r = Rig::new(&["web-app", "docs"], 1);
    let id = r.move_out(0);
    // Not the key window: no cursor blink.
    r.win(0, |pt| pt.window_focus(false));
    r.settle();
    // Frames until the window rests, with a bound: after it opens (and after
    // a resize) it takes a few frames to settle its bars (measured
    // 2026-11-26: about 11 at 1100 wide on main, up to 14 after a resize,
    // one chip more adds a couple), so a fixed count is a guess.
    let mut rested = false;
    for _ in 0..60 {
        if !r.win_frames(0, 1) {
            rested = true;
            break;
        }
    }
    assert!(rested, "the page window did not rest within 60 frames of opening");
    let drawn = r.win(0, |pt| pt.state.drawn);
    let wakes = r.wakes(Win::Page(id));
    assert!(!r.win_frames(0, 30), "no frame due and nothing to draw in 30 idle frames");
    r.main_frames(30);
    assert!(!r.win_frames(0, 5));
    assert_eq!(r.win(0, |pt| pt.state.drawn), drawn, "no poll found anything to draw");
    assert_eq!(r.wakes(Win::Page(id)), wakes, "idle main window turns never woke it");
    // Something it shows changes (a gate in the main window): woken, drawn once.
    r.main(|rt| rt.update(|s, _| s.palette_open()));
    assert!(r.wakes(Win::Page(id)) > wakes);
    assert!(r.win_frames(0, 1));
    // Its scrim comes in, then nothing more.
    r.win_frames(0, 40);
    assert!(!r.win_frames(0, 5));
}

/// A confirmation raised by what was done in the page window (here a file
/// dropped on its pane, through the drops dialog) is drawn there alone: the main
/// window says where it is and draws no copy of it.
#[test]
fn a_confirmation_raised_in_a_page_window_is_drawn_there_alone() {
    let mut r = Rig::new(&["web-app", "docs"], 1);
    let id = r.move_out(0);
    let files = dropped("drawn");
    assert!(r.run_from(id, move |a| a.pane_drop(0, &files)), "taken: {:?}", w::with(|h| h.gate().cloned()));
    r.settle();
    assert_eq!(w::with(|h| h.dialog_owner()), Some(Win::Page(id)), "it belongs to the page window: {:?}", r.rt.state.notice);
    assert!(r.drawn_win(0, "cf-dialog") && r.drawn_win(0, "cf-confirm"));
    assert!(!r.drawn_main("cf-dialog"), "no copy in the main window");
    assert!(r.drawn_main("cwin-elsewhere"), "the main window says where it is");
    assert!(r.app(|a| a.feats.dialog.as_ref().and_then(|d| d.confirm.as_ref()).is_some_and(|c| c.title.contains("into web-app"))), "for the window's pane");
    // Its button brings that window to the front.
    w::take_done();
    r.tap_main("cwin-elsewhere-show");
    assert_eq!(w::take_done(), vec![Native::Focus(Win::Page(id))]);
}

/// No key or press in the main window answers the page window's
/// confirmation, and no answer forged from another window is taken; its
/// own Cancel answers it, and sends nothing.
#[test]
fn only_the_page_window_answers_its_confirmation() {
    let mut r = Rig::new(&["web-app", "docs"], 1);
    let id = r.move_out(0);
    let files = dropped("answer");
    r.run_from(id, move |a| a.pane_drop(0, &files));
    r.settle();
    let ident = r.app(|a| a.feats.dialog.as_ref().map(Dialog::identity)).expect("up");
    for k in [Key::Escape, Key::Enter, Key::Text(" ".into()), Key::Tab] {
        r.main(|rt| {
            rt.key(KeyEvent { key: k.clone(), mods: Mods::default() });
            rt.frame(0.03);
        });
        r.settle();
    }
    r.tap_main("cwin-elsewhere-scrim");
    assert_eq!(r.app(|a| a.feats.dialog.as_ref().map(Dialog::identity)).as_deref(), Some(ident.as_str()), "the main window's keys and presses answered nothing");
    assert!(!w::with(|h| h.post(Win::Main, ToPrimary::Answer { confirm: true, dialog: ident.clone() })), "not from the main window");
    assert!(!w::with(|h| h.post(Win::Canvas, ToPrimary::Answer { confirm: true, dialog: ident.clone() })), "not from another window");
    r.tap_win(0, "cf-cancel");
    r.settle();
    assert!(r.app(|a| a.feats.dialog.is_none()), "cancelled in its own window");
    assert!(!r.drawn_win(0, "cf-dialog") && !r.drawn_main("cwin-elsewhere"));
}

/// A gate of the main window (here the palette) holds every page window:
/// its scrim says where, a press on its bar or a key into its terminal
/// reaches nothing, a drop is refused, and no input it posts is taken.
#[test]
fn a_gate_in_the_main_window_holds_every_page_window() {
    let mut r = Rig::new(&["web-app", "docs"], 1);
    let id = r.move_out(0);
    r.main(|rt| rt.update(|s, _| s.palette_open()));
    r.settle();
    assert!(r.drawn_win(0, "cwin-scrim") && r.drawn_win(0, "cwin-gate-show"));
    assert!(r.win(0, |pt| pt.state.shown_gate().is_some_and(|g| g.owner == Win::Main)));
    bytes(&r.inputs["web-app"]);
    r.type_win(0, "z");
    assert!(bytes(&r.inputs["web-app"]).is_empty(), "no key reached the terminal under the scrim");
    let refused = w::with(|h| h.refused);
    assert!(!r.run_from(id, |a| a.remove_pane(0)), "its input is refused");
    assert!(w::with(|h| h.refused) > refused);
    r.settle();
    assert_eq!(r.win_names(0), ["web-app"], "nothing ran");
    // Its button shows the main window.
    w::take_done();
    r.tap_win(0, "cwin-gate-show");
    assert_eq!(w::take_done(), vec![Native::FocusPrimary]);
    // Gone: the window takes keys again.
    r.main(|rt| rt.update(|s, _| s.palette_close()));
    r.settle();
    r.type_win(0, "z");
    assert_eq!(bytes(&r.inputs["web-app"]), b"z");
}

/// While the main window holds the keyboard for Jarvis's proposal
/// (`held_focus`), every page window refuses terminal keys through its
/// keyboard gate, its grid's lens included.
#[test]
fn a_page_window_refuses_terminal_keys_while_the_main_window_holds_the_keyboard() {
    let mut r = Rig::new(&["web-app", "docs"], 1);
    r.move_out(0);
    // The main window's turn ends holding the keyboard (its poll would give
    // it back at once with no proposal up: only the hold is under test).
    r.rt.update(|s, _| {
        w::primary_enter(s);
        s.held_focus = Some(Some("grid-1".into()));
        w::primary_leave(s);
    });
    r.win_frames(0, 3);
    assert!(r.win(0, |pt| !pt.state.terminal_may_take_keys(Some("grid-1")) && pt.state.covered()));
    bytes(&r.inputs["web-app"]);
    r.type_win(0, "q");
    r.win(0, |pt| {
        // The input method's text: refused by the same gate.
        let ime = rune::Ime::Commit("é".into());
        let took = pt.state.on_ime(&ime, Some("grid-1"));
        assert!(!took);
    });
    assert!(bytes(&r.inputs["web-app"]).is_empty(), "no key reached the terminal");
    r.rt.update(|s, _| {
        w::primary_enter(s);
        s.held_focus = None;
        w::primary_leave(s);
    });
    r.win_frames(0, 3);
    r.type_win(0, "q");
    assert_eq!(bytes(&r.inputs["web-app"]), b"q");
}

/// Closing the page window cancels its own confirmation, never confirms it.
#[test]
fn closing_a_page_window_cancels_its_confirmation() {
    let mut r = Rig::new(&["web-app", "docs"], 1);
    let id = r.move_out(0);
    let files = dropped("answer");
    r.run_from(id, move |a| a.pane_drop(0, &files));
    r.settle();
    assert!(r.app(|a| a.feats.dialog.is_some()));
    r.rt.state.notice = None;
    r.win(0, |pt| pt.update(|s, tx| pw::page_closed(s, tx)));
    let queued = w::with(|h| h.take_for_primary());
    assert!(queued.iter().any(|(f, m)| *f == Win::Page(id) && matches!(m, ToPrimary::Answer { confirm: false, .. })), "{queued:?}");
    assert!(!queued.iter().any(|(_, m)| matches!(m, ToPrimary::Answer { confirm: true, .. })));
    // Run as the primary would.
    for (f, m) in queued {
        w::with(|h| h.post(f, m));
    }
    r.settle();
    assert!(r.app(|a| a.feats.dialog.is_none()), "cancelled");
    assert!(r.rt.state.notice.as_ref().is_none_or(|n| !n.0.contains("/tmp/sbxw-uploads")), "nothing uploaded");
    assert_eq!(r.main_names(), [vec!["docs"], vec!["web-app"]]);
}

/// A long copy asked by a page window's program is offered in that window:
/// the main window shows where; Copy there copies it.
#[test]
fn a_page_windows_copy_offer_is_answered_there() {
    let mut r = Rig::new(&["web-app", "docs"], 1);
    let id = r.move_out(0);
    let text = "x".repeat(approval::OSC_COPY_QUIET_MAX + 10);
    let osc = format!("\x1b]52;c;{}\x07", b64(text.as_bytes()));
    r.events["web-app"].send(TermEvent::Output(osc.into_bytes())).unwrap();
    r.settle();
    assert_eq!(w::with(|h| h.copy_owner()), Some(Win::Page(id)));
    assert!(r.drawn_win(0, "copy-offer-copy"));
    assert!(!r.drawn_main("copy-offer") && r.drawn_main("cwin-elsewhere"));
    // The main window's keys do not answer it.
    r.main(|rt| {
        rt.key(KeyEvent { key: Key::Escape, mods: Mods::default() });
    });
    assert!(r.rt.state.copy_offer.is_some());
    assert!(!w::with(|h| h.post(Win::Main, ToPrimary::CopyAnswer { copy: true })));
    r.tap_win(0, "copy-offer-copy");
    r.settle();
    assert!(r.rt.state.copy_offer.is_none());
    assert!(r.rt.state.notice.as_ref().is_some_and(|n| n.0.starts_with("Copied")), "{:?}", r.rt.state.notice);
}

/// Dictation stays the main window's: a page window's microphone is drawn
/// disabled, saying so, and a press on it (or the action forged) starts nothing.
#[test]
fn a_page_windows_microphone_is_disabled_and_dictates_nothing() {
    let mut r = Rig::new(&["web-app", "docs"], 1);
    let id = r.move_out(0);
    let state = r.main(|rt| {
        let mut st = None;
        rt.update(|s, _| st = s.in_window(id, |a| a.pane_dictate_state(0)));
        st.unwrap()
    });
    assert_eq!(state, PaneDictate::Off("Dictation works in the main window.".into()));
    if r.drawn_win(0, "pane-1-dictate") {
        r.tap_win(0, "pane-1-dictate");
    }
    r.run_from(id, |a| a.dictate_pane_press(0));
    r.settle();
    assert!(r.rt.state.dictation.on.is_none() && r.rt.state.dictation.pane_ask.is_none(), "no dictation started");
    // The main window's own pane can still dictate (its gate as before).
    assert_ne!(r.rt.state.pane_dictate_state(0), PaneDictate::Off("Dictation works in the main window.".into()));
}

/// Quitting with a page window open: the state file's layout already holds
/// its page (after the main window's), so nothing is lost at the next
/// start; nothing is confirmed or sent, and dropping everything is clean.
#[test]
fn quitting_with_a_page_window_open_is_clean() {
    let mut r = Rig::new(&["web-app", "api", "docs"], 2);
    r.move_out(0);
    let files = dropped("quit");
    let id = w::with(|h| h.page_windows()[0]);
    r.run_from(id, move |a| a.pane_drop(0, &files));
    r.settle();
    let saved = r.rt.state.store.get("layout");
    assert_eq!(saved["panes"].as_array().map(Vec::len), Some(3), "{saved}");
    assert_eq!(saved["pages"].as_array().map(Vec::len), Some(2), "the page window's page is saved as a page: {saved}");
    // Rune's quit runs no answer: the confirmation is never confirmed.
    let mut quit = false;
    r.main(|rt| rt.update(|_, tx| {
        tx.quit();
        quit = true;
    }));
    assert!(quit);
    let sent: Vec<u8> = r.inputs.values().flat_map(bytes).collect();
    assert!(sent.is_empty(), "nothing typed on the way out");
    drop(r);
    w::reset();
}

/// The way out: the pager's menu, the palette and the chord, and none of
/// them in Leads only or for a page with nothing open.
#[test]
fn the_pager_menu_the_palette_and_the_chord_move_the_page_out() {
    let mut r = Rig::new(&["web-app", "docs"], 1);
    r.main(|rt| rt.update(|s, _| s.pager_menu = Some("per".into())));
    r.tap_main("pager-to-window");
    assert_eq!(r.app(|a| a.windowed.len()), 1, "from the pager's menu");
    let cmds = r.main(|rt| rt.state.palette_cmds());
    assert!(cmds.iter().any(|c| c.id == "page-to-window" && c.label == "Move Page to New Window"));
    assert!(cmds.iter().any(|c| c.id.ends_with("-back") && c.label == "Move Page Back to Main Window"));
    // The chord, in the main window: the other page.
    let chord = if cfg!(target_os = "macos") { Mods { logo: true, cmd: true, shift: true, ..Default::default() } } else { Mods { ctrl: true, shift: true, ..Default::default() } };
    let took = r.main(|rt| {
        let mut took = false;
        // As main.rs hands the window's keys over (its `on_key`).
        rt.update(|s, tx| took = s.on_key(&KeyEvent { key: Key::Command('M'), mods: chord }, tx));
        took
    });
    assert!(took);
    r.settle();
    assert_eq!(r.app(|a| a.windowed.len()), 2, "from the chord");
    assert_eq!(r.main_names(), [vec![String::new()]]);
    // Nothing open on the page: refused.
    assert_eq!(r.main(|rt| {
        let mut got = Some(0);
        rt.update(|s, _| got = s.move_page_out(0));
        got
    }), None);
}

/// The pager's button after the tabs: labelled, the chord in its tooltip,
/// the menus' own press (the page moves to a window); disabled with the
/// menu row's reason where that row is (a page with no terminal).
#[test]
fn the_pagers_button_moves_the_page_out_where_the_menu_would() {
    let mut r = Rig::new(&["web-app", "docs"], 1);
    let el = |r: &mut Rig| r.main(|rt| {
        rt.frame(0.03);
        rt.elements().into_iter().find(|e| e.key.as_deref() == Some("pager-to-window-now") && !e.exiting).expect("the button is drawn")
    });
    let e = el(&mut r);
    assert_eq!((e.role, e.label.as_deref()), ("button", Some("Move page to new window")));
    assert!(e.path.contains("pager"), "in the pager's row: {}", e.path);
    assert!(!e.inert && e.opacity > 0.99);
    assert!(r.app(|a| a.pager_drawn("pager-to-window-now")));
    let why = r.app(|a| a.page_movable());
    let (help, keys) = super::pager::to_window_help(why.as_deref());
    assert_eq!(help, "Move this page to a new window");
    assert_eq!(keys, super::pagewin::back_keys());
    assert_eq!(keys.concat(), if cfg!(target_os = "macos") { "⌘⇧M" } else { "CtrlShiftM" });
    r.tap_main("pager-to-window-now");
    assert_eq!(r.app(|a| a.windowed.len()), 1, "the page moved to a window");
    assert_eq!(r.main_names(), [vec!["docs".to_string()]]);
    // A new empty page: the menu row is disabled, so is the button.
    r.main(|rt| rt.update(|s, _| {
        s.run_intent("test", Intent::Pages(PageOp::New));
        s.push_data();
    }));
    r.settle();
    let why = r.app(|a| a.page_movable());
    assert_eq!(why.as_deref(), Some("This page shows no terminal"));
    assert_eq!(super::pager::to_window_help(why.as_deref()), ("This page shows no terminal".to_string(), vec![]));
    let e = el(&mut r);
    assert!(e.opacity < 0.99, "the disabled recipe: {}", e.opacity);
    let (x, y) = r.main(|rt| rt.find("pager-to-window-now").unwrap());
    r.main(|rt| {
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(0.03);
    });
    r.settle();
    assert_eq!(r.app(|a| a.windowed.len()), 1, "a disabled button moves nothing");
}

/// A session closed elsewhere (its thread ended) closes its pane in the page
/// window too, the window's page re-tiling; a stopped sandbox's terminals
/// there close as the main window's do.
#[test]
fn a_session_closed_elsewhere_closes_its_pane_in_a_page_window() {
    let mut r = Rig::new(&["web-app", "api", "docs"], 2);
    r.move_out(0);
    r.main(|rt| rt.update(|s, _| s.close_panes("api", None)));
    r.settle();
    assert_eq!(r.win_names(0), ["web-app"], "its pane went, the page re-tiled");
    r.main(|rt| rt.update(|s, _| s.run_intent("test", Intent::StopSandbox { sandbox: "web-app".into() })));
    r.settle();
    assert!(r.win(0, |pt| pt.state.page.as_ref().unwrap().panes[0].term.is_none()), "its terminal closed");
}

/// One view per terminal: the sidebar's open (its row's `open_terminal`,
/// through the screens' intents path) for a session shown in a page window
/// focuses that window and pane, and adds no pane in the main window; so
/// does the palette's open into a spare pane, and a start. Back in the main
/// window, the same open runs there as before.
#[test]
fn opening_a_session_shown_in_a_page_window_focuses_it_there() {
    let mut r = Rig::new(&["web-app", "api", "docs"], 2);
    let id = r.move_out(0);
    // The window's focus is on api; the main window has docs alone.
    r.win(0, |pt| pt.state.page.as_mut().unwrap().focused = 1);
    r.settle();
    let main_panes = r.rt.state.panes.len();
    w::take_done();
    let open = |sb: &str, extra: serde_json::Value| {
        let mut args = serde_json::json!({ "sandbox": sb });
        if let (Some(a), Some(e)) = (args.as_object_mut(), extra.as_object()) {
            a.extend(e.clone());
        }
        crate::intent::parse(&serde_json::json!({ "kind": "intent", "name": "open_terminal", "args": args })).unwrap()
    };
    let list = open("web-app", serde_json::json!({}));
    r.main(|rt| rt.update(|s, _| s.intents("sandboxes", list)));
    r.settle();
    assert_eq!(r.rt.state.panes.len(), main_panes, "no pane added in the main window");
    assert_eq!(r.main_names(), [vec!["docs"]]);
    assert!(w::take_done().contains(&Native::Focus(Win::Page(id))), "its window to the front");
    assert_eq!(r.win(0, |pt| pt.state.page.as_ref().unwrap().focused), 0, "and its pane focused");
    // The palette's open into a spare pane, and a start: the same.
    r.main(|rt| rt.update(|s, _| s.run_cmd(palette::Run::Intents(vec![("open_terminal".into(), serde_json::json!({ "sandbox": "api", "pane": "spare" }))]))));
    r.settle();
    assert_eq!((r.rt.state.panes.len(), r.win(0, |pt| pt.state.page.as_ref().unwrap().focused)), (main_panes, 1));
    r.main(|rt| rt.update(|s, _| s.run_intent("test", Intent::StartSandbox { sandbox: "web-app".into(), pane: PaneTarget::Spare })));
    r.settle();
    assert_eq!((r.rt.state.panes.len(), r.win(0, |pt| pt.state.page.as_ref().unwrap().focused)), (main_panes, 0));
    assert_eq!(r.generations().len(), 3, "no terminal opened");
    // A bash of web-app is another session: it opens in the main window.
    let list = open("web-app", serde_json::json!({ "mode": "bash" }));
    r.main(|rt| rt.update(|s, _| s.intents("sandboxes", list)));
    r.settle();
    assert!(r.rt.state.panes.iter().any(|p| p.sandbox.as_deref() == Some("web-app") && p.mode == Mode::Bash));
    // Back: the open runs in the main window as before.
    r.win(0, |pt| pt.state.back());
    r.settle();
    w::take_done();
    let list = open("api", serde_json::json!({}));
    r.main(|rt| rt.update(|s, _| s.intents("sandboxes", list)));
    r.settle();
    assert!(!w::take_done().iter().any(|n| matches!(n, Native::Focus(Win::Page(_)))));
    let f = r.rt.state.focused;
    assert_eq!(r.rt.state.panes[f].sandbox.as_deref(), Some("api"), "opened in the main window's focused pane");
}

/// The reverse: a page window cannot get a second view of a session shown
/// in the main window (its pane's mode switch, which opens the other
/// session of the sandbox): the main window's pane is focused instead.
#[test]
fn a_page_window_never_opens_a_second_view_of_a_main_window_session() {
    let mut r = Rig::new(&["web-app", "docs"], 1);
    // The main window's docs pane shows web-app's shell.
    r.rt.update(|s, _| {
        let i = s.panes.iter().position(|p| p.sandbox.as_deref() == Some("docs")).unwrap();
        s.panes[i].sandbox = Some("web-app".into());
        s.panes[i].mode = Mode::Bash;
    });
    let id = r.move_out(0);
    w::take_done();
    // From the window's bar: switch its web-app agent pane to the shell.
    r.run_from(id, |a| a.switch_mode(0));
    r.settle();
    assert!(r.win(0, |pt| pt.state.page.as_ref().unwrap().panes[0].mode == Mode::Agent), "the window kept its view");
    let gen = |p: &Pane| p.term.as_ref().map(|t| t.fit_key().rsplit_once("-g").unwrap().1.to_string());
    let mut gens: Vec<String> = r.rt.state.panes.iter().filter_map(gen).collect();
    gens.extend(r.win(0, |pt| pt.state.page.as_ref().unwrap().panes.iter().filter_map(gen).collect::<Vec<_>>()));
    assert_eq!(gens.len(), 2, "no terminal opened: {gens:?}");
    assert!(w::take_done().contains(&Native::FocusPrimary), "the main window to the front");
    let f = r.rt.state.focused;
    assert_eq!((r.rt.state.panes[f].sandbox.as_deref(), r.rt.state.panes[f].mode), (Some("web-app"), Mode::Bash));
    assert!(r.rt.state.notice.as_ref().is_some_and(|n| n.0.contains("shows in the main window")), "{:?}", r.rt.state.notice);
}

/// The window buttons (macOS) sit on the page window's header line, at its
/// padding; the header keeps 12 pt clear after them, and no room without them.
#[test]
fn the_page_windows_header_keeps_room_for_the_window_buttons() {
    let c = pw::page_chrome();
    assert!(c.full_size_content && c.drag_regions);
    let (x, y) = c.traffic_lights.unwrap();
    assert_eq!(x, layout::GUTTER);
    assert_eq!(y + rune::Chrome::TRAFFIC_LIGHT / 2.0, layout::GUTTER + layout::CONTROL_H / 2.0, "centred on the header");
    assert_eq!(pw::lights_room(0.0), 0.0);
    assert_eq!(pw::lights_room(76.0), 76.0 - layout::GUTTER + 12.0);
}

/// A terminal colour picked in the main window reaches a page window: its
/// window is woken, its terminals take the colours and its bars are drawn
/// again on the picked bar (term_colors.rs).
#[test]
fn a_page_window_follows_the_terminal_colours() {
    use crate::terminal::theme::Slot;
    let mut r = Rig::new(&["web-app", "api", "docs"], 2);
    let id = r.move_out(0);
    let bg = |r: &mut Rig| r.win(0, |pt| pt.state.page.as_ref().unwrap().panes.iter().map(|p| p.term.as_ref().unwrap().theme().background).collect::<Vec<_>>());
    let bar = |r: &mut Rig| {
        r.win(0, |pt| {
            pt.frame(0.03);
            pt.elements().into_iter().find(|e| !e.exiting && e.key.as_deref() == Some("pane-1-bar")).map(|e| e.fill)
        })
    };
    assert_eq!(bg(&mut r), [0x0d1117; 2]);
    let bar0 = bar(&mut r);
    let woken = r.wakes(Win::Page(id));
    r.main(|rt| {
        rt.update(|s, _| {
            s.set_term_color(Slot::Background, Slot::Background.parse("#fdf6e3"));
            s.set_term_color(Slot::Bar, Slot::Bar.parse("#4a2a2a"));
        })
    });
    assert!(r.wakes(Win::Page(id)) > woken, "the page window is woken");
    r.settle();
    assert_eq!(bg(&mut r), [0xfdf6e3; 2], "its terminals follow (Paper)");
    assert_ne!(bar(&mut r), bar0, "its bars too");
    let cursor = r.win(0, |pt| pt.state.page.as_ref().unwrap().panes[0].term.as_ref().unwrap().theme().cursor);
    assert_eq!(cursor, 0x0969da, "the light rules");
}
