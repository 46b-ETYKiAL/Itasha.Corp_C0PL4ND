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
        if surface_w <= 0.0 || surface_h <= 0.0 {
            return Vec::new();
        }
        quads
            .iter()
            .filter(|q| q.width > 0 && q.height > 0)
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

        // Pixel rect -> NDC (y flipped). Each vertex: [x, y, u, v].
        let x0 = q.x / sw * 2.0 - 1.0;
        let x1 = (q.x + q.width as f32) / sw * 2.0 - 1.0;
        let y0 = 1.0 - q.y / sh * 2.0;
        let y1 = 1.0 - (q.y + q.height as f32) / sh * 2.0;
        let verts: [[f32; 4]; 6] = [
            [x0, y0, 0.0, 0.0],
            [x0, y1, 0.0, 1.0],
            [x1, y1, 1.0, 1.0],
            [x0, y0, 0.0, 0.0],
            [x1, y1, 1.0, 1.0],
            [x1, y0, 1.0, 0.0],
        ];
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
