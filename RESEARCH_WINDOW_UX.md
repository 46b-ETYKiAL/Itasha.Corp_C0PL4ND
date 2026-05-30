---
title: "Best-in-Class Terminal Window Management, UX/Chrome, Input & Rendering — Research Report"
plan_type: research
status: draft
audience: c0pl4nd terminal (Rust + winit 0.30 / wgpu 29 / glyphon)
last_updated: "2026-05-30"
comparison_set: [Ghostty, WezTerm, Warp, Alacritty, Windows Terminal, kitty, iTerm2]
---

# Best-in-Class Terminal Emulator — Window/UX/Input/Rendering Checklist + winit/wgpu/glyphon Implementation Patterns

> Scope: a single-process GPU terminal built on **Rust + winit 0.30 + wgpu (29) + glyphon**, frameless
> (`with_decorations(false)`) with a custom brand titlebar (per the existing c0pl4nd architecture). This
> report is a feature gap-checklist against the seven leading terminals plus concrete, current-API
> implementation guidance for four specific features.

## Research provenance & verification note

Network access in the research sandbox was an intermittent allowlist that reliably reached only
`raw.githubusercontent.com`. The following were **fetched and verified from primary source** during this
research:

- **glyphon README** (`grovesNL/glyphon`): confirms glyphon "is a library for rendering text with wgpu"
  built on `cosmic-text` (layout/raster) + `etagere` (atlas packing); and explicitly lists
  **"Rendering shapes other than text (e.g. rectangles)"** under *"Functionality which is likely out of
  scope for this project."* — https://github.com/grovesNL/glyphon (README, verified 2026-05-30)
- **glyphon `Cargo.toml` (main, verified 2026-05-30)**: `version = "0.11.0"`, `wgpu = "29.0.0"`,
  `cosmic-text = "0.18"`, `etagere = "0.3.0"`, MSRV `1.92`; dev-deps `winit = "0.30.12"`. This is an
  **exact match for c0pl4nd's wgpu-29 pin** — use glyphon 0.11.x, not the older 0.10/0.27 line. —
  https://raw.githubusercontent.com/grovesNL/glyphon/main/Cargo.toml
- **glyphon `examples/hello-world.rs`**: the canonical `TextRenderer::prepare(...)` then
  `encoder.begin_render_pass(...)` then `text_renderer.render(atlas, viewport, &mut pass)` flow —
  https://github.com/grovesNL/glyphon/blob/main/examples/hello-world.rs (verified 2026-05-30)

Additionally fetched/verified: **Ghostty features page** (ghostty.org/docs/features — multi-window/tabs/
splits incl. *zoom split*, themes w/ auto dark-light, ligatures, GPU render, Kitty graphics+keyboard
protocols, macOS Quick Terminal/quake, native tabs+splits, proxy icon, secure keyboard entry); **Ghostty
config reference** (window-decoration native/client-side/none, window-padding-x/y, per-split opacity,
light:/dark: theme syntax, key sequences `ctrl+a>n`, key tables/modal copy-mode, click-to-move-cursor);
**WezTerm** (broadcast-input is NOT native — Lua workaround; copy_mode h/j/k/l/v/y; LEADER key);
**docs.rs/winit `Window`** (HTTP 200, API names confirmed: `inner_size`, `outer_position` may
`Err(NotSupported)`, `is_maximized`, `with_inner_size`/`with_position`/`with_maximized`; note
`request_inner_size` un-maximizes). `learn.microsoft.com/.../wm-nchittest` returned 404 at fetch time
(MS doc URL churn); the Win32 message/return-value facts below are from the stable Win32 API and the
`learn.microsoft.com/.../inputdev/wm-nchittest` canonical path. Where a fact is version-sensitive it is
flagged inline.

---

# PART 1 — Best-in-Class Feature Checklist

Priority key: **P0** = table stakes / users will reject the app without it; **P1** = strongly expected by
power users (the c0pl4nd target audience); **P2** = differentiator / nice-to-have; **P3** = niche or
explicitly out-of-taste for a "ship LESS" product (listed for completeness).

Reference-terminal columns use: **G**=Ghostty, **W**=WezTerm, **Wa**=Warp, **A**=Alacritty,
**WT**=Windows Terminal, **k**=kitty, **i**=iTerm2.

---

## 1. Tabs

| Item | What / Why | Implemented by | Priority |
|---|---|---|---|
| **Tab bar (show/hide/auto)** | A row of tabs in-window; auto-hide when only one tab. Users expect single-window multi-session. | G, W, Wa, WT, k, i (Alacritty deliberately **omits** tabs — delegates to tmux) | **P0** |
| **New / close tab** | `Ctrl/Cmd+T`, `Ctrl/Cmd+W`. | All except A | **P0** |
| **Reorder tabs (drag)** | Drag to reposition; keyboard move (`Ctrl+Shift+PageUp/Down` on WT). | G, W, Wa, WT, k, i | **P1** |
| **Tab titles — auto from cwd/cmd** | Title follows the foreground process / OSC 0/1/2 / shell-integration cwd. Critical for orientation across many tabs. | All (via OSC 0/1/2 + shell integration). kitty/WezTerm have template syntax. | **P0** (OSC title) / **P1** (template) |
| **Manual tab rename** | User overrides the auto title; persists for the tab. | W, WT, k, i (G via keybind) | **P1** |
| **Tab switching keybinds** | `Ctrl/Cmd+1..9` jump-to-N, `Ctrl+Tab` / `Ctrl+PageUp/Down` next/prev, last-used toggle. | All except A | **P0** |
| **Tab close confirmation** | Confirm-on-close when a process is still running (`Ctrl-C`-able job vs ssh). | WT, i, G | **P2** |
| **Tab activity/bell indicator** | Visual marker when a background tab rings the bell or produces output. | W, k, i, WT | **P2** |
| **Tab color / icon** | Per-tab tint or icon (often from profile/cwd). | WT, i, W | **P3** |
| **Tab overflow behavior** | Scroll vs shrink vs dropdown list when tabs exceed width. | WT (dropdown), i, W | **P1** |

**Notes for c0pl4nd:** you already render a custom titlebar; the tab bar should live in (or just below)
that titlebar so the frameless chrome is one cohesive surface. OSC 0/1/2 title parsing is the P0 path to
auto-titles; cwd-from-OSC-7 (already implemented per the codebase) feeds a better default template.

---

## 2. Splits / Panes

| Item | What / Why | Implemented by | Priority |
|---|---|---|---|
| **Horizontal & vertical split** | Divide a tab into two panes; the core multiplexing primitive. | G, W, Wa, k, i (WT calls them panes; A none) | **P0** (for a power terminal) |
| **Arbitrary grid / nested splits** | Recursive split tree → grids. | G, W, k, i, WT | **P1** |
| **Focus navigation** | Move focus by direction (`Ctrl+Alt+Arrow` / `leader h/j/k/l`) and by cycle. | G, W, k, i, WT | **P0** (once splits exist) |
| **Resize panes** | Keyboard resize + mouse-drag the divider. | G, W, k, i, WT | **P0** |
| **Zoom / maximize pane (toggle)** | Temporarily make the focused pane full-tab; toggle back. | W, k, i, WT, G | **P1** |
| **Close pane** | `Ctrl+Shift+W` close focused pane; collapse tree. | All-with-panes | **P0** |
| **Broadcast / synchronize input** | Type once → all panes receive it (or a chosen set). Power-user fleet ops. | W, k, i, WT (i "Broadcast Input"; k "broadcast") | **P2** |
| **New pane inherits cwd** | Split inherits the focused pane's working dir (shell-integration / OSC 7). | G, W, k, i, WT | **P1** |
| **Pane swap / rotate** | Move panes around the layout. | W, k, i | **P3** |
| **Save / restore pane layout** | Named layouts; restore on launch. | W (workspaces), k (layouts/sessions), i | **P2/P3** |

**Taste note (ship-LESS):** splits are the single biggest power-user expectation that separates a "real"
terminal from a toy. Recommend P0 H/V split + focus-nav + resize + close; defer broadcast-input and
layout-save to P2/P3.

---

## 3. Window UX / Chrome

| Item | What / Why | Implemented by | Priority |
|---|---|---|---|
| **Native vs custom titlebar (config)** | Some users want OS-native window controls; others want a unified GPU chrome. Best-in-class lets users choose. | G (`window-decoration`), WT (custom), Wa (custom), i/W/k (native+options) | **P1** (you already ship custom; expose a `window-decoration = native\|custom\|none` toggle) |
| **Working OS window-snap on frameless** | Aero Snap (Win), tiling (Linux WM), Rectangle/native (mac). A custom titlebar that breaks Win+Arrow snapping is a top complaint. | WT, G (native decorations preserve snap; custom must reimplement WM_NCHITTEST) | **P0** (see Pattern 3) |
| **Quake / dropdown mode** | Global hotkey slides a borderless terminal down from a screen edge; toggle to hide. *The* signature power feature. | G (`quick-terminal`), iTerm2 (Hotkey Window), WT (Quake mode `Win+\``), kitty (via `--start-as`/3rd-party), W (3rd-party) | **P1** (strong differentiator) |
| **Fullscreen toggle** | `F11` / native fullscreen; borderless-fullscreen option. | All | **P0** |
| **Always-on-top** | Keep window above others. | i, W, k, WT (limited) | **P2** |
| **Opacity / transparency** | Window-level alpha; often with a separate "unfocused" opacity. | G, W, k, i, WT, A | **P1** |
| **Background blur (acrylic/mica)** | Blur-behind for transparent windows (Win acrylic/mica, mac vibrancy, KDE blur). | WT (acrylic/mica), i (mac blur), G, k | **P2** |
| **Background image** | Image/GIF behind the cells with opacity/scaling. | W, i, k, WT | **P3** (often out-of-taste) |
| **Window padding** | Inner padding (cells inset from edges); per-side. | G, W, k, i, A, WT | **P1** |
| **Restore size/position/maximized on launch** | Re-open where you left it; remember maximized. | WT, i, W, G | **P1** (see Pattern 2) |
| **New window** | `Ctrl/Cmd+N` / `Cmd+Shift+N`; new window inherits profile/cwd. | All | **P0** |
| **Multiple windows of one process** | Several top-level windows share one process for shared config/perf. | W, k, i, WT, G | **P1** |
| **Confirm-close window with running procs** | Guard against losing work. | WT, i, G | **P2** |
| **Per-monitor DPI correctness** | Crisp text after moving between monitors with different scale. | All (quality varies) | **P0** |

---

## 4. Theme / Config

| Item | What / Why | Implemented by | Priority |
|---|---|---|---|
| **Built-in theme catalog + switch** | Ship many curated palettes; switch by name. | G (~300+ themes bundled), W (~stock schemes + iTerm2 import), WT (built-ins), k (kitty-themes), i (presets) | **P1** |
| **Live config reload** | Edit config → applies without restart (watch file or hotkey reload). | G (auto on some platforms + `reload_config`), W (auto-reload), k (`load_config`/auto), A (live reload), WT (auto on save) | **P1** |
| **Color-scheme import (iTerm2 `.itermcolors`)** | The de-facto interchange format; thousands of schemes exist (iterm2colorschemes.com). | W (imports `.itermcolors`), G (theme files), k (conversion tools) | **P2** (huge ecosystem win for low effort) |
| **base16 / base24 support** | Standard 16/24-color scheme framework. | W, k (community), A (community) | **P3** |
| **Light/dark auto (follow OS)** | Switch palette with the OS appearance. | G (`theme = light:…,dark:…`), WT, i, k | **P2** |
| **Config validation surfaced in-app** | Bad config → in-window diagnostic, not a silent fallback or crash. | WT (settings UI validation), G (errors on reload), k | **P1** (see Pattern 4 + §13) |
| **Per-OS / conditional config** | `os == windows` blocks, includes. | W, k, G | **P3** |

---

## 5. Font

| Item | What / Why | Implemented by | Priority |
|---|---|---|---|
| **Size adjust hotkeys (`Ctrl +`/`-`/`0`)** | Grow/shrink/reset font live; the most-used quick adjust. | All | **P0** |
| **Per-window/tab font size persistence** | Adjusted size sticks for the session/profile. | i, WT, k, W | **P2** |
| **Font picker / family config** | Choose monospace family (config string at minimum; GUI picker is a plus). | All via config; WT/i have GUI pickers | **P1** (config) / **P3** (GUI picker) |
| **Line height / cell height adjust** | `line-height` / `adjust-cell-height` for readability. | G (`adjust-cell-height`), k (`modify_font`), W (`line_height`), i | **P2** |
| **Cell width adjust** | Tighten/loosen horizontal spacing; fix CJK/ligature width. | G (`adjust-cell-width`), k, W | **P3** |
| **Font fallback chain (config)** | Explicit fallback fonts for glyphs/emoji/CJK; per-style fallback. | k (`symbol_map`/fallback), W (`font_with_fallback`), G, i | **P1** |
| **Bold/italic/bold-italic faces** | Distinct faces (not synthetic) per style; synthetic as fallback. | All | **P0** |
| **Ligatures (programming)** | Render `=>` `!=` etc. as ligatures (HarfBuzz shaping). | W, k, i, G, WT (partial) | **P2** (cosmic-text/glyphon: shaping support varies — verify) |
| **Variable-font weight / features** | OpenType features (`calt`, `ss01`), variable axes. | k, i, W | **P3** |
| **Emoji / color glyphs** | COLR/CBDT color emoji rendering. | All | **P1** |
| **Box-drawing / Powerline crispness** | Native-drawn box/Powerline glyphs so they tile seamlessly at any size. | G (built-in box-drawing), k, WT | **P2** |

> **glyphon constraint (verified):** glyphon uses cosmic-text for layout+rasterization. Programming
> ligatures and complex shaping depend on cosmic-text's shaping; color-emoji and fallback depend on the
> configured `FontSystem`. Confirm the cosmic-text version's ligature support for your target before
> promising P2 ligatures.

---

## 6. Cursor

| Item | What / Why | Implemented by | Priority |
|---|---|---|---|
| **Shape (block / bar / underline)** | Config default + DECSCUSR (`CSI Ps SP q`) from apps (vim modes). | All | **P0** |
| **Blink (on/off, rate)** | Config + DECSCUSR blink variants; respect `Cursor blinking` honor-app setting. | All | **P0** |
| **Cursor color (+ text-under color)** | Distinct cursor color; "reverse" or fixed. | All | **P1** |
| **Unfocused hollow / dim cursor** | Hollow outline when window/pane loses focus (clarity in splits). | i, k, W, G | **P1** |
| **Cursor trail / animation** | Animated cursor motion (a 2025 trend). | WT (experimental shader), Wa, kitty (cursor_trail) | **P3** |

---

## 7. Keybinds

| Item | What / Why | Implemented by | Priority |
|---|---|---|---|
| **Full remap of every action** | Any action bindable to any chord; a power-terminal baseline. | G, W, k, i, WT, A (subset) | **P0** |
| **Leader / prefix key** | tmux-style prefix (`Ctrl+A` then key) for split/tab actions without global-chord collisions. | k (`map kitty_mod`), W (`leader`), G | **P1** |
| **Multi-key chords / sequences** | `Ctrl+W` then `s` (key sequences). | W, k, G | **P2** |
| **Per-OS default keymaps** | Cmd-based on mac, Ctrl/Ctrl+Shift on Win/Linux, out of the box. | All | **P0** |
| **Copy mode / vi keys** | Keyboard-only selection & scrollback navigation (vi motions, search). | W (copy_mode), k (scrollback w/ pager), i, G (partial) | **P1** |
| **Action discoverability (list / palette)** | A searchable command palette of all bindable actions. | WT (command palette), Wa, i, G (`quick-terminal`/actions) | **P1** (you already ship a palette) |
| **"Send text"/macro bindings** | Bind a key to emit a string / run an action chain. | W, k, i, WT | **P2** |
| **Conditional bindings (mode/app aware)** | Bindings active only in copy mode / when app requests. | W, k | **P3** |
| **Keybind for config reload / debug overlay** | `Ctrl+Shift+,` reload; toggle perf overlay. | k, G, W | **P2** |

---

## 8. Mouse UX

| Item | What / Why | Implemented by | Priority |
|---|---|---|---|
| **Selection (word/line/block)** | Single/double/triple-click word/line; rectangular (block) selection with modifier. | All (block via Alt/Ctrl) | **P0** |
| **Copy-on-select / middle-click paste** | X11/Linux convention; optional on Win/mac. | All (configurable) | **P1** |
| **`Ctrl`/`Cmd`-click to open URL** | Detect URLs; modifier-click opens in browser (avoids accidental opens). | All (OSC 8 + heuristic detection) | **P0** |
| **Hover underline on URL** | Underline link under cursor; cursor → pointer. | k, i, W, WT, G | **P1** |
| **OSC 8 explicit hyperlinks** | Apps emit real hyperlinks (ls --hyperlink, gcc). | k (originated), W, i, G, WT | **P1** |
| **Mouse reporting passthrough** | Forward mouse to TUIs (vim/tmux) when they enable mouse mode; modifier to override for local selection. | All | **P0** |
| **Selection modifiers** | `Shift` to force local selection while app grabs mouse; `Alt` rectangular. | All | **P1** |
| **Click-to-move-cursor (shell integration)** | Click in the prompt line moves the readline cursor (needs shell integration). | i, Wa, W (cmd-click "jump") | **P3** (nice, niche) |
| **Scroll wheel → scrollback / app** | Wheel scrolls scrollback in normal mode, sends arrows in alt-screen apps (configurable). | All | **P0** |

---

## 9. Notifications

| Item | What / Why | Implemented by | Priority |
|---|---|---|---|
| **Audible bell (`BEL`/`\a`)** | Optional sound on bell. | All (off by default often) | **P1** |
| **Visual bell (flash)** | Flash the pane/window instead of/with sound. | All | **P1** |
| **OSC 9 desktop notification** | iTerm2-style `ESC]9;message BEL` → OS toast. | i, WT, k (also OSC 99), W | **P2** |
| **OSC 777 notify** | urxvt/`OSC 777;notify;title;body` → OS toast. | W, k, others | **P3** |
| **OSC 99 (kitty notifications)** | Rich notifications (kitty protocol): title/body/icon/actions. | k, (adopters growing) | **P3** |
| **Attention / urgency / taskbar flash** | Set WM urgency hint / flash taskbar / bounce dock on bell when unfocused. | All | **P2** |
| **"Command finished" notification** | Notify when a long command completes while unfocused (shell integration). | i, Wa, WT (w/ integration) | **P2** |

---

## 10. Profiles / Sessions

| Item | What / Why | Implemented by | Priority |
|---|---|---|---|
| **Named profiles** | Each profile = font/theme/startup-cmd/cwd; pick on new tab/window. | WT, i, W (domains), G (limited) | **P1** |
| **Startup command / program per profile** | Launch a specific shell/program (wsl, ssh, ps). | WT, i, W, k | **P1** |
| **Default working directory** | Profile default cwd; "open new tab here". | WT, i, W, G, k | **P1** |
| **cwd inheritance on split/new-tab** | New pane/tab inherits focused cwd (OSC 7 / shell integration). | G, W, k, i, WT | **P1** |
| **Session save / restore** | Persist open tabs/panes/cwd; restore on launch or on demand. | k (sessions file), W (workspaces + resurrect), i (restore), WT (restore last) | **P2** |
| **Quick "duplicate tab/pane"** | New tab/pane cloning current profile+cwd. | i, WT, W, k | **P2** |
| **Profile per OSC / automatic profile switching** | Switch profile when cwd/host changes (iTerm2 "Automatic Profile Switching"). | i | **P3** |

---

## 11. Performance

| Item | What / Why | Implemented by | Priority |
|---|---|---|---|
| **GPU rendering** | Glyph-atlas + quad rendering on GPU; baseline for modern terminals. | G, W, A, k, WT, Wa (all GPU) | **P0** (you have it) |
| **Render-on-change (damage / dirty)** | Only redraw when the grid changed; don't repaint at vsync when idle. Saves CPU/GPU/battery. | A, G, k, W | **P0** |
| **Dirty-region / cell-level damage tracking** | Re-rasterize/re-upload only changed cells/rows; minimal atlas churn. | A (damage tracking), k, G | **P1** |
| **VSync / present-mode config** | Fifo (vsync) vs Mailbox/Immediate (low-latency). Expose a toggle. | A, k (`sync_to_monitor`), G | **P1** |
| **Large-output throughput (`cat bigfile`)** | Coalesce PTY reads; cap redraws per frame; don't render every intermediate frame. Classic benchmark. | A (top), k, G, W | **P0** |
| **Large-paste handling / bracketed paste** | Chunk huge pastes; bracketed paste; confirm multi-line paste (security). | All | **P0** |
| **Scroll performance** | Smooth scrollback; GPU scroll; momentum. | k, G, i, WT | **P1** |
| **Frame pacing / latency target** | Minimize input-to-photon latency; key-to-render fast path; some redraw immediately on keypress. | A, k, G | **P1** |
| **Atlas eviction / growth** | Grow/evict glyph atlas without stalls; cap memory. | k, G, A | **P1** (glyphon `TextAtlas` handles packing; mind growth) |
| **Idle throttling / occluded skip** | Lower FPS or skip render when occluded/minimized/unfocused. | k, G | **P2** |
| **Multi-GPU / adapter selection** | Pick discrete vs integrated GPU. | k, WT (via OS) | **P3** |

> **glyphon/wgpu note:** glyphon re-`prepare`s text each frame; for render-on-change you gate the entire
> `RequestRedraw` on a "grid dirty" flag and only call `prepare` when content/viewport changed. The
> `TextAtlas` + `Viewport` are persistent; recreate the `Viewport` resolution on resize only.

---

## 12. Accessibility

| Item | What / Why | Implemented by | Priority |
|---|---|---|---|
| **High-contrast theme(s)** | Ship a high-contrast palette; honor OS high-contrast mode. | WT (honors OS HC), i, G | **P1** |
| **Minimum-contrast enforcement** | Auto-bump fg/bg contrast ratio so low-contrast app colors stay legible (iTerm2 "Minimum Contrast", kitty `text_contrast`). | i (Minimum Contrast), k (`text_contrast`), WT | **P2** |
| **Screen-reader hooks (UIA/AT-SPI/AX)** | Expose terminal text to screen readers (Windows Terminal pioneered UI Automation for terminals). | WT (UIA — best in class), i (mac AX, partial) | **P2** (hard; WT is the bar) |
| **Respects OS reduce-motion** | Disable cursor blink/animations under reduce-motion. | WT, i (partial) | **P2** |
| **Scalable UI / font for low vision** | Large fonts, UI scaling independent of cell font. | WT, i | **P2** |
| **Colorblind-safe defaults / palettes** | Provide CVD-friendly schemes. | community schemes | **P3** |

> Screen-reader support for a custom-GPU-drawn terminal is genuinely hard: GPU text has no accessibility
> tree. WT solved it with a parallel UI Automation text provider. For c0pl4nd, realistic P1 = high-contrast
> theme + min-contrast enforcement; full SR support is a large P2/P3 effort.

---

## 13. Stability / Quality

| Item | What / Why | Implemented by | Priority |
|---|---|---|---|
| **Graceful PTY/child exit** | When the shell exits: close pane/tab, or hold-open with a status line + "press key to close" (config). | G (`wait-after-command`), k (`close_on_child_death`), W, i, WT | **P0** |
| **Config validation surfaced in-app** | Parse errors shown in a window/overlay with line numbers; fall back to defaults *and tell the user*. | WT, G (reload errors), k | **P1** |
| **Crash recovery / session restore** | Restore tabs/panes/cwd after a crash or update. | i, WT, k, W | **P2** |
| **No-config-loss on bad reload** | A bad live-reload keeps the last-good config rather than blanking. | G, k | **P1** |
| **Update mechanism + integrity** | Signed auto-update (you already ship minisign-verified updates). | G, WT (store), i (Sparkle) | **P1** |
| **Robust resize (no garbage)** | Reflow scrollback or re-layout cleanly on resize; no flicker/garbage. | All (quality varies) | **P0** |
| **Sane defaults / zero-config usable** | Works great with no config; the Ghostty/Alacritty thesis. | G, A | **P0** |
| **Detached / daemon mode** | Server process survives window close (WezTerm mux, kitty `--single-instance`). | W, k | **P3** |

---

# PART 2 — Implementation Patterns (winit 0.30 / wgpu / glyphon)

## Pattern 1 — Solid-color quads BEHIND glyphon text (caption-button hover "backplates")

### The constraint (verified from primary source)

glyphon **renders text only**. Its README explicitly lists *"Rendering shapes other than text (e.g.
rectangles)"* under **out of scope** (https://github.com/grovesNL/glyphon, verified). So you cannot ask
glyphon to draw a hover rectangle; you must draw the rectangle yourself with your own wgpu pipeline.

### Recommended approach: your own tiny quad pipeline, drawn into the **same render pass, before** glyphon

glyphon's draw entry point is `TextRenderer::render(&self, atlas, viewport, pass: &mut RenderPass)` — it
takes a render pass the *caller* owns (verified in `examples/hello-world.rs`: the example calls
`encoder.begin_render_pass(...)` then `text_renderer.render(atlas, viewport, &mut pass)`). Because you own
the pass, you simply issue your quad draw calls into that same pass *first*, then call glyphon's `render`.
Painter's-algorithm ordering (no depth buffer needed for 2D UI): **clear → quads (backplates) → text**.

```rust
// One-time setup -------------------------------------------------------------

// Vertex: position in clip space (NDC) + RGBA color. Keep it dead simple.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct QuadVertex { pos: [f32; 2], color: [f32; 4] }

// WGSL: passthrough vertex, flat-color fragment, premultiplied alpha blend.
const QUAD_WGSL: &str = r#"
struct VsOut { @builtin(position) clip: vec4<f32>, @location(0) color: vec4<f32> };
@vertex fn vs(@location(0) pos: vec2<f32>, @location(1) color: vec4<f32>) -> VsOut {
    var o: VsOut; o.clip = vec4<f32>(pos, 0.0, 1.0); o.color = color; return o;
}
@fragment fn fs(in: VsOut) -> @location(0) vec4<f32> { return in.color; }
"#;

let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
    label: Some("quad"),
    source: wgpu::ShaderSource::Wgsl(QUAD_WGSL.into()),
});

let quad_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
    label: Some("quad-pipeline"),
    layout: None, // no bind groups needed; color is per-vertex
    vertex: wgpu::VertexState {
        module: &shader, entry_point: Some("vs"), compilation_options: Default::default(),
        buffers: &[wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<QuadVertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x4],
        }],
    },
    fragment: Some(wgpu::FragmentState {
        module: &shader, entry_point: Some("fs"), compilation_options: Default::default(),
        targets: &[Some(wgpu::ColorTargetState {
            format: surface_config.format,                 // MUST match the glyphon/text target format
            blend: Some(wgpu::BlendState::ALPHA_BLENDING), // or PREMULTIPLIED_ALPHA_BLENDING
            write_mask: wgpu::ColorWrites::ALL,
        })],
    }),
    primitive: wgpu::PrimitiveState::default(), // triangle-list
    depth_stencil: None,                        // 2D UI: rely on draw order, not depth
    multisample: wgpu::MultisampleState::default(), // keep == glyphon's MultisampleState
    multiview: None, cache: None,
});
```

```rust
// Per-frame: build the backplate quads (e.g. a hover rect for a caption button) ----

fn rect_to_quad(px: Rect, screen_w: f32, screen_h: f32, c: [f32;4]) -> [QuadVertex; 6] {
    // pixel-space rect -> NDC (note Y flips: NDC +Y is up)
    let x0 =  px.x        / screen_w * 2.0 - 1.0;
    let x1 = (px.x+px.w)  / screen_w * 2.0 - 1.0;
    let y0 = 1.0 - (px.y)        / screen_h * 2.0;
    let y1 = 1.0 - (px.y+px.h)   / screen_h * 2.0;
    let tl=[x0,y0]; let tr=[x1,y0]; let bl=[x0,y1]; let br=[x1,y1];
    [QuadVertex{pos:tl,color:c}, QuadVertex{pos:bl,color:c}, QuadVertex{pos:br,color:c},
     QuadVertex{pos:tl,color:c}, QuadVertex{pos:br,color:c}, QuadVertex{pos:tr,color:c}]
}
// Upload all quad vertices into one growable vertex buffer (queue.write_buffer or a staging belt).
```

```rust
// Render: ONE pass, quads first, then glyphon text  (wgpu 29 / glyphon 0.11 API) ----

// 0) update viewport resolution (do this when surface size changed)
viewport.update(&queue, glyphon::Resolution { width: cfg.width, height: cfg.height });

// 1) prepare glyphon text for this frame (verified arg order from hello-world.rs)
text_renderer.prepare(&device, &queue, &mut font_system, &mut atlas, &viewport,
                      text_areas, &mut swash_cache).unwrap();

// wgpu 29: get_current_texture() returns the CurrentSurfaceTexture enum — match it.
let frame = match surface.get_current_texture() {
    wgpu::CurrentSurfaceTexture::Success(f) => f,
    wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Suboptimal(_) => {
        surface.configure(&device, &cfg); window.request_redraw(); return;
    }
    _ => { window.request_redraw(); return; } // Timeout/Occluded/Lost/Validation handling
};
let frame_view = frame.texture.create_view(&Default::default());
let mut encoder = device.create_command_encoder(&Default::default());
{
    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("ui"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: &frame_view, resolve_target: None,
            depth_slice: None,                       // wgpu 29 field
            ops: wgpu::Operations { load: wgpu::LoadOp::Clear(bg), store: wgpu::StoreOp::Store },
        })],
        depth_stencil_attachment: None, timestamp_writes: None,
        occlusion_query_set: None, multiview_mask: None,   // wgpu 29 field
    });

    // 2) BACKPLATES first
    pass.set_pipeline(&quad_pipeline);
    pass.set_vertex_buffer(0, quad_vbuf.slice(..));
    pass.draw(0..quad_vertex_count, 0..1);

    // 3) TEXT on top — same pass
    text_renderer.render(&atlas, &viewport, &mut pass).unwrap();
}
queue.submit(Some(encoder.finish()));
frame.present();
atlas.trim(); // free unused atlas allocations each frame (verified in hello-world.rs)
```

**Why not a library:** for flat-color rects you do not need `lyon`, `vello`, or `egui`. A ~25-line WGSL
shader + a per-vertex-color triangle pipeline is the smallest correct thing and avoids a heavy dependency
(consistent with the project's "ship LESS / glyphon-direct" stance). If you later need rounded corners,
do it in the fragment shader with an SDF on a single quad (pass rect + radius as instance data) rather
than adding a vector-graphics crate.

**Gotchas:**
- The quad pipeline's `format` and `MultisampleState` must match glyphon's (glyphon uses
  `MultisampleState::default()` = 1 sample in the example). Mismatch = pipeline/pass validation error.
- Use premultiplied alpha consistently if your surface alpha mode is premultiplied (transparency window).
- Draw order is your z-order; no depth buffer for 2D chrome. If you want text *behind* a translucent
  panel, draw text first then the panel quad — but that needs two `prepare`/`render` partitions or two
  passes; for simple hover backplates, quads-then-text is correct.

Refs: glyphon README & `examples/hello-world.rs`
(https://github.com/grovesNL/glyphon ; https://github.com/grovesNL/glyphon/blob/main/examples/hello-world.rs),
wgpu `RenderPipeline`/`RenderPass` (https://docs.rs/wgpu/latest/wgpu/).

---

## Pattern 2 — Persist & restore window size / position / maximized (winit 0.30)

### APIs (winit 0.30)

- **Read on save** (call when the window is in a stable state, e.g. before exit / on focus-lost / debounced
  on moved/resized):
  - `Window::outer_position() -> Result<PhysicalPosition<i32>, NotSupportedError>` — top-left of the window
    *including* decorations (for a frameless window, outer ≈ inner). Returns `Err` on some platforms
    (notably Wayland, which forbids reading absolute position) — handle the error, don't unwrap.
  - `Window::inner_size() -> PhysicalSize<u32>` — client area.
  - `Window::is_maximized() -> bool`.
  - `Window::scale_factor() -> f64` and `Window::current_monitor()` — persist these alongside so you can
    sanity-check on restore.
- **Apply on launch** via `WindowAttributes` (winit 0.30 creates windows in `ApplicationHandler::resumed`
  through `ActiveEventLoop::create_window(attrs)`):
  - `WindowAttributes::with_inner_size(Size)`,
    `with_position(Position)`,
    `with_maximized(bool)`,
    `with_decorations(false)` (your frameless mode).

### Persist the *logical* size + monitor identity, not raw physical pixels

Persisting physical pixels alone breaks across DPI changes. Best practice:

1. Save **logical size** (`physical / scale_factor`) + **logical position relative to the monitor's origin**
   + a monitor identifier (name/size/position from `MonitorHandle`).
2. On restore, find a *currently connected* monitor matching the saved identity; if none matches, fall
   back to the primary monitor and clamp.

### Restore with multi-monitor + DPI safety (clamp to a connected monitor)

```rust
#[derive(serde::Serialize, serde::Deserialize)]
struct WindowState {
    // logical units
    x: f64, y: f64, w: f64, h: f64,
    maximized: bool,
    monitor_name: Option<String>,       // MonitorHandle::name()
    monitor_pos: (i32, i32),            // physical origin of that monitor when saved
    monitor_size: (u32, u32),           // physical size when saved
}

fn save_state(window: &winit::window::Window) -> Option<WindowState> {
    let sf = window.scale_factor();
    let mon = window.current_monitor()?;
    let mpos = mon.position();           // PhysicalPosition<i32>
    let msize = mon.size();             // PhysicalSize<u32>
    let pos = window.outer_position().ok()?;        // may Err on Wayland
    let size = window.inner_size();
    Some(WindowState {
        x: (pos.x - mpos.x) as f64 / sf,            // relative to monitor, logical
        y: (pos.y - mpos.y) as f64 / sf,
        w: size.width as f64 / sf,
        h: size.height as f64 / sf,
        maximized: window.is_maximized(),
        monitor_name: mon.name(),
        monitor_pos: (mpos.x, mpos.y),
        monitor_size: (msize.width, msize.height),
    })
}

fn attrs_from_state(
    event_loop: &winit::event_loop::ActiveEventLoop,
    s: &WindowState,
) -> winit::window::WindowAttributes {
    use winit::dpi::{LogicalSize, LogicalPosition};
    use winit::window::WindowAttributes;

    // 1) find a matching, currently-connected monitor (by name, else by geometry)
    let monitors: Vec<_> = event_loop.available_monitors().collect();
    let target = monitors.iter()
        .find(|m| m.name() == s.monitor_name && s.monitor_name.is_some())
        .or_else(|| monitors.iter().find(|m| {
            let p = m.position(); let sz = m.size();
            (p.x, p.y) == s.monitor_pos && (sz.width, sz.height) == s.monitor_size
        }))
        .cloned()
        .or_else(|| event_loop.primary_monitor());

    let mut attrs = WindowAttributes::default()
        .with_decorations(false)                    // frameless
        .with_maximized(s.maximized);

    if let Some(mon) = target {
        let sf = mon.scale_factor();
        let mpos = mon.position();
        let msize = mon.size();

        // logical -> physical on the TARGET monitor's scale factor
        let mut px = mpos.x + (s.x * sf).round() as i32;
        let mut py = mpos.y + (s.y * sf).round() as i32;
        let mut pw = (s.w * sf).round() as u32;
        let mut ph = (s.h * sf).round() as u32;

        // 2) clamp size to the monitor; ensure the titlebar stays grabbable
        pw = pw.min(msize.width);
        ph = ph.min(msize.height);
        let max_x = mpos.x + msize.width as i32 - 64;   // keep >=64px on screen
        let max_y = mpos.y + msize.height as i32 - 32;
        px = px.clamp(mpos.x, max_x.max(mpos.x));
        py = py.clamp(mpos.y, max_y.max(mpos.y));

        attrs = attrs
            .with_position(winit::dpi::PhysicalPosition::new(px, py))
            .with_inner_size(winit::dpi::PhysicalSize::new(pw, ph));
    } else {
        attrs = attrs.with_inner_size(LogicalSize::new(s.w.max(400.0), s.h.max(300.0)));
    }
    attrs
}
```

**Key safety rules:**
- **Never `unwrap()` `outer_position()`** — it returns `Err(NotSupportedError)` on Wayland (absolute
  positioning is forbidden). On Wayland, persist only size + maximized; let the compositor place it.
- **If `maximized == true`, also store the restored (un-maximized) bounds.** Win+restore semantics: set
  `with_maximized(true)` but keep a sensible inner-size for when the user un-maximizes (winit will restore
  to the attrs size).
- **Re-validate after creation in the first `Resumed`/`ScaleFactorChanged`:** monitors can change between
  save and restore. After the window exists, you can additionally call `set_outer_position` /
  `request_inner_size` if your clamp logic needs the live monitor list.
- **Persist scale factor** so you can detect a DPI change and reconvert via logical units (above), which
  is the whole reason to store logical not physical.
- Save trigger: debounce on `WindowEvent::Moved` / `Resized`, and always re-save on
  `WindowEvent::CloseRequested` / loop-exit.

Refs: winit `Window` (`outer_position`, `inner_size`, `is_maximized`, `scale_factor`,
`current_monitor`) and `WindowAttributes` (`with_position`, `with_inner_size`, `with_maximized`,
`with_decorations`) — https://docs.rs/winit/0.30/winit/window/struct.Window.html ;
https://docs.rs/winit/0.30/winit/window/struct.WindowAttributes.html ;
`MonitorHandle` https://docs.rs/winit/0.30/winit/monitor/struct.MonitorHandle.html ;
Wayland position limitation discussed in rust-windowing/winit issues (e.g. #1879).

---

## Pattern 3 — Win+Arrow Aero Snap on a frameless winit window (Windows)

### The problem

When you create a winit window with `with_decorations(false)`, winit drops the non-client frame styles.
On Windows the OS *only* offers Aero Snap (Win+Arrow, drag-to-edge, snap layouts) to windows that have the
right window styles in their non-client area and that respond correctly to non-client hit-testing and
min/max-info messages. A naive frameless window loses snapping, drag-from-titlebar, and the resize
borders. The fix is to keep the OS thinking the window is "real" (sizable, has a caption for restore
semantics) while you paint the whole client area yourself.

### What Windows requires (Win32 facts)

1. **Window styles must include `WS_THICKFRAME` (sizable) and `WS_CAPTION` for snap + restore + animations.**
   `WS_THICKFRAME` gives resize borders and is what makes Aero Snap available; `WS_CAPTION` participates in
   maximize/restore + Snap Layouts. Keep `WS_MAXIMIZEBOX | WS_MINIMIZEBOX` too so the Snap Layouts flyout
   (hover the maximize hot-corner / Win+Z) works and Win+Up/Down maximize/minimize work.
   - Caption-bar concepts: WM_NCCALCSIZE / non-client area
     (https://learn.microsoft.com/en-us/windows/win32/dwm/customframe).
2. **Remove the visible frame WITHOUT removing the styles — extend the frame into the client area.**
   Use `DwmExtendFrameIntoClientArea` with a zero/negative margin and **handle `WM_NCCALCSIZE`** (return
   the client rect unchanged, i.e. report no non-client area) so there's no visible title bar/border but
   the window is still a "framed" sizable window the snap system recognizes.
   - `DwmExtendFrameIntoClientArea`:
     https://learn.microsoft.com/en-us/windows/win32/api/dwmapi/nf-dwmapi-dwmextendframeintoclientarea
   - `WM_NCCALCSIZE`:
     https://learn.microsoft.com/en-us/windows/win32/winmsg/wm-nccalcsize
3. **`WM_NCHITTEST`: report your custom titlebar region as `HTCAPTION` and the outer edges as
   `HTLEFT/HTRIGHT/HTTOP/HTBOTTOM/HTTOPLEFT/...`** so the OS drives drag-to-snap and edge-resize. Returning
   `HTCAPTION` over your fake titlebar is what enables drag-to-top-to-maximize and drag-to-edge snapping;
   returning the H-edge codes enables OS-native resize (and thus the resize cursors).
   - `WM_NCHITTEST`:
     https://learn.microsoft.com/en-us/windows/win32/inputdev/wm-nchittest
   - Hit-test return values (`HTCAPTION`, `HTLEFT`, ...):
     https://learn.microsoft.com/en-us/windows/win32/inputdev/wm-nchittest#return-value
4. **`WM_GETMINMAXINFO`: set max size/position so a maximized frameless window doesn't cover the taskbar
   and doesn't overflow the monitor.** Frameless windows commonly maximize *over* the taskbar because the
   default max tracking size assumes a frame. Compute the work area
   (`MonitorFromWindow` + `GetMonitorInfo` → `rcWork`) and set `ptMaxSize`/`ptMaxPosition`.
   - `WM_GETMINMAXINFO`:
     https://learn.microsoft.com/en-us/windows/win32/winmsg/wm-getminmaxinfo
   - `GetMonitorInfo`/`MONITORINFO.rcWork`:
     https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-getmonitorinfo

### How to hook winit's window on Windows

winit 0.30 does not expose these messages, so subclass the HWND. Two routes:

- **Route A (recommended): set a subclass via `SetWindowSubclass`** (comctl32) on the HWND obtained from
  `raw_window_handle`. This composes cleanly with winit's own WndProc (your subclass proc runs first;
  call `DefSubclassProc` to chain to winit).
- **Route B: `SetWindowLongPtr(GWLP_WNDPROC, ...)`** to replace the proc and call the saved old proc via
  `CallWindowProc`. Works but more fragile re: ordering than subclassing.

```rust
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use windows::Win32::Foundation::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::Win32::UI::Shell::{SetWindowSubclass, DefSubclassProc};
use windows::Win32::Graphics::Dwm::DwmExtendFrameIntoClientArea;
use windows::Win32::UI::Controls::MARGINS;

const TITLEBAR_H: i32 = 36;     // your custom titlebar height (logical*scale)
const RESIZE_BORDER: i32 = 8;   // edge grab thickness

unsafe fn make_snappable(window: &winit::window::Window) {
    let RawWindowHandle::Win32(h) = window.window_handle().unwrap().as_raw() else { return; };
    let hwnd = HWND(h.hwnd.get() as *mut _);

    // 1) Ensure sizable + caption styles are present (winit may have dropped them).
    let style = GetWindowLongPtrW(hwnd, GWL_STYLE) as u32;
    let style = style | WS_THICKFRAME.0 | WS_CAPTION.0 | WS_MAXIMIZEBOX.0 | WS_MINIMIZEBOX.0;
    SetWindowLongPtrW(hwnd, GWL_STYLE, style as isize);

    // 2) Hide the visible frame but keep it a "framed" window: zero DWM margins +
    //    WM_NCCALCSIZE returns client rect unchanged (handled in subclass below).
    let m = MARGINS { cxLeftWidth: 0, cxRightWidth: 0, cyTopHeight: 0, cyBottomHeight: 0 };
    let _ = DwmExtendFrameIntoClientArea(hwnd, &m);

    // 3) Tell the system the frame changed so styles take effect.
    let _ = SetWindowPos(hwnd, None, 0,0,0,0,
        SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);

    // 4) Subclass to intercept the NC messages.
    let _ = SetWindowSubclass(hwnd, Some(subclass_proc), 1, 0);
}

unsafe extern "system" fn subclass_proc(
    hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM,
    _id: usize, _data: usize,
) -> LRESULT {
    match msg {
        // Remove the non-client area: report client == window so no titlebar/border is drawn,
        // but the window is still WS_THICKFRAME (snap-eligible).
        WM_NCCALCSIZE if wparam.0 != 0 => {
            // Returning 0 keeps the proposed client rect = full window rect.
            // (For maximized state add an inset so it doesn't bleed off-screen; see note.)
            LRESULT(0)
        }
        // Drive snapping/resizing by classifying the cursor location ourselves.
        WM_NCHITTEST => {
            let mut pt = POINT { x: (lparam.0 & 0xffff) as i16 as i32,
                                 y: ((lparam.0 >> 16) & 0xffff) as i16 as i32 };
            let mut rc = RECT::default(); let _ = GetWindowRect(hwnd, &mut rc);
            let (l, t, r, b) = (rc.left, rc.top, rc.right, rc.bottom);
            let on_left   = pt.x < l + RESIZE_BORDER;
            let on_right  = pt.x >= r - RESIZE_BORDER;
            let on_top    = pt.y < t + RESIZE_BORDER;
            let on_bottom = pt.y >= b - RESIZE_BORDER;
            let hit = match (on_top, on_bottom, on_left, on_right) {
                (true, _, true, _)  => HTTOPLEFT,
                (true, _, _, true)  => HTTOPRIGHT,
                (_, true, true, _)  => HTBOTTOMLEFT,
                (_, true, _, true)  => HTBOTTOMRIGHT,
                (true, ..)          => HTTOP,
                (_, true, ..)       => HTBOTTOM,
                (_, _, true, _)     => HTLEFT,
                (_, _, _, true)     => HTRIGHT,
                _ if pt.y < t + TITLEBAR_H => {
                    // Over the custom titlebar -> HTCAPTION drives drag-move + drag-snap.
                    // BUT: return HTCLIENT for areas where YOUR caption buttons / tabs live,
                    // so clicks reach winit (see note on caption buttons).
                    HTCAPTION
                }
                _ => HTCLIENT,
            };
            LRESULT(hit as isize)
        }
        // Keep a maximized frameless window inside the work area (off the taskbar).
        WM_GETMINMAXINFO => {
            let hmon = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
            let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
            if GetMonitorInfoW(hmon, &mut mi).as_bool() {
                let mmi = &mut *(lparam.0 as *mut MINMAXINFO);
                let work = mi.rcWork; let area = mi.rcMonitor;
                mmi.ptMaxPosition.x = (work.left  - area.left).max(0);
                mmi.ptMaxPosition.y = (work.top   - area.top ).max(0);
                mmi.ptMaxSize.x     = work.right  - work.left;
                mmi.ptMaxSize.y     = work.bottom - work.top;
            }
            LRESULT(0)
        }
        _ => DefSubclassProc(hwnd, msg, wparam, lparam),
    }
}
```

**Critical caveats / known issues:**
- **Caption buttons vs `HTCAPTION`:** if you return `HTCAPTION` over your custom min/max/close buttons,
  Windows will start a window-drag and your buttons won't get click events. Return `HTCLIENT` for the
  button hit-rects (and for the tab strip) so winit delivers `MouseInput`; only return `HTCAPTION` for the
  empty draggable strip. (This is exactly why your hover-backplate quads in Pattern 1 matter — you draw +
  handle the buttons yourself in client space.)
- **Snap Layouts hover (Win11):** to get the Snap Layouts flyout when hovering your custom maximize
  button, handle `WM_NCHITTEST` returning `HTMAXBUTTON` over that button's rect, and respond to
  `WM_NCMOUSELEAVE`/`WM_NCLBUTTONDOWN`/`WM_NCLBUTTONUP` for `HTMAXBUTTON` to perform maximize. (This is the
  Win11 caption-button contract for custom frames.)
- **`WM_NCCALCSIZE` maximized inset:** when maximized, returning the full window rect causes ~8px to bleed
  off every monitor edge. Detect maximized state and subtract the resize-frame thickness
  (`GetSystemMetricsForDpi(SM_CXSIZEFRAME)+SM_CXPADDEDBORDER`).
- **winit interaction:** winit 0.30 added `Window::drag_window()` and `Window::drag_resize_window(dir)` —
  these are a *portable* alternative to a custom WndProc for move/resize-by-drag, but they do **not** give
  you full Aero Snap fidelity (no edge-snap previews, no Snap Layouts). For true Win11 snapping you need
  the subclass approach above. Track upstream: rust-windowing/winit has long-standing discussion on
  decoration-less snapping (e.g. issues around `WS_THICKFRAME`/custom decorations — search the winit issue
  tracker for "snap" / "WS_THICKFRAME" / "custom decorations").
- **`raw-window-handle` version must match winit's** (winit 0.30 → raw-window-handle 0.6) and the
  `windows` crate version is your choice; use `windows`/`windows-sys` directly for the Win32 calls.

References (Microsoft): WM_NCHITTEST
(https://learn.microsoft.com/en-us/windows/win32/inputdev/wm-nchittest), WM_NCCALCSIZE
(https://learn.microsoft.com/en-us/windows/win32/winmsg/wm-nccalcsize), WM_GETMINMAXINFO
(https://learn.microsoft.com/en-us/windows/win32/winmsg/wm-getminmaxinfo), DwmExtendFrameIntoClientArea
(https://learn.microsoft.com/en-us/windows/win32/api/dwmapi/nf-dwmapi-dwmextendframeintoclientarea),
Custom window frame walkthrough
(https://learn.microsoft.com/en-us/windows/win32/dwm/customframe), Window styles
(https://learn.microsoft.com/en-us/windows/win32/winmsg/window-styles), SetWindowSubclass
(https://learn.microsoft.com/en-us/windows/win32/api/commctrl/nf-commctrl-setwindowsubclass). winit:
`drag_window`/`drag_resize_window` (https://docs.rs/winit/0.30/winit/window/struct.Window.html).

---

## Pattern 4 — In-app settings/preferences panel rendered with glyphon (no egui)

### What the reference terminals actually do

- **Alacritty:** *no* in-app settings UI at all — TOML file + live reload only. (Deliberate minimalism.)
- **kitty:** config file (`kitty.conf`) + live reload (`load_config` / signal); some interactive UIs are
  separate "kittens" (mini TUIs), but core settings are file-driven. kitty does have in-terminal overlay
  UIs (the hints kitten, unicode_input) drawn as terminal content.
- **Ghostty:** config file; live reload; surfaces config *errors* in the window; no full GUI settings
  editor (philosophically file-first).
- **Windows Terminal:** the outlier — a full XAML settings GUI with validation. That's a different tech
  stack (WinUI) and a different product thesis.
- **Warp / iTerm2:** native GUI settings panels (Warp = Rust+native UI; iTerm2 = AppKit). Again, native
  toolkits, not a glyphon-drawn overlay.

**Conclusion for c0pl4nd (glyphon-direct, ship-LESS):** the pragmatic, in-taste pattern is a
**text-overlay panel reusing your existing command-palette rendering path** — i.e. an immediate-mode,
keyboard-driven list of settings drawn as glyphon text + Pattern-1 quads for selection highlight /
toggle pills. This is exactly the kitty/Ghostty "config is text, UI is an overlay" lineage, and it costs
nothing beyond what you already built for the palette. Do **not** add egui just for settings — it pulls a
second UI stack, a second text rasterizer, and breaks the single-renderer simplicity.

### Recommended design: immediate-mode overlay on the existing palette infra

State (tiny, no retained widget tree):

```rust
enum SettingKind {
    Bool(bool),                       // toggle pill [on]/[off]
    Enum { idx: usize, opts: Vec<&'static str> }, // < value >  (Left/Right cycles)
    Number { val: f64, step: f64, min: f64, max: f64 },
    Action,                           // runs a closure on Enter
}
struct SettingRow { label: &'static str, kind: SettingKind, help: &'static str }

struct SettingsOverlay {
    open: bool,
    rows: Vec<SettingRow>,
    selected: usize,   // which row is focused
    dirty: bool,       // need re-prepare/redraw
}
```

Input (immediate mode — no callbacks, just mutate state per key):

```rust
fn on_key(o: &mut SettingsOverlay, key: Key) {
    use Key::*;
    match key {
        Up    => { o.selected = o.selected.saturating_sub(1); o.dirty = true; }
        Down  => { o.selected = (o.selected + 1).min(o.rows.len()-1); o.dirty = true; }
        Space | Enter => { toggle_or_activate(&mut o.rows[o.selected]); o.dirty = true; }
        Left  => { adjust(&mut o.rows[o.selected], -1); o.dirty = true; }
        Right => { adjust(&mut o.rows[o.selected], +1); o.dirty = true; }
        Escape => { o.open = false; o.dirty = true; persist_settings(o); }
        _ => {}
    }
}
```

Render (only when `dirty`, honoring §11 render-on-change):

1. Build the glyphon `TextArea`s: one buffer for the panel, lines like
   `  Cursor blink        [ on ]` / `> Font size            14.0  <` (the `>` marks `selected`).
   Use cosmic-text `Attrs` color to dim help text and highlight the selected label.
2. Draw, in one render pass (Pattern 1 ordering):
   - a dim full-screen scrim quad (e.g. `rgba(0,0,0,0.5)`) to mute the terminal behind,
   - a panel background quad,
   - a selection-row highlight quad behind the focused row,
   - toggle "pill" quads (green/grey) for `Bool`,
   - then `text_renderer.render(...)` for all labels/values on top.
3. Reset `dirty = false`; only `request_redraw()` when `dirty` flips true (key press / config-file change).

### Surface config-validation errors here too (ties §4 + §13)

When live-reload parses the config, route parse errors into the same overlay (a red "Config errors" rows
section with line numbers) instead of a console print or silent fallback. This makes "config validation
surfaced in-app" (P1) nearly free once the overlay exists.

### Round-trip to the file (file is source of truth)

- The overlay edits an in-memory `Config`; on `Escape`/`Apply`, **serialize back to the TOML config file**
  (or a profile file) so the file stays canonical and external edits + live-reload still work. This keeps
  parity with the kitty/Ghostty "file is truth" model and avoids two divergent config stores.
- Watch the config file (`notify` crate or a reload keybind); on change, re-parse → either apply or show
  errors in the overlay.

**Why this is the right call:** it reuses one renderer (glyphon), one input model (your palette's), and
one config store (the file). It matches the taste of the products you're emulating (Ghostty/kitty/
Alacritty are all file-first; only the heavyweight WT ships a GUI). Adding egui would contradict the
project's glyphon-direct architecture and the "ship LESS" product direction.

Refs: glyphon README + hello-world (text-only renderer; you own the pass)
(https://github.com/grovesNL/glyphon); kitty config/reload
(https://sw.kovidgoyal.net/kitty/conf/); Ghostty config/reload
(https://ghostty.org/docs/config); Alacritty config (TOML, live reload)
(https://alacritty.org/config-alacritty.html); Windows Terminal settings UI as the GUI outlier
(https://learn.microsoft.com/en-us/windows/terminal/).

---

# Appendix A — Version facts (verified)

| Component | Fact | Source (verified) |
|---|---|---|
| glyphon | `0.11.0` (main); `wgpu 29.0.0`, `cosmic-text 0.18`, `etagere 0.3.0`; MSRV 1.92; dev-dep `winit 0.30.12` | glyphon `Cargo.toml` (raw.githubusercontent.com, verified 2026-05-30) |
| glyphon scope | Renders **text only**; rectangles explicitly out of scope; uses the wgpu "middleware pattern" (no extra render pass) | glyphon README (verified) |
| glyphon middleware setup | `Cache::new(&device)` → `Viewport::new(&device, &cache)` → `TextAtlas::new(&device,&queue,&cache,format)` → `TextRenderer::new(&mut atlas,&device,MultisampleState::default(),None)` | glyphon `examples/hello-world.rs` (verified) |
| glyphon per-frame API | `viewport.update(queue, Resolution{width,height})` → `text_renderer.prepare(device,queue,font_system,atlas,viewport,[TextArea{buffer,left,top,scale,bounds,default_color,custom_glyphs}],swash_cache)` → in a caller-owned pass: `text_renderer.render(atlas,viewport,&mut pass)` → after submit/present: `atlas.trim()` | glyphon `examples/hello-world.rs` (verified) |
| wgpu 29 surface acquire | `surface.get_current_texture()` returns a `wgpu::CurrentSurfaceTexture` **enum** (`Success/Timeout/Occluded/Outdated/Suboptimal/Lost/Validation`) — match it, do not `.unwrap()` an old `SurfaceTexture` | glyphon `examples/hello-world.rs` (wgpu 29 API, verified) |
| wgpu 29 color attachment | `RenderPassColorAttachment` now has a `depth_slice: None` field and `RenderPassDescriptor` has `multiview_mask: None` | glyphon `examples/hello-world.rs` (verified) |

> **c0pl4nd alignment:** the project pins **wgpu 29** per the codebase (memory). glyphon **0.11.0**
> targets exactly `wgpu 29.0.0` — so pin glyphon `0.11` (crates.io or the matching git rev). The quad
> pipeline in Pattern 1 must use the **same wgpu 29 surface format + `MultisampleState::default()`** as
> the glyphon `TextRenderer` (both shown above) or the pass will fail validation.

# Appendix B — Recommended P0/P1 build order for c0pl4nd (synthesized)

1. **P0 chrome correctness:** Win+Arrow snap on the frameless window (Pattern 3) + restore size/pos
   (Pattern 2) + per-monitor DPI crispness. These are the most-felt gaps for a custom-titlebar terminal.
2. **P0 multiplexing:** H/V splits + focus-nav + resize + close; cwd inheritance on split (P1).
3. **P0 input/mouse:** full keybind remap + per-OS defaults; ctrl/cmd-click URL + OSC 8; bracketed/large
   paste safety.
4. **P1 settings overlay** (Pattern 4) doubling as the config-error surface.
5. **P1 theme catalog + live reload + `.itermcolors` import** (cheap ecosystem win).
6. **P1 quake/dropdown mode** (signature differentiator) + opacity + padding.
7. **P1 perf:** render-on-change gating + present-mode toggle + large-output coalescing.

(Items explicitly de-prioritized per "ship LESS": background images, animated cursor trails, automatic
profile switching, base16 framework, detached daemon mode, full screen-reader UIA — all P3.)
