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
    /// Whether the settings window is open.
    settings_open: bool,
    /// Recently-run commands, surfaced by the command palette for quick
    /// find/run. Captured best-effort from typed input (committed on Enter).
    cmd_history: c0pl4nd_core::command_history::CommandHistory,
    /// Accumulator for the line currently being typed in the focused pane.
    /// Committed to `cmd_history` on Enter, reset on focus change. Best-effort:
    /// it models printable text + Backspace, not full shell line-editing.
    input_line: String,
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
        let mut app = Self::bootstrap_with(load_config());
        // Install the chrome icon fonts AND the user's configured monospace
        // family + fallbacks (loaded from the system font DB and prepended to
        // `FontFamily::Monospace`), so the very first frame already renders the
        // grid in the chosen font. Done after the config load so `app.config.font`
        // is the source of truth.
        install_chrome_fonts(&cc.egui_ctx, &app.config.font);
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
    /// headless `egui_kittest` tests (which run without a window). Spawns a live
    /// [`PaneTerm`] for each initial pane (a failed spawn degrades to an error
    /// label, never a panic).
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
        let mut terms = HashMap::new();
        for pid in &initial {
            terms.insert(*pid, PaneTerm::spawn(theme.clone(), SPAWN_COLS, SPAWN_ROWS));
        }
        Self {
            config,
            theme,
            grid_tree,
            terms,
            pane_alloc,
            focused_pane,
            pinned: HashSet::new(),
            last_focused_size: None,
            shell_profiles: shells::detect_profiles(),
            active_shell: 0,
            fonts_installed: false,
            applied_font_family: String::new(),
            settings_open: false,
            cmd_history: c0pl4nd_core::command_history::CommandHistory::default(),
            input_line: String::new(),
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
            None => PaneTerm::spawn(theme, SPAWN_COLS, SPAWN_ROWS),
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
    /// title was actually shortened.
    fn cap_tab_title(raw: &str) -> String {
        let trimmed = raw.trim();
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
        theme: &c0pl4nd_core::Theme,
        font_size: f32,
        line_height_px: f32,
        cursor_cfg: c0pl4nd_core::config::CursorConfig,
        effects: c0pl4nd_core::config::EffectsConfig,
        padding: f32,
        bg_alpha: u8,
        search: Option<SearchHighlight<'_>>,
        links: &[(CellSpan, String)],
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

        PaneBodyOutcome {
            drag_started: resp.drag_started(),
            clicked: resp.clicked(),
            size: rect.size(),
            opened_url,
        }
    }

    /// Forward this frame's keyboard + paste events to the FOCUSED pane's PTY,
    /// using the SHARED core key encoder. Consumes Tab/arrows so egui does not
    /// steal them for widget navigation (recon dossier §5.1). Called once per
    /// frame. Returns the bytes forwarded (for tests that drive the real input
    /// path and assert what reached the PTY).
    fn forward_input_to_focused(&mut self, ctx: &egui::Context) -> Vec<u8> {
        use c0pl4nd_core::term::{KeyModifiers, LogicalKey};

        // Collect input events under the immutable input borrow first, THEN
        // mutate the PTY (egui forbids re-entrant input borrows).
        let mut keys: Vec<(LogicalKey, KeyModifiers)> = Vec::new();
        let mut pastes: Vec<String> = Vec::new();
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
                        keys.push((LogicalKey::Text(t.clone()), mods));
                    }
                    egui::Event::Paste(s) => pastes.push(s.clone()),
                    egui::Event::Key {
                        key,
                        pressed: true,
                        modifiers,
                        ..
                    } => {
                        let m = KeyModifiers {
                            ctrl: modifiers.ctrl,
                            alt: modifiers.alt,
                            shift: modifiers.shift,
                            logo: modifiers.command || modifiers.mac_cmd,
                        };
                        if let Some(lk) = egui_key_to_logical(*key, m) {
                            keys.push((lk, m));
                        }
                    }
                    _ => {}
                }
            }
        });

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
            for (lk, m) in &keys {
                forwarded.extend(term.forward_key(lk, *m));
            }
            for s in &pastes {
                term.write_bytes(s.as_bytes());
                forwarded.extend_from_slice(s.as_bytes());
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
        for (lk, _m) in &keys {
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
                    self.cmd_history.record(line);
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
            let theme = &self.theme;
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
                    theme,
                    font_size,
                    line_height_px,
                    cursor_cfg,
                    effects,
                    padding,
                    bg_alpha,
                    search,
                    links,
                );
                if outcome.clicked {
                    clicked = Some(pid);
                }
                if let Some(url) = outcome.opened_url {
                    opened_url = Some(url);
                }
                if pid == focused {
                    focused_size = Some((outcome.size.x, outcome.size.y));
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
        if let Some(s) = focused_size {
            self.last_focused_size = Some(s);
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
        let outcome = settings::show(ctx, &mut self.config, &mut open, colors);
        self.settings_open = open;

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
                    let _ = self.config.save_to(&path);
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

    /// The recorded command history, most-recent-first. Observation accessor for
    /// the interaction tests (asserts typed-then-Enter lines were captured).
    #[allow(dead_code)]
    pub fn command_history_entries(&self) -> Vec<String> {
        self.cmd_history.entries().map(str::to_string).collect()
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
                let _ = self.config.save_to(&path);
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
        // Live font apply: when the user changes the Family (or a Fallback) in
        // settings, the configured font stack changed since the last install —
        // re-install it THIS frame so the new typeface shows without a relaunch.
        // The `applied_font_family` key folds the family + fallbacks into one
        // string so the (expensive) re-install runs ONLY on an actual change,
        // never every frame.
        else {
            let want = font_apply_key(&self.config.font);
            if want != self.applied_font_family {
                install_chrome_fonts(ctx, &self.config.font);
                self.applied_font_family = want;
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

        // 0b) route this frame's input. When the palette is open, its navigation
        //     keys (↑/↓/Enter/Esc) are consumed here and the typed query is
        //     captured by the palette's focused TextEdit — NOT forwarded to the
        //     PTY. Otherwise keyboard/paste goes to the FOCUSED pane's PTY BEFORE
        //     the panels, so the keystrokes reach the PTY whose grid this same
        //     frame then snapshots (the load-bearing "typing reaches the PTY and
        //     the grid updates" round-trip).
        if self.palette_open {
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
        //     widget sitting at the very edge get its click.
        handle_frameless_resize(ctx);

        // Theme-derived chrome surface palette — the titlebar / tab strip /
        // status bar / central pane / settings window all follow the active
        // terminal theme through these (a light theme flips the whole chrome
        // light, a dark one dark). The wordmark keeps its fixed brand accent.
        let colors = theme::ChromeColors::from_theme(&self.theme);

        // 1) custom titlebar + tab strip. Fixed height so the drag region below
        //    is exactly the bar (not the whole remaining column), and so the
        //    caption-cluster geometry is stable.
        let actions = egui::TopBottomPanel::top("titlebar")
            .exact_height(40.0)
            .frame(egui::Frame::new().fill(colors.panel).inner_margin(6.0))
            .show(ctx, |ui| {
                // Frameless-window move: dragging any EMPTY part of the titlebar
                // moves the window; double-click toggles maximize. Added FIRST so
                // it sits behind the tabs/buttons (egui gives later widgets the
                // click), so only the empty bar area initiates a drag.
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
            .inner;

        // 2) status bar
        egui::TopBottomPanel::bottom("status")
            .frame(egui::Frame::new().fill(colors.panel).inner_margin(4.0))
            .show(ctx, |ui| self.status_bar(ui, colors));

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
        // View-mode toggle (#30): flip the pane shell layout (Grid ⇄ Tabs) and
        // persist it. The disk write is real-window-only (the headless harness
        // observes the in-memory flip; persisting there would pollute the user's
        // real config.toml — the same discipline `settings_window` follows).
        if actions.toggle_view_mode {
            self.config.view_mode = self.config.view_mode.toggled();
            if self.live_window {
                if let Some(path) = c0pl4nd_core::Config::default_path() {
                    let _ = self.config.save_to(&path);
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

        // 6) window color-tint overlay (a subtle full-window wash). Only painted
        //    when the window is effectively translucent AND the user dialled in
        //    a tint strength — mirrors SCR1B3. A solid (opaque) window never
        //    gets washed; the gate keeps the default experience untouched.
        if self.config.effective_translucent() && self.config.tint_strength > 0.0 {
            paint_tint_overlay(ctx, &self.config.tint, self.config.tint_strength);
        }

        // Live terminals: keep repainting so PTY output animates without waiting
        // for an input event — but ONLY in the real window (`live_window`). In the
        // headless `egui_kittest` harness an unconditional `request_repaint`
        // makes `Harness::run` loop until `max_steps` (the UI never settles); the
        // tests there drive frames explicitly with `h.run()` after each input, so
        // they do not need the animation pump.
        if self.live_window {
            ctx.request_repaint();
        }
    }
}

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

/// The vertical gap (POINTS) between CRT scanlines. A faithful CRT reads as
/// thin dark lines every ~2-3 points; pure so the spacing is unit-testable.
const CRT_SCANLINE_GAP: f32 = 3.0;
/// Per-line darkening alpha (0..=255) for a scanline. Subtle — the lines must
/// dim the phosphor, not black the text out — but visible enough to read as a
/// real scanned tube (≈0.22, up from the near-invisible old 0.11; #28).
const CRT_SCANLINE_ALPHA: u8 = 56;

/// The maximum horizontal RGB ghost offset (POINTS) — capped small so a wild
/// config value can never smear the text into illegibility.
const CHROMATIC_MAX_OFFSET: f32 = 3.0;
/// The minimum visible ghost offset (POINTS) once aberration is ON, so the
/// fringing actually READS as RGB separation rather than vanishing (issue #28:
/// "does nothing visible"). At least one whole pixel of separation.
const CHROMATIC_MIN_OFFSET: f32 = 1.0;

/// The horizontal RGB ghost offset (POINTS) for a chromatic-aberration
/// `intensity` (the `config.effects.chromatic_aberration` value). The red ghost
/// draws at `-offset`, the blue ghost at `+offset`; `intensity == 0` ⇒ offset
/// `0` (off). When ON it is floored at [`CHROMATIC_MIN_OFFSET`] (≥1px so the
/// fringe is visible) and capped at [`CHROMATIC_MAX_OFFSET`]. Pure →
/// unit-testable without a GPU.
fn chromatic_offset(intensity: f32) -> f32 {
    if !intensity.is_finite() || intensity <= 0.0 {
        return 0.0;
    }
    intensity.clamp(CHROMATIC_MIN_OFFSET, CHROMATIC_MAX_OFFSET)
}

/// The alpha (0..=255) of each RGB ghost for a chromatic-aberration
/// `intensity`. Scales with intensity so a faint ghost at low intensity grows
/// to a stronger (but never opaque) fringe — bumped to the 100..=140 band
/// (issue #28: the old 60..=120 was too faint to see) so the fringing is
/// clearly visible while the crisp main glyph still dominates. `intensity == 0`
/// ⇒ alpha `0` (no ghost).
fn chromatic_ghost_alpha(intensity: f32) -> u8 {
    let i = chromatic_offset(intensity);
    if i <= 0.0 {
        return 0;
    }
    // 100 at the 1.0 floor, scaling to 140 at the 3.0 cap.
    let t = (i - CHROMATIC_MIN_OFFSET) / (CHROMATIC_MAX_OFFSET - CHROMATIC_MIN_OFFSET);
    (100.0 + 40.0 * t).clamp(0.0, 140.0).round() as u8
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

/// The number of evenly-spaced [`CRT_SCANLINE_GAP`]-point dark scan lines that
/// fill a content `rect` of the given `height`. Pure and GPU-free so the line
/// geometry is unit-testable without a painter: the line at index `i` sits at
/// `top + i * CRT_SCANLINE_GAP`, and the count is exactly the number of those
/// that fall inside `[top, bottom)`. Issue #28: the prior implementation painted
/// a tinted vignette BOX around the frame instead of real scan lines across the
/// whole content; this is the geometry for the real horizontal lines.
fn scanline_count(height: f32) -> usize {
    if !height.is_finite() || height <= 0.0 {
        return 0;
    }
    // Lines at y = 0, GAP, 2*GAP, … strictly below `height`.
    (height / CRT_SCANLINE_GAP).ceil() as usize
}

/// Paint REAL CRT scan lines across the WHOLE pane content `rect` (issue #28).
/// Thin (1px) dark translucent horizontal rows every [`CRT_SCANLINE_GAP`]
/// points dim the phosphor like a scanned tube — drawn over the entire content,
/// NOT as a vignette box around the frame (the old wrong behaviour). GPU-free
/// (egui `hline` primitives only) and only invoked by the caller when
/// `crt_scanlines` is on, so it is strictly zero-cost when the effect is
/// disabled. The caller's `painter_at(rect)` clip keeps every line inside the
/// pane. A 1080p pane is ~360 thin lines — one cheap primitive batch.
fn paint_crt_scanlines(painter: &egui::Painter, rect: egui::Rect) {
    let line_col = egui::Color32::from_rgba_unmultiplied(0, 0, 0, CRT_SCANLINE_ALPHA);
    let lines = scanline_count(rect.height());
    for i in 0..lines {
        let y = rect.top() + i as f32 * CRT_SCANLINE_GAP;
        painter.hline(rect.x_range(), y, egui::Stroke::new(1.0, line_col));
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
/// per-cell colour runs from [`PaneTerm::grid_spans`]. This is the single,
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

    // Group the per-cell colour runs into ROWS (the `grid_spans` stream ends each
    // row with a `"\n"` run). Each row becomes one galley, painted at
    // `origin.y + row_idx * ch`.
    let rows: Vec<Vec<ColorRun>> = match term.grid_spans() {
        Some(runs) if !runs.is_empty() => split_runs_into_rows(runs),
        _ => {
            // No colour runs (e.g. dead session mid-frame): mono fallback so the
            // pane is never blank. One row per text line, all in the default fg.
            term.grid_text()
                .unwrap_or_default()
                .lines()
                .map(|line| vec![(line.to_string(), default_fg)])
                .collect()
        }
    };

    let ghost_offset = chromatic_offset(effects.chromatic_aberration);
    let ghost_alpha = chromatic_ghost_alpha(effects.chromatic_aberration);
    for (row_idx, runs) in rows.iter().enumerate() {
        let row_origin = egui::pos2(origin.x, origin.y + row_idx as f32 * ch);
        // --- chromatic aberration (research §2 + §2(b) edge-weighting): re-draw
        // the row's glyphs with R/B ghosts at ±offset BEHIND the crisp pass.
        // The offset is EDGE-WEIGHTED by the row's vertical position so the
        // fringing is stronger toward the top/bottom of the pane and near-zero
        // at the middle — the authentic CRT lens falloff. Zero-cost when the
        // setting is 0.0 (offset == 0 ⇒ skipped entirely).
        if ghost_offset > 0.0 {
            let row_y = row_origin.y;
            let off =
                chromatic_edge_weighted_offset(ghost_offset, row_y, rect.top(), rect.bottom());
            paint_row_galley(
                painter,
                row_origin + egui::vec2(-off, 0.0),
                runs,
                &font,
                Some(egui::Color32::from_rgba_unmultiplied(
                    255,
                    40,
                    40,
                    ghost_alpha,
                )),
                default_fg,
            );
            paint_row_galley(
                painter,
                row_origin + egui::vec2(off, 0.0),
                runs,
                &font,
                Some(egui::Color32::from_rgba_unmultiplied(
                    40,
                    80,
                    255,
                    ghost_alpha,
                )),
                default_fg,
            );
        }
        // Crisp main pass in the runs' real colours, on top of any ghosts.
        paint_row_galley(painter, row_origin, runs, &font, None, default_fg);
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

    // --- CRT scanlines + vignette (research §2): the LAST thing painted over
    // this pane's grid, so the lines dim the glyphs + cursor uniformly. Drawn
    // only when the setting is on (strictly zero-cost otherwise).
    if effects.crt_scanlines {
        paint_crt_scanlines(painter, rect);
    }
}

/// Split a flat colour-run stream (as produced by [`PaneTerm::grid_spans`],
/// which terminates each grid row with a `"\n"` run) into per-row run lists. The
/// newline marker runs are dropped; an embedded newline inside a run's text
/// (defensive — `grid_spans` does not produce these, but a fallback might) also
/// splits the row. Pure → unit-testable without a painter.
fn split_runs_into_rows(runs: Vec<ColorRun>) -> Vec<Vec<ColorRun>> {
    let mut rows: Vec<Vec<ColorRun>> = vec![Vec::new()];
    for (text, color) in runs {
        // A run is usually either a pure `"\n"` row terminator or newline-free
        // glyph text; handle the general case by splitting on '\n'.
        let mut parts = text.split('\n').peekable();
        while let Some(part) = parts.next() {
            if !part.is_empty() {
                rows.last_mut()
                    .expect("seeded with one row")
                    .push((part.to_string(), color));
            }
            // Every '\n' boundary (i.e. every gap BETWEEN parts) starts a new row.
            if parts.peek().is_some() {
                rows.push(Vec::new());
            }
        }
    }
    // A trailing newline leaves an empty final row; drop it so a blank line at
    // the end does not add phantom vertical space.
    if rows.last().is_some_and(Vec::is_empty) {
        rows.pop();
    }
    rows
}

/// Paint one grid row's colour runs as a single galley at `pos`. When
/// `override_color` is `Some`, every run is drawn in that colour (the R/B
/// chromatic-aberration ghost passes); when `None`, each run keeps its real SGR
/// colour (the crisp main pass). `default_fg` is the galley's fallback colour.
fn paint_row_galley(
    painter: &egui::Painter,
    pos: egui::Pos2,
    runs: &[ColorRun],
    font: &egui::FontId,
    override_color: Option<egui::Color32>,
    default_fg: (u8, u8, u8),
) {
    if runs.is_empty() {
        return;
    }
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
    let galley = painter.layout_job(job);
    painter.galley(
        pos,
        galley,
        egui::Color32::from_rgb(default_fg.0, default_fg.1, default_fg.2),
    );
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

/// Load the persisted user config from its canonical path, falling back to
/// defaults when it is absent or unreadable. Without this the egui app started
/// from `Config::default()` every launch, so on-disk settings (theme, opacity,
/// font, cursor, transparency, update prefs) the settings panel WROTE never
/// took effect across launches — the same bug the legacy binary's `main` had
/// already fixed for itself. Pure `core` APIs, so it is available in every
/// binary that includes this module (incl. the `#[path]`-included test bins).
fn load_config() -> c0pl4nd_core::Config {
    match c0pl4nd_core::Config::default_path().filter(|p| p.exists()) {
        Some(p) => std::fs::read_to_string(&p)
            .map_err(|e| e.to_string())
            .and_then(|s| c0pl4nd_core::Config::from_toml(&s, &p).map_err(|e| e.to_string()))
            .unwrap_or_else(|e| {
                eprintln!("c0pl4nd: failed to load config {p:?}: {e}; using defaults");
                c0pl4nd_core::Config::default()
            }),
        None => c0pl4nd_core::Config::default(),
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
    let base = base_font_definitions();
    // Fast path: nothing custom to load (default config / built-in choice) — set
    // the icon base and skip the expensive system-font enumeration entirely.
    let needs_load = !fonts::is_builtin_family(&font.family)
        || font.fallback.iter().any(|f| !fonts::is_builtin_family(f));
    if !needs_load {
        ctx.set_fonts(base);
        return;
    }
    let mut db = fontdb::Database::new();
    db.load_system_fonts();
    let (defs, _loaded) = fonts::build_font_definitions(base, &db, &font.family, &font.fallback);
    ctx.set_fonts(defs);
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

/// Per-mode CEILING (fraction) on the translucent panel alpha for the native
/// DWM-backdrop modes (Glass / Mica / Vibrancy). This is the load-bearing fix
/// for "all the modes look identical / nothing is see-through" (#27): the
/// window's clear-color is already a transparent hole (`window_clear_color`
/// returns `[0,0,0,0]` for these modes), but the pane + central panel fills
/// were painted at the raw `opacity` alpha — and the DEFAULT opacity is `1.0`,
/// so every native-blur mode re-filled the hole with a fully OPAQUE quad and
/// collapsed to a flat tint. Capping the fill alpha guarantees the backdrop
/// shows through even at opacity 1.0, and each mode's distinct ceiling makes
/// them visibly different: Acrylic (Glass) blurs strongly through the biggest
/// hole, Mica tints the wallpaper through a subtle one, Vibrancy is a plain
/// reduced-alpha see-through. Research §3 (the Win11 backdrop table). `None`
/// (Transparent / Opaque) means "no extra ceiling — the opacity slider alone
/// drives the alpha", because Transparent has no DWM hole to preserve.
fn translucent_alpha_ceiling(mode: c0pl4nd_core::config::WindowMode) -> Option<f32> {
    use c0pl4nd_core::config::WindowMode;
    match mode {
        // Acrylic: the strong live-blur backdrop — keep the largest hole.
        WindowMode::Glass => Some(0.35),
        // Mica: subtle wallpaper tint — a small hole reads as a faint wash.
        WindowMode::Mica => Some(0.45),
        // Vibrancy: plain reduced-alpha see-through, no DWM blur — a mid hole.
        WindowMode::Vibrancy => Some(0.55),
        // Transparent (portable, no DWM backdrop) + Opaque: slider-only.
        WindowMode::Transparent | WindowMode::Opaque => None,
    }
}

/// The alpha (0..=255) to paint the pane grid background (and the central panel
/// fill) with, for the current config:
///
/// * **Opaque** (master toggle off, or `Opaque` mode): `255` — a solid fill so
///   the desktop never bleeds through. The unchanged, safe default.
/// * **Translucent** (`effective_translucent()`): the `opacity` slider folded
///   into a 0..=255 alpha (floored at [`TRANSLUCENT_ALPHA_FLOOR`] so the grid
///   stays readable), then CAPPED by the per-mode
///   [`translucent_alpha_ceiling`] for the native-blur modes so the DWM
///   backdrop hole is never re-filled opaque. An opaque pane fill here is
///   exactly why transparency previously "did nothing" / "looked identical
///   across modes": at the default opacity `1.0` it covered the transparent
///   clear-color and the DWM backdrop (#27).
///
/// Pure (`&Config`) so the transparency wiring is unit-testable without a
/// window. Mirrors SCR1B3's translucent-panel pattern (research §1c).
fn pane_bg_alpha(config: &c0pl4nd_core::Config) -> u8 {
    if !config.effective_translucent() {
        return 255;
    }
    let mut a = config.opacity.clamp(TRANSLUCENT_ALPHA_FLOOR, 1.0);
    if let Some(ceiling) = translucent_alpha_ceiling(config.window_mode) {
        a = a.min(ceiling);
    }
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
mod tests {
    use super::*;

    #[test]
    fn bootstrap_opens_with_initial_panes() {
        let app = C0pl4ndApp::bootstrap();
        assert_eq!(app.pane_count(), INITIAL_PANES);
        assert!(!app.settings_is_open());
    }

    /// A short title passes through unchanged (trimmed); a title longer than the
    /// cap is shortened to exactly `MAX_TAB_TITLE` chars plus a `…` suffix, so a
    /// verbose program title cannot blow out the tab strip.
    #[test]
    fn cap_tab_title_trims_and_truncates() {
        assert_eq!(
            C0pl4ndApp::cap_tab_title("  vim  "),
            "vim",
            "a short title is trimmed and passed through verbatim"
        );
        // Exactly at the cap → no ellipsis.
        let at_cap: String = "a".repeat(C0pl4ndApp::MAX_TAB_TITLE);
        assert_eq!(
            C0pl4ndApp::cap_tab_title(&at_cap),
            at_cap,
            "a title exactly at the cap is not truncated"
        );
        // One over the cap → truncated to MAX_TAB_TITLE chars + ellipsis.
        let over_cap: String = "b".repeat(C0pl4ndApp::MAX_TAB_TITLE + 5);
        let capped = C0pl4ndApp::cap_tab_title(&over_cap);
        assert_eq!(
            capped.chars().count(),
            C0pl4ndApp::MAX_TAB_TITLE + 1,
            "an over-length title keeps MAX_TAB_TITLE chars plus one ellipsis char"
        );
        assert!(
            capped.ends_with('…') && capped.starts_with('b'),
            "the truncated title keeps the leading chars and ends with an ellipsis"
        );
    }

    /// Two panes whose shells set the SAME OSC title still get DISTINCT
    /// accessible tab labels. The visible tab text may collide (real terminals
    /// allow two same-named tabs), but the accessibility tree — and the
    /// `get_by_label` lookups the interaction tests rely on — must never have
    /// two nodes sharing one name. Every label is anchored on the unique
    /// `pane {id}`. Regression guard for the Windows-CI failure where both
    /// bootstrap shells set the same cwd title and the tab lookup went ambiguous.
    #[test]
    fn tab_a11y_label_is_unique_even_when_titles_collide() {
        // Identical display title for two different panes → distinct labels.
        let a = C0pl4ndApp::tab_a11y_label(PaneId(0), "make");
        let b = C0pl4ndApp::tab_a11y_label(PaneId(1), "make");
        assert_ne!(
            a, b,
            "colliding titles must still yield distinct accessible labels"
        );
        assert_eq!(a, "make (pane 0)");
        assert_eq!(b, "make (pane 1)");
        // WCAG 2.5.3 "Label in Name": the visible title is a prefix of the label.
        assert!(
            a.starts_with("make"),
            "the title leads the accessible label"
        );
        // The untitled fallback is already unique → not doubled into
        // "pane 2 (pane 2)".
        assert_eq!(
            C0pl4ndApp::tab_a11y_label(PaneId(2), "pane 2"),
            "pane 2",
            "the bare pane-id fallback carries no redundant suffix"
        );
    }

    /// A pane whose running program has not set an OSC title falls back to the
    /// generic `pane {id}` label — so untitled panes read exactly as before this
    /// feature, keeping the visual-order tab strip stable. (A fresh bootstrap
    /// shell has not emitted a title escape, so every label is the fallback.)
    #[test]
    fn pane_titles_fall_back_to_pane_id_without_osc_title() {
        let app = C0pl4ndApp::bootstrap();
        let titles = app.pane_titles();
        assert_eq!(titles.len(), app.pane_count());
        for (id, label) in titles {
            assert_eq!(
                label,
                format!("pane {}", id.raw()),
                "an untitled pane must use the pane-id fallback label"
            );
        }
    }

    #[test]
    fn grid_text_origin_insets_by_padding() {
        // The grid text origin is the pane top-left inset by the padding on
        // BOTH axes; a larger padding moves it further into the pane.
        let rect = egui::Rect::from_min_size(egui::pos2(10.0, 20.0), egui::vec2(400.0, 300.0));
        assert_eq!(
            grid_text_origin(rect, 8.0),
            egui::pos2(18.0, 28.0),
            "padding must inset the origin from the pane top-left on both axes"
        );
        let near = grid_text_origin(rect, 4.0);
        let far = grid_text_origin(rect, 16.0);
        assert!(
            far.x > near.x && far.y > near.y,
            "a larger padding must move the origin further into the pane"
        );
    }

    #[test]
    fn grid_text_origin_clamps_negative_padding() {
        // A bad (negative) config can never push the origin outside the pane —
        // it clamps to the pane top-left (zero inset).
        let rect = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(100.0, 100.0));
        assert_eq!(
            grid_text_origin(rect, -5.0),
            rect.left_top(),
            "negative padding must clamp to zero inset (origin == pane top-left)"
        );
    }

    #[test]
    fn cell_at_pos_maps_pointer_to_grid_cell() {
        // Origin (10,20), 8×16-point cells. A point inside cell (row 2, col 3)
        // maps to it; a point above/left of the origin is off-grid → None.
        let origin = egui::pos2(10.0, 20.0);
        let (cw, ch) = (8.0, 16.0);
        // Cell (2,3) spans x∈[34,42), y∈[52,68); pick a point inside.
        assert_eq!(
            cell_at_pos(egui::pos2(36.0, 60.0), origin, cw, ch),
            Some((2, 3))
        );
        // Exactly on the origin → cell (0,0).
        assert_eq!(cell_at_pos(origin, origin, cw, ch), Some((0, 0)));
        // Left of / above the grid → off-grid.
        assert_eq!(cell_at_pos(egui::pos2(9.0, 60.0), origin, cw, ch), None);
        assert_eq!(cell_at_pos(egui::pos2(36.0, 19.0), origin, cw, ch), None);
        // Degenerate cell size never divides by zero.
        assert_eq!(cell_at_pos(egui::pos2(36.0, 60.0), origin, 0.0, ch), None);
    }

    #[test]
    fn link_url_at_cell_matches_half_open_span() {
        // One link on row 0 spanning cols [4, 25). A col inside hits; the
        // exclusive end col does not; a different row does not.
        let links = vec![(
            CellSpan {
                line: 0,
                col_start: 4,
                col_end: 25,
            },
            "https://example.com".to_string(),
        )];
        assert_eq!(link_url_at_cell(&links, 0, 4), Some("https://example.com"));
        assert_eq!(link_url_at_cell(&links, 0, 24), Some("https://example.com"));
        assert_eq!(
            link_url_at_cell(&links, 0, 25),
            None,
            "end col is exclusive"
        );
        assert_eq!(link_url_at_cell(&links, 0, 3), None, "before the span");
        assert_eq!(link_url_at_cell(&links, 1, 10), None, "wrong row");
    }

    #[test]
    fn ctrl_click_on_a_seeded_url_records_it() {
        // Drive the SAME span-build + hit-test path a real Ctrl-click uses, over a
        // KNOWN seeded grid (PTY-independent). The URL "https://example.com" sits
        // at byte 4 on row 0 → char cols [4, 23).
        let mut app = C0pl4ndApp::bootstrap();
        app.test_seed_focused_grid("see https://example.com here\nplain line, no link");
        assert_eq!(app.last_opened_url(), None, "nothing opened yet");

        // A cell inside the URL span opens it and records it.
        let opened = app.test_open_url_at_cell(0, 8);
        assert_eq!(opened.as_deref(), Some("https://example.com"));
        assert_eq!(
            app.last_opened_url().as_deref(),
            Some("https://example.com"),
            "a Ctrl-click on a URL must record it as opened"
        );

        // A cell on the no-link line opens nothing (and does not clobber the
        // last-opened record).
        assert_eq!(app.test_open_url_at_cell(1, 2), None);
        assert_eq!(
            app.last_opened_url().as_deref(),
            Some("https://example.com"),
            "clicking a non-URL cell must not open or change the record"
        );
    }

    #[test]
    fn update_notice_surfaces_as_a_toast_and_is_recorded() {
        let mut app = C0pl4ndApp::bootstrap();
        assert_eq!(app.last_update_notice(), None, "no notice at start");
        assert!(app.toast.is_none(), "no toast at start");
        app.apply_update_notice("C0PL4ND 9.9.9 is available".to_string());
        assert_eq!(
            app.last_update_notice().as_deref(),
            Some("C0PL4ND 9.9.9 is available"),
            "the notice is recorded (observable)"
        );
        assert_eq!(
            app.toast.as_deref(),
            Some("C0PL4ND 9.9.9 is available"),
            "the notice is shown as a transient status-bar toast"
        );
    }

    #[test]
    fn launch_check_channel_polls_into_a_toast_then_stops() {
        // Simulates the background launch check: a notice sent on the attached
        // channel is picked up by `poll_update_check` (called each frame) and
        // surfaced; the channel is then dropped (one-shot).
        let mut app = C0pl4ndApp::bootstrap();
        let (tx, rx) = std::sync::mpsc::channel();
        app.attach_update_check(rx);
        assert!(app.update_rx.is_some(), "channel attached");
        // Nothing sent yet → poll is a no-op.
        app.poll_update_check();
        assert_eq!(app.last_update_notice(), None);
        // The background thread finds an update and sends one notice.
        tx.send("C0PL4ND 2.0.0 is available".to_string()).unwrap();
        app.poll_update_check();
        assert_eq!(
            app.last_update_notice().as_deref(),
            Some("C0PL4ND 2.0.0 is available"),
            "a received notice surfaces via the per-frame poll"
        );
        assert!(
            app.update_rx.is_none(),
            "the check is one-shot: the receiver is dropped after delivery"
        );
    }

    #[test]
    fn split_increases_pane_count() {
        let mut app = C0pl4ndApp::bootstrap();
        let before = app.pane_count();
        app.split(egui_tiles::LinearDir::Horizontal);
        assert_eq!(app.pane_count(), before + 1);
    }

    #[test]
    fn split_refuses_above_cap() {
        let mut app = C0pl4ndApp::bootstrap();
        while app.pane_count() < grid::MAX_PANES {
            app.split(egui_tiles::LinearDir::Horizontal);
        }
        assert_eq!(app.pane_count(), grid::MAX_PANES);
        app.split(egui_tiles::LinearDir::Vertical);
        assert_eq!(app.pane_count(), grid::MAX_PANES, "cap must hold");
        assert!(app.toast.is_some());
    }

    /// Regression for "the existing terminal goes black after I close one and
    /// open a new one". An orphaned pane (in storage but unreachable from the
    /// tree root) is COUNTED by `pane_count` but rendered NOWHERE — i.e. black.
    /// After close+new-terminal, EVERY pane must be reachable from the root.
    #[test]
    fn close_then_new_terminal_keeps_every_pane_reachable() {
        fn reachable(tree: &egui_tiles::Tree<Pane>) -> Vec<PaneId> {
            fn walk(tree: &egui_tiles::Tree<Pane>, id: egui_tiles::TileId, out: &mut Vec<PaneId>) {
                match tree.tiles.get(id) {
                    Some(egui_tiles::Tile::Pane(p)) => out.push(p.pane_id),
                    Some(egui_tiles::Tile::Container(c)) => {
                        for ch in c.children() {
                            walk(tree, *ch, out);
                        }
                    }
                    None => {}
                }
            }
            let mut out = Vec::new();
            if let Some(root) = tree.root {
                walk(tree, root, &mut out);
            }
            out
        }

        let mut app = C0pl4ndApp::bootstrap(); // 1 pane (id 0)
        app.new_terminal(); // → 0, 1
        assert_eq!(app.pane_count(), 2);
        app.close_pane(app.focused_pane); // close the new one
        assert_eq!(app.pane_count(), 1, "back to one pane after close");
        app.new_terminal(); // → survivor + a fresh pane
        assert_eq!(app.pane_count(), 2, "two panes after re-adding");

        let reachable = reachable(&app.grid_tree);
        assert_eq!(
            reachable.len(),
            app.pane_count(),
            "every pane must be reachable from the root after close+new (an \
             orphaned pane renders black); reachable={reachable:?}"
        );
    }

    /// Fast-close contract: `prepare_shutdown` must reap EVERY live terminal so
    /// no PTY child is orphaned after the window closes, while NOT terminating
    /// the process (the real Close handler calls `process::exit(0)` AFTER this;
    /// the test exercises only the cleanup so it does not kill the test runner).
    ///
    /// The config save is real-window-only (`live_window`); `bootstrap()` leaves
    /// it false, so this test deliberately does NOT write the user's real config
    /// (no test pollution) — it asserts the no-orphan child-reaping side effect,
    /// which is the load-bearing correctness guarantee of the fast exit.
    #[test]
    fn prepare_shutdown_reaps_all_terminals_without_exit() {
        let mut app = C0pl4ndApp::bootstrap();
        // Open a couple more panes so there are several live PaneTerms to reap.
        app.new_terminal();
        app.new_terminal();
        assert!(
            app.term_count() > 0,
            "precondition: at least one live terminal before shutdown"
        );
        assert!(
            !app.live_window,
            "bootstrap() is headless: the config save is skipped (no test pollution)"
        );

        // The cleanup the real Close handler runs before process::exit(0).
        app.prepare_shutdown();

        assert_eq!(
            app.term_count(),
            0,
            "every PaneTerm must be dropped (Session::Drop kills its PTY child) \
             so no shell is orphaned after the window closes"
        );
        // Reaching here proves prepare_shutdown returned normally — it did NOT
        // call process::exit (which would abort the test runner).
    }

    // ---- Transparency clear-color (SCR1B3-parity model) ----

    fn cfg_mode(enabled: bool, mode: c0pl4nd_core::config::WindowMode) -> c0pl4nd_core::Config {
        c0pl4nd_core::Config {
            transparency_enabled: enabled,
            window_mode: mode,
            ..Default::default()
        }
    }

    #[test]
    fn clear_color_is_opaque_when_transparency_off() {
        // The default (master off) must clear to a SOLID surface (alpha 1.0) so
        // the desktop never bleeds through — the safe, unchanged default.
        let app = C0pl4ndApp::bootstrap();
        let [_, _, _, a] = window_clear_color(&app.config, &app.theme);
        assert_eq!(a, 1.0, "opaque window clears at full alpha");
    }

    #[test]
    fn clear_color_is_transparent_for_native_blur_modes() {
        // Glass/Mica/Vibrancy want a fully transparent surface so the OS blur
        // backdrop shows through.
        let app = C0pl4ndApp::bootstrap();
        for mode in [
            c0pl4nd_core::config::WindowMode::Glass,
            c0pl4nd_core::config::WindowMode::Mica,
            c0pl4nd_core::config::WindowMode::Vibrancy,
        ] {
            let cfg = cfg_mode(true, mode);
            let [_, _, _, a] = window_clear_color(&cfg, &app.theme);
            assert_eq!(a, 0.0, "native-blur mode {mode:?} clears fully transparent");
        }
    }

    #[test]
    fn clear_color_folds_opacity_into_alpha_for_transparent_mode() {
        // Portable Transparent mode: alpha tracks the opacity slider so the
        // desktop shows through at the chosen strength.
        let app = C0pl4ndApp::bootstrap();
        let mut cfg = cfg_mode(true, c0pl4nd_core::config::WindowMode::Transparent);
        cfg.opacity = 0.6;
        let [_, _, _, a] = window_clear_color(&cfg, &app.theme);
        assert!(
            (a - 0.6).abs() < 1e-6,
            "Transparent mode alpha must equal the opacity slider (got {a})"
        );

        // The 0.05 floor (down from the old 0.30 dead band — #27) is honoured
        // even if a lower opacity slips through, so the slider's travel is live.
        cfg.opacity = 0.01;
        let [_, _, _, a2] = window_clear_color(&cfg, &app.theme);
        assert!(
            (a2 - TRANSLUCENT_ALPHA_FLOOR).abs() < 1e-6,
            "alpha is clamped to the 0.05 floor"
        );
    }

    #[test]
    fn clear_color_master_off_overrides_a_translucent_mode() {
        // A translucent mode with the MASTER toggle off must still clear opaque
        // — the master switch is the single kill-switch every path consults.
        let app = C0pl4ndApp::bootstrap();
        let cfg = cfg_mode(false, c0pl4nd_core::config::WindowMode::Glass);
        let [_, _, _, a] = window_clear_color(&cfg, &app.theme);
        assert_eq!(a, 1.0, "master off forces an opaque clear even for Glass");
    }

    // ---- Line-height row pitch ----

    #[test]
    fn line_height_multiplier_anchors_default_to_one() {
        // The 20.0-px default maps to a 1.0 multiplier (natural spacing), so the
        // default config reproduces the pre-feature row pitch exactly.
        assert!(
            (line_height_multiplier(LINE_HEIGHT_ANCHOR_PX) - 1.0).abs() < 1e-6,
            "the default line-height must yield a 1.0 (natural) pitch multiplier"
        );
        // A larger configured line-height opens the rows up (> 1.0); a smaller
        // one tightens them (< 1.0).
        assert!(line_height_multiplier(40.0) > 1.0, "40px loosens the pitch");
        assert!(
            line_height_multiplier(12.0) < 1.0,
            "12px tightens the pitch"
        );
    }

    #[test]
    fn line_height_multiplier_clamps_and_guards_bad_values() {
        // A degenerate / non-finite config can neither collapse rows nor scatter
        // them: the multiplier is clamped to a sane band, and 0 / negative /
        // non-finite fall back to the natural 1.0.
        assert_eq!(line_height_multiplier(0.0), 1.0, "zero → natural");
        assert_eq!(line_height_multiplier(-5.0), 1.0, "negative → natural");
        assert_eq!(line_height_multiplier(f32::NAN), 1.0, "NaN → natural");
        assert!(
            (line_height_multiplier(1000.0) - 4.0).abs() < 1e-6,
            "a huge line-height clamps to the 4.0 ceiling"
        );
        assert!(
            (line_height_multiplier(1.0) - 0.5).abs() < 1e-6,
            "a tiny line-height clamps to the 0.5 floor"
        );
    }

    #[test]
    fn effective_row_pitch_scales_natural_height_by_the_multiplier() {
        // At the default line-height the pitch equals the natural galley height;
        // doubling the line-height (40px) doubles the pitch; the pitch is never
        // below 1px (a degenerate natural height still yields a drawable row).
        let natural = 16.0;
        assert!(
            (effective_row_pitch(natural, LINE_HEIGHT_ANCHOR_PX) - natural).abs() < 1e-6,
            "default line-height keeps the natural pitch"
        );
        assert!(
            (effective_row_pitch(natural, 40.0) - natural * 2.0).abs() < 1e-3,
            "40px (2× the 20px anchor) doubles the row pitch"
        );
        assert!(
            effective_row_pitch(0.0, LINE_HEIGHT_ANCHOR_PX) >= 1.0,
            "the pitch floors at 1px so a row is always drawable"
        );
    }

    // ---- Grid run → row grouping (per-row galley positioning) ----

    #[test]
    fn split_runs_into_rows_groups_on_newline_terminators() {
        // `grid_spans` ends each row with a "\n" run; the splitter drops those
        // markers and keeps each row's colour runs together.
        let runs = vec![
            ("ab".to_string(), (1, 2, 3)),
            ("cd".to_string(), (4, 5, 6)),
            ("\n".to_string(), (0, 0, 0)),
            ("ef".to_string(), (7, 8, 9)),
            ("\n".to_string(), (0, 0, 0)),
        ];
        let rows = split_runs_into_rows(runs);
        assert_eq!(rows.len(), 2, "two newline-terminated rows");
        assert_eq!(rows[0].len(), 2, "first row keeps both colour runs");
        assert_eq!(rows[0][0].0, "ab");
        assert_eq!(rows[0][1].0, "cd");
        assert_eq!(rows[1][0].0, "ef");
    }

    #[test]
    fn split_runs_into_rows_handles_embedded_newline_and_no_trailing_phantom() {
        // A run carrying an embedded '\n' (defensive) splits into two rows; a
        // trailing newline must NOT leave a phantom empty final row.
        let runs = vec![
            ("foo\nbar".to_string(), (1, 1, 1)),
            ("\n".to_string(), (0, 0, 0)),
        ];
        let rows = split_runs_into_rows(runs);
        assert_eq!(rows.len(), 2, "embedded newline splits into two rows");
        assert_eq!(rows[0][0].0, "foo");
        assert_eq!(rows[1][0].0, "bar");
        assert!(
            rows.last().is_some_and(|r| !r.is_empty()),
            "a trailing newline must not leave an empty phantom row"
        );
    }

    // ---- CRT / chromatic-aberration helpers ----

    #[test]
    fn chromatic_offset_is_zero_when_off_and_clamps_when_wild() {
        // 0.0 (the default) → no ghost offset at all (the OFF fast-path).
        assert_eq!(chromatic_offset(0.0), 0.0, "0 intensity = no aberration");
        assert_eq!(chromatic_offset(-1.0), 0.0, "negative = off");
        assert_eq!(chromatic_offset(f32::NAN), 0.0, "NaN = off");
        // ON → floored at the visible 1px minimum (#28: the fringe must be
        // visible), and a wild value clamps to the 3px cap so the text can never
        // smear into illegibility.
        assert!(
            (chromatic_offset(0.3) - CHROMATIC_MIN_OFFSET).abs() < 1e-6,
            "a low intensity floors at the 1px visible minimum"
        );
        assert!(
            (chromatic_offset(2.0) - 2.0).abs() < 1e-6,
            "mid passes through"
        );
        assert!(
            (chromatic_offset(99.0) - CHROMATIC_MAX_OFFSET).abs() < 1e-6,
            "clamped to the 3px cap"
        );
    }

    #[test]
    fn chromatic_ghost_alpha_scales_with_intensity_and_is_zero_when_off() {
        // OFF → no ghost alpha (so no ghost passes are even drawn).
        assert_eq!(chromatic_ghost_alpha(0.0), 0, "0 intensity = no ghost");
        // ON → visible 100..=140 band (#28: the old 60..=120 was too faint).
        assert_eq!(
            chromatic_ghost_alpha(1.0),
            100,
            "intensity 1.0 → alpha 100 (visible floor)"
        );
        assert_eq!(
            chromatic_ghost_alpha(99.0),
            140,
            "alpha is capped at 140 even for a wild intensity"
        );
        assert!(
            chromatic_ghost_alpha(1.0) <= chromatic_ghost_alpha(2.5),
            "ghost alpha grows with intensity"
        );
        // Every visible ghost is firmly in the readable 100..=140 band.
        for i in [0.5_f32, 1.0, 2.0, 3.0, 9.0] {
            let a = chromatic_ghost_alpha(i);
            assert!((100..=140).contains(&a), "alpha {a} in the visible band");
        }
    }

    #[test]
    fn chromatic_edge_weight_is_zero_at_centre_and_full_at_edge() {
        // Edge-weighting: a glyph at the vertical centre fringes faintly (40% of
        // the base offset), the edges fringe at the full base offset (#28 / §2b).
        let base = 2.0;
        let (lo, hi) = (0.0, 100.0);
        let centre = chromatic_edge_weighted_offset(base, 50.0, lo, hi);
        let edge = chromatic_edge_weighted_offset(base, 100.0, lo, hi);
        assert!(
            (centre - base * 0.4).abs() < 1e-4,
            "centre keeps 40% of the offset (a faint fringe, never fully crisp)"
        );
        assert!(
            (edge - base).abs() < 1e-4,
            "the edge gets the full base offset"
        );
        assert!(edge > centre, "the edge separates more than the centre");
        // OFF / degenerate span → no offset, never NaN.
        assert_eq!(chromatic_edge_weighted_offset(0.0, 5.0, 0.0, 100.0), 0.0);
        assert_eq!(chromatic_edge_weighted_offset(2.0, 5.0, 10.0, 10.0), 2.0);
    }

    #[test]
    fn scanline_count_fills_the_whole_rect_and_is_zero_for_empty() {
        // Real scan lines (#28): the lines fill the WHOLE content height every
        // CRT_SCANLINE_GAP points — not a vignette box. A 300px pane at a 3px
        // gap yields 100 lines.
        assert_eq!(
            scanline_count(300.0),
            (300.0_f32 / CRT_SCANLINE_GAP).ceil() as usize
        );
        assert!(
            scanline_count(300.0) >= 90,
            "a tall pane is covered by many lines, not a 4-edge box"
        );
        // Degenerate heights paint nothing (no panic, no negative loop).
        assert_eq!(scanline_count(0.0), 0, "empty rect → no lines");
        assert_eq!(scanline_count(-5.0), 0, "negative → no lines");
        assert_eq!(scanline_count(f32::NAN), 0, "NaN → no lines");
    }

    // ---- Translucent pane background alpha (the transparency fix) ----

    #[test]
    fn pane_bg_alpha_is_opaque_by_default_and_when_master_off() {
        // The default (transparency off) paints the pane fill fully opaque so the
        // desktop never bleeds through — the safe, unchanged default.
        let app = C0pl4ndApp::bootstrap();
        assert_eq!(
            pane_bg_alpha(&app.config),
            255,
            "default pane fill is opaque"
        );
        // A translucent MODE with the master toggle off is still opaque.
        let cfg = cfg_mode(false, c0pl4nd_core::config::WindowMode::Glass);
        assert_eq!(pane_bg_alpha(&cfg), 255, "master off keeps the pane opaque");
    }

    #[test]
    fn pane_bg_alpha_folds_opacity_when_translucent() {
        use c0pl4nd_core::config::WindowMode;
        // Enabling transparency + a translucent mode makes the pane fill
        // non-opaque, folding the opacity slider into the alpha so the OS blur /
        // desktop shows through at the chosen strength (this is the lever that
        // made transparency visibly "do something"). Use Transparent mode (no
        // per-mode ceiling) so the slider value passes straight through.
        let mut cfg = cfg_mode(true, WindowMode::Transparent);
        cfg.opacity = 0.6;
        let a = pane_bg_alpha(&cfg);
        assert_eq!(
            a,
            (0.6 * 255.0_f32).round() as u8,
            "alpha tracks the opacity slider"
        );
        assert!(a < 255, "a translucent pane fill must be non-opaque");
        // The 0.05 floor (down from the old 0.30 dead band) is honoured so a
        // near-zero opacity can't make the grid invisible (#27).
        cfg.opacity = 0.0;
        assert_eq!(
            pane_bg_alpha(&cfg),
            (TRANSLUCENT_ALPHA_FLOOR * 255.0_f32).round() as u8,
            "alpha is clamped to the 0.05 floor"
        );
    }

    #[test]
    fn native_blur_modes_cap_alpha_so_the_backdrop_is_never_re_filled_opaque() {
        use c0pl4nd_core::config::WindowMode;
        // The #27 root cause: the default opacity is 1.0, so without a per-mode
        // ceiling every native-blur mode painted a FULLY OPAQUE pane fill over
        // the transparent DWM hole — collapsing every mode to a flat tint. Each
        // native-blur mode must cap its fill alpha well below 255 even at
        // opacity 1.0 so the backdrop shows through.
        for mode in [WindowMode::Glass, WindowMode::Mica, WindowMode::Vibrancy] {
            let mut cfg = cfg_mode(true, mode);
            cfg.opacity = 1.0; // the default — the worst case for occlusion
            let a = pane_bg_alpha(&cfg);
            assert!(
                a < 255,
                "{mode:?} must NOT paint an opaque fill at opacity 1.0 (it would \
                 occlude the DWM backdrop and look identical to every other mode)"
            );
            let ceiling = translucent_alpha_ceiling(mode).expect("native mode has a ceiling");
            assert_eq!(
                a,
                (ceiling * 255.0_f32).round() as u8,
                "{mode:?} alpha is capped at its per-mode ceiling"
            );
        }
    }

    #[test]
    fn native_blur_mode_ceilings_make_the_modes_visibly_distinct() {
        use c0pl4nd_core::config::WindowMode;
        // The distinct per-mode ceilings are what make Glass/Mica/Vibrancy look
        // DIFFERENT (the user-visible bug was "they all look identical"). At the
        // default opacity 1.0 each mode resolves to a different fill alpha.
        let alpha = |mode| {
            let mut cfg = cfg_mode(true, mode);
            cfg.opacity = 1.0;
            pane_bg_alpha(&cfg)
        };
        let glass = alpha(WindowMode::Glass);
        let mica = alpha(WindowMode::Mica);
        let vibrancy = alpha(WindowMode::Vibrancy);
        // Glass keeps the biggest hole (lowest fill alpha) for the strong blur;
        // Vibrancy the smallest. All three differ.
        assert!(
            glass < mica && mica < vibrancy,
            "the three native-blur modes must have distinct fill alphas \
             (glass {glass} < mica {mica} < vibrancy {vibrancy})"
        );
    }

    #[test]
    fn opaque_pane_fill_is_byte_identical_regardless_of_mode() {
        use c0pl4nd_core::config::WindowMode;
        // The opaque path must be untouched: master-off, any mode, any opacity →
        // a fully opaque 255 fill (premultiplied == unmultiplied at 255, so the
        // rendered Color32 is byte-identical to the pre-change default).
        for mode in [
            WindowMode::Opaque,
            WindowMode::Glass,
            WindowMode::Mica,
            WindowMode::Vibrancy,
            WindowMode::Transparent,
        ] {
            let mut cfg = cfg_mode(false, mode); // master OFF
            cfg.opacity = 0.2;
            assert_eq!(
                pane_bg_alpha(&cfg),
                255,
                "master-off {mode:?} stays fully opaque"
            );
        }
    }
}
