//! Crash-loop recovery guard for OS-composited "risky" window modes.
//!
//! Applying a layered-window attribute (`WS_EX_LAYERED` +
//! `SetLayeredWindowAttributes`, the `Dim` path) OR a DWM backdrop class
//! (acrylic / mica / vibrancy, the `Glass`/`Mica`/`Vibrancy` path) to a **live
//! Vulkan flip-model DXGI swapchain** is a documented incompatibility that can
//! make DWM stop compositing the window → a black or frozen window on some
//! GPU / driver / Windows combos. Because the window mode PERSISTS in config and
//! is re-applied at startup, an affected user would open a dead window on EVERY
//! launch with no way out (the settings gear is inside the dead window).
//!
//! This implements the standard "safe-start" (crash-loop recovery) pattern:
//!
//! 1. **Arm** — BEFORE applying a risky mode, write a small sentinel marker file
//!    under the config dir recording which mode is being attempted.
//! 2. **Disarm** — once the app has rendered successfully for a short
//!    steady-state window ([`STEADY_STATE_FRAMES`] successful `update()` frames,
//!    ~1.5 s), delete the marker.
//! 3. **Recover** — on the NEXT startup, if the marker is STILL present, the
//!    previous risky-mode launch never reached steady state (the user
//!    force-killed a black window), so auto-revert `window_mode` to the safe
//!    per-pixel [`WindowMode::Transparent`], persist that, clear the marker, and
//!    log a one-line notice.
//!
//! The pure decision core ([`decide`] + [`is_risky_os_composited`]) is
//! window-free and unit-tested; the tiny filesystem helpers ([`is_armed`],
//! [`arm`], [`disarm`]) take an explicit directory so a test drives the full
//! set → clear → revert state machine against a `tempfile::TempDir`.

use std::path::Path;

use c0pl4nd_core::config::WindowMode;

/// Number of successful rendered `update()` frames after which a risky-mode
/// launch is considered STABLE and the sentinel is cleared. ~1.5 s at 40 fps —
/// long enough that a window which never composites (the black-window failure)
/// is force-killed by the user well before this, leaving the marker set.
pub(crate) const STEADY_STATE_FRAMES: u32 = 60;

/// The sentinel marker file name, created inside the config dir alongside
/// `config.toml`.
const MARKER_FILE: &str = "risky-window-mode.pending";

/// What the startup guard should do for the configured mode, given whether a
/// stale sentinel from a previous launch is present.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StartupDecision {
    /// Not an OS-composited risky mode — apply it normally; nothing to arm.
    Proceed,
    /// Risky mode with NO stale marker — arm the sentinel, then apply the
    /// effect. The frame loop disarms it after [`STEADY_STATE_FRAMES`].
    Arm,
    /// Risky mode WITH a stale marker — the previous launch never stabilised, so
    /// revert to [`WindowMode::Transparent`] and do NOT apply the risky effect.
    Revert,
}

/// Whether `mode` reaches the window via an OS-composited path (a layered window
/// or a DWM backdrop class) that can stop DWM compositing on an incompatible
/// GPU / driver / Windows combo. `Dim` uses `SetLayeredWindowAttributes`;
/// `Glass`/`Mica`/`Vibrancy` set a DWM backdrop via `window-vibrancy`. The
/// portable [`WindowMode::Transparent`] (plain per-pixel alpha) and
/// [`WindowMode::Opaque`] never touch that path and are always safe.
pub(crate) fn is_risky_os_composited(mode: WindowMode) -> bool {
    matches!(
        mode,
        WindowMode::Dim | WindowMode::Glass | WindowMode::Mica | WindowMode::Vibrancy
    )
}

/// The pure startup decision: combine "is a stale marker present" with the
/// configured mode. No filesystem, no window — the whole state machine's logic
/// lives here so it is exhaustively unit-testable.
pub(crate) fn decide(marker_present: bool, mode: WindowMode) -> StartupDecision {
    if !is_risky_os_composited(mode) {
        StartupDecision::Proceed
    } else if marker_present {
        StartupDecision::Revert
    } else {
        StartupDecision::Arm
    }
}

/// The absolute sentinel path inside `config_dir`.
fn marker_path(config_dir: &Path) -> std::path::PathBuf {
    config_dir.join(MARKER_FILE)
}

/// Whether a sentinel from a previous (not-yet-stabilised) risky-mode launch is
/// present in `config_dir`.
pub(crate) fn is_armed(config_dir: &Path) -> bool {
    marker_path(config_dir).exists()
}

/// Arm the sentinel for `mode`: write the marker file (best-effort — a marker
/// that can't be written just disables recovery for this launch, never blocks
/// it). The body records the attempted mode for diagnostics. Creates the config
/// dir if it does not yet exist.
pub(crate) fn arm(config_dir: &Path, mode: WindowMode) {
    let _ = std::fs::create_dir_all(config_dir);
    let body = format!("attempting {mode:?} at startup\n");
    if let Err(e) = std::fs::write(marker_path(config_dir), body) {
        tracing::warn!("could not arm window-mode recovery sentinel: {e}");
    }
}

/// Disarm the sentinel: delete the marker (the launch reached steady state).
/// Best-effort and idempotent — an already-absent marker is success.
pub(crate) fn disarm(config_dir: &Path) {
    match std::fs::remove_file(marker_path(config_dir)) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => tracing::warn!("could not clear window-mode recovery sentinel: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn risky_modes_are_exactly_the_os_composited_ones() {
        // The layered-window / DWM-backdrop modes are risky…
        for m in [
            WindowMode::Dim,
            WindowMode::Glass,
            WindowMode::Mica,
            WindowMode::Vibrancy,
        ] {
            assert!(is_risky_os_composited(m), "{m:?} must be guarded");
        }
        // …the plain per-pixel / opaque surfaces never touch that path.
        assert!(!is_risky_os_composited(WindowMode::Transparent));
        assert!(!is_risky_os_composited(WindowMode::Opaque));
    }

    #[test]
    fn decide_proceeds_for_safe_modes_regardless_of_marker() {
        // A safe mode is applied normally whether or not a (stale, unrelated)
        // marker happens to be present — recovery never touches Transparent.
        assert_eq!(
            decide(false, WindowMode::Transparent),
            StartupDecision::Proceed
        );
        assert_eq!(
            decide(true, WindowMode::Transparent),
            StartupDecision::Proceed
        );
        assert_eq!(decide(true, WindowMode::Opaque), StartupDecision::Proceed);
    }

    #[test]
    fn decide_arms_a_risky_mode_on_a_clean_start() {
        // First launch of a risky mode (no marker) → arm the sentinel.
        assert_eq!(decide(false, WindowMode::Dim), StartupDecision::Arm);
        assert_eq!(decide(false, WindowMode::Glass), StartupDecision::Arm);
    }

    #[test]
    fn decide_reverts_a_risky_mode_when_marker_survives() {
        // A marker that survived the previous launch means it never stabilised
        // (black window force-killed) → revert to the safe mode.
        assert_eq!(decide(true, WindowMode::Dim), StartupDecision::Revert);
        assert_eq!(decide(true, WindowMode::Vibrancy), StartupDecision::Revert);
    }

    #[test]
    fn full_state_machine_set_clear_and_revert() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path();

        // Clean start: nothing armed → a risky mode arms.
        assert!(!is_armed(path), "fresh dir has no sentinel");
        assert_eq!(
            decide(is_armed(path), WindowMode::Dim),
            StartupDecision::Arm
        );

        // Arm it (simulating the startup guard applying the risky effect).
        arm(path, WindowMode::Dim);
        assert!(is_armed(path), "sentinel present after arm");
        // The body records the attempted mode for diagnostics.
        let body = std::fs::read_to_string(marker_path(path)).expect("marker readable");
        assert!(body.contains("Dim"), "marker records the attempted mode");

        // A NEXT launch that still sees the marker must REVERT (the previous
        // launch never disarmed → it never reached steady state).
        assert_eq!(
            decide(is_armed(path), WindowMode::Dim),
            StartupDecision::Revert
        );

        // Disarm (steady state reached, OR the revert path clears it).
        disarm(path);
        assert!(!is_armed(path), "sentinel cleared after disarm");

        // Idempotent: clearing an already-clear sentinel is a no-op, not an error.
        disarm(path);
        assert!(!is_armed(path));

        // After the clear, a subsequent risky launch arms cleanly again — the
        // guard is self-healing, not a permanent lock-out.
        assert_eq!(
            decide(is_armed(path), WindowMode::Dim),
            StartupDecision::Arm
        );
    }

    #[test]
    fn arm_creates_missing_config_dir() {
        // The config dir may not exist on a fresh install; arm() must create it
        // rather than silently failing to write the sentinel.
        let root = tempfile::tempdir().expect("tempdir");
        let nested = root.path().join("does").join("not").join("exist");
        assert!(!nested.exists());
        arm(&nested, WindowMode::Glass);
        assert!(
            is_armed(&nested),
            "arm created the dir and wrote the sentinel"
        );
    }
}
