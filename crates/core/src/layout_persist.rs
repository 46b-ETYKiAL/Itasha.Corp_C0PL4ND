//! Layout persistence: a serde view of the split tree plus per-leaf tab
//! metadata, with safe save / load and a never-crash fallback.
//!
//! The live [`Layout`](crate::layout::Layout) tracks structure and ids; it does
//! NOT own shells, working directories, or profiles (the engine is UI-free).
//! Persistence needs the *intent* of each cell — where to launch and which tab
//! was active — so a saved workspace can be reopened with **fresh** shells.
//! Live process state (scrollback, child PIDs, env) is deliberately NOT
//! persisted: reopening a workspace spawns new PTYs per leaf, matching the
//! "restore the shape, not the session" contract (tmux-server-grade live
//! restore is explicitly out of scope).
//!
//! # Format
//!
//! A [`LayoutSnapshot`] is JSON: a recursive [`NodeView`] tree (split axis +
//! flex-weighted children, or a leaf carrying [`LeafView`] tab metadata) plus
//! the focused-leaf ordinal. The format is structural only — there is no code,
//! no command string to execute, no path the loader will run. Loading reads
//! data, never instructions (STRIDE Tampering / Elevation review: the worst a
//! hostile file can do is request a `cwd` that does not exist, which the app
//! falls back from per [`LeafView::cwd`]).
//!
//! # Safety (pre-mortem #6)
//!
//! [`LayoutSnapshot::load`] never panics and never trusts the file blindly:
//! malformed JSON, a tree exceeding [`MAX_PANES`], an empty tree, a
//! non-finite/zero flex sum, or a structurally-broken split all resolve to a
//! single-pane fallback (logged at WARN), so a corrupt workspace file can
//! degrade the UI to one shell but can never crash it or wedge the grid.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::layout::{Axis, Child, Layout, LayoutNode, LeafId, TabGroup, MAX_PANES};

/// Per-leaf launch metadata captured for persistence. Mirrors one grid cell:
/// where its tabs should launch (`cwd`), which shell profile to use
/// (`profile`), and which tab was active.
///
/// `cwd` / `profile` are `Option`s: `None` means "use the app default" (the
/// configured shell, the process working directory). A persisted `cwd` that no
/// longer exists is the app's concern to fall back from — the format only
/// records the user's intent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeafView {
    /// Number of tabs this cell held. Always `>= 1` for a valid leaf; the
    /// loader clamps a zero to one so a restored cell always has a shell.
    pub tab_count: usize,
    /// Index (into the cell's tabs) of the tab that was visible. Clamped into
    /// range on load.
    pub active: usize,
    /// Working directory each fresh shell in this cell should launch in.
    /// `None` = the app default. Stored as a plain string (no path the loader
    /// executes — it is handed to the app, which may ignore a missing dir).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// Named shell profile for this cell's tabs. `None` = the configured
    /// default shell.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
}

impl LeafView {
    /// A single-tab leaf with no cwd/profile override (the default cell).
    #[must_use]
    pub fn single() -> Self {
        LeafView {
            tab_count: 1,
            active: 0,
            cwd: None,
            profile: None,
        }
    }

    /// A leaf carrying explicit launch intent.
    #[must_use]
    pub fn new(
        tab_count: usize,
        active: usize,
        cwd: Option<String>,
        profile: Option<String>,
    ) -> Self {
        LeafView {
            tab_count,
            active,
            cwd,
            profile,
        }
    }
}

/// A node in the persisted tree: either a split (axis + weighted children) or a
/// leaf carrying its [`LeafView`] metadata.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum NodeView {
    /// An internal split.
    Split {
        /// Split orientation.
        axis: Axis,
        /// Weighted children (the loader requires `>= 2`).
        children: Vec<ChildView>,
    },
    /// A terminal cell with its launch metadata.
    Leaf {
        /// The cell's tab/cwd/profile intent.
        view: LeafView,
    },
}

/// A weighted child inside a [`NodeView::Split`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChildView {
    /// Fractional share of the parent's extent along the split axis.
    pub flex: f32,
    /// The subtree under this child.
    pub node: NodeView,
}

/// A complete persisted layout: the structural tree plus the focused leaf's
/// ordinal (its 0-based position in left-to-right DFS leaf order).
///
/// Carries a `version` so a future format change can be detected and migrated
/// rather than mis-parsed. Unknown future versions are rejected on load (→
/// single-pane fallback), never silently mis-read.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LayoutSnapshot {
    /// Format version. Current is [`LayoutSnapshot::VERSION`].
    pub version: u32,
    /// Root of the persisted tree.
    pub root: NodeView,
    /// 0-based DFS ordinal of the focused leaf. Clamped on load.
    #[serde(default)]
    pub focused_ordinal: usize,
}

/// Why a saved layout could not be restored. Every variant degrades the UI to
/// a single pane; none ever panics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoadError {
    /// The file was missing or unreadable.
    Io(String),
    /// The JSON did not parse.
    Parse(String),
    /// The file's format version is not understood.
    UnsupportedVersion(u32),
    /// The tree was empty, exceeded [`MAX_PANES`], or was otherwise invalid.
    Invalid(String),
}

impl std::fmt::Display for LoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LoadError::Io(e) => write!(f, "layout file io error: {e}"),
            LoadError::Parse(e) => write!(f, "layout file parse error: {e}"),
            LoadError::UnsupportedVersion(v) => write!(f, "unsupported layout version {v}"),
            LoadError::Invalid(m) => write!(f, "invalid layout: {m}"),
        }
    }
}

impl std::error::Error for LoadError {}

/// The result of loading a workspace: a reconstructed [`Layout`] (fresh ids,
/// no live sessions) plus the per-leaf [`LeafView`] metadata in DFS order, so
/// the app can spawn one fresh shell per leaf with the right cwd/profile.
#[derive(Debug, Clone, PartialEq)]
pub struct RestoredLayout {
    /// The structural layout, ready to drive the renderer.
    pub layout: Layout,
    /// Per-leaf launch metadata, parallel to `layout.leaves()` (DFS order).
    pub leaves: Vec<(LeafId, LeafView)>,
}

impl RestoredLayout {
    /// The trivial single-pane workspace (used as the safe fallback).
    #[must_use]
    pub fn single_pane() -> Self {
        let layout = Layout::new();
        let focused = layout.focused;
        RestoredLayout {
            layout,
            leaves: vec![(focused, LeafView::single())],
        }
    }
}

impl LayoutSnapshot {
    /// Current persistence format version.
    pub const VERSION: u32 = 1;

    /// Build a snapshot from a live [`Layout`] and a per-leaf metadata lookup.
    ///
    /// `meta` is called for each [`LeafId`] in the tree; return the cell's
    /// [`LeafView`] (tab count, active index, cwd, profile). The app supplies
    /// this from its session store. The focused leaf's DFS ordinal is recorded.
    pub fn capture(layout: &Layout, mut meta: impl FnMut(LeafId) -> LeafView) -> Self {
        let leaves = layout.leaves();
        let focused_ordinal = leaves
            .iter()
            .position(|&id| id == layout.focused)
            .unwrap_or(0);
        LayoutSnapshot {
            version: Self::VERSION,
            root: node_view(&layout.root, &mut meta),
            focused_ordinal,
        }
    }

    /// Serialize to pretty JSON. Deterministic for a fixed snapshot (no maps,
    /// no time, no RNG) so saved files are byte-stable.
    pub fn to_json(&self) -> Result<String, LoadError> {
        serde_json::to_string_pretty(self).map_err(|e| LoadError::Parse(e.to_string()))
    }

    /// Parse from JSON, validating structure and the [`MAX_PANES`] cap. Does
    /// NOT reconstruct a [`Layout`] — use [`LayoutSnapshot::restore`] for that.
    pub fn from_json(src: &str) -> Result<Self, LoadError> {
        let snap: LayoutSnapshot =
            serde_json::from_str(src).map_err(|e| LoadError::Parse(e.to_string()))?;
        snap.validate()?;
        Ok(snap)
    }

    /// Validate the snapshot: known version, non-empty, within [`MAX_PANES`],
    /// every split has `>= 2` children, every flex is finite and non-negative.
    pub fn validate(&self) -> Result<(), LoadError> {
        if self.version != Self::VERSION {
            return Err(LoadError::UnsupportedVersion(self.version));
        }
        let n = count_leaves(&self.root)?;
        if n == 0 {
            return Err(LoadError::Invalid("layout has no panes".into()));
        }
        if n > MAX_PANES {
            return Err(LoadError::Invalid(format!(
                "layout has {n} panes, exceeds MAX_PANES ({MAX_PANES})"
            )));
        }
        Ok(())
    }

    /// Reconstruct a live [`Layout`] (fresh, deterministic ids) plus the DFS
    /// per-leaf metadata. Fails (→ caller falls back) on an invalid snapshot.
    pub fn restore(&self) -> Result<RestoredLayout, LoadError> {
        self.validate()?;
        let mut next_id = 0u64;
        let mut leaves: Vec<(LeafId, LeafView)> = Vec::new();
        let root = build_node(&self.root, &mut next_id, &mut leaves);
        if leaves.is_empty() {
            return Err(LoadError::Invalid("layout produced no leaves".into()));
        }
        let focused = leaves
            .get(self.focused_ordinal.min(leaves.len() - 1))
            .map(|(id, _)| *id)
            .unwrap_or(leaves[0].0);
        let layout = Layout {
            root,
            focused,
            zoomed: None,
            next_id,
        };
        Ok(RestoredLayout { layout, leaves })
    }

    /// Write the snapshot to `path` as pretty JSON, creating parent dirs.
    pub fn save(&self, path: &Path) -> Result<(), LoadError> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).map_err(|e| LoadError::Io(e.to_string()))?;
            }
        }
        let json = self.to_json()?;
        std::fs::write(path, json).map_err(|e| LoadError::Io(e.to_string()))
    }
}

/// Save a live [`Layout`] to `path`, capturing per-leaf metadata via `meta`.
/// A convenience wrapper over [`LayoutSnapshot::capture`] + `save`.
pub fn save(
    layout: &Layout,
    path: &Path,
    meta: impl FnMut(LeafId) -> LeafView,
) -> Result<(), LoadError> {
    LayoutSnapshot::capture(layout, meta).save(path)
}

/// Load and reconstruct a layout from `path`. **Never panics**: a missing,
/// unreadable, malformed, over-cap, or otherwise-invalid file logs a WARN and
/// returns the single-pane fallback (pre-mortem #6). The `Result` form is
/// available via [`load_strict`] when the caller wants to distinguish a real
/// failure from an absent file.
#[must_use]
pub fn load(path: &Path) -> RestoredLayout {
    match load_strict(path) {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!("layout restore failed ({e}); falling back to a single pane");
            RestoredLayout::single_pane()
        }
    }
}

/// Load and reconstruct a layout from `path`, returning the [`LoadError`] on
/// failure instead of the silent fallback. Used by tests and by callers that
/// want to surface a corrupt-file message.
pub fn load_strict(path: &Path) -> Result<RestoredLayout, LoadError> {
    let src = std::fs::read_to_string(path).map_err(|e| LoadError::Io(e.to_string()))?;
    let snap = LayoutSnapshot::from_json(&src)?;
    snap.restore()
}

// --- internal recursion helpers -------------------------------------------

/// Recursively build a [`NodeView`] from a live [`LayoutNode`], pulling each
/// leaf's metadata from `meta`.
fn node_view(node: &LayoutNode, meta: &mut impl FnMut(LeafId) -> LeafView) -> NodeView {
    match node {
        LayoutNode::Leaf(id) => NodeView::Leaf { view: meta(*id) },
        LayoutNode::Split { axis, children } => NodeView::Split {
            axis: *axis,
            children: children
                .iter()
                .map(|c| ChildView {
                    flex: c.flex,
                    node: node_view(&c.node, meta),
                })
                .collect(),
        },
    }
}

/// Count leaves in a [`NodeView`] while validating split arity (`>= 2`
/// children) and flex finiteness. Returns an error on a structurally-broken
/// tree.
fn count_leaves(node: &NodeView) -> Result<usize, LoadError> {
    match node {
        NodeView::Leaf { view } => {
            if !view.tab_count_is_sane() {
                return Err(LoadError::Invalid("leaf has zero tabs".into()));
            }
            Ok(1)
        }
        NodeView::Split { children, .. } => {
            if children.len() < 2 {
                return Err(LoadError::Invalid("split has fewer than 2 children".into()));
            }
            for c in children {
                if !c.flex.is_finite() || c.flex < 0.0 {
                    return Err(LoadError::Invalid("non-finite or negative flex".into()));
                }
            }
            let mut total = 0usize;
            for c in children {
                total += count_leaves(&c.node)?;
            }
            Ok(total)
        }
    }
}

/// Recursively build a live [`LayoutNode`] from a validated [`NodeView`],
/// allocating fresh deterministic ids and collecting per-leaf metadata in DFS
/// order. Flex weights are normalized per split so the cascade stays valid even
/// if the file's weights drifted.
fn build_node(
    node: &NodeView,
    next_id: &mut u64,
    leaves: &mut Vec<(LeafId, LeafView)>,
) -> LayoutNode {
    match node {
        NodeView::Leaf { view } => {
            let id = LeafId(*next_id);
            *next_id += 1;
            let mut v = view.clone();
            v.normalize();
            leaves.push((id, v));
            LayoutNode::Leaf(id)
        }
        NodeView::Split { axis, children } => {
            let mut kids: Vec<Child> = children
                .iter()
                .map(|c| {
                    let child_node = build_node(&c.node, next_id, leaves);
                    Child::new(child_node, c.flex.max(0.0))
                })
                .collect();
            let mut split = LayoutNode::Split {
                axis: *axis,
                children: std::mem::take(&mut kids),
            };
            split.renormalize_children();
            split
        }
    }
}

impl LeafView {
    /// `true` when the leaf claims at least one tab.
    fn tab_count_is_sane(&self) -> bool {
        self.tab_count >= 1
    }

    /// Clamp `tab_count`/`active` into a usable range (`tab_count >= 1`,
    /// `active < tab_count`) so a restored cell always has a visible shell.
    fn normalize(&mut self) {
        if self.tab_count == 0 {
            self.tab_count = 1;
        }
        if self.active >= self.tab_count {
            self.active = self.tab_count - 1;
        }
    }
}

/// Build the per-leaf [`LeafView`] for a [`TabGroup`] given an optional cwd and
/// profile. A small helper the app can use when capturing a snapshot.
#[must_use]
pub fn leaf_view_for(group: &TabGroup, cwd: Option<String>, profile: Option<String>) -> LeafView {
    LeafView {
        tab_count: group.len().max(1),
        active: group.active.min(group.len().saturating_sub(1)),
        cwd,
        profile,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::{Rect, SplitOutcome};

    /// Build a 2x2-ish layout via the guarded action layer.
    fn grid_of(n: usize) -> Layout {
        let mut l = Layout::new();
        let mut axis = Axis::Horizontal;
        while l.leaf_count() < n {
            let t = l.focused;
            if let SplitOutcome::Split(id) = l.try_split(t, axis) {
                l.focused = id;
            }
            axis = axis.opposite();
        }
        l
    }

    #[test]
    fn round_trip_is_structurally_equal_and_byte_stable() {
        let l = grid_of(4);
        let snap = LayoutSnapshot::capture(&l, |_| LeafView::single());

        let json = snap.to_json().expect("serialize");
        let back = LayoutSnapshot::from_json(&json).expect("deserialize");
        assert_eq!(snap, back, "snapshot round-trip must be structurally equal");

        // Byte-stable: re-serializing the decoded snapshot is identical.
        let json2 = back.to_json().expect("reserialize");
        assert_eq!(json, json2, "serde output must be byte-stable");
    }

    #[test]
    fn restore_rebuilds_an_equivalent_cascade() {
        let l = grid_of(4);
        let win = Rect::new(0, 0, 1200, 900);
        let before = l.cascade(win);

        let snap = LayoutSnapshot::capture(&l, |_| LeafView::single());
        let restored = snap.restore().expect("restore");
        let after = restored.layout.cascade(win);

        // Same number of cells and same rectangles (ids are freshly allocated
        // but the geometry is identical).
        assert_eq!(before.len(), after.len());
        let before_rects: Vec<_> = before.iter().map(|(_, r)| *r).collect();
        let after_rects: Vec<_> = after.iter().map(|(_, r)| *r).collect();
        assert_eq!(before_rects, after_rects);
        assert_eq!(restored.leaves.len(), 4);
    }

    #[test]
    fn restore_preserves_focused_ordinal() {
        let mut l = grid_of(3);
        // Focus the last leaf in DFS order.
        let last = *l.leaves().last().unwrap();
        l.focused = last;
        let snap = LayoutSnapshot::capture(&l, |_| LeafView::single());
        assert_eq!(snap.focused_ordinal, 2);

        let restored = snap.restore().expect("restore");
        let restored_last = *restored.layout.leaves().last().unwrap();
        assert_eq!(restored.layout.focused, restored_last);
    }

    #[test]
    fn restore_carries_cwd_and_profile_metadata() {
        let mut l = Layout::new();
        let _ = l.try_split(l.focused, Axis::Horizontal);
        let leaves = l.leaves();
        let snap = LayoutSnapshot::capture(&l, |id| {
            if id == leaves[0] {
                LeafView::new(2, 1, Some("/home/op".into()), Some("powershell".into()))
            } else {
                LeafView::single()
            }
        });
        let restored = snap.restore().expect("restore");
        let first = &restored.leaves[0].1;
        assert_eq!(first.tab_count, 2);
        assert_eq!(first.active, 1);
        assert_eq!(first.cwd.as_deref(), Some("/home/op"));
        assert_eq!(first.profile.as_deref(), Some("powershell"));
    }

    #[test]
    fn corrupt_json_strict_errors_and_load_falls_back() {
        let tmp = std::env::temp_dir().join(format!("c0pl4nd-corrupt-{}.json", std::process::id()));
        std::fs::write(&tmp, "this is { not ] valid json").unwrap();

        // Strict surfaces the parse error.
        let err = load_strict(&tmp).unwrap_err();
        assert!(matches!(err, LoadError::Parse(_)), "got {err:?}");

        // The safe loader degrades to a single pane, never panics.
        let restored = load(&tmp);
        assert_eq!(restored.layout.leaf_count(), 1);
        assert_eq!(restored.leaves.len(), 1);

        let _ = std::fs::remove_file(&tmp);
    }

    #[test]
    fn over_cap_tree_is_rejected() {
        // Hand-build a 7-leaf snapshot (over MAX_PANES) bypassing the action guard.
        let mut children = Vec::new();
        for _ in 0..7 {
            children.push(ChildView {
                flex: 1.0 / 7.0,
                node: NodeView::Leaf {
                    view: LeafView::single(),
                },
            });
        }
        let snap = LayoutSnapshot {
            version: LayoutSnapshot::VERSION,
            root: NodeView::Split {
                axis: Axis::Horizontal,
                children,
            },
            focused_ordinal: 0,
        };
        let err = snap.validate().unwrap_err();
        assert!(matches!(err, LoadError::Invalid(_)), "got {err:?}");
        // restore also refuses the over-cap tree.
        assert!(snap.restore().is_err());
    }

    #[test]
    fn invalid_flex_sum_is_rejected() {
        let snap = LayoutSnapshot {
            version: LayoutSnapshot::VERSION,
            root: NodeView::Split {
                axis: Axis::Horizontal,
                children: vec![
                    ChildView {
                        flex: f32::NAN,
                        node: NodeView::Leaf {
                            view: LeafView::single(),
                        },
                    },
                    ChildView {
                        flex: 0.5,
                        node: NodeView::Leaf {
                            view: LeafView::single(),
                        },
                    },
                ],
            },
            focused_ordinal: 0,
        };
        assert!(matches!(snap.validate(), Err(LoadError::Invalid(_))));
    }

    #[test]
    fn split_with_one_child_is_rejected() {
        let snap = LayoutSnapshot {
            version: LayoutSnapshot::VERSION,
            root: NodeView::Split {
                axis: Axis::Vertical,
                children: vec![ChildView {
                    flex: 1.0,
                    node: NodeView::Leaf {
                        view: LeafView::single(),
                    },
                }],
            },
            focused_ordinal: 0,
        };
        assert!(matches!(snap.validate(), Err(LoadError::Invalid(_))));
    }

    #[test]
    fn unsupported_version_is_rejected() {
        let snap = LayoutSnapshot {
            version: 999,
            root: NodeView::Leaf {
                view: LeafView::single(),
            },
            focused_ordinal: 0,
        };
        assert!(matches!(
            snap.validate(),
            Err(LoadError::UnsupportedVersion(999))
        ));
    }

    #[test]
    fn valid_single_pane_restores() {
        let snap = LayoutSnapshot {
            version: LayoutSnapshot::VERSION,
            root: NodeView::Leaf {
                view: LeafView::single(),
            },
            focused_ordinal: 0,
        };
        let restored = snap.restore().expect("single pane restores");
        assert_eq!(restored.layout.leaf_count(), 1);
    }

    #[test]
    fn save_then_load_round_trips_through_disk() {
        let l = grid_of(3);
        let tmp = std::env::temp_dir().join(format!("c0pl4nd-ws-{}.json", std::process::id()));
        save(&l, &tmp, |_| LeafView::single()).expect("save");

        let restored = load_strict(&tmp).expect("load");
        assert_eq!(restored.layout.leaf_count(), 3);
        let win = Rect::new(0, 0, 800, 600);
        assert_eq!(restored.layout.cascade(win).len(), 3);

        let _ = std::fs::remove_file(&tmp);
    }

    #[test]
    fn missing_file_falls_back_to_single_pane() {
        let missing = std::env::temp_dir().join("c0pl4nd-does-not-exist-xyzzy.json");
        let _ = std::fs::remove_file(&missing);
        let restored = load(&missing);
        assert_eq!(restored.layout.leaf_count(), 1);
    }

    #[test]
    fn leaf_view_for_clamps_active() {
        let mut g = TabGroup::new(LeafId(0), 0);
        g.add_tab(1);
        g.add_tab(2);
        g.active = 99; // out of range
        let v = leaf_view_for(&g, Some("/tmp".into()), None);
        assert_eq!(v.tab_count, 3);
        assert_eq!(v.active, 2);
        assert_eq!(v.cwd.as_deref(), Some("/tmp"));
    }
}
