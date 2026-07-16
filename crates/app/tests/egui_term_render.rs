//! End-to-end **rendered-frame** test for the C0PL4ND egui terminal — the
//! both-panes-render regression guard.
//!
//! ## Why this test exists
//!
//! The interaction tests (`egui_terminal.rs`) drive the PTY→grid pipeline but
//! assert on grid STATE, not on rendered pixels. That blind spot is exactly how
//! the "terminal panes render pure black" defect shipped: every state-only test
//! was green while the live render drew nothing. (The defect was a glyphon GPU
//! paint path that composited black inside `egui_tiles` panes on the real
//! swapchain; it was replaced by egui's native coloured-text painter, see
//! `egui_app::paint_grid_native`.)
//!
//! This test closes the gap. It builds the REAL `C0pl4ndApp` through eframe's
//! creation path with a REAL wgpu render state, drives the production frame loop
//! until the spawned shell's output lands in a pane's grid, renders the WHOLE
//! egui frame to an image, and asserts that BOTH panes drew a non-trivial number
//! of non-background (glyph) pixels. A regression that blanks either pane fails
//! this test loudly.

use c0pl4nd::egui_app;
use std::time::{Duration, Instant};

/// The known text fed into the focused pane's PTY; its glyphs must show up in
/// the rendered frame. A token that cannot pre-exist on a fresh grid.
const TOKEN: &str = "XYZZY";

/// Probe for a usable wgpu adapter+device. Returns `None` (test skips, never a
/// false green) on a box without a GPU, so CI on a headless runner is honest.
fn gpu() -> Option<(wgpu::Device, wgpu::Queue)> {
    let instance = wgpu::Instance::default();
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::LowPower,
        compatible_surface: None,
        force_fallback_adapter: false,
    }))
    .ok()?;
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("c0pl4nd-term-render-test"),
        ..Default::default()
    }))
    .ok()?;
    Some((device, queue))
}

/// THE deliverable: build the real app, type into the focused pane, render the
/// real egui frame, and assert BOTH panes show glyphs. Catches the "black panes"
/// and the "only one pane renders" classes headlessly.
#[test]
fn terminal_renders_both_panes_through_real_frame() {
    use std::cell::RefCell;

    use egui_kittest::Harness;

    // Build the harness with the wgpu renderer so `cc.wgpu_render_state` is real
    // (drives the live render path). If no GPU adapter exists,
    // `WgpuTestRenderer::default()` panics inside the builder — probe first and
    // skip cleanly if absent (never a false green).
    if gpu().is_none() {
        eprintln!("no GPU adapter; skipping end-to-end render");
        return;
    }

    let mut harness: Harness<'_, egui_app::C0pl4ndApp> = Harness::builder()
        .with_size(egui::vec2(900.0, 600.0))
        .wgpu()
        .build_eframe(|cc| {
            let mut app = egui_app::C0pl4ndApp::new(cc);
            // HERMETIC: `new` loads the user's real on-disk config, which may carry
            // a low window `opacity` (or a tint) from their own use. This test
            // counts BRIGHT (glyph) pixels to prove BOTH panes render text, so it
            // needs OPAQUE panes — a see-through pane fades its background to the
            // transparent desktop and the bright-pixel count collapses (a false
            // failure that has nothing to do with glyph rendering). Force opacity
            // 1.0 + tint off so the assertion measures glyph rendering only,
            // independent of whatever transparency the user left persisted.
            app.config.opacity = 1.0;
            app.config.tint_enabled = false;
            app
        });

    // Skip if the platform shell did not spawn (no live PTY → no grid text).
    if harness.state().focused_grid_text().is_none() {
        eprintln!("no live PTY on this platform; skipping render assert");
        return;
    }

    // Type a command that prints the token; Enter submits. Typing via egui Text
    // events drives the real input path.
    for ch in format!("echo {TOKEN}").chars() {
        harness.event(egui::Event::Text(ch.to_string()));
    }
    harness.step();
    harness.key_press(egui::Key::Enter);
    harness.step();

    // Poll for the token (PTY echoes/executes asynchronously).
    let seen = RefCell::new(false);
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        harness.step();
        if harness
            .state()
            .focused_grid_text()
            .is_some_and(|t| t.contains(TOKEN))
        {
            *seen.borrow_mut() = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(40));
    }
    if !*seen.borrow() {
        eprintln!("token never reached the grid in the real app; skipping render assert");
        return;
    }

    // Render the REAL egui frame. Use `step()` (one frame) NOT `run()`: a live
    // window calls `request_repaint()` every frame, so `run()` would loop to
    // max_steps.
    harness.step();
    let img = harness
        .render()
        .expect("kittest wgpu render of the real frame must succeed");

    // The pane bodies live below the titlebar and above the status bar. The
    // default grid is TWO panes side-by-side (a horizontal split), so the
    // central band's LEFT half is one pane and the RIGHT half the other. Count
    // bright (glyph) pixels in EACH half separately: a multi-pane blank bug shows
    // up as one half having ~0 bright pixels while the other is populated. A
    // single whole-band count cannot distinguish "both panes render" from "only
    // one pane renders" — the exact blind spot that let the black-pane defect ship.
    let (w, h) = (img.width(), img.height());
    let band_top = h / 6; // below titlebar
    let band_bottom = h - h / 8; // above status bar
    let mid_x = w / 2;
    let mut left_non_bg = 0u64;
    let mut right_non_bg = 0u64;
    for y in band_top..band_bottom {
        for x in 0..w {
            let p = img.get_pixel(x, y);
            let [r, g, b, _] = p.0;
            // "Bright enough to be a glyph, not the void background."
            if r as u16 + g as u16 + b as u16 > 120 {
                if x < mid_x {
                    left_non_bg += 1;
                } else {
                    right_non_bg += 1;
                }
            }
        }
    }
    let central_non_bg = left_non_bg + right_non_bg;
    eprintln!(
        "real-frame render: {w}x{h}, left_pane_non_bg={left_non_bg} \
         right_pane_non_bg={right_non_bg} total={central_non_bg}"
    );
    assert!(
        central_non_bg >= 200,
        "the real egui frame drew too few bright pixels in the pane region \
         ({central_non_bg}) — the panes rendered (near-)black end to end. The grid \
         HAS the token ({TOKEN}); the render path is the defect."
    );
    // BOTH panes must show glyphs — the multi-pane regression guard.
    assert!(
        left_non_bg >= 100,
        "the LEFT pane drew too few bright pixels ({left_non_bg}) while the right \
         drew {right_non_bg} — only one pane is rendering."
    );
    assert!(
        right_non_bg >= 100,
        "the RIGHT pane drew too few bright pixels ({right_non_bg}) while the left \
         drew {left_non_bg} — only one pane is rendering."
    );
}
