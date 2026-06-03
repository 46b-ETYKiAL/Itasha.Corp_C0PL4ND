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

use eframe::egui;

use c0pl4nd_core::config::{CursorStyle, WindowMode};
use c0pl4nd_core::Config;

use super::theme::ChromeColors;

/// Left-nav categories, in display order. Each maps to a section rendered by
/// [`render_sections`].
const CATEGORIES: &[&str] = &[
    "Appearance",
    "Font",
    "Cursor",
    "Terminal",
    "Window",
    "Keybindings",
];

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

/// A dim one-line helper label under a heading (SCR1B3's `weak().small()` idiom).
fn help(ui: &mut egui::Ui, text: &str) {
    ui.label(egui::RichText::new(text).weak().small());
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

    // Center the window the first time it opens via a one-time default position.
    // `.anchor()` is deliberately NOT used: an anchored egui window is re-pinned
    // to its anchor every frame and is therefore IMMOVABLE — that was the root
    // cause of the "settings can't be dragged" report. `.default_pos` places it
    // once, then the title bar drags freely.
    let win_size = egui::vec2(720.0, 560.0);
    let default_pos = ctx.content_rect().center() - win_size * 0.5;

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
        .collapsible(false)
        .resizable(false)
        .movable(true)
        .fixed_size(win_size)
        .default_pos(default_pos)
        .frame(egui::Frame::window(&ctx.global_style()).fill(colors.panel))
        .show(ctx, |ui| {
            // In-content header: title + an unmissable Close ✕. The egui
            // title-bar ✕ can read as low-contrast against the dark custom
            // frame (the "can't close it" report), so this is a clear,
            // always-visible dismiss. Fixed-height — it does not eat the scroll
            // area's fill below.
            ui.horizontal(|ui| {
                ui.heading(egui::RichText::new("Settings").color(colors.accent));
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
                    ui.horizontal(|ui| {
                        ui.label(egui_phosphor::thin::MAGNIFYING_GLASS);
                        ui.add(
                            egui::TextEdit::singleline(&mut query)
                                .hint_text("search settings")
                                .desired_width(f32::INFINITY),
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
                            changed |= render_sections(ui, config, sel, &q);
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
fn render_sections(ui: &mut egui::Ui, config: &mut Config, sel: &str, q: &str) -> bool {
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

            if row_visible(q, "tint color overlay wash") {
                ui.add_enabled_ui(config.transparency_enabled, |ui| {
                    ui.label("Tint color")
                        .on_hover_text("A #RRGGBB color washed over the window.");
                    ui.horizontal(|ui| {
                        changed |= ui
                            .add(
                                egui::TextEdit::singleline(&mut config.tint)
                                    .hint_text("#121212")
                                    .desired_width(96.0),
                            )
                            .changed();
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
                    .toggle_value(&mut config.effects.crt_scanlines, "Scanline overlay")
                    .on_hover_text("Auto-disabled under reduced-motion / battery-save.")
                    .changed();
                changed |= reset_to_default(
                    ui,
                    &mut config.effects.crt_scanlines,
                    &def.effects.crt_scanlines,
                );
                ui.end_row();
            }

            if row_visible(q, "chromatic aberration") {
                ui.label("Chromatic aberration");
                changed |= ui
                    .add(
                        egui::Slider::new(&mut config.effects.chromatic_aberration, 0.0..=1.0)
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
        &["family", "size", "line height", "ligatures"],
    ) {
        ui.heading("Font");
        help(ui, "Typeface, size, and text shaping.");
        grid("font_grid").show(ui, |ui| {
            if row_visible(q, "family typeface") {
                ui.label("Family");
                changed |= ui
                    .add(
                        egui::TextEdit::singleline(&mut config.font.family)
                            .hint_text("Monaspace Neon")
                            .desired_width(200.0),
                    )
                    .changed();
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
                ui.label("Line height");
                changed |= ui
                    .add(egui::Slider::new(&mut config.font.line_height, 12.0..=48.0).suffix(" px"))
                    .changed();
                changed |=
                    reset_to_default(ui, &mut config.font.line_height, &def.font.line_height);
                ui.end_row();
            }

            if row_visible(q, "ligatures shaping") {
                ui.label("Ligatures");
                changed |= ui
                    .toggle_value(&mut config.ligatures, "Programming ligatures (->, !=)")
                    .on_hover_text("Advanced text shaping; may break strict monospace alignment.")
                    .changed();
                changed |= reset_to_default(ui, &mut config.ligatures, &def.ligatures);
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
                ui.label("Scrollback");
                changed |= ui
                    .add(
                        egui::Slider::new(&mut config.scrollback_lines, 100..=1_000_000)
                            .logarithmic(true)
                            .suffix(" lines"),
                    )
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
                changed |= ui
                    .toggle_value(&mut config.copy_on_select, "X11-style auto-copy")
                    .on_hover_text("Copy a mouse selection to the clipboard when the drag ends.")
                    .changed();
                changed |= reset_to_default(ui, &mut config.copy_on_select, &def.copy_on_select);
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
    if section_visible(sel, q, "Window", &["padding"]) {
        ui.heading("Window");
        help(
            ui,
            "Inner padding. Geometry (size/position) is remembered automatically.",
        );
        grid("window_grid").show(ui, |ui| {
            if row_visible(q, "padding inner margin") {
                ui.label("Padding");
                changed |= ui
                    .add(
                        egui::DragValue::new(&mut config.window.padding)
                            .range(0..=32)
                            .suffix(" px"),
                    )
                    .changed();
                changed |= reset_to_default(ui, &mut config.window.padding, &def.window.padding);
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

    changed
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
}
