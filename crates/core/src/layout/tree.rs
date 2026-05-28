//! Split-tree data model for the C0PL4ND multiplexing layout engine.
//!
//! The tree is a binary/n-ary split tree with flex ratios — the model used by
//! Warp, Windows Terminal, and Tabby. A node is either a [`LayoutNode::Split`]
//! (an axis plus an ordered list of weighted children) or a
//! [`LayoutNode::Leaf`] (a [`LeafId`] referencing an app-side TabGroup). The
//! engine is deliberately UI-free: it owns *structure* and *ids*, never
//! `Session`s, PTYs, or rendering state. Geometry, tree edits, and navigation
//! live in sibling submodules.

use serde::{Deserialize, Serialize};

/// Stable, monotonically-allocated identifier for a leaf (a grid cell /
/// TabGroup). Survives tree moves so the app-side session store can keep a
/// stable mapping from `LeafId` to its `Session`s.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct LeafId(pub u64);

/// Split orientation.
///
/// `Horizontal` lays children out side-by-side (columns); `Vertical` stacks
/// them top-to-bottom (rows). This matches the Warp / Windows Terminal
/// convention where a "horizontal split" produces left/right panes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Axis {
    /// Children arranged left→right (a row of columns).
    Horizontal,
    /// Children arranged top→bottom (a column of rows).
    Vertical,
}

impl Axis {
    /// The axis orthogonal to this one.
    #[must_use]
    pub fn opposite(self) -> Axis {
        match self {
            Axis::Horizontal => Axis::Vertical,
            Axis::Vertical => Axis::Horizontal,
        }
    }
}

/// A weighted child within a [`LayoutNode::Split`]. `flex` is this child's
/// share of the parent's extent along the split axis; siblings' `flex` values
/// sum to `1.0` (the engine renormalizes after every edit).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Child {
    /// The subtree under this child slot.
    pub node: LayoutNode,
    /// Fractional share of the parent extent along the split axis. Positive;
    /// siblings normalized to sum 1.0.
    pub flex: f32,
}

impl Child {
    /// Construct a child with an explicit flex weight.
    #[must_use]
    pub fn new(node: LayoutNode, flex: f32) -> Self {
        Self { node, flex }
    }
}

/// A node in the split tree.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum LayoutNode {
    /// An internal split with an axis and 2+ weighted children.
    Split {
        /// Orientation of this split.
        axis: Axis,
        /// Ordered, weighted children (2 or more).
        children: Vec<Child>,
    },
    /// A terminal cell referencing an app-side TabGroup by id.
    Leaf(LeafId),
}

impl LayoutNode {
    /// Construct a single-leaf node.
    #[must_use]
    pub fn leaf(id: LeafId) -> Self {
        LayoutNode::Leaf(id)
    }

    /// `true` when this node is a leaf.
    #[must_use]
    pub fn is_leaf(&self) -> bool {
        matches!(self, LayoutNode::Leaf(_))
    }

    /// Number of leaves in this subtree.
    #[must_use]
    pub fn leaf_count(&self) -> usize {
        match self {
            LayoutNode::Leaf(_) => 1,
            LayoutNode::Split { children, .. } => {
                children.iter().map(|c| c.node.leaf_count()).sum()
            }
        }
    }

    /// Collect every [`LeafId`] in this subtree in left-to-right DFS order.
    pub fn collect_leaves(&self, out: &mut Vec<LeafId>) {
        match self {
            LayoutNode::Leaf(id) => out.push(*id),
            LayoutNode::Split { children, .. } => {
                for c in children {
                    c.node.collect_leaves(out);
                }
            }
        }
    }

    /// `true` when this subtree contains `target`.
    #[must_use]
    pub fn contains(&self, target: LeafId) -> bool {
        match self {
            LayoutNode::Leaf(id) => *id == target,
            LayoutNode::Split { children, .. } => children.iter().any(|c| c.node.contains(target)),
        }
    }

    /// Renormalize this node's direct children so their `flex` values sum to
    /// `1.0`. A no-op for leaves. Degenerate (all-zero / negative) weights are
    /// reset to a uniform distribution so the tree never produces NaN rects.
    pub fn renormalize_children(&mut self) {
        if let LayoutNode::Split { children, .. } = self {
            normalize_flex(children);
        }
    }
}

/// One tab slot inside a [`TabGroup`]. Core tracks only the opaque app-side
/// slot key and structural metadata; it never owns a `Session`. The app maps
/// `slot` → its live `Session` handle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TabSlot {
    /// Opaque index/key into the app-side session store.
    pub slot: u64,
}

impl TabSlot {
    /// Construct a tab slot from an app-side key.
    #[must_use]
    pub fn new(slot: u64) -> Self {
        Self { slot }
    }
}

/// A grid cell. Holds an ordered list of tab slots plus the active index.
/// A 1-tab group renders no tab bar (the render layer's concern); the engine
/// only tracks structure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TabGroup {
    /// Stable id, equal to the [`LeafId`] referencing this group in the tree.
    pub id: LeafId,
    /// Ordered tab slots; never empty for a live group.
    pub tabs: Vec<TabSlot>,
    /// Index into `tabs` of the visible tab.
    pub active: usize,
}

impl TabGroup {
    /// Construct a single-tab group.
    #[must_use]
    pub fn new(id: LeafId, slot: u64) -> Self {
        Self {
            id,
            tabs: vec![TabSlot::new(slot)],
            active: 0,
        }
    }

    /// The slot key of the currently-visible tab.
    #[must_use]
    pub fn active_slot(&self) -> u64 {
        self.tabs[self.active].slot
    }

    /// Number of tabs in this cell.
    #[must_use]
    pub fn len(&self) -> usize {
        self.tabs.len()
    }

    /// Whether the cell has no tabs (only transiently true mid-collapse).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.tabs.is_empty()
    }

    /// Append a tab for `slot` and make it active. Returns the new active index.
    pub fn add_tab(&mut self, slot: u64) -> usize {
        self.tabs.push(TabSlot::new(slot));
        self.active = self.tabs.len() - 1;
        self.active
    }

    /// Close the active tab. Returns the removed slot key, and `true` when the
    /// cell is now empty (the caller must collapse/remove the leaf). Active
    /// index clamps to the previous tab.
    pub fn close_active(&mut self) -> (u64, bool) {
        let removed = self.tabs.remove(self.active).slot;
        if self.tabs.is_empty() {
            return (removed, true);
        }
        if self.active >= self.tabs.len() {
            self.active = self.tabs.len() - 1;
        }
        (removed, false)
    }

    /// Cycle the active tab forward (wraps).
    pub fn next_tab(&mut self) {
        if !self.tabs.is_empty() {
            self.active = (self.active + 1) % self.tabs.len();
        }
    }

    /// Cycle the active tab backward (wraps).
    pub fn prev_tab(&mut self) {
        if !self.tabs.is_empty() {
            self.active = (self.active + self.tabs.len() - 1) % self.tabs.len();
        }
    }
}

/// The root layout: a split tree plus focus, an optional zoom override, and a
/// deterministic id allocator.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Layout {
    /// Root of the split tree.
    pub root: LayoutNode,
    /// Currently-focused leaf.
    pub focused: LeafId,
    /// When `Some`, the renderer draws only this leaf full-window. A pure
    /// render override — the tree is never mutated by zoom.
    pub zoomed: Option<LeafId>,
    /// Next id to hand out. Monotonic; never reused, so ids are stable.
    pub next_id: u64,
}

impl Layout {
    /// Construct a single-leaf layout, allocating leaf id `0` and focusing it.
    #[must_use]
    pub fn new() -> Self {
        let root_id = LeafId(0);
        Self {
            root: LayoutNode::Leaf(root_id),
            focused: root_id,
            zoomed: None,
            next_id: 1,
        }
    }

    /// Allocate the next [`LeafId`]. Deterministic and monotonic.
    pub fn alloc_id(&mut self) -> LeafId {
        let id = LeafId(self.next_id);
        self.next_id += 1;
        id
    }

    /// Number of leaves currently in the tree.
    #[must_use]
    pub fn leaf_count(&self) -> usize {
        self.root.leaf_count()
    }

    /// Every [`LeafId`] in left-to-right DFS order.
    #[must_use]
    pub fn leaves(&self) -> Vec<LeafId> {
        let mut out = Vec::new();
        self.root.collect_leaves(&mut out);
        out
    }

    /// `true` when `id` exists in the tree.
    #[must_use]
    pub fn contains(&self, id: LeafId) -> bool {
        self.root.contains(id)
    }
}

impl Default for Layout {
    fn default() -> Self {
        Self::new()
    }
}

/// Normalize a child slice's `flex` weights to sum 1.0 in place. Degenerate
/// inputs (empty, all-zero, any non-finite) fall back to a uniform split so
/// the cascade can never emit NaN or zero-area rects.
pub(crate) fn normalize_flex(children: &mut [Child]) {
    if children.is_empty() {
        return;
    }
    let n = children.len() as f32;
    let sum: f32 = children.iter().map(|c| c.flex.max(0.0)).sum();
    if !sum.is_finite() || sum <= f32::EPSILON {
        let uniform = 1.0 / n;
        for c in children.iter_mut() {
            c.flex = uniform;
        }
        return;
    }
    for c in children.iter_mut() {
        c.flex = c.flex.max(0.0) / sum;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_layout_is_single_focused_leaf() {
        let l = Layout::new();
        assert_eq!(l.leaf_count(), 1);
        assert_eq!(l.focused, LeafId(0));
        assert!(l.contains(LeafId(0)));
        assert!(!l.contains(LeafId(99)));
        assert_eq!(l.leaves(), vec![LeafId(0)]);
    }

    #[test]
    fn id_allocation_is_monotonic_and_deterministic() {
        let mut l = Layout::new();
        assert_eq!(l.alloc_id(), LeafId(1));
        assert_eq!(l.alloc_id(), LeafId(2));
        assert_eq!(l.alloc_id(), LeafId(3));
        assert_eq!(l.next_id, 4);
    }

    #[test]
    fn leaf_count_and_collect_recurse() {
        let node = LayoutNode::Split {
            axis: Axis::Horizontal,
            children: vec![
                Child::new(LayoutNode::leaf(LeafId(1)), 0.5),
                Child::new(
                    LayoutNode::Split {
                        axis: Axis::Vertical,
                        children: vec![
                            Child::new(LayoutNode::leaf(LeafId(2)), 0.5),
                            Child::new(LayoutNode::leaf(LeafId(3)), 0.5),
                        ],
                    },
                    0.5,
                ),
            ],
        };
        assert_eq!(node.leaf_count(), 3);
        let mut leaves = Vec::new();
        node.collect_leaves(&mut leaves);
        assert_eq!(leaves, vec![LeafId(1), LeafId(2), LeafId(3)]);
        assert!(node.contains(LeafId(3)));
        assert!(!node.contains(LeafId(4)));
    }

    #[test]
    fn axis_opposite() {
        assert_eq!(Axis::Horizontal.opposite(), Axis::Vertical);
        assert_eq!(Axis::Vertical.opposite(), Axis::Horizontal);
    }

    #[test]
    fn normalize_flex_sums_to_one() {
        let mut children = vec![
            Child::new(LayoutNode::leaf(LeafId(1)), 2.0),
            Child::new(LayoutNode::leaf(LeafId(2)), 6.0),
        ];
        normalize_flex(&mut children);
        let sum: f32 = children.iter().map(|c| c.flex).sum();
        assert!((sum - 1.0).abs() < 1e-6);
        assert!((children[0].flex - 0.25).abs() < 1e-6);
        assert!((children[1].flex - 0.75).abs() < 1e-6);
    }

    #[test]
    fn normalize_flex_degenerate_falls_back_uniform() {
        let mut children = vec![
            Child::new(LayoutNode::leaf(LeafId(1)), 0.0),
            Child::new(LayoutNode::leaf(LeafId(2)), 0.0),
            Child::new(LayoutNode::leaf(LeafId(3)), 0.0),
        ];
        normalize_flex(&mut children);
        for c in &children {
            assert!((c.flex - 1.0 / 3.0).abs() < 1e-6);
        }

        let mut nan_children = vec![
            Child::new(LayoutNode::leaf(LeafId(1)), f32::NAN),
            Child::new(LayoutNode::leaf(LeafId(2)), 1.0),
        ];
        normalize_flex(&mut nan_children);
        let sum: f32 = nan_children.iter().map(|c| c.flex).sum();
        assert!((sum - 1.0).abs() < 1e-6);
    }

    #[test]
    fn renormalize_children_via_node() {
        let mut node = LayoutNode::Split {
            axis: Axis::Horizontal,
            children: vec![
                Child::new(LayoutNode::leaf(LeafId(1)), 3.0),
                Child::new(LayoutNode::leaf(LeafId(2)), 1.0),
            ],
        };
        node.renormalize_children();
        if let LayoutNode::Split { children, .. } = &node {
            assert!((children[0].flex - 0.75).abs() < 1e-6);
        } else {
            panic!("expected split");
        }
        // No-op on a leaf.
        let mut leaf = LayoutNode::leaf(LeafId(5));
        leaf.renormalize_children();
        assert!(leaf.is_leaf());
    }
}
