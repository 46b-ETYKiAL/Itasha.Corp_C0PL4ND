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
pub fn find(lines: &[String], query: &str, opts: SearchOptions) -> Vec<SearchMatch> {
    if query.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    if opts.regex {
        let pat = if opts.case_insensitive {
            format!("(?i){query}")
        } else {
            query.to_string()
        };
        let Ok(re) = Regex::new(&pat) else {
            return Vec::new(); // invalid regex → no matches (UI shows 0)
        };
        for (i, line) in lines.iter().enumerate() {
            for m in re.find_iter(line) {
                out.push(SearchMatch {
                    line: i,
                    start: m.start(),
                    end: m.end(),
                });
            }
        }
    } else {
        for (i, line) in lines.iter().enumerate() {
            let (hay, needle) = if opts.case_insensitive {
                (line.to_lowercase(), query.to_lowercase())
            } else {
                (line.clone(), query.to_string())
            };
            let mut from = 0;
            while let Some(pos) = hay[from..].find(&needle) {
                let s = from + pos;
                out.push(SearchMatch {
                    line: i,
                    start: s,
                    end: s + needle.len(),
                });
                from = s + needle.len().max(1);
            }
        }
    }
    out
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
}
