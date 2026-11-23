//! The leader mark (the crown) on a team leader's pane bar (inside the name
//! pill, before the name) and page tab: the rule is `App::leaders`
//! (leads.rs), drawing only. And the name pill that carries team mode.

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
    // Inside the name pill (its first child, before the name's text), inside the bar.
    let (m, _) = r.rt.presented("pane-1-leader").unwrap();
    let (name, _) = r.rt.presented("pane-1-name").unwrap();
    let (bar, _) = r.rt.presented("pane-1-bar").unwrap();
    assert!(m[0] >= name[0] && m[0] + m[2] <= name[0] + name[2] + 0.5 && m[0] >= bar[0] && m[1] >= bar[1] && m[1] + m[3] <= bar[1] + bar[3] + 0.5);
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
        assert!(m[0] >= name[0] && m[0] + m[2] <= name[0] + name[2] + 0.5, "{w}: mark {m:?} inside the name pill {name:?}");
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

// ---------------------------------------------------------------------------
// The name pill: it carries team mode (the "Team on" chip is gone).

fn tap(r: &mut Rig, key: &str) {
    let (x, y) = r.rt.find(key).unwrap_or_else(|| panic!("{key} drawn"));
    r.rt.pointer_down(x, y);
    r.rt.pointer_up(x, y);
    r.settle();
}

/// The drawn x of each key that is on screen, left to right.
fn drawn_order(r: &Rig, keys: &[String]) -> Vec<String> {
    let mut v: Vec<(f32, String)> = keys.iter().filter_map(|k| r.rt.presented(k).map(|(m, _)| (m[0], k.clone()))).collect();
    v.sort_by(|a, b| a.0.total_cmp(&b.0));
    v.into_iter().map(|(_, k)| k).collect()
}

#[test]
fn the_name_pill_carries_the_team_and_no_team_chip_is_left() {
    let mut r = rig(1284.0);
    set(&mut r, serde_json::json!(["a"]));
    let team = |r: &Rig, i: usize| r.rt.state.pane_fit(i).unwrap().team.map(|t| t.on);
    // "a" leads: on; "b": off (it may lead); a thread of "a": no toggle.
    assert_eq!((team(&r, 0), team(&r, 1), team(&r, 2)), (Some(true), Some(false), None));
    for n in 1..=4 {
        assert!(!marked(&r, &format!("pane-{n}-x-lead")), "pane {n}: no Team chip");
        let f = r.rt.state.pane_fit(n - 1).unwrap();
        assert!(f.hidden.iter().all(|i| i.id != "lead"), "pane {n}: no Team row in the menu");
    }
    // On: the leader icon inside the pill, before the name's text, and the switch after the pill.
    let (pill, _) = r.rt.presented("pane-1-name").unwrap();
    let (mark, _) = r.rt.presented("pane-1-leader").unwrap();
    assert!(mark[0] >= pill[0] && mark[0] + mark[2] <= pill[0] + pill[2] + 0.5, "icon {mark:?} in pill {pill:?}");
    let (sw, _) = r.rt.presented("pane-1-mode-switch").unwrap();
    assert!(pill[0] + pill[2] <= sw[0] + 0.5);
    // Off and none: no icon.
    assert!(!marked(&r, "pane-2-leader") && !marked(&r, "pane-3-leader"));
}

#[test]
fn pressing_the_name_pill_runs_what_the_team_chip_ran() {
    let mut r = rig(1284.0);
    set(&mut r, serde_json::json!(["a"]));
    // On: lead_end, the core's confirmation first, as before.
    tap(&mut r, "pane-1-name");
    let title = r.rt.state.feats.dialog.as_ref().map(|d| d.confirm.as_ref().map(|c| c.title.clone()).unwrap_or_else(|| d.title.clone()));
    assert_eq!(title.as_deref(), Some("End Team mode for a?"));
    r.rt.update(|s, _| s.feats.dialog = None);
    r.settle();
    // Off: lead_start, straight to the daemon (none here: the call's own
    // words, "Team mode for b", say it was made).
    r.rt.update(|s, _| {
        s.notice = None;
        s.link = None;
    });
    tap(&mut r, "pane-2-name");
    assert!(r.rt.state.feats.dialog.is_none());
    let notice = r.rt.state.notice.as_ref().map(|n| n.0.clone()).unwrap_or_default();
    assert!(notice.contains("Team mode for b"), "{notice}");
    // A thread's name is plain text: pressing it starts nothing.
    r.rt.update(|s, _| s.notice = None);
    tap(&mut r, "pane-3-name");
    assert!(r.rt.state.feats.dialog.is_none() && r.rt.state.notice.is_none());
}

#[test]
fn the_bar_reads_by_group_left_to_right() {
    let mut r = rig(1284.0);
    set(&mut r, serde_json::json!(["a"]));
    for n in 1..=4 {
        let f = r.rt.state.pane_fit(n - 1).unwrap();
        let mut keys = vec![format!("pane-{n}-name"), format!("pane-{n}-mode-switch")];
        keys.extend(f.shown.iter().map(|it| it.key(n)));
        keys.push(format!("pane-{n}-more"));
        let order = drawn_order(&r, &keys);
        assert_eq!(order, keys.iter().filter(|k| r.rt.presented(k).is_some()).cloned().collect::<Vec<_>>(), "pane {n}");
        assert!(f.shown.windows(2).all(|w| bar::place(&w[0].id) <= bar::place(&w[1].id)), "pane {n}: {:?}", f.shown.iter().map(|i| &i.id).collect::<Vec<_>>());
    }
}
