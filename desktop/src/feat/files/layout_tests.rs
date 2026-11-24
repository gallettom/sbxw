//! The explorer's layout around uploads and saves: the upload block at the
//! foot of the Locations (no card across the top), Choose and Paste firing
//! the same intents, the saved toast at the bottom centre of the explorer
//! (a border, the FLOAT shadow), its expiry held by hover, a failure kept
//! until closed, and the narrow layouts.

use super::narrow_tests::{scene, violations, Case};
use super::scroll_tests::settle;
use super::*;
use crate::shell::App;
use rune::Runtime;
use std::time::Duration;

fn case(w: f32, h: f32, reader: bool) -> Case {
    Case { w, h, sidebar: false, reader, extra: 0, long: false, ratio: None, deep: false, offline: false }
}

fn rect(rt: &Runtime<App>, key: &str) -> [f32; 4] {
    rt.presented(key).unwrap_or_else(|| panic!("{key} not drawn")).0
}

fn drawn(rt: &Runtime<App>, key: &str) -> bool {
    rt.elements().into_iter().any(|e| !e.exiting && e.key.as_deref() == Some(key))
}

fn path_of(rt: &Runtime<App>, key: &str) -> String {
    rt.elements().into_iter().find(|e| !e.exiting && e.key.as_deref() == Some(key)).map(|e| e.path).unwrap_or_else(|| panic!("{key} not drawn"))
}

fn tap(rt: &mut Runtime<App>, key: &str) {
    let (x, y) = rt.find(key).unwrap_or_else(|| panic!("{key} drawn"));
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    settle(rt);
}

/// An intent as a screen's press sends it (the core then pushes the data
/// to the screens, which `run_intent` alone does not).
fn press(rt: &mut Runtime<App>, name: &str, args: J) {
    let list = crate::intent::parse(&json!({ "kind": "intent", "name": name, "args": args })).unwrap();
    rt.update(|s, _| crate::screen::Host::intents(s, "files", list));
    settle(rt);
}

/// Puts a toast in the feature's state (as a download's answer does,
/// see `a_download_answer_makes_the_toast`) and has the screen drawn again.
fn give_toast(rt: &mut Runtime<App>, error: Option<&str>) -> u64 {
    let seq = {
        let mut st = STATE().lock().unwrap();
        st.toast_seq += 1;
        st.toast = Some(view::Toast { seq: st.toast_seq, name: "graph-lookup.tar.gz".into(), path: Some("/Users/me/Downloads/graph-lookup.tar.gz".into()), error: error.map(str::to_string), expired: false });
        st.toast_seq
    };
    press(rt, "files_filter", json!({ "text": "" }));
    seq
}

/// Waits out the hover zone's delays on the test's own clock.
fn zone_wait(rt: &mut Runtime<App>, ms: u64) {
    crate::screen::advance_zone_clock(Duration::from_millis(ms));
    settle(rt);
}

/// From 900 pt: no upload card between the header and the columns; the
/// block sits in the left column under the Locations list, at its foot.
#[test]
fn the_upload_block_is_under_the_locations_not_across_the_top() {
    for (w, h) in [(1284.0, 816.0), (1000.0, 700.0), (1100.0, 480.0)] {
        let rt = scene(case(w, h, true));
        assert!(drawn(&rt, "files-left"), "{w}: three columns");
        let p = path_of(&rt, "files-drop");
        assert!(p.contains("/files-left/"), "{w}: the block is in the left column: {p}");
        let (drop, left, locs, cols) = (rect(&rt, "files-drop"), rect(&rt, "files-left"), rect(&rt, "files-locations-scroll"), rect(&rt, "files-columns"));
        assert!(drop[1] >= cols[1] - 0.5, "{w}: nothing of the upload above the columns: {drop:?} {cols:?}");
        assert!(drop[1] >= locs[1] + locs[3] - 0.5, "{w}: under the Locations list: {drop:?} {locs:?}");
        assert!((drop[1] + drop[3] - (left[1] + left[3])).abs() < 1.0, "{w}: at the column's foot: {drop:?} {left:?}");
        assert!(drop[2] <= 224.5, "{w}: fitted to the column: {drop:?}");
        for k in ["files-upload-pick", "files-upload-paste"] {
            let b = rect(&rt, k);
            assert!(b[0] >= drop[0] - 0.5 && b[0] + b[2] <= drop[0] + drop[2] + 0.5, "{w}: {k} inside the block");
        }
        let v = violations(&rt, (w, h));
        assert!(v.is_empty(), "{w}: {v:?}");
    }
}

/// The header's Upload marks the block (it never opens a card across the top).
#[test]
fn the_headers_upload_marks_the_block() {
    let mut rt = scene(case(1284.0, 816.0, false));
    let before = rect(&rt, "files-columns");
    tap(&mut rt, "files-upload");
    assert!(path_of(&rt, "files-drop").contains("/files-left/"));
    assert_eq!(rect(&rt, "files-columns")[1], before[1], "the columns did not move down");
    let n = files_node(&rt, "files-drop");
    assert_eq!(n.style.border, crate::theme::color_value("accent"), "marked");
}

fn files_node(rt: &Runtime<App>, key: &str) -> crate::luau::Node {
    fn find(n: &crate::luau::Node, k: &str) -> Option<crate::luau::Node> {
        if n.key.as_deref() == Some(k) {
            return Some(n.clone());
        }
        n.children.iter().find_map(|c| find(c, k))
    }
    let s = rt.state.screens.iter().find(|s| s.name == "files").expect("the files screen");
    s.last_good.as_ref().and_then(|n| find(n, key)).unwrap_or_else(|| panic!("{key} not in the tree"))
}

/// Choose… and Paste in the block fire the intents they always did.
#[test]
fn choose_and_paste_fire_the_same_intents() {
    let mut h = crate::luau::LuauHost::new(crate::luau::Limits::default()).unwrap();
    h.load("files", crate::screen::builtin("files").unwrap()).unwrap();
    for (action, name) in [("pick", "files_upload_pick"), ("paste", "files_upload_paste")] {
        let out = h.act(action, &J::Null, &J::Null).unwrap();
        assert_eq!((out["kind"].as_str(), out["name"].as_str()), (Some("intent"), Some(name)), "{action}: {out}");
    }
    let rt = scene(case(1284.0, 816.0, false));
    assert_eq!(files_node(&rt, "files-upload-pick").on.as_deref(), Some("pick"));
    assert_eq!(files_node(&rt, "files-upload-paste").on.as_deref(), Some("paste"));
}

/// A download's answer makes the saved toast (and its expiry is for that
/// toast only); a failure makes one that says why and stays.
#[test]
fn a_download_answer_makes_the_toast() {
    let d = {
        let v: J = serde_json::from_str(include_str!("../../../fixtures/files/standin.json")).unwrap();
        json!({ "sandboxes": v["daemon"]["sandboxes"], "app": { "kept": {}, "errors": {} } })
    };
    let cx = Ctx { data: &d };
    let mut st = State::default();
    st.sandbox = Some("web-app".into());
    let dl = Download { get: "/api/sandboxes/web-app/artifacts/download?path=report.md".into(), name: "report.md".into(), to: Dest::Downloads, after: After::Nothing, tag: "download".into() };
    st.on_answer(&cx, &dl.call("save report.md"), Ok((200, json!({ "saved": "/D/report.md", "name": "report.md", "bytes": 1, "verdict": "plain" }))));
    let t = st.toast.clone().unwrap();
    assert_eq!((t.name.as_str(), t.path.as_deref(), t.error.as_deref(), t.expired), ("report.md", Some("/D/report.md"), None, false));
    assert_eq!(st.ui(&cx)["toast"]["name"], "report.md");
    // An expiry for an older toast does nothing; its own expires it.
    st.act(&cx, &Asked { name: "files_toast_expire", args: json!({ "seq": t.seq + 7 }).as_object().unwrap().clone() }).unwrap();
    assert!(!st.toast.as_ref().unwrap().expired);
    st.act(&cx, &Asked { name: "files_toast_expire", args: json!({ "seq": t.seq }).as_object().unwrap().clone() }).unwrap();
    assert!(st.toast.as_ref().unwrap().expired);
    // A failure: the daemon's words, kept past its expiry, closed by its button.
    st.on_answer(&cx, &dl.call("save report.md"), Ok((502, json!({ "error": "web-app is not reachable" }))));
    let f = st.toast.clone().unwrap();
    assert!(f.error.as_deref().unwrap().contains("not reachable"), "{f:?}");
    st.act(&cx, &Asked { name: "files_toast_expire", args: json!({ "seq": f.seq }).as_object().unwrap().clone() }).unwrap();
    assert!(!st.toast.as_ref().unwrap().expired, "a failure stays");
    st.act(&cx, &Asked { name: "files_toast_close", args: Default::default() }).unwrap();
    assert!(st.toast.is_none());
    // A cancelled save says nothing.
    st.on_answer(&cx, &dl.call("save report.md"), Ok((499, json!({ "cancelled": true }))));
    assert!(st.toast.is_none());
}

/// The toast: centred at the bottom of the explorer (not of the window),
/// a 1 pt border in the theme's border colour, the surface fill, the FLOAT
/// shadow; clear of the reader's Download; no click outside its rect.
#[test]
fn the_saved_toast_floats_at_the_bottom_centre() {
    for (w, h) in [(1284.0, 816.0), (860.0, 600.0), (720.0, 480.0)] {
        let mut rt = scene(case(w, h, true));
        give_toast(&mut rt, None);
        let t = rect(&rt, "files-toast");
        let body = rect(&rt, "overlay-body-files");
        let mid = body[0] + body[2] / 2.0;
        assert!((t[0] + t[2] / 2.0 - mid).abs() < 14.0, "{w}: centred on the explorer: {t:?} in {body:?}");
        let bottom = body[1] + body[3];
        assert!(t[1] + t[3] <= bottom + 0.5 && t[1] + t[3] >= bottom - 40.0, "{w}: at its bottom: {t:?} in {body:?}");
        assert!(t[0] >= body[0] && t[0] + t[2] <= body[0] + body[2] + 0.5, "{w}: inside the explorer");
        let n = files_node(&rt, "files-toast");
        assert_eq!(n.style.border, crate::theme::color_value("border"));
        assert_eq!(n.style.fill, crate::theme::color_value("surface"));
        assert_eq!(n.style.radius, crate::theme::radius("md").ok());
        let pose = rt.presented_pose("files-toast").unwrap();
        let (y, blur) = crate::theme::elevation::FLOAT;
        assert!(pose.shadows().iter().any(|s| s.y == y && s.blur == blur), "{w}: the FLOAT shadow: {:?}", pose.shadows());
        assert!(drawn(&rt, "files-show-saved"));
        if drawn(&rt, "files-download") {
            let b = rect(&rt, "files-download");
            let apart = b[1] + b[3] <= t[1] || t[1] + t[3] <= b[1] || b[0] + b[2] <= t[0] || t[0] + t[2] <= b[0];
            assert!(apart, "{w}: the toast covers Download: {t:?} {b:?}");
        }
        // Beside it, the click goes to what is under it.
        let x = t[0] - 6.0;
        if x > body[0] {
            let hit = rt.element_at(x, t[1] + t[3] / 2.0).map(|p| format!("{p:?}")).unwrap_or_default();
            assert!(!hit.contains("files-toast"), "{w}: the toast took a click outside its rect: {hit}");
        }
    }
}

/// It goes once its time is up, unless the pointer rests on it: then it
/// goes when the pointer leaves.
#[test]
fn the_saved_toast_leaves_after_its_delay_and_stays_on_hover() {
    let mut rt = scene(case(1284.0, 816.0, true));
    let seq = give_toast(&mut rt, None);
    press(&mut rt, "files_toast_expire", json!({ "seq": seq }));
    assert!(!drawn(&rt, "files-toast"), "gone after its delay");
    // Again, the pointer resting on it this time.
    let seq = give_toast(&mut rt, None);
    let t = rect(&rt, "files-toast");
    let (x, y) = (t[0] + 20.0, t[1] + t[3] / 2.0);
    rt.pointer_move(x, y);
    rt.frame(0.02);
    rt.pointer_move(x + 2.0, y);
    zone_wait(&mut rt, 250);
    press(&mut rt, "files_toast_expire", json!({ "seq": seq }));
    assert!(drawn(&rt, "files-toast"), "held while the pointer is on it");
    rt.pointer_move(t[0] + t[2] / 2.0, t[1] - 200.0);
    rt.frame(0.02);
    zone_wait(&mut rt, 400);
    assert!(!drawn(&rt, "files-toast"), "gone once the pointer left");
}

/// A failure: the error in the error colour, kept past the delay, gone by its close.
#[test]
fn a_failed_download_toast_stays_until_closed() {
    let mut rt = scene(case(1284.0, 816.0, true));
    let seq = give_toast(&mut rt, Some("web-app is not reachable"));
    assert_eq!(files_node(&rt, "files-toast-error").style.color, crate::theme::color_value("red"));
    press(&mut rt, "files_toast_expire", json!({ "seq": seq }));
    assert!(drawn(&rt, "files-toast") && drawn(&rt, "files-toast-close"), "a failure stays");
    tap(&mut rt, "files-toast-close");
    assert!(!drawn(&rt, "files-toast"), "closed");
}

/// Under 900 pt there is no left column: Upload opens the block at the top
/// of the list's column (two columns, and narrow inside the scrolled tree);
/// it fits, nothing cut or overlapping, and nothing across the top.
#[test]
fn narrow_the_block_opens_in_the_lists_column() {
    for (w, h) in [(860.0, 600.0), (720.0, 480.0)] {
        let mut rt = scene(case(w, h, false));
        assert!(!drawn(&rt, "files-left"));
        assert!(!drawn(&rt, "files-drop"), "{w}: closed until asked");
        tap(&mut rt, "files-upload");
        let p = path_of(&rt, "files-drop");
        let head = rect(&rt, "files-head");
        let drop = rect(&rt, "files-drop");
        assert!(drop[1] >= head[1] + head[3], "{w}: under the header");
        if w < 760.0 {
            assert!(p.contains("files-tree-scroll"), "{w}: in the scrolled tree: {p}");
        } else {
            let bar = rect(&rt, "files-listbar");
            assert!(drop[1] + drop[3] <= bar[1] + 0.5 && drop[0] >= bar[0] - 0.5 && drop[0] + drop[2] <= bar[0] + bar[2] + 0.5, "{w}: above the list, in its column: {drop:?} {bar:?}");
        }
        let v = violations(&rt, (w, h));
        assert!(v.is_empty(), "{w}: {v:?}");
    }
}

