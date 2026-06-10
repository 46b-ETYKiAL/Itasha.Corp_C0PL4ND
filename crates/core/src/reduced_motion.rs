//! "Reduce motion" preference detection (F2-2, WCAG 2.3.3).
//!
//! C0PL4ND's CRT post-effect animates a rolling scan band. A user who has asked
//! their OS to reduce motion (vestibular comfort, focus, battery) should not be
//! subjected to that animation. Previously the only signal was the
//! `C0PL4ND_REDUCED_MOTION` env override; this module ADDS an OS-accessibility
//! query so the app honours the system setting out of the box.
//!
//! The OS query uses a **safe, dependency-free platform command** (no `unsafe`
//! FFI, no new crate) and is best-effort: any failure is treated as "motion
//! allowed". It is cached for the process lifetime (the OS setting does not
//! change mid-session in practice). The env override is re-read every call so it
//! always wins and stays test-controllable.

use std::sync::OnceLock;

/// Whether the user prefers reduced motion. True when the
/// `C0PL4ND_REDUCED_MOTION` env var is set to a truthy value, OR the OS
/// accessibility "reduce motion" setting is on. Animations (the CRT scan-band
/// roll) should freeze when this returns true; static visuals are unaffected.
pub fn reduced_motion() -> bool {
    if env_reduced_motion() {
        return true;
    }
    *OS_REDUCED_MOTION.get_or_init(os_reduced_motion)
}

/// The `C0PL4ND_REDUCED_MOTION` env override. Truthy = any non-empty value that
/// is not `0` / `false` (case-insensitive). Re-read every call so a relaunch or
/// a test can flip it, and so it ALWAYS overrides the OS query.
fn env_reduced_motion() -> bool {
    std::env::var("C0PL4ND_REDUCED_MOTION")
        .map(|v| is_truthy(&v))
        .unwrap_or(false)
}

/// Parse a truthy env string: non-empty AND not `0`/`false`/`no`/`off`.
fn is_truthy(v: &str) -> bool {
    let v = v.trim();
    !v.is_empty()
        && !v.eq_ignore_ascii_case("0")
        && !v.eq_ignore_ascii_case("false")
        && !v.eq_ignore_ascii_case("no")
        && !v.eq_ignore_ascii_case("off")
}

static OS_REDUCED_MOTION: OnceLock<bool> = OnceLock::new();

/// Query the OS accessibility "reduce motion" preference via a safe platform
/// command (no FFI). Best-effort: any spawn/parse failure → `false` (motion on).
///
/// - **Windows**: `HKCU\Control Panel\Desktop\WindowMetrics\MinAnimate` is the
///   REG_SZ behind `SPI_GETCLIENTAREAANIMATION`; `"0"` = window animations OFF =
///   reduce motion.
/// - **macOS**: `defaults read com.apple.universalaccess reduceMotion` → `1`.
/// - **Linux (GNOME-family)**: `gsettings get org.gnome.desktop.interface
///   enable-animations` → `false` = reduce motion.
fn os_reduced_motion() -> bool {
    #[cfg(target_os = "windows")]
    {
        // The last whitespace-delimited token of the `reg query` output line is
        // the value (e.g. `MinAnimate    REG_SZ    0`).
        query_cmd(
            "reg",
            &[
                "query",
                r"HKCU\Control Panel\Desktop\WindowMetrics",
                "/v",
                "MinAnimate",
            ],
        )
        .map(|o| o.split_whitespace().last() == Some("0"))
        .unwrap_or(false)
    }
    #[cfg(target_os = "macos")]
    {
        query_cmd(
            "defaults",
            &["read", "com.apple.universalaccess", "reduceMotion"],
        )
        .map(|o| o.trim() == "1")
        .unwrap_or(false)
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        query_cmd(
            "gsettings",
            &["get", "org.gnome.desktop.interface", "enable-animations"],
        )
        .map(|o| o.trim() == "false")
        .unwrap_or(false)
    }
    #[cfg(not(any(
        target_os = "windows",
        target_os = "macos",
        all(unix, not(target_os = "macos"))
    )))]
    {
        false
    }
}

/// Run a query command and return its stdout on success; `None` on any failure
/// (binary absent, non-zero exit, non-UTF8). Never panics; never writes.
#[cfg_attr(
    not(any(
        target_os = "windows",
        target_os = "macos",
        all(unix, not(target_os = "macos"))
    )),
    allow(dead_code)
)]
fn query_cmd(prog: &str, args: &[&str]) -> Option<String> {
    let out = std::process::Command::new(prog).args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truthy_parsing_covers_the_common_forms() {
        // The env-override truthiness contract (the load-bearing pure logic).
        // We deliberately do NOT mutate the global `C0PL4ND_REDUCED_MOTION` env
        // var in a unit test — that is process-global state and would pollute
        // other parallel tests (and `set_var` is `unsafe` under edition 2024).
        for on in ["1", "true", "TRUE", "yes", "on", "  1 "] {
            assert!(is_truthy(on), "{on:?} should be truthy");
        }
        for off in ["", "  ", "0", "false", "False", "no", "off"] {
            assert!(!is_truthy(off), "{off:?} should be falsy");
        }
    }
}
