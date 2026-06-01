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
| Glyphon GPU render | — | grid glyphs render to the pane texture | — | 🟡 needs human eyes / `screenshot.rs` (no GPU pass under kittest) |

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
