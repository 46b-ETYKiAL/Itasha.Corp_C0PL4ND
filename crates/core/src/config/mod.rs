//! Configuration: great defaults + a simple, non-programming-language TOML
//! file with line-level error surfacing. Zero-config is a first-class goal —
//! C0PL4ND must be fully usable before the user ever opens a config file.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// A configuration load error with enough context to point the user at the
/// offending line — never a bare panic on a malformed file.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// The config file did not exist at the resolved path; built-in defaults
    /// are used instead.
    #[error("config file not found at {0} (using built-in defaults)")]
    NotFound(PathBuf),
    /// The config file exists but could not be read (I/O error).
    #[error("could not read config file {path}: {source}")]
    Io {
        /// Path that failed to read.
        path: PathBuf,
        /// Underlying I/O error.
        source: std::io::Error,
    },
    /// The config file could not be parsed as TOML.
    #[error("config parse error in {path}: {message}")]
    Parse {
        /// Path that failed to parse.
        path: PathBuf,
        /// Human-readable parse-error message, including line context.
        message: String,
    },
    /// The config parsed but failed semantic validation (e.g. an out-of-range
    /// value).
    #[error("config validation error: {0}")]
    Invalid(String),
}

/// Font configuration: the primary family, size, line height, and glyph
/// fallback chain.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FontConfig {
    /// Primary font family name.
    pub family: String,
    /// Font size in points.
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

/// When (and whether) C0PL4ND checks for updates. Local-first: the default is
/// [`UpdateMode::Manual`] — the app makes NO automatic network connection; the
/// user checks on demand from Settings. `notify`/`auto` are explicit opt-ins to
/// an on-launch GitHub-Releases version check (a check reads only the public
/// GitHub Releases API and sends zero identifiers).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum UpdateMode {
    /// Never check, never touch the network for updates.
    Off,
    /// Check once per launch (when due); show a passive toast if newer exists.
    Notify,
    /// Default: check only when the user presses "Check for updates".
    #[default]
    Manual,
    /// Check once per launch (when due); download + apply a verified update when
    /// one is found.
    Auto,
}

/// Opt-in update behaviour. Local-first: the default mode is `manual`, so
/// C0PL4ND never contacts the network unless the user presses "Check for
/// updates", runs `c0pl4nd update`, or opts into `notify`/`auto`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct UpdateConfig {
    /// When the app checks for updates (`off`/`notify`/`manual`/`auto`).
    pub mode: UpdateMode,
    /// How often, in hours, an on-launch check (`notify`/`auto`) is due (1–168).
    /// Ignored for `off`/`manual`.
    pub check_interval_hours: u32,
    /// Legacy on-launch toggle, retained so older config files keep loading. The
    /// canonical control is now [`UpdateConfig::mode`]; `mode == Off|Manual`
    /// means "no on-launch network", `mode == Notify|Auto` means "check on
    /// launch". The launch path treats `check_on_launch == true` OR a
    /// network-on-launch `mode` as "check".
    pub check_on_launch: bool,
    /// Release channel to track.
    pub channel: String,
}

impl Default for UpdateConfig {
    fn default() -> Self {
        UpdateConfig {
            mode: UpdateMode::Manual,
            check_interval_hours: 24,
            check_on_launch: false,
            channel: "stable".to_string(),
        }
    }
}

impl UpdateConfig {
    /// Whether an on-launch (background) update check should run: true for the
    /// network-on-launch modes (`notify`/`auto`), OR when the legacy
    /// `check_on_launch` flag is set (so old config files keep their behaviour).
    pub fn checks_on_launch(&self) -> bool {
        matches!(self.mode, UpdateMode::Notify | UpdateMode::Auto) || self.check_on_launch
    }
}

/// Which side a popout panel docks to. Used by the command-history sidebar
/// (`#21`): the user prefers a popout sidebar over a dropdown, and can dock it
/// left or right.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum PanelSide {
    Left,
    #[default]
    Right,
}

/// User-rebindable key bindings (action name -> key combo string).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Keybindings {
    /// Copy the selection to the clipboard.
    pub copy: String,
    /// Paste from the clipboard.
    pub paste: String,
    /// Open a new tab.
    pub new_tab: String,
    /// Close the current tab.
    pub close_tab: String,
    /// Switch to the next tab.
    pub next_tab: String,
    /// Split the focused pane to the right.
    pub split_right: String,
    /// Split the focused pane downward.
    pub split_down: String,
    /// Open the in-buffer find / search overlay.
    pub search: String,
    /// Open the command palette.
    pub command_palette: String,
    /// Toggle the command-history quick-run sidebar (`#21`).
    pub history_sidebar: String,
    /// Increase the font size.
    pub increase_font: String,
    /// Decrease the font size.
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
            history_sidebar: "mod+shift+h".into(),
            increase_font: "mod+plus".into(),
            decrease_font: "mod+minus".into(),
        }
    }
}

/// The shape of the text cursor.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CursorStyle {
    /// A filled block covering the whole cell.
    Block,
    /// A thin vertical bar at the cell's left edge.
    Bar,
    /// A horizontal underline at the cell's baseline.
    Underline,
}

/// Cursor appearance configuration: shape and blink behaviour.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CursorConfig {
    /// The cursor shape.
    pub style: CursorStyle,
    /// Whether the cursor blinks.
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

/// Window configuration: the initial terminal dimensions, inner padding, and
/// the persisted geometry restored on the next launch.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WindowConfig {
    /// Initial terminal width in columns.
    pub cols: u16,
    /// Initial terminal height in rows.
    pub rows: u16,
    /// Inner padding between the window edge and the grid, in pixels.
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

/// How the pane shell lays out terminals: the multi-pane `egui_tiles` grid, or
/// a single full-size pane with the existing tab strip switching between them.
/// One shell layout is active at a time; the titlebar view-toggle button flips
/// between them, and the choice persists across restarts. Mirrors the
/// split-view vs tabs-view affordance common to modern terminals.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ViewMode {
    /// The `egui_tiles` tiling grid — every pane visible side-by-side/stacked.
    #[default]
    Grid,
    /// Only the focused pane is shown full-size; the tab strip switches panes.
    Tabs,
}

impl ViewMode {
    /// The mode reached by toggling this one (Grid ⇄ Tabs).
    pub fn toggled(self) -> ViewMode {
        match self {
            ViewMode::Grid => ViewMode::Tabs,
            ViewMode::Tabs => ViewMode::Grid,
        }
    }
}

/// Visual post-effects configuration: the CRT/scanline overlay and
/// chromatic-aberration controls. All effects are OFF by default.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct EffectsConfig {
    /// CRT scanline post-effect. OFF by default; also auto-disabled under
    /// reduced-motion / battery-save (see renderer).
    pub crt_scanlines: bool,
    /// Scanline darkness (0.0 = none .. 1.0 = strong). Tunes how dark the
    /// scanline troughs are painted; the renderer maps this to the dark-band
    /// alpha. Default [`DEFAULT_SCANLINE_DARKNESS`] reads as distinct lines
    /// rather than a flat grey film.
    #[serde(default = "default_scanline_darkness")]
    pub scanline_darkness: f32,
    /// Explicit chromatic-aberration ON/OFF toggle. Distinct from
    /// [`EffectsConfig::chromatic_aberration`] (the intensity) so the UI is a
    /// checkbox + an enabled-gated intensity slider rather than a single
    /// slider the user reads as "broken" when it sits at 0. Default OFF.
    #[serde(default)]
    pub chromatic_aberration_enabled: bool,
    /// Chromatic-aberration intensity (0.0 = off). Only applied when
    /// [`EffectsConfig::chromatic_aberration_enabled`] is `true`.
    pub chromatic_aberration: f32,
}

/// Default scanline darkness — strong enough to read as scan lines (not a flat
/// dimming film). Free function so `#[serde(default = ...)]` can name it.
pub fn default_scanline_darkness() -> f32 {
    DEFAULT_SCANLINE_DARKNESS
}

/// Default scanline-darkness value (≈38% trough darkening at the band centre).
pub const DEFAULT_SCANLINE_DARKNESS: f32 = 0.4;

/// The visible default chromatic-aberration intensity applied on first enable,
/// so flipping the toggle shows the effect immediately instead of a no-op at 0.
pub const DEFAULT_CHROMATIC_INTENSITY: f32 = 0.6;

impl Default for EffectsConfig {
    fn default() -> Self {
        EffectsConfig {
            crt_scanlines: false,
            scanline_darkness: DEFAULT_SCANLINE_DARKNESS,
            chromatic_aberration_enabled: false,
            chromatic_aberration: 0.0,
        }
    }
}

impl EffectsConfig {
    /// The effective chromatic-aberration intensity: the configured intensity
    /// only when the explicit toggle is on, else `0.0`. The single predicate the
    /// renderer consults so the toggle is honoured uniformly.
    pub fn effective_chromatic(&self) -> f32 {
        if self.chromatic_aberration_enabled {
            self.chromatic_aberration.max(0.0)
        } else {
            0.0
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
    /// Font configuration (family, size, line height, fallback chain).
    pub font: FontConfig,
    /// Number of scrollback lines retained per pane.
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
    /// Which side the command-history quick-run sidebar (`#21`) docks to when
    /// opened. Default [`PanelSide::Right`].
    #[serde(default)]
    pub history_sidebar_side: PanelSide,
    /// Pane shell layout: the multi-pane `egui_tiles` grid (default) or a single
    /// full-size pane with the tab strip switching panes. Toggled by the titlebar
    /// view button; persisted so the choice survives a relaunch.
    #[serde(default)]
    pub view_mode: ViewMode,
    /// Show the neofetch-style startup panel (logo + system info) on launch.
    pub startup_panel: bool,
    /// Override shell program; `None` = use the platform default shell.
    pub shell: Option<String>,
    /// The `TERM` value advertised to the spawned child shell (and every TUI it
    /// runs). Defaults to [`DEFAULT_TERM`] (`xterm-256color`), which matches what
    /// the emulator advertises on the wire (its DA / XTGETTCAP responses). Set
    /// this only if you need the child to see a different terminfo entry; an
    /// empty string falls back to the default. `COLORTERM` is always `truecolor`
    /// (the emulator renders 24-bit colour) and is not configurable.
    #[serde(default = "default_term")]
    pub term: String,
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
    /// Capture typed commands into the (in-memory) command history that feeds the
    /// Ctrl+Shift+P palette + the history sidebar. `true` (default) records
    /// echoed commands (passwords + inline secrets are excluded/redacted
    /// upstream). Set `false` for a no-history privacy posture; the
    /// per-session Incognito toggle forces this off regardless.
    #[serde(default = "default_true")]
    pub history_capture_enabled: bool,
}

/// serde default for boolean fields that should default to `true` when absent
/// from an older on-disk config (so upgrading never silently disables a feature).
fn default_true() -> bool {
    true
}

/// serde default for [`Config::term`] — the canonical `TERM` the emulator
/// advertises. Delegates to [`crate::pty::DEFAULT_TERM`] so the config default
/// and the PTY spawn default share one source of truth.
fn default_term() -> String {
    crate::pty::DEFAULT_TERM.to_string()
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
            history_sidebar_side: PanelSide::Right,
            view_mode: ViewMode::default(),
            startup_panel: true,
            shell: None,
            term: default_term(),
            ligatures: false,
            copy_on_select: false,
            paste_warn_multiline: true,
            history_capture_enabled: true,
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
    ///
    /// The file is created **owner-only** from the start (roadmap P-V2): `0600`
    /// on Unix, an owner-only DACL on Windows. The config may reflect the user's
    /// environment, so other local accounts should not be able to read it.
    ///
    /// The write goes through [`atomic_write_owner_only`], which writes the body
    /// to a sibling temp file, tightens it (on Unix `0600` is applied to the temp
    /// file **before** the rename), then atomically renames it over the
    /// destination. This closes the previous race where `std::fs::write` then
    /// `restrict_to_owner` left a brief window in which the file carried default
    /// (umask/inherited) permissions (audit P3-#2). Permission tightening itself
    /// remains BEST-EFFORT — a restrictive filesystem can never block a save —
    /// but is no longer applied after the content already exists world-readable.
    pub fn save_to(&self, path: &Path) -> Result<(), ConfigError> {
        let body = self.to_toml()?;
        // `atomic_write_owner_only` creates parent dirs, writes to a sibling
        // temp file, tightens perms (Unix: on the temp file pre-rename; Windows:
        // on the final path post-rename), and renames atomically — so the
        // destination never exists in a world-readable, default-perms state.
        crate::atomic_write::atomic_write_owner_only(path, body.as_bytes()).map_err(|e| {
            ConfigError::Io {
                path: path.to_path_buf(),
                source: e,
            }
        })?;
        Ok(())
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

    #[test]
    fn term_defaults_to_xterm_256color() {
        let c = Config::default();
        assert_eq!(
            c.term, "xterm-256color",
            "default TERM must match the emulator's on-the-wire identity"
        );
        // The config default shares one source of truth with the PTY spawn default.
        assert_eq!(c.term, crate::pty::DEFAULT_TERM);
    }

    #[test]
    fn term_parses_from_partial_toml_and_old_configs_backfill() {
        // An explicit override parses.
        let p = PathBuf::from("test.toml");
        let c = Config::from_toml("term = \"screen-256color\"\n", &p).unwrap();
        assert_eq!(c.term, "screen-256color");
        assert_eq!(c.theme, "itasha-corp"); // untouched
                                            // A config file with no `term` key backfills the default via serde(default).
        let c2 = Config::from_toml("theme = \"ghost-paper\"\n", &p).unwrap();
        assert_eq!(c2.term, "xterm-256color");
    }

    #[test]
    fn view_mode_defaults_to_grid() {
        assert_eq!(Config::default().view_mode, ViewMode::Grid);
    }

    #[test]
    fn view_mode_toggles_between_grid_and_tabs() {
        assert_eq!(ViewMode::Grid.toggled(), ViewMode::Tabs);
        assert_eq!(ViewMode::Tabs.toggled(), ViewMode::Grid);
        // Two toggles return to the start (involution).
        assert_eq!(ViewMode::Grid.toggled().toggled(), ViewMode::Grid);
    }

    #[test]
    fn view_mode_round_trips_through_toml() {
        // A config that only sets view_mode parses via serde(default), and the
        // lowercase rename matches the serialized form.
        let p = PathBuf::from("test.toml");
        let c = Config::from_toml("view_mode = \"tabs\"\n", &p).unwrap();
        assert_eq!(c.view_mode, ViewMode::Tabs);
        // Default theme untouched.
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

    // ---- Effects (CRT scanlines + chromatic aberration) ----

    #[test]
    fn effects_defaults_are_off_with_visible_scanline_darkness() {
        let e = EffectsConfig::default();
        assert!(!e.crt_scanlines, "scanlines opt-in");
        assert!(!e.chromatic_aberration_enabled, "chromatic opt-in");
        assert_eq!(e.chromatic_aberration, 0.0);
        assert!(
            (e.scanline_darkness - DEFAULT_SCANLINE_DARKNESS).abs() < f32::EPSILON,
            "default darkness reads as lines, not a flat film"
        );
    }

    #[test]
    fn effective_chromatic_is_gated_by_the_explicit_toggle() {
        // Intensity set but toggle off => effectively off (no "slider at 0.6 but
        // does nothing because the checkbox is unchecked" surprise — the toggle
        // is authoritative).
        let mut e = EffectsConfig {
            chromatic_aberration: 0.6,
            ..EffectsConfig::default()
        };
        assert_eq!(e.effective_chromatic(), 0.0, "toggle off => no aberration");
        // Toggle on => the intensity applies.
        e.chromatic_aberration_enabled = true;
        assert!((e.effective_chromatic() - 0.6).abs() < f32::EPSILON);
        // Negative intensity is floored to 0 even when enabled.
        e.chromatic_aberration = -1.0;
        assert_eq!(
            e.effective_chromatic(),
            0.0,
            "negative intensity floors to 0"
        );
    }

    #[test]
    fn old_config_without_new_effects_fields_still_parses() {
        // A pre-toggle config that only set crt_scanlines + the f32 intensity
        // must keep loading via serde(default), backfilling the new fields.
        let p = PathBuf::from("test.toml");
        let c = Config::from_toml(
            "[effects]\ncrt_scanlines = true\nchromatic_aberration = 0.5\n",
            &p,
        )
        .unwrap();
        assert!(c.effects.crt_scanlines);
        assert!((c.effects.chromatic_aberration - 0.5).abs() < f32::EPSILON);
        // The new toggle defaults OFF, so an old file's intensity stays inert
        // until the user opts in — backward-compatible.
        assert!(!c.effects.chromatic_aberration_enabled);
        assert!(
            (c.effects.scanline_darkness - DEFAULT_SCANLINE_DARKNESS).abs() < f32::EPSILON,
            "missing darkness backfills to the visible default"
        );
    }

    #[test]
    fn new_effects_fields_round_trip_through_toml() {
        let p = PathBuf::from("test.toml");
        let c = Config::from_toml(
            "[effects]\ncrt_scanlines = true\nscanline_darkness = 0.55\nchromatic_aberration_enabled = true\nchromatic_aberration = 0.8\n",
            &p,
        )
        .unwrap();
        assert!(c.effects.crt_scanlines);
        assert!((c.effects.scanline_darkness - 0.55).abs() < f32::EPSILON);
        assert!(c.effects.chromatic_aberration_enabled);
        assert!((c.effects.effective_chromatic() - 0.8).abs() < f32::EPSILON);
    }

    #[test]
    fn save_to_round_trips_and_creates_parent_dirs() {
        // A save followed by a load reproduces the persisted fields, and the
        // missing parent directory is created (the atomic-write path handles it).
        let dir = std::env::temp_dir()
            .join(format!("c0pl4nd-cfg-{}-rt", std::process::id()))
            .join("nested");
        let _ = std::fs::remove_dir_all(dir.parent().unwrap());
        let path = dir.join("config.toml");
        let c = Config {
            theme: "ghost-paper".to_string(),
            ..Config::default()
        };
        c.save_to(&path).expect("save");
        let loaded = Config::load_from(&path).expect("load");
        assert_eq!(loaded.theme, "ghost-paper");
        let _ = std::fs::remove_dir_all(dir.parent().unwrap());
    }

    #[cfg(unix)]
    #[test]
    fn save_to_writes_owner_only_0600_no_race() {
        // Audit P3-#2: the saved config must be owner-only (0600) — and because
        // it is created via atomic_write_owner_only, the file NEVER exists in a
        // world-readable default-perms state (the perms are applied to the temp
        // file before the rename, not after the content already exists).
        use std::os::unix::fs::PermissionsExt;
        let path =
            std::env::temp_dir().join(format!("c0pl4nd-cfg-{}-perms.toml", std::process::id()));
        let _ = std::fs::remove_file(&path);
        Config::default().save_to(&path).expect("save");
        let mode = std::fs::metadata(&path).expect("stat").permissions().mode();
        assert_eq!(
            mode & 0o777,
            0o600,
            "saved config must be owner-only 0600, got {:o}",
            mode & 0o777
        );
        let _ = std::fs::remove_file(&path);
    }
}
