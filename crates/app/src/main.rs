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

fn main() -> Result<()> {
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

    // `c0pl4nd update` — explicit, user-initiated upgrade via package manager.
    if args.iter().any(|a| a == "update") {
        return update::run_update();
    }

    let config = Config::default();

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

    // Opt-in, local-first launch version check (off by default).
    if config.update.check_on_launch {
        update::notify_if_outdated();
    }

    // Windowed GPU mode is provided by the app-shell window module.
    crate::run_gui(&config)
}

mod image_render;
mod screenshot;
mod update;
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
