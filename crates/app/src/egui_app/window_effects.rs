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

/// The 0..=255 alpha for the software frosted-glass wash at a given `amount`
/// (0..=1). Capped at `200/255` (~78%) even at the maximum so the glyph text
/// painted OVER the frost stays legible — a full-opaque frost would hide the
/// terminal. Pure → unit-testable.
pub(crate) fn frost_alpha(amount: f32) -> u8 {
    (amount.clamp(0.0, 1.0) * 200.0).round() as u8
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
/// The wash alpha is [`tint_alpha`] alone — INDEPENDENT of the window opacity, so
/// the tint ACTUALLY WORKS at any opacity (it colours the see-through glass rather
/// than vanishing as the window clears). Opacity is a SEPARATE control (the glass
/// clarity); frost ([`paint_frost`]) is a SEPARATE diffuse wash. A user who wants a
/// fully-clear window turns the tint (and frost) off.
pub(crate) fn paint_background_tint(ctx: &egui::Context, config: &c0pl4nd_core::Config) {
    // Explicit master switch first: when the user has toggled the tint wash OFF,
    // paint nothing even if a colour + strength are still parked in the config.
    if !config.tint_enabled || config.tint_strength <= 0.0 {
        return;
    }
    let alpha = tint_alpha(config.tint_strength);
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

/// Paint the software "frosted glass" wash on the BACKGROUND layer — a deliberate,
/// adjustable diffuse pane over the window, INDEPENDENT of the opacity slider (it
/// works at any opacity, unlike the tint's sibling wash which the opacity used to
/// fade). This is NOT a real desktop blur (impossible on the hybrid-GPU target);
/// it is a self-rendered frosted look: a colour wash at [`frost_alpha`] plus an
/// optional subtle procedural GRAIN texture so it reads as diffused glass rather
/// than a flat film. Painted on the background layer, so it sits BEHIND the glyph
/// text and BELOW the Settings window (both stay crisp/legible). A no-op when the
/// frost master toggle is off or the amount is 0.
///
/// The wash colour is [`c0pl4nd_core::Config::frost_color`] when set to a valid
/// `#RRGGBB`, else the active `theme` background (so the default frost reads as a
/// diffuse pane of the terminal's own colour).
pub(crate) fn paint_frost(
    ctx: &egui::Context,
    config: &c0pl4nd_core::Config,
    theme: &c0pl4nd_core::Theme,
) {
    if !config.frost_enabled || config.frost_amount <= 0.0 {
        return;
    }
    let alpha = frost_alpha(config.frost_amount);
    if alpha == 0 {
        return;
    }
    // Colour: explicit frost_color, else follow the theme background.
    let (r, g, b) = c0pl4nd_core::theme::parse_hex(&config.frost_color)
        .or_else(|_| c0pl4nd_core::theme::parse_hex(&theme.background))
        .unwrap_or((18, 18, 18));
    let rect = ctx.content_rect();
    let painter = ctx.layer_painter(egui::LayerId::background());
    painter.rect_filled(
        rect,
        0.0,
        egui::Color32::from_rgba_unmultiplied(r, g, b, alpha),
    );
    // Optional grain: one cheap tiling value-noise quad over the wash, its alpha
    // scaling with the frost amount, so a stronger frost reads as more diffused.
    if config.frost_grain {
        let grain_alpha = (config.frost_amount.clamp(0.0, 1.0) * 55.0).round() as u8;
        if grain_alpha > 0 {
            let tex = frost_grain_texture(ctx);
            // Tile the NxN noise across the whole rect via a Repeat-wrapped UV that
            // spans (rect / tile) tiles; tint carries the grain alpha (the grayscale
            // texels modulate the luminance, the tint sets the opacity).
            let tile = FROST_GRAIN_TILE as f32;
            let uv = egui::Rect::from_min_max(
                egui::pos2(0.0, 0.0),
                egui::pos2(rect.width() / tile, rect.height() / tile),
            );
            painter.image(
                tex.id(),
                rect,
                uv,
                egui::Color32::from_white_alpha(grain_alpha),
            );
        }
    }
}

/// Side length (texels) of the tiling frost-grain noise texture.
const FROST_GRAIN_TILE: usize = 64;

/// A small tiling value-noise texture for the frost grain, built ONCE and cached
/// in egui memory (a `TextureHandle` is a cheap `Arc`, so cloning it out of the
/// cache each frame is free — the GPU upload happens a single time). Deterministic
/// (a hash of the texel coords), so the grain is stable frame-to-frame and never
/// shimmers. Wrap mode is Repeat so the small tile fills any window size.
fn frost_grain_texture(ctx: &egui::Context) -> egui::TextureHandle {
    let id = egui::Id::new("c0pl4nd-frost-grain-tex");
    if let Some(h) = ctx.data(|d| d.get_temp::<egui::TextureHandle>(id)) {
        return h;
    }
    let n = FROST_GRAIN_TILE;
    let mut pixels = Vec::with_capacity(n * n);
    for y in 0..n {
        for x in 0..n {
            // Cheap deterministic hash → a grayscale value in a NARROW band around
            // mid (≈118..≈168) so the grain is a subtle diffusion, never harsh
            // salt-and-pepper. (`from_gray` makes an opaque grey; the paint tint
            // sets the final overlay alpha.)
            let h = (x as u32)
                .wrapping_mul(374_761_393)
                .wrapping_add((y as u32).wrapping_mul(668_265_263));
            let h = h ^ (h >> 13);
            let v = 118 + (h.wrapping_mul(1_274_126_177) % 50) as u8;
            pixels.push(egui::Color32::from_gray(v));
        }
    }
    let image = egui::ColorImage::new([n, n], pixels);
    let handle = ctx.load_texture(
        "c0pl4nd-frost-grain",
        image,
        egui::TextureOptions {
            magnification: egui::TextureFilter::Linear,
            minification: egui::TextureFilter::Linear,
            wrap_mode: egui::TextureWrapMode::Repeat,
            mipmap_mode: None,
        },
    );
    ctx.data_mut(|d| d.insert_temp(id, handle.clone()));
    handle
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
    use super::{flatten_chrome_buttons, fold_alpha, frost_alpha, tint_alpha};

    #[test]
    fn frost_alpha_scales_and_caps_for_legibility() {
        // 0 amount → no frost; full amount → the 200/255 cap (never fully opaque,
        // so glyph text over the frost stays legible); mid scales linearly; and
        // out-of-range inputs clamp.
        assert_eq!(frost_alpha(0.0), 0, "no frost at amount 0");
        assert_eq!(frost_alpha(-1.0), 0, "negative clamps to 0");
        assert_eq!(
            frost_alpha(1.0),
            200,
            "max frost is capped at ~78% for legibility"
        );
        assert_eq!(frost_alpha(2.0), 200, "above 1.0 clamps to the cap");
        assert_eq!(frost_alpha(0.5), 100, "mid amount scales linearly");
        // Strictly monotonic in the amount (the slider always does something).
        assert!(frost_alpha(0.2) < frost_alpha(0.6) && frost_alpha(0.6) < frost_alpha(0.9));
    }

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
