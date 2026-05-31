# C0PL4ND — Brand Guide

> *the operator's shell into the wired*

C0PL4ND is an Itasha.Corp product. It speaks the house dialect of the
**Retro-Future Anime OS** language: early-internet / Web 1.0 directness,
the wired melancholy of *Serial Experiments Lain*, the chrome-and-static of
early *Ghost in the Shell*, *Akira*-grade neon, and cassette-futurist CRT
texture. Everything reads like an operator's terminal that has been online
since before the network had a name.

---

## Palette

The two **brand primaries** below are the house colors of **every Itasha.Corp
app** and the foundation of the default theme (`assets/themes/itasha-corp.toml`).
Use **only** these hexes. No tints, no off-palette greys.

| Role | Name | Hex | Usage |
|------|------|-----|-------|
| **Brand primary — "Itasha"** | Itasha Purple | `#7700FF` | The `Itasha` half of the wordmark; structural accent — rings, frames, grid lines, links, the secondary chrome voice. |
| **Brand primary — ".Corp"** | Corp Green | `#00FF90` | The `.Corp` half of the wordmark; the live/cursor voice — cursor block, prompt chevron, primary accent, OK/success states. |
| Background | Void Black | `#0b0613` | Canvas, terminal body, plate fills. The default surface (a deep purple-black). |
| Alert | Signal Red | `#ff3b5c` | Errors, kill states, the red window dot. Use sparingly. |
| Light foreground | Ghost Paper | `#e8e6f0` | Body copy on void black, terminal output text, taglines. |

### Quick contrast rules
- Foreground text on Void Black: **Ghost Paper** for prose, **Corp Green** for live emphasis, **Itasha Purple** for structure.
- The two primaries carry equal weight: **Corp Green** is the *living* accent (cursor, prompt, success), **Itasha Purple** is the *structural* accent (frames, links, the secondary voice). Don't let one drown the other.
- Signal Red is reserved for genuine alert/error semantics. It is not decoration.

---

## Type Treatment

- **Family:** monospace / terminal first — `Courier New`, `DejaVu Sans Mono`,
  or any installed terminal mono. The product *is* a shell; the type should
  feel like fixed-cell output.
- **Corporate wordmark "Itasha.Corp":** always **two-tone** — `Itasha` in
  **Itasha Purple `#7700FF`** and `.Corp` in **Corp Green `#00FF90`** (the dot
  stays with `.Corp`). This split is the single most recognisable brand cue and
  is mandatory wherever the company name appears (about screens, footers,
  attributions, social cards). Never render `Itasha.Corp` in one flat color.
- **Product wordmark "C0PL4ND":** blocky, bold (700), wide letter-spacing (4–6).
  Note the leetspeak spelling — `0` for O, `4` for A. Always render it as
  `C0PL4ND`, never `COPLAND`. Drawn in Corp Green (the live accent).
- **Tagline:** lowercase, mono, Ghost Paper — *"the operator's shell into the wired"*.
- **Eyebrow / system lines:** prefix with a prompt token `> ` and use
  Status Teal or Signal Teal, small caps-feel via letter-spacing.

---

## Motif Usage

### Chromatic aberration
The wordmark casts a **Neon Pink** copy offset ~3–4px down/right beneath the
**Signal Teal** glyphs. This is the signature "bad CRT convergence" look.
Keep the offset small and consistent; it should read as signal bleed, not a
drop shadow.

### Scanlines
A faint horizontal line texture (1px line every 4–8px, Ghost Paper at
3–5% opacity) over Void Black. Always subtle — it is atmosphere, never a
foreground pattern. Scale the line spacing up on larger canvases.

### CRT curvature
On the app icon and full-bleed surfaces, a radial **vignette** darkens the
corners toward Void Black to suggest a curved tube. Optional inner teal glow
near the center. Keep edges soft.

### Kanji accents
Small CJK accents are encouraged as quiet signage, low opacity, Operator
Violet or muted Ghost Paper:
- 端末 — *terminal*
- 配線 — *wiring*

Never use kanji as the primary label or for meaning the audience must parse —
it is texture and atmosphere (the *Lain* "Wired" register), placed in corners
or window chrome.

### Registration ticks & grid
Corner L-ticks and a faint perspective **city grid** (violet horizontals +
teal vanishing-point verticals) evoke infrastructure signage and the
nightscape under the wired. Use for large cards (social, banner), not for the
compact mark.

### Prompt glyph
The core logomark is the shell prompt: a chevron `>` (Signal Teal) followed by
a **Status Teal cursor block**. This must remain legible at 16px.

---

## Asset Set

| File | Size | Purpose |
|------|------|---------|
| `wordmark.svg` | 720×180 | Horizontal logotype with chromatic aberration + cursor. |
| `logomark.svg` | 256×256 | Compact prompt mark; reads at 16px. |
| `app-icon.svg` | 512×512 | Desktop app icon source (CRT vignette + reg ticks). |
| `social-preview.svg` | 1280×640 | GitHub social / OG card. |
| `banner.svg` | 1280×320 | README hero banner with faux terminal. |

All assets are hand-authored standalone SVG: no external references, no raster
embeds, palette-locked.

---

## Do

- Keep Void Black as the dominant surface — let the neon breathe against dark.
- Use the chromatic-aberration offset on the wordmark, consistently sized.
- Render the product name exactly as `C0PL4ND`.
- Keep scanlines and CRT vignette subtle — texture, not pattern.
- Reserve Signal Red for true alert states.
- Treat Operator Violet as a line/structure color.

## Don't

- Don't recolor outside the seven palette hexes.
- Don't spell it `COPLAND`, `Copland`, or `C0pl4nd` (always all-caps `C0PL4ND`).
- Don't put Operator Violet body text on Void Black.
- Don't crank scanline opacity until the texture competes with content.
- Don't use kanji as a primary, must-read label.
- Don't add gradients beyond the defined CRT vignette / center glow.
- Don't place the logomark on a light background — the mark assumes Void Black.
