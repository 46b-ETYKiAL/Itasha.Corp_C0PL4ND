//! The terminal grid model — the data the renderer draws.
//!
//! A [`Grid`] is a rectangular array of [`Cell`]s plus a capped scrollback
//! ring. It is deliberately renderer-agnostic: the GPU layer reads a snapshot
//! each frame, and the VT parser ([`crate::term`]) is the only writer.

use serde::{Deserialize, Serialize};

/// An RGBA color. Defaults to "use the theme's default" via [`Color::Default`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Color {
    /// Inherit the theme default foreground/background.
    #[default]
    Default,
    /// One of the 16 ANSI indices (0-15).
    Indexed(u8),
    /// A direct 24-bit color.
    Rgb(u8, u8, u8),
}

/// Per-cell rendition attributes. Dependency-free flag set (serde-friendly).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct CellFlags {
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub inverse: bool,
    pub strikeout: bool,
}

impl CellFlags {
    /// No attributes set.
    pub const fn empty() -> Self {
        CellFlags {
            bold: false,
            italic: false,
            underline: false,
            inverse: false,
            strikeout: false,
        }
    }
}

/// A single grid cell.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cell {
    pub c: char,
    pub fg: Color,
    pub bg: Color,
    pub flags: CellFlags,
}

impl Default for Cell {
    fn default() -> Self {
        Cell {
            c: ' ',
            fg: Color::Default,
            bg: Color::Default,
            flags: CellFlags::empty(),
        }
    }
}

/// The terminal grid: a `rows x cols` matrix of cells.
#[derive(Debug, Clone)]
pub struct Grid {
    rows: usize,
    cols: usize,
    cells: Vec<Cell>,
    /// True when content changed since the last `clear_damage` — drives
    /// render-on-input so an idle terminal issues zero redraws.
    damaged: bool,
}

impl Grid {
    pub fn new(rows: usize, cols: usize) -> Self {
        let rows = rows.max(1);
        let cols = cols.max(1);
        Grid {
            rows,
            cols,
            cells: vec![Cell::default(); rows * cols],
            damaged: true,
        }
    }

    pub fn rows(&self) -> usize {
        self.rows
    }

    pub fn cols(&self) -> usize {
        self.cols
    }

    pub fn is_damaged(&self) -> bool {
        self.damaged
    }

    pub fn clear_damage(&mut self) {
        self.damaged = false;
    }

    /// Force the next frame to redraw (e.g. after a scroll-view change).
    pub fn touch(&mut self) {
        self.damaged = true;
    }

    fn idx(&self, row: usize, col: usize) -> usize {
        row * self.cols + col
    }

    pub fn cell(&self, row: usize, col: usize) -> Option<&Cell> {
        if row < self.rows && col < self.cols {
            Some(&self.cells[self.idx(row, col)])
        } else {
            None
        }
    }

    pub fn set(&mut self, row: usize, col: usize, cell: Cell) {
        if row < self.rows && col < self.cols {
            let i = self.idx(row, col);
            if self.cells[i] != cell {
                self.cells[i] = cell;
                self.damaged = true;
            }
        }
    }

    /// Clear the whole grid to blank cells.
    pub fn clear(&mut self) {
        for c in &mut self.cells {
            *c = Cell::default();
        }
        self.damaged = true;
    }

    /// Resize, preserving top-left content. Marks the grid damaged.
    pub fn resize(&mut self, rows: usize, cols: usize) {
        let rows = rows.max(1);
        let cols = cols.max(1);
        let mut next = vec![Cell::default(); rows * cols];
        for r in 0..rows.min(self.rows) {
            for c in 0..cols.min(self.cols) {
                next[r * cols + c] = self.cells[self.idx(r, c)].clone();
            }
        }
        self.rows = rows;
        self.cols = cols;
        self.cells = next;
        self.damaged = true;
    }

    /// Scroll the whole grid up by one line (top line lost, bottom blank).
    pub fn scroll_up(&mut self) {
        let _ = self.scroll_up_returning();
    }

    /// Scroll up by one line, returning the dropped top row (for scrollback).
    pub fn scroll_up_returning(&mut self) -> Vec<Cell> {
        let dropped: Vec<Cell> = self.cells.drain(0..self.cols).collect();
        self.cells
            .extend(std::iter::repeat_n(Cell::default(), self.cols));
        self.damaged = true;
        dropped
    }

    /// Scroll lines `[top, bottom]` (inclusive, 0-based) up by `n`, returning the
    /// `n` rows dropped off `top` (oldest first) so the caller can route them to
    /// scrollback. Rows below the bottom margin are untouched. New blank rows
    /// appear at the bottom of the region. `top`/`bottom` are clamped to the
    /// grid; `n` is clamped to the region height.
    pub fn scroll_region_up(&mut self, top: usize, bottom: usize, n: usize) -> Vec<Vec<Cell>> {
        let bottom = bottom.min(self.rows.saturating_sub(1));
        if top > bottom {
            return Vec::new();
        }
        let height = bottom - top + 1;
        let n = n.min(height);
        let mut dropped: Vec<Vec<Cell>> = Vec::with_capacity(n);
        for r in top..top + n {
            dropped.push(self.row(r).to_vec());
        }
        // Shift surviving rows up by n within the region.
        for r in top..=bottom {
            let src = r + n;
            for c in 0..self.cols {
                let dst = self.idx(r, c);
                let cell = if src <= bottom {
                    self.cells[self.idx(src, c)].clone()
                } else {
                    Cell::default()
                };
                self.cells[dst] = cell;
            }
        }
        self.damaged = true;
        dropped
    }

    /// Scroll lines `[top, bottom]` (inclusive, 0-based) down by `n`. New blank
    /// rows appear at `top`; rows scrolled past `bottom` are discarded. Used by
    /// reverse-index and DL/IL.
    pub fn scroll_region_down(&mut self, top: usize, bottom: usize, n: usize) {
        let bottom = bottom.min(self.rows.saturating_sub(1));
        if top > bottom {
            return;
        }
        let height = bottom - top + 1;
        let n = n.min(height);
        // Shift surviving rows down by n within the region (iterate top→bottom
        // from the bottom so we don't overwrite sources before reading them).
        for r in (top..=bottom).rev() {
            for c in 0..self.cols {
                let dst = self.idx(r, c);
                let cell = if r >= top + n {
                    self.cells[self.idx(r - n, c)].clone()
                } else {
                    Cell::default()
                };
                self.cells[dst] = cell;
            }
        }
        self.damaged = true;
    }

    /// Insert `count` blank cells at `(row, col)`, shifting the rest of the line
    /// right (ICH). Cells pushed past the right edge are lost.
    pub fn insert_blanks(&mut self, row: usize, col: usize, count: usize) {
        if row >= self.rows || col >= self.cols {
            return;
        }
        let count = count.min(self.cols - col);
        // Shift right: walk from the right edge inward.
        for c in (col..self.cols).rev() {
            let dst = self.idx(row, c);
            let cell = if c >= col + count {
                self.cells[self.idx(row, c - count)].clone()
            } else {
                Cell::default()
            };
            self.cells[dst] = cell;
        }
        self.damaged = true;
    }

    /// Delete `count` cells at `(row, col)`, shifting the rest of the line left
    /// (DCH). Blank cells fill in at the right edge.
    pub fn delete_chars(&mut self, row: usize, col: usize, count: usize) {
        if row >= self.rows || col >= self.cols {
            return;
        }
        let count = count.min(self.cols - col);
        for c in col..self.cols {
            let dst = self.idx(row, c);
            let cell = if c + count < self.cols {
                self.cells[self.idx(row, c + count)].clone()
            } else {
                Cell::default()
            };
            self.cells[dst] = cell;
        }
        self.damaged = true;
    }

    /// Erase `count` cells at `(row, col)` to blank without shifting (ECH).
    pub fn erase_chars(&mut self, row: usize, col: usize, count: usize) {
        if row >= self.rows || col >= self.cols {
            return;
        }
        let end = (col + count).min(self.cols);
        for c in col..end {
            let dst = self.idx(row, c);
            self.cells[dst] = Cell::default();
        }
        self.damaged = true;
    }

    /// Borrow one row's cells as a slice.
    pub fn row(&self, r: usize) -> &[Cell] {
        let start = r * self.cols;
        &self.cells[start..start + self.cols]
    }

    /// Render the grid to a plain `String` (one line per row) — used by tests
    /// and the headless smoke test before the GPU renderer is wired.
    pub fn to_text(&self) -> String {
        let mut out = String::with_capacity(self.rows * (self.cols + 1));
        for r in 0..self.rows {
            for c in 0..self.cols {
                out.push(self.cells[self.idx(r, c)].c);
            }
            out.push('\n');
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_grid_is_blank_and_damaged() {
        let g = Grid::new(3, 4);
        assert_eq!(g.rows(), 3);
        assert_eq!(g.cols(), 4);
        assert!(g.is_damaged());
        assert_eq!(g.cell(0, 0).unwrap().c, ' ');
    }

    #[test]
    fn set_marks_damage_only_on_change() {
        let mut g = Grid::new(2, 2);
        g.clear_damage();
        assert!(!g.is_damaged());
        g.set(
            0,
            0,
            Cell {
                c: 'x',
                ..Default::default()
            },
        );
        assert!(g.is_damaged());
        g.clear_damage();
        g.set(
            0,
            0,
            Cell {
                c: 'x',
                ..Default::default()
            },
        );
        assert!(
            !g.is_damaged(),
            "rewriting the same cell must not re-damage"
        );
    }

    #[test]
    fn scroll_up_drops_top_line() {
        let mut g = Grid::new(2, 1);
        g.set(
            0,
            0,
            Cell {
                c: 'a',
                ..Default::default()
            },
        );
        g.set(
            1,
            0,
            Cell {
                c: 'b',
                ..Default::default()
            },
        );
        g.scroll_up();
        assert_eq!(g.cell(0, 0).unwrap().c, 'b');
        assert_eq!(g.cell(1, 0).unwrap().c, ' ');
    }

    #[test]
    fn resize_preserves_top_left() {
        let mut g = Grid::new(2, 2);
        g.set(
            0,
            0,
            Cell {
                c: 'q',
                ..Default::default()
            },
        );
        g.resize(4, 4);
        assert_eq!(g.cell(0, 0).unwrap().c, 'q');
        assert_eq!(g.rows(), 4);
    }

    fn put(g: &mut Grid, r: usize, c: usize, ch: char) {
        g.set(
            r,
            c,
            Cell {
                c: ch,
                ..Default::default()
            },
        );
    }

    #[test]
    fn insert_blanks_shifts_right() {
        let mut g = Grid::new(1, 5);
        for (i, ch) in "abcde".chars().enumerate() {
            put(&mut g, 0, i, ch);
        }
        g.insert_blanks(0, 1, 2); // a__bc (de pushed off)
        assert_eq!(g.cell(0, 0).unwrap().c, 'a');
        assert_eq!(g.cell(0, 1).unwrap().c, ' ');
        assert_eq!(g.cell(0, 2).unwrap().c, ' ');
        assert_eq!(g.cell(0, 3).unwrap().c, 'b');
        assert_eq!(g.cell(0, 4).unwrap().c, 'c');
    }

    #[test]
    fn delete_chars_shifts_left() {
        let mut g = Grid::new(1, 5);
        for (i, ch) in "abcde".chars().enumerate() {
            put(&mut g, 0, i, ch);
        }
        g.delete_chars(0, 1, 2); // ade__ (b,c removed)
        assert_eq!(g.cell(0, 0).unwrap().c, 'a');
        assert_eq!(g.cell(0, 1).unwrap().c, 'd');
        assert_eq!(g.cell(0, 2).unwrap().c, 'e');
        assert_eq!(g.cell(0, 3).unwrap().c, ' ');
        assert_eq!(g.cell(0, 4).unwrap().c, ' ');
    }

    #[test]
    fn erase_chars_blanks_without_shift() {
        let mut g = Grid::new(1, 5);
        for (i, ch) in "abcde".chars().enumerate() {
            put(&mut g, 0, i, ch);
        }
        g.erase_chars(0, 1, 2); // a__de
        assert_eq!(g.cell(0, 0).unwrap().c, 'a');
        assert_eq!(g.cell(0, 1).unwrap().c, ' ');
        assert_eq!(g.cell(0, 2).unwrap().c, ' ');
        assert_eq!(g.cell(0, 3).unwrap().c, 'd');
        assert_eq!(g.cell(0, 4).unwrap().c, 'e');
    }

    #[test]
    fn scroll_region_up_within_margins() {
        let mut g = Grid::new(4, 1);
        for (r, ch) in "abcd".chars().enumerate() {
            put(&mut g, r, 0, ch);
        }
        // Scroll rows 1..=2 up by 1: row0 'a' fixed, row3 'd' fixed.
        let dropped = g.scroll_region_up(1, 2, 1);
        assert_eq!(dropped.len(), 1);
        assert_eq!(dropped[0][0].c, 'b');
        assert_eq!(g.cell(0, 0).unwrap().c, 'a');
        assert_eq!(g.cell(1, 0).unwrap().c, 'c');
        assert_eq!(g.cell(2, 0).unwrap().c, ' ');
        assert_eq!(g.cell(3, 0).unwrap().c, 'd');
    }

    #[test]
    fn scroll_region_down_within_margins() {
        let mut g = Grid::new(4, 1);
        for (r, ch) in "abcd".chars().enumerate() {
            put(&mut g, r, 0, ch);
        }
        // Scroll rows 1..=2 down by 1: blank at row1, c shifts to row2.
        g.scroll_region_down(1, 2, 1);
        assert_eq!(g.cell(0, 0).unwrap().c, 'a');
        assert_eq!(g.cell(1, 0).unwrap().c, ' ');
        assert_eq!(g.cell(2, 0).unwrap().c, 'b');
        assert_eq!(g.cell(3, 0).unwrap().c, 'd');
    }
}
