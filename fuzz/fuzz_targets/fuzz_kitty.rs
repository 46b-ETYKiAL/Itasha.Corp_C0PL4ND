#![no_main]
//! Fuzz target for the C0PL4ND Kitty graphics protocol decoder.
//!
//! The Kitty graphics protocol arrives over the PTY as an APC string
//! (`\x1b_G<control>;<base64-payload>\x1b\\`) emitted by ANY program — fully
//! untrusted input. Two pure surfaces decode it:
//!
//!   * `parse_kitty(body)` — splits the control keys (`f`/`s`/`v`/`a`/`m`/`i`)
//!     from the payload. It parses attacker-controlled integers and a UTF-8
//!     control string.
//!   * `decode_kitty(format, width, height, raw)` — turns the decoded payload
//!     into RGBA pixels per format: f=32 (RGBA), f=24 (RGB→RGBA), f=100 (a PNG
//!     parsed by the `image` crate). It does checked integer arithmetic on
//!     attacker-controlled `width`/`height` and hands arbitrary bytes to the
//!     PNG decoder.
//!
//! A malformed or hostile graphics command must never panic, hang, overflow, or
//! OOB. This target drives BOTH entry points on arbitrary bytes and asserts only
//! that they RETURN (`Option`) — a `None` on garbage is correct, a panic is a
//! bug. The first input bytes are used to derive the format/dimensions so all of
//! f=24 / f=32 / f=100 are exercised regardless of payload content.

use libfuzzer_sys::fuzz_target;

use c0pl4nd_core::image::{decode_kitty, parse_kitty};

fuzz_target!(|data: &[u8]| {
    // --- parse_kitty: feed the raw bytes as the APC body. The parser tolerates
    // non-`;` bodies and arbitrary control text; it returns None only on invalid
    // UTF-8. We never unwrap into a panic.
    let _ = parse_kitty(data);

    // --- decode_kitty: derive (format, width, height) from the leading bytes so
    // every supported format path (f=24/32/100) is reached across the corpus,
    // then feed the remaining bytes as the already-base64-decoded `raw` payload.
    if data.len() < 5 {
        return;
    }
    // Select among the three SUPPORTED formats (and occasionally an unsupported
    // one, to exercise the `_ => None` honest-gap branch) from byte 0.
    let format: u16 = match data[0] % 4 {
        0 => 24,
        1 => 32,
        2 => 100,
        _ => data[0] as u16, // an arbitrary (often unsupported) format code
    };
    // Bound width/height so the checked-mul allocation path is exercised without
    // the fuzzer trying to materialise a multi-GiB buffer every iteration (which
    // would be an OOM in the harness, not a bug in the decoder). The decoder's
    // own `checked_mul` / length-mismatch guards are still the thing under test:
    // small dimensions still hit the `width*height*N != raw.len()` reject path
    // and, when they happen to match, the real expansion path.
    let width = u16::from_le_bytes([data[1], data[2]]) as usize % 256;
    let height = u16::from_le_bytes([data[3], data[4]]) as usize % 256;
    let raw = &data[5..];

    let _ = decode_kitty(format, width, height, raw);

    // Also drive the f=100 PNG path explicitly with the raw bytes as a candidate
    // PNG file (width/height are read from the PNG itself for f=100, so the
    // dimensions above are ignored on that path — this exercises the `image`
    // crate's PNG decoder on arbitrary bytes).
    let _ = decode_kitty(100, 0, 0, data);
});
