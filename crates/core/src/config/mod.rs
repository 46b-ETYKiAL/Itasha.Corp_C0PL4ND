//! Configuration: great defaults + a simple, non-programming-language TOML
//! file with line-level error surfacing. Zero-config is a first-class goal —
//! C0PL4ND must be fully usable before the user ever opens a config file.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// A configuration load error with enough context to point the user at the
/// offending line — never a bare panic on a malformed file.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("config file not found at {0} (using built-in defaults)")]
    NotFound(PathBuf),
    #[error("could not read config file {path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("config parse error in {path}: {message}")]
    Parse { path: PathBuf, message: String },
    #[error("config validation error: {0}")]
    Invalid(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FontConfig {
    pub family: String,
    pub size: f32,
    /// Line height in pixels (cell vertical advance).
    pub line_height: f32,
    /// Ordered fallback families for glyphs the primary font lacks (CJK, etc.).
    pub fallback: Vec<String>,
}

impl Default for FontConfig {
    fn default() -> Self {
        // Monaspace Neon is the brand mono voice; fall back to any monospace.
        FontConfig {
            family: "Monaspace Neon".to_string(),
            size: 14.0,
            line_height: 20.0,
            fallback: vec!["Noto Sans JP".to_string(), "monospace".to_string()],
        }
    }
}

/// Opt-in update behaviour. Local-first: OFF by default — C0PL4ND never
/// contacts the network unless the user enables this or runs `c0pl4nd update`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct UpdateConfig {
    /// Check GitHub Releases for a newer version on launch (opt-in).
    pub check_on_launch: bool,
    /// Release channel to track.
    pub channel: String,
}

impl Default for UpdateConfig {
    fn default() -> Self {
        UpdateConfig {
            check_on_launch: false,
            channel: "stable".to_string(),
        }
    }
}

/// User-rebindable key bindings (action name -> key combo string).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Keybindings {
    pub copy: String,
    pub paste: String,
    pub new_tab: String,
    pub close_tab: String,
    pub next_tab: String,
    pub split_right: String,
    pub split_down: String,
    pub search: String,
    pub command_palette: String,
    pub increase_font: String,
    pub decrease_font: String,
}

impl Default for Keybindings {
    fn default() -> Self {
        // Platform-sensible defaults; the modifier is Ctrl+Shift on Win/Linux,
        // Cmd on macOS (the UI layer maps "mod" to the platform modifier).
        Keybindings {
            copy: "mod+shift+c".into(),
            paste: "mod+shift+v".into(),
            new_tab: "mod+shift+t".into(),
            close_tab: "mod+shift+w".into(),
            next_tab: "mod+shift+]".into(),
            split_right: "mod+shift+d".into(),
            split_down: "mod+shift+e".into(),
            search: "mod+shift+f".into(),
            command_palette: "mod+shift+p".into(),
            increase_font: "mod+plus".into(),
            decrease_font: "mod+minus".into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CursorStyle {
    Block,
    Bar,
    Underline,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CursorConfig {
    pub style: CursorStyle,
    pub blink: bool,
}

impl Default for CursorConfig {
    fn default() -> Self {
        CursorConfig {
            style: CursorStyle::Block,
            blink: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WindowConfig {
    pub cols: u16,
    pub rows: u16,
    pub padding: u16,
    /// Remembered window geometry (physical pixels), persisted on resize/move/
    /// exit and restored on launch. `None` = use the cols/rows-derived default.
    /// Optional so configs written before this field still parse cleanly.
    pub pos_x: Option<i32>,
    pub pos_y: Option<i32>,
    pub size_w: Option<u32>,
    pub size_h: Option<u32>,
    pub maximized: Option<bool>,
    /// Identifies the monitor the geometry was captured on, so a saved position
    /// is only restored when that monitor is still connected (multi-monitor
    /// safety). Matched against `MonitorHandle::name()` at restore time.
    pub monitor: Option<String>,
}

impl Default for WindowConfig {
    fn default() -> Self {
        WindowConfig {
            cols: 80,
            rows: 24,
            padding: 8,
            pos_x: None,
            pos_y: None,
            size_w: None,
            size_h: None,
            maximized: None,
            monitor: None,
        }
    }
}

/// Window translucency mode. `Opaque` is the default; the rest reveal what's
/// behind the window to varying degrees. `Transparent` is the portable
/// (cross-platform) reduced-alpha surface; `Glass`/`Mica`/`Vibrancy` request an
/// OS blur backdrop (acrylic/mica on Windows, vibrancy on macOS) and degrade to
/// the portable transparent surface where the API is absent (e.g. Linux).
///
/// Mirrors SCR1B3's `WindowMode` so the two sibling apps expose the same
/// transparency vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum WindowMode {
    #[default]
    Opaque,
    Transparent,
    Glass,
    Mica,
    Vibrancy,
}

impl WindowMode {
    /// Whether this mode wants a non-opaque surface.
    pub fn is_translucent(self) -> bool {
        !matches!(self, WindowMode::Opaque)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct EffectsConfig {
    /// CRT scanline post-effect. OFF by default; also auto-disabled under
    /// reduced-motion / battery-save (see renderer).
    pub crt_scanlines: bool,
    /// Chromatic-aberration intensity (0.0 = off).
    pub chromatic_aberration: f32,
}

impl Default for EffectsConfig {
    fn default() -> Self {
        EffectsConfig {
            crt_scanlines: false,
            chromatic_aberration: 0.0,
        }
    }
}

/// The default window tint color (`#RRGGBB`) — a near-black void wash matching
/// the brand background. A free function so `#[serde(default = ...)]` can name
/// it for the `tint` field.
fn default_tint() -> String {
    "#121212".to_string()
}

/// Top-level configuration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Name of the theme to load (matches a file stem in the themes dir).
    pub theme: String,
    pub font: FontConfig,
    pub scrollback_lines: usize,
    /// Window opacity 0.0..=1.0. Below 1.0 the window is created translucent
    /// (applies next launch); the desktop / acrylic backdrop shows through.
    pub opacity: f32,
    /// Opt-in Windows 11 acrylic/mica backdrop behind the translucent window.
    /// Default off, so the default experience is a solid window. Applies next
    /// launch. Honoured only when the GPU surface supports a non-opaque
    /// composite-alpha mode; otherwise the window stays opaque (graceful
    /// fallback).
    ///
    /// **Legacy field.** Superseded by [`Config::transparency_enabled`] +
    /// [`Config::window_mode`]; retained for backward-compat so older config
    /// files still parse and so `acrylic = true` migrates to
    /// `transparency_enabled = true, window_mode = "glass"` on load.
    #[serde(default)]
    pub acrylic: bool,
    /// Master on/off switch for the whole transparency system. When `false` the
    /// window paints fully opaque regardless of [`Config::window_mode`] — a
    /// fast, safe default that avoids the layered-window ghost-on-close failure
    /// mode on Windows. Mirrors SCR1B3's `transparency_enabled`.
    ///
    /// Default OFF — translucency is opt-in. Existing configs that set
    /// `opacity < 1.0` or `acrylic = true` are migrated to ON on load (see
    /// [`Config::migrate_legacy_transparency`]) so their appearance is
    /// preserved.
    #[serde(default)]
    pub transparency_enabled: bool,
    /// The translucency mode applied when [`Config::transparency_enabled`] is
    /// on. Mirrors SCR1B3's `window.mode`.
    #[serde(default)]
    pub window_mode: WindowMode,
    /// Tint color (`#RRGGBB`) painted over the window at
    /// [`Config::tint_strength`] when a translucent mode is active. Mirrors
    /// SCR1B3's `window.tint`.
    #[serde(default = "default_tint")]
    pub tint: String,
    /// Tint overlay strength (0.0 = none .. 1.0 = strong). Mirrors SCR1B3's
    /// `window.tint_strength`. Default 0.0 (no tint).
    #[serde(default)]
    pub tint_strength: f32,
    pub cursor: CursorConfig,
    pub window: WindowConfig,
    pub effects: EffectsConfig,
    pub keybindings: Keybindings,
    pub update: UpdateConfig,
    /// Show the neofetch-style startup panel (logo + system info) on launch.
    pub startup_panel: bool,
    /// Override shell program; `None` = use the platform default shell.
    pub shell: Option<String>,
    /// Enable font ligatures / complex text shaping in the renderer.
    ///
    /// Core-side preference flag only — the actual shaping is the renderer's
    /// concern (cosmic-text `Shaping::Advanced`). When `false`, the renderer
    /// should shape per-cell (`Shaping::Basic`) for monospace fidelity; when
    /// `true`, it may run advanced shaping so programming ligatures (e.g. `->`,
    /// `!=`) and complex scripts render. Defaults to `false` so monospace grid
    /// alignment is preserved unless the user opts in.
    pub ligatures: bool,
    /// Automatically copy a mouse text selection to the clipboard the moment
    /// the drag ends (X11-style "copy on select"). Write-only. Defaults to
    /// `false` — copy stays an explicit Ctrl/Cmd+Shift+C unless opted in.
    pub copy_on_select: bool,
    /// Warn before pasting clipboard text that contains a newline (a multi-line
    /// paste can run shell commands the instant it lands). When `true` (default)
    /// such a paste shows a confirm overlay first. A security feature — set
    /// `false` to paste multi-line content without confirmation.
    pub paste_warn_multiline: bool,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            theme: "itasha-corp".to_string(),
            font: FontConfig::default(),
            scrollback_lines: 10_000,
            opacity: 1.0,
            acrylic: false,
            transparency_enabled: false,
            window_mode: WindowMode::Opaque,
            tint: default_tint(),
            tint_strength: 0.0,
            cursor: CursorConfig::default(),
            window: WindowConfig::default(),
            effects: EffectsConfig::default(),
            keybindings: Keybindings::default(),
            update: UpdateConfig::default(),
            startup_panel: true,
            shell: None,
            ligatures: false,
            copy_on_select: false,
            paste_warn_multiline: true,
        }
    }
}

impl Config {
    /// Parse a TOML string into a `Config`, surfacing a readable error.
    pub fn from_toml(src: &str, path: &Path) -> Result<Config, ConfigError> {
        let mut cfg: Config = toml::from_str(src).map_err(|e| ConfigError::Parse {
            path: path.to_path_buf(),
            message: e.to_string(),
        })?;
        cfg.migrate_legacy_transparency();
        cfg.validate()?;
        Ok(cfg)
    }

    /// Whether translucency should actually be rendered: the master toggle is
    /// on AND the chosen mode wants a non-opaque surface. This is the single
    /// predicate every render path consults so the master switch is honoured
    /// uniformly (surface request, the opacity pass, and the tint overlay).
    /// Mirrors SCR1B3's `WindowConfig::effective_translucent`.
    pub fn effective_translucent(&self) -> bool {
        self.transparency_enabled && self.window_mode.is_translucent()
    }

    /// Migrate the pre-modes transparency model to the new master-toggle + mode
    /// model so existing config files keep their appearance.
    ///
    /// Older c0pl4nd configs expressed transparency via the top-level
    /// `opacity < 1.0` (translucent surface) and the `acrylic` bool (Win11
    /// blur). When a loaded config carries no explicit new-model fields
    /// (`transparency_enabled` is still its `false` default and `window_mode`
    /// still `Opaque`) but DOES carry a legacy translucency signal, promote it:
    /// `acrylic = true` → Glass; otherwise `opacity < 1.0` → Transparent. This
    /// runs only when the new fields are at their defaults, so a config that
    /// explicitly sets the new model is never overridden.
    pub fn migrate_legacy_transparency(&mut self) {
        // Only migrate when the user has NOT opted into the new model.
        if self.transparency_enabled || self.window_mode != WindowMode::Opaque {
            return;
        }
        if self.acrylic {
            self.transparency_enabled = true;
            self.window_mode = WindowMode::Glass;
        } else if self.opacity < 1.0 {
            self.transparency_enabled = true;
            self.window_mode = WindowMode::Transparent;
        }
    }

    /// Validate value ranges. Never panics; returns a descriptive error.
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.theme.trim().is_empty() {
            return Err(ConfigError::Invalid("theme name must not be empty".into()));
        }
        if !(0.0..=1.0).contains(&self.opacity) {
            return Err(ConfigError::Invalid(format!(
                "opacity must be between 0.0 and 1.0, got {}",
                self.opacity
            )));
        }
        if self.font.size <= 0.0 {
            return Err(ConfigError::Invalid("font.size must be positive".into()));
        }
        if self.window.cols == 0 || self.window.rows == 0 {
            return Err(ConfigError::Invalid(
                "window cols and rows must be non-zero".into(),
            ));
        }
        if !(0.0..=1.0).contains(&self.tint_strength) {
            return Err(ConfigError::Invalid(format!(
                "tint_strength must be between 0.0 and 1.0, got {}",
                self.tint_strength
            )));
        }
        if crate::theme::parse_hex(&self.tint).is_err() {
            return Err(ConfigError::Invalid(format!(
                "tint must be a #RRGGBB hex color, got {:?}",
                self.tint
            )));
        }
        Ok(())
    }

    /// The platform default per-user config file path.
    pub fn default_path() -> Option<PathBuf> {
        // ~/.config/c0pl4nd/config.toml on Unix; %APPDATA%\c0pl4nd\config.toml on Windows.
        #[cfg(windows)]
        {
            std::env::var_os("APPDATA")
                .map(|p| PathBuf::from(p).join("c0pl4nd").join("config.toml"))
        }
        #[cfg(not(windows))]
        {
            std::env::var_os("XDG_CONFIG_HOME")
                .map(PathBuf::from)
                .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
                .map(|p| p.join("c0pl4nd").join("config.toml"))
        }
    }

    /// Load from a specific path. Missing file → built-in defaults (zero-config).
    pub fn load_from(path: &Path) -> Result<Config, ConfigError> {
        match std::fs::read_to_string(path) {
            Ok(src) => Config::from_toml(&src, path),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Config::default()),
            Err(e) => Err(ConfigError::Io {
                path: path.to_path_buf(),
                source: e,
            }),
        }
    }

    /// Serialize to a pretty TOML string. The inverse of [`Config::from_toml`].
    pub fn to_toml(&self) -> Result<String, ConfigError> {
        toml::to_string_pretty(self).map_err(|e| ConfigError::Invalid(e.to_string()))
    }

    /// Persist to a specific path, creating parent directories as needed.
    /// Used by the settings panel and the window-geometry persistence so the
    /// config file stays the single source of truth.
    pub fn save_to(&self, path: &Path) -> Result<(), ConfigError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| ConfigError::Io {
                path: parent.to_path_buf(),
                source: e,
            })?;
        }
        let body = self.to_toml()?;
        std::fs::write(path, body).map_err(|e| ConfigError::Io {
            path: path.to_path_buf(),
            source: e,
        })
    }

    /// Update only the persisted window-geometry fields on the file at
    /// [`Config::default_path`], preserving every other field the user set.
    /// Best-effort: a load/parse failure falls back to the in-memory config so
    /// a corrupt file never blocks geometry capture. Returns the path written.
    pub fn persist_geometry(window: WindowConfig) -> Option<PathBuf> {
        let path = Config::default_path()?;
        let mut cfg = Config::load_from(&path).unwrap_or_default();
        // Copy only geometry; leave cols/rows/padding (size-on-first-launch)
        // untouched so an explicit user value is never clobbered.
        cfg.window.pos_x = window.pos_x;
        cfg.window.pos_y = window.pos_y;
        cfg.window.size_w = window.size_w;
        cfg.window.size_h = window.size_h;
        cfg.window.maximized = window.maximized;
        cfg.window.monitor = window.monitor;
        cfg.save_to(&path).ok()?;
        Some(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_sane_and_valid() {
        let c = Config::default();
        assert_eq!(c.theme, "itasha-corp");
        assert_eq!(c.scrollback_lines, 10_000);
        assert!(c.validate().is_ok());
    }

    #[test]
    fn partial_toml_fills_defaults() {
        let p = PathBuf::from("test.toml");
        let c = Config::from_toml("theme = \"ghost-paper\"\n", &p).unwrap();
        assert_eq!(c.theme, "ghost-paper");
        assert_eq!(c.font.size, 14.0); // default preserved
    }

    #[test]
    fn invalid_opacity_is_rejected() {
        let p = PathBuf::from("test.toml");
        let err = Config::from_toml("opacity = 5.0\n", &p);
        assert!(err.is_err());
    }

    #[test]
    fn malformed_toml_is_error_not_panic() {
        let p = PathBuf::from("test.toml");
        assert!(Config::from_toml("this is = = not toml", &p).is_err());
    }

    #[test]
    fn missing_file_yields_defaults() {
        let c = Config::load_from(&PathBuf::from("/nonexistent/c0pl4nd/config.toml")).unwrap();
        assert_eq!(c, Config::default());
    }

    #[test]
    fn ligatures_defaults_off() {
        let c = Config::default();
        assert!(!c.ligatures, "monospace fidelity is preserved by default");
    }

    #[test]
    fn ligatures_parses_from_partial_toml() {
        // A config file that only sets ligatures still works via serde(default).
        let p = PathBuf::from("test.toml");
        let c = Config::from_toml("ligatures = true\n", &p).unwrap();
        assert!(c.ligatures);
        // Untouched fields keep their defaults.
        assert_eq!(c.theme, "itasha-corp");
    }

    // ---- Transparency modes (SCR1B3-parity model) ----

    #[test]
    fn window_mode_is_translucent_per_variant() {
        assert!(!WindowMode::Opaque.is_translucent(), "opaque is solid");
        assert!(WindowMode::Transparent.is_translucent());
        assert!(WindowMode::Glass.is_translucent());
        assert!(WindowMode::Mica.is_translucent());
        assert!(WindowMode::Vibrancy.is_translucent());
    }

    #[test]
    fn effective_translucent_requires_master_and_translucent_mode() {
        // Default config: master off, mode opaque => not translucent.
        let mut c = Config::default();
        assert!(!c.effective_translucent(), "default is fully opaque");

        // Master on but mode still opaque => still not translucent.
        c.transparency_enabled = true;
        c.window_mode = WindowMode::Opaque;
        assert!(
            !c.effective_translucent(),
            "an opaque mode is never translucent even with the master on"
        );

        // Master on AND a translucent mode => translucent.
        c.window_mode = WindowMode::Glass;
        assert!(
            c.effective_translucent(),
            "master + a translucent mode renders translucent"
        );

        // Master off overrides any mode (the safe-default kill switch).
        c.transparency_enabled = false;
        assert!(
            !c.effective_translucent(),
            "the master switch off forces opaque regardless of mode"
        );
    }

    #[test]
    fn transparency_defaults_are_opaque_and_untinted() {
        let c = Config::default();
        assert!(!c.transparency_enabled, "transparency is opt-in (off)");
        assert_eq!(c.window_mode, WindowMode::Opaque);
        assert_eq!(c.tint_strength, 0.0, "no tint by default");
        assert_eq!(c.tint, "#121212");
    }

    #[test]
    fn legacy_acrylic_true_migrates_to_glass() {
        // A pre-modes config with the old acrylic bool must keep its blurred
        // look: acrylic = true => master ON + Glass mode.
        let p = PathBuf::from("test.toml");
        let c = Config::from_toml("acrylic = true\nopacity = 0.9\n", &p).unwrap();
        assert!(c.transparency_enabled, "acrylic implied transparency");
        assert_eq!(c.window_mode, WindowMode::Glass);
        assert!(c.effective_translucent());
    }

    #[test]
    fn legacy_low_opacity_migrates_to_transparent() {
        // A pre-modes config with only opacity < 1.0 (no acrylic) must keep its
        // see-through look: => master ON + Transparent mode.
        let p = PathBuf::from("test.toml");
        let c = Config::from_toml("opacity = 0.8\n", &p).unwrap();
        assert!(c.transparency_enabled, "low opacity implied transparency");
        assert_eq!(c.window_mode, WindowMode::Transparent);
        assert!(c.effective_translucent());
    }

    #[test]
    fn explicit_new_model_is_not_overridden_by_legacy_migration() {
        // A config that explicitly sets a new-model window_mode wins, even if a
        // legacy acrylic/opacity signal would imply a different mode. Migration
        // runs only when the new fields are at their defaults.
        let p = PathBuf::from("test.toml");
        let c = Config::from_toml(
            "acrylic = true\nopacity = 0.5\nwindow_mode = \"transparent\"\n",
            &p,
        )
        .unwrap();
        assert_eq!(
            c.window_mode,
            WindowMode::Transparent,
            "an explicit window_mode must survive legacy migration"
        );
        // The explicit mode is translucent, so it implies the master toggle on
        // only if the user also set it; here transparency_enabled stayed at its
        // false default and window_mode was explicit => migration is skipped and
        // the master stays off (explicit model, not promoted).
        assert!(
            !c.transparency_enabled,
            "explicit window_mode skips migration; master stays at its default"
        );
    }

    #[test]
    fn invalid_tint_strength_is_rejected() {
        let p = PathBuf::from("test.toml");
        assert!(
            Config::from_toml("tint_strength = 2.0\n", &p).is_err(),
            "tint_strength above 1.0 is invalid"
        );
    }

    #[test]
    fn invalid_tint_hex_is_rejected() {
        let p = PathBuf::from("test.toml");
        assert!(
            Config::from_toml("tint = \"not-a-color\"\n", &p).is_err(),
            "a non-#RRGGBB tint is invalid"
        );
    }

    #[test]
    fn window_mode_round_trips_through_toml() {
        let p = PathBuf::from("test.toml");
        let c = Config::from_toml(
            "transparency_enabled = true\nwindow_mode = \"mica\"\ntint = \"#aabbcc\"\ntint_strength = 0.4\n",
            &p,
        )
        .unwrap();
        assert_eq!(c.window_mode, WindowMode::Mica);
        assert_eq!(c.tint, "#aabbcc");
        assert!((c.tint_strength - 0.4).abs() < f32::EPSILON);
        assert!(c.effective_translucent());
    }
}
