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
        // Wired Noir — the canon brand default (DECISION-2026-005), shared with
        // the SCR1B3 editor. Cool near-black hull, off-white text, one teal
        // accent. Mirrors assets/themes/wired-noir.toml so the hard-coded
        // fallback looks identical to the bundled default.
        Theme {
            name: "Wired Noir (builtin)".into(),
            author: "Itasha.Corp".into(),
            background: row("#070a0c"),
            foreground: row("#c8d6dc"),
            cursor: row("#34e0d0"),
            cursor_text: row("#070a0c"),
            selection_background: row("#163a40"),
            selection_foreground: row("#c8d6dc"),
            normal: AnsiRow {
                black: "#0e1417".into(),
                red: "#ff3b30".into(),
                green: "#6fb89a".into(),
                yellow: "#f2b33d".into(),
                blue: "#79a0b0".into(),
                magenta: "#9d8bbf".into(),
                cyan: "#34e0d0".into(),
                white: "#c8d6dc".into(),
            },
            bright: AnsiRow {
                black: "#5a6b73".into(),
                red: "#ff5c52".into(),
                green: "#8da88c".into(),
                yellow: "#c9a86a".into(),
                blue: "#a9c2cc".into(),
                magenta: "#b8a6d4".into(),
                cyan: "#5cebdb".into(),
                white: "#e6eef2".into(),
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
        assert_eq!(t.ansi(6), (0x34, 0xe0, 0xd0)); // cyan = WIRED-NOIR TEAL
    }
}
