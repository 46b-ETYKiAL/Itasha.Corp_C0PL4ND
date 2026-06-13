//! Milestone 1 of the C0PL4ND egui chrome modernization (recon dossier
//! `.s4f3-data/recon-c0pl4nd-egui-modernization.md`, steps 1–4).
//!
//! This module is the modern `eframe`/`egui` application shell, shipped as a
//! SEPARATE binary (`c0pl4nd-egui`) so the existing winit-driven `c0pl4nd`
//! binary keeps building and shipping unchanged. The chrome (frameless
//! titlebar, two-tone wordmark, tab strip, caption buttons, status bar) and the
//! `egui_tiles` pane grid are real and clickable; each pane body hosts a live
//! PTY whose visible grid is drawn with egui's NATIVE coloured-text painter (see
//! [`paint_grid_native`]). An earlier milestone rendered the grid through a
//! glyphon GPU paint callback / offscreen texture, but that path composited
//! black inside `egui_tiles` panes on the real swapchain (while passing the wgpu
//! test harness); native text renders reliably everywhere and matches SCR1B3's
//! coloured-text approach, so the glyphon path was removed.
//!
//! eframe owns the event loop; no winit plumbing here.

pub mod bidi;
pub mod chrome;
pub mod fonts;
pub mod grid;
pub mod hyperlink;
pub mod pane_term;
mod search_ui;
mod settings;
pub mod shells;
mod theme;

use std::collections::{HashMap, HashSet};

use eframe::egui;

use c0pl4nd_core::term::ColorSet;
use grid::{count_panes, GridBehavior, Pane, PaneId, PaneIdAllocator};
use pane_term::{CellMetrics, ColorRun, PaneTerm};

/// How many placeholder panes the shell opens with on first launch.
const INITIAL_PANES: usize = 1;

/// A window-level caption command issued by the titlebar buttons. Routed through
/// [`chrome::ChromeActions`] so [`C0pl4ndApp::frame_tick`] is the single site
/// that (a) issues the real `egui::ViewportCommand` to the OS and (b) records
/// the command in [`C0pl4ndApp::last_window_cmd`] so an interaction test can
/// assert that clicking the real button produced the real effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowCmd {
    /// Minimize the window.
    Minimize,
    /// Toggle maximized/restored.
    ToggleMaximize,
    /// Close the window.
    Close,
}

/// Which of a grid row's up-to-three painted galleys this cache entry holds: the
/// crisp main pass in the runs' real colours, or one of the two pure-channel
/// chromatic-aberration ghost passes. Each pass for a row is keyed separately so
/// the ghost galleys (drawn in a single override colour) never collide with the
/// crisp galley (drawn in the runs' real per-cell colours).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum RowPass {
    /// The crisp main pass — each run keeps its real SGR colour.
    Main,
    /// The pure-red ghost (chromatic aberration), shifted left.
    GhostRed,
    /// The pure-blue ghost (chromatic aberration), shifted right.
    GhostBlue,
}

/// Per-(pane, row, pass) laid-out-galley cache for [`paint_grid_native`]
/// (audit #2). Each row's [`egui::Galley`] is re-laid-out only when its content
/// or style key changes; an idle or partially-changed grid reuses the cached
/// `Arc<Galley>` instead of rebuilding the [`egui::text::LayoutJob`] (the
/// per-run string-append allocations) and re-running layout every frame. egui's
/// own `Fonts` galley cache memoises identical jobs too, but it cannot save the
/// job-construction cost — this cache does, and skips the cache lookup entirely
/// on a hit. Cleared wholesale on a font re-install (the cached galleys reference
/// the old font atlas).
#[derive(Default)]
struct GalleyCache {
    /// `(pane, row_idx, pass) -> (content/style key, laid-out galley)`.
    rows: HashMap<(PaneId, usize, RowPass), (u64, std::sync::Arc<egui::Galley>)>,
    /// Row indices touched THIS frame, per pane, so rows that scrolled off (a
    /// shrunk grid) are pruned and the map cannot grow without bound.
    seen_this_frame: HashSet<(PaneId, usize, RowPass)>,
}

impl GalleyCache {
    /// Lay out one grid row's colour runs as a single galley, reusing the cached
    /// galley when the content/style `key` is unchanged. `build` constructs the
    /// [`egui::text::LayoutJob`] on a miss (kept as a closure so the per-run
    /// string allocations only happen when the cache misses). Records the entry
    /// as seen this frame for the end-of-frame prune.
    fn row_galley(
        &mut self,
        painter: &egui::Painter,
        pane: PaneId,
        row_idx: usize,
        pass: RowPass,
        key: u64,
        build: impl FnOnce() -> egui::text::LayoutJob,
    ) -> std::sync::Arc<egui::Galley> {
        let id = (pane, row_idx, pass);
        self.seen_this_frame.insert(id);
        if let Some((cached_key, galley)) = self.rows.get(&id) {
            if *cached_key == key {
                return galley.clone();
            }
        }
        let galley = painter.layout_job(build());
        self.rows.insert(id, (key, galley.clone()));
        galley
    }

    /// Drop cache entries for `(pane, row, pass)` tuples NOT touched this frame
    /// (rows that scrolled off / a closed pane) and reset the per-frame seen set.
    /// Called once at the end of [`C0pl4ndApp::grid_ui`].
    fn prune_unseen(&mut self) {
        if self.seen_this_frame.is_empty() {
            // Nothing painted this frame (e.g. every pane errored): keep entries
            // so a transient empty frame does not evict the whole cache.
            return;
        }
        self.rows.retain(|id, _| self.seen_this_frame.contains(id));
        self.seen_this_frame.clear();
    }

    /// Drop every entry — used when the font stack is re-installed (the cached
    /// galleys reference the previous font atlas and must be relaid).
    fn clear(&mut self) {
        self.rows.clear();
        self.seen_this_frame.clear();
    }
}

/// The modern egui chrome application. Holds the tiling grid, the focused pane,
/// a settings-window toggle, and a transient status-bar toast.
pub struct C0pl4ndApp {
    /// Core config (loaded best-effort; defaults when absent). Kept so Milestone
    /// 2 can read font/cursor/keybinding settings without re-plumbing.
    config: c0pl4nd_core::Config,
    /// The active colour theme — glyph colours for the terminal grid come from
    /// here (NOT egui Visuals, which only style the chrome).
    theme: c0pl4nd_core::Theme,
    /// The tiling pane grid.
    grid_tree: egui_tiles::Tree<Pane>,
    /// Per-pane live terminal state (PTY + grid), keyed by pane id. A pane with
    /// no entry (or a failed spawn) renders an error/placeholder body.
    terms: HashMap<PaneId, PaneTerm>,
    /// Panes whose PTY is DEFERRED until their real pixel rect is known. The
    /// initial pane(s) are registered here at construction WITHOUT a PTY: if we
    /// spawned them at the 80×24 placeholder (the cmd-banner cursor-home bug
    /// #40) the first `resize_to_px` to the real width (e.g. a 200-col config)
    /// would reflow cmd's grid and snap its cursor back to (0,0), so typing
    /// overwrites the banner. Instead [`render_pane_body`] spawns each pending
    /// pane at the MEASURED `(cols, rows)` on the first frame its rect is known —
    /// exactly how a manually-opened terminal (`spawn_term`) already behaves —
    /// after which the debounced resize is a no-op and the cursor stays put.
    pending_spawn: HashSet<PaneId>,
    /// Monotonic pane-id allocator.
    pane_alloc: PaneIdAllocator,
    /// The currently-focused pane (drives tab highlight + input routing).
    focused_pane: PaneId,
    /// Panes the user pinned: their tabs sort first and can't be closed via the
    /// tab × (must unpin first).
    pinned: HashSet<PaneId>,
    /// The focused pane's last-rendered size `(w, h)` in points. Drives the
    /// "+" button's split direction (split the longer axis to stay balanced).
    last_focused_size: Option<(f32, f32)>,
    /// Shells offered by the top-bar switcher, platform default first. Detected
    /// once at construction (`shells::detect_profiles`).
    shell_profiles: Vec<shells::ShellProfile>,
    /// Index into `shell_profiles` that the plain "+" button and new terminals
    /// use. Set when the user picks a shell from the top-bar ▾ menu.
    active_shell: usize,
    /// Whether the chrome fonts (incl. the `phosphor-fill` family used for a
    /// pinned tab's solid pin) have been installed on the egui context. Set in
    /// `new`; the first `frame_tick` installs them otherwise (e.g. headless
    /// tests built via `bootstrap()`), so referencing the `phosphor-fill` family
    /// can never hit an unregistered-family panic.
    fonts_installed: bool,
    /// The font-stack key (family + fallbacks folded into one string by
    /// [`font_apply_key`]) that was LAST installed into egui. Compared each frame
    /// against the live config so a Family/Fallback change in settings triggers a
    /// single live re-install of the font stack — and the (expensive) system-font
    /// load runs ONLY on an actual change, never per frame.
    applied_font_family: String,
    /// The UI scale (F2-3) currently applied to the egui context, tracked so
    /// `frame_tick` re-applies `set_zoom_factor` ONLY when the configured
    /// `ui_scale` actually changes (not every frame, and without fighting the
    /// transient Ctrl+/- keyboard zoom, which never writes `config.ui_scale`).
    /// Initialised to a sentinel `NaN` so the first frame always applies.
    applied_ui_scale: f32,
    /// Whether the settings window is open.
    settings_open: bool,
    /// Recently-run commands, surfaced by the command palette for quick
    /// find/run. Captured best-effort from typed input (committed on Enter).
    cmd_history: c0pl4nd_core::command_history::CommandHistory,
    /// Accumulator for the line currently being typed in the focused pane.
    /// Committed to `cmd_history` on Enter, reset on focus change. Best-effort:
    /// it models printable text + Backspace, not full shell line-editing.
    input_line: String,
    /// A multi-line paste deferred for confirmation (paste-safety). When
    /// `config.paste_warn_multiline` is on and a paste contains a newline, it is
    /// parked here and a confirm overlay is shown instead of executing it
    /// immediately (the embedded newline would otherwise run a command on land).
    /// Enter in the overlay sends it (through the paste-injection guard); Esc
    /// discards it.
    pending_paste: Option<String>,
    /// Incognito session: when `true`, NO typed commands are recorded into
    /// command history (regardless of `config.history_capture_enabled`). Runtime
    /// only — never persisted, so it always starts off and resets each launch.
    incognito: bool,
    /// Whether the command palette overlay is open.
    palette_open: bool,
    /// Whether the command-history quick-run sidebar (`#21`) is open. A docked
    /// `egui::SidePanel` (side from `config.history_sidebar_side`) that lists the
    /// history newest-first with a filter box; clicking a row re-runs it in the
    /// focused pane via the SAME path as the command palette.
    history_open: bool,
    /// The history sidebar's filter query (substring/fuzzy over the history).
    history_filter: String,
    /// The palette's fuzzy-search query.
    palette_query: String,
    /// The palette's selected row (index into the filtered results).
    palette_sel: usize,
    /// The command most recently run FROM the palette (Enter or click). Set in
    /// [`Self::run_palette_selection`] so an interaction test can assert that
    /// driving the real palette ran the real command — the same observation
    /// pattern as [`Self::last_window_cmd`] (the PTY write itself is not
    /// observable in the headless harness).
    last_palette_run: Option<String>,
    /// The most recent URL a Ctrl-click opened (most-recent-wins), or `None` if
    /// none this session. Observable so an interaction test can assert that a
    /// Ctrl-click on a URL in the grid opened it — the OS-opener side effect
    /// (`ctx.open_url`) itself is not observable in the headless harness.
    last_opened_url: Option<String>,
    /// Whether the in-terminal find overlay is open.
    search_open: bool,
    /// The find overlay's search query.
    search_query: String,
    /// Whether the find query is treated as a regular expression.
    search_regex: bool,
    /// Whether find matching is case-SENSITIVE (the core option speaks
    /// `case_insensitive`, so this is its inverse — the UI label is "Case").
    search_case_sensitive: bool,
    /// The matches found this frame for `search_query` over the focused pane's
    /// grid text, recomputed by [`Self::recompute_search`] whenever the query or
    /// a toggle changes (and once on open). Kept on `self` so the cycle keys
    /// (Enter / F3 / Shift+F3) and the highlight pass both read the same set.
    search_matches: Vec<c0pl4nd_core::search::SearchMatch>,
    /// Index of the currently-selected match in `search_matches` (0-based).
    /// Meaningful only when `search_matches` is non-empty.
    search_sel: usize,
    /// TEST-ONLY corpus override for the find overlay. When `Some`, the matcher
    /// searches these lines instead of the live PTY grid. The live PTY's
    /// `grid_text()` is async + platform-dependent (a CI box may have no usable
    /// shell), so the headless find tests seed a KNOWN corpus here to assert the
    /// search wiring deterministically. `None` in the shipping binary — the real
    /// focused-pane grid text is searched. Set via `test_seed_focused_grid`.
    search_test_corpus: Option<String>,
    /// A transient status-bar message (e.g. "max 6 panes").
    toast: Option<String>,
    /// Receiver for an opt-in launch update check spawned by the binary entry
    /// point (`egui_main`). The background thread sends a one-line "newer
    /// version available" notice exactly once; `frame_tick` polls this and
    /// surfaces it as a toast. `None` in the headless harness (tests never attach
    /// a check), so no network ever runs under test.
    update_rx: Option<std::sync::mpsc::Receiver<String>>,
    /// The most recent update notice surfaced (most-recent-wins), observable so
    /// an interaction test can assert the launch-check → toast wiring without a
    /// network call.
    last_update_notice: Option<String>,
    /// The most recent caption command issued (minimize/maximize/close). Set in
    /// [`Self::frame_tick`] alongside the real `ViewportCommand`, so interaction
    /// tests can assert that clicking a caption button had its real effect (the
    /// OS command itself is not observable in a headless harness).
    last_window_cmd: Option<WindowCmd>,
    /// True when running in a real eframe window (a wgpu render state exists),
    /// false in the headless `egui_kittest` harness. Drives the per-frame
    /// `request_repaint` pump so live PTY output animates without an input
    /// event — but NOT in headless tests, where an unconditional repaint would
    /// make `Harness::run` loop until `max_steps`.
    live_window: bool,
    /// Frameless terminal-only fullscreen (#36), toggled by F11 (and exited by
    /// F11 or Esc). TRANSIENT — never persisted to `Config`: F11 is a per-session
    /// view toggle, not a saved preference, so a relaunch is always windowed.
    /// While true, the titlebar + status panels (and the frameless resize bands)
    /// are not rendered, so only the grid fills the screen. The local mirror is
    /// the source of truth the panels read THIS frame (the OS-reported
    /// `i.viewport().fullscreen` lags a frame, which would flash the titlebar);
    /// it is reconciled from the OS value each frame to stay honest.
    fullscreen: bool,
    /// Whether the OS window held focus on the previous frame. Drives DEC
    /// `?1004` focus reporting: on a focus-in/out EDGE the focused pane's
    /// terminal is told (so vim/tmux see FocusGained/FocusLost). Initialised
    /// `true` so a window that starts focused does not emit a spurious report.
    was_focused: bool,
    /// Per-(pane,row) laid-out galley cache for [`paint_grid_native`] (audit #2).
    /// A row's galley is re-laid-out only when its content/style key changes, so
    /// an idle or partially-changed grid does not re-run text layout for every
    /// row every frame. Invalidated implicitly by the key (which folds font size,
    /// default fg, and the chromatic ghost params); cleared wholesale on a font
    /// re-install (family/fallback change). Bounded by per-pane row pruning.
    galley_cache: GalleyCache,
    /// Receiver for the off-thread system-font load (audit #3). When the default
    /// (or any custom) font config names a non-built-in family,
    /// `load_system_fonts()` (100s of ms) would block first paint; instead the
    /// first frame paints with the built-in mono and a worker thread enumerates
    /// the system font DB, sending the finished `FontDefinitions` here. `frame_tick`
    /// polls this and applies them via `set_fonts` when ready. `None` once applied
    /// (or when no system load is needed). Skipped entirely in the headless
    /// harness (no `live_window`), which keeps the synchronous path for
    /// deterministic tests.
    pending_fonts: Option<std::sync::mpsc::Receiver<egui::FontDefinitions>>,
    /// The in-progress IME pre-edit (composition) string for the focused pane,
    /// or `None` when no composition is active (F3-1). egui routes composed CJK /
    /// complex-script input through `Event::Ime` — the not-yet-committed
    /// candidate text arrives as `ImeEvent::Preedit` and is BUFFERED here for
    /// display only; it is NEVER sent to the PTY (only `ImeEvent::Commit` text
    /// reaches the shell). Painted underlined at the cursor by
    /// [`Self::render_pane_body`] so the user sees what they are composing before
    /// commit. Cleared on `ImeEvent::Enabled` / `Disabled` and on commit.
    ime_preedit: Option<String>,
}

/// The PTY grid size used to spawn a pane before its real pixel rect is known.
/// The first `resize_to_px` corrects it to fit the allocated rect.
const SPAWN_COLS: u16 = 80;
/// See [`SPAWN_COLS`].
const SPAWN_ROWS: u16 = 24;

/// The `config.font.line_height` value (PIXELS) that maps to a row-pitch
/// multiplier of exactly `1.0` — i.e. "natural" spacing (the rendered glyph's
/// own galley height). The field is an absolute pixel line-height (default
/// 20.0; settings slider 12..=48 px); this anchor turns it into a multiplier
/// RELATIVE to the default so the default config reproduces the natural pitch
/// (`20.0 / 20.0 == 1.0`) and raising the slider opens the rows up
/// proportionally, lowering it tightens them — without breaking the existing
/// absolute-px config field or its settings slider.
const LINE_HEIGHT_ANCHOR_PX: f32 = 20.0;

/// Convert the configured `config.font.line_height` (absolute px, default 20.0)
/// into a row-pitch MULTIPLIER relative to the natural galley height. Pure +
/// GPU-free so the pitch wiring is unit-testable.
///
/// * `line_height_px == LINE_HEIGHT_ANCHOR_PX` (the 20.0 default) → `1.0`
///   (natural spacing).
/// * A larger configured line-height → a multiplier `> 1.0` (looser rows).
/// * A smaller one → a multiplier `< 1.0` (tighter rows).
///
/// Clamped to a sane `0.5..=4.0` band so a corrupt config can neither collapse
/// rows onto each other nor scatter them across the pane.
fn line_height_multiplier(line_height_px: f32) -> f32 {
    if !line_height_px.is_finite() || line_height_px <= 0.0 {
        return 1.0;
    }
    (line_height_px / LINE_HEIGHT_ANCHOR_PX).clamp(0.5, 4.0)
}

/// The effective terminal ROW PITCH (vertical advance per grid row) given the
/// natural per-row galley height `natural_line_h` and the configured
/// `line_height_px`. This single helper is the source of truth shared by the
/// glyph painter, the cursor, the search highlight, the hyperlink hit-test, and
/// the PTY `(cols, rows)` resize math, so every Y position stays aligned to the
/// SAME pitch (the bug class where the cursor drifts off the text when the row
/// pitch changes). Pure + GPU-free → unit-testable without an egui frame.
fn effective_row_pitch(natural_line_h: f32, line_height_px: f32) -> f32 {
    (natural_line_h * line_height_multiplier(line_height_px)).max(1.0)
}

impl C0pl4ndApp {
    /// Build the app inside eframe, applying the brand Visuals + window effect,
    /// and computing the terminal cell metrics from egui's monospace font (the
    /// font the grid is actually drawn with). Marks the app as a live window so
    /// the per-frame repaint pump runs.
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        // Load persisted settings from disk so a user's saved theme / opacity /
        // font / cursor / update prefs take effect across launches. The headless
        // `bootstrap()` path keeps `Config::default()` for deterministic tests.
        // F5-2: load config AND capture any parse error, so a broken config file
        // surfaces as a visible toast instead of the silent fallback-to-defaults
        // that previously only `eprintln`'d (invisible to a GUI-launched user).
        let (cfg, config_error) = load_config_with_status();
        let mut app = Self::bootstrap_with(cfg);
        if let Some(err) = config_error {
            app.toast = Some(err);
        }
        // F5-3: first-run affordance. A fresh install has no config file yet —
        // and "zero-config is a first-class goal", so we deliberately do NOT
        // write one (that would defeat it). Surface a one-time welcome toast
        // pointing at Settings + the docs; it naturally stops once the user saves
        // any setting (which is what first writes the config file). Skipped when a
        // config-parse error already claimed the toast.
        if app.toast.is_none() && c0pl4nd_core::Config::default_path().is_some_and(|p| !p.exists())
        {
            app.toast = Some(
                "Welcome to C0PL4ND — open Settings (the gear) to customise; \
                 see TROUBLESHOOTING.md if anything looks off."
                    .to_string(),
            );
        }
        // Install the chrome icon fonts so the very first frame renders. When the
        // configured monospace family is a built-in choice this is the complete
        // install. When it names a SYSTEM family (the default config does —
        // "Monaspace Neon" / "Noto Sans JP"), the (100s-of-ms) system-font DB
        // load would block first paint, so instead we install the built-in base
        // immediately and enumerate the system DB on a worker thread (audit #3);
        // `frame_tick` swaps in the custom stack via `set_fonts` when it arrives.
        // Done after the config load so `app.config.font` is the source of truth.
        if system_font_load_needed(&app.config.font) {
            install_base_fonts(&cc.egui_ctx);
            app.pending_fonts = Some(spawn_system_font_load(&app.config.font));
        } else {
            install_chrome_fonts(&cc.egui_ctx, &app.config.font);
        }
        app.applied_font_family = font_apply_key(&app.config.font);
        // Apply the OS glass/acrylic/mica/vibrancy effect — ONLY when the master
        // transparency toggle is on AND the chosen mode wants a non-opaque
        // surface (`effective_translucent`). Otherwise the window is a normal
        // opaque window: no layered surface, so no DWM ghost-on-close risk.
        // Done after `bootstrap()` so `app.config` is the source of truth.
        if app.config.effective_translucent() {
            apply_window_effect(cc, app.config.window_mode, &app.config.tint);
        }
        // Apply Visuals DERIVED FROM the loaded terminal theme so the whole
        // chrome follows the active theme from the first frame (a light theme →
        // light UI, a dark theme → dark UI). Done after `bootstrap()` so
        // `app.theme` is loaded.
        cc.egui_ctx
            .set_visuals(theme::visuals_from_theme(&app.theme));
        app.fonts_installed = true; // already installed above; skip the frame-tick install
                                    // A wgpu render state means a real window (also true under the wgpu test
                                    // harness, which drives frames explicitly with `step()`); headless tests
                                    // built via `bootstrap()` leave this false.
        app.live_window = cc.wgpu_render_state.is_some();
        app
    }

    /// Construct the app state independent of eframe — used by `new` and by the
    /// headless `egui_kittest` tests (which run without a window). The initial
    /// pane(s) are registered in `pending_spawn` WITHOUT a PTY; each is spawned
    /// at its MEASURED size on the first frame its rect is known (bug #40), so
    /// the first pane behaves like a manually-opened one. A failed spawn degrades
    /// to an error label, never a panic.
    /// Default-config constructor used by the headless `egui_kittest` test
    /// binaries (the real app uses [`Self::new`], which loads the persisted
    /// config). `#[allow(dead_code)]` because it is unused in the shipping
    /// `c0pl4nd` binary itself — only the `#[path]`-including test bins call it.
    #[allow(dead_code)]
    pub fn bootstrap() -> Self {
        Self::bootstrap_with(c0pl4nd_core::Config::default())
    }

    /// Construct the app state from an EXPLICIT config — the shared body of
    /// [`Self::bootstrap`] (which passes `Config::default()`, used by the
    /// headless tests) and [`Self::new`] (which passes the config loaded from
    /// disk so persisted settings take effect across launches).
    pub fn bootstrap_with(config: c0pl4nd_core::Config) -> Self {
        let theme = load_terminal_theme(&config);
        let mut pane_alloc = PaneIdAllocator::default();
        let initial: Vec<PaneId> = (0..INITIAL_PANES).map(|_| pane_alloc.next()).collect();
        let focused_pane = initial[0];
        let grid_tree = grid::build_default_grid(&initial);
        // DEFER the initial pane PTYs: register them as pending and let
        // `render_pane_body` spawn each at the MEASURED `(cols, rows)` on the
        // first frame its rect is known (bug #40). Spawning here at the 80×24
        // placeholder is exactly what desynced cmd's cursor when the first
        // `resize_to_px` reflowed it to the real (e.g. 200-col) width.
        let terms: HashMap<PaneId, PaneTerm> = HashMap::new();
        let pending_spawn: HashSet<PaneId> = initial.iter().copied().collect();
        Self {
            config,
            theme,
            grid_tree,
            terms,
            pending_spawn,
            pane_alloc,
            focused_pane,
            pinned: HashSet::new(),
            last_focused_size: None,
            shell_profiles: shells::detect_profiles(),
            active_shell: 0,
            fonts_installed: false,
            applied_font_family: String::new(),
            applied_ui_scale: f32::NAN,
            settings_open: false,
            cmd_history: c0pl4nd_core::command_history::CommandHistory::default(),
            input_line: String::new(),
            pending_paste: None,
            incognito: false,
            palette_open: false,
            history_open: false,
            history_filter: String::new(),
            palette_query: String::new(),
            palette_sel: 0,
            last_palette_run: None,
            last_opened_url: None,
            search_open: false,
            search_query: String::new(),
            search_regex: false,
            search_case_sensitive: false,
            search_matches: Vec::new(),
            search_sel: 0,
            search_test_corpus: None,
            toast: None,
            update_rx: None,
            last_update_notice: None,
            last_window_cmd: None,
            live_window: false,
            fullscreen: false,
            was_focused: true,
            galley_cache: GalleyCache::default(),
            pending_fonts: None,
            ime_preedit: None,
        }
    }

    /// Spawn a fresh live terminal for `pid` running the active shell profile,
    /// and register it. Used by `split`. The default profile (program `None`,
    /// index 0) uses the platform default shell; a named profile launches its
    /// explicit program + args. A failed spawn degrades to an error pane.
    fn spawn_term(&mut self, pid: PaneId) {
        let theme = self.theme.clone();
        let profile = self.shell_profiles.get(self.active_shell);
        let term = match profile.and_then(|p| p.program.clone()) {
            Some(program) => {
                let args: Vec<String> = profile.map(|p| p.args.clone()).unwrap_or_default();
                let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
                PaneTerm::spawn_program(theme, &program, &arg_refs, SPAWN_COLS, SPAWN_ROWS)
            }
            None => PaneTerm::spawn_with_term(
                theme,
                SPAWN_COLS,
                SPAWN_ROWS,
                Some(self.config.term.as_str()),
            ),
        };
        self.terms.insert(pid, term);
    }

    // ---- public observation surface (production accessors, NOT test-only) ----
    //
    // These are real accessors that the `egui_kittest` interaction tests use to
    // assert observable outcomes after driving the REAL `frame_tick`. They are
    // deliberately not `#[cfg(test)]` so the test exercises the exact production
    // path (no test-only mirror that could drift from the real frame loop — that
    // drift is how "clicking does nothing" ships). `allow(dead_code)` because the
    // shipping binary does not yet call every accessor (the test crate, compiled
    // separately via `#[path]`, is the current consumer); they are a deliberate
    // public observation API, not dead code.
    #[allow(dead_code)]
    /// Number of open panes in the grid.
    pub fn pane_count(&self) -> usize {
        count_panes(&self.grid_tree)
    }

    /// Whether the settings window is currently open.
    #[allow(dead_code)]
    pub fn settings_is_open(&self) -> bool {
        self.settings_open
    }

    /// The current pane shell layout (`Grid` or `Tabs`) from the live config —
    /// the value the titlebar view-toggle button flips and that `grid_ui` reads
    /// each frame to decide whether to render the egui_tiles tree or a single
    /// full-size pane. Observation accessor for the view-toggle interaction test.
    #[allow(dead_code)]
    pub fn view_mode(&self) -> c0pl4nd_core::config::ViewMode {
        self.config.view_mode
    }

    /// Number of live terminal sessions currently held. Observation accessor for
    /// the fast-shutdown test: after [`prepare_shutdown`](Self::prepare_shutdown)
    /// this MUST be zero (every `PaneTerm` dropped → every PTY child killed, no
    /// orphans).
    #[allow(dead_code)]
    pub fn term_count(&self) -> usize {
        self.terms.len()
    }

    /// The currently-focused pane id.
    #[allow(dead_code)]
    pub fn focused_pane(&self) -> PaneId {
        self.focused_pane
    }

    /// Whether `pane_id` is currently pinned (tab sorts first, × hidden).
    #[allow(dead_code)]
    pub fn is_pinned(&self, pane_id: PaneId) -> bool {
        self.pinned.contains(&pane_id)
    }

    /// The pane's UNIQUE accessible tab label — the same string
    /// [`chrome`](super::chrome) sets as the tab's accessible name AND the base
    /// of its `pin`/`close` button labels, so an interaction test can look up a
    /// tab by `get_by_label(label)` without hardcoding a value the shell's
    /// window-title escape would change. The label is dynamic precisely because
    /// the title feature makes it so — a real shell sets its own title, so a
    /// fixed `"pane 0"` literal is no longer a stable lookup key.
    #[allow(dead_code)]
    pub fn tab_label_for_pane(&self, pane_id: PaneId) -> Option<String> {
        self.pane_titles()
            .into_iter()
            .find(|(id, _)| *id == pane_id)
            .map(|(id, label)| Self::tab_a11y_label(id, &label))
    }

    /// A pane's UNIQUE accessible tab label, derived from its displayed tab text.
    ///
    /// The VISIBLE tab text is just the title (or the `pane {id}` fallback), but
    /// two shells launched in the same directory routinely set the SAME OSC
    /// window title — so the visible text alone is NOT unique. An ambiguous
    /// accessible name is a real defect: a screen reader cannot distinguish the
    /// two tabs, and the accessibility tree has two nodes with one name (which
    /// also makes `get_by_label` lookups ambiguous). This stable-by-construction
    /// label fixes that by anchoring every label on the unique `pane {id}`:
    ///
    /// - untitled pane → `pane {id}` (already unique; no redundant suffix)
    /// - titled pane   → `{title} (pane {id})` (title for context + id for
    ///   uniqueness; the title is kept first so WCAG 2.5.3 "Label in Name" holds
    ///   against the visible text)
    fn tab_a11y_label(pane_id: PaneId, display: &str) -> String {
        let fallback = format!("pane {}", pane_id.raw());
        if display == fallback {
            fallback
        } else {
            format!("{display} (pane {})", pane_id.raw())
        }
    }

    /// The most recent caption command the user issued (min/max/close), or
    /// `None` if no caption button has been clicked this session.
    #[allow(dead_code)]
    pub fn last_window_cmd(&self) -> Option<WindowCmd> {
        self.last_window_cmd
    }

    /// The visible grid text of a pane's terminal, or `None` if the pane has no
    /// live terminal. Used by interaction tests to assert that PTY output landed
    /// on screen (the load-bearing type→PTY→grid round-trip).
    #[allow(dead_code)]
    pub fn pane_grid_text(&self, pane_id: PaneId) -> Option<String> {
        self.terms.get(&pane_id).and_then(PaneTerm::grid_text)
    }

    /// The focused pane's visible grid text. Convenience over
    /// [`Self::pane_grid_text`] for the common test assertion.
    #[allow(dead_code)]
    pub fn focused_grid_text(&self) -> Option<String> {
        self.pane_grid_text(self.focused_pane)
    }

    /// A pane's PTY grid size `(cols, rows)`, or `None` if it has no terminal.
    /// Used by the resize→PTY interaction test.
    #[allow(dead_code)]
    pub fn pane_size(&self, pane_id: PaneId) -> Option<(u16, u16)> {
        self.terms.get(&pane_id).map(PaneTerm::size)
    }

    /// The ids of every pane with a live terminal, in unspecified order. Used by
    /// tests to enumerate panes for focus routing assertions.
    #[allow(dead_code)]
    pub fn pane_ids(&self) -> Vec<PaneId> {
        self.pane_titles().into_iter().map(|(id, _)| id).collect()
    }

    /// Maximum displayed length of an OSC-derived tab title before it is
    /// truncated with an ellipsis. A program can set an arbitrarily long title
    /// (e.g. a full `user@host: /deep/path` string); the tab strip caps it so
    /// one verbose pane cannot blow out the whole strip.
    const MAX_TAB_TITLE: usize = 32;

    /// `(pane_id, title)` for every pane in the grid, in STABLE visual order
    /// (left→right, top→bottom). Built by walking the tree from the root via
    /// [`grid::panes_in_visual_order`] — NOT by iterating the `ahash::HashMap`
    /// storage, whose order changes every process launch (the "tab order
    /// reshuffles between launches" bug). The tab strip and every consumer of
    /// this list therefore stay in a fixed, on-screen-matching order.
    ///
    /// Each tab label is the running program's live OSC 0/2 title (trimmed and
    /// capped to [`Self::MAX_TAB_TITLE`] chars, with a `…` suffix when longer)
    /// when the program has set one — like every real terminal. Panes that have
    /// no title yet (a fresh shell, or one whose program never set a title) fall
    /// back to the generic `pane {id}` label, so untitled panes read identically
    /// to before.
    fn pane_titles(&self) -> Vec<(PaneId, String)> {
        grid::panes_in_visual_order(&self.grid_tree)
            .into_iter()
            .map(|pane_id| {
                let label = self
                    .terms
                    .get(&pane_id)
                    .and_then(PaneTerm::title)
                    .map(|t| Self::cap_tab_title(&t))
                    .unwrap_or_else(|| format!("pane {}", pane_id.raw()));
                (pane_id, label)
            })
            .collect()
    }

    /// Trim a raw OSC title and cap it to [`Self::MAX_TAB_TITLE`] CHARACTERS
    /// (not bytes — a multi-byte glyph is never split), appending `…` when the
    /// title was actually shortened. The raw title is first run through
    /// [`scrub_display_text`] so a hostile program/SSH host cannot inject bidi,
    /// zero-width, or control characters into the tab label.
    fn cap_tab_title(raw: &str) -> String {
        let scrubbed = scrub_display_text(raw);
        let trimmed = scrubbed.trim();
        if trimmed.chars().count() <= Self::MAX_TAB_TITLE {
            trimmed.to_string()
        } else {
            let kept: String = trimmed.chars().take(Self::MAX_TAB_TITLE).collect();
            format!("{kept}…")
        }
    }

    /// Split the focused pane, allocating a fresh placeholder pane. Refused (with
    /// a toast) at the 6-pane cap.
    fn split(&mut self, dir: egui_tiles::LinearDir) {
        if count_panes(&self.grid_tree) >= grid::MAX_PANES {
            self.toast = Some(format!("max {} panes", grid::MAX_PANES));
            return;
        }
        let new_pane = self.pane_alloc.next();
        if grid::split_focused(&mut self.grid_tree, self.focused_pane, new_pane, dir) {
            self.spawn_term(new_pane);
            self.focused_pane = new_pane;
            self.toast = None;
        }
    }

    /// Open a new terminal (the single "+" button). Splits the focused pane
    /// along its LONGER axis so panes stay balanced: a wide pane splits
    /// left|right, a tall pane splits top/bottom. This gives a "logical" grid
    /// expansion without asking the user to pick a direction.
    ///
    /// `pub` so the headless interaction tests can drive the split path directly
    /// (the same path the "+" button triggers) — the blank-pane-on-split
    /// regression test exercises this.
    pub fn new_terminal(&mut self) {
        let (w, h) = self.last_focused_size.unwrap_or((16.0, 9.0));
        let dir = if w >= h {
            egui_tiles::LinearDir::Horizontal // wide → side-by-side
        } else {
            egui_tiles::LinearDir::Vertical // tall → stacked
        };
        self.split(dir);
    }

    /// Make shell profile `idx` active and open a new terminal running it (the
    /// top-bar ▾ menu path). Subsequent plain "+" presses then use the same
    /// shell, mirroring the Windows-Terminal "+ ▾" profile behaviour. An
    /// out-of-range index is ignored (defensive — the menu only emits valid
    /// indices).
    fn open_shell(&mut self, idx: usize) {
        if idx < self.shell_profiles.len() {
            self.active_shell = idx;
            self.new_terminal();
        }
    }

    /// The shell profiles offered by the top-bar switcher (platform default
    /// first). Used by the chrome to render the ▾ menu.
    pub fn shell_profiles(&self) -> &[shells::ShellProfile] {
        &self.shell_profiles
    }

    /// The label of the currently-active shell profile (what new terminals run).
    /// Used by the chrome's hover text and by interaction tests.
    pub fn active_shell_label(&self) -> &str {
        self.shell_profiles
            .get(self.active_shell)
            .map(|p| p.label.as_str())
            .unwrap_or("Default shell")
    }

    /// Paint one terminal pane's body and wire its per-frame interaction:
    ///
    /// 1. Allocate the pane rect and paint the theme background quad + focus ring
    ///    behind the glyphs (so text never blends directly against the acrylic).
    /// 2. Compute the physical-pixel size and DEBOUNCED-resize the PTY to fit.
    /// 3. DISPLAY the visible grid with egui's native coloured-text painter via
    ///    [`paint_grid_native`].
    /// 4. Report click (refocus) + drag-start (egui_tiles).
    ///
    /// A failed-spawn pane paints an error label instead of a grid — never a
    /// panic. This is a FREE function (not `&mut self`) so the `grid_ui` closure
    /// can borrow `terms`/`theme` disjointly from `self.grid_tree` (which
    /// `tree.ui` borrows mutably) — the classic egui_tiles borrow split.
    #[allow(clippy::too_many_arguments)]
    fn render_pane_body(
        ui: &mut egui::Ui,
        pane_id: PaneId,
        focused: bool,
        terms: &mut HashMap<PaneId, PaneTerm>,
        pending_spawn: &mut HashSet<PaneId>,
        galley_cache: &mut GalleyCache,
        theme: &c0pl4nd_core::Theme,
        // The configured `TERM` advertised to a deferred-first-spawn pane, so the
        // initial pane's child PTY sees the same `TERM` as every later pane.
        term: &str,
        font_size: f32,
        line_height_px: f32,
        cursor_cfg: c0pl4nd_core::config::CursorConfig,
        effects: c0pl4nd_core::config::EffectsConfig,
        padding: f32,
        bg_alpha: u8,
        search: Option<SearchHighlight<'_>>,
        links: &[(CellSpan, String)],
        // The focused pane's in-progress IME pre-edit (composition) string, for
        // display at the cursor (F3-1). `None` for non-focused panes and when no
        // composition is active. Never sent to the PTY — display only.
        ime_preedit: Option<&str>,
    ) -> PaneBodyOutcome {
        let (rect, resp) =
            ui.allocate_exact_size(ui.available_size(), egui::Sense::click_and_drag());
        let ppp = ui.ctx().pixels_per_point();
        let painter = ui.painter_at(rect);
        // Cell metrics from the SAME monospace font the grid is drawn with, so
        // the PTY's `(cols, rows)` match the rendered glyph size. Measured via a
        // probe galley (`Painter::layout_job`); ppp scales points → physical px
        // to match the `rect * ppp` resize math below. The configured
        // Line-height folds into the row pitch here so the PTY reflows to the
        // SAME pitch the painter draws at.
        let cell_metrics = monospace_cell_metrics(&painter, font_size, ppp, line_height_px);

        // --- deferred first-spawn at the MEASURED size (bug #40) ---
        // The configurable inner padding (points) insets the text on every edge,
        // so the grid area is the rect minus 2×padding per axis. We need it both
        // for the deferred spawn (below) and the debounced resize (further down),
        // so compute it ONCE here.
        let pad = padding.max(0.0);
        let px_w = (rect.width() - 2.0 * pad).max(0.0) * ppp;
        let px_h = (rect.height() - 2.0 * pad).max(0.0) * ppp;
        // A pane whose PTY was deferred (the initial pane) is spawned HERE, at the
        // real `(cols, rows)` derived from its measured rect — exactly the size
        // `resize_to_px` would otherwise reflow it to a frame later. Spawning at
        // the correct size up front means the subsequent debounced `resize_to_px`
        // is a no-op, so cmd's banner/prompt cursor never snaps home to (0,0).
        if pending_spawn.remove(&pane_id) {
            let (cols, rows) = cell_metrics.cols_rows(px_w, px_h);
            terms.insert(
                pane_id,
                PaneTerm::spawn_with_term(theme.clone(), cols, rows, Some(term)),
            );
        }

        // --- background quad (theme bg) + focus ring ---
        // `bg_alpha` is 255 for an opaque window and the opacity-folded alpha
        // when the window is effectively translucent — painting the pane fill
        // non-opaque is what lets the OS acrylic/mica blur (or, in Transparent
        // mode, the desktop) show THROUGH the grid. An opaque fill here would
        // cover the transparent clear-color and defeat the whole DWM backdrop,
        // which is exactly why transparency "did nothing" before.
        let bg = terms
            .get(&pane_id)
            .map(PaneTerm::background_rgb)
            .unwrap_or((18, 18, 18));
        painter.rect_filled(
            rect,
            egui::CornerRadius::same(4),
            egui::Color32::from_rgba_unmultiplied(bg.0, bg.1, bg.2, bg_alpha),
        );
        // Focus ring + bezel follow the active theme (accent on focus, bezel
        // otherwise) so the grid chrome matches the rest of the themed UI.
        let pane_colors = theme::ChromeColors::from_theme(theme);
        let stroke = if focused {
            egui::Stroke::new(2.0, pane_colors.accent)
        } else {
            egui::Stroke::new(1.0, pane_colors.bezel)
        };
        painter.rect_stroke(
            rect,
            egui::CornerRadius::same(4),
            stroke,
            egui::StrokeKind::Inside,
        );

        // --- resize the PTY to fit this rect (debounced) ---
        // The configurable inner padding (points) insets the text on every edge,
        // so the area available for the terminal grid is the rect minus 2×padding
        // on each axis. Subtract it BEFORE the px conversion so the computed
        // (cols, rows) match what `paint_grid_native` actually draws inside the
        // padded origin — otherwise a large padding would size the PTY for the
        // full rect and clip the last row/column.
        let pad = padding.max(0.0);
        let px_w = (rect.width() - 2.0 * pad).max(0.0) * ppp;
        let px_h = (rect.height() - 2.0 * pad).max(0.0) * ppp;
        if let Some(term) = terms.get_mut(&pane_id) {
            term.resize_to_px(px_w, px_h, cell_metrics);
        }

        // --- display the grid ---
        match terms.get(&pane_id) {
            Some(term) if term.error().is_none() => {
                // Single native render path for BOTH the live window and headless
                // snapshots (see `paint_grid_native`). egui's own glyph painter
                // draws the coloured grid reliably on the real swapchain — the
                // glyphon GPU paths (callback + offscreen texture) composited
                // black inside `egui_tiles` panes live while passing the wgpu
                // test harness.
                paint_grid_native(
                    &painter,
                    rect,
                    term,
                    pane_id,
                    galley_cache,
                    font_size,
                    line_height_px,
                    theme,
                    focused,
                    cursor_cfg,
                    effects,
                    pad,
                );
                // Find-overlay highlight: tint every match span (and outline the
                // active one) over the rendered grid. Only the focused pane while
                // the overlay is open carries a `SearchHighlight`.
                if let Some(hl) = search {
                    paint_search_highlight(
                        &painter,
                        rect,
                        font_size,
                        line_height_px,
                        pad,
                        &pane_colors,
                        hl,
                    );
                }
            }
            Some(term) => {
                // Failed spawn: show the error, never panic.
                painter.text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    term.error().unwrap_or("terminal unavailable"),
                    egui::FontId::monospace(14.0),
                    pane_colors.fg,
                );
            }
            None => {
                painter.text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    format!("pane {} (no terminal)", pane_id.raw()),
                    egui::FontId::monospace(14.0),
                    pane_colors.fg,
                );
            }
        }

        // Ctrl-clickable hyperlinks. `links` is non-empty ONLY for the focused
        // pane while Ctrl (or Cmd) is held — the caller gates it — so its mere
        // presence means "the modifier is down this frame". Underline every URL,
        // show a hand cursor over the one under the pointer, and open the one a
        // click lands on. The pixel→cell mapping ([`cell_at_pos`]) and the span
        // hit test ([`link_url_at_cell`]) are pure + unit-tested; only this thin
        // wiring + the OS-opener side effect live here.
        let mut opened_url = None;
        if !links.is_empty() {
            let (cw, ch) = monospace_cell_points(&painter, font_size, line_height_px);
            let origin = grid_text_origin(rect, pad);
            paint_link_underlines(&painter, origin, cw, ch, &pane_colors, links);
            if let Some(hover) = resp.hover_pos() {
                if let Some((r, c)) = cell_at_pos(hover, origin, cw, ch) {
                    if link_url_at_cell(links, r, c).is_some() {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                    }
                }
            }
            if resp.clicked() {
                if let Some(click) = resp.interact_pointer_pos() {
                    if let Some((r, c)) = cell_at_pos(click, origin, cw, ch) {
                        if let Some(url) = link_url_at_cell(links, r, c) {
                            ui.ctx().open_url(egui::OpenUrl::new_tab(url));
                            opened_url = Some(url.to_string());
                        }
                    }
                }
            }
        }

        // --- mouse reporting (E6) + local wheel scrollback ---
        // When the program in this pane has grabbed the mouse (?1000/?1002/?1003)
        // translate pointer gestures into `encode_mouse` reports written to its
        // PTY (mouse in vim/tmux/htop/less). Otherwise the wheel scrolls this
        // pane's local scrollback. Two conventional overrides force LOCAL
        // handling even while a program grabs the mouse: holding Shift (the
        // standard "let me select/scroll" escape) and Ctrl-with-a-link-under-the-
        // pointer (`links` is non-empty only then — that click opens the URL).
        // Without this the canonical egui binary reported NO mouse at all and
        // could not scroll back through history; the legacy winit shell did both.
        let mut mouse_captured = false;
        {
            use c0pl4nd_core::term::{MouseButton, MouseEventKind, MouseMode, MouseModifiers};
            let (cw, ch) = monospace_cell_points(&painter, font_size, line_height_px);
            let origin = grid_text_origin(rect, pad);
            // 1-based (col, row) of a screen-space point over the grid, if any.
            let cell_of = |pos: egui::Pos2| -> Option<(usize, usize)> {
                cell_at_pos(pos, origin, cw, ch).map(|(r, c)| (c + 1, r + 1))
            };
            let m = ui.input(|i| i.modifiers);
            let mods = MouseModifiers {
                shift: m.shift,
                alt: m.alt,
                control: m.ctrl,
            };
            let scroll_y = ui.input(|i| i.smooth_scroll_delta.y);
            let mode = terms
                .get(&pane_id)
                .map(PaneTerm::mouse_mode)
                .unwrap_or(MouseMode::Off);
            // Report to the program only when it grabbed the mouse, Shift is not
            // forcing local selection, and we are not in ctrl-click-link mode.
            let report = mode != MouseMode::Off && !m.shift && links.is_empty();
            if report {
                // Button press/release at the interacted cell.
                let buttons = [
                    (egui::PointerButton::Primary, MouseButton::Left),
                    (egui::PointerButton::Middle, MouseButton::Middle),
                    (egui::PointerButton::Secondary, MouseButton::Right),
                ];
                let pos = resp
                    .interact_pointer_pos()
                    .or(resp.hover_pos())
                    .or_else(|| ui.input(|i| i.pointer.latest_pos()));
                if let Some(pos) = pos {
                    if let Some((col, row)) = cell_of(pos) {
                        if let Some(term) = terms.get_mut(&pane_id) {
                            for (egui_btn, term_btn) in buttons {
                                if ui.input(|i| i.pointer.button_pressed(egui_btn)) {
                                    mouse_captured |= term.report_mouse(
                                        term_btn,
                                        mods,
                                        col,
                                        row,
                                        MouseEventKind::Press,
                                    );
                                }
                                if ui.input(|i| i.pointer.button_released(egui_btn)) {
                                    term.report_mouse(
                                        term_btn,
                                        mods,
                                        col,
                                        row,
                                        MouseEventKind::Release,
                                    );
                                }
                            }
                            // Motion: ?1002 reports drag (button held), ?1003 any
                            // motion. encode_mouse gates by mode, so a bare hover
                            // under ?1002 yields nothing.
                            if resp.dragged() || resp.hovered() {
                                let held = if ui.input(|i| i.pointer.primary_down()) {
                                    MouseButton::Left
                                } else if ui.input(|i| i.pointer.secondary_down()) {
                                    MouseButton::Right
                                } else if ui.input(|i| i.pointer.middle_down()) {
                                    MouseButton::Middle
                                } else {
                                    MouseButton::None
                                };
                                if term.report_mouse(held, mods, col, row, MouseEventKind::Motion) {
                                    mouse_captured = true;
                                }
                            }
                        }
                    }
                }
                // Wheel → buttons 64/65 (one report per ~cell of travel, capped).
                if scroll_y.abs() > f32::EPSILON {
                    let pos = resp
                        .hover_pos()
                        .or_else(|| ui.input(|i| i.pointer.latest_pos()));
                    if let (Some(pos), Some(term)) = (pos, terms.get_mut(&pane_id)) {
                        if let Some((col, row)) = cell_of(pos) {
                            let btn = if scroll_y > 0.0 {
                                MouseButton::WheelUp
                            } else {
                                MouseButton::WheelDown
                            };
                            let ticks = ((scroll_y.abs() / ch.max(1.0)).round() as i32).clamp(1, 8);
                            for _ in 0..ticks {
                                term.report_mouse(btn, mods, col, row, MouseEventKind::Press);
                            }
                            mouse_captured = true;
                        }
                    }
                }
            } else if scroll_y.abs() > f32::EPSILON && resp.hovered() && !m.command {
                // Local scrollback: wheel up (positive y) goes BACK into history.
                // One cell of pointer travel ≈ one scrollback line. A Ctrl/Cmd-held
                // wheel is reserved for font zoom (handled in frame_tick), so it
                // does NOT scroll here.
                if let Some(term) = terms.get_mut(&pane_id) {
                    let lines = (scroll_y / ch.max(1.0)).round() as i32;
                    if lines != 0 {
                        term.scroll_view(lines);
                    }
                }
            }
        }

        // --- accessibility (F2-1): expose the grid text to screen readers ---
        // The terminal grid is custom-painted, so without an explicit AccessKit
        // node a screen reader perceives only an empty interactive region — the
        // terminal's actual content is invisible to assistive tech. Attach the
        // visible grid text as the pane's accessible value, marking the focused
        // pane active. egui invokes this closure LAZILY and ONLY while building an
        // AccessKit tree (i.e. when a screen reader / `egui_kittest` is attached),
        // so the full-grid `grid_text()` snapshot costs nothing in the common
        // no-assistive-tech case.
        resp.widget_info(|| {
            let text = terms
                .get(&pane_id)
                .and_then(PaneTerm::grid_text)
                .unwrap_or_default();
            egui::WidgetInfo::labeled(egui::WidgetType::Label, focused, text)
        });

        // --- IME composition (F3-1): cursor rect + pre-edit display ---
        // Compute the focused pane's terminal-cursor cell rect in screen space
        // using the SAME geometry the glyph painter, cursor, and link hit-test
        // share (`origin + (col*cw, row*ch)`). The caller hands this rect to
        // `ctx.output_mut(|o| o.ime = Some(IMEOutput {..}))` so winit's
        // `set_ime_cursor_area` places the OS candidate window AT the caret.
        // Only the focused pane reports a rect (the OS tracks a single caret).
        let mut ime_cursor_rect = None;
        if focused {
            if let Some((row, col)) = terms.get(&pane_id).and_then(PaneTerm::cursor_cell) {
                let (cw, ch) = monospace_cell_points(&painter, font_size, line_height_px);
                let origin = grid_text_origin(rect, pad);
                let cell_min = origin + egui::vec2(col as f32 * cw, row as f32 * ch);
                ime_cursor_rect = Some(egui::Rect::from_min_size(cell_min, egui::vec2(cw, ch)));

                // Paint the in-progress pre-edit string at the cursor, underlined
                // and in the theme fg, so the user sees what they are composing
                // before commit. The candidate window (positioned via the rect
                // above) shows the IME's own suggestion list; this is the inline
                // composition echo at the caret. The pre-edit is DISPLAY-ONLY —
                // it is never forwarded to the PTY (only `ImeEvent::Commit` is).
                if let Some(pre) = ime_preedit.filter(|s| !s.is_empty()) {
                    let fg = theme::ChromeColors::from_theme(theme).fg;
                    let font = egui::FontId::monospace(font_size);
                    let galley = painter.layout_no_wrap(pre.to_string(), font, fg);
                    let text_pos = origin + egui::vec2(col as f32 * cw, row as f32 * ch);
                    let galley_w = galley.size().x;
                    painter.galley(text_pos, galley, fg);
                    // Underline the composition span (the conventional pre-edit
                    // affordance), one device-px line at the cell baseline.
                    let underline = egui::Rect::from_min_size(
                        text_pos + egui::vec2(0.0, ch - 1.0),
                        egui::vec2(galley_w, 1.0),
                    );
                    painter.rect_filled(underline, 0.0, fg);
                }
            }
        }

        PaneBodyOutcome {
            // A body-drag normally tells egui_tiles to REARRANGE the pane. When a
            // program grabbed the mouse and we reported the drag to its PTY, the
            // gesture belongs to the program — never rearrange panes underneath it.
            drag_started: resp.drag_started() && !mouse_captured,
            clicked: resp.clicked(),
            size: rect.size(),
            opened_url,
            ime_cursor_rect,
        }
    }

    /// Forward this frame's keyboard + paste events to the FOCUSED pane's PTY,
    /// using the SHARED core key encoder. Consumes Tab/arrows so egui does not
    /// steal them for widget navigation (recon dossier §5.1). Called once per
    /// frame. Returns the bytes forwarded (for tests that drive the real input
    /// path and assert what reached the PTY).
    fn forward_input_to_focused(&mut self, ctx: &egui::Context) -> Vec<u8> {
        use c0pl4nd_core::term::{KeyEventKind, KeyModifiers, LogicalKey};

        // When the focused program negotiated the kitty keyboard protocol with
        // REPORT-EVENT-TYPES (bit2), ALSO forward key RELEASE and REPEAT events;
        // otherwise keep the legacy press-only behavior. Read the flag once.
        let report_event_types = self
            .terms
            .get(&self.focused_pane)
            .map(|t| t.kitty_reports_event_types())
            .unwrap_or(false);

        // Collect input events under the immutable input borrow first, THEN
        // mutate the PTY (egui forbids re-entrant input borrows).
        let mut keys: Vec<(LogicalKey, KeyModifiers, KeyEventKind)> = Vec::new();
        let mut pastes: Vec<String> = Vec::new();
        // The pre-edit (composition) string to store on `self` after the input
        // borrow closes. `Some(Some(s))` = set/replace the preedit; `Some(None)`
        // = clear it; `None` = no IME event this frame, leave it as-is (F3-1).
        let mut ime_update: Option<Option<String>> = None;
        ctx.input(|i| {
            let mods = KeyModifiers {
                ctrl: i.modifiers.ctrl,
                alt: i.modifiers.alt,
                shift: i.modifiers.shift,
                logo: i.modifiers.command || i.modifiers.mac_cmd,
            };
            for ev in &i.events {
                match ev {
                    // Composed text (printable chars, IME). Skip when Ctrl/logo
                    // is held so a shortcut chord (Ctrl+C etc.) is handled by the
                    // Key event below, not double-sent as raw text.
                    egui::Event::Text(t) if !mods.ctrl && !mods.logo => {
                        keys.push((LogicalKey::Text(t.clone()), mods, KeyEventKind::Press));
                    }
                    // IME composition (F3-1). When an IME (CJK / complex-script)
                    // is active, egui routes composed text through `Event::Ime`
                    // INSTEAD of `Event::Text`, so without this arm CJK input is
                    // impossible. The OS candidate-window position is set
                    // separately each frame via `ctx.output_mut(|o| o.ime = ...)`
                    // in `render_pane_body` (so the popup tracks the caret).
                    egui::Event::Ime(ime) => match ime {
                        // Final composed result: send it to the PTY exactly as
                        // ordinary `Event::Text` would, and clear the pre-edit.
                        // Commit text is final and MUST reach the shell
                        // regardless of modifier state (an IME commit is not a
                        // shortcut chord), so — unlike `Event::Text` above — it
                        // is forwarded even while Ctrl/logo is held.
                        egui::ImeEvent::Commit(text) => {
                            if !text.is_empty() {
                                keys.push((
                                    LogicalKey::Text(text.clone()),
                                    mods,
                                    KeyEventKind::Press,
                                ));
                            }
                            ime_update = Some(None);
                        }
                        // In-progress candidate text: buffer for DISPLAY only —
                        // never sent to the PTY. An empty pre-edit ends the
                        // current composition without committing.
                        egui::ImeEvent::Preedit(text) => {
                            ime_update = Some(if text.is_empty() {
                                None
                            } else {
                                Some(text.clone())
                            });
                        }
                        // Composition session boundaries: clear any stale
                        // pre-edit so a cancelled composition leaves nothing
                        // painted at the cursor.
                        egui::ImeEvent::Enabled | egui::ImeEvent::Disabled => {
                            ime_update = Some(None);
                        }
                    },
                    egui::Event::Paste(s) => pastes.push(s.clone()),
                    egui::Event::Key {
                        key,
                        pressed,
                        repeat,
                        modifiers,
                        ..
                    } => {
                        // Press-only by default; with REPORT-EVENT-TYPES also
                        // forward releases and distinguish repeats.
                        if !*pressed && !report_event_types {
                            continue;
                        }
                        let kind = if !*pressed {
                            KeyEventKind::Release
                        } else if *repeat {
                            KeyEventKind::Repeat
                        } else {
                            KeyEventKind::Press
                        };
                        let m = KeyModifiers {
                            ctrl: modifiers.ctrl,
                            alt: modifiers.alt,
                            shift: modifiers.shift,
                            logo: modifiers.command || modifiers.mac_cmd,
                        };
                        if let Some(lk) = egui_key_to_logical(*key, m) {
                            keys.push((lk, m, kind));
                        }
                    }
                    _ => {}
                }
            }
        });

        // Apply the buffered IME pre-edit change now the input borrow is closed
        // (F3-1). `None` means no IME event this frame — leave the pre-edit as-is
        // so a composition spanning multiple frames is not dropped.
        if let Some(new_preedit) = ime_update {
            self.ime_preedit = new_preedit;
        }

        // Tab/arrows must reach the PTY, not drive egui focus — consume them so
        // egui's built-in navigation does not also act on them.
        ctx.input_mut(|i| {
            for key in [
                egui::Key::Tab,
                egui::Key::ArrowUp,
                egui::Key::ArrowDown,
                egui::Key::ArrowLeft,
                egui::Key::ArrowRight,
            ] {
                while i.consume_key(egui::Modifiers::NONE, key) {}
            }
        });

        let mut forwarded: Vec<u8> = Vec::new();
        if let Some(term) = self.terms.get_mut(&self.focused_pane) {
            for (lk, m, kind) in &keys {
                // The common press path goes through the stable `forward_key`
                // wrapper; repeats/releases (kitty REPORT-EVENT-TYPES) take the
                // full event form.
                forwarded.extend(if *kind == KeyEventKind::Press {
                    term.forward_key(lk, *m)
                } else {
                    term.forward_key_event(lk, *m, *kind)
                });
            }
        }

        // Paste handling — SECURITY: every paste goes through the core paste-
        // injection guard (`PaneTerm::write_paste` → `Terminal::frame_paste`),
        // NEVER raw `write_bytes`. A multi-line paste can execute the instant its
        // embedded newline lands, so when `paste_warn_multiline` is on we DEFER a
        // multi-line paste to a confirm overlay (`pending_paste`) instead of
        // pasting immediately. The config read / `pending_paste` set / `terms`
        // borrow are sequential statements so they never alias `self`.
        for s in &pastes {
            if self.config.paste_warn_multiline && (s.contains('\n') || s.contains('\r')) {
                self.pending_paste = Some(s.clone());
            } else if let Some(term) = self.terms.get_mut(&self.focused_pane) {
                term.write_paste(s);
            }
        }

        // Best-effort capture of the line being typed, for the command-palette
        // history (see `c0pl4nd_core::command_history`). Printable text accrues,
        // Backspace pops one char, and Enter commits the line then clears the
        // accumulator. This models printable input + Backspace, NOT full shell
        // line-editing (cursor motion, kill-line) — exactly the contract the
        // `command_history` module documents. Only runs when typing reaches the
        // PTY (the palette routes its own keys away from here), so the history is
        // a record of what the user actually ran, not what they searched for.
        // Ordinary printable characters (incl. Space) arrive as `LogicalKey::Text`
        // (egui delivers them via `Event::Text`); only the special keys below are
        // `LogicalKey` variants, so this captures the full typed line.
        for (lk, _m, kind) in &keys {
            // Releases never accrue typed-line content (a released Enter must not
            // re-commit the line). Presses and repeats do.
            if *kind == KeyEventKind::Release {
                continue;
            }
            match lk {
                LogicalKey::Text(t) => {
                    // Ctrl-letter chords arrive here as a single C0 control byte
                    // (Ctrl+C = 0x03, Ctrl+U = 0x15, …), NOT printable line
                    // content. Ctrl+C / Ctrl+U abort the current line in a shell,
                    // so mirror that by clearing the accumulator; other control
                    // bytes are ignored. Printable text (incl. Space) accrues.
                    if t.chars().all(|c| !c.is_control()) {
                        self.input_line.push_str(t);
                    } else if t == "\u{3}" || t == "\u{15}" {
                        self.input_line.clear();
                    }
                }
                LogicalKey::Backspace => {
                    self.input_line.pop();
                }
                LogicalKey::Enter => {
                    let line = std::mem::take(&mut self.input_line);
                    if self.should_record_history(&line) {
                        // `record` redacts inline secrets (--password=…, API_KEY=…).
                        self.cmd_history.record(line);
                    }
                }
                _ => {}
            }
        }
        forwarded
    }

    /// Render the egui_tiles grid (live terminal panes) + enforce the 6-pane cap
    /// (clone-and-snap-back). The terminal bodies are painted by the FREE
    /// [`Self::render_pane_body`] so the closure can borrow `self.terms`/`theme`
    /// disjointly from `self.grid_tree` (which `tree.ui` borrows mutably).
    fn grid_ui(&mut self, ui: &mut egui::Ui) {
        let titles = self.pane_titles();
        let mut closes: Vec<PaneId> = Vec::new();
        let focused = self.focused_pane;
        let mut clicked: Option<PaneId> = None;
        let mut focused_size: Option<(f32, f32)> = None;
        let mut opened_url: Option<String> = None;
        // The focused pane's IME cursor rect, captured from the render closure
        // and fed into `ctx.output_mut(|o| o.ime = ...)` AFTER the disjoint-
        // borrow block so the OS candidate window tracks the caret (F3-1).
        let mut ime_cursor_rect: Option<egui::Rect> = None;

        // The find overlay highlights the FOCUSED pane only, and only while open.
        // Build the cell spans HERE (before the disjoint-borrow block takes
        // `&mut self.terms`), since `cell_spans_for_search` reads `self` via
        // `focused_grid_text`. Owned `Vec` + a copied index, so the render
        // closure borrows them disjointly from `self.grid_tree`.
        let search_spans: Vec<CellSpan> = if self.search_open {
            self.cell_spans_for_search()
        } else {
            Vec::new()
        };
        let search_sel = self.search_sel;

        // Ctrl-clickable hyperlinks: only built while the modifier (Ctrl, or Cmd
        // on macOS) is held, so URLs underline ON Ctrl-hover and a Ctrl-click
        // opens one — a plain click stays a normal pane interaction. Built HERE
        // (before the disjoint-borrow block) for the same reason as the search
        // spans; `find_urls` reads the focused grid via `focused_search_lines`.
        let link_modifier = ui.input(|i| i.modifiers.ctrl || i.modifiers.command);
        let link_spans: Vec<(CellSpan, String)> = if link_modifier {
            self.cell_spans_for_hyperlinks()
        } else {
            Vec::new()
        };

        // The active pane shell layout (#30), read LIVE so the titlebar toggle
        // takes effect this frame. Captured before the disjoint-borrow block
        // (which takes `&mut self.terms`).
        let view_mode = self.config.view_mode;

        // Snapshot BEFORE the frame so we can revert a drag that exceeds the cap.
        let pre = self.grid_tree.clone();
        {
            // Disjoint borrows: the closure touches these fields, NOT grid_tree.
            let terms = &mut self.terms;
            // Deferred first-spawn set (bug #40): disjoint field borrow, passed
            // through so `render_pane_body` can spawn a pending pane at the
            // MEASURED `(cols, rows)` on the first frame its rect is known.
            let pending_spawn = &mut self.pending_spawn;
            // The per-row galley cache is a separate field, so it borrows
            // disjointly from `terms` AND from `grid_tree` (audit #2).
            let galley_cache = &mut self.galley_cache;
            let theme = &self.theme;
            // The configured TERM, read alongside the other LIVE config reads so a
            // deferred-first-spawn pane advertises the same `TERM` as later panes.
            let term = self.config.term.as_str();
            let font_size = self.config.font.size;
            // Read the line-height LIVE from the config so a Settings change
            // reflows the row pitch (and the PTY rows/cursor/highlight) without a
            // relaunch. Folded into the row pitch by [`effective_row_pitch`].
            let line_height_px = self.config.font.line_height;
            let cursor_cfg = self.config.cursor;
            // CRT scanlines + chromatic aberration, read LIVE so toggling them in
            // Settings takes effect this frame; both are zero-cost when off/zero.
            let effects = self.config.effects;
            // Pane background alpha: full when opaque, opacity-folded when the
            // window is effectively translucent — painting the pane fill
            // non-opaque is what lets the OS blur / desktop show through (the
            // transparency fix). Read LIVE so the opacity slider applies without
            // a relaunch.
            let bg_alpha = pane_bg_alpha(&self.config);
            // Read the inner padding LIVE from the config so a Settings change
            // moves the grid inset without a relaunch (it was a hardcoded 4px
            // before). `u16` config → f32 points for the painter.
            let padding = f32::from(self.config.window.padding);
            let search_spans = &search_spans;
            let link_spans = &link_spans;
            let empty_links: &[(CellSpan, String)] = &[];
            // The active IME pre-edit, borrowed for the focused pane only (F3-1).
            // A `&str` borrow of `self.ime_preedit` is disjoint from the field
            // borrows above and from `grid_tree`, so it joins the closure cleanly.
            let ime_preedit = self.ime_preedit.as_deref();
            let ime_rect_out = &mut ime_cursor_rect;
            let mut render_body = |ui: &mut egui::Ui, pid: PaneId| -> bool {
                let search = if pid == focused && !search_spans.is_empty() {
                    Some(SearchHighlight {
                        spans: search_spans,
                        selected: search_sel,
                    })
                } else {
                    None
                };
                // Hyperlinks are interactive on the FOCUSED pane only (the others
                // get an empty slice → no underline, no hit test).
                let links: &[(CellSpan, String)] = if pid == focused {
                    link_spans
                } else {
                    empty_links
                };
                let outcome = Self::render_pane_body(
                    ui,
                    pid,
                    pid == focused,
                    terms,
                    pending_spawn,
                    galley_cache,
                    theme,
                    term,
                    font_size,
                    line_height_px,
                    cursor_cfg,
                    effects,
                    padding,
                    bg_alpha,
                    search,
                    links,
                    if pid == focused { ime_preedit } else { None },
                );
                if outcome.clicked {
                    clicked = Some(pid);
                }
                if let Some(url) = outcome.opened_url {
                    opened_url = Some(url);
                }
                if pid == focused {
                    focused_size = Some((outcome.size.x, outcome.size.y));
                    // The focused pane's caret rect drives IME candidate-window
                    // placement (set on the context after this block closes).
                    *ime_rect_out = outcome.ime_cursor_rect;
                }
                outcome.drag_started
            };
            // Pane shell layout (#30):
            // - Grid: drive the egui_tiles tree → every pane visible.
            // - Tabs: render ONLY the focused pane, full-size, in the content
            //   area. The tab strip (in the titlebar) stays the pane switcher;
            //   the multi-pane egui_tiles layout is skipped entirely this frame.
            //   The grid tree is NOT mutated, so flipping back to Grid restores
            //   the exact prior layout.
            if view_mode == c0pl4nd_core::config::ViewMode::Tabs {
                // The focused pane must exist in the tree; if it somehow does not
                // (defensive — focus is always re-anchored to a live pane), fall
                // back to the first pane so the content area is never blank.
                let show = if grid::tile_of_pane(&pre, focused).is_some() {
                    focused
                } else {
                    titles.first().map(|(id, _)| *id).unwrap_or(focused)
                };
                render_body(ui, show);
            } else {
                let mut behavior = GridBehavior {
                    titles: &titles,
                    render_body: &mut render_body,
                    close_requests: &mut closes,
                };
                self.grid_tree.ui(&mut behavior, ui);
            }
        }
        // Prune galley-cache rows not painted this frame (rows that scrolled off,
        // a pane switched away from in Tabs view, or a closed pane) so the cache
        // tracks only the live grid and cannot grow without bound (audit #2).
        self.galley_cache.prune_unseen();
        if let Some(s) = focused_size {
            self.last_focused_size = Some(s);
        }
        // Tell the OS where the IME candidate window should appear (F3-1): the
        // focused pane's terminal-cursor cell. Without this, `output.ime` stays
        // `None` (the grid is a custom-painted region, not an egui `TextEdit`,
        // so egui never sets it for us) and the candidate window anchors at the
        // screen origin or fails to appear. Setting `rect` (the cell) and
        // `cursor_rect` (the caret) drives winit's `set_ime_cursor_area`.
        if let Some(cursor_rect) = ime_cursor_rect {
            ui.ctx().output_mut(|o| {
                o.ime = Some(egui::output::IMEOutput {
                    rect: cursor_rect,
                    cursor_rect,
                });
            });
        }
        // Record a Ctrl-clicked URL (the browser open already fired in-render);
        // most-recent-wins, observable for the interaction test.
        if let Some(url) = opened_url {
            self.last_opened_url = Some(url);
        }

        // Enforce the cap: a drag-to-split that pushed us over 6 reverts.
        if count_panes(&self.grid_tree) > grid::MAX_PANES {
            self.grid_tree = pre;
            self.toast = Some(format!("max {} panes", grid::MAX_PANES));
        }

        if let Some(pid) = clicked {
            if pid != self.focused_pane {
                self.input_line.clear(); // the typed-line accumulator is per-pane
            }
            self.focused_pane = pid;
        }

        // Apply close requests; keep at least one pane alive. Drop the closed
        // pane's terminal (PTY + reader thread) so it does not leak.
        for pid in closes {
            self.close_pane(pid);
        }
    }

    /// Close one pane: remove its tile + terminal (PTY + reader thread), drop its
    /// pinned state, and re-anchor focus if the focused pane was the one closed.
    /// Keeps at least one pane alive — the last pane is never closed. Shared by
    /// the egui_tiles close button (via `grid_ui`) and the tab-bar × (via
    /// `frame_tick`).
    fn close_pane(&mut self, pid: PaneId) {
        if count_panes(&self.grid_tree) <= 1 {
            return;
        }
        let Some(tile) = grid::tile_of_pane(&self.grid_tree, pid) else {
            return;
        };
        self.grid_tree.tiles.remove(tile);
        self.grid_tree.simplify_children_of_tile(
            self.grid_tree.root.unwrap_or(tile),
            &egui_tiles::SimplificationOptions::default(),
        );
        self.terms.remove(&pid);
        self.pinned.remove(&pid);
        // Re-anchor focus if the focused pane was closed.
        if grid::tile_of_pane(&self.grid_tree, self.focused_pane).is_none() {
            if let Some((p, _)) = self.pane_titles().first() {
                self.focused_pane = *p;
            }
        }
    }

    /// The settings window (Milestone 2): a grouped, well-spaced, searchable
    /// two-pane window matching the sibling SCR1B3 editor's layout. Delegates the
    /// whole UI to the [`settings`] module (a free function so it never fights
    /// `self`'s borrow), then live-applies any change: persist the config to
    /// disk, reload the terminal color theme when the theme stem changed (so the
    /// live panes repaint, not just the chrome), and re-apply the egui Visuals.
    fn settings_window(&mut self, ctx: &egui::Context) {
        let mut open = self.settings_open;
        // Theme-derived palette so the settings window fill + headings follow
        // the active theme along with the rest of the chrome.
        let colors = theme::ChromeColors::from_theme(&self.theme);
        let outcome = settings::show(ctx, &mut self.config, &mut open, colors, self.incognito);
        self.settings_open = open;

        // Privacy-section actions (runtime, not config): handle before the
        // config-changed persistence below.
        if outcome.clear_history {
            self.clear_command_history();
        }
        if let Some(on) = outcome.set_incognito {
            self.set_incognito(on);
        }

        if outcome.changed {
            // Reload the terminal grid's color theme so a theme change shows in
            // the live PTY panes immediately (the chrome Visuals are re-applied
            // below; the grid glyph colours come from this `Theme`, not Visuals).
            if outcome.theme_changed {
                self.theme = load_terminal_theme(&self.config);
                // Propagate to the LIVE panes: each PaneTerm holds its own theme
                // clone (glyph + background colours resolve from it), so without
                // this the picker would change `self.theme` but no visible pane.
                for term in self.terms.values_mut() {
                    term.set_theme(self.theme.clone());
                }
            }
            // Re-apply the chrome Visuals DERIVED FROM the (possibly changed)
            // terminal theme so the WHOLE app UI — titlebar, tabs, status bar,
            // settings window, panel fills — follows the picked theme (a light
            // theme flips the chrome light, a dark one dark) without waiting for
            // a relaunch. `self.theme` was reloaded just above on a theme change.
            ctx.set_visuals(theme::visuals_from_theme(&self.theme));
            // Persist to the platform config file so the change survives a
            // relaunch — but ONLY in a real window. The headless `egui_kittest`
            // harness sets `live_window == false`; persisting there would write
            // the user's real `%APPDATA%\c0pl4nd\config.toml` from a test run
            // (test pollution). The live in-memory apply above is what the tests
            // observe; the disk write is a real-window-only side effect.
            // Best-effort: a write failure (e.g. read-only config dir) never
            // blocks the live in-memory apply.
            if self.live_window {
                if let Some(path) = c0pl4nd_core::Config::default_path() {
                    // Surface a persist failure (read-only %APPDATA%, full disk,
                    // permission error) instead of silently dropping the user's
                    // settings change — mirrors the legacy shell (window.rs).
                    if let Err(e) = self.config.save_to(&path) {
                        tracing::warn!("could not save config: {e}");
                    }
                }
            }
        }
    }

    // ---- settings observation surface (production accessors, NOT test-only) ----
    //
    // Real accessors the `egui_kittest` settings tests use to assert observable
    // Config/theme changes after driving the REAL `settings::show` through
    // `frame_tick`. Deliberately not `#[cfg(test)]` so the test exercises the
    // exact production path (the same observation-accessor discipline the other
    // public accessors above follow).

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

    /// Whether a multi-line paste is currently awaiting confirmation. (Test /
    /// observation API for the paste-safety overlay.)
    #[allow(dead_code)]
    pub fn has_pending_paste(&self) -> bool {
        self.pending_paste.is_some()
    }

    /// Send the deferred multi-line paste to the focused pane through the core
    /// paste-injection guard, then clear it. Returns the text that was sent (for
    /// tests; `None` if nothing was pending). The OS side effect aside, this is
    /// the same path a non-deferred paste takes.
    pub fn confirm_pending_paste(&mut self) -> Option<String> {
        let text = self.pending_paste.take()?;
        if let Some(term) = self.terms.get_mut(&self.focused_pane) {
            term.write_paste(&text);
        }
        Some(text)
    }

    /// Discard the deferred multi-line paste without sending it.
    pub fn cancel_pending_paste(&mut self) {
        self.pending_paste = None;
    }

    /// Whether a just-typed line should be recorded in command history. PRIVACY:
    /// the history feeds the palette + sidebar and must never capture secrets the
    /// user never meant to store. Two guards:
    ///
    /// 1. **Leading-space opt-out** (the HISTCONTROL=ignorespace convention): a
    ///    line the user prefixed with a space/tab is intentionally excluded.
    /// 2. **Password-prompt suppression** (the load-bearing one): a password
    ///    typed at `sudo` / `ssh` / `mysql -p` is NOT echoed by the tty, so its
    ///    characters never reach the grid. We record a line only if a short
    ///    prefix of it was ECHOED into the focused pane's visible text. A
    ///    non-echoed line (zero echo = password) is dropped. The prefix (the
    ///    earliest-typed chars, which have had the most time to round-trip
    ///    through the PTY) tolerates trailing-echo lag while still catching a
    ///    fully-unechoed secret. Privacy-conservative: when in doubt, drop —
    ///    losing a history entry is acceptable; storing a password is not.
    ///
    /// Inline secrets that ARE echoed (`--password=…`, `API_KEY=…`) are redacted
    /// downstream by [`c0pl4nd_core::command_history::redact_secrets`].
    fn should_record_history(&self, line: &str) -> bool {
        // Privacy controls: capture disabled in settings, or an incognito session.
        if !self.config.history_capture_enabled || self.incognito {
            return false;
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            return false;
        }
        if line.starts_with(' ') || line.starts_with('\t') {
            return false;
        }
        const PROBE_LEN: usize = 4;
        let probe: String = trimmed.chars().take(PROBE_LEN).collect();
        self.focused_grid_text()
            .is_some_and(|grid| grid.contains(&probe))
    }

    /// The multi-line-paste confirm overlay: a small centred modal showing how
    /// many lines the paste is and a preview, with Send / Cancel. Defends against
    /// the "paste a multi-line command that runs on the embedded newline" footgun
    /// — the paste does not reach the PTY until the user confirms. Enter = send,
    /// Esc = cancel (also handled here so the modal is keyboard-drivable).
    fn paste_confirm_window(&mut self, ctx: &egui::Context) {
        let Some(text) = self.pending_paste.clone() else {
            return;
        };
        // Keyboard: Esc cancels, Enter (or Ctrl+Enter) sends.
        let (send, cancel) = ctx.input(|i| {
            (
                i.key_pressed(egui::Key::Enter),
                i.key_pressed(egui::Key::Escape),
            )
        });
        if cancel {
            self.cancel_pending_paste();
            return;
        }
        if send {
            self.confirm_pending_paste();
            return;
        }

        let line_count = text.lines().count().max(1);
        // A short, control-stripped preview so the modal itself can't be used to
        // smuggle escape sequences into the chrome.
        let preview: String = text
            .chars()
            .filter(|c| !c.is_control() || *c == '\n')
            .take(400)
            .collect();

        let mut do_send = false;
        let mut do_cancel = false;
        egui::Window::new("Paste multiple lines?")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .show(ctx, |ui| {
                ui.label(format!(
                    "This paste contains {line_count} lines and may run commands as soon as it lands."
                ));
                ui.add_space(6.0);
                egui::ScrollArea::vertical().max_height(160.0).show(ui, |ui| {
                    ui.add(
                        egui::Label::new(egui::RichText::new(&preview).monospace())
                            .wrap(),
                    );
                });
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("Send paste (Enter)").clicked() {
                        do_send = true;
                    }
                    if ui.button("Cancel (Esc)").clicked() {
                        do_cancel = true;
                    }
                });
            });
        if do_send {
            self.confirm_pending_paste();
        } else if do_cancel {
            self.cancel_pending_paste();
        }
    }

    /// The current inner window padding (points) from the live config — the
    /// value `grid_ui` threads into the grid paint each frame, so it reflects
    /// what the terminal grid is actually inset by. Observation accessor for the
    /// padding live-apply interaction test.
    #[allow(dead_code)]
    pub fn config_window_padding(&self) -> u16 {
        self.config.window.padding
    }

    /// The grid text origin a focused pane WOULD draw at for a given body
    /// `rect`, using the LIVE config padding — exercising the exact
    /// [`grid_text_origin`] helper the production paint path uses. Lets the
    /// interaction test prove a Padding change moves the rendered origin (not
    /// just the stored config value), with no GPU. Pure read of live state.
    #[allow(dead_code)]
    pub fn grid_text_origin_for(&self, rect: egui::Rect) -> egui::Pos2 {
        grid_text_origin(rect, f32::from(self.config.window.padding))
    }

    // ---- command palette (quick find/run previously-run commands) ----
    //
    // The palette surfaces `cmd_history` (commands the user typed + ran in any
    // pane this session) and lets them fuzzy-search and re-run one with Enter.
    // It is opened with Ctrl+Shift+P (handled in `frame_tick`). These methods are
    // the production logic the frame loop calls — the interaction tests drive
    // them through the real frame loop, NOT as a test-only mirror.

    /// Toggle the command palette. Opening it resets the query, selection, and
    /// the in-flight typed-line accumulator (so a half-typed line is not later
    /// recorded as if it had been run after the palette closes).
    fn toggle_palette(&mut self) {
        self.palette_open = !self.palette_open;
        if self.palette_open {
            self.palette_query.clear();
            self.palette_sel = 0;
            self.input_line.clear();
        }
    }

    /// The palette's filtered results for the current query — every history entry
    /// (most-recent-first) when the query is empty, fuzzy-filtered otherwise.
    fn palette_results(&self) -> Vec<String> {
        self.cmd_history.search(&self.palette_query)
    }

    /// Move the palette selection by `delta` rows, clamped to the result range.
    /// A no-op when there are no results.
    fn palette_move(&mut self, delta: i64) {
        let n = self.palette_results().len();
        if n == 0 {
            self.palette_sel = 0;
            return;
        }
        let max = n as i64 - 1;
        let cur = self.palette_sel as i64;
        self.palette_sel = (cur + delta).clamp(0, max) as usize;
    }

    /// Run the currently-selected history entry in the focused pane: write it to
    /// the PTY followed by a carriage return (what the shell sees for Enter),
    /// move it to the front of the history, and close the palette. Returns the
    /// command run (for tests). Closes the palette with no command when the
    /// result set is empty.
    fn run_palette_selection(&mut self) -> Option<String> {
        let cmd = self.palette_results().get(self.palette_sel).cloned();
        if let Some(ref c) = cmd {
            self.run_command_in_focused(c);
        }
        self.last_palette_run = cmd.clone();
        self.palette_open = false;
        cmd
    }

    /// Write `cmd` followed by a carriage return (what the shell sees for Enter)
    /// to the focused pane's PTY, and move `cmd` to the front of the history (no
    /// duplicate). The single run path shared by the command palette
    /// ([`Self::run_palette_selection`]) and the history sidebar
    /// ([`Self::run_history_command`]) so both surfaces re-run a command
    /// identically.
    fn run_command_in_focused(&mut self, cmd: &str) {
        if let Some(term) = self.terms.get_mut(&self.focused_pane) {
            term.write_bytes(cmd.as_bytes());
            term.write_bytes(b"\r");
        }
        // Re-running moves the command to the front (no duplicate).
        self.cmd_history.record(cmd.to_string());
    }

    /// Open a native file picker (#35) and, on a pick, RUN the chosen script in
    /// the focused pane by feeding its PATH to the shell as a command — the
    /// shell then executes it via its own shebang/interpreter dispatch. Reading
    /// and injecting the file's lines instead would bypass the shebang, mangle
    /// multi-line scripts, and flood the shell's line history. The path is
    /// quoted for the active shell ([`quote_path_for_shell`]: PowerShell's call
    /// operator `& "…"`, else a `'…'`/`"…"`-quoted path). The blocking
    /// `pick_file()` is fine here — it is called from the post-panel action
    /// block (every panel has already closed) and the OS dialog runs its own
    /// modal loop, so no egui borrow is held and no animation is in flight.
    fn open_script_file(&mut self) {
        let picked = rfd::FileDialog::new()
            .set_title("Run a script in the focused terminal")
            .add_filter(
                "Scripts",
                &["sh", "ps1", "bat", "cmd", "py", "js", "rb", "fish", "zsh"],
            )
            .add_filter("All files", &["*"])
            .pick_file();
        if let Some(path) = picked {
            let quoted = quote_path_for_shell(&path, self.active_shell_label());
            self.run_command_in_focused(&quoted);
        }
    }

    // ---- command-history quick-run sidebar (#21) -------------------------
    //
    // A toggleable docked `egui::SidePanel` (side from `config.history_sidebar_
    // side`) listing the command history newest-first with a filter box. Clicking
    // a row re-runs it in the focused pane via the SAME `run_command_in_focused`
    // path the command palette uses. Opened/closed with Ctrl+Shift+H (handled in
    // `frame_tick`, with the chord filtered out of the PTY input stream).

    /// Toggle the command-history sidebar. Opening it clears the stale filter so
    /// the full history shows first.
    fn toggle_history_sidebar(&mut self) {
        self.history_open = !self.history_open;
        if self.history_open {
            self.history_filter.clear();
        }
    }

    /// Run `cmd` from the history sidebar: the same focused-pane run + history
    /// re-order path the palette uses, recorded in `last_palette_run` so an
    /// interaction test can assert the click ran the real command (reusing the
    /// palette's observable). Closes the sidebar after a run.
    fn run_history_command(&mut self, cmd: &str) {
        self.run_command_in_focused(cmd);
        self.last_palette_run = Some(cmd.to_string());
        self.history_open = false;
    }

    /// Whether the command-history sidebar is currently open. Observation
    /// accessor for the toggle interaction test.
    #[allow(dead_code)]
    pub fn history_sidebar_open(&self) -> bool {
        self.history_open
    }

    /// Which side the history sidebar docks to (from the live config).
    /// Observation accessor for the side-preference test.
    #[allow(dead_code)]
    pub fn history_sidebar_side(&self) -> c0pl4nd_core::config::PanelSide {
        self.config.history_sidebar_side
    }

    /// The history rows the sidebar would show for the current filter — every
    /// entry (most-recent-first) when the filter is empty, fuzzy-filtered
    /// otherwise. Pure read shared by the render and the click test.
    fn history_sidebar_rows(&self) -> Vec<String> {
        let f = self.history_filter.trim();
        if f.is_empty() {
            self.cmd_history.entries().map(str::to_string).collect()
        } else {
            self.cmd_history.search(f)
        }
    }

    /// Render the command-history quick-run sidebar as a docked, resizable
    /// `egui::SidePanel` on the configured side. Only called when `history_open`
    /// — a closed sidebar is NOT `.show`n, so the central terminal reflows to the
    /// full width (the "true popout" behaviour). A filter box sits at the top;
    /// below it the history is listed newest-first as clickable rows (failed
    /// vs ok styling is out of scope — the history holds commands, not exit
    /// codes). Clicking a row runs it via [`Self::run_history_command`].
    // egui 0.34 deprecated the top-level `SidePanel::show(ctx, …)` form in favour
    // of `show_inside(ui, …)`, but this frameless app shows its panels straight
    // from the `ctx` in `frame_tick` (there is no parent `&mut Ui` at this level
    // — same rationale as the titlebar/status TopBottomPanels). Allow it here as
    // those panels do.
    #[allow(deprecated)]
    fn history_sidebar(&mut self, ctx: &egui::Context, colors: theme::ChromeColors) {
        let rows = self.history_sidebar_rows();
        let history_empty = self.cmd_history.is_empty();
        let mut clicked: Option<String> = None;
        let mut close_requested = false;

        let mut body = |ui: &mut egui::Ui, filter: &mut String| {
            ui.horizontal(|ui| {
                ui.heading(egui::RichText::new("History").color(colors.fg));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .button(egui::RichText::new(egui_phosphor::thin::X).size(14.0))
                        .on_hover_text("Close history (Ctrl+Shift+H)")
                        .clicked()
                    {
                        close_requested = true;
                    }
                });
            });
            ui.add(
                egui::TextEdit::singleline(filter)
                    .hint_text("filter…")
                    .desired_width(f32::INFINITY),
            );
            ui.separator();
            if rows.is_empty() {
                ui.weak(if history_empty {
                    "No commands run yet."
                } else {
                    "No matches."
                });
            } else {
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        for cmd in &rows {
                            // A full-width clickable row in the chosen font; click
                            // re-runs it in the focused pane.
                            let resp = ui.add(
                                egui::Label::new(
                                    egui::RichText::new(cmd)
                                        .color(colors.fg)
                                        .family(egui::FontFamily::Monospace),
                                )
                                .sense(egui::Sense::click())
                                .wrap(),
                            );
                            if resp.clicked() {
                                clicked = Some(cmd.clone());
                            }
                            resp.on_hover_text("Run in the focused pane");
                        }
                    });
            }
            ui.add_space(4.0);
            ui.weak("Ctrl+Shift+H toggles · click a row to run");
        };

        let frame = egui::Frame::new().fill(colors.panel).inner_margin(8.0);
        // Snapshot the filter into a local so the panel closure's `&mut filter`
        // (the TextEdit) does not collide with the immutable `rows`/`self` reads.
        let mut filter = std::mem::take(&mut self.history_filter);
        match self.config.history_sidebar_side {
            c0pl4nd_core::config::PanelSide::Left => {
                egui::SidePanel::left("c0pl4nd_history")
                    .resizable(true)
                    .default_width(260.0)
                    .frame(frame)
                    .show(ctx, |ui| body(ui, &mut filter));
            }
            c0pl4nd_core::config::PanelSide::Right => {
                egui::SidePanel::right("c0pl4nd_history")
                    .resizable(true)
                    .default_width(260.0)
                    .frame(frame)
                    .show(ctx, |ui| body(ui, &mut filter));
            }
        }
        self.history_filter = filter;

        if close_requested {
            self.history_open = false;
        }
        if let Some(cmd) = clicked {
            self.run_history_command(&cmd);
        }
    }

    /// Run the history entry at `index` (newest-first) exactly as a real click
    /// does — through [`Self::run_history_command`]. `pub` for the
    /// `#[path]`-included interaction test (which seeds the history then drives a
    /// row "click"); inert in the shipping binary, which runs rows via real
    /// pointer clicks. Returns the command run, or `None` when `index` is out of
    /// range.
    #[allow(dead_code)]
    pub fn test_run_history_row(&mut self, index: usize) -> Option<String> {
        let cmd = self.history_sidebar_rows().get(index).cloned()?;
        self.run_history_command(&cmd);
        Some(cmd)
    }

    /// Whether the command palette is currently open. Observation accessor for
    /// the interaction tests (asserts Ctrl+Shift+P toggled it through the real
    /// frame loop).
    #[allow(dead_code)]
    pub fn palette_open(&self) -> bool {
        self.palette_open
    }

    /// Whether the app is in frameless terminal-only fullscreen (#36).
    /// Observation accessor for the interaction test (asserts an F11 press
    /// toggled it through the real frame loop, hiding the chrome panels).
    #[allow(dead_code)]
    pub fn fullscreen(&self) -> bool {
        self.fullscreen
    }

    /// The recorded command history, most-recent-first. Observation accessor for
    /// the interaction tests (asserts typed-then-Enter lines were captured).
    #[allow(dead_code)]
    pub fn command_history_entries(&self) -> Vec<String> {
        self.cmd_history.entries().map(str::to_string).collect()
    }

    /// Clear all recorded command history now (the buffers are zeroized). Wired
    /// to the Privacy settings "Clear command history" button.
    pub fn clear_command_history(&mut self) {
        self.cmd_history.clear();
    }

    /// Whether this session is in incognito mode (no command-history capture).
    #[allow(dead_code)]
    pub fn is_incognito(&self) -> bool {
        self.incognito
    }

    /// Toggle incognito (no-history) for this session. Runtime-only; never
    /// persisted. Entering incognito also clears any already-recorded history so
    /// the switch is a clean break.
    pub fn set_incognito(&mut self, on: bool) {
        self.incognito = on;
        if on {
            self.cmd_history.clear();
        }
    }

    /// The command most recently run from the palette, if any. Observation
    /// accessor for the interaction test (asserts Enter on a selection ran the
    /// real command through the real frame loop).
    #[allow(dead_code)]
    pub fn last_palette_run(&self) -> Option<String> {
        self.last_palette_run.clone()
    }

    /// The most recent URL a Ctrl-click opened, or `None`. Observable accessor
    /// for the hyperlink interaction test.
    #[allow(dead_code)]
    pub fn last_opened_url(&self) -> Option<String> {
        self.last_opened_url.clone()
    }

    /// Activate the URL (if any) at grid cell `(row, col)` exactly as a real
    /// Ctrl-click does — record it in [`Self::last_opened_url`] and return it.
    /// This shares the SAME span-build + hit-test path the renderer uses
    /// ([`Self::cell_spans_for_hyperlinks`] + [`link_url_at_cell`]); only the
    /// pixel→cell mapping (unit-tested separately via [`cell_at_pos`]) and the
    /// `ctx.open_url` OS side effect are omitted, neither of which is observable
    /// in the headless harness. `pub` for the `#[path]`-included test binary;
    /// inert in the shipping binary (which never calls it).
    #[allow(dead_code)]
    pub fn test_open_url_at_cell(&mut self, row: usize, col: usize) -> Option<String> {
        let links = self.cell_spans_for_hyperlinks();
        let url = link_url_at_cell(&links, row, col)?.to_string();
        self.last_opened_url = Some(url.clone());
        Some(url)
    }

    /// `(check_on_launch, channel)` from the loaded config — read by the binary
    /// entry point to decide whether to spawn the opt-in launch update check and
    /// which release channel to query. Kept here so the check lives outside the
    /// `egui_app` module (whose update *logic* dependency the test binaries do
    /// not carry) — the entry point owns the network call.
    #[allow(dead_code)]
    pub fn update_check_config(&self) -> (bool, String) {
        (
            self.config.update.check_on_launch,
            self.config.update.channel.clone(),
        )
    }

    /// Attach the receiver for a background launch update check. The entry point
    /// spawns the check (the only network surface) and hands the app the channel;
    /// [`Self::frame_tick`] polls it and surfaces a found update as a toast.
    #[allow(dead_code)]
    pub fn attach_update_check(&mut self, rx: std::sync::mpsc::Receiver<String>) {
        self.update_rx = Some(rx);
    }

    /// Surface an update notice: show it as a transient toast and record it
    /// (most-recent-wins) for the interaction test. Shared by the launch-check
    /// poll and the test, so both exercise one path.
    fn apply_update_notice(&mut self, notice: String) {
        self.toast = Some(notice.clone());
        self.last_update_notice = Some(notice);
    }

    /// The most recent update notice surfaced, or `None`. Observable accessor for
    /// the launch-check interaction test.
    #[allow(dead_code)]
    pub fn last_update_notice(&self) -> Option<String> {
        self.last_update_notice.clone()
    }

    /// Poll the launch-check channel (if attached) and surface a received notice
    /// as a toast. Non-blocking; the background thread sends at most one notice.
    fn poll_update_check(&mut self) {
        if let Some(rx) = &self.update_rx {
            if let Ok(notice) = rx.try_recv() {
                self.apply_update_notice(notice);
                self.update_rx = None; // one-shot: stop polling after the notice
            }
        }
    }

    /// Render the command-palette overlay: a centred window with an auto-focused
    /// fuzzy-search box over the command history and a selectable result list.
    /// Clicking a row runs it (same path as Enter). Navigation (↑/↓/Enter/Esc)
    /// is handled in [`Self::frame_tick`] before this renders, so the list here
    /// only needs to display the current query + selection and report a click.
    ///
    /// Immutable state is snapshotted into locals before the window closure so
    /// the closure's `&mut palette_query` (for the `TextEdit`) does not collide
    /// with reads of `palette_sel` / `cmd_history` on the same `self`.
    fn command_palette_window(&mut self, ctx: &egui::Context) {
        let results = self.palette_results();
        // Clamp selection if the result set shrank since the last frame.
        if self.palette_sel >= results.len() {
            self.palette_sel = results.len().saturating_sub(1);
        }
        let sel = self.palette_sel;
        let history_empty = self.cmd_history.is_empty();
        let query = &mut self.palette_query;
        let mut clicked: Option<usize> = None;

        egui::Window::new("Command palette")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 80.0))
            .default_width(540.0)
            .show(ctx, |ui| {
                let resp = ui.add(
                    egui::TextEdit::singleline(query)
                        .hint_text("Search previously-run commands…")
                        .desired_width(f32::INFINITY),
                );
                // Keep the search box focused for the palette's whole lifetime so
                // typed characters always populate the query, never the PTY.
                resp.request_focus();
                ui.separator();
                if results.is_empty() {
                    ui.weak(if history_empty {
                        "No commands run yet — run something, then reopen with Ctrl+Shift+P."
                    } else {
                        "No matches."
                    });
                } else {
                    egui::ScrollArea::vertical()
                        .max_height(280.0)
                        .auto_shrink([false, true])
                        .show(ui, |ui| {
                            for (i, cmd) in results.iter().enumerate() {
                                if ui.selectable_label(i == sel, cmd).clicked() {
                                    clicked = Some(i);
                                }
                            }
                        });
                }
                ui.separator();
                ui.weak("↑/↓ select · Enter run · Esc close");
            });

        if let Some(i) = clicked {
            self.palette_sel = i;
            self.run_palette_selection();
        }
    }

    // ---- in-terminal find overlay (Ctrl+F) -------------------------------
    //
    // The overlay searches the FOCUSED pane's visible/scrollback grid text via
    // the shared core matcher (`c0pl4nd_core::search::find`). It is opened with
    // Ctrl+F (handled in `frame_tick`), filters as you type, shows a live match
    // count, cycles matches with Enter / F3 / Shift+F3, and closes with Esc. The
    // Regex + Case toggles map onto `search::SearchOptions`. These methods are the
    // production logic `frame_tick` calls — the interaction tests drive them
    // THROUGH the real frame loop, not as a test-only mirror.

    /// The lines of the focused pane's grid text, one `String` per visual row —
    /// the slice handed to [`c0pl4nd_core::search::find`]. Empty when the focused
    /// pane has no live terminal (the matcher then yields no matches).
    fn focused_search_lines(&self) -> Vec<String> {
        // A seeded test corpus (headless find tests) takes precedence so the
        // search wiring can be asserted without a live PTY; otherwise the real
        // focused-pane grid text is searched.
        if let Some(corpus) = &self.search_test_corpus {
            return corpus.lines().map(str::to_string).collect();
        }
        self.focused_grid_text()
            .map(|t| t.lines().map(str::to_string).collect())
            .unwrap_or_default()
    }

    /// The current [`SearchOptions`](c0pl4nd_core::search::SearchOptions) derived
    /// from the two UI toggles. `case_sensitive` is the inverse of the core's
    /// `case_insensitive` field.
    fn search_options(&self) -> c0pl4nd_core::search::SearchOptions {
        c0pl4nd_core::search::SearchOptions {
            regex: self.search_regex,
            case_insensitive: !self.search_case_sensitive,
        }
    }

    /// Recompute `search_matches` for the current query + options over the
    /// focused pane's grid text, and clamp `search_sel` into range. Called on
    /// open, on every query/toggle change, and each frame the overlay is open
    /// (the live PTY grid scrolls, so the match set is not static). An invalid
    /// regex yields an empty set — surfaced calmly as "no matches", never a
    /// panic (the core matcher swallows the regex-compile error).
    fn recompute_search(&mut self) {
        let lines = self.focused_search_lines();
        self.search_matches =
            c0pl4nd_core::search::find(&lines, &self.search_query, self.search_options());
        if self.search_matches.is_empty() {
            self.search_sel = 0;
        } else if self.search_sel >= self.search_matches.len() {
            self.search_sel = self.search_matches.len() - 1;
        }
    }

    /// Toggle the find overlay. Opening it resets the selection to the first
    /// match and recomputes the match set for whatever is already on screen;
    /// closing it leaves the query intact (so reopening resumes the last search).
    fn toggle_search(&mut self) {
        self.search_open = !self.search_open;
        if self.search_open {
            self.search_sel = 0;
            self.recompute_search();
        }
    }

    /// Advance the selected match by `delta` (wrapping), a no-op when there are
    /// no matches. Enter / F3 step +1; Shift+F3 steps −1. Wrapping mirrors every
    /// real editor's find-next behaviour (the last match's "next" is the first).
    fn search_cycle(&mut self, delta: i64) {
        let n = self.search_matches.len();
        if n == 0 {
            self.search_sel = 0;
            return;
        }
        let n_i = n as i64;
        let cur = self.search_sel as i64;
        self.search_sel = (((cur + delta) % n_i + n_i) % n_i) as usize;
    }

    /// Whether the find overlay is currently open. Observation accessor for the
    /// interaction tests (asserts Ctrl+F toggled it through the real frame loop).
    #[allow(dead_code)]
    pub fn search_is_open(&self) -> bool {
        self.search_open
    }

    /// The number of matches the find overlay found this frame. Observation
    /// accessor for the interaction tests (asserts typing filters the grid).
    #[allow(dead_code)]
    pub fn search_match_count(&self) -> usize {
        self.search_matches.len()
    }

    /// The 0-based index of the currently-selected match. Observation accessor
    /// for the cycle tests (asserts F3 / Shift+F3 / Enter move the selection).
    #[allow(dead_code)]
    pub fn search_selected(&self) -> usize {
        self.search_sel
    }

    /// Whether the find query is currently treated as a regex. Observation
    /// accessor for the regex-toggle test.
    #[allow(dead_code)]
    pub fn search_regex_enabled(&self) -> bool {
        self.search_regex
    }

    /// Whether find matching is currently case-SENSITIVE. Observation accessor
    /// for the case-toggle test.
    #[allow(dead_code)]
    pub fn search_case_sensitive_enabled(&self) -> bool {
        self.search_case_sensitive
    }

    // ---- find-overlay test-support surface --------------------------------
    //
    // These let the headless interaction tests drive the find overlay against a
    // KNOWN corpus and flip the option toggles deterministically — the live PTY
    // grid is async + platform-dependent, so a CI box with no usable shell could
    // not otherwise exercise the matcher. They are `pub` (consumed by the
    // `#[path]`-included test binary) but operate ONLY on the test-corpus
    // override / option flags; they never touch the live PTY, so they are inert
    // in the shipping binary (which never calls them).

    /// Seed the find overlay's search corpus with a known multi-line string, so
    /// the headless tests can assert the matcher wiring without a live PTY. The
    /// shipping binary never calls this (`search_test_corpus` stays `None`).
    /// Recomputes the match set immediately if the overlay is already open.
    #[allow(dead_code)]
    pub fn test_seed_focused_grid(&mut self, corpus: &str) {
        self.search_test_corpus = Some(corpus.to_string());
        if self.search_open {
            self.recompute_search();
        }
    }

    /// Flip the regex option and recompute matches — the production effect of
    /// clicking the Regex toggle, exposed for the headless test (which cannot
    /// reliably click the overlay's flow buttons).
    #[allow(dead_code)]
    pub fn test_set_regex(&mut self, on: bool) {
        self.search_regex = on;
        if self.search_open {
            self.recompute_search();
        }
    }

    /// Flip the case-sensitivity option and recompute matches — the production
    /// effect of clicking the Case toggle, exposed for the headless test.
    #[allow(dead_code)]
    pub fn test_set_case_sensitive(&mut self, on: bool) {
        self.search_case_sensitive = on;
        if self.search_open {
            self.recompute_search();
        }
    }

    /// The current match set converted to CELL spans over the focused pane's
    /// grid text, ready for the highlight painter. Converts each match's BYTE
    /// span to character columns via [`byte_to_col`] against the matched line, so
    /// a multi-byte glyph before the match never offsets the highlight. A match
    /// whose `line` exceeds the visible rows (the grid scrolled since the set was
    /// computed) is dropped.
    fn cell_spans_for_search(&self) -> Vec<CellSpan> {
        if self.search_matches.is_empty() {
            return Vec::new();
        }
        let lines = self.focused_search_lines();
        self.search_matches
            .iter()
            .filter_map(|m| {
                let line = lines.get(m.line)?;
                Some(CellSpan {
                    line: m.line,
                    col_start: byte_to_col(line, m.start),
                    col_end: byte_to_col(line, m.end),
                })
            })
            .collect()
    }

    /// Every `http(s)://` URL in the FOCUSED pane's grid text, as `(CellSpan,
    /// url)` pairs ready for the Ctrl-hover underline and the Ctrl-click hit
    /// test. Built from [`Self::focused_search_lines`] (which honours the test
    /// corpus) via [`hyperlink::find_urls`], converting each URL's BYTE span to
    /// character columns with [`byte_to_col`] so a multi-byte glyph before the
    /// URL never offsets the underline. Computed once per frame before the
    /// disjoint-borrow render block (mirrors [`Self::cell_spans_for_search`]).
    fn cell_spans_for_hyperlinks(&self) -> Vec<(CellSpan, String)> {
        let lines = self.focused_search_lines();
        let mut out = Vec::new();
        for (row, line) in lines.iter().enumerate() {
            for span in hyperlink::find_urls(line) {
                out.push((
                    CellSpan {
                        line: row,
                        col_start: byte_to_col(line, span.start),
                        col_end: byte_to_col(line, span.end),
                    },
                    span.url,
                ));
            }
        }
        out
    }

    /// Render the find overlay (delegating to the [`search_ui`] free function so
    /// it never fights `self`'s borrow), then recompute the match set when the
    /// query or a toggle changed this frame. The `current` readout is the 1-based
    /// selection index when on a match, else 0.
    fn search_window(&mut self, ctx: &egui::Context) {
        let colors = theme::ChromeColors::from_theme(&self.theme);
        let match_count = self.search_matches.len();
        let current = if match_count == 0 {
            0
        } else {
            self.search_sel + 1
        };
        let outcome = {
            let state = search_ui::SearchState {
                query: &mut self.search_query,
                regex: &mut self.search_regex,
                case_sensitive: &mut self.search_case_sensitive,
            };
            search_ui::show(ctx, state, match_count, current, colors)
        };
        if outcome.changed {
            self.recompute_search();
        }
    }
}

impl eframe::App for C0pl4ndApp {
    /// Frameless window clear color.
    ///
    /// When the window is effectively translucent (master toggle on + a
    /// translucent mode) we clear to fully transparent so the rounded corners
    /// and the OS blur (acrylic / mica / vibrancy) — or, for `Transparent`
    /// mode and on Linux, the desktop itself — show through; `window_clear_color`
    /// folds the `opacity` slider into the alpha for the portable see-through
    /// look. When opaque, we clear to the theme background at full alpha so the
    /// desktop never bleeds through a solid window.
    fn clear_color(&self, _v: &egui::Visuals) -> [f32; 4] {
        window_clear_color(&self.config, &self.theme)
    }

    /// Do NOT persist egui `Memory` to disk (privacy F1).
    ///
    /// eframe's default `App::persist_egui_memory()` is `true`, which serializes
    /// the entire egui [`egui::Memory`] — including every widget's
    /// `TextEditState` and its `Undoer<(CCursorRange, String)>` undo stack — into
    /// `app.ron` under the `with_app_id` storage folder
    /// (`%APPDATA%\com.itashacorp.c0pl4nd\data\app.ron` on Windows;
    /// `~/.local/share/com.itashacorp.c0pl4nd/app.ron` on Linux). The undo stack
    /// stores the ACTUAL typed text, so fragments of the find overlay, the command
    /// palette, and the settings search — all of which are substrings of the
    /// user's scrollback — would land on disk in plaintext RON. Returning `false`
    /// keeps that typed-text undo history entirely in memory.
    ///
    /// Window geometry (position + size) is NOT lost by this: it is persisted
    /// independently via [`c0pl4nd_core::Config::persist_geometry`] into the
    /// config TOML AND by eframe's own `persist_window` native-window state, both
    /// of which are unaffected by `persist_egui_memory`.
    fn persist_egui_memory(&self) -> bool {
        false
    }

    /// eframe 0.34's `App` main entry is `ui(&mut self, &mut Ui, &mut Frame)`;
    /// the top-level panels are driven through the (deprecated-but-functional)
    /// `Panel::show(ctx, …)` path via a cloned `ctx`, matching the reference
    /// egui app. The work lives in [`frame_tick`](Self::frame_tick) so the
    /// headless tests can drive it without an `eframe::Frame`.
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.frame_tick(&ctx);
        // The grid is painted with egui's native text painter inside
        // `render_pane_body` during `frame_tick`; there is no post-frame GPU pass.
    }
}

impl C0pl4ndApp {
    /// Run the necessary on-close cleanup, synchronously and fast, so the Close
    /// handler can immediately `std::process::exit(0)` afterwards instead of
    /// waiting on eframe/wgpu's slow graceful GPU-device + swapchain +
    /// winit-window-destroy teardown (the actual source of the slow-to-close
    /// latency — the PTY teardown is ~2ms and is not the bottleneck).
    ///
    /// Two side effects, in order:
    ///
    /// 1. **Persist config** — the same best-effort `config.save_to(default_path)`
    ///    write the settings-change handler performs, gated on `live_window` so a
    ///    headless test never writes the user's real `%APPDATA%\c0pl4nd\config.toml`
    ///    (test pollution). This is the save-on-close that MUST still happen before
    ///    a fast exit.
    /// 2. **Drop every live terminal** — `self.terms.clear()` drops each
    ///    [`PaneTerm`], and dropping a `PaneTerm` runs its `Session::Drop`, which
    ///    kills the PTY child. This is the no-orphan guarantee: after this call no
    ///    `cmd.exe` (or other shell) is left running. It is ~2ms for the canonical
    ///    six-pane layout and is done BEFORE the process exits so the children are
    ///    reaped, not orphaned, even though `process::exit` runs no destructors.
    ///
    /// Kept separate from the `process::exit(0)` call so tests can exercise the
    /// cleanup (save + child reaping) WITHOUT terminating the test runner.
    pub fn prepare_shutdown(&mut self) {
        // 1) Persist config — real-window-only, best-effort (a write failure must
        //    never wedge the close path). Mirrors the settings-handler save.
        if self.live_window {
            if let Some(path) = c0pl4nd_core::Config::default_path() {
                // Surface a persist failure instead of silently dropping the
                // user's settings change — mirrors the legacy shell (window.rs).
                if let Err(e) = self.config.save_to(&path) {
                    tracing::warn!("could not save config: {e}");
                }
            }
        }
        // 2) Drop every PaneTerm → each Session::Drop kills its PTY child. No
        //    orphaned shells survive the close.
        self.terms.clear();
    }

    /// One per-frame tick of the chrome + grid. Separated from `eframe::App::ui`
    /// so `egui_kittest` can drive it through a `Context` without a `Frame`.
    ///
    /// egui 0.34 deprecated the top-level `Panel::show(ctx, …)` form in favour
    /// of `show_inside(ui, …)`, but `show_inside` needs a parent `&mut Ui` that
    /// the top-level entry does not provide; `show(ctx)` remains the working
    /// top-level path (same compromise the reference app documents).
    #[allow(deprecated)]
    pub fn frame_tick(&mut self, ctx: &egui::Context) {
        // Ensure the chrome fonts (incl. the `phosphor-fill` family) are
        // installed before any widget references them — `new()` does this for
        // the real app; headless tests built via `bootstrap()` install here on
        // frame 1 (otherwise the pinned tab's `FontFamily::Name("phosphor-fill")`
        // would panic on an unregistered family).
        if !self.fonts_installed {
            install_chrome_fonts(ctx, &self.config.font);
            self.applied_font_family = font_apply_key(&self.config.font);
            self.fonts_installed = true;
        }
        // F2-3: apply the persisted UI scale (accessibility zoom) to the whole
        // egui context — ONLY when the configured value changed since last
        // applied, so it is a no-op on steady-state frames and never overrides
        // the transient Ctrl+/- keyboard zoom (which does not write
        // `config.ui_scale`). The NaN-initialised `applied_ui_scale` guarantees
        // the first frame applies; `set_zoom_factor` is itself a no-op when the
        // value is unchanged.
        let ui_scale = self.config.effective_ui_scale();
        if ui_scale != self.applied_ui_scale {
            ctx.set_zoom_factor(ui_scale);
            self.applied_ui_scale = ui_scale;
        }
        // Wire each live pane's UI-wake callback (once) so live PTY output wakes
        // the render loop — the other half of the damage-tracked-redraw scheme
        // whose idle side lives in `idle_repaint_interval`. Real window only:
        // a wake that calls `request_repaint` would make headless `Harness::run`
        // loop until `max_steps`.
        // Drain each pane's terminal-owed effects every frame (runs headless too,
        // so interaction tests can assert a query reply reached the PTY): PTY
        // query replies are written back to their own pane inside
        // `pump_host_effects`; the host-global effects (clipboard / live theme /
        // notification) are applied by `pump_pane_effects`.
        self.pump_pane_effects(ctx);
        if self.live_window {
            self.wire_pane_wakes(ctx);
        }
        // Live font apply: when the user changes the Family (or a Fallback) in
        // settings, the configured font stack changed since the last install —
        // re-install it THIS frame so the new typeface shows without a relaunch.
        // The `applied_font_family` key folds the family + fallbacks into one
        // string so the (expensive) re-install runs ONLY on an actual change,
        // never every frame. A re-install changes the font atlas, so the cached
        // galleys (which reference the old atlas) must be dropped (audit #2).
        else {
            let want = font_apply_key(&self.config.font);
            if want != self.applied_font_family {
                install_chrome_fonts(ctx, &self.config.font);
                self.applied_font_family = want;
                self.galley_cache.clear();
                // A settings re-install supersedes any in-flight startup load.
                self.pending_fonts = None;
            }
        }
        // Off-thread startup font load (audit #3): when the worker thread that
        // enumerated the system font DB has finished, swap in the custom stack.
        // Until then the window painted with the built-in mono. `try_recv` is
        // non-blocking so the frame never stalls; a disconnected channel (the
        // worker failed to spawn or panicked) just drops the pending state and
        // keeps the built-in mono.
        if let Some(rx) = &self.pending_fonts {
            match rx.try_recv() {
                Ok(defs) => {
                    ctx.set_fonts(defs);
                    self.galley_cache.clear();
                    self.pending_fonts = None;
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.pending_fonts = None;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
            }
        }
        // Surface an opt-in launch update check result (if one arrived) as a
        // toast. No-op when no check was attached (every headless test).
        self.poll_update_check();
        // 0a) command palette: Ctrl+Shift+P (Cmd+Shift+P on macOS) toggles it. The
        //     matching key-press is removed from the event stream so it never
        //     reaches the PTY — without this, on the close frame (palette already
        //     open) the `P` would fall through to `forward_input_to_focused` and
        //     be encoded as the Ctrl+P control byte. Done explicitly rather than
        //     via `consume_key` so the ctrl-OR-command match is unambiguous on
        //     every platform.
        let toggle_palette = ctx.input_mut(|i| {
            let mut found = false;
            i.events.retain(|ev| {
                let hit = matches!(
                    ev,
                    egui::Event::Key { key: egui::Key::P, pressed: true, modifiers, .. }
                    if modifiers.shift && (modifiers.ctrl || modifiers.command)
                );
                found |= hit;
                !hit
            });
            found
        });
        if toggle_palette {
            self.toggle_palette();
        }

        // 0a') find overlay: Ctrl+F (Cmd+F on macOS) toggles it. The matching
        //      key-press is removed from the event stream so it never reaches the
        //      PTY — without this, on the close frame (overlay already open) the
        //      `F` would fall through to `forward_input_to_focused` and be encoded
        //      as the Ctrl+F control byte (0x06, the shell's forward-char). The
        //      ctrl-OR-command match is done explicitly (not via `consume_key`) so
        //      it is unambiguous on every platform — the same discipline the
        //      palette chord uses above.
        let toggle_search = ctx.input_mut(|i| {
            let mut found = false;
            i.events.retain(|ev| {
                let hit = matches!(
                    ev,
                    egui::Event::Key { key: egui::Key::F, pressed: true, modifiers, .. }
                    if (modifiers.ctrl || modifiers.command) && !modifiers.shift && !modifiers.alt
                );
                found |= hit;
                !hit
            });
            found
        });
        if toggle_search {
            self.toggle_search();
        }

        // 0a'') history sidebar: Ctrl+Shift+H (Cmd+Shift+H on macOS) toggles the
        //       command-history quick-run sidebar. The matching key-press is
        //       removed from the event stream so it never reaches the PTY — the
        //       same chord-leak discipline the palette + find chords use above
        //       (without this, `H` would fall through to the PTY as the Ctrl+H
        //       control byte = backspace). Done explicitly (not `consume_key`) so
        //       the ctrl-OR-command match is unambiguous on every platform.
        let toggle_history = ctx.input_mut(|i| {
            let mut found = false;
            i.events.retain(|ev| {
                let hit = matches!(
                    ev,
                    egui::Event::Key { key: egui::Key::H, pressed: true, modifiers, .. }
                    if modifiers.shift && (modifiers.ctrl || modifiers.command)
                );
                found |= hit;
                !hit
            });
            found
        });
        if toggle_history {
            self.toggle_history_sidebar();
        }

        // 0a''') frameless fullscreen (#36): F11 toggles borderless OS fullscreen
        //        (the window is already `decorations: false`, so `Fullscreen` —
        //        not `Maximized` — is the right call; it covers the monitor with
        //        no border and keeps DWM compositing so the acrylic/mica backdrop
        //        still composites). The F11 key-press is removed from the event
        //        stream so it never reaches the PTY as the F11 escape sequence —
        //        the SAME chord-leak discipline the palette / find / history
        //        chords use above. Esc ALSO exits fullscreen, but ONLY when no
        //        overlay owns Esc (the palette + find consume Esc to close
        //        themselves; handling it here too would fight them), and is left
        //        in the stream otherwise so those overlays still see it.
        let toggle_fullscreen = ctx.input_mut(|i| {
            let mut found = false;
            i.events.retain(|ev| {
                let hit = matches!(
                    ev,
                    egui::Event::Key {
                        key: egui::Key::F11,
                        pressed: true,
                        ..
                    }
                );
                found |= hit;
                !hit
            });
            found
        });
        let esc_exit_fullscreen = self.fullscreen
            && !self.palette_open
            && !self.search_open
            && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape));
        if toggle_fullscreen || esc_exit_fullscreen {
            // F11 toggles; an Esc in fullscreen always EXITS. Read the OS-reported
            // state so a fullscreen entered via another path is honoured.
            let now = ctx.input(|i| i.viewport().fullscreen.unwrap_or(self.fullscreen));
            let want = if esc_exit_fullscreen { false } else { !now };
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(want));
            // Local mirror is the source of truth the panels read THIS frame —
            // `i.viewport().fullscreen` updates a frame late (the OS reports back
            // next frame), so a local mirror avoids a one-frame flash of the
            // titlebar on enter / of the bare grid on exit.
            self.fullscreen = want;
        } else {
            // Reconcile the mirror from the OS each frame so a fullscreen toggled
            // via another path (e.g. a window-manager shortcut) stays honest.
            if let Some(os) = ctx.input(|i| i.viewport().fullscreen) {
                self.fullscreen = os;
            }
        }

        // 0a'''') font zoom (E-parity): Ctrl/Cmd with +/=/-/0, or Ctrl/Cmd+wheel.
        //         Mutates config.font.size (the renderer reads it every frame),
        //         clamped to [6, 48] like the legacy shell. The chords are
        //         consumed so they never reach the PTY; the pane's local wheel
        //         scrollback skips a Ctrl-held wheel (see render_pane_body) so a
        //         Ctrl+wheel only zooms.
        {
            let mut dz = 0.0_f32;
            let mut reset = false;
            ctx.input_mut(|i| {
                i.events.retain(|ev| {
                    if let egui::Event::Key {
                        key,
                        pressed: true,
                        modifiers,
                        ..
                    } = ev
                    {
                        if modifiers.command && !modifiers.alt {
                            match key {
                                egui::Key::Plus | egui::Key::Equals => {
                                    dz += 1.0;
                                    return false;
                                }
                                egui::Key::Minus => {
                                    dz -= 1.0;
                                    return false;
                                }
                                egui::Key::Num0 => {
                                    reset = true;
                                    return false;
                                }
                                _ => {}
                            }
                        }
                    }
                    true
                });
            });
            let wheel = ctx.input(|i| {
                if i.modifiers.command {
                    i.smooth_scroll_delta.y
                } else {
                    0.0
                }
            });
            if wheel.abs() > f32::EPSILON {
                dz += (wheel / 40.0).clamp(-2.0, 2.0);
            }
            if reset {
                self.config.font.size = c0pl4nd_core::Config::default().font.size;
                ctx.request_repaint();
            } else if dz != 0.0 {
                self.config.font.size = (self.config.font.size + dz).clamp(6.0, 48.0);
                ctx.request_repaint();
            }
        }

        // 0a''''') drag-and-drop (E-parity): insert each dropped file's
        //          shell-quoted path at the focused prompt as TEXT — never
        //          executed (no trailing newline beyond a separating space),
        //          matching the legacy shell. Routed through write_paste (the
        //          pastejacking-safe path).
        let dropped: Vec<std::path::PathBuf> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .filter_map(|f| f.path.clone())
                .collect()
        });
        if !dropped.is_empty() {
            let label = self.active_shell_label().to_string();
            let text: String = dropped
                .iter()
                .map(|p| format!("{} ", quote_path_for_shell(p, &label)))
                .collect();
            if let Some(term) = self.terms.get_mut(&self.focused_pane) {
                term.write_paste(&text);
            }
        }

        // 0a'''''') jump-to-prompt (E-parity): Ctrl+Shift+PageUp/PageDown scrolls
        //           the scrollback to the previous/next OSC 133 prompt mark. The
        //           chord is consumed so PageUp/Down don't also reach the PTY.
        let jump = ctx.input_mut(|i| {
            let z = egui::Modifiers::COMMAND | egui::Modifiers::SHIFT;
            if i.consume_key(z, egui::Key::PageUp) {
                Some(false) // backward → older prompt
            } else if i.consume_key(z, egui::Key::PageDown) {
                Some(true) // forward → newer prompt
            } else {
                None
            }
        });
        if let Some(forward) = jump {
            if let Some(term) = self.terms.get_mut(&self.focused_pane) {
                if term.jump_to_prompt(forward) {
                    ctx.request_repaint();
                }
            }
        }

        // 0a''''''') DEC ?1004 focus reporting (E-parity): on a window focus-in/out
        //            EDGE, tell the focused pane's program (so vim/tmux see
        //            FocusGained/FocusLost). report_focus is a no-op unless the
        //            program armed ?1004; the reply is drained by pump_host_effects.
        let focused_now = ctx.input(|i| i.viewport().focused.unwrap_or(self.was_focused));
        if focused_now != self.was_focused {
            if let Some(term) = self.terms.get_mut(&self.focused_pane) {
                term.report_focus(focused_now);
            }
            self.was_focused = focused_now;
        }

        // 0b) route this frame's input. When the palette is open, its navigation
        //     keys (↑/↓/Enter/Esc) are consumed here and the typed query is
        //     captured by the palette's focused TextEdit — NOT forwarded to the
        //     PTY. Otherwise keyboard/paste goes to the FOCUSED pane's PTY BEFORE
        //     the panels, so the keystrokes reach the PTY whose grid this same
        //     frame then snapshots (the load-bearing "typing reaches the PTY and
        //     the grid updates" round-trip).
        if self.pending_paste.is_some() {
            // A multi-line paste is awaiting confirmation: the confirm overlay is
            // modal, so DO NOT forward this frame's keystrokes to the PTY (else
            // the Enter that confirms the paste would also send a bare newline to
            // the shell). The overlay itself reads Enter/Esc in `paste_confirm_window`.
        } else if self.palette_open {
            let (up, down, enter, esc) = ctx.input_mut(|i| {
                (
                    i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp),
                    i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown),
                    i.consume_key(egui::Modifiers::NONE, egui::Key::Enter),
                    i.consume_key(egui::Modifiers::NONE, egui::Key::Escape),
                )
            });
            if up {
                self.palette_move(-1);
            }
            if down {
                self.palette_move(1);
            }
            if esc {
                self.palette_open = false;
            }
            if enter {
                self.run_palette_selection();
            }
        } else if self.search_open {
            // The find overlay owns input while open: its TextEdit captures the
            // typed query (it auto-focuses each frame), and the navigation keys
            // (Enter / F3 / Shift+F3 cycle, Esc close) are consumed HERE so they
            // never reach the PTY. F3 is consumed in BOTH shift states so the
            // shell never sees the F3 escape sequence while finding. Typed text
            // is deliberately NOT forwarded to the PTY this branch — the overlay
            // is modal over keyboard input, like the palette.
            // Consume Shift+F3 BEFORE plain F3: `consume_key` matches the most
            // specific modifier set, and consuming the SHIFT variant first means
            // a Shift+F3 press cannot also satisfy the bare-F3 consume (which
            // would step forward instead of back).
            let (enter, esc, f3_shift, f3) = ctx.input_mut(|i| {
                (
                    i.consume_key(egui::Modifiers::NONE, egui::Key::Enter),
                    i.consume_key(egui::Modifiers::NONE, egui::Key::Escape),
                    i.consume_key(egui::Modifiers::SHIFT, egui::Key::F3),
                    i.consume_key(egui::Modifiers::NONE, egui::Key::F3),
                )
            });
            if esc {
                self.search_open = false;
            } else {
                // Enter and F3 step to the next match; Shift+F3 steps to the
                // previous. The match set is recomputed each frame below so a
                // scrolling PTY keeps the cycle honest.
                if enter || f3 {
                    self.search_cycle(1);
                }
                if f3_shift {
                    self.search_cycle(-1);
                }
                // The live grid scrolls under the overlay, so refresh the match
                // set every open frame (cheap: a substring/regex scan of the
                // visible rows) before the highlight pass reads it.
                self.recompute_search();
            }
        } else {
            self.forward_input_to_focused(ctx);
        }

        // 0c) Frameless window edge/corner RESIZE (#24). The decorations are off,
        //     so the OS gives no resize border — we synthesize one: hint the
        //     matching resize cursor over an edge band, and on a primary press
        //     there (when no widget wants the pointer) start an OS resize via
        //     ViewportCommand::BeginResize. Run early, BEFORE the panels, so an
        //     edge grab wins; the `!wants_pointer_input()` guard still lets a
        //     widget sitting at the very edge get its click. Skipped in
        //     fullscreen (#36): there is no window edge to resize, and the
        //     synthetic resize cursors at the screen edge would be visually wrong.
        if !self.fullscreen {
            handle_frameless_resize(ctx);
        }

        // Theme-derived chrome surface palette — the titlebar / tab strip /
        // status bar / central pane / settings window all follow the active
        // terminal theme through these (a light theme flips the whole chrome
        // light, a dark one dark). The wordmark keeps its fixed brand accent.
        let colors = theme::ChromeColors::from_theme(&self.theme);

        // 1) custom titlebar + tab strip. Fixed height so the drag region below
        //    is exactly the bar (not the whole remaining column), and so the
        //    caption-cluster geometry is stable. In fullscreen (#36) the titlebar
        //    + status panels are NOT rendered so only the grid fills the screen;
        //    `actions` falls back to the empty default for that frame (no chrome
        //    means no chrome actions). The floating overlays (settings / palette /
        //    find / history) can still open over the grid while fullscreen.
        let actions = if self.fullscreen {
            chrome::ChromeActions::default()
        } else {
            egui::TopBottomPanel::top("titlebar")
                .exact_height(40.0)
                .frame(egui::Frame::new().fill(colors.panel).inner_margin(6.0))
                .show(ctx, |ui| {
                    // Frameless-window move: dragging any EMPTY part of the
                    // titlebar moves the window; double-click toggles maximize.
                    // Added FIRST so it sits behind the tabs/buttons (egui gives
                    // later widgets the click), so only the empty bar area
                    // initiates a drag.
                    let bar = ui.interact(
                        ui.max_rect(),
                        egui::Id::new("c0pl4nd_titlebar_drag"),
                        egui::Sense::click_and_drag(),
                    );
                    if bar.drag_started_by(egui::PointerButton::Primary) {
                        ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
                    }
                    if bar.double_clicked() {
                        let is_max = ui.ctx().input(|i| i.viewport().maximized.unwrap_or(false));
                        ui.ctx()
                            .send_viewport_cmd(egui::ViewportCommand::Maximized(!is_max));
                    }
                    self.titlebar_and_tabs(ui, colors)
                })
                .inner
        };

        // 2) status bar (hidden in fullscreen — see the titlebar gate above).
        if !self.fullscreen {
            egui::TopBottomPanel::bottom("status")
                .frame(egui::Frame::new().fill(colors.panel).inner_margin(4.0))
                .show(ctx, |ui| self.status_bar(ui, colors));
        }

        // 2b) command-history quick-run sidebar (#21), if open. Rendered as a
        //     docked SidePanel BEFORE the CentralPanel so the terminal grid
        //     reflows around it (and reclaims the full width when it closes — the
        //     panel is simply NOT shown when `history_open == false`).
        if self.history_open {
            self.history_sidebar(ctx, colors);
        }

        // 3) the pane grid (egui_tiles) — LIVE terminal panes (Milestone 2). The
        //    central-panel fill carries the SAME opacity-folded alpha the pane
        //    quads use when the window is effectively translucent, so the gap
        //    between/around panes also lets the OS blur (or desktop) show through
        //    — an opaque central fill here would cover the transparent clear
        //    color before the pane quads ever painted. Fully opaque otherwise.
        let central_alpha = pane_bg_alpha(&self.config);
        let central_fill = egui::Color32::from_rgba_unmultiplied(
            colors.bg.r(),
            colors.bg.g(),
            colors.bg.b(),
            central_alpha,
        );
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(central_fill))
            .show(ctx, |ui| self.grid_ui(ui));

        // Apply chrome actions AFTER the panels close (no mid-borrow mutation).
        if let Some(pid) = actions.focus_tab {
            if pid != self.focused_pane {
                self.input_line.clear(); // the typed-line accumulator is per-pane
            }
            self.focused_pane = pid;
        }
        if let Some(pid) = actions.pin_tab {
            // Toggle pinned state.
            if !self.pinned.remove(&pid) {
                self.pinned.insert(pid);
            }
        }
        if let Some(pid) = actions.close_tab {
            self.close_pane(pid);
        }
        if actions.new_terminal {
            self.new_terminal();
        }
        if let Some(idx) = actions.open_shell {
            self.open_shell(idx);
        }
        if actions.toggle_settings {
            self.settings_open = !self.settings_open;
        }
        // Script menu (#35), applied AFTER the panels close so neither the
        // `&mut self` run path nor the BLOCKING native file picker fires
        // mid-panel-borrow. A history re-run goes first; the "Open…" picker
        // (which blocks on its own OS modal loop) runs last.
        if let Some(cmd) = actions.rerun_command {
            self.run_command_in_focused(&cmd);
        }
        if actions.open_script_file {
            self.open_script_file();
        }
        // View-mode toggle (#30): flip the pane shell layout (Grid ⇄ Tabs) and
        // persist it. The disk write is real-window-only (the headless harness
        // observes the in-memory flip; persisting there would pollute the user's
        // real config.toml — the same discipline `settings_window` follows).
        if actions.toggle_view_mode {
            self.config.view_mode = self.config.view_mode.toggled();
            if self.live_window {
                if let Some(path) = c0pl4nd_core::Config::default_path() {
                    // Surface a persist failure (read-only %APPDATA%, full disk,
                    // permission error) instead of silently dropping the user's
                    // settings change — mirrors the legacy shell (window.rs).
                    if let Err(e) = self.config.save_to(&path) {
                        tracing::warn!("could not save config: {e}");
                    }
                }
            }
        }
        // Caption command: issue the REAL OS viewport command AND record it so an
        // interaction test can assert the click had its effect.
        if let Some(cmd) = actions.window_cmd {
            self.last_window_cmd = Some(cmd);
            let is_max = ctx.input(|i| i.viewport().maximized.unwrap_or(false));
            match cmd {
                WindowCmd::Minimize => {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
                }
                WindowCmd::ToggleMaximize => {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!is_max));
                }
                WindowCmd::Close => {
                    // Fast clean shutdown: run the necessary cleanup (persist
                    // config + reap every PTY child so none orphan), then exit
                    // immediately. This skips eframe/wgpu's slow graceful
                    // GPU-device + swapchain + winit-window-destroy teardown —
                    // the OS reclaims the GPU/window handles instantly — which is
                    // what made the window slow to close. `prepare_shutdown` does
                    // the load-bearing work; `process::exit(0)` is safe under
                    // `#![forbid(unsafe_code)]`.
                    self.prepare_shutdown();
                    std::process::exit(0);
                }
            }
        }

        // 4) the (opaque) settings window, if open
        if self.settings_open {
            self.settings_window(ctx);
        }

        // 5) the command palette overlay, if open (rendered last so it floats
        //    above the chrome + grid; its nav keys were handled in step 0b).
        if self.palette_open {
            self.command_palette_window(ctx);
        }

        // 5b) the find overlay, if open (floats above the grid; its nav keys
        //     were handled in step 0b). Recomputes matches on a query/toggle edit
        //     inside the window closure.
        if self.search_open {
            self.search_window(ctx);
        }

        // 5c) the multi-line paste confirm overlay, if a paste is pending. Floats
        //     above everything; Enter sends it through the injection guard, Esc
        //     discards it. Rendered before the tint so the wash sits over it too.
        if self.pending_paste.is_some() {
            self.paste_confirm_window(ctx);
        }

        // 6) window color-tint overlay (a subtle full-window wash). Only painted
        //    when the window is effectively translucent AND the user dialled in
        //    a tint strength — mirrors SCR1B3. A solid (opaque) window never
        //    gets washed; the gate keeps the default experience untouched.
        if self.config.effective_translucent() && self.config.tint_strength > 0.0 {
            paint_tint_overlay(ctx, &self.config.tint, self.config.tint_strength);
        }

        // Live terminals: schedule the IDLE repaint fallback — but ONLY in the
        // real window (`live_window`). PTY output now wakes the UI instantly via
        // each pane's Session wake callback (wired in `wire_pane_wakes`), and
        // real input repaints natively, so we no longer free-run at the monitor
        // refresh rate: an idle terminal drops from 60–144 fps to ~1–2 fps,
        // cutting idle GPU/CPU. `request_repaint_after` only sets a ceiling on
        // staleness (egui repaints at the SOONEST of all requests). In the
        // headless `egui_kittest` harness an unconditional repaint would make
        // `Harness::run` loop until `max_steps`, so the pump stays off there.
        if self.live_window {
            ctx.request_repaint_after(self.idle_repaint_interval());
        }
    }

    /// Wire every live pane's UI-wake callback exactly once. The reader thread
    /// invokes it after each chunk of PTY output so [`Self::idle_repaint_interval`]
    /// can let the UI sleep when idle while still repainting the instant output
    /// arrives. Idempotent per pane (see [`PaneTerm::wire_wake`]); cheap to call
    /// every frame. Only invoked for the real window.
    fn wire_pane_wakes(&mut self, ctx: &egui::Context) {
        for pane in self.terms.values_mut() {
            pane.wire_wake(|| {
                let ctx = ctx.clone();
                std::sync::Arc::new(move || ctx.request_repaint())
            });
        }
    }

    /// Drain every live pane's terminal-owed effects once per frame.
    ///
    /// PTY query replies (device attributes, cursor-position reports, OSC color
    /// queries, focus reports) are written straight back to their originating
    /// pane inside [`PaneTerm::pump_host_effects`]. The host-global effects it
    /// returns are applied here:
    /// - OSC 52 clipboard writes → the OS clipboard (`ctx.copy_text`).
    /// - OSC 4/10/11/12/104 color sets → the live [`Self::theme`], re-pushed to
    ///   every pane so the new palette shows the same frame.
    /// - OSC 9/777 notifications → a taskbar attention request while unfocused.
    ///
    /// Without this the canonical egui binary silently dropped every reply AND
    /// let the unread queues grow unbounded; the legacy winit shell drained them
    /// each frame. Runs in the headless harness too so interaction tests can
    /// assert a query reply reached the PTY.
    fn pump_pane_effects(&mut self, ctx: &egui::Context) {
        let mut clipboard: Vec<String> = Vec::new();
        let mut colors: Vec<ColorSet> = Vec::new();
        let mut notified = false;
        for pane in self.terms.values_mut() {
            let fx = pane.pump_host_effects();
            clipboard.extend(fx.clipboard_writes);
            colors.extend(fx.color_sets);
            notified |= fx.notified;
        }
        // OSC 52 → OS clipboard (write only; reads stay default-off in core).
        for text in clipboard {
            ctx.copy_text(text);
        }
        // OSC 4/10/11/12/104 → live theme, then repaint so the new palette shows.
        if !colors.is_empty() {
            for set in colors {
                self.apply_color_set(set);
            }
            for term in self.terms.values_mut() {
                term.set_theme(self.theme.clone());
            }
            ctx.request_repaint();
        }
        // OSC 9/777 desktop notification while the window is unfocused → request
        // user attention (taskbar flash). The notification TEXT is never read
        // here (privacy: it can carry a 2FA code / secret URL — never log it).
        // `focused` is `None` before the first focus event; treat that as focused
        // so a notification at startup does not spuriously flash.
        if notified && !ctx.input(|i| i.viewport().focused.unwrap_or(true)) {
            ctx.send_viewport_cmd(egui::ViewportCommand::RequestUserAttention(
                egui::UserAttentionType::Informational,
            ));
        }
    }

    /// Apply one drained [`ColorSet`] (OSC 4/10/11/12/104) to the live theme.
    /// Mirrors the legacy winit shell's mapping exactly: dynamic fg/bg/cursor
    /// update the theme's three core colors; indexed entries 0-15 update the
    /// 16-slot ANSI palette; 256-cube entries (index ≥ 16) have no theme slot
    /// and are ignored rather than misplaced.
    fn apply_color_set(&mut self, set: ColorSet) {
        use c0pl4nd_core::term::DynamicColor;
        let hex = |(r, g, b): (u8, u8, u8)| format!("#{r:02x}{g:02x}{b:02x}");
        match set {
            ColorSet::Dynamic { which, rgb } => match which {
                DynamicColor::Foreground => self.theme.foreground = hex(rgb),
                DynamicColor::Background => self.theme.background = hex(rgb),
                DynamicColor::Cursor => self.theme.cursor = hex(rgb),
            },
            ColorSet::Indexed { index, rgb } => {
                let row = if index < 8 {
                    &mut self.theme.normal
                } else if index < 16 {
                    &mut self.theme.bright
                } else {
                    // 256-color cube entries aren't represented in the 16-slot
                    // theme; ignore rather than misplace them.
                    return;
                };
                let slot = match index % 8 {
                    0 => &mut row.black,
                    1 => &mut row.red,
                    2 => &mut row.green,
                    3 => &mut row.yellow,
                    4 => &mut row.blue,
                    5 => &mut row.magenta,
                    6 => &mut row.cyan,
                    _ => &mut row.white,
                };
                *slot = hex(rgb);
            }
        }
    }

    /// The longest the live UI may wait before an *unforced* repaint. PTY output
    /// and user input repaint immediately; this only bounds idle staleness so an
    /// otherwise-quiescent terminal stops redrawing at the monitor refresh rate.
    fn idle_repaint_interval(&self) -> std::time::Duration {
        use std::time::Duration;
        // The CRT scanline post-effect is a continuous animation — keep it smooth
        // by ticking every frame while it is enabled (the scanline painter also
        // self-requests a repaint, so this just matches that cadence). F2-2: under
        // reduced-motion the roll band is frozen and does not self-request, so do
        // NOT pump the animation here either — fall through to the idle cadence.
        if self.config.effects.crt_scanlines && !c0pl4nd_core::reduced_motion::reduced_motion() {
            return Duration::ZERO; // == request_repaint(): animate at display rate
        }
        // A blink-enabled cursor must keep blinking on an otherwise-idle screen;
        // tick at the blink half-period so the caret toggles. The cursor painter
        // reads wall-clock time and does NOT self-request, so without this tick a
        // fully-idle screen would freeze the blink.
        if self.config_cursor_blink() {
            return Duration::from_millis(CURSOR_BLINK_HALF_PERIOD_MS);
        }
        // Fully quiescent: a 1 s safety-net tick bounds worst-case staleness if
        // any animation path forgot to self-request, while still cutting the idle
        // repaint rate ~60–140×. Output and input always repaint immediately.
        Duration::from_secs(1)
    }
}

/// Cursor-blink half-period, in milliseconds (the on/off toggle interval). Used
/// to schedule the idle repaint tick so a blinking caret keeps animating on an
/// otherwise-quiescent screen. Matches the 530 ms cadence the cursor painter and
/// the legacy winit shell use.
const CURSOR_BLINK_HALF_PERIOD_MS: u64 = 530;

/// Width of the 4 edge resize zones, in logical px. Slim so they only intercept
/// pointer events right at the window border (#24).
const RESIZE_EDGE_PX: f32 = 8.0;
/// Side length of the 4 corner resize zones, in logical px. Slightly larger than
/// the edges so diagonal grabs are forgiving (#24).
const RESIZE_CORNER_PX: f32 = 12.0;

/// Which window-edge resize direction (if any) the pointer `p` is over, given
/// the window `rect` and the edge/corner band widths. Corners (within `corner`
/// of two sides) take priority over straight edges; the interior returns `None`.
/// Pure + unit-tested so the frameless-resize hit-testing can't silently regress
/// into eating clicks meant for the tabs / caption buttons / panes (#24).
fn resize_dir_at(
    p: egui::Pos2,
    rect: egui::Rect,
    edge: f32,
    corner: f32,
) -> Option<egui::ResizeDirection> {
    use egui::ResizeDirection as D;
    let (l, r, t, b) = (
        p.x - rect.left(),
        rect.right() - p.x,
        p.y - rect.top(),
        rect.bottom() - p.y,
    );
    // Outside the window → not a resize zone.
    if l < 0.0 || r < 0.0 || t < 0.0 || b < 0.0 {
        return None;
    }
    let (w, e, n, s) = (l <= edge, r <= edge, t <= edge, b <= edge);
    let (nw, ne, nn, ns) = (l <= corner, r <= corner, t <= corner, b <= corner);
    if (n && nw) || (w && nn) {
        Some(D::NorthWest)
    } else if (n && ne) || (e && nn) {
        Some(D::NorthEast)
    } else if (s && nw) || (w && ns) {
        Some(D::SouthWest)
    } else if (s && ne) || (e && ns) {
        Some(D::SouthEast)
    } else if n {
        Some(D::North)
    } else if s {
        Some(D::South)
    } else if w {
        Some(D::West)
    } else if e {
        Some(D::East)
    } else {
        None
    }
}

/// Frameless window edge-resize, the no-Area way (#24). Each frame: if the
/// pointer is over an edge band, hint the matching resize cursor; on a primary
/// press there — and only when egui isn't already using the pointer for a
/// widget — start an OS resize via `ViewportCommand::BeginResize`. No persistent
/// `Order::Foreground` Areas, so it never swallows clicks meant for the tabs /
/// caption buttons / panes, and it works on every resize, not just the first.
fn handle_frameless_resize(ctx: &egui::Context) {
    use egui::{CursorIcon as C, ResizeDirection as D, ViewportCommand};
    let Some(p) = ctx.pointer_latest_pos() else {
        return;
    };
    // Hit-test against the FULL window surface (viewport_rect), NOT content_rect:
    // egui 0.34 split the old `screen_rect` into `viewport_rect()` (the whole
    // inner window) and `content_rect()` (the area inside the panels). We need
    // the whole window — content_rect excludes the top titlebar / bottom status
    // panels, which would push the resize bands inward off the real window edges
    // so the user couldn't grab them. `viewport_rect()` is the non-deprecated
    // successor for the whole-window surface the SCR1B3 reference used.
    let Some(dir) = resize_dir_at(p, ctx.viewport_rect(), RESIZE_EDGE_PX, RESIZE_CORNER_PX) else {
        return;
    };
    ctx.set_cursor_icon(match dir {
        D::North => C::ResizeNorth,
        D::South => C::ResizeSouth,
        D::West => C::ResizeWest,
        D::East => C::ResizeEast,
        D::NorthWest => C::ResizeNorthWest,
        D::NorthEast => C::ResizeNorthEast,
        D::SouthWest => C::ResizeSouthWest,
        D::SouthEast => C::ResizeSouthEast,
    });
    // Start the OS resize only if egui isn't consuming the press for a widget
    // (so a button / tab sitting at the very edge still gets its click).
    // `egui_wants_pointer_input` is the non-deprecated rename of
    // `wants_pointer_input` in egui 0.34.
    if ctx.input(|i| i.pointer.primary_pressed()) && !ctx.egui_wants_pointer_input() {
        ctx.send_viewport_cmd(ViewportCommand::BeginResize(dir));
        // The OS now owns the drag. winit's modal resize loop swallows the
        // button-up, so egui can be left believing a drag is still in progress —
        // which makes `wants_pointer_input()` return true forever and blocks
        // EVERY subsequent resize (the "works once, then never" bug). Clearing
        // egui's drag bookkeeping here unsticks that state so resize re-arms.
        ctx.stop_dragging();
    }
    // Belt-and-suspenders: with no button held there can be no legitimate drag,
    // so proactively clear any phantom drag the OS resize loop may have orphaned.
    if !ctx.input(|i| i.pointer.any_down()) {
        ctx.stop_dragging();
    }
}

/// Cell metrics (physical px) derived from egui's monospace font at `font_size`
/// — the same font [`paint_grid_native`] draws the grid with — so the PTY's
/// `(cols, rows)` match the rendered glyph size. Width is the advance of `'M'`;
/// height is the EFFECTIVE row pitch ([`effective_row_pitch`] of the font's
/// natural row height and the configured `line_height_px`), so a Line-height
/// change reflows the PTY to the SAME pitch the glyph painter draws at — rows
/// never overlap or leave a gap the resize math is unaware of. Both are scaled
/// to physical pixels by the context's `pixels_per_point`.
fn monospace_cell_metrics(
    painter: &egui::Painter,
    font_size: f32,
    ppp: f32,
    line_height_px: f32,
) -> CellMetrics {
    let probe = egui::text::LayoutJob::single_section(
        "M".to_string(),
        egui::text::TextFormat {
            font_id: egui::FontId::monospace(font_size.max(6.0)),
            ..Default::default()
        },
    );
    let size = painter.layout_job(probe).size();
    let pitch = effective_row_pitch(size.y, line_height_px);
    CellMetrics {
        advance_w: (size.x * ppp).max(1.0),
        line_h: (pitch * ppp).max(1.0),
    }
}

/// Outcome of painting one terminal pane's body for a frame.
struct PaneBodyOutcome {
    /// Whether the pane reported it wants to begin an egui_tiles drag.
    drag_started: bool,
    /// True when the pane body was clicked (a refocus request).
    clicked: bool,
    /// The pane's body size (points) this frame — used to pick the "+" split
    /// direction for the focused pane.
    size: egui::Vec2,
    /// A URL the user Ctrl-clicked in this pane's grid this frame, if any. The
    /// caller records it in [`C0pl4ndApp::last_opened_url`]; the OS-opener call
    /// (`ctx.open_url`) already happened inside the render.
    opened_url: Option<String>,
    /// The screen-space rect of the terminal cursor cell for this pane, in
    /// points (F3-1). `Some` only for the FOCUSED pane that has a live cursor;
    /// the caller feeds it into `ctx.output_mut(|o| o.ime = Some(IMEOutput {..}))`
    /// so the OS IME candidate window tracks the caret instead of anchoring at
    /// the screen origin.
    ime_cursor_rect: Option<egui::Rect>,
}

/// One find-overlay match converted to CELL coordinates: the visual row and the
/// `[col_start, col_end)` character columns the match spans. Built in
/// [`C0pl4ndApp::cell_spans_for_search`] from the byte spans the core matcher
/// returns, so the painter never re-derives columns from bytes.
#[derive(Clone, Copy)]
struct CellSpan {
    /// Visual row (line index into the pane's grid text).
    line: usize,
    /// First character column of the match (inclusive).
    col_start: usize,
    /// One-past-the-last character column of the match (exclusive).
    col_end: usize,
}

/// The find-overlay highlight inputs for ONE pane render: the cell spans to tint
/// plus the index of the active (selected) span. Borrowed from a per-frame
/// `Vec<CellSpan>` for the focused pane only while the overlay is open.
#[derive(Clone, Copy)]
struct SearchHighlight<'a> {
    /// Every match span in CELL coordinates over the pane's grid text.
    spans: &'a [CellSpan],
    /// Index into `spans` of the currently-selected match (the one Enter / F3
    /// cycles to); drawn with an outline so it stands out from the dim tints.
    selected: usize,
}

/// The byte offset `byte` within `line` converted to a CHARACTER column (the
/// terminal grid is monospace, so one char == one cell). The core matcher
/// returns BYTE spans (`str::find` / `Regex::find` offsets); a multi-byte glyph
/// before the match would otherwise over-count the column if bytes were used
/// directly. Clamps to the line length so a stale span (the grid scrolled since
/// the match was computed) can never index past the row.
fn byte_to_col(line: &str, byte: usize) -> usize {
    let b = byte.min(line.len());
    // Count chars up to the byte boundary; `char_indices` walks char starts.
    line.char_indices().take_while(|(i, _)| *i < b).count()
}

/// Map a pointer position (POINTS, in screen space) to the `(row, col)` grid
/// cell under it, given the grid text `origin` (top-left of the first cell) and
/// the cell size `(cw, ch)` in points. Returns `None` when the position is above
/// or left of the grid (a negative cell index). Pure so the Ctrl-click hit test
/// is unit-testable without an egui frame. Out-of-range high indices are NOT
/// clamped here — the caller's span list simply won't contain a matching span.
fn cell_at_pos(pos: egui::Pos2, origin: egui::Pos2, cw: f32, ch: f32) -> Option<(usize, usize)> {
    if pos.x < origin.x || pos.y < origin.y || cw <= 0.0 || ch <= 0.0 {
        return None;
    }
    let col = ((pos.x - origin.x) / cw).floor() as usize;
    let row = ((pos.y - origin.y) / ch).floor() as usize;
    Some((row, col))
}

/// Strip characters that are dangerous to render in app chrome from `s`,
/// returning a cleaned copy. A program (or a remote SSH host) controls the OSC
/// 0/2 terminal title and any OSC-8 / detected hyperlink URI; rendering those
/// strings verbatim in a tab label or link preview is a spoofing surface
/// (bidi-override "evil.com<U+202E>gpj.exe", zero-width obfuscation, embedded
/// control codes). This is a WHITELIST: we keep ordinary printable text — including
/// non-ASCII printable glyphs (accented Latin, CJK, emoji) — and drop only the
/// dangerous set:
///
/// - C0 controls `U+0000..=U+001F` and `U+007F`, and C1 controls
///   `U+0080..=U+009F`. For a one-line chrome label there is no legitimate
///   `\t`/`\n`/`\r`, so all control chars (including those) are removed.
/// - Bidirectional formatting: the embeddings/overrides `U+202A..=U+202E`
///   (LRE/RLE/PDF/LRO/RLO), the isolates `U+2066..=U+2069`
///   (LRI/RLI/FSI/PDI), and the marks `U+200E`/`U+200F` (LRM/RLM).
/// - Zero-width: `U+200B..=U+200D` (ZWSP/ZWNJ/ZWJ) and `U+FEFF` (ZWNBSP / BOM).
///
/// `pub(crate)` so any future chrome path that shows attacker-controlled text
/// (e.g. an OSC-8 hyperlink-URI preview) can reuse the exact same filter.
pub(crate) fn scrub_display_text(s: &str) -> String {
    s.chars()
        .filter(|&c| {
            // Drop all control characters (C0 + DEL + C1). `char::is_control`
            // covers U+0000..=U+001F, U+007F, and U+0080..=U+009F.
            if c.is_control() {
                return false;
            }
            !matches!(
                c,
                // Bidi embeddings / overrides + isolates + marks.
                '\u{202A}'..='\u{202E}'
                    | '\u{2066}'..='\u{2069}'
                    | '\u{200E}'
                    | '\u{200F}'
                    // Zero-width joiners/non-joiners/space + BOM/ZWNBSP.
                    | '\u{200B}'..='\u{200D}'
                    | '\u{FEFF}'
            )
        })
        .collect()
}

/// The URL whose cell span covers grid cell `(row, col)`, or `None`. Scans the
/// precomputed `(CellSpan, url)` links (built by
/// [`C0pl4ndApp::cell_spans_for_hyperlinks`]); the column test is half-open
/// `[col_start, col_end)`, matching how the spans were built.
fn link_url_at_cell(links: &[(CellSpan, String)], row: usize, col: usize) -> Option<&str> {
    links
        .iter()
        .find(|(s, _)| s.line == row && col >= s.col_start && col < s.col_end)
        .map(|(_, url)| url.as_str())
}

/// Cell `(width, height)` in POINTS for the terminal grid: the width is the
/// monospace `M` advance; the height is the EFFECTIVE row pitch
/// ([`effective_row_pitch`] of the natural galley height and the configured
/// `line_height_px`) — the SAME pitch `paint_grid_native` draws rows at, so
/// hyperlink underlines, the Ctrl-click hit test, and the search highlight all
/// land exactly on the rendered glyph grid regardless of the Line-height
/// setting.
fn monospace_cell_points(
    painter: &egui::Painter,
    font_size: f32,
    line_height_px: f32,
) -> (f32, f32) {
    let size = painter
        .layout_job(egui::text::LayoutJob::single_section(
            "M".to_string(),
            egui::text::TextFormat {
                font_id: egui::FontId::monospace(font_size),
                ..Default::default()
            },
        ))
        .size();
    (size.x.max(1.0), effective_row_pitch(size.y, line_height_px))
}

/// Underline every Ctrl-clickable URL span over the rendered grid (drawn only
/// while the modifier is held — see the caller). A thin accent line under each
/// span's cells signals "this is a link"; GPU-free (one `line_segment` per span).
/// The painter's clip rect keeps an over-wide span inside the pane.
fn paint_link_underlines(
    painter: &egui::Painter,
    origin: egui::Pos2,
    cw: f32,
    ch: f32,
    colors: &theme::ChromeColors,
    links: &[(CellSpan, String)],
) {
    for (s, _) in links {
        let col_end = s.col_end.max(s.col_start + 1);
        let x0 = origin.x + s.col_start as f32 * cw;
        let x1 = origin.x + col_end as f32 * cw;
        // Baseline-ish: 1px above the cell bottom so the rule reads as an
        // underline rather than a row separator.
        let y = origin.y + s.line as f32 * ch + ch - 1.0;
        painter.line_segment(
            [egui::pos2(x0, y), egui::pos2(x1, y)],
            egui::Stroke::new(1.0, colors.accent),
        );
    }
}

/// Paint the find-overlay highlight over a pane's rendered grid: a dim tint
/// quad behind every match span and an accent outline around the active one.
/// Cell geometry is derived from the SAME monospace probe-galley the cursor
/// uses, so the quads land on the cell grid. GPU-free (egui rects only). A
/// match whose `line` exceeds the visible row count is skipped (the grid may
/// have scrolled since the match set was computed mid-frame).
fn paint_search_highlight(
    painter: &egui::Painter,
    rect: egui::Rect,
    font_size: f32,
    line_height_px: f32,
    padding: f32,
    colors: &theme::ChromeColors,
    hl: SearchHighlight<'_>,
) {
    if hl.spans.is_empty() {
        return;
    }
    // Cell size in POINTS — identical to the cursor's/grid's metric (the `M`
    // advance for width, the effective row pitch for height) so the highlight
    // aligns with the glyphs at any Line-height setting.
    let (cw, ch) = monospace_cell_points(painter, font_size, line_height_px);
    let origin = grid_text_origin(rect, padding);

    for (idx, s) in hl.spans.iter().enumerate() {
        // Spans are already in cell coordinates (built by `cell_spans_for_search`
        // via `byte_to_col`). The painter's clip rect keeps any over-wide quad
        // inside the pane, so no extra bounds math is needed.
        let col_end = s.col_end.max(s.col_start + 1);
        let x0 = origin.x + s.col_start as f32 * cw;
        let w = (col_end - s.col_start) as f32 * cw;
        let y0 = origin.y + s.line as f32 * ch;
        let span = egui::Rect::from_min_size(egui::pos2(x0, y0), egui::vec2(w, ch));
        // Dim accent tint behind every match.
        painter.rect_filled(span, 1.0, colors.accent.gamma_multiply(0.30));
        // The active match also gets a crisp outline so it reads as "current".
        if idx == hl.selected {
            painter.rect_stroke(
                span,
                1.0,
                egui::Stroke::new(1.5, colors.accent),
                egui::StrokeKind::Inside,
            );
        }
    }
}

/// The theme's default foreground as an `(r,g,b)` triple — the glyph colour for
/// runs with no explicit SGR colour, and the egui-painter fallback colour.
fn term_default_fg(theme: &c0pl4nd_core::Theme) -> (u8, u8, u8) {
    c0pl4nd_core::theme::parse_hex(&theme.foreground).unwrap_or((232, 230, 240))
}

// ---- CRT / chromatic-aberration painter effects (research §2) -------------
//
// eframe 0.34 owns the wgpu surface + render loop, so a TRUE fullscreen
// post-process shader over the whole composited UI is infeasible without
// dropping eframe for a raw egui-winit + egui-wgpu host (research §2 verdict).
// These are the STABLE painter-based approximations the research recommends:
// scanlines + vignette drawn over the grid with `egui::Painter`, and a
// per-glyph RGB ghost at the text-draw site. Both are GPU-free and ZERO-cost
// when the setting is off/zero (the caller gates on `crt_scanlines` /
// `chromatic_aberration > 0`).

/// The scanline period in PHYSICAL pixels. A scanline reads as a *line* only
/// when the eye resolves an alternating dark-band / lit-band pattern; on a
/// HiDPI panel a 3-logical-px period collapses sub-physical-px and the GPU
/// antialiases it into a uniform grey film (issue #28). Anchoring the period to
/// PHYSICAL pixels (`PERIOD / ppp` logical points) keeps the band/gap contrast
/// resolvable at any scale factor. ~3 physical px = a believable tube pitch.
const CRT_SCANLINE_PERIOD_PHYS_PX: f32 = 3.0;
/// Fraction of each period painted as the DARK band (the rest is the lit gap).
/// Real CRT shaders darken the *trough region* by ~40-70%, not a 1px sliver —
/// a wide band is what reads as a line. 0.66 = a 2-px-dark / 1-px-lit feel at a
/// 3-physical-px period.
const CRT_SCANLINE_DUTY: f32 = 0.66;
/// The dark-band alpha (0..=255) at the maximum configured darkness (1.0). The
/// effective alpha is `scanline_darkness * THIS` so the config slider tunes
/// trough darkness. The default darkness (0.4) lands at alpha 96 (~38% darken)
/// — the research band that reads as distinct lines, not a flat film (#28); full
/// darkness (1.0) caps at 240 (a near-black trough for a heavy-CRT look).
const CRT_SCANLINE_MAX_DARK_ALPHA: f32 = 240.0;
/// The animated rolling "scan" band speed (LOGICAL points / second) — the
/// classic CRT refresh sweep drifting down the pane.
const CRT_ROLL_SPEED_PTS_PER_SEC: f32 = 60.0;
/// The rolling scan band's height as a fraction of the content height.
const CRT_ROLL_HEIGHT_FRAC: f32 = 0.18;

/// The maximum horizontal RGB ghost offset (PHYSICAL pixels) — capped so a wild
/// config value can never smear the text into illegibility.
const CHROMATIC_MAX_OFFSET_PHYS_PX: f32 = 6.0;
/// The minimum visible ghost offset (PHYSICAL pixels) once aberration is ON. The
/// ghost must clear the opaque main glyph's edge to read as RGB separation
/// rather than vanishing under it (issue #28: "does nothing visible"). ≥2
/// physical px is the floor at which the fringe escapes the glyph.
const CHROMATIC_MIN_OFFSET_PHYS_PX: f32 = 2.0;

/// The horizontal RGB ghost offset (LOGICAL points) for a chromatic-aberration
/// `intensity`, resolved against the display's `ppp` (pixels-per-point). The
/// physical-px offset is `(MIN..=MAX) * intensity` clamped, then divided by
/// `ppp` to logical points the painter consumes — so on a 2× HiDPI panel the
/// fringe is still ≥2 PHYSICAL px and visibly clears the glyph (issue #28). The
/// red ghost draws at `-offset`, the blue ghost at `+offset`; `intensity == 0`
/// ⇒ offset `0` (off). Pure → unit-testable without a GPU.
fn chromatic_offset(intensity: f32, ppp: f32) -> f32 {
    if !intensity.is_finite() || intensity <= 0.0 {
        return 0.0;
    }
    let ppp = if ppp.is_finite() && ppp > 0.0 {
        ppp
    } else {
        1.0
    };
    // Physical-px separation scales with intensity from the visible floor to the
    // illegibility cap, so intensity 1.0 ≈ MIN..MAX-spanning fringe.
    let phys = (CHROMATIC_MIN_OFFSET_PHYS_PX
        + (CHROMATIC_MAX_OFFSET_PHYS_PX - CHROMATIC_MIN_OFFSET_PHYS_PX) * intensity.min(1.0))
    .clamp(CHROMATIC_MIN_OFFSET_PHYS_PX, CHROMATIC_MAX_OFFSET_PHYS_PX);
    phys / ppp
}

/// The alpha (0..=255) of each PURE-channel RGB ghost for a chromatic-aberration
/// `intensity`. The ghosts are pure red `(255,0,0)` / pure blue `(0,0,255)`
/// drawn BEHIND the crisp glyph, so only the un-occluded fringe shows as an
/// additive RGB split. Alpha is kept high (the fringe sits behind, never greys
/// the main glyph) and scales with intensity. `intensity == 0` ⇒ alpha `0`.
fn chromatic_ghost_alpha(intensity: f32) -> u8 {
    if !intensity.is_finite() || intensity <= 0.0 {
        return 0;
    }
    // 150 at low intensity scaling to 220 at full — saturated enough to POP as
    // RGB fringing (issue #28: the old 100..=140 tinted galleys washed to grey).
    let t = intensity.clamp(0.0, 1.0);
    (150.0 + 70.0 * t).clamp(0.0, 220.0).round() as u8
}

/// Edge-weight a base chromatic-aberration `offset` (points) by a glyph's
/// horizontal position, so the RGB fringing is stronger toward the screen
/// edges and near-zero at the centre — the authentic lens-style falloff a real
/// CRT shows (research §2(b): "edge-weighted aberration looks more authentic
/// than uniform"). `x` is the glyph's x; `[left, right]` the content span. The
/// normalised distance from centre (0 at centre, 1 at either edge) scales the
/// offset between 40% (centre) and 100% (edge), so the centre still shows a
/// faint fringe (never fully crisp) while the edges separate strongly. Pure →
/// unit-testable.
fn chromatic_edge_weighted_offset(offset: f32, x: f32, left: f32, right: f32) -> f32 {
    let span = right - left;
    if offset <= 0.0 || !span.is_finite() || span <= 0.0 {
        return offset.max(0.0);
    }
    let centre = left + span * 0.5;
    // 0 at centre → 1 at either edge.
    let dist = ((x - centre).abs() / (span * 0.5)).clamp(0.0, 1.0);
    offset * (0.4 + 0.6 * dist)
}

/// The scanline period in LOGICAL points for a display `ppp`. Anchored to
/// [`CRT_SCANLINE_PERIOD_PHYS_PX`] PHYSICAL pixels so the band/gap contrast is
/// resolvable at any scale factor (issue #28: a fixed logical period collapses
/// sub-physical-px on HiDPI and reads as a flat film). Pure → unit-testable.
fn scanline_period_pts(ppp: f32) -> f32 {
    let ppp = if ppp.is_finite() && ppp > 0.0 {
        ppp
    } else {
        1.0
    };
    CRT_SCANLINE_PERIOD_PHYS_PX / ppp
}

/// The number of dark scanline BANDS that fill a content `rect` of the given
/// `height` at the given `ppp`. One band per [`scanline_period_pts`]. Pure +
/// GPU-free so the band geometry is unit-testable without a painter.
fn scanline_count(height: f32, ppp: f32) -> usize {
    if !height.is_finite() || height <= 0.0 {
        return 0;
    }
    (height / scanline_period_pts(ppp)).ceil() as usize
}

/// The dark-band alpha (0..=255) for a configured `darkness` (0..=1). Maps the
/// config slider onto [`CRT_SCANLINE_MAX_DARK_ALPHA`] so the trough darkening is
/// tunable. Pure → unit-testable.
fn scanline_dark_alpha(darkness: f32) -> u8 {
    if !darkness.is_finite() || darkness <= 0.0 {
        return 0;
    }
    (darkness.clamp(0.0, 1.0) * CRT_SCANLINE_MAX_DARK_ALPHA)
        .clamp(0.0, 255.0)
        .round() as u8
}

/// The top Y (LOGICAL points) of the animated rolling "scan" band at time `t`
/// seconds for a content rect `[top, bottom)`. The band drifts down at
/// [`CRT_ROLL_SPEED_PTS_PER_SEC`] and wraps, starting fully off the top so it
/// sweeps in from above — the classic CRT refresh sweep. Pure → unit-testable.
fn scanline_roll_top(top: f32, height: f32, roll_h: f32, t: f32) -> f32 {
    if !height.is_finite() || height <= 0.0 {
        return top;
    }
    let span = height + roll_h;
    let phase = (t * CRT_ROLL_SPEED_PTS_PER_SEC).rem_euclid(span);
    top + phase - roll_h
}

/// Paint REAL CRT scan lines across the WHOLE pane content `rect` (issue #28) —
/// filled DARK BANDS (not 1px slivers) at a PHYSICAL-px-anchored period, plus an
/// animated rolling brighten band so the tube visibly "scans". `ppp` resolves
/// the period to logical points; `t` is the animation clock (seconds);
/// `darkness` (0..=1) tunes the trough darkness. GPU-free (filled rects). The
/// caller's `painter_at(rect)` clip keeps every band inside the pane; the caller
/// also requests a repaint each frame so the roll keeps moving.
fn paint_crt_scanlines(painter: &egui::Painter, rect: egui::Rect, ppp: f32, t: f32, darkness: f32) {
    let period = scanline_period_pts(ppp);
    let band_h = period * CRT_SCANLINE_DUTY;
    let dark = egui::Color32::from_black_alpha(scanline_dark_alpha(darkness));
    // --- static dark bands: filled rects across the whole content width ---
    let lines = scanline_count(rect.height(), ppp);
    for i in 0..lines {
        let y = rect.top() + i as f32 * period;
        let band = egui::Rect::from_min_max(
            egui::pos2(rect.left(), y),
            egui::pos2(rect.right(), y + band_h),
        );
        painter.rect_filled(band, 0.0, dark);
    }
    // --- animated rolling "scan" band: a soft white brighten bar drifting down,
    // built from a few stacked translucent rects for a cheap gaussian falloff.
    let roll_h = (rect.height() * CRT_ROLL_HEIGHT_FRAC).max(1.0);
    let roll_top = scanline_roll_top(rect.top(), rect.height(), roll_h, t);
    for k in 0..4u8 {
        let a = (10u8.saturating_sub(k * 2)).max(2);
        let inset = roll_h * f32::from(k) / 8.0;
        let band = egui::Rect::from_min_max(
            egui::pos2(rect.left(), roll_top + inset),
            egui::pos2(rect.right(), roll_top + roll_h - inset),
        );
        painter.rect_filled(band, 0.0, egui::Color32::from_white_alpha(a));
    }
}

/// The top-left point at which a pane's terminal grid text is drawn, given the
/// pane's body `rect` and the configurable inner `padding` (points). Pure +
/// GPU-free so the padding live-apply wiring is unit-testable: the origin must
/// move with the padding (a larger padding insets the grid further from the
/// pane's top-left corner). Negative paddings are clamped to zero so a bad
/// config can never push the origin outside the pane.
fn grid_text_origin(rect: egui::Rect, padding: f32) -> egui::Pos2 {
    let p = padding.max(0.0);
    rect.left_top() + egui::vec2(p, p)
}

/// Paint a pane's visible grid with egui's NATIVE text painter, using the
/// per-row colour runs from [`PaneTerm::grid_rows`]. This is the single,
/// engine-agnostic render path for BOTH the live window and the headless
/// snapshot tests — identical code, so a passing test faithfully proves the
/// live render. It deliberately uses egui's own glyph rasteriser (the same one
/// that draws the chrome, and the same approach SCR1B3 uses for coloured code)
/// rather than a glyphon GPU paint callback: the glyphon paint (in-pass
/// callback AND offscreen texture) composited black inside `egui_tiles` panes
/// on the real eframe/winit swapchain — a class of defect the wgpu test harness
/// could not reproduce — whereas native text renders reliably everywhere.
///
/// Rows are NOT wrapped (`max_width = INFINITY`): each terminal row stays one
/// visual line and is clipped at the pane edge by the caller's `painter_at`
/// clip rect, so row alignment is preserved.
///
/// The argument list is a bundle of per-frame render inputs (font size,
/// line-height, theme, focus, cursor config, effects, padding) threaded from the
/// single call site in [`C0pl4ndApp::render_pane_body`]; the
/// `too_many_arguments` allow matches that sibling free function for the same
/// reason.
///
/// Rows are painted ONE GALLEY PER ROW at the effective row pitch
/// ([`effective_row_pitch`] of the natural galley height and the configured
/// `line_height_px`) rather than as a single multi-row galley — egui's combined
/// galley uses the font's own line spacing, which the Line-height setting could
/// not influence. Per-row positioning makes the row pitch the live, configurable
/// thing the cursor / search / hit-test all share.
#[allow(clippy::too_many_arguments)]
fn paint_grid_native(
    painter: &egui::Painter,
    rect: egui::Rect,
    term: &PaneTerm,
    pane_id: PaneId,
    galley_cache: &mut GalleyCache,
    font_size: f32,
    line_height_px: f32,
    theme: &c0pl4nd_core::Theme,
    focused: bool,
    cursor_cfg: c0pl4nd_core::config::CursorConfig,
    effects: c0pl4nd_core::config::EffectsConfig,
    padding: f32,
) {
    let default_fg = term_default_fg(theme);
    let font = egui::FontId::monospace(font_size);
    // Inset the grid by the configurable window padding (points), read live from
    // `config.window.padding` each frame (threaded down from `grid_ui`). Pure
    // [`grid_text_origin`] helper so the live-apply wiring is unit-testable.
    let origin = grid_text_origin(rect, padding);
    // Cell size in POINTS: `M` advance for width, the effective row pitch for the
    // vertical advance per grid row. This is the SAME `(cw, ch)` the cursor,
    // search highlight, and hyperlink hit-test use, so every Y stays aligned.
    let (cw, ch) = monospace_cell_points(painter, font_size, line_height_px);

    // Each row becomes one galley, painted at `origin.y + row_idx * ch`.
    // Damage-gated, already grouped per-row by [`PaneTerm::grid_rows`] (an `Rc`
    // clone on the idle/blinking-cursor path — no per-frame grid clone, run
    // rebuild, or newline-split). The fallback (dead session mid-frame) wraps the
    // mono text in the same `Rc` shape so the paint loop below is uniform.
    let rows: std::rc::Rc<Vec<Vec<ColorRun>>> = match term.grid_rows() {
        Some(rows) if !rows.is_empty() => rows,
        _ => {
            // No colour runs (e.g. dead session mid-frame): mono fallback so the
            // pane is never blank. One row per text line, all in the default fg.
            std::rc::Rc::new(
                term.grid_text()
                    .unwrap_or_default()
                    .lines()
                    .map(|line| vec![(line.to_string(), default_fg)])
                    .collect(),
            )
        }
    };

    // The effective chromatic intensity (gated by the explicit enable toggle),
    // resolved to a PHYSICAL-px-aware ghost offset so the fringe clears the glyph
    // on HiDPI panels (issue #28). Zero-cost when off (offset == 0 ⇒ skipped).
    let ppp = painter.ctx().pixels_per_point();
    let chroma = effects.effective_chromatic();
    let ghost_offset = chromatic_offset(chroma, ppp);
    let ghost_alpha = chromatic_ghost_alpha(chroma);
    // Style bits shared by every row this frame (font size + the fallback fg).
    // Folded into each row's cache key so a font-size or theme change relays the
    // rows (a font FAMILY change clears the whole cache via `clear()`).
    let style_key = row_style_key(font_size, default_fg);
    for (row_idx, runs) in rows.iter().enumerate() {
        let row_origin = egui::pos2(origin.x, origin.y + row_idx as f32 * ch);
        // Content hash of this row's runs (text + per-cell colours), folded with
        // the shared style bits — the cache key for the crisp pass. Computed once
        // and reused for the ghost passes (which add the override colour).
        let content_key = row_content_key(runs, style_key);
        // --- chromatic aberration (research §2 + §2(b) edge-weighting): re-draw
        // the row's glyphs as PURE-CHANNEL ghosts at ±offset BEHIND the crisp
        // pass — a pure-red copy shifted left and a pure-blue copy shifted right,
        // so only the un-occluded fringe spills past the glyph edge as an
        // authentic additive RGB split (tinted galleys washed to grey under the
        // glyph; pure channels pop). The offset is EDGE-WEIGHTED by the row's
        // vertical position so the fringing is stronger toward the top/bottom of
        // the pane and near-zero at the middle — the CRT lens falloff. The ghost
        // galleys are cached separately per pass (the override colour + alpha
        // fold into their key) so they too skip re-layout on an unchanged row.
        if ghost_offset > 0.0 {
            let row_y = row_origin.y;
            let off =
                chromatic_edge_weighted_offset(ghost_offset, row_y, rect.top(), rect.bottom());
            // Pure red, shifted LEFT — drawn first (behind).
            let red = egui::Color32::from_rgba_unmultiplied(255, 0, 0, ghost_alpha);
            let red_galley = galley_cache.row_galley(
                painter,
                pane_id,
                row_idx,
                RowPass::GhostRed,
                content_key ^ (u64::from(ghost_alpha) << 8 | 0x01),
                || build_row_job(runs, &font, Some(red)),
            );
            painter.galley(
                row_origin + egui::vec2(-off, 0.0),
                red_galley,
                egui::Color32::from_rgb(default_fg.0, default_fg.1, default_fg.2),
            );
            // Pure blue, shifted RIGHT.
            let blue = egui::Color32::from_rgba_unmultiplied(0, 0, 255, ghost_alpha);
            let blue_galley = galley_cache.row_galley(
                painter,
                pane_id,
                row_idx,
                RowPass::GhostBlue,
                content_key ^ (u64::from(ghost_alpha) << 8 | 0x02),
                || build_row_job(runs, &font, Some(blue)),
            );
            painter.galley(
                row_origin + egui::vec2(off, 0.0),
                blue_galley,
                egui::Color32::from_rgb(default_fg.0, default_fg.1, default_fg.2),
            );
        }
        // Crisp main pass in the runs' real colours, on top of any ghosts.
        let main_galley = galley_cache.row_galley(
            painter,
            pane_id,
            row_idx,
            RowPass::Main,
            content_key,
            || build_row_job(runs, &font, None),
        );
        painter.galley(
            row_origin,
            main_galley,
            egui::Color32::from_rgb(default_fg.0, default_fg.1, default_fg.2),
        );
    }

    // --- terminal cursor ---
    if let Some((row, col)) = term.cursor_cell() {
        let cell_min = origin + egui::vec2(col as f32 * cw, row as f32 * ch);
        let cell = egui::Rect::from_min_size(cell_min, egui::vec2(cw, ch));
        let cur = c0pl4nd_core::theme::parse_hex(&theme.cursor).unwrap_or((0, 255, 144));
        let col32 = egui::Color32::from_rgb(cur.0, cur.1, cur.2);
        // Blink only on the focused pane (and only if configured). The live
        // window repaints every frame, so the phase animates without an explicit
        // repaint request; headless tests see a steady ON frame.
        let on = if cursor_cfg.blink && focused {
            (painter.ctx().input(|i| i.time) / 1.06).fract() < 0.5
        } else {
            true
        };
        if on {
            match cursor_cfg.style {
                c0pl4nd_core::config::CursorStyle::Block => {
                    if focused {
                        // Semi-transparent fill so the glyph beneath stays legible.
                        painter.rect_filled(cell, 1.0, col32.gamma_multiply(0.55));
                    } else {
                        painter.rect_stroke(
                            cell,
                            1.0,
                            egui::Stroke::new(1.0, col32),
                            egui::StrokeKind::Inside,
                        );
                    }
                }
                c0pl4nd_core::config::CursorStyle::Bar => {
                    let bar = egui::Rect::from_min_size(cell_min, egui::vec2(2.0, ch));
                    painter.rect_filled(bar, 0.0, col32);
                }
                c0pl4nd_core::config::CursorStyle::Underline => {
                    let under = egui::Rect::from_min_size(
                        cell_min + egui::vec2(0.0, ch - 2.0),
                        egui::vec2(cw, 2.0),
                    );
                    painter.rect_filled(under, 0.0, col32);
                }
            }
        }
    }

    // --- CRT scanlines (research §1): the LAST thing painted over this pane's
    // grid, so the dark bands dim the glyphs + cursor uniformly. Filled dark
    // bands at a physical-px-anchored period + an animated rolling scan band.
    // Drawn only when the setting is on (strictly zero-cost otherwise); the
    // repaint request keeps the roll animating without an explicit timer.
    if effects.crt_scanlines {
        // F2-2: honour the user's reduced-motion preference (env override OR the
        // OS accessibility setting). When reduced motion is requested, FREEZE the
        // rolling scan band (`t = 0` → a static frame; the dark scan-line bands
        // are a texture, not motion, so they remain) and STOP the per-frame
        // animation repaint. This makes the "Auto-disabled under reduced-motion"
        // promise the settings UI already shows actually true.
        let reduce = c0pl4nd_core::reduced_motion::reduced_motion();
        let t = if reduce {
            0.0
        } else {
            painter.ctx().input(|i| i.time) as f32
        };
        paint_crt_scanlines(painter, rect, ppp, t, effects.scanline_darkness);
        if !reduce {
            painter.ctx().request_repaint();
        }
    }
}

/// Build the [`egui::text::LayoutJob`] for one grid row's colour runs (a single
/// unwrapped line). When `override_color` is `Some`, every run is drawn in that
/// colour (the R/B chromatic-aberration ghost passes); when `None`, each run
/// keeps its real SGR colour (the crisp main pass). Called only on a galley-cache
/// MISS (the per-run string-append allocations are the cost the cache avoids on a
/// hit — see [`GalleyCache::row_galley`]).
fn build_row_job(
    runs: &[ColorRun],
    font: &egui::FontId,
    override_color: Option<egui::Color32>,
) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::default();
    job.wrap.max_width = f32::INFINITY;
    for (text, (r, g, b)) in runs {
        let color = override_color.unwrap_or_else(|| egui::Color32::from_rgb(*r, *g, *b));
        job.append(
            text,
            0.0,
            egui::text::TextFormat {
                font_id: font.clone(),
                color,
                ..Default::default()
            },
        );
    }
    job
}

/// Fold the per-frame style bits (font size + fallback fg) into a stable seed for
/// a row's galley-cache key. Font SIZE is captured here (so a size change relays
/// the rows); a font FAMILY/fallback change instead clears the whole cache (the
/// galleys reference the old atlas). Pure → unit-testable.
fn row_style_key(font_size: f32, default_fg: (u8, u8, u8)) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    font_size.to_bits().hash(&mut h);
    default_fg.hash(&mut h);
    h.finish()
}

/// Hash a grid row's colour runs (text + per-cell RGB) folded with the shared
/// `style_key` seed — the galley-cache key for the crisp main pass. Two rows
/// produce the same key iff they lay out to the same galley, so an unchanged row
/// reuses its cached galley instead of re-running layout. Pure → unit-testable.
fn row_content_key(runs: &[ColorRun], style_key: u64) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    style_key.hash(&mut h);
    for (text, rgb) in runs {
        text.hash(&mut h);
        rgb.hash(&mut h);
    }
    h.finish()
}

/// Quote a script `path` as a command line for the active shell (#35), so the
/// shell EXECUTES the file (via its shebang/interpreter) rather than the app
/// reading + injecting its lines. The form depends on the shell named by
/// `shell_label`:
///
/// * **PowerShell** (`PowerShell 7` / `Windows PowerShell`): the call operator
///   `& "<path>"` — required because a bare quoted path in PowerShell is a
///   string expression, not an invocation. Embedded `"` are backtick-escaped
///   (PowerShell's double-quote escape inside a `"…"` string).
/// * **cmd / Default shell on Windows**: a plain double-quoted path `"<path>"`.
/// * **POSIX shells** (bash/zsh/fish/sh, the Default shell off Windows): a
///   single-quoted path `'<path>'`, with the POSIX `'\''` escape for any
///   embedded single quote.
///
/// Pure (no I/O) so the per-shell quoting is unit-testable. The path is rendered
/// with `Path::display()` (lossy on non-UTF-8 paths — acceptable for a
/// user-picked script path typed into a shell).
fn quote_path_for_shell(path: &std::path::Path, shell_label: &str) -> String {
    let raw = path.display().to_string();
    if shell_label.contains("PowerShell") {
        // PowerShell call operator; `"` → `` ` `` + `"` inside the double-quoted
        // string.
        let escaped = raw.replace('"', "`\"");
        return format!("& \"{escaped}\"");
    }
    if cfg!(windows) {
        // cmd.exe (incl. the Windows "Default shell"): a double-quoted path. cmd
        // has no in-quote escape for `"`, but Windows paths cannot contain `"`,
        // so a plain wrap is correct.
        return format!("\"{raw}\"");
    }
    // POSIX shell: single-quote, escaping any embedded single quote as '\''.
    let escaped = raw.replace('\'', "'\\''");
    format!("'{escaped}'")
}

/// Map an `egui::Key` (+ modifiers) onto the engine-agnostic [`LogicalKey`] for
/// the special keys the PTY needs as escape sequences. Returns `None` for keys
/// whose text is already delivered via `egui::Event::Text` (ordinary printable
/// characters), so they are not double-sent. Ctrl-letter chords ARE encoded
/// here (egui does not emit `Event::Text` for them) into their C0 control byte.
fn egui_key_to_logical(
    key: egui::Key,
    mods: c0pl4nd_core::term::KeyModifiers,
) -> Option<c0pl4nd_core::term::LogicalKey> {
    use c0pl4nd_core::term::LogicalKey;
    use egui::Key;
    let lk = match key {
        Key::Enter => LogicalKey::Enter,
        Key::Backspace => LogicalKey::Backspace,
        Key::Tab => LogicalKey::Tab,
        Key::Escape => LogicalKey::Escape,
        Key::Space if mods.ctrl => {
            // Ctrl+Space → NUL (the canonical set-mark byte). Ordinary Space is
            // delivered via Event::Text, so only the Ctrl chord is handled here.
            return Some(LogicalKey::Text(String::from('\u{0}')));
        }
        Key::ArrowUp => LogicalKey::ArrowUp,
        Key::ArrowDown => LogicalKey::ArrowDown,
        Key::ArrowRight => LogicalKey::ArrowRight,
        Key::ArrowLeft => LogicalKey::ArrowLeft,
        Key::Home => LogicalKey::Home,
        Key::End => LogicalKey::End,
        Key::Insert => LogicalKey::Insert,
        Key::Delete => LogicalKey::Delete,
        Key::PageUp => LogicalKey::PageUp,
        Key::PageDown => LogicalKey::PageDown,
        Key::F1 => LogicalKey::Function(1),
        Key::F2 => LogicalKey::Function(2),
        Key::F3 => LogicalKey::Function(3),
        Key::F4 => LogicalKey::Function(4),
        Key::F5 => LogicalKey::Function(5),
        Key::F6 => LogicalKey::Function(6),
        Key::F7 => LogicalKey::Function(7),
        Key::F8 => LogicalKey::Function(8),
        Key::F9 => LogicalKey::Function(9),
        Key::F10 => LogicalKey::Function(10),
        Key::F11 => LogicalKey::Function(11),
        Key::F12 => LogicalKey::Function(12),
        other => {
            // Ctrl + a-z → the C0 control byte (Ctrl+C = 0x03, etc.). egui does
            // not emit Event::Text for these chords, so encode them here.
            if mods.ctrl {
                if let Some(name) = other.name().chars().next() {
                    let up = name.to_ascii_uppercase();
                    if up.is_ascii_uppercase() {
                        let ctrl_byte = (up as u8) & 0x1f;
                        return Some(LogicalKey::Text(
                            String::from_utf8(vec![ctrl_byte]).unwrap_or_default(),
                        ));
                    }
                }
            }
            return None;
        }
    };
    Some(lk)
}

/// Load the persisted user config from its canonical path, returning the config
/// AND a parse-error message (F5-2) when a config file EXISTS but fails to
/// read/parse — so the caller can surface it as a visible toast instead of the
/// silent fallback-to-defaults that previously only `eprintln`'d (invisible to a
/// GUI-launched user). Without loading at all the egui app would start from
/// `Config::default()` every launch, so on-disk settings the panel WROTE never
/// took effect. `None` error means the file was absent (normal zero-config
/// start) or parsed cleanly. Pure `core` APIs, available in every binary that
/// includes this module (incl. the `#[path]`-included test bins).
fn load_config_with_status() -> (c0pl4nd_core::Config, Option<String>) {
    load_config_from(c0pl4nd_core::Config::default_path().filter(|p| p.exists()))
}

/// Pure core of config loading, parameterised on the path so it is unit-testable
/// (the real entry resolves `Config::default_path()`). An absent path → defaults
/// with no error; a present-but-invalid file → defaults WITH an error message
/// (the F5-2 surfacing contract).
fn load_config_from(path: Option<std::path::PathBuf>) -> (c0pl4nd_core::Config, Option<String>) {
    match path {
        Some(p) => match std::fs::read_to_string(&p)
            .map_err(|e| e.to_string())
            .and_then(|s| c0pl4nd_core::Config::from_toml(&s, &p).map_err(|e| e.to_string()))
        {
            Ok(cfg) => (cfg, None),
            Err(e) => {
                eprintln!("c0pl4nd: failed to load config {p:?}: {e}; using defaults");
                (
                    c0pl4nd_core::Config::default(),
                    Some(format!("config error — using defaults: {e}")),
                )
            }
        },
        None => (c0pl4nd_core::Config::default(), None),
    }
}

/// Load the terminal colour theme named by `config.theme` from the bundled
/// themes dir (next to the binary or in the source tree during development),
/// falling back to the built-in Itasha.Corp void theme when the file is absent.
/// The terminal grid's glyph colours come from this theme — NOT egui Visuals.
fn load_terminal_theme(config: &c0pl4nd_core::Config) -> c0pl4nd_core::Theme {
    let mut candidates: Vec<std::path::PathBuf> =
        vec![std::path::PathBuf::from("assets/themes").join(format!("{}.toml", config.theme))];
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            candidates.push(
                parent
                    .join("assets/themes")
                    .join(format!("{}.toml", config.theme)),
            );
        }
    }
    for c in candidates {
        if let Ok(t) = c0pl4nd_core::Theme::load_from(&c) {
            return t;
        }
    }
    // No on-disk file matched (the common case in the INSTALLED app, whose CWD
    // is not the source tree and which ships no `assets/themes/` next to the
    // exe). Resolve from the COMPILED-IN theme set so selection still works —
    // this is the fix for "the theme doesn't change". On-disk files above still
    // win when present, so a user can override a built-in or add their own.
    if let Some(t) = c0pl4nd_core::Theme::builtin_named(&config.theme) {
        return t;
    }
    c0pl4nd_core::Theme::builtin_void()
}

/// Build egui's base font set: the Phosphor icon font merged into BOTH the
/// proportional and monospace families (so the chrome's caption glyphs —
/// close/maximize/minimize/gear, split-right/down — render as crisp icons
/// instead of tofu), plus the SOLID `phosphor-fill` family used by a pinned
/// tab's pin. This is the icon-only base; [`install_chrome_fonts`] layers the
/// user's configured monospace family on top.
fn base_font_definitions() -> egui::FontDefinitions {
    let mut fonts = egui::FontDefinitions::default();
    // Thin = the default chrome icon weight (registered as "phosphor").
    egui_phosphor::add_to_fonts(&mut fonts, egui_phosphor::Variant::Thin);
    // Fill = SOLID glyphs, registered under a SEPARATE family so most icons stay
    // thin but a pinned tab can show a solid pin (`add_to_fonts` always uses the
    // "phosphor" key, so a second call would overwrite Thin with Fill). Use via
    // `RichText::new(fill_glyph).family(FontFamily::Name("phosphor-fill".into()))`.
    fonts.font_data.insert(
        "phosphor-fill".to_owned(),
        egui_phosphor::Variant::Fill.font_data().into(),
    );
    fonts.families.insert(
        egui::FontFamily::Name("phosphor-fill".into()),
        vec!["phosphor-fill".to_owned()],
    );
    // `add_to_fonts` registers the "phosphor" font_data and inserts it into the
    // Proportional family; also append it to Monospace so monospace buttons can
    // resolve the icons.
    if let Some(mono) = fonts.families.get_mut(&egui::FontFamily::Monospace) {
        if !mono.iter().any(|f| f == "phosphor") {
            mono.push("phosphor".to_owned());
        }
    }
    fonts
}

/// Install the chrome icon fonts AND the user's configured monospace family +
/// fallbacks into egui's font set. The configured family (and each fallback,
/// in order) is loaded from the system font DB and PREPENDED to
/// `FontFamily::Monospace`, so the terminal grid and every monospace UI surface
/// render in the chosen font; egui's default monospace + the Phosphor icons stay
/// at the END as the ultimate fallback. A family that is the built-in label, is
/// "(none)", or is simply not installed is skipped gracefully (no panic) and the
/// built-in monospace remains in use.
///
/// Loading the system font DB is slow (100s of ms), so it runs ONLY when the
/// config names at least one real (non-built-in) family to load — the common
/// "built-in mono" path pays nothing. Called from `new()` and from the
/// first-frame gate in `frame_tick`, and re-run live by [`C0pl4ndApp::frame_tick`]
/// when the user changes the family/fallback in settings.
fn install_chrome_fonts(ctx: &egui::Context, font: &c0pl4nd_core::config::FontConfig) {
    // Fast path: nothing custom to load (default config / built-in choice) — set
    // the icon base and skip the expensive system-font enumeration entirely.
    if !system_font_load_needed(font) {
        ctx.set_fonts(base_font_definitions());
        return;
    }
    // Custom family: the (100s-of-ms) system DB load runs synchronously here. The
    // startup path avoids this by going through `install_base_fonts` +
    // `spawn_system_font_load` instead (audit #3); this synchronous form is the
    // settings-change re-install (user-initiated, expects an immediate apply) and
    // the headless-test path (deterministic, no worker thread).
    ctx.set_fonts(build_system_font_definitions(font));
}

/// Whether the configured font stack names any non-built-in family, i.e. whether
/// the (slow) system-font DB load is required. Pure → unit-testable.
fn system_font_load_needed(font: &c0pl4nd_core::config::FontConfig) -> bool {
    !fonts::is_builtin_family(&font.family)
        || font.fallback.iter().any(|f| !fonts::is_builtin_family(f))
}

/// Install ONLY the built-in icon/base fonts (no system-DB enumeration), so the
/// first frame paints immediately with the built-in monospace while the custom
/// system fonts load on a worker thread (audit #3).
fn install_base_fonts(ctx: &egui::Context) {
    ctx.set_fonts(base_font_definitions());
}

/// Build the full [`egui::FontDefinitions`] for a custom font stack: enumerate
/// the system font DB and prepend the chosen family + fallbacks to
/// `FontFamily::Monospace`. This is the heavy (100s-of-ms) call — invoked off the
/// startup critical path on a worker thread by [`spawn_system_font_load`], and
/// synchronously by [`install_chrome_fonts`] on a settings change.
fn build_system_font_definitions(font: &c0pl4nd_core::config::FontConfig) -> egui::FontDefinitions {
    let base = base_font_definitions();
    let mut db = fontdb::Database::new();
    db.load_system_fonts();
    let (defs, _loaded) = fonts::build_font_definitions(base, &db, &font.family, &font.fallback);
    defs
}

/// Spawn a worker thread that builds the custom-font [`egui::FontDefinitions`]
/// off the startup critical path (audit #3) and returns the receiver the frame
/// loop polls. The closure owns a clone of the font config so the thread is
/// self-contained. `frame_tick` calls `ctx.set_fonts(defs)` when the result
/// arrives — until then the window paints with the built-in mono from
/// [`install_base_fonts`]. A send failure (the app dropped the receiver, e.g. at
/// shutdown) is ignored — the result is simply discarded.
fn spawn_system_font_load(
    font: &c0pl4nd_core::config::FontConfig,
) -> std::sync::mpsc::Receiver<egui::FontDefinitions> {
    let (tx, rx) = std::sync::mpsc::channel();
    let font = font.clone();
    std::thread::Builder::new()
        .name("c0pl4nd-font-load".to_string())
        .spawn(move || {
            let defs = build_system_font_definitions(&font);
            let _ = tx.send(defs);
        })
        // A spawn failure (resource-exhausted) is non-fatal: fall back to the
        // built-in mono already installed; no custom font this session.
        .ok();
    rx
}

/// Fold a [`FontConfig`](c0pl4nd_core::config::FontConfig)'s family + ordered
/// fallbacks into a single stable key. Two configs produce the same key iff they
/// install the SAME monospace font stack, so the frame loop can re-install the
/// egui fonts ONLY when this key actually changes (the live-apply gate). Pure +
/// GPU-free so the gate is unit-testable. Size / line-height are deliberately
/// excluded — they do not change which font FILE is loaded, only how it is
/// drawn.
fn font_apply_key(font: &c0pl4nd_core::config::FontConfig) -> String {
    let mut key = font.family.trim().to_string();
    for f in &font.fallback {
        key.push('\u{1f}'); // unit-separator: cannot appear in a family name
        key.push_str(f.trim());
    }
    key
}

/// The minimum translucent panel alpha (fraction). Below this the grid text
/// would be unreadable; matches SCR1B3's `0.05` slider floor so the full
/// opacity-slider travel is live (the old `0.30` floor was a dead band that
/// made low opacities "just dim" instead of going see-through — issue #27).
const TRANSLUCENT_ALPHA_FLOOR: f32 = 0.05;

/// The alpha (0..=255) to paint the pane grid background (and the central panel
/// fill) with, for the current config:
///
/// * **Opaque** (master toggle off, or `Opaque` mode): `255` — a solid fill so
///   the desktop never bleeds through. The unchanged, safe default.
/// * **Translucent** (`effective_translucent()`): the `opacity` slider folded
///   into a 0..=255 alpha (floored at [`TRANSLUCENT_ALPHA_FLOOR`] so the grid
///   never fully vanishes). The opacity slider drives the fill alpha across its
///   FULL range in every translucent mode — Glass/Mica/Vibrancy are
///   distinguished by their DWM backdrop EFFECT (acrylic / mica / plain, applied
///   separately via `window-vibrancy`), NOT by capping the alpha. A prior
///   per-mode ceiling (#27) capped Glass at 0.35 etc., which made the slider a
///   no-op above the cap AND washed the terminal content out to near-invisible
///   over a bright backdrop (#41). The backdrop now shows through because the
///   DEFAULT opacity is < 1.0 (see `Config` default), not because the alpha is
///   force-capped — so opacity 1.0 legitimately means "fully opaque".
///
/// Pure (`&Config`) so the transparency wiring is unit-testable without a
/// window.
fn pane_bg_alpha(config: &c0pl4nd_core::Config) -> u8 {
    if !config.effective_translucent() {
        return 255;
    }
    // The opacity slider drives the alpha directly in ALL translucent modes,
    // floored so the grid stays readable. No per-mode ceiling: the modes differ
    // by their DWM backdrop, not by a forced alpha cap (#41).
    let a = config.opacity.clamp(TRANSLUCENT_ALPHA_FLOOR, 1.0);
    (a * 255.0).round().clamp(0.0, 255.0) as u8
}

/// The frameless-window clear color for the current config + theme.
///
/// * **Opaque** (master off, or `Opaque` mode): the theme background at full
///   alpha — a solid window the desktop never bleeds through.
/// * **Translucent with a native blur** (`Glass`/`Mica`/`Vibrancy`): fully
///   transparent so the OS blur backdrop shows through.
/// * **`Transparent` mode** (portable, no native blur): the theme background
///   with alpha folded down to the `opacity` slider so the desktop shows
///   through at the chosen strength.
///
/// Free function (takes `&Config`, `&Theme`) so the headless tests can assert
/// the clear color for a given config without an eframe window.
fn window_clear_color(config: &c0pl4nd_core::Config, theme: &c0pl4nd_core::Theme) -> [f32; 4] {
    if !config.effective_translucent() {
        // Opaque: solid theme background, full alpha.
        let (r, g, b) =
            c0pl4nd_core::theme::parse_hex(&theme.background).unwrap_or((0x12, 0x12, 0x12));
        return [r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, 1.0];
    }
    match config.window_mode {
        // Native blur backdrops want a fully transparent surface so the OS
        // composited blur shows through.
        c0pl4nd_core::config::WindowMode::Glass
        | c0pl4nd_core::config::WindowMode::Mica
        | c0pl4nd_core::config::WindowMode::Vibrancy => [0.0, 0.0, 0.0, 0.0],
        // Portable see-through: theme background, alpha = opacity slider. Floored
        // at the shared TRANSLUCENT_ALPHA_FLOOR (0.05) so the slider's full travel
        // is live (the old 0.30 floor was a dead band — #27).
        c0pl4nd_core::config::WindowMode::Transparent => {
            let (r, g, b) =
                c0pl4nd_core::theme::parse_hex(&theme.background).unwrap_or((0x12, 0x12, 0x12));
            let a = config.opacity.clamp(TRANSLUCENT_ALPHA_FLOOR, 1.0);
            [r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, a]
        }
        // Unreachable: effective_translucent() ruled Opaque out above.
        c0pl4nd_core::config::WindowMode::Opaque => [0.0, 0.0, 0.0, 0.0],
    }
}

/// Paint a full-window translucent tint overlay (a subtle color wash) on a
/// foreground layer — portable across every translucent mode and OS, mirroring
/// SCR1B3's `paint_tint_overlay`. A no-op when `strength <= 0` or the tint is
/// not a valid `#RRGGBB`.
fn paint_tint_overlay(ctx: &egui::Context, tint_hex: &str, strength: f32) {
    if strength <= 0.0 {
        return;
    }
    let Ok((r, g, b)) = c0pl4nd_core::theme::parse_hex(tint_hex) else {
        return;
    };
    let a = (strength.clamp(0.0, 1.0) * 90.0).round() as u8;
    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Foreground,
        egui::Id::new("c0pl4nd-tint-overlay"),
    ));
    painter.rect_filled(
        ctx.content_rect(),
        0.0,
        egui::Color32::from_rgba_unmultiplied(r, g, b, a),
    );
}

/// Parse a `#RRGGBB` tint to an RGBA quad for native blur tinting.
///
/// Only consumed by Windows' `window_vibrancy::apply_acrylic` (acrylic takes a
/// tint; mica/vibrancy do not). Gating the fn to Windows keeps `-D warnings`
/// (clippy `dead_code`) green on Linux and macOS without a blanket allow.
#[cfg(windows)]
fn tint_rgba(hex: &str, alpha: u8) -> Option<(u8, u8, u8, u8)> {
    c0pl4nd_core::theme::parse_hex(hex)
        .ok()
        .map(|(r, g, b)| (r, g, b, alpha))
}

/// Apply the OS window effect for the chosen [`WindowMode`] (best-effort,
/// graceful on unsupported platforms — recon dossier §3.3). Windows:
/// acrylic (Glass) / mica (Mica); macOS: vibrancy; elsewhere (Linux) the
/// portable transparent surface + the tint overlay carry the look. Called only
/// when the master transparency toggle is on AND the mode wants a non-opaque
/// surface (`Config::effective_translucent`), so an opaque window never gets a
/// layered surface (no ghost-on-close risk).
///
/// LIVE-APPLY VERDICT (research §1): this is invoked ONCE at startup in
/// [`C0pl4ndApp::new`] because it needs the `eframe::CreationContext`'s raw
/// window handle, which `frame_tick` (driven only by `&egui::Context`) does not
/// expose — eframe 0.34 gives no stable cross-platform way to re-apply a DWM
/// backdrop class to the live window from inside the frame loop. So switching
/// the transparency MODE (Glass⇄Mica⇄Transparent) or toggling the master switch
/// at runtime needs a RELAUNCH for the DWM backdrop class to change. What IS
/// live: the PANEL/grid translucency — [`pane_bg_alpha`] reads `opacity` +
/// `effective_translucent()` from the config EVERY frame, so the opacity slider
/// and the pane see-through (the main visible lever) take effect immediately
/// without a relaunch.
fn apply_window_effect(
    cc: &eframe::CreationContext<'_>,
    mode: c0pl4nd_core::config::WindowMode,
    tint_hex: &str,
) {
    let _ = (cc, tint_hex);
    match mode {
        c0pl4nd_core::config::WindowMode::Glass => {
            #[cfg(windows)]
            {
                let _ = window_vibrancy::apply_acrylic(cc, tint_rgba(tint_hex, 160));
            }
            #[cfg(target_os = "macos")]
            {
                let _ = window_vibrancy::apply_vibrancy(
                    cc,
                    window_vibrancy::NSVisualEffectMaterial::HudWindow,
                    None,
                    None,
                );
            }
        }
        c0pl4nd_core::config::WindowMode::Mica => {
            #[cfg(windows)]
            {
                let _ = window_vibrancy::apply_mica(cc, Some(true));
            }
            #[cfg(target_os = "macos")]
            {
                let _ = window_vibrancy::apply_vibrancy(
                    cc,
                    window_vibrancy::NSVisualEffectMaterial::HudWindow,
                    None,
                    None,
                );
            }
        }
        c0pl4nd_core::config::WindowMode::Vibrancy => {
            #[cfg(target_os = "macos")]
            {
                let _ = window_vibrancy::apply_vibrancy(
                    cc,
                    window_vibrancy::NSVisualEffectMaterial::Sidebar,
                    None,
                    None,
                );
            }
        }
        // Transparent: the portable reduced-alpha surface carries the look (no
        // native blur). Opaque: no effect at all.
        c0pl4nd_core::config::WindowMode::Transparent
        | c0pl4nd_core::config::WindowMode::Opaque => {}
    }
}

#[cfg(test)]
mod resize_tests {
    //! Regression guard for the frameless edge-resize hit-testing (#24). The
    //! interior MUST NOT be a resize zone (that is what would make the resize
    //! overlay eat tab / caption / pane clicks); edges/corners must map to the
    //! right direction. Pure, so it runs every CI build and pins the geometry
    //! across window sizes. Ported from the SCR1B3 sibling app. The OS resize
    //! itself (`BeginResize` + `stop_dragging`) is OS-level and not headless-
    //! testable; this pins the pure hit-test that drives it.
    use super::resize_dir_at;
    use egui::{pos2, Rect, ResizeDirection as D};

    fn win() -> Rect {
        Rect::from_min_max(pos2(0.0, 0.0), pos2(1000.0, 700.0))
    }

    #[test]
    fn interior_is_never_a_resize_zone() {
        assert_eq!(resize_dir_at(pos2(500.0, 350.0), win(), 6.0, 12.0), None);
        // A representative titlebar position — must NOT be grabbed as a resize.
        assert_eq!(resize_dir_at(pos2(574.0, 48.0), win(), 6.0, 12.0), None);
    }

    #[test]
    fn edges_map_to_their_direction() {
        assert_eq!(
            resize_dir_at(pos2(500.0, 1.0), win(), 6.0, 12.0),
            Some(D::North)
        );
        assert_eq!(
            resize_dir_at(pos2(500.0, 699.0), win(), 6.0, 12.0),
            Some(D::South)
        );
        assert_eq!(
            resize_dir_at(pos2(1.0, 350.0), win(), 6.0, 12.0),
            Some(D::West)
        );
        assert_eq!(
            resize_dir_at(pos2(999.0, 350.0), win(), 6.0, 12.0),
            Some(D::East)
        );
    }

    #[test]
    fn corners_take_priority_over_edges() {
        assert_eq!(
            resize_dir_at(pos2(2.0, 2.0), win(), 6.0, 12.0),
            Some(D::NorthWest)
        );
        assert_eq!(
            resize_dir_at(pos2(998.0, 2.0), win(), 6.0, 12.0),
            Some(D::NorthEast)
        );
        assert_eq!(
            resize_dir_at(pos2(2.0, 698.0), win(), 6.0, 12.0),
            Some(D::SouthWest)
        );
        assert_eq!(
            resize_dir_at(pos2(998.0, 698.0), win(), 6.0, 12.0),
            Some(D::SouthEast)
        );
        // On the top edge but within the corner band of the left side → NW.
        assert_eq!(
            resize_dir_at(pos2(8.0, 1.0), win(), 6.0, 12.0),
            Some(D::NorthWest)
        );
    }

    #[test]
    fn outside_the_window_is_none() {
        assert_eq!(resize_dir_at(pos2(-5.0, 350.0), win(), 6.0, 12.0), None);
        assert_eq!(resize_dir_at(pos2(500.0, 800.0), win(), 6.0, 12.0), None);
    }
}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
#[cfg(test)]
mod config_load_tests {
    //! F5-2: a present-but-broken config file must surface an error (so the host
    //! can toast it), an absent file must NOT, and a valid file parses cleanly.
    use super::load_config_from;

    #[test]
    fn absent_config_yields_defaults_with_no_error() {
        let (cfg, err) = load_config_from(None);
        assert_eq!(cfg, c0pl4nd_core::Config::default());
        assert!(
            err.is_none(),
            "an absent config is normal — no error surfaced"
        );
    }

    #[test]
    fn corrupt_config_yields_defaults_with_a_surfaced_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "this is = not valid toml [[[").unwrap();
        let (cfg, err) = load_config_from(Some(path));
        assert_eq!(
            cfg,
            c0pl4nd_core::Config::default(),
            "falls back to defaults"
        );
        assert!(
            err.is_some(),
            "a present-but-invalid config MUST surface an error for the toast"
        );
        assert!(err.unwrap().to_lowercase().contains("config"));
    }

    #[test]
    fn valid_config_parses_with_no_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "theme = \"ghost-paper\"\n").unwrap();
        let (cfg, err) = load_config_from(Some(path));
        assert_eq!(cfg.theme, "ghost-paper");
        assert!(err.is_none());
    }
}
