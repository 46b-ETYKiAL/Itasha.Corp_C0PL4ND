//! Milestone 1 of the C0PL4ND egui chrome modernization (recon dossier
//! `.s4f3-data/recon-c0pl4nd-egui-modernization.md`, steps 1–4).
//!
//! This module is the modern `eframe`/`egui` application shell, shipped as a
//! SEPARATE binary (`c0pl4nd-egui`) so the existing winit-driven `c0pl4nd`
//! binary keeps building and shipping unchanged. The chrome (frameless
//! titlebar, two-tone wordmark, tab strip, caption buttons, status bar) and the
//! `egui_tiles` pane grid are real and clickable; the pane BODIES are
//! placeholders (a colored rect + the pane id) — Milestone 2 replaces them with
//! the live glyphon terminal via an egui-wgpu paint callback.
//!
//! No PTY, no terminal, no winit event loop here — eframe owns the loop.

pub mod chrome;
pub mod grid;
pub mod pane_term;
pub mod term_render;
mod theme;

use std::collections::HashMap;

use eframe::egui;

use grid::{count_panes, GridBehavior, Pane, PaneId, PaneIdAllocator};
use pane_term::{CellMetrics, PaneTerm};

/// How many placeholder panes the shell opens with on first launch.
const INITIAL_PANES: usize = 2;

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
    /// The cell metrics (physical px) used to map a pane rect → `(cols, rows)`.
    /// Refreshed from the GPU font once the glyphon resources exist; the
    /// fallback keeps headless math sane before the first real frame.
    cell_metrics: CellMetrics,
    /// Monotonic pane-id allocator.
    pane_alloc: PaneIdAllocator,
    /// The currently-focused pane (drives tab highlight + input routing).
    focused_pane: PaneId,
    /// Whether the settings window is open.
    settings_open: bool,
    /// A transient status-bar message (e.g. "max 6 panes").
    toast: Option<String>,
    /// The most recent caption command issued (minimize/maximize/close). Set in
    /// [`Self::frame_tick`] alongside the real `ViewportCommand`, so interaction
    /// tests can assert that clicking a caption button had its real effect (the
    /// OS command itself is not observable in a headless harness).
    last_window_cmd: Option<WindowCmd>,
    /// True once the glyphon GPU resources have been installed into egui-wgpu's
    /// `callback_resources` (only in a real eframe window, never in headless
    /// tests). When false, panes render the headless text fallback.
    gpu_ready: bool,
}

/// The PTY grid size used to spawn a pane before its real pixel rect is known.
/// The first `resize_to_px` corrects it to fit the allocated rect.
const SPAWN_COLS: u16 = 80;
/// See [`SPAWN_COLS`].
const SPAWN_ROWS: u16 = 24;

impl C0pl4ndApp {
    /// Build the app inside eframe, applying the brand Visuals + window effect,
    /// and capturing egui's shared wgpu device to stand up the glyphon terminal
    /// resources (recon dossier §2.2). Falls back to the headless build when the
    /// wgpu render state is absent (should not happen with the `wgpu` backend).
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        cc.egui_ctx.set_visuals(theme::itasha_corp_visuals());
        apply_window_effect(cc);
        let mut app = Self::bootstrap();
        app.install_gpu(cc);
        app
    }

    /// Capture egui's shared `wgpu::Device`/`Queue`/`target_format` and install
    /// the glyphon [`term_render::TermGpu`] into egui-wgpu's `callback_resources`
    /// so the per-pane paint callbacks can reach it. Refreshes the cell metrics
    /// from the real GPU font. No-op (leaves `gpu_ready=false`) when the wgpu
    /// backend is unavailable — panes then render the headless text fallback.
    fn install_gpu(&mut self, cc: &eframe::CreationContext<'_>) {
        let Some(rs) = cc.wgpu_render_state.as_ref() else {
            tracing::warn!("eframe has no wgpu render state; terminal grid uses text fallback");
            return;
        };
        let font_px = self.config.font.size.max(6.0);
        let line_px = font_px * 1.3;
        let mut gpu =
            term_render::TermGpu::new(&rs.device, &rs.queue, rs.target_format, font_px, line_px);
        self.cell_metrics = gpu.cell_metrics();
        rs.renderer.write().callback_resources.insert(gpu);
        self.gpu_ready = true;
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
            cell_metrics: CellMetrics::FALLBACK,
            pane_alloc,
            focused_pane,
            settings_open: false,
            toast: None,
            last_window_cmd: None,
            gpu_ready: false,
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

    /// The most recent caption command the user issued (min/max/close), or
    /// `None` if no caption button has been clicked this session.
    #[allow(dead_code)]
    pub fn last_window_cmd(&self) -> Option<WindowCmd> {
        self.last_window_cmd
    }

    /// `(pane_id, title)` for every pane in the grid, in tree order.
    fn pane_titles(&self) -> Vec<(PaneId, String)> {
        self.grid_tree
            .tiles
            .iter()
            .filter_map(|(_, tile)| match tile {
                egui_tiles::Tile::Pane(p) => Some((p.pane_id, format!("pane {}", p.pane_id.raw()))),
                _ => None,
            })
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

    /// Paint one terminal pane's body and wire its per-frame interaction:
    ///
    /// 1. Allocate the pane rect and paint the theme background quad behind the
    ///    glyphs (so text never blends directly against the acrylic — pitfall #3).
    /// 2. Compute the physical-pixel rect and DEBOUNCED-resize the PTY to fit.
    /// 3. Snapshot the grid into colour runs and queue the glyphon paint
    ///    callback (recon dossier §2.3) — or, when the GPU is unavailable
    ///    (headless tests), paint the grid text with egui's own painter so the
    ///    pane is never blank.
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
        cell_metrics: CellMetrics,
        gpu_ready: bool,
        font_size: f32,
    ) -> PaneBodyOutcome {
        let (rect, resp) =
            ui.allocate_exact_size(ui.available_size(), egui::Sense::click_and_drag());
        let ppp = ui.ctx().pixels_per_point();
        let painter = ui.painter_at(rect);

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

        // --- render the grid ---
        match terms.get(&pane_id) {
            Some(term) if term.error().is_none() => {
                if gpu_ready {
                    // Queue the real glyphon GPU paint callback. The grid
                    // snapshot happens on the CPU here; the callback only paints.
                    if let Some(runs) = term.grid_spans() {
                        let fg = term_default_fg(theme);
                        ui.painter().add(egui_wgpu::Callback::new_paint_callback(
                            rect,
                            term_render::TermPaint {
                                pane_id,
                                px_rect: [rect.left() * ppp, rect.top() * ppp, px_w, px_h],
                                default_fg: [fg.0, fg.1, fg.2],
                                runs: std::sync::Arc::new(runs),
                            },
                        ));
                    }
                } else {
                    // Headless fallback: paint the grid text with egui so the
                    // pane is never blank (and so a snapshot test can read it).
                    let fg = term_default_fg(theme);
                    let text = term.grid_text().unwrap_or_default();
                    painter.text(
                        rect.left_top() + egui::vec2(4.0, 4.0),
                        egui::Align2::LEFT_TOP,
                        text,
                        egui::FontId::monospace(font_size),
                        egui::Color32::from_rgb(fg.0, fg.1, fg.2),
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

        // Snapshot BEFORE the frame so we can revert a drag that exceeds the cap.
        let pre = self.grid_tree.clone();
        {
            // Disjoint borrows: the closure touches these fields, NOT grid_tree.
            let terms = &mut self.terms;
            let theme = &self.theme;
            let cell_metrics = self.cell_metrics;
            let gpu_ready = self.gpu_ready;
            let font_size = self.config.font.size;
            let mut render_body = |ui: &mut egui::Ui, pid: PaneId| -> bool {
                let outcome = Self::render_pane_body(
                    ui,
                    pid,
                    pid == focused,
                    terms,
                    theme,
                    cell_metrics,
                    gpu_ready,
                    font_size,
                );
                if outcome.clicked {
                    clicked = Some(pid);
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
        if !closes.is_empty() {
            for pid in closes {
                if count_panes(&self.grid_tree) <= 1 {
                    break;
                }
                if let Some(tile) = grid::tile_of_pane(&self.grid_tree, pid) {
                    self.grid_tree.tiles.remove(tile);
                    self.grid_tree.simplify_children_of_tile(
                        self.grid_tree.root.unwrap_or(tile),
                        &egui_tiles::SimplificationOptions::default(),
                    );
                    self.terms.remove(&pid);
                }
            }
            // Re-anchor focus if the focused pane was closed.
            if grid::tile_of_pane(&self.grid_tree, self.focused_pane).is_none() {
                if let Some((pid, _)) = self.pane_titles().first() {
                    self.focused_pane = *pid;
                }
            }
        }
    }

    /// The settings window (opaque placeholder — Milestone 2 fills it in).
    fn settings_window(&mut self, ctx: &egui::Context) {
        let mut open = self.settings_open;
        egui::Window::new("Settings")
            .open(&mut open)
            .resizable(true)
            .frame(egui::Frame::window(&ctx.global_style()).fill(theme::brand::PANEL))
            .show(ctx, |ui| {
                ui.label("C0PL4ND settings (placeholder — milestone 2)");
                ui.separator();
                ui.label(egui::RichText::new("Theme: itasha_corp").color(theme::brand::GREEN));
                ui.label(format!(
                    "Font: {} @ {}pt",
                    self.config.font.family, self.config.font.size
                ));
            });
        self.settings_open = open;
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
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.frame_tick(&ctx);
        // Reap closed-pane GPU buffers via the eframe render state (only
        // reachable through the Frame, not the Context).
        if let Some(rs) = frame.wgpu_render_state() {
            self.gc_gpu_panes(rs);
        }
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
        // 0) forward this frame's keyboard/paste to the FOCUSED pane's PTY. Done
        //    BEFORE the panels so the keystrokes reach the PTY whose grid this
        //    same frame then snapshots — proving the round-trip (the load-bearing
        //    "typing reaches the PTY and the grid updates" path).
        self.forward_input_to_focused(ctx);

        // 1) custom titlebar + tab strip
        let actions = egui::TopBottomPanel::top("titlebar")
            .frame(
                egui::Frame::new()
                    .fill(theme::brand::PANEL)
                    .inner_margin(6.0),
            )
            .show(ctx, |ui| self.titlebar_and_tabs(ui))
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
        if actions.split_right {
            self.split(egui_tiles::LinearDir::Horizontal);
        }
        if actions.split_down {
            self.split(egui_tiles::LinearDir::Vertical);
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
        // for an input event — but ONLY in the real window (`gpu_ready`). In the
        // headless `egui_kittest` harness an unconditional `request_repaint`
        // makes `Harness::run` loop until `max_steps` (the UI never settles); the
        // tests there drive frames explicitly with `h.run()` after each input, so
        // they do not need the animation pump. (Closed-pane GPU buffers are
        // reaped in `App::ui` via the eframe Frame's render state.)
        if self.gpu_ready {
            ctx.request_repaint();
        }
    }

    /// Drop the glyphon GPU buffers/renderers for panes that no longer exist, so
    /// a closed pane does not leak its glyph buffer. Called from `App::ui` with
    /// the eframe render state (the only place the wgpu backend is reachable).
    /// No-op in headless tests (no render state, `gpu_ready == false`).
    fn gc_gpu_panes(&mut self, render_state: &eframe::egui_wgpu::RenderState) {
        if !self.gpu_ready {
            return;
        }
        let live: Vec<PaneId> = self.terms.keys().copied().collect();
        if let Some(gpu) = render_state
            .renderer
            .write()
            .callback_resources
            .get_mut::<term_render::TermGpu>()
        {
            gpu.retain_panes(&live);
        }
    }
}

/// Outcome of painting one terminal pane's body for a frame.
struct PaneBodyOutcome {
    /// Whether the pane reported it wants to begin an egui_tiles drag.
    drag_started: bool,
    /// True when the pane body was clicked (a refocus request).
    clicked: bool,
}

/// The theme's default foreground as an `(r,g,b)` triple — the glyph colour for
/// runs with no explicit SGR colour, and the egui-painter fallback colour.
fn term_default_fg(theme: &c0pl4nd_core::Theme) -> (u8, u8, u8) {
    c0pl4nd_core::theme::parse_hex(&theme.foreground).unwrap_or((232, 230, 240))
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
