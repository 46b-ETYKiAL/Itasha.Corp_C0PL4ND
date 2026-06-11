# C0PL4ND Wave 3 — Handoff (SUPERSEDED 2026-06-11)

> **This checkpoint is historical and no longer actionable.** Every item it
> listed has shipped, and both of its "hard blockers" are resolved. Kept only
> for provenance. For current state read `GAP_LIST.md` (as-shipped ledger) and
> `docs/control-test-ledger.md` (control → simulated-input test map).

## Resolution of the checkpoint

- **Both environmental blockers are gone.** crates.io and github are reachable;
  the offline-cargo / offline-github conditions that froze wave-3 no longer hold.
- **All "NEXT STEPS" items shipped to master**: E3 bracketed paste, E5/E7 cursor
  draw, E6 mouse reporting, E9 URL ctrl-click, E14 Ctrl+1-9 tab switch, E16
  graceful PTY exit, the full OSC suite (52 / 4 / 10 / 11 / 12 / 104 / 9 / 777,
  title stack), D4 Win+Arrow snap, programming ligatures, BiDi/RTL, and full
  Kitty graphics. See `GAP_LIST.md` § "Shipped Status".
- **The "DEFER w/ justification" list is void** — every entry (D4, BiDi/RTL,
  ligatures, Kitty graphics) was implemented once crates.io was reachable.

## egui-shell parity follow-through (2026-06-11)

The one regression the legacy-shell ledger could not see: the PR #166 **egui
shell** rewrite (the shipping binary) had not ported the legacy per-frame
terminal-effect draining or the pointer→PTY mouse path. A whole-app audit found
5 P0 egui-path gaps (clipboard / notification / colour-set / PTY-response /
progress drains) plus missing mouse reporting and scrollback. All are now wired
and unit-tested — see `GAP_LIST.md` § "egui-shell parity (2026-06-11)" and
`docs/control-test-ledger.md` § Milestone 2.2.

## Durable facts (still true)

- App pkg name = `c0pl4nd` (NOT `c0pl4nd-app`): `cargo check -p c0pl4nd`.
- Grid types: `c0pl4nd_core::{Cell{c,fg,bg,flags}, Color{Default,Indexed(u8),Rgb}, CellFlags{bold,italic,underline,inverse,strikeout}}`.
- Core terminal-effect API (drained per frame by the egui shell):
  `take_pty_response` / `take_clipboard_writes` / `take_color_sets` /
  `take_notifications` / `take_progress` / `encode_mouse` / `mouse_mode`.
- Mouse/OSC core types are re-exported at `c0pl4nd_core::term::`
  (`MouseMode`, `MouseButton`, `MouseEventKind`, `MouseModifiers`, `ColorSet`,
  `DynamicColor`).
