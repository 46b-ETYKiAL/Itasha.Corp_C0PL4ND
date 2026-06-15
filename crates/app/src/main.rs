// Release GUI builds use the Windows subsystem so launching the terminal does
// NOT spawn a separate console window alongside it. Debug builds keep the
// console subsystem so `--demo` / `--version` / `--screenshot` print during
// development and tests.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

//! C0PL4ND terminal — application entrypoint.
//!
//! Run modes:
//!   c0pl4nd --version       Print version and exit.
//!   c0pl4nd --demo          Headless demo: spawn the shell, run a command,
//!                           and render the live grid to stdout. Verifies the
//!                           full core engine (PTY + VT + grid) end-to-end.
//!   c0pl4nd                 Launch the windowed terminal (GPU shell).

use anyhow::Result;
use std::time::{Duration, Instant};

use c0pl4nd_core::{Config, Session};

// mimalloc as the global allocator (see crates/app/Cargo.toml) — matches the
// canonical `c0pl4nd` binary so both bins share the same allocator behaviour.
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

fn main() -> Result<()> {
    // FIRST statement: harden the Windows DLL search order before any other DLL
    // could be implicitly loaded, defeating DLL-planting when launched from an
    // untrusted directory (e.g. Downloads). No-op off Windows.
    dll_hardening::harden_dll_search_order();

    // Install the unexpected-panic crash hook early: `panic = "abort"` otherwise
    // kills the GUI with no diagnostic. Writes a rotating crash log (and, on
    // Windows, a MessageBox) before chaining to the default hook and aborting.
    panic_hook::install();

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("C0PL4ND_LOG")
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .with_writer(std::io::stderr)
        .init();

    let args: Vec<String> = std::env::args().collect();

    if args.iter().any(|a| a == "--version" || a == "-V") {
        println!("{} {}", c0pl4nd_core::PRODUCT_NAME, c0pl4nd_core::version());
        return Ok(());
    }

    // `c0pl4nd-legacy --diagnostics` (alias `--doctor`) — one-shot env/config
    // dump, exit before window init. The legacy winit binary handles only
    // `KeyboardInput` (no `WindowEvent::Ime` arm), so composed-text/IME routing
    // is NOT compiled into this binary — reported honestly as `false`.
    if diagnostics::requested(&args) {
        // `run` prints the report and returns the exit code (0); a non-zero code
        // would surface as a process failure, but the dump never fails.
        let _code = diagnostics::run(false, panic_hook::crash_log_dir());
        return Ok(());
    }

    // Load the user config from its canonical path, falling back to defaults
    // when it is absent or unreadable. Previously this was Config::default()
    // unconditionally, so on-disk settings (theme, opacity, font, cursor,
    // acrylic, …) never took effect across launches even though the settings
    // panel wrote them to the file.
    let config = match Config::default_path().filter(|p| p.exists()) {
        Some(p) => match std::fs::read_to_string(&p)
            .map_err(|e| e.to_string())
            .and_then(|s| Config::from_toml(&s, &p).map_err(|e| e.to_string()))
        {
            Ok(c) => c,
            Err(e) => {
                eprintln!("c0pl4nd: failed to load config {p:?}: {e}; using defaults");
                Config::default()
            }
        },
        None => Config::default(),
    };

    // `c0pl4nd update` — explicit, user-initiated update check against the
    // public GitHub Releases API for the configured channel.
    if args.iter().any(|a| a == "update") {
        return update::run_update(&config.update.channel);
    }

    // `c0pl4nd --screenshot <path.png>` — headless render for README/CI media.
    if let Some(pos) = args.iter().position(|a| a == "--screenshot") {
        let out = args
            .get(pos + 1)
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| std::path::PathBuf::from("c0pl4nd.png"));
        screenshot::capture(&config, &out)?;
        println!("screenshot written to {}", out.display());
        return Ok(());
    }

    if args.iter().any(|a| a == "--demo" || a == "--headless") {
        return run_demo(&config);
    }

    // Launch version check. Runs under the default `notify` mode (and `auto`) or
    // the legacy `check_on_launch` flag, throttled by `check_interval_hours`, and
    // on a background thread so startup never blocks on the network (mirrors the
    // canonical egui binary). The attempt is recorded regardless of outcome so a
    // transient offline launch does not re-check on every subsequent start.
    if config.update.checks_on_launch() && update::check_due(config.update.check_interval_hours) {
        let channel = config.update.channel.clone();
        std::thread::spawn(move || {
            if let Some(notice) = update::check_for_update(&channel) {
                eprintln!("{notice}");
            }
            update::record_check_now();
        });
    }

    // Windowed GPU mode is provided by the app-shell window module.
    crate::run_gui(&config)
}

mod diagnostics;
mod dll_hardening;
mod drag;
mod image_render;
mod pane_render;
mod panic_hook;
mod screenshot;
mod update;
#[cfg(windows)]
mod win_snap;
mod window;
pub use window::run_gui;

/// Headless demo: prove the engine works end-to-end without a GPU/display.
fn run_demo(config: &Config) -> Result<()> {
    let banner = format!(
        "{} {} — {}",
        c0pl4nd_core::PRODUCT_NAME,
        c0pl4nd_core::version(),
        c0pl4nd_core::TAGLINE
    );
    println!("{banner}");
    println!(
        "[headless demo] theme={} font={} {}pt",
        config.theme, config.font.family, config.font.size
    );

    let rows = config.window.rows;
    let cols = config.window.cols;

    // A deterministic, cross-platform command that exercises the VT engine.
    #[cfg(windows)]
    let mut session = Session::spawn_program(
        "cmd.exe",
        &[
            "/C",
            "echo C0PL4ND online && echo wired: present day, present time",
        ],
        rows,
        cols,
    )?;
    #[cfg(not(windows))]
    let mut session = Session::spawn_program(
        "/bin/sh",
        &[
            "-c",
            "printf 'C0PL4ND online\\nwired: present day, present time\\n'",
        ],
        rows,
        cols,
    )?;

    // Poll the grid until output appears (or ~3s elapse).
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        if session.snapshot_text().contains("C0PL4ND online") {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    println!("--- rendered grid ({rows}x{cols}) ---");
    for line in session.snapshot_text().lines().take(4) {
        let trimmed = line.trim_end();
        if !trimmed.is_empty() {
            println!("{trimmed}");
        }
    }
    println!("--- engine OK ---");
    let _ = &mut session;
    Ok(())
}
