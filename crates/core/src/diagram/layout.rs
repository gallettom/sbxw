//! Where nodes go. Positions are stored in the diagram, so a place the person gave survives:
//! [`place_missing`] only places unplaced nodes (what the daemon runs on an agent's diagram),
//! and [`relayout`] is the explicit "lay out again" that places everything afresh.
//!
//! - Workflow, dataflow and lifecycle: the layered layout of [`super::layered`] (a port of
//!   rune-mermaid's). Lanes are top-level clusters kept in their declared order on every
//!   rank, then pulled apart into disjoint bands across the flow, one per lane, in order.
//!   Groups are clusters, nested in their lane.
//! - Sequence: lifelines in array order, left to right, tops aligned; each message's row is
//!   [`message_y`] of its order.
//!
//! Deterministic: the same diagram gives the same positions (no hashing order, no clock).
//! Coordinates are whole numbers, with the content's top left corner at (0, 0) after a full
//! layout.

use super::layered::{self, GCluster, GEdge, Graph, Spacing};
use super::{Diagram, Direction, Kind, Node, Placed, Shape, MAX_COORD};

/// Gap between nodes on one rank, and between ranks.
pub const NODE_GAP: f32 = 40.0;
pub const RANK_GAP: f32 = 64.0;
/// Padding inside a group's box, and room for its title.
pub const CLUSTER_PAD: f32 = 16.0;
pub const GROUP_TITLE: f32 = 22.0;
/// Padding inside a lane's band on each side, and the thickness of an empty lane.
pub const LANE_PAD: f32 = 24.0;
pub const EMPTY_LANE: f32 = 80.0;
/// Room left before each band's nodes for the lane's title (left in `lr`, top in `tb`).
pub const LANE_TITLE: f32 = 120.0;
/// Sequence: gap between lifelines, the first message below the heads, and the row height.
pub const SEQ_GAP: f32 = 60.0;
pub const SEQ_FIRST: f32 = 40.0;
pub const SEQ_ROW: f32 = 44.0;
/// Gap between nodes already placed and a block of new ones.
pub const NEW_GAP: f32 = 80.0;

fn chars(s: &str) -> f32 {
    s.chars().count() as f32
}

/// A node's size from its texts, before a renderer measures anything: about 7.5 points a
/// label character, 6.5 a sub character, whole multiples of 8.
pub fn estimate(n: &Node) -> (f32, f32) {
    let w = (chars(&n.label) * 7.5 + 32.0).max(chars(&n.sub) * 6.5 + 32.0).clamp(96.0, 280.0);
    let h = if n.sub.is_empty() { 44.0 } else { 64.0 };
    let (w, h) = if n.shape == Shape::Decision { (w + 32.0, h + 16.0) } else { (w, h) };
    ((w / 8.0).ceil() * 8.0, h)
}

/// The size an edge label takes, `[w, h]` on screen.
fn label_size(s: &str) -> Option<[f32; 2]> {
    (!s.is_empty()).then(|| [chars(s) * 6.5 + 16.0, 20.0])
}

/// The `y` of a sequence message's row.
pub fn message_y(d: &Diagram, order: u32) -> f32 {
    let head = d.nodes.iter().map(|n| n.y + n.h).fold(0.0, f32::max);
    head + SEQ_FIRST + order as f32 * SEQ_ROW
}

/// Places every node afresh ("lay out again"). Places the person gave are lost: the desktop
/// keeps the step before for undo.
pub fn relayout(d: &mut Diagram) {
    for n in &mut d.nodes {
        n.placed = Placed::None;
    }
    place_missing(d);
}

/// Places the unplaced nodes. With none placed, a full layout. With some placed, the new
/// ones are laid out among themselves and set as one block beside what is there (right of it
/// in `lr`, below in `tb`), so nothing placed moves.
pub fn place_missing(d: &mut Diagram) {
    let unplaced: Vec<usize> = (0..d.nodes.len()).filter(|&i| d.nodes[i].placed == Placed::None).collect();
    if unplaced.is_empty() {
        return;
    }
    let all = unplaced.len() == d.nodes.len();
    let mut fresh = d.clone();
    match d.kind {
        Kind::Sequence => lifelines(&mut fresh),
        _ => layered_layout(&mut fresh),
    }
    if all {
        *d = fresh;
        return;
    }
    // Beside the placed ones, as one block.
    let placed: Vec<usize> = (0..d.nodes.len()).filter(|&i| d.nodes[i].placed != Placed::None).collect();
    let right = placed.iter().map(|&i| d.nodes[i].x + d.nodes[i].w).fold(f32::MIN, f32::max);
    let bottom = placed.iter().map(|&i| d.nodes[i].y + d.nodes[i].h).fold(f32::MIN, f32::max);
    let top = placed.iter().map(|&i| d.nodes[i].y).fold(f32::MAX, f32::min);
    let left = placed.iter().map(|&i| d.nodes[i].x).fold(f32::MAX, f32::min);
    let bx = unplaced.iter().map(|&i| fresh.nodes[i].x).fold(f32::MAX, f32::min);
    let by = unplaced.iter().map(|&i| fresh.nodes[i].y).fold(f32::MAX, f32::min);
    let beside_right = d.kind == Kind::Sequence || d.direction == Direction::Lr;
    let (dx, dy) = if beside_right { (right + NEW_GAP - bx, top - by) } else { (left - bx, bottom + NEW_GAP - by) };
    for &i in &unplaced {
        let f = &fresh.nodes[i];
        let n = &mut d.nodes[i];
        n.x = (f.x + dx).round().clamp(-MAX_COORD, MAX_COORD);
        n.y = (f.y + dy).round().clamp(-MAX_COORD, MAX_COORD);
        n.w = f.w;
        n.h = f.h;
        n.placed = Placed::Layout;
    }
    fit_lanes(d);
}

/// Lifelines left to right, tops at 0.
fn lifelines(d: &mut Diagram) {
    let mut x = 0.0;
    for n in &mut d.nodes {
        n.x = x;
        n.y = 0.0;
        n.placed = Placed::Layout;
        x += n.w + SEQ_GAP;
    }
}

/// Each lane's band around its members, after nodes were placed one by one.
fn fit_lanes(d: &mut Diagram) {
    let lr = d.direction == Direction::Lr;
    for l in &mut d.lanes {
        let ext = d.nodes.iter().filter(|n| n.lane.as_deref() == Some(&l.id)).map(|n| if lr { (n.y, n.y + n.h) } else { (n.x, n.x + n.w) }).fold((f32::MAX, f32::MIN), |a, b| (a.0.min(b.0), a.1.max(b.1)));
        if ext.0 <= ext.1 {
            l.at = (ext.0 - LANE_PAD).round();
            l.span = (ext.1 - ext.0 + 2.0 * LANE_PAD).round();
        }
    }
}

/// The layered layout of every node, then lane bands.
fn layered_layout(d: &mut Diagram) {
    let lr = d.direction == Direction::Lr;
    // Screen [w, h] into the layout's axes [along a rank, across ranks], and back.
    let axes = |s: [f32; 2]| if lr { [s[1], s[0]] } else { s };
    let index: std::collections::HashMap<&str, usize> = d.nodes.iter().enumerate().map(|(i, n)| (n.id.as_str(), i)).collect();
    let lanes = d.lanes.len();
    let lane_ix: Vec<Option<usize>> = d.nodes.iter().map(|n| n.lane.as_ref().and_then(|l| d.lanes.iter().position(|x| &x.id == l))).collect();
    let mut clusters: Vec<GCluster> = (0..lanes).map(|_| GCluster { parent: None, lead: [0.0, 0.0], min_span: [0.0, 0.0] }).collect();
    let mut node_cluster: Vec<Option<usize>> = lane_ix.clone();
    for (gi, g) in d.groups.iter().enumerate() {
        let parent = g.nodes.first().and_then(|id| index.get(id.as_str())).and_then(|&i| lane_ix[i]);
        let title = chars(&g.label) * 6.5 + 24.0;
        let (lead, min_span) = if lr { ([GROUP_TITLE, 0.0], [0.0, title]) } else { ([0.0, GROUP_TITLE], [title, 0.0]) };
        clusters.push(GCluster { parent, lead, min_span });
        for id in &g.nodes {
            if let Some(&i) = index.get(id.as_str()) {
                node_cluster[i] = Some(lanes + gi);
            }
        }
    }
    let g = Graph {
        sizes: d.nodes.iter().map(|n| axes([n.w, n.h])).collect(),
        edges: d
            .edges
            .iter()
            .filter_map(|e| Some(GEdge { from: *index.get(e.from.as_str())?, to: *index.get(e.to.as_str())?, label: label_size(&e.label).map(axes), minlen: 1 }))
            .collect(),
        node_cluster,
        clusters,
        fixed: (0..lanes).map(|i| Some(i as f32)).collect(),
    };
    let sp = Spacing { node: NODE_GAP, rank: RANK_GAP, cluster: CLUSTER_PAD };
    let placed = layered::layout(&g, &sp);
    // Too tangled for the layered layout (thousands of long-edge points): a plain grid.
    let Some(placed) = placed else {
        grid(d);
        return;
    };
    let mut c = placed.centers;
    // Lane bands: each lane's members moved along their ranks into a band of their own.
    if lanes > 0 {
        let title = LANE_TITLE;
        let mut cursor = 0.0;
        for li in 0..lanes {
            let members: Vec<usize> = (0..d.nodes.len()).filter(|&i| lane_ix[i] == Some(li)).collect();
            let in_lane: Vec<usize> = (0..d.groups.len()).filter(|&gi| d.groups[gi].nodes.iter().any(|id| index.get(id.as_str()).is_some_and(|&i| lane_ix[i] == Some(li)))).collect();
            let snapped = snap_rows(&members, &mut c, &g.sizes, &in_lane, &g.node_cluster, lanes);
            let (mut lo, mut hi) = (f32::MAX, f32::MIN);
            for &i in &members {
                lo = lo.min(c[i][0] - g.sizes[i][0] / 2.0);
                hi = hi.max(c[i][0] + g.sizes[i][0] / 2.0);
            }
            for &gi in &in_lane {
                let b = placed.clusters[lanes + gi];
                if snapped {
                    // Group boxes are drawn around their members: room for the padding and the title.
                    lo = lo.min(lo - CLUSTER_PAD - g.clusters[lanes + gi].lead[0]);
                    hi = hi.max(hi + CLUSTER_PAD);
                } else if b[0] <= b[2] {
                    lo = lo.min(b[0]);
                    hi = hi.max(b[2]);
                }
            }
            let span = if lo <= hi { hi - lo + 2.0 * LANE_PAD } else { EMPTY_LANE };
            if lo <= hi {
                let shift = cursor + LANE_PAD - lo;
                for &i in &members {
                    c[i][0] += shift;
                }
            }
            d.lanes[li].at = cursor;
            d.lanes[li].span = span.round();
            cursor += span.round();
        }
        // Room for the lanes' titles before the first rank.
        for p in &mut c {
            p[1] += title;
        }
    }
    for (i, n) in d.nodes.iter_mut().enumerate() {
        let [cx, cy] = if lr { [c[i][1], c[i][0]] } else { c[i] };
        n.x = (cx - n.w / 2.0).round().clamp(-MAX_COORD, MAX_COORD);
        n.y = (cy - n.h / 2.0).round().clamp(-MAX_COORD, MAX_COORD);
        n.placed = Placed::Layout;
    }
}

/// Straightens one lane: its nodes on each rank (same place across the flow) take rows of one
/// height, in the order the layered layout gave them, ranks with fewer nodes centred. So a lane
/// with one node per rank is one straight line. Left as laid out when one of the lane's groups
/// would then enclose a node that is not in it (a non-member on a rank between the group's).
/// Positions are relative; the lane's band moves them after. Whether it straightened.
fn snap_rows(members: &[usize], c: &mut [[f32; 2]], sizes: &[[f32; 2]], groups: &[usize], node_cluster: &[Option<usize>], lanes: usize) -> bool {
    if members.is_empty() {
        return false;
    }
    for &gi in groups {
        let inside = |i: usize| node_cluster[i] == Some(lanes + gi);
        let (lo, hi) = members.iter().filter(|&&i| inside(i)).fold((f32::MAX, f32::MIN), |a, &i| (a.0.min(c[i][1]), a.1.max(c[i][1])));
        if members.iter().any(|&i| !inside(i) && c[i][1] >= lo && c[i][1] <= hi) {
            return false;
        }
    }
    // Ranks by their place across the flow, each sorted along.
    let mut ranks: Vec<(f32, Vec<usize>)> = Vec::new();
    for &i in members {
        match ranks.iter_mut().find(|r| (r.0 - c[i][1]).abs() < 0.5) {
            Some(r) => r.1.push(i),
            None => ranks.push((c[i][1], vec![i])),
        }
    }
    let rows = ranks.iter().map(|r| r.1.len()).max().unwrap_or(1);
    let row = members.iter().map(|&i| sizes[i][0]).fold(0.0, f32::max) + NODE_GAP;
    for (_, r) in &mut ranks {
        r.sort_by(|&a, &b| c[a][0].total_cmp(&c[b][0]));
        let offset = (rows - r.len()) as f32 * row / 2.0;
        for (k, &i) in r.iter().enumerate() {
            c[i][0] = offset + (k as f32 + 0.5) * row;
        }
    }
    true
}

/// Rows of up to 8 nodes, in array order: the fallback when the layered layout refuses.
fn grid(d: &mut Diagram) {
    let (cw, ch) = d.nodes.iter().fold((0.0f32, 0.0f32), |a, n| (a.0.max(n.w), a.1.max(n.h)));
    for (i, n) in d.nodes.iter_mut().enumerate() {
        n.x = (i % 8) as f32 * (cw + NODE_GAP);
        n.y = (i / 8) as f32 * (ch + NODE_GAP);
        n.placed = Placed::Layout;
    }
    fit_lanes(d);
}

/// Pairs of nodes whose boxes overlap (touching edges do not count).
pub fn overlaps(d: &Diagram) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for (i, a) in d.nodes.iter().enumerate() {
        for b in &d.nodes[i + 1..] {
            if a.x < b.x + b.w && b.x < a.x + a.w && a.y < b.y + b.h && b.y < a.y + a.h {
                out.push((a.id.clone(), b.id.clone()));
            }
        }
    }
    out
}
