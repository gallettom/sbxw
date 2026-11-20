//! The leader mark (the crown) before the name on a team leader's pane bar
//! and page tab: the rule is `App::leaders` (leads.rs), drawing only.

use super::layout_tests::Rig;
use super::*;

fn set(r: &mut Rig, leads: serde_json::Value) {
    r.rt.update(|s, _| {
        s.sandboxes = ["a", "b", "c"].iter().map(|n| crate::client::Sandbox::new(n, "claude", "running")).collect();
        s.feats.data.insert("leads".into(), leads);
        s.feats.data.entry("plans").or_insert(serde_json::json!([]));
        s.push_data();
    });
    r.settle();
}

/// Panes 1 and 2: "a" (leads) and "b"; pane 3: a thread of "a"; pane 4: "c".
fn rig(w: f32) -> Rig {
    let mut r = Rig::new(&["a", "b", "a", "c"], w, 816.0);
    r.rt.update(|s, _| s.panes[2].thread = Some("t1".into()));
    set(&mut r, serde_json::json!([]));
    r
}

#[test]
fn a_leader_pane_has_the_mark_a_thread_and_a_plain_session_do_not() {
    let mut r = rig(1284.0);
    for n in 1..=4 {
        assert!(!r.rt.state.drawn(&format!("pane-{n}-leader")), "no team: pane {n} has no mark");
    }
    set(&mut r, serde_json::json!(["a"]));
    assert!(r.rt.state.drawn("pane-1-leader"));
    for n in 2..=4 {
        assert!(!r.rt.state.drawn(&format!("pane-{n}-leader")), "pane {n} is not a leader");
    }
    // Before the name, inside the bar, not touching the name.
    let (m, _) = r.rt.presented("pane-1-leader").unwrap();
    let (name, _) = r.rt.presented("pane-1-name").unwrap();
    let (bar, _) = r.rt.presented("pane-1-bar").unwrap();
    assert!(m[0] + m[2] <= name[0] + 0.5 && m[0] >= bar[0] && m[1] >= bar[1] && m[1] + m[3] <= bar[1] + bar[3] + 0.5);
}

#[test]
fn the_mark_follows_the_team_starting_and_ending() {
    let mut r = rig(1284.0);
    set(&mut r, serde_json::json!(["a"]));
    assert!(r.rt.state.drawn("pane-1-leader"));
    set(&mut r, serde_json::json!(["b"]));
    assert!(!r.rt.state.drawn("pane-1-leader") && r.rt.state.drawn("pane-2-leader"));
    set(&mut r, serde_json::json!([]));
    assert!(!r.rt.state.drawn("pane-2-leader"));
}

#[test]
fn the_mark_shows_with_lead_mode_off_and_on() {
    let mut r = rig(1284.0);
    set(&mut r, serde_json::json!(["a"]));
    assert!(!r.rt.state.leads_on() && r.rt.state.drawn("pane-1-leader"));
    r.rt.update(|s, _| s.toggle_leads());
    r.settle();
    assert!(r.rt.state.leads_on());
    assert!(r.rt.state.drawn("pane-1-leader"));
}

#[test]
fn at_a_narrow_pane_the_mark_stays_whole_and_nothing_overlaps() {
    let mut r = rig(1284.0);
    r.rt.update(|s, _| s.panes[0].sandbox = Some("a-sandbox-with-a-very-long-name-indeed".into()));
    set(&mut r, serde_json::json!(["a-sandbox-with-a-very-long-name-indeed"]));
    for w in [1284.0, 900.0, 720.0] {
        r.rt.set_viewport((w, 816.0), 1.0);
        r.settle();
        let (bar, _) = r.rt.presented("pane-1-bar").unwrap();
        let (m, _) = r.rt.presented("pane-1-leader").expect("the mark is drawn");
        let (name, _) = r.rt.presented("pane-1-name").unwrap();
        let (close, _) = r.rt.presented("pane-1-close").unwrap();
        assert!((m[2] - bar::CHIP_ICON).abs() < 0.5, "{w}: the mark keeps its width, {m:?}");
        assert!(m[0] + m[2] <= name[0] + 0.5, "{w}: mark {m:?} before name {name:?}");
        assert!(name[0] + name[2] <= close[0] + 0.5, "{w}: name {name:?} clear of close {close:?}");
        assert!(close[0] + close[2] <= bar[0] + bar[2] + 0.5);
        let f = r.rt.state.pane_fit(0).unwrap();
        for it in &f.shown {
            let (c, _) = r.rt.presented(&it.key(1)).unwrap();
            assert!(c[0] + c[2] <= close[0] + 0.5 && c[0] >= name[0] + 0.5 - 1.0, "{w}: chip {c:?}");
        }
    }
}

#[test]
fn a_single_terminal_page_tab_carries_the_mark_too() {
    let mut r = Rig::new(&["a", "b"], 1284.0, 816.0);
    r.rt.update(|s, _| {
        s.pages.per = 1;
    });
    set(&mut r, serde_json::json!(["a"]));
    let mut singles = 0;
    for k in 0..r.rt.state.pages.list.len() {
        let order = r.rt.state.pages.list[k].tile.order();
        if order.len() == 1 {
            singles += 1;
            let sb = r.rt.state.pane_by_uid(order[0]).and_then(|p| p.sandbox.clone());
            assert_eq!(r.rt.state.tab_info(k).leader, sb.as_deref() == Some("a"), "page {k}: {sb:?}");
        }
    }
    assert!(singles >= 1);
}
