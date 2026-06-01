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

use c0pl4nd_core::term::{encode_key, KeyModifiers, LogicalKey};
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
    /// A sane fallback used before the GPU font metrics are known (and in
    /// headless tests). Roughly a 14px monospace cell.
    pub const FALLBACK: CellMetrics = CellMetrics {
        advance_w: 8.0,
        line_h: 18.0,
    };

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
}

#[cfg(test)]
mod tests {
    use super::*;

    fn void_theme() -> Theme {
        Theme::builtin_void()
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
