//! `proptest` property suite for terminal resize / reflow.
//!
//! Resize is the gnarliest grid operation: it re-wraps soft-wrapped logical
//! lines on the primary screen, clamps the cursor, and adjusts the scroll
//! region. A bad resize is the source of the classic "pane goes blank on split"
//! and "cursor escapes the grid after SIGWINCH" bugs. These properties pin the
//! load-bearing invariants across arbitrary content + arbitrary resize sequences.

use c0pl4nd_core::term::Terminal;
use proptest::collection::vec as prop_vec;
use proptest::prelude::*;

proptest! {
    /// A single resize to arbitrary dims (each clamped to ≥1 by the engine) must
    /// not panic, must yield EXACTLY the requested dims, and must leave the
    /// cursor addressable.
    #[test]
    fn resize_yields_requested_dims_and_keeps_cursor_in_bounds(
        content in prop_vec(any::<u8>(), 0..1024),
        rows in 1usize..200,
        cols in 1usize..400,
    ) {
        let mut t = Terminal::new(24, 80);
        t.advance(&content);
        t.resize(rows, cols);
        prop_assert_eq!(t.grid().rows(), rows);
        prop_assert_eq!(t.grid().cols(), cols);
        if let Some((r, c)) = t.cursor_position() {
            prop_assert!(r < rows, "cursor row {} >= rows {}", r, rows);
            prop_assert!(c <= cols, "cursor col {} > cols {}", c, cols);
        }
    }

    /// Zero / sub-minimum dims are CLAMPED to 1, never panic, never produce a
    /// zero-area grid (a 0×N grid breaks every downstream index).
    #[test]
    fn resize_clamps_to_min_one(rows in 0usize..3, cols in 0usize..3) {
        let mut t = Terminal::new(24, 80);
        t.resize(rows, cols);
        prop_assert_eq!(t.grid().rows(), rows.max(1));
        prop_assert_eq!(t.grid().cols(), cols.max(1));
    }

    /// A SEQUENCE of arbitrary resizes (the real churn a window-drag produces)
    /// never panics, and after each step the grid matches the last requested
    /// dims and the cursor stays in bounds. display_rows() length == rows and
    /// each row width == cols at every step.
    #[test]
    fn resize_sequence_is_stable(
        content in prop_vec(any::<u8>(), 0..512),
        steps in prop_vec((1usize..120, 1usize..240), 1..12),
    ) {
        let mut t = Terminal::new(24, 80);
        t.advance(&content);
        for (rows, cols) in steps {
            t.resize(rows, cols);
            prop_assert_eq!(t.grid().rows(), rows);
            prop_assert_eq!(t.grid().cols(), cols);
            let display = t.display_rows();
            prop_assert_eq!(display.len(), rows);
            for row in &display {
                prop_assert_eq!(row.len(), cols);
            }
            if let Some((r, c)) = t.cursor_position() {
                prop_assert!(r < rows);
                prop_assert!(c <= cols);
            }
        }
    }

    /// Resizing back to the ORIGINAL width after a detour preserves the visible
    /// text of content that fit on a single line (no soft-wrap to re-wrap), i.e.
    /// reflow is lossless for the simple case. Uses printable ASCII shorter than
    /// the narrowest width visited so the line never wraps.
    #[test]
    fn reflow_round_trip_preserves_short_line(
        text in "[a-zA-Z0-9]{1,20}",
        detour_cols in 30usize..80,
    ) {
        let mut t = Terminal::new(24, 80);
        t.advance(text.as_bytes());
        let before: String = t.grid().row(0).iter().take(text.len()).map(|c| c.c).collect();
        t.resize(24, detour_cols);
        t.resize(24, 80);
        let after: String = t.grid().row(0).iter().take(text.len()).map(|c| c.c).collect();
        prop_assert_eq!(before, after);
    }
}
