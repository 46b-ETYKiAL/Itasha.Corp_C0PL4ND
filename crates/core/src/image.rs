//! Inline-image protocol decoding.
//!
//! Implements a self-contained **Sixel** decoder (the broadly-compatible
//! fallback) plus detection/control-parse for the **Kitty graphics protocol**
//! (the modern tier). Both produce a [`DecodedImage`] of RGBA pixels that the
//! renderer uploads as a texture. Decoding is pure and dependency-free so it is
//! fully unit-testable without a GPU.

/// A decoded image: tightly-packed RGBA8, row-major, `width * height * 4` bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedImage {
    pub width: usize,
    pub height: usize,
    pub rgba: Vec<u8>,
}

impl DecodedImage {
    #[cfg(test)]
    fn pixel(&self, x: usize, y: usize) -> [u8; 4] {
        let i = (y * self.width + x) * 4;
        [
            self.rgba[i],
            self.rgba[i + 1],
            self.rgba[i + 2],
            self.rgba[i + 3],
        ]
    }
}

/// Decode a Sixel data stream (the bytes between `DCS ... q` and `ST`).
/// Returns `None` if the stream contains no drawable pixels.
pub fn decode_sixel(data: &[u8]) -> Option<DecodedImage> {
    // Palette: index -> RGBA. Sixel RGB components are 0..=100.
    let mut palette: std::collections::HashMap<u16, [u8; 4]> = std::collections::HashMap::new();
    // Sparse pixel map keyed by (x, y) so we can grow without pre-sizing.
    let mut pixels: std::collections::HashMap<(usize, usize), [u8; 4]> =
        std::collections::HashMap::new();

    let mut color: [u8; 4] = [255, 255, 255, 255];
    let mut x = 0usize;
    let mut band_top = 0usize; // y of the current 6-pixel band
    let mut max_x = 0usize;
    let mut max_y = 0usize;

    let mut i = 0;
    while i < data.len() {
        let b = data[i];
        match b {
            b'#' => {
                // Colour introducer: #Pc  or  #Pc;Pu;Px;Py;Pz
                i += 1;
                let (n, adv) = parse_u16(&data[i..]);
                i += adv;
                if i < data.len() && data[i] == b';' {
                    // Definition: ;2;r;g;b (2 = RGB).
                    let mut nums = Vec::new();
                    while i < data.len() && data[i] == b';' {
                        i += 1;
                        let (v, a) = parse_u16(&data[i..]);
                        i += a;
                        nums.push(v);
                    }
                    if nums.len() == 4 && nums[0] == 2 {
                        let to255 = |c: u16| ((c.min(100) as u32 * 255) / 100) as u8;
                        palette.insert(n, [to255(nums[1]), to255(nums[2]), to255(nums[3]), 255]);
                    }
                }
                color = *palette.get(&n).unwrap_or(&color);
            }
            b'$' => {
                // Graphics CR: back to the start of the current band.
                i += 1;
                x = 0;
            }
            b'-' => {
                // Graphics LF: move down one band (6 px).
                i += 1;
                x = 0;
                band_top += 6;
            }
            b'!' => {
                // RLE: !Pn <sixel> repeats the sixel Pn times.
                i += 1;
                let (count, adv) = parse_u16(&data[i..]);
                i += adv;
                if i < data.len() {
                    let sx = data[i];
                    i += 1;
                    if (0x3f..=0x7e).contains(&sx) {
                        let bits = sx - 0x3f;
                        for _ in 0..count.max(1) {
                            plot(
                                &mut pixels,
                                &mut max_x,
                                &mut max_y,
                                x,
                                band_top,
                                bits,
                                color,
                            );
                            x += 1;
                        }
                    }
                }
            }
            0x3f..=0x7e => {
                // A sixel: 6 vertical pixels.
                let bits = b - 0x3f;
                plot(
                    &mut pixels,
                    &mut max_x,
                    &mut max_y,
                    x,
                    band_top,
                    bits,
                    color,
                );
                x += 1;
                i += 1;
            }
            b'"' => {
                // Raster attributes "Pan;Pad;Ph;Pv — skip to next non-digit/;.
                i += 1;
                while i < data.len() && (data[i].is_ascii_digit() || data[i] == b';') {
                    i += 1;
                }
            }
            _ => i += 1, // ignore unknown bytes
        }
    }

    if pixels.is_empty() {
        return None;
    }
    let width = max_x + 1;
    let height = max_y + 1;
    let mut rgba = vec![0u8; width * height * 4];
    for ((px, py), c) in pixels {
        let idx = (py * width + px) * 4;
        rgba[idx..idx + 4].copy_from_slice(&c);
    }
    Some(DecodedImage {
        width,
        height,
        rgba,
    })
}

fn plot(
    pixels: &mut std::collections::HashMap<(usize, usize), [u8; 4]>,
    max_x: &mut usize,
    max_y: &mut usize,
    x: usize,
    band_top: usize,
    bits: u8,
    color: [u8; 4],
) {
    for row in 0..6 {
        if bits & (1 << row) != 0 {
            let y = band_top + row;
            pixels.insert((x, y), color);
            *max_x = (*max_x).max(x);
            *max_y = (*max_y).max(y);
        }
    }
}

/// Parse a leading run of ASCII digits as a u16; returns (value, bytes_read).
fn parse_u16(data: &[u8]) -> (u16, usize) {
    let mut v: u32 = 0;
    let mut n = 0;
    while n < data.len() && data[n].is_ascii_digit() {
        v = v * 10 + (data[n] - b'0') as u32;
        n += 1;
    }
    (v.min(u16::MAX as u32) as u16, n)
}

/// A parsed Kitty graphics command (control keys + raw payload).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KittyCommand {
    /// Format: 24 = RGB, 32 = RGBA, 100 = PNG.
    pub format: u16,
    pub width: usize,
    pub height: usize,
    /// The base64-encoded payload (undecoded — caller decodes per `format`).
    pub payload: Vec<u8>,
}

/// Parse the body of a Kitty graphics APC: `G<control>;<base64-payload>`.
/// (The caller strips the `\x1b_G` prefix and `\x1b\\` terminator.)
pub fn parse_kitty(body: &[u8]) -> Option<KittyCommand> {
    let s = std::str::from_utf8(body).ok()?;
    let (control, payload) = match s.split_once(';') {
        Some((c, p)) => (c, p.as_bytes().to_vec()),
        None => (s, Vec::new()),
    };
    let mut format = 32;
    let mut width = 0;
    let mut height = 0;
    for kv in control.split(',') {
        if let Some((k, v)) = kv.split_once('=') {
            let n: usize = v.parse().unwrap_or(0);
            match k {
                "f" => format = n as u16,
                "s" => width = n,
                "v" => height = n,
                _ => {}
            }
        }
    }
    Some(KittyCommand {
        format,
        width,
        height,
        payload,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sixel_single_column_sets_pixels() {
        // Define colour 0 as red, then one sixel with all 6 bits set (~ = 0x7e).
        let data = b"#0;2;100;0;0#0~";
        let img = decode_sixel(data).expect("decoded");
        assert_eq!(img.width, 1);
        assert_eq!(img.height, 6);
        assert_eq!(img.pixel(0, 0), [255, 0, 0, 255]);
        assert_eq!(img.pixel(0, 5), [255, 0, 0, 255]);
    }

    #[test]
    fn sixel_rle_repeats() {
        // !5~ repeats a full sixel 5 times → width 5.
        let data = b"#0;2;0;100;0!5~";
        let img = decode_sixel(data).expect("decoded");
        assert_eq!(img.width, 5);
        assert_eq!(img.pixel(4, 0), [0, 255, 0, 255]);
    }

    #[test]
    fn sixel_newline_advances_band() {
        // One sixel, graphics-newline, one sixel → height spans two bands.
        let data = b"#0;2;0;0;100~-~";
        let img = decode_sixel(data).expect("decoded");
        assert_eq!(img.height, 12);
    }

    #[test]
    fn sixel_empty_is_none() {
        assert!(decode_sixel(b"").is_none());
    }

    #[test]
    fn kitty_parses_control_and_payload() {
        let cmd = parse_kitty(b"f=32,s=2,v=2;YWJjZA==").expect("parsed");
        assert_eq!(cmd.format, 32);
        assert_eq!(cmd.width, 2);
        assert_eq!(cmd.height, 2);
        assert_eq!(cmd.payload, b"YWJjZA==");
    }
}
