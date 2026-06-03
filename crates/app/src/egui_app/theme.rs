//! Theme-derived egui chrome styling. [`visuals_from_theme`] and
//! [`ChromeColors`] derive the egui `Visuals` + the chrome surface palette
//! (titlebar / tab strip / status bar / settings window / panel fills) FROM the
//! active terminal [`c0pl4nd_core::Theme`], so the WHOLE app UI follows the
//! selected theme: a LIGHT theme (e.g. `ghost-paper`) produces a light egui
//! base, a DARK theme a dark base (chosen from the theme background's luminance
//! via [`is_light`]). The terminal grid's glyph colours still come from the same
//! `Theme`'s ANSI map (Milestone 2).
//!
//! The two-tone C0PL4ND wordmark keeps its fixed brand accent — Itasha purple
//! `#7700FF` + `.Corp` green `#00FF90` — the brand identity; everything else
//! (surfaces, text, hover/press/selection accents) is derived from the theme.
//! The [`brand`] module exposes those two accents plus `BG`/`FG` fallbacks used
//! when a minimal theme omits the optional background/foreground/selection slots.

use egui::{Color32, CornerRadius, Stroke, Visuals};

/// Perceptual luminance (sRGB Rec.601 weights, 0.0..=1.0) of an egui colour.
/// Used to pick a LIGHT vs DARK egui base for a theme and to derive sensible
/// shaded panel/widget fills regardless of the theme's polarity.
pub fn luminance(c: Color32) -> f32 {
    let [r, g, b, _] = c.to_array();
    (0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32) / 255.0
}

/// True when `c` reads as a LIGHT colour (luminance > 0.5). The whole-app
/// theming pivot: a light theme background produces a light egui base, a dark
/// one a dark base.
pub fn is_light(c: Color32) -> bool {
    luminance(c) > 0.5
}

/// Parse a `c0pl4nd_core::Theme` `#rrggbb` field into an egui `Color32`, falling
/// back to `fallback` when the field is empty or unparseable (e.g. the optional
/// `selection_background` slot a minimal theme omits).
fn theme_color(hex: &str, fallback: Color32) -> Color32 {
    match c0pl4nd_core::theme::parse_hex(hex) {
        Ok((r, g, b)) => Color32::from_rgb(r, g, b),
        Err(_) => fallback,
    }
}

/// Shade `base` toward white (dark themes) or toward black (light themes) by
/// `amount` (0.0..=1.0), so panels/widgets read as a subtly-raised surface above
/// the window background in EITHER polarity. On a dark base this lightens; on a
/// light base it darkens — the conventional "elevated surface" cue.
fn shade(base: Color32, amount: f32) -> Color32 {
    let toward = if is_light(base) {
        Color32::BLACK
    } else {
        Color32::WHITE
    };
    base.lerp_to_gamma(toward, amount)
}

/// Build an `egui::Visuals` DERIVED FROM the active terminal colour `theme`, so
/// the whole chrome (titlebar / tab strip / status bar / settings window /
/// panel fills) follows the selected theme — light themes (e.g. `ghost-paper`)
/// produce a LIGHT egui base, dark themes a dark base.
///
/// The polarity is chosen from the theme background's luminance
/// ([`is_light`]); the chosen [`Visuals::light`]/[`Visuals::dark`] base is then
/// overridden so window/panel/widget backgrounds derive from `theme.background`
/// (panels/widgets subtly [`shade`]d so they read as raised surfaces), text from
/// `theme.foreground`, and selection/hyperlink/accent from
/// `theme.selection_background` (falling back to a bright accent when the theme
/// omits it). The two-tone C0PL4ND wordmark keeps its fixed brand accent (drawn
/// directly in `chrome.rs`); only the surfaces follow the theme.
pub fn visuals_from_theme(theme: &c0pl4nd_core::Theme) -> Visuals {
    let bg = theme_color(&theme.background, Color32::from_rgb(0x12, 0x12, 0x12));
    let fg = theme_color(&theme.foreground, Color32::from_rgb(0xe8, 0xe6, 0xf0));
    let light = is_light(bg);

    // Panel + bezel as raised surfaces above the window bg, deeper shade for the
    // bezel (widget fills) than the panel so the elevation reads at a glance.
    let panel = shade(bg, 0.06);
    let bezel = shade(bg, 0.12);

    // Accent: the theme's selection colour drives the live/hover accent and
    // selection wash. The cursor colour is the press/active accent. Both fall
    // back to the brand pair when the theme omits the optional slots.
    let accent = theme_color(&theme.selection_background, brand::GREEN);
    let press = theme_color(&theme.cursor, brand::PURPLE);
    let sel = {
        let [r, g, b, _] = accent.to_array();
        Color32::from_rgba_unmultiplied(r, g, b, 0x60)
    };

    // Weak/secondary text: blend fg toward bg so it reads as muted in either
    // polarity (the analogue of the fixed `MUTED` tone the dark theme used).
    let muted = fg.lerp_to_gamma(bg, 0.55);

    let mut v = if light {
        Visuals::light()
    } else {
        Visuals::dark()
    };
    v.extreme_bg_color = bg;
    v.panel_fill = panel;
    v.window_fill = panel;
    v.faint_bg_color = panel;
    v.override_text_color = Some(fg);
    v.hyperlink_color = accent;
    v.selection.bg_fill = sel;
    v.selection.stroke = Stroke::new(1.0, accent);
    v.error_fg_color = Color32::from_rgb(0xff, 0x3b, 0x5c); // alarm red (polarity-agnostic)
    v.warn_fg_color = Color32::from_rgb(0xff, 0xc4, 0x4d); // warn amber

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
    v.widgets.hovered.bg_stroke = Stroke::new(1.0, accent); // accent outline on hover
    v.widgets.hovered.fg_stroke = Stroke::new(1.0, accent);
    v.widgets.active.bg_fill = bezel;
    v.widgets.active.bg_stroke = Stroke::new(1.0, press); // press accent
    v.widgets.active.fg_stroke = Stroke::new(1.0, fg);

    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, bezel); // separators
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, fg);
    v.weak_text_color = Some(muted);
    v.window_corner_radius = CornerRadius::same(8);
    v.window_stroke = Stroke::new(1.0, bezel);

    v
}

/// Theme-derived chrome colours, computed once per frame from the active
/// terminal [`c0pl4nd_core::Theme`] and handed to the chrome painters so the
/// titlebar / tab strip / status bar / settings panels follow the theme without
/// each call-site re-deriving them. The two-tone C0PL4ND wordmark keeps its
/// fixed [`brand::PURPLE`]/[`brand::GREEN`] accent; everything else is theme-led.
#[derive(Debug, Clone, Copy)]
pub struct ChromeColors {
    /// Window background (the central pane fill behind the grid).
    pub bg: Color32,
    /// Raised-surface fill for the titlebar / status bar / settings window.
    pub panel: Color32,
    /// Deeper-shaded fill for widget surfaces / hairlines / inactive borders.
    pub bezel: Color32,
    /// Primary text colour (from the theme foreground).
    pub fg: Color32,
    /// Muted/secondary text + glyph-button base (fg blended toward bg).
    pub muted: Color32,
    /// Live/selected accent (from the theme selection colour; brand green when
    /// the theme omits it). Used for the focused tab, status accent, headings.
    pub accent: Color32,
}

impl ChromeColors {
    /// Derive the chrome colours from the active terminal theme — the single
    /// place the chrome's surface palette is computed (mirrors the shading the
    /// egui Visuals use in [`visuals_from_theme`] so chrome painted directly
    /// with these colours matches the Visuals-styled widgets exactly).
    pub fn from_theme(theme: &c0pl4nd_core::Theme) -> Self {
        let bg = theme_color(&theme.background, brand::BG);
        let fg = theme_color(&theme.foreground, brand::FG);
        let accent = theme_color(&theme.selection_background, brand::GREEN);
        Self {
            bg,
            panel: shade(bg, 0.06),
            bezel: shade(bg, 0.12),
            fg,
            muted: fg.lerp_to_gamma(bg, 0.55),
            accent,
        }
    }
}

/// Brand accent colors exposed to the chrome module so the wordmark and
/// placeholder panes can paint with the same palette without re-deriving it.
pub mod brand {
    use egui::Color32;

    /// `#7700FF` — Itasha purple (structural accent).
    pub const PURPLE: Color32 = Color32::from_rgb(0x77, 0x00, 0xff);
    /// `#00FF90` — .Corp green (live accent).
    pub const GREEN: Color32 = Color32::from_rgb(0x00, 0xff, 0x90);
    /// `#e8e6f0` — foreground text (fallback when a theme omits foreground).
    pub const FG: Color32 = Color32::from_rgb(0xe8, 0xe6, 0xf0);
    /// `#121212` — void background (fallback when a theme omits background).
    pub const BG: Color32 = Color32::from_rgb(0x12, 0x12, 0x12);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn luminance_separates_light_and_dark() {
        assert!(is_light(Color32::WHITE));
        assert!(!is_light(Color32::BLACK));
        // ghost-paper bg #f0eef5 is light; the void #121212 is dark.
        assert!(is_light(Color32::from_rgb(0xf0, 0xee, 0xf5)));
        assert!(!is_light(Color32::from_rgb(0x12, 0x12, 0x12)));
    }

    #[test]
    fn shade_lightens_dark_and_darkens_light() {
        // Dark base shades TOWARD white (raised surface reads brighter).
        let dark = Color32::from_rgb(0x12, 0x12, 0x12);
        assert!(luminance(shade(dark, 0.12)) > luminance(dark));
        // Light base shades TOWARD black (raised surface reads darker).
        let light = Color32::from_rgb(0xf0, 0xee, 0xf5);
        assert!(luminance(shade(light, 0.12)) < luminance(light));
    }

    #[test]
    fn visuals_from_dark_theme_are_dark() {
        let t = c0pl4nd_core::Theme::builtin_void();
        let v = visuals_from_theme(&t);
        assert!(
            !is_light(v.window_fill),
            "a dark theme must produce a dark egui base (window_fill={:?})",
            v.window_fill
        );
        // Text + extreme bg derive from the theme, not the fixed brand palette.
        assert_eq!(
            v.override_text_color,
            Some(theme_color(&t.foreground, brand::FG))
        );
        assert_eq!(v.extreme_bg_color, theme_color(&t.background, brand::BG));
    }

    #[test]
    fn visuals_from_light_theme_are_light() {
        let t = c0pl4nd_core::Theme::builtin_named("ghost-paper").expect("ghost-paper embedded");
        let v = visuals_from_theme(&t);
        assert!(
            is_light(v.window_fill),
            "a light theme (ghost-paper) must produce a LIGHT egui base \
             (window_fill={:?})",
            v.window_fill
        );
        // The extreme bg is the theme's light background, not the dark void.
        assert!(is_light(v.extreme_bg_color));
    }

    #[test]
    fn chrome_colors_follow_theme_polarity() {
        let dark = ChromeColors::from_theme(&c0pl4nd_core::Theme::builtin_void());
        assert!(!is_light(dark.bg) && !is_light(dark.panel));
        let light = ChromeColors::from_theme(
            &c0pl4nd_core::Theme::builtin_named("ghost-paper").expect("ghost-paper embedded"),
        );
        assert!(is_light(light.bg) && is_light(light.panel));
    }
}
