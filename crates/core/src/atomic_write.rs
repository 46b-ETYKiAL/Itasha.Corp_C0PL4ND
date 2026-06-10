//! Crash-safe atomic file writes via temp-file + rename.
//!
//! `std::fs::write` is non-atomic: a crash (or power loss) mid-write can leave a
//! truncated, half-written file on disk. For persisted session/workspace state
//! that is read back on the next launch, a torn file means a corrupt restore.
//!
//! [`atomic_write`] eliminates the torn-write window by writing the full payload
//! to a sibling temp file in the **same directory** and then `rename`-ing it over
//! the target. A same-volume rename is atomic on NTFS (Windows) and on modern
//! POSIX filesystems: a concurrent or post-crash reader sees either the complete
//! old file or the complete new file, never a partial one.
//!
//! # Windows note
//!
//! `std::fs::rename` maps to `MoveFileExW(MOVEFILE_REPLACE_EXISTING)` on Windows
//! when source and destination are on the same volume, which replaces an existing
//! destination atomically (the semantics `ReplaceFileW` also provides). Because
//! the temp file is created as a sibling of the target (same parent dir → same
//! volume), the rename is always same-volume and therefore atomic. No
//! cross-volume copy fallback is needed for this use.
//!
//! # Cleanup contract
//!
//! On the success path the temp file is consumed by the rename — **no `.tmp` is
//! left behind**. On a write/flush/rename error the partial temp file is removed
//! on a best-effort basis so a failed save does not litter the state directory.

use std::fs;
use std::io::{self, Write};
use std::path::Path;

/// Suffix used for the sibling temp file. Kept short and `.tmp`-shaped so a
/// human inspecting the state dir recognizes it as scratch.
const TMP_SUFFIX: &str = ".tmp";

/// Atomically write `bytes` to `path`.
///
/// Writes to `<path><TMP_SUFFIX>` in the same directory, flushes it to the OS,
/// then renames it over `path`. Parent directories are created if missing.
///
/// On the success path no temp file remains. On any failure the temp file is
/// removed best-effort and the originating [`io::Error`] is returned — the
/// existing target (if any) is left untouched, preserving the never-corrupt
/// contract callers rely on for restore.
///
/// # Errors
///
/// Returns the underlying [`io::Error`] if the parent dir cannot be created, the
/// temp file cannot be written/flushed, or the rename fails.
pub fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }

    let tmp = tmp_path_for(path);

    // Scope the file handle so it is closed before the rename (Windows refuses to
    // rename over / move an open handle in some configurations).
    if let Err(e) = write_and_sync(&tmp, bytes) {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }

    if let Err(e) = fs::rename(&tmp, path) {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }

    Ok(())
}

/// Atomically write `bytes` to `path` and tighten it to **owner-only** access.
///
/// Identical to [`atomic_write`] (same temp-file + rename never-corrupt
/// contract), but additionally restricts the result so other local accounts
/// cannot read it. Use this for state files that can reflect the user's
/// environment — e.g. saved workspace layouts, which record each pane's `cwd`
/// (revealing usernames and project paths).
///
/// On Unix the owner-only `0600` mode is applied to the **temp file before the
/// rename**, so the file is never world-readable even for the brief window
/// between create and rename. On Windows the inheritance-stripped owner-only
/// ACL (via `icacls`) is applied to the final `path` after the rename, matching
/// the config-file tightening exactly.
///
/// Permission tightening is best-effort and never fails the write: a restrictive
/// filesystem must not block a state save. Only an I/O failure of the write or
/// rename itself is surfaced.
///
/// # Errors
///
/// Returns the underlying [`io::Error`] if the parent dir cannot be created, the
/// temp file cannot be written/flushed, or the rename fails.
pub fn atomic_write_owner_only(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }

    let tmp = tmp_path_for(path);

    if let Err(e) = write_and_sync(&tmp, bytes) {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }

    // Tighten on Unix BEFORE the rename so the file is never world-readable
    // (closes the create→rename window). Windows owner-only ACL is path-based
    // and is applied to the final path after the rename below.
    #[cfg(unix)]
    crate::fs_perms::restrict_to_owner(&tmp);

    if let Err(e) = fs::rename(&tmp, path) {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }

    // On Windows the icacls grant must reference the final path; on Unix this is
    // a no-op cfg branch (perms were already applied to the temp file).
    #[cfg(windows)]
    crate::fs_perms::restrict_to_owner(path);

    Ok(())
}

/// Write the payload to `tmp`, flush the userspace buffer, and `sync_all` so the
/// bytes reach the device before the rename makes them visible at `path`.
fn write_and_sync(tmp: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut f = fs::File::create(tmp)?;
    f.write_all(bytes)?;
    f.flush()?;
    // Best-effort durability: persist the data blocks before the rename. A
    // platform that does not support fsync surfaces the error here rather than
    // silently skipping it.
    f.sync_all()?;
    Ok(())
}

/// Derive the sibling temp path for `path` by appending [`TMP_SUFFIX`] to the
/// full file name (so it lands in the same directory → same volume).
fn tmp_path_for(path: &Path) -> std::path::PathBuf {
    let mut name = path
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_default();
    name.push(TMP_SUFFIX);
    match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.join(name),
        _ => std::path::PathBuf::from(name),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("c0pl4nd-aw-{}-{}", std::process::id(), name))
    }

    #[test]
    fn writes_new_file() {
        let p = scratch("new.bin");
        let _ = fs::remove_file(&p);
        atomic_write(&p, b"hello").expect("write");
        assert_eq!(fs::read(&p).expect("read"), b"hello");
        let _ = fs::remove_file(&p);
    }

    #[test]
    fn replaces_existing_file_and_leaves_no_tmp() {
        let p = scratch("replace.bin");
        fs::write(&p, b"old contents that are longer").expect("seed");
        atomic_write(&p, b"new").expect("write");
        assert_eq!(fs::read(&p).expect("read"), b"new");

        // No sibling .tmp left behind on success.
        let tmp = tmp_path_for(&p);
        assert!(
            !tmp.exists(),
            "temp file {tmp:?} must not remain after success"
        );

        let _ = fs::remove_file(&p);
    }

    #[test]
    fn creates_missing_parent_dirs() {
        let dir = scratch("nested-dir");
        let _ = fs::remove_dir_all(&dir);
        let p = dir.join("sub").join("file.json");
        atomic_write(&p, b"{}").expect("write");
        assert_eq!(fs::read(&p).expect("read"), b"{}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn tmp_path_is_sibling_of_target() {
        let p = Path::new("/some/dir/workspace.json");
        let tmp = tmp_path_for(p);
        assert_eq!(tmp.parent(), p.parent(), "temp must share the target's dir");
        assert_eq!(
            tmp.file_name().unwrap().to_string_lossy(),
            "workspace.json.tmp"
        );
    }

    #[test]
    fn owner_only_writes_content_and_leaves_no_tmp() {
        let p = scratch("owner-only.json");
        let _ = fs::remove_file(&p);
        atomic_write_owner_only(&p, b"{\"cwd\":\"/home/alice/proj\"}").expect("write");
        assert_eq!(
            fs::read(&p).expect("read"),
            b"{\"cwd\":\"/home/alice/proj\"}"
        );

        // Same never-corrupt contract: no sibling .tmp left on success.
        let tmp = tmp_path_for(&p);
        assert!(!tmp.exists(), "temp file {tmp:?} must not remain");

        let _ = fs::remove_file(&p);
    }

    #[cfg(unix)]
    #[test]
    fn owner_only_is_0600_on_unix() {
        use std::os::unix::fs::PermissionsExt;
        let p = scratch("owner-only-mode.json");
        let _ = fs::remove_file(&p);
        // Workspace files record per-pane cwd → must be owner-only.
        atomic_write_owner_only(&p, b"workspace with /home/alice/secret cwd").expect("write");
        let mode = fs::metadata(&p).expect("stat").permissions().mode();
        assert_eq!(
            mode & 0o777,
            0o600,
            "owner-only workspace file must be 0600, got {:o}",
            mode & 0o777
        );
        let _ = fs::remove_file(&p);
    }
}
