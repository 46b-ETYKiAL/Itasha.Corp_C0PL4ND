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

#[cfg(test)]
mod tests {
    use super::*;

    /// Every `ppp` these tests sweep: 1× (standard), 1.5× and 2× (HiDPI) — the
    /// scale factors issue #28 was reported against.
    const PPPS: [f32; 3] = [1.0, 1.5, 2.0];

    // ---- chromatic_offset ----

    /// Aberration OFF ⇒ no ghost. A non-finite intensity (a corrupt config) counts
    /// as OFF rather than as "infinitely strong" — it must never smear the text.
    #[test]
    fn chromatic_offset_is_zero_when_the_effect_is_off_or_the_config_is_corrupt() {
        for intensity in [0.0, -0.5, f32::NAN, f32::NEG_INFINITY, f32::INFINITY] {
            assert_eq!(
                chromatic_offset(intensity, 1.0),
                0.0,
                "intensity {intensity} must produce no ghost"
            );
        }
    }

    /// The LOAD-BEARING property of issue #28: once aberration is on, the ghost is
    /// at least `CHROMATIC_MIN_OFFSET_PHYS_PX` PHYSICAL px at ANY scale factor, so
    /// the fringe always clears the opaque glyph instead of vanishing under it.
    /// Asserted in physical px (offset × ppp) — the units the eye sees.
    #[test]
    fn chromatic_offset_always_clears_the_glyph_in_physical_pixels() {
        for ppp in PPPS {
            for intensity in [0.01, 0.25, 0.5, 1.0] {
                let phys = chromatic_offset(intensity, ppp) * ppp;
                assert!(
                    phys >= CHROMATIC_MIN_OFFSET_PHYS_PX - 1e-4,
                    "intensity {intensity} at {ppp}× gave {phys} physical px, \
                     below the {CHROMATIC_MIN_OFFSET_PHYS_PX}px visibility floor"
                );
            }
        }
    }

    /// A wild config value can never smear the text into illegibility: the ghost is
    /// capped at `CHROMATIC_MAX_OFFSET_PHYS_PX` physical px at every scale.
    ///
    /// Only FINITE intensities are swept here: a non-finite one is treated as "off"
    /// (asserted separately), which would satisfy a `<= cap` assertion vacuously.
    #[test]
    fn chromatic_offset_is_capped_however_wild_the_intensity() {
        for ppp in PPPS {
            for intensity in [1.0, 5.0, 1e6] {
                let phys = chromatic_offset(intensity, ppp) * ppp;
                assert!(
                    phys <= CHROMATIC_MAX_OFFSET_PHYS_PX + 1e-4,
                    "intensity {intensity} at {ppp}× gave {phys} physical px, \
                     above the {CHROMATIC_MAX_OFFSET_PHYS_PX}px cap"
                );
                assert!(
                    phys > 0.0,
                    "a positive intensity must still paint a ghost (not a vacuous 0)"
                );
            }
        }
    }

    /// A saturating intensity reaches the cap EXACTLY — the clamp is live, not a
    /// ceiling the function never approaches.
    #[test]
    fn chromatic_offset_reaches_the_cap_at_full_intensity() {
        let phys = chromatic_offset(1.0, 1.0) * 1.0;
        assert!(
            (phys - CHROMATIC_MAX_OFFSET_PHYS_PX).abs() < 1e-4,
            "full intensity must reach the {CHROMATIC_MAX_OFFSET_PHYS_PX}px cap, got {phys}"
        );
    }

    /// The ghost grows with intensity (it is a scale, not a constant) — the control
    /// proving the floor/cap tests above are not satisfied by a fixed value.
    #[test]
    fn chromatic_offset_grows_with_intensity() {
        let low = chromatic_offset(0.1, 1.0);
        let high = chromatic_offset(1.0, 1.0);
        assert!(
            high > low,
            "a stronger aberration must separate further: {low} → {high}"
        );
    }

    /// A nonsense `ppp` (zero / negative / non-finite) falls back to 1× rather than
    /// producing an infinite or NaN offset the painter would smear.
    #[test]
    fn chromatic_offset_falls_back_to_1x_for_a_nonsense_ppp() {
        let expected = chromatic_offset(0.5, 1.0);
        for ppp in [0.0, -2.0, f32::NAN, f32::INFINITY] {
            assert_eq!(
                chromatic_offset(0.5, ppp),
                expected,
                "ppp {ppp} must fall back to 1×"
            );
        }
    }

    // ---- chromatic_ghost_alpha ----

    /// Aberration OFF ⇒ fully transparent ghosts (nothing painted). A non-finite
    /// intensity counts as OFF, matching [`chromatic_offset`] — the two must agree,
    /// or a corrupt config would paint an opaque ghost at a zero offset.
    #[test]
    fn chromatic_ghost_alpha_is_zero_when_the_effect_is_off_or_the_config_is_corrupt() {
        for intensity in [0.0, -1.0, f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            assert_eq!(
                chromatic_ghost_alpha(intensity),
                0,
                "intensity {intensity} must paint no ghost"
            );
        }
    }

    /// The ghosts stay saturated enough to read as RGB fringing (issue #28: the old
    /// 100..=140 alphas washed out to grey), and never exceed the 220 ceiling — for
    /// every FINITE positive intensity, including out-of-range ones.
    #[test]
    fn chromatic_ghost_alpha_stays_in_the_saturated_band() {
        for intensity in [0.01, 0.5, 1.0, 9.0, 1e6] {
            let a = chromatic_ghost_alpha(intensity);
            assert!(
                (150..=220).contains(&a),
                "intensity {intensity} gave alpha {a}, outside the 150..=220 band"
            );
        }
    }

    /// Alpha scales with intensity — the control proving the band test above is not
    /// passing on a constant.
    #[test]
    fn chromatic_ghost_alpha_grows_with_intensity() {
        assert!(chromatic_ghost_alpha(1.0) > chromatic_ghost_alpha(0.1));
    }

    // ---- chromatic_edge_weighted_offset ----

    /// The authentic lens falloff: near-zero fringing at the centre, full at the
    /// edges, and symmetric about the centre. The centre keeps 40% (never fully
    /// crisp) and the edge keeps 100%.
    #[test]
    fn edge_weighting_is_weakest_at_the_centre_and_full_at_the_edges() {
        let (left, right, offset) = (0.0_f32, 100.0_f32, 10.0_f32);

        let centre = chromatic_edge_weighted_offset(offset, 50.0, left, right);
        let at_left = chromatic_edge_weighted_offset(offset, left, left, right);
        let at_right = chromatic_edge_weighted_offset(offset, right, left, right);

        assert!(
            (centre - offset * 0.4).abs() < 1e-4,
            "the centre must keep 40% of the offset, got {centre}"
        );
        assert!(
            (at_left - offset).abs() < 1e-4 && (at_right - offset).abs() < 1e-4,
            "both edges must keep the full offset, got {at_left} / {at_right}"
        );
        assert!(
            (at_left - at_right).abs() < 1e-4,
            "the falloff must be symmetric about the centre"
        );
        assert!(
            centre < at_left,
            "the fringe must be weaker at the centre than at the edge"
        );
    }

    /// A glyph beyond the content span clamps to the edge weight rather than
    /// extrapolating past the full offset.
    #[test]
    fn edge_weighting_clamps_outside_the_span() {
        let full = chromatic_edge_weighted_offset(10.0, 0.0, 0.0, 100.0);
        for x in [-500.0, 600.0] {
            let v = chromatic_edge_weighted_offset(10.0, x, 0.0, 100.0);
            assert!(
                (v - full).abs() < 1e-4,
                "x {x} outside the span must clamp to the edge weight {full}, got {v}"
            );
        }
    }

    /// A degenerate span (zero / inverted / non-finite) cannot divide by zero into a
    /// NaN the painter would consume; the offset passes through, floored at 0.
    #[test]
    fn edge_weighting_survives_a_degenerate_span() {
        for (left, right) in [(0.0_f32, 0.0_f32), (100.0, 0.0), (0.0, f32::NAN)] {
            let v = chromatic_edge_weighted_offset(10.0, 5.0, left, right);
            assert!(v.is_finite(), "span [{left}, {right}] produced {v}");
            assert_eq!(v, 10.0, "a degenerate span passes the offset through");
        }
        assert_eq!(
            chromatic_edge_weighted_offset(-3.0, 5.0, 0.0, 0.0),
            0.0,
            "a negative offset floors at 0 rather than inverting the ghost"
        );
    }

    // ---- scanline geometry ----

    /// The period is anchored to PHYSICAL pixels, so on HiDPI it shrinks in logical
    /// points and the band/gap contrast stays resolvable (issue #28).
    #[test]
    fn scanline_period_is_anchored_to_physical_pixels() {
        for ppp in PPPS {
            let period = scanline_period_pts(ppp);
            assert!(
                (period * ppp - CRT_SCANLINE_PERIOD_PHYS_PX).abs() < 1e-4,
                "at {ppp}× the period must still be {CRT_SCANLINE_PERIOD_PHYS_PX} physical px, got {}",
                period * ppp
            );
        }
    }

    /// A nonsense `ppp` falls back to 1× instead of dividing by zero into infinity.
    #[test]
    fn scanline_period_falls_back_to_1x_for_a_nonsense_ppp() {
        for ppp in [0.0, -1.0, f32::NAN, f32::INFINITY] {
            assert_eq!(scanline_period_pts(ppp), CRT_SCANLINE_PERIOD_PHYS_PX);
        }
    }

    /// A pane with no height (or a non-finite one) paints no bands.
    #[test]
    fn scanline_count_is_zero_for_a_degenerate_height() {
        for height in [0.0, -10.0, f32::NAN] {
            assert_eq!(scanline_count(height, 1.0), 0, "height {height}");
        }
    }

    /// The bands fill the WHOLE pane: the count covers the full height (rounding up
    /// so a partial band at the bottom is still painted), and a taller pane or a
    /// denser (HiDPI) period yields more bands.
    #[test]
    fn scanline_count_covers_the_whole_height() {
        for ppp in PPPS {
            let period = scanline_period_pts(ppp);
            let height = 100.0_f32;
            let n = scanline_count(height, ppp);
            assert!(
                n as f32 * period >= height,
                "{n} bands of {period}pt at {ppp}× leave a gap below {height}pt"
            );
            assert!(
                (n - 1) as f32 * period < height,
                "{n} bands at {ppp}× overshoot {height}pt by more than one band"
            );
        }
        assert!(
            scanline_count(200.0, 1.0) > scanline_count(100.0, 1.0),
            "a taller pane must take more bands"
        );
        assert!(
            scanline_count(100.0, 2.0) > scanline_count(100.0, 1.0),
            "a HiDPI panel packs more (physically-anchored) bands into the same pane"
        );
    }

    // ---- scanline_dark_alpha ----

    /// Darkness 0 ⇒ no darkening at all (the effect reads as off).
    #[test]
    fn scanline_dark_alpha_is_zero_when_darkness_is_off() {
        for darkness in [0.0, -1.0, f32::NAN] {
            assert_eq!(scanline_dark_alpha(darkness), 0);
        }
    }

    /// The config slider maps onto the documented band: the DEFAULT darkness (0.4)
    /// lands at alpha 96 — the "distinct lines, not a flat film" point from issue
    /// #28 — and full darkness caps at the near-black trough.
    #[test]
    fn scanline_dark_alpha_maps_the_slider_onto_the_documented_band() {
        assert_eq!(
            scanline_dark_alpha(0.4),
            96,
            "the default darkness must land on the researched alpha"
        );
        assert_eq!(scanline_dark_alpha(1.0), CRT_SCANLINE_MAX_DARK_ALPHA as u8);
        assert_eq!(
            scanline_dark_alpha(9.0),
            CRT_SCANLINE_MAX_DARK_ALPHA as u8,
            "an out-of-range darkness clamps rather than wrapping"
        );
        assert!(
            scanline_dark_alpha(0.8) > scanline_dark_alpha(0.2),
            "the slider must actually tune the trough"
        );
    }

    // ---- scanline_drift ----

    /// The drift stays inside one period and never runs away with the clock — the
    /// property that makes the creep seamless.
    #[test]
    fn scanline_drift_stays_within_one_period() {
        let period = scanline_period_pts(1.0);
        for t in [0.0, 0.1, 1.0, 60.0, 3600.0] {
            let d = scanline_drift(period, t);
            assert!(
                (0.0..period).contains(&d),
                "t {t}s drifted {d}, outside 0..{period}"
            );
        }
    }

    /// The pattern at a whole-period displacement is identical to zero: the wrap is
    /// seamless, which is what stops the field visibly jumping each cycle.
    #[test]
    fn scanline_drift_wraps_seamlessly_every_period() {
        let period = scanline_period_pts(1.0);
        let one_cycle = period / CRT_SCANLINE_DRIFT_PTS_PER_SEC;
        assert!(
            scanline_drift(period, one_cycle) < 1e-3,
            "a full cycle must land back at 0"
        );
        let mid = scanline_drift(period, one_cycle * 0.5);
        assert!(
            (scanline_drift(period, one_cycle * 1.5) - mid).abs() < 1e-3,
            "the same phase one cycle later must drift identically"
        );
    }

    /// The field actually MOVES (the point of the effect) — the control proving the
    /// wrap tests above are not passing on a constant 0.
    #[test]
    fn scanline_drift_actually_moves_over_time() {
        let period = scanline_period_pts(1.0);
        assert!(
            scanline_drift(period, 0.1) > 0.0,
            "the band field must creep once the clock advances"
        );
    }

    /// A degenerate period cannot produce a NaN offset the painter would consume.
    #[test]
    fn scanline_drift_is_zero_for_a_degenerate_period() {
        for period in [0.0, -3.0, f32::NAN, f32::INFINITY] {
            assert_eq!(scanline_drift(period, 1.0), 0.0, "period {period}");
        }
    }

    // ---- paint_crt_scanlines (observes the REAL shape stream) ----

    /// Collect the rects `paint_crt_scanlines` actually emitted in one frame, read
    /// off egui's real `FullOutput::shapes` — so this measures the geometry the
    /// painter produced, not a test mirror of the maths.
    fn painted_bands(rect: egui::Rect, ppp: f32, t: f32, darkness: f32) -> Vec<egui::Rect> {
        let ctx = egui::Context::default();
        let input = egui::RawInput {
            screen_rect: Some(rect),
            ..Default::default()
        };
        let out = ctx.run_ui(input, |ui| {
            let painter = ui.ctx().layer_painter(egui::LayerId::new(
                egui::Order::Background,
                egui::Id::new("crt-scanline-test"),
            ));
            paint_crt_scanlines(&painter, rect, ppp, t, darkness);
        });
        out.shapes
            .iter()
            .filter_map(|cs| match &cs.shape {
                egui::Shape::Rect(r) => Some(r.rect),
                _ => None,
            })
            .collect()
    }

    /// The painter emits one band per period PLUS the extra band above the top edge
    /// (the `i = -1` start) that stops the drift exposing an ungapped strip, and
    /// every band spans the full pane width at the duty-cycle height.
    #[test]
    fn painting_emits_a_full_width_band_per_period_plus_the_drift_guard() {
        let rect = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(200.0, 100.0));
        let ppp = 1.0;
        let bands = painted_bands(rect, ppp, 0.0, 0.4);

        let expected = scanline_count(rect.height(), ppp) + 1;
        assert_eq!(
            bands.len(),
            expected,
            "expected one band per period plus the top drift guard"
        );

        let period = scanline_period_pts(ppp);
        for band in &bands {
            assert!(
                (band.left() - rect.left()).abs() < 1e-4
                    && (band.right() - rect.right()).abs() < 1e-4,
                "every band must span the full pane width, got {band:?}"
            );
            assert!(
                (band.height() - period * CRT_SCANLINE_DUTY).abs() < 1e-3,
                "every band must be one duty-cycle tall, got {}",
                band.height()
            );
        }
    }

    /// The first band starts ABOVE the pane top, so the drift never exposes an
    /// ungapped strip at the top edge (the painter's clip trims the overhang).
    #[test]
    fn painting_starts_a_band_above_the_top_edge() {
        let rect = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(200.0, 100.0));
        let bands = painted_bands(rect, 1.0, 0.0, 0.4);
        assert!(
            bands.iter().any(|b| b.top() < rect.top()),
            "a band must start above the top edge to cover the drift"
        );
    }

    /// A HiDPI panel packs MORE bands into the same pane — the physical-px anchoring
    /// reaching the painter, not just the maths helper.
    #[test]
    fn painting_packs_more_bands_on_a_hidpi_panel() {
        let rect = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(200.0, 100.0));
        assert!(
            painted_bands(rect, 2.0, 0.0, 0.4).len() > painted_bands(rect, 1.0, 0.0, 0.4).len(),
            "a 2× panel must paint more bands than a 1× panel"
        );
    }

    /// A zero-height pane paints nothing but the drift guard — no panic, no
    /// runaway loop.
    #[test]
    fn painting_a_zero_height_pane_emits_only_the_drift_guard() {
        let rect = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(200.0, 0.0));
        assert_eq!(painted_bands(rect, 1.0, 0.0, 0.4).len(), 1);
    }
}
