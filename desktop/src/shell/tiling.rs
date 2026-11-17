//! One page's tiling: a binary tree of splits whose leaves are panes (by
//! their stable id, [`Uid`]). A split is side by side (`Row`: `a` left of
//! `b`) or stacked (`Col`: `a` above `b`), `ratio` being `a`'s share. Any
//! pane can so be tall (portrait) or wide (landscape).
//!
//! Pure and tested: the shell maps uids to its panes and draws the tree
//! (shell.rs, `tile_view`).
//!
//! Two ways a pane leaves or joins a tree:
//! * the shell's own (open a sandbox, close a pane, re-flow): while the tree
//!   still has the **default shape** for its count (the old fixed layouts:
//!   1, 2 side by side or stacked, 3 = one left and two stacked right,
//!   4 = a 2x2 grid, then 5 and 6 on three columns), it goes to the default
//!   shape for one more or one less, panes in reading order; otherwise a
//!   new pane splits the largest pane along its longer side, and a removed
//!   one gives its room to its sibling.
//! * the person's (drag and drop, Move, Rotate, Dock): exactly what was
//!   asked; a removed pane always gives its room to its sibling.

use serde::{Deserialize, Serialize};

pub type Uid = u64;
/// `[x, y, w, h]`.
pub type Rect = [f32; 4];

/// The least size of a pane, in points: about 14 columns by 3 rows of a
/// 14 pt terminal under its 32 pt bar. Dividers stop there.
pub const MIN_W: f32 = 120.0;
pub const MIN_H: f32 = 80.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Dir {
    /// Side by side.
    Row,
    /// Stacked.
    Col,
}

impl Dir {
    pub fn flip(self) -> Dir {
        match self {
            Dir::Row => Dir::Col,
            Dir::Col => Dir::Row,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
    Top,
    Bottom,
}

impl Side {
    pub fn dir(self) -> Dir {
        match self {
            Side::Left | Side::Right => Dir::Row,
            Side::Top | Side::Bottom => Dir::Col,
        }
    }
    /// Whether the pane put on this side comes first (`a`).
    pub fn first(self) -> bool {
        matches!(self, Side::Left | Side::Top)
    }
    pub fn name(self) -> &'static str {
        match self {
            Side::Left => "left",
            Side::Right => "right",
            Side::Top => "up",
            Side::Bottom => "down",
        }
    }
    pub fn parse(s: &str) -> Option<Side> {
        Some(match s {
            "left" => Side::Left,
            "right" => Side::Right,
            "up" | "top" => Side::Top,
            "down" | "bottom" => Side::Bottom,
            _ => return None,
        })
    }
    pub const ALL: [Side; 4] = [Side::Left, Side::Right, Side::Top, Side::Bottom];
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Tile {
    Leaf(Uid),
    Split { dir: Dir, ratio: f32, a: Box<Tile>, b: Box<Tile> },
}

fn split(dir: Dir, ratio: f32, a: Tile, b: Tile) -> Tile {
    Tile::Split { dir, ratio, a: Box::new(a), b: Box::new(b) }
}

/// The default shape for these panes, in reading order (`rows`: two panes stacked).
pub fn default(ids: &[Uid], rows: bool) -> Tile {
    let l = |i: usize| Tile::Leaf(ids[i]);
    match ids.len() {
        0 => panic!("a tiling has at least one pane"),
        1 => l(0),
        2 => split(if rows { Dir::Col } else { Dir::Row }, 0.5, l(0), l(1)),
        3 => split(Dir::Row, 0.5, l(0), split(Dir::Col, 0.5, l(1), l(2))),
        4 => split(Dir::Row, 0.5, split(Dir::Col, 0.5, l(0), l(2)), split(Dir::Col, 0.5, l(1), l(3))),
        5 => split(Dir::Row, 1.0 / 3.0, split(Dir::Col, 0.5, l(0), l(3)), split(Dir::Row, 0.5, split(Dir::Col, 0.5, l(1), l(4)), l(2))),
        6 => split(Dir::Row, 1.0 / 3.0, split(Dir::Col, 0.5, l(0), l(3)), split(Dir::Row, 0.5, split(Dir::Col, 0.5, l(1), l(4)), split(Dir::Col, 0.5, l(2), l(5)))),
        n => {
            let mut t = default(&ids[..6], false);
            for &id in &ids[6..] {
                t.split_largest(id, 1.6);
            }
            let _ = n;
            t
        }
    }
}

impl Tile {
    pub fn count(&self) -> usize {
        match self {
            Tile::Leaf(_) => 1,
            Tile::Split { a, b, .. } => a.count() + b.count(),
        }
    }

    /// The leaves, depth first (`a` before `b`).
    pub fn leaves(&self) -> Vec<Uid> {
        let mut v = vec![];
        self.walk(&mut |id| v.push(id));
        v
    }

    fn walk(&self, f: &mut impl FnMut(Uid)) {
        match self {
            Tile::Leaf(id) => f(*id),
            Tile::Split { a, b, .. } => {
                a.walk(f);
                b.walk(f);
            }
        }
    }

    pub fn contains(&self, id: Uid) -> bool {
        match self {
            Tile::Leaf(x) => *x == id,
            Tile::Split { a, b, .. } => a.contains(id) || b.contains(id),
        }
    }

    /// Each pane's rect in `area`, with `gap` between the two sides of a split.
    pub fn rects(&self, area: Rect, gap: f32) -> Vec<(Uid, Rect)> {
        let mut v = vec![];
        self.rects_into(area, gap, &mut v);
        v
    }

    fn rects_into(&self, r: Rect, gap: f32, out: &mut Vec<(Uid, Rect)>) {
        match self {
            Tile::Leaf(id) => out.push((*id, r)),
            Tile::Split { dir, ratio, a, b } => {
                let (ra, rb) = halves(r, *dir, *ratio, gap);
                a.rects_into(ra, gap, out);
                b.rects_into(rb, gap, out);
            }
        }
    }

    /// The panes in reading order: top to bottom, then left to right, by
    /// where their top-left corner is.
    pub fn order(&self) -> Vec<Uid> {
        let mut r = self.rects([0.0, 0.0, 1.0, 1.0], 0.0);
        r.sort_by(|(_, p), (_, q)| (p[1] * 1000.0).round().total_cmp(&(q[1] * 1000.0).round()).then(p[0].total_cmp(&q[0])));
        r.into_iter().map(|(id, _)| id).collect()
    }

    /// Same structure (directions and nesting), whatever the ids and ratios.
    fn same_shape(&self, o: &Tile) -> bool {
        match (self, o) {
            (Tile::Leaf(_), Tile::Leaf(_)) => true,
            (Tile::Split { dir: d1, a: a1, b: b1, .. }, Tile::Split { dir: d2, a: a2, b: b2, .. }) => d1 == d2 && a1.same_shape(a2) && b1.same_shape(b2),
            _ => false,
        }
    }

    /// Whether this is a default shape: `Some(rows)` (rows: two stacked).
    pub fn default_shape(&self) -> Option<bool> {
        let ids = self.order();
        [false, true].into_iter().find(|&rows| (rows == false || ids.len() == 2) && self.same_shape(&default(&ids, rows)))
    }

    /// Adds a pane the shell's way (see the module's doc). `aspect`: the
    /// page's width over its height, for "the longer side".
    pub fn insert(&mut self, id: Uid, aspect: f32) {
        if let Some(_rows) = self.default_shape() {
            let mut ids = self.order();
            ids.push(id);
            *self = default(&ids, false);
            return;
        }
        self.split_largest(id, aspect);
    }

    fn split_largest(&mut self, id: Uid, aspect: f32) {
        let rects = self.rects([0.0, 0.0, aspect.max(0.1), 1.0], 0.0);
        let Some((big, r)) = rects.into_iter().max_by(|(_, p), (_, q)| (p[2] * p[3]).total_cmp(&(q[2] * q[3]))) else { return };
        let side = if r[2] >= r[3] { Side::Right } else { Side::Bottom };
        self.split_at(big, side, id);
    }

    /// Removes a pane the shell's way: a default shape stays one (the
    /// others in reading order); else its sibling takes its room. None when
    /// it was the only pane.
    pub fn remove(self, id: Uid) -> Option<Tile> {
        if !self.contains(id) {
            return Some(self);
        }
        if self.count() > 1 && self.default_shape().is_some() {
            let ids: Vec<Uid> = self.order().into_iter().filter(|x| *x != id).collect();
            return Some(default(&ids, false));
        }
        self.collapse(id)
    }

    /// Removes a pane, its sibling taking its room (ratios elsewhere kept).
    pub fn collapse(self, id: Uid) -> Option<Tile> {
        match self {
            Tile::Leaf(x) if x == id => None,
            Tile::Leaf(_) => Some(self),
            Tile::Split { dir, ratio, a, b } => match (a.collapse(id), b.collapse(id)) {
                (Some(a), Some(b)) => Some(split(dir, ratio, a, b)),
                (Some(x), None) | (None, Some(x)) => Some(x),
                (None, None) => None,
            },
        }
    }

    /// Splits pane `target` in two, `id` taking the half on `side`.
    pub fn split_at(&mut self, target: Uid, side: Side, id: Uid) -> bool {
        match self {
            Tile::Leaf(x) if *x == target => {
                let (a, b) = if side.first() { (Tile::Leaf(id), Tile::Leaf(target)) } else { (Tile::Leaf(target), Tile::Leaf(id)) };
                *self = split(side.dir(), 0.5, a, b);
                true
            }
            Tile::Leaf(_) => false,
            Tile::Split { a, b, .. } => a.split_at(target, side, id) || b.split_at(target, side, id),
        }
    }

    /// The two panes trade places.
    pub fn swap(&mut self, x: Uid, y: Uid) {
        match self {
            Tile::Leaf(id) if *id == x => *id = y,
            Tile::Leaf(id) if *id == y => *id = x,
            Tile::Leaf(_) => {}
            Tile::Split { a, b, .. } => {
                a.swap(x, y);
                b.swap(x, y);
            }
        }
    }

    /// Takes pane `id` out and docks it along the whole `side` of the page,
    /// with `share` of it. Not when it is the only pane.
    pub fn dock(self, id: Uid, side: Side, share: f32) -> Tile {
        let Some(rest) = self.clone().collapse(id) else { return self };
        let (a, b, ratio) = if side.first() { (Tile::Leaf(id), rest, share) } else { (rest, Tile::Leaf(id), 1.0 - share) };
        split(side.dir(), ratio, a, b)
    }

    /// The pane and its sibling switch between side by side and stacked.
    /// False for a page of one pane.
    pub fn rotate(&mut self, id: Uid) -> bool {
        match self {
            Tile::Leaf(_) => false,
            Tile::Split { dir, a, b, .. } => {
                if matches!(**a, Tile::Leaf(x) if x == id) || matches!(**b, Tile::Leaf(x) if x == id) {
                    *dir = dir.flip();
                    return true;
                }
                a.rotate(id) || b.rotate(id)
            }
        }
    }

    /// The pane next to `id` on `side` (most shared edge, then nearest), by
    /// where they are drawn in a page of that aspect.
    pub fn neighbour(&self, id: Uid, side: Side, aspect: f32) -> Option<Uid> {
        let rects = self.rects([0.0, 0.0, aspect.max(0.1), 1.0], 0.0);
        let r = rects.iter().find(|(x, _)| *x == id)?.1;
        let e = 1e-4;
        let overlap = |a0: f32, a1: f32, b0: f32, b1: f32| (a1.min(b1) - a0.max(b0)).max(0.0);
        rects
            .iter()
            .filter(|(x, _)| *x != id)
            .filter_map(|(x, q)| {
                let (gap, ov) = match side {
                    Side::Left => (r[0] - (q[0] + q[2]), overlap(r[1], r[1] + r[3], q[1], q[1] + q[3])),
                    Side::Right => (q[0] - (r[0] + r[2]), overlap(r[1], r[1] + r[3], q[1], q[1] + q[3])),
                    Side::Top => (r[1] - (q[1] + q[3]), overlap(r[0], r[0] + r[2], q[0], q[0] + q[2])),
                    Side::Bottom => (q[1] - (r[1] + r[3]), overlap(r[0], r[0] + r[2], q[0], q[0] + q[2])),
                };
                (gap > -e && ov > e).then_some((*x, gap, ov))
            })
            .min_by(|a, b| a.1.total_cmp(&b.1).then(b.2.total_cmp(&a.2)))
            .map(|(x, _, _)| x)
    }

    /// Whether the pane already spans the whole `side` of the page.
    pub fn spans(&self, id: Uid, side: Side) -> bool {
        match self {
            Tile::Leaf(x) => *x == id,
            Tile::Split { dir, a, b, .. } => {
                if *dir == side.dir() {
                    let first = if side.first() { a } else { b };
                    matches!(**first, Tile::Leaf(x) if x == id)
                } else {
                    false
                }
            }
        }
    }

    /// The split at `path` (`a`/`b` steps from the root; "" is the root).
    pub fn at_mut(&mut self, path: &str) -> Option<&mut Tile> {
        let mut t = self;
        for c in path.chars() {
            t = match t {
                Tile::Split { a, b, .. } => {
                    if c == 'a' {
                        a
                    } else {
                        b
                    }
                }
                Tile::Leaf(_) => return None,
            };
        }
        Some(t)
    }

    pub fn at(&self, path: &str) -> Option<&Tile> {
        let mut t = self;
        for c in path.chars() {
            t = match t {
                Tile::Split { a, b, .. } => {
                    if c == 'a' {
                        a
                    } else {
                        b
                    }
                }
                Tile::Leaf(_) => return None,
            };
        }
        Some(t)
    }

    /// The least length this tile can have along `dir`, with `gap` between panes.
    pub fn min_len(&self, dir: Dir, gap: f32) -> f32 {
        match self {
            Tile::Leaf(_) => match dir {
                Dir::Row => MIN_W,
                Dir::Col => MIN_H,
            },
            Tile::Split { dir: d, a, b, .. } if *d == dir => a.min_len(dir, gap) + gap + b.min_len(dir, gap),
            Tile::Split { a, b, .. } => a.min_len(dir, gap).max(b.min_len(dir, gap)),
        }
    }

    /// Sets the ratio of the split at `path`, kept so both sides keep their
    /// least size in `len` points (when they can both fit; else the middle).
    pub fn set_ratio(&mut self, path: &str, ratio: f32, len: f32, gap: f32) -> f32 {
        let Some(Tile::Split { dir, ratio: r, a, b }) = self.at_mut(path) else { return 0.5 };
        let room = (len - gap).max(1.0);
        let (lo, hi) = (a.min_len(*dir, gap) / room, 1.0 - b.min_len(*dir, gap) / room);
        *r = if lo <= hi { ratio.clamp(lo, hi) } else { 0.5 };
        *r
    }

    /// How many panes lie along `dir` (for equal shares).
    fn weight(&self, dir: Dir) -> f32 {
        match self {
            Tile::Leaf(_) => 1.0,
            Tile::Split { dir: d, a, b, .. } if *d == dir => a.weight(dir) + b.weight(dir),
            Tile::Split { a, b, .. } => a.weight(dir).max(b.weight(dir)),
        }
    }

    /// Equal shares: every split under this one gives each pane along its
    /// direction the same length.
    pub fn equalise(&mut self) {
        if let Tile::Split { dir, ratio, a, b } = self {
            let (wa, wb) = (a.weight(*dir), b.weight(*dir));
            *ratio = wa / (wa + wb);
            a.equalise();
            b.equalise();
        }
    }

    /// The same tree with each leaf mapped (persistence: uid to index and back).
    pub fn map(&self, f: &mut impl FnMut(Uid) -> Option<Uid>) -> Option<Tile> {
        match self {
            Tile::Leaf(x) => f(*x).map(Tile::Leaf),
            Tile::Split { dir, ratio, a, b } => match (a.map(f), b.map(f)) {
                (Some(a), Some(b)) => Some(split(*dir, *ratio, a, b)),
                (Some(x), None) | (None, Some(x)) => Some(x),
                (None, None) => None,
            },
        }
    }
}

/// The two parts of `r` a split makes.
pub fn halves(r: Rect, dir: Dir, ratio: f32, gap: f32) -> (Rect, Rect) {
    match dir {
        Dir::Row => {
            let wa = (r[2] - gap).max(0.0) * ratio;
            ([r[0], r[1], wa, r[3]], [r[0] + wa + gap, r[1], (r[2] - gap - wa).max(0.0), r[3]])
        }
        Dir::Col => {
            let ha = (r[3] - gap).max(0.0) * ratio;
            ([r[0], r[1], r[2], ha], [r[0], r[1] + ha + gap, r[2], (r[3] - gap - ha).max(0.0)])
        }
    }
}

/// The part of `r` on `side`, `share` of it.
pub fn part(r: Rect, side: Side, share: f32) -> Rect {
    match side {
        Side::Left => [r[0], r[1], r[2] * share, r[3]],
        Side::Right => [r[0] + r[2] * (1.0 - share), r[1], r[2] * share, r[3]],
        Side::Top => [r[0], r[1], r[2], r[3] * share],
        Side::Bottom => [r[0], r[1] + r[3] * (1.0 - share), r[2], r[3] * share],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shape(t: &Tile) -> String {
        match t {
            Tile::Leaf(x) => x.to_string(),
            Tile::Split { dir, a, b, .. } => format!("{}({},{})", if *dir == Dir::Row { "R" } else { "C" }, shape(a), shape(b)),
        }
    }

    #[test]
    fn the_default_shapes_are_the_old_layouts_in_reading_order() {
        assert_eq!(shape(&default(&[1, 2], false)), "R(1,2)");
        assert_eq!(shape(&default(&[1, 2], true)), "C(1,2)");
        assert_eq!(shape(&default(&[1, 2, 3], false)), "R(1,C(2,3))");
        assert_eq!(shape(&default(&[1, 2, 3, 4], false)), "R(C(1,3),C(2,4))");
        for n in 1..=6u64 {
            let ids: Vec<Uid> = (1..=n).collect();
            let t = default(&ids, false);
            assert_eq!(t.order(), ids, "{n} panes read in order");
            assert_eq!(t.default_shape(), Some(false));
        }
        assert_eq!(default(&[1, 2], true).default_shape(), Some(true));
    }

    #[test]
    fn insert_and_remove_keep_a_default_shape_else_split_the_largest() {
        let mut t = Tile::Leaf(1);
        for id in 2..=4 {
            t.insert(id, 1.6);
        }
        assert_eq!(shape(&t), "R(C(1,3),C(2,4))");
        let t = t.remove(2).unwrap();
        assert_eq!(shape(&t), "R(1,C(3,4))", "back to the 3-pane default, in reading order");
        // Not a default shape: the largest pane splits along its longer side.
        let mut t = split(Dir::Col, 0.5, split(Dir::Row, 0.5, Tile::Leaf(1), Tile::Leaf(2)), Tile::Leaf(3));
        t.insert(4, 1.6);
        assert_eq!(shape(&t), "C(R(1,2),R(3,4))");
        let t = t.remove(4).unwrap();
        assert_eq!(shape(&t), "C(R(1,2),3)", "its sibling takes the room");
        // A dragged ratio is still the default shape: it grows as the old layouts did.
        let mut t = split(Dir::Col, 0.7, Tile::Leaf(1), Tile::Leaf(2));
        t.insert(3, 1.6);
        assert_eq!(shape(&t), "R(1,C(2,3))");
        assert_eq!(Tile::Leaf(1).remove(1), None);
    }

    #[test]
    fn split_each_direction_swap_rotate_dock() {
        for (side, want) in [(Side::Left, "R(9,1)"), (Side::Right, "R(1,9)"), (Side::Top, "C(9,1)"), (Side::Bottom, "C(1,9)")] {
            let mut t = Tile::Leaf(1);
            t.split_at(1, side, 9);
            assert_eq!(shape(&t), want, "{side:?}");
        }
        let mut t = default(&[1, 2, 3], false);
        t.swap(1, 3);
        assert_eq!(shape(&t), "R(3,C(2,1))");
        // Rotate: 2 and its sibling go from stacked to side by side (portrait to landscape).
        assert!(t.rotate(2));
        assert_eq!(shape(&t), "R(3,R(2,1))");
        let mut one = Tile::Leaf(1);
        assert!(!one.rotate(1));
        // Dock 1 along the whole bottom.
        let t = default(&[1, 2, 3, 4], false).dock(1, Side::Bottom, 0.25);
        assert_eq!(shape(&t), "C(R(3,C(2,4)),1)");
        assert!(t.spans(1, Side::Bottom));
        if let Tile::Split { ratio, .. } = t {
            assert!((ratio - 0.75).abs() < 1e-6);
        }
    }

    #[test]
    fn neighbours_follow_the_drawing() {
        let t = default(&[1, 2, 3, 4], false);
        assert_eq!(t.neighbour(1, Side::Right, 1.6), Some(2));
        assert_eq!(t.neighbour(1, Side::Bottom, 1.6), Some(3));
        assert_eq!(t.neighbour(4, Side::Left, 1.6), Some(3));
        assert_eq!(t.neighbour(4, Side::Top, 1.6), Some(2));
        assert_eq!(t.neighbour(1, Side::Left, 1.6), None);
        let t = default(&[1, 2, 3], false);
        assert_eq!(t.neighbour(3, Side::Left, 1.6), Some(1));
    }

    #[test]
    fn ratios_keep_the_least_size_and_equalise() {
        let mut t = default(&[1, 2, 3], false);
        let r = t.set_ratio("", 0.01, 600.0, 8.0);
        assert!((r * 592.0 - MIN_W).abs() < 0.01, "stops at the least width: {r}");
        let r = t.set_ratio("b", 0.99, 300.0, 8.0);
        assert!(((1.0 - r) * 292.0 - MIN_H).abs() < 0.01, "stops at the least height: {r}");
        // Too small for both: the middle.
        assert_eq!(t.set_ratio("", 0.2, 200.0, 8.0), 0.5);
        let mut t = split(Dir::Row, 0.8, Tile::Leaf(1), split(Dir::Row, 0.2, Tile::Leaf(2), Tile::Leaf(3)));
        t.equalise();
        let w: Vec<f32> = t.rects([0.0, 0.0, 300.0, 100.0], 0.0).iter().map(|(_, r)| r[2]).collect();
        assert!(w.iter().all(|x| (x - 100.0).abs() < 0.01), "{w:?}");
        assert!((default(&[1, 2, 3, 4, 5, 6], false).min_len(Dir::Row, 8.0) - (MIN_W * 3.0 + 16.0)).abs() < 0.01);
    }

    #[test]
    fn it_round_trips_through_json() {
        let t = default(&[1, 2, 3], false);
        let j = serde_json::to_value(&t).unwrap();
        assert_eq!(serde_json::from_value::<Tile>(j).unwrap(), t);
        let m = t.map(&mut |x| (x != 2).then_some(x * 10)).unwrap();
        assert_eq!(shape(&m), "R(10,30)");
    }
}
