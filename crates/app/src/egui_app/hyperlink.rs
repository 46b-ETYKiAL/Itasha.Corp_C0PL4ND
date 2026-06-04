//! URL detection for ctrl-click-to-open in the terminal grid.
//!
//! Terminal output routinely prints bare `http(s)://…` URLs (build logs, error
//! messages, `gh`/`cargo` output). This module finds those spans in a line of
//! grid text so the renderer can underline them on Ctrl-hover and open the one
//! under the cursor on Ctrl-click. It is PURE (no egui, no I/O) so the matching
//! and trailing-punctuation rules are unit-testable without a live terminal.
//!
//! Scope: only the unambiguous `http://` and `https://` schemes are matched —
//! they are safe to hand to the OS opener verbatim. Scheme-less `www.` text and
//! OSC 8 explicit hyperlinks are deliberately out of scope (a `www.` host needs
//! a guessed scheme; OSC 8 is a separate terminal mechanism that would require
//! per-cell hyperlink ids in the core grid).

/// One detected URL in a line: the byte range `[start, end)` of the (trimmed)
/// URL within the source line, plus the URL text itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UrlSpan {
    /// Byte offset of the first character of the URL within the line.
    pub start: usize,
    /// Byte offset one past the last character of the (trimmed) URL.
    pub end: usize,
    /// The URL text (`line[start..end]`), safe to pass to the OS opener.
    pub url: String,
}

/// Characters permitted INSIDE a URL while scanning. A superset of RFC 3986's
/// unreserved + reserved set, minus the delimiters a terminal line would use
/// around a URL (whitespace, quotes, angle brackets, backtick, pipe, caret,
/// braces, backslash). Sentence punctuation that is also a legal URL byte
/// (`.,;:!?` and closing brackets) is allowed here so it does not truncate a
/// URL mid-way, then stripped from the TRAILING end by [`trim_url`].
fn is_url_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || "-._~:/?#[]@!$&'()*+,;=%".contains(ch)
}

/// The earliest byte offset in `s` at which an `http://` or `https://` scheme
/// begins, or `None`. (`https://` is not a substring of `http://`, so the two
/// searches never alias.)
fn next_scheme(s: &str) -> Option<usize> {
    match (s.find("http://"), s.find("https://")) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    }
}

/// Strip trailing characters that are almost always sentence punctuation rather
/// than part of the URL: `.,;:!?` and quotes unconditionally, and a closing
/// bracket ONLY when it is unbalanced within the candidate (so a Wikipedia-style
/// `…_(disambiguation)` URL keeps its matched `)`, but `(see http://x)` drops
/// the trailing one). Returns the kept byte length.
fn trim_url(s: &str) -> usize {
    let mut end = s.len();
    loop {
        let cur = &s[..end];
        let Some(last) = cur.chars().last() else {
            break;
        };
        let drop = match last {
            '.' | ',' | ';' | ':' | '!' | '?' | '\'' | '"' => true,
            ')' => cur.matches('(').count() < cur.matches(')').count(),
            ']' => cur.matches('[').count() < cur.matches(']').count(),
            '}' => cur.matches('{').count() < cur.matches('}').count(),
            _ => false,
        };
        if drop {
            end -= last.len_utf8();
        } else {
            break;
        }
    }
    end
}

/// Find every `http(s)://` URL in `line`, in left-to-right order, as byte spans.
/// A bare scheme with no host (`http://` followed immediately by a delimiter) is
/// not reported. Multi-byte glyphs before a URL are handled correctly because
/// all offsets are byte offsets into the original line.
pub fn find_urls(line: &str) -> Vec<UrlSpan> {
    let mut out = Vec::new();
    let mut base = 0usize;
    while base < line.len() {
        let Some(rel) = next_scheme(&line[base..]) else {
            break;
        };
        let url_start = base + rel;
        // Extend over allowed URL characters.
        let tail = &line[url_start..];
        let mut raw_end = 0usize;
        for (ci, ch) in tail.char_indices() {
            if is_url_char(ch) {
                raw_end = ci + ch.len_utf8();
            } else {
                break;
            }
        }
        let candidate = &line[url_start..url_start + raw_end];
        let kept = trim_url(candidate);
        let url = &candidate[..kept];
        // Require a host after the scheme — reject a bare "http://".
        let has_host = url
            .strip_prefix("https://")
            .or_else(|| url.strip_prefix("http://"))
            .is_some_and(|host| !host.is_empty());
        if has_host {
            out.push(UrlSpan {
                start: url_start,
                end: url_start + kept,
                url: url.to_string(),
            });
        }
        // Continue scanning AFTER the raw (untrimmed) run so the next iteration
        // cannot re-match inside the same token.
        base = (url_start + raw_end).max(url_start + 1);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn urls(line: &str) -> Vec<String> {
        find_urls(line).into_iter().map(|s| s.url).collect()
    }

    #[test]
    fn finds_a_plain_https_url() {
        assert_eq!(urls("see https://example.com now"), ["https://example.com"]);
    }

    #[test]
    fn finds_http_and_https_and_paths_queries_fragments() {
        assert_eq!(
            urls("a http://a.io/x?y=1#f b https://b.dev/p/q"),
            ["http://a.io/x?y=1#f", "https://b.dev/p/q"]
        );
    }

    #[test]
    fn no_url_returns_empty() {
        assert!(find_urls("just some plain text, no link").is_empty());
        // A scheme-shaped word that is not really a URL host is rejected.
        assert!(find_urls("http://").is_empty());
    }

    #[test]
    fn trailing_sentence_punctuation_is_trimmed() {
        assert_eq!(urls("visit https://example.com."), ["https://example.com"]);
        assert_eq!(
            urls("(see https://example.com), ok"),
            ["https://example.com"]
        );
        assert_eq!(urls("link: https://x.io/a!"), ["https://x.io/a"]);
    }

    #[test]
    fn balanced_parens_inside_url_are_kept() {
        // A matched '(' … ')' belongs to the URL (Wikipedia disambiguation form).
        assert_eq!(
            urls("https://en.wikipedia.org/wiki/Rust_(programming_language)"),
            ["https://en.wikipedia.org/wiki/Rust_(programming_language)"]
        );
    }

    #[test]
    fn byte_spans_are_correct_with_a_multibyte_prefix() {
        // A multi-byte glyph (→, 3 bytes) before the URL must not corrupt offsets.
        let line = "→ https://x.io";
        let spans = find_urls(line);
        assert_eq!(spans.len(), 1);
        let s = &spans[0];
        assert_eq!(&line[s.start..s.end], "https://x.io");
        assert_eq!(s.url, "https://x.io");
    }

    #[test]
    fn two_urls_separated_by_whitespace() {
        let spans = find_urls("https://a.io https://b.io");
        assert_eq!(spans.len(), 2);
        assert_eq!(spans[0].start, 0);
        assert!(spans[1].start > spans[0].end);
    }

    #[test]
    fn quotes_and_angle_brackets_delimit_the_url() {
        assert_eq!(urls("\"https://x.io/a\""), ["https://x.io/a"]);
        assert_eq!(urls("<https://x.io/a>"), ["https://x.io/a"]);
    }
}
