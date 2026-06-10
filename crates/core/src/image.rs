//! Inline-image protocol decoding.
//!
//! Implements a self-contained **Sixel** decoder (the broadly-compatible
//! fallback) plus detection/control-parse for the **Kitty graphics protocol**
//! (the modern tier). Both produce a [`DecodedImage`] of RGBA pixels that the
//! renderer uploads as a texture. Decoding is pure and dependency-free so it is
//! fully unit-testable without a GPU.

/// Per-axis ceiling (px) for a decoded inline image. Bounds the declared
/// `IHDR`/header dimensions of a Kitty `f=100` PNG so a tiny highly-compressed
/// payload cannot declare an enormous surface (decompression bomb). Generous for
/// any real terminal image; `MAX_SIXEL_PIXELS` (16 Mpx) bounds the Sixel path.
const MAX_IMAGE_DIM: u32 = 8192;

/// Total allocation ceiling (bytes) the PNG decoder may use, enforced DURING
/// decode (before the full surface is allocated). 256 MiB caps even an
/// 8192×8192×4 (256 MiB) worst case at the dimension limit.
const MAX_IMAGE_ALLOC_BYTES: u64 = 256 * 1024 * 1024;

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

    // Per-axis ceiling so a hostile RLE run (`!Pn`) or a long column of band-LFs
    // cannot grow `x` / `band_top` (and thus the sparse pixel map) without bound
    // before the final whole-canvas check. 4096 per axis keeps the dense worst
    // case at the 16 Mpx `MAX_SIXEL_PIXELS` ceiling. Found by `fuzz_sixel`: a
    // tiny payload of repeated max-count RLE tokens otherwise pins a core thread
    // and balloons the pixel HashMap.
    const MAX_SIXEL_DIM: usize = 4096;
    const MAX_SIXEL_PIXELS: usize = 16 * 1024 * 1024;

    let mut i = 0;
    while i < data.len() {
        // Backstop: never let the sparse pixel map exceed the output ceiling,
        // regardless of how the bytes try to grow it.
        if pixels.len() > MAX_SIXEL_PIXELS {
            return None;
        }
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
                // Graphics LF: move down one band (6 px). Capped per-axis so a
                // long column of LFs cannot grow the canvas without bound.
                i += 1;
                x = 0;
                if band_top < MAX_SIXEL_DIM {
                    band_top += 6;
                }
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
                        // Clamp the repeat so `x` can never exceed the per-axis
                        // ceiling — bounds the loop AND the pixel map.
                        let reps = (count.max(1) as usize).min(MAX_SIXEL_DIM.saturating_sub(x));
                        for _ in 0..reps {
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
                if x < MAX_SIXEL_DIM {
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
    // Final whole-canvas guard: bound the output allocation against a hostile
    // stream (the per-axis `MAX_SIXEL_DIM` clamp above already bounds each
    // dimension; this is the belt-and-suspenders product check). Mirrors
    // `decode_kitty`'s checked-multiply guard.
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
///
/// SECURITY: the accumulation is **saturating** and clamped to `u16::MAX` on
/// every step. A naive `v = v * 10 + d` overflows `u32` on a long digit run
/// (e.g. a hostile `#999999999999...` colour index or `!999999999999...` RLE
/// count) — which panics under overflow-checks (the cargo-fuzz profile) and
/// silently wraps to a wrong value in release. Saturating keeps the parser
/// position correct (every digit is still consumed, so `n` advances past the
/// whole run) while bounding the value. Found by `fuzz_sixel`.
fn parse_u16(data: &[u8]) -> (u16, usize) {
    let mut v: u32 = 0;
    let mut n = 0;
    while n < data.len() && data[n].is_ascii_digit() {
        v = v
            .saturating_mul(10)
            .saturating_add((data[n] - b'0') as u32)
            .min(u16::MAX as u32);
        n += 1;
    }
    (v as u16, n)
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
            // PNG (Kitty `f=100`). SECURITY: a tiny, highly-compressed PNG can
            // declare enormous dimensions (e.g. 30000×30000×4 ≈ 3.4 GB RGBA) — a
            // decompression bomb reachable from ANY program printing a Kitty
            // graphics escape (fully untrusted input). The 8 MiB compressed-input
            // cap upstream does NOT bound the decoded surface. Decode through the
            // limit-aware `ImageReader` so the dimension + allocation ceilings are
            // enforced DURING decode, before the surface is allocated — mirroring
            // the in-house Sixel path's `MAX_SIXEL_PIXELS` guard.
            let mut reader =
                image::ImageReader::with_format(std::io::Cursor::new(raw), image::ImageFormat::Png);
            let mut limits = image::Limits::default();
            limits.max_image_width = Some(MAX_IMAGE_DIM);
            limits.max_image_height = Some(MAX_IMAGE_DIM);
            limits.max_alloc = Some(MAX_IMAGE_ALLOC_BYTES);
            reader.limits(limits);
            let dynimg = reader.decode().ok()?;
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

    /// CRC-32 (PNG/zlib polynomial) over a chunk's type+data — for building test
    /// PNGs with a valid IHDR.
    fn png_crc32(bytes: &[u8]) -> u32 {
        let mut crc: u32 = 0xFFFF_FFFF;
        for &b in bytes {
            crc ^= b as u32;
            for _ in 0..8 {
                crc = if crc & 1 != 0 {
                    (crc >> 1) ^ 0xEDB8_8320
                } else {
                    crc >> 1
                };
            }
        }
        crc ^ 0xFFFF_FFFF
    }

    /// A minimal PNG whose IHDR DECLARES `w × h` (8-bit RGBA), with a valid IHDR
    /// CRC but no real pixel data. A limit-aware decoder rejects it at the IHDR
    /// dimension check — before allocating — so it models a decompression bomb
    /// (tiny payload, enormous declared surface).
    fn png_declaring_dims(w: u32, h: u32) -> Vec<u8> {
        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(b"IHDR");
        ihdr.extend_from_slice(&w.to_be_bytes());
        ihdr.extend_from_slice(&h.to_be_bytes());
        ihdr.extend_from_slice(&[8, 6, 0, 0, 0]); // 8-bit, RGBA, deflate, none, no-interlace
        let crc = png_crc32(&ihdr);
        let mut png = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        png.extend_from_slice(&13u32.to_be_bytes()); // IHDR data length
        png.extend_from_slice(&ihdr); // "IHDR" + 13 data bytes
        png.extend_from_slice(&crc.to_be_bytes());
        png
    }

    /// SECURITY: a Kitty `f=100` PNG that declares dimensions beyond the decode
    /// ceiling is rejected (returns `None`) instead of allocating a multi-GB
    /// surface — the image-decompression-bomb guard. (A legitimate small PNG
    /// still decoding through the limit-aware path is covered by
    /// `decode_kitty_f100_decodes_png`.)
    #[test]
    fn kitty_png_oversized_dimensions_are_rejected() {
        let bomb = png_declaring_dims(30000, 30000); // ~3.4 GB RGBA if honoured
        assert!(
            decode_kitty(100, 0, 0, &bomb).is_none(),
            "a PNG declaring 30000x30000 must be rejected by the decode limit"
        );
        // A dimension just past the cap is also rejected.
        let over = png_declaring_dims(MAX_IMAGE_DIM + 1, 1);
        assert!(
            decode_kitty(100, 0, 0, &over).is_none(),
            "width > MAX_IMAGE_DIM rejected"
        );
    }

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
    fn parse_u16_saturates_long_digit_run_without_overflow() {
        // Regression (fuzz_sixel): a digit run longer than u32 can hold must
        // NOT overflow the accumulator — it saturates to u16::MAX and still
        // consumes every digit so the parser position is correct.
        let long = b"99999999999999999999rest";
        let (v, n) = parse_u16(long);
        assert_eq!(v, u16::MAX);
        assert_eq!(n, 20); // all 20 nines consumed, stops at 'r'
    }

    #[test]
    fn sixel_hostile_rle_count_is_bounded_no_panic() {
        // Regression (fuzz_sixel): a huge RLE count (`!<many digits>~`) must not
        // overflow parse_u16 NOR grow the canvas/pixel-map without bound. The
        // per-axis clamp keeps width within MAX_SIXEL_DIM; decode returns
        // bounded output (or None) and never panics or hangs.
        let mut data = b"#0;2;100;0;0!".to_vec();
        data.extend_from_slice(b"99999999999999999999"); // count overflows u32 pre-fix
        data.push(b'~');
        let img = decode_sixel(&data).expect("bounded decode");
        assert!(img.width <= 4096, "width clamped to MAX_SIXEL_DIM");
    }

    #[test]
    fn sixel_hostile_band_advance_is_bounded_no_panic() {
        // Regression (fuzz_sixel): a long column of graphics-LFs cannot grow the
        // canvas height past the per-axis ceiling.
        let mut data = b"#0;2;0;0;100~".to_vec();
        for _ in 0..10000 {
            data.push(b'-'); // 10000 band advances * 6 = 60000 px without the cap
            data.push(b'~');
        }
        let img = decode_sixel(&data);
        if let Some(img) = img {
            // Bounded near MAX_SIXEL_DIM (the cap stops at the first band_top
            // step >= the ceiling, so a small overshoot of one band + 6 rows is
            // expected). The point: it is NOT the 60000 px an unbounded decoder
            // would reach from 10000 band advances.
            assert!(
                img.height < 4200,
                "height bounded near MAX_SIXEL_DIM, got {}",
                img.height
            );
        }
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
