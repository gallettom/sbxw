//! The Sandboxes list's order and its session blocks (sandboxes.luau, "The
//! order"): active sandboxes first in three tiers, at both levels; the order
//! holds under the pointer and while a menu is open; the tinted block of a
//! sandbox with several sessions. Fixture: fixtures/shell/order.json
//! (invented names; the state file is memory only in tests).

use super::*;
use crate::luau::Node;
use crate::screen::ScreenState;
use std::rc::Rc;

const FIXTURE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/shell/order.json");

fn app() -> App {
    App::new(Args { fixture: Some(FIXTURE.into()), ..Args::default() })
}

fn key(n: &Node) -> Option<String> {
    crate::screen::key_of(n)
}

fn find<'a>(n: &'a Node, k: &str) -> Option<&'a Node> {
    if key(n).as_deref() == Some(k) {
        return Some(n);
    }
    n.children.iter().find_map(|c| find(c, k))
}

/// Every key starting with `prefix`, in the tree's order.
fn keys(n: &Node, prefix: &str, out: &mut Vec<String>) {
    if let Some(k) = key(n).filter(|k| k.starts_with(prefix)) {
        out.push(k);
    }
    for c in &n.children {
        keys(c, prefix, out);
    }
}

/// The sandboxes' names in the order the list draws their rows.
fn rows(n: &Node) -> Vec<String> {
    let mut v = vec![];
    keys(n, "row-", &mut v);
    v.into_iter().map(|k| k[4..].to_string()).collect()
}

/// The list's top-level entries: group blocks and ungrouped sandboxes.
fn tops(n: &Node) -> Vec<String> {
    let list = find_list(n).expect("the list");
    let mut out = vec![];
    for b in &list.children {
        let k = key(b).unwrap_or_default();
        if k.starts_with("block-ungrouped") {
            out.extend(b.children.iter().filter_map(key));
        } else {
            out.push(k);
        }
    }
    out
}

fn find_list(n: &Node) -> Option<&Node> {
    if n.group.is_some() {
        return Some(n);
    }
    n.children.iter().find_map(find_list)
}

/// The list drawn alone from `d`, a screen of its own (its VM keeps the
/// tiers it applied between draws).
struct List {
    s: ScreenState,
}

impl List {
    fn new(d: &J) -> List {
        let s = ScreenState::new("sandboxes", &std::env::temp_dir().join("sbxw-no-overrides"), d.clone());
        assert!(s.error.is_none(), "{:?}", s.error);
        List { s }
    }
    fn draw(&mut self, d: &J) -> Node {
        self.s.set_data_shown(Rc::new(d.clone()), true);
        assert!(self.s.error.is_none(), "{:?}", self.s.error);
        self.s.last_good.clone().unwrap()
    }
}

fn status(d: &mut J, name: &str, st: &str) {
    for sb in d["sandboxes"].as_array_mut().unwrap() {
        if sb["name"] == name {
            sb["status"] = st.into();
        }
    }
}

fn set_state(d: &mut J, name: &str, st: &str) {
    for s in d["sessions"].as_array_mut().unwrap() {
        if s["sandbox"] == name {
            s["state"] = st.into();
        }
    }
}

fn add_session(d: &mut J, name: &str) {
    d["sessions"].as_array_mut().unwrap().push(serde_json::json!({ "sandbox": name, "mode": "claude", "state": "idle", "agent": "claude" }));
}

fn end_sessions(d: &mut J, name: &str) {
    d["sessions"].as_array_mut().unwrap().retain(|s| s["sandbox"] != name);
}

/// ember's session ends (it falls to tier 2, after cobalt) or opens again,
/// in the app's own data (the fixture's `sessions`, refreshed only on events).
fn ember_session(s: &mut App, open: bool) {
    let edit = |a: &mut Vec<J>| {
        a.retain(|x| x["sandbox"] != "ember");
        if open {
            a.push(serde_json::json!({ "sandbox": "ember", "mode": "claude", "state": "working", "agent": "claude" }));
        }
    };
    if let Some(J::Array(a)) = s.feats.data.get_mut("sessions") {
        edit(a);
    }
    if let Some(a) = s.raw["sessions"].as_array_mut() {
        edit(a);
    }
    s.push_data();
}

const ORDER: &[&str] = &[
    // Tier 1: the atlas group (atlas-web works in it), then ember, fjord, ivy.
    "atlas-web", "atlas-api", "atlas-ops", "ember", "fjord", "ivy",
    // Tier 2: cobalt runs with no session.
    "cobalt",
    // Tier 3: the Chats, harbor, then the stopped ungrouped ones by name.
    "chat-ideas", "chat-notes", "harbor-a", "harbor-b", "aurora", "birch", "delta", "granite",
];

#[test]
fn active_sandboxes_come_first_at_both_levels() {
    let a = app();
    let d = a.screen_data();
    let mut l = List::new(&d);
    let n = l.draw(&d);
    assert_eq!(rows(&n), ORDER, "the three tiers");
    // Groups and ungrouped sandboxes in one order; a group ranks by its best member.
    assert_eq!(
        tops(&n),
        ["block-atlas", "sb-ember", "sb-fjord", "sb-ivy", "sb-cobalt", "block-Chats", "block-harbor", "sb-aurora", "sb-birch", "sb-delta", "sb-granite"]
    );
    // Ungrouped sandboxes between groups sit in plain blocks of their own.
    let mut blocks = vec![];
    keys(&n, "block-ungrouped", &mut blocks);
    assert_eq!(blocks, ["block-ungrouped", "block-ungrouped-2"]);
}

#[test]
fn a_state_flicker_reorders_nothing_and_slow_facts_do() {
    let a = app();
    let mut d = a.screen_data();
    let mut l = List::new(&d);
    assert_eq!(rows(&l.draw(&d)), ORDER);
    // Working, idle, wants you: no move.
    for st in ["idle", "attention", "working", "idle"] {
        set_state(&mut d, "ivy", st);
        set_state(&mut d, "atlas-web", st);
        assert_eq!(rows(&l.draw(&d)), ORDER, "{st}");
    }
    // A stop: ember goes to the stopped ones.
    status(&mut d, "ember", "stopping…");
    let r = rows(&l.draw(&d));
    assert_eq!(&r[12..15], ["delta", "ember", "granite"], "{r:?}");
    // A start: aurora up, no session yet: beside cobalt.
    status(&mut d, "aurora", "running");
    let r = rows(&l.draw(&d));
    assert_eq!(&r[5..7], ["aurora", "cobalt"], "{r:?}");
    // A session opens on cobalt: tier 1, after ivy.
    add_session(&mut d, "cobalt");
    let r = rows(&l.draw(&d));
    // (atlas, ember, then cobalt by its place in the daemon list, before fjord)
    assert!(r.iter().position(|x| x == "cobalt") < r.iter().position(|x| x == "fjord"), "{r:?}");
    assert!(r.iter().position(|x| x == "cobalt") > r.iter().position(|x| x == "atlas-ops"), "{r:?}");
    // Its session ends: back to tier 2.
    end_sessions(&mut d, "cobalt");
    let r = rows(&l.draw(&d));
    assert!(r.iter().position(|x| x == "cobalt") > r.iter().position(|x| x == "ivy"), "{r:?}");
    // A session busy on its own background work counts whatever its word.
    end_sessions(&mut d, "ivy");
    d["sessions"].as_array_mut().unwrap().push(serde_json::json!({ "sandbox": "ivy", "state": "done", "background": true }));
    let r = rows(&l.draw(&d));
    assert!(r.iter().position(|x| x == "ivy") < r.iter().position(|x| x == "cobalt"), "background counts: {r:?}");
    // The atlas group's only active member stops: the group goes down with it.
    status(&mut d, "atlas-web", "stopped");
    let n = l.draw(&d);
    let t = tops(&n);
    assert!(t.iter().position(|x| x == "block-atlas") > t.iter().position(|x| x == "sb-cobalt"), "{t:?}");
}

#[test]
fn the_order_holds_under_the_pointer_and_an_open_menu() {
    let a = app();
    let mut d = a.screen_data();
    let mut l = List::new(&d);
    assert_eq!(rows(&l.draw(&d)), ORDER);
    // Resting on a row (or a group's header): a stop does not move anything.
    for zone in ["sb:fjord", "group:atlas"] {
        d["app"]["hover_zone"] = zone.into();
        status(&mut d, "ember", "stopped");
        assert_eq!(rows(&l.draw(&d)), ORDER, "held under {zone}");
        status(&mut d, "ember", "running");
    }
    status(&mut d, "ember", "stopped");
    // Left: the move lands.
    d["app"]["hover_zone"] = J::Null;
    let r = rows(&l.draw(&d));
    assert_ne!(r, ORDER);
    assert!(r.iter().position(|x| x == "ember") > r.iter().position(|x| x == "cobalt"), "{r:?}");
    // A menu open: held, then applied when it shuts.
    d["app"]["sidebar_menu"] = true.into();
    let held = rows(&l.draw(&d));
    status(&mut d, "ember", "running");
    assert_eq!(rows(&l.draw(&d)), held, "held while a menu is open");
    d["app"]["sidebar_menu"] = false.into();
    assert_eq!(rows(&l.draw(&d)), ORDER, "applied when it shuts");
    // A new sandbox takes its place at once, even under the pointer.
    d["app"]["hover_zone"] = "sb:fjord".into();
    d["sandboxes"].as_array_mut().unwrap().push(serde_json::json!({ "name": "alder", "agent": "claude", "status": "stopped" }));
    let r = rows(&l.draw(&d));
    assert_eq!(r.last().map(String::as_str), Some("alder"), "the daemon's order: {r:?}");
    assert!(r.iter().any(|x| x == "alder"));
}

#[test]
fn several_sessions_get_a_block_and_one_keeps_todays_look() {
    let a = app();
    let mut d = a.screen_data();
    let groups = d["app"]["groups"].clone();
    let look = d["app"]["group_look"].clone();
    let hex = |s: &J| u32::from_str_radix(s.as_str().unwrap().trim_start_matches('#'), 16).unwrap();
    let mut l = List::new(&d);
    let n = l.draw(&d);
    let fill = |n: &Node, k: &str| find(n, k).unwrap_or_else(|| panic!("{k}")).style.fill;
    // Ungrouped, its own workspace's colour: that colour's tint (its terminal header's hue).
    let ivy = hex(&groups["ivy"]);
    assert_eq!(fill(&n, "sb-ivy"), Some(hex(&look[groups["ivy"].as_str().unwrap()]["tint"])), "ivy's block, colour {ivy:06x}");
    // Ungrouped with no workspace: the neutral block.
    assert_eq!(fill(&n, "sb-fjord"), crate::theme::color_value("surface-alt"));
    // In a group: the lighter treatment (the group's tint a step stronger), not a second block.
    let atlas = &groups["atlas-web"];
    assert_eq!(fill(&n, "sb-atlas-web"), Some(hex(&look[atlas.as_str().unwrap()]["inner"])));
    assert_ne!(fill(&n, "sb-atlas-web"), fill(&n, "block-atlas"));
    // One session (ember), none (cobalt), stopped (aurora): no block.
    for k in ["sb-ember", "sb-cobalt", "sb-aurora", "sb-atlas-api"] {
        assert_eq!(fill(&n, k), None, "{k}");
    }
    // The row's text does not move when the block comes or goes: the same
    // room either way (the stack's 4 pt sides are always there).
    let px = |n: &Node, k: &str| find(n, k).unwrap().style.px;
    assert_eq!(px(&n, "sb-ivy"), px(&n, "sb-ember"));
    assert_eq!(px(&n, "sb-atlas-web"), px(&n, "sb-atlas-api"));
    // Down to one session: the block goes.
    let rows_of = d["app"]["extras"]["rows"]["ivy"].as_array().unwrap()[..1].to_vec();
    d["app"]["extras"]["rows"]["ivy"] = J::Array(rows_of);
    let n = l.draw(&d);
    assert_eq!(fill(&n, "sb-ivy"), None);
}

/// The list in the window: the order holds while the pointer rests on a row
/// and lands after it leaves; the keyboard stays on the sandbox that moved;
/// a menu open holds it too (the core tells the list).
#[test]
fn in_the_window_the_order_waits_for_the_pointer_and_the_focus_follows() {
    use super::tests::tap;
    let mut rt = rune::Runtime::new(app(), view, (1284.0, 816.0), rune::Env::new(true, 1.0));
    let settle = |rt: &mut rune::Runtime<App>| {
        for _ in 0..40 {
            rt.update(|s, tx| {
                s.poll(0.0, tx);
            });
            rt.frame(1.0 / 60.0);
        }
    };
    settle(&mut rt);
    let y = |rt: &rune::Runtime<App>, k: &str| rt.presented(k).map(|(r, _)| r[1]).unwrap_or(-1.0);
    let ember0 = y(&rt, "row-ember");
    assert!(ember0 < y(&rt, "row-aurora"), "ember first");
    // The keyboard on ember's row, ember selected.
    rt.update(|s, tx| {
        s.selected = Some("ember".into());
        tx.focus("row-ember".to_string(), true);
        s.push_data();
    });
    settle(&mut rt);
    // The pointer rests on fjord's row.
    let (r, _) = rt.presented("row-fjord").unwrap();
    let (x, yy) = (r[0] + r[2] / 2.0, r[1] + r[3] / 2.0);
    rt.pointer_move(x, yy);
    rt.frame(1.0 / 60.0);
    rt.pointer_move(x + 3.0, yy);
    crate::screen::advance_zone_clock(crate::screen::ZONE_OPEN + std::time::Duration::from_millis(40));
    settle(&mut rt);
    let fjord0 = y(&rt, "row-fjord");
    // ember stops: nothing moves under the pointer.
    rt.update(|s, _| ember_session(s, false));
    settle(&mut rt);
    assert!((y(&rt, "row-fjord") - fjord0).abs() < 0.5, "held under the pointer");
    assert!((y(&rt, "row-ember") - ember0).abs() < 0.5, "ember stays while hovered");
    // The pointer leaves for the panes: the move lands after the close delay.
    let (p, _) = rt.presented("panes").unwrap();
    rt.pointer_move(p[0] + p[2] / 2.0, p[1] + p[3] / 2.0);
    settle(&mut rt);
    crate::screen::advance_zone_clock(crate::screen::ZONE_CLOSE + std::time::Duration::from_millis(40));
    settle(&mut rt);
    for _ in 0..40 {
        rt.frame(1.0 / 60.0);
    }
    assert!(y(&rt, "row-ember") > y(&rt, "row-cobalt"), "ember went down after cobalt");
    // The keyboard and the selection stayed on ember.
    let focused = std::rc::Rc::new(std::cell::RefCell::new(None::<String>));
    let f2 = focused.clone();
    rt.update(move |_, tx| *f2.borrow_mut() = tx.focused().map(str::to_string));
    assert_eq!(focused.borrow().as_deref(), Some("row-ember"));
    let side = rt.state.screens.iter().find(|s| s.name == "sandboxes").unwrap();
    assert!(find(side.last_good.as_ref().unwrap(), "row-ember").unwrap().selected);
    // A menu open (a row's ⋯): the core tells the list, which holds its order.
    rt.update(|s, tx| {
        s.screens.iter_mut().find(|s| s.name == "sandboxes").unwrap().ui.open = Some("extra-sandbox-ember".into());
        s.poll(0.0, tx);
    });
    assert_eq!(rt.state.screen_data()["app"]["sidebar_menu"], true);
    let before = y(&rt, "row-ember");
    rt.update(|s, _| ember_session(s, true));
    settle(&mut rt);
    assert!((y(&rt, "row-ember") - before).abs() < 0.5, "held while the menu is open");
    tap(&mut rt, "extra-sandbox-ember-scrim");
    settle(&mut rt);
    for _ in 0..40 {
        rt.frame(1.0 / 60.0);
    }
    assert!(y(&rt, "row-ember") < y(&rt, "row-cobalt"), "lands when the menu shuts");
}

/// The list at the sidebar's narrowest (200 pt) and its default width, with
/// 15 sandboxes: rows never overlap, stay inside the sidebar and scroll in
/// it; the status dots keep their size; hidden and shown again, filtered,
/// and under Lead Mode, the same.
#[test]
fn the_list_fits_at_200_pt_and_at_the_default_width() {
    use super::tests::tap;
    for w in [200.0_f32, crate::theme::layout::SIDEBAR_W] {
        let mut a = app();
        a.sidebar_live = Some(w);
        let mut rt = rune::Runtime::new(a, view, (1284.0, 816.0), rune::Env::new(true, 1.0));
        for _ in 0..30 {
            rt.update(|s, tx| {
                s.poll(0.0, tx);
            });
            rt.frame(1.0 / 60.0);
        }
        audit(&rt, w, ORDER.len(), "shown");
        // Hidden and shown again.
        rt.update(|s, _| s.set_sidebar(false));
        for _ in 0..20 {
            rt.frame(1.0 / 60.0);
        }
        assert!(rt.presented("sidebar").is_none(), "hidden");
        rt.update(|s, _| s.set_sidebar(true));
        for _ in 0..10 {
            rt.frame(1.0 / 60.0);
        }
        audit(&rt, w, ORDER.len(), "shown again");
        // Lead Mode on: the list is the same.
        rt.update(|s, tx| {
            s.intents("test", crate::intent::parse(&serde_json::json!({ "kind": "intent", "name": "team_leads", "args": { "show": "show" } })).unwrap());
            s.poll(0.0, tx);
        });
        for _ in 0..10 {
            rt.frame(1.0 / 60.0);
        }
        audit(&rt, w, ORDER.len(), "Lead Mode");
        // The name filter: only atlas's rows, in order.
        tap(&mut rt, "filter");
        for ch in "atlas".chars() {
            rt.key(KeyEvent { key: Key::Text(ch.to_string()), mods: rune::Mods::default() });
            rt.frame(1.0 / 60.0);
        }
        for _ in 0..10 {
            rt.frame(1.0 / 60.0);
        }
        audit(&rt, w, 3, "filtered");
        let side = rt.state.screens.iter().find(|s| s.name == "sandboxes").unwrap();
        assert_eq!(rows(side.last_good.as_ref().unwrap()), ["atlas-web", "atlas-api", "atlas-ops"]);
    }
}

fn audit(rt: &rune::Runtime<App>, w: f32, count: usize, what: &str) {
    let side = rt.state.screens.iter().find(|s| s.name == "sandboxes").unwrap();
    let names = rows(side.last_good.as_ref().unwrap());
    assert_eq!(names.len(), count, "{what}: {names:?}");
    let (list, _) = rt.presented("sidebar").expect("the sidebar drawn");
    let mut seen: Vec<[f32; 4]> = vec![];
    for n in &names {
        let Some((r, _)) = rt.presented(&format!("row-{n}")) else { continue };
        assert!(r[0] >= list[0] - 0.5 && r[0] + r[2] <= list[0] + list[2] + 0.5, "{what} at {w}: row-{n} {r:?} out of the sidebar {list:?}");
        for o in &seen {
            assert!(r[1] >= o[1] + o[3] - 0.5 || r[1] + r[3] <= o[1] + 0.5, "{what} at {w}: row-{n} {r:?} overlaps {o:?}");
        }
        seen.push(r);
        if let Some((dot, _)) = rt.presented(&format!("dot-{n}")) {
            assert!((dot[2] - 12.0).abs() < 0.5, "{what} at {w}: dot-{n} {dot:?}");
        }
    }
    // Fifteen rows are taller than the window: the list scrolls inside the sidebar.
    if count == ORDER.len() {
        assert!(seen.len() < count || seen.iter().any(|r| r[1] + r[3] > list[1] + list[3]), "{what}: the list is taller than the sidebar");
    }
}

/// The hold always ends, so a pending move never stays stuck: the pointer
/// leaving the window, a menu shut by Escape or by a press outside it, and
/// the sidebar hidden (its rows gone) each release it.
#[test]
fn the_hold_ends_when_the_pointer_leaves_a_menu_shuts_or_the_sidebar_hides() {
    use super::tests::tap;
    let settle = |rt: &mut rune::Runtime<App>| {
        for _ in 0..40 {
            rt.update(|s, tx| {
                s.poll(0.0, tx);
            });
            rt.frame(1.0 / 60.0);
        }
    };
    let rows_now = |rt: &rune::Runtime<App>| rows(rt.state.screens.iter().find(|s| s.name == "sandboxes").unwrap().last_good.as_ref().unwrap());
    let stop_ember = |rt: &mut rune::Runtime<App>, _: &str| rt.update(|s, _| ember_session(s, false));
    let ember_at = |rt: &rune::Runtime<App>| rows_now(rt).iter().position(|x| x == "ember").unwrap();
    let hover = |rt: &mut rune::Runtime<App>, k: &str| {
        let (r, _) = rt.presented(k).unwrap();
        let (x, y) = (r[0] + r[2] / 2.0, r[1] + r[3] / 2.0);
        rt.pointer_move(x, y);
        rt.frame(1.0 / 60.0);
        rt.pointer_move(x + 3.0, y);
        crate::screen::advance_zone_clock(crate::screen::ZONE_OPEN + std::time::Duration::from_millis(40));
    };
    let release = || crate::screen::advance_zone_clock(crate::screen::ZONE_CLOSE + std::time::Duration::from_millis(40));
    // The row's ⋯, pressed as the person does.
    let open_menu = |rt: &mut rune::Runtime<App>| {
        tap(rt, "extra-sandbox-ember");
        assert!(rt.state.drawn("extra-sandbox-ember-list"), "the menu opened");
    };
    for case in ["pointer leaves the window", "Escape", "a press outside", "sidebar hidden"] {
        let mut rt = rune::Runtime::new(app(), view, (1284.0, 816.0), rune::Env::new(true, 1.0));
        settle(&mut rt);
        let first = ember_at(&rt);
        match case {
            "pointer leaves the window" => hover(&mut rt, "row-fjord"),
            "sidebar hidden" => hover(&mut rt, "row-fjord"),
            _ => open_menu(&mut rt),
        }
        settle(&mut rt);
        stop_ember(&mut rt, "stopped");
        settle(&mut rt);
        assert_eq!(ember_at(&rt), first, "{case}: held");
        match case {
            "pointer leaves the window" => rt.pointer_leave(),
            "Escape" => rt.key(KeyEvent { key: Key::Escape, mods: rune::Mods::default() }),
            "a press outside" => tap(&mut rt, "extra-sandbox-ember-scrim"),
            _ => rt.update(|s, _| s.set_sidebar(false)),
        }
        // The leave lands on the clock as it is, then the close delay runs out.
        settle(&mut rt);
        release();
        settle(&mut rt);
        if case == "sidebar hidden" {
            rt.update(|s, _| s.set_sidebar(true));
            settle(&mut rt);
        }
        assert!(ember_at(&rt) > first, "{case}: released, ember moved down: {:?}", rows_now(&rt));
        assert_eq!(rt.state.screen_data()["app"]["sidebar_menu"], false, "{case}");
    }
}


