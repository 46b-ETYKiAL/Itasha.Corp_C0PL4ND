//! The glyphon paint glue for the egui shell (Milestone 2).
//!
//! This module renders a [`super::pane_term::PaneTerm`]'s grid INSIDE egui's own
//! wgpu render pass via an [`egui_wgpu::CallbackTrait`] (recon dossier §2.1
//! Pattern A). It captures egui's shared `wgpu::Device`/`Queue`/`target_format`
//! (§2.2) and builds the glyphon `TextAtlas`/`Viewport`/per-pane `TextRenderer`
//! on THAT device — never a second device — so there is no duplicate-wgpu
//! problem and the atlas format matches egui's surface (pitfall #1, #2).
//!
//! The per-pane colour runs are produced by `PaneTerm::grid_spans` (which has no
//! glyphon/egui dependency and is headlessly testable); this module is the thin,
//! GPU-only layer that turns those runs into glyphon glyphs. Because the glyphon
//! pass cannot run under kittest's software path, the BUG-PRONE logic
//! (PTY/input/resize/grid-snapshot) lives in `pane_term`, tested headlessly;
//! this layer is verified by the offscreen `screenshot.rs` visual-QA path.

use std::collections::HashMap;

use eframe::egui_wgpu::{self, CallbackTrait};
use eframe::wgpu;
use egui::PaintCallbackInfo;
use glyphon::{
    Attrs, Buffer, Cache, Color as GColor, Family, FontSystem, Metrics, Resolution, Shaping,
    SwashCache, TextArea, TextAtlas, TextBounds, TextRenderer, Viewport,
};

use super::grid::PaneId;
use super::pane_term::{CellMetrics, ColorRun};

/// Padding (physical px) between the pane edge and the first glyph.
const PANE_PAD: f32 = 4.0;

/// Shared glyphon GPU resources, stored in egui-wgpu's `callback_resources`
/// type-map so every per-pane paint callback can reach them. The atlas /
/// viewport / font system / swash cache are SHARED across panes; each pane keeps
/// its own `TextRenderer` + `Buffer` (keyed by [`PaneId`]) so two panes' glyph
/// data don't collide in one renderer.
pub struct TermGpu {
    /// glyphon's GPU cache (pipelines/bind-group layouts), bound to egui's
    /// device. Held to keep the cache alive for the atlas/viewport's lifetime;
    /// not read directly after construction.
    #[allow(dead_code)]
    cache: Cache,
    /// The shared glyph atlas, created with egui's `target_format` (pitfall #2).
    atlas: TextAtlas,
    /// The shared viewport (screen resolution), updated each frame.
    viewport: Viewport,
    /// CPU font shaping/layout state.
    font_system: FontSystem,
    /// Rasterised-glyph cache.
    swash_cache: SwashCache,
    /// Per-pane text renderer (one prepare/render unit per pane per frame).
    renderers: HashMap<PaneId, TextRenderer>,
    /// Per-pane laid-out glyph buffer, rebuilt from the pane's colour runs.
    buffers: HashMap<PaneId, Buffer>,
    /// The base monospace metrics derived once at construction.
    metrics: Metrics,
}

impl TermGpu {
    /// Build the shared glyphon resources on egui's wgpu device/queue/format.
    /// `font_px` / `line_px` are the base monospace cell metrics.
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        target_format: wgpu::TextureFormat,
        font_px: f32,
        line_px: f32,
    ) -> Self {
        let cache = Cache::new(device);
        let viewport = Viewport::new(device, &cache);
        let atlas = TextAtlas::new(device, queue, &cache, target_format);
        Self {
            cache,
            atlas,
            viewport,
            font_system: FontSystem::new(),
            swash_cache: SwashCache::new(),
            renderers: HashMap::new(),
            buffers: HashMap::new(),
            metrics: Metrics::new(font_px, line_px),
        }
    }

    /// The cell metrics (physical px) implied by the current font, measured from
    /// a shaped reference glyph so column math matches the rendered advance.
    pub fn cell_metrics(&mut self) -> CellMetrics {
        // Measure a monospace 'M' advance to get the true cell width; fall back
        // to a font-size heuristic if shaping yields nothing.
        let mut probe = Buffer::new(&mut self.font_system, self.metrics);
        probe.set_text(
            &mut self.font_system,
            "M",
            &Attrs::new().family(Family::Monospace),
            Shaping::Advanced,
            None,
        );
        probe.shape_until_scroll(&mut self.font_system, false);
        let mut advance = self.metrics.font_size * 0.6;
        for run in probe.layout_runs() {
            for g in run.glyphs.iter() {
                advance = advance.max(g.w);
            }
        }
        CellMetrics {
            advance_w: advance.max(1.0),
            line_h: self.metrics.line_height.max(1.0),
        }
    }

    /// Drop the GPU buffers/renderers for panes that no longer exist, so closed
    /// panes don't leak glyph buffers.
    pub fn retain_panes(&mut self, live: &[PaneId]) {
        self.buffers.retain(|id, _| live.contains(id));
        self.renderers.retain(|id, _| live.contains(id));
    }

    /// Build a pane's glyphon `Buffer` from its colour runs, update the shared
    /// viewport to the full screen, and `prepare` the pane's `TextRenderer` so
    /// the glyphs are ready to draw. This is the SHARED draw-build code: it is
    /// called by BOTH the live egui [`TermPaint::prepare`] callback AND the
    /// offscreen pixel-readback test, so the test exercises the exact production
    /// glyph build/shape/prepare path (recon dossier §7 visual-QA + the
    /// black-pane regression guard). `screen_px` is the FULL surface size in
    /// physical pixels — glyphon maps glyph positions against it, so it MUST be
    /// the real surface size, not 0×0.
    ///
    /// Returns `Ok(())` once the renderer is prepared; surfaces the glyphon error
    /// otherwise (never silently no-ops).
    #[allow(clippy::too_many_arguments)]
    pub fn prepare_pane(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        pane_id: PaneId,
        px_rect: [f32; 4],
        default_fg: [u8; 3],
        runs: &[ColorRun],
        screen_px: [u32; 2],
    ) -> Result<(), glyphon::PrepareError> {
        self.viewport.update(
            queue,
            Resolution {
                width: screen_px[0],
                height: screen_px[1],
            },
        );

        let [left, top, w, h] = px_rect;
        let metrics = self.metrics;
        let default_attrs = Attrs::new().family(Family::Monospace).color(GColor::rgb(
            default_fg[0],
            default_fg[1],
            default_fg[2],
        ));

        // Borrow the font system + buffers disjointly to (re)build this pane's
        // laid-out glyph buffer from the colour runs.
        let TermGpu {
            font_system,
            buffers,
            ..
        } = self;
        let buf = buffers
            .entry(pane_id)
            .or_insert_with(|| Buffer::new(font_system, metrics));
        buf.set_size(
            font_system,
            Some((w - 2.0 * PANE_PAD).max(1.0)),
            Some((h - 2.0 * PANE_PAD).max(1.0)),
        );
        buf.set_rich_text(
            font_system,
            runs.iter().map(|(s, (r, g, b))| {
                (
                    s.as_str(),
                    Attrs::new()
                        .family(Family::Monospace)
                        .color(GColor::rgb(*r, *g, *b)),
                )
            }),
            &default_attrs,
            Shaping::Advanced,
            None,
        );
        buf.shape_until_scroll(font_system, false);

        // Clip glyphs to the pane's physical rect (pitfall #7) via glyphon's own
        // TextBounds (the egui callback ALSO sets a wgpu scissor for the live
        // path; the offscreen test relies on these bounds).
        let bounds = TextBounds {
            left: left as i32,
            top: top as i32,
            right: (left + w) as i32,
            bottom: (top + h) as i32,
        };

        let TermGpu {
            atlas,
            viewport,
            font_system,
            swash_cache,
            renderers,
            buffers,
            ..
        } = self;
        let renderer = renderers.entry(pane_id).or_insert_with(|| {
            TextRenderer::new(atlas, device, wgpu::MultisampleState::default(), None)
        });
        let buffer = buffers
            .get(&pane_id)
            .expect("buffer was just inserted above");
        let areas = [TextArea {
            buffer,
            left: left + PANE_PAD,
            top: top + PANE_PAD,
            scale: 1.0,
            bounds,
            default_color: GColor::rgb(default_fg[0], default_fg[1], default_fg[2]),
            custom_glyphs: &[],
        }];
        renderer.prepare(
            device,
            queue,
            font_system,
            atlas,
            viewport,
            areas,
            swash_cache,
        )
    }

    /// Render a previously-[`prepare_pane`]d pane into `pass`. Shared by the live
    /// egui [`TermPaint::paint`] callback (after it restores the full-screen
    /// viewport) and the offscreen pixel-readback test.
    pub fn render_pane(
        &self,
        pane_id: PaneId,
        pass: &mut wgpu::RenderPass<'_>,
    ) -> Result<(), glyphon::RenderError> {
        let Some(renderer) = self.renderers.get(&pane_id) else {
            return Ok(());
        };
        renderer.render(&self.atlas, &self.viewport, pass)
    }
}

/// Per-pane, per-frame paint payload: the pane's id, its physical-pixel rect,
/// and the colour runs to render. Built by the host BEFORE paint (the grid
/// snapshot already happened on the CPU), so `prepare`/`paint` only touch GPU.
pub struct TermPaint {
    /// Which pane this callback paints.
    pub pane_id: PaneId,
    /// The pane body's physical-pixel rect within the egui surface
    /// `(left, top, width, height)` — used for the glyph origin and the
    /// `TextBounds` clip (pitfall #7).
    pub px_rect: [f32; 4],
    /// The default foreground colour (theme fg) for glyphs with no explicit run.
    pub default_fg: [u8; 3],
    /// The colour runs produced by `PaneTerm::grid_spans` this frame.
    pub runs: std::sync::Arc<Vec<ColorRun>>,
}

impl CallbackTrait for TermPaint {
    fn prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        screen: &egui_wgpu::ScreenDescriptor,
        _encoder: &mut wgpu::CommandEncoder,
        resources: &mut egui_wgpu::CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        let Some(gpu) = resources.get_mut::<TermGpu>() else {
            return Vec::new();
        };
        // Delegate to the SHARED build/shape/prepare path so the live callback
        // and the offscreen pixel-readback test exercise the exact same draw
        // code (the regression guard for the black-pane bug).
        if let Err(e) = gpu.prepare_pane(
            device,
            queue,
            self.pane_id,
            self.px_rect,
            self.default_fg,
            &self.runs,
            screen.size_in_pixels,
        ) {
            tracing::warn!("glyphon prepare failed for pane {:?}: {e}", self.pane_id);
        }
        Vec::new()
    }

    fn paint(
        &self,
        info: PaintCallbackInfo,
        pass: &mut wgpu::RenderPass<'static>,
        resources: &egui_wgpu::CallbackResources,
    ) {
        let Some(gpu) = resources.get::<TermGpu>() else {
            return;
        };
        // ROOT CAUSE of the black-pane bug (Milestone 2): before invoking a paint
        // callback, egui-wgpu calls `render_pass.set_viewport(rect)` to the
        // callback's pane rect (egui-wgpu-0.34 renderer.rs §"default viewport for
        // the render pass"). glyphon's vertex shader maps each glyph's ABSOLUTE
        // pixel position into NDC against the FULL window resolution
        // (`2*pos/screen_resolution-1`). With egui's sub-rect viewport active,
        // that full-screen NDC is then re-mapped into the tiny pane sub-rect —
        // offsetting + squishing every glyph far outside the pane's scissor, so
        // NOTHING is visible (pure black). The fix: restore the FULL-screen
        // viewport here so glyphon's absolute coordinates map correctly. The
        // scissor (set by egui-wgpu from the callback's clip_rect) STILL clips to
        // the pane rect, so glyphs never bleed outside the pane.
        let [sw, sh] = info.screen_size_px;
        pass.set_viewport(0.0, 0.0, sw as f32, sh as f32, 0.0, 1.0);
        if let Err(e) = gpu.render_pane(self.pane_id, pass) {
            tracing::warn!("glyphon render failed for pane {:?}: {e}", self.pane_id);
        }
    }
}

#[cfg(test)]
mod tests {
    // `TermGpu`/`TermPaint` require a live wgpu device + egui render pass, which
    // kittest's software path does not provide (recon dossier §7) — they are
    // exercised by the offscreen `screenshot.rs` visual-QA path. The headless,
    // bug-prone logic (grid snapshot, input, resize) is tested in `pane_term`
    // and the `egui_terminal` interaction-test binary. This block asserts only
    // the pure, no-GPU shape: that a `TermPaint` payload carries the pane's
    // physical rect through to the glyph origin/bounds the GPU layer consumes.
    use super::{PaneId, TermPaint, PANE_PAD};

    #[test]
    fn term_paint_preserves_pane_geometry() {
        let paint = TermPaint {
            pane_id: PaneId(7),
            px_rect: [100.0, 50.0, 640.0, 480.0],
            default_fg: [10, 20, 30],
            runs: std::sync::Arc::new(vec![("hi".to_string(), (1, 2, 3))]),
        };
        // The pane rect is carried verbatim for the GPU layer to clip against.
        assert_eq!(paint.px_rect, [100.0, 50.0, 640.0, 480.0]);
        assert_eq!(paint.pane_id, PaneId(7));
        // The glyph origin sits inside the rect by PANE_PAD on each axis, so the
        // padded inset never escapes the pane's left/top edge.
        let glyph_left = paint.px_rect[0] + PANE_PAD;
        let glyph_top = paint.px_rect[1] + PANE_PAD;
        assert!(glyph_left > paint.px_rect[0]);
        assert!(glyph_top > paint.px_rect[1]);
        assert!(
            PANE_PAD < paint.px_rect[2],
            "pad must not consume the width"
        );
    }
}
