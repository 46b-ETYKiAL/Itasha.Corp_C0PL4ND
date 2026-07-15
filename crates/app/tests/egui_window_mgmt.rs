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
//! Most outcomes here are PTY-independent: the history feeds from a seeded
//! command history and the chord handlers act on app state, so those tests are
//! stable on a CI box with no usable PTY. Where a test needs a populated history
//! it SEEDS the history via the real type-then-Enter capture path (echo-gated,
//! like the palette suite) so the sidebar lists real recorded commands.
//!
//! The three scrollback/copy tests are the exception and DO require a live PTY:
//! they call [`ensure_focused_spawned`], which waits for the pane's emulator to
//! spawn and for the shell's startup banner to land before feeding synthetic
//! rows. See that helper for why the second wait is not optional.

use c0pl4nd::egui_app;
use std::cell::RefCell;
use std::time::{Duration, Instant};

use egui_kittest::kittest::Queryable;
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

/// How long to wait for the real shell to echo a typed line.
///
/// Generous on purpose. The wait ends the instant the echo lands, so a large bound
/// costs a healthy run NOTHING — it only decides how much scheduler starvation is
/// tolerated before the suite calls the shell dead. A sibling suite hit exactly
/// that: under ~27 concurrent rustc processes the echo took >10s and tests failed
/// with no product defect. This is not a threshold being widened to hide a
/// failure; the assertion is unchanged and a shell that never echoes still fails,
/// just loudly and after more patience.
const ECHO_TIMEOUT: Duration = Duration::from_secs(45);

/// Poll until the focused pane's grid shows `needle` (the shell echoed it), so
/// the echo-gated history capture records the line deterministically.
fn wait_for_echo(h: &mut Harness<'_>, app: &RefCell<C0pl4ndApp>, needle: &str) {
    let deadline = Instant::now() + ECHO_TIMEOUT;
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
    // Fail LOUD. This used to fall out of the loop and return normally, so a
    // timed-out wait silently seeded nothing and the caller then asserted against
    // an empty history — surfacing as a baffling "the sidebar lists the wrong
    // commands" that reads like a product bug. A wait that gives up must say so.
    panic!(
        "the shell never echoed {needle:?} within {ECHO_TIMEOUT:?} — the command \
         history could not be seeded (the shell is not up, not a product bug)"
    );
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
fn a_consumed_chord_fires_its_action_and_leaks_no_byte_to_the_pty() {
    // SECURITY/correctness: a window-management chord (here Ctrl+Shift+D = split)
    // must be CONSUMED before reaching the PTY — its control byte must NOT also be
    // forwarded to the shell. An action-only assertion (pane_count grew) would
    // NOT catch a regression where the chord's `events.retain` keeps the event:
    // that would fire the action AND forward the byte. Assert BOTH: the action
    // fired AND nothing was forwarded this frame.
    let app = RefCell::new(C0pl4ndApp::bootstrap());
    let before = app.borrow().pane_count();
    let mut h = harness(&app);

    press_ctrl_shift(&mut h, egui::Key::D);

    assert_eq!(
        app.borrow().pane_count(),
        before + 1,
        "Ctrl+Shift+D fires its action (the pane split)"
    );
    assert!(
        app.borrow().test_last_forwarded().is_empty(),
        "the consumed chord must forward NO byte to the PTY (got {:?})",
        app.borrow().test_last_forwarded()
    );
}

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

#[test]
fn opening_a_new_tab_while_zoomed_exits_zoom() {
    // A zoom renders ONLY the zoomed pane, but focus can move to a DIFFERENT pane
    // while zoomed (here via Ctrl+Shift+T opening a new, focused tab). The
    // zoom↔focus reconcile must drop the zoom so the newly-focused pane is the one
    // shown — otherwise the screen keeps showing the old pane while keystrokes go
    // to the now-focused hidden pane (a silent display/input mismatch).
    let app = RefCell::new(C0pl4ndApp::bootstrap());
    let mut h = harness(&app);

    press_ctrl_shift(&mut h, egui::Key::Z); // zoom the focused pane
    assert!(
        app.borrow().zoomed_pane().is_some(),
        "the focused pane is zoomed"
    );

    press_ctrl_shift(&mut h, egui::Key::T); // new tab → focus moves to the new pane
    h.step(); // the start-of-frame reconcile runs on the next frame
    assert_eq!(
        app.borrow().zoomed_pane(),
        None,
        "opening a new tab (focus diverges from the zoomed pane) must exit zoom"
    );
    assert_eq!(
        app.borrow().pane_count(),
        2,
        "the new tab was created (the focus divergence that triggered the un-zoom)"
    );
}

// ---- directional pane focus (Ctrl+Shift+Arrow) ------------------------------

#[test]
fn ctrl_shift_arrow_moves_directional_pane_focus_across_a_split() {
    // Ctrl/Cmd+Shift+Arrow moves keyboard focus to the geometrically adjacent
    // pane. Build a side-by-side split, render so both pane rects are captured,
    // then assert Left and Right address two DIFFERENT panes (robust to whichever
    // pane the split leaves focused). Driven through the REAL frame_tick path
    // (the chord is intercepted before the arrow reaches the PTY).
    let app = RefCell::new(C0pl4ndApp::bootstrap());
    let mut h = harness(&app);

    press_ctrl_shift(&mut h, egui::Key::D); // split right → two side-by-side panes
    assert_eq!(app.borrow().pane_count(), 2, "split makes two panes");
    h.run(); // a full render populates pane_rects for both panes

    press_ctrl_shift(&mut h, egui::Key::ArrowLeft);
    let after_left = app.borrow().focused_pane();
    press_ctrl_shift(&mut h, egui::Key::ArrowRight);
    let after_right = app.borrow().focused_pane();
    assert_ne!(
        after_left, after_right,
        "Ctrl+Shift+Left and Ctrl+Shift+Right must focus the two different panes"
    );

    // At the left edge there is no pane further left, so a second Left is a no-op.
    press_ctrl_shift(&mut h, egui::Key::ArrowLeft);
    assert_eq!(
        app.borrow().focused_pane(),
        after_left,
        "Ctrl+Shift+Left at the left edge is idempotent (no pane further left)"
    );
}

// ---- pointer-driven mouse gestures ------------------------------------------

/// Send a primary pointer button press/release at `pos`.
fn pointer_primary(h: &mut Harness<'_>, pos: egui::Pos2, pressed: bool) {
    h.event(egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::default(),
    });
}

#[test]
fn mouse_wheel_scrolls_the_pane_scrollback() {
    // Hovering a pane and rolling the wheel UP scrolls its LOCAL scrollback back
    // into history (no program has grabbed the mouse). Driven through the real
    // frame_tick wheel handler; the observable is the focused pane's view offset.
    let app = RefCell::new(C0pl4ndApp::bootstrap());
    let mut h = harness(&app);
    {
        let mut a = app.borrow_mut();
        for i in 0..200 {
            a.test_feed_focused(format!("line {i}\r\n").as_bytes());
        }
    }
    h.run();
    assert_eq!(
        app.borrow().test_focused_view_offset(),
        Some(0),
        "the view starts at the live bottom"
    );

    // Hover the pane centre, then wheel up (positive y → back into history).
    h.event(egui::Event::PointerMoved(egui::pos2(600.0, 400.0)));
    h.step();
    h.event(egui::Event::MouseWheel {
        unit: egui::MouseWheelUnit::Line,
        delta: egui::vec2(0.0, 5.0),
        phase: egui::TouchPhase::Move,
        modifiers: egui::Modifiers::default(),
    });
    h.step();

    let off = app
        .borrow()
        .test_focused_view_offset()
        .expect("focused pane exists");
    assert!(
        off > 0,
        "wheel up must scroll into the pane scrollback (offset {off} > 0)"
    );
}

#[test]
fn primary_drag_selects_text_in_the_pane() {
    // A primary-button drag across the pane grid selects text (line-wise). With
    // copy_on_select off (the default) the selection persists after release, so
    // we can assert the non-empty selection the drag produced.
    let app = RefCell::new(C0pl4ndApp::bootstrap());
    let mut h = harness(&app);
    {
        let mut a = app.borrow_mut();
        a.test_feed_focused(b"hello world this is a line of selectable text\r\n");
    }
    h.run();
    assert!(
        app.borrow().test_selection().is_none(),
        "there is no selection before the drag"
    );

    // Press at one cell and drag well to the right (past the drag threshold).
    let p0 = egui::pos2(120.0, 200.0);
    let p1 = egui::pos2(420.0, 200.0);
    h.event(egui::Event::PointerMoved(p0));
    pointer_primary(&mut h, p0, true);
    h.step();
    h.event(egui::Event::PointerMoved(p1));
    h.step();
    pointer_primary(&mut h, p1, false);
    h.step();

    let (anchor, head, is_block) = app
        .borrow()
        .test_selection()
        .expect("a primary drag must produce a selection");
    assert_ne!(anchor, head, "the drag selects a non-empty range");
    assert!(!is_block, "a plain (non-Alt) drag is line-wise, not block");
    // NOTE: Alt+drag (block selection) is exercised deterministically by the core
    // `selection_text_block_mode_clips_every_row_to_the_column_range` unit test
    // (the rectangular extraction). It is not driven here because egui's kittest
    // synthetic drag detection does not register a drag when a modifier is held
    // on the pointer events (a harness limitation, not a real-app behaviour).
}

// ---- clear-scrollback + copy-all conveniences -------------------------------

/// Wait out the focused pane's deferred first-frame PTY spawn AND the spawned
/// shell's own startup output, so a subsequent `test_feed_focused` is neither
/// silently dropped nor wiped out from under the test.
///
/// Two separate waits, and both are load-bearing:
///
/// 1. **Spawn.** The initial pane's emulator is created lazily during the first
///    body render, and `test_feed_focused` no-ops on a pane that has not spawned.
///
/// 2. **Shell startup.** `test_focused_alive()` goes true as soon as the PTY
///    exists — but the shell has not written anything yet. On Windows, conhost
///    then emits its banner (`Microsoft Windows [Version ...]` + prompt) roughly
///    40ms later, and that startup write RESETS THE SCREEN. A test that feeds
///    synthetic rows into that window has its grid blanked mid-test when the
///    banner lands: the fed rows already in scrollback survive, everything still
///    on screen does not.
///
/// Waiting only for (1) made these tests race the banner and win only by
/// finishing first — measured, the whole test took ~10ms against a ~42ms banner.
/// That is an accidental pass, and it inverts under load: with the machine busy
/// the banner arrives mid-test and `ctrl_shift_a_copies_the_whole_buffer_to_the
/// _clipboard` failed ~2 runs in 10 (and once in a full instrumented sweep),
/// reporting the newest fed row missing while the oldest survived.
///
/// So wait for the shell's first output before handing the pane to a test. Once
/// the banner has landed the emulator is quiescent — the shell writes nothing
/// more until it is sent input — so anything fed after this point stays put.
/// This waits on an OBSERVABLE condition (real output reached the emulator), not
/// a fixed sleep, so it cannot be tuned into flakiness by a slower box.
fn ensure_focused_spawned(h: &mut Harness<'_>, app: &RefCell<C0pl4ndApp>) {
    for _ in 0..120 {
        if app.borrow().test_focused_alive() {
            break;
        }
        h.run();
    }
    if !app.borrow().test_focused_alive() {
        panic!("the focused pane never spawned its emulator");
    }

    // The pane is alive; now let the shell finish announcing itself.
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline {
        h.step();
        let landed = app
            .borrow()
            .test_focused_buffer_text()
            .is_some_and(|t| !t.trim().is_empty());
        if landed {
            // The banner is in; drain the remaining startup chunks (the prompt
            // usually arrives a frame or two after the version lines) so the
            // screen is settled, not mid-write.
            for _ in 0..20 {
                h.step();
            }
            return;
        }
    }
    panic!(
        "the spawned shell never produced startup output — the pane is alive but \
         silent, so a fed row could still be wiped by a late banner"
    );
}

/// The `OutputCommand::CopyText` payload emitted on the last frame, if any — how
/// a test observes what a copy chord actually placed on the clipboard. Reads the
/// harness's captured `FullOutput` (kittest applies but also retains it), so the
/// assertion is on the REAL platform-output the frame produced.
fn last_copied_text(h: &Harness<'_>) -> Option<String> {
    h.output()
        .platform_output
        .commands
        .iter()
        .find_map(|cmd| match cmd {
            egui::OutputCommand::CopyText(t) => Some(t.clone()),
            _ => None,
        })
}

#[test]
fn ctrl_shift_k_clears_the_focused_scrollback() {
    // Ctrl+Shift+K (WezTerm's clear-scrollback chord) drops the focused pane's
    // scrollback history while keeping the live screen. Build deterministic
    // scrollback by feeding lines into the focused emulator, confirm it is
    // non-empty, then assert the chord empties it — driven through the REAL
    // frame_tick chord handler.
    let app = RefCell::new(C0pl4ndApp::bootstrap());
    let mut h = harness(&app);
    ensure_focused_spawned(&mut h, &app);
    {
        let mut a = app.borrow_mut();
        for i in 0..200 {
            a.test_feed_focused(format!("line {i}\r\n").as_bytes());
        }
    }
    h.step();

    let sb = app
        .borrow()
        .test_focused_scrollback_len()
        .expect("focused pane exists");
    assert!(sb > 0, "feeding 200 lines must build scrollback (got {sb})");

    press_ctrl_shift(&mut h, egui::Key::K);
    assert_eq!(
        app.borrow().test_focused_scrollback_len(),
        Some(0),
        "Ctrl+Shift+K must clear the focused pane's scrollback"
    );
}

#[test]
fn ctrl_shift_a_copies_the_whole_buffer_to_the_clipboard() {
    // Ctrl+Shift+A (the "Select all" → copy convention of Windows Terminal /
    // Ghostty) copies the focused pane's ENTIRE buffer (scrollback + screen) to
    // the clipboard — the no-selection companion to Ctrl+Shift+C. We feed known
    // lines, drive the chord through the REAL frame_tick handler, and assert the
    // clipboard payload the chord actually emitted contains them.
    let app = RefCell::new(C0pl4ndApp::bootstrap());
    let mut h = harness(&app);
    ensure_focused_spawned(&mut h, &app);
    {
        let mut a = app.borrow_mut();
        for i in 0..50 {
            a.test_feed_focused(format!("row{i}\r\n").as_bytes());
        }
    }
    h.step();

    // The extracted payload spans scrollback + screen (an early row scrolled off
    // into history AND a late row on-screen must both be present).
    let payload = app
        .borrow()
        .test_focused_buffer_text()
        .expect("a non-empty buffer yields copy-all text");
    assert!(
        payload.contains("row0"),
        "the oldest fed row is in the buffer"
    );
    assert!(
        payload.contains("row49"),
        "the newest fed row is in the buffer"
    );

    press_ctrl_shift(&mut h, egui::Key::A);
    let copied = last_copied_text(&h).expect("Ctrl+Shift+A must place the buffer on the clipboard");
    assert!(
        copied.contains("row0") && copied.contains("row49"),
        "the clipboard payload must be the whole buffer (got {} bytes)",
        copied.len()
    );
}

/// Open the pane's right-click context menu: move the pointer over the pane
/// centre, then press+release the SECONDARY button there (egui opens a
/// `context_menu` on secondary-click release over the response).
fn open_pane_context_menu(h: &mut Harness<'_>) {
    let p = egui::pos2(600.0, 400.0);
    h.event(egui::Event::PointerMoved(p));
    h.step();
    for pressed in [true, false] {
        h.event(egui::Event::PointerButton {
            pos: p,
            button: egui::PointerButton::Secondary,
            pressed,
            modifiers: egui::Modifiers::default(),
        });
    }
    h.step();
}

#[test]
fn context_menu_clear_scrollback_empties_the_scrollback() {
    // The right-click "Clear scrollback" menu item drops the focused pane's
    // scrollback. This drives the REAL popup: open the context menu with a
    // secondary click, then click the item by its accessible label and assert the
    // observable scrollback goes empty. The menu can need a re-open between frames
    // (the popup is a transient Area), so retry until the item's effect lands.
    let app = RefCell::new(C0pl4ndApp::bootstrap());
    let mut h = harness(&app);
    ensure_focused_spawned(&mut h, &app);
    {
        let mut a = app.borrow_mut();
        for i in 0..200 {
            a.test_feed_focused(format!("line {i}\r\n").as_bytes());
        }
    }
    h.run();
    assert!(
        app.borrow().test_focused_scrollback_len().unwrap_or(0) > 0,
        "feeding must build scrollback before the menu clears it"
    );

    let mut cleared = false;
    for _ in 0..80 {
        if app.borrow().test_focused_scrollback_len() == Some(0) {
            cleared = true;
            break;
        }
        if let Some(item) = h.query_by_label("Clear scrollback") {
            item.click();
            h.run();
        } else {
            open_pane_context_menu(&mut h);
        }
    }
    assert!(
        cleared,
        "the context-menu 'Clear scrollback' item must empty the scrollback"
    );
}
