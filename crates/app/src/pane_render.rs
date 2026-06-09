//! Per-leaf render geometry + pane chrome (gutters, borders, focus accent).
//!
//! The split-tree engine in
//! `c0pl4nd-core::layout` produces a cascade of `(LeafId, Rect)` pairs; this
//! module turns those rects into the pieces the GPU renderer needs:
//!
//! - [`leaf_text_bounds`] / [`leaf_text_origin`] — where a leaf's terminal grid
//!   text buffer is placed and clipped (one glyphon `TextArea` per visible
//!   leaf, all drawn in a single `prepare`).
//! - [`leaf_scissor`] — the wgpu scissor rect for a leaf's inline-image,
//!   cursor, and selection quads, so per-cell content cannot bleed across a
//!   pane border.
//! - [`ChromeRenderer`] — a tiny solid-colour quad pipeline that draws the
//!   inter-pane gutters and a per-leaf border, highlighting the focused leaf in
//!   the brand accent and muting the rest (the Windows-Terminal pattern).
//!
//! A SINGLE pane (one leaf filling the whole area) draws no border and no
//! gutters — visually identical to the pre-split renderer.

use c0pl4nd_core::layout::Rect;
use glyphon::TextBounds;

/// Border thickness, in physical pixels, drawn inside each multi-pane leaf
/// rect. Zero for a single pane (no chrome).
pub const BORDER_PX: i32 = 1;

/// A solid-colour rectangle in physical-pixel surface coordinates, with a
/// straight-alpha RGBA colour. Consumed by [`ChromeRenderer::prepare`].
#[derive(Debug, Clone, Copy)]
pub struct ColorRect {
    /// Left edge (px).
    pub x: i32,
    /// Top edge (px).
    pub y: i32,
    /// Width (px).
    pub w: i32,
    /// Height (px).
    pub h: i32,
    /// Straight-alpha sRGB colour, 0..=1 per channel.
    pub rgba: [f32; 4],
}

impl ColorRect {
    /// Construct a colour rect.
    #[must_use]
    pub fn new(x: i32, y: i32, w: i32, h: i32, rgba: [f32; 4]) -> Self {
        Self { x, y, w, h, rgba }
    }
}

/// The glyphon [`TextBounds`] clipping a leaf's grid text to its cell, inset by
/// the pane border so glyphs never paint over the border line.
#[must_use]
pub fn leaf_text_bounds(cell: Rect, border: i32) -> TextBounds {
    let left = cell.x + border;
    let top = cell.y + border;
    let right = (cell.x + cell.w - border).max(left);
    let bottom = (cell.y + cell.h - border).max(top);
    TextBounds {
        left,
        top,
        right,
        bottom,
    }
}

/// The top-left pixel origin (left, top) where a leaf's grid text buffer is
/// placed, inset by the border plus a small left pad matching the single-pane
/// renderer's content inset.
#[must_use]
pub fn leaf_text_origin(cell: Rect, border: i32, left_pad: f32, top_pad: f32) -> (f32, f32) {
    (
        cell.x as f32 + border as f32 + left_pad,
        cell.y as f32 + border as f32 + top_pad,
    )
}

/// The wgpu scissor rect `(x, y, w, h)` for a leaf's per-cell content (images,
/// cursor, selection), clamped to non-negative extents inside the surface.
#[must_use]
pub fn leaf_scissor(
    cell: Rect,
    border: i32,
    surface_w: u32,
    surface_h: u32,
) -> (u32, u32, u32, u32) {
    let x = (cell.x + border).max(0);
    let y = (cell.y + border).max(0);
    let right = (cell.x + cell.w - border).min(surface_w as i32);
    let bottom = (cell.y + cell.h - border).min(surface_h as i32);
    let w = (right - x).max(0);
    let h = (bottom - y).max(0);
    (x as u32, y as u32, w as u32, h as u32)
}

/// Build the gutter + per-leaf border chrome quads for a cascade.
///
/// When `cells` holds a single leaf the result is empty (no chrome for a
/// single pane). Otherwise each leaf gets a 1px border in `border_rgba`, the
/// focused leaf's border is `accent_rgba`, and the gaps between cells are
/// filled with `gutter_rgba`.
#[must_use]
pub fn chrome_quads(
    cells: &[(c0pl4nd_core::layout::LeafId, Rect)],
    focused: c0pl4nd_core::layout::LeafId,
    accent_rgba: [f32; 4],
    border_rgba: [f32; 4],
    gutter_rgba: [f32; 4],
    surface: Rect,
) -> Vec<ColorRect> {
    let mut out = Vec::new();
    if cells.len() <= 1 {
        return out;
    }
    // Gutter background fills the whole surface; cell borders + cell interiors
    // paint over it, leaving the 1px seams between cells showing the gutter.
    out.push(ColorRect::new(
        surface.x,
        surface.y,
        surface.w,
        surface.h,
        gutter_rgba,
    ));
    for (id, cell) in cells {
        let rgba = if *id == focused {
            accent_rgba
        } else {
            border_rgba
        };
        out.extend(border_ring(*cell, BORDER_PX, rgba));
    }
    out
}

/// The four edge rects forming a `thickness`-px border ring around `cell`.
fn border_ring(cell: Rect, thickness: i32, rgba: [f32; 4]) -> [ColorRect; 4] {
    let t = thickness.max(1);
    [
        // top
        ColorRect::new(cell.x, cell.y, cell.w, t, rgba),
        // bottom
        ColorRect::new(cell.x, cell.y + cell.h - t, cell.w, t, rgba),
        // left
        ColorRect::new(cell.x, cell.y, t, cell.h, rgba),
        // right
        ColorRect::new(cell.x + cell.w - t, cell.y, t, cell.h, rgba),
    ]
}

/// Render a cell's nested-tab strip into a single line of text: each tab shown
/// as ` <n> ` (1-based), the active tab wrapped in brackets `[<n>]`, truncated
/// grapheme-aware to `max_cols` columns. Returns an empty string for a <2-tab
/// cell (no strip). Pure — the caller places it as a TextArea at the cell top.
#[must_use]
pub fn cell_tabbar_text(tab_count: usize, active: usize, max_cols: usize) -> String {
    if tab_count < 2 || max_cols == 0 {
        return String::new();
    }
    let mut s = String::new();
    for i in 0..tab_count {
        let label = if i == active {
            format!("[{}]", i + 1)
        } else {
            format!(" {} ", i + 1)
        };
        s.push_str(&label);
    }
    // Grapheme-agnostic char truncation is safe here: labels are ASCII digits +
    // brackets + spaces, so a char boundary is always a grapheme boundary.
    if s.chars().count() > max_cols {
        s = s.chars().take(max_cols).collect();
    }
    s
}

/// A minimal solid-colour quad pipeline for pane chrome (gutters + borders).
///
/// Mirrors the structure of `image_render::ImageRenderer`: build per-frame
/// vertex buffers in [`ChromeRenderer::prepare`], then issue draws in
/// [`ChromeRenderer::draw`]. No external dependency — vertices are cast to
/// bytes the same way the image renderer does.
pub struct ChromeRenderer {
    pipeline: wgpu::RenderPipeline,
}

/// A prepared chrome quad: its vertex buffer (two triangles, 6 verts of
/// `[x, y, r, g, b, a]`).
pub struct PreparedQuad {
    vbuf: wgpu::Buffer,
}

impl ChromeRenderer {
    /// Construct the chrome quad pipeline for `format`.
    #[must_use]
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("chrome-quad-shader"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("chrome-quad-layout"),
            bind_group_layouts: &[],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("chrome-quad-pipeline"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: 6 * 4,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &[
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x2,
                            offset: 0,
                            shader_location: 0,
                        },
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x4,
                            offset: 2 * 4,
                            shader_location: 1,
                        },
                    ],
                }],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        Self { pipeline }
    }

    /// Build one vertex buffer per quad, mapping pixel rects into clip space
    /// for the `surface_w`×`surface_h` target.
    #[must_use]
    pub fn prepare(
        &self,
        device: &wgpu::Device,
        surface_w: f32,
        surface_h: f32,
        quads: &[ColorRect],
    ) -> Vec<PreparedQuad> {
        quads
            .iter()
            .filter(|q| q.w > 0 && q.h > 0 && surface_w > 0.0 && surface_h > 0.0)
            .map(|q| {
                let verts = quad_verts(q, surface_w, surface_h);
                let vbuf = device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("chrome-quad-vbuf"),
                    size: std::mem::size_of_val(&verts) as u64,
                    usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: true,
                });
                vbuf.slice(..)
                    .get_mapped_range_mut()
                    .copy_from_slice(verts_bytes(&verts));
                vbuf.unmap();
                PreparedQuad { vbuf }
            })
            .collect()
    }

    /// Draw the prepared chrome quads.
    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>, prepared: &[PreparedQuad]) {
        if prepared.is_empty() {
            return;
        }
        pass.set_pipeline(&self.pipeline);
        for p in prepared {
            pass.set_vertex_buffer(0, p.vbuf.slice(..));
            pass.draw(0..6, 0..1);
        }
    }
}

/// Two triangles (6 verts of `[x, y, r, g, b, a]`) for `q` in clip space.
fn quad_verts(q: &ColorRect, sw: f32, sh: f32) -> [[f32; 6]; 6] {
    // Pixel → NDC: x in [-1, 1] left→right, y in [1, -1] top→bottom.
    let to_ndc = |px: f32, py: f32| -> (f32, f32) { (px / sw * 2.0 - 1.0, 1.0 - py / sh * 2.0) };
    let (l, t) = to_ndc(q.x as f32, q.y as f32);
    let (r, b) = to_ndc((q.x + q.w) as f32, (q.y + q.h) as f32);
    let [cr, cg, cb, ca] = q.rgba;
    let v = |x: f32, y: f32| [x, y, cr, cg, cb, ca];
    [v(l, t), v(r, t), v(l, b), v(r, t), v(r, b), v(l, b)]
}

/// Reinterpret the vertex array as bytes (no external bytemuck dependency).
fn verts_bytes(verts: &[[f32; 6]; 6]) -> &[u8] {
    // SAFETY: `verts.as_ptr()` is a valid, aligned pointer to `[[f32;6];6]` — a
    // contiguous block of `f32`s (no padding, all bit patterns valid as bytes).
    // The length is exactly its byte size, so the reinterpreted `u8` slice stays
    // in-bounds, and it borrows `verts` so the source outlives the returned slice.
    unsafe { std::slice::from_raw_parts(verts.as_ptr() as *const u8, std::mem::size_of_val(verts)) }
}

const SHADER: &str = r#"
struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) color: vec4<f32>,
};

@vertex
fn vs(@location(0) pos: vec2<f32>, @location(1) color: vec4<f32>) -> VsOut {
    var o: VsOut;
    o.pos = vec4<f32>(pos, 0.0, 1.0);
    o.color = color;
    return o;
}

@fragment
fn fs(in: VsOut) -> @location(0) vec4<f32> {
    return in.color;
}
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use c0pl4nd_core::layout::LeafId;

    #[test]
    fn single_pane_has_no_chrome() {
        let cells = vec![(LeafId(0), Rect::new(0, 0, 800, 600))];
        let quads = chrome_quads(
            &cells,
            LeafId(0),
            [0.0, 1.0, 1.0, 1.0],
            [0.3, 0.3, 0.3, 1.0],
            [0.1, 0.1, 0.1, 1.0],
            Rect::new(0, 0, 800, 600),
        );
        assert!(quads.is_empty(), "single pane must draw no border/gutter");
    }

    #[test]
    fn multi_pane_chrome_has_gutter_plus_four_edges_per_cell() {
        let cells = vec![
            (LeafId(0), Rect::new(0, 0, 400, 600)),
            (LeafId(1), Rect::new(401, 0, 399, 600)),
        ];
        let quads = chrome_quads(
            &cells,
            LeafId(0),
            [0.0, 1.0, 1.0, 1.0],
            [0.3, 0.3, 0.3, 1.0],
            [0.1, 0.1, 0.1, 1.0],
            Rect::new(0, 0, 800, 600),
        );
        // 1 gutter fill + 4 border edges per cell × 2 cells.
        assert_eq!(quads.len(), 1 + 4 * 2);
        // First quad is the full-surface gutter fill.
        assert_eq!(
            (quads[0].x, quads[0].y, quads[0].w, quads[0].h),
            (0, 0, 800, 600)
        );
    }

    #[test]
    fn focused_leaf_uses_accent_border() {
        let cells = vec![
            (LeafId(0), Rect::new(0, 0, 400, 600)),
            (LeafId(1), Rect::new(401, 0, 399, 600)),
        ];
        let accent = [0.0, 1.0, 1.0, 1.0];
        let border = [0.3, 0.3, 0.3, 1.0];
        let quads = chrome_quads(
            &cells,
            LeafId(1),
            accent,
            border,
            [0.1, 0.1, 0.1, 1.0],
            Rect::new(0, 0, 800, 600),
        );
        // Quads 1..=4 are leaf 0's border (muted); 5..=8 are leaf 1 (accent).
        assert_eq!(quads[1].rgba, border);
        assert_eq!(quads[5].rgba, accent);
    }

    #[test]
    fn text_bounds_inset_by_border() {
        let b = leaf_text_bounds(Rect::new(10, 20, 100, 50), 1);
        assert_eq!(b.left, 11);
        assert_eq!(b.top, 21);
        assert_eq!(b.right, 109);
        assert_eq!(b.bottom, 69);
    }

    #[test]
    fn text_bounds_never_inverts_on_tiny_cell() {
        // A cell smaller than 2× the border must not produce right < left.
        let b = leaf_text_bounds(Rect::new(0, 0, 1, 1), 1);
        assert!(b.right >= b.left);
        assert!(b.bottom >= b.top);
    }

    #[test]
    fn text_origin_adds_border_and_pad() {
        let (x, y) = leaf_text_origin(Rect::new(10, 20, 100, 50), 1, 8.0, 2.0);
        assert_eq!(x, 19.0); // 10 + 1 + 8
        assert_eq!(y, 23.0); // 20 + 1 + 2
    }

    #[test]
    fn scissor_clamps_to_surface_and_border() {
        let (x, y, w, h) = leaf_scissor(Rect::new(0, 0, 400, 600), 1, 800, 600);
        assert_eq!(x, 1);
        assert_eq!(y, 1);
        assert_eq!(w, 398); // 0+400-1 (right) - 1 (x)
        assert_eq!(h, 598); // 0+600-1 (bottom) - 1 (y)
    }

    #[test]
    fn scissor_never_overflows_surface() {
        // A cell that extends to the surface edge stays within bounds.
        let (x, y, w, h) = leaf_scissor(Rect::new(700, 500, 100, 100), 0, 800, 600);
        assert!(x + w <= 800);
        assert!(y + h <= 600);
    }

    #[test]
    fn cell_tabbar_empty_below_two_tabs() {
        assert_eq!(cell_tabbar_text(0, 0, 80), "");
        assert_eq!(cell_tabbar_text(1, 0, 80), "");
        assert_eq!(cell_tabbar_text(2, 0, 0), "");
    }

    #[test]
    fn cell_tabbar_marks_active_tab() {
        // Three tabs, second active: " 1 [2] 3 ".
        let s = cell_tabbar_text(3, 1, 80);
        assert_eq!(s, " 1 [2] 3 ");
        // First active.
        assert_eq!(cell_tabbar_text(2, 0, 80), "[1] 2 ");
    }

    #[test]
    fn cell_tabbar_truncates_to_max_cols() {
        let s = cell_tabbar_text(6, 0, 5);
        assert_eq!(s.chars().count(), 5);
    }
}
