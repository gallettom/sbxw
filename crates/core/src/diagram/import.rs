//! Archify's JSON (the `archify` skill, schemas v1 and v2) into sbxw's schema.
//!
//! Tolerant: fields sbxw has no use for are ignored (meta beyond the title, cards, sources,
//! icons, brand marks, phases, main paths, activations, segments, routing hints, grid
//! places). Unknown diagram types, and schema versions past 2, are refused with a reason.
//! What is kept:
//!
//! | Archify | sbxw |
//! |---|---|
//! | `workflow`: lanes, nodes (`type`), edges (`variant`, `role`), groups (lane + columns) | workflow, `lr` |
//! | `sequence`: participants, messages (`y`, `variant`) | sequence: lifelines in order, messages ordered by `y` |
//! | `dataflow`: stages, nodes, flows | dataflow, `lr`, one group per stage |
//! | `lifecycle`: lanes, states (`type`), transitions | lifecycle, `lr` |
//! | `architecture`: components, boundaries, connections | workflow, `lr`, a group per boundary |
//!
//! Archify ids (`[a-zA-Z][a-zA-Z0-9_-]*`) are lower-cased; two that then collide, or one
//! still not an sbxw id (too long), are refused. Nothing is placed, every node comes out
//! unplaced for [`super::layout::place_missing`], except architecture components with a
//! `pos` (and `size`): Archify draws those where the author put them, and so does sbxw.

use super::{check, counts, Diagram, Direction, Edge, Group, Kind, Lane, Node, Placed, Shape, Style, MAX_SOURCE_BYTES, VERSION};
use serde_json::Value;

/// Whether `json` looks like Archify's (it has `diagram_type`) rather than sbxw's.
pub fn is_archify(v: &Value) -> bool {
    v.get("diagram_type").is_some()
}

/// Either format: Archify's when it has `diagram_type`, sbxw's otherwise.
pub fn parse_any(json: &str) -> Result<Diagram, String> {
    if json.len() > MAX_SOURCE_BYTES {
        return Err(format!("the diagram is {} KB, at most {} KB", json.len() >> 10, MAX_SOURCE_BYTES >> 10));
    }
    let v: Value = serde_json::from_str(json).map_err(|e| format!("not JSON: {e}"))?;
    if is_archify(&v) {
        archify_value(&v)
    } else {
        super::parse(json)
    }
}

/// An Archify document into a checked sbxw diagram.
pub fn archify(json: &str) -> Result<Diagram, String> {
    if json.len() > MAX_SOURCE_BYTES {
        return Err(format!("the diagram is {} KB, at most {} KB", json.len() >> 10, MAX_SOURCE_BYTES >> 10));
    }
    let v: Value = serde_json::from_str(json).map_err(|e| format!("not JSON: {e}"))?;
    archify_value(&v)
}

fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(Value::as_str).unwrap_or("")
}

fn list<'a>(v: &'a Value, k: &str) -> &'a [Value] {
    v.get(k).and_then(Value::as_array).map_or(&[][..], Vec::as_slice)
}

/// An Archify id as an sbxw id (lower-cased; checked later with every other id).
fn id(v: &Value, k: &str) -> String {
    s(v, k).to_ascii_lowercase()
}

fn component_shape(t: &str) -> Shape {
    match t {
        "database" => Shape::Store,
        "external" => Shape::External,
        _ => Shape::Process,
    }
}

fn variant_style(v: &str) -> Style {
    match v {
        "dashed" | "return" => Style::Dashed,
        "emphasis" | "security" => Style::Emphasis,
        _ => Style::Solid,
    }
}

fn edge(v: &Value, label_key: &str) -> Edge {
    let mut style = variant_style(s(v, "variant"));
    match s(v, "role") {
        "error" => style = Style::Error,
        "async" | "return" if style == Style::Solid => style = Style::Dashed,
        _ => {}
    }
    Edge { id: id(v, "id"), from: id(v, "from"), to: id(v, "to"), label: s(v, label_key).to_string(), style, order: None }
}

fn node(v: &Value, shape: Shape, lane: Option<String>) -> Node {
    Node { id: id(v, "id"), label: s(v, "label").to_string(), sub: s(v, "sublabel").to_string(), shape, lane, placed: Placed::None, ..Node::default() }
}

fn archify_value(v: &Value) -> Result<Diagram, String> {
    let t = s(v, "diagram_type");
    if t.is_empty() {
        return Err("an Archify diagram has a diagram_type".into());
    }
    match v.get("schema_version").and_then(Value::as_u64) {
        Some(1 | 2) => {}
        Some(n) => return Err(format!("Archify schema version {n} is not one sbxw imports (1 and 2)")),
        None => return Err("an Archify diagram has a schema_version".into()),
    }
    let title = v.get("meta").map(|m| s(m, "title")).unwrap_or("").to_string();
    let mut d = Diagram { version: VERSION, id: String::new(), kind: Kind::Workflow, title, direction: Direction::Lr, lanes: Vec::new(), groups: Vec::new(), nodes: Vec::new(), edges: Vec::new(), rev: 0, author: super::Author::Agent, updated_ms: 0 };
    let lanes = |d: &mut Diagram, key: &str| {
        for l in list(v, key) {
            d.lanes.push(Lane { id: id(l, "id"), label: s(l, "label").to_string(), at: 0.0, span: 0.0 });
        }
    };
    match t {
        "workflow" => {
            let (nodes, edges, groups) = (list(v, "nodes"), list(v, "edges"), list(v, "groups"));
            counts(nodes.len(), edges.len(), list(v, "lanes").len(), groups.len())?;
            lanes(&mut d, "lanes");
            for n in nodes {
                d.nodes.push(node(n, component_shape(s(n, "type")), Some(id(n, "lane"))));
            }
            d.edges = edges.iter().map(|e| edge(e, "label")).collect();
            // A group is a lane's columns `fromCol..=toCol`; a node already in one stays there.
            let mut taken = std::collections::HashSet::new();
            for g in groups {
                let (lane, lo, hi) = (id(g, "lane"), g.get("fromCol").and_then(Value::as_i64), g.get("toCol").and_then(Value::as_i64));
                let members: Vec<String> = nodes
                    .iter()
                    .filter(|n| id(n, "lane") == lane)
                    .filter(|n| matches!((n.get("col").and_then(Value::as_i64), lo, hi), (Some(c), Some(lo), Some(hi)) if (lo..=hi).contains(&c)))
                    .map(|n| id(n, "id"))
                    .filter(|n| taken.insert(n.clone()))
                    .collect();
                if !members.is_empty() {
                    d.groups.push(Group { id: id(g, "id"), label: s(g, "label").to_string(), nodes: members });
                }
            }
        }
        "sequence" => {
            d.kind = Kind::Sequence;
            let (parts, msgs) = (list(v, "participants"), list(v, "messages"));
            counts(parts.len(), msgs.len(), 0, 0)?;
            for p in parts {
                let shape = match s(p, "type") {
                    "external" => Shape::Actor,
                    t => component_shape(t),
                };
                d.nodes.push(node(p, shape, None));
            }
            let mut by: Vec<(f64, usize)> = msgs.iter().enumerate().map(|(i, m)| (m.get("y").and_then(Value::as_f64).unwrap_or(i as f64), i)).collect();
            by.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
            let mut order = vec![0u32; msgs.len()];
            for (row, (_, i)) in by.into_iter().enumerate() {
                order[i] = row as u32;
            }
            for (i, m) in msgs.iter().enumerate() {
                let mut e = edge(m, "label");
                e.order = Some(order[i]);
                d.edges.push(e);
            }
        }
        "dataflow" => {
            d.kind = Kind::Dataflow;
            let (stages, nodes, flows) = (list(v, "stages"), list(v, "nodes"), list(v, "flows"));
            counts(nodes.len(), flows.len(), 0, stages.len())?;
            for n in nodes {
                d.nodes.push(node(n, component_shape(s(n, "type")), None));
            }
            d.edges = flows.iter().map(|e| edge(e, "label")).collect();
            for (i, st) in stages.iter().enumerate() {
                let members: Vec<String> = nodes.iter().filter(|n| n.get("stage").and_then(Value::as_u64) == Some(i as u64)).map(|n| id(n, "id")).collect();
                if !members.is_empty() {
                    d.groups.push(Group { id: format!("stage-{}", i + 1), label: s(st, "label").to_string(), nodes: members });
                }
            }
        }
        "lifecycle" => {
            d.kind = Kind::Lifecycle;
            let (states, trans) = (list(v, "states"), list(v, "transitions"));
            counts(states.len(), trans.len(), list(v, "lanes").len(), 0)?;
            lanes(&mut d, "lanes");
            let mut started = false;
            for st in states {
                let shape = match s(st, "type") {
                    // One start: later ones are ordinary states.
                    "start" if !started => {
                        started = true;
                        Shape::Start
                    }
                    "success" | "failure" => Shape::End,
                    "decision" => Shape::Decision,
                    "external" => Shape::External,
                    _ => Shape::Process,
                };
                d.nodes.push(node(st, shape, Some(id(st, "lane"))));
            }
            d.edges = trans.iter().map(|e| edge(e, "label")).collect();
        }
        "architecture" => {
            let (comps, conns, bounds) = (list(v, "components"), list(v, "connections"), list(v, "boundaries"));
            counts(comps.len(), conns.len(), 0, bounds.len())?;
            for c in comps {
                let mut n = node(c, component_shape(s(c, "type")), None);
                // The author's own places (`pos`, `size`) are kept: they are an agent's placing.
                let pair = |k: &str| match list(c, k) {
                    [a, b] => Some((a.as_f64()? as f32, b.as_f64()? as f32)),
                    _ => None,
                };
                if let Some((x, y)) = pair("pos") {
                    (n.x, n.y, n.placed) = (x, y, Placed::Agent);
                    if let Some((w, h)) = pair("size") {
                        (n.w, n.h) = (w, h);
                    }
                }
                d.nodes.push(n);
            }
            d.edges = conns.iter().map(|e| edge(e, "label")).collect();
            // Members that are not components (a boundary may wrap a boundary) are left out.
            let known: std::collections::HashSet<String> = d.nodes.iter().map(|n| n.id.clone()).collect();
            let mut taken = std::collections::HashSet::new();
            for (i, b) in bounds.iter().enumerate() {
                let members: Vec<String> = list(b, "wraps").iter().filter_map(Value::as_str).map(str::to_ascii_lowercase).filter(|n| known.contains(n) && taken.insert(n.clone())).collect();
                if !members.is_empty() {
                    d.groups.push(Group { id: format!("boundary-{}", i + 1), label: s(b, "label").to_string(), nodes: members });
                }
            }
        }
        other => return Err(format!("Archify diagram type \"{}\" is not one sbxw imports (workflow, sequence, dataflow, lifecycle, architecture)", super::shown(other))),
    }
    // Ids lower-cased may collide: say so in Archify's terms rather than as a duplicate.
    let mut seen = std::collections::HashSet::new();
    for n in &d.nodes {
        if !seen.insert(n.id.as_str()) {
            return Err(format!("two nodes have the id \"{}\" once lower-cased", super::shown(&n.id)));
        }
    }
    check(&mut d)?;
    Ok(d)
}
