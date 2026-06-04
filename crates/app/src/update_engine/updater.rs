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
    /// A verified new binary has been staged; restart to finish.
    ReadyToApply { staged: PathBuf, version: String },
    /// The verified binary was swapped in; restart to run it.
    Applied { version: String },
    /// The last operation failed; `String` is a human-readable reason.
    Failed(String),
}

/// Cross-thread messages from a worker back to the UI thread.
enum UpdateMsg {
    CheckResult(Result<Option<ReleaseInfo>, String>),
    Progress { received: u64, total: u64 },
    Downloaded(Result<(PathBuf, String), String>),
}

/// UI-thread updater model: a polled [`UpdateState`] plus the channel to the
/// current worker.
#[derive(Default)]
pub struct Updater {
    pub state: UpdateState,
    rx: Option<Receiver<UpdateMsg>>,
    /// Why the in-flight check was started (decides auto-download on success).
    launch_kind: LaunchKind,
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
    pub fn start_download(&mut self, ctx: &egui::Context, info: ReleaseInfo) {
        if self.is_busy() {
            return;
        }
        self.state = UpdateState::Downloading {
            received: 0,
            total: 0,
        };
        let (tx, rx) = std::sync::mpsc::channel();
        self.rx = Some(rx);
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let staging = std::env::temp_dir().join("c0pl4nd-update");
            let _ = std::fs::remove_dir_all(&staging);
            let version = info.version.to_string();
            let result = match std::fs::create_dir_all(&staging) {
                Ok(()) => {
                    let ptx = tx.clone();
                    let pctx = ctx.clone();
                    net::download_verify_extract(&info, &staging, move |received, total| {
                        let _ = ptx.send(UpdateMsg::Progress { received, total });
                        pctx.request_repaint();
                    })
                    .map(|path| (path, version))
                }
                Err(e) => Err(format!("cannot create staging dir: {e}")),
            };
            let _ = tx.send(UpdateMsg::Downloaded(result));
            ctx.request_repaint();
        });
    }

    /// Swap the running executable for the staged, verified binary and best-
    /// effort relaunch. On success the window is asked to close.
    ///
    /// Defense-in-depth: before the `self-replace` swap, keep a copy of the
    /// current executable next to it (`<exe>.c0pl4nd-bak`) via
    /// [`apply::install_with_backup`]'s sibling helper. If the swap fails, the
    /// running binary is untouched; if a later relaunch problem is detected the
    /// backup is the recovery surface. The backup is best-effort — a failure to
    /// write it never blocks an otherwise-valid update.
    pub fn apply_and_restart(&mut self, ctx: &egui::Context) {
        let UpdateState::ReadyToApply { staged, version } = &self.state else {
            return;
        };
        let (staged, version) = (staged.clone(), version.clone());

        // Best-effort keep-one-prior backup of the current exe, so a botched
        // install is recoverable via `apply::rollback`.
        let backup = std::env::current_exe()
            .ok()
            .map(|exe| exe.with_extension("c0pl4nd-bak"));
        if let (Ok(exe), Some(bak)) = (std::env::current_exe(), backup.as_ref()) {
            let _ = super::apply::back_up(&exe, bak);
        }

        match super::apply::replace_running_executable(&staged) {
            Ok(()) => {
                if let Ok(exe) = std::env::current_exe() {
                    match std::process::Command::new(&exe).spawn() {
                        Ok(_) => {}
                        Err(e) => {
                            // Relaunch of the swapped binary failed — restore the
                            // prior exe from the backup so the user is not left
                            // with a non-starting install.
                            if let Some(bak) = backup.as_ref() {
                                let _ = super::apply::rollback(bak, &exe);
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
                Ok(UpdateMsg::Downloaded(Ok((staged, version)))) => {
                    self.state = UpdateState::ReadyToApply { staged, version };
                }
                Ok(UpdateMsg::Downloaded(Err(e))) => {
                    self.state = UpdateState::Failed(e);
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    disconnect = true;
                    break;
                }
            }
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
