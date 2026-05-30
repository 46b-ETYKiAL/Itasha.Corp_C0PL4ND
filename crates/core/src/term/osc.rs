//! OSC (Operating System Command) helpers for the terminal core.
//!
//! Houses self-contained helpers for OSC sequence handling so the main
//! [`crate::term`] module's `Perform` impl stays focused on dispatch. This
//! includes a dependency-free RFC 4648 base64 codec (for OSC 52 clipboard
//! payloads), X-style color spec parsing/formatting (for OSC 4/10/11/12), and
//! the small value types the public API hands back to the application.

use crate::term::Rgb;

// ============================================================================
// Clipboard (OSC 52)
// ============================================================================

/// Which selection an OSC 52 clipboard operation targets.
///
/// An OSC 52 sequence may list several selection characters (e.g. `c` for the
/// system clipboard, `p` for the primary selection). We model the two we care
/// about; any unknown/empty selection list falls back to
/// [`ClipboardSelection::Clipboard`], matching xterm's default.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClipboardSelection {
    /// The system clipboard (`c`).
    Clipboard,
    /// The primary selection (`p`).
    Primary,
}

/// A pending OSC 52 clipboard write request drained by the application.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClipboardWrite {
    /// The selection targeted by the request.
    pub selection: ClipboardSelection,
    /// The decoded UTF-8 text to place on the clipboard.
    pub text: String,
}

// ============================================================================
// base64 (RFC 4648, standard alphabet)
// ============================================================================

const B64_ALPHA: &[u8; 64] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Encodes raw bytes as standard-alphabet RFC 4648 base64 with padding.
///
/// Used to build OSC 52 clipboard READ replies. Dependency-free.
pub fn base64_encode(input: &[u8]) -> String {
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    let mut chunks = input.chunks_exact(3);
    for c in &mut chunks {
        let n = (c[0] as u32) << 16 | (c[1] as u32) << 8 | c[2] as u32;
        out.push(B64_ALPHA[(n >> 18 & 0x3f) as usize] as char);
        out.push(B64_ALPHA[(n >> 12 & 0x3f) as usize] as char);
        out.push(B64_ALPHA[(n >> 6 & 0x3f) as usize] as char);
        out.push(B64_ALPHA[(n & 0x3f) as usize] as char);
    }
    let rem = chunks.remainder();
    match rem.len() {
        0 => {}
        1 => {
            let n = (rem[0] as u32) << 16;
            out.push(B64_ALPHA[(n >> 18 & 0x3f) as usize] as char);
            out.push(B64_ALPHA[(n >> 12 & 0x3f) as usize] as char);
            out.push('=');
            out.push('=');
        }
        2 => {
            let n = (rem[0] as u32) << 16 | (rem[1] as u32) << 8;
            out.push(B64_ALPHA[(n >> 18 & 0x3f) as usize] as char);
            out.push(B64_ALPHA[(n >> 12 & 0x3f) as usize] as char);
            out.push(B64_ALPHA[(n >> 6 & 0x3f) as usize] as char);
            out.push('=');
        }
        _ => unreachable!("chunks_exact remainder is < 3"),
    }
    out
}

/// Decodes standard-alphabet RFC 4648 base64 into raw bytes.
///
/// Whitespace is ignored. Padding (`=`) is honoured. Returns `None` on any
/// invalid character or malformed length. Dependency-free.
pub fn base64_decode(input: &[u8]) -> Option<Vec<u8>> {
    fn val(b: u8) -> Option<u8> {
        match b {
            b'A'..=b'Z' => Some(b - b'A'),
            b'a'..=b'z' => Some(b - b'a' + 26),
            b'0'..=b'9' => Some(b - b'0' + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }

    // Strip whitespace.
    let mut symbols: Vec<u8> = Vec::with_capacity(input.len());
    for &b in input {
        if matches!(b, b' ' | b'\t' | b'\r' | b'\n') {
            continue;
        }
        symbols.push(b);
    }

    // Count trailing padding and validate it is only at the end.
    let mut pad = 0usize;
    while symbols.last() == Some(&b'=') {
        symbols.pop();
        pad += 1;
    }
    if pad > 2 || symbols.iter().any(|&b| b == b'=') {
        return None;
    }

    // Leftover symbol count must be consistent with the padding.
    match (pad, symbols.len() % 4) {
        (0, 0) | (1, 3) | (2, 2) => {}
        _ => return None,
    }

    let mut out = Vec::with_capacity(symbols.len() * 3 / 4 + 3);
    let mut chunk = symbols.chunks_exact(4);
    for c in &mut chunk {
        let a = val(c[0])?;
        let b = val(c[1])?;
        let d = val(c[2])?;
        let e = val(c[3])?;
        let n = (a as u32) << 18 | (b as u32) << 12 | (d as u32) << 6 | (e as u32);
        out.push((n >> 16) as u8);
        out.push((n >> 8) as u8);
        out.push(n as u8);
    }
    let tail = chunk.remainder();
    match tail.len() {
        0 => {}
        2 => {
            let a = val(tail[0])?;
            let b = val(tail[1])?;
            let n = (a as u32) << 18 | (b as u32) << 12;
            out.push((n >> 16) as u8);
        }
        3 => {
            let a = val(tail[0])?;
            let b = val(tail[1])?;
            let d = val(tail[2])?;
            let n = (a as u32) << 18 | (b as u32) << 12 | (d as u32) << 6;
            out.push((n >> 16) as u8);
            out.push((n >> 8) as u8);
        }
        _ => return None,
    }
    Some(out)
}

// ============================================================================
// Color spec parsing / formatting (OSC 4 / 10 / 11 / 12)
// ============================================================================

/// Parses an X11-style color spec into an [`Rgb`].
///
/// Supports the two forms terminals commonly emit:
/// - `rgb:RR/GG/BB`, `rgb:RRRR/GGGG/BBBB` (1-4 hex digits per channel; scaled
///   to 8 bits).
/// - `#RGB`, `#RRGGBB`, `#RRRRGGGGBBBB` (xterm `#` form).
///
/// Returns `None` for unrecognised specs (including the `?` query sentinel,
/// which the caller handles separately).
pub fn parse_color_spec(spec: &str) -> Option<Rgb> {
    let spec = spec.trim();
    if let Some(rest) = spec.strip_prefix("rgb:") {
        let mut parts = rest.split('/');
        let r = scale_hex_channel(parts.next()?)?;
        let g = scale_hex_channel(parts.next()?)?;
        let b = scale_hex_channel(parts.next()?)?;
        if parts.next().is_some() {
            return None;
        }
        return Some(Rgb::new(r, g, b));
    }
    if let Some(rest) = spec.strip_prefix('#') {
        let per = match rest.len() {
            3 => 1,
            6 => 2,
            12 => 4,
            _ => return None,
        };
        let r = scale_hex_channel(&rest[0..per])?;
        let g = scale_hex_channel(&rest[per..per * 2])?;
        let b = scale_hex_channel(&rest[per * 2..per * 3])?;
        return Some(Rgb::new(r, g, b));
    }
    None
}

/// Scales a 1-4 digit hex channel string into an 8-bit value.
///
/// A channel of width `n` hex digits represents a value in `[0, 16^n - 1]`;
/// we rescale to `[0, 255]` with rounding. A single digit `f` therefore maps to
/// `0xff`, matching xterm.
fn scale_hex_channel(s: &str) -> Option<u8> {
    if s.is_empty() || s.len() > 4 {
        return None;
    }
    let max = (1u32 << (4 * s.len() as u32)) - 1;
    let v = u32::from_str_radix(s, 16).ok()?;
    if v > max {
        return None;
    }
    Some(((v * 255 + max / 2) / max) as u8)
}

/// Formats an [`Rgb`] as the `rgb:RRRR/GGGG/BBBB` 16-bit-per-channel reply form
/// xterm uses in OSC 4/10/11/12 query responses.
///
/// Each 8-bit channel is replicated into the high byte (`v -> v*0x101`) so that
/// `0xff` becomes `ffff`, matching xterm's reported values.
pub fn format_color_reply(rgb: Rgb) -> String {
    let expand = |v: u8| (v as u16) * 0x101;
    format!(
        "rgb:{:04x}/{:04x}/{:04x}",
        expand(rgb.r),
        expand(rgb.g),
        expand(rgb.b)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_base64_decode_basic() {
        assert_eq!(base64_decode(b"aGVsbG8=").unwrap(), b"hello");
        assert_eq!(base64_decode(b"aGVsbG8gd29ybGQ=").unwrap(), b"hello world");
    }

    #[test]
    fn test_base64_decode_no_padding() {
        assert_eq!(base64_decode(b"Zm9v").unwrap(), b"foo");
    }

    #[test]
    fn test_base64_decode_padding_variants() {
        assert_eq!(base64_decode(b"Zg==").unwrap(), b"f");
        assert_eq!(base64_decode(b"Zm8=").unwrap(), b"fo");
    }

    #[test]
    fn test_base64_decode_whitespace_ignored() {
        assert_eq!(base64_decode(b"aGVs\r\nbG8=").unwrap(), b"hello");
    }

    #[test]
    fn test_base64_decode_invalid() {
        assert!(base64_decode(b"!!!!").is_none());
        assert!(base64_decode(b"aGVsbG8==").is_none()); // 3 pad
        assert!(base64_decode(b"aGV").is_none()); // bad length, no pad
    }

    #[test]
    fn test_base64_encode_basic() {
        assert_eq!(base64_encode(b"hello"), "aGVsbG8=");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
    }

    #[test]
    fn test_base64_roundtrip() {
        let data = b"The quick brown fox jumps over the lazy dog.";
        let enc = base64_encode(data);
        assert_eq!(base64_decode(enc.as_bytes()).unwrap(), data);
    }

    #[test]
    fn test_parse_color_spec_rgb() {
        assert_eq!(parse_color_spec("rgb:ff/00/00"), Some(Rgb::new(255, 0, 0)));
        assert_eq!(
            parse_color_spec("rgb:ffff/0000/8080"),
            Some(Rgb::new(255, 0, 128))
        );
        assert_eq!(parse_color_spec("rgb:f/0/0"), Some(Rgb::new(255, 0, 0)));
    }

    #[test]
    fn test_parse_color_spec_hash() {
        assert_eq!(parse_color_spec("#ff0000"), Some(Rgb::new(255, 0, 0)));
        assert_eq!(parse_color_spec("#f00"), Some(Rgb::new(255, 0, 0)));
        assert_eq!(
            parse_color_spec("#ffff00008080"),
            Some(Rgb::new(255, 0, 128))
        );
    }

    #[test]
    fn test_parse_color_spec_invalid() {
        assert!(parse_color_spec("?").is_none());
        assert!(parse_color_spec("rgb:zz/00/00").is_none());
        assert!(parse_color_spec("#12345").is_none());
    }

    #[test]
    fn test_format_color_reply() {
        assert_eq!(
            format_color_reply(Rgb::new(255, 0, 0)),
            "rgb:ffff/0000/0000"
        );
        assert_eq!(
            format_color_reply(Rgb::new(26, 27, 38)),
            "rgb:1a1a/1b1b/2626"
        );
    }
}
