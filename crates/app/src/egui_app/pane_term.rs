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
//! - [`PaneTerm::grid_rows`] snapshots the visible grid into per-row colour
//!   runs ready for the paint layer, reusing [`Theme::cell_colors`] so the
//!   foreground/background/inverse handling matches the winit renderer exactly.
//!
//! The glyphon GPU paint itself lives in [`super::term_render`]; this module is
//! UI-toolkit-free (no egui, no wgpu) so it can be driven headlessly with
//! simulated input — which is exactly the "typing reaches the PTY and the grid
//! updates" class of bug Milestone 2 must guard against.

use std::cell::RefCell;
use std::rc::Rc;

use c0pl4nd_core::term::{
    encode_key, encode_key_kitty, ColorSet, KeyEventKind, KeyModifiers, LogicalKey, MouseButton,
    MouseEventKind, MouseMode, MouseModifiers,
};
use c0pl4nd_core::{Session, Theme};

/// A foreground colour run: a string of consecutive same-colour glyphs and the
/// RGB triple they render in. The egui paint layer turns these into glyphon
/// `Attrs`; keeping the type as a plain `(String, (u8,u8,u8))` keeps this module
/// free of any glyphon/egui dependency (so it stays headlessly testable).
pub type ColorRun = (String, (u8, u8, u8));

/// Damage-gated cache of the visible grid as per-row colour runs (the output of
/// [`PaneTerm::grid_rows`]). The renderer calls `grid_rows` every frame; this
/// cache lets an UNCHANGED pane (the blinking-cursor / idle-effect case) skip the
/// whole snapshot-and-group pass and reuse the previous frame's rows.
///
/// Validity is keyed on everything that can change the *rendered* runs without
/// going through a grid cell-write: the grid's own per-row damage bits
/// ([`Grid::is_damaged`]), the scrollback `view_offset`, the grid dimensions, the
/// DECSCNM reverse-screen flag (swaps default fg/bg without touching cells), and
/// the active theme (set out-of-band via [`PaneTerm::set_theme`], which clears
/// this cache). Anything that edits a cell already marks its row dirty, so a
/// cell change forces a rebuild.
struct RowSpanCache {
    rows: Rc<Vec<Vec<ColorRun>>>,
    view_offset: usize,
    cols: usize,
    grid_rows: usize,
    reverse: bool,
    valid: bool,
}

impl Default for RowSpanCache {
    fn default() -> Self {
        Self {
            rows: Rc::new(Vec::new()),
            view_offset: 0,
            cols: 0,
            grid_rows: 0,
            reverse: false,
            valid: false,
        }
    }
}

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

/// OS-level side effects drained from one pane's terminal in a single frame,
/// returned by [`PaneTerm::pump_host_effects`] for the app shell to apply
/// globally. PTY query replies (DA / DSR / cursor-position / OSC color-query /
/// focus reports) are written straight back to the originating pane's own PTY
/// inside `pump_host_effects`, so they are NOT surfaced here — only the effects
/// that touch host-global state (the OS clipboard, the live theme, the taskbar)
/// are returned.
#[derive(Default)]
pub struct HostEffects {
    /// OSC 52 clipboard-write payloads (plaintext; the zeroizing buffer was
    /// drained into these owned `String`s). The app writes them to the OS
    /// clipboard.
    pub clipboard_writes: Vec<String>,
    /// OSC 4 / 10 / 11 / 12 / 104 color set/reset requests. The app applies each
    /// to its live theme and repaints.
    pub color_sets: Vec<ColorSet>,
    /// `true` if a desktop notification (OSC 9 / OSC 777) fired this drain. The
    /// app requests user attention (taskbar flash) when the window is unfocused.
    /// The notification TEXT is deliberately not surfaced — it can carry 2FA
    /// codes / secret URLs and must never be logged (privacy).
    pub notified: bool,
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
    /// Whether this pane's [`Session`] has had its UI-wake callback wired yet.
    /// The app wires it once (it needs an `egui::Context`, only available from a
    /// frame); the flag makes [`PaneTerm::wire_wake`] idempotent so the per-frame
    /// sweep does not re-register a fresh closure every frame.
    wake_wired: bool,
    /// Damage-gated cache of the last `grid_rows()` snapshot. Interior-mutable so
    /// the per-frame `grid_rows(&self)` read path can refresh it without forcing a
    /// `&mut` on every chrome accessor. `PaneTerm` lives on the UI thread only
    /// (the cross-thread boundary is the terminal's own `Mutex`), so a `RefCell`
    /// is sound here.
    span_cache: RefCell<RowSpanCache>,
}

impl PaneTerm {
    /// Spawn a pane backed by the platform default shell at `(cols, rows)`.
    /// Never panics: a spawn failure yields a pane whose [`PaneTerm::error`] is
    /// set and whose body renders an error label instead of a grid.
    ///
    /// Uses the canonical [`c0pl4nd_core::pty::DEFAULT_TERM`]; the shipping app
    /// goes through [`PaneTerm::spawn_with_term`] so the user's `term` config
    /// override is honoured.
    ///
    /// `allow(dead_code)`: the shipping `c0pl4nd-egui` binary spawns via
    /// [`PaneTerm::spawn_with_term`] (so the `term` config override applies); the
    /// no-override `spawn` is retained for the deterministic interaction tests.
    #[allow(dead_code)]
    pub fn spawn(theme: Theme, cols: u16, rows: u16) -> Self {
        Self::spawn_with_term(theme, cols, rows, None)
    }

    /// Like [`PaneTerm::spawn`] but with an explicit `TERM` override (the
    /// config-driven `term` key). `term = None` / `Some("")` uses the canonical
    /// [`c0pl4nd_core::pty::DEFAULT_TERM`]. This is the path the shipping
    /// `c0pl4nd-egui` binary takes so the child PTY's `TERM` matches the user's
    /// configuration.
    pub fn spawn_with_term(theme: Theme, cols: u16, rows: u16, term: Option<&str>) -> Self {
        match Session::spawn_shell_with_term(None, rows, cols, term) {
            Ok(session) => Self {
                session: Some(session),
                error: None,
                theme,
                size: (cols, rows),
                wake_wired: false,
                span_cache: RefCell::new(RowSpanCache::default()),
            },
            Err(e) => Self {
                session: None,
                error: Some(format!("shell failed to start: {e}")),
                theme,
                size: (cols, rows),
                wake_wired: false,
                span_cache: RefCell::new(RowSpanCache::default()),
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
                wake_wired: false,
                span_cache: RefCell::new(RowSpanCache::default()),
            },
            Err(e) => Self {
                session: None,
                error: Some(format!("program failed to start: {e}")),
                theme,
                size: (cols, rows),
                wake_wired: false,
                span_cache: RefCell::new(RowSpanCache::default()),
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

    /// Wire this pane's UI-wake callback exactly once. The reader thread invokes
    /// `wake` after each chunk of PTY output so the render loop can sleep when
    /// idle and repaint the instant output arrives. `make_wake` is a thunk so the
    /// (cheap) callback `Arc` is only built on the first call for a live pane —
    /// the per-frame sweep that calls this is a no-op after the first wiring (or
    /// for a failed-spawn pane with no session).
    pub fn wire_wake(&mut self, make_wake: impl FnOnce() -> c0pl4nd_core::WakeFn) {
        if self.wake_wired {
            return;
        }
        if let Some(session) = &self.session {
            session.set_wake_callback(make_wake());
            self.wake_wired = true;
        }
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

    /// Drain this pane's terminal-owed effects ONCE for the current frame.
    ///
    /// Two distinct destinations:
    /// - **PTY query replies** ([`Terminal::take_pty_response`]: device
    ///   attributes, cursor-position reports, OSC 4/10/11/12 color *queries*,
    ///   focus reports) are written STRAIGHT BACK to THIS pane's PTY — they are
    ///   answers this terminal owes the program running in it.
    /// - **Host-global effects** (OSC 52 clipboard writes, OSC 4/10/11/12/104
    ///   color *sets*, OSC 9/777 notifications) are returned in [`HostEffects`]
    ///   for the app shell to apply once.
    ///
    /// Also drains the `OSC 9 ; 4` taskbar-progress queue (currently no UI) so
    /// it cannot grow without bound while a build tool streams progress. Without
    /// this whole drain the egui shell silently dropped every reply AND leaked
    /// the unread queues — the legacy winit shell drained them but the egui
    /// rewrite never ported the wiring.
    ///
    /// No-op (empty effects) for a failed-spawn pane or a poisoned terminal lock.
    pub fn pump_host_effects(&mut self) -> HostEffects {
        let mut out = HostEffects::default();
        let Some(session) = self.session.as_mut() else {
            return out;
        };
        // `terminal()` clones the Arc, so the immutable borrow of `session` ends
        // immediately — the `write_input(&mut self)` below is then free to take
        // the mutable borrow. Compute the reply bytes under the lock, drop the
        // lock, THEN write.
        let term_arc = session.terminal();
        let response = {
            let Ok(mut term) = term_arc.lock() else {
                return out;
            };
            let response = term.take_pty_response();
            for mut cw in term.take_clipboard_writes() {
                // `ClipboardWrite` zeroizes its buffer on drop; take the text out
                // (leaving an empty buffer to drop) rather than moving the field
                // out of the Drop type.
                out.clipboard_writes.push(std::mem::take(&mut cw.text));
            }
            out.color_sets = term.take_color_sets();
            if !term.take_notifications().is_empty() {
                out.notified = true;
            }
            // Bounded-growth guard: drain the progress queue even though there is
            // no taskbar-progress UI yet (matches the legacy shell, which also
            // has none — but the legacy shell never let the queue accumulate).
            let _ = term.take_progress();
            response
        };
        if !response.is_empty() {
            let _ = session.write_input(&response);
        }
        out
    }

    /// Report a mouse event to the program running in this pane, IFF that
    /// program has grabbed the mouse (`?1000` / `?1002` / `?1003`, encoded per
    /// the negotiated `?1006`/`?1015` mode). Maps to the SHARED core encoder
    /// ([`Terminal::encode_mouse`]) so the wire bytes match the winit shell
    /// exactly. `col`/`row` are 1-based grid coordinates.
    ///
    /// Returns `true` when the event was encoded and written to the PTY — the
    /// caller then skips the local selection/scroll handling so the program owns
    /// the gesture. Returns `false` (a no-op) when mouse reporting is Off, the
    /// event encodes to nothing for the active mode, the lock is poisoned, or
    /// the pane has no live session.
    pub fn report_mouse(
        &mut self,
        button: MouseButton,
        mods: MouseModifiers,
        col: usize,
        row: usize,
        kind: MouseEventKind,
    ) -> bool {
        let Some(session) = self.session.as_mut() else {
            return false;
        };
        let term_arc = session.terminal();
        let bytes = {
            let Ok(term) = term_arc.lock() else {
                return false;
            };
            if term.mouse_mode() == MouseMode::Off {
                return false;
            }
            term.encode_mouse(button, mods, col, row, kind)
        };
        match bytes {
            Some(b) => {
                let _ = session.write_input(&b);
                true
            }
            None => false,
        }
    }

    /// Scroll the scrollback VIEW by `lines` (positive = back into older
    /// history, negative = forward toward the live bottom). Used for local
    /// mouse-wheel scrollback when the running program has NOT grabbed the mouse
    /// ([`MouseMode::Off`]). No-op on a poisoned lock or a dead pane.
    pub fn scroll_view(&mut self, lines: i32) {
        let Some(session) = self.session.as_ref() else {
            return;
        };
        if let Ok(mut term) = session.terminal().lock() {
            if lines > 0 {
                term.scroll_up_view(lines as usize);
            } else if lines < 0 {
                term.scroll_down_view((-lines) as usize);
            }
        }
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

    /// Write a clipboard paste to the PTY through the core paste-injection guard
    /// ([`c0pl4nd_core::Terminal::frame_paste`]): an embedded `ESC[201~` is
    /// stripped and the text is bracket-wrapped iff the program enabled `?2004`.
    /// This is the ONLY path egui pastes take — never `write_bytes(raw)` — so a
    /// hostile clipboard payload cannot break out of bracketed paste and inject
    /// commands (pastejacking). The terminal is also scrolled to the bottom so
    /// the paste lands at the prompt.
    pub fn write_paste(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        let Some(session) = &mut self.session else {
            return;
        };
        let bytes = match session.terminal().lock() {
            Ok(mut term) => {
                term.scroll_to_bottom();
                term.frame_paste(text)
            }
            Err(_) => return,
        };
        let _ = session.write_input(&bytes);
    }

    /// The terminal's current kitty-keyboard-protocol flags (`0` = not
    /// negotiated → legacy encoding). Locks the terminal briefly, mirroring
    /// [`app_cursor`](Self::app_cursor), and returns `0` when there is no live
    /// session or the lock is poisoned.
    fn kitty_flags(&self) -> u8 {
        let Some(session) = &self.session else {
            return 0;
        };
        session
            .terminal()
            .lock()
            .map(|t| t.kitty_keyboard_flags())
            .unwrap_or(0)
    }

    /// Whether the focused program negotiated the kitty REPORT-EVENT-TYPES
    /// flag (bit 2). The egui input loop reads this to decide whether to also
    /// forward key RELEASE / REPEAT events (default: press-only).
    pub fn kitty_reports_event_types(&self) -> bool {
        self.kitty_flags() & 2 != 0
    }

    /// Encode a key PRESS via the SHARED core encoder and write the bytes to the
    /// PTY. Thin wrapper over [`forward_key_event`](Self::forward_key_event) so
    /// existing callers (and tests) that only deal in presses are unchanged.
    pub fn forward_key(&mut self, key: &LogicalKey, mods: KeyModifiers) -> Vec<u8> {
        self.forward_key_event(key, mods, KeyEventKind::Press)
    }

    /// Encode a key event (press / repeat / release) and write the resulting
    /// bytes to the PTY. Returns the bytes written (empty when the key encodes
    /// nothing or the session is dead) so tests can assert exactly what reached
    /// the wire.
    ///
    /// When the running program negotiated the kitty keyboard protocol
    /// (`kitty_flags() != 0`), the CSI-u encoder is tried first and used when it
    /// produces a sequence; otherwise the encoding falls back to the legacy
    /// [`encode_key`] (which only ever encodes presses — a release with no kitty
    /// encoding yields no bytes).
    pub fn forward_key_event(
        &mut self,
        key: &LogicalKey,
        mods: KeyModifiers,
        kind: KeyEventKind,
    ) -> Vec<u8> {
        let flags = self.kitty_flags();
        if flags != 0 {
            if let Some(bytes) = encode_key_kitty(key, mods, flags, kind) {
                self.write_bytes(&bytes);
                return bytes;
            }
        }
        // Legacy fallback. Only presses and repeats produce legacy bytes; a
        // release with no kitty encoding writes nothing.
        if kind == KeyEventKind::Release {
            return Vec::new();
        }
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

    /// Snapshot the visible grid into per-row colour runs ready for the egui paint
    /// layer. Each row is grouped into runs of consecutive same-colour glyphs
    /// (cheap; one `Attrs` per run, not per glyph). Honours DECSCNM reverse-screen
    /// and SGR inverse via [`Theme::cell_colors`]. Returns `None` only when the
    /// session is dead (no terminal to read).
    ///
    /// **Damage-gated.** The grid tracks per-row damage; when nothing changed
    /// since the last call — no dirty rows, same scrollback `view_offset`, same
    /// dimensions, same reverse-screen flag — this returns the cached rows (an
    /// `Rc` clone, no grid work). That is the idle path: a pane with only a
    /// blinking cursor or a running CRT effect repaints from cache instead of
    /// re-grouping every row every frame (the cursor is an overlay drawn
    /// separately, not part of these runs, so it animates without dirtying the
    /// grid). On a miss it rebuilds, clears the grid's damage bits, and refreshes
    /// the cache. A theme change bypasses the grid (cells are unchanged), so
    /// [`PaneTerm::set_theme`] invalidates this cache explicitly.
    pub fn grid_rows(&self) -> Option<Rc<Vec<Vec<ColorRun>>>> {
        let session = self.session.as_ref()?;
        let term = session.terminal();
        let mut guard = term.lock().ok()?;

        let view_offset = guard.view_offset();
        let reverse = guard.reverse_screen();
        let cols = guard.grid().cols();
        let grid_rows = guard.grid().rows();
        let damaged = guard.grid().is_damaged();

        // Cache hit: nothing that affects the rendered runs has changed.
        {
            let cache = self.span_cache.borrow();
            if cache.valid
                && !damaged
                && cache.view_offset == view_offset
                && cache.cols == cols
                && cache.grid_rows == grid_rows
                && cache.reverse == reverse
            {
                return Some(Rc::clone(&cache.rows));
            }
        }

        // Miss: rebuild the per-row runs from the borrowing visible-rows iterator
        // (no whole-grid clone). History rows shorter than the grid width are
        // padded-on-read — columns at/past `row.len()` are treated as
        // `Cell::default()` so the output matches a width-padded grid without ever
        // materialising a padded `Vec`. Rows are grouped DIRECTLY into the final
        // `Vec<Vec<ColorRun>>` (one inner Vec per visible row) — no flat stream +
        // newline-split round-trip.
        let theme_fg =
            c0pl4nd_core::theme::parse_hex(&self.theme.foreground).unwrap_or((232, 230, 240));
        let theme_bg =
            c0pl4nd_core::theme::parse_hex(&self.theme.background).unwrap_or((18, 18, 18));
        // DECSCNM (`?5`): reverse-video screen swaps the default fg/bg.
        let (default_fg, default_bg) = if reverse {
            (theme_bg, theme_fg)
        } else {
            (theme_fg, theme_bg)
        };
        let default_cell = c0pl4nd_core::Cell::default();
        let mut rows_out: Vec<Vec<ColorRun>> = Vec::with_capacity(grid_rows);
        guard.for_visible_rows(|_, row| {
            let mut runs: Vec<ColorRun> = Vec::new();
            let mut run = String::new();
            let mut run_color: Option<(u8, u8, u8)> = None;
            for col in 0..cols {
                let cell = row.get(col).unwrap_or(&default_cell);
                let (fg, _bg) = self.theme.cell_colors(cell, default_fg, default_bg);
                if run_color != Some(fg) {
                    if let Some(pc) = run_color.take() {
                        runs.push((std::mem::take(&mut run), pc));
                    }
                    run_color = Some(fg);
                }
                run.push(cell.c);
            }
            if let Some(pc) = run_color {
                runs.push((run, pc));
            }
            // BiDi (F3-2): reorder this row's logical-order runs into VISUAL
            // order for right-to-left scripts (Arabic/Hebrew). The fast path
            // returns the LTR-only row UNCHANGED (zero cost); only a row that
            // actually contains RTL content is reordered. Display-only — the
            // logical grid/cells are untouched (see `super::bidi`).
            if let Some(visual) = super::bidi::reorder_runs_visual(&runs) {
                runs = visual;
            }
            rows_out.push(runs);
        });
        // The renderer has now consumed this frame's damage; clear it so the next
        // frame's `is_damaged()` reflects only writes that arrive after this point.
        guard.clear_damage();
        drop(guard);

        let rc = Rc::new(rows_out);
        *self.span_cache.borrow_mut() = RowSpanCache {
            rows: Rc::clone(&rc),
            view_offset,
            cols,
            grid_rows,
            reverse,
            valid: true,
        };
        Some(rc)
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
    /// clone (both [`grid_rows`](Self::grid_rows) glyph colours AND
    /// [`background_rgb`](Self::background_rgb) resolve from it), so a theme
    /// change in settings must be propagated here for the live panes to repaint
    /// in the new colours — otherwise the picker appears to do nothing.
    ///
    /// A theme change recolours every cell WITHOUT touching the grid (no cell
    /// write, no damage bit), so the [`grid_rows`](Self::grid_rows) damage cache
    /// would otherwise serve stale-coloured rows. Invalidate it here so the next
    /// frame rebuilds in the new colours.
    pub fn set_theme(&mut self, theme: Theme) {
        self.theme = theme;
        self.span_cache.borrow_mut().valid = false;
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

    /// [`PaneTerm::report_mouse`] must gate on the program's mouse mode: a fresh
    /// pane (`?1000` not requested) reports NOTHING (returns false), so the host
    /// keeps the gesture for local selection/scroll; after the program enables
    /// `?1000` a button press is encoded and written (returns true). This is the
    /// wiring that makes the mouse work in vim/tmux/htop — the canonical egui
    /// shell did not report mouse at all before it.
    #[test]
    fn report_mouse_gates_on_mouse_mode() {
        let mut pane = PaneTerm::spawn(void_theme(), 80, 24);
        let mods = MouseModifiers::default();
        // Off by default → no report (host keeps the gesture).
        assert!(
            !pane.report_mouse(MouseButton::Left, mods, 3, 5, MouseEventKind::Press),
            "a press while mouse mode is Off must not be reported"
        );
        // If the shell could not spawn there is no terminal to enable ?1000 on;
        // the Off assertion above is still the meaningful one.
        let Some(term) = pane.terminal_for_test() else {
            return;
        };
        term.lock().unwrap().advance(b"\x1b[?1000h");
        assert!(
            pane.report_mouse(MouseButton::Left, mods, 3, 5, MouseEventKind::Press),
            "after ?1000h a button press must be encoded and written to the PTY"
        );
        // ?1000 reports buttons only — a bare-motion event encodes to nothing, so
        // it must NOT consume the gesture.
        assert!(
            !pane.report_mouse(MouseButton::None, mods, 3, 5, MouseEventKind::Motion),
            "?1000 reports buttons only — bare motion must not be reported"
        );
    }

    /// [`PaneTerm::pump_host_effects`] must drain ALL of the terminal's per-frame
    /// queues in one call: write back the PTY query reply (so a program querying
    /// the cursor position gets an answer), surface the OSC 52 clipboard write,
    /// the OSC 4 color set, and the OSC 9 notification for the host, and leave
    /// every queue empty so none can grow unbounded. The canonical egui shell
    /// dropped (and leaked) every one of these before this wiring.
    #[test]
    fn pump_host_effects_drains_every_queue() {
        let pane = PaneTerm::spawn(void_theme(), 80, 24);
        let Some(term) = pane.terminal_for_test() else {
            return;
        };
        {
            let mut t = term.lock().unwrap();
            t.advance(b"\x1b[6n"); // cursor-position report → pty_response
            t.advance(b"\x1b]52;c;aGVsbG8=\x07"); // OSC 52 write "hello"
            t.advance(b"\x1b]4;1;rgb:ff/00/00\x07"); // OSC 4 set index 1 = red
            t.advance(b"\x1b]9;Build complete\x07"); // OSC 9 desktop notification
        }
        let mut pane = pane;
        let fx = pane.pump_host_effects();
        assert_eq!(
            fx.clipboard_writes,
            vec!["hello".to_string()],
            "OSC 52 write must surface as a clipboard write"
        );
        assert_eq!(
            fx.color_sets,
            vec![ColorSet::Indexed {
                index: 1,
                rgb: (255, 0, 0),
            }],
            "OSC 4 set must surface as an indexed color set"
        );
        assert!(fx.notified, "OSC 9 must mark a notification as received");
        // The PTY reply and every other queue must now be drained.
        let mut t = term.lock().unwrap();
        assert!(
            t.take_pty_response().is_empty(),
            "pump must have drained + written back the cursor-position reply"
        );
        assert!(
            t.take_clipboard_writes().is_empty(),
            "clipboard queue drained"
        );
        assert!(t.take_color_sets().is_empty(), "color-set queue drained");
        assert!(
            t.take_notifications().is_empty(),
            "notification queue drained"
        );
    }

    /// [`PaneTerm::scroll_view`] drives local scrollback: after enough output to
    /// fill the scrollback, scrolling back raises the view offset off the live
    /// bottom and scrolling forward past the bottom clamps to offset 0. This is
    /// the mouse-wheel scrollback the canonical egui shell lacked (the wheel did
    /// nothing — you could not scroll up to read history).
    #[test]
    fn scroll_view_moves_the_scrollback_offset() {
        let pane = PaneTerm::spawn(void_theme(), 80, 6);
        let Some(term) = pane.terminal_for_test() else {
            return;
        };
        // Produce many more lines than the 6-row screen so there is scrollback.
        {
            let mut t = term.lock().unwrap();
            for i in 0..40 {
                t.advance(format!("line {i}\r\n").as_bytes());
            }
            assert_eq!(
                t.view_offset(),
                0,
                "output pins the view to the live bottom"
            );
        }
        let mut pane = pane;
        pane.scroll_view(5); // back into history
        assert!(
            term.lock().unwrap().view_offset() > 0,
            "scrolling back must raise the view offset off the bottom"
        );
        pane.scroll_view(-1000); // forward, clamped to the bottom
        assert_eq!(
            term.lock().unwrap().view_offset(),
            0,
            "scrolling forward past the bottom clamps to the live view"
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

    /// `grid_rows` is damage-gated: an unchanged grid reuses the cached `Rc`
    /// (a real cache hit — zero rebuild), and the rebuilt rows carry the text
    /// that was written. The cache-hit identity check is guarded on "still
    /// clean" so a racing async shell-prompt write can never flake it.
    #[test]
    fn grid_rows_is_damage_gated_and_content_correct() {
        let pane = PaneTerm::spawn(void_theme(), 80, 24);
        // If the shell could not spawn on this box there is no terminal to read.
        let Some(term) = pane.terminal_for_test() else {
            return;
        };
        term.lock().unwrap().advance(b"hello world");
        let r1 = pane.grid_rows().expect("live session yields rows");
        // r1 cleared the grid's damage. If no async PTY output has arrived since,
        // a second call with NO new writes must reuse the cached Rc.
        if !term.lock().unwrap().grid().is_damaged() {
            let r2 = pane.grid_rows().expect("live session yields rows");
            assert!(
                Rc::ptr_eq(&r1, &r2),
                "an unchanged, undamaged grid reuses the cached rows (no rebuild)"
            );
        }
        let joined: String = r1
            .iter()
            .flat_map(|row| row.iter().map(|(t, _)| t.as_str()))
            .collect();
        assert!(
            joined.contains("hello world"),
            "rebuilt rows carry the written text; got {joined:?}"
        );
    }

    /// A theme change recolours cells WITHOUT a grid write (no damage bit), so
    /// `set_theme` must invalidate the row cache — otherwise stale-coloured rows
    /// would be served. This holds regardless of any async reader activity.
    #[test]
    fn grid_rows_cache_invalidated_by_set_theme() {
        let mut pane = PaneTerm::spawn(void_theme(), 80, 24);
        let Some(term) = pane.terminal_for_test() else {
            return;
        };
        term.lock().unwrap().advance(b"x");
        let r1 = pane.grid_rows().expect("live session yields rows");
        pane.set_theme(Theme::builtin_named("itasha-void").unwrap());
        let r2 = pane.grid_rows().expect("live session yields rows");
        assert!(
            !Rc::ptr_eq(&r1, &r2),
            "set_theme invalidates the damage-gated row cache"
        );
    }
}
