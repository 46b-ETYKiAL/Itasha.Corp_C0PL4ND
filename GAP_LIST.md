# C0PL4ND Prioritized Gap List (audit + best-in-class research)

Synthesized from `RESEARCH_TERMINAL_SPEC.md` (80+ features, 7-terminal matrix), `RESEARCH_WINDOW_UX.md` (window/UX + winit/wgpu patterns), and `AUDIT_FINDINGS.md` (current-state structural audit, twice-verified). Comparison set: Ghostty, WezTerm, kitty, Alacritty, iTerm2, Windows Terminal, Konsole.

Legend: **P0** = expected-everywhere / correctness or breaks real apps · **P1** = strongly wanted · **P2** = power-user · **P3** = niche. Status: ✅ built · ⚠️ partial · ❌ absent.

---

## Already built (confirmed by audit — NOT gaps)
- ✅ Frameless custom titlebar (min/max/close), double-click-maximize, edge-resize + hover cursor (PR #23)
- ✅ Splits/panes (H/V, grid, focus-nav, resize, close) — `core/src/layout/`
- ✅ Tabs (per-cell tab strips, new/close/switch)
- ✅ Command palette (25 actions), fuzzy filter
- ✅ Themes + theme files (`assets/themes/*.toml`), opacity, CRT effects
- ✅ TOML config w/ line-error surfacing, zero-config defaults
- ✅ Truecolor / 256 / 16-color SGR (foreground)
- ✅ OSC 0/2 (title), OSC 7 (cwd), OSC 8 (hyperlinks), OSC 133 (prompt marks; deliberately never reports back — anti-CVE)
- ✅ Sixel graphics (8 MiB cap)
- ✅ Scrollback (10k default) + search
- ✅ Workspace save/restore, sessions
- ✅ Font size hotkeys, fallback chain; cursor style/blink in config
- ✅ Auto-update (opt-in), neofetch splash

---

## P0 — Correctness / breaks real apps (HIGHEST VALUE)

| ID | Gap | Status | Evidence | Chosen fix | Source |
|----|-----|--------|----------|-----------|--------|
| E1 | **Background color + inverse video not rendered** — SGR bg/reverse are parsed but `leaf_spans` resolves fg only. Every app using colored backgrounds (ls, grep --color, syntax bg, htop bars, selections, vim themes) renders wrong. | ❌ render | AUDIT term.rs / leaf_spans | Render per-cell bg quads (reuse `pane_render::ChromeRenderer` ColorRect) behind text; honor reverse-video by swapping fg/bg; honor default-bg. | term-spec §Color; audit |
| E2 | **Alternate screen buffer** (DEC ?1049/?47/?1047) | ❌ | AUDIT: no DEC private modes | Add alt-screen grid; switch on ?1049h/l; restore primary + cursor. Enables vim/less/htop/tmux without trashing scrollback. | term-spec §Misc VT |
| E3 | **Bracketed paste** (DEC ?2004) + paste sanitization | ❌ | AUDIT | Track ?2004; wrap pasted text in ESC[200~..201~; strip embedded 201~ + C0/C1 (anti-injection). | term-spec §Paste (P0 security) |
| E4 | **DEC private mode framework** (?25 cursor visibility, ?1049, ?2004, ?1000/1002/1003/1006 mouse, ?2026 sync) | ❌ | AUDIT | Central CSI ? h/l handler dispatching to mode flags; foundation for E2/E3/E6. | term-spec §Mouse/Misc |
| E5 | **Cursor visibility (DECTCEM ?25)** | ❌ | AUDIT | Hide/show cursor; many TUIs hide it. | term-spec |

## P1 — Strongly wanted

| ID | Gap | Status | Chosen fix | Source |
|----|-----|--------|-----------|--------|
| E6 | **Mouse reporting** (1000/1002/1003 + SGR 1006 + focus 1004) | ❌ | Encode mouse events per active mode; report to PTY. Enables mouse in vim/tmux/htop. | term-spec §Mouse |
| E7 | **DECSCUSR cursor-shape escape** (`CSI Ps SP q`) | ❌ | Let apps set bar/underline/block + blink (nvim mode cursor). | term-spec |
| E8 | **Live config reload** (doc-claimed, unimplemented) | ❌ | `notify` file-watcher on config path → reload + re-apply (no restart). | audit; window-ux §12 |
| E9 | **URL detection + Ctrl/Cmd-click open** (OSC 8 already parsed) | ⚠️ | Heuristic URL scan + hover underline + ctrl-click → open. | term-spec §Hyperlinks |
| E10 | **OSC 52 clipboard write** (read default-OFF) | ❌ | Honor OSC 52 write (remote yank); never auto-read. | term-spec §Clipboard |
| E11 | **OSC 4 / 10 / 11 color query+set** | ❌ | TUI bg-detection theming; respond on PTY. | term-spec §Color |
| E12 | **Synchronized output (DEC ?2026)** | ❌ | Buffer frame between begin/end; flicker-free TUI redraw. | term-spec |
| E13 | **cwd inheritance on split/new-tab** (via OSC 7) | ⚠️ | New pane/tab starts in active pane's OSC7 cwd. | window-ux §4 |
| E14 | **Tab auto-title from OSC 0/1/2** + Ctrl+1..9 switch | ⚠️ | Title tabs from running cmd; numeric switch. | window-ux §5 |
| E15 | **Render-on-change (damage) gating** | ⚠️ | Gate request_redraw + glyphon prepare on dirty flag (perf/battery). | window-ux §9 |
| E16 | **Graceful PTY-exit handling** | ⚠️ | Close pane/tab cleanly on shell exit; show status. | window-ux §15 |

## D — Named deferred items
- ✅ D1 hover backplates (f9efb7a + borrow fix 2334b74)
- ✅ D2 window geometry persistence (652c44b config + c424634 window)
- ✅ D3 in-app settings panel (613cee7 + import fix 7d3f9c2)
- 🔄 D4 Win+arrow snap (delegated to app-correctness agent)

## VT-core (merged 21e3b70) — provides API for E-integration
- ✅ DEC private-mode framework, alt screen, DECSCUSR cursor shape, bracketed-paste/cursor-visibility getters, encode_mouse() (818542e; 177 core tests pass)
- 🔄 App-side wiring (E1/E5/E7/E2/E3/E6) in flight on app-correctness branch

## P2 / P3 — deferred with justification (out of "ship LESS" taste or niche)
- P2: programming ligatures (HarfBuzz), kitty graphics protocol (Sixel already covers inline images), `.itermcolors` import, quake/dropdown mode, leader/prefix keybinds, copy-mode/vi keys, block/rectangular selection, title stack (XTWINOPS 22/23).
- P3: BiDi/RTL (even Ghostty/Alacritty punt), background image, animated cursor trail, base16 framework, detached daemon, screen-reader UIA, iTerm2 inline images (OSC 1337).

---

## Execution decision
1. **D1–D4** — impl agent (running).
2. **Phase E (this pass): E1, E5, E4, E2, E3** — the P0 correctness set (bg/inverse rendering first = most visible; cursor visibility; DEC mode framework; alt screen; bracketed paste). These make the terminal render correctly and run TUIs. Then **E7 (DECSCUSR)** and **E6 (mouse)** if clean.
3. **Remaining P1 (E8–E16)** and all P2/P3 — documented backlog; implement opportunistically / future pass with explicit justification (no silent deferral; this list IS the tracked record).

Rationale: a terminal that renders background colors wrong and can't run vim is failing its core job — these P0s outrank chrome polish. All term.rs P0s touch the same files, so they're done sequentially (one commit each, with tests) after D1–D4 frees `window.rs`.

---

## Shipped Status (final update — master 2026-05-30)

This section is the authoritative as-shipped ledger; earlier rows above are the original audit.

### ✅ Shipped & merged to master
- **D1** caption-button hover/press backplates
- **D2** remembered window size/position/maximized (multi-monitor + DPI clamp)
- **D3** in-app settings panel (font/theme/cursor/scrollback/opacity/startup; live-apply + TOML write-back)
- **VT-core parser**: DEC private modes, alternate screen, DECSCUSR cursor shape, bracketed-paste/cursor-visibility getters, `encode_mouse()`, `cursor_position()`
- **E1** per-cell background color + inverse-video rendering
- **E3** clipboard paste + bracketed-paste wrapping (dependency-free shell-out)
- **E5/E7** terminal cursor draw (block/bar/underline, blink, visibility)
- **E6** mouse reporting → PTY (DEC 1000/1002/1003 + SGR 1006)
- **E14** Ctrl+1-9 window-tab switching
- **OSC suite (core)**: OSC 52 (write; read default-off), OSC 4/10/11/12 query+set, OSC 104 reset, OSC 9/777 notifications, title stack (CSI 22/23 t), `.itermcolors` import, ligatures config flag, PTY-response channel
- **OSC app-wiring (E10/E11)**: `pump_terminal_io()` drains query replies → PTY, clipboard writes → OS clipboard (`write_os_clipboard`), color sets → live theme, notifications → log
- **E16** graceful PTY exit (close pane on shell exit; exit app when last pane closes)

### ⏳ Remaining (not yet implemented)
- **E9** URL detection + Ctrl-click to open — designed (heuristic row-scan for http(s)://|file:// under the cursor → `open_path`); not yet wired into the mouse-press handler.

### ❌ Deferred — with reasons
- **D4 Win+Arrow Aero snap** — BLOCKED by the offline build environment: requires the `windows` crate, which cannot be fetched (no crates.io access). Environmental constraint, not a scoping choice. Full Win32 subclass recipe (WS_THICKFRAME + WM_NCCALCSIZE/NCHITTEST/GETMINMAXINFO) is recorded for when deps are available.
- **BiDi/RTL** — needs the `unicode-bidi` crate (offline-blocked) plus visual reordering in the shaper.
- **Programming ligatures** — core `ligatures` flag shipped; the renderer-side cosmic-text shaping change is deferred.
- **Full kitty graphics protocol** — Sixel already covers inline images; full kitty-graphics is lower-value.
