//! Performance smoke tests for the C0PL4ND core engine.
//!
//! These are NOT micro-benchmarks (the `throughput` Criterion bench owns
//! precise measurement). They are **regression tripwires**: each asserts a
//! GENEROUS upper bound — roughly 10x a comfortable local runtime — so a
//! pathological algorithmic regression (an accidental O(n²) in the hot path,
//! an unbounded allocation) fails the suite, while ordinary timing noise and
//! slow CI runners do not. Measured durations are printed via `eprintln!`
//! (visible with `cargo test -- --nocapture`) so a human can spot drift long
//! before the hard bound trips.

use std::time::Instant;

use c0pl4nd_core::search::{find, SearchOptions};
use c0pl4nd_core::term::osc::base64_encode;
use c0pl4nd_core::Terminal;

/// (a) Feed ~1 MB of mixed printable text + SGR colour changes through
/// `advance`. This is the hottest path in the engine (every PTY read lands
/// here). Bound: 2s (typical local run is well under 200ms).
#[test]
fn advance_one_megabyte_of_mixed_text_and_sgr() {
    // Build a ~1 MB buffer: lines of text interleaved with SGR colour toggles,
    // CR/LF, and an occasional cursor move — the shape of real colourful output.
    let mut buf: Vec<u8> = Vec::with_capacity(1_100_000);
    let palette: [&[u8]; 4] = [b"\x1b[31m", b"\x1b[32m", b"\x1b[36m", b"\x1b[0m"];
    let mut i = 0usize;
    while buf.len() < 1_000_000 {
        buf.extend_from_slice(palette[i % palette.len()]);
        buf.extend_from_slice(b"the operator's shell into the wired 0123456789");
        if i.is_multiple_of(3) {
            buf.extend_from_slice(b"\r\n");
        }
        i += 1;
    }
    let bytes = buf.len();

    let mut t = Terminal::with_scrollback(50, 200, 10_000);
    let start = Instant::now();
    t.advance(&buf);
    let elapsed = start.elapsed();
    eprintln!(
        "advance_one_megabyte: {bytes} bytes in {elapsed:?} \
         ({:.1} MB/s)",
        bytes as f64 / 1e6 / elapsed.as_secs_f64().max(1e-9)
    );

    assert!(
        elapsed.as_secs_f64() < 2.0,
        "feeding ~1MB through advance() took {elapsed:?}, over the 2s regression bound"
    );
    // Sanity: it actually did work (the grid is non-empty).
    assert!(!t.grid().to_text().trim().is_empty());
}

/// (b) Build `display_rows()` repeatedly over a deep (10k-line) scrollback.
/// `display_rows` runs every frame the user scrolls, so it must stay cheap.
/// Bound: 2s for 500 builds at various scroll offsets.
#[test]
fn display_rows_over_deep_scrollback_is_cheap() {
    let mut t = Terminal::with_scrollback(40, 120, 10_000);
    // Fill ~10k lines into history.
    let mut feed: Vec<u8> = Vec::with_capacity(400_000);
    for i in 0..10_000 {
        feed.extend_from_slice(format!("scrollback line {i} with some content\r\n").as_bytes());
    }
    t.advance(&feed);
    assert!(t.scrollback_len() >= 9_000, "expected a deep scrollback");

    let iterations = 500usize;
    let start = Instant::now();
    let mut total_rows = 0usize;
    for k in 0..iterations {
        // Vary the scroll offset across the run so we exercise the
        // history+grid splice path, not just the offset-0 fast case.
        t.set_view_offset((k * 17) % t.scrollback_len().max(1));
        let rows = t.display_rows();
        total_rows += rows.len();
    }
    let elapsed = start.elapsed();
    eprintln!(
        "display_rows x{iterations} over 10k scrollback: {elapsed:?} \
         ({total_rows} rows built)"
    );
    assert!(
        elapsed.as_secs_f64() < 2.0,
        "{iterations} display_rows() builds took {elapsed:?}, over the 2s regression bound"
    );
}

/// (c) Decode a moderately large Sixel image and a moderately large Kitty
/// image. Decoders run on untrusted PTY data, so a quadratic blowup here is a
/// DoS surface. Bound: 2s.
#[test]
fn decode_moderate_images_is_bounded() {
    // --- Sixel: ~200 columns wide, repeated full-height sixels (RLE) ---
    let mut sixel = Vec::new();
    sixel.extend_from_slice(b"\x1bPq#0;2;0;100;100"); // DCS q + cyan colour def
    sixel.extend_from_slice(b"!200~"); // RLE: 200 full sixel columns
    sixel.extend_from_slice(b"-"); // next band
    sixel.extend_from_slice(b"!200~");
    sixel.extend_from_slice(b"\x1b\\"); // ST

    let mut t = Terminal::new(40, 120);
    let start = Instant::now();
    t.advance(&sixel);
    let sixel_elapsed = start.elapsed();
    eprintln!("decode sixel (200-wide, 2 bands): {sixel_elapsed:?}");
    assert_eq!(t.images().len(), 1, "the sixel image should decode");

    // --- Kitty: a 64x64 RGBA image transmitted in one APC ---
    let (w, h) = (64usize, 64usize);
    let mut rgba = Vec::with_capacity(w * h * 4);
    for p in 0..(w * h) {
        rgba.extend_from_slice(&[(p & 0xff) as u8, 0, 128, 255]);
    }
    let payload = base64_encode(&rgba);
    let apc = format!("\x1b_Gf=32,s={w},v={h},a=T;{payload}\x1b\\");

    let mut t2 = Terminal::new(40, 120);
    let start = Instant::now();
    t2.advance(apc.as_bytes());
    let kitty_elapsed = start.elapsed();
    eprintln!("decode kitty (64x64 RGBA): {kitty_elapsed:?}");
    assert_eq!(t2.images().len(), 1, "the kitty image should decode");
    assert_eq!(t2.images()[0].image.width, w);
    assert_eq!(t2.images()[0].image.height, h);

    assert!(
        sixel_elapsed.as_secs_f64() < 2.0 && kitty_elapsed.as_secs_f64() < 2.0,
        "image decode exceeded the 2s regression bound (sixel {sixel_elapsed:?}, kitty {kitty_elapsed:?})"
    );
}

/// (d) Throughput over BiDi (RTL) + wide-char-heavy lines. The width-lookup +
/// combining-mark + continuation-cell path is per-grapheme work; feeding a lot
/// of it must stay linear. Bound: 2s.
#[test]
fn bidi_and_wide_char_heavy_throughput_is_bounded() {
    // A line mixing Arabic (RTL), CJK wide glyphs, and combining marks.
    // "مرحبا" (Arabic) + "世界" (CJK wide) + "é" as e+combining-acute.
    let unit = "مرحبا 世界 e\u{0301} ✨\u{FE0F} ";
    let mut feed = String::with_capacity(600_000);
    while feed.len() < 500_000 {
        feed.push_str(unit);
        // Hard newline every few units so we exercise newline + scroll too.
        if feed.len() % 97 < unit.len() {
            feed.push_str("\r\n");
        }
    }
    let bytes = feed.len();

    let mut t = Terminal::with_scrollback(40, 120, 5_000);
    let start = Instant::now();
    t.advance(feed.as_bytes());
    let elapsed = start.elapsed();
    eprintln!(
        "bidi+wide-char throughput: {bytes} bytes in {elapsed:?} \
         ({:.1} MB/s)",
        bytes as f64 / 1e6 / elapsed.as_secs_f64().max(1e-9)
    );
    assert!(
        elapsed.as_secs_f64() < 2.0,
        "BiDi/wide-char heavy feed took {elapsed:?}, over the 2s regression bound"
    );
    assert!(!t.grid().to_text().is_empty());
}

/// (e) Reflow stress: repeated resizes across a populated scrollback. Reflow is
/// the most algorithmically involved path (it reconstructs logical lines and
/// re-wraps them), so a regression here is the most likely O(n²) culprit.
/// `#[ignore]`d by default: a wall-clock bound is unreliable on shared CI
/// runners (observed 4.1s on a loaded Windows runner vs <2s locally), so it is
/// a local/manual regression tripwire, not a CI gate. Run with:
/// `cargo test -p c0pl4nd-core --release -- --ignored reflow_stress`.
#[test]
#[ignore = "wall-clock perf bound is variable on shared CI runners; run manually"]
fn reflow_stress_across_many_resizes_is_bounded() {
    let mut t = Terminal::with_scrollback(40, 120, 5_000);
    // Populate a few thousand soft-wrappable lines of varying length.
    let mut feed = String::with_capacity(300_000);
    for i in 0..3_000 {
        let len = 40 + (i % 160);
        let line: String = (0..len)
            .map(|j| char::from(b'a' + ((i + j) % 26) as u8))
            .collect();
        feed.push_str(&line);
        feed.push_str("\r\n");
    }
    t.advance(feed.as_bytes());

    let widths = [80usize, 200, 60, 160, 100, 240, 40, 120];
    let start = Instant::now();
    for (n, &w) in widths.iter().cycle().take(40).enumerate() {
        let rows = 30 + (n % 20);
        t.resize(rows, w);
    }
    let elapsed = start.elapsed();
    eprintln!("reflow stress (40 resizes over ~3k lines): {elapsed:?}");
    assert!(
        elapsed.as_secs_f64() < 3.0,
        "40 reflow resizes took {elapsed:?}, over the 3s regression bound"
    );
    // The engine is still coherent after the resize storm.
    assert!(t.grid().rows() >= 1 && t.grid().cols() >= 1);
}

/// (f) Search across a deep scrollback. `search::find` runs over `all_lines()`
/// on every query keystroke in the find bar, so it must stay linear in the
/// buffer size — a regex backtracking blowup or an accidental re-scan per line
/// would be a user-facing hang. Sized for a debug CI run: ~1.5k lines x 8 mixed
/// literal/regex/case-insensitive query-runs lands well under the 3s bound on a
/// healthy engine (regex compilation + scanning in a debug build dominates),
/// while an O(n^2) / catastrophic-backtracking regression blows far past it.
/// The 3s bound matches the reflow tripwire's allowance for debug + CI-runner
/// variability.
#[test]
fn search_over_deep_scrollback_is_bounded() {
    let mut t = Terminal::with_scrollback(40, 120, 2_000);
    let mut feed: Vec<u8> = Vec::with_capacity(120_000);
    for i in 0..1_500 {
        feed.extend_from_slice(
            format!("log line {i} status=ok user=operator path=/tmp/run-{i}.log\r\n").as_bytes(),
        );
    }
    t.advance(&feed);
    let lines = t.all_lines();
    assert!(lines.len() >= 1_300, "expected a deep scrollback to search");

    // A mix that exercises the literal-escape path, the (?i) case-insensitive
    // flag, a real regex with a quantifier, and a guaranteed-miss query.
    let queries: [(&str, SearchOptions); 4] = [
        ("operator", SearchOptions::default()),
        (
            "STATUS=OK",
            SearchOptions {
                regex: false,
                case_insensitive: true,
            },
        ),
        (
            r"line \d{3,}",
            SearchOptions {
                regex: true,
                case_insensitive: false,
            },
        ),
        ("needle-that-never-occurs", SearchOptions::default()),
    ];

    let iterations = 2usize;
    let start = Instant::now();
    let mut total_matches = 0usize;
    for _ in 0..iterations {
        for (q, opts) in queries.iter() {
            total_matches += find(&lines, q, *opts).len();
        }
    }
    let elapsed = start.elapsed();
    eprintln!(
        "search x{} over ~1.5k-line scrollback: {elapsed:?} ({total_matches} matches)",
        iterations * queries.len()
    );
    assert!(
        elapsed.as_secs_f64() < 3.0,
        "{} searches over scrollback took {elapsed:?}, over the 3s regression bound",
        iterations * queries.len()
    );
    // The literal + case-insensitive + regex queries all match the seeded lines;
    // a zero here means search silently stopped working (not a perf pass).
    assert!(
        total_matches > 0,
        "the matching queries returned nothing — search regressed, not just slow"
    );
}
