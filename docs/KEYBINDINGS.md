# Keybindings

C0PL4ND ships with the keybindings below. In the current shell these shortcuts
are **fixed** — they are not yet user-rebindable. The `[keybindings]` section of
`config.toml` (and its read-only mirror in the Settings window) is reserved for a
future rebinding dispatcher; editing it does not yet change the live shortcuts.
The second table lists built-in window/overlay controls.

The `mod` modifier below maps to the platform's primary modifier: **Ctrl** on
Windows and Linux, **Cmd** (⌘) on macOS. So `mod+shift+c` is `Ctrl+Shift+C` on
Windows/Linux and `Cmd+Shift+C` on macOS.

> Tip: open the **command palette** (`mod+shift+p`) at any time to discover the
> available actions and their shortcuts.

## Action shortcuts (fixed)

These shortcuts are implemented (hardcoded) in the shell's `frame_tick`. The
`Config key` column names the corresponding `[keybindings]` field — shown in the
Settings → Keybindings panel as a read-only reference until rebinding is wired.

| Action | Config key | Win / Linux | macOS |
|--------|------------|-------------|-------|
| Copy selection | `copy` | `Ctrl+Shift+C` | `Cmd+Shift+C` |
| Paste | `paste` | `Ctrl+Shift+V` | `Cmd+Shift+V` |
| New tab | `new_tab` | `Ctrl+Shift+T` | `Cmd+Shift+T` |
| Close tab | `close_tab` | `Ctrl+Shift+W` | `Cmd+Shift+W` |
| Split pane right | `split_right` | `Ctrl+Shift+D` | `Cmd+Shift+D` |
| Split pane down | `split_down` | `Ctrl+Shift+E` | `Cmd+Shift+E` |
| Open find / search | `search` | `Ctrl+Shift+F` | `Cmd+Shift+F` |
| Open command palette | `command_palette` | `Ctrl+Shift+P` | `Cmd+Shift+P` |
| Toggle history sidebar | `history_sidebar` | `Ctrl+Shift+H` | `Cmd+Shift+H` |
| Increase font size | `increase_font` | `Ctrl++` | `Cmd++` |
| Decrease font size | `decrease_font` | `Ctrl+-` | `Cmd+-` |

The default field values live in `crates/core/src/config/mod.rs`
(`Keybindings::default()`); the shell's actual chords are hardcoded to match them
in `crates/app/src/egui_app/mod.rs`.

## Built-in window & overlay controls

These controls are handled directly by the app and are **not** configurable via
`[keybindings]`. They are listed here for completeness.

| Key | Action | Available when |
|-----|--------|----------------|
| `F11` | Toggle borderless fullscreen | Always |
| `Esc` | Exit fullscreen | In fullscreen, and no overlay is open |
| `mod+shift+Home` | Scroll to the top of the scrollback (oldest line) | Always |
| `mod+shift+End` | Scroll back to following live output (bottom) | Always |
| `mod+shift+PageUp` | Jump to the previous shell-prompt mark (OSC 133) | Always |
| `mod+shift+PageDown` | Jump to the next shell-prompt mark (OSC 133) | Always |
| `mod+shift+Z` | Toggle zoom on the focused pane (full-size; siblings hidden) | Always |
| `mod+shift+←` / `→` / `↑` / `↓` | Move focus to the adjacent pane in that direction | Multiple panes |
| `Enter` / `F3` | Jump to next search match | Find overlay open |
| `Shift+F3` | Jump to previous search match | Find overlay open |
| `Esc` | Close the find overlay | Find overlay open |
| `↑` / `↓` | Move selection in the command palette | Palette open |
| `Enter` | Run the selected palette action | Palette open |
| `Esc` | Close the command palette | Palette open |
| `Enter` / `Esc` | Confirm / cancel a multi-line paste | Paste-confirm overlay open |

The fullscreen, scroll-to-edge, jump-to-prompt, find, palette, history, and
paste-confirm chords are implemented in `crates/app/src/egui_app/mod.rs`. They
are deliberately consumed before keystrokes reach the shell, so the chord never
leaks a stray control byte to the running program.

## Mouse gestures

| Gesture | Action |
|---------|--------|
| Drag | Select text (line-wise) |
| **Alt + drag** | Select a rectangular **block** (each row clipped to the same columns) |
| **Double-click** | Select the word under the cursor |
| **Triple-click** | Select the whole line |
| Hover a URL | Underline it + show the hand cursor (Ctrl/Cmd-click to open) |
| Ctrl/Cmd + click a URL | Open the link in the browser |
| **Right-click** | Open the pane context menu (Copy, Clear scrollback, Split, New tab, Close) |
| Wheel | Scroll this pane's scrollback (when no program grabbed the mouse) |

## Related selection behaviour

- **Copy-on-select** (`copy_on_select`, default `false`): when enabled, ending a
  mouse text selection copies it to the clipboard immediately (X11-style). When
  off, copy is the explicit `copy` binding above.
- **Multi-line paste warning** (`paste_warn_multiline`, default `true`): pasting
  clipboard text that contains a newline shows a confirm overlay first, so a
  pasted command cannot auto-run. See [TROUBLESHOOTING.md](../TROUBLESHOOTING.md)
  if pastes feel blocked.

## See also

- [CONFIG.md](../CONFIG.md) — full configuration reference, including how to
  override these bindings.
- [docs/discoverability.md](discoverability.md) — discoverability levers.
- [TROUBLESHOOTING.md](../TROUBLESHOOTING.md) — common issues and fixes.
