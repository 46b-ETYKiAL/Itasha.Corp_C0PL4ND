# C0PL4ND — Layout & Multiplexing

C0PL4ND uses a binary/n-ary **split-tree** to arrange multiple terminals in a single window, with **nested tabs per cell**, **drag-to-rearrange**, **layout persistence**, and **quick-layout presets** — all keyboard-first, none of it required if you only want a single pane.

---

## The grid model

```
WINDOW
└── TAB(s)
    └── LAYOUT (split-tree, up to MAX_PANES = 6 leaves)
        └── CELL (each leaf = a TabGroup)
            └── NESTED TAB(s)
                └── TERMINAL (PTY)
```

- Each **window** holds one or more **window-level tabs**.
- Each window-level tab holds a **split-tree layout** of up to **six cells (panes)**.
- Each **cell** is a TabGroup that holds one or more **nested tabs**.
- Each nested tab owns a terminal (PTY) with its own shell + scrollback.
- Splits run horizontally (side-by-side) or vertically (top/bottom).
- A single-pane tab draws **no** pane chrome — visually identical to a non-multiplexed terminal.
- More than one pane gets a 1-px chrome: the **focused** pane carries a subtle signal-teal border; the others a muted grey.

`MAX_PANES = 6` is a readability guardrail — past six panes the per-cell text becomes too small to scan. Trying to split past it is blocked with a transient notice.

---

## Default keybindings

All keybindings are user-overridable in **[CONFIG.md](../CONFIG.md)**; the defaults below are the ones the app ships with.

### Window-level tabs
| Action | Key |
|---|---|
| New tab | `Ctrl+Shift+T` |
| Close focused tab or pane | `Ctrl+Shift+W` |
| Next / previous tab | `Ctrl+Shift+]` / `Ctrl+Shift+[` |
| Cycle tabs | `Ctrl+Shift+Tab` |

### Splits & panes
| Action | Key |
|---|---|
| Split right (vertical) | `Ctrl+Shift+D` |
| Split down (horizontal) | `Ctrl+Shift+E` |
| Close other panes | `Ctrl+Shift+O` |
| Focus pane by direction | `Alt + Arrow` |
| Drag-to-rearrange (mouse) | hold `Ctrl+Shift` and drag from a pane |
| Keyboard swap (rearrange) | `Alt+Shift + Arrow` |
| Pane zoom (toggle) | `Ctrl+Shift+Z` |
| Equalize cells | `Ctrl+Shift+=` |

### Nested tabs (within a cell)
| Action | Key |
|---|---|
| Next nested tab | `Ctrl+PageDown` |
| Previous nested tab | `Ctrl+PageUp` |

The window-level and cell-level tab keys are deliberately distinct so they never overlap.

### Search & palette
| Action | Key |
|---|---|
| Search scrollback | `Ctrl+Shift+F` |
| Command palette | `Ctrl+Shift+P` |

The palette is the canonical entry-point for everything below — presets, save/restore, equalize, zoom.

---

## Drag-to-rearrange

Hold `Ctrl+Shift` and click-drag from inside any pane. A **6 px move threshold** ensures normal clicks are never interpreted as drags. While dragging, the source pane dims and the candidate target shows a drop-zone highlight.

Each target pane is divided into **five drop zones**:

```
┌────────────────────┐
│         TOP        │   Drop in TOP / BOTTOM / LEFT / RIGHT
│ ┌────────────────┐ │   → place the source on that side of the target
│ │                │ │     (creates a tree split)
│ │L     CENTER   R│ │
│ │                │ │   Drop in CENTER
│ └────────────────┘ │   → merge the source's tabs into the
│       BOTTOM       │     target's TabGroup
└────────────────────┘
```

The keyboard-only equivalent is `Alt+Shift + Arrow` — swap the focused pane with its neighbor in that direction.

---

## Quick-layout presets

From the palette (`Ctrl+Shift+P`):

| Preset | Shape |
|---|---|
| `Layout: 1` | single pane |
| `Layout: 1x2` | two side-by-side |
| `Layout: 2x1` | two stacked |
| `Layout: 1+2` | one main left, two stacked right |
| `Layout: 2x2` | four cells, grid |
| `Layout: 1+3` | one main left, three stacked right |
| `Layout: 2x3` | six cells, grid (the MAX_PANES ceiling) |
| `Equalize Cells` | rebalance flex ratios |
| `Pane Zoom` | toggle full-window zoom of the focused pane |

---

## Save & restore — named workspaces

Use the palette:

- **`Save Layout As…`** prompts for a name; the current split-tree (shape + per-leaf cwd, profile, and active-nested-tab index) is written to the workspaces directory next to your config file. The saved file is plain JSON — diff/version-control-friendly.
- **`Restore Layout`** lists saved workspaces; selecting one rebuilds the split-tree, spawning **fresh** PTYs per leaf. **Live process state is not restored** — by design (a terminal is not a checkpoint/restore system; bringing back stale processes is the opposite of what an operator wants).

On launch, if a saved **default** layout exists it is restored automatically; otherwise C0PL4ND starts with a single pane — the zero-config baseline.

---

## Recovery / safety net

C0PL4ND **never crashes** on a malformed layout file. If a saved layout is:

- **corrupt JSON**, or
- **larger than `MAX_PANES`**, or
- has **invalid flex sums**,

the loader logs the issue and falls back to a single pane. Your config is untouched. You can then:

- run **`Reset Layout`** from the palette to clear to a single pane,
- **`Equalize Cells`** to rebalance flex ratios when a drag has left things lopsided,
- **`Pane Zoom`** to focus on one pane while keeping the others alive in the background.

---

## Architecture notes

- The split-tree engine lives in `crates/core/src/layout/` — pure data, no GPU coupling, fully unit-tested.
- Layout persistence is in `crates/core/src/layout_persist.rs` (serde JSON; round-trip + byte-stable + validation tests).
- Drag state-machine and 5-zone classifier are in `crates/app/src/drag.rs`.
- Per-leaf render geometry, pane chrome (gutters + 1-px borders + focused accent), and the cell tab-bar are in `crates/app/src/pane_render.rs`.

The split-tree (vs. flat grid) decision and the MAX_PANES rationale are recorded in `docs/adr/`.
