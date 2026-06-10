//! BiDi (right-to-left) DISPLAY reordering for the egui terminal grid (F3-2).
//!
//! A terminal stores and processes cells in LOGICAL (memory) order. For
//! left-to-right scripts that is also the visual order, so the overwhelmingly
//! common case is a no-op. For right-to-left scripts (Arabic, Hebrew) the
//! logical order reads BACKWARDS on screen unless the row is reordered into
//! VISUAL order per the Unicode Bidirectional Algorithm (UAX #9) before paint.
//!
//! This module reorders ONE rendered row's foreground colour runs from logical
//! into visual order, preserving each glyph's colour through the reorder. It is
//! a pure, display-side projection over the already-built [`ColorRun`] vector —
//! it does NOT touch cell storage, input handling, or the logical grid snapshot.
//!
//! # Scope boundary (deliberate, P2-proportionate — NOT a TODO)
//!
//! This module implements ONLY per-row VISUAL REORDERING of rendered runs —
//! the load-bearing, user-visible behaviour that makes RTL text read in the
//! correct direction. The following are documented KNOWN LIMITATIONS and the
//! defined behaviour boundary of this feature, shared by most grid terminals
//! (foot, Alacritty punt entirely):
//!
//! * **Cursor motion / mouse-cell mapping stay in LOGICAL columns.** The caret
//!   and click-to-cell hit-testing address the logical grid, so on a reordered
//!   RTL row the visual caret column can differ from the logical one. Reordering
//!   is display-only; the grid the cursor walks is untouched.
//! * **Per-row base direction only.** The base paragraph direction is inferred
//!   per visible row (first strong character) rather than per logical paragraph;
//!   a terminal has no paragraph structure to span rows.
//! * **Per-cell background quads are not reordered** — they are painted in
//!   logical columns by the render path; the common RTL line (default
//!   background) is unaffected.
//!
//! These boundaries are the DEFINED behaviour of F3-2, not deferred work.

use super::pane_term::ColorRun;

/// Reorder one row's logical-order colour runs into VISUAL order using the
/// Unicode Bidirectional Algorithm, preserving each character's colour.
///
/// Returns `None` for the common no-RTL case (pure-LTR / ASCII rows), so the
/// caller keeps the original runs with ZERO cost — this is the fast path the
/// hard constraint requires (LTR-only content is byte-for-byte unchanged).
///
/// Returns `Some(runs)` only when the row actually contains right-to-left
/// content, where `runs` is the visually-reordered, colour-preserving run list.
///
/// The function is PURE: it reads the input runs and allocates a fresh output;
/// it mutates nothing. This makes it exhaustively unit-testable as the safety
/// net for the render integration.
pub fn reorder_runs_visual(runs: &[ColorRun]) -> Option<Vec<ColorRun>> {
    // Fast path 1: an all-ASCII row can never contain RTL script — skip the UBA
    // entirely (no string build, no BidiInfo). This is the dominant case for a
    // terminal and must cost nothing.
    if runs.iter().all(|(s, _)| s.bytes().all(|b| b.is_ascii())) {
        return None;
    }

    // Flatten the runs into a per-character (char, colour) sequence and the
    // concatenated logical line. `char_colors[i]` is the colour of the i-th
    // character in `line` (by char index).
    let mut line = String::new();
    let mut char_colors: Vec<(u8, u8, u8)> = Vec::new();
    for (text, color) in runs {
        for ch in text.chars() {
            line.push(ch);
            char_colors.push(*color);
        }
    }

    // Run the UBA over the single logical line. `ParagraphBidiInfo` treats the
    // whole string as one paragraph (correct for a terminal row — no embedded
    // newlines reach here). The base direction is auto-detected (first strong
    // char) by passing `None`.
    let info = unicode_bidi::ParagraphBidiInfo::new(&line, None);

    // Fast path 2: the UBA confirms the row has no RTL run — keep logical order.
    if !info.has_rtl() {
        return None;
    }

    // Map each char-start BYTE offset in `line` to its logical CHAR index, so a
    // visual run's byte range can recover the per-char colours captured above.
    let mut byte_to_ci = vec![0usize; line.len() + 1];
    for (ci, (b, _)) in line.char_indices().enumerate() {
        byte_to_ci[b] = ci;
    }

    // `visual_runs` returns the runs already in VISUAL (left-to-right paint)
    // order; characters WITHIN an RTL run remain in logical order and must be
    // reversed so the run paints right-to-left.
    let (levels, vis_runs) = info.visual_runs(0..line.len());

    // Re-emit characters in visual order, then coalesce same-colour neighbours
    // back into runs (so the render path keeps its run-based galley batching).
    let mut out_chars: Vec<(char, (u8, u8, u8))> = Vec::with_capacity(char_colors.len());
    for run in vis_runs {
        let rtl = levels[run.start].is_rtl();
        let mut seg: Vec<(char, (u8, u8, u8))> = line[run.clone()]
            .char_indices()
            .map(|(local_b, ch)| (ch, char_colors[byte_to_ci[run.start + local_b]]))
            .collect();
        if rtl {
            seg.reverse();
        }
        out_chars.extend(seg);
    }

    Some(coalesce(&out_chars))
}

/// Coalesce a per-character `(char, colour)` sequence into colour runs, merging
/// consecutive characters that share a colour. Inverse of the flatten step.
fn coalesce(chars: &[(char, (u8, u8, u8))]) -> Vec<ColorRun> {
    let mut runs: Vec<ColorRun> = Vec::new();
    let mut cur = String::new();
    let mut cur_color: Option<(u8, u8, u8)> = None;
    for (ch, color) in chars {
        if cur_color != Some(*color) {
            if let Some(pc) = cur_color.take() {
                runs.push((std::mem::take(&mut cur), pc));
            }
            cur_color = Some(*color);
        }
        cur.push(*ch);
    }
    if let Some(pc) = cur_color {
        runs.push((cur, pc));
    }
    runs
}

#[cfg(test)]
mod tests {
    use super::*;

    const RED: (u8, u8, u8) = (255, 0, 0);
    const GREEN: (u8, u8, u8) = (0, 255, 0);
    const BLUE: (u8, u8, u8) = (0, 0, 255);

    /// The full row text, concatenated across runs (visual order helper).
    fn text_of(runs: &[ColorRun]) -> String {
        runs.iter().map(|(s, _)| s.as_str()).collect()
    }

    /// Per-character (char, colour) pairs, for asserting colour preservation.
    fn chars_of(runs: &[ColorRun]) -> Vec<(char, (u8, u8, u8))> {
        runs.iter()
            .flat_map(|(s, c)| s.chars().map(move |ch| (ch, *c)))
            .collect()
    }

    #[test]
    fn ltr_ascii_row_is_unchanged_fast_path() {
        // HARD CONSTRAINT: LTR-only content returns None (caller keeps the
        // original runs byte-for-byte — zero cost).
        let row = vec![("hello world".to_string(), RED)];
        assert!(
            reorder_runs_visual(&row).is_none(),
            "a pure-ASCII LTR row must take the fast path and reorder nothing"
        );
    }

    #[test]
    fn empty_row_is_unchanged() {
        let row: Vec<ColorRun> = vec![];
        assert!(reorder_runs_visual(&row).is_none());
    }

    #[test]
    fn non_rtl_unicode_row_is_unchanged() {
        // CJK / accented Latin are non-ASCII but carry NO RTL — UBA fast path 2
        // (has_rtl() == false) must still return None.
        let row = vec![("café 日本語".to_string(), GREEN)];
        assert!(
            reorder_runs_visual(&row).is_none(),
            "non-ASCII but non-RTL content must not be reordered"
        );
    }

    #[test]
    fn pure_rtl_row_reverses_to_visual_order() {
        // Hebrew "shalom" = שלום. In logical (memory) order the characters are
        // ש ל ו ם; on screen (visual order, painted left-to-right) they read
        // ם ו ל ש — the reverse. UBA must produce that visual order.
        let logical = "שלום";
        let row = vec![(logical.to_string(), BLUE)];
        let visual = reorder_runs_visual(&row).expect("an RTL row reorders");
        let expected: String = logical.chars().rev().collect();
        assert_eq!(
            text_of(&visual),
            expected,
            "a pure-RTL row must be reversed into visual order"
        );
    }

    #[test]
    fn mixed_ltr_rtl_reorders_only_the_rtl_run() {
        // The classic BiDi case: "abc " + Hebrew "שלום". Base direction is LTR
        // (first strong char is Latin), so the visual line is:
        //   "abc " kept LTR, then the Hebrew reversed → "abc " + "םולש".
        let row = vec![("abc ".to_string(), RED), ("שלום".to_string(), BLUE)];
        let visual = reorder_runs_visual(&row).expect("a mixed row reorders");
        let hebrew_visual: String = "שלום".chars().rev().collect();
        let expected = format!("abc {hebrew_visual}");
        assert_eq!(
            text_of(&visual),
            expected,
            "the LTR run stays LTR; only the RTL run is reversed"
        );
    }

    #[test]
    fn colors_stay_attached_to_their_characters_through_reorder() {
        // Two differently-coloured Hebrew runs. After reordering, every glyph
        // must still carry the colour it had in logical order. We verify by
        // checking each (char, colour) pair survives (as a multiset) and that
        // the specific first/last logical glyphs keep their colours.
        let row = vec![("של".to_string(), RED), ("ום".to_string(), GREEN)];
        let visual = reorder_runs_visual(&row).expect("an RTL row reorders");

        let logical_pairs = chars_of(&row);
        let mut visual_pairs = chars_of(&visual);
        let mut logical_sorted = logical_pairs.clone();
        logical_sorted.sort();
        visual_pairs.sort();
        assert_eq!(
            visual_pairs, logical_sorted,
            "every (char, colour) pair must be preserved through the reorder"
        );

        // Concretely: 'ש' was RED and 'ם' was GREEN in logical order; they must
        // remain RED and GREEN respectively after reordering.
        let find = |ch: char| {
            visual
                .iter()
                .find_map(|(s, c)| s.contains(ch).then_some(*c))
        };
        assert_eq!(find('ש'), Some(RED), "'ש' keeps its RED colour");
        assert_eq!(find('ם'), Some(GREEN), "'ם' keeps its GREEN colour");
    }

    #[test]
    fn rtl_reorder_preserves_character_count() {
        // No glyph is dropped or duplicated by the reorder.
        let row = vec![("מרחבא world".to_string(), RED)];
        let visual = reorder_runs_visual(&row).expect("an RTL row reorders");
        assert_eq!(
            text_of(&visual).chars().count(),
            "מרחבא world".chars().count(),
            "reordering preserves the character count"
        );
    }
}
