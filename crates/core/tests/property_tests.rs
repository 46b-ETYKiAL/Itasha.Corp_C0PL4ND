//! Property-based tests for the VT parser / terminal state machine (proptest).
//!
//! The example-based unit tests in `term/tests.rs` pin specific escape sequences
//! to specific grid outcomes. These tests cover the COMPLEMENTARY space: for
//! ARBITRARY input the parser must uphold its invariants — never panic, never
//! corrupt the grid geometry, never let the cursor escape the grid, and (the
//! load-bearing one for a STREAMING parser) produce the same result whether a
//! byte stream arrives whole or split across read-buffer boundaries.
//!
//! This is the missing "property-based" test TYPE (best-in-class research:
//! property tests catch the unknown edge cases example tests miss, and the
//! parser is exactly the kind of input-driven state machine they were made for).

use c0pl4nd_core::term::Terminal;
use proptest::prelude::*;

/// Default test grid. Small enough that wrap/scroll behaviour is exercised by
/// short inputs, large enough to hold a line of text.
const ROWS: usize = 24;
const COLS: usize = 80;

fn fresh() -> Terminal {
    Terminal::new(ROWS, COLS)
}

proptest! {
    // The parser must be TOTAL: no sequence of arbitrary bytes — valid escape
    // codes, truncated ones, garbage, control bytes — may panic, and the grid
    // geometry it was created with is an invariant of `advance`.
    #[test]
    fn advance_arbitrary_bytes_never_panics_and_preserves_dims(
        bytes in proptest::collection::vec(any::<u8>(), 0..4096)
    ) {
        let mut t = fresh();
        t.advance(&bytes);
        prop_assert_eq!(t.grid().rows(), ROWS);
        prop_assert_eq!(t.grid().cols(), COLS);
    }

    // Whatever the input does, a reported cursor position is ALWAYS addressable
    // in the current grid — an out-of-bounds cursor is the classic
    // CUP/scroll-region off-by-one that corrupts every subsequent write.
    #[test]
    fn cursor_stays_in_bounds_under_arbitrary_input(
        bytes in proptest::collection::vec(any::<u8>(), 0..4096)
    ) {
        let mut t = fresh();
        t.advance(&bytes);
        if let Some((r, c)) = t.cursor_position() {
            prop_assert!(r < t.grid().rows(), "cursor row {r} >= {}", t.grid().rows());
            prop_assert!(c <= t.grid().cols(), "cursor col {c} > {}", t.grid().cols());
        }
    }

    // Arbitrary VALID UTF-8 (multibyte, CJK, emoji, combining marks) must never
    // panic — width handling + grapheme accumulation is a known panic surface.
    #[test]
    fn arbitrary_utf8_never_panics(s in ".{0,2000}") {
        let mut t = fresh();
        t.advance(s.as_bytes());
        prop_assert_eq!(t.grid().rows(), ROWS);
    }

    // Resizing to any reasonable geometry must not panic, must take effect, and
    // must leave the cursor addressable in the NEW grid (reflow off-by-ones are
    // a classic resize crash).
    #[test]
    fn resize_to_arbitrary_dims_never_panics_and_updates_grid(
        pre in proptest::collection::vec(any::<u8>(), 0..512),
        rows in 1usize..200,
        cols in 1usize..400,
    ) {
        let mut t = fresh();
        t.advance(&pre);
        t.resize(rows, cols);
        prop_assert_eq!(t.grid().rows(), rows);
        prop_assert_eq!(t.grid().cols(), cols);
        if let Some((r, c)) = t.cursor_position() {
            prop_assert!(r < rows);
            prop_assert!(c <= cols);
        }
    }

    // A short run of printable ASCII (no control/ESC bytes) written to a fresh
    // terminal lands verbatim in row 0 — the most basic "characters reach the
    // grid" contract, proven for arbitrary content rather than one literal.
    #[test]
    fn printable_ascii_round_trips_into_row_0(
        s in proptest::string::string_regex("[ -~]{0,80}").unwrap()
    ) {
        let mut t = fresh();
        t.advance(s.as_bytes());
        let row0: String = t.grid().row(0).iter().take(s.len()).map(|cell| cell.c).collect();
        prop_assert_eq!(row0, s);
    }

    // THE load-bearing streaming property: the PTY reader delivers bytes in
    // arbitrary-sized chunks (read-buffer boundaries fall anywhere — mid-escape-
    // sequence OR mid-multibyte-UTF-8). For a WELL-FORMED stream (text, CJK,
    // emoji, well-formed CSI sequences) the final grid MUST be identical whether
    // the bytes arrive whole or split at any point — the parser must buffer the
    // partial sequence/codepoint across reads. (Deliberately scoped to VALID
    // input: for INVALID UTF-8, U+FFFD replacement emission is inherently
    // position-dependent and chunk-invariance does NOT — and need not — hold.)
    #[test]
    fn chunking_a_valid_vt_stream_is_invariant(
        s in proptest::collection::vec(
            prop_oneof![
                // A well-formed CSI sequence (cursor moves, SGR, erase, CSI-u).
                proptest::string::string_regex("\\x1b\\[[0-9;]{0,4}[A-DHJKmsu]").unwrap(),
                // Printable ASCII + newline + CJK + an emoji (multibyte widths).
                proptest::string::string_regex("[A-Za-z0-9 \\n\u{3042}\u{4e00}\u{1f600}]").unwrap(),
            ],
            0..150,
        ).prop_map(|v| v.concat()),
        split in 0usize..8192,
    ) {
        let bytes = s.as_bytes();
        let k = split.min(bytes.len());
        let mut whole = fresh();
        whole.advance(bytes);

        let mut chunked = fresh();
        chunked.advance(&bytes[..k]);
        chunked.advance(&bytes[k..]);

        prop_assert_eq!(
            whole.display_rows(),
            chunked.display_rows(),
            "grid differs when a valid VT stream is split at byte {}",
            k
        );
        prop_assert_eq!(whole.cursor_position(), chunked.cursor_position());
    }

    // After an SGR reset (`ESC [ 0 m`) any subsequently printed cell carries the
    // DEFAULT rendition — no attribute (bold/inverse/underline) leaks past a
    // reset, regardless of what attributes preceded it.
    #[test]
    fn sgr_reset_clears_attributes(
        pre_attrs in proptest::collection::vec(1u8..=9, 0..6)
    ) {
        let mut t = fresh();
        // Apply some arbitrary attributes, then reset, then print a marker.
        for a in &pre_attrs {
            t.advance(format!("\x1b[{a}m").as_bytes());
        }
        t.advance(b"\x1b[0mX");
        let cell = t.grid().cell(0, 0).expect("cell 0,0 exists");
        prop_assert_eq!(cell.c, 'X');
        prop_assert!(!cell.flags.bold, "bold leaked past reset");
        prop_assert!(!cell.flags.italic, "italic leaked past reset");
        prop_assert!(!cell.flags.inverse, "inverse leaked past reset");
        prop_assert!(!cell.flags.strikeout, "strikeout leaked past reset");
    }
}

/// Regression: a multibyte UTF-8 codepoint whose bytes straddle a read boundary
/// (the PTY reader's 64 KiB buffer can split any char) MUST be buffered across
/// `advance()` calls and render as the correct glyph — not two replacement
/// chars. This is the concrete, named guarantee behind the `chunking_*` property.
#[test]
fn utf8_multibyte_split_across_advance_is_buffered() {
    // 日 = E6 97 A5 (3 bytes). Split mid-sequence across two advance() calls.
    let mut t = Terminal::new(2, 8);
    t.advance(&[0xE6]); // first byte of 日
    t.advance(&[0x97, 0xA5]); // remaining bytes
    assert_eq!(
        t.grid().cell(0, 0).map(|c| c.c),
        Some('日'),
        "partial UTF-8 must be buffered across read boundaries, not dropped to U+FFFD"
    );
}
