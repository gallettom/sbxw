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
    rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key))
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
