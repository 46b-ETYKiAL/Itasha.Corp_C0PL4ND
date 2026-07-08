# C0PL4ND transparency simplification — v0.4.21

**Status:** built, green, pushed, PR open. **NOT released** — needs the user's eyes on the look.

## Binary ready to launch

```
../c0pl4nd-trans-simplify/target/release/c0pl4nd.exe
```

(absolute: `C:/Users/.46b_/Itasha.Corp_S4F3-R0UT3-4RB1T3R/.s4f3-data/pubrepo-work/c0pl4nd-trans-simplify/target/release/c0pl4nd.exe`)

The main thread should launch this so the user can confirm **opacity 0 = maximally see-through** (Settings → Appearance → Opacity slider to 0%: the panes AND the resting chrome — toolbar buttons, tab chips, title bar — should fade away, leaving only the glyph text over the desktop; hover/press, popups, and the Settings window stay legible).

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
