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
// The egui chrome shell is pure-safe Rust: `deny(unsafe_code)` keeps ALL of this
// binary's own UI code unsafe-free. The SOLE exception is the audited
// `dll_hardening` platform-init module (it must call two Win32 search-order
// functions); it opts back in with a narrowly-scoped `#![allow(unsafe_code)]`.
// `deny` (not `forbid`) is required precisely so that one vetted module can
// override it — `forbid` is unconditional and would reject the submodule.
#![deny(unsafe_code)]

// mimalloc as the global allocator (see crates/app/Cargo.toml) — the
// declaration is safe; no `unsafe` needed.
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

mod diagnostics;
mod dll_hardening;
#[path = "egui_app/mod.rs"]
mod egui_app;
// W1TN3SS opt-in reporting glue (Tier-1 crash spool + manual issue intake).
// Pure consumers of the pinned-tag `itasha-report-core` SDK; both default OFF.
mod issue_intake;
mod panic_hook;
mod reporting;
#[path = "update/mod.rs"]
mod update;
mod user_error;

use eframe::egui;

fn main() -> eframe::Result<()> {
    // FIRST statement: harden the Windows DLL search order before any other DLL
    // could be implicitly loaded, defeating DLL-planting when launched from an
    // untrusted directory (e.g. Downloads). Safe wrapper; the Win32 `unsafe` FFI
    // lives in `dll_hardening` so this `#![forbid(unsafe_code)]` binary stays
    // unsafe-free. No-op off Windows.
    dll_hardening::harden_dll_search_order();

    // Install the unexpected-panic crash hook early (before the window /
    // event-loop): `panic = "abort"` otherwise kills the GUI with no diagnostic.
    // The hook writes a rotating crash log (and, on Windows, shows a MessageBox)
    // then chains to the default hook — it runs before the abort fires.
    panic_hook::install();

    // Create the process-wide KILL_ON_JOB_CLOSE job object (Windows) so no PTY
    // child shell — or anything it spawns — can outlive this process, even on a
    // hard `std::process::exit(0)` or a crash. Best-effort; the fast-close
    // `kill_child` path is the primary no-orphan guarantee, this is the backstop.
    egui_app::job_object::init();

    // Best-effort tracing. Mirror the legacy binary EXACTLY (F9-2): read the
    // `C0PL4ND_LOG` env var (NOT the default `RUST_LOG`) and default to `warn`.
    // The two binaries previously diverged — this one read `RUST_LOG` and
    // defaulted to the noisier `info` — so a user setting `C0PL4ND_LOG` saw it
    // honoured by the legacy binary but ignored by the canonical one, which also
    // logged at `info` in release. Both now share one contract: C0PL4ND_LOG/warn.
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("C0PL4ND_LOG")
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .try_init();

    let args: Vec<String> = std::env::args().collect();

    // `c0pl4nd --version` — print and exit (no window).
    if args.iter().any(|a| a == "--version" || a == "-V") {
        println!("{} {}", c0pl4nd_core::PRODUCT_NAME, c0pl4nd_core::version());
        return Ok(());
    }

    // `c0pl4nd --diagnostics` (alias `--doctor`) — print a one-shot env/config
    // dump and exit BEFORE any window/GPU init. IME text routing is always
    // compiled into the egui app (it handles `egui::Event::Text`).
    if diagnostics::requested(&args) {
        diagnostics::run(true, panic_hook::crash_log_dir());
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
            tracing::warn!(target: "c0pl4nd::update", detail = ?e, "update subcommand failed");
            eprintln!("Couldn't check for updates: an unexpected problem occurred.");
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
    prefer_backend_on_windows(
        &mut options,
        launch_transparency_enabled(),
        launch_backend_override(),
    );
    apply_gpu_preference(&mut options, launch_gpu_preference());

    // We KEEP eframe's default present latency (2). A previous
    // `desired_maximum_frame_latency = Some(1)` "optimization" (shave one frame
    // off keystroke→glyph lag) turned out to CORRUPT the terminal grid on real
    // hardware: at latency 1 the swapchain draw could race egui's font-atlas
    // texture upload, so glyphs rendered from a not-yet-complete atlas — badly
    // garbled on a fast discrete GPU (NVIDIA), mildly on an integrated one, and
    // NEVER in an offscreen/synchronous render (every headless snapshot was
    // pixel-perfect, which is what made it so hard to pin down). One extra frame
    // of latency is imperceptible for a terminal; a garbled grid is not. Do NOT
    // re-add the latency-1 override without proving the atlas upload is
    // synchronised first. We also deliberately keep the default vsync present
    // mode (NOT Mailbox/AutoNoVsync): the app is event-driven, so an idle
    // terminal repaints ~0 fps and Mailbox would force wasteful continuous
    // high-FPS rendering.

    let result = eframe::run_native(
        "C0PL4ND",
        options,
        Box::new(|cc| {
            let mut app = egui_app::C0pl4ndApp::new(cc);
            // Launch update check. The ONE network call runs on a background
            // thread so startup never blocks; the app polls the channel each
            // frame and surfaces a found update as a toast. Runs by default —
            // the default `notify` mode performs this on-launch check (as does
            // `auto`), plus the legacy `check_on_launch` flag; `manual`/`off`
            // suppress it. The check is throttled by `check_interval_hours`. The
            // Updates settings page owns the richer in-app download/install
            // flow; this launch path is the lightweight "newer version" notice.
            let (should_check, channel) = launch_check_config();
            if should_check {
                let (tx, rx) = std::sync::mpsc::channel();
                let ctx = cc.egui_ctx.clone();
                std::thread::spawn(move || {
                    if let Some(notice) = update::check_for_update(&channel) {
                        let _ = tx.send(notice);
                        ctx.request_repaint(); // wake the UI to show the toast
                    }
                    // Record the attempt (success OR failure) so the interval
                    // throttle suppresses the next launch's check until due.
                    update::record_check_now();
                });
                app.attach_update_check(rx);
            }
            Ok(Box::new(app))
        }),
    );

    // A GPU adapter/device or window-init failure comes back as a clean `Err`
    // (NOT a panic), so the panic hook never fires and a release GUI build — which
    // has no console — would otherwise show nothing at all. Surface it with a
    // diagnostic + a recovery hint before propagating the error.
    if let Err(e) = &result {
        panic_hook::show_startup_error("C0PL4ND couldn't start", &user_error::gpu_init_failed(e));
    }
    result
}

/// Decide whether to run the lightweight on-launch update check and which
/// release channel to query. Reads the persisted config directly (the same
/// load path the `c0pl4nd update` CLI subcommand uses) so the decision honours
/// the canonical `[update] mode` (`notify`/`auto` check on launch) as well as
/// the legacy `check_on_launch` flag, without depending on the host app's
/// accessor. When no config file exists yet (first-ever launch) the canonical
/// [`UpdateConfig`] default is used, so a brand-new user gets the same default
/// (`notify`) behaviour as one whose config has already been written — no
/// special-cased divergence.
fn launch_check_config() -> (bool, String) {
    // The configured update settings (from the persisted config, or the canonical
    // default when no config exists yet). A check runs only when the mode opts in
    // (`notify`/`auto` or the legacy flag) AND the interval throttle says it is due
    // — so the default `notify` mode does not hit the GitHub API on every launch.
    let update = c0pl4nd_core::Config::default_path()
        .filter(|p| p.exists())
        .and_then(|p| {
            std::fs::read_to_string(&p)
                .ok()
                .and_then(|s| c0pl4nd_core::Config::from_toml(&s, &p).ok())
        })
        .map(|c| c.update)
        .unwrap_or_else(|| c0pl4nd_core::Config::default().update);
    let should_check = update.checks_on_launch() && update::check_due(update.check_interval_hours);
    (should_check, update.channel)
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

/// The persisted `graphics_gpu` preference, read from the on-disk config at
/// startup. Mirrors [`launch_transparency_enabled`]: a missing / unreadable config
/// yields the default [`GpuPreference::Auto`], never a crash. Fed into
/// [`apply_gpu_preference`], where `WGPU_POWER_PREF` still wins.
fn launch_gpu_preference() -> c0pl4nd_core::config::GpuPreference {
    c0pl4nd_core::Config::default_path()
        .filter(|p| p.exists())
        .and_then(|p| {
            std::fs::read_to_string(&p)
                .ok()
                .and_then(|s| c0pl4nd_core::Config::from_toml(&s, &p).ok())
        })
        .map(|c| c.graphics_gpu)
        .unwrap_or_default()
}

/// The persisted `graphics_backend` override, read from the on-disk config at
/// startup (BEFORE the GPU device is created). Mirrors
/// [`launch_transparency_enabled`]: a missing / unreadable config yields the
/// default [`GraphicsBackend::Auto`] (platform-smart choice), never a crash.
/// Fed into [`prefer_backend_on_windows`], where `WGPU_BACKEND` still wins.
fn launch_backend_override() -> c0pl4nd_core::config::GraphicsBackend {
    c0pl4nd_core::Config::default_path()
        .filter(|p| p.exists())
        .and_then(|p| {
            std::fs::read_to_string(&p)
                .ok()
                .and_then(|s| c0pl4nd_core::Config::from_toml(&s, &p).ok())
        })
        .map(|c| c.graphics_backend)
        .unwrap_or_default()
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
///
/// `backend_override` is the persisted [`GraphicsBackend`](c0pl4nd_core::config::GraphicsBackend)
/// setting: `Auto` keeps the transparency-smart default above, while an explicit
/// variant forces that backend — the in-app escape hatch for a driver-specific
/// rendering glitch (e.g. corrupted grid glyphs under a bad DX12 driver → pick
/// Vulkan). Precedence: `WGPU_BACKEND` env (one-off debug) > config override >
/// platform default. Effective on Windows; inert elsewhere (the glitch it exists
/// to work around is the Windows DX12 present path).
fn prefer_backend_on_windows(
    options: &mut eframe::NativeOptions,
    want_transparency: bool,
    backend_override: c0pl4nd_core::config::GraphicsBackend,
) {
    #[cfg(target_os = "windows")]
    {
        use eframe::wgpu::Backends;
        if let eframe::egui_wgpu::WgpuSetup::CreateNew(setup) = &mut options.wgpu_options.wgpu_setup
        {
            // Platform-smart default: Vulkan when the user wants a see-through
            // window, else the more overlay-robust DX12.
            let platform_default = if want_transparency {
                Backends::VULKAN
            } else {
                Backends::DX12
            };
            // The persisted setting maps to an explicit backend; `Auto` defers to
            // the platform default. The `WGPU_BACKEND` env still wins over both.
            let configured = resolve_configured_backends(backend_override, platform_default);
            let resolved = Backends::from_env().unwrap_or(configured);
            setup.instance_descriptor.backends = resolved;
            // F4-3: if transparency was requested but the resolved backend is not
            // Vulkan (e.g. the user forced `WGPU_BACKEND=dx12` or the config
            // override to dodge a Vulkan-overlay crash), the window will be
            // opaque. Tell them why and how to recover, instead of leaving them to
            // wonder why "transparency does nothing". Pairs with the F4-1 crash
            // hook: a Vulkan-overlay panic now also self-diagnoses via the crash log.
            if let Some(msg) = transparency_fallback_warning(
                want_transparency,
                resolved.contains(Backends::VULKAN),
            ) {
                tracing::warn!("{msg}");
            }
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = want_transparency;
        let _ = backend_override;
        let _ = options; // used on every platform; backend default is correct off Windows
    }
}

/// F4-3 — the user-facing notice for the transparency/Vulkan dependency.
///
/// Returns the warning to surface when window transparency was requested but a
/// non-Vulkan GPU backend ended up selected. In that case the window is OPAQUE:
/// real transparency needs Vulkan's WSI alpha; a DX12 swapchain is opaque to
/// DWM. Pure (no I/O, no wgpu types) so it is unit-testable on every platform;
/// the Windows caller passes `resolved.contains(Backends::VULKAN)`.
///
/// Only CALLED on Windows (the backend-selection logic above is Windows-only),
/// but defined cross-platform so the `#[cfg(test)]` unit tests exercise it on
/// every OS — hence `allow(dead_code)` off Windows, where there is no caller.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn transparency_fallback_warning(
    want_transparency: bool,
    backend_is_vulkan: bool,
) -> Option<&'static str> {
    if want_transparency && !backend_is_vulkan {
        Some(
            "window transparency was requested but a non-Vulkan GPU backend was selected; \
             the window will be OPAQUE — real transparency requires the Vulkan backend. \
             Unset WGPU_BACKEND (or set WGPU_BACKEND=vulkan) to enable transparency.",
        )
    } else {
        None
    }
}

/// Map the persisted [`GraphicsBackend`](c0pl4nd_core::config::GraphicsBackend)
/// override + the platform default to the concrete wgpu `Backends` mask, BEFORE
/// the `WGPU_BACKEND` env override is applied. `Auto` defers to `platform_default`;
/// each explicit variant forces that backend. Pure + cross-platform (wgpu's
/// `Backends` flags exist on every OS) so it is unit-testable everywhere; only
/// CALLED inside the Windows backend-selection block, hence `allow(dead_code)`
/// off Windows where there is no caller.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn resolve_configured_backends(
    backend_override: c0pl4nd_core::config::GraphicsBackend,
    platform_default: eframe::wgpu::Backends,
) -> eframe::wgpu::Backends {
    use c0pl4nd_core::config::GraphicsBackend;
    use eframe::wgpu::Backends;
    match backend_override {
        GraphicsBackend::Auto => platform_default,
        GraphicsBackend::Dx12 => Backends::DX12,
        GraphicsBackend::Vulkan => Backends::VULKAN,
        GraphicsBackend::Gl => Backends::GL,
    }
}

/// Resolve the wgpu `PowerPreference` from the persisted GPU preference, the
/// `WGPU_POWER_PREF` env override, and the platform default. Precedence:
/// `env_override` (one-off debug) > the explicit config choice > `default`. `Auto`
/// defers to `default`. Pure + cross-platform so it is unit-testable everywhere.
fn resolve_power_preference(
    gpu: c0pl4nd_core::config::GpuPreference,
    env_override: Option<eframe::wgpu::PowerPreference>,
    default: eframe::wgpu::PowerPreference,
) -> eframe::wgpu::PowerPreference {
    use c0pl4nd_core::config::GpuPreference;
    use eframe::wgpu::PowerPreference;
    if let Some(env) = env_override {
        return env; // WGPU_POWER_PREF wins over the persisted setting
    }
    match gpu {
        GpuPreference::Auto => default,
        GpuPreference::Integrated => PowerPreference::LowPower,
        GpuPreference::Discrete => PowerPreference::HighPerformance,
    }
}

/// Apply the persisted GPU preference to the wgpu setup by setting the adapter
/// `power_preference` — the in-app fix for a multi-GPU (Optimus) machine whose
/// discrete-GPU driver corrupts rendering: `Integrated` routes to the low-power
/// iGPU. Cross-platform (hybrid graphics exist on Windows + Linux). `Auto` +
/// `WGPU_POWER_PREF` are honoured via [`resolve_power_preference`]. No-op unless
/// the setup is `CreateNew` (eframe is creating its own device).
fn apply_gpu_preference(
    options: &mut eframe::NativeOptions,
    gpu: c0pl4nd_core::config::GpuPreference,
) {
    use eframe::wgpu::PowerPreference;
    if let eframe::egui_wgpu::WgpuSetup::CreateNew(setup) = &mut options.wgpu_options.wgpu_setup {
        // eframe's default already folds in `WGPU_POWER_PREF` (its base is
        // `from_env().unwrap_or(HighPerformance)`); re-derive so the env keeps
        // priority over the persisted config choice.
        let env_override = PowerPreference::from_env();
        setup.power_preference =
            resolve_power_preference(gpu, env_override, PowerPreference::HighPerformance);
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

#[cfg(test)]
mod tests {
    use super::{
        resolve_configured_backends, resolve_power_preference, transparency_fallback_warning,
    };
    use c0pl4nd_core::config::GraphicsBackend;
    use eframe::wgpu::Backends;

    #[test]
    fn gpu_preference_maps_and_env_wins() {
        use c0pl4nd_core::config::GpuPreference;
        use eframe::wgpu::PowerPreference;
        // Auto defers to the platform default.
        assert_eq!(
            resolve_power_preference(GpuPreference::Auto, None, PowerPreference::HighPerformance),
            PowerPreference::HighPerformance,
        );
        // Integrated → low-power (the iGPU escape hatch); Discrete → high-perf.
        assert_eq!(
            resolve_power_preference(
                GpuPreference::Integrated,
                None,
                PowerPreference::HighPerformance
            ),
            PowerPreference::LowPower,
        );
        assert_eq!(
            resolve_power_preference(GpuPreference::Discrete, None, PowerPreference::LowPower),
            PowerPreference::HighPerformance,
        );
        // WGPU_POWER_PREF env overrides even an explicit config choice.
        assert_eq!(
            resolve_power_preference(
                GpuPreference::Discrete,
                Some(PowerPreference::LowPower),
                PowerPreference::HighPerformance,
            ),
            PowerPreference::LowPower,
            "env override wins over the persisted setting",
        );
    }

    #[test]
    fn auto_backend_defers_to_the_platform_default() {
        // `Auto` must NOT force a backend — it passes the platform default through
        // unchanged (DX12 for opaque, Vulkan for transparency on Windows).
        assert_eq!(
            resolve_configured_backends(GraphicsBackend::Auto, Backends::DX12),
            Backends::DX12,
        );
        assert_eq!(
            resolve_configured_backends(GraphicsBackend::Auto, Backends::VULKAN),
            Backends::VULKAN,
        );
    }

    #[test]
    fn explicit_backend_overrides_the_platform_default() {
        // Each explicit choice forces THAT backend regardless of the default —
        // the garble escape hatch (e.g. Vulkan over a bad DX12 default).
        assert_eq!(
            resolve_configured_backends(GraphicsBackend::Vulkan, Backends::DX12),
            Backends::VULKAN,
        );
        assert_eq!(
            resolve_configured_backends(GraphicsBackend::Dx12, Backends::VULKAN),
            Backends::DX12,
        );
        assert_eq!(
            resolve_configured_backends(GraphicsBackend::Gl, Backends::DX12),
            Backends::GL,
        );
    }

    #[test]
    fn warns_only_when_transparency_wanted_but_backend_not_vulkan() {
        // Transparency requested + non-Vulkan backend → opaque-window warning.
        assert!(transparency_fallback_warning(true, false).is_some());
        // Transparency requested + Vulkan backend → no warning (it will work).
        assert!(transparency_fallback_warning(true, true).is_none());
        // Opaque window requested → never warn, regardless of backend.
        assert!(transparency_fallback_warning(false, false).is_none());
        assert!(transparency_fallback_warning(false, true).is_none());
    }

    #[test]
    fn warning_text_names_the_recovery_lever() {
        let msg = transparency_fallback_warning(true, false).unwrap();
        assert!(msg.contains("WGPU_BACKEND"));
        assert!(msg.to_lowercase().contains("vulkan"));
    }
}
