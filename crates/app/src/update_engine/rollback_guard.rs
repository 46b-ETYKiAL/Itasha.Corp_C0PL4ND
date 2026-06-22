//! Anti-rollback (version-downgrade) protection for the in-app self-updater.
//!
//! ## Why integrity is not enough
//!
//! [`super::verify`] proves an artifact's INTEGRITY (SHA-256) and AUTHENTICITY
//! (minisign against the embedded key) — it answers "is this a genuine C0PL4ND
//! release?". It does NOT answer "is this release FRESH?". A *validly signed
//! OLDER release* is a genuine artifact: an attacker who MITMs or replays the
//! GitHub Releases listing can pin the user to a signed-but-vulnerable prior
//! version (a version-rollback / BlackLotus-class downgrade). Integrity ≠
//! freshness.
//!
//! ## The monotonic rule
//!
//! This module enforces a strictly-monotonic version floor: an update is
//! applied ONLY when the candidate version is strictly greater than the
//! **baseline** — the highest version this installation has ever run. The
//! baseline is `max(compiled-in CARGO_PKG_VERSION, the persisted high-water
//! record)`, so it survives both first-run (no record yet → the compiled
//! version is the floor) and a downgrade attempt that targets a version that is
//! older than the high-water mark yet (briefly) newer than a freshly-flashed
//! binary's `CARGO_PKG_VERSION`. An equal version is a no-op (nothing to do);
//! a strictly-lower or unparseable candidate is BLOCKED, fail-closed.
//!
//! ## Defense in depth — gate at APPLY time, not just CHECK time
//!
//! [`super::net::select_update`] already rejects `latest <= current` at *check*
//! time. This module re-evaluates the rule at *apply* time, immediately before
//! the `self-replace` swap, closing the time-of-check/time-of-use window in
//! which a replayed listing or a stale staged artifact could otherwise install
//! an older binary. The two checks are deliberately redundant: the check-time
//! one is UX (don't offer a downgrade), the apply-time one is the security
//! boundary (don't INSTALL a downgrade).
//!
//! ## Persistence
//!
//! The high-water record is a single line — the highest installed semver — in
//! `<exe-dir>/.c0pl4nd-installed-version`, written next to the running
//! executable (the same directory the keep-one-prior `.c0pl4nd-bak` backup
//! already uses). No new dependency: plain `std::fs` text I/O. Reads are
//! tolerant (a missing/corrupt/empty record simply means "no record" and the
//! compiled version becomes the floor); the record is only ever advanced
//! upward, never lowered, so a tampered low value cannot weaken the floor below
//! the compiled-in version.

use std::path::{Path, PathBuf};

use semver::Version;

/// File name of the high-water "highest version ever installed" record, stored
/// next to the running executable.
const INSTALLED_VERSION_FILE: &str = ".c0pl4nd-installed-version";

/// The outcome of the anti-rollback evaluation for a candidate update.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RollbackDecision {
    /// The candidate is strictly newer than the baseline — safe to apply.
    Allow,
    /// The candidate equals the baseline — already installed, nothing to do.
    NoOp,
    /// The candidate is older than the baseline (a downgrade) — REFUSED.
    /// Carries `(candidate, baseline)` so the UI can render a clear reason.
    Blocked { candidate: String, baseline: String },
    /// The candidate version string could not be parsed — REFUSED, fail-closed.
    /// An unparseable version is never silently treated as "newer".
    Malformed { candidate: String },
}

impl RollbackDecision {
    /// True only for [`RollbackDecision::Allow`] — the single state in which the
    /// swap may proceed. `NoOp`, `Blocked`, and `Malformed` all stop the apply.
    pub fn may_apply(&self) -> bool {
        matches!(self, RollbackDecision::Allow)
    }

    /// A human-readable, single-line reason for a non-`Allow` outcome, suitable
    /// for surfacing in the Updates pane. Returns `None` for [`Self::Allow`].
    pub fn reason(&self) -> Option<String> {
        match self {
            RollbackDecision::Allow => None,
            RollbackDecision::NoOp => {
                Some("already up to date — no newer version to install".to_string())
            }
            RollbackDecision::Blocked {
                candidate,
                baseline,
            } => Some(format!(
                "downgrade blocked: {candidate} < {baseline} (refusing to install an older, \
                 signed-but-superseded version)"
            )),
            RollbackDecision::Malformed { candidate } => Some(format!(
                "downgrade blocked: candidate version {candidate:?} is unparseable \
                 (refusing to install — fail-closed)"
            )),
        }
    }
}

/// The pure anti-rollback decision: compare a parsed `candidate` against a
/// parsed `baseline`. Strictly-greater → [`RollbackDecision::Allow`]; equal →
/// [`RollbackDecision::NoOp`]; strictly-less → [`RollbackDecision::Blocked`].
///
/// This is the load-bearing rule, split out so it is unit-testable without any
/// filesystem or version-string parsing.
pub fn decide(candidate: &Version, baseline: &Version) -> RollbackDecision {
    use std::cmp::Ordering;
    match candidate.cmp(baseline) {
        Ordering::Greater => RollbackDecision::Allow,
        Ordering::Equal => RollbackDecision::NoOp,
        Ordering::Less => RollbackDecision::Blocked {
            candidate: candidate.to_string(),
            baseline: baseline.to_string(),
        },
    }
}

/// Evaluate a candidate version STRING (as carried in
/// [`super::updater::UpdateState::ReadyToApply`]) against `baseline`. A
/// candidate that does not parse as semver is [`RollbackDecision::Malformed`]
/// (fail-closed) — it is NEVER treated as newer.
pub fn evaluate(candidate_version: &str, baseline: &Version) -> RollbackDecision {
    match Version::parse(candidate_version.trim().trim_start_matches('v')) {
        Ok(candidate) => decide(&candidate, baseline),
        Err(_) => RollbackDecision::Malformed {
            candidate: candidate_version.to_string(),
        },
    }
}

/// The compiled-in version of the running build (authoritative lower bound on
/// the freshness floor — a build can never be older than itself).
fn compiled_version() -> Version {
    // CARGO_PKG_VERSION is a valid semver by Cargo's own contract; if a future
    // exotic value ever failed to parse, fall back to 0.0.0 so the persisted
    // record still governs (never panic in the apply path).
    Version::parse(env!("CARGO_PKG_VERSION")).unwrap_or_else(|_| Version::new(0, 0, 0))
}

/// Path to the high-water record next to `exe`.
fn record_path_for(exe: &Path) -> PathBuf {
    exe.with_file_name(INSTALLED_VERSION_FILE)
}

/// Read the persisted high-water version from the record next to `exe`, if any.
/// Tolerant: a missing/unreadable/empty/corrupt record yields `None` (the
/// compiled version then becomes the floor). Never errors — a record problem
/// must never block a legitimate update nor be mistaken for a low floor.
fn read_record(exe: &Path) -> Option<Version> {
    let text = std::fs::read_to_string(record_path_for(exe)).ok()?;
    Version::parse(text.trim().trim_start_matches('v')).ok()
}

/// The freshness baseline: `max(compiled-in version, persisted high-water
/// record)`. Pure given the two inputs so it is unit-testable; the
/// filesystem-reading wrapper is [`installed_baseline`].
fn baseline_from(compiled: Version, record: Option<Version>) -> Version {
    match record {
        Some(r) if r > compiled => r,
        _ => compiled,
    }
}

/// The freshness baseline for the running installation, reading the high-water
/// record next to the current executable. Falls back to the compiled-in version
/// when `current_exe()` is unavailable or no record exists.
pub fn installed_baseline() -> Version {
    let compiled = compiled_version();
    let record = std::env::current_exe()
        .ok()
        .and_then(|exe| read_record(&exe));
    baseline_from(compiled, record)
}

/// Evaluate a candidate version string against the running installation's
/// freshness baseline (the production entry point used by the apply path).
pub fn evaluate_installed(candidate_version: &str) -> RollbackDecision {
    evaluate(candidate_version, &installed_baseline())
}

/// Advance the high-water record next to `exe` to `version` IFF it is higher
/// than the current record (monotonic — never lowers the floor). Best-effort:
/// a write failure is returned to the caller but never blocks an applied
/// update (the compiled-in version of the freshly-installed binary still
/// governs the next launch's floor). Called AFTER a successful swap.
pub fn record_installed(exe: &Path, version: &Version) -> std::io::Result<()> {
    // Only advance — never regress the high-water mark.
    if let Some(existing) = read_record(exe) {
        if existing >= *version {
            return Ok(());
        }
    }
    std::fs::write(record_path_for(exe), format!("{version}\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &str) -> Version {
        Version::parse(s).unwrap()
    }

    #[test]
    fn decide_allows_strictly_newer() {
        assert_eq!(decide(&v("1.2.0"), &v("1.1.9")), RollbackDecision::Allow);
        assert_eq!(decide(&v("2.0.0"), &v("1.9.9")), RollbackDecision::Allow);
        assert!(decide(&v("1.2.0"), &v("1.1.9")).may_apply());
    }

    #[test]
    fn decide_noops_on_equal() {
        assert_eq!(decide(&v("1.2.3"), &v("1.2.3")), RollbackDecision::NoOp);
        assert!(!decide(&v("1.2.3"), &v("1.2.3")).may_apply());
    }

    #[test]
    fn decide_blocks_strictly_older() {
        let d = decide(&v("1.0.0"), &v("1.2.0"));
        assert_eq!(
            d,
            RollbackDecision::Blocked {
                candidate: "1.0.0".to_string(),
                baseline: "1.2.0".to_string(),
            }
        );
        assert!(!d.may_apply());
        // The reason names both versions for the UI.
        let reason = d.reason().expect("blocked has a reason");
        assert!(reason.contains("downgrade blocked"));
        assert!(reason.contains("1.0.0"));
        assert!(reason.contains("1.2.0"));
    }

    #[test]
    fn evaluate_blocks_older_string_candidate() {
        let baseline = v("0.3.0");
        assert!(matches!(
            evaluate("0.2.9", &baseline),
            RollbackDecision::Blocked { .. }
        ));
        // Tolerates a leading `v`.
        assert!(matches!(
            evaluate("v0.2.0", &baseline),
            RollbackDecision::Blocked { .. }
        ));
    }

    #[test]
    fn evaluate_allows_newer_string_candidate() {
        assert_eq!(evaluate("0.4.0", &v("0.3.0")), RollbackDecision::Allow);
        assert_eq!(evaluate("v0.4.0", &v("0.3.0")), RollbackDecision::Allow);
    }

    #[test]
    fn evaluate_noops_on_equal_string_candidate() {
        assert_eq!(evaluate("0.3.0", &v("0.3.0")), RollbackDecision::NoOp);
    }

    #[test]
    fn evaluate_blocks_malformed_candidate_fail_closed() {
        let baseline = v("0.3.0");
        // A malformed version is NEVER treated as newer — it is refused.
        for bad in ["", "   ", "not-a-version", "1", "1.2", "??", "0.0.0.0"] {
            let d = evaluate(bad, &baseline);
            assert!(
                matches!(d, RollbackDecision::Malformed { .. }),
                "expected Malformed for {bad:?}, got {d:?}"
            );
            assert!(!d.may_apply(), "malformed {bad:?} must not apply");
        }
    }

    #[test]
    fn baseline_is_max_of_compiled_and_record() {
        // Record higher than compiled -> record wins (closes the re-flashed-
        // lower-CARGO_PKG_VERSION downgrade window).
        assert_eq!(
            baseline_from(v("0.3.0"), Some(v("0.5.0"))),
            v("0.5.0"),
            "a higher persisted high-water mark must raise the floor"
        );
        // Record lower than compiled -> compiled wins (a tampered low record
        // cannot weaken the floor below the running build's own version).
        assert_eq!(
            baseline_from(v("0.4.0"), Some(v("0.1.0"))),
            v("0.4.0"),
            "a low/tampered record must not lower the floor below the compiled version"
        );
        // No record -> compiled is the floor (first run).
        assert_eq!(baseline_from(v("0.4.0"), None), v("0.4.0"));
    }

    #[test]
    fn record_round_trips_and_is_monotonic() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("c0pl4nd.exe");
        // No record yet.
        assert_eq!(read_record(&exe), None);

        // Record an install.
        record_installed(&exe, &v("0.4.0")).unwrap();
        assert_eq!(read_record(&exe), Some(v("0.4.0")));

        // A higher install advances the mark.
        record_installed(&exe, &v("0.5.0")).unwrap();
        assert_eq!(read_record(&exe), Some(v("0.5.0")));

        // A LOWER (or equal) "install" must NOT lower the high-water mark.
        record_installed(&exe, &v("0.4.1")).unwrap();
        assert_eq!(
            read_record(&exe),
            Some(v("0.5.0")),
            "record must be monotonic — never regress"
        );
        record_installed(&exe, &v("0.5.0")).unwrap();
        assert_eq!(read_record(&exe), Some(v("0.5.0")));
    }

    #[test]
    fn read_record_tolerates_corrupt_file() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("c0pl4nd");
        std::fs::write(record_path_for(&exe), "garbage not a version\n").unwrap();
        // A corrupt record reads as None -> the compiled version becomes the
        // floor (never a crash, never a spuriously-low floor).
        assert_eq!(read_record(&exe), None);
    }

    #[test]
    fn evaluate_against_a_recorded_baseline_blocks_replay() {
        // Simulate the attack: the high-water mark is 0.5.0 (the user once ran
        // it), and a replayed listing offers a signed 0.4.0. The baseline read
        // from the record blocks it even though 0.4.0 is a genuine release.
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("c0pl4nd.exe");
        record_installed(&exe, &v("0.5.0")).unwrap();
        let baseline = baseline_from(v("0.3.0"), read_record(&exe));
        assert_eq!(baseline, v("0.5.0"));
        assert!(matches!(
            evaluate("0.4.0", &baseline),
            RollbackDecision::Blocked { .. }
        ));
    }

    #[test]
    fn installed_baseline_is_at_least_the_compiled_version() {
        // The production entry point reads the high-water record next to the
        // running test exe (likely absent) and falls back to the compiled
        // version. It must NEVER be below the compiled-in version — that is the
        // floor a tampered/low record can never weaken. Never panics.
        let compiled = Version::parse(env!("CARGO_PKG_VERSION")).unwrap();
        let baseline = installed_baseline();
        assert!(
            baseline >= compiled,
            "the installed baseline {baseline} must be >= the compiled version {compiled}"
        );
    }

    #[test]
    fn evaluate_installed_blocks_a_zero_version_downgrade_and_malformed() {
        // The apply-path entry point: 0.0.1 is older than any real build, so it
        // is always blocked as a downgrade (never silently treated as newer).
        let d = evaluate_installed("0.0.1");
        assert!(
            matches!(d, RollbackDecision::Blocked { .. }),
            "0.0.1 against the live baseline must be a downgrade block, got {d:?}"
        );
        assert!(!d.may_apply());
        // A malformed candidate is fail-closed Malformed, never applied.
        let m = evaluate_installed("not-a-version");
        assert!(matches!(m, RollbackDecision::Malformed { .. }));
        assert!(!m.may_apply());
    }

    #[test]
    fn record_path_is_a_sibling_of_the_exe() {
        // The high-water record lives NEXT TO the exe (same parent dir), under
        // the fixed `.c0pl4nd-installed-version` name.
        let exe = Path::new("/opt/app/c0pl4nd.exe");
        let rec = record_path_for(exe);
        assert_eq!(rec.parent(), exe.parent(), "record is a sibling of the exe");
        assert_eq!(
            rec.file_name().and_then(|n| n.to_str()),
            Some(INSTALLED_VERSION_FILE)
        );
    }

    #[test]
    fn read_record_tolerates_an_empty_or_whitespace_file() {
        // An empty record file (zero bytes) reads as None — the compiled version
        // then governs the floor (never a crash, never a spuriously-low 0.0.0).
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("c0pl4nd");
        std::fs::write(record_path_for(&exe), "").unwrap();
        assert_eq!(read_record(&exe), None);
        // A whitespace-only record is likewise None.
        std::fs::write(record_path_for(&exe), "   \n  ").unwrap();
        assert_eq!(read_record(&exe), None);
    }

    #[test]
    fn read_record_strips_a_leading_v() {
        // The record reader tolerates a leading `v` (mirrors the candidate
        // parser) so a `v0.5.0` line round-trips to the parsed version.
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("c0pl4nd");
        std::fs::write(record_path_for(&exe), "v0.5.0\n").unwrap();
        assert_eq!(read_record(&exe), Some(v("0.5.0")));
    }
}
