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
use egui_phosphor::thin as icon;

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
fn clicking_plus_splits_a_new_pane() {
    let app = RefCell::new(C0pl4ndApp::bootstrap());
    let before = app.borrow().pane_count();
    let mut h = harness(&app);

    h.get_by_label(icon::COLUMNS).click();
    h.run();

    let after = app.borrow().pane_count();
    assert_eq!(
        after,
        before + 1,
        "clicking + must spawn exactly one pane (before={before}, after={after})"
    );
}

#[test]
fn clicking_split_down_splits_a_new_pane() {
    let app = RefCell::new(C0pl4ndApp::bootstrap());
    let before = app.borrow().pane_count();
    let mut h = harness(&app);

    h.get_by_label(icon::ROWS).click();
    h.run();

    assert_eq!(
        app.borrow().pane_count(),
        before + 1,
        "split-down adds a pane"
    );
}

#[test]
fn clicking_a_tab_changes_the_focused_pane() {
    // Bootstrap opens two panes (ids 0 and 1); pane 0 is focused initially.
    let app = RefCell::new(C0pl4ndApp::bootstrap());
    assert_eq!(
        app.borrow().focused_pane(),
        PaneId(0),
        "pane 0 focused at start"
    );
    let mut h = harness(&app);

    // Click the OTHER pane's tab (label "pane 1") and assert focus actually moved.
    h.get_by_label("pane 1").click();
    h.run();

    assert_eq!(
        app.borrow().focused_pane(),
        PaneId(1),
        "clicking the 'pane 1' tab must move focus to pane 1"
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

    // bootstrap=2 panes; click + four times to reach 6.
    for _ in 0..4 {
        h.get_by_label(icon::COLUMNS).click();
        h.run();
    }
    assert_eq!(app.borrow().pane_count(), 6, "reached the 6-pane cap");

    // One more + must be refused (count stays 6).
    h.get_by_label(icon::COLUMNS).click();
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
