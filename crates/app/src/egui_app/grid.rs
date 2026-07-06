//! Milestone 1 pane grid — `egui_tiles::Tree<Pane>` with PLACEHOLDER panes.
//!
//! Each [`Pane`] paints a colored rect + its id as a label; there is NO real
//! terminal here (that is Milestone 2, which swaps the placeholder body for a
//! glyphon paint-callback). The grid supports drag-rearrange (egui_tiles
//! native), programmatic split-right / split-down, a hard **6-pane cap**
//! enforced via clone-and-snap-back, and per-pane close.
//!
//! ## Concepts
//!
//! - [`PaneId`] — stable monotonic identifier for each pane. Survives the tree
//!   round-trip through serde as a plain integer.
//! - [`Pane`] — thin handle wrapping a `PaneId`. The heavy per-pane state
//!   (terminal/PTY/glyphon) lives in the host app, keyed by `PaneId`; the pane
//!   itself carries only the id.
//! - [`MAX_PANES`] — hard cap (six). Enforced post-frame: clone the `Tree`
//!   before each frame, snap back if `count > MAX_PANES`.

use egui_tiles::{LinearDir, TileId};
use serde::{Deserialize, Serialize};

/// Hard upper bound on simultaneously visible panes (recon dossier §4.1).
pub const MAX_PANES: usize = 6;

/// Stable, monotonically-allocated pane identifier. `#[serde(transparent)]`
/// over a `u64` newtype so it encodes as a plain integer in any persisted tree.
#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PaneId(pub u64);

impl PaneId {
    /// Pluck the integer for direct use in egui id-stack scopes.
    #[inline]
    pub fn raw(self) -> u64 {
        self.0
    }
}

/// A leaf in the `egui_tiles::Tree`. A handle into the host's per-pane state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pane {
    pub pane_id: PaneId,
}

impl Pane {
    pub fn new(pane_id: PaneId) -> Self {
        Self { pane_id }
    }
}

/// Monotonic `PaneId` allocator. Held by the app; bumped on every new pane.
#[derive(Debug, Default)]
pub struct PaneIdAllocator {
    next: u64,
}

impl PaneIdAllocator {
    /// Allocate the next id.
    pub fn next(&mut self) -> PaneId {
        let id = PaneId(self.next);
        self.next = self.next.wrapping_add(1);
        id
    }

    /// Resume allocation from a known counter — used by layout-restore so the
    /// allocator never re-issues an id already present in the restored tree.
    pub fn seeded(next: u64) -> Self {
        Self { next }
    }

    /// The next id this allocator would hand out, without advancing — captured
    /// into the layout snapshot so a restored allocator resumes past every
    /// previously-issued id.
    pub fn peek_next(&self) -> u64 {
        self.next
    }
}

/// Count the leaf panes in an `egui_tiles::Tree`. Used by the 6-pane cap
/// enforcement after `tree.ui()` runs each frame. Counts EVERY pane in the
/// tree's storage (a tab container holding N panes counts as N).
pub fn count_panes(tree: &egui_tiles::Tree<Pane>) -> usize {
    tree.tiles
        .iter()
        .filter(|(_, tile)| matches!(tile, egui_tiles::Tile::Pane(_)))
        .count()
}

/// Reset every Linear split container's shares to EQUAL, so all sibling panes in
/// each row/column become the same size. Every child share is set to 1.0; egui_tiles
/// normalises shares proportionally, so all-equal == equal sizes. Returns `true`
/// when at least one Linear container was touched (i.e. a real split existed) so
/// the caller can request a repaint only when something changed.
///
/// Only Linear containers are handled — that is the only split kind C0PL4ND ever
/// creates (`build_default_grid` / `split_focused` use horizontal/vertical Linear
/// tiles); Tabs and Grid containers (never constructed here) are left untouched.
///
/// Used by the top-bar "make symmetrical" button (one-shot) and, when the
/// `link_pane_dividers` setting is on, every frame so the dividers stay locked
/// at equal positions (drag one and they all hold equal — "move together").
pub fn equalize_pane_shares(tree: &mut egui_tiles::Tree<Pane>) -> bool {
    let mut changed = false;
    for tile in tree.tiles.tiles_mut() {
        if let egui_tiles::Tile::Container(egui_tiles::Container::Linear(linear)) = tile {
            // Only a real split (2+ siblings) has dividers to equalise; a lone
            // child is not a split, so touching it would be a no-op that wrongly
            // reports a change.
            if linear.children.len() < 2 {
                continue;
            }
            for child in linear.children.clone() {
                linear.shares.set_share(child, 1.0);
                changed = true;
            }
        }
    }
    changed
}

/// Build a default grid from a list of pane ids — every pane becomes a leaf
/// inside a single horizontal container (visible side-by-side from the start).
/// The fixed id-stack key keeps any future persistence stable across versions.
pub fn build_default_grid(panes: &[PaneId]) -> egui_tiles::Tree<Pane> {
    let mut tiles = egui_tiles::Tiles::default();
    let pane_ids: Vec<TileId> = panes
        .iter()
        .map(|p| tiles.insert_pane(Pane::new(*p)))
        .collect();
    if pane_ids.is_empty() {
        return egui_tiles::Tree::empty("c0pl4nd-grid");
    }
    let root = tiles.insert_horizontal_tile(pane_ids);
    egui_tiles::Tree::new("c0pl4nd-grid", root, tiles)
}

/// Rebuild the layout as a UNIFORM auto-grid of every pane — equal-sized cells
/// arranged in a balanced grid (egui_tiles picks the column count from the pane
/// count), preserving visual pane order. This is what the "make panes
/// symmetrical" button uses: unlike [`equalize_pane_shares`] — which only
/// equalises shares WITHIN each existing Linear split, so a nested / asymmetric
/// arrangement (e.g. one pane beside a column of three) stays visually uneven —
/// a Grid container lays every pane out in equal cells regardless of the prior
/// structure. Returns `None` (leave the tree unchanged) for a 0/1-pane tree,
/// which has nothing to gridify. Panes carry only their [`PaneId`], so rebuilding
/// preserves every attached terminal (keyed by id in `self.terms`).
pub fn rebuild_as_uniform_grid(tree: &egui_tiles::Tree<Pane>) -> Option<egui_tiles::Tree<Pane>> {
    let panes = panes_in_visual_order(tree);
    if panes.len() < 2 {
        return None;
    }
    let mut tiles = egui_tiles::Tiles::default();
    let ids: Vec<TileId> = panes
        .iter()
        .map(|p| tiles.insert_pane(Pane::new(*p)))
        .collect();
    let root = tiles.insert_grid_tile(ids);
    Some(egui_tiles::Tree::new("c0pl4nd-grid", root, tiles))
}

/// Find the `TileId` of the leaf pane whose `pane_id` matches, if present.
pub fn tile_of_pane(tree: &egui_tiles::Tree<Pane>, pane_id: PaneId) -> Option<TileId> {
    tree.tiles.iter().find_map(|(id, tile)| match tile {
        egui_tiles::Tile::Pane(p) if p.pane_id == pane_id => Some(*id),
        _ => None,
    })
}

/// Every pane's [`PaneId`] in STABLE visual order — a depth-first walk from the
/// tree root following each container's declared child order (left→right for a
/// horizontal container, top→bottom for a vertical one, declared order for tabs
/// and grids). This is the order the panes APPEAR on screen.
///
/// The raw `tree.tiles` storage is an `ahash::HashMap`, so iterating it yields a
/// DIFFERENT order every process launch — that is the reported "tab order
/// changed between launches (pane 1, pane 0)" bug. Walking the tree from the
/// root instead gives a deterministic order that matches the on-screen layout,
/// so the tab strip never reshuffles.
///
/// Panes that are in storage but NOT reachable from the root (a transient state
/// egui_tiles' `simplify` resolves next frame) are appended afterwards, sorted
/// by `PaneId`, so the result still covers every pane deterministically.
pub fn panes_in_visual_order(tree: &egui_tiles::Tree<Pane>) -> Vec<PaneId> {
    fn walk(tree: &egui_tiles::Tree<Pane>, id: TileId, out: &mut Vec<PaneId>) {
        match tree.tiles.get(id) {
            Some(egui_tiles::Tile::Pane(p)) => out.push(p.pane_id),
            Some(egui_tiles::Tile::Container(c)) => {
                for child in c.children() {
                    walk(tree, *child, out);
                }
            }
            None => {}
        }
    }
    let mut out = Vec::new();
    if let Some(root) = tree.root {
        walk(tree, root, &mut out);
    }
    // Append any storage panes not reachable from the root (sorted by id so the
    // order is stable), so a transient unreachable pane is still enumerated.
    let mut orphans: Vec<PaneId> = tree
        .tiles
        .iter()
        .filter_map(|(_, tile)| match tile {
            egui_tiles::Tile::Pane(p) if !out.contains(&p.pane_id) => Some(p.pane_id),
            _ => None,
        })
        .collect();
    orphans.sort();
    out.extend(orphans);
    out
}

/// Split the focused pane in the given direction, inserting `new_pane` next to
/// it. Returns `true` if the split was applied, `false` if it was refused
/// (because the tree is already at [`MAX_PANES`], or the focused pane is not in
/// the tree).
///
/// Strategy (recon dossier §4.4): if the focused pane's parent container is a
/// linear container already running in the requested direction, append the new
/// pane there; otherwise wrap the focused tile in a fresh linear container of
/// `[focused, new]`, swapping it into the focused tile's slot (or making it the
/// new root when the focused tile is the root).
pub fn split_focused(
    tree: &mut egui_tiles::Tree<Pane>,
    focus: PaneId,
    new_pane: PaneId,
    dir: LinearDir,
) -> bool {
    if count_panes(tree) >= MAX_PANES {
        return false;
    }
    let Some(focus_tile) = tile_of_pane(tree, focus) else {
        return false;
    };
    // Capture the focused tile's parent NOW, before we create any wrapper
    // container. Critical: `insert_container([focus_tile, new_tile])` makes
    // `focus_tile` a child of the new container WHILE it is still a child of its
    // original parent, so a later `parent_of(focus_tile)` would be ambiguous and
    // return whichever parent HashMap iteration hits first — corrupting the tree
    // (the wrapper gets orphaned, then egui_tiles' simplify GCs the new pane).
    // This was the split-down "adds no pane" bug, caught by interaction tests.
    let orig_parent = tree.tiles.parent_of(focus_tile);
    let new_tile = tree.tiles.insert_pane(Pane::new(new_pane));

    // If the focused tile's parent is a linear container of the same direction,
    // append into it — egui_tiles keeps the existing fractions.
    if let Some(parent) = orig_parent {
        if let Some(egui_tiles::Tile::Container(egui_tiles::Container::Linear(lin))) =
            tree.tiles.get(parent)
        {
            if lin.dir == dir {
                // Insert immediately after the focused tile.
                let index = lin
                    .children
                    .iter()
                    .position(|c| *c == focus_tile)
                    .map(|i| i + 1)
                    .unwrap_or(lin.children.len());
                tree.move_tile_to_container(new_tile, parent, index, false);
                return true;
            }
        }
    }

    // Otherwise wrap the focused tile in a new linear container, then relink the
    // ORIGINAL parent's slot (captured above) to point at the wrapper.
    let container = tree
        .tiles
        .insert_container(egui_tiles::Linear::new(dir, vec![focus_tile, new_tile]));
    match orig_parent {
        // The focused tile was the root: the wrapper becomes the new root.
        None => tree.root = Some(container),
        // Replace focus_tile with the wrapper in the captured parent's children.
        Some(parent) => replace_child_in_parent(tree, parent, focus_tile, container),
    }
    true
}

/// Replace child `old` with `new` in `parent`'s children. Takes the parent
/// explicitly (the caller captured it before any structural mutation) so this
/// never calls the now-ambiguous `parent_of`.
fn replace_child_in_parent(
    tree: &mut egui_tiles::Tree<Pane>,
    parent: TileId,
    old: TileId,
    new: TileId,
) {
    if let Some(egui_tiles::Tile::Container(container)) = tree.tiles.get_mut(parent) {
        match container {
            egui_tiles::Container::Linear(lin) => {
                for child in &mut lin.children {
                    if *child == old {
                        *child = new;
                        break;
                    }
                }
            }
            egui_tiles::Container::Tabs(tabs) => {
                for child in &mut tabs.children {
                    if *child == old {
                        *child = new;
                        break;
                    }
                }
            }
            egui_tiles::Container::Grid(grid) => {
                let index = grid.children().position(|c| *c == old);
                if let Some(index) = index {
                    let _ = grid.replace_at(index, new);
                }
            }
        }
    }
}

// ---- Behavior<Pane> implementation ----
//
// The host populates `PaneCallbacks` each frame with closures over its own
// state and feeds it to `tree.ui(&mut behavior, ui)`. This avoids holding
// `&mut self` across the closure (the borrow problem the dossier flagged).

/// Callbacks the grid renderer dispatches to. The host passes closures closing
/// over its own state; the `Behavior` impl below only forwards calls.
pub struct GridBehavior<'a> {
    /// `(pane_id, title)` pairs for every pane. Used by `tab_title_for_pane`.
    pub titles: &'a [(PaneId, String)],
    /// Renderer hook: paint the pane body for the given id. Returns true if the
    /// pane reported it wants to be dragged this frame.
    pub render_body: &'a mut dyn FnMut(&mut egui::Ui, PaneId) -> bool,
    /// Drained by the host after `tree.ui(...)`: ids the pane chrome requested
    /// be closed this frame.
    pub close_requests: &'a mut Vec<PaneId>,
}

impl egui_tiles::Behavior<Pane> for GridBehavior<'_> {
    fn tab_title_for_pane(&mut self, pane: &Pane) -> egui::WidgetText {
        let label = self
            .titles
            .iter()
            .find(|(id, _)| *id == pane.pane_id)
            .map(|(_, t)| t.as_str())
            .unwrap_or("(closed)");
        label.into()
    }

    fn pane_ui(
        &mut self,
        ui: &mut egui::Ui,
        _tile_id: TileId,
        pane: &mut Pane,
    ) -> egui_tiles::UiResponse {
        let drag_started = (self.render_body)(ui, pane.pane_id);
        if drag_started {
            egui_tiles::UiResponse::DragStarted
        } else {
            egui_tiles::UiResponse::None
        }
    }

    fn min_size(&self) -> f32 {
        120.0
    }

    fn gap_width(&self, _style: &egui::Style) -> f32 {
        4.0
    }

    /// The seam between panes is NEGATIVE SPACE, not a painted line.
    ///
    /// egui_tiles' default idle `resize_stroke` fills the whole `gap_width` with
    /// `tab_bar_color` = `visuals.extreme_bg_color` (the OPAQUE theme background),
    /// so the divider read as a hard dark bar that stayed solid no matter the
    /// window opacity/tint — the reported "dividers seem unaffected by tint or
    /// transparency" bug. Returning `Stroke::NONE` for the idle state paints
    /// NOTHING in the gap, so the translucent central-fill / desktop tint shows
    /// straight through it, exactly like the pane bodies. This is correct by
    /// construction across opaque, transparent, and tinted modes (the kitty
    /// `draw_minimal_borders` / i3 "gaps show the surface" model) — the seam can
    /// never be more opaque than the panes because it is never painted. Pane
    /// separation is carried by each pane's own translucency-folded bezel border
    /// (see `render_pane_body`).
    ///
    /// Hovering/Dragging keep the default widget stroke: the resize handle is a
    /// transient INTERACTION affordance that should reappear at full strength
    /// while the user is actually grabbing it (VS Code's `sash.hoverBorder`
    /// reveal-on-interaction), so it is deliberately NOT folded down.
    fn resize_stroke(&self, style: &egui::Style, state: egui_tiles::ResizeState) -> egui::Stroke {
        match state {
            egui_tiles::ResizeState::Idle => egui::Stroke::NONE,
            egui_tiles::ResizeState::Hovering => style.visuals.widgets.hovered.fg_stroke,
            egui_tiles::ResizeState::Dragging => style.visuals.widgets.active.fg_stroke,
        }
    }

    fn retain_pane(&mut self, pane: &Pane) -> bool {
        !self.close_requests.contains(&pane.pane_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pane_id_allocator_monotonic() {
        let mut a = PaneIdAllocator::default();
        let a0 = a.next();
        let a1 = a.next();
        assert!(a0.0 < a1.0);
    }

    #[test]
    fn build_default_grid_counts_panes() {
        let tree = build_default_grid(&[PaneId(0), PaneId(1), PaneId(2)]);
        assert_eq!(count_panes(&tree), 3);
    }

    #[test]
    fn build_default_grid_empty_is_empty_tree() {
        let tree = build_default_grid(&[]);
        assert_eq!(count_panes(&tree), 0);
    }

    /// Skewing a split's shares then equalising must restore all children to the
    /// SAME share (equal pane sizes) and report that it changed something.
    #[test]
    fn equalize_pane_shares_makes_all_siblings_equal() {
        let mut tree = build_default_grid(&[PaneId(0), PaneId(1), PaneId(2)]);
        // Find the root Linear container and skew its children to very unequal shares.
        let mut skewed_any = false;
        for tile in tree.tiles.tiles_mut() {
            if let egui_tiles::Tile::Container(egui_tiles::Container::Linear(lin)) = tile {
                let kids: Vec<TileId> = lin.children.clone();
                for (i, id) in kids.iter().enumerate() {
                    lin.shares.set_share(*id, 1.0 + i as f32 * 5.0); // 1, 6, 11
                    skewed_any = true;
                }
            }
        }
        assert!(
            skewed_any,
            "the 3-pane grid must contain a Linear split to skew"
        );

        assert!(
            equalize_pane_shares(&mut tree),
            "equalising a real split must report a change"
        );

        // Every Linear container's child shares must now be identical.
        for tile in tree.tiles.tiles_mut() {
            if let egui_tiles::Tile::Container(egui_tiles::Container::Linear(lin)) = tile {
                let shares: Vec<f32> = lin.shares.iter().map(|(_, s)| *s).collect();
                if let Some(first) = shares.first() {
                    assert!(
                        shares.iter().all(|s| (s - first).abs() < f32::EPSILON),
                        "all sibling shares must be equal after equalise, got {shares:?}"
                    );
                }
            }
        }
    }

    /// Equalising a single-pane tree (no split container) touches nothing and
    /// reports no change — the caller must not request a needless repaint.
    #[test]
    fn equalize_pane_shares_is_noop_without_a_split() {
        let mut tree = build_default_grid(&[PaneId(0)]);
        assert!(
            !equalize_pane_shares(&mut tree),
            "a single-pane tree has no Linear split to equalise"
        );
    }

    /// "Make symmetrical" rebuilds ANY layout — including a nested / asymmetric one
    /// (one pane beside a column of two) — into a single UNIFORM Grid container
    /// holding every pane, so all cells are equal regardless of the prior
    /// structure. This is the fix for "clicked symmetrical but panes stayed uneven"
    /// (which `equalize_pane_shares` could not fix — it only balances WITHIN each
    /// Linear split, leaving the asymmetric nesting).
    #[test]
    fn rebuild_as_uniform_grid_gridifies_every_pane() {
        let mut tiles = egui_tiles::Tiles::default();
        let a = tiles.insert_pane(Pane::new(PaneId(0)));
        let b = tiles.insert_pane(Pane::new(PaneId(1)));
        let c = tiles.insert_pane(Pane::new(PaneId(2)));
        let col = tiles.insert_vertical_tile(vec![b, c]);
        let root = tiles.insert_horizontal_tile(vec![a, col]);
        let tree = egui_tiles::Tree::new("t", root, tiles);

        let grid = rebuild_as_uniform_grid(&tree).expect("2+ panes gridify");
        assert_eq!(count_panes(&grid), 3, "every pane preserved");
        let root = grid.root.expect("grid has a root");
        assert!(
            matches!(
                grid.tiles.get(root),
                Some(egui_tiles::Tile::Container(egui_tiles::Container::Grid(_)))
            ),
            "the rebuilt root must be a uniform Grid container"
        );
        assert_eq!(
            panes_in_visual_order(&grid),
            vec![PaneId(0), PaneId(1), PaneId(2)],
            "pane order is preserved"
        );
    }

    /// A 0/1-pane tree has nothing to gridify → `None` (leave it unchanged).
    #[test]
    fn rebuild_as_uniform_grid_is_none_for_single_pane() {
        assert!(rebuild_as_uniform_grid(&build_default_grid(&[PaneId(0)])).is_none());
        assert!(rebuild_as_uniform_grid(&build_default_grid(&[])).is_none());
    }

    #[test]
    fn split_focused_adds_a_pane() {
        let mut tree = build_default_grid(&[PaneId(0)]);
        assert_eq!(count_panes(&tree), 1);
        let applied = split_focused(&mut tree, PaneId(0), PaneId(1), LinearDir::Horizontal);
        assert!(applied);
        assert_eq!(count_panes(&tree), 2);
    }

    #[test]
    fn split_focused_refuses_above_cap() {
        let ids: Vec<PaneId> = (0..MAX_PANES as u64).map(PaneId).collect();
        let mut tree = build_default_grid(&ids);
        assert_eq!(count_panes(&tree), MAX_PANES);
        let applied = split_focused(&mut tree, PaneId(0), PaneId(99), LinearDir::Vertical);
        assert!(!applied, "split must refuse at the 6-pane cap");
        assert_eq!(count_panes(&tree), MAX_PANES);
    }

    #[test]
    fn split_focused_wrap_path_adds_a_reachable_pane() {
        // The WRAP path: a 2-pane horizontal root, split one pane DOWN (vertical)
        // — direction differs from the parent, so split_focused must wrap the
        // focused tile in a fresh vertical container. The new pane must be in
        // storage AND reachable from the root (else egui_tiles' simplify GCs it).
        let mut tree = build_default_grid(&[PaneId(0), PaneId(1)]);
        assert_eq!(count_panes(&tree), 2);
        let applied = split_focused(&mut tree, PaneId(0), PaneId(2), LinearDir::Vertical);
        assert!(
            applied,
            "vertical split of a horizontal-parent pane must apply"
        );
        assert_eq!(
            count_panes(&tree),
            3,
            "wrap split must add a pane to storage"
        );

        // Reachability: walk from root; every counted pane must be reachable.
        let reachable = reachable_pane_count(&tree);
        assert_eq!(
            reachable, 3,
            "all 3 panes must be REACHABLE from the root (got {reachable}); an \
             unreachable pane is pruned by egui_tiles simplify next frame"
        );
    }

    /// Count panes reachable by walking containers from the tree root — distinct
    /// from `count_panes` (which counts raw storage). A pane in storage but not
    /// reachable is a latent bug: egui_tiles' `simplify` removes it.
    fn reachable_pane_count(tree: &egui_tiles::Tree<Pane>) -> usize {
        fn walk(tree: &egui_tiles::Tree<Pane>, id: egui_tiles::TileId, n: &mut usize) {
            match tree.tiles.get(id) {
                Some(egui_tiles::Tile::Pane(_)) => *n += 1,
                Some(egui_tiles::Tile::Container(c)) => {
                    for child in c.children() {
                        walk(tree, *child, n);
                    }
                }
                None => {}
            }
        }
        let mut n = 0;
        if let Some(root) = tree.root {
            walk(tree, root, &mut n);
        }
        n
    }

    #[test]
    fn split_focused_unknown_focus_is_noop() {
        let mut tree = build_default_grid(&[PaneId(0)]);
        let applied = split_focused(&mut tree, PaneId(42), PaneId(1), LinearDir::Horizontal);
        assert!(!applied);
        assert_eq!(count_panes(&tree), 1);
    }

    /// The 6-pane cap depends on `Tree<Pane>` cloning losslessly (clone before
    /// the frame, snap back if the count exceeds the cap).
    #[test]
    fn grid_clones_losslessly() {
        let tree = build_default_grid(&[PaneId(10), PaneId(20), PaneId(30)]);
        let snapshot = tree.clone();
        assert_eq!(count_panes(&snapshot), 3);
    }

    /// Bug-2 guard: `panes_in_visual_order` must be STABLE across repeated calls
    /// AND must match the declared left→right order — never the random
    /// `ahash::HashMap` storage order that made the tab strip reshuffle between
    /// launches. A single call cannot catch nondeterminism, so we call it many
    /// times and assert every result is identical AND equals the build order.
    #[test]
    fn panes_in_visual_order_is_stable_and_matches_layout() {
        let ids = [PaneId(0), PaneId(1), PaneId(2), PaneId(3)];
        let tree = build_default_grid(&ids);
        let first = panes_in_visual_order(&tree);
        assert_eq!(
            first,
            ids.to_vec(),
            "visual order must be the declared left→right order"
        );
        // Repeated calls must be byte-identical — a HashMap-iteration result
        // would vary run-to-run (and often call-to-call within a run).
        for _ in 0..50 {
            assert_eq!(
                panes_in_visual_order(&tree),
                first,
                "pane visual order must be deterministic across calls"
            );
        }
    }

    /// A fresh `Tree` built from the SAME ids in a NEW process-like allocation
    /// yields the SAME visual order — the property the tab strip relies on so it
    /// does not reshuffle "pane 1, pane 0" between launches.
    #[test]
    fn panes_in_visual_order_stable_across_rebuilds() {
        let ids = [PaneId(7), PaneId(8), PaneId(9)];
        let a = panes_in_visual_order(&build_default_grid(&ids));
        let b = panes_in_visual_order(&build_default_grid(&ids));
        assert_eq!(
            a, b,
            "two builds of the same grid must enumerate identically"
        );
        assert_eq!(a, ids.to_vec());
    }

    /// After a vertical split (the wrap path), the new pane must appear in the
    /// visual order in its tree position — and the order stays stable.
    #[test]
    fn panes_in_visual_order_covers_split_panes() {
        let mut tree = build_default_grid(&[PaneId(0), PaneId(1)]);
        assert!(split_focused(
            &mut tree,
            PaneId(0),
            PaneId(2),
            LinearDir::Vertical
        ));
        let order = panes_in_visual_order(&tree);
        assert_eq!(order.len(), 3, "all three panes enumerated");
        // Determinism still holds post-split.
        for _ in 0..20 {
            assert_eq!(panes_in_visual_order(&tree), order);
        }
        // Every pane present exactly once.
        for id in [PaneId(0), PaneId(1), PaneId(2)] {
            assert_eq!(
                order.iter().filter(|p| **p == id).count(),
                1,
                "pane {id:?} must appear exactly once"
            );
        }
    }
}
