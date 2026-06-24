# Keybindings

C0PL4ND ships with sensible default keybindings. Every binding in the first
table is **rebindable** — override any of them in the `[keybindings]` section of
your `config.toml` (see [CONFIG.md](../CONFIG.md)). The bindings in the second
table are built-in window/overlay controls.

The `mod` modifier in a binding string maps to the platform's primary modifier:
**Ctrl** on Windows and Linux, **Cmd** (⌘) on macOS. So `mod+shift+c` is
`Ctrl+Shift+C` on Windows/Linux and `Cmd+Shift+C` on macOS.

> Tip: open the **command palette** (`mod+shift+p`) at any time to discover the
> available actions and their current shortcuts.

## Rebindable actions (defaults)

These are the default bindings from the built-in keymap. Each is the value of a
field in the `[keybindings]` config table; set a field to a different combo to
rebind it, or to `"none"` to disable it.

| Action | Config key | Default (Win / Linux) | Default (macOS) |
|--------|------------|-----------------------|-----------------|
| Copy selection | `copy` | `Ctrl+Shift+C` | `Cmd+Shift+C` |
| Paste | `paste` | `Ctrl+Shift+V` | `Cmd+Shift+V` |
| New tab | `new_tab` | `Ctrl+Shift+T` | `Cmd+Shift+T` |
| Close tab | `close_tab` | `Ctrl+Shift+W` | `Cmd+Shift+W` |
| Next tab | `next_tab` | `Ctrl+Shift+]` | `Cmd+Shift+]` |
| Split pane right | `split_right` | `Ctrl+Shift+D` | `Cmd+Shift+D` |
| Split pane down | `split_down` | `Ctrl+Shift+E` | `Cmd+Shift+E` |
| Open find / search | `search` | `Ctrl+Shift+F` | `Cmd+Shift+F` |
| Open command palette | `command_palette` | `Ctrl+Shift+P` | `Cmd+Shift+P` |
| Toggle history sidebar | `history_sidebar` | `Ctrl+Shift+H` | `Cmd+Shift+H` |
| Increase font size | `increase_font` | `Ctrl++` | `Cmd++` |
| Decrease font size | `decrease_font` | `Ctrl+-` | `Cmd+-` |

These defaults are defined in `crates/core/src/config/mod.rs`
(`Keybindings::default()`). The config layer is the single source of truth — if
you change a default in your `config.toml`, the table above no longer reflects
your install; the command palette always shows the live values.

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
