//! The `itasha_corp` egui theme — maps the C0PL4ND brand palette onto
//! `egui::Visuals` for the chrome (titlebar / tabs / status bar / settings
//! window). The terminal grid's glyph colors come from `c0pl4nd_core::theme`'s
//! ANSI map (Milestone 2); these Visuals style ONLY the egui chrome.
//!
//! Palette (recon dossier §6, identical to the reference SCR1B3 brand theme):
//! - bg     `#121212` — void
//! - purple `#7700FF` — Itasha — structural accent / press
//! - green  `#00FF90` — .Corp — live accent / hover / cursor
//! - fg     `#e8e6f0` — text
//! - red    `#ff3b5c` — alarms only
//! - amber  `#ffc44d` — warnings

use egui::{Color32, CornerRadius, Stroke, Visuals};

/// Build the `itasha_corp` dark Visuals applied at startup (and on any future
/// theme change). Ported verbatim from the recon dossier §6.
pub fn itasha_corp_visuals() -> Visuals {
    let bg = Color32::from_rgb(0x12, 0x12, 0x12); // void
    let panel = Color32::from_rgb(0x1c, 0x1c, 0x1f);
    let bezel = Color32::from_rgb(0x2c, 0x2c, 0x33);
    let fg = Color32::from_rgb(0xe8, 0xe6, 0xf0); // text
    let green = Color32::from_rgb(0x00, 0xff, 0x90); // .Corp — live accent / cursor
    let purple = Color32::from_rgb(0x77, 0x00, 0xff); // Itasha — structural / keyword
    let red = Color32::from_rgb(0xff, 0x3b, 0x5c); // alarms only
    let amber = Color32::from_rgb(0xff, 0xc4, 0x4d); // warnings
    let sel = Color32::from_rgba_unmultiplied(0x77, 0x00, 0xff, 0x40); // purple wash

    let mut v = Visuals::dark();
    v.extreme_bg_color = bg;
    v.panel_fill = panel;
    v.window_fill = panel;
    v.faint_bg_color = panel;
    v.override_text_color = Some(fg);
    v.hyperlink_color = green;
    v.selection.bg_fill = sel;
    v.selection.stroke = Stroke::new(1.0, green);
    v.error_fg_color = red;
    v.warn_fg_color = amber;

    // Widgets: rounded, panel-fill, green hover / purple press accent.
    let radius = CornerRadius::same(4);
    for ws in [
        &mut v.widgets.inactive,
        &mut v.widgets.hovered,
        &mut v.widgets.active,
    ] {
        ws.corner_radius = radius;
    }
    v.widgets.noninteractive.bg_fill = panel;
    v.widgets.inactive.bg_fill = bezel;
    v.widgets.inactive.weak_bg_fill = panel;
    v.widgets.inactive.fg_stroke = Stroke::new(1.0, fg);
    v.widgets.hovered.bg_fill = bezel;
    v.widgets.hovered.bg_stroke = Stroke::new(1.0, green); // green outline on hover
    v.widgets.hovered.fg_stroke = Stroke::new(1.0, green);
    v.widgets.active.bg_fill = bezel;
    v.widgets.active.bg_stroke = Stroke::new(1.0, purple); // purple on press
    v.widgets.active.fg_stroke = Stroke::new(1.0, fg);
    v.window_corner_radius = CornerRadius::same(8); // frameless rounded
    v.window_stroke = Stroke::new(1.0, bezel);
    v
}

/// Brand accent colors exposed to the chrome module so the wordmark and
/// placeholder panes can paint with the same palette without re-deriving it.
pub mod brand {
    use egui::Color32;

    /// `#7700FF` — Itasha purple (structural accent).
    pub const PURPLE: Color32 = Color32::from_rgb(0x77, 0x00, 0xff);
    /// `#00FF90` — .Corp green (live accent).
    pub const GREEN: Color32 = Color32::from_rgb(0x00, 0xff, 0x90);
    /// `#e8e6f0` — foreground text.
    pub const FG: Color32 = Color32::from_rgb(0xe8, 0xe6, 0xf0);
    /// `#121212` — void background.
    pub const BG: Color32 = Color32::from_rgb(0x12, 0x12, 0x12);
    /// `#1c1c1f` — panel fill.
    pub const PANEL: Color32 = Color32::from_rgb(0x1c, 0x1c, 0x1f);
    /// `#2c2c33` — bezel.
    pub const BEZEL: Color32 = Color32::from_rgb(0x2c, 0x2c, 0x33);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn visuals_carry_brand_accents() {
        let v = itasha_corp_visuals();
        // override_text_color is the brand fg.
        assert_eq!(v.override_text_color, Some(brand::FG));
        // selection stroke is the .Corp green.
        assert_eq!(v.selection.stroke.color, brand::GREEN);
        // press accent is Itasha purple.
        assert_eq!(v.widgets.active.bg_stroke.color, brand::PURPLE);
    }
}
