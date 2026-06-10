//! Unit tests for the VT parser / terminal state machine, split out of
//! `term.rs` (F6-1 decomposition) — pure structural move, no test changes.

use super::*;

#[test]
fn prints_plain_text() {
    let mut t = Terminal::new(4, 20);
    t.advance(b"hello");
    assert!(t.grid().to_text().starts_with("hello"));
}

#[test]
fn clear_damage_resets_until_next_write() {
    // The renderer's damage gate (PaneTerm::grid_rows) relies on this:
    // clear_damage() must reset is_damaged(), and a later write must re-mark
    // it so the next frame redraws.
    let mut t = Terminal::new(4, 20);
    t.advance(b"hi");
    assert!(t.grid().is_damaged(), "a write marks the row dirty");
    t.clear_damage();
    assert!(
        !t.grid().is_damaged(),
        "clear_damage resets every per-row damage bit"
    );
    t.advance(b"x");
    assert!(
        t.grid().is_damaged(),
        "a write after clear_damage re-marks the row dirty"
    );
}

#[test]
fn handles_crlf() {
    let mut t = Terminal::new(4, 20);
    t.advance(b"ab\r\ncd");
    let text = t.grid().to_text();
    let mut lines = text.lines();
    assert!(lines.next().unwrap().starts_with("ab"));
    assert!(lines.next().unwrap().starts_with("cd"));
}

#[test]
fn sgr_sets_indexed_color() {
    let mut t = Terminal::new(2, 10);
    t.advance(b"\x1b[31mR"); // red foreground
    let cell = t.grid().cell(0, 0).unwrap();
    assert_eq!(cell.c, 'R');
    assert_eq!(cell.fg, Color::Indexed(1));
}

#[test]
fn sgr_truecolor() {
    let mut t = Terminal::new(2, 10);
    t.advance(b"\x1b[38;2;0;229;255mX");
    assert_eq!(t.grid().cell(0, 0).unwrap().fg, Color::Rgb(0, 229, 255));
}

#[test]
fn erase_display_clears() {
    let mut t = Terminal::new(2, 10);
    t.advance(b"junk\x1b[2J");
    assert_eq!(t.grid().cell(0, 0).unwrap().c, ' ');
}

#[test]
fn osc_sets_title() {
    let mut t = Terminal::new(2, 10);
    t.advance(b"\x1b]0;C0PL4ND\x07");
    assert_eq!(t.title(), "C0PL4ND");
}

#[test]
fn line_wrap_advances_row() {
    let mut t = Terminal::new(3, 3);
    t.advance(b"abcd"); // wraps after 3 cols
    assert_eq!(t.grid().cell(0, 0).unwrap().c, 'a');
    assert_eq!(t.grid().cell(1, 0).unwrap().c, 'd');
}

#[test]
fn scrollback_retains_lines_pushed_off_top() {
    let mut t = Terminal::with_scrollback(2, 4, 100);
    // 5 lines into a 2-row grid: 3 lines scroll into history.
    t.advance(b"L0\r\nL1\r\nL2\r\nL3\r\nL4");
    assert!(
        t.scrollback_len() >= 3,
        "history should retain scrolled lines"
    );
    let all = t.all_lines();
    assert!(all.iter().any(|l| l.starts_with("L0")));
    assert!(all.iter().any(|l| l.starts_with("L4")));
}

#[test]
fn scroll_view_offset_clamps_and_resets() {
    let mut t = Terminal::with_scrollback(2, 4, 100);
    t.advance(b"a\r\nb\r\nc\r\nd\r\ne");
    t.scroll_up_view(1000);
    assert_eq!(
        t.view_offset(),
        t.scrollback_len(),
        "offset clamps to history"
    );
    t.scroll_to_bottom();
    assert_eq!(t.view_offset(), 0);
}

#[test]
fn display_rows_follows_live_at_bottom() {
    let mut t = Terminal::new(3, 5);
    t.advance(b"x");
    let rows = t.display_rows();
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0][0].c, 'x');
}

#[test]
fn osc8_captures_hyperlink() {
    let mut t = Terminal::new(2, 40);
    t.advance(b"\x1b]8;;https://itasha.corp\x07link\x1b]8;;\x07");
    assert_eq!(t.hyperlinks(), &["https://itasha.corp".to_string()]);
}

#[test]
fn title_is_length_capped() {
    // A hostile OSC 2 stuffing a huge title must NOT be stored verbatim
    // (memory-DoS). The stored title is capped to TITLE_MAX_CHARS.
    let mut t = Terminal::new(2, 40);
    let mut seq = b"\x1b]2;".to_vec();
    seq.extend(std::iter::repeat_n(b'A', 100_000));
    seq.push(0x07);
    t.advance(&seq);
    assert!(
        t.title().chars().count() <= Screen::TITLE_MAX_CHARS,
        "title must be length-capped (was {})",
        t.title().chars().count()
    );
}

#[test]
fn osc52_oversized_write_is_dropped() {
    // A multi-megabyte OSC 52 clipboard write must be dropped, not buffered.
    let mut t = Terminal::new(2, 40);
    let big_b64 = "QQ".repeat(1_500_000); // ~3 MB of base64 → >1 MiB decoded
    let mut seq = b"\x1b]52;c;".to_vec();
    seq.extend_from_slice(big_b64.as_bytes());
    seq.push(0x07);
    t.advance(&seq);
    assert!(
        t.take_clipboard_writes().is_empty(),
        "an oversized OSC 52 write must be dropped"
    );

    // A small write still works (cap doesn't break the legit feature).
    t.advance(b"\x1b]52;c;aGVsbG8=\x07"); // base64("hello")
    assert_eq!(
        t.take_clipboard_writes().len(),
        1,
        "a small OSC 52 write is kept"
    );
}

/// SECURITY (device-reply echo-to-stdin, the #1 terminal-RCE class —
/// CVE-2022-45872 etc.): every reply the terminal queues for the PTY must be
/// built ONLY from validated internal state, never reflect attacker-supplied
/// request bytes, and must be 7-bit-clean (no embedded C0 controls other than
/// the `ESC` / `BEL` / `ST` framing). We feed a battery of malformed
/// DECRQSS / XTGETTCAP / OSC-color-query requests carrying hostile bytes and
/// assert the drained reply never smuggles a control byte that could be
/// echoed onto the shell's stdin as if typed.
#[test]
fn device_replies_are_7bit_clean_with_no_smuggled_controls() {
    let inputs: &[&[u8]] = &[
        b"\x1bP$qm\x1b\\",        // DECRQSS: request SGR
        b"\x1bP$q\"q\x1b\\",      // DECRQSS: request DECSCA
        b"\x1bP$q\x07evil\x1b\\", // DECRQSS with an embedded BEL + junk
        b"\x1bP+q686f7374\x1b\\", // XTGETTCAP: hex name "host"
        b"\x1bP+q00ff\x1b\\",     // XTGETTCAP: hex decoding to NUL/0xff
        b"\x1b]10;?\x07",         // OSC 10 foreground color query
        b"\x1b]11;?\x1b\\",       // OSC 11 background color query (ST-terminated)
        b"\x1b]4;1;?\x07",        // OSC 4 indexed color query
    ];
    for inp in inputs {
        let mut t = Terminal::new(4, 20);
        t.advance(inp);
        let reply = t.take_pty_response();
        for (i, &b) in reply.iter().enumerate() {
            let is_framing = b == 0x1b || b == 0x5c || b == 0x07; // ESC, '\', BEL
            let is_printable = (0x20..=0x7e).contains(&b);
            assert!(
                is_framing || is_printable,
                "device reply for {inp:x?} smuggled control byte {b:#04x} at {i}: {reply:x?}"
            );
        }
    }
}

#[test]
fn osc133_records_prompt_mark() {
    let mut t = Terminal::new(4, 20);
    t.advance(b"\x1b]133;A\x07$ ");
    assert_eq!(t.prompt_marks().len(), 1);
}

#[test]
fn dcs_sixel_captures_image() {
    let mut t = Terminal::new(4, 20);
    // DCS q ... ST  with a red colour def + one full sixel column.
    t.advance(b"\x1bPq#0;2;100;0;0~\x1b\\");
    assert_eq!(t.images().len(), 1);
    assert_eq!(t.images()[0].image.height, 6);
}

#[test]
fn scrollback_cap_is_enforced() {
    let mut t = Terminal::with_scrollback(1, 4, 2);
    for i in 0..10 {
        t.advance(format!("{i}\r\n").as_bytes());
    }
    assert!(t.scrollback_len() <= 2, "history must not exceed the cap");
}

// ---- Audit finding #4: anchored-metadata Vecs are bounded under a flood ----

#[test]
fn hyperlink_vec_is_count_capped_under_flood() {
    let mut t = Terminal::with_scrollback(4, 80, 100_000);
    // Emit far more OSC 8 hyperlinks than the cap. Each carries a distinct
    // URI so none is de-duplicated; without the cap this Vec grows forever.
    let flood = Screen::HYPERLINKS_MAX + 5_000;
    for i in 0..flood {
        t.advance(format!("\x1b]8;;https://h/{i}\x07x\x1b]8;;\x07").as_bytes());
    }
    assert!(
        t.hyperlinks().len() <= Screen::HYPERLINKS_MAX,
        "hyperlinks must stay <= cap ({}), got {}",
        Screen::HYPERLINKS_MAX,
        t.hyperlinks().len()
    );
    // The most-recent URI is retained (ring-buffer keeps the newest).
    let last = format!("https://h/{}", flood - 1);
    assert_eq!(t.hyperlinks().last(), Some(&last));
}

#[test]
fn prompt_marks_vec_is_count_capped_under_flood() {
    // Huge scrollback so a unique `abs` is produced per mark (abs grows
    // monotonically with history.len()), exercising the count cap, not
    // the dedup path.
    let mut t = Terminal::with_scrollback(2, 8, 1_000_000);
    let flood = Screen::PROMPT_MARKS_MAX + 2_000;
    for _ in 0..flood {
        // A newline bumps history.len() so the next mark's abs differs.
        t.advance(b"\x1b]133;A\x07\r\n");
    }
    assert!(
        t.prompt_marks().len() <= Screen::PROMPT_MARKS_MAX,
        "prompt_marks must stay <= cap ({}), got {}",
        Screen::PROMPT_MARKS_MAX,
        t.prompt_marks().len()
    );
}

#[test]
fn command_marks_vec_is_count_capped_under_flood() {
    let mut t = Terminal::with_scrollback(2, 8, 1_000_000);
    let flood = Screen::COMMAND_MARKS_MAX + 2_000;
    for _ in 0..flood {
        // OSC 133 ; C marks output-start; newline keeps abs advancing.
        t.advance(b"\x1b]133;C\x07\r\n");
    }
    assert!(
        t.command_marks().len() <= Screen::COMMAND_MARKS_MAX,
        "command_marks must stay <= cap ({}), got {}",
        Screen::COMMAND_MARKS_MAX,
        t.command_marks().len()
    );
}

#[test]
fn images_vec_is_count_capped_under_flood() {
    let mut t = Terminal::with_scrollback(4, 20, 1_000_000);
    // Each minimal Sixel produces one TerminalImage. Flood past the cap.
    let flood = Screen::IMAGES_MAX + 500;
    for _ in 0..flood {
        t.advance(b"\x1bPq#0;2;100;0;0~\x1b\\");
        // Advance the cursor so successive images anchor at distinct rows.
        t.advance(b"\r\n");
    }
    assert!(
        t.images().len() <= Screen::IMAGES_MAX,
        "images must stay <= cap ({}), got {}",
        Screen::IMAGES_MAX,
        t.images().len()
    );
}

#[test]
fn images_vec_is_byte_capped_under_flood() {
    let mut t = Terminal::with_scrollback(4, 20, 1_000_000);
    // Flood enough small images that, were they all retained, total RGBA
    // bytes would exceed the byte cap. The byte cap evicts oldest first.
    for _ in 0..(Screen::IMAGES_MAX) {
        t.advance(b"\x1bPq#0;2;100;0;0~\x1b\\");
        t.advance(b"\r\n");
    }
    let total: usize = t.images().iter().map(|i| i.image.rgba.len()).sum();
    assert!(
        total <= Screen::IMAGES_MAX_BYTES,
        "retained image bytes ({total}) must stay <= byte cap ({})",
        Screen::IMAGES_MAX_BYTES
    );
}

#[test]
fn erase_scrollback_reanchors_and_drops_stale_marks() {
    // A small grid + scrollback. Build history, place a prompt mark on a
    // scrolled-off line and another on the live grid, then ESC[3J.
    let mut t = Terminal::with_scrollback(2, 8, 100);
    // Scroll some lines into history first.
    t.advance(b"a\r\nb\r\nc\r\nd\r\n");
    // Mark on the current live grid row (will survive the scrollback erase,
    // re-based to the new history length 0).
    t.advance(b"\x1b]133;A\x07");
    let before = t.prompt_marks().len();
    assert!(before >= 1, "expected at least one prompt mark");
    // Erase scrollback. The live-grid mark survives but re-bases so it is
    // still consistent with the now-zero history length.
    t.advance(b"\x1b[3J");
    assert_eq!(t.scrollback_len(), 0, "scrollback cleared");
    // Surviving marks must now sit within the live coordinate space
    // (history.len() + grid rows) — never dangling above it.
    let rows = t.grid().rows();
    for &m in t.prompt_marks() {
        assert!(
            m <= t.scrollback_len() + rows,
            "re-anchored prompt mark {m} out of live range"
        );
    }
}

#[test]
fn hard_reset_clears_anchored_metadata() {
    let mut t = Terminal::with_scrollback(2, 8, 100);
    t.advance(b"\x1b]8;;https://x\x07L\x1b]8;;\x07");
    t.advance(b"\x1b]133;A\x07");
    t.advance(b"\x1bPq#0;2;100;0;0~\x1b\\");
    assert!(!t.hyperlinks().is_empty());
    // RIS hard reset must drop all anchored metadata (grid + scrollback gone).
    t.advance(b"\x1bc");
    assert!(
        t.hyperlinks().is_empty(),
        "hyperlinks cleared on hard reset"
    );
    assert!(
        t.prompt_marks().is_empty(),
        "prompt_marks cleared on hard reset"
    );
    assert!(t.images().is_empty(), "images cleared on hard reset");
    assert!(
        t.command_marks().is_empty(),
        "command_marks cleared on hard reset"
    );
}

/// Deterministic robustness regression mirroring the `vt_parser` fuzz
/// target (see `fuzz/fuzz_targets/vt_parser.rs`). A terminal parser
/// consumes fully untrusted bytes; hostile or malformed escape sequences
/// must never panic, hang, or produce an inconsistent grid. These seeds
/// double as the fuzzer's regression corpus and run in the normal stable
/// test suite on every platform (the fuzz harness itself needs nightly).
#[test]
fn parser_survives_adversarial_escape_sequences() {
    let seeds: &[&[u8]] = &[
        b"\x1b[",                                  // bare CSI, no final byte
        b"\x1b[999999999999999999999999999m",      // CSI param overflow
        b"\x1b[;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;m", // many empty params
        b"\x1b[38;2;",                             // truncated truecolor SGR
        b"\x1b]0;",                                // OSC with no terminator
        b"\x1b]8;;",                               // OSC 8 hyperlink, truncated
        b"\x1b]52;c;",                             // OSC 52 clipboard, truncated
        b"\x1b]1337;File=",                        // iTerm2 image, truncated
        b"\x1bPq",                                 // DCS / Sixel introducer
        b"\x1b#8",                                 // DECALN screen-align
        b"\x08\x08\x08\x08",                       // backspaces past col 0
        b"\x1b[999999;999999H",                    // cursor move far OOB
        b"\x1b[2J\x1b[3J\x1b[1J\x1b[0J",           // erase-display variants
        b"\xff\xfe\xfd\xfc\x00\x01\x02",           // invalid UTF-8 / control bytes
        b"\xe2\x82",                               // truncated UTF-8 multibyte
        b"\x1b[6n\x1b[5n",                         // device status report queries
    ];

    for seed in seeds {
        let mut t = Terminal::with_scrollback(24, 80, 1000);
        // Feed in 1-byte chunks so sequences straddle advance() calls —
        // the realistic split-across-PTY-reads case.
        for b in seed.iter() {
            t.advance(&[*b]);
        }
        // Touch the derived read surface to catch read-side inconsistency.
        let _ = t.title();
        let _ = t.cwd();
        let _ = t.hyperlinks();
        let _ = t.images();
        let _ = t.display_rows();
        let _ = t.all_lines();
        let _ = t.scrollback_len();
        // The new mode-state read surface must also stay consistent.
        let _ = t.dec_modes();
        let _ = t.is_cursor_visible();
        let _ = t.alt_screen_active();
        let _ = t.mouse_mode();
        let _ = t.mouse_encoding();
        let _ = t.cursor_shape();
        let _ = t.cursor_blink();
    }
}

// ---- DEC private mode framework (item 1) ----

#[test]
fn dec_modes_default_state() {
    let t = Terminal::new(4, 20);
    let m = t.dec_modes();
    assert!(m.cursor_visible, "cursor visible by default");
    assert!(m.autowrap, "autowrap on by default");
    assert!(!m.bracketed_paste);
    assert!(!m.application_cursor_keys);
    assert!(!m.focus_reporting);
    assert!(!m.sync_output);
    assert_eq!(m.mouse_mode, MouseMode::Off);
    assert_eq!(m.mouse_encoding, MouseEncoding::X10);
}

#[test]
fn dec_mode_set_and_reset_cursor_visibility() {
    let mut t = Terminal::new(4, 20);
    t.advance(b"\x1b[?25l");
    assert!(!t.is_cursor_visible(), "?25l hides the cursor");
    t.advance(b"\x1b[?25h");
    assert!(t.is_cursor_visible(), "?25h shows it again");
}

#[test]
fn dec_mode_multiple_params_in_one_sequence() {
    let mut t = Terminal::new(4, 20);
    // Enter alt screen AND select SGR mouse encoding in one CSI.
    t.advance(b"\x1b[?1049;1006h");
    assert!(t.alt_screen_active(), "1049 applied");
    assert_eq!(
        t.mouse_encoding(),
        MouseEncoding::Sgr,
        "1006 applied from the same sequence"
    );
}

#[test]
fn dec_mode_bracketed_paste_focus_sync() {
    let mut t = Terminal::new(4, 20);
    t.advance(b"\x1b[?2004h\x1b[?1004h\x1b[?2026h");
    assert!(t.bracketed_paste());
    assert!(t.focus_reporting());
    assert!(t.sync_output());
    t.advance(b"\x1b[?2004l\x1b[?1004l\x1b[?2026l");
    assert!(!t.bracketed_paste());
    assert!(!t.focus_reporting());
    assert!(!t.sync_output());
}

#[test]
fn dec_mode_mouse_tracking_modes() {
    let mut t = Terminal::new(4, 20);
    t.advance(b"\x1b[?1000h");
    assert_eq!(t.mouse_mode(), MouseMode::Normal);
    t.advance(b"\x1b[?1002h");
    assert_eq!(t.mouse_mode(), MouseMode::ButtonEvent);
    t.advance(b"\x1b[?1003h");
    assert_eq!(t.mouse_mode(), MouseMode::AnyEvent);
    t.advance(b"\x1b[?1003l");
    assert_eq!(t.mouse_mode(), MouseMode::Off);
}

#[test]
fn dec_mode_application_cursor_keys_and_autowrap() {
    let mut t = Terminal::new(4, 20);
    t.advance(b"\x1b[?1h");
    assert!(t.application_cursor_keys());
    t.advance(b"\x1b[?7l");
    assert!(!t.autowrap());
    t.advance(b"\x1b[?7h");
    assert!(t.autowrap());
}

#[test]
fn dec_mode_unknown_number_is_ignored() {
    let mut t = Terminal::new(4, 20);
    // 9999 is not a mode we model; must not panic or disturb defaults.
    t.advance(b"\x1b[?9999h");
    assert!(t.is_cursor_visible());
    assert_eq!(t.mouse_mode(), MouseMode::Off);
}

// ---- Alternate screen (item 2) ----

#[test]
fn alt_screen_preserves_primary_content_and_cursor() {
    let mut t = Terminal::new(4, 20);
    t.advance(b"primary"); // cursor now at col 7
    t.advance(b"\x1b[?1049h"); // enter alt
    assert!(t.alt_screen_active());
    // Alt screen starts blank.
    assert_eq!(t.grid().cell(0, 0).unwrap().c, ' ');
    t.advance(b"ALTBUF");
    assert_eq!(t.grid().cell(0, 0).unwrap().c, 'A');
    t.advance(b"\x1b[?1049l"); // leave alt
    assert!(!t.alt_screen_active());
    // Primary content is intact and the cursor was restored.
    assert!(t.grid().to_text().starts_with("primary"));
}

#[test]
fn alt_screen_does_not_pollute_scrollback() {
    let mut t = Terminal::with_scrollback(2, 4, 100);
    t.advance(b"\x1b[?1049h");
    // Scroll the alt screen well past its height.
    t.advance(b"a\r\nb\r\nc\r\nd\r\ne\r\nf");
    assert_eq!(
        t.scrollback_len(),
        0,
        "alt-screen scrolling must not feed scrollback"
    );
    t.advance(b"\x1b[?1049l");
    assert_eq!(t.scrollback_len(), 0);
}

#[test]
fn alt_screen_47_variant_switches_without_cursor_save() {
    let mut t = Terminal::new(4, 20);
    t.advance(b"hi");
    t.advance(b"\x1b[?47h");
    assert!(t.alt_screen_active());
    t.advance(b"\x1b[?47l");
    assert!(!t.alt_screen_active());
    assert!(t.grid().to_text().starts_with("hi"));
}

#[test]
fn alt_screen_duplicate_enter_is_noop() {
    let mut t = Terminal::new(4, 20);
    t.advance(b"base");
    t.advance(b"\x1b[?1049h");
    t.advance(b"XX");
    t.advance(b"\x1b[?1049h"); // second enter must not clobber the saved primary
    t.advance(b"\x1b[?1049l");
    assert!(t.grid().to_text().starts_with("base"));
}

// ---- Bracketed paste (item 3) ----

#[test]
fn bracketed_paste_flag_tracks_mode() {
    let mut t = Terminal::new(4, 20);
    assert!(!t.bracketed_paste());
    t.advance(b"\x1b[?2004h");
    assert!(t.bracketed_paste());
    t.advance(b"\x1b[?2004l");
    assert!(!t.bracketed_paste());
}

// ---- Cursor visibility (item 4) covered by dec_mode_set_and_reset_cursor_visibility ----

// ---- DECSCUSR cursor shape (item 5) ----

#[test]
fn decscusr_sets_shapes() {
    let cases: &[(&[u8], CursorShape, bool)] = &[
        (b"\x1b[0 q", CursorShape::Block, true),
        (b"\x1b[1 q", CursorShape::Block, true),
        (b"\x1b[2 q", CursorShape::Block, false),
        (b"\x1b[3 q", CursorShape::Underline, true),
        (b"\x1b[4 q", CursorShape::Underline, false),
        (b"\x1b[5 q", CursorShape::Bar, true),
        (b"\x1b[6 q", CursorShape::Bar, false),
    ];
    for (seq, shape, blink) in cases {
        let mut t = Terminal::new(2, 10);
        t.advance(seq);
        assert_eq!(t.cursor_shape(), *shape, "shape for {seq:?}");
        assert_eq!(t.cursor_blink(), *blink, "blink for {seq:?}");
    }
}

#[test]
fn decscusr_default_is_block() {
    let t = Terminal::new(2, 10);
    assert_eq!(t.cursor_shape(), CursorShape::Block);
}

#[test]
fn cursor_position_tracks_display_space() {
    let mut t = Terminal::new(4, 20);
    assert_eq!(t.cursor_position(), Some((0, 0)), "home at start");
    t.advance(b"hello");
    assert_eq!(t.cursor_position(), Some((0, 5)), "advanced 5 cols");
    t.advance(b"\r\nx");
    assert_eq!(t.cursor_position(), Some((1, 1)), "next row, 1 col");
    // CSI H homes the cursor.
    t.advance(b"\x1b[H");
    assert_eq!(t.cursor_position(), Some((0, 0)), "CUP home");
}

#[test]
fn dec_mode_12_drives_cursor_blink() {
    let mut t = Terminal::new(2, 10);
    // Steady block via DECSCUSR (blink=false), then ?12h enables blink.
    t.advance(b"\x1b[2 q");
    assert!(!t.cursor_blink());
    t.advance(b"\x1b[?12h");
    assert!(t.cursor_blink(), "?12h enables blink independently");
}

// ---- DECAWM autowrap behaviour ----

#[test]
fn autowrap_off_clamps_to_last_column() {
    let mut t = Terminal::new(3, 3);
    t.advance(b"\x1b[?7l"); // disable autowrap
    t.advance(b"abcd"); // 'd' overwrites the last cell instead of wrapping
    assert_eq!(t.grid().cell(0, 0).unwrap().c, 'a');
    assert_eq!(t.grid().cell(0, 2).unwrap().c, 'd', "last col overwritten");
    assert_eq!(t.grid().cell(1, 0).unwrap().c, ' ', "no wrap to next line");
}

// ---- Mouse encoding helper (item 6) ----

#[test]
fn encode_mouse_off_returns_none() {
    let t = Terminal::new(4, 20);
    let out = t.encode_mouse(
        MouseButton::Left,
        MouseModifiers::default(),
        5,
        7,
        MouseEventKind::Press,
    );
    assert!(out.is_none(), "no report when mouse mode is off");
}

#[test]
fn encode_mouse_sgr_left_press_and_release() {
    let mut t = Terminal::new(4, 20);
    t.advance(b"\x1b[?1000h\x1b[?1006h");
    let press = t
        .encode_mouse(
            MouseButton::Left,
            MouseModifiers::default(),
            5,
            7,
            MouseEventKind::Press,
        )
        .unwrap();
    assert_eq!(press, b"\x1b[<0;5;7M");
    let release = t
        .encode_mouse(
            MouseButton::Left,
            MouseModifiers::default(),
            5,
            7,
            MouseEventKind::Release,
        )
        .unwrap();
    assert_eq!(release, b"\x1b[<0;5;7m", "release uses lowercase final m");
}

#[test]
fn encode_mouse_sgr_modifiers_and_buttons() {
    let mut t = Terminal::new(4, 20);
    t.advance(b"\x1b[?1000h\x1b[?1006h");
    // Right button (2) + control (16) = 18.
    let out = t
        .encode_mouse(
            MouseButton::Right,
            MouseModifiers {
                control: true,
                ..Default::default()
            },
            1,
            1,
            MouseEventKind::Press,
        )
        .unwrap();
    assert_eq!(out, b"\x1b[<18;1;1M");
}

#[test]
fn encode_mouse_x10_press_offsets_by_32() {
    let mut t = Terminal::new(4, 20);
    t.advance(b"\x1b[?1000h"); // X10 encoding by default
    let out = t
        .encode_mouse(
            MouseButton::Left,
            MouseModifiers::default(),
            1,
            1,
            MouseEventKind::Press,
        )
        .unwrap();
    // CSI M  Cb(0+32=32=' ')  Cx(1+32=33='!')  Cy(1+32=33='!')
    assert_eq!(out, b"\x1b[M !!");
}

#[test]
fn encode_mouse_x10_clamps_large_coords() {
    let mut t = Terminal::new(4, 20);
    t.advance(b"\x1b[?1000h");
    let out = t
        .encode_mouse(
            MouseButton::Left,
            MouseModifiers::default(),
            1000,
            1000,
            MouseEventKind::Press,
        )
        .unwrap();
    // Coords clamp to 223; 223 + 32 = 255.
    assert_eq!(out[0], 0x1b);
    assert_eq!(&out[1..3], b"[M");
    assert_eq!(out[4], 255, "x clamps to 255");
    assert_eq!(out[5], 255, "y clamps to 255");
}

#[test]
fn encode_mouse_normal_mode_drops_motion() {
    let mut t = Terminal::new(4, 20);
    t.advance(b"\x1b[?1000h\x1b[?1006h");
    let out = t.encode_mouse(
        MouseButton::Left,
        MouseModifiers::default(),
        5,
        5,
        MouseEventKind::Motion,
    );
    assert!(out.is_none(), "?1000 reports buttons only, not motion");
}

#[test]
fn encode_mouse_button_event_motion_requires_button() {
    let mut t = Terminal::new(4, 20);
    t.advance(b"\x1b[?1002h\x1b[?1006h");
    // Motion with no button held → no report.
    assert!(t
        .encode_mouse(
            MouseButton::None,
            MouseModifiers::default(),
            3,
            3,
            MouseEventKind::Motion,
        )
        .is_none());
    // Motion while a button is held → reported (drag, +32 motion bit).
    let drag = t
        .encode_mouse(
            MouseButton::Left,
            MouseModifiers::default(),
            3,
            3,
            MouseEventKind::Motion,
        )
        .unwrap();
    assert_eq!(drag, b"\x1b[<32;3;3M", "drag sets the motion bit (0+32)");
}

#[test]
fn encode_mouse_any_event_reports_bare_motion() {
    let mut t = Terminal::new(4, 20);
    t.advance(b"\x1b[?1003h\x1b[?1006h");
    let out = t
        .encode_mouse(
            MouseButton::None,
            MouseModifiers::default(),
            2,
            2,
            MouseEventKind::Motion,
        )
        .unwrap();
    // No button base = 3, + motion 32 = 35.
    assert_eq!(out, b"\x1b[<35;2;2M");
}

#[test]
fn encode_mouse_urxvt_encoding() {
    let mut t = Terminal::new(4, 20);
    t.advance(b"\x1b[?1000h\x1b[?1015h");
    let out = t
        .encode_mouse(
            MouseButton::Left,
            MouseModifiers::default(),
            5,
            7,
            MouseEventKind::Press,
        )
        .unwrap();
    // urxvt: button offset by 32 → 32; decimal coords; final M.
    assert_eq!(out, b"\x1b[32;5;7M");
}

#[test]
fn encode_mouse_wheel_up_is_button_64() {
    let mut t = Terminal::new(4, 20);
    t.advance(b"\x1b[?1000h\x1b[?1006h");
    let out = t
        .encode_mouse(
            MouseButton::WheelUp,
            MouseModifiers::default(),
            1,
            1,
            MouseEventKind::Press,
        )
        .unwrap();
    assert_eq!(out, b"\x1b[<64;1;1M");
}

// ---- OSC 52 clipboard ----

#[test]
fn osc52_clipboard_write_decodes_base64() {
    let mut t = Terminal::new(4, 20);
    // "hello" base64 = aGVsbG8=
    t.advance(b"\x1b]52;c;aGVsbG8=\x07");
    let w = t.take_clipboard_write().expect("clipboard write");
    assert_eq!(w.selection, ClipboardSelection::Clipboard);
    assert_eq!(w.text, "hello");
    assert!(t.take_clipboard_write().is_none(), "drained once");
}

#[test]
fn osc52_clipboard_write_text_zeroized_on_demand() {
    // P-V3: a drained ClipboardWrite's sensitive `text` is wiped by
    // `zeroize()` — the buffer becomes empty AND its previously-occupied
    // backing bytes are scrubbed (verified via the raw heap pointer, the
    // canonical zeroize test). This is the same buffer the app drops after
    // copying to the OS clipboard, so the Drop impl scrubs it identically.
    let mut t = Terminal::new(4, 20);
    // "s3cr3t-token" base64.
    t.advance(b"\x1b]52;c;czNjcjN0LXRva2Vu\x07");
    let mut w = t.take_clipboard_write().expect("clipboard write");
    assert_eq!(w.text, "s3cr3t-token");

    let ptr = w.text.as_ptr();
    let len = w.text.len();
    assert!(len > 0);

    w.zeroize();

    // After zeroize the logical string is empty…
    assert!(w.text.is_empty(), "text must be emptied by zeroize");
    // …and the bytes that backed the secret are wiped to zero. The buffer
    // capacity is retained by zeroize::Zeroize for String, so the original
    // allocation is still valid to read here.
    // SAFETY: `ptr`/`len` describe the still-allocated backing buffer of
    // `w.text`; zeroize keeps the allocation (only sets len=0), so reading
    // `len` bytes from `ptr` is in-bounds. No aliasing: `w` is not borrowed.
    let wiped = unsafe { std::slice::from_raw_parts(ptr, len) };
    assert!(
        wiped.iter().all(|&b| b == 0),
        "secret bytes must be zeroed after zeroize(), got {wiped:?}"
    );
}

#[test]
fn hard_reset_clears_pending_clipboard_writes() {
    // P-V3: a hard reset (RIS, `ESC c`) scrubs any still-pending OSC 52
    // clipboard payloads the app had not yet drained.
    let mut t = Terminal::new(4, 20);
    t.advance(b"\x1b]52;c;aGVsbG8=\x07"); // "hello" queued
                                          // RIS — full reset. Must wipe the pending queue.
    t.advance(b"\x1bc");
    assert!(
        t.take_clipboard_write().is_none(),
        "pending clipboard writes must be cleared by a hard reset"
    );
}

#[test]
fn osc52_primary_selection() {
    let mut t = Terminal::new(4, 20);
    t.advance(b"\x1b]52;p;aGVsbG8=\x07");
    let w = t.take_clipboard_write().unwrap();
    assert_eq!(w.selection, ClipboardSelection::Primary);
}

#[test]
fn osc52_read_default_off_emits_nothing() {
    let mut t = Terminal::new(4, 20);
    // Read request: payload is '?'. Default-off -> no PTY response, no write.
    t.advance(b"\x1b]52;c;?\x07");
    assert!(t.take_clipboard_write().is_none());
    assert!(
        t.take_pty_response().is_empty(),
        "must NOT auto-respond with host clipboard contents"
    );
}

#[test]
fn osc52_read_opt_in_uses_app_provided_text() {
    let mut t = Terminal::new(4, 20);
    // Even opted in, the core never reads the host clipboard from the OSC
    // sequence; the host must supply the text explicitly.
    t.set_clipboard_read_enabled(true);
    t.advance(b"\x1b]52;c;?\x07");
    assert!(
        t.take_pty_response().is_empty(),
        "the read request alone emits nothing"
    );
    t.respond_clipboard_read(ClipboardSelection::Clipboard, "hi");
    // "hi" base64 = aGk=
    assert_eq!(t.take_pty_response().as_slice(), b"\x1b]52;c;aGk=\x07");
}

// ---- OSC 4 / 10 / 11 / 12 colors ----

#[test]
fn osc4_set_indexed_color() {
    let mut t = Terminal::new(4, 20);
    t.advance(b"\x1b]4;1;rgb:ff/00/00\x07");
    assert_eq!(t.palette_color(1), (255, 0, 0));
    assert_eq!(
        t.take_color_sets(),
        vec![ColorSet::Indexed {
            index: 1,
            rgb: (255, 0, 0)
        }]
    );
}

#[test]
fn osc4_query_emits_reply() {
    let mut t = Terminal::new(4, 20);
    t.advance(b"\x1b]4;1;rgb:ff/00/00\x07");
    let _ = t.take_color_sets();
    t.advance(b"\x1b]4;1;?\x07");
    assert_eq!(
        t.take_pty_response().as_slice(),
        b"\x1b]4;1;rgb:ffff/0000/0000\x07"
    );
}

#[test]
fn osc11_background_query_emits_reply() {
    let mut t = Terminal::new(4, 20);
    // Default background is xterm black -> rgb:0000/0000/0000
    t.advance(b"\x1b]11;?\x07");
    assert_eq!(
        t.take_pty_response().as_slice(),
        b"\x1b]11;rgb:0000/0000/0000\x07"
    );
}

#[test]
fn osc10_foreground_set() {
    let mut t = Terminal::new(4, 20);
    t.advance(b"\x1b]10;rgb:12/34/56\x07");
    assert_eq!(
        t.dynamic_color(DynamicColor::Foreground),
        (0x12, 0x34, 0x56)
    );
    assert_eq!(
        t.take_color_sets(),
        vec![ColorSet::Dynamic {
            which: DynamicColor::Foreground,
            rgb: (0x12, 0x34, 0x56)
        }]
    );
}

#[test]
fn osc12_cursor_set() {
    let mut t = Terminal::new(4, 20);
    t.advance(b"\x1b]12;rgb:00/ff/00\x07");
    assert_eq!(t.dynamic_color(DynamicColor::Cursor), (0, 255, 0));
}

// ---- OSC 104 / 110-112 reset ----

#[test]
fn osc104_reset_single_index() {
    let mut t = Terminal::new(4, 20);
    let original = t.palette_color(2);
    t.advance(b"\x1b]4;2;rgb:ff/00/00\x07");
    let _ = t.take_color_sets();
    assert_eq!(t.palette_color(2), (255, 0, 0));
    t.advance(b"\x1b]104;2\x07");
    assert_eq!(t.palette_color(2), original);
}

#[test]
fn osc104_reset_all() {
    let mut t = Terminal::new(4, 20);
    t.advance(b"\x1b]4;5;rgb:ff/00/00\x07");
    let _ = t.take_color_sets();
    t.advance(b"\x1b]104\x07");
    assert_ne!(t.palette_color(5), (255, 0, 0));
    assert_eq!(
        t.take_color_sets().len(),
        256,
        "every entry reset is surfaced"
    );
}

#[test]
fn osc110_reset_foreground() {
    let mut t = Terminal::new(4, 20);
    let original = t.dynamic_color(DynamicColor::Foreground);
    t.advance(b"\x1b]10;rgb:ff/ff/ff\x07");
    let _ = t.take_color_sets();
    t.advance(b"\x1b]110\x07");
    assert_eq!(t.dynamic_color(DynamicColor::Foreground), original);
}

// ---- OSC 9 / 777 notifications ----

#[test]
fn osc9_notification() {
    let mut t = Terminal::new(4, 20);
    t.advance(b"\x1b]9;Build complete\x07");
    let n = t.take_notification().expect("notification");
    assert_eq!(n.title, "");
    assert_eq!(n.body, "Build complete");
}

#[test]
fn osc777_notification() {
    let mut t = Terminal::new(4, 20);
    t.advance(b"\x1b]777;notify;Title;Body text\x07");
    let n = t.take_notification().unwrap();
    assert_eq!(n.title, "Title");
    assert_eq!(n.body, "Body text");
}

// ---- Title stack (XTWINOPS 22/23) ----

#[test]
fn title_stack_push_pop() {
    let mut t = Terminal::new(4, 20);
    t.advance(b"\x1b]2;First\x07");
    assert_eq!(t.title(), "First");
    t.advance(b"\x1b[22;0t"); // push
    assert_eq!(t.title_stack_depth(), 1);
    t.advance(b"\x1b]2;Second\x07");
    assert_eq!(t.title(), "Second");
    t.advance(b"\x1b[23;0t"); // pop
    assert_eq!(t.title(), "First");
    assert_eq!(t.title_stack_depth(), 0);
}

#[test]
fn title_stack_pop_empty_is_noop() {
    let mut t = Terminal::new(4, 20);
    t.advance(b"\x1b]2;Only\x07");
    t.advance(b"\x1b[23;0t"); // pop with empty stack
    assert_eq!(t.title(), "Only");
}

// ---- OSC 133 still never replies (regression guard) ----

#[test]
fn osc133_never_writes_pty_response() {
    let mut t = Terminal::new(4, 20);
    t.advance(b"\x1b]133;A\x07");
    assert!(
        t.take_pty_response().is_empty(),
        "OSC 133 marks must remain capture-only (anti-CVE)"
    );
}

// ---- Kitty graphics protocol (APC extraction + decode + placement) ----

#[test]
fn kitty_apc_displays_image_and_passes_text_through() {
    let mut t = Terminal::new(4, 20);
    // "hi" then a Kitty APC (a defaults to T) for a 1x1 RGBA image
    // (payload [1,2,3,4] = base64 "AQIDBA=="), ST-terminated, then "ok".
    t.advance(b"hi\x1b_Gf=32,s=1,v=1;AQIDBA==\x1b\\ok");
    // Exactly one image at the cursor.
    assert_eq!(t.images().len(), 1, "one image produced");
    let img = &t.images()[0].image;
    assert_eq!(img.width, 1);
    assert_eq!(img.height, 1);
    assert_eq!(&img.rgba, &[1, 2, 3, 4]);
    // The non-APC text reaches the grid intact ("hiok").
    assert!(
        t.grid().to_text().starts_with("hiok"),
        "non-APC bytes must reach the grid: got {:?}",
        t.grid().to_text()
    );
}

#[test]
fn kitty_apc_split_across_two_advances() {
    let mut t = Terminal::new(4, 20);
    // Cut the APC mid-payload across two advance() calls.
    t.advance(b"hi\x1b_Gf=32,s=1,v=1;AQID");
    // Nothing finalised yet; "hi" is on the grid, no image.
    assert_eq!(t.images().len(), 0, "APC not yet terminated");
    t.advance(b"BA==\x1b\\ok");
    assert_eq!(t.images().len(), 1, "one image after the second chunk");
    assert_eq!(&t.images()[0].image.rgba, &[1, 2, 3, 4]);
    assert!(
        t.grid().to_text().starts_with("hiok"),
        "no stray APC bytes leak to the grid: got {:?}",
        t.grid().to_text()
    );
}

#[test]
fn kitty_apc_bel_terminated() {
    let mut t = Terminal::new(4, 20);
    // BEL (0x07) terminates the APC instead of ST.
    t.advance(b"x\x1b_Gf=32,s=1,v=1;AQIDBA==\x07y");
    assert_eq!(t.images().len(), 1, "BEL-terminated APC produces an image");
    assert_eq!(&t.images()[0].image.rgba, &[1, 2, 3, 4]);
    assert!(t.grid().to_text().starts_with("xy"));
}

#[test]
fn kitty_chunked_transmission_m_flag() {
    let mut t = Terminal::new(4, 20);
    // 1x1 RGBA split into two base64 chunks via m=1 / m=0, same id.
    // "AQID" then "BA==" together decode to [1,2,3,4].
    t.advance(b"\x1b_Gf=32,s=1,v=1,i=9,m=1;AQID\x1b\\");
    assert_eq!(t.images().len(), 0, "more=1 chunk does not finalise");
    t.advance(b"\x1b_Ga=T,i=9,m=0;BA==\x1b\\");
    assert_eq!(t.images().len(), 1, "m=0 finalises the chunked image");
    assert_eq!(&t.images()[0].image.rgba, &[1, 2, 3, 4]);
}

#[test]
fn kitty_transmit_only_then_display() {
    let mut t = Terminal::new(4, 20);
    // a=t stores the image without displaying it.
    t.advance(b"\x1b_Ga=t,f=32,s=1,v=1,i=5;AQIDBA==\x1b\\");
    assert_eq!(t.images().len(), 0, "a=t must not display");
    // a=p displays the previously-stored id.
    t.advance(b"\x1b_Ga=p,i=5\x1b\\");
    assert_eq!(t.images().len(), 1, "a=p displays the stored image");
    assert_eq!(&t.images()[0].image.rgba, &[1, 2, 3, 4]);
}

#[test]
fn kitty_delete_clears_storage() {
    let mut t = Terminal::new(4, 20);
    t.advance(b"\x1b_Ga=t,f=32,s=1,v=1,i=3;AQIDBA==\x1b\\");
    // a=d clears the store; a later a=p finds nothing.
    t.advance(b"\x1b_Ga=d\x1b\\");
    t.advance(b"\x1b_Ga=p,i=3\x1b\\");
    assert_eq!(t.images().len(), 0, "a=d cleared the stored image");
}

#[test]
fn kitty_f24_rgb_displayed_as_rgba() {
    let mut t = Terminal::new(4, 20);
    // 1x1 RGB (3 bytes [10,20,30] = base64 "ChQe"); expands to RGBA.
    t.advance(b"\x1b_Gf=24,s=1,v=1;ChQe\x1b\\");
    assert_eq!(t.images().len(), 1);
    assert_eq!(&t.images()[0].image.rgba, &[10, 20, 30, 255]);
}

#[test]
fn non_kitty_apc_is_swallowed_and_text_survives() {
    let mut t = Terminal::new(4, 20);
    // A non-graphics APC (no leading G) is swallowed (matching vte); the
    // surrounding text still reaches the grid and no image is produced.
    t.advance(b"a\x1b_Xsome-other-apc\x1b\\b");
    assert_eq!(t.images().len(), 0);
    assert!(
        t.grid().to_text().starts_with("ab"),
        "text around a non-kitty APC survives: got {:?}",
        t.grid().to_text()
    );
}

#[test]
fn esc_not_introducing_apc_passes_through_intact() {
    let mut t = Terminal::new(4, 20);
    // A plain SGR escape (ESC not followed by '_') must reach vte intact.
    t.advance(b"\x1b[31mR");
    assert_eq!(t.grid().cell(0, 0).unwrap().c, 'R');
    assert_eq!(t.grid().cell(0, 0).unwrap().fg, Color::Indexed(1));
    assert_eq!(t.images().len(), 0);
}

// ============================================================
// VT correctness P0 batch (C1-C8)
// ============================================================

// ---- C1: DA1 / DA2 device attributes ----

#[test]
fn da1_primary_device_attributes_reply() {
    let mut t = Terminal::new(4, 20);
    t.advance(b"\x1b[c");
    assert_eq!(t.take_pty_response().as_slice(), b"\x1b[?62;1;6;22c");
}

#[test]
fn da1_with_explicit_zero_param_replies() {
    let mut t = Terminal::new(4, 20);
    t.advance(b"\x1b[0c");
    assert_eq!(t.take_pty_response().as_slice(), b"\x1b[?62;1;6;22c");
}

#[test]
fn da2_secondary_device_attributes_reply() {
    let mut t = Terminal::new(4, 20);
    t.advance(b"\x1b[>c");
    assert_eq!(t.take_pty_response().as_slice(), b"\x1b[>0;0;0c");
}

// ---- C2: DSR / CPR ----

#[test]
fn dsr_status_report_ok() {
    let mut t = Terminal::new(4, 20);
    t.advance(b"\x1b[5n");
    assert_eq!(t.take_pty_response().as_slice(), b"\x1b[0n");
}

#[test]
fn cpr_reports_one_based_cursor_position() {
    let mut t = Terminal::new(10, 40);
    // Move cursor to row 3, col 7 (0-based 2,6) then request CPR.
    t.advance(b"\x1b[3;7H\x1b[6n");
    assert_eq!(t.take_pty_response().as_slice(), b"\x1b[3;7R");
}

#[test]
fn cpr_after_printing_text() {
    let mut t = Terminal::new(4, 20);
    t.advance(b"hello\x1b[6n"); // cursor at row1 col6 (1-based)
    assert_eq!(t.take_pty_response().as_slice(), b"\x1b[1;6R");
}

// ---- C3: IL / DL / ICH / DCH / ECH ----

#[test]
fn ich_inserts_blanks_shifting_right() {
    let mut t = Terminal::new(2, 6);
    t.advance(b"abcdef\x1b[H"); // fill row 0, home
    t.advance(b"\x1b[3G"); // move to col 3 (1-based) = 0-based 2 ('c')
    t.advance(b"\x1b[2@"); // insert 2 blanks
    let line: String = (0..6).map(|c| t.grid().cell(0, c).unwrap().c).collect();
    assert_eq!(line, "ab  cd");
}

#[test]
fn dch_deletes_chars_shifting_left() {
    let mut t = Terminal::new(2, 6);
    t.advance(b"abcdef\x1b[H");
    t.advance(b"\x1b[3G\x1b[2P"); // at col 3, delete 2 chars
    let line: String = (0..6).map(|c| t.grid().cell(0, c).unwrap().c).collect();
    assert_eq!(line, "abef  ");
}

#[test]
fn ech_erases_chars_without_shift() {
    let mut t = Terminal::new(2, 6);
    t.advance(b"abcdef\x1b[H");
    t.advance(b"\x1b[3G\x1b[2X"); // at col 3, erase 2
    let line: String = (0..6).map(|c| t.grid().cell(0, c).unwrap().c).collect();
    assert_eq!(line, "ab  ef");
}

#[test]
fn il_inserts_lines_within_scroll_region() {
    let mut t = Terminal::new(4, 3);
    t.advance(b"aaa\r\nbbb\r\nccc\r\nddd");
    // Cursor to row 2 (1-based), insert 1 line.
    t.advance(b"\x1b[2;1H\x1b[L");
    assert_eq!(t.grid().cell(0, 0).unwrap().c, 'a');
    assert_eq!(
        t.grid().cell(1, 0).unwrap().c,
        ' ',
        "blank inserted at row 2"
    );
    assert_eq!(t.grid().cell(2, 0).unwrap().c, 'b', "bbb shifted down");
    assert_eq!(
        t.grid().cell(3, 0).unwrap().c,
        'c',
        "ccc shifted down; ddd lost"
    );
}

#[test]
fn dl_deletes_lines_within_scroll_region() {
    let mut t = Terminal::new(4, 3);
    t.advance(b"aaa\r\nbbb\r\nccc\r\nddd");
    // Cursor to row 2, delete 1 line.
    t.advance(b"\x1b[2;1H\x1b[M");
    assert_eq!(t.grid().cell(0, 0).unwrap().c, 'a');
    assert_eq!(t.grid().cell(1, 0).unwrap().c, 'c', "ccc shifted up");
    assert_eq!(t.grid().cell(2, 0).unwrap().c, 'd', "ddd shifted up");
    assert_eq!(t.grid().cell(3, 0).unwrap().c, ' ', "blank at bottom");
}

#[test]
fn il_dl_respect_custom_scroll_region() {
    let mut t = Terminal::new(5, 3);
    t.advance(b"r0\r\nr1\r\nr2\r\nr3\r\nr4");
    // Region rows 2..4 (1-based), i.e. 0-based 1..3.
    t.advance(b"\x1b[2;4r");
    // After DECSTBM the cursor is homed to the top margin (row 1, 0-based).
    // Delete 1 line at the top of the region.
    t.advance(b"\x1b[M");
    assert_eq!(t.grid().cell(0, 0).unwrap().c, 'r', "row 0 untouched");
    assert_eq!(t.grid().cell(0, 1).unwrap().c, '0');
    assert_eq!(
        t.grid().cell(1, 1).unwrap().c,
        '2',
        "r2 shifted up into region top"
    );
    assert_eq!(
        t.grid().cell(3, 1).unwrap().c,
        ' ',
        "blank at region bottom"
    );
    assert_eq!(
        t.grid().cell(4, 1).unwrap().c,
        '4',
        "row 4 below region untouched"
    );
}

// ---- C4: DECSC / DECRC + SCOSC / SCORC ----

#[test]
fn decsc_decrc_round_trips_cursor() {
    let mut t = Terminal::new(6, 20);
    t.advance(b"\x1b[3;5H"); // row 3 col 5 (1-based)
    t.advance(b"\x1b7"); // DECSC save
    t.advance(b"\x1b[1;1H"); // home
    assert_eq!(t.cursor_position(), Some((0, 0)));
    t.advance(b"\x1b8"); // DECRC restore
    assert_eq!(t.cursor_position(), Some((2, 4)), "restored to row3,col5");
}

#[test]
fn scosc_scorc_aliases_save_restore() {
    let mut t = Terminal::new(6, 20);
    t.advance(b"\x1b[4;3H\x1b[s"); // CSI s save
    t.advance(b"\x1b[1;1H\x1b[u"); // home then CSI u restore
    assert_eq!(t.cursor_position(), Some((3, 2)));
}

#[test]
fn decsc_saves_pen() {
    let mut t = Terminal::new(2, 10);
    t.advance(b"\x1b[31m\x1b7"); // red pen, save
    t.advance(b"\x1b[0m"); // reset pen
    t.advance(b"\x1b8X"); // restore -> prints red X
    assert_eq!(t.grid().cell(0, 0).unwrap().fg, Color::Indexed(1));
}

// ---- C5: DECSTBM scroll region ----

#[test]
fn decstbm_constrains_scrolling() {
    let mut t = Terminal::new(4, 3);
    // Region = rows 1..2 (1-based) = 0-based 0..1.
    t.advance(b"\x1b[1;2r");
    // Fill the region and force a scroll: rows below the region stay put.
    t.advance(b"x3\r\n"); // row3 marker first
                          // Reset region to write a fixed bottom line, then re-set region.
    t.advance(b"\x1b[1;4r\x1b[4;1Hbot\x1b[1;2r\x1b[1;1H");
    // Now scroll within region 0..1 by printing 3 lines.
    t.advance(b"AA\r\nBB\r\nCC");
    // Region top should now hold BB (AA scrolled out of the 2-row region).
    assert_eq!(t.grid().cell(0, 0).unwrap().c, 'B');
    assert_eq!(t.grid().cell(1, 0).unwrap().c, 'C');
    // The fixed bottom line outside the region is preserved.
    let bottom: String = (0..3).map(|c| t.grid().cell(3, c).unwrap().c).collect();
    assert_eq!(bottom, "bot");
}

#[test]
fn decstbm_no_params_resets_full_screen() {
    let mut t = Terminal::new(3, 3);
    t.advance(b"\x1b[1;2r"); // custom region
    t.advance(b"\x1b[r"); // reset
                          // Full-screen scroll feeds scrollback again.
    let mut t2 = Terminal::with_scrollback(3, 3, 100);
    t2.advance(b"\x1b[1;2r\x1b[r");
    t2.advance(b"a\r\nb\r\nc\r\nd");
    assert!(
        t2.scrollback_len() >= 1,
        "full region feeds scrollback after reset"
    );
    let _ = t;
}

// ---- C6: DEC line-drawing charset ----

#[test]
fn dec_line_drawing_maps_box_chars() {
    let mut t = Terminal::new(2, 10);
    t.advance(b"\x1b(0"); // select DEC special graphics into G0
    t.advance(b"lqk"); // upper-left, horiz, upper-right
    assert_eq!(t.grid().cell(0, 0).unwrap().c, '\u{250c}'); // ┌
    assert_eq!(t.grid().cell(0, 1).unwrap().c, '\u{2500}'); // ─
    assert_eq!(t.grid().cell(0, 2).unwrap().c, '\u{2510}'); // ┐
}

#[test]
fn esc_paren_b_returns_to_ascii() {
    let mut t = Terminal::new(2, 10);
    t.advance(b"\x1b(0q\x1b(Bq"); // graphics q (─) then ASCII q
    assert_eq!(t.grid().cell(0, 0).unwrap().c, '\u{2500}');
    assert_eq!(t.grid().cell(0, 1).unwrap().c, 'q', "ASCII restored");
}

#[test]
fn si_so_switch_g0_g1() {
    let mut t = Terminal::new(2, 10);
    // G0 = ASCII (default), G1 = line-drawing.
    t.advance(b"\x1b)0"); // designate G1 = graphics
    t.advance(b"q"); // GL=G0=ASCII -> 'q'
    t.advance(b"\x0eq"); // SO -> GL=G1=graphics -> ─
    t.advance(b"\x0fq"); // SI -> back to G0 ASCII -> 'q'
    assert_eq!(t.grid().cell(0, 0).unwrap().c, 'q');
    assert_eq!(t.grid().cell(0, 1).unwrap().c, '\u{2500}');
    assert_eq!(t.grid().cell(0, 2).unwrap().c, 'q');
}

// ---- C7: wide-cell width ----

#[test]
fn wide_char_advances_two_columns() {
    let mut t = Terminal::new(2, 10);
    t.advance("世".as_bytes()); // East-Asian wide
                                // Occupies cols 0 + 1 (continuation spacer); cursor now at col 2.
    assert_eq!(t.grid().cell(0, 0).unwrap().c, '世');
    assert_eq!(t.grid().cell(0, 1).unwrap().c, ' ', "continuation spacer");
    assert_eq!(t.cursor_position(), Some((0, 2)));
}

#[test]
fn wide_char_then_narrow() {
    let mut t = Terminal::new(2, 10);
    t.advance("世a".as_bytes());
    assert_eq!(t.grid().cell(0, 0).unwrap().c, '世');
    assert_eq!(t.grid().cell(0, 2).unwrap().c, 'a', "narrow lands at col 2");
}

#[test]
fn wide_char_wraps_at_last_column() {
    let mut t = Terminal::new(2, 3); // 3 cols
    t.advance(b"ab"); // cols 0,1 filled; cursor at col 2 (last)
    t.advance("世".as_bytes()); // can't fit width-2 at col 2 -> wraps to row 1
    assert_eq!(t.grid().cell(0, 0).unwrap().c, 'a');
    assert_eq!(t.grid().cell(0, 1).unwrap().c, 'b');
    assert_eq!(
        t.grid().cell(1, 0).unwrap().c,
        '世',
        "wide char wrapped to next row"
    );
    assert_eq!(t.grid().cell(1, 1).unwrap().c, ' ');
}

// ---- C8: ED / EL sub-modes ----

#[test]
fn ed_mode0_erases_cursor_to_end() {
    let mut t = Terminal::new(2, 4);
    t.advance(b"abcd\r\nefgh");
    t.advance(b"\x1b[1;3H\x1b[0J"); // row1 col3, erase to end
    assert_eq!(t.grid().cell(0, 0).unwrap().c, 'a');
    assert_eq!(t.grid().cell(0, 1).unwrap().c, 'b');
    assert_eq!(t.grid().cell(0, 2).unwrap().c, ' ', "from cursor erased");
    assert_eq!(t.grid().cell(1, 0).unwrap().c, ' ', "rows below erased");
}

#[test]
fn ed_mode1_erases_start_to_cursor() {
    let mut t = Terminal::new(2, 4);
    t.advance(b"abcd\r\nefgh");
    t.advance(b"\x1b[2;2H\x1b[1J"); // row2 col2, erase start->cursor
    assert_eq!(t.grid().cell(0, 0).unwrap().c, ' ', "row above erased");
    assert_eq!(t.grid().cell(1, 0).unwrap().c, ' ');
    assert_eq!(t.grid().cell(1, 1).unwrap().c, ' ', "cursor cell inclusive");
    assert_eq!(t.grid().cell(1, 2).unwrap().c, 'g', "after cursor kept");
}

#[test]
fn ed_mode3_clears_scrollback() {
    let mut t = Terminal::with_scrollback(2, 4, 100);
    t.advance(b"L0\r\nL1\r\nL2\r\nL3");
    assert!(t.scrollback_len() > 0);
    t.advance(b"\x1b[3J");
    assert_eq!(t.scrollback_len(), 0, "ESC[3J clears scrollback");
}

#[test]
fn el_mode1_erases_bol_to_cursor() {
    let mut t = Terminal::new(2, 5);
    t.advance(b"abcde");
    t.advance(b"\x1b[1;3H\x1b[1K"); // col3, erase BOL->cursor
    assert_eq!(t.grid().cell(0, 0).unwrap().c, ' ');
    assert_eq!(t.grid().cell(0, 1).unwrap().c, ' ');
    assert_eq!(t.grid().cell(0, 2).unwrap().c, ' ', "cursor inclusive");
    assert_eq!(t.grid().cell(0, 3).unwrap().c, 'd', "after cursor kept");
}

#[test]
fn el_mode2_erases_whole_line() {
    let mut t = Terminal::new(2, 5);
    t.advance(b"abcde\x1b[1;3H\x1b[2K");
    for c in 0..5 {
        assert_eq!(t.grid().cell(0, c).unwrap().c, ' ');
    }
}

// ---- C9/C10 bonus: ESC M reverse index, RIS, DECSTR ----

#[test]
fn reverse_index_scrolls_region_down_at_top() {
    let mut t = Terminal::new(3, 3);
    t.advance(b"aaa\r\nbbb\r\nccc");
    t.advance(b"\x1b[1;1H\x1bM"); // home then RI -> scroll down
    assert_eq!(
        t.grid().cell(0, 0).unwrap().c,
        ' ',
        "blank scrolled in at top"
    );
    assert_eq!(t.grid().cell(1, 0).unwrap().c, 'a', "aaa pushed down");
}

#[test]
fn ris_resets_terminal() {
    let mut t = Terminal::with_scrollback(2, 4, 100);
    t.advance(b"junk\r\nmore\r\noverflow\x1b[31m");
    t.advance(b"\x1bc"); // RIS
    assert_eq!(t.grid().cell(0, 0).unwrap().c, ' ', "screen cleared");
    assert_eq!(t.scrollback_len(), 0, "scrollback cleared");
    assert_eq!(t.cursor_position(), Some((0, 0)));
    t.advance(b"x"); // pen reset -> default fg
    assert_eq!(t.grid().cell(0, 0).unwrap().fg, Color::Default);
}

#[test]
fn decstr_soft_reset_keeps_scrollback() {
    let mut t = Terminal::with_scrollback(2, 4, 100);
    t.advance(b"L0\r\nL1\r\nL2");
    let hist = t.scrollback_len();
    t.advance(b"\x1b[!p"); // DECSTR soft reset
    assert_eq!(t.scrollback_len(), hist, "soft reset preserves scrollback");
    assert_eq!(t.cursor_position(), Some((0, 0)));
}

// ---- Bonus: REP, CHA/VPA absolute moves ----

#[test]
fn rep_repeats_last_char() {
    let mut t = Terminal::new(2, 10);
    t.advance(b"x\x1b[3b"); // print x, repeat 3 more
    let line: String = (0..4).map(|c| t.grid().cell(0, c).unwrap().c).collect();
    assert_eq!(line, "xxxx");
}

#[test]
fn rep_count_is_clamped_no_dos() {
    // An attacker-controlled huge REP count must NOT spin billions of times
    // (the iTerm2 REP DoS). The clamp bounds it to <= MAX_REP (~1M) so this
    // returns promptly; we just assert it completes and the grid is sane.
    let mut t = Terminal::with_scrollback(24, 80, 1000);
    let start = std::time::Instant::now();
    t.advance(b"z\x1b[2000000000b"); // REP 2 billion
    assert!(
        start.elapsed() < std::time::Duration::from_secs(5),
        "REP with a 2-billion count must be clamped, not loop unbounded"
    );
    // The visible cursor row is full of 'z' (sanity: parsing still works).
    assert_eq!(
        t.grid().cell(t.cursor_position().unwrap().0, 0).unwrap().c,
        'z'
    );
}

// ---- Paste-injection guard (frame_paste) ----

#[test]
fn frame_paste_unbracketed_strips_end_sentinel() {
    let t = Terminal::new(4, 20);
    assert!(!t.bracketed_paste());
    // A hostile clipboard payload carrying an embedded ESC[201~ must have it
    // stripped even on the un-bracketed path; no 200~/201~ framing is added.
    let out = t.frame_paste("a\x1b[201~rm -rf ~");
    assert_eq!(out, b"arm -rf ~");
    assert!(!contains_subslice(&out, b"\x1b[201~"));
}

#[test]
fn frame_paste_bracketed_wraps_and_neutralizes_injection() {
    let mut t = Terminal::new(4, 20);
    t.advance(b"\x1b[?2004h"); // enable bracketed paste
    assert!(t.bracketed_paste());
    // The classic pastejacking payload: an embedded ESC[201~ tries to close
    // the bracket early so `rm -rf ~\n` runs as typed. frame_paste must strip
    // the embedded sentinel and wrap the whole (now-safe) payload exactly
    // once, so nothing escapes the bracket.
    let out = t.frame_paste("a\x1b[201~rm -rf ~\n");
    assert!(out.starts_with(b"\x1b[200~"), "must open the bracket");
    assert!(out.ends_with(b"\x1b[201~"), "must close the bracket");
    // Exactly ONE 201~ (the closing frame) — the embedded one was stripped.
    let closes = out.windows(6).filter(|w| *w == b"\x1b[201~").count();
    assert_eq!(closes, 1, "embedded end-sentinel must be stripped");
}

fn contains_subslice(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

#[test]
fn cha_vpa_absolute_moves() {
    let mut t = Terminal::new(5, 10);
    t.advance(b"\x1b[5G"); // column 5 (1-based) -> col 4
    assert_eq!(t.cursor_position(), Some((0, 4)));
    t.advance(b"\x1b[3d"); // row 3 (1-based) -> row 2
    assert_eq!(t.cursor_position(), Some((2, 4)));
}

// ============================================================
// VT correctness P1 batch (C14 / C16 / C19)
// ============================================================

// ---- C19: settable tab stops (HTS / TBC / CHT / CBT) ----

#[test]
fn tab_default_stops_every_eight() {
    let mut t = Terminal::new(2, 30);
    t.advance(b"\t"); // col 0 -> 8
    assert_eq!(t.cursor_position(), Some((0, 8)));
    t.advance(b"\t"); // 8 -> 16
    assert_eq!(t.cursor_position(), Some((0, 16)));
}

#[test]
fn tab_from_mid_default_stop_advances_to_next_multiple() {
    let mut t = Terminal::new(2, 30);
    t.advance(b"abc\t"); // col 3 -> next stop at 8
    assert_eq!(t.cursor_position(), Some((0, 8)));
}

#[test]
fn hts_sets_custom_tab_stop() {
    let mut t = Terminal::new(2, 30);
    // Move to col 3 (1-based 4) and set a stop there via HTS (ESC H).
    t.advance(b"\x1b[4G"); // col 4 (1-based) = col 3 (0-based)
    t.advance(b"\x1bH"); // HTS at col 3
                         // Home, then tab: should stop at the new custom stop (col 3), not col 8.
    t.advance(b"\x1b[1G\t");
    assert_eq!(
        t.cursor_position(),
        Some((0, 3)),
        "tab honours custom HTS stop"
    );
}

#[test]
fn tbc_clear_all_then_tab_goes_to_last_col() {
    let mut t = Terminal::new(2, 10);
    t.advance(b"\x1b[3g"); // TBC 3 — clear every stop
    t.advance(b"\x1b[1G\t"); // home, tab with no stops -> last column (9)
    assert_eq!(t.cursor_position(), Some((0, 9)), "no stops -> last col");
}

#[test]
fn tbc_clear_current_stop() {
    let mut t = Terminal::new(2, 30);
    // Clear the default stop at col 8, then tab from home jumps to col 16.
    t.advance(b"\x1b[9G"); // col 9 (1-based) = col 8 (0-based), a default stop
    t.advance(b"\x1b[0g"); // TBC 0 — clear stop at cursor (col 8)
    t.advance(b"\x1b[1G\t");
    assert_eq!(
        t.cursor_position(),
        Some((0, 16)),
        "cleared col-8 stop skipped"
    );
}

#[test]
fn cht_forward_tabs_n() {
    let mut t = Terminal::new(2, 40);
    t.advance(b"\x1b[3I"); // CHT 3 — forward 3 tab stops from col 0 -> 8,16,24
    assert_eq!(t.cursor_position(), Some((0, 24)));
}

#[test]
fn cbt_back_tabs_n() {
    let mut t = Terminal::new(2, 40);
    t.advance(b"\x1b[30G"); // col 30 (1-based) = col 29
    t.advance(b"\x1b[2Z"); // CBT 2 — back 2 stops: 24 then 16
    assert_eq!(t.cursor_position(), Some((0, 16)));
}

#[test]
fn cbt_stops_at_column_zero() {
    let mut t = Terminal::new(2, 40);
    t.advance(b"\x1b[5G"); // col 4
    t.advance(b"\x1b[9Z"); // back far more stops than exist
    assert_eq!(t.cursor_position(), Some((0, 0)), "CBT clamps at col 0");
}

#[test]
fn tab_stops_reset_on_ris() {
    let mut t = Terminal::new(2, 30);
    t.advance(b"\x1b[3g"); // clear all stops
    t.advance(b"\x1bc"); // RIS — restores default stops
    t.advance(b"\t");
    assert_eq!(
        t.cursor_position(),
        Some((0, 8)),
        "RIS restores default stops"
    );
}

// ---- C14: focus reporting emit (core half) ----

#[test]
fn focus_report_silent_when_mode_off() {
    let mut t = Terminal::new(4, 20);
    t.focus_report(true);
    t.focus_report(false);
    assert!(
        t.take_pty_response().is_empty(),
        "no focus reports unless ?1004 is enabled"
    );
}

#[test]
fn focus_report_emits_when_mode_on() {
    let mut t = Terminal::new(4, 20);
    t.advance(b"\x1b[?1004h"); // enable focus reporting
    assert!(t.focus_reporting());
    t.focus_report(true);
    assert_eq!(
        t.take_pty_response().as_slice(),
        b"\x1b[I",
        "focus-in emits CSI I"
    );
    t.focus_report(false);
    assert_eq!(
        t.take_pty_response().as_slice(),
        b"\x1b[O",
        "focus-out emits CSI O"
    );
}

#[test]
fn focus_report_stops_after_mode_reset() {
    let mut t = Terminal::new(4, 20);
    t.advance(b"\x1b[?1004h");
    t.focus_report(true);
    let _ = t.take_pty_response();
    t.advance(b"\x1b[?1004l"); // disable again
    t.focus_report(true);
    assert!(
        t.take_pty_response().is_empty(),
        "disabling ?1004 silences reports"
    );
}

// ---- C16: reflow / rewrap on resize ----

#[test]
fn reflow_narrowing_rewraps_without_losing_chars() {
    // A 12-char logical line on a 20-col grid (one physical row) re-wraps
    // onto a 10-col grid (two physical rows) without losing characters.
    let mut t = Terminal::new(4, 20);
    t.advance(b"abcdefghijkl"); // 12 chars, no wrap at 20 cols
    t.resize(4, 10);
    // Row 0 holds "abcdefghij", row 1 holds "kl".
    let row0: String = (0..10).map(|c| t.grid().cell(0, c).unwrap().c).collect();
    let row1: String = (0..2).map(|c| t.grid().cell(1, c).unwrap().c).collect();
    assert_eq!(row0, "abcdefghij");
    assert_eq!(row1, "kl");
}

#[test]
fn reflow_widening_rejoins_a_wrapped_line() {
    // Print 12 chars into a 5-col grid: it soft-wraps across rows. Widening
    // to 20 cols must rejoin the whole logical line onto one row.
    let mut t = Terminal::new(6, 5);
    t.advance(b"abcdefghijkl"); // wraps: abcde/fghij/kl
    t.resize(6, 20);
    let joined: String = (0..12).map(|c| t.grid().cell(0, c).unwrap().c).collect();
    assert_eq!(joined, "abcdefghijkl", "wrapped line rejoined on widen");
}

#[test]
fn reflow_never_merges_across_hard_newline() {
    // Two separate hard lines must stay separate across a reflow, even when
    // each is short enough that a naive join would merge them.
    let mut t = Terminal::new(6, 20);
    t.advance(b"foo\r\nbar");
    t.resize(6, 8);
    let row0: String = (0..3).map(|c| t.grid().cell(0, c).unwrap().c).collect();
    let row1: String = (0..3).map(|c| t.grid().cell(1, c).unwrap().c).collect();
    assert_eq!(row0, "foo");
    assert_eq!(row1, "bar", "hard newline preserved — not merged into foo");
}

#[test]
fn reflow_roundtrip_preserves_text() {
    // Narrow then widen back: the text content must survive intact.
    let mut t = Terminal::new(6, 20);
    t.advance(b"the quick brown fox"); // 19 chars, fits one row at 20
    t.resize(6, 7); // narrow — forces wrap
    t.resize(6, 20); // widen back
    let joined: String = (0..19).map(|c| t.grid().cell(0, c).unwrap().c).collect();
    assert_eq!(joined, "the quick brown fox", "narrow→widen round-trips");
}

#[test]
fn reflow_preserves_scrollback_lines() {
    // Lines pushed to scrollback survive a reflow (non-lossy preservation).
    let mut t = Terminal::with_scrollback(2, 8, 100);
    t.advance(b"L0\r\nL1\r\nL2\r\nL3"); // L0/L1 scroll into history
    let before = t.all_lines();
    assert!(before.iter().any(|l| l.starts_with("L0")));
    t.resize(2, 12);
    let after = t.all_lines();
    assert!(
        after.iter().any(|l| l.starts_with("L0")),
        "scrollback line L0 survives reflow"
    );
    assert!(after.iter().any(|l| l.starts_with("L3")));
}

#[test]
fn reflow_alt_screen_uses_plain_resize() {
    // On the alt screen, resize must NOT reflow (the TUI redraws itself);
    // it must remain a no-panic plain resize.
    let mut t = Terminal::new(4, 20);
    t.advance(b"\x1b[?1049h");
    t.advance(b"ALT");
    t.resize(6, 10);
    assert!(t.alt_screen_active());
    assert_eq!(t.grid().rows(), 6);
    assert_eq!(t.grid().cols(), 10);
}

#[test]
fn growing_grid_expands_full_screen_scroll_region() {
    // ROOT CAUSE of the blank-pane-on-split bug. A terminal spawned at 24
    // rows has a full-screen scroll region of `0..=23`. Growing the grid to
    // 38 rows must EXPAND the region to `0..=37` — a full-screen region stays
    // full-screen across a grow-resize. The pre-fix code tested
    // `scroll_bottom + 1 >= grid.rows()` AFTER the grid had already grown
    // (24 < 38 → mis-classified as a CUSTOM region → frozen at 23), which let
    // a shell's multi-line resize-redraw scroll all content out of the
    // restricted 0..=23 region, blanking the pane.
    let mut t = Terminal::new(24, 80);
    assert_eq!(
        t.scroll_region(),
        (0, 23),
        "fresh 24-row spawn is full-screen"
    );
    t.resize(38, 80);
    assert_eq!(
        t.scroll_region(),
        (0, 37),
        "growing the grid must expand the full-screen scroll region to the new height"
    );
    // And it must do so for a grow-AND-narrow in one step (the real split path).
    let mut t2 = Terminal::new(24, 80);
    t2.resize(38, 57);
    assert_eq!(t2.scroll_region(), (0, 37));
}

#[test]
fn growing_grid_then_full_redraw_keeps_content_visible() {
    // The end-to-end shape of the blank-pane-on-split bug, deterministic.
    // Spawn at 24 rows (default), grow to 38 rows + narrow, then apply a
    // shell-style full-screen redraw (cursor-home + one line per row, the
    // trailing rows blank `ESC[K\r\n`). With the scroll-region-grow fix the
    // content stays on screen; without it the 38-line redraw scrolls it all
    // out of the frozen 0..=23 region and the grid goes blank.
    let mut t = Terminal::new(24, 80);
    t.advance(b"line-one\r\nline-two\r\nC:\\Users\\x>");
    t.resize(38, 57);
    // A 38-row redraw: 3 content rows then blank ESC[K rows, each ESC[K\r\n
    // except the last (exactly as a Windows shell emits on resize).
    let mut redraw = String::from("\x1b[?25l\x1b[H");
    let content = ["line-one", "line-two", "C:\\Users\\x>"];
    for r in 0..38 {
        if r < content.len() {
            redraw.push_str(content[r]);
        }
        redraw.push_str("\x1b[K");
        if r < 37 {
            redraw.push_str("\r\n");
        }
    }
    redraw.push_str("\x1b[?25h");
    t.advance(redraw.as_bytes());
    let nonblank = t
        .grid()
        .to_text()
        .lines()
        .filter(|l| !l.trim().is_empty())
        .count();
    assert!(
        nonblank >= 3,
        "after a grow-resize + full redraw the grid must keep its content \
             visible (blank-pane-on-split regression), got {nonblank} nonblank rows:\n{}",
        t.grid().to_text()
    );
}

// ============================================================
// VT correctness P2/P3 batch (C20/C22/C25/C26/C27/C28/C30/C33/C34)
// ============================================================

use crate::grid::UnderlineStyle;

// ---- C20: styled underlines + underline color ----

#[test]
fn sgr_plain_underline_is_single() {
    let mut t = Terminal::new(2, 10);
    t.advance(b"\x1b[4mX");
    assert_eq!(
        t.grid().cell(0, 0).unwrap().flags.underline_style,
        UnderlineStyle::Single
    );
    assert!(t.grid().cell(0, 0).unwrap().flags.underline());
}

#[test]
fn sgr_colon_styled_underlines() {
    let cases: &[(&[u8], UnderlineStyle)] = &[
        (b"\x1b[4:0mX", UnderlineStyle::None),
        (b"\x1b[4:1mX", UnderlineStyle::Single),
        (b"\x1b[4:2mX", UnderlineStyle::Double),
        (b"\x1b[4:3mX", UnderlineStyle::Curly),
        (b"\x1b[4:4mX", UnderlineStyle::Dotted),
        (b"\x1b[4:5mX", UnderlineStyle::Dashed),
    ];
    for (seq, style) in cases {
        let mut t = Terminal::new(2, 10);
        t.advance(seq);
        assert_eq!(
            t.grid().cell(0, 0).unwrap().flags.underline_style,
            *style,
            "style for {seq:?}"
        );
    }
}

#[test]
fn sgr_double_underline_via_21() {
    let mut t = Terminal::new(2, 10);
    t.advance(b"\x1b[21mX");
    assert_eq!(
        t.grid().cell(0, 0).unwrap().flags.underline_style,
        UnderlineStyle::Double
    );
}

#[test]
fn sgr_24_resets_underline() {
    let mut t = Terminal::new(2, 10);
    t.advance(b"\x1b[4:3m\x1b[24mX");
    assert_eq!(
        t.grid().cell(0, 0).unwrap().flags.underline_style,
        UnderlineStyle::None
    );
}

#[test]
fn sgr_58_underline_color_indexed() {
    let mut t = Terminal::new(2, 10);
    t.advance(b"\x1b[4:3;58:5:9mX"); // curly + indexed underline color 9
    let cell = t.grid().cell(0, 0).unwrap();
    assert_eq!(cell.flags.underline_style, UnderlineStyle::Curly);
    assert_eq!(cell.underline_color, Some(Color::Indexed(9)));
}

#[test]
fn sgr_58_underline_color_rgb_colon_empty_colorspace() {
    let mut t = Terminal::new(2, 10);
    // `58:2::255:0:0` — note the empty colorspace slot between 2 and r.
    t.advance(b"\x1b[58:2::255:0:0mX");
    assert_eq!(
        t.grid().cell(0, 0).unwrap().underline_color,
        Some(Color::Rgb(255, 0, 0))
    );
}

#[test]
fn sgr_58_underline_color_rgb_semicolon_form() {
    let mut t = Terminal::new(2, 10);
    t.advance(b"\x1b[58;2;10;20;30mX");
    assert_eq!(
        t.grid().cell(0, 0).unwrap().underline_color,
        Some(Color::Rgb(10, 20, 30))
    );
}

#[test]
fn sgr_59_resets_underline_color() {
    let mut t = Terminal::new(2, 10);
    t.advance(b"\x1b[58:5:9m\x1b[59mX");
    assert_eq!(t.grid().cell(0, 0).unwrap().underline_color, None);
}

#[test]
fn sgr_extended_fg_color_still_works_after_refactor() {
    // Regression: the sgr() rewrite must not break 38;2 / 38;5.
    let mut t = Terminal::new(2, 10);
    t.advance(b"\x1b[38;5;200mA\x1b[38;2;1;2;3mB");
    assert_eq!(t.grid().cell(0, 0).unwrap().fg, Color::Indexed(200));
    assert_eq!(t.grid().cell(0, 1).unwrap().fg, Color::Rgb(1, 2, 3));
}

// ---- C22: REP (verify still green after refactor) ----

#[test]
fn rep_after_p2_changes() {
    let mut t = Terminal::new(2, 10);
    t.advance(b"q\x1b[2b"); // print q, repeat twice more
    let line: String = (0..3).map(|c| t.grid().cell(0, c).unwrap().c).collect();
    assert_eq!(line, "qqq");
}

// ---- C25: DECSCNM / IRM / DECOM ----

#[test]
fn decscnm_reverse_screen_flag() {
    let mut t = Terminal::new(2, 10);
    assert!(!t.reverse_screen());
    t.advance(b"\x1b[?5h");
    assert!(t.reverse_screen(), "?5h sets reverse-video screen");
    t.advance(b"\x1b[?5l");
    assert!(!t.reverse_screen());
}

#[test]
fn irm_insert_mode_shifts_line_right() {
    let mut t = Terminal::new(2, 6);
    t.advance(b"abcd\x1b[H"); // fill, home
    t.advance(b"\x1b[4h"); // enable IRM
    t.advance(b"XY"); // insert at col 0: XYabcd -> XYabcd (d pushed off)
    assert!(t.insert_mode());
    let line: String = (0..6).map(|c| t.grid().cell(0, c).unwrap().c).collect();
    assert_eq!(line, "XYabcd");
}

#[test]
fn irm_reset_returns_to_overwrite() {
    let mut t = Terminal::new(2, 6);
    t.advance(b"abcd\x1b[H\x1b[4h\x1b[4l"); // set then reset IRM
    assert!(!t.insert_mode());
    t.advance(b"X"); // overwrite, not insert
    let line: String = (0..4).map(|c| t.grid().cell(0, c).unwrap().c).collect();
    assert_eq!(line, "Xbcd");
}

#[test]
fn decom_origin_mode_relative_addressing() {
    let mut t = Terminal::new(6, 10);
    t.advance(b"\x1b[2;4r"); // scroll region rows 2..4 (0-based 1..3)
    t.advance(b"\x1b[?6h"); // enable origin mode (homes to top margin)
    assert!(t.origin_mode());
    // CUP row 1 with origin mode -> absolute row = scroll_top (1).
    t.advance(b"\x1b[1;1H");
    assert_eq!(
        t.cursor_position(),
        Some((1, 0)),
        "row 1 maps to top margin"
    );
    // CUP row 2 -> scroll_top + 1 = row 2.
    t.advance(b"\x1b[2;1H");
    assert_eq!(t.cursor_position(), Some((2, 0)));
    // Past the bottom margin clamps to scroll_bottom (3).
    t.advance(b"\x1b[99;1H");
    assert_eq!(
        t.cursor_position(),
        Some((3, 0)),
        "clamped to bottom margin"
    );
}

#[test]
fn decom_off_uses_absolute_addressing() {
    let mut t = Terminal::new(6, 10);
    t.advance(b"\x1b[2;4r"); // region set
    t.advance(b"\x1b[1;1H"); // origin mode OFF -> absolute row 0
    assert_eq!(t.cursor_position(), Some((0, 0)));
}

// ---- C26: OSC 9;4 progress ----

#[test]
fn osc9_4_progress_normal() {
    let mut t = Terminal::new(2, 10);
    t.advance(b"\x1b]9;4;1;42\x07");
    let p = t.take_progress();
    assert_eq!(p.len(), 1);
    assert_eq!(p[0].state, ProgressState::Normal);
    assert_eq!(p[0].percent, 42);
    assert!(t.take_progress().is_empty(), "drained once");
}

#[test]
fn osc9_4_progress_states() {
    let mut t = Terminal::new(2, 10);
    t.advance(b"\x1b]9;4;0;0\x07"); // remove
    t.advance(b"\x1b]9;4;2;99\x07"); // error
    t.advance(b"\x1b]9;4;3;50\x07"); // indeterminate (percent ignored)
    t.advance(b"\x1b]9;4;4;75\x07"); // warning
    let p = t.take_progress();
    assert_eq!(p[0].state, ProgressState::Remove);
    assert_eq!(p[1].state, ProgressState::Error);
    assert_eq!(p[1].percent, 99);
    assert_eq!(p[2].state, ProgressState::Indeterminate);
    assert_eq!(p[2].percent, 0, "indeterminate ignores percent");
    assert_eq!(p[3].state, ProgressState::Warning);
    assert_eq!(p[3].percent, 75);
}

#[test]
fn osc9_4_clamps_percent() {
    let mut t = Terminal::new(2, 10);
    t.advance(b"\x1b]9;4;1;250\x07");
    assert_eq!(t.take_progress()[0].percent, 100, "percent clamps to 100");
}

#[test]
fn osc9_plain_notification_not_progress() {
    // OSC 9 without the ;4 sub-code is still a notification.
    let mut t = Terminal::new(2, 10);
    t.advance(b"\x1b]9;Hello\x07");
    assert!(t.take_progress().is_empty());
    assert_eq!(t.take_notification().unwrap().body, "Hello");
}

// ---- C27 / C34: combining marks + variation selectors ----

#[test]
fn combining_mark_attaches_to_previous_cell() {
    let mut t = Terminal::new(2, 10);
    t.advance("e\u{0301}".as_bytes()); // e + combining acute
    let cell = t.grid().cell(0, 0).unwrap();
    assert_eq!(cell.c, 'e');
    assert_eq!(t.grid().grapheme_at(0, 0), "e\u{0301}");
    // The combining mark did NOT advance the cursor into col 1.
    assert_eq!(t.cursor_position(), Some((0, 1)));
    assert_eq!(t.grid().cell(0, 1).unwrap().c, ' ', "no own cell for mark");
}

#[test]
fn multiple_combining_marks_stack() {
    let mut t = Terminal::new(2, 10);
    t.advance("a\u{0301}\u{0302}".as_bytes());
    assert_eq!(t.grid().grapheme_at(0, 0), "a\u{0301}\u{0302}");
}

#[test]
fn variation_selector_attaches_zero_width() {
    let mut t = Terminal::new(2, 10);
    // heart + VS16 (emoji presentation) — VS16 is zero-width, attaches.
    t.advance("\u{2764}\u{FE0F}".as_bytes());
    let cell = t.grid().cell(0, 0).unwrap();
    assert_eq!(cell.c, '\u{2764}');
    assert_eq!(t.grid().grapheme_at(0, 0), "\u{2764}\u{FE0F}");
    assert_eq!(
        t.cursor_position(),
        Some((0, 1)),
        "VS16 did not advance cursor"
    );
}

#[test]
fn combining_mark_attaches_to_wide_glyph_base() {
    let mut t = Terminal::new(2, 10);
    // Wide CJK glyph occupies cols 0+1; a following combining mark must
    // attach to the BASE (col 0), not the continuation spacer (col 1).
    t.advance("世\u{0301}".as_bytes());
    assert_eq!(t.grid().grapheme_at(0, 0), "世\u{0301}");
    assert_eq!(t.cursor_position(), Some((0, 2)));
}

#[test]
fn leading_combining_mark_lands_in_cell() {
    // A combining mark with no preceding cell (col 0) is given a cell so it
    // is not silently lost.
    let mut t = Terminal::new(2, 10);
    t.advance("\u{0301}".as_bytes());
    assert_eq!(
        t.cursor_position(),
        Some((0, 1)),
        "leading mark occupies a cell"
    );
}

// ---- C28: OSC 133 C/D command marks ----

#[test]
fn osc133_command_output_start_mark() {
    let mut t = Terminal::new(4, 20);
    t.advance(b"\x1b]133;C\x07");
    let marks = t.command_marks();
    assert_eq!(marks.len(), 1);
    assert!(matches!(marks[0].kind, CommandMarkKind::OutputStart));
}

#[test]
fn osc133_command_end_with_exit_code() {
    let mut t = Terminal::new(4, 20);
    t.advance(b"\x1b]133;D;0\x07"); // success
    t.advance(b"\x1b]133;D;127\x07"); // failure
    let marks = t.command_marks();
    assert_eq!(marks.len(), 2);
    assert!(matches!(
        marks[0].kind,
        CommandMarkKind::CommandEnd { exit_code: Some(0) }
    ));
    assert!(matches!(
        marks[1].kind,
        CommandMarkKind::CommandEnd {
            exit_code: Some(127)
        }
    ));
}

#[test]
fn osc133_command_end_without_exit_code() {
    let mut t = Terminal::new(4, 20);
    t.advance(b"\x1b]133;D\x07");
    assert!(matches!(
        t.command_marks()[0].kind,
        CommandMarkKind::CommandEnd { exit_code: None }
    ));
}

#[test]
fn osc133_cd_never_writes_pty_response() {
    let mut t = Terminal::new(4, 20);
    t.advance(b"\x1b]133;C\x07\x1b]133;D;0\x07");
    assert!(
        t.take_pty_response().is_empty(),
        "OSC 133 C/D stay capture-only (anti-CVE)"
    );
}

#[test]
fn osc133_a_still_records_prompt_mark_not_command_mark() {
    let mut t = Terminal::new(4, 20);
    t.advance(b"\x1b]133;A\x07");
    assert_eq!(t.prompt_marks().len(), 1);
    assert_eq!(
        t.command_marks().len(),
        0,
        "A is a prompt mark, not command"
    );
}

#[test]
fn last_command_exit_code_none_when_no_command_finished() {
    // A fresh terminal (or one that has only seen a `C` output-start mark)
    // has no finished command — the accessor reports None so the status
    // bar shows no indicator.
    let mut t = Terminal::new(4, 20);
    assert_eq!(t.last_command_exit_code(), None);
    t.advance(b"\x1b]133;C\x07"); // output start only, no `D`
    assert_eq!(
        t.last_command_exit_code(),
        None,
        "a C (output-start) mark is not a finished command"
    );
}

#[test]
fn last_command_exit_code_reports_latest_success_then_failure() {
    let mut t = Terminal::new(4, 20);
    t.advance(b"\x1b]133;D;0\x07"); // first command: success
    assert_eq!(t.last_command_exit_code(), Some(Some(0)));
    t.advance(b"\x1b]133;D;127\x07"); // second command: failure 127
    assert_eq!(
        t.last_command_exit_code(),
        Some(Some(127)),
        "the accessor must report the MOST RECENT command-end mark"
    );
}

#[test]
fn last_command_exit_code_some_none_when_code_absent() {
    // `OSC 133 ; D` with no third field: the command finished but the shell
    // did not report a code — Some(None), distinct from the no-command None.
    let mut t = Terminal::new(4, 20);
    t.advance(b"\x1b]133;D\x07");
    assert_eq!(t.last_command_exit_code(), Some(None));
}

#[test]
fn last_command_exit_code_ignores_trailing_output_start() {
    // After a finished command (`D`), a new command's output begins (`C`)
    // before it ends. The accessor must still report the LAST FINISHED
    // command's code, ignoring the dangling `C`.
    let mut t = Terminal::new(4, 20);
    t.advance(b"\x1b]133;D;0\x07"); // command 1 finished, success
    t.advance(b"\x1b]133;C\x07"); // command 2 output started, not finished
    assert_eq!(
        t.last_command_exit_code(),
        Some(Some(0)),
        "a dangling C must not mask the previous finished command's code"
    );
}

// ---- C30: XTGETTCAP ----

#[test]
fn xtgettcap_replies_to_colors_capability() {
    let mut t = Terminal::new(4, 20);
    // "Co" hex = 436F. Query DCS + q 436F ST.
    t.advance(b"\x1bP+q436F\x1b\\");
    let resp = t.take_pty_response();
    // Valid reply form: DCS 1 + r 436F = <hex of "256"> ST.
    // "256" hex = 323536.
    assert_eq!(resp.as_slice(), b"\x1bP1+r436F=323536\x1b\\");
}

#[test]
fn xtgettcap_unknown_capability_invalid_reply() {
    let mut t = Terminal::new(4, 20);
    // "ZZ" hex = 5A5A — not a capability we report.
    t.advance(b"\x1bP+q5A5A\x1b\\");
    let resp = t.take_pty_response();
    // Invalid form: DCS 0 + r 5A5A ST.
    assert_eq!(resp.as_slice(), b"\x1bP0+r5A5A\x1b\\");
}

#[test]
fn xtgettcap_does_not_disturb_sixel() {
    // A plain DCS q (Sixel, no '+') must still go to the image path, not
    // XTGETTCAP — regression guard for the hook disambiguation.
    let mut t = Terminal::new(4, 20);
    t.advance(b"\x1bPq#0;2;100;0;0~\x1b\\");
    assert_eq!(t.images().len(), 1);
    assert!(
        t.take_pty_response().is_empty(),
        "sixel emits no XTGETTCAP reply"
    );
}

// ---- C33: DECRQM ----

#[test]
fn decrqm_reports_set_private_mode() {
    let mut t = Terminal::new(4, 20);
    t.advance(b"\x1b[?2004h"); // enable bracketed paste
    t.advance(b"\x1b[?2004$p"); // DECRQM query
                                // Reply: CSI ? 2004 ; 1 $ y  (1 = set).
    assert_eq!(t.take_pty_response().as_slice(), b"\x1b[?2004;1$y");
}

#[test]
fn decrqm_reports_reset_private_mode() {
    let mut t = Terminal::new(4, 20);
    t.advance(b"\x1b[?2004$p"); // never enabled -> reset (2)
    assert_eq!(t.take_pty_response().as_slice(), b"\x1b[?2004;2$y");
}

#[test]
fn decrqm_reports_unrecognised_mode_zero() {
    let mut t = Terminal::new(4, 20);
    t.advance(b"\x1b[?9999$p");
    assert_eq!(t.take_pty_response().as_slice(), b"\x1b[?9999;0$y");
}

#[test]
fn decrqm_reports_ansi_irm() {
    let mut t = Terminal::new(4, 20);
    t.advance(b"\x1b[4h"); // IRM on
    t.advance(b"\x1b[4$p"); // ANSI DECRQM (no '?')
    assert_eq!(t.take_pty_response().as_slice(), b"\x1b[4;1$y");
}

#[test]
fn p2p3_modes_reset_on_ris() {
    let mut t = Terminal::new(4, 20);
    t.advance(b"\x1b[?5h\x1b[4h\x1b[?6h"); // reverse + insert + origin
    t.advance(b"\x1bc"); // RIS
    assert!(!t.reverse_screen());
    assert!(!t.insert_mode());
    assert!(!t.origin_mode());
}

// ---- VT parser memchr fast-path equivalence ----

/// The `memchr` ESC-scan fast path (and the bulk run-skip in the APC
/// pre-filter) must be byte-for-byte behaviour-identical to feeding the same
/// stream one byte at a time. We build a stream with LONG runs of plain
/// printable ASCII interleaved with real escape sequences (SGR colour, cursor
/// moves, an OSC title, and a Kitty APC) and assert the resulting grid text,
/// cursor position, and PTY response are identical whether the bytes arrive
/// in one `advance()` call (fast path + bulk skip) or one byte per call
/// (scalar path, ESC never bulk-skipped).
#[test]
fn memchr_fast_path_matches_byte_at_a_time() {
    // A long plain run, an SGR colour change, more plain text, a CUP, an OSC
    // title set, a Kitty graphics APC (filtered), and a trailing plain run.
    let plain_a = "abcdefghijklmnopqrstuvwxyz0123456789".repeat(4);
    let plain_b = "the quick brown fox jumps over the lazy dog".repeat(3);
    let mut stream: Vec<u8> = Vec::new();
    stream.extend_from_slice(plain_a.as_bytes());
    stream.extend_from_slice(b"\x1b[31m"); // SGR red
    stream.extend_from_slice(plain_b.as_bytes());
    stream.extend_from_slice(b"\x1b[2;3H"); // CUP row 2 col 3
    stream.extend_from_slice(b"\x1b]0;a title\x07"); // OSC 0 title
    stream.extend_from_slice(b"\x1b_Gf=24,s=1,v=1;AAA\x1b\\"); // Kitty APC
    stream.extend_from_slice(b"tail-run-plain-text"); // trailing plain run

    // (a) whole chunk: exercises the memchr fast-path gate + bulk run-skip.
    let mut whole = Terminal::new(8, 40);
    whole.advance(&stream);

    // (b) one byte per advance(): ESC can never be bulk-skipped; each byte
    // walks the state machine individually.
    let mut split = Terminal::new(8, 40);
    for &b in &stream {
        split.advance(&[b]);
    }

    assert_eq!(
        whole.grid().to_text(),
        split.grid().to_text(),
        "grid text must match between whole-chunk and byte-at-a-time parsing"
    );
    assert_eq!(
        whole.cursor_position(),
        split.cursor_position(),
        "cursor position must match"
    );
    assert_eq!(whole.title(), split.title(), "OSC title must match");
    assert_eq!(
        whole.images().len(),
        split.images().len(),
        "Kitty APC image count must match"
    );
}

/// A pure plain-ASCII chunk with NO escape byte must take the fast path and
/// land verbatim on the grid (the overwhelmingly common bulk-output case).
#[test]
fn memchr_fast_path_pure_plain_run() {
    let mut t = Terminal::new(4, 80);
    let run = "plain text with no escapes whatsoever 1234567890";
    t.advance(run.as_bytes());
    assert!(
        t.grid().to_text().contains(run),
        "a pure plain run must reach the grid unchanged"
    );
}

// ---- kitty keyboard protocol (CSI u progressive enhancement) ----

#[test]
fn kitty_flags_default_to_zero() {
    let t = Terminal::new(4, 20);
    assert_eq!(t.kitty_keyboard_flags(), 0);
}

#[test]
fn kitty_push_sets_current_flags() {
    let mut t = Terminal::new(4, 20);
    t.advance(b"\x1b[>5u"); // push flags=5 (disambiguate | alternate-keys)
    assert_eq!(t.kitty_keyboard_flags(), 5);
}

#[test]
fn kitty_push_defaults_to_zero_flags() {
    let mut t = Terminal::new(4, 20);
    t.advance(b"\x1b[>u"); // push with no param → flags 0
    assert_eq!(t.kitty_keyboard_flags(), 0);
    // But it DID push an entry — popping reveals the empty stack.
    t.advance(b"\x1b[>9u");
    assert_eq!(t.kitty_keyboard_flags(), 9);
    t.advance(b"\x1b[<u"); // pop 1 → back to the flags-0 entry
    assert_eq!(t.kitty_keyboard_flags(), 0);
}

#[test]
fn kitty_pop_restores_previous() {
    let mut t = Terminal::new(4, 20);
    t.advance(b"\x1b[>1u");
    t.advance(b"\x1b[>3u");
    assert_eq!(t.kitty_keyboard_flags(), 3);
    t.advance(b"\x1b[<u"); // pop 1 (default)
    assert_eq!(t.kitty_keyboard_flags(), 1);
    t.advance(b"\x1b[<5u"); // pop more than remaining → saturates to empty
    assert_eq!(t.kitty_keyboard_flags(), 0);
}

#[test]
fn kitty_set_mode_replace_or_clear() {
    let mut t = Terminal::new(4, 20);
    // Set on an empty stack pushes the result (mode 1 replace, flags=5).
    t.advance(b"\x1b[=5;1u");
    assert_eq!(t.kitty_keyboard_flags(), 5);
    // Mode 2 = OR in bit2.
    t.advance(b"\x1b[=2;2u");
    assert_eq!(t.kitty_keyboard_flags(), 7);
    // Mode 3 = clear bit1.
    t.advance(b"\x1b[=1;3u");
    assert_eq!(t.kitty_keyboard_flags(), 6);
    // Mode defaults to 1 (replace) when omitted.
    t.advance(b"\x1b[=8u");
    assert_eq!(t.kitty_keyboard_flags(), 8);
}

#[test]
fn kitty_query_replies_with_current_flags() {
    let mut t = Terminal::new(4, 20);
    t.advance(b"\x1b[>13u");
    t.advance(b"\x1b[?u"); // query
    assert_eq!(t.take_pty_response(), b"\x1b[?13u".to_vec());
}

#[test]
fn kitty_stack_depth_is_capped() {
    let mut t = Terminal::new(4, 20);
    // Push the cap + extra; the oldest is dropped, never grows unbounded.
    for i in 0..(KITTY_KBD_STACK_MAX + 5) {
        let seq = format!("\x1b[>{}u", (i % 7) + 1);
        t.advance(seq.as_bytes());
    }
    assert_eq!(t.screen.kitty_kbd_stack.len(), KITTY_KBD_STACK_MAX);
}

#[test]
fn bare_csi_u_is_still_scorc_cursor_restore() {
    // The bare `CSI u` (no intermediate) MUST remain the ANSI.SYS SCORC
    // cursor-restore alias and NOT be hijacked by the kitty handler.
    let mut t = Terminal::new(8, 40);
    t.advance(b"\x1b[3;5H"); // move cursor to row 3, col 5 (1-based)
    t.advance(b"\x1b[s"); // SCOSC — save cursor
    t.advance(b"\x1b[1;1H"); // move to home
    assert_eq!(t.cursor_position(), Some((0, 0)));
    t.advance(b"\x1b[u"); // bare CSI u → SCORC restore
    assert_eq!(t.cursor_position(), Some((2, 4)));
    // And it left the kitty stack untouched + emitted NO query reply.
    assert_eq!(t.kitty_keyboard_flags(), 0);
    assert!(t.take_pty_response().is_empty());
}
