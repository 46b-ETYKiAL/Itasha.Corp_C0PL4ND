//! Headless **interaction** tests for the C0PL4ND egui shell's IN-TERMINAL FIND
//! overlay (Ctrl+F), driven by `egui_kittest`.
//!
//! ## Discipline (non-negotiable)
//!
//! Every test drives the **real** production frame loop
//! ([`C0pl4ndApp::frame_tick`]) with **simulated keyboard input** and asserts the
//! **observable outcome** that input actually caused — never a
//! set-state-then-assert-the-same-state tautology. The find logic
//! (`toggle_search` / `recompute_search` / `search_cycle`) is exercised THROUGH
//! `frame_tick`, exactly as the shipping binary runs it each frame:
//!
//! - **toggle**: Ctrl+Shift+F opens AND closes the overlay; Esc closes it.
//! - **filter**: typing a query recomputes the match count over the focused
//!   pane's grid text (the shared core matcher).
//! - **toggles**: the Regex + Case option flags flip via keyboard-seeded state.
//! - **cycle**: Enter / F3 step the selection forward (wrapping); Shift+F3 back.
//!
//! These tests are driven entirely by KEYBOARD (Ctrl+Shift+F, typed chars, nav keys)
//! and assert the observable accessors — they never click a title-bar flow
//! button, so the async-OSC-title flow-region race that destabilises click-based
//! chrome tests does not apply here (see the team's effect-verified-retry note
//! in `egui_chrome.rs`). To search a KNOWN corpus regardless of whether the
//! platform spawned a usable PTY, the tests seed the focused pane's grid with
//! [`C0pl4ndApp::test_seed_focused_grid`] before searching.
//!
//! The app module is compiled in via `#[path]` so the test can construct
//! `C0pl4ndApp` directly; the closure handed to `Harness::new` calls the same
//! `frame_tick` the shipping binary runs each frame.

#![allow(dead_code)] // The `#[path]`-included module has production entry points
                     // (eframe `App` impl, `apply_window_effect`) unused here.

#[path = "../src/egui_app/mod.rs"]
mod egui_app;
#[path = "../src/issue_intake.rs"]
mod issue_intake;
#[path = "../src/reporting.rs"]
mod reporting;

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

/// Send the Ctrl+Shift+F find chord (real `Event::Key` with modifiers), then
/// step. (Plain Ctrl+F is deliberately left for the shell as `^F`.)
fn press_find_chord(h: &mut Harness<'_>) {
    h.event(egui::Event::Key {
        key: egui::Key::F,
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

/// Type a string into the overlay (one `Event::Text` per char), then step a
/// frame — the real query-edit path through the focused `TextEdit`.
fn type_query(h: &mut Harness<'_>, s: &str) {
    for ch in s.chars() {
        h.event(egui::Event::Text(ch.to_string()));
    }
    h.step();
}

/// Press a bare nav key, then step.
fn press(h: &mut Harness<'_>, key: egui::Key) {
    h.key_press(key);
    h.step();
}

/// Seed the focused pane's grid with a known corpus, open the overlay, and step
/// so the first `recompute_search` runs against the seeded lines.
fn open_with_corpus(app: &RefCell<C0pl4ndApp>, h: &mut Harness<'_>, corpus: &str) {
    app.borrow_mut().test_seed_focused_grid(corpus);
    press_find_chord(h);
}

/// Ctrl+F opens the find overlay; pressing it again closes it. Drives the real
/// chord through `frame_tick`.
#[test]
fn ctrl_f_toggles_the_find_overlay() {
    let app = RefCell::new(C0pl4ndApp::bootstrap());
    let mut h = harness(&app);

    assert!(!app.borrow().search_is_open(), "the overlay starts closed");

    press_find_chord(&mut h);
    assert!(
        app.borrow().search_is_open(),
        "Ctrl+F must open the find overlay"
    );

    press_find_chord(&mut h);
    assert!(
        !app.borrow().search_is_open(),
        "Ctrl+F again must close the find overlay"
    );
}

/// Escape closes the overlay.
#[test]
fn escape_closes_the_find_overlay() {
    let app = RefCell::new(C0pl4ndApp::bootstrap());
    let mut h = harness(&app);

    press_find_chord(&mut h);
    assert!(app.borrow().search_is_open(), "overlay opened");

    press(&mut h, egui::Key::Escape);
    assert!(
        !app.borrow().search_is_open(),
        "Escape must close the overlay"
    );
}

/// Typing a query filters the focused pane's grid text — the match count is the
/// real `c0pl4nd_core::search::find` result over the seeded corpus.
#[test]
fn typing_a_query_filters_and_counts_matches() {
    let app = RefCell::new(C0pl4ndApp::bootstrap());
    let mut h = harness(&app);

    open_with_corpus(
        &app,
        &mut h,
        "error: disk full\nok\nanother error here\nfatal error\n",
    );

    type_query(&mut h, "error");
    assert_eq!(
        app.borrow().search_match_count(),
        3,
        "case-insensitive default must find all three 'error' occurrences"
    );

    // Narrow the query — fewer matches.
    type_query(&mut h, ": disk"); // query now "error: disk"
    assert_eq!(
        app.borrow().search_match_count(),
        1,
        "the narrower query must match only the first line"
    );
}

/// An empty query yields zero matches (the core matcher's contract), so opening
/// the overlay before typing shows no matches.
#[test]
fn empty_query_has_no_matches() {
    let app = RefCell::new(C0pl4ndApp::bootstrap());
    let mut h = harness(&app);

    open_with_corpus(&app, &mut h, "alpha\nbeta\n");
    assert_eq!(
        app.borrow().search_match_count(),
        0,
        "an empty query must yield no matches"
    );
}

/// The case-sensitivity toggle changes the match set: case-insensitive (default)
/// finds both `Error` and `error`; flipping to case-sensitive finds only the
/// exact-case query.
#[test]
fn case_toggle_changes_matches() {
    let app = RefCell::new(C0pl4ndApp::bootstrap());
    let mut h = harness(&app);

    open_with_corpus(&app, &mut h, "Error one\nerror two\n");
    type_query(&mut h, "error");
    assert_eq!(
        app.borrow().search_match_count(),
        2,
        "case-insensitive default finds both 'Error' and 'error'"
    );

    // Flip to case-sensitive and recompute (drive the production toggle path).
    app.borrow_mut().test_set_case_sensitive(true);
    h.step();
    assert!(
        app.borrow().search_case_sensitive_enabled(),
        "the case toggle must be on"
    );
    assert_eq!(
        app.borrow().search_match_count(),
        1,
        "case-sensitive must match only the lowercase 'error'"
    );
}

/// The regex toggle changes interpretation: a regex anchor `^error` matches only
/// the line that starts with it, where the literal would match anywhere.
#[test]
fn regex_toggle_changes_matches() {
    let app = RefCell::new(C0pl4ndApp::bootstrap());
    let mut h = harness(&app);

    open_with_corpus(&app, &mut h, "error first\nan error mid\n");
    type_query(&mut h, "^error");
    // As a LITERAL, "^error" matches nothing (no caret in the corpus).
    assert_eq!(
        app.borrow().search_match_count(),
        0,
        "the literal '^error' matches no line"
    );

    // Flip to regex: '^error' now anchors to the start, matching the first line.
    app.borrow_mut().test_set_regex(true);
    h.step();
    assert!(
        app.borrow().search_regex_enabled(),
        "the regex toggle must be on"
    );
    assert_eq!(
        app.borrow().search_match_count(),
        1,
        "as a regex, '^error' matches only the start-anchored line"
    );
}

/// An invalid regex does NOT panic — the core matcher yields no matches and the
/// overlay surfaces it calmly as zero.
#[test]
fn invalid_regex_yields_zero_not_panic() {
    let app = RefCell::new(C0pl4ndApp::bootstrap());
    let mut h = harness(&app);

    open_with_corpus(&app, &mut h, "anything\n");
    app.borrow_mut().test_set_regex(true);
    type_query(&mut h, "(unclosed");
    assert_eq!(
        app.borrow().search_match_count(),
        0,
        "an invalid regex must yield zero matches, never panic"
    );
    assert!(
        app.borrow().search_is_open(),
        "the overlay stays open and usable after a bad regex"
    );
}

/// Enter / F3 cycle the selection forward and wrap; Shift+F3 cycles back.
#[test]
fn enter_f3_cycle_forward_and_wrap_shift_f3_back() {
    let app = RefCell::new(C0pl4ndApp::bootstrap());
    let mut h = harness(&app);

    // Three matches across three lines → selection cycles 0,1,2,0,…
    open_with_corpus(&app, &mut h, "hit a\nhit b\nhit c\n");
    type_query(&mut h, "hit");
    assert_eq!(app.borrow().search_match_count(), 3, "three matches");
    assert_eq!(app.borrow().search_selected(), 0, "selection starts at 0");

    press(&mut h, egui::Key::Enter);
    assert_eq!(app.borrow().search_selected(), 1, "Enter steps to match 1");

    press(&mut h, egui::Key::F3);
    assert_eq!(app.borrow().search_selected(), 2, "F3 steps to match 2");

    press(&mut h, egui::Key::F3);
    assert_eq!(
        app.borrow().search_selected(),
        0,
        "F3 from the last match wraps to the first"
    );

    // Shift+F3 steps backward (and wraps from 0 to the last).
    h.event(egui::Event::Key {
        key: egui::Key::F3,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers {
            shift: true,
            ..Default::default()
        },
    });
    h.step();
    assert_eq!(
        app.borrow().search_selected(),
        2,
        "Shift+F3 from match 0 wraps back to the last match"
    );
}
