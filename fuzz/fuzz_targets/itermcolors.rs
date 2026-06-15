#![no_main]
//! Fuzz target for C0PL4ND's iTerm2 `.itermcolors` theme importer.
//!
//! An `.itermcolors` file is an Apple property-list (plist) XML document a user
//! imports to theme the terminal. It is fully **untrusted input**: it arrives
//! from the internet (iTerm2-Color-Schemes and similar repos) or is hand-edited.
//! The importer is a dependency-free linear scan over the `<key>`/`<real>` token
//! stream (`crates/core/src/theme/itermcolors.rs`) with hand-rolled `<dict>`
//! depth-balancing and byte-index arithmetic on attacker-controlled tag offsets
//! — exactly the shape that hides a slice/`find`/depth-counter edge. A malformed
//! or hostile document must never panic, hang, overflow, or OOB; a clean
//! `ThemeError` (or a partial theme with builtin fallbacks) is the only correct
//! outcome.
//!
//! This drives the real public entry point
//! `c0pl4nd_core::Theme::from_itermcolors`, which:
//!   * scans for `Ansi 0..15 Color` + `Foreground`/`Background`/`Cursor Color`
//!     slots, balancing nested `<dict>` tags,
//!   * parses each color dict's `Red/Green/Blue Component` `<real>` values,
//!   * falls back to `Theme::builtin_void` for absent slots,
//!   * errors only when NO recognisable color entry is found.
//!
//! On any document that yields a `Theme` it also runs `theme.validate()` (the
//! hex-color check) and reads back the resolved color strings, so an
//! inconsistency reachable only through the constructed theme is caught.

use libfuzzer_sys::fuzz_target;

use c0pl4nd_core::Theme;

fuzz_target!(|data: &[u8]| {
    // The plist parser operates on `&str` (it does `xml.find(...)` + UTF-8 slice
    // arithmetic). Feed valid UTF-8 so the fuzzer reaches the tag-balancing /
    // slice logic rather than the trivial non-UTF-8 rejection.
    if let Ok(xml) = std::str::from_utf8(data) {
        if let Ok(theme) = Theme::from_itermcolors(xml, "fuzz-theme") {
            // A theme was constructed from fuzzer-reached slots. Validate it (hex
            // check) and touch every resolved color string so any inconsistency
            // built from a hostile dict is surfaced.
            let _ = theme.validate();
            let _ = theme.background.len();
            let _ = theme.foreground.len();
            let _ = theme.cursor.len();
            for hex in [
                &theme.normal.black,
                &theme.normal.red,
                &theme.normal.green,
                &theme.normal.yellow,
                &theme.normal.blue,
                &theme.normal.magenta,
                &theme.normal.cyan,
                &theme.normal.white,
                &theme.bright.black,
                &theme.bright.red,
                &theme.bright.green,
                &theme.bright.yellow,
                &theme.bright.blue,
                &theme.bright.magenta,
                &theme.bright.cyan,
                &theme.bright.white,
            ] {
                let _ = hex.len();
            }
        }
    }

    // Also feed the raw bytes lossily coerced to UTF-8, so a single stray
    // invalid byte mid-document (the realistic "one corrupted byte" case) still
    // reaches the slice/balance arithmetic on an otherwise-structured plist.
    let lossy = String::from_utf8_lossy(data);
    let _ = Theme::from_itermcolors(&lossy, "fuzz-theme-lossy");
});
