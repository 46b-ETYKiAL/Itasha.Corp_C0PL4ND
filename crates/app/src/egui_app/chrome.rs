//! The C0PL4ND chrome: a frameless titlebar (two-tone wordmark + tab strip +
//! caption buttons), a settings gear, and a bottom status bar. Window controls
//! use `egui::ViewportCommand` (no Win32) per recon dossier §3.2.
//!
//! The titlebar mirrors the sibling SCR1B3 editor's frameless titlebar for a
//! same-product-family read: a `horizontal_centered` row with the left content
//! (wordmark + tabs + split buttons) first, then the caption cluster
//! (settings/minimize/maximize/close) pinned to the window's right edge. The
//! cluster is placed at absolute rects anchored to `ctx().screen_rect()` — the
//! only reliable bounded right edge in this non-justified row (the ui's own
//! `clip_rect()`/`max_rect()` right edge is unbounded in the render frame). The
//! buttons use Phosphor glyphs via `ui.button` so they self-size and never fall
//! back to tofu.

use egui::{RichText, Sense};
use egui_phosphor::thin as icon;

use super::theme::brand;
use super::C0pl4ndApp;

/// Outcome of one chrome frame — the actions the user requested via the chrome
/// widgets. The host applies them after the panel closure returns so that the
/// grid/tree mutation does not happen mid-borrow.
#[derive(Debug, Default, Clone)]
pub struct ChromeActions {
    /// User clicked a tab; switch focus to this pane.
    pub focus_tab: Option<super::grid::PaneId>,
    /// User clicked a tab's close (×); close this pane.
    pub close_tab: Option<super::grid::PaneId>,
    /// User clicked a tab's pin; toggle this pane's pinned state.
    pub pin_tab: Option<super::grid::PaneId>,
    /// User clicked the `+` / split-right button.
    pub split_right: bool,
    /// User clicked the split-down button.
    pub split_down: bool,
    /// User toggled the settings window.
    pub toggle_settings: bool,
    /// User clicked a caption button (minimize / maximize / close). Routed
    /// through the action struct (instead of sending the `ViewportCommand`
    /// inline) so `frame_tick` is the single place that issues the real OS
    /// command AND records it for the interaction tests to observe — a click on
    /// the real button thus has an assertable outcome without a window.
    pub window_cmd: Option<super::WindowCmd>,
}

impl C0pl4ndApp {
    /// Paint the titlebar (wordmark + tab strip + caption controls). Returns the
    /// actions the host should apply this frame.
    pub(super) fn titlebar_and_tabs(&self, ui: &mut egui::Ui) -> ChromeActions {
        let mut actions = ChromeActions::default();
        // One left→right row: the LEFT content (wordmark + tabs + split buttons)
        // flows normally; the caption cluster is then placed at absolute rects
        // pinned to the window's right edge (see the cluster block below).
        ui.horizontal_centered(|ui| {
            // two-tone C0PL4ND wordmark + drag/double-click caption region.
            let mut job = egui::text::LayoutJob::default();
            let fmt = |color| egui::text::TextFormat {
                color,
                font_id: egui::FontId::proportional(16.0),
                ..Default::default()
            };
            job.append("C0PL", 0.0, fmt(brand::PURPLE));
            job.append("4ND", 0.0, fmt(brand::GREEN));
            let title_resp = ui.add(egui::Label::new(job).sense(Sense::click_and_drag()));
            if title_resp.drag_started_by(egui::PointerButton::Primary) {
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
            }
            if title_resp.double_clicked() {
                actions.window_cmd = Some(super::WindowCmd::ToggleMaximize);
            }

            ui.separator();

            // Tab strip: one tab per pane, SCR1B3-style — each tab is
            // [title] [pin] [×]. Pinned tabs sort first and carry a violet pin;
            // their × is hidden (unpin to close) so they can't be shut by
            // accident. Clicking the title focuses the pane; × closes it.
            let mut tabs = self.pane_titles();
            // Stable sort: pinned first, original visual order preserved within
            // each group (`sort_by_key` is stable).
            tabs.sort_by_key(|(pid, _)| !self.pinned.contains(pid));
            for (pane_id, title) in tabs {
                let selected = pane_id == self.focused_pane;
                let is_pinned = self.pinned.contains(&pane_id);
                // Per-tab accessible labels: the pin/× are glyph buttons, so each
                // would otherwise expose the same name across tabs (ambiguous for
                // screen readers AND `get_by_label` tests). Make them unique +
                // descriptive per tab.
                let pin_label = format!("{} {title}", if is_pinned { "unpin" } else { "pin" });
                let close_label = format!("close {title}");
                ui.scope(|ui| {
                    // Tight spacing INSIDE a tab so title/pin/× read as one unit.
                    ui.spacing_mut().item_spacing.x = 3.0;
                    let label = RichText::new(&title).color(if selected {
                        brand::GREEN
                    } else {
                        brand::FG
                    });
                    if ui.selectable_label(selected, label).clicked() {
                        actions.focus_tab = Some(pane_id);
                    }
                    let pin_col = if is_pinned {
                        brand::PURPLE
                    } else {
                        brand::MUTED
                    };
                    let pin = ui
                        .add(
                            egui::Button::new(
                                RichText::new(icon::PUSH_PIN).size(13.0).color(pin_col),
                            )
                            .frame(false),
                        )
                        .on_hover_text(&pin_label);
                    pin.widget_info(|| {
                        egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &pin_label)
                    });
                    if pin.clicked() {
                        actions.pin_tab = Some(pane_id);
                    }
                    if !is_pinned {
                        let close = ui
                            .add(
                                egui::Button::new(
                                    RichText::new(icon::X).size(13.0).color(brand::MUTED),
                                )
                                .frame(false),
                            )
                            .on_hover_text(&close_label);
                        close.widget_info(|| {
                            egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &close_label)
                        });
                        if close.clicked() {
                            actions.close_tab = Some(pane_id);
                        }
                    }
                });
                ui.separator();
            }

            // new-pane (split-right) / split-down buttons.
            if ui
                .button(RichText::new(icon::COLUMNS).size(16.0))
                .on_hover_text("split right (new pane)")
                .clicked()
            {
                actions.split_right = true;
            }
            if ui
                .button(RichText::new(icon::ROWS).size(16.0))
                .on_hover_text("split down")
                .clicked()
            {
                actions.split_down = true;
            }

            // ---- right-pinned caption cluster ----
            // Placed at ABSOLUTE rects via `ui.put`. Every layout-flow attempt
            // (`right_to_left`, `Sides`, `allocate_ui_with_layout`, an
            // `available_width()` spacer) AND every right-edge taken from the
            // ui's own rects left the right side EMPTY: in this non-justified
            // `horizontal_centered` the ui's `clip_rect()`/`max_rect()` right edge
            // is UNBOUNDED in the render frame (`rect_filled(clip_rect)` paints
            // the visible width, but `clip_rect().right()` is ~f32::MAX), so a
            // right-anchored x landed off-screen. The window's `screen_rect()` is
            // the only reliable bounded right edge; `min_rect()` (the content laid
            // out so far) gives the true row Y. Reads left→right ⚙ — ▢ ✕.
            let screen = ui.ctx().content_rect();
            let row = ui.min_rect();
            let bw = 42.0_f32;
            let bh = 28.0_f32;
            let cy = row.center().y;
            let right_edge = screen.right() - 8.0; // window edge minus panel inset
            let specs: [(&str, &str, super::WindowCmd, bool); 4] = [
                (icon::X, "close", super::WindowCmd::Close, false),
                (
                    icon::SQUARE,
                    "maximize",
                    super::WindowCmd::ToggleMaximize,
                    false,
                ),
                (icon::MINUS, "minimize", super::WindowCmd::Minimize, false),
                (icon::GEAR, "settings", super::WindowCmd::Close, true), // gear → settings
            ];
            let mut right_x = right_edge;
            for (glyph, hover, cmd, is_gear) in specs {
                let rect = egui::Rect::from_min_max(
                    egui::pos2(right_x - bw, cy - bh / 2.0),
                    egui::pos2(right_x, cy + bh / 2.0),
                );
                let resp = ui
                    .put(
                        rect,
                        egui::Button::new(RichText::new(glyph).size(16.0).color(brand::MUTED)),
                    )
                    .on_hover_text(hover);
                // Accessible label (for screen readers AND the `get_by_label`
                // interaction tests) — the visible content is a glyph, so the
                // semantic name must be set explicitly.
                resp.widget_info(|| {
                    egui::WidgetInfo::labeled(egui::WidgetType::Button, true, hover)
                });
                if resp.clicked() {
                    if is_gear {
                        actions.toggle_settings = true;
                    } else {
                        actions.window_cmd = Some(cmd);
                    }
                }
                right_x -= bw + 2.0;
            }
        });
        actions
    }

    /// Paint the bottom status bar — pane count + a brand-tinted hint.
    pub(super) fn status_bar(&self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let panes = super::grid::count_panes(&self.grid_tree);
            ui.label(
                RichText::new(format!("{panes}/{} panes", super::grid::MAX_PANES))
                    .color(brand::GREEN),
            );
            ui.separator();
            ui.label(
                RichText::new("C0PL4ND — local-first terminal")
                    .color(brand::FG)
                    .weak(),
            );
            if let Some(toast) = &self.toast {
                ui.separator();
                ui.label(RichText::new(toast).color(brand::PURPLE));
            }
        });
    }
}
