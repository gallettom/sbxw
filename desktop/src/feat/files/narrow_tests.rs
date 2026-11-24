//! The Files overlay on a small window: nothing is cut, overflows its
//! column or overlaps its neighbour (layout only). Each case draws the
//! overlay in the whole app, at a window size, with the sidebar open or
//! collapsed, a file open in the reader or not, and checks the laid-out
//! rect of every element against its parent's and its siblings'.

use super::scroll_tests::{intent, settle};
use crate::shell::{view, App, Args};
use rune::Runtime;
use serde_json::{json, Value as J};

const EPS: f32 = 0.6;
const WEB: &str = "/Users/me/web";

#[derive(Clone, Copy, Debug)]
pub(super) struct Case {
    pub w: f32,
    pub h: f32,
    pub sidebar: bool,
    pub reader: bool,
    /// 0: the fixture's own rows; else this many rows more.
    pub extra: usize,
    pub long: bool,
    /// The split's ratio as the person dragged it (None: as drawn at first).
    pub ratio: Option<f32>,
    /// Browse a deep folder whose segments include a name of 120 characters.
    pub deep: bool,
    /// The sandbox is stopped (the badge beside the scope).
    pub offline: bool,
}

pub(super) const SIZES: [(f32, f32); 5] = [(1284.0, 816.0), (1000.0, 700.0), (860.0, 600.0), (720.0, 480.0), (900.0, 480.0)];

/// A name of 120 characters without a space.
pub(super) fn long_name(n: usize) -> String {
    format!("{n:03}-{}.md", "x".repeat(112))
}

fn fixture(c: &Case) -> String {
    let mut v: J = serde_json::from_str(include_str!("../../../fixtures/files/standin.json")).unwrap();
    let entries = v["daemon"]["files_tree"][WEB]["entries"].as_array_mut().unwrap();
    if c.long {
        for i in 0..3 {
            entries.push(json!({ "name": long_name(i), "kind": if i == 0 { "dir" } else { "file" }, "link": false, "size": 123456789, "modified": 1794199880 }));
        }
    }
    for i in 0..c.extra {
        entries.push(json!({ "name": format!("file-{i:04}.txt"), "kind": "file", "link": false, "size": 1000 + i, "modified": 1794199880 }));
    }
    v["daemon"]["files_dir"]["offline"] = c.offline.into();
    let deep = deep_path();
    let mut d = v["daemon"]["files_tree"][WEB].clone();
    d["path"] = deep.clone().into();
    d["parent"] = deep.rsplit_once('/').unwrap().0.into();
    v["daemon"]["files_tree"][&deep] = d;
    let long_md = format!("{WEB}/{}", long_name(1));
    v["daemon"]["files_content"][&long_md] = "# A heading that is long enough to need the outline\n\n## Second\n\ntext\n\n## Third\n\ntext\n".into();
    let p = std::env::temp_dir().join(format!("sbxw-files-narrow-{}-{}-{}.json", std::process::id(), c.extra, c.long));
    std::fs::write(&p, serde_json::to_string(&v).unwrap()).unwrap();
    p.to_string_lossy().to_string()
}

fn deep_path() -> String {
    format!("{WEB}/alpha/{}/gamma/delta/epsilon/zeta/eta", long_name(7))
}

pub(super) fn scene(c: Case) -> Runtime<App> {
    let a = App::new(Args { fixture: Some(fixture(&c)), ..Args::default() });
    let mut rt = Runtime::new(a, view, (c.w, c.h), rune::Env::new(true, 1.0));
    settle(&mut rt);
    rt.update(|s, _| s.set_sidebar(c.sidebar));
    intent(&mut rt, "files_open", json!({ "sandbox": "web-app" }));
    intent(&mut rt, "files_browse", json!({ "path": WEB }));
    if c.deep {
        intent(&mut rt, "files_browse", json!({ "path": deep_path() }));
    }
    if c.reader {
        let id = if c.deep {
            format!("{}/README.md", deep_path())
        } else if c.long { format!("{WEB}/{}", long_name(1)) } else { format!("{WEB}/README.md") };
        intent(&mut rt, "files_select", json!({ "id": id }));
    }
    if let Some(r) = c.ratio {
        use crate::screen::Host;
        rt.update(|s, _| { s.screen("files").ui.splits.insert("files-split".into(), (r, 600.0)); });
    }
    settle(&mut rt);
    rt
}

struct Node {
    path: String,
    key: Option<String>,
    rect: [f32; 4],
    text: Option<String>,
}

fn nodes(rt: &Runtime<App>) -> Vec<Node> {
    rt.elements().into_iter().filter(|e| !e.exiting).map(|e| Node { path: e.path, key: e.key, rect: e.rect, text: e.text }).collect()
}

fn parent_of(path: &str) -> Option<&str> {
    path.rsplit_once('/').map(|p| p.0)
}

fn name(n: &Node) -> String {
    n.key.clone().or_else(|| n.text.as_ref().map(|t| format!("text {:?}", t.chars().take(24).collect::<String>()))).unwrap_or_else(|| n.path.rsplit('/').next().unwrap_or("").to_string())
}

/// What is cut, outside its parent or overlapping a sibling, in the
/// overlay's body and header. `vertical`: also checks the heights inside
/// the rows (not the lists, which scroll).
pub(super) fn violations(rt: &Runtime<App>, win: (f32, f32)) -> Vec<String> {
    let all = nodes(rt);
    let by_path: std::collections::HashMap<&str, &Node> = all.iter().map(|n| (n.path.as_str(), n)).collect();
    let mut out = vec![];
    let Some(shell) = all.iter().find(|n| n.key.as_deref() == Some("overlay-files")) else { return vec!["the overlay is not drawn".into()] };
    if shell.rect[0] < -EPS || shell.rect[0] + shell.rect[2] > win.0 + EPS {
        out.push(format!("the overlay {:?} leaves the window {win:?}", shell.rect));
    }
    if let Some(h) = all.iter().find(|n| n.key.as_deref() == Some("files-head")) {
        // One row; narrow, two (the buttons, then the path).
        let two = all.iter().any(|n| n.key.as_deref() == Some("files-head-buttons"));
        if h.rect[3] > if two { 62.0 } else { 34.0 } {
            out.push(format!("files-head is {} tall: the path wrapped onto lines", h.rect[3]));
        }
    }
    let inside = |n: &Node| n.path.contains("/overlay-body-files") || n.path.contains("/overlay-head-files");
    // Rows whose height must hold their content (the lists scroll).
    let rowish = |p: &str| {
        let seg = p.rsplit('/').next().unwrap_or("");
        ["files-head", "files-listbar", "files-listing-head", "files-row-", "files-crumbs", "files-acts"].iter().any(|k| seg.starts_with(k))
    };
    let mut kids: std::collections::HashMap<&str, Vec<&Node>> = Default::default();
    for n in &all {
        if !inside(n) || n.rect[2] <= 0.0 && n.rect[3] <= 0.0 {
            continue;
        }
        let Some(pp) = parent_of(&n.path) else { continue };
        let Some(p) = by_path.get(pp) else { continue };
        kids.entry(pp).or_default().push(n);
        let r = n.rect;
        // Sideways, against every ancestor up to the overlay's body (the
        // nearest one it leaves is named).
        let mut up = Some(pp);
        let mut named = false;
        while let Some(a) = up {
            let Some(q) = by_path.get(a) else { break };
            if r[0] < q.rect[0] - EPS || r[0] + r[2] > q.rect[0] + q.rect[2] + EPS {
                out.push(format!("{} {:?} sticks out of {} {:?} sideways", name(n), r, name(q), q.rect));
                named = true;
                break;
            }
            if a.ends_with("/overlay-body-files") || a.ends_with("/overlay-head-files") {
                break;
            }
            up = parent_of(a);
        }
        let q = p.rect;
        if !named && rowish(pp) && (r[1] < q[1] - EPS || r[1] + r[3] > q[1] + q[3] + EPS) {
            out.push(format!("{} {:?} sticks out of {} {:?} vertically", name(n), r, name(p), q));
        }
    }
    for (pp, ks) in &kids {
        for (i, a) in ks.iter().enumerate() {
            for b in &ks[i + 1..] {
                // A thin pinned mark (the accent edge) lies on its row by design,
                // and the toast floats over the columns.
                if [a, b].iter().any(|n| n.key.as_deref() == Some("files-toast")) || a.rect[2] <= 3.0 || b.rect[2] <= 3.0 || a.rect[3] <= 0.0 || b.rect[3] <= 0.0 {
                    continue;
                }
                let (x, y) = (a.rect[0].max(b.rect[0]) < (a.rect[0] + a.rect[2]).min(b.rect[0] + b.rect[2]) - EPS, a.rect[1].max(b.rect[1]) < (a.rect[1] + a.rect[3]).min(b.rect[1] + b.rect[3]) - EPS);
                if x && y {
                    out.push(format!("{} {:?} overlaps {} {:?} in {}", name(a), a.rect, name(b), b.rect, pp.rsplit('/').next().unwrap_or("")));
                }
            }
        }
    }
    out
}

pub(super) fn cases() -> Vec<Case> {
    let mut v = vec![];
    for (w, h) in SIZES {
        for sidebar in [true, false] {
            for reader in [true, false] {
                v.push(Case { w, h, sidebar, reader, extra: 0, long: false, ratio: None, deep: false, offline: false });
            }
        }
    }
    v
}

fn check(cs: &[Case]) {
    let mut bad = vec![];
    for c in cs {
        let rt = scene(*c);
        assert!(rt.presented("screen-error-files").is_none(), "{c:?}: the screen failed to render");
        for v in violations(&rt, (c.w, c.h)) {
            bad.push(format!("{c:?}: {v}"));
        }
    }
    assert!(bad.is_empty(), "{} problems:\n{}", bad.len(), bad.join("\n"));
}

/// The fixture's folder at every size, the sidebar open and collapsed, the
/// reader open and not.
#[test]
fn nothing_is_cut_or_overlaps_at_any_size() {
    check(&cases());
}

/// Long names (120 characters, no space) in the list, the reader and the
/// path; a deep folder; a stopped sandbox's badge.
#[test]
fn long_names_and_a_deep_path_hold() {
    let mut cs = vec![];
    for (w, h) in SIZES {
        for sidebar in [true, false] {
            for reader in [true, false] {
                cs.push(Case { w, h, sidebar, reader, extra: 0, long: true, ratio: None, deep: false, offline: true });
                cs.push(Case { w, h, sidebar, reader, extra: 0, long: false, ratio: None, deep: true, offline: false });
            }
        }
    }
    check(&cs);
}

/// A folder of 500 entries (some with 120-character names): the screen
/// renders inside its budgets and the rows stay in their column.
#[test]
fn a_folder_of_500_entries_holds() {
    let mut cs = vec![];
    for (w, h) in [(1284.0, 816.0), (860.0, 600.0), (720.0, 480.0)] {
        cs.push(Case { w, h, sidebar: true, reader: false, extra: 500, long: true, ratio: None, deep: false, offline: false });
    }
    check(&cs);
}

/// The list and the reader keep their least size when the handle is dragged
/// to either end (the split's range; see `screen::split_range`).
#[test]
fn the_dragged_columns_stay_usable() {
    let mut cs = vec![];
    for r in [0.1, 0.2, 0.8, 0.9] {
        for (w, h, sidebar) in [(1284.0, 816.0, true), (1284.0, 816.0, false), (900.0, 600.0, false), (860.0, 600.0, false)] {
            cs.push(Case { w, h, sidebar, reader: true, extra: 0, long: true, ratio: Some(r), deep: false, offline: false });
        }
    }
    check(&cs);
    let rt = scene(Case { w: 1284.0, h: 816.0, sidebar: true, reader: true, extra: 0, long: false, ratio: Some(0.1), deep: false, offline: false });
    let tree = rt.presented("files-listbar").unwrap().0;
    assert!(tree[2] >= 219.0, "the list keeps 220 pt: {tree:?}");
    let rt = scene(Case { w: 1284.0, h: 816.0, sidebar: true, reader: true, extra: 0, long: false, ratio: Some(0.9), deep: false, offline: false });
    let acts = rt.presented("files-acts").unwrap().0;
    assert!(acts[2] >= 320.0, "the reader keeps 340 pt: {acts:?}");
}

/// The handle, dragged with the pointer to either end, stops where the list
/// and the reader keep their least size; a ratio stored out of range shows
/// inside it; a split without minimums elsewhere keeps 0.1 to 0.9 (see
/// `screen::tests`).
#[test]
fn the_handle_stops_at_the_least_sizes() {
    for (w, h, sidebar) in [(1284.0, 816.0, true), (1284.0, 816.0, false), (900.0, 600.0, false), (860.0, 600.0, false)] {
        let mut rt = scene(Case { w, h, sidebar, reader: true, extra: 0, long: false, ratio: None, deep: false, offline: false });
        for to in [-2000.0, 2000.0] {
            let handle = rt.presented("files-split-handle").unwrap().0;
            let (x, y) = (handle[0] + handle[2] / 2.0, handle[1] + 100.0);
            rt.pointer_down(x, y);
            rt.frame(0.02);
            rt.pointer_move(x + to / 2.0, y);
            rt.frame(0.02);
            rt.pointer_move(x + to, y);
            rt.frame(0.02);
            rt.pointer_up(x + to, y);
            settle(&mut rt);
            let list = rt.presented("files-listbar").unwrap().0;
            let reader = rt.presented("files-acts").unwrap().0;
            assert!(list[2] >= 219.0, "{w} sidebar {sidebar}: dragged to {to}: the list is {} wide", list[2]);
            assert!(reader[2] >= 320.0, "{w} sidebar {sidebar}: dragged to {to}: the reader is {} wide", reader[2]);
            // The drag moved it, and it stopped at the minimum.
            if to < 0.0 {
                assert!(list[2] < 232.0, "{w} sidebar {sidebar}: the list went to its least ({})", list[2]);
            } else {
                assert!(reader[2] < 352.0, "{w} sidebar {sidebar}: the reader went to its least ({})", reader[2]);
            }
            let v = violations(&rt, (w, h));
            assert!(v.is_empty(), "{w} sidebar {sidebar} dragged to {to}: {v:?}");
        }
    }
    // An old stored ratio, out of range (0.05), is shown inside it.
    let mut rt = scene(Case { w: 1284.0, h: 816.0, sidebar: true, reader: true, extra: 0, long: false, ratio: Some(0.05), deep: false, offline: false });
    settle(&mut rt);
    assert!(rt.presented("files-listbar").unwrap().0[2] >= 219.0);
}
