# C0PL4ND — Control-Test Ledger

A running list of **every interactive control** in the modern egui chrome and the
**simulated-input** test that drives it. A control is "green" only when a test
clicks/types/drags the **real** widget against the **real** frame loop
(`C0pl4ndApp::frame_tick`) and asserts the **observable outcome** — never
set-state-then-assert-the-same-state.

Test harness: `egui_kittest` (`crates/app/tests/egui_chrome.rs`). Queries are
semantic (`get_by_label`), never pixel coordinates.

## Legend
- ✅ **green** — a simulated-input test drives the real widget and the real effect is asserted, passing.
- 🟡 **needs human eyes** — cannot be asserted headlessly (OS/GPU effect); verify visually.
- ⬜ **not yet built** — control arrives in a later milestone.

## Milestone 1 — chrome + placeholder grid

| Control | User action | Asserted outcome | Test | Status |
|---|---|---|---|---|
| New-pane `+` | click | pane count +1 | `clicking_plus_splits_a_new_pane` | ✅ |
| Split-down `⬓` | click | pane count +1 (vertical split) | `clicking_split_down_splits_a_new_pane` | ✅ |
| Tab (`pane N`) | click | focused pane changes to N | `clicking_a_tab_changes_the_focused_pane` | ✅ |
| Settings gear `⚙` | click | settings window opens | `clicking_gear_opens_settings` | ✅ |
| Settings **Close window** | click the window's ✕ | settings window actually closes | `settings_close_button_actually_closes_the_window` | ✅ |
| Caption close `✕` | click | real `ViewportCommand::Close` issued | `clicking_close_caption_issues_a_close_command` | ✅ |
| Caption minimize `—` | click | real `ViewportCommand::Minimized` issued | `clicking_minimize_caption_issues_a_minimize_command` | ✅ |
| Caption maximize `◻` | click | real `ViewportCommand::Maximized` issued | `clicking_maximize_caption_issues_a_maximize_command` | ✅ |
| 6-pane cap | click `+` past 6 | count holds at 6 | `splitting_past_six_panes_is_refused` | ✅ |
| Window drag (wordmark) | press+drag | OS window move (`StartDrag`) | — | 🟡 needs human eyes (OS drag not observable headlessly) |
| Acrylic / rounded corners | — | OS blur + rounded frame | — | 🟡 needs human eyes (DWM/GPU effect) |
| Pane drag-rearrange | drag pane onto pane | tile order changes | — | ⬜ egui_tiles native; add a drag test |

### Bugs caught by this discipline
- **split-down added no pane** (`⬓` was dead): the wrap path made the focused
  tile a child of two containers, so `parent_of` was ambiguous and the new
  container was orphaned then GC'd. Fixed by capturing the parent before
  wrapping (`grid.rs::split_focused`); proved by
  `split_focused_wrap_path_adds_a_reachable_pane` + the interaction test above.
  The old state-only test (`split` called directly with `Horizontal`) never hit
  this path.

## Milestone 2 — live glyphon terminal panes

Test harness: `egui_kittest` + real PTYs (`crates/app/tests/egui_terminal.rs`).
The PTY/input/resize **logic** is the bug-prone part and is tested headlessly
with simulated input; the glyphon GPU paint callback can't run under kittest's
software path (recon §7) so the pixel render is 🟡 — verify via the offscreen
`screenshot.rs` visual-QA path, NOT kittest.

| Control | User action | Asserted outcome | Test | Status |
|---|---|---|---|---|
| Terminal pane: type text | type `echo <tok>` + Enter | bytes reach the focused pane's real PTY AND `<tok>` appears in that pane's grid | `typing_a_command_reaches_the_pty_and_updates_the_grid` | ✅ |
| Pane focus | click pane 1's tab, then type | input routes to pane 1's PTY/grid; does NOT leak to pane 0 | `clicking_a_pane_routes_typed_input_to_that_pane_only` | ✅ |
| Resize → PTY | shrink the window | the focused pane's PTY grid `(cols,rows)` shrinks | `shrinking_the_window_resizes_the_pane_pty` | ✅ |
| Glyphon GPU render (single pane) | — | grid glyphs render to the pane texture | `glyphon_terminal_render_produces_visible_pixels` (offscreen wgpu readback) | ✅ |
| Glyphon GPU render (**multi-pane**) | — | BOTH panes of a 2-pane split render glyphs into their OWN sub-rects through the shared `TermGpu` (prepare-both-then-render-both), each clipped inside its rect | `glyphon_two_panes_both_render_visible_pixels` (offscreen wgpu readback, two distinct PTY tokens) + `glyphon_terminal_render_through_real_egui_callback` (real egui frame, LEFT+RIGHT halves each ≥100 bright px) | ✅ |
| Tab order stability | relaunch / re-render | tab strip enumerates panes in STABLE left→right order (never the `ahash::HashMap` random storage order) | `panes_in_visual_order_is_stable_and_matches_layout` + `panes_in_visual_order_stable_across_rebuilds` + `panes_in_visual_order_covers_split_panes` | ✅ |
| Caption cluster flush-right | — | ⚙ — ◻ ✕ hug the window's RIGHT edge at any width (close button's right edge ≥ `win_w − 16px`; reads ⚙…✕ left→right) | `caption_cluster_is_flush_right` | ✅ |

### Bugs caught / fixed by this discipline (Milestone 2.1)

- **Multi-pane render — only one pane shows / intermittent black.** The
  single-pane readback test was structurally blind to the multi-pane defect.
  The new `glyphon_two_panes_both_render_visible_pixels` renders TWO `PaneTerm`s
  (distinct PTY tokens) into two sub-rects of one offscreen surface through the
  real `prepare_pane`/`render_pane` path with the egui-wgpu per-callback
  scissor+viewport sequence, and asserts BOTH pane rects light up (970 / 1042
  non-bg px, 0 leak) — proving the shared `TermGpu` (shared atlas/viewport/font
  system, per-pane renderer+buffer) is correct across panes. The real-egui-frame
  test additionally splits the central band into LEFT/RIGHT halves so a black
  half can no longer hide behind an aggregate count.
- **Tab order reshuffled between launches (pane 1, pane 0).** `pane_titles()`
  iterated the `ahash::HashMap` tile storage, whose order changes every process
  launch. Fixed by walking the tree from the root in declared child order
  (`grid::panes_in_visual_order`); now deterministic and matching the on-screen
  layout.
- **Caption buttons not flush-right.** The titlebar nested a `left_to_right`
  layout inside an outer `right_to_left`, floating the cluster mid-strip. Rebuilt
  on SCR1B3's idiom (`horizontal_centered` with left content first, then a nested
  `right_to_left` over the remaining width) + painter-drawn caption buttons ported
  from SCR1B3's `caption_btn`, so the cluster pins flush-right at any width.

Headless logic also unit-tested in `pane_term`/`term_render`/`core::term::keys`
(key→PTY encoding, debounced resize, pixel→cell mapping, payload geometry).

## Milestone 3+ (planned controls — tests required before "done")

| Control | Asserted outcome | Status |
|---|---|---|
| Tab middle-click | tab/pane closes | ⬜ |
| Tab drag A→B | order changes | ⬜ |
| Settings: theme dropdown | runtime visuals change | ⬜ |
| Settings: opacity slider | window opacity changes | ⬜ |
| Settings: acrylic toggle | backdrop toggles | ⬜ |
| Settings: font size | grid cell size changes | ⬜ |
| Command palette | command executes | ⬜ |
| Scrollback | viewport scrolls | ⬜ |
| Copy / paste | clipboard round-trips to PTY | ⬜ |
