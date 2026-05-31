//! Windows custom-frame Aero Snap for the frameless window (D4).
//!
//! C0PL4ND runs `with_decorations(false)`, which on Windows produces a window
//! with no `WS_THICKFRAME`/`WS_CAPTION` — so Win+Arrow snap, the maximize /
//! minimize animations, and the snap-layouts flyout do not work. The standard
//! Win32 fix (Microsoft Learn "Custom Window Frame Using DWM" + the widely-used
//! borderless-window pattern, grassator/win32-window-custom-titlebar) is to:
//!
//! 1. Add `WS_THICKFRAME | WS_CAPTION` back to the window style — this is what
//!    tells the DWM the window participates in snap / min-max animations — then
//! 2. handle `WM_NCCALCSIZE` (wparam == TRUE) by returning the proposed client
//!    rect UNCHANGED, so the frame the style would normally draw covers the
//!    whole window and stays invisible (we keep drawing our own chrome), and
//! 3. handle `WM_NCHITTEST` ourselves so the invisible frame still reports the
//!    resize edges + the title-bar drag strip, and
//! 4. clamp `WM_GETMINMAXINFO` to the monitor work area so a maximized window
//!    does not cover the taskbar.
//!
//! All of this is installed via `SetWindowSubclass` so winit's own wndproc keeps
//! running for everything we don't intercept. Entirely `#[cfg(windows)]`; the
//! caller no-ops elsewhere. This module owns ONLY the native frame — the GPU
//! chrome (wordmark, buttons, hover backplates) is unchanged and still drawn by
//! the renderer. Geometry is passed in from `window.rs` so the hit-test strip
//! matches the rendered title bar exactly.

#![cfg(windows)]

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MonitorFromWindow, HMONITOR, MONITORINFO, MONITOR_DEFAULTTONEAREST,
};
use windows::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::{
    GetClientRect, GetWindowLongPtrW, SetWindowLongPtrW, SetWindowPos, GWL_STYLE, MINMAXINFO,
    SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, WM_GETMINMAXINFO,
    WM_NCCALCSIZE, WM_NCHITTEST, WS_CAPTION, WS_MAXIMIZEBOX, WS_MINIMIZEBOX, WS_THICKFRAME,
};

// Hit-test result codes (Win32 `HT*`). The windows crate exposes these as `u32`
// constants; the wndproc returns an `isize` LRESULT, so we use the plain numeric
// values for clarity and a single cast site.
const HTCLIENT: isize = 1;
const HTCAPTION: isize = 2;
const HTLEFT: isize = 10;
const HTRIGHT: isize = 11;
const HTTOP: isize = 12;
const HTTOPLEFT: isize = 13;
const HTTOPRIGHT: isize = 14;
const HTBOTTOM: isize = 15;
const HTBOTTOMLEFT: isize = 16;
const HTBOTTOMRIGHT: isize = 17;

/// Subclass id for our snap wndproc (any stable per-process value).
const SUBCLASS_ID: usize = 0xC0_71_4D; // "C0PL4ND"-ish marker.

/// Geometry the hit-test needs, in PHYSICAL pixels, matching the renderer.
/// Stored in a leaked `Box` whose pointer is the subclass `dwrefdata`, so the
/// wndproc can read it without any global state.
#[derive(Clone, Copy)]
struct SnapGeometry {
    /// Title-bar drag-strip height (physical px).
    titlebar_h: i32,
    /// Resize-border band thickness (physical px).
    resize_border: i32,
    /// Width (physical px) of the caption-button cluster at the right edge; the
    /// drag strip excludes this region so a click on min/max/close is HTCLIENT
    /// and reaches our own button handler.
    buttons_w: i32,
}

/// Install the custom-frame subclass on `hwnd` and re-add the snap-enabling
/// window styles. The geometry is given in physical pixels and must match the
/// renderer's `TITLEBAR_H`, `RESIZE_BORDER`, and caption-button cluster width.
///
/// # Safety
/// `hwnd` must be a valid top-level window handle for the current process,
/// alive for the duration of the subclass. Called once per window from the UI
/// thread.
pub unsafe fn install(hwnd: isize, titlebar_h: i32, resize_border: i32, buttons_w: i32) {
    let hwnd = HWND(hwnd as *mut core::ffi::c_void);
    let geom = Box::new(SnapGeometry {
        titlebar_h,
        resize_border,
        buttons_w,
    });
    let geom_ptr = Box::into_raw(geom) as usize;

    // Add the styles that make the DWM treat the window as snap-able + animatable
    // while keeping it frameless (WM_NCCALCSIZE below hides the frame). Keep
    // whatever winit set and OR in the snap styles.
    let cur = GetWindowLongPtrW(hwnd, GWL_STYLE);
    let add = (WS_THICKFRAME | WS_CAPTION | WS_MAXIMIZEBOX | WS_MINIMIZEBOX).0 as isize;
    SetWindowLongPtrW(hwnd, GWL_STYLE, cur | add);

    // SetWindowSubclass inserts our proc into the chain; def_subclass keeps
    // winit's wndproc running for everything we don't intercept.
    let _ = SetWindowSubclass(hwnd, Some(snap_wndproc), SUBCLASS_ID, geom_ptr);

    // Force a frame recalculation so WM_NCCALCSIZE runs immediately with the new
    // styles (otherwise the OS frame flashes until the first resize).
    let _ = SetWindowPos(
        hwnd,
        None,
        0,
        0,
        0,
        0,
        SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
    );
}

/// Remove the subclass. Safe to call if never installed.
///
/// # Safety
/// `hwnd` must be the same handle passed to [`install`].
#[allow(dead_code)] // Teardown API: winit drops the HWND at process exit, so the
                    // happy path never calls this; kept for symmetry + tests.
pub unsafe fn uninstall(hwnd: isize) {
    let hwnd = HWND(hwnd as *mut core::ffi::c_void);
    // RemoveWindowSubclass does not hand back dwrefdata, so the tiny per-window
    // geometry Box is reclaimed by the OS at process exit. Bounded, documented,
    // teardown-path-only.
    let _ = RemoveWindowSubclass(hwnd, Some(snap_wndproc), SUBCLASS_ID);
}

/// The subclass wndproc. Intercepts the three frame messages and forwards
/// everything else down the chain (ultimately winit's wndproc) via
/// `DefSubclassProc`.
unsafe extern "system" fn snap_wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    refdata: usize,
) -> LRESULT {
    let geom = refdata as *const SnapGeometry;
    match msg {
        // Returning the proposed client rect unchanged (for wparam == TRUE)
        // makes the client area fill the whole window, hiding the native frame
        // WS_THICKFRAME/WS_CAPTION would otherwise draw — while keeping snap.
        WM_NCCALCSIZE if wparam.0 != 0 => LRESULT(0),

        // Map the cursor to a resize edge, the drag strip, or the client.
        WM_NCHITTEST => {
            if geom.is_null() {
                return def_subclass(hwnd, msg, wparam, lparam);
            }
            hit_test(hwnd, lparam, &*geom)
        }

        // Clamp a maximized window to the monitor work area so it never covers
        // the taskbar (a frameless window otherwise maximizes over it).
        WM_GETMINMAXINFO => {
            clamp_maxinfo(hwnd, lparam);
            // Let the default proc run too so it fills the other fields.
            def_subclass(hwnd, msg, wparam, lparam)
        }

        _ => def_subclass(hwnd, msg, wparam, lparam),
    }
}

/// Forward to the next handler in the subclass chain (ultimately winit's
/// wndproc). `DefSubclassProc` is the documented continue-the-chain call.
unsafe fn def_subclass(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    DefSubclassProc(hwnd, msg, wparam, lparam)
}

/// Resolve `WM_NCHITTEST`: resize edges within the border band, the title-bar
/// drag strip (HTCAPTION) outside the caption-button region, else HTCLIENT.
unsafe fn hit_test(hwnd: HWND, lparam: LPARAM, geom: &SnapGeometry) -> LRESULT {
    use windows::Win32::Foundation::POINT;
    use windows::Win32::Graphics::Gdi::ScreenToClient;

    // lparam packs the screen-space cursor: low word x, high word y (signed).
    let sx = (lparam.0 & 0xFFFF) as i16 as i32;
    let sy = ((lparam.0 >> 16) & 0xFFFF) as i16 as i32;
    let mut pt = POINT { x: sx, y: sy };
    let _ = ScreenToClient(hwnd, &mut pt);

    let mut rc = RECT::default();
    if GetClientRect(hwnd, &mut rc).is_err() {
        return LRESULT(HTCLIENT);
    }
    let w = rc.right - rc.left;
    let h = rc.bottom - rc.top;
    let b = geom.resize_border;

    let on_left = pt.x < b;
    let on_right = pt.x >= w - b;
    let on_top = pt.y < b;
    let on_bottom = pt.y >= h - b;

    let code = match (on_top, on_bottom, on_left, on_right) {
        (true, _, true, _) => HTTOPLEFT,
        (true, _, _, true) => HTTOPRIGHT,
        (_, true, true, _) => HTBOTTOMLEFT,
        (_, true, _, true) => HTBOTTOMRIGHT,
        (true, ..) => HTTOP,
        (_, true, ..) => HTBOTTOM,
        (_, _, true, _) => HTLEFT,
        (_, _, _, true) => HTRIGHT,
        _ => {
            // Inside the body. The title-bar strip (below the top resize band,
            // above titlebar_h) is the drag region — EXCEPT the caption-button
            // cluster at the right edge, which stays HTCLIENT so our own
            // min/max/close handler receives the click.
            if pt.y < geom.titlebar_h && pt.x < w - geom.buttons_w {
                HTCAPTION
            } else {
                HTCLIENT
            }
        }
    };
    LRESULT(code)
}

/// Clamp `WM_GETMINMAXINFO`'s max size/position to the monitor's work area so a
/// frameless maximize does not paint over the taskbar.
unsafe fn clamp_maxinfo(hwnd: HWND, lparam: LPARAM) {
    let mmi = lparam.0 as *mut MINMAXINFO;
    if mmi.is_null() {
        return;
    }
    let hmon: HMONITOR = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
    let mut mi = MONITORINFO {
        cbSize: core::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    if GetMonitorInfoW(hmon, &mut mi).as_bool() {
        let work = mi.rcWork;
        let mon = mi.rcMonitor;
        // Position is relative to the monitor origin.
        (*mmi).ptMaxPosition.x = work.left - mon.left;
        (*mmi).ptMaxPosition.y = work.top - mon.top;
        (*mmi).ptMaxSize.x = work.right - work.left;
        (*mmi).ptMaxSize.y = work.bottom - work.top;
        (*mmi).ptMaxTrackSize.x = work.right - work.left;
        (*mmi).ptMaxTrackSize.y = work.bottom - work.top;
    }
}
