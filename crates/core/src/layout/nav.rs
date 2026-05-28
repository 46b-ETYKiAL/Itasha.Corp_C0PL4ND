//! Directional focus navigation by geometry.
//!
//! Given the focused leaf and a [`Direction`], find the neighboring leaf the
//! user would expect when pressing e.g. `Alt+Right`. The strategy is the
//! geometry-based one used by Warp and Windows Terminal: cascade the tree,
//! then from the focused cell's edge midpoint, scan in the requested
//! direction for the nearest cell whose span overlaps the focused cell on the
//! orthogonal axis. This is robust across arbitrary nested splits (a pure
//! tree-walk gives surprising results at L-shaped boundaries).

use serde::{Deserialize, Serialize};

use super::geometry::Rect;
use super::tree::{Layout, LeafId};

/// Focus / swap movement direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Direction {
    /// Toward smaller x.
    Left,
    /// Toward larger x.
    Right,
    /// Toward smaller y.
    Up,
    /// Toward larger y.
    Down,
}

impl Layout {
    /// The leaf adjacent to `from` in `dir`, computed against `window`
    /// geometry. Returns `None` when there is no neighbor in that direction
    /// (the focused cell is already at the window edge in that axis) or when
    /// `from` is not in the tree.
    #[must_use]
    pub fn neighbor(&self, from: LeafId, dir: Direction, window: Rect) -> Option<LeafId> {
        let rects = self.cascade(window);
        let src = rects.iter().find(|(id, _)| *id == from)?.1;

        // Candidate cells lie strictly on the requested side and overlap the
        // source on the orthogonal axis. Pick the nearest by the gap to the
        // shared edge, breaking ties by orthogonal-center proximity.
        let mut best: Option<(LeafId, i32, i32)> = None;
        for (id, r) in &rects {
            if *id == from {
                continue;
            }
            let on_side = match dir {
                Direction::Left => r.x + r.w <= src.x,
                Direction::Right => r.x >= src.x + src.w,
                Direction::Up => r.y + r.h <= src.y,
                Direction::Down => r.y >= src.y + src.h,
            };
            if !on_side {
                continue;
            }
            let overlaps = match dir {
                Direction::Left | Direction::Right => overlap_1d(src.y, src.h, r.y, r.h) > 0,
                Direction::Up | Direction::Down => overlap_1d(src.x, src.w, r.x, r.w) > 0,
            };
            if !overlaps {
                continue;
            }
            let gap = match dir {
                Direction::Left => src.x - (r.x + r.w),
                Direction::Right => r.x - (src.x + src.w),
                Direction::Up => src.y - (r.y + r.h),
                Direction::Down => r.y - (src.y + src.h),
            };
            let (sc, rc) = match dir {
                Direction::Left | Direction::Right => (src.center().1, r.center().1),
                Direction::Up | Direction::Down => (src.center().0, r.center().0),
            };
            let cross = (sc - rc).abs();
            match best {
                Some((_, bgap, bcross)) if (gap, cross) >= (bgap, bcross) => {}
                _ => best = Some((*id, gap, cross)),
            }
        }
        best.map(|(id, _, _)| id)
    }

    /// Move focus in `dir` if a neighbor exists; returns the new focused leaf.
    /// A no-op (returns the current focus) when there is no neighbor.
    pub fn focus_dir(&mut self, dir: Direction, window: Rect) -> LeafId {
        if let Some(n) = self.neighbor(self.focused, dir, window) {
            self.focused = n;
        }
        self.focused
    }
}

/// Length of the overlap between segment `[a, a+al)` and `[b, b+bl)`.
fn overlap_1d(a: i32, al: i32, b: i32, bl: i32) -> i32 {
    let lo = a.max(b);
    let hi = (a + al).min(b + bl);
    (hi - lo).max(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::tree::Axis;

    /// Build a 2x2 grid: vertical split of two horizontal rows.
    /// Layout (ids): 1 2
    ///               3 4
    fn grid_2x2() -> Layout {
        use crate::layout::tree::{Child, LayoutNode};
        let mut l = Layout::new();
        let row = |a: u64, b: u64| {
            Child::new(
                LayoutNode::Split {
                    axis: Axis::Horizontal,
                    children: vec![
                        Child::new(LayoutNode::leaf(LeafId(a)), 0.5),
                        Child::new(LayoutNode::leaf(LeafId(b)), 0.5),
                    ],
                },
                0.5,
            )
        };
        l.root = LayoutNode::Split {
            axis: Axis::Vertical,
            children: vec![row(1, 2), row(3, 4)],
        };
        l.next_id = 5;
        l.focused = LeafId(1);
        l
    }

    #[test]
    fn navigate_2x2_all_directions() {
        let l = grid_2x2();
        let win = Rect::new(0, 0, 800, 600);
        // From top-left (1): right→2, down→3, left/up→None.
        assert_eq!(
            l.neighbor(LeafId(1), Direction::Right, win),
            Some(LeafId(2))
        );
        assert_eq!(l.neighbor(LeafId(1), Direction::Down, win), Some(LeafId(3)));
        assert_eq!(l.neighbor(LeafId(1), Direction::Left, win), None);
        assert_eq!(l.neighbor(LeafId(1), Direction::Up, win), None);
        // From bottom-right (4): left→3, up→2.
        assert_eq!(l.neighbor(LeafId(4), Direction::Left, win), Some(LeafId(3)));
        assert_eq!(l.neighbor(LeafId(4), Direction::Up, win), Some(LeafId(2)));
        assert_eq!(l.neighbor(LeafId(4), Direction::Right, win), None);
        assert_eq!(l.neighbor(LeafId(4), Direction::Down, win), None);
    }

    #[test]
    fn focus_dir_updates_focus() {
        let mut l = grid_2x2();
        let win = Rect::new(0, 0, 800, 600);
        assert_eq!(l.focus_dir(Direction::Right, win), LeafId(2));
        assert_eq!(l.focused, LeafId(2));
        assert_eq!(l.focus_dir(Direction::Down, win), LeafId(4));
        // No neighbor right of 4 → focus unchanged.
        assert_eq!(l.focus_dir(Direction::Right, win), LeafId(4));
    }

    #[test]
    fn navigate_1_plus_2_layout() {
        // Main pane left (1), two stacked on right (2 over 3).
        use crate::layout::tree::{Child, LayoutNode};
        let mut l = Layout::new();
        l.root = LayoutNode::Split {
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
        let win = Rect::new(0, 0, 800, 600);
        // From the big left pane, right reaches the top-right (nearest center).
        assert_eq!(
            l.neighbor(LeafId(1), Direction::Right, win),
            Some(LeafId(2))
        );
        // From 2 (top-right): left→1, down→3.
        assert_eq!(l.neighbor(LeafId(2), Direction::Left, win), Some(LeafId(1)));
        assert_eq!(l.neighbor(LeafId(2), Direction::Down, win), Some(LeafId(3)));
        // From 3 (bottom-right): up→2, left→1.
        assert_eq!(l.neighbor(LeafId(3), Direction::Up, win), Some(LeafId(2)));
        assert_eq!(l.neighbor(LeafId(3), Direction::Left, win), Some(LeafId(1)));
    }

    #[test]
    fn single_leaf_has_no_neighbors() {
        let l = Layout::new();
        let win = Rect::new(0, 0, 800, 600);
        assert_eq!(l.neighbor(LeafId(0), Direction::Right, win), None);
    }

    #[test]
    fn unknown_leaf_returns_none() {
        let l = grid_2x2();
        let win = Rect::new(0, 0, 800, 600);
        assert_eq!(l.neighbor(LeafId(99), Direction::Right, win), None);
    }

    #[test]
    fn overlap_1d_math() {
        assert_eq!(overlap_1d(0, 10, 5, 10), 5);
        assert_eq!(overlap_1d(0, 10, 10, 10), 0);
        assert_eq!(overlap_1d(0, 10, 20, 5), 0);
    }
}
