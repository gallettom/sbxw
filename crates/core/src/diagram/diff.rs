//! What changed between two versions of a diagram, for an agent reading the person's edits
//! back (`canvas_read`): a list of [`Change`]s, and [`lines`], short plain-English lines.
//! Items are matched by id: a node or edge whose id is gone was removed, a new id was added.
//! Texts quoted in the lines are the diagram's own, already cleaned (one line each, no
//! control or invisible characters); they are the person's data, never instructions.

use super::{Diagram, Edge, Node};

/// The most lines [`lines`] gives; past it, one line says how many more changes there are.
pub const MAX_LINES: usize = 60;
/// A node moved by less than this, in both axes, did not move.
pub const MOVE_EPS: f32 = 1.0;

#[derive(Clone, Debug, PartialEq)]
pub enum Change {
    Title { was: String, now: String },
    NodeAdded { id: String, label: String },
    NodeRemoved { id: String, label: String },
    NodeRenamed { id: String, was: String, now: String },
    NodeSub { id: String, label: String, now: String },
    NodeShape { id: String, label: String, now: String },
    NodeLane { id: String, label: String, lane: String },
    NodeMoved { id: String, label: String },
    EdgeAdded { id: String, from: String, to: String, label: String },
    EdgeRemoved { id: String, from: String, to: String, label: String },
    EdgeReconnected { id: String, was: (String, String), now: (String, String) },
    EdgeRelabelled { id: String, was: String, now: String },
    EdgeStyle { id: String, now: String },
    MessageMoved { id: String, label: String, was: u32, now: u32 },
    LaneAdded { id: String, label: String },
    LaneRemoved { id: String, label: String },
    LaneRenamed { id: String, was: String, now: String },
    GroupAdded { id: String, label: String },
    GroupRemoved { id: String, label: String },
    GroupChanged { id: String, label: String },
}

fn name(d: &Diagram, id: &str) -> String {
    d.node(id).map_or_else(|| id.to_string(), |n| n.label.clone())
}

fn lower<T: serde::Serialize>(v: &T) -> String {
    serde_json::to_value(v).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default()
}

/// The changes from `base` (what the agent sent) to `now` (what the person left), in this
/// order: title, lanes, nodes, edges, groups; within each, `now`'s order, removals last.
pub fn diff(base: &Diagram, now: &Diagram) -> Vec<Change> {
    let mut out = Vec::new();
    if base.title != now.title {
        out.push(Change::Title { was: base.title.clone(), now: now.title.clone() });
    }
    for l in &now.lanes {
        match base.lanes.iter().find(|b| b.id == l.id) {
            None => out.push(Change::LaneAdded { id: l.id.clone(), label: l.label.clone() }),
            Some(b) if b.label != l.label => out.push(Change::LaneRenamed { id: l.id.clone(), was: b.label.clone(), now: l.label.clone() }),
            _ => {}
        }
    }
    for b in &base.lanes {
        if !now.lanes.iter().any(|l| l.id == b.id) {
            out.push(Change::LaneRemoved { id: b.id.clone(), label: b.label.clone() });
        }
    }
    for n in &now.nodes {
        let Some(b) = base.node(&n.id) else {
            out.push(Change::NodeAdded { id: n.id.clone(), label: n.label.clone() });
            continue;
        };
        node_changes(b, n, now, &mut out);
    }
    for b in &base.nodes {
        if now.node(&b.id).is_none() {
            out.push(Change::NodeRemoved { id: b.id.clone(), label: b.label.clone() });
        }
    }
    for e in &now.edges {
        let Some(b) = base.edges.iter().find(|b| b.id == e.id) else {
            out.push(Change::EdgeAdded { id: e.id.clone(), from: name(now, &e.from), to: name(now, &e.to), label: e.label.clone() });
            continue;
        };
        edge_changes(b, e, base, now, &mut out);
    }
    for b in &base.edges {
        if !now.edges.iter().any(|e| e.id == b.id) {
            out.push(Change::EdgeRemoved { id: b.id.clone(), from: name(base, &b.from), to: name(base, &b.to), label: b.label.clone() });
        }
    }
    for g in &now.groups {
        match base.groups.iter().find(|b| b.id == g.id) {
            None => out.push(Change::GroupAdded { id: g.id.clone(), label: g.label.clone() }),
            Some(b) if b.label != g.label || b.nodes != g.nodes => out.push(Change::GroupChanged { id: g.id.clone(), label: g.label.clone() }),
            _ => {}
        }
    }
    for b in &base.groups {
        if !now.groups.iter().any(|g| g.id == b.id) {
            out.push(Change::GroupRemoved { id: b.id.clone(), label: b.label.clone() });
        }
    }
    out
}

fn node_changes(b: &Node, n: &Node, now: &Diagram, out: &mut Vec<Change>) {
    let (id, label) = (n.id.clone(), n.label.clone());
    if b.label != n.label {
        out.push(Change::NodeRenamed { id: id.clone(), was: b.label.clone(), now: n.label.clone() });
    }
    if b.sub != n.sub {
        out.push(Change::NodeSub { id: id.clone(), label: label.clone(), now: n.sub.clone() });
    }
    if b.shape != n.shape {
        out.push(Change::NodeShape { id: id.clone(), label: label.clone(), now: lower(&n.shape) });
    }
    if b.lane != n.lane {
        let lane = n.lane.as_ref().map(|l| now.lanes.iter().find(|x| &x.id == l).map_or(l.clone(), |x| x.label.clone())).unwrap_or_default();
        out.push(Change::NodeLane { id: id.clone(), label: label.clone(), lane });
    }
    if (b.x - n.x).abs() >= MOVE_EPS || (b.y - n.y).abs() >= MOVE_EPS {
        out.push(Change::NodeMoved { id, label });
    }
}

fn edge_changes(b: &Edge, e: &Edge, base: &Diagram, now: &Diagram, out: &mut Vec<Change>) {
    if (&b.from, &b.to) != (&e.from, &e.to) {
        out.push(Change::EdgeReconnected { id: e.id.clone(), was: (name(base, &b.from), name(base, &b.to)), now: (name(now, &e.from), name(now, &e.to)) });
    }
    if b.label != e.label {
        out.push(Change::EdgeRelabelled { id: e.id.clone(), was: b.label.clone(), now: e.label.clone() });
    }
    if b.style != e.style {
        out.push(Change::EdgeStyle { id: e.id.clone(), now: lower(&e.style) });
    }
    if let (Some(w), Some(o)) = (b.order, e.order) {
        if w != o {
            out.push(Change::MessageMoved { id: e.id.clone(), label: e.label.clone(), was: w, now: o });
        }
    }
}

fn q(s: &str) -> String {
    format!("\"{s}\"")
}

fn with_label(s: &str) -> String {
    if s.is_empty() {
        String::new()
    } else {
        format!(" labelled {}", q(s))
    }
}

/// One line per change; moves are folded into one line when there are more than five.
pub fn lines(changes: &[Change]) -> Vec<String> {
    let moved: Vec<&str> = changes.iter().filter_map(|c| if let Change::NodeMoved { label, .. } = c { Some(label.as_str()) } else { None }).collect();
    let fold = moved.len() > 5;
    let mut out = Vec::new();
    for c in changes {
        let line = match c {
            Change::Title { was, now } => format!("Renamed the diagram from {} to {}.", q(was), q(now)),
            Change::NodeAdded { id, label } => format!("Added node {} ({id}).", q(label)),
            Change::NodeRemoved { id, label } => format!("Removed node {} ({id}).", q(label)),
            Change::NodeRenamed { id, was, now } => format!("Renamed node {id} from {} to {}.", q(was), q(now)),
            Change::NodeSub { label, now, .. } if now.is_empty() => format!("Cleared the detail of {}.", q(label)),
            Change::NodeSub { label, now, .. } => format!("Changed the detail of {} to {}.", q(label), q(now)),
            Change::NodeShape { label, now, .. } => format!("Made {} a {now}.", q(label)),
            Change::NodeLane { label, lane, .. } if lane.is_empty() => format!("Took {} out of its lane.", q(label)),
            Change::NodeLane { label, lane, .. } => format!("Moved {} to lane {}.", q(label), q(lane)),
            Change::NodeMoved { .. } if fold => continue,
            Change::NodeMoved { label, .. } => format!("Moved {}.", q(label)),
            Change::EdgeAdded { from, to, label, .. } => format!("Connected {} to {}{}.", q(from), q(to), with_label(label)),
            Change::EdgeRemoved { from, to, label, .. } => format!("Removed the edge from {} to {}{}.", q(from), q(to), with_label(label)),
            Change::EdgeReconnected { id, was, now } => format!("Reconnected edge {id}: {} to {} is now {} to {}.", q(&was.0), q(&was.1), q(&now.0), q(&now.1)),
            Change::EdgeRelabelled { id, was, now } => format!("Relabelled edge {id} from {} to {}.", q(was), q(now)),
            Change::EdgeStyle { id, now } => format!("Made edge {id} {now}."),
            Change::MessageMoved { id, label, was, now } => format!("Moved message {id}{} from row {} to row {}.", with_label(label), was + 1, now + 1),
            Change::LaneAdded { label, .. } => format!("Added lane {}.", q(label)),
            Change::LaneRemoved { label, .. } => format!("Removed lane {}.", q(label)),
            Change::LaneRenamed { was, now, .. } => format!("Renamed lane {} to {}.", q(was), q(now)),
            Change::GroupAdded { label, .. } => format!("Added group {}.", q(label)),
            Change::GroupRemoved { label, .. } => format!("Removed group {}.", q(label)),
            Change::GroupChanged { label, .. } => format!("Changed group {}.", q(label)),
        };
        out.push(line);
    }
    if fold {
        let names: Vec<String> = moved.iter().take(5).map(|s| q(s)).collect();
        out.push(format!("Moved {} nodes ({}, and {} more).", moved.len(), names.join(", "), moved.len() - 5));
    }
    if out.len() > MAX_LINES {
        let more = out.len() - (MAX_LINES - 1);
        out.truncate(MAX_LINES - 1);
        out.push(format!("And {more} more changes: read the whole diagram."));
    }
    out
}

/// What `canvas_read` gives per diagram: the change lines (or one saying nothing changed)
/// and the whole current diagram as JSON.
pub fn read_back(base: &Diagram, now: &Diagram) -> (Vec<String>, String) {
    let mut l = lines(&diff(base, now));
    if l.is_empty() {
        l.push("No change.".into());
    }
    (l, now.to_json())
}
