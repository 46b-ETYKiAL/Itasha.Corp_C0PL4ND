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
mod theme;

use eframe::egui;

use grid::{count_panes, GridBehavior, Pane, PaneId, PaneIdAllocator};

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
    #[allow(dead_code)]
    config: c0pl4nd_core::Config,
    /// The tiling pane grid (placeholder panes in Milestone 1).
    grid_tree: egui_tiles::Tree<Pane>,
    /// Monotonic pane-id allocator.
    pane_alloc: PaneIdAllocator,
    /// The currently-focused pane (drives tab highlight + future input routing).
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
}

impl C0pl4ndApp {
    /// Build the app inside eframe, applying the brand Visuals + window effect.
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        cc.egui_ctx.set_visuals(theme::itasha_corp_visuals());
        apply_window_effect(cc);
        Self::bootstrap()
    }

    /// Construct the app state independent of eframe — used by `new` and by the
    /// headless `egui_kittest` tests (which run without a window).
    pub fn bootstrap() -> Self {
        let config = c0pl4nd_core::Config::default();
        let mut pane_alloc = PaneIdAllocator::default();
        let initial: Vec<PaneId> = (0..INITIAL_PANES).map(|_| pane_alloc.next()).collect();
        let focused_pane = initial[0];
        let grid_tree = grid::build_default_grid(&initial);
        Self {
            config,
            grid_tree,
            pane_alloc,
            focused_pane,
            settings_open: false,
            toast: None,
            last_window_cmd: None,
        }
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
            self.focused_pane = new_pane;
            self.toast = None;
        }
    }

    /// Paint the placeholder body for one pane: a brand-tinted rect + its id.
    /// Returns true if the pane wants to begin a drag (egui_tiles `DragStarted`).
    /// Milestone 2 replaces this with the glyphon terminal paint callback.
    fn paint_placeholder_pane(ui: &mut egui::Ui, pane_id: PaneId, focused: bool) -> bool {
        let (rect, resp) =
            ui.allocate_exact_size(ui.available_size(), egui::Sense::click_and_drag());
        // Deterministic per-pane tint derived from the id, so panes are visually
        // distinct without needing a palette table.
        let hue = (pane_id.raw().wrapping_mul(2654435761) % 360) as f32;
        let fill = hsv_to_color32(hue, 0.45, 0.22);
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, egui::CornerRadius::same(4), fill);
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
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            format!("pane {}", pane_id.raw()),
            egui::FontId::monospace(18.0),
            theme::brand::FG,
        );
        resp.drag_started()
    }

    /// Render the egui_tiles grid + enforce the 6-pane cap (clone-and-snap-back).
    fn grid_ui(&mut self, ui: &mut egui::Ui) {
        let titles = self.pane_titles();
        let mut closes: Vec<PaneId> = Vec::new();
        let focused = self.focused_pane;
        let mut clicked: Option<PaneId> = None;

        // Snapshot BEFORE the frame so we can revert a drag that exceeds the cap.
        let pre = self.grid_tree.clone();
        {
            let mut render_body = |ui: &mut egui::Ui, pid: PaneId| -> bool {
                let drag = Self::paint_placeholder_pane(ui, pid, pid == focused);
                // Record a click anywhere in the pane body to refocus it.
                if ui
                    .interact(ui.max_rect(), ui.id().with(pid), egui::Sense::click())
                    .clicked()
                {
                    clicked = Some(pid);
                }
                drag
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

        // Apply close requests; keep at least one pane alive.
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
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.frame_tick(&ctx);
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

        // 3) the pane grid (egui_tiles) — placeholder panes in Milestone 1
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
    }
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

/// Minimal HSV→`Color32` for the per-pane placeholder tints. `h` in `[0,360)`,
/// `s`/`v` in `[0,1]`.
fn hsv_to_color32(h: f32, s: f32, v: f32) -> egui::Color32 {
    let c = v * s;
    let h6 = h / 60.0;
    let x = c * (1.0 - (h6 % 2.0 - 1.0).abs());
    let (r, g, b) = match h6 as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = v - c;
    egui::Color32::from_rgb(
        ((r + m) * 255.0) as u8,
        ((g + m) * 255.0) as u8,
        ((b + m) * 255.0) as u8,
    )
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
