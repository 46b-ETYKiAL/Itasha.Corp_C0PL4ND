//! VT/ANSI interpreter.
//!
//! Drives a [`Grid`] from a raw PTY byte stream using the `vte` parser (the
//! same state machine alacritty is built on). Implements the common subset
//! needed by real shells and TUIs: printable text with line wrap, CR/LF/BS/
//! TAB, SGR colour + attributes, cursor positioning, and erase. OSC 7 (cwd)
//! and the window title are captured; OSC 133 semantic zones hook in here.

use crate::grid::{Cell, CellFlags, Color, Grid};
use std::collections::VecDeque;
use vte::{Params, Parser, Perform};

/// Default scrollback line cap when not configured.
pub const DEFAULT_SCROLLBACK: usize = 10_000;

/// A decoded inline image anchored to a grid position (absolute line + column).
#[derive(Debug, Clone)]
pub struct TerminalImage {
    pub image: crate::image::DecodedImage,
    pub line: usize,
    pub col: usize,
}

/// The current drawing pen (colours + attributes applied to printed cells).
#[derive(Debug, Clone)]
struct Pen {
    fg: Color,
    bg: Color,
    flags: CellFlags,
}

impl Default for Pen {
    fn default() -> Self {
        Pen {
            fg: Color::Default,
            bg: Color::Default,
            flags: CellFlags::empty(),
        }
    }
}

/// Mouse tracking mode requested by the application via DEC private modes
/// 1000/1002/1003. The host event loop consults this to decide whether — and
/// for which events — to forward mouse activity to the PTY.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MouseMode {
    /// No mouse reporting (the default).
    #[default]
    Off,
    /// `?1000` — report button press and release only.
    Normal,
    /// `?1002` — report press/release plus motion while a button is held.
    ButtonEvent,
    /// `?1003` — report press/release plus all motion (even with no button).
    AnyEvent,
}

/// Wire encoding for mouse reports. Selected by DEC private modes 1006/1015;
/// defaults to the legacy X10/normal byte encoding when neither is set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MouseEncoding {
    /// Legacy `CSI M Cb Cx Cy` with each value offset by 32 (X10/normal).
    #[default]
    X10,
    /// `?1006` — SGR extended: `CSI < b ; x ; y M` (press) / `m` (release).
    Sgr,
    /// `?1015` — urxvt: `CSI b ; x ; y M` (decimal, all values offset by 32).
    Urxvt,
}

/// Cursor glyph shape requested via DECSCUSR (`CSI Ps SP q`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CursorShape {
    /// Filled rectangle (the default).
    #[default]
    Block,
    /// Underline bar at the cell baseline.
    Underline,
    /// Thin vertical bar at the cell's left edge.
    Bar,
}

/// DEC private mode state (`CSI ? Pm h` set / `CSI ? Pm l` reset).
///
/// Each field tracks one terminal mode the application can toggle. The host
/// renderer and event loop read these via [`Terminal`]'s getters to drive
/// cursor visibility, mouse forwarding, paste bracketing, and so on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DecModes {
    /// `?25` DECTCEM — cursor visible. Default `true`.
    pub cursor_visible: bool,
    /// `?2004` — bracketed paste mode.
    pub bracketed_paste: bool,
    /// `?1` DECCKM — cursor-key application mode (affects arrow-key encoding).
    pub application_cursor_keys: bool,
    /// `?7` DECAWM — autowrap at the right margin. Default `true`.
    pub autowrap: bool,
    /// `?12` — cursor blink (att610). Independent of DECSCUSR's blink bit.
    pub cursor_blink: bool,
    /// `?1004` — focus in/out reporting (`CSI I` / `CSI O`).
    pub focus_reporting: bool,
    /// `?2026` — synchronized output (batch updates between begin/end).
    pub sync_output: bool,
    /// Active mouse tracking mode (`?1000` / `?1002` / `?1003`).
    pub mouse_mode: MouseMode,
    /// Active mouse report encoding (`?1006` / `?1015`).
    pub mouse_encoding: MouseEncoding,
}

impl Default for DecModes {
    fn default() -> Self {
        DecModes {
            cursor_visible: true,
            bracketed_paste: false,
            application_cursor_keys: false,
            autowrap: true,
            cursor_blink: false,
            focus_reporting: false,
            sync_output: false,
            mouse_mode: MouseMode::Off,
            mouse_encoding: MouseEncoding::X10,
        }
    }
}

/// Saved primary-screen state captured when switching to the alternate screen.
#[derive(Debug, Clone)]
struct SavedScreen {
    grid: Grid,
    row: usize,
    col: usize,
    pen: Pen,
}

/// A mouse button (or wheel direction) for [`Terminal::encode_mouse`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Middle,
    Right,
    /// Wheel scrolled up (button 4).
    WheelUp,
    /// Wheel scrolled down (button 5).
    WheelDown,
    /// No button — used for bare-motion reports under `?1003`.
    None,
}

/// The kind of mouse event being reported.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseEventKind {
    /// Button pressed down.
    Press,
    /// Button released.
    Release,
    /// Pointer moved (drag with `?1002`, or any motion with `?1003`).
    Motion,
}

/// Keyboard modifiers held during a mouse event. These OR into the button byte
/// per the xterm protocol (shift=4, meta/alt=8, control=16).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MouseModifiers {
    pub shift: bool,
    pub alt: bool,
    pub control: bool,
}

/// Mutable terminal screen state (the `vte::Perform` implementor).
struct Screen {
    grid: Grid,
    row: usize,
    col: usize,
    pen: Pen,
    /// Lines scrolled off the top, newest at the back. Capped at `max_scrollback`.
    history: VecDeque<Vec<Cell>>,
    max_scrollback: usize,
    /// Lines scrolled up from the live bottom (0 = following live output).
    view_offset: usize,
    /// Captured window title (OSC 0/2).
    title: String,
    /// Captured working directory (OSC 7).
    cwd: Option<String>,
    /// Absolute line indices where a shell prompt began (OSC 133 ; A),
    /// for jump-to-prompt. Absolute = history length + grid row at mark time.
    prompt_marks: Vec<usize>,
    /// Hyperlink URIs seen via OSC 8, in arrival order.
    hyperlinks: Vec<String>,
    /// Decoded inline images (Sixel via DCS), anchored to grid positions.
    images: Vec<TerminalImage>,
    /// In-progress Sixel DCS payload accumulator (Some between hook and unhook).
    sixel_accum: Option<Vec<u8>>,
    /// DEC private mode flags toggled via `CSI ? Pm h` / `CSI ? Pm l`.
    dec_modes: DecModes,
    /// Requested cursor shape (DECSCUSR `CSI Ps SP q`).
    cursor_shape: CursorShape,
    /// Requested cursor blink from DECSCUSR (separate from `?12`).
    cursor_shape_blink: bool,
    /// Saved primary screen, present iff the alternate screen is active.
    /// Switching to the alt screen stashes the primary grid + cursor + pen
    /// here; switching back restores them. Scrollback (`history`) is NOT
    /// touched while the alt screen is active, so full-screen TUIs never
    /// pollute the user's scrollback.
    saved_primary: Option<SavedScreen>,
}

impl Screen {
    fn new(rows: usize, cols: usize, max_scrollback: usize) -> Self {
        Screen {
            grid: Grid::new(rows, cols),
            row: 0,
            col: 0,
            pen: Pen::default(),
            history: VecDeque::new(),
            max_scrollback,
            view_offset: 0,
            title: String::new(),
            cwd: None,
            prompt_marks: Vec::new(),
            hyperlinks: Vec::new(),
            images: Vec::new(),
            sixel_accum: None,
            dec_modes: DecModes::default(),
            cursor_shape: CursorShape::default(),
            cursor_shape_blink: false,
            saved_primary: None,
        }
    }

    /// Apply a DEC private mode set/reset to one mode number.
    ///
    /// `set` is `true` for `CSI ? Pm h`, `false` for `CSI ? Pm l`. Unknown mode
    /// numbers are ignored (the same forgiving posture real terminals take).
    fn set_dec_mode(&mut self, mode: u16, set: bool) {
        match mode {
            1 => self.dec_modes.application_cursor_keys = set,
            7 => self.dec_modes.autowrap = set,
            12 => self.dec_modes.cursor_blink = set,
            25 => self.dec_modes.cursor_visible = set,
            // Alternate screen — 1049 saves/clears, 47/1047 are the older forms.
            47 | 1047 | 1049 => self.set_alt_screen(mode, set),
            1000 => {
                self.dec_modes.mouse_mode = if set { MouseMode::Normal } else { MouseMode::Off };
            }
            1002 => {
                self.dec_modes.mouse_mode =
                    if set { MouseMode::ButtonEvent } else { MouseMode::Off };
            }
            1003 => {
                self.dec_modes.mouse_mode = if set { MouseMode::AnyEvent } else { MouseMode::Off };
            }
            1006 => {
                self.dec_modes.mouse_encoding = if set {
                    MouseEncoding::Sgr
                } else {
                    MouseEncoding::X10
                };
            }
            1015 => {
                self.dec_modes.mouse_encoding = if set {
                    MouseEncoding::Urxvt
                } else {
                    MouseEncoding::X10
                };
            }
            1004 => self.dec_modes.focus_reporting = set,
            2004 => self.dec_modes.bracketed_paste = set,
            2026 => self.dec_modes.sync_output = set,
            _ => {}
        }
    }

    /// Enter or leave the alternate screen.
    ///
    /// `1049` saves the cursor + clears + switches to a blank alt grid on set,
    /// and restores the cursor on reset. `1047` clears the alt screen when
    /// switching away. `47` switches without saving/restoring the cursor.
    /// Re-applying a set while already on the alt screen (or a reset while
    /// already on the primary) is a no-op so duplicate sequences are safe.
    fn set_alt_screen(&mut self, mode: u16, set: bool) {
        let saves_cursor = mode == 1049;
        if set {
            if self.saved_primary.is_some() {
                return; // Already on the alt screen.
            }
            let rows = self.grid.rows();
            let cols = self.grid.cols();
            let saved = SavedScreen {
                grid: std::mem::replace(&mut self.grid, Grid::new(rows, cols)),
                row: self.row,
                col: self.col,
                pen: self.pen.clone(),
            };
            self.saved_primary = Some(saved);
            // A fresh alt grid starts blank; home the cursor for 1049/1047.
            if saves_cursor || mode == 1047 {
                self.row = 0;
                self.col = 0;
            }
        } else {
            let Some(saved) = self.saved_primary.take() else {
                return; // Already on the primary screen.
            };
            if mode == 1047 {
                // 1047 clears the alt screen on the way out.
                self.grid.clear();
            }
            self.grid = saved.grid;
            self.grid.touch();
            if saves_cursor {
                self.row = saved.row;
                self.col = saved.col;
                self.pen = saved.pen;
            }
            // For 47/1047 the cursor is left where it is (xterm behaviour), but
            // clamp it so it can never index outside the restored grid.
            self.row = self.row.min(self.grid.rows() - 1);
            self.col = self.col.min(self.grid.cols() - 1);
        }
    }

    /// Apply DECSCUSR (`CSI Ps SP q`) cursor-shape selection.
    fn set_cursor_shape(&mut self, ps: u16) {
        let (shape, blink) = match ps {
            0 | 1 => (CursorShape::Block, true),
            2 => (CursorShape::Block, false),
            3 => (CursorShape::Underline, true),
            4 => (CursorShape::Underline, false),
            5 => (CursorShape::Bar, true),
            6 => (CursorShape::Bar, false),
            _ => return,
        };
        self.cursor_shape = shape;
        self.cursor_shape_blink = blink;
    }

    fn newline(&mut self) {
        if self.row + 1 >= self.grid.rows() {
            let dropped = self.grid.scroll_up_returning();
            // The alternate screen must never feed the user's scrollback — a
            // full-screen TUI (vim, less, htop) scrolling its own buffer would
            // otherwise flood history with transient content.
            if self.max_scrollback > 0 && self.saved_primary.is_none() {
                self.history.push_back(dropped);
                while self.history.len() > self.max_scrollback {
                    self.history.pop_front();
                }
            }
        } else {
            self.row += 1;
        }
    }

    fn sgr(&mut self, params: &Params) {
        // Flatten top-level params to their first subparameter.
        let codes: Vec<u16> = params
            .iter()
            .map(|p| p.first().copied().unwrap_or(0))
            .collect();
        let mut i = 0;
        if codes.is_empty() {
            self.pen = Pen::default();
            return;
        }
        while i < codes.len() {
            match codes[i] {
                0 => self.pen = Pen::default(),
                1 => self.pen.flags.bold = true,
                3 => self.pen.flags.italic = true,
                4 => self.pen.flags.underline = true,
                7 => self.pen.flags.inverse = true,
                9 => self.pen.flags.strikeout = true,
                22 => self.pen.flags.bold = false,
                23 => self.pen.flags.italic = false,
                24 => self.pen.flags.underline = false,
                27 => self.pen.flags.inverse = false,
                29 => self.pen.flags.strikeout = false,
                30..=37 => self.pen.fg = Color::Indexed((codes[i] - 30) as u8),
                40..=47 => self.pen.bg = Color::Indexed((codes[i] - 40) as u8),
                90..=97 => self.pen.fg = Color::Indexed((codes[i] - 90 + 8) as u8),
                100..=107 => self.pen.bg = Color::Indexed((codes[i] - 100 + 8) as u8),
                39 => self.pen.fg = Color::Default,
                49 => self.pen.bg = Color::Default,
                38 | 48 => {
                    // Extended colour: 38;5;n (indexed) or 38;2;r;g;b (rgb).
                    let target_is_fg = codes[i] == 38;
                    if let Some(&kind) = codes.get(i + 1) {
                        if kind == 5 {
                            if let Some(&n) = codes.get(i + 2) {
                                let c = Color::Indexed(n as u8);
                                if target_is_fg {
                                    self.pen.fg = c;
                                } else {
                                    self.pen.bg = c;
                                }
                                i += 2;
                            }
                        } else if kind == 2 {
                            if let (Some(&r), Some(&g), Some(&b)) =
                                (codes.get(i + 2), codes.get(i + 3), codes.get(i + 4))
                            {
                                let c = Color::Rgb(r as u8, g as u8, b as u8);
                                if target_is_fg {
                                    self.pen.fg = c;
                                } else {
                                    self.pen.bg = c;
                                }
                                i += 4;
                            }
                        }
                    }
                }
                _ => {}
            }
            i += 1;
        }
    }

    fn first_param(params: &Params, default: u16) -> usize {
        let v = params
            .iter()
            .next()
            .and_then(|p| p.first().copied())
            .unwrap_or(0);
        if v == 0 {
            default as usize
        } else {
            v as usize
        }
    }
}

impl Perform for Screen {
    fn print(&mut self, c: char) {
        if self.col >= self.grid.cols() {
            if self.dec_modes.autowrap {
                self.col = 0;
                self.newline();
            } else {
                // DECAWM off (`?7l`): clamp to the last column and overwrite it
                // in place rather than wrapping to the next line.
                self.col = self.grid.cols() - 1;
            }
        }
        self.grid.set(
            self.row,
            self.col,
            Cell {
                c,
                fg: self.pen.fg,
                bg: self.pen.bg,
                flags: self.pen.flags,
            },
        );
        self.col += 1;
    }

    fn execute(&mut self, byte: u8) {
        match byte {
            b'\n' => self.newline(),
            b'\r' => self.col = 0,
            b'\t' => {
                self.col = ((self.col / 8) + 1) * 8;
                if self.col >= self.grid.cols() {
                    self.col = self.grid.cols() - 1;
                }
            }
            0x08 => {
                self.col = self.col.saturating_sub(1);
            }
            _ => {}
        }
    }

    fn csi_dispatch(
        &mut self,
        params: &Params,
        intermediates: &[u8],
        _ignore: bool,
        action: char,
    ) {
        // DEC private mode set/reset: `CSI ? Pm h` / `CSI ? Pm l`. The `?`
        // arrives in the intermediates. Every param in the sequence is applied
        // (e.g. `CSI ? 1049 ; 1006 h` toggles both modes).
        if intermediates.contains(&b'?') && (action == 'h' || action == 'l') {
            let set = action == 'h';
            for p in params.iter() {
                if let Some(&m) = p.first() {
                    self.set_dec_mode(m, set);
                }
            }
            return;
        }
        // DECSCUSR: `CSI Ps SP q` — the space (0x20) is the intermediate.
        if action == 'q' && intermediates.contains(&b' ') {
            let ps = params
                .iter()
                .next()
                .and_then(|p| p.first().copied())
                .unwrap_or(0);
            self.set_cursor_shape(ps);
            return;
        }
        match action {
            'm' => self.sgr(params),
            'H' | 'f' => {
                // Cursor position: row;col, 1-based.
                let mut it = params.iter();
                let row = it
                    .next()
                    .and_then(|p| p.first().copied())
                    .unwrap_or(1)
                    .max(1) as usize;
                let col = it
                    .next()
                    .and_then(|p| p.first().copied())
                    .unwrap_or(1)
                    .max(1) as usize;
                self.row = (row - 1).min(self.grid.rows() - 1);
                self.col = (col - 1).min(self.grid.cols() - 1);
            }
            'A' => self.row = self.row.saturating_sub(Self::first_param(params, 1)),
            'B' => self.row = (self.row + Self::first_param(params, 1)).min(self.grid.rows() - 1),
            'C' => self.col = (self.col + Self::first_param(params, 1)).min(self.grid.cols() - 1),
            'D' => self.col = self.col.saturating_sub(Self::first_param(params, 1)),
            'J' => {
                // Erase in display: 2 = whole screen.
                let mode = params
                    .iter()
                    .next()
                    .and_then(|p| p.first().copied())
                    .unwrap_or(0);
                if mode == 2 {
                    self.grid.clear();
                    self.row = 0;
                    self.col = 0;
                }
            }
            'K' => {
                // Erase in line from cursor to end.
                for c in self.col..self.grid.cols() {
                    self.grid.set(self.row, c, Cell::default());
                }
            }
            _ => {}
        }
    }

    fn osc_dispatch(&mut self, params: &[&[u8]], _bell_terminated: bool) {
        if params.is_empty() {
            return;
        }
        let code = std::str::from_utf8(params[0]).ok();
        match code {
            Some("0") | Some("2") => {
                if let Some(t) = params.get(1).and_then(|p| std::str::from_utf8(p).ok()) {
                    self.title = t.to_string();
                }
            }
            Some("7") => {
                if let Some(uri) = params.get(1).and_then(|p| std::str::from_utf8(p).ok()) {
                    self.cwd = Some(uri.to_string());
                }
            }
            Some("8") => {
                // OSC 8 ; params ; URI  — capture non-empty URIs for click/open.
                if let Some(uri) = params.get(2).and_then(|p| std::str::from_utf8(p).ok()) {
                    if !uri.is_empty() {
                        self.hyperlinks.push(uri.to_string());
                    }
                }
            }
            Some("133") => {
                // OSC 133 ; A  marks a shell prompt start (jump-to-prompt).
                // We never route any report-back into the PTY (security: this
                // is the iTerm2 CVE-2024-38395/38396 class — capture only).
                let kind = params.get(1).copied();
                if kind == Some(b"A".as_slice()) || kind == Some(b"B".as_slice()) {
                    let abs = self.history.len() + self.row;
                    if self.prompt_marks.last() != Some(&abs) {
                        self.prompt_marks.push(abs);
                    }
                }
            }
            _ => {}
        }
    }

    fn hook(&mut self, _params: &Params, _intermediates: &[u8], _ignore: bool, action: char) {
        // DCS with final byte 'q' is a Sixel image; start accumulating payload.
        if action == 'q' {
            self.sixel_accum = Some(Vec::new());
        }
    }

    fn put(&mut self, byte: u8) {
        if let Some(buf) = &mut self.sixel_accum {
            // Bound the payload so a hostile stream can't exhaust memory.
            if buf.len() < 8 * 1024 * 1024 {
                buf.push(byte);
            }
        }
    }

    fn unhook(&mut self) {
        if let Some(buf) = self.sixel_accum.take() {
            if let Some(img) = crate::image::decode_sixel(&buf) {
                self.images.push(TerminalImage {
                    image: img,
                    line: self.history.len() + self.row,
                    col: self.col,
                });
            }
        }
    }
}

/// A terminal: VT parser + screen state. Feed it PTY bytes; read its grid.
pub struct Terminal {
    parser: Parser,
    screen: Screen,
}

impl Terminal {
    pub fn new(rows: usize, cols: usize) -> Self {
        Self::with_scrollback(rows, cols, DEFAULT_SCROLLBACK)
    }

    /// Construct with an explicit scrollback line cap.
    pub fn with_scrollback(rows: usize, cols: usize, max_scrollback: usize) -> Self {
        Terminal {
            parser: Parser::new(),
            screen: Screen::new(rows, cols, max_scrollback),
        }
    }

    /// Feed raw PTY bytes through the VT state machine.
    pub fn advance(&mut self, bytes: &[u8]) {
        // Split borrow of two distinct fields is allowed.
        self.parser.advance(&mut self.screen, bytes);
    }

    pub fn grid(&self) -> &Grid {
        &self.screen.grid
    }

    pub fn grid_mut(&mut self) -> &mut Grid {
        &mut self.screen.grid
    }

    pub fn title(&self) -> &str {
        &self.screen.title
    }

    pub fn cwd(&self) -> Option<&str> {
        self.screen.cwd.as_deref()
    }

    /// Absolute line indices of captured shell-prompt marks (OSC 133).
    pub fn prompt_marks(&self) -> &[usize] {
        &self.screen.prompt_marks
    }

    /// Hyperlink URIs captured via OSC 8, in arrival order.
    pub fn hyperlinks(&self) -> &[String] {
        &self.screen.hyperlinks
    }

    /// Decoded inline images (Sixel) anchored to grid positions.
    pub fn images(&self) -> &[TerminalImage] {
        &self.screen.images
    }

    /// The current DEC private mode flags (cursor visibility, mouse, paste, …).
    pub fn dec_modes(&self) -> DecModes {
        self.screen.dec_modes
    }

    /// Whether the cursor should be drawn (DECTCEM `?25`). Default `true`.
    pub fn is_cursor_visible(&self) -> bool {
        self.screen.dec_modes.cursor_visible
    }

    /// Whether the alternate screen is currently active. While active, the
    /// visible-grid accessors ([`Terminal::grid`], [`Terminal::display_rows`])
    /// transparently return the alt grid and scrollback is not extended.
    pub fn alt_screen_active(&self) -> bool {
        self.screen.saved_primary.is_some()
    }

    /// Whether bracketed-paste mode (`?2004`) is enabled.
    ///
    /// Integration point: when this is `true`, the host's paste handler must
    /// wrap the pasted text in `ESC [ 200 ~` … `ESC [ 201 ~` before writing it
    /// to the PTY (and strip any embedded bracket sequences from the paste).
    /// That framing lives in the app's input layer, not in this core crate.
    pub fn bracketed_paste(&self) -> bool {
        self.screen.dec_modes.bracketed_paste
    }

    /// The active mouse tracking mode (`?1000` / `?1002` / `?1003`).
    pub fn mouse_mode(&self) -> MouseMode {
        self.screen.dec_modes.mouse_mode
    }

    /// The active mouse report encoding (`?1006` / `?1015`).
    pub fn mouse_encoding(&self) -> MouseEncoding {
        self.screen.dec_modes.mouse_encoding
    }

    /// Whether DECCKM cursor-key application mode (`?1`) is enabled. The host
    /// uses this to choose between `ESC O A` (application) and `ESC [ A`
    /// (normal) arrow-key encodings.
    pub fn application_cursor_keys(&self) -> bool {
        self.screen.dec_modes.application_cursor_keys
    }

    /// Whether autowrap (DECAWM `?7`) is enabled. Default `true`.
    pub fn autowrap(&self) -> bool {
        self.screen.dec_modes.autowrap
    }

    /// Whether focus in/out reporting (`?1004`) is enabled. When `true`, the
    /// host sends `ESC [ I` on focus-in and `ESC [ O` on focus-out.
    pub fn focus_reporting(&self) -> bool {
        self.screen.dec_modes.focus_reporting
    }

    /// Whether synchronized output (`?2026`) is currently requested.
    pub fn sync_output(&self) -> bool {
        self.screen.dec_modes.sync_output
    }

    /// The requested cursor shape (DECSCUSR `CSI Ps SP q`). Default block.
    pub fn cursor_shape(&self) -> CursorShape {
        self.screen.cursor_shape
    }

    /// Whether the cursor should blink. This is `true` if either DECSCUSR
    /// selected a blinking shape or `?12` (att610 blink) is set.
    pub fn cursor_blink(&self) -> bool {
        self.screen.cursor_shape_blink || self.screen.dec_modes.cursor_blink
    }

    /// Encode a mouse event into the byte sequence to write to the PTY, using
    /// the terminal's currently-active mouse mode and encoding.
    ///
    /// `col` and `row` are **1-based** cell coordinates. Returns `None` when no
    /// report should be sent — either mouse reporting is off, or the event kind
    /// is not enabled by the active mode (e.g. motion under `?1000`). This lets
    /// the host's event loop call it unconditionally and forward only when a
    /// sequence is produced.
    ///
    /// Encodings:
    /// - **SGR (`?1006`)**: `CSI < b ; x ; y M` for press/motion, `… m` for
    ///   release. No coordinate clamping is needed (decimal, unbounded).
    /// - **urxvt (`?1015`)**: `CSI ( b+32 ) ; x ; y M` (decimal, all offset 32).
    /// - **X10/normal**: `CSI M Cb Cx Cy` with each byte = value + 32, with
    ///   coordinates clamped to 223 (the max a single offset byte can carry).
    pub fn encode_mouse(
        &self,
        button: MouseButton,
        modifiers: MouseModifiers,
        col: usize,
        row: usize,
        kind: MouseEventKind,
    ) -> Option<Vec<u8>> {
        let mode = self.screen.dec_modes.mouse_mode;
        if mode == MouseMode::Off {
            return None;
        }
        // Gate the event by the active mode's interest set.
        match kind {
            MouseEventKind::Motion => match mode {
                MouseMode::Normal => return None, // ?1000 reports buttons only.
                MouseMode::ButtonEvent => {
                    // ?1002 reports motion only while a button is held.
                    if button == MouseButton::None {
                        return None;
                    }
                }
                MouseMode::AnyEvent => {} // ?1003 reports all motion.
                MouseMode::Off => return None,
            },
            MouseEventKind::Press | MouseEventKind::Release => {}
        }

        // Low button bits per xterm: left=0, middle=1, right=2, release=3
        // (legacy only). Wheel buttons set bit 6 (64).
        let (mut cb, is_wheel): (u32, bool) = match button {
            MouseButton::Left => (0, false),
            MouseButton::Middle => (1, false),
            MouseButton::Right => (2, false),
            MouseButton::WheelUp => (64, true),
            MouseButton::WheelDown => (65, true),
            MouseButton::None => (3, false), // "no button" base for motion.
        };
        // Motion adds 32 (the drag/motion bit).
        if kind == MouseEventKind::Motion {
            cb += 32;
        }
        // Modifier bits: shift=4, meta/alt=8, control=16.
        if modifiers.shift {
            cb += 4;
        }
        if modifiers.alt {
            cb += 8;
        }
        if modifiers.control {
            cb += 16;
        }

        match self.screen.dec_modes.mouse_encoding {
            MouseEncoding::Sgr => {
                // SGR uses the *unoffset* button value; release uses a final
                // `m`. Wheel events are always reported as a press (`M`).
                let final_byte = if kind == MouseEventKind::Release && !is_wheel {
                    'm'
                } else {
                    'M'
                };
                Some(format!("\x1b[<{cb};{col};{row}{final_byte}").into_bytes())
            }
            MouseEncoding::Urxvt => {
                // urxvt: decimal, button value offset by 32, always final `M`.
                let b = cb + 32;
                Some(format!("\x1b[{b};{col};{row}M").into_bytes())
            }
            MouseEncoding::X10 => {
                // Legacy: single bytes, each offset by 32. Release collapses to
                // button 3 (the protocol cannot distinguish which button was
                // released). Coordinates clamp to 223 (255 - 32).
                let cb_byte = if kind == MouseEventKind::Release && !is_wheel {
                    // Keep modifier/motion bits but force the low button to 3.
                    (cb & !0b11) | 0b11
                } else {
                    cb
                };
                let cx = (col.min(223) as u32) + 32;
                let cy = (row.min(223) as u32) + 32;
                let mut out = b"\x1b[M".to_vec();
                out.push((cb_byte + 32) as u8);
                out.push(cx as u8);
                out.push(cy as u8);
                Some(out)
            }
        }
    }

    /// Number of lines retained in scrollback history.
    pub fn scrollback_len(&self) -> usize {
        self.screen.history.len()
    }

    /// Current scroll-up offset (0 = following live output).
    pub fn view_offset(&self) -> usize {
        self.screen.view_offset
    }

    /// Scroll the view up by `n` lines (toward history), clamped.
    pub fn scroll_up_view(&mut self, n: usize) {
        let max = self.screen.history.len();
        self.screen.view_offset = (self.screen.view_offset + n).min(max);
        self.screen.grid.touch();
    }

    /// Scroll the view down by `n` lines (toward live), clamped.
    pub fn scroll_down_view(&mut self, n: usize) {
        self.screen.view_offset = self.screen.view_offset.saturating_sub(n);
        self.screen.grid.touch();
    }

    /// Set the absolute scroll-up offset (clamped to history length).
    pub fn set_view_offset(&mut self, offset: usize) {
        let max = self.screen.history.len();
        self.screen.view_offset = offset.min(max);
        self.screen.grid.touch();
    }

    /// Snap back to following live output.
    pub fn scroll_to_bottom(&mut self) {
        if self.screen.view_offset != 0 {
            self.screen.view_offset = 0;
            self.screen.grid.touch();
        }
    }

    /// The `rows` rows currently visible, accounting for scrollback offset.
    /// When `view_offset == 0` this is exactly the live grid.
    pub fn display_rows(&self) -> Vec<Vec<Cell>> {
        let rows = self.screen.grid.rows();
        let cols = self.screen.grid.cols();
        let hist = &self.screen.history;
        let total = hist.len() + rows;
        // Bottom-anchored window of `rows` lines, shifted up by view_offset.
        let end = total.saturating_sub(self.screen.view_offset);
        let start = end.saturating_sub(rows);
        let mut out = Vec::with_capacity(rows);
        for line in start..end {
            if line < hist.len() {
                let mut row = hist[line].clone();
                row.resize(cols, Cell::default());
                out.push(row);
            } else {
                out.push(self.screen.grid.row(line - hist.len()).to_vec());
            }
        }
        while out.len() < rows {
            out.push(vec![Cell::default(); cols]);
        }
        out
    }

    /// All buffered lines (history + live grid) as plain text — used by search.
    pub fn all_lines(&self) -> Vec<String> {
        let mut lines: Vec<String> = self
            .screen
            .history
            .iter()
            .map(|row| row.iter().map(|c| c.c).collect::<String>())
            .collect();
        lines.extend(self.screen.grid.to_text().lines().map(|s| s.to_string()));
        lines
    }

    pub fn resize(&mut self, rows: usize, cols: usize) {
        self.screen.grid.resize(rows, cols);
        self.screen.row = self.screen.row.min(rows - 1);
        self.screen.col = self.screen.col.min(cols - 1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prints_plain_text() {
        let mut t = Terminal::new(4, 20);
        t.advance(b"hello");
        assert!(t.grid().to_text().starts_with("hello"));
    }

    #[test]
    fn handles_crlf() {
        let mut t = Terminal::new(4, 20);
        t.advance(b"ab\r\ncd");
        let text = t.grid().to_text();
        let mut lines = text.lines();
        assert!(lines.next().unwrap().starts_with("ab"));
        assert!(lines.next().unwrap().starts_with("cd"));
    }

    #[test]
    fn sgr_sets_indexed_color() {
        let mut t = Terminal::new(2, 10);
        t.advance(b"\x1b[31mR"); // red foreground
        let cell = t.grid().cell(0, 0).unwrap();
        assert_eq!(cell.c, 'R');
        assert_eq!(cell.fg, Color::Indexed(1));
    }

    #[test]
    fn sgr_truecolor() {
        let mut t = Terminal::new(2, 10);
        t.advance(b"\x1b[38;2;0;229;255mX");
        assert_eq!(t.grid().cell(0, 0).unwrap().fg, Color::Rgb(0, 229, 255));
    }

    #[test]
    fn erase_display_clears() {
        let mut t = Terminal::new(2, 10);
        t.advance(b"junk\x1b[2J");
        assert_eq!(t.grid().cell(0, 0).unwrap().c, ' ');
    }

    #[test]
    fn osc_sets_title() {
        let mut t = Terminal::new(2, 10);
        t.advance(b"\x1b]0;C0PL4ND\x07");
        assert_eq!(t.title(), "C0PL4ND");
    }

    #[test]
    fn line_wrap_advances_row() {
        let mut t = Terminal::new(3, 3);
        t.advance(b"abcd"); // wraps after 3 cols
        assert_eq!(t.grid().cell(0, 0).unwrap().c, 'a');
        assert_eq!(t.grid().cell(1, 0).unwrap().c, 'd');
    }

    #[test]
    fn scrollback_retains_lines_pushed_off_top() {
        let mut t = Terminal::with_scrollback(2, 4, 100);
        // 5 lines into a 2-row grid: 3 lines scroll into history.
        t.advance(b"L0\r\nL1\r\nL2\r\nL3\r\nL4");
        assert!(
            t.scrollback_len() >= 3,
            "history should retain scrolled lines"
        );
        let all = t.all_lines();
        assert!(all.iter().any(|l| l.starts_with("L0")));
        assert!(all.iter().any(|l| l.starts_with("L4")));
    }

    #[test]
    fn scroll_view_offset_clamps_and_resets() {
        let mut t = Terminal::with_scrollback(2, 4, 100);
        t.advance(b"a\r\nb\r\nc\r\nd\r\ne");
        t.scroll_up_view(1000);
        assert_eq!(
            t.view_offset(),
            t.scrollback_len(),
            "offset clamps to history"
        );
        t.scroll_to_bottom();
        assert_eq!(t.view_offset(), 0);
    }

    #[test]
    fn display_rows_follows_live_at_bottom() {
        let mut t = Terminal::new(3, 5);
        t.advance(b"x");
        let rows = t.display_rows();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0][0].c, 'x');
    }

    #[test]
    fn osc8_captures_hyperlink() {
        let mut t = Terminal::new(2, 40);
        t.advance(b"\x1b]8;;https://itasha.corp\x07link\x1b]8;;\x07");
        assert_eq!(t.hyperlinks(), &["https://itasha.corp".to_string()]);
    }

    #[test]
    fn osc133_records_prompt_mark() {
        let mut t = Terminal::new(4, 20);
        t.advance(b"\x1b]133;A\x07$ ");
        assert_eq!(t.prompt_marks().len(), 1);
    }

    #[test]
    fn dcs_sixel_captures_image() {
        let mut t = Terminal::new(4, 20);
        // DCS q ... ST  with a red colour def + one full sixel column.
        t.advance(b"\x1bPq#0;2;100;0;0~\x1b\\");
        assert_eq!(t.images().len(), 1);
        assert_eq!(t.images()[0].image.height, 6);
    }

    #[test]
    fn scrollback_cap_is_enforced() {
        let mut t = Terminal::with_scrollback(1, 4, 2);
        for i in 0..10 {
            t.advance(format!("{i}\r\n").as_bytes());
        }
        assert!(t.scrollback_len() <= 2, "history must not exceed the cap");
    }

    /// Deterministic robustness regression mirroring the `vt_parser` fuzz
    /// target (see `fuzz/fuzz_targets/vt_parser.rs`). A terminal parser
    /// consumes fully untrusted bytes; hostile or malformed escape sequences
    /// must never panic, hang, or produce an inconsistent grid. These seeds
    /// double as the fuzzer's regression corpus and run in the normal stable
    /// test suite on every platform (the fuzz harness itself needs nightly).
    #[test]
    fn parser_survives_adversarial_escape_sequences() {
        let seeds: &[&[u8]] = &[
            b"\x1b[",                                  // bare CSI, no final byte
            b"\x1b[999999999999999999999999999m",      // CSI param overflow
            b"\x1b[;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;m", // many empty params
            b"\x1b[38;2;",                             // truncated truecolor SGR
            b"\x1b]0;",                                // OSC with no terminator
            b"\x1b]8;;",                               // OSC 8 hyperlink, truncated
            b"\x1b]52;c;",                             // OSC 52 clipboard, truncated
            b"\x1b]1337;File=",                        // iTerm2 image, truncated
            b"\x1bPq",                                 // DCS / Sixel introducer
            b"\x1b#8",                                 // DECALN screen-align
            b"\x08\x08\x08\x08",                       // backspaces past col 0
            b"\x1b[999999;999999H",                    // cursor move far OOB
            b"\x1b[2J\x1b[3J\x1b[1J\x1b[0J",           // erase-display variants
            b"\xff\xfe\xfd\xfc\x00\x01\x02",           // invalid UTF-8 / control bytes
            b"\xe2\x82",                               // truncated UTF-8 multibyte
            b"\x1b[6n\x1b[5n",                         // device status report queries
        ];

        for seed in seeds {
            let mut t = Terminal::with_scrollback(24, 80, 1000);
            // Feed in 1-byte chunks so sequences straddle advance() calls —
            // the realistic split-across-PTY-reads case.
            for b in seed.iter() {
                t.advance(&[*b]);
            }
            // Touch the derived read surface to catch read-side inconsistency.
            let _ = t.title();
            let _ = t.cwd();
            let _ = t.hyperlinks();
            let _ = t.images();
            let _ = t.display_rows();
            let _ = t.all_lines();
            let _ = t.scrollback_len();
            // The new mode-state read surface must also stay consistent.
            let _ = t.dec_modes();
            let _ = t.is_cursor_visible();
            let _ = t.alt_screen_active();
            let _ = t.mouse_mode();
            let _ = t.mouse_encoding();
            let _ = t.cursor_shape();
            let _ = t.cursor_blink();
        }
    }

    // ---- DEC private mode framework (item 1) ----

    #[test]
    fn dec_modes_default_state() {
        let t = Terminal::new(4, 20);
        let m = t.dec_modes();
        assert!(m.cursor_visible, "cursor visible by default");
        assert!(m.autowrap, "autowrap on by default");
        assert!(!m.bracketed_paste);
        assert!(!m.application_cursor_keys);
        assert!(!m.focus_reporting);
        assert!(!m.sync_output);
        assert_eq!(m.mouse_mode, MouseMode::Off);
        assert_eq!(m.mouse_encoding, MouseEncoding::X10);
    }

    #[test]
    fn dec_mode_set_and_reset_cursor_visibility() {
        let mut t = Terminal::new(4, 20);
        t.advance(b"\x1b[?25l");
        assert!(!t.is_cursor_visible(), "?25l hides the cursor");
        t.advance(b"\x1b[?25h");
        assert!(t.is_cursor_visible(), "?25h shows it again");
    }

    #[test]
    fn dec_mode_multiple_params_in_one_sequence() {
        let mut t = Terminal::new(4, 20);
        // Enter alt screen AND select SGR mouse encoding in one CSI.
        t.advance(b"\x1b[?1049;1006h");
        assert!(t.alt_screen_active(), "1049 applied");
        assert_eq!(
            t.mouse_encoding(),
            MouseEncoding::Sgr,
            "1006 applied from the same sequence"
        );
    }

    #[test]
    fn dec_mode_bracketed_paste_focus_sync() {
        let mut t = Terminal::new(4, 20);
        t.advance(b"\x1b[?2004h\x1b[?1004h\x1b[?2026h");
        assert!(t.bracketed_paste());
        assert!(t.focus_reporting());
        assert!(t.sync_output());
        t.advance(b"\x1b[?2004l\x1b[?1004l\x1b[?2026l");
        assert!(!t.bracketed_paste());
        assert!(!t.focus_reporting());
        assert!(!t.sync_output());
    }

    #[test]
    fn dec_mode_mouse_tracking_modes() {
        let mut t = Terminal::new(4, 20);
        t.advance(b"\x1b[?1000h");
        assert_eq!(t.mouse_mode(), MouseMode::Normal);
        t.advance(b"\x1b[?1002h");
        assert_eq!(t.mouse_mode(), MouseMode::ButtonEvent);
        t.advance(b"\x1b[?1003h");
        assert_eq!(t.mouse_mode(), MouseMode::AnyEvent);
        t.advance(b"\x1b[?1003l");
        assert_eq!(t.mouse_mode(), MouseMode::Off);
    }

    #[test]
    fn dec_mode_application_cursor_keys_and_autowrap() {
        let mut t = Terminal::new(4, 20);
        t.advance(b"\x1b[?1h");
        assert!(t.application_cursor_keys());
        t.advance(b"\x1b[?7l");
        assert!(!t.autowrap());
        t.advance(b"\x1b[?7h");
        assert!(t.autowrap());
    }

    #[test]
    fn dec_mode_unknown_number_is_ignored() {
        let mut t = Terminal::new(4, 20);
        // 9999 is not a mode we model; must not panic or disturb defaults.
        t.advance(b"\x1b[?9999h");
        assert!(t.is_cursor_visible());
        assert_eq!(t.mouse_mode(), MouseMode::Off);
    }

    // ---- Alternate screen (item 2) ----

    #[test]
    fn alt_screen_preserves_primary_content_and_cursor() {
        let mut t = Terminal::new(4, 20);
        t.advance(b"primary"); // cursor now at col 7
        t.advance(b"\x1b[?1049h"); // enter alt
        assert!(t.alt_screen_active());
        // Alt screen starts blank.
        assert_eq!(t.grid().cell(0, 0).unwrap().c, ' ');
        t.advance(b"ALTBUF");
        assert_eq!(t.grid().cell(0, 0).unwrap().c, 'A');
        t.advance(b"\x1b[?1049l"); // leave alt
        assert!(!t.alt_screen_active());
        // Primary content is intact and the cursor was restored.
        assert!(t.grid().to_text().starts_with("primary"));
    }

    #[test]
    fn alt_screen_does_not_pollute_scrollback() {
        let mut t = Terminal::with_scrollback(2, 4, 100);
        t.advance(b"\x1b[?1049h");
        // Scroll the alt screen well past its height.
        t.advance(b"a\r\nb\r\nc\r\nd\r\ne\r\nf");
        assert_eq!(
            t.scrollback_len(),
            0,
            "alt-screen scrolling must not feed scrollback"
        );
        t.advance(b"\x1b[?1049l");
        assert_eq!(t.scrollback_len(), 0);
    }

    #[test]
    fn alt_screen_47_variant_switches_without_cursor_save() {
        let mut t = Terminal::new(4, 20);
        t.advance(b"hi");
        t.advance(b"\x1b[?47h");
        assert!(t.alt_screen_active());
        t.advance(b"\x1b[?47l");
        assert!(!t.alt_screen_active());
        assert!(t.grid().to_text().starts_with("hi"));
    }

    #[test]
    fn alt_screen_duplicate_enter_is_noop() {
        let mut t = Terminal::new(4, 20);
        t.advance(b"base");
        t.advance(b"\x1b[?1049h");
        t.advance(b"XX");
        t.advance(b"\x1b[?1049h"); // second enter must not clobber the saved primary
        t.advance(b"\x1b[?1049l");
        assert!(t.grid().to_text().starts_with("base"));
    }

    // ---- Bracketed paste (item 3) ----

    #[test]
    fn bracketed_paste_flag_tracks_mode() {
        let mut t = Terminal::new(4, 20);
        assert!(!t.bracketed_paste());
        t.advance(b"\x1b[?2004h");
        assert!(t.bracketed_paste());
        t.advance(b"\x1b[?2004l");
        assert!(!t.bracketed_paste());
    }

    // ---- Cursor visibility (item 4) covered by dec_mode_set_and_reset_cursor_visibility ----

    // ---- DECSCUSR cursor shape (item 5) ----

    #[test]
    fn decscusr_sets_shapes() {
        let cases: &[(&[u8], CursorShape, bool)] = &[
            (b"\x1b[0 q", CursorShape::Block, true),
            (b"\x1b[1 q", CursorShape::Block, true),
            (b"\x1b[2 q", CursorShape::Block, false),
            (b"\x1b[3 q", CursorShape::Underline, true),
            (b"\x1b[4 q", CursorShape::Underline, false),
            (b"\x1b[5 q", CursorShape::Bar, true),
            (b"\x1b[6 q", CursorShape::Bar, false),
        ];
        for (seq, shape, blink) in cases {
            let mut t = Terminal::new(2, 10);
            t.advance(seq);
            assert_eq!(t.cursor_shape(), *shape, "shape for {seq:?}");
            assert_eq!(t.cursor_blink(), *blink, "blink for {seq:?}");
        }
    }

    #[test]
    fn decscusr_default_is_block() {
        let t = Terminal::new(2, 10);
        assert_eq!(t.cursor_shape(), CursorShape::Block);
    }

    #[test]
    fn dec_mode_12_drives_cursor_blink() {
        let mut t = Terminal::new(2, 10);
        // Steady block via DECSCUSR (blink=false), then ?12h enables blink.
        t.advance(b"\x1b[2 q");
        assert!(!t.cursor_blink());
        t.advance(b"\x1b[?12h");
        assert!(t.cursor_blink(), "?12h enables blink independently");
    }

    // ---- DECAWM autowrap behaviour ----

    #[test]
    fn autowrap_off_clamps_to_last_column() {
        let mut t = Terminal::new(3, 3);
        t.advance(b"\x1b[?7l"); // disable autowrap
        t.advance(b"abcd"); // 'd' overwrites the last cell instead of wrapping
        assert_eq!(t.grid().cell(0, 0).unwrap().c, 'a');
        assert_eq!(t.grid().cell(0, 2).unwrap().c, 'd', "last col overwritten");
        assert_eq!(t.grid().cell(1, 0).unwrap().c, ' ', "no wrap to next line");
    }

    // ---- Mouse encoding helper (item 6) ----

    #[test]
    fn encode_mouse_off_returns_none() {
        let t = Terminal::new(4, 20);
        let out = t.encode_mouse(
            MouseButton::Left,
            MouseModifiers::default(),
            5,
            7,
            MouseEventKind::Press,
        );
        assert!(out.is_none(), "no report when mouse mode is off");
    }

    #[test]
    fn encode_mouse_sgr_left_press_and_release() {
        let mut t = Terminal::new(4, 20);
        t.advance(b"\x1b[?1000h\x1b[?1006h");
        let press = t
            .encode_mouse(
                MouseButton::Left,
                MouseModifiers::default(),
                5,
                7,
                MouseEventKind::Press,
            )
            .unwrap();
        assert_eq!(press, b"\x1b[<0;5;7M");
        let release = t
            .encode_mouse(
                MouseButton::Left,
                MouseModifiers::default(),
                5,
                7,
                MouseEventKind::Release,
            )
            .unwrap();
        assert_eq!(release, b"\x1b[<0;5;7m", "release uses lowercase final m");
    }

    #[test]
    fn encode_mouse_sgr_modifiers_and_buttons() {
        let mut t = Terminal::new(4, 20);
        t.advance(b"\x1b[?1000h\x1b[?1006h");
        // Right button (2) + control (16) = 18.
        let out = t
            .encode_mouse(
                MouseButton::Right,
                MouseModifiers {
                    control: true,
                    ..Default::default()
                },
                1,
                1,
                MouseEventKind::Press,
            )
            .unwrap();
        assert_eq!(out, b"\x1b[<18;1;1M");
    }

    #[test]
    fn encode_mouse_x10_press_offsets_by_32() {
        let mut t = Terminal::new(4, 20);
        t.advance(b"\x1b[?1000h"); // X10 encoding by default
        let out = t
            .encode_mouse(
                MouseButton::Left,
                MouseModifiers::default(),
                1,
                1,
                MouseEventKind::Press,
            )
            .unwrap();
        // CSI M  Cb(0+32=32=' ')  Cx(1+32=33='!')  Cy(1+32=33='!')
        assert_eq!(out, b"\x1b[M !!");
    }

    #[test]
    fn encode_mouse_x10_clamps_large_coords() {
        let mut t = Terminal::new(4, 20);
        t.advance(b"\x1b[?1000h");
        let out = t
            .encode_mouse(
                MouseButton::Left,
                MouseModifiers::default(),
                1000,
                1000,
                MouseEventKind::Press,
            )
            .unwrap();
        // Coords clamp to 223; 223 + 32 = 255.
        assert_eq!(out[0], 0x1b);
        assert_eq!(&out[1..3], b"[M");
        assert_eq!(out[4], 255, "x clamps to 255");
        assert_eq!(out[5], 255, "y clamps to 255");
    }

    #[test]
    fn encode_mouse_normal_mode_drops_motion() {
        let mut t = Terminal::new(4, 20);
        t.advance(b"\x1b[?1000h\x1b[?1006h");
        let out = t.encode_mouse(
            MouseButton::Left,
            MouseModifiers::default(),
            5,
            5,
            MouseEventKind::Motion,
        );
        assert!(out.is_none(), "?1000 reports buttons only, not motion");
    }

    #[test]
    fn encode_mouse_button_event_motion_requires_button() {
        let mut t = Terminal::new(4, 20);
        t.advance(b"\x1b[?1002h\x1b[?1006h");
        // Motion with no button held → no report.
        assert!(t
            .encode_mouse(
                MouseButton::None,
                MouseModifiers::default(),
                3,
                3,
                MouseEventKind::Motion,
            )
            .is_none());
        // Motion while a button is held → reported (drag, +32 motion bit).
        let drag = t
            .encode_mouse(
                MouseButton::Left,
                MouseModifiers::default(),
                3,
                3,
                MouseEventKind::Motion,
            )
            .unwrap();
        assert_eq!(drag, b"\x1b[<32;3;3M", "drag sets the motion bit (0+32)");
    }

    #[test]
    fn encode_mouse_any_event_reports_bare_motion() {
        let mut t = Terminal::new(4, 20);
        t.advance(b"\x1b[?1003h\x1b[?1006h");
        let out = t
            .encode_mouse(
                MouseButton::None,
                MouseModifiers::default(),
                2,
                2,
                MouseEventKind::Motion,
            )
            .unwrap();
        // No button base = 3, + motion 32 = 35.
        assert_eq!(out, b"\x1b[<35;2;2M");
    }

    #[test]
    fn encode_mouse_urxvt_encoding() {
        let mut t = Terminal::new(4, 20);
        t.advance(b"\x1b[?1000h\x1b[?1015h");
        let out = t
            .encode_mouse(
                MouseButton::Left,
                MouseModifiers::default(),
                5,
                7,
                MouseEventKind::Press,
            )
            .unwrap();
        // urxvt: button offset by 32 → 32; decimal coords; final M.
        assert_eq!(out, b"\x1b[32;5;7M");
    }

    #[test]
    fn encode_mouse_wheel_up_is_button_64() {
        let mut t = Terminal::new(4, 20);
        t.advance(b"\x1b[?1000h\x1b[?1006h");
        let out = t
            .encode_mouse(
                MouseButton::WheelUp,
                MouseModifiers::default(),
                1,
                1,
                MouseEventKind::Press,
            )
            .unwrap();
        assert_eq!(out, b"\x1b[<64;1;1M");
    }
}
