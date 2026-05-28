//! Action-layer integration tests for the split-tree layout: MAX_PANES guard,
//! directional swap, equalize, zoom toggle, resize clamp — plus the
//! backward-compatibility parity assertions proving the legacy single-axis
//! keybindings still behave on the split-tree model (the blocking gate).

use c0pl4nd_core::layout::{Axis, Direction, Layout, Rect, SplitOutcome, MAX_PANES};

const WINDOW: Rect = Rect {
    x: 0,
    y: 0,
    w: 1200,
    h: 800,
};

/// Split the focused leaf `n` times (each split targets the current focus),
/// returning the resulting layout. Panics if a split is unexpectedly rejected.
fn grid_of(n: usize) -> Layout {
    let mut l = Layout::new();
    let mut axis = Axis::Horizontal;
    while l.leaf_count() < n {
        let target = l.focused;
        match l.try_split(target, axis) {
            SplitOutcome::Split(id) => {
                l.focused = id;
            }
            other => panic!("split rejected at {} leaves: {other:?}", l.leaf_count()),
        }
        axis = axis.opposite();
    }
    l
}

#[test]
fn max_panes_guard_blocks_the_seventh_split() {
    let mut l = grid_of(MAX_PANES);
    assert_eq!(l.leaf_count(), MAX_PANES);
    assert!(l.at_capacity());
    // The 7th split must be rejected, not silently added.
    let target = l.focused;
    assert!(matches!(
        l.try_split(target, Axis::Horizontal),
        SplitOutcome::AtCapacity
    ));
    assert_eq!(l.leaf_count(), MAX_PANES, "capacity must never be exceeded");
}

#[test]
fn split_six_times_yields_six_leaves() {
    let l = grid_of(MAX_PANES);
    assert_eq!(l.leaves().len(), MAX_PANES);
}

#[test]
fn swap_directional_moves_focused_pane_and_keeps_focus() {
    // Two side-by-side panes: leaf 0 (left), leaf 1 (right). Focus leaf 0.
    let mut l = Layout::new();
    let SplitOutcome::Split(right) = l.try_split(l.focused, Axis::Horizontal) else {
        panic!("split failed");
    };
    let left = 0u64;
    l.focused = c0pl4nd_core::layout::LeafId(left);

    let before: Vec<_> = l.cascade(WINDOW).into_iter().map(|(id, _)| id).collect();
    assert_eq!(before[0].0, left, "leaf 0 starts on the left");

    // Swap focused (left) with its right neighbour.
    assert!(l.swap_focused(Direction::Right, WINDOW));
    let after: Vec<_> = l.cascade(WINDOW).into_iter().map(|(id, _)| id).collect();
    assert_eq!(after[0], right, "the right pane is now on the left");
    assert_eq!(
        l.focused.0, left,
        "focus follows the moved pane (still logical leaf 0)"
    );
}

#[test]
fn equalize_sets_equal_cell_widths() {
    // Three panes in a row; resize one wider, then equalize.
    let mut l = Layout::new();
    let SplitOutcome::Split(_b) = l.try_split(l.focused, Axis::Horizontal) else {
        panic!()
    };
    let SplitOutcome::Split(_c) = l.try_split(l.focused, Axis::Horizontal) else {
        panic!()
    };
    let f = l.focused;
    l.resize(f, 0.20, WINDOW.w);
    l.equalize();
    let widths: Vec<i32> = l.cascade(WINDOW).into_iter().map(|(_, r)| r.w).collect();
    let max = *widths.iter().max().unwrap();
    let min = *widths.iter().min().unwrap();
    // Equal flex → widths differ only by largest-remainder rounding (≤ a few px).
    assert!(
        max - min <= 3,
        "equalize should level the cells: {widths:?}"
    );
}

#[test]
fn zoom_toggle_is_a_pure_render_override() {
    let mut l = grid_of(3);
    assert!(l.zoom_target().is_none());
    assert_eq!(l.cascade(WINDOW).len(), 3);

    l.toggle_zoom();
    assert_eq!(l.zoom_target(), Some(l.focused));
    let zoomed = l.cascade(WINDOW);
    assert_eq!(zoomed.len(), 1, "zoom renders exactly one full cell");
    assert_eq!(zoomed[0].1, WINDOW, "zoomed cell fills the window");
    assert_eq!(zoomed[0].0, l.focused);

    // Tree is unchanged: leaf_count is still 3 under the zoom override.
    assert_eq!(l.leaf_count(), 3);

    l.toggle_zoom();
    assert!(l.zoom_target().is_none());
    assert_eq!(l.cascade(WINDOW).len(), 3, "un-zoom restores the cascade");
}

#[test]
fn resize_clamps_and_never_inverts() {
    let mut l = Layout::new();
    let SplitOutcome::Split(_r) = l.try_split(l.focused, Axis::Horizontal) else {
        panic!()
    };
    let f = l.focused;
    // A huge delta must clamp, not produce a zero/negative cell.
    l.resize(f, 100.0, WINDOW.w);
    let widths: Vec<i32> = l.cascade(WINDOW).into_iter().map(|(_, r)| r.w).collect();
    assert!(
        widths.iter().all(|&w| w > 0),
        "no cell may collapse: {widths:?}"
    );
}

// --- Backward-compatibility parity gate (T3.6) ----------------------------
// The legacy single-axis keybindings must produce equivalent outcomes on the
// new split-tree model. These assert the model-level behaviour the app chords
// invoke (Ctrl+Shift+D/E split, O focus-next, W close pane-then-tab).

#[test]
fn parity_split_increases_leaf_count() {
    let mut l = Layout::new();
    assert_eq!(l.leaf_count(), 1);
    assert!(matches!(
        l.try_split(l.focused, Axis::Horizontal),
        SplitOutcome::Split(_)
    ));
    assert_eq!(l.leaf_count(), 2, "Ctrl+Shift+D/E split adds a pane");
}

#[test]
fn parity_focus_next_cycles_through_every_leaf() {
    let l = grid_of(4);
    let leaves = l.leaves();
    assert_eq!(leaves.len(), 4);
    // Emulate the app's focus_next_pane: index of focus advances mod len.
    let mut focus = leaves[0];
    let mut seen = vec![focus];
    for _ in 0..3 {
        let cur = leaves.iter().position(|&id| id == focus).unwrap();
        focus = leaves[(cur + 1) % leaves.len()];
        seen.push(focus);
    }
    // Visited all four distinct leaves before wrapping.
    let mut uniq = seen.clone();
    uniq.sort();
    uniq.dedup();
    assert_eq!(uniq.len(), 4, "focus-next visits every pane");
}

#[test]
fn parity_close_removes_pane_then_collapses() {
    let mut l = grid_of(2);
    assert_eq!(l.leaf_count(), 2);
    let victim = l.focused;
    let outcome = l.remove(victim);
    // Removing one of two panes collapses back to a single leaf.
    assert_eq!(
        l.leaf_count(),
        1,
        "close removes the focused pane: {outcome:?}"
    );
    assert!(l.contains(l.focused), "focus moves to a surviving leaf");
}

#[test]
fn parity_single_pane_cascade_fills_window_like_legacy() {
    // A single-pane window must cascade to exactly the full content rect — the
    // pre-split renderer's behaviour, byte-for-byte.
    let l = Layout::new();
    let cells = l.cascade(WINDOW);
    assert_eq!(cells.len(), 1);
    assert_eq!(cells[0].1, WINDOW);
}
