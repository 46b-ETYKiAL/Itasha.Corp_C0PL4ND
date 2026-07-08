//! Whole-window motion overlays (SCR1B3 parity): flicker, VHS-tracking,
//! wired node-mesh ambient background, cursor ghost-trail, and the one-shot
//! boot-glitch sweep.
//!
//! Ported from SCR1B3's `app/effects.rs`. Each is a pure `ctx`-layer painter —
//! no wgpu shader, GPU-free filled rects/lines, and ZERO-cost when the caller
//! gates on the effect being off (the master `animations_enabled` switch plus
//! each per-effect toggle). All motion is deterministic in `t` (seconds) so the
//! reduced-motion resting frame is stable. Re-exported into `egui_app` via
//! `pub(crate) use motion_fx::*`.
//!
//! Unlike the per-pane CRT scanlines in [`crate::egui_app::crt`] (drawn inside
//! each pane's grid painter), these wash the WHOLE window and so are painted
//! once per frame at the `Context` layer's `Order::Middle`: ABOVE the panes +
//! chrome (which render on `Order::Background`, whose opaque fills would fully
//! occlude a Background-order overlay) yet strictly BELOW egui popups, menus,
//! color-pickers, and tooltips (`Order::Foreground`/`Order::Tooltip`). That last
//! part is load-bearing — at `Order::Foreground` the mesh painted OVER the tint
//! colour-picker popup and obscured the swatch grid; `Order::Middle` guarantees
//! every effect sits under any open popup. (egui Windows are also `Order::Middle`,
//! so the Settings/palette/paste panels are kept clean via the `exclude` rect.)
//!
//! ## Live-preview exclude rect
//! Each painter takes an `exclude: Option<Rect>` — the bounding rect of any open
//! centered chrome panel (Settings window, command palette, paste confirm). The
//! effect is painted everywhere EXCEPT that rect, so a Motion setting previews
//! live on the terminal WHILE the Settings panel stays clean (the reported "the
//! mesh overlays the settings menu" is fixed without suppressing the preview the
//! user needs to see what they are tuning). `None` paints the whole window.

use egui::{Color32, Context, Id, LayerId, Order, Pos2, Rect, Stroke};

/// The `Context`-layer order EVERY whole-window motion overlay paints at. It is
/// `Order::Middle`: above the panes + chrome (`Order::Background`, whose opaque
/// fills would occlude a Background overlay) yet strictly BELOW egui popups,
/// menus, color-pickers, and tooltips (`Order::Foreground`/`Order::Tooltip`). The
/// single source of truth so all five painters move together and the
/// "below-popups" invariant is unit-testable (see the tests). Load-bearing: at
/// `Order::Foreground` the mesh painted OVER the tint colour-picker popup.
pub(crate) const EFFECT_LAYER_ORDER: Order = Order::Middle;

/// Fill `outer` with `color`, but leave the `exclude` rectangle unpainted so a
/// full-window wash never washes over an open centered panel. `None` (or a
/// non-overlapping exclude) paints `outer` in one rect; otherwise the remainder
/// is split into up to four surrounding bands (top / bottom / left / right).
fn fill_around(
    painter: &egui::Painter,
    outer: Rect,
    exclude: Option<Rect>,
    rounding: f32,
    color: Color32,
) {
    let hole = match exclude {
        Some(h) => h.intersect(outer),
        None => {
            painter.rect_filled(outer, rounding, color);
            return;
        }
    };
    if !hole.is_positive() {
        painter.rect_filled(outer, rounding, color);
        return;
    }
    // Top band — full width, above the hole.
    if hole.top() > outer.top() {
        painter.rect_filled(
            Rect::from_min_max(outer.left_top(), Pos2::new(outer.right(), hole.top())),
            rounding,
            color,
        );
    }
    // Bottom band — full width, below the hole.
    if hole.bottom() < outer.bottom() {
        painter.rect_filled(
            Rect::from_min_max(Pos2::new(outer.left(), hole.bottom()), outer.right_bottom()),
            rounding,
            color,
        );
    }
    // Left band — beside the hole, vertically clamped to the hole's span.
    let band_top = hole.top().max(outer.top());
    let band_bottom = hole.bottom().min(outer.bottom());
    if hole.left() > outer.left() {
        painter.rect_filled(
            Rect::from_min_max(
                Pos2::new(outer.left(), band_top),
                Pos2::new(hole.left(), band_bottom),
            ),
            rounding,
            color,
        );
    }
    // Right band.
    if hole.right() < outer.right() {
        painter.rect_filled(
            Rect::from_min_max(
                Pos2::new(hole.right(), band_top),
                Pos2::new(outer.right(), band_bottom),
            ),
            rounding,
            color,
        );
    }
}

/// Subtle full-window brightness flicker (CRT-style). A translucent black wash
/// whose alpha wanders via layered sines of `t` (deterministic — no RNG, so the
/// reduced-motion resting frame is stable). `strength` (0..=1) scales the wash;
/// even at 1.0 the alpha peaks near 18/255 (~7%) — a photosensitivity-comfort
/// ceiling, well short of a full-black strobe. `Order::Middle` so it modulates the
/// whole composited view; `exclude` keeps an open panel clean.
pub(crate) fn paint_flicker(ctx: &Context, strength: f32, t: f64, exclude: Option<Rect>) {
    let s = strength.clamp(0.0, 1.0);
    if s <= 0.0 {
        return;
    }
    let n = ((t * 17.0).sin() * 0.5 + (t * 53.0).sin() * 0.3 + (t * 97.0).sin() * 0.2).abs();
    let a = (s * n as f32 * 18.0).round().clamp(0.0, 255.0) as u8;
    if a == 0 {
        return;
    }
    let painter = ctx.layer_painter(LayerId::new(EFFECT_LAYER_ORDER, Id::new("motion-flicker")));
    fill_around(
        &painter,
        ctx.content_rect(),
        exclude,
        0.0,
        Color32::from_rgba_unmultiplied(0, 0, 0, a),
    );
}

/// VHS-style tracking lines: faint bright horizontal bands sweeping down the
/// window at two different speeds, like analogue tape tracking error. `intensity`
/// (0..=1, default 0.5) scales how bright the bands read. `exclude` keeps an open
/// panel clean.
pub(crate) fn paint_vhs_tracking(ctx: &Context, t: f64, intensity: f32, exclude: Option<Rect>) {
    let rect = ctx.content_rect();
    if rect.height() < 1.0 {
        return;
    }
    // The base alphas below (9 / 7) are the shipped look at the DEFAULT intensity
    // (0.5), so `k = intensity * 2` keeps a just-enabled VHS effect identical to
    // the old feel while letting the Motion → VHS-intensity slider dim it toward
    // nothing or brighten it to a bold, unmistakable band.
    let k = intensity.clamp(0.0, 1.0) * 2.0;
    if k <= 0.0 {
        return;
    }
    let a_main = (9.0 * k).round().clamp(0.0, 255.0) as u8;
    let a_core = (7.0 * k).round().clamp(0.0, 255.0) as u8;
    let painter = ctx.layer_painter(LayerId::new(
        EFFECT_LAYER_ORDER,
        Id::new("motion-vhs-tracking"),
    ));
    for (i, speed) in [(0u32, 0.13f64), (1, 0.071)].iter() {
        let phase = (t * speed + *i as f64 * 0.5).rem_euclid(1.0) as f32;
        let y = rect.top() + phase * rect.height();
        let band_h = 16.0;
        fill_around(
            &painter,
            Rect::from_min_max(
                Pos2::new(rect.left(), y),
                Pos2::new(rect.right(), y + band_h),
            ),
            exclude,
            0.0,
            Color32::from_rgba_unmultiplied(255, 255, 255, a_main),
        );
        fill_around(
            &painter,
            Rect::from_min_max(
                Pos2::new(rect.left(), y + band_h * 0.4),
                Pos2::new(rect.right(), y + band_h * 0.6),
            ),
            exclude,
            0.0,
            Color32::from_rgba_unmultiplied(255, 255, 255, a_core),
        );
    }
}

/// Animated wired node-mesh ambient background (Lain "Wired" feel). `density`
/// (0..=2) drives the node count; nodes drift slowly and near neighbours are
/// linked with faint accent lines. `brightness` (0..=3, 1.0 = shipped) scales the
/// link + dot opacity so the lattice can be dimmed toward invisible or brightened
/// to clearly pop — the "mesh is too dim to notice" report. O(n²) over the capped
/// node count — bounded per frame.
///
/// Painted at `Order::Middle` (a faint over-everything veil) rather than
/// `Order::Background`: C0PL4ND's terminal panes paint OPAQUE fills when window
/// transparency is off, which fully occluded a Background-order mesh — the
/// "enabling the mesh does nothing" report. A restrained foreground alpha keeps
/// the wired lattice visible on every pane background while staying well under
/// the text so legibility holds. The node dots + links use the theme accent so
/// the density slider is perceptible (more nodes ⇒ visibly more lattice).
/// `exclude` (an open panel's rect) drops any node/link/dot that would fall over
/// the panel so the mesh previews on the terminal without washing the panel.
pub(crate) fn paint_wired_mesh(
    ctx: &Context,
    density: f32,
    brightness: f32,
    color: Color32,
    t: f64,
    exclude: Option<Rect>,
) {
    let rect = ctx.content_rect();
    if rect.width() < 1.0 || rect.height() < 1.0 {
        return;
    }
    let painter = ctx.layer_painter(LayerId::new(
        EFFECT_LAYER_ORDER,
        Id::new("motion-wired-mesh"),
    ));
    let d = density.clamp(0.0, 2.0);
    // AREA-AWARE node count. A fixed 8..64 count is invisibly sparse on a large
    // (e.g. 4K) display — the nodes scatter far past the link radius and read as a
    // few stray specks with no connecting web ("enabling the mesh does nothing").
    // Scaling the count with the window area keeps the lattice a legible connected
    // web at any size; density interpolates from a calm field to a busy one. Capped
    // at 160 so the O(n²) neighbour pass stays bounded per frame.
    let area_cap = (rect.width() * rect.height() / 26_000.0).clamp(24.0, 160.0);
    let n = (12.0 + d * (area_cap - 12.0)).max(12.0) as usize;
    let mut pts: Vec<Pos2> = Vec::with_capacity(n);
    for i in 0..n {
        let fi = i as f64;
        let bx = (fi * 0.732).fract() as f32;
        let by = (fi * 0.387 + 0.13).fract() as f32;
        let dx = ((t * 0.07 + fi * 1.3).sin() * 0.5 + 0.5) as f32;
        let dy = ((t * 0.05 + fi * 0.7).cos() * 0.5 + 0.5) as f32;
        let x = rect.left() + (bx * 0.9 + dx * 0.1) * rect.width();
        let y = rect.top() + (by * 0.9 + dy * 0.1) * rect.height();
        pts.push(Pos2::new(x, y));
    }
    // Bolder than the old Background values (16/40) so the web clearly reads over an
    // opaque pane at Foreground order — thin bright lines + visible node dots — yet
    // stays translucent enough not to fight the terminal text. `brightness` scales
    // the base alpha (1.0 = shipped) so the Motion → Mesh-brightness slider can dim
    // the lattice toward invisible or brighten it to clearly pop.
    let b = brightness.clamp(0.0, 3.0);
    let scale_a = |base: f32| (base * b).round().clamp(0.0, 255.0) as u8;
    let link = Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), scale_a(42.0));
    let dot = Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), scale_a(120.0));
    // A fully-dimmed mesh has nothing to paint — skip the O(n²) pass entirely.
    if link.a() == 0 && dot.a() == 0 {
        return;
    }
    // Drop any node that falls inside an open panel's rect so the mesh previews on
    // the terminal without washing over the panel. `contains` is cheap; skipping a
    // node also skips every link touching it (both endpoints are tested below).
    // `!is_some_and(…)` (not `is_none_or`, which is Rust 1.82+, nor `map_or(true,…)`,
    // which clippy's `unnecessary_map_or` rejects) keeps the crate's 1.80 MSRV: no
    // exclude ⇒ nothing is "some and inside" ⇒ every node is outside.
    let outside = |p: Pos2| !exclude.is_some_and(|e| e.contains(p));
    // The link radius tracks the ACTUAL mean node spacing (≥ a screen-fraction
    // floor), so neighbours reliably connect at any window size — otherwise, on a
    // large screen, no pair falls within a fixed radius and the "mesh" collapses to
    // unconnected dots. 1.5× the mean spacing gives each node several links → a web.
    let mean_spacing = (rect.width() * rect.height() / n.max(1) as f32).sqrt();
    let max_d = (rect.width().min(rect.height()) * 0.16).max(mean_spacing * 1.5);
    for i in 0..n {
        if !outside(pts[i]) {
            continue;
        }
        for j in (i + 1)..n {
            if outside(pts[j]) && pts[i].distance(pts[j]) < max_d {
                painter.line_segment([pts[i], pts[j]], Stroke::new(1.0, link));
            }
        }
        painter.circle_filled(pts[i], 2.2, dot);
    }
}

/// The lifetime (seconds) of a single cursor-trail echo for a given `intensity`
/// (0..=1). A higher intensity lets each echo linger longer, so the trail reads
/// as a longer comet tail. Shared by the painter (fade math) AND the caller's
/// deque-prune so the two never disagree about when an echo is dead. Pure →
/// unit-testable, and the single source of truth for the trail's temporal span.
pub(crate) fn cursor_trail_life(intensity: f32) -> f64 {
    // 0.35s at zero intensity (a short flick) .. 1.35s at max (a long comet
    // tail). The default config intensity (0.6) lands at ~0.65s.
    (0.35 + 0.5 * intensity.clamp(0.0, 2.0)) as f64
}

/// Cursor ghost-trail: fading echoes of recent focused-cursor cell rectangles.
/// The caller feeds `trail` (rect + birth-time) as the terminal cursor moves;
/// `intensity` (0..=1) scales BOTH the echo opacity and its lifetime (via
/// [`cursor_trail_life`]) so the Motion → Cursor-trail-intensity slider tunes the
/// trail from a faint flick to a bold comet tail. `Order::Middle` so the
/// echoes sit over the grid like the live cursor.
pub(crate) fn paint_cursor_trail(
    ctx: &Context,
    trail: &std::collections::VecDeque<(Rect, f64)>,
    color: Color32,
    now: f64,
    intensity: f32,
    exclude: Option<Rect>,
) {
    if trail.is_empty() {
        return;
    }
    let life = cursor_trail_life(intensity);
    // Peak echo alpha scales from 110 (faint) up to 255 (bold) with intensity, so
    // a pronounced trail is unmistakable while a low setting stays subtle. This is
    // well above the old fixed 90 — the "trail is barely visible" report.
    let peak = 110.0 + 100.0 * intensity.clamp(0.0, 2.0);
    let painter = ctx.layer_painter(LayerId::new(
        EFFECT_LAYER_ORDER,
        Id::new("motion-cursor-trail"),
    ));
    for (rect, born) in trail.iter() {
        // Skip an echo that would fall over an open panel (live-preview exclude).
        if exclude.is_some_and(|e| e.intersects(*rect)) {
            continue;
        }
        let age = (now - born).clamp(0.0, life);
        let f = 1.0 - (age / life) as f32;
        if f <= 0.0 {
            continue;
        }
        let a = (f * peak).clamp(0.0, 255.0) as u8;
        painter.rect_filled(
            *rect,
            1.0,
            Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), a),
        );
    }
}

/// One-shot boot "glitch" sweep over the first ~0.55s after launch: a bright
/// scan line descends while a few dark offset bands flicker, all fading out.
/// `elapsed` is seconds since the first frame; outside `[0, DUR]` it no-ops.
pub(crate) fn paint_boot_glitch(ctx: &Context, elapsed: f64) {
    const DUR: f64 = 0.55;
    if !(0.0..=DUR).contains(&elapsed) {
        return;
    }
    let rect = ctx.content_rect();
    if rect.width() < 160.0 {
        return; // first-frame 0-width content_rect guard
    }
    let painter = ctx.layer_painter(LayerId::new(
        EFFECT_LAYER_ORDER,
        Id::new("motion-boot-glitch"),
    ));
    let p = (elapsed / DUR) as f32;
    let fade = 1.0 - p;
    let y = rect.top() + p * rect.height();
    painter.rect_filled(
        Rect::from_min_max(
            Pos2::new(rect.left(), y - 2.0),
            Pos2::new(rect.right(), y + 2.0),
        ),
        0.0,
        Color32::from_rgba_unmultiplied(255, 255, 255, (fade * 120.0) as u8),
    );
    for i in 0..3u32 {
        let fi = i as f32;
        let gy = rect.top() + ((p * 2.0 + fi * 0.27).fract()) * rect.height();
        let gh = 6.0 + fi * 4.0;
        painter.rect_filled(
            Rect::from_min_max(Pos2::new(rect.left(), gy), Pos2::new(rect.right(), gy + gh)),
            0.0,
            Color32::from_rgba_unmultiplied(0, 0, 0, (fade * 60.0) as u8),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::EFFECT_LAYER_ORDER;
    use egui::Order;

    #[test]
    fn effect_layer_order_is_below_popups_and_above_panes() {
        // FIX B invariant: every whole-window motion overlay paints STRICTLY BELOW
        // egui popups / menus / color-pickers (`Order::Foreground`) and tooltips
        // (`Order::Tooltip`), so an open tint colour-picker is never obscured by the
        // mesh — while staying ABOVE the panes + chrome (`Order::Background`) so the
        // effects remain visible over the terminal. If someone moves the effects
        // back to `Order::Foreground`, this fails.
        assert!(
            EFFECT_LAYER_ORDER < Order::Foreground,
            "effects must render below popups/color-pickers (got {EFFECT_LAYER_ORDER:?})"
        );
        assert!(
            EFFECT_LAYER_ORDER < Order::Tooltip,
            "effects must render below tooltips"
        );
        assert!(
            EFFECT_LAYER_ORDER > Order::Background,
            "effects must render above the panes/chrome so they stay visible"
        );
    }
}
