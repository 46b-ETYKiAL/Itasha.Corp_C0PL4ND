//! Network half of the in-app self-updater.
//!
//! Telemetry-free by construction: the only network surfaces are
//! 1. a single unauthenticated `GET` of the public GitHub Releases API, and
//! 2. downloads of the release archive + its `.minisig` + `.sha256` siblings.
//!
//! No analytics, no identifiers, no payload: every request sends only a generic
//! `User-Agent` (app name + version), and the asset is verified (SHA-256 THEN
//! minisign against [`super::verify::EMBEDDED_PUBLIC_KEY`]) before the extracted
//! binary is ever returned. A verify failure deletes the staging area and the
//! binary is NEVER returned unverified.
//!
//! Pure decision logic ([`select_update`]) is split out from the I/O so it can
//! be unit-tested offline against a fixture [`RawRelease`].
//!
//! ## Asset naming
//!
//! C0PL4ND's release workflow publishes, per target, an archive whose name
//! embeds both the tag and the Rust target triple — `c0pl4nd-<tag>-<target>.zip`
//! on Windows, `c0pl4nd-<tag>-<target>.tar.gz` on Unix — plus a `.sha256` and a
//! `.minisig` sidecar. Because the tag is part of the name, [`select_update`]
//! matches by the **target-triple substring + archive extension** rather than an
//! exact filename, so it is robust to the tag prefix.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use super::verify::{verify_artifact, EMBEDDED_PUBLIC_KEY};

/// Mandatory `User-Agent` for every request. App name + version ONLY — no
/// machine identifier, OS fingerprint, install ID, or any unique token.
const USER_AGENT: &str = concat!("c0pl4nd-updater/", env!("CARGO_PKG_VERSION"));

/// GitHub REST API version header value.
const GITHUB_API_VERSION: &str = "2026-03-10";

/// GitHub Releases API `Accept` header value.
const GITHUB_ACCEPT: &str = "application/vnd.github+json";

/// A single release asset as returned by the GitHub Releases API. Only the
/// fields the updater needs are deserialized.
#[derive(Clone, Debug, Deserialize)]
pub struct RawAsset {
    pub name: String,
    pub browser_download_url: String,
}

/// The subset of the GitHub `releases/latest` JSON the updater reads. Made
/// public + constructible so [`select_update`] can be unit-tested with a
/// fixture (no network).
#[derive(Clone, Debug, Deserialize)]
pub struct RawRelease {
    pub tag_name: String,
    #[serde(default)]
    pub prerelease: bool,
    #[serde(default)]
    pub draft: bool,
    #[serde(default)]
    pub html_url: String,
    #[serde(default)]
    pub assets: Vec<RawAsset>,
}

/// One resolved, newer-than-current release ready to download.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReleaseInfo {
    pub version: semver::Version,
    /// The original tag string (e.g. `v0.4.0`).
    pub tag: String,
    /// The archive (`.zip`/`.tar.gz`) browser_download_url.
    pub asset_url: String,
    /// The archive name (used to pick the `.zip` vs `.tar.gz` extractor).
    pub asset_name: String,
    /// The `.minisig` url.
    pub sig_url: String,
    /// The `.sha256` url.
    pub sha_url: String,
    /// The release page (for "view all releases" / changelog in a browser).
    pub html_url: String,
}

/// Parse a release `tag_name` into a [`semver::Version`], tolerating a single
/// leading `v`. Returns `None` on malformed input (the caller treats that as
/// "no update", never a crash).
fn parse_tag(tag: &str) -> Option<semver::Version> {
    let s = tag.trim();
    let s = s.strip_prefix('v').unwrap_or(s);
    semver::Version::parse(s).ok()
}

/// The archive file extension this build's release artifact carries: `.zip` on
/// Windows, `.tar.gz` elsewhere (matches `release.yml`'s per-OS packaging).
pub const fn archive_ext() -> &'static str {
    if cfg!(windows) {
        ".zip"
    } else {
        ".tar.gz"
    }
}

/// Blocking GET of `/repos/{owner}/{repo}/releases/latest`. Any network/HTTP/
/// decode error is mapped to a human `String`; this function never panics.
pub fn fetch_latest_release(owner: &str, repo: &str) -> Result<RawRelease, String> {
    let url = format!("https://api.github.com/repos/{owner}/{repo}/releases/latest");
    let body = ureq::get(&url)
        .set("User-Agent", USER_AGENT)
        .set("Accept", GITHUB_ACCEPT)
        .set("X-GitHub-Api-Version", GITHUB_API_VERSION)
        .call()
        .map_err(|e| format!("failed to fetch latest release: {e}"))?
        .into_string()
        .map_err(|e| format!("failed to read release response: {e}"))?;
    serde_json::from_str::<RawRelease>(&body)
        .map_err(|e| format!("failed to parse release JSON: {e}"))
}

/// PURE (no network) decision: given the raw release, the current version, this
/// build's target triple, and the archive extension, return `Some(ReleaseInfo)`
/// when the release is newer AND a matching archive asset (containing `target`
/// and ending in `ext`) is present WITH both `.minisig` and `.sha256` siblings;
/// `None` when up-to-date, malformed, a prerelease/draft, or no matching asset
/// triple exists. The archive is matched by **substring** (`<target>` +
/// `<ext>`), so the tag prefix in the filename does not matter.
pub fn select_update(
    raw: &RawRelease,
    current: &semver::Version,
    target: &str,
    ext: &str,
) -> Option<ReleaseInfo> {
    if raw.prerelease || raw.draft {
        return None;
    }
    let latest = parse_tag(&raw.tag_name)?;
    if latest <= *current {
        return None;
    }
    if target.is_empty() {
        return None; // no baked target triple -> no asset can match this build
    }

    // The archive: an asset whose name contains the target triple AND ends in
    // the platform extension, but is NOT itself a sidecar.
    let archive = raw.assets.iter().find(|a| {
        a.name.contains(target)
            && a.name.ends_with(ext)
            && !a.name.ends_with(".minisig")
            && !a.name.ends_with(".sha256")
    })?;

    let sig_name = format!("{}.minisig", archive.name);
    let sha_name = format!("{}.sha256", archive.name);
    let find = |name: &str| -> Option<&str> {
        raw.assets
            .iter()
            .find(|a| a.name == name)
            .map(|a| a.browser_download_url.as_str())
    };
    let sig_url = find(&sig_name)?;
    let sha_url = find(&sha_name)?;

    Some(ReleaseInfo {
        version: latest,
        tag: raw.tag_name.clone(),
        asset_url: archive.browser_download_url.clone(),
        asset_name: archive.name.clone(),
        sig_url: sig_url.to_string(),
        sha_url: sha_url.to_string(),
        html_url: raw.html_url.clone(),
    })
}

/// Convenience: fetch + select in one blocking call (the worker thread calls
/// this). `Ok(None)` means "up to date / no matching asset"; `Err` means the
/// network fetch itself failed.
pub fn check_for_update(
    owner: &str,
    repo: &str,
    current: &semver::Version,
    target: &str,
) -> Result<Option<ReleaseInfo>, String> {
    let raw = fetch_latest_release(owner, repo)?;
    Ok(select_update(&raw, current, target, archive_ext()))
}

/// Blocking GET of a small file (sig / sha), returning its raw bytes.
fn download_small(url: &str) -> Result<Vec<u8>, String> {
    let mut buf = Vec::new();
    ureq::get(url)
        .set("User-Agent", USER_AGENT)
        .call()
        .map_err(|e| format!("download failed for {url}: {e}"))?
        .into_reader()
        .read_to_end(&mut buf)
        .map_err(|e| format!("read failed for {url}: {e}"))?;
    Ok(buf)
}

/// Blocking GET of a large asset, streaming the body to drive `progress`
/// (`downloaded`, `total`). `total` is read from `Content-Length`; if absent it
/// is reported as `0` (the UI shows an indeterminate bar). Returns the full
/// asset bytes.
fn download_asset(url: &str, mut progress: impl FnMut(u64, u64)) -> Result<Vec<u8>, String> {
    let resp = ureq::get(url)
        .set("User-Agent", USER_AGENT)
        .call()
        .map_err(|e| format!("download failed for {url}: {e}"))?;

    let total: u64 = resp
        .header("Content-Length")
        .and_then(|s| s.trim().parse::<u64>().ok())
        .unwrap_or(0);

    let mut reader = resp.into_reader();
    let mut buf: Vec<u8> = Vec::with_capacity(total as usize);
    let mut chunk = [0u8; 64 * 1024];
    let mut downloaded: u64 = 0;
    progress(0, total);
    loop {
        let n = reader
            .read(&mut chunk)
            .map_err(|e| format!("read failed for {url}: {e}"))?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
        downloaded += n as u64;
        progress(downloaded, total);
    }
    Ok(buf)
}

/// The expected binary file name on this platform.
fn binary_file_names() -> [&'static str; 2] {
    ["c0pl4nd", "c0pl4nd.exe"]
}

/// Mark `path` executable (`0o755`) on unix; a no-op on other platforms.
#[cfg(unix)]
fn set_executable(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = fs::metadata(path)
        .map_err(|e| format!("failed to stat extracted binary: {e}"))?
        .permissions();
    perms.set_mode(0o755);
    fs::set_permissions(path, perms).map_err(|e| format!("failed to chmod extracted binary: {e}"))
}

#[cfg(not(unix))]
fn set_executable(_path: &Path) -> Result<(), String> {
    Ok(())
}

/// Extract the single `c0pl4nd` / `c0pl4nd.exe` binary entry from a `.tar.gz`
/// archive's bytes into `dir`, returning the path to the extracted file. On
/// unix the extracted file is made executable (`0o755`).
fn extract_binary_targz(archive_bytes: &[u8], dir: &Path) -> Result<PathBuf, String> {
    let gz = flate2::read::GzDecoder::new(archive_bytes);
    let mut archive = tar::Archive::new(gz);
    let entries = archive
        .entries()
        .map_err(|e| format!("failed to read tar entries: {e}"))?;
    for entry in entries {
        let mut entry = entry.map_err(|e| format!("failed to read tar entry: {e}"))?;
        let path = entry
            .path()
            .map_err(|e| format!("bad tar entry path: {e}"))?;
        let file_name = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n.to_string(),
            None => continue,
        };
        if binary_file_names().contains(&file_name.as_str()) {
            let out_path = dir.join(&file_name);
            let mut out = fs::File::create(&out_path)
                .map_err(|e| format!("failed to create {}: {e}", out_path.display()))?;
            std::io::copy(&mut entry, &mut out)
                .map_err(|e| format!("failed to write extracted binary: {e}"))?;
            drop(out);
            set_executable(&out_path)?;
            return Ok(out_path);
        }
    }
    Err("archive did not contain a c0pl4nd / c0pl4nd.exe binary".to_string())
}

/// Extract the single `c0pl4nd` / `c0pl4nd.exe` binary entry from a `.zip`
/// archive's bytes into `dir`, returning the path to the extracted file.
fn extract_binary_zip(archive_bytes: &[u8], dir: &Path) -> Result<PathBuf, String> {
    let reader = std::io::Cursor::new(archive_bytes);
    let mut zip =
        zip::ZipArchive::new(reader).map_err(|e| format!("failed to read zip archive: {e}"))?;
    for i in 0..zip.len() {
        let mut file = zip
            .by_index(i)
            .map_err(|e| format!("failed to read zip entry {i}: {e}"))?;
        let entry_name = match file.enclosed_name() {
            Some(p) => p.to_path_buf(),
            None => continue, // skip path-traversal entries
        };
        let file_name = match entry_name.file_name().and_then(|n| n.to_str()) {
            Some(n) => n.to_string(),
            None => continue,
        };
        if binary_file_names().contains(&file_name.as_str()) {
            let out_path = dir.join(&file_name);
            let mut out = fs::File::create(&out_path)
                .map_err(|e| format!("failed to create {}: {e}", out_path.display()))?;
            std::io::copy(&mut file, &mut out)
                .map_err(|e| format!("failed to write extracted binary: {e}"))?;
            drop(out);
            set_executable(&out_path)?;
            return Ok(out_path);
        }
    }
    Err("archive did not contain a c0pl4nd / c0pl4nd.exe binary".to_string())
}

/// Extract the binary from a `.zip` or `.tar.gz` archive (selected by the
/// asset name), returning the extracted path. Split out from
/// [`download_verify_extract`] so it can be unit-tested directly (no network,
/// no signature) — the production path NEVER reaches here without a passing
/// `verify_artifact`.
fn extract_binary(archive_bytes: &[u8], asset_name: &str, dir: &Path) -> Result<PathBuf, String> {
    if asset_name.ends_with(".zip") {
        extract_binary_zip(archive_bytes, dir)
    } else {
        extract_binary_targz(archive_bytes, dir)
    }
}

/// Download the release archive + `.minisig` + `.sha256`, verify (SHA-256 THEN
/// minisign against the embedded key — fails closed), then extract the single
/// binary from the archive into `staging_dir`, returning the path to the
/// extracted, verified binary.
///
/// `progress` is called as `(downloaded_bytes, total_bytes)` for the big asset
/// so the UI can show a bar. ANY failure (network OR verify) deletes
/// `staging_dir` and returns `Err` — the binary is NEVER returned unverified.
pub fn download_verify_extract(
    info: &ReleaseInfo,
    staging_dir: &Path,
    progress: impl FnMut(u64, u64),
) -> Result<PathBuf, String> {
    match download_verify_extract_inner(info, staging_dir, progress) {
        Ok(p) => Ok(p),
        Err(e) => {
            let _ = fs::remove_dir_all(staging_dir);
            Err(e)
        }
    }
}

fn download_verify_extract_inner(
    info: &ReleaseInfo,
    staging_dir: &Path,
    progress: impl FnMut(u64, u64),
) -> Result<PathBuf, String> {
    fs::create_dir_all(staging_dir).map_err(|e| format!("failed to create staging dir: {e}"))?;

    // Big asset (streamed for progress) + the two tiny sidecars.
    let asset_bytes = download_asset(&info.asset_url, progress)?;
    let sig_bytes = download_small(&info.sig_url)?;
    let sha_text = download_small(&info.sha_url)?;

    // The .sha256 sidecar is text — either a bare hex digest or the
    // `<hex>  <filename>` `sha256sum` form. Take the first whitespace token.
    let sha_str = String::from_utf8(sha_text)
        .map_err(|e| format!("sha256 sidecar is not valid UTF-8: {e}"))?;
    let expected_sha = sha_str
        .split_whitespace()
        .next()
        .ok_or_else(|| "sha256 sidecar was empty".to_string())?;

    let sig_str =
        String::from_utf8(sig_bytes).map_err(|e| format!("minisig is not valid UTF-8: {e}"))?;

    // SHA-256 THEN minisign against the embedded public key. Fails closed.
    verify_artifact(&asset_bytes, expected_sha, &sig_str, EMBEDDED_PUBLIC_KEY)?;

    // Only reached when verification passed.
    extract_binary(&asset_bytes, &info.asset_name, staging_dir)
}

#[cfg(test)]
mod tests {
    use super::super::verify::sha256_hex;
    use super::*;
    use std::io::Write;

    fn asset(name: &str, url: &str) -> RawAsset {
        RawAsset {
            name: name.to_string(),
            browser_download_url: url.to_string(),
        }
    }

    /// A release fixture for `<target>` with a full asset triple at `tag`,
    /// matching C0PL4ND's `c0pl4nd-<tag>-<target>.<ext>` naming.
    fn release_with_triple(tag: &str, target: &str, ext: &str) -> RawRelease {
        let base = format!("c0pl4nd-{tag}-{target}{ext}");
        RawRelease {
            tag_name: tag.to_string(),
            prerelease: false,
            draft: false,
            html_url: "https://github.com/o/r/releases/tag/x".to_string(),
            assets: vec![
                asset(&base, &format!("https://dl/{base}")),
                asset(
                    &format!("{base}.minisig"),
                    &format!("https://dl/{base}.minisig"),
                ),
                asset(
                    &format!("{base}.sha256"),
                    &format!("https://dl/{base}.sha256"),
                ),
            ],
        }
    }

    #[test]
    fn select_update_returns_some_on_newer_with_matching_triple() {
        let target = "x86_64-unknown-linux-gnu";
        let raw = release_with_triple("v0.4.0", target, ".tar.gz");
        let current = semver::Version::parse("0.3.2").unwrap();
        let info = select_update(&raw, &current, target, ".tar.gz").expect("expected an update");
        assert_eq!(info.version, semver::Version::parse("0.4.0").unwrap());
        assert_eq!(info.tag, "v0.4.0");
        assert!(info.asset_name.contains(target));
        assert!(info.asset_name.ends_with(".tar.gz"));
        assert!(info.sig_url.ends_with(".minisig"));
        assert!(info.sha_url.ends_with(".sha256"));
    }

    #[test]
    fn select_update_matches_zip_on_windows_target() {
        let target = "x86_64-pc-windows-msvc";
        let raw = release_with_triple("v1.0.0", target, ".zip");
        let current = semver::Version::parse("0.9.0").unwrap();
        let info = select_update(&raw, &current, target, ".zip").expect("zip asset matched");
        assert!(info.asset_name.ends_with(".zip"));
    }

    #[test]
    fn select_update_none_when_not_newer() {
        let target = "x86_64-unknown-linux-gnu";
        let raw = release_with_triple("v0.3.0", target, ".tar.gz");
        let current = semver::Version::parse("0.3.0").unwrap();
        assert!(select_update(&raw, &current, target, ".tar.gz").is_none());
        // An older release is also not an update.
        let older = release_with_triple("v0.2.0", target, ".tar.gz");
        assert!(select_update(&older, &current, target, ".tar.gz").is_none());
    }

    #[test]
    fn select_update_none_for_prerelease_or_draft() {
        let target = "x86_64-unknown-linux-gnu";
        let current = semver::Version::parse("0.3.0").unwrap();
        let mut pre = release_with_triple("v0.4.0", target, ".tar.gz");
        pre.prerelease = true;
        assert!(select_update(&pre, &current, target, ".tar.gz").is_none());
        let mut draft = release_with_triple("v0.4.0", target, ".tar.gz");
        draft.draft = true;
        assert!(select_update(&draft, &current, target, ".tar.gz").is_none());
    }

    #[test]
    fn select_update_none_when_sidecars_missing() {
        let target = "x86_64-unknown-linux-gnu";
        let current = semver::Version::parse("0.3.0").unwrap();
        // Archive present, but the `.minisig` sibling is absent → cannot verify.
        let base = format!("c0pl4nd-v0.4.0-{target}.tar.gz");
        let raw = RawRelease {
            tag_name: "v0.4.0".to_string(),
            prerelease: false,
            draft: false,
            html_url: String::new(),
            assets: vec![
                asset(&base, &format!("https://dl/{base}")),
                asset(
                    &format!("{base}.sha256"),
                    &format!("https://dl/{base}.sha256"),
                ),
            ],
        };
        assert!(select_update(&raw, &current, target, ".tar.gz").is_none());
    }

    #[test]
    fn select_update_none_when_no_matching_target() {
        let raw = release_with_triple("v0.4.0", "x86_64-pc-windows-msvc", ".zip");
        let current = semver::Version::parse("0.3.0").unwrap();
        // We are a linux build → the windows asset must not match.
        assert!(select_update(&raw, &current, "aarch64-apple-darwin", ".tar.gz").is_none());
    }

    #[test]
    fn select_update_empty_target_never_matches() {
        let raw = release_with_triple("v0.4.0", "x86_64-unknown-linux-gnu", ".tar.gz");
        let current = semver::Version::parse("0.3.0").unwrap();
        assert!(select_update(&raw, &current, "", ".tar.gz").is_none());
    }

    #[test]
    fn extract_binary_targz_pulls_the_binary_and_chmods() {
        let dir = tempfile::tempdir().unwrap();
        let payload = b"#!/bin/sh\necho c0pl4nd\n";
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        {
            let mut builder = tar::Builder::new(&mut gz);
            // A decoy + the real binary.
            for (name, data) in [("README.txt", &b"x"[..]), ("c0pl4nd", &payload[..])] {
                let mut header = tar::Header::new_gnu();
                header.set_size(data.len() as u64);
                header.set_mode(0o644);
                header.set_cksum();
                builder.append_data(&mut header, name, data).unwrap();
            }
            builder.finish().unwrap();
        }
        let archive_bytes = gz.finish().unwrap();
        let extracted = extract_binary(&archive_bytes, "c0pl4nd-x.tar.gz", dir.path()).unwrap();
        assert_eq!(fs::read(&extracted).unwrap(), payload);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&extracted).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o755);
        }
    }

    #[test]
    fn extract_binary_zip_pulls_the_binary() {
        let dir = tempfile::tempdir().unwrap();
        let payload = b"MZ\x90\x00 fake c0pl4nd.exe";
        let mut buf = Vec::new();
        {
            let mut zw = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
            let opts: zip::write::FileOptions<()> = zip::write::FileOptions::default();
            zw.start_file("README.md", opts).unwrap();
            zw.write_all(b"docs").unwrap();
            zw.start_file("c0pl4nd.exe", opts).unwrap();
            zw.write_all(payload).unwrap();
            zw.finish().unwrap();
        }
        let extracted = extract_binary(&buf, "c0pl4nd-x.zip", dir.path()).unwrap();
        assert_eq!(fs::read(&extracted).unwrap(), payload);
    }

    #[test]
    fn extract_binary_errs_when_no_binary_entry() {
        let dir = tempfile::tempdir().unwrap();
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        {
            let mut builder = tar::Builder::new(&mut gz);
            let data = b"readme";
            let mut header = tar::Header::new_gnu();
            header.set_size(data.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder
                .append_data(&mut header, "README.txt", &data[..])
                .unwrap();
            builder.finish().unwrap();
        }
        let archive_bytes = gz.finish().unwrap();
        assert!(extract_binary(&archive_bytes, "c0pl4nd-x.tar.gz", dir.path()).is_err());
    }

    #[test]
    fn sha_sidecar_first_token_matches_archive_digest() {
        let archive = b"pretend tarball bytes";
        let digest = sha256_hex(archive);
        let sidecar = format!("{digest}  c0pl4nd-x.tar.gz\n");
        let first = sidecar.split_whitespace().next().unwrap();
        assert_eq!(first, digest);
    }

    #[test]
    fn archive_ext_matches_platform() {
        if cfg!(windows) {
            assert_eq!(archive_ext(), ".zip");
        } else {
            assert_eq!(archive_ext(), ".tar.gz");
        }
    }
}
