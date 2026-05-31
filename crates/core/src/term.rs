//! VT/ANSI interpreter.
//!
//! Drives a [`Grid`] from a raw PTY byte stream using the `vte` parser (the
//! same state machine alacritty is built on). Implements the common subset
//! needed by real shells and TUIs: printable text with line wrap, CR/LF/BS/
//! TAB, SGR colour + attributes, cursor positioning, and erase. OSC 7 (cwd)
//! and the window title are captured; OSC 133 semantic zones hook in here.

use crate::grid::{Cell, CellFlags, Color, Grid, UnderlineStyle};
use std::collections::VecDeque;
use vte::{Params, Parser, Perform};

pub mod osc;

pub use osc::{
    ClipboardSelection, ClipboardWrite, ColorSet, CommandMark, CommandMarkKind, DynamicColor,
    Notification, Progress, ProgressState,
};
use osc::{base64_decode, base64_encode, format_color_reply, parse_color_spec, Rgb};

/// Default scrollback line cap when not configured.
pub const DEFAULT_SCROLLBACK: usize = 10_000;

/// A decoded inline image anchored to a grid position (absolute line + column).
#[derive(Debug, Clone)]
pub struct TerminalImage {
    pub image: crate::image::DecodedImage,
    pub line: usize,
    pub col: usize,
}

/// An in-progress Kitty graphics transmission, accumulated across chunks and
/// keyed by image id. Per the Kitty protocol the format and dimensions ride on
/// the FIRST chunk only — continuation chunks (`m=1`) resend just `m` + payload
/// — so `format`/`width`/`height` are captured at creation and reused when the
/// `m=0` boundary finalises the image.
#[derive(Debug, Clone)]
struct KittyChunk {
    /// Accumulated, still-base64-encoded payload across every chunk.
    payload: Vec<u8>,
    /// Pixel format declared on the first chunk (24 = RGB, 32 = RGBA, 100 = PNG).
    format: u16,
    width: usize,
    height: usize,
}

/// The current drawing pen (colours + attributes applied to printed cells).
#[derive(Debug, Clone)]
struct Pen {
    fg: Color,
    bg: Color,
    flags: CellFlags,
    /// Underline color (C20, SGR 58/59). `None` = inherit foreground.
    underline_color: Option<Color>,
}

impl Default for Pen {
    fn default() -> Self {
        Pen {
            fg: Color::Default,
            bg: Color::Default,
            flags: CellFlags::empty(),
            underline_color: None,
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

/// Builds the standard xterm 256-color palette as `(r, g, b)` triples.
///
/// 0-15 are the canonical xterm ANSI 16; 16-231 are the 6×6×6 color cube;
/// 232-255 are the 24-step grayscale ramp. The palette seeds OSC 4 query
/// replies so a program detecting colors gets sensible values before the host
/// applies its own theme via the [`ColorSet`] drain. The host's theme is the
/// source of truth for rendering — this palette is only the protocol-default
/// baseline for query/reset.
fn build_default_palette() -> [Rgb; 256] {
    // Canonical xterm ANSI 0-15.
    const ANSI16: [Rgb; 16] = [
        (0, 0, 0),       // 0 black
        (205, 0, 0),     // 1 red
        (0, 205, 0),     // 2 green
        (205, 205, 0),   // 3 yellow
        (0, 0, 238),     // 4 blue
        (205, 0, 205),   // 5 magenta
        (0, 205, 205),   // 6 cyan
        (229, 229, 229), // 7 white
        (127, 127, 127), // 8 bright black
        (255, 0, 0),     // 9 bright red
        (0, 255, 0),     // 10 bright green
        (255, 255, 0),   // 11 bright yellow
        (92, 92, 255),   // 12 bright blue
        (255, 0, 255),   // 13 bright magenta
        (0, 255, 255),   // 14 bright cyan
        (255, 255, 255), // 15 bright white
    ];
    let mut p: [Rgb; 256] = [(0, 0, 0); 256];
    p[..16].copy_from_slice(&ANSI16);
    // 6x6x6 cube: levels are 0, 95, 135, 175, 215, 255.
    const LEVELS: [u8; 6] = [0, 95, 135, 175, 215, 255];
    for i in 0..216usize {
        let r = LEVELS[(i / 36) % 6];
        let g = LEVELS[(i / 6) % 6];
        let b = LEVELS[i % 6];
        p[16 + i] = (r, g, b);
    }
    // Grayscale ramp 232-255: 8, 18, ..., 238 (step 10).
    for i in 0..24usize {
        let v = (8 + i * 10) as u8;
        p[232 + i] = (v, v, v);
    }
    p
}

/// Saved primary-screen state captured when switching to the alternate screen.
#[derive(Debug, Clone)]
struct SavedScreen {
    grid: Grid,
    row: usize,
    col: usize,
    pen: Pen,
}

/// Saved cursor state for DECSC (`ESC 7`) / DECRC (`ESC 8`) and the ANSI.SYS
/// `CSI s` / `CSI u` aliases. Captures position, pen, and the active charset
/// selection so a restore round-trips the full drawing context.
#[derive(Debug, Clone)]
struct SavedCursor {
    row: usize,
    col: usize,
    pen: Pen,
    charset_g0: Charset,
}

/// A G0/G1 charset designation. Only the two sets a real shell exercises are
/// modelled: plain ASCII and the DEC Special Graphics (line-drawing) set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Charset {
    /// US-ASCII (`ESC ( B`) — the default.
    #[default]
    Ascii,
    /// DEC Special Graphics / line drawing (`ESC ( 0`).
    DecLineDrawing,
}

/// Map a printable byte (`0x60..=0x7e`) from the DEC Special Graphics set to its
/// Unicode box-drawing equivalent. Characters outside that range pass through
/// unchanged. This is the canonical VT100 line-drawing table.
fn dec_line_draw(c: char) -> char {
    match c {
        '`' => '\u{25c6}', // ◆ diamond
        'a' => '\u{2592}', // ▒ checkerboard
        'b' => '\u{2409}', // ␉ HT
        'c' => '\u{240c}', // ␌ FF
        'd' => '\u{240d}', // ␍ CR
        'e' => '\u{240a}', // ␊ LF
        'f' => '\u{00b0}', // ° degree
        'g' => '\u{00b1}', // ± plus/minus
        'h' => '\u{2424}', // ␤ NL
        'i' => '\u{240b}', // ␋ VT
        'j' => '\u{2518}', // ┘ lower-right corner
        'k' => '\u{2510}', // ┐ upper-right corner
        'l' => '\u{250c}', // ┌ upper-left corner
        'm' => '\u{2514}', // └ lower-left corner
        'n' => '\u{253c}', // ┼ crossing
        'o' => '\u{23ba}', // ⎺ scan line 1
        'p' => '\u{23bb}', // ⎻ scan line 3
        'q' => '\u{2500}', // ─ horizontal line
        'r' => '\u{23bc}', // ⎼ scan line 7
        's' => '\u{23bd}', // ⎽ scan line 9
        't' => '\u{251c}', // ├ left tee
        'u' => '\u{2524}', // ┤ right tee
        'v' => '\u{2534}', // ┴ bottom tee
        'w' => '\u{252c}', // ┬ top tee
        'x' => '\u{2502}', // │ vertical line
        'y' => '\u{2264}', // ≤ less-than-or-equal
        'z' => '\u{2265}', // ≥ greater-than-or-equal
        '{' => '\u{03c0}', // π pi
        '|' => '\u{2260}', // ≠ not-equal
        '}' => '\u{00a3}', // £ pound
        '~' => '\u{00b7}', // · centre dot
        other => other,
    }
}

/// True for the Unicode variation selectors VS15 (U+FE0E, text presentation)
/// and VS16 (U+FE0F, emoji presentation). They are treated as zero-width
/// combining marks (C34) — they modify the previous grapheme's presentation
/// rather than occupying a cell.
fn is_variation_selector(c: char) -> bool {
    matches!(c, '\u{FE0E}' | '\u{FE0F}')
}

/// Map a mode-active boolean to the DECRQM `Pv` value: 1 = set, 2 = reset.
fn bool_mode(active: bool) -> u8 {
    if active {
        1
    } else {
        2
    }
}

/// Decode an ASCII-hex byte string (XTGETTCAP capability name) into a UTF-8
/// string. Returns `None` on odd length, non-hex bytes, or invalid UTF-8.
fn hex_decode(hex: &[u8]) -> Option<String> {
    if !hex.len().is_multiple_of(2) {
        return None;
    }
    let mut out = Vec::with_capacity(hex.len() / 2);
    for pair in hex.chunks_exact(2) {
        let hi = (pair[0] as char).to_digit(16)?;
        let lo = (pair[1] as char).to_digit(16)?;
        out.push((hi * 16 + lo) as u8);
    }
    String::from_utf8(out).ok()
}

/// Encode bytes as uppercase ASCII-hex (for XTGETTCAP replies).
fn hex_encode(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        s.push_str(&format!("{b:02X}"));
    }
    s
}

/// Default tab-stop interval (a stop every N columns), matching xterm/VT100.
const DEFAULT_TAB_INTERVAL: usize = 8;

/// Build the default per-column tab-stop bitset for a `cols`-wide screen: a stop
/// at every multiple of [`DEFAULT_TAB_INTERVAL`] (column 0, 8, 16, …).
fn default_tab_stops(cols: usize) -> Vec<bool> {
    (0..cols).map(|c| c % DEFAULT_TAB_INTERVAL == 0).collect()
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
    /// Parallel to `history`: `history_wrapped[i] == true` means history line `i`
    /// soft-wrapped into the next line (no hard newline). Kept the same length as
    /// `history` so reflow can reconstruct logical lines across the history/grid
    /// boundary.
    history_wrapped: VecDeque<bool>,
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
    /// Decoded inline images (Sixel via DCS, Kitty via APC), anchored to grid
    /// positions.
    images: Vec<TerminalImage>,
    /// In-progress Sixel DCS payload accumulator (Some between hook and unhook).
    sixel_accum: Option<Vec<u8>>,
    /// In-progress XTGETTCAP DCS payload accumulator (`DCS + q … ST`, C30). Some
    /// between hook and unhook when the DCS is an XTGETTCAP request, exclusive
    /// with `sixel_accum`.
    xtgettcap_accum: Option<Vec<u8>>,
    /// In-progress Kitty transmissions keyed by image id, accumulated across
    /// `m=1` … `m=0` chunk boundaries (decoded once at the `m=0` boundary).
    /// Format/width/height are captured from the FIRST chunk — the Kitty spec
    /// carries them only on the first chunk of a multi-chunk transmission.
    kitty_chunks: std::collections::HashMap<u32, KittyChunk>,
    /// Transmit-stored Kitty images (`a=t`) keyed by image id, available for a
    /// later `a=p` display. Bounded against hostile streams.
    kitty_store: std::collections::HashMap<u32, crate::image::DecodedImage>,
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
    /// Saved cursor for DECSC/DECRC (`ESC 7`/`ESC 8`) + ANSI.SYS `CSI s`/`CSI u`.
    saved_cursor: Option<SavedCursor>,
    /// Top margin of the scroll region (DECSTBM), 0-based inclusive. Defaults
    /// to row 0 (full screen).
    scroll_top: usize,
    /// Bottom margin of the scroll region (DECSTBM), 0-based inclusive. Defaults
    /// to the last row (full screen).
    scroll_bottom: usize,
    /// Active G0 charset designation (`ESC ( 0` / `ESC ( B`). Shift-In (SI) /
    /// Shift-Out (SO) toggle between this and `charset_g1`.
    charset_g0: Charset,
    /// Active G1 charset designation (`ESC ) 0` / `ESC ) B`).
    charset_g1: Charset,
    /// True when SO (0x0E) invoked G1 into GL; SI (0x0F) returns to G0.
    use_g1: bool,
    /// Per-column tab-stop flags (`tab_stops[c] == true` means a stop sits at
    /// column `c`). Length is always `grid.cols()`. Defaults to a stop every 8
    /// columns. Set by HTS (`ESC H`), cleared by TBC (`CSI g`/`CSI 3 g`).
    tab_stops: Vec<bool>,
    /// Last grapheme printed, for REP (`CSI b`).
    last_print: Option<char>,
    /// IRM insert mode (`CSI 4 h` / `l`, C25). When set, printing a glyph shifts
    /// the rest of the line right instead of overwriting.
    insert_mode: bool,
    /// DECSCNM reverse-video screen (`?5`, C25). When set, the renderer swaps
    /// default fg/bg for the whole screen. Stored here, read via a getter.
    reverse_screen: bool,
    /// DECOM origin mode (`?6`, C25). When set, cursor row addressing (CUP/VPA)
    /// is relative to the scroll region's top margin and clamped to it.
    origin_mode: bool,
    /// OSC 9 ; 4 progress reports (`ConEmu`/Windows-Terminal taskbar progress),
    /// drained by the app to drive a tab/taskbar indicator (C26).
    pending_progress: Vec<osc::Progress>,
    /// OSC 133 command zones (C28): each command's output-start and end (with
    /// optional exit code) marks, accumulated for prompt success/fail glyphs and
    /// command-duration display. Capture-only — never reported back to the PTY.
    command_marks: Vec<osc::CommandMark>,
    /// Pending bytes to write back to the PTY (OSC 4/10/11/12 query replies and
    /// OSC 52 read replies). Drained by [`Terminal::take_pty_response`]. The
    /// terminal NEVER queues a reply for OSC 133 marks (anti-CVE, capture-only).
    pty_response: Vec<u8>,
    /// OSC 52 clipboard WRITE requests, drained by the app. READs are
    /// DEFAULT-OFF (see `clipboard_read_enabled`) to avoid the canonical
    /// host-clipboard-exfiltration vulnerability.
    pending_clipboard_writes: Vec<ClipboardWrite>,
    clipboard_read_enabled: bool,
    /// OSC 4 / 10 / 11 / 12 / 104 / 11x color-set requests, drained by the app
    /// so it can apply them to its live theme.
    pending_color_sets: Vec<ColorSet>,
    /// OSC 9 / OSC 777 desktop notifications, drained by the app.
    pending_notifications: Vec<Notification>,
    /// XTWINOPS 22/23 t title stack.
    title_stack: Vec<String>,
    /// Live indexed palette (256 entries) used to answer OSC 4 queries and
    /// OSC 104 resets. Seeded from the xterm default; OSC 4 sets update it.
    palette: [Rgb; 256],
    /// Immutable default palette for OSC 104 reset.
    default_palette: [Rgb; 256],
    /// Live dynamic colors (default fg/bg/cursor) for OSC 10/11/12 queries.
    dynamic_fg: Rgb,
    dynamic_bg: Rgb,
    dynamic_cursor: Rgb,
    /// Defaults for OSC 110/111/112 reset.
    default_dynamic_fg: Rgb,
    default_dynamic_bg: Rgb,
    default_dynamic_cursor: Rgb,
}

impl Screen {
    fn new(rows: usize, cols: usize, max_scrollback: usize) -> Self {
        Screen {
            grid: Grid::new(rows, cols),
            row: 0,
            col: 0,
            pen: Pen::default(),
            history: VecDeque::new(),
            history_wrapped: VecDeque::new(),
            max_scrollback,
            view_offset: 0,
            title: String::new(),
            cwd: None,
            prompt_marks: Vec::new(),
            hyperlinks: Vec::new(),
            images: Vec::new(),
            sixel_accum: None,
            xtgettcap_accum: None,
            kitty_chunks: std::collections::HashMap::new(),
            kitty_store: std::collections::HashMap::new(),
            dec_modes: DecModes::default(),
            cursor_shape: CursorShape::default(),
            cursor_shape_blink: false,
            saved_primary: None,
            saved_cursor: None,
            scroll_top: 0,
            scroll_bottom: rows.saturating_sub(1),
            charset_g0: Charset::default(),
            charset_g1: Charset::default(),
            use_g1: false,
            tab_stops: default_tab_stops(cols),
            last_print: None,
            insert_mode: false,
            reverse_screen: false,
            origin_mode: false,
            pending_progress: Vec::new(),
            command_marks: Vec::new(),
            pty_response: Vec::new(),
            pending_clipboard_writes: Vec::new(),
            clipboard_read_enabled: false,
            pending_color_sets: Vec::new(),
            pending_notifications: Vec::new(),
            title_stack: Vec::new(),
            palette: build_default_palette(),
            default_palette: build_default_palette(),
            // xterm protocol defaults: white on black, white cursor.
            dynamic_fg: (229, 229, 229),
            dynamic_bg: (0, 0, 0),
            dynamic_cursor: (255, 255, 255),
            default_dynamic_fg: (229, 229, 229),
            default_dynamic_bg: (0, 0, 0),
            default_dynamic_cursor: (255, 255, 255),
        }
    }

    // ------------------------------------------------------------------
    // OSC color / clipboard / notification handlers (called by osc_dispatch).
    // ------------------------------------------------------------------

    /// Handles `OSC 4 ; i ; spec [; i ; spec ...]` (set/query indexed colors).
    ///
    /// Multiple `index;spec` pairs may appear. A `spec` of `?` queues a query
    /// reply reporting the current palette value; any other spec sets it (and
    /// queues a [`ColorSet`] for the app to apply to its theme).
    fn handle_osc_4(&mut self, params: &[&[u8]]) {
        let mut i = 1;
        while i + 1 < params.len() {
            let idx = std::str::from_utf8(params[i])
                .ok()
                .and_then(|s| s.trim().parse::<u16>().ok());
            let spec = String::from_utf8_lossy(params[i + 1]);
            if let Some(idx) = idx {
                if idx <= 255 {
                    let idx = idx as u8;
                    if spec.trim() == "?" {
                        let reply = format_color_reply(self.palette[idx as usize]);
                        let resp = format!("\x1b]4;{};{}\x07", idx, reply);
                        self.pty_response.extend_from_slice(resp.as_bytes());
                    } else if let Some(rgb) = parse_color_spec(&spec) {
                        self.palette[idx as usize] = rgb;
                        self.pending_color_sets
                            .push(ColorSet::Indexed { index: idx, rgb });
                    }
                }
            }
            i += 2;
        }
    }

    /// Handles `OSC 10/11/12 ; spec` (set/query default fg/bg/cursor).
    fn handle_dynamic_color(&mut self, params: &[&[u8]], which: DynamicColor) {
        let Some(spec) = params.get(1) else {
            return;
        };
        let spec = String::from_utf8_lossy(spec);
        if spec.trim() == "?" {
            let current = match which {
                DynamicColor::Foreground => self.dynamic_fg,
                DynamicColor::Background => self.dynamic_bg,
                DynamicColor::Cursor => self.dynamic_cursor,
            };
            let code = match which {
                DynamicColor::Foreground => 10,
                DynamicColor::Background => 11,
                DynamicColor::Cursor => 12,
            };
            let resp = format!("\x1b]{};{}\x07", code, format_color_reply(current));
            self.pty_response.extend_from_slice(resp.as_bytes());
        } else if let Some(rgb) = parse_color_spec(&spec) {
            match which {
                DynamicColor::Foreground => self.dynamic_fg = rgb,
                DynamicColor::Background => self.dynamic_bg = rgb,
                DynamicColor::Cursor => self.dynamic_cursor = rgb,
            }
            self.pending_color_sets
                .push(ColorSet::Dynamic { which, rgb });
        }
    }

    /// Handles `OSC 104 [; i ...]` (reset indexed palette entries to defaults).
    ///
    /// With no index arguments, resets the entire palette. Otherwise resets only
    /// the named indices. Each reset queues a [`ColorSet`] carrying the default
    /// color so the app can mirror the change.
    fn handle_osc_104(&mut self, params: &[&[u8]]) {
        if params.len() <= 1 {
            self.palette = self.default_palette;
            for (idx, &rgb) in self.default_palette.iter().enumerate() {
                self.pending_color_sets.push(ColorSet::Indexed {
                    index: idx as u8,
                    rgb,
                });
            }
            return;
        }
        for raw in &params[1..] {
            if let Some(idx) = std::str::from_utf8(raw)
                .ok()
                .and_then(|s| s.trim().parse::<u16>().ok())
            {
                if idx <= 255 {
                    let idx = idx as u8;
                    let rgb = self.default_palette[idx as usize];
                    self.palette[idx as usize] = rgb;
                    self.pending_color_sets
                        .push(ColorSet::Indexed { index: idx, rgb });
                }
            }
        }
    }

    /// Resets a dynamic color (OSC 110/111/112) to its default value.
    fn reset_dynamic_color(&mut self, which: DynamicColor) {
        let rgb = match which {
            DynamicColor::Foreground => self.default_dynamic_fg,
            DynamicColor::Background => self.default_dynamic_bg,
            DynamicColor::Cursor => self.default_dynamic_cursor,
        };
        match which {
            DynamicColor::Foreground => self.dynamic_fg = rgb,
            DynamicColor::Background => self.dynamic_bg = rgb,
            DynamicColor::Cursor => self.dynamic_cursor = rgb,
        }
        self.pending_color_sets
            .push(ColorSet::Dynamic { which, rgb });
    }

    /// Handles `OSC 52 ; <selection> ; <base64|?>` (clipboard set/query).
    ///
    /// WRITE-only by default. A `?` payload is a clipboard READ request and is
    /// ignored unless [`Terminal::set_clipboard_read_enabled`] is opted into —
    /// and even then the core does not read the host clipboard itself; the app
    /// must call [`Terminal::respond_clipboard_read`]. This avoids the canonical
    /// OSC 52 host-clipboard-exfiltration vulnerability.
    fn handle_osc_52(&mut self, params: &[&[u8]]) {
        let sel_bytes = params.get(1).copied().unwrap_or(b"c");
        let payload = params.get(2).copied().unwrap_or(b"");

        // Selection: first recognised char; empty -> clipboard.
        let selection = sel_bytes
            .iter()
            .find_map(|&b| match b {
                b'c' | b's' | b'0' => Some(ClipboardSelection::Clipboard),
                b'p' => Some(ClipboardSelection::Primary),
                _ => None,
            })
            .unwrap_or(ClipboardSelection::Clipboard);

        if payload == b"?" {
            // Clipboard READ request: DEFAULT-OFF; never auto-respond with host
            // clipboard contents. Dropped; the app may later call
            // respond_clipboard_read after opting in.
            return;
        }

        if let Some(decoded) = base64_decode(payload) {
            let text = String::from_utf8_lossy(&decoded).into_owned();
            self.pending_clipboard_writes
                .push(ClipboardWrite { selection, text });
        }
    }

    /// Handles `OSC 9 ; 4 ; <state> ; <percent>` (C26 — taskbar/tab progress).
    ///
    /// State 0 removes the indicator, 1 = normal, 2 = error, 3 = indeterminate,
    /// 4 = warning. `percent` is clamped to 0-100 and ignored for states 0/3.
    /// Pushes a [`osc::Progress`] into the drained queue; never replies to PTY.
    fn handle_osc_9_4(&mut self, params: &[&[u8]]) {
        let state_n = params
            .get(2)
            .and_then(|p| std::str::from_utf8(p).ok())
            .and_then(|s| s.trim().parse::<u8>().ok())
            .unwrap_or(0);
        let percent = params
            .get(3)
            .and_then(|p| std::str::from_utf8(p).ok())
            .and_then(|s| s.trim().parse::<u32>().ok())
            .unwrap_or(0)
            .min(100) as u8;
        let state = match state_n {
            1 => osc::ProgressState::Normal,
            2 => osc::ProgressState::Error,
            3 => osc::ProgressState::Indeterminate,
            4 => osc::ProgressState::Warning,
            _ => osc::ProgressState::Remove,
        };
        let percent = match state {
            osc::ProgressState::Remove | osc::ProgressState::Indeterminate => 0,
            _ => percent,
        };
        self.pending_progress.push(osc::Progress { state, percent });
    }

    /// Pushes the current title onto the title stack (XTWINOPS `CSI 22 t`).
    fn push_title(&mut self) {
        self.title_stack.push(self.title.clone());
    }

    /// Pops a title from the title stack into the current title
    /// (XTWINOPS `CSI 23 t`). No-op when the stack is empty.
    fn pop_title(&mut self) {
        if let Some(t) = self.title_stack.pop() {
            self.title = t;
        }
    }

    /// Apply a DEC private mode set/reset to one mode number.
    ///
    /// `set` is `true` for `CSI ? Pm h`, `false` for `CSI ? Pm l`. Unknown mode
    /// numbers are ignored (the same forgiving posture real terminals take).
    fn set_dec_mode(&mut self, mode: u16, set: bool) {
        match mode {
            1 => self.dec_modes.application_cursor_keys = set,
            5 => self.reverse_screen = set, // DECSCNM — reverse-video screen (C25).
            6 => {
                // DECOM origin mode (C25): toggling it homes the cursor to the
                // origin (top margin / top-left), per the VT spec.
                self.origin_mode = set;
                self.row = if set { self.scroll_top } else { 0 };
                self.col = 0;
            }
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

    /// True when a scroll region narrower than the full screen is active.
    fn region_is_full(&self) -> bool {
        self.scroll_top == 0 && self.scroll_bottom + 1 >= self.grid.rows()
    }

    /// Clamp the scroll region to the current grid bounds (called after resize
    /// and at DECSTBM-set time). A region that became invalid resets to full.
    fn clamp_scroll_region(&mut self) {
        let last = self.grid.rows().saturating_sub(1);
        if self.scroll_bottom > last {
            self.scroll_bottom = last;
        }
        if self.scroll_top > self.scroll_bottom {
            self.scroll_top = 0;
            self.scroll_bottom = last;
        }
    }

    // ------------------------------------------------------------------
    // Tab stops (C19): HTS (ESC H) / TBC (CSI g) / CHT (CSI I) / CBT (CSI Z)
    // and the `\t` / 0x09 advance.
    // ------------------------------------------------------------------

    /// Resize the tab-stop bitset to the current column count, seeding any
    /// newly-exposed columns with the default every-8 stops and preserving the
    /// stops that still fit. Called after a grid resize.
    fn resize_tab_stops(&mut self, cols: usize) {
        let old = self.tab_stops.len();
        if cols == old {
            return;
        }
        self.tab_stops.resize(cols, false);
        // Seed columns beyond the old width with the default stops so a widened
        // screen tabs sensibly into the new region.
        for c in old..cols {
            self.tab_stops[c] = c % DEFAULT_TAB_INTERVAL == 0;
        }
    }

    /// HTS (`ESC H`): set a tab stop at the current column.
    fn set_tab_stop(&mut self) {
        if let Some(slot) = self.tab_stops.get_mut(self.col) {
            *slot = true;
        }
    }

    /// TBC (`CSI g` / `CSI 0 g`): clear the stop at the cursor column.
    /// `CSI 3 g` clears every stop.
    fn clear_tab_stop(&mut self, mode: u16) {
        match mode {
            0 => {
                if let Some(slot) = self.tab_stops.get_mut(self.col) {
                    *slot = false;
                }
            }
            3 => {
                for slot in &mut self.tab_stops {
                    *slot = false;
                }
            }
            _ => {}
        }
    }

    /// CHT (`CSI I`) / `\t`: advance the cursor forward `n` tab stops, stopping
    /// at the last column. When no stop lies ahead the cursor lands on the last
    /// column (matching xterm).
    fn tab_forward(&mut self, n: usize) {
        let last = self.grid.cols().saturating_sub(1);
        for _ in 0..n.max(1) {
            if self.col >= last {
                self.col = last;
                break;
            }
            let mut next = self.col + 1;
            while next < last && !self.tab_stops.get(next).copied().unwrap_or(false) {
                next += 1;
            }
            self.col = next;
        }
    }

    /// CBT (`CSI Z`): move the cursor back `n` tab stops, stopping at column 0.
    fn tab_back(&mut self, n: usize) {
        for _ in 0..n.max(1) {
            if self.col == 0 {
                break;
            }
            let mut prev = self.col - 1;
            while prev > 0 && !self.tab_stops.get(prev).copied().unwrap_or(false) {
                prev -= 1;
            }
            self.col = prev;
        }
    }

    /// C14 (focus reporting, core half): when DEC mode `?1004` is enabled, queue
    /// the focus-event report — `ESC [ I` on focus-in, `ESC [ O` on focus-out —
    /// into `pty_response` for the app to write to the PTY. A no-op when `?1004`
    /// is off, so the host may call it unconditionally on every focus change.
    fn focus_report(&mut self, focused: bool) {
        if !self.dec_modes.focus_reporting {
            return;
        }
        let seq: &[u8] = if focused { b"\x1b[I" } else { b"\x1b[O" };
        self.pty_response.extend_from_slice(seq);
    }

    /// C16 — reflow the scrollback + visible grid to a new column width without
    /// losing characters.
    ///
    /// Logical lines (runs of physical rows joined across soft-wrap boundaries)
    /// are reconstructed from `history` + `history_wrapped` + the grid's
    /// per-row wrap flags, then re-laid-out at `new_cols`. A hard newline is
    /// never merged across the wrap boundary (only `wrapped == true` rows join).
    /// Scrollback is preserved: the re-laid-out rows fill the bottom `new_rows`
    /// into the grid and the remainder back into history. The cursor is mapped
    /// to its logical position on a best-effort basis (exact when the cursor's
    /// logical line is fully materialised in the visible grid).
    fn reflow_resize(&mut self, new_rows: usize, new_cols: usize) {
        let old_rows = self.grid.rows();
        let old_cols = self.grid.cols();

        // --- Step 1: gather every physical row (history first, then grid) with
        // its soft-wrap flag, and remember which physical row holds the cursor.
        struct PhysRow {
            cells: Vec<Cell>,
            wrapped: bool,
        }
        let mut phys: Vec<PhysRow> = Vec::with_capacity(self.history.len() + old_rows);
        for (i, row) in self.history.iter().enumerate() {
            let mut cells = row.clone();
            cells.resize(old_cols, Cell::default());
            phys.push(PhysRow {
                cells,
                wrapped: self.history_wrapped.get(i).copied().unwrap_or(false),
            });
        }
        let cursor_phys = self.history.len() + self.row.min(old_rows.saturating_sub(1));
        let cursor_col = self.col.min(old_cols);
        for r in 0..old_rows {
            phys.push(PhysRow {
                cells: self.grid.row(r).to_vec(),
                wrapped: self.grid.is_wrapped(r),
            });
        }

        // Drop trailing fully-blank rows that sit BELOW the cursor row: they are
        // unused screen space, not content, and would otherwise push real lines
        // into scrollback on reflow. The cursor row itself is always retained.
        let mut keep = phys.len();
        while keep > cursor_phys + 1 {
            let pr = &phys[keep - 1];
            let blank = pr.cells.iter().all(|c| *c == Cell::default());
            if blank && !pr.wrapped {
                keep -= 1;
            } else {
                break;
            }
        }
        phys.truncate(keep);

        // --- Step 2: join physical rows into logical lines. Track, for the
        // cursor, the absolute cell offset within its logical line.
        struct LogicalLine {
            cells: Vec<Cell>,
        }
        let mut logical: Vec<LogicalLine> = Vec::new();
        let mut cur: Vec<Cell> = Vec::new();
        let mut cursor_logical: Option<usize> = None;
        let mut cursor_offset: usize = 0;

        for (idx, pr) in phys.iter().enumerate() {
            // If the cursor lives on this physical row, its offset is the cells
            // accumulated so far in this logical line plus the cursor column.
            if idx == cursor_phys {
                cursor_logical = Some(logical.len());
                cursor_offset = cur.len() + cursor_col;
            }
            cur.extend_from_slice(&pr.cells);
            if !pr.wrapped {
                // Hard line ending — trim trailing blank cells so padding does
                // not get re-wrapped, but keep at least the cursor's reach.
                let mut end = cur.len();
                while end > 0 && cur[end - 1] == Cell::default() {
                    end -= 1;
                }
                if cursor_logical == Some(logical.len()) {
                    end = end.max(cursor_offset);
                }
                cur.truncate(end);
                logical.push(LogicalLine {
                    cells: std::mem::take(&mut cur),
                });
            }
        }
        // A trailing soft-wrapped run with no hard terminator (the live last
        // line) still forms a logical line.
        if !cur.is_empty() || cursor_logical == Some(logical.len()) {
            logical.push(LogicalLine {
                cells: std::mem::take(&mut cur),
            });
        }

        // --- Step 3: re-lay-out each logical line into new_cols-wide rows.
        let mut out_rows: Vec<Vec<Cell>> = Vec::new();
        let mut out_wrapped: Vec<bool> = Vec::new();
        // Maps (logical index, offset) -> (out_row, out_col) for the cursor.
        let mut cursor_out: Option<(usize, usize)> = None;

        for (li, line) in logical.iter().enumerate() {
            let cells = &line.cells;
            if cells.is_empty() {
                // Empty logical line -> one blank row, hard ending.
                if cursor_logical == Some(li) {
                    cursor_out = Some((out_rows.len(), 0));
                }
                out_rows.push(vec![Cell::default(); new_cols]);
                out_wrapped.push(false);
                continue;
            }
            let mut start = 0usize;
            while start < cells.len() {
                let end = (start + new_cols).min(cells.len());
                let mut row: Vec<Cell> = cells[start..end].to_vec();
                let continues = end < cells.len();
                // Cursor falls in this segment?
                if cursor_logical == Some(li)
                    && cursor_offset >= start
                    && (cursor_offset < end || (!continues && cursor_offset <= cells.len()))
                {
                    let col = (cursor_offset - start).min(new_cols.saturating_sub(1));
                    cursor_out = Some((out_rows.len(), col));
                }
                row.resize(new_cols, Cell::default());
                out_rows.push(row);
                out_wrapped.push(continues);
                start = end;
            }
        }

        if out_rows.is_empty() {
            out_rows.push(vec![Cell::default(); new_cols]);
            out_wrapped.push(false);
        }

        // --- Step 4: split into bottom `new_rows` (grid) + remainder (history).
        // Keep the cursor's row visible: if the cursor maps into history, clamp
        // the visible window down so it stays on screen.
        let total = out_rows.len();
        let mut grid_start = total.saturating_sub(new_rows);
        if let Some((cr, _)) = cursor_out {
            if cr < grid_start {
                grid_start = cr;
            }
        }
        // Rebuild history from the rows above the visible window.
        self.history.clear();
        self.history_wrapped.clear();
        for i in 0..grid_start {
            self.history.push_back(out_rows[i].clone());
            self.history_wrapped.push_back(out_wrapped[i]);
        }
        while self.history.len() > self.max_scrollback {
            self.history.pop_front();
            self.history_wrapped.pop_front();
        }

        // Rebuild the grid from the visible window, padding to new_rows.
        let mut new_grid = Grid::new(new_rows, new_cols);
        for vr in 0..new_rows {
            let src = grid_start + vr;
            if src < total {
                for (c, cell) in out_rows[src].iter().enumerate() {
                    new_grid.set(vr, c, cell.clone());
                }
                new_grid.set_wrapped(vr, out_wrapped[src]);
            }
        }
        self.grid = new_grid;

        // --- Step 5: place the cursor (best-effort).
        match cursor_out {
            Some((cr, cc)) if cr >= grid_start => {
                self.row = (cr - grid_start).min(new_rows - 1);
                self.col = cc.min(new_cols - 1);
            }
            _ => {
                // Cursor scrolled into history or could not be mapped: clamp to
                // the last visible row, preserving column where possible.
                self.row = new_rows - 1;
                self.col = self.col.min(new_cols - 1);
            }
        }
        self.view_offset = 0;
        self.resize_tab_stops(new_cols);
    }

    fn newline(&mut self) {
        // At the bottom margin: scroll the region up by one. Otherwise just
        // advance the row. When the region spans the whole screen, the dropped
        // top line feeds scrollback (unless on the alt screen).
        if self.row == self.scroll_bottom {
            if self.region_is_full() {
                // Capture the top row's soft-wrap flag BEFORE the scroll shifts
                // the flags up, so history records whether the dropped line
                // continued into the next.
                let dropped_wrapped = self.grid.is_wrapped(0);
                let dropped = self.grid.scroll_up_returning();
                // The alternate screen must never feed the user's scrollback — a
                // full-screen TUI (vim, less, htop) scrolling its own buffer
                // would otherwise flood history with transient content.
                if self.max_scrollback > 0 && self.saved_primary.is_none() {
                    self.history.push_back(dropped);
                    self.history_wrapped.push_back(dropped_wrapped);
                    while self.history.len() > self.max_scrollback {
                        self.history.pop_front();
                        self.history_wrapped.pop_front();
                    }
                }
            } else {
                // Region scroll: top line is discarded (not scrollback).
                self.grid
                    .scroll_region_up(self.scroll_top, self.scroll_bottom, 1);
            }
        } else if self.row + 1 < self.grid.rows() {
            self.row += 1;
        }
    }

    /// Reverse index (`ESC M`): move up one line within the scroll region,
    /// scrolling the region down when already at the top margin.
    fn reverse_index(&mut self) {
        if self.row == self.scroll_top {
            self.grid
                .scroll_region_down(self.scroll_top, self.scroll_bottom, 1);
        } else if self.row > 0 {
            self.row -= 1;
        }
    }

    /// DECSC (`ESC 7`) / `CSI s`: stash cursor position, pen, and charset.
    fn save_cursor(&mut self) {
        self.saved_cursor = Some(SavedCursor {
            row: self.row,
            col: self.col,
            pen: self.pen.clone(),
            charset_g0: self.charset_g0,
        });
    }

    /// DECRC (`ESC 8`) / `CSI u`: restore a previously-saved cursor. When none
    /// was saved, home the cursor (xterm behaviour for a bare restore).
    fn restore_cursor(&mut self) {
        if let Some(s) = self.saved_cursor.clone() {
            self.row = s.row.min(self.grid.rows().saturating_sub(1));
            self.col = s.col.min(self.grid.cols().saturating_sub(1));
            self.pen = s.pen;
            self.charset_g0 = s.charset_g0;
        } else {
            self.row = 0;
            self.col = 0;
        }
    }

    /// RIS (`ESC c`) hard reset and DECSTR (`CSI ! p`) soft reset share the
    /// recovery surface a garbled terminal needs: clear the screen, home the
    /// cursor, reset the pen, scroll region, charsets, and saved cursor. RIS
    /// additionally drops scrollback; the `hard` flag selects that.
    fn reset_state(&mut self, hard: bool) {
        self.grid.clear();
        self.row = 0;
        self.col = 0;
        self.pen = Pen::default();
        self.scroll_top = 0;
        self.scroll_bottom = self.grid.rows().saturating_sub(1);
        self.charset_g0 = Charset::Ascii;
        self.charset_g1 = Charset::Ascii;
        self.use_g1 = false;
        self.tab_stops = default_tab_stops(self.grid.cols());
        self.saved_cursor = None;
        self.dec_modes = DecModes::default();
        self.last_print = None;
        self.insert_mode = false;
        self.reverse_screen = false;
        self.origin_mode = false;
        if hard {
            self.history.clear();
            self.history_wrapped.clear();
            self.view_offset = 0;
            self.saved_primary = None;
        }
    }

    fn full_reset(&mut self) {
        self.reset_state(true);
    }

    fn sgr(&mut self, params: &Params) {
        // Each top-level param may carry colon-joined sub-parameters (e.g.
        // `4:3` curly underline, `58:2::r:g:b` underline color). Snapshot the
        // sub-parameter lists so colon-form attributes (C20) and the legacy
        // semicolon-form extended colors are both handled.
        let groups: Vec<Vec<u16>> = params.iter().map(|p| p.to_vec()).collect();
        // The flat first-subparameter view for the legacy `38;5;n` etc. walk.
        let codes: Vec<u16> = groups
            .iter()
            .map(|g| g.first().copied().unwrap_or(0))
            .collect();
        if codes.is_empty() {
            self.pen = Pen::default();
            return;
        }
        let mut i = 0;
        while i < codes.len() {
            match codes[i] {
                0 => self.pen = Pen::default(),
                1 => self.pen.flags.bold = true,
                3 => self.pen.flags.italic = true,
                4 => {
                    // C20 — styled underline. `4` alone = single. The colon
                    // sub-parameter form `4:n` selects the style (0=off, 1=single,
                    // 2=double, 3=curly, 4=dotted, 5=dashed).
                    let style = match groups[i].get(1).copied() {
                        None | Some(1) => UnderlineStyle::Single,
                        Some(0) => UnderlineStyle::None,
                        Some(2) => UnderlineStyle::Double,
                        Some(3) => UnderlineStyle::Curly,
                        Some(4) => UnderlineStyle::Dotted,
                        Some(5) => UnderlineStyle::Dashed,
                        Some(_) => UnderlineStyle::Single,
                    };
                    self.pen.flags.underline_style = style;
                }
                7 => self.pen.flags.inverse = true,
                9 => self.pen.flags.strikeout = true,
                21 => self.pen.flags.underline_style = UnderlineStyle::Double,
                22 => self.pen.flags.bold = false,
                23 => self.pen.flags.italic = false,
                24 => self.pen.flags.underline_style = UnderlineStyle::None,
                27 => self.pen.flags.inverse = false,
                29 => self.pen.flags.strikeout = false,
                30..=37 => self.pen.fg = Color::Indexed((codes[i] - 30) as u8),
                40..=47 => self.pen.bg = Color::Indexed((codes[i] - 40) as u8),
                90..=97 => self.pen.fg = Color::Indexed((codes[i] - 90 + 8) as u8),
                100..=107 => self.pen.bg = Color::Indexed((codes[i] - 100 + 8) as u8),
                39 => self.pen.fg = Color::Default,
                49 => self.pen.bg = Color::Default,
                58 => {
                    // C20 — underline color. Colon form `58:2::r:g:b` (RGB) or
                    // `58:5:n` (indexed) carries everything in one group;
                    // semicolon form `58;2;r;g;b` / `58;5;n` spreads across the
                    // following top-level params (consumed via `i` advance).
                    if let Some(color) = self.parse_extended_color(&groups[i], &codes, &mut i) {
                        self.pen.underline_color = Some(color);
                    }
                }
                59 => self.pen.underline_color = None,
                38 | 48 => {
                    let target_is_fg = codes[i] == 38;
                    if let Some(color) = self.parse_extended_color(&groups[i], &codes, &mut i) {
                        if target_is_fg {
                            self.pen.fg = color;
                        } else {
                            self.pen.bg = color;
                        }
                    }
                }
                _ => {}
            }
            i += 1;
        }
    }

    /// Parse an extended-color operand for SGR 38/48/58. Handles BOTH the colon
    /// sub-parameter form (everything packed into `group`, e.g. `38:2::r:g:b` or
    /// `38:5:n`) and the legacy semicolon form (spread across the following
    /// top-level `codes`, consumed by advancing `i`). Returns the parsed color,
    /// or `None` when the operand is malformed/truncated.
    fn parse_extended_color(
        &self,
        group: &[u16],
        codes: &[u16],
        i: &mut usize,
    ) -> Option<Color> {
        // Colon sub-parameter form: the kind + channels live inside `group`.
        if group.len() >= 2 {
            let kind = group[1];
            if kind == 5 {
                return group.get(2).map(|&n| Color::Indexed(n as u8));
            }
            if kind == 2 {
                // `58:2::r:g:b` carries an empty colorspace slot at index 2, so
                // the channels may start at index 2 OR 3. Take the LAST three
                // values present after the kind as r/g/b.
                let chans: Vec<u16> = group[2..].to_vec();
                if chans.len() >= 3 {
                    let r = chans[chans.len() - 3] as u8;
                    let g = chans[chans.len() - 2] as u8;
                    let b = chans[chans.len() - 1] as u8;
                    return Some(Color::Rgb(r, g, b));
                }
                return None;
            }
            return None;
        }
        // Legacy semicolon form: kind + channels are subsequent top-level codes.
        match codes.get(*i + 1).copied() {
            Some(5) => {
                let n = codes.get(*i + 2).copied()?;
                *i += 2;
                Some(Color::Indexed(n as u8))
            }
            Some(2) => {
                let r = codes.get(*i + 2).copied()?;
                let g = codes.get(*i + 3).copied()?;
                let b = codes.get(*i + 4).copied()?;
                *i += 4;
                Some(Color::Rgb(r as u8, g as u8, b as u8))
            }
            _ => None,
        }
    }

    /// Resolve a 0-based target row for an absolute cursor move (CUP/VPA),
    /// honouring DECOM origin mode (C25). When origin mode is set, the row is
    /// relative to `scroll_top` and clamped to `[scroll_top, scroll_bottom]`;
    /// otherwise it is an absolute screen row clamped to the grid.
    fn resolve_row(&self, target: usize) -> usize {
        if self.origin_mode {
            (self.scroll_top + target).min(self.scroll_bottom)
        } else {
            target.min(self.grid.rows().saturating_sub(1))
        }
    }

    /// DECRQM reply (C33): emit `CSI [?] Ps ; Pv $ y` reporting mode `ps`'s
    /// current value. `Pv` is 1 (set), 2 (reset), or 0 (not recognised).
    fn report_mode(&mut self, ps: u16, private: bool) {
        let value: u8 = if private {
            match ps {
                1 => bool_mode(self.dec_modes.application_cursor_keys),
                5 => bool_mode(self.reverse_screen),
                6 => bool_mode(self.origin_mode),
                7 => bool_mode(self.dec_modes.autowrap),
                12 => bool_mode(self.dec_modes.cursor_blink),
                25 => bool_mode(self.dec_modes.cursor_visible),
                1000 => bool_mode(self.dec_modes.mouse_mode == MouseMode::Normal),
                1002 => bool_mode(self.dec_modes.mouse_mode == MouseMode::ButtonEvent),
                1003 => bool_mode(self.dec_modes.mouse_mode == MouseMode::AnyEvent),
                1004 => bool_mode(self.dec_modes.focus_reporting),
                1006 => bool_mode(self.dec_modes.mouse_encoding == MouseEncoding::Sgr),
                2004 => bool_mode(self.dec_modes.bracketed_paste),
                2026 => bool_mode(self.dec_modes.sync_output),
                47 | 1047 | 1049 => bool_mode(self.saved_primary.is_some()),
                _ => 0,
            }
        } else {
            match ps {
                4 => bool_mode(self.insert_mode), // IRM
                _ => 0,
            }
        };
        let lead = if private { "?" } else { "" };
        let resp = format!("\x1b[{lead}{ps};{value}$y");
        self.pty_response.extend_from_slice(resp.as_bytes());
    }

    /// XTGETTCAP reply (C30): the DCS `q` payload is a space-separated list of
    /// hex-encoded terminfo capability names. For each recognised capability we
    /// reply `DCS 1 + r <hex-name> = <hex-value> ST`; for unrecognised ones we
    /// reply the invalid form `DCS 0 + r <hex-name> ST`. Apps that gate styled
    /// underline / truecolor on this must get a reply or they hang.
    fn report_xtgettcap(&mut self, payload: &[u8]) {
        for token in payload.split(|&b| b == b';') {
            if token.is_empty() {
                continue;
            }
            let Some(name) = hex_decode(token) else {
                continue;
            };
            // Recognised capabilities. `Co`/`colors` = 256, `TN` (terminal name)
            // = "xterm-256color", `RGB` = present (truecolor).
            let value: Option<&str> = match name.as_str() {
                "Co" | "colors" => Some("256"),
                "TN" | "name" => Some("xterm-256color"),
                "RGB" => Some(""), // boolean capability — present, empty value.
                _ => None,
            };
            let resp = match value {
                Some(v) => {
                    let name_hex = hex_encode(name.as_bytes());
                    if v.is_empty() {
                        format!("\x1bP1+r{name_hex}\x1b\\")
                    } else {
                        let val_hex = hex_encode(v.as_bytes());
                        format!("\x1bP1+r{name_hex}={val_hex}\x1b\\")
                    }
                }
                None => {
                    let name_hex = hex_encode(name.as_bytes());
                    format!("\x1bP0+r{name_hex}\x1b\\")
                }
            };
            self.pty_response.extend_from_slice(resp.as_bytes());
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
        use unicode_width::UnicodeWidthChar;

        // Apply the active charset (DEC line-drawing maps 0x60..0x7e).
        let active = if self.use_g1 {
            self.charset_g1
        } else {
            self.charset_g0
        };
        let c = if active == Charset::DecLineDrawing {
            dec_line_draw(c)
        } else {
            c
        };

        // C27 / C34 — a zero-width char (a combining mark, or a variation
        // selector VS15 U+FE0E / VS16 U+FE0F) does not occupy its own cell: it
        // attaches to the previous cell's base grapheme. This keeps `é` (e +
        // U+0301) and emoji presentation selectors in a single cell.
        let raw_width = UnicodeWidthChar::width(c).unwrap_or(0);
        if (raw_width == 0 || is_variation_selector(c)) && self.col > 0 {
            let prev = self.col - 1;
            // Append to the base cell, not the wide-glyph continuation spacer.
            let base = if prev > 0 && self.grid.is_continuation(self.row, prev) {
                prev - 1
            } else {
                prev
            };
            self.grid.push_combining_at(self.row, base, c);
            return;
        }

        self.last_print = Some(c);

        // East-Asian width: a wide glyph occupies two columns. A leading
        // zero-width char (no previous cell to attach to) still lands in a cell
        // so it is not lost.
        let width = raw_width.max(1);
        let cols = self.grid.cols();

        // IRM (insert mode, C25): when set, a printed glyph SHIFTS the rest of
        // the line right by its width rather than overwriting.
        if self.insert_mode && self.col < cols {
            self.grid.insert_blanks(self.row, self.col, width);
        }

        if self.col >= cols {
            if self.dec_modes.autowrap {
                // The current row filled to the width: mark it soft-wrapped so
                // reflow on resize can rejoin it with the continuation row.
                self.grid.set_wrapped(self.row, true);
                self.col = 0;
                self.newline();
            } else {
                // DECAWM off (`?7l`): clamp to the last column and overwrite it
                // in place rather than wrapping to the next line.
                self.col = cols - 1;
            }
        }

        // A width-2 glyph that would straddle the right edge wraps to the next
        // line first (the trailing cell is left blank), matching xterm.
        if width == 2 && self.col + 1 >= cols {
            if self.dec_modes.autowrap {
                self.grid.set_wrapped(self.row, true);
                self.col = 0;
                self.newline();
            } else {
                // No room and no wrap: drop the wide glyph rather than corrupt.
                return;
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
                underline_color: self.pen.underline_color,
                combining: None,
            },
        );
        self.col += 1;
        if width == 2 {
            // Continuation cell: a blank spacer that the renderer skips. Keeping
            // it as a space keeps grid->text extraction sane (one visible glyph).
            self.grid.set_continuation(
                self.row,
                self.col,
                Cell {
                    c: ' ',
                    fg: self.pen.fg,
                    bg: self.pen.bg,
                    flags: self.pen.flags,
                    underline_color: self.pen.underline_color,
                    combining: None,
                },
            );
            self.col += 1;
        }
    }

    fn execute(&mut self, byte: u8) {
        match byte {
            b'\n' => self.newline(),
            b'\r' => self.col = 0,
            b'\t' => self.tab_forward(1),
            0x08 => {
                self.col = self.col.saturating_sub(1);
            }
            0x0e => self.use_g1 = true,  // SO — invoke G1 into GL.
            0x0f => self.use_g1 = false, // SI — return to G0.
            _ => {}
        }
    }

    fn esc_dispatch(&mut self, intermediates: &[u8], _ignore: bool, byte: u8) {
        // Charset designation: `ESC ( <set>` for G0, `ESC ) <set>` for G1.
        if intermediates == [b'('] || intermediates == [b')'] {
            let cs = match byte {
                b'0' => Charset::DecLineDrawing,
                // B = US-ASCII; 'A'/'1'/'2' are other 8-bit-ish sets we treat as
                // ASCII (we don't draw their glyph variants).
                _ => Charset::Ascii,
            };
            if intermediates == [b'('] {
                self.charset_g0 = cs;
            } else {
                self.charset_g1 = cs;
            }
            return;
        }
        // Non-intermediate single-byte escapes.
        if intermediates.is_empty() {
            match byte {
                b'7' => self.save_cursor(),    // DECSC
                b'8' => self.restore_cursor(), // DECRC
                b'H' => self.set_tab_stop(),   // HTS — set tab stop at cursor.
                b'D' => self.newline(),        // IND — index (down, scroll region).
                b'M' => self.reverse_index(),  // RI — reverse index (up).
                b'E' => {
                    // NEL — next line: CR + LF.
                    self.col = 0;
                    self.newline();
                }
                b'c' => self.full_reset(), // RIS — reset to initial state.
                _ => {}
            }
        }
    }

    fn csi_dispatch(
        &mut self,
        params: &Params,
        intermediates: &[u8],
        _ignore: bool,
        action: char,
    ) {
        // DECRQM — request mode (C33): `CSI ? Ps $ p` (private) or
        // `CSI Ps $ p` (ANSI). Reply `CSI [?] Ps ; Pv $ y` where Pv is the
        // mode value (0 = unrecognised, 1 = set, 2 = reset). Apps that probe
        // modes hang without a reply.
        if action == 'p' && intermediates.contains(&b'$') {
            let ps = params
                .iter()
                .next()
                .and_then(|p| p.first().copied())
                .unwrap_or(0);
            let private = intermediates.contains(&b'?');
            self.report_mode(ps, private);
            return;
        }
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
        // ANSI (non-private) mode set/reset: `CSI Pm h` / `CSI Pm l`. The only
        // ANSI mode we model is IRM (insert/replace, mode 4 — C25).
        if intermediates.is_empty() && (action == 'h' || action == 'l') {
            let set = action == 'h';
            for p in params.iter() {
                if p.first().copied() == Some(4) {
                    self.insert_mode = set;
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
        // DECSTR soft reset: `CSI ! p` — the `!` is the intermediate.
        if action == 'p' && intermediates.contains(&b'!') {
            self.reset_state(false);
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
                self.row = self.resolve_row(row - 1);
                self.col = (col - 1).min(self.grid.cols() - 1);
            }
            'A' => self.row = self.row.saturating_sub(Self::first_param(params, 1)),
            'B' => self.row = (self.row + Self::first_param(params, 1)).min(self.grid.rows() - 1),
            'C' => self.col = (self.col + Self::first_param(params, 1)).min(self.grid.cols() - 1),
            'D' => self.col = self.col.saturating_sub(Self::first_param(params, 1)),
            'E' => {
                // CNL — cursor next line: down N, column 1.
                self.row = (self.row + Self::first_param(params, 1)).min(self.grid.rows() - 1);
                self.col = 0;
            }
            'F' => {
                // CPL — cursor previous line: up N, column 1.
                self.row = self.row.saturating_sub(Self::first_param(params, 1));
                self.col = 0;
            }
            'G' => {
                // CHA — cursor horizontal absolute (1-based column).
                self.col = (Self::first_param(params, 1) - 1).min(self.grid.cols() - 1);
            }
            'I' => {
                // CHT — cursor forward tabulation (advance N tab stops).
                self.tab_forward(Self::first_param(params, 1));
            }
            'Z' => {
                // CBT — cursor backward tabulation (back N tab stops).
                self.tab_back(Self::first_param(params, 1));
            }
            'g' => {
                // TBC — tab clear. 0 = clear stop at cursor, 3 = clear all.
                let mode = params
                    .iter()
                    .next()
                    .and_then(|p| p.first().copied())
                    .unwrap_or(0);
                self.clear_tab_stop(mode);
            }
            'd' => {
                // VPA — vertical position absolute (1-based row). Honours DECOM.
                self.row = self.resolve_row(Self::first_param(params, 1) - 1);
            }
            'r' => {
                // DECSTBM — set top/bottom scroll margins (1-based). No params
                // (or both default) resets to the full screen.
                let mut it = params.iter();
                let top = it.next().and_then(|p| p.first().copied()).unwrap_or(0) as usize;
                let bot = it.next().and_then(|p| p.first().copied()).unwrap_or(0) as usize;
                let last = self.grid.rows() - 1;
                let top = if top == 0 { 0 } else { top - 1 };
                let bottom = if bot == 0 { last } else { (bot - 1).min(last) };
                // A valid region needs top < bottom; otherwise reset to full.
                if top < bottom {
                    self.scroll_top = top;
                    self.scroll_bottom = bottom;
                } else {
                    self.scroll_top = 0;
                    self.scroll_bottom = last;
                }
                // DECSTBM homes the cursor (to origin / top-left).
                self.row = self.scroll_top;
                self.col = 0;
            }
            'L' => {
                // IL — insert N blank lines at the cursor row, within the scroll
                // region. Lines below shift down; lines past the bottom are lost.
                if self.row >= self.scroll_top && self.row <= self.scroll_bottom {
                    let n = Self::first_param(params, 1);
                    self.grid
                        .scroll_region_down(self.row, self.scroll_bottom, n);
                    self.col = 0;
                }
            }
            'M' => {
                // DL — delete N lines at the cursor row, within the scroll
                // region. Lines below shift up; blanks fill at the bottom.
                if self.row >= self.scroll_top && self.row <= self.scroll_bottom {
                    let n = Self::first_param(params, 1);
                    self.grid.scroll_region_up(self.row, self.scroll_bottom, n);
                    self.col = 0;
                }
            }
            'S' => {
                // SU — scroll the scroll region up N lines.
                let n = Self::first_param(params, 1);
                self.grid
                    .scroll_region_up(self.scroll_top, self.scroll_bottom, n);
            }
            'T' => {
                // SD — scroll the scroll region down N lines.
                let n = Self::first_param(params, 1);
                self.grid
                    .scroll_region_down(self.scroll_top, self.scroll_bottom, n);
            }
            '@' => {
                // ICH — insert N blank characters at the cursor (shift right).
                let n = Self::first_param(params, 1);
                self.grid.insert_blanks(self.row, self.col, n);
            }
            'P' => {
                // DCH — delete N characters at the cursor (shift left).
                let n = Self::first_param(params, 1);
                self.grid.delete_chars(self.row, self.col, n);
            }
            'X' => {
                // ECH — erase N characters at the cursor (blank, no shift).
                let n = Self::first_param(params, 1);
                self.grid.erase_chars(self.row, self.col, n);
            }
            'b' => {
                // REP — repeat the last printed grapheme N times.
                if let Some(c) = self.last_print {
                    let n = Self::first_param(params, 1);
                    for _ in 0..n {
                        self.print(c);
                    }
                }
            }
            's' => self.save_cursor(),    // SCOSC — ANSI.SYS save cursor.
            'u' => self.restore_cursor(), // SCORC — ANSI.SYS restore cursor.
            'c' => {
                // DA — device attributes. Primary (`CSI c` / `CSI 0 c`) reports
                // a VT220-class terminal with 132-column + selective-erase. The
                // `>` intermediate selects secondary DA (terminal id + version).
                if intermediates.contains(&b'>') {
                    // Secondary DA: CSI > 0 ; 0 ; 0 c  (VT100-family, no firmware).
                    self.pty_response.extend_from_slice(b"\x1b[>0;0;0c");
                } else {
                    // Primary DA: VT220 with 132-col (1), selective erase (6),
                    // ANSI color (22). Capability-probing apps need a reply or
                    // they hang.
                    self.pty_response.extend_from_slice(b"\x1b[?62;1;6;22c");
                }
            }
            'n' => {
                // DSR — device status report. `5n` → terminal OK; `6n` (and the
                // DECXCPR `?6n` form) → cursor position report.
                let ps = params
                    .iter()
                    .next()
                    .and_then(|p| p.first().copied())
                    .unwrap_or(0);
                match ps {
                    5 => self.pty_response.extend_from_slice(b"\x1b[0n"),
                    6 => {
                        // CPR: 1-based row;col. The DEC private form (`?6n`) uses
                        // the same body here.
                        let resp = format!("\x1b[{};{}R", self.row + 1, self.col + 1);
                        self.pty_response.extend_from_slice(resp.as_bytes());
                    }
                    _ => {}
                }
            }
            'J' => {
                // Erase in display. 0 = cursor→end, 1 = start→cursor, 2 = all,
                // 3 = all + scrollback.
                let mode = params
                    .iter()
                    .next()
                    .and_then(|p| p.first().copied())
                    .unwrap_or(0);
                let cols = self.grid.cols();
                let rows = self.grid.rows();
                match mode {
                    0 => {
                        // Cursor to end of screen.
                        for c in self.col..cols {
                            self.grid.set(self.row, c, Cell::default());
                        }
                        for r in (self.row + 1)..rows {
                            for c in 0..cols {
                                self.grid.set(r, c, Cell::default());
                            }
                        }
                    }
                    1 => {
                        // Start of screen to cursor (inclusive).
                        for r in 0..self.row {
                            for c in 0..cols {
                                self.grid.set(r, c, Cell::default());
                            }
                        }
                        for c in 0..=self.col.min(cols - 1) {
                            self.grid.set(self.row, c, Cell::default());
                        }
                    }
                    2 => {
                        self.grid.clear();
                        self.row = 0;
                        self.col = 0;
                    }
                    3 => {
                        // Erase scrollback (the `clear` command's second half).
                        self.history.clear();
                        self.history_wrapped.clear();
                        self.view_offset = 0;
                    }
                    _ => {}
                }
            }
            'K' => {
                // Erase in line. 0 = cursor→EOL, 1 = BOL→cursor, 2 = whole line.
                let mode = params
                    .iter()
                    .next()
                    .and_then(|p| p.first().copied())
                    .unwrap_or(0);
                let cols = self.grid.cols();
                match mode {
                    0 => {
                        for c in self.col..cols {
                            self.grid.set(self.row, c, Cell::default());
                        }
                    }
                    1 => {
                        for c in 0..=self.col.min(cols - 1) {
                            self.grid.set(self.row, c, Cell::default());
                        }
                    }
                    2 => {
                        for c in 0..cols {
                            self.grid.set(self.row, c, Cell::default());
                        }
                    }
                    _ => {}
                }
            }
            't' => {
                // XTWINOPS. We implement only the title-stack ops; geometry
                // window-manipulation ops are intentionally ignored (the app
                // owns the window). 22;n pushes the title, 23;n pops it; n
                // selects icon(1)/window(2)/both(0) title — we keep a single
                // title, so n is accepted but treated uniformly.
                let op = params
                    .iter()
                    .next()
                    .and_then(|p| p.first().copied())
                    .unwrap_or(0);
                match op {
                    22 => self.push_title(),
                    23 => self.pop_title(),
                    _ => {}
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
                // OSC 133 semantic-prompt zones. A/B mark prompt start/end
                // (jump-to-prompt); C/D mark command-output-start / command-end
                // with an optional exit code (C28). We never route any
                // report-back into the PTY (security: iTerm2 CVE-2024-38395/
                // 38396 class — capture only).
                let kind = params.get(1).copied();
                let abs = self.history.len() + self.row;
                match kind {
                    Some(b"A") | Some(b"B")
                        if self.prompt_marks.last() != Some(&abs) =>
                    {
                        self.prompt_marks.push(abs);
                    }
                    Some(b"A") | Some(b"B") => {}
                    Some(b"C") => {
                        self.command_marks.push(osc::CommandMark {
                            kind: osc::CommandMarkKind::OutputStart,
                            line: abs,
                        });
                    }
                    Some(b"D") => {
                        // `OSC 133 ; D [; exit_code]`. The exit code may be the
                        // 3rd field directly or embedded in a `D;aid=…` form;
                        // parse the first field that looks like an integer.
                        let exit_code = params.get(2).and_then(|p| {
                            std::str::from_utf8(p).ok().and_then(|s| {
                                s.split([';', '='])
                                    .find_map(|tok| tok.trim().parse::<i32>().ok())
                            })
                        });
                        self.command_marks.push(osc::CommandMark {
                            kind: osc::CommandMarkKind::CommandEnd { exit_code },
                            line: abs,
                        });
                    }
                    _ => {}
                }
            }
            Some("4") => self.handle_osc_4(params),
            Some("10") => self.handle_dynamic_color(params, DynamicColor::Foreground),
            Some("11") => self.handle_dynamic_color(params, DynamicColor::Background),
            Some("12") => self.handle_dynamic_color(params, DynamicColor::Cursor),
            Some("52") => self.handle_osc_52(params),
            Some("104") => self.handle_osc_104(params),
            Some("110") => self.reset_dynamic_color(DynamicColor::Foreground),
            Some("111") => self.reset_dynamic_color(DynamicColor::Background),
            Some("112") => self.reset_dynamic_color(DynamicColor::Cursor),
            Some("9") if params.get(1).copied() == Some(b"4".as_slice()) => {
                // OSC 9 ; 4 ; state ; percent — taskbar/tab progress (C26).
                self.handle_osc_9_4(params);
            }
            Some("9") => {
                // OSC 9: desktop notification (body only).
                if let Some(body) = params
                    .get(1)
                    .and_then(|p| std::str::from_utf8(p).ok())
                    .filter(|b| !b.is_empty())
                {
                    self.pending_notifications.push(Notification {
                        title: String::new(),
                        body: body.to_string(),
                    });
                }
            }
            // OSC 777: rxvt-unicode extension.
            // Format: OSC 777 ; notify ; <title> ; <body> ST
            Some("777") if params.get(1).copied() == Some(b"notify".as_slice()) => {
                let title = params
                    .get(2)
                    .and_then(|p| std::str::from_utf8(p).ok())
                    .unwrap_or("")
                    .to_string();
                let body = params
                    .get(3)
                    .and_then(|p| std::str::from_utf8(p).ok())
                    .unwrap_or("")
                    .to_string();
                if !title.is_empty() || !body.is_empty() {
                    self.pending_notifications.push(Notification { title, body });
                }
            }
            _ => {}
        }
    }

    fn hook(&mut self, _params: &Params, intermediates: &[u8], _ignore: bool, action: char) {
        // `DCS + q … ST` is an XTGETTCAP capability request (C30) — the `+`
        // arrives as an intermediate. `DCS q …` (no intermediate) is a Sixel
        // image. The two are disambiguated by the intermediate.
        if action == 'q' && intermediates.contains(&b'+') {
            self.xtgettcap_accum = Some(Vec::new());
        } else if action == 'q' {
            self.sixel_accum = Some(Vec::new());
        }
    }

    fn put(&mut self, byte: u8) {
        if let Some(buf) = &mut self.sixel_accum {
            // Bound the payload so a hostile stream can't exhaust memory.
            if buf.len() < 8 * 1024 * 1024 {
                buf.push(byte);
            }
        } else if let Some(buf) = &mut self.xtgettcap_accum {
            // XTGETTCAP names are tiny; bound generously against hostile input.
            if buf.len() < 4096 {
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
        } else if let Some(buf) = self.xtgettcap_accum.take() {
            self.report_xtgettcap(&buf);
        }
    }
}

impl Screen {
    // ------------------------------------------------------------------
    // Kitty graphics protocol (APC `ESC _ G ... ST`). Driven by the APC
    // pre-filter in `Terminal::advance`, which extracts complete bodies the
    // `vte` parser silently discards (vte 0.15 has no APC `Perform` callback).
    // ------------------------------------------------------------------

    /// Caps on the transmit-store so a hostile `a=t` stream cannot exhaust
    /// memory: at most this many stored images, and this many total bytes.
    const KITTY_STORE_MAX_IMAGES: usize = 64;
    const KITTY_STORE_MAX_BYTES: usize = 64 * 1024 * 1024;

    /// Handle one complete Kitty graphics APC body: the bytes between `ESC _`
    /// and the ST terminator, i.e. starting with `G`. Non-`G` APCs are the
    /// caller's responsibility to filter out (it only routes `ESC _ G` here).
    fn handle_kitty_apc(&mut self, body: &[u8]) {
        // Strip the leading 'G' graphics introducer.
        let body = match body.split_first() {
            Some((b'G', rest)) => rest,
            _ => return,
        };
        let cmd = match crate::image::parse_kitty(body) {
            Some(c) => c,
            None => return,
        };

        match cmd.action {
            'd' => {
                // Delete: clear all stored images and any in-flight chunks.
                self.kitty_store.clear();
                self.kitty_chunks.clear();
            }
            'p' => {
                // Display a previously transmit-stored image at the cursor.
                if let Some(img) = self.kitty_store.get(&cmd.id).cloned() {
                    self.place_kitty_image(img);
                }
            }
            't' | 'T' => {
                // Accumulate this chunk's base64 text keyed by image id. Format
                // + dimensions ride on the FIRST chunk only (continuation
                // chunks resend just `m` + payload), so capture them at creation
                // and reuse them when the m=0 boundary finalises the image.
                let chunk = self.kitty_chunks.entry(cmd.id).or_insert_with(|| KittyChunk {
                    payload: Vec::new(),
                    format: cmd.format,
                    width: cmd.width,
                    height: cmd.height,
                });
                if chunk.payload.len() + cmd.payload.len() <= 8 * 1024 * 1024 {
                    chunk.payload.extend_from_slice(&cmd.payload);
                } else {
                    // Oversized accumulation — drop the in-flight chunk set.
                    self.kitty_chunks.remove(&cmd.id);
                    return;
                }
                if cmd.more {
                    // More chunks follow — wait for the m=0 boundary.
                    return;
                }
                // Final (or only) chunk: decode the accumulated base64 once,
                // then decode pixels per the FIRST chunk's declared format.
                let chunk = match self.kitty_chunks.remove(&cmd.id) {
                    Some(c) => c,
                    None => return,
                };
                let raw = match osc::base64_decode(&chunk.payload) {
                    Some(r) => r,
                    None => return,
                };
                let decoded = match crate::image::decode_kitty(
                    chunk.format,
                    chunk.width,
                    chunk.height,
                    &raw,
                ) {
                    Some(d) => d,
                    None => return,
                };
                if cmd.action == 't' {
                    // Transmit-only: store by id for a later a=p display.
                    self.store_kitty_image(cmd.id, decoded);
                } else {
                    // a=T: display now, and also store if an id was given.
                    if cmd.id != 0 {
                        self.store_kitty_image(cmd.id, decoded.clone());
                    }
                    self.place_kitty_image(decoded);
                }
            }
            // Unknown action — ignore.
            _ => {}
        }
    }

    /// Anchor a decoded Kitty image at the current cursor position.
    fn place_kitty_image(&mut self, image: crate::image::DecodedImage) {
        self.images.push(TerminalImage {
            image,
            line: self.history.len() + self.row,
            col: self.col,
        });
    }

    /// Insert a decoded image into the transmit-store, evicting the oldest-keyed
    /// entries when the count or total-byte caps are exceeded.
    fn store_kitty_image(&mut self, id: u32, image: crate::image::DecodedImage) {
        self.kitty_store.insert(id, image);
        // Enforce the image-count cap (evict lowest ids first — deterministic).
        while self.kitty_store.len() > Self::KITTY_STORE_MAX_IMAGES {
            if let Some(&min_id) = self.kitty_store.keys().min() {
                self.kitty_store.remove(&min_id);
            } else {
                break;
            }
        }
        // Enforce the total-byte cap.
        let mut total: usize = self.kitty_store.values().map(|i| i.rgba.len()).sum();
        while total > Self::KITTY_STORE_MAX_BYTES && self.kitty_store.len() > 1 {
            if let Some(&min_id) = self.kitty_store.keys().min() {
                if let Some(removed) = self.kitty_store.remove(&min_id) {
                    total -= removed.rgba.len();
                }
            } else {
                break;
            }
        }
    }
}

/// Bound on a single accumulated APC body (mirrors the Sixel cap). A body that
/// exceeds this is dropped — a hostile stream cannot exhaust memory.
const KITTY_APC_MAX_BYTES: usize = 8 * 1024 * 1024;

/// State of the APC pre-filter that runs ahead of the `vte` parser.
///
/// `vte` 0.15's `Perform` trait has no APC callback, so APC strings
/// (`ESC _ … ST`) — exactly how the Kitty graphics protocol transmits — are
/// routed to vte's internal `SosPmApcString` state and silently discarded.
/// This small persistent state machine extracts complete `ESC _ G … ST` bodies
/// before vte sees them and passes every other byte through unchanged. An APC
/// can be split across `advance()` calls (PTY reads are arbitrary chunks), so
/// the state lives on the [`Terminal`] across calls.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ApcFilter {
    /// Not inside any escape sequence.
    Normal,
    /// Saw a lone `ESC` in `Normal`; deciding whether it introduces an APC.
    Esc,
    /// Inside an APC body (`ESC _` seen); accumulating until ST.
    /// `is_kitty` records whether the body began with `G` (Kitty graphics);
    /// non-Kitty APCs are accumulated only so we can swallow them (match vte).
    Apc { is_kitty: bool, seen_first: bool },
    /// Inside an APC and just saw `ESC`; `\` completes the ST terminator.
    ApcEsc,
}

/// A terminal: VT parser + screen state. Feed it PTY bytes; read its grid.
pub struct Terminal {
    parser: Parser,
    screen: Screen,
    /// Persistent APC pre-filter state (Kitty graphics extraction).
    apc_state: ApcFilter,
    /// Accumulated bytes of the in-progress APC body (between `ESC _` and ST).
    apc_accum: Vec<u8>,
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
            apc_state: ApcFilter::Normal,
            apc_accum: Vec::new(),
        }
    }

    /// Feed raw PTY bytes through the VT state machine.
    ///
    /// An APC pre-filter runs first: it extracts complete Kitty graphics APC
    /// bodies (`ESC _ G … ST`) that `vte` would otherwise silently discard, and
    /// forwards every non-APC byte to the `vte` parser in its original order.
    /// Passthrough bytes are batched in a scratch buffer and flushed to the
    /// parser on each state transition so byte ordering is preserved exactly.
    pub fn advance(&mut self, bytes: &[u8]) {
        // Fast path: when no escape sequence is in flight and the chunk has no
        // ESC byte, hand it straight to vte (the overwhelmingly common case).
        if self.apc_state == ApcFilter::Normal && !bytes.contains(&0x1b) {
            self.parser.advance(&mut self.screen, bytes);
            return;
        }

        let mut passthrough: Vec<u8> = Vec::new();
        for &b in bytes {
            match self.apc_state {
                ApcFilter::Normal => {
                    if b == 0x1b {
                        // Possible escape sequence — hold the ESC until we know.
                        self.apc_state = ApcFilter::Esc;
                    } else {
                        passthrough.push(b);
                    }
                }
                ApcFilter::Esc => {
                    if b == 0x5f {
                        // `ESC _` — APC string begins. Flush pending text first.
                        if !passthrough.is_empty() {
                            self.parser.advance(&mut self.screen, &passthrough);
                            passthrough.clear();
                        }
                        self.apc_accum.clear();
                        self.apc_state = ApcFilter::Apc {
                            is_kitty: false,
                            seen_first: false,
                        };
                    } else if b == 0x1b {
                        // Another ESC: emit the held ESC, stay in Esc for this one.
                        passthrough.push(0x1b);
                    } else {
                        // Not an APC: re-emit the held ESC + this byte intact so
                        // vte parses the original escape sequence unchanged.
                        passthrough.push(0x1b);
                        passthrough.push(b);
                        self.apc_state = ApcFilter::Normal;
                    }
                }
                ApcFilter::Apc {
                    is_kitty,
                    seen_first,
                } => {
                    if b == 0x07 {
                        // BEL terminates the APC.
                        self.finish_apc(is_kitty);
                    } else if b == 0x1b {
                        // Possible ST (`ESC \`) — wait for the next byte.
                        self.apc_state = ApcFilter::ApcEsc;
                    } else {
                        let is_kitty = if !seen_first {
                            b == b'G'
                        } else {
                            is_kitty
                        };
                        // Only accumulate Kitty bodies; bound the size.
                        if is_kitty {
                            if self.apc_accum.len() < KITTY_APC_MAX_BYTES {
                                self.apc_accum.push(b);
                            }
                        }
                        self.apc_state = ApcFilter::Apc {
                            is_kitty,
                            seen_first: true,
                        };
                    }
                }
                ApcFilter::ApcEsc => {
                    // We are inside an APC whose previous byte was ESC.
                    let was_kitty = self.apc_accum.first() == Some(&b'G');
                    if b == 0x5c {
                        // `ESC \` = ST — the APC ends here.
                        self.finish_apc(was_kitty);
                    } else if b == 0x07 {
                        // ESC then BEL: BEL still terminates the APC.
                        self.finish_apc(was_kitty);
                    } else if b == 0x1b {
                        // ESC ESC inside an APC — stay waiting for the terminator.
                        // (The intervening ESC is not part of the body.)
                    } else {
                        // The ESC was embedded in the body, not a terminator.
                        // Re-enter Apc and accumulate this byte (Kitty only).
                        if was_kitty && self.apc_accum.len() < KITTY_APC_MAX_BYTES {
                            self.apc_accum.push(b);
                        }
                        self.apc_state = ApcFilter::Apc {
                            is_kitty: was_kitty,
                            seen_first: true,
                        };
                    }
                }
            }
        }

        // Flush any trailing passthrough text. A half-finished escape/APC stays
        // in `self.apc_state` for the next advance() call.
        if !passthrough.is_empty() {
            self.parser.advance(&mut self.screen, &passthrough);
        }
    }

    /// Complete an APC body: dispatch Kitty graphics, swallow everything else
    /// (matching vte's discard behaviour), then return to Normal.
    fn finish_apc(&mut self, is_kitty: bool) {
        if is_kitty {
            let body = std::mem::take(&mut self.apc_accum);
            self.screen.handle_kitty_apc(&body);
        }
        self.apc_accum.clear();
        self.apc_state = ApcFilter::Normal;
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

    /// Drains pending bytes to write back to the PTY.
    ///
    /// These are replies the terminal owes the running program: OSC 4/10/11/12
    /// color query responses and OSC 52 clipboard read responses (only when
    /// reads are opted into). The host event loop calls this each frame and
    /// writes any returned bytes to the PTY master. Returns an empty vec when
    /// there is nothing to send. The terminal NEVER queues a reply for OSC 133
    /// marks (anti-CVE, capture-only).
    pub fn take_pty_response(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.screen.pty_response)
    }

    /// C14 — focus reporting (core half). The host calls this on every window
    /// focus change (e.g. a winit `WindowEvent::Focused(b)`): `true` for
    /// focus-in, `false` for focus-out. When DEC mode `?1004` is enabled, it
    /// queues `ESC [ I` (in) / `ESC [ O` (out) into the PTY-response buffer,
    /// which the host drains via [`Terminal::take_pty_response`]. When `?1004`
    /// is disabled (the default) it is a no-op, so the host can call it
    /// unconditionally without checking [`Terminal::focus_reporting`] first.
    pub fn focus_report(&mut self, focused: bool) {
        self.screen.focus_report(focused);
    }

    /// Drains the oldest pending OSC 52 clipboard-write request, if any.
    ///
    /// The host calls this each frame to learn whether a program requested a
    /// clipboard update, then applies it to the host clipboard. Only WRITE
    /// requests are ever produced here; clipboard READ requests are ignored
    /// unless [`Terminal::set_clipboard_read_enabled`] is opted into.
    pub fn take_clipboard_write(&mut self) -> Option<ClipboardWrite> {
        if self.screen.pending_clipboard_writes.is_empty() {
            None
        } else {
            Some(self.screen.pending_clipboard_writes.remove(0))
        }
    }

    /// Drains all pending OSC 52 clipboard-write requests at once.
    pub fn take_clipboard_writes(&mut self) -> Vec<ClipboardWrite> {
        std::mem::take(&mut self.screen.pending_clipboard_writes)
    }

    /// Enables (or disables) responding to OSC 52 clipboard READ requests.
    ///
    /// DEFAULT-OFF. When disabled (the default), an `OSC 52 ; c ; ?` query is
    /// silently dropped — the terminal never leaks host clipboard contents back
    /// to a program, which is the canonical OSC 52 read vulnerability. Even when
    /// enabled, the core never reads the host clipboard itself; the host
    /// supplies the text via [`Terminal::respond_clipboard_read`].
    pub fn set_clipboard_read_enabled(&mut self, enabled: bool) {
        self.screen.clipboard_read_enabled = enabled;
    }

    /// Returns whether OSC 52 clipboard READ responses are enabled.
    pub fn clipboard_read_enabled(&self) -> bool {
        self.screen.clipboard_read_enabled
    }

    /// Queues an OSC 52 clipboard READ reply for the given selection and text.
    ///
    /// No-op unless [`Terminal::clipboard_read_enabled`] is `true`. The host
    /// supplies the clipboard text so the core never reads the clipboard itself.
    /// Encoded as standard base64, matching the OSC 52 write encoding. The
    /// queued bytes are drained via [`Terminal::take_pty_response`].
    pub fn respond_clipboard_read(&mut self, selection: ClipboardSelection, text: &str) {
        if !self.screen.clipboard_read_enabled {
            return;
        }
        let sel = match selection {
            ClipboardSelection::Clipboard => 'c',
            ClipboardSelection::Primary => 'p',
        };
        let encoded = base64_encode(text.as_bytes());
        let resp = format!("\x1b]52;{};{}\x07", sel, encoded);
        self.screen.pty_response.extend_from_slice(resp.as_bytes());
    }

    /// Drains all pending color-set requests (OSC 4 / 10 / 11 / 12 / 104 / 11x).
    ///
    /// The host applies each to its live theme. Reset operations are surfaced as
    /// sets carrying the default color value.
    pub fn take_color_sets(&mut self) -> Vec<ColorSet> {
        std::mem::take(&mut self.screen.pending_color_sets)
    }

    /// Drains the oldest pending desktop notification (OSC 9 / OSC 777).
    pub fn take_notification(&mut self) -> Option<Notification> {
        if self.screen.pending_notifications.is_empty() {
            None
        } else {
            Some(self.screen.pending_notifications.remove(0))
        }
    }

    /// Drains all pending desktop notifications at once.
    pub fn take_notifications(&mut self) -> Vec<Notification> {
        std::mem::take(&mut self.screen.pending_notifications)
    }

    /// Whether DECSCNM reverse-video screen mode (`?5`, C25) is active. When
    /// `true`, the renderer should swap the default fg/bg for the whole screen.
    pub fn reverse_screen(&self) -> bool {
        self.screen.reverse_screen
    }

    /// Whether IRM insert mode (`CSI 4 h`, C25) is active. Exposed for the
    /// renderer/host; printing already honours it internally.
    pub fn insert_mode(&self) -> bool {
        self.screen.insert_mode
    }

    /// Whether DECOM origin mode (`?6`, C25) is active (cursor addressing is
    /// relative to the scroll region).
    pub fn origin_mode(&self) -> bool {
        self.screen.origin_mode
    }

    /// Drains all pending `OSC 9 ; 4` taskbar/tab progress reports (C26). The
    /// host applies the most recent to its tab/taskbar progress indicator.
    pub fn take_progress(&mut self) -> Vec<osc::Progress> {
        std::mem::take(&mut self.screen.pending_progress)
    }

    /// The captured OSC 133 command-zone marks (C28): output-start (`C`) and
    /// command-end (`D`, with optional exit code). Capture-only — the terminal
    /// never reports these back to the PTY.
    pub fn command_marks(&self) -> &[osc::CommandMark] {
        &self.screen.command_marks
    }

    /// Returns the current indexed-palette color (0-255) as an `(r, g, b)`
    /// triple. Reflects OSC 4 sets and OSC 104 resets.
    pub fn palette_color(&self, index: u8) -> (u8, u8, u8) {
        self.screen.palette[index as usize]
    }

    /// Returns the current dynamic color (fg/bg/cursor) as an `(r, g, b)`
    /// triple. Reflects OSC 10/11/12 sets and OSC 110/111/112 resets.
    pub fn dynamic_color(&self, which: DynamicColor) -> (u8, u8, u8) {
        match which {
            DynamicColor::Foreground => self.screen.dynamic_fg,
            DynamicColor::Background => self.screen.dynamic_bg,
            DynamicColor::Cursor => self.screen.dynamic_cursor,
        }
    }

    /// Returns the current window title-stack depth (XTWINOPS 22/23).
    pub fn title_stack_depth(&self) -> usize {
        self.screen.title_stack.len()
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

    /// The cursor position in **display space** (`(row, col)`, 0-based) — the
    /// coordinate system of [`Terminal::display_rows`], so the renderer can map
    /// it straight onto the visible grid. Returns `None` when the cursor is not
    /// in the visible window: the live cursor sits in the bottom `grid.rows()`
    /// lines, so any non-zero scrollback `view_offset` scrolls it out of view
    /// (a terminal hides the cursor while you scroll back). The column is
    /// clamped to the last grid column.
    pub fn cursor_position(&self) -> Option<(usize, usize)> {
        if self.screen.view_offset != 0 {
            return None;
        }
        let cols = self.screen.grid.cols();
        let row = self.screen.row.min(self.screen.grid.rows().saturating_sub(1));
        let col = self.screen.col.min(cols.saturating_sub(1));
        Some((row, col))
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
        let rows = rows.max(1);
        let cols = cols.max(1);
        let old_cols = self.screen.grid.cols();

        // C16 — reflow on resize. When the column count changes on the PRIMARY
        // screen, re-wrap soft-wrapped logical lines to the new width instead of
        // truncating (the lossy `Grid::resize` path). The alternate screen holds
        // a full-screen TUI that redraws itself on SIGWINCH, so reflowing it is
        // both unnecessary and wrong (alt content has no logical-line history) —
        // there we fall back to the simple grid resize.
        if cols != old_cols && self.screen.saved_primary.is_none() {
            self.screen.reflow_resize(rows, cols);
        } else {
            self.screen.grid.resize(rows, cols);
            self.screen.row = self.screen.row.min(rows - 1);
            self.screen.col = self.screen.col.min(cols - 1);
            self.screen.resize_tab_stops(cols);
        }

        // A full-screen scroll region tracks the new height; a custom region is
        // clamped and reset if it no longer fits.
        if self.screen.scroll_bottom + 1 >= self.screen.grid.rows()
            || self.screen.scroll_bottom == 0
        {
            self.screen.scroll_top = 0;
            self.screen.scroll_bottom = rows - 1;
        } else {
            self.screen.clamp_scroll_region();
        }
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
    fn cursor_position_tracks_display_space() {
        let mut t = Terminal::new(4, 20);
        assert_eq!(t.cursor_position(), Some((0, 0)), "home at start");
        t.advance(b"hello");
        assert_eq!(t.cursor_position(), Some((0, 5)), "advanced 5 cols");
        t.advance(b"\r\nx");
        assert_eq!(t.cursor_position(), Some((1, 1)), "next row, 1 col");
        // CSI H homes the cursor.
        t.advance(b"\x1b[H");
        assert_eq!(t.cursor_position(), Some((0, 0)), "CUP home");
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

    // ---- OSC 52 clipboard ----

    #[test]
    fn osc52_clipboard_write_decodes_base64() {
        let mut t = Terminal::new(4, 20);
        // "hello" base64 = aGVsbG8=
        t.advance(b"\x1b]52;c;aGVsbG8=\x07");
        let w = t.take_clipboard_write().expect("clipboard write");
        assert_eq!(w.selection, ClipboardSelection::Clipboard);
        assert_eq!(w.text, "hello");
        assert!(t.take_clipboard_write().is_none(), "drained once");
    }

    #[test]
    fn osc52_primary_selection() {
        let mut t = Terminal::new(4, 20);
        t.advance(b"\x1b]52;p;aGVsbG8=\x07");
        let w = t.take_clipboard_write().unwrap();
        assert_eq!(w.selection, ClipboardSelection::Primary);
    }

    #[test]
    fn osc52_read_default_off_emits_nothing() {
        let mut t = Terminal::new(4, 20);
        // Read request: payload is '?'. Default-off -> no PTY response, no write.
        t.advance(b"\x1b]52;c;?\x07");
        assert!(t.take_clipboard_write().is_none());
        assert!(
            t.take_pty_response().is_empty(),
            "must NOT auto-respond with host clipboard contents"
        );
    }

    #[test]
    fn osc52_read_opt_in_uses_app_provided_text() {
        let mut t = Terminal::new(4, 20);
        // Even opted in, the core never reads the host clipboard from the OSC
        // sequence; the host must supply the text explicitly.
        t.set_clipboard_read_enabled(true);
        t.advance(b"\x1b]52;c;?\x07");
        assert!(
            t.take_pty_response().is_empty(),
            "the read request alone emits nothing"
        );
        t.respond_clipboard_read(ClipboardSelection::Clipboard, "hi");
        // "hi" base64 = aGk=
        assert_eq!(t.take_pty_response().as_slice(), b"\x1b]52;c;aGk=\x07");
    }

    // ---- OSC 4 / 10 / 11 / 12 colors ----

    #[test]
    fn osc4_set_indexed_color() {
        let mut t = Terminal::new(4, 20);
        t.advance(b"\x1b]4;1;rgb:ff/00/00\x07");
        assert_eq!(t.palette_color(1), (255, 0, 0));
        assert_eq!(
            t.take_color_sets(),
            vec![ColorSet::Indexed {
                index: 1,
                rgb: (255, 0, 0)
            }]
        );
    }

    #[test]
    fn osc4_query_emits_reply() {
        let mut t = Terminal::new(4, 20);
        t.advance(b"\x1b]4;1;rgb:ff/00/00\x07");
        let _ = t.take_color_sets();
        t.advance(b"\x1b]4;1;?\x07");
        assert_eq!(
            t.take_pty_response().as_slice(),
            b"\x1b]4;1;rgb:ffff/0000/0000\x07"
        );
    }

    #[test]
    fn osc11_background_query_emits_reply() {
        let mut t = Terminal::new(4, 20);
        // Default background is xterm black -> rgb:0000/0000/0000
        t.advance(b"\x1b]11;?\x07");
        assert_eq!(
            t.take_pty_response().as_slice(),
            b"\x1b]11;rgb:0000/0000/0000\x07"
        );
    }

    #[test]
    fn osc10_foreground_set() {
        let mut t = Terminal::new(4, 20);
        t.advance(b"\x1b]10;rgb:12/34/56\x07");
        assert_eq!(t.dynamic_color(DynamicColor::Foreground), (0x12, 0x34, 0x56));
        assert_eq!(
            t.take_color_sets(),
            vec![ColorSet::Dynamic {
                which: DynamicColor::Foreground,
                rgb: (0x12, 0x34, 0x56)
            }]
        );
    }

    #[test]
    fn osc12_cursor_set() {
        let mut t = Terminal::new(4, 20);
        t.advance(b"\x1b]12;rgb:00/ff/00\x07");
        assert_eq!(t.dynamic_color(DynamicColor::Cursor), (0, 255, 0));
    }

    // ---- OSC 104 / 110-112 reset ----

    #[test]
    fn osc104_reset_single_index() {
        let mut t = Terminal::new(4, 20);
        let original = t.palette_color(2);
        t.advance(b"\x1b]4;2;rgb:ff/00/00\x07");
        let _ = t.take_color_sets();
        assert_eq!(t.palette_color(2), (255, 0, 0));
        t.advance(b"\x1b]104;2\x07");
        assert_eq!(t.palette_color(2), original);
    }

    #[test]
    fn osc104_reset_all() {
        let mut t = Terminal::new(4, 20);
        t.advance(b"\x1b]4;5;rgb:ff/00/00\x07");
        let _ = t.take_color_sets();
        t.advance(b"\x1b]104\x07");
        assert_ne!(t.palette_color(5), (255, 0, 0));
        assert_eq!(t.take_color_sets().len(), 256, "every entry reset is surfaced");
    }

    #[test]
    fn osc110_reset_foreground() {
        let mut t = Terminal::new(4, 20);
        let original = t.dynamic_color(DynamicColor::Foreground);
        t.advance(b"\x1b]10;rgb:ff/ff/ff\x07");
        let _ = t.take_color_sets();
        t.advance(b"\x1b]110\x07");
        assert_eq!(t.dynamic_color(DynamicColor::Foreground), original);
    }

    // ---- OSC 9 / 777 notifications ----

    #[test]
    fn osc9_notification() {
        let mut t = Terminal::new(4, 20);
        t.advance(b"\x1b]9;Build complete\x07");
        let n = t.take_notification().expect("notification");
        assert_eq!(n.title, "");
        assert_eq!(n.body, "Build complete");
    }

    #[test]
    fn osc777_notification() {
        let mut t = Terminal::new(4, 20);
        t.advance(b"\x1b]777;notify;Title;Body text\x07");
        let n = t.take_notification().unwrap();
        assert_eq!(n.title, "Title");
        assert_eq!(n.body, "Body text");
    }

    // ---- Title stack (XTWINOPS 22/23) ----

    #[test]
    fn title_stack_push_pop() {
        let mut t = Terminal::new(4, 20);
        t.advance(b"\x1b]2;First\x07");
        assert_eq!(t.title(), "First");
        t.advance(b"\x1b[22;0t"); // push
        assert_eq!(t.title_stack_depth(), 1);
        t.advance(b"\x1b]2;Second\x07");
        assert_eq!(t.title(), "Second");
        t.advance(b"\x1b[23;0t"); // pop
        assert_eq!(t.title(), "First");
        assert_eq!(t.title_stack_depth(), 0);
    }

    #[test]
    fn title_stack_pop_empty_is_noop() {
        let mut t = Terminal::new(4, 20);
        t.advance(b"\x1b]2;Only\x07");
        t.advance(b"\x1b[23;0t"); // pop with empty stack
        assert_eq!(t.title(), "Only");
    }

    // ---- OSC 133 still never replies (regression guard) ----

    #[test]
    fn osc133_never_writes_pty_response() {
        let mut t = Terminal::new(4, 20);
        t.advance(b"\x1b]133;A\x07");
        assert!(
            t.take_pty_response().is_empty(),
            "OSC 133 marks must remain capture-only (anti-CVE)"
        );
    }

    // ---- Kitty graphics protocol (APC extraction + decode + placement) ----

    #[test]
    fn kitty_apc_displays_image_and_passes_text_through() {
        let mut t = Terminal::new(4, 20);
        // "hi" then a Kitty APC (a defaults to T) for a 1x1 RGBA image
        // (payload [1,2,3,4] = base64 "AQIDBA=="), ST-terminated, then "ok".
        t.advance(b"hi\x1b_Gf=32,s=1,v=1;AQIDBA==\x1b\\ok");
        // Exactly one image at the cursor.
        assert_eq!(t.images().len(), 1, "one image produced");
        let img = &t.images()[0].image;
        assert_eq!(img.width, 1);
        assert_eq!(img.height, 1);
        assert_eq!(&img.rgba, &[1, 2, 3, 4]);
        // The non-APC text reaches the grid intact ("hiok").
        assert!(
            t.grid().to_text().starts_with("hiok"),
            "non-APC bytes must reach the grid: got {:?}",
            t.grid().to_text()
        );
    }

    #[test]
    fn kitty_apc_split_across_two_advances() {
        let mut t = Terminal::new(4, 20);
        // Cut the APC mid-payload across two advance() calls.
        t.advance(b"hi\x1b_Gf=32,s=1,v=1;AQID");
        // Nothing finalised yet; "hi" is on the grid, no image.
        assert_eq!(t.images().len(), 0, "APC not yet terminated");
        t.advance(b"BA==\x1b\\ok");
        assert_eq!(t.images().len(), 1, "one image after the second chunk");
        assert_eq!(&t.images()[0].image.rgba, &[1, 2, 3, 4]);
        assert!(
            t.grid().to_text().starts_with("hiok"),
            "no stray APC bytes leak to the grid: got {:?}",
            t.grid().to_text()
        );
    }

    #[test]
    fn kitty_apc_bel_terminated() {
        let mut t = Terminal::new(4, 20);
        // BEL (0x07) terminates the APC instead of ST.
        t.advance(b"x\x1b_Gf=32,s=1,v=1;AQIDBA==\x07y");
        assert_eq!(t.images().len(), 1, "BEL-terminated APC produces an image");
        assert_eq!(&t.images()[0].image.rgba, &[1, 2, 3, 4]);
        assert!(t.grid().to_text().starts_with("xy"));
    }

    #[test]
    fn kitty_chunked_transmission_m_flag() {
        let mut t = Terminal::new(4, 20);
        // 1x1 RGBA split into two base64 chunks via m=1 / m=0, same id.
        // "AQID" then "BA==" together decode to [1,2,3,4].
        t.advance(b"\x1b_Gf=32,s=1,v=1,i=9,m=1;AQID\x1b\\");
        assert_eq!(t.images().len(), 0, "more=1 chunk does not finalise");
        t.advance(b"\x1b_Ga=T,i=9,m=0;BA==\x1b\\");
        assert_eq!(t.images().len(), 1, "m=0 finalises the chunked image");
        assert_eq!(&t.images()[0].image.rgba, &[1, 2, 3, 4]);
    }

    #[test]
    fn kitty_transmit_only_then_display() {
        let mut t = Terminal::new(4, 20);
        // a=t stores the image without displaying it.
        t.advance(b"\x1b_Ga=t,f=32,s=1,v=1,i=5;AQIDBA==\x1b\\");
        assert_eq!(t.images().len(), 0, "a=t must not display");
        // a=p displays the previously-stored id.
        t.advance(b"\x1b_Ga=p,i=5\x1b\\");
        assert_eq!(t.images().len(), 1, "a=p displays the stored image");
        assert_eq!(&t.images()[0].image.rgba, &[1, 2, 3, 4]);
    }

    #[test]
    fn kitty_delete_clears_storage() {
        let mut t = Terminal::new(4, 20);
        t.advance(b"\x1b_Ga=t,f=32,s=1,v=1,i=3;AQIDBA==\x1b\\");
        // a=d clears the store; a later a=p finds nothing.
        t.advance(b"\x1b_Ga=d\x1b\\");
        t.advance(b"\x1b_Ga=p,i=3\x1b\\");
        assert_eq!(t.images().len(), 0, "a=d cleared the stored image");
    }

    #[test]
    fn kitty_f24_rgb_displayed_as_rgba() {
        let mut t = Terminal::new(4, 20);
        // 1x1 RGB (3 bytes [10,20,30] = base64 "ChQe"); expands to RGBA.
        t.advance(b"\x1b_Gf=24,s=1,v=1;ChQe\x1b\\");
        assert_eq!(t.images().len(), 1);
        assert_eq!(&t.images()[0].image.rgba, &[10, 20, 30, 255]);
    }

    #[test]
    fn non_kitty_apc_is_swallowed_and_text_survives() {
        let mut t = Terminal::new(4, 20);
        // A non-graphics APC (no leading G) is swallowed (matching vte); the
        // surrounding text still reaches the grid and no image is produced.
        t.advance(b"a\x1b_Xsome-other-apc\x1b\\b");
        assert_eq!(t.images().len(), 0);
        assert!(
            t.grid().to_text().starts_with("ab"),
            "text around a non-kitty APC survives: got {:?}",
            t.grid().to_text()
        );
    }

    #[test]
    fn esc_not_introducing_apc_passes_through_intact() {
        let mut t = Terminal::new(4, 20);
        // A plain SGR escape (ESC not followed by '_') must reach vte intact.
        t.advance(b"\x1b[31mR");
        assert_eq!(t.grid().cell(0, 0).unwrap().c, 'R');
        assert_eq!(t.grid().cell(0, 0).unwrap().fg, Color::Indexed(1));
        assert_eq!(t.images().len(), 0);
    }

    // ============================================================
    // VT correctness P0 batch (C1-C8)
    // ============================================================

    // ---- C1: DA1 / DA2 device attributes ----

    #[test]
    fn da1_primary_device_attributes_reply() {
        let mut t = Terminal::new(4, 20);
        t.advance(b"\x1b[c");
        assert_eq!(t.take_pty_response().as_slice(), b"\x1b[?62;1;6;22c");
    }

    #[test]
    fn da1_with_explicit_zero_param_replies() {
        let mut t = Terminal::new(4, 20);
        t.advance(b"\x1b[0c");
        assert_eq!(t.take_pty_response().as_slice(), b"\x1b[?62;1;6;22c");
    }

    #[test]
    fn da2_secondary_device_attributes_reply() {
        let mut t = Terminal::new(4, 20);
        t.advance(b"\x1b[>c");
        assert_eq!(t.take_pty_response().as_slice(), b"\x1b[>0;0;0c");
    }

    // ---- C2: DSR / CPR ----

    #[test]
    fn dsr_status_report_ok() {
        let mut t = Terminal::new(4, 20);
        t.advance(b"\x1b[5n");
        assert_eq!(t.take_pty_response().as_slice(), b"\x1b[0n");
    }

    #[test]
    fn cpr_reports_one_based_cursor_position() {
        let mut t = Terminal::new(10, 40);
        // Move cursor to row 3, col 7 (0-based 2,6) then request CPR.
        t.advance(b"\x1b[3;7H\x1b[6n");
        assert_eq!(t.take_pty_response().as_slice(), b"\x1b[3;7R");
    }

    #[test]
    fn cpr_after_printing_text() {
        let mut t = Terminal::new(4, 20);
        t.advance(b"hello\x1b[6n"); // cursor at row1 col6 (1-based)
        assert_eq!(t.take_pty_response().as_slice(), b"\x1b[1;6R");
    }

    // ---- C3: IL / DL / ICH / DCH / ECH ----

    #[test]
    fn ich_inserts_blanks_shifting_right() {
        let mut t = Terminal::new(2, 6);
        t.advance(b"abcdef\x1b[H"); // fill row 0, home
        t.advance(b"\x1b[3G"); // move to col 3 (1-based) = 0-based 2 ('c')
        t.advance(b"\x1b[2@"); // insert 2 blanks
        let line: String = (0..6).map(|c| t.grid().cell(0, c).unwrap().c).collect();
        assert_eq!(line, "ab  cd");
    }

    #[test]
    fn dch_deletes_chars_shifting_left() {
        let mut t = Terminal::new(2, 6);
        t.advance(b"abcdef\x1b[H");
        t.advance(b"\x1b[3G\x1b[2P"); // at col 3, delete 2 chars
        let line: String = (0..6).map(|c| t.grid().cell(0, c).unwrap().c).collect();
        assert_eq!(line, "abef  ");
    }

    #[test]
    fn ech_erases_chars_without_shift() {
        let mut t = Terminal::new(2, 6);
        t.advance(b"abcdef\x1b[H");
        t.advance(b"\x1b[3G\x1b[2X"); // at col 3, erase 2
        let line: String = (0..6).map(|c| t.grid().cell(0, c).unwrap().c).collect();
        assert_eq!(line, "ab  ef");
    }

    #[test]
    fn il_inserts_lines_within_scroll_region() {
        let mut t = Terminal::new(4, 3);
        t.advance(b"aaa\r\nbbb\r\nccc\r\nddd");
        // Cursor to row 2 (1-based), insert 1 line.
        t.advance(b"\x1b[2;1H\x1b[L");
        assert_eq!(t.grid().cell(0, 0).unwrap().c, 'a');
        assert_eq!(t.grid().cell(1, 0).unwrap().c, ' ', "blank inserted at row 2");
        assert_eq!(t.grid().cell(2, 0).unwrap().c, 'b', "bbb shifted down");
        assert_eq!(t.grid().cell(3, 0).unwrap().c, 'c', "ccc shifted down; ddd lost");
    }

    #[test]
    fn dl_deletes_lines_within_scroll_region() {
        let mut t = Terminal::new(4, 3);
        t.advance(b"aaa\r\nbbb\r\nccc\r\nddd");
        // Cursor to row 2, delete 1 line.
        t.advance(b"\x1b[2;1H\x1b[M");
        assert_eq!(t.grid().cell(0, 0).unwrap().c, 'a');
        assert_eq!(t.grid().cell(1, 0).unwrap().c, 'c', "ccc shifted up");
        assert_eq!(t.grid().cell(2, 0).unwrap().c, 'd', "ddd shifted up");
        assert_eq!(t.grid().cell(3, 0).unwrap().c, ' ', "blank at bottom");
    }

    #[test]
    fn il_dl_respect_custom_scroll_region() {
        let mut t = Terminal::new(5, 3);
        t.advance(b"r0\r\nr1\r\nr2\r\nr3\r\nr4");
        // Region rows 2..4 (1-based), i.e. 0-based 1..3.
        t.advance(b"\x1b[2;4r");
        // After DECSTBM the cursor is homed to the top margin (row 1, 0-based).
        // Delete 1 line at the top of the region.
        t.advance(b"\x1b[M");
        assert_eq!(t.grid().cell(0, 0).unwrap().c, 'r', "row 0 untouched");
        assert_eq!(t.grid().cell(0, 1).unwrap().c, '0');
        assert_eq!(t.grid().cell(1, 1).unwrap().c, '2', "r2 shifted up into region top");
        assert_eq!(t.grid().cell(3, 1).unwrap().c, ' ', "blank at region bottom");
        assert_eq!(t.grid().cell(4, 1).unwrap().c, '4', "row 4 below region untouched");
    }

    // ---- C4: DECSC / DECRC + SCOSC / SCORC ----

    #[test]
    fn decsc_decrc_round_trips_cursor() {
        let mut t = Terminal::new(6, 20);
        t.advance(b"\x1b[3;5H"); // row 3 col 5 (1-based)
        t.advance(b"\x1b7"); // DECSC save
        t.advance(b"\x1b[1;1H"); // home
        assert_eq!(t.cursor_position(), Some((0, 0)));
        t.advance(b"\x1b8"); // DECRC restore
        assert_eq!(t.cursor_position(), Some((2, 4)), "restored to row3,col5");
    }

    #[test]
    fn scosc_scorc_aliases_save_restore() {
        let mut t = Terminal::new(6, 20);
        t.advance(b"\x1b[4;3H\x1b[s"); // CSI s save
        t.advance(b"\x1b[1;1H\x1b[u"); // home then CSI u restore
        assert_eq!(t.cursor_position(), Some((3, 2)));
    }

    #[test]
    fn decsc_saves_pen() {
        let mut t = Terminal::new(2, 10);
        t.advance(b"\x1b[31m\x1b7"); // red pen, save
        t.advance(b"\x1b[0m"); // reset pen
        t.advance(b"\x1b8X"); // restore -> prints red X
        assert_eq!(t.grid().cell(0, 0).unwrap().fg, Color::Indexed(1));
    }

    // ---- C5: DECSTBM scroll region ----

    #[test]
    fn decstbm_constrains_scrolling() {
        let mut t = Terminal::new(4, 3);
        // Region = rows 1..2 (1-based) = 0-based 0..1.
        t.advance(b"\x1b[1;2r");
        // Fill the region and force a scroll: rows below the region stay put.
        t.advance(b"x3\r\n"); // row3 marker first
        // Reset region to write a fixed bottom line, then re-set region.
        t.advance(b"\x1b[1;4r\x1b[4;1Hbot\x1b[1;2r\x1b[1;1H");
        // Now scroll within region 0..1 by printing 3 lines.
        t.advance(b"AA\r\nBB\r\nCC");
        // Region top should now hold BB (AA scrolled out of the 2-row region).
        assert_eq!(t.grid().cell(0, 0).unwrap().c, 'B');
        assert_eq!(t.grid().cell(1, 0).unwrap().c, 'C');
        // The fixed bottom line outside the region is preserved.
        let bottom: String = (0..3).map(|c| t.grid().cell(3, c).unwrap().c).collect();
        assert_eq!(bottom, "bot");
    }

    #[test]
    fn decstbm_no_params_resets_full_screen() {
        let mut t = Terminal::new(3, 3);
        t.advance(b"\x1b[1;2r"); // custom region
        t.advance(b"\x1b[r"); // reset
        // Full-screen scroll feeds scrollback again.
        let mut t2 = Terminal::with_scrollback(3, 3, 100);
        t2.advance(b"\x1b[1;2r\x1b[r");
        t2.advance(b"a\r\nb\r\nc\r\nd");
        assert!(t2.scrollback_len() >= 1, "full region feeds scrollback after reset");
        let _ = t;
    }

    // ---- C6: DEC line-drawing charset ----

    #[test]
    fn dec_line_drawing_maps_box_chars() {
        let mut t = Terminal::new(2, 10);
        t.advance(b"\x1b(0"); // select DEC special graphics into G0
        t.advance(b"lqk"); // upper-left, horiz, upper-right
        assert_eq!(t.grid().cell(0, 0).unwrap().c, '\u{250c}'); // ┌
        assert_eq!(t.grid().cell(0, 1).unwrap().c, '\u{2500}'); // ─
        assert_eq!(t.grid().cell(0, 2).unwrap().c, '\u{2510}'); // ┐
    }

    #[test]
    fn esc_paren_b_returns_to_ascii() {
        let mut t = Terminal::new(2, 10);
        t.advance(b"\x1b(0q\x1b(Bq"); // graphics q (─) then ASCII q
        assert_eq!(t.grid().cell(0, 0).unwrap().c, '\u{2500}');
        assert_eq!(t.grid().cell(0, 1).unwrap().c, 'q', "ASCII restored");
    }

    #[test]
    fn si_so_switch_g0_g1() {
        let mut t = Terminal::new(2, 10);
        // G0 = ASCII (default), G1 = line-drawing.
        t.advance(b"\x1b)0"); // designate G1 = graphics
        t.advance(b"q"); // GL=G0=ASCII -> 'q'
        t.advance(b"\x0eq"); // SO -> GL=G1=graphics -> ─
        t.advance(b"\x0fq"); // SI -> back to G0 ASCII -> 'q'
        assert_eq!(t.grid().cell(0, 0).unwrap().c, 'q');
        assert_eq!(t.grid().cell(0, 1).unwrap().c, '\u{2500}');
        assert_eq!(t.grid().cell(0, 2).unwrap().c, 'q');
    }

    // ---- C7: wide-cell width ----

    #[test]
    fn wide_char_advances_two_columns() {
        let mut t = Terminal::new(2, 10);
        t.advance("世".as_bytes()); // East-Asian wide
        // Occupies cols 0 + 1 (continuation spacer); cursor now at col 2.
        assert_eq!(t.grid().cell(0, 0).unwrap().c, '世');
        assert_eq!(t.grid().cell(0, 1).unwrap().c, ' ', "continuation spacer");
        assert_eq!(t.cursor_position(), Some((0, 2)));
    }

    #[test]
    fn wide_char_then_narrow() {
        let mut t = Terminal::new(2, 10);
        t.advance("世a".as_bytes());
        assert_eq!(t.grid().cell(0, 0).unwrap().c, '世');
        assert_eq!(t.grid().cell(0, 2).unwrap().c, 'a', "narrow lands at col 2");
    }

    #[test]
    fn wide_char_wraps_at_last_column() {
        let mut t = Terminal::new(2, 3); // 3 cols
        t.advance(b"ab"); // cols 0,1 filled; cursor at col 2 (last)
        t.advance("世".as_bytes()); // can't fit width-2 at col 2 -> wraps to row 1
        assert_eq!(t.grid().cell(0, 0).unwrap().c, 'a');
        assert_eq!(t.grid().cell(0, 1).unwrap().c, 'b');
        assert_eq!(t.grid().cell(1, 0).unwrap().c, '世', "wide char wrapped to next row");
        assert_eq!(t.grid().cell(1, 1).unwrap().c, ' ');
    }

    // ---- C8: ED / EL sub-modes ----

    #[test]
    fn ed_mode0_erases_cursor_to_end() {
        let mut t = Terminal::new(2, 4);
        t.advance(b"abcd\r\nefgh");
        t.advance(b"\x1b[1;3H\x1b[0J"); // row1 col3, erase to end
        assert_eq!(t.grid().cell(0, 0).unwrap().c, 'a');
        assert_eq!(t.grid().cell(0, 1).unwrap().c, 'b');
        assert_eq!(t.grid().cell(0, 2).unwrap().c, ' ', "from cursor erased");
        assert_eq!(t.grid().cell(1, 0).unwrap().c, ' ', "rows below erased");
    }

    #[test]
    fn ed_mode1_erases_start_to_cursor() {
        let mut t = Terminal::new(2, 4);
        t.advance(b"abcd\r\nefgh");
        t.advance(b"\x1b[2;2H\x1b[1J"); // row2 col2, erase start->cursor
        assert_eq!(t.grid().cell(0, 0).unwrap().c, ' ', "row above erased");
        assert_eq!(t.grid().cell(1, 0).unwrap().c, ' ');
        assert_eq!(t.grid().cell(1, 1).unwrap().c, ' ', "cursor cell inclusive");
        assert_eq!(t.grid().cell(1, 2).unwrap().c, 'g', "after cursor kept");
    }

    #[test]
    fn ed_mode3_clears_scrollback() {
        let mut t = Terminal::with_scrollback(2, 4, 100);
        t.advance(b"L0\r\nL1\r\nL2\r\nL3");
        assert!(t.scrollback_len() > 0);
        t.advance(b"\x1b[3J");
        assert_eq!(t.scrollback_len(), 0, "ESC[3J clears scrollback");
    }

    #[test]
    fn el_mode1_erases_bol_to_cursor() {
        let mut t = Terminal::new(2, 5);
        t.advance(b"abcde");
        t.advance(b"\x1b[1;3H\x1b[1K"); // col3, erase BOL->cursor
        assert_eq!(t.grid().cell(0, 0).unwrap().c, ' ');
        assert_eq!(t.grid().cell(0, 1).unwrap().c, ' ');
        assert_eq!(t.grid().cell(0, 2).unwrap().c, ' ', "cursor inclusive");
        assert_eq!(t.grid().cell(0, 3).unwrap().c, 'd', "after cursor kept");
    }

    #[test]
    fn el_mode2_erases_whole_line() {
        let mut t = Terminal::new(2, 5);
        t.advance(b"abcde\x1b[1;3H\x1b[2K");
        for c in 0..5 {
            assert_eq!(t.grid().cell(0, c).unwrap().c, ' ');
        }
    }

    // ---- C9/C10 bonus: ESC M reverse index, RIS, DECSTR ----

    #[test]
    fn reverse_index_scrolls_region_down_at_top() {
        let mut t = Terminal::new(3, 3);
        t.advance(b"aaa\r\nbbb\r\nccc");
        t.advance(b"\x1b[1;1H\x1bM"); // home then RI -> scroll down
        assert_eq!(t.grid().cell(0, 0).unwrap().c, ' ', "blank scrolled in at top");
        assert_eq!(t.grid().cell(1, 0).unwrap().c, 'a', "aaa pushed down");
    }

    #[test]
    fn ris_resets_terminal() {
        let mut t = Terminal::with_scrollback(2, 4, 100);
        t.advance(b"junk\r\nmore\r\noverflow\x1b[31m");
        t.advance(b"\x1bc"); // RIS
        assert_eq!(t.grid().cell(0, 0).unwrap().c, ' ', "screen cleared");
        assert_eq!(t.scrollback_len(), 0, "scrollback cleared");
        assert_eq!(t.cursor_position(), Some((0, 0)));
        t.advance(b"x"); // pen reset -> default fg
        assert_eq!(t.grid().cell(0, 0).unwrap().fg, Color::Default);
    }

    #[test]
    fn decstr_soft_reset_keeps_scrollback() {
        let mut t = Terminal::with_scrollback(2, 4, 100);
        t.advance(b"L0\r\nL1\r\nL2");
        let hist = t.scrollback_len();
        t.advance(b"\x1b[!p"); // DECSTR soft reset
        assert_eq!(t.scrollback_len(), hist, "soft reset preserves scrollback");
        assert_eq!(t.cursor_position(), Some((0, 0)));
    }

    // ---- Bonus: REP, CHA/VPA absolute moves ----

    #[test]
    fn rep_repeats_last_char() {
        let mut t = Terminal::new(2, 10);
        t.advance(b"x\x1b[3b"); // print x, repeat 3 more
        let line: String = (0..4).map(|c| t.grid().cell(0, c).unwrap().c).collect();
        assert_eq!(line, "xxxx");
    }

    #[test]
    fn cha_vpa_absolute_moves() {
        let mut t = Terminal::new(5, 10);
        t.advance(b"\x1b[5G"); // column 5 (1-based) -> col 4
        assert_eq!(t.cursor_position(), Some((0, 4)));
        t.advance(b"\x1b[3d"); // row 3 (1-based) -> row 2
        assert_eq!(t.cursor_position(), Some((2, 4)));
    }

    // ============================================================
    // VT correctness P1 batch (C14 / C16 / C19)
    // ============================================================

    // ---- C19: settable tab stops (HTS / TBC / CHT / CBT) ----

    #[test]
    fn tab_default_stops_every_eight() {
        let mut t = Terminal::new(2, 30);
        t.advance(b"\t"); // col 0 -> 8
        assert_eq!(t.cursor_position(), Some((0, 8)));
        t.advance(b"\t"); // 8 -> 16
        assert_eq!(t.cursor_position(), Some((0, 16)));
    }

    #[test]
    fn tab_from_mid_default_stop_advances_to_next_multiple() {
        let mut t = Terminal::new(2, 30);
        t.advance(b"abc\t"); // col 3 -> next stop at 8
        assert_eq!(t.cursor_position(), Some((0, 8)));
    }

    #[test]
    fn hts_sets_custom_tab_stop() {
        let mut t = Terminal::new(2, 30);
        // Move to col 3 (1-based 4) and set a stop there via HTS (ESC H).
        t.advance(b"\x1b[4G"); // col 4 (1-based) = col 3 (0-based)
        t.advance(b"\x1bH"); // HTS at col 3
        // Home, then tab: should stop at the new custom stop (col 3), not col 8.
        t.advance(b"\x1b[1G\t");
        assert_eq!(t.cursor_position(), Some((0, 3)), "tab honours custom HTS stop");
    }

    #[test]
    fn tbc_clear_all_then_tab_goes_to_last_col() {
        let mut t = Terminal::new(2, 10);
        t.advance(b"\x1b[3g"); // TBC 3 — clear every stop
        t.advance(b"\x1b[1G\t"); // home, tab with no stops -> last column (9)
        assert_eq!(t.cursor_position(), Some((0, 9)), "no stops -> last col");
    }

    #[test]
    fn tbc_clear_current_stop() {
        let mut t = Terminal::new(2, 30);
        // Clear the default stop at col 8, then tab from home jumps to col 16.
        t.advance(b"\x1b[9G"); // col 9 (1-based) = col 8 (0-based), a default stop
        t.advance(b"\x1b[0g"); // TBC 0 — clear stop at cursor (col 8)
        t.advance(b"\x1b[1G\t");
        assert_eq!(t.cursor_position(), Some((0, 16)), "cleared col-8 stop skipped");
    }

    #[test]
    fn cht_forward_tabs_n() {
        let mut t = Terminal::new(2, 40);
        t.advance(b"\x1b[3I"); // CHT 3 — forward 3 tab stops from col 0 -> 8,16,24
        assert_eq!(t.cursor_position(), Some((0, 24)));
    }

    #[test]
    fn cbt_back_tabs_n() {
        let mut t = Terminal::new(2, 40);
        t.advance(b"\x1b[30G"); // col 30 (1-based) = col 29
        t.advance(b"\x1b[2Z"); // CBT 2 — back 2 stops: 24 then 16
        assert_eq!(t.cursor_position(), Some((0, 16)));
    }

    #[test]
    fn cbt_stops_at_column_zero() {
        let mut t = Terminal::new(2, 40);
        t.advance(b"\x1b[5G"); // col 4
        t.advance(b"\x1b[9Z"); // back far more stops than exist
        assert_eq!(t.cursor_position(), Some((0, 0)), "CBT clamps at col 0");
    }

    #[test]
    fn tab_stops_reset_on_ris() {
        let mut t = Terminal::new(2, 30);
        t.advance(b"\x1b[3g"); // clear all stops
        t.advance(b"\x1bc"); // RIS — restores default stops
        t.advance(b"\t");
        assert_eq!(t.cursor_position(), Some((0, 8)), "RIS restores default stops");
    }

    // ---- C14: focus reporting emit (core half) ----

    #[test]
    fn focus_report_silent_when_mode_off() {
        let mut t = Terminal::new(4, 20);
        t.focus_report(true);
        t.focus_report(false);
        assert!(
            t.take_pty_response().is_empty(),
            "no focus reports unless ?1004 is enabled"
        );
    }

    #[test]
    fn focus_report_emits_when_mode_on() {
        let mut t = Terminal::new(4, 20);
        t.advance(b"\x1b[?1004h"); // enable focus reporting
        assert!(t.focus_reporting());
        t.focus_report(true);
        assert_eq!(t.take_pty_response().as_slice(), b"\x1b[I", "focus-in emits CSI I");
        t.focus_report(false);
        assert_eq!(t.take_pty_response().as_slice(), b"\x1b[O", "focus-out emits CSI O");
    }

    #[test]
    fn focus_report_stops_after_mode_reset() {
        let mut t = Terminal::new(4, 20);
        t.advance(b"\x1b[?1004h");
        t.focus_report(true);
        let _ = t.take_pty_response();
        t.advance(b"\x1b[?1004l"); // disable again
        t.focus_report(true);
        assert!(t.take_pty_response().is_empty(), "disabling ?1004 silences reports");
    }

    // ---- C16: reflow / rewrap on resize ----

    #[test]
    fn reflow_narrowing_rewraps_without_losing_chars() {
        // A 12-char logical line on a 20-col grid (one physical row) re-wraps
        // onto a 10-col grid (two physical rows) without losing characters.
        let mut t = Terminal::new(4, 20);
        t.advance(b"abcdefghijkl"); // 12 chars, no wrap at 20 cols
        t.resize(4, 10);
        // Row 0 holds "abcdefghij", row 1 holds "kl".
        let row0: String = (0..10).map(|c| t.grid().cell(0, c).unwrap().c).collect();
        let row1: String = (0..2).map(|c| t.grid().cell(1, c).unwrap().c).collect();
        assert_eq!(row0, "abcdefghij");
        assert_eq!(row1, "kl");
    }

    #[test]
    fn reflow_widening_rejoins_a_wrapped_line() {
        // Print 12 chars into a 5-col grid: it soft-wraps across rows. Widening
        // to 20 cols must rejoin the whole logical line onto one row.
        let mut t = Terminal::new(6, 5);
        t.advance(b"abcdefghijkl"); // wraps: abcde/fghij/kl
        t.resize(6, 20);
        let joined: String = (0..12).map(|c| t.grid().cell(0, c).unwrap().c).collect();
        assert_eq!(joined, "abcdefghijkl", "wrapped line rejoined on widen");
    }

    #[test]
    fn reflow_never_merges_across_hard_newline() {
        // Two separate hard lines must stay separate across a reflow, even when
        // each is short enough that a naive join would merge them.
        let mut t = Terminal::new(6, 20);
        t.advance(b"foo\r\nbar");
        t.resize(6, 8);
        let row0: String = (0..3).map(|c| t.grid().cell(0, c).unwrap().c).collect();
        let row1: String = (0..3).map(|c| t.grid().cell(1, c).unwrap().c).collect();
        assert_eq!(row0, "foo");
        assert_eq!(row1, "bar", "hard newline preserved — not merged into foo");
    }

    #[test]
    fn reflow_roundtrip_preserves_text() {
        // Narrow then widen back: the text content must survive intact.
        let mut t = Terminal::new(6, 20);
        t.advance(b"the quick brown fox"); // 19 chars, fits one row at 20
        t.resize(6, 7); // narrow — forces wrap
        t.resize(6, 20); // widen back
        let joined: String = (0..19).map(|c| t.grid().cell(0, c).unwrap().c).collect();
        assert_eq!(joined, "the quick brown fox", "narrow→widen round-trips");
    }

    #[test]
    fn reflow_preserves_scrollback_lines() {
        // Lines pushed to scrollback survive a reflow (non-lossy preservation).
        let mut t = Terminal::with_scrollback(2, 8, 100);
        t.advance(b"L0\r\nL1\r\nL2\r\nL3"); // L0/L1 scroll into history
        let before = t.all_lines();
        assert!(before.iter().any(|l| l.starts_with("L0")));
        t.resize(2, 12);
        let after = t.all_lines();
        assert!(
            after.iter().any(|l| l.starts_with("L0")),
            "scrollback line L0 survives reflow"
        );
        assert!(after.iter().any(|l| l.starts_with("L3")));
    }

    #[test]
    fn reflow_alt_screen_uses_plain_resize() {
        // On the alt screen, resize must NOT reflow (the TUI redraws itself);
        // it must remain a no-panic plain resize.
        let mut t = Terminal::new(4, 20);
        t.advance(b"\x1b[?1049h");
        t.advance(b"ALT");
        t.resize(6, 10);
        assert!(t.alt_screen_active());
        assert_eq!(t.grid().rows(), 6);
        assert_eq!(t.grid().cols(), 10);
    }

    // ============================================================
    // VT correctness P2/P3 batch (C20/C22/C25/C26/C27/C28/C30/C33/C34)
    // ============================================================

    use crate::grid::UnderlineStyle;

    // ---- C20: styled underlines + underline color ----

    #[test]
    fn sgr_plain_underline_is_single() {
        let mut t = Terminal::new(2, 10);
        t.advance(b"\x1b[4mX");
        assert_eq!(
            t.grid().cell(0, 0).unwrap().flags.underline_style,
            UnderlineStyle::Single
        );
        assert!(t.grid().cell(0, 0).unwrap().flags.underline());
    }

    #[test]
    fn sgr_colon_styled_underlines() {
        let cases: &[(&[u8], UnderlineStyle)] = &[
            (b"\x1b[4:0mX", UnderlineStyle::None),
            (b"\x1b[4:1mX", UnderlineStyle::Single),
            (b"\x1b[4:2mX", UnderlineStyle::Double),
            (b"\x1b[4:3mX", UnderlineStyle::Curly),
            (b"\x1b[4:4mX", UnderlineStyle::Dotted),
            (b"\x1b[4:5mX", UnderlineStyle::Dashed),
        ];
        for (seq, style) in cases {
            let mut t = Terminal::new(2, 10);
            t.advance(seq);
            assert_eq!(
                t.grid().cell(0, 0).unwrap().flags.underline_style,
                *style,
                "style for {seq:?}"
            );
        }
    }

    #[test]
    fn sgr_double_underline_via_21() {
        let mut t = Terminal::new(2, 10);
        t.advance(b"\x1b[21mX");
        assert_eq!(
            t.grid().cell(0, 0).unwrap().flags.underline_style,
            UnderlineStyle::Double
        );
    }

    #[test]
    fn sgr_24_resets_underline() {
        let mut t = Terminal::new(2, 10);
        t.advance(b"\x1b[4:3m\x1b[24mX");
        assert_eq!(
            t.grid().cell(0, 0).unwrap().flags.underline_style,
            UnderlineStyle::None
        );
    }

    #[test]
    fn sgr_58_underline_color_indexed() {
        let mut t = Terminal::new(2, 10);
        t.advance(b"\x1b[4:3;58:5:9mX"); // curly + indexed underline color 9
        let cell = t.grid().cell(0, 0).unwrap();
        assert_eq!(cell.flags.underline_style, UnderlineStyle::Curly);
        assert_eq!(cell.underline_color, Some(Color::Indexed(9)));
    }

    #[test]
    fn sgr_58_underline_color_rgb_colon_empty_colorspace() {
        let mut t = Terminal::new(2, 10);
        // `58:2::255:0:0` — note the empty colorspace slot between 2 and r.
        t.advance(b"\x1b[58:2::255:0:0mX");
        assert_eq!(
            t.grid().cell(0, 0).unwrap().underline_color,
            Some(Color::Rgb(255, 0, 0))
        );
    }

    #[test]
    fn sgr_58_underline_color_rgb_semicolon_form() {
        let mut t = Terminal::new(2, 10);
        t.advance(b"\x1b[58;2;10;20;30mX");
        assert_eq!(
            t.grid().cell(0, 0).unwrap().underline_color,
            Some(Color::Rgb(10, 20, 30))
        );
    }

    #[test]
    fn sgr_59_resets_underline_color() {
        let mut t = Terminal::new(2, 10);
        t.advance(b"\x1b[58:5:9m\x1b[59mX");
        assert_eq!(t.grid().cell(0, 0).unwrap().underline_color, None);
    }

    #[test]
    fn sgr_extended_fg_color_still_works_after_refactor() {
        // Regression: the sgr() rewrite must not break 38;2 / 38;5.
        let mut t = Terminal::new(2, 10);
        t.advance(b"\x1b[38;5;200mA\x1b[38;2;1;2;3mB");
        assert_eq!(t.grid().cell(0, 0).unwrap().fg, Color::Indexed(200));
        assert_eq!(t.grid().cell(0, 1).unwrap().fg, Color::Rgb(1, 2, 3));
    }

    // ---- C22: REP (verify still green after refactor) ----

    #[test]
    fn rep_after_p2_changes() {
        let mut t = Terminal::new(2, 10);
        t.advance(b"q\x1b[2b"); // print q, repeat twice more
        let line: String = (0..3).map(|c| t.grid().cell(0, c).unwrap().c).collect();
        assert_eq!(line, "qqq");
    }

    // ---- C25: DECSCNM / IRM / DECOM ----

    #[test]
    fn decscnm_reverse_screen_flag() {
        let mut t = Terminal::new(2, 10);
        assert!(!t.reverse_screen());
        t.advance(b"\x1b[?5h");
        assert!(t.reverse_screen(), "?5h sets reverse-video screen");
        t.advance(b"\x1b[?5l");
        assert!(!t.reverse_screen());
    }

    #[test]
    fn irm_insert_mode_shifts_line_right() {
        let mut t = Terminal::new(2, 6);
        t.advance(b"abcd\x1b[H"); // fill, home
        t.advance(b"\x1b[4h"); // enable IRM
        t.advance(b"XY"); // insert at col 0: XYabcd -> XYabcd (d pushed off)
        assert!(t.insert_mode());
        let line: String = (0..6).map(|c| t.grid().cell(0, c).unwrap().c).collect();
        assert_eq!(line, "XYabcd");
    }

    #[test]
    fn irm_reset_returns_to_overwrite() {
        let mut t = Terminal::new(2, 6);
        t.advance(b"abcd\x1b[H\x1b[4h\x1b[4l"); // set then reset IRM
        assert!(!t.insert_mode());
        t.advance(b"X"); // overwrite, not insert
        let line: String = (0..4).map(|c| t.grid().cell(0, c).unwrap().c).collect();
        assert_eq!(line, "Xbcd");
    }

    #[test]
    fn decom_origin_mode_relative_addressing() {
        let mut t = Terminal::new(6, 10);
        t.advance(b"\x1b[2;4r"); // scroll region rows 2..4 (0-based 1..3)
        t.advance(b"\x1b[?6h"); // enable origin mode (homes to top margin)
        assert!(t.origin_mode());
        // CUP row 1 with origin mode -> absolute row = scroll_top (1).
        t.advance(b"\x1b[1;1H");
        assert_eq!(t.cursor_position(), Some((1, 0)), "row 1 maps to top margin");
        // CUP row 2 -> scroll_top + 1 = row 2.
        t.advance(b"\x1b[2;1H");
        assert_eq!(t.cursor_position(), Some((2, 0)));
        // Past the bottom margin clamps to scroll_bottom (3).
        t.advance(b"\x1b[99;1H");
        assert_eq!(t.cursor_position(), Some((3, 0)), "clamped to bottom margin");
    }

    #[test]
    fn decom_off_uses_absolute_addressing() {
        let mut t = Terminal::new(6, 10);
        t.advance(b"\x1b[2;4r"); // region set
        t.advance(b"\x1b[1;1H"); // origin mode OFF -> absolute row 0
        assert_eq!(t.cursor_position(), Some((0, 0)));
    }

    // ---- C26: OSC 9;4 progress ----

    #[test]
    fn osc9_4_progress_normal() {
        let mut t = Terminal::new(2, 10);
        t.advance(b"\x1b]9;4;1;42\x07");
        let p = t.take_progress();
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].state, ProgressState::Normal);
        assert_eq!(p[0].percent, 42);
        assert!(t.take_progress().is_empty(), "drained once");
    }

    #[test]
    fn osc9_4_progress_states() {
        let mut t = Terminal::new(2, 10);
        t.advance(b"\x1b]9;4;0;0\x07"); // remove
        t.advance(b"\x1b]9;4;2;99\x07"); // error
        t.advance(b"\x1b]9;4;3;50\x07"); // indeterminate (percent ignored)
        t.advance(b"\x1b]9;4;4;75\x07"); // warning
        let p = t.take_progress();
        assert_eq!(p[0].state, ProgressState::Remove);
        assert_eq!(p[1].state, ProgressState::Error);
        assert_eq!(p[1].percent, 99);
        assert_eq!(p[2].state, ProgressState::Indeterminate);
        assert_eq!(p[2].percent, 0, "indeterminate ignores percent");
        assert_eq!(p[3].state, ProgressState::Warning);
        assert_eq!(p[3].percent, 75);
    }

    #[test]
    fn osc9_4_clamps_percent() {
        let mut t = Terminal::new(2, 10);
        t.advance(b"\x1b]9;4;1;250\x07");
        assert_eq!(t.take_progress()[0].percent, 100, "percent clamps to 100");
    }

    #[test]
    fn osc9_plain_notification_not_progress() {
        // OSC 9 without the ;4 sub-code is still a notification.
        let mut t = Terminal::new(2, 10);
        t.advance(b"\x1b]9;Hello\x07");
        assert!(t.take_progress().is_empty());
        assert_eq!(t.take_notification().unwrap().body, "Hello");
    }

    // ---- C27 / C34: combining marks + variation selectors ----

    #[test]
    fn combining_mark_attaches_to_previous_cell() {
        let mut t = Terminal::new(2, 10);
        t.advance("e\u{0301}".as_bytes()); // e + combining acute
        let cell = t.grid().cell(0, 0).unwrap();
        assert_eq!(cell.c, 'e');
        assert_eq!(cell.grapheme(), "e\u{0301}");
        // The combining mark did NOT advance the cursor into col 1.
        assert_eq!(t.cursor_position(), Some((0, 1)));
        assert_eq!(t.grid().cell(0, 1).unwrap().c, ' ', "no own cell for mark");
    }

    #[test]
    fn multiple_combining_marks_stack() {
        let mut t = Terminal::new(2, 10);
        t.advance("a\u{0301}\u{0302}".as_bytes());
        assert_eq!(t.grid().cell(0, 0).unwrap().grapheme(), "a\u{0301}\u{0302}");
    }

    #[test]
    fn variation_selector_attaches_zero_width() {
        let mut t = Terminal::new(2, 10);
        // heart + VS16 (emoji presentation) — VS16 is zero-width, attaches.
        t.advance("\u{2764}\u{FE0F}".as_bytes());
        let cell = t.grid().cell(0, 0).unwrap();
        assert_eq!(cell.c, '\u{2764}');
        assert_eq!(cell.grapheme(), "\u{2764}\u{FE0F}");
        assert_eq!(t.cursor_position(), Some((0, 1)), "VS16 did not advance cursor");
    }

    #[test]
    fn combining_mark_attaches_to_wide_glyph_base() {
        let mut t = Terminal::new(2, 10);
        // Wide CJK glyph occupies cols 0+1; a following combining mark must
        // attach to the BASE (col 0), not the continuation spacer (col 1).
        t.advance("世\u{0301}".as_bytes());
        assert_eq!(t.grid().cell(0, 0).unwrap().grapheme(), "世\u{0301}");
        assert_eq!(t.cursor_position(), Some((0, 2)));
    }

    #[test]
    fn leading_combining_mark_lands_in_cell() {
        // A combining mark with no preceding cell (col 0) is given a cell so it
        // is not silently lost.
        let mut t = Terminal::new(2, 10);
        t.advance("\u{0301}".as_bytes());
        assert_eq!(t.cursor_position(), Some((0, 1)), "leading mark occupies a cell");
    }

    // ---- C28: OSC 133 C/D command marks ----

    #[test]
    fn osc133_command_output_start_mark() {
        let mut t = Terminal::new(4, 20);
        t.advance(b"\x1b]133;C\x07");
        let marks = t.command_marks();
        assert_eq!(marks.len(), 1);
        assert!(matches!(marks[0].kind, CommandMarkKind::OutputStart));
    }

    #[test]
    fn osc133_command_end_with_exit_code() {
        let mut t = Terminal::new(4, 20);
        t.advance(b"\x1b]133;D;0\x07"); // success
        t.advance(b"\x1b]133;D;127\x07"); // failure
        let marks = t.command_marks();
        assert_eq!(marks.len(), 2);
        assert!(matches!(
            marks[0].kind,
            CommandMarkKind::CommandEnd { exit_code: Some(0) }
        ));
        assert!(matches!(
            marks[1].kind,
            CommandMarkKind::CommandEnd {
                exit_code: Some(127)
            }
        ));
    }

    #[test]
    fn osc133_command_end_without_exit_code() {
        let mut t = Terminal::new(4, 20);
        t.advance(b"\x1b]133;D\x07");
        assert!(matches!(
            t.command_marks()[0].kind,
            CommandMarkKind::CommandEnd { exit_code: None }
        ));
    }

    #[test]
    fn osc133_cd_never_writes_pty_response() {
        let mut t = Terminal::new(4, 20);
        t.advance(b"\x1b]133;C\x07\x1b]133;D;0\x07");
        assert!(
            t.take_pty_response().is_empty(),
            "OSC 133 C/D stay capture-only (anti-CVE)"
        );
    }

    #[test]
    fn osc133_a_still_records_prompt_mark_not_command_mark() {
        let mut t = Terminal::new(4, 20);
        t.advance(b"\x1b]133;A\x07");
        assert_eq!(t.prompt_marks().len(), 1);
        assert_eq!(t.command_marks().len(), 0, "A is a prompt mark, not command");
    }

    // ---- C30: XTGETTCAP ----

    #[test]
    fn xtgettcap_replies_to_colors_capability() {
        let mut t = Terminal::new(4, 20);
        // "Co" hex = 436F. Query DCS + q 436F ST.
        t.advance(b"\x1bP+q436F\x1b\\");
        let resp = t.take_pty_response();
        // Valid reply form: DCS 1 + r 436F = <hex of "256"> ST.
        // "256" hex = 323536.
        assert_eq!(resp.as_slice(), b"\x1bP1+r436F=323536\x1b\\");
    }

    #[test]
    fn xtgettcap_unknown_capability_invalid_reply() {
        let mut t = Terminal::new(4, 20);
        // "ZZ" hex = 5A5A — not a capability we report.
        t.advance(b"\x1bP+q5A5A\x1b\\");
        let resp = t.take_pty_response();
        // Invalid form: DCS 0 + r 5A5A ST.
        assert_eq!(resp.as_slice(), b"\x1bP0+r5A5A\x1b\\");
    }

    #[test]
    fn xtgettcap_does_not_disturb_sixel() {
        // A plain DCS q (Sixel, no '+') must still go to the image path, not
        // XTGETTCAP — regression guard for the hook disambiguation.
        let mut t = Terminal::new(4, 20);
        t.advance(b"\x1bPq#0;2;100;0;0~\x1b\\");
        assert_eq!(t.images().len(), 1);
        assert!(t.take_pty_response().is_empty(), "sixel emits no XTGETTCAP reply");
    }

    // ---- C33: DECRQM ----

    #[test]
    fn decrqm_reports_set_private_mode() {
        let mut t = Terminal::new(4, 20);
        t.advance(b"\x1b[?2004h"); // enable bracketed paste
        t.advance(b"\x1b[?2004$p"); // DECRQM query
        // Reply: CSI ? 2004 ; 1 $ y  (1 = set).
        assert_eq!(t.take_pty_response().as_slice(), b"\x1b[?2004;1$y");
    }

    #[test]
    fn decrqm_reports_reset_private_mode() {
        let mut t = Terminal::new(4, 20);
        t.advance(b"\x1b[?2004$p"); // never enabled -> reset (2)
        assert_eq!(t.take_pty_response().as_slice(), b"\x1b[?2004;2$y");
    }

    #[test]
    fn decrqm_reports_unrecognised_mode_zero() {
        let mut t = Terminal::new(4, 20);
        t.advance(b"\x1b[?9999$p");
        assert_eq!(t.take_pty_response().as_slice(), b"\x1b[?9999;0$y");
    }

    #[test]
    fn decrqm_reports_ansi_irm() {
        let mut t = Terminal::new(4, 20);
        t.advance(b"\x1b[4h"); // IRM on
        t.advance(b"\x1b[4$p"); // ANSI DECRQM (no '?')
        assert_eq!(t.take_pty_response().as_slice(), b"\x1b[4;1$y");
    }

    #[test]
    fn p2p3_modes_reset_on_ris() {
        let mut t = Terminal::new(4, 20);
        t.advance(b"\x1b[?5h\x1b[4h\x1b[?6h"); // reverse + insert + origin
        t.advance(b"\x1bc"); // RIS
        assert!(!t.reverse_screen());
        assert!(!t.insert_mode());
        assert!(!t.origin_mode());
    }
}
