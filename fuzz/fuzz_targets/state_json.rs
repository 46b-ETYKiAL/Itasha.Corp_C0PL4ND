#![no_main]
//! Fuzz target for C0PL4ND's persisted-state JSON parser/validator.
//!
//! The window/pane layout and multi-tab workspace are serialised to JSON on
//! disk (eframe `persistence`). On startup that JSON is read back and parsed by
//! `c0pl4nd_core::layout_persist`. A persisted-state file is **untrusted at
//! rest**: it can be hand-edited, corrupted by a crash, truncated by a full
//! disk, or tampered with by another process. A malformed snapshot must never
//! crash, panic, hang, overflow, or build a degenerate layout tree — the loader
//! is documented to "fall back" on bad input, never to abort.
//!
//! This target drives both public parse entry points:
//!   * `WorkspaceSnapshot::from_json` — the multi-tab wrapper, including the
//!     v1 → v2 migration shim (it first tries the wrapper, then falls back to a
//!     bare single-tab `LayoutSnapshot`), then `validate_and_normalize`.
//!   * `LayoutSnapshot::from_json` — the v1 single-tab format, then `validate`.
//!
//! On any snapshot that parses, it also exercises the reconstruction paths
//! (`restore_all` / `restore`) which walk the layout tree, allocate fresh ids,
//! count leaves and enforce the `MAX_PANES` cap — the logic most likely to hit
//! an arithmetic/recursion edge on a hostile tree shape.
//!
//! Both `arbitrary`-mutated *raw bytes interpreted as UTF-8* and the same bytes
//! lossily coerced to a `String` are fed in, so the fuzzer reaches both the
//! "not valid UTF-8 / not valid JSON" rejection path and the "valid JSON, weird
//! structure" validation path.

use libfuzzer_sys::fuzz_target;

use c0pl4nd_core::layout_persist::{LayoutSnapshot, WorkspaceSnapshot};

fuzz_target!(|data: &[u8]| {
    // Path A: only feed inputs that are already valid UTF-8 to serde_json so the
    // fuzzer spends its energy on JSON *structure* rather than the trivial
    // "invalid UTF-8 -> parse error" rejection. serde_json itself is fuzzed
    // upstream; we care about C0PL4ND's validation + reconstruction on top.
    if let Ok(src) = std::str::from_utf8(data) {
        // The multi-tab wrapper entry point (also covers the v1 single-tab
        // fallback + the unsupported-version reject branch internally).
        if let Ok(ws) = WorkspaceSnapshot::from_json(src) {
            // Reconstruct every tab: walks the tree, allocates ids, counts
            // leaves, clamps the active index. Panic / overflow hunting.
            let _ = ws.restore_all();
        }

        // The bare single-tab v1 entry point, driven directly so the fuzzer
        // does not have to discover the wrapper-vs-bare disambiguation to reach
        // it.
        if let Ok(layout) = LayoutSnapshot::from_json(src) {
            let _ = layout.validate();
            let _ = layout.restore();
        }
    }

    // Path B: also feed the raw bytes lossily coerced to UTF-8, so a single
    // stray invalid byte in an otherwise-structured payload still reaches the
    // JSON parser (the realistic "one corrupted byte mid-file" case).
    let lossy = String::from_utf8_lossy(data);
    let _ = WorkspaceSnapshot::from_json(&lossy);
    let _ = LayoutSnapshot::from_json(&lossy);
});
