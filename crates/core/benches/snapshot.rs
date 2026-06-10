//! Snapshot / scroll / contention benchmarks.
//!
//! `throughput.rs` measures the parse (`advance`) path; this file measures the
//! READ side that the egui paint loop drives every frame: pulling the visible
//! rows out of the terminal (`display_rows` vs the zero-alloc `for_visible_rows`
//! iterator), scrolling through scrollback, and the lock-contention cost of one
//! thread snapshotting while another keeps parsing. Run with
//! `cargo bench -p c0pl4nd-core --bench snapshot`.

use std::hint::black_box;
use std::sync::{Arc, Mutex};
use std::thread;

use c0pl4nd_core::Terminal;
use criterion::{criterion_group, criterion_main, Criterion};

/// Mixed plain text + SGR colour sequences, like real program output.
fn make_payload(lines: usize) -> Vec<u8> {
    let mut out = Vec::new();
    for i in 0..lines {
        out.extend_from_slice(b"\x1b[32m[ok]\x1b[0m processing item ");
        out.extend_from_slice(i.to_string().as_bytes());
        out.extend_from_slice(b" \x1b[38;2;0;229;255mc0pl4nd\x1b[0m\r\n");
    }
    out
}

/// A 40x120 terminal with `scrollback` lines of history already accumulated and
/// the view following the live bottom (`view_offset == 0`).
fn make_terminal(scrollback_lines: usize) -> Terminal {
    let mut term = Terminal::with_scrollback(40, 120, 10_000);
    term.advance(&make_payload(scrollback_lines));
    term
}

/// The per-frame visible-row read. Compares the allocating `display_rows()`
/// (clones the whole visible grid into `Vec<Vec<Cell>>`) against the borrowing
/// `for_visible_rows()` (walks history + grid in place, zero allocation) — the
/// D2 (PERF-2) target. The closure mirrors the real consumer's work: a cheap
/// per-cell touch so the read is not optimised away.
fn bench_display_rows(c: &mut Criterion) {
    let term = make_terminal(1_000);
    let mut group = c.benchmark_group("display_rows");

    group.bench_function("display_rows_clone", |b| {
        b.iter(|| {
            let rows = term.display_rows();
            let mut acc = 0u32;
            for row in &rows {
                for cell in row {
                    acc = acc.wrapping_add(cell.c as u32);
                }
            }
            black_box(acc)
        });
    });

    group.bench_function("for_visible_rows_borrow", |b| {
        b.iter(|| {
            let mut acc = 0u32;
            term.for_visible_rows(|_, row| {
                for cell in row {
                    acc = acc.wrapping_add(cell.c as u32);
                }
            });
            black_box(acc)
        });
    });

    group.finish();
}

/// Scroll the view up and down through a full scrollback. A scroll op only
/// shifts `view_offset` + touches the grid, but it is the gesture that drives
/// the visible-row read above, so we measure it end-to-end (scroll + snapshot).
fn bench_scroll(c: &mut Criterion) {
    let mut group = c.benchmark_group("scroll");
    group.bench_function("scroll_up_then_snapshot", |b| {
        let mut term = make_terminal(5_000);
        b.iter(|| {
            term.scroll_up_view(black_box(1));
            let mut acc = 0u32;
            term.for_visible_rows(|_, row| {
                for cell in row {
                    acc = acc.wrapping_add(cell.c as u32);
                }
            });
            if term.view_offset() >= term.scrollback_len() {
                term.scroll_to_bottom();
            }
            black_box(acc)
        });
    });
    group.finish();
}

/// Lock-contention cost: a reader thread `advance`-ing under `Mutex<Terminal>`
/// while the bench thread snapshots the visible rows. The terminal is not
/// `Mutex`-wrapped inside core (the app owns that seam — `Session::terminal()`
/// returns `Arc<Mutex<Terminal>>`), so the bench wraps a local `Mutex` to model
/// the same contention the egui paint loop sees against the PTY reader thread.
fn bench_contended_advance(c: &mut Criterion) {
    let mut group = c.benchmark_group("contended");
    group.bench_function("snapshot_under_writer", |b| {
        let term = Arc::new(Mutex::new(make_terminal(1_000)));
        let writer_term = Arc::clone(&term);
        let stop = Arc::new(Mutex::new(false));
        let writer_stop = Arc::clone(&stop);

        // Background writer: keeps locking + advancing a small chunk, modelling
        // the PTY reader thread feeding bytes while the UI snapshots.
        let writer = thread::spawn(move || {
            let chunk = make_payload(8);
            loop {
                {
                    if *writer_stop.lock().unwrap() {
                        break;
                    }
                    writer_term.lock().unwrap().advance(&chunk);
                }
                thread::yield_now();
            }
        });

        b.iter(|| {
            let guard = term.lock().unwrap();
            let mut acc = 0u32;
            guard.for_visible_rows(|_, row| {
                for cell in row {
                    acc = acc.wrapping_add(cell.c as u32);
                }
            });
            black_box(acc)
        });

        *stop.lock().unwrap() = true;
        writer.join().unwrap();
    });
    group.finish();
}

criterion_group!(
    benches,
    bench_display_rows,
    bench_scroll,
    bench_contended_advance
);
criterion_main!(benches);
