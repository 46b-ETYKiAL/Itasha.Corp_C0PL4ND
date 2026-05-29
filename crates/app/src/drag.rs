//! Mouse drag-to-rearrange state machine + 5-zone drop targeting.
//!
//! The keyboard directional-swap (`Alt+Arrow`) shipped first in Phase 3; this
//! module is the mouse *enhancement* on top of it. A drag is armed only while
//! `Ctrl+Shift` is held (the Tabby model) and only enters the `Dragging` state
//! after the cursor travels past [`DRAG_THRESHOLD_PX`] — so a normal click never
//! gets eaten (pre-mortem #5).
//!
//! Each candidate target pane is divided into five drop zones: four edge bands
//! (Left / Right / Top / Bottom, ~25% inset) and a center. An edge zone moves
//! the dragged pane to that side of the target (a tree split); the center
//! merges the dragged pane's tabs into the target's TabGroup. The geometry is a
//! pure function of `(cursor, rect)` — [`classify_zone`] — so it is unit-tested
//! without any GPU or window.

use c0pl4nd_core::layout::{Axis, LeafId, Rect};

/// Pixels the cursor must travel from the press point before a press becomes a
/// drag. Below this, the gesture is treated as a normal click (focus / select).
pub const DRAG_THRESHOLD_PX: f64 = 6.0;

/// Fraction of a pane's extent, measured in from each edge, that counts as that
/// edge's drop band. The remaining center rectangle is the merge/center zone.
/// 0.25 → the outer quarter on each side is an edge band.
const EDGE_FRACTION: f32 = 0.25;

/// Where, within a target pane, a drop lands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DropZone {
    /// Left edge band → place the source to the left of the target.
    Left,
    /// Right edge band → place the source to the right of the target.
    Right,
    /// Top edge band → place the source above the target.
    Top,
    /// Bottom edge band → place the source below the target.
    Bottom,
    /// Center → merge the source's tabs into the target's TabGroup.
    Center,
}

impl DropZone {
    /// The split axis + side this zone implies for an edge drop, or `None` for
    /// the center (which is a merge, not a split). `before == true` means the
    /// source goes on the left/top side.
    #[must_use]
    pub fn edge_split(self) -> Option<(Axis, bool)> {
        match self {
            DropZone::Left => Some((Axis::Horizontal, true)),
            DropZone::Right => Some((Axis::Horizontal, false)),
            DropZone::Top => Some((Axis::Vertical, true)),
            DropZone::Bottom => Some((Axis::Vertical, false)),
            DropZone::Center => None,
        }
    }
}

/// The drag interaction state machine.
///
/// `Eq` is intentionally NOT derived: the `Pressed`/`Dragging` variants carry
/// `(f64, f64)` payloads and `f64` is not `Eq` (NaN ≠ NaN). `PartialEq` is
/// sufficient for the state-machine compare sites and no code requires `Eq`.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum DragState {
    /// No drag in progress.
    #[default]
    Idle,
    /// The button is down over `leaf` at `origin` (physical px) but the cursor
    /// has not yet crossed [`DRAG_THRESHOLD_PX`], so it may still be a click.
    Pressed {
        /// Press origin in physical pixels.
        origin: (f64, f64),
        /// Leaf the press landed on.
        leaf: LeafId,
    },
    /// Actively dragging `leaf`; `cursor` is the latest physical-px position.
    Dragging {
        /// The pane being dragged.
        leaf: LeafId,
        /// Latest cursor position (physical px).
        cursor: (f64, f64),
    },
}

impl DragState {
    /// Begin a potential drag: the button went down over `leaf` at `pos`.
    #[must_use]
    pub fn press(leaf: LeafId, pos: (f64, f64)) -> Self {
        DragState::Pressed { origin: pos, leaf }
    }

    /// Feed a cursor move. Promotes `Pressed → Dragging` once the move exceeds
    /// [`DRAG_THRESHOLD_PX`]; updates the cursor while `Dragging`. Returns the
    /// dragged leaf when a drag is (now) active, else `None`.
    pub fn cursor_moved(&mut self, pos: (f64, f64)) -> Option<LeafId> {
        match *self {
            DragState::Pressed { origin, leaf } => {
                let dx = pos.0 - origin.0;
                let dy = pos.1 - origin.1;
                if (dx * dx + dy * dy).sqrt() >= DRAG_THRESHOLD_PX {
                    *self = DragState::Dragging { leaf, cursor: pos };
                    Some(leaf)
                } else {
                    None
                }
            }
            DragState::Dragging { leaf, .. } => {
                *self = DragState::Dragging { leaf, cursor: pos };
                Some(leaf)
            }
            DragState::Idle => None,
        }
    }

    /// The leaf being dragged, if the drag has crossed the threshold.
    #[must_use]
    pub fn dragging_leaf(&self) -> Option<LeafId> {
        match self {
            DragState::Dragging { leaf, .. } => Some(*leaf),
            _ => None,
        }
    }

    /// The current cursor position while dragging.
    #[must_use]
    pub fn cursor(&self) -> Option<(f64, f64)> {
        match self {
            DragState::Dragging { cursor, .. } => Some(*cursor),
            _ => None,
        }
    }

    /// `true` once the gesture is a real drag (past the threshold).
    #[must_use]
    pub fn is_dragging(&self) -> bool {
        matches!(self, DragState::Dragging { .. })
    }

    /// Release the button. Returns `Some(leaf)` when a real drag was in progress
    /// (so the caller resolves a drop); `None` for a plain click. Resets to
    /// `Idle` either way.
    pub fn release(&mut self) -> Option<LeafId> {
        let dragged = self.dragging_leaf();
        *self = DragState::Idle;
        dragged
    }

    /// Abort any in-progress gesture (e.g. modifier released).
    pub fn cancel(&mut self) {
        *self = DragState::Idle;
    }
}

/// Classify where `(cx, cy)` (physical px) lands inside `rect`: one of four edge
/// bands or the center. The nearest edge wins when a corner is ambiguous (the
/// smaller normalized inset decides). Points outside `rect` clamp to the nearest
/// edge zone. Pure — no GPU / window state.
#[must_use]
pub fn classify_zone(rect: Rect, cx: i32, cy: i32) -> DropZone {
    if rect.w <= 0 || rect.h <= 0 {
        return DropZone::Center;
    }
    // Normalized position in [0, 1] within the rect (clamped).
    let nx = ((cx - rect.x) as f32 / rect.w as f32).clamp(0.0, 1.0);
    let ny = ((cy - rect.y) as f32 / rect.h as f32).clamp(0.0, 1.0);

    // Distance into each edge band; the smallest that is within EDGE_FRACTION
    // wins. Center when the point is inside the central rectangle on both axes.
    let left = nx;
    let right = 1.0 - nx;
    let top = ny;
    let bottom = 1.0 - ny;
    let min_edge = left.min(right).min(top).min(bottom);
    if min_edge >= EDGE_FRACTION {
        return DropZone::Center;
    }
    if min_edge == left {
        DropZone::Left
    } else if min_edge == right {
        DropZone::Right
    } else if min_edge == top {
        DropZone::Top
    } else {
        DropZone::Bottom
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn press_does_not_drag_below_threshold() {
        let mut s = DragState::press(LeafId(1), (100.0, 100.0));
        // A 5px move stays a click.
        assert_eq!(s.cursor_moved((103.0, 104.0)), None);
        assert!(!s.is_dragging());
        // The release of a sub-threshold gesture is a click, not a drop.
        assert_eq!(s.release(), None);
    }

    #[test]
    fn press_promotes_to_drag_past_threshold() {
        let mut s = DragState::press(LeafId(2), (100.0, 100.0));
        // A move >6px promotes to Dragging.
        assert_eq!(s.cursor_moved((110.0, 100.0)), Some(LeafId(2)));
        assert!(s.is_dragging());
        assert_eq!(s.cursor(), Some((110.0, 100.0)));
        // Release of a real drag yields the dragged leaf.
        assert_eq!(s.release(), Some(LeafId(2)));
        assert_eq!(s, DragState::Idle);
    }

    #[test]
    fn cancel_resets_to_idle() {
        let mut s = DragState::press(LeafId(1), (0.0, 0.0));
        s.cancel();
        assert_eq!(s, DragState::Idle);
        assert_eq!(s.release(), None);
    }

    #[test]
    fn zones_classify_each_edge_and_center() {
        let r = Rect::new(0, 0, 400, 400);
        // Center.
        assert_eq!(classify_zone(r, 200, 200), DropZone::Center);
        // Left band (within 25% = first 100px).
        assert_eq!(classify_zone(r, 10, 200), DropZone::Left);
        // Right band.
        assert_eq!(classify_zone(r, 390, 200), DropZone::Right);
        // Top band.
        assert_eq!(classify_zone(r, 200, 10), DropZone::Top);
        // Bottom band.
        assert_eq!(classify_zone(r, 200, 390), DropZone::Bottom);
    }

    #[test]
    fn zone_edge_split_mapping() {
        assert_eq!(DropZone::Left.edge_split(), Some((Axis::Horizontal, true)));
        assert_eq!(
            DropZone::Right.edge_split(),
            Some((Axis::Horizontal, false))
        );
        assert_eq!(DropZone::Top.edge_split(), Some((Axis::Vertical, true)));
        assert_eq!(DropZone::Bottom.edge_split(), Some((Axis::Vertical, false)));
        assert_eq!(DropZone::Center.edge_split(), None);
    }

    #[test]
    fn out_of_rect_clamps_to_an_edge() {
        let r = Rect::new(100, 100, 200, 200);
        // Far left of the rect → Left zone.
        assert_eq!(classify_zone(r, -50, 200), DropZone::Left);
        // Degenerate rect → Center (never panics).
        assert_eq!(classify_zone(Rect::new(0, 0, 0, 0), 5, 5), DropZone::Center);
    }
}
