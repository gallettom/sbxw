//! The pane area hidden, then shown again (RESIZE.md): while something is
//! drawn over the panes (the base view, an overlay) no terminal is resized;
//! when they show again, each shown terminal ends with the size its rect
//! allows, with one resize for each one whose size changed and none for the
//! others, whatever changed meanwhile (Leads only, pages, Tabs or Grid, the
//! sidebar, the window).

use super::layout_tests::Rig;
use super::tests::tap;
use super::*;

/// About a second of frames, each followed by the app's per-frame work as
/// main.rs runs it (`App::poll`), so a debounced resize goes out.
fn settle(r: &mut Rig) {
    for _ in 0..40 {
        r.clock += 0.03;
        r.rt.frame(0.03);
        let c = r.clock;
        r.rt.update(|s, tx| {
            s.poll(c, tx);
        });
    }
}

/// Four terminals in a grid on one page; "a" leads a team.
fn rig() -> Rig {
    let mut r = Rig::new(&["a", "b", "c", "d"], 1284.0, 816.0);
    // The fixture daemon's first lists in, then ours over them.
    settle(&mut r);
    r.rt.update(|s, _| {
        s.sandboxes = ["a", "b", "c", "d"].iter().map(|n| crate::client::Sandbox::new(n, "claude", "running")).collect();
        s.feats.data.insert("leads".into(), serde_json::json!(["a"]));
        s.feats.data.entry("plans").or_insert(serde_json::json!([]));
        s.push_data();
    });
    settle(&mut r);
    r.take();
    r
}

/// The cells pane `uid`'s laid-out room holds (`TerminalState::fit_inner`'s rule).
fn allowed(r: &Rig, uid: Uid) -> (usize, usize) {
    let t = r.rt.state.pane_by_uid(uid).and_then(|p| p.term.as_ref()).expect("a terminal");
    let (rect, _) = r.rt.presented(&t.fit_key()).expect("its room is drawn");
    let cols = (rect[2] / crate::terminal::cell_width()).floor().max(10.0) as usize;
    let rows = (rect[3] / crate::terminal::LINE_HEIGHT).floor().max(3.0) as usize;
    (cols, rows)
}

/// The panes on the current page, by id.
fn shown(r: &Rig) -> Vec<Uid> {
    r.rt.state.pages.cur().tile.order()
}

/// Each shown terminal has the size its rect allows (grid, wanted, sent).
fn all_fit(r: &Rig, what: &str) {
    for u in shown(r) {
        let t = r.rt.state.pane_by_uid(u).and_then(|p| p.term.as_ref()).expect("a terminal");
        let a = allowed(r, u);
        assert_eq!((t.size, t.wanted_size(), t.sent_size()), (a, Some(a), Some(a)), "{what}: pane {u} does not fill its rect");
    }
}

/// Resizes sent since the last take, by pane: at most one each, and exactly
/// one for the panes in `changed`.
fn one_each(r: &mut Rig, changed: &[Uid], what: &str) {
    for (u, sizes) in r.take() {
        let want = usize::from(changed.contains(&u));
        assert_eq!(sizes.len(), want, "{what}: pane {u} sent {sizes:?}");
    }
}

fn sizes(r: &Rig) -> HashMap<Uid, (usize, usize)> {
    r.rt.state.panes.iter().filter_map(|p| Some((p.uid, p.term.as_ref()?.size))).collect()
}

/// The terminals whose size differs from `before`.
fn changed(r: &Rig, before: &HashMap<Uid, (usize, usize)>) -> Vec<Uid> {
    sizes(r).into_iter().filter(|(u, z)| before.get(u) != Some(z)).map(|(u, _)| u).collect()
}

/// The user's report: TUI with four terminals, GUI, Leads only on, TUI: the
/// leader's pane fills the area, and its terminal must too.
#[test]
fn lead_mode_turned_on_in_gui_fills_the_leader_s_pane_back_in_tui() {
    let mut r = rig();
    all_fit(&r, "four in a grid");
    let a = r.rt.state.panes[0].uid;
    let small = sizes(&r)[&a];
    tap(&mut r.rt, "top-view-gui");
    settle(&mut r);
    one_each(&mut r, &[], "to GUI");
    tap(&mut r.rt, "top-lead");
    settle(&mut r);
    assert!(r.rt.state.leads_on());
    one_each(&mut r, &[], "Leads only on under GUI");
    tap(&mut r.rt, "top-view-tui");
    settle(&mut r);
    assert_eq!(shown(&r), vec![a]);
    all_fit(&r, "back in TUI");
    assert_ne!(sizes(&r)[&a], small, "the leader's terminal grew");
    one_each(&mut r, &[a], "back in TUI");
}

/// Leads only off again in GUI, then TUI: the four panes come back in their
/// grid, each terminal at its size; the leader's shrinks back with one
/// resize, the parked ones never changed and send none.
#[test]
fn lead_mode_turned_off_in_gui_puts_every_terminal_back_at_its_size() {
    let mut r = rig();
    let a = r.rt.state.panes[0].uid;
    let before = sizes(&r);
    for key in ["top-view-gui", "top-lead", "top-view-tui", "top-view-gui"] {
        tap(&mut r.rt, key);
        settle(&mut r);
    }
    r.take();
    tap(&mut r.rt, "top-lead");
    settle(&mut r);
    assert!(!r.rt.state.leads_on());
    one_each(&mut r, &[], "Leads only off under GUI");
    tap(&mut r.rt, "top-view-tui");
    settle(&mut r);
    assert_eq!(shown(&r).len(), 4);
    all_fit(&r, "the grid back");
    assert_eq!(sizes(&r), before, "every terminal at its size from before");
    one_each(&mut r, &[a], "the grid back");
}

/// Leads only in TUI directly (already right before the fix): one resize
/// for the leader each way, none for the others.
#[test]
fn lead_mode_toggled_in_tui_resizes_the_leader_once_each_way() {
    let mut r = rig();
    let a = r.rt.state.panes[0].uid;
    tap(&mut r.rt, "top-lead");
    settle(&mut r);
    all_fit(&r, "Leads only on in TUI");
    one_each(&mut r, &[a], "Leads only on in TUI");
    tap(&mut r.rt, "top-lead");
    settle(&mut r);
    all_fit(&r, "Leads only off in TUI");
    one_each(&mut r, &[a], "Leads only off in TUI");
}

/// Pages and Tabs | Grid changed while Sessions is drawn (from the
/// palette, an intent): the panes then shown fit their rects once TUI is
/// back; changed in TUI, then GUI and TUI, nothing more is sent.
#[test]
fn pages_and_tabs_or_grid_changed_under_gui_fit_back_in_tui() {
    let mut r = rig();
    let ids = shown(&r);
    // Tabs in TUI, then GUI, then TUI: the sizes of Tabs, nothing more.
    r.rt.update(|s, _| s.run_intent("test", Intent::Pages(PageOp::View(true))));
    settle(&mut r);
    all_fit(&r, "Tabs in TUI");
    r.take();
    tap(&mut r.rt, "top-view-gui");
    settle(&mut r);
    tap(&mut r.rt, "top-view-tui");
    settle(&mut r);
    all_fit(&r, "Tabs, GUI, TUI");
    one_each(&mut r, &[], "Tabs, GUI, TUI");
    // Grid chosen under GUI: the four tiles again, each terminal fitted.
    tap(&mut r.rt, "top-view-gui");
    settle(&mut r);
    r.rt.update(|s, _| s.run_intent("test", Intent::Pages(PageOp::View(false))));
    settle(&mut r);
    one_each(&mut r, &[], "Grid under GUI");
    let was = sizes(&r);
    tap(&mut r.rt, "top-view-tui");
    settle(&mut r);
    assert_eq!(shown(&r), ids);
    all_fit(&r, "Grid under GUI, then TUI");
    // The tab that was current had grown; the others kept their tile's size.
    let moved = changed(&r, &was);
    assert_eq!(moved.len(), 1, "{moved:?}");
    one_each(&mut r, &moved, "Grid under GUI, then TUI");
    // 2 per page and page 2 under GUI: page 2's two panes fit.
    tap(&mut r.rt, "top-view-gui");
    settle(&mut r);
    r.rt.update(|s, _| {
        s.run_intent("test", Intent::Pages(PageOp::Per(2)));
        s.run_intent("test", Intent::Pages(PageOp::Show(1)));
    });
    settle(&mut r);
    one_each(&mut r, &[], "pages under GUI");
    let was = sizes(&r);
    tap(&mut r.rt, "top-view-tui");
    settle(&mut r);
    let page2 = shown(&r);
    assert_eq!(page2.len(), 2);
    all_fit(&r, "page 2 under GUI, then TUI");
    let mut moved = changed(&r, &was);
    moved.sort();
    let mut want = page2.clone();
    want.sort();
    assert_eq!(moved, want, "page 2's two terminals grew");
    one_each(&mut r, &page2, "page 2 under GUI, then TUI");
}

/// The window resized while Sessions is drawn: nothing is sent then, and
/// back in TUI each terminal fits the new window with one resize.
#[test]
fn a_window_resized_under_gui_fits_back_in_tui() {
    let mut r = rig();
    let ids = shown(&r);
    tap(&mut r.rt, "top-view-gui");
    settle(&mut r);
    r.rt.set_viewport((1000.0, 700.0), 1.0);
    settle(&mut r);
    one_each(&mut r, &[], "window resized under GUI");
    tap(&mut r.rt, "top-view-tui");
    settle(&mut r);
    all_fit(&r, "back in TUI after a window resize");
    one_each(&mut r, &ids, "back in TUI after a window resize");
}

/// The sidebar hidden while Sessions is drawn: nothing then, one resize per
/// terminal back in TUI (as hiding it in TUI does).
#[test]
fn the_sidebar_hidden_under_gui_fits_back_in_tui() {
    let mut r = rig();
    let ids = shown(&r);
    tap(&mut r.rt, "top-view-gui");
    settle(&mut r);
    r.rt.update(|s, _| s.run_intent("test", Intent::Sidebar { show: Some(false) }));
    settle(&mut r);
    one_each(&mut r, &[], "sidebar hidden under GUI");
    tap(&mut r.rt, "top-view-tui");
    settle(&mut r);
    all_fit(&r, "sidebar hidden under GUI, then TUI");
    one_each(&mut r, &ids, "sidebar hidden under GUI, then TUI");
}

/// An overlay (Files) over the terminals while Leads only is turned on from
/// the top bar: nothing resizes under Files; closing it fits the leader.
/// Then Leads only off under Files, closed: the grid back at its sizes.
#[test]
fn lead_mode_toggled_under_an_overlay_fits_once_it_closes() {
    let mut r = rig();
    let a = r.rt.state.panes[0].uid;
    let before = sizes(&r);
    tap(&mut r.rt, "rail-files");
    settle(&mut r);
    assert_eq!(r.rt.state.feats.overlay.as_ref().map(|o| o.0.as_str()), Some("files"));
    tap(&mut r.rt, "top-lead");
    settle(&mut r);
    assert!(r.rt.state.leads_on());
    one_each(&mut r, &[], "Leads only on under Files");
    r.rt.key(KeyEvent { key: rune::Key::Escape, mods: rune::Mods::default() });
    settle(&mut r);
    assert!(r.rt.state.feats.overlay.is_none());
    all_fit(&r, "Files closed");
    one_each(&mut r, &[a], "Files closed");
    tap(&mut r.rt, "rail-files");
    settle(&mut r);
    tap(&mut r.rt, "top-lead");
    settle(&mut r);
    one_each(&mut r, &[], "Leads only off under Files");
    tap(&mut r.rt, "overlay-close");
    settle(&mut r);
    all_fit(&r, "Files closed again");
    assert_eq!(sizes(&r), before);
    one_each(&mut r, &[a], "Files closed again");
}

/// A restart that restores GUI with Leads only on: the terminals are not
/// fitted under Sessions, and TUI fits the leader's to the whole area.
#[test]
fn a_restart_in_gui_with_lead_mode_on_fits_the_leader_in_tui() {
    let mut r = rig();
    let a = r.rt.state.panes[0].uid;
    r.rt.update(|s, _| {
        s.store.set(super::features::BASE_KEY, J::String("monitor".into()));
        s.store.set(super::leads::LEAD_KEY, J::Bool(true));
        s.kept_base();
        s.leads_restore = true;
        s.listed = true;
        s.feats.data.entry("sessions").or_insert(serde_json::json!([]));
    });
    settle(&mut r);
    assert!(r.rt.state.leads_on() && r.rt.state.feats.base.as_deref() == Some("monitor"), "GUI with Leads only on, as kept");
    one_each(&mut r, &[], "restored under GUI");
    tap(&mut r.rt, "top-view-tui");
    settle(&mut r);
    assert_eq!(shown(&r), vec![a]);
    all_fit(&r, "TUI after the restart");
    one_each(&mut r, &[a], "TUI after the restart");
}

/// The web UI owns a terminal's size (`size` frames, owner false): GUI and
/// TUI with its room unchanged send nothing for it, and it is still drawn
/// at the PTY's size (scaled), not resized.
#[test]
fn a_terminal_sized_by_another_window_stays_scaled_through_gui_and_tui() {
    let mut r = rig();
    let b = r.rt.state.panes[1].uid;
    r.events[1].send(crate::client::TermEvent::Size { cols: 120, rows: 40, owner: false }).unwrap();
    settle(&mut r);
    let t = |r: &Rig| r.rt.state.pane_by_uid(b).and_then(|p| p.term.as_ref()).map(|t| (t.size, t.sized_elsewhere())).unwrap();
    assert_eq!(t(&r), ((120, 40), true));
    r.take();
    tap(&mut r.rt, "top-view-gui");
    settle(&mut r);
    tap(&mut r.rt, "top-view-tui");
    settle(&mut r);
    one_each(&mut r, &[], "GUI and TUI with the web UI owning b");
    assert_eq!(t(&r), ((120, 40), true), "still the PTY's size, sized elsewhere");
}
