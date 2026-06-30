//! Owner-only filesystem permission tightening for user-private state files.
//!
//! Persisted state that can reflect the user's environment — the config file,
//! and saved workspace layouts (which record each pane's `cwd`, revealing
//! usernames and project paths) — should not be readable by other local
//! accounts. [`restrict_to_owner`] applies the platform's owner-only access
//! model: `0600` on Unix, an inheritance-stripped owner-only ACL on Windows.
//!
//! This is the single source of truth shared by [`crate::config`] and
//! [`crate::atomic_write`] so the config file and the workspace files are
//! tightened identically.

use std::path::Path;

/// Best-effort tighten `path` to **owner-only** access: `0600` on Unix, an
/// inheritance-stripped owner-only ACL on Windows. State files can reflect the
/// user's environment (e.g. a saved workspace records each pane's `cwd`), so
/// other local accounts should not read them.
///
/// Failure is intentionally swallowed — a restrictive / locked-down filesystem
/// must never block a state save (the write already succeeded by the time this
/// is called). The permission tightening is defense-in-depth, not a gate.
pub fn restrict_to_owner(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // 0600 = owner read/write, no group/other.
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    #[cfg(windows)]
    {
        // The file lives under the per-user `%APPDATA%` profile, whose NTFS ACLs
        // already deny other standard users; as defense-in-depth we additionally
        // remove inheritance and grant only the current user, best-effort via
        // `icacls`. Output is discarded and any failure is ignored.
        //
        // PERF: `icacls` is a blocking subprocess spawn (process creation +
        // Defender scan + NTFS ACL rewrite) costing tens-to-hundreds of ms. The
        // ACL is a stable property of the FILE, not the write — once tightened it
        // stays tightened, so re-running it on every save (e.g. a view-mode
        // toggle, which persists config on the UI frame) added that latency to
        // each save for no benefit. Tighten each path at most ONCE per process;
        // subsequent saves of the same file skip the spawn entirely.
        if already_tightened_this_process(path) {
            return;
        }
        if let Ok(user) = std::env::var("USERNAME") {
            if !user.is_empty() {
                let _ = std::process::Command::new("icacls")
                    .arg(path)
                    .arg("/inheritance:r")
                    .arg("/grant:r")
                    .arg(format!("{user}:F"))
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .status();
            }
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = path;
    }
}

/// Whether `path`'s owner-only ACL has already been applied in this process.
/// Returns `true` (and records nothing new) if the path was tightened before;
/// returns `false` the first time, recording it so the costly `icacls` spawn
/// runs at most once per file per process. Best-effort: a poisoned lock falls
/// through to running the tighten (correctness over the perf optimisation).
///
/// Cross-platform (not `#[cfg(windows)]`) ON PURPOSE: the cache DECISION is pure
/// logic and is unit-tested on every OS, so a regression in it is caught by the
/// mutation/coverage gates. Only the `icacls` spawn it guards is Windows-specific
/// — that lives in [`restrict_to_owner`]'s `#[cfg(windows)]` block, which is the
/// sole caller, so the fn is dead code on non-Windows builds (allowed below).
#[cfg_attr(not(windows), allow(dead_code))]
fn already_tightened_this_process(path: &Path) -> bool {
    use std::collections::HashSet;
    use std::path::PathBuf;
    use std::sync::{Mutex, OnceLock};
    static SEEN: OnceLock<Mutex<HashSet<PathBuf>>> = OnceLock::new();
    let cell = SEEN.get_or_init(|| Mutex::new(HashSet::new()));
    match cell.lock() {
        // `insert` returns false when the value was already present.
        Ok(mut seen) => !seen.insert(path.to_path_buf()),
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restrict_to_owner_is_owner_only_and_does_not_error() {
        let p = std::env::temp_dir().join(format!(
            "c0pl4nd-fsperms-{}-{}.bin",
            std::process::id(),
            "owner"
        ));
        std::fs::write(&p, b"secret cwd /home/alice/project").expect("seed");

        // Must not panic / error regardless of platform.
        restrict_to_owner(&p);

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&p).expect("stat").permissions().mode();
            assert_eq!(
                mode & 0o777,
                0o600,
                "file must be owner-only (0600), got {:o}",
                mode & 0o777
            );
        }

        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn restrict_to_owner_missing_path_is_silent() {
        // A nonexistent path must not panic — best-effort contract.
        let p = std::env::temp_dir().join(format!(
            "c0pl4nd-fsperms-{}-{}.bin",
            std::process::id(),
            "missing"
        ));
        let _ = std::fs::remove_file(&p);
        restrict_to_owner(&p); // no panic
    }

    /// Tightening must NOT alter the file's CONTENT — only its permissions. A
    /// caller relies on `restrict_to_owner` being a pure permission op so the
    /// just-written state survives verbatim.
    #[test]
    fn restrict_to_owner_preserves_content() {
        let p = std::env::temp_dir().join(format!(
            "c0pl4nd-fsperms-{}-{}.bin",
            std::process::id(),
            "content"
        ));
        let payload = b"workspace cwd=/home/alice/secret-project layout=v2";
        std::fs::write(&p, payload).expect("seed");
        restrict_to_owner(&p);
        assert_eq!(
            std::fs::read(&p).expect("read"),
            payload,
            "restrict_to_owner must leave the file content unchanged"
        );
        let _ = std::fs::remove_file(&p);
    }

    /// The per-process tighten cache must return `false` the FIRST time it sees a
    /// path (so the caller runs the costly `icacls` spawn once) and `true` on
    /// every subsequent call for the SAME path (so the spawn is skipped). A
    /// distinct path is independent. Kills the mutation-gate survivors that
    /// replace the body with a constant `true`/`false` or drop the `!`.
    #[test]
    fn already_tightened_this_process_is_false_first_then_true_per_path() {
        // Unique per test run so the process-global cache can't be pre-seeded by
        // another test (the set is never cleared within a process).
        let a = std::env::temp_dir().join(format!(
            "c0pl4nd-tighten-cache-{}-{}-A.bin",
            std::process::id(),
            line!()
        ));
        let b = std::env::temp_dir().join(format!(
            "c0pl4nd-tighten-cache-{}-{}-B.bin",
            std::process::id(),
            line!()
        ));

        // First sighting of `a`: not yet tightened → caller SHOULD run the op.
        assert!(
            !already_tightened_this_process(&a),
            "first call for a path must report NOT-yet-tightened (false)"
        );
        // Second sighting of the SAME path: already recorded → caller SKIPS the op.
        assert!(
            already_tightened_this_process(&a),
            "second call for the same path must report already-tightened (true)"
        );
        // A DIFFERENT path is tracked independently → first sighting is false.
        assert!(
            !already_tightened_this_process(&b),
            "a distinct path must be independent (first call false)"
        );
        // …and it too flips to true once recorded.
        assert!(
            already_tightened_this_process(&b),
            "the distinct path must also report true on its second call"
        );
    }

    /// A directory target is also handled best-effort without panic (the
    /// permission op applies to any path the OS accepts; a dir is a valid path).
    #[test]
    fn restrict_to_owner_on_directory_is_silent() {
        let d = std::env::temp_dir().join(format!(
            "c0pl4nd-fsperms-dir-{}-{}",
            std::process::id(),
            "owner"
        ));
        std::fs::create_dir_all(&d).expect("mkdir");
        restrict_to_owner(&d); // must not panic on a directory
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&d).expect("stat").permissions().mode();
            assert_eq!(mode & 0o777, 0o600, "0600 applies to the dir path on unix");
        }
        let _ = std::fs::remove_dir_all(&d);
    }
}
