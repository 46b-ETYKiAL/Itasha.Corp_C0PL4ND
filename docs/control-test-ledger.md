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

## Milestone 2.2 — egui input/response parity with the legacy shell

The PR #166 refactor brought up the egui shell but did NOT port the legacy
`window.rs` per-frame terminal-effect draining or the pointer→PTY mouse path.
The shipping egui binary therefore silently dropped every PTY query reply,
OSC 52 clipboard write, OSC 4/10/11/12/104 colour set, and OSC 9/777
notification, AND let those unread queues grow unbounded — and offered no
mouse reporting or scrollback navigation at all. This milestone closes that
parity gap (`pump_host_effects`/`pump_pane_effects`/`apply_color_set` +
`report_mouse`/`scroll_view` in `pane_term.rs`/`mod.rs`).

Coverage is **deterministic `pane_term` unit tests** that drive the REAL core
`Terminal` (feed escape bytes, drain, assert the observable outcome) — not a
live-PTY `egui_kittest` frame test, because reproducing mouse-mode/OSC escapes
through a cross-platform live shell is non-deterministic. The frame-loop glue
(read `ui.input` → call these methods) is thin and compile-checked against the
production `frame_tick`.

| Capability | Asserted outcome | Test | Status |
|---|---|---|---|
| OSC query reply → PTY | a `CSI 6n` cursor-position query is drained and the reply is written BACK to the pane's own PTY (queue emptied) | `pump_host_effects_drains_every_queue` | ✅ |
| OSC 52 clipboard write → host | an `OSC 52` write surfaces as a `clipboard_writes` payload for `ctx.copy_text` (zeroizing buffer drained) | `pump_host_effects_drains_every_queue` | ✅ |
| OSC 4/10/11/12/104 colour set → live theme | an `OSC 4` set surfaces as a `ColorSet::Indexed` applied via `apply_color_set` to the live `Theme` | `pump_host_effects_drains_every_queue` | ✅ |
| OSC 9/777 notification → taskbar | an `OSC 9` notification marks `notified` (→ `RequestUserAttention` while unfocused); the text is never read (privacy) | `pump_host_effects_drains_every_queue` | ✅ |
| OSC 9;4 progress drain (bounded growth) | the progress queue is drained each frame so it cannot grow without bound (no UI yet) | `pump_host_effects_drains_every_queue` | ✅ |
| Mouse reporting → PTY (E6) | `report_mouse` encodes + writes to the PTY only when `mouse_mode() != Off`; a bare-motion event under `?1000` reports nothing | `report_mouse_gates_on_mouse_mode` | ✅ |
| Mouse-wheel scrollback | `scroll_view(+n)` raises the view offset off the live bottom; scrolling forward past the bottom clamps to 0 | `scroll_view_moves_the_scrollback_offset` | ✅ |

### Bugs/gaps caught by this discipline (Milestone 2.2)

- **Every PTY query reply silently dropped in the egui shell.** A TUI that
  queries device attributes / cursor position got no answer (misdetect/hang).
  Now drained and written back to the originating pane's PTY each frame.
- **Unbounded queue growth.** `take_*` drain internal `Vec`s via `mem::take`;
  un-drained in the egui path they grew without bound (a slow memory leak on
  top of the functional gap). Draining each frame fixes both at once.

## Milestone 2.3 — whole-app audit-and-fix program (2026-06-12)

The 6-dimension audit's eight PRs each shipped with regression coverage. The
egui-shell-facing ones driven through the real `frame_tick`:

| Capability | Asserted outcome | Test | Status |
|---|---|---|---|
| Font zoom | Ctrl+Plus grows / Ctrl+Minus shrinks / Ctrl+0 resets the grid font | `ctrl_plus_minus_zero_zooms_the_grid_font` (egui_chrome) | ✅ |
| New-pane shortcut | Ctrl+Shift+T opens a new pane (pane count +1) | `ctrl_shift_t_opens_a_new_pane_via_keyboard` (egui_chrome) | ✅ |
| Focus reporting `?1004` | report only when armed; ESC[I focus-in / ESC[O focus-out | `report_focus_only_reports_when_armed` (pane_term) | ✅ |
| Jump-to-prompt | view scrolls to a prior OSC 133 prompt mark | `jump_to_prompt_scrolls_to_a_prompt_mark` (pane_term) | ✅ |
| Mouse selection copy | covered cells extracted, ordered, trimmed, newline-joined | `selection_text_extracts_ordered_trimmed_rows` (pane_term) | ✅ |
| Keyboard→PTY map | named keys / F-keys / Ctrl-chords → correct `LogicalKey` | `egui_key_to_logical_maps_keys_and_ctrl_chords` (mod) | ✅ |
| Search/link column map | UTF-8 byte offset → character column | `byte_to_col_counts_chars_to_the_byte_boundary` (mod) | ✅ |
| Acrylic tint parse | `#rrggbb` + alpha → RGBA; non-hex → None | `tint_rgba_parses_hex_and_appends_alpha` (mod) | ✅ |

The core VT-correctness + DoS-cap fixes (PR #169) carry their own
`c0pl4nd-core` regression tests (continuation lockstep ×5, CUU/CUD margins, SU
scrollback ×2, the six queue/cap DoS tests). See `GAP_LIST.md` §
"Whole-app audit-and-fix program (2026-06-12)".

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
| Scrollback (mouse-wheel) | view offset moves off the live bottom (see Milestone 2.2 `scroll_view_moves_the_scrollback_offset`) | ✅ |
| Copy / paste | clipboard round-trips to PTY | ⬜ |
