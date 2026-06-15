//! Telemetry-free update CHECK.
//!
//! The version check runs on launch under the default `notify` mode (and
//! `auto`), when the user invokes `c0pl4nd update`, or when the legacy
//! `[update] check_on_launch = true` flag is set; `manual`/`off` suppress it.
//! The single network surface is the **public GitHub Releases API** for this
//! repository — unauthenticated, zero PII, no custom server, no shipped token,
//! reached over a host-confined https GET ([`confined_api_get`]). The check is
//! offline-graceful: an unreachable API is reported as "could not check", never
//! an error that blocks the app.
//!
//! Scope: this module CHECKS for a newer release and points the user at the
//! download page. It does NOT download-and-apply a binary in place — that
//! ("auto" install) requires cryptographically signed release artifacts (a
//! minisign keypair + a release-workflow signing step) so the updater can
//! verify-before-swap; until that signing infrastructure exists, directing the
//! user to the signed GitHub release is the safe, honest path. The decision
//! logic here ([`parse_tag`], [`pick_channel_tag`], [`is_newer`]) is pure and
//! unit-tested without any network.
//!
//! ## In-app verified self-updater
//!
//! The CLI surface in THIS module is the legacy, browser-pointing path. The
//! richer **verify-before-swap** in-app updater (download + SHA-256/minisign
//! verify + atomic `self-replace`, mirroring the SCR1B3 editor) lives in the
//! sibling `update_engine/` module, which the Settings → Updates page drives.
//! It is kept separate so the egui-only updater backend (and its `eframe`
//! dependency) is compiled exactly once — by the egui app + the egui test
//! binaries — without forcing it onto the legacy winit `c0pl4nd-legacy` binary
//! that also uses this CLI module.

use std::io::Read;
use std::time::Duration;

use anyhow::{Context, Result};

/// Canonical public release repository. (The prior value `itasha-corp/c0pl4nd`
/// 404'd — there is no such repo — so every check silently failed.)
const REPO: &str = "46b-ETYKiAL/Itasha.Corp_C0PL4ND";

/// `User-Agent` for the one API call. App name + version ONLY — no PII. The
/// GitHub API rejects requests without a User-Agent, so this is mandatory.
const USER_AGENT: &str = concat!("c0pl4nd-updater/", env!("CARGO_PKG_VERSION"));

/// Largest Releases-API JSON body the check will read. A real `releases/latest`
/// or `releases?per_page=20` response is far smaller; the cap stops a hostile or
/// MITM'd endpoint from streaming an unbounded body that could OOM the process.
const MAX_RELEASE_JSON_BYTES: u64 = 4 * 1024 * 1024;

/// Redirect hops followed MANUALLY, each re-confined to https + an allow-listed
/// host. A normal GitHub REST response is a direct 200, so this is headroom.
const MAX_REDIRECTS: usize = 4;

/// The only host this version check may contact. It calls the GitHub REST API
/// and nothing else, so the allow-list is exactly that host — a MITM'd or
/// hostile redirect cannot point the request (and our `User-Agent`) at an
/// arbitrary server. This mirrors the hardened `update_engine::net` allow-list;
/// it is duplicated here rather than shared because this dependency-light module
/// is ALSO compiled into the legacy `c0pl4nd-legacy` binary, which excludes the
/// `update_engine`/eframe stack.
fn host_allowed(host: &str) -> bool {
    host.eq_ignore_ascii_case("api.github.com")
}

/// Lowercased host of an `https://host[:port]/...` URL (strips userinfo + port).
fn url_host(url: &str) -> Option<String> {
    let after = url.split_once("://")?.1;
    let authority = after.split(['/', '?', '#']).next()?;
    let host_port = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    let host = host_port.split(':').next()?;
    if host.is_empty() {
        return None;
    }
    Some(host.to_ascii_lowercase())
}

/// Refuse any URL that is not `https://` to an allow-listed host. Re-checked at
/// every redirect hop by [`confined_api_get`].
fn assert_confined(url: &str) -> Result<()> {
    let https = url
        .split_once("://")
        .map(|(scheme, _)| scheme.eq_ignore_ascii_case("https"))
        .unwrap_or(false);
    if !https {
        anyhow::bail!("refusing non-https update URL: {url}");
    }
    match url_host(url) {
        Some(h) if host_allowed(&h) => Ok(()),
        Some(h) => anyhow::bail!("refusing update request to non-allowlisted host: {h}"),
        None => anyhow::bail!("malformed update URL (no host): {url}"),
    }
}

/// Resolve a redirect `Location` against the current URL. Absolute targets pass
/// through (their host is re-validated by the caller); origin-relative (`/path`)
/// targets keep the current scheme+host; anything else is refused.
fn resolve_redirect(base: &str, loc: &str) -> Result<String> {
    if loc.contains("://") {
        Ok(loc.to_string())
    } else if let Some(rest) = loc.strip_prefix('/') {
        let (scheme, after) = base
            .split_once("://")
            .with_context(|| format!("malformed base URL: {base}"))?;
        let host = after.split(['/', '?', '#']).next().unwrap_or(after);
        Ok(format!("{scheme}://{host}/{rest}"))
    } else {
        anyhow::bail!("unsupported relative redirect target: {loc}")
    }
}

/// Host-confined, redirect-controlled, size-capped GET of a GitHub REST URL.
/// ureq's default agent follows up to 5 redirects to ARBITRARY hosts; this
/// builds a `redirects(0)` agent and walks the chain itself, re-asserting https
/// AND an allow-listed host at EVERY hop, then reads at most
/// [`MAX_RELEASE_JSON_BYTES`]. Connect/read timeouts bound a hung launch thread.
fn confined_api_get(url: &str) -> Result<String> {
    assert_confined(url)?;
    let agent = ureq::AgentBuilder::new()
        .redirects(0)
        .timeout_connect(Duration::from_secs(10))
        .timeout_read(Duration::from_secs(20))
        .build();
    let mut current = url.to_string();
    for _ in 0..=MAX_REDIRECTS {
        // With redirects(0) a 3xx returns Ok (status in 300..400); ureq still
        // maps >=400 to Err(Status). Accept a 3xx from either shape.
        let resp = match agent
            .get(&current)
            .set("User-Agent", USER_AGENT)
            .set("Accept", "application/vnd.github+json")
            .call()
        {
            Ok(r) => r,
            Err(ureq::Error::Status(code, r)) if (300..400).contains(&code) => r,
            Err(e) => return Err(anyhow::anyhow!("failed to reach GitHub Releases: {e}")),
        };
        if (300..400).contains(&resp.status()) {
            let loc = resp
                .header("Location")
                .with_context(|| format!("redirect {} without Location", resp.status()))?;
            let next = resolve_redirect(&current, loc)?;
            assert_confined(&next)?;
            current = next;
            continue;
        }
        let mut body = String::new();
        resp.into_reader()
            .take(MAX_RELEASE_JSON_BYTES)
            .read_to_string(&mut body)
            .context("failed to read release response")?;
        return Ok(body);
    }
    anyhow::bail!("too many redirects (> {MAX_REDIRECTS}) fetching {url}")
}

/// The running binary's version.
pub fn current_version() -> &'static str {
    c0pl4nd_core::version()
}

/// The human-facing page where a user downloads the latest signed release.
pub fn release_page_url() -> String {
    format!("https://github.com/{REPO}/releases/latest")
}

/// Extract the `tag_name` from a SINGLE-release JSON body (the
/// `…/releases/latest` response), stripping a leading `v`. Pure string scan —
/// no JSON dependency, matching the crate's dependency-light posture.
fn parse_tag(body: &str) -> Option<String> {
    // After splitting on the literal key, the remainder starts at the `:`, so
    // the value is the FIRST quoted token: `:"v0.4.1",…` → split('"') yields
    // [":", "v0.4.1", …], i.e. index 1.
    body.split("\"tag_name\"")
        .nth(1)?
        .split('"')
        .nth(1)
        .map(|s| s.trim_start_matches('v').to_string())
}

/// From a release-LIST JSON body (the `…/releases` response, which GitHub
/// returns newest-first), pick the newest tag whose name contains `channel`
/// (e.g. `beta`/`nightly`), falling back to the newest tag of ANY kind when no
/// release matches the channel. `None` only when the list is empty.
fn pick_channel_tag(body: &str, channel: &str) -> Option<String> {
    let tags: Vec<&str> = body
        .split("\"tag_name\"")
        .skip(1)
        .filter_map(|s| s.split('"').nth(1))
        .collect();
    let channel = channel.to_lowercase();
    tags.iter()
        .find(|tag| tag.to_lowercase().contains(&channel))
        .or_else(|| tags.first())
        .map(|s| s.trim_start_matches('v').to_string())
}

/// Compare dotted numeric versions; `true` when `latest` is strictly newer than
/// `current`. Leading `v` is ignored; a non-numeric suffix on a component
/// (`1.2.0-beta`) compares on the numeric prefix.
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

/// Query GitHub Releases for the newest version tag on `channel`. Network call —
/// only invoked on explicit user action or the opt-in launch check. `stable`
/// reads the dedicated `releases/latest` endpoint (newest non-prerelease);
/// other channels scan the full `releases` list for a matching tag.
pub fn latest_version(channel: &str) -> Result<String> {
    if channel.eq_ignore_ascii_case("stable") {
        let body = confined_api_get(&format!(
            "https://api.github.com/repos/{REPO}/releases/latest"
        ))?;
        parse_tag(&body).context("no tag_name in latest release")
    } else {
        let body = confined_api_get(&format!(
            "https://api.github.com/repos/{REPO}/releases?per_page=20"
        ))?;
        pick_channel_tag(&body, channel).context("no releases found")
    }
}

/// Check `channel` for a newer release and return a one-line user notice when
/// one exists, else `None`. Offline-graceful: a failed check returns `None`
/// (never an error) so a launch check never blocks or alarms.
pub fn check_for_update(channel: &str) -> Option<String> {
    let latest = latest_version(channel).ok()?;
    if is_newer(&latest, current_version()) {
        Some(format!(
            "C0PL4ND {latest} is available (you have {}). Download: {}",
            current_version(),
            release_page_url()
        ))
    } else {
        None
    }
}

/// `c0pl4nd update` — an explicit, user-initiated check. Prints whether a newer
/// release exists and, if so, the download page. Does NOT install in place (see
/// the module docs): the user downloads the signed release. `channel` selects
/// the release stream.
pub fn run_update(channel: &str) -> Result<()> {
    println!(
        "C0PL4ND updater — current version {} (channel: {channel})",
        current_version()
    );
    match latest_version(channel) {
        Ok(latest) if is_newer(&latest, current_version()) => {
            println!("A newer version is available: {latest}");
            println!("Download the signed release at: {}", release_page_url());
        }
        Ok(latest) => {
            println!("You are up to date ({latest}).");
        }
        Err(e) => {
            // Offline / API unreachable is not a failure of the command.
            eprintln!("Could not check for updates ({e}). You are offline or the");
            eprintln!("GitHub Releases API is unreachable; nothing was changed.");
        }
    }
    Ok(())
}

// ----------------------------------------------------------------------------
// On-launch check throttle (`[update] check_interval_hours`)
// ----------------------------------------------------------------------------
//
// The default `notify` mode runs a version check on launch. Without a throttle
// that would hit the GitHub REST API on EVERY launch and risk the 60-req/hr
// unauthenticated rate limit. We persist the Unix-seconds timestamp of the last
// launch check in a tiny sibling file next to the config and only re-check once
// `check_interval_hours` have elapsed. The decision ([`is_due`]) is pure and
// unit-tested; the I/O wrappers are best-effort (a read/write failure just means
// the next launch re-checks, which is safe).

/// Path of the file recording the last on-launch check time (Unix seconds).
/// Sibling of the config file so it lives in the same per-user config dir.
fn check_state_path() -> Option<std::path::PathBuf> {
    let cfg = c0pl4nd_core::Config::default_path()?;
    let dir = cfg.parent()?;
    Some(dir.join("last-update-check"))
}

/// Seconds since the Unix epoch, or `None` if the system clock predates it.
fn now_unix() -> Option<u64> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|d| d.as_secs())
}

/// Read the recorded last-check time (Unix seconds), if present and parseable.
fn read_last_check() -> Option<u64> {
    let path = check_state_path()?;
    std::fs::read_to_string(path).ok()?.trim().parse().ok()
}

/// Record "a launch check happened now" so the next launch within
/// `check_interval_hours` skips the network. Best-effort: a write failure just
/// means the next launch re-checks (no throttle), which is safe. Called AFTER
/// every launch-check attempt regardless of its outcome, so a transient offline
/// launch does not spam the API on every subsequent start.
pub fn record_check_now() {
    if let (Some(path), Some(now)) = (check_state_path(), now_unix()) {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(path, now.to_string());
    }
}

/// PURE due-ness decision. A check is due when:
/// * there is no recorded last-check (`last == None`), or
/// * the interval is `0` (throttle disabled), or
/// * `last` is in the FUTURE relative to `now` (clock moved backwards — re-check
///   rather than suppress indefinitely), or
/// * at least `interval_hours` have elapsed since `last`.
fn is_due(now: u64, last: Option<u64>, interval_hours: u32) -> bool {
    let Some(last) = last else {
        return true;
    };
    if interval_hours == 0 || last > now {
        return true;
    }
    now - last >= u64::from(interval_hours) * 3600
}

/// Whether an on-launch update check is due now, given the configured interval.
/// A clock error (epoch unreadable) is treated as "due" so a check is never
/// permanently suppressed.
pub fn check_due(interval_hours: u32) -> bool {
    let now = now_unix().unwrap_or(u64::MAX);
    is_due(now, read_last_check(), interval_hours)
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
        // Numeric-prefix comparison tolerates a channel suffix on a component.
        assert!(is_newer("1.3.0-beta", "1.2.0"));
    }

    #[test]
    fn current_version_nonempty() {
        assert!(!current_version().is_empty());
    }

    #[test]
    fn release_page_url_points_at_the_real_repo() {
        let url = release_page_url();
        assert!(
            url.contains("46b-ETYKiAL/Itasha.Corp_C0PL4ND"),
            "release URL must target the real repo, not the old 404 path: {url}"
        );
        assert!(
            !url.contains("itasha-corp/c0pl4nd"),
            "must not use the dead path"
        );
    }

    #[test]
    fn parse_tag_extracts_and_strips_v() {
        let body = r#"{"url":"x","tag_name":"v0.4.1","name":"0.4.1"}"#;
        assert_eq!(parse_tag(body).as_deref(), Some("0.4.1"));
        // No tag_name → None (drives the offline-graceful path).
        assert_eq!(parse_tag(r#"{"message":"Not Found"}"#), None);
    }

    #[test]
    fn pick_channel_tag_prefers_channel_then_falls_back() {
        // Newest-first list with a beta and two stables.
        let body = r#"
            {"tag_name":"v0.5.0-beta.1"},
            {"tag_name":"v0.4.2"},
            {"tag_name":"v0.4.1"}
        "#;
        assert_eq!(
            pick_channel_tag(body, "beta").as_deref(),
            Some("0.5.0-beta.1")
        );
        // A channel with no matching release falls back to the newest tag.
        assert_eq!(
            pick_channel_tag(body, "nightly").as_deref(),
            Some("0.5.0-beta.1")
        );
        // Empty list → None.
        assert_eq!(pick_channel_tag("{}", "beta"), None);
    }

    #[test]
    fn assert_confined_allows_only_https_github_api() {
        // The real endpoints this module uses are accepted.
        assert!(assert_confined("https://api.github.com/repos/x/y/releases/latest").is_ok());
        // Plain http is refused (TLS-downgrade defense).
        assert!(assert_confined("http://api.github.com/repos/x/y/releases/latest").is_err());
        // A different host (e.g. a MITM redirect target) is refused even over https.
        assert!(assert_confined("https://evil.example.com/releases").is_err());
        // github.com (not the API host) is NOT on this module's allow-list.
        assert!(assert_confined("https://github.com/x/y").is_err());
        // Userinfo cannot smuggle a foreign host past the check.
        assert!(assert_confined("https://api.github.com@evil.example.com/").is_err());
        // Malformed (no host) is refused.
        assert!(assert_confined("https:///releases").is_err());
    }

    #[test]
    fn url_host_strips_userinfo_and_port() {
        assert_eq!(
            url_host("https://api.github.com/x").as_deref(),
            Some("api.github.com")
        );
        assert_eq!(
            url_host("https://api.github.com:443/x").as_deref(),
            Some("api.github.com")
        );
        assert_eq!(
            url_host("https://user:pw@evil.example.com/x").as_deref(),
            Some("evil.example.com")
        );
        assert_eq!(url_host("https:///x"), None);
    }

    #[test]
    fn resolve_redirect_keeps_origin_for_relative_and_passes_absolute() {
        // Origin-relative redirect keeps the current scheme + host.
        assert_eq!(
            resolve_redirect("https://api.github.com/a", "/b/c").unwrap(),
            "https://api.github.com/b/c"
        );
        // Absolute target passes through (the caller re-validates its host).
        assert_eq!(
            resolve_redirect("https://api.github.com/a", "https://api.github.com/d").unwrap(),
            "https://api.github.com/d"
        );
        // A scheme-relative / weird relative target is refused outright.
        assert!(resolve_redirect("https://api.github.com/a", "b/c").is_err());
    }

    #[test]
    fn is_due_when_never_checked() {
        assert!(
            is_due(1_000_000, None, 24),
            "no recorded check → always due"
        );
    }

    #[test]
    fn is_due_respects_the_interval() {
        let last = 1_000_000u64;
        let day = 24 * 3600;
        // Exactly at the interval boundary → due.
        assert!(is_due(last + day, Some(last), 24));
        // One second past → due.
        assert!(is_due(last + day + 1, Some(last), 24));
        // One second short → NOT due (throttled).
        assert!(!is_due(last + day - 1, Some(last), 24));
        // Same instant → not due.
        assert!(!is_due(last, Some(last), 24));
    }

    #[test]
    fn is_due_when_interval_is_zero_disables_throttle() {
        assert!(
            is_due(1_000_000, Some(1_000_000), 0),
            "interval 0 means check every launch"
        );
    }

    #[test]
    fn is_due_when_clock_moved_backwards() {
        // Recorded time is in the FUTURE relative to now (clock rolled back):
        // re-check rather than suppress forever.
        assert!(is_due(1_000, Some(5_000), 24));
    }
}
