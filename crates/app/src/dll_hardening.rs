//! Windows DLL search-order hardening — defeat DLL planting / preloading.
//!
//! A GUI executable launched from an untrusted directory (the classic case is
//! the user's `Downloads` folder) is a DLL-planting target: by default the
//! Windows loader searches the *application directory* and the *current working
//! directory* BEFORE `System32` when resolving a DLL by name. An attacker who
//! drops a malicious `version.dll` / `dwmapi.dll` / `uxtheme.dll` (etc.) next to
//! the exe — or in the directory the exe is launched from — can get their code
//! loaded into our process at startup.
//!
//! The fix is the documented loader-hardening pair (Microsoft Learn, "Dynamic-Link
//! Library Search Order" + "SetDefaultDllDirectories"):
//!
//! 1. [`SetDefaultDllDirectories`] with `LOAD_LIBRARY_SEARCH_SYSTEM32 |
//!    LOAD_LIBRARY_SEARCH_USER_DIRS` removes the application directory and the
//!    CWD from the implicit search path for *all* subsequent `LoadLibrary`
//!    resolutions, restricting it to `System32` plus any directories explicitly
//!    added via `AddDllDirectory`.
//! 2. [`SetDllDirectoryW`] with an EMPTY string removes the current working
//!    directory from the legacy search path as well (it governs the older
//!    `LoadLibrary` search that `SetDefaultDllDirectories` does not fully cover),
//!    closing the CWD-planting vector for any code that predates / bypasses the
//!    default-directories mechanism.
//!
//! Both calls are process-wide and, by design, irreversible for the lifetime of
//! the process — which is exactly what we want for a security boundary set at
//! the very first instruction of `main`, before any other DLL can be loaded.
//!
//! The single [`harden_dll_search_order`] entry point is SAFE: the `unsafe` Win32
//! FFI lives entirely inside this module (each call wrapped with a `// SAFETY:`
//! justification), so the canonical `#![forbid(unsafe_code)]` `egui_main` binary
//! can call it without compromising that invariant. On non-Windows targets it is
//! a no-op stub.

/// Harden the DLL search order against planting / preloading attacks.
///
/// MUST be called as the first statement of `main` — before any code path that
/// could trigger an implicit `LoadLibrary` — so the restricted search path is in
/// effect for every subsequent module resolution. Process-wide and irreversible
/// by design. No-op on non-Windows platforms.
#[cfg(windows)]
pub fn harden_dll_search_order() {
    use windows::core::w;
    use windows::Win32::System::LibraryLoader::{
        SetDefaultDllDirectories, SetDllDirectoryW, LOAD_LIBRARY_SEARCH_SYSTEM32,
        LOAD_LIBRARY_SEARCH_USER_DIRS,
    };

    // SAFETY: `SetDefaultDllDirectories` is a plain kernel32 call taking a bitwise
    // OR of documented `LOAD_LIBRARY_SEARCH_*` flag constants and no pointers; it
    // has no memory-safety preconditions. We pass the documented system32+user-dirs
    // combination. A failure (e.g. on a host where the API is unavailable) is
    // non-fatal — the loader simply keeps its default behaviour — so the
    // `Result` is intentionally ignored.
    let _ = unsafe {
        SetDefaultDllDirectories(LOAD_LIBRARY_SEARCH_SYSTEM32 | LOAD_LIBRARY_SEARCH_USER_DIRS)
    };

    // SAFETY: `SetDllDirectoryW` takes a single null-terminated wide-string
    // pointer. `w!("")` is a compile-time `PCWSTR` to a `'static`, NUL-terminated
    // empty UTF-16 literal that outlives the call, satisfying the FFI contract.
    // Passing the empty string is the documented way to REMOVE the current working
    // directory from the legacy search path. Failure is non-fatal, so the `Result`
    // is intentionally ignored.
    let _ = unsafe { SetDllDirectoryW(w!("")) };
}

/// Non-Windows: there is no Win32 DLL search order to harden. No-op.
#[cfg(not(windows))]
pub fn harden_dll_search_order() {}
