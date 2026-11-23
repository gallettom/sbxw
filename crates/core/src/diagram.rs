//! Diagrams an agent provisions onto a canvas and the person edits: sbxw's own small schema,
//! its limits and checks, an importer from Archify's JSON ([`import`]), the layouts
//! ([`layout`]: a layered one for flows, lifelines for sequences) and the diff an agent reads
//! back ([`diff`]). Pure: no I/O, no clock, no randomness; the caller brings ids and times.
//!
//! # The schema (version 1)
//!
//! ```json
//! { "version": 1, "kind": "workflow", "title": "Release", "direction": "lr",
//!   "lanes":  [{ "id": "dev", "label": "Developer" }, { "id": "ci", "label": "CI" }],
//!   "groups": [{ "id": "checks", "label": "Checks", "nodes": ["test", "lint"] }],
//!   "nodes":  [{ "id": "push", "label": "Push", "sub": "a branch", "lane": "dev" },
//!              { "id": "test", "label": "Test", "lane": "ci", "shape": "process" },
//!              { "id": "lint", "label": "Lint", "lane": "ci" }],
//!   "edges":  [{ "from": "push", "to": "test", "label": "webhook" },
//!              { "from": "push", "to": "lint", "style": "dashed" }] }
//! ```
//!
//! - `kind`: `workflow`, `sequence`, `dataflow` or `lifecycle`. `direction`: `lr` (default for
//!   every kind) or `tb`.
//! - A node may carry `x`, `y` (its top left corner), `w`, `h` and `placed` (`agent`,
//!   `person`, `layout`, or `none`). Without `x` and `y` it is unplaced, and
//!   [`layout::place_missing`] gives it a place. Without `w` and `h` its size is estimated
//!   from its texts. `shape`: `process` (default), `decision`, `start`, `end`, `store`,
//!   `external`, `actor`, `note`. Edge `style`: `solid` (default), `dashed`, `emphasis`,
//!   `error`. An edge `id` is optional: missing ones are numbered `e1`, `e2`, …
//! - Sequence diagrams: the nodes are the lifelines (left to right in array order), the edges
//!   are the messages, each with an `order` (its row, from 0); an edge from a lifeline to
//!   itself is a self message. Without any `order`, array order is used. No lanes, no groups.
//! - Lanes are swimlanes: bands across the flow (rows in `lr`, columns in `tb`), in array
//!   order. With lanes, every node names one, and a group's nodes all sit in one lane. A
//!   node is in at most one group. The layout stores each lane's band (`at`, `span`).
//! - Stored metadata the daemon sets: `id` (`d-` and 12 lower-case hex digits), `rev`,
//!   `author` (`agent` or `person`), `updated_ms`.
//!
//! Parsing ([`parse`]) refuses unknown fields: an agent writing this schema learns of a typo.
//! The Archify importer ignores them.
//!
//! # Limits, refused at parse with a reason (never clipped)
//!
//! | What | At most |
//! |---|---|
//! | source JSON | [`MAX_SOURCE_BYTES`] (256 KB) |
//! | nodes, edges, lanes, groups | [`MAX_NODES`] 200, [`MAX_EDGES`] 400, [`MAX_LANES`] 16, [`MAX_GROUPS`] 32 |
//! | title | [`MAX_TITLE`] 120 characters |
//! | node, lane, group and edge label | [`MAX_LABEL`] 80 characters |
//! | node `sub` | [`MAX_SUB`] 160 characters |
//! | ids | [`MAX_ID`] 40 characters, `[a-z0-9][a-z0-9_-]*` |
//! | `x`, `y`, lane `at` | within ±[`MAX_COORD`] (100 000), finite |
//! | `w`, `h`, lane `span` | [`MIN_SIDE`] 16 to [`MAX_SIDE`] 2000 (lane span up to [`MAX_COORD`]) |
//! | sequence `order` | below [`MAX_EDGES`] |
//!
//! Every text is data: it goes through [`crate::canvas::clean`] (control characters,
//! including new lines, and the shared invisible list of [`crate::invisible`] removed,
//! trimmed), and the length limit applies to the cleaned text. A label empty after cleaning
//! is refused for nodes and lanes, allowed for edges. Ids are never cleaned into shape: one
//! that does not match is refused, so nothing from outside becomes an id by accident, and no
//! id here is ever a path or a file name.
//!
//! # Semantic checks
//!
//! Ids unique within nodes, edges, lanes and groups; edges name existing nodes; no edge from
//! a node to itself except in sequence and lifecycle diagrams; a lifecycle has at most one
//! `start`; a sequence has every message ordered and no lanes or groups; an `order` only in a
//! sequence.

use serde::{Deserialize, Serialize};

pub mod diff;
pub mod import;
pub mod layered;
pub mod layout;

#[cfg(test)]
mod tests;

/// The schema version this module writes and reads.
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Direction {
    /// Left to right: ranks are columns, lanes are rows.
    #[default]
    Lr,
    /// Top to bottom: ranks are rows, lanes are columns.
    Tb,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Style {
    #[default]
    Solid,
    Dashed,
    Emphasis,
    Error,
}

/// Who gave a node its place.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Placed {
    /// No place yet: the next [`layout::place_missing`] gives one.
    #[default]
    None,
    Layout,
    Agent,
    Person,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Author {
    #[default]
    Agent,
    Person,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Lane {
    pub id: String,
    pub label: String,
    /// The band's start across the flow (`y` in `lr`, `x` in `tb`), and its thickness; set by
    /// the layout, 0 until then.
    pub at: f32,
    pub span: f32,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Group {
    pub id: String,
    pub label: String,
    pub nodes: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Node {
    pub id: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub sub: String,
    pub shape: Shape,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lane: Option<String>,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub placed: Placed,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Edge {
    pub id: String,
    pub from: String,
    pub to: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub label: String,
    pub style: Style,
    /// A sequence message's row, from 0.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub order: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Diagram {
    pub version: u32,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub id: String,
    pub kind: Kind,
    pub title: String,
    pub direction: Direction,
    pub lanes: Vec<Lane>,
    pub groups: Vec<Group>,
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    pub rev: u32,
    pub author: Author,
    pub updated_ms: u64,
}

impl Diagram {
    pub fn node(&self, id: &str) -> Option<&Node> {
        self.nodes.iter().find(|n| n.id == id)
    }

    /// The whole diagram as the JSON an agent reads back (pretty, the schema above).
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }
}

// ---------------------------------------------------------------------------
// Ids and texts

/// A diagram id as the daemon makes them: `d-` and 12 lower-case hex digits.
pub fn valid_diagram_id(id: &str) -> bool {
    id.len() == 14 && id.starts_with("d-") && id[2..].bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// An item id: `[a-z0-9][a-z0-9_-]*`, at most [`MAX_ID`] bytes.
pub fn valid_id(id: &str) -> bool {
    let b = id.as_bytes();
    !b.is_empty() && b.len() <= MAX_ID && (b[0].is_ascii_lowercase() || b[0].is_ascii_digit()) && b.iter().all(|&c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'_' || c == b'-')
}

/// `id` as an error message may quote it: valid ids as they are, anything else cleaned and cut.
fn shown(id: &str) -> String {
    let c = crate::canvas::clean(id, 24, false);
    if c.chars().count() < id.chars().count() {
        format!("{c}…")
    } else {
        c
    }
}

fn check_id(what: &str, id: &str) -> Result<(), String> {
    if valid_id(id) {
        Ok(())
    } else {
        Err(format!("{what} id \"{}\" is not an id: use 1 to {MAX_ID} of a-z, 0-9, _ and -, starting with a letter or digit", shown(id)))
    }
}

/// `s` cleaned as canvas texts are ([`crate::canvas::clean`]), refused past `max` characters.
pub fn text(what: &str, s: &str, max: usize) -> Result<String, String> {
    let t = crate::canvas::clean(s, usize::MAX, false);
    let n = t.chars().count();
    if n > max {
        return Err(format!("{what} is {n} characters, at most {max}"));
    }
    Ok(t)
}

fn coord(what: &str, v: f32) -> Result<f32, String> {
    if v.is_finite() && v.abs() <= MAX_COORD {
        Ok(v)
    } else {
        Err(format!("{what} is out of bounds (within ±{MAX_COORD})"))
    }
}

fn side(what: &str, v: f32, max: f32) -> Result<f32, String> {
    if v.is_finite() && (MIN_SIDE..=max).contains(&v) {
        Ok(v)
    } else {
        Err(format!("{what} must be between {MIN_SIDE} and {max}"))
    }
}

// ---------------------------------------------------------------------------
// Parsing: the input shape (optional fields) checked into a Diagram

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct In {
    version: u32,
    #[serde(default)]
    id: Option<String>,
    kind: Kind,
    #[serde(default)]
    title: String,
    #[serde(default)]
    direction: Option<Direction>,
    #[serde(default)]
    lanes: Vec<InLane>,
    #[serde(default)]
    groups: Vec<Group>,
    #[serde(default)]
    nodes: Vec<InNode>,
    #[serde(default)]
    edges: Vec<InEdge>,
    #[serde(default)]
    rev: u32,
    #[serde(default)]
    author: Author,
    #[serde(default)]
    updated_ms: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct InLane {
    id: String,
    label: String,
    #[serde(default)]
    at: f32,
    #[serde(default)]
    span: f32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct InNode {
    id: String,
    label: String,
    #[serde(default)]
    sub: String,
    #[serde(default)]
    shape: Shape,
    #[serde(default)]
    lane: Option<String>,
    x: Option<f32>,
    y: Option<f32>,
    w: Option<f32>,
    h: Option<f32>,
    placed: Option<Placed>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct InEdge {
    #[serde(default)]
    id: Option<String>,
    from: String,
    to: String,
    #[serde(default)]
    label: String,
    #[serde(default)]
    style: Style,
    #[serde(default)]
    order: Option<u32>,
}

/// Counts are checked on the raw JSON before anything is built from it.
fn counts(nodes: usize, edges: usize, lanes: usize, groups: usize) -> Result<(), String> {
    for (what, n, max) in [("nodes", nodes, MAX_NODES), ("edges", edges, MAX_EDGES), ("lanes", lanes, MAX_LANES), ("groups", groups, MAX_GROUPS)] {
        if n > max {
            return Err(format!("{n} {what}, at most {max}"));
        }
    }
    Ok(())
}

/// A diagram in sbxw's schema, from an agent or from storage: parsed, cleaned and checked.
pub fn parse(json: &str) -> Result<Diagram, String> {
    if json.len() > MAX_SOURCE_BYTES {
        return Err(format!("the diagram is {} KB, at most {} KB", json.len() >> 10, MAX_SOURCE_BYTES >> 10));
    }
    let v: In = serde_json::from_str(json).map_err(|e| format!("not a diagram: {e}"))?;
    if v.version != VERSION {
        return Err(format!("version {} is not one sbxw reads (it reads {VERSION})", v.version));
    }
    counts(v.nodes.len(), v.edges.len(), v.lanes.len(), v.groups.len())?;
    let mut d = Diagram {
        version: VERSION,
        id: v.id.unwrap_or_default(),
        kind: v.kind,
        title: v.title,
        direction: v.direction.unwrap_or_default(),
        lanes: Vec::new(),
        groups: v.groups,
        nodes: Vec::new(),
        edges: Vec::new(),
        rev: v.rev,
        author: v.author,
        updated_ms: v.updated_ms,
    };
    for l in v.lanes {
        d.lanes.push(Lane { id: l.id, label: l.label, at: l.at, span: l.span });
    }
    for n in v.nodes {
        let placed = match (n.x, n.y, n.placed) {
            (Some(_), Some(_), Some(p)) => p,
            (Some(_), Some(_), None) => Placed::Agent,
            (None, None, None | Some(Placed::None)) => Placed::None,
            _ => return Err(format!("node \"{}\": give both x and y, or neither (and no placed)", shown(&n.id))),
        };
        d.nodes.push(Node { id: n.id, label: n.label, sub: n.sub, shape: n.shape, lane: n.lane, x: n.x.unwrap_or(0.0), y: n.y.unwrap_or(0.0), w: n.w.unwrap_or(0.0), h: n.h.unwrap_or(0.0), placed });
    }
    let orders = v.edges.iter().filter(|e| e.order.is_some()).count();
    if d.kind == Kind::Sequence && orders != 0 && orders != v.edges.len() {
        return Err("a sequence gives every message an order, or none (then array order is used)".into());
    }
    for (i, e) in v.edges.into_iter().enumerate() {
        let order = if d.kind == Kind::Sequence { Some(e.order.unwrap_or(i as u32)) } else { e.order };
        d.edges.push(Edge { id: e.id.unwrap_or_default(), from: e.from, to: e.to, label: e.label, style: e.style, order });
    }
    check(&mut d)?;
    Ok(d)
}

/// Cleans and checks `d` in place: every rule of the module doc. Fills edge ids that are
/// missing, estimates sizes that are 0, and renumbers sequence orders 0, 1, 2, … in their
/// order (ties by array place). The daemon runs it on everything it is sent.
pub fn check(d: &mut Diagram) -> Result<(), String> {
    d.version = VERSION;
    counts(d.nodes.len(), d.edges.len(), d.lanes.len(), d.groups.len())?;
    if !d.id.is_empty() && !valid_diagram_id(&d.id) {
        return Err(format!("diagram id \"{}\" is not one sbxw makes (d- and 12 hex digits)", shown(&d.id)));
    }
    d.title = text("the title", &d.title, MAX_TITLE)?;

    let mut lane_ids = std::collections::HashSet::new();
    for l in &mut d.lanes {
        check_id("lane", &l.id)?;
        if !lane_ids.insert(l.id.clone()) {
            return Err(format!("lane id \"{}\" is used twice", l.id));
        }
        l.label = text(&format!("lane \"{}\": the label", l.id), &l.label, MAX_LABEL)?;
        if l.label.is_empty() {
            return Err(format!("lane \"{}\" has no label", l.id));
        }
        l.at = coord(&format!("lane \"{}\": at", l.id), l.at)?;
        if l.span != 0.0 {
            l.span = side(&format!("lane \"{}\": span", l.id), l.span, MAX_COORD)?;
        }
    }

    let mut node_ids = std::collections::HashSet::new();
    let mut starts = 0;
    for n in &mut d.nodes {
        check_id("node", &n.id)?;
        if !node_ids.insert(n.id.clone()) {
            return Err(format!("node id \"{}\" is used twice", n.id));
        }
        let who = format!("node \"{}\"", n.id);
        n.label = text(&format!("{who}: the label"), &n.label, MAX_LABEL)?;
        if n.label.is_empty() {
            return Err(format!("{who} has no label"));
        }
        n.sub = text(&format!("{who}: sub"), &n.sub, MAX_SUB)?;
        match &n.lane {
            Some(l) if !lane_ids.contains(l) => return Err(format!("{who} names lane \"{}\", which is not in lanes", shown(l))),
            None if !d.lanes.is_empty() => return Err(format!("{who} has no lane: with lanes, every node names one")),
            _ => {}
        }
        n.x = coord(&format!("{who}: x"), n.x)?;
        n.y = coord(&format!("{who}: y"), n.y)?;
        let (ew, eh) = layout::estimate(n);
        n.w = if n.w == 0.0 { ew } else { side(&format!("{who}: w"), n.w, MAX_SIDE)? };
        n.h = if n.h == 0.0 { eh } else { side(&format!("{who}: h"), n.h, MAX_SIDE)? };
        if n.shape == Shape::Start {
            starts += 1;
        }
    }
    if d.kind == Kind::Lifecycle && starts > 1 {
        return Err(format!("a lifecycle has at most one start state ({starts} given)"));
    }

    let mut group_ids = std::collections::HashSet::new();
    let mut grouped = std::collections::HashSet::new();
    for g in &mut d.groups {
        check_id("group", &g.id)?;
        if !group_ids.insert(g.id.clone()) {
            return Err(format!("group id \"{}\" is used twice", g.id));
        }
        g.label = text(&format!("group \"{}\": the label", g.id), &g.label, MAX_LABEL)?;
        if g.nodes.len() > MAX_NODES {
            return Err(format!("group \"{}\" has {} nodes, at most {MAX_NODES}", g.id, g.nodes.len()));
        }
        let mut lane: Option<&Option<String>> = None;
        for id in &g.nodes {
            let Some(n) = d.nodes.iter().find(|n| &n.id == id) else {
                return Err(format!("group \"{}\" names node \"{}\", which is not in nodes", g.id, shown(id)));
            };
            if !grouped.insert(id.clone()) {
                return Err(format!("node \"{id}\" is in two groups: a node is in at most one"));
            }
            match lane {
                None => lane = Some(&n.lane),
                Some(l) if *l != n.lane => return Err(format!("group \"{}\" spans two lanes: a group's nodes sit in one lane", g.id)),
                _ => {}
            }
        }
    }

    let mut edge_ids: std::collections::HashSet<String> = d.edges.iter().filter(|e| !e.id.is_empty()).map(|e| e.id.clone()).collect();
    let mut next = 1;
    for e in &mut d.edges {
        if e.id.is_empty() {
            while edge_ids.contains(&format!("e{next}")) {
                next += 1;
            }
            e.id = format!("e{next}");
            edge_ids.insert(e.id.clone());
        }
    }
    let mut seen = std::collections::HashSet::new();
    for e in &mut d.edges {
        check_id("edge", &e.id)?;
        if !seen.insert(e.id.clone()) {
            return Err(format!("edge id \"{}\" is used twice", e.id));
        }
        let who = format!("edge \"{}\"", e.id);
        for end in [&e.from, &e.to] {
            if !node_ids.contains(end) {
                return Err(format!("{who} names node \"{}\", which is not in nodes", shown(end)));
            }
        }
        if e.from == e.to && !matches!(d.kind, Kind::Sequence | Kind::Lifecycle) {
            return Err(format!("{who} goes from \"{}\" to itself, which only sequence and lifecycle diagrams allow", e.from));
        }
        e.label = text(&format!("{who}: the label"), &e.label, MAX_LABEL)?;
        match (d.kind, e.order) {
            (Kind::Sequence, None) => return Err(format!("{who}: every message of a sequence has an order")),
            (Kind::Sequence, Some(o)) if o as usize >= MAX_EDGES => return Err(format!("{who}: order {o} is past {}", MAX_EDGES - 1)),
            (Kind::Sequence, _) => {}
            (_, Some(_)) => return Err(format!("{who}: only sequence messages have an order")),
            (_, None) => {}
        }
    }
    if d.kind == Kind::Sequence {
        if !d.lanes.is_empty() || !d.groups.is_empty() {
            return Err("a sequence has no lanes or groups: its nodes are the lifelines".into());
        }
        let mut by: Vec<usize> = (0..d.edges.len()).collect();
        by.sort_by_key(|&i| (d.edges[i].order, i));
        for (row, i) in by.into_iter().enumerate() {
            d.edges[i].order = Some(row as u32);
        }
    }
    Ok(())
}
