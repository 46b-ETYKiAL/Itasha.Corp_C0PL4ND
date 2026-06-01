//! Headless egui chrome tests (Milestone 1) via `egui_kittest`.
//!
//! These are STATE-assertion tests, not pixel snapshots — the recon dossier §7
//! deliberately keeps pixel snapshots out of CI for now (driver-version wobble).
//! The harness drives the real titlebar + grid closure; we click the `+`
//! split-right button by its accessible label and assert the pane count rises,
//! and toggle the settings gear and assert the window-open flag flips.
//!
//! The app module is compiled into the test binary via `#[path]` so the test
//! can construct `C0pl4ndApp` directly (no eframe / no window).

// The chrome module is compiled standalone into this test crate via `#[path]`.
// Production entry points it does not exercise here (`C0pl4ndApp::new`,
// `apply_window_effect`, and the eframe `App` impl) are legitimately unused in
// the test binary — allow dead code at the crate level so `-D warnings` stays
// green without sprinkling per-item allows across the shared module.
#![allow(dead_code)]

#[path = "../src/egui_app/mod.rs"]
mod egui_app;

use std::cell::RefCell;

use egui_kittest::kittest::Queryable;
use egui_kittest::Harness;

use egui_app::C0pl4ndApp;

/// Build a headless harness that renders the chrome (titlebar + tabs + buttons)
/// and the grid for a single shared app instance.
fn harness(app: &RefCell<C0pl4ndApp>) -> Harness<'_> {
    Harness::new_ui(move |ui| {
        // Render the titlebar/tab strip; apply the returned chrome actions so
        // a clicked `+` actually splits (mirrors the real `update` flow).
        let actions = app.borrow().titlebar_and_tabs_for_test(ui);
        {
            let mut a = app.borrow_mut();
            if actions.split_right {
                a.split_right_for_test();
            }
            if actions.split_down {
                a.split_down_for_test();
            }
            if actions.toggle_settings {
                a.toggle_settings_for_test();
            }
            if let Some(pid) = actions.focus_tab {
                a.focus_for_test(pid);
            }
        }
        ui.separator();
        app.borrow_mut().grid_ui_for_test(ui);
    })
}

#[test]
fn clicking_plus_increases_pane_count() {
    let app = RefCell::new(C0pl4ndApp::bootstrap());
    let before = app.borrow().pane_count();

    let mut h = harness(&app);
    // The split-right button is labelled "+" (recon dossier §4/§7).
    h.get_by_label("+").click();
    h.run();

    let after = app.borrow().pane_count();
    assert_eq!(
        after,
        before + 1,
        "clicking + must spawn one placeholder pane (before={before}, after={after})"
    );
}

#[test]
fn clicking_gear_toggles_settings() {
    let app = RefCell::new(C0pl4ndApp::bootstrap());
    assert!(!app.borrow().settings_is_open());

    let mut h = harness(&app);
    // The settings button is the gear glyph "⚙".
    h.get_by_label("⚙").click();
    h.run();

    assert!(
        app.borrow().settings_is_open(),
        "clicking the gear must open the settings window"
    );
}
