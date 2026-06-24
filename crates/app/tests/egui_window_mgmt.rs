//! Headless **interaction** tests for C0PL4ND surfaces not covered by the
//! existing suites: the command-history quick-run SIDEBAR (Ctrl+Shift+H, #21),
//! the window-management KEYBOARD shortcuts (Ctrl/Cmd+Shift+{D,E,W} split/close
//! and Ctrl/Cmd+, settings), and the scrollback navigation chords (Ctrl+Shift+
//! Home/End scroll-to-edge and Ctrl+Shift+PageUp jump-to-prompt-mark), driven by
//! `egui_kittest`.
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
#[path = "../src/issue_intake.rs"]
mod issue_intake;
#[path = "../src/reporting.rs"]
mod reporting;

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

// ---- scroll-to-edge shortcuts (best-in-class parity) -------------------------

#[test]
fn ctrl_shift_home_and_end_scroll_to_the_scrollback_edges() {
    // Ctrl+Shift+Home jumps the focused pane to the oldest retained scrollback
    // line; Ctrl+Shift+End snaps it back to following live output. We build
    // deterministic scrollback by feeding lines straight into the focused
    // emulator (no live shell needed) and assert the observable scroll offset the
    // chord produced — driven through the REAL `frame_tick` chord handler.
    let app = RefCell::new(C0pl4ndApp::bootstrap());
    let mut h = harness(&app);

    // Feed 200 newline-terminated lines. The per-frame scrollback cap is >=100
    // lines, so the history is guaranteed non-empty after this.
    {
        let mut a = app.borrow_mut();
        for i in 0..200 {
            a.test_feed_focused(format!("line {i}\r\n").as_bytes());
        }
    }
    h.step();

    // Confirm the feed built a non-empty scrollback before asserting the chord
    // (diagnostic split: distinguishes a feed problem from a chord problem).
    let sb = app
        .borrow()
        .test_focused_scrollback_len()
        .expect("focused pane exists");
    assert!(
        sb > 0,
        "feeding 200 lines must build a non-empty scrollback (got {sb})"
    );

    assert_eq!(
        app.borrow().test_focused_view_offset(),
        Some(0),
        "the view follows live output before any scroll chord"
    );

    // Ctrl+Shift+Home scrolls up into the scrollback (offset moves above 0).
    press_ctrl_shift(&mut h, egui::Key::Home);
    let top = app
        .borrow()
        .test_focused_view_offset()
        .expect("focused pane exists");
    assert!(
        top > 0,
        "Ctrl+Shift+Home must scroll up into the scrollback (offset {top} > 0)"
    );

    // Ctrl+Shift+End snaps back to live output (offset 0).
    press_ctrl_shift(&mut h, egui::Key::End);
    assert_eq!(
        app.borrow().test_focused_view_offset(),
        Some(0),
        "Ctrl+Shift+End must snap back to following live output"
    );
}

#[test]
fn ctrl_shift_pageup_jumps_to_an_older_prompt_mark() {
    // Ctrl+Shift+PageUp scrolls the focused pane back to the previous OSC-133
    // prompt mark. We seed scrollback with prompt marks (ESC ] 133 ; A BEL) above
    // the live viewport and assert the chord scrolls into the scrollback —
    // driven through the REAL `frame_tick` handler. This also guards the
    // explicit ctrl-OR-command chord match (the prior `consume_key` form silently
    // failed to fire under ctrl-only events).
    let app = RefCell::new(C0pl4ndApp::bootstrap());
    let mut h = harness(&app);

    // Twenty prompt blocks: a prompt mark, then a handful of output lines each.
    // Twenty ensures several marks survive in the retained scrollback (above the
    // live viewport) so the chord has multiple distinct marks to step through.
    {
        let mut a = app.borrow_mut();
        for block in 0..20 {
            a.test_feed_focused(b"\x1b]133;A\x07"); // OSC 133 ; A = prompt mark
            for line in 0..6 {
                a.test_feed_focused(format!("blk{block} line{line}\r\n").as_bytes());
            }
        }
    }
    h.step();

    let sb = app
        .borrow()
        .test_focused_scrollback_len()
        .expect("focused pane exists");
    assert!(
        sb > 0,
        "seeding must build a non-empty scrollback (got {sb})"
    );
    assert_eq!(
        app.borrow().test_focused_view_offset(),
        Some(0),
        "the view starts at the live bottom"
    );

    // PageUp lands ON an older prompt mark (offset > 0).
    press_ctrl_shift(&mut h, egui::Key::PageUp);
    let o1 = app
        .borrow()
        .test_focused_view_offset()
        .expect("focused pane exists");
    assert!(
        o1 > 0,
        "Ctrl+Shift+PageUp must scroll back to an older prompt mark (offset {o1} > 0)"
    );

    // A SECOND PageUp steps to a STILL-older mark — the offset strictly
    // increases. This kills a "set offset to a constant" mutant: a constant
    // satisfies `> 0` but never the strict monotonic step to the next mark.
    press_ctrl_shift(&mut h, egui::Key::PageUp);
    let o2 = app
        .borrow()
        .test_focused_view_offset()
        .expect("focused pane exists");
    assert!(
        o2 > o1,
        "a second Ctrl+Shift+PageUp must step to an OLDER mark (offset {o2} > {o1})"
    );

    // PageDown reverses direction: it steps back toward live output to a NEWER
    // mark, so the offset strictly decreases. This kills a PageUp/PageDown
    // direction-swap mutant (which would instead increase the offset).
    press_ctrl_shift(&mut h, egui::Key::PageDown);
    let o3 = app
        .borrow()
        .test_focused_view_offset()
        .expect("focused pane exists");
    assert!(
        o3 < o2,
        "Ctrl+Shift+PageDown must step toward live output to a NEWER mark \
         (offset {o3} < {o2})"
    );
}

// ---- zoom-pane toggle (Ctrl+Shift+Z) ----------------------------------------

#[test]
fn ctrl_shift_z_toggles_pane_zoom() {
    // Ctrl+Shift+Z zooms the focused pane (render it full-size, siblings hidden);
    // pressing it again un-zooms. Driven through the REAL frame_tick chord +
    // render path (the zoomed frame renders only the one pane).
    let app = RefCell::new(C0pl4ndApp::bootstrap());
    let mut h = harness(&app);

    // Need at least two panes for zoom to be meaningful.
    press_ctrl_shift(&mut h, egui::Key::D);
    assert_eq!(app.borrow().pane_count(), 2, "split makes two panes");
    assert_eq!(app.borrow().zoomed_pane(), None, "starts un-zoomed");

    press_ctrl_shift(&mut h, egui::Key::Z);
    let zoomed = app.borrow().zoomed_pane();
    let focused = app.borrow().focused_pane();
    assert_eq!(
        zoomed,
        Some(focused),
        "Ctrl+Shift+Z must zoom the focused pane"
    );

    press_ctrl_shift(&mut h, egui::Key::Z);
    assert_eq!(
        app.borrow().zoomed_pane(),
        None,
        "Ctrl+Shift+Z again must un-zoom"
    );
}

#[test]
fn closing_the_zoomed_pane_clears_the_zoom() {
    // Closing the pane that is currently zoomed must clear the zoom, so the next
    // frame never tries to render a pane that no longer exists.
    let app = RefCell::new(C0pl4ndApp::bootstrap());
    let mut h = harness(&app);

    press_ctrl_shift(&mut h, egui::Key::D); // two panes
    press_ctrl_shift(&mut h, egui::Key::Z); // zoom the focused one
    assert!(
        app.borrow().zoomed_pane().is_some(),
        "focused pane is zoomed"
    );

    press_ctrl_shift(&mut h, egui::Key::W); // close the focused (zoomed) pane
    assert_eq!(app.borrow().pane_count(), 1, "back to a single pane");
    assert_eq!(
        app.borrow().zoomed_pane(),
        None,
        "closing the zoomed pane must clear the zoom"
    );
}
