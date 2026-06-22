//! Pixel-rect cascade for the split tree.
//!
//! Given a root rectangle, [`Layout::cascade`] computes each leaf's absolute
//! pixel rect from the flex ratios, inserting an integer-pixel gutter between
//! siblings. The cascade is a pure function of `(tree, window_rect)` — same
//! input always yields the same output (no RNG, no time dependence) — so the
//! renderer can cache the result and recompute only on layout change or
//! resize. When the layout is zoomed, the focused-or-zoomed leaf occupies the
//! whole window rect and all siblings are omitted.

use serde::{Deserialize, Serialize};

use super::tree::{Axis, Layout, LayoutNode, LeafId};

/// Integer-pixel gutter inserted between sibling cells. Doubles as the
/// resize hit-target band at the render layer.
pub const GUTTER: i32 = 1;

/// Minimum pixel extent (width or height) a cell may occupy along the split
/// axis before the resize clamp refuses to shrink it further.
pub const MIN_CELL: i32 = 24;

/// An axis-aligned integer-pixel rectangle. Top-left origin (`x`/`y` grow
/// right/down), matching the winit/wgpu surface coordinate convention.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rect {
    /// Left edge.
    pub x: i32,
    /// Top edge.
    pub y: i32,
    /// Width in pixels (>= 0).
    pub w: i32,
    /// Height in pixels (>= 0).
    pub h: i32,
}

impl Rect {
    /// Construct a rect.
    #[must_use]
    pub fn new(x: i32, y: i32, w: i32, h: i32) -> Self {
        Self { x, y, w, h }
    }

    /// Area in square pixels.
    #[must_use]
    pub fn area(&self) -> i64 {
        i64::from(self.w.max(0)) * i64::from(self.h.max(0))
    }

    /// `true` when `(px, py)` lies inside the rect (left/top inclusive,
    /// right/bottom exclusive).
    #[must_use]
    pub fn contains_point(&self, px: i32, py: i32) -> bool {
        px >= self.x && px < self.x + self.w && py >= self.y && py < self.y + self.h
    }

    /// The extent of this rect along `axis`.
    #[must_use]
    pub fn extent(&self, axis: Axis) -> i32 {
        match axis {
            Axis::Horizontal => self.w,
            Axis::Vertical => self.h,
        }
    }

    /// Center point of the rect.
    #[must_use]
    pub fn center(&self) -> (i32, i32) {
        (self.x + self.w / 2, self.y + self.h / 2)
    }
}

impl Layout {
    /// Compute each leaf's absolute pixel rect for `window`.
    ///
    /// When the layout is zoomed (`zoomed.is_some()` and the id still exists),
    /// the single zoomed leaf is returned at the full `window` rect. Otherwise
    /// the tree is cascaded with integer gutters between siblings.
    #[must_use]
    pub fn cascade(&self, window: Rect) -> Vec<(LeafId, Rect)> {
        if let Some(zid) = self.zoomed {
            if self.contains(zid) {
                return vec![(zid, window)];
            }
        }
        let mut out = Vec::with_capacity(self.leaf_count());
        cascade_node(&self.root, window, &mut out);
        out
    }
}

/// Recursively cascade `node` into `rect`, pushing `(LeafId, Rect)` pairs.
fn cascade_node(node: &LayoutNode, rect: Rect, out: &mut Vec<(LeafId, Rect)>) {
    match node {
        LayoutNode::Leaf(id) => out.push((*id, rect)),
        LayoutNode::Split { axis, children } => {
            let n = children.len();
            if n == 0 {
                return;
            }
            let total_gutter = GUTTER * (n as i32 - 1);
            let avail = (rect.extent(*axis) - total_gutter).max(0);

            // Distribute `avail` by flex with largest-remainder rounding so the
            // child extents sum EXACTLY to `avail` (no off-by-one drift, the
            // gutters + cells exactly tile the parent rect).
            let extents = distribute(avail, children.iter().map(|c| c.flex));

            let mut cursor = match axis {
                Axis::Horizontal => rect.x,
                Axis::Vertical => rect.y,
            };
            for (child, ext) in children.iter().zip(extents.iter()) {
                let child_rect = match axis {
                    Axis::Horizontal => Rect::new(cursor, rect.y, *ext, rect.h),
                    Axis::Vertical => Rect::new(rect.x, cursor, rect.w, *ext),
                };
                cascade_node(&child.node, child_rect, out);
                cursor += ext + GUTTER;
            }
        }
    }
}

/// Apportion `total` pixels across weights using the largest-remainder
/// (Hamilton) method, guaranteeing the parts sum to exactly `total`.
fn distribute(total: i32, weights: impl Iterator<Item = f32>) -> Vec<i32> {
    let weights: Vec<f32> = weights.map(|w| w.max(0.0)).collect();
    let n = weights.len();
    if n == 0 {
        return Vec::new();
    }
    let sum: f32 = weights.iter().sum();
    let total_f = total.max(0) as f32;

    // Ideal (fractional) share per weight.
    let ideal: Vec<f32> = if sum > f32::EPSILON {
        weights.iter().map(|w| w / sum * total_f).collect()
    } else {
        vec![total_f / n as f32; n]
    };

    // Floor each, track remainders, hand out the leftover one pixel at a time
    // to the largest remainders.
    let mut parts: Vec<i32> = ideal.iter().map(|f| f.floor() as i32).collect();
    let assigned: i32 = parts.iter().sum();
    let mut leftover = total.max(0) - assigned;

    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by(|&a, &b| {
        let ra = ideal[a] - ideal[a].floor();
        let rb = ideal[b] - ideal[b].floor();
        rb.partial_cmp(&ra).unwrap_or(std::cmp::Ordering::Equal)
    });
    let mut oi = 0;
    while leftover > 0 && n > 0 {
        parts[order[oi % n]] += 1;
        leftover -= 1;
        oi += 1;
    }
    parts
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::tree::{Child, TabGroup};

    fn split(axis: Axis, children: Vec<Child>) -> LayoutNode {
        LayoutNode::Split { axis, children }
    }

    #[test]
    fn single_leaf_fills_window() {
        let l = Layout::new();
        let rects = l.cascade(Rect::new(0, 0, 800, 600));
        assert_eq!(rects, vec![(LeafId(0), Rect::new(0, 0, 800, 600))]);
    }

    #[test]
    fn horizontal_half_split_no_gutter_effect_on_sum() {
        // Two 0.5 children over 800 wide, GUTTER=1 → 799 usable → 400 + 399.
        let mut l = Layout::new();
        l.root = split(
            Axis::Horizontal,
            vec![
                Child::new(LayoutNode::leaf(LeafId(1)), 0.5),
                Child::new(LayoutNode::leaf(LeafId(2)), 0.5),
            ],
        );
        let rects = l.cascade(Rect::new(0, 0, 800, 600));
        assert_eq!(rects.len(), 2);
        // Widths + 1px gutter exactly tile 800.
        let total: i32 = rects.iter().map(|(_, r)| r.w).sum::<i32>() + GUTTER;
        assert_eq!(total, 800);
        // Both full height.
        for (_, r) in &rects {
            assert_eq!(r.h, 600);
            assert_eq!(r.y, 0);
        }
        // Second cell starts after first + gutter.
        assert_eq!(rects[1].1.x, rects[0].1.w + GUTTER);
    }

    #[test]
    fn nested_2x3_yields_six_cells_tiling_the_window() {
        // Vertical split into 3 rows, each a horizontal split of 2 → 2x3 grid.
        let mut next = 1u64;
        let mut alloc = || {
            let id = LeafId(next);
            next += 1;
            id
        };
        let row = |a: LeafId, b: LeafId| {
            Child::new(
                split(
                    Axis::Horizontal,
                    vec![
                        Child::new(LayoutNode::leaf(a), 0.5),
                        Child::new(LayoutNode::leaf(b), 0.5),
                    ],
                ),
                1.0 / 3.0,
            )
        };
        let (a, b, c, d, e, f) = (alloc(), alloc(), alloc(), alloc(), alloc(), alloc());
        let mut l = Layout::new();
        l.root = split(Axis::Vertical, vec![row(a, b), row(c, d), row(e, f)]);

        let win = Rect::new(0, 0, 1200, 900);
        let rects = l.cascade(win);
        assert_eq!(rects.len(), 6);

        // Every cell non-empty.
        for (_, r) in &rects {
            assert!(r.w > 0 && r.h > 0, "empty cell {r:?}");
        }
        // Cells exactly tile: sum of areas + gutter bands == window area
        // is hard to assert directly with cross gutters, so verify columns
        // and rows tile their axes.
        // Row heights + 2 gutters == 900.
        let row_heights: Vec<i32> = vec![rects[0].1.h, rects[2].1.h, rects[4].1.h];
        assert_eq!(row_heights.iter().sum::<i32>() + 2 * GUTTER, 900);
        // Column widths in row 0 + 1 gutter == 1200.
        assert_eq!(rects[0].1.w + rects[1].1.w + GUTTER, 1200);
    }

    #[test]
    fn zoom_returns_single_full_rect() {
        let mut l = Layout::new();
        l.root = split(
            Axis::Horizontal,
            vec![
                Child::new(LayoutNode::leaf(LeafId(1)), 0.5),
                Child::new(LayoutNode::leaf(LeafId(2)), 0.5),
            ],
        );
        l.zoomed = Some(LeafId(2));
        let win = Rect::new(0, 0, 800, 600);
        let rects = l.cascade(win);
        assert_eq!(rects, vec![(LeafId(2), win)]);

        // Stale zoom id falls through to the normal cascade.
        l.zoomed = Some(LeafId(999));
        assert_eq!(l.cascade(win).len(), 2);
    }

    #[test]
    fn distribute_sums_exactly() {
        // 7 px across three equal weights → 3 + 2 + 2 (largest remainder).
        let parts = distribute(7, [1.0, 1.0, 1.0].into_iter());
        assert_eq!(parts.iter().sum::<i32>(), 7);
        assert_eq!(parts.len(), 3);
        // 100 px across 0.7 / 0.3.
        let parts = distribute(100, [0.7, 0.3].into_iter());
        assert_eq!(parts.iter().sum::<i32>(), 100);
        assert_eq!(parts, vec![70, 30]);
    }

    #[test]
    fn distribute_empty_weights_is_empty() {
        let parts = distribute(100, std::iter::empty::<f32>());
        assert!(parts.is_empty(), "no weights → no parts");
    }

    #[test]
    fn distribute_zero_weight_sum_splits_evenly() {
        // All-zero weights hit the `sum <= EPSILON` branch: even split, exact sum.
        let parts = distribute(10, [0.0, 0.0, 0.0].into_iter());
        assert_eq!(parts.len(), 3);
        assert_eq!(parts.iter().sum::<i32>(), 10, "still tiles exactly");
        // Even-ish: 4 + 3 + 3 via the largest-remainder leftover hand-out.
        let mut sorted = parts.clone();
        sorted.sort_unstable();
        assert_eq!(sorted, vec![3, 3, 4]);
    }

    #[test]
    fn distribute_negative_weights_are_floored_to_zero() {
        // Negative weights clamp to 0; the lone positive weight takes all of it.
        let parts = distribute(50, [-1.0, 2.0].into_iter());
        assert_eq!(parts.iter().sum::<i32>(), 50);
        assert_eq!(parts, vec![0, 50]);
    }

    #[test]
    fn cascade_empty_split_pushes_nothing() {
        // A split with zero children (degenerate, never produced by the engine
        // but defended against) cascades to no cells, never panics.
        let mut l = Layout::new();
        l.root = split(Axis::Horizontal, vec![]);
        let rects = l.cascade(Rect::new(0, 0, 800, 600));
        assert!(rects.is_empty(), "empty split yields no cells");
    }

    #[test]
    fn cascade_clamps_when_gutters_exceed_extent() {
        // Two children in a 1px-wide window: total_gutter (1) == extent, so
        // avail clamps to 0 and both cells get zero width without underflow.
        let mut l = Layout::new();
        l.root = split(
            Axis::Horizontal,
            vec![
                Child::new(LayoutNode::leaf(LeafId(1)), 0.5),
                Child::new(LayoutNode::leaf(LeafId(2)), 0.5),
            ],
        );
        let rects = l.cascade(Rect::new(0, 0, 1, 600));
        assert_eq!(rects.len(), 2);
        for (_, r) in &rects {
            assert!(
                r.w >= 0,
                "no negative width under tight gutter clamp: {r:?}"
            );
        }
    }

    #[test]
    fn rect_helpers() {
        let r = Rect::new(10, 20, 100, 50);
        assert_eq!(r.area(), 5000);
        assert!(r.contains_point(10, 20));
        assert!(r.contains_point(109, 69));
        assert!(!r.contains_point(110, 20));
        assert!(!r.contains_point(9, 20));
        assert_eq!(r.extent(Axis::Horizontal), 100);
        assert_eq!(r.extent(Axis::Vertical), 50);
        assert_eq!(r.center(), (60, 45));
    }

    #[test]
    fn cascade_respects_flex_ratio() {
        let mut l = Layout::new();
        l.root = split(
            Axis::Horizontal,
            vec![
                Child::new(LayoutNode::leaf(LeafId(1)), 0.75),
                Child::new(LayoutNode::leaf(LeafId(2)), 0.25),
            ],
        );
        // 401 usable after gutter; 0.75 → ~300, 0.25 → ~100.
        let rects = l.cascade(Rect::new(0, 0, 402, 100));
        // first ≈ 3× second.
        assert!(rects[0].1.w > rects[1].1.w * 2);
        // Construct a TabGroup to exercise the type from this module's scope.
        let _tg = TabGroup::new(LeafId(1), 0);
    }
}
