# C0PL4ND Release-Channel Verification Checklist

Run this checklist **per release**, after the `Release` workflow has published
the GitHub Release and its assets (binaries, installer, SBOM, build-provenance
text, checksums, minisig signatures). It smoke-tests every install channel the
README advertises so a releaser confirms each channel actually resolves an
installable artifact — not just that a manifest file exists in the repo.

> Honesty note: this checklist distinguishes **manifest present in-repo** from
> **channel live end-to-end**. Several manifests ship as *skeletons* with
> placeholder checksums that the release flow (or a one-time external
> submission) must finalize. Items marked **"manifest not yet live"** are NOT a
> passing channel until the noted prerequisite is done — do not advertise them
> as working until then.

---

## 0. Pre-flight (every channel depends on this)

- [ ] The GitHub Release for the tag exists and is **not** a draft.
- [ ] `gh release view <tag> --json assets -q '.assets[].name'` lists the
      expected assets: `*-x86_64-unknown-linux-gnu.tar.gz`,
      `*-x86_64-pc-windows-msvc.zip`, `c0pl4nd-<tag>-x86_64-setup.exe`,
      `*.cdx.json` (SBOM), `BUILD-PROVENANCE.txt`, `*.sha256`, `SHA256SUMS`,
      and (when signing is provisioned) `*.minisig` siblings.
- [ ] `gh attestation verify <asset> --repo 46b-ETYKiAL/Itasha.Corp_C0PL4ND`
      passes for at least one binary + the installer (SLSA build provenance).
- [ ] `BUILD-PROVENANCE.txt` records the `rustc -vV` identity and the
      `rust-toolchain.toml` pin + its sha256.

---

## 1. Windows — winget (`Itasha.C0PL4ND`)

**Manifest in-repo:** `packaging/windows/winget/Itasha.C0PL4ND.yaml` (singleton
skeleton). **Status: manifest not yet live in `microsoft/winget-pkgs`.** The
checked-in manifest carries placeholder `InstallerSha256`
(`0000…`) and `ProductCode` (`{0000…}`); `winget install Itasha.C0PL4ND` will
NOT resolve until the manifest is finalized with the real MSI URL + sha256 +
ProductCode and **submitted to the Windows Package Manager Community
Repository**. Create-before-advertising.

Smoke tests once the manifest is finalized + submitted:

- [ ] `winget show Itasha.C0PL4ND` — confirms the package is discoverable and
      shows the new `PackageVersion`.
- [ ] `winget install Itasha.C0PL4ND` (on a clean Windows VM) — installs under
      *Program Files*, adds to `PATH`, creates the Start Menu entry.
- [ ] `winget show Itasha.C0PL4ND --versions` — the new release tag is listed.
- [ ] Confirm `InstallerSha256` in the submitted manifest matches the MSI's
      sha256 from `SHA256SUMS`.

Validate the local manifest before submitting:

- [ ] `winget validate --manifest packaging/windows/winget/` (or the split
      multi-file form) — schema-validates the manifest.

---

## 2. Windows — MSI / native installer (direct download)

**In-repo:** WiX source `packaging/windows/c0pl4nd.wxs` + the native
Itasha.Corp installer built in `release.yml` (`windows-installer` job →
`c0pl4nd-<tag>-x86_64-setup.exe`). This channel IS produced by the release
flow.

- [ ] Download `c0pl4nd-<tag>-x86_64-setup.exe` from the Release.
- [ ] Verify checksum: `Get-FileHash c0pl4nd-<tag>-x86_64-setup.exe -Algorithm SHA256`
      matches the `.sha256` sibling.
- [ ] Run the installer on a clean VM; confirm it installs under
      `C:\Program Files\Itasha.Corp\C0PL4ND`, and `c0pl4nd --version` prints the
      new version from a fresh shell.
- [ ] If SignPath signing is provisioned (`SIGNPATH_ORG_ID` set), confirm the
      installer is Authenticode-signed (right-click → Properties → Digital
      Signatures).

---

## 3. macOS — Homebrew cask (`brew install --cask c0pl4nd`)

**Manifest in-repo:** `packaging/macos/c0pl4nd.rb` (cask skeleton).
**Status: manifest not yet live.** The cask carries placeholder `sha256`
values (`0000…` / `1111…`) and points at a `version "0.1.0"` with
`itasha-corp/c0pl4nd` URLs. The bare README command `brew install --cask
c0pl4nd` only resolves if the cask is published to **homebrew-cask** OR a tap
(the cask's own header says distribute via `itasha-corp/homebrew-tap` →
`brew install --cask itasha-corp/tap/c0pl4nd`). No tap repo is confirmed
present. Additionally, the macOS DMG this cask points at is **not built by the
current `release.yml`** (the release matrix ships Linux + Windows only;
`packaging/macos/build-dmg.sh` exists but is not wired into CI) — so the cask's
download URLs would 404. Create-before-advertising: build + publish the DMG and
the tap, and fill the real `version` + per-arch `sha256`, before advertising.

Smoke tests once the cask + DMG are live:

- [ ] `brew info --cask itasha-corp/tap/c0pl4nd` (or `c0pl4nd` if in
      homebrew-cask) — shows the new version.
- [ ] `brew install --cask itasha-corp/tap/c0pl4nd` on a clean macOS — installs
      `C0PL4ND.app` and the `c0pl4nd` CLI symlink.
- [ ] `c0pl4nd --version` prints the new version.
- [ ] `brew audit --cask --new c0pl4nd` (or `brew style packaging/macos/c0pl4nd.rb`)
      passes for the finalized cask.
- [ ] Confirm each arch's `sha256` in the cask matches the published DMG.

---

## 4. Linux — AppImage (download + `chmod +x` + run)

**In-repo:** `packaging/linux/build-appimage.sh` (builds
`c0pl4nd-v<version>-<arch>.AppImage`). **Status: build script present, but the
AppImage is NOT attached by the current `release.yml`** (the release matrix
produces `.tar.gz` + `.zip` + `.exe`/installer only; no AppImage job). The
README's `Releases` AppImage download therefore has nothing to download until
an AppImage build+upload step is added or the script is run manually and the
asset uploaded. Note also the README uses `C0PL4ND-*.AppImage` (uppercase)
while the script emits `c0pl4nd-v<ver>-<arch>.AppImage` (lowercase) —
reconcile the casing in either the README glob or the script output.
Create-before-advertising: wire the AppImage into the release (or upload it
manually) before advertising the channel.

Smoke tests once an AppImage asset is attached to the Release:

- [ ] Download the AppImage from the Release page.
- [ ] `chmod +x c0pl4nd-v<tag>-x86_64.AppImage`
- [ ] `./c0pl4nd-v<tag>-x86_64.AppImage --version` prints the new version.
- [ ] On a host without FUSE: `./c0pl4nd-*.AppImage --appimage-extract-and-run --version`
      still works (FUSE-less fallback).
- [ ] Verify the AppImage's `.sha256` sibling (if uploaded) matches.

---

## 5. Linux/macOS — install script (`curl … | sh`)

**In-repo:** `packaging/linux/install.sh` (detects OS/arch, downloads the
latest release tarball, verifies sha256, installs to `~/.local/bin`).
**Status: script present and functional against the GitHub Releases tarballs**
(which the release flow DOES produce). Caveat: the README advertises
`https://get.c0pl4nd.dev/install.sh`, but the script's own usage header points
at `https://raw.githubusercontent.com/itasha-corp/c0pl4nd/main/packaging/linux/install.sh`.
The `get.c0pl4nd.dev` vanity domain is **unverified** — confirm it resolves and
serves the current script, or update the README to the raw GitHub URL.
The script's `REPO="itasha-corp/c0pl4nd"` must also match the actual release
repo (`46b-ETYKiAL/Itasha.Corp_C0PL4ND`) for the download URLs to resolve —
verify/realign before advertising.

Smoke tests:

- [ ] **Dry-run / inspect first (never pipe-to-shell blind):**
      `curl -fsSL <install-url> -o /tmp/install.sh && less /tmp/install.sh` —
      review before running.
- [ ] `sh /tmp/install.sh` on a clean Linux container — confirm it downloads
      the new tarball, the sha256 check passes, and the binary lands in
      `~/.local/bin/c0pl4nd`.
- [ ] `C0PL4ND_VERSION=<tag> sh /tmp/install.sh` — version pin honored.
- [ ] `~/.local/bin/c0pl4nd --version` prints the new version.
- [ ] Confirm the `<install-url>` advertised in the README actually serves this
      script (resolve `get.c0pl4nd.dev` or switch the README to the raw URL).
- [ ] Confirm `REPO` in the script equals the real release repo.

---

## 6. Portable archives (zip / tar.gz — no installer)

These are produced directly by the `build` job and are the most reliable
channel.

- [ ] Download `c0pl4nd-<tag>-x86_64-pc-windows-msvc.zip` (Windows) /
      `c0pl4nd-<tag>-x86_64-unknown-linux-gnu.tar.gz` (Linux).
- [ ] Verify against `SHA256SUMS`.
- [ ] Extract, run `c0pl4nd --version` (Linux: `./c0pl4nd`; Windows:
      `c0pl4nd.exe`).

---

## Channel status summary (this release)

| Channel | Manifest/asset in-repo | Built/attached by `release.yml` | Live end-to-end? |
|---------|------------------------|---------------------------------|------------------|
| winget `Itasha.C0PL4ND` | Yes (skeleton, placeholder sha256/ProductCode) | No (manifest values rewritten per release, but **not submitted**) | **No** — submit to `microsoft/winget-pkgs` first |
| MSI / native setup.exe | Yes (`.wxs` + native installer job) | **Yes** (`c0pl4nd-<tag>-x86_64-setup.exe`) | Yes (direct download) |
| brew `--cask c0pl4nd` | Yes (skeleton, placeholder sha256) | No (DMG not built in CI) | **No** — build+publish DMG + tap, fill sha256 |
| AppImage | Yes (`build-appimage.sh`) | **No** (no AppImage in release matrix) | **No** — wire AppImage into release or upload manually |
| `curl … \| sh` | Yes (`install.sh`) | n/a (consumes release tarballs) | Partial — works against tarballs; **verify vanity domain + REPO** |
| zip / tar.gz | n/a | **Yes** | Yes |

Update this table each release as channels move from "manifest not yet live" to
"live end-to-end."
