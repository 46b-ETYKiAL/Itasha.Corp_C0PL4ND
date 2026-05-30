# C0PL4ND Feature-Completeness Audit + Implementation Plan

Branch: `feat/feature-completeness-audit` (off master @ 6001457, which includes PR #23).
Repo: Itasha.Corp_C0PL4ND (public satellite). Flow: worktree clone → branch → PR → admin-merge.
Stack: Rust + winit/wgpu/glyphon/vte/portable-pty. Local-first (no telemetry/accounts/network). CRT brand.

## Phases

### Phase B — Research (parallel agents, read-only)
- B-A: terminal-spec/escape-sequence coverage (truecolor, OSC 8/52/7/133, bracketed paste, sixel/kitty, URL detect, ligatures, IME/CJK, bidi). Cite.
- B-B: window/UX/chrome + input + winit/wgpu/glyphon patterns (tabs/splits/quake, transparency, theme hot-reload, keybinds, copy mode, perf/damage; backplate/geometry/snap patterns). Cite.

### Phase C — Audit (me, parallel with B)
- config/mod.rs schema; render pass layer model (for backplates); window event handling (geometry + snap); palette actions + term.rs caps.
- Output: prioritized gap list P0→P3.

### Phase D — Implement the 4 named deferred items (main thread, sequential — all touch window.rs/config)
- D1 hover backplates (rect layer behind chrome text)
- D2 remembered window size/position (config + restore, multi-monitor/DPI clamp)
- D3 in-app settings panel (native UI, bidirectional w/ TOML, live-apply)
- D4 Win+arrow snap (WM_ handling, frameless)

### Phase E — Remaining P0/P1 gaps from gap list

### Phase F — Ship
- release build green → PR → admin-merge → verify (PR MERGED + remote master HEAD == merge oid)
- Final what's-done/deferred summary

## Constraints / lessons
- Cross-verify tool output (prior sessions had injected noise) via independent checks.
- Implementation items conflict on window.rs/config → do on MAIN THREAD sequentially, not parallel write-agents.
- Parallelism = research agents (read-only) + my audit; monitor agents for stalls (600s no-output watchdog).
- Per-task commits; verify build before each ship.
