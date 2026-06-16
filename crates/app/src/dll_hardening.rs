//! Windows DLL search-order hardening — defeat DLL planting / preloading.
//!
// This is the ONE audited platform-FFI module the otherwise unsafe-free
// `c0pl4nd` binary permits: the two Win32 loader calls below are inherently
// `unsafe`, so this module opts back in (its parent binary uses
// `deny(unsafe_code)`, which this scoped `allow` overrides). Every `unsafe`
// block carries a `// SAFETY:` justification.
#![allow(unsafe_code)]
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

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(not(windows))]
    #[test]
    fn harden_is_a_noop_off_windows() {
        // On non-Windows the function is a pure no-op stub: it must be callable
        // and return without panicking. (There is no observable side effect to
        // assert beyond "does not panic / does not abort".)
        harden_dll_search_order();
    }

    #[cfg(not(windows))]
    #[test]
    fn harden_is_idempotent_off_windows() {
        // The no-op stub carries no state, so repeated calls are safe and
        // remain a no-op. Calling several times must still not panic.
        for _ in 0..5 {
            harden_dll_search_order();
        }
    }

    #[cfg(windows)]
    #[test]
    fn harden_runs_without_panic_on_windows() {
        // On Windows the two Win32 loader calls are process-wide and by design
        // irreversible, but they are idempotent and non-fatal (the `Result`s
        // are intentionally ignored). Calling the entry point must not panic.
        // We deliberately call it only once-per-test-process-effect via a
        // single invocation; a second call would simply re-apply the same
        // restriction and is also safe, but one call is sufficient to prove
        // the FFI path is sound.
        harden_dll_search_order();
    }
}
