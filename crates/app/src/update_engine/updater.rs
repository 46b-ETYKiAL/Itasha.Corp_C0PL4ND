//! In-app self-updater UI orchestration (egui-thread state machine).
//!
//! The network discovery, signature/checksum verification, archive extraction,
//! and binary swap all live in [`super::net`] / [`super::verify`] /
//! [`super::apply`] (and are unit-tested there). This module owns only the
//! egui-thread-friendly orchestration: each operation runs on a `std::thread`,
//! communicates back over an `mpsc` channel, and calls `ctx.request_repaint()`
//! so the UI wakes to drain it. The Settings → Updates page renders
//! [`UpdateState`].
//!
//! Privacy: the ONLY network the updater performs is a single HTTPS GET to the
//! public GitHub Releases API (plus the asset/sig/sha downloads when the user
//! chooses to install). No identifiers, no analytics.
//!
//! The [`Updater`] is held across frames by the Settings module via egui
//! `ctx` temp-data (so the settings `show` function stays a free function and
//! needs no host wiring) — see `egui_app::settings`.

use std::path::PathBuf;
use std::sync::mpsc::Receiver;

use super::net::{self, ReleaseInfo};
use super::{UPDATE_OWNER, UPDATE_REPO};

/// Persist a [`tempfile::TempDir`] past its drop-guard, returning its path. The
/// updater owns the dir's lifetime explicitly (it must survive from download
/// until apply) and deletes it itself via `Updater::cleanup_staging_dir`.
fn persist_tempdir(dir: tempfile::TempDir) -> PathBuf {
    dir.keep()
}

/// This build's Rust target triple, used to pick the matching release asset.
///
/// Prefers a build-time-baked `C0PL4ND_TARGET` env (if a build script or the
/// release workflow sets one), and otherwise reconstructs the triple from
/// `cfg!` for the two SHIPPED desktop targets — `x86_64-pc-windows-msvc` and
/// `x86_64-unknown-linux-gnu` (the only two `release.yml` publishes). Any other
/// host compiles to `""`, so no asset matches and the updater reports "no
/// update for this platform" rather than misbehaving. Asset matching is by
/// **substring**, so an exact-but-unbaked triple is unnecessary here.
pub const BUILD_TARGET: &str = match option_env!("C0PL4ND_TARGET") {
    Some(t) => t,
    None => detected_target(),
};

/// Best-effort target-triple reconstruction from `cfg!` for the shipped desktop
/// release targets. Returns `""` for any host we do not publish an asset for.
const fn detected_target() -> &'static str {
    if cfg!(all(target_arch = "x86_64", target_os = "windows")) {
        "x86_64-pc-windows-msvc"
    } else if cfg!(all(target_arch = "x86_64", target_os = "linux")) {
        "x86_64-unknown-linux-gnu"
    } else {
        ""
    }
}

/// The running app version (compile-time, authoritative).
pub fn current_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// Why a check was started — decides what a found update does on completion.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LaunchKind {
    /// User pressed "Check for updates" — show inline state only.
    #[default]
    Manual,
    /// `notify` mode — a found update surfaces a passive notice (the host's
    /// existing launch-toast path owns that; the Updates page just shows state).
    Notify,
    /// `auto` mode — a found update is downloaded + applied automatically.
    Auto,
}

/// What the updater is doing right now. Rendered by the Settings Updates pane.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum UpdateState {
    /// Nothing in flight; no result yet.
    #[default]
    Idle,
    /// A version check is running.
    Checking,
    /// The latest release is the running version (or older).
    UpToDate,
    /// A newer release is available and ready to download.
    Available(ReleaseInfo),
    /// The asset is downloading (`received`/`total` bytes).
    Downloading { received: u64, total: u64 },
    /// A verified new binary has been staged; restart to finish. `release_index`
    /// is the Tier-1 manifest ordinal to persist on a successful apply (`None`
    /// for a legacy, manifest-absent update).
    ReadyToApply {
        staged: PathBuf,
        version: String,
        release_index: Option<u64>,
    },
    /// The verified binary was swapped in; restart to run it.
    Applied { version: String },
    /// The last operation failed; `String` is a human-readable reason.
    Failed(String),
}

/// Cross-thread messages from a worker back to the UI thread.
enum UpdateMsg {
    CheckResult(Result<Option<ReleaseInfo>, String>),
    Progress {
        received: u64,
        total: u64,
    },
    /// `Ok((staged_path, version, release_index))` — `release_index` is the
    /// Tier-1 manifest ordinal to persist on a successful apply (`None` for a
    /// legacy, manifest-absent update path).
    Downloaded(Result<(PathBuf, String, Option<u64>), String>),
}

/// UI-thread updater model: a polled [`UpdateState`] plus the channel to the
/// current worker.
#[derive(Default)]
pub struct Updater {
    pub state: UpdateState,
    rx: Option<Receiver<UpdateMsg>>,
    /// Why the in-flight check was started (decides auto-download on success).
    launch_kind: LaunchKind,
    /// The per-run, freshly-created, user-only staging directory of the most
    /// recent download (audit finding #5, TOCTOU). Tracked so it can be removed
    /// after apply (success or failure) instead of leaking in `temp_dir()`.
    staging_dir: Option<PathBuf>,
}

impl Updater {
    /// True while a network/apply operation is in flight (used to disable the
    /// "Check for updates" button so a second click can't spawn a second job).
    pub fn is_busy(&self) -> bool {
        matches!(
            self.state,
            UpdateState::Checking | UpdateState::Downloading { .. }
        )
    }

    /// Spawn a background version check. `kind` decides what a found update does
    /// on completion: [`LaunchKind::Auto`] auto-downloads; others show inline
    /// state only (the user clicks "Update").
    pub fn start_check(&mut self, ctx: &egui::Context, kind: LaunchKind) {
        if self.is_busy() {
            return;
        }
        self.state = UpdateState::Checking;
        self.launch_kind = kind;
        let (tx, rx) = std::sync::mpsc::channel();
        self.rx = Some(rx);
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let result = match semver::Version::parse(current_version()) {
                Ok(current) => {
                    net::check_for_update(UPDATE_OWNER, UPDATE_REPO, &current, BUILD_TARGET)
                }
                Err(e) => Err(format!("internal: bad current version: {e}")),
            };
            let _ = tx.send(UpdateMsg::CheckResult(result));
            ctx.request_repaint();
        });
    }

    /// Spawn the download + verify + extract worker for a chosen release.
    ///
    /// The staging directory is a **freshly created, uniquely named,
    /// user-only-permission** temp dir (audit finding #5). Each download attempt
    /// gets its own randomized dir via [`tempfile::Builder`] rather than the old
    /// fixed, predictable, world-readable `temp_dir()/c0pl4nd-update` — closing
    /// the local TOCTOU / info-leak surface on a multi-user box. The dir is
    /// removed after apply (success or failure), and any prior staging dir from
    /// an earlier attempt is cleaned up here before a new one is created.
    pub fn start_download(&mut self, ctx: &egui::Context, info: ReleaseInfo) {
        if self.is_busy() {
            return;
        }
        // Clean up any staging dir left over from a prior download attempt.
        self.cleanup_staging_dir();

        // Create the per-run unique staging dir up front. `tempfile` makes it
        // with secure permissions — 0700 (owner-only) on unix via mkdtemp, and
        // a random name under the per-user %TEMP% on Windows — and a random
        // suffix so the path is unpredictable (no pre-create / race / read by
        // another local user).
        let staging = match tempfile::Builder::new().prefix("c0pl4nd-update-").tempdir() {
            Ok(dir) => {
                // Persist the dir past this `TempDir` guard so the verified
                // binary survives until `apply_and_restart`; we delete it
                // ourselves in `cleanup_staging_dir`.
                persist_tempdir(dir)
            }
            Err(e) => {
                self.state = UpdateState::Failed(format!("cannot create staging dir: {e}"));
                return;
            }
        };
        self.staging_dir = Some(staging.clone());

        self.state = UpdateState::Downloading {
            received: 0,
            total: 0,
        };
        let (tx, rx) = std::sync::mpsc::channel();
        self.rx = Some(rx);
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let version = info.version.to_string();
            // Tier-1 manifest ordinal to persist after a successful apply.
            let release_index = info.release_index;
            let ptx = tx.clone();
            let pctx = ctx.clone();
            let result = net::download_verify_extract(&info, &staging, move |received, total| {
                let _ = ptx.send(UpdateMsg::Progress { received, total });
                pctx.request_repaint();
            })
            .map(|path| (path, version, release_index));
            let _ = tx.send(UpdateMsg::Downloaded(result));
            ctx.request_repaint();
        });
    }

    /// Remove the tracked per-run staging directory, if any. Best-effort: a
    /// failure to delete never blocks the updater (the OS reclaims temp dirs).
    fn cleanup_staging_dir(&mut self) {
        if let Some(dir) = self.staging_dir.take() {
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    /// Swap the running executable for the staged, verified binary and best-
    /// effort relaunch. On success the window is asked to close.
    ///
    /// ## Anti-rollback (version-downgrade) gate — fail-closed, BEFORE the swap
    ///
    /// Signature/checksum verification proves the staged binary is a GENUINE
    /// C0PL4ND release, but a validly-signed OLDER release is still genuine: a
    /// MITM'd or replayed Releases listing could hand back a signed-but-
    /// superseded version (a BlackLotus-class downgrade). So before touching the
    /// live executable we re-evaluate the strictly-monotonic freshness rule via
    /// [`super::rollback_guard`] against the highest version this installation
    /// has ever run. A candidate that is older (or an unparseable version) is
    /// REFUSED here, even though it would pass `verify_artifact_bound` — integrity ≠
    /// freshness. An equal version is a no-op (already installed). This is the
    /// security boundary that complements `net::select_update`'s check-time
    /// `latest <= current` rejection, closing the check→apply replay window.
    ///
    /// Defense-in-depth: before the `self-replace` swap, keep a copy of the
    /// current executable next to it (`<exe>.c0pl4nd-bak`) via
    /// [`apply::install_with_backup`]'s sibling helper. If the swap fails, the
    /// running binary is untouched; if a later relaunch problem is detected the
    /// backup is the recovery surface. The backup is best-effort — a failure to
    /// write it never blocks an otherwise-valid update.
    pub fn apply_and_restart(&mut self, ctx: &egui::Context) {
        let UpdateState::ReadyToApply {
            staged,
            version,
            release_index,
        } = &self.state
        else {
            return;
        };
        let (staged, version, release_index) = (staged.clone(), version.clone(), *release_index);

        // Anti-rollback gate (fail-closed): refuse to INSTALL anything that is
        // not strictly newer than the highest version ever installed, even
        // though it passed signature/checksum verification. A downgrade or an
        // unparseable version stops here and never reaches the swap.
        let decision = super::rollback_guard::evaluate_installed(&version);
        if !decision.may_apply() {
            let reason = decision
                .reason()
                .unwrap_or_else(|| "update refused by anti-rollback gate".to_string());
            // The staged artifact is no longer needed — drop the per-run dir.
            self.cleanup_staging_dir();
            self.state = UpdateState::Failed(reason);
            return;
        }

        // Capture the running exe path ONCE, BEFORE the swap. After
        // `replace_running_executable` the OS may report `current_exe()` as a
        // moved/deleted path (Linux "(deleted)", Windows rename-aside), which
        // would make the anti-rollback record write below silently no-op and the
        // relaunch target wrong. The path is stable across an in-place swap, so
        // capture it now and reuse it for backup, the high-water record, and the
        // relaunch.
        let pre_swap_exe = std::env::current_exe().ok();
        // Best-effort keep-one-prior backup of the current exe, so a botched
        // install is recoverable via `apply::rollback`.
        let backup = pre_swap_exe
            .as_ref()
            .map(|exe| exe.with_extension("c0pl4nd-bak"));
        if let (Some(exe), Some(bak)) = (pre_swap_exe.as_ref(), backup.as_ref()) {
            let _ = super::apply::back_up(exe, bak);
        }

        let swap = super::apply::replace_running_executable(&staged);
        // The swap has consumed the staged binary (success) or failed; either
        // way the per-run staging dir is no longer needed — delete it so the
        // verified binary + sidecars do not linger in temp (audit finding #5).
        self.cleanup_staging_dir();

        match swap {
            Ok(()) => {
                // Advance the anti-rollback high-water mark to the just-installed
                // version (monotonic — never lowers it). Best-effort: a failed
                // record write never blocks the applied update, because the
                // freshly-installed binary's own compiled CARGO_PKG_VERSION will
                // govern the floor on the next launch regardless.
                if let (Some(exe), Ok(applied)) =
                    (pre_swap_exe.as_ref(), semver::Version::parse(&version))
                {
                    let _ = super::rollback_guard::record_installed(exe, &applied);
                }
                // Tier-1: advance the persisted `release_index` high-water mark
                // (monotonic, best-effort) so a later replay of an older signed
                // manifest is refused at the next check. A failed write never
                // blocks the applied update (the version floor still governs).
                if let Some(idx) = release_index {
                    super::update_state::record_applied_index_for_current_exe(idx);
                }
                if let Some(exe) = pre_swap_exe.as_ref() {
                    match std::process::Command::new(exe).spawn() {
                        Ok(_) => {}
                        Err(e) => {
                            // Relaunch of the swapped binary failed — restore the
                            // prior exe from the backup so the user is not left
                            // with a non-starting install.
                            if let Some(bak) = backup.as_ref() {
                                let _ = super::apply::rollback(bak, exe);
                            }
                            self.state =
                                UpdateState::Failed(format!("relaunch failed, rolled back: {e}"));
                            return;
                        }
                    }
                }
                self.state = UpdateState::Applied { version };
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            Err(e) => self.state = UpdateState::Failed(format!("install failed: {e}")),
        }
    }

    /// Drain worker messages and advance the state. Call once per frame.
    pub fn poll(&mut self, ctx: &egui::Context) {
        let Some(rx) = &self.rx else {
            return;
        };
        let mut disconnect = false;
        let mut cleanup_staging = false;
        loop {
            match rx.try_recv() {
                Ok(UpdateMsg::CheckResult(Ok(Some(info)))) => {
                    let auto = self.launch_kind == LaunchKind::Auto;
                    self.launch_kind = LaunchKind::Manual;
                    self.state = UpdateState::Available(info.clone());
                    if auto {
                        // `auto` mode downloads + applies without a further click.
                        self.start_download(ctx, info);
                        break; // start_download installs a fresh rx; drain next frame
                    }
                }
                Ok(UpdateMsg::CheckResult(Ok(None))) => {
                    self.launch_kind = LaunchKind::Manual;
                    self.state = UpdateState::UpToDate;
                }
                Ok(UpdateMsg::CheckResult(Err(e))) => {
                    self.launch_kind = LaunchKind::Manual;
                    self.state = UpdateState::Failed(e);
                }
                Ok(UpdateMsg::Progress { received, total }) => {
                    self.state = UpdateState::Downloading { received, total };
                }
                Ok(UpdateMsg::Downloaded(Ok((staged, version, release_index)))) => {
                    self.state = UpdateState::ReadyToApply {
                        staged,
                        version,
                        release_index,
                    };
                }
                Ok(UpdateMsg::Downloaded(Err(e))) => {
                    // Verify/extract failed — `download_verify_extract` already
                    // removed the dir contents; drop our tracker so nothing leaks.
                    // Deferred past the `rx` borrow (cleanup needs `&mut self`).
                    cleanup_staging = true;
                    self.state = UpdateState::Failed(e);
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    disconnect = true;
                    break;
                }
            }
        }
        if cleanup_staging {
            self.cleanup_staging_dir();
        }
        if disconnect {
            self.rx = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_target_is_baked_or_empty() {
        // build.rs bakes C0PL4ND_TARGET; under `cargo test` it is present. Either
        // way the constant resolves (never panics) — that is the contract.
        let _ = BUILD_TARGET;
    }

    #[test]
    fn current_version_parses_as_semver() {
        assert!(semver::Version::parse(current_version()).is_ok());
    }

    #[test]
    fn idle_updater_is_not_busy() {
        let u = Updater::default();
        assert!(!u.is_busy());
        assert!(matches!(u.state, UpdateState::Idle));
    }

    /// Build an `Updater` parked in a given state (private fields default).
    fn updater_in(state: UpdateState) -> Updater {
        Updater {
            state,
            ..Default::default()
        }
    }

    #[test]
    fn busy_states_report_busy() {
        assert!(updater_in(UpdateState::Checking).is_busy());
        assert!(updater_in(UpdateState::Downloading {
            received: 1,
            total: 2,
        })
        .is_busy());
        assert!(!updater_in(UpdateState::UpToDate).is_busy());
        assert!(
            !updater_in(UpdateState::ReadyToApply {
                staged: PathBuf::from("x"),
                version: "1.0.0".into(),
                release_index: None,
            })
            .is_busy(),
            "ready-to-apply is idle (awaits a user click)"
        );
    }

    #[test]
    fn launch_kind_default_is_manual() {
        assert_eq!(LaunchKind::default(), LaunchKind::Manual);
    }

    #[test]
    fn apply_blocks_a_downgrade_staged_version() {
        // A staged version BELOW the running build's own version is an attempted
        // downgrade. Even though it (hypothetically) passed signature/checksum
        // verification to reach ReadyToApply, the anti-rollback gate in
        // `apply_and_restart` must refuse it — moving to Failed("downgrade
        // blocked: …") WITHOUT performing the swap. The baseline here is at
        // least the compiled CARGO_PKG_VERSION, so 0.0.1 is always older.
        let ctx = egui::Context::default();
        let mut u = updater_in(UpdateState::ReadyToApply {
            staged: PathBuf::from("nonexistent-staged-binary"),
            version: "0.0.1".into(),
            release_index: None,
        });
        u.apply_and_restart(&ctx);
        match &u.state {
            UpdateState::Failed(msg) => {
                assert!(
                    msg.contains("downgrade blocked"),
                    "expected a downgrade-blocked failure, got: {msg}"
                );
                assert!(msg.contains("0.0.1"), "reason names the candidate: {msg}");
            }
            other => panic!("expected Failed(downgrade blocked), got {other:?}"),
        }
    }

    #[test]
    fn apply_blocks_a_malformed_staged_version() {
        // An unparseable staged version is refused fail-closed — never treated
        // as "newer" and never swapped in.
        let ctx = egui::Context::default();
        let mut u = updater_in(UpdateState::ReadyToApply {
            staged: PathBuf::from("nonexistent-staged-binary"),
            version: "not-a-version".into(),
            release_index: None,
        });
        u.apply_and_restart(&ctx);
        match &u.state {
            UpdateState::Failed(msg) => {
                assert!(
                    msg.contains("downgrade blocked") && msg.contains("unparseable"),
                    "expected an unparseable-version block, got: {msg}"
                );
            }
            other => panic!("expected Failed(unparseable), got {other:?}"),
        }
    }

    #[test]
    fn apply_is_a_noop_when_not_ready() {
        // The gate only runs from ReadyToApply; any other state leaves apply a
        // no-op (guards the early-return contract the gate piggybacks on).
        let ctx = egui::Context::default();
        let mut u = updater_in(UpdateState::UpToDate);
        u.apply_and_restart(&ctx);
        assert_eq!(u.state, UpdateState::UpToDate);
    }

    /// A release-info fixture for driving the `poll` state machine offline.
    fn fake_release(version: &str) -> ReleaseInfo {
        ReleaseInfo {
            version: semver::Version::parse(version).unwrap(),
            tag: format!("v{version}"),
            asset_url: "https://dl/c0pl4nd.zip".to_string(),
            asset_name: "c0pl4nd.zip".to_string(),
            sig_url: "https://dl/c0pl4nd.zip.minisig".to_string(),
            sha_url: "https://dl/c0pl4nd.zip.sha256".to_string(),
            html_url: "https://github.com/o/r".to_string(),
            pinned_sha256: "deadbeef".to_string(),
            release_index: None,
        }
    }

    /// Build an `Updater` with an injected receiver + launch kind, so `poll` can
    /// be driven with synthetic worker messages (no network, no thread).
    fn updater_with_rx(rx: Receiver<UpdateMsg>, kind: LaunchKind) -> Updater {
        Updater {
            state: UpdateState::Checking,
            rx: Some(rx),
            launch_kind: kind,
            staging_dir: None,
        }
    }

    #[test]
    fn poll_with_no_receiver_is_a_noop() {
        // Without an in-flight worker (no rx), poll leaves the state untouched.
        let ctx = egui::Context::default();
        let mut u = updater_in(UpdateState::UpToDate);
        u.poll(&ctx);
        assert_eq!(u.state, UpdateState::UpToDate);
    }

    #[test]
    fn poll_check_result_none_moves_to_up_to_date() {
        // A "no newer release" check result transitions Checking → UpToDate and
        // resets the launch kind to Manual.
        let ctx = egui::Context::default();
        let (tx, rx) = std::sync::mpsc::channel();
        let mut u = updater_with_rx(rx, LaunchKind::Notify);
        tx.send(UpdateMsg::CheckResult(Ok(None))).unwrap();
        u.poll(&ctx);
        assert_eq!(u.state, UpdateState::UpToDate);
    }

    #[test]
    fn poll_check_result_err_moves_to_failed() {
        let ctx = egui::Context::default();
        let (tx, rx) = std::sync::mpsc::channel();
        let mut u = updater_with_rx(rx, LaunchKind::Manual);
        tx.send(UpdateMsg::CheckResult(Err("offline".to_string())))
            .unwrap();
        u.poll(&ctx);
        assert_eq!(u.state, UpdateState::Failed("offline".to_string()));
    }

    #[test]
    fn poll_check_result_available_in_manual_mode_shows_available_only() {
        // A found update under a NON-auto launch kind parks at Available and
        // does NOT auto-start a download (the user must click Update).
        let ctx = egui::Context::default();
        let (tx, rx) = std::sync::mpsc::channel();
        let info = fake_release("9.9.9");
        let mut u = updater_with_rx(rx, LaunchKind::Manual);
        tx.send(UpdateMsg::CheckResult(Ok(Some(info.clone()))))
            .unwrap();
        u.poll(&ctx);
        assert_eq!(u.state, UpdateState::Available(info));
        assert!(
            !u.is_busy(),
            "manual mode parks at Available without auto-downloading"
        );
    }

    #[test]
    fn poll_progress_updates_downloading_bytes() {
        let ctx = egui::Context::default();
        let (tx, rx) = std::sync::mpsc::channel();
        let mut u = updater_with_rx(rx, LaunchKind::Manual);
        tx.send(UpdateMsg::Progress {
            received: 512,
            total: 2048,
        })
        .unwrap();
        u.poll(&ctx);
        assert_eq!(
            u.state,
            UpdateState::Downloading {
                received: 512,
                total: 2048,
            }
        );
    }

    #[test]
    fn poll_downloaded_ok_moves_to_ready_to_apply() {
        let ctx = egui::Context::default();
        let (tx, rx) = std::sync::mpsc::channel();
        let mut u = updater_with_rx(rx, LaunchKind::Manual);
        tx.send(UpdateMsg::Downloaded(Ok((
            PathBuf::from("/staging/c0pl4nd"),
            "9.9.9".to_string(),
            Some(9_009_009),
        ))))
        .unwrap();
        u.poll(&ctx);
        assert_eq!(
            u.state,
            UpdateState::ReadyToApply {
                staged: PathBuf::from("/staging/c0pl4nd"),
                version: "9.9.9".to_string(),
                release_index: Some(9_009_009),
            }
        );
    }

    #[test]
    fn poll_downloaded_err_moves_to_failed_and_clears_staging() {
        // A verify/extract failure surfaces Failed and triggers staging cleanup
        // (the cleanup is best-effort; with no real dir it is a harmless no-op).
        let ctx = egui::Context::default();
        let (tx, rx) = std::sync::mpsc::channel();
        let mut u = updater_with_rx(rx, LaunchKind::Manual);
        u.staging_dir = Some(PathBuf::from("/nonexistent/staging-dir"));
        tx.send(UpdateMsg::Downloaded(Err("checksum mismatch".to_string())))
            .unwrap();
        u.poll(&ctx);
        assert_eq!(
            u.state,
            UpdateState::Failed("checksum mismatch".to_string())
        );
        assert!(
            u.staging_dir.is_none(),
            "a failed download clears the tracked staging dir"
        );
    }

    #[test]
    fn poll_drops_receiver_on_disconnect() {
        // When the worker's tx is dropped (thread done, channel disconnected),
        // poll clears the receiver so subsequent polls are no-ops.
        let ctx = egui::Context::default();
        let (tx, rx) = std::sync::mpsc::channel::<UpdateMsg>();
        let mut u = updater_with_rx(rx, LaunchKind::Manual);
        drop(tx); // disconnect with no pending messages
        u.poll(&ctx);
        // The state stays Checking (no message arrived), but rx is now cleared:
        // a second poll is a pure no-op (covered by poll_with_no_receiver).
        u.poll(&ctx);
        assert_eq!(u.state, UpdateState::Checking);
    }

    #[test]
    fn state_transitions_are_observable_via_partial_eq() {
        // The state machine's variants compare by value, so the UI can branch on
        // them and tests can assert transitions without driving real I/O.
        assert_eq!(UpdateState::Idle, UpdateState::default());
        assert_ne!(UpdateState::Idle, UpdateState::Checking);
        assert_eq!(
            UpdateState::Downloading {
                received: 5,
                total: 10
            },
            UpdateState::Downloading {
                received: 5,
                total: 10
            }
        );
        assert_ne!(
            UpdateState::Failed("a".into()),
            UpdateState::Failed("b".into())
        );
    }
}
