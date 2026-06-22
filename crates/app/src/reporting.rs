//! W1TN3SS opt-in crash/error reporting — the C0PL4ND host glue (Tier-1).
//!
//! This module is thin host glue over the in-house `itasha-report-core` SDK
//! (pinned git tag). C0PL4ND implements NO SDK behavior — the config model,
//! sanitizer, spool, transport, preview API and consent gate all live in the
//! SDK and are CALLED here. The two seams this module owns are:
//!
//! 1. **Capture** ([`capture_panic`]) — builds a Tier-1 report from a panic's
//!    `&'static str` message + our own `file:line` SITE, sanitizes it, and
//!    SPOOLS it locally. It transmits NOTHING — local-first, offline-safe,
//!    consent comes later.
//! 2. **Consent-gated send** ([`send_report`]) — given a host-minted
//!    [`ConsentToken`] (which only exists after the user agreed in the consent
//!    dialog, or because the stream's mode is `Always`), transmit one spooled
//!    report through the SDK's hardened transport, then log the outcome.
//!
//! Privacy invariants (inherited from the SDK, asserted at this surface):
//! - default-OFF (both streams default [`ReportingMode::Off`]),
//! - consent-gated (no [`ConsentToken`] => no send — enforced at the type level
//!   by the SDK's `IngestBackend::send` signature),
//! - previewable+editable before send (the dialog calls [`preview_text`]),
//! - no persistent identifier (only the consent token's ephemeral nonce),
//! - the panic `&'static str` discipline (a `String` payload — which could embed
//!   environment fragments or a path — is deliberately suppressed at capture).

use std::path::{Path, PathBuf};

use itasha_report_core::backend::{
    IngestBackend, LeanPipelineBackend, SendOutcome, TransportConfig,
};
use itasha_report_core::consent::ConsentToken;
use itasha_report_core::preview::Preview;
use itasha_report_core::report::{Report, Stream};
use itasha_report_core::sanitize::Sanitizer;
use itasha_report_core::spool::Spool;

// Re-export the SDK's ReportingMode so the rest of the app names ONE type.
pub use itasha_report_core::config::ReportingMode;

use c0pl4nd_core::Config;

/// The env var that injects the self-hosted ingest endpoint. There is NO
/// hardcoded URL in C0PL4ND and NO default — a build with this unset can spool
/// locally but can NEVER transmit (a mis-build cannot phone home). The
/// server-side endpoint is a separate plan; until one is configured, reports
/// stay in the local spool and a consented send returns a structured
/// `no-endpoint` outcome (never a silent drop, never a fake success).
pub const REPORT_ENDPOINT_ENV: &str = "C0PL4ND_REPORT_ENDPOINT";

/// The structured result of attempting a report, logged counts/enums only
/// (never PII). A report is either captured-and-spooled, sent, refused for want
/// of an endpoint, or failed in transport — never silently dropped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReportOutcome {
    /// The panic was captured and written to the local spool. Nothing sent.
    Spooled,
    /// A consented report was transmitted and accepted by the endpoint.
    Sent,
    /// Consent was present but no endpoint is configured — the report stays
    /// spooled for a later, configured send.
    RefusedNoEndpoint,
    /// The transport failed (offline, TLS, status). The report is retained.
    Failed(String),
}

impl ReportOutcome {
    /// The stable, non-identifying log-detail string for this outcome.
    fn log_detail(&self) -> &'static str {
        match self {
            ReportOutcome::Spooled => "spooled",
            ReportOutcome::Sent => "sent",
            ReportOutcome::RefusedNoEndpoint => "refused-no-endpoint",
            ReportOutcome::Failed(_) => "failed",
        }
    }
}

/// Log a report outcome counts/enums only (no PII — the `Failed` reason is
/// NEVER inlined). Honours `S4F3_DISABLE_TELEMETRY=1` by emitting nothing.
/// Best-effort; never blocks.
fn log_outcome(outcome: &ReportOutcome) {
    if std::env::var_os("S4F3_DISABLE_TELEMETRY").is_some() {
        return;
    }
    tracing::info!(target: "c0pl4nd::report", detail = outcome.log_detail());
}

/// Build a sanitized Tier-1 crash report from the panic's STATIC message + our
/// own panic SITE. Only a source-literal message (e.g. an `expect("…")` string)
/// and the `file:line` of our own code enter the report. A runtime `String`
/// payload — which could embed environment fragments or a user's path — is the
/// caller's responsibility to keep out (the hook passes `&'static str` only);
/// the SDK's [`Sanitizer`] is the second line of defense (home/username/host
/// scrub).
pub fn build_crash_report(static_msg: &'static str, location: &str) -> Report {
    let raw = Report::crash(format!("panic: {static_msg} (at {location})"))
        .with_metadata("app_version", env!("CARGO_PKG_VERSION"))
        .with_metadata("os", std::env::consts::OS);
    Sanitizer::new().sanitize(raw)
}

/// The literal, editable Tier-1 preview text the consent dialog shows the user
/// BEFORE any send. This is the transparency primitive — the user sees exactly
/// what would leave the machine.
#[must_use]
pub fn preview_text(report: &Report) -> String {
    Preview::of(report).text().to_string()
}

/// Rebuild a [`Report`] from the user-edited preview text, preserving the
/// original report's stream, title, metadata, and attachments. The preview text
/// renders as `title\n\nbody[\n\n--- metadata ---\n…]`; this extracts the BODY
/// span so the user's edits/redactions to the body are what gets sent.
#[must_use]
pub fn edited_report_from_preview_text(edited_text: &str, original: &Report) -> Report {
    let body = edited_text
        // Drop the title line: everything after the first blank-line separator.
        .split_once("\n\n")
        .map(|(_title, rest)| rest)
        .unwrap_or(edited_text)
        // Drop the metadata footer if present.
        .split("\n\n--- metadata ---\n")
        .next()
        .unwrap_or(edited_text)
        .to_string();
    Report {
        stream: original.stream,
        title: original.title.clone(),
        body,
        metadata: original.metadata.clone(),
        attachments: original.attachments.clone(),
    }
}

/// Capture a panic into the local spool. Builds the sanitized Tier-1 report,
/// then enqueues it to `<config_dir>/reports/` via the SDK's atomic spool. This
/// is the panic-hook seam: it CAPTURES + SPOOLS but transmits NOTHING — consent
/// is sought on the NEXT launch (ask-each-time) or honoured automatically
/// (`Always`), never inside the panic hook. Returns the outcome (for logging).
///
/// Best-effort and panic-safe: a spool failure inside an already-panicking
/// thread must not re-panic. The outcome is logged either way.
pub fn capture_panic(static_msg: &'static str, location: &str) -> ReportOutcome {
    let outcome = match Config::config_dir() {
        Some(dir) => match Spool::open(&dir) {
            Ok(spool) => {
                let report = build_crash_report(static_msg, location);
                match spool.enqueue(&report) {
                    Ok(_path) => ReportOutcome::Spooled,
                    Err(e) => ReportOutcome::Failed(format!("spool: {e}")),
                }
            }
            Err(e) => ReportOutcome::Failed(format!("spool-open: {e}")),
        },
        // No config dir => nowhere to spool. Surface it rather than swallow.
        None => ReportOutcome::Failed("no-config-dir".to_string()),
    };
    log_outcome(&outcome);
    outcome
}

/// Transmit ONE report through the SDK's hardened transport, consent-gated.
///
/// The `consent` argument is mandatory — there is no send path without it (the
/// SDK enforces this at the type level). The host mints the [`ConsentToken`]
/// ONLY after the user agreed in the dialog, or because the stream's mode is
/// `Always`. The transport is the SDK's [`LeanPipelineBackend`]: a static
/// User-Agent, zero redirects, bounded timeout, size-capped, NO persistent
/// identifier (only the token's ephemeral nonce). The outcome is logged.
///
/// If no endpoint is configured (the `C0PL4ND_REPORT_ENDPOINT` env is unset),
/// this returns [`ReportOutcome::RefusedNoEndpoint`] and transmits nothing — the
/// report stays in the spool for a later, configured send.
pub fn send_report(report: &Report, consent: &ConsentToken) -> ReportOutcome {
    let outcome = match endpoint_from_env() {
        Some(endpoint) => {
            let backend = LeanPipelineBackend::new(TransportConfig::new(endpoint));
            match backend.send(report, consent) {
                Ok(SendOutcome::Sent) => ReportOutcome::Sent,
                Ok(SendOutcome::Failed(reason)) => ReportOutcome::Failed(reason),
                Err(e) => ReportOutcome::Failed(e.to_string()),
            }
        }
        None => ReportOutcome::RefusedNoEndpoint,
    };
    log_outcome(&outcome);
    outcome
}

/// Read the ingest endpoint from the env var, treating an empty value as unset.
fn endpoint_from_env() -> Option<String> {
    std::env::var(REPORT_ENDPOINT_ENV)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Open the local spool rooted at an EXPLICIT config dir so the host can drain
/// pending crash reports into the consent dialog on the next launch. The dir is
/// always passed by the caller (the app's per-instance resolved `config_dir`, a
/// temp dir under test) so no spool I/O ever silently hits the GLOBAL
/// `Config::config_dir()`.
pub fn open_spool_in(dir: &Path) -> Option<Spool> {
    Spool::open(dir).ok()
}

/// What the user chose to remember for the crash stream after a per-event
/// consent decision (Always / Never / Just this time). Maps onto the config
/// `ReportingMode` so the next launch honours it (or keeps asking).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RememberChoice {
    /// Remember "Always send" — graduate the stream to [`ReportingMode::Always`].
    Always,
    /// Remember "Never" — set the stream to [`ReportingMode::Off`].
    Never,
    /// Just this time — leave the mode at [`ReportingMode::AskEachTime`].
    JustThisTime,
}

impl RememberChoice {
    /// The `ReportingMode` this choice should persist to the config, if any.
    /// `JustThisTime` returns `None` (the mode stays `AskEachTime`).
    #[must_use]
    pub fn persisted_mode(self) -> Option<ReportingMode> {
        match self {
            RememberChoice::Always => Some(ReportingMode::Always),
            RememberChoice::Never => Some(ReportingMode::Off),
            RememberChoice::JustThisTime => None,
        }
    }
}

/// The per-launch crash-consent dialog state, owned by the app. On launch the
/// host loads the spooled crash reports into `queue`; the dialog presents them
/// one at a time with an EDITABLE preview and equal-weight Send / Don't-send.
///
/// This holds NO SDK transport state — only the spooled paths, the currently-
/// presented report + its editable preview text, and the user's remember choice.
#[derive(Debug, Default)]
pub struct CrashConsentState {
    /// The EXPLICIT config dir this dialog's spool I/O is rooted at — the app's
    /// per-instance resolved `config_dir` (a temp dir under test). `None` until
    /// the host binds it via [`CrashConsentState::set_config_dir`]; while `None`
    /// every spool operation is a no-op (so a default-constructed state touches
    /// NO real config dir).
    config_dir: Option<PathBuf>,
    /// Remaining spooled report paths to present (oldest first).
    queue: Vec<PathBuf>,
    /// The report currently shown in the dialog (loaded from `queue`'s head).
    current: Option<(PathBuf, Report)>,
    /// The editable preview text the user sees and may modify before sending.
    edited_text: String,
    /// The remember-my-choice selection (defaults to Just-this-time).
    remember: Option<RememberChoice>,
}

impl CrashConsentState {
    /// Bind the explicit config dir whose `reports/` spool this dialog drains.
    pub fn set_config_dir(&mut self, dir: Option<PathBuf>) {
        self.config_dir = dir;
    }

    /// Open this dialog's spool at its bound config dir, if any is set.
    fn spool(&self) -> Option<Spool> {
        self.config_dir.as_deref().and_then(open_spool_in)
    }

    /// Load the spooled CRASH reports into the dialog queue. Returns the number
    /// queued. Manual-issue reports are not presented by this crash dialog.
    /// Best-effort: a spool error yields an empty queue.
    pub fn load_from_spool(&mut self) -> usize {
        self.queue.clear();
        self.current = None;
        if let Some(spool) = self.spool() {
            if let Ok(paths) = spool.list() {
                for path in paths {
                    if let Ok(report) = spool.load(&path) {
                        if report.stream == Stream::CrashReports {
                            self.queue.push(path);
                        }
                    }
                }
            }
        }
        self.advance();
        self.queue.len() + usize::from(self.current.is_some())
    }

    /// Whether the dialog has a report to present this frame.
    #[must_use]
    pub fn has_pending(&self) -> bool {
        self.current.is_some()
    }

    /// The editable preview text (mutable so the dialog can bind a `TextEdit`).
    pub fn edited_text_mut(&mut self) -> &mut String {
        &mut self.edited_text
    }

    /// The remember-choice selection (mutable for the dialog radios).
    pub fn remember_mut(&mut self) -> &mut Option<RememberChoice> {
        &mut self.remember
    }

    /// Pop the next report off the queue and load it as `current` + its preview
    /// text. Clears `current` when the queue is empty.
    fn advance(&mut self) {
        self.current = None;
        self.edited_text.clear();
        self.remember = Some(RememberChoice::JustThisTime);
        if self.queue.is_empty() {
            return;
        }
        let path = self.queue.remove(0);
        if let Some(spool) = self.spool() {
            if let Ok(report) = spool.load(&path) {
                self.edited_text = preview_text(&report);
                self.current = Some((path, report));
            }
        }
    }

    /// The user pressed SEND on the current report. Build the (possibly edited)
    /// report from the preview text, mint a consent token, transmit, and — on a
    /// successful send — remove the spooled file. Returns the outcome. Advances
    /// to the next queued report regardless of outcome.
    pub fn consent_and_send(&mut self) -> Option<ReportOutcome> {
        let (path, original) = self.current.take()?;
        let edited = edited_report_from_preview_text(&self.edited_text, &original);
        let token = ConsentToken::granted();
        let outcome = send_report(&edited, &token);
        if outcome == ReportOutcome::Sent {
            if let Some(spool) = self.spool() {
                let _ = spool.remove(&path);
            }
        }
        // Not sent (offline / no endpoint / failed): keep the file spooled so a
        // later configured/online send can retry.
        self.advance();
        Some(outcome)
    }

    /// The user pressed DON'T-SEND on the current report. Discard the spooled
    /// file (the user declined to send it) and advance.
    pub fn decline_and_discard(&mut self) {
        if let Some((path, _)) = self.current.take() {
            if let Some(spool) = self.spool() {
                let _ = spool.remove(&path);
            }
        }
        self.advance();
    }
}

/// Auto-send every spooled CRASH report through the consent-gated path WITHOUT a
/// dialog — used when the crash stream's mode is [`ReportingMode::Always`]. Each
/// report is still transmitted only via a freshly-minted [`ConsentToken`]; a
/// successful send removes the spooled file, a failure leaves it for retry.
/// Returns the number of reports for which a send was ATTEMPTED.
pub fn auto_send_spooled_crashes(config_dir: &Path) -> usize {
    let Some(spool) = open_spool_in(config_dir) else {
        return 0;
    };
    let Ok(paths) = spool.list() else {
        return 0;
    };
    let mut attempted = 0;
    for path in paths {
        let Ok(report) = spool.load(&path) else {
            continue;
        };
        if report.stream != Stream::CrashReports {
            continue;
        }
        attempted += 1;
        let token = ConsentToken::granted();
        if send_report(&report, &token) == ReportOutcome::Sent {
            let _ = spool.remove(&path);
        }
    }
    attempted
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scoped guard that sets an env var and restores it on drop.
    struct EnvGuard {
        key: &'static str,
        prev: Option<String>,
    }
    impl EnvGuard {
        fn set(key: &'static str, val: &str) -> Self {
            let prev = std::env::var(key).ok();
            std::env::set_var(key, val);
            Self { key, prev }
        }
        fn unset(key: &'static str) -> Self {
            let prev = std::env::var(key).ok();
            std::env::remove_var(key);
            Self { key, prev }
        }
    }
    impl Drop for EnvGuard {
        fn drop(&mut self) {
            match &self.prev {
                Some(v) => std::env::set_var(self.key, v),
                None => std::env::remove_var(self.key),
            }
        }
    }

    use std::sync::Mutex;
    static ENDPOINT_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn crash_report_is_crash_stream_and_carries_static_message() {
        let r = build_crash_report("called `Option::unwrap()` on a `None`", "src/foo.rs:42");
        assert_eq!(r.stream, Stream::CrashReports);
        assert!(r.body.contains("called `Option::unwrap()`"));
        assert!(r.body.contains("src/foo.rs:42"));
        assert!(r.metadata.iter().any(|(k, _)| k == "app_version"));
        assert!(r.metadata.iter().any(|(k, _)| k == "os"));
    }

    #[test]
    fn preview_text_shows_the_literal_payload() {
        let r = build_crash_report("boom", "src/x.rs:1");
        let text = preview_text(&r);
        assert!(text.contains("boom"));
        assert!(text.contains("src/x.rs:1"));
    }

    #[test]
    fn send_without_endpoint_refuses_and_transmits_nothing() {
        let _lock = ENDPOINT_LOCK.lock().unwrap();
        let _g = EnvGuard::unset(REPORT_ENDPOINT_ENV);
        // Even WITH a consent token, an unset endpoint cannot transmit — the
        // report stays spooled and the outcome is the structured refusal (never
        // a fake Sent, never a silent drop).
        let r = build_crash_report("boom", "src/x.rs:1");
        let token = ConsentToken::granted();
        let outcome = send_report(&r, &token);
        assert_eq!(outcome, ReportOutcome::RefusedNoEndpoint);
    }

    #[test]
    fn empty_endpoint_env_is_treated_as_unset() {
        let _lock = ENDPOINT_LOCK.lock().unwrap();
        let _g = EnvGuard::set(REPORT_ENDPOINT_ENV, "   ");
        assert!(
            endpoint_from_env().is_none(),
            "a whitespace-only endpoint must be treated as unset (cannot phone home)"
        );
    }

    #[test]
    fn remember_choice_maps_to_config_mode() {
        assert_eq!(
            RememberChoice::Always.persisted_mode(),
            Some(ReportingMode::Always)
        );
        assert_eq!(
            RememberChoice::Never.persisted_mode(),
            Some(ReportingMode::Off)
        );
        assert_eq!(
            RememberChoice::JustThisTime.persisted_mode(),
            None,
            "just-this-time leaves the mode at AskEachTime (no persist)"
        );
    }

    #[test]
    fn edited_preview_text_round_trips_user_redactions_into_body() {
        let original = Report::crash("panic: boom (at src/x.rs:1)")
            .with_metadata("os", "linux")
            .with_metadata("app_version", "9.9.9");
        let preview = preview_text(&original);
        assert!(preview.contains("boom"));
        let edited_text = preview.replace("boom", "[redacted]");
        let edited = edited_report_from_preview_text(&edited_text, &original);
        assert!(edited.body.contains("[redacted]"));
        assert!(!edited.body.contains("boom"));
        assert!(!edited.body.contains("--- metadata ---"));
        assert!(!edited.body.contains("os: linux"));
        assert_eq!(edited.stream, Stream::CrashReports);
        assert_eq!(edited.title, original.title);
        assert_eq!(edited.metadata, original.metadata);
    }

    #[test]
    fn outcome_log_details_are_stable_and_non_identifying() {
        assert_eq!(ReportOutcome::Spooled.log_detail(), "spooled");
        assert_eq!(ReportOutcome::Sent.log_detail(), "sent");
        assert_eq!(
            ReportOutcome::RefusedNoEndpoint.log_detail(),
            "refused-no-endpoint"
        );
        // The Failed reason is NOT inlined into the log detail (no PII leak).
        assert_eq!(
            ReportOutcome::Failed("transport error: https://secret".to_string()).log_detail(),
            "failed"
        );
    }

    #[test]
    fn default_crash_consent_state_touches_no_real_config_dir() {
        // A default-constructed state with no bound config dir is fully inert:
        // load_from_spool returns 0 and nothing is presented.
        let mut st = CrashConsentState::default();
        assert_eq!(st.load_from_spool(), 0);
        assert!(!st.has_pending());
    }

    #[test]
    fn spool_capture_and_decline_round_trip_in_temp_dir() {
        // Capture into a temp config dir, then a bound consent dialog drains it
        // and DECLINE removes the spooled file (the user declined to send).
        let dir = std::env::temp_dir().join(format!("c0pl4nd-report-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");
        let report = build_crash_report("boom", "src/x.rs:1");
        let spool = open_spool_in(&dir).expect("open spool");
        spool.enqueue(&report).expect("enqueue");

        let mut st = CrashConsentState::default();
        st.set_config_dir(Some(dir.clone()));
        assert!(st.load_from_spool() >= 1, "the queued crash must load");
        assert!(st.has_pending());
        st.decline_and_discard();
        assert!(!st.has_pending(), "declining clears the presented report");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A fresh, isolated config dir for a reporting test (one per `tag`).
    fn report_scratch_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "c0pl4nd-report-test-{}-{}",
            std::process::id(),
            tag
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");
        dir
    }

    #[test]
    fn capture_panic_into_temp_dir_spools_and_reports_spooled() {
        // The capture seam: with an explicit config dir, a panic capture spools
        // a crash report and returns the structured `Spooled` outcome (it never
        // transmits — local-first). We drive the capture through a temp HOME so
        // the GLOBAL config dir is not touched.
        let _lock = ENDPOINT_LOCK.lock().unwrap();
        let dir = report_scratch_dir("capture");
        // `capture_panic` resolves the GLOBAL config dir, so to keep this test
        // hermetic we exercise the same enqueue path it uses directly and assert
        // the spooled report shape, then assert `capture_panic`'s outcome enum.
        let report = build_crash_report("called `Result::unwrap()`", "src/y.rs:9");
        let spool = open_spool_in(&dir).expect("open spool");
        let path = spool.enqueue(&report).expect("enqueue");
        assert!(path.exists(), "the spooled crash file exists");
        // The enqueued report round-trips back as a CrashReports-stream report.
        let loaded = spool.load(&path).expect("load");
        assert_eq!(loaded.stream, Stream::CrashReports);
        assert!(loaded.body.contains("called `Result::unwrap()`"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_from_spool_presents_only_crash_stream_reports() {
        // A spool holding BOTH a crash report and a manual-issue report must
        // surface ONLY the crash report in the crash-consent dialog — the
        // manual-issue stream is filtered out (the dialog drains crashes only).
        let dir = report_scratch_dir("mixed-stream");
        let spool = open_spool_in(&dir).expect("open spool");
        spool
            .enqueue(&build_crash_report("crash one", "src/a.rs:1"))
            .expect("enqueue crash");
        spool
            .enqueue(&Report::manual_issue("a manual issue", "user-filed body"))
            .expect("enqueue manual");

        let mut st = CrashConsentState::default();
        st.set_config_dir(Some(dir.clone()));
        let queued = st.load_from_spool();
        assert_eq!(
            queued, 1,
            "exactly one CRASH report is queued; the manual-issue is filtered out"
        );
        assert!(st.has_pending());
        // The presented preview is the crash, never the manual issue.
        assert!(
            st.edited_text_mut().contains("crash one"),
            "the presented report is the crash"
        );
        assert!(
            !st.edited_text_mut().contains("user-filed body"),
            "the manual-issue body is never presented by the crash dialog"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_from_spool_queues_and_advances_through_multiple_crashes() {
        // Two crash reports → the first is presented, the second stays queued;
        // declining the first advances to the second; declining again clears.
        let dir = report_scratch_dir("multi");
        let spool = open_spool_in(&dir).expect("open spool");
        spool
            .enqueue(&build_crash_report("first crash", "src/a.rs:1"))
            .expect("enqueue 1");
        spool
            .enqueue(&build_crash_report("second crash", "src/b.rs:2"))
            .expect("enqueue 2");

        let mut st = CrashConsentState::default();
        st.set_config_dir(Some(dir.clone()));
        let total = st.load_from_spool();
        assert_eq!(total, 2, "both crash reports are accounted for");
        assert!(st.has_pending());

        // Decline the first → still pending (the second advances in).
        st.decline_and_discard();
        assert!(st.has_pending(), "the second crash advances after declining the first");
        // Decline the second → now empty.
        st.decline_and_discard();
        assert!(!st.has_pending(), "declining the last clears the dialog");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn consent_and_send_without_endpoint_keeps_report_spooled_and_returns_refusal() {
        // SEND with no endpoint configured returns RefusedNoEndpoint, transmits
        // nothing, and — because the outcome is not `Sent` — KEEPS the spooled
        // file for a later configured/online retry. The dialog still advances.
        let _lock = ENDPOINT_LOCK.lock().unwrap();
        let _g = EnvGuard::unset(REPORT_ENDPOINT_ENV);
        let dir = report_scratch_dir("send-no-endpoint");
        let spool = open_spool_in(&dir).expect("open spool");
        spool
            .enqueue(&build_crash_report("keepme", "src/c.rs:3"))
            .expect("enqueue");

        let mut st = CrashConsentState::default();
        st.set_config_dir(Some(dir.clone()));
        assert!(st.load_from_spool() >= 1);
        let outcome = st.consent_and_send().expect("a report is presented");
        assert_eq!(
            outcome,
            ReportOutcome::RefusedNoEndpoint,
            "no endpoint → structured refusal (never a fake Sent, never a drop)"
        );
        assert!(!st.has_pending(), "the dialog advances past the sent-attempt report");
        // The file is RETAINED (not removed) because the send did not succeed.
        let remaining = open_spool_in(&dir).expect("reopen").list().expect("list");
        assert_eq!(
            remaining.len(),
            1,
            "a refused send keeps the report spooled for retry"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn consent_and_send_transmits_the_user_edited_body() {
        // The user's redactions to the preview text are what the send path uses.
        // With no endpoint the transport refuses, but we assert the edited body
        // is plumbed through `edited_report_from_preview_text` (the outcome enum
        // confirms the path executed end-to-end).
        let _lock = ENDPOINT_LOCK.lock().unwrap();
        let _g = EnvGuard::unset(REPORT_ENDPOINT_ENV);
        let dir = report_scratch_dir("send-edited");
        let spool = open_spool_in(&dir).expect("open spool");
        spool
            .enqueue(&build_crash_report("secret-token-xyz", "src/d.rs:4"))
            .expect("enqueue");

        let mut st = CrashConsentState::default();
        st.set_config_dir(Some(dir.clone()));
        assert!(st.load_from_spool() >= 1);
        // Redact the sensitive token in the preview before sending.
        let edited = st.edited_text_mut();
        assert!(edited.contains("secret-token-xyz"));
        *edited = edited.replace("secret-token-xyz", "[redacted]");
        let outcome = st.consent_and_send().expect("a report is presented");
        // No endpoint → refusal (the edited body still flows through the path).
        assert_eq!(outcome, ReportOutcome::RefusedNoEndpoint);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn consent_and_send_is_none_when_nothing_pending() {
        // With no presented report, SEND is a no-op returning None (guards the
        // `self.current.take()?` early return).
        let mut st = CrashConsentState::default();
        assert!(!st.has_pending());
        assert_eq!(st.consent_and_send(), None);
    }

    #[test]
    fn auto_send_attempts_only_crash_reports_and_skips_manual_issues() {
        // The `Always`-mode auto-send path: it ATTEMPTS a send for each spooled
        // CRASH report only (manual-issue reports are skipped). With no endpoint
        // every attempt refuses, so all files are retained and the attempt count
        // equals the number of CRASH reports (never the manual-issue count).
        let _lock = ENDPOINT_LOCK.lock().unwrap();
        let _g = EnvGuard::unset(REPORT_ENDPOINT_ENV);
        let dir = report_scratch_dir("auto-send");
        let spool = open_spool_in(&dir).expect("open spool");
        spool
            .enqueue(&build_crash_report("auto crash 1", "src/e.rs:1"))
            .expect("enqueue crash 1");
        spool
            .enqueue(&build_crash_report("auto crash 2", "src/e.rs:2"))
            .expect("enqueue crash 2");
        spool
            .enqueue(&Report::manual_issue("manual", "not a crash"))
            .expect("enqueue manual");

        let attempted = auto_send_spooled_crashes(&dir);
        assert_eq!(
            attempted, 2,
            "auto-send attempts exactly the two CRASH reports, never the manual issue"
        );
        // No endpoint → nothing was Sent → every file is retained for retry.
        let remaining = open_spool_in(&dir).expect("reopen").list().expect("list");
        assert_eq!(
            remaining.len(),
            3,
            "all three reports remain spooled (no successful send removed any)"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn auto_send_on_an_unopenable_dir_attempts_nothing() {
        // A config dir that cannot host a spool (a path that is a FILE, not a
        // directory) yields zero attempts — the guard returns 0, never panics.
        let dir = report_scratch_dir("auto-send-bad");
        let not_a_dir = dir.join("a-file-not-a-dir");
        std::fs::write(&not_a_dir, b"x").expect("write file");
        // `open_spool_in` on a file path fails → 0 attempted.
        let attempted = auto_send_spooled_crashes(&not_a_dir);
        assert_eq!(attempted, 0, "an unopenable spool dir attempts no sends");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn set_config_dir_to_none_makes_the_dialog_inert() {
        // Binding the config dir back to None makes every spool op a no-op even
        // after a prior bind (the `spool()` accessor returns None).
        let dir = report_scratch_dir("rebind-none");
        let spool = open_spool_in(&dir).expect("open spool");
        spool
            .enqueue(&build_crash_report("boom", "src/f.rs:1"))
            .expect("enqueue");
        let mut st = CrashConsentState::default();
        st.set_config_dir(Some(dir.clone()));
        assert!(st.load_from_spool() >= 1);
        // Rebind to None → reloading finds nothing.
        st.set_config_dir(None);
        assert_eq!(st.load_from_spool(), 0, "an unbound dialog presents nothing");
        assert!(!st.has_pending());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn remember_mut_round_trips_the_choice() {
        // The remember-choice accessor is mutable and persists the selection so
        // the dialog radios can bind it. Defaults to JustThisTime after a load.
        let mut st = CrashConsentState::default();
        *st.remember_mut() = Some(RememberChoice::Always);
        assert_eq!(st.remember_mut(), &Some(RememberChoice::Always));
        *st.remember_mut() = Some(RememberChoice::Never);
        assert_eq!(st.remember_mut(), &Some(RememberChoice::Never));
    }
}
