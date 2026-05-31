//! iTerm2 `.itermcolors` import.
//!
//! `.itermcolors` files are Apple property-list (plist) XML documents whose
//! root `<dict>` maps color-slot names to nested `<dict>`s carrying sRGB
//! component values in `[0.0, 1.0]`. The slot names this importer understands:
//!
//! - `Ansi 0 Color` .. `Ansi 15 Color` — the 16 ANSI palette entries
//!   (0-7 -> [`AnsiRow`] normal, 8-15 -> bright).
//! - `Foreground Color`, `Background Color`, `Cursor Color` — the dynamic
//!   colors stored on [`Theme`].
//!
//! Each color `<dict>` contains `Red Component`, `Green Component`, and
//! `Blue Component` `<real>` entries (an optional `Alpha Component` is ignored).
//!
//! The format is fixed and small, so this is a dependency-free linear scan over
//! the `<key>` / `<real>` token stream rather than a full XML/plist parser.

use super::{AnsiRow, Theme, ThemeError};

/// A parsed sRGB color from an `.itermcolors` color dict.
#[derive(Debug, Clone, Copy, Default)]
struct ColorDict {
    red: f64,
    green: f64,
    blue: f64,
}

impl ColorDict {
    /// Formats the color as a `#rrggbb` hex string (the [`Theme`] color form).
    fn to_hex(self) -> String {
        let chan = |v: f64| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        format!(
            "#{:02x}{:02x}{:02x}",
            chan(self.red),
            chan(self.green),
            chan(self.blue)
        )
    }
}

/// Extracts the text between the first `<{tag}>` and its matching `</{tag}>` at
/// or after `from`. Returns the trimmed inner text and the index just past the
/// closing tag.
fn next_tag<'a>(xml: &'a str, tag: &str, from: usize) -> Option<(&'a str, usize)> {
    let open = format!("<{}>", tag);
    let close = format!("</{}>", tag);
    let start = xml[from..].find(&open)? + from + open.len();
    let end = xml[start..].find(&close)? + start;
    Some((xml[start..end].trim(), end + close.len()))
}

/// Parses a single color `<dict>` body (the substring between a `<dict>` and its
/// matching `</dict>`) into a [`ColorDict`].
fn parse_color_dict(body: &str) -> ColorDict {
    let mut c = ColorDict::default();
    let mut pos = 0;
    while let Some((key, after_key)) = next_tag(body, "key", pos) {
        // The value following a key is the next <real> (or <integer>) token.
        let value =
            next_tag(body, "real", after_key).or_else(|| next_tag(body, "integer", after_key));
        let Some((val_str, after_val)) = value else {
            break;
        };
        let v: f64 = val_str.parse().unwrap_or(0.0);
        match key {
            "Red Component" => c.red = v,
            "Green Component" => c.green = v,
            "Blue Component" => c.blue = v,
            _ => {}
        }
        pos = after_val;
    }
    c
}

/// Finds the `<dict>...</dict>` body that immediately follows the
/// `<key>name</key>` entry in the root dict. Nested dicts are balanced so a
/// color dict's body is captured in full.
fn color_dict_body<'a>(xml: &'a str, name: &str) -> Option<&'a str> {
    let key_tag = format!("<key>{}</key>", name);
    let key_pos = xml.find(&key_tag)? + key_tag.len();
    let dict_open = xml[key_pos..].find("<dict>")? + key_pos + "<dict>".len();
    // Balance nested <dict>/</dict>.
    let mut depth = 1usize;
    let mut scan = dict_open;
    loop {
        let next_open = xml[scan..].find("<dict>").map(|i| i + scan);
        let next_close = xml[scan..].find("</dict>").map(|i| i + scan);
        match (next_open, next_close) {
            (Some(o), Some(c)) if o < c => {
                depth += 1;
                scan = o + "<dict>".len();
            }
            (_, Some(c)) => {
                depth -= 1;
                if depth == 0 {
                    return Some(&xml[dict_open..c]);
                }
                scan = c + "</dict>".len();
            }
            _ => return None,
        }
    }
}

/// Resolves the `Ansi {i} Color` slot to a hex string, or `fallback` when the
/// slot is absent. Sets `found` when a slot is present.
fn ansi_hex(xml: &str, i: usize, fallback: &str, found: &mut bool) -> String {
    let slot = format!("Ansi {} Color", i);
    if let Some(body) = color_dict_body(xml, &slot) {
        *found = true;
        parse_color_dict(body).to_hex()
    } else {
        fallback.to_string()
    }
}

/// Resolves a dynamic-color slot (`Foreground`/`Background`/`Cursor Color`) to
/// a hex string, or `fallback` when absent. Sets `found` when present.
fn resolve_dynamic(xml: &str, slot: &str, fallback: &str, found: &mut bool) -> String {
    if let Some(body) = color_dict_body(xml, slot) {
        *found = true;
        parse_color_dict(body).to_hex()
    } else {
        fallback.to_string()
    }
}

/// Builds an [`AnsiRow`] from eight already-resolved hex strings.
fn ansi_row(hex: &[String; 8]) -> AnsiRow {
    AnsiRow {
        black: hex[0].clone(),
        red: hex[1].clone(),
        green: hex[2].clone(),
        yellow: hex[3].clone(),
        blue: hex[4].clone(),
        magenta: hex[5].clone(),
        cyan: hex[6].clone(),
        white: hex[7].clone(),
    }
}

/// Parses an iTerm2 `.itermcolors` plist XML document into a [`Theme`].
///
/// Missing slots fall back to the [`Theme::builtin_void`] values, so a partial
/// file still yields a valid theme. Returns [`ThemeError::Parse`] when the
/// document contains no recognisable color dicts at all, or
/// [`ThemeError::BadHex`] via [`Theme::validate`] if a parsed color is somehow
/// malformed (not normally reachable since channels are clamped).
pub fn from_itermcolors(xml: &str, name: &str) -> Result<Theme, ThemeError> {
    let default = Theme::builtin_void();
    let mut found_any = false;

    let normal_fallback = [
        default.normal.black.clone(),
        default.normal.red.clone(),
        default.normal.green.clone(),
        default.normal.yellow.clone(),
        default.normal.blue.clone(),
        default.normal.magenta.clone(),
        default.normal.cyan.clone(),
        default.normal.white.clone(),
    ];
    let bright_fallback = [
        default.bright.black.clone(),
        default.bright.red.clone(),
        default.bright.green.clone(),
        default.bright.yellow.clone(),
        default.bright.blue.clone(),
        default.bright.magenta.clone(),
        default.bright.cyan.clone(),
        default.bright.white.clone(),
    ];

    let mut normal_hex: [String; 8] = Default::default();
    let mut bright_hex: [String; 8] = Default::default();
    for i in 0..8 {
        normal_hex[i] = ansi_hex(xml, i, normal_fallback[i].as_str(), &mut found_any);
        bright_hex[i] = ansi_hex(xml, i + 8, bright_fallback[i].as_str(), &mut found_any);
    }

    let foreground = resolve_dynamic(xml, "Foreground Color", &default.foreground, &mut found_any);
    let background = resolve_dynamic(xml, "Background Color", &default.background, &mut found_any);
    let cursor = resolve_dynamic(xml, "Cursor Color", &default.cursor, &mut found_any);

    if !found_any {
        return Err(ThemeError::Parse(
            "no recognisable color entries in .itermcolors document".to_string(),
        ));
    }

    let theme = Theme {
        name: name.to_string(),
        author: "imported from .itermcolors".to_string(),
        background,
        foreground,
        cursor,
        cursor_text: default.cursor_text,
        selection_background: default.selection_background,
        selection_foreground: default.selection_foreground,
        normal: ansi_row(&normal_hex),
        bright: ansi_row(&bright_hex),
    };
    theme.validate()?;
    Ok(theme)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>Ansi 0 Color</key>
	<dict>
		<key>Color Space</key>
		<string>sRGB</string>
		<key>Blue Component</key>
		<real>0.0</real>
		<key>Green Component</key>
		<real>0.0</real>
		<key>Red Component</key>
		<real>0.0</real>
	</dict>
	<key>Ansi 1 Color</key>
	<dict>
		<key>Blue Component</key>
		<real>0.0</real>
		<key>Green Component</key>
		<real>0.0</real>
		<key>Red Component</key>
		<real>1.0</real>
	</dict>
	<key>Background Color</key>
	<dict>
		<key>Blue Component</key>
		<real>0.13333333</real>
		<key>Green Component</key>
		<real>0.10588235</real>
		<key>Red Component</key>
		<real>0.10196078</real>
	</dict>
	<key>Foreground Color</key>
	<dict>
		<key>Blue Component</key>
		<real>1.0</real>
		<key>Green Component</key>
		<real>1.0</real>
		<key>Red Component</key>
		<real>1.0</real>
	</dict>
	<key>Cursor Color</key>
	<dict>
		<key>Blue Component</key>
		<real>0.0</real>
		<key>Green Component</key>
		<real>1.0</real>
		<key>Red Component</key>
		<real>0.0</real>
	</dict>
</dict>
</plist>
"#;

    #[test]
    fn from_itermcolors_basic() {
        let theme = from_itermcolors(SAMPLE, "Sample").unwrap();
        assert_eq!(theme.name, "Sample");
        // Ansi 0 = black, Ansi 1 = red.
        assert_eq!(theme.normal.black, "#000000");
        assert_eq!(theme.normal.red, "#ff0000");
        // Foreground white, Cursor green.
        assert_eq!(theme.foreground, "#ffffff");
        assert_eq!(theme.cursor, "#00ff00");
        // The imported theme is valid (all fields are real hex triples).
        assert!(theme.validate().is_ok());
    }

    #[test]
    fn from_itermcolors_background_rounding() {
        let theme = from_itermcolors(SAMPLE, "Sample").unwrap();
        // 0.10196078*255 ~= 26 (1a), 0.10588235*255 ~= 27 (1b), 0.13333333*255 ~= 34 (22)
        assert_eq!(theme.background, "#1a1b22");
    }

    #[test]
    fn from_itermcolors_missing_slots_fall_back() {
        // Only one ANSI color present; the rest fall back to builtin defaults.
        let xml = r#"<plist><dict>
            <key>Ansi 2 Color</key>
            <dict>
                <key>Red Component</key><real>0.0</real>
                <key>Green Component</key><real>1.0</real>
                <key>Blue Component</key><real>0.0</real>
            </dict>
        </dict></plist>"#;
        let theme = from_itermcolors(xml, "Partial").unwrap();
        assert_eq!(theme.normal.green, "#00ff00");
        let default = Theme::builtin_void();
        assert_eq!(theme.normal.black, default.normal.black);
        assert_eq!(theme.foreground, default.foreground);
    }

    #[test]
    fn from_itermcolors_empty_errors() {
        assert!(from_itermcolors("<plist><dict></dict></plist>", "Empty").is_err());
    }
}
