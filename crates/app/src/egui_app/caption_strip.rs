//! Strip the residual native caption buttons off the frameless eframe window.
//!
//! ## Root cause (primary-sourced)
//!
//! C0PL4ND runs `with_decorations(false)` + `with_transparent(true)` and draws
//! its OWN min/max/close in the egui titlebar. But winit leaves
//! `WS_SYSMENU | WS_MINIMIZEBOX | WS_MAXIMIZEBOX` set on an UNDECORATED top-level
//! window — it strips only `WS_CAPTION`/`WS_SIZEBOX` (winit #2754). On Win11 DWM
//! draws the native min/max/close caption buttons GATED ON THOSE STYLE BITS.
//! While the window is opaque, its own opaque pixels hide the DWM-drawn buttons;
//! the moment a translucent backdrop is applied (`apply_mica`/`apply_acrylic`,
//! a DWM-composited frame) those native buttons show THROUGH as a SECOND,
//! offset set over our custom titlebar — the reported "doubled caption buttons".
//!
//! ## Why `WM_NCCALCSIZE` cannot fix it
//!
//! Returning 0 from `WM_NCCALCSIZE` removes the standard non-client frame, but
//! Microsoft documents it "does not affect frames extended into the client area"
//! / DWM-composited content. The caption buttons are DWM-composited, so
//! `WM_NCCALCSIZE` is structurally incapable of removing them.
//!
//! ## The fix (canonical: melak47/BorderlessWindow, the MS DWM custom-frame
//! sample, Tao/Tauri, Electron; matches the SCR1B3 `scribe-win32-chrome` crate)
//!
//! CLEAR the caption-button style bits with `SetWindowLongPtrW(GWL_STYLE, …)` +
//! `SetWindowPos(SWP_FRAMECHANGED)`. With the bits gone, DWM draws no native
//! buttons, in opaque OR transparent mode, and winit's transparency is left
//! untouched. Re-applied every frame because winit re-derives styles from its
//! `WindowFlags` on some resize/restore paths — cheap, because the strip only
//! writes when a caption-button bit is actually present.
//!
//! Trade-off of clearing `WS_SYSMENU`: Alt+Space and the taskbar right-click
//! system menu go away; the custom titlebar already provides min/max/close.
//! Programmatic maximize/minimize (our buttons issue `ViewportCommand`) does not
//! depend on `WS_MAXIMIZEBOX`/`WS_MINIMIZEBOX`, so it keeps working.
//!
//! Windows-only; every entry point is a no-op elsewhere (the bug is Win11-DWM
//! specific). Lives as an `egui_app` submodule (not a crate-root module) so the
//! `#[path=…]`-included test harnesses resolve it without re-declaring it.

/// Prime the module with THIS process's main window handle, taken from eframe's
/// `CreationContext` (the same handle `window-vibrancy` applies the backdrop to).
/// Idempotent; a zero handle is ignored. Windows-only; a no-op elsewhere.
#[cfg(windows)]
pub(crate) fn set_main_hwnd(hwnd: isize) {
    imp::set_main_hwnd(hwnd);
}

/// No-op on non-Windows platforms.
#[cfg(not(windows))]
pub(crate) fn set_main_hwnd(_hwnd: isize) {}

/// Clear the residual native caption-button window styles so DWM stops
/// compositing the doubled min/max/close over the custom titlebar. Safe + cheap
/// to call every frame: it only writes when a caption-button bit is actually set,
/// so it self-heals if winit re-asserts the styles on a resize/restore and costs
/// near-nothing otherwise. Windows-only; a no-op elsewhere and before the HWND
/// is primed.
#[cfg(windows)]
pub(crate) fn ensure_caption_stripped() {
    imp::ensure_caption_stripped();
}

/// No-op on non-Windows platforms.
#[cfg(not(windows))]
pub(crate) fn ensure_caption_stripped() {}

#[cfg(windows)]
mod imp {
    // The audited Win32 FFI is quarantined here with `// SAFETY:` justifications,
    // mirroring the other `#[cfg(windows)]` modules (job_object, dll_hardening).
    #![allow(unsafe_code)]

    use std::sync::atomic::{AtomicIsize, Ordering};

    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{
        GetWindowLongPtrW, SetWindowLongPtrW, SetWindowPos, GWL_STYLE, SWP_FRAMECHANGED,
        SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, WS_CAPTION, WS_MAXIMIZEBOX,
        WS_MINIMIZEBOX, WS_SYSMENU,
    };

    /// The window-style bits that make Win11 DWM draw the native min/max/close
    /// caption buttons. winit leaves `WS_SYSMENU | WS_MINIMIZEBOX | WS_MAXIMIZEBOX`
    /// on the undecorated window (winit #2754); `WS_CAPTION` is included for
    /// completeness — clearing an already-absent bit is a no-op.
    const CAPTION_BUTTON_STYLES: u32 =
        WS_SYSMENU.0 | WS_MINIMIZEBOX.0 | WS_MAXIMIZEBOX.0 | WS_CAPTION.0;

    /// Whether any caption-button style bit is currently set on `style`.
    pub(super) fn caption_button_styles_present(style: u32) -> bool {
        style & CAPTION_BUTTON_STYLES != 0
    }

    /// `style` with every caption-button bit cleared; all other bits preserved.
    pub(super) fn style_without_caption_buttons(style: u32) -> u32 {
        style & !CAPTION_BUTTON_STYLES
    }

    /// Cached main-window HWND (0 = not yet primed). C0PL4ND uses ONE OS window.
    static CACHED_HWND: AtomicIsize = AtomicIsize::new(0);

    pub fn set_main_hwnd(hwnd: isize) {
        if hwnd != 0 {
            CACHED_HWND.store(hwnd, Ordering::Relaxed);
        }
    }

    pub fn ensure_caption_stripped() {
        let hwnd = CACHED_HWND.load(Ordering::Relaxed);
        if hwnd == 0 {
            return; // not primed yet — new() primes it before the first frame.
        }
        let h = HWND(hwnd as *mut core::ffi::c_void);
        // SAFETY: `hwnd` is this process's main top-level window handle, primed
        // from eframe's `CreationContext`; `GetWindowLongPtrW(GWL_STYLE)` only
        // reads this window's own style word.
        let style = unsafe { GetWindowLongPtrW(h, GWL_STYLE) } as u32;
        if !caption_button_styles_present(style) {
            return; // already stripped — nothing to do this frame.
        }
        // SAFETY: same valid `hwnd`; writing the style word with the caption-button
        // bits cleared is a well-formed `SetWindowLongPtrW` — the canonical
        // borderless-window fix (melak47/BorderlessWindow, MS DWM custom-frame).
        unsafe { SetWindowLongPtrW(h, GWL_STYLE, style_without_caption_buttons(style) as isize) };
        // SAFETY: same valid `hwnd`; the SWP_* no-move/no-size/no-zorder/no-activate
        // flags mean every positional argument is ignored, so this only forces the
        // frame recalc that makes DWM drop the now-unstyled native caption buttons.
        let _ = unsafe {
            SetWindowPos(
                h,
                None,
                0,
                0,
                0,
                0,
                SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
            )
        };
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use windows::Win32::UI::WindowsAndMessaging::{
            WS_CAPTION, WS_MAXIMIZE, WS_MAXIMIZEBOX, WS_MINIMIZEBOX, WS_SYSMENU,
        };

        #[test]
        fn caption_button_styles_detected_and_cleared() {
            // A typical winit undecorated style: the caption-button bits set, plus
            // an unrelated bit (WS_MAXIMIZE = the maximized STATE) that must survive.
            let with =
                WS_CAPTION.0 | WS_SYSMENU.0 | WS_MAXIMIZEBOX.0 | WS_MINIMIZEBOX.0 | WS_MAXIMIZE.0;
            assert!(caption_button_styles_present(with));

            let stripped = style_without_caption_buttons(with);
            assert!(
                !caption_button_styles_present(stripped),
                "all caption-button bits must be cleared"
            );
            // The unrelated maximized-state bit survives the strip.
            assert_eq!(stripped & WS_MAXIMIZE.0, WS_MAXIMIZE.0);
            // Idempotent: stripping an already-clean style changes nothing.
            assert_eq!(style_without_caption_buttons(stripped), stripped);
            // A style with none of the bits is reported clean (no needless work).
            assert!(!caption_button_styles_present(WS_MAXIMIZE.0));
        }
    }
}
