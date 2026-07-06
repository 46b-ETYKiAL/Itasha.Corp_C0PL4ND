//! `C0pl4ndApp` config + theme observation accessors.
//!
//! Small getters over the live `config`/`theme` that the egui_kittest settings
//! tests use to assert observable changes after driving the real settings UI
//! through `frame_tick` — the same production path the app uses. Grouped out of
//! the `C0pl4ndApp` impl in the god-module; behaviour unchanged.

use super::theme;

impl super::C0pl4ndApp {
    /// The current font size (pt) from the live config. Used by the settings
    /// slider interaction test.
    #[allow(dead_code)]
    pub fn config_font_size(&self) -> f32 {
        self.config.font.size
    }

    /// The current primary font family from the live config. Observation accessor
    /// for the Font-family dropdown interaction test.
    #[allow(dead_code)]
    pub fn config_font_family(&self) -> String {
        self.config.font.family.clone()
    }

    /// The current ordered fallback font families from the live config.
    /// Observation accessor for the Fallback dropdown interaction test.
    #[allow(dead_code)]
    pub fn config_font_fallback(&self) -> Vec<String> {
        self.config.font.fallback.clone()
    }

    /// The font-stack key (family + fallbacks) most recently INSTALLED into egui.
    /// Observation accessor for the live-apply interaction test: after a family
    /// change is driven through the real frame loop, this MUST reflect the new
    /// family (proving the font was actually re-installed, not just stored in
    /// config). Mirrors [`font_apply_key`].
    #[allow(dead_code)]
    pub fn applied_font_key(&self) -> String {
        self.applied_font_family.clone()
    }

    /// The current cursor blink flag from the live config.
    #[allow(dead_code)]
    pub fn config_cursor_blink(&self) -> bool {
        self.config.cursor.blink
    }

    /// The customizable toolbar's LEFT-group action ids from the live config.
    /// Observation accessor for the Settings → Toolbar interaction tests.
    #[allow(dead_code)]
    pub fn config_toolbar_left(&self) -> Vec<String> {
        self.config.toolbar.left.clone()
    }

    /// The customizable toolbar's RIGHT-cluster action ids from the live config.
    #[allow(dead_code)]
    pub fn config_toolbar_right(&self) -> Vec<String> {
        self.config.toolbar.right.clone()
    }

    /// The customizable toolbar's overflow-menu ids from the live config.
    #[allow(dead_code)]
    pub fn config_toolbar_menu(&self) -> Vec<String> {
        self.config.toolbar.menu.clone()
    }

    /// Whether the toolbar's overflow "⋯" button is enabled in the live config.
    #[allow(dead_code)]
    pub fn config_toolbar_show_overflow(&self) -> bool {
        self.config.toolbar.show_overflow
    }

    /// The master transparency toggle from the live config. Observation
    /// accessor for the transparency interaction tests.
    #[allow(dead_code)]
    pub fn config_transparency_enabled(&self) -> bool {
        self.config.transparency_enabled
    }

    /// The current window translucency mode from the live config.
    #[allow(dead_code)]
    pub fn config_window_mode(&self) -> c0pl4nd_core::config::WindowMode {
        self.config.window_mode
    }

    /// The current window opacity (0.30..=1.0) from the live config.
    #[allow(dead_code)]
    pub fn config_opacity(&self) -> f32 {
        self.config.opacity
    }

    /// The current window tint strength (0.0..=1.0) from the live config.
    #[allow(dead_code)]
    pub fn config_tint_strength(&self) -> f32 {
        self.config.tint_strength
    }

    /// Whether the window is effectively translucent for the live config
    /// (master toggle on AND a non-opaque mode).
    #[allow(dead_code)]
    pub fn config_effective_translucent(&self) -> bool {
        self.config.effective_translucent()
    }

    /// The current terminal color theme stem from the live config.
    #[allow(dead_code)]
    pub fn config_theme(&self) -> &str {
        &self.config.theme
    }

    /// Whether the egui Visuals DERIVED from the active terminal theme read as
    /// LIGHT (window-fill luminance > 0.5). Observation accessor for the
    /// whole-app-theming interaction test: it asserts the chrome flips light
    /// after picking a light theme (ghost-paper) and dark after a dark one,
    /// exercising the same `visuals_from_theme` derivation the live app applies.
    #[allow(dead_code)]
    pub fn visuals_are_light(&self) -> bool {
        theme::is_light(theme::visuals_from_theme(&self.theme).window_fill)
    }

    /// The current scrollback line count from the live config.
    #[allow(dead_code)]
    pub fn config_scrollback_lines(&self) -> usize {
        self.config.scrollback_lines
    }

    /// The current multi-line-paste-warning flag from the live config.
    #[allow(dead_code)]
    pub fn config_paste_warn_multiline(&self) -> bool {
        self.config.paste_warn_multiline
    }
}
