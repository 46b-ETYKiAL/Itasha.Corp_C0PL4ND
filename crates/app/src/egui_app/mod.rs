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
    /// Whether the chrome fonts (incl. the `phosphor-fill` family used for a
    /// pinned tab's solid pin) have been installed on the egui context. Set in
    /// `new`; the first `frame_tick` installs them otherwise (e.g. headless
    /// tests built via `bootstrap()`), so referencing the `phosphor-fill` family
    /// can never hit an unregistered-family panic.
    fonts_installed: bool,
    /// Whether the settings window is open.
    settings_open: bool,
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
        cc.egui_ctx.set_visuals(theme::itasha_corp_visuals());
        install_chrome_fonts(&cc.egui_ctx);
        apply_window_effect(cc);
        let mut app = Self::bootstrap();
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
            fonts_installed: false,
            settings_open: false,
            toast: None,
            last_window_cmd: None,
            live_window: false,
        }
    }

    /// Spawn a fresh live terminal for `pid` and register it. Used by `split`.
    fn spawn_term(&mut self, pid: PaneId) {
        self.terms.insert(
            pid,
            PaneTerm::spawn(self.theme.clone(), SPAWN_COLS, SPAWN_ROWS),
        );
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

    /// `(pane_id, title)` for every pane in the grid, in STABLE visual order
    /// (left→right, top→bottom). Built by walking the tree from the root via
    /// [`grid::panes_in_visual_order`] — NOT by iterating the `ahash::HashMap`
    /// storage, whose order changes every process launch (the "tab order
    /// reshuffles between launches" bug). The tab strip and every consumer of
    /// this list therefore stay in a fixed, on-screen-matching order.
    fn pane_titles(&self) -> Vec<(PaneId, String)> {
        grid::panes_in_visual_order(&self.grid_tree)
            .into_iter()
            .map(|pane_id| (pane_id, format!("pane {}", pane_id.raw())))
            .collect()
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
    fn new_terminal(&mut self) {
        let (w, h) = self.last_focused_size.unwrap_or((16.0, 9.0));
        let dir = if w >= h {
            egui_tiles::LinearDir::Horizontal // wide → side-by-side
        } else {
            egui_tiles::LinearDir::Vertical // tall → stacked
        };
        self.split(dir);
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
        let stroke = if focused {
            egui::Stroke::new(2.0, theme::brand::GREEN)
        } else {
            egui::Stroke::new(1.0, theme::brand::BEZEL)
        };
        painter.rect_stroke(
            rect,
            egui::CornerRadius::same(4),
            stroke,
            egui::StrokeKind::Inside,
        );

        // --- resize the PTY to fit this rect (debounced) ---
        let px_w = rect.width() * ppp;
        let px_h = rect.height() * ppp;
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
                paint_grid_native(&painter, rect, term, font_size, theme, focused, cursor_cfg);
            }
            Some(term) => {
                // Failed spawn: show the error, never panic.
                painter.text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    term.error().unwrap_or("terminal unavailable"),
                    egui::FontId::monospace(14.0),
                    theme::brand::FG,
                );
            }
            None => {
                painter.text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    format!("pane {} (no terminal)", pane_id.raw()),
                    egui::FontId::monospace(14.0),
                    theme::brand::FG,
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
            let mut render_body = |ui: &mut egui::Ui, pid: PaneId| -> bool {
                let outcome = Self::render_pane_body(
                    ui,
                    pid,
                    pid == focused,
                    terms,
                    theme,
                    font_size,
                    cursor_cfg,
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
        let outcome = settings::show(ctx, &mut self.config, &mut open);
        self.settings_open = open;

        if outcome.changed {
            // Reload the terminal grid's color theme so a theme change shows in
            // the live PTY panes immediately (the chrome Visuals are re-applied
            // below; the grid glyph colours come from this `Theme`, not Visuals).
            if outcome.theme_changed {
                self.theme = load_terminal_theme(&self.config);
            }
            // Re-apply the chrome Visuals so opacity/theme tweaks repaint the
            // egui surface without waiting for a relaunch.
            ctx.set_visuals(theme::itasha_corp_visuals());
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

    /// The current terminal color theme stem from the live config.
    #[allow(dead_code)]
    pub fn config_theme(&self) -> &str {
        &self.config.theme
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

impl eframe::App for C0pl4ndApp {
    /// Frameless + transparent => clear to transparent so rounded corners and
    /// the OS acrylic blur show through.
    fn clear_color(&self, _v: &egui::Visuals) -> [f32; 4] {
        [0.0, 0.0, 0.0, 0.0]
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
        // 0) forward this frame's keyboard/paste to the FOCUSED pane's PTY. Done
        //    BEFORE the panels so the keystrokes reach the PTY whose grid this
        //    same frame then snapshots — proving the round-trip (the load-bearing
        //    "typing reaches the PTY and the grid updates" path).
        self.forward_input_to_focused(ctx);

        // 1) custom titlebar + tab strip. Fixed height so the drag region below
        //    is exactly the bar (not the whole remaining column), and so the
        //    caption-cluster geometry is stable.
        let actions = egui::TopBottomPanel::top("titlebar")
            .exact_height(40.0)
            .frame(
                egui::Frame::new()
                    .fill(theme::brand::PANEL)
                    .inner_margin(6.0),
            )
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
                self.titlebar_and_tabs(ui)
            })
            .inner;

        // 2) status bar
        egui::TopBottomPanel::bottom("status")
            .frame(
                egui::Frame::new()
                    .fill(theme::brand::PANEL)
                    .inner_margin(4.0),
            )
            .show(ctx, |ui| self.status_bar(ui));

        // 3) the pane grid (egui_tiles) — LIVE terminal panes (Milestone 2)
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(theme::brand::BG))
            .show(ctx, |ui| self.grid_ui(ui));

        // Apply chrome actions AFTER the panels close (no mid-borrow mutation).
        if let Some(pid) = actions.focus_tab {
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
        if actions.toggle_settings {
            self.settings_open = !self.settings_open;
        }
        // Caption command: issue the REAL OS viewport command AND record it so an
        // interaction test can assert the click had its effect.
        if let Some(cmd) = actions.window_cmd {
            self.last_window_cmd = Some(cmd);
            let is_max = ctx.input(|i| i.viewport().maximized.unwrap_or(false));
            let vp = match cmd {
                WindowCmd::Minimize => egui::ViewportCommand::Minimized(true),
                WindowCmd::ToggleMaximize => egui::ViewportCommand::Maximized(!is_max),
                WindowCmd::Close => egui::ViewportCommand::Close,
            };
            ctx.send_viewport_cmd(vp);
        }

        // 4) the (opaque) settings window, if open
        if self.settings_open {
            self.settings_window(ctx);
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
fn paint_grid_native(
    painter: &egui::Painter,
    rect: egui::Rect,
    term: &PaneTerm,
    font_size: f32,
    theme: &c0pl4nd_core::Theme,
    focused: bool,
    cursor_cfg: c0pl4nd_core::config::CursorConfig,
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
    let origin = rect.left_top() + egui::vec2(4.0, 4.0);
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

/// Apply the OS window effect (acrylic on Windows, vibrancy on macOS). Best-
/// effort + graceful on unsupported platforms (recon dossier §3.3).
fn apply_window_effect(cc: &eframe::CreationContext<'_>) {
    let _ = cc;
    #[cfg(windows)]
    {
        // Tinted blur matching the void background (#121212 @ 160 alpha).
        let _ = window_vibrancy::apply_acrylic(cc, Some((0x12, 0x12, 0x12, 160)));
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
    // Linux: the transparent surface + brand tint carry the look (no native API).
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
}
