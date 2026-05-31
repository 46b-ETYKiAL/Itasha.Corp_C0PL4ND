//! End-to-end integration tests for the C0PL4ND core engine.
//!
//! These exercise the real bundled assets and a live PTY session, complementing
//! the per-module unit tests.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use c0pl4nd_core::config::Config;
use c0pl4nd_core::session::Session;
use c0pl4nd_core::theme::Theme;

fn themes_dir() -> PathBuf {
    // crate manifest dir is crates/core; themes live at apps/c0pl4nd/assets/themes.
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/themes")
}

#[test]
fn all_bundled_themes_load_and_validate() {
    let dir = themes_dir();
    let expected = [
        "itasha-void.toml",
        "itasha-void-high-contrast.toml",
        "ghost-paper.toml",
        "wired-colorblind.toml",
    ];
    for name in expected {
        let path = dir.join(name);
        assert!(path.exists(), "bundled theme missing: {path:?}");
        let theme = Theme::load_from(&path).unwrap_or_else(|e| panic!("theme {name} failed: {e}"));
        theme
            .validate()
            .unwrap_or_else(|e| panic!("theme {name} invalid: {e}"));
        // Every ANSI index must resolve to a real colour.
        for i in 0u8..16 {
            let _ = theme.ansi(i);
        }
    }
}

#[test]
fn flagship_theme_is_wired_noir() {
    // Wired Noir is the brand-canon flagship (DECISION-2026-005), shared with
    // the SCR1B3 editor: cool near-black void hull + one teal accent.
    let theme = Theme::load_from(&themes_dir().join("wired-noir.toml")).expect("load wired-noir");
    assert_eq!(
        c0pl4nd_core::theme::parse_hex(&theme.background).unwrap(),
        (0x07, 0x0a, 0x0c), // void near-black
    );
    assert_eq!(
        c0pl4nd_core::theme::parse_hex(&theme.cursor).unwrap(),
        (0x34, 0xe0, 0xd0), // teal — the system voice
    );
    // The prior itasha-void theme remains bundled as a non-default alternative.
    let void = Theme::load_from(&themes_dir().join("itasha-void.toml")).expect("load itasha-void");
    assert_eq!(
        c0pl4nd_core::theme::parse_hex(&void.background).unwrap(),
        (8, 6, 13)
    );
}

#[test]
fn full_config_round_trips_through_toml() {
    let cfg = Config::default();
    let toml_str = toml::to_string(&cfg).expect("serialize config");
    let parsed = Config::from_toml(&toml_str, Path::new("roundtrip.toml")).expect("parse config");
    assert_eq!(cfg, parsed, "config did not survive a TOML round-trip");
    // The expanded customization surface must be present.
    assert!(toml_str.contains("[keybindings]"));
    assert!(toml_str.contains("[update]"));
    assert!(toml_str.contains("[font]"));
}

#[test]
fn startup_panel_is_plain_text_for_overlay() {
    // The startup panel is drawn as an app-rendered overlay (the renderer
    // colours it), NOT injected into the PTY grid — on Windows ConPTY repaints
    // the screen on shell start and would wipe a grid-injected panel. So the
    // panel must be PLAIN text (no ANSI escapes) carrying the logo + stats.
    let info = c0pl4nd_core::fetch::SystemInfo::gather(Some("Integration GPU"));
    let panel = c0pl4nd_core::fetch::render_panel(&info);

    assert!(
        panel.contains("the operator's shell"),
        "panel missing logo; got:\n{panel}"
    );
    assert!(
        panel.contains("Integration GPU"),
        "panel missing gpu stat; got:\n{panel}"
    );
    assert!(
        !panel.contains('\x1b'),
        "panel must be plain text (renderer colours the overlay); got:\n{panel}"
    );
}

#[test]
fn session_runs_multiple_commands_end_to_end() {
    let token_a = "c0pl4nd_alpha";
    let token_b = "c0pl4nd_beta";
    #[cfg(windows)]
    let session = Session::spawn_program(
        "cmd.exe",
        &["/C", &format!("echo {token_a} && echo {token_b}")],
        24,
        80,
    )
    .expect("spawn session");
    #[cfg(not(windows))]
    let session = Session::spawn_program(
        "/bin/sh",
        &["-c", &format!("printf '{token_a}\\n{token_b}\\n'")],
        24,
        80,
    )
    .expect("spawn session");

    let deadline = Instant::now() + Duration::from_secs(5);
    let mut both = false;
    while Instant::now() < deadline {
        let text = session.snapshot_text();
        if text.contains(token_a) && text.contains(token_b) {
            both = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(both, "expected both command outputs in the grid");
}
