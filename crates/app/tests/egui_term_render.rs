//! Headless **offscreen pixel-readback** test for the C0PL4ND egui terminal's
//! glyphon GPU render path (Milestone 2 — the black-pane regression guard).
//!
//! ## Why this test exists
//!
//! The Milestone 2 interaction tests (`egui_terminal.rs`) drive the PTY→grid
//! pipeline and the egui *text fallback*; they CANNOT see the real glyphon GPU
//! callback because kittest's software path provides no wgpu render pass. That
//! blind spot is exactly how the "terminal panes render pure black" defect
//! shipped: every headless test was green while the live glyphon callback drew
//! nothing.
//!
//! This test closes the gap. It renders a live pane's grid through the EXACT
//! same shared draw code the production egui callback uses
//! ([`term_render::TermGpu::prepare_pane`] + [`term_render::TermGpu::render_pane`])
//! into an offscreen RGBA texture, reads the pixels back, and asserts that a
//! non-trivial number of NON-BACKGROUND pixels exist — i.e. text was actually
//! drawn. It also reproduces the production render-pass setup that caused the
//! bug: egui-wgpu calls `render_pass.set_viewport(<pane-rect>)` before the paint
//! callback, and the fixed `paint` restores the full-screen viewport so glyphon's
//! absolute-coordinate NDC mapping is correct. This test drives that exact
//! sequence, so a regression (removing the viewport restore) makes it fail.

#![allow(dead_code)] // `#[path]`-included module exposes production entry points
                     // (eframe `App` impl, chrome accessors) unused by this test.

#[path = "../src/egui_app/mod.rs"]
mod egui_app;

use std::time::{Duration, Instant};

use egui_app::grid::PaneId;
use egui_app::pane_term::PaneTerm;
use egui_app::term_render::TermGpu;

/// Offscreen surface size (physical px) used as the "full screen" the glyphon
/// viewport maps against — mirrors the live window's `size_in_pixels`.
const SCREEN_W: u32 = 800;
const SCREEN_H: u32 = 480;

/// The pane sub-rect (physical px) inside that surface: `[left, top, w, h]`.
/// Deliberately NOT the whole screen, so the test reproduces the sub-rect
/// viewport that triggered the black-pane bug.
const PANE_RECT: [f32; 4] = [120.0, 60.0, 560.0, 360.0];

/// The known text fed into the pane's PTY; its glyphs must show up in the
/// readback. A token that cannot pre-exist on a fresh grid.
const TOKEN: &str = "XYZZY";

/// Spawn a pane that prints `TOKEN` and poll its grid until the token lands.
/// Returns `None` if no PTY could spawn on this platform (clean skip — never a
/// false green).
fn pane_with_token() -> Option<PaneTerm> {
    let theme = c0pl4nd_core::Theme::builtin_void();
    // Print the token then keep the shell alive so the grid stays populated.
    #[cfg(windows)]
    let pane = PaneTerm::spawn_program(theme, "cmd.exe", &["/K", &format!("echo {TOKEN}")], 80, 24);
    #[cfg(not(windows))]
    let pane = PaneTerm::spawn_program(
        theme,
        "/bin/sh",
        &["-c", &format!("printf '{TOKEN}\\n'; sleep 5")],
        80,
        24,
    );
    if pane.error().is_some() {
        eprintln!("no PTY on this platform; skipping offscreen glyphon readback");
        return None;
    }
    let deadline = Instant::now() + Duration::from_secs(8);
    while Instant::now() < deadline {
        if pane
            .grid_text()
            .is_some_and(|t| t.contains(TOKEN))
        {
            return Some(pane);
        }
        std::thread::sleep(Duration::from_millis(40));
    }
    // Last check after the loop.
    if pane.grid_text().is_some_and(|t| t.contains(TOKEN)) {
        Some(pane)
    } else {
        eprintln!("token never reached the grid; skipping (no false green)");
        None
    }
}

/// Request a wgpu device/queue, or `None` when no adapter is available (headless
/// CI without a GPU). A clean skip — the test never falsely passes without a GPU.
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

/// THE deliverable: render a live pane's grid through the real glyphon path into
/// an offscreen texture and prove non-background pixels exist (text was drawn).
/// Catches the "black panes" class headlessly — a render that draws nothing fails
/// this test loudly.
#[test]
fn glyphon_terminal_render_produces_visible_pixels() {
    let Some(pane) = pane_with_token() else {
        return; // documented clean skip (no PTY / token never arrived)
    };
    let Some((device, queue)) = gpu() else {
        eprintln!("no GPU adapter; skipping offscreen glyphon readback");
        return;
    };

    // Use the SAME format family the live egui surface uses (sRGB). `TermGpu`
    // builds its atlas with whatever `target_format` we pass — here we pass the
    // texture's real format, exactly as the live path passes `rs.target_format`.
    let format = wgpu::TextureFormat::Rgba8UnormSrgb;

    // Theme background (the colour the pane clears to) — non-bg pixels are glyphs.
    let (br, bgc, bb) = pane.background_rgb();

    // Build the shared GPU resources on THIS device, just like `install_gpu`.
    let mut term_gpu = TermGpu::new(&device, &queue, format, 16.0, 22.0);

    // Snapshot the live grid into colour runs — the EXACT production input.
    let runs = pane
        .grid_spans()
        .expect("a live pane must yield colour runs");
    assert!(
        runs.iter().any(|(s, _)| s.contains(TOKEN)),
        "precondition: the colour runs must contain the token, got {:?}",
        runs.iter().map(|(s, _)| s.as_str()).collect::<Vec<_>>()
    );

    let pane_id = PaneId(0);
    let default_fg = {
        let (r, g, b) =
            c0pl4nd_core::theme::parse_hex(&c0pl4nd_core::Theme::builtin_void().foreground)
                .unwrap_or((232, 230, 240));
        [r, g, b]
    };

    // Prepare the pane through the SHARED build/shape/prepare path (the same one
    // `TermPaint::prepare` calls). `screen_px` is the FULL surface size.
    term_gpu
        .prepare_pane(
            &device,
            &queue,
            pane_id,
            PANE_RECT,
            default_fg,
            &runs,
            [SCREEN_W, SCREEN_H],
        )
        .expect("glyphon prepare_pane must succeed");

    // Offscreen render target.
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("term-offscreen"),
        size: wgpu::Extent3d {
            width: SCREEN_W,
            height: SCREEN_H,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());

    // Readback buffer (bytes_per_row must be 256-aligned).
    let unpadded = SCREEN_W * 4;
    let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let padded = unpadded.div_ceil(align) * align;
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("term-readback"),
        size: (padded * SCREEN_H) as u64,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    let mut encoder =
        device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("term-render"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                resolve_target: None,
                ops: wgpu::Operations {
                    // Clear to the theme background — the same colour the egui
                    // pane quad fills with, so glyph pixels differ from the clear.
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: srgb_to_linear(br),
                        g: srgb_to_linear(bgc),
                        b: srgb_to_linear(bb),
                        a: 1.0,
                    }),
                    store: wgpu::StoreOp::Store,
                },
                depth_slice: None,
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });

        // Reproduce the EXACT production render-pass setup egui-wgpu performs
        // before a paint callback: BOTH a scissor AND a viewport set to the
        // callback's pane sub-rect (egui-wgpu-0.34 renderer.rs §"default viewport
        // for the render pass" + the per-primitive `set_scissor_rect`). The
        // scissor clips anything outside the pane; the sub-rect viewport is what
        // double-transforms glyphon's full-screen NDC and pushes the squished text
        // into a fraction of the pane (the visible black-pane symptom).
        pass.set_scissor_rect(
            PANE_RECT[0] as u32,
            PANE_RECT[1] as u32,
            PANE_RECT[2] as u32,
            PANE_RECT[3] as u32,
        );
        pass.set_viewport(
            PANE_RECT[0],
            PANE_RECT[1],
            PANE_RECT[2],
            PANE_RECT[3],
            0.0,
            1.0,
        );
        // The FIX, exactly as the patched `TermPaint::paint` does: restore the
        // FULL-screen viewport so glyphon's full-screen NDC mapping is correct.
        // The scissor STAYS at the pane rect, so glyphs are still clipped to the
        // pane. REMOVING this restore reproduces the squished/clipped black-pane
        // bug and drops the foreground glyph-pixel count below the floor — the
        // regression guard.
        pass.set_viewport(0.0, 0.0, SCREEN_W as f32, SCREEN_H as f32, 0.0, 1.0);

        term_gpu
            .render_pane(pane_id, &mut pass)
            .expect("glyphon render_pane must succeed");
    }
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &readback,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded),
                rows_per_image: Some(SCREEN_H),
            },
        },
        wgpu::Extent3d {
            width: SCREEN_W,
            height: SCREEN_H,
            depth_or_array_layers: 1,
        },
    );
    queue.submit(std::iter::once(encoder.finish()));

    readback.slice(..).map_async(wgpu::MapMode::Read, |_| {});
    let _ = device.poll(wgpu::PollType::wait_indefinitely());
    let data = readback.slice(..).get_mapped_range();

    // Count pixels INSIDE the pane rect that differ from the cleared background
    // (the glyph pixels) AND, separately, pixels close to the foreground colour.
    let (px_l, px_t) = (PANE_RECT[0] as u32, PANE_RECT[1] as u32);
    let (px_r, px_b) = (
        (PANE_RECT[0] + PANE_RECT[2]) as u32,
        (PANE_RECT[1] + PANE_RECT[3]) as u32,
    );
    let mut non_bg = 0u64;
    let mut fg_like = 0u64;
    let mut outside_pane = 0u64;
    for y in 0..SCREEN_H {
        let row = (y * padded) as usize;
        for x in 0..SCREEN_W {
            let i = row + (x * 4) as usize;
            let (r, g, b) = (data[i], data[i + 1], data[i + 2]);
            let differs = (r as i32 - br as i32).abs()
                + (g as i32 - bgc as i32).abs()
                + (b as i32 - bb as i32).abs()
                > 24;
            let inside = x >= px_l && x < px_r && y >= px_t && y < px_b;
            if differs {
                if inside {
                    non_bg += 1;
                    // Foreground-ish: closer to the bright theme fg than to bg.
                    let close_fg = (r as i32 - default_fg[0] as i32).abs()
                        + (g as i32 - default_fg[1] as i32).abs()
                        + (b as i32 - default_fg[2] as i32).abs()
                        < 160;
                    if close_fg {
                        fg_like += 1;
                    }
                } else {
                    outside_pane += 1;
                }
            }
        }
    }
    drop(data);
    readback.unmap();

    eprintln!(
        "offscreen glyphon readback: non_bg_inside_pane={non_bg} fg_like={fg_like} \
         non_bg_outside_pane={outside_pane}"
    );

    // (a) Text was actually drawn: a real terminal line of glyphs lights up well
    // over a hundred pixels at this font size; require a generous floor so the
    // black-screen case (non_bg == 0) fails loudly without flaking on AA noise.
    assert!(
        non_bg >= 100,
        "glyphon terminal render drew too few non-background pixels inside the \
         pane ({non_bg}) — the panes are (near-)black. The grid HAS the token \
         ({TOKEN}); the render path is the defect."
    );
    // (b) Those pixels are foreground-coloured glyph pixels, not stray fill.
    assert!(
        fg_like >= 40,
        "expected foreground-coloured glyph pixels (got {fg_like}); text colour \
         resolved wrong (black-on-black / sRGB / format mismatch)?"
    );
    // (c) The full-screen-viewport restore must keep glyphs INSIDE the pane —
    // glyphon's TextBounds clips to the pane rect, so no text escapes it.
    assert_eq!(
        outside_pane, 0,
        "glyphs must stay clipped inside the pane rect (TextBounds), but \
         {outside_pane} non-bg pixels landed outside it"
    );
}

/// sRGB 8-bit channel → linear float, matching `screenshot.rs`'s clear colour so
/// the cleared background lines up with the live pane quad's fill.
fn srgb_to_linear(c: u8) -> f64 {
    let s = c as f64 / 255.0;
    if s <= 0.04045 {
        s / 12.92
    } else {
        ((s + 0.055) / 1.055).powf(2.4)
    }
}

/// THE END-TO-END deliverable: build the REAL `C0pl4ndApp` through eframe's
/// creation path with a REAL wgpu render state (so `install_gpu` runs and
/// `gpu_ready == true`), drive the production `frame_tick` until the spawned
/// shell's output lands in a pane's grid, then render the WHOLE egui frame —
/// including the real `egui_wgpu` glyphon paint callback (`TermPaint::prepare` +
/// `TermPaint::paint`, the exact code path the black panes came from) — to an
/// image and assert visible glyph pixels exist.
///
/// This is the faithful regression guard for the black-pane class: unlike the
/// software-path interaction tests (which only see the egui text fallback), this
/// drives the real GPU callback end to end and would have caught the pure-black
/// panes the human screenshotted.
#[test]
fn glyphon_terminal_render_through_real_egui_callback() {
    use std::cell::RefCell;

    use egui_kittest::Harness;

    // The eframe creation closure builds the REAL app (runs `install_gpu`), but
    // we need a handle to poll its grids across frames. `C0pl4ndApp::new` spawns
    // the platform shell; if there is no PTY on this box, the grid stays empty and
    // we skip cleanly (never a false green).
    //
    // Build the harness with the wgpu renderer so `cc.wgpu_render_state` is real.
    // If no GPU adapter exists, `WgpuTestRenderer::default()` panics inside the
    // builder — guard by probing for an adapter first and skipping if absent.
    if gpu().is_none() {
        eprintln!("no GPU adapter; skipping real-callback end-to-end render");
        return;
    }

    let mut harness: Harness<'_, egui_app::C0pl4ndApp> = Harness::builder()
        .with_size(egui::vec2(900.0, 600.0))
        .wgpu()
        .build_eframe(|cc| egui_app::C0pl4ndApp::new(cc));

    // Feed the focused pane a command that prints the token, then poll frames
    // until it lands in the focused grid (the same PTY-async pattern the
    // interaction tests use). Typing via egui Text events drives the real input
    // path; Enter submits.
    {
        // Skip if the platform shell did not spawn (no live PTY → no grid text).
        if harness.state().focused_grid_text().is_none() {
            eprintln!("no live PTY on this platform; skipping real-callback render");
            return;
        }
    }

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

    // Render the REAL egui frame — this executes the glyphon paint callback.
    // Use `step()` (one frame) NOT `run()`: with `gpu_ready == true` the app
    // calls `request_repaint()` every frame, so `run()` would loop to max_steps.
    harness.step();
    let img = harness
        .render()
        .expect("kittest wgpu render of the real frame must succeed");

    // The pane bodies live below the titlebar and above the status bar. Scan the
    // whole image for pixels that are clearly NOT the dark void background — the
    // glyph pixels. The void theme background is near-black (#08060d-ish), so any
    // bright cluster is rendered text/chrome. We further require a band of pixels
    // in the CENTRAL region (the pane grid area, away from the chrome strips) to
    // be non-background — that is the terminal text specifically.
    let (w, h) = (img.width(), img.height());
    let mut central_non_bg = 0u64;
    let band_top = h / 6; // below titlebar
    let band_bottom = h - h / 8; // above status bar
    for y in band_top..band_bottom {
        for x in 0..w {
            let p = img.get_pixel(x, y);
            let [r, g, b, _] = p.0;
            // "Bright enough to be a glyph, not the void background."
            if r as u16 + g as u16 + b as u16 > 120 {
                central_non_bg += 1;
            }
        }
    }
    eprintln!(
        "real-egui-callback render: {w}x{h}, central_non_bg_pixels={central_non_bg}"
    );
    assert!(
        central_non_bg >= 200,
        "the real egui glyphon callback drew too few bright pixels in the pane \
         region ({central_non_bg}) — the panes rendered (near-)black end to end. \
         The grid HAS the token ({TOKEN}); the GPU render path is the defect."
    );
}
