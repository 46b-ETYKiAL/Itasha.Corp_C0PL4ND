//! The xterm 256-color default palette.
//!
//! Self-contained palette construction split out of the main [`crate::term`]
//! module. Builds the protocol-default indexed palette that seeds OSC 4 query
//! replies and OSC 104 resets. Colors are [`super::osc::Rgb`] triples — the same
//! representation the rest of the terminal core uses.

use super::osc::Rgb;

/// Builds the standard xterm 256-color palette as `(r, g, b)` triples.
///
/// 0-15 are the canonical xterm ANSI 16; 16-231 are the 6×6×6 color cube;
/// 232-255 are the 24-step grayscale ramp. The palette seeds OSC 4 query
/// replies so a program detecting colors gets sensible values before the host
/// applies its own theme via the [`super::ColorSet`] drain. The host's theme is
/// the source of truth for rendering — this palette is only the protocol-default
/// baseline for query/reset.
pub(crate) fn build_default_palette() -> [Rgb; 256] {
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
