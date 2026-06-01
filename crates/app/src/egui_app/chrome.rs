//! The C0PL4ND chrome: a frameless titlebar (two-tone wordmark + tab strip +
//! caption buttons), a settings gear, and a bottom status bar. Window controls
//! use `egui::ViewportCommand` (no Win32) per recon dossier §3.2.

use egui::{Align, Layout, RichText, Sense};
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
        // Drive the whole strip right-to-left over the FULL available width so the
        // caption cluster (close/maximize/minimize/gear) is pinned FLUSH to the
        // window's RIGHT edge regardless of width. The previous code nested the
        // right_to_left layout AFTER the left-aligned widgets, so it only occupied
        // the leftover width and floated mid-strip — the reported bug. The
        // left-aligned content (wordmark + tabs + split buttons) is then laid out
        // left_to_right inside the remaining space.
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            // ---- caption cluster: emitted FIRST in a right_to_left layout, so it
            //      hugs the far right edge. Order is reversed here (close added
            //      first lands furthest right), so the cluster READS left→right as
            //      ⚙  —  ◻  ✕ at the far right.
            if ui
                .button(RichText::new(icon::X).size(16.0))
                .on_hover_text("close")
                .clicked()
            {
                actions.window_cmd = Some(super::WindowCmd::Close);
            }
            if ui
                .button(RichText::new(icon::SQUARE).size(16.0))
                .on_hover_text("maximize")
                .clicked()
            {
                actions.window_cmd = Some(super::WindowCmd::ToggleMaximize);
            }
            if ui
                .button(RichText::new(icon::MINUS).size(16.0))
                .on_hover_text("minimize")
                .clicked()
            {
                actions.window_cmd = Some(super::WindowCmd::Minimize);
            }
            if ui
                .button(RichText::new(icon::GEAR).size(16.0))
                .on_hover_text("settings")
                .clicked()
            {
                actions.toggle_settings = true;
            }

            // ---- left-aligned content fills the remaining space ----
            ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
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

                // tab strip (one tab per pane).
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
