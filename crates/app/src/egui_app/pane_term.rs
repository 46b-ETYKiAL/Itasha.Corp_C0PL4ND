//! Per-pane live terminal state for the egui shell (Milestone 2).
//!
//! Each open pane in the [`super::grid`] tree is backed by a [`PaneTerm`]: a
//! live [`c0pl4nd_core::Session`] (PTY + reader thread + shared [`Terminal`]),
//! the active [`Theme`], and the cell metrics that map a pixel rect onto a
//! `(cols, rows)` grid. This module is the testable, bug-prone CORE of
//! Milestone 2:
//!
//! - [`PaneTerm::forward_key`] / [`PaneTerm::write_bytes`] push input to the PTY
//!   via the SHARED [`c0pl4nd_core::term::encode_key`] (the SAME escape-sequence
//!   encoder the winit shell uses — never re-derived here).
//! - [`PaneTerm::resize_to_px`] recomputes `(cols, rows)` from a pixel rect and
//!   resizes the PTY + reflows the grid, but ONLY when the dimensions actually
//!   change (debounced).
//! - [`PaneTerm::grid_spans`] snapshots the visible grid into per-row colour
//!   runs ready for a glyphon `Buffer`, reusing [`Theme::cell_colors`] so the
//!   foreground/background/inverse handling matches the winit renderer exactly.
//!
//! The glyphon GPU paint itself lives in [`super::term_render`]; this module is
//! UI-toolkit-free (no egui, no wgpu) so it can be driven headlessly with
//! simulated input — which is exactly the "typing reaches the PTY and the grid
//! updates" class of bug Milestone 2 must guard against.

use c0pl4nd_core::term::{encode_key, KeyModifiers, LogicalKey, MouseMode};
use c0pl4nd_core::{Session, Theme};

/// A foreground colour run: a string of consecutive same-colour glyphs and the
/// RGB triple they render in. The egui paint layer turns these into glyphon
/// `Attrs`; keeping the type as a plain `(String, (u8,u8,u8))` keeps this module
/// free of any glyphon/egui dependency (so it stays headlessly testable).
pub type ColorRun = (String, (u8, u8, u8));

/// The cell metrics (in physical pixels) used to map a pane's pixel rect onto a
/// terminal `(cols, rows)` grid. Derived from the glyphon font metrics; a
/// monospace cell is `advance_w` wide and `line_h` tall.
#[derive(Debug, Clone, Copy)]
pub struct CellMetrics {
    /// Horizontal advance of one monospace cell, in physical pixels.
    pub advance_w: f32,
    /// Line height of one cell, in physical pixels.
    pub line_h: f32,
}

impl CellMetrics {
    /// Compute `(cols, rows)` for a pane of `px_w` × `px_h` physical pixels,
    /// each clamped to at least 1 (a degenerate zero-size pane still gets a
    /// 1×1 grid rather than a panic / zero-division).
    pub fn cols_rows(&self, px_w: f32, px_h: f32) -> (u16, u16) {
        let cols = (px_w / self.advance_w.max(1.0)).floor().max(1.0) as u16;
        let rows = (px_h / self.line_h.max(1.0)).floor().max(1.0) as u16;
        (cols, rows)
    }
}

/// One pane's live terminal. Owns the PTY session and the rendering inputs the
/// egui shell needs each frame.
pub struct PaneTerm {
    /// The live PTY-backed session (None only if the shell failed to spawn —
    /// see [`PaneTerm::error`]).
    session: Option<Session>,
    /// A human-readable spawn error, shown as a fallback label when `session`
    /// is `None`. Never panics on a failed spawn — the pane degrades to a label.
    error: Option<String>,
    /// The active colour theme (glyph colours come from here, NOT egui Visuals).
    theme: Theme,
    /// The last `(cols, rows)` the PTY was sized to — used to debounce resizes.
    size: (u16, u16),
}

impl PaneTerm {
    /// Spawn a pane backed by the platform default shell at `(cols, rows)`.
    /// Never panics: a spawn failure yields a pane whose [`PaneTerm::error`] is
    /// set and whose body renders an error label instead of a grid.
    pub fn spawn(theme: Theme, cols: u16, rows: u16) -> Self {
        match Session::spawn_shell(None, rows, cols) {
            Ok(session) => Self {
                session: Some(session),
                error: None,
                theme,
                size: (cols, rows),
            },
            Err(e) => Self {
                session: None,
                error: Some(format!("shell failed to start: {e}")),
                theme,
                size: (cols, rows),
            },
        }
    }

    /// Spawn a pane running an explicit program (deterministic tests). Mirrors
    /// [`Session::spawn_program`]; same no-panic degradation on failure.
    ///
    /// `allow(dead_code)`: consumed by the `egui_terminal` interaction-test
    /// binary (via `#[path]`), not by the shipping `c0pl4nd-egui` binary — the
    /// same deliberate test-facing-public-API pattern the chrome accessors use.
    #[allow(dead_code)]
    pub fn spawn_program(theme: Theme, program: &str, args: &[&str], cols: u16, rows: u16) -> Self {
        match Session::spawn_program(program, args, rows, cols) {
            Ok(session) => Self {
                session: Some(session),
                error: None,
                theme,
                size: (cols, rows),
            },
            Err(e) => Self {
                session: None,
                error: Some(format!("program failed to start: {e}")),
                theme,
                size: (cols, rows),
            },
        }
    }

    /// The spawn error, if the shell could not start. `None` for a live pane.
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// Whether the backing shell is still running. `false` for a failed-spawn
    /// pane or one whose child has exited. (Test/observation API — see
    /// `spawn_program` note.)
    #[allow(dead_code)]
    pub fn is_alive(&self) -> bool {
        self.session.as_ref().is_some_and(Session::is_alive)
    }

    /// The current PTY grid size `(cols, rows)`. (Test/observation API.)
    #[allow(dead_code)]
    pub fn size(&self) -> (u16, u16) {
        self.size
    }

    /// Whether the terminal currently requests DECCKM application-cursor mode
    /// (`?1`) — drives whether arrows encode SS3 vs CSI. Locks the terminal
    /// briefly; defaults to `false` if the lock is poisoned.
    fn app_cursor(&self) -> bool {
        let Some(session) = &self.session else {
            return false;
        };
        session
            .terminal()
            .lock()
            .map(|t| t.application_cursor_keys())
            .unwrap_or(false)
    }

    /// The terminal's active mouse-reporting mode (`?1000` / `?1002` / `?1003`),
    /// as requested by the focused application (vim/tmux/htop grab the mouse this
    /// way). Locks the terminal briefly, mirroring
    /// [`app_cursor`](Self::app_cursor), and returns [`MouseMode::Off`] when there
    /// is no live session or the lock is poisoned. Cheap and non-panicking, so it
    /// is safe to call once per frame from the status-bar paint.
    pub fn mouse_mode(&self) -> MouseMode {
        let Some(session) = &self.session else {
            return MouseMode::Off;
        };
        session
            .terminal()
            .lock()
            .map(|t| t.mouse_mode())
            .unwrap_or(MouseMode::Off)
    }

    /// The terminal's current window title, as set by the running program via
    /// an OSC 0/2 escape (`ESC ] 0 ; <title> BEL`). Locks the terminal briefly,
    /// mirroring [`app_cursor`](Self::app_cursor) / [`mouse_mode`](Self::mouse_mode),
    /// and returns the trimmed title when non-empty. Returns `None` when there is
    /// no live session, the lock is poisoned, or the program has not set a title
    /// yet (empty) — so the tab strip can fall back to its `pane {id}` label.
    pub fn title(&self) -> Option<String> {
        let session = self.session.as_ref()?;
        let term = session.terminal();
        let guard = term.lock().ok()?;
        let title = guard.title().trim();
        if title.is_empty() {
            None
        } else {
            Some(title.to_string())
        }
    }

    /// The exit code of the focused program's most recently finished command,
    /// derived from OSC 133 `D` command-end marks (shell prompt integration).
    /// Locks the terminal briefly, mirroring [`title`](Self::title) /
    /// [`mouse_mode`](Self::mouse_mode), and returns the core accessor's
    /// double-`Option`:
    ///
    /// - outer `None` — no live session, the lock is poisoned, or no command
    ///   has finished yet (so the status bar shows NO indicator);
    /// - inner `Option` — the OSC-133-reported exit code (`Some(0)` success,
    ///   `Some(code)` failure), or `None` when the shell reported no code.
    ///
    /// Cheap and non-panicking, so it is safe to call once per frame from the
    /// status-bar paint.
    pub fn last_command_exit_code(&self) -> Option<Option<i32>> {
        let session = self.session.as_ref()?;
        let term = session.terminal();
        let guard = term.lock().ok()?;
        guard.last_command_exit_code()
    }

    /// Write raw bytes straight to the PTY (used for pasted text). Best-effort:
    /// a closed/dead session silently drops the write rather than panicking.
    pub fn write_bytes(&mut self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        if let Some(session) = &mut self.session {
            let _ = session.write_input(bytes);
        }
    }

    /// Encode a logical key + modifiers via the SHARED core encoder and write
    /// the resulting bytes to the PTY. Returns the bytes written (empty when the
    /// key encodes nothing or the session is dead) so tests can assert exactly
    /// what reached the wire.
    pub fn forward_key(&mut self, key: &LogicalKey, mods: KeyModifiers) -> Vec<u8> {
        let app_cursor = self.app_cursor();
        match encode_key(key, app_cursor, mods) {
            Some(bytes) => {
                self.write_bytes(&bytes);
                bytes
            }
            None => Vec::new(),
        }
    }

    /// Resize the PTY + reflow the grid to fit a pane of `px_w` × `px_h`
    /// physical pixels, given the cell metrics. DEBOUNCED: does nothing when the
    /// computed `(cols, rows)` equals the current size, so a sub-pixel drag does
    /// not thrash the PTY. Returns the new `(cols, rows)` when a resize was
    /// applied, `None` when it was a no-op.
    pub fn resize_to_px(
        &mut self,
        px_w: f32,
        px_h: f32,
        metrics: CellMetrics,
    ) -> Option<(u16, u16)> {
        let (cols, rows) = metrics.cols_rows(px_w, px_h);
        self.resize(cols, rows)
    }

    /// Resize the PTY + grid to an explicit `(cols, rows)`. Debounced against
    /// the current size (no-op when unchanged). Returns the new size when
    /// applied. A failed-spawn pane records the size but performs no PTY op.
    pub fn resize(&mut self, cols: u16, rows: u16) -> Option<(u16, u16)> {
        let cols = cols.max(1);
        let rows = rows.max(1);
        if (cols, rows) == self.size {
            return None;
        }
        self.size = (cols, rows);
        if let Some(session) = &mut self.session {
            let _ = session.resize(rows, cols);
        }
        Some((cols, rows))
    }

    /// Snapshot the visible grid into per-row colour runs ready for a glyphon
    /// `Buffer::set_rich_text`. Each row is grouped into runs of consecutive
    /// same-colour glyphs (cheap; one `Attrs` per run, not per glyph), and the
    /// rows are joined by `'\n'` so glyphon lays them out as lines. Honours
    /// DECSCNM reverse-screen and SGR inverse via [`Theme::cell_colors`].
    /// Returns `None` only when the session is dead (no terminal to read).
    pub fn grid_spans(&self) -> Option<Vec<ColorRun>> {
        let session = self.session.as_ref()?;
        let term = session.terminal();
        let guard = term.lock().ok()?;
        let theme_fg =
            c0pl4nd_core::theme::parse_hex(&self.theme.foreground).unwrap_or((232, 230, 240));
        let theme_bg =
            c0pl4nd_core::theme::parse_hex(&self.theme.background).unwrap_or((18, 18, 18));
        // DECSCNM (`?5`): reverse-video screen swaps the default fg/bg.
        let (default_fg, default_bg) = if guard.reverse_screen() {
            (theme_bg, theme_fg)
        } else {
            (theme_fg, theme_bg)
        };
        let rows = guard.display_rows();
        let mut out: Vec<ColorRun> = Vec::new();
        for row in &rows {
            let mut run = String::new();
            let mut run_color: Option<(u8, u8, u8)> = None;
            for cell in row {
                let (fg, _bg) = self.theme.cell_colors(cell, default_fg, default_bg);
                if run_color != Some(fg) {
                    if let Some(pc) = run_color.take() {
                        out.push((std::mem::take(&mut run), pc));
                    }
                    run_color = Some(fg);
                }
                run.push(cell.c);
            }
            if let Some(pc) = run_color {
                out.push((run, pc));
            }
            out.push(("\n".to_string(), default_fg));
        }
        Some(out)
    }

    /// The cursor cell as `(row, col)` (0-based, within the visible grid) when
    /// it should be DRAWN: the session is alive, the cursor is visible (DECTCEM
    /// `?25`), and the view is not scrolled back into history (a terminal hides
    /// the cursor while you scroll up). `None` otherwise — the caller draws no
    /// caret.
    pub fn cursor_cell(&self) -> Option<(usize, usize)> {
        let session = self.session.as_ref()?;
        let term = session.terminal();
        let guard = term.lock().ok()?;
        if !guard.is_cursor_visible() {
            return None;
        }
        guard.cursor_position()
    }

    /// The visible grid as plain text (used by tests to assert PTY output landed
    /// on screen, and as a headless render fallback). `None` for a dead session.
    pub fn grid_text(&self) -> Option<String> {
        let session = self.session.as_ref()?;
        let term = session.terminal();
        let guard = term.lock().ok()?;
        Some(guard.grid().to_text())
    }

    /// The active theme's default background as an `(r,g,b)` triple — the colour
    /// the egui pane body clears to behind the glyphon text.
    pub fn background_rgb(&self) -> (u8, u8, u8) {
        c0pl4nd_core::theme::parse_hex(&self.theme.background).unwrap_or((18, 18, 18))
    }

    /// Swap the pane's active colour theme. Existing panes hold their OWN theme
    /// clone (both [`grid_spans`](Self::grid_spans) glyph colours AND
    /// [`background_rgb`](Self::background_rgb) resolve from it), so a theme
    /// change in settings must be propagated here for the live panes to repaint
    /// in the new colours — otherwise the picker appears to do nothing.
    pub fn set_theme(&mut self, theme: Theme) {
        self.theme = theme;
    }

    /// Test-only handle to the shared terminal, so a unit test can drive escape
    /// sequences (e.g. `?1000h`) directly into the parser — deterministically,
    /// without depending on the asynchronous PTY reader thread (which would make
    /// the test flaky). Returns `None` for a failed-spawn pane.
    #[cfg(test)]
    fn terminal_for_test(
        &self,
    ) -> Option<std::sync::Arc<std::sync::Mutex<c0pl4nd_core::Terminal>>> {
        self.session.as_ref().map(Session::terminal)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn void_theme() -> Theme {
        Theme::builtin_void()
    }

    /// A theme change must reach a live pane's rendered colours. The pane holds
    /// its own theme clone (background + glyphs resolve from it), so `set_theme`
    /// must update `background_rgb` — this is the propagation that makes the
    /// settings theme-picker visibly change the panes.
    #[test]
    fn set_theme_repaints_pane_background() {
        let mut pane = PaneTerm::spawn(Theme::builtin_named("itasha-void").unwrap(), 80, 24);
        let before = pane.background_rgb();
        pane.set_theme(Theme::builtin_named("ghost-paper").unwrap());
        let after = pane.background_rgb();
        assert_ne!(
            before, after,
            "set_theme must change the pane's resolved background colour"
        );
    }

    #[test]
    fn cell_metrics_cols_rows_floors_and_clamps() {
        let m = CellMetrics {
            advance_w: 10.0,
            line_h: 20.0,
        };
        assert_eq!(m.cols_rows(105.0, 81.0), (10, 4));
        // Degenerate zero size still yields a 1x1 grid (no zero-division panic).
        assert_eq!(m.cols_rows(0.0, 0.0), (1, 1));
    }

    #[test]
    fn forward_key_encodes_via_shared_core_path() {
        // Drive a real shell pane and assert the bytes the key produced — the
        // SAME bytes the core encoder yields, proving reuse (not re-derivation).
        let pane = PaneTerm::spawn(void_theme(), 80, 24);
        // If the shell could not spawn on this box, the encode path is still
        // exercised (write is a no-op but the returned bytes are asserted).
        let mut pane = pane;
        let bytes = pane.forward_key(&LogicalKey::Text("x".into()), KeyModifiers::NONE);
        assert_eq!(bytes, b"x".to_vec(), "text key forwards its UTF-8 bytes");
        let enter = pane.forward_key(&LogicalKey::Enter, KeyModifiers::NONE);
        assert_eq!(enter, b"\r".to_vec(), "Enter forwards CR");
    }

    #[test]
    fn resize_is_debounced() {
        let mut pane = PaneTerm::spawn(void_theme(), 80, 24);
        assert_eq!(pane.size(), (80, 24));
        // Same size → no-op.
        assert_eq!(pane.resize(80, 24), None);
        // Different size → applied.
        assert_eq!(pane.resize(40, 12), Some((40, 12)));
        assert_eq!(pane.size(), (40, 12));
        // Idempotent again.
        assert_eq!(pane.resize(40, 12), None);
    }

    /// The status-bar mouse-mode badge is driven by [`PaneTerm::mouse_mode`].
    /// A fresh pane reports [`MouseMode::Off`] (no badge); after the focused app
    /// enables DEC `?1000` mouse tracking the accessor must reflect it (badge
    /// shown). Drives the shared terminal directly — deterministic, no reliance
    /// on the async PTY reader thread.
    #[test]
    fn mouse_mode_reflects_terminal_state() {
        let pane = PaneTerm::spawn(void_theme(), 80, 24);
        // Default: no mouse reporting → no badge.
        assert_eq!(
            pane.mouse_mode(),
            MouseMode::Off,
            "a fresh pane must report MouseMode::Off (badge hidden)"
        );
        // If the shell could not spawn on this box, there is no terminal to
        // drive; the Off default above is still the meaningful assertion.
        let Some(term) = pane.terminal_for_test() else {
            return;
        };
        // App enables ?1000 (normal button tracking) — the badge-trigger state.
        term.lock().unwrap().advance(b"\x1b[?1000h");
        assert_eq!(
            pane.mouse_mode(),
            MouseMode::Normal,
            "after ?1000h the accessor must report Normal (badge shown)"
        );
        // ?1003 (any-event) is also a reporting mode (badge stays shown).
        term.lock().unwrap().advance(b"\x1b[?1003h");
        assert_eq!(pane.mouse_mode(), MouseMode::AnyEvent);
        // App disables tracking → back to Off (badge hidden again).
        term.lock().unwrap().advance(b"\x1b[?1003l");
        assert_eq!(
            pane.mouse_mode(),
            MouseMode::Off,
            "after ?1003l the accessor must report Off (badge hidden)"
        );
    }

    /// The tab strip is driven by [`PaneTerm::title`]. A fresh pane has no
    /// program-set title yet, so the accessor returns `None` (the strip falls
    /// back to `pane {id}`); after the running program emits an OSC 0 title
    /// (`ESC ] 0 ; <title> BEL`) the accessor must reflect it. Drives the shared
    /// terminal directly — deterministic, no reliance on the async PTY reader.
    #[test]
    fn title_reflects_osc_set_title() {
        let pane = PaneTerm::spawn(void_theme(), 80, 24);
        // Default: no program-set title → None (tab falls back to "pane {id}").
        assert_eq!(
            pane.title(),
            None,
            "a fresh pane must report no title (tab strip uses the pane-id fallback)"
        );
        // If the shell could not spawn on this box there is no terminal to
        // drive; the None default above is still the meaningful assertion.
        let Some(term) = pane.terminal_for_test() else {
            return;
        };
        // The program sets its window title via OSC 0 (BEL-terminated).
        term.lock().unwrap().advance(b"\x1b]0;mytitle\x07");
        assert_eq!(
            pane.title(),
            Some("mytitle".to_string()),
            "after ESC]0;mytitle BEL the accessor must report the OSC title"
        );
    }

    /// The status-bar exit-code indicator is driven by
    /// [`PaneTerm::last_command_exit_code`]. A fresh pane has no finished
    /// command (no OSC 133 `D` mark yet), so the accessor returns `None` (no
    /// indicator). After the running shell emits `OSC 133 ; D ; <code>` the
    /// accessor must reflect the latest code. Drives the shared terminal
    /// directly — deterministic, no reliance on the async PTY reader or on a
    /// real prompt-integrated shell.
    #[test]
    fn last_command_exit_code_reflects_osc133_command_end() {
        let pane = PaneTerm::spawn(void_theme(), 80, 24);
        // Default: no finished command → None (status bar shows no indicator).
        assert_eq!(
            pane.last_command_exit_code(),
            None,
            "a fresh pane must report no finished command (no indicator)"
        );
        // If the shell could not spawn on this box there is no terminal to
        // drive; the None default above is still the meaningful assertion.
        let Some(term) = pane.terminal_for_test() else {
            return;
        };
        // The shell reports a successful command end (`OSC 133 ; D ; 0`).
        term.lock().unwrap().advance(b"\x1b]133;D;0\x07");
        assert_eq!(
            pane.last_command_exit_code(),
            Some(Some(0)),
            "after ESC]133;D;0 BEL the accessor must report success (code 0)"
        );
        // A subsequent failing command end (`OSC 133 ; D ; 1`) supersedes it.
        term.lock().unwrap().advance(b"\x1b]133;D;1\x07");
        assert_eq!(
            pane.last_command_exit_code(),
            Some(Some(1)),
            "the accessor must report the MOST RECENT command's exit code"
        );
    }

    #[test]
    fn resize_to_px_maps_through_metrics() {
        let mut pane = PaneTerm::spawn(void_theme(), 80, 24);
        let m = CellMetrics {
            advance_w: 10.0,
            line_h: 20.0,
        };
        // 200px / 10 = 20 cols ; 200px / 20 = 10 rows.
        assert_eq!(pane.resize_to_px(200.0, 200.0, m), Some((20, 10)));
        assert_eq!(pane.size(), (20, 10));
    }
}
