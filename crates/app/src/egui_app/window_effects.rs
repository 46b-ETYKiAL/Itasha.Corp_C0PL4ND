//! Window translucency and tint.
//!
//! The window is ALWAYS created transparent-capable (`with_transparent`); a single
//! **opacity** slider (0.0 = fully see-through, 1.0 = solid) drives the pane +
//! resting-chrome fill alpha, and an optional tint wash colours the window. There
//! is no window-mode selector and no OS blur backdrop (acrylic / mica / vibrancy):
//! on the hybrid-GPU (Optimus) target those backdrops never composited, so the one
//! effect that works — the portable per-pixel transparent surface — is the only
//! effect. The colour math is pure (`&Config`) and unit-testable. Re-exported via
//! `pub(crate) use window_effects::*`.

/// The minimum BACKGROUND alpha (fraction). `0.0` = the pane/window background can
/// go FULLY transparent (only the terminal text — drawn at its own full alpha —
/// and the desktop remain), which is what "maximum transparency" means. There is
/// no readability floor because the grid TEXT alpha is independent of this
/// background alpha, so a near-zero background never hides the text; the user
/// drives the whole range with the opacity slider.
pub(crate) const TRANSLUCENT_ALPHA_FLOOR: f32 = 0.0;

/// The alpha (0..=255) to paint the pane grid background (and the central panel
/// fill) with, for the current config: the `opacity` slider folded into a 0..=255
/// alpha (floored at [`TRANSLUCENT_ALPHA_FLOOR`], `0.0`, so the background can go
/// fully transparent — text stays, drawn at its own alpha). `opacity == 1.0`
/// yields `255` (a solid fill — the "opaque" look) and `0.0` yields `0` (maximum
/// see-through). Pure (`&Config`) so the transparency wiring is unit-testable
/// without a window.
pub(crate) fn pane_bg_alpha(config: &c0pl4nd_core::Config) -> u8 {
    let a = config.opacity.clamp(TRANSLUCENT_ALPHA_FLOOR, 1.0);
    (a * 255.0).round().clamp(0.0, 255.0) as u8
}

/// The frameless-window clear color: unconditionally FULLY transparent
/// `[0,0,0,0]` — exactly like the sibling app SCR1B3 (whose `clear_color` is
/// unconditionally `[0,0,0,0]`), which IS see-through on the hybrid-GPU laptop.
/// The `opacity` slider is folded ONLY into the PANEL fills ([`pane_bg_alpha`]),
/// NEVER the clear.
///
/// Why this matters (the transparency bug): eframe issues the clear as the wgpu
/// render-pass `LoadOp::Clear`, so it sets the base framebuffer alpha for EVERY
/// pixel before egui paints. An old build cleared to `[theme_bg, opacity]`; egui
/// then painted the panels (also `[theme_bg, opacity]`) ON TOP, so the two alphas
/// COMPOUNDED (`0.6` clear over `0.6` panel ≈ `0.84`) and the RGB darkened — the
/// window read as near-opaque BLACK well before the slider reached 100%. Clearing
/// to `[0,0,0,0]` makes the panel alpha the SOLE determinant of see-through, so
/// `opacity` behaves linearly across its whole range: at `1.0` the opaque panels
/// cover the transparent clear (solid look); below `1.0` the desktop shows through.
pub(crate) fn window_clear_color() -> [f32; 4] {
    [0.0, 0.0, 0.0, 0.0]
}

/// Fade the RESTING chrome + background fills in `v` by the window `opacity`
/// (0.0..=1.0) so that at low opacity the toolbar / tab / title-bar SHELL goes
/// see-through along with the panes — leaving only the glyph text (painted opaque
/// on top) legible over the desktop at `opacity == 0.0`. Ported from SCR1B3
/// (v0.4.59 fade-chrome): the resting `noninteractive.bg_fill` + the `inactive`
/// `weak_bg_fill` (the idle button/tab chip fill) fade with the alpha, while:
///
/// * **hovered / active** widget fills stay OPAQUE — pointer feedback must always
///   read, even over a fully see-through window;
/// * the scrollbar HANDLE (`inactive.bg_fill`, which egui paints the idle handle
///   from) stays OPAQUE so the scrollbar never vanishes;
/// * `window_fill` stays OPAQUE — egui draws combo dropdowns, context menus,
///   tooltips, and the floating Settings window from it, so fading it would make
///   every popup/tooltip see-through and the Settings window darken toward black.
///
/// Pure, so the fade is unit-testable without a window. Applied right after
/// `set_visuals(visuals_from_theme(..))` (each theme/opacity change re-applies the
/// visuals), so the opacity slider drives the resting chrome live.
pub(crate) fn apply_window_opacity(v: &mut egui::Visuals, opacity: f32) {
    let a = (opacity.clamp(0.0, 1.0) * 255.0).round() as u8;
    let with_a = |c: egui::Color32| egui::Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), a);
    // Background / panel surfaces (what sits over the desktop).
    v.panel_fill = with_a(v.panel_fill);
    v.extreme_bg_color = with_a(v.extreme_bg_color);
    v.faint_bg_color = with_a(v.faint_bg_color);
    // Resting chrome: the non-interactive surface fill and the idle button/tab
    // chip fill fade so the toolbar/tab/title-bar shell is see-through at low
    // opacity. Hovered/active/`inactive.bg_fill` (scrollbar handle) are left
    // untouched (opaque) for feedback + legibility, and `window_fill` stays opaque
    // so popups/tooltips/Settings hold their colour.
    v.widgets.noninteractive.bg_fill = with_a(v.widgets.noninteractive.bg_fill);
    v.widgets.noninteractive.weak_bg_fill = with_a(v.widgets.noninteractive.weak_bg_fill);
    v.widgets.inactive.weak_bg_fill = with_a(v.widgets.inactive.weak_bg_fill);
}

/// The 0..=255 alpha for a given tint `strength` (0..=1). Scaled so the slider's
/// top end is a clearly-visible wash without fully hiding the background. Pure, so
/// the mapping is unit-testable.
pub(crate) fn tint_alpha(strength: f32) -> u8 {
    (strength.clamp(0.0, 1.0) * 90.0).round() as u8
}

/// The tint wash alpha folded with the window `opacity`, so the tint fades WITH
/// the window: `opacity == 0.0` yields `0` (fully clear — the wash vanishes along
/// with the panes, so the maximally-transparent window shows ONLY glyph text over
/// the desktop), and `opacity == 1.0` yields the full [`tint_alpha`] weight. The
/// tint is painted on the background layer BEHIND the translucent panels, so
/// without this fold it stayed at a fixed alpha and showed straight through the
/// see-through panels at opacity 0 as a frosted haze. Pure → unit-testable.
pub(crate) fn tint_wash_alpha(strength: f32, opacity: f32) -> u8 {
    (f32::from(tint_alpha(strength)) * opacity.clamp(0.0, 1.0)).round() as u8
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
/// tinted" issues. A no-op when the tint master toggle
/// ([`c0pl4nd_core::Config::tint_enabled`]) is off, when `tint_strength <= 0`, or
/// when the hex is invalid.
///
/// The wash alpha is folded with the window `opacity` (see [`tint_wash_alpha`]),
/// so the tint FADES WITH the window: at opacity 0 it vanishes completely (only
/// the glyph text remains over the desktop — no frosted colour wash left on top),
/// climbing back to its `tint_strength` weight as the window approaches solid.
/// Without this the tint painted at a FIXED alpha on the background layer and
/// showed straight through the fully-transparent panels at opacity 0 as a uniform
/// frosted haze — the "opacity 0 still looks frosted" report.
pub(crate) fn paint_background_tint(ctx: &egui::Context, config: &c0pl4nd_core::Config) {
    // Explicit master switch first: when the user has toggled the tint wash OFF,
    // paint nothing even if a colour + strength are still parked in the config.
    if !config.tint_enabled || config.tint_strength <= 0.0 {
        return;
    }
    let alpha = tint_wash_alpha(config.tint_strength, config.opacity);
    if alpha == 0 {
        return;
    }
    let Ok((r, g, b)) = c0pl4nd_core::theme::parse_hex(&config.tint) else {
        return;
    };
    let painter = ctx.layer_painter(egui::LayerId::background());
    painter.rect_filled(
        ctx.content_rect(),
        0.0,
        egui::Color32::from_rgba_unmultiplied(r, g, b, alpha),
    );
}

/// Style the chrome (titlebar + status bar) buttons as FLAT: no idle background,
/// with a subtle fill appearing ONLY on hover / press. This is the industry
/// standard for a translucent titlebar — Windows Terminal (`useAcrylicInTabRow`),
/// VS Code's titlebar toolbar, macOS vibrancy title-bar controls, and libadwaita
/// `.flat` header buttons all draw idle icon-buttons with NO background so they
/// read as part of the bar. A per-button opaque fill (what egui draws by default
/// from `inactive.weak_bg_fill`) turns every control into a floating chip once the
/// bar itself goes translucent — the reported "buttons don't fit the top bar" bug.
///
/// Scoped to the chrome `ui`: the Settings window + overlays (separate uis) keep
/// their normal filled buttons. `dark` picks a white hover-veil on dark themes and
/// a black one on light, so the hover reads over whatever desktop shows through.
/// Called each frame, so it tracks live opacity/theme changes. The window tint
/// reaches the bar as a background wash through the translucent panel fill (see
/// [`paint_background_tint`]) — never as a flat film painted over the buttons.
pub(crate) fn flatten_chrome_buttons(ui: &mut egui::Ui, dark: bool) {
    let veil = |a: u8| {
        if dark {
            egui::Color32::from_white_alpha(a)
        } else {
            egui::Color32::from_black_alpha(a)
        }
    };
    let widgets = &mut ui.visuals_mut().widgets;
    // Idle = frameless: no chip, no border. The icon/label sits on the bar itself.
    widgets.inactive.weak_bg_fill = egui::Color32::TRANSPARENT;
    widgets.inactive.bg_fill = egui::Color32::TRANSPARENT;
    widgets.inactive.bg_stroke = egui::Stroke::NONE;
    // A subtle fill appears only under the pointer / while pressed — the sole
    // affordance, shared across every mode so a button can never revert to a chip.
    widgets.hovered.weak_bg_fill = veil(20);
    widgets.hovered.bg_fill = veil(20);
    widgets.active.weak_bg_fill = veil(32);
    widgets.active.bg_fill = veil(32);
}

/// Fold the window-transparency alpha (`bg_alpha`, 0..=255 — the same value
/// [`pane_bg_alpha`] paints the pane bodies with) into a chrome stroke colour so
/// the stroke is exactly as translucent as the panes and never out-paints them.
///
/// This is the fix for the pane-divider/border reading as a hard OPAQUE line that
/// is "unaffected by tint or transparency": the per-pane bezel/focus border was
/// drawn at full alpha over the translucent framebuffer, so it sat ON TOP of the
/// see-through window as a solid bar. Multiplying its alpha by the window alpha
/// makes the border fade into negative space as the window goes see-through
/// (the kitty/i3 "the seam is the surface" model) and tint uniformly with the
/// rest of the chrome at partial opacity. `bg_alpha == 255` (opaque window)
/// returns the colour unchanged, so the opaque path is byte-for-byte untouched.
///
/// egui `Color32` is premultiplied, so [`egui::Color32::gamma_multiply`] scales
/// every premultiplied channel together — the premult-correct way to lower a
/// colour's effective opacity. Pure, so the fold is unit-testable without a window.
pub(crate) fn fold_alpha(color: egui::Color32, bg_alpha: u8) -> egui::Color32 {
    if bg_alpha == 255 {
        return color;
    }
    color.gamma_multiply(f32::from(bg_alpha) / 255.0)
}

#[cfg(test)]
mod tint_tests {
    use super::{flatten_chrome_buttons, fold_alpha, tint_alpha, tint_wash_alpha};

    #[test]
    fn fold_alpha_passes_opaque_through_and_scales_translucent() {
        let accent = egui::Color32::from_rgb(0x00, 0xFF, 0x90); // brand green, opaque
                                                                // Opaque window (bg_alpha 255): the colour is returned UNCHANGED so the
                                                                // existing opaque divider path is byte-for-byte identical.
        assert_eq!(
            fold_alpha(accent, 255),
            accent,
            "opaque window must not alter the stroke colour"
        );
        // Fully transparent fold collapses the stroke to nothing (negative-space
        // seam) — the divider cannot out-paint a fully see-through window.
        assert_eq!(
            fold_alpha(accent, 0).a(),
            0,
            "zero window alpha → invisible seam"
        );
        // A partial window opacity yields a partial, non-zero, non-opaque stroke
        // that tints/fades with the window instead of sitting on top of it.
        let folded = fold_alpha(accent, 128);
        assert!(
            folded.a() > 0 && folded.a() < 255,
            "half opacity → translucent stroke (got alpha {})",
            folded.a()
        );
    }

    #[test]
    fn pane_bg_alpha_folds_opacity_across_the_full_range() {
        // Single-model: the opacity slider drives the pane fill alpha directly.
        // 1.0 → 255 (solid), 0.0 → 0 (fully see-through), and it is monotonic.
        let mk = |o: f32| {
            super::pane_bg_alpha(&c0pl4nd_core::Config {
                opacity: o,
                ..Default::default()
            })
        };
        assert_eq!(mk(1.0), 255, "opacity 1.0 is a solid (opaque) fill");
        assert_eq!(mk(0.0), 0, "opacity 0.0 is a fully transparent fill");
        assert_eq!(mk(0.6), (0.6 * 255.0_f32).round() as u8);
        assert!(mk(0.2) < mk(0.5) && mk(0.5) < mk(0.85) && mk(0.85) < mk(1.0));
    }

    #[test]
    fn window_clear_color_is_always_fully_transparent() {
        // The clear is unconditionally [0,0,0,0]; the opacity slider drives the
        // PANEL alpha, never the clear (the SCR1B3-parity anti-"opaque-black" fix).
        assert_eq!(super::window_clear_color(), [0.0, 0.0, 0.0, 0.0]);
    }

    #[test]
    fn apply_window_opacity_fades_resting_chrome_and_keeps_feedback_opaque() {
        // The fade-chrome port: the resting background + chrome fills fade with the
        // opacity alpha (so the whole shell goes see-through at opacity 0), while
        // hovered/active fills, the scrollbar handle (inactive.bg_fill), and
        // window_fill (popups/tooltips/Settings) stay OPAQUE for feedback/legibility.
        let mut v = egui::Visuals::dark();
        v.window_fill = egui::Color32::from_rgb(0x20, 0x20, 0x20);
        v.panel_fill = egui::Color32::from_rgb(0x10, 0x10, 0x10);
        v.extreme_bg_color = egui::Color32::from_rgb(0x05, 0x05, 0x05);
        v.faint_bg_color = egui::Color32::from_rgb(0x15, 0x15, 0x15);
        v.widgets.noninteractive.bg_fill = egui::Color32::from_rgb(0x11, 0x11, 0x11);
        v.widgets.inactive.weak_bg_fill = egui::Color32::from_rgb(0x22, 0x22, 0x22);
        v.widgets.inactive.bg_fill = egui::Color32::from_rgb(0x33, 0x33, 0x33);
        v.widgets.hovered.weak_bg_fill = egui::Color32::from_rgb(0x44, 0x44, 0x44);
        v.widgets.active.weak_bg_fill = egui::Color32::from_rgb(0x55, 0x55, 0x55);

        // Half opacity: resting fills go ~half translucent.
        super::apply_window_opacity(&mut v, 0.5);
        assert_eq!(v.panel_fill.a(), 128, "panel fades");
        assert_eq!(v.extreme_bg_color.a(), 128);
        assert_eq!(v.faint_bg_color.a(), 128);
        assert_eq!(
            v.widgets.noninteractive.bg_fill.a(),
            128,
            "resting chrome fades"
        );
        assert_eq!(v.widgets.inactive.weak_bg_fill.a(), 128, "idle chip fades");
        // Feedback + legibility surfaces stay opaque.
        assert_eq!(v.window_fill.a(), 255, "window_fill (popups) stays opaque");
        assert_eq!(
            v.widgets.inactive.bg_fill.a(),
            255,
            "scrollbar handle stays opaque"
        );
        assert_eq!(
            v.widgets.hovered.weak_bg_fill.a(),
            255,
            "hover stays opaque"
        );
        assert_eq!(
            v.widgets.active.weak_bg_fill.a(),
            255,
            "active stays opaque"
        );

        // Opacity 0.0 → resting chrome fully transparent (only glyphs remain).
        let mut z = egui::Visuals::dark();
        z.panel_fill = egui::Color32::from_rgb(0x10, 0x10, 0x10);
        z.widgets.noninteractive.bg_fill = egui::Color32::from_rgb(0x11, 0x11, 0x11);
        z.widgets.inactive.weak_bg_fill = egui::Color32::from_rgb(0x22, 0x22, 0x22);
        super::apply_window_opacity(&mut z, 0.0);
        assert_eq!(z.panel_fill.a(), 0);
        assert_eq!(z.widgets.noninteractive.bg_fill.a(), 0);
        assert_eq!(z.widgets.inactive.weak_bg_fill.a(), 0);
        assert_eq!(
            z.window_fill.a(),
            255,
            "window_fill opaque even at opacity 0"
        );
    }

    #[test]
    fn tint_alpha_scales_and_clamps() {
        // Off / clamped low.
        assert_eq!(tint_alpha(0.0), 0);
        assert_eq!(tint_alpha(-1.0), 0);
        // Full strength = a clearly-visible (but not opaque) wash (~35% max).
        assert_eq!(tint_alpha(1.0), 90);
        assert_eq!(tint_alpha(2.0), 90, "clamped above 1.0");
        // Mid strength scales linearly.
        assert_eq!(tint_alpha(0.5), 45);
    }

    #[test]
    fn tint_wash_alpha_folds_opacity_so_it_vanishes_at_zero() {
        // The frost fix: the tint wash alpha is folded with the window opacity, so
        // at opacity 0 the tint is GONE (no frosted colour wash over a maximally-
        // transparent window — only glyph text remains), and at opacity 1 it is the
        // full `tint_alpha` weight.
        // Opacity 0 → fully clear regardless of strength (kills the frost).
        assert_eq!(
            tint_wash_alpha(1.0, 0.0),
            0,
            "opacity 0 → no tint wash at all"
        );
        assert_eq!(tint_wash_alpha(0.5, 0.0), 0);
        // Opacity 1 → the unmodified tint weight.
        assert_eq!(tint_wash_alpha(1.0, 1.0), tint_alpha(1.0));
        assert_eq!(tint_wash_alpha(0.5, 1.0), tint_alpha(0.5));
        // Mid opacity scales the wash down proportionally (strength 1 → alpha 90).
        assert_eq!(tint_wash_alpha(1.0, 0.5), 45);
        // Monotonic in opacity for a fixed strength (more solid ⇒ stronger wash).
        assert!(tint_wash_alpha(1.0, 0.25) < tint_wash_alpha(1.0, 0.75));
    }

    #[test]
    fn flatten_chrome_buttons_makes_idle_frameless_and_hover_visible() {
        // The core contract: after flattening, an idle chrome button paints NO
        // background (so it can never float as an opaque chip over a translucent
        // bar), while hover/press DO paint a subtle veil (the sole affordance).
        egui::__run_test_ui(|ui| {
            // Precondition: egui's default idle button fill is NOT transparent.
            assert_ne!(
                ui.visuals().widgets.inactive.weak_bg_fill,
                egui::Color32::TRANSPARENT,
                "precondition: default idle fill is a visible chip"
            );

            flatten_chrome_buttons(ui, true); // dark theme → white veil

            let w = &ui.visuals().widgets;
            assert_eq!(
                w.inactive.weak_bg_fill,
                egui::Color32::TRANSPARENT,
                "idle button must be frameless"
            );
            assert_eq!(w.inactive.bg_fill, egui::Color32::TRANSPARENT);
            assert_eq!(w.inactive.bg_stroke, egui::Stroke::NONE);
            // Hover/active carry a subtle, non-transparent, non-opaque veil.
            assert_eq!(w.hovered.weak_bg_fill, egui::Color32::from_white_alpha(20));
            assert_eq!(w.active.weak_bg_fill, egui::Color32::from_white_alpha(32));
            assert!(w.hovered.weak_bg_fill.a() > 0 && w.hovered.weak_bg_fill.a() < 255);
        });
    }

    #[test]
    fn flatten_chrome_buttons_light_theme_uses_black_veil() {
        egui::__run_test_ui(|ui| {
            flatten_chrome_buttons(ui, false); // light theme → black veil
            assert_eq!(
                ui.visuals().widgets.hovered.weak_bg_fill,
                egui::Color32::from_black_alpha(20),
                "light theme hover must be a black veil so it reads on a light bar"
            );
        });
    }
}
