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

/// How a downloaded update is applied once verified.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ApplyStrategy {
    /// Writable install dir (portable / per-user) → download the archive and
    /// swap the running binary in place, then relaunch. SILENT: no installer UI,
    /// no elevation, one click — the streamlined path a per-user
    /// (`%LOCALAPPDATA%\Programs\…`) install always takes.
    InPlaceSwap,
    /// Admin-owned dir (e.g. `C:\Program Files`) WITH a shipped self-elevating
    /// installer → download the verified `setup.exe` and run it UNATTENDED
    /// (`--silent --dir`): a single UAC prompt, then a headless in-place install
    /// and auto-relaunch — NO installer window and NO "click Continue". This is
    /// the SCR1B3-parity fallback that fixes updating from Program Files.
    RunInstaller,
    /// Admin-owned dir WITHOUT an installer for this platform → an actionable
    /// failure ("move it, or run once elevated"). The only remaining dead-end.
    NoInstallerAvailable,
}

/// Choose the apply strategy from whether the install dir is writable and
/// whether the release ships a platform installer. The in-place swap is
/// PREFERRED whenever the dir is writable — so a per-user install never routes
/// through the elevated installer (no UAC + no installer click-through). Pure
/// (no filesystem / no `self`) so the routing is unit-testable: a regression
/// that re-routes a writable install through the installer — re-introducing the
/// friction the silent swap exists to avoid — is caught by
/// [`tests::writable_dir_selects_in_place_swap_not_installer`], and a regression
/// that fails-closed on an admin-owned dir that DOES ship an installer — the
/// exact Program-Files bug this change fixes — is caught by
/// [`tests::admin_dir_with_installer_runs_installer`].
pub(crate) fn select_apply_strategy(dir_writable: bool, has_installer: bool) -> ApplyStrategy {
    if dir_writable {
        ApplyStrategy::InPlaceSwap
    } else if has_installer {
        ApplyStrategy::RunInstaller
    } else {
        ApplyStrategy::NoInstallerAvailable
    }
}

/// Build the PowerShell `Start-Process -Verb RunAs` script that runs the
/// FORGE-WIRE `setup.exe` UNATTENDED (no installer window, no "click Continue")
/// and then relaunches C0PL4ND. Pure (so it is unit-testable): every path is
/// single-quoted for PowerShell with any embedded single quote escaped by
/// doubling.
///
/// The installer's `--silent` mode (F0RG3-W1R3 headless install) performs the
/// install with NO UI; `--dir <install_dir>` targets the exact directory
/// C0PL4ND currently runs from so the update lands in place (rather than the
/// installer's own `C:\Program Files\…` default, which could split an install
/// that lives elsewhere). `-Wait` blocks the unelevated helper until the
/// elevated install finishes, and only THEN is the freshly-installed binary
/// relaunched. The relaunch is UNCONDITIONAL (a `try/catch` swallows a UAC
/// cancel or an install error) so the user is never left without C0PL4ND — on
/// failure/decline the prior binary already at `app_exe` simply comes back.
#[cfg(windows)]
fn powershell_runas_script(
    installer: &std::path::Path,
    install_dir: &std::path::Path,
    app_exe: &std::path::Path,
) -> String {
    let q = |p: &std::path::Path| p.to_string_lossy().replace('\'', "''");
    let inst = q(installer);
    let dir = q(install_dir);
    let app = q(app_exe);
    format!(
        "try {{ Start-Process -FilePath '{inst}' \
         -ArgumentList '--silent','--dir','{dir}' -Verb RunAs -Wait }} catch {{ }}; \
         Start-Process -FilePath '{app}'"
    )
}

/// Run the verified self-elevating installer SILENTLY (one UAC prompt, NO
/// installer UI, NO click-through), then relaunch C0PL4ND in place.
///
/// The `setup.exe` carries a `requireAdministrator` manifest. A plain
/// `Command::spawn` (CreateProcess) CANNOT start such a binary — Windows returns
/// ERROR_ELEVATION_REQUIRED (os error 740). Elevation needs ShellExecute
/// semantics, reached here WITHOUT any `unsafe` via PowerShell's `Start-Process
/// -Verb RunAs`, which raises the standard UAC prompt and runs the installer
/// elevated. We pass the installer's `--silent`/`--dir` flags so it installs
/// unattended into the running install's location — the single UAC prompt is
/// unavoidable for a Program-Files write, but there is no interactive installer
/// window and no "Continue" button. The child is routed through the shared
/// no-console-window helper (`CREATE_NO_WINDOW`) so no console flashes.
#[cfg(windows)]
fn launch_installer_elevated(installer: &std::path::Path) -> std::io::Result<()> {
    use c0pl4nd_core::win_process::NoConsoleWindow;
    // Resolve the running install's directory so the silent update overwrites
    // the app exactly where it lives (in place) and so we relaunch THAT binary.
    let app_exe = std::env::current_exe()?;
    let install_dir = app_exe
        .parent()
        .map(std::path::Path::to_path_buf)
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    let script = powershell_runas_script(installer, &install_dir, &app_exe);
    std::process::Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .no_console_window()
        .spawn()
        .map(|_| ())
}

/// Non-Windows fallback: the self-elevating installer is a Windows-only path,
/// but keep the updater buildable everywhere — a plain spawn is correct off
/// Windows (no admin-owned Program-Files apply path exists there).
#[cfg(not(windows))]
fn launch_installer_elevated(installer: &std::path::Path) -> std::io::Result<()> {
    std::process::Command::new(installer).spawn().map(|_| ())
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
    /// The install dir is admin-owned (e.g. Program Files) so an in-place swap
    /// can't write it — a verified self-elevating installer (`setup.exe`) has
    /// been staged instead. Running it SILENTLY (`--silent --dir`) updates in
    /// place (one UAC prompt, no installer UI) and relaunches C0PL4ND. `staged`
    /// is the path to the verified installer.
    ReadyToRunInstaller { installer: PathBuf, version: String },
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
    /// The verified self-elevating `setup.exe` has been staged (the admin-owned
    /// / Program-Files fallback). `Ok((installer_path, version))` — the elevated
    /// install performs its own in-place write, so no `release_index` is carried
    /// (the freshly-installed binary's own version governs the floor).
    InstallerReady(Result<(PathBuf, String), String>),
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
    /// When set, a completed download chains STRAIGHT into apply — no second
    /// click. This is the one-click banner "Update now" path (and `auto` mode),
    /// mirroring SCR1B3's `Downloaded(Ok) -> apply_and_restart` reducer chain so
    /// a single click drives download → verify → silent self-replace → relaunch.
    /// The apply-time anti-rollback + writability gates still run unchanged.
    chain_apply: bool,
    /// The persistent update banner was dismissed by the user for the CURRENT
    /// available release. Reset on every fresh check so a newer release re-shows
    /// the banner; the Settings → Updates page is unaffected by this flag.
    banner_dismissed: bool,
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

    /// True when the persistent notification banner should be shown: a newer
    /// release is available (or an in-flight/finished one-click apply is in
    /// progress) AND the user has not dismissed the banner for this release.
    /// The banner drives the WHOLE flow inline, so it stays visible through
    /// Downloading / ReadyToApply / Applied / Failed once shown.
    pub fn banner_visible(&self) -> bool {
        if self.banner_dismissed {
            return false;
        }
        matches!(
            self.state,
            UpdateState::Available(_)
                | UpdateState::Downloading { .. }
                | UpdateState::ReadyToApply { .. }
                | UpdateState::ReadyToRunInstaller { .. }
                | UpdateState::Applied { .. }
                | UpdateState::Failed(_)
        )
    }

    /// Dismiss the notification banner for the current release (the Settings →
    /// Updates page still works). A later check that finds a NEWER release
    /// re-shows it (the flag is reset in [`Self::start_check`]).
    pub fn dismiss_banner(&mut self) {
        self.banner_dismissed = true;
    }

    /// One-click entry point for the notification banner "Update now" button.
    /// From `Available` it arms the auto-apply chain and starts the download, so
    /// a SINGLE click drives download → verify → silent self-replace → relaunch
    /// (the apply-time anti-rollback + install-dir-writable gates still run).
    /// From `ReadyToApply` (a download already completed) it applies directly.
    /// From `Failed` it re-checks (a retry). Any other state is a no-op.
    pub fn update_now(&mut self, ctx: &egui::Context) {
        match &self.state {
            UpdateState::Available(info) => {
                let info = info.clone();
                self.chain_apply = true;
                self.start_download(ctx, info);
            }
            UpdateState::ReadyToApply { .. } => self.apply_and_restart(ctx),
            UpdateState::ReadyToRunInstaller { .. } => self.run_installer(ctx),
            UpdateState::Failed(_) => self.start_check(ctx, LaunchKind::Notify),
            _ => {}
        }
    }

    /// Spawn a background version check. `kind` decides what a found update does
    /// on completion: [`LaunchKind::Auto`] auto-downloads + applies; others show
    /// inline state only (the user clicks "Update now" / "Download & install").
    pub fn start_check(&mut self, ctx: &egui::Context, kind: LaunchKind) {
        if self.is_busy() {
            return;
        }
        // A fresh check re-arms the banner (a newer release re-shows it) and
        // clears any stale one-click chain from a prior attempt.
        self.banner_dismissed = false;
        self.chain_apply = false;
        tracing::info!(
            target: "c0pl4nd::update",
            event = "update_check_requested",
            launch_kind = ?kind,
            "update check requested",
        );
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
        tracing::info!(
            target: "c0pl4nd::update",
            event = "update_download_started",
            version = %info.version,
            asset = %info.asset_name,
            "downloading + verifying update asset",
        );
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
                tracing::error!(
                    target: "c0pl4nd::update",
                    event = "staging_dir_failed",
                    "failed to create the per-run update staging directory"
                );
                tracing::debug!(
                    target: "c0pl4nd::update",
                    event = "staging_dir_failed_detail",
                    detail = %e,
                    "staging-dir creation error detail"
                );
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

        // Pick the apply strategy (pure, testable) from whether the install dir
        // is writable and whether the release ships a self-elevating installer:
        //  • writable (portable / per-user) → InPlaceSwap: download the archive,
        //    swap the running binary in place + relaunch. SILENT, no elevation.
        //  • admin-owned (Program Files) WITH a setup.exe → RunInstaller:
        //    download the verified installer and run it (`--silent --dir`, UAC).
        //  • admin-owned WITHOUT an installer → an actionable failure.
        // Probed HERE (on the UI thread, before the worker) so the strategy is a
        // plain value the worker branches on with no filesystem coupling.
        let strategy = select_apply_strategy(
            super::apply::install_dir_writable(),
            info.installer.is_some(),
        );
        let installer = info.installer.clone();

        std::thread::spawn(move || {
            let version = info.version.to_string();
            // Tier-1 manifest ordinal to persist after a successful apply.
            let release_index = info.release_index;

            match strategy {
                ApplyStrategy::RunInstaller => {
                    // Only chosen when an installer is present, so the take holds.
                    let inst = installer
                        .expect("RunInstaller is only selected when an installer is present");
                    let ptx = tx.clone();
                    let pctx = ctx.clone();
                    let result =
                        net::download_verify_installer(&inst, &staging, move |received, total| {
                            let _ = ptx.send(UpdateMsg::Progress { received, total });
                            pctx.request_repaint();
                        })
                        .map(|path| (path, version));
                    let _ = tx.send(UpdateMsg::InstallerReady(result));
                    ctx.request_repaint();
                    return;
                }
                ApplyStrategy::NoInstallerAvailable => {
                    // Admin-owned dir, no installer for this platform — the one
                    // remaining actionable dead-end (routed to the relocate copy).
                    let _ = tx.send(UpdateMsg::InstallerReady(Err(
                        "install location not writable: C0PL4ND cannot modify its own \
                         program folder and this release has no installer for your \
                         platform — move it to a folder you own or run once elevated"
                            .to_string(),
                    )));
                    ctx.request_repaint();
                    return;
                }
                ApplyStrategy::InPlaceSwap => {}
            }

            // InPlaceSwap (silent, no elevation, no installer UI): download the
            // verified archive and chain into the in-place swap + relaunch.
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
            // SECURITY: a validly-signed but OLDER (or unparseable) version is a
            // downgrade/replay attempt that passed integrity but fails freshness.
            // Log the security refusal naming the candidate + the gate's reason
            // (both app-controlled strings, no secret/PII) — previously this set
            // `Failed` with no log, leaving a BlackLotus-class downgrade silent.
            tracing::warn!(
                target: "c0pl4nd::update",
                event = "update_refused",
                gate = "anti_rollback",
                candidate_version = %version,
                reason = %reason,
                "refusing update: anti-rollback (downgrade) gate, before any swap"
            );
            // The staged artifact is no longer needed — drop the per-run dir.
            self.cleanup_staging_dir();
            self.state = UpdateState::Failed(reason);
            return;
        }

        // Writability pre-check (fail-fast, BEFORE any rename/copy touches the
        // install dir): the in-place `self-replace` swap must write into the
        // running exe's OWN directory (rename the running binary aside, copy the
        // new one in, rename into place). When C0PL4ND lives in an admin-owned
        // location (`C:\Program Files`, an admin-extracted folder, a read-only
        // mount) that account can't write there, and the swap fails with an
        // access-denied `os error 5` that the old code surfaced as a MISLEADING
        // "free disk space" message — the exact dead-end the user hit. Probe the
        // install dir first and, when it is not writable, stop with a DISTINCT,
        // actionable reason (routed to the relocate/elevate copy in
        // `user_error`) WITHOUT half-renaming the running binary aside.
        if !super::apply::install_dir_writable() {
            tracing::warn!(
                target: "c0pl4nd::update",
                event = "update_refused",
                gate = "install_dir_writable",
                candidate_version = %version,
                "refusing in-place update: install directory is not writable by this account (relocate or run elevated)"
            );
            self.cleanup_staging_dir();
            self.state = UpdateState::Failed(
                "install location not writable: C0PL4ND cannot modify its own \
                 program folder — move it to a folder you own or run once elevated"
                    .to_string(),
            );
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
                    use c0pl4nd_core::win_process::NoConsoleWindow;
                    // Route the relaunch through the shared no-console-window
                    // helper so the swapped-in binary is started without any
                    // transient console (no-op off Windows).
                    match std::process::Command::new(exe).no_console_window().spawn() {
                        Ok(_) => {}
                        Err(e) => {
                            // Relaunch of the swapped binary failed — restore the
                            // prior exe from the backup so the user is not left
                            // with a non-starting install.
                            if let Some(bak) = backup.as_ref() {
                                let _ = super::apply::rollback(bak, exe);
                            }
                            tracing::error!(
                                target: "c0pl4nd::update",
                                event = "relaunch_failed_rolled_back",
                                version = %version,
                                "relaunch of the updated binary failed; rolled back to the prior binary"
                            );
                            tracing::debug!(
                                target: "c0pl4nd::update",
                                event = "relaunch_failed_detail",
                                detail = %e,
                                "relaunch failure detail"
                            );
                            self.state =
                                UpdateState::Failed(format!("relaunch failed, rolled back: {e}"));
                            return;
                        }
                    }
                }
                tracing::info!(
                    target: "c0pl4nd::update",
                    event = "update_applied",
                    version = %version,
                    "update applied: verified binary swapped in; relaunching"
                );
                self.state = UpdateState::Applied { version };
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            Err(e) => {
                tracing::error!(
                    target: "c0pl4nd::update",
                    event = "apply_failed",
                    version = %version,
                    "update install/swap failed; running binary untouched"
                );
                tracing::debug!(
                    target: "c0pl4nd::update",
                    event = "apply_failed_detail",
                    detail = %e,
                    "install/swap failure detail"
                );
                self.state = UpdateState::Failed(format!("install failed: {e}"));
            }
        }
    }

    /// Launch the staged, verified self-elevating installer in SILENT mode and
    /// close the app so the elevated `setup.exe` can replace the files in place.
    /// The helper ([`launch_installer_elevated`]) shows ONE UAC prompt, runs the
    /// installer with no window and no click-through (`--silent --dir`), waits
    /// for it, then relaunches C0PL4ND — so a machine-wide (Program-Files) update
    /// is as seamless as the per-user in-place swap, minus the one unavoidable
    /// elevation prompt. This is the SCR1B3-parity fallback that fixes updating
    /// from an admin-owned folder.
    ///
    /// ## Anti-rollback (version-downgrade) gate — fail-closed, BEFORE launch
    ///
    /// The installer path re-checks the strictly-monotonic freshness rule at
    /// APPLY time — identical to [`Self::apply_and_restart`] — so a replayed
    /// older-but-validly-signed installer can never be run over a newer running
    /// build. Integrity (verify) ≠ freshness (this). A downgrade or unparseable
    /// version is refused here and never launches anything.
    ///
    /// The anti-rollback HIGH-WATER MARK is deliberately NOT advanced here (it is
    /// on the in-place swap). The elevated install is asynchronous and may be
    /// declined at the UAC prompt; advancing the persisted floor before the
    /// install is confirmed would wrongly block a legitimate retry at the next
    /// check. The freshly-installed binary's own compiled `CARGO_PKG_VERSION`
    /// governs the version floor on the next launch, so anti-rollback stays
    /// intact without the premature record write.
    pub fn run_installer(&mut self, ctx: &egui::Context) {
        let UpdateState::ReadyToRunInstaller { installer, version } = &self.state else {
            return;
        };
        let (installer, version) = (installer.clone(), version.clone());

        // Anti-rollback gate (fail-closed), BEFORE any elevation: refuse to run
        // an installer whose version is not strictly newer than the highest
        // version ever installed, even though it passed signature/checksum
        // verification. The in-place-swap path already does this; the installer
        // path MUST too, or the two apply routes defend asymmetrically.
        let decision = super::rollback_guard::evaluate_installed(&version);
        if !decision.may_apply() {
            let reason = decision
                .reason()
                .unwrap_or_else(|| "update refused by anti-rollback gate".to_string());
            tracing::warn!(
                target: "c0pl4nd::update",
                event = "update_refused",
                gate = "anti_rollback",
                candidate_version = %version,
                reason = %reason,
                "refusing installer: anti-rollback (downgrade) gate, before elevation"
            );
            self.cleanup_staging_dir();
            self.state = UpdateState::Failed(reason);
            return;
        }

        // Launch with a UAC elevation prompt — the setup.exe is
        // requireAdministrator, so a plain CreateProcess fails with os error 740.
        // The staging dir is NOT cleaned on success: the installer is running
        // FROM it; the OS reclaims the per-run temp dir after the app closes.
        match launch_installer_elevated(&installer) {
            Ok(()) => {
                tracing::info!(
                    target: "c0pl4nd::update",
                    event = "update_applied",
                    version = %version,
                    "update: elevated silent installer launched; closing to apply in place"
                );
                self.state = UpdateState::Applied { version };
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            Err(e) => {
                // The verified installer could not be launched at all (not a UAC
                // decline — that is swallowed by the script's try/catch and the
                // prior binary relaunches). Surface an actionable failure and
                // clean the staged installer.
                tracing::error!(
                    target: "c0pl4nd::update",
                    event = "installer_launch_failed",
                    version = %version,
                    "failed to launch the staged elevated installer; running binary untouched"
                );
                tracing::debug!(
                    target: "c0pl4nd::update",
                    event = "installer_launch_failed_detail",
                    detail = %e,
                    "installer launch failure detail"
                );
                self.cleanup_staging_dir();
                self.state = UpdateState::Failed(format!(
                    "couldn't launch the installer ({e}). You can run it manually: {}",
                    installer.display()
                ));
            }
        }
    }

    /// Drain worker messages and advance the state. Call once per frame.
    pub fn poll(&mut self, ctx: &egui::Context) {
        let Some(rx) = &self.rx else {
            return;
        };
        let mut disconnect = false;
        let mut cleanup_staging = false;
        let mut chain_apply_now = false;
        let mut run_installer_now = false;
        loop {
            match rx.try_recv() {
                Ok(UpdateMsg::CheckResult(Ok(Some(info)))) => {
                    let auto = self.launch_kind == LaunchKind::Auto;
                    self.launch_kind = LaunchKind::Manual;
                    self.state = UpdateState::Available(info.clone());
                    if auto {
                        // `auto` mode downloads AND applies without a further
                        // click — arm the one-click chain so the completed
                        // download flows straight into apply + relaunch.
                        self.chain_apply = true;
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
                    if self.chain_apply {
                        // One-click / auto path: chain straight into apply (the
                        // apply-time anti-rollback + writability gates still run).
                        // Deferred past the `rx` borrow — apply needs `&mut self`.
                        chain_apply_now = true;
                        break;
                    }
                }
                Ok(UpdateMsg::Downloaded(Err(e))) => {
                    // Verify/extract failed — `download_verify_extract` already
                    // removed the dir contents; drop our tracker so nothing leaks.
                    // Deferred past the `rx` borrow (cleanup needs `&mut self`).
                    cleanup_staging = true;
                    self.state = UpdateState::Failed(e);
                }
                Ok(UpdateMsg::InstallerReady(Ok((installer, version)))) => {
                    // The verified self-elevating installer is staged (admin-owned
                    // / Program-Files fallback). Park at ReadyToRunInstaller; the
                    // one-click / auto chain then launches it (deferred past the
                    // `rx` borrow — run_installer needs `&mut self`).
                    self.state = UpdateState::ReadyToRunInstaller { installer, version };
                    if self.chain_apply {
                        run_installer_now = true;
                        break;
                    }
                }
                Ok(UpdateMsg::InstallerReady(Err(e))) => {
                    // Installer download/verify failed (fail-closed) OR the
                    // admin-owned dir has no installer for this platform. The
                    // staging dir was already wiped by download_verify_installer
                    // on a verify failure; drop our tracker so nothing leaks.
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
        if chain_apply_now {
            // Consume the one-shot chain flag and run the apply now that the
            // `rx` borrow is released (apply needs `&mut self`). This is the
            // no-second-click hand-off from a completed download to the swap.
            self.chain_apply = false;
            self.apply_and_restart(ctx);
        }
        if run_installer_now {
            // Same one-shot hand-off for the elevated-installer (Program-Files)
            // path: a completed + verified installer download flows straight into
            // the silent elevated launch (no second click), the `rx` borrow now
            // released. The apply-time anti-rollback gate in `run_installer`
            // still runs.
            self.chain_apply = false;
            self.run_installer(ctx);
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
            installer: None,
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
            chain_apply: false,
            banner_dismissed: false,
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

    #[test]
    fn one_click_chain_downloaded_ok_flows_straight_into_apply() {
        // The load-bearing one-click property: with the chain armed, a completed
        // download must NOT park at ReadyToApply — it chains straight into apply
        // within the SAME poll (no second click). We use a DOWNGRADE version so
        // the apply-time anti-rollback gate stops it BEFORE the real self-replace
        // swap (so the running test binary is never touched), which still proves
        // the chain fired: the state ends at Failed(downgrade), not ReadyToApply.
        let ctx = egui::Context::default();
        let (tx, rx) = std::sync::mpsc::channel();
        let mut u = updater_with_rx(rx, LaunchKind::Manual);
        u.chain_apply = true;
        tx.send(UpdateMsg::Downloaded(Ok((
            PathBuf::from("nonexistent-staged-binary"),
            "0.0.1".to_string(),
            None,
        ))))
        .unwrap();
        u.poll(&ctx);
        match &u.state {
            UpdateState::Failed(msg) => assert!(
                msg.contains("downgrade blocked"),
                "one-click chain must reach the apply-time gate: {msg}"
            ),
            other => panic!("chain must flow into apply, not park; got {other:?}"),
        }
        assert!(
            !u.chain_apply,
            "the chain flag is one-shot (consumed after apply)"
        );
    }

    #[test]
    fn without_chain_downloaded_ok_parks_for_an_explicit_click() {
        // The non-one-click Settings path is preserved: a completed download with
        // the chain NOT armed parks at ReadyToApply awaiting "Restart to finish".
        let ctx = egui::Context::default();
        let (tx, rx) = std::sync::mpsc::channel();
        let mut u = updater_with_rx(rx, LaunchKind::Manual);
        assert!(!u.chain_apply);
        tx.send(UpdateMsg::Downloaded(Ok((
            PathBuf::from("/staging/c0pl4nd"),
            "9.9.9".to_string(),
            None,
        ))))
        .unwrap();
        u.poll(&ctx);
        assert!(
            matches!(u.state, UpdateState::ReadyToApply { .. }),
            "no chain -> park at ReadyToApply, got {:?}",
            u.state
        );
    }

    #[test]
    fn update_now_from_ready_to_apply_downgrade_is_refused_without_swap() {
        // update_now on a ReadyToApply(downgrade) applies directly and the
        // anti-rollback gate refuses it — no swap of the running binary.
        let ctx = egui::Context::default();
        let mut u = updater_in(UpdateState::ReadyToApply {
            staged: PathBuf::from("nonexistent"),
            version: "0.0.1".into(),
            release_index: None,
        });
        u.update_now(&ctx);
        assert!(
            matches!(&u.state, UpdateState::Failed(m) if m.contains("downgrade blocked")),
            "got {:?}",
            u.state
        );
    }

    #[test]
    fn update_now_is_a_noop_off_actionable_states() {
        let ctx = egui::Context::default();
        for state in [
            UpdateState::Idle,
            UpdateState::Checking,
            UpdateState::UpToDate,
        ] {
            let mut u = updater_in(state.clone());
            u.update_now(&ctx);
            assert_eq!(u.state, state, "update_now must be a no-op from {state:?}");
        }
    }

    #[test]
    fn banner_visible_reflects_state_and_dismissal() {
        // The banner shows for every actionable in-flight/finished state and is
        // hidden for the idle/quiet ones.
        assert!(!updater_in(UpdateState::Idle).banner_visible());
        assert!(!updater_in(UpdateState::Checking).banner_visible());
        assert!(!updater_in(UpdateState::UpToDate).banner_visible());
        assert!(updater_in(UpdateState::Available(fake_release("9.9.9"))).banner_visible());
        assert!(updater_in(UpdateState::Downloading {
            received: 1,
            total: 2
        })
        .banner_visible());
        assert!(updater_in(UpdateState::ReadyToApply {
            staged: PathBuf::from("x"),
            version: "9.9.9".into(),
            release_index: None,
        })
        .banner_visible());
        assert!(updater_in(UpdateState::Applied {
            version: "9.9.9".into()
        })
        .banner_visible());
        assert!(updater_in(UpdateState::Failed("x".into())).banner_visible());
    }

    #[test]
    fn dismiss_banner_hides_it_until_a_fresh_check() {
        // Dismissing hides the banner for the CURRENT release; a fresh check
        // clears the dismissal so a newer release re-shows it.
        let mut u = updater_in(UpdateState::Available(fake_release("9.9.9")));
        assert!(u.banner_visible());
        u.dismiss_banner();
        assert!(!u.banner_visible(), "dismissed banner is hidden");
        assert!(u.banner_dismissed);
        // The reset happens in start_check; assert the field-level contract the
        // start_check reset relies on (start_check itself spawns a network thread,
        // so the reset line is exercised there, not in this offline unit test).
        u.banner_dismissed = false;
        assert!(u.banner_visible(), "re-armed banner shows again");
    }

    // --- apply-strategy routing (the Program-Files fallback decision) ----------

    #[test]
    fn writable_dir_selects_in_place_swap_not_installer() {
        // The streamlined-update contract: a writable (portable / per-user)
        // install ALWAYS takes the silent in-place swap — never the elevated
        // installer — regardless of whether a setup.exe was published. A
        // regression here is exactly the UAC + installer-click-through friction
        // the silent swap exists to avoid.
        assert_eq!(
            select_apply_strategy(true, true),
            ApplyStrategy::InPlaceSwap,
            "writable dir + installer present must STILL swap in place (silent)"
        );
        assert_eq!(
            select_apply_strategy(true, false),
            ApplyStrategy::InPlaceSwap,
            "writable dir + no installer must swap in place"
        );
    }

    #[test]
    fn admin_dir_with_installer_runs_installer() {
        // The load-bearing FIX: an admin-owned (Program Files) install that DOES
        // ship a self-elevating setup.exe now routes to the silent elevated
        // installer instead of failing closed with "move it / run as admin".
        assert_eq!(
            select_apply_strategy(false, true),
            ApplyStrategy::RunInstaller,
            "admin-owned dir WITH an installer runs it silently+elevated (the fix)"
        );
    }

    #[test]
    fn admin_dir_without_installer_is_actionable_failure() {
        // The only remaining dead-end: an admin-owned dir on a platform with no
        // setup.exe (e.g. an admin-extracted Linux install) stays an actionable
        // relocate/elevate failure, never a fake success.
        assert_eq!(
            select_apply_strategy(false, false),
            ApplyStrategy::NoInstallerAvailable,
            "admin-owned dir with NO installer is an actionable failure, not a swap"
        );
    }

    #[cfg(windows)]
    #[test]
    fn powershell_runas_script_is_silent_and_targets_running_install() {
        use std::path::Path;
        // The seamless-update contract for the Program-Files fallback: the
        // installer is invoked UNATTENDED (`--silent`), targets the exact
        // directory C0PL4ND runs from (`--dir`, so the update lands in place, not
        // the installer's own default), WAITS for the elevated install, and then
        // relaunches the app. A regression that drops any of these re-introduces
        // the interactive "click Continue" window the silent invocation removes.
        let script = powershell_runas_script(
            Path::new(r"C:\pf\c0pl4nd-setup.exe"),
            Path::new(r"C:\pf\C0PL4ND"),
            Path::new(r"C:\pf\C0PL4ND\c0pl4nd.exe"),
        );
        assert!(
            script.contains("'--silent'"),
            "must run unattended: {script}"
        );
        assert!(
            script.contains(r"'--dir','C:\pf\C0PL4ND'"),
            "must target the running install dir in place: {script}"
        );
        assert!(
            script.contains("-Verb RunAs"),
            "must still elevate: {script}"
        );
        assert!(
            script.contains("-Wait"),
            "must wait for the elevated install before relaunch: {script}"
        );
        assert!(
            script.contains(r"Start-Process -FilePath 'C:\pf\C0PL4ND\c0pl4nd.exe'"),
            "must relaunch the app after the install: {script}"
        );
    }

    #[cfg(windows)]
    #[test]
    fn powershell_runas_script_escapes_embedded_single_quote() {
        use std::path::Path;
        // Every path is single-quoted; an embedded single quote in ANY path is
        // escaped by doubling (the PowerShell rule), so a crafted path can never
        // break out of the quoted string (command-injection defense).
        let script = powershell_runas_script(
            Path::new(r"C:\o'brien\c0pl4nd-setup.exe"),
            Path::new(r"C:\o'brien\app"),
            Path::new(r"C:\o'brien\app\c0pl4nd.exe"),
        );
        assert_eq!(
            script,
            "try { Start-Process -FilePath 'C:\\o''brien\\c0pl4nd-setup.exe' \
             -ArgumentList '--silent','--dir','C:\\o''brien\\app' \
             -Verb RunAs -Wait } catch { }; \
             Start-Process -FilePath 'C:\\o''brien\\app\\c0pl4nd.exe'"
        );
    }

    #[test]
    fn run_installer_is_a_noop_when_not_ready() {
        // The gate only runs from ReadyToRunInstaller; any other state leaves
        // run_installer a no-op (guards the early-return contract).
        let ctx = egui::Context::default();
        let mut u = updater_in(UpdateState::UpToDate);
        u.run_installer(&ctx);
        assert_eq!(u.state, UpdateState::UpToDate);
    }

    #[test]
    fn run_installer_refuses_a_downgrade_before_elevation() {
        // Anti-rollback parity with the in-place path: a staged installer whose
        // version is NOT strictly newer than the running build is refused at
        // apply time (before any elevation) and lands in Failed — never
        // launching PowerShell. 0.0.1 is always older than the compiled version.
        let ctx = egui::Context::default();
        let mut u = updater_in(UpdateState::ReadyToRunInstaller {
            installer: PathBuf::from("nonexistent-c0pl4nd-setup.exe"),
            version: "0.0.1".into(),
        });
        u.run_installer(&ctx);
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
    fn banner_visible_includes_ready_to_run_installer() {
        // The Program-Files fallback state is an actionable in-flight state, so
        // the banner must stay visible through it (never silently disappear while
        // an elevated install is pending).
        assert!(updater_in(UpdateState::ReadyToRunInstaller {
            installer: PathBuf::from("x"),
            version: "9.9.9".into(),
        })
        .banner_visible());
    }

    #[test]
    fn poll_installer_ready_ok_parks_without_chain() {
        // A completed installer download with the chain NOT armed parks at
        // ReadyToRunInstaller awaiting an explicit "Restart & install" click.
        let ctx = egui::Context::default();
        let (tx, rx) = std::sync::mpsc::channel();
        let mut u = updater_with_rx(rx, LaunchKind::Manual);
        tx.send(UpdateMsg::InstallerReady(Ok((
            PathBuf::from("/staging/c0pl4nd-setup.exe"),
            "9.9.9".to_string(),
        ))))
        .unwrap();
        u.poll(&ctx);
        assert_eq!(
            u.state,
            UpdateState::ReadyToRunInstaller {
                installer: PathBuf::from("/staging/c0pl4nd-setup.exe"),
                version: "9.9.9".to_string(),
            }
        );
    }

    #[test]
    fn poll_installer_ready_err_moves_to_failed_and_clears_staging() {
        // A verify failure / no-installer error surfaces Failed and clears the
        // tracked staging dir (best-effort; a nonexistent dir is a no-op).
        let ctx = egui::Context::default();
        let (tx, rx) = std::sync::mpsc::channel();
        let mut u = updater_with_rx(rx, LaunchKind::Manual);
        u.staging_dir = Some(PathBuf::from("/nonexistent/staging-dir"));
        tx.send(UpdateMsg::InstallerReady(Err(
            "install location not writable: …".to_string(),
        )))
        .unwrap();
        u.poll(&ctx);
        assert!(matches!(u.state, UpdateState::Failed(_)));
        assert!(
            u.staging_dir.is_none(),
            "a failed installer path clears the tracked staging dir"
        );
    }

    #[test]
    fn one_click_chain_installer_ready_flows_straight_into_run_installer() {
        // The one-click property for the Program-Files path: with the chain
        // armed, a completed installer download must NOT park — it chains into
        // run_installer within the SAME poll. A DOWNGRADE version makes the
        // apply-time anti-rollback gate stop it BEFORE any elevation (so the test
        // never spawns PowerShell), proving the chain fired: the state ends at
        // Failed(downgrade), not ReadyToRunInstaller.
        let ctx = egui::Context::default();
        let (tx, rx) = std::sync::mpsc::channel();
        let mut u = updater_with_rx(rx, LaunchKind::Manual);
        u.chain_apply = true;
        tx.send(UpdateMsg::InstallerReady(Ok((
            PathBuf::from("nonexistent-c0pl4nd-setup.exe"),
            "0.0.1".to_string(),
        ))))
        .unwrap();
        u.poll(&ctx);
        match &u.state {
            UpdateState::Failed(msg) => assert!(
                msg.contains("downgrade blocked"),
                "one-click installer chain must reach the apply-time gate: {msg}"
            ),
            other => panic!("chain must flow into run_installer, not park; got {other:?}"),
        }
        assert!(!u.chain_apply, "the chain flag is one-shot (consumed)");
    }
}
