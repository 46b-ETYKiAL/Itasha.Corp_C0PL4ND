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
