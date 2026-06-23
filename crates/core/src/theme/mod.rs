//! Theme schema + loader. Themes are simple TOML data files (see
//! `assets/themes/*.toml`); the flagship default is `itasha-void`.

use serde::{Deserialize, Serialize};
use std::path::Path;

mod itermcolors;

#[derive(Debug, thiserror::Error)]
pub enum ThemeError {
    #[error("could not read theme {0}")]
    Io(String),
    #[error("theme parse error: {0}")]
    Parse(String),
    #[error("invalid hex color {0:?}")]
    BadHex(String),
}

/// The eight ANSI colors for one intensity row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnsiRow {
    pub black: String,
    pub red: String,
    pub green: String,
    pub yellow: String,
    pub blue: String,
    pub magenta: String,
    pub cyan: String,
    pub white: String,
}

/// A complete color scheme.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Theme {
    pub name: String,
    #[serde(default)]
    pub author: String,
    pub background: String,
    pub foreground: String,
    pub cursor: String,
    #[serde(default)]
    pub cursor_text: String,
    #[serde(default)]
    pub selection_background: String,
    #[serde(default)]
    pub selection_foreground: String,
    pub normal: AnsiRow,
    pub bright: AnsiRow,
}

/// Parse `#RRGGBB` into an `(r, g, b)` triple.
///
/// # Examples
///
/// ```
/// use c0pl4nd_core::theme::parse_hex;
///
/// assert_eq!(parse_hex("#ff0090").unwrap(), (255, 0, 144));
/// // The leading '#' is optional and surrounding whitespace is trimmed.
/// assert_eq!(parse_hex("  00ff90 ").unwrap(), (0, 255, 144));
/// // Wrong length or non-hex digits are rejected.
/// assert!(parse_hex("#fff").is_err());
/// assert!(parse_hex("#gggggg").is_err());
/// ```
pub fn parse_hex(s: &str) -> Result<(u8, u8, u8), ThemeError> {
    let h = s.trim().trim_start_matches('#');
    if h.len() != 6 {
        return Err(ThemeError::BadHex(s.to_string()));
    }
    let r = u8::from_str_radix(&h[0..2], 16).map_err(|_| ThemeError::BadHex(s.to_string()))?;
    let g = u8::from_str_radix(&h[2..4], 16).map_err(|_| ThemeError::BadHex(s.to_string()))?;
    let b = u8::from_str_radix(&h[4..6], 16).map_err(|_| ThemeError::BadHex(s.to_string()))?;
    Ok((r, g, b))
}

impl Theme {
    pub fn from_toml(src: &str) -> Result<Theme, ThemeError> {
        let t: Theme = toml::from_str(src).map_err(|e| ThemeError::Parse(e.to_string()))?;
        t.validate()?;
        Ok(t)
    }

    pub fn load_from(path: &Path) -> Result<Theme, ThemeError> {
        let src =
            std::fs::read_to_string(path).map_err(|e| ThemeError::Io(format!("{path:?}: {e}")))?;
        Theme::from_toml(&src)
    }

    /// Confirm every color field is a valid hex triple.
    pub fn validate(&self) -> Result<(), ThemeError> {
        for c in [
            &self.background,
            &self.foreground,
            &self.cursor,
            &self.normal.black,
            &self.normal.red,
            &self.normal.green,
            &self.normal.yellow,
            &self.normal.blue,
            &self.normal.magenta,
            &self.normal.cyan,
            &self.normal.white,
            &self.bright.black,
            &self.bright.red,
            &self.bright.green,
            &self.bright.yellow,
            &self.bright.blue,
            &self.bright.magenta,
            &self.bright.cyan,
            &self.bright.white,
        ] {
            parse_hex(c)?;
        }
        // The optional slots default to an empty string ("theme omits this slot",
        // consumers fall back to a brand colour) — that empty case stays valid.
        // But an explicitly-SET bad hex here should be rejected, not silently
        // ignored, so the user gets feedback that the value was wrong.
        for c in [
            &self.cursor_text,
            &self.selection_background,
            &self.selection_foreground,
        ] {
            if !c.trim().is_empty() {
                parse_hex(c)?;
            }
        }
        Ok(())
    }

    /// Resolve an ANSI index (0-15) to an `(r,g,b)` triple.
    ///
    /// # Examples
    ///
    /// ```
    /// use c0pl4nd_core::theme::{Theme, parse_hex};
    ///
    /// let t = Theme::builtin_void();
    /// // Index 0..8 read the `normal` row; 8..16 read the `bright` row.
    /// assert_eq!(t.ansi(0), parse_hex(&t.normal.black).unwrap());
    /// assert_eq!(t.ansi(9), parse_hex(&t.bright.red).unwrap());
    /// ```
    pub fn ansi(&self, index: u8) -> (u8, u8, u8) {
        let row = if index < 8 {
            &self.normal
        } else {
            &self.bright
        };
        let s = match index % 8 {
            0 => &row.black,
            1 => &row.red,
            2 => &row.green,
            3 => &row.yellow,
            4 => &row.blue,
            5 => &row.magenta,
            6 => &row.cyan,
            _ => &row.white,
        };
        parse_hex(s).unwrap_or((255, 255, 255))
    }

    /// Resolve a [`crate::grid::Color`] against this theme to a concrete
    /// `(r,g,b)` triple. `Color::Default` yields the supplied `default_rgb`
    /// (the caller's effective default fg or bg, which may itself be swapped
    /// under DECSCNM reverse-screen). This is the single color-resolution path
    /// shared by both the winit and egui renderers — neither re-derives it.
    pub fn resolve_color(
        &self,
        color: crate::grid::Color,
        default_rgb: (u8, u8, u8),
    ) -> (u8, u8, u8) {
        match color {
            crate::grid::Color::Default => default_rgb,
            crate::grid::Color::Indexed(i) => self.ansi(i),
            crate::grid::Color::Rgb(r, g, b) => (r, g, b),
        }
    }

    /// Resolve a cell's effective `(foreground, Option<background>)` RGB,
    /// applying SGR inverse/reverse video. The background is `None` when it
    /// should use the window default (so the renderer can skip painting a quad
    /// for the common case). For an inverse cell, the effective foreground is
    /// the cell's background and vice-versa — matching every mainstream terminal
    /// (selections, `\e[7m`, cursor-on-cell all rely on this). `default_fg` /
    /// `default_bg` are the effective defaults (already swapped under DECSCNM).
    #[allow(clippy::type_complexity)]
    pub fn cell_colors(
        &self,
        cell: &crate::grid::Cell,
        default_fg: (u8, u8, u8),
        default_bg: (u8, u8, u8),
    ) -> ((u8, u8, u8), Option<(u8, u8, u8)>) {
        let fg = self.resolve_color(cell.fg, default_fg);
        let bg = match cell.bg {
            crate::grid::Color::Default => None,
            other => Some(self.resolve_color(other, default_bg)),
        };
        if cell.flags.inverse {
            let eff_bg = bg.unwrap_or(default_bg);
            (eff_bg, Some(fg))
        } else {
            (fg, bg)
        }
    }

    /// Imports an iTerm2 `.itermcolors` plist XML document into a [`Theme`].
    ///
    /// Maps `Ansi 0..15 Color` into the [`AnsiRow`] normal (0-7) and bright
    /// (8-15) rows, and `Foreground`/`Background`/`Cursor Color` into the
    /// dynamic colors. Missing slots fall back to [`Theme::builtin_void`].
    /// Returns [`ThemeError::Parse`] if the document carries no recognisable
    /// color entries. `name` becomes the resulting theme's name.
    pub fn from_itermcolors(xml: &str, name: &str) -> Result<Theme, ThemeError> {
        itermcolors::from_itermcolors(xml, name)
    }

    /// A hard-coded fallback used only when no theme file can be loaded —
    /// keeps the terminal usable even if the themes dir is missing.
    ///
    /// # Examples
    ///
    /// ```
    /// use c0pl4nd_core::theme::Theme;
    ///
    /// // The builtin is always self-consistent (every colour parses, etc.).
    /// let t = Theme::builtin_void();
    /// assert!(t.validate().is_ok());
    /// assert!(!t.name.is_empty());
    /// ```
    pub fn builtin_void() -> Theme {
        let row = |k: &str| k.to_string();
        // Itasha.Corp — the house brand default, shared by every Itasha.Corp
        // app. Brand primaries: electric purple #7700FF + spring green #00FF90.
        // Mirrors assets/themes/itasha-corp.toml so the hard-coded fallback
        // looks identical to the bundled default.
        Theme {
            name: "Itasha.Corp (builtin)".into(),
            author: "Itasha.Corp".into(),
            background: row("#121212"),
            foreground: row("#e8e6f0"),
            cursor: row("#00ff90"),
            cursor_text: row("#121212"),
            selection_background: row("#33106b"),
            selection_foreground: row("#e8e6f0"),
            normal: AnsiRow {
                black: "#1c1c1c".into(),
                red: "#ff3b5c".into(),
                green: "#00ff90".into(),
                yellow: "#ffc44d".into(),
                blue: "#7700ff".into(),
                magenta: "#b44dff".into(),
                cyan: "#00ffc8".into(),
                white: "#e8e6f0".into(),
            },
            bright: AnsiRow {
                black: "#4a4366".into(),
                red: "#ff6f88".into(),
                green: "#5cffb4".into(),
                yellow: "#ffd57a".into(),
                blue: "#9a4dff".into(),
                magenta: "#cf8aff".into(),
                cyan: "#5cffda".into(),
                white: "#ffffff".into(),
            },
        }
    }

    /// Themes COMPILED INTO the binary, keyed by their config name (the file
    /// stem under `assets/themes/`). The terminal-theme loader resolves these so
    /// theme selection ALWAYS works regardless of the process's CWD or whether an
    /// `assets/themes/` directory ships next to the installed binary. The prior
    /// file-only loader silently fell back to `builtin_void` whenever the file
    /// could not be found (i.e. in every installed launch), which the user
    /// experienced as "the theme doesn't change".
    pub const EMBEDDED_THEMES: &'static [(&'static str, &'static str)] = &[
        (
            "itasha-corp",
            include_str!("../../../../assets/themes/itasha-corp.toml"),
        ),
        (
            "itasha-void",
            include_str!("../../../../assets/themes/itasha-void.toml"),
        ),
        (
            "itasha-void-high-contrast",
            include_str!("../../../../assets/themes/itasha-void-high-contrast.toml"),
        ),
        (
            "ghost-paper",
            include_str!("../../../../assets/themes/ghost-paper.toml"),
        ),
        (
            "wired-noir",
            include_str!("../../../../assets/themes/wired-noir.toml"),
        ),
        (
            "wired-colorblind",
            include_str!("../../../../assets/themes/wired-colorblind.toml"),
        ),
        // Ported from the SCR1B3 editor for a cohesive Itasha.Corp product
        // family (calm-canon line).
        (
            "phosphor-amber",
            include_str!("../../../../assets/themes/phosphor-amber.toml"),
        ),
        (
            "lain-mauve",
            include_str!("../../../../assets/themes/lain-mauve.toml"),
        ),
        (
            "a11y-high-contrast",
            include_str!("../../../../assets/themes/a11y-high-contrast.toml"),
        ),
        // itasha-neon family (brand-signature line).
        (
            "itasha-neon",
            include_str!("../../../../assets/themes/itasha-neon.toml"),
        ),
        (
            "itasha-neon-pastel",
            include_str!("../../../../assets/themes/itasha-neon-pastel.toml"),
        ),
        (
            "itasha-neon-soft",
            include_str!("../../../../assets/themes/itasha-neon-soft.toml"),
        ),
        (
            "itasha-neon-night",
            include_str!("../../../../assets/themes/itasha-neon-night.toml"),
        ),
        (
            "itasha-neon-dawn",
            include_str!("../../../../assets/themes/itasha-neon-dawn.toml"),
        ),
        (
            "itasha-neon-aurora",
            include_str!("../../../../assets/themes/itasha-neon-aurora.toml"),
        ),
        // Heritage-alt influence palettes.
        (
            "geocities-bbs",
            include_str!("../../../../assets/themes/geocities-bbs.toml"),
        ),
        (
            "lain-wired",
            include_str!("../../../../assets/themes/lain-wired.toml"),
        ),
        (
            "kusanagi-dive",
            include_str!("../../../../assets/themes/kusanagi-dive.toml"),
        ),
        (
            "akira-redshift",
            include_str!("../../../../assets/themes/akira-redshift.toml"),
        ),
        (
            "atompunk-sodium",
            include_str!("../../../../assets/themes/atompunk-sodium.toml"),
        ),
        (
            "terminal-lock",
            include_str!("../../../../assets/themes/terminal-lock.toml"),
        ),
        (
            "mecha-armour",
            include_str!("../../../../assets/themes/mecha-armour.toml"),
        ),
        (
            "shutoko-night",
            include_str!("../../../../assets/themes/shutoko-night.toml"),
        ),
    ];

    /// Resolve a compiled-in theme by its config name. Returns `None` for an
    /// unknown name or a theme that fails to parse/validate. This is the
    /// CWD-independent resolution path that makes theme selection work in the
    /// installed app (see [`Theme::EMBEDDED_THEMES`]).
    pub fn builtin_named(name: &str) -> Option<Theme> {
        Self::EMBEDDED_THEMES
            .iter()
            .find(|(n, _)| *n == name)
            .and_then(|(_, src)| Theme::from_toml(src).ok())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_hex_works() {
        assert_eq!(parse_hex("#08060d").unwrap(), (8, 6, 13));
        assert_eq!(parse_hex("00e5ff").unwrap(), (0, 229, 255));
        assert!(parse_hex("#12345").is_err());
        assert!(parse_hex("#zzzzzz").is_err());
    }

    /// The compiled-in theme set is the CWD-independent resolution path that
    /// fixes "the theme doesn't change" in the installed app: every advertised
    /// name must parse+validate, distinct themes must differ, unknown → None.
    #[test]
    fn embedded_themes_resolve_and_differ() {
        for (name, _) in Theme::EMBEDDED_THEMES {
            assert!(
                Theme::builtin_named(name).is_some(),
                "embedded theme {name:?} must parse + validate"
            );
        }
        let noir = Theme::builtin_named("wired-noir").expect("wired-noir embedded");
        let paper = Theme::builtin_named("ghost-paper").expect("ghost-paper embedded");
        assert_ne!(
            noir.background, paper.background,
            "distinct embedded themes must have distinct backgrounds"
        );
        assert!(Theme::builtin_named("no-such-theme").is_none());
    }

    #[test]
    fn builtin_theme_is_valid() {
        let t = Theme::builtin_void();
        assert!(t.validate().is_ok());
        assert_eq!(t.ansi(6), (0x00, 0xff, 0xc8)); // cyan = brand mint
        assert_eq!(t.ansi(4), (0x77, 0x00, 0xff)); // blue = brand purple #7700FF
        assert_eq!(t.ansi(2), (0x00, 0xff, 0x90)); // green = brand green #00FF90
    }

    #[test]
    fn validate_checks_optional_slots_when_set_but_allows_empty() {
        // Empty optional slots (the "theme omits this slot" default) stay valid.
        let t = Theme::builtin_void();
        assert!(t.validate().is_ok());

        // An explicitly-set BAD hex in an optional slot is now rejected (was
        // silently ignored — the user got no feedback the value was wrong).
        let mut bad = Theme::builtin_void();
        bad.selection_background = "not-a-color".to_string();
        assert!(matches!(bad.validate(), Err(ThemeError::BadHex(_))));

        // A valid hex in an optional slot passes.
        let mut good = Theme::builtin_void();
        good.cursor_text = "#abcdef".to_string();
        good.selection_foreground = "#012345".to_string();
        assert!(good.validate().is_ok());
    }

    // --- additional edge-coverage -----------------------------------------

    const MINIMAL_TOML: &str = r##"
name = "mini"
background = "#000000"
foreground = "#ffffff"
cursor = "#00ff00"
[normal]
black = "#101010"
red = "#ff0000"
green = "#00ff00"
yellow = "#ffff00"
blue = "#0000ff"
magenta = "#ff00ff"
cyan = "#00ffff"
white = "#cccccc"
[bright]
black = "#202020"
red = "#ff4040"
green = "#40ff40"
yellow = "#ffff40"
blue = "#4040ff"
magenta = "#ff40ff"
cyan = "#40ffff"
white = "#ffffff"
"##;

    #[test]
    fn from_toml_parses_and_defaults_optional_fields() {
        let t = Theme::from_toml(MINIMAL_TOML).expect("parse minimal theme");
        assert_eq!(t.name, "mini");
        // Omitted optional fields default to empty strings.
        assert_eq!(t.author, "");
        assert_eq!(t.cursor_text, "");
        assert_eq!(t.selection_background, "");
        assert_eq!(t.selection_foreground, "");
        assert_eq!(t.normal.red, "#ff0000");
        assert_eq!(t.bright.white, "#ffffff");
    }

    #[test]
    fn from_toml_rejects_malformed_toml() {
        let err = Theme::from_toml("this is = = not valid toml [[[").unwrap_err();
        assert!(matches!(err, ThemeError::Parse(_)), "got {err:?}");
    }

    #[test]
    fn from_toml_rejects_bad_hex_via_validate() {
        // Parses as TOML but a color is not a hex triple → BadHex via validate().
        let bad = MINIMAL_TOML.replace("#ff0000", "not-a-hex");
        let err = Theme::from_toml(&bad).unwrap_err();
        assert!(matches!(err, ThemeError::BadHex(_)), "got {err:?}");
    }

    #[test]
    fn load_from_reads_disk_and_round_trips() {
        let tmp = std::env::temp_dir().join(format!("c0pl4nd-theme-{}.toml", std::process::id()));
        std::fs::write(&tmp, MINIMAL_TOML).unwrap();
        let t = Theme::load_from(&tmp).expect("load_from disk");
        assert_eq!(t.name, "mini");
        assert_eq!(t.ansi(1), (0xff, 0, 0)); // normal red
        let _ = std::fs::remove_file(&tmp);
    }

    #[test]
    fn load_from_missing_file_is_io_error() {
        let missing = std::env::temp_dir().join("c0pl4nd-theme-absent-xyzzy.toml");
        let _ = std::fs::remove_file(&missing);
        let err = Theme::load_from(&missing).unwrap_err();
        assert!(matches!(err, ThemeError::Io(_)), "got {err:?}");
    }

    #[test]
    fn theme_error_display_variants() {
        assert!(ThemeError::Io("x".into())
            .to_string()
            .contains("could not read theme"));
        assert!(ThemeError::Parse("y".into())
            .to_string()
            .contains("theme parse error"));
        assert!(ThemeError::BadHex("zz".into())
            .to_string()
            .contains("invalid hex color"));
    }

    #[test]
    fn ansi_resolves_every_index_to_the_right_slot() {
        let t = Theme::builtin_void();
        // normal row (0-7) maps black..white in order.
        assert_eq!(t.ansi(0), parse_hex(&t.normal.black).unwrap());
        assert_eq!(t.ansi(1), parse_hex(&t.normal.red).unwrap());
        assert_eq!(t.ansi(2), parse_hex(&t.normal.green).unwrap());
        assert_eq!(t.ansi(3), parse_hex(&t.normal.yellow).unwrap());
        assert_eq!(t.ansi(4), parse_hex(&t.normal.blue).unwrap());
        assert_eq!(t.ansi(5), parse_hex(&t.normal.magenta).unwrap());
        assert_eq!(t.ansi(6), parse_hex(&t.normal.cyan).unwrap());
        assert_eq!(t.ansi(7), parse_hex(&t.normal.white).unwrap());
        // bright row (8-15) maps the bright slots.
        assert_eq!(t.ansi(8), parse_hex(&t.bright.black).unwrap());
        assert_eq!(t.ansi(9), parse_hex(&t.bright.red).unwrap());
        assert_eq!(t.ansi(10), parse_hex(&t.bright.green).unwrap());
        assert_eq!(t.ansi(11), parse_hex(&t.bright.yellow).unwrap());
        assert_eq!(t.ansi(12), parse_hex(&t.bright.blue).unwrap());
        assert_eq!(t.ansi(13), parse_hex(&t.bright.magenta).unwrap());
        assert_eq!(t.ansi(14), parse_hex(&t.bright.cyan).unwrap());
        assert_eq!(t.ansi(15), parse_hex(&t.bright.white).unwrap());
    }

    #[test]
    fn ansi_index_above_15_wraps_via_modulo() {
        let t = Theme::builtin_void();
        // index 16 → bright row (>=8), 16 % 8 == 0 → bright.black.
        assert_eq!(t.ansi(16), parse_hex(&t.bright.black).unwrap());
    }

    #[test]
    fn ansi_falls_back_to_white_on_bad_hex() {
        // A theme with a non-hex ANSI slot resolves that index to the
        // (255,255,255) fallback rather than panicking.
        let mut t = Theme::builtin_void();
        t.normal.red = "garbage".into();
        assert_eq!(t.ansi(1), (255, 255, 255), "bad hex → white fallback");
    }

    #[test]
    fn resolve_color_handles_all_three_variants() {
        use crate::grid::Color;
        let t = Theme::builtin_void();
        let dflt = (1, 2, 3);
        // Default → the supplied default rgb.
        assert_eq!(t.resolve_color(Color::Default, dflt), dflt);
        // Indexed → the ANSI table.
        assert_eq!(t.resolve_color(Color::Indexed(2), dflt), t.ansi(2));
        // Rgb → passed through verbatim.
        assert_eq!(t.resolve_color(Color::Rgb(9, 8, 7), dflt), (9, 8, 7));
    }

    #[test]
    fn cell_colors_default_bg_yields_none() {
        use crate::grid::{Cell, Color};
        let t = Theme::builtin_void();
        let cell = Cell {
            fg: Color::Rgb(10, 20, 30),
            bg: Color::Default,
            ..Cell::default()
        };
        let (fg, bg) = t.cell_colors(&cell, (0, 0, 0), (99, 99, 99));
        assert_eq!(fg, (10, 20, 30));
        assert_eq!(bg, None, "Default bg → None so the renderer skips a quad");
    }

    #[test]
    fn cell_colors_explicit_bg_is_some() {
        use crate::grid::{Cell, Color};
        let t = Theme::builtin_void();
        let cell = Cell {
            fg: Color::Rgb(1, 2, 3),
            bg: Color::Rgb(4, 5, 6),
            ..Cell::default()
        };
        let (fg, bg) = t.cell_colors(&cell, (0, 0, 0), (0, 0, 0));
        assert_eq!(fg, (1, 2, 3));
        assert_eq!(bg, Some((4, 5, 6)));
    }

    #[test]
    fn cell_colors_inverse_swaps_fg_and_bg() {
        use crate::grid::{Cell, CellFlags, Color};
        let t = Theme::builtin_void();
        // Inverse cell with explicit bg: effective fg becomes the bg, effective
        // bg becomes the fg.
        let cell = Cell {
            fg: Color::Rgb(11, 22, 33),
            bg: Color::Rgb(44, 55, 66),
            flags: CellFlags {
                inverse: true,
                ..CellFlags::empty()
            },
            ..Cell::default()
        };
        let (fg, bg) = t.cell_colors(&cell, (0, 0, 0), (7, 7, 7));
        assert_eq!(fg, (44, 55, 66), "inverse fg = the cell's bg");
        assert_eq!(bg, Some((11, 22, 33)), "inverse bg = the cell's fg");
    }

    #[test]
    fn cell_colors_inverse_with_default_bg_uses_default_bg_as_fg() {
        use crate::grid::{Cell, CellFlags, Color};
        let t = Theme::builtin_void();
        // Inverse cell whose bg is Default: the effective foreground falls back to
        // default_bg (the `bg.unwrap_or(default_bg)` branch).
        let cell = Cell {
            fg: Color::Rgb(200, 100, 50),
            bg: Color::Default,
            flags: CellFlags {
                inverse: true,
                ..CellFlags::empty()
            },
            ..Cell::default()
        };
        let (fg, bg) = t.cell_colors(&cell, (1, 1, 1), (9, 8, 7));
        assert_eq!(fg, (9, 8, 7), "inverse fg falls back to default_bg");
        assert_eq!(bg, Some((200, 100, 50)), "inverse bg = the cell's fg");
    }

    #[test]
    fn from_itermcolors_delegates_to_importer() {
        // Smoke the public Theme::from_itermcolors delegation path (the importer
        // itself is unit-tested in the submodule).
        let xml = r#"<plist><dict>
            <key>Ansi 1 Color</key>
            <dict>
                <key>Red Component</key><real>1.0</real>
                <key>Green Component</key><real>0.0</real>
                <key>Blue Component</key><real>0.0</real>
            </dict>
        </dict></plist>"#;
        let t = Theme::from_itermcolors(xml, "Imported").expect("import");
        assert_eq!(t.name, "Imported");
        assert_eq!(t.normal.red, "#ff0000");
    }
}
