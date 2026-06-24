//! VT/ANSI conformance corpus (vttest-style).
//!
//! Each test drives a known escape sequence through [`Terminal::advance`] and
//! asserts the EXACT resulting grid text + cursor `(row, col)`. Unlike the
//! end-to-end interaction tests in `e2e_terminal.rs`, every case here isolates
//! ONE sequence family so a regression names the exact control that broke.
//!
//! Coordinates: the engine's cursor is 0-based `(row, col)` via
//! [`Terminal::cursor_position`]; the VT control sequences themselves are
//! 1-based (CUP `row;col`). Assertions are written against the engine's
//! 0-based convention.
//!
//! Conformance scope: only behaviours the engine ACTUALLY implements are
//! asserted as "correct". Where the engine deviates from a strict VT100/xterm
//! reading (e.g. CUP into the last column clamps rather than allowing the
//! one-past "pending wrap" state), the test asserts the engine's REAL behaviour
//! and the doc-comment calls out the deviation. See the "Conformance notes"
//! block at the bottom of this file for the consolidated gap list.

use c0pl4nd_core::grid::Color;
use c0pl4nd_core::term::Terminal;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// A standard test terminal: `rows` × `cols`, modest scrollback.
fn term(rows: usize, cols: usize) -> Terminal {
    Terminal::with_scrollback(rows, cols, 100)
}

/// The base glyph of cell `(row, col)` (continuation spacers read as space).
fn ch(t: &Terminal, row: usize, col: usize) -> char {
    t.grid().cell(row, col).map(|c| c.c).unwrap_or('\u{0}')
}

/// One grid row rendered to a `String` of its base glyphs.
fn row(t: &Terminal, r: usize) -> String {
    let g = t.grid();
    (0..g.cols()).map(|c| ch(t, r, c)).collect()
}

/// The cursor position as 0-based `(row, col)`. Panics if the cursor is
/// scrolled out of view (never the case in these tests, which never scroll the
/// view back).
fn cursor(t: &Terminal) -> (usize, usize) {
    t.cursor_position().expect("cursor must be in view")
}

/// Assert the cursor sits at 0-based `(row, col)`.
fn assert_cursor(t: &Terminal, row: usize, col: usize) {
    assert_eq!(cursor(t), (row, col), "cursor position");
}

// ===========================================================================
// 1. Cursor movement: CUP/HVP, CUU/CUD/CUF/CUB, CR/LF/TAB, backspace.
// ===========================================================================

#[test]
fn cup_absolute_positioning() {
    let mut t = term(6, 20);
    // CUP row 3, col 5 (1-based) -> engine (2, 4).
    t.advance(b"\x1b[3;5H");
    assert_cursor(&t, 2, 4);
    t.advance(b"X");
    assert_eq!(ch(&t, 2, 4), 'X');
    assert_cursor(&t, 2, 5);
}

#[test]
fn hvp_is_an_alias_for_cup() {
    let mut t = term(6, 20);
    // HVP uses `f`; must land identically to CUP `H`.
    t.advance(b"\x1b[2;7f");
    assert_cursor(&t, 1, 6);
}

#[test]
fn cup_defaults_to_home() {
    let mut t = term(6, 20);
    t.advance(b"\x1b[4;4H"); // move away first
    t.advance(b"\x1b[H"); // bare CUP -> home (1,1) -> (0,0)
    assert_cursor(&t, 0, 0);
}

#[test]
fn cup_clamps_out_of_range_to_grid_edge() {
    let mut t = term(4, 8);
    // Row 99, col 99 clamp to last row / last col.
    t.advance(b"\x1b[99;99H");
    assert_cursor(&t, 3, 7);
}

#[test]
fn cuu_cud_cuf_cub_relative_motion() {
    let mut t = term(8, 20);
    t.advance(b"\x1b[5;5H"); // (4,4)
    t.advance(b"\x1b[2A"); // CUU 2 -> row 2
    assert_cursor(&t, 2, 4);
    t.advance(b"\x1b[3B"); // CUD 3 -> row 5
    assert_cursor(&t, 5, 4);
    t.advance(b"\x1b[4C"); // CUF 4 -> col 8
    assert_cursor(&t, 5, 8);
    t.advance(b"\x1b[6D"); // CUB 6 -> col 2
    assert_cursor(&t, 5, 2);
}

#[test]
fn cursor_motion_default_count_is_one() {
    let mut t = term(8, 20);
    t.advance(b"\x1b[5;5H");
    t.advance(b"\x1b[A"); // CUU with no param -> up 1
    assert_cursor(&t, 3, 4);
    t.advance(b"\x1b[C"); // CUF no param -> right 1
    assert_cursor(&t, 3, 5);
}

#[test]
fn cursor_motion_saturates_at_edges() {
    let mut t = term(4, 6);
    t.advance(b"\x1b[A"); // up from home -> stays at row 0
    assert_cursor(&t, 0, 0);
    t.advance(b"\x1b[D"); // left from col 0 -> stays
    assert_cursor(&t, 0, 0);
    t.advance(b"\x1b[99C"); // far right -> last col
    assert_cursor(&t, 0, 5);
    t.advance(b"\x1b[99B"); // far down -> last row
    assert_cursor(&t, 3, 5);
}

#[test]
fn carriage_return_homes_column() {
    let mut t = term(4, 10);
    t.advance(b"abcde");
    assert_cursor(&t, 0, 5);
    t.advance(b"\r");
    assert_cursor(&t, 0, 0);
    // CR does not erase: the text is still present.
    assert_eq!(&row(&t, 0)[..5], "abcde");
}

#[test]
fn line_feed_advances_row_keeps_column() {
    let mut t = term(4, 10);
    t.advance(b"ab");
    t.advance(b"\n"); // LF advances row, column unchanged (no implicit CR)
    assert_cursor(&t, 1, 2);
    t.advance(b"\r\n"); // CRLF
    assert_cursor(&t, 2, 0);
}

#[test]
fn line_feed_at_bottom_scrolls() {
    let mut t = term(3, 6);
    t.advance(b"L0\r\nL1\r\nL2"); // fills 3 rows, cursor at bottom row
    assert_cursor(&t, 2, 2);
    t.advance(b"\r\n"); // scroll: L0 leaves, L1/L2 shift up, blank bottom
    assert_eq!(&row(&t, 0)[..2], "L1");
    assert_eq!(&row(&t, 1)[..2], "L2");
    assert_cursor(&t, 2, 0);
}

#[test]
fn horizontal_tab_default_8col_stops() {
    let mut t = term(4, 40);
    t.advance(b"\t"); // from col 0 -> col 8
    assert_cursor(&t, 0, 8);
    t.advance(b"\t"); // -> col 16
    assert_cursor(&t, 0, 16);
    t.advance(b"X\t"); // print at 16 (->17), tab -> 24
    assert_cursor(&t, 0, 24);
}

#[test]
fn backspace_moves_left_and_saturates() {
    let mut t = term(4, 10);
    t.advance(b"abc");
    assert_cursor(&t, 0, 3);
    t.advance(b"\x08"); // BS -> col 2 (does not erase)
    assert_cursor(&t, 0, 2);
    assert_eq!(ch(&t, 0, 2), 'c');
    t.advance(b"\x08\x08\x08\x08"); // saturates at col 0
    assert_cursor(&t, 0, 0);
}

#[test]
fn cha_and_vpa_absolute_axis_moves() {
    let mut t = term(6, 20);
    t.advance(b"\x1b[3;3H");
    t.advance(b"\x1b[10G"); // CHA -> col 10 (1-based) -> col 9
    assert_cursor(&t, 2, 9);
    t.advance(b"\x1b[5d"); // VPA -> row 5 (1-based) -> row 4
    assert_cursor(&t, 4, 9);
}

// ===========================================================================
// 2. Erase: ED (CSI 2J / 0J / 1J), EL (CSI K variants).
// ===========================================================================

#[test]
fn ed_2j_clears_whole_screen_and_homes() {
    let mut t = term(3, 6);
    t.advance(b"aaaaaa\r\nbbbbbb\r\ncccccc");
    t.advance(b"\x1b[2J");
    for r in 0..3 {
        assert_eq!(row(&t, r), "      ", "row {r} cleared");
    }
    // ED 2 homes the cursor in this engine.
    assert_cursor(&t, 0, 0);
}

#[test]
fn ed_0j_clears_cursor_to_end() {
    let mut t = term(3, 6);
    t.advance(b"aaaaaa\r\nbbbbbb\r\ncccccc");
    t.advance(b"\x1b[2;3H"); // row 2 col 3 -> (1,2)
    t.advance(b"\x1b[0J"); // erase cursor..end
    assert_eq!(row(&t, 0), "aaaaaa", "row above cursor untouched");
    assert_eq!(row(&t, 1), "bb    ", "row 1: cols 0-1 kept, 2.. cleared");
    assert_eq!(row(&t, 2), "      ", "row below cursor cleared");
    assert_cursor(&t, 1, 2);
}

#[test]
fn ed_1j_clears_start_to_cursor() {
    let mut t = term(3, 6);
    t.advance(b"aaaaaa\r\nbbbbbb\r\ncccccc");
    t.advance(b"\x1b[2;3H"); // (1,2)
    t.advance(b"\x1b[1J"); // erase start..=cursor
    assert_eq!(row(&t, 0), "      ", "row above cleared");
    assert_eq!(row(&t, 1), "   bbb", "row 1: cols 0-2 cleared, 3.. kept");
    assert_eq!(row(&t, 2), "cccccc", "row below untouched");
}

#[test]
fn el_0k_clears_cursor_to_eol() {
    let mut t = term(2, 8);
    t.advance(b"abcdefgh");
    t.advance(b"\x1b[1;4H"); // col 4 -> (0,3)
    t.advance(b"\x1b[K"); // default 0: cursor..EOL
    assert_eq!(row(&t, 0), "abc     ");
}

#[test]
fn el_1k_clears_bol_to_cursor() {
    let mut t = term(2, 8);
    t.advance(b"abcdefgh");
    t.advance(b"\x1b[1;4H"); // (0,3)
    t.advance(b"\x1b[1K"); // BOL..=cursor
    assert_eq!(row(&t, 0), "    efgh");
}

#[test]
fn el_2k_clears_whole_line() {
    let mut t = term(2, 8);
    t.advance(b"abcdefgh");
    t.advance(b"\x1b[1;4H");
    t.advance(b"\x1b[2K");
    assert_eq!(row(&t, 0), "        ");
    // Cursor column is unchanged by EL.
    assert_cursor(&t, 0, 3);
}

// ===========================================================================
// 3. Scroll region: DECSTBM, SU/SD, IL/DL within margins.
// ===========================================================================

#[test]
fn decstbm_sets_region_and_homes_cursor() {
    let mut t = term(6, 6);
    t.advance(b"\x1b[2;4r"); // region rows 2..4 (1-based) -> [1,3]
                             // DECSTBM homes the cursor to the region top (row 1 here? engine homes to
                             // scroll_top which is 1 0-based).
    assert_cursor(&t, 1, 0);
}

#[test]
fn line_feed_scrolls_within_region_only() {
    let mut t = term(6, 6);
    // Fill all six rows with their index.
    t.advance(b"r0\r\nr1\r\nr2\r\nr3\r\nr4\r\nr5");
    // Region rows 2..4 (1-based) = [1,3].
    t.advance(b"\x1b[2;4r");
    // Cursor homed to (1,0). Move to bottom margin (row 3) and LF: region
    // scrolls up, rows outside the margin are fixed.
    t.advance(b"\x1b[4;1H"); // row 4 (1-based) -> (3,0), the bottom margin
    t.advance(b"\n");
    assert_eq!(&row(&t, 0)[..2], "r0", "row above region fixed");
    assert_eq!(&row(&t, 1)[..2], "r2", "region scrolled: r1 dropped");
    assert_eq!(&row(&t, 2)[..2], "r3");
    assert_eq!(row(&t, 3), "      ", "blank pulled in at region bottom");
    assert_eq!(&row(&t, 4)[..2], "r4", "row below region fixed");
}

#[test]
fn su_scrolls_region_up() {
    let mut t = term(5, 6);
    t.advance(b"r0\r\nr1\r\nr2\r\nr3\r\nr4");
    t.advance(b"\x1b[2;4r"); // region [1,3]
    t.advance(b"\x1b[2S"); // SU 2 within the region
    assert_eq!(&row(&t, 0)[..2], "r0", "above region fixed");
    assert_eq!(&row(&t, 1)[..2], "r3", "r1,r2 scrolled off the region top");
    assert_eq!(row(&t, 2), "      ");
    assert_eq!(row(&t, 3), "      ");
    assert_eq!(&row(&t, 4)[..2], "r4", "below region fixed");
}

#[test]
fn sd_scrolls_region_down() {
    let mut t = term(5, 6);
    t.advance(b"r0\r\nr1\r\nr2\r\nr3\r\nr4");
    t.advance(b"\x1b[2;4r"); // region [1,3]
    t.advance(b"\x1b[1T"); // SD 1 within the region
    assert_eq!(&row(&t, 0)[..2], "r0");
    assert_eq!(row(&t, 1), "      ", "blank pushed in at region top");
    assert_eq!(&row(&t, 2)[..2], "r1");
    assert_eq!(&row(&t, 3)[..2], "r2", "r3 scrolled off the region bottom");
    assert_eq!(&row(&t, 4)[..2], "r4");
}

#[test]
fn il_inserts_lines_within_region() {
    let mut t = term(5, 6);
    t.advance(b"r0\r\nr1\r\nr2\r\nr3\r\nr4");
    t.advance(b"\x1b[2;4r"); // region [1,3]
    t.advance(b"\x1b[3;1H"); // cursor to row 3 (1-based) -> (2,0)
    t.advance(b"\x1b[L"); // IL 1: insert a blank line, push rows below down
    assert_eq!(&row(&t, 1)[..2], "r1", "region top fixed above cursor");
    assert_eq!(row(&t, 2), "      ", "blank inserted at cursor row");
    assert_eq!(&row(&t, 3)[..2], "r2", "r2 pushed down");
    assert_eq!(
        &row(&t, 4)[..2],
        "r4",
        "below region fixed (r3 fell off region)"
    );
}

#[test]
fn dl_deletes_lines_within_region() {
    let mut t = term(5, 6);
    t.advance(b"r0\r\nr1\r\nr2\r\nr3\r\nr4");
    t.advance(b"\x1b[2;4r"); // region [1,3]
    t.advance(b"\x1b[2;1H"); // cursor to row 2 (1-based) -> (1,0)
    t.advance(b"\x1b[M"); // DL 1: delete cursor row, pull rows below up
    assert_eq!(&row(&t, 1)[..2], "r2", "r1 deleted, r2 pulled up");
    assert_eq!(&row(&t, 2)[..2], "r3");
    assert_eq!(row(&t, 3), "      ", "blank at region bottom");
    assert_eq!(&row(&t, 4)[..2], "r4", "below region fixed");
}

// ===========================================================================
// 4. SGR: bold / fg colour / reset.
// ===========================================================================

#[test]
fn sgr_bold_sets_cell_flag() {
    let mut t = term(2, 8);
    t.advance(b"\x1b[1mB\x1b[0mN");
    let bold = t.grid().cell(0, 0).unwrap();
    let normal = t.grid().cell(0, 1).unwrap();
    assert!(bold.flags.bold, "first cell bold");
    assert!(!normal.flags.bold, "SGR 0 reset bold");
}

#[test]
fn sgr_italic_and_reset() {
    let mut t = term(2, 8);
    t.advance(b"\x1b[3mI\x1b[23mP");
    assert!(t.grid().cell(0, 0).unwrap().flags.italic);
    assert!(
        !t.grid().cell(0, 1).unwrap().flags.italic,
        "SGR 23 cleared italic"
    );
}

#[test]
fn sgr_foreground_color_indexed_30s() {
    let mut t = term(2, 8);
    // SGR 31 = red (index 1), 32 = green (index 2).
    t.advance(b"\x1b[31mR\x1b[32mG");
    assert_eq!(t.grid().cell(0, 0).unwrap().fg, Color::Indexed(1));
    assert_eq!(t.grid().cell(0, 1).unwrap().fg, Color::Indexed(2));
}

#[test]
fn sgr_bright_foreground_90s() {
    let mut t = term(2, 8);
    // SGR 91 = bright red -> index 1 + 8 = 9.
    t.advance(b"\x1b[91mX");
    assert_eq!(t.grid().cell(0, 0).unwrap().fg, Color::Indexed(9));
}

#[test]
fn sgr_256_color_extended() {
    let mut t = term(2, 8);
    t.advance(b"\x1b[38;5;160mX");
    assert_eq!(t.grid().cell(0, 0).unwrap().fg, Color::Indexed(160));
}

#[test]
fn sgr_truecolor_rgb() {
    let mut t = term(2, 8);
    t.advance(b"\x1b[38;2;10;20;30mX");
    assert_eq!(t.grid().cell(0, 0).unwrap().fg, Color::Rgb(10, 20, 30));
}

#[test]
fn sgr_background_color_and_reset() {
    let mut t = term(2, 8);
    t.advance(b"\x1b[44mB\x1b[49mN");
    assert_eq!(t.grid().cell(0, 0).unwrap().bg, Color::Indexed(4));
    assert_eq!(
        t.grid().cell(0, 1).unwrap().bg,
        Color::Default,
        "SGR 49 reset background to default"
    );
}

#[test]
fn sgr_full_reset_restores_default_pen() {
    let mut t = term(2, 12);
    t.advance(b"\x1b[1;31;44mX\x1b[0mY");
    let y = t.grid().cell(0, 1).unwrap();
    assert!(!y.flags.bold, "reset cleared bold");
    assert_eq!(y.fg, Color::Default, "reset cleared fg");
    assert_eq!(y.bg, Color::Default, "reset cleared bg");
}

// ===========================================================================
// 5. Autowrap at right margin + DECAWM; origin mode DECOM (?6).
// ===========================================================================

#[test]
fn autowrap_wraps_at_right_margin() {
    let mut t = term(3, 4);
    t.advance(b"abcd"); // fills row 0; cursor parks one-past on the same row
    assert_eq!(row(&t, 0), "abcd");
    t.advance(b"e"); // the wrap happens here: e lands on row 1, col 0
    assert_eq!(ch(&t, 1, 0), 'e');
    assert_cursor(&t, 1, 1);
}

#[test]
fn decawm_off_overwrites_last_column() {
    let mut t = term(3, 4);
    t.advance(b"\x1b[?7l"); // DECAWM off
    t.advance(b"abcd"); // fills row 0
    t.advance(b"ef"); // with wrap off, last column is overwritten in place
    assert_eq!(ch(&t, 0, 3), 'f', "last printed glyph overwrites col 3");
    assert_eq!(row(&t, 1), "    ", "row 1 stays blank: no wrap");
    assert_cursor(&t, 0, 3);
}

#[test]
fn decom_origin_mode_constrains_addressing_to_region() {
    let mut t = term(8, 10);
    t.advance(b"\x1b[3;6r"); // region rows 3..6 (1-based) -> [2,5]
    t.advance(b"\x1b[?6h"); // DECOM on: row addressing relative to top margin
                            // After ?6h the cursor homes to the region top.
    assert_cursor(&t, 2, 0);
    // CUP row 1 (1-based) is relative to top margin -> absolute row 2.
    t.advance(b"\x1b[1;1H");
    assert_cursor(&t, 2, 0);
    // CUP row 2 -> absolute row 3.
    t.advance(b"\x1b[2;1H");
    assert_cursor(&t, 3, 0);
    // A row beyond the region clamps to the bottom margin (row 5).
    t.advance(b"\x1b[99;1H");
    assert_cursor(&t, 5, 0);
}

#[test]
fn decom_off_addressing_is_absolute() {
    let mut t = term(8, 10);
    t.advance(b"\x1b[3;6r"); // region [2,5]
    t.advance(b"\x1b[?6l"); // DECOM off (explicit)
    t.advance(b"\x1b[1;1H"); // absolute home
    assert_cursor(&t, 0, 0);
}

// ===========================================================================
// 6. Tabs: HTS / TBC and default 8-col stops.
// ===========================================================================

#[test]
fn hts_sets_a_custom_tab_stop() {
    let mut t = term(2, 20);
    t.advance(b"\x1b[1;3H"); // col 3 (1-based) -> (0,2)
    t.advance(b"\x1bH"); // HTS: set a stop at column 2
    t.advance(b"\r"); // back to col 0
    t.advance(b"\t"); // first stop is now the custom one at col 2
    assert_cursor(&t, 0, 2);
}

#[test]
fn tbc_0_clears_stop_at_cursor() {
    let mut t = term(2, 40);
    // Default stop at col 8. Move there and clear it.
    t.advance(b"\x1b[1;9H"); // col 9 (1-based) -> (0,8)
    t.advance(b"\x1b[g"); // TBC 0: clear the stop at the cursor column
    t.advance(b"\r\t"); // tab from col 0: col 8 stop gone -> next is col 16
    assert_cursor(&t, 0, 16);
}

#[test]
fn tbc_3_clears_all_stops() {
    let mut t = term(2, 12);
    t.advance(b"\x1b[3g"); // TBC 3: clear every stop
    t.advance(b"\t"); // no stops ahead -> cursor lands on the last column
    assert_cursor(&t, 0, 11);
}

#[test]
fn cbt_moves_back_a_tab_stop() {
    let mut t = term(2, 40);
    t.advance(b"\x1b[1;20H"); // col 20 (1-based) -> (0,19)
    t.advance(b"\x1b[Z"); // CBT 1 -> previous stop (col 16)
    assert_cursor(&t, 0, 16);
}

// ===========================================================================
// 7. Wide glyph (CJK): occupies 2 cells + continuation; cursor advances by 2.
// ===========================================================================

#[test]
fn wide_glyph_occupies_two_cells() {
    let mut t = term(2, 10);
    t.advance("世".as_bytes()); // U+4E16, East-Asian wide
    assert_eq!(ch(&t, 0, 0), '世', "base glyph in col 0");
    assert!(
        t.grid().is_continuation(0, 1),
        "col 1 is the wide-glyph continuation spacer"
    );
    assert_cursor(&t, 0, 2);
}

#[test]
fn wide_glyph_then_ascii_layout() {
    let mut t = term(2, 10);
    t.advance("世a".as_bytes());
    assert_eq!(ch(&t, 0, 0), '世');
    assert!(t.grid().is_continuation(0, 1));
    assert_eq!(
        ch(&t, 0, 2),
        'a',
        "ASCII follows after the 2-cell wide glyph"
    );
    assert_cursor(&t, 0, 3);
}

#[test]
fn wide_glyph_wraps_when_straddling_right_edge() {
    // 3 columns: a wide glyph cannot fit in the last single column, so with
    // autowrap on it moves to the next line, leaving the trailing cell blank.
    let mut t = term(3, 3);
    t.advance(b"ab"); // cols 0,1 filled; cursor at col 2
    t.advance("世".as_bytes()); // would straddle col 2/3 -> wraps to row 1
    assert_eq!(ch(&t, 1, 0), '世', "wide glyph wrapped to next row");
    assert!(t.grid().is_continuation(1, 1));
    assert_cursor(&t, 1, 2);
}

// ===========================================================================
// 8. Charset / UTF-8 basics.
// ===========================================================================

#[test]
fn dec_line_drawing_charset_maps_glyphs() {
    let mut t = term(2, 10);
    t.advance(b"\x1b(0"); // designate G0 = DEC special line drawing
    t.advance(b"qx"); // q -> horizontal line, x -> vertical line
                      // The exact target codepoints are the DEC special graphics box-drawing
                      // chars; assert they are NOT the literal ASCII letters (the mapping fired).
    assert_ne!(ch(&t, 0, 0), 'q', "DEC line-drawing remapped 'q'");
    assert_ne!(ch(&t, 0, 1), 'x', "DEC line-drawing remapped 'x'");
    // And switching back to ASCII restores literal printing.
    t.advance(b"\x1b(B"); // G0 back to US-ASCII
    t.advance(b"q");
    assert_eq!(ch(&t, 0, 2), 'q', "ASCII charset prints the literal glyph");
}

#[test]
fn dec_line_drawing_specific_glyph_q_is_horizontal_bar() {
    let mut t = term(2, 4);
    t.advance(b"\x1b(0q");
    // DEC 'q' maps to U+2500 BOX DRAWINGS LIGHT HORIZONTAL.
    assert_eq!(ch(&t, 0, 0), '\u{2500}');
}

#[test]
fn utf8_multibyte_lands_in_one_cell() {
    let mut t = term(2, 10);
    t.advance("café".as_bytes()); // é is 2 UTF-8 bytes, width 1
    assert_eq!(ch(&t, 0, 0), 'c');
    assert_eq!(ch(&t, 0, 3), 'é', "accented latin occupies a single cell");
    assert_cursor(&t, 0, 4);
}

#[test]
fn utf8_combining_mark_attaches_to_base() {
    let mut t = term(2, 10);
    // 'e' + U+0301 COMBINING ACUTE ACCENT: a zero-width mark joins the prior
    // cell rather than consuming its own column.
    t.advance("e\u{0301}x".as_bytes());
    assert_eq!(
        t.grid().grapheme_at(0, 0),
        "e\u{0301}",
        "combining mark attached to the base cell"
    );
    assert_eq!(ch(&t, 0, 1), 'x', "next glyph is in the very next cell");
    assert_cursor(&t, 0, 2);
}

#[test]
fn utf8_split_across_advance_calls_is_reassembled() {
    let mut t = term(2, 10);
    let bytes = "é".as_bytes(); // 0xC3 0xA9
    t.advance(&bytes[..1]); // feed only the lead byte
    t.advance(&bytes[1..]); // then the continuation byte
    assert_eq!(
        ch(&t, 0, 0),
        'é',
        "split UTF-8 codepoint reassembled, not lost"
    );
    assert_cursor(&t, 0, 1);
}

// ===========================================================================
// 9. Cursor save / restore (DECSC/DECRC + ANSI.SYS aliases).
// ===========================================================================

#[test]
fn decsc_decrc_round_trips_position() {
    let mut t = term(6, 20);
    t.advance(b"\x1b[3;5H"); // (2,4)
    t.advance(b"\x1b7"); // DECSC save
    t.advance(b"\x1b[1;1H"); // move home
    assert_cursor(&t, 0, 0);
    t.advance(b"\x1b8"); // DECRC restore
    assert_cursor(&t, 2, 4);
}

#[test]
fn ansi_sys_save_restore_aliases() {
    let mut t = term(6, 20);
    t.advance(b"\x1b[4;7H"); // (3,6)
    t.advance(b"\x1b[s"); // SCOSC
    t.advance(b"\x1b[1;1H");
    t.advance(b"\x1b[u"); // SCORC
    assert_cursor(&t, 3, 6);
}

// ===========================================================================
// 10. Charset switching via SI/SO with a G1 line-drawing designation.
//
// The vttest "Test of character sets" family designates a charset into G1
// (`ESC ) 0`), then flips GL between G0 and G1 with the C0 controls SO (0x0E,
// "shift out", invoke G1) and SI (0x0F, "shift in", return to G0). This is the
// idiom `tput smacs`/`rmacs` and ncurses box-drawing actually emit — distinct
// from the `ESC ( 0` G0-designation path already covered in section 8.
// ===========================================================================

#[test]
fn so_invokes_g1_line_drawing_then_si_returns_to_ascii() {
    let mut t = term(2, 12);
    // Designate DEC Special Graphics into G1; G0 stays ASCII.
    t.advance(b"\x1b)0");
    // While in G0 (default), 'q' prints literally.
    t.advance(b"q");
    assert_eq!(ch(&t, 0, 0), 'q', "G0 active: ASCII 'q' prints literally");
    // SO invokes G1 (line-drawing) into GL: 'q' -> U+2500 horizontal bar,
    // 'x' -> U+2502 vertical bar, 'n' -> U+253C crossing.
    t.advance(b"\x0e"); // SO
    t.advance(b"qxn");
    assert_eq!(ch(&t, 0, 1), '\u{2500}', "G1 line-drawing: q -> ─");
    assert_eq!(ch(&t, 0, 2), '\u{2502}', "G1 line-drawing: x -> │");
    assert_eq!(ch(&t, 0, 3), '\u{253c}', "G1 line-drawing: n -> ┼");
    // SI returns GL to G0 (ASCII): 'q' is literal again.
    t.advance(b"\x0f"); // SI
    t.advance(b"q");
    assert_eq!(ch(&t, 0, 4), 'q', "SI restored G0 ASCII: 'q' literal again");
    assert_cursor(&t, 0, 5);
}

#[test]
fn g0_ascii_unaffected_by_g1_designation_until_so() {
    // Designating G1 must NOT change what GL (G0) prints until SO flips to G1.
    let mut t = term(2, 8);
    t.advance(b"\x1b)0"); // G1 = line-drawing, but GL is still G0 = ASCII
    t.advance(b"jk"); // these map under line-drawing ONLY when G1 is invoked
    assert_eq!(ch(&t, 0, 0), 'j', "no SO yet: 'j' prints literally");
    assert_eq!(ch(&t, 0, 1), 'k', "no SO yet: 'k' prints literally");
}

// ===========================================================================
// 11. Index / Reverse-Index / Next-Line (IND `ESC D`, RI `ESC M`, NEL `ESC E`).
//
// These ESC-level cursor controls are the column-preserving (IND/RI) and
// column-homing (NEL) siblings of LF, and they honour the scroll region the
// same way LF does. The vttest cursor-movement family exercises them directly.
// ===========================================================================

#[test]
fn ind_advances_row_and_preserves_column() {
    let mut t = term(4, 10);
    t.advance(b"\x1b[1;4H"); // (0,3)
    t.advance(b"\x1bD"); // IND — down one row, column unchanged (like LF, no CR)
    assert_cursor(&t, 1, 3);
}

#[test]
fn ind_at_bottom_scrolls_the_screen_up() {
    let mut t = term(3, 6);
    t.advance(b"r0\r\nr1\r\nr2"); // cursor parks on the bottom row
    assert_cursor(&t, 2, 2);
    t.advance(b"\x1bD"); // IND at the bottom margin scrolls the region up
    assert_eq!(&row(&t, 0)[..2], "r1", "r0 scrolled off the top");
    assert_eq!(&row(&t, 1)[..2], "r2");
    assert_eq!(row(&t, 2), "      ", "blank pulled in at the bottom");
    // IND keeps the column even when it scrolls at the bottom margin.
    assert_cursor(&t, 2, 2);
}

#[test]
fn ri_moves_up_and_scrolls_down_at_top_margin() {
    let mut t = term(3, 6);
    t.advance(b"r0\r\nr1\r\nr2");
    t.advance(b"\x1b[1;1H"); // home -> top margin (0,0)
    t.advance(b"\x1bM"); // RI at the top margin scrolls the region DOWN
    assert_eq!(row(&t, 0), "      ", "blank pushed in at the top");
    assert_eq!(&row(&t, 1)[..2], "r0", "r0 pushed down");
    assert_eq!(&row(&t, 2)[..2], "r1", "r2 scrolled off the bottom");
    assert_cursor(&t, 0, 0);
}

#[test]
fn ri_above_top_margin_just_moves_up() {
    let mut t = term(4, 6);
    t.advance(b"\x1b[3;5H"); // (2,4)
    t.advance(b"\x1bM"); // RI not at top margin -> simple up-one, column kept
    assert_cursor(&t, 1, 4);
}

#[test]
fn nel_homes_column_and_advances_row() {
    let mut t = term(4, 10);
    t.advance(b"\x1b[1;5H"); // (0,4)
    t.advance(b"\x1bE"); // NEL — CR + LF: column to 0, row down one
    assert_cursor(&t, 1, 0);
}

// ===========================================================================
// 12. Tab-stop semantics: HTS sets, TBC clears, `\t` advances to the next set
// stop. Section 6 covered the single-stop and clear-all cases; these pin the
// MULTI-stop interplay (two custom stops + advance across both, and the
// "no stop ahead -> last column" fallback after a targeted clear).
// ===========================================================================

#[test]
fn two_custom_tab_stops_are_visited_in_order() {
    let mut t = term(2, 20);
    // Clear the default 8-col grid first so only our custom stops exist.
    t.advance(b"\x1b[3g"); // TBC 3: clear every stop
    t.advance(b"\x1b[1;4H"); // col 4 (1-based) -> (0,3)
    t.advance(b"\x1bH"); // HTS: stop at col 3
    t.advance(b"\x1b[1;10H"); // col 10 -> (0,9)
    t.advance(b"\x1bH"); // HTS: stop at col 9
    t.advance(b"\r"); // back to col 0
    t.advance(b"\t"); // first custom stop -> col 3
    assert_cursor(&t, 0, 3);
    t.advance(b"\t"); // second custom stop -> col 9
    assert_cursor(&t, 0, 9);
    // No stop beyond col 9 -> the next tab lands on the last column.
    t.advance(b"\t");
    assert_cursor(&t, 0, 19);
}

#[test]
fn tab_advances_across_two_default_stops() {
    let mut t = term(2, 40);
    // Default 8-col stops: col 8, 16, 24, ...
    t.advance(b"\t\t"); // two tabs from col 0 -> col 16
    assert_cursor(&t, 0, 16);
}

#[test]
fn cleared_stop_is_skipped_to_the_next_stop() {
    let mut t = term(2, 40);
    // Clear the col-8 stop; a tab from col 0 then skips to col 16.
    t.advance(b"\x1b[1;9H"); // col 9 -> (0,8)
    t.advance(b"\x1b[g"); // TBC 0 at the cursor column (8)
    t.advance(b"\r"); // back to col 0
    t.advance(b"\t");
    // col-8 stop cleared -> tab skips to the next surviving stop at col 16.
    assert_cursor(&t, 0, 16);
    // The col-16 stop survives: a second tab advances to col 24.
    t.advance(b"\t");
    assert_cursor(&t, 0, 24);
}

// ===========================================================================
// 13. Scroll-region edge interplay: IND honours a SET margin (the ESC-level
// sibling of the `line_feed_scrolls_within_region_only` LF case in section 3).
// ===========================================================================

#[test]
fn ind_scrolls_only_within_a_set_region() {
    let mut t = term(6, 6);
    t.advance(b"r0\r\nr1\r\nr2\r\nr3\r\nr4\r\nr5");
    t.advance(b"\x1b[2;4r"); // region rows 2..4 (1-based) -> [1,3]
    t.advance(b"\x1b[4;1H"); // to the bottom margin (row 4 1-based -> (3,0))
    t.advance(b"\x1bD"); // IND at the bottom margin: region scrolls, outside fixed
    assert_eq!(&row(&t, 0)[..2], "r0", "above region fixed");
    assert_eq!(&row(&t, 1)[..2], "r2", "region scrolled: r1 dropped");
    assert_eq!(&row(&t, 2)[..2], "r3");
    assert_eq!(row(&t, 3), "      ", "blank pulled in at region bottom");
    assert_eq!(&row(&t, 4)[..2], "r4", "below region fixed");
}

// ===========================================================================
// Conformance notes (gaps discovered while authoring this corpus):
//
//  * CUP into the LAST column clamps the cursor to `cols-1` (see
//    `cup_clamps_out_of_range_to_grid_edge`). The engine does not model the
//    xterm "pending wrap" / one-past-the-end column state for explicit cursor
//    addressing; the deferred-wrap state only arises from PRINTING into the
//    last column (`autowrap_wraps_at_right_margin`). Both behaviours are
//    asserted as the engine's real contract.
//  * `ESC ( A`, `ESC ( 1`, `ESC ( 2` charset designations are all folded to
//    US-ASCII (only `ESC ( 0` DEC line-drawing is distinct). Tests assert only
//    the ASCII vs. line-drawing distinction the engine actually makes.
//  * SGR underline-color (58/59) and styled underline (`4:n`) ARE modelled but
//    are exercised by `term/tests.rs` / `e2e_terminal.rs`; this corpus focuses
//    on the cursor/erase/scroll/SGR-basic/wrap/tab/wide families per the plan.
//  * SU/SD on a FULL-screen region feed scrollback; this corpus asserts the
//    margined-region (no-scrollback) behaviour to keep the assertions local to
//    the visible grid. Full-region scrollback feeding is covered in
//    `e2e_terminal.rs`.
// ===========================================================================
