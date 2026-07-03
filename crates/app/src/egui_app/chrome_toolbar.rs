//! The customizable top-bar quick-action toolbar: the action CATALOG plus the
//! pure list operations (reorder / add / remove) that back the Settings → Toolbar
//! editor. Kept UI-free and side-effect-free so the reorder/add/remove logic is
//! unit-testable without a live `egui` context (mirroring SCR1B3's
//! `apply_toolbar_drop`); the actual rendering + the settings widgets live in
//! `chrome.rs` / `settings.rs` and drive these functions.
//!
//! An action `id` is a stable, config-persisted string (see
//! [`c0pl4nd_core::config::ToolbarConfig`]); the human label + which chrome
//! affordance it maps to are resolved from [`TOOLBAR_ACTIONS`] here and rendered
//! in `chrome.rs`. An id not in the catalog (a config from a newer/older build) is
//! simply skipped at render time, so the bar never breaks.

/// The catalog of customizable quick actions, as `(id, human_label)`. The `id` is
/// what persists in `config.toolbar.items` / `.menu`; the label is shown in the
/// Settings → Toolbar editor and the overflow "⋯" menu. Order here is the
/// canonical "palette" order (how un-added actions list in the add menu). The
/// fixed affordances (wordmark, tabs, the tab-adjacent "+", and the window caption
/// cluster) are deliberately NOT in this catalog — only the customizable cluster.
pub(crate) const TOOLBAR_ACTIONS: &[(&str, &str)] = &[
    ("view_mode", "Toggle grid / tabs view"),
    ("equalize_panes", "Equalize pane sizes"),
    ("shell_switcher", "Shell switcher"),
    ("script_launcher", "Script launcher"),
];

/// The human label for an action id, or `None` if the id is not in the catalog.
pub(crate) fn action_label(id: &str) -> Option<&'static str> {
    TOOLBAR_ACTIONS
        .iter()
        .find(|(aid, _)| *aid == id)
        .map(|(_, label)| *label)
}

/// Whether `id` is a known catalog action (an unknown id is skipped at render).
pub(crate) fn is_known_action(id: &str) -> bool {
    action_label(id).is_some()
}

/// Move the item at `idx` one slot toward the front (LEFT). No-op at the front or
/// on an out-of-range index. Returns whether the list changed.
pub(crate) fn move_up(list: &mut [String], idx: usize) -> bool {
    if idx == 0 || idx >= list.len() {
        return false;
    }
    list.swap(idx - 1, idx);
    true
}

/// Move the item at `idx` one slot toward the back (RIGHT). No-op at the back or
/// on an out-of-range index. Returns whether the list changed.
pub(crate) fn move_down(list: &mut [String], idx: usize) -> bool {
    if idx + 1 >= list.len() {
        return false;
    }
    list.swap(idx, idx + 1);
    true
}

/// Remove the item at `idx`, returning the removed id (or `None` if out of range).
pub(crate) fn remove_at(list: &mut Vec<String>, idx: usize) -> Option<String> {
    if idx >= list.len() {
        return None;
    }
    Some(list.remove(idx))
}

/// Append `id` to `list` if it is a known action AND not already present anywhere
/// in either list (`items` + `menu` are the two lists an id can live in — an
/// action lives in at most one). Returns whether it was added.
pub(crate) fn add_unique(list: &mut Vec<String>, other: &[String], id: &str) -> bool {
    if !is_known_action(id) || list.iter().any(|x| x == id) || other.iter().any(|x| x == id) {
        return false;
    }
    list.push(id.to_string());
    true
}

/// Catalog action ids not currently present in EITHER `items` or `menu` — the
/// palette of actions the "Add ▾" menu offers, in canonical catalog order.
pub(crate) fn available_to_add(items: &[String], menu: &[String]) -> Vec<&'static str> {
    TOOLBAR_ACTIONS
        .iter()
        .map(|(id, _)| *id)
        .filter(|id| !items.iter().any(|x| x == id) && !menu.iter().any(|x| x == id))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &[&str]) -> Vec<String> {
        s.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn catalog_ids_are_unique_and_known() {
        // No duplicate ids in the catalog, and every default item is a catalog id.
        let mut seen = std::collections::HashSet::new();
        for (id, label) in TOOLBAR_ACTIONS {
            assert!(seen.insert(*id), "duplicate catalog id {id}");
            assert!(!label.is_empty());
            assert!(is_known_action(id));
        }
        for id in c0pl4nd_core::config::ToolbarConfig::default_items() {
            assert!(is_known_action(&id), "default item {id} not in catalog");
        }
        assert!(!is_known_action("not_a_real_action"));
        assert_eq!(action_label("view_mode"), Some("Toggle grid / tabs view"));
        assert_eq!(action_label("nope"), None);
    }

    #[test]
    fn move_up_reorders_and_clamps() {
        let mut list = v(&["a", "b", "c"]);
        assert!(move_up(&mut list, 2)); // c moves before b
        assert_eq!(list, v(&["a", "c", "b"]));
        assert!(!move_up(&mut list, 0)); // front is a no-op
        assert_eq!(list, v(&["a", "c", "b"]));
        assert!(!move_up(&mut list, 9)); // out of range is a safe no-op
        assert_eq!(list, v(&["a", "c", "b"]));
    }

    #[test]
    fn move_down_reorders_and_clamps() {
        let mut list = v(&["a", "b", "c"]);
        assert!(move_down(&mut list, 0)); // a moves after b
        assert_eq!(list, v(&["b", "a", "c"]));
        assert!(!move_down(&mut list, 2)); // back is a no-op
        assert_eq!(list, v(&["b", "a", "c"]));
        assert!(!move_down(&mut list, 9)); // out of range is a safe no-op
        assert_eq!(list, v(&["b", "a", "c"]));
    }

    #[test]
    fn remove_at_returns_id_and_clamps() {
        let mut list = v(&["a", "b", "c"]);
        assert_eq!(remove_at(&mut list, 1).as_deref(), Some("b"));
        assert_eq!(list, v(&["a", "c"]));
        assert_eq!(remove_at(&mut list, 9), None); // out of range
        assert_eq!(list, v(&["a", "c"]));
    }

    #[test]
    fn add_unique_dedups_across_both_lists_and_rejects_unknown() {
        let mut items = v(&["view_mode"]);
        let menu = v(&["shell_switcher"]);
        // Known, absent → added.
        assert!(add_unique(&mut items, &menu, "equalize_panes"));
        assert_eq!(items, v(&["view_mode", "equalize_panes"]));
        // Already in items → rejected.
        assert!(!add_unique(&mut items, &menu, "view_mode"));
        // Present in the OTHER list (menu) → rejected (an action lives in one list).
        assert!(!add_unique(&mut items, &menu, "shell_switcher"));
        // Unknown id → rejected.
        assert!(!add_unique(&mut items, &menu, "bogus"));
        assert_eq!(items, v(&["view_mode", "equalize_panes"]));
    }

    #[test]
    fn available_to_add_excludes_present_in_either_list() {
        let items = v(&["view_mode", "script_launcher"]);
        let menu = v(&["equalize_panes"]);
        // Only shell_switcher is unplaced.
        assert_eq!(available_to_add(&items, &menu), vec!["shell_switcher"]);
        // Everything placed → empty palette.
        let full = c0pl4nd_core::config::ToolbarConfig::default_items();
        assert!(available_to_add(&full, &[]).is_empty());
    }
}
