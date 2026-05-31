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
    // Cap the total pixel count to bound the output allocation against a hostile
    // stream. The input is already capped at 8 MiB, but a sparse plot could
    // still imply a very large canvas; 16 Mpx (≈ 4096×4096) is the practical
    // Sixel ceiling. Mirrors `decode_kitty`'s checked-multiply guard.
    const MAX_SIXEL_PIXELS: usize = 16 * 1024 * 1024;
    if width
        .checked_mul(height)
        .is_none_or(|n| n > MAX_SIXEL_PIXELS)
    {
        return None;
    }
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
    /// Action: `'t'` transmit-only, `'T'` transmit+display, `'p'` display-stored,
    /// `'d'` delete. When the `a=` key is ABSENT the Kitty spec defaults to the
    /// transmit-and-display path, so we default to `'T'` here.
    pub action: char,
    /// `m=` more-chunks flag: `true` (1) means another chunk follows; `false`
    /// (0 or absent) means this is the last (or only) chunk.
    pub more: bool,
    /// `i=` image id (0 when absent). Used to key chunked transmissions and the
    /// transmit-only / display-stored (`a=t` / `a=p`) store.
    pub id: u32,
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
    // Absent `a=` ⇒ transmit-and-display ('T') per the Kitty spec default.
    let mut action = 'T';
    let mut more = false;
    let mut id = 0u32;
    for kv in control.split(',') {
        if let Some((k, v)) = kv.split_once('=') {
            match k {
                "f" => format = v.parse().unwrap_or(0),
                "s" => width = v.parse().unwrap_or(0),
                "v" => height = v.parse().unwrap_or(0),
                "a" => action = v.chars().next().unwrap_or('T'),
                "m" => more = v.trim() == "1",
                "i" => id = v.parse().unwrap_or(0),
                _ => {}
            }
        }
    }
    Some(KittyCommand {
        format,
        width,
        height,
        action,
        more,
        id,
        payload,
    })
}

/// Decode an already-base64-decoded Kitty payload into RGBA pixels.
///
/// `raw` is the *decoded* image data (the caller must base64-decode first,
/// accumulating chunked payloads across `m=1` … `m=0` boundaries before
/// calling). Supported formats:
///
/// - `f=32` — RGBA8, tightly packed (`width * height * 4`).
/// - `f=24` — RGB8 (`width * height * 3`), expanded to RGBA with alpha 255.
/// - `f=100` — a PNG file; `width`/`height` are read from the PNG itself.
///
/// Any other format (or a length mismatch for the raw formats) returns `None` —
/// an honest gap. `f=32`/`f=24`/`f=100` cover the `icat`/`timg` tools.
pub fn decode_kitty(format: u16, width: usize, height: usize, raw: &[u8]) -> Option<DecodedImage> {
    match format {
        32 => {
            if width == 0 || height == 0 {
                return None;
            }
            let expected = width.checked_mul(height)?.checked_mul(4)?;
            if raw.len() != expected {
                return None;
            }
            Some(DecodedImage {
                width,
                height,
                rgba: raw.to_vec(),
            })
        }
        24 => {
            if width == 0 || height == 0 {
                return None;
            }
            let expected = width.checked_mul(height)?.checked_mul(3)?;
            if raw.len() != expected {
                return None;
            }
            let mut rgba = Vec::with_capacity(width * height * 4);
            for px in raw.chunks_exact(3) {
                rgba.extend_from_slice(&[px[0], px[1], px[2], 255]);
            }
            Some(DecodedImage {
                width,
                height,
                rgba,
            })
        }
        100 => {
            let dynimg = image::load_from_memory_with_format(raw, image::ImageFormat::Png).ok()?;
            let rgba8 = dynimg.to_rgba8();
            let (w, h) = (rgba8.width() as usize, rgba8.height() as usize);
            Some(DecodedImage {
                width: w,
                height: h,
                rgba: rgba8.into_raw(),
            })
        }
        // Honest gap: f != 32/24/100 is unsupported.
        _ => None,
    }
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
        // Defaults when a/m/i are absent: action 'T', not-more, id 0.
        assert_eq!(cmd.action, 'T');
        assert!(!cmd.more);
        assert_eq!(cmd.id, 0);
    }

    #[test]
    fn kitty_captures_action_more_and_id() {
        let cmd = parse_kitty(b"a=t,f=24,s=3,v=1,m=1,i=42;Zm9v").expect("parsed");
        assert_eq!(cmd.action, 't');
        assert_eq!(cmd.format, 24);
        assert_eq!(cmd.width, 3);
        assert_eq!(cmd.height, 1);
        assert!(cmd.more);
        assert_eq!(cmd.id, 42);
        assert_eq!(cmd.payload, b"Zm9v");

        // m=0 explicitly means last chunk; action 'p' / 'd' captured verbatim.
        let last = parse_kitty(b"a=p,i=7,m=0;").expect("parsed");
        assert_eq!(last.action, 'p');
        assert_eq!(last.id, 7);
        assert!(!last.more);

        let del = parse_kitty(b"a=d").expect("parsed");
        assert_eq!(del.action, 'd');
    }

    #[test]
    fn decode_kitty_f32_roundtrips_rgba() {
        // 2x2 RGBA: red, green, blue, white — tightly packed.
        let raw = [
            255, 0, 0, 255, // (0,0) red
            0, 255, 0, 255, // (1,0) green
            0, 0, 255, 255, // (0,1) blue
            255, 255, 255, 255, // (1,1) white
        ];
        let img = decode_kitty(32, 2, 2, &raw).expect("decoded");
        assert_eq!(img.width, 2);
        assert_eq!(img.height, 2);
        assert_eq!(img.pixel(0, 0), [255, 0, 0, 255]);
        assert_eq!(img.pixel(1, 0), [0, 255, 0, 255]);
        assert_eq!(img.pixel(0, 1), [0, 0, 255, 255]);
        assert_eq!(img.pixel(1, 1), [255, 255, 255, 255]);
    }

    #[test]
    fn decode_kitty_f32_rejects_wrong_length() {
        // 2x2 RGBA needs 16 bytes; give 15.
        assert!(decode_kitty(32, 2, 2, &[0u8; 15]).is_none());
    }

    #[test]
    fn decode_kitty_f24_expands_rgb_to_rgba() {
        // 2x1 RGB: red, green.
        let raw = [255, 0, 0, 0, 255, 0];
        let img = decode_kitty(24, 2, 1, &raw).expect("decoded");
        assert_eq!(img.width, 2);
        assert_eq!(img.height, 1);
        assert_eq!(img.pixel(0, 0), [255, 0, 0, 255]);
        assert_eq!(img.pixel(1, 0), [0, 255, 0, 255]);
    }

    #[test]
    fn decode_kitty_f100_decodes_png() {
        // Build a tiny 2x2 PNG in-memory with the `image` crate, then decode it.
        let mut src = image::RgbaImage::new(2, 2);
        src.put_pixel(0, 0, image::Rgba([255, 0, 0, 255]));
        src.put_pixel(1, 0, image::Rgba([0, 255, 0, 255]));
        src.put_pixel(0, 1, image::Rgba([0, 0, 255, 255]));
        src.put_pixel(1, 1, image::Rgba([10, 20, 30, 255]));
        let mut png_bytes: Vec<u8> = Vec::new();
        image::DynamicImage::ImageRgba8(src)
            .write_to(
                &mut std::io::Cursor::new(&mut png_bytes),
                image::ImageFormat::Png,
            )
            .expect("encoded png");

        // width/height args are ignored for f=100 — taken from the PNG itself.
        let img = decode_kitty(100, 0, 0, &png_bytes).expect("decoded");
        assert_eq!(img.width, 2);
        assert_eq!(img.height, 2);
        assert_eq!(img.pixel(0, 0), [255, 0, 0, 255]);
        assert_eq!(img.pixel(1, 1), [10, 20, 30, 255]);
    }

    #[test]
    fn decode_kitty_unsupported_format_is_none() {
        assert!(decode_kitty(7, 2, 2, &[0u8; 16]).is_none());
    }
}
