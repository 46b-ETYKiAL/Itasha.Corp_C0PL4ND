//! Unit tests for the legacy winit binary's entrypoint module.
//!
//! Split into its own file (mirroring `window_tests.rs`) so the test code is
//! attributed to THIS file rather than inflating `main.rs`'s own coverage
//! number with the tests that measure it.

use super::*;

/// `--demo` is C0PL4ND's headless engine smoke test (PTY + VT + grid, no
/// window/GPU). This drives the REAL path and asserts the spawned command's
/// output actually reached the rendered grid — a test that only checked
/// `run_demo(..).is_ok()` would pass even with a dead engine, because the
/// poll loop simply times out and still returns `Ok`.
#[test]
fn headless_demo_renders_the_command_output_from_a_real_pty() {
    let config = Config::default();
    let text = demo_grid_text(&config).expect("the demo engine must run headlessly");
    assert!(
        text.contains("C0PL4ND online"),
        "the demo command's stdout must reach the grid, got: {text:?}"
    );
}

/// The demo grid is sized from the config, not hard-coded.
#[test]
fn demo_honours_the_configured_grid_size() {
    let mut config = Config::default();
    config.window.rows = 10;
    config.window.cols = 40;
    let text = demo_grid_text(&config).expect("the demo engine must run headlessly");
    // A 40-col grid can never render a longer line than 40 cells.
    for line in text.lines() {
        assert!(
            line.chars().count() <= 40,
            "a {}-char line escaped the 40-col grid: {line:?}",
            line.chars().count()
        );
    }
}

/// `run_demo` itself must stay a clean, printing wrapper over the engine.
#[test]
fn run_demo_succeeds_headlessly() {
    assert!(run_demo(&Config::default()).is_ok());
}
