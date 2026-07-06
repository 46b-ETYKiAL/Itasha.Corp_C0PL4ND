// Scoped-unsafe Win32 FFI module — the SECOND audited exception to the binary's
// `#![deny(unsafe_code)]` (the first is `dll_hardening`). Every `unsafe` here
// wraps a single documented Win32 call; the public API is safe.
#![allow(unsafe_code)]

//! Windows Job Object that guarantees no PTY child shell (or its descendants)
//! can outlive the app — even on a hard `std::process::exit(0)` or a crash.
//!
//! The app's fast-close path already `TerminateProcess`-es every pane's shell
//! before exiting (see `PaneTerm::kill_child`), which fixes the close LATENCY.
//! This module is the belt-and-suspenders NO-ORPHAN guarantee on top of it:
//!
//! 1. [`init`] creates one process-wide job object with
//!    `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` and keeps its handle open for the
//!    whole process lifetime (stored in a `OnceLock`, never closed at runtime —
//!    closing it would kill the children immediately).
//! 2. [`assign`] adds each freshly-spawned shell PID to the job. A process in the
//!    job automatically pulls its own children into the job too (unless they
//!    break away), so a program launched inside the shell is covered as well.
//! 3. When the app process exits by ANY means, the OS closes the job handle,
//!    which — because of `KILL_ON_JOB_CLOSE` — terminates every still-assigned
//!    process instantly. No lingering `conhost.exe`, no orphaned `cmd.exe`.
//!
//! Everything is BEST-EFFORT and non-fatal: a job-creation or assignment failure
//! (e.g. an old Windows that forbids nesting the ConPTY child in another job) is
//! logged and ignored — the app still runs and the `kill_child` fast-close path
//! still prevents orphans. No-op on non-Windows.

#[cfg(windows)]
mod imp {
    use std::sync::OnceLock;

    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
        SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };
    use windows::Win32::System::Threading::{OpenProcess, PROCESS_SET_QUOTA, PROCESS_TERMINATE};

    /// The process-wide job handle, stored as an `isize` so the `OnceLock` is
    /// `Send + Sync` (a raw `HANDLE` is not). Set once by [`init`]; NEVER closed
    /// while the process runs (the OS closes it on exit, which is what fires the
    /// kill-on-close). `0` is only ever observed if `init` failed.
    static JOB: OnceLock<isize> = OnceLock::new();

    /// Create the kill-on-close job object. Idempotent (only the first call wins,
    /// via `OnceLock`). Best-effort: on failure the job stays unset and [`assign`]
    /// becomes a no-op.
    pub fn init() {
        JOB.get_or_init(|| {
            // A nameless job object owned by this process.
            // SAFETY: `CreateJobObjectW(None, None)` creates an unnamed job with
            // default security; it borrows no memory and returns an owned handle
            // (or an error, which we handle).
            let job = match unsafe { CreateJobObjectW(None, None) } {
                Ok(h) if !h.is_invalid() => h,
                Ok(_) | Err(_) => {
                    tracing::warn!(
                        "job_object: CreateJobObjectW failed; PTY children rely on kill_child only"
                    );
                    return 0;
                }
            };
            // Set KILL_ON_JOB_CLOSE: when the last handle to the job closes (i.e.
            // this process exits), every assigned process is terminated.
            let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            // SAFETY: `job` is the valid handle created above; the pointer + byte
            // length describe a fully-initialised, correctly-typed, stack-owned
            // `JOBOBJECT_EXTENDED_LIMIT_INFORMATION` that outlives the call.
            let set = unsafe {
                SetInformationJobObject(
                    job,
                    JobObjectExtendedLimitInformation,
                    std::ptr::addr_of!(info).cast(),
                    std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                )
            };
            if set.is_err() {
                tracing::warn!("job_object: SetInformationJobObject(KILL_ON_JOB_CLOSE) failed");
                // SAFETY: `job` is a valid, still-open handle we own; close it on
                // the failure path so the half-set-up job does not leak.
                let _ = unsafe { CloseHandle(job) };
                return 0;
            }
            // Intentionally NEVER closed at runtime — the handle must stay open so
            // the OS can fire kill-on-close on process exit.
            job.0 as isize
        });
    }

    /// Assign a spawned child shell (by PID) to the kill-on-close job. Best-effort
    /// and non-fatal: a failure (job unset, or the ConPTY child already in a job
    /// that forbids nesting) is logged and ignored.
    pub fn assign(pid: u32) {
        let Some(&raw) = JOB.get() else { return };
        if raw == 0 {
            return; // init failed; kill_child is the fallback
        }
        let job = HANDLE(raw as *mut core::ffi::c_void);
        // PROCESS_SET_QUOTA is what AssignProcessToJobObject needs;
        // PROCESS_TERMINATE lets the job's kill-on-close terminate it.
        // SAFETY: `OpenProcess` takes access-rights flags + a PID by value and
        // returns an owned handle (or an error we handle); it borrows no memory.
        let child = match unsafe { OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, false, pid) }
        {
            Ok(h) if !h.is_invalid() => h,
            Ok(_) | Err(_) => {
                tracing::debug!(
                    pid,
                    "job_object: OpenProcess failed; child relies on kill_child"
                );
                return;
            }
        };
        // SAFETY: both `job` and `child` are valid handles owned here (the job for
        // the process lifetime; `child` closed just below); the call takes them by
        // value and borrows no memory.
        if unsafe { AssignProcessToJobObject(job, child) }.is_err() {
            tracing::debug!(
                pid,
                "job_object: AssignProcessToJobObject failed (nested-job?)"
            );
        }
        // SAFETY: `child` is the valid handle from `OpenProcess`, not used after
        // this; closing it releases our reference (job membership persists).
        let _ = unsafe { CloseHandle(child) };
    }
}

/// Create the process-wide kill-on-close job object (Windows only; no-op else).
/// Best-effort — see the module docs.
pub fn init() {
    #[cfg(windows)]
    imp::init();
}

/// Assign a spawned child shell PID to the kill-on-close job (Windows only;
/// no-op else). Best-effort — see the module docs.
pub fn assign(pid: u32) {
    #[cfg(windows)]
    imp::assign(pid);
    #[cfg(not(windows))]
    let _ = pid;
}
