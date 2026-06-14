//! Persisted split-pane layout — serialize the `egui_tiles` tree + per-pane cwd
//! across launches so a user's panes/splits and their working directories come
//! back (fresh shells; live processes are an explicit non-goal — a process from a
//! previous run cannot be re-attached).
//!
//! The snapshot rides eframe's `persistence` feature: [`C0pl4ndApp::save`] writes
//! it under [`LAYOUT_STORAGE_KEY`] via `eframe::set_value` (RON, in the app's
//! `with_app_id` data folder, local-only) and the constructor reads it back via
//! `eframe::get_value`. The egui-tiles `serde` feature is on by default, and
//! [`Pane`]/[`PaneId`] derive `Serialize`/`Deserialize` (`PaneId` is
//! `#[serde(transparent)]` over a `u64`), so the tree round-trips as-is.
//!
//! Only structural state is persisted — never typed text. `cwds` holds filesystem
//! paths the shell already reported via OSC 7; nothing here touches scrollback,
//! history, or the egui `Memory` undo stack (that stays in-memory per the privacy
//! `persist_egui_memory() == false` policy).

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use super::grid::{self, PaneId};

/// Storage key for the persisted layout snapshot (RON value under the app-id
/// data folder). Versioned so a future incompatible shape can bump the suffix
/// rather than mis-deserialize an old blob (a failed `get_value` is treated as
/// "no snapshot" and falls back to the default grid — never a panic).
pub const LAYOUT_STORAGE_KEY: &str = "c0pl4nd_layout_v1";

/// A persisted snapshot of the pane layout: the tiling tree, each live pane's
/// working directory, the focused pane, the pinned set, and the allocator's next
/// id so restored ids never collide with freshly-allocated ones.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LayoutSnapshot {
    /// The full `egui_tiles` tree (splits, tabs, fractions, pane leaves).
    pub tree: egui_tiles::Tree<super::grid::Pane>,
    /// Per-pane working directory (OSC 7), for panes that reported one. A pane
    /// with no entry simply re-spawns in the default dir.
    pub cwds: HashMap<PaneId, String>,
    /// The focused pane at save time.
    pub focused: PaneId,
    /// The pinned-pane set at save time.
    pub pinned: Vec<PaneId>,
    /// The pane-id allocator's next counter at save time.
    pub next_id: u64,
}

/// Whether a restored snapshot is structurally usable: it must contain at least
/// one pane and no more than the live pane cap. An out-of-range pane count means
/// the blob is stale/corrupt against the current build and the caller keeps the
/// default grid instead.
pub fn snapshot_is_restorable(pane_count: usize) -> bool {
    (1..=grid::MAX_PANES).contains(&pane_count)
}

/// The focus to use after restore: the saved focus if it is still a pane in the
/// restored tree, otherwise the first pane in visual order (a saved focus can
/// dangle if the tree shape and focus disagree in a corrupt blob).
pub fn restored_focus(panes: &[PaneId], saved: PaneId) -> PaneId {
    if panes.contains(&saved) {
        saved
    } else {
        panes[0]
    }
}

/// The allocator counter to resume from: at least the saved `next_id`, and
/// always strictly greater than every restored pane id, so a fresh pane can
/// never collide with a restored one even if the saved counter was stale.
pub fn restored_next_id(panes: &[PaneId], saved_next: u64) -> u64 {
    let max_present = panes
        .iter()
        .map(|p| p.raw().wrapping_add(1))
        .max()
        .unwrap_or(0);
    saved_next.max(max_present)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::egui_app::grid::{self, PaneId};

    fn snapshot_with(panes: &[PaneId], focused: PaneId, next_id: u64) -> LayoutSnapshot {
        LayoutSnapshot {
            tree: grid::build_default_grid(panes),
            cwds: HashMap::new(),
            focused,
            pinned: Vec::new(),
            next_id,
        }
    }

    #[test]
    fn snapshot_round_trips_through_ron() {
        // The risky external dependency: egui_tiles' tree must survive a
        // serialize→deserialize cycle with its pane ids and shape intact.
        let panes = [PaneId(0), PaneId(1), PaneId(2)];
        let mut snap = snapshot_with(&panes, PaneId(1), 3);
        snap.cwds.insert(PaneId(0), "/home/op/work".to_string());
        snap.pinned.push(PaneId(2));

        let encoded = ron::to_string(&snap).expect("serialize");
        let decoded: LayoutSnapshot = ron::from_str(&encoded).expect("deserialize");

        assert_eq!(
            grid::panes_in_visual_order(&decoded.tree),
            grid::panes_in_visual_order(&snap.tree),
        );
        assert_eq!(decoded.focused, PaneId(1));
        assert_eq!(decoded.next_id, 3);
        assert_eq!(decoded.pinned, vec![PaneId(2)]);
        assert_eq!(
            decoded.cwds.get(&PaneId(0)).map(String::as_str),
            Some("/home/op/work")
        );
    }

    #[test]
    fn restorable_only_within_one_to_max_panes() {
        assert!(!snapshot_is_restorable(0));
        assert!(snapshot_is_restorable(1));
        assert!(snapshot_is_restorable(grid::MAX_PANES));
        assert!(!snapshot_is_restorable(grid::MAX_PANES + 1));
    }

    #[test]
    fn focus_falls_back_to_first_pane_when_saved_focus_dangles() {
        let panes = [PaneId(5), PaneId(7)];
        assert_eq!(restored_focus(&panes, PaneId(7)), PaneId(7));
        // A saved focus absent from the tree falls back to the first pane.
        assert_eq!(restored_focus(&panes, PaneId(99)), PaneId(5));
    }

    #[test]
    fn next_id_always_clears_every_restored_id() {
        let panes = [PaneId(2), PaneId(9), PaneId(4)];
        // Stale saved counter (3) is lifted above the max present id (9) + 1.
        assert_eq!(restored_next_id(&panes, 3), 10);
        // A saved counter already past the max is preserved.
        assert_eq!(restored_next_id(&panes, 20), 20);
        // Empty pane list → saved counter passes through unchanged.
        assert_eq!(restored_next_id(&[], 7), 7);
    }
}
