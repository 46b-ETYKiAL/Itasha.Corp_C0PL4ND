//! Headless **interaction** tests for the C0PL4ND egui chrome (Milestone 1),
//! driven by `egui_kittest`.
//!
//! ## Discipline (non-negotiable)
//!
//! Every test here drives the **real** production frame loop
//! ([`C0pl4ndApp::frame_tick`]) and clicks the **real** widgets by their
//! accessible label, then asserts the **observable outcome** the click actually
//! caused. There is NO test-only mirror of the frame loop and NO
//! set-state-then-assert-the-same-state tautology — those are exactly how
//! "clicking a tab does nothing" and "the ✕ doesn't close" ship. A control is
//! only considered working when a simulated click here produces its real effect.
//!
//! The app module is compiled into this test binary via `#[path]` so the test
//! can construct `C0pl4ndApp` directly (no eframe window). The closure handed to
//! `Harness::new` calls `frame_tick` verbatim — the same function the binary's
//! `eframe::App::ui` calls — so what the test exercises is what ships.

#![allow(dead_code)] // The `#[path]`-included module has production entry points
                     // (eframe `App` impl, `apply_window_effect`) that this test
                     // binary does not call; they are legitimately unused here.

#[path = "../src/egui_app/mod.rs"]
mod egui_app;

use std::cell::RefCell;

use egui_kittest::kittest::Queryable;
use egui_kittest::Harness;

use egui_app::grid::PaneId;
use egui_app::{C0pl4ndApp, WindowCmd};

/// Build a headless harness that drives the REAL `frame_tick` for one shared app
/// instance. The same function the shipping binary runs each frame — so a click
/// the harness delivers travels the exact production path (widget → action →
/// state mutation), with no test-only shim that could drift from the app.
fn harness(app: &RefCell<C0pl4ndApp>) -> Harness<'_> {
    // `Harness::new` (the Context-closure form) is marked deprecated in
    // egui_kittest 0.34 in favour of `new_ui`, but `new_ui` gives only a `&mut
    // Ui` — and egui_kittest's own `new_ui` docs say: "If you need to create
    // Windows / Panels, you can use `Harness::new` instead." `frame_tick` builds
    // `TopBottomPanel`/`CentralPanel`/`Window`, so the Context-closure form is
    // the correct one here. Allow the deprecation deliberately.
    #[allow(deprecated)]
    Harness::new(move |ctx| app.borrow_mut().frame_tick(ctx))
}

#[test]
fn clicking_new_terminal_adds_a_pane() {
    // Bootstrap opens ONE pane; the single "+" button adds another.
    let app = RefCell::new(C0pl4ndApp::bootstrap());
    let before = app.borrow().pane_count();
    assert_eq!(before, 1, "app opens with a single terminal");
    let mut h = harness(&app);

    h.get_by_label("new terminal").click();
    h.run();

    let after = app.borrow().pane_count();
    assert_eq!(
        after,
        before + 1,
        "clicking the new-terminal button must spawn exactly one pane (before={before}, after={after})"
    );
}

#[test]
fn clicking_a_tab_changes_the_focused_pane() {
    // Bootstrap opens one pane (id 0, focused). Add a second, then click back.
    let app = RefCell::new(C0pl4ndApp::bootstrap());
    assert_eq!(
        app.borrow().focused_pane(),
        PaneId(0),
        "pane 0 focused at start"
    );
    let mut h = harness(&app);

    // Add a second terminal → focus moves to the new pane (id 1).
    h.get_by_label("new terminal").click();
    h.run();
    assert_eq!(
        app.borrow().focused_pane(),
        PaneId(1),
        "a new terminal takes focus"
    );

    // Click pane 0's tab → focus moves back to pane 0.
    h.get_by_label("pane 0").click();
    h.run();
    assert_eq!(
        app.borrow().focused_pane(),
        PaneId(0),
        "clicking the 'pane 0' tab must move focus to pane 0"
    );
}

#[test]
fn clicking_gear_opens_settings() {
    let app = RefCell::new(C0pl4ndApp::bootstrap());
    assert!(!app.borrow().settings_is_open(), "settings closed at start");
    let mut h = harness(&app);

    h.get_by_label("settings").click();
    h.run();

    assert!(
        app.borrow().settings_is_open(),
        "clicking the gear must open the settings window"
    );
}

#[test]
fn settings_close_button_actually_closes_the_window() {
    // The user's canonical case: do NOT assume the framework's close works —
    // open the window, click its real ✕/Close button, and prove it closed.
    let app = RefCell::new(C0pl4ndApp::bootstrap());
    let mut h = harness(&app);

    // Open settings (gear) — the egui Window renders in the same frame_tick.
    h.get_by_label("settings").click();
    h.run();
    assert!(
        app.borrow().settings_is_open(),
        "precondition: settings open"
    );

    // Click the egui Window's own close button (egui labels it "Close window",
    // distinct from the caption "✕" — verified from the accesskit node dump).
    h.get_by_label("Close window").click();
    h.run();

    assert!(
        !app.borrow().settings_is_open(),
        "clicking the settings window's Close button must actually close it"
    );
}

#[test]
fn clicking_close_caption_issues_a_close_command() {
    let app = RefCell::new(C0pl4ndApp::bootstrap());
    assert_eq!(app.borrow().last_window_cmd(), None);
    let mut h = harness(&app);

    h.get_by_label("close").click();
    h.run();

    assert_eq!(
        app.borrow().last_window_cmd(),
        Some(WindowCmd::Close),
        "the ✕ caption button must issue a real Close window command"
    );
}

#[test]
fn clicking_minimize_caption_issues_a_minimize_command() {
    let app = RefCell::new(C0pl4ndApp::bootstrap());
    let mut h = harness(&app);

    h.get_by_label("minimize").click();
    h.run();

    assert_eq!(app.borrow().last_window_cmd(), Some(WindowCmd::Minimize));
}

#[test]
fn clicking_maximize_caption_issues_a_maximize_command() {
    let app = RefCell::new(C0pl4ndApp::bootstrap());
    let mut h = harness(&app);

    h.get_by_label("maximize").click();
    h.run();

    assert_eq!(
        app.borrow().last_window_cmd(),
        Some(WindowCmd::ToggleMaximize)
    );
}

#[test]
fn splitting_past_six_panes_is_refused() {
    // Click + until the 6-pane cap, then once more, and assert the cap holds —
    // the real cap logic in frame_tick, exercised by real clicks.
    let app = RefCell::new(C0pl4ndApp::bootstrap());
    let mut h = harness(&app);

    // bootstrap=1 pane; click "new terminal" five times to reach 6.
    for _ in 0..5 {
        h.get_by_label("new terminal").click();
        h.run();
    }
    assert_eq!(app.borrow().pane_count(), 6, "reached the 6-pane cap");

    // One more must be refused (count stays 6).
    h.get_by_label("new terminal").click();
    h.run();
    assert_eq!(
        app.borrow().pane_count(),
        6,
        "the 6-pane cap must hold against a 7th split"
    );
}

#[test]
fn caption_cluster_is_flush_right() {
    // Bug-3 guard: the caption cluster (⚙ — ◻ ✕) must hug the window's RIGHT
    // edge at a known width — not float mid-strip (the reported layout bug). The
    // close button is the rightmost control, so its right edge must sit within a
    // few logical px of the titlebar's right edge. The old nested-layout code
    // floated the cluster after the leftover width, failing this.
    let app = RefCell::new(C0pl4ndApp::bootstrap());
    let win_w = 900.0;
    // The `build(ctx-closure)` form is deprecated in favour of `build_ui`, but
    // `build_ui` hands only a `&mut Ui` — `frame_tick` builds TopBottom/Central
    // panels, so the ctx-closure form is the correct one (same deliberate
    // deprecation allowance as the shared `harness()` helper above).
    #[allow(deprecated)]
    let mut h: Harness<'_> = Harness::builder()
        .with_size(egui::vec2(win_w, 600.0))
        .build(move |ctx| app.borrow_mut().frame_tick(ctx));
    h.run();

    let close = h.get_by_label("close");
    let close_rect = close.rect();
    let gear = h.get_by_label("settings");
    let gear_rect = gear.rect();

    // The close button's right edge must be within the titlebar inner margin
    // (6px) + a small tolerance of the window's right edge — i.e. flush right.
    assert!(
        close_rect.max.x >= win_w - 16.0,
        "close button right edge ({}) is not flush to the window right edge \
         ({win_w}); the caption cluster floated mid-strip (Bug 3)",
        close_rect.max.x
    );
    // Reading order at the far right is ⚙ — ◻ ✕: the gear (leftmost of the
    // cluster) sits to the LEFT of the close (rightmost).
    assert!(
        gear_rect.min.x < close_rect.min.x,
        "caption cluster must read ⚙ … ✕ left→right: gear ({}) must be left of \
         close ({})",
        gear_rect.min.x,
        close_rect.min.x
    );
}

#[test]
fn clicking_tab_pin_toggles_pinned() {
    // Bootstrap opens one pane (id 0); nothing pinned initially.
    let app = RefCell::new(C0pl4ndApp::bootstrap());
    assert!(!app.borrow().is_pinned(PaneId(0)), "pane 0 starts unpinned");
    let mut h = harness(&app);

    // Click pane 0's pin (label "pin pane 0") → it becomes pinned.
    h.get_by_label("pin pane 0").click();
    h.run();
    assert!(
        app.borrow().is_pinned(PaneId(0)),
        "clicking the tab pin must pin the pane"
    );

    // The pin relabels to "unpin pane 0"; clicking it unpins.
    h.get_by_label("unpin pane 0").click();
    h.run();
    assert!(
        !app.borrow().is_pinned(PaneId(0)),
        "clicking the pin again must unpin the pane"
    );
}

#[test]
fn clicking_tab_close_removes_the_pane() {
    // Open a second terminal so there are two panes (0, 1) to close one of.
    let app = RefCell::new(C0pl4ndApp::bootstrap());
    let mut h = harness(&app);
    h.get_by_label("new terminal").click();
    h.run();
    let before = app.borrow().pane_count();
    assert_eq!(before, 2, "two panes after adding one");

    // Click pane 1's close (label "close pane 1") → exactly one pane closes.
    h.get_by_label("close pane 1").click();
    h.run();

    let after = app.borrow().pane_count();
    assert_eq!(
        after,
        before - 1,
        "clicking a tab × must close exactly that pane (before={before}, after={after})"
    );
    // The surviving pane is focused (focus re-anchors off the closed pane).
    assert_eq!(
        app.borrow().focused_pane(),
        PaneId(0),
        "focus re-anchors to the surviving pane"
    );
}

#[test]
fn pinned_tab_has_no_close_button() {
    // A pinned tab hides its × so it can't be closed by accident (unpin first).
    // Open a second terminal so pane 1's close button is present to compare.
    let app = RefCell::new(C0pl4ndApp::bootstrap());
    let mut h = harness(&app);
    h.get_by_label("new terminal").click();
    h.run();

    // Both tabs start with a close button.
    assert!(
        h.query_by_label("close pane 0").is_some(),
        "an unpinned tab exposes a close button"
    );

    // Pin pane 0; its close button disappears, pane 1's remains.
    h.get_by_label("pin pane 0").click();
    h.run();
    assert!(
        h.query_by_label("close pane 0").is_none(),
        "a pinned tab must NOT expose a close button"
    );
    assert!(
        h.query_by_label("close pane 1").is_some(),
        "the still-unpinned tab keeps its close button"
    );
}
