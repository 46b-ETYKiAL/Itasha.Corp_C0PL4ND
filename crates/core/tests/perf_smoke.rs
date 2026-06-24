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

use c0pl4nd_core::layout::{Axis, Direction, Layout, Rect, SplitOutcome};
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

/// (g) Reflow under repeated resize of a LARGE, populated grid. This is the CI
/// sibling of the `#[ignore]`d `reflow_stress` tripwire above: it uses a
/// smaller line count and FEWER resizes so a debug CI run lands comfortably
/// under a generous bound, while still exercising the full logical-line
/// reconstruction + re-wrap path that a quadratic regression would blow past.
/// Bound: 3s (a healthy debug run is well under 300ms), matching the search /
/// reflow bound for debug + shared-runner variability. Each resize forces a
/// complete reflow of the populated scrollback, so an accidental O(n²) wrap
/// (e.g. re-scanning the whole history per row) trips the bound.
#[test]
fn reflow_under_resize_of_large_grid_is_bounded() {
    let mut t = Terminal::with_scrollback(40, 120, 4_000);
    // ~2k soft-wrappable lines of varying length so reflow has real work to do
    // (short lines, full-width lines, and over-width lines that wrap).
    let mut feed = String::with_capacity(200_000);
    for i in 0..2_000 {
        let len = 20 + (i % 180);
        let line: String = (0..len)
            .map(|j| char::from(b'a' + ((i + j) % 26) as u8))
            .collect();
        feed.push_str(&line);
        feed.push_str("\r\n");
    }
    t.advance(feed.as_bytes());
    assert!(
        t.scrollback_len() >= 1_500,
        "expected a deep scrollback to reflow"
    );

    // Cycle through a spread of widths: each transition re-wraps every logical
    // line. 24 resizes is enough to surface a per-resize quadratic.
    let widths = [80usize, 200, 60, 160, 100, 40];
    let start = Instant::now();
    for (n, &w) in widths.iter().cycle().take(24).enumerate() {
        let rows = 24 + (n % 24);
        t.resize(rows, w);
    }
    let elapsed = start.elapsed();
    eprintln!("reflow under resize (24 resizes over ~2k lines): {elapsed:?}");
    assert!(
        elapsed.as_secs_f64() < 3.0,
        "24 reflow resizes over a large grid took {elapsed:?}, over the 3s regression bound"
    );
    // The engine is still coherent after the resize storm.
    assert!(
        t.grid().rows() >= 1 && t.grid().cols() >= 1,
        "grid dimensions degenerate after reflow storm"
    );
}

/// (h) Grid snapshot churn. Rendering the visible grid to a reviewable text +
/// attribute snapshot (the shape used by `grid_snapshot.rs` reference tests and
/// by any serialize/copy-all path) walks every cell of every visible row. It
/// runs whenever the grid is captured, so it must stay linear in the cell
/// count. Bound: 2s for 2000 full-grid snapshots of a 50×200 grid (a healthy
/// run is well under 200ms); a per-cell allocation regression or an accidental
/// re-walk would blow past it.
#[test]
fn grid_snapshot_churn_is_bounded() {
    let mut t = Terminal::with_scrollback(50, 200, 1_000);
    // Fill the visible grid with mixed content + attributes so the snapshot
    // does real per-cell flag inspection, not just blank-cell fast paths.
    let mut feed: Vec<u8> = Vec::new();
    for r in 0..50 {
        feed.extend_from_slice(b"\x1b[1;33m"); // bold yellow
        feed.extend_from_slice(format!("row {r}: ").as_bytes());
        feed.extend_from_slice(b"\x1b[0;7m"); // reset + reverse
        feed.extend_from_slice(b"the wired operator stares back 0123456789 ");
        feed.extend_from_slice(b"\x1b[0m\r\n");
    }
    t.advance(&feed);

    let iterations = 2_000usize;
    let start = Instant::now();
    let mut total_cells = 0usize;
    for _ in 0..iterations {
        let grid = t.grid();
        let mut snap = String::new();
        for r in 0..grid.rows() {
            let cells = grid.row(r);
            for c in cells {
                // Touch the glyph + a couple of flags so the optimiser cannot
                // elide the walk: this mirrors a real snapshot serialiser.
                snap.push(c.c);
                if c.flags.bold {
                    snap.push('b');
                }
                if c.flags.inverse {
                    snap.push('r');
                }
                total_cells += 1;
            }
            snap.push('\n');
        }
        // Keep the result observable so the loop body is not dead code.
        assert!(!snap.is_empty());
    }
    let elapsed = start.elapsed();
    eprintln!(
        "grid snapshot x{iterations} of 50x200 grid: {elapsed:?} ({total_cells} cells walked)"
    );
    assert!(
        elapsed.as_secs_f64() < 2.0,
        "{iterations} grid snapshots took {elapsed:?}, over the 2s regression bound"
    );
    assert_eq!(
        total_cells,
        iterations * 50 * 200,
        "snapshot walked the wrong number of cells — grid geometry regressed"
    );
}

/// (i) Layout geometry churn. `Layout::cascade` recomputes every leaf's
/// absolute rectangle from the split tree; a renderer recomputes it on every
/// layout change, and the action ops (split / equalize / swap / zoom) mutate
/// the tree. A regression that turns the tree walk superlinear in the leaf
/// count (or that reallocates per call) would stutter the UI. Bound: 2s for
/// 100k mixed cascade + action ops over a multi-pane tree (a healthy run is
/// sub-50ms); generous enough to never flake, tight enough to catch a blowup.
#[test]
fn layout_ops_churn_is_bounded() {
    let window = Rect {
        x: 0,
        y: 0,
        w: 1920,
        h: 1080,
    };
    // Build a 6-leaf tree (alternating split axes) — a realistic dense layout.
    let mut l = Layout::new();
    let mut axis = Axis::Horizontal;
    while l.leaf_count() < 6 {
        let target = l.focused;
        match l.try_split(target, axis) {
            SplitOutcome::Split(id) => l.focused = id,
            other => panic!("split rejected at {} leaves: {other:?}", l.leaf_count()),
        }
        axis = axis.opposite();
    }
    assert_eq!(l.leaf_count(), 6, "expected a 6-pane tree to exercise");

    let iterations = 100_000usize;
    let start = Instant::now();
    let mut total_rects = 0usize;
    for k in 0..iterations {
        // Recompute geometry (the per-frame hot op).
        let rects = l.cascade(window);
        total_rects += rects.len();
        // Periodically mutate the tree so the cascade is not over a frozen
        // shape: equalize, a directional swap, and a zoom toggle round-trip.
        if k % 4 == 0 {
            l.equalize();
        } else if k % 4 == 1 {
            let _ = l.swap_focused(Direction::Right, window);
        } else if k % 4 == 2 {
            l.toggle_zoom();
        }
    }
    let elapsed = start.elapsed();
    eprintln!(
        "layout ops x{iterations} over a 6-pane tree: {elapsed:?} ({total_rects} rects computed)"
    );
    assert!(
        elapsed.as_secs_f64() < 2.0,
        "{iterations} layout ops took {elapsed:?}, over the 2s regression bound"
    );
    // cascade always yields exactly one rect per leaf; a zero or wrong count
    // means the geometry pass regressed, not just slowed.
    assert!(
        total_rects > 0,
        "cascade returned no rectangles — layout geometry regressed"
    );
}

/// (j) Scrollback churn. Pushing many short lines drives the
/// newline -> scroll-into-history path: each line that scrolls off the top is
/// ring-buffered into `history` (with its wrap flag), and the buffer is capped
/// to `max_scrollback`. This is the steady-state cost of a chatty process
/// (a build log, a `yes` loop), so it must stay linear in the line count and
/// must not grow the history past its cap. Bound: 3s for ~80k pushed lines
/// (a healthy run is well under 300ms), matching the search / reflow tests'
/// generous bound so it stays green under llvm-cov instrumentation (~7x slower)
/// and on noisy shared CI runners; an O(n) per-push history scan or an
/// unbounded buffer would blow the bound or the cap assertion regardless.
#[test]
fn scrollback_churn_is_bounded() {
    let max_scrollback = 5_000usize;
    let mut t = Terminal::with_scrollback(30, 80, max_scrollback);

    // Pre-build the feed so the timed section measures the engine, not the
    // string formatting. ~80k short lines, 16x the scrollback cap, so the ring
    // buffer churns continuously (push + pop_front per line).
    let line_count = 80_000usize;
    let mut feed: Vec<u8> = Vec::with_capacity(line_count * 24);
    for i in 0..line_count {
        feed.extend_from_slice(format!("build step {i} :: ok\r\n").as_bytes());
    }

    let start = Instant::now();
    t.advance(&feed);
    let elapsed = start.elapsed();
    eprintln!("scrollback churn ({line_count} lines, cap {max_scrollback}): {elapsed:?}");
    assert!(
        elapsed.as_secs_f64() < 3.0,
        "pushing {line_count} lines took {elapsed:?}, over the 3s regression bound"
    );
    // The ring buffer must be capped — an unbounded grow is a memory-DoS
    // regression, not a perf-only one.
    assert!(
        t.scrollback_len() <= max_scrollback,
        "scrollback ({}) exceeded its cap ({max_scrollback}) — ring buffer regressed",
        t.scrollback_len()
    );
    // And it must have actually filled (the churn did work).
    assert!(
        t.scrollback_len() >= max_scrollback - 30,
        "scrollback ({}) far below cap — lines were lost, not just slow",
        t.scrollback_len()
    );
}
