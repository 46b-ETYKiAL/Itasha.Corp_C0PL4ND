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

use c0pl4nd_core::config::{CursorStyle, UpdateMode, WindowMode};
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
    "Font",
    "Cursor",
    "Terminal",
    "Window",
    "Keybindings",
    "Updates",
];

/// The release channels the Updates section offers. Mirrors the channels the
/// `c0pl4nd update` checker understands; a free choice list, not invented.
const UPDATE_CHANNELS: &[&str] = &["stable", "beta", "nightly"];

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

/// A dim helper label under a heading (SCR1B3's `weak().small()` idiom). WRAPS
/// to the available width — a long single-line help string (e.g. the Updates
/// page's) otherwise sets the content's min width and forced the whole settings
/// window WIDER on that page than the others (the reported per-page width drift).
fn help(ui: &mut egui::Ui, text: &str) {
    ui.add(egui::Label::new(egui::RichText::new(text).weak().small()).wrap());
    ui.add_space(2.0);
}

/// Render the settings window. `open` is toggled false when the user closes it
/// (via the egui Window's built-in ✕). Returns the [`Outcome`] for this frame so
/// the host can persist + re-apply the theme.
pub fn show(
    ctx: &egui::Context,
    config: &mut Config,
    open: &mut bool,
    colors: ChromeColors,
) -> Outcome {
    let mut changed = false;
    let mut keep_open = *open;

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
    // The window is now edge/corner RESIZABLE (#25): `.resizable(true)` +
    // `.default_size` (first-open size) + `.min_size` (a floor so it can't be
    // shrunk to uselessness). The stable Id is still derived from the "settings"
    // name (unchanged), so once eframe `persistence` lands the size will be
    // remembered automatically — that is a SEPARATE PR; we only make it
    // resizable here.
    let default_size = egui::vec2(720.0, 560.0);
    let min_size = egui::vec2(560.0, 420.0);
    let default_pos = ctx.content_rect().center() - default_size * 0.5;

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
    egui::Window::new("settings")
        // No egui title bar: it rendered a SECOND, redundant top bar (a centered
        // lowercase "settings") above the in-content header that carries the
        // "Settings" heading + the ✕ close button. The in-content header is the
        // single titlebar; dragging still works via egui's window-frame drag.
        .title_bar(false)
        .collapsible(false)
        // Edge/corner resizable (#25). `default_size` is the first-open size;
        // `min_size` is a sensible floor. The window keeps its stable Id (from
        // the "settings" name) so a future `persistence` PR remembers the size.
        .resizable(true)
        .default_size(default_size)
        .min_size(min_size)
        .movable(true)
        .default_pos(default_pos)
        .frame(egui::Frame::window(&ctx.global_style()).fill(colors.panel))
        .show(ctx, |ui| {
            // ---- Width discipline (#26) ----
            // egui's window auto-sizing measures content DESIRED width with an
            // effectively unbounded available width (~f32::MAX). Any child that
            // returns `available_width()` (e.g. a `horizontal` row) or
            // `f32::INFINITY` (the search box below) as its desired size would
            // therefore demand a near-infinite width and push the whole window
            // WIDER on the page that has it — and by a DIFFERENT amount per page
            // (the reported "every page is a different width + content runs past
            // the ✕" bug). The robust fix (proven on the sibling SCR1B3 editor)
            // is to clamp the content `Ui` to the window's current inner width up
            // front: no page can then demand more than that, so EVERY page
            // renders at the same width and content can never exceed the window
            // (so it can't draw past the ✕). When the user widens the window,
            // `available_width()` grows and every page uses the extra width
            // equally; overflow always goes to the vertical ScrollArea, never to
            // horizontal growth.
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
                    ui.set_width(168.0);
                    ui.add_space(4.0);
                    for cat in CATEGORIES {
                        ui.selectable_value(&mut category, (*cat).to_string(), *cat);
                        ui.add_space(2.0);
                    }
                });
                ui.separator();

                // ---- Searchable, internally-scrolling content pane ----
                ui.vertical(|ui| {
                    // Clamp the content pane to the width left after the fixed
                    // 168px nav + separator. This makes the pane width identical
                    // on every page (the grids below size to THIS width, not to
                    // their own desired width), and gives the search box a finite
                    // width to fill. Without this clamp the pane would size to the
                    // widest page's content and the `f32::INFINITY` search box
                    // would demand near-infinite width during measurement.
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
                            changed |= render_sections(ui, config, &updater, sel, &q);
                        });
                });
            });
        });

    if close_requested {
        keep_open = false;
    }

    ctx.data_mut(|d| {
        d.insert_temp(cat_id, category);
        d.insert_temp(q_id, query);
    });
    *open = keep_open;

    Outcome {
        changed,
        theme_changed: changed && config.theme != theme_before,
    }
}

/// Render every category section visible for the current selection / search
/// query. The single most impactful "un-cramp" change is setting
/// `item_spacing` to 8 px vertical (vs egui's default 3 px) at the top; section
/// gaps add a further 14 px of breathing room.
fn render_sections(
    ui: &mut egui::Ui,
    config: &mut Config,
    updater: &Arc<Mutex<Updater>>,
    sel: &str,
    q: &str,
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
            .min_col_width(150.0)
    };

    // ---------------------------------------------------------------- Appearance
    if section_visible(
        sel,
        q,
        "Appearance",
        &[
            "theme",
            "transparency",
            "mode",
            "opacity",
            "glass",
            "mica",
            "vibrancy",
            "tint",
            "acrylic",
            "scanlines",
            "scanline darkness",
            "chromatic aberration",
        ],
    ) {
        ui.heading("Appearance");
        help(ui, "Colors, transparency, and CRT post-effects.");
        grid("appearance_grid").show(ui, |ui| {
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
                    .toggle_value(
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

            if row_visible(q, "transparency mode glass mica vibrancy") {
                ui.add_enabled_ui(config.transparency_enabled, |ui| {
                    ui.label("Mode").on_hover_text(
                        "Opaque · Transparent (portable) · Glass/Mica/Vibrancy \
                         (OS blur — applies on restart).",
                    );
                    ui.horizontal(|ui| {
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
                        changed |= reset_to_default(ui, &mut config.window_mode, &def.window_mode);
                    });
                });
                ui.end_row();
            }

            if row_visible(q, "opacity transparency") {
                ui.add_enabled_ui(
                    config.transparency_enabled && config.window_mode.is_translucent(),
                    |ui| {
                        ui.label("Opacity").on_hover_text(
                            "Surface opacity for translucent modes — below 100% the \
                             desktop / blur shows through.",
                        );
                        changed |= ui
                            .add(
                                egui::Slider::new(&mut config.opacity, 0.30..=1.0)
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
                    },
                );
                ui.end_row();
            }

            if row_visible(q, "tint color overlay wash picker") {
                ui.add_enabled_ui(config.transparency_enabled, |ui| {
                    ui.label("Tint color")
                        .on_hover_text("Color washed over the window (strength below).");
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
                        changed |= reset_to_default(ui, &mut config.tint, &def.tint);
                    });
                });
                ui.end_row();
            }

            if row_visible(q, "tint strength wash overlay") {
                ui.add_enabled_ui(config.transparency_enabled, |ui| {
                    ui.label("Tint strength")
                        .on_hover_text("0% = no tint .. 100% = strong color wash.");
                    changed |= ui
                        .add(
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
                });
                ui.end_row();
            }

            if row_visible(q, "crt scanlines") {
                ui.label("CRT scanlines");
                changed |= ui
                    .toggle_value(&mut config.effects.crt_scanlines, "Animated scan lines")
                    .on_hover_text(
                        "Dark scan-line bands with a rolling refresh sweep. \
                         Auto-disabled under reduced-motion / battery-save.",
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
                let on = config.effects.crt_scanlines;
                changed |= ui
                    .add_enabled(
                        on,
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
                // Explicit ON/OFF checkbox (issue #28): the intensity slider alone
                // read as "broken" when it sat at 0. On first enable, default the
                // intensity to a visible value so the effect shows immediately.
                let was_enabled = config.effects.chromatic_aberration_enabled;
                if ui
                    .checkbox(
                        &mut config.effects.chromatic_aberration_enabled,
                        "RGB split",
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
                let on = config.effects.chromatic_aberration_enabled;
                changed |= ui
                    .add_enabled(
                        on,
                        egui::Slider::new(&mut config.effects.chromatic_aberration, 0.1..=1.5)
                            .text("intensity"),
                    )
                    .changed();
                changed |= reset_to_default(
                    ui,
                    &mut config.effects.chromatic_aberration,
                    &def.effects.chromatic_aberration,
                );
                ui.end_row();
            }
        });
        group_gap(ui);
    }

    // ---------------------------------------------------------------------- Font
    if section_visible(
        sel,
        q,
        "Font",
        &["family", "size", "line height", "ligatures", "fallback"],
    ) {
        ui.heading("Font");
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
                    ui.toggle_value(&mut config.ligatures, "Programming ligatures (->, !=)")
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
                    .toggle_value(&mut config.cursor.blink, "Blink the cursor")
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
        grid("terminal_grid").show(ui, |ui| {
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
                changed |= ui
                    .toggle_value(&mut config.startup_panel, "Show logo + system info")
                    .on_hover_text("A neofetch-style panel shown on launch.")
                    .changed();
                changed |= reset_to_default(ui, &mut config.startup_panel, &def.startup_panel);
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

            if row_visible(q, "copy on select clipboard") {
                ui.label("Copy on select");
                // DISABLED: this depends on MOUSE TEXT-SELECTION (drag to select
                // grid text), which the egui terminal surface does not implement
                // yet — there is no selection for an auto-copy to act on. Greyed
                // with an honest tooltip rather than a dead toggle. (When mouse
                // selection lands, re-enable this and wire it to the drag-end.)
                ui.add_enabled_ui(false, |ui| {
                    ui.toggle_value(&mut config.copy_on_select, "X11-style auto-copy")
                        .on_hover_text(
                            "Not available yet: needs mouse text-selection (drag to \
                             select), which this terminal does not support yet.",
                        );
                });
                ui.end_row();
            }

            if row_visible(q, "paste warn multiline newline security") {
                ui.label("Multi-line paste");
                changed |= ui
                    .toggle_value(
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
    if section_visible(sel, q, "Window", &["padding", "columns", "rows"]) {
        ui.heading("Window");
        help(
            ui,
            "Inner padding and the initial grid size. Live size/position is \
             remembered automatically.",
        );
        grid("window_grid").show(ui, |ui| {
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

            // Initial terminal grid width at launch. The live window size is
            // remembered separately (geometry persistence), so this is the
            // first-launch / no-saved-geometry size; it takes effect on restart.
            // Floor of 1 mirrors the core validator (cols/rows must be non-zero).
            if row_visible(q, "columns cols initial width grid size") {
                ui.label("Initial columns").on_hover_text(
                    "Terminal width (columns) used on first launch / when no window \
                     size is remembered. Applies on restart.",
                );
                changed |= ui
                    .add(
                        egui::DragValue::new(&mut config.window.cols)
                            .range(1..=500)
                            .suffix(" cols"),
                    )
                    .on_hover_text("Applies on the next launch.")
                    .changed();
                changed |= reset_to_default(ui, &mut config.window.cols, &def.window.cols);
                ui.end_row();
            }

            // Initial terminal grid height at launch (see cols above).
            if row_visible(q, "rows initial height grid size") {
                ui.label("Initial rows").on_hover_text(
                    "Terminal height (rows) used on first launch / when no window \
                     size is remembered. Applies on restart.",
                );
                changed |= ui
                    .add(
                        egui::DragValue::new(&mut config.window.rows)
                            .range(1..=300)
                            .suffix(" rows"),
                    )
                    .on_hover_text("Applies on the next launch.")
                    .changed();
                changed |= reset_to_default(ui, &mut config.window.rows, &def.window.rows);
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
            "Editable key combos. \"mod\" is Ctrl+Shift on Windows/Linux, Cmd on macOS.",
        );
        // Each row: action label + editable combo string + ↺. A `&mut String`
        // per field keeps every binding live-editable.
        grid("keybindings_grid").show(ui, |ui| {
            macro_rules! keybind_row {
                ($field:ident, $label:literal, $search:literal) => {
                    if row_visible(q, $search) {
                        ui.label($label);
                        changed |= ui
                            .add(
                                egui::TextEdit::singleline(&mut config.keybindings.$field)
                                    .desired_width(180.0)
                                    .font(egui::TextStyle::Monospace),
                            )
                            .changed();
                        changed |= reset_to_default(
                            ui,
                            &mut config.keybindings.$field,
                            &def.keybindings.$field,
                        );
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
        Download(update_engine::net::ReleaseInfo),
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
                act = Some(Act::Download(info.clone()));
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
            ui.colored_label(err, format!("Update failed: {e}"));
            if ui.button("Retry").clicked() {
                act = Some(Act::Recheck);
            }
        }
    }

    if let Some(act) = act {
        if let Ok(mut u) = updater.lock() {
            match act {
                Act::Download(info) => u.start_download(ui.ctx(), info),
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
