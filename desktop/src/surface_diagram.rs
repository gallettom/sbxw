//! Diagrams on the canvas: the schema the daemon sends (sbxw-core's
//! `diagram` module, version 1), its limits checked here before a save
//! (the daemon checks again), the drawing of one diagram as items of the
//! canvas world, the hit tests, the person's edits, and undo.
//!
//! * **Data, never controls.** Every label is drawn as text, cleaned as the
//!   core cleans it ([`clean`]: control and invisible characters removed,
//!   trimmed). Ids are never cleaned into shape: an id that is not one the
//!   core accepts is refused.
//! * **Coordinates.** A node's `x, y` are in the diagram's own points; the
//!   canvas places the diagram's (0, 0) at its item's origin. Edges are routed
//!   here at draw time (orthogonal elbows between node sides), never stored.
//! * **Layout.** The desktop does not link the core: "Lay out again" sends
//!   every node without a place ([`Placed::None`], no `x, y`) and the daemon's
//!   core layout (`place_missing`) places them all, as for an agent's diagram.
//! * **Cost.** One element per node, one per edge (its label, or an empty
//!   point for its accessible name), one shape for every line and arrow.
//!   Zoomed far out, labels are left out (unreadable there).

use rune::{el, text, El};
use serde::Deserialize;
use serde_json::{json, Value as J};

// ---------------------------------------------------------------------------
// The schema (crates/core/src/diagram.rs)

pub const VERSION: u32 = 1;
pub const MAX_SOURCE_BYTES: usize = 256 << 10;
pub const MAX_NODES: usize = 200;
pub const MAX_EDGES: usize = 400;
pub const MAX_LANES: usize = 16;
pub const MAX_GROUPS: usize = 32;
pub const MAX_TITLE: usize = 120;
pub const MAX_LABEL: usize = 80;
pub const MAX_SUB: usize = 160;
pub const MAX_ID: usize = 40;
pub const MAX_COORD: f32 = 100_000.0;
pub const MIN_SIDE: f32 = 16.0;
pub const MAX_SIDE: f32 = 2000.0;

/// The core's layout constants (crates/core/src/diagram/layout.rs).
pub const RANK_GAP: f32 = 64.0;
pub const NODE_GAP: f32 = 40.0;
pub const CLUSTER_PAD: f32 = 16.0;
pub const GROUP_TITLE: f32 = 22.0;
pub const LANE_PAD: f32 = 24.0;
pub const LANE_TITLE: f32 = 120.0;
pub const SEQ_GAP: f32 = 60.0;
pub const SEQ_FIRST: f32 = 40.0;
pub const SEQ_ROW: f32 = 44.0;

/// Undo steps kept per diagram.
pub const HISTORY: usize = 100;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    #[default]
    Workflow,
    Sequence,
    Dataflow,
    Lifecycle,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::Workflow => "workflow",
            Kind::Sequence => "sequence",
            Kind::Dataflow => "dataflow",
            Kind::Lifecycle => "lifecycle",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Direction {
    #[default]
    Lr,
    Tb,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Shape {
    #[default]
    Process,
    Decision,
    Start,
    End,
    Store,
    External,
    Actor,
    Note,
}

impl Shape {
    pub const ALL: [Shape; 8] = [Shape::Process, Shape::Decision, Shape::Start, Shape::End, Shape::Store, Shape::External, Shape::Actor, Shape::Note];
    pub fn name(self) -> &'static str {
        match self {
            Shape::Process => "process",
            Shape::Decision => "decision",
            Shape::Start => "start",
            Shape::End => "end",
            Shape::Store => "store",
            Shape::External => "external",
            Shape::Actor => "actor",
            Shape::Note => "note",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Style {
    #[default]
    Solid,
    Dashed,
    Emphasis,
    Error,
}

impl Style {
    pub const ALL: [Style; 4] = [Style::Solid, Style::Dashed, Style::Emphasis, Style::Error];
    pub fn name(self) -> &'static str {
        match self {
            Style::Solid => "solid",
            Style::Dashed => "dashed",
            Style::Emphasis => "emphasis",
            Style::Error => "error",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Placed {
    #[default]
    None,
    Layout,
    Agent,
    Person,
}

impl Placed {
    fn name(self) -> &'static str {
        match self {
            Placed::None => "none",
            Placed::Layout => "layout",
            Placed::Agent => "agent",
            Placed::Person => "person",
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Lane {
    pub id: String,
    pub label: String,
    pub at: f32,
    pub span: f32,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Group {
    pub id: String,
    pub label: String,
    pub nodes: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Node {
    pub id: String,
    pub label: String,
    pub sub: String,
    pub shape: Shape,
    pub lane: Option<String>,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub placed: Placed,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Edge {
    pub id: String,
    pub from: String,
    pub to: String,
    pub label: String,
    pub style: Style,
    pub order: Option<u32>,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Diagram {
    pub version: u32,
    pub id: String,
    pub kind: Kind,
    pub title: String,
    pub direction: Direction,
    pub lanes: Vec<Lane>,
    pub groups: Vec<Group>,
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    pub rev: u32,
    /// `agent` or `person`: kept as the daemon sent it.
    pub author: String,
    pub updated_ms: u64,
}

impl Diagram {
    pub fn node(&self, id: &str) -> Option<&Node> {
        self.nodes.iter().find(|n| n.id == id)
    }
    fn ni(&self, id: &str) -> Option<usize> {
        self.nodes.iter().position(|n| n.id == id)
    }
    pub fn edge(&self, id: &str) -> Option<&Edge> {
        self.edges.iter().find(|e| e.id == id)
    }
    pub fn lr(&self) -> bool {
        self.direction == Direction::Lr || self.kind == Kind::Sequence
    }
    /// A node's label for a sentence (an edge's accessible name, a notice).
    pub fn label_of(&self, id: &str) -> String {
        self.node(id).map(|n| n.label.clone()).unwrap_or_else(|| "a removed node".into())
    }
}

impl serde::Serialize for Diagram {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        wire(self).serialize(s)
    }
}

/// The whole diagram as the save route takes it (`POST
/// /api/canvas/:id/diagrams/:did`): sbxw's schema, no field the core does not
/// know. A node without a place ([`Placed::None`]) goes without `x`, `y` and
/// `placed`, so the daemon lays it out.
pub fn wire(d: &Diagram) -> J {
    let nodes: Vec<J> = d
        .nodes
        .iter()
        .map(|n| {
            let mut o = json!({ "id": n.id, "label": n.label, "shape": n.shape.name(), "w": n.w, "h": n.h });
            if !n.sub.is_empty() {
                o["sub"] = n.sub.clone().into();
            }
            if let Some(l) = &n.lane {
                o["lane"] = l.clone().into();
            }
            if n.placed != Placed::None {
                o["x"] = n.x.into();
                o["y"] = n.y.into();
                o["placed"] = n.placed.name().into();
            }
            o
        })
        .collect();
    let edges: Vec<J> = d
        .edges
        .iter()
        .map(|e| {
            let mut o = json!({ "id": e.id, "from": e.from, "to": e.to, "style": e.style.name() });
            if !e.label.is_empty() {
                o["label"] = e.label.clone().into();
            }
            if let Some(k) = e.order {
                o["order"] = k.into();
            }
            o
        })
        .collect();
    let lanes: Vec<J> = d.lanes.iter().map(|l| json!({ "id": l.id, "label": l.label, "at": l.at, "span": l.span })).collect();
    let groups: Vec<J> = d.groups.iter().map(|g| json!({ "id": g.id, "label": g.label, "nodes": g.nodes })).collect();
    let mut o = json!({ "version": VERSION, "kind": d.kind.name(), "title": d.title, "direction": if d.direction == Direction::Tb { "tb" } else { "lr" },
        "lanes": lanes, "groups": groups, "nodes": nodes, "edges": edges, "rev": d.rev, "updated_ms": d.updated_ms });
    if !d.id.is_empty() {
        o["id"] = d.id.clone().into();
    }
    if d.author == "agent" || d.author == "person" {
        o["author"] = d.author.clone().into();
    }
    o
}

// ---------------------------------------------------------------------------
// Texts and limits

/// A text as the core cleans canvas texts: control characters (new lines
/// too) and the shared invisible list removed, trimmed.
pub fn clean(s: &str) -> String {
    let t: String = s.chars().filter(|c| !c.is_control() && !crate::safe::is_invisible(*c)).collect();
    t.trim().to_string()
}

/// Every text of `d` cleaned and cut to its limit, as the core gives them:
/// run on what the daemon sends, so whatever arrives is drawn as plain
/// one-line text.
pub fn tidy(d: &mut Diagram) {
    let cut = |s: &str, max: usize| clean(s).chars().take(max).collect::<String>();
    d.title = cut(&d.title, MAX_TITLE);
    for l in &mut d.lanes {
        l.label = cut(&l.label, MAX_LABEL);
    }
    for g in &mut d.groups {
        g.label = cut(&g.label, MAX_LABEL);
    }
    for n in &mut d.nodes {
        n.label = cut(&n.label, MAX_LABEL);
        n.sub = cut(&n.sub, MAX_SUB);
    }
    for e in &mut d.edges {
        e.label = cut(&e.label, MAX_LABEL);
    }
}

/// An item id the core accepts: `[a-z0-9][a-z0-9_-]*`, at most 40 bytes.
pub fn valid_id(id: &str) -> bool {
    let b = id.as_bytes();
    !b.is_empty() && b.len() <= MAX_ID && (b[0].is_ascii_lowercase() || b[0].is_ascii_digit()) && b.iter().all(|&c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'_' || c == b'-')
}

/// A diagram id as the daemon makes them: `d-` and 12 lower-case hex digits.
pub fn valid_diagram_id(id: &str) -> bool {
    id.len() == 14 && id.starts_with("d-") && id[2..].bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn len(s: &str) -> usize {
    s.chars().count()
}

/// Every rule of the core's `check`, on a diagram the person changed: the
/// counts, the text lengths (cleaned), the ids, the bounds, the references,
/// the rules of each kind, and the size the save route takes. Refused with
/// a reason a person reads; nothing is cut to fit.
pub fn check(d: &Diagram) -> Result<(), String> {
    for (what, n, max) in [("nodes", d.nodes.len(), MAX_NODES), ("edges", d.edges.len(), MAX_EDGES), ("lanes", d.lanes.len(), MAX_LANES), ("groups", d.groups.len(), MAX_GROUPS)] {
        if n > max {
            return Err(format!("{n} {what}: a diagram holds at most {max}"));
        }
    }
    if len(&clean(&d.title)) > MAX_TITLE {
        return Err(format!("the title is {} characters, at most {MAX_TITLE}", len(&clean(&d.title))));
    }
    let coord = |what: &str, v: f32| if v.is_finite() && v.abs() <= MAX_COORD { Ok(()) } else { Err(format!("{what} is out of bounds (within ±{MAX_COORD})")) };
    let mut lanes = std::collections::HashSet::new();
    for l in &d.lanes {
        if !valid_id(&l.id) || !lanes.insert(l.id.as_str()) {
            return Err(format!("lane id \"{}\" is not one the daemon takes", crate::feat::monitor::attach::clipped_text(&l.id, 24)));
        }
        let t = clean(&l.label);
        if t.is_empty() || len(&t) > MAX_LABEL {
            return Err(format!("a lane's label is 1 to {MAX_LABEL} characters ({} given)", len(&t)));
        }
        coord("a lane's place", l.at)?;
        if l.span != 0.0 && !(l.span.is_finite() && (MIN_SIDE..=MAX_COORD).contains(&l.span)) {
            return Err("a lane's band is out of bounds".into());
        }
    }
    let mut ids = std::collections::HashSet::new();
    let mut starts = 0;
    for n in &d.nodes {
        if !valid_id(&n.id) {
            return Err(format!("node id \"{}\" is not one the daemon takes", crate::feat::monitor::attach::clipped_text(&n.id, 24)));
        }
        if !ids.insert(n.id.as_str()) {
            return Err(format!("node id \"{}\" is used twice", n.id));
        }
        let t = clean(&n.label);
        if t.is_empty() {
            return Err("a node needs a label".into());
        }
        if len(&t) > MAX_LABEL {
            return Err(format!("a node's label is {} characters, at most {MAX_LABEL}", len(&t)));
        }
        if len(&clean(&n.sub)) > MAX_SUB {
            return Err(format!("a node's second line is {} characters, at most {MAX_SUB}", len(&clean(&n.sub))));
        }
        match &n.lane {
            Some(l) if !lanes.contains(l.as_str()) => return Err(format!("\"{t}\" names a lane the diagram does not have")),
            None if !d.lanes.is_empty() => return Err(format!("\"{t}\" has no lane: with lanes, every node is in one")),
            _ => {}
        }
        if n.placed != Placed::None {
            coord(&format!("\"{t}\""), n.x)?;
            coord(&format!("\"{t}\""), n.y)?;
        }
        for v in [n.w, n.h] {
            if !(v.is_finite() && (MIN_SIDE..=MAX_SIDE).contains(&v)) {
                return Err(format!("\"{t}\" is not {MIN_SIDE} to {MAX_SIDE} points a side"));
            }
        }
        if n.shape == Shape::Start {
            starts += 1;
        }
    }
    if d.kind == Kind::Lifecycle && starts > 1 {
        return Err(format!("a lifecycle has at most one start state ({starts} now)"));
    }
    let mut grouped = std::collections::HashSet::new();
    let mut gids = std::collections::HashSet::new();
    for g in &d.groups {
        if !valid_id(&g.id) || !gids.insert(g.id.as_str()) {
            return Err("a group id is not one the daemon takes".into());
        }
        if len(&clean(&g.label)) > MAX_LABEL {
            return Err(format!("a group's label is at most {MAX_LABEL} characters"));
        }
        let mut lane: Option<&Option<String>> = None;
        for id in &g.nodes {
            let Some(n) = d.node(id) else { return Err(format!("group \"{}\" names a node the diagram does not have", clean(&g.label))) };
            if !grouped.insert(id.as_str()) {
                return Err(format!("\"{}\" is in two groups", n.label));
            }
            match lane {
                None => lane = Some(&n.lane),
                Some(l) if *l != n.lane => return Err(format!("group \"{}\" spans two lanes: a group's nodes sit in one lane", clean(&g.label))),
                _ => {}
            }
        }
    }
    let mut eids = std::collections::HashSet::new();
    for e in &d.edges {
        if !valid_id(&e.id) || !eids.insert(e.id.as_str()) {
            return Err("an edge id is not one the daemon takes".into());
        }
        if !ids.contains(e.from.as_str()) || !ids.contains(e.to.as_str()) {
            return Err("an edge names a node the diagram does not have".into());
        }
        if e.from == e.to && !matches!(d.kind, Kind::Sequence | Kind::Lifecycle) {
            return Err(format!("\"{}\" cannot link to itself: only sequence and lifecycle diagrams have such edges", d.label_of(&e.from)));
        }
        if len(&clean(&e.label)) > MAX_LABEL {
            return Err(format!("an edge's label is {} characters, at most {MAX_LABEL}", len(&clean(&e.label))));
        }
        match (d.kind, e.order) {
            (Kind::Sequence, None) => return Err("every message of a sequence has a row".into()),
            (Kind::Sequence, Some(o)) if o as usize >= MAX_EDGES => return Err("a message's row is out of bounds".into()),
            (Kind::Sequence, _) | (_, None) => {}
            (_, Some(_)) => return Err("only a sequence's messages have rows".into()),
        }
    }
    if d.kind == Kind::Sequence && (!d.lanes.is_empty() || !d.groups.is_empty()) {
        return Err("a sequence has no lanes or groups".into());
    }
    let bytes = wire(d).to_string().len();
    if bytes > MAX_SOURCE_BYTES {
        return Err(format!("the diagram would be {} KB, at most {} KB", bytes >> 10, MAX_SOURCE_BYTES >> 10));
    }
    Ok(())
}

/// A node's size from its texts, as the core estimates it.
pub fn estimate(label: &str, sub: &str, shape: Shape) -> (f32, f32) {
    let w = (len(label) as f32 * 7.5 + 32.0).max(len(sub) as f32 * 6.5 + 32.0).clamp(96.0, 280.0);
    let h = if sub.is_empty() { 44.0 } else { 64.0 };
    let (w, h) = if shape == Shape::Decision { (w + 32.0, h + 16.0) } else { (w, h) };
    ((w / 8.0).ceil() * 8.0, h)
}

// ---------------------------------------------------------------------------
// Geometry (the diagram's own points)

/// The `y` of a sequence message's row (the core's `message_y`).
pub fn message_y(d: &Diagram, order: u32) -> f32 {
    let head = d.nodes.iter().map(|n| n.y + n.h).fold(0.0, f32::max);
    head + SEQ_FIRST + order as f32 * SEQ_ROW
}

/// Where a sequence's lifelines end.
fn lifeline_end(d: &Diagram) -> f32 {
    let last = d.edges.iter().filter_map(|e| e.order).max();
    match last {
        Some(o) => message_y(d, o) + SEQ_ROW,
        None => d.nodes.iter().map(|n| n.y + n.h).fold(0.0, f32::max) + SEQ_FIRST + SEQ_ROW,
    }
}

/// The extent of the nodes along the flow: (min, max) of x in `lr`, of y in `tb`.
fn along(d: &Diagram) -> (f32, f32) {
    let lr = d.lr();
    d.nodes.iter().map(|n| if lr { (n.x, n.x + n.w) } else { (n.y, n.y + n.h) }).fold((f32::MAX, f32::MIN), |a, b| (a.0.min(b.0), a.1.max(b.1)))
}

/// A lane's band, `[x, y, w, h]`: across the flow from its `at, span`, along
/// it from the title's room before the first node to past the last.
pub fn lane_rect(d: &Diagram, l: &Lane) -> [f32; 4] {
    let (a0, a1) = along(d);
    let (a0, a1) = if a0 > a1 { (0.0, LANE_TITLE) } else { ((a0 - LANE_TITLE).min(0.0), a1 + LANE_PAD) };
    let span = if l.span > 0.0 { l.span } else { 80.0 };
    if d.lr() {
        [a0, l.at, a1 - a0, span]
    } else {
        [l.at, a0, span, a1 - a0]
    }
}

/// A group's frame around its members, with room for its title above them.
pub fn group_rect(d: &Diagram, g: &Group) -> Option<[f32; 4]> {
    let mut it = g.nodes.iter().filter_map(|id| d.node(id));
    let f = it.next()?;
    let (mut x0, mut y0, mut x1, mut y1) = (f.x, f.y, f.x + f.w, f.y + f.h);
    for n in it {
        (x0, y0, x1, y1) = (x0.min(n.x), y0.min(n.y), x1.max(n.x + n.w), y1.max(n.y + n.h));
    }
    Some([x0 - CLUSTER_PAD, y0 - CLUSTER_PAD - GROUP_TITLE, x1 - x0 + 2.0 * CLUSTER_PAD, y1 - y0 + 2.0 * CLUSTER_PAD + GROUP_TITLE])
}

/// The box of everything drawn: nodes, lane bands, group frames, a
/// sequence's lifelines and self messages. `[x, y, w, h]`, at least 1 x 1.
pub fn content_box(d: &Diagram) -> [f32; 4] {
    let mut b = [f32::MAX, f32::MAX, f32::MIN, f32::MIN];
    let add = |b: &mut [f32; 4], r: [f32; 4]| {
        *b = [b[0].min(r[0]), b[1].min(r[1]), b[2].max(r[0] + r[2]), b[3].max(r[1] + r[3])];
    };
    for n in &d.nodes {
        add(&mut b, [n.x, n.y, n.w, n.h]);
    }
    for l in &d.lanes {
        add(&mut b, lane_rect(d, l));
    }
    for g in &d.groups {
        if let Some(r) = group_rect(d, g) {
            add(&mut b, r);
        }
    }
    if d.kind == Kind::Sequence && !d.nodes.is_empty() {
        let x1 = d.nodes.iter().map(|n| n.x + n.w).fold(f32::MIN, f32::max);
        let r = [b[0], b[1], (x1 + SELF_LOOP + 8.0 - b[0]).max(1.0), lifeline_end(d) - b[1]];
        add(&mut b, r);
    }
    if b[0] > b[2] {
        return [0.0, 0.0, 1.0, 1.0];
    }
    [b[0], b[1], (b[2] - b[0]).max(1.0), (b[3] - b[1]).max(1.0)]
}

/// How far a self edge loops out of its node.
const SELF_LOOP: f32 = 28.0;

/// An edge's path (polyline, the diagram's points) and the point its label sits on.
#[derive(Debug, Clone, PartialEq)]
pub struct Route {
    pub points: Vec<[f32; 2]>,
    pub label_at: [f32; 2],
}

fn centre(n: &Node) -> [f32; 2] {
    [n.x + n.w / 2.0, n.y + n.h / 2.0]
}

/// Edge `e`'s route: a sequence message is a straight line on its row
/// between two lifelines; otherwise orthogonal elbows from the side of the
/// first node facing the second to the side of the second facing the
/// first, the label on the middle segment. A self edge loops out of the
/// node's corner.
pub fn route(d: &Diagram, e: &Edge) -> Option<Route> {
    let a = d.node(&e.from)?;
    let b = d.node(&e.to)?;
    if d.kind == Kind::Sequence {
        let y = message_y(d, e.order.unwrap_or(0));
        let (xa, xb) = (centre(a)[0], centre(b)[0]);
        if e.from == e.to {
            let pts = vec![[xa, y], [xa + SELF_LOOP, y], [xa + SELF_LOOP, y + 16.0], [xa, y + 16.0]];
            return Some(Route { points: pts, label_at: [xa + SELF_LOOP + 4.0, y - 10.0] });
        }
        return Some(Route { points: vec![[xa, y], [xb, y]], label_at: [(xa + xb) / 2.0, y - 10.0] });
    }
    if e.from == e.to {
        let (r, t) = (a.x + a.w, a.y);
        let pts = vec![[r - 16.0, t], [r - 16.0, t - SELF_LOOP], [r + SELF_LOOP, t - SELF_LOOP], [r + SELF_LOOP, t + 16.0], [r, t + 16.0]];
        return Some(Route { points: pts, label_at: [r + SELF_LOOP / 2.0, t - SELF_LOOP - 10.0] });
    }
    let (ca, cb) = (centre(a), centre(b));
    let (dx, dy) = (cb[0] - ca[0], cb[1] - ca[1]);
    // Horizontal sides when the nodes are side by side (the gap between
    // them along x is larger than along y), else top and bottom.
    let gap_x = (b.x - (a.x + a.w)).max(a.x - (b.x + b.w));
    let gap_y = (b.y - (a.y + a.h)).max(a.y - (b.y + b.h));
    let horizontal = if gap_x > 0.0 || gap_y > 0.0 { gap_x >= gap_y } else { dx.abs() * a.h.max(b.h) >= dy.abs() * a.w.max(b.w) };
    let pts = if horizontal {
        let (p0, p3) = if dx >= 0.0 { ([a.x + a.w, ca[1]], [b.x, cb[1]]) } else { ([a.x, ca[1]], [b.x + b.w, cb[1]]) };
        let m = ((p0[0] + p3[0]) / 2.0).round();
        if (p0[1] - p3[1]).abs() < 0.5 { vec![p0, p3] } else { vec![p0, [m, p0[1]], [m, p3[1]], p3] }
    } else {
        let (p0, p3) = if dy >= 0.0 { ([ca[0], a.y + a.h], [cb[0], b.y]) } else { ([ca[0], a.y], [cb[0], b.y + b.h]) };
        let m = ((p0[1] + p3[1]) / 2.0).round();
        if (p0[0] - p3[0]).abs() < 0.5 { vec![p0, p3] } else { vec![p0, [p0[0], m], [p3[0], m], p3] }
    };
    // The label on the middle of the path's middle segment.
    let k = (pts.len() - 1) / 2;
    let (s, t) = (pts[k], pts[k + 1]);
    Some(Route { label_at: [(s[0] + t[0]) / 2.0, (s[1] + t[1]) / 2.0], points: pts })
}

/// The size an edge label takes (the core's estimate).
pub fn label_size(s: &str) -> [f32; 2] {
    [len(s) as f32 * 6.5 + 16.0, 20.0]
}

/// The handle a new edge is dragged from: the selected node's side along
/// the flow (right in `lr`, bottom in `tb`).
pub fn handle_at(d: &Diagram, n: &Node) -> [f32; 2] {
    if d.lr() {
        [n.x + n.w, n.y + n.h / 2.0]
    } else {
        [n.x + n.w / 2.0, n.y + n.h]
    }
}

fn seg_dist(p: [f32; 2], a: [f32; 2], b: [f32; 2]) -> f32 {
    let (vx, vy) = (b[0] - a[0], b[1] - a[1]);
    let l2 = vx * vx + vy * vy;
    let t = if l2 > 0.0 { (((p[0] - a[0]) * vx + (p[1] - a[1]) * vy) / l2).clamp(0.0, 1.0) } else { 0.0 };
    (p[0] - a[0] - t * vx).hypot(p[1] - a[1] - t * vy)
}

/// What is under point `p` (the diagram's points) of a diagram.
#[derive(Debug, Clone, PartialEq)]
pub enum Under {
    Node(usize),
    Edge(usize),
}

/// The node under `p` (the last drawn wins), else the edge within `tol` of
/// it or under its label.
pub fn under(d: &Diagram, p: [f32; 2], tol: f32) -> Option<Under> {
    if let Some(k) = d.nodes.iter().rposition(|n| p[0] >= n.x && p[0] <= n.x + n.w && p[1] >= n.y && p[1] <= n.y + n.h) {
        return Some(Under::Node(k));
    }
    let mut best: Option<(f32, usize)> = None;
    for (k, e) in d.edges.iter().enumerate() {
        let Some(r) = route(d, e) else { continue };
        let mut dist = r.points.windows(2).map(|w| seg_dist(p, w[0], w[1])).fold(f32::MAX, f32::min);
        if !e.label.is_empty() {
            let s = label_size(&e.label);
            if (p[0] - r.label_at[0]).abs() <= s[0] / 2.0 && (p[1] - r.label_at[1]).abs() <= s[1] / 2.0 {
                dist = 0.0;
            }
        }
        if dist <= tol && best.is_none_or(|b| dist < b.0) {
            best = Some((dist, k));
        }
    }
    best.map(|(_, k)| Under::Edge(k))
}

/// The row a message dragged to `y` takes (0 to the last row).
pub fn row_at(d: &Diagram, y: f32) -> u32 {
    let n = d.edges.len().max(1) as f32;
    (((y - message_y(d, 0)) / SEQ_ROW).round()).clamp(0.0, n - 1.0) as u32
}

// ---------------------------------------------------------------------------
// Selection

/// A part of a diagram, as the canvas's selection names it:
/// `<diagram id>` (the whole), `<diagram id>:n:<node>`, `<diagram id>:e:<edge>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Part {
    Whole,
    Node(String),
    Edge(String),
}

impl Part {
    pub fn key(&self, did: &str) -> String {
        match self {
            Part::Whole => did.to_string(),
            Part::Node(n) => format!("{did}:n:{n}"),
            Part::Edge(e) => format!("{did}:e:{e}"),
        }
    }
}

/// `sel` read as a diagram's part: its diagram id and the part.
pub fn parse_sel(sel: &str) -> Option<(&str, Part)> {
    let (did, rest) = match sel.split_once(':') {
        Some((d, r)) => (d, Some(r)),
        None => (sel, None),
    };
    if !valid_diagram_id(did) {
        return None;
    }
    let part = match rest {
        None => Part::Whole,
        Some(r) => match r.split_once(':') {
            Some(("n", id)) if valid_id(id) => Part::Node(id.to_string()),
            Some(("e", id)) if valid_id(id) => Part::Edge(id.to_string()),
            _ => return None,
        },
    };
    Some((did, part))
}

// ---------------------------------------------------------------------------
// Edits: each on a copy, checked whole before it is kept

/// A change the person made on the surface or with a key.
#[derive(Debug, Clone, PartialEq)]
pub enum Op {
    /// A node dragged to (x, y), the diagram's points (a lifeline: x only).
    Move { node: String, x: f32, y: f32 },
    /// An edge dragged from one node's handle to another node.
    Link { from: String, to: String },
    /// From a node's handle to empty ground: a new node there, linked.
    LinkNew { from: String, x: f32, y: f32 },
    /// A sequence message dragged to another row.
    Reorder { edge: String, order: u32 },
    /// A new node centred on (x, y).
    AddAt { x: f32, y: f32 },
    /// A new node right of (or below) the selected one, linked to it (Tab).
    Successor { from: String },
    /// The part's new label (Enter in the inline field).
    Rename { part: Part, text: String },
    /// The part removed (a node takes its edges).
    Delete { part: Part },
    /// Every node without a place: the daemon lays the diagram out again.
    Relayout,
}

fn fresh_id(taken: impl Iterator<Item = String>, prefix: char) -> String {
    let taken: std::collections::HashSet<String> = taken.collect();
    (1..).map(|k| format!("{prefix}{k}")).find(|i| !taken.contains(i)).unwrap_or_default()
}

/// The lane whose band holds point `p`, if any.
fn lane_at(d: &Diagram, p: [f32; 2]) -> Option<String> {
    // Across the flow only: a band runs on along the flow past its last node.
    let a = if d.lr() { p[1] } else { p[0] };
    d.lanes.iter().find(|l| l.span > 0.0 && a >= l.at && a <= l.at + l.span).map(|l| l.id.clone())
}

/// Each lane's band refitted around its members, as the core does after a
/// node is placed (`fit_lanes`). A lane left empty keeps its band.
pub fn fit_lanes(d: &mut Diagram) {
    let lr = d.lr();
    let nodes = d.nodes.clone();
    for l in &mut d.lanes {
        let ext = nodes.iter().filter(|n| n.lane.as_deref() == Some(&l.id)).map(|n| if lr { (n.y, n.y + n.h) } else { (n.x, n.x + n.w) }).fold((f32::MAX, f32::MIN), |a, b| (a.0.min(b.0), a.1.max(b.1)));
        if ext.0 <= ext.1 {
            l.at = (ext.0 - LANE_PAD).round();
            l.span = (ext.1 - ext.0 + 2.0 * LANE_PAD).round();
        }
    }
}

fn renumber(d: &mut Diagram) {
    if d.kind != Kind::Sequence {
        return;
    }
    let mut by: Vec<usize> = (0..d.edges.len()).collect();
    by.sort_by_key(|&i| (d.edges[i].order, i));
    for (row, i) in by.into_iter().enumerate() {
        d.edges[i].order = Some(row as u32);
    }
}

fn new_node(d: &Diagram, label: &str, cx: f32, cy: f32, lane: Option<String>) -> Node {
    let (w, h) = estimate(label, "", Shape::Process);
    let id = fresh_id(d.nodes.iter().map(|n| n.id.clone()), 'n');
    let lane = if d.lanes.is_empty() { None } else { lane.or_else(|| lane_at(d, [cx, cy])).or_else(|| d.lanes.first().map(|l| l.id.clone())) };
    let y = if d.kind == Kind::Sequence { d.nodes.first().map_or(0.0, |n| n.y) } else { (cy - h / 2.0).round() };
    Node { id, label: label.to_string(), sub: String::new(), shape: Shape::Process, lane, x: (cx - w / 2.0).round(), y, w, h, placed: Placed::Person }
}

fn new_edge(d: &Diagram, from: &str, to: &str) -> Edge {
    let id = fresh_id(d.edges.iter().map(|e| e.id.clone()), 'e');
    let order = (d.kind == Kind::Sequence).then(|| d.edges.iter().filter_map(|e| e.order).max().map_or(0, |o| o + 1));
    Edge { id, from: from.to_string(), to: to.to_string(), label: String::new(), style: Style::Solid, order }
}

/// The label a new node starts with (renamed at once).
pub const NEW_LABEL: &str = "New step";

/// What an edit made: the diagram, and the part to select after it (a new
/// node is selected, and renamed at once when `rename`).
#[derive(Debug, Clone, PartialEq)]
pub struct Done {
    pub d: Diagram,
    pub select: Option<Part>,
    pub rename: bool,
}

/// Applies `op` to a copy of `d` and checks the whole result: an edit that
/// breaks a limit or a rule is refused, `d` unchanged.
pub fn apply(d: &Diagram, op: &Op) -> Result<Done, String> {
    let mut n = d.clone();
    let mut select = None;
    let mut rename = false;
    match op {
        Op::Move { node, x, y } => {
            let k = n.ni(node).ok_or("that node is gone")?;
            let seq = n.kind == Kind::Sequence;
            let nd = &mut n.nodes[k];
            nd.x = x.round();
            if !seq {
                nd.y = y.round();
            }
            nd.placed = Placed::Person;
            // Dropped in another lane's band (and not held to a group's): it moves there.
            if !n.lanes.is_empty() && !n.groups.iter().any(|g| g.nodes.contains(node)) {
                let c = centre(&n.nodes[k]);
                if let Some(l) = lane_at(d, c) {
                    n.nodes[k].lane = Some(l);
                }
            }
            if seq {
                // Lifelines stay in their left-to-right order.
                n.nodes.sort_by(|a, b| a.x.total_cmp(&b.x));
            }
            fit_lanes(&mut n);
            select = Some(Part::Node(node.clone()));
        }
        Op::Link { from, to } => {
            if n.node(from).is_none() || n.node(to).is_none() {
                return Err("that node is gone".into());
            }
            if n.kind != Kind::Sequence && n.edges.iter().any(|e| &e.from == from && &e.to == to) {
                return Err(format!("\"{}\" already links to \"{}\"", n.label_of(from), n.label_of(to)));
            }
            let e = new_edge(&n, from, to);
            select = Some(Part::Edge(e.id.clone()));
            n.edges.push(e);
        }
        Op::LinkNew { from, x, y } => {
            let lane = n.node(from).ok_or("that node is gone")?.lane.clone();
            let lane = if n.lanes.is_empty() { None } else { lane_at(&n, [*x, *y]).or(lane) };
            let nd = new_node(&n, NEW_LABEL, *x, *y, lane);
            let e = new_edge(&n, from, &nd.id);
            select = Some(Part::Node(nd.id.clone()));
            rename = true;
            n.nodes.push(nd);
            n.edges.push(e);
            if n.kind == Kind::Sequence {
                n.nodes.sort_by(|a, b| a.x.total_cmp(&b.x));
            }
            fit_lanes(&mut n);
        }
        Op::Reorder { edge, order } => {
            if n.kind != Kind::Sequence {
                return Err("only a sequence's messages have rows".into());
            }
            let k = n.edges.iter().position(|e| &e.id == edge).ok_or("that message is gone")?;
            let mut rows: Vec<usize> = (0..n.edges.len()).filter(|&i| i != k).collect();
            rows.sort_by_key(|&i| (n.edges[i].order, i));
            let at = (*order as usize).min(rows.len());
            rows.insert(at, k);
            for (row, i) in rows.into_iter().enumerate() {
                n.edges[i].order = Some(row as u32);
            }
            select = Some(Part::Edge(edge.clone()));
        }
        Op::AddAt { x, y } => {
            let nd = new_node(&n, NEW_LABEL, *x, *y, None);
            select = Some(Part::Node(nd.id.clone()));
            rename = true;
            n.nodes.push(nd);
            if n.kind == Kind::Sequence {
                n.nodes.sort_by(|a, b| a.x.total_cmp(&b.x));
            }
            fit_lanes(&mut n);
        }
        Op::Successor { from } => {
            let a = n.node(from).ok_or("that node is gone")?.clone();
            let (w, h) = estimate(NEW_LABEL, "", Shape::Process);
            let seq = n.kind == Kind::Sequence;
            // Right of it (below in `tb`), then further across until nothing is in the way.
            let (mut cx, mut cy) = if seq {
                let right = n.nodes.iter().map(|q| q.x + q.w).fold(a.x + a.w, f32::max);
                (right + SEQ_GAP + w / 2.0, a.y + h / 2.0)
            } else if n.lr() {
                (a.x + a.w + RANK_GAP + w / 2.0, a.y + a.h / 2.0)
            } else {
                (a.x + a.w / 2.0, a.y + a.h + RANK_GAP + h / 2.0)
            };
            let clash = |cx: f32, cy: f32, n: &Diagram| n.nodes.iter().any(|q| cx - w / 2.0 < q.x + q.w && cx + w / 2.0 > q.x && cy - h / 2.0 < q.y + q.h && cy + h / 2.0 > q.y);
            for _ in 0..MAX_NODES {
                if !clash(cx, cy, &n) {
                    break;
                }
                if n.lr() && !seq {
                    cy += h + NODE_GAP;
                } else {
                    cx += w + NODE_GAP;
                }
            }
            let nd = new_node(&n, NEW_LABEL, cx, cy, a.lane.clone());
            let e = new_edge(&n, from, &nd.id);
            select = Some(Part::Node(nd.id.clone()));
            rename = true;
            n.nodes.push(nd);
            n.edges.push(e);
            if seq {
                n.nodes.sort_by(|a, b| a.x.total_cmp(&b.x));
            }
            fit_lanes(&mut n);
        }
        Op::Rename { part, text } => {
            let t = clean(text);
            match part {
                Part::Node(id) => {
                    if t.is_empty() {
                        return Err("a node needs a label: type one, or press Escape to keep the old one".into());
                    }
                    if len(&t) > MAX_LABEL {
                        return Err(format!("a label is at most {MAX_LABEL} characters (this one is {})", len(&t)));
                    }
                    let k = n.ni(id).ok_or("that node is gone")?;
                    let nd = &mut n.nodes[k];
                    nd.label = t;
                    // Wider for a longer label, never narrower than it was.
                    let (w, _) = estimate(&nd.label, &nd.sub, nd.shape);
                    nd.w = nd.w.max(w).min(MAX_SIDE);
                    select = Some(part.clone());
                    fit_lanes(&mut n);
                }
                Part::Edge(id) => {
                    if len(&t) > MAX_LABEL {
                        return Err(format!("a label is at most {MAX_LABEL} characters (this one is {})", len(&t)));
                    }
                    let e = n.edges.iter_mut().find(|e| &e.id == id).ok_or("that edge is gone")?;
                    e.label = t;
                    select = Some(part.clone());
                }
                Part::Whole => {
                    if len(&t) > MAX_TITLE {
                        return Err(format!("a title is at most {MAX_TITLE} characters (this one is {})", len(&t)));
                    }
                    n.title = t;
                    select = Some(Part::Whole);
                }
            }
        }
        Op::Delete { part } => match part {
            Part::Node(id) => {
                n.ni(id).ok_or("that node is gone")?;
                n.nodes.retain(|x| &x.id != id);
                n.edges.retain(|e| &e.from != id && &e.to != id);
                for g in &mut n.groups {
                    g.nodes.retain(|x| x != id);
                }
                renumber(&mut n);
                fit_lanes(&mut n);
            }
            Part::Edge(id) => {
                let before = n.edges.len();
                n.edges.retain(|e| &e.id != id);
                if n.edges.len() == before {
                    return Err("that edge is gone".into());
                }
                renumber(&mut n);
            }
            Part::Whole => return Err("a diagram stays on the canvas: select a node or an edge to remove it".into()),
        },
        Op::Relayout => {
            for nd in &mut n.nodes {
                nd.placed = Placed::None;
            }
        }
    }
    check(&n)?;
    Ok(Done { d: n, select, rename })
}

/// Undo and redo of one diagram: whole versions, [`HISTORY`] steps back.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct History {
    past: Vec<Diagram>,
    future: Vec<Diagram>,
}

impl History {
    /// `before` is the version an edit replaced: one step back. A new edit drops the redo steps.
    pub fn push(&mut self, before: Diagram) {
        self.past.push(before);
        if self.past.len() > HISTORY {
            self.past.remove(0);
        }
        self.future.clear();
    }
    /// The version before `now`, if any (`now` is kept for redo).
    pub fn undo(&mut self, now: &Diagram) -> Option<Diagram> {
        let prev = self.past.pop()?;
        self.future.push(now.clone());
        Some(prev)
    }
    pub fn redo(&mut self, now: &Diagram) -> Option<Diagram> {
        let next = self.future.pop()?;
        self.past.push(now.clone());
        Some(next)
    }
    pub fn can_undo(&self) -> bool {
        !self.past.is_empty()
    }
    pub fn can_redo(&self) -> bool {
        !self.future.is_empty()
    }
    pub fn steps(&self) -> (usize, usize) {
        (self.past.len(), self.future.len())
    }
}

// ---------------------------------------------------------------------------
// Drawing

/// Below this zoom, labels are left out (a 13 point label under 4 points on screen).
pub const FAR: f32 = 0.3;
/// Corners in the diagram's points: a node's, a group frame's, an edge label's.
const NODE_R: f32 = 8.0;
const GROUP_R: f32 = 6.0;
const LABEL_R: f32 = 4.0;

/// The handle's radius on screen, points.
pub const HANDLE_R: f32 = 6.0;

/// What a diagram is drawn with.
pub struct Look<'a> {
    /// The item's id (the diagram's id): every key starts `canvas-dg-<id>`.
    pub id: &'a str,
    pub d: &'a Diagram,
    /// Where the diagram's (0, 0) sits in the world.
    pub origin: [f32; 2],
    pub zoom: f32,
    pub light: bool,
    /// The part selected, if this diagram holds the selection.
    pub sel: Option<&'a Part>,
    /// A new edge being dragged: from this node to this point (the diagram's points).
    pub rubber: Option<(&'a str, [f32; 2])>,
    /// A message being dragged to this row.
    pub row_drag: Option<(&'a str, u32)>,
    pub title: &'a str,
}

fn font(size: f32, weight: f32) -> rune::TextStyle {
    crate::screen::body(size, weight)
}

/// A dashed polyline as short strokes (Rune's strokes are solid): dash `k`
/// covers `[k * period, k * period + dash]` along the line, cut at corners.
fn dashed(mut s: rune::Shape, pts: &[[f32; 2]], width: f32, dash: f32, gap: f32, color: rune::Color) -> rune::Shape {
    let total: f32 = pts.windows(2).map(|w| (w[1][0] - w[0][0]).hypot(w[1][1] - w[0][1])).sum();
    if !(total.is_finite() && total > 0.0 && dash > 0.0) {
        return s;
    }
    // At most 60 dashes a line: a long one gets longer dashes.
    let period = (dash + gap).max(total / 60.0);
    let dash = period * dash / (dash + gap);
    let mut d0 = 0.0f32;
    for w in pts.windows(2) {
        let (a, b) = (w[0], w[1]);
        let l = (b[0] - a[0]).hypot(b[1] - a[1]);
        if l <= 0.0 {
            continue;
        }
        let d1 = d0 + l;
        let at = |u: f32| {
            let t = ((u - d0) / l).clamp(0.0, 1.0);
            [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t]
        };
        let (k0, k1) = ((d0 / period).floor() as i64, (d1 / period).floor() as i64);
        for k in k0..=k1 {
            let (s0, s1) = ((k as f32 * period).max(d0), (k as f32 * period + dash).min(d1));
            if s1 > s0 {
                s = s.line(at(s0), at(s1), width, rune::LineCap::Butt, color);
            }
        }
        d0 = d1;
    }
    s
}

/// An arrowhead at `tip`, pointing from `from`.
fn arrow(s: rune::Shape, from: [f32; 2], tip: [f32; 2], size: f32, color: rune::Color) -> rune::Shape {
    let (dx, dy) = (tip[0] - from[0], tip[1] - from[1]);
    let l = dx.hypot(dy);
    if l <= 0.0 {
        return s;
    }
    let (ux, uy) = (dx / l, dy / l);
    let base = [tip[0] - ux * size, tip[1] - uy * size];
    let half = size * 0.45;
    s.fill(vec![tip, [base[0] - uy * half, base[1] + ux * half], [base[0] + uy * half, base[1] - ux * half]], color)
}

/// The colour token of an edge style.
pub fn style_token(s: Style) -> &'static str {
    match s {
        Style::Solid | Style::Dashed => "canvas-diagram-edge",
        Style::Emphasis => "canvas-diagram-emphasis",
        Style::Error => "canvas-diagram-error",
    }
}

/// An edge's accessible name: "A to B: label".
pub fn edge_name(d: &Diagram, e: &Edge) -> String {
    let base = format!("{} to {}", d.label_of(&e.from), d.label_of(&e.to));
    if e.label.is_empty() { base } else { format!("{base}: {}", e.label) }
}

/// The elements of one diagram, placed in world points: lane bands, group
/// frames, every line and arrow in one shape, edge labels, nodes, the
/// selected node's handle, and the title above. Texts are text.
pub fn draw(l: &Look) -> Vec<El> {
    let d = l.d;
    let inv = 1.0 / l.zoom;
    let k = |name: &str| crate::theme::canvas_c(l.light, name);
    let [ox, oy] = l.origin;
    let pre = format!("canvas-dg-{}", l.id);
    let far = l.zoom < FAR;
    let bx = content_box(d);
    let mut out = vec![];
    // The title, above the content, at a size that stays readable (as a caption).
    let fs = (12.0 * inv).min(100.0);
    let kind = d.kind.name();
    let head = format!("Diagram · {} · {kind} · {} node{}", if l.title.is_empty() { "untitled" } else { l.title }, d.nodes.len(), if d.nodes.len() == 1 { "" } else { "s" });
    out.push(
        el().key(format!("{pre}-title"))
            .pin(Some(ox + bx[0]), Some(oy + bx[1] - (fs * 1.6) - (6.0 * inv).min(16.0)), None, None)
            .w(bx[2].max(200.0))
            .child(text(head).font(font(fs, 600.0)).color(k("canvas-text-muted")).truncate().w(bx[2].max(200.0))),
    );
    for ln in &d.lanes {
        let r = lane_rect(d, ln);
        let mut b = el().key(format!("{pre}-lane-{}", ln.id)).pin(Some(ox + r[0]), Some(oy + r[1]), None, None).w(r[2]).h(r[3]).fill(k("canvas-diagram-lane")).border(inv, k("canvas-line"));
        if !far {
            let tw = if d.lr() { LANE_TITLE - 16.0 } else { r[2] - 16.0 };
            b = b.child(el().pin(Some(8.0), Some(8.0), None, None).w(tw).child(text(ln.label.clone()).font(font(12.0, 600.0)).color(k("canvas-text-muted")).wrap().max_lines(3).w(tw)));
        }
        out.push(b);
    }
    for g in &d.groups {
        let Some(r) = group_rect(d, g) else { continue };
        let mut b = el().key(format!("{pre}-group-{}", g.id)).pin(Some(ox + r[0]), Some(oy + r[1]), None, None).w(r[2]).h(r[3]).border(1.5 * inv, k("canvas-diagram-group")).radius(GROUP_R);
        if !far && !g.label.is_empty() {
            b = b.child(el().pin(Some(CLUSTER_PAD / 2.0), Some(4.0), None, None).w(r[2] - CLUSTER_PAD).child(text(g.label.clone()).font(font(11.0, 600.0)).color(k("canvas-text-muted")).truncate().w(r[2] - CLUSTER_PAD)));
        }
        out.push(b);
    }
    // Every line in one shape over the content box (and a margin for arrows).
    let m = 24.0;
    let (sx, sy) = (bx[0] - m, bx[1] - m);
    let at = |p: [f32; 2]| [p[0] - sx, p[1] - sy];
    let mut shape = rune::Shape::new();
    let line_w = 1.5 * inv;
    if d.kind == Kind::Sequence {
        let end = lifeline_end(d);
        for n in &d.nodes {
            let c = centre(n);
            shape = dashed(shape, &[at([c[0], n.y + n.h]), at([c[0], end])], inv, 5.0 * inv, 4.0 * inv, k("canvas-diagram-lifeline"));
        }
    }
    let selected_edge = match l.sel {
        Some(Part::Edge(e)) => Some(e.as_str()),
        _ => None,
    };
    let mut labels = vec![];
    for e in &d.edges {
        let Some(mut r) = route(d, e) else { continue };
        // A message being dragged is drawn on the row it would take.
        if let Some((id, row)) = l.row_drag.filter(|(id, _)| *id == e.id) {
            let mut moved = e.clone();
            moved.order = Some(row);
            if let Some(m) = route(d, &moved) {
                r = m;
            }
            let _ = id;
        }
        let sel = selected_edge == Some(e.id.as_str());
        let color = if sel { k("canvas-accent") } else { k(style_token(e.style)) };
        let w = match e.style {
            Style::Emphasis => 2.5 * inv,
            _ => line_w,
        } + if sel { inv } else { 0.0 };
        let pts: Vec<[f32; 2]> = r.points.iter().map(|p| at(*p)).collect();
        // The line stops at the arrow's base, so a thick line does not blunt its tip.
        let ah = (8.0 * inv).max(w * 3.0);
        let mut line = pts.clone();
        if line.len() >= 2 {
            let (a, b) = (line[line.len() - 2], line[line.len() - 1]);
            let l2 = (b[0] - a[0]).hypot(b[1] - a[1]);
            if l2 > ah {
                let n = line.len() - 1;
                line[n] = [b[0] - (b[0] - a[0]) / l2 * ah * 0.8, b[1] - (b[1] - a[1]) / l2 * ah * 0.8];
            }
        }
        shape = match e.style {
            Style::Dashed => dashed(shape, &line, w, 6.0 * inv, 4.0 * inv, color),
            _ => shape.stroke(line, w, color),
        };
        if pts.len() >= 2 {
            shape = arrow(shape, pts[pts.len() - 2], pts[pts.len() - 1], ah, color);
        }
        labels.push((e, r.label_at, sel));
    }
    if let Some((from, to)) = l.rubber {
        if let Some(n) = d.node(from) {
            let a = handle_at(d, n);
            shape = dashed(shape, &[at(a), at(to)], 2.0 * inv, 6.0 * inv, 4.0 * inv, k("canvas-accent"));
            shape = arrow(shape, at(a), at(to), 9.0 * inv, k("canvas-accent"));
        }
    }
    out.push(rune::canvas(shape).key(format!("{pre}-lines")).pin(Some(ox + sx), Some(oy + sy), None, None).w(bx[2] + 2.0 * m).h(bx[3] + 2.0 * m));
    // Edge labels, and each edge's accessible name.
    for (e, p, sel) in labels {
        let name = edge_name(d, e);
        if e.label.is_empty() || far {
            out.push(el().key(format!("{pre}-e-{}", e.id)).pin(Some(ox + p[0]), Some(oy + p[1]), None, None).w(1.0).h(1.0).role("img").label(name));
            continue;
        }
        let s = label_size(&e.label);
        out.push(
            el().key(format!("{pre}-e-{}", e.id))
                .pin(Some(ox + p[0] - s[0] / 2.0), Some(oy + p[1] - s[1] / 2.0), None, None)
                .w(s[0])
                .h(s[1])
                .fill(k("canvas-ground"))
                .radius(LABEL_R)
                .border(if sel { inv } else { 0.0 }, k("canvas-accent"))
                .items_center()
                .justify_center()
                .role("img")
                .label(name)
                .child(text(e.label.clone()).font(font(11.0, 400.0)).color(k(if e.style == Style::Error { "canvas-diagram-error" } else { "canvas-text" })).truncate().w(s[0] - 8.0).align(rune::Align::Center)),
        );
    }
    let selected_node = match l.sel {
        Some(Part::Node(n)) => Some(n.as_str()),
        _ => None,
    };
    for n in &d.nodes {
        let sel = selected_node == Some(n.id.as_str());
        out.push(node_el(&pre, n, [ox, oy], inv, far, sel, l.light));
        if sel {
            let h = handle_at(d, n);
            let r = HANDLE_R * inv;
            out.push(el().key(format!("{pre}-handle")).pin(Some(ox + h[0] - r), Some(oy + h[1] - r), None, None).w(2.0 * r).h(2.0 * r).radius(r).fill(k("canvas-accent")).border(1.5 * inv, k("canvas-handle-edge")).role("img").label("Drag to link to another node, or to empty ground for a new one"));
        }
    }
    out
}

/// One node: its box by shape, its label and second line, a button named by its label.
fn node_el(pre: &str, n: &Node, o: [f32; 2], inv: f32, far: bool, sel: bool, light: bool) -> El {
    let k = |name: &str| crate::theme::canvas_c(light, name);
    let edge_c = if sel { k("canvas-accent") } else if n.shape == Shape::Note { k("canvas-note-edge") } else { k("canvas-diagram-node-edge") };
    let fill = if n.shape == Shape::Note { k("canvas-note") } else { k("canvas-diagram-node") };
    let bw = if sel { 2.0 * inv } else { inv.max(1.0).min(1.5 * inv) };
    let (w, h) = (n.w, n.h);
    let mut b = el().key(format!("{pre}-n-{}", n.id)).pin(Some(o[0] + n.x), Some(o[1] + n.y), None, None).w(w).h(h).role("button").label(if n.sub.is_empty() { n.label.clone() } else { format!("{}, {}", n.label, n.sub) });
    match n.shape {
        Shape::Decision => {
            let pts = vec![[w / 2.0, 0.0], [w, h / 2.0], [w / 2.0, h], [0.0, h / 2.0]];
            b = b.shape(rune::Shape::new().fill(pts.clone(), fill).stroke_with(pts, true, bw, rune::LineCap::Round, edge_c));
        }
        Shape::Start | Shape::End => {
            b = b.fill(fill).radius(h / 2.0).border(if n.shape == Shape::End { bw * 2.0 } else { bw }, edge_c);
        }
        Shape::Store => {
            // A store: a box with the cylinder's rims, top and bottom.
            b = b.fill(fill).radius(h / 4.0).border(bw, edge_c).shape(rune::Shape::new().line([0.0, 8.0], [w, 8.0], inv, rune::LineCap::Butt, edge_c));
        }
        Shape::External => {
            // Square corners and a second inner frame.
            b = b.fill(fill).border(bw, edge_c).shape(rune::Shape::new().stroke_with(vec![[4.0, 4.0], [w - 4.0, 4.0], [w - 4.0, h - 4.0], [4.0, h - 4.0]], true, inv, rune::LineCap::Butt, edge_c));
        }
        Shape::Actor => {
            // A head and shoulders at the left.
            let c = [14.0, h / 2.0 - 5.0];
            b = b.fill(fill).radius(NODE_R).border(bw, edge_c).shape(rune::Shape::new().line(c, c, 9.0, rune::LineCap::Round, edge_c).stroke(vec![[6.0, h / 2.0 + 12.0], [14.0, h / 2.0 + 3.0], [22.0, h / 2.0 + 12.0]], 1.5, edge_c));
        }
        Shape::Note => {
            // A folded corner.
            b = b.fill(fill).border(bw, edge_c).shape(rune::Shape::new().stroke(vec![[w - 12.0, 0.0], [w - 12.0, 12.0], [w, 12.0]], inv, edge_c));
        }
        Shape::Process => {
            b = b.fill(fill).radius(NODE_R).border(bw, edge_c);
        }
    }
    if far {
        return b;
    }
    let pad = if n.shape == Shape::Decision { w / 5.0 } else if n.shape == Shape::Actor { 28.0 } else { 10.0 };
    let tw = (w - 2.0 * pad).max(8.0);
    let mut lines = rune::col().gap(2.0).w(tw).items_center().child(text(n.label.clone()).font(font(13.0, 600.0)).color(k("canvas-text")).wrap().max_lines(2).w(tw).align(rune::Align::Center));
    if !n.sub.is_empty() {
        lines = lines.child(text(n.sub.clone()).font(font(11.0, 400.0)).color(k("canvas-text-muted")).wrap().max_lines(2).w(tw).align(rune::Align::Center));
    }
    b.child(el().pin(Some(pad), Some(0.0), None, None).w(tw).h(h).items_center().justify_center().child(lines))
}

#[cfg(test)]
mod tests {
    use super::*;

    pub fn sample() -> Diagram {
        serde_json::from_value(json!({
            "version": 1, "id": "d-0123456789ab", "kind": "workflow", "title": "Release", "direction": "lr",
            "lanes": [{ "id": "dev", "label": "Developer", "at": 0, "span": 112 }, { "id": "ci", "label": "CI", "at": 112, "span": 112 }],
            "groups": [{ "id": "checks", "label": "Checks", "nodes": ["test", "lint"] }],
            "nodes": [
                { "id": "push", "label": "Push", "sub": "a branch", "shape": "start", "lane": "dev", "x": 136, "y": 24, "w": 112, "h": 64, "placed": "layout" },
                { "id": "test", "label": "Test", "shape": "process", "lane": "ci", "x": 336, "y": 136, "w": 96, "h": 44, "placed": "layout" },
                { "id": "lint", "label": "Lint", "shape": "decision", "lane": "ci", "x": 496, "y": 136, "w": 128, "h": 60, "placed": "layout" }
            ],
            "edges": [{ "id": "e1", "from": "push", "to": "test", "label": "webhook", "style": "solid" }, { "id": "e2", "from": "push", "to": "lint", "style": "dashed" }],
            "rev": 2, "author": "agent", "updated_ms": 5
        }))
        .unwrap()
    }

    pub fn sequence() -> Diagram {
        serde_json::from_value(json!({
            "version": 1, "id": "d-00000000000a", "kind": "sequence", "title": "Review", "direction": "lr", "lanes": [], "groups": [],
            "nodes": [
                { "id": "you", "label": "You", "shape": "actor", "x": 0, "y": 0, "w": 128, "h": 44, "placed": "layout" },
                { "id": "app", "label": "App", "x": 188, "y": 0, "w": 104, "h": 44, "placed": "layout" },
                { "id": "agent", "label": "Agent", "x": 352, "y": 0, "w": 120, "h": 44, "placed": "layout" }
            ],
            "edges": [
                { "id": "m1", "from": "you", "to": "app", "label": "pin", "style": "solid", "order": 0 },
                { "id": "m2", "from": "app", "to": "agent", "label": "send", "style": "emphasis", "order": 1 },
                { "id": "m3", "from": "agent", "to": "agent", "label": "think", "style": "dashed", "order": 2 }
            ],
            "rev": 1, "author": "agent", "updated_ms": 1
        }))
        .unwrap()
    }

    #[test]
    fn the_wire_form_is_the_schema_and_an_unplaced_node_has_no_place() {
        let d = sample();
        let w = wire(&d);
        assert_eq!(w["nodes"][0]["placed"], "layout");
        assert_eq!(w["nodes"][0]["sub"], "a branch");
        assert!(w["nodes"][1].get("sub").is_none(), "no empty sub");
        assert!(w["edges"][1].get("label").is_none() && w["edges"][1].get("order").is_none());
        let keys: Vec<&String> = w.as_object().unwrap().keys().collect();
        for k in keys {
            assert!(["version", "id", "kind", "title", "direction", "lanes", "groups", "nodes", "edges", "rev", "author", "updated_ms"].contains(&k.as_str()), "{k}");
        }
        let r = apply(&d, &Op::Relayout).unwrap().d;
        let w = wire(&r);
        for n in w["nodes"].as_array().unwrap() {
            assert!(n.get("x").is_none() && n.get("y").is_none() && n.get("placed").is_none(), "{n}");
        }
    }

    #[test]
    fn every_limit_is_refused_before_sending() {
        let d = sample();
        assert!(check(&d).is_ok());
        let long = "x".repeat(MAX_LABEL + 1);
        assert!(apply(&d, &Op::Rename { part: Part::Node("push".into()), text: long.clone() }).unwrap_err().contains("at most 80"));
        assert!(apply(&d, &Op::Rename { part: Part::Node("push".into()), text: "x".repeat(MAX_LABEL) }).is_ok(), "80 is the bound");
        assert!(apply(&d, &Op::Rename { part: Part::Edge("e1".into()), text: long }).is_err());
        assert!(apply(&d, &Op::Rename { part: Part::Node("push".into()), text: " \u{200b}\n ".into() }).unwrap_err().contains("needs a label"));
        assert!(apply(&d, &Op::Link { from: "test".into(), to: "test".into() }).unwrap_err().contains("itself"));
        assert!(apply(&d, &Op::Move { node: "push".into(), x: 200_000.0, y: 0.0 }).unwrap_err().contains("out of bounds"));
        // 200 nodes is the bound: the 201st is refused.
        let mut big = d.clone();
        big.lanes.clear();
        big.groups.clear();
        for n in &mut big.nodes {
            n.lane = None;
        }
        while big.nodes.len() < MAX_NODES {
            let k = big.nodes.len();
            big.nodes.push(Node { id: format!("x{k}"), label: format!("N{k}"), w: 96.0, h: 44.0, x: k as f32 * 10.0, placed: Placed::Layout, ..Node::default() });
        }
        assert!(check(&big).is_ok());
        assert!(apply(&big, &Op::AddAt { x: 0.0, y: 0.0 }).unwrap_err().contains("at most 200"));
        assert!(apply(&big, &Op::Successor { from: "push".into() }).is_err());
        // 400 edges likewise.
        let mut many = big.clone();
        many.edges.clear();
        for k in 0..MAX_EDGES {
            many.edges.push(Edge { id: format!("e{}", k + 1), from: format!("x{}", 3 + k % 190), to: format!("x{}", 4 + k % 190), ..Edge::default() });
        }
        assert!(check(&many).is_ok());
        assert!(apply(&many, &Op::Link { from: "push".into(), to: "test".into() }).unwrap_err().contains("at most 400"));
        // A lifecycle takes one start.
        let mut lc = d.clone();
        lc.kind = Kind::Lifecycle;
        lc.nodes[1].shape = Shape::Start;
        assert!(check(&lc).unwrap_err().contains("one start"));
        // A bad id from the daemon is refused, never cleaned.
        let mut bad = d.clone();
        bad.nodes[0].id = "../etc".into();
        assert!(check(&bad).is_err());
    }

    #[test]
    fn a_hostile_label_is_cleaned_as_the_core_cleans_it() {
        let d = sample();
        let t = "  Lead\u{202e}review\n\u{200b}\u{7}  ";
        let r = apply(&d, &Op::Rename { part: Part::Node("test".into()), text: t.into() }).unwrap().d;
        assert_eq!(r.node("test").unwrap().label, "Leadreview");
        assert_eq!(clean("<b>x</b>"), "<b>x</b>", "markup stays text");
    }

    #[test]
    fn routes_leave_from_the_facing_sides_and_messages_sit_on_their_rows() {
        let d = sample();
        let r = route(&d, &d.edges[0]).unwrap();
        let (a, b) = (d.node("push").unwrap(), d.node("test").unwrap());
        assert_eq!(r.points[0], [a.x + a.w, a.y + a.h / 2.0], "from push's right side");
        assert_eq!(*r.points.last().unwrap(), [b.x, b.y + b.h / 2.0], "into test's left side");
        assert!(r.points.windows(2).all(|w| w[0][0] == w[1][0] || w[0][1] == w[1][1]), "orthogonal: {:?}", r.points);
        let s = sequence();
        for e in &s.edges {
            let r = route(&s, e).unwrap();
            assert_eq!(r.points[0][1], message_y(&s, e.order.unwrap()));
        }
        assert_eq!(message_y(&s, 0), 44.0 + SEQ_FIRST);
        assert_eq!(route(&s, &s.edges[2]).unwrap().points.len(), 4, "a self message loops");
    }

    #[test]
    fn hit_tests_find_nodes_then_edges() {
        let d = sample();
        assert_eq!(under(&d, [140.0, 30.0], 4.0), Some(Under::Node(0)));
        let r = route(&d, &d.edges[0]).unwrap();
        assert_eq!(under(&d, r.label_at, 4.0), Some(Under::Edge(0)));
        assert_eq!(under(&d, [5000.0, 5000.0], 4.0), None);
    }

    #[test]
    fn edits_link_add_delete_and_reorder() {
        let d = sample();
        let r = apply(&d, &Op::Link { from: "test".into(), to: "lint".into() }).unwrap();
        assert_eq!(r.d.edges.len(), 3);
        assert_eq!(r.select, Some(Part::Edge("e3".into())));
        let r = apply(&d, &Op::LinkNew { from: "test".into(), x: 700.0, y: 160.0 }).unwrap();
        assert_eq!(r.d.nodes.len(), 4);
        let n = r.d.node("n1").unwrap();
        assert_eq!((n.lane.as_deref(), n.placed, n.label.as_str()), (Some("ci"), Placed::Person, NEW_LABEL));
        assert!(r.rename);
        assert!(r.d.edges.iter().any(|e| e.from == "test" && e.to == "n1"));
        // Tab: a successor right of it, linked, not on anything.
        let r = apply(&d, &Op::Successor { from: "test".into() }).unwrap();
        let (a, n) = (r.d.node("test").unwrap(), r.d.node("n1").unwrap());
        assert!(n.x >= a.x + a.w, "right of it");
        assert!(r.d.nodes.iter().filter(|q| q.id != "n1").all(|q| n.x >= q.x + q.w || n.x + n.w <= q.x || n.y >= q.y + q.h || n.y + n.h <= q.y), "no overlap");
        // A node takes its edges.
        let r = apply(&d, &Op::Delete { part: Part::Node("push".into()) }).unwrap();
        assert!(r.d.edges.is_empty() && r.d.node("push").is_none());
        let r = apply(&d, &Op::Delete { part: Part::Node("test".into()) }).unwrap();
        assert_eq!(r.d.groups[0].nodes, vec!["lint".to_string()], "out of its group");
        // A sequence: the last message to the top.
        let s = sequence();
        let r = apply(&s, &Op::Reorder { edge: "m3".into(), order: 0 }).unwrap();
        let ord: Vec<(String, u32)> = r.d.edges.iter().map(|e| (e.id.clone(), e.order.unwrap())).collect();
        assert_eq!(ord, vec![("m1".into(), 1), ("m2".into(), 2), ("m3".into(), 0)]);
        // A lifeline moves sideways only, and keeps the left-to-right order.
        let r = apply(&s, &Op::Move { node: "you".into(), x: 500.0, y: 300.0 }).unwrap();
        let you = r.d.node("you").unwrap();
        assert_eq!((you.x, you.y), (500.0, 0.0));
        assert_eq!(r.d.nodes.last().unwrap().id, "you");
    }

    #[test]
    fn a_node_dropped_in_another_lane_moves_there_unless_its_group_holds_it() {
        let d = sample();
        let r = apply(&d, &Op::Move { node: "push".into(), x: 600.0, y: 150.0 }).unwrap().d;
        assert_eq!(r.node("push").unwrap().lane.as_deref(), Some("ci"));
        let r = apply(&d, &Op::Move { node: "test".into(), x: 336.0, y: 20.0 }).unwrap().d;
        assert_eq!(r.node("test").unwrap().lane.as_deref(), Some("ci"), "held by its group");
    }

    #[test]
    fn undo_and_redo_keep_a_hundred_steps() {
        let mut h = History::default();
        let mut d = sample();
        for k in 0..150 {
            let next = apply(&d, &Op::Move { node: "push".into(), x: 136.0 + k as f32, y: 24.0 }).unwrap().d;
            h.push(std::mem::replace(&mut d, next));
        }
        assert_eq!(h.steps(), (HISTORY, 0));
        let mut back = 0;
        while let Some(p) = h.undo(&d) {
            d = p;
            back += 1;
        }
        assert_eq!(back, HISTORY);
        assert_eq!(d.node("push").unwrap().x, 136.0 + 49.0);
        let d2 = h.redo(&d).unwrap();
        assert_eq!(d2.node("push").unwrap().x, 136.0 + 50.0);
        h.push(d2);
        assert!(!h.can_redo(), "a new edit drops the redo steps");
    }

    #[test]
    fn selections_name_their_diagram_and_part() {
        assert_eq!(parse_sel("d-0123456789ab"), Some(("d-0123456789ab", Part::Whole)));
        assert_eq!(parse_sel("d-0123456789ab:n:push"), Some(("d-0123456789ab", Part::Node("push".into()))));
        assert_eq!(parse_sel("d-0123456789ab:e:e1"), Some(("d-0123456789ab", Part::Edge("e1".into()))));
        assert_eq!(parse_sel("s-1-0"), None);
        assert_eq!(parse_sel("pin-3"), None);
        assert_eq!(parse_sel("d-0123456789ab:n:../x"), None);
        assert_eq!(Part::Node("a".into()).key("d-0123456789ab"), "d-0123456789ab:n:a");
    }
}
