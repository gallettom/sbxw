//! The pages, the pager and moving panes, in the app and in Rune's runtime:
//! nothing a move does reopens a terminal or sends more than one size.

use super::*;
use crate::client::{TermEvent, TermInput, TerminalConn};
use std::sync::mpsc;

/// The app in Rune's runtime with live terminals on one page, one per name,
/// whose input we read. Each terminal's event sender is kept alive (its
/// socket stays open).
pub(super) struct Rig {
    pub rt: rune::Runtime<App>,
    pub inputs: Vec<(Uid, mpsc::Receiver<TermInput>)>,
    _events: Vec<mpsc::Sender<TermEvent>>,
    pub clock: f64,
}

impl Rig {
    pub fn new(names: &[&str], w: f32, h: f32) -> Rig {
        let fixture = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/shell/two-panes.json");
        let mut a = App::new(Args { fixture: Some(fixture.into()), ..Args::default() });
        let opt: Vec<Option<&str>> = names.iter().map(|n| Some(*n)).collect();
        super::tests::set_panes(&mut a, &opt);
        let (mut inputs, mut events) = (vec![], vec![]);
        for i in 0..names.len() {
            let (ev_tx, ev_rx) = mpsc::channel();
            let (in_tx, in_rx) = mpsc::channel();
            let mut t = TerminalState::new(TerminalConn { events: ev_rx, input: in_tx });
            t.id = (i + 1).to_string();
            a.panes[i].term = Some(t);
            inputs.push((a.panes[i].uid, in_rx));
            events.push(ev_tx);
        }
        let mut rt = rune::Runtime::new(a, view, (w, h), rune::Env::new(true, 1.0));
        rt.on_key = Some(|s: &mut App, ev: &KeyEvent, tx: &mut Tx| s.on_key(ev, tx));
        let mut r = Rig { rt, inputs, _events: events, clock: 0.0 };
        r.settle();
        r.take();
        r
    }

    /// Frames for about a second of the app's clock (resizes are debounced).
    pub fn settle(&mut self) {
        for _ in 0..40 {
            self.clock += 0.03;
            self.rt.frame(0.03);
        }
    }

    /// The sizes each terminal sent since the last call, by pane id.
    pub fn take(&mut self) -> HashMap<Uid, Vec<(u16, u16)>> {
        let mut out = HashMap::new();
        for (u, rx) in &self.inputs {
            let v: Vec<(u16, u16)> = rx.try_iter().filter_map(|i| if let TermInput::Resize { cols, rows } = i { Some((cols, rows)) } else { None }).collect();
            out.insert(*u, v);
        }
        out
    }

    /// Each terminal's identity (its generation, which a new terminal changes).
    pub fn generations(&self) -> HashMap<Uid, String> {
        self.rt.state.panes.iter().filter_map(|p| p.term.as_ref().map(|t| (p.uid, t.fit_key().rsplit_once("-g").unwrap().1.to_string()))).collect()
    }

    pub fn names(&self) -> Vec<Vec<String>> {
        let s = &self.rt.state;
        s.pages.list.iter().map(|p| p.tile.order().iter().map(|u| s.pane_by_uid(*u).and_then(|p| p.sandbox.clone()).unwrap_or_default()).collect()).collect()
    }

    pub fn op(&mut self, op: PageOp) {
        self.rt.update(|s, _| {
            s.run_intent("test", Intent::Pages(op));
            s.push_data();
        });
        self.settle();
    }

    /// After a rearrangement: every terminal is the one it was (same
    /// generation, input still open), and each sent at most one size.
    pub fn check_kept(&mut self, before: &HashMap<Uid, String>, what: &str) {
        assert_eq!(&self.generations(), before, "{what}: a terminal was recreated");
        for (u, sizes) in self.take() {
            assert!(sizes.len() <= 1, "{what}: pane {u} sent {} sizes: {sizes:?}", sizes.len());
        }
        for p in &self.rt.state.panes {
            if let Some(t) = &p.term {
                assert!(!t.status.starts_with("closed"), "{what}: {} closed", p.uid);
            }
        }
    }
}

#[test]
fn moving_panes_and_pages_never_recreates_or_reconnects_a_terminal() {
    let mut r = Rig::new(&["a", "b", "c", "d"], 1284.0, 816.0);
    let before = r.generations();
    assert_eq!(before.len(), 4);
    let steps: Vec<(&str, PageOp)> = vec![
        ("tabs", PageOp::View(true)),
        ("page 3", PageOp::Show(2)),
        ("next page", PageOp::Step(1)),
        ("grid", PageOp::View(false)),
        ("2 per page", PageOp::Per(2)),
        ("to page 1", PageOp::ToPage { pane: Some(2), page: Some(0) }),
        ("gather", PageOp::Gather),
        ("move right", PageOp::Move { pane: Some(0), to: "right".into() }),
        ("rotate", PageOp::Rotate(Some(0))),
        ("dock down", PageOp::Move { pane: Some(0), to: "down".into() }),
        ("to a new page", PageOp::ToPage { pane: Some(0), page: None }),
        ("equalise", PageOp::Equalise),
        ("close a page", PageOp::Close(Some(1))),
    ];
    for (what, op) in steps {
        r.op(op);
        let keep: HashMap<Uid, String> = before.iter().filter(|(u, _)| r.rt.state.index_of(**u).is_some()).map(|(u, g)| (*u, g.clone())).collect();
        r.check_kept(&keep, what);
    }
    // The page closed took its panes' views (their terminals' sockets); the
    // others are the same four minus those.
    assert!(r.rt.state.panes.iter().filter(|p| p.term.is_some()).count() < 4);
}

#[test]
fn a_hidden_page_keeps_its_terminal_running_and_sends_no_size() {
    let mut r = Rig::new(&["a", "b"], 1284.0, 816.0);
    r.op(PageOp::Per(1));
    assert_eq!(r.names(), [vec!["a"], vec!["b"]]);
    let sizes = r.take();
    assert!(sizes.values().all(|v| v.len() <= 1), "{sizes:?}");
    // Page 2 shows, page 1 hidden: its terminal still takes output.
    r.op(PageOp::Show(1));
    let a = r.rt.state.panes[0].uid;
    r.rt.update(|s, _| s.panes[0].term.as_mut().unwrap().feed(b"still here"));
    r.settle();
    assert!(r.rt.state.pane_by_uid(a).unwrap().term.as_ref().unwrap().visible_text().contains("still here"));
    assert!(r.take().values().all(|v| v.len() <= 1), "b fits once to its page");
    // Back and forth: the same sizes as before, nothing sent.
    r.op(PageOp::Show(0));
    r.op(PageOp::Show(1));
    assert!(r.take().values().all(|v| v.is_empty()));
}

#[test]
fn ctrl_tab_turns_pages_even_in_a_terminal_and_sends_it_nothing() {
    let mut r = Rig::new(&["a", "b", "c"], 1284.0, 816.0);
    r.op(PageOp::Per(1));
    r.op(PageOp::Show(0));
    // The terminal's grid has the keyboard.
    let k = r.rt.state.panes[0].term.as_ref().unwrap().key("grid");
    r.rt.update(|_, tx| tx.focus(k.clone(), false));
    r.settle();
    let typed = |r: &mut Rig| -> Vec<u8> {
        r.inputs.iter().flat_map(|(_, rx)| rx.try_iter().filter_map(|i| if let TermInput::Bytes(b) = i { Some(b) } else { None }).flatten().collect::<Vec<u8>>()).collect()
    };
    let key = |r: &mut Rig, shift: bool| {
        r.rt.key(KeyEvent { key: Key::Tab, mods: rune::Mods { ctrl: true, shift, ..Default::default() } });
        r.settle();
    };
    key(&mut r, false);
    assert_eq!(r.rt.state.pages.current, 1);
    key(&mut r, false);
    assert_eq!(r.rt.state.pages.current, 2);
    key(&mut r, false);
    assert_eq!(r.rt.state.pages.current, 0, "round");
    key(&mut r, true);
    assert_eq!(r.rt.state.pages.current, 2, "back");
    assert_eq!(typed(&mut r), b"", "no Tab reached a terminal");
    // A plain Tab in the terminal is still the program's.
    let k = r.rt.state.panes[2].term.as_ref().unwrap().key("grid");
    r.rt.update(|_, tx| tx.focus(k.clone(), false));
    r.settle();
    r.rt.key(KeyEvent { key: Key::Tab, mods: rune::Mods::default() });
    r.settle();
    assert_eq!(typed(&mut r), b"\t");
    // Cmd/Ctrl+1 is the current page's first pane.
    assert_eq!(r.rt.state.focused, 2);
}

#[test]
fn the_pager_shows_pages_and_fits_from_720_to_1800() {
    for w in [720.0, 900.0, 1284.0, 1800.0] {
        let names: Vec<String> = (0..9).map(|i| format!("sandbox-number-{i}")).collect();
        let refs: Vec<&str> = names.iter().map(String::as_str).collect();
        let mut r = Rig::new(&refs[..4], w, 480.0f32.max(w * 0.6));
        r.rt.update(|s, _| {
            // Five more on further pages (no terminals: their names suffice).
            for n in &names[4..] {
                let id = s.fresh();
                let mut p = Pane::empty(id);
                p.sandbox = Some(n.clone());
                s.panes.push(p);
                let e: std::collections::HashSet<Uid> = std::collections::HashSet::new();
                s.pages.place(id, &|x| e.contains(&x));
            }
            s.sync_panes(None);
            s.run_intent("test", Intent::Pages(PageOp::Per(1)));
            s.run_intent("test", Intent::Pages(PageOp::Show(5)));
            s.push_data();
        });
        r.settle();
        let s = &r.rt.state;
        assert_eq!(s.pages.list.len(), 9);
        let (pr, _) = r.rt.presented("pager").unwrap();
        let (a, b) = s.pager_fit();
        assert!((a..b).contains(&5), "{w}: the current page's tab is shown ({a}..{b})");
        for k in a..b {
            let (t, _) = r.rt.presented(&format!("page-{}", k + 1)).unwrap_or_else(|| panic!("{w}: tab {} not drawn", k + 1));
            assert!(t[0] >= pr[0] - 0.5 && t[0] + t[2] <= pr[0] + pr[2] + 0.5, "{w}: tab {} {t:?} out of the pager {pr:?}", k + 1);
        }
        for key in ["page-new", "pager-view-tabs", "pager-view-grid", "pager-per"] {
            let (t, _) = r.rt.presented(key).unwrap_or_else(|| panic!("{w}: {key}"));
            assert!(t[0] + t[2] <= pr[0] + pr[2] + 0.5 && t[0] >= pr[0] - 0.5, "{w}: {key} {t:?} out of {pr:?}");
        }
        if b - a < 9 {
            assert!(r.rt.presented("pager-more").is_some(), "{w}: the hidden tabs have their menu");
            assert!(s.drawn("pager-more"));
        }
        eprintln!("pager at {w}: tabs {a}..{b} of 9");
    }
}

#[test]
fn the_pager_menu_sets_the_count_and_tabs_show_pages() {
    let mut r = Rig::new(&["a", "b", "c", "d"], 1284.0, 816.0);
    super::tests::tap(&mut r.rt, "pager-per");
    assert!(r.rt.state.drawn("pager-per-list"));
    super::tests::tap(&mut r.rt, "pager-per-2");
    r.settle();
    assert_eq!(r.names(), [vec!["a", "b"], vec!["c", "d"]]);
    super::tests::tap(&mut r.rt, "page-2");
    assert_eq!(r.rt.state.pages.current, 1);
    super::tests::tap(&mut r.rt, "pager-view-tabs");
    r.settle();
    assert_eq!(r.rt.state.pages.list.len(), 4);
    assert_eq!(r.rt.state.panes[r.rt.state.focused].sandbox.as_deref(), Some("c"), "the focused terminal's page shows");
    super::tests::tap(&mut r.rt, "pager-view-grid");
    r.settle();
    assert_eq!(r.rt.state.pages.per, 2, "Grid goes back to the last count above one");
    // Raising the count moves nothing; Gather does.
    super::tests::tap(&mut r.rt, "pager-per");
    super::tests::tap(&mut r.rt, "pager-per-4");
    assert_eq!(r.rt.state.pages.list.len(), 4);
    super::tests::tap(&mut r.rt, "pager-per");
    super::tests::tap(&mut r.rt, "pager-gather");
    r.settle();
    assert_eq!(r.names(), [vec!["a", "b", "c", "d"]]);
    // "+": a new empty page, shown; closing it goes back.
    super::tests::tap(&mut r.rt, "page-new");
    assert_eq!((r.rt.state.pages.list.len(), r.rt.state.pages.current), (2, 1));
    assert!(r.rt.presented("pane-5-empty").is_some(), "the new page's empty pane");
    r.settle();
    super::tests::tap(&mut r.rt, "page-2-close");
    assert_eq!(r.rt.state.pages.list.len(), 1);
    // Closing a page closes its views, never the sessions (no stop is asked).
    super::tests::tap(&mut r.rt, "page-1-close");
    assert!(r.rt.state.panes.iter().all(|p| p.sandbox.is_none()));
    assert!(r.rt.state.sandboxes.iter().all(|s| !s.status.starts_with("stopping")));
}

#[test]
fn pages_persist_and_the_old_layout_migrates() {
    // The old fixed layout (a fixture's panes/split/open): one page, the same
    // panes in the same arrangement.
    let a = App::new(Args { fixture: Some(format!("{}/fixtures/shell/design-4.json", env!("CARGO_MANIFEST_DIR"))), ..Args::default() });
    assert_eq!(a.pages.list.len(), 1);
    let ids = a.pages.cur().tile.order();
    assert_eq!(a.pages.cur().tile, tiling::default(&ids, false), "the old 4-pane grid");
    // Saved, then brought back: a stopped sandbox's pane is left out and its
    // page closes the gap; nothing is started for it.
    let mut a = App::new(Args { fixture: Some(format!("{}/fixtures/shell/two-panes.json", env!("CARGO_MANIFEST_DIR"))), ..Args::default() });
    let running: Vec<String> = a.sandboxes.iter().filter(|s| s.status == "running").map(|s| s.name.clone()).collect();
    let stopped: Vec<String> = a.sandboxes.iter().filter(|s| s.status == "stopped").map(|s| s.name.clone()).collect();
    assert!(running.len() >= 2 && !stopped.is_empty(), "the fixture has both: {running:?} {stopped:?}");
    let names: Vec<Option<&str>> = vec![Some(&running[0]), Some(&stopped[0]), Some(&running[1])];
    super::tests::set_panes(&mut a, &names);
    a.run_intent("test", Intent::Pages(PageOp::Per(2)));
    a.run_intent("test", Intent::Pages(PageOp::Rotate(Some(0))));
    let saved = a.store.get("layout").clone();
    assert_eq!(saved["per"], 2);
    assert_eq!(saved["pages"].as_array().unwrap().len(), 2);
    assert_eq!(saved["panes"][1]["sandbox"], stopped[0].as_str());
    let mut b = App::new(Args { fixture: Some(format!("{}/fixtures/shell/two-panes.json", env!("CARGO_MANIFEST_DIR"))), ..Args::default() });
    b.panes.iter_mut().for_each(|p| p.clear());
    b.restore = Some(saved);
    b.restore_layout();
    let got: Vec<Vec<Option<String>>> = b.pages.list.iter().map(|p| p.tile.order().iter().map(|u| b.pane_by_uid(*u).unwrap().sandbox.clone()).collect()).collect();
    assert_eq!(got, [vec![Some(running[0].clone())], vec![Some(running[1].clone())]], "the stopped sandbox's pane did not come back");
    assert_eq!(b.pages.per, 2);
    assert!(b.expect.is_empty(), "nothing is being started");
}

impl Rig {
    /// A point in the empty part of pane `n`'s bar (right of its name).
    fn bar_point(&self, n: usize) -> (f32, f32) {
        let (r, _) = self.rt.presented(&format!("pane-{n}-grip")).unwrap_or_else(|| panic!("pane {n}'s grip"));
        (r[0] + r[2] / 2.0, r[1] + r[3] / 2.0)
    }

    fn rect(&self, key: &str) -> [f32; 4] {
        self.rt.presented(key).unwrap_or_else(|| panic!("{key} drawn")).0
    }

    /// Presses pane `n`'s bar and moves the pointer to `to` in steps; does not release.
    fn drag_to(&mut self, n: usize, to: (f32, f32)) {
        let (x, y) = self.bar_point(n);
        self.rt.pointer_down(x, y);
        self.rt.frame(0.016);
        for k in 1..=6 {
            let f = k as f32 / 6.0;
            self.rt.pointer_move(x + (to.0 - x) * f, y + (to.1 - y) * f);
            self.rt.frame(0.016);
        }
    }

    fn release(&mut self, at: (f32, f32)) {
        self.rt.pointer_up(at.0, at.1);
        self.rt.frame(0.016);
        self.settle();
    }

    fn zone(&self) -> Option<drag::Zone> {
        self.rt.state.drag.as_ref().and_then(|d| d.zone)
    }
}

/// Each kind of drop zone, its preview, the drop, and a cancel; nothing
/// recreated, at most one size per terminal, no key or click to a terminal.
#[test]
fn dragging_a_pane_by_its_bar_shows_zones_and_drops_once() {
    let mut r = Rig::new(&["a", "b", "c"], 1284.0, 816.0);
    let before = r.generations();
    let typed = |r: &Rig| -> usize { r.inputs.iter().map(|(_, rx)| rx.try_iter().filter(|i| matches!(i, TermInput::Bytes(_))).count()).sum() };
    let uid = |r: &Rig, name: &str| r.rt.state.panes.iter().find(|p| p.sandbox.as_deref() == Some(name)).unwrap().uid;
    // A press on the bar that does not travel only focuses its pane.
    let (x, y) = r.bar_point(2);
    r.rt.pointer_down(x, y);
    r.rt.pointer_up(x + 1.0, y);
    r.settle();
    assert_eq!(r.rt.state.panes[r.rt.state.focused].sandbox.as_deref(), Some("b"));
    assert!(r.rt.state.drag.is_none());
    // a | b over c. Drag a over b's bottom half: split, preview = b's bottom half.
    let b = r.rect("pane-2");
    let to = (b[0] + b[2] / 2.0, b[1] + b[3] * 0.9);
    r.drag_to(1, to);
    assert_eq!(r.zone(), Some(drag::Zone::Split { target: uid(&r, "b"), side: Side::Bottom }));
    let p = r.rect("drag-preview");
    assert!((p[1] - (b[1] + b[3] / 2.0)).abs() < 2.0 && (p[3] - b[3] / 2.0).abs() < 2.0, "preview {p:?} is b's bottom half of {b:?}");
    assert!(r.rt.presented("drag-ghost").is_some());
    assert_eq!(r.names(), [vec!["a", "b", "c"]], "nothing changes before the drop");
    assert!(r.take().values().all(|v| v.is_empty()), "no size while dragging");
    r.release(to);
    let t = &r.rt.state.pages.cur().tile;
    assert_eq!(t.order(), [uid(&r, "b"), uid(&r, "a"), uid(&r, "c")], "a went under b: b, a stacked, then c");
    assert!(r.rt.presented("drag-preview").is_none());
    r.check_kept(&before, "split");
    // Swap: a over c's middle.
    let n_a = r.rt.state.index_of(uid(&r, "a")).unwrap() + 1;
    let c = r.rect(&format!("pane-{}", r.rt.state.index_of(uid(&r, "c")).unwrap() + 1));
    let to = (c[0] + c[2] / 2.0, c[1] + c[3] / 2.0);
    r.drag_to(n_a, to);
    assert_eq!(r.zone(), Some(drag::Zone::Swap { target: uid(&r, "c") }));
    let p = r.rect("drag-preview");
    assert!((p[2] - c[2]).abs() < 2.0 && (p[3] - c[3]).abs() < 2.0, "swap preview is c whole");
    r.release(to);
    r.check_kept(&before, "swap");
    // Dock: c along the page's right edge.
    let area = r.rect("page-area");
    let n_c = r.rt.state.index_of(uid(&r, "c")).unwrap() + 1;
    let to = (area[0] + area[2] - 6.0, area[1] + area[3] / 2.0);
    r.drag_to(n_c, to);
    assert_eq!(r.zone(), Some(drag::Zone::Dock { side: Side::Right }));
    r.release(to);
    assert!(r.rt.state.pages.cur().tile.spans(uid(&r, "c"), Side::Right));
    r.check_kept(&before, "dock");
    // Cancel: Escape mid-drag puts nothing anywhere.
    let shape = r.rt.state.pages.cur().tile.clone();
    let b = r.rect(&format!("pane-{}", r.rt.state.index_of(uid(&r, "b")).unwrap() + 1));
    let to = (b[0] + b[2] / 2.0, b[1] + b[3] / 2.0);
    let n_c = r.rt.state.index_of(uid(&r, "c")).unwrap() + 1;
    r.drag_to(n_c, to);
    assert!(r.zone().is_some());
    r.rt.key(KeyEvent { key: Key::Escape, mods: rune::Mods::default() });
    r.rt.frame(0.016);
    r.release(to);
    assert_eq!(r.rt.state.pages.cur().tile, shape, "Escape cancelled the drop");
    // A drop outside any zone (the sidebar) changes nothing either.
    let side = r.rect("sidebar");
    let to = (side[0] + 40.0, side[1] + 200.0);
    r.drag_to(n_c, to);
    assert_eq!(r.zone(), None);
    r.release(to);
    assert_eq!(r.rt.state.pages.cur().tile, shape);
    // The pager's "+": a new page with that pane, shown.
    let plus = r.rect("page-new");
    let to = (plus[0] + plus[2] / 2.0, plus[1] + plus[3] / 2.0);
    r.drag_to(n_c, to);
    assert_eq!(r.zone(), Some(drag::Zone::NewPage));
    r.release(to);
    assert_eq!(r.rt.state.pages.list.len(), 2);
    assert_eq!(r.rt.state.pages.current, 1);
    r.check_kept(&before, "new page");
    // Back onto page 1 by its tab.
    r.settle();
    let tab = r.rect("page-1");
    let to = (tab[0] + tab[2] / 2.0, tab[1] + tab[3] / 2.0);
    let n = r.rt.state.index_of(uid(&r, "c")).unwrap() + 1;
    r.drag_to(n, to);
    assert_eq!(r.zone(), Some(drag::Zone::Page { page: 0, full: false }));
    r.release(to);
    assert_eq!(r.rt.state.pages.list.len(), 1, "the emptied page went");
    assert_eq!(r.rt.state.pages.current, 0);
    r.check_kept(&before, "to page 1");
    assert_eq!(typed(&r), 0, "no key or click reached a terminal");
}

/// The pane menu's moves: the keyboard way to the same places.
#[test]
fn the_pane_menu_moves_rotates_and_sends_to_a_page() {
    let mut r = Rig::new(&["a", "b", "c"], 1284.0, 816.0);
    let before = r.generations();
    super::tests::tap(&mut r.rt, "pane-2-more");
    assert!(r.rt.state.drawn("pane-2-rotate") && r.rt.state.drawn("pane-2-move-left"));
    // b and c are stacked on the right: rotate makes them side by side.
    super::tests::tap(&mut r.rt, "pane-2-rotate");
    r.settle();
    match r.rt.state.pages.cur().tile.at("b") {
        Some(Tile::Split { dir, .. }) => assert_eq!(*dir, tiling::Dir::Row, "landscape to portrait"),
        other => panic!("{other:?}"),
    }
    r.check_kept(&before, "rotate");
    // Move left: a and b trade places.
    let b = r.rt.state.panes.iter().position(|p| p.sandbox.as_deref() == Some("b")).unwrap() + 1;
    super::tests::tap(&mut r.rt, &format!("pane-{b}-more"));
    super::tests::tap(&mut r.rt, &format!("pane-{b}-move-left"));
    r.settle();
    assert_eq!(r.names(), [vec!["b", "a", "c"]]);
    r.check_kept(&before, "move left");
    // At the left edge, Move left is off (it spans the whole left side).
    assert!(r.rt.state.pane_moves(0).iter().any(|m| m.key == "pane-1-move-left" && m.op.is_none()));
    super::tests::tap(&mut r.rt, "pane-1-more");
    super::tests::tap(&mut r.rt, "pane-1-to-new-page");
    r.settle();
    assert_eq!(r.names(), [vec!["a", "c"], vec!["b"]]);
    r.check_kept(&before, "to a new page");
}

#[test]
fn dividers_keep_the_least_size_and_double_press_equalises() {
    let mut r = Rig::new(&["a", "b"], 1284.0, 816.0);
    let h = r.rect("panes-handle");
    let (x, y) = (h[0] + h[2] / 2.0, h[1] + h[3] / 2.0);
    r.rt.pointer_down(x, y);
    r.rt.frame(0.016);
    r.rt.pointer_move(0.0, y);
    r.rt.frame(0.016);
    r.rt.pointer_up(0.0, y);
    r.settle();
    let a = r.rect("pane-1");
    assert!((a[2] - tiling::MIN_W).abs() < 2.0, "pane 1 stops at its least width: {a:?}");
    for (_, sizes) in r.take() {
        assert!(sizes.len() <= 2, "the divider's live fit is debounced: {sizes:?}");
    }
    // Double press: equal again.
    let h = r.rect("panes-handle");
    let (x, y) = (h[0] + h[2] / 2.0, h[1] + h[3] / 2.0);
    for _ in 0..2 {
        r.rt.pointer_down(x, y);
        r.rt.pointer_up(x, y);
        r.rt.frame(0.05);
    }
    r.settle();
    let (a, b) = (r.rect("pane-1"), r.rect("pane-2"));
    assert!((a[2] - b[2]).abs() < 2.0, "equal: {a:?} {b:?}");
}

/// A session line's "+" (desktop-netfilter: `open_terminal {sandbox,
/// thread, pane = "spare"}` from the sandboxes screen) with the page full:
/// the session goes to the next page with room, else a new page, and no
/// open pane is ever replaced (the old rule replaced the least recently
/// used one when all four were full).
#[test]
fn a_spare_open_from_a_screen_never_replaces_an_open_pane() {
    let mut r = Rig::new(&["a", "b", "c", "d"], 1284.0, 816.0);
    let before = r.generations();
    let plus = |r: &mut Rig, thread: &str| {
        let t = thread.to_string();
        r.rt.update(move |s, _| {
            let i = Intent::OpenTerminal { sandbox: "web-app".into(), mode: Mode::Agent, thread: Some(t), pane: PaneTarget::Spare, start: crate::intent::ThreadStart::default() };
            crate::screen::Host::intents(s, "sandboxes", vec![i]);
        });
        r.settle();
    };
    plus(&mut r, "t1");
    assert_eq!(r.names(), [vec!["a", "b", "c", "d"], vec!["web-app"]], "page 1 full: a new page");
    assert_eq!(r.rt.state.pages.current, 1);
    r.check_kept(&before, "first +");
    // Back on page 1 (still full): the next page with room, page 2.
    r.op(PageOp::Show(0));
    plus(&mut r, "t2");
    assert_eq!(r.names(), [vec!["a", "b", "c", "d"], vec!["web-app", "web-app"]]);
    assert_eq!(r.rt.state.pages.current, 1);
    // Room on the current page: it stays there.
    r.op(PageOp::Show(0));
    r.op(PageOp::Per(6));
    plus(&mut r, "t3");
    assert_eq!(r.rt.state.pages.current, 0);
    assert_eq!(r.names()[0], ["a", "b", "c", "d", "web-app"]);
    r.check_kept(&before, "+ with room");
    assert!(r.rt.state.notice.as_ref().map_or(true, |n| !n.0.contains("least recently used")));
}

/// Every modal fixture (a fixture whose `app.modal` opens one), at a narrow
/// window and a wide one: no keyed element of the modal's screen lies
/// outside the modal's rectangle (a field's button pushed out of its row,
/// a row wider than the card), and a screen sized by its content gets a card
/// no taller than it needs.
#[test]
fn modal_content_stays_inside_the_modal() {
    fn fixtures(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        for e in std::fs::read_dir(dir).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                fixtures(&p, out);
            } else if p.extension().is_some_and(|x| x == "json") {
                let v: J = std::fs::read_to_string(&p).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or(J::Null);
                if v["app"]["modal"].is_array() {
                    out.push(p);
                }
            }
        }
    }
    fn keys(n: &crate::luau::Node, out: &mut Vec<String>) {
        if let Some(k) = crate::screen::key_of(n) {
            out.push(k);
        }
        n.children.iter().for_each(|c| keys(c, out));
    }
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures");
    let mut found = vec![];
    fixtures(&root, &mut found);
    // (fixture, intent opening the modal: none when the fixture opens it).
    let mut list: Vec<(std::path::PathBuf, J)> = found.into_iter().map(|p| (p, J::Null)).collect();
    for f in ["setup/full.json", "setup/errors.json"] {
        for tab in ["secrets", "kits", "skills", "mcp", "hosts", "voice"] {
            list.push((root.join(f), serde_json::json!({ "kind": "intent", "name": "open_modal", "args": { "screen": "setup", "arg": tab } })));
        }
    }
    for f in ["create/workspace-full.json", "create/workspace-errors.json", "create/chat.json"] {
        for d in ["chooser", "workspace", "chat"] {
            list.push((root.join(f), serde_json::json!({ "kind": "intent", "name": "create_open", "args": { "dialog": d } })));
        }
    }
    list.push((root.join("envfile/basic.json"), J::Null));
    list.sort_by(|a, b| a.0.cmp(&b.0));
    assert!(list.len() >= 20, "{list:?}");
    let mut bad = vec![];
    let mut checked = 0;
    for (path, open) in &list {
        for (w, h) in [(420.0, 700.0), (1284.0, 816.0)] {
            let mut a = App::new(Args { fixture: Some(path.to_string_lossy().to_string().into()), ..Args::default() });
            if !open.is_null() {
                for i in crate::intent::parse(open).unwrap() {
                    a.run_intent("test", i);
                }
            }
            let mut rt = rune::Runtime::new(a, view, (w, h), rune::Env::new(true, 1.0));
            // Past the card's entrance (it grows in).
            for _ in 0..60 {
                rt.frame(0.016);
            }
            let Some((m, _)) = rt.presented("modal") else { continue };
            let Some((name, _)) = rt.state.feats.modal.clone() else { continue };
            let Some(node) = rt.state.screens.iter().find(|s| s.name == name).and_then(|s| s.last_good.clone()) else { continue };
            let mut ks = vec![];
            keys(&node, &mut ks);
            for k in ks {
                let Some((r, _)) = rt.presented(&k) else { continue };
                checked += 1;
                // Sideways: inside the card. Down: only above its top (a
                // long screen scrolls inside the card).
                if r[0] < m[0] - 0.5 || r[0] + r[2] > m[0] + m[2] + 0.5 || r[1] < m[1] - 0.5 {
                    bad.push(format!("{} at {w}: {name}: {k} at {r:?}, modal {m:?}", path.file_name().unwrap().to_string_lossy()));
                }
            }
            if crate::screen::sized_by_content(&node) {
                let body = rt.presented("modal-body").map(|b| b.0[3]).unwrap_or(0.0);
                assert!(m[3] <= body + 32.0 + 0.5, "{}: the card ({}) is taller than its content ({body})", path.display(), m[3]);
            }
        }
    }
    assert!(checked > 50, "{checked} elements checked");
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}
