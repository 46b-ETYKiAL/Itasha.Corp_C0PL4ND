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
use windows::Win32::UI::Shell::{
    DefSubclassProc, GetWindowSubclass, RemoveWindowSubclass, SetWindowSubclass,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetClientRect, GetWindowLongPtrW, SetWindowLongPtrW, SetWindowPos, GWL_STYLE, MINMAXINFO,
    SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, WM_GETMINMAXINFO,
    WM_NCCALCSIZE, WM_NCDESTROY, WM_NCHITTEST, WS_CAPTION, WS_MAXIMIZEBOX, WS_MINIMIZEBOX,
    WS_THICKFRAME,
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

/// Physical-pixel x-ranges within the title-bar strip that are INTERACTIVE
/// (the tab chips, the new-tab `+`, the settings gear). These MUST hit-test as
/// `HTCLIENT` so a click reaches winit's `MouseInput` (and our tab router)
/// instead of being swallowed by Windows as a title-bar drag (`HTCAPTION`).
/// Published every frame by the renderer (`window.rs`) after it shapes the tab
/// chrome. A single global is correct: C0PL4ND uses ONE OS window — tabs are
/// in-window — so per-HWND keying is unnecessary.
static INTERACTIVE_ZONES: std::sync::OnceLock<std::sync::Mutex<Vec<(i32, i32)>>> =
    std::sync::OnceLock::new();

/// Publish the interactive title-bar x-ranges (physical px) for the hit-test.
/// Called from the renderer each frame. Lock-guarded, no `unsafe`.
pub fn set_interactive_zones(zones: Vec<(i32, i32)>) {
    let cell = INTERACTIVE_ZONES.get_or_init(|| std::sync::Mutex::new(Vec::new()));
    if let Ok(mut g) = cell.lock() {
        *g = zones;
    }
}

/// True when client-space x `px` falls within a published interactive zone.
fn in_interactive_zone(px: i32) -> bool {
    INTERACTIVE_ZONES
        .get()
        .and_then(|m| m.lock().ok())
        .map(|g| zone_contains(px, &g))
        .unwrap_or(false)
}

/// Pure half-open membership test: is `px` within any `[x0, x1)` range? Split
/// out so the hit-test predicate is unit-testable without a window/global.
fn zone_contains(px: i32, zones: &[(i32, i32)]) -> bool {
    zones.iter().any(|&(x0, x1)| px >= x0 && px < x1)
}

/// Geometry the hit-test needs, in PHYSICAL pixels, matching the renderer.
/// Heap-allocated via `Box`; the raw pointer is the subclass `dwrefdata`, so
/// the wndproc can read it without any global state. The `Box` is RECLAIMED
/// (freed) when the subclass is removed — either explicitly via [`uninstall`]
/// or automatically on `WM_NCDESTROY` (see [`reclaim_geom`]) — so the
/// allocation never leaks across a window lifetime.
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
pub unsafe fn install(
    hwnd: isize,
    titlebar_h: i32,
    resize_border: i32,
    buttons_w: i32,
    acrylic: bool,
) {
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

    // Windows 11: round the window corners to match the modern OS chrome.
    // Ignored as a harmless no-op on Windows 10 (the attribute is unknown there).
    use windows::Win32::Graphics::Dwm::{
        DwmSetWindowAttribute, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND,
    };
    let corner_pref = DWMWCP_ROUND;
    let _ = DwmSetWindowAttribute(
        hwnd,
        DWMWA_WINDOW_CORNER_PREFERENCE,
        &corner_pref as *const _ as *const core::ffi::c_void,
        core::mem::size_of_val(&corner_pref) as u32,
    );

    // Windows 11 acrylic/mica backdrop (opt-in). Only meaningful when the window
    // is translucent (the wgpu surface clears with alpha < 1); on an opaque
    // surface the backdrop is simply never seen. No-op on Windows 10.
    if acrylic {
        use windows::Win32::Graphics::Dwm::{DWMSBT_TRANSIENTWINDOW, DWMWA_SYSTEMBACKDROP_TYPE};
        let backdrop = DWMSBT_TRANSIENTWINDOW;
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_SYSTEMBACKDROP_TYPE,
            &backdrop as *const _ as *const core::ffi::c_void,
            core::mem::size_of_val(&backdrop) as u32,
        );
    }
}

/// Reclaim (free) the heap-allocated [`SnapGeometry`] backing the subclass on
/// `hwnd`, if one is installed, then remove the subclass. Idempotent: a second
/// call finds no subclass and is a no-op, so it can never double-free.
///
/// `RemoveWindowSubclass` does NOT hand back the `dwRefData`, so we first read
/// it with `GetWindowSubclass`, remove the subclass (after which no further
/// wndproc call can observe the pointer), and only THEN reconstruct the `Box`
/// to drop it. Ordering matters: removing before the `Box::from_raw` closes the
/// window where a concurrent message could read freed memory.
///
/// # Safety
/// `hwnd` must be a valid window handle, and any installed subclass on it must
/// have been installed by [`install`] (so the `dwRefData` is a `Box<SnapGeometry>`
/// pointer this module created).
unsafe fn reclaim_geom(hwnd: HWND) {
    let mut refdata: usize = 0;
    // `GetWindowSubclass` returns TRUE and fills `refdata` only when OUR subclass
    // (matching proc + id) is currently installed on the window.
    let installed =
        GetWindowSubclass(hwnd, Some(snap_wndproc), SUBCLASS_ID, Some(&mut refdata)).as_bool();
    // Always attempt removal; harmless when not installed.
    let _ = RemoveWindowSubclass(hwnd, Some(snap_wndproc), SUBCLASS_ID);
    if installed && refdata != 0 {
        // SAFETY: `refdata` is the exact pointer produced by `Box::into_raw` in
        // `install`; the subclass is now removed so no wndproc can read it; we
        // own it and drop it exactly once.
        drop(Box::from_raw(refdata as *mut SnapGeometry));
    }
}

/// Remove the subclass and free its geometry allocation. Safe to call if never
/// installed (no-op) and safe to call more than once (idempotent — the second
/// call finds no subclass).
///
/// # Safety
/// `hwnd` must be the same handle passed to [`install`].
#[allow(dead_code)] // Teardown API: winit drops the HWND at process exit (where
                    // WM_NCDESTROY also reclaims), so the happy path may never
                    // call this; kept for explicit teardown + tests.
pub unsafe fn uninstall(hwnd: isize) {
    let hwnd = HWND(hwnd as *mut core::ffi::c_void);
    reclaim_geom(hwnd);
}

/// Flash the taskbar button until the window is next brought to the foreground
/// (`FLASHW_TRAY | FLASHW_TIMERNOFG`). Used to surface an OSC 9/777 desktop
/// notification that arrived while the window was unfocused.
///
/// # Safety
/// `hwnd` must be a live top-level window handle for the current process.
pub unsafe fn flash_taskbar(hwnd: isize) {
    use windows::Win32::UI::WindowsAndMessaging::{
        FlashWindowEx, FLASHWINFO, FLASHW_TIMERNOFG, FLASHW_TRAY,
    };
    let hwnd = HWND(hwnd as *mut core::ffi::c_void);
    let info = FLASHWINFO {
        cbSize: core::mem::size_of::<FLASHWINFO>() as u32,
        hwnd,
        dwFlags: FLASHW_TRAY | FLASHW_TIMERNOFG,
        uCount: 0,
        dwTimeout: 0,
    };
    let _ = FlashWindowEx(&info);
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

        // Window is being destroyed — the LAST message a window receives. Free
        // the heap-allocated geometry and detach the subclass here so the `Box`
        // is reclaimed even when the host (winit) never calls `uninstall`. We
        // forward to the default proc FIRST so the rest of the chain sees the
        // destroy, then reclaim (reclaim is idempotent, so a later `uninstall`
        // is a harmless no-op).
        WM_NCDESTROY => {
            let r = def_subclass(hwnd, msg, wparam, lparam);
            reclaim_geom(hwnd);
            r
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
            // above titlebar_h) is the drag region — EXCEPT (a) the
            // caption-button cluster at the right edge and (b) the interactive
            // controls in the tab strip (tab chips, the new-tab `+`, the
            // settings gear), both of which stay HTCLIENT so the click reaches
            // winit's MouseInput handler instead of starting a window drag.
            if pt.y < geom.titlebar_h && pt.x < w - geom.buttons_w && !in_interactive_zone(pt.x) {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zone_contains_is_half_open_and_multi_range() {
        // Two interactive ranges (e.g. the tab chip and the '+' affordance).
        let zones = [(96, 121), (140, 181)];
        // Inside the first range: HTCLIENT (interactive).
        assert!(zone_contains(96, &zones)); // left edge inclusive
        assert!(zone_contains(120, &zones));
        // The right edge is EXCLUSIVE so adjacent ranges never double-count.
        assert!(!zone_contains(121, &zones));
        // The gap between ranges stays draggable (HTCAPTION).
        assert!(!zone_contains(130, &zones));
        // Inside the second range.
        assert!(zone_contains(140, &zones));
        assert!(zone_contains(180, &zones));
        assert!(!zone_contains(181, &zones));
        // No zones published yet -> nothing is interactive (whole strip drags).
        assert!(!zone_contains(100, &[]));
    }

    #[test]
    fn set_interactive_zones_publishes_for_the_hit_test() {
        set_interactive_zones(vec![(10, 20), (30, 40)]);
        assert!(in_interactive_zone(15));
        assert!(in_interactive_zone(35));
        assert!(!in_interactive_zone(25));
        // Re-publishing replaces (does not accumulate) the previous frame.
        set_interactive_zones(vec![(100, 110)]);
        assert!(in_interactive_zone(105));
        assert!(!in_interactive_zone(15));
    }
}
