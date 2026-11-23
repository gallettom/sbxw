use super::*;

const WORKFLOW: &str = include_str!("fixtures/thread-review.workflow.json");
const SEQUENCE: &str = include_str!("fixtures/canvas-review.sequence.json");
const LIFECYCLE: &str = include_str!("fixtures/thread-run.lifecycle.json");
const ARCHITECTURE: &str = include_str!("fixtures/codemap.architecture.json");
const FIXTURES: [&str; 4] = [WORKFLOW, SEQUENCE, LIFECYCLE, ARCHITECTURE];

/// The module doc's example.
const EXAMPLE: &str = r#"{ "version": 1, "kind": "workflow", "title": "Release", "direction": "lr",
  "lanes":  [{ "id": "dev", "label": "Developer" }, { "id": "ci", "label": "CI" }],
  "groups": [{ "id": "checks", "label": "Checks", "nodes": ["test", "lint"] }],
  "nodes":  [{ "id": "push", "label": "Push", "sub": "a branch", "lane": "dev" },
             { "id": "test", "label": "Test", "lane": "ci", "shape": "process" },
             { "id": "lint", "label": "Lint", "lane": "ci" }],
  "edges":  [{ "from": "push", "to": "test", "label": "webhook" },
             { "from": "push", "to": "lint", "style": "dashed" }] }"#;

fn laid(json: &str) -> Diagram {
    let mut d = import::archify(json).unwrap();
    layout::place_missing(&mut d);
    d
}

fn flow(nodes: usize, edges: usize) -> serde_json::Value {
    let ns: Vec<_> = (0..nodes).map(|i| serde_json::json!({ "id": format!("n{i}"), "label": format!("Node {i}") })).collect();
    let es: Vec<_> = (0..edges).map(|i| serde_json::json!({ "from": "n0", "to": format!("n{}", 1 + i % (nodes - 1)) })).collect();
    serde_json::json!({ "version": 1, "kind": "workflow", "title": "t", "nodes": ns, "edges": es })
}

fn parse_v(v: &serde_json::Value) -> Result<Diagram, String> {
    parse(&v.to_string())
}

// ---------------------------------------------------------------------------
// Schema

#[test]
fn the_example_parses_with_defaults_filled() {
    let d = parse(EXAMPLE).unwrap();
    assert_eq!((d.kind, d.direction, d.nodes.len(), d.edges.len()), (Kind::Workflow, Direction::Lr, 3, 2));
    assert_eq!(d.edges.iter().map(|e| e.id.as_str()).collect::<Vec<_>>(), ["e1", "e2"]);
    assert!(d.nodes.iter().all(|n| n.placed == Placed::None && n.w >= 96.0 && n.h >= 44.0));
    assert_eq!(d.node("push").unwrap().h, 64.0, "a sub makes a node taller");
}

#[test]
fn the_schema_round_trips_before_and_after_layout() {
    let mut d = parse(EXAMPLE).unwrap();
    assert_eq!(parse(&d.to_json()).unwrap(), d);
    layout::place_missing(&mut d);
    d.id = "d-0123456789ab".into();
    d.rev = 3;
    d.author = Author::Person;
    d.updated_ms = 1_700_000_000_000;
    d.nodes[0].placed = Placed::Person;
    let back = parse(&d.to_json()).unwrap();
    assert_eq!(back, d);
    for f in FIXTURES {
        let d = laid(f);
        assert_eq!(parse(&d.to_json()).unwrap(), d, "{}", d.title);
    }
}

#[test]
fn unknown_fields_and_versions_are_refused_in_sbxw_schema() {
    let e = parse(&EXAMPLE.replace("\"title\": \"Release\"", "\"title\": \"Release\", \"colour\": 1")).unwrap_err();
    assert!(e.contains("colour"), "{e}");
    let e = parse(&EXAMPLE.replace("\"version\": 1", "\"version\": 2")).unwrap_err();
    assert!(e.contains("version 2"), "{e}");
    let e = parse(&EXAMPLE.replace("\"workflow\"", "\"gantt\"")).unwrap_err();
    assert!(e.contains("gantt"), "{e}");
}

#[test]
fn half_a_position_is_refused() {
    let e = parse(&EXAMPLE.replace("\"label\": \"Lint\"", "\"label\": \"Lint\", \"x\": 4")).unwrap_err();
    assert!(e.contains("both x and y"), "{e}");
    let d = parse(&EXAMPLE.replace("\"label\": \"Lint\"", "\"label\": \"Lint\", \"x\": 4, \"y\": 5")).unwrap();
    let n = d.node("lint").unwrap();
    assert_eq!((n.x, n.y, n.placed), (4.0, 5.0, Placed::Agent));
}

// ---------------------------------------------------------------------------
// Limits, each at its bound

#[test]
fn counts_are_refused_one_past_their_bound() {
    assert!(parse_v(&flow(MAX_NODES, 10)).is_ok());
    assert!(parse_v(&flow(MAX_NODES + 1, 10)).unwrap_err().contains("201 nodes"));
    assert!(parse_v(&flow(20, MAX_EDGES)).is_ok());
    assert!(parse_v(&flow(20, MAX_EDGES + 1)).unwrap_err().contains("401 edges"));
    let lanes = |n: usize| {
        let mut v = flow(2, 1);
        v["lanes"] = (0..n).map(|i| serde_json::json!({ "id": format!("l{i}"), "label": "L" })).collect();
        for node in v["nodes"].as_array_mut().unwrap() {
            node["lane"] = "l0".into();
        }
        v
    };
    assert!(parse_v(&lanes(MAX_LANES)).is_ok());
    assert!(parse_v(&lanes(MAX_LANES + 1)).unwrap_err().contains("17 lanes"));
    let groups = |n: usize| {
        let mut v = flow(40, 1);
        v["groups"] = (0..n).map(|i| serde_json::json!({ "id": format!("g{i}"), "label": "G", "nodes": [format!("n{i}")] })).collect();
        v
    };
    assert!(parse_v(&groups(MAX_GROUPS)).is_ok());
    assert!(parse_v(&groups(MAX_GROUPS + 1)).unwrap_err().contains("33 groups"));
}

#[test]
fn a_huge_count_is_refused_before_anything_is_built() {
    // Ten thousand nodes fit in the size limit only when tiny; the count says no first.
    let ns: String = (0..10_000).map(|i| format!("{{\"id\":\"{i}\",\"label\":\"x\"}}")).collect::<Vec<_>>().join(",");
    let json = format!("{{\"version\":1,\"kind\":\"workflow\",\"nodes\":[{ns}]}}");
    assert!(json.len() <= MAX_SOURCE_BYTES);
    assert!(parse(&json).unwrap_err().contains("10000 nodes"));
}

#[test]
fn the_source_size_is_refused_past_its_bound() {
    let pad = |n: usize| format!("{}{}", EXAMPLE, " ".repeat(n - EXAMPLE.len()));
    assert!(parse(&pad(MAX_SOURCE_BYTES)).is_ok());
    assert!(parse(&pad(MAX_SOURCE_BYTES + 1)).unwrap_err().contains("at most 256 KB"));
    assert!(import::archify(&pad(MAX_SOURCE_BYTES + 1)).unwrap_err().contains("at most 256 KB"));
    assert!(import::parse_any(&pad(MAX_SOURCE_BYTES + 1)).unwrap_err().contains("at most 256 KB"));
}

#[test]
fn text_lengths_are_refused_one_past_their_bound() {
    let set = |path: &[&str], len: usize| {
        let mut v: serde_json::Value = serde_json::from_str(EXAMPLE).unwrap();
        let mut at = &mut v;
        for p in path {
            at = match p.parse::<usize>() {
                Ok(i) => &mut at[i],
                Err(_) => &mut at[*p],
            };
        }
        *at = "é".repeat(len).into(); // characters, not bytes
        parse_v(&v)
    };
    for (path, max) in [(&["title"][..], MAX_TITLE), (&["nodes", "0", "label"], MAX_LABEL), (&["nodes", "0", "sub"], MAX_SUB), (&["edges", "0", "label"], MAX_LABEL), (&["lanes", "0", "label"], MAX_LABEL), (&["groups", "0", "label"], MAX_LABEL)] {
        assert!(set(path, max).is_ok(), "{path:?} at {max}");
        let e = set(path, max + 1).unwrap_err();
        assert!(e.contains(&format!("{} characters, at most {max}", max + 1)), "{path:?}: {e}");
    }
}

#[test]
fn ids_are_refused_one_past_their_bound() {
    let with = |id: &str| parse(&EXAMPLE.replace("\"push\"", &format!("\"{id}\"")));
    assert!(with(&"a".repeat(MAX_ID)).is_ok());
    assert!(with(&"a".repeat(MAX_ID + 1)).unwrap_err().contains("is not an id"));
}

#[test]
fn coordinates_and_sizes_are_refused_past_their_bounds() {
    let at = |x: f32, w: f32| {
        let mut d = parse(EXAMPLE).unwrap();
        d.nodes[0].x = x;
        d.nodes[0].w = w;
        check(&mut d)
    };
    assert!(at(MAX_COORD, MIN_SIDE).is_ok());
    assert!(at(-MAX_COORD, MAX_SIDE).is_ok());
    assert!(at(MAX_COORD + 1.0, 100.0).unwrap_err().contains("out of bounds"));
    assert!(at(-MAX_COORD - 1.0, 100.0).unwrap_err().contains("out of bounds"));
    assert!(at(f32::NAN, 100.0).unwrap_err().contains("out of bounds"));
    assert!(at(f32::INFINITY, 100.0).unwrap_err().contains("out of bounds"));
    assert!(at(0.0, MIN_SIDE - 1.0).unwrap_err().contains("between"));
    assert!(at(0.0, MAX_SIDE + 1.0).unwrap_err().contains("between"));
    let json = EXAMPLE.replace("\"label\": \"Lint\"", "\"label\": \"Lint\", \"x\": 1e9, \"y\": 0");
    assert!(parse(&json).unwrap_err().contains("out of bounds"));
}

#[test]
fn a_sequence_order_is_refused_past_its_bound() {
    let seq = |order: u32| {
        parse_v(&serde_json::json!({ "version": 1, "kind": "sequence", "nodes": [{ "id": "a", "label": "A" }],
            "edges": [{ "from": "a", "to": "a", "order": order }] }))
    };
    assert!(seq(MAX_EDGES as u32 - 1).is_ok());
    assert!(seq(MAX_EDGES as u32).unwrap_err().contains("past 399"));
}

// ---------------------------------------------------------------------------
// Hostile texts and ids

#[test]
fn texts_lose_control_and_invisible_characters_as_canvas_texts_do() {
    let hostile = "\u{202e}Deploy\u{200b}\n\u{7}now\u{e0041}\t";
    let mut d = parse(EXAMPLE).unwrap();
    d.nodes[0].label = hostile.into();
    d.title = format!("  {hostile}  ");
    d.edges[0].label = "\u{2066}ok\u{2069}".into();
    check(&mut d).unwrap();
    assert_eq!(d.nodes[0].label, crate::canvas::clean(hostile, usize::MAX, false));
    assert_eq!(d.nodes[0].label, "Deploynow");
    assert_eq!(d.title, "Deploynow");
    assert_eq!(d.edges[0].label, "ok");
    for c in crate::invisible::all() {
        let mut d = parse(EXAMPLE).unwrap();
        d.nodes[1].label = format!("a{c}b");
        check(&mut d).unwrap();
        assert_eq!(d.nodes[1].label, "ab", "U+{:04X}", c as u32);
    }
}

#[test]
fn a_label_made_only_of_invisible_characters_is_refused() {
    let mut d = parse(EXAMPLE).unwrap();
    d.nodes[0].label = "\u{200b}\u{202e} \u{feff}".into();
    assert!(check(&mut d).unwrap_err().contains("has no label"));
    let mut d = parse(EXAMPLE).unwrap();
    d.lanes[0].label = "\u{2060}".into();
    assert!(check(&mut d).unwrap_err().contains("has no label"));
}

#[test]
fn path_like_and_odd_ids_are_refused_not_cleaned() {
    for id in ["../etc", "a/b", "a\\\\b", "C:x", ".hidden", "-lead", "_x", "Push", "a b", "é", "", "a\\u0000", "a\\u200bb", "~", "%2e%2e"] {
        let e = parse(&EXAMPLE.replace("\"id\": \"push\"", &format!("\"id\": \"{id}\""))).unwrap_err();
        assert!(e.contains("is not an id"), "{id}: {e}");
        assert!(!e.contains('\u{0}') && !e.contains('\u{200b}'), "the error quotes the id cleaned: {e:?}");
    }
    for id in ["../d-0123", "d-0123456789AB", "d-0123456789a", "c-0123456789ab"] {
        let e = parse(&EXAMPLE.replace("\"version\": 1", &format!("\"version\": 1, \"id\": \"{id}\""))).unwrap_err();
        assert!(e.contains("not one sbxw makes"), "{id}: {e}");
    }
    assert!(valid_diagram_id("d-0123456789ab"));
}

#[test]
fn references_must_name_what_exists() {
    let cases = [
        (EXAMPLE.replace("\"to\": \"test\"", "\"to\": \"../test\""), "not in nodes"),
        (EXAMPLE.replace("\"lane\": \"dev\"", "\"lane\": \"ops\""), "not in lanes"),
        (EXAMPLE.replace("\"nodes\": [\"test\", \"lint\"]", "\"nodes\": [\"test\", \"ghost\"]"), "not in nodes"),
        (EXAMPLE.replace(", \"lane\": \"dev\"", ""), "has no lane"),
        (EXAMPLE.replace("\"nodes\": [\"test\", \"lint\"]", "\"nodes\": [\"test\", \"push\"]"), "spans two lanes"),
        (EXAMPLE.replace("\"to\": \"test\"", "\"to\": \"push\""), "to itself"),
        (EXAMPLE.replace("\"id\": \"lint\"", "\"id\": \"test\""), "used twice"),
        (EXAMPLE.replace("\"style\": \"dashed\"", "\"style\": \"dashed\", \"order\": 1"), "only sequence"),
    ];
    for (json, want) in cases {
        let e = parse(&json).unwrap_err();
        assert!(e.contains(want), "{want}: {e}");
    }
    let two_groups = EXAMPLE.replace("\"groups\": [", "\"groups\": [{ \"id\": \"g2\", \"label\": \"G\", \"nodes\": [\"lint\"] }, ");
    assert!(parse(&two_groups).unwrap_err().contains("in two groups"));
}

#[test]
fn kind_rules_hold() {
    let life = |starts: usize| {
        let ns: Vec<_> = (0..3).map(|i| serde_json::json!({ "id": format!("s{i}"), "label": "S", "shape": if i < starts { "start" } else { "process" } })).collect();
        parse_v(&serde_json::json!({ "version": 1, "kind": "lifecycle", "nodes": ns, "edges": [{ "from": "s1", "to": "s1" }] }))
    };
    assert!(life(1).is_ok(), "a lifecycle may loop on a state");
    assert!(life(2).unwrap_err().contains("at most one start"));
    let seq = |edges: serde_json::Value| parse_v(&serde_json::json!({ "version": 1, "kind": "sequence", "nodes": [{ "id": "a", "label": "A" }, { "id": "b", "label": "B" }], "edges": edges }));
    let d = seq(serde_json::json!([{ "from": "a", "to": "b" }, { "from": "b", "to": "a" }])).unwrap();
    assert_eq!(d.edges.iter().map(|e| e.order).collect::<Vec<_>>(), [Some(0), Some(1)], "array order without orders");
    let d = seq(serde_json::json!([{ "from": "a", "to": "b", "order": 7 }, { "from": "b", "to": "a", "order": 3 }])).unwrap();
    assert_eq!(d.edges.iter().map(|e| e.order).collect::<Vec<_>>(), [Some(1), Some(0)], "orders renumbered from 0");
    assert!(seq(serde_json::json!([{ "from": "a", "to": "b", "order": 1 }, { "from": "b", "to": "a" }])).unwrap_err().contains("every message an order, or none"));
    let mut v = serde_json::json!({ "version": 1, "kind": "sequence", "nodes": [{ "id": "a", "label": "A" }], "lanes": [{ "id": "l", "label": "L" }] });
    v["nodes"][0]["lane"] = "l".into();
    assert!(parse_v(&v).unwrap_err().contains("no lanes or groups"));
}

// ---------------------------------------------------------------------------
// The Archify importer

#[test]
fn the_workflow_fixture_imports() {
    let d = import::archify(WORKFLOW).unwrap();
    assert_eq!((d.kind, d.direction, d.title.as_str()), (Kind::Workflow, Direction::Lr, "A thread's work, from plan to merge"));
    assert_eq!(d.lanes.iter().map(|l| l.id.as_str()).collect::<Vec<_>>(), ["person", "lead", "thread", "checks"]);
    assert_eq!((d.nodes.len(), d.edges.len()), (10, 11));
    assert!(d.node("mergetree").is_some(), "ids are lower-cased");
    assert_eq!(d.node("ask").unwrap().shape, Shape::External);
    assert_eq!(d.groups, vec![Group { id: "build_loop".into(), label: "Build and test loop".into(), nodes: vec!["work".into(), "test".into()] }]);
    let style = |from: &str, to: &str| d.edges.iter().find(|e| e.from == from && e.to == to).unwrap().style;
    assert_eq!(style("test", "blocked"), Style::Error);
    assert_eq!(style("test", "work"), Style::Dashed);
    assert_eq!(style("split", "approve"), Style::Emphasis);
    assert_eq!(d.edges[0].id, "ask-split");
    assert_eq!(d.edges[3].id, "e1", "missing edge ids are numbered");
}

#[test]
fn the_sequence_fixture_imports_with_messages_in_y_order() {
    let d = import::archify(SEQUENCE).unwrap();
    assert_eq!(d.kind, Kind::Sequence);
    assert_eq!(d.nodes.iter().map(|n| n.id.as_str()).collect::<Vec<_>>(), ["agent", "relay", "daemon", "store", "desktop", "person"]);
    assert_eq!(d.node("person").unwrap().shape, Shape::Actor);
    assert_eq!(d.node("store").unwrap().shape, Shape::Store);
    let orders: Vec<u32> = d.edges.iter().map(|e| e.order.unwrap()).collect();
    assert_eq!(orders, (0..10).collect::<Vec<_>>());
    let check = d.edges.iter().find(|e| e.id == "check").unwrap();
    assert_eq!(check.from, check.to, "a self message is kept");
    assert_eq!(d.edges.iter().find(|e| e.id == "ready").unwrap().style, Style::Dashed);
}

#[test]
fn the_lifecycle_fixture_imports() {
    let d = import::archify(LIFECYCLE).unwrap();
    assert_eq!(d.kind, Kind::Lifecycle);
    assert_eq!(d.node("planned").unwrap().shape, Shape::Start);
    assert_eq!(d.node("merged").unwrap().shape, Shape::End);
    assert!(d.edges.iter().any(|e| e.from == "working" && e.to == "working"));
}

#[test]
fn the_codemap_architecture_imports_as_a_workflow_with_its_places() {
    let d = import::archify(ARCHITECTURE).unwrap();
    assert_eq!((d.kind, d.nodes.len(), d.edges.len()), (Kind::Workflow, 12, 12));
    let embed = d.node("embed").unwrap();
    assert_eq!((embed.x, embed.y, embed.w, embed.h, embed.placed), (20.0, 200.0, 170.0, 64.0, Placed::Agent));
    assert!(layout::overlaps(&d).is_empty(), "the author's places do not overlap");
}

#[test]
fn the_importer_ignores_what_it_does_not_know_and_refuses_what_it_cannot_read() {
    let mut v: serde_json::Value = serde_json::from_str(LIFECYCLE).unwrap();
    v["future_field"] = serde_json::json!({ "x": [1, 2] });
    v["states"][0]["glow"] = true.into();
    assert!(import::archify(&v.to_string()).is_ok());
    for (t, want) in [("gantt", "\"gantt\" is not one sbxw imports"), ("", "has a diagram_type")] {
        v["diagram_type"] = t.into();
        assert!(import::archify(&v.to_string()).unwrap_err().contains(want), "{t}");
    }
    let mut v: serde_json::Value = serde_json::from_str(LIFECYCLE).unwrap();
    v["schema_version"] = 3.into();
    assert!(import::archify(&v.to_string()).unwrap_err().contains("version 3"));
    let mut v: serde_json::Value = serde_json::from_str(LIFECYCLE).unwrap();
    v["states"][1]["id"] = "PLANNED".into();
    assert!(import::archify(&v.to_string()).unwrap_err().contains("once lower-cased"));
    v["states"][1]["id"] = "x".repeat(41).into();
    assert!(import::archify(&v.to_string()).unwrap_err().contains("is not an id"));
    v["states"][1]["id"] = "../../etc/passwd".into();
    assert!(import::archify(&v.to_string()).unwrap_err().contains("is not an id"));
    v["states"][1]["id"] = "working".into();
    v["states"][1]["label"] = "\u{202e}Working\u{200d}".into();
    let d = import::archify(&v.to_string()).unwrap();
    assert_eq!(d.node("working").unwrap().label, "Working");
    assert!(import::archify("[1, 2]").unwrap_err().contains("diagram_type"));
    assert!(import::archify("{").unwrap_err().contains("not JSON"));
}

#[test]
fn parse_any_tells_the_formats_apart() {
    assert_eq!(import::parse_any(EXAMPLE).unwrap(), parse(EXAMPLE).unwrap());
    assert_eq!(import::parse_any(WORKFLOW).unwrap(), import::archify(WORKFLOW).unwrap());
}

// ---------------------------------------------------------------------------
// Layout

fn relaid(json: &str) -> Diagram {
    let mut d = import::archify(json).unwrap();
    layout::relayout(&mut d);
    d
}

#[test]
fn layout_is_deterministic() {
    for f in FIXTURES {
        let (a, b) = (relaid(f), relaid(f));
        assert_eq!(a, b, "{}", a.title);
        let mut c = a.clone();
        layout::relayout(&mut c);
        assert_eq!(c, a, "laying out again gives the same places");
        assert!(a.nodes.iter().all(|n| n.placed == Placed::Layout && n.x == n.x.round() && n.y == n.y.round()));
    }
}

#[test]
fn no_node_boxes_overlap_on_the_fixtures() {
    for f in FIXTURES {
        for d in [laid(f), relaid(f)] {
            assert!(layout::overlaps(&d).is_empty(), "{}: {:?}", d.title, layout::overlaps(&d));
            assert!(d.nodes.iter().all(|n| n.x >= 0.0 && n.y >= 0.0), "{}", d.title);
        }
    }
}

#[test]
fn lanes_are_disjoint_bands_in_order_holding_their_nodes() {
    for f in [WORKFLOW, LIFECYCLE] {
        let d = relaid(f);
        let mut at = 0.0;
        for l in &d.lanes {
            assert_eq!(l.at, at, "{}: bands follow each other in declared order", l.id);
            at += l.span;
            for n in d.nodes.iter().filter(|n| n.lane.as_deref() == Some(&l.id)) {
                assert!(n.y >= l.at && n.y + n.h <= l.at + l.span, "{} inside {}", n.id, l.id);
            }
        }
    }
    // One node per rank in a lane: a straight line.
    let d = relaid(LIFECYCLE);
    let mains: Vec<f32> = d.nodes.iter().filter(|n| n.lane.as_deref() == Some("main")).map(|n| n.y + n.h / 2.0).collect();
    assert!(mains.windows(2).all(|w| w[0] == w[1]), "{mains:?}");
}

#[test]
fn flows_run_left_to_right() {
    let d = relaid(WORKFLOW);
    let x = |id: &str| d.node(id).unwrap().x;
    for (a, b) in [("ask", "split"), ("split", "approve"), ("approve", "work"), ("work", "test"), ("test", "report"), ("report", "mergetree"), ("mergetree", "review"), ("review", "merge")] {
        assert!(x(a) < x(b), "{a} before {b}");
    }
}

#[test]
fn top_to_bottom_transposes() {
    let mut d = parse(&EXAMPLE.replace("\"lr\"", "\"tb\"")).unwrap();
    layout::place_missing(&mut d);
    let (push, test) = (d.node("push").unwrap(), d.node("test").unwrap());
    assert!(push.y + push.h <= test.y, "ranks go down");
    assert!(d.lanes[0].at + d.lanes[0].span <= d.lanes[1].at + 0.5, "lanes are columns");
    assert!(push.x >= d.lanes[0].at && test.x >= d.lanes[1].at);
    assert!(layout::overlaps(&d).is_empty());
}

#[test]
fn sequence_lifelines_line_up_and_messages_go_down() {
    let d = relaid(SEQUENCE);
    assert!(d.nodes.iter().all(|n| n.y == 0.0));
    assert!(d.nodes.windows(2).all(|w| w[0].x + w[0].w + layout::SEQ_GAP == w[1].x));
    let ys: Vec<f32> = (0..d.edges.len() as u32).map(|o| layout::message_y(&d, o)).collect();
    assert!(ys[0] > 64.0 && ys.windows(2).all(|w| w[1] - w[0] == layout::SEQ_ROW));
}

#[test]
fn a_persons_move_survives_and_new_nodes_go_beside() {
    let mut d = relaid(WORKFLOW);
    d.nodes[0].x = 5000.0;
    d.nodes[0].placed = Placed::Person;
    let before = d.clone();
    layout::place_missing(&mut d);
    assert_eq!(d, before, "nothing unplaced: nothing moves");
    d.nodes.push(Node { id: "extra".into(), label: "Extra step".into(), lane: Some("lead".into()), ..Node::default() });
    d.edges.push(Edge { id: "x1".into(), from: "merge".into(), to: "extra".into(), ..Edge::default() });
    check(&mut d).unwrap();
    layout::place_missing(&mut d);
    for (a, b) in before.nodes.iter().zip(&d.nodes) {
        assert_eq!((a.x, a.y), (b.x, b.y), "{} kept its place", a.id);
    }
    let extra = d.node("extra").unwrap();
    assert_eq!(extra.placed, Placed::Layout);
    assert!(extra.x > 5000.0, "right of everything placed");
    assert!(layout::overlaps(&d).is_empty());
    layout::relayout(&mut d);
    assert!(d.nodes[0].x < 5000.0 && d.nodes[0].placed == Placed::Layout, "lay out again moves everything");
}

#[test]
fn a_big_diagram_lays_out_without_overlap() {
    let mut d = parse_v(&flow(MAX_NODES, MAX_EDGES)).unwrap();
    layout::place_missing(&mut d);
    assert!(layout::overlaps(&d).is_empty());
}

#[test]
fn the_port_keeps_upstream_behaviour() {
    use layered::{GEdge, Graph, Spacing};
    let graph = |n: usize, edges: &[(usize, usize)]| Graph {
        sizes: vec![[60.0, 30.0]; n],
        edges: edges.iter().map(|&(from, to)| GEdge { from, to, label: None, minlen: 1 }).collect(),
        node_cluster: vec![None; n],
        clusters: Vec::new(),
        fixed: Vec::new(),
    };
    let sp = Spacing { node: 30.0, rank: 40.0, cluster: 10.0 };
    // Upstream's a_chain_goes_down_in_a_straight_line and a_fork_spreads_children…
    let p = layered::layout(&graph(3, &[(0, 1), (1, 2)]), &sp).unwrap();
    assert!(p.centers[0][1] < p.centers[1][1] && p.centers[1][1] < p.centers[2][1]);
    assert!((p.centers[0][0] - p.centers[2][0]).abs() < 0.5);
    assert_eq!(p.centers[1][1] - p.centers[0][1], 30.0 + 40.0);
    let p = layered::layout(&graph(3, &[(0, 1), (0, 2)]), &sp).unwrap();
    let (a, b) = (p.centers[1], p.centers[2]);
    assert_eq!(a[1], b[1]);
    assert!((a[0] - b[0]).abs() >= 90.0 - 0.01);
    assert!((p.centers[0][0] - (a[0] + b[0]) / 2.0).abs() < 1.0);
    let p = layered::layout(&graph(3, &[(0, 1), (1, 2), (2, 0), (1, 1)]), &sp).unwrap();
    assert!(p.centers.iter().all(|c| c[0].is_finite() && c[1].is_finite()));
}

// ---------------------------------------------------------------------------
// Diff

#[test]
fn the_diff_of_a_known_edit() {
    let base = relaid(LIFECYCLE);
    let mut now = base.clone();
    // Renamed, moved, a node and an edge added, one removed, one reconnected, one relabelled.
    let review = now.nodes.iter_mut().find(|n| n.id == "review").unwrap();
    review.label = "Lead review".into();
    review.y += 40.0;
    now.nodes.push(Node { id: "archived".into(), label: "Archived".into(), shape: Shape::End, lane: Some("exits".into()), w: 96.0, h: 44.0, placed: Placed::Person, ..Node::default() });
    now.edges.push(Edge { id: "p1".into(), from: "merged".into(), to: "archived".into(), label: "later".into(), ..Edge::default() });
    now.edges.retain(|e| !(e.from == "working" && e.to == "stopped"));
    let e = now.edges.iter_mut().find(|e| e.from == "review" && e.to == "merged").unwrap();
    e.to = "stopped".into();
    let id = e.id.clone();
    now.edges.iter_mut().find(|e| e.from == "planned").unwrap().label = "go".into();
    now.nodes.retain(|n| n.id != "blocked");
    now.edges.retain(|e| e.from != "blocked" && e.to != "blocked");
    check(&mut now).unwrap();
    let lines = diff::lines(&diff::diff(&base, &now));
    assert_eq!(
        lines,
        [
            "Renamed node review from \"In review\" to \"Lead review\".".to_string(),
            "Moved \"Lead review\".".into(),
            "Added node \"Archived\" (archived).".into(),
            "Removed node \"Blocked\" (blocked).".into(),
            "Relabelled edge e1 from \"approved\" to \"go\".".into(),
            format!("Reconnected edge {id}: \"In review\" to \"Merged\" is now \"Lead review\" to \"Stopped\"."),
            "Connected \"Merged\" to \"Archived\" labelled \"later\".".into(),
            "Removed the edge from \"Working\" to \"Blocked\" labelled \"asks\".".into(),
            "Removed the edge from \"Blocked\" to \"Working\" labelled \"answered\".".into(),
            "Removed the edge from \"Working\" to \"Stopped\" labelled \"cancelled\".".into(),
        ]
    );
    let (l, json) = diff::read_back(&base, &now);
    assert_eq!(l, lines);
    assert_eq!(parse(&json).unwrap(), now, "the whole diagram reads back");
    assert_eq!(diff::read_back(&base, &base).0, ["No change."]);
}

#[test]
fn many_moves_fold_into_one_line_and_long_diffs_are_cut() {
    let base = relaid(WORKFLOW);
    let mut now = base.clone();
    for n in &mut now.nodes {
        n.x += 100.0;
    }
    let lines = diff::lines(&diff::diff(&base, &now));
    assert_eq!(lines.len(), 1);
    assert!(lines[0].starts_with("Moved 10 nodes (\"Asks for a change\", ") && lines[0].ends_with("and 5 more)."), "{}", lines[0]);
    let base = parse_v(&flow(MAX_NODES, 1)).unwrap();
    let mut now = base.clone();
    for n in &mut now.nodes {
        n.label.push('!');
    }
    let lines = diff::lines(&diff::diff(&base, &now));
    assert_eq!(lines.len(), diff::MAX_LINES);
    assert_eq!(lines.last().unwrap(), "And 141 more changes: read the whole diagram.");
}

#[test]
fn sequence_reorders_and_lane_moves_read_plainly() {
    let base = import::archify(SEQUENCE).unwrap();
    let mut now = base.clone();
    now.edges[0].order = Some(9);
    now.edges[9].order = Some(0);
    let lines = diff::lines(&diff::diff(&base, &now));
    assert!(lines.contains(&"Moved message add labelled \"canvas_add_shots\" from row 1 to row 10.".to_string()), "{lines:?}");
    let base = import::archify(WORKFLOW).unwrap();
    let mut now = base.clone();
    now.nodes[0].lane = Some("lead".into());
    now.lanes[0].label = "The person".into();
    let lines = diff::lines(&diff::diff(&base, &now));
    assert_eq!(lines, ["Renamed lane \"Person\" to \"The person\".", "Moved \"Asks for a change\" to lane \"Lead session\"."]);
}

#[test]
fn a_dataflow_imports_with_stages_as_groups() {
    let json = r#"{ "schema_version": 1, "diagram_type": "dataflow", "meta": { "title": "Events", "output": "x.html" },
      "stages": [{ "label": "Producers" }, { "label": "Store" }],
      "nodes": [{ "id": "api", "type": "backend", "label": "API", "stage": 0, "row": 0 },
                { "id": "db", "type": "database", "label": "Events table", "stage": 1, "row": 0 }],
      "flows": [{ "from": "api", "to": "db", "label": "insert", "classification": "pii" }] }"#;
    let mut d = import::archify(json).unwrap();
    assert_eq!(d.kind, Kind::Dataflow);
    assert_eq!(d.groups.iter().map(|g| (g.id.as_str(), g.label.as_str())).collect::<Vec<_>>(), [("stage-1", "Producers"), ("stage-2", "Store")]);
    assert_eq!(d.node("db").unwrap().shape, Shape::Store);
    layout::place_missing(&mut d);
    assert!(layout::overlaps(&d).is_empty() && d.node("api").unwrap().x < d.node("db").unwrap().x);
}
