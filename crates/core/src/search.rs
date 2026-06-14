//! In-terminal search over the scrollback buffer.
//!
//! Searches the plain-text lines produced by [`crate::Terminal::all_lines`].
//! Supports literal and regex queries, case-insensitive by default; returns
//! every match as a `(line, start, end)` span so the UI can jump to and
//! highlight it.

use regex::Regex;

/// A single match: line index into the searched slice, and the byte range.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SearchMatch {
    pub line: usize,
    pub start: usize,
    pub end: usize,
}

/// Search options.
#[derive(Debug, Clone, Copy)]
pub struct SearchOptions {
    pub regex: bool,
    pub case_insensitive: bool,
}

impl Default for SearchOptions {
    fn default() -> Self {
        SearchOptions {
            regex: false,
            case_insensitive: true,
        }
    }
}

/// Find every match of `query` across `lines`. Empty/invalid queries yield none.
///
/// Match offsets are byte ranges into the ORIGINAL line (what `all_lines`
/// produced and what the UI's `byte_to_col` maps against). Both literal and
/// regex queries run through the regex engine against the unmodified line, and
/// case-insensitivity is the `(?i)` flag — NOT lowercasing the haystack. The
/// previous literal case-insensitive path searched `line.to_lowercase()` and
/// returned offsets into THAT string; `to_lowercase()` is not byte-length
/// preserving (e.g. `İ` U+0130 → `i̇` 2→3 bytes, Kelvin `K` U+212A → `k` 3→1),
/// so any such char before a match shifted the highlight/jump to the wrong
/// column. Routing the literal path through `regex::escape` keeps the query's
/// characters verbatim while giving source-relative offsets.
pub fn find(lines: &[String], query: &str, opts: SearchOptions) -> Vec<SearchMatch> {
    if query.is_empty() {
        return Vec::new();
    }
    let Ok(re) = Regex::new(&build_pattern(query, opts)) else {
        return Vec::new(); // invalid regex → no matches (UI shows 0)
    };
    let mut out = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        for m in re.find_iter(line) {
            out.push(SearchMatch {
                line: i,
                start: m.start(),
                end: m.end(),
            });
        }
    }
    out
}

/// Build the effective regex pattern: a literal query is `regex::escape`d so its
/// metacharacters match verbatim; a regex query is used as-is. Case-insensitive
/// search applies the `(?i)` flag rather than lowercasing the haystack, so match
/// offsets stay relative to the original (un-lowercased) line.
fn build_pattern(query: &str, opts: SearchOptions) -> String {
    let body = if opts.regex {
        query.to_string()
    } else {
        regex::escape(query)
    };
    if opts.case_insensitive {
        format!("(?i){body}")
    } else {
        body
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines() -> Vec<String> {
        vec![
            "error: file not found".to_string(),
            "ok".to_string(),
            "another ERROR here".to_string(),
        ]
    }

    #[test]
    fn empty_query_yields_nothing() {
        assert!(find(&lines(), "", SearchOptions::default()).is_empty());
    }

    #[test]
    fn literal_case_insensitive_finds_all() {
        let m = find(&lines(), "error", SearchOptions::default());
        assert_eq!(m.len(), 2);
        assert_eq!(m[0].line, 0);
        assert_eq!(m[1].line, 2);
    }

    #[test]
    fn case_sensitive_respects_case() {
        let m = find(
            &lines(),
            "ERROR",
            SearchOptions {
                regex: false,
                case_insensitive: false,
            },
        );
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].line, 2);
    }

    #[test]
    fn regex_matches() {
        let m = find(
            &lines(),
            r"^error",
            SearchOptions {
                regex: true,
                case_insensitive: true,
            },
        );
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].line, 0);
    }

    #[test]
    fn invalid_regex_yields_nothing() {
        let m = find(
            &lines(),
            "(unclosed",
            SearchOptions {
                regex: true,
                case_insensitive: true,
            },
        );
        assert!(m.is_empty());
    }

    #[test]
    fn multiple_matches_per_line() {
        let l = vec!["aXaXa".to_string()];
        let m = find(
            &l,
            "a",
            SearchOptions {
                regex: false,
                case_insensitive: false,
            },
        );
        assert_eq!(m.len(), 3);
    }

    #[test]
    fn case_insensitive_offsets_are_source_relative() {
        // Regression: `to_lowercase()` is not byte-length-preserving. 'İ'
        // (U+0130, 2 bytes) lowercases to "i̇" (3 bytes), so the old path —
        // which searched `line.to_lowercase()` — returned a match offset shifted
        // by +1 byte; the UI's `byte_to_col` then mapped it to the wrong column.
        // Offsets must be byte ranges into the ORIGINAL line.
        let l = vec!["İx".to_string()]; // 'İ' = 2 bytes; 'x' at byte offset 2
        let m = find(&l, "x", SearchOptions::default()); // case-insensitive default
        assert_eq!(m.len(), 1);
        assert_eq!(
            m[0].start, 2,
            "offset must index the original line, not its lowercase form"
        );
        assert_eq!(&l[0][m[0].start..m[0].end], "x");
    }

    #[test]
    fn literal_query_metacharacters_match_verbatim() {
        // A literal (non-regex) query must match its characters verbatim — the
        // escape path keeps `.` from matching any char.
        let l = vec!["a.b".to_string(), "axb".to_string()];
        let m = find(
            &l,
            "a.b",
            SearchOptions {
                regex: false,
                case_insensitive: false,
            },
        );
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].line, 0);
    }
}
