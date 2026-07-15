//! On-demand VISUAL-QA snapshots: render the REAL C0pl4ndApp egui frame in
//! several states to PNGs so a human — or an agent with image-reading — can
//! EYEBALL the rendering that cannot be asserted from accessibility/grid state
//! alone.
//!
//! Run on demand (needs a real GPU; skips cleanly otherwise):
//!   cargo test -p c0pl4nd --test qa_wide_glyph_snapshot -- --ignored --nocapture
//! Each test prints the absolute PNG path it wrote. These are NOT CI gates (they
//! are `#[ignore]`d and pixel output is non-deterministic across GPU drivers —
//! pixel snapshots are deliberately never gated); they are deliberate visual-QA
//! aids.

use c0pl4nd::egui_app;
use std::time::{Duration, Instant};

use egui_kittest::kittest::Queryable;
use egui_kittest::Harness;

/// Probe for a usable wgpu adapter; `false` → skip (never a false green).
fn gpu_available() -> bool {
    let instance = wgpu::Instance::default();
    pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::LowPower,
        compatible_surface: None,
        force_fallback_adapter: false,
    }))
    .is_ok()
}

/// Build a real-wgpu harness over the production `C0pl4ndApp`, or `None` if no
/// GPU adapter is present on this host.
fn build() -> Option<Harness<'static, egui_app::C0pl4ndApp>> {
    if !gpu_available() {
        eprintln!("QA-SNAPSHOT: no GPU adapter on this host; skipping (not a failure).");
        return None;
    }
    Some(
        Harness::builder()
            .with_size(egui::vec2(1100.0, 720.0))
            .wgpu()
            .build_eframe(|cc| egui_app::C0pl4ndApp::new(cc)),
    )
}

/// Render the current frame and save it to `%TEMP%/c0pl4nd-qa-<name>.png`.
fn snapshot(h: &mut Harness<'_, egui_app::C0pl4ndApp>, name: &str) {
    h.step();
    let img = h.render().expect("kittest wgpu render must succeed");
    let out = std::env::temp_dir().join(format!("c0pl4nd-qa-{name}.png"));
    img.save(&out).expect("save QA snapshot PNG");
    eprintln!(
        "QA-SNAPSHOT[{name}]: {}x{} -> {}",
        img.width(),
        img.height(),
        out.display()
    );
}

/// Send a Ctrl+Shift+<key> chord (the default keybinding modifier on this host).
fn chord(h: &mut Harness<'_, egui_app::C0pl4ndApp>, key: egui::Key) {
    h.event(egui::Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers {
            ctrl: true,
            shift: true,
            ..Default::default()
        },
    });
    for _ in 0..3 {
        h.step();
    }
}

/// Type a line into the focused pane and submit it, then poll for it to land.
fn type_line(h: &mut Harness<'_, egui_app::C0pl4ndApp>, line: &str, needle: &str) {
    if h.state().focused_grid_text().is_none() {
        return;
    }
    for ch in line.chars() {
        h.event(egui::Event::Text(ch.to_string()));
    }
    h.step();
    h.key_press(egui::Key::Enter);
    h.step();
    let deadline = Instant::now() + Duration::from_secs(8);
    while Instant::now() < deadline {
        h.step();
        if h.state()
            .focused_grid_text()
            .is_some_and(|t| t.contains(needle))
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(40));
    }
}

#[test]
#[ignore = "visual-QA aid: needs a real GPU; run explicitly with --ignored"]
fn qa_launch_frame() {
    let Some(mut h) = build() else { return };
    // A few frames to let the shell banner + prompt land.
    for _ in 0..30 {
        h.step();
        std::thread::sleep(Duration::from_millis(20));
    }
    snapshot(&mut h, "launch");
}

#[test]
#[ignore = "visual-QA aid: needs a real GPU; run explicitly with --ignored"]
fn qa_launch_frame_hidpi() {
    // Reproduce a 1.5x HiDPI display (the reported garble machine) — the default
    // qa harness renders at ppp 1.0, which never reproduced it.
    if !gpu_available() {
        eprintln!("QA-SNAPSHOT: no GPU adapter on this host; skipping (not a failure).");
        return;
    }
    let mut h: Harness<'static, egui_app::C0pl4ndApp> = Harness::builder()
        .with_size(egui::vec2(1100.0, 720.0))
        .with_pixels_per_point(1.5)
        .wgpu()
        .build_eframe(|cc| egui_app::C0pl4ndApp::new(cc));
    for _ in 0..30 {
        h.step();
        std::thread::sleep(Duration::from_millis(20));
    }
    snapshot(&mut h, "launch-hidpi");
}

#[test]
#[ignore = "visual-QA aid: needs a real GPU; run explicitly with --ignored"]
fn qa_wide_glyph_frame() {
    let Some(mut h) = build() else { return };
    type_line(&mut h, "echo ASCII | 日本語 | ＡＢＣ | 😀 | end", "end");
    snapshot(&mut h, "wide-glyph");
}

#[test]
#[ignore = "visual-QA aid: needs a real GPU; run explicitly with --ignored"]
fn qa_settings_page() {
    let Some(mut h) = build() else { return };
    for _ in 0..10 {
        h.step();
    }
    // The gear caption button is labelled "settings" in the AccessKit tree.
    h.get_by_label("settings").click();
    for _ in 0..4 {
        h.step();
    }
    snapshot(&mut h, "settings");
}

#[test]
#[ignore = "visual-QA aid: needs a real GPU; run explicitly with --ignored"]
fn qa_toolbar_settings_page() {
    let Some(mut h) = build() else { return };
    for _ in 0..10 {
        h.step();
    }
    h.get_by_label("settings").click();
    for _ in 0..4 {
        h.step();
    }
    // Select the Toolbar category to render the toolbar editor (icons must NOT be
    // tofu; every row must show the reorder arrows + X remove + move menu).
    h.get_by_label("Toolbar").click();
    for _ in 0..4 {
        h.step();
    }
    snapshot(&mut h, "toolbar-settings");
}

#[test]
#[ignore = "visual-QA aid: needs a real GPU; run explicitly with --ignored"]
fn qa_motion_settings_page() {
    let Some(mut h) = build() else { return };
    for _ in 0..10 {
        h.step();
    }
    h.get_by_label("settings").click();
    for _ in 0..4 {
        h.step();
    }
    // Select the Motion category to render the regrouped Motion page — the four
    // grouped sections (master / CRT screen / Ambient node-mesh / Tape & motion
    // accents) with checkbox-left rows and the new movement/intensity sliders.
    h.get_by_label("Motion").click();
    for _ in 0..4 {
        h.step();
    }
    snapshot(&mut h, "motion-settings");
}

#[test]
#[ignore = "visual-QA aid: needs a real GPU; run explicitly with --ignored"]
fn qa_command_palette() {
    let Some(mut h) = build() else { return };
    for _ in 0..10 {
        h.step();
    }
    chord(&mut h, egui::Key::P); // Ctrl+Shift+P
    snapshot(&mut h, "palette");
}

#[test]
#[ignore = "visual-QA aid: needs a real GPU; run explicitly with --ignored"]
fn qa_find_overlay() {
    let Some(mut h) = build() else { return };
    type_line(&mut h, "echo findme_token_123", "findme");
    chord(&mut h, egui::Key::F); // Ctrl+Shift+F opens the find overlay
    for ch in "findme".chars() {
        h.event(egui::Event::Text(ch.to_string()));
    }
    snapshot(&mut h, "find");
}

#[test]
#[ignore = "visual-QA aid: needs a real GPU; run explicitly with --ignored"]
fn qa_split_panes() {
    let Some(mut h) = build() else { return };
    for _ in 0..10 {
        h.step();
    }
    chord(&mut h, egui::Key::D); // Ctrl+Shift+D — split right
    for _ in 0..6 {
        h.step();
    }
    snapshot(&mut h, "split");
}

/// Build a real-wgpu harness whose live config has window-transparency ON with a
/// strong background TINT at a LOW opacity — the exact state the tint/transparency
/// fixes must be eyeballed in. `new(cc)` loads the persisted config, then we
/// override just the transparency fields for the QA render (mirrors the user
/// dialling Settings → Appearance). `None` when no GPU adapter is present.
fn build_tinted(
    opacity: f32,
    tint_strength: f32,
) -> Option<Harness<'static, egui_app::C0pl4ndApp>> {
    if !gpu_available() {
        eprintln!("QA-SNAPSHOT: no GPU adapter on this host; skipping (not a failure).");
        return None;
    }
    Some(
        Harness::builder()
            .with_size(egui::vec2(1100.0, 720.0))
            .wgpu()
            .build_eframe(move |cc| {
                let mut app = egui_app::C0pl4ndApp::new(cc);
                // Single always-transparent model: the opacity slider is the whole
                // see-through control; a low opacity + a strong tint is the state to
                // eyeball the tint/transparency fixes in.
                app.config.opacity = opacity;
                app.config.tint = "#ff0040".to_string();
                app.config.tint_enabled = true;
                app.config.tint_strength = tint_strength;
                app
            }),
    )
}

/// VISUAL-QA: window transparency ON + strong red tint at LOW opacity, split into
/// two panes. Eyeball checklist against the tint/transparency fixes:
///   1. the TINT wash reaches the panes, the gap/divider between them, AND the
///      top bar + status bar UNIFORMLY (not just the pane backgrounds);
///   2. the terminal TEXT is NOT reddened (tint is behind the glyphs);
///   3. at this low opacity the background reads as clearly see-through.
#[test]
#[ignore = "visual-QA aid: needs a real GPU; run explicitly with --ignored"]
fn qa_tint_transparent_low_opacity() {
    let Some(mut h) = build_tinted(0.10, 0.8) else {
        return;
    };
    for _ in 0..10 {
        h.step();
    }
    chord(&mut h, egui::Key::D); // split right → two panes + a divider to inspect
    for _ in 0..8 {
        h.step();
    }
    snapshot(&mut h, "tint-transparent-low-opacity");
}

/// VISUAL-QA: with the SAME tint/transparency config, open Settings. The Settings
/// window MUST stay solid + readable (opaque) and MUST NOT be washed red — the
/// reported "settings window is tinted/transparent" bug.
#[test]
#[ignore = "visual-QA aid: needs a real GPU; run explicitly with --ignored"]
fn qa_tint_settings_stays_opaque() {
    let Some(mut h) = build_tinted(0.10, 0.8) else {
        return;
    };
    for _ in 0..10 {
        h.step();
    }
    h.get_by_label("settings").click();
    for _ in 0..4 {
        h.step();
    }
    snapshot(&mut h, "tint-settings-opaque");
}

/// Render the real app with a config mutation applied, or `None` without a GPU.
fn render_with(
    mutate: impl FnOnce(&mut c0pl4nd_core::Config) + 'static,
) -> Option<image::RgbaImage> {
    if !gpu_available() {
        eprintln!("no GPU adapter; skipping (not a failure).");
        return None;
    }
    let mut h = Harness::builder()
        .with_size(egui::vec2(1100.0, 720.0))
        .wgpu()
        .build_eframe(move |cc| {
            let mut app = egui_app::C0pl4ndApp::new(cc);
            mutate(&mut app.config);
            app
        });
    for _ in 0..5 {
        h.step();
    }
    Some(h.render().expect("render"))
}

/// The modal non-zero alpha (and its pixel count) over the pane band — the alpha
/// the terminal BACKING composited to, ignoring the fully-transparent pixels.
fn modal_pane_alpha(img: &image::RgbaImage) -> (u8, u64) {
    let (w, hgt) = (img.width(), img.height());
    let mut hist = [0u64; 256];
    for y in (hgt / 6)..(hgt - hgt / 8) {
        for x in 0..w {
            hist[img.get_pixel(x, y).0[3] as usize] += 1;
        }
    }
    hist.iter()
        .enumerate()
        .skip(1) // ignore fully-transparent
        .max_by_key(|(_, n)| **n)
        .map(|(a, n)| (a as u8, *n))
        .unwrap_or((0, 0))
}

/// Fraction (%) of the WHOLE window that is non-transparent.
fn nonzero_pct(img: &image::RgbaImage) -> f64 {
    let (w, hgt) = (img.width(), img.height());
    let mut nz = 0u64;
    for y in 0..hgt {
        for x in 0..w {
            if img.get_pixel(x, y).0[3] != 0 {
                nz += 1;
            }
        }
    }
    nz as f64 / (u64::from(w) * u64::from(hgt)) as f64 * 100.0
}

/// REGRESSION GUARD (needs a GPU): with the tint AND frost OFF and no ambient
/// effects, opacity 0 is a genuinely CLEAR window — the whole surface reaches
/// alpha 0 except the sparse, opaque glyph text. Proves the clean-glass path
/// (opacity purely controls see-through; nothing hazes it when tint/frost are off).
#[test]
#[ignore = "needs a real GPU; run with --ignored"]
fn opacity_zero_is_clear_when_tint_and_frost_off() {
    let Some(img) = render_with(|c| {
        c.opacity = 0.0;
        c.tint_enabled = false;
        c.frost_enabled = false;
        c.effects.wired_ambient = false;
    }) else {
        return;
    };
    let pct = nonzero_pct(&img);
    assert!(
        pct < 3.0,
        "opacity-0 clean glass must be ~fully clear: {pct:.2}% non-transparent \
         (expected < 3% — only sparse glyph text + focus ring)"
    );
}

/// GUARD (needs a GPU): the node mesh is INDEPENDENT of the window opacity — it
/// no longer fades with the Opacity slider. At opacity 0 (fully see-through) the
/// mesh must STILL paint a visible lattice over the desktop (it used to be scaled
/// to nothing by `opacity`), so turning the mesh on adds clearly more non-
/// transparent pixels than the mesh-off clean-glass baseline.
#[test]
#[ignore = "needs a real GPU; run with --ignored"]
fn mesh_shows_at_opacity_zero_independent_of_opacity() {
    let Some(off) = render_with(|c| {
        c.opacity = 0.0;
        c.tint_enabled = false;
        c.frost_enabled = false;
        c.effects.wired_ambient = false;
    }) else {
        return;
    };
    let on = render_with(|c| {
        c.opacity = 0.0; // fully transparent glass …
        c.tint_enabled = false;
        c.frost_enabled = false;
        c.effects.animations_enabled = true;
        c.effects.wired_ambient = true; // … yet the mesh still paints
        c.effects.mesh_density = 1.5;
        c.effects.mesh_brightness = 2.0;
    })
    .expect("GPU was available for the first render");
    let (off_pct, on_pct) = (nonzero_pct(&off), nonzero_pct(&on));
    eprintln!("mesh off at opacity0 = {off_pct:.2}%, mesh on = {on_pct:.2}%");
    assert!(
        on_pct > off_pct + 1.0,
        "the mesh must remain visible at opacity 0 (independent of opacity): \
         off={off_pct:.2}% vs on={on_pct:.2}% — the mesh was scaled away by opacity"
    );
}

/// PART 1 GUARD (needs a GPU): the terminal background is painted ONCE, so the
/// pane-backing alpha is LINEAR in opacity — a single 0.7 opacity yields ≈179
/// (0.7·255), NOT the ≈125 (0.7²·255) it would if the CentralPanel fill and a
/// per-pane fill both painted at the opacity alpha and COMPOUNDED (the haze bug).
#[test]
#[ignore = "needs a real GPU; run with --ignored"]
fn pane_backing_alpha_is_linear_single_paint() {
    let Some(img) = render_with(|c| {
        c.opacity = 0.7;
        c.tint_enabled = false;
        c.frost_enabled = false;
        c.effects.wired_ambient = false;
    }) else {
        return;
    };
    let (modal, _) = modal_pane_alpha(&img);
    let linear = (0.7 * 255.0_f32).round() as i32; // 179
    let squared = (0.7 * 0.7 * 255.0_f32).round() as i32; // 125
    eprintln!("pane backing modal alpha = {modal} (linear≈{linear}, squared≈{squared})");
    assert!(
        (i32::from(modal) - linear).abs() <= 8,
        "opacity 0.7 backing must be LINEAR (~{linear}), got {modal} — a value near \
         {squared} would mean the background is still painted twice (compounding)"
    );
}

/// PART 2 GUARD (needs a GPU): the frosted-glass wash is painted ONLY when
/// enabled, and it visibly thickens the backing (independent of opacity). At a
/// see-through opacity, turning frost ON must raise the pane-backing alpha well
/// above the frost-OFF baseline.
#[test]
#[ignore = "needs a real GPU; run with --ignored"]
fn frost_wash_appears_only_when_enabled() {
    let Some(off) = render_with(|c| {
        c.opacity = 0.3;
        c.tint_enabled = false;
        c.frost_enabled = false;
    }) else {
        return;
    };
    let on = render_with(|c| {
        c.opacity = 0.3;
        c.tint_enabled = false;
        c.frost_enabled = true;
        c.frost_amount = 0.6;
        c.frost_grain = false; // flat wash for a deterministic alpha comparison
    })
    .expect("GPU was available for the first render");
    let (off_a, _) = modal_pane_alpha(&off);
    let (on_a, _) = modal_pane_alpha(&on);
    eprintln!("frost off backing alpha = {off_a}, frost on = {on_a}");
    assert!(
        i32::from(on_a) >= i32::from(off_a) + 40,
        "enabling frost must visibly thicken the backing wash (off={off_a}, on={on_a})"
    );
}
