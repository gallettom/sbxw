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
