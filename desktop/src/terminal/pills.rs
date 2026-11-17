//! Keyword pills in the grid: a decoration over the cells that hold a
//! `[[...]]` marker ([`crate::emphasis`]). Only drawing changes: the
//! terminal's cells, selection, copy, search, scrollback and what goes to the
//! program never see it.
//!
//! * Matching runs on a **logical line** (rows joined across soft wraps), so
//!   a marker the wrap cut is drawn as one piece per row, square at the cut.
//! * Only lines with a damaged row are matched again (`TerminalState::rebuild`
//!   keeps each row's pieces), and a line with no `[[` costs one pass over its
//!   cells and no allocation ([`Pills::line`]).
//! * The pieces are drawn by `render::build_row`: the brackets' cells become
//!   the pill's padding, the word is in the tone's strong colour (bold), the
//!   detail in the pill's text colour, all over a rounded span background
//!   (Rune's own rounded rect, behind the glyphs). The pill covers exactly
//!   the marker's cells.

use crate::emphasis::{self, Mark, Pill, Tone, Vocab};
use alacritty_terminal::grid::{Dimensions, Grid};
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::cell::{Cell, Flags};
use std::rc::Rc;

/// One pill's part on one row, in that row's columns.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Piece {
    /// Columns the piece covers, `start..end`.
    pub start: u16,
    pub end: u16,
    /// The word's columns and the detail's (`:` and after), clipped to the
    /// row; the rest of `start..end` is padding (the brackets' cells).
    pub word: (u16, u16),
    pub detail: (u16, u16),
    pub tone: Tone,
    /// The pill continues on the row above (square left end) or below
    /// (square right end).
    pub cut_left: bool,
    pub cut_right: bool,
}

impl Piece {
    pub fn covers(&self, c: usize) -> bool {
        (self.start as usize..self.end as usize).contains(&c)
    }
}

/// What a pill cell is drawn as.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    Pad,
    Word,
    Detail,
}

impl Piece {
    pub fn part(&self, c: usize) -> Part {
        let c = c as u16;
        if (self.word.0..self.word.1).contains(&c) {
            Part::Word
        } else if (self.detail.0..self.detail.1).contains(&c) {
            Part::Detail
        } else {
            Part::Pad
        }
    }
}

/// The terminal's pills: the vocabulary when they are on, the colours for
/// its theme, and the buffers matching reuses.
#[derive(Default)]
pub struct Pills {
    pub vocab: Option<Rc<Vocab>>,
    /// Per tone ([`emphasis::TONES`] order), for the terminal's background.
    pub palette: [Pill; 7],
    chars: Vec<char>,
    /// For each char of `chars`: (row of the logical line, column, width).
    cells: Vec<(u16, u16, u8)>,
    marks: Vec<Mark>,
    /// Logical lines looked at (the `[[` pre-check included) and matched
    /// (past the pre-check), over the run.
    pub scanned: usize,
    pub matched: usize,
}

/// Whether row `line`'s text continues on the next row (soft wrap).
pub fn wraps(grid: &Grid<Cell>, line: Line) -> bool {
    let cols = grid.columns();
    cols > 0 && grid[line][Column(cols - 1)].flags.contains(Flags::WRAPLINE)
}

/// The rows of the logical line holding `line`: first and last.
pub fn logical_line(grid: &Grid<Cell>, line: Line) -> (Line, Line) {
    let top = Line(-(grid.history_size() as i32));
    let bottom = Line(grid.screen_lines() as i32 - 1);
    let mut a = line;
    while a > top && wraps(grid, a - 1) {
        a -= 1;
    }
    let mut b = line;
    while b < bottom && wraps(grid, b) {
        b += 1;
    }
    (a, b)
}

impl Pills {
    pub fn set_palette(&mut self, background: u32, foreground: u32) {
        self.palette = emphasis::palettes(background, foreground);
    }

    pub fn colors(&self, t: Tone) -> Pill {
        self.palette[emphasis::index(t)]
    }

    /// The pieces of the logical line `a..=b`, appended per row to `out`
    /// (one `Vec` per row, `out[k]` for row `a + k`; a row with none gets an
    /// empty `Vec`, which allocates nothing).
    pub fn line(&mut self, grid: &Grid<Cell>, a: Line, b: Line, out: &mut Vec<Vec<Piece>>) {
        let nrows = (b.0 - a.0 + 1) as usize;
        out.clear();
        out.resize_with(nrows, Vec::new);
        self.scanned += 1;
        let Some(vocab) = self.vocab.clone() else { return };
        let cols = grid.columns();
        // The pre-check: `[[` anywhere on the line, across the wrap too.
        let mut prev = '\0';
        let mut any = false;
        'rows: for k in 0..nrows {
            let row = &grid[a + k as i32];
            for c in 0..cols {
                let ch = row[Column(c)].c;
                if ch == '[' && prev == '[' {
                    any = true;
                    break 'rows;
                }
                prev = ch;
            }
        }
        if !any {
            return;
        }
        self.matched += 1;
        self.chars.clear();
        self.cells.clear();
        for k in 0..nrows {
            let row = &grid[a + k as i32];
            for c in 0..cols {
                let cell = &row[Column(c)];
                if cell.flags.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER) {
                    continue;
                }
                let ch = if cell.c == '\0' || cell.c == '\t' { ' ' } else { cell.c };
                self.chars.push(ch);
                self.cells.push((k as u16, c as u16, if cell.flags.contains(Flags::WIDE_CHAR) { 2 } else { 1 }));
            }
        }
        self.marks.clear();
        emphasis::find(&self.chars, &vocab, &mut self.marks);
        for m in &self.marks {
            // Each row the marker touches gets a piece.
            let (r0, r1) = (self.cells[m.start].0, self.cells[m.end - 1].0);
            let span = |i: usize, j: usize, row: u16| -> (u16, u16) {
                // Columns of chars i..j that sit on `row`.
                let mut lo = u16::MAX;
                let mut hi = 0;
                for &(r, c, w) in &self.cells[i..j] {
                    if r == row {
                        lo = lo.min(c);
                        hi = hi.max(c + w as u16);
                    }
                }
                if lo == u16::MAX {
                    (0, 0)
                } else {
                    (lo, hi)
                }
            };
            for row in r0..=r1 {
                let (start, end) = span(m.start, m.end, row);
                out[row as usize].push(Piece {
                    start,
                    end,
                    word: span(m.start + 2, m.word_end, row),
                    detail: span(m.word_end, m.label_end, row),
                    tone: m.tone,
                    cut_left: row > r0,
                    cut_right: row < r1,
                });
            }
        }
    }
}
