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

// =========================================================================
// GPU-less `App` surface.
//
// `App::new` sets `gpu: None`, and every method below either never touches the
// GPU or reads it through an `Option`, so the real shipped code runs here with
// no window, event loop, or adapter. See the module docs in `window.rs` for the
// functions that genuinely cannot be reached this way (`render`, `window_event`,
// `resumed`).
// =========================================================================

/// A GPU-less `App` on the default config (`gpu: None`, no tabs, no sessions).
fn test_app() -> App {
    App::new(Config::default())
}

// --- Window geometry ------------------------------------------------------

#[test]
fn content_rect_without_a_surface_derives_from_the_configured_grid() {
    let mut app = test_app();
    app.config.window.cols = 80;
    app.config.window.rows = 24;
    // Before the first frame there is no surface, so the content area is the
    // configured grid (80*9 x 24*20) below the 30px title bar.
    let r = app.content_rect();
    assert_eq!((r.x, r.y, r.w, r.h), (0, 30, 720, 480));
}

#[test]
fn content_rect_reserves_the_titlebar_at_any_grid_size() {
    let mut app = test_app();
    app.config.window.cols = 120;
    app.config.window.rows = 40;
    let r = app.content_rect();
    assert_eq!(
        r.y, TITLEBAR_H as i32,
        "the grid never starts under the chrome"
    );
    assert_eq!((r.w, r.h), (1080, 800));
}

// --- Font zoom ------------------------------------------------------------

#[test]
fn font_scale_clamps_to_the_supported_zoom_range() {
    let mut app = test_app();
    assert_eq!(app.font_scale, 1.0, "a fresh window is un-zoomed");
    app.set_font_scale(99.0);
    assert_eq!(app.font_scale, FONT_SCALE_MAX);
    app.set_font_scale(0.01);
    assert_eq!(app.font_scale, FONT_SCALE_MIN);
}

#[test]
fn reset_font_scale_returns_to_the_identity_cell_size() {
    let mut app = test_app();
    app.set_font_scale(2.5);
    assert_eq!(app.cell_w(), CELL_W * 2.5);
    assert_eq!(app.cell_h(), LINE_HEIGHT * 2.5);
    app.reset_font_scale();
    // Scale 1.0 is byte-identical to the un-zoomed layout.
    assert_eq!(app.font_scale, 1.0);
    assert_eq!(app.cell_w(), CELL_W);
    assert_eq!(app.cell_h(), LINE_HEIGHT);
}

#[test]
fn zoom_font_steps_relatively_and_saturates_at_the_ceiling() {
    let mut app = test_app();
    app.zoom_font(0.5);
    assert!(
        (app.font_scale - 1.5).abs() < 1e-6,
        "got {}",
        app.font_scale
    );
    app.zoom_font(-0.25);
    assert!(
        (app.font_scale - 1.25).abs() < 1e-6,
        "got {}",
        app.font_scale
    );
    // Repeated zoom-in saturates instead of growing without bound.
    for _ in 0..100 {
        app.zoom_font(0.5);
    }
    assert_eq!(app.font_scale, FONT_SCALE_MAX);
    for _ in 0..100 {
        app.zoom_font(-0.5);
    }
    assert_eq!(app.font_scale, FONT_SCALE_MIN);
}

// --- Title-bar hit-testing ------------------------------------------------

#[test]
fn titlebar_hit_routes_the_drag_strip_and_each_caption_button() {
    let w = 1000.0_f64;
    // 1000 - (15 cells * 9px) - 8px right margin.
    let left = App::buttons_left_px(w as f32) as f64;
    assert_eq!(left, 857.0, "button cluster origin");

    // Anything below the title bar belongs to the terminal, not the chrome.
    assert_eq!(
        titlebar_hit_at(500.0, TITLEBAR_H as f64 + 0.1, w),
        TitlebarHit::None
    );
    // The boundary pixel is still chrome.
    assert_eq!(
        titlebar_hit_at(500.0, TITLEBAR_H as f64, w),
        TitlebarHit::Drag
    );

    // Left of the cluster is the window-drag strip.
    assert_eq!(titlebar_hit_at(0.0, 5.0, w), TitlebarHit::Drag);
    assert_eq!(titlebar_hit_at(left - 1.0, 5.0, w), TitlebarHit::Drag);

    // Each button owns a 45px (5-cell) slot.
    assert_eq!(titlebar_hit_at(left, 5.0, w), TitlebarHit::Minimize);
    assert_eq!(titlebar_hit_at(left + 44.0, 5.0, w), TitlebarHit::Minimize);
    assert_eq!(titlebar_hit_at(left + 45.0, 5.0, w), TitlebarHit::Maximize);
    assert_eq!(titlebar_hit_at(left + 89.0, 5.0, w), TitlebarHit::Maximize);
    assert_eq!(titlebar_hit_at(left + 90.0, 5.0, w), TitlebarHit::Close);
    // The 8px right margin past the cluster still closes — no dead pixels in
    // the corner the user throws the mouse at.
    assert_eq!(titlebar_hit_at(w - 1.0, 5.0, w), TitlebarHit::Close);
}

#[test]
fn caption_backplate_rect_agrees_with_the_button_hit_zone() {
    // The load-bearing invariant of a frameless window: the rect we PAINT for a
    // hovered button and the zone that HIT-TESTS to that button are the same
    // geometry. These are two independent code paths (`caption_backplates` vs
    // `titlebar_hit_at`); this cross-checks them.
    let width = 1000.0_f32;
    let mut app = test_app();
    for btn in [
        TitlebarHit::Minimize,
        TitlebarHit::Maximize,
        TitlebarHit::Close,
    ] {
        app.hovered_button = Some(btn);
        let plates = app.caption_backplates(width, [1.0, 1.0, 1.0, 1.0]);
        assert_eq!(plates.len(), 1, "only the hovered button gets a backplate");
        let p = plates[0];
        assert_eq!(
            p.h, TITLEBAR_H as i32,
            "backplate spans the title bar height"
        );
        for x in [p.x, p.x + p.w / 2, p.x + p.w - 1] {
            assert_eq!(
                titlebar_hit_at(x as f64, (p.y + p.h / 2) as f64, width as f64),
                btn,
                "pixel {x} of the {btn:?} backplate must hit-test back to {btn:?}"
            );
        }
    }
}

#[test]
fn caption_backplates_are_empty_until_hover_or_press() {
    let app = test_app();
    assert!(
        app.caption_backplates(1000.0, [1.0; 4]).is_empty(),
        "an untouched title bar paints no button highlights"
    );
}

#[test]
fn close_backplate_is_danger_red_and_strengthens_on_press() {
    let mut app = test_app();
    app.hovered_button = Some(TitlebarHit::Close);
    let hover = app.caption_backplates(1000.0, [0.9, 0.9, 0.9, 1.0])[0].rgba;
    assert_eq!(
        [hover[0], hover[1], hover[2]],
        [1.0, 0.0, 0.25],
        "close hover is the danger red, not the foreground wash"
    );
    app.pressed_button = Some(TitlebarHit::Close);
    let press = app.caption_backplates(1000.0, [0.9, 0.9, 0.9, 1.0])[0].rgba;
    assert_eq!([press[0], press[1], press[2]], [1.0, 0.0, 0.25]);
    assert!(
        press[3] > hover[3],
        "pressed must read stronger than hovered ({} vs {})",
        press[3],
        hover[3]
    );
}

#[test]
fn min_max_backplate_tints_with_the_theme_foreground_not_red() {
    let mut app = test_app();
    let fg = [0.1, 0.2, 0.3, 1.0];
    for btn in [TitlebarHit::Minimize, TitlebarHit::Maximize] {
        app.hovered_button = Some(btn);
        let r = app.caption_backplates(1000.0, fg)[0].rgba;
        assert_eq!(
            [r[0], r[1], r[2]],
            [fg[0], fg[1], fg[2]],
            "{btn:?} hover uses the theme fg wash"
        );
        assert!(r[3] < 0.2, "the wash stays subtle, got alpha {}", r[3]);
    }
}

#[test]
fn pressing_one_button_while_hovering_another_plates_both() {
    // Press min, then slide onto close without releasing: the pressed button
    // keeps its active plate and the hovered one gets its hover plate.
    let mut app = test_app();
    app.pressed_button = Some(TitlebarHit::Minimize);
    app.hovered_button = Some(TitlebarHit::Close);
    let plates = app.caption_backplates(1000.0, [0.9, 0.9, 0.9, 1.0]);
    assert_eq!(plates.len(), 2);
    // Cluster order is min, max, close — so the minimize plate comes first.
    assert!(plates[0].x < plates[1].x);
}

// --- Resize edges ---------------------------------------------------------

#[test]
fn resize_edges_classify_all_eight_directions() {
    let (w, h) = (800.0_f64, 600.0_f64);
    let e = |x, y| resize_edge_at(x, y, w, h);
    assert_eq!(e(0.0, 0.0), Some(ResizeDirection::NorthWest));
    assert_eq!(e(799.0, 0.0), Some(ResizeDirection::NorthEast));
    assert_eq!(e(0.0, 599.0), Some(ResizeDirection::SouthWest));
    assert_eq!(e(799.0, 599.0), Some(ResizeDirection::SouthEast));
    assert_eq!(e(400.0, 2.0), Some(ResizeDirection::North));
    assert_eq!(e(400.0, 598.0), Some(ResizeDirection::South));
    assert_eq!(e(2.0, 300.0), Some(ResizeDirection::West));
    assert_eq!(e(798.0, 300.0), Some(ResizeDirection::East));
    // The interior is the terminal, not a resize handle.
    assert_eq!(e(400.0, 300.0), None);
}

#[test]
fn resize_band_is_exactly_the_documented_thickness() {
    // RESIZE_BORDER = 8: the grab band must be wide enough to hit but must not
    // steal clicks from the terminal beyond it.
    assert_eq!(RESIZE_BORDER, 8.0);
    assert_eq!(
        resize_edge_at(8.0, 300.0, 800.0, 600.0),
        Some(ResizeDirection::West)
    );
    assert_eq!(resize_edge_at(8.01, 300.0, 800.0, 600.0), None);
}

#[test]
fn degenerate_surface_reports_no_resize_edge() {
    // Before the first `Resized` the surface is 0x0. Without the guard every
    // point satisfies BOTH `x <= b` and `x >= w - b`, so the whole window would
    // report NorthWest and the terminal would be un-clickable.
    assert_eq!(resize_edge_at(0.0, 0.0, 0.0, 0.0), None);
    assert_eq!(resize_edge_at(5.0, 5.0, 800.0, 0.0), None);
    assert_eq!(resize_edge_at(5.0, 5.0, 0.0, 600.0), None);
}

#[test]
fn hit_tab_ignores_clicks_below_the_titlebar() {
    let app = test_app();
    assert_eq!(app.hit_tab(100.0, TITLEBAR_H as f64 + 1.0), None);
    // With no surface yet no zones have been published, so nothing is clickable.
    assert_eq!(app.hit_tab(100.0, 5.0), None);
}

// --- Theme colours --------------------------------------------------------

#[test]
fn bg_color_stays_opaque_without_a_translucent_surface() {
    let mut app = test_app();
    app.config.opacity = 0.5;
    // No surface => no alpha mode => opacity must NOT be applied, or a window
    // on an opaque surface would clear to a half-black nothing.
    assert_eq!(app.bg_color().a, 1.0);
}

#[test]
fn bg_color_linearizes_the_theme_background() {
    let mut app = test_app();
    app.theme.background = "#000000".to_string();
    let c = app.bg_color();
    assert_eq!((c.r, c.g, c.b), (0.0, 0.0, 0.0));

    app.theme.background = "#ffffff".to_string();
    assert!((app.bg_color().r - 1.0).abs() < 1e-9);

    // Mid-grey must be LINEARIZED (~0.216) — passing 0.5 straight through is
    // the classic gamma bug and would wash the window out.
    app.theme.background = "#808080".to_string();
    let mid = app.bg_color().r;
    assert!(
        (mid - 0.2158).abs() < 1e-3,
        "expected linearized ~0.2158, got {mid}"
    );
}

#[test]
fn fg_and_accent_read_their_own_theme_fields() {
    let mut app = test_app();
    app.theme.foreground = "#102030".to_string();
    app.theme.cursor = "#ff0040".to_string();
    let fg = app.fg_color();
    assert_eq!((fg.r(), fg.g(), fg.b()), (0x10, 0x20, 0x30));
    let acc = app.accent_color();
    assert_eq!((acc.r(), acc.g(), acc.b()), (255, 0, 64));
}

#[test]
fn unparseable_theme_colours_fall_back_instead_of_blanking_the_window() {
    let mut app = test_app();
    app.theme.background = "not-a-hex".to_string();
    app.theme.foreground = "nope".to_string();
    app.theme.cursor = String::new();
    // A corrupt/partial theme file must never yield a black-on-black window.
    let bg = app.bg_color();
    assert!((bg.r - srgb_to_linear(8)).abs() < 1e-12);
    let fg = app.fg_color();
    assert_eq!((fg.r(), fg.g(), fg.b()), (240, 238, 245));
    let acc = app.accent_color();
    assert_eq!((acc.r(), acc.g(), acc.b()), (0, 229, 255));
}

#[test]
fn window_srgb_to_linear_switches_branch_at_the_knee() {
    assert_eq!(srgb_to_linear(0), 0.0);
    assert!((srgb_to_linear(255) - 1.0).abs() < 1e-12);
    // Linear segment (c=10 -> s=0.039 <= 0.04045).
    let s10 = 10.0_f64 / 255.0;
    assert!((srgb_to_linear(10) - s10 / 12.92).abs() < 1e-12);
    // Gamma segment (c=11 -> s=0.043 > 0.04045).
    let s11 = 11.0_f64 / 255.0;
    assert!((srgb_to_linear(11) - ((s11 + 0.055) / 1.055).powf(2.4)).abs() < 1e-12);
    // Prove the two branches actually diverge at the knee, so a mutant that
    // drops the gamma branch cannot pass.
    assert!((srgb_to_linear(11) - s11 / 12.92).abs() > 1e-9);
}

#[test]
fn load_theme_returns_none_for_a_theme_that_does_not_exist() {
    assert!(load_theme("definitely-not-a-real-theme-9c3f").is_none());
}

#[test]
fn discover_themes_is_sorted_deduped_and_never_empty() {
    let themes = App::discover_themes();
    assert!(!themes.is_empty(), "the theme cycler always has an entry");
    let mut sorted = themes.clone();
    sorted.sort();
    assert_eq!(themes, sorted, "themes are offered in sorted order");
    let mut deduped = themes.clone();
    deduped.dedup();
    assert_eq!(
        themes, deduped,
        "a stem present in both the bundled and user dirs is listed once"
    );
}

// --- Settings panel -------------------------------------------------------

#[test]
fn settings_panel_opens_on_the_first_row_and_lists_every_setting() {
    let mut app = test_app();
    assert!(
        app.settings_text().is_none(),
        "closed panel renders nothing"
    );
    app.open_settings_panel();
    assert_eq!(app.settings.as_ref().map(|p| p.idx), Some(0));
    let text = app.settings_text().expect("an open panel renders text");
    for row in SettingRow::ALL {
        assert!(
            text.contains(row.label()),
            "panel must list {}",
            row.label()
        );
    }
}

#[test]
fn settings_arrows_move_the_selection_and_wrap_both_ways() {
    let mut app = test_app();
    app.open_settings_panel();
    let n = SettingRow::ALL.len();
    let idx = |a: &App| a.settings.as_ref().unwrap().idx;

    app.handle_settings_key(&Key::Named(NamedKey::ArrowDown));
    assert_eq!(idx(&app), 1);
    // Up past the top wraps to the last row.
    app.handle_settings_key(&Key::Named(NamedKey::ArrowUp));
    app.handle_settings_key(&Key::Named(NamedKey::ArrowUp));
    assert_eq!(idx(&app), n - 1);
    // Down past the bottom wraps to the first.
    app.handle_settings_key(&Key::Named(NamedKey::ArrowDown));
    assert_eq!(idx(&app), 0);
}

#[test]
fn escape_closes_the_settings_panel() {
    let mut app = test_app();
    app.open_settings_panel();
    app.handle_settings_key(&Key::Named(NamedKey::Escape));
    assert!(app.settings.is_none());
    assert!(app.settings_text().is_none());
}

#[test]
fn settings_text_marks_exactly_the_selected_row() {
    let mut app = test_app();
    app.open_settings_panel();
    app.handle_settings_key(&Key::Named(NamedKey::ArrowDown));
    let text = app.settings_text().unwrap();
    let marked: Vec<&str> = text.lines().filter(|l| l.starts_with('\u{25b8}')).collect();
    assert_eq!(marked.len(), 1, "exactly one row carries the caret");
    assert!(
        marked[0].contains(SettingRow::Theme.label()),
        "row 1 is Theme, got {:?}",
        marked[0]
    );
}

#[test]
fn settings_text_renders_the_live_value_of_every_row() {
    let mut app = test_app();
    app.open_settings_panel();
    app.config.font.size = 13.5;
    app.config.theme = "wired-noir".to_string();
    app.config.cursor.style = CursorStyle::Bar;
    app.config.cursor.blink = true;
    app.config.scrollback_lines = 5000;
    app.config.opacity = 0.85;
    app.config.startup_panel = false;
    let t = app.settings_text().unwrap();
    let line = |label: &str| {
        t.lines()
            .find(|l| l.contains(label))
            .unwrap_or_else(|| panic!("no row for {label}"))
            .to_string()
    };
    assert!(line("Font size").contains("13.5"));
    assert!(line("Theme").contains("wired-noir"));
    assert!(line("Cursor style").contains("bar"));
    assert!(line("Scrollback").contains("5000"));
    assert!(line("Opacity").contains("0.85"));
    // The two booleans render through `bool_label`, not Debug's true/false.
    assert!(
        line("Cursor blink").ends_with("on"),
        "{}",
        line("Cursor blink")
    );
    assert!(
        line("Startup panel").ends_with("off"),
        "{}",
        line("Startup panel")
    );
}

#[test]
fn clicking_an_unselected_settings_row_selects_without_changing_its_value() {
    let mut app = test_app();
    app.open_settings_panel();
    let before = app.config.opacity;
    // Row 5 = Opacity, while row 0 is selected: a first click only moves focus.
    app.click_settings_row(5);
    assert_eq!(app.settings.as_ref().unwrap().idx, 5);
    assert_eq!(
        app.config.opacity, before,
        "the first click selects a row, it must not adjust it"
    );
}

#[test]
fn settings_row_at_needs_an_open_panel() {
    let app = test_app();
    assert_eq!(app.settings_row_at(100.0, 100.0), None);
}

// --- Settings value adjustment -------------------------------------------

#[test]
fn adjust_font_size_steps_by_half_a_point_and_clamps() {
    let mut app = test_app();
    app.config.font.size = 14.0;
    app.adjust_setting(SettingRow::FontSize, 1, &[]);
    assert_eq!(app.config.font.size, 14.5);
    app.adjust_setting(SettingRow::FontSize, -1, &[]);
    assert_eq!(app.config.font.size, 14.0);
    for _ in 0..200 {
        app.adjust_setting(SettingRow::FontSize, 1, &[]);
    }
    assert_eq!(
        app.config.font.size, 48.0,
        "clamped at the readable maximum"
    );
    for _ in 0..200 {
        app.adjust_setting(SettingRow::FontSize, -1, &[]);
    }
    assert_eq!(app.config.font.size, 6.0, "clamped at the legible minimum");
}

#[test]
fn adjust_opacity_never_reaches_fully_invisible() {
    let mut app = test_app();
    app.config.opacity = 1.0;
    app.adjust_setting(SettingRow::Opacity, -1, &[]);
    assert!((app.config.opacity - 0.95).abs() < 1e-6);
    for _ in 0..200 {
        app.adjust_setting(SettingRow::Opacity, -1, &[]);
    }
    // A 0-opacity window would be invisible AND un-clickable — unrecoverable
    // without hand-editing the config file.
    assert!(
        (app.config.opacity - 0.1).abs() < 1e-6,
        "got {}",
        app.config.opacity
    );
    for _ in 0..200 {
        app.adjust_setting(SettingRow::Opacity, 1, &[]);
    }
    assert!((app.config.opacity - 1.0).abs() < 1e-6);
}

#[test]
fn adjust_scrollback_steps_by_a_thousand_and_saturates_at_zero() {
    let mut app = test_app();
    app.config.scrollback_lines = 1000;
    app.adjust_setting(SettingRow::Scrollback, 1, &[]);
    assert_eq!(app.config.scrollback_lines, 2000);
    // Stepping below zero must saturate, not wrap the usize to ~1.8e19.
    for _ in 0..5 {
        app.adjust_setting(SettingRow::Scrollback, -1, &[]);
    }
    assert_eq!(app.config.scrollback_lines, 0);
    for _ in 0..2000 {
        app.adjust_setting(SettingRow::Scrollback, 1, &[]);
    }
    assert_eq!(
        app.config.scrollback_lines, 1_000_000,
        "clamped at the ceiling"
    );
}

#[test]
fn cursor_style_cycles_forward_and_backward_through_all_three() {
    let mut app = test_app();
    app.config.cursor.style = CursorStyle::Block;
    for expected in [CursorStyle::Bar, CursorStyle::Underline, CursorStyle::Block] {
        app.adjust_setting(SettingRow::CursorStyle, 1, &[]);
        assert_eq!(app.config.cursor.style, expected);
    }
    // Backward is the exact inverse of forward.
    for expected in [CursorStyle::Underline, CursorStyle::Bar, CursorStyle::Block] {
        app.adjust_setting(SettingRow::CursorStyle, -1, &[]);
        assert_eq!(app.config.cursor.style, expected);
    }
}

#[test]
fn boolean_settings_toggle_regardless_of_direction() {
    let mut app = test_app();
    app.config.cursor.blink = false;
    app.adjust_setting(SettingRow::CursorBlink, 1, &[]);
    assert!(app.config.cursor.blink);
    // Left on a toggle also flips it (there is no "less than off").
    app.adjust_setting(SettingRow::CursorBlink, -1, &[]);
    assert!(!app.config.cursor.blink);

    app.config.startup_panel = false;
    app.adjust_setting(SettingRow::StartupPanel, 1, &[]);
    assert!(app.config.startup_panel);
    app.adjust_setting(SettingRow::StartupPanel, -1, &[]);
    assert!(!app.config.startup_panel);
}

#[test]
fn theme_row_cycles_the_discovered_list_and_wraps_both_ways() {
    let mut app = test_app();
    let themes = vec!["a".to_string(), "b".to_string(), "c".to_string()];
    app.config.theme = "a".to_string();
    app.adjust_setting(SettingRow::Theme, 1, &themes);
    assert_eq!(app.config.theme, "b");
    app.adjust_setting(SettingRow::Theme, -1, &themes);
    assert_eq!(app.config.theme, "a");
    // Backward past the start uses rem_euclid, not a negative index panic.
    app.adjust_setting(SettingRow::Theme, -1, &themes);
    assert_eq!(app.config.theme, "c");
    app.adjust_setting(SettingRow::Theme, 1, &themes);
    assert_eq!(app.config.theme, "a");
}

#[test]
fn theme_row_is_inert_without_a_discovered_theme_list() {
    let mut app = test_app();
    app.config.theme = "keep-me".to_string();
    app.adjust_setting(SettingRow::Theme, 1, &[]);
    assert_eq!(
        app.config.theme, "keep-me",
        "an empty theme list must not blank the configured theme"
    );
}

#[test]
fn a_theme_no_longer_on_disk_cycles_to_the_first_entry() {
    let mut app = test_app();
    app.config.theme = "deleted-theme".to_string();
    let themes = vec!["a".to_string(), "b".to_string()];
    app.adjust_setting(SettingRow::Theme, 1, &themes);
    assert_eq!(app.config.theme, "a");
}

#[test]
fn bool_label_renders_on_and_off() {
    assert_eq!(bool_label(true), "on");
    assert_eq!(bool_label(false), "off");
}

// --- Command palette ------------------------------------------------------

#[test]
fn palette_opens_empty_and_offers_every_action() {
    let mut app = test_app();
    app.palette_query = "stale".to_string();
    app.palette_idx = 5;
    app.enter_palette();
    assert!(app.palette_mode);
    assert_eq!(app.palette_query, "", "opening clears the previous query");
    assert_eq!(app.palette_idx, 0);
    assert_eq!(app.palette_filtered().len(), PALETTE_ACTIONS.len());
}

#[test]
fn palette_query_narrows_the_action_list() {
    let mut app = test_app();
    app.enter_palette();
    let all = app.palette_filtered().len();
    app.palette_query = "Split".to_string();
    let hits = app.palette_filtered();
    assert!(hits.len() < all, "a query must narrow the list: {hits:?}");
    assert!(hits.contains(&"Split Right"), "got {hits:?}");
    assert!(!hits.contains(&"Quit"), "got {hits:?}");
}

#[test]
fn palette_arrows_wrap_within_the_filtered_list() {
    let mut app = test_app();
    app.enter_palette();
    let n = app.palette_filtered().len();
    assert_eq!(app.palette_key_nav(&Key::Named(NamedKey::ArrowDown)), None);
    assert_eq!(app.palette_idx, 1);
    app.palette_key_nav(&Key::Named(NamedKey::ArrowUp));
    app.palette_key_nav(&Key::Named(NamedKey::ArrowUp));
    assert_eq!(app.palette_idx, n - 1, "up past the top wraps to the last");
    app.palette_key_nav(&Key::Named(NamedKey::ArrowDown));
    assert_eq!(app.palette_idx, 0, "down past the end wraps to the first");
}

#[test]
fn palette_enter_returns_the_selected_action_and_closes() {
    let mut app = test_app();
    app.enter_palette();
    app.palette_key_nav(&Key::Named(NamedKey::ArrowDown));
    let picked = app.palette_key_nav(&Key::Named(NamedKey::Enter));
    assert_eq!(picked, Some(PALETTE_ACTIONS[1]));
    assert!(!app.palette_mode, "Enter dismisses the palette");
}

#[test]
fn palette_typing_filters_and_resets_the_highlight() {
    let mut app = test_app();
    app.enter_palette();
    app.palette_key_nav(&Key::Named(NamedKey::ArrowDown));
    app.palette_key_nav(&Key::Named(NamedKey::ArrowDown));
    assert_eq!(app.palette_idx, 2);
    // Typing must reset the highlight: the old index could point past the end
    // of the newly-filtered list and select the wrong action on Enter.
    app.palette_key_nav(&Key::Character("S".into()));
    assert_eq!(app.palette_query, "S");
    assert_eq!(app.palette_idx, 0);
    app.palette_key_nav(&Key::Named(NamedKey::Backspace));
    assert_eq!(app.palette_query, "");
    assert_eq!(app.palette_idx, 0);
}

#[test]
fn palette_enter_with_no_match_picks_nothing() {
    let mut app = test_app();
    app.enter_palette();
    app.palette_query = "zzzznotanaction".to_string();
    assert!(
        app.palette_filtered().is_empty(),
        "precondition: no matches"
    );
    assert_eq!(app.palette_key_nav(&Key::Named(NamedKey::Enter)), None);
    assert!(!app.palette_mode);
}

#[test]
fn palette_arrows_survive_an_empty_filtered_list() {
    // The `.max(1)` guard: a modulo by a zero-length list would panic and take
    // the whole window down while the user is typing.
    let mut app = test_app();
    app.enter_palette();
    app.palette_query = "zzzznotanaction".to_string();
    app.palette_key_nav(&Key::Named(NamedKey::ArrowDown));
    app.palette_key_nav(&Key::Named(NamedKey::ArrowUp));
    assert_eq!(app.palette_idx, 0);
}

#[test]
fn palette_escape_closes_without_picking() {
    let mut app = test_app();
    app.enter_palette();
    assert_eq!(app.palette_key_nav(&Key::Named(NamedKey::Escape)), None);
    assert!(!app.palette_mode);
}

#[test]
fn every_advertised_palette_shortcut_is_a_ctrl_chord() {
    assert_eq!(action_hint("New Tab"), "Ctrl+Shift+T");
    assert_eq!(action_hint("Close Tab"), "Ctrl+Shift+W");
    assert_eq!(action_hint("Next Tab"), "Ctrl+Shift+]");
    assert_eq!(action_hint("Previous Tab"), "Ctrl+Shift+[");
    assert_eq!(action_hint("Split Right"), "Ctrl+Shift+D");
    assert_eq!(action_hint("Split Down"), "Ctrl+Shift+E");
    assert_eq!(action_hint("Search"), "Ctrl+Shift+F");
    assert_eq!(action_hint("Settings"), "Ctrl+,");
    // An action with no global shortcut advertises nothing.
    assert_eq!(action_hint("Quit"), "");
    assert_eq!(action_hint("Layout: 2x2"), "");
    // Every hint the palette shows must belong to a REAL palette action, and
    // must read as a chord (catches a typo'd action name in the hint map).
    for a in PALETTE_ACTIONS {
        let h = action_hint(a);
        if !h.is_empty() {
            assert!(h.starts_with("Ctrl"), "{a} -> {h:?}");
        }
    }
}

// --- Search ---------------------------------------------------------------

#[test]
fn enter_search_resets_the_previous_query() {
    let mut app = test_app();
    app.search_query = "stale".to_string();
    app.search_idx = 7;
    app.enter_search();
    assert!(app.search_mode);
    assert_eq!(app.search_query, "");
    assert_eq!(app.search_idx, 0);
}

#[test]
fn search_typing_and_backspace_edit_the_query() {
    let mut app = test_app();
    app.enter_search();
    app.handle_search_key(&Key::Character("e".into()));
    app.handle_search_key(&Key::Character("r".into()));
    app.handle_search_key(&Key::Character("r".into()));
    assert_eq!(app.search_query, "err");
    app.handle_search_key(&Key::Named(NamedKey::Backspace));
    assert_eq!(app.search_query, "er");
}

#[test]
fn escape_leaves_search_mode_and_clears_the_query() {
    let mut app = test_app();
    app.enter_search();
    app.handle_search_key(&Key::Character("x".into()));
    app.handle_search_key(&Key::Named(NamedKey::Escape));
    assert!(!app.search_mode);
    assert_eq!(app.search_query, "");
    assert!(app.search_matches.is_empty());
}

#[test]
fn next_match_is_a_no_op_with_no_matches() {
    // Enter in search mode with nothing found must not panic (`% 0`).
    let mut app = test_app();
    app.enter_search();
    app.handle_search_key(&Key::Named(NamedKey::Enter));
    assert_eq!(app.search_idx, 0);
}

// --- Workspace prompts ----------------------------------------------------

#[test]
fn save_prompt_accepts_typed_text_and_backspace() {
    let mut app = test_app();
    app.open_save_prompt();
    for c in ["d", "e", "v"] {
        app.handle_workspace_prompt_key(&Key::Character(c.into()));
    }
    match app.workspace_prompt.as_ref() {
        Some(WorkspacePrompt::Save { name }) => assert_eq!(name, "dev"),
        _ => panic!("the save prompt should still be open"),
    }
    app.handle_workspace_prompt_key(&Key::Named(NamedKey::Backspace));
    match app.workspace_prompt.as_ref() {
        Some(WorkspacePrompt::Save { name }) => assert_eq!(name, "de"),
        _ => panic!("the save prompt should still be open"),
    }
}

#[test]
fn escape_dismisses_the_save_prompt_without_saving() {
    let mut app = test_app();
    app.open_save_prompt();
    app.handle_workspace_prompt_key(&Key::Character("x".into()));
    app.handle_workspace_prompt_key(&Key::Named(NamedKey::Escape));
    assert!(app.workspace_prompt.is_none());
}

#[test]
fn restore_prompt_arrows_wrap_over_the_name_list() {
    let mut app = test_app();
    app.workspace_prompt = Some(WorkspacePrompt::Restore {
        names: vec!["a".to_string(), "b".to_string(), "c".to_string()],
        idx: 0,
    });
    let idx = |a: &App| match a.workspace_prompt.as_ref() {
        Some(WorkspacePrompt::Restore { idx, .. }) => *idx,
        _ => panic!("the restore prompt should still be open"),
    };
    app.handle_workspace_prompt_key(&Key::Named(NamedKey::ArrowDown));
    assert_eq!(idx(&app), 1);
    app.handle_workspace_prompt_key(&Key::Named(NamedKey::ArrowUp));
    app.handle_workspace_prompt_key(&Key::Named(NamedKey::ArrowUp));
    assert_eq!(idx(&app), 2, "up past the top wraps to the last name");
    app.handle_workspace_prompt_key(&Key::Named(NamedKey::ArrowDown));
    assert_eq!(idx(&app), 0, "down past the end wraps to the first");
}

#[test]
fn a_prompt_key_with_no_prompt_open_is_a_no_op() {
    let mut app = test_app();
    app.handle_workspace_prompt_key(&Key::Named(NamedKey::Enter));
    assert!(app.workspace_prompt.is_none());
}

#[test]
fn workspace_name_sanitizing_blocks_path_escape() {
    // SECURITY: the name is user-supplied and becomes a file stem, so a
    // traversal attempt must be flattened, never resolved.
    let s = sanitize_workspace_name("../../etc/passwd");
    assert_eq!(s, "______etc_passwd");
    assert!(!s.contains(".."));
    assert!(!s.contains('/'));
    let win = sanitize_workspace_name(r"..\..\Windows\System32");
    assert!(!win.contains('\\') && !win.contains(".."));
    // NUL / control / Unicode all fold to a single '_' each.
    assert_eq!(sanitize_workspace_name("a\0b\nc\u{1F600}"), "a_b_c_");
    // The allowed set survives untouched.
    assert_eq!(sanitize_workspace_name("My-Layout_2"), "My-Layout_2");
    // An empty (or fully-stripped) name never yields an empty stem.
    assert_eq!(sanitize_workspace_name(""), "default");
    // Over-long names truncate to a bounded stem.
    assert_eq!(sanitize_workspace_name(&"a".repeat(200)).len(), 64);
}

#[test]
fn workspace_path_is_always_a_flat_stem_inside_the_workspaces_dir() {
    let dir = App::workspaces_dir().expect("this platform has a per-user config dir");
    let p = App::workspace_path("../../evil").expect("a workspaces dir exists");
    assert_eq!(
        p.parent(),
        Some(dir.as_path()),
        "a hostile name must not escape the workspaces dir"
    );
    assert_eq!(
        p.file_name().and_then(|s| s.to_str()),
        Some("______evil.layout.json")
    );
}

// --- Multi-monitor geometry safety ---------------------------------------

#[test]
fn saved_geometry_is_accepted_when_no_monitor_info_is_available() {
    // Wayland / headless reports no monitors: decline to assert position
    // validity rather than discard a valid saved size.
    assert!(geometry_on_screen(0, 0, 800, 600, &[]));
    assert!(geometry_on_screen(-9999, -9999, 800, 600, &[]));
}

#[test]
fn a_window_must_keep_64px_visible_on_a_monitor() {
    // One 1920x1080 monitor at the origin; an 800x600 window at (px, py).
    let mon = LRect::new(0, 0, 1920, 1080);
    let vis = |px, py| window_visible_on_monitor(LRect::new(px, py, 800, 600), mon);
    assert!(vis(100, 100), "fully on-screen");
    // Exactly the documented 64px minimum on both axes.
    assert!(vis(1920 - 64, 1080 - 64));
    // One pixel less and the window is effectively lost.
    assert!(!vis(1920 - 63, 1080 - 63));
    // Same at the left/top edges (negative origins).
    assert!(vis(-800 + 64, -600 + 64));
    assert!(!vis(-800 + 63, -600 + 63));
    // Entirely off-screen — e.g. a saved position on an unplugged monitor.
    assert!(!vis(3000, 100));
    assert!(!vis(100, 3000));
}

#[test]
fn overlap_must_be_sufficient_on_both_axes_not_just_one() {
    let mon = LRect::new(0, 0, 1920, 1080);
    // Overlaps fully in x but is far below the monitor in y: still lost, so the
    // check must be an AND, not an OR.
    assert!(!window_visible_on_monitor(
        LRect::new(0, 2000, 800, 600),
        mon
    ));
    // ...and the mirror case.
    assert!(!window_visible_on_monitor(
        LRect::new(3000, 0, 800, 600),
        mon
    ));
}

// --- Drag overlay ---------------------------------------------------------

#[test]
fn no_drag_overlay_while_idle_or_merely_pressed() {
    let accent = GColor::rgb(0, 229, 255);
    let app = test_app();
    assert!(
        app.drag_overlay_quads(accent).is_empty(),
        "idle draws nothing"
    );
    // A press that has not crossed the threshold is still a click, not a drag.
    let mut pressed = test_app();
    pressed.drag = DragState::press(LeafId(0), (10.0, 10.0));
    assert!(
        pressed.drag_overlay_quads(accent).is_empty(),
        "a sub-threshold press must not paint a drag overlay"
    );
}

#[test]
fn drag_ghost_drops_its_motion_halo_under_reduced_motion() {
    let accent = GColor::rgb(0, 229, 255);
    let mut app = test_app();
    app.drag = DragState::Dragging {
        leaf: LeafId(0),
        cursor: (400.0, 300.0),
    };

    app.reduced_motion = false;
    let motion = app.drag_overlay_quads(accent);
    app.reduced_motion = true;
    let reduced = app.drag_overlay_quads(accent);

    assert_eq!(
        motion.len(),
        reduced.len() + 1,
        "the halo is the only motion-suggesting quad"
    );

    // The crisp 26px ghost survives in both, centred on the cursor.
    let ghost = *reduced.last().unwrap();
    assert_eq!((ghost.w, ghost.h), (26, 26));
    assert_eq!((ghost.x, ghost.y), (400 - 13, 300 - 13));
    // ...and it carries the accent tint.
    assert!((ghost.rgba[1] - 229.0 / 255.0).abs() < 1e-6);

    // The halo is the larger, fainter square behind it.
    let halo = motion[motion.len() - 2];
    assert_eq!((halo.w, halo.h), (40, 40));
    assert!(
        halo.rgba[3] < ghost.rgba[3],
        "the halo must be fainter than the ghost"
    );
}

// --- Zone highlight geometry ---------------------------------------------

#[test]
fn zone_highlight_covers_the_expected_band_of_the_target() {
    let t = LRect::new(100, 200, 300, 600); // thirds: 100 wide, 200 tall
    let c = [0.0, 1.0, 1.0, 0.35];
    let r = |z| {
        let q = zone_highlight_rect(t, z, c);
        (q.x, q.y, q.w, q.h)
    };
    assert_eq!(r(DropZone::Left), (100, 200, 100, 600));
    assert_eq!(r(DropZone::Right), (300, 200, 100, 600));
    assert_eq!(r(DropZone::Top), (100, 200, 300, 200));
    assert_eq!(r(DropZone::Bottom), (100, 600, 300, 200));
    assert_eq!(r(DropZone::Center), (200, 400, 100, 200));
    // The requested colour is carried through untouched.
    assert_eq!(zone_highlight_rect(t, DropZone::Center, c).rgba, c);
}

#[test]
fn zone_highlight_never_degenerates_on_a_tiny_target() {
    // A 1x1 target: the thirds round to 0 and the centre would be -1 wide.
    let r = zone_highlight_rect(LRect::new(0, 0, 1, 1), DropZone::Center, [0.0; 4]);
    assert!(
        r.w >= 1 && r.h >= 1,
        "a quad must never have a non-positive extent, got {}x{}",
        r.w,
        r.h
    );
}

// --- Grid sizing ----------------------------------------------------------

#[test]
fn focused_cell_dims_falls_back_to_a_sane_grid_without_tabs() {
    let app = test_app();
    assert_eq!(app.focused_cell_dims(LRect::new(0, 30, 800, 600)), (24, 80));
}

// --- Key encoding ---------------------------------------------------------

#[test]
fn every_function_key_encodes_a_distinct_sequence() {
    // Catches a copy-paste in the F1..F12 mapping, without restating the
    // escape sequences the core encoder owns.
    let none = ModifiersState::empty();
    let keys = [
        NamedKey::F1,
        NamedKey::F2,
        NamedKey::F3,
        NamedKey::F4,
        NamedKey::F5,
        NamedKey::F6,
        NamedKey::F7,
        NamedKey::F8,
        NamedKey::F9,
        NamedKey::F10,
        NamedKey::F11,
        NamedKey::F12,
    ];
    // `NamedKey` is `Copy`, so it is passed by value (a `.clone()` here would
    // trip `clippy::clone_on_copy`, which CI denies).
    let mut seen = std::collections::HashSet::new();
    for k in keys {
        let b = key_to_bytes(&Key::Named(k), &None, false, none)
            .unwrap_or_else(|| panic!("{k:?} must encode to something"));
        let fresh = seen.insert(b.clone());
        assert!(fresh, "{k:?} duplicated the encoding {b:?}");
    }
    assert_eq!(seen.len(), 12);
}

#[test]
fn home_and_end_honour_decckm() {
    let none = ModifiersState::empty();
    for k in [NamedKey::Home, NamedKey::End] {
        let normal = key_to_bytes(&Key::Named(k), &None, false, none);
        let app_mode = key_to_bytes(&Key::Named(k), &None, true, none);
        assert!(normal.is_some() && app_mode.is_some());
        assert_ne!(
            normal, app_mode,
            "{k:?} must switch CSI->SS3 under application-cursor mode"
        );
    }
}

#[test]
fn a_key_with_no_text_encodes_nothing() {
    // A bare modifier press delivers no composed text, so nothing reaches the PTY.
    let none = ModifiersState::empty();
    assert_eq!(
        key_to_bytes(&Key::Named(NamedKey::Shift), &None, false, none),
        None
    );
}
// --- Cell / Tab structure (needs a real PTY child) ------------------------
//
// `Cell`/`Tab` own live `Session`s, so these spawn a real, immediately-exiting
// PTY child. That is the same headless pattern `crates/core` already uses for
// its own session tests (and what `--demo` does), so it needs no display/GPU —
// only the `Session` VALUE is under test here, not any shell output.

/// A throwaway `Session` whose child exits immediately.
fn test_session() -> Session {
    #[cfg(windows)]
    let s = Session::spawn_program("cmd.exe", &["/C", "exit"], 24, 80);
    #[cfg(not(windows))]
    let s = Session::spawn_program("/bin/sh", &["-c", "true"], 24, 80);
    s.expect("spawning a throwaway PTY child")
}

#[test]
fn a_fresh_cell_holds_exactly_one_active_tab() {
    let c = Cell::single(LeafId(3), test_session());
    assert_eq!(c.tab_count(), 1);
    assert_eq!(c.group.active, 0);
    assert_eq!(c.group.id, LeafId(3), "the group id tracks its leaf");
    assert!(c.active().is_some());
}

#[test]
fn adding_a_cell_tab_makes_the_new_tab_active() {
    let mut c = Cell::single(LeafId(0), test_session());
    c.add(test_session());
    assert_eq!(c.tab_count(), 2);
    assert_eq!(c.group.active, 1, "a newly-opened cell tab takes focus");
    assert!(c.active().is_some());
    // The group's slot list and the parallel session list stay in lockstep —
    // they are indexed by the same `group.active`, so a drift here would show
    // the wrong shell.
    assert_eq!(c.group.tabs.len(), c.sessions.len());
}

#[test]
fn closing_tabs_reports_empty_only_on_the_last_one() {
    let mut c = Cell::single(LeafId(0), test_session());
    c.add(test_session());
    assert!(!c.close_active(), "a 2-tab cell survives closing one");
    assert_eq!(c.tab_count(), 1);
    assert_eq!(c.group.tabs.len(), c.sessions.len());
    // Closing the last tab reports empty so the caller collapses the leaf out
    // of the split tree (rather than leaving a blank pane behind).
    assert!(c.close_active(), "closing the last tab empties the cell");
    assert_eq!(c.tab_count(), 0);
    assert!(c.active().is_none());
}

#[test]
fn a_fresh_tab_has_a_focused_session_on_its_root_leaf() {
    let t = Tab::single(test_session());
    assert_eq!(t.cells.len(), 1);
    assert!(
        t.cells.contains_key(&t.layout.focused),
        "the focused leaf must own a cell"
    );
    assert!(t.focused_session().is_some());
}

#[test]
fn active_session_follows_the_active_tab_index() {
    let mut app = test_app();
    assert!(app.active_session().is_none(), "no tabs => no session");
    app.tabs.push(Tab::single(test_session()));
    assert!(app.active_tab().is_some());
    assert!(app.active_session().is_some());
    // An `active` index past the end must resolve to None, never panic.
    app.active = 9;
    assert!(app.active_tab().is_none());
    assert!(app.active_session().is_none());
    assert!(app.active_session_mut().is_none());
}

#[test]
fn leaf_at_maps_content_pixels_to_the_pane_and_chrome_to_nothing() {
    let mut app = test_app();
    app.tabs.push(Tab::single(test_session()));
    let focused = app.tabs[0].layout.focused;
    let content = app.content_rect();
    // A point inside the content area resolves to the single (root) leaf.
    assert_eq!(
        app.leaf_at((content.x + 5) as f64, (content.y + 5) as f64),
        Some(focused)
    );
    // A point in the title bar is chrome, not a pane.
    assert_eq!(app.leaf_at(5.0, 1.0), None);
    // A single leaf fills the whole content area.
    assert_eq!(
        app.leaf_rect(focused).map(|r| (r.x, r.y, r.w, r.h)),
        Some((content.x, content.y, content.w, content.h))
    );
}

#[test]
fn leaf_rect_is_none_for_a_leaf_that_is_not_in_the_tree() {
    let mut app = test_app();
    app.tabs.push(Tab::single(test_session()));
    assert_eq!(app.leaf_rect(LeafId(9999)), None);
}

#[test]
fn focused_cell_dims_derive_the_grid_from_the_cell_rect() {
    let mut app = test_app();
    app.tabs.push(Tab::single(test_session()));
    let content = LRect::new(0, 30, 800, 600);
    let (rows, cols) = app.focused_cell_dims(content);
    // A single 800x600 pane, 1px border each side, 9x20 cells: the strip line
    // is reserved because the cell is tall enough for one.
    assert_eq!(cols, 88, "(800 - 2) / 9 = 88 columns");
    assert_eq!(rows, 28, "(600 - 2 - 20 strip) / 20 = 28 rows");
    // Zooming the font must shrink the grid.
    app.set_font_scale(2.0);
    let (rows2, cols2) = app.focused_cell_dims(content);
    assert!(
        cols2 < cols && rows2 < rows,
        "a bigger font means fewer cells"
    );
}

// --- Pane management (drives the real split tree over live PTYs) -----------
//
// These exercise the pane surface the user actually operates: split, focus,
// zoom, merge, and the pixel->cell mapping. They spawn real shells (as the app
// does), pointed at a cheap program so the test stays fast; `Session::drop`
// kills each child, so nothing is leaked.

/// An `App` with one tab whose splits spawn a cheap child rather than the
/// developer's real interactive shell.
fn app_with_tab() -> App {
    let mut app = test_app();
    #[cfg(windows)]
    {
        app.config.shell = Some("cmd.exe".to_string());
    }
    #[cfg(not(windows))]
    {
        app.config.shell = Some("/bin/sh".to_string());
    }
    app.tabs.push(Tab::single(test_session()));
    app
}

#[test]
fn splitting_grows_the_tree_and_gives_every_pane_a_session() {
    let mut app = app_with_tab();
    assert_eq!(app.tabs[0].layout.leaf_count(), 1);
    app.split_active(Axis::Horizontal);
    assert_eq!(app.tabs[0].layout.leaf_count(), 2, "a split adds a leaf");
    // The invariant the rollback in `split_active` exists to protect: every
    // leaf in the tree owns a cell with a live session — never a blank pane.
    for leaf in app.tabs[0].layout.leaves() {
        assert!(
            app.tabs[0]
                .cells
                .get(&leaf)
                .and_then(Cell::active)
                .is_some(),
            "leaf {leaf:?} has no session"
        );
    }
    app.split_active(Axis::Vertical);
    assert_eq!(app.tabs[0].layout.leaf_count(), 3);
    for leaf in app.tabs[0].layout.leaves() {
        assert!(app.tabs[0].cells.contains_key(&leaf));
    }
}

#[test]
fn a_horizontal_split_halves_the_width_and_keeps_the_height() {
    let mut app = app_with_tab();
    let full = app.leaf_rect(app.tabs[0].layout.focused).unwrap();
    app.split_active(Axis::Horizontal);
    let leaves = app.tabs[0].layout.leaves();
    let rects: Vec<LRect> = leaves.iter().filter_map(|&l| app.leaf_rect(l)).collect();
    assert_eq!(rects.len(), 2);
    for r in &rects {
        assert!(r.w < full.w, "pane {r:?} should be narrower than {full:?}");
        assert_eq!(r.h, full.h, "a horizontal split keeps the full height");
    }
}

#[test]
fn focus_next_pane_cycles_every_leaf_and_wraps_home() {
    let mut app = app_with_tab();
    app.split_active(Axis::Horizontal);
    app.split_active(Axis::Vertical);
    let n = app.tabs[0].layout.leaf_count();
    assert_eq!(n, 3);
    let start = app.tabs[0].layout.focused;
    let mut seen = vec![start];
    for _ in 1..n {
        app.focus_next_pane();
        seen.push(app.tabs[0].layout.focused);
    }
    // Every pane is reachable by repeated Focus Next...
    let mut uniq: Vec<u64> = seen.iter().map(|l| l.0).collect();
    uniq.sort_unstable();
    uniq.dedup();
    assert_eq!(uniq.len(), n, "focus cycle visited {seen:?}");
    // ...and one more step wraps back to where it began.
    app.focus_next_pane();
    assert_eq!(app.tabs[0].layout.focused, start);
}

#[test]
fn focus_next_pane_is_a_no_op_on_a_single_pane() {
    let mut app = app_with_tab();
    let only = app.tabs[0].layout.focused;
    app.focus_next_pane();
    assert_eq!(app.tabs[0].layout.focused, only);
}

#[test]
fn zooming_a_pane_makes_it_fill_the_content_area_without_mutating_the_tree() {
    let mut app = app_with_tab();
    app.split_active(Axis::Horizontal);
    let focused = app.tabs[0].layout.focused;
    let content = app.content_rect();
    let split_rect = app.leaf_rect(focused).unwrap();
    assert!(split_rect.w < content.w, "precondition: the pane is split");

    app.toggle_zoom();
    let zoomed = app.leaf_rect(focused).unwrap();
    assert_eq!(
        (zoomed.w, zoomed.h),
        (content.w, content.h),
        "a zoomed pane fills the window"
    );
    // Zoom is a pure render override, NOT a tree mutation — the sibling lives.
    assert_eq!(app.tabs[0].layout.leaf_count(), 2);

    app.toggle_zoom();
    let restored = app.leaf_rect(focused).unwrap();
    assert_eq!((restored.w, restored.h), (split_rect.w, split_rect.h));
}

#[test]
fn merging_folds_the_source_tabs_into_the_target_and_drops_the_leaf() {
    let mut app = app_with_tab();
    app.split_active(Axis::Horizontal);
    let leaves = app.tabs[0].layout.leaves();
    let (src, dst) = (leaves[0], leaves[1]);
    let src_tabs = app.tabs[0].cells[&src].tab_count();
    let dst_tabs = app.tabs[0].cells[&dst].tab_count();

    app.merge_into(src, dst);

    assert_eq!(
        app.tabs[0].layout.leaf_count(),
        1,
        "the source leaf is gone"
    );
    assert!(!app.tabs[0].cells.contains_key(&src));
    assert_eq!(
        app.tabs[0].cells[&dst].tab_count(),
        src_tabs + dst_tabs,
        "the source's sessions became nested tabs of the target"
    );
    assert_eq!(app.tabs[0].layout.focused, dst, "focus follows the merge");
}

#[test]
fn merging_a_pane_into_itself_is_a_no_op() {
    let mut app = app_with_tab();
    app.split_active(Axis::Horizontal);
    let src = app.tabs[0].layout.leaves()[0];
    let before = app.tabs[0].layout.leaf_count();
    app.merge_into(src, src);
    assert_eq!(app.tabs[0].layout.leaf_count(), before);
    assert!(
        app.tabs[0].cells.contains_key(&src),
        "a self-merge must not lose the cell"
    );
}

#[test]
fn merging_into_a_missing_target_puts_the_source_back() {
    // The "shouldn't happen" branch: the source's live shells must never be
    // dropped on the floor.
    let mut app = app_with_tab();
    app.split_active(Axis::Horizontal);
    let src = app.tabs[0].layout.leaves()[0];
    let before = app.tabs[0].cells[&src].tab_count();
    app.merge_into(src, LeafId(9999));
    assert!(
        app.tabs[0].cells.contains_key(&src),
        "the source cell must be restored"
    );
    assert_eq!(app.tabs[0].cells[&src].tab_count(), before);
}

#[test]
fn cell_at_pixel_maps_the_grid_origin_to_row_zero_col_zero() {
    let app = app_with_tab();
    let leaf = app.tabs[0].layout.focused;
    let rect = app.leaf_rect(leaf).unwrap();
    // A single pane draws no border; the grid origin is inset by the config pad.
    let (ox, oy) = leaf_text_origin(rect, 0, app.config.window.padding as f32, 2.0);
    assert_eq!(
        app.cell_at_pixel(ox as f64, oy as f64),
        Some((leaf, 0, 0)),
        "the first glyph's origin is cell (0,0)"
    );
    // One cell right and one row down.
    assert_eq!(
        app.cell_at_pixel((ox + CELL_W) as f64, (oy + LINE_HEIGHT) as f64),
        Some((leaf, 1, 1))
    );
    // Left of / above the grid origin is padding, not a cell.
    assert_eq!(app.cell_at_pixel((ox - 1.0) as f64, oy as f64), None);
    assert_eq!(app.cell_at_pixel(ox as f64, (oy - 1.0) as f64), None);
}

#[test]
fn cell_at_pixel_tracks_the_font_zoom() {
    let mut app = app_with_tab();
    let leaf = app.tabs[0].layout.focused;
    let rect = app.leaf_rect(leaf).unwrap();
    let (ox, oy) = leaf_text_origin(rect, 0, app.config.window.padding as f32, 2.0);
    // At 2x zoom the 4th column starts twice as far right. Render (col->px) and
    // hit-test (px->col) must agree, or clicks land off the glyph.
    app.set_font_scale(2.0);
    let px = (ox + 4.0 * CELL_W * 2.0) as f64;
    assert_eq!(app.cell_at_pixel(px, oy as f64), Some((leaf, 0, 4)));
}

#[test]
fn selection_text_is_none_without_a_selection() {
    let app = app_with_tab();
    assert!(app.selection_text().is_none());
}

#[test]
fn selection_text_is_none_for_a_leaf_outside_the_active_tab() {
    let mut app = app_with_tab();
    app.selection = Some(Selection {
        leaf: LeafId(9999),
        anchor: (0, 0),
        head: (0, 5),
        active: false,
    });
    assert!(
        app.selection_text().is_none(),
        "a stale selection must not panic or read another pane"
    );
}

#[test]
fn url_at_finds_nothing_over_a_blank_grid() {
    // A freshly-spawned pane has produced no output, so no cell holds a URL.
    let app = app_with_tab();
    let rect = app.leaf_rect(app.tabs[0].layout.focused).unwrap();
    assert!(app
        .url_at((rect.x + 20) as f64, (rect.y + 20) as f64)
        .is_none());
}

#[test]
fn cursor_quads_are_empty_when_the_focused_leaf_is_not_laid_out() {
    let app = app_with_tab();
    let focused = app.tabs[0].layout.focused;
    // No cascade entry for the leaf -> nothing to draw (the guard that stops the
    // renderer indexing a pane that has no rect this frame).
    assert!(app
        .cursor_quads(
            focused,
            &[],
            BORDER_PX,
            &std::collections::HashSet::new(),
            GColor::rgb(0, 229, 255)
        )
        .is_empty());
}

#[test]
fn applying_a_preset_lays_out_its_pane_count_and_sessions_them_all() {
    for (preset, want) in [
        (Preset::Single, 1usize),
        (Preset::TwoColumns, 2),
        (Preset::TwoRows, 2),
        (Preset::Grid2x2, 4),
    ] {
        let mut app = app_with_tab();
        app.apply_preset(preset);
        let n = app.tabs[0].layout.leaf_count();
        assert_eq!(n, want, "{preset:?} should lay out {want} panes, got {n}");
        for leaf in app.tabs[0].layout.leaves() {
            assert!(
                app.tabs[0]
                    .cells
                    .get(&leaf)
                    .and_then(Cell::active)
                    .is_some(),
                "{preset:?} left leaf {leaf:?} without a session"
            );
        }
    }
}

#[test]
fn cell_tab_combo_zooms_the_font_and_reports_consumption() {
    let mut app = test_app();
    // Ctrl+= zooms in, Ctrl+- out, Ctrl+0 resets — each CONSUMED so the key is
    // not also forwarded to the shell.
    assert!(app.handle_cell_tab_combo(&Key::Character("=".into())));
    assert!(
        (app.font_scale - 1.1).abs() < 1e-6,
        "got {}",
        app.font_scale
    );
    assert!(app.handle_cell_tab_combo(&Key::Character("-".into())));
    assert!(
        (app.font_scale - 1.0).abs() < 1e-6,
        "got {}",
        app.font_scale
    );
    app.set_font_scale(2.0);
    assert!(app.handle_cell_tab_combo(&Key::Character("0".into())));
    assert_eq!(app.font_scale, 1.0);
    // An unrelated key is NOT consumed, so it still reaches the shell.
    assert!(!app.handle_cell_tab_combo(&Key::Character("q".into())));
    assert!(!app.handle_cell_tab_combo(&Key::Named(NamedKey::F5)));
}

#[test]
fn alt_arrow_is_consumed_only_for_the_four_directions() {
    let mut app = app_with_tab();
    for k in [
        NamedKey::ArrowLeft,
        NamedKey::ArrowRight,
        NamedKey::ArrowUp,
        NamedKey::ArrowDown,
    ] {
        assert!(
            app.handle_alt_combo(&Key::Named(k)),
            "{k:?} is a pane chord"
        );
    }
    // Everything else falls through to the PTY.
    assert!(!app.handle_alt_combo(&Key::Named(NamedKey::Enter)));
    assert!(!app.handle_alt_combo(&Key::Character("b".into())));
}
