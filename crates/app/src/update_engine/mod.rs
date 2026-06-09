//! In-app verified self-updater backend (the egui Settings → Updates page).
//!
//! A complete, telemetry-free, **verify-before-swap** updater mirroring the
//! sibling SCR1B3 editor, so the app can download + verify + apply a new
//! release in place — no browser, no installer hand-off:
//!
//! - [`net`] — discover the latest release, download the asset/sig/sha, verify,
//!   then extract (blocking I/O; the pure [`net::select_update`] decision is
//!   unit-tested offline).
//! - [`verify`] — SHA-256 **then** minisign against an EMBEDDED public key;
//!   fails closed (an unverified binary is NEVER returned).
//! - [`apply`] — keep-one-prior backup + `self-replace` atomic swap + rollback.
//! - [`rollback_guard`] — anti-rollback (version-downgrade) protection: a
//!   strictly-monotonic version floor re-checked at APPLY time so a
//!   validly-signed but OLDER release (a replayed/MITM'd listing) cannot
//!   downgrade the install. Integrity (verify) ≠ freshness (this).
//! - [`updater`] — the egui-thread [`updater::Updater`] state machine
//!   (`std::thread` + `mpsc`) the Updates page drives.
//!
//! This module is `#[path]`-included by `egui_app::settings` so it resolves
//! identically in the shipping `c0pl4nd` binary AND in the `egui_kittest`
//! integration test binaries. It is kept separate from the legacy CLI `update`
//! module so the egui-only updater backend is compiled exactly once (by the
//! egui surfaces), never by the legacy winit `c0pl4nd-legacy` binary.

pub mod apply;
pub mod net;
pub mod rollback_guard;
pub mod updater;
pub mod verify;

/// GitHub repo coordinates for the Releases API. Public values, shared by the
/// in-app [`net`] / [`updater`] modules and the "View all releases" link.
pub const UPDATE_OWNER: &str = "46b-ETYKiAL";
pub const UPDATE_REPO: &str = "Itasha.Corp_C0PL4ND";
