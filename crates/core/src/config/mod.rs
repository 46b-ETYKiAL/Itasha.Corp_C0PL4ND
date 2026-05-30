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

/// Top-level configuration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Name of the theme to load (matches a file stem in the themes dir).
    pub theme: String,
    pub font: FontConfig,
    pub scrollback_lines: usize,
    /// Window opacity 0.0..=1.0.
    pub opacity: f32,
    pub cursor: CursorConfig,
    pub window: WindowConfig,
    pub effects: EffectsConfig,
    pub keybindings: Keybindings,
    pub update: UpdateConfig,
    /// Show the neofetch-style startup panel (logo + system info) on launch.
    pub startup_panel: bool,
    /// Override shell program; `None` = use the platform default shell.
    pub shell: Option<String>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            theme: "itasha-void".to_string(),
            font: FontConfig::default(),
            scrollback_lines: 10_000,
            opacity: 1.0,
            cursor: CursorConfig::default(),
            window: WindowConfig::default(),
            effects: EffectsConfig::default(),
            keybindings: Keybindings::default(),
            update: UpdateConfig::default(),
            startup_panel: true,
            shell: None,
        }
    }
}

impl Config {
    /// Parse a TOML string into a `Config`, surfacing a readable error.
    pub fn from_toml(src: &str, path: &Path) -> Result<Config, ConfigError> {
        let cfg: Config = toml::from_str(src).map_err(|e| ConfigError::Parse {
            path: path.to_path_buf(),
            message: e.to_string(),
        })?;
        cfg.validate()?;
        Ok(cfg)
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
        assert_eq!(c.theme, "itasha-void");
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
}
