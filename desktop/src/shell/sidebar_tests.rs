//! The Sandboxes list's order and its session blocks (sandboxes.luau, "The
//! order"): active sandboxes first in three tiers, at both levels; the order
//! holds under the pointer and while a menu is open; the tinted block of a
//! sandbox with several sessions, the one card every sandbox has. Fixture: fixtures/shell/order.json
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
    /// A push of `d`: the core's tiers computed again from it (screen_data's
    /// `app.tiers`), as each push does.
    fn draw(&mut self, d: &J) -> Node {
        let mut d = d.clone();
        d["app"]["tiers"] = crate::scope::tiers_json(&d);
        self.s.set_data_shown(Rc::new(d), true);
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
fn several_sessions_get_a_tinted_card_and_one_the_plain_one() {
    let mut a = app();
    // The first push learns which sandboxes draw a block (their own colour).
    a.push_data();
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
    // Ungrouped with no workspace: a hue of its own, distinct from every group's.
    let own = &groups["fjord"];
    assert_eq!(fill(&n, "sb-fjord"), Some(hex(&look[own.as_str().unwrap()]["tint"])));
    let hue = |c: u32| {
        let (r, g, b) = (((c >> 16) & 255) as f32, ((c >> 8) & 255) as f32, (c & 255) as f32);
        let (mx, mn) = (r.max(g).max(b), r.min(g).min(b));
        let dd = mx - mn;
        if dd == 0.0 { 0.0 } else if mx == r { 60.0 * (((g - b) / dd).rem_euclid(6.0)) } else if mx == g { 60.0 * ((b - r) / dd + 2.0) } else { 60.0 * ((r - g) / dd + 4.0) }
    };
    for (n, c) in groups.as_object().unwrap().iter().filter(|(n, _)| *n != "fjord") {
        let dist = (hue(hex(own)) - hue(hex(c))).rem_euclid(360.0);
        assert!(dist.min(360.0 - dist) >= 39.0, "fjord's hue near {n}'s: {own} {c}");
    }
    // Only a sandbox that draws a block takes a hue (cobalt, aurora: none).
    for k in ["cobalt", "aurora", "birch", "delta", "granite"] {
        assert!(groups[k].is_null(), "{k} has no colour");
    }
    // In a group: no second block, a bar in the group's colour beside its rows.
    assert_eq!(fill(&n, "sb-atlas-web"), None);
    let bar = find(&n, "sb-bar-atlas-web").expect("the bar");
    assert_eq!(bar.style.fill, Some(hex(&groups["atlas-web"])));
    assert!(find(&n, "sb-bar-atlas-api").is_none() && find(&n, "sb-bar-ivy").is_none());
    // One session (ember), none (cobalt), stopped (aurora): the plain card
    // every sandbox out of a group has; in a group, the group's block.
    let alt = Some(crate::theme::color_value("surface-alt").unwrap());
    for k in ["sb-ember", "sb-cobalt", "sb-aurora"] {
        assert_eq!(fill(&n, k), alt, "{k}");
    }
    assert_eq!(fill(&n, "sb-atlas-api"), None);
    // The row's text does not move when the block comes or goes: the same
    // room either way (the stack's 4 pt sides are always there).
    let px = |n: &Node, k: &str| find(n, k).unwrap().style.px;
    assert_eq!(px(&n, "sb-ivy"), px(&n, "sb-ember"));
    assert_eq!(px(&n, "sb-atlas-web"), px(&n, "sb-atlas-api"));
    // In a group the bar's slot is there for every sandbox, drawn or not.
    assert_eq!(find(&n, "sb-row-atlas-api").unwrap().children.len(), 2);
    // Down to one session: the block goes.
    let rows_of = d["app"]["extras"]["rows"]["ivy"].as_array().unwrap()[..1].to_vec();
    d["app"]["extras"]["rows"]["ivy"] = J::Array(rows_of);
    let n = l.draw(&d);
    assert_eq!(fill(&n, "sb-ivy"), alt);
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
/// and under Leads only, the same.
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
        // Leads only on: the list is the same.
        rt.update(|s, tx| {
            s.intents("test", crate::intent::parse(&serde_json::json!({ "kind": "intent", "name": "team_leads", "args": { "show": "show" } })).unwrap());
            s.poll(0.0, tx);
        });
        for _ in 0..10 {
            rt.frame(1.0 / 60.0);
        }
        audit(&rt, w, ORDER.len(), "Leads only");
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
    // No status dot before a name (the state word says it); every name
    // starts at the same distance from its row's edge.
    let mut name_x: Option<f32> = None;
    for n in &names {
        let Some((r, _)) = rt.presented(&format!("row-{n}")) else { continue };
        assert!(r[0] >= list[0] - 0.5 && r[0] + r[2] <= list[0] + list[2] + 0.5, "{what} at {w}: row-{n} {r:?} out of the sidebar {list:?}");
        for o in &seen {
            assert!(r[1] >= o[1] + o[3] - 0.5 || r[1] + r[3] <= o[1] + 0.5, "{what} at {w}: row-{n} {r:?} overlaps {o:?}");
        }
        seen.push(r);
        assert!(rt.presented(&format!("dot-{n}")).is_none(), "{what} at {w}: a status dot before {n}");
        if let Some((t, _)) = rt.presented(&format!("name-{n}")) {
            let dx = t[0] - r[0];
            let first = *name_x.get_or_insert(dx);
            assert!((dx - first).abs() < 0.5, "{what} at {w}: {n}'s name starts {dx} pt in, the first row's {first}");
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



/// One colour everywhere: a sandbox with no workspace that draws a session
/// block has its hue in the list's block, its pane header and the scope
/// capsule (all from `App::group_colors` / `app.groups`); with one session
/// left, the hue goes from all of them at once.
#[test]
fn a_sandboxs_own_colour_is_the_same_everywhere_and_goes_with_its_block() {
    let open = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/shell/order-open.json");
    let mut a = App::new(Args { fixture: Some(open.into()), ..Args::default() });
    a.push_data();
    let d = a.screen_data();
    let c = a.group_colors()["fjord"];
    let pane = a.panes.iter().position(|p| p.sandbox.as_deref() == Some("fjord")).expect("fjord in a pane");
    assert_eq!(a.pane_group(pane), Some(c), "its pane header");
    assert_eq!(d["app"]["groups"]["fjord"], format!("#{c:06x}"), "the capsule, Sessions and the Board read app.groups");
    let n = List::new(&d).draw(&d);
    let (tint, _, _) = crate::theme::group_tints(c);
    assert_eq!(find(&n, "sb-fjord").unwrap().style.fill, Some(tint), "its block");
    // One session left: no block, no colour, anywhere.
    let mut rows = d["app"]["extras"]["rows"].clone();
    rows["fjord"] = J::Array(rows["fjord"].as_array().unwrap()[..1].to_vec());
    a.own_colors = a.own_hues(&rows);
    assert!(a.group_colors().get("fjord").is_none());
    assert_eq!(a.pane_group(pane), None);
}

/// A copy of the order fixture where `aurora` (stopped) has another name.
fn fixture_renamed(to: &str) -> String {
    let text = std::fs::read_to_string(FIXTURE).unwrap().replace("\"aurora\"", &format!("\"{to}\""));
    let path = std::env::temp_dir().join(format!("sbxw-remove-stopped-{}-{}.json", std::process::id(), to.len()));
    std::fs::write(&path, text).unwrap();
    path.to_string_lossy().into_owned()
}

/// A stopped row has a Remove button; no other row has one.
#[test]
fn only_stopped_rows_have_a_remove_button() {
    let a = app();
    let d = a.screen_data();
    let n = List::new(&d).draw(&d);
    let (mut stopped, mut other) = (0, 0);
    for sb in d["sandboxes"].as_array().unwrap() {
        let name = sb["name"].as_str().unwrap();
        let has = find(&n, &format!("remove-{name}")).is_some();
        if sb["status"] == "stopped" {
            assert!(has, "{name} is stopped");
            stopped += 1;
        } else {
            assert!(!has, "{name} is not stopped");
            other += 1;
        }
    }
    assert!(stopped > 0 && other > 0, "the fixture has both ({stopped}, {other})");
}

/// One press raises the removal intent for that sandbox: the core's
/// Confirm names it, and nothing runs until Confirm is pressed.
#[test]
fn a_press_asks_for_the_confirmation_and_removes_nothing() {
    use super::tests::tap;
    let mut rt = rune::Runtime::new(app(), view, (1284.0, 816.0), rune::Env::new(true, 1.0));
    for _ in 0..30 {
        rt.update(|s, tx| {
            s.poll(0.0, tx);
        });
        rt.frame(1.0 / 60.0);
    }
    assert!(rt.state.feats.dialog.is_none());
    tap(&mut rt, "remove-atlas-ops");
    for _ in 0..6 {
        rt.update(|s, tx| {
            s.poll(0.0, tx);
        });
        rt.frame(1.0 / 60.0);
    }
    let d = rt.state.feats.dialog.as_ref().expect("the core's confirmation opened");
    assert_eq!(d.title, "Remove atlas-ops?");
    assert!(d.running.is_none(), "nothing runs on the first press");
    assert!(rt.state.screen_data()["sandboxes"].as_array().unwrap().iter().any(|s| s["name"] == "atlas-ops"), "atlas-ops is still listed");
}

/// A 60-character name ends in an ellipsis; the buttons stay inside the
/// row and the row inside the sidebar, at the narrowest width and the default.
#[test]
fn a_long_name_never_pushes_the_remove_button_out() {
    let long = "a".repeat(60);
    let fx = fixture_renamed(&long);
    for w in [200.0_f32, crate::theme::layout::SIDEBAR_W] {
        let mut a = App::new(Args { fixture: Some(fx.clone()), ..Args::default() });
        a.sidebar_live = Some(w);
        let mut rt = rune::Runtime::new(a, view, (1284.0, 816.0), rune::Env::new(true, 1.0));
        for _ in 0..30 {
            rt.update(|s, tx| {
                s.poll(0.0, tx);
            });
            rt.frame(1.0 / 60.0);
        }
        let (list, _) = rt.presented("sidebar").expect("the sidebar drawn");
        let (row, _) = rt.presented(&format!("row-{long}")).expect("the long row drawn");
        let (rm, _) = rt.presented(&format!("remove-{long}")).unwrap_or_else(|| panic!("remove button drawn at {w}"));
        assert!(row[0] >= list[0] - 0.5 && row[0] + row[2] <= list[0] + list[2] + 0.5, "row {row:?} in sidebar {list:?} at {w}");
        assert!(rm[0] >= row[0] - 0.5 && rm[0] + rm[2] <= row[0] + row[2] + 0.5, "button {rm:?} in row {row:?} at {w}");
        assert!(rm[2] > 8.0, "the button keeps its size: {rm:?}");
    }
    let _ = std::fs::remove_file(fx);
}

/// Twenty sandboxes, stopped ones included, draw within the Luau budgets.
#[test]
fn twenty_sandboxes_draw_within_the_budgets() {
    let a = app();
    let mut d = a.screen_data();
    let mut extra = vec![];
    for i in 0..20 - d["sandboxes"].as_array().unwrap().len().min(20) {
        let mut sb = d["sandboxes"].as_array().unwrap().iter().find(|s| s["status"] == "stopped").unwrap().clone();
        sb["name"] = format!("filler-{i}").into();
        extra.push(sb);
    }
    d["sandboxes"].as_array_mut().unwrap().extend(extra);
    assert!(d["sandboxes"].as_array().unwrap().len() >= 20);
    let n = List::new(&d).draw(&d);
    let mut r = vec![];
    keys(&n, "remove-", &mut r);
    assert!(r.len() >= 10, "{}", r.len());
}

/// One card recipe for every sandbox (the person's screenshot: a lone
/// stopped sandbox last in the list drew no card). A team leader (fjord,
/// three sessions), lone running sandboxes, a group of stopped ones and a
/// lone stopped one last: every sandbox's card, its own stack out of a
/// group, its group's block in one, has the same x and width and a fill.
#[test]
fn every_sandbox_card_has_the_same_width_and_a_fill() {
    let mut a = app();
    a.feats.data.insert("leads".into(), serde_json::json!(["fjord"]));
    let mut rt = rune::Runtime::new(a, view, (1284.0, 2400.0), rune::Env::new(true, 1.0));
    for _ in 0..30 {
        rt.update(|s, tx| {
            s.poll(0.0, tx);
        });
        rt.frame(1.0 / 60.0);
    }
    let side = rt.state.screens.iter().find(|s| s.name == "sandboxes").unwrap();
    let n = side.last_good.clone().unwrap();
    let list = find_list(&n).expect("the list");
    let mut cards: Vec<(String, String)> = vec![];
    for b in &list.children {
        let k = key(b).unwrap_or_default();
        if k.starts_with("block-ungrouped") {
            for c in &b.children {
                let k = key(c).unwrap();
                cards.push((k[3..].to_string(), k));
            }
        } else {
            let mut v = vec![];
            keys(b, "row-", &mut v);
            cards.extend(v.into_iter().map(|r| (r[4..].to_string(), k.clone())));
        }
    }
    let names: Vec<&str> = cards.iter().map(|c| c.0.as_str()).collect();
    for want in ["fjord", "cobalt", "ember", "atlas-api"] {
        assert!(names.contains(&want), "{want} in {names:?}");
    }
    let (last, last_card) = cards.last().unwrap();
    assert!(last_card.starts_with("sb-"), "the last one is out of a group: {last_card}");
    let sbs = rt.state.screen_data()["sandboxes"].clone();
    let status = |n: &str| sbs.as_array().unwrap().iter().find(|s| s["name"] == n).map(|s| s["status"].clone()).unwrap();
    assert_eq!(status(last), "stopped", "{last} last");
    assert_eq!(status("atlas-api"), "stopped");
    let mut first: Option<[f32; 4]> = None;
    for (sb, card) in &cards {
        let node = find(&n, card).unwrap_or_else(|| panic!("{card}"));
        assert!(node.style.fill.is_some(), "{sb}'s card {card} has no fill");
        let (r, _) = rt.presented(card).unwrap_or_else(|| panic!("{card} drawn"));
        let f = *first.get_or_insert(r);
        assert!((r[0] - f[0]).abs() < 0.5 && (r[2] - f[2]).abs() < 0.5, "{sb}'s card {card} {r:?}, the first {f:?}");
    }
    // The same padding and radius on every card out of a group.
    let solo: Vec<&Node> = cards.iter().filter(|c| c.1.starts_with("sb-")).map(|c| find(&n, &c.1).unwrap()).collect();
    for c in &solo {
        assert_eq!((c.style.px, c.style.py, c.style.radius), (solo[0].style.px, solo[0].style.py, solo[0].style.radius), "{:?}", key(c));
    }
}
