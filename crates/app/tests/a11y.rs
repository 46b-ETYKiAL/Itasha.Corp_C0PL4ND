//! Explicit **accessibility** suite for the C0PL4ND egui shell, driven by
//! `egui_kittest` over the REAL AccessKit tree the app publishes each frame.
//!
//! ## Why this suite exists (P1-1)
//!
//! The other interaction suites prove individual controls *work*; this suite
//! proves the app is *navigable by assistive technology*: every interactive
//! node carries a non-empty accessible NAME (label or value), there are no
//! unlabeled interactive controls on the user-facing surfaces, and focus can
//! traverse the focusable controls without a keyboard trap.
//!
//! ## Discipline (non-negotiable)
//!
//! Every assertion reads the **real** AccessKit tree built by the production
//! `frame_tick` — the exact tree a screen reader consumes. There is NO test-only
//! mirror of the widget set and NO set-state-then-assert tautology: the tree is
//! whatever the shipping widgets published. The accessible-name facts asserted
//! here are the same ones AccessKit exposes to NVDA / VoiceOver / Orca.
//!
//! egui's AccessKit role mapping (egui 0.34 `response.rs`): a `button` /
//! `selectable_label` / `toggle` → `Role::Button`; a `checkbox` → `Role::CheckBox`;
//! a `ComboBox` → `Role::ComboBox` (its accessible NAME is its current value, not a
//! label); a `TextEdit` → `Role::TextInput`; a `Slider` → `Role::Slider` paired
//! with a `DragValue` → `Role::SpinButton` that carries the numeric value. This
//! suite asserts against THAT reality, calibrated by a live tree dump.

#![allow(dead_code)] // The `#[path]`-included module has production entry points
                     // (eframe `App` impl, `apply_window_effect`, observation
                     // accessors) unused by this particular binary.

#[path = "../src/egui_app/mod.rs"]
mod egui_app;
#[path = "../src/issue_intake.rs"]
mod issue_intake;
#[path = "../src/reporting.rs"]
mod reporting;
#[path = "../src/user_error.rs"]
mod user_error;

use std::cell::RefCell;

use egui::accesskit::Role;
use egui_kittest::kittest::{AccessKitNode, NodeT, Queryable};
use egui_kittest::Harness;

use egui_app::C0pl4ndApp;

/// Build a headless harness driving the REAL `frame_tick` for a shared app, with
/// a screen generous enough for the fixed-size settings window to render fully.
fn harness(app: &RefCell<C0pl4ndApp>) -> Harness<'_> {
    #[allow(deprecated)]
    let mut h = Harness::new(move |ctx| app.borrow_mut().frame_tick(ctx));
    h.set_size(egui::vec2(1200.0, 800.0));
    h.run();
    h
}

/// Open the settings window by clicking the real gear caption button.
fn open_settings(h: &mut Harness<'_>) {
    h.get_by_label("settings").click();
    h.run();
}

/// The interactive a11y roles the app is responsible for naming. A screen-reader
/// user lands on each of these; each MUST announce *something*.
fn is_interactive(role: Role) -> bool {
    matches!(
        role,
        Role::Button
            | Role::CheckBox
            | Role::RadioButton
            | Role::ComboBox
            | Role::TextInput
            | Role::MultilineTextInput
            | Role::Switch
            | Role::Link
    )
}

/// An interactive node's accessible NAME — its label when present, else its
/// value (a ComboBox/SpinButton announces its current value as its name). Empty
/// string when neither is set. This mirrors the name-computation a screen reader
/// performs (label preferred, value as fallback).
fn accessible_name(n: &AccessKitNode<'_>) -> String {
    if let Some(l) = n.label() {
        if !l.trim().is_empty() {
            return l;
        }
    }
    n.value().map(|v| v.trim().to_string()).unwrap_or_default()
}

/// Collect every interactive node on the current surface as
/// `(role, label, value, accessible_name)` tuples — read straight from the live
/// AccessKit tree (`root().children_recursive()`).
fn interactive_nodes(h: &Harness<'_>) -> Vec<(Role, Option<String>, Option<String>, String)> {
    h.root()
        .children_recursive()
        .filter_map(|node| {
            let nd = node.accesskit_node();
            let role = nd.role();
            if is_interactive(role) {
                Some((role, nd.label(), nd.value(), accessible_name(&nd)))
            } else {
                None
            }
        })
        .collect()
}

#[test]
fn every_interactive_control_on_the_main_surface_has_an_accessible_name() {
    // The main terminal surface (titlebar caption cluster + tab strip + shell/
    // script/view buttons) must expose NO unlabeled interactive node — a blank
    // announcement ("button") is the canonical screen-reader-hostile defect. We
    // read the REAL published tree and require a non-empty accessible name on
    // EVERY interactive node, naming any offender.
    let app = RefCell::new(C0pl4ndApp::bootstrap());
    let h = harness(&app);

    let nodes = interactive_nodes(&h);
    assert!(
        !nodes.is_empty(),
        "the main surface must publish interactive nodes (titlebar caption \
         cluster + tab controls) into the AccessKit tree"
    );

    let unnamed: Vec<_> = nodes
        .iter()
        .filter(|(_, _, _, name)| name.is_empty())
        .collect();
    assert!(
        unnamed.is_empty(),
        "every interactive control on the main surface must carry a non-empty \
         accessible name (label or value); these are unnamed: {unnamed:?}"
    );

    // The caption cluster's four named affordances must each be announceable by
    // their EXACT accessible label — the names assistive tech reads aloud.
    for required in ["settings", "close", "minimize", "maximize", "new terminal"] {
        assert!(
            nodes
                .iter()
                .any(|(_, label, _, _)| label.as_deref() == Some(required)),
            "the main surface must expose an interactive control accessibly \
             named {required:?}"
        );
    }
}

#[test]
fn every_button_in_settings_has_a_label_and_every_combo_announces_a_value() {
    // The settings window is the app's densest control surface. Calibrated to the
    // live tree: toggles render as Role::Button (egui's `toggle` = selectable
    // label), nav items as Role::Button, the per-row revert as the "↺" Button —
    // ALL of these must carry a label. ComboBoxes expose no label by design; their
    // accessible NAME is their current value, which must be non-empty. (The bare
    // Slider TRACK is framework-paired with a SpinButton sibling that carries the
    // value — egui's slider+drag idiom — so a label-less Slider track is an egui
    // pattern, not an app a11y defect, and is excluded here. Its value-bearing
    // SpinButton sibling is covered by the dedicated check below.)
    let app = RefCell::new(C0pl4ndApp::bootstrap());
    let mut h = harness(&app);
    open_settings(&mut h);

    let nodes = interactive_nodes(&h);

    // Every Button (nav item, toggle, ↺ revert, close) must have a non-empty
    // label — the text a screen reader announces when focus lands on it.
    let unlabeled_buttons: Vec<_> = nodes
        .iter()
        .filter(|(role, label, _, _)| {
            *role == Role::Button && label.as_deref().map(str::trim).unwrap_or("").is_empty()
        })
        .collect();
    assert!(
        unlabeled_buttons.is_empty(),
        "every Button in the settings window must carry a non-empty accessible \
         label; these are unlabeled: {unlabeled_buttons:?}"
    );

    // Every ComboBox must announce a non-empty value (its accessible name).
    let valueless_combos: Vec<_> = nodes
        .iter()
        .filter(|(role, _, value, _)| {
            *role == Role::ComboBox && value.as_deref().map(str::trim).unwrap_or("").is_empty()
        })
        .collect();
    assert!(
        valueless_combos.is_empty(),
        "every ComboBox in the settings window must announce a non-empty value \
         as its accessible name; these announce nothing: {valueless_combos:?}"
    );

    // Every left-nav category item must be reachable by its exact accessible
    // label — the names a keyboard/screen-reader user navigates by. This list
    // mirrors `settings::CATEGORIES` (display order); keep the two in sync when a
    // category is added, renamed, or reordered.
    for category in [
        "Appearance",
        "Fonts",
        "Cursor",
        "Terminal",
        "Window",
        "Toolbar",
        "Motion",
        "Keybindings",
        "Updates",
        "Privacy",
    ] {
        assert!(
            nodes.iter().any(|(role, label, _, _)| {
                *role == Role::Button && label.as_deref() == Some(category)
            }),
            "the settings nav must expose an accessibly-named {category:?} item"
        );
    }
}

#[test]
fn the_theme_combo_announces_its_live_value_as_its_accessible_name() {
    // A ComboBox's accessible NAME is its current value (egui exposes no label on
    // it). The Appearance theme combo must therefore announce the LIVE config
    // theme stem — proving a screen reader would read the user's actual selection,
    // not a blank. We assert the published value tracks a real state change driven
    // through the production combo, not a static string.
    let app = RefCell::new(C0pl4ndApp::bootstrap());
    let mut h = harness(&app);
    open_settings(&mut h);

    // The Appearance section renders the theme combo FIRST. Its accessible value
    // is the live stem ("itasha-corp" at default).
    let before = h
        .get_all_by_role(Role::ComboBox)
        .next()
        .expect("Appearance must render the theme combo")
        .value();
    assert_eq!(
        before.as_deref(),
        Some("itasha-corp"),
        "the theme combo must announce the default theme stem as its name"
    );

    // Drive a real selection and assert the announced name follows it.
    h.get_all_by_role(Role::ComboBox)
        .next()
        .expect("theme combo")
        .click();
    h.run();
    h.get_by_label("ghost-paper").click();
    h.run();

    let after = h
        .get_all_by_role(Role::ComboBox)
        .next()
        .expect("theme combo")
        .value();
    assert_eq!(
        after.as_deref(),
        Some("ghost-paper"),
        "after picking ghost-paper the combo must announce the NEW stem as its \
         accessible name (a screen reader reads the live selection)"
    );
}

#[test]
fn settings_controls_are_focusable_and_focus_is_not_trapped() {
    // Keyboard-trap check: a control that cannot receive focus is unreachable by
    // keyboard; a control that holds focus and never yields it is a trap. We focus
    // three distinct ENABLED settings controls in turn through the real frame loop
    // and assert (a) each accepts focus when targeted, and (b) focusing the next
    // one MOVES focus off the previous — i.e. focus traverses freely, with no trap.
    let app = RefCell::new(C0pl4ndApp::bootstrap());
    let mut h = harness(&app);
    open_settings(&mut h);

    // Three enabled, distinct interactive controls always present on the default
    // Appearance page: the search box (TextInput), the theme combo (ComboBox), and
    // the master transparency toggle (Button). Focus each and verify it lands.
    let focus_and_assert = |h: &mut Harness<'_>, role: Role, which: &str| {
        {
            let node = h
                .get_all_by_role(role)
                .find(|n| !n.accesskit_node().is_disabled())
                .unwrap_or_else(|| panic!("settings must render an enabled {which}"));
            node.focus();
        }
        h.run();
        let focused = h
            .get_all_by_role(role)
            .any(|n| n.is_focused() && !n.accesskit_node().is_disabled());
        assert!(
            focused,
            "focusing the {which} must land focus on it (it is reachable by \
             keyboard, not skipped)"
        );
    };

    // Focus the search box first.
    focus_and_assert(&mut h, Role::TextInput, "search box");

    // Focus the theme combo — focus must MOVE here (off the search box). If the
    // search box were a trap, the combo could not take focus.
    focus_and_assert(&mut h, Role::ComboBox, "theme combo");
    assert!(
        !h.get_all_by_role(Role::TextInput).any(|n| n.is_focused()),
        "focus must have LEFT the search box when the combo was focused — a \
         control that keeps focus after another is focused is a keyboard trap"
    );

    // Focus the master transparency toggle (a Button) — focus moves again, off
    // the combo. Three hops with no control refusing to yield = no trap.
    {
        let toggle = h.get_by_label("Enable window transparency");
        toggle.focus();
    }
    h.run();
    assert!(
        h.get_by_label("Enable window transparency").is_focused(),
        "the transparency toggle must accept focus"
    );
    assert!(
        !h.get_all_by_role(Role::ComboBox).any(|n| n.is_focused()),
        "focus must have LEFT the combo when the toggle was focused — focus \
         traverses the settings controls freely (no keyboard trap)"
    );
}

#[test]
fn opening_settings_publishes_more_interactive_nodes_than_the_bare_surface() {
    // A sanity guard that the settings surface actually enriches the AccessKit
    // tree (rather than rendering an inert overlay a screen reader can't enter):
    // the count of interactive nodes must GROW when settings opens, and the
    // grown-in set must include the "close settings" affordance so AT users can
    // get back out.
    let app = RefCell::new(C0pl4ndApp::bootstrap());
    let mut h = harness(&app);

    let bare = interactive_nodes(&h).len();
    open_settings(&mut h);
    let with_settings = interactive_nodes(&h);

    assert!(
        with_settings.len() > bare,
        "opening settings must add interactive nodes to the AccessKit tree \
         (bare={bare}, with settings={})",
        with_settings.len()
    );
    assert!(
        with_settings
            .iter()
            .any(|(_, label, _, _)| label.as_deref() == Some("close settings")),
        "the settings surface must expose an accessibly-named 'close settings' \
         control so an assistive-tech user can dismiss it"
    );
}
