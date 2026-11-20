//! The leader mark (the crown) before the name on a team leader's pane bar
//! and page tab: the rule is `App::leaders` (leads.rs), drawing only.

use super::layout_tests::Rig;
use super::*;

fn marked(r: &Rig, key: &str) -> bool {
    r.rt.presented(key).is_some()
}

fn set(r: &mut Rig, leads: serde_json::Value) {
    r.rt.update(|s, _| {
        let mut names: Vec<String> = ["a", "b", "c"].iter().map(|n| n.to_string()).collect();
        names.extend(s.panes.iter().filter_map(|p| p.sandbox.clone()));
        s.sandboxes = names.iter().map(|n| crate::client::Sandbox::new(n, "claude", "running")).collect();
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
        assert!(!marked(&r, &format!("pane-{n}-leader")), "no team: pane {n} has no mark");
    }
    set(&mut r, serde_json::json!(["a"]));
    assert!(marked(&r, "pane-1-leader"));
    for n in 2..=4 {
        assert!(!marked(&r, &format!("pane-{n}-leader")), "pane {n} is not a leader");
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
    assert!(marked(&r, "pane-1-leader"));
    set(&mut r, serde_json::json!(["b"]));
    assert!(!marked(&r, "pane-1-leader") && marked(&r, "pane-2-leader"));
    set(&mut r, serde_json::json!([]));
    assert!(!marked(&r, "pane-2-leader"));
}

#[test]
fn the_mark_shows_with_lead_mode_off_and_on() {
    let mut r = rig(1284.0);
    set(&mut r, serde_json::json!(["a"]));
    assert!(!r.rt.state.leads_on() && marked(&r, "pane-1-leader"));
    r.rt.update(|s, _| s.toggle_leads());
    r.settle();
    assert!(r.rt.state.leads_on());
    assert!(marked(&r, "pane-1-leader"));
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
    let mut r = Rig::new(&["a"], 1284.0, 816.0);
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

#[test]
fn a_tab_naming_several_terminals_has_a_crown_before_each_leader() {
    let mut r = Rig::new(&["a", "b"], 1284.0, 816.0);
    set(&mut r, serde_json::json!([]));
    let t = r.rt.state.tab_info(0);
    assert_eq!(t.label, "a, b");
    assert!(!t.leader && t.crowns() == 0 && !marked(&r, "page-1-leader-1"));
    set(&mut r, serde_json::json!(["b"]));
    let t = r.rt.state.tab_info(0);
    assert_eq!(t.parts, vec![("a".to_string(), false), ("b".to_string(), true)]);
    assert!(t.leader && t.crowns() == 1);
    assert!(!marked(&r, "page-1-leader-1") && marked(&r, "page-1-leader-2"));
    set(&mut r, serde_json::json!(["a", "b"]));
    assert!(marked(&r, "page-1-leader-1") && marked(&r, "page-1-leader-2"));
    let (c1, _) = r.rt.presented("page-1-leader-1").unwrap();
    let (c2, _) = r.rt.presented("page-1-leader-2").unwrap();
    assert!(c1[0] + c1[2] <= c2[0], "crowns {c1:?} {c2:?} do not overlap");
    // The tab's width counts the crowns: the strip's fit stays inside the pager.
    let (first, end) = r.rt.state.pager_fit();
    assert!(first == 0 && end >= 1);
}

#[test]
fn a_count_label_gets_one_crown_when_any_terminal_leads() {
    let mut r = Rig::new(&["a", "b", "c", "d"], 1284.0, 816.0);
    set(&mut r, serde_json::json!(["c"]));
    r.rt.update(|s, _| {
        for p in s.panes.iter_mut() {
            p.sandbox = p.sandbox.as_ref().map(|n| format!("{n}-with-a-rather-long-sandbox-name"));
        }
        let names: Vec<String> = s.panes.iter().filter_map(|p| p.sandbox.clone()).collect();
        s.sandboxes = names.iter().map(|n| crate::client::Sandbox::new(n, "claude", "running")).collect();
        s.feats.data.insert("leads".into(), serde_json::json!([names[2]]));
        s.push_data();
    });
    r.settle();
    let t = r.rt.state.tab_info(0);
    assert!(t.label.starts_with("Page ") && t.parts.is_empty() && t.leader);
    assert!(marked(&r, "page-1-leader"));
}
