//! In-session history of commands the user has run, for the command-palette
//! quick find/run (#24 — "quick find/run previously-run scripts").
//!
//! Recorded best-effort by the app layer (it accumulates typed input and
//! commits the line on Enter). The list is most-recent-first, de-duplicated
//! (re-running a command moves it to the front rather than appending a
//! duplicate), and bounded so a long session cannot grow it without limit.
//! Search reuses the crate's dependency-free [`crate::fuzzy`] matcher.

use std::collections::VecDeque;
use std::sync::OnceLock;

use regex::Regex;

use crate::fuzzy;

/// Redact obvious inline secrets from a command line before it is stored in
/// history (and shown in the palette + sidebar). Conservative by design — only
/// UNAMBIGUOUS secret-bearing tokens are masked, so ordinary commands are never
/// mangled:
/// - long-form credential flags: `--password=…`, `--token=…`, `--secret=…`,
///   `--api-key=…`, `--access-key=…`, `--private-key=…`, `--auth-token=…`;
/// - environment-style assignments whose NAME names a secret:
///   `API_KEY=…`, `DB_PASSWORD=…`, `GH_TOKEN=…`, etc.
///
/// The value is replaced with `<redacted>` while the command shape is preserved
/// so the entry stays useful for recall. Interactive password PROMPTS (`sudo`,
/// `ssh`, `mysql -p`) are handled separately upstream — those keystrokes are not
/// echoed by the tty, and the app drops non-echoed lines before they ever reach
/// `record`. The ambiguous short `-p<value>` flag is deliberately NOT matched
/// (it collides with non-secret uses like `cp -p`); precision over recall.
pub fn redact_secrets(line: &str) -> String {
    static FLAG: OnceLock<Regex> = OnceLock::new();
    static ENV: OnceLock<Regex> = OnceLock::new();
    let flag = FLAG.get_or_init(|| {
        Regex::new(
            r"(?i)(--(?:password|passwd|secret|token|api[-_]?key|access[-_]?key|private[-_]?key|auth[-_]?token)=)\S+",
        )
        .expect("static secret-flag regex is valid")
    });
    let env = ENV.get_or_init(|| {
        Regex::new(
            r"(?i)\b([A-Z0-9_]*(?:PASSWORD|PASSWD|SECRET|TOKEN|API[-_]?KEY|ACCESS[-_]?KEY|PRIVATE[-_]?KEY|AUTH[-_]?TOKEN)[A-Z0-9_]*=)\S+",
        )
        .expect("static secret-env regex is valid")
    });
    let out = flag.replace_all(line, "${1}<redacted>");
    env.replace_all(&out, "${1}<redacted>").into_owned()
}

/// Default maximum number of distinct commands kept.
pub const DEFAULT_CAP: usize = 200;

/// A bounded, most-recent-first, de-duplicated list of run commands.
#[derive(Debug, Clone)]
pub struct CommandHistory {
    entries: VecDeque<String>,
    cap: usize,
}

impl Default for CommandHistory {
    fn default() -> Self {
        Self::with_capacity(DEFAULT_CAP)
    }
}

impl CommandHistory {
    /// A history bounded to at most `cap` entries (clamped to ≥ 1).
    pub fn with_capacity(cap: usize) -> Self {
        Self {
            entries: VecDeque::new(),
            cap: cap.max(1),
        }
    }

    /// Record `command` as the most recent. Whitespace-only input is ignored.
    /// An existing identical entry is moved to the front (no duplicate). Trims
    /// to the capacity, dropping the oldest entries.
    pub fn record(&mut self, command: impl Into<String>) {
        let command = command.into();
        let trimmed = command.trim();
        if trimmed.is_empty() {
            return;
        }
        // Redact inline secrets before the command is stored / shown in the
        // palette + sidebar. Interactive password prompts are excluded upstream
        // (the app drops non-echoed lines); this catches secrets typed ON the
        // command line (`mysql --password=…`, `export API_KEY=…`).
        let value = redact_secrets(trimmed);
        if let Some(pos) = self.entries.iter().position(|e| e == &value) {
            self.entries.remove(pos);
        }
        self.entries.push_front(value);
        while self.entries.len() > self.cap {
            self.entries.pop_back();
        }
    }

    /// All entries, most-recent-first.
    pub fn entries(&self) -> impl Iterator<Item = &str> {
        self.entries.iter().map(String::as_str)
    }

    /// Number of stored commands.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether no commands have been recorded yet.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Fuzzy-search the history. An empty query returns every entry in
    /// most-recent-first order. Results are OWNED strings so a caller can hold
    /// them across a mutable borrow of the surrounding UI state (the egui
    /// command palette computes this before borrowing `self` for its widgets).
    pub fn search(&self, query: &str) -> Vec<String> {
        let items: Vec<&str> = self.entries.iter().map(String::as_str).collect();
        fuzzy::filter_sorted(&items, query)
            .into_iter()
            .map(str::to_string)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_most_recent_first() {
        let mut h = CommandHistory::default();
        h.record("ls");
        h.record("cargo build");
        let got: Vec<&str> = h.entries().collect();
        assert_eq!(got, vec!["cargo build", "ls"], "newest entry is first");
    }

    #[test]
    fn ignores_whitespace_only() {
        let mut h = CommandHistory::default();
        h.record("   ");
        h.record("\t\n");
        assert!(h.is_empty(), "blank lines must not be recorded");
    }

    #[test]
    fn trims_surrounding_whitespace() {
        let mut h = CommandHistory::default();
        h.record("  git status  ");
        assert_eq!(h.entries().next(), Some("git status"));
    }

    #[test]
    fn rerunning_moves_to_front_without_duplicating() {
        let mut h = CommandHistory::default();
        h.record("a");
        h.record("b");
        h.record("a"); // re-run the older command
        let got: Vec<&str> = h.entries().collect();
        assert_eq!(got, vec!["a", "b"], "re-run moves to front, no duplicate");
        assert_eq!(h.len(), 2);
    }

    #[test]
    fn drops_oldest_past_capacity() {
        let mut h = CommandHistory::with_capacity(2);
        h.record("one");
        h.record("two");
        h.record("three");
        let got: Vec<&str> = h.entries().collect();
        assert_eq!(got, vec!["three", "two"], "oldest ('one') dropped at cap 2");
    }

    #[test]
    fn search_empty_query_returns_all_recent_first() {
        let mut h = CommandHistory::default();
        h.record("alpha");
        h.record("beta");
        assert_eq!(h.search(""), vec!["beta".to_string(), "alpha".to_string()]);
    }

    #[test]
    fn search_fuzzy_filters_matches() {
        let mut h = CommandHistory::default();
        h.record("cargo build");
        h.record("cargo test");
        h.record("ls -la");
        let hits = h.search("crgts"); // subsequence of "cargo test"
        assert_eq!(hits, vec!["cargo test".to_string()]);
    }

    #[test]
    fn redact_secrets_masks_credential_flags() {
        assert_eq!(
            redact_secrets("mysql --password=hunter2 -h db"),
            "mysql --password=<redacted> -h db"
        );
        assert_eq!(
            redact_secrets("curl --token=ghp_abc123 https://x"),
            "curl --token=<redacted> https://x"
        );
        assert_eq!(
            redact_secrets("foo --api-key=AKIA1234"),
            "foo --api-key=<redacted>"
        );
    }

    #[test]
    fn redact_secrets_masks_secret_env_assignments() {
        assert_eq!(
            redact_secrets("export API_KEY=sk-secret-value"),
            "export API_KEY=<redacted>"
        );
        assert_eq!(
            redact_secrets("DB_PASSWORD=p@ss ./run"),
            "DB_PASSWORD=<redacted> ./run"
        );
        assert_eq!(
            redact_secrets("GH_TOKEN=ghp_xyz gh pr list"),
            "GH_TOKEN=<redacted> gh pr list"
        );
    }

    #[test]
    fn redact_secrets_leaves_ordinary_commands_untouched() {
        // No secret indicators → identical. Precision over recall: the short
        // `-p` flag and non-secret `=` assignments must NOT be mangled.
        for cmd in [
            "git commit -m 'fix build'",
            "cp -p src dst",
            "make CC=gcc TARGET=release",
            "cargo build --release",
            "PATH=/usr/bin:$PATH ls",
        ] {
            assert_eq!(redact_secrets(cmd), cmd, "must not mangle: {cmd}");
        }
    }

    #[test]
    fn record_applies_redaction() {
        let mut h = CommandHistory::default();
        h.record("psql --password=topsecret");
        assert_eq!(h.entries().next(), Some("psql --password=<redacted>"));
    }
}
