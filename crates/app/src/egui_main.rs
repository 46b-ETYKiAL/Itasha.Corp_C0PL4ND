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
#[path = "update/mod.rs"]
mod update;

use eframe::egui;

fn main() -> eframe::Result<()> {
    // Best-effort tracing; the env filter mirrors the legacy binary.
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .try_init();

    let args: Vec<String> = std::env::args().collect();

    // `c0pl4nd --version` — print and exit (no window).
    if args.iter().any(|a| a == "--version" || a == "-V") {
        println!("{} {}", c0pl4nd_core::PRODUCT_NAME, c0pl4nd_core::version());
        return Ok(());
    }

    // `c0pl4nd update` — explicit, user-initiated update check (no window). Reads
    // the configured channel from the persisted config, defaulting to stable.
    if args.iter().any(|a| a == "update") {
        let channel = c0pl4nd_core::Config::default_path()
            .filter(|p| p.exists())
            .and_then(|p| {
                std::fs::read_to_string(&p)
                    .ok()
                    .and_then(|s| c0pl4nd_core::Config::from_toml(&s, &p).ok())
            })
            .map(|c| c.update.channel)
            .unwrap_or_else(|| "stable".to_string());
        // `run_update` is offline-graceful and only prints; surface any hard
        // error to stderr but still exit 0 (a failed check is not a crash).
        if let Err(e) = update::run_update(&channel) {
            eprintln!("c0pl4nd update: {e}");
        }
        return Ok(());
    }

    // The window position + size are persisted natively by eframe via the
    // `persistence` feature + `NativeOptions.persist_window` below (ron state
    // stored under the stable `with_app_id` folder). We set only the FIRST-RUN
    // default size here; eframe restores the user's last size on later launches.
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

    let mut options = eframe::NativeOptions {
        viewport,
        // Persist native window position + size across restarts (#24/#25): pairs
        // with the eframe `persistence` feature + the stable `with_app_id` above.
        // eframe stores the geometry (and any `App`-saved state) as ron under the
        // app_id folder and restores it on the next launch; it also fires
        // `App::save()` on exit/interval once persistence is on. The settings
        // `egui::Window` (a resizable Window with a stable Id) likewise has its
        // size persisted automatically by this feature — no hand-rolled plumbing.
        persist_window: true,
        // Keep the wgpu backend (default via the `wgpu` feature); do NOT enable
        // glow — glyphon (Milestone 2) shares egui's wgpu device.
        ..Default::default()
    };
    prefer_backend_on_windows(&mut options, launch_transparency_enabled());

    eframe::run_native(
        "C0PL4ND",
        options,
        Box::new(|cc| {
            let mut app = egui_app::C0pl4ndApp::new(cc);
            // Opt-in, local-first launch update check. The ONE network call runs
            // on a background thread so startup never blocks; the app polls the
            // channel each frame and surfaces a found update as a toast. Off by
            // default — fires only for the network-on-launch update modes
            // (`notify`/`auto`) OR the legacy `check_on_launch` flag. The Updates
            // settings page owns the richer in-app download/install flow; this
            // launch path is the lightweight "a newer version exists" notice.
            let (should_check, channel) = launch_check_config();
            if should_check {
                let (tx, rx) = std::sync::mpsc::channel();
                let ctx = cc.egui_ctx.clone();
                std::thread::spawn(move || {
                    if let Some(notice) = update::check_for_update(&channel) {
                        let _ = tx.send(notice);
                        ctx.request_repaint(); // wake the UI to show the toast
                    }
                });
                app.attach_update_check(rx);
            }
            Ok(Box::new(app))
        }),
    )
}

/// Decide whether to run the lightweight on-launch update check and which
/// release channel to query. Reads the persisted config directly (the same
/// load path the `c0pl4nd update` CLI subcommand uses) so the decision honours
/// the canonical `[update] mode` (`notify`/`auto` check on launch) as well as
/// the legacy `check_on_launch` flag, without depending on the host app's
/// accessor. Defaults to (false, "stable") when no config exists — local-first.
fn launch_check_config() -> (bool, String) {
    c0pl4nd_core::Config::default_path()
        .filter(|p| p.exists())
        .and_then(|p| {
            std::fs::read_to_string(&p)
                .ok()
                .and_then(|s| c0pl4nd_core::Config::from_toml(&s, &p).ok())
        })
        .map(|c| (c.update.checks_on_launch(), c.update.channel))
        .unwrap_or_else(|| (false, "stable".to_string()))
}

/// Whether the persisted config has window transparency enabled — read at launch
/// to pick a transparency-capable wgpu backend (see [`prefer_backend_on_windows`]).
/// Defaults to `false` (opaque) when no config exists, matching the app default.
fn launch_transparency_enabled() -> bool {
    c0pl4nd_core::Config::default_path()
        .filter(|p| p.exists())
        .and_then(|p| {
            std::fs::read_to_string(&p)
                .ok()
                .and_then(|s| c0pl4nd_core::Config::from_toml(&s, &p).ok())
        })
        .map(|c| c.effective_translucent())
        .unwrap_or(false)
}

/// Choose the wgpu backend on Windows.
///
/// **Real window transparency requires the Vulkan backend.** A wgpu swapchain
/// bound to a plain Win32 HWND through DX12/DXGI cannot per-pixel alpha-composite
/// with the desktop — `CreateSwapChainForHwnd` forces `DXGI_ALPHA_MODE_UNSPECIFIED`
/// (opaque to DWM), so `with_transparent(true)` + `clear_color=[0,0,0,0]` is a
/// silent no-op and the `window-vibrancy` acrylic/mica backdrop is fully occluded
/// by the opaque swapchain (the "solid dark box" transparency bug). Vulkan's WSI
/// DOES expose `VK_COMPOSITE_ALPHA_PRE_MULTIPLIED`, so a see-through / acrylic
/// window only works on Vulkan — empirically verified on Win11 (and the reason the
/// sibling SCR1B3, which uses the default Vulkan-first backend, is see-through).
///
/// The trade-off: some third-party Vulkan *overlay layers* (e.g.
/// `GalaxyOverlayVkLayer`) corrupt the Vulkan instance and panic egui-wgpu, which
/// is why the OPAQUE path keeps the more robust DX12. So: when the user has
/// enabled window transparency we select Vulkan; otherwise DX12. `WGPU_BACKEND`
/// always overrides (a user hitting a Vulkan-overlay crash can force `dx12`,
/// trading transparency for stability). No-op on non-Windows platforms.
fn prefer_backend_on_windows(options: &mut eframe::NativeOptions, want_transparency: bool) {
    #[cfg(target_os = "windows")]
    {
        use eframe::wgpu::Backends;
        if let eframe::egui_wgpu::WgpuSetup::CreateNew(setup) = &mut options.wgpu_options.wgpu_setup
        {
            let default = if want_transparency {
                Backends::VULKAN
            } else {
                Backends::DX12
            };
            setup.instance_descriptor.backends = Backends::from_env().unwrap_or(default);
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = want_transparency;
        let _ = options; // used on every platform; backend default is correct off Windows
    }
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
