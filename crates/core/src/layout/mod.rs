//! Split-tree layout engine for C0PL4ND's multiplexing grid.
//!
//! Pure, UI-free geometry and tree logic — no winit, wgpu, glyphon, or PTY
//! dependency. The engine owns the split-tree *structure* and stable *ids*;
//! the app crate maps each [`LeafId`] to its live `Session`s and drives
//! rendering from the [`cascade`](Layout::cascade) output. This is the
//! foundation every later phase (render integration, keyboard actions, nested
//! tabs, drag-rearrange, persistence) builds on.
//!
//! # Model
//!
//! - [`LayoutNode`] is either a [`LayoutNode::Split`] (an [`Axis`] plus
//!   weighted [`Child`]ren) or a [`LayoutNode::Leaf`] referencing a
//!   [`TabGroup`] by [`LeafId`].
//! - [`Layout`] wraps the root node with `focused`, an optional `zoomed`
//!   render override, and a deterministic id allocator.
//!
//! # Capabilities (Phase 1)
//!
//! | Concern | Entry point |
//! |---|---|
//! | Pixel cascade with gutters | [`Layout::cascade`] |
//! | Split a leaf | [`Layout::split`] / [`Layout::try_split`] (MAX_PANES guard) |
//! | Remove + collapse | [`Layout::remove`] |
//! | Resize a split | [`Layout::resize`] |
//! | Directional focus | [`Layout::neighbor`] / [`Layout::focus_dir`] |
//! | Directional swap | [`Layout::swap_focused`] |
//! | Pane-zoom (pure query) | [`Layout::zoom_target`] / [`Layout::toggle_zoom`] |
//! | Squarest-grid rebalance | [`Layout::rebalance_squarest`] |
//! | serde persistence | `#[derive(Serialize, Deserialize)]` on [`Layout`] |

mod action;
mod geometry;
mod nav;
mod ops;
mod tree;

pub use action::{Preset, SplitOutcome, MAX_PANES};
pub use geometry::{Rect, GUTTER, MIN_CELL};
pub use nav::Direction;
pub use ops::RemoveOutcome;
pub use tree::{Axis, Child, Layout, LayoutNode, LeafId, TabGroup, TabSlot};

#[cfg(test)]
mod tests {
    //! Cross-module integration tests: serde round-trip and end-to-end
    //! split→navigate→remove→rebalance sequences against the public surface.

    use super::*;

    #[test]
    fn serde_round_trip_is_structurally_stable() {
        // Build a non-trivial 2x2-ish tree.
        let mut l = Layout::new();
        let win = Rect::new(0, 0, 800, 600);
        let a = match l.try_split(LeafId(0), Axis::Horizontal) {
            SplitOutcome::Split(id) => id,
            o => panic!("{o:?}"),
        };
        let _ = l.try_split(LeafId(0), Axis::Vertical);
        let _ = l.try_split(a, Axis::Vertical);
        l.focused = a;
        l.zoomed = None;

        let json = serde_json::to_string(&l).expect("serialize");
        let back: Layout = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(l, back, "round-trip must be structurally identical");

        // Byte-stable: re-serializing the decoded value yields the same bytes.
        let json2 = serde_json::to_string(&back).expect("reserialize");
        assert_eq!(json, json2, "serde output must be byte-stable");

        // Cascade equivalence after round-trip.
        assert_eq!(l.cascade(win), back.cascade(win));
    }

    #[test]
    fn serde_round_trip_with_zoom_and_focus() {
        let mut l = Layout::new();
        let b = match l.try_split(LeafId(0), Axis::Horizontal) {
            SplitOutcome::Split(id) => id,
            o => panic!("{o:?}"),
        };
        l.toggle_zoom(); // zoom focused (b)
        assert_eq!(l.zoom_target(), Some(b));

        let json = serde_json::to_string(&l).unwrap();
        let back: Layout = serde_json::from_str(&json).unwrap();
        assert_eq!(back.zoomed, Some(b));
        assert_eq!(back.focused, b);
        assert_eq!(l, back);
    }

    #[test]
    fn end_to_end_split_navigate_remove_rebalance() {
        let mut l = Layout::new();
        let win = Rect::new(0, 0, 1000, 800);

        // Grow to a 2x2-ish grid via the guarded action layer.
        let mut ids = vec![LeafId(0)];
        for _ in 0..3 {
            let target = *ids.last().unwrap();
            let axis = if ids.len() % 2 == 1 {
                Axis::Horizontal
            } else {
                Axis::Vertical
            };
            if let SplitOutcome::Split(id) = l.try_split(target, axis) {
                ids.push(id);
            }
        }
        assert_eq!(l.leaf_count(), 4);

        // Rebalance into the squarest grid (2x2) and cascade cleanly.
        l.rebalance_squarest();
        let rects = l.cascade(win);
        assert_eq!(rects.len(), 4);
        for (_, r) in &rects {
            assert!(r.w > 0 && r.h > 0);
        }

        // Navigate from the first cell rightward, then remove it.
        let first = rects[0].0;
        l.focused = first;
        let right = l.neighbor(first, Direction::Right, win);
        assert!(right.is_some(), "2x2 first cell must have a right neighbor");
        let outcome = l.remove(first);
        assert!(matches!(
            outcome,
            RemoveOutcome::Removed | RemoveOutcome::Collapsed
        ));
        assert_eq!(l.leaf_count(), 3);
        assert!(l.contains(l.focused), "focus must survive removal");

        // The freed flex kept the tree valid — cascade still tiles.
        let rects = l.cascade(win);
        assert_eq!(rects.len(), 3);
        for (_, r) in &rects {
            assert!(r.w > 0 && r.h > 0);
        }
    }

    #[test]
    fn max_panes_is_six() {
        assert_eq!(MAX_PANES, 6);
        let mut l = Layout::new();
        for _ in 0..10 {
            let _ = l.try_split(l.focused, Axis::Horizontal);
        }
        assert_eq!(l.leaf_count(), MAX_PANES, "must never exceed 6 panes");
    }

    #[test]
    fn public_types_construct() {
        // Smoke: every re-exported type is reachable and constructible.
        let _ = TabGroup::new(LeafId(1), 0);
        let _ = TabSlot::new(3);
        let _ = Child::new(LayoutNode::leaf(LeafId(0)), 1.0);
        let _ = Rect::new(0, 0, GUTTER, MIN_CELL);
        let _ = Direction::Left;
        let _ = Axis::Horizontal;
    }
}
