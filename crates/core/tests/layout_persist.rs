//! Integration tests for layout persistence against the public surface:
//! serde round-trip + byte-stability, structural restore, per-leaf metadata
//! survival, the corrupt/over-cap/invalid → single-pane safe-fallback contract
//! (pre-mortem #6), and the preset builders' leaf-count + shape (T6.3).

use std::path::PathBuf;

use c0pl4nd_core::layout::{Axis, Layout, LayoutNode, Preset, Rect, SplitOutcome, MAX_PANES};
use c0pl4nd_core::layout_persist::{
    self, ChildView, LayoutSnapshot, LeafView, LoadError, NodeView,
};

const WIN: Rect = Rect {
    x: 0,
    y: 0,
    w: 1280,
    h: 800,
};

/// Build an `n`-leaf layout through the guarded action layer.
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

/// Unique temp path per test (process id + tag) to avoid cross-test clobber.
fn tmp(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!("c0pl4nd-persist-{}-{tag}.json", std::process::id()))
}

#[test]
fn snapshot_round_trip_is_structurally_equal_and_byte_stable() {
    let l = grid_of(5);
    let snap = LayoutSnapshot::capture(&l, |_| LeafView::single());

    let json = snap.to_json().expect("serialize");
    let back = LayoutSnapshot::from_json(&json).expect("deserialize");
    assert_eq!(snap, back, "round-trip must be structurally identical");

    let json2 = back.to_json().expect("reserialize");
    assert_eq!(
        json, json2,
        "serde output must be byte-stable for a fixed tree"
    );
}

#[test]
fn save_then_load_restores_the_same_geometry_with_fresh_ids() {
    let l = grid_of(6);
    let before: Vec<_> = l.cascade(WIN).into_iter().map(|(_, r)| r).collect();

    let path = tmp("geom");
    layout_persist::save(&l, &path, |_| LeafView::single()).expect("save");
    let restored = layout_persist::load_strict(&path).expect("load");

    let after: Vec<_> = restored
        .layout
        .cascade(WIN)
        .into_iter()
        .map(|(_, r)| r)
        .collect();
    assert_eq!(before, after, "restored cascade geometry must match");
    assert_eq!(restored.leaves.len(), 6);

    let _ = std::fs::remove_file(&path);
}

#[test]
fn per_leaf_cwd_profile_and_tab_metadata_survive_round_trip() {
    let mut l = Layout::new();
    let SplitOutcome::Split(_) = l.try_split(l.focused, Axis::Vertical) else {
        panic!("split failed");
    };
    let leaves = l.leaves();
    let snap = LayoutSnapshot::capture(&l, |id| {
        if id == leaves[0] {
            LeafView::new(3, 2, Some("/srv/work".into()), Some("zsh".into()))
        } else {
            LeafView::new(1, 0, None, None)
        }
    });

    let restored = snap.restore().expect("restore");
    let first = &restored.leaves[0].1;
    assert_eq!(first.tab_count, 3);
    assert_eq!(first.active, 2);
    assert_eq!(first.cwd.as_deref(), Some("/srv/work"));
    assert_eq!(first.profile.as_deref(), Some("zsh"));
    let second = &restored.leaves[1].1;
    assert_eq!(second.tab_count, 1);
    assert!(second.cwd.is_none());
}

#[test]
fn corrupt_file_falls_back_to_single_pane_and_never_panics() {
    let path = tmp("corrupt");
    std::fs::write(&path, "{ broken json ]]").unwrap();

    // Strict surfaces the parse error...
    assert!(matches!(
        layout_persist::load_strict(&path),
        Err(LoadError::Parse(_))
    ));
    // ...while the safe loader degrades to one pane.
    let restored = layout_persist::load(&path);
    assert_eq!(restored.layout.leaf_count(), 1);
    assert_eq!(restored.leaves.len(), 1);

    let _ = std::fs::remove_file(&path);
}

#[test]
fn over_cap_tree_is_rejected_on_load() {
    let mut children = Vec::new();
    for _ in 0..(MAX_PANES + 1) {
        children.push(ChildView {
            flex: 1.0 / (MAX_PANES as f32 + 1.0),
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
    let json = serde_json::to_string(&snap).unwrap();
    let path = tmp("overcap");
    std::fs::write(&path, &json).unwrap();

    assert!(matches!(
        layout_persist::load_strict(&path),
        Err(LoadError::Invalid(_))
    ));
    // Safe loader still gives a usable single pane.
    assert_eq!(layout_persist::load(&path).layout.leaf_count(), 1);

    let _ = std::fs::remove_file(&path);
}

#[test]
fn valid_file_restores_exactly() {
    let snap = LayoutSnapshot {
        version: LayoutSnapshot::VERSION,
        root: NodeView::Split {
            axis: Axis::Horizontal,
            children: vec![
                ChildView {
                    flex: 0.5,
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
        focused_ordinal: 1,
    };
    let path = tmp("valid");
    snap.save(&path).expect("save");
    let restored = layout_persist::load_strict(&path).expect("load");
    assert_eq!(restored.layout.leaf_count(), 2);
    // focused_ordinal 1 → the second DFS leaf.
    let leaves = restored.layout.leaves();
    assert_eq!(restored.layout.focused, leaves[1]);

    let _ = std::fs::remove_file(&path);
}

#[test]
fn missing_file_is_a_single_pane() {
    let path = tmp("missing");
    let _ = std::fs::remove_file(&path);
    let restored = layout_persist::load(&path);
    assert_eq!(restored.layout.leaf_count(), 1);
}

// --- preset builders (T6.3) -----------------------------------------------

#[test]
fn preset_builders_produce_expected_leaf_counts() {
    let cases = [
        (Preset::Single, 1),
        (Preset::TwoColumns, 2),
        (Preset::TwoRows, 2),
        (Preset::MainLeftTwoStacked, 3),
        (Preset::Grid2x2, 4),
        (Preset::MainLeftThreeStacked, 4),
        (Preset::Grid2x3, 6),
    ];
    for (preset, expected) in cases {
        let l = Layout::from_preset(preset);
        assert_eq!(l.leaf_count(), expected, "preset {}", preset.label());
        assert!(l.leaf_count() <= MAX_PANES);
        // Renders without an empty cell.
        for (_, r) in l.cascade(WIN) {
            assert!(r.w > 0 && r.h > 0, "empty cell in {}", preset.label());
        }
    }
}

#[test]
fn grid_2x3_is_three_rows_of_two() {
    let l = Layout::from_preset(Preset::Grid2x3);
    match &l.root {
        LayoutNode::Split { axis, children } => {
            assert_eq!(*axis, Axis::Vertical);
            assert_eq!(children.len(), 3, "three rows");
            for row in children {
                match &row.node {
                    LayoutNode::Split { axis, children } => {
                        assert_eq!(*axis, Axis::Horizontal);
                        assert_eq!(children.len(), 2, "two columns per row");
                    }
                    _ => panic!("each row must be a horizontal split"),
                }
            }
        }
        _ => panic!("2x3 root must be a vertical split"),
    }
}

#[test]
fn auto_arrange_rebalances_into_the_squarest_grid() {
    // Six panes in an arbitrary shape → squarest grid (3 cols x 2 rows).
    let mut l = grid_of(6);
    l.rebalance_squarest();
    assert_eq!(l.leaf_count(), 6);
    let rects = l.cascade(WIN);
    assert_eq!(rects.len(), 6);
    for (_, r) in &rects {
        assert!(r.w > 0 && r.h > 0);
    }
}
