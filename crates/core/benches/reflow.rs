//! Grid reflow (resize) benchmark.
//!
//! `throughput.rs` measures the parse path and `snapshot.rs` the per-frame read
//! path; this file measures REFLOW — rewrapping the scrollback + visible grid to
//! a new column width. It is the expensive path on a window resize or a
//! font-size change, and (unlike parse/read) it touches every buffered line.
//! Run with `cargo bench -p c0pl4nd-core --bench reflow`.

use std::hint::black_box;

use c0pl4nd_core::Terminal;
use criterion::{criterion_group, criterion_main, BatchSize, Criterion};

/// Mixed plain text + SGR colour sequences, like real program output. Lines are
/// long enough that a narrowing reflow must wrap them (exercising the rewrap).
fn make_payload(lines: usize) -> Vec<u8> {
    let mut out = Vec::new();
    for i in 0..lines {
        out.extend_from_slice(b"\x1b[32m[ok]\x1b[0m processing item ");
        out.extend_from_slice(i.to_string().as_bytes());
        out.extend_from_slice(
            b" with a deliberately long trailing description \x1b[38;2;0;229;255mc0pl4nd\x1b[0m\r\n",
        );
    }
    out
}

fn bench_reflow(c: &mut Criterion) {
    let payload = make_payload(20_000);
    let mut group = c.benchmark_group("grid_reflow");
    // Narrow (forces a rewrap of every buffered line) then widen back — the
    // round trip a user makes dragging a window edge. `iter_batched` rebuilds a
    // fresh, already-populated terminal per iteration so the one-time `advance`
    // cost is NOT folded into the measured reflow time.
    group.bench_function("resize_120_to_80_to_120", |b| {
        b.iter_batched(
            || {
                let mut t = Terminal::with_scrollback(40, 120, 25_000);
                t.advance(&payload);
                t
            },
            |mut t| {
                t.resize(40, 80);
                t.resize(40, 120);
                black_box(t.grid().cols());
            },
            BatchSize::SmallInput,
        );
    });
    group.finish();
}

criterion_group!(benches, bench_reflow);
criterion_main!(benches);
