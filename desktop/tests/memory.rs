//! What a terminal's history costs on the heap, measured with the app's own
//! counting allocator (`sbxw_desktop::mem`): its own binary, so the count is
//! this test's. MEMORY.md (root artifacts folder) has the numbers.
//!
//! * `a_terminal_s_history_costs_what_its_width_says`: 10,000 lines kept at
//!   200 and at 62 columns stay within 25% of lines x columns x 24 bytes (a
//!   cell) + 32 (a row): a 2x regression fails.
//! * `rows_take_the_narrower_width_once_it_holds`: a terminal filled at 200
//!   columns, then narrowed to 62, gives back the wider rows' memory after
//!   `compact_rows` (it kept the 200-column rows before).
//! * `costs` (`--ignored`, run in release): the time of a compaction, of a
//!   scroll through 10,000 lines, and of a full 200x60 screen of pills.

use sbxw_desktop::client::{TermEvent, TermInput, TerminalConn};
use sbxw_desktop::mem;
use sbxw_desktop::terminal::TerminalState;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Mutex;
use std::time::Instant;

#[global_allocator]
static A: mem::Counting = mem::Counting;

/// The tests share the one counter: one at a time.
static ONE: Mutex<()> = Mutex::new(());

const CELL_ROW: (usize, usize) = (24, 32);

fn term(cols: u16, rows: u16) -> (TerminalState, Sender<TermEvent>, Receiver<TermInput>) {
    let (ev_tx, ev_rx) = mpsc::channel();
    let (in_tx, in_rx) = mpsc::channel();
    let mut t = TerminalState::new(TerminalConn { events: ev_rx, input: in_tx });
    ev_tx.send(TermEvent::Size { cols, rows, owner: true }).unwrap();
    t.poll(None);
    (t, ev_tx, in_rx)
}

/// Agent-like output: colours, bold, wide characters; `n` lines, each
/// shorter than 62 columns so no line wraps at either width.
fn output(n: usize) -> Vec<u8> {
    let words = ["Read", "Edit", "Bash", "src/area/File.tsx", "tests", "passed", "the", "checkout", "step", "日本語", "✅"];
    let mut s = String::new();
    for i in 0..n {
        s.push_str(&format!("\x1b[2m{i:6}\x1b[0m "));
        for k in 0..(3 + i % 6) {
            let w = words[(i * 7 + k * 3) % words.len()];
            match (i + k) % 5 {
                0 => s.push_str(&format!("\x1b[38;5;{}m{w}\x1b[0m ", 16 + (i + k) % 200)),
                1 => s.push_str(&format!("\x1b[1;3{}m{w}\x1b[0m ", 1 + k % 6)),
                _ => {
                    s.push_str(w);
                    s.push(' ');
                }
            }
        }
        s.push_str("\r\n");
    }
    s.into_bytes()
}

fn feed(t: &mut TerminalState, tx: &Sender<TermEvent>, bytes: Vec<u8>) {
    for chunk in bytes.chunks(64 * 1024) {
        tx.send(TermEvent::Output(chunk.to_vec())).unwrap();
        t.poll(None);
    }
}

fn expected(lines: usize, cols: usize) -> u64 {
    (lines * (cols * CELL_ROW.0 + CELL_ROW.1)) as u64
}

#[test]
fn a_terminal_s_history_costs_what_its_width_says() {
    let _one = ONE.lock().unwrap_or_else(|e| e.into_inner());
    mem::count();
    for cols in [200u16, 62] {
        let out = output(12_000);
        let before = mem::live().unwrap();
        let (mut t, tx, _rx) = term(cols, 60);
        feed(&mut t, &tx, out);
        let used = mem::live().unwrap() - before;
        let m = t.mem();
        assert_eq!(m.lines, sbxw_desktop::terminal::SCROLLBACK + 60, "history full");
        let want = expected(m.lines, cols as usize);
        eprintln!("{cols} columns: {} lines, {:.1} MB on the heap (lines x cols x 24 + 32 = {:.1} MB)", m.lines, used as f64 / 1048576.0, want as f64 / 1048576.0);
        assert!(used < want * 5 / 4 + (4 << 20), "{cols} columns: {used} bytes for {} lines, over 1.25 x {want} + 4 MiB (vte's 2 MiB buffer, the screens)", m.lines);
        drop(t);
    }
}

#[test]
fn rows_take_the_narrower_width_once_it_holds() {
    let _one = ONE.lock().unwrap_or_else(|e| e.into_inner());
    mem::count();
    let out = output(12_000);
    let before = mem::live().unwrap();
    let (mut t, tx, _rx) = term(200, 60);
    feed(&mut t, &tx, out);
    let wide = mem::live().unwrap() - before;
    tx.send(TermEvent::Size { cols: 62, rows: 60, owner: true }).unwrap();
    t.poll(None);
    let narrowed = mem::live().unwrap() - before;
    let text = t.visible_text();
    t.scroll_lines(5_000);
    let history = t.visible_text();
    t.scroll_to_bottom();
    t.compact_rows();
    let compact = mem::live().unwrap() - before;
    let m = t.mem();
    eprintln!("200 columns {:.1} MB, narrowed to 62 {:.1} MB, compacted {:.1} MB ({} lines)", wide as f64 / 1048576.0, narrowed as f64 / 1048576.0, compact as f64 / 1048576.0, m.lines);
    assert!(narrowed > wide * 8 / 10, "narrowing alone keeps the wide rows (alacritty's Row::shrink keeps the capacity): {narrowed} of {wide}");
    assert!(compact < expected(m.lines, 62) * 5 / 4 + (4 << 20), "compacted: {compact} bytes, {} lines at 62 columns", m.lines);
    assert_eq!(t.visible_text(), text, "the same text on screen");
    t.scroll_lines(5_000);
    assert_eq!(t.visible_text(), history, "the history reads as before");
    assert!(history.contains("checkout") || history.contains("Read"), "{history}");
}

/// `cargo test --release --test memory -- --ignored --nocapture costs`
#[test]
#[ignore]
fn costs() {
    let _one = ONE.lock().unwrap_or_else(|e| e.into_inner());
    let (mut t, tx, _rx) = term(200, 60);
    feed(&mut t, &tx, output(12_000));
    // A scroll through the whole history, a line at a time, each drawn.
    let t0 = Instant::now();
    let mut steps = 0;
    while t.display_offset() < t.history_len() {
        t.scroll_lines(1);
        t.poll(None);
        steps += 1;
    }
    let per = t0.elapsed().as_secs_f64() * 1e6 / steps as f64;
    eprintln!("scroll: {steps} lines, {per:.1} us a line (rows rebuilt and drawn data built)");
    t.scroll_to_bottom();
    t.poll(None);
    // Compaction of 10,060 rows at 200 columns, after a narrowing to 120.
    tx.send(TermEvent::Size { cols: 120, rows: 60, owner: true }).unwrap();
    t.poll(None);
    let t0 = Instant::now();
    t.compact_rows();
    eprintln!("compaction: {:.2} ms for {} lines at 120 columns", t0.elapsed().as_secs_f64() * 1000.0, t.mem().lines);
    // The same scroll over the compacted rows.
    let t0 = Instant::now();
    let mut steps = 0;
    while t.display_offset() < t.history_len() {
        t.scroll_lines(1);
        t.poll(None);
        steps += 1;
    }
    eprintln!("scroll after compaction: {steps} lines, {:.1} us a line", t0.elapsed().as_secs_f64() * 1e6 / steps as f64);
    pills_cost();
}

/// EMPHASIS.md's measure: the pills of a full 200x60 screen with markers on
/// every row, and with none.
fn pills_cost() {
    use alacritty_terminal::grid::Dimensions;
    use alacritty_terminal::index::Line;
    use alacritty_terminal::term::{Config, Term};
    use alacritty_terminal::vte::ansi::{Processor, StdSyncHandler};
    use sbxw_desktop::terminal::pills::{logical_line, Pills};
    struct Size;
    impl Dimensions for Size {
        fn total_lines(&self) -> usize {
            60
        }
        fn screen_lines(&self) -> usize {
            60
        }
        fn columns(&self) -> usize {
            200
        }
    }
    for (name, row) in [("markers on every row", "Tests [[Done]] and [[Failed: 3 tests]] then [[Needs you]]"), ("no markers", "Tests done and three failed, then nothing for you")] {
        let body: String = (0..60).map(|i| format!("\x1b[{};1H{row} {i}", i + 1)).collect();
        let mut term = Term::new(Config::default(), &Size, alacritty_terminal::event::VoidListener);
        let mut p: Processor<StdSyncHandler> = Processor::new();
        p.advance(&mut term, body.as_bytes());
        let mut pills = Pills::default();
        pills.vocab = Some(std::rc::Rc::new(sbxw_desktop::emphasis::Vocab::default()));
        let mut out = Vec::new();
        let n = 2_000;
        let t0 = Instant::now();
        for _ in 0..n {
            let grid = term.grid();
            for r in 0..60 {
                let (a, b) = logical_line(grid, Line(r));
                pills.line(grid, a, b, &mut out);
            }
        }
        eprintln!("pills, a full 200x60 screen, {name}: {:.1} us", t0.elapsed().as_secs_f64() * 1e6 / n as f64);
    }
}
