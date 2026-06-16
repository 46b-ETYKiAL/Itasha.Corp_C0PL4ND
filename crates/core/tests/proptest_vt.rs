//! `proptest` property suite for the VT parser / terminal state machine.
//!
//! The hand-rolled SplitMix64 suite in `property_tests.rs` already covers the
//! same INVARIANTS, but it cannot SHRINK a counterexample to a minimal input
//! and it keeps no on-disk regression corpus. This suite ports the load-bearing
//! invariants to `proptest!` so that:
//!
//!   * a failure shrinks to the smallest byte stream that still violates the
//!     property (far easier to debug than a 4 KiB random blob), and
//!   * the failing seed is persisted to `crates/core/proptest-regressions/` and
//!     committed, so a fixed bug can never silently regress.
//!
//! The properties asserted here are the defining contracts of a streaming VT
//! engine:
//!   1. TOTALITY — no `Vec<u8>` (malformed UTF-8, control bytes, half-finished
//!      escape sequences included) may panic, and the grid geometry is invariant.
//!   2. CURSOR CONTAINMENT — a reported cursor is always addressable.
//!   3. CHUNK INVARIANCE — for a WELL-FORMED stream, splitting the byte vector at
//!      an arbitrary boundary yields the SAME final grid. This is the single most
//!      valuable VT invariant: the PTY reader delivers bytes in arbitrary chunks,
//!      and a boundary can fall mid-escape-sequence or mid-UTF-8-codepoint.
//!   4. WIDE-GLYPH WIDTH — a width-2 (CJK / emoji) glyph occupies its anchor cell
//!      and marks the next cell as a continuation spacer.

use c0pl4nd_core::term::Terminal;
use proptest::collection::vec as prop_vec;
use proptest::prelude::*;

const ROWS: usize = 24;
const COLS: usize = 80;

fn fresh() -> Terminal {
    Terminal::new(ROWS, COLS)
}

/// A strategy producing a WELL-FORMED, valid-UTF-8 VT stream: a mix of
/// well-formed CSI sequences, printable + multibyte text, and newlines. Chunk
/// invariance is scoped to valid input — for INVALID UTF-8 the U+FFFD emission
/// is inherently position-dependent, so chunk-invariance neither holds nor needs
/// to (that case is covered by the totality property instead).
fn vt_stream() -> impl Strategy<Value = String> {
    // Each token is one of: a CSI sequence, a newline, or a printable/multibyte
    // char drawn from the ranges a real terminal sees (ASCII, Latin-1, CJK,
    // Hiragana, emoji — exercising 1- and 2-column widths).
    let csi = (
        prop_vec(0u16..40, 0..3),
        prop::sample::select(b"ABCDHJKmsu".to_vec()),
    )
        .prop_map(|(params, final_byte)| {
            let mut s = String::from("\u{1b}[");
            for (i, p) in params.iter().enumerate() {
                if i > 0 {
                    s.push(';');
                }
                s.push_str(&p.to_string());
            }
            s.push(final_byte as char);
            s
        });
    let glyph = prop_oneof![
        (0x20u32..0x7f).prop_map(|c| char::from_u32(c).unwrap().to_string()),
        (0xa0u32..0x100).prop_map(|c| char::from_u32(c).unwrap().to_string()),
        (0x4e00u32..0x5000).prop_map(|c| char::from_u32(c).unwrap().to_string()),
        (0x3040u32..0x30a0).prop_map(|c| char::from_u32(c).unwrap().to_string()),
        Just("\u{1f600}".to_string()),
        Just("\n".to_string()),
    ];
    prop_vec(prop_oneof![csi, glyph], 0..200).prop_map(|tokens| tokens.concat())
}

proptest! {
    /// TOTALITY: feeding an arbitrary byte vector — including malformed UTF-8,
    /// raw control bytes, and truncated escape sequences — must never panic, and
    /// the grid geometry must stay exactly ROWS×COLS.
    #[test]
    fn arbitrary_bytes_never_panic_and_preserve_dims(bytes in prop_vec(any::<u8>(), 0..4096)) {
        let mut t = fresh();
        t.advance(&bytes);
        prop_assert_eq!(t.grid().rows(), ROWS);
        prop_assert_eq!(t.grid().cols(), COLS);
        // Every visible row produced by display_rows() is exactly COLS wide.
        for row in t.display_rows() {
            prop_assert_eq!(row.len(), COLS);
        }
    }

    /// CURSOR CONTAINMENT: after arbitrary input the reported cursor is always
    /// addressable — out-of-bounds is the classic CUP / scroll-region off-by-one
    /// that corrupts every subsequent write.
    #[test]
    fn cursor_stays_in_bounds(bytes in prop_vec(any::<u8>(), 0..4096)) {
        let mut t = fresh();
        t.advance(&bytes);
        if let Some((r, c)) = t.cursor_position() {
            prop_assert!(r < t.grid().rows(), "cursor row {} out of bounds", r);
            prop_assert!(c <= t.grid().cols(), "cursor col {} out of bounds", c);
        }
    }

    /// CHUNK INVARIANCE (the load-bearing streaming property): for a well-formed
    /// stream the final grid + cursor must be identical whether the bytes arrive
    /// whole or split at any single boundary — the parser must buffer the partial
    /// sequence / codepoint across `advance()` calls.
    #[test]
    fn chunked_equals_whole(s in vt_stream(), split_frac in 0.0f64..=1.0) {
        let bytes = s.as_bytes();
        prop_assume!(!bytes.is_empty());
        let k = ((bytes.len() as f64) * split_frac) as usize;
        let k = k.min(bytes.len());

        let mut whole = fresh();
        whole.advance(bytes);

        let mut chunked = fresh();
        chunked.advance(&bytes[..k]);
        chunked.advance(&bytes[k..]);

        prop_assert_eq!(
            whole.display_rows(),
            chunked.display_rows(),
            "grid differs when split at byte {}",
            k
        );
        prop_assert_eq!(whole.cursor_position(), chunked.cursor_position());
    }

    /// CHUNK INVARIANCE, generalised: splitting the stream into MANY arbitrary
    /// chunks (not just one boundary) is still invariant vs feeding it whole.
    #[test]
    fn many_chunk_boundaries_equal_whole(
        s in vt_stream(),
        boundaries in prop_vec(0usize..1000, 0..8),
    ) {
        let bytes = s.as_bytes();
        prop_assume!(!bytes.is_empty());

        let mut cuts: Vec<usize> = boundaries.into_iter().map(|b| b % bytes.len()).collect();
        cuts.push(0);
        cuts.push(bytes.len());
        cuts.sort_unstable();
        cuts.dedup();

        let mut whole = fresh();
        whole.advance(bytes);

        let mut chunked = fresh();
        for w in cuts.windows(2) {
            chunked.advance(&bytes[w[0]..w[1]]);
        }

        prop_assert_eq!(whole.display_rows(), chunked.display_rows());
        prop_assert_eq!(whole.cursor_position(), chunked.cursor_position());
    }

    /// WIDE-GLYPH WIDTH: a single width-2 glyph written to a fresh terminal
    /// occupies the anchor cell with the glyph itself and marks the FOLLOWING
    /// cell as a wide-glyph continuation spacer (so it advances the cursor by 2).
    #[test]
    fn wide_glyph_marks_continuation_cell(cp in 0x4e00u32..0x9fff) {
        let ch = char::from_u32(cp).unwrap();
        let mut t = Terminal::new(4, 10);
        let mut buf = [0u8; 4];
        t.advance(ch.encode_utf8(&mut buf).as_bytes());
        // The anchor cell holds the glyph...
        prop_assert_eq!(t.grid().cell(0, 0).map(|c| c.c), Some(ch));
        // ...and the next cell is its continuation spacer.
        prop_assert!(
            t.grid().is_continuation(0, 1),
            "cell after a wide glyph must be a continuation spacer"
        );
        // The cursor advanced past both cells.
        if let Some((r, c)) = t.cursor_position() {
            prop_assert_eq!(r, 0);
            prop_assert_eq!(c, 2);
        }
    }

    /// SGR RESET: after `ESC [ 0 m` any subsequently printed cell carries the
    /// default rendition — no attribute leaks past a reset.
    #[test]
    fn sgr_reset_clears_attributes(attrs in prop_vec(1u8..=9, 0..6)) {
        let mut t = fresh();
        for a in attrs {
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
