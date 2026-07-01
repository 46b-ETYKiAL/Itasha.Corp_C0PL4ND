//! `C0pl4ndApp` in-terminal find overlay (Ctrl+Shift+F).
//!
//! The focused-pane grid-text search: line extraction, the core-matcher driver,
//! toggle/cycle state, the CELL-span conversions for the highlight + hyperlink
//! overlays, and the search window render. Grouped out of the C0pl4ndApp god-impl;
//! behaviour unchanged (the interaction tests drive these through the real loop).

use eframe::egui;

use super::{byte_to_col, hyperlink, search_ui, theme, CellSpan};

impl super::C0pl4ndApp {
    // ---- in-terminal find overlay (Ctrl+Shift+F) -------------------------
    //
    // The overlay searches the FOCUSED pane's visible/scrollback grid text via
    // the shared core matcher (`c0pl4nd_core::search::find`). It is opened with
    // Ctrl+Shift+F (handled in `frame_tick`), filters as you type, shows a live match
    // count, cycles matches with Enter / F3 / Shift+F3, and closes with Esc. The
    // Regex + Case toggles map onto `search::SearchOptions`. These methods are the
    // production logic `frame_tick` calls — the interaction tests drive them
    // THROUGH the real frame loop, not as a test-only mirror.

    /// The lines of the focused pane's grid text, one `String` per visual row —
    /// the slice handed to [`c0pl4nd_core::search::find`]. Empty when the focused
    /// pane has no live terminal (the matcher then yields no matches).
    pub(crate) fn focused_search_lines(&self) -> Vec<String> {
        // A seeded test corpus (headless find tests) takes precedence so the
        // search wiring can be asserted without a live PTY; otherwise the real
        // focused-pane grid text is searched.
        if let Some(corpus) = &self.search_test_corpus {
            return corpus.lines().map(str::to_string).collect();
        }
        self.focused_grid_text()
            .map(|t| t.lines().map(str::to_string).collect())
            .unwrap_or_default()
    }

    /// The current [`SearchOptions`](c0pl4nd_core::search::SearchOptions) derived
    /// from the two UI toggles. `case_sensitive` is the inverse of the core's
    /// `case_insensitive` field.
    fn search_options(&self) -> c0pl4nd_core::search::SearchOptions {
        c0pl4nd_core::search::SearchOptions {
            regex: self.search_regex,
            case_insensitive: !self.search_case_sensitive,
        }
    }

    /// Recompute `search_matches` for the current query + options over the
    /// focused pane's grid text, and clamp `search_sel` into range. Called on
    /// open, on every query/toggle change, and each frame the overlay is open
    /// (the live PTY grid scrolls, so the match set is not static). An invalid
    /// regex yields an empty set — surfaced calmly as "no matches", never a
    /// panic (the core matcher swallows the regex-compile error).
    pub(crate) fn recompute_search(&mut self) {
        let lines = self.focused_search_lines();
        self.search_matches =
            c0pl4nd_core::search::find(&lines, &self.search_query, self.search_options());
        if self.search_matches.is_empty() {
            self.search_sel = 0;
        } else if self.search_sel >= self.search_matches.len() {
            self.search_sel = self.search_matches.len() - 1;
        }
    }

    /// Toggle the find overlay. Opening it resets the selection to the first
    /// match and recomputes the match set for whatever is already on screen;
    /// closing it leaves the query intact (so reopening resumes the last search).
    pub(crate) fn toggle_search(&mut self) {
        self.search_open = !self.search_open;
        if self.search_open {
            self.search_sel = 0;
            self.recompute_search();
        }
    }

    /// Advance the selected match by `delta` (wrapping), a no-op when there are
    /// no matches. Enter / F3 step +1; Shift+F3 steps −1. Wrapping mirrors every
    /// real editor's find-next behaviour (the last match's "next" is the first).
    pub(crate) fn search_cycle(&mut self, delta: i64) {
        let n = self.search_matches.len();
        if n == 0 {
            self.search_sel = 0;
            return;
        }
        let n_i = n as i64;
        let cur = self.search_sel as i64;
        self.search_sel = (((cur + delta) % n_i + n_i) % n_i) as usize;
    }

    /// Whether the find overlay is currently open. Observation accessor for the
    /// interaction tests (asserts Ctrl+Shift+F toggled it through the real frame loop).
    #[allow(dead_code)]
    pub fn search_is_open(&self) -> bool {
        self.search_open
    }

    /// The number of matches the find overlay found this frame. Observation
    /// accessor for the interaction tests (asserts typing filters the grid).
    #[allow(dead_code)]
    pub fn search_match_count(&self) -> usize {
        self.search_matches.len()
    }

    /// The 0-based index of the currently-selected match. Observation accessor
    /// for the cycle tests (asserts F3 / Shift+F3 / Enter move the selection).
    #[allow(dead_code)]
    pub fn search_selected(&self) -> usize {
        self.search_sel
    }

    /// Whether the find query is currently treated as a regex. Observation
    /// accessor for the regex-toggle test.
    #[allow(dead_code)]
    pub fn search_regex_enabled(&self) -> bool {
        self.search_regex
    }

    /// Whether find matching is currently case-SENSITIVE. Observation accessor
    /// for the case-toggle test.
    #[allow(dead_code)]
    pub fn search_case_sensitive_enabled(&self) -> bool {
        self.search_case_sensitive
    }

    // ---- find-overlay test-support surface --------------------------------
    //
    // These let the headless interaction tests drive the find overlay against a
    // KNOWN corpus and flip the option toggles deterministically — the live PTY
    // grid is async + platform-dependent, so a CI box with no usable shell could
    // not otherwise exercise the matcher. They are `pub` (consumed by the
    // `#[path]`-included test binary) but operate ONLY on the test-corpus
    // override / option flags; they never touch the live PTY, so they are inert
    // in the shipping binary (which never calls them).

    /// Seed the find overlay's search corpus with a known multi-line string, so
    /// the headless tests can assert the matcher wiring without a live PTY. The
    /// shipping binary never calls this (`search_test_corpus` stays `None`).
    /// Recomputes the match set immediately if the overlay is already open.
    #[allow(dead_code)]
    pub fn test_seed_focused_grid(&mut self, corpus: &str) {
        self.search_test_corpus = Some(corpus.to_string());
        if self.search_open {
            self.recompute_search();
        }
    }

    /// Flip the regex option and recompute matches — the production effect of
    /// clicking the Regex toggle, exposed for the headless test (which cannot
    /// reliably click the overlay's flow buttons).
    #[allow(dead_code)]
    pub fn test_set_regex(&mut self, on: bool) {
        self.search_regex = on;
        if self.search_open {
            self.recompute_search();
        }
    }

    /// Flip the case-sensitivity option and recompute matches — the production
    /// effect of clicking the Case toggle, exposed for the headless test.
    #[allow(dead_code)]
    pub fn test_set_case_sensitive(&mut self, on: bool) {
        self.search_case_sensitive = on;
        if self.search_open {
            self.recompute_search();
        }
    }

    /// Feed raw bytes straight into the FOCUSED pane's terminal emulator,
    /// bypassing the PTY — so a headless test can build deterministic scrollback
    /// (e.g. many `\r\n`-terminated lines) without depending on a live shell.
    /// The shipping binary never calls this; production output always arrives via
    /// the PTY pump.
    #[allow(dead_code)]
    pub fn test_feed_focused(&mut self, bytes: &[u8]) {
        if let Some(term) = self.terms.get_mut(&self.focused_pane) {
            term.test_advance(bytes);
        }
    }

    /// The FOCUSED pane's current scroll-up offset (0 = following live output).
    /// Exposed so the scroll-to-edge chord test can assert the observable scroll
    /// position the keybinding produced.
    #[allow(dead_code)]
    pub fn test_focused_view_offset(&self) -> Option<usize> {
        self.terms.get(&self.focused_pane).map(|t| t.view_offset())
    }

    /// The FOCUSED pane's scrollback history length. Exposed so the scroll-edge
    /// chord test can confirm a non-empty history was built before asserting the
    /// chord scrolled into it.
    #[allow(dead_code)]
    pub fn test_focused_scrollback_len(&self) -> Option<usize> {
        self.terms
            .get(&self.focused_pane)
            .map(|t| t.scrollback_len())
    }

    /// The FOCUSED pane's whole-buffer copy text (the payload Ctrl+Shift+A / the
    /// "Copy all" menu item place on the clipboard). Exposed so the copy-all
    /// interaction test can assert the extracted content matches what was fed.
    #[allow(dead_code)]
    pub fn test_focused_buffer_text(&self) -> Option<String> {
        self.terms
            .get(&self.focused_pane)
            .and_then(|t| t.buffer_text())
    }

    /// Whether the FOCUSED pane's emulator has spawned and is live. Exposed so an
    /// interaction test can wait out the deferred first-frame PTY spawn before
    /// feeding it (an early `test_feed_focused` on a not-yet-spawned pane no-ops).
    #[allow(dead_code)]
    pub fn test_focused_alive(&self) -> bool {
        self.terms
            .get(&self.focused_pane)
            .is_some_and(|t| t.is_alive())
    }

    /// The current match set converted to CELL spans over the focused pane's
    /// grid text, ready for the highlight painter. Converts each match's BYTE
    /// span to character columns via [`byte_to_col`] against the matched line, so
    /// a multi-byte glyph before the match never offsets the highlight. A match
    /// whose `line` exceeds the visible rows (the grid scrolled since the set was
    /// computed) is dropped.
    pub(crate) fn cell_spans_for_search(&self, lines: &[String]) -> Vec<CellSpan> {
        if self.search_matches.is_empty() {
            return Vec::new();
        }
        self.search_matches
            .iter()
            .filter_map(|m| {
                let line = lines.get(m.line)?;
                Some(CellSpan {
                    line: m.line,
                    col_start: byte_to_col(line, m.start),
                    col_end: byte_to_col(line, m.end),
                })
            })
            .collect()
    }

    /// Every `http(s)://` URL in the FOCUSED pane's grid text, as `(CellSpan,
    /// url)` pairs ready for the Ctrl-hover underline and the Ctrl-click hit
    /// test. Built from [`Self::focused_search_lines`] (which honours the test
    /// corpus) via [`hyperlink::find_urls`], converting each URL's BYTE span to
    /// character columns with [`byte_to_col`] so a multi-byte glyph before the
    /// URL never offsets the underline. Computed once per frame before the
    /// disjoint-borrow render block (mirrors [`Self::cell_spans_for_search`]).
    pub(crate) fn cell_spans_for_hyperlinks(&self, lines: &[String]) -> Vec<(CellSpan, String)> {
        let mut out = Vec::new();
        for (row, line) in lines.iter().enumerate() {
            for span in hyperlink::find_urls(line) {
                out.push((
                    CellSpan {
                        line: row,
                        col_start: byte_to_col(line, span.start),
                        col_end: byte_to_col(line, span.end),
                    },
                    span.url,
                ));
            }
        }
        out
    }

    /// Render the find overlay (delegating to the [`search_ui`] free function so
    /// it never fights `self`'s borrow), then recompute the match set when the
    /// query or a toggle changed this frame. The `current` readout is the 1-based
    /// selection index when on a match, else 0.
    pub(crate) fn search_window(&mut self, ctx: &egui::Context) {
        let colors = theme::ChromeColors::from_theme(&self.theme);
        let match_count = self.search_matches.len();
        let current = if match_count == 0 {
            0
        } else {
            self.search_sel + 1
        };
        let outcome = {
            let state = search_ui::SearchState {
                query: &mut self.search_query,
                regex: &mut self.search_regex,
                case_sensitive: &mut self.search_case_sensitive,
            };
            search_ui::show(ctx, state, match_count, current, colors)
        };
        if outcome.changed {
            self.recompute_search();
        }
    }
}
