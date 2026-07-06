//! CRT / chromatic-aberration painter effects (research §2).
//!
//! Pure, GPU-free painter approximations extracted from the `egui_app` god-module.
//! The math fns are unit-testable without a GPU; the painter fn draws filled dark
//! bands with a slow vertical drift via `egui::Painter`. ZERO-cost when the caller
//! gates on the setting being off/zero. Re-exported into `egui_app` via `pub(crate) use crt::*`.

// ---- CRT / chromatic-aberration painter effects (research §2) -------------
//
// eframe 0.34 owns the wgpu surface + render loop, so a TRUE fullscreen
// post-process shader over the whole composited UI is infeasible without
// dropping eframe for a raw egui-winit + egui-wgpu host (research §2 verdict).
// These are the STABLE painter-based approximations the research recommends:
// scanlines + vignette drawn over the grid with `egui::Painter`, and a
// per-glyph RGB ghost at the text-draw site. Both are GPU-free and ZERO-cost
// when the setting is off/zero (the caller gates on `crt_scanlines` /
// `chromatic_aberration > 0`).

/// The scanline period in PHYSICAL pixels. A scanline reads as a *line* only
/// when the eye resolves an alternating dark-band / lit-band pattern; on a
/// HiDPI panel a 3-logical-px period collapses sub-physical-px and the GPU
/// antialiases it into a uniform grey film (issue #28). Anchoring the period to
/// PHYSICAL pixels (`PERIOD / ppp` logical points) keeps the band/gap contrast
/// resolvable at any scale factor. ~3 physical px = a believable tube pitch.
pub(crate) const CRT_SCANLINE_PERIOD_PHYS_PX: f32 = 3.0;
/// Fraction of each period painted as the DARK band (the rest is the lit gap).
/// SCR1B3 parity (issue: "the scan lines don't move nice compared to Scribe"):
/// a THIN dark line on a lit gap (≈1/3 duty) reads as distinct lines sweeping
/// down, whereas the old 0.66 (2-dark / 1-lit) painted a mostly-dark field whose
/// drift looked like a shifting shadow-film rather than moving lines. 0.34 = a
/// ~1-px-dark / 2-px-lit feel at a 3-physical-px period — SCR1B3's clean shimmer.
pub(crate) const CRT_SCANLINE_DUTY: f32 = 0.34;
/// The dark-band alpha (0..=255) at the maximum configured darkness (1.0). The
/// effective alpha is `scanline_darkness * THIS` so the config slider tunes
/// trough darkness. The default darkness (0.4) lands at alpha 96 (~38% darken)
/// — the research band that reads as distinct lines, not a flat film (#28); full
/// darkness (1.0) caps at 240 (a near-black trough for a heavy-CRT look).
pub(crate) const CRT_SCANLINE_MAX_DARK_ALPHA: f32 = 240.0;
/// The speed (LOGICAL points / second) at which the whole dark-band field slowly
/// drifts DOWN — a calm CRT shimmer. Modeled on SCR1B3's `effects.rs` motion
/// (~6 pt/s): a gentle sub-period creep, NOT the old bright "rolling scan" bar
/// that swept a white bloom down the pane (it read as a distracting flash). At
/// this speed the pattern moves perceptibly without the flash.
pub(crate) const CRT_SCANLINE_DRIFT_PTS_PER_SEC: f32 = 6.0;

/// The maximum horizontal RGB ghost offset (PHYSICAL pixels) — capped so a wild
/// config value can never smear the text into illegibility.
pub(crate) const CHROMATIC_MAX_OFFSET_PHYS_PX: f32 = 6.0;
/// The minimum visible ghost offset (PHYSICAL pixels) once aberration is ON. The
/// ghost must clear the opaque main glyph's edge to read as RGB separation
/// rather than vanishing under it (issue #28: "does nothing visible"). ≥2
/// physical px is the floor at which the fringe escapes the glyph.
pub(crate) const CHROMATIC_MIN_OFFSET_PHYS_PX: f32 = 2.0;

/// The horizontal RGB ghost offset (LOGICAL points) for a chromatic-aberration
/// `intensity`, resolved against the display's `ppp` (pixels-per-point). The
/// physical-px offset is `(MIN..=MAX) * intensity` clamped, then divided by
/// `ppp` to logical points the painter consumes — so on a 2× HiDPI panel the
/// fringe is still ≥2 PHYSICAL px and visibly clears the glyph (issue #28). The
/// red ghost draws at `-offset`, the blue ghost at `+offset`; `intensity == 0`
/// ⇒ offset `0` (off). Pure → unit-testable without a GPU.
pub(crate) fn chromatic_offset(intensity: f32, ppp: f32) -> f32 {
    if !intensity.is_finite() || intensity <= 0.0 {
        return 0.0;
    }
    let ppp = if ppp.is_finite() && ppp > 0.0 {
        ppp
    } else {
        1.0
    };
    // Physical-px separation scales with intensity from the visible floor to the
    // illegibility cap, so intensity 1.0 ≈ MIN..MAX-spanning fringe.
    let phys = (CHROMATIC_MIN_OFFSET_PHYS_PX
        + (CHROMATIC_MAX_OFFSET_PHYS_PX - CHROMATIC_MIN_OFFSET_PHYS_PX) * intensity.min(1.0))
    .clamp(CHROMATIC_MIN_OFFSET_PHYS_PX, CHROMATIC_MAX_OFFSET_PHYS_PX);
    phys / ppp
}

/// The alpha (0..=255) of each PURE-channel RGB ghost for a chromatic-aberration
/// `intensity`. The ghosts are pure red `(255,0,0)` / pure blue `(0,0,255)`
/// drawn BEHIND the crisp glyph, so only the un-occluded fringe shows as an
/// additive RGB split. Alpha is kept high (the fringe sits behind, never greys
/// the main glyph) and scales with intensity. `intensity == 0` ⇒ alpha `0`.
pub(crate) fn chromatic_ghost_alpha(intensity: f32) -> u8 {
    if !intensity.is_finite() || intensity <= 0.0 {
        return 0;
    }
    // 150 at low intensity scaling to 220 at full — saturated enough to POP as
    // RGB fringing (issue #28: the old 100..=140 tinted galleys washed to grey).
    let t = intensity.clamp(0.0, 1.0);
    (150.0 + 70.0 * t).clamp(0.0, 220.0).round() as u8
}

/// Edge-weight a base chromatic-aberration `offset` (points) by a glyph's
/// horizontal position, so the RGB fringing is stronger toward the screen
/// edges and near-zero at the centre — the authentic lens-style falloff a real
/// CRT shows (research §2(b): "edge-weighted aberration looks more authentic
/// than uniform"). `x` is the glyph's x; `[left, right]` the content span. The
/// normalised distance from centre (0 at centre, 1 at either edge) scales the
/// offset between 40% (centre) and 100% (edge), so the centre still shows a
/// faint fringe (never fully crisp) while the edges separate strongly. Pure →
/// unit-testable.
pub(crate) fn chromatic_edge_weighted_offset(offset: f32, x: f32, left: f32, right: f32) -> f32 {
    let span = right - left;
    if offset <= 0.0 || !span.is_finite() || span <= 0.0 {
        return offset.max(0.0);
    }
    let centre = left + span * 0.5;
    // 0 at centre → 1 at either edge.
    let dist = ((x - centre).abs() / (span * 0.5)).clamp(0.0, 1.0);
    offset * (0.4 + 0.6 * dist)
}

/// The scanline period in LOGICAL points for a display `ppp`. Anchored to
/// [`CRT_SCANLINE_PERIOD_PHYS_PX`] PHYSICAL pixels so the band/gap contrast is
/// resolvable at any scale factor (issue #28: a fixed logical period collapses
/// sub-physical-px on HiDPI and reads as a flat film). Pure → unit-testable.
pub(crate) fn scanline_period_pts(ppp: f32) -> f32 {
    let ppp = if ppp.is_finite() && ppp > 0.0 {
        ppp
    } else {
        1.0
    };
    CRT_SCANLINE_PERIOD_PHYS_PX / ppp
}

/// The number of dark scanline BANDS that fill a content `rect` of the given
/// `height` at the given `ppp`. One band per [`scanline_period_pts`]. Pure +
/// GPU-free so the band geometry is unit-testable without a painter.
pub(crate) fn scanline_count(height: f32, ppp: f32) -> usize {
    if !height.is_finite() || height <= 0.0 {
        return 0;
    }
    (height / scanline_period_pts(ppp)).ceil() as usize
}

/// The dark-band alpha (0..=255) for a configured `darkness` (0..=1). Maps the
/// config slider onto [`CRT_SCANLINE_MAX_DARK_ALPHA`] so the trough darkening is
/// tunable. Pure → unit-testable.
pub(crate) fn scanline_dark_alpha(darkness: f32) -> u8 {
    if !darkness.is_finite() || darkness <= 0.0 {
        return 0;
    }
    (darkness.clamp(0.0, 1.0) * CRT_SCANLINE_MAX_DARK_ALPHA)
        .clamp(0.0, 255.0)
        .round() as u8
}

/// The vertical DRIFT offset (LOGICAL points, in `0..period`) of the scanline
/// field at time `t` seconds. The whole band pattern creeps down at
/// [`CRT_SCANLINE_DRIFT_PTS_PER_SEC`] and wraps every `period`, so the motion is
/// seamless — the pattern at `drift == period` is identical to `drift == 0`.
/// Pure → unit-testable.
pub(crate) fn scanline_drift(period: f32, t: f32) -> f32 {
    if !period.is_finite() || period <= 0.0 {
        return 0.0;
    }
    (t * CRT_SCANLINE_DRIFT_PTS_PER_SEC).rem_euclid(period)
}

/// Paint REAL CRT scan lines across the WHOLE pane content `rect` (issue #28) —
/// filled DARK BANDS (not 1px slivers) at a PHYSICAL-px-anchored period, the
/// whole field drifting slowly DOWN for a calm CRT shimmer (SCR1B3-style; the old
/// bright "rolling scan" bar was removed — it read as a distracting flash). `ppp`
/// resolves the period to logical points; `t` is the animation clock (seconds);
/// `darkness` (0..=1) tunes the trough darkness. GPU-free (filled rects). The
/// caller's `painter_at(rect)` clip keeps every band inside the pane and trims the
/// drift overhang; the caller also requests a repaint each frame so the drift
/// keeps moving.
pub(crate) fn paint_crt_scanlines(
    painter: &egui::Painter,
    rect: egui::Rect,
    ppp: f32,
    t: f32,
    darkness: f32,
) {
    let period = scanline_period_pts(ppp);
    let band_h = period * CRT_SCANLINE_DUTY;
    let dark = egui::Color32::from_black_alpha(scanline_dark_alpha(darkness));
    // The whole band field drifts down by a sub-period offset that wraps every
    // period (seamless). Start one band ABOVE the top (i = -1) so the drift never
    // exposes an ungapped strip at the top edge; the painter clip trims the
    // overhang at the top and bottom.
    let drift = scanline_drift(period, t);
    let lines = scanline_count(rect.height(), ppp);
    for i in -1..lines as i32 {
        let y = rect.top() + i as f32 * period + drift;
        let band = egui::Rect::from_min_max(
            egui::pos2(rect.left(), y),
            egui::pos2(rect.right(), y + band_h),
        );
        painter.rect_filled(band, 0.0, dark);
    }
}
