//! Detected shell profiles for the C0PL4ND top-bar shell switcher (#23).
//!
//! A [`ShellProfile`] names a launchable shell. The default profile (program
//! `None`) spawns the platform default shell via [`super::PaneTerm::spawn`]; a
//! named profile spawns its `program` + `args` via
//! [`super::PaneTerm::spawn_program`]. The titlebar's "new terminal" ▾ menu
//! lists these so a user can open a pane running PowerShell, cmd, WSL, bash, …
//! without editing config (the user's "run things other than PowerShell — an
//! easy switch like a dropdown in the top bar" request).
//!
//! Detection is best-effort and side-effect-free: each candidate is probed
//! against `PATH` (a pure filesystem check — it NEVER executes the shell), and
//! only the ones that resolve are offered, so the menu never lists a shell that
//! cannot launch. The platform default is ALWAYS first, so there is always at
//! least one entry even on a bare machine.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// A launchable shell the user can pick from the top-bar switcher.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellProfile {
    /// Human label shown in the menu (e.g. "PowerShell 7", "Command Prompt").
    pub label: String,
    /// The program to launch. `None` means the platform default shell
    /// ([`super::PaneTerm::spawn`]); `Some(path)` means
    /// [`super::PaneTerm::spawn_program`].
    pub program: Option<String>,
    /// Arguments passed to `program` (empty for a bare interactive shell).
    pub args: Vec<String>,
}

impl ShellProfile {
    /// The platform default shell entry (program `None`).
    fn default_profile() -> Self {
        Self {
            label: "Default shell".to_string(),
            program: None,
            args: Vec::new(),
        }
    }

    /// A named profile bound to an explicit program + args.
    fn named(label: &str, program: &str, args: &[&str]) -> Self {
        Self {
            label: label.to_string(),
            program: Some(program.to_string()),
            args: args.iter().map(|a| (*a).to_string()).collect(),
        }
    }
}

/// Resolve `program` against `PATH`, returning its full path if found.
///
/// An absolute/explicit path is accepted iff it exists. A bare name is probed
/// in each `PATH` directory; on Windows a name without an extension is also
/// probed with each `PATHEXT` extension (defaulting to the standard set). This
/// is a pure filesystem probe — it never spawns the program.
fn which(program: &str) -> Option<PathBuf> {
    let p = Path::new(program);
    if p.is_absolute() {
        return p.is_file().then(|| p.to_path_buf());
    }
    let path = std::env::var_os("PATH")?;
    let win_exts: Vec<String> = if cfg!(windows) {
        std::env::var("PATHEXT")
            .unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".to_string())
            .split(';')
            .map(|e| e.trim().to_string())
            .filter(|e| !e.is_empty())
            .collect()
    } else {
        Vec::new()
    };
    for dir in std::env::split_paths(&path) {
        // The name as-given (covers a name that already carries an extension).
        let direct = dir.join(program);
        if direct.is_file() {
            return Some(direct);
        }
        if cfg!(windows) && p.extension().is_none() {
            for ext in &win_exts {
                let cand = dir.join(format!("{program}{ext}"));
                if cand.is_file() {
                    return Some(cand);
                }
            }
        }
    }
    None
}

/// Detect the shells available on this machine, platform default first.
///
/// The returned list is deduplicated by program (case-insensitive) so a shell
/// reachable under two names is offered once. The first entry is always the
/// platform default (program `None`).
pub fn detect_profiles() -> Vec<ShellProfile> {
    let mut out = vec![ShellProfile::default_profile()];

    #[cfg(windows)]
    {
        // (label, program, args). cmd.exe is essentially always present; the
        // rest are offered only when they resolve on PATH. `bash.exe` is
        // intentionally omitted on Windows because System32\bash.exe is the WSL
        // launcher (a duplicate of the WSL entry) — Git Bash users can set an
        // explicit shell in settings.
        let candidates: [(&str, &str, &[&str]); 4] = [
            ("PowerShell 7", "pwsh.exe", &[]),
            ("Windows PowerShell", "powershell.exe", &[]),
            ("Command Prompt", "cmd.exe", &[]),
            ("WSL", "wsl.exe", &[]),
        ];
        for (label, prog, args) in candidates {
            if which(prog).is_some() {
                out.push(ShellProfile::named(label, prog, args));
            }
        }
    }
    #[cfg(not(windows))]
    {
        // Common interactive shells; include only those present. We bind to the
        // resolved absolute path so the PTY launch does not re-search PATH.
        let candidates: [(&str, &str, &[&str]); 5] = [
            ("Bash", "bash", &[]),
            ("Zsh", "zsh", &[]),
            ("Fish", "fish", &[]),
            ("Nushell", "nu", &[]),
            ("POSIX sh", "sh", &[]),
        ];
        for (label, prog, args) in candidates {
            if let Some(path) = which(prog) {
                out.push(ShellProfile::named(label, &path.to_string_lossy(), args));
            }
        }
    }

    dedup_by_program(out)
}

/// Keep the first profile for each distinct program (case-insensitive). The
/// default profile's `None` program is its own unique key, so it is preserved.
fn dedup_by_program(profiles: Vec<ShellProfile>) -> Vec<ShellProfile> {
    let mut seen: HashSet<Option<String>> = HashSet::new();
    profiles
        .into_iter()
        .filter(|p| seen.insert(p.program.as_deref().map(str::to_lowercase)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_profiles_always_starts_with_the_default() {
        let profiles = detect_profiles();
        assert!(!profiles.is_empty(), "there is always at least the default");
        assert!(
            profiles[0].program.is_none(),
            "the first profile must be the platform default (program None)"
        );
        assert_eq!(profiles[0].label, "Default shell");
    }

    #[test]
    fn which_returns_none_for_a_bogus_binary() {
        assert!(
            which("c0pl4nd-definitely-not-a-real-binary-xyz").is_none(),
            "a name that is not on PATH must not resolve"
        );
    }

    #[test]
    fn dedup_keeps_the_first_profile_per_program() {
        let input = vec![
            ShellProfile::default_profile(),
            ShellProfile::named("Aye", "dup.exe", &[]),
            ShellProfile::named("Bee", "DUP.EXE", &["-x"]), // same program, different case
            ShellProfile::named("Cee", "other.exe", &[]),
        ];
        let out = dedup_by_program(input);
        assert_eq!(out.len(), 3, "the case-duplicate program is collapsed");
        assert_eq!(out[1].label, "Aye", "first occurrence of dup.exe wins");
        assert_eq!(out[2].label, "Cee");
    }

    #[test]
    fn named_profile_carries_program_and_args() {
        let p = ShellProfile::named("X", "wsl.exe", &["-d", "Ubuntu"]);
        assert_eq!(p.program.as_deref(), Some("wsl.exe"));
        assert_eq!(p.args, vec!["-d".to_string(), "Ubuntu".to_string()]);
    }

    #[test]
    fn detected_named_profiles_are_unique_by_program() {
        let profiles = detect_profiles();
        let mut programs: Vec<_> = profiles.iter().filter_map(|p| p.program.clone()).collect();
        let len = programs.len();
        programs.sort();
        programs.dedup();
        assert_eq!(
            len,
            programs.len(),
            "detected profiles must not duplicate a program"
        );
    }

    #[test]
    fn which_resolves_a_bare_name_against_a_synthetic_path() {
        // The happy path: a bare program name present in a directory on a
        // synthetic PATH resolves to the full path. We create our own file so
        // the test does not depend on any real installed binary.
        let dir = tempfile::tempdir().unwrap();
        // On Windows the loader searches PATHEXT extensions for a bare name, so
        // give the file a `.EXE` and probe the extension-less name.
        let (file_name, probe_name) = if cfg!(windows) {
            ("mytool.exe", "mytool")
        } else {
            ("mytool", "mytool")
        };
        let exe = dir.path().join(file_name);
        std::fs::write(&exe, b"#!/bin/sh\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&exe).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&exe, perms).unwrap();
        }
        // Run `which` with a PATH that contains ONLY our temp dir. Guarded by an
        // env lock so concurrent tests do not clobber PATH/PATHEXT.
        let _lock = PATH_ENV_LOCK.lock().unwrap();
        let prev_path = std::env::var_os("PATH");
        let prev_pathext = std::env::var_os("PATHEXT");
        std::env::set_var("PATH", dir.path());
        if cfg!(windows) {
            std::env::set_var("PATHEXT", ".EXE");
        }
        let resolved = which(probe_name);
        // Restore env before asserting (so a failed assert does not leak state).
        match prev_path {
            Some(v) => std::env::set_var("PATH", v),
            None => std::env::remove_var("PATH"),
        }
        match prev_pathext {
            Some(v) => std::env::set_var("PATHEXT", v),
            None => std::env::remove_var("PATHEXT"),
        }
        let resolved = resolved.expect("the bare name resolves on the synthetic PATH");
        // On Windows the resolver appends the PATHEXT entry verbatim (`.EXE`),
        // and the filesystem is case-insensitive, so compare case-folded.
        assert_eq!(
            resolved
                .file_name()
                .and_then(|n| n.to_str())
                .map(str::to_ascii_lowercase),
            Some(file_name.to_ascii_lowercase()),
            "which returns the full path to the matched file (case-insensitive on Windows)"
        );
        assert!(resolved.is_file(), "the resolved path is the real file");
    }

    #[test]
    fn which_accepts_an_existing_absolute_path_and_rejects_a_missing_one() {
        // An absolute path is accepted IFF it exists; a non-existent absolute
        // path is rejected. This covers the `p.is_absolute()` early branch.
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("abs-tool");
        std::fs::write(&exe, b"x").unwrap();
        let abs = exe.to_string_lossy().to_string();
        assert_eq!(
            which(&abs).as_deref(),
            Some(exe.as_path()),
            "an existing absolute path resolves to itself"
        );
        let missing = dir.path().join("does-not-exist-abs");
        assert!(
            which(&missing.to_string_lossy()).is_none(),
            "a non-existent absolute path does not resolve"
        );
    }

    use std::sync::Mutex;
    static PATH_ENV_LOCK: Mutex<()> = Mutex::new(());
}
