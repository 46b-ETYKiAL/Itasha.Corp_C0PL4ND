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
    // Uniform-"dim" mode dims the WHOLE window at the OS layer (layered-window
    // alpha), so its content is painted OPAQUE — the per-pixel pane alpha must stay
    // 255 or the window would be double-dimmed (per-pixel × OS-layer).
    if config.window_mode.is_uniform_dim() {
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
/// * **Every translucent mode** (`Transparent`/`Glass`/`Mica`/`Vibrancy`): FULLY
///   transparent `[0,0,0,0]` — exactly like the sibling app SCR1B3 (whose
///   `clear_color` is unconditionally `[0,0,0,0]`), which IS see-through on the same
///   hybrid-GPU laptop. The `opacity` slider is folded ONLY into the PANEL fills
///   ([`pane_bg_alpha`]), NOT the clear.
///
///   Why this matters (the transparency bug): eframe issues the clear as the wgpu
///   render-pass `LoadOp::Clear`, so it sets the base framebuffer alpha for EVERY
///   pixel before egui paints. The old code cleared `Transparent` mode to
///   `[theme_bg, opacity]`; egui then painted the panels (also `[theme_bg, opacity]`)
///   ON TOP, so the two alphas COMPOUNDED (`0.6` clear over `0.6` panel ≈ `0.84`)
///   and the RGB darkened — the window read as near-opaque BLACK well before the
///   slider reached 100%. Clearing to `[0,0,0,0]` (SCR1B3-parity) makes the panel
///   alpha the SOLE determinant of see-through, so `opacity` behaves linearly and
///   reaches genuinely-transparent at its low end.
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
        // Uniform "dim": the content is painted OPAQUE (solid theme background) and
        // the OS layered-window alpha dims the whole composited window uniformly —
        // so the clear colour is the theme background at FULL alpha, exactly like
        // Opaque. (A per-pixel-transparent clear here would double-dim.)
        c0pl4nd_core::config::WindowMode::Dim => {
            let (r, g, b) =
                c0pl4nd_core::theme::parse_hex(&theme.background).unwrap_or((0x12, 0x12, 0x12));
            [r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, 1.0]
        }
        // Every per-pixel translucent mode clears FULLY TRANSPARENT (SCR1B3-parity).
        // Native blur backdrops (Glass/Mica/Vibrancy) need it so the OS blur shows
        // through; portable `Transparent` needs it so the opacity slider is not
        // compounded by a dark clear (the "opaque black" bug). The panel fills carry
        // the `opacity` alpha; the terminal text keeps its own alpha over the desktop.
        c0pl4nd_core::config::WindowMode::Transparent
        | c0pl4nd_core::config::WindowMode::Glass
        | c0pl4nd_core::config::WindowMode::Mica
        | c0pl4nd_core::config::WindowMode::Vibrancy => [0.0, 0.0, 0.0, 0.0],
        // Unreachable: effective_translucent() ruled Opaque out above.
        c0pl4nd_core::config::WindowMode::Opaque => [0.0, 0.0, 0.0, 0.0],
    }
}

/// The 0..=255 alpha for a given tint `strength` (0..=1). Scaled so the slider's
/// top end is a clearly-visible wash without fully hiding the background. Pure, so
/// the mapping is unit-testable.
pub(crate) fn tint_alpha(strength: f32) -> u8 {
    (strength.clamp(0.0, 1.0) * 90.0).round() as u8
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
pub(crate) fn paint_background_tint(ctx: &egui::Context, config: &c0pl4nd_core::Config) {
    // Explicit master switch first: when the user has toggled the tint wash OFF,
    // paint nothing even if a colour + strength are still parked in the config.
    if !config.tint_enabled || config.tint_strength <= 0.0 {
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
    opacity: f32,
) {
    let _ = (cc, tint_hex, opacity);
    match mode {
        // Uniform "dim": dim the WHOLE window (chrome + panes + text) by one alpha
        // via the Win32 layered-window attribute — genuinely distinct from the
        // per-pixel `Transparent` mode. Windows-only; elsewhere it degrades to the
        // opaque surface (its `window_clear_color`/`pane_bg_alpha` are already
        // opaque, so nothing shows through off-Windows — an honest no-op).
        c0pl4nd_core::config::WindowMode::Dim => {
            #[cfg(windows)]
            {
                use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
                if let Ok(handle) = cc.window_handle() {
                    if let RawWindowHandle::Win32(h) = handle.as_raw() {
                        dim_imp::set_uniform_dim(h.hwnd.get(), opacity);
                    }
                }
            }
        }
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

/// Quarantined Win32 FFI for the uniform-"dim" transparency mode
/// ([`c0pl4nd_core::config::WindowMode::Dim`]). Isolated behind
/// `#![allow(unsafe_code)]` with `// SAFETY:` justifications, mirroring the other
/// `#[cfg(windows)]` FFI islands (`caption_close::imp`, `job_object`).
#[cfg(windows)]
mod dim_imp {
    #![allow(unsafe_code)]

    use windows::Win32::Foundation::{COLORREF, HWND};
    use windows::Win32::UI::WindowsAndMessaging::{
        GetWindowLongPtrW, SetLayeredWindowAttributes, SetWindowLongPtrW, GWL_EXSTYLE, LWA_ALPHA,
        WS_EX_LAYERED,
    };

    /// Dim the ENTIRE window (chrome + panes + text) by one alpha via the layered-
    /// window attribute — the uniform "dim" mode, genuinely distinct from the
    /// per-pixel `Transparent` mode. `opacity` (0..=1) → alpha, floored at ~5% so
    /// the window can never become fully invisible / unclickable. Ensures
    /// `WS_EX_LAYERED` first (required by `SetLayeredWindowAttributes`). Best-effort;
    /// a null handle is a no-op.
    pub(super) fn set_uniform_dim(hwnd: isize, opacity: f32) {
        if hwnd == 0 {
            return;
        }
        let h = HWND(hwnd as *mut core::ffi::c_void);
        let alpha = (opacity.clamp(0.05, 1.0) * 255.0).round() as u8;
        let layered = i64::from(WS_EX_LAYERED.0) as isize;
        // SAFETY: `hwnd` is this process's live top-level window handle (from
        // eframe's CreationContext); this only reads this window's own extended-
        // style word.
        let ex = unsafe { GetWindowLongPtrW(h, GWL_EXSTYLE) };
        // SAFETY: rewrites this window's own extended-style word to add
        // WS_EX_LAYERED (all other bits preserved) — the prerequisite for
        // SetLayeredWindowAttributes.
        unsafe {
            SetWindowLongPtrW(h, GWL_EXSTYLE, ex | layered);
        }
        // SAFETY: sets this window's whole-window alpha (LWA_ALPHA); the colour key
        // is unused for LWA_ALPHA so COLORREF(0) is inert.
        let _ = unsafe { SetLayeredWindowAttributes(h, COLORREF(0), alpha, LWA_ALPHA) };
    }
}

#[cfg(test)]
mod tint_tests {
    use super::{flatten_chrome_buttons, fold_alpha, tint_alpha};

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
    fn dim_mode_paints_opaque_content_transparent_mode_is_per_pixel() {
        // The two see-through modes must be GENUINELY DISTINCT at the framebuffer:
        // Dim paints OPAQUE content (the OS layered-window alpha dims the whole
        // window uniformly), while Transparent folds the opacity into the per-pixel
        // pane/clear alpha (widgets stay solid, gaps see through). Proven from the
        // pure colour math without a window.
        let mut config = c0pl4nd_core::Config {
            transparency_enabled: true,
            opacity: 0.4,
            ..Default::default()
        };
        let theme = c0pl4nd_core::Theme::builtin_void();

        config.window_mode = c0pl4nd_core::config::WindowMode::Dim;
        assert_eq!(
            super::pane_bg_alpha(&config),
            255,
            "Dim keeps per-pixel panes fully opaque (OS layer does the dimming)"
        );
        assert_eq!(
            super::window_clear_color(&config, &theme)[3],
            1.0,
            "Dim clear colour is fully opaque"
        );

        config.window_mode = c0pl4nd_core::config::WindowMode::Transparent;
        assert!(
            super::pane_bg_alpha(&config) < 255,
            "Transparent folds opacity into the per-pixel pane alpha"
        );
        assert!(
            super::window_clear_color(&config, &theme)[3] < 1.0,
            "Transparent clear colour is per-pixel translucent"
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
