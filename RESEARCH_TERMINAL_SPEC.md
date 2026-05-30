---
title: "Best-in-Class Terminal-Spec / Escape-Sequence / Text-Rendering Feature Checklist"
scope: "C0PL4ND terminal emulator — terminal-spec dimension"
comparison_set: [Ghostty, WezTerm, kitty, Alacritty, iTerm2, Windows Terminal, Konsole]
date: "2026-05-30"
author: "terminal-emulator domain researcher"
status: research
---

# Best-in-Class Terminal-Spec Feature Checklist

> Dimension under audit: **terminal-spec / escape-sequence / text-rendering**.
> Comparison set: Ghostty, WezTerm, kitty, Alacritty, iTerm2, Windows Terminal (WT), Konsole.
>
> Priority legend: **P0** = expected everywhere (a terminal is broken without it) · **P1** = strongly-wanted (every serious modern emulator has it) · **P2** = power-user (differentiator) · **P3** = niche.
>
> "vte difficulty" = effort inside a Rust parser layer (`vte`/`vtparse`-style state machine) plus the grid/renderer work it implies.

Legend for implements columns: ● = full · ◐ = partial/opt-in · ○ = no / behind flag · — = N/A.

---

## 1. Color

| # | Feature | What / Why | Ghostty | WezTerm | kitty | Alacritty | iTerm2 | WT | Konsole | Priority | vte difficulty |
|---|---------|------------|:--:|:--:|:--:|:--:|:--:|:--:|:--:|:--:|----|
| 1.1 | **Truecolor (24-bit) SGR 38;2 / 48;2** | `ESC[38;2;r;g;b m`. Full RGB foreground/background. Every modern TUI (delta, bat, nvim themes) expects it. | ● | ● | ● | ● | ● | ● | ● | **P0** | Low — parse SGR params, store RGB in cell attrs. |
| 1.2 | **256-color (SGR 38;5;n / 48;5;n)** | Indexed palette: 16 ANSI + 216 cube + 24 grays. Legacy-but-ubiquitous fallback for non-truecolor apps. | ● | ● | ● | ● | ● | ● | ● | **P0** | Low — index→RGB lookup table. |
| 1.3 | **16 ANSI + bright (SGR 30–37 / 90–97, 40–47 / 100–107)** | Base palette. Bold-as-bright legacy toggle. | ● | ● | ● | ● | ● | ● | ● | **P0** | Low. |
| 1.4 | **Configurable theme / palette** | User-supplied 16-color + fg/bg/cursor/selection. Theme-switching, light/dark. | ● | ● | ● | ● | ● | ● | ● | **P0** | Low — config plumbing. |
| 1.5 | **OSC 4 set/query palette color** | `OSC 4;n;rgb:RR/GG/BB ST` set; `OSC 4;n;? ST` query → terminal replies. Lets apps read/change the 256-color palette at runtime. | ● | ● | ● | ◐ | ● | ◐ | ● | **P1** | Med — query requires emitting a response back up the PTY. |
| 1.6 | **OSC 10 / 11 set/query default fg / bg** | `OSC 10;? ST` / `OSC 11;? ST`. Apps (vim, etc.) detect background lightness to pick a theme. **Background-color query is the single most-requested OSC by TUI apps.** | ● | ● | ● | ◐ | ● | ● | ● | **P1** | Med — store + reply. |
| 1.7 | **OSC 12 set/query cursor color** | Cursor color get/set. | ● | ● | ● | ◐ | ● | ◐ | ● | **P2** | Low. |
| 1.8 | **OSC 104 / 110 / 111 / 112 reset palette/fg/bg/cursor** | `OSC 104` resets one or all palette entries; 110/111/112 reset fg/bg/cursor to defaults. Cleanup after a themed app exits. | ● | ● | ● | ◐ | ● | ◐ | ● | **P2** | Low. |
| 1.9 | **OSC 5 / 105 special colors** (bold, underline, etc.) | xterm "special color" slots. Niche; few apps use it. | ◐ | ◐ | ◐ | ○ | ◐ | ○ | ◐ | **P3** | Low. |
| 1.10 | **`rgb:`/`rgba:`/`#rrggbb` color spec parsing** | XParseColor-style specs in OSC 4/10/11. Needed for any OSC color set. | ● | ● | ● | ◐ | ● | ● | ● | **P1** | Low — string parser. |
| 1.11 | **Color profile / sRGB-correct blending, dim (SGR 2)** | Faint text + correct alpha blend for selection/transparency. | ● | ● | ● | ● | ● | ● | ● | **P2** | Med — renderer blend math. |

**C0PL4ND note:** 1.1–1.4 are table stakes. 1.5–1.8 (the OSC color query/set family) are the highest-value gap because TUI apps actively probe OSC 11 for theme detection; the response path (writing back to the PTY) is the load-bearing piece.

---

## 2. Clipboard

| # | Feature | What / Why | Ghostty | WezTerm | kitty | Alacritty | iTerm2 | WT | Konsole | Priority | vte difficulty |
|---|---------|------------|:--:|:--:|:--:|:--:|:--:|:--:|:--:|:--:|----|
| 2.1 | **OSC 52 clipboard WRITE** | `OSC 52;c;<base64> ST` — app copies into the system clipboard (works over SSH/tmux). Essential for remote yank (nvim, tmux). | ● | ● | ● | ● | ● | ● | ● | **P1** | Med — base64 decode + clipboard API + size cap. |
| 2.2 | **OSC 52 clipboard READ — opt-in/off by default** | `OSC 52;c;? ST` lets a remote app *read* your clipboard. **Security posture: default-deny.** kitty/WezTerm/Ghostty gate it (`read`/`ask`/`deny`). Alacritty refuses read entirely. | ◐ | ◐ | ◐ | ○ | ◐ | ◐ | ◐ | **P1** (write) / **P2** (read) | Med — must wire a permission policy, not just decode. |
| 2.3 | **Selection vs clipboard targets (`c` / `p` / `s`)** | OSC 52 selection char: `c`=clipboard, `p`=primary (X11), `s`=select. | ● | ● | ● | ◐ | ● | ◐ | ◐ | **P2** | Low. |
| 2.4 | **Clipboard size limit / chunking** | Cap pasted/copied payload (DoS guard); some terminals chunk large OSC 52. | ● | ● | ● | ● | ● | ● | ◐ | **P2** | Low — length guard. |

**C0PL4ND note:** Ship OSC 52 **write** (P1). For **read**, follow the consensus default-off/ask posture — leaking the host clipboard to a remote process is the canonical OSC 52 vuln class.

---

## 3. Working Directory & Shell Integration

| # | Feature | What / Why | Ghostty | WezTerm | kitty | Alacritty | iTerm2 | WT | Konsole | Priority | vte difficulty |
|---|---------|------------|:--:|:--:|:--:|:--:|:--:|:--:|:--:|:--:|----|
| 3.1 | **OSC 7 report cwd** | `OSC 7;file://host/path ST`. New tab/split/window inherits the current directory. The #1 shell-integration QoL feature. | ● | ● | ● | ○ | ● | ● | ● | **P1** | Low — parse + store per-pane cwd; renderer-independent. |
| 3.2 | **OSC 133;A prompt start** | Mark where the prompt begins. | ● | ● | ● | ○ | ● | ● | ◐ | **P1** | Low — record row marks. |
| 3.3 | **OSC 133;B command start** | Mark end-of-prompt / start of typed command. | ● | ● | ● | ○ | ● | ● | ◐ | **P1** | Low. |
| 3.4 | **OSC 133;C output start** | Mark start of command output. | ● | ● | ● | ○ | ● | ● | ◐ | **P1** | Low. |
| 3.5 | **OSC 133;D;<exit> command finished + exit code** | Exit status capture; enables success/fail prompt glyphs. | ● | ● | ● | ○ | ● | ● | ◐ | **P1** | Low. |
| 3.6 | **Jump-to-prompt navigation** | Ctrl+↑/↓ between OSC 133;A marks. Power-user scrollback nav. | ● | ● | ● | ○ | ● | ◐ | ◐ | **P2** | Med — needs the marks + scroll integration. |
| 3.7 | **Command duration display** | Time between 133;C and 133;D shown inline / in status bar. | ● | ● | ◐ | ○ | ● | ◐ | ○ | **P2** | Low (given the marks). |
| 3.8 | **OSC 9;4 progress (taskbar/ConEmu)** | `OSC 9;4;st;pr ST` — st: 0 remove, 1 set value (pr 0–100), 2 error, 3 indeterminate, 4 paused. Long-running task progress (apt, builds) → taskbar/tab indicator. ConEmu origin; WT/Ghostty adopted. | ● | ● | ◐ | ○ | ◐ | ● | ○ | **P2** | Low — parse state/pct, surface to UI. |
| 3.9 | **Select last command output / re-run** | Click a prompt mark to select its whole output block. | ● | ● | ● | ○ | ● | ◐ | ○ | **P2** | Med. |
| 3.10 | **OSC 1337 SetUserVar / shell-injection helpers** | iTerm2 user vars + auto-injected shell integration scripts. | ○ | ◐ | ◐ | ○ | ● | ○ | ○ | **P3** | Med. |

**C0PL4ND note:** OSC 7 + the OSC 133 A/B/C/D quartet are the backbone of "modern" shell integration; they are pure grid-mark bookkeeping (no renderer work) and unlock jump-to-prompt, command-duration, and per-pane cwd. High value / low cost.

---

## 4. Hyperlinks

| # | Feature | What / Why | Ghostty | WezTerm | kitty | Alacritty | iTerm2 | WT | Konsole | Priority | vte difficulty |
|---|---------|------------|:--:|:--:|:--:|:--:|:--:|:--:|:--:|:--:|----|
| 4.1 | **OSC 8 explicit hyperlinks** | `OSC 8;id=x;URI ST text OSC 8;; ST`. App-supplied clickable links (ls --hyperlink, gcc diagnostics). | ● | ● | ● | ○ | ● | ● | ● | **P1** | Med — store URI per cell-run + `id` grouping for multi-line links. |
| 4.2 | **Heuristic URL auto-detection** | Regex-scan visible grid for `http(s)://`, `file://`, emails, paths. | ● | ● | ● | ● | ● | ● | ● | **P1** | Med — viewport regex + hit-testing. |
| 4.3 | **Ctrl/Cmd-click (or modifier) to open** | Avoid accidental opens; security gate on scheme. | ● | ● | ● | ● | ● | ● | ● | **P1** | Low. |
| 4.4 | **Hover underline + multi-cell `id` highlight** | Highlight the whole link (incl. across wraps) on hover. | ● | ● | ● | ◐ | ● | ● | ● | **P2** | Med. |
| 4.5 | **URI scheme allow-list / open confirmation** | Block `javascript:`, `file:` surprises; confirm non-http. | ● | ● | ● | ◐ | ● | ● | ● | **P2** | Low. |

---

## 5. Graphics (Inline Images)

| # | Feature | What / Why | Ghostty | WezTerm | kitty | Alacritty | iTerm2 | WT | Konsole | Priority | vte difficulty |
|---|---------|------------|:--:|:--:|:--:|:--:|:--:|:--:|:--:|:--:|----|
| 5.1 | **Sixel graphics** | DCS `q`-introduced bitmap protocol. Legacy but widely-targeted (img2sixel, gnuplot, mpv -vo sixel). | ● | ● | ● | ◐* | ● | ◐ | ● | **P2** | High — DCS sixel decoder + raster compositing into the grid. |
| 5.2 | **kitty graphics protocol** | `APC G ...` — modern, ID-tracked, z-index, animation, placement, shared-mem/file transfer, delete/query. Best-in-class image protocol. | ● | ● | ● | ○ | ○ | ○ | ○ | **P2** | High — APC parser, transmission modes (direct/file/temp/shmem), placement model, GPU upload. |
| 5.3 | **iTerm2 inline images (OSC 1337 File=...)** | `OSC 1337;File=...:<base64> ST`. imgcat. Simple base64 blob. | ◐ | ● | ○ | ○ | ● | ○ | ○ | **P3** | Med — OSC 1337 arg parse + decode + place. |
| 5.4 | **Graphics z-index / cursor interaction / scroll-with-content** | Images scroll, clip, and layer correctly relative to text + cursor. | ● | ● | ● | — | ● | — | ◐ | **P2** | High — compositor bookkeeping. |
| 5.5 | **Graphics protocol detection/query response** | Apps query support (kitty `a=q`, sixel via DA1 `4`). Avoids garbage on unsupported terms. | ● | ● | ● | ◐ | ● | ◐ | ● | **P2** | Med — DA1/DA2 + kitty query reply. |

*Alacritty sixel exists only on a long-standing fork/branch, not mainline.

**C0PL4ND note:** Already decodes sixel + kitty graphics — that puts it ahead of Alacritty/WT/Konsole-on-kitty and at parity with Ghostty/WezTerm/kitty on the graphics axis. Remaining polish: query/detection replies (5.5) and correct z-index/scroll (5.4).

---

## 6. Paste

| # | Feature | What / Why | Ghostty | WezTerm | kitty | Alacritty | iTerm2 | WT | Konsole | Priority | vte difficulty |
|---|---------|------------|:--:|:--:|:--:|:--:|:--:|:--:|:--:|:--:|----|
| 6.1 | **Bracketed paste (DEC mode 2004)** | Wrap pasted text in `ESC[200~ … ESC[201~` so apps don't execute it as keystrokes. **Critical security feature** (prevents paste-injection of `\n rm -rf`). | ● | ● | ● | ● | ● | ● | ● | **P0** | Low — mode flag + wrap on paste. |
| 6.2 | **Multiline / dangerous-paste confirm** | Warn before pasting text containing newlines or control chars when an app *isn't* in bracketed mode. | ● | ● | ● | ◐ | ● | ● | ◐ | **P1** | Low — scan clipboard for `\n`/controls. |
| 6.3 | **Strip control chars / sanitize paste** | Filter C0/C1 (WezTerm guidance: strip codes 0–8, 11, 12, 14–31) to defeat hidden escape injection. | ● | ● | ● | ◐ | ● | ● | ◐ | **P1** | Low. |
| 6.4 | **Filter bracketed-paste markers from pasted content** | If clipboard itself contains `ESC[201~`, neutralize it (CVE-class). | ● | ● | ● | ● | ● | ● | ◐ | **P1** | Low. |

---

## 7. Mouse

| # | Feature | What / Why | Ghostty | WezTerm | kitty | Alacritty | iTerm2 | WT | Konsole | Priority | vte difficulty |
|---|---------|------------|:--:|:--:|:--:|:--:|:--:|:--:|:--:|:--:|----|
| 7.1 | **SGR mouse reporting (mode 1006)** | `ESC[<b;x;y M/m`. Coordinate-unlimited mouse events. The modern default; required for nvim/tmux mouse. | ● | ● | ● | ● | ● | ● | ● | **P0** | Med — encode button/mods/coords on mouse events. |
| 7.2 | **X10 / normal / button-event / any-event modes (9/1000/1002/1003)** | Click-only, click+release, drag-tracking, all-motion. Apps pick granularity. | ● | ● | ● | ● | ● | ● | ● | **P0** | Med. |
| 7.3 | **UTF-8 (1005) & urxvt (1015) mouse encodings** | Legacy wide-coord encodings; some old apps still request. | ◐ | ● | ● | ◐ | ● | ◐ | ● | **P2** | Low. |
| 7.4 | **Focus reporting (mode 1004 → CSI I / CSI O)** | Terminal sends `ESC[I` on focus-in, `ESC[O` on focus-out. nvim autoread, tmux focus events. | ● | ● | ● | ● | ● | ● | ● | **P1** | Low — emit on window focus change. |
| 7.5 | **Pixel-coordinate mouse (SGR-pixels 1016)** | Sub-cell pixel mouse position. Niche (graphics apps). | ● | ● | ● | ○ | ◐ | ○ | ◐ | **P3** | Med. |
| 7.6 | **Mouse scroll → arrow keys in altscreen** | Wheel scroll translated to ↑/↓ for pagers (less, man) that don't request mouse. | ● | ● | ● | ● | ● | ● | ● | **P1** | Low. |
| 7.7 | **Shift-override to force local selection** | Hold Shift to bypass app mouse-capture and select text. | ● | ● | ● | ● | ● | ● | ● | **P1** | Low. |

---

## 8. Text Shaping & Rendering

| # | Feature | What / Why | Ghostty | WezTerm | kitty | Alacritty | iTerm2 | WT | Konsole | Priority | vte difficulty |
|---|---------|------------|:--:|:--:|:--:|:--:|:--:|:--:|:--:|:--:|----|
| 8.1 | **Grapheme clustering (UAX #29)** | Treat `e + combining accent`, ZWJ emoji sequences, flags as one cell-cluster. Without it, cursor/width math corrupts. | ● | ● | ● | ◐ | ● | ● | ● | **P0/P1** | High — Unicode segmentation + cell model that holds multi-codepoint clusters. |
| 8.2 | **CJK / wide-cell handling (East Asian Width, wcwidth)** | Wide glyphs occupy 2 cells; correct width or the whole grid misaligns. Unicode 9+ width tables. | ● | ● | ● | ◐ | ● | ● | ● | **P0** | Med — width table; the *table version* is a real correctness lever. |
| 8.3 | **Combining marks stacking** | Render diacritics over base glyph in one cell. | ● | ● | ● | ◐ | ● | ● | ● | **P1** | Med — glyph stacking in the cell. |
| 8.4 | **Emoji rendering + color (incl. ZWJ sequences, skin-tone modifiers)** | 👨‍👩‍👧, 👍🏽. Color emoji font + proper cluster width. | ● | ● | ● | ◐ | ● | ● | ● | **P1** | High — color-bitmap/COLR font support + width policy. |
| 8.5 | **Programming ligatures (calt/liga)** | `=>`, `!=`, `->` fused via HarfBuzz shaping. Fira Code / JetBrains Mono. Polarizing but strongly-wanted. | ● | ● | ● | ○ | ◐ | ○ | ◐ | **P2** | High — HarfBuzz shaping over runs + handling cursor/selection inside a ligature. |
| 8.6 | **Font fallback chain** | When the primary font lacks a glyph, walk a fallback list (CJK, emoji, symbols, Nerd Font). | ● | ● | ● | ◐ | ● | ● | ● | **P1** | High — per-glyph coverage lookup + fallback ordering + metrics normalization. |
| 8.7 | **Variation selectors (VS15/VS16 text vs emoji presentation)** | `︎`/`️` switch a char between text & emoji form; affects width + glyph. | ● | ● | ● | ○ | ● | ◐ | ● | **P2** | Med — honor VS in shaping + width. |
| 8.8 | **BiDi / RTL (Arabic, Hebrew)** | UAX #9 bidirectional reordering. Genuinely hard; most GPU terminals punt. kitty/Konsole strongest. | ○ | ◐ | ● | ○ | ◐ | ◐ | ● | **P3** | Very High — reordering + caret mapping; the hardest item on this list. |
| 8.9 | **Box-drawing / Powerline / block glyph crispness** | Pixel-perfect `│ ─ ┼ █  ` drawn by the terminal (not the font) so TUIs and powerline seams align. | ● | ● | ● | ● | ● | ● | ● | **P1** | Med — built-in vector box-drawing renderer. |
| 8.10 | **Subpixel / grayscale AA, gamma-correct text blend** | Crisp small text; correct light-on-dark contrast. | ● | ● | ● | ● | ● | ● | ● | **P2** | Med — renderer. |
| 8.11 | **Bold/italic/underline styles incl. curly/dotted/dashed underline (SGR 4:3)** | `SGR 4:3` curly underline + `SGR 58` underline color — used by LSP diagnostics in nvim. | ● | ● | ● | ◐ | ● | ◐ | ● | **P2** | Low/Med — extra SGR + renderer underline variants. |
| 8.12 | **Strikethrough (SGR 9), overline (SGR 53), double-underline (4:2)** | Less common SGR styles. | ● | ● | ● | ◐ | ● | ◐ | ● | **P3** | Low. |
| 8.13 | **kitty text-sizing protocol (multi-cell glyphs / scaled text)** | Newest kitty extension for 2×/fractional-size text. Cutting-edge, kitty-only. | ○ | ○ | ● | ○ | ○ | ○ | ○ | **P3** | High. |

**C0PL4ND note:** 8.1, 8.2, 8.6 (clustering, width tables, fallback) are the correctness foundation — get these wrong and *everything* misaligns. Ligatures (8.5) and BiDi (8.8) are the two genuinely hard, optional differentiators; BiDi is where even Ghostty/Alacritty punt.

---

## 9. Input Encoding (Keyboard)

| # | Feature | What / Why | Ghostty | WezTerm | kitty | Alacritty | iTerm2 | WT | Konsole | Priority | vte difficulty |
|---|---------|------------|:--:|:--:|:--:|:--:|:--:|:--:|:--:|:--:|----|
| 9.1 | **Legacy key encoding (cursor/function keys, DECCKM)** | Correct `ESC[A`, `ESC OA`, F-keys, keypad app mode. Baseline; everything depends on it. | ● | ● | ● | ● | ● | ● | ● | **P0** | Med — keymap tables + mode-dependent encoding. |
| 9.2 | **Alt-as-Meta (ESC-prefix) toggle** | Alt+key sends `ESC<key>`; configurable vs composed chars. | ● | ● | ● | ● | ● | ● | ● | **P0** | Low. |
| 9.3 | **xterm modifyOtherKeys (CSI 27;mod;code ~)** | Levels 1/2: disambiguate Ctrl/Alt+key beyond legacy. Pre-kitty standard; many apps still detect it. | ● | ● | ● | ◐ | ● | ● | ● | **P1** | Med. |
| 9.4 | **kitty keyboard protocol (CSI u, progressive enhancement)** | 5 flag levels: disambiguate, report event types (press/repeat/**release**), alternate keys, all-as-escape, associated text. Stack push/pop (`CSI > flags u` / `CSI < n u`), query `CSI ? u`. The modern best-in-class input protocol — fixes Ctrl+I≠Tab, Shift+Enter, key-release for games/TUIs. | ● | ● | ● | ● | ◐ | ◐ | ◐ | **P1/P2** | High — full CSI u encoder, flag stack, functional-key PUA codepoints, event-type + associated-text reporting. |
| 9.5 | **Key disambiguation (Ctrl+I vs Tab, Ctrl+M vs Enter, Esc)** | The concrete payoff of 9.4 flag 0b1. | ● | ● | ● | ● | ◐ | ◐ | ◐ | **P2** | (subset of 9.4) |
| 9.6 | **Key-release & repeat events** | kitty flag 0b10 — required for terminal games / Vim-like real-time input. | ● | ● | ● | ● | ○ | ○ | ○ | **P2** | (subset of 9.4) |
| 9.7 | **IME / dead-key / compose support** | CJK input methods, compose-key (´ + e → é). OS-integration heavy. | ● | ● | ● | ◐ | ● | ● | ● | **P1** | High — platform IME plumbing, not vte. |
| 9.8 | **Configurable keybindings / leader chords** | User-remappable shortcuts, multi-key chords. | ● | ● | ● | ● | ● | ● | ● | **P1** | Low/Med — app layer. |
| 9.9 | **Bracketed-paste-aware key handling + kitty `CSI = 0 ; mode u` reset on alt-screen** | Reset enhancement flags correctly across altscreen/app exit so a crashed app doesn't leave the keyboard in CSI-u mode. | ● | ● | ● | ◐ | ◐ | ◐ | ◐ | **P2** | Med — lifecycle correctness. |

**C0PL4ND note:** Implement modifyOtherKeys (9.3) *and* the kitty keyboard protocol (9.4) — they're the two things that make modern nvim/helix/tmux keybindings work. The kitty protocol's flag-stack lifecycle (9.9) is the subtle correctness trap.

---

## 10. Scrollback

| # | Feature | What / Why | Ghostty | WezTerm | kitty | Alacritty | iTerm2 | WT | Konsole | Priority | vte difficulty |
|---|---------|------------|:--:|:--:|:--:|:--:|:--:|:--:|:--:|:--:|----|
| 10.1 | **Configurable scrollback buffer size** | N lines retained; user-set. | ● | ● | ● | ● | ● | ● | ● | **P0** | Low — ring buffer. |
| 10.2 | **Scrollback search (regex / incremental)** | Find text in history, highlight matches, jump. | ● | ● | ● | ◐* | ● | ● | ● | **P1** | Med — search over the line store + match highlight. |
| 10.3 | **Reflow / rewrap on resize** | Re-wrap long lines when columns change (no truncation/orphan wraps). Hard to get right. | ● | ● | ● | ◐ | ● | ● | ● | **P1** | High — wrap-flag tracking + history rewrap. |
| 10.4 | **Scroll-to-bottom-on-output toggle** | Jump to live output on new data (or stay put while scrolled up). | ● | ● | ● | ● | ● | ● | ● | **P1** | Low. |
| 10.5 | **Infinite / disk-backed scrollback** | Unbounded history, optionally paged to disk. | ◐ | ● | ● | ○ | ◐ | ◐ | ◐ | **P2** | Med/High. |
| 10.6 | **Alt-screen scrollback opt-in (mouse-wheel passthrough)** | Optionally scroll the *terminal* even inside fullscreen apps. | ◐ | ● | ● | ◐ | ● | ◐ | ◐ | **P2** | Low. |
| 10.7 | **Clear scrollback (OSC/`ESC[3J`) + "clear & reset"** | `ESC[3J` erases saved lines; Cmd+K / clear command. | ● | ● | ● | ● | ● | ● | ● | **P1** | Low. |

*Alacritty offers vi-mode search rather than a search UI.

---

## 11. Selection

| # | Feature | What / Why | Ghostty | WezTerm | kitty | Alacritty | iTerm2 | WT | Konsole | Priority | vte difficulty |
|---|---------|------------|:--:|:--:|:--:|:--:|:--:|:--:|:--:|:--:|----|
| 11.1 | **Char / word / line selection (single/double/triple-click)** | The baseline selection model. | ● | ● | ● | ● | ● | ● | ● | **P0** | Med — hit-test + grapheme-aware boundaries. |
| 11.2 | **Block / rectangular selection** | Alt-drag column selection. Code/table extraction. | ● | ● | ● | ● | ● | ● | ● | **P1** | Med. |
| 11.3 | **Smart selection rules (URLs, paths, semantic units)** | Double-click grabs a whole URL/path/word per configurable regex. iTerm2's signature. | ◐ | ● | ◐ | ◐ | ● | ◐ | ◐ | **P2** | Med — regex boundary rules. |
| 11.4 | **Copy-on-select** | Selecting auto-copies (X11 primary or clipboard). | ● | ● | ● | ● | ● | ● | ● | **P1** | Low. |
| 11.5 | **Trailing-whitespace trim on copy** | Strip cell-fill spaces at line ends when copying. | ● | ● | ● | ● | ● | ● | ● | **P1** | Low. |
| 11.6 | **Wrapped-line "unwrap" on copy** | Copy a soft-wrapped logical line as one line (no injected `\n`). | ● | ● | ● | ◐ | ● | ● | ● | **P2** | Med — needs the wrap flags from 10.3. |
| 11.7 | **Selection extend (shift-click) + word-grow drag** | Extend existing selection; drag past initial word grows by word. | ● | ● | ● | ● | ● | ● | ● | **P2** | Low. |
| 11.8 | **Primary (X11) selection / middle-click paste** | Unix selection-clipboard convention. | ● | ● | ● | ● | — | — | ● | **P2** (Linux) | Low. |

---

## 12. Misc VT / Mode State

| # | Feature | What / Why | Ghostty | WezTerm | kitty | Alacritty | iTerm2 | WT | Konsole | Priority | vte difficulty |
|---|---------|------------|:--:|:--:|:--:|:--:|:--:|:--:|:--:|:--:|----|
| 12.1 | **Alternate screen buffer (modes 1049/47/1047)** | Fullscreen apps (vim, less) use a separate buffer; restore on exit. | ● | ● | ● | ● | ● | ● | ● | **P0** | Med — second grid + save/restore cursor. |
| 12.2 | **Cursor styles (DECSCUSR `CSI Ps SP q`)** | 0/1 blink block, 2 steady block, 3 blink underline, 4 steady underline, 5 blink bar, 6 steady bar. nvim mode-cursor. | ● | ● | ● | ● | ● | ● | ● | **P1** | Low — parse + renderer cursor shape/blink. |
| 12.3 | **Synchronized output (DEC mode 2026 / DCS BSU/ESU)** | `CSI ?2026h/l` brackets a frame so the terminal renders it atomically → no tearing/flicker on full-screen redraws. Modern must-have; tmux/nvim emit it. | ● | ● | ● | ◐ | ◐ | ● | ◐ | **P1** | Med — buffer writes between begin/end, flush once; timeout safety. |
| 12.4 | **Title set + title stack (OSC 0/1/2, XTWINOPS 22/23)** | `OSC 0/2;title ST` set window/icon title; `CSI 22;t` push title, `CSI 23;t` pop. Restore title after an app changes it. | ● | ● | ● | ● | ● | ● | ● | **P1** | Low — title + stack. |
| 12.5 | **XTWINOPS window manipulation (resize/report 8/14/16/18/19)** | Report cell/pixel size, text-area size; some resize ops. Apps query geometry (e.g., for sixel sizing). | ◐ | ● | ● | ◐ | ◐ | ◐ | ● | **P2** | Med — geometry replies (resize ops often deliberately disabled for security). |
| 12.6 | **Tab stops (HTS `ESC H`, TBC `CSI g`, `CSI Ps I/Z`)** | Set/clear/navigate horizontal tabs; correct `\t` rendering. | ● | ● | ● | ● | ● | ● | ● | **P1** | Low — tab-stop bitset. |
| 12.7 | **REP (`CSI Ps b`) repeat last char** | Compression: repeat preceding grapheme N times. Used by some TUIs/recordings. | ● | ● | ● | ● | ● | ● | ● | **P2** | Low. |
| 12.8 | **DEC special graphics / line-drawing charset (`ESC ( 0`)** | Designate G0 to box-drawing charset; legacy TUIs (dialog, ncurses) rely on it. | ● | ● | ● | ● | ● | ● | ● | **P1** | Low — charset translation table. |
| 12.9 | **Origin/autowrap/insert modes (DECOM, DECAWM 7, IRM 4)** | Core DEC private/ANSI modes apps toggle. | ● | ● | ● | ● | ● | ● | ● | **P0** | Low/Med. |
| 12.10 | **Scroll regions (DECSTBM `CSI t;b r`), left/right margins (DECLRMM 69 + DECSLRM)** | Top/bottom (and l/r) scroll margins — vim split scroll, status lines. L/R margins are rarer. | ● | ● | ● | ● | ● | ● | ● | **P1** (TB) / **P3** (LR) | Med — region-aware scrolling. |
| 12.11 | **Erase/insert/delete line & char (EL/ED/IL/DL/ICH/DCH/ECH)** | `CSI K/J/L/M/@/P/X`. Editing primitives; every TUI uses them. | ● | ● | ● | ● | ● | ● | ● | **P0** | Med. |
| 12.12 | **Save/restore cursor (DECSC/DECRC, SCOSC/SCORC)** | `ESC 7`/`ESC 8`. Cursor + attrs save/restore. | ● | ● | ● | ● | ● | ● | ● | **P0** | Low. |
| 12.13 | **Device attributes / status (DA1/DA2/DA3, DSR, DECRQM)** | `CSI c` / `CSI > c` / `CSI 5n` / `CSI 6n` (cursor pos) / `CSI ?Ps$p` mode-report. Apps capability-probe via these — **must reply correctly or apps hang/garble.** | ● | ● | ● | ● | ● | ● | ● | **P0** | Med — accurate reply strings are load-bearing. |
| 12.14 | **XTGETTCAP / terminfo `Smulx`, true `TERM`/`COLORTERM`** | Respond to `DCS + q ... ST` capability queries; set `COLORTERM=truecolor`. Apps gate truecolor/styled-underline on this. | ◐ | ● | ● | ○ | ◐ | ◐ | ◐ | **P2** | Med — XTGETTCAP responder. |
| 12.15 | **Bell (BEL) — audible / visual / urgency hint** | `\a` → visual flash, sound, or window-urgency; configurable. | ● | ● | ● | ● | ● | ● | ● | **P1** | Low. |
| 12.16 | **C1 control handling (8-bit + ESC-7bit forms), UTF-8 robustness** | Correctly parse `ESC [` vs raw `0x9B`, invalid-UTF-8 → U+FFFD, never desync the parser. | ● | ● | ● | ● | ● | ● | ● | **P0** | Med — robust state machine (this is the core of a `vte`-class parser). |
| 12.17 | **Reset (RIS `ESC c`, DECSTR `CSI ! p`) — soft & hard reset** | `reset`/`tput reset` recovery after a garbled app. | ● | ● | ● | ● | ● | ● | ● | **P1** | Low/Med. |
| 12.18 | **Reverse video / DECSCNM (mode 5), bracket of overall states** | Whole-screen invert; misc DEC private modes. | ● | ● | ● | ● | ● | ● | ● | **P2** | Low. |
| 12.19 | **In-band terminal resize notification (DEC mode 2048)** | Newer push notification of size changes to apps (vs SIGWINCH only). Emerging. | ● | ◐ | ● | ○ | ○ | ○ | ○ | **P3** | Low. |

**C0PL4ND note:** The non-negotiable P0 cluster here is **DA/DSR replies (12.13)**, **robust C1/UTF-8 parsing (12.16)**, **alt-screen (12.1)**, **the editing primitives (12.11)** and **cursor save/restore (12.12)** — these are what "VT compliance" actually means. **Synchronized output (12.3)** and **DECSCUSR cursor styles (12.2)** are the two modern-era items most users *notice* (no flicker, correct nvim cursor).

---

## Top-15 Gaps a Typical Modern Terminal Must Have (priority-ordered)

These are the items most likely to be missing/half-done in a young Rust terminal and most likely to make a TUI app misbehave:

1. **Correct DA1/DA2/DSR/DECRQM replies (12.13, P0)** — apps capability-probe; wrong/absent replies → hangs and garbage.
2. **Robust C1 + UTF-8 + invalid-byte recovery in the parser (12.16, P0)** — the core of not desyncing.
3. **Bracketed paste (mode 2004) + paste sanitization (6.1/6.3, P0)** — security-critical (paste-injection).
4. **Grapheme clustering + East-Asian width tables (8.1/8.2, P0/P1)** — wrong width corrupts the whole grid.
5. **SGR mouse 1006 + the mouse-mode family + focus reporting 1004 (7.1/7.2/7.4, P0/P1)** — nvim/tmux mouse + focus events.
6. **Synchronized output (DEC 2026) (12.3, P1)** — flicker-free full-screen redraws; tmux/nvim emit it.
7. **kitty keyboard protocol + modifyOtherKeys (9.3/9.4, P1/P2)** — modern keybindings (Ctrl+I≠Tab, Shift+Enter, key release).
8. **OSC 7 cwd + OSC 133 A/B/C/D shell-integration marks (3.1–3.5, P1)** — inherit-cwd, jump-to-prompt, command duration; cheap grid bookkeeping.
9. **OSC 11/10/4 color query+set (1.5/1.6, P1)** — TUI background-detection theming; the response path is the work.
10. **OSC 52 clipboard write, with read default-off (2.1/2.2, P1)** — remote yank; security posture matters.
11. **OSC 8 hyperlinks + heuristic URL detection + Ctrl-click (4.1–4.3, P1)** — clickable `ls`/compiler links.
12. **Font fallback chain + emoji/color + combining marks (8.6/8.4/8.3, P1)** — no tofu/boxes for CJK/emoji/symbols.
13. **Scrollback search + reflow-on-resize (10.2/10.3, P1)** — reflow is the hard one most young terminals skip.
14. **DECSCUSR cursor styles + title stack + DEC line-drawing charset (12.2/12.4/12.8, P1)** — nvim mode-cursor, title restore, ncurses box-drawing.
15. **Block/rectangular selection + copy-on-select + trailing-whitespace trim + unwrap-on-copy (11.2/11.4/11.5/11.6, P1)** — the selection behaviors users expect by muscle memory.

**Two genuine hard differentiators (optional):** programming **ligatures via HarfBuzz** (8.5, P2) and **BiDi/RTL** (8.8, P3) — BiDi is where even Ghostty and Alacritty punt, so it's pure upside but expensive. **Inline graphics** (sixel + kitty, §5) is already C0PL4ND's strength — keep the detection/query replies (5.5) and z-index/scroll correctness (5.4) tight to fully bank that lead.

---

## Sources

Primary specs and authoritative docs consulted:

- kitty — Terminal graphics protocol: https://sw.kovidgoyal.net/kitty/graphics-protocol/
- kitty — Comprehensive keyboard handling (CSI u / progressive enhancement): https://sw.kovidgoyal.net/kitty/keyboard-protocol/
- kitty — Shell integration (OSC 7 / OSC 133): https://sw.kovidgoyal.net/kitty/shell-integration/
- kitty — Text-sizing protocol: https://sw.kovidgoyal.net/kitty/text-sizing-protocol/
- kitty — main docs / feature overview: https://sw.kovidgoyal.net/kitty/
- freedesktop terminal-wg — Semantic prompts (OSC 133) proposal: https://gitlab.freedesktop.org/Per_Bothner/specifications/-/blob/master/proposals/semantic-prompts.md
- freedesktop terminal-wg — de-facto terminal standards index: https://gitlab.freedesktop.org/terminal-wg/specifications
- Per Bothner — FinalTerm shell-integration semantic sequences (blog): https://per.bothner.com/blog/2019/shell-integration/
- Matt Hawkins — OSC 7 and shell integration: https://matth-hawkins.dev/posts/osc7-shell-integration/
- egmontkob — Hyperlinks in terminal emulators (OSC 8 spec gist): https://gist.github.com/egmontkob/eb114294efbcd5adb1944c9f3cb5feda
- Alhadis — OSC8 adoption tracker: https://github.com/Alhadis/OSC8-Adoption
- yudai — OSC 52 clipboard gist: https://gist.github.com/yudai/95b20e3da66df1b066531997f982b57b
- chromium SSH client — Clipboard / OSC 52 docs: https://chromedevtools.github.io/ssh/clipboard
- theimpostor/osc — OSC 52 clipboard tool: https://github.com/theimpostor/osc
- xterm — Control Sequences (ctlseqs: OSC color codes, DECSCUSR, OSC 52, XTWINOPS, DA/DSR): https://invisible-island.net/xterm/ctlseqs/ctlseqs.html
- iTerm2 — Synchronized updates spec (DEC 2026): https://gitlab.com/gnachman/iterm2/-/wikis/synchronized-updates-spec
- iTerm2 — Shell integration wiki: https://gitlab.com/gnachman/iterm2/-/wikis/shell-integration
- contour-terminal — Synchronized output VT extension: https://github.com/contour-terminal/contour/blob/master/docs/vt-extensions/synchronized-output.md
- Ghostty — homepage / feature overview: https://ghostty.org/
- Ghostty — VT sequences reference: https://ghostty.org/docs/vt
- Ghostty — README: https://github.com/ghostty-org/ghostty/blob/main/README.md
- WezTerm — Escape sequences reference: https://wezterm.org/escape-sequences.html
- WezTerm — repository / docs: https://github.com/wez/wezterm
- leonerd — fixterms / the CSI u spec: http://www.leonerd.org.uk/hacks/fixterms/
- tasshi-me — awesome-terminal-graphics (sixel/kitty/iTerm2 image protocol survey): https://github.com/tasshi-me/awesome-terminal-graphics
- ratatui-image — multi-backend graphics protocol notes: https://github.com/benjajaja/ratatui-image
- Microsoft Learn — Windows Terminal OSC 9;4 progress-bar sequences: https://learn.microsoft.com/en-us/windows/terminal/tutorials/progress-bar-sequences
- rockorager.dev — OSC 9;4 progress bars reference: https://rockorager.dev/misc/osc-9-4-progress-bars/
- vt100.net — DECSCUSR (Set Cursor Style, VT510 manual): https://vt100.net/docs/vt510-rm/DECSCUSR.html
- christianparpart gist — Terminal Spec: Synchronized Output: https://gist.github.com/christianparpart/d8a62cc1ab659194337d73e399004036
- christianparpart (DEV) — A look into a terminal emulator's text stack: https://dev.to/christianparpart/look-into-a-terminal-emulator-s-text-stack-3poe
- Mitchell Hashimoto — Grapheme clusters in terminals: https://mitchellh.com/writing/grapheme-clusters-in-terminals
- Mitchell Hashimoto — Ghostty Devlog 004 (styled underlines / XTGETTCAP): https://mitchellh.com/writing/ghostty-devlog-004
- iTerm2 — Inline images documentation (OSC 1337): https://iterm2.com/documentation-images.html
- iTerm2 — Feature reporting spec (DA1 ext, focus, etc.): https://iterm2.com/feature-reporting/
- WezTerm — Shell integration (OSC 7 / OSC 133): https://wezterm.org/shell-integration.html
- WezTerm — Scrollback / reflow docs: https://wezterm.org/scrollback.html
- WezTerm — Key encoding (kitty keyboard): https://wezterm.org/config/key-encoding.html
- Akmatori — Terminal graphics protocols (Kitty/Sixel/iTerm2): https://akmatori.com/blog/terminal-graphics-protocols
- Santhosh Thottingal — Complex scripts in terminal & OSC 66: https://thottingal.in/blog/2026/03/22/complex-scripts-in-terminal/
- Terminal Trove — Terminal emulators comparison table (2026): https://terminaltrove.com/compare/terminals/
