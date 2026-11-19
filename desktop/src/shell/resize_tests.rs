//! The pane area hidden, then shown again (RESIZE.md): while something is
//! drawn over the panes (the base view, an overlay) no terminal is resized;
//! when they show again, each shown terminal ends with the size its rect
//! allows, with one resize for each one whose size changed and none for the
//! others, whatever changed meanwhile (Lead Mode, pages, Tabs or Grid, the
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

/// The user's report: TUI with four terminals, GUI, Lead Mode on, TUI: the
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
    one_each(&mut r, &[], "Lead Mode on under GUI");
    tap(&mut r.rt, "top-view-tui");
    settle(&mut r);
    assert_eq!(shown(&r), vec![a]);
    all_fit(&r, "back in TUI");
    assert_ne!(sizes(&r)[&a], small, "the leader's terminal grew");
    one_each(&mut r, &[a], "back in TUI");
}
