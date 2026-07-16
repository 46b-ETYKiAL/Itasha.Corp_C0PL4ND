//! GPU textured-quad renderer for inline images (decoded Sixel/Kitty).
//!
//! Uploads each [`c0pl4nd_core::image::DecodedImage`] to a texture and draws it
//! as an alpha-blended quad at a pixel position over the terminal grid.
//!
//! NOTE: this is the LEGACY-winit GPU render path; the shipping `c0pl4nd` egui
//! binary does NOT use it — it uploads inline images through the cached
//! `ImageTextureCache` in `egui_app/mod.rs` (a seen-set + end-of-frame
//! `prune_unseen`, i.e. there IS a cache lifecycle on the live path). This
//! renderer is kept simple — resources rebuilt per frame — because the legacy
//! path it serves is only built under the `legacy-winit` feature.

use wgpu::util::DeviceExt;

/// A request to draw one image at a pixel rectangle.
pub struct ImageQuad {
    pub rgba: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub x: f32,
    pub y: f32,
}

/// GPU resources for one prepared image (kept alive across the render pass).
pub struct Prepared {
    bind_group: wgpu::BindGroup,
    vbuf: wgpu::Buffer,
    _texture: wgpu::Texture,
}

pub struct ImageRenderer {
    pipeline: wgpu::RenderPipeline,
    sampler: wgpu::Sampler,
    layout: wgpu::BindGroupLayout,
}

impl ImageRenderer {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("image-quad"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("image-bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("image-pl"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("image-pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs"),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: 16,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2],
                }],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs"),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("image-sampler"),
            ..Default::default()
        });
        ImageRenderer {
            pipeline,
            sampler,
            layout,
        }
    }

    /// Upload + build GPU resources for every quad. Call before the render pass.
    pub fn prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        surface_w: f32,
        surface_h: f32,
        quads: &[ImageQuad],
    ) -> Vec<Prepared> {
        // Guard a degenerate surface: the NDC math in `prepare_one` divides by
        // `surface_w`/`surface_h`, so a zero surface would yield inf/NaN verts.
        // Mirrors `pane_render.rs::ChromeRenderer::prepare`.
        if !surface_is_drawable(surface_w, surface_h) {
            return Vec::new();
        }
        quads
            .iter()
            .filter(|q| is_drawable(q))
            .map(|q| self.prepare_one(device, queue, surface_w, surface_h, q))
            .collect()
    }

    fn prepare_one(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        sw: f32,
        sh: f32,
        q: &ImageQuad,
    ) -> Prepared {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("inline-image"),
            size: wgpu::Extent3d {
                width: q.width,
                height: q.height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &q.rgba,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(q.width * 4),
                rows_per_image: Some(q.height),
            },
            wgpu::Extent3d {
                width: q.width,
                height: q.height,
                depth_or_array_layers: 1,
            },
        );
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("image-bg"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });

        // Pixel rect -> NDC (y flipped). Each vertex: [x, y, u, v]. The math is
        // extracted into the pure `quad_ndc_verts` so it is unit-testable without
        // a GPU device (the rest of this function needs a live `wgpu::Device`).
        let verts = quad_ndc_verts(q.x, q.y, q.width as f32, q.height as f32, sw, sh);
        let vbuf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("image-vbuf"),
            contents: bytemuck_cast(&verts),
            usage: wgpu::BufferUsages::VERTEX,
        });
        Prepared {
            bind_group,
            vbuf,
            _texture: texture,
        }
    }

    /// Draw all prepared quads into an open render pass.
    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>, prepared: &[Prepared]) {
        if prepared.is_empty() {
            return;
        }
        pass.set_pipeline(&self.pipeline);
        for p in prepared {
            pass.set_bind_group(0, &p.bind_group, &[]);
            pass.set_vertex_buffer(0, p.vbuf.slice(..));
            pass.draw(0..6, 0..1);
        }
    }
}

/// Whether a quad has any drawable area. A zero-dimension image would produce a
/// zero-extent texture, which `wgpu` rejects (a validation error, not a silent
/// no-op), so these are filtered out before [`ImageRenderer::prepare_one`].
/// Split out of the `prepare` filter so the predicate is unit-testable against
/// the REAL code path rather than a re-stated copy of it.
fn is_drawable(q: &ImageQuad) -> bool {
    q.width > 0 && q.height > 0
}

/// Whether the target surface has area. The NDC math in
/// [`ImageRenderer::prepare_one`] divides by the surface extents, so a
/// zero-sized surface (e.g. a minimized window) would yield inf/NaN vertices.
/// Mirrors `pane_render.rs::ChromeRenderer::prepare`.
fn surface_is_drawable(surface_w: f32, surface_h: f32) -> bool {
    surface_w > 0.0 && surface_h > 0.0
}

/// Map a pixel-space rectangle `(x, y, w, h)` over a `(sw, sh)` surface into the
/// six NDC vertices (two triangles) of a textured quad, with V flipped so the
/// image is upright. Each vertex is `[ndc_x, ndc_y, u, v]`. Pure — no GPU.
fn quad_ndc_verts(x: f32, y: f32, w: f32, h: f32, sw: f32, sh: f32) -> [[f32; 4]; 6] {
    let x0 = x / sw * 2.0 - 1.0;
    let x1 = (x + w) / sw * 2.0 - 1.0;
    let y0 = 1.0 - y / sh * 2.0;
    let y1 = 1.0 - (y + h) / sh * 2.0;
    [
        [x0, y0, 0.0, 0.0],
        [x0, y1, 0.0, 1.0],
        [x1, y1, 1.0, 1.0],
        [x0, y0, 0.0, 0.0],
        [x1, y1, 1.0, 1.0],
        [x1, y0, 1.0, 0.0],
    ]
}

/// Reinterpret the vertex array as bytes (no external bytemuck dependency).
fn bytemuck_cast(verts: &[[f32; 4]; 6]) -> &[u8] {
    // SAFETY: `verts.as_ptr()` is a valid, aligned pointer to `[[f32;4];6]` — a
    // contiguous block of `f32`s (no padding, all bit patterns valid as bytes).
    // The length is exactly its byte size, so the reinterpreted `u8` slice stays
    // in-bounds, and it borrows `verts` so the source outlives the returned slice.
    unsafe { std::slice::from_raw_parts(verts.as_ptr() as *const u8, std::mem::size_of_val(verts)) }
}

const SHADER: &str = r#"
struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs(@location(0) pos: vec2<f32>, @location(1) uv: vec2<f32>) -> VsOut {
    var o: VsOut;
    o.pos = vec4<f32>(pos, 0.0, 1.0);
    o.uv = uv;
    return o;
}

@group(0) @binding(0) var tex: texture_2d<f32>;
@group(0) @binding(1) var samp: sampler;

@fragment
fn fs(in: VsOut) -> @location(0) vec4<f32> {
    return textureSample(tex, samp, in.uv);
}
"#;

#[cfg(test)]
mod tests {
    use super::*;

    /// A quad covering the whole surface maps to the full NDC square with
    /// upright UVs (top-left = (0,0), bottom-right = (1,1)).
    #[test]
    fn full_surface_quad_maps_to_full_ndc_square() {
        let v = quad_ndc_verts(0.0, 0.0, 100.0, 50.0, 100.0, 50.0);
        // Triangle-1 first vertex = top-left: NDC (-1, +1), UV (0,0).
        assert_eq!(v[0], [-1.0, 1.0, 0.0, 0.0]);
        // Second vertex = bottom-left: NDC (-1, -1), UV (0,1).
        assert_eq!(v[1], [-1.0, -1.0, 0.0, 1.0]);
        // Third = bottom-right: NDC (+1, -1), UV (1,1).
        assert_eq!(v[2], [1.0, -1.0, 1.0, 1.0]);
        // Last vertex = top-right: NDC (+1, +1), UV (1,0).
        assert_eq!(v[5], [1.0, 1.0, 1.0, 0.0]);
    }

    /// The two triangles share the TL and BR vertices (a degenerate-free quad):
    /// v[0]==v[3] (top-left) and v[2]==v[4] (bottom-right).
    #[test]
    fn two_triangles_share_diagonal_vertices() {
        let v = quad_ndc_verts(10.0, 20.0, 30.0, 40.0, 200.0, 100.0);
        assert_eq!(v[0], v[3], "both triangles start at the top-left vertex");
        assert_eq!(v[2], v[4], "both triangles share the bottom-right vertex");
    }

    /// V is flipped: a quad at the TOP of the surface (small y) has the HIGHER
    /// NDC y (closer to +1), and U increases left→right.
    #[test]
    fn v_axis_is_flipped_and_offset_quad_is_centered() {
        // A 50x50 quad centred on a 100x100 surface spans pixel [25,75] in both
        // axes => NDC [-0.5, +0.5] in x, and y flipped to [+0.5, -0.5].
        let v = quad_ndc_verts(25.0, 25.0, 50.0, 50.0, 100.0, 100.0);
        let (x0, y0) = (v[0][0], v[0][1]);
        let (x1, y1) = (v[2][0], v[2][1]);
        assert!((x0 - -0.5).abs() < 1e-6, "left edge at NDC -0.5, got {x0}");
        assert!(
            (y0 - 0.5).abs() < 1e-6,
            "top edge at NDC +0.5 (flipped), got {y0}"
        );
        assert!((x1 - 0.5).abs() < 1e-6, "right edge at NDC +0.5, got {x1}");
        assert!(
            (y1 - -0.5).abs() < 1e-6,
            "bottom edge at NDC -0.5 (flipped), got {y1}"
        );
    }

    /// `bytemuck_cast` reinterprets the 6×4 f32 vertex array as exactly its byte
    /// size with no truncation, and the bytes round-trip back to the f32 values.
    #[test]
    fn bytemuck_cast_has_exact_byte_length_and_round_trips() {
        let v = quad_ndc_verts(0.0, 0.0, 10.0, 10.0, 20.0, 20.0);
        let bytes = bytemuck_cast(&v);
        assert_eq!(bytes.len(), 6 * 4 * std::mem::size_of::<f32>());
        // First f32 (v[0][0]) reconstructed from its little/native-endian bytes.
        let first = f32::from_ne_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
        assert_eq!(first, v[0][0]);
    }

    fn quad(width: u32, height: u32) -> ImageQuad {
        ImageQuad {
            rgba: vec![0; (width as usize * height as usize * 4).max(4)],
            width,
            height,
            x: 0.0,
            y: 0.0,
        }
    }

    /// The drawable filter `prepare` actually applies: a zero-dimension image
    /// would build a zero-extent texture and trip wgpu validation.
    ///
    /// This calls the REAL `is_drawable`. The previous version of this test
    /// declared its own `|w, h| w > 0 && h > 0` closure and asserted on that, so
    /// it passed identically whether or not the production filter existed at all.
    #[test]
    fn zero_dimension_quads_are_not_drawable() {
        assert!(!is_drawable(&quad(0, 10)), "zero width is not drawable");
        assert!(!is_drawable(&quad(10, 0)), "zero height is not drawable");
        assert!(!is_drawable(&quad(0, 0)));
        assert!(is_drawable(&quad(1, 1)), "a 1x1 image is drawable");
        assert!(is_drawable(&quad(64, 32)));
    }

    /// A zero-sized surface (e.g. a minimized window) must short-circuit before
    /// the NDC divide, which would otherwise emit inf/NaN vertices.
    #[test]
    fn zero_sized_surface_is_not_drawable() {
        assert!(!surface_is_drawable(0.0, 100.0));
        assert!(!surface_is_drawable(100.0, 0.0));
        assert!(!surface_is_drawable(-1.0, 100.0));
        assert!(surface_is_drawable(1.0, 1.0));
        assert!(surface_is_drawable(1920.0, 1080.0));
    }

    /// Guard the reason the surface check exists: the NDC math really does blow
    /// up on a zero surface, so the early return is load-bearing, not defensive
    /// decoration.
    #[test]
    fn ndc_math_would_produce_non_finite_verts_on_a_zero_surface() {
        let v = quad_ndc_verts(0.0, 0.0, 10.0, 10.0, 0.0, 0.0);
        assert!(
            v.iter().flatten().any(|c| !c.is_finite()),
            "a zero surface must yield non-finite verts, which is why prepare() bails first"
        );
        // ...and a real surface stays finite.
        let ok = quad_ndc_verts(0.0, 0.0, 10.0, 10.0, 100.0, 100.0);
        assert!(ok.iter().flatten().all(|c| c.is_finite()));
    }
}
