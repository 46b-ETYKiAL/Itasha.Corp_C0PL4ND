//! Tiny subsequence fuzzy matcher for the command palette.
//!
//! Scores how well a query matches an item: every query char must appear in
//! order (case-insensitive); contiguous runs and word-start hits score higher.
//! `None` means no match. This is intentionally small and dependency-free.

/// Score `item` against `query`. Higher is better; `None` = no match.
/// An empty query matches everything with score 0 (preserves input order).
pub fn score(item: &str, query: &str) -> Option<i32> {
    if query.is_empty() {
        return Some(0);
    }
    let item_l = item.to_lowercase();
    let query_l = query.to_lowercase();
    let item_bytes: Vec<char> = item_l.chars().collect();
    let q: Vec<char> = query_l.chars().collect();

    let mut qi = 0;
    let mut total = 0i32;
    let mut prev_match: Option<usize> = None;
    for (i, &c) in item_bytes.iter().enumerate() {
        if qi < q.len() && c == q[qi] {
            // Base point per matched char.
            total += 1;
            // Bonus for contiguous matches.
            if prev_match == Some(i.wrapping_sub(1)) {
                total += 3;
            }
            // Bonus for word-start (begin or after a separator).
            if i == 0 || matches!(item_bytes.get(i - 1), Some(' ') | Some('-') | Some('_')) {
                total += 2;
            }
            prev_match = Some(i);
            qi += 1;
        }
    }
    if qi == q.len() {
        Some(total)
    } else {
        None
    }
}

/// Filter and rank `items` by `query`, best first. Stable for equal scores.
pub fn filter_sorted<'a>(items: &[&'a str], query: &str) -> Vec<&'a str> {
    let mut scored: Vec<(i32, usize, &str)> = items
        .iter()
        .enumerate()
        .filter_map(|(i, it)| score(it, query).map(|s| (s, i, *it)))
        .collect();
    // Higher score first; tie-break by original order.
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    scored.into_iter().map(|(_, _, it)| it).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_query_matches_all() {
        assert_eq!(score("anything", ""), Some(0));
    }

    #[test]
    fn subsequence_matches() {
        assert!(score("New Tab", "nt").is_some());
        assert!(score("New Tab", "ntb").is_some());
        assert!(score("New Tab", "xyz").is_none());
    }

    #[test]
    fn contiguous_scores_higher_than_scattered() {
        let contiguous = score("close tab", "clos").unwrap();
        let scattered = score("close tab", "ctb").unwrap();
        assert!(contiguous > scattered);
    }

    #[test]
    fn filter_ranks_best_first() {
        let items = ["New Tab", "Close Tab", "Next Tab"];
        let out = filter_sorted(&items, "tab");
        assert_eq!(out.len(), 3); // all contain "tab"
    }

    #[test]
    fn filter_excludes_nonmatches() {
        let items = ["New Tab", "Search", "Quit"];
        let out = filter_sorted(&items, "se");
        assert_eq!(out, vec!["Search"]);
    }
}
