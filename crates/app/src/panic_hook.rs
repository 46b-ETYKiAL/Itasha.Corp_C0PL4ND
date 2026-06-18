//! Unexpected-panic crash diagnostics (finding F4-1).
//!
//! The workspace sets `panic = "abort"` (see `Cargo.toml`), so an *unexpected*
//! panic terminates the process immediately — the GUI window vanishes with zero
//! diagnostic. The reader thread and the failed-spawn paths already surface
//! their own errors; this module covers the residual UNEXPECTED-panic path.
//!
//! [`install`] registers a `std::panic::set_hook` early in `main` that, before
//! the abort fires:
//!
//! 1. Writes the panic message + location + a captured backtrace to a rotating
//!    crash log under the per-user `c0pl4nd` data dir (the same dir as
//!    `config.toml`), reusing the crash-safe [`c0pl4nd_core::atomic_write`]
//!    helper. The log is kept owner-only — a panic payload can contain
//!    user-environment fragments.
//! 2. On Windows, additionally shows a `MessageBoxW` so a user who launched the
//!    GUI (no console attached) is told the app crashed and where the log is.
//! 3. Chains to the previously-installed hook, so the default panic output (and
//!    any earlier custom hook) still runs.
//!
//! The hook composes with `panic = "abort"`: a panic hook runs *before* the
//! runtime aborts, so the report is always written first.

use std::backtrace::Backtrace;
use std::panic::PanicHookInfo;
use std::path::{Path, PathBuf};

/// How many `crash-NN.log` files to keep before recycling. Bounded so a crash
/// loop cannot fill the disk; old entries are overwritten round-robin.
const MAX_CRASH_LOGS: u32 = 5;

/// Install the crash-diagnostics panic hook. Call once, early in `main`.
///
/// Chains to any previously-installed hook so default panic output is
/// preserved. Safe to call before the window/event-loop is created.
pub fn install() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        // Resolve the crash-log directory; if we cannot (no config path), skip
        // the file write but still run the message box + the previous hook.
        if let Some(dir) = crash_log_dir() {
            let report = format_crash_report(info, &Backtrace::force_capture());
            if let Some(path) = write_crash_log(&dir, &report) {
                #[cfg(windows)]
                windows_msgbox::show_crash_dialog(&path);
                let _ = &path; // used only on Windows
            }
        }
        // W1TN3SS Tier-1: ALSO spool a sanitized, opt-in crash report locally so
        // the user can review + consent-send it on the NEXT launch (the consent
        // dialog drains the spool). Nothing transmits here — capture is
        // local-first, default-OFF, consent-gated. Only the panic's STATIC
        // `&'static str` message (a source-literal, e.g. an `expect("…")`
        // string) + our own panic SITE enter the report — a runtime `String`
        // payload (which could embed environment fragments / paths) is
        // deliberately NOT spooled. Best-effort; a spool failure in an
        // already-panicking thread is swallowed (never re-panics).
        capture_panic_w1tn3ss(info);
        // Always chain to the previous hook (default abort message, etc.).
        previous(info);
    }));
}

/// W1TN3SS Tier-1 capture: spool a sanitized, opt-in crash report from the
/// panic's STATIC message + our panic SITE via [`crate::reporting::capture_panic`].
///
/// Only a `&'static str` panic payload (a source-literal message, e.g. from
/// `panic!("lit")` / `expect("…")` / `unwrap()` — the latter's std message is a
/// `&'static str`) is spooled, honouring the SDK's static-message discipline: a
/// runtime `String` payload (from `panic!("{}", x)`) could embed environment
/// fragments or a path, so it is deliberately NOT spooled (only the static
/// shape + the location reaches the report). Best-effort: a non-static payload
/// or a spool failure is a no-op — the panic hook must never itself re-panic.
fn capture_panic_w1tn3ss(info: &PanicHookInfo<'_>) {
    // Only the `&'static str` arm is spooled (the static-message discipline).
    let Some(static_msg) = info.payload().downcast_ref::<&'static str>() else {
        return;
    };
    let location = info
        .location()
        .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
        .unwrap_or_else(|| "<unknown>".to_string());
    let _ = crate::reporting::capture_panic(static_msg, &location);
}

/// The directory crash logs are written to: a `crashes/` subdir of the per-user
/// `c0pl4nd` data dir (the parent of `config.toml`). Returns `None` when no
/// config path can be resolved (no `%APPDATA%` / `$HOME`).
pub fn crash_log_dir() -> Option<PathBuf> {
    c0pl4nd_core::Config::default_path()
        .and_then(|p| p.parent().map(Path::to_path_buf))
        .map(|d| d.join("crashes"))
}

/// Build the human-readable crash report from the panic info + backtrace.
///
/// Pure function (no I/O) so it is unit-testable with a synthetic payload. The
/// format is deterministic given its inputs: a fixed header, the panic payload,
/// the source location (when available), and the backtrace.
pub fn format_crash_report(info: &PanicHookInfo<'_>, backtrace: &Backtrace) -> String {
    let payload = panic_payload_str(info);
    let location = info
        .location()
        .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
        .unwrap_or_else(|| "<unknown>".to_string());

    format!(
        "C0PL4ND crash report\n\
         version: {} {}\n\
         os: {} {}\n\
         location: {}\n\
         message: {}\n\
         \n\
         backtrace:\n{}\n",
        c0pl4nd_core::PRODUCT_NAME,
        c0pl4nd_core::version(),
        std::env::consts::OS,
        std::env::consts::ARCH,
        location,
        payload,
        backtrace,
    )
}

/// Extract the panic payload as a string. `PanicHookInfo::payload()` is a
/// `&dyn Any`; the common shapes are `&str` (from `panic!("lit")`) and `String`
/// (from `panic!("{}", x)`). Anything else falls back to a placeholder.
fn panic_payload_str(info: &PanicHookInfo<'_>) -> String {
    let p = info.payload();
    if let Some(s) = p.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = p.downcast_ref::<String>() {
        s.clone()
    } else {
        "<non-string panic payload>".to_string()
    }
}

/// Write `report` to the next rotating `crash-NN.log` slot under `dir`, returning
/// the path written. Best-effort: returns `None` on any I/O failure (a crash
/// reporter must never itself panic or block the abort).
///
/// Rotation is wall-clock-independent (a panic hook cannot rely on a usable
/// clock): the slot index is `(highest existing index + 1) mod MAX_CRASH_LOGS`,
/// so the writer is deterministic and bounded without needing `SystemTime`.
pub fn write_crash_log(dir: &Path, report: &str) -> Option<PathBuf> {
    let slot = next_crash_slot(dir);
    let path = dir.join(format!("crash-{slot:02}.log"));
    // Owner-only: a panic payload + backtrace can leak environment fragments
    // (paths, usernames). Mirrors the workspace-state tightening.
    c0pl4nd_core::atomic_write::atomic_write_owner_only(&path, report.as_bytes()).ok()?;
    Some(path)
}

/// Choose the next rotating slot index in `[0, MAX_CRASH_LOGS)`.
///
/// Scans `dir` for existing `crash-NN.log` files and returns
/// `(max_index + 1) mod MAX_CRASH_LOGS`, or `0` when none exist (or the dir
/// cannot be read). This avoids any dependence on wall-clock time while keeping
/// the newest report distinguishable and the total bounded.
fn next_crash_slot(dir: &Path) -> u32 {
    let mut highest: Option<u32> = None;
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            if let Some(idx) = parse_crash_slot(&entry.file_name().to_string_lossy()) {
                highest = Some(highest.map_or(idx, |h| h.max(idx)));
            }
        }
    }
    match highest {
        Some(h) => (h + 1) % MAX_CRASH_LOGS,
        None => 0,
    }
}

/// Parse the slot index out of a `crash-NN.log` file name, or `None` if it does
/// not match that exact shape (or the index is out of range).
fn parse_crash_slot(name: &str) -> Option<u32> {
    let stem = name.strip_prefix("crash-")?.strip_suffix(".log")?;
    let idx: u32 = stem.parse().ok()?;
    (idx < MAX_CRASH_LOGS).then_some(idx)
}

/// Surface a FATAL STARTUP error that occurs before the egui window exists
/// (e.g. GPU adapter/device init failure, which `eframe::run_native` returns as
/// a clean `Err` — NOT a panic, so the panic hook never fires). A release GUI
/// build has no console, so without this a user just sees the window never
/// appear with zero explanation. Prints to stderr on every platform AND, on
/// Windows, shows a modal `MessageBox`. Best-effort; never panics.
pub fn show_startup_error(title: &str, body: &str) {
    eprintln!("{title}: {body}");
    #[cfg(windows)]
    windows_msgbox::show_dialog(title, body);
}

/// Windows-only `MessageBoxW` crash notification.
///
/// This is the second audited platform-FFI surface in this otherwise
/// unsafe-free binary (the first is `dll_hardening`). The single `MessageBoxW`
/// call is inherently `unsafe`, so the module opts back in with a
/// narrowly-scoped `#![allow(unsafe_code)]` + a `// SAFETY:` justification,
/// mirroring `dll_hardening.rs` exactly. The parent binary uses
/// `deny(unsafe_code)` (not `forbid`), so this scoped `allow` is permitted.
#[cfg(windows)]
mod windows_msgbox {
    #![allow(unsafe_code)]

    use std::path::Path;
    use windows::core::PCWSTR;
    use windows::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_OK};

    /// Show a modal "C0PL4ND crashed" dialog naming the crash-log path. Runs
    /// inside the panic hook (before abort), so it must not itself panic; any
    /// failure is swallowed.
    pub fn show_crash_dialog(log_path: &Path) {
        show_dialog(
            "C0PL4ND crashed",
            &format!(
                "C0PL4ND crashed unexpectedly.\n\nA crash log was written to:\n{}",
                log_path.display()
            ),
        );
    }

    /// Show a modal error dialog with an arbitrary `title` + `body`. Must not
    /// panic (callers run in pre-abort / pre-window contexts); any failure is
    /// swallowed.
    pub fn show_dialog(title: &str, body: &str) {
        let title = to_wide(title);
        let body = to_wide(body);

        // SAFETY: `MessageBoxW` is a user32 call taking an optional owner `HWND`
        // (we pass `None` for a top-level dialog), two NUL-terminated wide-string
        // pointers, and a `MESSAGEBOX_STYLE` flag set. `title` and `body` are
        // `Vec<u16>` buffers that include a trailing NUL and outlive the call;
        // the `PCWSTR`s point at their first element. The call has no
        // memory-safety preconditions beyond valid NUL-terminated pointers,
        // which are satisfied here. The return value is ignored — a failed
        // dialog must not block the caller.
        unsafe {
            MessageBoxW(
                None,
                PCWSTR(body.as_ptr()),
                PCWSTR(title.as_ptr()),
                MB_OK | MB_ICONERROR,
            );
        }
    }

    /// Encode a `&str` as a NUL-terminated UTF-16 buffer for the Win32 `W` API.
    fn to_wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a synthetic `PanicHookInfo` is not constructible outside std, so the
    /// writer + formatter are tested via their public, info-free seams: the pure
    /// report shape is exercised by writing a known report string and reading it
    /// back; the rotation logic is exercised directly.
    fn scratch_dir(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!("c0pl4nd-crash-test-{}-{}", std::process::id(), tag))
    }

    #[test]
    fn write_crash_log_writes_content_and_returns_path() {
        let dir = scratch_dir("write");
        let _ = std::fs::remove_dir_all(&dir);
        let report = "C0PL4ND crash report\nmessage: synthetic boom\n";
        let path = write_crash_log(&dir, report).expect("write should succeed");
        assert!(path.exists(), "crash log file must exist");
        let read = std::fs::read_to_string(&path).expect("read back");
        assert_eq!(read, report, "written content must round-trip exactly");
        assert_eq!(
            path.file_name().unwrap().to_string_lossy(),
            "crash-00.log",
            "first crash in an empty dir uses slot 0"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rotation_advances_and_wraps() {
        let dir = scratch_dir("rotate");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");

        // Empty dir → slot 0.
        assert_eq!(next_crash_slot(&dir), 0);

        // Seed crash-00..crash-04 (the full ring) → next wraps back to 0.
        for i in 0..MAX_CRASH_LOGS {
            std::fs::write(dir.join(format!("crash-{i:02}.log")), b"x").expect("seed");
        }
        assert_eq!(
            next_crash_slot(&dir),
            0,
            "highest index {} + 1 must wrap to 0",
            MAX_CRASH_LOGS - 1
        );

        // With only crash-02 present, next is 3.
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");
        std::fs::write(dir.join("crash-02.log"), b"x").expect("seed");
        assert_eq!(next_crash_slot(&dir), 3);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn parse_crash_slot_only_matches_exact_shape() {
        assert_eq!(parse_crash_slot("crash-00.log"), Some(0));
        assert_eq!(parse_crash_slot("crash-04.log"), Some(4));
        // Out of ring range.
        assert_eq!(parse_crash_slot("crash-99.log"), None);
        // Wrong shapes.
        assert_eq!(parse_crash_slot("crash-.log"), None);
        assert_eq!(parse_crash_slot("crash-00.txt"), None);
        assert_eq!(parse_crash_slot("notacrash.log"), None);
        assert_eq!(parse_crash_slot("config.toml"), None);
    }

    #[test]
    fn format_crash_report_includes_key_fields() {
        // We cannot construct a real `PanicHookInfo`, so exercise the formatter
        // by capturing inside an actual (caught) panic on a worker thread, where
        // the hook receives a genuine info. We assert the report names version,
        // os/arch, the panic message, and a backtrace section — without aborting
        // the test process.
        let captured = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
        let sink = captured.clone();
        // Install a temporary hook that formats into the sink, then restore.
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let bt = Backtrace::disabled();
            *sink.lock().unwrap() = format_crash_report(info, &bt);
        }));
        let _ = std::panic::catch_unwind(|| panic!("synthetic boom 42"));
        std::panic::set_hook(previous);

        let report = captured.lock().unwrap().clone();
        assert!(report.contains("C0PL4ND crash report"), "header: {report}");
        assert!(
            report.contains(c0pl4nd_core::version()),
            "version: {report}"
        );
        assert!(report.contains(std::env::consts::OS), "os: {report}");
        assert!(report.contains(std::env::consts::ARCH), "arch: {report}");
        assert!(report.contains("synthetic boom 42"), "message: {report}");
        assert!(report.contains("location:"), "location field: {report}");
        assert!(report.contains("backtrace:"), "backtrace section: {report}");
    }
}
