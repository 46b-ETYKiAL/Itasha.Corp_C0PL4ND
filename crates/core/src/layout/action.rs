//! Action-layer guards and grid helpers.
//!
//! Phase 1 ships the pieces of the action layer that are pure tree logic: the
//! `MAX_PANES` readability guardrail enforced on split, the directional swap,
//! the pane-zoom query, and a "rebalance into the squarest grid" helper used
//! by the grid presets. The keyboard/palette wiring (winit events, PTY
//! resize, toasts) lands in the app crate in Phase 3.

use super::geometry::Rect;
use super::nav::Direction;
use super::tree::{Axis, Child, Layout, LayoutNode, LeafId};

/// Maximum number of panes allowed in one window. A deliberate readability
/// guardrail (the product differentiator) enforced at the action layer — the
/// raw tree primitives in `ops.rs` do not cap, so callers must route splits
/// through [`Layout::try_split`].
pub const MAX_PANES: usize = 6;

/// A quick-layout preset (Phase 6 / research §4). Selecting a preset rebuilds
/// the split tree into a fixed shape and allocates the leaves it needs; the app
/// then spawns/reuses one shell per leaf. All presets respect [`MAX_PANES`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Preset {
    /// One full-window pane.
    Single,
    /// Two columns, side by side (`1x2`).
    TwoColumns,
    /// Two rows, stacked (`2x1`).
    TwoRows,
    /// A main pane on the left + two stacked panes on the right (`1+2`).
    MainLeftTwoStacked,
    /// A 2x2 grid (four panes).
    Grid2x2,
    /// A main pane on the left + three stacked panes on the right (`1+3`).
    MainLeftThreeStacked,
    /// A 2x3 grid (six panes — the maximum).
    Grid2x3,
}

impl Preset {
    /// Every preset, in palette display order.
    pub const ALL: [Preset; 7] = [
        Preset::Single,
        Preset::TwoColumns,
        Preset::TwoRows,
        Preset::MainLeftTwoStacked,
        Preset::Grid2x2,
        Preset::MainLeftThreeStacked,
        Preset::Grid2x3,
    ];

    /// The short shape label used in the command palette (e.g. `2x2`, `1+2`).
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Preset::Single => "1",
            Preset::TwoColumns => "1x2",
            Preset::TwoRows => "2x1",
            Preset::MainLeftTwoStacked => "1+2",
            Preset::Grid2x2 => "2x2",
            Preset::MainLeftThreeStacked => "1+3",
            Preset::Grid2x3 => "2x3",
        }
    }

    /// Number of leaves this preset produces (always `1..=MAX_PANES`).
    #[must_use]
    pub fn leaf_count(self) -> usize {
        match self {
            Preset::Single => 1,
            Preset::TwoColumns | Preset::TwoRows => 2,
            Preset::MainLeftTwoStacked => 3,
            Preset::Grid2x2 => 4,
            Preset::MainLeftThreeStacked => 4,
            Preset::Grid2x3 => 6,
        }
    }

    /// Match a palette label (`"1"`, `"2x2"`, …) back to a preset.
    #[must_use]
    pub fn from_label(label: &str) -> Option<Preset> {
        Preset::ALL.into_iter().find(|p| p.label() == label)
    }
}

/// Outcome of an action-layer split attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplitOutcome {
    /// Split applied; carries the new leaf's id.
    Split(LeafId),
    /// Rejected because the window is already at [`MAX_PANES`].
    AtCapacity,
    /// Target leaf was not found.
    NotFound,
}

impl Layout {
    /// Action-layer split that enforces the [`MAX_PANES`] guardrail. Allocates
    /// a fresh leaf id, splits `target` along `axis`, focuses the new leaf,
    /// and returns the outcome. The 7th split (when `leaf_count == MAX_PANES`)
    /// is rejected with [`SplitOutcome::AtCapacity`].
    pub fn try_split(&mut self, target: LeafId, axis: Axis) -> SplitOutcome {
        if !self.contains(target) {
            return SplitOutcome::NotFound;
        }
        if self.leaf_count() >= MAX_PANES {
            return SplitOutcome::AtCapacity;
        }
        let new_leaf = self.alloc_id();
        if self.split(target, axis, new_leaf) {
            self.focused = new_leaf;
            SplitOutcome::Split(new_leaf)
        } else {
            SplitOutcome::NotFound
        }
    }

    /// `true` when no further split is permitted.
    #[must_use]
    pub fn at_capacity(&self) -> bool {
        self.leaf_count() >= MAX_PANES
    }

    /// Swap the focused leaf with its `dir` neighbor (computed against
    /// `window`). Pure structural swap — exchanges the two leaf ids in place,
    /// so flex ratios and tree shape are unchanged. Returns `true` if a swap
    /// happened. Focus follows the moved pane.
    pub fn swap_focused(&mut self, dir: Direction, window: Rect) -> bool {
        let from = self.focused;
        let Some(to) = self.neighbor(from, dir, window) else {
            return false;
        };
        swap_ids(&mut self.root, from, to);
        // Focus stays on the same logical pane, which now sits where `to` was.
        self.focused = from;
        true
    }

    /// Pane-zoom as a pure query: the leaf the renderer should draw
    /// full-window, or `None` when not zoomed. Never mutates the tree.
    #[must_use]
    pub fn zoom_target(&self) -> Option<LeafId> {
        self.zoomed.filter(|id| self.contains(*id))
    }

    /// Toggle zoom on the focused leaf (sets/clears the render override).
    pub fn toggle_zoom(&mut self) {
        self.zoomed = match self.zoomed {
            Some(id) if id == self.focused => None,
            _ => Some(self.focused),
        };
    }

    /// Rebuild the tree as the squarest grid that holds the current leaves
    /// (preserving their ids and DFS order), without exceeding [`MAX_PANES`].
    /// Used by the grid presets ("auto-arrange"). Columns = `ceil(sqrt(n))`,
    /// rows fill left-to-right, top-to-bottom; the last row may be short.
    pub fn rebalance_squarest(&mut self) {
        let leaves = self.leaves();
        let n = leaves.len();
        if n <= 1 {
            return;
        }
        let cols = (n as f64).sqrt().ceil() as usize;
        let rows = n.div_ceil(cols);

        // Build rows of up to `cols` leaves each as horizontal splits, then a
        // vertical split over the rows.
        let mut row_children = Vec::with_capacity(rows);
        let mut idx = 0;
        for _ in 0..rows {
            let take = cols.min(n - idx);
            let cells: Vec<_> = (0..take)
                .map(|k| {
                    super::tree::Child::new(LayoutNode::leaf(leaves[idx + k]), 1.0 / take as f32)
                })
                .collect();
            idx += take;
            let row_node = if take == 1 {
                LayoutNode::leaf(leaves[idx - 1])
            } else {
                LayoutNode::Split {
                    axis: Axis::Horizontal,
                    children: cells,
                }
            };
            row_children.push(super::tree::Child::new(row_node, 1.0 / rows as f32));
        }
        self.root = if rows == 1 {
            // A single row → just the horizontal split (or the lone leaf).
            row_children.into_iter().next().map(|c| c.node).unwrap()
        } else {
            LayoutNode::Split {
                axis: Axis::Vertical,
                children: row_children,
            }
        };
        if !self.contains(self.focused) {
            self.focused = self.leaves().first().copied().unwrap_or(self.focused);
        }
    }

    /// Build a fresh [`Layout`] for `preset`, allocating leaf ids `0..count`
    /// deterministically and focusing the first leaf. Pure constructor: the app
    /// reads [`Layout::leaves`] to know how many shells to spawn/reuse. The
    /// resulting tree never exceeds [`MAX_PANES`] (every [`Preset`] is bounded).
    #[must_use]
    pub fn from_preset(preset: Preset) -> Layout {
        let mut next = 0u64;
        let mut leaf = || {
            let id = LeafId(next);
            next += 1;
            id
        };
        let cols = |children: Vec<LayoutNode>| split(Axis::Horizontal, children);
        let rows = |children: Vec<LayoutNode>| split(Axis::Vertical, children);

        let root = match preset {
            Preset::Single => LayoutNode::leaf(leaf()),
            Preset::TwoColumns => cols(vec![LayoutNode::leaf(leaf()), LayoutNode::leaf(leaf())]),
            Preset::TwoRows => rows(vec![LayoutNode::leaf(leaf()), LayoutNode::leaf(leaf())]),
            Preset::MainLeftTwoStacked => {
                let main = LayoutNode::leaf(leaf());
                let side = rows(vec![LayoutNode::leaf(leaf()), LayoutNode::leaf(leaf())]);
                cols(vec![main, side])
            }
            Preset::Grid2x2 => {
                let r0 = cols(vec![LayoutNode::leaf(leaf()), LayoutNode::leaf(leaf())]);
                let r1 = cols(vec![LayoutNode::leaf(leaf()), LayoutNode::leaf(leaf())]);
                rows(vec![r0, r1])
            }
            Preset::MainLeftThreeStacked => {
                let main = LayoutNode::leaf(leaf());
                let side = rows(vec![
                    LayoutNode::leaf(leaf()),
                    LayoutNode::leaf(leaf()),
                    LayoutNode::leaf(leaf()),
                ]);
                cols(vec![main, side])
            }
            Preset::Grid2x3 => {
                let r0 = cols(vec![LayoutNode::leaf(leaf()), LayoutNode::leaf(leaf())]);
                let r1 = cols(vec![LayoutNode::leaf(leaf()), LayoutNode::leaf(leaf())]);
                let r2 = cols(vec![LayoutNode::leaf(leaf()), LayoutNode::leaf(leaf())]);
                rows(vec![r0, r1, r2])
            }
        };

        let mut layout = Layout {
            root,
            focused: LeafId(0),
            zoomed: None,
            next_id: next,
        };
        // Equal flex everywhere so a preset always opens balanced.
        layout.equalize();
        layout
    }
}

/// Build an n-ary split with equal flex across `children`.
fn split(axis: Axis, children: Vec<LayoutNode>) -> LayoutNode {
    let n = children.len().max(1) as f32;
    LayoutNode::Split {
        axis,
        children: children
            .into_iter()
            .map(|node| Child::new(node, 1.0 / n))
            .collect(),
    }
}

/// Swap two leaf ids wherever they appear in the tree.
fn swap_ids(node: &mut LayoutNode, a: LeafId, b: LeafId) {
    match node {
        LayoutNode::Leaf(id) => {
            if *id == a {
                *id = b;
            } else if *id == b {
                *id = a;
            }
        }
        LayoutNode::Split { children, .. } => {
            for c in children.iter_mut() {
                swap_ids(&mut c.node, a, b);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fill_to(l: &mut Layout, target: usize) {
        while l.leaf_count() < target {
            let f = l.focused;
            // Alternate axes for a real grid shape.
            let axis = if l.leaf_count().is_multiple_of(2) {
                Axis::Horizontal
            } else {
                Axis::Vertical
            };
            assert!(matches!(l.try_split(f, axis), SplitOutcome::Split(_)));
        }
    }

    #[test]
    fn try_split_allocates_and_focuses() {
        let mut l = Layout::new();
        match l.try_split(LeafId(0), Axis::Horizontal) {
            SplitOutcome::Split(id) => {
                assert_eq!(l.focused, id);
                assert_eq!(l.leaf_count(), 2);
            }
            other => panic!("expected Split, got {other:?}"),
        }
    }

    #[test]
    fn max_panes_guard_rejects_seventh() {
        let mut l = Layout::new();
        fill_to(&mut l, MAX_PANES);
        assert_eq!(l.leaf_count(), MAX_PANES);
        assert!(l.at_capacity());
        // The 7th split is rejected.
        assert_eq!(
            l.try_split(l.focused, Axis::Horizontal),
            SplitOutcome::AtCapacity
        );
        assert_eq!(l.leaf_count(), MAX_PANES);
    }

    #[test]
    fn try_split_unknown_target() {
        let mut l = Layout::new();
        assert_eq!(
            l.try_split(LeafId(77), Axis::Horizontal),
            SplitOutcome::NotFound
        );
    }

    #[test]
    fn toggle_zoom_and_query() {
        let mut l = Layout::new();
        let b = l.alloc_id();
        l.split(LeafId(0), Axis::Horizontal, b);
        l.focused = b;
        assert_eq!(l.zoom_target(), None);
        l.toggle_zoom();
        assert_eq!(l.zoom_target(), Some(b));
        l.toggle_zoom();
        assert_eq!(l.zoom_target(), None);
        // Zoom does NOT mutate the tree.
        assert_eq!(l.leaf_count(), 2);
    }

    #[test]
    fn zoom_target_filters_stale() {
        let mut l = Layout::new();
        l.zoomed = Some(LeafId(999));
        assert_eq!(l.zoom_target(), None);
    }

    #[test]
    fn swap_exchanges_ids_in_place() {
        let mut l = Layout::new();
        let b = l.alloc_id();
        l.split(LeafId(0), Axis::Horizontal, b); // 0 | b
        l.focused = LeafId(0);
        let win = Rect::new(0, 0, 800, 600);
        // Swap focused (0) with right neighbor (b).
        assert!(l.swap_focused(Direction::Right, win));
        // Now the leaf that was on the left is `b`, right is `0`.
        let rects = l.cascade(win);
        assert_eq!(rects[0].0, b);
        assert_eq!(rects[1].0, LeafId(0));
        // Same leaf count, same shape.
        assert_eq!(l.leaf_count(), 2);
    }

    #[test]
    fn swap_no_neighbor_is_noop() {
        let mut l = Layout::new();
        let win = Rect::new(0, 0, 800, 600);
        assert!(!l.swap_focused(Direction::Left, win));
    }

    #[test]
    fn rebalance_squarest_for_1_to_6_leaves() {
        for n in 1..=MAX_PANES {
            let mut l = Layout::new();
            fill_to(&mut l, n);
            l.rebalance_squarest();
            assert_eq!(l.leaf_count(), n, "leaf count changed for n={n}");
            assert!(l.contains(l.focused), "focus dangling for n={n}");

            let win = Rect::new(0, 0, 1200, 900);
            let rects = l.cascade(win);
            assert_eq!(rects.len(), n);
            for (_, r) in &rects {
                assert!(r.w > 0 && r.h > 0, "empty cell for n={n}: {r:?}");
            }

            // Expected grid shape: cols = ceil(sqrt(n)).
            let cols = (n as f64).sqrt().ceil() as usize;
            match &l.root {
                LayoutNode::Leaf(_) => assert_eq!(n, 1),
                LayoutNode::Split { axis, children } => {
                    if n <= cols {
                        // Single row → horizontal split.
                        assert_eq!(*axis, Axis::Horizontal);
                        assert_eq!(children.len(), n);
                    } else {
                        // Multi-row → vertical split of rows.
                        assert_eq!(*axis, Axis::Vertical);
                        let rows = n.div_ceil(cols);
                        assert_eq!(children.len(), rows);
                    }
                }
            }
        }
    }

    #[test]
    fn every_preset_builds_expected_leaf_count_within_cap() {
        for preset in Preset::ALL {
            let l = Layout::from_preset(preset);
            assert_eq!(
                l.leaf_count(),
                preset.leaf_count(),
                "preset {} leaf count",
                preset.label()
            );
            assert!(
                l.leaf_count() <= MAX_PANES,
                "preset {} over cap",
                preset.label()
            );
            // Cascades cleanly with no empty cells.
            let win = Rect::new(0, 0, 1200, 900);
            let rects = l.cascade(win);
            assert_eq!(rects.len(), preset.leaf_count());
            for (_, r) in &rects {
                assert!(r.w > 0 && r.h > 0, "empty cell in {}", preset.label());
            }
            // Focus is a real leaf.
            assert!(l.contains(l.focused));
        }
    }

    #[test]
    fn preset_shapes_match_their_labels() {
        // 1x2 → one horizontal split of two leaves.
        let l = Layout::from_preset(Preset::TwoColumns);
        match &l.root {
            LayoutNode::Split { axis, children } => {
                assert_eq!(*axis, Axis::Horizontal);
                assert_eq!(children.len(), 2);
                assert!(children.iter().all(|c| c.node.is_leaf()));
            }
            _ => panic!("1x2 must be a horizontal split"),
        }

        // 2x1 → one vertical split of two leaves.
        let l = Layout::from_preset(Preset::TwoRows);
        match &l.root {
            LayoutNode::Split { axis, children } => {
                assert_eq!(*axis, Axis::Vertical);
                assert_eq!(children.len(), 2);
            }
            _ => panic!("2x1 must be a vertical split"),
        }

        // 1+2 → horizontal split: [leaf, vertical-split-of-2].
        let l = Layout::from_preset(Preset::MainLeftTwoStacked);
        match &l.root {
            LayoutNode::Split { axis, children } => {
                assert_eq!(*axis, Axis::Horizontal);
                assert_eq!(children.len(), 2);
                assert!(children[0].node.is_leaf(), "main pane is a leaf");
                match &children[1].node {
                    LayoutNode::Split { axis, children } => {
                        assert_eq!(*axis, Axis::Vertical);
                        assert_eq!(children.len(), 2);
                    }
                    _ => panic!("1+2 side must be a vertical split of 2"),
                }
            }
            _ => panic!("1+2 must be a horizontal split"),
        }

        // 2x2 → vertical split of two horizontal rows.
        let l = Layout::from_preset(Preset::Grid2x2);
        match &l.root {
            LayoutNode::Split { axis, children } => {
                assert_eq!(*axis, Axis::Vertical);
                assert_eq!(children.len(), 2);
                for row in children {
                    match &row.node {
                        LayoutNode::Split { axis, children } => {
                            assert_eq!(*axis, Axis::Horizontal);
                            assert_eq!(children.len(), 2);
                        }
                        _ => panic!("2x2 rows must be horizontal splits of 2"),
                    }
                }
            }
            _ => panic!("2x2 must be a vertical split"),
        }

        // 2x3 → vertical split of three horizontal rows = 6 leaves.
        let l = Layout::from_preset(Preset::Grid2x3);
        assert_eq!(l.leaf_count(), 6);
    }

    #[test]
    fn preset_label_round_trips() {
        for preset in Preset::ALL {
            assert_eq!(Preset::from_label(preset.label()), Some(preset));
        }
        assert_eq!(Preset::from_label("nope"), None);
    }

    #[test]
    fn rebalance_reattaches_dangling_focus() {
        // rebalance_squarest preserves the leaf-id SET, so a focus pointing at a
        // real leaf always survives. To exercise the focus-reattach guard, point
        // focus at a NON-existent id before rebalancing: the guard must move it
        // onto a real leaf rather than leave it dangling.
        let mut l = Layout::new();
        fill_to(&mut l, 4);
        l.focused = LeafId(9999); // not in the tree
        assert!(!l.contains(l.focused));
        l.rebalance_squarest();
        assert!(l.contains(l.focused), "rebalance must reattach a dangling focus");
    }

    #[test]
    fn rebalance_single_leaf_is_noop() {
        // n <= 1 returns early without rebuilding.
        let mut l = Layout::new();
        let root_before = l.root.clone();
        l.rebalance_squarest();
        assert_eq!(l.root, root_before, "single-leaf rebalance is a no-op");
    }

    #[test]
    fn rebalance_three_leaves_has_single_cell_last_row() {
        // n=3 → cols=2, rows=2; the last row holds a single leaf (the take==1
        // branch that builds a bare leaf row, not a 1-child split).
        let mut l = Layout::new();
        fill_to(&mut l, 3);
        l.rebalance_squarest();
        match &l.root {
            LayoutNode::Split { axis, children } => {
                assert_eq!(*axis, Axis::Vertical);
                assert_eq!(children.len(), 2, "two rows");
                // Row 0: a 2-leaf horizontal split. Row 1: a bare leaf.
                assert!(
                    matches!(children[0].node, LayoutNode::Split { .. }),
                    "first row is a horizontal split of two"
                );
                assert!(
                    children[1].node.is_leaf(),
                    "single-cell last row is a bare leaf, not a 1-child split"
                );
            }
            _ => panic!("3-leaf rebalance must be a vertical split"),
        }
        assert_eq!(l.leaf_count(), 3);
    }

    #[test]
    fn main_left_three_stacked_shape() {
        // The 1+3 preset: horizontal split of [main-leaf, vertical-split-of-3].
        let l = Layout::from_preset(Preset::MainLeftThreeStacked);
        assert_eq!(l.leaf_count(), 4);
        match &l.root {
            LayoutNode::Split { axis, children } => {
                assert_eq!(*axis, Axis::Horizontal);
                assert_eq!(children.len(), 2);
                assert!(children[0].node.is_leaf(), "main pane is a leaf");
                match &children[1].node {
                    LayoutNode::Split { axis, children } => {
                        assert_eq!(*axis, Axis::Vertical);
                        assert_eq!(children.len(), 3, "side stack holds three");
                    }
                    _ => panic!("1+3 side must be a vertical split of 3"),
                }
            }
            _ => panic!("1+3 must be a horizontal split"),
        }
    }

    #[test]
    fn swap_ids_handles_both_orderings() {
        // swap_ids has two arms (`id==a` and `id==b`). Build 0 | b | c and swap
        // 0 with c — whichever is visited first in DFS hits the other arm.
        let mut l = Layout::new();
        let b = l.alloc_id();
        l.split(LeafId(0), Axis::Horizontal, b);
        let c = l.alloc_id();
        l.split(b, Axis::Horizontal, c); // 0 | b | c
        l.focused = LeafId(0);
        let win = Rect::new(0, 0, 900, 300);
        // The right neighbor of leaf 0 is b; swap them.
        assert!(l.swap_focused(Direction::Right, win));
        let rects = l.cascade(win);
        // Leaf b now sits leftmost, 0 sits where b was.
        assert_eq!(rects[0].0, b);
        assert_eq!(rects[1].0, LeafId(0));
        assert_eq!(rects[2].0, c);
        assert_eq!(l.leaf_count(), 3);
    }

    #[test]
    fn at_capacity_tracks_max_panes() {
        let mut l = Layout::new();
        assert!(!l.at_capacity());
        fill_to(&mut l, MAX_PANES);
        assert!(l.at_capacity());
    }

    #[test]
    fn rebalance_preserves_leaf_ids() {
        let mut l = Layout::new();
        fill_to(&mut l, 5);
        let before: std::collections::BTreeSet<_> = l.leaves().into_iter().collect();
        l.rebalance_squarest();
        let after: std::collections::BTreeSet<_> = l.leaves().into_iter().collect();
        assert_eq!(before, after, "rebalance must preserve the set of leaf ids");
    }
}
