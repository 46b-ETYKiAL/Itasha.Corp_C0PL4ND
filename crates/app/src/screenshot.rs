//! Headless screenshot capture.
//!
//! Renders a sample C0PL4ND frame to an offscreen texture (no window/surface)
//! and writes it as a PNG. Used to produce README/marketing media on CI
//! runners that have no display. Reuses the same glyphon text pipeline as the
//! live renderer.

use std::path::Path;

use anyhow::{Context, Result};
use c0pl4nd_core::{theme::parse_hex, Config, Theme};
use glyphon::{
    Attrs, Buffer, Cache, Color as GColor, Family, FontSystem, Metrics, Resolution, Shaping,
    SwashCache, TextArea, TextAtlas, TextBounds, TextRenderer, Viewport,
};

const W: u32 = 920;
const H: u32 = 560;

/// Render a sample frame and save it to `out` as PNG.
pub fn capture(config: &Config, out: &Path) -> Result<()> {
    let theme = load_theme(&config.theme).unwrap_or_else(Theme::builtin_void);
    let (br, bg_, bb) = parse_hex(&theme.background).unwrap_or((8, 6, 13));
    let (fr, fg_, fb) = parse_hex(&theme.foreground).unwrap_or((240, 238, 245));
    let (cr, cg, cb) = parse_hex(&theme.cursor).unwrap_or((0, 229, 255));
    let fg = GColor::rgb(fr, fg_, fb);
    let accent = GColor::rgb(cr, cg, cb);

    let instance = wgpu::Instance::default();
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::LowPower,
        compatible_surface: None,
        force_fallback_adapter: false,
    }))
    .context("no GPU adapter for screenshot")?;
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("c0pl4nd-screenshot"),
        ..Default::default()
    }))
    .context("request_device failed")?;

    let format = wgpu::TextureFormat::Rgba8UnormSrgb;
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("offscreen"),
        size: wgpu::Extent3d {
            width: W,
            height: H,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());

    let mut font_system = FontSystem::new();
    let mut swash = SwashCache::new();
    let cache = Cache::new(&device);
    let mut viewport = Viewport::new(&device, &cache);
    let mut atlas = TextAtlas::new(&device, &queue, &cache, format);
    let mut renderer =
        TextRenderer::new(&mut atlas, &device, wgpu::MultisampleState::default(), None);

    // Decode a Sixel image and prepare it for the inline-image renderer — this
    // exercises the full decode -> GPU-texture -> quad path headlessly.
    let img_renderer = crate::image_render::ImageRenderer::new(&device, format);
    let mut sixel: Vec<u8> = b"#0;2;0;90;100".to_vec(); // teal band
    sixel.extend_from_slice(b"!260~-!260~-!260~");
    let mut sixel2: Vec<u8> = b"#1;2;88;12;100".to_vec(); // pink band
    sixel2.extend_from_slice(b"!260~-!260~-!260~");
    let mut sixel_all = sixel;
    sixel_all.extend_from_slice(b"-");
    sixel_all.extend_from_slice(&sixel2);
    let image_quads: Vec<crate::image_render::ImageQuad> =
        match c0pl4nd_core::image::decode_sixel(&sixel_all) {
            Some(img) => vec![crate::image_render::ImageQuad {
                rgba: img.rgba,
                width: img.width as u32,
                height: img.height as u32,
                x: 360.0,
                y: 250.0,
            }],
            None => Vec::new(),
        };

    // Sample content showcasing the brand theme + colours.
    let green = GColor::rgb(0, 255, 179);
    let pink = GColor::rgb(224, 32, 255);
    let red = GColor::rgb(255, 0, 64);
    let spans: Vec<(&str, GColor)> = vec![
        (
            " C0PL4ND  \u{2014}  the operator's shell into the wired\n\n",
            accent,
        ),
        ("operator@wired", green),
        (":", fg),
        ("~/net", GColor::rgb(0, 102, 255)),
        ("$ ", accent),
        ("c0pl4nd --version\n", fg),
        ("C0PL4ND 0.1.0\n", fg),
        ("operator@wired", green),
        (":", fg),
        ("~/net", GColor::rgb(0, 102, 255)),
        ("$ ", accent),
        ("ls\n", fg),
        ("themes/  ", accent),
        ("plugins/  ", pink),
        ("config.toml  ", fg),
        ("README.md\n", fg),
        ("[ok] ", green),
        ("GPU render \u{2022} ", fg),
        ("[warn] ", GColor::rgb(217, 165, 33)),
        ("present-day \u{2022} ", fg),
        ("[err] ", red),
        ("present-time\n", fg),
        ("inline image (Sixel) \u{2193}\n", accent),
        ("\u{2588}", accent),
    ];
    let mut buffer = Buffer::new(&mut font_system, Metrics::new(16.0, 22.0));
    buffer.set_size(
        &mut font_system,
        Some(W as f32 - 24.0),
        Some(H as f32 - 24.0),
    );
    buffer.set_rich_text(
        &mut font_system,
        spans
            .iter()
            .map(|(s, c)| (*s, Attrs::new().family(Family::Monospace).color(*c))),
        &Attrs::new().family(Family::Monospace).color(fg),
        Shaping::Advanced,
        None,
    );
    buffer.shape_until_scroll(&mut font_system, false);

    viewport.update(
        &queue,
        Resolution {
            width: W,
            height: H,
        },
    );
    renderer
        .prepare(
            &device,
            &queue,
            &mut font_system,
            &mut atlas,
            &viewport,
            [TextArea {
                buffer: &buffer,
                left: 14.0,
                top: 12.0,
                scale: 1.0,
                bounds: TextBounds {
                    left: 0,
                    top: 0,
                    right: W as i32,
                    bottom: H as i32,
                },
                default_color: fg,
                custom_glyphs: &[],
            }],
            &mut swash,
        )
        .context("glyphon prepare failed")?;

    let prepared_imgs = img_renderer.prepare(&device, &queue, W as f32, H as f32, &image_quads);

    // Readback buffer: bytes_per_row must be 256-aligned.
    let unpadded = W * 4;
    let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let padded = unpadded.div_ceil(align) * align;
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("readback"),
        size: (padded * H) as u64,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    let mut encoder =
        device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("screenshot"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: srgb_to_linear(br),
                        g: srgb_to_linear(bg_),
                        b: srgb_to_linear(bb),
                        a: 1.0,
                    }),
                    store: wgpu::StoreOp::Store,
                },
                depth_slice: None,
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        renderer
            .render(&atlas, &viewport, &mut pass)
            .context("glyphon render failed")?;
        img_renderer.draw(&mut pass, &prepared_imgs);
    }
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &readback,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded),
                rows_per_image: Some(H),
            },
        },
        wgpu::Extent3d {
            width: W,
            height: H,
            depth_or_array_layers: 1,
        },
    );
    queue.submit(std::iter::once(encoder.finish()));

    // Map and read back, stripping row padding.
    readback.slice(..).map_async(wgpu::MapMode::Read, |_| {});
    let _ = device.poll(wgpu::PollType::wait_indefinitely());
    let data = readback.slice(..).get_mapped_range();
    let mut rgba = Vec::with_capacity((W * H * 4) as usize);
    for row in 0..H {
        let start = (row * padded) as usize;
        rgba.extend_from_slice(&data[start..start + unpadded as usize]);
    }
    drop(data);
    readback.unmap();

    let img = image::RgbaImage::from_raw(W, H, rgba).context("image buffer size mismatch")?;
    if let Some(parent) = out.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    img.save(out)
        .with_context(|| format!("failed to write {out:?}"))?;
    Ok(())
}

fn srgb_to_linear(c: u8) -> f64 {
    let s = c as f64 / 255.0;
    if s <= 0.04045 {
        s / 12.92
    } else {
        ((s + 0.055) / 1.055).powf(2.4)
    }
}

fn load_theme(name: &str) -> Option<Theme> {
    let p = std::path::PathBuf::from("assets/themes").join(format!("{name}.toml"));
    Theme::load_from(&p).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reference IEC 61966-2-1 sRGB→linear transfer for cross-checking.
    fn reference_srgb_to_linear(c: u8) -> f64 {
        let s = c as f64 / 255.0;
        if s <= 0.04045 {
            s / 12.92
        } else {
            ((s + 0.055) / 1.055).powf(2.4)
        }
    }

    #[test]
    fn srgb_endpoints_are_exact() {
        // 0 → 0.0 and 255 → 1.0 are the anchor points; the readback PNG's
        // clear-colour correctness depends on these being exact.
        assert_eq!(srgb_to_linear(0), 0.0);
        assert!((srgb_to_linear(255) - 1.0).abs() < 1e-12);
    }

    #[test]
    fn srgb_uses_linear_segment_below_threshold() {
        // For the small-value linear segment (s <= 0.04045), the transfer is a
        // plain divide by 12.92. c=10 → s=0.0392.. which is below threshold.
        let c = 10u8;
        let s = c as f64 / 255.0;
        assert!(s <= 0.04045, "precondition: c=10 is in the linear segment");
        let expected = s / 12.92;
        assert!((srgb_to_linear(c) - expected).abs() < 1e-12);
    }

    #[test]
    fn srgb_uses_gamma_segment_above_threshold() {
        // c=11 → s=0.0431.. is just ABOVE the 0.04045 knee, so it must take the
        // power-curve branch, NOT the linear divide. This kills a mutant that
        // flips the comparison or drops the gamma branch.
        let c = 11u8;
        let s = c as f64 / 255.0;
        assert!(s > 0.04045, "precondition: c=11 is in the gamma segment");
        let gamma = ((s + 0.055) / 1.055).powf(2.4);
        let linear_wrong = s / 12.92;
        assert!((srgb_to_linear(c) - gamma).abs() < 1e-12);
        // And prove the two branches actually diverge here.
        assert!((gamma - linear_wrong).abs() > 1e-9);
    }

    #[test]
    fn srgb_is_monotonic_nondecreasing() {
        // The transfer must be monotonic across the whole 0..=255 domain.
        let mut prev = -1.0_f64;
        for c in 0u8..=255 {
            let v = srgb_to_linear(c);
            assert!(v >= prev, "non-monotonic at c={c}: {v} < {prev}");
            assert!((0.0..=1.0).contains(&v), "out of [0,1] at c={c}: {v}");
            prev = v;
        }
    }

    #[test]
    fn srgb_midpoint_matches_reference() {
        // Mid-grey 128 against an independent reference implementation.
        let v = srgb_to_linear(128);
        assert!((v - reference_srgb_to_linear(128)).abs() < 1e-12);
        // Sanity: sRGB mid-grey is ~0.2158 linear (well below 0.5 — the whole
        // point of the gamma curve).
        assert!((v - 0.2158).abs() < 1e-3, "128 → {v}, expected ~0.2158");
    }
}
