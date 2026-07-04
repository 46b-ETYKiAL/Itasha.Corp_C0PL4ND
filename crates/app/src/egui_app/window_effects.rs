//! Window translucency, tint, and OS backdrop effects.
//!
//! Pane/window alpha + clear-colour computation, the egui tint overlay, and the
//! per-OS backdrop (acrylic / mica / vibrancy via `window-vibrancy`) — extracted
//! from the `egui_app` god-module. The colour math is pure (`&Config`/`&Theme`) and
//! unit-testable. Re-exported via `pub(crate) use window_effects::*`.

/// The minimum translucent BACKGROUND alpha (fraction). `0.0` = the pane/window
/// background can go FULLY transparent (only the terminal text — drawn at its own
/// full alpha — and the desktop remain), which is what "maximum transparency"
/// means. There is no readability floor because the grid TEXT alpha is independent
/// of this background alpha, so a near-zero background never hides the text; the
/// user drives the whole range with the opacity slider.
pub(crate) const TRANSLUCENT_ALPHA_FLOOR: f32 = 0.0;

/// The alpha (0..=255) to paint the pane grid background (and the central panel
/// fill) with, for the current config:
///
/// * **Opaque** (master toggle off, or `Opaque` mode): `255` — a solid fill so
///   the desktop never bleeds through. The unchanged, safe default.
/// * **Translucent** (`effective_translucent()`): the `opacity` slider folded
///   into a 0..=255 alpha (floored at [`TRANSLUCENT_ALPHA_FLOOR`], now `0.0`, so
///   the background can go fully transparent — text stays, drawn at its own
///   alpha). The opacity slider drives the fill alpha across its
///   FULL range in every translucent mode — Glass/Mica/Vibrancy are
///   distinguished by their DWM backdrop EFFECT (acrylic / mica / plain, applied
///   separately via `window-vibrancy`), NOT by capping the alpha. A prior
///   per-mode ceiling (#27) capped Glass at 0.35 etc., which made the slider a
///   no-op above the cap AND washed the terminal content out to near-invisible
///   over a bright backdrop (#41). The backdrop now shows through because the
///   DEFAULT opacity is < 1.0 (see `Config` default), not because the alpha is
///   force-capped — so opacity 1.0 legitimately means "fully opaque".
///
/// Pure (`&Config`) so the transparency wiring is unit-testable without a
/// window.
pub(crate) fn pane_bg_alpha(config: &c0pl4nd_core::Config) -> u8 {
    if !config.effective_translucent() {
        return 255;
    }
    // The opacity slider drives the alpha directly in ALL translucent modes,
    // floored so the grid stays readable. No per-mode ceiling: the modes differ
    // by their DWM backdrop, not by a forced alpha cap (#41).
    let a = config.opacity.clamp(TRANSLUCENT_ALPHA_FLOOR, 1.0);
    (a * 255.0).round().clamp(0.0, 255.0) as u8
}

/// The frameless-window clear color for the current config + theme.
///
/// * **Opaque** (master off, or `Opaque` mode): the theme background at full
///   alpha — a solid window the desktop never bleeds through.
/// * **Translucent with a native blur** (`Glass`/`Mica`/`Vibrancy`): fully
///   transparent so the OS blur backdrop shows through.
/// * **`Transparent` mode** (portable, no native blur): the theme background
///   with alpha folded down to the `opacity` slider so the desktop shows
///   through at the chosen strength.
///
/// Free function (takes `&Config`, `&Theme`) so the headless tests can assert
/// the clear color for a given config without an eframe window.
pub(crate) fn window_clear_color(
    config: &c0pl4nd_core::Config,
    theme: &c0pl4nd_core::Theme,
) -> [f32; 4] {
    if !config.effective_translucent() {
        // Opaque: solid theme background, full alpha.
        let (r, g, b) =
            c0pl4nd_core::theme::parse_hex(&theme.background).unwrap_or((0x12, 0x12, 0x12));
        return [r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, 1.0];
    }
    match config.window_mode {
        // Native blur backdrops want a fully transparent surface so the OS
        // composited blur shows through.
        c0pl4nd_core::config::WindowMode::Glass
        | c0pl4nd_core::config::WindowMode::Mica
        | c0pl4nd_core::config::WindowMode::Vibrancy => [0.0, 0.0, 0.0, 0.0],
        // Portable see-through: theme background, alpha = opacity slider directly.
        // The floor is now 0.0 (TRANSLUCENT_ALPHA_FLOOR), so the slider reaches a
        // FULLY transparent background at its low end — the terminal text (its own
        // alpha) stays visible over the desktop.
        c0pl4nd_core::config::WindowMode::Transparent => {
            let (r, g, b) =
                c0pl4nd_core::theme::parse_hex(&theme.background).unwrap_or((0x12, 0x12, 0x12));
            let a = config.opacity.clamp(TRANSLUCENT_ALPHA_FLOOR, 1.0);
            [r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, a]
        }
        // Unreachable: effective_translucent() ruled Opaque out above.
        c0pl4nd_core::config::WindowMode::Opaque => [0.0, 0.0, 0.0, 0.0],
    }
}

/// The 0..=255 alpha for a given tint `strength` (0..=1). Scaled so the slider's
/// top end is a clearly-visible wash without fully hiding the background. Pure, so
/// the mapping is unit-testable.
pub(crate) fn tint_alpha(strength: f32) -> u8 {
    (strength.clamp(0.0, 1.0) * 120.0).round() as u8
}

/// Paint the window tint as a single colour wash on the BACKGROUND layer, EARLY —
/// call this BEFORE any panel is shown. Because it is the first thing drawn on the
/// background layer, it sits BEHIND every translucent background fill (pane
/// backgrounds, the gaps between panes, the titlebar + status panels): at a low
/// opacity those fills let the wash show through UNIFORMLY across the whole window,
/// so panes, dividers, and the chrome/buttons are all tinted the same. Crucially it
/// is BEHIND the glyph text (drawn later on the same layer) and BELOW the Settings
/// window (a higher-order area), so neither the terminal text nor the settings UI
/// is ever tinted — the reported "tint colours the text / the buttons aren't
/// tinted" issues. A no-op when `tint_strength <= 0` or the hex is invalid.
pub(crate) fn paint_background_tint(ctx: &egui::Context, config: &c0pl4nd_core::Config) {
    if config.tint_strength <= 0.0 {
        return;
    }
    let Ok((r, g, b)) = c0pl4nd_core::theme::parse_hex(&config.tint) else {
        return;
    };
    let painter = ctx.layer_painter(egui::LayerId::background());
    painter.rect_filled(
        ctx.content_rect(),
        0.0,
        egui::Color32::from_rgba_unmultiplied(r, g, b, tint_alpha(config.tint_strength)),
    );
}

/// Paint the tint wash OVER a chrome panel's content (`ui.max_rect()`) — used for
/// the titlebar + status bar. Unlike the terminal grid (whose TEXT must stay
/// untinted, so the wash is painted BEHIND it via [`paint_background_tint`]), the
/// chrome's buttons/labels SHOULD carry the wash, so here it is painted on top at
/// the same `tint_alpha`. This gives the top bar + status bar the same tint the
/// panes get behind the background layer. A no-op when tint is off / hex invalid.
pub(crate) fn paint_tint_over(ui: &egui::Ui, config: &c0pl4nd_core::Config) {
    if config.tint_strength <= 0.0 {
        return;
    }
    let Ok((r, g, b)) = c0pl4nd_core::theme::parse_hex(&config.tint) else {
        return;
    };
    ui.painter().rect_filled(
        ui.max_rect(),
        0.0,
        egui::Color32::from_rgba_unmultiplied(r, g, b, tint_alpha(config.tint_strength)),
    );
}

/// Parse a `#RRGGBB` tint to an RGBA quad for native blur tinting.
///
/// Only consumed by Windows' `window_vibrancy::apply_acrylic` (acrylic takes a
/// tint; mica/vibrancy do not). Gating the fn to Windows keeps `-D warnings`
/// (clippy `dead_code`) green on Linux and macOS without a blanket allow.
#[cfg(windows)]
pub(crate) fn tint_rgba(hex: &str, alpha: u8) -> Option<(u8, u8, u8, u8)> {
    c0pl4nd_core::theme::parse_hex(hex)
        .ok()
        .map(|(r, g, b)| (r, g, b, alpha))
}

/// Apply the OS window effect for the chosen [`WindowMode`] (best-effort,
/// graceful on unsupported platforms — recon dossier §3.3). Windows:
/// acrylic (Glass) / mica (Mica); macOS: vibrancy; elsewhere (Linux) the
/// portable transparent surface + the tint overlay carry the look. Called only
/// when the master transparency toggle is on AND the mode wants a non-opaque
/// surface (`Config::effective_translucent`), so an opaque window never gets a
/// layered surface (no ghost-on-close risk).
///
/// LIVE-APPLY VERDICT (research §1): this is invoked ONCE at startup in
/// [`super::C0pl4ndApp::new`] because it needs the `eframe::CreationContext`'s raw
/// window handle, which `frame_tick` (driven only by `&egui::Context`) does not
/// expose — eframe 0.34 gives no stable cross-platform way to re-apply a DWM
/// backdrop class to the live window from inside the frame loop. So switching
/// the transparency MODE (Glass⇄Mica⇄Transparent) or toggling the master switch
/// at runtime needs a RELAUNCH for the DWM backdrop class to change. What IS
/// live: the PANEL/grid translucency — [`pane_bg_alpha`] reads `opacity` +
/// `effective_translucent()` from the config EVERY frame, so the opacity slider
/// and the pane see-through (the main visible lever) take effect immediately
/// without a relaunch.
pub(crate) fn apply_window_effect(
    cc: &eframe::CreationContext<'_>,
    mode: c0pl4nd_core::config::WindowMode,
    tint_hex: &str,
) {
    let _ = (cc, tint_hex);
    match mode {
        c0pl4nd_core::config::WindowMode::Glass => {
            #[cfg(windows)]
            {
                let _ = window_vibrancy::apply_acrylic(cc, tint_rgba(tint_hex, 160));
            }
            #[cfg(target_os = "macos")]
            {
                let _ = window_vibrancy::apply_vibrancy(
                    cc,
                    window_vibrancy::NSVisualEffectMaterial::HudWindow,
                    None,
                    None,
                );
            }
        }
        c0pl4nd_core::config::WindowMode::Mica => {
            #[cfg(windows)]
            {
                let _ = window_vibrancy::apply_mica(cc, Some(true));
            }
            #[cfg(target_os = "macos")]
            {
                let _ = window_vibrancy::apply_vibrancy(
                    cc,
                    window_vibrancy::NSVisualEffectMaterial::HudWindow,
                    None,
                    None,
                );
            }
        }
        c0pl4nd_core::config::WindowMode::Vibrancy => {
            #[cfg(target_os = "macos")]
            {
                let _ = window_vibrancy::apply_vibrancy(
                    cc,
                    window_vibrancy::NSVisualEffectMaterial::Sidebar,
                    None,
                    None,
                );
            }
        }
        // Transparent: the portable reduced-alpha surface carries the look (no
        // native blur). Opaque: no effect at all.
        c0pl4nd_core::config::WindowMode::Transparent
        | c0pl4nd_core::config::WindowMode::Opaque => {}
    }
}

#[cfg(test)]
mod tint_tests {
    use super::tint_alpha;

    #[test]
    fn tint_alpha_scales_and_clamps() {
        // Off / clamped low.
        assert_eq!(tint_alpha(0.0), 0);
        assert_eq!(tint_alpha(-1.0), 0);
        // Full strength = a clearly-visible (but not opaque) wash.
        assert_eq!(tint_alpha(1.0), 120);
        assert_eq!(tint_alpha(2.0), 120, "clamped above 1.0");
        // Mid strength scales linearly.
        assert_eq!(tint_alpha(0.5), 60);
    }
}
