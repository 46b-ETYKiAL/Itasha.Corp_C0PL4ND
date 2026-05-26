//! Windowed GPU terminal shell: winit window + wgpu surface + glyphon text.
//!
//! Frameless by design — C0PL4ND draws its own brand title bar (wordmark +
//! min/max/close buttons) so the chrome matches the Retro-Future Anime OS
//! aesthetic instead of the OS default. Renders the live terminal grid from
//! `c0pl4nd-core` and forwards keyboard input to the PTY. Redraws on a light
//! poll so shell output appears promptly while an idle screen stays cheap.

use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use c0pl4nd_core::{theme::parse_hex, Config, Session, Theme};
use glyphon::{
    Attrs, Buffer, Cache, Color as GColor, Family, FontSystem, Metrics, Resolution, Shaping,
    SwashCache, TextArea, TextAtlas, TextBounds, TextRenderer, Viewport,
};
use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::window::{Window, WindowId};

const LINE_HEIGHT: f32 = 20.0;
const CELL_W: f32 = 9.0;
/// Height of the custom (frameless) title bar, in physical pixels.
const TITLEBAR_H: f32 = 30.0;
/// Width of each title-bar button hit zone, in physical pixels.
const BUTTON_W: f32 = 46.0;
/// Total width of the 3-button cluster.
const BUTTONS_W: f32 = BUTTON_W * 3.0;

/// Public entrypoint: open the windowed terminal.
pub fn run_gui(config: &Config) -> Result<()> {
    let event_loop = EventLoop::new()?;
    event_loop.set_control_flow(ControlFlow::Wait);
    let mut app = App::new(config.clone());
    event_loop.run_app(&mut app)?;
    Ok(())
}

/// Which title-bar button a point falls on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TitlebarHit {
    None,
    Drag,
    Minimize,
    Maximize,
    Close,
}

struct Gpu {
    window: Arc<Window>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    surface: wgpu::Surface<'static>,
    surface_config: wgpu::SurfaceConfiguration,
    font_system: FontSystem,
    swash_cache: SwashCache,
    atlas: TextAtlas,
    viewport: Viewport,
    text_renderer: TextRenderer,
    grid_buffer: Buffer,
    chrome_buffer: Buffer,
    palette_buffer: Buffer,
    image_renderer: crate::image_render::ImageRenderer,
}

struct App {
    config: Config,
    theme: Theme,
    /// One tab per entry; each tab holds one or more split panes.
    tabs: Vec<Tab>,
    active: usize,
    gpu: Option<Gpu>,
    next_poll: Instant,
    cursor: (f64, f64),
    modifiers: ModifiersState,
    search_mode: bool,
    search_query: String,
    search_matches: Vec<c0pl4nd_core::search::SearchMatch>,
    search_idx: usize,
    palette_mode: bool,
    palette_query: String,
    palette_idx: usize,
}

/// Actions available from the command palette.
const PALETTE_ACTIONS: &[&str] = &[
    "New Tab",
    "Close Tab",
    "Next Tab",
    "Previous Tab",
    "Split Right",
    "Split Down",
    "Focus Next Pane",
    "Search",
    "Scroll To Bottom",
    "Quit",
];

/// A tab holds one or more split panes (each its own shell session).
struct Tab {
    panes: Vec<Session>,
    focus: usize,
    /// true = panes stacked vertically; false = side by side.
    stacked: bool,
}

impl Tab {
    fn single(session: Session) -> Self {
        Tab {
            panes: vec![session],
            focus: 0,
            stacked: false,
        }
    }
}

impl App {
    fn new(config: Config) -> Self {
        let theme = load_theme(&config.theme).unwrap_or_else(Theme::builtin_void);
        App {
            config,
            theme,
            tabs: Vec::new(),
            active: 0,
            gpu: None,
            next_poll: Instant::now(),
            cursor: (0.0, 0.0),
            modifiers: ModifiersState::empty(),
            search_mode: false,
            search_query: String::new(),
            search_matches: Vec::new(),
            search_idx: 0,
            palette_mode: false,
            palette_query: String::new(),
            palette_idx: 0,
        }
    }

    fn enter_palette(&mut self) {
        self.palette_mode = true;
        self.palette_query.clear();
        self.palette_idx = 0;
    }

    fn palette_filtered(&self) -> Vec<&'static str> {
        c0pl4nd_core::fuzzy::filter_sorted(PALETTE_ACTIONS, &self.palette_query)
    }

    fn handle_palette_key(&mut self, key: &Key, event_loop: &ActiveEventLoop) {
        match key {
            Key::Named(NamedKey::Escape) => {
                self.palette_mode = false;
            }
            Key::Named(NamedKey::ArrowDown) => {
                let n = self.palette_filtered().len().max(1);
                self.palette_idx = (self.palette_idx + 1) % n;
            }
            Key::Named(NamedKey::ArrowUp) => {
                let n = self.palette_filtered().len().max(1);
                self.palette_idx = (self.palette_idx + n - 1) % n;
            }
            Key::Named(NamedKey::Enter) => {
                let pick = self.palette_filtered().get(self.palette_idx).copied();
                self.palette_mode = false;
                if let Some(action) = pick {
                    self.execute_palette_action(action, event_loop);
                }
            }
            Key::Named(NamedKey::Backspace) => {
                self.palette_query.pop();
                self.palette_idx = 0;
            }
            Key::Character(s) => {
                self.palette_query.push_str(s);
                self.palette_idx = 0;
            }
            _ => {}
        }
        self.request_redraw();
    }

    fn execute_palette_action(&mut self, action: &str, event_loop: &ActiveEventLoop) {
        match action {
            "New Tab" => self.spawn_tab(),
            "Close Tab" => self.close_active_tab(event_loop),
            "Next Tab" => self.next_tab(),
            "Previous Tab" => self.prev_tab(),
            "Split Right" => self.split_active(false),
            "Split Down" => self.split_active(true),
            "Focus Next Pane" => self.focus_next_pane(),
            "Search" => self.enter_search(),
            "Scroll To Bottom" => {
                if let Some(s) = self.active_session() {
                    if let Ok(mut t) = s.terminal().lock() {
                        t.scroll_to_bottom();
                    }
                }
            }
            "Quit" => event_loop.exit(),
            _ => {}
        }
    }

    fn enter_search(&mut self) {
        self.search_mode = true;
        self.search_query.clear();
        self.search_matches.clear();
        self.search_idx = 0;
    }

    fn exit_search(&mut self) {
        self.search_mode = false;
        self.search_query.clear();
        self.search_matches.clear();
        if let Some(s) = self.active_session() {
            if let Ok(mut t) = s.terminal().lock() {
                t.scroll_to_bottom();
            }
        }
    }

    fn recompute_search(&mut self) {
        let lines = self
            .active_session()
            .and_then(|s| s.terminal().lock().ok().map(|t| t.all_lines()));
        if let Some(lines) = lines {
            self.search_matches =
                c0pl4nd_core::search::find(&lines, &self.search_query, Default::default());
            self.search_idx = 0;
            self.jump_to_current_match();
        }
    }

    fn next_match(&mut self) {
        if self.search_matches.is_empty() {
            return;
        }
        self.search_idx = (self.search_idx + 1) % self.search_matches.len();
        self.jump_to_current_match();
    }

    /// Scroll the active terminal so the current match line is visible.
    fn jump_to_current_match(&self) {
        if let Some(m) = self.search_matches.get(self.search_idx) {
            if let Some(s) = self.active_session() {
                if let Ok(mut t) = s.terminal().lock() {
                    let offset = t.scrollback_len().saturating_sub(m.line);
                    t.set_view_offset(offset);
                }
            }
        }
    }

    fn handle_search_key(&mut self, key: &Key) {
        match key {
            Key::Named(NamedKey::Escape) => self.exit_search(),
            Key::Named(NamedKey::Enter) => self.next_match(),
            Key::Named(NamedKey::Backspace) => {
                self.search_query.pop();
                self.recompute_search();
            }
            Key::Character(s) => {
                self.search_query.push_str(s);
                self.recompute_search();
            }
            _ => {}
        }
        self.request_redraw();
    }

    fn active_tab(&self) -> Option<&Tab> {
        self.tabs.get(self.active)
    }

    fn active_session(&self) -> Option<&Session> {
        self.tabs
            .get(self.active)
            .and_then(|t| t.panes.get(t.focus))
    }

    fn active_session_mut(&mut self) -> Option<&mut Session> {
        self.tabs
            .get_mut(self.active)
            .and_then(|t| t.panes.get_mut(t.focus))
    }

    /// Split the active tab, adding a pane. `stacked` chooses the orientation.
    fn split_active(&mut self, stacked: bool) {
        let (cols, rows) = self.grid_dims();
        if let Ok(s) = Session::spawn_shell(self.config.shell.as_deref(), rows, cols) {
            if let Some(tab) = self.tabs.get_mut(self.active) {
                tab.stacked = stacked;
                tab.panes.push(s);
                tab.focus = tab.panes.len() - 1;
                self.relayout_active();
            }
        }
    }

    fn focus_next_pane(&mut self) {
        if let Some(tab) = self.tabs.get_mut(self.active) {
            if !tab.panes.is_empty() {
                tab.focus = (tab.focus + 1) % tab.panes.len();
            }
        }
    }

    /// Resize every pane in the active tab to its share of the grid.
    fn relayout_active(&mut self) {
        let (cols, rows) = self.grid_dims();
        if let Some(tab) = self.tabs.get_mut(self.active) {
            let n = tab.panes.len().max(1) as u16;
            let (pc, pr) = if tab.stacked {
                (cols, (rows / n).max(1))
            } else {
                ((cols / n).max(1), rows)
            };
            for p in &mut tab.panes {
                let _ = p.resize(pr, pc);
            }
        }
    }

    /// Collect inline-image draw quads for the focused pane, positioned in the
    /// current viewport (skips images scrolled out of view).
    fn collect_image_quads(&self) -> Vec<crate::image_render::ImageQuad> {
        let mut out = Vec::new();
        let Some(s) = self.active_session() else {
            return out;
        };
        if let Ok(t) = s.terminal().lock() {
            let rows = t.grid().rows();
            let window_start = t.scrollback_len().saturating_sub(t.view_offset());
            for img in t.images() {
                if img.line < window_start {
                    continue;
                }
                let vrow = img.line - window_start;
                if vrow >= rows {
                    continue;
                }
                out.push(crate::image_render::ImageQuad {
                    rgba: img.image.rgba.clone(),
                    width: img.image.width as u32,
                    height: img.image.height as u32,
                    x: 8.0 + img.col as f32 * CELL_W,
                    y: TITLEBAR_H + 2.0 + vrow as f32 * LINE_HEIGHT,
                });
            }
        }
        out
    }

    /// Compose the active tab's panes into a single grid of cells: side-by-side
    /// panes are separated by a vertical rule, stacked panes by a horizontal
    /// one. Clears each pane's damage flag as it is read.
    fn compose_active_rows(&self) -> Vec<Vec<c0pl4nd_core::Cell>> {
        use c0pl4nd_core::{Cell, CellFlags, Color};
        let sep = |c: char| Cell {
            c,
            fg: Color::Indexed(8),
            bg: Color::Default,
            flags: CellFlags::empty(),
        };
        let Some(tab) = self.active_tab() else {
            return Vec::new();
        };
        let mut pane_rows: Vec<Vec<Vec<Cell>>> = Vec::new();
        for p in &tab.panes {
            if let Ok(mut t) = p.terminal().lock() {
                let rows = t.display_rows();
                t.grid_mut().clear_damage();
                pane_rows.push(rows);
            }
        }
        if pane_rows.len() <= 1 {
            return pane_rows.into_iter().next().unwrap_or_default();
        }
        if tab.stacked {
            let mut out = Vec::new();
            for (i, pr) in pane_rows.iter().enumerate() {
                if i > 0 {
                    let width = pr.first().map(|r| r.len()).unwrap_or(1).max(1);
                    out.push(vec![sep('\u{2500}'); width]);
                }
                out.extend(pr.iter().cloned());
            }
            out
        } else {
            let nrows = pane_rows.iter().map(|p| p.len()).max().unwrap_or(0);
            let mut out = Vec::with_capacity(nrows);
            for i in 0..nrows {
                let mut line = Vec::new();
                for (pi, pr) in pane_rows.iter().enumerate() {
                    if pi > 0 {
                        line.push(sep('\u{2502}'));
                    }
                    if let Some(row) = pr.get(i) {
                        line.extend(row.iter().cloned());
                    }
                }
                out.push(line);
            }
            out
        }
    }

    fn request_redraw(&self) {
        if let Some(g) = &self.gpu {
            g.window.request_redraw();
        }
    }

    /// Current grid dimensions from the GPU surface (cols, rows).
    fn grid_dims(&self) -> (u16, u16) {
        match &self.gpu {
            Some(g) => {
                let cols = (g.surface_config.width as f32 / CELL_W).floor().max(1.0) as u16;
                let usable = (g.surface_config.height as f32 - TITLEBAR_H).max(LINE_HEIGHT);
                let rows = (usable / LINE_HEIGHT).floor().max(1.0) as u16;
                (cols, rows)
            }
            None => (self.config.window.cols, self.config.window.rows),
        }
    }

    fn spawn_tab(&mut self) {
        let (cols, rows) = self.grid_dims();
        match Session::spawn_shell(self.config.shell.as_deref(), rows, cols) {
            Ok(s) => {
                self.tabs.push(Tab::single(s));
                self.active = self.tabs.len() - 1;
            }
            Err(e) => tracing::error!("failed to spawn tab: {e}"),
        }
    }

    fn close_active_tab(&mut self, event_loop: &ActiveEventLoop) {
        // Close the focused pane first if the tab is split.
        if let Some(tab) = self.tabs.get_mut(self.active) {
            if tab.panes.len() > 1 {
                tab.panes.remove(tab.focus);
                if tab.focus >= tab.panes.len() {
                    tab.focus = tab.panes.len() - 1;
                }
                self.relayout_active();
                return;
            }
        }
        if self.tabs.len() <= 1 {
            event_loop.exit();
            return;
        }
        self.tabs.remove(self.active);
        if self.active >= self.tabs.len() {
            self.active = self.tabs.len() - 1;
        }
    }

    fn next_tab(&mut self) {
        if !self.tabs.is_empty() {
            self.active = (self.active + 1) % self.tabs.len();
        }
    }

    fn prev_tab(&mut self) {
        if !self.tabs.is_empty() {
            self.active = (self.active + self.tabs.len() - 1) % self.tabs.len();
        }
    }

    /// Handle a Ctrl/Cmd+Shift tab-control combo. Returns true if consumed.
    fn handle_tab_combo(&mut self, key: &Key, event_loop: &ActiveEventLoop) -> bool {
        let handled = match key {
            Key::Character(s) => match s.chars().next().map(|c| c.to_ascii_lowercase()) {
                Some('t') => {
                    self.spawn_tab();
                    true
                }
                Some('w') => {
                    self.close_active_tab(event_loop);
                    true
                }
                Some(']') => {
                    self.next_tab();
                    true
                }
                Some('[') => {
                    self.prev_tab();
                    true
                }
                Some('f') => {
                    self.enter_search();
                    true
                }
                Some('p') => {
                    self.enter_palette();
                    true
                }
                Some('d') => {
                    self.split_active(false);
                    true
                }
                Some('e') => {
                    self.split_active(true);
                    true
                }
                Some('o') => {
                    self.focus_next_pane();
                    true
                }
                _ => false,
            },
            Key::Named(NamedKey::Tab) => {
                self.next_tab();
                true
            }
            _ => false,
        };
        if handled {
            self.request_redraw();
        }
        handled
    }

    fn bg_color(&self) -> wgpu::Color {
        let (r, g, b) = parse_hex(&self.theme.background).unwrap_or((8, 6, 13));
        wgpu::Color {
            r: srgb_to_linear(r),
            g: srgb_to_linear(g),
            b: srgb_to_linear(b),
            a: 1.0,
        }
    }

    fn fg_color(&self) -> GColor {
        let (r, g, b) = parse_hex(&self.theme.foreground).unwrap_or((240, 238, 245));
        GColor::rgb(r, g, b)
    }

    fn accent_color(&self) -> GColor {
        let (r, g, b) = parse_hex(&self.theme.cursor).unwrap_or((0, 229, 255));
        GColor::rgb(r, g, b)
    }

    /// Classify a physical-pixel point against the title bar.
    fn hit_titlebar(&self, x: f64, y: f64) -> TitlebarHit {
        let width = self
            .gpu
            .as_ref()
            .map(|g| g.surface_config.width as f64)
            .unwrap_or(0.0);
        if y > TITLEBAR_H as f64 {
            return TitlebarHit::None;
        }
        let buttons_left = width - BUTTONS_W as f64;
        if x < buttons_left {
            return TitlebarHit::Drag;
        }
        match ((x - buttons_left) / BUTTON_W as f64) as i32 {
            0 => TitlebarHit::Minimize,
            1 => TitlebarHit::Maximize,
            _ => TitlebarHit::Close,
        }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.gpu.is_some() {
            return;
        }
        let cols = self.config.window.cols;
        let rows = self.config.window.rows;
        let width = (cols as f32 * CELL_W) as u32 + 16;
        let height = (rows as f32 * LINE_HEIGHT) as u32 + 16 + TITLEBAR_H as u32;

        let attrs = Window::default_attributes()
            .with_title(c0pl4nd_core::PRODUCT_NAME)
            .with_decorations(false)
            .with_inner_size(winit::dpi::LogicalSize::new(width as f64, height as f64));
        let window = Arc::new(event_loop.create_window(attrs).expect("create window"));

        let gpu = match pollster::block_on(Gpu::new(window.clone(), self.config.font.size)) {
            Ok(g) => g,
            Err(e) => {
                tracing::error!("GPU init failed: {e}");
                event_loop.exit();
                return;
            }
        };

        self.gpu = Some(gpu);
        if self.tabs.is_empty() {
            self.spawn_tab();
        }
        if let Some(g) = &self.gpu {
            g.window.request_redraw();
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor = (position.x, position.y);
            }
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            } => {
                let hit = self.hit_titlebar(self.cursor.0, self.cursor.1);
                if let Some(gpu) = &self.gpu {
                    match hit {
                        TitlebarHit::Close => event_loop.exit(),
                        TitlebarHit::Minimize => gpu.window.set_minimized(true),
                        TitlebarHit::Maximize => {
                            let max = gpu.window.is_maximized();
                            gpu.window.set_maximized(!max);
                        }
                        TitlebarHit::Drag => {
                            let _ = gpu.window.drag_window();
                        }
                        TitlebarHit::None => {}
                    }
                }
            }
            WindowEvent::Resized(size) => {
                if let Some(gpu) = &mut self.gpu {
                    gpu.resize(size.width.max(1), size.height.max(1));
                    let cols = (size.width as f32 / CELL_W).floor().max(1.0) as u16;
                    let usable_h = (size.height as f32 - TITLEBAR_H).max(LINE_HEIGHT);
                    let rows = (usable_h / LINE_HEIGHT).floor().max(1.0) as u16;
                    for tab in &mut self.tabs {
                        let n = tab.panes.len().max(1) as u16;
                        let (pc, pr) = if tab.stacked {
                            (cols, (rows / n).max(1))
                        } else {
                            ((cols / n).max(1), rows)
                        };
                        for p in &mut tab.panes {
                            let _ = p.resize(pr, pc);
                        }
                    }
                    gpu.window.request_redraw();
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let (up, lines) = match delta {
                    winit::event::MouseScrollDelta::LineDelta(_, y) => {
                        (y > 0.0, (y.abs().ceil() as usize).max(1))
                    }
                    winit::event::MouseScrollDelta::PixelDelta(p) => (
                        p.y > 0.0,
                        ((p.y.abs() / LINE_HEIGHT as f64).ceil() as usize).max(1),
                    ),
                };
                if let Some(s) = self.active_session() {
                    if let Ok(mut term) = s.terminal().lock() {
                        if up {
                            term.scroll_up_view(lines);
                        } else {
                            term.scroll_down_view(lines);
                        }
                    }
                }
                if let Some(g) = &self.gpu {
                    g.window.request_redraw();
                }
            }
            WindowEvent::ModifiersChanged(m) => {
                self.modifiers = m.state();
            }
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                // Overlay modes capture keystrokes instead of the PTY.
                if self.palette_mode {
                    self.handle_palette_key(&event.logical_key, event_loop);
                    return;
                }
                if self.search_mode {
                    self.handle_search_key(&event.logical_key);
                    return;
                }
                // Tab control combos (Ctrl+Shift+… ; on macOS Cmd+Shift+…).
                let mod_combo = (self.modifiers.control_key() || self.modifiers.super_key())
                    && self.modifiers.shift_key();
                if mod_combo && self.handle_tab_combo(&event.logical_key, event_loop) {
                    return;
                }
                if let Some(bytes) = key_to_bytes(&event.logical_key, &event.text) {
                    if let Some(s) = self.active_session_mut() {
                        if let Ok(mut term) = s.terminal().lock() {
                            term.scroll_to_bottom();
                        }
                        let _ = s.write_input(&bytes);
                    }
                }
            }
            WindowEvent::RedrawRequested => self.render(),
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let now = Instant::now();
        if now >= self.next_poll {
            self.next_poll = now + Duration::from_millis(16);
            let damaged = self
                .active_tab()
                .map(|tab| {
                    tab.panes.iter().any(|p| {
                        p.terminal()
                            .lock()
                            .map(|t| t.grid().is_damaged())
                            .unwrap_or(false)
                    })
                })
                .unwrap_or(false);
            if damaged {
                if let Some(g) = &self.gpu {
                    g.window.request_redraw();
                }
            }
        }
        event_loop.set_control_flow(ControlFlow::WaitUntil(self.next_poll));
    }
}

impl App {
    fn render(&mut self) {
        let fg = self.fg_color();
        let accent = self.accent_color();
        let bg = self.bg_color();
        let signal_red = GColor::rgb(255, 0, 64);
        let default_rgb = parse_hex(&self.theme.foreground).unwrap_or((240, 238, 245));

        // Snapshot the grid into per-colour foreground spans.
        let composed = self.compose_active_rows();
        let mut spans: Vec<(String, GColor)> = Vec::new();
        for row in &composed {
            let mut run = String::new();
            let mut run_color: Option<GColor> = None;
            for cell in row {
                let rgb = resolve_fg(cell.fg, &self.theme, default_rgb);
                let gcol = GColor::rgb(rgb.0, rgb.1, rgb.2);
                if run_color != Some(gcol) {
                    if let Some(pc) = run_color {
                        spans.push((std::mem::take(&mut run), pc));
                    }
                    run_color = Some(gcol);
                }
                run.push(cell.c);
            }
            if let Some(pc) = run_color {
                spans.push((run, pc));
            }
            spans.push(("\n".to_string(), fg));
        }

        // Command-palette overlay text (built before borrowing gpu).
        let palette_text = if self.palette_mode {
            let items = self.palette_filtered();
            let mut s = format!(
                "\u{2592} command palette  \u{203a} {}\n\n",
                self.palette_query
            );
            if items.is_empty() {
                s.push_str("  (no matches)\n");
            }
            for (i, it) in items.iter().enumerate() {
                let marker = if i == self.palette_idx {
                    "\u{25b8} "
                } else {
                    "  "
                };
                s.push_str(marker);
                s.push_str(it);
                s.push('\n');
            }
            Some(s)
        } else {
            None
        };

        let image_quads = self.collect_image_quads();

        let Some(gpu) = &mut self.gpu else { return };
        let width = gpu.surface_config.width as f32;

        // Terminal grid (coloured spans) below the title bar.
        let default_attrs = Attrs::new().family(Family::Monospace).color(fg);
        gpu.grid_buffer.set_rich_text(
            &mut gpu.font_system,
            spans.iter().map(|(s, col)| {
                (
                    s.as_str(),
                    Attrs::new().family(Family::Monospace).color(*col),
                )
            }),
            &default_attrs,
            Shaping::Advanced,
            None,
        );
        gpu.grid_buffer
            .shape_until_scroll(&mut gpu.font_system, false);

        // Custom chrome: wordmark (accent) on the left, buttons on the right.
        let buttons_left = width - BUTTONS_W;
        let pad_cols = ((buttons_left - 12.0) / CELL_W).max(0.0) as usize;
        let mut chrome = if self.search_mode {
            let n = self.search_matches.len();
            let cur = if n == 0 { 0 } else { self.search_idx + 1 };
            format!(
                " search /{}  [{cur}/{n}]  (esc to exit) ",
                self.search_query
            )
        } else if self.tabs.len() > 1 {
            format!(
                " {}  [{}/{}] ",
                c0pl4nd_core::PRODUCT_NAME,
                self.active + 1,
                self.tabs.len()
            )
        } else {
            format!(" {} ", c0pl4nd_core::PRODUCT_NAME)
        };
        while (chrome.chars().count() as f32) < pad_cols as f32 {
            chrome.push(' ');
        }
        // Minimize, maximize, close glyphs, spaced to the button zones.
        let chrome_spans = [
            (chrome, accent),
            ("  \u{2014}  ".to_string(), fg),       // minimize  —
            (" \u{25a1}  ".to_string(), fg),        // maximize  □
            (" \u{2715} ".to_string(), signal_red), // close  ✕
        ];
        gpu.chrome_buffer.set_rich_text(
            &mut gpu.font_system,
            chrome_spans.iter().map(|(s, col)| {
                (
                    s.as_str(),
                    Attrs::new().family(Family::Monospace).color(*col),
                )
            }),
            &Attrs::new().family(Family::Monospace).color(fg),
            Shaping::Advanced,
            None,
        );
        gpu.chrome_buffer
            .shape_until_scroll(&mut gpu.font_system, false);

        if let Some(pt) = &palette_text {
            gpu.palette_buffer.set_text(
                &mut gpu.font_system,
                pt,
                &Attrs::new().family(Family::Monospace).color(accent),
                Shaping::Advanced,
                None,
            );
            gpu.palette_buffer
                .shape_until_scroll(&mut gpu.font_system, false);
        }

        let frame = match gpu.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(t)
            | wgpu::CurrentSurfaceTexture::Suboptimal(t) => t,
            _ => {
                gpu.reconfigure();
                return;
            }
        };
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        gpu.viewport.update(
            &gpu.queue,
            Resolution {
                width: gpu.surface_config.width,
                height: gpu.surface_config.height,
            },
        );

        let w = gpu.surface_config.width as i32;
        let h = gpu.surface_config.height as i32;
        let mut areas = vec![
            // Title bar chrome.
            TextArea {
                buffer: &gpu.chrome_buffer,
                left: 6.0,
                top: 6.0,
                scale: 1.0,
                bounds: TextBounds {
                    left: 0,
                    top: 0,
                    right: w,
                    bottom: TITLEBAR_H as i32,
                },
                default_color: fg,
                custom_glyphs: &[],
            },
            // Terminal grid.
            TextArea {
                buffer: &gpu.grid_buffer,
                left: 8.0,
                top: TITLEBAR_H + 2.0,
                scale: 1.0,
                bounds: TextBounds {
                    left: 0,
                    top: TITLEBAR_H as i32,
                    right: w,
                    bottom: h,
                },
                default_color: fg,
                custom_glyphs: &[],
            },
        ];
        if palette_text.is_some() {
            // Centered command-palette overlay.
            areas.push(TextArea {
                buffer: &gpu.palette_buffer,
                left: (w as f32 * 0.25).max(40.0),
                top: TITLEBAR_H + 40.0,
                scale: 1.0,
                bounds: TextBounds {
                    left: 0,
                    top: TITLEBAR_H as i32,
                    right: w,
                    bottom: h,
                },
                default_color: accent,
                custom_glyphs: &[],
            });
        }
        let prepared = gpu.text_renderer.prepare(
            &gpu.device,
            &gpu.queue,
            &mut gpu.font_system,
            &mut gpu.atlas,
            &gpu.viewport,
            areas,
            &mut gpu.swash_cache,
        );
        if let Err(e) = prepared {
            tracing::error!("glyphon prepare failed: {e}");
            return;
        }

        // Prepare inline-image quads (uploaded textures + vertex buffers).
        let prepared_images =
            gpu.image_renderer
                .prepare(&gpu.device, &gpu.queue, w as f32, h as f32, &image_quads);

        let mut encoder = gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("c0pl4nd"),
            });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("text"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(bg),
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            let _ = gpu
                .text_renderer
                .render(&gpu.atlas, &gpu.viewport, &mut pass);
            // Draw inline images over the grid.
            gpu.image_renderer.draw(&mut pass, &prepared_images);
        }
        gpu.queue.submit(std::iter::once(encoder.finish()));
        frame.present();
        gpu.atlas.trim();
    }
}

impl Gpu {
    async fn new(window: Arc<Window>, font_size: f32) -> Result<Gpu> {
        let size = window.inner_size();
        let instance = wgpu::Instance::default();
        let surface = instance.create_surface(window.clone())?;
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::LowPower,
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
            })
            .await?;
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("c0pl4nd-device"),
                ..Default::default()
            })
            .await?;

        let caps = surface.get_capabilities(&adapter);
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|f| f.is_srgb())
            .unwrap_or(caps.formats[0]);
        let surface_config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.width.max(1),
            height: size.height.max(1),
            // Prefer Mailbox (low-latency triple-buffer) over Fifo when
            // supported — saves up to ~1-2 frames of lag (research perf P0).
            present_mode: if caps.present_modes.contains(&wgpu::PresentMode::Mailbox) {
                wgpu::PresentMode::Mailbox
            } else {
                wgpu::PresentMode::Fifo
            },
            alpha_mode: caps.alpha_modes[0],
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &surface_config);

        let mut font_system = FontSystem::new();
        let swash_cache = SwashCache::new();
        let cache = Cache::new(&device);
        let viewport = Viewport::new(&device, &cache);
        let mut atlas = TextAtlas::new(&device, &queue, &cache, format);
        let text_renderer =
            TextRenderer::new(&mut atlas, &device, wgpu::MultisampleState::default(), None);
        let metrics = Metrics::new(font_size.max(8.0), LINE_HEIGHT.max(font_size + 2.0));
        let mut grid_buffer = Buffer::new(&mut font_system, metrics);
        grid_buffer.set_size(
            &mut font_system,
            Some(size.width as f32),
            Some(size.height as f32),
        );
        let mut chrome_buffer = Buffer::new(&mut font_system, metrics);
        chrome_buffer.set_size(&mut font_system, Some(size.width as f32), Some(TITLEBAR_H));
        let mut palette_buffer = Buffer::new(&mut font_system, metrics);
        palette_buffer.set_size(
            &mut font_system,
            Some(size.width as f32),
            Some(size.height as f32),
        );
        let image_renderer = crate::image_render::ImageRenderer::new(&device, format);

        Ok(Gpu {
            window,
            device,
            queue,
            surface,
            surface_config,
            font_system,
            swash_cache,
            atlas,
            viewport,
            text_renderer,
            grid_buffer,
            chrome_buffer,
            palette_buffer,
            image_renderer,
        })
    }

    fn resize(&mut self, width: u32, height: u32) {
        self.surface_config.width = width;
        self.surface_config.height = height;
        self.surface.configure(&self.device, &self.surface_config);
        self.grid_buffer.set_size(
            &mut self.font_system,
            Some(width as f32),
            Some(height as f32),
        );
        self.chrome_buffer
            .set_size(&mut self.font_system, Some(width as f32), Some(TITLEBAR_H));
    }

    fn reconfigure(&mut self) {
        self.surface.configure(&self.device, &self.surface_config);
    }
}

/// Resolve a cell's foreground [`c0pl4nd_core::Color`] to an RGB triple.
fn resolve_fg(
    color: c0pl4nd_core::Color,
    theme: &Theme,
    default_rgb: (u8, u8, u8),
) -> (u8, u8, u8) {
    match color {
        c0pl4nd_core::Color::Default => default_rgb,
        c0pl4nd_core::Color::Indexed(i) => theme.ansi(i),
        c0pl4nd_core::Color::Rgb(r, g, b) => (r, g, b),
    }
}

/// Approximate sRGB(0-255) → linear(0-1) for the wgpu clear color.
fn srgb_to_linear(c: u8) -> f64 {
    let s = c as f64 / 255.0;
    if s <= 0.04045 {
        s / 12.92
    } else {
        ((s + 0.055) / 1.055).powf(2.4)
    }
}

/// Map a key press to the bytes to send to the PTY.
fn key_to_bytes(key: &Key, text: &Option<winit::keyboard::SmolStr>) -> Option<Vec<u8>> {
    match key {
        Key::Named(NamedKey::Enter) => Some(vec![b'\r']),
        Key::Named(NamedKey::Backspace) => Some(vec![0x7f]),
        Key::Named(NamedKey::Tab) => Some(vec![b'\t']),
        Key::Named(NamedKey::Escape) => Some(vec![0x1b]),
        Key::Named(NamedKey::Space) => Some(vec![b' ']),
        Key::Named(NamedKey::ArrowUp) => Some(b"\x1b[A".to_vec()),
        Key::Named(NamedKey::ArrowDown) => Some(b"\x1b[B".to_vec()),
        Key::Named(NamedKey::ArrowRight) => Some(b"\x1b[C".to_vec()),
        Key::Named(NamedKey::ArrowLeft) => Some(b"\x1b[D".to_vec()),
        _ => text.as_ref().map(|s| s.as_bytes().to_vec()),
    }
}

/// Load a theme file from the bundled themes directory (next to the binary or
/// in the source tree during development).
fn load_theme(name: &str) -> Option<Theme> {
    let mut candidates: Vec<std::path::PathBuf> =
        vec![std::path::PathBuf::from("assets/themes").join(format!("{name}.toml"))];
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            candidates.push(parent.join("assets/themes").join(format!("{name}.toml")));
        }
    }
    // Themes contributed by installed declarative plugins, matched by file stem.
    if let Some(dir) = c0pl4nd_core::plugin::default_plugins_dir() {
        let registry = c0pl4nd_core::plugin::PluginRegistry::load(&dir);
        for p in registry.contributed_theme_paths() {
            if p.file_stem().and_then(|s| s.to_str()) == Some(name) {
                candidates.push(p);
            }
        }
    }
    for c in candidates {
        if let Ok(t) = Theme::load_from(&c) {
            return Some(t);
        }
    }
    None
}
