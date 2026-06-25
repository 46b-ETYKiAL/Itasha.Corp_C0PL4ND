# Changelog

All notable changes to C0PL4ND are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project
adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

Full per-release artifacts (signed binaries, SBOMs, provenance) are on the
[GitHub Releases](https://github.com/46b-ETYKiAL/Itasha.Corp_C0PL4ND/releases)
page.

## [0.4.9] - 2026-06-25

### Fixed

- **In-app auto-update was rejecting the download** with *"signature
  trusted-comment file mismatch: signed for `release/c0pl4nd-…`, expected
  `c0pl4nd-…`"*. The release workflow signed each artifact by its `release/<name>`
  path, which minisign records verbatim in the signature's trusted comment
  (`file:release/<name>`); the fail-closed updater binds that token to the **bare**
  asset name and so refused the otherwise-valid, correctly-checksummed artifact —
  breaking auto-update for every deployed client. The release now signs **bare
  filenames** (trusted comment `file:<name>`), so every existing client can verify
  and install the update; a CI guard fails the release if any signature carries a
  path prefix. The updater's trusted-comment binding was additionally hardened to
  compare basenames on both sides (defence-in-depth for future builds). If you are
  on an older version, update to v0.4.9 — it installs cleanly.

## [0.4.8] - 2026-06-25

A best-in-class interaction wave bringing the egui shell to parity with
mainstream terminals — selection, navigation, and pane management — followed by
a production-scale QA pass (full feature inventory, risk-based edge cases, and
the complete issue-finding tool sweep).

### Added

- **Scrollback navigation chords.** `mod+shift+Home` / `mod+shift+End` jump to
  the top / bottom of the scrollback; `mod+shift+PageUp` / `mod+shift+PageDown`
  jump to the previous / next shell-prompt mark (OSC 133). All are consumed
  before reaching the shell, so the chord never leaks a control byte to the PTY.
- **Word and line selection.** Double-click selects the word under the cursor
  (its run includes path / URL / identifier punctuation); triple-click selects
  the whole line.
- **Block (rectangular) selection.** Hold `Alt` while dragging to clip every row
  to the same column range instead of the line-wise span.
- **Right-click pane context menu.** Copy (when a selection exists), Clear
  scrollback, Split right, Split down, New tab, and Close pane. (Paste is offered
  via the keyboard shortcut.)
- **Zoom-pane toggle (`mod+shift+Z`).** Render only the focused pane full-size,
  siblings hidden; toggle again to restore the exact prior layout.
- **Directional pane focus (`mod+shift+Arrow`).** Move keyboard focus to the
  geometrically adjacent pane.
- **Hover-URL affordance.** A detected URL underlines and shows the hand cursor
  on plain hover (no modifier), signalling it is `mod`-clickable to open.

### Fixed

- **Jump-to-prompt-mark chords now fire on Windows and Linux.** The
  `mod+shift+PageUp/PageDown` chords were silently dead off-macOS because the
  matcher required the platform `command` modifier; they now use the same
  explicit ctrl-or-command discipline as the other chords.
- **Zoom no longer shows a stale pane after focus moves.** Switching tabs (or
  opening a new tab) while zoomed kept the old pane on screen while keystrokes
  routed to the now-focused hidden pane; a per-frame reconcile drops the zoom
  when focus diverges, so the focused pane is always the one shown.

### Documentation

- `KEYBINDINGS.md` corrected to reflect that the shell's shortcuts are currently
  fixed (not yet user-rebindable) — matching the Settings panel — and a mouse-
  gestures reference table was added.

## [0.4.7] - 2026-06-17

### Changed

- **New app icon: a monochrome phosphor-teal daemon-sigil.** The app icon is
  redrawn as a single-hue (`#1ad6c0`) occult summoning seal — a double
  containment ring with radial seal-ticks, a faint kamea grid, inward goetic
  bind-rune spikes, and cardinal pommel dots — wrapped around the bold `>_`
  shell prompt mark, with a `fork()` trident as the daemon signature at the
  base. This replaces the previous multi-colour cyan/violet/red icon and
  realigns the mark to C0PL4ND's phosphor-teal brand voice. The new art ships
  in the embedded Windows `.ico`, the runtime window icon, and the full icon
  source family (`assets/svg/app-icon*.svg`, `logomark.svg`). Legibility is
  preserved down to 16px via a size-tiered render (the clean prompt mark at
  small sizes, the full seal at large sizes).

## [0.4.6] - 2026-06-15

### Changed

- **Update checks now default to `notify` (previously `manual`).** On launch,
  C0PL4ND performs a single **read-only** check against the public GitHub
  Releases API and shows a passive toast if a newer version exists. The check is
  throttled to at most once per `check_interval_hours` (default 24h) via a
  persisted `last-update-check` timestamp, and it **sends zero identifiers**.
  This is a privacy-relevant default change: a fresh install now makes one
  on-launch network connection. To keep update checks but make **no on-launch
  network connection**, set `mode = "manual"`; to disable all network access,
  set `mode = "off"`. See [PRIVACY.md](PRIVACY.md).

### Fixed

- **Wide (CJK / emoji) glyphs render cell-accurately again.** Rendering is now
  per-cell — each glyph is painted at its computed grid column rather than from
  accumulated font advances — so a wide or fallback glyph can never shift
  neighbouring cells. This properly fixes the launch-time "scattered glyphs"
  regression that was reverted in 0.4.5.
- Copying a selection across a wide glyph no longer inserts a stray space.
- Rows mixing right-to-left script with wide CJK/emoji glyphs keep correct
  cell-column alignment through BiDi reordering.

### Security

- The on-launch and `c0pl4nd update` CLI version checks now use a host-confined
  HTTPS GET — no arbitrary-host redirects (manual redirect walk re-asserting the
  `api.github.com` allow-list at every hop), a response size cap, and
  connect/read timeouts — matching the in-app updater's hardened network path.
- `check_interval_hours` is now enforced (a persisted last-check timestamp), so
  the default `notify` check cannot exceed GitHub's unauthenticated rate limit.

### Tests

- Added property-based tests (proptest), a VT/ANSI conformance corpus, an
  explicit accessibility (AccessKit) suite, additional fuzz targets
  (config-TOML / `.itermcolors` / OSC), and grid-reflow + search benchmarks.
  Filled previously-untested modules (palette, update-engine constants, DLL
  hardening, screenshot math). Wide-glyph cell positioning is now unit-tested
  without a display via a pure positioning function.

## [0.4.5] - 2026-06-14

### Fixed

- Reverted the 0.4.4 wide-glyph rendering change, which scattered glyphs on
  launch when a proportional fallback font loaded before the monospace font.
  (0.4.6 reintroduces the fix correctly via per-cell rendering.)

## [0.4.4] - 2026-06-14

### Changed

- First attempt at cell-accurate wide-glyph rendering (superseded; see 0.4.5 /
  0.4.6).

## [0.4.3] - 2026-01-17

Earlier releases (0.1.0 – 0.4.3) predate this changelog; see the GitHub Releases
page for their notes and signed artifacts.

[0.4.6]: https://github.com/46b-ETYKiAL/Itasha.Corp_C0PL4ND/releases/tag/v0.4.6
[0.4.5]: https://github.com/46b-ETYKiAL/Itasha.Corp_C0PL4ND/releases/tag/v0.4.5
[0.4.4]: https://github.com/46b-ETYKiAL/Itasha.Corp_C0PL4ND/releases/tag/v0.4.4
[0.4.3]: https://github.com/46b-ETYKiAL/Itasha.Corp_C0PL4ND/releases/tag/v0.4.3
