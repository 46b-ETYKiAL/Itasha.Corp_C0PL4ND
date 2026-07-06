//! In-app settings window for the C0PL4ND egui chrome (Milestone 2).
//!
//! Replaces the Milestone-1 read-only placeholder with a polished, grouped,
//! well-spaced window that maps the real [`c0pl4nd_core::Config`] fields into
//! logical sections — matching the sibling editor SCR1B3's settings layout for
//! same-company visual cohesion.
//!
//! Layout: a fixed-size window with a **left category nav** + a **searchable,
//! internally-scrolling content pane**, so every setting is reachable at the
//! default window size without ever resizing (the
//! `ScrollArea::auto_shrink([false, false])` idiom is load-bearing here). Each
//! section is a heading + a two-column [`egui::Grid`] of label/control rows, so
//! every control lines up vertically — the system-settings look.
//!
//! Visual cohesion with SCR1B3: identical two-pane structure, GNOME-style
//! spacing rhythm (8 px between related rows, 14 px between sections), brand
//! accents inherited from [`super::theme::itasha_corp_visuals`] (Operator Violet
//! `#7700FF` for press / structure, `.Corp` green `#00FF90` for the live /
//! selected accent), and the per-setting ↺ revert affordance.
//!
//! Kept as a free function so it never fights the `C0pl4ndApp` borrow — the host
//! ([`super::C0pl4ndApp::settings_window`]) calls [`show`] with `&mut config` and
//! reacts to the returned [`Outcome`] (persist + re-apply theme).

use std::sync::{Arc, Mutex};

use eframe::egui;

use c0pl4nd_core::config::{CursorStyle, GpuPreference, GraphicsBackend, UpdateMode, WindowMode};
use c0pl4nd_core::Config;

use super::theme::ChromeColors;

// The in-app self-updater backend (download + SHA-256/minisign verify + atomic
// self-replace) and its egui state machine. Declared here via `#[path]` so the
// Updates settings page is fully self-contained: it resolves identically in the
// shipping `c0pl4nd` binary AND in the `egui_kittest` integration test binaries
// (which `#[path]`-include `egui_app/mod.rs` but not the crate-root `update`
// module). The shipping binary also declares a crate-root `update` for the CLI
// (`c0pl4nd update`) + launch-check; this second view is private to `settings`
// and never shares a type across that boundary, so the two coexist cleanly.
#[path = "../update_engine/mod.rs"]
mod update_engine;

use update_engine::updater::{LaunchKind, UpdateState, Updater};

/// Left-nav categories, in display order. Each maps to a section rendered by
/// [`render_sections`].
const CATEGORIES: &[&str] = &[
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
];

/// Cross-category search labels for the **Appearance** section. Kept as a named
/// const so the CRT/scanline/chromatic terms that MOVED to the Motion section are
/// provably absent here (a stale label would resurrect a phantom empty Appearance
/// section and hide the moved control from a full-name search).
const APPEARANCE_SEARCH_LABELS: &[&str] = &[
    "theme",
    "transparency",
    "mode",
    "opacity",
    "glass",
    "mica",
    "vibrancy",
    "tint",
    "acrylic",
    "ui scale",
    "zoom",
    "accessibility",
];

/// Cross-category search labels for the **Motion** section. Each entry is the
/// canonical label of exactly one Motion row (see the `row_visible(q, …)` calls),
/// so any query that reveals the section via a label also reveals its row — no
/// phantom empty section, and every moved control (CRT scanlines, scanline
/// darkness, chromatic aberration) is reachable by its full name. Shared with the
/// `motion_section_is_cross_category_searchable` test so the invariant is checked
/// against the PRODUCTION labels, not an inline copy.
const MOTION_SEARCH_LABELS: &[&str] = &[
    "enable animations",
    "animation speed",
    "crt scanlines",
    "scanline darkness",
    "chromatic aberration",
    "wired mesh background",
    "mesh density",
    "vhs tracking",
    "screen flicker",
    "flicker strength",
    "cursor trail",
    "cursor trail intensity",
    "boot glitch",
];

/// The release channels the Updates section offers. Mirrors the channels the
/// `c0pl4nd update` checker understands; a free choice list, not invented.
const UPDATE_CHANNELS: &[&str] = &["stable", "beta", "nightly"];

// ---- Fixed width budget (the runaway-width root-cause fix, #26) ----
// The settings window is content-sized by egui. WITHOUT a hard cap, any child
// that reports `available_width()` / `f32::INFINITY` as its desired size (a
// filling slider, the search box) makes the window grow each frame until it hits
// the screen edge — leaving the controls spread out with a wide blank gutter on
// the right (the reported "much too wide + blank space" bug). The fix is a FIXED
// content-width budget: clamp the content `Ui` to these consts (NOT to
// `available_width()`, which is the already-inflated post-balloon width and so
// clamps nothing). No page can then demand more than the budget, so the window
// holds a snug, stable width identical on every page. Sized to fit the 170px
// left nav + a comfortable control pane (label + control + ↺ columns).

/// Snug default window WIDTH (px), before the screen-fit clamp. Matches the
/// sibling SCR1B3 editor's settings width for cross-app cohesion.
const SETTINGS_DEFAULT_W: f32 = 760.0;
/// Default window HEIGHT (px), before the screen-fit clamp.
const SETTINGS_DEFAULT_H: f32 = 560.0;
/// Fixed width of the left category nav column.
const SETTINGS_NAV_W: f32 = 170.0;
/// Minimum window WIDTH (px) the user may shrink the resizable settings window
/// to. Below this the nav + content pane collide, so the resize handle stops.
const SETTINGS_MIN_W: f32 = 560.0;
/// Minimum window HEIGHT (px) for the resizable settings window.
const SETTINGS_MIN_H: f32 = 400.0;

/// The terminal color themes that ship in `assets/themes/` (file stems). The
/// theme combo offers these built-ins; a free text field below it accepts any
/// user theme name (a TOML under the config dir's themes folder). This list is
/// the ground truth of what actually ships — it is NOT invented; mirror it if a
/// theme file is added or removed.
const BUILTIN_THEMES: &[&str] = &[
    "itasha-corp",
    "itasha-void",
    "itasha-void-high-contrast",
    "ghost-paper",
    "wired-noir",
    "wired-colorblind",
    // Ported from the SCR1B3 editor (calm-canon line).
    "phosphor-amber",
    "lain-mauve",
    "a11y-high-contrast",
    // itasha-neon family (brand-signature line).
    "itasha-neon",
    "itasha-neon-pastel",
    "itasha-neon-soft",
    "itasha-neon-night",
    "itasha-neon-dawn",
    "itasha-neon-aurora",
    // Heritage-alt influence palettes.
    "geocities-bbs",
    "lain-wired",
    "kusanagi-dive",
    "akira-redshift",
    "atompunk-sodium",
    "terminal-lock",
    "mecha-armour",
    "shutoko-night",
];

/// What [`show`] reports back to the host after a frame.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Outcome {
    /// Any config field changed this frame — the host should persist the config
    /// to disk and re-apply the egui Visuals.
    pub changed: bool,
    /// The `theme` field (terminal color theme stem) changed this frame — the
    /// host should additionally reload the terminal grid's color theme so the
    /// change shows in the live panes, not only the chrome.
    pub theme_changed: bool,
    /// The user clicked "Clear command history" in the Privacy section — the host
    /// should clear (and zeroize) the in-memory command history.
    pub clear_history: bool,
    /// The user toggled the Incognito switch this frame, to the contained value.
    /// `None` when unchanged. Runtime-only (not a config field).
    pub set_incognito: Option<bool>,
    /// The settings window's on-screen rect this frame (`None` if it did not
    /// render). The host excludes this rect from the whole-window motion overlays
    /// so a live Motion-setting preview shows on the terminal without the mesh /
    /// flicker washing over the settings panel itself.
    pub window_rect: Option<egui::Rect>,
}

/// Whether a category section should render: its own tab when not searching, or
/// any-label-matches when a search query is active (cross-category results).
/// Pure — copied in spirit from SCR1B3's `section_visible`.
fn section_visible(selected: &str, q: &str, category: &str, labels: &[&str]) -> bool {
    if q.is_empty() {
        selected == category
    } else {
        category.to_lowercase().contains(q) || labels.iter().any(|l| l.to_lowercase().contains(q))
    }
}

/// Whether an individual row should render given the active search query.
fn row_visible(q: &str, label: &str) -> bool {
    q.is_empty() || label.to_lowercase().contains(q)
}

/// The label to show in the Font Family combo for the stored config value. A
/// stored value that resolves to egui's built-in monospace (empty, the synthetic
/// label, or the generic "monospace") is shown as the built-in label so the combo
/// reads cleanly; any other (installed-family) value is shown verbatim.
fn family_display(family: &str) -> String {
    if super::fonts::is_builtin_family(family) {
        super::fonts::BUILTIN_MONOSPACE_LABEL.to_string()
    } else {
        family.to_string()
    }
}

/// The value to STORE in `config.font.family` for a chosen combo entry. The
/// built-in label is stored verbatim (it round-trips through `family_display` and
/// is recognised by `fonts::is_builtin_family`, so no custom face is loaded); an
/// installed family name is stored as-is.
fn family_value(choice: &str) -> String {
    choice.to_string()
}

/// One Fallback-slot ComboBox over the installed monospace families plus the
/// "(none)" sentinel. Mutates `slot` in place and returns whether it changed.
/// Factored out so the two slots share one widget definition.
fn fallback_combo(ui: &mut egui::Ui, id_salt: &str, choices: &[String], slot: &mut String) -> bool {
    let mut changed = false;
    egui::ComboBox::from_id_salt(id_salt)
        .selected_text(slot.clone())
        .width(220.0)
        .show_ui(ui, |ui| {
            // "(none)" first so an empty slot is the obvious default choice.
            changed |= ui
                .selectable_value(
                    slot,
                    super::fonts::NONE_LABEL.to_string(),
                    super::fonts::NONE_LABEL,
                )
                .changed();
            for fam in choices {
                // The built-in label is not a meaningful FALLBACK (it is already
                // the ultimate fallback), so offer only real installed families.
                if fam == super::fonts::BUILTIN_MONOSPACE_LABEL {
                    continue;
                }
                changed |= ui.selectable_value(slot, fam.clone(), fam).changed();
            }
        });
    changed
}

/// A per-setting "restore default" affordance. Renders a small ↺ button that is
/// enabled only when `cur != def`; clicking it resets the field and returns
/// `true` so the caller marks settings dirty. Placed as the last cell of a Grid
/// row, it gives every scalar setting a one-click revert without a global "reset
/// everything" sledgehammer (the modern Fluent `SettingsCard` ↺ pattern).
/// Mirrors SCR1B3's `reset_to_default` verbatim — pure and app-agnostic.
fn reset_to_default<T: PartialEq + Clone>(ui: &mut egui::Ui, cur: &mut T, def: &T) -> bool {
    let differs = *cur != *def;
    let resp = ui
        .add_enabled(
            differs,
            egui::Button::new(egui::RichText::new("↺").small()).frame(false),
        )
        .on_hover_text(if differs {
            "Restore default"
        } else {
            "Already default"
        });
    if differs && resp.clicked() {
        *cur = def.clone();
        return true;
    }
    false
}

/// Equal-weight 3-way consent selector for a W1TN3SS reporting stream
/// (`Off` / `Ask each time` / `Always`). The three radios carry EQUAL visual
/// weight — there is no pre-ticked default-on or dark-pattern asymmetry; `Off`
/// is first and is the default. Returns `true` when the user changed the mode.
/// Pure UI over the SDK's `ReportingMode`; the host persists the config on
/// change like any other setting.
fn reporting_mode_selector(
    ui: &mut egui::Ui,
    id_salt: &str,
    mode: &mut itasha_report_core::config::ReportingMode,
) -> bool {
    use itasha_report_core::config::ReportingMode;
    let mut changed = false;
    ui.push_id(id_salt, |ui| {
        ui.horizontal(|ui| {
            // Off FIRST (the privacy-default, selected by default) — equal weight.
            changed |= ui
                .radio_value(mode, ReportingMode::Off, "Off")
                .on_hover_text("Never report for this stream (the default).")
                .changed();
            changed |= ui
                .radio_value(mode, ReportingMode::AskEachTime, "Ask each time")
                .on_hover_text("Show each report to you — editable — and ask before sending.")
                .changed();
            changed |= ui
                .radio_value(mode, ReportingMode::Always, "Always send")
                .on_hover_text(
                    "Send reports for this stream without asking each time. You can \
                     turn this off at any time.",
                )
                .changed();
        });
    });
    changed
}

/// A dim helper label under a heading (SCR1B3's `weak().small()` idiom). WRAPS
/// to the available width — a long single-line help string (e.g. the Updates
/// page's) otherwise sets the content's min width and forced the whole settings
/// window WIDER on that page than the others (the reported per-page width drift).
fn help(ui: &mut egui::Ui, text: &str) {
    ui.add(egui::Label::new(egui::RichText::new(text).weak().small()).wrap());
    ui.add_space(2.0);
}

/// The eframe app-id used for native window-state + (formerly) egui-memory
/// persistence. Must match the `with_app_id(..)` in `egui_main.rs`.
const EFRAME_APP_ID: &str = "com.itashacorp.c0pl4nd";

/// Absolute path to eframe's `app.ron` persisted-state file, resolved the SAME
/// way eframe itself resolves it: [`eframe::storage_dir`] for our app-id, then
/// the `app.ron` leaf inside it. `None` only when no platform storage dir is
/// available (the same condition under which eframe would not persist either).
///
/// On Windows this is `%APPDATA%\com.itashacorp.c0pl4nd\data\app.ron`; on Linux
/// `~/.local/share/com.itashacorp.c0pl4nd/app.ron` — the dir returned by
/// `storage_dir` is canonical, so we never hard-code the platform path here.
fn app_ron_path() -> Option<std::path::PathBuf> {
    eframe::storage_dir(EFRAME_APP_ID).map(|dir| dir.join("app.ron"))
}

/// Delete eframe's persisted `app.ron` (privacy F1 user control). Returns a
/// short, user-facing status string for the settings page. A missing file is a
/// success ("nothing to clear"); a real delete error is surfaced without leaking
/// the internal path (Tauri-IPC-style error sanitisation discipline).
fn clear_saved_ui_state() -> String {
    let Some(path) = app_ron_path() else {
        return "No saved UI state on this platform.".to_string();
    };
    match std::fs::remove_file(&path) {
        Ok(()) => "Saved window/UI state cleared.".to_string(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            "No saved UI state to clear.".to_string()
        }
        Err(e) => {
            tracing::warn!(target: "c0pl4nd::settings", detail = %e, "clear saved UI state failed");
            "Couldn't clear the saved window/UI state. Make sure C0PL4ND has \
             permission to its settings folder and try again."
                .to_string()
        }
    }
}

/// F5-2: open `path` in the OS file manager (reveal-in-folder / open-with-
/// default-app). Uses the platform opener via `std::process` — no `unsafe`, no
/// network. Best-effort: a spawn failure is ignored (worst case the user
/// navigates to the shown path manually). Consistent with a terminal emulator
/// that already spawns child processes.
fn reveal_in_file_manager(path: &std::path::Path) {
    #[cfg(target_os = "windows")]
    let _ = std::process::Command::new("explorer").arg(path).spawn();
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg(path).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let _ = std::process::Command::new("xdg-open").arg(path).spawn();
}

/// Render the settings window. `open` is toggled false when the user closes it
/// (via the egui Window's built-in ✕). Returns the [`Outcome`] for this frame so
/// the host can persist + re-apply the theme.
pub fn show(
    ctx: &egui::Context,
    config: &mut Config,
    open: &mut bool,
    colors: ChromeColors,
    incognito: bool,
    place_now: bool,
) -> Outcome {
    let mut changed = false;
    let mut keep_open = *open;
    // Privacy-section actions accumulated this frame (reported via Outcome).
    let mut clear_history = false;
    let mut set_incognito: Option<bool> = None;

    // Selected category + search query survive across frames via ctx temp-data
    // (the SCR1B3 pattern) so the window remembers where the user was.
    let cat_id = egui::Id::new("c0pl4nd_settings_cat");
    let q_id = egui::Id::new("c0pl4nd_settings_query");
    let mut category = ctx
        .data_mut(|d| d.get_temp::<String>(cat_id))
        .unwrap_or_else(|| "Appearance".to_string());
    let mut query = ctx
        .data_mut(|d| d.get_temp::<String>(q_id))
        .unwrap_or_default();

    // Snapshot the theme stem so we can tell the host whether it changed (the
    // host reloads the terminal color theme only when this differs).
    let theme_before = config.theme.clone();

    // The in-app self-updater state machine. Held across frames in `ctx`
    // temp-data as an `Arc<Mutex<Updater>>` (Arc is Clone, which egui temp-data
    // requires; the `Updater` itself holds a non-Clone mpsc Receiver). This
    // keeps `show` a free function with the host's fixed signature — the host
    // (mod.rs) never has to know the updater exists. We poll it every frame so
    // background-worker messages advance the state machine even while the
    // Updates page is not the visible tab.
    let updater = get_updater(ctx);
    if let Ok(mut u) = updater.lock() {
        u.poll(ctx);
    }

    // Center the window the first time it opens via a one-time default position.
    // `.anchor()` is deliberately NOT used: an anchored egui window is re-pinned
    // to its anchor every frame and is therefore IMMOVABLE — that was the root
    // cause of the "settings can't be dragged" report. `.default_pos` places it
    // once, then the title bar drags freely.
    //
    // Initial window size: the user's last resized size (persisted in the config
    // TOML), else the built-in default. Each dimension is clamped to a sane floor
    // (`SETTINGS_MIN_*`) and to the live screen (minus a 24px margin) so a
    // malformed persisted value or a tiny display can never spawn an unusable or
    // off-screen window. A non-finite persisted value is ignored. The window is
    // now user-resizable (see the builder below); a filling child no longer
    // balloons it because the content `ScrollArea` uses `auto_shrink([false,
    // false])` — it fills the width it is GIVEN rather than demanding its own.
    // Full window rect, read robustly: `viewport_rect()` is the whole inner
    // window, but on some frames it can read empty — fall back to the input-layer
    // screen rect, which is populated directly from the windowing backend. This is
    // the size/position reference for the window below.
    let screen = {
        let vr = ctx.viewport_rect();
        if vr.width() > 1.0 && vr.height() > 1.0 {
            vr
        } else {
            // Paranoia fallback for a not-yet-sized viewport: a sane default rect
            // from the origin. The clamps below keep the window on-screen once the
            // real viewport size arrives.
            egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1100.0, 720.0))
        }
    };
    let screen_max_w = (screen.width() - 24.0).max(SETTINGS_MIN_W);
    let screen_max_h = (screen.height() - 24.0).max(SETTINGS_MIN_H);
    let want_w = config
        .settings_win_w
        .filter(|v| v.is_finite())
        .unwrap_or(SETTINGS_DEFAULT_W);
    let want_h = config
        .settings_win_h
        .filter(|v| v.is_finite())
        .unwrap_or(SETTINGS_DEFAULT_H);
    let def_w = want_w.clamp(SETTINGS_MIN_W, screen_max_w);
    let def_h = want_h.clamp(SETTINGS_MIN_H, screen_max_h);
    let default_size = egui::vec2(def_w, def_h);

    // Target top-left for a forced placement: the saved position (a prior move) if
    // both coordinates are present + finite, else CENTERED over the window. Both
    // are clamped to the live screen so a stale off-screen value can never hide the
    // window. This is applied via `current_pos` ONLY on the open frame (`place_now`)
    // — deterministic, so it never depends on `default_pos` timing.
    let centered = screen.center() - default_size * 0.5;
    let saved_pos = match (config.settings_win_x, config.settings_win_y) {
        (Some(x), Some(y)) if x.is_finite() && y.is_finite() => Some(egui::pos2(x, y)),
        _ => None,
    };
    let target_pos = saved_pos.unwrap_or(centered);
    // Keep it on-screen: clamp so the whole window stays within the viewport.
    let max_x = (screen.right() - def_w).max(screen.left());
    let max_y = (screen.bottom() - def_h).max(screen.top());
    let target_pos = egui::pos2(
        target_pos.x.clamp(screen.left(), max_x),
        target_pos.y.clamp(screen.top(), max_y),
    );

    // Esc dismisses settings (in addition to the title-bar ✕ and the in-content
    // Close button) — the conventional overlay-dismiss key.
    if keep_open && ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        keep_open = false;
    }

    // Set by the in-content Close button below; applied after the frame so the
    // mutation does not fight the `&mut keep_open` borrow the Window holds.
    let mut close_requested = false;

    // NOTE: deliberately NO `.open(&mut keep_open)` — that adds egui's own
    // title-bar ✕, which (a) duplicates the clear in-content Close button below
    // and (b) reads as low-contrast on the dark custom frame. The in-content
    // button + Esc are the single, obvious dismiss path (the "two close buttons"
    // report). Closing flows through `keep_open` → `*open` exactly as before.
    let mut win = egui::Window::new("settings");
    // Force the position on the open frame ONLY (`place_now`): `current_pos` pins
    // the window this frame, egui then remembers it, and on later frames we stop
    // pinning so the user can freely drag it. This is what makes the first open
    // land centered (or at the saved spot) deterministically.
    if place_now {
        win = win.current_pos(target_pos);
    }
    let win_resp = win
        // No egui title bar: it rendered a SECOND, redundant top bar (a centered
        // lowercase "settings") above the in-content header that carries the
        // "Settings" heading + the ✕ close button. The in-content header is the
        // single titlebar; dragging still works via egui's window-frame drag.
        .title_bar(false)
        .collapsible(false)
        // USER-RESIZABLE with a persisted size. `default_size` seeds the window
        // the first time it appears (from the persisted-or-default size computed
        // above); thereafter egui remembers the user's drag within the session and
        // the write-back below persists it across restarts. `min_size` stops the
        // user from shrinking it until the nav + content pane collide. The old
        // ballooning failure mode (a filling child demanding near-infinite width)
        // is prevented at the source: the content `ScrollArea` uses
        // `auto_shrink([false, false])`, so it fills the width the window gives it
        // rather than reporting its own desired width back up to the Resize widget.
        .resizable(true)
        .default_size(default_size)
        .min_size(egui::vec2(SETTINGS_MIN_W, SETTINGS_MIN_H))
        // Constrain to the WHOLE screen (keep the window on-screen), NOT to a
        // default_size box re-centered every frame. The latter re-pinned the
        // window to the live screen centre each frame, so maximizing the MAIN
        // window (which moves `screen.center()`) yanked the settings Area to the
        // new centre while its content had been laid out at the old position —
        // the reported "contents get moved and cut off when maximized". Bounding
        // to the full screen keeps it visible without forcing a re-centre.
        .constrain_to(screen)
        .movable(true)
        .frame(egui::Frame::window(&ctx.global_style()).fill(colors.panel))
        .show(ctx, |ui| {
            // ---- Width discipline ----
            // The content fills the width the (now user-resizable) window gives
            // it. The old ballooning failure mode — a filling child reporting
            // `available_width()`/`INFINITY` as its desired size and driving the
            // window ever wider — is prevented at the source by the content
            // `ScrollArea`'s `auto_shrink([false, false])` below: it consumes the
            // width it is given rather than demanding its own. So `available_width`
            // here is a real, bounded number (the window's inner width), and using
            // it lets a grown window put the extra space to work instead of
            // stranding it in a right-hand gutter.
            let content_w = ui.available_width();
            ui.set_max_width(content_w);
            // In-content header: title + an unmissable Close ✕. The egui
            // title-bar ✕ can read as low-contrast against the dark custom
            // frame (the "can't close it" report), so this is a clear,
            // always-visible dismiss. Fixed-height — it does not eat the scroll
            // area's fill below.
            ui.horizontal(|ui| {
                // The heading uses the bright theme FOREGROUND, not the accent:
                // the accent (theme selection colour) read as low-contrast purple
                // against the dark panel fill (the reported "hard to read" bug).
                // fg is readable in both light and dark themes.
                ui.heading(egui::RichText::new("Settings").color(colors.fg));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let close = ui
                        .button(egui::RichText::new(egui_phosphor::thin::X).size(16.0))
                        .on_hover_text("Close settings (Esc)");
                    close.widget_info(|| {
                        egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "close settings")
                    });
                    if close.clicked() {
                        close_requested = true;
                    }
                });
            });
            ui.separator();

            ui.horizontal_top(|ui| {
                // ---- Left category nav ----
                ui.vertical(|ui| {
                    ui.set_width(SETTINGS_NAV_W);
                    ui.add_space(4.0);
                    for cat in CATEGORIES {
                        ui.selectable_value(&mut category, (*cat).to_string(), *cat);
                        ui.add_space(2.0);
                    }
                });
                ui.separator();

                // ---- Searchable, internally-scrolling content pane ----
                ui.vertical(|ui| {
                    // The content pane fills the width left after the nav +
                    // separator. The grids below size to THIS width (not to their
                    // own desired width), and the search box gets a finite width to
                    // fill. Ballooning is prevented at the source by the content
                    // `ScrollArea`'s `auto_shrink([false, false])`, so raw
                    // `available_width()` is a real bounded number here.
                    let pane_w = ui.available_width();
                    ui.set_max_width(pane_w);
                    ui.horizontal(|ui| {
                        ui.label(egui_phosphor::thin::MAGNIFYING_GLASS);
                        // A bounded width (was `f32::INFINITY`): leave room for the
                        // magnifier glyph + the clear ✕ so the box fills the pane
                        // WITHOUT demanding more than the pane is wide. The 56px
                        // reserve covers the glyph + the optional clear button +
                        // inter-item spacing; `max(0.0)` guards a tiny pane.
                        let search_w = (pane_w - 56.0).max(0.0);
                        ui.add(
                            egui::TextEdit::singleline(&mut query)
                                .hint_text("search settings")
                                .desired_width(search_w),
                        );
                        if !query.is_empty() && ui.button("✕").clicked() {
                            query.clear();
                        }
                    });
                    ui.separator();

                    let q = query.trim().to_lowercase();
                    let sel = category.as_str();
                    egui::ScrollArea::vertical()
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            changed |= render_sections(
                                ui,
                                config,
                                &updater,
                                sel,
                                &q,
                                incognito,
                                &mut clear_history,
                                &mut set_incognito,
                            );
                        });
                });
            });
        });

    if close_requested {
        keep_open = false;
    }

    // Persist a user resize/move so the window reopens at the size + position they
    // left it. Only write when the pointer is UP (the drag has settled). The SIZE
    // dead-band is deliberately larger than the window frame margin so the
    // outer-rect/inner-size difference never re-arms a write on a plain reopen; the
    // POSITION dead-band is small (position is stable when not dragged). Writing
    // either updates all four fields so size + position stay coherent.
    if !place_now {
        if let Some(win_resp) = &win_resp {
            let rect = win_resp.response.rect;
            let sz = rect.size();
            let pos = rect.min;
            let pointer_up = ctx.input(|i| !i.pointer.any_down());
            let base_w = config.settings_win_w.unwrap_or(SETTINGS_DEFAULT_W);
            let base_h = config.settings_win_h.unwrap_or(SETTINGS_DEFAULT_H);
            let base_x = config.settings_win_x.unwrap_or(pos.x);
            let base_y = config.settings_win_y.unwrap_or(pos.y);
            const SIZE_DEAD_BAND: f32 = 12.0;
            const POS_DEAD_BAND: f32 = 4.0;
            let all_finite =
                sz.x.is_finite() && sz.y.is_finite() && pos.x.is_finite() && pos.y.is_finite();
            let size_moved =
                (sz.x - base_w).abs() > SIZE_DEAD_BAND || (sz.y - base_h).abs() > SIZE_DEAD_BAND;
            let pos_moved =
                (pos.x - base_x).abs() > POS_DEAD_BAND || (pos.y - base_y).abs() > POS_DEAD_BAND;
            if pointer_up && all_finite && (size_moved || pos_moved) {
                config.settings_win_w = Some(sz.x);
                config.settings_win_h = Some(sz.y);
                config.settings_win_x = Some(pos.x);
                config.settings_win_y = Some(pos.y);
                changed = true;
            }
        }
    }

    ctx.data_mut(|d| {
        d.insert_temp(cat_id, category);
        d.insert_temp(q_id, query);
    });
    *open = keep_open;

    Outcome {
        changed,
        theme_changed: changed && config.theme != theme_before,
        clear_history,
        set_incognito,
        window_rect: win_resp.as_ref().map(|r| r.response.rect),
    }
}

/// A deferred toolbar edit, applied AFTER the render loop so an immutable
/// render-borrow of a zone list never overlaps the mutable edit.
enum ToolbarEdit {
    /// Move the item at `idx` earlier (left/up) within its zone.
    Up(super::chrome_toolbar::Zone, usize),
    /// Move the item at `idx` later (right/down) within its zone.
    Down(super::chrome_toolbar::Zone, usize),
    /// Move `id` to a different zone (or Hidden).
    MoveTo(String, super::chrome_toolbar::Zone),
}

/// Settings → Toolbar: place each quick-action button in a zone — LEFT (after the
/// "+"), RIGHT (next to the settings gear), or the overflow "⋯" menu — or hide it;
/// reorder within a zone; and reset. Returns whether `config.toolbar` changed.
fn toolbar_settings_section(ui: &mut egui::Ui, config: &mut Config) -> bool {
    use super::chrome_toolbar::{self as tb, Zone};
    let mut changed = false;
    ui.heading("Toolbar");
    help(
        ui,
        "Choose where each quick-action button lives in the top bar: on the LEFT \
         (after the +), on the RIGHT (next to the settings gear), or parked in an \
         overflow menu. Use the arrows to reorder within a zone, the X to remove a \
         button, and the dots menu to move it to another zone.",
    );

    // One deferred edit per frame (the user clicks a single control), applied
    // after every list has rendered so no render-borrow overlaps the mutation.
    let mut edit: Option<ToolbarEdit> = None;

    toolbar_zone_list(
        ui,
        "Left of the gear (after +)",
        &config.toolbar.left,
        Zone::Left,
        &mut edit,
    );
    ui.add_space(8.0);
    toolbar_zone_list(
        ui,
        "Right (next to the gear)",
        &config.toolbar.right,
        Zone::Right,
        &mut edit,
    );

    ui.add_space(8.0);
    changed |= ui
        .checkbox(
            &mut config.toolbar.show_overflow,
            "Show the overflow menu button when its menu has actions",
        )
        .changed();
    toolbar_zone_list(
        ui,
        "Overflow menu",
        &config.toolbar.menu,
        Zone::Menu,
        &mut edit,
    );

    // Hidden pool — actions currently shown nowhere, each with a "show on ▾" menu.
    let hidden = tb::hidden_actions(
        &config.toolbar.left,
        &config.toolbar.right,
        &config.toolbar.menu,
    );
    if !hidden.is_empty() {
        ui.add_space(8.0);
        ui.label(egui::RichText::new("Hidden (not shown)").strong());
        for &id in &hidden {
            ui.horizontal(|ui| {
                toolbar_place_menu(ui, id, None, &mut edit);
                ui.label(tb::action_label(id).unwrap_or(id));
            });
        }
    }

    ui.add_space(10.0);
    if ui
        .button("Reset toolbar to defaults")
        .on_hover_text("Restore the default button placement (script launcher on the right, the rest on the left).")
        .clicked()
    {
        config.toolbar = c0pl4nd_core::config::ToolbarConfig::default();
        changed = true;
    }

    if let Some(e) = edit {
        changed |= apply_toolbar_edit(config, e);
    }
    changed
}

/// Render one zone's action list: each row is `▲ ▼ ⋯move  Label`. Sets `*edit` on
/// a click (applied by the caller after every list has rendered). Zone-agnostic.
fn toolbar_zone_list(
    ui: &mut egui::Ui,
    title: &str,
    list: &[String],
    zone: super::chrome_toolbar::Zone,
    edit: &mut Option<ToolbarEdit>,
) {
    ui.label(egui::RichText::new(title).strong());
    if list.is_empty() {
        ui.weak("   — none —");
        return;
    }
    let n = list.len();
    for (i, id) in list.iter().enumerate() {
        ui.horizontal(|ui| {
            ui.add_enabled_ui(i > 0, |ui| {
                if tb_icon_button(ui, egui_phosphor::thin::CARET_UP, "Move earlier (left)")
                    .clicked()
                {
                    *edit = Some(ToolbarEdit::Up(zone, i));
                }
            });
            ui.add_enabled_ui(i + 1 < n, |ui| {
                if tb_icon_button(ui, egui_phosphor::thin::CARET_DOWN, "Move later (right)")
                    .clicked()
                {
                    *edit = Some(ToolbarEdit::Down(zone, i));
                }
            });
            // Remove this button from the bar (moves it to the Hidden pool, whence
            // it can be re-added) — the visible X remove affordance.
            if tb_icon_button(ui, egui_phosphor::thin::X, "Remove from the bar").clicked() {
                *edit = Some(ToolbarEdit::MoveTo(
                    id.clone(),
                    super::chrome_toolbar::Zone::Hidden,
                ));
            }
            toolbar_place_menu(ui, id, Some(zone), edit);
            ui.label(super::chrome_toolbar::action_label(id).unwrap_or(id));
        });
    }
}

/// A "move to zone" (⋯) menu for a toolbar-editor row. `current` is the action's
/// present zone (`None` = a hidden action). Offers every OTHER zone (+ Hide for a
/// shown action). Sets `*edit` on selection.
fn toolbar_place_menu(
    ui: &mut egui::Ui,
    id: &str,
    current: Option<super::chrome_toolbar::Zone>,
    edit: &mut Option<ToolbarEdit>,
) {
    use super::chrome_toolbar::Zone;
    // Zone moves only (Left / Right / Overflow); removal is the row's X button.
    let targets = [
        (Zone::Left, "Move to left"),
        (Zone::Right, "Move to right"),
        (Zone::Menu, "Move to overflow menu"),
    ];
    let hover = if current.is_none() {
        "Add this button to the bar"
    } else {
        "Move this button to another zone"
    };
    ui.menu_button(
        egui::RichText::new(egui_phosphor::thin::DOTS_THREE).size(14.0),
        |ui| {
            for (tz, label) in targets {
                if current == Some(tz) {
                    continue; // already in this zone
                }
                // A hidden action is being ADDED — say so instead of "Move to".
                let text = if current.is_none() {
                    match tz {
                        Zone::Left => "Add to the left",
                        Zone::Right => "Add to the right",
                        Zone::Menu => "Add to the overflow menu",
                        Zone::Hidden => label,
                    }
                } else {
                    label
                };
                if ui.button(text).clicked() {
                    *edit = Some(ToolbarEdit::MoveTo(id.to_string(), tz));
                    ui.close_kind(egui::UiKind::Menu);
                }
            }
        },
    )
    .response
    .on_hover_text(hover);
}

/// A compact Phosphor-glyph button for the toolbar editor (loaded icon font, not a
/// raw Unicode arrow — those render as tofu in the UI font).
fn tb_icon_button(ui: &mut egui::Ui, glyph: &str, hover: &str) -> egui::Response {
    ui.button(egui::RichText::new(glyph).size(14.0))
        .on_hover_text(hover)
}

/// Apply a single deferred toolbar edit to the live config. Returns whether it
/// changed anything.
fn apply_toolbar_edit(config: &mut Config, edit: ToolbarEdit) -> bool {
    use super::chrome_toolbar::{self as tb, Zone};
    match edit {
        ToolbarEdit::Up(z, i) => match z {
            Zone::Left => tb::move_up(&mut config.toolbar.left, i),
            Zone::Right => tb::move_up(&mut config.toolbar.right, i),
            Zone::Menu => tb::move_up(&mut config.toolbar.menu, i),
            Zone::Hidden => false,
        },
        ToolbarEdit::Down(z, i) => match z {
            Zone::Left => tb::move_down(&mut config.toolbar.left, i),
            Zone::Right => tb::move_down(&mut config.toolbar.right, i),
            Zone::Menu => tb::move_down(&mut config.toolbar.menu, i),
            Zone::Hidden => false,
        },
        ToolbarEdit::MoveTo(id, target) => tb::move_to_zone(
            &mut config.toolbar.left,
            &mut config.toolbar.right,
            &mut config.toolbar.menu,
            target,
            &id,
        ),
    }
}

/// Render every category section visible for the current selection / search
/// query. The single most impactful "un-cramp" change is setting
/// `item_spacing` to 8 px vertical (vs egui's default 3 px) at the top; section
/// gaps add a further 14 px of breathing room.
#[allow(clippy::too_many_arguments)]
fn render_sections(
    ui: &mut egui::Ui,
    config: &mut Config,
    updater: &Arc<Mutex<Updater>>,
    sel: &str,
    q: &str,
    incognito: bool,
    clear_history: &mut bool,
    set_incognito: &mut Option<bool>,
) -> bool {
    let mut changed = false;
    // Un-cramp every row + give buttons comfier hit targets (the load-bearing
    // spacing fix the user asked for).
    ui.spacing_mut().item_spacing = egui::vec2(8.0, 8.0);
    ui.spacing_mut().button_padding = egui::vec2(8.0, 4.0);
    // Page margin so content never hugs the scrollbar / separator.
    ui.add_space(4.0);
    let group_gap = |ui: &mut egui::Ui| ui.add_space(14.0);
    // Used by every per-setting ↺ revert. Cheap to construct once per render.
    let def = Config::default();

    // Shared Grid config so every section's label column is the same width and
    // all controls line up at the same x — the aligned, scannable look.
    let grid = |id: &'static str| {
        egui::Grid::new(id)
            .num_columns(3) // label | control | ↺
            .spacing([24.0, 10.0])
            .min_col_width(140.0) // SCR1B3-parity column floor
    };

    // Sub-group header WITHIN a section: a strong label, a muted one-line
    // description, and a divider rule — the SCR1B3 settings idiom that breaks a
    // long section into scannable, clearly-titled blocks with visible dividers
    // (the "divider lines, clear headings, descriptions" the user asked for).
    // Skipped while a search query is active: search flattens to the matching
    // rows, so decorative sub-headers (which don't themselves match) would just
    // leave empty titled blocks.
    let group = |ui: &mut egui::Ui, label: &str, desc: &str| {
        if !q.is_empty() {
            return;
        }
        ui.add_space(8.0);
        ui.label(egui::RichText::new(label).strong());
        if !desc.is_empty() {
            ui.label(egui::RichText::new(desc).weak().small());
        }
        ui.separator();
    };

    // ---------------------------------------------------------------- Appearance
    if section_visible(sel, q, "Appearance", APPEARANCE_SEARCH_LABELS) {
        ui.heading("Appearance");
        help(ui, "Colors and window transparency.");
        group(ui, "Theme", "The terminal colour palette.");
        grid("appearance_theme").show(ui, |ui| {
            if row_visible(q, "theme color") {
                ui.label("Theme")
                    .on_hover_text("Terminal color theme — a file stem under the themes dir.");
                ui.horizontal(|ui| {
                    egui::ComboBox::from_id_salt("c0pl4nd-theme-picker")
                        .selected_text(config.theme.clone())
                        .show_ui(ui, |ui| {
                            for name in BUILTIN_THEMES {
                                changed |= ui
                                    .selectable_value(&mut config.theme, (*name).to_string(), *name)
                                    .changed();
                            }
                        });
                    // Up/down step arrows: cycle through the built-ins without
                    // opening the dropdown. `delta` wraps around the list; a
                    // custom (non-built-in) theme name steps onto the first/last
                    // built-in so the arrows always have a defined landing spot.
                    let mut step = |delta: isize| {
                        let n = BUILTIN_THEMES.len() as isize;
                        let cur = BUILTIN_THEMES.iter().position(|t| *t == config.theme);
                        let next = match cur {
                            Some(i) => (i as isize + delta).rem_euclid(n),
                            None if delta > 0 => 0,
                            None => n - 1,
                        };
                        config.theme = BUILTIN_THEMES[next as usize].to_string();
                        changed = true;
                    };
                    if ui
                        .small_button("⏶")
                        .on_hover_text("Previous theme")
                        .clicked()
                    {
                        step(-1);
                    }
                    if ui.small_button("⏷").on_hover_text("Next theme").clicked() {
                        step(1);
                    }
                });
                changed |= reset_to_default(ui, &mut config.theme, &def.theme);
                ui.end_row();

                ui.label("…or theme name");
                changed |= ui
                    .add(
                        egui::TextEdit::singleline(&mut config.theme)
                            .hint_text("itasha-corp")
                            .desired_width(200.0),
                    )
                    .on_hover_text(
                        "A user theme TOML under the config dir's themes folder overrides \
                         the built-in of the same name.",
                    )
                    .changed();
                ui.label(""); // no ↺ on the alias field (it edits the same `theme`)
                ui.end_row();
            }
        });

        group(
            ui,
            "Transparency & tint",
            "Make the window see-through, choose the OS blur backend, and wash it \
             with a colour.",
        );
        grid("appearance_transparency").show(ui, |ui| {
            // ---- Transparency / glass (SCR1B3-parity model) ----
            // Master on/off switch for the whole transparency system. Off by
            // default: a solid window is fast and never leaves a DWM ghost on
            // close. Live-applies the opacity/tint passes immediately; switching
            // a native-blur backend (glass/mica/vibrancy) on/off needs a window
            // re-init, so that part takes effect on restart (labelled below).
            if row_visible(q, "transparency master enable glass") {
                ui.label("Transparency")
                    .on_hover_text("Master switch for the whole transparency system.");
                changed |= ui
                    .checkbox(
                        &mut config.transparency_enabled,
                        "Enable window transparency",
                    )
                    .on_hover_text(
                        "Master switch. When off, the window is fully opaque \
                         regardless of the mode below. Turn on to use transparent / \
                         glass / mica / vibrancy.",
                    )
                    .changed();
                changed |= reset_to_default(
                    ui,
                    &mut config.transparency_enabled,
                    &def.transparency_enabled,
                );
                ui.end_row();
            }

            // Each dependent row is THREE direct grid cells (label · control · ↺)
            // exactly like every other section — NOT one `add_enabled_ui` wrapping
            // all three, which collapsed the row into a single column-1 cell and
            // left columns 2/3 empty (the reported "Transparency & tint isn't
            // aligned in columns"). The control cell greys out when disabled; the
            // label + reset stay put so the columns line up on every row.
            if row_visible(q, "transparency mode glass mica vibrancy") {
                let en = config.transparency_enabled;
                ui.label("Mode").on_hover_text(
                    "Opaque · Transparent (portable) · Glass/Mica/Vibrancy \
                     (OS blur — applies on restart).",
                );
                ui.add_enabled_ui(en, |ui| {
                    let wmodes = [
                        (WindowMode::Opaque, "opaque"),
                        (WindowMode::Transparent, "transparent"),
                        (WindowMode::Glass, "glass / acrylic"),
                        (WindowMode::Mica, "mica (Win11)"),
                        (WindowMode::Vibrancy, "vibrancy (macOS)"),
                    ];
                    egui::ComboBox::from_id_salt("c0pl4nd-window-mode")
                        .selected_text(
                            wmodes
                                .iter()
                                .find(|(m, _)| *m == config.window_mode)
                                .map(|(_, s)| *s)
                                .unwrap_or("opaque"),
                        )
                        .show_ui(ui, |ui| {
                            for (m, label) in wmodes {
                                changed |= ui
                                    .selectable_value(&mut config.window_mode, m, label)
                                    .changed();
                            }
                        })
                        .response
                        .on_hover_text(
                            "Transparent applies live; Glass/Mica/Vibrancy switch \
                             the OS blur backend and apply on restart.",
                        );
                });
                changed |= reset_to_default(ui, &mut config.window_mode, &def.window_mode);
                ui.end_row();
            }

            if row_visible(q, "opacity transparency") {
                let en = config.transparency_enabled && config.window_mode.is_translucent();
                ui.label("Opacity").on_hover_text(
                    "Surface opacity for every translucent mode (Glass / Mica / \
                     Vibrancy / Transparent) — below 100% the desktop / blur shows \
                     through, and the slider tunes how see-through the terminal is \
                     across its full range.",
                );
                changed |= ui
                    .add_enabled(
                        en,
                        egui::Slider::new(&mut config.opacity, 0.0..=1.0)
                            .custom_formatter(|v, _| format!("{:.0}%", v * 100.0))
                            .custom_parser(|s| {
                                s.trim_end_matches('%')
                                    .trim()
                                    .parse::<f64>()
                                    .ok()
                                    .map(|v| v / 100.0)
                            }),
                    )
                    .changed();
                changed |= reset_to_default(ui, &mut config.opacity, &def.opacity);
                ui.end_row();
            }

            if row_visible(q, "tint color overlay wash picker") {
                let en = config.transparency_enabled;
                ui.label("Tint color")
                    .on_hover_text("Color washed over the window (strength below).");
                ui.add_enabled_ui(en, |ui| {
                    ui.horizontal(|ui| {
                        // A real color PICKER (swatch button → palette popup) over
                        // the `#RRGGBB` config string, instead of raw hex entry.
                        // parse the stored hex → swatch; on pick, write hex back.
                        let (r, g, b) =
                            c0pl4nd_core::theme::parse_hex(&config.tint).unwrap_or((18, 18, 18));
                        let mut rgb = [r, g, b];
                        if ui.color_edit_button_srgb(&mut rgb).changed() {
                            config.tint = format!("#{:02x}{:02x}{:02x}", rgb[0], rgb[1], rgb[2]);
                            changed = true;
                        }
                        // Compact hex readout so the exact value is visible/copyable.
                        ui.monospace(&config.tint);
                    });
                });
                changed |= reset_to_default(ui, &mut config.tint, &def.tint);
                ui.end_row();
            }

            if row_visible(q, "tint strength wash overlay") {
                let en = config.transparency_enabled;
                ui.label("Tint strength")
                    .on_hover_text("0% = no tint .. 100% = strong color wash.");
                changed |= ui
                    .add_enabled(
                        en,
                        egui::Slider::new(&mut config.tint_strength, 0.0..=1.0)
                            .custom_formatter(|v, _| format!("{:.0}%", v * 100.0))
                            .custom_parser(|s| {
                                s.trim_end_matches('%')
                                    .trim()
                                    .parse::<f64>()
                                    .ok()
                                    .map(|v| v / 100.0)
                            }),
                    )
                    .changed();
                changed |= reset_to_default(ui, &mut config.tint_strength, &def.tint_strength);
                ui.end_row();
            }
        });

        group(
            ui,
            "Interface scale",
            "Accessibility zoom for the whole interface, saved across launches.",
        );
        grid("appearance_scale").show(ui, |ui| {
            // ---- UI scale (F2-3): persisted accessibility zoom for the UI ----
            // Placed AFTER opacity + tint so the existing slider-order assertions
            // in egui_settings.rs (opacity = slider 0, tint = slider 1) hold.
            if row_visible(q, "ui scale zoom accessibility size") {
                ui.label("UI scale").on_hover_text(
                    "Accessibility zoom for the WHOLE interface (chrome + grid), \
                     persisted across launches. 1.0 = 100%. (Ctrl+/- also zooms, \
                     but is not saved.)",
                );
                changed |= ui
                    .add(
                        egui::Slider::new(&mut config.ui_scale, 0.5..=3.0)
                            .fixed_decimals(2)
                            .suffix("×"),
                    )
                    .changed();
                changed |= reset_to_default(ui, &mut config.ui_scale, &def.ui_scale);
                ui.end_row();
            }
        });
        group_gap(ui);
    }

    // ---------------------------------------------------------------------- Font
    if section_visible(
        sel,
        q,
        "Fonts",
        &[
            "font",
            "family",
            "size",
            "line height",
            "ligatures",
            "fallback",
        ],
    ) {
        ui.heading("Fonts");
        help(ui, "Typeface, size, and text shaping.");
        grid("font_grid").show(ui, |ui| {
            if row_visible(q, "family typeface") {
                ui.label("Family").on_hover_text(
                    "Primary monospace typeface, picked from the fonts installed \
                     on this system. Applies live.",
                );
                // The installed monospace families (enumerated once + cached) plus
                // the built-in label. A ComboBox so the user picks a real font
                // that the app actually loads — not free text that did nothing.
                let choices = super::fonts::monospace_family_choices();
                let selected = family_display(&config.font.family);
                egui::ComboBox::from_id_salt("c0pl4nd-font-family")
                    .selected_text(selected)
                    .width(220.0)
                    .show_ui(ui, |ui| {
                        for fam in choices {
                            let value = family_value(fam);
                            changed |= ui
                                .selectable_value(&mut config.font.family, value, fam)
                                .changed();
                        }
                    });
                changed |= reset_to_default(ui, &mut config.font.family, &def.font.family);
                ui.end_row();
            }

            if row_visible(q, "size") {
                ui.label("Size");
                changed |= ui
                    .add(
                        egui::Slider::new(&mut config.font.size, 8.0..=32.0)
                            .suffix(" pt")
                            .step_by(0.5),
                    )
                    .changed();
                changed |= reset_to_default(ui, &mut config.font.size, &def.font.size);
                ui.end_row();
            }

            if row_visible(q, "line height") {
                ui.label("Line height").on_hover_text(
                    "Row height for the primary font. Applies on restart — the grid \
                     cell metrics are derived at launch.",
                );
                changed |= ui
                    .add(egui::Slider::new(&mut config.font.line_height, 12.0..=48.0).suffix(" px"))
                    .on_hover_text("Applies on the next launch.")
                    .changed();
                changed |=
                    reset_to_default(ui, &mut config.font.line_height, &def.font.line_height);
                ui.end_row();
            }

            if row_visible(q, "ligatures shaping") {
                ui.label("Ligatures");
                // DISABLED: the egui native text painter draws the grid glyph-by-
                // glyph (strict monospace cell fidelity) and does NOT run a shaping
                // engine (HarfBuzz / cosmic-text), so programming ligatures can't
                // be formed. Shown greyed with an honest tooltip rather than as a
                // dead toggle that silently does nothing.
                ui.add_enabled_ui(false, |ui| {
                    ui.checkbox(&mut config.ligatures, "Programming ligatures (->, !=)")
                        .on_hover_text(
                            "Not available: the GPU text renderer draws strict \
                             monospace cells and does not do glyph shaping.",
                        );
                });
                ui.end_row();
            }

            // Two ORDERED fallback slots for glyphs the primary font lacks (CJK,
            // Arabic, emoji). Each slot is a ComboBox over the installed
            // monospace families + a "(none)" choice; the picks are round-tripped
            // into the `Vec<String>` config field (a "(none)" slot is dropped, so
            // an empty earlier slot never leaves a blank family before a later
            // one). Applies live alongside the primary family.
            if row_visible(q, "fallback fonts families cjk polyglot") {
                ui.label("Fallback fonts").on_hover_text(
                    "Up to two fallback families for glyphs the primary font lacks \
                     (CJK, Arabic, emoji). Applies live.",
                );
                // The two current slots (padded with "(none)" so both combos
                // always render even when the config has 0 or 1 fallbacks).
                let mut slot0 = config
                    .font
                    .fallback
                    .first()
                    .cloned()
                    .unwrap_or_else(|| super::fonts::NONE_LABEL.to_string());
                let mut slot1 = config
                    .font
                    .fallback
                    .get(1)
                    .cloned()
                    .unwrap_or_else(|| super::fonts::NONE_LABEL.to_string());
                let choices = super::fonts::monospace_family_choices();
                let mut slots_changed = false;
                ui.vertical(|ui| {
                    slots_changed |=
                        fallback_combo(ui, "c0pl4nd-font-fallback-0", choices, &mut slot0);
                    slots_changed |=
                        fallback_combo(ui, "c0pl4nd-font-fallback-1", choices, &mut slot1);
                });
                if slots_changed {
                    // Rebuild the ordered vec, dropping the "(none)" sentinel and
                    // the built-in label (neither is a real fallback face), and
                    // collapsing a hole so [none, "Noto"] becomes ["Noto"].
                    config.font.fallback = [slot0, slot1]
                        .into_iter()
                        .filter(|s| {
                            !s.trim().is_empty()
                                && s != super::fonts::NONE_LABEL
                                && s != super::fonts::BUILTIN_MONOSPACE_LABEL
                        })
                        .collect();
                    changed = true;
                }
                changed |= reset_to_default(ui, &mut config.font.fallback, &def.font.fallback);
                ui.end_row();
            }
        });
        group_gap(ui);
    }

    // -------------------------------------------------------------------- Cursor
    if section_visible(sel, q, "Cursor", &["style", "blink"]) {
        ui.heading("Cursor");
        help(ui, "Caret shape and blink.");
        grid("cursor_grid").show(ui, |ui| {
            if row_visible(q, "style shape block bar underline") {
                ui.label("Style");
                egui::ComboBox::from_id_salt("c0pl4nd-cursor-style")
                    .selected_text(cursor_style_label(config.cursor.style))
                    .show_ui(ui, |ui| {
                        for (val, label) in [
                            (CursorStyle::Block, "block"),
                            (CursorStyle::Bar, "bar"),
                            (CursorStyle::Underline, "underline"),
                        ] {
                            changed |= ui
                                .selectable_value(&mut config.cursor.style, val, label)
                                .changed();
                        }
                    });
                changed |= reset_to_default(ui, &mut config.cursor.style, &def.cursor.style);
                ui.end_row();
            }

            if row_visible(q, "blink") {
                ui.label("Blink");
                changed |= ui
                    .checkbox(&mut config.cursor.blink, "Blink the cursor")
                    .changed();
                changed |= reset_to_default(ui, &mut config.cursor.blink, &def.cursor.blink);
                ui.end_row();
            }
        });
        group_gap(ui);
    }

    // ------------------------------------------------------------------ Terminal
    if section_visible(
        sel,
        q,
        "Terminal",
        &[
            "scrollback",
            "startup panel",
            "shell",
            "copy on select",
            "paste",
        ],
    ) {
        ui.heading("Terminal");
        help(ui, "Scrollback, shell, and clipboard behavior.");
        group(
            ui,
            "Shell & scrollback",
            "Which shell launches and how much history each pane keeps.",
        );
        grid("terminal_shell").show(ui, |ui| {
            if row_visible(q, "scrollback lines history") {
                ui.label("Scrollback").on_hover_text(
                    "History lines kept per pane. Applies on restart — a pane's \
                     buffer is sized when its shell spawns.",
                );
                changed |= ui
                    .add(
                        egui::Slider::new(&mut config.scrollback_lines, 100..=1_000_000)
                            .logarithmic(true)
                            .suffix(" lines"),
                    )
                    .on_hover_text("Applies on the next launch.")
                    .changed();
                changed |=
                    reset_to_default(ui, &mut config.scrollback_lines, &def.scrollback_lines);
                ui.end_row();
            }

            if row_visible(q, "startup panel neofetch logo") {
                ui.label("Startup panel");
                // DISABLED: the neofetch-style launch splash is not drawn by the
                // egui shell (only the legacy winit shell rendered it). Greyed
                // with an honest tooltip rather than a dead toggle that silently
                // does nothing — matching the ligatures / copy-on-select rows.
                ui.add_enabled_ui(false, |ui| {
                    ui.checkbox(&mut config.startup_panel, "Show logo + system info")
                        .on_hover_text(
                            "Not available: the launch splash is not drawn in this shell yet.",
                        );
                });
                ui.end_row();
            }

            if row_visible(q, "shell override program") {
                ui.label("Shell override")
                    .on_hover_text("Leave empty to use the OS default shell.");
                let mut shell = config.shell.clone().unwrap_or_default();
                if ui
                    .add(
                        egui::TextEdit::singleline(&mut shell)
                            .hint_text("platform default")
                            .desired_width(200.0),
                    )
                    .changed()
                {
                    config.shell = if shell.trim().is_empty() {
                        None
                    } else {
                        Some(shell)
                    };
                    changed = true;
                }
                changed |= reset_to_default(ui, &mut config.shell, &def.shell);
                ui.end_row();
            }
        });

        group(ui, "Clipboard", "Copy-on-select and paste safety.");
        grid("terminal_clipboard").show(ui, |ui| {
            if row_visible(q, "copy on select clipboard") {
                ui.label("Copy on select");
                // Live again: mouse text-selection now exists in the egui shell
                // (drag to select), so the drag-end can auto-copy. When OFF the
                // selection is still made (and Ctrl/Cmd+Shift+C copies on demand);
                // when ON the selection is copied to the clipboard on release.
                changed |= ui
                    .checkbox(&mut config.copy_on_select, "X11-style auto-copy")
                    .on_hover_text(
                        "Copy a mouse selection to the clipboard automatically when \
                         the drag is released.",
                    )
                    .changed();
                changed |= reset_to_default(ui, &mut config.copy_on_select, &def.copy_on_select);
                ui.end_row();
            }

            if row_visible(q, "paste warn multiline newline security") {
                ui.label("Multi-line paste");
                changed |= ui
                    .checkbox(
                        &mut config.paste_warn_multiline,
                        "Warn before multi-line paste",
                    )
                    .on_hover_text("Security: a pasted newline can run a shell command instantly.")
                    .changed();
                changed |= reset_to_default(
                    ui,
                    &mut config.paste_warn_multiline,
                    &def.paste_warn_multiline,
                );
                ui.end_row();
            }
        });
        group_gap(ui);
    }

    // -------------------------------------------------------------------- Window
    if section_visible(
        sel,
        q,
        "Window",
        &[
            "padding",
            "columns",
            "rows",
            "panes",
            "dividers",
            "linked",
            "symmetrical",
            "status bar",
            "graphics",
            "backend",
            "gpu",
            "renderer",
        ],
    ) {
        ui.heading("Window");
        help(
            ui,
            "Inner padding and the initial grid size. Live size/position is \
             remembered automatically.",
        );
        group(
            ui,
            "Layout",
            "Padding, split-pane dividers, and the status bar.",
        );
        grid("window_layout").show(ui, |ui| {
            if row_visible(q, "padding inner margin") {
                ui.label("Padding")
                    .on_hover_text("Inner inset between the pane edge and the terminal grid.");
                changed |= ui
                    .add(
                        egui::DragValue::new(&mut config.window.padding)
                            .range(0..=32)
                            .suffix(" px"),
                    )
                    .on_hover_text("Applies live — the grid re-insets without a restart.")
                    .changed();
                changed |= reset_to_default(ui, &mut config.window.padding, &def.window.padding);
                ui.end_row();
            }

            if row_visible(q, "panes dividers linked symmetrical equal split") {
                ui.label("Linked dividers");
                changed |= ui
                    .checkbox(&mut config.link_pane_dividers, "Keep panes equal")
                    .on_hover_text(
                        "Hold split-pane dividers at equal positions so every pane \
                         stays the same size. The top-bar symmetrical button equalises \
                         once regardless of this setting.",
                    )
                    .changed();
                changed |=
                    reset_to_default(ui, &mut config.link_pane_dividers, &def.link_pane_dividers);
                ui.end_row();
            }

            if row_visible(q, "status bar bottom hide show") {
                ui.label("Status bar");
                changed |= ui
                    .checkbox(&mut config.show_status_bar, "Show bottom status bar")
                    .on_hover_text(
                        "The bottom bar (pane count + hints). Turn off to reclaim the \
                         row for the terminal grid.",
                    )
                    .changed();
                changed |= reset_to_default(ui, &mut config.show_status_bar, &def.show_status_bar);
                ui.end_row();
            }
        });

        group(
            ui,
            "Graphics",
            "GPU backend and adapter — change only if glyphs render garbled.",
        );
        grid("window_graphics").show(ui, |ui| {
            if row_visible(q, "graphics backend gpu renderer dx12 vulkan gl") {
                ui.label("Graphics backend");
                let backends = [
                    (GraphicsBackend::Auto, "auto (recommended)"),
                    (GraphicsBackend::Dx12, "DX12"),
                    (GraphicsBackend::Vulkan, "Vulkan"),
                    (GraphicsBackend::Gl, "OpenGL"),
                ];
                egui::ComboBox::from_id_salt("c0pl4nd-graphics-backend")
                    .selected_text(
                        backends
                            .iter()
                            .find(|(b, _)| *b == config.graphics_backend)
                            .map(|(_, s)| *s)
                            .unwrap_or("auto (recommended)"),
                    )
                    .show_ui(ui, |ui| {
                        for (b, label) in backends {
                            changed |= ui
                                .selectable_value(&mut config.graphics_backend, b, label)
                                .changed();
                        }
                    })
                    .response
                    .on_hover_text(
                        "The GPU backend the renderer uses (Windows). Leave on Auto \
                         unless the terminal grid renders corrupted/garbled glyphs — \
                         then try Vulkan (or OpenGL) to route around a bad DX12 \
                         driver. Applies on restart.",
                    );
                changed |=
                    reset_to_default(ui, &mut config.graphics_backend, &def.graphics_backend);
                ui.end_row();
            }

            if row_visible(
                q,
                "graphics gpu preference integrated discrete optimus hybrid",
            ) {
                ui.label("GPU preference");
                let gpus = [
                    (GpuPreference::Auto, "auto (recommended)"),
                    (GpuPreference::Integrated, "integrated (low power)"),
                    (GpuPreference::Discrete, "discrete (high performance)"),
                ];
                egui::ComboBox::from_id_salt("c0pl4nd-gpu-preference")
                    .selected_text(
                        gpus.iter()
                            .find(|(g, _)| *g == config.graphics_gpu)
                            .map(|(_, s)| *s)
                            .unwrap_or("auto (recommended)"),
                    )
                    .show_ui(ui, |ui| {
                        for (g, label) in gpus {
                            changed |= ui
                                .selectable_value(&mut config.graphics_gpu, g, label)
                                .changed();
                        }
                    })
                    .response
                    .on_hover_text(
                        "Which GPU to render on, on a laptop with two GPUs (Intel + \
                         NVIDIA/AMD). Leave on Auto unless the terminal grid renders \
                         corrupted/garbled glyphs on the discrete GPU — then pick \
                         Integrated to render on the built-in GPU. Applies on restart.",
                    );
                changed |= reset_to_default(ui, &mut config.graphics_gpu, &def.graphics_gpu);
                ui.end_row();
            }
        });

        group(
            ui,
            "Initial grid size",
            "Legacy columns/rows — this shell sizes the window in pixels and \
             remembers it, so these are inactive.",
        );
        grid("window_initial_size").show(ui, |ui| {
            // Initial terminal grid width at launch. The live window size is
            // remembered separately (geometry persistence), so this is the
            // first-launch / no-saved-geometry size; it takes effect on restart.
            // Floor of 1 mirrors the core validator (cols/rows must be non-zero).
            // DISABLED: this shell sizes the window in PIXELS and remembers the
            // size across launches (drag the window edge; eframe persists it) —
            // it has no columns/rows startup-size path, so these legacy fields are
            // inert here. Greyed with an honest tooltip rather than left as live
            // sliders that silently do nothing (matching the ligatures /
            // startup-panel rows). The TOML fields remain for the legacy shell.
            if row_visible(q, "columns cols initial width grid size") {
                ui.label("Initial columns");
                ui.add_enabled_ui(false, |ui| {
                    ui.add(
                        egui::DragValue::new(&mut config.window.cols)
                            .range(1..=500)
                            .suffix(" cols"),
                    )
                    .on_hover_text(
                        "Not used in this shell: the window is sized in pixels and \
                         its size is remembered across launches (resize by dragging \
                         the window edge).",
                    );
                });
                ui.end_row();
            }

            if row_visible(q, "rows initial height grid size") {
                ui.label("Initial rows");
                ui.add_enabled_ui(false, |ui| {
                    ui.add(
                        egui::DragValue::new(&mut config.window.rows)
                            .range(1..=300)
                            .suffix(" rows"),
                    )
                    .on_hover_text(
                        "Not used in this shell: the window is sized in pixels and \
                         its size is remembered across launches (resize by dragging \
                         the window edge).",
                    );
                });
                ui.end_row();
            }
        });
        group_gap(ui);
    }

    // ------------------------------------------------------------------- Toolbar
    if section_visible(
        sel,
        q,
        "Toolbar",
        &[
            "toolbar",
            "buttons",
            "quick actions",
            "top bar",
            "overflow",
            "reorder",
        ],
    ) {
        changed |= toolbar_settings_section(ui, config);
        group_gap(ui);
    }

    // ---------------------------------------------------------------------- Motion
    if section_visible(sel, q, "Motion", MOTION_SEARCH_LABELS) {
        ui.heading("Motion");
        help(
            ui,
            "Interface animation and retro CRT post-effects. Turn the master \
             switch off for a fully static UI; every effect below is gated behind \
             it and off by default.",
        );
        grid("motion_grid").show(ui, |ui| {
            if row_visible(q, "enable animations") {
                ui.label("Animations");
                changed |= ui
                    .checkbox(&mut config.effects.animations_enabled, "Enable animations")
                    .on_hover_text(
                        "Master switch. When off, transitions are instant (no fades) \
                         and every motion effect below is suppressed — idle frames \
                         cost the same as plain egui.",
                    )
                    .changed();
                changed |= reset_to_default(
                    ui,
                    &mut config.effects.animations_enabled,
                    &def.effects.animations_enabled,
                );
                ui.end_row();
            }

            let on = config.effects.animations_enabled;

            if row_visible(q, "animation speed") {
                ui.label("Animation speed").on_hover_text(
                    "Scale the motion-effect speed. 0 freezes every transition; 1 is \
                     the shipped feel; up to 2 runs the drift effects (mesh / VHS / \
                     flicker) at double-rate. Above 1 only speeds the effects — the \
                     UI fades stay snappy.",
                );
                changed |= ui
                    .add_enabled(
                        on,
                        egui::Slider::new(&mut config.effects.animation_intensity, 0.0..=2.0),
                    )
                    .changed();
                changed |= reset_to_default(
                    ui,
                    &mut config.effects.animation_intensity,
                    &def.effects.animation_intensity,
                );
                ui.end_row();
            }

            if row_visible(q, "crt scanlines") {
                ui.label("CRT scanlines");
                changed |= ui
                    .add_enabled(
                        on,
                        egui::Checkbox::new(
                            &mut config.effects.crt_scanlines,
                            "Animated scan lines",
                        ),
                    )
                    .on_hover_text(
                        "Dark scan-line bands drifting slowly down the panes for a \
                         calm retro CRT look. Auto-disabled under reduced-motion.",
                    )
                    .changed();
                changed |= reset_to_default(
                    ui,
                    &mut config.effects.crt_scanlines,
                    &def.effects.crt_scanlines,
                );
                ui.end_row();
            }

            if row_visible(q, "scanline darkness") {
                ui.label("Scanline darkness");
                changed |= ui
                    .add_enabled(
                        on && config.effects.crt_scanlines,
                        egui::Slider::new(&mut config.effects.scanline_darkness, 0.0..=1.0)
                            .text("darkness"),
                    )
                    .on_hover_text("How dark the scan-line troughs read. Enable scan lines first.")
                    .changed();
                changed |= reset_to_default(
                    ui,
                    &mut config.effects.scanline_darkness,
                    &def.effects.scanline_darkness,
                );
                ui.end_row();
            }

            if row_visible(q, "chromatic aberration") {
                ui.label("Chromatic aberration");
                // The checkbox + the intensity slider share ONE grid cell (col 2),
                // stacked vertically, so this stays a 3-column row like every other
                // one (a filling `Slider` in its own 4th column has no width budget
                // and balloons the grid past the close button).
                ui.vertical(|ui| {
                    // Explicit ON/OFF checkbox: the intensity slider alone read as
                    // "broken" when it sat at 0. On first enable, default the
                    // intensity to a visible value so the effect shows.
                    let was_enabled = config.effects.chromatic_aberration_enabled;
                    if ui
                        .add_enabled(
                            on,
                            egui::Checkbox::new(
                                &mut config.effects.chromatic_aberration_enabled,
                                "RGB split",
                            ),
                        )
                        .on_hover_text("Pure-channel red/blue fringing behind the text.")
                        .changed()
                    {
                        changed = true;
                        if config.effects.chromatic_aberration_enabled
                            && !was_enabled
                            && config.effects.chromatic_aberration <= 0.0
                        {
                            config.effects.chromatic_aberration =
                                c0pl4nd_core::config::DEFAULT_CHROMATIC_INTENSITY;
                        }
                    }
                    let ca_on = on && config.effects.chromatic_aberration_enabled;
                    changed |= ui
                        .add_enabled(
                            ca_on,
                            egui::Slider::new(&mut config.effects.chromatic_aberration, 0.1..=1.5)
                                .text("intensity"),
                        )
                        .changed();
                });
                changed |= reset_to_default(
                    ui,
                    &mut config.effects.chromatic_aberration,
                    &def.effects.chromatic_aberration,
                );
                ui.end_row();
            }

            if row_visible(q, "wired mesh background") {
                ui.label("Wired mesh");
                changed |= ui
                    .add_enabled(
                        on,
                        egui::Checkbox::new(
                            &mut config.effects.wired_ambient,
                            "Node-mesh background",
                        ),
                    )
                    .on_hover_text(
                        "An animated node-mesh ambient background drawn BEHIND the \
                         panes (a calm \"Wired\" field). Shows through when the window \
                         uses a translucent mode; hidden behind fully-opaque panes.",
                    )
                    .changed();
                changed |= reset_to_default(
                    ui,
                    &mut config.effects.wired_ambient,
                    &def.effects.wired_ambient,
                );
                ui.end_row();
            }

            if row_visible(q, "mesh density") {
                ui.label("Mesh density");
                changed |= ui
                    .add_enabled(
                        on && config.effects.wired_ambient,
                        egui::Slider::new(&mut config.effects.mesh_density, 0.0..=2.0)
                            .text("density"),
                    )
                    .on_hover_text(
                        "How many nodes the wired-mesh background draws (sparse to dense).",
                    )
                    .changed();
                changed |= reset_to_default(
                    ui,
                    &mut config.effects.mesh_density,
                    &def.effects.mesh_density,
                );
                ui.end_row();
            }

            if row_visible(q, "mesh brightness") {
                ui.label("Mesh brightness");
                changed |= ui
                    .add_enabled(
                        on && config.effects.wired_ambient,
                        egui::Slider::new(&mut config.effects.mesh_brightness, 0.0..=3.0)
                            .text("brightness"),
                    )
                    .on_hover_text(
                        "How bright the wired-mesh lattice reads — dim it toward \
                         invisible or brighten it to clearly pop. 1 is the default.",
                    )
                    .changed();
                changed |= reset_to_default(
                    ui,
                    &mut config.effects.mesh_brightness,
                    &def.effects.mesh_brightness,
                );
                ui.end_row();
            }

            if row_visible(q, "vhs tracking") {
                ui.label("VHS tracking");
                changed |= ui
                    .add_enabled(
                        on,
                        egui::Checkbox::new(&mut config.effects.vhs_tracking, "Tracking lines"),
                    )
                    .on_hover_text(
                        "Faint bright bands sweep down the window like analogue tape \
                         tracking error.",
                    )
                    .changed();
                changed |= reset_to_default(
                    ui,
                    &mut config.effects.vhs_tracking,
                    &def.effects.vhs_tracking,
                );
                ui.end_row();
            }

            if row_visible(q, "screen flicker") {
                ui.label("Screen flicker");
                changed |= ui
                    .add_enabled(
                        on,
                        egui::Checkbox::new(&mut config.effects.flicker, "Brightness flicker"),
                    )
                    .on_hover_text("Subtle CRT-style brightness flicker over the whole window.")
                    .changed();
                changed |= reset_to_default(ui, &mut config.effects.flicker, &def.effects.flicker);
                ui.end_row();
            }

            if row_visible(q, "flicker strength") {
                ui.label("Flicker strength");
                changed |= ui
                    .add_enabled(
                        on && config.effects.flicker,
                        egui::Slider::new(&mut config.effects.flicker_strength, 0.0..=1.0)
                            .text("strength"),
                    )
                    .on_hover_text(
                        "How strong the screen flicker is. Even at max the wash stays \
                         short of a full-black strobe (comfort guard).",
                    )
                    .changed();
                changed |= reset_to_default(
                    ui,
                    &mut config.effects.flicker_strength,
                    &def.effects.flicker_strength,
                );
                ui.end_row();
            }

            if row_visible(q, "cursor trail") {
                ui.label("Cursor trail");
                changed |= ui
                    .add_enabled(
                        on,
                        egui::Checkbox::new(&mut config.effects.cursor_trail, "Ghost-trail"),
                    )
                    .on_hover_text("A fading echo follows the focused terminal cursor as it moves.")
                    .changed();
                changed |= reset_to_default(
                    ui,
                    &mut config.effects.cursor_trail,
                    &def.effects.cursor_trail,
                );
                ui.end_row();
            }

            if row_visible(q, "cursor trail intensity") {
                ui.label("Cursor-trail intensity");
                changed |= ui
                    .add_enabled(
                        on && config.effects.cursor_trail,
                        egui::Slider::new(&mut config.effects.cursor_trail_intensity, 0.0..=2.0)
                            .text("intensity"),
                    )
                    .on_hover_text(
                        "How bold and long the cursor ghost-trail is — from a faint \
                         flick to a pronounced comet tail. Up to 2 for a long, \
                         unmistakable tail.",
                    )
                    .changed();
                changed |= reset_to_default(
                    ui,
                    &mut config.effects.cursor_trail_intensity,
                    &def.effects.cursor_trail_intensity,
                );
                ui.end_row();
            }

            if row_visible(q, "boot glitch") {
                ui.label("Boot glitch");
                changed |= ui
                    .add_enabled(
                        on,
                        egui::Checkbox::new(&mut config.effects.boot_glitch, "Startup sweep"),
                    )
                    .on_hover_text(
                        "A one-shot glitch sweep plays for a moment when the app launches.",
                    )
                    .changed();
                changed |= reset_to_default(
                    ui,
                    &mut config.effects.boot_glitch,
                    &def.effects.boot_glitch,
                );
                ui.end_row();
            }
        });
        group_gap(ui);
    }

    // --------------------------------------------------------------- Keybindings
    if section_visible(
        sel,
        q,
        "Keybindings",
        &[
            "copy",
            "paste",
            "new tab",
            "close tab",
            "next tab",
            "split right",
            "split down",
            "search",
            "command palette",
            "increase font",
            "decrease font",
        ],
    ) {
        ui.heading("Keybindings");
        help(
            ui,
            "Reference — the shell's shortcuts are currently FIXED (not yet \
             user-rebindable in this shell). \"mod\" is Ctrl+Shift on \
             Windows/Linux, Cmd on macOS.",
        );
        // READ-ONLY: a configurable-keybinding dispatcher is not wired in the
        // egui shell — the shortcuts are hardcoded in `frame_tick`. The rows are
        // shown disabled (the active combo, for reference) rather than as
        // editable fields that silently control nothing (the prior dead-editor
        // state). Matches the ligatures / copy-on-select honest-disable pattern.
        grid("keybindings_grid").show(ui, |ui| {
            macro_rules! keybind_row {
                ($field:ident, $label:literal, $search:literal) => {
                    if row_visible(q, $search) {
                        ui.label($label);
                        ui.add_enabled_ui(false, |ui| {
                            ui.add(
                                egui::TextEdit::singleline(&mut config.keybindings.$field)
                                    .desired_width(180.0)
                                    .font(egui::TextStyle::Monospace),
                            )
                            .on_hover_text(
                                "Fixed shortcut — not yet user-rebindable in this shell.",
                            );
                        });
                        ui.end_row();
                    }
                };
            }
            keybind_row!(copy, "Copy", "copy");
            keybind_row!(paste, "Paste", "paste");
            keybind_row!(new_tab, "New tab", "new tab");
            keybind_row!(close_tab, "Close tab", "close tab");
            keybind_row!(next_tab, "Next tab", "next tab");
            keybind_row!(split_right, "Split right", "split right");
            keybind_row!(split_down, "Split down", "split down");
            keybind_row!(search, "Search", "search");
            keybind_row!(command_palette, "Command palette", "command palette");
            keybind_row!(increase_font, "Increase font", "increase font");
            keybind_row!(decrease_font, "Decrease font", "decrease font");
        });
        // F5-1: surface keybinding conflicts + blank bindings inline. The combos
        // are free-text, so two actions can collide on one combo (only one wins)
        // or a binding can be left empty (the action becomes unreachable) — both
        // silently. validate() makes that explicit right under the editor instead
        // of the user wondering why a shortcut "does nothing".
        for issue in config.keybindings.validate() {
            ui.colored_label(
                egui::Color32::from_rgb(0xff, 0xb0, 0x00),
                format!("\u{26a0} {}", issue.message()),
            );
        }
        group_gap(ui);
    }

    // --------------------------------------------------------------------- Config
    if section_visible(
        sel,
        q,
        "Config",
        &[
            "config", "file", "folder", "open", "reveal", "path", "edit", "toml",
        ],
    ) {
        ui.heading("Config");
        if let Some(path) = c0pl4nd_core::Config::default_path() {
            help(
                ui,
                "Settings are saved to a single TOML file. Open its folder to back \
                 it up or hand-edit it.",
            );
            ui.label(
                egui::RichText::new(path.display().to_string())
                    .weak()
                    .small()
                    .monospace(),
            );
            ui.horizontal(|ui| {
                if ui.button("Open config folder").clicked() {
                    if let Some(dir) = path.parent() {
                        // Create the dir if it does not exist yet (zero-config
                        // start) so "reveal" always lands somewhere real.
                        let _ = std::fs::create_dir_all(dir);
                        reveal_in_file_manager(dir);
                    }
                }
                // Only offer "open file" when it actually exists — opening a
                // non-existent path just fails silently.
                if path.exists() && ui.button("Open config file").clicked() {
                    reveal_in_file_manager(&path);
                }
            });
        } else {
            help(ui, "No config path is available on this platform.");
        }
        group_gap(ui);
    }

    // --------------------------------------------------------------------- Updates
    if section_visible(
        sel,
        q,
        "Updates",
        &[
            "update", "mode", "off", "notify", "manual", "auto", "check", "interval", "channel",
            "stable", "beta", "nightly", "install", "download", "releases",
        ],
    ) {
        // NOTE: the prior per-page `set_max_width(min(480.0))` band-aid is GONE.
        // The real root cause (a page demanding more width than the window) is
        // now fixed once, at the top of the window's content closure, by clamping
        // the whole content `Ui` to the window's inner width — so EVERY page
        // (this one included) is bounded to the same width, and the long help
        // line below wraps via `help`'s `.wrap()` instead of widening the page.
        // Clamping THIS page narrower than its siblings would make it the one
        // odd-width page — exactly the drift we are removing.
        ui.heading("Updates");
        ui.label(
            egui::RichText::new(format!(
                "You are running v{}.",
                update_engine::updater::current_version()
            ))
            .weak()
            .small(),
        );
        help(
            ui,
            "Local-first: a check reads only the public GitHub Releases API and \
             sends no identifiers. off and manual never touch the network on \
             their own; notify and auto check once per launch.",
        );
        grid("updates_grid").show(ui, |ui| {
            // ---- Mode: off / notify / manual / auto ----
            // off    = never check, never touch the network
            // notify = check on launch (when due), passive toast if newer
            // manual = check only when the button below is pressed (default)
            // auto   = check on launch (when due), download + apply when found
            if row_visible(q, "update mode off notify manual auto network") {
                let modes = [
                    (UpdateMode::Off, "off"),
                    (UpdateMode::Notify, "notify"),
                    (UpdateMode::Manual, "manual"),
                    (UpdateMode::Auto, "auto"),
                ];
                ui.label("Mode").on_hover_text(
                    "When C0PL4ND checks for updates: off (never), manual (only when \
                     you press Check for updates), notify (check once per launch, show \
                     a notice if newer), auto (check once per launch, download + install \
                     a verified update). A check reads only the public GitHub Releases \
                     API and sends no identifiers.",
                );
                egui::ComboBox::from_id_salt("c0pl4nd-update-mode")
                    .selected_text(
                        modes
                            .iter()
                            .find(|(m, _)| *m == config.update.mode)
                            .map(|(_, s)| *s)
                            .unwrap_or("manual"),
                    )
                    .show_ui(ui, |ui| {
                        for (m, label) in modes {
                            changed |= ui
                                .selectable_value(&mut config.update.mode, m, label)
                                .changed();
                        }
                    });
                changed |= reset_to_default(ui, &mut config.update.mode, &def.update.mode);
                ui.end_row();
            }

            // ---- Check interval (hours) — only relevant for notify/auto ----
            if row_visible(q, "check interval hours") {
                let on_launch = matches!(config.update.mode, UpdateMode::Notify | UpdateMode::Auto);
                ui.add_enabled_ui(on_launch, |ui| {
                    ui.label("Check interval (hours)").on_hover_text(
                        "How often, in hours, an on-launch check (notify/auto) is due \
                         (1–168). Ignored for off and manual.",
                    );
                });
                ui.add_enabled_ui(on_launch, |ui| {
                    changed |= ui
                        .add(egui::Slider::new(
                            &mut config.update.check_interval_hours,
                            1..=168,
                        ))
                        .changed();
                });
                changed |= reset_to_default(
                    ui,
                    &mut config.update.check_interval_hours,
                    &def.update.check_interval_hours,
                );
                ui.end_row();
            }

            // ---- Release channel ----
            if row_visible(q, "channel release stable beta nightly") {
                let networked = config.update.mode != UpdateMode::Off;
                ui.add_enabled_ui(networked, |ui| {
                    ui.label("Release channel")
                        .on_hover_text("Which release line update checks follow.");
                });
                ui.add_enabled_ui(networked, |ui| {
                    egui::ComboBox::from_id_salt("c0pl4nd-update-channel")
                        .selected_text(config.update.channel.clone())
                        .show_ui(ui, |ui| {
                            for chan in UPDATE_CHANNELS {
                                changed |= ui
                                    .selectable_value(
                                        &mut config.update.channel,
                                        (*chan).to_string(),
                                        *chan,
                                    )
                                    .changed();
                            }
                        });
                });
                changed |= reset_to_default(ui, &mut config.update.channel, &def.update.channel);
                ui.end_row();
            }
        });

        // ---- Check for updates + inline status + action buttons ----
        if row_visible(q, "check for updates now install download update") {
            ui.add_space(6.0);
            // The check / update buttons NEVER open a browser — they drive the
            // in-app updater state machine (download + verify + apply in place).
            let networked = config.update.mode != UpdateMode::Off;
            ui.horizontal_wrapped(|ui| {
                let busy = updater.lock().map(|u| u.is_busy()).unwrap_or(false);
                if ui
                    .add_enabled(networked && !busy, egui::Button::new("Check for updates"))
                    .on_hover_text(if networked {
                        "Ask the public GitHub Releases API whether a newer version \
                         exists. No identifiers are sent. Stays in-app — no browser."
                    } else {
                        "Set Mode to manual/notify/auto to enable update checks."
                    })
                    .clicked()
                {
                    // The configured Mode decides what a found update does: in
                    // `auto` it downloads + installs without a further click; in
                    // `notify`/`manual` it surfaces the inline "Download & install"
                    // button. Pressing the button always performs the check now.
                    let kind = match config.update.mode {
                        UpdateMode::Auto => LaunchKind::Auto,
                        UpdateMode::Notify => LaunchKind::Notify,
                        UpdateMode::Off | UpdateMode::Manual => LaunchKind::Manual,
                    };
                    if let Ok(mut u) = updater.lock() {
                        u.start_check(ui.ctx(), kind);
                    }
                }
                render_update_status(ui, updater);
            });
            ui.add_space(4.0);
            // The releases LINK is the ONE deliberate browser hand-off (changelog
            // / manual download); the check + update buttons above never browse.
            if ui
                .link("View all releases on GitHub")
                .on_hover_text("Open the C0PL4ND releases page in your browser.")
                .clicked()
            {
                ui.ctx().open_url(egui::OpenUrl::new_tab(format!(
                    "https://github.com/{}/{}/releases",
                    update_engine::UPDATE_OWNER,
                    update_engine::UPDATE_REPO
                )));
            }
        }
        group_gap(ui);
    }

    // ---- Privacy ----
    if section_visible(
        sel,
        q,
        "Privacy",
        &["history", "incognito", "clear", "secret"],
    ) {
        ui.heading("Privacy");
        help(
            ui,
            "C0PL4ND is local-first: no telemetry, no accounts. The only network \
             connection is the opt-in update check. Command history is kept in \
             memory only (never written to disk).",
        );

        // ---- Command history (grid-aligned, matching every other section) ----
        group(
            ui,
            "Command history",
            "Kept in memory only — never written to disk.",
        );
        grid("privacy_history").show(ui, |ui| {
            if row_visible(q, "record command history capture") {
                ui.label("Record history").on_hover_text(
                    "Capture typed commands for the Ctrl+Shift+P palette + the \
                     history sidebar.",
                );
                changed |= ui
                    .checkbox(
                        &mut config.history_capture_enabled,
                        "Capture typed commands",
                    )
                    .on_hover_text(
                        "Passwords typed at prompts are never captured (they are not \
                         echoed); inline secrets like --password=… / API_KEY=… are \
                         redacted. Turn off for a no-history posture.",
                    )
                    .changed();
                changed |= reset_to_default(
                    ui,
                    &mut config.history_capture_enabled,
                    &def.history_capture_enabled,
                );
                ui.end_row();
            }

            if row_visible(q, "incognito secret session no history") {
                // Incognito is RUNTIME state (never persisted) owned by the host;
                // reflect it and report a toggle back via `set_incognito`.
                ui.label("Incognito").on_hover_text(
                    "Stop recording history for THIS session; resets off next launch.",
                );
                let mut inc = incognito;
                if ui
                    .checkbox(&mut inc, "No history this session")
                    .on_hover_text(
                        "Stop recording command history for THIS session and clear \
                         what is already recorded. Resets to off on the next launch.",
                    )
                    .changed()
                {
                    *set_incognito = Some(inc);
                }
                ui.label(""); // runtime state — no ↺ revert column
                ui.end_row();
            }
        });

        // ---- Clear data (grid-aligned action rows) ----
        let status_id = egui::Id::new("c0pl4nd_clear_ui_state_status");
        group(
            ui,
            "Clear data",
            "One-click erase of in-memory and older on-disk state.",
        );
        grid("privacy_clear").show(ui, |ui| {
            if row_visible(q, "clear command history erase now") {
                ui.label("Command history");
                if ui
                    .button("Clear now")
                    .on_hover_text("Erase (zeroize) every recorded command immediately.")
                    .clicked()
                {
                    *clear_history = true;
                }
                ui.label(""); // action row — no ↺ column
                ui.end_row();
            }

            if row_visible(q, "clear saved window ui state") {
                // Delete eframe's persisted `app.ron` (window/UI state). Even
                // though C0PL4ND no longer persists egui Memory (privacy F1 —
                // `C0pl4ndApp::persist_egui_memory` returns false), a file written
                // by an OLDER build may still hold typed find/palette undo history,
                // so give the user an explicit one-click erase. Window geometry is
                // unaffected: it lives in the config TOML, not app.ron.
                ui.label("Window/UI state");
                ui.vertical(|ui| {
                    if ui
                        .button("Clear saved state")
                        .on_hover_text(
                            "Delete the on-disk app.ron that older builds used to \
                             persist window/UI state (and, in those builds, typed \
                             find/palette undo history). Your window size/position \
                             are kept (stored separately).",
                        )
                        .clicked()
                    {
                        let msg = clear_saved_ui_state();
                        ui.ctx().data_mut(|d| d.insert_temp(status_id, msg));
                    }
                    if let Some(msg) = ui.ctx().data(|d| d.get_temp::<String>(status_id)) {
                        ui.add(egui::Label::new(egui::RichText::new(msg).weak().small()).wrap());
                    }
                });
                ui.label(""); // action row — no ↺ column
                ui.end_row();
            }
        });

        // ---- W1TN3SS opt-in crash/error/issue reporting (default OFF) ----
        group(
            ui,
            "Report a crash or issue",
            "Opt-in and OFF by default — nothing is sent without your per-report \
             consent, and every report is shown to you (editable) before it leaves.",
        );
        grid("privacy_reporting").show(ui, |ui| {
            // Equal-weight 3-way selectors (Off / Ask each time / Always) — no
            // pre-ticked default-on path; the Off radio is first + selected by
            // default. Mirrors the proven consent shape.
            if row_visible(q, "crash reports") {
                ui.label("Crash reports");
                changed |= reporting_mode_selector(
                    ui,
                    "crash_reports_mode",
                    &mut config.reporting.streams.crash_reports,
                );
                ui.label(""); // selector carries its own state — no ↺ column
                ui.end_row();
            }
            if row_visible(q, "manual issue reports") {
                ui.label("Manual issues");
                changed |= reporting_mode_selector(
                    ui,
                    "manual_issues_mode",
                    &mut config.reporting.streams.manual_issues,
                );
                ui.label("");
                ui.end_row();
            }
        });

        group_gap(ui);
    }

    changed
}

/// Retrieve (or lazily create) the shared in-app updater held across frames in
/// `ctx` temp-data. Wrapped in `Arc<Mutex<_>>` because egui temp-data requires
/// `Clone + Send + Sync + 'static` and the `Updater` owns a non-Clone mpsc
/// `Receiver`. The `Arc` clone is cheap; one `Updater` instance persists for the
/// app's lifetime under this id.
fn get_updater(ctx: &egui::Context) -> Arc<Mutex<Updater>> {
    let id = egui::Id::new("c0pl4nd_in_app_updater");
    ctx.data_mut(|d| {
        d.get_temp::<Arc<Mutex<Updater>>>(id).unwrap_or_else(|| {
            let u = Arc::new(Mutex::new(Updater::default()));
            d.insert_temp(id, u.clone());
            u
        })
    })
}

/// Render the inline update status + action buttons next to the "Check for
/// updates" button, driven by the [`UpdateState`] machine. Mutating calls
/// (start download, apply, recheck) are deferred past the immutable state
/// borrow so the borrow checker is satisfied. The buttons here NEVER open a
/// browser — they download + verify + apply in place.
fn render_update_status(ui: &mut egui::Ui, updater: &Arc<Mutex<Updater>>) {
    enum Act {
        // Boxed: `ReleaseInfo` is large (it carries the version, several URLs,
        // and the pinned manifest digest), so boxing keeps the enum small and
        // satisfies `clippy::large_enum_variant`.
        Download(Box<update_engine::net::ReleaseInfo>),
        Apply,
        Recheck,
    }
    let mut act: Option<Act> = None;

    // Snapshot the state under a short-lived lock so the render closure does not
    // hold the lock across the deferred mutating calls below.
    let state = match updater.lock() {
        Ok(u) => u.state.clone(),
        Err(_) => return,
    };

    match &state {
        UpdateState::Idle => {}
        UpdateState::Checking => {
            ui.spinner();
            ui.label("Checking…");
        }
        UpdateState::UpToDate => {
            ui.label(
                egui::RichText::new(format!(
                    "You're on the latest version (v{}).",
                    update_engine::updater::current_version()
                ))
                .weak(),
            );
        }
        UpdateState::Available(info) => {
            ui.label(format!("v{} is available.", info.version));
            if ui
                .button("Download & install")
                .on_hover_text(
                    "Download the verified release, check its SHA-256 + signature, and \
                     stage it for install. Stays in-app — no browser.",
                )
                .clicked()
            {
                act = Some(Act::Download(Box::new(info.clone())));
            }
        }
        UpdateState::Downloading { received, total } => {
            let frac = if *total > 0 {
                *received as f32 / *total as f32
            } else {
                0.0
            };
            ui.add(
                egui::ProgressBar::new(frac)
                    .show_percentage()
                    .desired_width(150.0),
            );
        }
        UpdateState::ReadyToApply { version, .. } => {
            ui.label(format!("v{version} downloaded + verified."));
            if ui
                .button("Restart to finish update")
                .on_hover_text("Replace the running C0PL4ND with the new version and relaunch.")
                .clicked()
            {
                act = Some(Act::Apply);
            }
        }
        UpdateState::Applied { version } => {
            ui.label(format!("Updated to v{version} — restarting…"));
        }
        UpdateState::Failed(e) => {
            let err = ui.visuals().error_fg_color;
            ui.colored_label(err, crate::user_error::update_failed_user_copy(e));
            if ui.button("Retry").clicked() {
                act = Some(Act::Recheck);
            }
        }
    }

    if let Some(act) = act {
        if let Ok(mut u) = updater.lock() {
            match act {
                Act::Download(info) => u.start_download(ui.ctx(), *info),
                Act::Apply => u.apply_and_restart(ui.ctx()),
                Act::Recheck => u.start_check(ui.ctx(), LaunchKind::Manual),
            }
        }
    }
}

/// Human label for a cursor style (used by the combo's selected-text + items).
fn cursor_style_label(style: CursorStyle) -> &'static str {
    match style {
        CursorStyle::Block => "block",
        CursorStyle::Bar => "bar",
        CursorStyle::Underline => "underline",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_themes_include_the_default() {
        assert!(
            BUILTIN_THEMES.contains(&Config::default().theme.as_str()),
            "the default theme stem must be one of the offered built-ins"
        );
    }

    #[test]
    fn app_ron_path_targets_the_eframe_app_id_file() {
        // When a platform storage dir is resolvable, the helper must point at the
        // `app.ron` leaf inside the `com.itashacorp.c0pl4nd` app-id folder — the
        // exact file eframe writes (and the F1 leak target the Clear button
        // deletes). We assert the two load-bearing components rather than the
        // full platform-specific path so the test is OS-portable.
        match app_ron_path() {
            Some(p) => {
                assert_eq!(
                    p.file_name().and_then(|f| f.to_str()),
                    Some("app.ron"),
                    "must resolve the app.ron leaf eframe persists state into"
                );
                let s = p.to_string_lossy().replace('\\', "/");
                assert!(
                    s.contains(EFRAME_APP_ID),
                    "path must live under the eframe app-id folder \
                     '{EFRAME_APP_ID}'; got {s}"
                );
            }
            // No platform storage dir (e.g. the relevant env var is unset on this
            // runner) — eframe would not persist either, so there is nothing to
            // resolve. Mirror that condition by asserting the app-id constant
            // matches the `with_app_id` in egui_main.rs.
            None => assert_eq!(EFRAME_APP_ID, "com.itashacorp.c0pl4nd"),
        }
    }

    #[test]
    fn section_visible_matches_selected_when_not_searching() {
        assert!(section_visible("Font", "", "Font", &["size"]));
        assert!(!section_visible("Font", "", "Cursor", &["blink"]));
    }

    #[test]
    fn section_visible_is_cross_category_when_searching() {
        // A query that matches a Cursor label reveals the Cursor section even
        // while "Font" is the selected tab (cross-category search).
        assert!(section_visible("Font", "blink", "Cursor", &["blink"]));
    }

    #[test]
    fn row_visible_filters_by_query() {
        assert!(row_visible("", "anything"));
        assert!(row_visible("opa", "Window opacity"));
        assert!(!row_visible("zzz", "Window opacity"));
    }

    #[test]
    fn cursor_style_label_round_trips() {
        assert_eq!(cursor_style_label(CursorStyle::Block), "block");
        assert_eq!(cursor_style_label(CursorStyle::Bar), "bar");
        assert_eq!(cursor_style_label(CursorStyle::Underline), "underline");
    }

    #[test]
    fn updates_is_a_navigable_category() {
        // The new Updates section must be reachable from the left nav.
        assert!(
            CATEGORIES.contains(&"Updates"),
            "Updates must be a left-nav category so its rows are reachable"
        );
    }

    #[test]
    fn motion_and_fonts_are_navigable_categories() {
        // The renamed "Fonts" tab and the new "Motion" tab (which now owns the
        // CRT / chromatic / animation controls) must both be reachable from the
        // left nav; the old "Font" name must be gone.
        assert!(
            CATEGORIES.contains(&"Motion"),
            "Motion must be a left-nav category so its effect rows are reachable"
        );
        assert!(
            CATEGORIES.contains(&"Fonts"),
            "the Font section was renamed to Fonts for SCR1B3 parity"
        );
        assert!(
            !CATEGORIES.contains(&"Font"),
            "the old singular 'Font' category name must not linger"
        );
    }

    #[test]
    fn motion_section_is_cross_category_searchable() {
        // The controls that MOVED from Appearance to Motion must be reachable by
        // their FULL names from any tab via the PRODUCTION label set — and must no
        // longer resolve to Appearance (whose stale CRT labels were removed). This
        // asserts against the real `MOTION_SEARCH_LABELS`/`APPEARANCE_SEARCH_LABELS`
        // consts, so it would fail if a moved label were dropped or left behind.
        for term in [
            "crt scanlines",
            "scanline darkness",
            "chromatic aberration",
            "cursor trail",
        ] {
            assert!(
                section_visible("Appearance", term, "Motion", MOTION_SEARCH_LABELS),
                "'{term}' must reveal the Motion section that now owns it"
            );
            assert!(
                !section_visible("Motion", term, "Appearance", APPEARANCE_SEARCH_LABELS),
                "'{term}' must NOT resurrect a phantom empty Appearance section"
            );
        }
        // Every Motion search label must correspond to a Motion row (its canonical
        // `row_visible` label), so a query can never reveal the section with no
        // matching row. The row labels ARE the search labels by construction.
        assert!(MOTION_SEARCH_LABELS.contains(&"chromatic aberration"));
        assert!(MOTION_SEARCH_LABELS.contains(&"cursor trail"));
    }

    #[test]
    fn update_channels_include_the_default_channel() {
        // The channel combo must offer the default channel, or selecting it back
        // would be impossible.
        assert!(
            UPDATE_CHANNELS.contains(&Config::default().update.channel.as_str()),
            "the default update channel must be one of the offered choices"
        );
    }

    #[test]
    fn updates_section_is_cross_category_searchable() {
        // Searching for an Updates label reveals the section even when another
        // tab is selected (proves the section's labels are wired into search).
        assert!(section_visible(
            "Appearance",
            "channel",
            "Updates",
            &["check on launch", "channel"]
        ));
    }
}
