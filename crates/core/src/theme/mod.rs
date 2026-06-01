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
        Ok(())
    }

    /// Resolve an ANSI index (0-15) to an `(r,g,b)` triple.
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

    #[test]
    fn builtin_theme_is_valid() {
        let t = Theme::builtin_void();
        assert!(t.validate().is_ok());
        assert_eq!(t.ansi(6), (0x00, 0xff, 0xc8)); // cyan = brand mint
        assert_eq!(t.ansi(4), (0x77, 0x00, 0xff)); // blue = brand purple #7700FF
        assert_eq!(t.ansi(2), (0x00, 0xff, 0x90)); // green = brand green #00FF90
    }
}
