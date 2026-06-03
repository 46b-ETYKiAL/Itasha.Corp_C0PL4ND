//! Headless **interaction** tests for the C0PL4ND egui shell's COMMAND PALETTE
//! (quick find/run of previously-run commands), driven by `egui_kittest`.
//!
//! ## Discipline (non-negotiable)
//!
//! Every test drives the **real** production frame loop
//! ([`C0pl4ndApp::frame_tick`]) with **simulated input** and asserts the
//! **observable outcome** the input actually caused — never a
//! set-state-then-assert-the-same-state tautology. The palette logic
//! (`toggle_palette` / `palette_move` / `run_palette_selection`) is exercised
//! THROUGH `frame_tick`, exactly as the shipping binary runs it each frame:
//!
//! - **history capture**: typing a line + Enter records it in the command
//!   history (the line that the palette later finds + re-runs).
//! - **toggle**: Ctrl+Shift+P opens AND closes the palette.
//! - **run**: selecting an entry and pressing Enter runs it (sets the observable
//!   `last_palette_run`, moves it to the history front) and closes the palette.
//!
//! These outcomes are PTY-independent — the capture loop and palette logic run
//! whether or not the platform spawned a live shell — so the tests are stable on
//! a CI box with no usable PTY.
//!
//! The app module is compiled in via `#[path]` so the test can construct
//! `C0pl4ndApp` directly; the closure handed to `Harness::new` calls the same
//! `frame_tick` the shipping binary runs each frame.

#![allow(dead_code)] // The `#[path]`-included module has production entry points
                     // (eframe `App` impl, `apply_window_effect`) unused here.

#[path = "../src/egui_app/mod.rs"]
mod egui_app;

use std::cell::RefCell;

use egui_kittest::Harness;

use egui_app::C0pl4ndApp;

/// Build a headless harness driving the REAL `frame_tick` for a shared app.
fn harness(app: &RefCell<C0pl4ndApp>) -> Harness<'_> {
    #[allow(deprecated)]
    let mut h = Harness::new(move |ctx| app.borrow_mut().frame_tick(ctx));
    h.set_size(egui::vec2(1000.0, 700.0));
    h.run();
    h
}

/// Type a string into the focused pane (one `Event::Text` per char), then step a
/// frame — the real `forward_input_to_focused` capture path.
fn type_text(h: &mut Harness<'_>, s: &str) {
    for ch in s.chars() {
        h.event(egui::Event::Text(ch.to_string()));
    }
    h.step();
}

/// Press Enter (a real `Event::Key`), then step a frame.
fn press_enter(h: &mut Harness<'_>) {
    h.key_press(egui::Key::Enter);
    h.step();
}

/// Send the Ctrl+Shift+P palette chord (real `Event::Key` with modifiers), then
/// step a frame.
fn press_palette_chord(h: &mut Harness<'_>) {
    h.event(egui::Event::Key {
        key: egui::Key::P,
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

/// Type-then-Enter a command into the focused pane (records it in the history).
fn run_line(h: &mut Harness<'_>, line: &str) {
    type_text(h, line);
    press_enter(h);
}

/// Typing a line and pressing Enter records it in the command history,
/// most-recent-first and de-duplicated (the palette's source of truth).
#[test]
fn typed_lines_are_recorded_in_command_history() {
    let app = RefCell::new(C0pl4ndApp::bootstrap());
    let mut h = harness(&app);

    run_line(&mut h, "echo one");
    run_line(&mut h, "echo two");

    let entries = app.borrow().command_history_entries();
    assert_eq!(
        entries,
        vec!["echo two".to_string(), "echo one".to_string()],
        "typed-then-Enter lines must be recorded most-recent-first"
    );
}

/// A whitespace-only line is not recorded, and Backspace edits the captured line
/// before Enter commits it.
#[test]
fn blank_lines_are_ignored_and_backspace_edits_the_line() {
    let app = RefCell::new(C0pl4ndApp::bootstrap());
    let mut h = harness(&app);

    // Just Enter on an empty line → nothing recorded.
    press_enter(&mut h);
    assert!(
        app.borrow().command_history_entries().is_empty(),
        "an empty line must not be recorded"
    );

    // Type "lss", backspace once → "ls", Enter → records "ls".
    type_text(&mut h, "lss");
    h.key_press(egui::Key::Backspace);
    h.step();
    press_enter(&mut h);
    assert_eq!(
        app.borrow().command_history_entries(),
        vec!["ls".to_string()],
        "Backspace must edit the captured line before Enter commits it"
    );
}

/// Ctrl+Shift+P opens the palette; pressing it again closes it. Drives the real
/// chord through `frame_tick`.
#[test]
fn ctrl_shift_p_toggles_the_command_palette() {
    let app = RefCell::new(C0pl4ndApp::bootstrap());
    let mut h = harness(&app);

    assert!(!app.borrow().palette_open(), "the palette starts closed");

    press_palette_chord(&mut h);
    assert!(
        app.borrow().palette_open(),
        "Ctrl+Shift+P must open the palette"
    );

    press_palette_chord(&mut h);
    assert!(
        !app.borrow().palette_open(),
        "Ctrl+Shift+P again must close the palette"
    );
}

/// Opening the palette and pressing Enter on the (default-selected, most-recent)
/// entry runs it: sets the observable `last_palette_run`, moves it to the history
/// front, and closes the palette. Down-arrow first selects the older entry.
#[test]
fn selecting_and_running_a_palette_entry_runs_it_and_closes() {
    let app = RefCell::new(C0pl4ndApp::bootstrap());
    let mut h = harness(&app);

    // History (most-recent-first): ["echo two", "echo one"].
    run_line(&mut h, "echo one");
    run_line(&mut h, "echo two");

    // Open the palette (empty query → all entries, selection at row 0 = newest).
    press_palette_chord(&mut h);
    assert!(app.borrow().palette_open(), "palette opened");

    // Down-arrow moves the selection to row 1 = the OLDER "echo one".
    h.key_press(egui::Key::ArrowDown);
    h.step();

    // Enter runs the selected entry.
    h.key_press(egui::Key::Enter);
    h.step();

    let app_ref = app.borrow();
    assert_eq!(
        app_ref.last_palette_run(),
        Some("echo one".to_string()),
        "Enter must run the selected (older) history entry"
    );
    assert!(
        !app_ref.palette_open(),
        "running an entry must close the palette"
    );
    assert_eq!(
        app_ref
            .command_history_entries()
            .first()
            .map(String::as_str),
        Some("echo one"),
        "re-running an entry moves it to the front of the history"
    );
}

/// Escape closes the palette without running anything.
#[test]
fn escape_closes_the_palette_without_running() {
    let app = RefCell::new(C0pl4ndApp::bootstrap());
    let mut h = harness(&app);

    run_line(&mut h, "echo one");
    press_palette_chord(&mut h);
    assert!(app.borrow().palette_open(), "palette opened");

    h.key_press(egui::Key::Escape);
    h.step();

    let app_ref = app.borrow();
    assert!(!app_ref.palette_open(), "Escape must close the palette");
    assert_eq!(
        app_ref.last_palette_run(),
        None,
        "Escape must not run any command"
    );
}
