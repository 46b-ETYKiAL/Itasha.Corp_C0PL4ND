# Changelog

All notable changes to C0PL4ND are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project
adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

Full per-release artifacts (signed binaries, SBOMs, provenance) are on the
[GitHub Releases](https://github.com/46b-ETYKiAL/Itasha.Corp_C0PL4ND/releases)
page.

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
