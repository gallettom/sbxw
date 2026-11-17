//! Dragging a pane by its bar: where it would land (the drop zones, as
//! Windows Snap shows them) and the rectangle it would take (the preview).
//!
//! Pure and tested; the shell feeds it the rects Rune laid out (`on_layout`,
//! window points, the same space as `Drag::window_point`) and acts only on
//! the drop. Nothing changes while the pointer moves.
//!
//! The zones, first match wins:
//! * the pager's "+": a new page;
//! * a page tab: that page (refused, shown so, when it is full);
//! * within [`EDGE`] of the page area's outer edge: dock along that whole side;
//! * over another pane: its middle ([`CENTRE`] of each axis) swaps the two;
//!   else the half nearest the pointer: split that pane, the dragged one
//!   taking that half;
//! * anywhere else (the dragged pane itself, outside the panes): nothing.

use super::tiling::{self, Rect, Side, Uid};

/// How far in from the page area's edge docking applies, in points.
pub const EDGE: f32 = 20.0;
/// The middle part of a pane (of its width and of its height) that swaps.
pub const CENTRE: f32 = 0.4;
/// The pointer travel (points) before a press on a pane bar becomes a drag.
pub const THRESHOLD: f32 = 4.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Zone {
    Split { target: Uid, side: Side },
    Swap { target: Uid },
    Dock { side: Side },
    /// A page tab (index from 0); `full`: it cannot take one more.
    Page { page: usize, full: bool },
    NewPage,
}

impl Zone {
    /// What a drop there does, in words (the preview's label).
    pub fn label(&self) -> String {
        match self {
            Zone::Split { side, .. } => format!("Split: {}", match side {
                Side::Left => "left half",
                Side::Right => "right half",
                Side::Top => "top half",
                Side::Bottom => "bottom half",
            }),
            Zone::Swap { .. } => "Swap the two panes".into(),
            Zone::Dock { side } => format!("Dock along the {} side", match side {
                Side::Left => "left",
                Side::Right => "right",
                Side::Top => "top",
                Side::Bottom => "bottom",
            }),
            Zone::Page { page, full: false } => format!("Move to page {}", page + 1),
            Zone::Page { page, full: true } => format!("Page {} is full", page + 1),
            Zone::NewPage => "Move to a new page".into(),
        }
    }

    /// Whether a drop there changes something.
    pub fn acts(&self) -> bool {
        !matches!(self, Zone::Page { full: true, .. })
    }
}

/// What the pointer is over, laid out.
pub struct Scene<'a> {
    pub area: Rect,
    pub panes: &'a [(Uid, Rect)],
    pub tabs: &'a [(usize, Rect)],
    pub plus: Option<Rect>,
    pub current: usize,
    pub dragged: Uid,
    /// Whether page `k` has room.
    pub room: &'a dyn Fn(usize) -> bool,
}

pub fn inside(r: Rect, p: [f32; 2]) -> bool {
    p[0] >= r[0] && p[0] < r[0] + r[2] && p[1] >= r[1] && p[1] < r[1] + r[3]
}

/// The zone under `p`.
pub fn zone_at(s: &Scene, p: [f32; 2]) -> Option<Zone> {
    if s.plus.is_some_and(|r| inside(r, p)) {
        return Some(Zone::NewPage);
    }
    if let Some((k, _)) = s.tabs.iter().find(|(_, r)| inside(*r, p)) {
        if *k == s.current {
            return None;
        }
        return Some(Zone::Page { page: *k, full: !(s.room)(*k) });
    }
    let a = s.area;
    if !inside(a, p) {
        return None;
    }
    if s.panes.len() > 1 {
        let d = [(p[0] - a[0], Side::Left), (a[0] + a[2] - p[0], Side::Right), (p[1] - a[1], Side::Top), (a[1] + a[3] - p[1], Side::Bottom)];
        if let Some((_, side)) = d.iter().filter(|(x, _)| *x < EDGE).min_by(|x, y| x.0.total_cmp(&y.0)) {
            return Some(Zone::Dock { side: *side });
        }
    }
    let (target, r) = *s.panes.iter().find(|(_, r)| inside(*r, p))?;
    if target == s.dragged {
        return None;
    }
    let (fx, fy) = ((p[0] - r[0]) / r[2].max(1.0), (p[1] - r[1]) / r[3].max(1.0));
    let lo = (1.0 - CENTRE) / 2.0;
    if (lo..=1.0 - lo).contains(&fx) && (lo..=1.0 - lo).contains(&fy) {
        return Some(Zone::Swap { target });
    }
    // The nearest edge, measured in points so a wide pane's sides are not favoured.
    let d = [(fx * r[2], Side::Left), ((1.0 - fx) * r[2], Side::Right), (fy * r[3], Side::Top), ((1.0 - fy) * r[3], Side::Bottom)];
    let side = d.iter().min_by(|x, y| x.0.total_cmp(&y.0)).unwrap().1;
    Some(Zone::Split { target, side })
}

/// The share of the page a docked pane takes: half with two panes, a
/// third with three, a quarter from four.
pub fn dock_share(panes: usize) -> f32 {
    (1.0 / panes.max(2) as f32).max(0.25)
}

/// The rectangle the dragged pane would take (window points).
pub fn preview(s: &Scene, z: &Zone) -> Option<Rect> {
    let rect_of = |id: Uid| s.panes.iter().find(|(x, _)| *x == id).map(|(_, r)| *r);
    match *z {
        Zone::Split { target, side } => rect_of(target).map(|r| tiling::part(r, side, 0.5)),
        Zone::Swap { target } => rect_of(target),
        Zone::Dock { side } => Some(tiling::part(s.area, side, dock_share(s.panes.len()))),
        Zone::Page { page, .. } => s.tabs.iter().find(|(k, _)| *k == page).map(|(_, r)| *r),
        Zone::NewPage => s.plus,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scene<'a>(panes: &'a [(Uid, Rect)], tabs: &'a [(usize, Rect)], room: &'a dyn Fn(usize) -> bool) -> Scene<'a> {
        Scene { area: [100.0, 100.0, 800.0, 600.0], panes, tabs, plus: Some([300.0, 60.0, 28.0, 28.0]), current: 0, dragged: 1, room }
    }

    #[test]
    fn each_zone_is_found_where_it_should_be() {
        // Two panes side by side: 1 on the left (dragged), 2 on the right.
        let panes = [(1, [100.0, 100.0, 396.0, 600.0]), (2, [504.0, 100.0, 396.0, 600.0])];
        let tabs = [(0, [100.0, 60.0, 90.0, 28.0]), (1, [194.0, 60.0, 90.0, 28.0])];
        let room = |k: usize| k == 1;
        let s = scene(&panes, &tabs, &room);
        let z = |x: f32, y: f32| zone_at(&s, [x, y]);
        assert_eq!(z(702.0, 400.0), Some(Zone::Swap { target: 2 }));
        assert_eq!(z(540.0, 400.0), Some(Zone::Split { target: 2, side: Side::Left }));
        assert_eq!(z(860.0, 400.0), Some(Zone::Split { target: 2, side: Side::Right }));
        assert_eq!(z(702.0, 140.0), Some(Zone::Split { target: 2, side: Side::Top }));
        assert_eq!(z(702.0, 660.0), Some(Zone::Split { target: 2, side: Side::Bottom }));
        assert_eq!(z(890.0, 400.0), Some(Zone::Dock { side: Side::Right }), "the outer edge docks");
        assert_eq!(z(400.0, 690.0), Some(Zone::Dock { side: Side::Bottom }));
        assert_eq!(z(300.0, 400.0), None, "over itself: nothing");
        assert_eq!(z(500.0, 400.0), None, "the gap between panes: nothing");
        assert_eq!(z(50.0, 400.0), None, "outside: nothing");
        assert_eq!(z(200.0, 70.0), Some(Zone::Page { page: 1, full: false }));
        assert_eq!(z(120.0, 70.0), None, "its own page");
        assert_eq!(z(310.0, 70.0), Some(Zone::NewPage));
        let full = |_: usize| false;
        let s2 = scene(&panes, &tabs, &full);
        let zf = zone_at(&s2, [200.0, 70.0]).unwrap();
        assert!(!zf.acts());
        assert_eq!(zf.label(), "Page 2 is full");
    }

    #[test]
    fn the_preview_is_the_rectangle_the_pane_would_take() {
        let panes = [(1, [100.0, 100.0, 396.0, 600.0]), (2, [504.0, 100.0, 396.0, 600.0])];
        let room = |_: usize| true;
        let s = scene(&panes, &[], &room);
        assert_eq!(preview(&s, &Zone::Split { target: 2, side: Side::Bottom }), Some([504.0, 400.0, 396.0, 300.0]));
        assert_eq!(preview(&s, &Zone::Swap { target: 2 }), Some(panes[1].1));
        assert_eq!(preview(&s, &Zone::Dock { side: Side::Top }), Some([100.0, 100.0, 800.0, 300.0]));
        assert_eq!(preview(&s, &Zone::NewPage), Some([300.0, 60.0, 28.0, 28.0]));
        // One pane alone: no docking (nothing to dock against), and over itself nothing.
        let one = [(1, [100.0, 100.0, 800.0, 600.0])];
        let s = scene(&one, &[], &room);
        assert_eq!(zone_at(&s, [105.0, 400.0]), None);
    }
}
