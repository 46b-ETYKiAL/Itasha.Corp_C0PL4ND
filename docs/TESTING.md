# Testing

C0PL4ND is tested with the full best-in-class taxonomy for a terminal emulator.
This page is the map: every relevant test *type*, what it covers, and how to run
it. The two deterministic crates (`c0pl4nd-core`, `c0pl4nd-renderer`) are headless;
the `c0pl4nd` egui shell is exercised headed via `egui_kittest`.

## Test types at a glance

| Type | Where | What it guards | Run |
|---|---|---|---|
| **Unit** | `#[cfg(test)]` in every `src` module (core ≈ 450, app ≈ 235) | per-function logic, with access to private internals | `cargo test --lib` |
| **Integration** | `crates/core/tests/*.rs` | cross-module behaviour against real deps (real PTY in `e2e_terminal.rs`) | `cargo test --test '*'` |
| **End-to-end / UI** | `crates/app/tests/egui_*.rs` (egui_kittest) | the real frame loop driven by simulated input; the "typing reaches the PTY and the grid updates" class | `cargo test -p c0pl4nd` |
| **Accessibility** | egui_kittest `query_by_label*` + the AccessKit grid node | screen-reader exposure of chrome + terminal grid | part of the egui_* suites |
| **Property-based** | `crates/core/tests/property_tests.rs` (proptest) | parser invariants for *arbitrary* input: no panic, grid geometry preserved, cursor in bounds, **chunk-invariance** of valid VT streams across read boundaries, SGR-reset isolation | `cargo test --test property_tests` |
| **Snapshot / reference** | `crates/core/tests/grid_snapshot.rs` | the *whole* grid (text + attributes) after a script vs a known-good golden — catches an unexpected change anywhere (Alacritty `ref.rs` pattern) | `cargo test --test grid_snapshot` |
| **Fuzzing** | `fuzz/fuzz_targets/*` (cargo-fuzz) | the VT parser, sixel/kitty decoders, archive + state JSON against adversarial bytes | `cargo +nightly fuzz run vt_parser` |
| **Benchmark / performance** | `crates/core/benches/*` (criterion) + `tests/perf_smoke.rs` | parser throughput + snapshot cost, with a smoke floor in CI | `cargo bench` |
| **Mutation** | `mutants.toml` + `.github/workflows/mutants.yml` (cargo-mutants) | *test effectiveness* — injects code changes and checks a test catches each; diff-scoped on PRs | `cargo mutants` |
| **Coverage** | `.github/workflows/coverage.yml` (cargo-llvm-cov) | line/region coverage of the core engine, **gated** so it cannot regress | `cargo llvm-cov -p c0pl4nd-core` |

## VT conformance

The VT100/VT220/xterm feature surface (SGR, cursor addressing, scroll regions,
erase, tabs, autowrap, DEC private modes, alt-screen, charsets, OSC, mouse,
bracketed paste, kitty keyboard, wide/combining glyphs) is covered by the
example-based unit tests in `crates/core/src/term/tests.rs` and the real-PTY
integration tests in `crates/core/tests/e2e_terminal.rs`. The property and
snapshot suites cover the complementary "arbitrary input" and "whole-grid
golden" angles. The de-facto external corpora ([esctest], [vttest]) are the
reference standards these tests are written against.

## Coverage gate

`coverage.yml` runs `cargo llvm-cov -p c0pl4nd-core --fail-under-lines 88`.
Core line coverage was ~91% when the gate was introduced; the 88% floor leaves a
small ratchet of headroom while failing on a real regression. The egui shell is
not in the coverage number (it needs a GPU/display); it is covered by the headed
egui_kittest suites in the Build & Test matrix instead.

## Running everything locally

```sh
cargo test --workspace                 # unit + integration + e2e + property + snapshot
cargo llvm-cov -p c0pl4nd-core         # coverage report
cargo mutants                          # mutation sweep (slow; CI runs it diff-scoped)
cargo +nightly fuzz run vt_parser      # fuzzing (nightly)
cargo bench                            # benchmarks
```

[esctest]: https://github.com/ThomasDickey/esctest2
[vttest]: https://invisible-island.net/vttest/
