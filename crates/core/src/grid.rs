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
}
