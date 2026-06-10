# C0PL4ND Configuration

C0PL4ND is designed to be **great out of the box with zero configuration**. You never *have* to edit a config file. When you want to customize the terminal, everything lives in a single, readable [TOML](https://toml.io) file — never a scripting language, and never something a typo can lock you out of (an invalid config is reported and the previous good settings are kept).

## Config file location

| Platform | Path |
| --- | --- |
| Linux | `~/.config/c0pl4nd/config.toml` |
| macOS | `~/.config/c0pl4nd/config.toml` |
| Windows | `%APPDATA%\c0pl4nd\config.toml` |

If the file doesn't exist, C0PL4ND runs entirely on its defaults. Create the file to begin customizing.

## Live reload

Changes are applied **live the moment you save** — no restart required. If your edit contains a syntax error or an invalid value, C0PL4ND reports the problem and continues running with your last valid configuration, so a bad edit never leaves you locked out of your terminal.

---

## Sections at a glance

| Section | What it controls |
| --- | --- |
| `[font]` | Font family and size |
| `[theme]` | Active color theme (default: `itasha-void`) |
| `[window]` | Background opacity |
| `[cursor]` | Cursor shape and blink |
| `[scrollback]` | Scrollback buffer size |
| `[tabs]` | Tab bar behavior |
| `[splits]` | Split/pane behavior |
| `[session]` | Close-confirmation and session safety |
| `[effects]` | The optional CRT/scanline effect (off by default) |
| `[keybindings]` | Keyboard shortcut overrides |
| `startup_panel` | The neofetch-style launch splash (on by default) |

---

## Font

```toml
[font]
family = "JetBrains Mono"   # any installed monospace font family
size = 13.0                  # points
```

If the requested family isn't installed, C0PL4ND falls back to a bundled monospace default.

## Theme

The **default theme is `itasha-void`** — the Retro-Future Anime OS aesthetic: **VOID BLACK** background, **SIGNAL TEAL** accents, and **NEON PINK** highlights.

```toml
[theme]
name = "itasha-void"   # the default; other built-in themes can be selected by name
```

## Window / opacity

```toml
[window]
opacity = 1.0   # 0.0 (fully transparent) .. 1.0 (fully opaque)
```

## Cursor

```toml
[cursor]
style = "block"   # "block" | "beam" | "underline"
blink = true
```

## Scrollback

```toml
[scrollback]
lines = 10000   # number of lines retained per pane
```

## Tabs

```toml
[tabs]
enabled = true
position = "top"          # "top" | "bottom"
show_when_single = false  # hide the tab bar when only one tab is open
```

## Splits

```toml
[splits]
# New splits inherit the working directory of the focused pane (via OSC 7).
inherit_cwd = true
```

## Session safety

C0PL4ND never closes a window full of running work without asking.

```toml
[session]
confirm_close_with_processes = true   # warn before closing a window with running child processes
```

## CRT / scanline effect

A retro CRT/scanline overlay is available for the full *into the wired* aesthetic. It is **off by default** for clarity and performance.

```toml
[effects]
crt = false        # master toggle for the CRT/scanline effect
scanline = 0.0     # 0.0 (none) .. 1.0 (strong) — scanline intensity, applies when crt = true
```

## Startup panel

On launch, C0PL4ND shows a neofetch-style splash: a brand ASCII logo beside your local system stats (OS, kernel, host, uptime, shell, terminal, CPU, memory, GPU). It reads only local facts and never touches the network. On by default.

```toml
startup_panel = true   # set false to launch straight to a clean prompt
```

## Keybindings

C0PL4ND ships with sensible default keybindings (open the **command palette** to discover available actions and their current shortcuts). For a scannable table of every default binding, see **[docs/KEYBINDINGS.md](docs/KEYBINDINGS.md)**. You can override any binding here. Bindings are simple `key = action` entries; defaults you don't override stay active.

```toml
[keybindings]
# Examples — adjust to taste:
"ctrl+shift+t"     = "tab.new"
"ctrl+shift+w"     = "tab.close"
"ctrl+shift+d"     = "split.right"
"ctrl+shift+e"     = "split.down"
"ctrl+shift+f"     = "search.open"
"ctrl+shift+p"     = "palette.open"
"ctrl+tab"         = "tab.next"
"ctrl+shift+tab"   = "tab.previous"
```

To remove a default binding, set it to `"none"`:

```toml
[keybindings]
"ctrl+shift+w" = "none"
```

---

## Full example `config.toml`

A complete configuration with every section populated at its default value. Copy this to your config path and edit freely.

```toml
# ~/.config/c0pl4nd/config.toml
# (Windows: %APPDATA%\c0pl4nd\config.toml)
#
# Every setting here is shown at its default value.
# Delete any line to fall back to the default — C0PL4ND works fine with an empty file.

startup_panel = true          # neofetch-style splash on launch

[font]
family = "JetBrains Mono"
size = 13.0

[theme]
name = "itasha-void"          # VOID BLACK + SIGNAL TEAL + NEON PINK

[window]
opacity = 1.0                 # 0.0 .. 1.0

[cursor]
style = "block"               # "block" | "beam" | "underline"
blink = true

[scrollback]
lines = 10000

[tabs]
enabled = true
position = "top"              # "top" | "bottom"
show_when_single = false

[splits]
inherit_cwd = true            # new panes open in the focused pane's directory

[session]
confirm_close_with_processes = true

[effects]
crt = false                   # CRT/scanline overlay — OFF by default
scanline = 0.0                # 0.0 .. 1.0, used when crt = true

[keybindings]
"ctrl+shift+t"   = "tab.new"
"ctrl+shift+w"   = "tab.close"
"ctrl+shift+d"   = "split.right"
"ctrl+shift+e"   = "split.down"
"ctrl+shift+f"   = "search.open"
"ctrl+shift+p"   = "palette.open"
"ctrl+tab"       = "tab.next"
"ctrl+shift+tab" = "tab.previous"
```

---

## Notes

- **Defaults are the product.** The single most-praised property of a great terminal is being usable instantly. Treat config as optional polish, not a requirement.
- **No programming language.** Configuration is declarative TOML by design. There is no scripting runtime to learn, and no way for a config file to make the terminal unusable.
- **Privacy.** Nothing in your config — and nothing about your shell session — is transmitted anywhere. See [SECURITY.md](SECURITY.md).
