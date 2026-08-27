//! Deterministic offscreen captures of the ported WGSL pipelines.
//!
//! Each scene drives exactly one of the pipelines in
//! [`crate::ui::wgpu_pipelines`] with the fixed, procedurally generated inputs
//! in [`crate::capture_scenes`], so two runs on one host produce
//! byte-identical PNGs and the Metal capture of the same scene set
//! ([`crate::metal_shader_capture`]) can be compared against them
//! (`eseq-linux.25`). The renderer is headless: it draws into an
//! `Rgba8Unorm` texture and reads it back, so no window or surface format is
//! involved.

use std::collections::BTreeMap;
use std::path::Path;

use wgpu::util::DeviceExt;

use crate::capture_scenes::{
    glyph_atlas_pixels, image_pixels, image_vertices, live_spectrogram_instances,
    patch_cable_instances, sha256_hex, spectrogram_smoothed, spectrogram_waterfall, text_vertices,
    waveform_buckets, waveform_instance, wavetable_bank, wavetable_instance,
    widget_instances_for_scene, widget_scene_shader,
};
use crate::ui::wgpu_pipelines as pipelines;
use crate::ui::wgsl_shaders;
use crate::widget_render::{ShaderBackend, WidgetInstance};

/// Schema of the emitted `manifest.json`. Bump when the scene set or the file
/// layout changes so an old capture cannot be mistaken for a current one.
pub const SCHEMA_VERSION: u32 = 3;

pub use crate::capture_scenes::{HEIGHT, SCENES, WIDTH, pixel};

const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
/// A dark, non-neutral clear so a pipeline that writes nothing is obvious and
/// so alpha blending has something to blend against.
const CLEAR: wgpu::Color = wgpu::Color {
    r: 0.04,
    g: 0.05,
    b: 0.07,
    a: 1.0,
};


// ── Renderer ──────────────────────────────────────

/// A headless device plus the readback plumbing shared by every scene.
pub struct CaptureRenderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    adapter_name: String,
    adapter_backend: String,
}

impl CaptureRenderer {
    /// Returns `None` when the machine exposes no wgpu adapter at all, so
    /// callers can skip rather than fail on a headless box without a GPU.
    pub fn new() -> Option<Self> {
        let instance = wgpu::Instance::default();
        let adapter =
            pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))?;
        let info = adapter.get_info();
        let (device, queue) = pollster::block_on(adapter.request_device(
            &wgpu::DeviceDescriptor {
                label: Some("eseqlisp shader capture device"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::downlevel_defaults(),
                memory_hints: wgpu::MemoryHints::Performance,
            },
            None,
        ))
        .ok()?;
        Some(Self {
            device,
            queue,
            adapter_name: info.name,
            adapter_backend: format!("{:?}", info.backend),
        })
    }

    pub fn adapter_name(&self) -> &str {
        &self.adapter_name
    }

    pub fn adapter_backend(&self) -> &str {
        &self.adapter_backend
    }

    /// Measure completed GPU frame wall time for a generated widget fragment.
    ///
    /// The target is retained across frames and there is no readback in the
    /// measured loop. `device.poll(Wait)` makes each sample include the actual
    /// GPU completion time instead of only CPU command submission.
    pub fn benchmark_widget_fragment(
        &self,
        fragment_source: &str,
        instances: &[WidgetInstance],
        width: u32,
        height: u32,
        warmup_frames: usize,
        sample_frames: usize,
    ) -> Vec<std::time::Duration> {
        assert!(!instances.is_empty(), "the benchmark needs widget instances");
        assert!(width > 0 && height > 0, "the benchmark target must be nonzero");
        assert!(sample_frames > 0, "the benchmark needs at least one sample");

        let pipeline = pipelines::widget_pipeline(
            &self.device,
            "eseqlisp generated SDF lighting probe",
            None,
            fragment_source,
            FORMAT,
        );
        let instance_buffer = self.instance_buffer(instances);
        let target = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("eseqlisp SDF lighting probe target"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let view = target.create_view(&Default::default());

        let render = || {
            let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("eseqlisp SDF lighting probe encoder"),
            });
            {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("eseqlisp SDF lighting probe pass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &view,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(CLEAR),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                });
                pass.set_pipeline(&pipeline);
                pass.set_vertex_buffer(0, instance_buffer.slice(..));
                pass.draw(0..6, 0..instances.len() as u32);
            }
            self.queue.submit(Some(encoder.finish()));
            self.device.poll(wgpu::Maintain::Wait);
        };

        for _ in 0..warmup_frames {
            render();
        }
        (0..sample_frames)
            .map(|_| {
                let started = std::time::Instant::now();
                render();
                started.elapsed()
            })
            .collect()
    }

    fn instance_buffer<T: bytemuck::Pod>(&self, data: &[T]) -> wgpu::Buffer {
        self.device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("eseqlisp capture instances"),
                contents: bytemuck::cast_slice(data),
                usage: wgpu::BufferUsages::VERTEX,
            })
    }

    fn storage_buffer(&self, data: &[f32]) -> wgpu::Buffer {
        self.device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("eseqlisp capture sample data"),
                contents: bytemuck::cast_slice(data),
                usage: wgpu::BufferUsages::STORAGE,
            })
    }

    fn storage_bind_group(
        &self,
        layout: &wgpu::BindGroupLayout,
        buffers: &[&wgpu::Buffer],
    ) -> wgpu::BindGroup {
        let entries: Vec<_> = buffers
            .iter()
            .enumerate()
            .map(|(index, buffer)| wgpu::BindGroupEntry {
                binding: index as u32,
                resource: buffer.as_entire_binding(),
            })
            .collect();
        self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("eseqlisp capture storage bind group"),
            layout,
            entries: &entries,
        })
    }

    /// Upload one texture and pair it with a sampler of the requested filter.
    fn texture_bind_group(
        &self,
        layout: &wgpu::BindGroupLayout,
        width: u32,
        height: u32,
        format: wgpu::TextureFormat,
        pixels: &[u8],
        filter: wgpu::FilterMode,
    ) -> wgpu::BindGroup {
        let texture = self.device.create_texture_with_data(
            &self.queue,
            &wgpu::TextureDescriptor {
                label: Some("eseqlisp capture texture"),
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            },
            wgpu::util::TextureDataOrder::LayerMajor,
            pixels,
        );
        let view = texture.create_view(&Default::default());
        let sampler = self.device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("eseqlisp capture sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: filter,
            min_filter: filter,
            mipmap_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });
        self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("eseqlisp capture texture bind group"),
            layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
            ],
        })
    }

    /// Render one named scene and return its RGBA8 pixels, row-major from the
    /// top-left. Panics on an unknown scene name so a typo cannot silently
    /// produce a blank capture.
    pub fn render(&self, scene: &str) -> Vec<u8> {
        let target = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("eseqlisp capture target"),
            size: wgpu::Extent3d {
                width: WIDTH,
                height: HEIGHT,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = target.create_view(&Default::default());
        let mut encoder = self.device.create_command_encoder(&Default::default());

        // Resources must outlive the render pass, so each arm binds them here
        // and the pass below only references them.
        let atlas_layout = pipelines::texture_bind_group_layout(&self.device, "capture atlas");
        let storage1 = pipelines::storage_bind_group_layout(&self.device, "capture data", 1);
        let storage2 = pipelines::storage_bind_group_layout(&self.device, "capture data", 2);

        let (text_pipeline, prop_pipeline) =
            pipelines::text_pipelines(&self.device, &atlas_layout, FORMAT);
        let image_pipeline = pipelines::image_pipeline(&self.device, &atlas_layout, FORMAT);
        let patch_cable_pipeline = pipelines::patch_cable_pipeline(&self.device, FORMAT);
        // Non-widget scenes still need a widget pipeline object to exist, since
        // the pipelines are all built before the pass; any fragment will do.
        let (widget_vertex, widget_fragment) = widget_scene_shader(scene, ShaderBackend::Wgsl)
            .unwrap_or((None, wgsl_shaders::BUTTON_SURFACE_WGSL));
        let widget_pipeline = pipelines::widget_pipeline(
            &self.device,
            "eseqlisp capture widget shader",
            widget_vertex,
            widget_fragment,
            FORMAT,
        );
        let wavetable_pipeline = pipelines::wavetable_pipeline(&self.device, &storage1, FORMAT);
        let waveform_pipeline = pipelines::waveform_pipeline(&self.device, &storage1, FORMAT);
        let spectrogram_pipelines =
            pipelines::live_spectrogram_pipelines(&self.device, &storage2, FORMAT);

        let text_vertices = text_vertices();
        let text_buffer = self.instance_buffer(&text_vertices);
        let image_verts = image_vertices();
        let image_buffer = self.instance_buffer(&image_verts);
        let cables = patch_cable_instances();
        let cable_buffer = self.instance_buffer(&cables);
        let widgets = widget_instances_for_scene(scene);
        let widget_buffer = self.instance_buffer(&widgets);
        let wavetable = [wavetable_instance()];
        let wavetable_buffer = self.instance_buffer(&wavetable);
        let waveform = [waveform_instance()];
        let waveform_buffer = self.instance_buffer(&waveform);
        let spectrograms = live_spectrogram_instances();
        // The fragment entry point is selected by pipeline, so instances must
        // be grouped by mode before issuing their draw calls. As in the Metal
        // shader, mode 1 is EQ and every other value is waterfall.
        let (eq_spectrograms, waterfall_spectrograms): (Vec<_>, Vec<_>) =
            spectrograms.into_iter().partition(|instance| instance.mode == 1);
        let waterfall_spectrogram_buffer = self.instance_buffer(&waterfall_spectrograms);
        let eq_spectrogram_buffer = self.instance_buffer(&eq_spectrograms);

        let bank = self.storage_buffer(&wavetable_bank());
        let buckets = self.storage_buffer(&waveform_buckets());
        let waterfall = self.storage_buffer(&spectrogram_waterfall());
        let smoothed = self.storage_buffer(&spectrogram_smoothed());

        let bank_group = self.storage_bind_group(&storage1, &[&bank]);
        let buckets_group = self.storage_bind_group(&storage1, &[&buckets]);
        let spectrogram_group = self.storage_bind_group(&storage2, &[&waterfall, &smoothed]);

        let atlas = glyph_atlas_pixels();
        let nearest_group = self.texture_bind_group(
            &atlas_layout,
            64,
            64,
            wgpu::TextureFormat::R8Unorm,
            &atlas,
            wgpu::FilterMode::Nearest,
        );
        let linear_group = self.texture_bind_group(
            &atlas_layout,
            64,
            64,
            wgpu::TextureFormat::R8Unorm,
            &atlas,
            wgpu::FilterMode::Linear,
        );
        let image_group = self.texture_bind_group(
            &atlas_layout,
            64,
            64,
            wgpu::TextureFormat::Rgba8Unorm,
            &image_pixels(),
            wgpu::FilterMode::Linear,
        );

        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("eseqlisp capture pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(CLEAR),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            match scene {
                "text" => {
                    pass.set_pipeline(&text_pipeline);
                    pass.set_bind_group(0, &nearest_group, &[]);
                    pass.set_vertex_buffer(0, text_buffer.slice(..));
                    pass.draw(0..text_vertices.len() as u32, 0..1);
                }
                "proportional-text" => {
                    pass.set_pipeline(&prop_pipeline);
                    pass.set_bind_group(0, &linear_group, &[]);
                    pass.set_vertex_buffer(0, text_buffer.slice(..));
                    pass.draw(0..text_vertices.len() as u32, 0..1);
                }
                "image" => {
                    pass.set_pipeline(&image_pipeline);
                    pass.set_bind_group(0, &image_group, &[]);
                    pass.set_vertex_buffer(0, image_buffer.slice(..));
                    pass.draw(0..image_verts.len() as u32, 0..1);
                }
                "patch-cable" => {
                    pass.set_pipeline(&patch_cable_pipeline);
                    pass.set_vertex_buffer(0, cable_buffer.slice(..));
                    pass.draw(0..6, 0..cables.len() as u32);
                }
                widget_scene if widget_scene_shader(widget_scene, ShaderBackend::Wgsl).is_some() => {
                    pass.set_pipeline(&widget_pipeline);
                    pass.set_vertex_buffer(0, widget_buffer.slice(..));
                    pass.draw(0..6, 0..widgets.len() as u32);
                }
                "wavetable" => {
                    pass.set_pipeline(&wavetable_pipeline);
                    pass.set_bind_group(0, &bank_group, &[]);
                    pass.set_vertex_buffer(0, wavetable_buffer.slice(..));
                    pass.draw(0..6, 0..1);
                }
                "waveform" => {
                    pass.set_pipeline(&waveform_pipeline);
                    pass.set_bind_group(0, &buckets_group, &[]);
                    pass.set_vertex_buffer(0, waveform_buffer.slice(..));
                    pass.draw(0..6, 0..1);
                }
                "live-spectrogram" => {
                    pass.set_bind_group(0, &spectrogram_group, &[]);
                    pass.set_pipeline(&spectrogram_pipelines.waterfall);
                    pass.set_vertex_buffer(0, waterfall_spectrogram_buffer.slice(..));
                    pass.draw(0..6, 0..waterfall_spectrograms.len() as u32);
                    pass.set_pipeline(&spectrogram_pipelines.eq);
                    pass.set_vertex_buffer(0, eq_spectrogram_buffer.slice(..));
                    pass.draw(0..6, 0..eq_spectrograms.len() as u32);
                }
                other => panic!("unknown capture scene {other:?}"),
            }
        }

        // WIDTH * 4 is already a multiple of COPY_BYTES_PER_ROW_ALIGNMENT.
        let bytes_per_row = WIDTH * 4;
        let readback = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("eseqlisp capture readback"),
            size: (bytes_per_row * HEIGHT) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        encoder.copy_texture_to_buffer(
            target.as_image_copy(),
            wgpu::ImageCopyBuffer {
                buffer: &readback,
                layout: wgpu::ImageDataLayout {
                    offset: 0,
                    bytes_per_row: Some(bytes_per_row),
                    rows_per_image: Some(HEIGHT),
                },
            },
            wgpu::Extent3d {
                width: WIDTH,
                height: HEIGHT,
                depth_or_array_layers: 1,
            },
        );
        self.queue.submit(Some(encoder.finish()));

        readback.slice(..).map_async(wgpu::MapMode::Read, |_| {});
        self.device.poll(wgpu::Maintain::Wait);
        let pixels = readback.slice(..).get_mapped_range().to_vec();
        readback.unmap();
        pixels
    }
}

/// Render every scene and write `<output_dir>/<name>/{<scene>.png, manifest.json}`.
pub fn write_capture(
    renderer: &CaptureRenderer,
    output_dir: &Path,
    name: &str,
) -> std::io::Result<()> {
    let dir = output_dir.join(name);
    std::fs::create_dir_all(&dir)?;

    let mut digests = BTreeMap::new();
    for scene in SCENES {
        let pixels = renderer.render(scene);
        let image = image::RgbaImage::from_raw(WIDTH, HEIGHT, pixels)
            .expect("readback is exactly WIDTH * HEIGHT RGBA pixels");
        let path = dir.join(format!("{scene}.png"));
        image
            .save(&path)
            .map_err(|error| std::io::Error::other(error.to_string()))?;
        digests.insert((*scene).to_string(), sha256_hex(&std::fs::read(&path)?));
    }

    let manifest = serde_json::json!({
        "schema_version": SCHEMA_VERSION,
        "capture_name": name,
        "backend": "wgsl",
        "width": WIDTH,
        "height": HEIGHT,
        "adapter": renderer.adapter_name(),
        "adapter_backend": renderer.adapter_backend(),
        "scenes": SCENES,
        "png_sha256": digests,
    });
    std::fs::write(
        dir.join("manifest.json"),
        format!("{}\n", serde_json::to_string_pretty(&manifest)?),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The clear color as it lands in the `Rgba8Unorm` target — i.e. "nothing
    /// was drawn here".
    const CLEAR_RGBA: [u8; 4] = [10, 13, 18, 255];

    fn renderer() -> Option<CaptureRenderer> {
        let renderer = CaptureRenderer::new();
        if renderer.is_none() {
            eprintln!("SKIPPED: no wgpu adapter available on this machine");
        }
        renderer
    }

    #[track_caller]
    fn assert_near(actual: [u8; 4], expected: [u8; 4], what: &str) {
        let close = actual
            .iter()
            .zip(expected.iter())
            .all(|(a, b)| a.abs_diff(*b) <= 2);
        assert!(close, "{what}: got {actual:?}, expected about {expected:?}");
    }

    fn luminance(pixel: [u8; 4]) -> u32 {
        pixel[0] as u32 + pixel[1] as u32 + pixel[2] as u32
    }

    /// Every scene must paint something, and two renders of one scene must be
    /// byte-identical — that reproducibility is what makes the committed PNGs
    /// usable as a reference for the later Metal comparison.
    #[test]
    fn every_scene_paints_and_reproduces_exactly() {
        let Some(renderer) = renderer() else { return };
        for scene in SCENES {
            let first = renderer.render(scene);
            let second = renderer.render(scene);
            assert_eq!(first, second, "{scene} is not reproducible");

            let painted = first
                .chunks_exact(4)
                .filter(|pixel| *pixel != CLEAR_RGBA)
                .count();
            let total = (WIDTH * HEIGHT) as usize;
            assert!(
                painted * 50 > total,
                "{scene} covered only {painted}/{total} pixels — the pipeline drew almost nothing"
            );
        }
    }

    /// `text_frag` returns `mix(bg, fg, coverage)`, so a fully covered texel is
    /// the foreground and an uncovered one inside the same quad is that quad's
    /// own background — not the clear color, and not its neighbour's.
    #[test]
    fn text_pipeline_mixes_coverage_between_each_quad_foreground_and_background() {
        let Some(renderer) = renderer() else { return };
        let pixels = renderer.render("text");

        assert_near(pixel(&pixels, 464, 124), [255, 89, 89, 255], "cell 7 center");
        assert_near(pixel(&pixels, 438, 98), [61, 26, 31, 255], "cell 7 corner");
        assert_near(pixel(&pixels, 378, 98), [26, 31, 46, 255], "cell 6 corner");
    }

    /// `prop_text_frag` emits coverage as alpha and never paints a background,
    /// so the same corner that carries a background above is the clear color
    /// here while the covered center still resolves to the foreground.
    #[test]
    fn proportional_text_pipeline_emits_coverage_as_alpha_only() {
        let Some(renderer) = renderer() else { return };
        let pixels = renderer.render("proportional-text");

        assert_near(pixel(&pixels, 464, 124), [255, 89, 89, 255], "cell 7 center");
        assert_eq!(
            pixel(&pixels, 438, 98),
            CLEAR_RGBA,
            "the proportional fragment must not paint a cell background"
        );
    }

    /// One quad per branch of `image_frag`: unclipped keeps its corner, the
    /// rounded and circle-clipped quads cut theirs, and the rotated quad is
    /// dimmed by its opacity.
    #[test]
    fn image_pipeline_applies_each_clip_mode_and_opacity() {
        let Some(renderer) = renderer() else { return };
        let pixels = renderer.render("image");

        assert_ne!(
            pixel(&pixels, 26, 66),
            CLEAR_RGBA,
            "the unclipped quad must keep its corner"
        );
        assert_eq!(pixel(&pixels, 146, 66), CLEAR_RGBA, "rounded corner");
        assert_eq!(pixel(&pixels, 266, 66), CLEAR_RGBA, "circle-clipped corner");

        // Compare interiors rather than single texels: the rotated quad
        // samples the atlas through a rotation, so only the average over the
        // quad isolates the 0.55 opacity from the checkerboard underneath.
        let mean = |x0: u32| -> u32 {
            let region: Vec<u32> = (79..145)
                .flat_map(|y| (x0 + 15..x0 + 81).map(move |x| (x, y)))
                .map(|(x, y)| luminance(pixel(&pixels, x, y)))
                .collect();
            region.iter().sum::<u32>() / region.len() as u32
        };
        let opaque = mean(24);
        let translucent = mean(384);
        assert!(
            translucent * 4 < opaque * 3,
            "the 0.55-opacity quad should be markedly dimmer: {translucent} vs {opaque}"
        );
    }

    /// `patch_cable_frag` lights a near-white core inside a darkened edge, and
    /// the segmented router only paints its own path — never the area the
    /// bend encloses.
    #[test]
    fn patch_cable_pipeline_shades_a_bright_core_inside_a_dark_edge() {
        let Some(renderer) = renderer() else { return };
        let pixels = renderer.render("patch-cable");

        let core = pixel(&pixels, 32, 40);
        let edge = pixel(&pixels, 32, 43);
        assert!(
            luminance(core) > luminance(edge) * 3,
            "cable core should dominate its edge: {core:?} vs {edge:?}"
        );

        assert_ne!(
            pixel(&pixels, 296, 90),
            CLEAR_RGBA,
            "the segmented cable's first leg must be drawn"
        );
        assert_eq!(
            pixel(&pixels, 380, 90),
            CLEAR_RGBA,
            "the area inside the segmented bend must stay empty"
        );
    }

    /// Drives the shared preamble end to end: `WidgetInstance` attributes,
    /// `widget_vert`, `WidgetVaryings`, and the SDF helpers the button surface
    /// calls. The tab shape's splay is the check that `uv` reaches the fragment
    /// the right way up.
    #[test]
    fn widget_preamble_drives_the_button_surface_fragment() {
        let Some(renderer) = renderer() else { return };
        let pixels = renderer.render("widget-surface");

        let fill = pixel(&pixels, 88, 80);
        assert!(
            fill[2] > fill[1] && fill[1] > fill[0],
            "the blue instance should read blue: {fill:?}"
        );
        assert_eq!(
            pixel(&pixels, 26, 42),
            CLEAR_RGBA,
            "the rounded corner must be cut away"
        );

        let painted = |y: u32| (344..472).filter(|&x| pixel(&pixels, x, y) != CLEAR_RGBA).count();
        assert!(
            painted(45) > painted(115) + 8,
            "the tab shape must splay wider at the top: {} vs {}",
            painted(45),
            painted(115)
        );
    }

    /// The selected wave is drawn last and in its own color, over the gray
    /// inactive rows; the horizontal padding outside the plot stays background.
    #[test]
    fn wavetable_pipeline_draws_the_selected_wave_over_the_inactive_rows() {
        let Some(renderer) = renderer() else { return };
        let pixels = renderer.render("wavetable");

        assert_near(pixel(&pixels, 20, 30), [13, 15, 23, 255], "padding column");

        let found = (16..240).any(|y| {
            (16..496).any(|x| {
                let p = pixel(&pixels, x, y);
                p[0].abs_diff(255) <= 2 && p[1].abs_diff(199) <= 3 && p[2].abs_diff(56) <= 4
            })
        });
        assert!(found, "no pixel reached the selected wave's color");
    }

    /// Inside the selection the waveform uses its active color; outside it uses
    /// the desaturated inactive color.
    #[test]
    fn waveform_pipeline_separates_the_selection_from_the_inactive_region() {
        let Some(renderer) = renderer() else { return };
        let pixels = renderer.render("waveform");

        let inside = pixel(&pixels, 250, 128);
        let outside = pixel(&pixels, 424, 128);
        assert!(
            inside[2] > outside[2] + 60 && inside[1] > outside[1] + 60,
            "selected audio should read brighter and bluer: {inside:?} vs {outside:?}"
        );
        assert!(
            outside[2].abs_diff(outside[0]) < 40,
            "unselected audio should stay near-neutral: {outside:?}"
        );
    }

    /// Waterfall mode must reach the hot end of the heat ramp where the
    /// synthetic formant peaks; EQ mode must fill below its curve and leave the
    /// area above it untouched.
    #[test]
    fn live_spectrogram_pipeline_renders_both_of_its_modes() {
        let Some(renderer) = renderer() else { return };
        let pixels = renderer.render("live-spectrogram");

        let hottest = (16..240)
            .flat_map(|y| (16..248).map(move |x| (x, y)))
            .map(|(x, y)| pixel(&pixels, x, y))
            .max_by_key(|p| luminance(*p))
            .expect("the waterfall widget has pixels");
        assert_near(hottest, [255, 229, 115, 255], "waterfall peak");

        assert_eq!(
            pixel(&pixels, 300, 40),
            CLEAR_RGBA,
            "nothing is drawn above the EQ curve"
        );
        let below = pixel(&pixels, 300, 220);
        assert!(
            below[1] > below[0] + 30 && below[2] > below[0] + 30,
            "below the EQ curve should carry the teal fill: {below:?}"
        );
    }
}
