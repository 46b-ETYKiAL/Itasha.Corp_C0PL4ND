//! Applying a verified update: keep-one-prior-binary backup, atomic install,
//! and rollback. The running-executable swap is delegated to `self-replace`
//! (which handles the Windows locked-file rename-aside trick); the testable
//! backup/install/rollback logic operates on arbitrary paths.
//!
//! The caller MUST have verified `new` (checksum + minisign) via
//! [`super::verify::verify_artifact`] before any function here touches the live
//! executable — verify-before-swap.

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
pub fn replace_running_executable(new: &Path) -> io::Result<()> {
    self_replace::self_replace(new)
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
}
