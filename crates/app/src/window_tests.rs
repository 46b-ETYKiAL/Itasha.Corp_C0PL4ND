//! Unit tests for the legacy winit window, split out of `window.rs`
//! (F6-1 decomposition) — pure structural move, no test changes.

use super::*;

// --- Modern tab-strip + settings click routing (the user-facing clicks) ---

#[test]
fn tab_zone_at_routes_clicks_to_the_right_target() {
    // Zones modelled on the real glyph layout (the on-device DIAG measured
    // Tab(0)=96-121px, '+'=121-162px at the test size).
    let zones = vec![
        (TabZone::Tab(0), 96.0_f32, 121.0),
        (TabZone::Tab(1), 121.0, 146.0),
        (TabZone::NewTab, 150.0, 191.0),
        (TabZone::Settings, 195.0, 233.0),
    ];
    assert_eq!(tab_zone_at(100.0, &zones), Some(TabZone::Tab(0)));
    assert_eq!(tab_zone_at(130.0, &zones), Some(TabZone::Tab(1)));
    assert_eq!(tab_zone_at(170.0, &zones), Some(TabZone::NewTab));
    assert_eq!(tab_zone_at(210.0, &zones), Some(TabZone::Settings));
    // x0 inclusive, x1 exclusive.
    assert_eq!(tab_zone_at(121.0, &zones), Some(TabZone::Tab(1)));
    assert_eq!(tab_zone_at(120.999, &zones), Some(TabZone::Tab(0)));
    // Gaps between zones and outside the strip resolve to nothing.
    assert_eq!(tab_zone_at(148.0, &zones), None);
    assert_eq!(tab_zone_at(50.0, &zones), None);
    assert_eq!(tab_zone_at(300.0, &zones), None);
    // Empty zones (e.g. search mode) -> nothing clickable, never panics.
    assert_eq!(tab_zone_at(100.0, &[]), None);
}

#[test]
fn settings_row_index_maps_clicks_to_every_row() {
    let w = 900.0_f64;
    let panel_left = (w * 0.25).max(40.0);
    let panel_top = TITLEBAR_H as f64 + 40.0;
    let lh = LINE_HEIGHT as f64;
    let first_row_top = panel_top + 2.0 * lh; // 2-line header precedes rows
                                              // Every settings row is reachable by a click at its line.
    for i in 0..SettingRow::ALL.len() {
        let y = first_row_top + i as f64 * lh + 1.0;
        assert_eq!(
            settings_row_index(panel_left + 20.0, y, w),
            Some(i),
            "click on row {i} should select row {i}"
        );
    }
    // The header band is not a row.
    assert_eq!(
        settings_row_index(panel_left + 20.0, panel_top + 1.0, w),
        None
    );
    // Below the last row is not a row.
    let below = first_row_top + (SettingRow::ALL.len() as f64) * lh + 1.0;
    assert_eq!(settings_row_index(panel_left + 20.0, below, w), None);
    // Clicks left of / far right of the panel miss it (so they can dismiss it).
    assert_eq!(
        settings_row_index(panel_left - 50.0, first_row_top + 1.0, w),
        None
    );
    assert_eq!(
        settings_row_index(panel_left + 500.0, first_row_top + 1.0, w),
        None
    );
}

#[test]
fn settings_panel_left_edge_clamps_on_narrow_windows() {
    // On a very narrow window the panel clamps to x=40, not w*0.25.
    let narrow = 100.0_f64;
    let y = TITLEBAR_H as f64 + 40.0 + 2.0 * LINE_HEIGHT as f64 + 1.0;
    assert_eq!(settings_row_index(40.0, y, narrow), Some(0));
    assert_eq!(settings_row_index(20.0, y, narrow), None);
}

#[test]
fn resolve_tab_number_handles_one_based_and_nine_is_last() {
    // Ctrl+1..5 -> tabs 0..4; Ctrl+9 always = last; out-of-range = None.
    assert_eq!(resolve_tab_number(1, 5), Some(0));
    assert_eq!(resolve_tab_number(3, 5), Some(2));
    assert_eq!(resolve_tab_number(5, 5), Some(4));
    assert_eq!(resolve_tab_number(9, 5), Some(4));
    assert_eq!(resolve_tab_number(9, 10), Some(9));
    assert_eq!(resolve_tab_number(6, 5), None);
    assert_eq!(resolve_tab_number(1, 0), None);
}

#[test]
fn choose_alpha_mode_prefers_transparency_then_falls_back() {
    use wgpu::CompositeAlphaMode::*;
    // Default (no transparency wanted): the native/first mode, untouched.
    assert_eq!(choose_alpha_mode(false, &[Opaque, PostMultiplied]), Opaque);
    // Transparency wanted: PostMultiplied preferred (no premultiply needed).
    assert_eq!(
        choose_alpha_mode(true, &[Opaque, PostMultiplied, PreMultiplied]),
        PostMultiplied
    );
    // PostMultiplied unavailable -> PreMultiplied.
    assert_eq!(
        choose_alpha_mode(true, &[Opaque, PreMultiplied]),
        PreMultiplied
    );
    // Only Inherit available alongside Opaque (e.g. the Intel/Vulkan
    // surface) -> Inherit is preferred so a transparent window composites.
    assert_eq!(choose_alpha_mode(true, &[Opaque, Inherit]), Inherit);
    // No transparency-capable mode at all -> graceful fallback to native.
    assert_eq!(choose_alpha_mode(true, &[Opaque]), Opaque);
    assert_eq!(choose_alpha_mode(true, &[]), Opaque);
}

#[test]
fn caption_glyph_left_centres_each_button_in_its_cell() {
    let cluster = 1000.0_f32;
    let cell = BUTTON_CELLS * CELL_W; // 45.0
                                      // A 9px-advance glyph in a 45px cell -> 18px of slack, 9px each side.
    assert_eq!(caption_glyph_left(cluster, 0, cell, 9.0), 1000.0 + 18.0);
    assert_eq!(
        caption_glyph_left(cluster, 1, cell, 9.0),
        1000.0 + cell + 18.0
    );
    assert_eq!(
        caption_glyph_left(cluster, 2, cell, 9.0),
        1000.0 + 2.0 * cell + 18.0
    );
    // A WIDER symbol-fallback glyph (e.g. ✕ at 14px) still centres: less
    // slack, but symmetric — this is exactly what space-padding could not do.
    let left_wide = caption_glyph_left(cluster, 2, cell, 14.0);
    let slot_x = cluster + 2.0 * cell;
    assert!((left_wide - (slot_x + (cell - 14.0) / 2.0)).abs() < 1e-3);
    // Glyph wider than its cell clamps to the slot's left edge (never negative).
    assert_eq!(caption_glyph_left(cluster, 0, cell, 60.0), cluster);
}

#[test]
fn strip_hidden_for_single_tab_cell() {
    // A 1-tab cell never lays out a strip, regardless of size.
    let tall = LRect::new(0, 0, 400, 600);
    assert!(!strip_laid_out(1, tall));
    assert!(!strip_laid_out(0, tall));
}

#[test]
fn strip_laid_out_when_tall_enough_and_multi_tab() {
    let tall = LRect::new(0, 0, 400, CELL_STRIP_MIN_H + 2 * BORDER_PX + 5);
    assert!(
        strip_laid_out(2, tall),
        "tall multi-tab cell lays out a strip"
    );
}

#[test]
fn strip_not_laid_out_on_short_cell() {
    // Below the min height the strip auto-hides (it can still hover-reveal as
    // an overlay, but is NOT laid out — grid geometry is unchanged).
    let short = LRect::new(0, 0, 400, CELL_TABBAR_H as i32 + 2 * BORDER_PX);
    assert!(!strip_laid_out(3, short));
}

#[test]
fn font_scale_cell_dims_are_consistent() {
    // Scale 1.0 is a no-op: the un-zoomed layout is byte-identical to before.
    assert_eq!(scaled_cell_w(1.0), CELL_W);
    assert_eq!(scaled_cell_h(1.0), LINE_HEIGHT);
    assert_eq!(scaled_cell_w(2.0), CELL_W * 2.0);
    // The load-bearing property: render (col → pixel) and hit-test
    // (pixel → col) use the SAME scaled cell width, so a round-trip is
    // stable at every zoom — clicks always land on the rendered glyph.
    for scale in [0.5f32, 1.0, 1.5, 2.0, 3.0] {
        let cw = scaled_cell_w(scale);
        for col in [0usize, 1, 7, 42, 199] {
            let px = col as f32 * cw; // render
            let back = (px / cw).floor() as usize; // hit-test
            assert_eq!(back, col, "round-trip broke at scale {scale}, col {col}");
        }
    }
}

#[test]
fn key_encodes_function_and_edit_keys() {
    let none = ModifiersState::empty();
    assert_eq!(
        key_to_bytes(&Key::Named(NamedKey::F1), &None, false, none),
        Some(b"\x1bOP".to_vec())
    );
    assert_eq!(
        key_to_bytes(&Key::Named(NamedKey::F5), &None, false, none),
        Some(b"\x1b[15~".to_vec())
    );
    assert_eq!(
        key_to_bytes(&Key::Named(NamedKey::Delete), &None, false, none),
        Some(b"\x1b[3~".to_vec())
    );
    assert_eq!(
        key_to_bytes(&Key::Named(NamedKey::PageUp), &None, false, none),
        Some(b"\x1b[5~".to_vec())
    );
}

#[test]
fn key_arrows_honour_decckm() {
    let none = ModifiersState::empty();
    // Normal mode → CSI.
    assert_eq!(
        key_to_bytes(&Key::Named(NamedKey::ArrowUp), &None, false, none),
        Some(b"\x1b[A".to_vec())
    );
    // Application-cursor mode (DECCKM) → SS3.
    assert_eq!(
        key_to_bytes(&Key::Named(NamedKey::ArrowUp), &None, true, none),
        Some(b"\x1bOA".to_vec())
    );
}

#[test]
fn key_alt_is_meta_prefix() {
    let alt = ModifiersState::ALT;
    // Alt+b → ESC b (Meta) via the text fallback.
    let text = Some(winit::keyboard::SmolStr::new("b"));
    assert_eq!(
        key_to_bytes(&Key::Character("b".into()), &text, false, alt),
        Some(b"\x1bb".to_vec())
    );
    // Alt+Up must NOT double-prefix ESC (the arrow already starts with ESC).
    assert_eq!(
        key_to_bytes(&Key::Named(NamedKey::ArrowUp), &None, false, alt),
        Some(b"\x1b[A".to_vec())
    );
}

#[test]
fn selection_ordered_normalizes_direction() {
    // anchor after head on screen → ordered swaps them.
    let s = Selection {
        leaf: LeafId(0),
        anchor: (3, 5),
        head: (1, 2),
        active: false,
    };
    assert_eq!(s.ordered(), ((1, 2), (3, 5)));
    // same row, head right of anchor → unchanged.
    let s2 = Selection {
        leaf: LeafId(0),
        anchor: (1, 2),
        head: (1, 8),
        active: false,
    };
    assert_eq!(s2.ordered(), ((1, 2), (1, 8)));
}

#[test]
fn dropped_path_plain_is_raw_with_trailing_space() {
    let out = format_dropped_path(std::path::Path::new("/home/user/file.txt"));
    assert_eq!(out, "/home/user/file.txt ");
}

#[test]
fn dropped_path_with_space_is_double_quoted() {
    let out = format_dropped_path(std::path::Path::new(r"C:\My Files\note.txt"));
    assert_eq!(out, "\"C:\\My Files\\note.txt\" ");
}

#[test]
fn dropped_path_never_contains_newline() {
    // A dropped file must never be executed — no newline is ever emitted.
    let out = format_dropped_path(std::path::Path::new("/tmp/x;rm -rf ~"));
    assert!(!out.contains('\n') && !out.contains('\r'));
    // Shell-significant chars force quoting so the literal can't break out.
    assert!(out.starts_with('"') && out.ends_with("\" "));
}

#[test]
fn find_url_accepts_http_but_rejects_file_scheme() {
    let chars = |s: &str| s.chars().collect::<Vec<char>>();
    // http(s) URLs are detected from the click column.
    let c = chars("see https://example.com/x now");
    assert_eq!(
        find_url_in_line(&c, 6),
        Some("https://example.com/x".to_string()),
        "https URLs are clickable"
    );
    // SECURITY (the bug this fixes): a file:// URL printed by an
    // attacker-controlled program must NOT be auto-detected — ctrl-clicking it
    // would feed it to open_path (`cmd /C start`) and launch an executable.
    assert_eq!(
        find_url_in_line(&chars("file://attacker/share/evil.exe"), 5),
        None,
        "remote file:// (UNC) is never a clickable URL"
    );
    assert_eq!(
        find_url_in_line(&chars("file:///C:/Windows/System32/calc.exe"), 5),
        None,
        "local file:// is rejected too"
    );
}

#[test]
fn find_url_stops_at_shell_metacharacters() {
    let chars = |s: &str| s.chars().collect::<Vec<char>>();
    // SECURITY: the token is bounded to the RFC URL charset, so a cmd
    // metacharacter an attacker-controlled program prints adjacent to a URL
    // terminates the token and is NEVER carried into the `cmd /C start` opener.
    assert_eq!(
        find_url_in_line(&chars("http://x.io/a|whoami"), 8),
        Some("http://x.io/a".to_string()),
        "pipe terminates the URL token"
    );
    assert_eq!(
        find_url_in_line(&chars("http://x.io/a^b"), 8),
        Some("http://x.io/a".to_string()),
        "caret terminates the URL token"
    );
    assert_eq!(
        find_url_in_line(&chars("http://x.io/a`calc`"), 8),
        Some("http://x.io/a".to_string()),
        "backtick terminates the URL token"
    );
    // '&' IS a legitimate URL query separator and stays part of the token.
    assert_eq!(
        find_url_in_line(&chars("http://x.io/a?b=1&c=2"), 8),
        Some("http://x.io/a?b=1&c=2".to_string()),
        "ampersand is a valid query char and is kept"
    );
}

#[test]
fn bidi_ascii_row_skips_reorder() {
    let c = GColor::rgb(200, 200, 200);
    let row: Vec<(char, GColor)> = "hello -> world".chars().map(|ch| (ch, c)).collect();
    assert!(
        App::bidi_visual_order(&row).is_none(),
        "an all-ASCII row takes the zero-alloc fast path"
    );
}

#[test]
fn bidi_pure_rtl_row_reverses_to_visual_order() {
    let c = GColor::rgb(200, 200, 200);
    // Hebrew "אבג" (alef, bet, gimel) in logical (storage) order.
    let row: Vec<(char, GColor)> = "אבג".chars().map(|ch| (ch, c)).collect();
    let visual = App::bidi_visual_order(&row).expect("an RTL row reorders");
    let chars: String = visual.iter().map(|(ch, _)| *ch).collect();
    assert_eq!(
        chars, "גבא",
        "RTL text displays right-to-left (visually reversed)"
    );
}

#[test]
fn bidi_preserves_per_char_color_through_reorder() {
    let red = GColor::rgb(255, 0, 0);
    let blue = GColor::rgb(0, 0, 255);
    // Logical: alef=red, bet=blue, gimel=red.
    let row = vec![('א', red), ('ב', blue), ('ג', red)];
    let visual = App::bidi_visual_order(&row).expect("an RTL row reorders");
    let chars: String = visual.iter().map(|(ch, _)| *ch).collect();
    assert_eq!(chars, "גבא");
    assert!(
        visual[0].1 == red && visual[1].1 == blue && visual[2].1 == red,
        "each glyph keeps its own colour through the visual reorder"
    );
}

#[test]
fn bidi_non_rtl_unicode_row_skips_reorder() {
    // Accented Latin / CJK is non-ASCII but not RTL — must not reorder.
    let c = GColor::rgb(10, 20, 30);
    let row: Vec<(char, GColor)> = "café 日本語".chars().map(|ch| (ch, c)).collect();
    assert!(
        App::bidi_visual_order(&row).is_none(),
        "non-RTL Unicode keeps logical order"
    );
}
