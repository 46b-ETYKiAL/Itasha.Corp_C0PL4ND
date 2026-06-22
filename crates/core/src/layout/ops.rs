//! Structural tree edits: split, remove (with parent collapse + flex
//! redistribution), and resize. All operations preserve the invariant that
//! every `Split` has >= 2 children and that sibling `flex` weights sum to 1.0.

use super::geometry::MIN_CELL;
use super::tree::{normalize_flex, Axis, Child, Layout, LayoutNode, LeafId};

/// Outcome of a [`Layout::remove`] call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoveOutcome {
    /// Target leaf was not found in the tree.
    NotFound,
    /// Target was the only leaf; the tree is unchanged (cannot remove the
    /// last pane — the window always has at least one cell).
    LastLeaf,
    /// Leaf removed; the parent split kept >= 2 children.
    Removed,
    /// Leaf removed and the parent split collapsed into its sole remaining
    /// child (which was hoisted up into the grandparent).
    Collapsed,
}

impl Layout {
    /// Split `target` along `axis`, inserting a new leaf with id `new_leaf`.
    ///
    /// If `target`'s parent split already runs along `axis`, the new leaf is
    /// inserted as a sibling immediately after `target` (sharing `target`'s
    /// flex equally). Otherwise `target`'s slot is replaced by a fresh
    /// 2-child split `{ target, new_leaf }` along `axis`. Returns `true` on
    /// success, `false` if `target` was not found.
    ///
    /// Note: this is the structural primitive — the action layer (Phase 3)
    /// owns the `MAX_PANES` guard. This method itself does not enforce a cap.
    pub fn split(&mut self, target: LeafId, axis: Axis, new_leaf: LeafId) -> bool {
        if !self.contains(target) {
            return false;
        }
        // Special case: target IS the root leaf → replace root with a split.
        if matches!(self.root, LayoutNode::Leaf(id) if id == target) {
            self.root = LayoutNode::Split {
                axis,
                children: vec![
                    Child::new(LayoutNode::leaf(target), 0.5),
                    Child::new(LayoutNode::leaf(new_leaf), 0.5),
                ],
            };
            return true;
        }
        split_in(&mut self.root, target, axis, new_leaf)
    }

    /// Remove `leaf` from the tree, collapsing a now-single-child parent into
    /// its grandparent and redistributing the freed flex among siblings.
    ///
    /// Focus moves to the first remaining leaf when the focused leaf was the
    /// one removed. The last remaining leaf cannot be removed.
    pub fn remove(&mut self, leaf: LeafId) -> RemoveOutcome {
        if !self.contains(leaf) {
            return RemoveOutcome::NotFound;
        }
        if self.leaf_count() == 1 {
            return RemoveOutcome::LastLeaf;
        }
        // Root is a split (guaranteed, since leaf_count > 1).
        let outcome = remove_in(&mut self.root, leaf);
        // After removal the root split may itself be a single child → hoist.
        if let LayoutNode::Split { children, .. } = &self.root {
            if children.len() == 1 {
                let only = self.root_take_single_child();
                self.root = only;
            }
        }
        if self.zoomed == Some(leaf) {
            self.zoomed = None;
        }
        if self.focused == leaf {
            self.focused = self.leaves().first().copied().unwrap_or(self.focused);
        }
        outcome
    }

    /// Hoist the root split's single remaining child up to become the root.
    fn root_take_single_child(&mut self) -> LayoutNode {
        if let LayoutNode::Split { children, .. } = &mut self.root {
            if children.len() == 1 {
                return children.remove(0).node;
            }
        }
        // Unreachable in practice; return a clone to stay total.
        self.root.clone()
    }

    /// Adjust the flex split between `leaf` and its next sibling along the
    /// parent's axis by `delta` (a fraction, e.g. `+0.05`). Clamps so neither
    /// the leaf nor its sibling shrinks below [`MIN_CELL`] given `axis_extent`
    /// (the parent's pixel extent along the split axis), then renormalizes.
    /// Returns `true` if a resize was applied.
    pub fn resize(&mut self, leaf: LeafId, delta: f32, axis_extent: i32) -> bool {
        resize_in(&mut self.root, leaf, delta, axis_extent)
    }

    /// Set every sibling group along the tree to equal flex (the "equalize"
    /// action). Walks the whole tree.
    pub fn equalize(&mut self) {
        equalize_in(&mut self.root);
    }

    /// Move `source` so it becomes a sibling of `target` along `axis`, on the
    /// `before` side (true = left/up, false = right/down). This is the tree edit
    /// behind an edge-zone drag-drop: the source pane is detached from wherever
    /// it sat and re-attached next to the drop target.
    ///
    /// Returns `true` when the move applied. No-ops (returns `false`) when
    /// `source == target`, either id is missing, or `source` is the only leaf.
    /// `source` keeps its [`LeafId`], so the app-side session mapping survives.
    pub fn move_leaf(&mut self, source: LeafId, target: LeafId, axis: Axis, before: bool) -> bool {
        if source == target || !self.contains(source) || !self.contains(target) {
            return false;
        }
        if self.leaf_count() <= 1 {
            return false;
        }
        // Detach the source. (Collapsing a parent is fine — target keeps its id,
        // which is all we look up next.)
        if matches!(
            self.remove(source),
            RemoveOutcome::NotFound | RemoveOutcome::LastLeaf
        ) {
            return false;
        }
        if !self.contains(target) {
            // Pathological: target vanished (cannot happen for distinct leaves in
            // a >=2-leaf tree, but stay total). Re-attach source at the root so
            // no pane is lost.
            self.reattach_at_root(source, axis, before);
            return true;
        }
        // Split the target with a placeholder, then rename the placeholder to the
        // real source id and (for `before`) swap it ahead of the target.
        let placeholder = self.alloc_id();
        if !self.split(target, axis, placeholder) {
            self.reattach_at_root(source, axis, before);
            return true;
        }
        rename_leaf(&mut self.root, placeholder, source);
        if before {
            reorder_before(&mut self.root, source, target);
        }
        self.focused = source;
        true
    }

    /// Re-attach `leaf` as a sibling of the current root along `axis`, used as a
    /// safety net when a move's target disappears. Never loses the pane.
    fn reattach_at_root(&mut self, leaf: LeafId, axis: Axis, before: bool) {
        let old_root = std::mem::replace(&mut self.root, LayoutNode::leaf(leaf));
        let (a, b) = if before {
            (LayoutNode::leaf(leaf), old_root)
        } else {
            (old_root, LayoutNode::leaf(leaf))
        };
        self.root = LayoutNode::Split {
            axis,
            children: vec![Child::new(a, 0.5), Child::new(b, 0.5)],
        };
        self.focused = leaf;
    }
}

/// Rename every occurrence of leaf id `from` to `to` in the subtree.
fn rename_leaf(node: &mut LayoutNode, from: LeafId, to: LeafId) {
    match node {
        LayoutNode::Leaf(id) => {
            if *id == from {
                *id = to;
            }
        }
        LayoutNode::Split { children, .. } => {
            for c in children.iter_mut() {
                rename_leaf(&mut c.node, from, to);
            }
        }
    }
}

/// Where `moved` and `anchor` are adjacent leaf children of the same split,
/// ensure `moved` sits immediately before `anchor` (swapping the two child
/// slots when `split` placed `moved` after `anchor`).
fn reorder_before(node: &mut LayoutNode, moved: LeafId, anchor: LeafId) {
    if let LayoutNode::Split { children, .. } = node {
        let mi = children
            .iter()
            .position(|c| matches!(c.node, LayoutNode::Leaf(id) if id == moved));
        let ai = children
            .iter()
            .position(|c| matches!(c.node, LayoutNode::Leaf(id) if id == anchor));
        if let (Some(mi), Some(ai)) = (mi, ai) {
            if mi > ai {
                children.swap(mi, ai);
            }
            return;
        }
        for c in children.iter_mut() {
            reorder_before(&mut c.node, moved, anchor);
        }
    }
}

/// Recursive split: find `target` among `node`'s direct children and act.
fn split_in(node: &mut LayoutNode, target: LeafId, axis: Axis, new_leaf: LeafId) -> bool {
    let LayoutNode::Split {
        axis: parent_axis,
        children,
    } = node
    else {
        return false;
    };
    // Is `target` a direct leaf child here?
    if let Some(idx) = children
        .iter()
        .position(|c| matches!(c.node, LayoutNode::Leaf(id) if id == target))
    {
        if *parent_axis == axis {
            // Same axis: insert sibling after target, splitting target's flex.
            let half = children[idx].flex * 0.5;
            children[idx].flex = half;
            children.insert(idx + 1, Child::new(LayoutNode::leaf(new_leaf), half));
            normalize_flex(children);
        } else {
            // Different axis: replace target leaf with a nested 2-child split.
            let flex = children[idx].flex;
            children[idx] = Child::new(
                LayoutNode::Split {
                    axis,
                    children: vec![
                        Child::new(LayoutNode::leaf(target), 0.5),
                        Child::new(LayoutNode::leaf(new_leaf), 0.5),
                    ],
                },
                flex,
            );
        }
        return true;
    }
    // Recurse into child splits.
    for c in children.iter_mut() {
        if c.node.contains(target) && split_in(&mut c.node, target, axis, new_leaf) {
            return true;
        }
    }
    false
}

/// Recursive remove: delete `leaf` from `node`'s subtree, collapsing a
/// single-child split into its remaining child.
fn remove_in(node: &mut LayoutNode, leaf: LeafId) -> RemoveOutcome {
    let LayoutNode::Split { children, .. } = node else {
        return RemoveOutcome::NotFound;
    };
    if let Some(idx) = children
        .iter()
        .position(|c| matches!(c.node, LayoutNode::Leaf(id) if id == leaf))
    {
        children.remove(idx);
        if children.len() == 1 {
            // Collapse: hoist the single remaining child up in place of this
            // split. Caller replaces *node.
            let only = children.remove(0).node;
            *node = only;
            return RemoveOutcome::Collapsed;
        }
        normalize_flex(children);
        return RemoveOutcome::Removed;
    }
    // Recurse.
    for c in children.iter_mut() {
        if c.node.contains(leaf) {
            let outcome = remove_in(&mut c.node, leaf);
            if outcome != RemoveOutcome::NotFound {
                // If the child split collapsed, its node was rewritten in
                // place; renormalize this level's flex defensively.
                if let LayoutNode::Split { children: cc, .. } = node {
                    normalize_flex(cc);
                }
                return outcome;
            }
        }
    }
    RemoveOutcome::NotFound
}

/// Recursive resize: find the split whose direct child is `leaf`, then shift
/// flex from the next sibling to `leaf` (or vice versa on negative delta).
fn resize_in(node: &mut LayoutNode, leaf: LeafId, delta: f32, axis_extent: i32) -> bool {
    let LayoutNode::Split { children, .. } = node else {
        return false;
    };
    if let Some(idx) = children
        .iter()
        .position(|c| matches!(c.node, LayoutNode::Leaf(id) if id == leaf))
    {
        // Resize against the next sibling; if `leaf` is last, use the previous.
        let (a, b) = if idx + 1 < children.len() {
            (idx, idx + 1)
        } else if idx > 0 {
            (idx - 1, idx)
        } else {
            return false; // single child — nothing to resize against
        };
        let min_frac = if axis_extent > 0 {
            (MIN_CELL as f32 / axis_extent as f32).min(0.45)
        } else {
            0.05
        };
        let new_a = (children[a].flex + delta).clamp(min_frac, 1.0 - min_frac);
        let actual = new_a - children[a].flex;
        children[a].flex += actual;
        children[b].flex -= actual;
        normalize_flex(children);
        return true;
    }
    for c in children.iter_mut() {
        if c.node.contains(leaf) && resize_in(&mut c.node, leaf, delta, axis_extent) {
            return true;
        }
    }
    false
}

/// Recursively set equal flex among every split's children.
fn equalize_in(node: &mut LayoutNode) {
    if let LayoutNode::Split { children, .. } = node {
        let n = children.len() as f32;
        for c in children.iter_mut() {
            c.flex = 1.0 / n;
            equalize_in(&mut c.node);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flex_sum_ok(node: &LayoutNode) -> bool {
        match node {
            LayoutNode::Leaf(_) => true,
            LayoutNode::Split { children, .. } => {
                let sum: f32 = children.iter().map(|c| c.flex).sum();
                (sum - 1.0).abs() < 1e-4
                    && children.len() >= 2
                    && children.iter().all(|c| flex_sum_ok(&c.node))
            }
        }
    }

    #[test]
    fn split_root_leaf_creates_two_child_split() {
        let mut l = Layout::new();
        let new = l.alloc_id();
        assert!(l.split(LeafId(0), Axis::Horizontal, new));
        assert_eq!(l.leaf_count(), 2);
        assert!(l.contains(new));
        assert!(flex_sum_ok(&l.root));
    }

    #[test]
    fn split_same_axis_inserts_sibling() {
        let mut l = Layout::new();
        let b = l.alloc_id();
        l.split(LeafId(0), Axis::Horizontal, b);
        let c = l.alloc_id();
        // Split `b` along the SAME axis → 3 children in one split.
        assert!(l.split(b, Axis::Horizontal, c));
        assert_eq!(l.leaf_count(), 3);
        if let LayoutNode::Split { children, axis } = &l.root {
            assert_eq!(*axis, Axis::Horizontal);
            assert_eq!(children.len(), 3, "expected n-ary same-axis insert");
        } else {
            panic!("root should be a split");
        }
        assert!(flex_sum_ok(&l.root));
    }

    #[test]
    fn split_diff_axis_nests_new_branch() {
        let mut l = Layout::new();
        let b = l.alloc_id();
        l.split(LeafId(0), Axis::Horizontal, b);
        let c = l.alloc_id();
        // Split `b` along the OTHER axis → nested split, root stays 2 children.
        assert!(l.split(b, Axis::Vertical, c));
        assert_eq!(l.leaf_count(), 3);
        if let LayoutNode::Split { children, .. } = &l.root {
            assert_eq!(children.len(), 2);
            assert!(matches!(children[1].node, LayoutNode::Split { .. }));
        } else {
            panic!("root should be a split");
        }
        assert!(flex_sum_ok(&l.root));
    }

    #[test]
    fn split_unknown_target_is_noop() {
        let mut l = Layout::new();
        assert!(!l.split(LeafId(42), Axis::Horizontal, LeafId(99)));
        assert_eq!(l.leaf_count(), 1);
    }

    #[test]
    fn remove_collapses_parent() {
        let mut l = Layout::new();
        let b = l.alloc_id();
        l.split(LeafId(0), Axis::Horizontal, b);
        // Remove b → collapse back to a single leaf (the root leaf).
        assert_eq!(l.remove(b), RemoveOutcome::Collapsed);
        assert_eq!(l.leaf_count(), 1);
        assert!(matches!(l.root, LayoutNode::Leaf(LeafId(0))));
        assert_eq!(l.focused, LeafId(0));
    }

    #[test]
    fn remove_keeps_split_when_three_children() {
        let mut l = Layout::new();
        let b = l.alloc_id();
        let c = l.alloc_id();
        l.split(LeafId(0), Axis::Horizontal, b);
        l.split(b, Axis::Horizontal, c); // 3-child split
        assert_eq!(l.remove(c), RemoveOutcome::Removed);
        assert_eq!(l.leaf_count(), 2);
        assert!(flex_sum_ok(&l.root));
    }

    #[test]
    fn remove_last_leaf_refused() {
        let mut l = Layout::new();
        assert_eq!(l.remove(LeafId(0)), RemoveOutcome::LastLeaf);
        assert_eq!(l.leaf_count(), 1);
    }

    #[test]
    fn remove_not_found() {
        let mut l = Layout::new();
        assert_eq!(l.remove(LeafId(123)), RemoveOutcome::NotFound);
    }

    #[test]
    fn remove_focused_moves_focus() {
        let mut l = Layout::new();
        let b = l.alloc_id();
        l.split(LeafId(0), Axis::Horizontal, b);
        l.focused = b;
        l.remove(b);
        assert_eq!(l.focused, LeafId(0));
    }

    #[test]
    fn remove_clears_stale_zoom() {
        let mut l = Layout::new();
        let b = l.alloc_id();
        l.split(LeafId(0), Axis::Horizontal, b);
        l.zoomed = Some(b);
        l.remove(b);
        assert_eq!(l.zoomed, None);
    }

    #[test]
    fn resize_shifts_flex_and_clamps() {
        let mut l = Layout::new();
        let b = l.alloc_id();
        l.split(LeafId(0), Axis::Horizontal, b);
        // Grow leaf 0 by 0.2 over an 800px extent.
        assert!(l.resize(LeafId(0), 0.2, 800));
        if let LayoutNode::Split { children, .. } = &l.root {
            assert!((children[0].flex - 0.7).abs() < 1e-4);
            assert!((children[1].flex - 0.3).abs() < 1e-4);
        }
        assert!(flex_sum_ok(&l.root));

        // A huge delta is clamped so neither cell drops below MIN_CELL.
        let mut l2 = Layout::new();
        let b2 = l2.alloc_id();
        l2.split(LeafId(0), Axis::Horizontal, b2);
        l2.resize(LeafId(0), 5.0, 200);
        if let LayoutNode::Split { children, .. } = &l2.root {
            assert!(children[1].flex >= MIN_CELL as f32 / 200.0 - 1e-4);
        }
        assert!(flex_sum_ok(&l2.root));
    }

    #[test]
    fn resize_last_child_uses_previous_sibling() {
        let mut l = Layout::new();
        let b = l.alloc_id();
        l.split(LeafId(0), Axis::Horizontal, b);
        // Resize the LAST child (b) — should pair with the previous (0).
        assert!(l.resize(b, 0.1, 800));
        assert!(flex_sum_ok(&l.root));
    }

    #[test]
    fn equalize_resets_to_uniform() {
        let mut l = Layout::new();
        let b = l.alloc_id();
        let c = l.alloc_id();
        l.split(LeafId(0), Axis::Horizontal, b);
        l.split(b, Axis::Horizontal, c); // 3 children, lopsided after inserts
        l.resize(LeafId(0), 0.2, 800);
        l.equalize();
        if let LayoutNode::Split { children, .. } = &l.root {
            for ch in children {
                assert!((ch.flex - 1.0 / children.len() as f32).abs() < 1e-4);
            }
        }
        assert!(flex_sum_ok(&l.root));
    }

    /// Collect the direct-child leaf ids of the root split, in order. Panics if
    /// the root is not a split (the test asserts shape first).
    fn root_child_leaf_ids(l: &Layout) -> Vec<Option<LeafId>> {
        match &l.root {
            LayoutNode::Split { children, .. } => children
                .iter()
                .map(|c| match c.node {
                    LayoutNode::Leaf(id) => Some(id),
                    _ => None,
                })
                .collect(),
            LayoutNode::Leaf(_) => panic!("root is a leaf, not a split"),
        }
    }

    #[test]
    fn resize_single_root_leaf_is_noop() {
        // A bare root leaf has no split to resize against — returns false.
        let mut l = Layout::new();
        assert!(!l.resize(LeafId(0), 0.2, 800));
    }

    #[test]
    fn resize_unknown_leaf_is_noop() {
        let mut l = Layout::new();
        let b = l.alloc_id();
        l.split(LeafId(0), Axis::Horizontal, b);
        assert!(!l.resize(LeafId(404), 0.2, 800));
    }

    #[test]
    fn resize_recurses_into_nested_split() {
        // 0 | (b / c)  — resize c, which lives in the nested vertical split.
        let mut l = Layout::new();
        let b = l.alloc_id();
        l.split(LeafId(0), Axis::Horizontal, b);
        let c = l.alloc_id();
        l.split(b, Axis::Vertical, c); // nested split { b, c }
        assert!(
            l.resize(c, 0.1, 600),
            "resize must recurse into the nested split"
        );
        assert!(flex_sum_ok(&l.root));
    }

    #[test]
    fn resize_zero_axis_extent_uses_floor_min_frac() {
        // axis_extent <= 0 takes the 0.05 floor branch rather than MIN_CELL/extent.
        let mut l = Layout::new();
        let b = l.alloc_id();
        l.split(LeafId(0), Axis::Horizontal, b);
        // Huge delta with zero extent clamps to 1.0 - 0.05 = 0.95 on leaf 0.
        assert!(l.resize(LeafId(0), 5.0, 0));
        if let LayoutNode::Split { children, .. } = &l.root {
            assert!(
                (children[0].flex - 0.95).abs() < 1e-4,
                "got {}",
                children[0].flex
            );
            assert!(
                (children[1].flex - 0.05).abs() < 1e-4,
                "got {}",
                children[1].flex
            );
        } else {
            panic!("root should be a split");
        }
    }

    #[test]
    fn split_recurses_into_nested_branch() {
        // Build 0 | (b / c); splitting c (a nested leaf) exercises the recursive
        // `split_in` branch, not the root special-case.
        let mut l = Layout::new();
        let b = l.alloc_id();
        l.split(LeafId(0), Axis::Horizontal, b);
        let c = l.alloc_id();
        l.split(b, Axis::Vertical, c); // nested { b, c }
        let d = l.alloc_id();
        assert!(l.split(c, Axis::Vertical, d), "must recurse to find c");
        assert_eq!(l.leaf_count(), 4);
        assert!(l.contains(d));
        assert!(flex_sum_ok(&l.root));
    }

    #[test]
    fn remove_recurses_and_renormalizes_parent() {
        // 0 | (b / c / e): removing one of b/c/e keeps the nested split (3→2)
        // and exercises the recursive remove_in + parent renormalize path.
        let mut l = Layout::new();
        let b = l.alloc_id();
        l.split(LeafId(0), Axis::Horizontal, b);
        let c = l.alloc_id();
        l.split(b, Axis::Vertical, c);
        let e = l.alloc_id();
        l.split(c, Axis::Vertical, e); // nested vertical { b, c, e }
        assert_eq!(l.leaf_count(), 4);
        assert_eq!(l.remove(c), RemoveOutcome::Removed);
        assert_eq!(l.leaf_count(), 3);
        // Root still 2 children: leaf 0 + the (now 2-child) nested split.
        assert_eq!(root_child_leaf_ids(&l).len(), 2);
        assert!(flex_sum_ok(&l.root));
    }

    #[test]
    fn remove_nested_collapse_hoists_sibling_then_renormalizes_root() {
        // 0 | (b / c): removing b collapses the nested split to a bare leaf c,
        // which is hoisted into the root split. Root stays a 2-child split {0, c}.
        let mut l = Layout::new();
        let b = l.alloc_id();
        l.split(LeafId(0), Axis::Horizontal, b);
        let c = l.alloc_id();
        l.split(b, Axis::Vertical, c); // nested { b, c }
        assert_eq!(l.remove(b), RemoveOutcome::Collapsed);
        assert_eq!(l.leaf_count(), 2);
        let ids = root_child_leaf_ids(&l);
        assert_eq!(
            ids,
            vec![Some(LeafId(0)), Some(c)],
            "c hoisted in place of the nested split"
        );
        assert!(flex_sum_ok(&l.root));
    }

    #[test]
    fn remove_root_split_collapse_hoists_to_root_leaf() {
        // A 2-child root split → removing one child hoists the survivor to be
        // the bare root leaf (the root_take_single_child path).
        let mut l = Layout::new();
        let b = l.alloc_id();
        l.split(LeafId(0), Axis::Horizontal, b);
        assert_eq!(l.remove(LeafId(0)), RemoveOutcome::Collapsed);
        assert!(matches!(l.root, LayoutNode::Leaf(id) if id == b));
        assert_eq!(l.focused, b, "focus moved to the surviving leaf");
    }

    #[test]
    fn move_leaf_noop_when_source_equals_target() {
        let mut l = Layout::new();
        let b = l.alloc_id();
        l.split(LeafId(0), Axis::Horizontal, b);
        assert!(!l.move_leaf(b, b, Axis::Horizontal, false));
        assert_eq!(l.leaf_count(), 2);
    }

    #[test]
    fn move_leaf_noop_when_either_id_missing() {
        let mut l = Layout::new();
        let b = l.alloc_id();
        l.split(LeafId(0), Axis::Horizontal, b);
        assert!(!l.move_leaf(LeafId(404), b, Axis::Horizontal, false));
        assert!(!l.move_leaf(b, LeafId(404), Axis::Horizontal, false));
        assert_eq!(l.leaf_count(), 2);
    }

    #[test]
    fn move_leaf_noop_when_single_leaf() {
        let mut l = Layout::new();
        // Only one leaf: nothing to move (the source==target guard catches the
        // self case; this asserts the leaf_count<=1 guard too via a distinct id).
        assert!(!l.move_leaf(LeafId(0), LeafId(0), Axis::Horizontal, false));
        assert_eq!(l.leaf_count(), 1);
    }

    #[test]
    fn move_leaf_after_target_reattaches_with_stable_id() {
        // 0 | b | c (3-child horizontal). Move b to sit AFTER c.
        let mut l = Layout::new();
        let b = l.alloc_id();
        l.split(LeafId(0), Axis::Horizontal, b);
        let c = l.alloc_id();
        l.split(b, Axis::Horizontal, c); // 0 | b | c
        assert_eq!(l.leaves(), vec![LeafId(0), b, c]);

        assert!(l.move_leaf(b, c, Axis::Horizontal, false));
        // b keeps its id and now sits AFTER c in DFS order.
        let leaves = l.leaves();
        assert_eq!(leaves.len(), 3);
        assert!(leaves.contains(&b) && leaves.contains(&c) && leaves.contains(&LeafId(0)));
        let bi = leaves.iter().position(|&x| x == b).unwrap();
        let ci = leaves.iter().position(|&x| x == c).unwrap();
        assert!(bi > ci, "b must sit after c: {leaves:?}");
        assert_eq!(l.focused, b, "focus follows the moved pane");
        assert!(flex_sum_ok(&l.root));
    }

    #[test]
    fn move_leaf_before_target_swaps_ahead() {
        // 0 | b | c. Move c to sit BEFORE 0 — exercises reorder_before's swap.
        let mut l = Layout::new();
        let b = l.alloc_id();
        l.split(LeafId(0), Axis::Horizontal, b);
        let c = l.alloc_id();
        l.split(b, Axis::Horizontal, c); // 0 | b | c

        assert!(l.move_leaf(c, LeafId(0), Axis::Horizontal, true));
        let leaves = l.leaves();
        let ci = leaves.iter().position(|&x| x == c).unwrap();
        let zi = leaves.iter().position(|&x| x == LeafId(0)).unwrap();
        assert!(ci < zi, "c must sit before 0: {leaves:?}");
        assert_eq!(l.focused, c);
        assert!(flex_sum_ok(&l.root));
    }

    #[test]
    fn move_leaf_to_other_axis_nests() {
        // 0 | b. Move 0 below b (different axis) → nested vertical split.
        let mut l = Layout::new();
        let b = l.alloc_id();
        l.split(LeafId(0), Axis::Horizontal, b);
        assert!(l.move_leaf(LeafId(0), b, Axis::Vertical, false));
        assert_eq!(l.leaf_count(), 2);
        assert!(l.contains(LeafId(0)) && l.contains(b));
        assert_eq!(l.focused, LeafId(0));
        assert!(flex_sum_ok(&l.root));
    }

    #[test]
    fn rename_leaf_recurses_into_nested_split() {
        // rename_leaf is also exercised by move_leaf, but assert the recursive
        // arm directly via a deeply-nested move.
        let mut l = Layout::new();
        let b = l.alloc_id();
        l.split(LeafId(0), Axis::Horizontal, b);
        let c = l.alloc_id();
        l.split(b, Axis::Vertical, c); // 0 | (b / c)
        let d = l.alloc_id();
        l.split(c, Axis::Vertical, d); // 0 | (b / c / d)
                                       // Move the deeply-nested d next to 0 — forces rename into the nested split.
        assert!(l.move_leaf(d, LeafId(0), Axis::Horizontal, false));
        assert!(l.contains(d), "d kept its id through the nested rename");
        assert_eq!(l.leaf_count(), 4);
        assert!(flex_sum_ok(&l.root));
    }

    #[test]
    fn move_leaf_clears_stale_zoom_on_detach() {
        // Moving the source removes-then-reattaches it; if it was zoomed, the
        // intervening remove clears the zoom (zoom is keyed on the detached id).
        let mut l = Layout::new();
        let b = l.alloc_id();
        l.split(LeafId(0), Axis::Horizontal, b);
        let c = l.alloc_id();
        l.split(b, Axis::Horizontal, c);
        l.zoomed = Some(b);
        assert!(l.move_leaf(b, c, Axis::Horizontal, false));
        assert_eq!(l.zoomed, None, "the detach-remove cleared the stale zoom");
    }

    #[test]
    fn equalize_recurses_into_nested_splits() {
        // 0 | (b / c / d), all lopsided → equalize sets EVERY split uniform.
        let mut l = Layout::new();
        let b = l.alloc_id();
        l.split(LeafId(0), Axis::Horizontal, b);
        let c = l.alloc_id();
        l.split(b, Axis::Vertical, c);
        let d = l.alloc_id();
        l.split(c, Axis::Vertical, d);
        l.resize(LeafId(0), 0.3, 800); // skew root
        l.resize(c, 0.2, 600); // skew nested
        l.equalize();
        // Root: 2 children → 0.5 each.
        if let LayoutNode::Split { children, .. } = &l.root {
            for ch in children {
                assert!((ch.flex - 0.5).abs() < 1e-4, "root child flex {}", ch.flex);
            }
            // The nested split's children are all 1/3.
            for ch in children {
                if let LayoutNode::Split {
                    children: nested, ..
                } = &ch.node
                {
                    for nc in nested {
                        assert!(
                            (nc.flex - 1.0 / 3.0).abs() < 1e-4,
                            "nested flex {}",
                            nc.flex
                        );
                    }
                }
            }
        } else {
            panic!("root should be a split");
        }
        assert!(flex_sum_ok(&l.root));
    }

    #[test]
    fn property_random_split_remove_keeps_tree_valid() {
        // Deterministic pseudo-random sequence (LCG) — no external rng dep.
        let mut l = Layout::new();
        let mut state: u64 = 0x1234_5678_9abc_def0;
        let mut next = || {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
            (state >> 33) as u32
        };
        for _ in 0..200 {
            let leaves = l.leaves();
            let pick = leaves[(next() as usize) % leaves.len()];
            if next() % 2 == 0 && l.leaf_count() < 12 {
                let id = l.alloc_id();
                let axis = if next() % 2 == 0 {
                    Axis::Horizontal
                } else {
                    Axis::Vertical
                };
                l.split(pick, axis, id);
            } else {
                l.remove(pick);
            }
            assert!(flex_sum_ok(&l.root), "invariant broken: {:?}", l.root);
            assert!(l.leaf_count() >= 1);
            assert!(l.contains(l.focused), "focus dangling");
        }
    }
}
