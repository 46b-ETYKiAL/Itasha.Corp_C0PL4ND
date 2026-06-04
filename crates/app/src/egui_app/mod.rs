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
pub mod grid;
pub mod pane_term;
mod settings;
pub mod shells;
mod theme;

use std::collections::{HashMap, HashSet};

use eframe::egui;

use grid::{count_panes, GridBehavior, Pane, PaneId, PaneIdAllocator};
use pane_term::{CellMetrics, PaneTerm};

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
    /// A transient status-bar message (e.g. "max 6 panes").
    toast: Option<String>,
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

impl C0pl4ndApp {
    /// Build the app inside eframe, applying the brand Visuals + window effect,
    /// and computing the terminal cell metrics from egui's monospace font (the
    /// font the grid is actually drawn with). Marks the app as a live window so
    /// the per-frame repaint pump runs.
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        install_chrome_fonts(&cc.egui_ctx);
        let mut app = Self::bootstrap();
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
    pub fn bootstrap() -> Self {
        let config = c0pl4nd_core::Config::default();
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
            settings_open: false,
            cmd_history: c0pl4nd_core::command_history::CommandHistory::default(),
            input_line: String::new(),
            palette_open: false,
            palette_query: String::new(),
            palette_sel: 0,
            last_palette_run: None,
            toast: None,
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

    /// The exact tab label a pane currently renders — its live OSC title when
    /// the running program set one, else the `pane {id}` fallback. This is the
    /// SAME string [`chrome`](super::chrome) uses for the tab text AND for the
    /// per-tab accessible labels (`pin {label}` / `close {label}`), so an
    /// interaction test can look up a tab by `get_by_label(label)` without
    /// hardcoding a value that the shell's window-title escape would change. The
    /// label is dynamic precisely because the title feature makes it so — a real
    /// shell sets its own title, so a fixed `"pane 0"` literal is no longer a
    /// stable lookup key.
    #[allow(dead_code)]
    pub fn tab_label_for_pane(&self, pane_id: PaneId) -> Option<String> {
        self.pane_titles()
            .into_iter()
            .find(|(id, _)| *id == pane_id)
            .map(|(_, label)| label)
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
        cursor_cfg: c0pl4nd_core::config::CursorConfig,
        padding: f32,
    ) -> PaneBodyOutcome {
        let (rect, resp) =
            ui.allocate_exact_size(ui.available_size(), egui::Sense::click_and_drag());
        let ppp = ui.ctx().pixels_per_point();
        let painter = ui.painter_at(rect);
        // Cell metrics from the SAME monospace font the grid is drawn with, so
        // the PTY's `(cols, rows)` match the rendered glyph size. Measured via a
        // probe galley (`Painter::layout_job`); ppp scales points → physical px
        // to match the `rect * ppp` resize math below.
        let cell_metrics = monospace_cell_metrics(&painter, font_size, ppp);

        // --- background quad (theme bg) + focus ring ---
        let bg = terms
            .get(&pane_id)
            .map(PaneTerm::background_rgb)
            .unwrap_or((18, 18, 18));
        painter.rect_filled(
            rect,
            egui::CornerRadius::same(4),
            egui::Color32::from_rgb(bg.0, bg.1, bg.2),
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
                    &painter, rect, term, font_size, theme, focused, cursor_cfg, pad,
                );
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

        PaneBodyOutcome {
            drag_started: resp.drag_started(),
            clicked: resp.clicked(),
            size: rect.size(),
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

        // Snapshot BEFORE the frame so we can revert a drag that exceeds the cap.
        let pre = self.grid_tree.clone();
        {
            // Disjoint borrows: the closure touches these fields, NOT grid_tree.
            let terms = &mut self.terms;
            let theme = &self.theme;
            let font_size = self.config.font.size;
            let cursor_cfg = self.config.cursor;
            // Read the inner padding LIVE from the config so a Settings change
            // moves the grid inset without a relaunch (it was a hardcoded 4px
            // before). `u16` config → f32 points for the painter.
            let padding = f32::from(self.config.window.padding);
            let mut render_body = |ui: &mut egui::Ui, pid: PaneId| -> bool {
                let outcome = Self::render_pane_body(
                    ui,
                    pid,
                    pid == focused,
                    terms,
                    theme,
                    font_size,
                    cursor_cfg,
                    padding,
                );
                if outcome.clicked {
                    clicked = Some(pid);
                }
                if pid == focused {
                    focused_size = Some((outcome.size.x, outcome.size.y));
                }
                outcome.drag_started
            };
            let mut behavior = GridBehavior {
                titles: &titles,
                render_body: &mut render_body,
                close_requests: &mut closes,
            };
            self.grid_tree.ui(&mut behavior, ui);
        }
        if let Some(s) = focused_size {
            self.last_focused_size = Some(s);
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
            if let Some(term) = self.terms.get_mut(&self.focused_pane) {
                term.write_bytes(c.as_bytes());
                term.write_bytes(b"\r");
            }
            // Re-running moves the command to the front (no duplicate).
            self.cmd_history.record(c.clone());
        }
        self.last_palette_run = cmd.clone();
        self.palette_open = false;
        cmd
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
            install_chrome_fonts(ctx);
            self.fonts_installed = true;
        }
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
        } else {
            self.forward_input_to_focused(ctx);
        }

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

        // 3) the pane grid (egui_tiles) — LIVE terminal panes (Milestone 2)
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(colors.bg))
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

/// Cell metrics (physical px) derived from egui's monospace font at `font_size`
/// — the same font [`paint_grid_native`] draws the grid with — so the PTY's
/// `(cols, rows)` match the rendered glyph size. Width is the advance of `'M'`;
/// height is the font's row height; both scaled to physical pixels by the
/// context's `pixels_per_point`.
fn monospace_cell_metrics(painter: &egui::Painter, font_size: f32, ppp: f32) -> CellMetrics {
    let probe = egui::text::LayoutJob::single_section(
        "M".to_string(),
        egui::text::TextFormat {
            font_id: egui::FontId::monospace(font_size.max(6.0)),
            ..Default::default()
        },
    );
    let size = painter.layout_job(probe).size();
    CellMetrics {
        advance_w: (size.x * ppp).max(1.0),
        line_h: (size.y * ppp).max(1.0),
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
}

/// The theme's default foreground as an `(r,g,b)` triple — the glyph colour for
/// runs with no explicit SGR colour, and the egui-painter fallback colour.
fn term_default_fg(theme: &c0pl4nd_core::Theme) -> (u8, u8, u8) {
    c0pl4nd_core::theme::parse_hex(&theme.foreground).unwrap_or((232, 230, 240))
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
/// The argument list is a bundle of per-frame render inputs (font size, theme,
/// focus, cursor config, padding) threaded from the single call site in
/// [`C0pl4ndApp::render_pane_body`]; the `too_many_arguments` allow matches that
/// sibling free function for the same reason.
#[allow(clippy::too_many_arguments)]
fn paint_grid_native(
    painter: &egui::Painter,
    rect: egui::Rect,
    term: &PaneTerm,
    font_size: f32,
    theme: &c0pl4nd_core::Theme,
    focused: bool,
    cursor_cfg: c0pl4nd_core::config::CursorConfig,
    padding: f32,
) {
    let default_fg = term_default_fg(theme);
    let mut job = egui::text::LayoutJob::default();
    job.wrap.max_width = f32::INFINITY;
    let font = egui::FontId::monospace(font_size);
    match term.grid_spans() {
        Some(runs) if !runs.is_empty() => {
            for (text, (r, g, b)) in runs {
                job.append(
                    &text,
                    0.0,
                    egui::text::TextFormat {
                        font_id: font.clone(),
                        color: egui::Color32::from_rgb(r, g, b),
                        ..Default::default()
                    },
                );
            }
        }
        _ => {
            // No colour runs (e.g. dead session mid-frame): mono fallback so the
            // pane is never blank.
            job.append(
                &term.grid_text().unwrap_or_default(),
                0.0,
                egui::text::TextFormat {
                    font_id: font.clone(),
                    color: egui::Color32::from_rgb(default_fg.0, default_fg.1, default_fg.2),
                    ..Default::default()
                },
            );
        }
    }
    let galley = painter.layout_job(job);
    // Inset the grid by the configurable window padding (points). This is read
    // live from `config.window.padding` each frame (threaded down from
    // `grid_ui`), so changing Padding in settings moves the text origin without
    // a relaunch — the previously-hardcoded 4px is now the default value of the
    // setting, not a constant. The origin is computed by the pure
    // [`grid_text_origin`] helper so the live-apply wiring is unit-testable
    // without a GPU.
    let origin = grid_text_origin(rect, padding);
    painter.galley(
        origin,
        galley,
        egui::Color32::from_rgb(default_fg.0, default_fg.1, default_fg.2),
    );

    // --- terminal cursor ---
    if let Some((row, col)) = term.cursor_cell() {
        // Cell size in POINTS from the same monospace font the grid uses (a
        // probe-galley 'M' advance), so the caret lands on the cell grid.
        let probe = painter.layout_job(egui::text::LayoutJob::single_section(
            "M".to_string(),
            egui::text::TextFormat {
                font_id: egui::FontId::monospace(font_size),
                ..Default::default()
            },
        ));
        let (cw, ch) = (probe.size().x.max(1.0), probe.size().y.max(1.0));
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

/// Install the Phosphor icon font into egui's font set so the chrome's caption
/// glyphs (close/maximize/minimize/gear, split-right/down) render as crisp icons
/// instead of the default-font missing-glyph tofu boxes. Phosphor is merged into
/// BOTH the proportional and monospace families so a chrome button using either
/// font resolves the icon codepoint. Called once at startup.
fn install_chrome_fonts(ctx: &egui::Context) {
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
    ctx.set_fonts(fonts);
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
        // Portable see-through: theme background, alpha = opacity slider.
        c0pl4nd_core::config::WindowMode::Transparent => {
            let (r, g, b) =
                c0pl4nd_core::theme::parse_hex(&theme.background).unwrap_or((0x12, 0x12, 0x12));
            let a = config.opacity.clamp(0.30, 1.0);
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

        // The 0.30 floor is honoured even if a lower opacity slips through.
        cfg.opacity = 0.1;
        let [_, _, _, a2] = window_clear_color(&cfg, &app.theme);
        assert!(
            (a2 - 0.30).abs() < 1e-6,
            "alpha is clamped to the 0.30 floor"
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
}
