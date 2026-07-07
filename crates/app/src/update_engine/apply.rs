//! Applying a verified update: keep-one-prior-binary backup, atomic install,
//! and rollback. The running-executable swap is delegated to `self-replace`
//! (which handles the Windows locked-file rename-aside trick); the testable
//! backup/install/rollback logic operates on arbitrary paths.
//!
//! The caller MUST have verified `new` (checksum + minisign + asset binding) via
//! [`super::verify::verify_artifact_bound`] before any function here touches the
//! live executable — verify-before-swap.

use std::fs;
use std::io;
use std::path::Path;

/// Copy `target` to `backup` (keep-one-prior for rollback), then move `new`
/// into `target`. Caller MUST have verified `new` (checksum + signature) first.
///
/// This is the install primitive for a NON-running target binary (an atomic
/// rename with a cross-filesystem copy fallback). Swapping the *currently
/// running* executable uses [`replace_running_executable`] instead, because the
/// running exe is locked on Windows and needs the `self-replace` rename-aside
/// dance. Both share the same keep-one-prior backup discipline.
///
/// `#[allow(dead_code)]`: this is the tested install primitive for a NON-running
/// target binary. The production self-updater swaps the RUNNING exe (via
/// `replace_running_executable` + `back_up`), so this entry point has no live
/// caller in the binary today — it is retained as the audited, unit-tested
/// counterpart to `rollback` and the install path a non-running-target caller
/// (e.g. a future packaging helper) would use. Not an error suppression.
#[allow(dead_code)]
pub fn install_with_backup(new: &Path, target: &Path, backup: &Path) -> io::Result<()> {
    back_up(target, backup)?;
    // Prefer an atomic rename; fall back to copy across filesystems.
    match fs::rename(new, target) {
        Ok(()) => Ok(()),
        Err(_) => {
            fs::copy(new, target)?;
            let _ = fs::remove_file(new);
            Ok(())
        }
    }
}

/// Keep-one-prior backup: copy `target` to `backup` when `target` exists (a
/// no-op for a first install). Shared by [`install_with_backup`] and the
/// running-exe swap path so both keep a single restorable prior binary.
pub fn back_up(target: &Path, backup: &Path) -> io::Result<()> {
    if target.exists() {
        fs::copy(target, backup)?;
    }
    Ok(())
}

/// Restore the prior binary from `backup` over `target`.
pub fn rollback(backup: &Path, target: &Path) -> io::Result<()> {
    if !backup.exists() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "no backup to roll back to",
        ));
    }
    fs::copy(backup, target)?;
    Ok(())
}

/// Replace the *currently running* executable with `new` (already verified).
/// Uses `self-replace` so it works while the binary is running, including the
/// Windows locked-file case.
///
/// On Windows `self-replace` stages the swap ENTIRELY inside the running exe's
/// own directory (it renames the running binary aside, `fs::copy`s the new one
/// into the same directory, then renames it into place). Two consequences the
/// caller relies on:
///
/// 1. The staging dir being on a DIFFERENT volume than the install dir is a
///    non-issue for THIS primitive — the cross-volume hop is a `fs::copy`, never
///    a `fs::rename`, so there is no `os error 17`.
/// 2. The swap needs the INSTALL DIRECTORY to be writable by this account. When
///    C0PL4ND lives in an admin-owned location (`C:\Program Files`, an
///    admin-extracted folder, a read-only mount) the rename/copy fails with an
///    access-denied `os error 5`. The caller MUST probe [`install_dir_writable`]
///    up front and surface a precise "relocate / run once elevated" message
///    rather than letting that access-denied error masquerade as "out of disk".
pub fn replace_running_executable(new: &Path) -> io::Result<()> {
    self_replace::self_replace(new)
}

/// Behavioral probe: can this process CREATE (and delete) a file in `dir`?
/// Creates a uniquely-named probe file and removes it. This is the SCR1B3
/// `dir_writable` shape — a real write attempt, never a path-name heuristic
/// (so it is correct for a Program Files install, a read-only mount, an
/// admin-extracted folder, or an ACL that denies THIS account regardless of the
/// directory's name). Returns `false` on any error (the dir does not exist, or
/// the create is denied); `true` only when the probe file was actually written.
pub fn dir_writable(dir: &Path) -> bool {
    // A per-attempt unique name so two concurrent probes never collide.
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let probe = dir.join(format!(".c0pl4nd-write-probe-{nanos}"));
    match fs::File::create(&probe) {
        Ok(_) => {
            let _ = fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

/// Whether the RUNNING executable's own directory is writable by this account —
/// the precondition for the in-place `self-replace` swap. `None` current-exe /
/// no-parent is treated as writable (`true`) so an unknown layout still ATTEMPTS
/// the swap and surfaces the real OS error, rather than false-blocking.
pub fn install_dir_writable() -> bool {
    match std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(|p| p.to_path_buf()))
    {
        Some(dir) => dir_writable(&dir),
        None => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write(path: &Path, content: &[u8]) {
        let mut f = fs::File::create(path).unwrap();
        f.write_all(content).unwrap();
    }

    #[test]
    fn install_creates_backup_and_swaps() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("c0pl4nd.bin");
        let new = dir.path().join("c0pl4nd.new");
        let backup = dir.path().join("c0pl4nd.bak");
        write(&target, b"v1");
        write(&new, b"v2");

        install_with_backup(&new, &target, &backup).unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"v2");
        assert_eq!(fs::read(&backup).unwrap(), b"v1");
    }

    #[test]
    fn install_without_existing_target_makes_no_backup() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("c0pl4nd.bin");
        let new = dir.path().join("c0pl4nd.new");
        let backup = dir.path().join("c0pl4nd.bak");
        write(&new, b"first-install");

        install_with_backup(&new, &target, &backup).unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"first-install");
        assert!(!backup.exists(), "no prior binary -> no backup written");
    }

    #[test]
    fn rollback_restores_prior() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("c0pl4nd.bin");
        let new = dir.path().join("c0pl4nd.new");
        let backup = dir.path().join("c0pl4nd.bak");
        write(&target, b"v1");
        write(&new, b"v2-broken");

        install_with_backup(&new, &target, &backup).unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"v2-broken");
        // Self-test failed -> roll back.
        rollback(&backup, &target).unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"v1");
    }

    #[test]
    fn rollback_without_backup_errors() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("t");
        let backup = dir.path().join("nope.bak");
        write(&target, b"x");
        assert!(rollback(&backup, &target).is_err());
    }

    #[test]
    fn dir_writable_true_for_a_fresh_tempdir() {
        // A freshly-created, owner-only temp dir is writable — the probe writes
        // and removes its sentinel file and reports `true`.
        let dir = tempfile::tempdir().unwrap();
        assert!(dir_writable(dir.path()));
        // The probe must not leave its sentinel behind.
        let leftovers: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with(".c0pl4nd-write-probe")
            })
            .collect();
        assert!(leftovers.is_empty(), "probe file must be cleaned up");
    }

    #[test]
    fn dir_writable_false_for_a_nonexistent_dir() {
        // A directory that does not exist cannot be written to — no create, no
        // panic, just `false` (the fail-closed default the caller relies on to
        // route a non-writable install to the actionable "relocate" message).
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("does-not-exist-subdir");
        assert!(!dir_writable(&missing));
    }

    #[cfg(windows)]
    #[test]
    fn dir_writable_false_for_the_windows_system_dir() {
        // The load-bearing case behind the user's "couldn't be saved or unpacked"
        // report: a system-owned directory this (non-elevated) account cannot
        // write. `C:\Windows` is denied for a normal user, so the probe must
        // return `false` — which is what routes the updater to the precise
        // "move the app / run once elevated" copy instead of a wrong disk-space
        // error. (Skipped implicitly if the test somehow runs elevated: an
        // elevated writer would legitimately see `true`, so only assert the
        // negative when we are NOT able to write — mirror the real gate.)
        let sys = std::path::Path::new(r"C:\Windows");
        // If we can actually write here (elevated CI), the probe is allowed to be
        // true; otherwise it MUST be false. Either way it must never panic.
        let can = dir_writable(sys);
        let elevated_write = std::fs::File::create(sys.join(".c0pl4nd-probe-elev")).is_ok();
        if elevated_write {
            let _ = std::fs::remove_file(sys.join(".c0pl4nd-probe-elev"));
        } else {
            assert!(
                !can,
                "a non-elevated account must not be able to write C:\\Windows"
            );
        }
    }

    #[test]
    fn install_dir_writable_never_panics() {
        // Whatever the test runner's install layout, the probe resolves to a bool
        // and never panics — the contract the apply-time gate depends on.
        let _ = install_dir_writable();
    }
}
