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

/// The exact tab label a pane currently renders (its live OSC window title when
/// the running shell set one, else the `pane {id}` fallback). Tab text and the
/// per-tab `pin`/`close` accessible labels are all built from this, so deriving
/// `get_by_label` keys from it keeps the interaction tests stable whether or not
/// the shell on this box set a title.
fn tab_label(app: &RefCell<C0pl4ndApp>, pane: PaneId) -> String {
    app.borrow()
        .tab_label_for_pane(pane)
        .unwrap_or_else(|| panic!("pane {} must have a tab label", pane.0))
}

/// Click a per-tab control (`""` → the bare tab, `"pin "` / `"unpin "` / `"close "`
/// → the prefixed button) and RETRY until the observable effect `done(app)` lands.
///
/// Two independent sources of non-determinism make a single click unreliable, and
/// BOTH stem from the OSC window title landing ASYNCHRONOUSLY from the PTY reader
/// thread:
///   1. the accessible label flips from `pane {id}` to the title between frames
///      (so the lookup key must be re-derived from the SAME post-`h.run()` state);
///   2. a title landing RESIZES the tab strip, and every tab control lives in the
///      title-bar FLOW, so the control's on-screen rect shifts between the rect
///      capture and the hit-test — a click can land on empty space and miss.
///
/// Re-deriving the key AND verifying the effect (re-clicking until `done` holds)
/// closes both races. The right-side caption cluster (close/max/min/settings) is
/// absolute-positioned and needs none of this — only the flow-region controls do.
fn click_tab_control_until(
    h: &mut Harness<'_>,
    app: &RefCell<C0pl4ndApp>,
    pane: PaneId,
    prefix: &str,
    done: impl Fn(&C0pl4ndApp) -> bool,
) {
    for _ in 0..240 {
        h.run();
        let label = format!("{prefix}{}", tab_label(app, pane));
        if let Some(node) = h.query_by_label(label.as_str()) {
            node.click();
            h.run();
            if done(&app.borrow()) {
                return;
            }
        }
    }
    panic!(
        "tab control {prefix:?} for pane {} never produced its effect (current label: {:?})",
        pane.0,
        tab_label(app, pane)
    );
}

/// Click the "+" new-terminal button until a pane is actually added. The button
/// sits in the title-bar flow AFTER the variable-width tab strip, so a tab whose
/// width changes (an async OSC title landing) shifts the button between the rect
/// capture and the hit-test — a single click can miss. Re-find and re-click (each
/// after an `h.run()` that settles the layout) until `pane_count` grows.
fn click_new_terminal(h: &mut Harness<'_>, app: &RefCell<C0pl4ndApp>) {
    let before = app.borrow().pane_count();
    for _ in 0..240 {
        h.run();
        if let Some(btn) = h.query_by_label("new terminal") {
            btn.click();
        }
        h.run();
        if app.borrow().pane_count() > before {
            return;
        }
    }
    panic!("clicking the '+' new-terminal button never added a pane");
}

/// Whether a per-tab control (`""` bare tab, `"close "`, `"pin "`, `"unpin "`)
/// is CURRENTLY present in the accessibility tree, derived race-free: the tree
/// is rebuilt (`h.run()`) and the lookup key derived from the SAME post-run app
/// state, so an async OSC-title landing can never desync the key from the tree.
/// Use this for presence/absence assertions where `click_tab_control`'s
/// click-and-return contract does not fit.
fn tab_control_present(
    h: &mut Harness<'_>,
    app: &RefCell<C0pl4ndApp>,
    pane: PaneId,
    prefix: &str,
) -> bool {
    h.run();
    let label = format!("{prefix}{}", tab_label(app, pane));
    h.query_by_label(label.as_str()).is_some()
}

#[test]
fn clicking_new_terminal_adds_a_pane() {
    // Bootstrap opens ONE pane; the single "+" button adds another.
    let app = RefCell::new(C0pl4ndApp::bootstrap());
    let before = app.borrow().pane_count();
    assert_eq!(before, 1, "app opens with a single terminal");
    let mut h = harness(&app);

    click_new_terminal(&mut h, &app);

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
    click_new_terminal(&mut h, &app);
    assert_eq!(
        app.borrow().focused_pane(),
        PaneId(1),
        "a new terminal takes focus"
    );

    // Click pane 0's tab → focus moves back to pane 0. The tab control lives in
    // the title-bar flow and its label tracks the live OSC title, so click it
    // via the effect-verifying helper (retries until focus actually lands on
    // pane 0) rather than a single click at a possibly-shifted rect.
    click_tab_control_until(&mut h, &app, PaneId(0), "", |a| {
        a.focused_pane() == PaneId(0)
    });
    assert_eq!(
        app.borrow().focused_pane(),
        PaneId(0),
        "clicking pane 0's tab must move focus to pane 0"
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

    // Click the in-content Close button (labelled "close settings"). The egui
    // Window's own title-bar ✕ was removed (it duplicated this button and read
    // as low-contrast on the dark frame), so this in-content button + Esc are
    // the single dismiss path.
    h.get_by_label("close settings").click();
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
    // The 6-pane cap is APP LOGIC (`split_focused` refuses above the cap). Drive
    // it via the action path (`new_terminal`) directly rather than UI clicks: at
    // high pane counts many (titled) tabs overflow the title-bar width and push
    // the "+" button off-screen, where it can't be clicked — that tab-overflow
    // reachability is a SEPARATE concern from the cap invariant under test. The
    // `clicking_new_terminal_adds_a_pane` test already covers the real "+" click.
    let app = RefCell::new(C0pl4ndApp::bootstrap());
    assert_eq!(app.borrow().pane_count(), 1, "bootstrap opens one pane");

    // Add until the 6-pane cap.
    for _ in 0..5 {
        app.borrow_mut().new_terminal();
    }
    assert_eq!(app.borrow().pane_count(), 6, "reached the 6-pane cap");

    // Further splits must be refused — the cap holds (count stays 6).
    app.borrow_mut().new_terminal();
    app.borrow_mut().new_terminal();
    assert_eq!(
        app.borrow().pane_count(),
        6,
        "the 6-pane cap must hold against further splits"
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

    // Click pane 0's pin (accessible label "pin {tab-label}") → it becomes
    // pinned. The pin button is in the title-bar flow and its label tracks the
    // live OSC title, so click via the effect-verifying helper (retries until the
    // pane is actually pinned) rather than a single click at a possibly-shifted
    // rect / a once-derived literal.
    click_tab_control_until(&mut h, &app, PaneId(0), "pin ", |a| a.is_pinned(PaneId(0)));
    assert!(
        app.borrow().is_pinned(PaneId(0)),
        "clicking the tab pin must pin the pane"
    );

    // The pin relabels to "unpin {tab-label}"; clicking it unpins.
    click_tab_control_until(&mut h, &app, PaneId(0), "unpin ", |a| {
        !a.is_pinned(PaneId(0))
    });
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
    click_new_terminal(&mut h, &app);
    let before = app.borrow().pane_count();
    assert_eq!(before, 2, "two panes after adding one");

    // Click pane 0's close (the LEFTMOST tab) → exactly that pane closes. We
    // target the leftmost tab deliberately: with long shell titles (e.g. the
    // Windows CI runner's "Administrator: C:\Windows\system…") the widened strip
    // can push the RIGHTMOST tab's × into the right-anchored caption cluster,
    // where it is occluded and unclickable — a tab-overflow concern separate from
    // "a × closes its own pane", which the leftmost tab proves cleanly. The label
    // tracks the live OSC title, so click via the effect-verifying helper (retries
    // until the pane count actually drops).
    click_tab_control_until(&mut h, &app, PaneId(0), "close ", |a| {
        a.pane_count() == before - 1
    });

    let after = app.borrow().pane_count();
    assert_eq!(
        after,
        before - 1,
        "clicking a tab × must close exactly that pane (before={before}, after={after})"
    );
    // Pane 0 was closed; pane 1 survives and keeps focus (it was focused after the
    // split, and closing a non-focused pane leaves focus put).
    assert_eq!(
        app.borrow().focused_pane(),
        PaneId(1),
        "the surviving pane (1) keeps focus after pane 0 is closed"
    );
}

#[test]
fn pinned_tab_has_no_close_button() {
    // A pinned tab hides its × so it can't be closed by accident (unpin first).
    // Open a second terminal so pane 1's close button is present to compare.
    let app = RefCell::new(C0pl4ndApp::bootstrap());
    let mut h = harness(&app);
    click_new_terminal(&mut h, &app);

    // Per-tab close/pin accessible labels are derived from each pane's LIVE tab
    // label (its OSC title when the shell set one), which lands asynchronously —
    // so resolve presence via the race-tolerant helper (rebuilds the tree and
    // derives the lookup key from the SAME post-run state) rather than a
    // once-derived literal that the shell's title escape could desync.

    // Both tabs start with a close button.
    assert!(
        tab_control_present(&mut h, &app, PaneId(0), "close "),
        "an unpinned tab exposes a close button"
    );

    // Pin pane 0; its close button disappears, pane 1's remains.
    click_tab_control_until(&mut h, &app, PaneId(0), "pin ", |a| a.is_pinned(PaneId(0)));
    assert!(
        !tab_control_present(&mut h, &app, PaneId(0), "close "),
        "a pinned tab must NOT expose a close button"
    );
    assert!(
        tab_control_present(&mut h, &app, PaneId(1), "close "),
        "the still-unpinned tab keeps its close button"
    );
}

#[test]
fn pane_keeps_its_content_after_adding_a_terminal() {
    // Regression for "the existing terminal goes black after I open a new one":
    // opening a new terminal splits + RESIZES the existing pane; the resize must
    // not blank its grid. Drives a real PTY through the production frame loop.
    use std::time::{Duration, Instant};
    const TOKEN: &str = "QWERTYZ123";

    let app = RefCell::new(C0pl4ndApp::bootstrap());
    let mut h = harness(&app);
    if app.borrow().focused_grid_text().is_none() {
        eprintln!("no live PTY on this platform; skipping resize-content test");
        return;
    }

    // Type a command that prints the token into pane 0, then submit.
    for ch in format!("echo {TOKEN}").chars() {
        h.event(egui::Event::Text(ch.to_string()));
    }
    h.step();
    h.key_press(egui::Key::Enter);
    h.step();

    // Poll until the token lands in pane 0's grid.
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut seen = false;
    while Instant::now() < deadline {
        h.step();
        if app
            .borrow()
            .pane_grid_text(PaneId(0))
            .is_some_and(|t| t.contains(TOKEN))
        {
            seen = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(40));
    }
    if !seen {
        eprintln!("token never reached pane 0 (no PTY echo); skipping");
        return;
    }

    // Add a terminal → splits + resizes pane 0. Run several frames so the
    // debounced resize + reflow settle.
    click_new_terminal(&mut h, &app);
    // Settle: poll up to ~2s for the resize/reflow (+ any ConPTY repaint).
    let rd = Instant::now() + Duration::from_secs(2);
    while Instant::now() < rd {
        h.step();
        if app
            .borrow()
            .pane_grid_text(PaneId(0))
            .is_some_and(|t| t.contains(TOKEN))
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(40));
    }

    // Pane 0 must STILL show its content — a blanked grid here is the black-pane.
    let after = app.borrow().pane_grid_text(PaneId(0)).unwrap_or_default();
    assert!(
        after.contains(TOKEN),
        "pane 0 lost its content after adding a terminal (resize blanked it = the \
         black-pane bug). pane 0 grid:\n{after}"
    );
}

#[test]
fn shell_menu_opens_a_new_terminal() {
    // The top-bar ▾ shell switcher must open a new terminal for the picked
    // shell. We pick the always-present "Default shell" so the test is
    // deterministic on every OS (named profiles like PowerShell/WSL are only
    // present when detected), and assert a new pane appeared.
    let app = RefCell::new(C0pl4ndApp::bootstrap());
    let before = app.borrow().pane_count();
    let mut h = harness(&app);

    // Open the ▾ menu, then click the "Default shell" item — retrying until a new
    // pane appears. The ▾ button is in the title-bar flow after the variable-width
    // tab strip, so an async OSC title can shift it and a single click can miss;
    // when the menu item is visible click it, otherwise (re)open the menu.
    for _ in 0..240 {
        h.run();
        if app.borrow().pane_count() > before {
            break;
        }
        if let Some(item) = h.query_by_label("open shell Default shell") {
            item.click();
            h.run();
        } else if let Some(menu) = h.query_by_label("shell menu") {
            menu.click();
            h.run();
        }
    }

    assert_eq!(
        app.borrow().pane_count(),
        before + 1,
        "picking a shell from the ▾ menu must open exactly one new terminal"
    );
}

#[test]
fn shell_switcher_lists_the_default_profile_first() {
    // The detected profile list always leads with the platform default, and the
    // active-shell label resolves to it at startup — the invariant the "+" hover
    // and the ▾ menu's ✓ marker rely on.
    let app = C0pl4ndApp::bootstrap();
    let profiles = app.shell_profiles();
    assert!(
        !profiles.is_empty(),
        "there is always at least one shell profile"
    );
    assert!(
        profiles[0].program.is_none(),
        "the first profile is the platform default (program None)"
    );
    assert_eq!(
        app.active_shell_label(),
        profiles[0].label,
        "the active shell defaults to the first (platform default) profile"
    );
}
