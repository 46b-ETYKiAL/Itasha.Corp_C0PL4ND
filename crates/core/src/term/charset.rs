//! G0/G1 charset designation and DEC Special Graphics translation.
//!
//! Self-contained charset helpers split out of the main [`crate::term`] module
//! so the `Perform` impl stays focused on dispatch. Holds the [`Charset`]
//! designation enum, the canonical VT100 line-drawing table ([`dec_line_draw`]),
//! and the variation-selector predicate ([`is_variation_selector`]). No terminal
//! state is referenced here — these are pure functions over a single `char`.

/// A G0/G1 charset designation. Only the two sets a real shell exercises are
/// modelled: plain ASCII and the DEC Special Graphics (line-drawing) set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum Charset {
    /// US-ASCII (`ESC ( B`) — the default.
    #[default]
    Ascii,
    /// DEC Special Graphics / line drawing (`ESC ( 0`).
    DecLineDrawing,
}

/// Map a printable byte (`0x60..=0x7e`) from the DEC Special Graphics set to its
/// Unicode box-drawing equivalent. Characters outside that range pass through
/// unchanged. This is the canonical VT100 line-drawing table.
pub(crate) fn dec_line_draw(c: char) -> char {
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
pub(crate) fn is_variation_selector(c: char) -> bool {
    matches!(c, '\u{FE0E}' | '\u{FE0F}')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn charset_defaults_to_ascii() {
        assert_eq!(Charset::default(), Charset::Ascii);
    }

    #[test]
    fn dec_line_draw_maps_every_special_graphics_glyph() {
        // The complete canonical VT100 Special Graphics table (`0x60..=0x7e`).
        // Asserting every arm pins the box-drawing output a real `tput smacs`
        // session relies on, and covers the whole match.
        let table: &[(char, char)] = &[
            ('`', '\u{25c6}'),
            ('a', '\u{2592}'),
            ('b', '\u{2409}'),
            ('c', '\u{240c}'),
            ('d', '\u{240d}'),
            ('e', '\u{240a}'),
            ('f', '\u{00b0}'),
            ('g', '\u{00b1}'),
            ('h', '\u{2424}'),
            ('i', '\u{240b}'),
            ('j', '\u{2518}'),
            ('k', '\u{2510}'),
            ('l', '\u{250c}'),
            ('m', '\u{2514}'),
            ('n', '\u{253c}'),
            ('o', '\u{23ba}'),
            ('p', '\u{23bb}'),
            ('q', '\u{2500}'),
            ('r', '\u{23bc}'),
            ('s', '\u{23bd}'),
            ('t', '\u{251c}'),
            ('u', '\u{2524}'),
            ('v', '\u{2534}'),
            ('w', '\u{252c}'),
            ('x', '\u{2502}'),
            ('y', '\u{2264}'),
            ('z', '\u{2265}'),
            ('{', '\u{03c0}'),
            ('|', '\u{2260}'),
            ('}', '\u{00a3}'),
            ('~', '\u{00b7}'),
        ];
        for (input, expected) in table {
            assert_eq!(
                dec_line_draw(*input),
                *expected,
                "DEC line-draw for {input:?} wrong"
            );
        }
    }

    #[test]
    fn dec_line_draw_passes_non_graphics_chars_through_unchanged() {
        for c in ['A', 'Z', '0', '9', ' ', '\u{65e5}', '_', '^'] {
            assert_eq!(dec_line_draw(c), c, "{c:?} must pass through unchanged");
        }
    }

    #[test]
    fn variation_selectors_are_recognised() {
        assert!(is_variation_selector('\u{FE0E}'), "VS15 text presentation");
        assert!(is_variation_selector('\u{FE0F}'), "VS16 emoji presentation");
        for c in ['a', '\u{FE0D}', '\u{FE10}', '\u{200D}', '日'] {
            assert!(
                !is_variation_selector(c),
                "{c:?} is not a variation selector"
            );
        }
    }
}
