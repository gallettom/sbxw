//! A frame without markers allocates nothing in the pill matcher
//! (`terminal::pills::Pills::line`, `emphasis::find`): its own binary, so
//! a counting allocator sees only this test's thread.

use alacritty_terminal::event::VoidListener;
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::Line;
use alacritty_terminal::term::{Config, Term};
use alacritty_terminal::vte::ansi::{Processor, StdSyncHandler};
use sbxw_desktop::emphasis::Vocab;
use sbxw_desktop::terminal::pills::{logical_line, Pills};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::rc::Rc;

struct Counting;
thread_local!(static ALLOCS: Cell<usize> = const { Cell::new(0) });
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        ALLOCS.with(|a| a.set(a.get() + 1));
        unsafe { System.alloc(l) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        unsafe { System.dealloc(p, l) }
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
        ALLOCS.with(|a| a.set(a.get() + 1));
        unsafe { System.realloc(p, l, n) }
    }
}
#[global_allocator]
static A: Counting = Counting;

struct Size(usize, usize);
impl Dimensions for Size {
    fn total_lines(&self) -> usize {
        self.1
    }
    fn screen_lines(&self) -> usize {
        self.1
    }
    fn columns(&self) -> usize {
        self.0
    }
}

fn screen(text: &str) -> Term<VoidListener> {
    let mut t = Term::new(Config::default(), &Size(200, 60), VoidListener);
    let mut p: Processor<StdSyncHandler> = Processor::new();
    p.advance(&mut t, text.as_bytes());
    t
}

fn allocations(t: &Term<VoidListener>, pills: &mut Pills, out: &mut Vec<Vec<sbxw_desktop::terminal::pills::Piece>>) -> usize {
    let grid = t.grid();
    let before = ALLOCS.with(Cell::get);
    for r in 0..60 {
        let (a, b) = logical_line(grid, Line(r));
        pills.line(grid, a, b, out);
    }
    ALLOCS.with(Cell::get) - before
}

#[test]
fn a_frame_without_markers_allocates_nothing() {
    // Agent-like text, single brackets, one `[` at a row's end, wrapped lines.
    let body: String = (0..60).map(|i| format!("\x1b[{};1H  [ok] item {i}: lorem ipsum (see a[0]) {:<120}[", i + 1, "dolor sit amet ".repeat(5))).collect();
    let t = screen(&body);
    let mut pills = Pills::default();
    pills.vocab = Some(Rc::new(Vocab::default()));
    let mut out = Vec::new();
    // The first frame may size the caller's buffer once.
    allocations(&t, &mut pills, &mut out);
    assert_eq!(allocations(&t, &mut pills, &mut out), 0);
    assert_eq!(pills.matched, 0, "nothing passed the pre-check");

    // With markers it does match (and may allocate for the pieces).
    let t = screen("\x1b[1;1HTests [[Done]] and [[Failed: 3 tests]]");
    allocations(&t, &mut pills, &mut out);
    assert!(pills.matched > 0);
}
