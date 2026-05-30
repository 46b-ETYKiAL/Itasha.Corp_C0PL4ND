# C0PL4ND Wave 3 — Handoff (context-compaction checkpoint)

## Repo / branch
- Clone: `.s4f3-data/pubrepo-work/c0pl4nd-audit`
- Branch: `feat/vt-input-wave3` @ `2549ee4` (E3 committed, release-build-clean, working tree CLEAN)
- `origin/master` = `5dae8f2` (end of wave 2; E1 shipped). E3 NOT yet on master.

## SHIPPED to master (verified merged, network was up)
- PR #24 → `c28e76b`: D1 backplates, D2 window geometry, D3 settings panel, VT-core parser (177 core tests).
- PR #25 → `5dae8f2`: E1 per-cell background color + inverse video rendering.

## DONE locally on feat/vt-input-wave3, NOT shipped (blocked on network) — 4 commits ahead of master, release-build-clean
- E3 clipboard paste + bracketed-paste wrap, dependency-free (`2549ee4`). Ctrl+Shift+V → `read_os_clipboard()` shell-out → wrap ESC[200~/201~ + strip embedded 201~ when `term.bracketed_paste()`.
- E6 mouse reporting → PTY (`214968b`, amended clean). Left press/release/wheel → `term.encode_mouse()` when `mouse_mode()!=Off`; 1-based cell coords from focused leaf origin; titlebar/resize still win. Enables mouse in vim/tmux/htop/less.
- `cursor_position()` core getter (`d8c7107`, +unit test, 147 core tests pass).
- E5/E7 terminal cursor draw (`cb6f23e`). Block (semi-transparent)/Bar/Underline per DECSCUSR; hidden on ?25 or scrollback; 530ms blink with idle blink-redraw tick. The app drew NO cursor before this.

## DONE on feat/core-osc (separate clone, by background agent) — NOT merged
- OSC 52 (write; read default-off), OSC 4/10/11/12 query+set, OSC 104 reset, OSC 9/777 notifications, title stack (CSI 22/23 t), `Theme::from_itermcolors()`, `ligatures` config flag, new PTY-response channel. 211 core tests, clippy-clean. Commit `d0b82af` on origin; `29154a9`/`d8fcd9c`/`1beab33` local w/ push-monitor `b1f52pnbd`. New API: take_pty_response/take_clipboard_write/take_color_sets/take_notification/etc (see agent report).
- APP-WIRING STILL NEEDED for this: drain take_pty_response()→PTY each frame; take_color_sets()→live theme; take_clipboard_write()→write_os_clipboard() (need to add inverse shell-out); take_notification()→desktop notifier; config.ligatures→cosmic-text Shaping.

## TWO HARD BLOCKERS (environmental, not choices)
1. **OFFLINE cargo** — cannot fetch crates.io. NO new deps possible. Blocks: arboard (worked around via shell-out), `windows` crate → **D4 Win+Arrow snap BLOCKED**, `unicode-bidi` → BiDi BLOCKED.
2. **OFFLINE github** (`Could not resolve host: github.com`) — cannot push/PR/merge. E3 + all future wave-3 commits are stuck local until network returns.

## NEXT STEPS (all dependency-free; implement locally, stack commits, ship when net returns)
1. **Push E3** the moment github resolves: `git push -u origin feat/vt-input-wave3` → PR --base master → `gh pr merge --admin` → verify (3 stable ls-remote + gh pr view MERGED + 5dae8f2 & new-commit ancestors).
2. **E6 mouse→PTY** (in progress, NOT started in code): in MouseInput Pressed/Released (~line 2290) + MouseWheel handlers, when focused `term.mouse_mode() != MouseMode::Off`, compute 1-based (col,row) from `self.cursor` pixel via `content_rect()` origin + CELL_W=9.0/LINE_HEIGHT=20.0 (subtract focused leaf cell origin; mind TITLEBAR_H=30 + leaf_text_origin pad 8.0/2.0), build `MouseModifiers{shift,alt,control}` from `self.modifiers`, call `term.encode_mouse(button, mods, col, row, kind)` → `Option<Vec<u8>>`, write to PTY via session.write_input; skip normal scroll/selection when consumed. Types are NOT at crate root — use `c0pl4nd_core::term::{MouseMode, MouseButton as TermMouseButton, MouseEventKind, MouseModifiers}` (winit::event::MouseButton already imported — ALIAS the core one). `encode_mouse` sig at term.rs:760.
3. **E5/E7 cursor** — core has NO cursor_position() getter and app draws NO terminal cursor. Add `pub fn cursor_position(&self)->(usize,usize)` to term.rs (+unit test) on branch feat/core-cursor → merge first; then app draws focused-leaf cursor ColorRect honoring cursor_shape (Block/Bar/Underline) + cursor_blink (Instant 530ms) + is_cursor_visible. Getters exist: is_cursor_visible()@680, cursor_shape()@735, cursor_blink()@741.
4. E16 graceful PTY exit (close pane when `session.is_alive()`==false @ session.rs:93); E14 Ctrl+1-9 tab switch; E9 URL ctrl-click (heuristic scan + open_path).
5. Core OSC branch: OSC52 write + take_clipboard_write() wired to a new write_os_clipboard() (inverse shell-out: Set-Clipboard/pbcopy/xclip -i); OSC4/10/11 query+response; OSC104; OSC9/777 notify; title stack CSI 22/23 t; Theme::from_itermcolors. Unit tests.
6. P2 quake --quake flag.

## DEFER w/ justification (final summary must state WHY)
- **D4 Win+Arrow snap — BLOCKED by offline env** (needs `windows` crate, uncached). Not a choice.
- BiDi/RTL — needs unicode-bidi (offline) + shaper reorder.
- HarfBuzz programming ligatures — cosmic-text already does basic shaping.
- Full kitty graphics protocol — Sixel already covers inline images.

## KEY FACTS (verified)
- App pkg name = `c0pl4nd` (NOT c0pl4nd-app). `cargo check -p c0pl4nd`.
- Grid: `c0pl4nd_core::{Cell{c,fg,bg,flags}, Color{Default,Indexed(u8),Rgb}, CellFlags{bold,italic,underline,inverse,strikeout}}`.
- Quad pipeline: `pane_render::{ChromeRenderer, ColorRect::new(x,y,w,h,[f32;4])}`; chrome quads drawn BEFORE glyphon text in render() (~line 2580-2890).
- `read_os_clipboard()` is placed just before `open_path()` in window.rs.
- Tool channel intermittently drops output — re-probe empties, never fabricate, verify merges across 3+ stable sources, reject sequential-looking hashes.
- Two delegated sub-agents stalled earlier (delivered nothing usable) — all shipped work was main-thread. Prefer main-thread for remaining items.
