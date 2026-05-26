# C0PL4ND Extensions

C0PL4ND supports **declarative extensions** — plugins that contribute themes
and keybinding overrides as plain data. This is what the large majority of
real-world terminal "extensions" are, and it is safe by construction: a
declarative plugin executes no code, cannot write to your shell, and cannot
touch the filesystem or network.

## Where plugins live

```
<config-dir>/c0pl4nd/plugins/<plugin-id>/extension.toml
```

- Linux/macOS: `~/.config/c0pl4nd/plugins/`
- Windows: `%APPDATA%\c0pl4nd\plugins\`

Each plugin is a folder containing an `extension.toml` manifest.

## Manifest: `extension.toml`

```toml
[extension]
id = "neon-pack"            # unique id
name = "Neon Theme Pack"
version = "0.1.0"
author = "you"
api_version = "0.1.0"        # host extension-API version targeted

# Capabilities are DEFAULT-DENY. The declarative layer never grants these;
# declaring them only records intent (and triggers an over-ask warning).
[capabilities]
pty_write = false
filesystem = false
network = false
process_spawn = false

[contributes]
# Theme files relative to this folder (must stay inside the folder).
themes = ["themes/neon.toml"]
# Optional keybinding-override file.
keybindings = "keymap.toml"
```

## Rules the loader enforces

| Rule | Behaviour |
|------|-----------|
| `api_version` major must match the host | Mismatched plugins are rejected |
| Contributed paths must stay inside the plugin folder | `..` / absolute paths are rejected |
| Dangerous capabilities are default-deny | Declaring them is flagged as an over-ask; never granted by the declarative layer |
| Malformed/missing manifest | That plugin is skipped (others still load) |

## Using a contributed theme

Once a plugin contributes `themes/neon.toml` (with `name = "neon"`), select it
in your config:

```toml
theme = "neon"
```

C0PL4ND resolves the theme by file stem across the bundled themes and every
installed plugin.

## Installing

- **Local**: drop a plugin folder into the plugins directory.
- **From git**: clone a plugin repo into the plugins directory.

The host extension-API version is `0.1.0`. The manifest's `api_version` and
`[capabilities]` block are the forward-compatible contract for future
extension surfaces.
