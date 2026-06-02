//! Binary entry point for `c0pl4nd` — the modern eframe/egui app (the canonical
//! binary). The legacy winit-driven terminal (`src/main.rs`) ships as
//! `c0pl4nd-legacy`; both build side by side.
//!
//! eframe owns the winit event loop — there is no second event loop here, and
//! no Win32 window subclass (window controls go through `ViewportCommand`).

// Release builds are a GUI subsystem app: no extra console window pops up
// alongside the window (a debug build keeps the console so tracing/wgpu logs
// are visible). Without this, launching the installed app spawns a second
// "terminal" window showing the wgpu/Vulkan INFO log spam.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
// The egui chrome shell is pure-safe Rust; no `unsafe` FFI lives in this binary
// (the `win_snap.rs` Win32 subclass is NOT compiled into `c0pl4nd-egui`).
#![forbid(unsafe_code)]

#[path = "egui_app/mod.rs"]
mod egui_app;

use eframe::egui;

fn main() -> eframe::Result<()> {
    // Best-effort tracing; the env filter mirrors the legacy binary.
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .try_init();

    let mut viewport = egui::ViewportBuilder::default()
        .with_inner_size([1100.0, 720.0])
        .with_min_inner_size([520.0, 360.0])
        .with_app_id("com.itashacorp.c0pl4nd")
        .with_title("C0PL4ND")
        .with_decorations(false) // frameless — we draw our own titlebar
        .with_transparent(true); // required for rounded corners + acrylic blur
                                 // Runtime window + taskbar icon (the sigil). The exe's embedded icon
                                 // resource (build.rs) covers the Start-menu shortcut / Explorer /
                                 // Add-Remove-Programs; this covers the live window. Best-effort — a decode
                                 // failure leaves the platform default rather than blocking startup.
    if let Some(icon) = load_app_icon() {
        viewport = viewport.with_icon(std::sync::Arc::new(icon));
    }

    let options = eframe::NativeOptions {
        viewport,
        // Keep the wgpu backend (default via the `wgpu` feature); do NOT enable
        // glow — glyphon (Milestone 2) shares egui's wgpu device.
        ..Default::default()
    };

    eframe::run_native(
        "C0PL4ND",
        options,
        Box::new(|cc| Ok(Box::new(egui_app::C0pl4ndApp::new(cc)))),
    )
}

/// Decode the embedded sigil PNG into an eframe window icon. Returns `None` on a
/// decode failure (the caller falls back to the platform default).
fn load_app_icon() -> Option<egui::IconData> {
    // `packaging/windows/c0pl4nd-256.png` is the 256px sigil (same mark as the
    // embedded `.ico`), included at compile time so the icon ships in the binary.
    const PNG: &[u8] = include_bytes!("../../../packaging/windows/c0pl4nd-256.png");
    let img = image::load_from_memory(PNG).ok()?.into_rgba8();
    let (width, height) = img.dimensions();
    Some(egui::IconData {
        rgba: img.into_raw(),
        width,
        height,
    })
}
