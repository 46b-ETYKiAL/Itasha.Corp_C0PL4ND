//! The C0PL4ND chrome: a frameless titlebar (two-tone wordmark + tab strip +
//! caption buttons), a settings gear, and a bottom status bar. Window controls
//! use `egui::ViewportCommand` (no Win32) per recon dossier §3.2.
//!
//! The titlebar layout mirrors the sibling SCR1B3 editor's frameless titlebar
//! (`scribe-app/src/app.rs`) for a same-product-family read: a
//! `horizontal_centered` row with the left content (wordmark + tabs + split
//! buttons) first, then a NESTED `right_to_left` layout that takes the remaining
//! width and pins the caption cluster FLUSH to the window's right edge. The
//! caption buttons are painter-drawn ([`caption_btn`]) — ported from SCR1B3's
//! `caption_btn` — so they never depend on icon-font glyph coverage and land at
//! a consistent 46×28 Windows-11 hit target.

use egui::{Align, Color32, Layout, RichText, Sense};
use egui_phosphor::thin as icon;

use super::theme::brand;
use super::C0pl4ndApp;

/// A frameless-titlebar caption glyph. Painter-drawn (ported from SCR1B3's
/// `CaptionIcon`) so the control never depends on icon-font coverage and reads
/// identically across the C0PL4ND / SCR1B3 product family.
#[derive(Clone, Copy)]
enum CaptionIcon {
    /// Minimize to the taskbar.
    Minimize,
    /// Maximize / restore toggle (single square — the app reads OS state to pick
    /// the right viewport command).
    Maximize,
    /// Settings gear (drawn as an outlined hex nut so it reads as "settings"
    /// without a font glyph).
    Gear,
    /// Close the window.
    Close,
}

/// Paint one titlebar caption button at SCR1B3's 46×28 Windows-11 caption hit
/// target with a hover wash (close gets the conventional red; the rest a soft
/// white). The icon is stroked by the painter so it never falls back to tofu.
/// Ported from `scribe-app/src/app.rs::caption_btn` for cross-app cohesion.
fn caption_btn(
    ui: &mut egui::Ui,
    icon: CaptionIcon,
    base: Color32,
    hover_fill: Color32,
) -> egui::Response {
    let size = egui::vec2(46.0, 28.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    // Accessible label so screen readers AND the semantic interaction tests
    // (`get_by_label`) can find the painter-drawn button — it has no text glyph.
    let label = match icon {
        CaptionIcon::Minimize => "minimize",
        CaptionIcon::Maximize => "maximize",
        CaptionIcon::Gear => "settings",
        CaptionIcon::Close => "close",
    };
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, label));
    let painter = ui.painter();
    if resp.hovered() {
        painter.rect_filled(rect, 2.0, hover_fill);
    }
    let col = if resp.hovered() { Color32::WHITE } else { base };
    let c = rect.center();
    let s = 4.5_f32;
    let stroke = egui::Stroke::new(1.4, col);
    match icon {
        CaptionIcon::Minimize => {
            painter.line_segment([egui::pos2(c.x - s, c.y), egui::pos2(c.x + s, c.y)], stroke);
        }
        CaptionIcon::Maximize => {
            painter.rect_stroke(
                egui::Rect::from_center_size(c, egui::vec2(2.0 * s, 2.0 * s)),
                1.0,
                stroke,
                egui::StrokeKind::Outside,
            );
        }
        CaptionIcon::Gear => {
            // A simple gear read: an outer ring with short radial teeth + a hub.
            painter.circle_stroke(c, s + 1.0, stroke);
            painter.circle_filled(c, 1.6, col);
            for k in 0..6 {
                let a = std::f32::consts::TAU * (k as f32) / 6.0;
                let (dx, dy) = (a.cos(), a.sin());
                painter.line_segment(
                    [
                        egui::pos2(c.x + dx * (s + 1.0), c.y + dy * (s + 1.0)),
                        egui::pos2(c.x + dx * (s + 3.0), c.y + dy * (s + 3.0)),
                    ],
                    stroke,
                );
            }
        }
        CaptionIcon::Close => {
            painter.line_segment(
                [egui::pos2(c.x - s, c.y - s), egui::pos2(c.x + s, c.y + s)],
                stroke,
            );
            painter.line_segment(
                [egui::pos2(c.x - s, c.y + s), egui::pos2(c.x + s, c.y - s)],
                stroke,
            );
        }
    }
    resp
}

/// Outcome of one chrome frame — the actions the user requested via the chrome
/// widgets. The host applies them after the panel closure returns so that the
/// grid/tree mutation does not happen mid-borrow.
#[derive(Debug, Default, Clone)]
pub struct ChromeActions {
    /// User clicked a tab; switch focus to this pane.
    pub focus_tab: Option<super::grid::PaneId>,
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
        // SCR1B3 titlebar idiom (scribe-app/src/app.rs §"Custom frameless
        // titlebar"): a `horizontal_centered` row. The LEFT content (wordmark +
        // tabs + split buttons) is emitted FIRST as siblings; then a NESTED
        // `right_to_left` layout consumes the REMAINING width and right-aligns
        // the caption cluster, pinning it FLUSH to the window's right edge at any
        // width. The previous attempt made `right_to_left` the OUTER layout and
        // nested `left_to_right` inside its leftover width, so the caption cluster
        // floated mid-strip — the reported bug. Mirroring SCR1B3 fixes it.
        ui.horizontal_centered(|ui| {
            // ---- left content (laid out left→right by horizontal_centered) ----
            // two-tone C0PL4ND wordmark + draggable caption region.
            let mut job = egui::text::LayoutJob::default();
            let fmt = |color| egui::text::TextFormat {
                color,
                font_id: egui::FontId::proportional(16.0),
                ..Default::default()
            };
            // "C0PL4ND" split: purple structural glyphs, green live glyphs.
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

            // tab strip (one tab per pane) — in STABLE visual order.
            for (pane_id, title) in self.pane_titles() {
                let selected = pane_id == self.focused_pane;
                let text = if selected {
                    RichText::new(&title).color(brand::GREEN)
                } else {
                    RichText::new(&title).color(brand::FG)
                };
                if ui.selectable_label(selected, text).clicked() {
                    actions.focus_tab = Some(pane_id);
                }
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

            // ---- caption cluster: a NESTED right_to_left layout over the
            //      REMAINING width pins this flush to the far right. Buttons are
            //      added close→max→min→gear; right_to_left places the FIRST-added
            //      furthest right, so the cluster READS left→right as ⚙ — ◻ ✕.
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let muted = brand::MUTED;
                let close_hover = Color32::from_rgb(0xE8, 0x11, 0x23);
                let soft_hover = Color32::from_rgba_unmultiplied(0xff, 0xff, 0xff, 26);
                if caption_btn(ui, CaptionIcon::Close, muted, close_hover)
                    .on_hover_text("close")
                    .clicked()
                {
                    actions.window_cmd = Some(super::WindowCmd::Close);
                }
                if caption_btn(ui, CaptionIcon::Maximize, muted, soft_hover)
                    .on_hover_text("maximize")
                    .clicked()
                {
                    actions.window_cmd = Some(super::WindowCmd::ToggleMaximize);
                }
                if caption_btn(ui, CaptionIcon::Minimize, muted, soft_hover)
                    .on_hover_text("minimize")
                    .clicked()
                {
                    actions.window_cmd = Some(super::WindowCmd::Minimize);
                }
                if caption_btn(ui, CaptionIcon::Gear, muted, soft_hover)
                    .on_hover_text("settings")
                    .clicked()
                {
                    actions.toggle_settings = true;
                }
            });
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
                RichText::new("C0PL4ND — egui chrome (milestone 1, placeholder grid)")
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
