# C0PL4ND transparency simplification — v0.4.21

**Status:** built, green, pushed, PR open. **NOT released** — needs the user's eyes on the look.

## Binary ready to launch

```
../c0pl4nd-trans-simplify/target/release/c0pl4nd.exe
```

(absolute: `C:/Users/.46b_/Itasha.Corp_S4F3-R0UT3-4RB1T3R/.s4f3-data/pubrepo-work/c0pl4nd-trans-simplify/target/release/c0pl4nd.exe`)

### Latest additions (v0.4.21, folded into PR #277)
- **Opacity is now clean + linear** — the terminal background was double-painted (CentralPanel fill + per-pane fill), compounding to ≈opacity² (0.7 → ~0.91 haze). Now painted once; verified on-GPU: opacity 0.7 → backing alpha **179** (linear), not 125 (squared).
- **Software "frosted glass"** (Settings → Appearance → Frosted glass): toggle + amount slider + colour picker + grain checkbox. Independent of opacity. Honest hover: "does not blur the desktop." Verified on-GPU: frost off backing=77 → on=161.
- **Tint un-coupled from opacity** — it now works at any opacity (colours the glass) instead of fading away. Opacity / Tint / Frost are three independent controls; fully-clear = tint-off + frost-off.
- **All Settings dropdowns** are fixed-width with an up/down ▲/▼ stepper (spinner) to the right, cycling options with wrap-around — the combo/stepper no longer move with the value length.

The main thread should launch this so the user can confirm:
1. **Opacity 0 = truly clear (no frosted wash)** — the frost was a tint colour wash painting at a fixed alpha regardless of opacity (the user's config has `opacity = 0.0`, `tint_strength = 0.18`). The tint + ambient effects now fade with opacity, so at 0% only glyph text remains over the desktop. Verified headlessly: opacity-0 surface is <3% non-transparent even with tint + mesh ON.
2. **Tint color picker fully visible** — the wired-mesh/effects now render strictly behind popups/color-pickers (moved to `Order::Middle`).
3. **Opacity 0 = maximally see-through** (panes + resting chrome fade; hover/press, popups, Settings stay legible).
4. **UI-scale slider no longer runs away** (rescales only on release).

## What changed

### Task 1 — one Transparent effect
- Removed `WindowMode` (Opaque/Transparent/Dim/Glass/Mica/Vibrancy), `window_mode`, `acrylic`, `transparency_enabled`, `effective_translucent()`, `migrate_legacy_transparency()` from the config.
- Removed the mode dropdown + master toggle from Settings; removed `apply_window_effect`, the `window-vibrancy` dependency, the Dim layered-window FFI, and the crash-loop `startup_recovery` module.
- Window is always `with_transparent(true)`; clear colour always `[0,0,0,0]`; `pane_bg_alpha` folds opacity directly (`1.0`→255 solid, `0.0`→0). One **Opacity** slider + tint toggle/colour/strength; gpu-diag kept.
- Old configs still load: retired `window_mode`/`acrylic`/`transparency_enabled` keys are ignored (serde unknown-field drop); retained `opacity` carries the level. Config-migration test added.

### Task 2 — fade resting chrome (SCR1B3 v0.4.59 port)
- New `window_effects::apply_window_opacity()` fades resting `noninteractive.bg_fill` + `inactive.weak_bg_fill` with the opacity alpha (called after each `set_visuals`), so the shell fades with the panes. Hover/active, scrollbar handle, and `window_fill` (popups/tooltips/Settings) stay opaque.
- Opacity floor confirmed `0.0`.
- Note: the SCR1B3 public scan is at v0.4.57 (not v0.4.59 yet), so the behaviour was implemented from the task's spec, not copied verbatim.

### Follow-up bug — UI-scale slider runaway (fixed in this branch)
- The UI-scale slider wrote `config.ui_scale` live every frame, and the frame loop applies `set_zoom_factor` the moment it changes — so dragging rescaled the slider under the cursor, remapped the pointer, and ran the scale away to the 3.0 max (UI gigantic/unusable).
- Fix (`settings.rs`): the slider is driven from a per-frame working value in egui temp memory while dragged; the value commits to `config.ui_scale` (the sole `set_zoom_factor` trigger) only when NOT dragging — release / track click / keyboard step (pure `ui_scale_commit` helper). One rescale per interaction, never mid-drag. `effective_ui_scale()` keeps its 0.5..=3.0 + non-finite clamp as a safety net; reset-to-default still works. Unit tests added.

## Verification
- `cargo check --workspace --all-targets` — clean
- `cargo clippy --workspace --all-targets -- -D warnings` — clean
- `cargo fmt --all` — applied
- `cargo test --workspace` — green (one pre-existing GPU render timing flake, `terminal_renders_both_panes_through_real_frame`, passed on retry; unrelated to this change)
- `cargo build --release` — clean (10m15s)
- version 0.4.20 → 0.4.21 + `cargo update -w`

## Git
- Branch `feat/transparency-simplify`, 2 commits (core model collapse; app + fade-chrome + tests + docs).
- PR: https://github.com/46b-ETYKiAL/Itasha.Corp_C0PL4ND/pull/277
