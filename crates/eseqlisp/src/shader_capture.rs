//! Deterministic offscreen captures of the ported WGSL pipelines.
//!
//! Each scene drives exactly one of the pipelines in
//! [`crate::ui::wgpu_pipelines`] with fixed, procedurally generated inputs —
//! no fonts, no clock, no sample files — so two runs on one host produce
//! byte-identical PNGs and a later Metal capture of the same scene set can be
//! compared against them (`eseq-linux.25`). The renderer is headless: it draws
//! into an `Rgba8Unorm` texture and reads it back, so no window or surface
//! format is involved.

use std::collections::BTreeMap;
use std::path::Path;

use wgpu::util::DeviceExt;

use crate::ui::gpu_geometry::{
    ImageVertex, LiveSpectrogramInstance, PatchCableInstance, Vertex, WaveformInstance,
    WavetableInstance,
};
use crate::ui::wgpu_pipelines as pipelines;
use crate::ui::wgsl_shaders;
use crate::widget_render::WidgetInstance;

/// Schema of the emitted `manifest.json`. Bump when the scene set or the file
/// layout changes so an old capture cannot be mistaken for a current one.
pub const SCHEMA_VERSION: u32 = 1;

pub const WIDTH: u32 = 512;
pub const HEIGHT: u32 = 256;
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// One capture per ported pipeline, in the order the bead lists them.
pub const SCENES: &[&str] = &[
    "text",
    "proportional-text",
    "image",
    "patch-cable",
    "widget-surface",
    "wavetable",
    "waveform",
    "live-spectrogram",
];

/// A dark, non-neutral clear so a pipeline that writes nothing is obvious and
/// so alpha blending has something to blend against.
const CLEAR: wgpu::Color = wgpu::Color {
    r: 0.04,
    g: 0.05,
    b: 0.07,
    a: 1.0,
};

/// Pixel (top-left origin, y down) to normalized device coordinates.
fn ndc(x: f32, y: f32) -> [f32; 2] {
    [
        x / WIDTH as f32 * 2.0 - 1.0,
        1.0 - y / HEIGHT as f32 * 2.0,
    ]
}

// ── Procedural inputs ────────────────────────────────────────────────────

/// A 64×64 single-channel atlas of sixteen 16×16 cells, each holding an
/// antialiased disc whose radius grows with the cell index. It stands in for a
/// glyph atlas: the only thing the text pipelines read is `.r` coverage, and a
/// coverage ramp exercises both the nearest and the linear sampler.
fn glyph_atlas_pixels() -> Vec<u8> {
    let mut pixels = vec![0u8; 64 * 64];
    for cell in 0..16u32 {
        let (cx, cy) = (cell % 4, cell / 4);
        let radius = 2.0 + cell as f32 * 0.35;
        for y in 0..16u32 {
            for x in 0..16u32 {
                let dx = x as f32 + 0.5 - 8.0;
                let dy = y as f32 + 0.5 - 8.0;
                let d = (dx * dx + dy * dy).sqrt() - radius;
                let coverage = (0.5 - d).clamp(0.0, 1.0);
                pixels[((cy * 16 + y) * 64 + cx * 16 + x) as usize] = (coverage * 255.0) as u8;
            }
        }
    }
    pixels
}

/// A 64×64 RGBA checkerboard tinted by a diagonal gradient, so the image
/// pipeline's rotation and rounding are both visible against known content.
fn image_pixels() -> Vec<u8> {
    let mut pixels = vec![0u8; 64 * 64 * 4];
    for y in 0..64u32 {
        for x in 0..64u32 {
            let checker = ((x / 8) + (y / 8)) % 2 == 0;
            let t = (x + y) as f32 / 126.0;
            let base = if checker { 0.85 } else { 0.20 };
            let offset = ((y * 64 + x) * 4) as usize;
            pixels[offset] = (base * 255.0) as u8;
            pixels[offset + 1] = (base * t * 255.0) as u8;
            pixels[offset + 2] = (base * (1.0 - t) * 255.0) as u8;
            pixels[offset + 3] = 255;
        }
    }
    pixels
}

const WAVETABLE_FRAME_LEN: u32 = 256;
const WAVETABLE_WAVES: u32 = 4;

/// Four classic single-cycle shapes, so morphing between neighbours is visible.
fn wavetable_bank() -> Vec<f32> {
    let mut bank = Vec::with_capacity((WAVETABLE_FRAME_LEN * WAVETABLE_WAVES) as usize);
    for wave in 0..WAVETABLE_WAVES {
        for i in 0..WAVETABLE_FRAME_LEN {
            let phase = i as f32 / WAVETABLE_FRAME_LEN as f32;
            let value = match wave {
                0 => (phase * std::f32::consts::TAU).sin(),
                1 => phase * 2.0 - 1.0,
                2 => {
                    if phase < 0.5 {
                        1.0
                    } else {
                        -1.0
                    }
                }
                _ => 1.0 - (phase * 4.0 - 1.0).abs().min(1.0) * 2.0,
            };
            bank.push(value);
        }
    }
    bank
}

const WAVEFORM_BUCKETS: u32 = 256;

/// Min/max pairs from a decaying sine burst: amplitude sweeps from full scale
/// down to near silence, so the fill, the edge highlight and the minimum
/// thickness clamp are all exercised in one capture.
fn waveform_buckets() -> Vec<f32> {
    let mut data = Vec::with_capacity(WAVEFORM_BUCKETS as usize * 2);
    for i in 0..WAVEFORM_BUCKETS {
        let t = i as f32 / (WAVEFORM_BUCKETS - 1) as f32;
        let envelope = (1.0 - t).powf(1.6) * (0.35 + 0.65 * (t * 22.0).sin().abs());
        data.push(-envelope);
        data.push(envelope);
    }
    data
}

const SPECTROGRAM_BINS: u32 = 128;
const SPECTROGRAM_SLICES: u32 = 64;

/// A drifting formant peak over time: the waterfall rows sweep the peak up the
/// spectrum, so a row-ordering mistake shows as a discontinuity.
fn spectrogram_waterfall() -> Vec<f32> {
    let mut data = Vec::with_capacity((SPECTROGRAM_BINS * SPECTROGRAM_SLICES) as usize);
    for row in 0..SPECTROGRAM_SLICES {
        let center = 12.0 + row as f32 * 1.4;
        for bin in 0..SPECTROGRAM_BINS {
            let d = (bin as f32 - center) / 9.0;
            let peak = (-d * d).exp();
            let floor = 0.05 * (1.0 - bin as f32 / SPECTROGRAM_BINS as f32);
            data.push((peak + floor).min(1.0));
        }
    }
    data
}

/// A single smoothed spectrum row for the EQ mode, with two resonances.
fn spectrogram_smoothed() -> Vec<f32> {
    (0..SPECTROGRAM_BINS)
        .map(|bin| {
            let x = bin as f32 / SPECTROGRAM_BINS as f32;
            let low = (-((x - 0.18) / 0.10).powi(2)).exp();
            let high = 0.7 * (-((x - 0.62) / 0.06).powi(2)).exp();
            (0.12 + low + high).min(1.0)
        })
        .collect()
}

// ── Scene geometry ───────────────────────────────────────────────────────

/// Six vertices covering one axis-aligned quad, carrying uv, fg and bg.
fn text_quad(
    x: f32,
    y: f32,
    size: f32,
    uv_min: [f32; 2],
    uv_max: [f32; 2],
    fg: [f32; 4],
    bg: [f32; 4],
) -> [Vertex; 6] {
    let corners = [
        ([0.0, 0.0], [uv_min[0], uv_min[1]]),
        ([0.0, 1.0], [uv_min[0], uv_max[1]]),
        ([1.0, 0.0], [uv_max[0], uv_min[1]]),
        ([1.0, 0.0], [uv_max[0], uv_min[1]]),
        ([0.0, 1.0], [uv_min[0], uv_max[1]]),
        ([1.0, 1.0], [uv_max[0], uv_max[1]]),
    ];
    std::array::from_fn(|i| {
        let (corner, uv) = corners[i];
        Vertex {
            position: ndc(x + corner[0] * size, y + corner[1] * size),
            uv,
            fg,
            bg,
        }
    })
}

/// Eight glyph cells across the frame, each with its own foreground color and
/// a background that alternates so the monospace fragment's `mix(bg, fg, cov)`
/// is visibly doing both halves of the blend.
fn text_vertices() -> Vec<Vertex> {
    let mut vertices = Vec::new();
    for cell in 0..8u32 {
        let (cx, cy) = (cell % 4, cell / 4);
        let uv_min = [cx as f32 * 0.25, cy as f32 * 0.25];
        let uv_max = [uv_min[0] + 0.25, uv_min[1] + 0.25];
        let hue = cell as f32 / 7.0;
        let fg = [0.25 + 0.75 * hue, 0.90 - 0.55 * hue, 0.35 + 0.5 * (1.0 - hue), 1.0];
        let bg = if cell % 2 == 0 {
            [0.10, 0.12, 0.18, 1.0]
        } else {
            [0.24, 0.10, 0.12, 1.0]
        };
        vertices.extend(text_quad(
            16.0 + cell as f32 * 60.0,
            96.0,
            56.0,
            uv_min,
            uv_max,
            fg,
            bg,
        ));
    }
    vertices
}

fn image_quad(
    x: f32,
    y: f32,
    size: f32,
    opacity: f32,
    radius: f32,
    rotation: f32,
    clip_circle: f32,
) -> [ImageVertex; 6] {
    let half = size * 0.5;
    let corners = [
        [0.0, 0.0],
        [0.0, 1.0],
        [1.0, 0.0],
        [1.0, 0.0],
        [0.0, 1.0],
        [1.0, 1.0],
    ];
    std::array::from_fn(|i| {
        let corner = corners[i];
        ImageVertex {
            position: ndc(x + corner[0] * size, y + corner[1] * size),
            uv: corner,
            opacity,
            local_pos: [(corner[0] - 0.5) * size, (corner[1] - 0.5) * size],
            half_size: [half, half],
            radius,
            rotation,
            clip_circle,
        }
    })
}

/// Four quads: unclipped, rounded, circle-clipped, and rotated with partial
/// opacity — one per branch of the image fragment.
fn image_vertices() -> Vec<ImageVertex> {
    let mut vertices = Vec::new();
    vertices.extend(image_quad(24.0, 64.0, 96.0, 1.0, 0.0, 0.0, 0.0));
    vertices.extend(image_quad(144.0, 64.0, 96.0, 1.0, 24.0, 0.0, 0.0));
    vertices.extend(image_quad(264.0, 64.0, 96.0, 1.0, 0.0, 0.0, 1.0));
    vertices.extend(image_quad(384.0, 64.0, 96.0, 0.55, 16.0, 0.4, 0.0));
    vertices
}

fn patch_cable_instances() -> Vec<PatchCableInstance> {
    let cable = |bounds: [f32; 4],
                 start: [f32; 2],
                 control1: [f32; 2],
                 control2: [f32; 2],
                 end: [f32; 2],
                 color: [f32; 4],
                 radius_px: f32,
                 is_segmented: f32,
                 segment_y_px: f32| PatchCableInstance {
        ndc_min: ndc(bounds[0], bounds[1]),
        ndc_max: ndc(bounds[2], bounds[3]),
        bounds_min: [bounds[0], bounds[1]],
        bounds_max: [bounds[2], bounds[3]],
        start,
        control1,
        control2,
        end,
        color,
        radius_px,
        is_segmented,
        segment_y_px,
        corner_radius_px: 12.0,
    };
    vec![
        // Bezier: a long horizontal S-curve.
        cable(
            [16.0, 16.0, 240.0, 120.0],
            [32.0, 40.0],
            [140.0, 40.0],
            [120.0, 100.0],
            [224.0, 100.0],
            [0.20, 0.75, 1.00, 1.0],
            4.0,
            0.0,
            0.0,
        ),
        // Thicker bezier, warmer, so radius and color both vary between draws.
        cable(
            [16.0, 128.0, 240.0, 240.0],
            [32.0, 224.0],
            [180.0, 224.0],
            [80.0, 152.0],
            [224.0, 152.0],
            [1.00, 0.55, 0.18, 1.0],
            7.0,
            0.0,
            0.0,
        ),
        // Segmented: the orthogonal router with rounded corners.
        cable(
            [264.0, 16.0, 496.0, 240.0],
            [296.0, 48.0],
            [0.0, 0.0],
            [0.0, 0.0],
            [464.0, 208.0],
            [0.55, 1.00, 0.45, 1.0],
            5.0,
            1.0,
            128.0,
        ),
    ]
}

fn widget_instances() -> Vec<WidgetInstance> {
    let instance = |x: f32, y: f32, w: f32, h: f32, shape: f32, radius: f32, tint: [f32; 4]| {
        WidgetInstance {
            ndc_min: ndc(x, y + h),
            ndc_max: ndc(x + w, y),
            value_t: 0.5,
            orientation: 0.0,
            itime: 0.0,
            uniform_a: [shape, 0.0, 0.0, 0.0],
            uniform_b: [0.0; 4],
            uniform_c: [0.0, 0.0, 1.0, 1.0],
            uniform_d: [0.0; 4],
            color_a: tint,
            color_b: [0.85, 0.88, 0.95, 1.0],
            color_c: [1.0, 1.0, 1.0, 0.65],
            color_d: [0.02, 0.03, 0.06, 0.8],
            corner_radius: radius,
            pixel_aspect: w / h,
        }
    };
    vec![
        instance(24.0, 40.0, 128.0, 80.0, 0.0, 0.0, [0.24, 0.42, 0.78, 1.0]),
        instance(184.0, 40.0, 128.0, 80.0, 0.0, 0.35, [0.72, 0.28, 0.34, 1.0]),
        instance(344.0, 40.0, 128.0, 80.0, 1.0, 0.30, [0.26, 0.60, 0.42, 1.0]),
        instance(104.0, 148.0, 304.0, 72.0, 0.0, 0.55, [0.52, 0.46, 0.20, 1.0]),
    ]
}

fn wavetable_instance() -> WavetableInstance {
    WavetableInstance {
        ndc_min: ndc(16.0, 240.0),
        ndc_max: ndc(496.0, 16.0),
        widget_px_w: 480.0,
        widget_px_h: 224.0,
        frame_len: WAVETABLE_FRAME_LEN,
        set_base: 0,
        waves_in_set: WAVETABLE_WAVES,
        wave_pos: 1.6,
        // Mild on purpose: enough warp and fold that both terms are exercised,
        // little enough that the four base shapes stay recognisable when the
        // capture is judged by eye.
        warp: 0.12,
        fold: 0.08,
        domain: 0,
        selected_color: [1.00, 0.78, 0.22, 1.0],
        inactive_color: [0.45, 0.50, 0.58, 0.85],
        bg_color: [0.05, 0.06, 0.09, 1.0],
    }
}

fn waveform_instance() -> WaveformInstance {
    WaveformInstance {
        ndc_min: ndc(16.0, 240.0),
        ndc_max: ndc(496.0, 16.0),
        sample_start: 0.0,
        sample_end: 1.0,
        bucket_count: WAVEFORM_BUCKETS,
        aspect_ratio: 224.0 / 480.0,
        selection_start: 0.25,
        selection_end: 0.70,
        show_selection_start: 1,
        show_selection_end: 1,
        playhead_position: 0.45,
        show_playhead: 1,
        waveform_color: [0.36, 0.82, 1.00, 1.0],
        inactive_waveform_color: [0.30, 0.34, 0.40, 1.0],
        marker_color: [0.90, 0.90, 0.95, 1.0],
        active_marker_color: [1.00, 0.72, 0.20, 1.0],
        active_selection_start: 1,
        active_selection_end: 0,
        selection_color: [0.55, 0.70, 1.00, 1.0],
        bg_color: [0.05, 0.06, 0.09, 1.0],
        border_color: [0.35, 0.40, 0.50, 1.0],
    }
}

/// Waterfall on the left, EQ curve on the right, so both branches of the
/// fragment land in one capture.
fn live_spectrogram_instances() -> Vec<LiveSpectrogramInstance> {
    let instance = |x0: f32, x1: f32, mode: u32| LiveSpectrogramInstance {
        ndc_min: ndc(x0, 240.0),
        ndc_max: ndc(x1, 16.0),
        widget_px_w: x1 - x0,
        widget_px_h: 224.0,
        bins: SPECTROGRAM_BINS,
        time_slices: SPECTROGRAM_SLICES,
        write_head: 20,
        mode,
        freq_scale: 0,
        sample_rate: 48_000.0,
        display_hz: [40.0, 18_000.0],
        display_hz_padding: [0.0, 0.0],
        min_color: [0.04, 0.05, 0.16, 1.0],
        mid_color: [0.20, 0.55, 0.75, 1.0],
        max_color: [1.00, 0.90, 0.45, 1.0],
        eq_line_color: [0.40, 0.95, 0.80, 1.0],
        eq_fill_color: [0.16, 0.45, 0.42, 0.8],
        background_color: [0.05, 0.06, 0.09, 1.0],
    };
    vec![instance(16.0, 248.0, 0), instance(264.0, 496.0, 1)]
}

// ── Renderer ─────────────────────────────────────────────────────────────

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
        let widget_pipeline = pipelines::widget_pipeline(
            &self.device,
            "eseqlisp capture button surface",
            None,
            wgsl_shaders::BUTTON_SURFACE_WGSL,
            FORMAT,
        );
        let wavetable_pipeline = pipelines::wavetable_pipeline(&self.device, &storage1, FORMAT);
        let waveform_pipeline = pipelines::waveform_pipeline(&self.device, &storage1, FORMAT);
        let spectrogram_pipeline =
            pipelines::live_spectrogram_pipeline(&self.device, &storage2, FORMAT);

        let text_vertices = text_vertices();
        let text_buffer = self.instance_buffer(&text_vertices);
        let image_verts = image_vertices();
        let image_buffer = self.instance_buffer(&image_verts);
        let cables = patch_cable_instances();
        let cable_buffer = self.instance_buffer(&cables);
        let widgets = widget_instances();
        let widget_buffer = self.instance_buffer(&widgets);
        let wavetable = [wavetable_instance()];
        let wavetable_buffer = self.instance_buffer(&wavetable);
        let waveform = [waveform_instance()];
        let waveform_buffer = self.instance_buffer(&waveform);
        let spectrograms = live_spectrogram_instances();
        let spectrogram_buffer = self.instance_buffer(&spectrograms);

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
                "widget-surface" => {
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
                    pass.set_pipeline(&spectrogram_pipeline);
                    pass.set_bind_group(0, &spectrogram_group, &[]);
                    pass.set_vertex_buffer(0, spectrogram_buffer.slice(..));
                    pass.draw(0..6, 0..spectrograms.len() as u32);
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

/// RGBA at pixel (x, y), top-left origin.
pub fn pixel(pixels: &[u8], x: u32, y: u32) -> [u8; 4] {
    let offset = ((y * WIDTH + x) * 4) as usize;
    [
        pixels[offset],
        pixels[offset + 1],
        pixels[offset + 2],
        pixels[offset + 3],
    ]
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
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
