# Changelog

All notable changes to C0PL4ND are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project
adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

Full per-release artifacts (signed binaries, SBOMs, provenance) are on the
[GitHub Releases](https://github.com/46b-ETYKiAL/Itasha.Corp_C0PL4ND/releases)
page.

## [0.4.14] - 2026-07-03

### Fixed — rendering

- **Terminal-grid glyph garble eliminated.** Intermittently on some NVIDIA DX12
  drivers the grid text rendered as garbled or blank glyphs — a DX12 font-atlas
  `write_texture`→sample hazard (wgpu#1306 / #6829, DX12-only). Two-part fix: an
  atlas-warmup GPU fence that guarantees the font atlas is uploaded and resident
  before the grid draws (including across the off-thread system-font swap), and —
  the definitive fix — **Vulkan is now the default GPU backend on Windows**, which
  is immune to the hazard. DX12 remains one setting away (Settings → Window →
  Graphics backend, or `WGPU_BACKEND=dx12`) for anyone who hits a Vulkan
  overlay-layer issue.
- **"Make panes symmetrical" now produces an even grid.** It rebuilds the layout
  into a uniform grid for any pane count, instead of only rebalancing shares
  within existing splits (which left a nested/asymmetric layout uneven).

### Added — customizable toolbar

- **Settings → Toolbar.** The top-bar quick-action buttons (view-toggle,
  equalize, shell-switcher, script-launcher) are now fully customizable: place
  each on the LEFT (after the +), on the RIGHT (next to the settings gear), or in
  an overflow "…" menu — or hide it — and reorder within a zone. The script
  launcher now sits on the right by the gear by default.

### Changed — performance

- **Instant multi-pane close.** Closing the window no longer stalls while shells
  tear down: every pane's shell is killed in one pass and the blocking
  `ClosePseudoConsole` teardown is skipped on exit, with a Windows Job Object
  (`KILL_ON_JOB_CLOSE`) guaranteeing no orphaned `conhost`/`cmd` processes.
- Font-zoom (Ctrl+scroll) config writes are now debounced into a single write,
  and terminal synchronized-output (DEC `?2026`) is honored so full-screen TUIs
  repaint without tearing.

### Fixed — window transparency & tint

- **Transparency now covers the whole window.** The titlebar and status bar were
  opaque even with transparency on, so only the pane backgrounds went
  see-through. They now fold in the same opacity as the panes, so the entire
  window — chrome included — is translucent.
- **The color tint no longer discolors text or the Settings window.** It was a
  full-window overlay painted over everything; it is now a single wash on the
  background layer that shows through the panes, gaps, and top-bar buttons
  uniformly, while the terminal text and the (opaque) Settings window stay their
  true colors.
- **Opacity can go fully transparent.** The opacity slider floored at 15%; it now
  runs the whole 0–100% range for a much more see-through background (the text
  stays readable at its own opacity).
- **Top-bar buttons fit the bar again.** Once the titlebar went translucent, every
  control kept its own opaque background and read as a floating chip. The chrome
  buttons are now flat — no idle background, with a subtle fill only on hover or
  press — the standard for a translucent titlebar (Windows Terminal, VS Code,
  macOS vibrancy, libadwaita `.flat`), so they sit on the bar instead of over it.
- **Pane dividers now respect transparency and tint.** The seams between split
  terminals were painted as opaque bars that stayed solid regardless of the
  window opacity/tint. The divider is now negative space — the gap shows the same
  translucent, tinted background as the panes — and each pane's border folds in
  the window opacity, so a focused pane still reads clearly while the seams blend
  into the see-through window.

## [0.4.13] - 2026-06-30

### Added — macOS and ARM64 builds

- **Releases now ship for all six desktop targets**: Windows, Linux, and macOS,
  each in x64 and ARM64. macOS builds are not yet Apple-notarized (a certificate
  is pending) — see the README/TROUBLESHOOTING for the one-time Gatekeeper step.
  Update security is unchanged: every asset and the update manifest are
  minisign-signed and SLSA-attested, and verified before installing.

### Changed — supply-chain hygiene

- Resolved two dependency advisories at the source instead of suppressing them:
  `anyhow` is on the patched 1.0.103 (RUSTSEC-2026-0190) and `memmap2` was
  bumped to the patched 0.9.11 (RUSTSEC-2026-0186); both ignore entries dropped.

### Fixed — docs + internals

- Documented the GPU requirement (no software-render fallback) and the
  unsigned-installer/SmartScreen and macOS-Gatekeeper steps.
- Internal: the frame-scheduling policy (render-on-damage vs continuous) is now
  the typed `FramePolicy` contract shared with the renderer crate. No
  user-visible change.

## [0.4.12] - 2026-06-30

### Added — update diagnostics

- **Structured local diagnostics on the updater's security path.** The in-app
  updater now records, to the local diagnostic log only, what happened when an
  update is checked, downloaded, verified, and applied — including the specific
  reason an update was refused (checksum mismatch, signature failure, an
  insecure transport, or a downgrade attempt). These records carry only
  non-identifying detail (no URLs, tokens, or payload contents) and are never
  sent anywhere — C0PL4ND remains telemetry-free.

### Fixed — release verification

- The independent post-release verification job now runs automatically on every
  published release (previously it had to be started by hand). Each release's
  signed manifest, per-asset signatures, and checksum bindings are re-checked
  against the embedded key as soon as the release publishes.

### Changed — dependencies

- `anyhow` updated to 1.0.103; pinned GitHub Actions kept current. `egui` 0.35 /
  `egui_tiles` 0.16 are intentionally held until the icon-font dependency
  supports egui 0.35.

## [0.4.11] - 2026-06-30

### Changed — clearer error messages

- **User-visible error messages rewritten** to be plain-language and actionable,
  and to never expose internal details. Errors across the terminal, settings,
  updater, config/theme loading, and the `update` CLI now say what happened and
  what you can do about it, with the technical detail kept in local diagnostic
  logs instead of the message. No raw error chains, file paths, hostnames, or
  internal identifiers appear in any error you see.

### Added — release-pipeline hardening (Tier 2)

- Build-provenance (SLSA) attestation now covers **every** published asset,
  including the signed update manifest. An independent post-release verification
  job re-downloads the published assets and re-checks every signature, checksum,
  and the manifest binding against the embedded key — so a release can never
  publish an artifact the deployed app cannot verify.

## [0.4.10] - 2026-06-29

### Changed — update-security hardening

- **Signed update manifest (identity binding).** Each release now publishes a
  signed `latest.json` declaring, per platform, `{asset_name, sha256, size}` plus
  `{version, release_index, minimum_version, valid_until}`. The in-app updater
  verifies the manifest against the embedded key and binds every download to the
  **signed** SHA-256 — so an update's identity is a signed hash, not an unsigned
  field. The updater **requires** a verified manifest and refuses (rather than
  installing) if one is absent or unverifiable — there is no weaker path.
- **Downgrade, freeze, and rollback protection.** The updater refuses an older or
  equal version, a stale manifest (past its `valid_until`), a replayed
  `release_index`, and an install below the signed `minimum_version` floor.
- **Fail-closed release self-verification.** The release workflow verifies every
  signed artifact against the embedded public key (crypto + bare-filename
  identity + digest + count-parity + tamper test) before publishing, so a build
  can never ship an artifact the deployed client cannot verify.

Internal to the updater + release pipeline; no change to how you use C0PL4ND.
Updating from an earlier version works as it did before.

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
