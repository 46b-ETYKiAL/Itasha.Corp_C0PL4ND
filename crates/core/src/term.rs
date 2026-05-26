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
        }
    }

    fn newline(&mut self) {
        if self.row + 1 >= self.grid.rows() {
            let dropped = self.grid.scroll_up_returning();
            if self.max_scrollback > 0 {
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
            self.col = 0;
            self.newline();
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
        _intermediates: &[u8],
        _ignore: bool,
        action: char,
    ) {
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
}
