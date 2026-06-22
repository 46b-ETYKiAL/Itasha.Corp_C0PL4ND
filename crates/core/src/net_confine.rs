//! Pure URL-confinement primitives shared by the app's two updater paths.
//!
//! C0PL4ND's dependency-light CLI/launch update check (`app::update`) and its
//! in-app verified updater (`app::update_engine::net`) both confine every
//! request — and every redirect hop — to `https://` and an allow-listed GitHub
//! host, so a MITM'd or hostile redirect cannot point the request (and our
//! `User-Agent`) at an arbitrary server. The HOST allow-list legitimately
//! differs between the two (the CLI check only hits the REST API; the in-app
//! updater also follows asset downloads onto the GitHub CDN), so it stays a
//! per-caller predicate. The PURE URL plumbing — scheme check, host extraction,
//! redirect resolution — is identical, and lives HERE (in the core crate, which
//! both updater paths and every `#[path]`-duplicated integration test already
//! depend on) so the two callers cannot drift apart. These functions perform NO
//! network I/O — they are string parsing only (the `no-network-gate` forbids
//! actual `ureq`/socket call sites, not these helpers) — and are exhaustively
//! unit-tested.

/// True iff `url` uses the `https` scheme (case-insensitive per RFC 3986).
pub fn is_https(url: &str) -> bool {
    url.split_once("://")
        .map(|(scheme, _)| scheme.eq_ignore_ascii_case("https"))
        .unwrap_or(false)
}

/// The lowercased host of an `https://host[:port]/...` URL, stripping any
/// userinfo and port. `None` if the URL has no authority (so a caller treats a
/// malformed URL as not-allowed rather than guessing).
pub fn url_host(url: &str) -> Option<String> {
    let after = url.split_once("://")?.1;
    let authority = after.split(['/', '?', '#']).next()?;
    let host_port = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    let host = host_port.split(':').next()?;
    if host.is_empty() {
        return None;
    }
    Some(host.to_ascii_lowercase())
}

/// Resolve a redirect `Location` against the current URL. Absolute targets pass
/// through (their host is re-validated by the caller); origin-relative (`/path`)
/// targets keep the current scheme+host; anything else is refused. The caller
/// maps the `&'static str` error into its own error type.
pub fn resolve_redirect(base: &str, loc: &str) -> Result<String, &'static str> {
    if loc.contains("://") {
        Ok(loc.to_string())
    } else if let Some(rest) = loc.strip_prefix('/') {
        let (scheme, after) = base.split_once("://").ok_or("malformed base URL")?;
        let host = after.split(['/', '?', '#']).next().unwrap_or(after);
        Ok(format!("{scheme}://{host}/{rest}"))
    } else {
        Err("unsupported relative redirect target")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_https_is_scheme_only_and_case_insensitive() {
        assert!(is_https("https://api.github.com/x"));
        assert!(is_https("HTTPS://api.github.com/x"));
        assert!(!is_https("http://api.github.com/x"));
        assert!(!is_https("ftp://x/y"));
        assert!(!is_https("no-scheme"));
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
        // Userinfo cannot smuggle a foreign host past the host check.
        assert_eq!(
            url_host("https://api.github.com@evil.example.com/x").as_deref(),
            Some("evil.example.com")
        );
        assert_eq!(
            url_host("https://CODELOAD.github.com/x").as_deref(),
            Some("codeload.github.com")
        );
        assert_eq!(url_host("https:///x"), None);
        assert_eq!(url_host("no-authority"), None);
    }

    #[test]
    fn resolve_redirect_keeps_origin_for_relative_and_passes_absolute() {
        assert_eq!(
            resolve_redirect("https://api.github.com/a", "/b/c").unwrap(),
            "https://api.github.com/b/c"
        );
        assert_eq!(
            resolve_redirect("https://api.github.com/a", "https://codeload.github.com/d").unwrap(),
            "https://codeload.github.com/d"
        );
        // A scheme-relative / weird relative target is refused.
        assert!(resolve_redirect("https://api.github.com/a", "b/c").is_err());
        assert!(resolve_redirect("no-scheme", "/b").is_err());
    }

    /// The exact error MESSAGES are part of the caller contract (the caller maps
    /// the `&'static str` into its own error). Pin them so a reword is caught.
    #[test]
    fn resolve_redirect_error_messages_are_stable() {
        // Origin-relative target against a base with no scheme → "malformed base URL".
        assert_eq!(
            resolve_redirect("no-scheme-base", "/path"),
            Err("malformed base URL")
        );
        // A non-absolute, non-origin-relative target → "unsupported relative …".
        assert_eq!(
            resolve_redirect("https://api.github.com/a", "relative/path"),
            Err("unsupported relative redirect target")
        );
    }

    /// `resolve_redirect` with an origin-relative target preserves the base's
    /// scheme+host even when the base carries a port and a query — the host is
    /// taken from the authority up to the first `/?#`.
    #[test]
    fn resolve_redirect_origin_relative_keeps_scheme_and_authority() {
        assert_eq!(
            resolve_redirect("https://codeload.github.com:443/owner/repo?x=1", "/d/e").unwrap(),
            "https://codeload.github.com:443/d/e",
            "origin-relative redirect keeps the base scheme + authority (incl. port)"
        );
    }

    /// `url_host` returns `None` for a present-but-EMPTY host (`https://:443/x`)
    /// — the `if host.is_empty()` guard, distinct from the no-authority `None`.
    #[test]
    fn url_host_empty_host_is_none() {
        assert_eq!(url_host("https://:443/x"), None, "empty host → None");
        assert_eq!(
            url_host("https://@/x"),
            None,
            "userinfo then empty host → None"
        );
    }

    /// `is_https` on a bare scheme with no authority still classifies by scheme
    /// only (`https:` alone is true; the host check is a separate concern).
    #[test]
    fn is_https_scheme_only_even_without_authority() {
        assert!(is_https("https://"), "scheme-only https is still https");
        assert!(!is_https("HTTPX://x"));
    }
}
