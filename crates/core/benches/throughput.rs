//! VT-parse throughput benchmark.
//!
//! Measures how fast the VT engine ingests shell output (the "cat a large
//! file" workload). Research target: Ghostty-class >100 MB/s; Alacritty/iTerm2
//! are the reference tier. Run with `cargo bench -p c0pl4nd-core`.

use std::hint::black_box;

use c0pl4nd_core::Terminal;
use criterion::{criterion_group, criterion_main, Criterion, Throughput};

fn make_payload(lines: usize) -> Vec<u8> {
    // Mixed plain text + SGR colour sequences, like real program output.
    let mut out = Vec::new();
    for i in 0..lines {
        out.extend_from_slice(b"\x1b[32m[ok]\x1b[0m processing item ");
        out.extend_from_slice(i.to_string().as_bytes());
        out.extend_from_slice(b" \x1b[38;2;0;229;255mc0pl4nd\x1b[0m\r\n");
    }
    out
}

fn bench_throughput(c: &mut Criterion) {
    let payload = make_payload(50_000);
    let mut group = c.benchmark_group("vt_parse");
    group.throughput(Throughput::Bytes(payload.len() as u64));
    group.bench_function("advance_50k_lines", |b| {
        b.iter(|| {
            let mut term = Terminal::with_scrollback(40, 120, 10_000);
            term.advance(black_box(&payload));
            black_box(term.grid().rows());
        });
    });
    group.finish();
}

criterion_group!(benches, bench_throughput);
criterion_main!(benches);
