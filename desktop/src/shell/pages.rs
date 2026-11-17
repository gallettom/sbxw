//! Pages of terminals: the open panes live on pages; a page holds up to
//! `per` of them (the person's "terminals per page", 1 to 6) as a tiling
//! (tiling.rs). One page shows at a time; the pager (pager.rs) switches.
//! `per == 1` is the tabbed view: one terminal per page, the pager its tab
//! strip.
//!
//! Pure and tested: pages hold pane ids ([`Uid`]); the shell owns the panes
//! (and their terminals, which never move or reconnect because of a page).
//! A page always has at least one pane: an empty one when nothing is open
//! on it (the caller gives a fresh id for it). A page emptied by a move or
//! a close disappears, unless it is the only one.

use super::tiling::{self, Side, Tile, Uid};
use serde_json::{json, Value as J};

/// The choices the pager offers for terminals per page.
pub const PER_CHOICES: &[usize] = &[1, 2, 3, 4, 6];
pub const PER_MAX: usize = 6;
/// The page's aspect assumed for "the longer side" before it is laid out.
pub const ASPECT: f32 = 1.6;

#[derive(Clone, Debug, PartialEq)]
pub struct Page {
    pub tile: Tile,
    /// The pane that had the keyboard when the page was last shown.
    pub focus: Option<Uid>,
}

impl Page {
    pub fn of(tile: Tile) -> Page {
        Page { tile, focus: None }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Pages {
    pub list: Vec<Page>,
    pub current: usize,
    pub per: usize,
    /// The last `per` above 1: what "Grid" goes back to.
    pub grid_per: usize,
}

/// Where [`Pages::place`] put a new pane.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Placed {
    /// Into this empty pane (the caller fills it; the new id is unused).
    Into(Uid),
    /// The new id was added as a pane.
    Added,
}

impl Pages {
    pub fn new(first: Uid) -> Pages {
        Pages { list: vec![Page::of(Tile::Leaf(first))], current: 0, per: 4, grid_per: 4 }
    }

    /// Every pane, page by page, each page in reading order.
    pub fn all(&self) -> Vec<Uid> {
        self.list.iter().flat_map(|p| p.tile.order()).collect()
    }

    pub fn page_of(&self, id: Uid) -> Option<usize> {
        self.list.iter().position(|p| p.tile.contains(id))
    }

    pub fn cur(&self) -> &Page {
        &self.list[self.current]
    }

    pub fn tabs(&self) -> bool {
        self.per == 1
    }

    /// The order pages are looked at for room: this one, the next ones,
    /// then the ones before.
    fn search_order(&self) -> Vec<usize> {
        (self.current..self.list.len()).chain(0..self.current).collect()
    }

    /// Where a newly opened terminal goes: an empty pane of the current
    /// page, else a new pane there while it has room, else the next page
    /// with an empty pane or room, else a new page. That page becomes current.
    pub fn place(&mut self, id: Uid, empty: &dyn Fn(Uid) -> bool) -> Placed {
        for k in self.search_order() {
            if let Some(e) = self.list[k].tile.order().into_iter().find(|&x| empty(x)) {
                self.current = k;
                return Placed::Into(e);
            }
            if self.list[k].tile.count() < self.per {
                self.list[k].tile.insert(id, ASPECT);
                self.current = k;
                return Placed::Added;
            }
        }
        self.list.push(Page::of(Tile::Leaf(id)));
        self.current = self.list.len() - 1;
        Placed::Added
    }

    /// Removes page `k` (or, the only one, gives it the pane `fresh`).
    /// True when `fresh` was used.
    fn drop_page(&mut self, k: usize, fresh: Uid) -> bool {
        if self.list.len() > 1 {
            self.list.remove(k);
            if self.current > k || self.current >= self.list.len() {
                self.current = self.current.saturating_sub(1);
            }
            false
        } else {
            self.list[k] = Page::of(Tile::Leaf(fresh));
            true
        }
    }

    /// Takes a pane off its page: the shell's way (`retile`, a default
    /// shape stays one) or the person's (its sibling takes the room). True
    /// when `fresh` was used for an emptied only page.
    pub fn remove(&mut self, id: Uid, retile: bool, fresh: Uid) -> bool {
        let Some(k) = self.page_of(id) else { return false };
        let t = self.list[k].tile.clone();
        let rest = if retile { t.remove(id) } else { t.collapse(id) };
        match rest {
            Some(t) => {
                self.list[k].tile = t;
                false
            }
            None => self.drop_page(k, fresh),
        }
    }

    /// Sets terminals per page. Pages with more panes than that give up
    /// their empty panes first, then their last ones in reading order, onto
    /// new pages right after them (in order). Raising it moves nothing
    /// (see [`Pages::gather`]). Returns the empty panes dropped.
    pub fn set_per(&mut self, n: usize, empty: &dyn Fn(Uid) -> bool) -> Vec<Uid> {
        let n = n.clamp(1, PER_MAX);
        self.per = n;
        if n > 1 {
            self.grid_per = n;
        }
        let mut dropped = vec![];
        let mut k = 0;
        while k < self.list.len() {
            let order = self.list[k].tile.order();
            if order.len() > n {
                let mut excess = order.len() - n;
                for e in order.iter().rev().copied().filter(|&x| empty(x)).collect::<Vec<_>>() {
                    if excess == 0 {
                        break;
                    }
                    if let Some(t) = self.list[k].tile.clone().remove(e) {
                        self.list[k].tile = t;
                        dropped.push(e);
                        excess -= 1;
                    }
                }
                let order = self.list[k].tile.order();
                if order.len() > n {
                    let over = order[n..].to_vec();
                    for &o in &over {
                        self.list[k].tile = self.list[k].tile.clone().remove(o).expect("keeps n panes");
                    }
                    for (j, chunk) in over.chunks(n).enumerate() {
                        self.list.insert(k + 1 + j, Page::of(tiling::default(chunk, false)));
                        if self.current > k + j {
                            self.current += 1;
                        }
                    }
                }
            }
            k += 1;
        }
        dropped
    }

    /// Puts every open terminal back on as few pages as `per` allows, in
    /// their order; empty panes go (one stays when nothing is open).
    /// Returns the empty panes dropped.
    pub fn gather(&mut self, empty: &dyn Fn(Uid) -> bool) -> Vec<Uid> {
        let all = self.all();
        let (open, empties): (Vec<Uid>, Vec<Uid>) = all.iter().partition(|&&x| !empty(x));
        if open.is_empty() {
            let keep = empties[0];
            self.list = vec![Page::of(Tile::Leaf(keep))];
            self.current = 0;
            return empties[1..].to_vec();
        }
        self.list = open.chunks(self.per).map(|c| Page::of(tiling::default(c, false))).collect();
        self.current = self.current.min(self.list.len() - 1);
        empties
    }

    /// Closes page `k`: returns its panes (their views close; the sessions go on).
    pub fn close_page(&mut self, k: usize, fresh: Uid) -> Vec<Uid> {
        let Some(p) = self.list.get(k) else { return vec![] };
        let gone = p.tile.leaves();
        self.drop_page(k, fresh);
        gone
    }

    /// A new page with one empty pane, at the end; it becomes current.
    pub fn new_page(&mut self, fresh: Uid) {
        self.list.push(Page::of(Tile::Leaf(fresh)));
        self.current = self.list.len() - 1;
    }

    pub fn show(&mut self, k: usize) {
        self.current = k.min(self.list.len() - 1);
    }

    /// The next (`delta` 1) or previous (-1) page, round.
    pub fn step(&mut self, delta: i32) {
        let n = self.list.len() as i32;
        self.current = (self.current as i32 + delta).rem_euclid(n) as usize;
    }

    /// Moves pane `id` to page `to` (None: a new page at the end), which
    /// becomes current: into an empty pane there, else as one more pane
    /// while it has room. Its old page closes the gap (and goes, emptied).
    /// Returns the empty pane it replaced, if any.
    pub fn move_to_page(&mut self, id: Uid, to: Option<usize>, empty: &dyn Fn(Uid) -> bool) -> Result<Option<Uid>, String> {
        let from = self.page_of(id).ok_or("no such pane")?;
        if to == Some(from) {
            return Err(format!("already on page {}", from + 1));
        }
        if let Some(k) = to {
            let page = self.list.get(k).ok_or_else(|| format!("no page {}", k + 1))?;
            let e = page.tile.order().into_iter().find(|&x| empty(x));
            if e.is_none() && page.tile.count() >= self.per {
                return Err(format!("page {} is full ({} per page)", k + 1, self.per));
            }
        }
        // Out of its page first (a page left empty goes: then indices shift).
        let alone = self.list[from].tile.count() == 1;
        let to = to.map(|k| if alone && k > from { k - 1 } else { k });
        if alone {
            self.list.remove(from);
        } else {
            self.list[from].tile = self.list[from].tile.clone().collapse(id).expect("other panes stay");
        }
        let mut replaced = None;
        match to {
            None => {
                self.list.push(Page::of(Tile::Leaf(id)));
                self.current = self.list.len() - 1;
            }
            Some(k) => {
                let t = &mut self.list[k].tile;
                match t.order().into_iter().find(|&x| empty(x)) {
                    Some(e) => {
                        *t = t.map(&mut |x| Some(if x == e { id } else { x })).expect("same panes");
                        replaced = Some(e);
                    }
                    None => t.insert(id, ASPECT),
                }
                self.current = k;
            }
        }
        Ok(replaced)
    }

    /// Whether page `k` can take one more pane.
    pub fn has_room(&self, k: usize, empty: &dyn Fn(Uid) -> bool) -> bool {
        self.list.get(k).is_some_and(|p| p.tile.count() < self.per || p.tile.leaves().into_iter().any(empty))
    }

    /// On the current page: `id` leaves its place and takes the `side` half
    /// of `target`.
    pub fn split_into(&mut self, id: Uid, target: Uid, side: Side) -> bool {
        let t = &self.list[self.current].tile;
        if id == target || !t.contains(id) || !t.contains(target) {
            return false;
        }
        let Some(mut rest) = t.clone().collapse(id) else { return false };
        rest.split_at(target, side, id);
        self.list[self.current].tile = rest;
        true
    }

    /// The tree of the current page, to change in place.
    pub fn tile_mut(&mut self) -> &mut Tile {
        &mut self.list[self.current].tile
    }

    /// For the state file: each page's tiling with panes as `index(uid)`.
    pub fn to_json(&self, index: &dyn Fn(Uid) -> Option<Uid>) -> J {
        let pages: Vec<J> = self.list.iter().filter_map(|p| p.tile.map(&mut |x| index(x))).map(|t| serde_json::to_value(t).unwrap_or(J::Null)).collect();
        json!({ "per": self.per, "grid_per": self.grid_per, "current": self.current, "pages": pages })
    }

    /// From the state file: `uid(index)` gives a pane's id, None drops it
    /// (its page closes the gap). None when nothing is left.
    pub fn from_json(v: &J, uid: &mut dyn FnMut(Uid) -> Option<Uid>) -> Option<Pages> {
        let per = v["per"].as_u64().map_or(4, |n| n as usize).clamp(1, PER_MAX);
        let grid_per = v["grid_per"].as_u64().map_or(4, |n| n as usize).clamp(2, PER_MAX);
        let list: Vec<Page> = v["pages"]
            .as_array()?
            .iter()
            .filter_map(|p| serde_json::from_value::<Tile>(p.clone()).ok())
            .filter_map(|t| t.map(&mut |x| uid(x)))
            .map(Page::of)
            .collect();
        if list.is_empty() {
            return None;
        }
        let current = (v["current"].as_u64().unwrap_or(0) as usize).min(list.len() - 1);
        Some(Pages { list, current, per, grid_per })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn ids(p: &Pages) -> Vec<Vec<Uid>> {
        p.list.iter().map(|x| x.tile.order()).collect()
    }

    /// Opens `n` terminals with ids from `start`.
    fn open(p: &mut Pages, start: Uid, n: Uid, empties: &HashSet<Uid>) {
        for id in start..start + n {
            let e = |x| empties.contains(&x);
            assert_eq!(p.place(id, &e), Placed::Added);
        }
    }

    #[test]
    fn opening_fills_the_page_then_the_next_with_room_then_a_new_one() {
        let none = HashSet::new();
        let mut p = Pages { list: vec![Page::of(Tile::Leaf(1))], current: 0, per: 2, grid_per: 2 };
        open(&mut p, 2, 4, &none);
        assert_eq!(ids(&p), [vec![1, 2], vec![3, 4], vec![5]]);
        assert_eq!(p.current, 2, "the page it went to shows");
        // An empty pane on the current page is taken before anything else.
        let mut p = Pages::new(1);
        let empty = |x: Uid| x == 1;
        assert_eq!(p.place(7, &empty), Placed::Into(1));
        // From page 1 (full), the next page with room is page 3, not page 2.
        let mut p = Pages { list: vec![Page::of(tiling::default(&[1, 2], false)), Page::of(tiling::default(&[3, 4], false)), Page::of(Tile::Leaf(5))], current: 0, per: 2, grid_per: 2 };
        p.place(6, &|_| false);
        assert_eq!(ids(&p), [vec![1, 2], vec![3, 4], vec![5, 6]]);
        assert_eq!(p.current, 2);
    }

    #[test]
    fn lowering_per_reflows_onto_following_pages_raising_does_not_and_gather_does() {
        let mut p = Pages::new(1);
        open(&mut p, 2, 5, &HashSet::new());
        assert_eq!(ids(&p), [vec![1, 2, 3, 4], vec![5, 6]]);
        p.current = 1;
        let dropped = p.set_per(2, &|_| false);
        assert!(dropped.is_empty());
        assert_eq!(ids(&p), [vec![1, 2], vec![3, 4], vec![5, 6]], "overflow right after its page, in order");
        assert_eq!(p.current, 2, "the same page (5, 6) still shows");
        p.set_per(1, &|_| false);
        assert_eq!(p.list.len(), 6, "tabs: one per page");
        assert!(p.tabs());
        assert_eq!(p.grid_per, 2, "Grid goes back to 2");
        p.set_per(4, &|_| false);
        assert_eq!(p.list.len(), 6, "raising moves nothing");
        p.gather(&|_| false);
        assert_eq!(ids(&p), [vec![1, 2, 3, 4], vec![5, 6]]);
        // Empty panes go first when a page is over.
        let mut p = Pages { list: vec![Page::of(tiling::default(&[1, 9, 2, 8], false))], current: 0, per: 4, grid_per: 4 };
        let e = |x: Uid| x >= 8;
        let dropped = p.set_per(2, &e);
        assert_eq!(dropped, [8, 9]);
        assert_eq!(ids(&p), [vec![1, 2]]);
        // Gather with nothing open keeps one empty pane.
        let mut p = Pages { list: vec![Page::of(Tile::Leaf(8)), Page::of(Tile::Leaf(9))], current: 1, per: 4, grid_per: 4 };
        assert_eq!(p.gather(&|_| true), [9]);
        assert_eq!(ids(&p), [vec![8]]);
    }

    #[test]
    fn closing_panes_and_pages_and_the_emptied_page() {
        let mut p = Pages::new(1);
        open(&mut p, 2, 2, &HashSet::new());
        p.set_per(1, &|_| false);
        assert_eq!(ids(&p), [vec![1], vec![2], vec![3]]);
        p.current = 2;
        assert!(!p.remove(3, true, 99), "an emptied page goes");
        assert_eq!(ids(&p), [vec![1], vec![2]]);
        assert_eq!(p.current, 1);
        assert_eq!(p.close_page(0, 99), [1]);
        assert_eq!(ids(&p), [vec![2]]);
        assert!(p.remove(2, true, 99), "the only page keeps an empty pane");
        assert_eq!(ids(&p), [vec![99]]);
        p.new_page(100);
        assert_eq!((p.list.len(), p.current), (2, 1));
        // Closing a pane re-tiles its page.
        let mut p = Pages::new(1);
        open(&mut p, 2, 3, &HashSet::new());
        p.remove(2, true, 99);
        assert_eq!(p.list[0].tile, tiling::default(&[1, 3, 4], false));
    }

    #[test]
    fn moving_across_pages() {
        let mut p = Pages { list: vec![Page::of(tiling::default(&[1, 2], false)), Page::of(Tile::Leaf(3))], current: 0, per: 2, grid_per: 2 };
        assert_eq!(p.move_to_page(1, Some(1), &|_| false), Ok(None));
        assert_eq!(ids(&p), [vec![2], vec![3, 1]]);
        assert_eq!(p.current, 1);
        assert!(p.move_to_page(2, Some(1), &|_| false).unwrap_err().contains("full"));
        // The last pane of a page leaves: the page goes, the target's index follows.
        assert_eq!(p.move_to_page(2, None, &|_| false), Ok(None));
        assert_eq!(ids(&p), [vec![3, 1], vec![2]]);
        let mut p = Pages { list: vec![Page::of(Tile::Leaf(1)), Page::of(tiling::default(&[2, 3], false)), Page::of(Tile::Leaf(9))], current: 0, per: 2, grid_per: 2 };
        assert_eq!(p.move_to_page(1, Some(2), &|x| x == 9), Ok(Some(9)), "into the empty pane");
        assert_eq!(ids(&p), [vec![2, 3], vec![1]]);
        assert_eq!(p.current, 1);
    }

    #[test]
    fn drops_on_the_current_page() {
        let mut p = Pages::new(1);
        open(&mut p, 2, 2, &HashSet::new());
        assert!(p.split_into(1, 3, Side::Bottom));
        assert_eq!(p.cur().tile.order(), [2, 3, 1]);
        assert!(!p.split_into(1, 1, Side::Left));
    }

    #[test]
    fn it_persists_and_drops_what_cannot_come_back() {
        let mut p = Pages::new(10);
        open(&mut p, 11, 4, &HashSet::new());
        p.set_per(3, &|_| false);
        let index = |u: Uid| Some(u - 10);
        let j = p.to_json(&index);
        let back = Pages::from_json(&j, &mut |i| Some(i + 10)).unwrap();
        assert_eq!(back.list.iter().map(|x| x.tile.clone()).collect::<Vec<_>>(), p.list.iter().map(|x| x.tile.clone()).collect::<Vec<_>>());
        assert_eq!((back.per, back.grid_per, back.current), (3, 3, p.current));
        // A pane that cannot come back (its sandbox is stopped): its page closes the gap.
        let back = Pages::from_json(&j, &mut |i| (i != 3 && i != 4).then_some(i + 10)).unwrap();
        assert_eq!(ids(&back), [vec![10, 11, 12]]);
        assert!(Pages::from_json(&j, &mut |_| None).is_none());
    }
}
