//! In-terminal find overlay for the C0PL4ND egui shell.
//!
//! A small search box that floats over the focused pane: typing filters the
//! pane's visible/scrollback lines via the shared core matcher
//! ([`c0pl4nd_core::search::find`]), a live "N matches" / "M of N" count, and
//! Regex + Case toggle buttons. Navigation (Enter / F3 / Shift+F3 cycle, Esc
//! close) is handled in [`super::C0pl4ndApp::frame_tick`] BEFORE this renders,
//! so this widget only displays the current query + toggles + count and reports
//! the user's edits back through `&mut` state.
//!
//! Kept as a FREE function (mirroring [`super::settings::show`]) so it never
//! fights the `C0pl4ndApp` borrow — the host snapshots the immutable count into
//! a local and hands this only the mutable fields it edits.

use eframe::egui;

use super::theme::ChromeColors;

/// The mutable search state the overlay edits, threaded from
/// [`super::C0pl4ndApp`]. Borrowed disjointly from the rest of `self` so the
/// `TextEdit`'s `&mut query` does not collide with reads elsewhere on `self`.
pub struct SearchState<'a> {
    /// The live search query the `TextEdit` binds to.
    pub query: &'a mut String,
    /// Whether the query is treated as a regular expression.
    pub regex: &'a mut bool,
    /// Whether matching is case-SENSITIVE (the core option is
    /// `case_insensitive`, so this is its inverse — the UI speaks "Case").
    pub case_sensitive: &'a mut bool,
}

/// What [`show`] reports back to the host after a frame.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Outcome {
    /// The query OR a toggle changed this frame — the host should recompute the
    /// match set (and reset the selection into range) before the next frame.
    pub changed: bool,
}

/// Render the find overlay: a compact top-anchored box with an auto-focused
/// query field, the two option toggles (Regex, Case), and a live match-count
/// readout. Returns [`Outcome::changed`] when the query or a toggle changed so
/// the host recomputes matches.
///
/// `match_count` and `current` (1-based index of the active match, or 0 when
/// there are none) are immutable snapshots the host computed this frame — the
/// overlay only displays them. Phosphor THIN glyphs are NOT required here: the
/// toggles use plain text labels (`.Rx` / `Aa`) so the overlay never depends on
/// a glyph that the font atlas might not carry, and the only colours used come
/// from the active theme palette (no alarm-red).
pub fn show(
    ctx: &egui::Context,
    state: SearchState<'_>,
    match_count: usize,
    current: usize,
    colors: ChromeColors,
) -> Outcome {
    let mut changed = false;

    egui::Window::new("Find")
        .title_bar(false)
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::RIGHT_TOP, egui::vec2(-12.0, 52.0))
        .frame(
            egui::Frame::new()
                .fill(colors.panel)
                .stroke(egui::Stroke::new(1.0f32, colors.bezel))
                .inner_margin(8.0)
                .corner_radius(6.0),
        )
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                // The query field. Auto-focused every frame the overlay is open
                // so typed characters always populate the query, never the PTY
                // (the same focus discipline the command palette uses).
                let resp = ui.add(
                    egui::TextEdit::singleline(state.query)
                        .hint_text("Find in terminal…")
                        .desired_width(220.0),
                );
                if resp.changed() {
                    changed = true;
                }
                resp.request_focus();

                // Regex toggle. A toggled-on button reads "on" via the active
                // theme accent; off uses the bezel — no alarm-red.
                if toggle(ui, ".Rx", *state.regex, "Regex", colors).clicked() {
                    *state.regex = !*state.regex;
                    changed = true;
                }
                // Case-sensitivity toggle.
                if toggle(ui, "Aa", *state.case_sensitive, "Case sensitive", colors).clicked() {
                    *state.case_sensitive = !*state.case_sensitive;
                    changed = true;
                }

                // Live match readout: "M of N" when on a match, else "N matches"
                // / "no matches". Calm — an invalid regex simply yields 0 here.
                let label = match_count_label(match_count, current);
                ui.add_space(4.0);
                ui.colored_label(colors.fg, label);
            });
            ui.add_space(2.0);
            ui.weak("Enter / F3 next · Shift+F3 prev · Esc close");
        });

    Outcome { changed }
}

/// A small two-state toggle button. ON renders with the theme accent fill; OFF
/// with the bezel stroke. Returns the click response so the caller flips state.
fn toggle(
    ui: &mut egui::Ui,
    text: &str,
    on: bool,
    hover: &str,
    colors: ChromeColors,
) -> egui::Response {
    let fill = if on { colors.accent } else { colors.panel };
    let text_color = if on { colors.panel } else { colors.fg };
    ui.add(
        egui::Button::new(egui::RichText::new(text).color(text_color))
            .fill(fill)
            .stroke(egui::Stroke::new(1.0f32, colors.bezel))
            .min_size(egui::vec2(30.0, 0.0)),
    )
    .on_hover_text(hover)
}

/// The human-readable match-count label.
///
/// * 0 matches → `"no matches"`
/// * N matches, `current == 0` (none active yet) → `"N matches"`
/// * N matches, `current` in `1..=N` → `"M of N"`
///
/// Pure so it is unit-testable without a UI.
fn match_count_label(match_count: usize, current: usize) -> String {
    if match_count == 0 {
        "no matches".to_string()
    } else if current == 0 || current > match_count {
        format!("{match_count} matches")
    } else {
        format!("{current} of {match_count}")
    }
}

#[cfg(test)]
mod tests {
    use super::match_count_label;

    #[test]
    fn label_no_matches() {
        assert_eq!(match_count_label(0, 0), "no matches");
        assert_eq!(match_count_label(0, 3), "no matches");
    }

    #[test]
    fn label_count_only_when_none_active() {
        assert_eq!(match_count_label(5, 0), "5 matches");
    }

    #[test]
    fn label_m_of_n_when_active() {
        assert_eq!(match_count_label(5, 1), "1 of 5");
        assert_eq!(match_count_label(5, 5), "5 of 5");
    }

    #[test]
    fn label_clamps_out_of_range_current_to_count() {
        // A stale current beyond the count degrades to the plain count, never a
        // nonsensical "9 of 5".
        assert_eq!(match_count_label(5, 9), "5 matches");
    }
}
