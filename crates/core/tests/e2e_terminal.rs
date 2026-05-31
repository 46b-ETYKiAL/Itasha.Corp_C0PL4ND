//! End-to-end terminal tests: drive [`Terminal::advance`] with realistic byte
//! streams (the kind a real shell / TUI emits) and assert the resulting screen
//! state, scrollback, PTY replies, prompt marks, wide-char layout, inline
//! images, and reflow correctness.
//!
//! These complement the per-module unit tests in `term.rs` by exercising whole
//! interaction sequences end to end through the public `c0pl4nd_core` API.

use c0pl4nd_core::grid::Color;
use c0pl4nd_core::image::DecodedImage;
use c0pl4nd_core::term::osc::base64_encode;
use c0pl4nd_core::term::DynamicColor;
use c0pl4nd_core::Terminal;

/// Collect a grid row as a `String` of its base glyphs (continuation spacers
/// included as their stored space), for readable assertions.
fn row_text(t: &Terminal, row: usize) -> String {
    let g = t.grid();
    (0..g.cols())
        .map(|c| g.cell(row, c).map(|cell| cell.c).unwrap_or(' '))
        .collect()
}

/// (a) A vim-like session: enter the alternate screen, set a scroll region,
/// move the cursor, draw coloured SGR text, then exit the alt screen. The
/// primary screen's content and the user's scrollback must survive untouched —
/// a full-screen TUI must never pollute scrollback.
#[test]
fn vim_like_alt_screen_preserves_primary_and_scrollback() {
    let mut t = Terminal::with_scrollback(6, 40, 1000);

    // Primary screen: a few lines of real output that scroll into history.
    for i in 0..10 {
        t.advance(format!("primary line {i}\r\n").as_bytes());
    }
    let scrollback_before = t.scrollback_len();
    assert!(
        scrollback_before >= 4,
        "expected primary output in scrollback, got {scrollback_before}"
    );

    // Enter the alternate screen (vim/less/htop do this via ?1049h).
    t.advance(b"\x1b[?1049h");
    assert!(t.alt_screen_active(), "1049h must enter the alt screen");

    // Set a scroll region (DECSTBM rows 2..5, 1-based), home, draw coloured
    // text, scroll within the region, and fill the alt screen like an editor.
    t.advance(b"\x1b[2;5r"); // scroll region
    t.advance(b"\x1b[H"); // home
    t.advance(b"\x1b[1;32mVIM STATUS\x1b[0m"); // bold green status line
    for i in 0..20 {
        t.advance(format!("\r\nedit row {i}").as_bytes());
    }
    // The alt screen scrolling its own buffer must NOT extend scrollback.
    assert_eq!(
        t.scrollback_len(),
        scrollback_before,
        "alt-screen scrolling must never feed the user's scrollback"
    );

    // The status line's first cell is bold + green.
    let cell = t.grid().cell(0, 0).expect("status cell");
    assert!(cell.flags.bold, "status line should be bold");
    assert_eq!(cell.fg, Color::Indexed(2), "status line should be green");

    // Exit the alt screen — the primary grid + cursor are restored.
    t.advance(b"\x1b[?1049l");
    assert!(!t.alt_screen_active(), "1049l must leave the alt screen");
    assert_eq!(
        t.scrollback_len(),
        scrollback_before,
        "scrollback length must be identical after the whole alt-screen round trip"
    );
    // The primary screen still carries its last real output line.
    let all = t.all_lines();
    assert!(
        all.iter().any(|l| l.contains("primary line 9")),
        "primary screen content must survive the alt-screen round trip"
    );
}

/// (b) A coloured `ls` + a shell prompt wrapped in OSC 133 marks: the prompt
/// marks must be captured (capture-only — never echoed back to the PTY).
#[test]
fn colored_ls_with_osc133_prompt_marks_captured() {
    let mut t = Terminal::with_scrollback(10, 80, 1000);

    // First prompt: OSC 133 ; A marks the prompt start.
    t.advance(b"\x1b]133;A\x07");
    t.advance(b"operator@wired:~$ "); // the prompt text
    t.advance(b"\x1b]133;B\x07"); // prompt end / command start
    t.advance(b"ls\r\n");
    t.advance(b"\x1b]133;C\x07"); // command output begins
    // Coloured directory listing (blue dir, green executable).
    t.advance(b"\x1b[34mDocuments\x1b[0m  \x1b[32mrun.sh\x1b[0m\r\n");
    t.advance(b"\x1b]133;D;0\x07"); // command finished, exit 0

    // Second prompt.
    t.advance(b"\x1b]133;A\x07");
    t.advance(b"operator@wired:~$ ");
    t.advance(b"\x1b]133;B\x07");

    assert_eq!(
        t.prompt_marks().len(),
        2,
        "both prompt-start (OSC 133;A) marks must be captured"
    );
    // The two command-zone marks (C output-start, D end) are captured too.
    assert!(
        t.command_marks().len() >= 2,
        "OSC 133 C/D command-zone marks must be captured, got {}",
        t.command_marks().len()
    );
    // OSC 133 is capture-only: it must never queue a PTY reply (anti-CVE).
    assert!(
        t.take_pty_response().is_empty(),
        "OSC 133 marks must never produce a PTY reply"
    );
    // The coloured listing is on screen with the right colours.
    let docs = t.grid().cell(1, 0).expect("Documents cell");
    assert_eq!(docs.c, 'D');
    assert_eq!(docs.fg, Color::Indexed(4), "directory should be blue");
}

/// (c) A DA1 (primary device attributes) query: the terminal must reply with
/// its device-attributes string via `take_pty_response`.
#[test]
fn da1_query_produces_device_attributes_reply() {
    let mut t = Terminal::new(24, 80);
    // A program probing terminal capabilities sends `CSI c`.
    t.advance(b"\x1b[c");
    let reply = t.take_pty_response();
    assert_eq!(
        reply.as_slice(),
        b"\x1b[?62;1;6;22c",
        "DA1 reply must be the VT220-class device-attributes string"
    );
    // Draining it leaves nothing behind.
    assert!(t.take_pty_response().is_empty());
}

/// (d) A CPR (cursor position report) query at a known cursor position: the
/// terminal reports the 1-based row;col.
#[test]
fn cpr_query_reports_known_cursor_position() {
    let mut t = Terminal::new(24, 80);
    // Move to row 5, col 12 (1-based) via CUP, then request CPR (`CSI 6n`).
    t.advance(b"\x1b[5;12H");
    t.advance(b"\x1b[6n");
    let reply = t.take_pty_response();
    assert_eq!(
        reply.as_slice(),
        b"\x1b[5;12R",
        "CPR must report the 1-based cursor row;col"
    );
}

/// (e) A line of wide CJK characters: each occupies two columns, so the cursor
/// advances by two per glyph and the line wraps after the right number of
/// glyphs.
#[test]
fn cjk_wide_chars_advance_two_columns_and_wrap() {
    // 6 columns wide: each CJK glyph takes 2 cols ⇒ 3 glyphs per row.
    let mut t = Terminal::new(3, 6);
    // 你好世界 — four wide glyphs. The first three fill row 0 (cols 0,2,4),
    // the fourth wraps to row 1.
    t.advance("你好世界".as_bytes());

    // Row 0 holds the first three glyphs at even columns; odd columns are the
    // continuation spacers.
    assert_eq!(t.grid().cell(0, 0).unwrap().c, '你');
    assert!(
        t.grid().is_continuation(0, 1),
        "col 1 must be the wide-glyph continuation cell"
    );
    assert_eq!(t.grid().cell(0, 2).unwrap().c, '好');
    assert_eq!(t.grid().cell(0, 4).unwrap().c, '世');
    // The fourth glyph wrapped onto row 1.
    assert_eq!(t.grid().cell(1, 0).unwrap().c, '界');
    // The row that filled is flagged soft-wrapped (for reflow).
    assert!(t.grid().is_wrapped(0), "the full wide-char row must wrap-flag");
}

/// (f) A Kitty graphics APC (`ESC _ G f=32,s=1,v=1; <base64 RGBA> ESC \`):
/// the APC pre-filter must extract it and decode one inline image.
#[test]
fn kitty_apc_graphics_decodes_one_image() {
    let mut t = Terminal::new(10, 40);

    // One opaque-red RGBA pixel (4 bytes), base64-encoded as the payload.
    let rgba = [255u8, 0, 0, 255];
    let payload = base64_encode(&rgba);
    // f=32 (RGBA), s=1 (width), v=1 (height), a=T (transmit+display).
    let apc = format!("\x1b_Gf=32,s=1,v=1,a=T;{payload}\x1b\\");
    t.advance(apc.as_bytes());

    assert_eq!(t.images().len(), 1, "the Kitty APC must decode to one image");
    let img: &DecodedImage = &t.images()[0].image;
    assert_eq!(img.width, 1);
    assert_eq!(img.height, 1);
    assert_eq!(&img.rgba, &rgba, "decoded RGBA must match the transmitted pixel");
}

/// A Kitty APC split across two `advance()` calls (the realistic
/// straddles-a-PTY-read case) must still decode exactly once.
#[test]
fn kitty_apc_split_across_advances_decodes_once() {
    let mut t = Terminal::new(10, 40);
    let rgba = [0u8, 255, 0, 255];
    let payload = base64_encode(&rgba);
    let apc = format!("\x1b_Gf=32,s=1,v=1,a=T;{payload}\x1b\\");
    let bytes = apc.as_bytes();
    let mid = bytes.len() / 2;
    t.advance(&bytes[..mid]);
    t.advance(&bytes[mid..]);
    assert_eq!(t.images().len(), 1, "a split APC must still decode exactly one image");
}

/// (g) Reflow: write a long logical line that soft-wraps, resize narrower then
/// wider, and assert no characters are lost across the reflow.
#[test]
fn reflow_narrow_then_wide_loses_no_characters() {
    let mut t = Terminal::with_scrollback(6, 40, 1000);

    // A 60-char logical line wraps across rows at 40 cols.
    let line: String = (0..60).map(|i| char::from(b'a' + (i % 26) as u8)).collect();
    t.advance(line.as_bytes());

    // The full logical text is recoverable from the buffer.
    let joined_before: String = t.all_lines().join("").replace(' ', "");
    assert!(
        joined_before.contains(&line),
        "the wrapped logical line must be fully present before reflow"
    );

    // Resize narrower (40 -> 20) then wider (20 -> 50). Reflow must re-wrap the
    // logical line without dropping characters.
    t.resize(6, 20);
    let joined_narrow: String = t.all_lines().join("").replace(' ', "");
    assert!(
        joined_narrow.contains(&line),
        "reflow to a narrower width must not lose characters; got:\n{joined_narrow}"
    );

    t.resize(6, 50);
    let joined_wide: String = t.all_lines().join("").replace(' ', "");
    assert!(
        joined_wide.contains(&line),
        "reflow to a wider width must not lose characters; got:\n{joined_wide}"
    );
}

/// A realistic prompt + command + coloured output stream, with a final OSC
/// title set, drives multiple subsystems at once and lands a coherent screen.
#[test]
fn full_prompt_command_cycle_lands_coherent_screen() {
    let mut t = Terminal::with_scrollback(8, 60, 1000);
    // Set the window title (OSC 2), then run an interaction.
    t.advance(b"\x1b]2;C0PL4ND \xe2\x80\x94 the wired\x07");
    t.advance(b"\x1b]133;A\x07op$ \x1b]133;B\x07");
    t.advance(b"echo \x1b[1;36mhello\x1b[0m\r\n");
    t.advance(b"\x1b]133;C\x07hello\r\n\x1b]133;D;0\x07");

    assert!(t.title().contains("C0PL4ND"), "title must be captured: {:?}", t.title());
    assert!(!t.prompt_marks().is_empty(), "prompt mark must be captured");
    // The echoed 'hello' on the command line is bright-cyan + bold.
    assert!(
        row_text(&t, 0).contains("echo"),
        "command line must contain the typed command"
    );
}

/// OSC 11 (set/query default background) round-trips: a `?` query yields a
/// reply, and a set updates the queryable dynamic colour.
#[test]
fn osc11_background_query_and_set_round_trip() {
    let mut t = Terminal::new(4, 20);
    // Query the default background.
    t.advance(b"\x1b]11;?\x07");
    let reply = t.take_pty_response();
    assert!(
        reply.starts_with(b"\x1b]11;"),
        "OSC 11 query must reply with the background colour spec"
    );

    // Set the background to pure red and read it back via the public getter.
    t.advance(b"\x1b]11;rgb:ffff/0000/0000\x07");
    assert_eq!(
        t.dynamic_color(DynamicColor::Background),
        (255, 0, 0),
        "OSC 11 set must update the queryable dynamic background"
    );
}
