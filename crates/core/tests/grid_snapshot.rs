//! Grid snapshot / reference tests.
//!
//! Best-in-class research (Alacritty's `ref.rs`): a "reference test" feeds a
//! known input to the terminal and compares the WHOLE resulting grid against a
//! known-good snapshot. Unlike the per-cell example tests in `term/tests.rs`
//! (which assert only the cells they name), a full-grid snapshot catches an
//! UNEXPECTED change ANYWHERE — a stray attribute, a shifted glyph, a wrong
//! background — in one assertion.
//!
//! The snapshot here is human-readable (a text layer + an attribute layer)
//! rather than opaque JSON, so a diff is reviewable. Inputs are chosen so the
//! expected output is hand-verifiable.

use c0pl4nd_core::term::Terminal;

/// Render a terminal's visible grid to a reviewable two-layer snapshot:
/// a TEXT layer (one line per row, trailing blanks trimmed) and an ATTR layer
/// (one char per cell: `.` plain, `r` reverse/inverse, `b` bold, `u` underline,
/// `*` bold+reverse). The two layers are separated by a `---` rule.
fn snapshot(t: &Terminal) -> String {
    let grid = t.grid();
    let mut text = String::new();
    let mut attrs = String::new();
    for r in 0..grid.rows() {
        let row = grid.row(r);
        let line: String = row.iter().map(|c| c.c).collect();
        text.push_str(line.trim_end());
        text.push('\n');
        for c in row {
            let f = &c.flags;
            let ch = match (f.bold, f.inverse, f.underline()) {
                (true, true, _) => '*',
                (_, true, _) => 'r',
                (true, _, _) => 'b',
                (_, _, true) => 'u',
                _ => '.',
            };
            attrs.push(ch);
        }
        attrs.push('\n');
    }
    format!("{text}---\n{attrs}")
}

#[test]
fn cursor_addressing_overwrite_and_attributes_snapshot() {
    // 4 rows × 12 cols. Script (all positions 1-based in CUP):
    //   clear + home, print "ABC" on row 1,
    //   CUP to row 3 col 5, print "XY",
    //   CUP home, print an inverse "Z" (overwriting the 'A').
    let mut t = Terminal::new(4, 12);
    t.advance(b"\x1b[2J\x1b[H");
    t.advance(b"ABC");
    t.advance(b"\x1b[3;5HXY");
    t.advance(b"\x1b[1;1H\x1b[7mZ\x1b[0m");

    // Hand-verifiable expected grid:
    //   row0: "ZBC"   (Z is reverse-video, B/C plain)
    //   row1: empty
    //   row2: "    XY"
    //   row3: empty
    let expected = "\
ZBC

    XY

---
r...........
............
............
............
";
    assert_eq!(snapshot(&t), expected);
}

#[test]
fn wrap_and_scroll_snapshot() {
    // 3 rows × 4 cols. Print 10 'A'..'J': fills row0 (ABCD), wraps to row1
    // (EFGH), wraps to row2 (IJ..). No scroll yet (exactly 3 rows used).
    let mut t = Terminal::new(3, 4);
    t.advance(b"ABCDEFGHIJ");
    let expected = "\
ABCD
EFGH
IJ
---
....
....
....
";
    assert_eq!(snapshot(&t), expected);
}

#[test]
fn bold_and_underline_attributes_snapshot() {
    // SGR 1 (bold), 4 (underline) carry onto printed cells; SGR 0 resets.
    let mut t = Terminal::new(1, 8);
    t.advance(b"\x1b[1mB\x1b[0m\x1b[4mU\x1b[0mp");
    let expected = "\
BUp
---
bu......
";
    assert_eq!(snapshot(&t), expected);
}
