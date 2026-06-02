//! Headless **interaction** tests for the C0PL4ND egui shell's LIVE TERMINAL
//! panes (Milestone 2), driven by `egui_kittest` + real PTYs.
//!
//! ## Discipline (non-negotiable)
//!
//! Every test here drives the **real** production frame loop
//! ([`C0pl4ndApp::frame_tick`]) with **simulated input** and asserts the
//! **observable outcome** the input actually caused — there is NO
//! set-state-then-assert-the-same-state tautology. Specifically these tests
//! prove the bug-prone core of Milestone 2:
//!
//! - **type → PTY round-trip**: simulated keystrokes reach the focused pane's
//!   real PTY AND the pane's grid updates with the shell's output. This is the
//!   exact "typing does nothing" failure class the milestone must guard.
//! - **pane focus**: clicking a pane routes input to it (and away from others).
//! - **resize → PTY**: shrinking a pane's rect resizes its PTY grid.
//!
//! The glyphon GPU paint callback cannot run under kittest's headless software
//! path (recon dossier §7) — so the app paints a text fallback when no GPU is
//! present, and these tests assert the PTY/input/resize LOGIC (the bug-prone
//! part). The pixel render is left to the offscreen `screenshot.rs` visual-QA.
//!
//! The app module is compiled in via `#[path]` so the test can construct
//! `C0pl4ndApp` directly; the closure handed to `Harness::new` calls the same
//! `frame_tick` the shipping binary runs each frame.

#![allow(dead_code)] // The `#[path]`-included module has production entry points
                     // (eframe `App` impl, `apply_window_effect`) unused here.

#[path = "../src/egui_app/mod.rs"]
mod egui_app;

use std::cell::RefCell;
use std::time::{Duration, Instant};

use egui_kittest::kittest::Queryable;
use egui_kittest::Harness;

use egui_app::grid::PaneId;
use egui_app::C0pl4ndApp;

/// Build a headless harness driving the REAL `frame_tick` for a shared app, with
/// a generous screen so panes get a real pixel rect (so resize math runs).
fn harness(app: &RefCell<C0pl4ndApp>) -> Harness<'_> {
    #[allow(deprecated)]
    let mut h = Harness::new(move |ctx| app.borrow_mut().frame_tick(ctx));
    h.set_size(egui::vec2(1000.0, 700.0));
    h.run();
    h
}

/// Type a string into the focused pane: queue one `egui::Event::Text` per char
/// and step a frame for each (the real `forward_input_to_focused` path).
fn type_text(h: &mut Harness<'_>, s: &str) {
    for ch in s.chars() {
        h.event(egui::Event::Text(ch.to_string()));
    }
    h.step();
}

/// Press Enter (a real `egui::Event::Key` — the shell sees `\r`).
fn press_enter(h: &mut Harness<'_>) {
    h.key_press(egui::Key::Enter);
    h.step();
}

/// Poll the focused pane's grid for `needle`, stepping frames + sleeping (the
/// PTY echoes/executes asynchronously, exactly like `e2e_terminal.rs` polls).
fn poll_focused_contains(
    h: &mut Harness<'_>,
    app: &RefCell<C0pl4ndApp>,
    needle: &str,
    timeout: Duration,
) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        h.step();
        if app
            .borrow()
            .focused_grid_text()
            .is_some_and(|t| t.contains(needle))
        {
            return true;
        }
        std::thread::sleep(Duration::from_millis(40));
    }
    // One last check after the final step.
    app.borrow()
        .focused_grid_text()
        .is_some_and(|t| t.contains(needle))
}

/// THE load-bearing test: type a command into the focused pane and assert the
/// echoed/executed output lands in that pane's REAL terminal grid. This proves
/// keystrokes reach the PTY AND the grid updates — the "typing does nothing"
/// failure class this milestone exists to guard.
#[test]
fn typing_a_command_reaches_the_pty_and_updates_the_grid() {
    let app = RefCell::new(C0pl4ndApp::bootstrap());
    // Skip cleanly if the platform shell could not spawn (no PTY on this box) —
    // never a false green: assert the pane is live before driving it.
    {
        let a = app.borrow();
        let focused = a.focused_pane();
        if a.pane_grid_text(focused).is_none() {
            eprintln!("no live PTY on this platform; skipping round-trip");
            return;
        }
    }
    let mut h = harness(&app);

    // A token that cannot pre-exist on the prompt line. `echo` it so the shell
    // prints it back (works on cmd.exe and POSIX sh — the default shells).
    let token = "c0pl4nd_grid_ok";
    type_text(&mut h, &format!("echo {token}"));
    press_enter(&mut h);

    let seen = poll_focused_contains(&mut h, &app, token, Duration::from_secs(8));
    assert!(
        seen,
        "the typed `echo {token}` must reach the PTY and its output must appear \
         in the focused pane's grid — proves keystrokes → PTY → grid"
    );
}

/// Pane focus: bootstrap opens two panes. Click pane 1's tab to focus it, type a
/// token, and assert it lands in pane 1's grid and NOT in pane 0's. This proves
/// input routes to the clicked pane (and away from the other).
#[test]
fn clicking_a_pane_routes_typed_input_to_that_pane_only() {
    let app = RefCell::new(C0pl4ndApp::bootstrap());
    {
        let a = app.borrow();
        if a.pane_grid_text(PaneId(0)).is_none() || a.pane_grid_text(PaneId(1)).is_none() {
            eprintln!("no live PTY on this platform; skipping focus routing");
            return;
        }
        assert_eq!(a.focused_pane(), PaneId(0), "pane 0 focused at start");
    }
    let mut h = harness(&app);

    // Focus pane 1 by clicking its tab (the real chrome path).
    h.get_by_label("pane 1").click();
    h.run();
    assert_eq!(
        app.borrow().focused_pane(),
        PaneId(1),
        "clicking 'pane 1' must focus pane 1"
    );

    // Type a unique token; it must land in pane 1's grid.
    let token = "c0pl4nd_pane1_only";
    type_text(&mut h, &format!("echo {token}"));
    press_enter(&mut h);

    let seen_in_1 = poll_focused_contains(&mut h, &app, token, Duration::from_secs(8));
    assert!(
        seen_in_1,
        "input must route to the focused (clicked) pane 1's PTY+grid"
    );
    // And it must NOT appear in pane 0 (input did not leak to the unfocused pane).
    let in_0 = app
        .borrow()
        .pane_grid_text(PaneId(0))
        .is_some_and(|t| t.contains(token));
    assert!(
        !in_0,
        "the token must NOT appear in the unfocused pane 0 — input must not leak"
    );
}

/// Resize → PTY: a pane laid out in a large window has a wide grid; shrinking the
/// window shrinks the pane rect, which must resize the pane's PTY grid (fewer
/// cols/rows). Drives the real `render_pane_body` resize path via the frame loop.
#[test]
fn shrinking_the_window_resizes_the_pane_pty() {
    let app = RefCell::new(C0pl4ndApp::bootstrap());
    {
        let a = app.borrow();
        if a.pane_grid_text(a.focused_pane()).is_none() {
            eprintln!("no live PTY on this platform; skipping resize");
            return;
        }
    }
    let mut h = harness(&app);
    h.set_size(egui::vec2(1200.0, 800.0));
    h.run();
    h.run();

    let focused = app.borrow().focused_pane();
    let big = app
        .borrow()
        .pane_size(focused)
        .expect("focused pane has a PTY size");
    assert!(
        big.0 >= 1 && big.1 >= 1,
        "a laid-out pane must have a positive grid size, got {big:?}"
    );

    // Shrink the window substantially; the pane rect shrinks with it.
    h.set_size(egui::vec2(360.0, 320.0));
    h.run();
    h.run();

    let small = app
        .borrow()
        .pane_size(focused)
        .expect("focused pane still has a PTY size");
    assert!(
        small.0 < big.0 || small.1 < big.1,
        "shrinking the window must shrink the pane's PTY grid \
         (was {big:?}, now {small:?})"
    );
}
