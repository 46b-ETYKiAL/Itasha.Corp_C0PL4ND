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

#![allow(dead_code)]

#[path = "../src/egui_app/mod.rs"]
mod egui_app;
#[path = "../src/issue_intake.rs"]
mod issue_intake;
#[path = "../src/reporting.rs"]
mod reporting;
#[path = "../src/user_error.rs"]
mod user_error;

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
