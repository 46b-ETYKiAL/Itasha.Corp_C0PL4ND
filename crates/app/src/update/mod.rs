//! Telemetry-free, opt-in update CHECK.
//!
//! C0PL4ND never phones home on its own. The version check runs only when the
//! user invokes `c0pl4nd update` or explicitly sets `[update] check_on_launch =
//! true`. The single network surface is the **public GitHub Releases API** for
//! this repository — unauthenticated, zero PII, no custom server, no shipped
//! token. The check is offline-graceful: an unreachable API is reported as
//! "could not check", never an error that blocks the app.
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

use anyhow::{Context, Result};

/// Canonical public release repository. (The prior value `itasha-corp/c0pl4nd`
/// 404'd — there is no such repo — so every check silently failed.)
const REPO: &str = "46b-ETYKiAL/Itasha.Corp_C0PL4ND";

/// `User-Agent` for the one API call. App name + version ONLY — no PII. The
/// GitHub API rejects requests without a User-Agent, so this is mandatory.
const USER_AGENT: &str = concat!("c0pl4nd-updater/", env!("CARGO_PKG_VERSION"));

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
    let fetch = |url: &str| -> Result<String> {
        ureq::get(url)
            .set("User-Agent", USER_AGENT)
            .set("Accept", "application/vnd.github+json")
            .call()
            .context("failed to reach GitHub Releases")?
            .into_string()
            .context("failed to read release response")
    };
    if channel.eq_ignore_ascii_case("stable") {
        let body = fetch(&format!(
            "https://api.github.com/repos/{REPO}/releases/latest"
        ))?;
        parse_tag(&body).context("no tag_name in latest release")
    } else {
        let body = fetch(&format!(
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
}
