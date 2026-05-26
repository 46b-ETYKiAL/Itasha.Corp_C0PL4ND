//! Opt-in, local-first updater.
//!
//! C0PL4ND never phones home on its own. The version check only runs when the
//! user invokes `c0pl4nd update` or explicitly sets `[update] check_on_launch
//! = true`. It contacts only the public GitHub Releases API — no account, no
//! telemetry, no shell I/O ever leaves the device. The actual upgrade is
//! delegated to the OS package manager the app was installed with, which is
//! the correct path for a packaged application (winget / Homebrew / apt).

use anyhow::{Context, Result};
use std::process::Command;

/// Canonical public release repository (no internal references).
const REPO: &str = "itasha-corp/c0pl4nd";

/// The running binary's version.
pub fn current_version() -> &'static str {
    c0pl4nd_core::version()
}

/// Query GitHub Releases for the latest published version tag. Network call —
/// only invoked on explicit user action or opt-in.
pub fn latest_version() -> Result<String> {
    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let body = ureq::get(&url)
        .set("User-Agent", "c0pl4nd-updater")
        .set("Accept", "application/vnd.github+json")
        .call()
        .context("failed to reach GitHub Releases")?
        .into_string()
        .context("failed to read release response")?;
    let tag = body
        .split("\"tag_name\"")
        .nth(1)
        .and_then(|s| s.split('"').nth(2))
        .map(|s| s.trim_start_matches('v').to_string())
        .context("no tag_name in release response")?;
    Ok(tag)
}

/// Compare dotted numeric versions; true when `latest` is strictly newer.
pub fn is_newer(latest: &str, current: &str) -> bool {
    fn parts(v: &str) -> Vec<u64> {
        v.trim_start_matches('v')
            .split('.')
            .map(|p| {
                p.chars()
                    .take_while(|c| c.is_ascii_digit())
                    .collect::<String>()
            })
            .map(|s| s.parse().unwrap_or(0))
            .collect()
    }
    let (l, c) = (parts(latest), parts(current));
    for i in 0..l.len().max(c.len()) {
        let lv = l.get(i).copied().unwrap_or(0);
        let cv = c.get(i).copied().unwrap_or(0);
        if lv != cv {
            return lv > cv;
        }
    }
    false
}

/// Print a non-intrusive notice if a newer version exists (opt-in launch check).
pub fn notify_if_outdated() {
    if let Ok(latest) = latest_version() {
        if is_newer(&latest, current_version()) {
            eprintln!(
                "C0PL4ND {latest} is available (you have {}). Run `c0pl4nd update`.",
                current_version()
            );
        }
    }
}

/// Perform the upgrade through the OS package manager. Explicit user action.
pub fn run_update() -> Result<()> {
    println!("C0PL4ND updater — current version {}", current_version());
    match latest_version() {
        Ok(latest) if is_newer(&latest, current_version()) => {
            println!("Newer version available: {latest}. Upgrading via package manager...");
        }
        Ok(latest) => {
            println!("Already up to date ({latest}).");
            return Ok(());
        }
        Err(e) => {
            eprintln!("Could not check latest version: {e}");
        }
    }

    let status = invoke_package_manager()?;
    if status {
        println!("Upgrade command completed.");
    } else {
        println!(
            "Automatic upgrade is handled by your package manager. \
             See https://github.com/{REPO} for install/upgrade instructions."
        );
    }
    Ok(())
}

/// Invoke the platform package manager. Returns true if a manager was found.
fn invoke_package_manager() -> Result<bool> {
    #[cfg(windows)]
    {
        let ok = Command::new("winget")
            .args(["upgrade", "--id", "Itasha.C0PL4ND", "-e", "--silent"])
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        return Ok(ok);
    }
    #[cfg(target_os = "macos")]
    {
        let ok = Command::new("brew")
            .args(["upgrade", "--cask", "c0pl4nd"])
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        return Ok(ok);
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        // Try apt; otherwise fall back to printed instructions.
        let ok = Command::new("sh")
            .args(["-c", "command -v apt >/dev/null 2>&1 && sudo apt update && sudo apt install --only-upgrade c0pl4nd"])
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        return Ok(ok);
    }
    #[allow(unreachable_code)]
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_comparison() {
        assert!(is_newer("1.2.0", "1.1.9"));
        assert!(is_newer("0.2.0", "0.1.0"));
        assert!(is_newer("v1.0.1", "1.0.0"));
        assert!(!is_newer("1.0.0", "1.0.0"));
        assert!(!is_newer("0.9.0", "1.0.0"));
    }

    #[test]
    fn current_version_nonempty() {
        assert!(!current_version().is_empty());
    }
}
