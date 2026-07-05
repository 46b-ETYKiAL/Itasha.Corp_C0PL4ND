//! Remove ONLY the residual native CLOSE (system-menu) caption button so DWM
//! stops compositing a second "×" over the app's own titlebar close button.
//!
//! ## Why this is the ONE runtime style touch
//!
//! The native minimize/maximize caption buttons are suppressed at WINDOW
//! CREATION via `ViewportBuilder::with_minimize_button(false)` /
//! `with_maximize_button(false)` (see `egui_main.rs`) — egui-winit maps those to
//! winit's `enabled_buttons`, so `WS_MINIMIZEBOX` / `WS_MAXIMIZEBOX` are never
//! set and DWM never draws min/max. That is the clean, frame-safe path.
//!
//! There is NO creation-time flag that clears `WS_SYSMENU`: winit puts it in its
//! unconditional base style (`WS_CAPTION | WS_BORDER | WS_CLIPSIBLINGS |
//! WS_SYSMENU`). On Win11, DWM draws the native CLOSE button from `WS_SYSMENU`,
//! and on an undecorated window (winit #2754) it composites through as a second,
//! offset "×" over our custom titlebar. So the close button is the one caption
//! control that must be cleared at runtime.
//!
//! ## Why this does NOT reintroduce the red-titlebar regression
//!
//! The earlier full strip also cleared `WS_CAPTION` and forced a frame recalc,
//! which fought winit's frameless composition (winit KEEPS `WS_CAPTION` and hides
//! the frame via `WM_NCCALCSIZE`) and repainted a stray native caption band — the
//! reported red titlebar. This clears ONLY `WS_SYSMENU` and leaves
//! `WS_CAPTION` / `WS_BORDER` intact, so the frameless composition is undisturbed:
//! `WS_SYSMENU` controls the system menu + native close glyph, not the frame.
//!
//! ## Trade-off (mitigated)
//!
//! Clearing `WS_SYSMENU` removes the in-window system menu (Alt+Space, and the
//! title-bar right-click menu). Alt+F4 is restored in-app by
//! [`super::C0pl4ndApp`] (an Alt+F4 key event sends `ViewportCommand::Close`),
//! and the taskbar "Close window" command sends `WM_CLOSE` via the shell
//! independently of `WS_SYSMENU`, so both keep working. The app's own titlebar
//! close button is unaffected.
//!
//! Windows-only; every entry point is a no-op elsewhere. Lives as an `egui_app`
//! submodule (not a crate-root module) so the `#[path=…]`-included test harnesses
//! resolve it without re-declaring it.

/// Prime the module with THIS process's main window handle, taken from eframe's
/// `CreationContext`. Idempotent; a zero handle is ignored. Windows-only.
#[cfg(windows)]
pub(crate) fn set_main_hwnd(hwnd: isize) {
    imp::set_main_hwnd(hwnd);
}

/// No-op on non-Windows platforms.
#[cfg(not(windows))]
pub(crate) fn set_main_hwnd(_hwnd: isize) {}

/// Clear the residual `WS_SYSMENU` bit so DWM stops drawing the native close
/// button. Safe + cheap to call every frame: it only writes when `WS_SYSMENU` is
/// actually set, so it self-heals if winit re-asserts it on a resize/restore and
/// costs near-nothing otherwise. Windows-only; a no-op elsewhere and before the
/// HWND is primed.
#[cfg(windows)]
pub(crate) fn ensure_close_button_stripped() {
    imp::ensure_close_button_stripped();
}

/// No-op on non-Windows platforms.
#[cfg(not(windows))]
pub(crate) fn ensure_close_button_stripped() {}

#[cfg(windows)]
mod imp {
    // The audited Win32 FFI is quarantined here with `// SAFETY:` justifications,
    // mirroring the other `#[cfg(windows)]` modules (job_object, dll_hardening).
    #![allow(unsafe_code)]

    use std::sync::atomic::{AtomicIsize, Ordering};

    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{
        GetWindowLongPtrW, SetWindowLongPtrW, SetWindowPos, GWL_STYLE, SWP_FRAMECHANGED,
        SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, WS_SYSMENU,
    };

    /// The single style bit that makes Win11 DWM draw the native CLOSE caption
    /// button. Deliberately does NOT include `WS_CAPTION`/`WS_BORDER` (clearing
    /// those fights winit's frameless composition and repaints a native frame).
    const CLOSE_BUTTON_STYLE: u32 = WS_SYSMENU.0;

    /// Whether the close-button (system-menu) style bit is currently set.
    pub(super) fn close_button_style_present(style: u32) -> bool {
        style & CLOSE_BUTTON_STYLE != 0
    }

    /// `style` with the system-menu bit cleared; all other bits preserved.
    pub(super) fn style_without_close_button(style: u32) -> u32 {
        style & !CLOSE_BUTTON_STYLE
    }

    /// Cached main-window HWND (0 = not yet primed). C0PL4ND uses ONE OS window.
    static CACHED_HWND: AtomicIsize = AtomicIsize::new(0);

    pub fn set_main_hwnd(hwnd: isize) {
        if hwnd != 0 {
            CACHED_HWND.store(hwnd, Ordering::Relaxed);
        }
    }

    pub fn ensure_close_button_stripped() {
        let hwnd = CACHED_HWND.load(Ordering::Relaxed);
        if hwnd == 0 {
            return; // not primed yet — new() primes it before the first frame.
        }
        let h = HWND(hwnd as *mut core::ffi::c_void);
        // SAFETY: `hwnd` is this process's main top-level window handle, primed
        // from eframe's `CreationContext`; `GetWindowLongPtrW(GWL_STYLE)` only
        // reads this window's own style word.
        let style = unsafe { GetWindowLongPtrW(h, GWL_STYLE) } as u32;
        if !close_button_style_present(style) {
            return; // already stripped — nothing to do this frame.
        }
        // SAFETY: same valid `hwnd`; writing the style word with ONLY the
        // system-menu bit cleared is a well-formed `SetWindowLongPtrW`. WS_CAPTION
        // is intentionally preserved so the frameless composition is untouched.
        unsafe { SetWindowLongPtrW(h, GWL_STYLE, style_without_close_button(style) as isize) };
        // SAFETY: same valid `hwnd`; the SWP_* no-move/no-size/no-zorder/no-activate
        // flags mean every positional argument is ignored, so this only forces the
        // frame recalc that makes DWM drop the now-unstyled native close button.
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
        use windows::Win32::UI::WindowsAndMessaging::{WS_CAPTION, WS_MAXIMIZE, WS_SYSMENU};

        #[test]
        fn close_button_style_detected_and_cleared_leaving_caption() {
            // A window carrying the system-menu bit plus WS_CAPTION (the frame bit
            // winit relies on) and an unrelated maximized-STATE bit.
            let with = WS_SYSMENU.0 | WS_CAPTION.0 | WS_MAXIMIZE.0;
            assert!(close_button_style_present(with));

            let stripped = style_without_close_button(with);
            assert!(
                !close_button_style_present(stripped),
                "the system-menu (close) bit must be cleared"
            );
            // WS_CAPTION MUST survive — clearing it is what caused the red-frame
            // regression, so this asserts we never touch it.
            assert_eq!(stripped & WS_CAPTION.0, WS_CAPTION.0);
            // The unrelated maximized-state bit survives too.
            assert_eq!(stripped & WS_MAXIMIZE.0, WS_MAXIMIZE.0);
            // Idempotent: stripping an already-clean style changes nothing.
            assert_eq!(style_without_close_button(stripped), stripped);
            // A style without the bit is reported clean (no needless work).
            assert!(!close_button_style_present(WS_CAPTION.0));
        }
    }
}
