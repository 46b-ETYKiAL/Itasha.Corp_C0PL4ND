#![no_main]
//! Fuzz target for C0PL4ND's OSC (Operating System Command) handling.
//!
//! OSC sequences (`ESC ] … ST`/`BEL`) carry the highest-risk untrusted payloads
//! a terminal handles: clipboard writes (OSC 52, base64), color sets/queries
//! (OSC 4/10/11/12, X-style color specs), window/icon title (OSC 0/1/2), the
//! current-dir + semantic-prompt protocols (OSC 7/133), hyperlinks (OSC 8), and
//! desktop notifications (OSC 9/777). Every byte arrives over the PTY from **any**
//! program running in the terminal, so it is fully untrusted. A malformed or
//! hostile OSC string must never panic, hang, overflow, or OOB — a past CVE-class
//! bug in this exact surface (`#<invalid-utf8>` color spec slicing into a
//! multibyte codepoint, fixed in `osc::parse_color_spec`) is precisely what this
//! target guards against regressing.
//!
//! It drives BOTH layers of the OSC surface:
//!   1. The **standalone** `c0pl4nd_core::term::osc` helpers directly — the
//!      base64 codec (`base64_decode`/`base64_encode`, OSC 52) and the X-style
//!      color-spec parser/formatter (`parse_color_spec`/`format_color_reply`,
//!      OSC 4/10/11/12) — so the fuzzer reaches the byte-index/slice arithmetic
//!      without first having to synthesise a perfectly-framed escape sequence.
//!   2. The **integrated** dispatch path: the same bytes are wrapped into a real
//!      `ESC ] … BEL` OSC frame and pushed through `Terminal::advance`, so the
//!      parser's OSC accumulation, ST/BEL termination, and per-command dispatch
//!      (title, cwd, clipboard, color, notification, progress, prompt-mark) are
//!      exercised end-to-end. The drained public surface is then read back.

use libfuzzer_sys::fuzz_target;

use c0pl4nd_core::term::osc::{base64_decode, base64_encode, format_color_reply, parse_color_spec};
use c0pl4nd_core::Terminal;

fuzz_target!(|data: &[u8]| {
    // --- Layer 1: the standalone OSC helpers, fed raw bytes directly. ---

    // OSC 52 clipboard payloads are base64. Decode arbitrary bytes (whitespace
    // skipping, padding validation, 4-char chunk arithmetic) — never panic; and
    // re-encode whatever decoded to exercise the encode path too.
    if let Some(decoded) = base64_decode(data) {
        let _ = base64_encode(&decoded);
    }

    // The X-style color spec parser (OSC 4/10/11/12). Interpreting the bytes as
    // a spec string reaches the `rgb:`/`#` byte-slice arithmetic that once
    // panicked on a non-ASCII `#` form. Then format any parsed triple back.
    if let Ok(spec) = std::str::from_utf8(data) {
        if let Some(rgb) = parse_color_spec(spec) {
            let _ = format_color_reply(rgb);
        }
    }
    // Also drive the lossy form so a stray non-UTF-8 byte still reaches the
    // `#<U+FFFD>` slicing path (the exact regressed CVE-class shape).
    let lossy = String::from_utf8_lossy(data);
    let _ = parse_color_spec(&lossy);

    // --- Layer 2: the integrated OSC dispatch path through the parser. ---
    //
    // Frame the fuzzer bytes as a real OSC sequence: `ESC ] <payload> BEL`. This
    // exercises the parser's OSC string accumulation, BEL/ST termination, and
    // the per-Ps dispatch (title/cwd/clipboard/color/notify/progress/mark). The
    // payload bytes are inserted verbatim, so the fuzzer controls the OSC number,
    // the `;`-separated parameters, and the body.
    let mut term = Terminal::with_scrollback(24, 80, 200);
    let mut framed = Vec::with_capacity(data.len() + 3);
    framed.push(0x1b); // ESC
    framed.push(b']'); // OSC introducer
    framed.extend_from_slice(data);
    framed.push(0x07); // BEL terminator
    term.advance(&framed);

    // Drain the public OSC-derived surface so any inconsistency reachable only
    // through the post-dispatch state is caught (panics / OOB hunting).
    let _ = term.title();
    let _ = term.cwd();
    let _ = term.hyperlinks();
    let _ = term.prompt_marks();
});
