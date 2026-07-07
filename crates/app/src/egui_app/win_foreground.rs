//! Bring the app window to the foreground on FIRST launch (Windows 11
//! foreground-lock backstop).
//!
//! ## Why this exists
//!
//! `ViewportBuilder::with_active(true)` (see `egui_main.rs`) plus a one-shot
//! `ViewportCommand::Focus` on the first frame ask winit/the OS to focus the
//! window. On Windows 11 that is frequently ignored: the OS *foreground lock*
//! (`SPI_SETFOREGROUNDLOCKTIMEOUT`) refuses a raw `SetForegroundWindow` from a
//! process that does not currently own the foreground, so a freshly-launched
//! app can open BEHIND the window the user was last in — the reported "opens in
//! the background" bug.
//!
//! ## The workaround (research §4)
//!
//! Temporarily attach THIS thread's input queue to the current foreground
//! window's thread (`AttachThreadInput`). While attached the two threads share
//! one input state, so Windows treats our `SetForegroundWindow` as coming from
//! the foreground thread itself and honours it. We then `ShowWindow` +
//! `BringWindowToTop` + `SetForegroundWindow` our own window and detach again.
//!
//! ## Discipline
//!
//! This runs **exactly once**, on the first frame of initial launch (the caller
//! gates it behind a one-shot flag). It NEVER runs on later frames, so it can
//! never steal focus back from another app the user has since switched to. It
//! also never lowers the global `SPI_SETFOREGROUNDLOCKTIMEOUT` (a system-wide
//! side effect). Best-effort and non-fatal: every call is a no-op if the window
//! handle was not primed, and no failure is surfaced.
//!
//! Windows-only; every entry point is a no-op elsewhere. Lives as an `egui_app`
//! submodule (not a crate-root module) so the `#[path=…]`-included test
//! harnesses resolve it without re-declaring it.

/// Prime the module with THIS process's main window handle, taken from eframe's
/// `CreationContext` (same handle `caption_close` uses). Idempotent; a zero
/// handle is ignored. Windows-only.
#[cfg(windows)]
pub(crate) fn set_main_hwnd(hwnd: isize) {
    imp::set_main_hwnd(hwnd);
}

/// No-op on non-Windows platforms. The sole caller (in `mod.rs`) is itself
/// `#[cfg(windows)]`-gated because it reads the Win32 raw window handle, so this
/// stub is never called off-Windows — `allow(dead_code)` keeps the symmetric
/// no-op API surface without tripping the `-D warnings` CI build.
#[cfg(not(windows))]
#[allow(dead_code)]
pub(crate) fn set_main_hwnd(_hwnd: isize) {}

/// Nudge the primed main window to the foreground ONCE (see module docs). The
/// caller must invoke this at most once per launch (it is gated behind a
/// one-shot flag in `frame_tick`). No-op before the HWND is primed, and a no-op
/// on non-Windows platforms.
pub(crate) fn force_foreground_main() {
    #[cfg(windows)]
    imp::force_foreground_main();
}

#[cfg(windows)]
mod imp {
    // The audited Win32 FFI is quarantined here with `// SAFETY:` justifications,
    // mirroring the other `#[cfg(windows)]` modules (caption_close, job_object,
    // dll_hardening).
    #![allow(unsafe_code)]

    use std::sync::atomic::{AtomicIsize, Ordering};

    use windows::Win32::Foundation::HWND;
    // AttachThreadInput + GetCurrentThreadId live under System::Threading in the
    // `windows` crate (already an enabled feature for job_object's OpenProcess).
    use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
    use windows::Win32::UI::WindowsAndMessaging::{
        BringWindowToTop, GetForegroundWindow, GetWindowThreadProcessId, SetForegroundWindow,
        ShowWindow, SW_SHOW,
    };

    /// Cached main-window HWND (0 = not yet primed). C0PL4ND uses ONE OS window.
    static CACHED_HWND: AtomicIsize = AtomicIsize::new(0);

    pub fn set_main_hwnd(hwnd: isize) {
        if hwnd != 0 {
            CACHED_HWND.store(hwnd, Ordering::Relaxed);
        }
    }

    pub fn force_foreground_main() {
        let hwnd = CACHED_HWND.load(Ordering::Relaxed);
        if hwnd == 0 {
            return; // not primed (or non-window build) — nothing to raise.
        }
        force_foreground(hwnd);
    }

    /// The AttachThreadInput foreground dance (research §4). Best-effort: if the
    /// attach is refused we still issue the raise calls (they simply may not beat
    /// the foreground lock).
    fn force_foreground(hwnd: isize) {
        let target = HWND(hwnd as *mut core::ffi::c_void);

        // SAFETY: `GetForegroundWindow` borrows nothing; it returns the current
        // foreground window handle (possibly null) by value.
        let fg = unsafe { GetForegroundWindow() };
        // SAFETY: reads the owning thread id of `fg` (0 for a null handle); the
        // process-id out-param is `None`, so nothing is written back.
        let fg_thread = unsafe { GetWindowThreadProcessId(fg, None) };
        // SAFETY: returns THIS thread's id; borrows no memory.
        let our_thread = unsafe { GetCurrentThreadId() };

        // Attach our input queue to the foreground thread's so Win11 honours the
        // SetForegroundWindow below. Only when the foreground belongs to a
        // DIFFERENT, known thread — attaching a thread to itself is invalid, and
        // an unknown (0) foreground thread cannot be attached. `AttachThreadInput`
        // returns a `windows_core::BOOL`: `.as_bool()` is `true` on success.
        let attached = fg_thread != 0 && fg_thread != our_thread && {
            // SAFETY: attaching two real, distinct thread-input queues by id;
            // borrows no memory. Paired with the detach on the same ids below.
            unsafe { AttachThreadInput(fg_thread, our_thread, true).as_bool() }
        };

        // The three raise calls each get their OWN `unsafe` block (clippy
        // `multiple_unsafe_ops_per_block`). `target` is this process's own main
        // top-level window handle, primed from eframe's `CreationContext`; each
        // call only shows / reorders / activates that one window, and the return
        // values are intentionally ignored — the raise is best-effort.
        // SAFETY: valid own-window handle; makes the window visible if hidden.
        let _ = unsafe { ShowWindow(target, SW_SHOW) };
        // SAFETY: same valid handle; raises it to the top of the Z-order.
        let _ = unsafe { BringWindowToTop(target) };
        // SAFETY: same valid handle; requests foreground activation (honoured
        // because we attached to the foreground thread's input above).
        let _ = unsafe { SetForegroundWindow(target) };

        if attached {
            // SAFETY: detach the exact two thread ids we attached above, restoring
            // the independent input queues. Non-fatal if it fails.
            unsafe {
                let _ = AttachThreadInput(fg_thread, our_thread, false);
            }
        }
    }
}
