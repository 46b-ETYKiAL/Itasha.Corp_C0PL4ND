//! Headless **interaction** tests for C0PL4ND surfaces not covered by the
//! existing suites: the command-history quick-run SIDEBAR (Ctrl+Shift+H, #21) and
//! the window-management KEYBOARD shortcuts (Ctrl/Cmd+Shift+{D,E,W} split/close
//! and Ctrl/Cmd+, settings), driven by `egui_kittest`.
//!
//! ## Discipline (non-negotiable)
//!
//! Every test drives the **real** production frame loop
//! ([`C0pl4ndApp::frame_tick`]) with **simulated input** and asserts the
//! **observable outcome** the input actually caused — never a
//! set-state-then-assert-the-same-state tautology. The sidebar toggle/run logic
//! and the chord handlers run THROUGH `frame_tick`, exactly as the shipping
//! binary runs them each frame. All outcomes are observed via the app's public
//! accessors (`history_sidebar_open`, `pane_count`, `settings_is_open`,
//! `last_palette_run`) — the same accessors the existing chrome/palette suites
//! use — NOT internal mirrors.
//!
//! These outcomes are PTY-independent: the history feeds from a seeded command
//! history and the chord handlers act on app state, so the tests are stable on a
//! CI box with no usable PTY. Where a test needs a populated history it SEEDS the
//! history via the real type-then-Enter capture path (echo-gated, like the
//! palette suite) so the sidebar lists real recorded commands.

#![allow(dead_code)] // The `#[path]`-included module has production entry points
                     // (eframe `App` impl, `apply_window_effect`) unused here.

#[path = "../src/egui_app/mod.rs"]
mod egui_app;

use std::cell::RefCell;
use std::time::{Duration, Instant};

use egui_kittest::Harness;

use egui_app::C0pl4ndApp;

/// Build a headless harness driving the REAL `frame_tick` for a shared app.
fn harness(app: &RefCell<C0pl4ndApp>) -> Harness<'_> {
    #[allow(deprecated)]
    let mut h = Harness::new(move |ctx| app.borrow_mut().frame_tick(ctx));
    h.set_size(egui::vec2(1200.0, 800.0));
    h.run();
    h
}

/// Send a Ctrl+Shift+`key` chord (real `Event::Key` with modifiers), then step.
fn press_ctrl_shift(h: &mut Harness<'_>, key: egui::Key) {
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
    h.step();
}

/// Send a Ctrl+`key` chord (no shift), then step.
fn press_ctrl(h: &mut Harness<'_>, key: egui::Key) {
    h.event(egui::Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers {
            ctrl: true,
            ..Default::default()
        },
    });
    h.step();
}

/// Type a string into the focused pane (one `Event::Text` per char), then step —
/// the real capture path the command history records from.
fn type_text(h: &mut Harness<'_>, s: &str) {
    for ch in s.chars() {
        h.event(egui::Event::Text(ch.to_string()));
    }
    h.step();
}

/// Poll until the focused pane's grid shows `needle` (the shell echoed it), so
/// the echo-gated history capture records the line deterministically.
fn wait_for_echo(h: &mut Harness<'_>, app: &RefCell<C0pl4ndApp>, needle: &str) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        h.step();
        if app
            .borrow()
            .focused_grid_text()
            .is_some_and(|t| t.contains(needle))
        {
            return;
        }
        std::thread::sleep(Duration::from_millis(40));
    }
}

/// Type-then-Enter a command into the focused pane (records it in the history).
fn run_line(h: &mut Harness<'_>, app: &RefCell<C0pl4ndApp>, line: &str) {
    type_text(h, line);
    wait_for_echo(h, app, line);
    h.key_press(egui::Key::Enter);
    h.step();
}

// ---- command-history quick-run sidebar (#21, Ctrl+Shift+H) -------------------

#[test]
fn ctrl_shift_h_toggles_the_history_sidebar() {
    // Ctrl+Shift+H opens the docked command-history sidebar; pressing it again
    // closes it. Drives the real chord through `frame_tick`.
    let app = RefCell::new(C0pl4ndApp::bootstrap());
    let mut h = harness(&app);

    assert!(
        !app.borrow().history_sidebar_open(),
        "the history sidebar starts closed"
    );

    press_ctrl_shift(&mut h, egui::Key::H);
    assert!(
        app.borrow().history_sidebar_open(),
        "Ctrl+Shift+H must open the command-history sidebar"
    );

    press_ctrl_shift(&mut h, egui::Key::H);
    assert!(
        !app.borrow().history_sidebar_open(),
        "Ctrl+Shift+H again must close the command-history sidebar"
    );
}

#[test]
fn the_history_sidebar_lists_recorded_commands_newest_first() {
    // With a populated history, the open sidebar's rows are the live history,
    // most-recent-first — the same source the palette uses. We seed two commands
    // through the real capture path, open the sidebar, and assert the rows it
    // would render match the recorded history order.
    let app = RefCell::new(C0pl4ndApp::bootstrap());
    let mut h = harness(&app);

    run_line(&mut h, &app, "echo alpha");
    run_line(&mut h, &app, "echo beta");

    press_ctrl_shift(&mut h, egui::Key::H);
    assert!(app.borrow().history_sidebar_open(), "sidebar opened");

    // The sidebar's source of truth IS the command history (newest-first); a
    // populated history must surface BOTH seeded commands with the newest first.
    let entries = app.borrow().command_history_entries();
    assert_eq!(
        entries,
        vec!["echo beta".to_string(), "echo alpha".to_string()],
        "the history (the sidebar's row source) must be newest-first"
    );
}

#[test]
fn clicking_a_history_row_runs_it_in_the_focused_pane_and_closes_the_sidebar() {
    // A history row "click" re-runs the command in the focused pane and closes the
    // sidebar — the real run path (`run_history_command`) the rendered rows invoke.
    // We drive it via the production `test_run_history_row` entry point (the exact
    // function a rendered row's click calls) and assert the observable effects: the
    // command ran (recorded in `last_palette_run`, shared with the palette) and the
    // sidebar closed.
    let app = RefCell::new(C0pl4ndApp::bootstrap());
    let mut h = harness(&app);

    run_line(&mut h, &app, "echo gamma");
    run_line(&mut h, &app, "echo delta");

    press_ctrl_shift(&mut h, egui::Key::H);
    assert!(app.borrow().history_sidebar_open(), "sidebar opened");

    // Row 1 (newest-first) is the OLDER "echo gamma". Running it must report it as
    // the last run command and close the sidebar.
    let ran = app.borrow_mut().test_run_history_row(1);
    h.run();
    assert_eq!(
        ran.as_deref(),
        Some("echo gamma"),
        "running history row 1 must run the older 'echo gamma'"
    );
    assert_eq!(
        app.borrow().last_palette_run().as_deref(),
        Some("echo gamma"),
        "a sidebar row run must record the command as the last run (the \
         observable shared with the palette)"
    );
    assert!(
        !app.borrow().history_sidebar_open(),
        "running a history row must close the sidebar"
    );
}

#[test]
fn an_out_of_range_history_row_runs_nothing() {
    // Defensive: asking to run a row index past the end is a no-op (None), never a
    // panic — the row-index path must tolerate a stale/oversized index gracefully.
    let app = RefCell::new(C0pl4ndApp::bootstrap());
    let mut h = harness(&app);

    run_line(&mut h, &app, "echo only");
    press_ctrl_shift(&mut h, egui::Key::H);

    let ran = app.borrow_mut().test_run_history_row(99);
    h.run();
    assert_eq!(
        ran, None,
        "an out-of-range history row index must run nothing (None), not panic"
    );
}

// ---- window-management keyboard shortcuts (F-parity) -------------------------

#[test]
fn ctrl_shift_d_splits_the_focused_pane_horizontally() {
    // Ctrl/Cmd+Shift+D splits the focused pane left|right, adding a pane. The egui
    // shell offered split only as chrome before; this asserts the keyboard parity.
    let app = RefCell::new(C0pl4ndApp::bootstrap());
    let before = app.borrow().pane_count();
    assert_eq!(before, 1, "bootstrap opens a single pane");
    let mut h = harness(&app);

    press_ctrl_shift(&mut h, egui::Key::D);

    assert_eq!(
        app.borrow().pane_count(),
        before + 1,
        "Ctrl+Shift+D must split the focused pane, adding exactly one pane"
    );
}

#[test]
fn ctrl_shift_e_splits_the_focused_pane_vertically() {
    // Ctrl/Cmd+Shift+E splits the focused pane top/bottom, adding a pane.
    let app = RefCell::new(C0pl4ndApp::bootstrap());
    let before = app.borrow().pane_count();
    let mut h = harness(&app);

    press_ctrl_shift(&mut h, egui::Key::E);

    assert_eq!(
        app.borrow().pane_count(),
        before + 1,
        "Ctrl+Shift+E must split the focused pane, adding exactly one pane"
    );
}

#[test]
fn ctrl_shift_w_closes_the_focused_pane() {
    // Ctrl/Cmd+Shift+W closes the focused pane. Open a second pane first (the app
    // keeps at least one pane alive), then close the focused one and assert the
    // count dropped by one.
    let app = RefCell::new(C0pl4ndApp::bootstrap());
    let mut h = harness(&app);

    // Add a pane via the keyboard new-pane chord (Ctrl+Shift+T), so there are two.
    press_ctrl_shift(&mut h, egui::Key::T);
    assert_eq!(app.borrow().pane_count(), 2, "two panes after Ctrl+Shift+T");

    press_ctrl_shift(&mut h, egui::Key::W);
    assert_eq!(
        app.borrow().pane_count(),
        1,
        "Ctrl+Shift+W must close the focused pane (count drops to one)"
    );
}

#[test]
fn ctrl_comma_toggles_the_settings_window() {
    // Ctrl/Cmd+, is the conventional "open settings" chord. It must toggle the
    // settings window open, then closed — the keyboard parity for the gear button.
    let app = RefCell::new(C0pl4ndApp::bootstrap());
    let mut h = harness(&app);
    assert!(!app.borrow().settings_is_open(), "settings closed at start");

    press_ctrl(&mut h, egui::Key::Comma);
    assert!(
        app.borrow().settings_is_open(),
        "Ctrl+, must open the settings window"
    );

    press_ctrl(&mut h, egui::Key::Comma);
    assert!(
        !app.borrow().settings_is_open(),
        "Ctrl+, again must close the settings window"
    );
}
