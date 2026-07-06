//! The customizable top-bar quick-action toolbar: the action CATALOG plus the
//! pure list/zone operations (reorder within a zone, move between zones, the
//! hidden pool) that back the Settings → Toolbar editor. Kept UI-free and
//! side-effect-free so the logic is unit-testable without a live `egui` context;
//! the rendering lives in `chrome.rs` and the settings widgets in `settings.rs`.
//!
//! Each catalog action is placed in exactly ONE of three zones — the LEFT group
//! (titlebar flow, after the "+"), the RIGHT cluster (by the settings gear), or
//! the overflow "⋯" menu — or is HIDDEN (in none). An action `id` is a stable,
//! config-persisted string (see [`c0pl4nd_core::config::ToolbarConfig`]); the
//! human label is resolved from [`TOOLBAR_ACTIONS`]. An id not in the catalog (a
//! config from a newer/older build) is skipped at render time, so the bar never
//! breaks.

/// The catalog of customizable quick actions, as `(id, human_label)`. The `id` is
/// what persists in `config.toolbar.{left,right,menu}`; the label shows in the
/// Settings → Toolbar editor and the overflow "⋯" menu. Order here is the
/// canonical order the "hidden" pool lists in. The fixed affordances (wordmark,
/// tabs, the tab-adjacent "+", and the window caption cluster) are NOT in this
/// catalog.
pub(crate) const TOOLBAR_ACTIONS: &[(&str, &str)] = &[
    ("view_mode", "Toggle grid / tabs view"),
    ("equalize_panes", "Equalize pane sizes"),
    ("shell_switcher", "Shell switcher"),
    ("script_launcher", "Script launcher"),
];

/// The three placement zones an action can live in (plus `Hidden` = none).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Zone {
    /// The titlebar flow, after the tab-adjacent "+".
    Left,
    /// The cluster pinned to the right, by the settings gear.
    Right,
    /// The overflow "⋯" menu.
    Menu,
    /// Not shown anywhere (removed from the bar).
    Hidden,
}

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

/// Move the item at `idx` one slot toward the front (earlier / LEFT). No-op at the
/// front or on an out-of-range index. Returns whether the list changed.
pub(crate) fn move_up(list: &mut [String], idx: usize) -> bool {
    if idx == 0 || idx >= list.len() {
        return false;
    }
    list.swap(idx - 1, idx);
    true
}

/// Move the item at `idx` one slot toward the back (later / RIGHT). No-op at the
/// back or on an out-of-range index. Returns whether the list changed.
pub(crate) fn move_down(list: &mut [String], idx: usize) -> bool {
    if idx + 1 >= list.len() {
        return false;
    }
    list.swap(idx, idx + 1);
    true
}

/// Move `id` into `target`, first removing it from EVERY zone (an action lives in
/// at most one zone; `Hidden` leaves it in none). A known id is appended to the
/// end of the target list. Returns whether the placement actually changed the
/// lists. An unknown id is a no-op.
pub(crate) fn move_to_zone(
    left: &mut Vec<String>,
    right: &mut Vec<String>,
    menu: &mut Vec<String>,
    target: Zone,
    id: &str,
) -> bool {
    if !is_known_action(id) {
        return false;
    }
    let before = (left.clone(), right.clone(), menu.clone());
    left.retain(|x| x != id);
    right.retain(|x| x != id);
    menu.retain(|x| x != id);
    match target {
        Zone::Left => left.push(id.to_string()),
        Zone::Right => right.push(id.to_string()),
        Zone::Menu => menu.push(id.to_string()),
        Zone::Hidden => {}
    }
    (&before.0, &before.1, &before.2) != (&*left, &*right, &*menu)
}

/// Catalog action ids not present in ANY of the three zones — the HIDDEN pool the
/// "Add ▾" palettes offer (in canonical catalog order).
pub(crate) fn hidden_actions(
    left: &[String],
    right: &[String],
    menu: &[String],
) -> Vec<&'static str> {
    TOOLBAR_ACTIONS
        .iter()
        .map(|(id, _)| *id)
        .filter(|id| {
            !left.iter().any(|x| x == id)
                && !right.iter().any(|x| x == id)
                && !menu.iter().any(|x| x == id)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &[&str]) -> Vec<String> {
        s.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn catalog_ids_are_unique_and_defaults_are_known() {
        let mut seen = std::collections::HashSet::new();
        for (id, label) in TOOLBAR_ACTIONS {
            assert!(seen.insert(*id), "duplicate catalog id {id}");
            assert!(!label.is_empty());
            assert!(is_known_action(id));
        }
        // Every default-zoned id is a catalog id; together they cover the catalog.
        let d = c0pl4nd_core::config::ToolbarConfig::default();
        for id in d.left.iter().chain(d.right.iter()).chain(d.menu.iter()) {
            assert!(is_known_action(id), "default id {id} not in catalog");
        }
        assert!(hidden_actions(&d.left, &d.right, &d.menu).is_empty());
        assert!(!is_known_action("not_a_real_action"));
        assert_eq!(action_label("view_mode"), Some("Toggle grid / tabs view"));
        assert_eq!(action_label("nope"), None);
    }

    #[test]
    fn move_up_down_reorder_and_clamp() {
        let mut list = v(&["a", "b", "c"]);
        assert!(move_up(&mut list, 2));
        assert_eq!(list, v(&["a", "c", "b"]));
        assert!(!move_up(&mut list, 0)); // front no-op
        assert!(!move_up(&mut list, 9)); // out of range no-op
        assert!(move_down(&mut list, 0));
        assert_eq!(list, v(&["c", "a", "b"]));
        assert!(!move_down(&mut list, 2)); // back no-op
        assert!(!move_down(&mut list, 9)); // out of range no-op
    }

    #[test]
    fn move_to_zone_relocates_and_dedups_across_zones() {
        let mut left = v(&["view_mode", "equalize_panes", "shell_switcher"]);
        let mut right = v(&["script_launcher"]);
        let mut menu = v(&[]);

        // Pin shell_switcher to the RIGHT — it leaves LEFT and joins RIGHT (end).
        assert!(move_to_zone(
            &mut left,
            &mut right,
            &mut menu,
            Zone::Right,
            "shell_switcher"
        ));
        assert_eq!(left, v(&["view_mode", "equalize_panes"]));
        assert_eq!(right, v(&["script_launcher", "shell_switcher"]));

        // Park view_mode in the overflow menu (leaves LEFT).
        assert!(move_to_zone(
            &mut left,
            &mut right,
            &mut menu,
            Zone::Menu,
            "view_mode"
        ));
        assert_eq!(left, v(&["equalize_panes"]));
        assert_eq!(menu, v(&["view_mode"]));

        // Hide equalize_panes (leaves LEFT, joins no zone).
        assert!(move_to_zone(
            &mut left,
            &mut right,
            &mut menu,
            Zone::Hidden,
            "equalize_panes"
        ));
        assert!(left.is_empty());
        assert_eq!(hidden_actions(&left, &right, &menu), vec!["equalize_panes"]);

        // Re-placing where it already is (alone) is a no-op → not "changed".
        assert!(!move_to_zone(
            &mut left,
            &mut right,
            &mut menu,
            Zone::Menu,
            "view_mode"
        ));
        // An unknown id never mutates.
        assert!(!move_to_zone(
            &mut left,
            &mut right,
            &mut menu,
            Zone::Right,
            "bogus"
        ));
    }

    #[test]
    fn hidden_actions_excludes_every_placed_id() {
        let left = v(&["view_mode"]);
        let right = v(&["script_launcher"]);
        let menu = v(&["equalize_panes"]);
        // Only shell_switcher is unplaced.
        assert_eq!(hidden_actions(&left, &right, &menu), vec!["shell_switcher"]);
    }
}
