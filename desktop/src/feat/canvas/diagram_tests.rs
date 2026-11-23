//! Diagrams on the canvas: drawn as items of the world (every shape, edge
//! style, lanes, groups, a sequence), agent text as text, the 200 / 400
//! budget; then the person's edits, saves and undo.

use super::window::{opened_of, settle};
use super::*;
use crate::surface_canvas::diagram::{self as dg, Op, Part};
use crate::theme::canvas_c;

pub const FLOW: &str = "d-0000000000a1";
pub const SEQ: &str = "d-0000000000b2";

fn el(rt: &rune::Runtime<crate::shell::App>, key: &str) -> Option<rune::ElementInfo> {
    rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key) && !e.exiting)
}

fn nk(did: &str, n: &str) -> String {
    format!("canvas-dg-{did}-n-{n}")
}

fn ek(did: &str, e: &str) -> String {
    format!("canvas-dg-{did}-e-{e}")
}

/// The diagrams fixture opened, the workflow's first node drawn.
pub fn opened() -> rune::Runtime<crate::shell::App> {
    let mut rt = opened_of("diagrams", &format!("dg-{FLOW}-n-ask"), 1284.0, 816.0);
    // The workflow in view, near enough to read.
    intent_args(&mut rt, "canvas_zoom", json!({ "cmd": "actual" }));
    // The workflow brought into view (fitted, at most 100 %), nothing selected.
    intent_args(&mut rt, "canvas_select", json!({ "target": FLOW }));
    intent_args(&mut rt, "canvas_select", json!({ "target": "" }));
    rt
}

#[test]
fn each_diagram_is_one_item_at_its_place_drawn_with_every_shape_style_lane_and_group() {
    let mut rt = opened();
    settle(&mut rt, 4);
    // One item per diagram, at the manifest's place, as big as its content.
    let scene = crate::surface_canvas::scene(SURF);
    let items: Vec<&crate::surface_canvas::Item> = scene.items.iter().filter(|i| matches!(i.body, crate::surface_canvas::Body::Diagram { .. })).collect();
    assert_eq!(items.len(), 2);
    let flow = items.iter().find(|i| i.id == FLOW).unwrap();
    assert!(flow.movable);
    assert_eq!((flow.x, flow.y), (0.0, 0.0), "lanes start at the diagram's 0, 0");
    assert_eq!((flow.w, flow.h), (1728.0 + dg::LANE_PAD, 336.0));
    let seq = items.iter().find(|i| i.id == SEQ).unwrap();
    assert_eq!(seq.y, 816.0);
    // Every node, a button named by its label; shapes by their box.
    let d: dg::Diagram = serde_json::from_value(window_fixture()["diagrams"][0]["diagram"].clone()).unwrap();
    let shapes: std::collections::BTreeSet<&str> = d.nodes.iter().map(|n| n.shape.name()).collect();
    assert_eq!(shapes.len(), dg::Shape::ALL.len(), "the fixture holds every shape");
    for n in &d.nodes {
        let e = el(&rt, &nk(FLOW, &n.id)).unwrap_or_else(|| panic!("node {} drawn", n.id));
        assert_eq!(e.role, "button", "{}", n.id);
        assert!(e.label.as_deref().unwrap_or("").starts_with(&n.label), "{}: {:?}", n.id, e.label);
        let fill = match n.shape {
            dg::Shape::Note => canvas_c(true, "canvas-note"),
            // A diamond paints its own shape; its box has no fill.
            dg::Shape::Decision => rune::Color::TRANSPARENT,
            _ => canvas_c(true, "canvas-diagram-node"),
        };
        assert_eq!(e.fill, fill, "{}", n.id);
        // Start and end are round; a process a rounded rectangle.
        let process = el(&rt, &nk(FLOW, "plan")).unwrap().radius;
        match n.shape {
            dg::Shape::Start | dg::Shape::End => assert!(e.radius > 2.0 * process, "{}: round ({} against {process})", n.id, e.radius),
            dg::Shape::Process => assert!(e.radius > 0.0, "{}", n.id),
            _ => {}
        }
        let label = rt.elements().into_iter().find(|t| t.text.as_deref() == Some(n.label.as_str()) && t.path.starts_with(&e.path)).unwrap_or_else(|| panic!("{}: its label is text", n.id));
        assert_eq!(label.text_color, Some(canvas_c(true, "canvas-text")));
        if !n.sub.is_empty() {
            assert!(rt.elements().iter().any(|t| t.text.as_deref() == Some(n.sub.as_str())), "{}: its second line", n.id);
        }
    }
    // Every edge: its accessible name, "A to B: label"; labels drawn as text.
    let styles: std::collections::BTreeSet<&str> = d.edges.iter().map(|e| e.style.name()).collect();
    assert_eq!(styles.len(), dg::Style::ALL.len(), "the fixture holds every edge style");
    for e in &d.edges {
        let x = el(&rt, &ek(FLOW, &e.id)).unwrap_or_else(|| panic!("edge {} drawn", e.id));
        assert_eq!(x.label.as_deref(), Some(dg::edge_name(&d, e).as_str()));
        if !e.label.is_empty() {
            assert!(rt.elements().iter().any(|t| t.text.as_deref() == Some(e.label.as_str()) && t.path.starts_with(&x.path)), "{}: label", e.id);
        }
    }
    assert_eq!(el(&rt, &ek(FLOW, "e1")).unwrap().label.as_deref(), Some("Ask to Plan the split: asks"));
    assert_eq!(el(&rt, &ek(FLOW, "e5")).unwrap().label.as_deref(), Some("Work to Test"));
    let err = rt.elements().into_iter().find(|t| t.text.as_deref() == Some("no")).unwrap();
    assert_eq!(err.text_color, Some(canvas_c(true, "canvas-diagram-error")), "an error edge's label in its colour");
    // Every line in one shape element.
    assert!(el(&rt, &format!("canvas-dg-{FLOW}-lines")).is_some());
    // Lanes as bands, in order, each across the whole flow; the group as a frame round its nodes.
    let mut last = f32::MIN;
    for l in ["person", "lead", "thread"] {
        let b = el(&rt, &format!("canvas-dg-{FLOW}-lane-{l}")).unwrap_or_else(|| panic!("lane {l}"));
        assert_eq!(b.fill, canvas_c(true, "canvas-diagram-lane"));
        assert!(b.rect[1] > last, "{l} below the one before");
        last = b.rect[1];
    }
    let g = el(&rt, &format!("canvas-dg-{FLOW}-group-build")).unwrap();
    for n in ["work", "test"] {
        let r = el(&rt, &nk(FLOW, n)).unwrap().rect;
        assert!(r[0] >= g.rect[0] && r[1] >= g.rect[1] && r[0] + r[2] <= g.rect[0] + g.rect[2] && r[1] + r[3] <= g.rect[1] + g.rect[3], "{n} inside its group");
    }
    assert!(rt.elements().iter().any(|t| t.text.as_deref() == Some("Build and test")));
    // The title above the diagram.
    let t = el(&rt, &format!("canvas-dg-{FLOW}-title")).unwrap();
    assert!(t.rect[1] < el(&rt, &format!("canvas-dg-{FLOW}-lane-person")).unwrap().rect[1]);
}

#[test]
fn a_sequence_draws_lifelines_and_its_messages_in_order() {
    let mut rt = opened();
    settle(&mut rt, 2);
    let a = intent_args(&mut rt, "canvas_select", json!({ "target": SEQ }));
    let _ = a;
    settle(&mut rt, 6);
    let mut ys = vec![];
    for (k, m) in ["m1", "m2", "m3", "m4", "m5", "m6"].iter().enumerate() {
        let e = el(&rt, &ek(SEQ, m)).unwrap_or_else(|| panic!("message {m}"));
        ys.push((k, e.rect[1]));
    }
    assert!(ys.windows(2).all(|w| w[0].1 < w[1].1), "rows top to bottom in order: {ys:?}");
    assert_eq!(el(&rt, &ek(SEQ, "m5")).unwrap().label.as_deref(), Some("Agent to Agent: read"), "a self message");
    // Lifeline heads sit side by side on one row.
    let heads: Vec<[f32; 4]> = ["you", "app", "daemon", "agent"].iter().map(|n| el(&rt, &nk(SEQ, n)).unwrap().rect).collect();
    assert!(heads.windows(2).all(|w| w[0][0] < w[1][0] && (w[0][1] - w[1][1]).abs() < 0.5));
    // Messages below the heads.
    assert!(ys[0].1 > heads[0][1] + heads[0][3]);
}

#[test]
fn drawn_on_the_dark_ground_in_the_themes_tokens_and_far_out_without_labels() {
    let mut rt = opened();
    let list = crate::intent::parse(&json!({ "kind": "intent", "name": "canvas_ground", "args": { "ground": "dark" } })).unwrap();
    rt.update(|s, _| list.into_iter().for_each(|i| s.run_intent("test", i)));
    settle(&mut rt, 30);
    assert_eq!(el(&rt, &nk(FLOW, "plan")).unwrap().fill, canvas_c(false, "canvas-diagram-node"));
    assert_eq!(el(&rt, &format!("canvas-dg-{FLOW}-lane-lead")).unwrap().fill, canvas_c(false, "canvas-diagram-lane"));
    let t = rt.elements().into_iter().find(|t| t.text.as_deref() == Some("Plan the split")).unwrap();
    assert_eq!(t.text_color, Some(canvas_c(false, "canvas-text")));
    // Zoomed far out: boxes stay, labels go (unreadable there, and cheaper).
    for _ in 0..12 {
        intent_args(&mut rt, "canvas_zoom", json!({ "cmd": "out" }));
    }
    settle(&mut rt, 4);
    assert!(crate::surface_canvas::camera(SURF).zoom < dg::FAR);
    assert!(el(&rt, &nk(FLOW, "plan")).is_some(), "the node's box");
    assert!(!rt.elements().iter().any(|t| t.text.as_deref() == Some("Plan the split")), "no label far out");
    assert!(el(&rt, &ek(FLOW, "e1")).unwrap().label.is_some(), "still named");
}

#[test]
fn hostile_diagram_text_is_drawn_as_plain_text() {
    let evil = "Push\u{1b}]8;;https://x\u{7}\u{202e}exe.gnp\n\"); intent('canvas_send')";
    let mut w = World::new("diagrams");
    let base = &mut w.daemon["canvas"]["canvases"]["c-00000000000000d1"]["diagrams"][0];
    base["title"] = evil.into();
    base["diagram"]["title"] = evil.into();
    base["diagram"]["nodes"][0]["label"] = evil.into();
    base["diagram"]["nodes"][0]["sub"] = "x".repeat(400).into();
    base["diagram"]["edges"][0]["label"] = evil.into();
    base["diagram"]["lanes"][0]["label"] = evil.into();
    w.open();
    let scene = crate::surface_canvas::scene(SURF);
    let (d, title) = scene.items.iter().find_map(|i| match &i.body { crate::surface_canvas::Body::Diagram { diagram, title, .. } if i.id == FLOW => Some((diagram.clone(), title.clone())), _ => None }).unwrap();
    for t in [d.title.as_str(), d.nodes[0].label.as_str(), d.edges[0].label.as_str(), d.lanes[0].label.as_str(), title.as_str()] {
        assert!(!t.contains('\u{1b}') && !t.contains('\u{7}') && !t.contains('\u{202e}') && !t.contains('\n'), "{t:?}");
        assert!(t.chars().count() <= dg::MAX_TITLE);
    }
    assert_eq!(d.nodes[0].sub.chars().count(), dg::MAX_SUB, "cut to its limit");
    assert!(d.nodes[0].label.contains("intent('canvas_send')"), "kept as text, not run");
    // The screen never keys a control by diagram text.
    let mut h = LuauHost::new(Limits::default()).unwrap();
    h.load("canvas", include_str!("../../../screens/canvas.luau")).unwrap();
    let n = h.render(&w.data).unwrap();
    let mut nodes = vec![];
    super::walk(&n, &mut nodes);
    for x in nodes.iter().filter(|x| x.on.is_some()) {
        let k = x.key.clone().unwrap_or_default();
        assert!(!k.contains("Push") && !k.contains("exe"), "a control keyed by diagram text: {k}");
    }
}

/// 200 nodes and 400 edges, the schema's most: a build of the diagram's
/// elements, and the hit test a press runs. Prints the numbers
/// (`cargo test diagram_cost -- --nocapture`); the bounds are loose (a debug
/// build on a loaded machine); the frame time is for a Mac.
#[test]
fn diagram_cost_at_200_nodes_and_400_edges() {
    let d = big(dg::MAX_NODES, dg::MAX_EDGES);
    assert!(dg::check(&d).is_ok());
    let runs = 10;
    let t = std::time::Instant::now();
    let mut n = 0;
    for _ in 0..runs {
        let look = dg::Look { id: "d-00000000000f", d: &d, origin: [0.0, 0.0], zoom: 1.0, light: true, sel: Some(&Part::Node("n7".into())), rubber: None, row_drag: None, title: "Big" };
        let els = dg::draw(&look);
        n = els.len();
        std::hint::black_box(els);
    }
    let draw_ms = t.elapsed().as_secs_f64() * 1000.0 / runs as f64;
    let t = std::time::Instant::now();
    for k in 0..100 {
        std::hint::black_box(dg::under(&d, [k as f32 * 37.0, k as f32 * 11.0], 6.0));
    }
    let hit_ms = t.elapsed().as_secs_f64() * 1000.0 / 100.0;
    let t = std::time::Instant::now();
    for _ in 0..runs {
        std::hint::black_box(dg::check(&d).unwrap());
    }
    let check_ms = t.elapsed().as_secs_f64() * 1000.0 / runs as f64;
    println!("diagram 200 nodes / 400 edges: {n} elements, {draw_ms:.2} ms a build, {hit_ms:.3} ms a hit test, {check_ms:.2} ms a check");
    // One element a node and a label each, one an edge, lanes, title, lines, handle.
    assert!(n <= 2 * dg::MAX_NODES + dg::MAX_EDGES + 8, "{n}");
    assert!(draw_ms < 250.0, "{draw_ms:.1} ms a build");
    assert!(hit_ms < 25.0, "{hit_ms:.2} ms a hit test");
}

/// A workflow of `nodes` nodes in a grid and `edges` edges, two lanes.
pub fn big(nodes: usize, edges: usize) -> dg::Diagram {
    let mut d = dg::Diagram { version: 1, id: "d-00000000000f".into(), kind: dg::Kind::Workflow, title: "Big".into(), ..dg::Diagram::default() };
    d.lanes = vec![dg::Lane { id: "a".into(), label: "Lane A".into(), at: 0.0, span: 100.0 }, dg::Lane { id: "b".into(), label: "Lane B".into(), at: 100.0, span: 100.0 }];
    for k in 0..nodes {
        let (col, row) = (k % 20, k / 20);
        d.nodes.push(dg::Node { id: format!("n{k}"), label: format!("Step number {k}"), sub: if k % 3 == 0 { "with a second line".into() } else { String::new() }, shape: dg::Shape::ALL[k % 8], lane: Some(if row < 5 { "a" } else { "b" }.into()), x: 120.0 + col as f32 * 220.0, y: 24.0 + row as f32 * 110.0, w: 160.0, h: 64.0, placed: dg::Placed::Layout });
    }
    dg::fit_lanes(&mut d);
    for k in 0..edges {
        let a = k % nodes;
        let b = (a + 1 + k / nodes * 19) % nodes;
        let b = if a == b { (b + 1) % nodes } else { b };
        d.edges.push(dg::Edge { id: format!("e{}", k + 1), from: format!("n{a}"), to: format!("n{b}"), label: if k % 2 == 0 { format!("go {k}") } else { String::new() }, style: dg::Style::ALL[k % 4], order: None });
    }
    d
}

fn window_fixture() -> J {
    super::fixture("diagrams")["daemon"]["canvas"]["canvases"]["c-00000000000000d1"].clone()
}

fn intent_args(rt: &mut rune::Runtime<crate::shell::App>, name: &str, args: J) {
    let list = crate::intent::parse(&json!({ "kind": "intent", "name": name, "args": args })).unwrap();
    rt.update(|s, _| list.into_iter().for_each(|i| s.run_intent("test", i)));
    settle(rt, 4);
}

#[test]
fn the_pure_edits_refuse_what_the_core_would() {
    // (The edits' own tests are in surface_diagram.rs; this one checks the
    // fixture's diagrams pass the same rules as loaded.)
    for k in 0..2 {
        let d: dg::Diagram = serde_json::from_value(window_fixture()["diagrams"][k]["diagram"].clone()).unwrap();
        assert!(dg::check(&d).is_ok(), "{k}: {:?}", dg::check(&d));
        assert!(dg::apply(&d, &Op::Delete { part: Part::Whole }).is_err());
    }
}

// ---------------------------------------------------------------------------
// Editing: each gesture, as the person makes it on the surface

use rune::{Key, KeyEvent, Mods};

fn centre(r: [f32; 4]) -> (f32, f32) {
    (r[0] + r[2] / 2.0, r[1] + r[3] / 2.0)
}

fn at(rt: &rune::Runtime<crate::shell::App>, key: &str) -> (f32, f32) {
    centre(el(rt, key).unwrap_or_else(|| panic!("{key} drawn")).rect)
}

/// A selected node's handle: drawn, on the middle of its right side (a
/// workflow left to right). Read from the node, whose place is settled (the
/// handle may still be growing in).
fn handle(rt: &rune::Runtime<crate::shell::App>, did: &str, node: &str) -> (f32, f32) {
    assert!(el(rt, &format!("canvas-dg-{did}-handle")).is_some(), "the handle drawn");
    let r = el(rt, &nk(did, node)).unwrap().rect;
    (r[0] + r[2], r[1] + r[3] / 2.0)
}

/// A press, a drag in steps, a release.
fn drag(rt: &mut rune::Runtime<crate::shell::App>, from: (f32, f32), to: (f32, f32)) {
    rt.pointer_down(from.0, from.1);
    rt.frame(0.02);
    for k in 1..=6 {
        let t = k as f32 / 6.0;
        rt.pointer_move(from.0 + (to.0 - from.0) * t, from.1 + (to.1 - from.1) * t);
        rt.frame(0.02);
    }
    rt.pointer_up(to.0, to.1);
    settle(rt, 4);
}

fn click(rt: &mut rune::Runtime<crate::shell::App>, k: &str) {
    let p = at(rt, k);
    rt.pointer_down(p.0, p.1);
    rt.pointer_up(p.0, p.1);
    settle(rt, 3);
}

fn double(rt: &mut rune::Runtime<crate::shell::App>, k: &str) {
    let p = at(rt, k);
    double_at(rt, p);
}

fn double_at(rt: &mut rune::Runtime<crate::shell::App>, p: (f32, f32)) {
    rt.pointer_down(p.0, p.1);
    rt.pointer_up(p.0, p.1);
    rt.frame(0.02);
    rt.pointer_down(p.0, p.1);
    rt.pointer_up(p.0, p.1);
    settle(rt, 4);
}

fn key(rt: &mut rune::Runtime<crate::shell::App>, k: Key, mods: Mods) {
    rt.key(KeyEvent { key: k, mods });
    settle(rt, 4);
}

fn cmd() -> Mods {
    Mods { cmd: true, ..Mods::default() }
}

fn typed(rt: &mut rune::Runtime<crate::shell::App>, t: &str) {
    rt.ime(rune::Ime::Commit(t.into()));
    settle(rt, 2);
}

/// The editor of diagram `did` as the feature holds it.
fn cur(did: &str) -> dg::Diagram {
    STATE().lock().unwrap().diagrams[did].cur.clone()
}

/// Waits (frames) until the diagram's saves are answered.
fn saved(rt: &mut rune::Runtime<crate::shell::App>, did: &str) -> u32 {
    for _ in 0..60 {
        settle(rt, 1);
        let s = STATE().lock().unwrap();
        let st = &s.diagrams[did];
        if !st.saving && !st.dirty {
            return st.rev;
        }
    }
    panic!("the diagram's save did not come back");
}

fn rename_box(rt: &rune::Runtime<crate::shell::App>) -> bool {
    el(rt, &format!("canvas-dg-rename-box-{SURF}")).is_some()
}

#[test]
fn dragging_a_node_moves_it_its_edges_follow_and_the_whole_diagram_is_saved() {
    let mut rt = opened();
    let p = at(&rt, &nk(FLOW, "plan"));
    let z = crate::surface_canvas::camera(SURF).zoom;
    drag(&mut rt, p, (p.0 + 80.0, p.1 + 8.0));
    let d = cur(FLOW);
    let n = d.node("plan").unwrap();
    assert_eq!((n.x, n.y), ((336.0 + 80.0 / z).round(), (136.0 + 8.0 / z).round()));
    assert_eq!(n.placed, dg::Placed::Person);
    // Its edges follow: the edge into it ends on its new left side.
    let r = dg::route(&d, d.edges.iter().find(|e| e.to == "plan").unwrap()).unwrap();
    assert_eq!(r.points.last().unwrap()[0], n.x);
    assert_eq!(STATE().lock().unwrap().selected.as_deref(), Some(format!("{FLOW}:n:plan").as_str()));
    assert_eq!(saved(&mut rt, FLOW), 2, "saved: revision 2");
    // Drawn where it now is.
    let q = at(&rt, &nk(FLOW, "plan"));
    assert!((q.0 - p.0 - 80.0).abs() < 2.0, "{p:?} -> {q:?}");
}

#[test]
fn dragging_a_nodes_handle_to_another_node_links_them_and_to_the_ground_adds_a_linked_node() {
    let mut rt = opened();
    click(&mut rt, &nk(FLOW, "plan"));
    let h = handle(&rt, FLOW, "plan");
    let to = at(&rt, &nk(FLOW, "results"));
    drag(&mut rt, h, to);
    let d = cur(FLOW);
    let e = d.edges.iter().find(|e| e.from == "plan" && e.to == "results").expect("a new edge");
    assert_eq!(STATE().lock().unwrap().selected.as_deref(), Some(format!("{FLOW}:e:{}", e.id).as_str()));
    saved(&mut rt, FLOW);
    // From the handle to empty ground: a new node there, linked, its rename open.
    click(&mut rt, &nk(FLOW, "plan"));
    let h = handle(&rt, FLOW, "plan");
    let lane = el(&rt, &format!("canvas-dg-{FLOW}-lane-lead")).unwrap().rect;
    let spot = (h.0 + 40.0, lane[1] + lane[3] / 2.0);
    let before = cur(FLOW).nodes.len();
    drag(&mut rt, h, (spot.0 + 1.0, spot.1));
    let d = cur(FLOW);
    assert_eq!(d.nodes.len(), before + 1);
    let n = d.nodes.last().unwrap();
    assert_eq!(n.lane.as_deref(), Some("lead"), "in the lane it was dropped in");
    assert!(d.edges.iter().any(|e| e.from == "plan" && e.to == n.id));
    assert!(rename_box(&rt), "renamed at once");
    typed(&mut rt, "Deploy");
    key(&mut rt, Key::Enter, Mods::default());
    assert_eq!(cur(FLOW).node(&n.id).unwrap().label, "Deploy");
    assert!(!rename_box(&rt));
    saved(&mut rt, FLOW);
}

#[test]
fn a_double_click_renames_inline_enter_commits_and_escape_cancels() {
    let mut rt = opened();
    double(&mut rt, &nk(FLOW, "test"));
    assert!(rename_box(&rt), "the field over the node");
    assert_eq!(STATE().lock().unwrap().dg_rename.as_deref(), Some(format!("{FLOW}:n:test").as_str()));
    typed(&mut rt, "Unit tests");
    key(&mut rt, Key::Escape, Mods::default());
    assert!(!rename_box(&rt), "Escape closes it");
    assert_eq!(cur(FLOW).node("test").unwrap().label, "Test", "and changes nothing");
    assert!(!STATE().lock().unwrap().diagrams[FLOW].hist.can_undo());
    // Again, with Enter: the field starts from the label, all of it selected.
    double(&mut rt, &nk(FLOW, "test"));
    assert!(rename_box(&rt));
    typed(&mut rt, "Unit tests");
    key(&mut rt, Key::Enter, Mods::default());
    assert_eq!(cur(FLOW).node("test").unwrap().label, "Unit tests");
    // An edge's label too (a double click on it).
    double(&mut rt, &ek(FLOW, "e7"));
    assert_eq!(STATE().lock().unwrap().dg_rename.as_deref(), Some(format!("{FLOW}:e:e7").as_str()));
    typed(&mut rt, "starts");
    key(&mut rt, Key::Enter, Mods::default());
    assert_eq!(cur(FLOW).edge("e7").unwrap().label, "starts");
    // Enter on a selected node opens it too.
    click(&mut rt, &nk(FLOW, "ci"));
    key(&mut rt, Key::Enter, Mods::default());
    assert_eq!(STATE().lock().unwrap().dg_rename.as_deref(), Some(format!("{FLOW}:n:ci").as_str()));
    key(&mut rt, Key::Escape, Mods::default());
    saved(&mut rt, FLOW);
}

#[test]
fn tab_adds_a_linked_successor_delete_removes_a_node_with_its_edges_and_undo_redo_walk_back() {
    let mut rt = opened();
    click(&mut rt, &nk(FLOW, "work"));
    key(&mut rt, Key::Tab, Mods::default());
    let d = cur(FLOW);
    let n = d.nodes.last().unwrap().clone();
    assert!(d.edges.iter().any(|e| e.from == "work" && e.to == n.id), "linked to it");
    assert_eq!(n.lane.as_deref(), Some("thread"));
    assert!(rename_box(&rt), "the successor is renamed at once");
    key(&mut rt, Key::Escape, Mods::default());
    // Delete: the node and every edge on it.
    click(&mut rt, &nk(FLOW, "test"));
    key(&mut rt, Key::Delete, Mods::default());
    let d = cur(FLOW);
    assert!(d.node("test").is_none());
    assert!(!d.edges.iter().any(|e| e.from == "test" || e.to == "test"));
    assert_eq!(STATE().lock().unwrap().selected.as_deref(), Some(FLOW), "its diagram stays selected");
    // ⌘Z brings it back, ⇧⌘Z removes it again, each saved.
    key(&mut rt, Key::Command('z'), cmd());
    assert!(cur(FLOW).node("test").is_some());
    assert_eq!(cur(FLOW).edges.iter().filter(|e| e.from == "test" || e.to == "test").count(), 3);
    key(&mut rt, Key::Command('z'), Mods { shift: true, ..cmd() });
    assert!(cur(FLOW).node("test").is_none());
    key(&mut rt, Key::Command('z'), cmd());
    key(&mut rt, Key::Command('z'), cmd());
    assert!(cur(FLOW).nodes.iter().all(|x| x.id != n.id), "back before the Tab");
    saved(&mut rt, FLOW);
    // Backspace removes an edge.
    click(&mut rt, &ek(FLOW, "e1"));
    key(&mut rt, Key::Backspace, Mods::default());
    assert!(cur(FLOW).edge("e1").is_none());
    saved(&mut rt, FLOW);
}

#[test]
fn add_node_and_lay_out_again_from_the_bar_lay_out_is_one_undo_step() {
    let mut rt = opened();
    click(&mut rt, &nk(FLOW, "plan"));
    assert!(el(&rt, "canvas-diagram-bar").is_some(), "the diagram's bar");
    let before = cur(FLOW);
    super::window::tap(&mut rt, "canvas-diagram-add");
    assert_eq!(cur(FLOW).nodes.len(), before.nodes.len() + 1);
    assert!(rename_box(&rt));
    key(&mut rt, Key::Escape, Mods::default());
    saved(&mut rt, FLOW);
    // A double click on the diagram's empty ground adds one there.
    let lane = el(&rt, &format!("canvas-dg-{FLOW}-lane-lead")).unwrap().rect;
    double_at(&mut rt, (lane[0] + 30.0, lane[1] + lane[3] - 8.0));
    assert_eq!(cur(FLOW).nodes.len(), before.nodes.len() + 2);
    key(&mut rt, Key::Escape, Mods::default());
    saved(&mut rt, FLOW);
    let placed = cur(FLOW);
    super::window::tap(&mut rt, "canvas-diagram-relayout");
    saved(&mut rt, FLOW);
    let laid = cur(FLOW);
    assert!(laid.nodes.iter().all(|n| n.placed == dg::Placed::Layout), "the daemon's layout placed them all");
    assert_ne!(laid.nodes.iter().map(|n| (n.x, n.y)).collect::<Vec<_>>(), placed.nodes.iter().map(|n| (n.x, n.y)).collect::<Vec<_>>());
    super::window::tap(&mut rt, "canvas-diagram-undo");
    saved(&mut rt, FLOW);
    let back = cur(FLOW);
    assert_eq!(back.nodes.iter().map(|n| (n.id.clone(), n.x, n.y)).collect::<Vec<_>>(), placed.nodes.iter().map(|n| (n.id.clone(), n.x, n.y)).collect::<Vec<_>>(), "one undo step back");
}

#[test]
fn a_lifeline_moves_sideways_only_and_a_message_reorders_by_a_vertical_drag() {
    let mut rt = opened();
    intent_args(&mut rt, "canvas_select", json!({ "target": SEQ }));
    settle(&mut rt, 4);
    let p = at(&rt, &nk(SEQ, "app"));
    let z = crate::surface_canvas::camera(SURF).zoom;
    drag(&mut rt, p, (p.0 + 30.0, p.1 + 120.0));
    let n = cur(SEQ).node("app").unwrap().clone();
    assert_eq!((n.x, n.y), ((188.0 + 30.0 / z).round(), 0.0), "sideways only");
    saved(&mut rt, SEQ);
    // The last message dragged to the top row.
    let m6 = at(&rt, &ek(SEQ, "m6"));
    let m1 = at(&rt, &ek(SEQ, "m1"));
    drag(&mut rt, m6, (m6.0, m1.1));
    let d = cur(SEQ);
    assert_eq!(d.edge("m6").unwrap().order, Some(0));
    assert_eq!(d.edge("m1").unwrap().order, Some(1));
    saved(&mut rt, SEQ);
}

// ---------------------------------------------------------------------------
// Saves: the body, one in flight, the 409 path; limits before sending; Send

fn dsaves(w: &World) -> Vec<&Call> {
    w.calls.iter().filter(|c| c.tag.starts_with("cv-dsave:")).collect()
}

#[test]
fn a_save_is_the_whole_diagram_from_its_revision_one_in_flight_and_nothing_is_lost() {
    let mut w = World::new("diagrams");
    w.open();
    w.hold = vec!["cv-dsave:"];
    w.surface(CanvasEvent::Diagram { item: FLOW.into(), op: Op::Move { node: "plan".into(), x: 400.0, y: 140.0 } });
    w.surface(CanvasEvent::Diagram { item: FLOW.into(), op: Op::Rename { part: Part::Node("plan".into()), text: "Split the work".into() } });
    w.surface(CanvasEvent::Diagram { item: FLOW.into(), op: Op::Link { from: "plan".into(), to: "merged".into() } });
    assert_eq!(w.held.len(), 1, "one save in flight");
    let first = w.held.remove(0);
    assert_eq!(first.path, format!("/api/canvas/c-00000000000000d1/diagrams/{FLOW}"));
    assert_eq!(first.body["rev"], 1);
    let n = first.body["diagram"]["nodes"].as_array().unwrap().iter().find(|n| n["id"] == "plan").unwrap().clone();
    assert_eq!((n["x"].as_f64(), n["placed"].as_str()), (Some(400.0), Some("person")));
    assert_eq!(first.body["diagram"]["version"], 1);
    // The answer sends the rest, as one, from the new revision.
    w.hold.clear();
    let (code, v) = crate::feat::answer_from(&mut w.daemon, "POST", &first.path, &first.body, &mut |_, _| {});
    assert_eq!(code, 200);
    let more = on_answer(&w.cx(), &first, Ok((code, v)));
    w.run(more);
    let last = *dsaves(&w).last().unwrap();
    assert_eq!(last.body["rev"], 2);
    let nodes = last.body["diagram"]["nodes"].as_array().unwrap();
    assert!(nodes.iter().any(|n| n["id"] == "plan" && n["label"] == "Split the work"));
    assert!(last.body["diagram"]["edges"].as_array().unwrap().iter().any(|e| e["from"] == "plan" && e["to"] == "merged"));
    assert_eq!(STATE().lock().unwrap().diagrams[FLOW].rev, 3);
    // The Send button counts the changed diagram, as the daemon does.
    assert_eq!(w.ui()["send"]["new"], 1);
}

#[test]
fn a_stale_save_is_said_with_reload_and_reapply_my_change() {
    for choice in ["reapply", "reload"] {
        let mut w = World::new("diagrams");
        w.open();
        // The agent replaces the diagram meanwhile (revision 4).
        let mut agent = super::fixture("diagrams")["daemon"]["canvas"]["canvases"]["c-00000000000000d1"]["diagrams"][0].clone();
        agent["diagram"]["rev"] = 4.into();
        agent["diagram"]["nodes"][1]["label"] = "Plan with the agent".into();
        w.daemon["canvas"]["canvases"]["c-00000000000000d1"]["diagrams"][0] = agent;
        w.surface(CanvasEvent::Diagram { item: FLOW.into(), op: Op::Rename { part: Part::Node("ask".into()), text: "Ask again".into() } });
        let st = STATE().lock().unwrap().diagrams[FLOW].clone();
        let server = st.conflict.clone().expect("the newer version held");
        assert_eq!(server.rev, 4);
        assert_eq!(st.cur.node("ask").unwrap().label, "Ask again", "mine kept");
        let ui = w.ui()["diagram"].clone();
        assert!(ui["conflict"].as_str().unwrap().contains("changed elsewhere"), "{ui}");
        // Shown with its two buttons, as text.
        let mut h = LuauHost::new(Limits::default()).unwrap();
        h.load("canvas", include_str!("../../../screens/canvas.luau")).unwrap();
        let tree = h.render(&w.data).unwrap();
        let mut nodes = vec![];
        super::walk(&tree, &mut nodes);
        for k in ["canvas-diagram-reload", "canvas-diagram-reapply", "canvas-diagram-conflict"] {
            assert!(nodes.iter().any(|n| n.key.as_deref() == Some(k)), "{k}");
        }
        // Nothing is saved until one is chosen.
        let n = dsaves(&w).len();
        w.surface(CanvasEvent::Diagram { item: FLOW.into(), op: Op::Move { node: "ask".into(), x: 140.0, y: 24.0 } });
        assert_eq!(dsaves(&w).len(), n);
        // Sending waits too.
        assert!(w.intent("canvas_send", json!({})).unwrap_err().contains("Reload"));
        let a = w.intent("canvas_diagram", json!({ "op": choice, "id": FLOW })).unwrap();
        w.run(a);
        let st = STATE().lock().unwrap().diagrams[FLOW].clone();
        assert!(st.conflict.is_none());
        if choice == "reapply" {
            let last = *dsaves(&w).last().unwrap();
            assert_eq!(last.body["rev"], 4, "mine over the newer revision");
            assert_eq!(st.rev, 5);
            assert_eq!(st.cur.node("ask").unwrap().label, "Ask again");
        } else {
            assert_eq!(st.cur.node("plan").unwrap().label, "Plan with the agent");
            assert_eq!(st.rev, 4);
            assert!(st.hist.can_undo(), "mine one undo step back");
        }
    }
}

#[test]
fn an_edit_past_a_limit_is_refused_before_anything_is_sent() {
    let mut w = World::new("diagrams");
    w.open();
    for op in [
        Op::Rename { part: Part::Node("plan".into()), text: "y".repeat(81) },
        Op::Rename { part: Part::Node("plan".into()), text: "\u{200b}\n".into() },
        Op::Link { from: "plan".into(), to: "plan".into() },
        Op::Move { node: "plan".into(), x: 1.0e6, y: 0.0 },
    ] {
        let acts = w.surface(CanvasEvent::Diagram { item: FLOW.into(), op: op.clone() });
        assert!(acts.iter().any(|a| matches!(a, Action::Notice { error: true, text, .. } if text.starts_with("Not changed"))), "{op:?}: {acts:?}");
    }
    assert!(dsaves(&w).is_empty(), "nothing sent");
    assert!(!STATE().lock().unwrap().diagrams[FLOW].hist.can_undo());
    // A diagram at 200 nodes takes no 201st.
    let big = big(dg::MAX_NODES, 10);
    let item = json!({ "id": "d-00000000000f", "title": "Big", "kind": "workflow", "round": 3, "added_ms": 1, "x": 0, "y": 2000, "w": 100, "h": 100, "moved": false, "bytes": 1, "diagram": dg::wire(&big) });
    let (code, _) = crate::feat::answer_from(&mut w.daemon, "POST", "/api/canvas/c-00000000000000d1/standin-diagram", &json!({ "diagram": item }), &mut |_, _| {});
    assert_eq!(code, 200);
    let id = STATE().lock().unwrap().current.clone().unwrap();
    let a = vec![STATE().lock().unwrap().read_canvas(&id)];
    w.run(a);
    let acts = w.surface(CanvasEvent::Diagram { item: "d-00000000000f".into(), op: Op::AddAt { x: 0.0, y: 0.0 } });
    assert!(acts.iter().any(|a| matches!(a, Action::Notice { error: true, text, .. } if text.contains("at most 200"))));
    assert!(dsaves(&w).is_empty());
}

#[test]
fn a_diagram_change_counts_in_the_send_and_send_keeps_its_one_confirmation() {
    let mut w = World::new("diagrams");
    w.open();
    assert_eq!(w.ui()["send"]["new"], 0);
    assert!(w.intent("canvas_send", json!({})).is_err());
    w.surface(CanvasEvent::Diagram { item: FLOW.into(), op: Op::Rename { part: Part::Node("merged".into()), text: "Merged to main".into() } });
    assert_eq!(w.ui()["send"]["new"], 1);
    assert_eq!(w.ui()["send"]["ok"], true);
    let acts = w.intent("canvas_send", json!({})).unwrap();
    let c = w.confirm(&acts);
    assert!(c.title.starts_with("Send 1 remark to"), "{}", c.title);
    assert!(c.lines.iter().any(|l| l.contains("1 changed diagram(s)")), "{:?}", c.lines);
    let after = w.press(&c);
    let _ = after;
    let review = &w.daemon["canvas"]["canvases"]["c-00000000000000d1"]["reviews"][0];
    assert_eq!(review["diagrams"], 1);
    assert!(w.calls.iter().any(|c| c.tag.starts_with("cv-push:")), "the fixed line typed");
}


