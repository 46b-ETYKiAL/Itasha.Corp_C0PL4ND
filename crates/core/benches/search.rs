//! Search data-prep benchmark.
//!
//! The in-buffer find pulls every buffered line as text (`all_lines`) and scans
//! it; on a deep scrollback that allocation + scan is the cost the find overlay
//! pays per query. `throughput.rs`/`snapshot.rs` do not cover it. Run with
//! `cargo bench -p c0pl4nd-core --bench search`.

use std::hint::black_box;

use c0pl4nd_core::Terminal;
use criterion::{criterion_group, criterion_main, Criterion};

fn make_payload(lines: usize) -> Vec<u8> {
    let mut out = Vec::new();
    for i in 0..lines {
        out.extend_from_slice(b"\x1b[32m[ok]\x1b[0m processing item ");
        out.extend_from_slice(i.to_string().as_bytes());
        out.extend_from_slice(b" \x1b[38;2;0;229;255mc0pl4nd\x1b[0m\r\n");
    }
    out
}

fn bench_search(c: &mut Criterion) {
    let payload = make_payload(20_000);
    let mut term = Terminal::with_scrollback(40, 120, 25_000);
    term.advance(&payload);

    let mut group = c.benchmark_group("search");
    // The data-prep allocation: every buffered line as an owned String.
    group.bench_function("all_lines_20k", |b| {
        b.iter(|| black_box(term.all_lines()));
    });
    // A representative query: count the lines containing a needle. This mirrors
    // what the find overlay does each keystroke over the full scrollback.
    group.bench_function("scan_substring_20k", |b| {
        b.iter(|| {
            let lines = term.all_lines();
            let hits = lines
                .iter()
                .filter(|l| l.contains(black_box("c0pl4nd")))
                .count();
            black_box(hits)
        });
    });
    group.finish();
}

criterion_group!(benches, bench_search);
criterion_main!(benches);
