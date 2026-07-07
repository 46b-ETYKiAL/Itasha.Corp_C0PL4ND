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
        // JetBrains Mono is the default mono voice — and, critically, it is a
        // BUNDLED face (see the app crate's `fonts::BUNDLED_FONTS`), so the
        // zero-config default renders in the intended typeface on every machine.
        // The prior default ("Monaspace Neon") was NOT bundled, so `bundled_face`
        // returned `None` and the default silently fell back to egui's built-in
        // monospace — a latent default-font bug this fixes. An explicit user
        // `font.family` is untouched (this is the serde default only).
        FontConfig {
            family: "JetBrains Mono".to_string(),
            // 13 logical points — the industry-standard terminal grid size
            // (Windows Terminal / WezTerm sit at 12, VS Code's terminal at 14).
            // egui interprets a `FontId` size as LOGICAL points scaled by the
            // display's `pixels_per_point`, so this is the on-screen size at 100%
            // scaling and scales up cleanly on HiDPI (e.g. 26 physical px at 2×).
            // The previous 14.0 read a touch large, especially on HiDPI panels.
            size: 13.0,
            line_height: 20.0,
            fallback: vec!["Noto Sans JP".to_string(), "monospace".to_string()],
        }
    }
}

/// When (and whether) C0PL4ND checks for updates. The default is
/// [`UpdateMode::Notify`] — once per launch (when due) the app reads the public
/// GitHub Releases API and shows a passive toast if a newer version exists. The
/// check is read-only and sends zero identifiers, and it never downloads or
/// installs on its own. Users who want a fully local-first, no-network-on-launch
/// experience set `manual` (check only on demand from Settings) or `off` (never
/// touch the network for updates).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum UpdateMode {
    /// Never check, never touch the network for updates.
    Off,
    /// Default: check once per launch (when due); show a passive toast if a newer
    /// version exists. Read-only; never downloads or installs on its own.
    #[default]
    Notify,
    /// Check only when the user presses "Check for updates".
    Manual,
    /// Check once per launch (when due); download + apply a verified update when
    /// one is found.
    Auto,
}

/// Update behaviour. The default mode is `notify`, so on launch (when due)
/// C0PL4ND performs a single read-only GitHub-Releases version check and shows
/// a passive toast if a newer version exists. Set `manual` (check only on
/// demand from Settings or `c0pl4nd update`) or `off` (never touch the network)
/// for a fully local-first experience.
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
            mode: UpdateMode::Notify,
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

/// A problem found in a [`Keybindings`] set by [`Keybindings::validate`] (F5-1).
///
/// The bindings are user-editable, so two actions can end up bound to the SAME
/// combo (only one would ever fire) or a binding can be left blank (the action
/// becomes unreachable) — both silently, with no surfacing. `validate` makes
/// these explicit so the settings UI can warn instead of the user wondering why
/// a shortcut "does nothing".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeybindingIssue {
    /// `action` has an empty / whitespace-only combo — it can never trigger.
    Empty { action: &'static str },
    /// `actions` (≥2) are all bound to the same `combo` (normalized) — they
    /// collide; at most one can win.
    Conflict {
        combo: String,
        actions: Vec<&'static str>,
    },
}

impl KeybindingIssue {
    /// A human-readable, settings-surfaceable description of the issue.
    pub fn message(&self) -> String {
        match self {
            KeybindingIssue::Empty { action } => {
                format!("'{action}' has no key bound — it cannot be triggered")
            }
            KeybindingIssue::Conflict { combo, actions } => {
                format!(
                    "'{combo}' is bound to multiple actions: {}",
                    actions.join(", ")
                )
            }
        }
    }
}

impl Keybindings {
    /// Every (action-name, combo) pair, in a stable declaration order. The
    /// single source of truth both [`Keybindings::validate`] and any UI iteration
    /// key off, so a new binding is covered by adding ONE line here.
    pub fn entries(&self) -> [(&'static str, &str); 12] {
        [
            ("copy", &self.copy),
            ("paste", &self.paste),
            ("new_tab", &self.new_tab),
            ("close_tab", &self.close_tab),
            ("next_tab", &self.next_tab),
            ("split_right", &self.split_right),
            ("split_down", &self.split_down),
            ("search", &self.search),
            ("command_palette", &self.command_palette),
            ("history_sidebar", &self.history_sidebar),
            ("increase_font", &self.increase_font),
            ("decrease_font", &self.decrease_font),
        ]
    }

    /// Canonical form of a combo for conflict comparison: lowercased, trimmed,
    /// split on `+`, empties dropped, tokens sorted — so `"shift+mod+c"` and
    /// `"mod+shift+c"` compare equal. An all-empty combo normalizes to `""`.
    fn normalize_combo(combo: &str) -> String {
        let mut parts: Vec<String> = combo
            .split('+')
            .map(|p| p.trim().to_ascii_lowercase())
            .filter(|p| !p.is_empty())
            .collect();
        parts.sort();
        parts.join("+")
    }

    /// Detect keybinding issues: blank bindings (unreachable actions) and combos
    /// bound to more than one action (collisions). Returns an empty Vec when the
    /// set is clean — the default set is clean by construction. Pure + order-
    /// deterministic (empties first in declaration order, then conflicts sorted
    /// by combo) so the settings surfacing is stable frame-to-frame.
    pub fn validate(&self) -> Vec<KeybindingIssue> {
        let entries = self.entries();
        let mut issues = Vec::new();

        // Blank bindings: an action with no resolvable combo can never fire.
        for (name, combo) in entries.iter() {
            if Self::normalize_combo(combo).is_empty() {
                issues.push(KeybindingIssue::Empty { action: name });
            }
        }

        // Collisions: group non-empty bindings by their normalized combo.
        let mut groups: Vec<(String, Vec<&'static str>)> = Vec::new();
        for (name, combo) in entries.iter() {
            let norm = Self::normalize_combo(combo);
            if norm.is_empty() {
                continue;
            }
            if let Some(slot) = groups.iter_mut().find(|(c, _)| *c == norm) {
                slot.1.push(name);
            } else {
                groups.push((norm, vec![name]));
            }
        }
        groups.sort_by(|a, b| a.0.cmp(&b.0));
        for (combo, actions) in groups {
            if actions.len() > 1 {
                issues.push(KeybindingIssue::Conflict { combo, actions });
            }
        }

        issues
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

/// Which physical GPU the renderer prefers at startup, on a multi-GPU (hybrid /
/// Optimus) machine. `Auto` (default) keeps the platform/eframe default
/// (high-performance / discrete). `Integrated` forces the low-power integrated
/// GPU and `Discrete` forces the high-performance one — the escape hatch when one
/// GPU's driver corrupts rendering (the canonical case: a laptop discrete-GPU
/// driver garbles the terminal-grid glyph atlas while the integrated GPU renders
/// it perfectly). Maps to eframe's wgpu `PowerPreference`. Applies on restart; the
/// `WGPU_POWER_PREF` env var still overrides it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum GpuPreference {
    /// Platform/eframe default (high-performance / discrete GPU).
    #[default]
    Auto,
    /// Force the low-power integrated GPU (e.g. Intel iGPU on an Optimus laptop).
    Integrated,
    /// Force the high-performance discrete GPU.
    Discrete,
}

/// GPU backend the renderer requests at startup. `Auto` (default) keeps the
/// platform-smart choice; the explicit variants force a specific wgpu backend to
/// work around a driver-specific rendering glitch (the canonical case: corrupted
/// terminal-grid glyphs under a bad Windows DX12 driver, fixed by switching to
/// Vulkan). Serialized lowercase (`auto` / `dx12` / `vulkan` / `gl`). The choice
/// applies on restart (the backend is chosen once, before the GPU device is
/// created); the `WGPU_BACKEND` env var still overrides it for one-off debugging.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum GraphicsBackend {
    /// Platform-smart default: DX12 on Windows (Vulkan when window transparency
    /// is enabled), wgpu's platform default elsewhere.
    #[default]
    Auto,
    /// Direct3D 12 (Windows).
    Dx12,
    /// Vulkan (Windows / Linux). Required for real window transparency on Windows.
    Vulkan,
    /// OpenGL — a last-resort fallback when neither DX12 nor Vulkan renders
    /// correctly.
    Gl,
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

    // ---- Motion / animation (SCR1B3 parity) --------------------------------
    /// Master switch for the whole animation + motion-effect system. When
    /// `false`, egui's animation time is zeroed (instant transitions, a fully
    /// static UI) and every motion overlay below is suppressed regardless of its
    /// own toggle. Default ON — preserves the current animated feel; a user who
    /// wants a static surface flips this off.
    #[serde(default = "default_animations_enabled")]
    pub animations_enabled: bool,
    /// 0.0..=1.0 scale applied to egui's global animation time when
    /// [`animations_enabled`](Self::animations_enabled) is on. 1.0 = egui's
    /// default speed (the shipped feel); lower slows fades/collapses.
    #[serde(default = "default_animation_intensity")]
    pub animation_intensity: f32,
    /// Subtle full-window CRT-style brightness flicker. OFF by default; gated
    /// behind the master switch.
    #[serde(default)]
    pub flicker: bool,
    /// Flicker strength (0.0 = none .. capped at 0.20 for accessibility).
    #[serde(default = "default_flicker_strength")]
    pub flicker_strength: f32,
    /// VHS-style horizontal tracking lines drifting down the window. OFF by
    /// default.
    #[serde(default)]
    pub vhs_tracking: bool,
    /// VHS tracking-line intensity (0.0 = faint .. 1.0 = bold, clamped). Scales
    /// how bright the drifting tracking bands read. Only applied when
    /// [`vhs_tracking`](Self::vhs_tracking) is on.
    #[serde(default = "default_vhs_intensity")]
    pub vhs_intensity: f32,
    /// Animated wired node-mesh ambient background (Lain "Wired" feel), drawn at
    /// Background order behind the panes. OFF by default.
    #[serde(default)]
    pub wired_ambient: bool,
    /// Node-mesh density (0.0 = sparse .. 2.0 = dense, clamped). Drives the node
    /// count of the wired-ambient background.
    #[serde(default = "default_mesh_density")]
    pub mesh_density: f32,
    /// Node-mesh brightness (0.0 = invisible .. 1.0 = shipped .. 3.0 = bold,
    /// clamped). Scales the lattice link + node-dot opacity so the mesh can be
    /// dimmed toward nothing or brightened to clearly pop. Only applied when
    /// [`wired_ambient`](Self::wired_ambient) is on.
    #[serde(default = "default_mesh_brightness")]
    pub mesh_brightness: f32,
    /// Node-mesh movement amount (0.0 = a still lattice .. 1.0 = the shipped
    /// drift .. 2.0 = brisk, clamped). Scales how fast the mesh nodes drift; at
    /// 0 the field holds a static frame. Only applied when
    /// [`wired_ambient`](Self::wired_ambient) is on.
    #[serde(default = "default_mesh_speed")]
    pub mesh_speed: f32,
    /// Cursor ghost-trail: a fading echo follows the focused terminal cursor as
    /// it moves. OFF by default.
    #[serde(default)]
    pub cursor_trail: bool,
    /// Cursor ghost-trail intensity (0.0 = faint/short .. 1.0 = bold/long,
    /// clamped). Scales both the echo opacity and how long each echo lingers, so
    /// the trail can be tuned from a barely-there whisper to a pronounced comet
    /// tail. Only applied when [`cursor_trail`](Self::cursor_trail) is on.
    #[serde(default = "default_cursor_trail_intensity")]
    pub cursor_trail_intensity: f32,
    /// One-shot boot "glitch" sweep on the first frames after launch. OFF by
    /// default; self-terminates.
    #[serde(default)]
    pub boot_glitch: bool,
}

/// Default master-animation switch — ON, preserving the shipped animated feel.
pub fn default_animations_enabled() -> bool {
    true
}

/// Default animation-intensity — 1.0 = egui's stock animation speed (no change
/// to the current feel until the user tunes it down).
pub fn default_animation_intensity() -> f32 {
    1.0
}

/// Default flicker strength — barely perceptible.
pub fn default_flicker_strength() -> f32 {
    0.06
}

/// Default node-mesh density — a calm, sparse field.
pub fn default_mesh_density() -> f32 {
    0.4
}

/// Default node-mesh brightness — 1.0 = the shipped lattice opacity (no change
/// to the current look until the user brightens or dims it).
pub fn default_mesh_brightness() -> f32 {
    1.0
}

/// Default cursor-trail intensity — a clearly-visible-but-tasteful comet tail
/// (mid-high so the trail reads at once when first enabled, tunable either way).
pub fn default_cursor_trail_intensity() -> f32 {
    0.6
}

/// Default node-mesh movement — 1.0 = the shipped drift speed (no change to the
/// current feel until the user tunes it).
pub fn default_mesh_speed() -> f32 {
    1.0
}

/// Default VHS tracking-line intensity — a clearly-visible-but-calm band.
pub fn default_vhs_intensity() -> f32 {
    0.5
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
            animations_enabled: default_animations_enabled(),
            animation_intensity: default_animation_intensity(),
            flicker: false,
            flicker_strength: default_flicker_strength(),
            vhs_tracking: false,
            vhs_intensity: default_vhs_intensity(),
            wired_ambient: false,
            mesh_density: default_mesh_density(),
            mesh_brightness: default_mesh_brightness(),
            mesh_speed: default_mesh_speed(),
            cursor_trail: false,
            cursor_trail_intensity: default_cursor_trail_intensity(),
            boot_glitch: false,
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

    /// Animation-intensity clamped to `0.0..=2.0`. 1.0 is egui's stock feel; the
    /// band extends to 2.0 so the Motion → Animation-speed slider can drive the
    /// continuous drift overlays (mesh / VHS / flicker) up to double-rate. The
    /// egui-chrome consumer separately caps the factor at 1.0 (see `mod.rs`) so a
    /// >1.0 speed only accelerates the effects, never lengthens the UI fades.
    pub fn clamped_animation_intensity(&self) -> f32 {
        self.animation_intensity.clamp(0.0, 2.0)
    }

    /// Flicker strength clamped to `0.0..=1.0`. The old `0.20` ceiling read as a
    /// barely-there shimmer; 1.0 lets the flicker reach a clearly-visible CRT
    /// wobble. The painter's own alpha math keeps even the max well short of a
    /// full-black strobe (photosensitivity guard), so the band is safe to widen.
    pub fn clamped_flicker_strength(&self) -> f32 {
        self.flicker_strength.clamp(0.0, 1.0)
    }

    /// Node-mesh density clamped to `0.0..=2.0` so the mesh can go from a calm
    /// field to a busy web (the painter caps the node count for the O(n²) pass).
    pub fn clamped_mesh_density(&self) -> f32 {
        self.mesh_density.clamp(0.0, 2.0)
    }

    /// Node-mesh brightness clamped to `0.0..=3.0`. 1.0 is the shipped alpha;
    /// below 1.0 dims the lattice toward invisible, above 1.0 brightens it so a
    /// user who finds the default mesh too dim can make it clearly pop. Scales the
    /// link + dot alpha in the painter.
    pub fn clamped_mesh_brightness(&self) -> f32 {
        self.mesh_brightness.clamp(0.0, 3.0)
    }

    /// Cursor-trail intensity clamped to `0.0..=2.0` so the echo opacity /
    /// lifetime can reach a pronounced comet tail while a malformed config still
    /// can't drive it out of band.
    pub fn clamped_cursor_trail_intensity(&self) -> f32 {
        self.cursor_trail_intensity.clamp(0.0, 2.0)
    }

    /// Node-mesh movement clamped to `0.0..=2.0`. 0 holds a static lattice; 1.0
    /// is the shipped drift; up to 2.0 is a brisk field. Scales the mesh
    /// animation phase in the painter.
    pub fn clamped_mesh_speed(&self) -> f32 {
        self.mesh_speed.clamp(0.0, 2.0)
    }

    /// VHS tracking-line intensity clamped to `0.0..=1.0`. Scales the drifting
    /// tracking-band alpha in the painter.
    pub fn clamped_vhs_intensity(&self) -> f32 {
        self.vhs_intensity.clamp(0.0, 1.0)
    }
}

/// Customizable top-bar quick-action toolbar (the cluster pinned to the RIGHT of
/// the titlebar, immediately left of the settings gear). The user chooses which
/// quick actions appear, in what order, and which are parked in an overflow "⋯"
/// menu — edited in Settings → Toolbar. Action ids are from the app's
/// `TOOLBAR_ACTIONS` catalog; an unknown id is skipped at render time (so a config
/// written by a newer/older build never breaks the bar). The fixed affordances
/// (wordmark, tab strip, the tab-adjacent "+", and the window caption cluster) are
/// NOT part of this list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ToolbarConfig {
    /// Actions in the LEFT group (titlebar flow, after the "+"), left→right order.
    pub left: Vec<String>,
    /// Actions in the RIGHT cluster (pinned by the settings gear), left→right (the
    /// LAST id renders nearest the gear).
    pub right: Vec<String>,
    /// Actions parked in the overflow "⋯" menu instead of taking a bar slot.
    pub menu: Vec<String>,
    /// Show the overflow "⋯" menu button when [`menu`](Self::menu) is non-empty.
    /// Default `true`; turn off to hide the overflow button entirely (parked
    /// actions stay reachable via the command palette / keybindings).
    pub show_overflow: bool,
}

impl ToolbarConfig {
    /// The shipped default LEFT group (titlebar flow, after the "+"): view-toggle,
    /// equalize, shell-switcher — exactly where they were before the toolbar
    /// became customizable.
    pub fn default_left() -> Vec<String> {
        ["view_mode", "equalize_panes", "shell_switcher"]
            .iter()
            .map(|s| s.to_string())
            .collect()
    }

    /// The shipped default RIGHT cluster: just the script launcher, pinned next to
    /// the settings gear.
    pub fn default_right() -> Vec<String> {
        vec!["script_launcher".to_string()]
    }
}

impl Default for ToolbarConfig {
    fn default() -> Self {
        ToolbarConfig {
            left: ToolbarConfig::default_left(),
            right: ToolbarConfig::default_right(),
            menu: Vec::new(),
            show_overflow: true,
        }
    }
}

/// W1TN3SS manual "Report an issue" coordinates: the GitHub `owner/repo` the
/// prefilled Issue-Form deep link targets, and the support email alias the
/// `mailto:` fallback addresses. Both have sane C0PL4ND defaults and are
/// overridable in config. NO persistent identifier and NO transport state is
/// ever stored here — only the public repo coordinates the deep link needs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct IssueIntakeConfig {
    /// The GitHub `owner/repo` the prefilled Issue-Form deep link targets.
    pub repo: String,
    /// The support email alias the `mailto:` fallback addresses.
    pub mailto_alias: String,
}

impl Default for IssueIntakeConfig {
    fn default() -> Self {
        IssueIntakeConfig {
            repo: "46b-ETYKiAL/Itasha.Corp_C0PL4ND".to_string(),
            mailto_alias: "46b.AbandonSomething@proton.me".to_string(),
        }
    }
}

/// Opt-in reporting configuration. Wraps the W1TN3SS SDK's two-stream
/// [`itasha_report_core::config::ReportingConfig`] (crash + manual-issue
/// consent posture, **both default OFF**) and adds the manual-issue repo
/// coordinates. This block is **additive** to the on-disk config (serde
/// `default`) — an older config file with no `[reporting]` table loads with
/// both streams OFF, so upgrading never silently enables any reporting.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ReportingConfig {
    /// Per-stream consent posture (crash_reports + manual_issues). **Default
    /// OFF** for every stream — the privacy-default; nothing transmits until
    /// the user explicitly opts in.
    pub streams: itasha_report_core::config::ReportingConfig,
    /// Manual "Report an issue" deep-link coordinates.
    pub issue_intake: IssueIntakeConfig,
}

impl Default for ReportingConfig {
    /// Both streams OFF (the SDK's privacy-default), default issue-intake coords.
    fn default() -> Self {
        ReportingConfig {
            streams: itasha_report_core::config::ReportingConfig::all_off(),
            issue_intake: IssueIntakeConfig::default(),
        }
    }
}

/// The default window tint color (`#RRGGBB`) — brand-canon VOID BLACK, a
/// near-black, marginally violet-tinted void wash matching the brand background.
/// A free function so `#[serde(default = ...)]` can name it for the `tint` field.
///
/// Superseded the earlier `#121212` default; a stored config still carrying the
/// old `#121212` default is remapped to this value once by [`Config::migrate`]
/// (v1 → v2), while any user-customised tint is left untouched.
fn default_tint() -> String {
    "#08060d".to_string()
}

/// The pre-v2 default window tint (`#121212`). A stored config whose tint is
/// PROVABLY this old default is remapped to [`default_tint`] by the v1 → v2
/// migration; any other value (a user's custom tint) is preserved verbatim.
const LEGACY_DEFAULT_TINT: &str = "#121212";

/// Current config schema version. Bumped whenever a one-time, version-gated
/// migration is needed (see [`Config::migrate`]). A config written before schema
/// versioning existed deserializes with `schema_version == 0` (the serde default
/// for a missing field) and is migrated up on load.
///
/// - v1 (legacy-transparency model promotion): the pre-modes `opacity < 1.0` /
///   `acrylic = true` transparency signals are promoted to the
///   `transparency_enabled` + `window_mode` model exactly once, so an existing
///   config keeps its appearance. This wraps the pre-existing
///   [`Config::migrate_legacy_transparency`] in a `schema_version < 1` gate — no
///   behaviour is lost; it simply runs once and is then recorded as migrated.
/// - v2 (brand-canon default tint remap): a stored config whose `tint` is
///   PROVABLY the old `#121212` default is remapped to the new `#08060d`
///   (VOID BLACK) brand-canon default exactly once. A user's custom tint (any
///   value ≠ `#121212`) is left UNTOUCHED — no silent clobber. Gated on
///   `schema_version < 2`.
pub const CURRENT_SCHEMA_VERSION: u32 = 2;

/// Top-level configuration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// One-time-migration schema version. A fresh config is born at
    /// [`CURRENT_SCHEMA_VERSION`]; an existing config written before versioning
    /// loads as `0` (serde default for the missing field) and [`Config::migrate`]
    /// brings it forward exactly once. NEVER hand-edit downward.
    #[serde(default)]
    pub schema_version: u32,
    /// Name of the theme to load (matches a file stem in the themes dir).
    pub theme: String,
    /// Font configuration (family, size, line height, fallback chain).
    pub font: FontConfig,
    /// Persisted UI scale / accessibility zoom (F2-3): a multiplier applied to
    /// the WHOLE interface (chrome + grid), distinct from the transient Ctrl+/-
    /// keyboard zoom which is not persisted. `1.0` = 100%. Clamped to
    /// `0.5..=3.0` on use via [`Config::effective_ui_scale`] so a malformed value
    /// can never make the UI unusably tiny or huge. Defaults to `1.0`.
    #[serde(default = "default_ui_scale")]
    pub ui_scale: f32,
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
    /// Show the bottom status bar (pane count + hints). `true` (default) shows it;
    /// set `false` to hide it and reclaim the row for the terminal grid.
    #[serde(default = "default_true")]
    pub show_status_bar: bool,
    /// Customizable top-bar quick-action toolbar (right cluster, by the gear). See
    /// [`ToolbarConfig`]; edited in Settings → Toolbar.
    #[serde(default)]
    pub toolbar: ToolbarConfig,
    /// GPU backend the renderer requests at startup. [`GraphicsBackend::Auto`]
    /// (default) keeps the platform-smart choice (DX12 on Windows, Vulkan when
    /// window transparency is on; wgpu default elsewhere). Set an explicit
    /// backend to work around a driver-specific rendering glitch (e.g. corrupted
    /// glyphs under a bad DX12 driver → switch to Vulkan). Applied on restart;
    /// the `WGPU_BACKEND` env var still overrides this.
    #[serde(default)]
    pub graphics_backend: GraphicsBackend,
    /// Which physical GPU to prefer on a multi-GPU (hybrid / Optimus) machine.
    /// [`GpuPreference::Auto`] (default) uses the platform default; set
    /// `Integrated` to force the low-power iGPU when the discrete GPU's driver
    /// corrupts rendering (the terminal-grid glyph garble). Applied on restart;
    /// the `WGPU_POWER_PREF` env var still overrides this.
    #[serde(default)]
    pub graphics_gpu: GpuPreference,
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
    /// Keep split-pane dividers LINKED so every sibling pane stays the same size.
    /// When `true`, the dividers are held at equal positions each frame — drag one
    /// and they hold equal ("move together"). Defaults to `false` so panes are
    /// freely resizable; the top-bar "make symmetrical" button equalises once
    /// regardless of this setting.
    #[serde(default)]
    pub link_pane_dividers: bool,
    /// Capture typed commands into the (in-memory) command history that feeds the
    /// Ctrl+Shift+P palette + the history sidebar. `true` (default) records
    /// echoed commands (passwords + inline secrets are excluded/redacted
    /// upstream). Set `false` for a no-history privacy posture; the
    /// per-session Incognito toggle forces this off regardless.
    #[serde(default = "default_true")]
    pub history_capture_enabled: bool,
    /// Opt-in crash/error/issue reporting (W1TN3SS). **Both streams default
    /// OFF** — nothing is captured-for-send or transmitted until the user
    /// explicitly opts in from Settings → Privacy. Additive: an older config
    /// with no `[reporting]` table loads with reporting fully off.
    #[serde(default)]
    pub reporting: ReportingConfig,
    /// Persisted size of the Settings sub-window, in logical points, so a user's
    /// resize sticks across launches. `None` (default, and for older configs
    /// without the field) = use the built-in default size. Clamped to a sane
    /// floor/ceiling on use so a malformed value can never spawn an unusable
    /// window.
    #[serde(default)]
    pub settings_win_w: Option<f32>,
    #[serde(default)]
    pub settings_win_h: Option<f32>,
    /// Persisted top-left POSITION of the Settings sub-window, in logical points,
    /// so a user's move sticks across launches. `None` (default) = no saved
    /// position yet, so the window opens centered over the app. Clamped to the
    /// live screen on use so a stale off-screen value can never hide the window.
    #[serde(default)]
    pub settings_win_x: Option<f32>,
    #[serde(default)]
    pub settings_win_y: Option<f32>,
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

/// serde default for [`Config::ui_scale`] — 100% (no scaling).
fn default_ui_scale() -> f32 {
    1.0
}

impl Default for Config {
    fn default() -> Self {
        Config {
            // A FRESH config is born already at the current schema version, so
            // `migrate` is a no-op for new users (no spurious first-run rewrite).
            // Only an EXISTING file (which deserializes `schema_version` to 0) is
            // migrated forward on load.
            schema_version: CURRENT_SCHEMA_VERSION,
            theme: "itasha-corp".to_string(),
            font: FontConfig::default(),
            ui_scale: default_ui_scale(),
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
            link_pane_dividers: false,
            show_status_bar: true,
            toolbar: ToolbarConfig::default(),
            graphics_backend: GraphicsBackend::default(),
            graphics_gpu: GpuPreference::default(),
            shell: None,
            term: default_term(),
            ligatures: false,
            copy_on_select: false,
            paste_warn_multiline: true,
            history_capture_enabled: true,
            reporting: ReportingConfig::default(),
            settings_win_w: None,
            settings_win_h: None,
            settings_win_x: None,
            settings_win_y: None,
        }
    }
}

impl Config {
    /// The UI scale to actually apply (F2-3), clamped to a safe `0.5..=3.0` and
    /// guarded against a non-finite (NaN/inf) value from a malformed config — so
    /// a bad `ui_scale` can never render the interface unusably tiny, huge, or
    /// blank. `1.0` = 100%.
    pub fn effective_ui_scale(&self) -> f32 {
        if self.ui_scale.is_finite() {
            self.ui_scale.clamp(0.5, 3.0)
        } else {
            default_ui_scale()
        }
    }

    /// Parse a TOML string into a `Config`, surfacing a readable error.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::path::Path;
    /// use c0pl4nd_core::config::Config;
    ///
    /// // An empty document yields the default config (every field optional).
    /// let cfg = Config::from_toml("", Path::new("config.toml")).unwrap();
    /// assert!(cfg.effective_ui_scale() > 0.0);
    ///
    /// // Malformed TOML is surfaced as an error, never a silent default.
    /// assert!(Config::from_toml("not valid =", Path::new("config.toml")).is_err());
    /// ```
    pub fn from_toml(src: &str, path: &Path) -> Result<Config, ConfigError> {
        let mut cfg: Config = toml::from_str(src).map_err(|e| ConfigError::Parse {
            path: path.to_path_buf(),
            message: e.to_string(),
        })?;
        cfg.migrate();
        cfg.clamp_values();
        cfg.validate()?;
        Ok(cfg)
    }

    /// Clamp out-of-range / non-finite numeric values to their documented ranges
    /// at LOAD (audit LO-3). A hand-edited or corrupt config could feed a garbage
    /// font size, a non-finite / zero line-height, an absurd update interval, or
    /// a multi-GB scrollback request straight into the renderer / scheduler,
    /// because `validate()` only checks keybindings and several fields were only
    /// clamped inconsistently at use-site. This CLAMPS (never rejects) — a bad
    /// value degrades to a sane one rather than failing the whole load, matching
    /// the graceful-degradation contract.
    fn clamp_values(&mut self) {
        // Font: size + line-height must be finite and positive; clamp to a band
        // that stays renderable. A non-finite or non-positive value resets to the
        // default before the band clamp (so `0`/`NaN`/`-1` never reach the layout).
        if !self.font.size.is_finite() || self.font.size <= 0.0 {
            self.font.size = FontConfig::default().size;
        }
        self.font.size = self.font.size.clamp(4.0, 256.0);
        if !self.font.line_height.is_finite() || self.font.line_height <= 0.0 {
            self.font.line_height = FontConfig::default().line_height;
        }
        self.font.line_height = self.font.line_height.clamp(1.0, 512.0);
        // `ui_scale`'s in-RANGE clamp intentionally stays at use-site
        // (`effective_ui_scale`); fix only finiteness here so a NaN/inf cannot
        // propagate. (`opacity` / `tint_strength` are deliberately left alone —
        // `validate()` already REJECTS an out-of-range value there, a stricter
        // contract this clamp must not soften; only the fields validate() omits
        // are clamped here.)
        if !self.ui_scale.is_finite() {
            self.ui_scale = default_ui_scale();
        }
        // Update check interval: documented 1..=168 hours.
        self.update.check_interval_hours = self.update.check_interval_hours.clamp(1, 168);
        // Scrollback: cap to a generous ceiling so a typo cannot request a
        // multi-GB history allocation.
        self.scrollback_lines = self.scrollback_lines.min(10_000_000);
    }

    /// Whether translucency should actually be rendered: the master toggle is
    /// on AND the chosen mode wants a non-opaque surface. This is the single
    /// predicate every render path consults so the master switch is honoured
    /// uniformly (surface request, the opacity pass, and the tint overlay).
    /// Mirrors SCR1B3's `WindowConfig::effective_translucent`.
    pub fn effective_translucent(&self) -> bool {
        self.transparency_enabled && self.window_mode.is_translucent()
    }

    /// Apply one-time, version-gated migrations in place. Returns `true` when
    /// anything changed (the caller should then persist).
    ///
    /// **Why this exists.** Every field is `#[serde(default)]`, so a value STORED
    /// in the user's file always wins over the source default. A good default
    /// flipped on in a later release can therefore never reach a user whose config
    /// predates the flip. Each step re-applies its intended baseline ONCE, then
    /// bumps `schema_version`, so the user's own later deliberate changes are never
    /// overridden again (the step won't re-run).
    ///
    /// v0 → v1 wraps the pre-existing [`Config::migrate_legacy_transparency`] so
    /// the legacy `opacity`/`acrylic` translucency promotion runs exactly once and
    /// is then recorded as migrated — no behaviour is lost.
    ///
    /// v1 → v2 remaps the tint ONLY when it is provably the old `#121212` default
    /// to the new `#08060d` (VOID BLACK) brand-canon default; a user's custom tint
    /// is preserved verbatim (no silent clobber).
    pub fn migrate(&mut self) -> bool {
        let original = self.schema_version;
        let mut changed = false;

        // v0 → v1: promote the pre-modes transparency signals to the
        // master-toggle + mode model. One-shot: after this, `schema_version == 1`
        // and the block is skipped, so a config that later sets the new model
        // explicitly is never re-promoted.
        if self.schema_version < 1 {
            self.migrate_legacy_transparency();
            self.schema_version = 1;
            changed = true;
        }

        // v1 → v2: remap the tint default to brand-canon VOID BLACK. Only a config
        // whose tint is PROVABLY the old `#121212` default is remapped to
        // `#08060d`; ANY user-customised tint (any value ≠ `#121212`) is left
        // untouched — no silent clobber. One-shot: after this, `schema_version == 2`
        // and the block is skipped, so a config that later sets `#121212`
        // deliberately is never re-remapped.
        if self.schema_version < 2 {
            if self.tint == LEGACY_DEFAULT_TINT {
                self.tint = default_tint();
            }
            self.schema_version = 2;
            changed = true;
        }

        // Migration invariants (debug-only): it must never LOWER the version, and
        // any config that started below the current schema must end exactly at it.
        // A FORWARD-version config (`original > CURRENT`, e.g. a file written by a
        // newer build then opened by an older one) is left untouched and
        // legitimately stays ahead — so we must NOT assert an upper bound.
        debug_assert!(self.schema_version >= original);
        debug_assert!(
            original >= CURRENT_SCHEMA_VERSION || self.schema_version == CURRENT_SCHEMA_VERSION
        );
        changed
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

    /// The per-user C0PL4ND data directory (the PARENT of `config.toml`) — the
    /// root the W1TN3SS report spool (`reports/`) is created under. Returns
    /// `None` when no config path resolves (no `%APPDATA%` / `$HOME`). Shares
    /// the resolution with [`Config::default_path`] so the spool, the config
    /// file, and the crash logs all live under one per-user dir.
    pub fn config_dir() -> Option<PathBuf> {
        Self::default_path().and_then(|p| p.parent().map(Path::to_path_buf))
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
        // Surface a save failure (audit LO-4): previously `.ok()?` swallowed it
        // silently, unlike the loader's `tracing::warn!` convention, so a
        // persistently-unwritable config dir lost window geometry with no trace.
        if let Err(e) = cfg.save_to(&path) {
            tracing::warn!("failed to persist window geometry to {path:?}: {e}");
            return None;
        }
        Some(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamp_values_bounds_out_of_range_fields() {
        // audit LO-3: out-of-range / non-finite numeric fields clamp at load to
        // their documented ranges (never reach the renderer / scheduler raw).
        let mut cfg = Config::default();
        cfg.font.size = 0.0; // non-positive → reset + band-clamp
        cfg.font.line_height = -10.0; // negative → reset
        cfg.ui_scale = f32::INFINITY; // non-finite → reset
        cfg.update.check_interval_hours = 9999; // > 168
        cfg.scrollback_lines = 999_999_999_999; // absurd
        cfg.clamp_values();
        assert!(
            (4.0..=256.0).contains(&cfg.font.size),
            "font.size clamped: {}",
            cfg.font.size
        );
        assert!(cfg.font.line_height >= 1.0, "line_height positive");
        assert!(cfg.ui_scale.is_finite(), "ui_scale finite");
        assert!(
            (1..=168).contains(&cfg.update.check_interval_hours),
            "interval clamped"
        );
        assert!(cfg.scrollback_lines <= 10_000_000, "scrollback capped");
        // opacity / tint_strength are intentionally NOT clamped here — validate()
        // rejects an out-of-range value, a stricter contract this must not soften.
    }

    #[test]
    fn defaults_are_sane_and_valid() {
        let c = Config::default();
        assert_eq!(c.theme, "itasha-corp");
        assert_eq!(c.scrollback_lines, 10_000);
        assert!(c.validate().is_ok());
    }

    #[test]
    fn effects_motion_defaults_and_backward_compat() {
        // New EffectsConfig defaults: the master animation switch is ON at full
        // intensity (reproducing the shipped feel) and every motion overlay is
        // OFF — the master toggle changes nothing until a user opts an effect in.
        let d = EffectsConfig::default();
        assert!(d.animations_enabled);
        assert_eq!(d.animation_intensity, 1.0);
        assert!(!d.flicker && !d.vhs_tracking && !d.wired_ambient);
        assert!(!d.cursor_trail && !d.boot_glitch);
        // The cursor-trail intensity defaults mid-high so a freshly-enabled trail
        // is visible at once (tunable either way by the Motion slider).
        assert_eq!(d.cursor_trail_intensity, 0.6);

        // An OLD config written before the Motion fields existed (only the
        // original four effect keys) MUST still deserialize: the struct-level
        // `#[serde(default)]` fills every missing field from Default, never a
        // parse error. This is the load-compat guarantee the reorg depends on.
        let old: EffectsConfig = toml::from_str(
            "crt_scanlines = true\nscanline_darkness = 0.7\n\
             chromatic_aberration_enabled = false\nchromatic_aberration = 0.0\n",
        )
        .expect("legacy effects config without motion fields must load");
        assert!(old.crt_scanlines);
        assert_eq!(old.scanline_darkness, 0.7);
        assert!(old.animations_enabled, "missing master switch defaults ON");
        assert_eq!(old.animation_intensity, 1.0);
        assert!(!old.boot_glitch, "missing motion effect defaults OFF");
        assert_eq!(
            old.cursor_trail_intensity, 0.6,
            "missing cursor-trail intensity defaults to the mid-high band"
        );
        assert_eq!(
            old.mesh_brightness, 1.0,
            "missing mesh brightness defaults to the shipped 1.0 (no visual change)"
        );

        // Clamped accessors keep a malformed / out-of-band value inside its
        // (widened) design range before it can reach a painter.
        let wild = EffectsConfig {
            animation_intensity: 9.0,
            flicker_strength: 5.0,
            mesh_density: -3.0,
            mesh_brightness: 9.0,
            cursor_trail_intensity: 9.0,
            mesh_speed: 9.0,
            vhs_intensity: 5.0,
            ..EffectsConfig::default()
        };
        assert_eq!(wild.clamped_animation_intensity(), 2.0);
        assert_eq!(wild.clamped_flicker_strength(), 1.0);
        assert_eq!(wild.clamped_mesh_density(), 0.0);
        assert_eq!(wild.clamped_mesh_brightness(), 3.0);
        assert_eq!(wild.clamped_cursor_trail_intensity(), 2.0);
        // Out-of-band high inputs clamp to each band's ceiling — proves the clamp
        // is load-bearing (kills the "remove clamp" mutant on the new accessors).
        assert_eq!(wild.clamped_mesh_speed(), 2.0);
        assert_eq!(wild.clamped_vhs_intensity(), 1.0);
    }

    #[test]
    fn motion_defaults_are_exact_and_in_band_values_pass_through() {
        // Pin the EXACT default of every Motion knob — the shipped feel is these
        // numbers, and a silent drift (e.g. a default bumped to 0/1) would change
        // the out-of-box look. Asserting the concrete value (not just "non-zero")
        // is what proves the default is the intended one.
        assert_eq!(default_animation_intensity(), 1.0);
        assert_eq!(default_flicker_strength(), 0.06);
        assert_eq!(default_mesh_density(), 0.4);
        assert_eq!(default_mesh_brightness(), 1.0);
        assert_eq!(default_mesh_speed(), 1.0);
        assert_eq!(default_vhs_intensity(), 0.5);
        assert_eq!(default_cursor_trail_intensity(), 0.6);

        // A value already INSIDE each band must pass through UNCHANGED — the
        // clamp only touches out-of-range inputs. This is the half the `wild`
        // (out-of-band) test can't see: it proves the accessor returns the real
        // input mid-band, not a constant, so the slider actually drives the
        // effect across its whole range.
        let mid = EffectsConfig {
            animation_intensity: 1.5,
            flicker_strength: 0.5,
            mesh_density: 1.2,
            mesh_brightness: 1.5,
            mesh_speed: 1.5,
            vhs_intensity: 0.7,
            cursor_trail_intensity: 1.0,
            ..EffectsConfig::default()
        };
        assert_eq!(mid.clamped_animation_intensity(), 1.5);
        assert_eq!(mid.clamped_flicker_strength(), 0.5);
        assert_eq!(mid.clamped_mesh_density(), 1.2);
        assert_eq!(mid.clamped_mesh_brightness(), 1.5);
        assert_eq!(mid.clamped_mesh_speed(), 1.5);
        assert_eq!(mid.clamped_vhs_intensity(), 0.7);
        assert_eq!(mid.clamped_cursor_trail_intensity(), 1.0);
    }

    #[test]
    fn graphics_backend_defaults_to_auto_and_round_trips() {
        let p = std::path::Path::new("config.toml");
        // Default is Auto (the platform-smart choice), and a missing key parses
        // to Auto (serde default) so older configs keep working.
        assert_eq!(Config::default().graphics_backend, GraphicsBackend::Auto);
        assert_eq!(
            Config::from_toml("", p).unwrap().graphics_backend,
            GraphicsBackend::Auto,
            "absent key → Auto (backward compatible)"
        );
        // An explicit lowercase value parses, and the round-trip preserves it.
        let c = Config::from_toml("graphics_backend = \"vulkan\"\n", p).unwrap();
        assert_eq!(c.graphics_backend, GraphicsBackend::Vulkan);
        let toml = c.to_toml().unwrap();
        assert_eq!(
            Config::from_toml(&toml, p).unwrap().graphics_backend,
            GraphicsBackend::Vulkan,
            "graphics_backend survives a serialize→parse round-trip"
        );
    }

    /// `config_dir()` (the per-user data dir the W1TN3SS report spool lives under)
    /// MUST be exactly the parent of `default_path()` (the dir holding
    /// `config.toml`) — so the spool, config, and crash logs share one root. This
    /// pins the value: it is neither `None` when a config path resolves, nor an
    /// empty/default `PathBuf` — it is the real `…/c0pl4nd` directory.
    #[test]
    fn config_dir_is_parent_of_default_path() {
        match Config::default_path() {
            Some(p) => {
                let expected = p.parent().map(Path::to_path_buf);
                assert_eq!(
                    Config::config_dir(),
                    expected,
                    "config_dir must equal the parent of default_path"
                );
                let dir =
                    Config::config_dir().expect("config_dir resolves whenever default_path does");
                assert!(
                    !dir.as_os_str().is_empty(),
                    "config_dir must not be an empty path"
                );
                assert!(
                    dir.ends_with("c0pl4nd"),
                    "config_dir must be the per-user c0pl4nd data dir, got {dir:?}"
                );
            }
            // No config path resolves in this environment → config_dir is None too.
            None => assert_eq!(Config::config_dir(), None),
        }
    }

    /// Doc/behaviour parity guard: the default update mode is `notify`, which
    /// means a fresh install DOES make a once-per-launch network version check.
    /// This is a privacy-relevant default documented in PRIVACY.md / CHANGELOG.md
    /// — if this ever changes, those docs MUST change too. Failing this test is a
    /// signal that the documented default has silently drifted from the code.
    #[test]
    fn default_update_mode_is_notify_and_checks_on_launch() {
        assert_eq!(UpdateMode::default(), UpdateMode::Notify);
        let u = UpdateConfig::default();
        assert_eq!(u.mode, UpdateMode::Notify);
        assert!(
            u.checks_on_launch(),
            "the default `notify` mode performs an on-launch check — PRIVACY.md \
             must say so"
        );
        assert_eq!(u.check_interval_hours, 24);
    }

    #[test]
    fn font_default_is_the_sane_industry_size() {
        // Regression guard for the "grid text renders too large" report: the
        // default terminal font size is a calm, industry-standard 13 logical
        // points — NOT the previous 14.0 (which read large, especially on HiDPI).
        // A user-saved size is untouched; only the default is pinned here.
        assert_eq!(FontConfig::default().size, 13.0);
    }

    #[test]
    fn partial_toml_fills_defaults() {
        let p = PathBuf::from("test.toml");
        let c = Config::from_toml("theme = \"ghost-paper\"\n", &p).unwrap();
        assert_eq!(c.theme, "ghost-paper");
        assert_eq!(c.font.size, 13.0); // default preserved
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
    fn default_keybindings_have_no_conflicts() {
        // The shipped default set must be clean — no collisions, no blanks.
        assert!(Keybindings::default().validate().is_empty());
    }

    #[test]
    fn ui_scale_defaults_to_one_and_backfills_for_old_configs() {
        assert_eq!(Config::default().ui_scale, 1.0);
        // A config file with no `ui_scale` key backfills via serde(default).
        let p = PathBuf::from("test.toml");
        let c = Config::from_toml("theme = \"ghost-paper\"\n", &p).unwrap();
        assert_eq!(c.ui_scale, 1.0);
    }

    #[test]
    fn effective_ui_scale_clamps_and_guards_against_garbage() {
        let mk = |s: f32| Config {
            ui_scale: s,
            ..Config::default()
        };
        assert_eq!(mk(1.5).effective_ui_scale(), 1.5); // in-range passes through
        assert_eq!(mk(0.1).effective_ui_scale(), 0.5); // clamped up to the floor
        assert_eq!(mk(99.0).effective_ui_scale(), 3.0); // clamped down to the ceil
        assert_eq!(mk(f32::NAN).effective_ui_scale(), 1.0); // garbage → safe default
        assert_eq!(mk(f32::INFINITY).effective_ui_scale(), 1.0);
    }

    #[test]
    fn validate_detects_a_duplicate_combo_collision() {
        // Bind `paste` to the SAME combo as `copy` (order-insensitive form to
        // prove normalization): copy = "mod+shift+c".
        let kb = Keybindings {
            paste: "shift+mod+c".into(),
            ..Default::default()
        };
        let issues = kb.validate();
        assert_eq!(issues.len(), 1, "exactly one conflict expected: {issues:?}");
        match &issues[0] {
            KeybindingIssue::Conflict { combo, actions } => {
                assert_eq!(combo, "c+mod+shift"); // normalized: sorted tokens
                assert!(actions.contains(&"copy") && actions.contains(&"paste"));
            }
            other => panic!("expected a Conflict, got {other:?}"),
        }
    }

    #[test]
    fn validate_detects_an_empty_binding() {
        let kb = Keybindings {
            search: "   ".into(), // whitespace-only → unreachable
            ..Default::default()
        };
        let issues = kb.validate();
        assert!(
            issues
                .iter()
                .any(|i| matches!(i, KeybindingIssue::Empty { action } if *action == "search")),
            "an empty binding must be reported: {issues:?}"
        );
    }

    #[test]
    fn keybinding_issue_messages_are_human_readable() {
        let empty = KeybindingIssue::Empty { action: "copy" };
        assert!(empty.message().contains("copy"));
        let conflict = KeybindingIssue::Conflict {
            combo: "mod+shift+c".into(),
            actions: vec!["copy", "paste"],
        };
        let m = conflict.message();
        assert!(m.contains("copy") && m.contains("paste") && m.contains("mod+shift+c"));
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
        assert_eq!(
            c.tint, "#08060d",
            "fresh default tint is brand-canon VOID BLACK"
        );
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

    // ---- W1TN3SS opt-in reporting (additive, default-OFF) ----

    #[test]
    fn reporting_defaults_both_streams_off() {
        use itasha_report_core::config::ReportingMode;
        let c = Config::default();
        assert_eq!(
            c.reporting.streams.crash_reports,
            ReportingMode::Off,
            "crash reporting MUST default OFF (opt-in only)"
        );
        assert_eq!(
            c.reporting.streams.manual_issues,
            ReportingMode::Off,
            "manual-issue reporting MUST default OFF (opt-in only)"
        );
    }

    #[test]
    fn old_config_without_reporting_table_loads_both_streams_off() {
        use itasha_report_core::config::ReportingMode;
        // A config file written BEFORE the reporting feature existed (no
        // `[reporting]` table) must still parse, and reporting must come up OFF
        // — upgrading never silently enables any reporting stream.
        let p = PathBuf::from("test.toml");
        let c = Config::from_toml("theme = \"ghost-paper\"\n", &p).unwrap();
        assert_eq!(c.reporting.streams.crash_reports, ReportingMode::Off);
        assert_eq!(c.reporting.streams.manual_issues, ReportingMode::Off);
        // Issue-intake coords backfill to the C0PL4ND defaults.
        assert_eq!(
            c.reporting.issue_intake.repo,
            "46b-ETYKiAL/Itasha.Corp_C0PL4ND"
        );
    }

    #[test]
    fn reporting_round_trips_and_is_reversible() {
        use itasha_report_core::config::ReportingMode;
        // A user opts crash reporting into AskEachTime; the migrate is reversible
        // (serialize → parse reproduces the stored value; an unset stream stays
        // OFF). serde stored-value wins, so an explicit value is never clobbered.
        let p = PathBuf::from("test.toml");
        let toml = "[reporting.streams]\ncrash_reports = \"ask_each_time\"\n";
        let c = Config::from_toml(toml, &p).unwrap();
        assert_eq!(
            c.reporting.streams.crash_reports,
            ReportingMode::AskEachTime
        );
        // Manual-issue stream stays OFF (only the set field changed).
        assert_eq!(c.reporting.streams.manual_issues, ReportingMode::Off);
        // Round-trip: serialize then re-parse reproduces the posture exactly.
        let serialized = c.to_toml().unwrap();
        let c2 = Config::from_toml(&serialized, &p).unwrap();
        assert_eq!(c2.reporting, c.reporting);
    }

    // ---- validate() Invalid arms: each out-of-range / empty value is rejected
    //      with the EXACT ConfigError::Invalid variant and a message that names
    //      the offending field, so the settings UI can surface a useful error. ----

    /// Pull the inner message out of a `ConfigError::Invalid`, panicking with the
    /// real variant if the error is anything else — so a wrong *variant* (not just
    /// a wrong message) fails the test.
    fn expect_invalid(err: ConfigError) -> String {
        match err {
            ConfigError::Invalid(msg) => msg,
            other => panic!("expected ConfigError::Invalid, got {other:?}"),
        }
    }

    #[test]
    fn validate_rejects_empty_theme_with_named_message() {
        let c = Config {
            theme: "   ".to_string(), // whitespace-only trims to empty
            ..Config::default()
        };
        let msg = expect_invalid(c.validate().unwrap_err());
        assert_eq!(msg, "theme name must not be empty");
    }

    #[test]
    fn validate_rejects_out_of_range_opacity_with_value_in_message() {
        // Above the 0.0..=1.0 band.
        let hi = Config {
            opacity: 5.0,
            ..Config::default()
        };
        let msg = expect_invalid(hi.validate().unwrap_err());
        assert_eq!(msg, "opacity must be between 0.0 and 1.0, got 5");
        // Below the band (negative) is equally rejected; the literal value is echoed.
        let lo = Config {
            opacity: -0.5,
            ..Config::default()
        };
        let msg = expect_invalid(lo.validate().unwrap_err());
        assert_eq!(msg, "opacity must be between 0.0 and 1.0, got -0.5");
        // The exact band edges are VALID (boundary inclusivity).
        assert!(Config {
            opacity: 0.0,
            ..Config::default()
        }
        .validate()
        .is_ok());
        assert!(Config {
            opacity: 1.0,
            ..Config::default()
        }
        .validate()
        .is_ok());
    }

    #[test]
    fn validate_rejects_non_positive_font_size() {
        // Zero is non-positive → rejected.
        let zero = Config {
            font: FontConfig {
                size: 0.0,
                ..FontConfig::default()
            },
            ..Config::default()
        };
        assert_eq!(
            expect_invalid(zero.validate().unwrap_err()),
            "font.size must be positive"
        );
        // Negative is also rejected.
        let neg = Config {
            font: FontConfig {
                size: -3.0,
                ..FontConfig::default()
            },
            ..Config::default()
        };
        assert_eq!(
            expect_invalid(neg.validate().unwrap_err()),
            "font.size must be positive"
        );
    }

    #[test]
    fn validate_rejects_zero_cols_or_rows() {
        // cols == 0 (rows fine) → rejected.
        let c0 = Config {
            window: WindowConfig {
                cols: 0,
                ..WindowConfig::default()
            },
            ..Config::default()
        };
        assert_eq!(
            expect_invalid(c0.validate().unwrap_err()),
            "window cols and rows must be non-zero"
        );
        // rows == 0 (cols fine) → also rejected (the OR arm).
        let r0 = Config {
            window: WindowConfig {
                rows: 0,
                ..WindowConfig::default()
            },
            ..Config::default()
        };
        assert_eq!(
            expect_invalid(r0.validate().unwrap_err()),
            "window cols and rows must be non-zero"
        );
    }

    #[test]
    fn validate_rejects_out_of_range_tint_strength_with_value() {
        let c = Config {
            tint_strength: 1.5,
            ..Config::default()
        };
        assert_eq!(
            expect_invalid(c.validate().unwrap_err()),
            "tint_strength must be between 0.0 and 1.0, got 1.5"
        );
    }

    #[test]
    fn validate_rejects_bad_tint_hex_with_debug_quoted_value() {
        // A non-#RRGGBB tint is rejected, and the offending value is echoed
        // Debug-quoted (so `{:?}` of the String shows the surrounding quotes).
        let c = Config {
            tint: "nope".to_string(),
            ..Config::default()
        };
        assert_eq!(
            expect_invalid(c.validate().unwrap_err()),
            "tint must be a #RRGGBB hex color, got \"nope\""
        );
    }

    #[test]
    fn validate_passes_for_a_fully_valid_non_default_config() {
        // Every guarded field set to a valid non-default value still validates,
        // proving the happy path threads all the way through validate().
        let c = Config {
            theme: "ghost-paper".to_string(),
            opacity: 0.5,
            tint_strength: 0.5,
            tint: "#abcdef".to_string(),
            font: FontConfig {
                size: 10.0,
                ..FontConfig::default()
            },
            window: WindowConfig {
                cols: 120,
                rows: 40,
                ..WindowConfig::default()
            },
            ..Config::default()
        };
        assert!(c.validate().is_ok());
    }

    // ---- ConfigError variants: construction, Display, and the load/save Io arms ----

    #[test]
    fn config_error_not_found_display_names_path_and_defaults() {
        let e = ConfigError::NotFound(PathBuf::from("/etc/c0pl4nd/config.toml"));
        let s = e.to_string();
        assert!(
            s.contains("config file not found") && s.contains("using built-in defaults"),
            "NotFound Display must explain the fallback, got: {s}"
        );
        assert!(
            s.contains("config.toml"),
            "NotFound Display must name the path, got: {s}"
        );
    }

    #[test]
    fn config_error_parse_is_the_parse_variant_with_path_and_message() {
        // Malformed TOML surfaces a Parse error (not Invalid/Io), carrying the
        // path we passed and a non-empty human-readable message.
        let p = PathBuf::from("broken.toml");
        let err = Config::from_toml("this is = = not toml", &p).unwrap_err();
        match err {
            ConfigError::Parse { path, message } => {
                assert_eq!(path, p, "Parse error must echo the source path");
                assert!(!message.is_empty(), "Parse error must carry a message");
                // The Display format embeds both the path and the message.
                let disp = ConfigError::Parse { path, message }.to_string();
                assert!(disp.contains("config parse error in") && disp.contains("broken.toml"));
            }
            other => panic!("expected ConfigError::Parse, got {other:?}"),
        }
    }

    #[test]
    fn config_error_invalid_display_format() {
        let e = ConfigError::Invalid("widget out of range".to_string());
        assert_eq!(
            e.to_string(),
            "config validation error: widget out of range"
        );
    }

    #[test]
    fn config_error_io_display_names_path_and_source() {
        let e = ConfigError::Io {
            path: PathBuf::from("/no/such/cfg.toml"),
            source: std::io::Error::new(std::io::ErrorKind::PermissionDenied, "denied"),
        };
        let s = e.to_string();
        assert!(
            s.contains("could not read config file")
                && s.contains("cfg.toml")
                && s.contains("denied"),
            "Io Display must name the path and embed the source, got: {s}"
        );
    }

    #[test]
    fn load_from_existing_valid_file_round_trips_through_from_toml() {
        // The Ok arm of load_from: an existing, readable, valid file parses.
        let path =
            std::env::temp_dir().join(format!("c0pl4nd-load-{}-ok.toml", std::process::id()));
        let _ = std::fs::remove_file(&path);
        std::fs::write(&path, "theme = \"ghost-paper\"\nligatures = true\n").expect("seed");
        let c = Config::load_from(&path).expect("valid file loads");
        assert_eq!(c.theme, "ghost-paper");
        assert!(c.ligatures);
        assert_eq!(c.font.size, 13.0, "unset fields keep their defaults");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn load_from_invalid_file_surfaces_validation_error_not_defaults() {
        // The Ok arm reads, but validation rejects an out-of-range value — the
        // error must propagate (not silently fall back to defaults).
        let path =
            std::env::temp_dir().join(format!("c0pl4nd-load-{}-bad.toml", std::process::id()));
        let _ = std::fs::remove_file(&path);
        std::fs::write(&path, "opacity = 9.0\n").expect("seed");
        let err = Config::load_from(&path).unwrap_err();
        assert!(
            matches!(err, ConfigError::Invalid(_)),
            "an out-of-range value in a real file must surface as Invalid, got {err:?}"
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn save_to_under_a_regular_file_parent_yields_io_error() {
        // Provoke the save_to Io arm cross-platform: make the parent a regular
        // FILE, so create_dir_all (inside atomic_write_owner_only) cannot create
        // a directory there. The error must surface as ConfigError::Io carrying
        // the target path — never a panic, never a silent success.
        let file_as_parent =
            std::env::temp_dir().join(format!("c0pl4nd-save-{}-blocker", std::process::id()));
        let _ = std::fs::remove_file(&file_as_parent);
        let _ = std::fs::remove_dir_all(&file_as_parent);
        std::fs::write(&file_as_parent, b"i am a file, not a dir").expect("seed blocker");
        // Target a path *under* the regular file → its parent dir cannot exist.
        let target = file_as_parent.join("sub").join("config.toml");
        let err = Config::default().save_to(&target).unwrap_err();
        match err {
            ConfigError::Io { path, source: _ } => {
                assert_eq!(path, target, "Io error must echo the target path");
            }
            other => panic!("expected ConfigError::Io, got {other:?}"),
        }
        let _ = std::fs::remove_file(&file_as_parent);
    }

    #[cfg(windows)]
    #[test]
    fn load_from_unreadable_path_surfaces_io_not_notfound() {
        // On Windows an invalid filename (reserved characters) yields an IO error
        // whose kind is NOT NotFound, so it routes to the load_from Io arm rather
        // than the "missing file → defaults" arm. This covers the third match arm
        // of load_from that NotFound can never reach on this platform.
        let bad = std::env::temp_dir().join("c0pl4nd-inv<>:|?.toml");
        match Config::load_from(&bad) {
            Err(ConfigError::Io { path, source }) => {
                assert_eq!(path, bad);
                assert_ne!(
                    source.kind(),
                    std::io::ErrorKind::NotFound,
                    "a NotFound kind should have fallen back to defaults, not Io"
                );
            }
            // If the platform unexpectedly treats it as NotFound, the contract is
            // still honoured: missing file → defaults. Accept that outcome too so
            // the test is not brittle across Windows builds.
            Ok(c) => assert_eq!(c, Config::default()),
            Err(other) => panic!("expected Io or default-fallback, got {other:?}"),
        }
    }

    // ---- to_toml: round-trip fidelity through the public serialize path ----

    #[test]
    fn to_toml_serializes_then_from_toml_reproduces_the_config() {
        // to_toml is the inverse of from_toml; a non-default config survives the
        // round trip field-for-field. Exercises the Ok arm of to_toml and proves
        // the serialized form re-parses to an equal value.
        let original = Config {
            theme: "ghost-paper".to_string(),
            opacity: 0.75,
            transparency_enabled: true,
            window_mode: WindowMode::Mica,
            tint: "#aabbcc".to_string(),
            tint_strength: 0.3,
            ligatures: true,
            ..Config::default()
        };
        let serialized = original.to_toml().expect("serialize");
        assert!(
            serialized.contains("theme = \"ghost-paper\""),
            "serialized TOML should contain the theme key"
        );
        let p = PathBuf::from("roundtrip.toml");
        let reparsed = Config::from_toml(&serialized, &p).expect("re-parse");
        assert_eq!(reparsed, original, "to_toml ∘ from_toml must be identity");
    }

    // ---- IssueIntake / Reporting default coordinates (exact values pinned) ----

    #[test]
    fn issue_intake_defaults_are_the_c0pl4nd_coordinates() {
        let i = IssueIntakeConfig::default();
        assert_eq!(i.repo, "46b-ETYKiAL/Itasha.Corp_C0PL4ND");
        assert_eq!(i.mailto_alias, "46b.AbandonSomething@proton.me");
        // The top-level ReportingConfig embeds those same defaults.
        let r = ReportingConfig::default();
        assert_eq!(r.issue_intake, i);
    }

    // ---- PanelSide default + serde round-trip ----

    #[test]
    fn panel_side_defaults_right_and_round_trips() {
        assert_eq!(PanelSide::default(), PanelSide::Right);
        assert_eq!(Config::default().history_sidebar_side, PanelSide::Right);
        let p = PathBuf::from("test.toml");
        let c = Config::from_toml("history_sidebar_side = \"left\"\n", &p).unwrap();
        assert_eq!(c.history_sidebar_side, PanelSide::Left);
    }

    // ---- CursorStyle / CursorConfig parsing + defaults ----

    #[test]
    fn cursor_defaults_block_blinking_and_parses_snake_case_styles() {
        let cc = CursorConfig::default();
        assert_eq!(cc.style, CursorStyle::Block);
        assert!(cc.blink);
        // snake_case rename: "underline" / "bar" parse to their variants, and a
        // partial [cursor] table backfills the unspecified field.
        let p = PathBuf::from("test.toml");
        let c = Config::from_toml("[cursor]\nstyle = \"underline\"\nblink = false\n", &p).unwrap();
        assert_eq!(c.cursor.style, CursorStyle::Underline);
        assert!(!c.cursor.blink);
        let c2 = Config::from_toml("[cursor]\nstyle = \"bar\"\n", &p).unwrap();
        assert_eq!(c2.cursor.style, CursorStyle::Bar);
        assert!(
            c2.cursor.blink,
            "unset blink backfills to the default (true)"
        );
    }

    // ---- UpdateMode parsing + checks_on_launch for every mode ----

    #[test]
    fn update_mode_parses_all_variants_and_checks_on_launch_is_per_mode() {
        let p = PathBuf::from("test.toml");
        for (raw, mode, on_launch) in [
            ("off", UpdateMode::Off, false),
            ("notify", UpdateMode::Notify, true),
            ("manual", UpdateMode::Manual, false),
            ("auto", UpdateMode::Auto, true),
        ] {
            let toml = format!("[update]\nmode = \"{raw}\"\n");
            let c = Config::from_toml(&toml, &p).unwrap();
            assert_eq!(c.update.mode, mode, "mode {raw:?} must parse to {mode:?}");
            assert_eq!(
                c.update.checks_on_launch(),
                on_launch,
                "checks_on_launch for {raw:?} must be {on_launch}"
            );
        }
    }

    #[test]
    fn legacy_check_on_launch_flag_forces_on_launch_even_when_mode_is_off() {
        // The legacy boolean OR-arm: mode = off (no network) but the old
        // check_on_launch = true flag still forces an on-launch check, so old
        // config files keep their behaviour.
        let p = PathBuf::from("test.toml");
        let c =
            Config::from_toml("[update]\nmode = \"off\"\ncheck_on_launch = true\n", &p).unwrap();
        assert_eq!(c.update.mode, UpdateMode::Off);
        assert!(
            c.update.checks_on_launch(),
            "the legacy check_on_launch flag forces a launch check despite mode=off"
        );
        // And with the flag false AND mode off, no launch check (the false arm).
        let c2 = Config::from_toml("[update]\nmode = \"manual\"\n", &p).unwrap();
        assert!(!c2.update.checks_on_launch());
    }

    #[test]
    fn migrate_legacy_transparency_is_noop_when_no_legacy_signal() {
        // Fully-default opacity (1.0) + acrylic off + new fields at defaults:
        // migration must change nothing (the early-return / no-signal path).
        let mut c = Config::default();
        c.migrate_legacy_transparency();
        assert!(!c.transparency_enabled);
        assert_eq!(c.window_mode, WindowMode::Opaque);
        // And the explicit-new-model early return: transparency_enabled already
        // true → migration leaves the chosen mode untouched.
        let mut c2 = Config {
            transparency_enabled: true,
            window_mode: WindowMode::Vibrancy,
            acrylic: true, // would imply Glass if migration ran
            opacity: 0.2,
            ..Config::default()
        };
        c2.migrate_legacy_transparency();
        assert_eq!(
            c2.window_mode,
            WindowMode::Vibrancy,
            "an explicit master-on config is never re-migrated"
        );
    }

    #[test]
    fn fresh_config_is_born_at_current_schema_version() {
        // A brand-new config is stamped at the current version so `migrate` is a
        // no-op for new users (no spurious first-run rewrite).
        let c = Config::default();
        assert_eq!(c.schema_version, CURRENT_SCHEMA_VERSION);
        // migrate() on a fresh config changes nothing and reports no change.
        let mut c2 = Config::default();
        assert!(!c2.migrate(), "a fresh config must not be migrated");
        assert_eq!(c2.schema_version, CURRENT_SCHEMA_VERSION);
    }

    #[test]
    fn schema_version_round_trips() {
        // A default config serialized then reloaded (which runs migrate) stays at
        // the current version — the version survives a save/load cycle.
        let p = PathBuf::from("cfg.toml");
        let toml = toml::to_string(&Config::default()).expect("serialize");
        let back = Config::from_toml(&toml, &p).expect("reload");
        assert_eq!(back.schema_version, CURRENT_SCHEMA_VERSION);
    }

    #[test]
    fn migrate_is_a_noop_and_never_panics_on_a_forward_version_config() {
        // A config written by a NEWER build (schema_version ahead of ours) opened
        // by an older build must be left untouched, report no change, and — the
        // load-bearing part — never trip the debug-only migration invariants.
        let mut forward = Config {
            schema_version: CURRENT_SCHEMA_VERSION + 5,
            ..Config::default()
        };
        let changed = forward.migrate();
        assert!(!changed, "a forward-version config must not be migrated");
        assert_eq!(
            forward.schema_version,
            CURRENT_SCHEMA_VERSION + 5,
            "a forward-version config legitimately stays ahead"
        );
    }

    #[test]
    fn legacy_transparency_still_fires_from_v0() {
        // An EXISTING config with no `schema_version` (→ 0) that carries a legacy
        // translucency signal (`opacity < 1.0`) must still be promoted to the
        // master-toggle + mode model on load, and be stamped at v1 afterward — the
        // v0→v1 step wraps the pre-existing transparency migration with no loss.
        let p = PathBuf::from("legacy.toml");
        let c = Config::from_toml("opacity = 0.8\n", &p).unwrap();
        assert_eq!(c.schema_version, CURRENT_SCHEMA_VERSION);
        assert!(
            c.transparency_enabled,
            "a legacy opacity<1.0 config must migrate transparency ON"
        );
        assert_eq!(c.window_mode, WindowMode::Transparent);

        // The acrylic legacy signal likewise still promotes to Glass from v0.
        let c2 = Config::from_toml("acrylic = true\n", &p).unwrap();
        assert!(c2.transparency_enabled);
        assert_eq!(c2.window_mode, WindowMode::Glass);
        assert_eq!(c2.schema_version, CURRENT_SCHEMA_VERSION);
    }

    #[test]
    fn fresh_config_default_tint_is_void_black() {
        // A brand-new config is born with the brand-canon VOID BLACK default tint.
        let c = Config::default();
        assert_eq!(c.tint, "#08060d");
    }

    #[test]
    fn pre_v2_default_tint_migrates_to_void_black() {
        // An EXISTING config (no `schema_version` → 0) still carrying the OLD
        // `#121212` default tint is remapped to `#08060d` on load, then stamped at
        // the current schema version.
        let p = PathBuf::from("legacy-tint.toml");
        let c = Config::from_toml("tint = \"#121212\"\n", &p).unwrap();
        assert_eq!(c.tint, "#08060d", "old default tint remaps to VOID BLACK");
        assert_eq!(c.schema_version, CURRENT_SCHEMA_VERSION);
    }

    #[test]
    fn pre_v2_custom_tint_is_preserved_verbatim() {
        // A user's CUSTOM tint (any value ≠ the old `#121212` default) must survive
        // the v1 → v2 migration untouched — no silent clobber.
        let p = PathBuf::from("custom-tint.toml");
        let c = Config::from_toml("tint = \"#445566\"\n", &p).unwrap();
        assert_eq!(c.tint, "#445566", "a user's custom tint is never remapped");
        assert_eq!(c.schema_version, CURRENT_SCHEMA_VERSION);
    }

    #[test]
    fn toolbar_config_defaults_when_absent_and_round_trips() {
        let p = std::path::PathBuf::from("cfg.toml");
        // An EXISTING config predating the feature (no `[toolbar]` table) loads the
        // shipped defaults — never broken by the new field (serde-default merge).
        // The default keeps view/equalize/shell on the LEFT and pins ONLY the
        // script launcher on the RIGHT (by the gear).
        let c = Config::from_toml("", &p).unwrap();
        assert_eq!(c.toolbar, ToolbarConfig::default());
        assert_eq!(
            c.toolbar.left,
            vec!["view_mode", "equalize_panes", "shell_switcher"]
        );
        assert_eq!(c.toolbar.right, vec!["script_launcher"]);
        assert!(c.toolbar.show_overflow);
        assert!(c.toolbar.menu.is_empty());

        // A PARTIAL `[toolbar]` (only `right`) fills the other fields from defaults.
        let partial = Config::from_toml("[toolbar]\nright = [\"view_mode\"]\n", &p).unwrap();
        assert_eq!(partial.toolbar.right, vec!["view_mode"]);
        assert!(partial.toolbar.show_overflow); // default true, not clobbered
        assert!(partial.toolbar.menu.is_empty());

        // A fully-customized toolbar round-trips through TOML unchanged.
        let mut custom = Config::default();
        custom.toolbar.left = vec!["view_mode".into()];
        custom.toolbar.right = vec!["script_launcher".into(), "shell_switcher".into()];
        custom.toolbar.menu = vec!["equalize_panes".into()];
        custom.toolbar.show_overflow = false;
        let toml = toml::to_string(&custom).expect("serialize");
        let back = Config::from_toml(&toml, &p).unwrap();
        assert_eq!(back.toolbar, custom.toolbar);
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
