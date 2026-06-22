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
