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
    /// glyphon's GPU cache (pipelines/bind-group layouts), bound to egui's device.
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

impl TermPaint {
    /// (Re)build this pane's glyphon `Buffer` from the colour runs and lay it
    /// out to the pane's pixel size. Shared so `prepare` stays small.
    fn rebuild_buffer(&self, gpu: &mut TermGpu) {
        let [_, _, w, h] = self.px_rect;
        let metrics = gpu.metrics;
        let default = self.default_fg;
        let default_attrs = Attrs::new()
            .family(Family::Monospace)
            .color(GColor::rgb(default[0], default[1], default[2]));
        // Borrow the font system disjointly from the buffer entry.
        let TermGpu {
            font_system,
            buffers,
            ..
        } = gpu;
        let buf = buffers
            .entry(self.pane_id)
            .or_insert_with(|| Buffer::new(font_system, metrics));
        buf.set_size(
            font_system,
            Some((w - 2.0 * PANE_PAD).max(1.0)),
            Some((h - 2.0 * PANE_PAD).max(1.0)),
        );
        buf.set_rich_text(
            font_system,
            self.runs.iter().map(|(s, (r, g, b))| {
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
    }
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
        gpu.viewport.update(
            queue,
            Resolution {
                width: screen.size_in_pixels[0],
                height: screen.size_in_pixels[1],
            },
        );
        self.rebuild_buffer(gpu);

        let [left, top, w, h] = self.px_rect;
        // Clip the glyphs to the pane's physical rect (pitfall #7) — both via
        // egui's callback scissor AND glyphon's own TextBounds, belt-and-braces.
        let bounds = TextBounds {
            left: left as i32,
            top: top as i32,
            right: (left + w) as i32,
            bottom: (top + h) as i32,
        };
        let default = self.default_fg;
        // Build the renderer + buffer borrows disjointly.
        let TermGpu {
            atlas,
            viewport,
            font_system,
            swash_cache,
            renderers,
            buffers,
            cache,
            ..
        } = gpu;
        let renderer = renderers.entry(self.pane_id).or_insert_with(|| {
            TextRenderer::new(atlas, device, wgpu::MultisampleState::default(), None)
        });
        let Some(buffer) = buffers.get(&self.pane_id) else {
            let _ = cache;
            return Vec::new();
        };
        let areas = [TextArea {
            buffer,
            left: left + PANE_PAD,
            top: top + PANE_PAD,
            scale: 1.0,
            bounds,
            default_color: GColor::rgb(default[0], default[1], default[2]),
            custom_glyphs: &[],
        }];
        if let Err(e) = renderer.prepare(
            device,
            queue,
            font_system,
            atlas,
            viewport,
            areas,
            swash_cache,
        ) {
            tracing::warn!("glyphon prepare failed for pane {:?}: {e}", self.pane_id);
        }
        Vec::new()
    }

    fn paint(
        &self,
        _info: PaintCallbackInfo,
        pass: &mut wgpu::RenderPass<'static>,
        resources: &egui_wgpu::CallbackResources,
    ) {
        let Some(gpu) = resources.get::<TermGpu>() else {
            return;
        };
        let Some(renderer) = gpu.renderers.get(&self.pane_id) else {
            return;
        };
        if let Err(e) = renderer.render(&gpu.atlas, &gpu.viewport, pass) {
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
    // the pure constants/shape that need no GPU.
    use super::PANE_PAD;

    #[test]
    fn pane_pad_is_nonnegative() {
        assert!(PANE_PAD >= 0.0);
    }
}
