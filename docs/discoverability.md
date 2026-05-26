# Repository Discoverability

Levers to make C0PL4ND easy to find and evaluate on GitHub.

## Repository description (one line, SEO)

> C0PL4ND — a fast, GPU-accelerated, local-first terminal emulator for Windows, Linux, and macOS. Zero-config, built-in tabs & splits, no account, no telemetry.

## Topics / tags

Set these in the repo's "About" → Topics:

```
terminal terminal-emulator rust wgpu gpu cross-platform windows linux macos
tui cli zero-config local-first tabs splits retro-futurism lain ghost-in-the-shell
```

## Social preview image

Upload `assets/svg/social-preview.svg` (rasterize to 1280×640 PNG via
`packaging/gen-icons.sh` or any SVG→PNG tool) under
Settings → Social preview. This is the card shown when the repo is shared.

## README hero

`README.md` references `assets/svg/banner.svg` as the hero image and links the
per-OS install one-liners high above the fold.

## Release artifacts

Tag-driven releases (`v*`) attach per-OS binaries + installers + `SHA256SUMS`
so the "Releases" page and `install.sh` resolve the latest build automatically.

## Recommended GitHub settings

- Enable **Issues** and **Discussions**.
- Add the description + topics above.
- Pin a "Getting started" discussion linking the install section.
- Add `good first issue` / `help wanted` labels for contributors.
