//! Portable application backend: winit window + input pump + wgpu renderer.
//!
//! This is the non-macOS counterpart of `ui/metal_backend.rs`'s `MetalBackend`.
//! It exposes the same inherent API the sequencer's event loop consumes, so the
//! app shell selects a backend with one cfg'd type alias. The frame flow is a
//! direct port of `MetalBackend::render_tiled`, minus the retained-scene and
//! compiled-widget-run caches (always the dynamic path; perf work is tracked
//! by eseq-linux.12).
//!
//! Rendering happens in two phases so wgpu lifetimes stay simple: a mutable
//! *plan* phase builds every vertex/instance buffer and resource bind group,
//! then an immutable *encode* phase replays the ordered draw list into one
//! render pass with per-draw scissors.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use wgpu::util::DeviceExt;
use winit::{
    dpi::LogicalSize,
    event::{
        ElementState, Event as WEvent, MouseButton as WMouseButton, TouchPhase, WindowEvent,
    },
    event_loop::{ControlFlow, EventLoop},
    keyboard::{Key, KeyCode as WinitKeyCode, NamedKey, PhysicalKey},
    platform::pump_events::EventLoopExtPumpEvents,
    window::{CursorIcon, Window},
};

use crate::audio::sample::get_registered_sample;
use crate::backend::{
    AUTOCOMPLETE_ANCHOR_GAP_PX, AUTOCOMPLETE_PANEL_BORDER_WIDTH_PX,
    AUTOCOMPLETE_PANEL_CORNER_RADIUS_PX, AUTOCOMPLETE_ROW_CORNER_RADIUS_PX,
    AUTOCOMPLETE_TEXT_CELL_SCALE, Backend, BackendError, BackendEvent, Color, RenderFrame,
    TiledRenderFrame, completion_panel_columns,
};
use crate::layout::TextMeasurer;
use crate::live_audio;
use crate::theme;
use crate::ui::glyph_atlas::{GlyphAtlas, ProportionalGlyphAtlas, SizedFontCache};
use crate::ui::gpu_adapter;
use crate::ui::gpu_scene::{self, PatchCableDrawInstance, PropTextLayoutCache, TextOffset};
use crate::ui::gpu_geometry::{
    LiveSpectrogramInstance, ScissorRect, Vertex, WaveformInstance, WavetableInstance,
};
use crate::ui::platform;
use crate::ui::wgpu_pipelines as pipelines;
use crate::ui::wgpu_frame_stats::{FrameSample, WgpuFrameStats};
use crate::ui::wgsl_shaders;
use crate::widget_render::{self, WidgetInstance, WidgetViewport};

use super::DEFAULT_MONOSPACE_FONT_SIZE_PT;

const MONOSPACE_FONT_NAME: &str = "JetBrainsMono-Regular";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TiledRenderStatus {
    Presented,
    NotPresented,
}

/// Lightweight TextMeasurer that delegates to `SizedFontCache` for font
/// metrics without needing a GPU atlas. Used by the layout engine.
///
/// The scale factor is shared with the backend through an atomic because the
/// measurer is created once at startup and handed off to the layout engine,
/// while Wayland only reports the window's real (possibly fractional) scale
/// after the surface maps to an output; the font cache rebuilds lazily on the
/// first measurement after a change.
pub(crate) struct PropTextMeasurer {
    fonts: std::cell::RefCell<SizedFontCache>,
    scale_bits: Arc<AtomicU64>,
    built_scale_bits: std::cell::Cell<u64>,
}

impl PropTextMeasurer {
    pub(crate) fn new(scale_bits: Arc<AtomicU64>) -> Option<Self> {
        let bits = scale_bits.load(Ordering::Relaxed);
        let fonts = SizedFontCache::new(f64::from_bits(bits))?;
        Some(Self {
            fonts: std::cell::RefCell::new(fonts),
            scale_bits,
            built_scale_bits: std::cell::Cell::new(bits),
        })
    }

    fn sync_scale(&self) {
        let bits = self.scale_bits.load(Ordering::Relaxed);
        if bits == self.built_scale_bits.get() {
            return;
        }
        if let Some(fonts) = SizedFontCache::new(f64::from_bits(bits)) {
            *self.fonts.borrow_mut() = fonts;
        }
        self.built_scale_bits.set(bits);
    }
}

impl TextMeasurer for PropTextMeasurer {
    fn measure_text_px(&self, text: &str, font_size: f32) -> f32 {
        if text.is_empty() {
            return 0.0;
        }
        self.sync_scale();
        let size_tenths = (font_size * 10.0).round() as u16;
        self.fonts.borrow_mut().measure_text(text, size_tenths)
    }

    fn line_height_px(&self, font_size: f32) -> f32 {
        self.sync_scale();
        let size_tenths = (font_size * 10.0).round() as u16;
        self.fonts.borrow_mut().line_height(size_tenths)
    }

    fn cap_height_px(&self, font_size: f32) -> f32 {
        self.sync_scale();
        let size_tenths = (font_size * 10.0).round() as u16;
        self.fonts.borrow_mut().cap_height(size_tenths)
    }
}

// ── Atlas wrappers ───────────────────────────────────────────────────────────

/// A CPU glyph atlas plus its R8 wgpu texture and cached bind groups. The
/// texture re-uploads in full whenever the CPU bitmap revision changes (the
/// bitmap is 1 MiB; changes stop once the glyph set stabilizes).
struct WgpuGlyphAtlas {
    atlas: GlyphAtlas,
    texture: wgpu::Texture,
    bind_group: wgpu::BindGroup,
    uploaded_revision: u64,
}

impl WgpuGlyphAtlas {
    fn new(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        sampler: &wgpu::Sampler,
        font_name: &str,
        font_size: f64,
    ) -> Option<Self> {
        let atlas = GlyphAtlas::new(font_name, font_size)?;
        let (texture, bind_group) = atlas_texture_and_group(
            device,
            layout,
            sampler,
            atlas.bitmap.width() as u32,
            atlas.bitmap.height() as u32,
        );
        Some(Self {
            atlas,
            texture,
            bind_group,
            uploaded_revision: 0,
        })
    }

    fn sync_texture(&mut self, queue: &wgpu::Queue) {
        let revision = self.atlas.bitmap.revision();
        if revision == self.uploaded_revision {
            return;
        }
        upload_r8_bitmap(
            queue,
            &self.texture,
            self.atlas.bitmap.pixels(),
            self.atlas.bitmap.width() as u32,
            self.atlas.bitmap.height() as u32,
        );
        self.uploaded_revision = revision;
    }
}

impl std::ops::Deref for WgpuGlyphAtlas {
    type Target = GlyphAtlas;
    fn deref(&self) -> &Self::Target {
        &self.atlas
    }
}

impl std::ops::DerefMut for WgpuGlyphAtlas {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.atlas
    }
}

struct WgpuPropGlyphAtlas {
    atlas: ProportionalGlyphAtlas,
    texture: wgpu::Texture,
    bind_group: wgpu::BindGroup,
    uploaded_revision: u64,
}

impl WgpuPropGlyphAtlas {
    fn new(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        sampler: &wgpu::Sampler,
        scale: f64,
    ) -> Option<Self> {
        let atlas = ProportionalGlyphAtlas::new(scale)?;
        let (texture, bind_group) = atlas_texture_and_group(
            device,
            layout,
            sampler,
            atlas.bitmap.width() as u32,
            atlas.bitmap.height() as u32,
        );
        Some(Self {
            atlas,
            texture,
            bind_group,
            uploaded_revision: 0,
        })
    }

    fn sync_texture(&mut self, queue: &wgpu::Queue) {
        let revision = self.atlas.bitmap.revision();
        if revision == self.uploaded_revision {
            return;
        }
        upload_r8_bitmap(
            queue,
            &self.texture,
            self.atlas.bitmap.pixels(),
            self.atlas.bitmap.width() as u32,
            self.atlas.bitmap.height() as u32,
        );
        self.uploaded_revision = revision;
    }
}

fn atlas_texture_and_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    sampler: &wgpu::Sampler,
    width: u32,
    height: u32,
) -> (wgpu::Texture, wgpu::BindGroup) {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("eseqlisp glyph atlas"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::R8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let view = texture.create_view(&Default::default());
    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("eseqlisp glyph atlas bind group"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
        ],
    });
    (texture, bind_group)
}

fn upload_r8_bitmap(
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    pixels: &[u8],
    width: u32,
    height: u32,
) {
    queue.write_texture(
        texture.as_image_copy(),
        pixels,
        wgpu::ImageDataLayout {
            offset: 0,
            bytes_per_row: Some(width),
            rows_per_image: Some(height),
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
}

// ── GPU resources ────────────────────────────────────────────────────────────

struct WaveformGpuResource {
    bucket_count: u32,
    bind_group: wgpu::BindGroup,
}

struct WavetableGpuResource {
    revision: u64,
    bind_group: wgpu::BindGroup,
}

struct LiveSpectrogramGpuResource {
    revision: u64,
    bins: u32,
    time_slices: u32,
    write_head: u32,
    sample_rate: f32,
    bind_group: wgpu::BindGroup,
}

struct ImageTextureResource {
    bind_group: wgpu::BindGroup,
    width: u32,
    height: u32,
    modified: Option<std::time::SystemTime>,
}

struct ImageDecodeJob {
    path: PathBuf,
    modified: Option<std::time::SystemTime>,
}

struct DecodedImageData {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
}

struct ImageDecodeResult {
    path: PathBuf,
    modified: Option<std::time::SystemTime>,
    image: Option<DecodedImageData>,
}

struct ImageRotationState {
    src: String,
    angle: f32,
    speed: f32,
    time_seconds: f32,
}

fn decode_image_job(job: ImageDecodeJob) -> ImageDecodeResult {
    let path = job.path;
    let modified = job.modified;
    let image = decode_image_path(&path);
    ImageDecodeResult {
        path,
        modified,
        image,
    }
}

fn decode_image_path(path: &PathBuf) -> Option<DecodedImageData> {
    let mut decoded = image::ImageReader::open(path).ok()?.decode().ok()?;
    let max_dimension = decoded.width().max(decoded.height());
    if max_dimension > 640 {
        let scale = 640.0 / max_dimension as f32;
        let width = (decoded.width() as f32 * scale).round().max(1.0) as u32;
        let height = (decoded.height() as f32 * scale).round().max(1.0) as u32;
        decoded = decoded.resize(width, height, image::imageops::FilterType::Triangle);
    }
    let rgba = decoded.to_rgba8();
    let (width, height) = rgba.dimensions();
    if width == 0 || height == 0 {
        return None;
    }
    Some(DecodedImageData {
        width,
        height,
        rgba: rgba.into_raw(),
    })
}

// ── Draw plan ────────────────────────────────────────────────────────────────

/// Which pipeline (and its fixed bind group) a planned draw uses. Resolved at
/// encode time against the caches the plan phase populated.
enum PipelineRef {
    /// Monospace text pipeline; `zoomed` selects the text atlas over the
    /// widget-grid atlas.
    Text { zoomed: bool },
    Prop,
    Cable,
    Widget(String),
    Image(PathBuf),
    Waveform((String, u32)),
    Wavetable(String),
    SpectrogramWaterfall(String),
    SpectrogramEq(String),
}

enum DrawKind {
    /// `draw(0..count, 0..1)` — vertex-stepped geometry.
    Vertices(u32),
    /// `draw(0..6, 0..count)` — instance-stepped quads.
    Instanced(u32),
}

struct DrawCmd {
    scissor: ScissorRect,
    pipeline: PipelineRef,
    buffer: wgpu::Buffer,
    kind: DrawKind,
}

struct GpuState {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    text_pipeline: wgpu::RenderPipeline,
    prop_pipeline: wgpu::RenderPipeline,
    image_pipeline: wgpu::RenderPipeline,
    cable_pipeline: wgpu::RenderPipeline,
    waveform_pipeline: wgpu::RenderPipeline,
    wavetable_pipeline: wgpu::RenderPipeline,
    spectrogram_pipelines: pipelines::LiveSpectrogramPipelines,
    widget_pipelines: HashMap<String, wgpu::RenderPipeline>,
    atlas_layout: wgpu::BindGroupLayout,
    storage1_layout: wgpu::BindGroupLayout,
    storage2_layout: wgpu::BindGroupLayout,
    nearest_sampler: wgpu::Sampler,
    linear_sampler: wgpu::Sampler,
}

impl GpuState {
    fn compile_widget_pipeline(
        &self,
        label: &str,
        vertex_source: Option<&str>,
        fragment_source: &str,
    ) -> Option<wgpu::RenderPipeline> {
        self.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let pipeline = pipelines::widget_pipeline(
            &self.device,
            label,
            vertex_source,
            fragment_source,
            self.config.format,
        );
        match pollster::block_on(self.device.pop_error_scope()) {
            None => Some(pipeline),
            Some(error) => {
                eprintln!("wgpu widget shader compile failed for {label}: {error}");
                None
            }
        }
    }
}

// ── Backend ──────────────────────────────────────────────────────────────────

pub struct WgpuAppBackend {
    // Winit
    event_loop: Option<EventLoop<()>>,
    window: Option<Arc<Window>>,
    pending: VecDeque<Event>,
    pending_drag: Option<Event>,
    pending_move: Option<Event>,
    pending_magnify: VecDeque<(f64, (f32, f32))>,
    pending_scroll: VecDeque<((f32, f32), (f32, f32))>,
    pending_file_drops: VecDeque<Vec<PathBuf>>,
    pending_resize: bool,
    /// Scale factor delivered by `WindowEvent::ScaleFactorChanged`, applied
    /// after the winit pump returns (atlas rebuilds need `&mut self`).
    pending_scale_factor: Option<f64>,
    /// Scale factor the current glyph atlases were built at.
    atlas_scale_factor: f64,
    /// Live scale factor (f64 bits) shared with `PropTextMeasurer`.
    prop_text_scale_bits: Arc<AtomicU64>,
    close_requested: bool,
    suppress_scroll_until: Option<Instant>,
    modifiers: KeyModifiers,
    pressed_mouse_button: Option<MouseButton>,
    cursor_cell: (u16, u16),
    cursor_pos: (f32, f32),
    last_precise_mouse: Option<(f32, f32)>,
    // GPU
    gpu: Option<GpuState>,
    // Atlases
    atlas: Option<WgpuGlyphAtlas>,
    text_atlas: Option<WgpuGlyphAtlas>,
    text_atlas_zoom: f32,
    prop_atlas: Option<WgpuPropGlyphAtlas>,
    prop_text_layout_cache: PropTextLayoutCache,
    // Resources
    waveform_buffers: HashMap<(String, u32), WaveformGpuResource>,
    wavetable_buffers: HashMap<String, WavetableGpuResource>,
    live_spectrogram_buffers: HashMap<String, LiveSpectrogramGpuResource>,
    image_textures: HashMap<PathBuf, ImageTextureResource>,
    image_decode_tx: mpsc::Sender<ImageDecodeJob>,
    image_decode_rx: mpsc::Receiver<ImageDecodeResult>,
    image_decode_in_flight: HashSet<PathBuf>,
    image_rotation_states: HashMap<u64, ImageRotationState>,
    // SDF widget pipelines from user content
    sdf_widget_pipeline_sources: HashMap<String, String>,
    sdf_widget_pipeline_registry_generation: u64,
    agent_instrument_stub_animation_visible: bool,
    last_window_bg: Option<Color>,
    /// Per-frame profiling aggregate; see `ui/wgpu_frame_stats.rs`. Also holds
    /// the "was the adapter already reported" latch, because both are
    /// once-per-process startup/diagnostic state.
    frame_stats: WgpuFrameStats,
    reported_adapter: bool,
    start_time: Instant,
    initial_window_size: LogicalSize<f64>,
    initial_window_visible: bool,
    monospace_font_size_pt: f64,
}

impl WgpuAppBackend {
    pub fn new() -> Result<Self, BackendError> {
        Self::new_with_size(1350, 900)
    }

    pub fn new_with_size(width: u32, height: u32) -> Result<Self, BackendError> {
        Self::new_with_size_and_font_size(width, height, DEFAULT_MONOSPACE_FONT_SIZE_PT)
    }

    pub fn new_with_size_and_font_size(
        width: u32,
        height: u32,
        monospace_font_size_pt: f64,
    ) -> Result<Self, BackendError> {
        if !monospace_font_size_pt.is_finite() || monospace_font_size_pt <= 0.0 {
            return Err(BackendError::MetalError);
        }
        let (image_decode_tx, image_decode_job_rx) = mpsc::channel::<ImageDecodeJob>();
        let (image_decode_result_tx, image_decode_rx) = mpsc::channel::<ImageDecodeResult>();
        std::thread::Builder::new()
            .name("eseqlisp-image-decoder".to_string())
            .spawn(move || {
                while let Ok(job) = image_decode_job_rx.recv() {
                    let decoded = decode_image_job(job);
                    if image_decode_result_tx.send(decoded).is_err() {
                        break;
                    }
                }
            })
            .map_err(|_| BackendError::MetalError)?;
        Ok(Self {
            event_loop: None,
            window: None,
            pending: VecDeque::new(),
            pending_drag: None,
            pending_move: None,
            pending_magnify: VecDeque::new(),
            pending_scroll: VecDeque::new(),
            pending_file_drops: VecDeque::new(),
            pending_resize: false,
            pending_scale_factor: None,
            atlas_scale_factor: 1.0,
            prop_text_scale_bits: Arc::new(AtomicU64::new(1.0f64.to_bits())),
            close_requested: false,
            suppress_scroll_until: None,
            modifiers: KeyModifiers::NONE,
            pressed_mouse_button: None,
            cursor_cell: (0, 0),
            cursor_pos: (0.0, 0.0),
            last_precise_mouse: None,
            gpu: None,
            atlas: None,
            text_atlas: None,
            text_atlas_zoom: 0.0,
            prop_atlas: None,
            prop_text_layout_cache: PropTextLayoutCache::new(),
            waveform_buffers: HashMap::new(),
            wavetable_buffers: HashMap::new(),
            live_spectrogram_buffers: HashMap::new(),
            image_textures: HashMap::new(),
            image_decode_tx,
            image_decode_rx,
            image_decode_in_flight: HashSet::new(),
            image_rotation_states: HashMap::new(),
            sdf_widget_pipeline_sources: HashMap::new(),
            sdf_widget_pipeline_registry_generation: 0,
            agent_instrument_stub_animation_visible: false,
            last_window_bg: None,
            frame_stats: WgpuFrameStats::new(),
            reported_adapter: false,
            start_time: Instant::now(),
            initial_window_size: LogicalSize::new(width as f64, height as f64),
            initial_window_visible: true,
            monospace_font_size_pt,
        })
    }

    fn elapsed_time_seconds(&self) -> f32 {
        self.start_time.elapsed().as_secs_f32()
    }

    pub fn time_seconds(&self) -> f32 {
        self.elapsed_time_seconds()
    }

    pub fn agent_instrument_stub_animation_visible(&self) -> bool {
        self.agent_instrument_stub_animation_visible
    }

    pub fn take_last_precise_mouse(&mut self) -> Option<(f32, f32)> {
        self.last_precise_mouse.take()
    }

    pub fn take_pending_magnify(&mut self) -> Option<(f64, (f32, f32))> {
        self.pending_magnify.pop_front()
    }

    pub fn take_pending_scroll(&mut self) -> Option<((f32, f32), (f32, f32))> {
        self.pending_scroll.pop_front()
    }

    pub fn set_widget_cursor(&self, cursor: widget_render::WidgetCursor) {
        let Some(window) = &self.window else {
            return;
        };
        let icon = match cursor {
            widget_render::WidgetCursor::Default => CursorIcon::Default,
            widget_render::WidgetCursor::EwResize => CursorIcon::EwResize,
            widget_render::WidgetCursor::NsResize => CursorIcon::NsResize,
            widget_render::WidgetCursor::Grab => CursorIcon::Grab,
            widget_render::WidgetCursor::DragCopy => CursorIcon::Copy,
            widget_render::WidgetCursor::DragNotAllowed => CursorIcon::NotAllowed,
        };
        window.set_cursor_icon(icon);
    }

    /// Create a TextMeasurer for the proportional font. Called once after
    /// atlas initialization to hand off to the layout engine.
    pub fn create_text_measurer(&self) -> Option<Box<dyn TextMeasurer>> {
        let measurer = PropTextMeasurer::new(self.prop_text_scale_bits.clone())?;
        Some(Box::new(measurer))
    }

    pub fn cell_dimensions(&self) -> (f32, f32) {
        self.atlas
            .as_ref()
            .map(|a| (a.cell_w.max(1) as f32, a.cell_h.max(1) as f32))
            .unwrap_or((8.0, 16.0))
    }

    pub fn sync_text_zoom(&mut self, zoom: f32) -> Option<(f32, f32)> {
        if !zoom.is_finite() || zoom <= 0.0 {
            return self
                .text_atlas
                .as_ref()
                .map(|a| (a.cell_w.max(1) as f32, a.cell_h.max(1) as f32));
        }

        let needs_rebuild =
            self.text_atlas.is_none() || (self.text_atlas_zoom - zoom).abs() > 0.001;
        if needs_rebuild {
            let scale = self
                .window
                .as_ref()
                .map(|w| w.scale_factor())
                .unwrap_or(1.0);
            let gpu = self.gpu.as_ref()?;
            let next = WgpuGlyphAtlas::new(
                &gpu.device,
                &gpu.atlas_layout,
                &gpu.nearest_sampler,
                MONOSPACE_FONT_NAME,
                self.monospace_font_size_pt * zoom as f64 * scale,
            )?;
            self.text_atlas = Some(next);
            self.text_atlas_zoom = zoom;
        }

        self.text_atlas
            .as_ref()
            .map(|a| (a.cell_w.max(1) as f32, a.cell_h.max(1) as f32))
    }

    /// Rebuild scale-dependent resources after `WindowEvent::ScaleFactorChanged`.
    /// Wayland reports scale 1.0 at window creation and delivers the real
    /// (possibly fractional, via wp-fractional-scale-v1) factor only once the
    /// surface maps to an output, so the setup-time atlases can be built at
    /// the wrong size. Also fires when the window moves between monitors.
    fn apply_scale_factor(&mut self, scale: f64) {
        if !scale.is_finite() || scale <= 0.0 {
            return;
        }
        if (scale - self.atlas_scale_factor).abs() < 1e-3 {
            return;
        }
        eprintln!(
            "eseq: window scale factor {} -> {}",
            self.atlas_scale_factor, scale
        );
        self.atlas_scale_factor = scale;
        self.prop_text_scale_bits
            .store(scale.to_bits(), Ordering::Relaxed);
        widget_render::set_ui_scale_factor(scale as f32);
        if let Some(gpu) = self.gpu.as_ref() {
            self.atlas = WgpuGlyphAtlas::new(
                &gpu.device,
                &gpu.atlas_layout,
                &gpu.nearest_sampler,
                MONOSPACE_FONT_NAME,
                self.monospace_font_size_pt * scale,
            );
            let text_zoom = if self.text_atlas_zoom.is_finite() && self.text_atlas_zoom > 0.0 {
                self.text_atlas_zoom as f64
            } else {
                1.0
            };
            self.text_atlas = WgpuGlyphAtlas::new(
                &gpu.device,
                &gpu.atlas_layout,
                &gpu.nearest_sampler,
                MONOSPACE_FONT_NAME,
                self.monospace_font_size_pt * text_zoom * scale,
            );
            self.prop_atlas = WgpuPropGlyphAtlas::new(
                &gpu.device,
                &gpu.atlas_layout,
                &gpu.linear_sampler,
                scale,
            );
            self.prop_text_layout_cache = PropTextLayoutCache::new();
        }
        // The cell grid changed size: force a relayout against the new grid.
        if let Some(window) = &self.window {
            let size = window.inner_size();
            self.pending
                .push_back(Event::Resize(size.width as u16, size.height as u16));
            window.request_redraw();
        }
    }

    /// Recompile widget pipelines whose SDF registry sources changed (user
    /// content compiled to WGSL by the shader DSL).
    fn compile_pending_sdf_pipelines(&mut self) -> bool {
        use crate::widget_render::sdf_widget;
        let generation = sdf_widget::sdf_widget_registry_generation();
        if self.sdf_widget_pipeline_registry_generation == generation {
            return false;
        }
        let Some(gpu) = self.gpu.as_mut() else {
            return false;
        };
        let mut changed = false;
        for (name, shader_src) in sdf_widget::sdf_widget_shader_sources() {
            if self
                .sdf_widget_pipeline_sources
                .get(&name)
                .is_some_and(|current| current == &shader_src)
            {
                continue;
            }
            match gpu.compile_widget_pipeline(&name, None, &shader_src) {
                Some(pipeline) => {
                    gpu.widget_pipelines.insert(name.clone(), pipeline);
                    changed = true;
                }
                None => {
                    gpu.widget_pipelines.remove(&name);
                }
            }
            self.sdf_widget_pipeline_sources.insert(name, shader_src);
        }
        self.sdf_widget_pipeline_registry_generation = generation;
        changed
    }

    pub fn poll_editable_shader_overrides(&mut self) -> bool {
        self.compile_pending_sdf_pipelines()
    }

    fn sync_window_theme(&mut self) {
        let bg = theme::BG();
        if self.last_window_bg == Some(bg) {
            return;
        }
        if let Some(window) = &self.window {
            platform::sync_window_theme(window, bg);
            self.last_window_bg = Some(bg);
        }
    }

    fn drain_decoded_images(&mut self, mut upload_budget: usize) {
        while upload_budget > 0 {
            let Ok(result) = self.image_decode_rx.try_recv() else {
                break;
            };
            self.image_decode_in_flight.remove(&result.path);
            let Some(decoded) = result.image else {
                continue;
            };
            let Some(gpu) = self.gpu.as_ref() else {
                continue;
            };
            let texture = gpu.device.create_texture_with_data(
                &gpu.queue,
                &wgpu::TextureDescriptor {
                    label: Some("eseqlisp image"),
                    size: wgpu::Extent3d {
                        width: decoded.width,
                        height: decoded.height,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                },
                wgpu::util::TextureDataOrder::LayerMajor,
                &decoded.rgba,
            );
            let view = texture.create_view(&Default::default());
            let bind_group = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("eseqlisp image bind group"),
                layout: &gpu.atlas_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&gpu.linear_sampler),
                    },
                ],
            });
            self.image_textures.insert(
                result.path,
                ImageTextureResource {
                    bind_group,
                    width: decoded.width,
                    height: decoded.height,
                    modified: result.modified,
                },
            );
            upload_budget -= 1;
        }
    }

    fn image_path_and_modified(src: &str) -> Option<(PathBuf, Option<std::time::SystemTime>)> {
        if src.is_empty() {
            return None;
        }
        let mut path = PathBuf::from(src);
        if !path.is_absolute() {
            path = std::env::current_dir().ok()?.join(path);
        }
        let metadata = std::fs::metadata(&path).ok()?;
        let modified = metadata.modified().ok();
        Some((path, modified))
    }

    fn ensure_image_texture(&mut self, src: &str, load_budget: &mut usize) -> Option<PathBuf> {
        let (path, modified) = Self::image_path_and_modified(src)?;
        let should_reload = self
            .image_textures
            .get(&path)
            .map(|cached| cached.modified != modified)
            .unwrap_or(true);
        if should_reload {
            if self.image_decode_in_flight.contains(&path) || *load_budget == 0 {
                return None;
            }
            *load_budget = load_budget.saturating_sub(1);
            if self.image_decode_in_flight.insert(path.clone())
                && self
                    .image_decode_tx
                    .send(ImageDecodeJob {
                        path: path.clone(),
                        modified,
                    })
                    .is_err()
            {
                self.image_decode_in_flight.remove(&path);
            }
            return None;
        }
        self.image_textures.contains_key(&path).then_some(path)
    }

    fn effective_image_rotation(
        &mut self,
        image: &widget_render::GpuImagePrimitive,
        time_seconds: f32,
    ) -> f32 {
        const SEEK_SNAP_THRESHOLD_RADIANS: f32 = 1.0;

        let base_angle = image.rotation;
        let mut angle = base_angle;
        if let Some(state) = self.image_rotation_states.get(&image.widget_id)
            && state.src == image.src
        {
            let dt = (time_seconds - state.time_seconds).max(0.0);
            let predicted = state.angle + state.speed * dt;
            if gpu_scene::angular_distance(predicted, base_angle) < SEEK_SNAP_THRESHOLD_RADIANS {
                angle = predicted;
            }
        }

        self.image_rotation_states.insert(
            image.widget_id,
            ImageRotationState {
                src: image.src.clone(),
                angle,
                speed: image.rotation_speed,
                time_seconds,
            },
        );
        angle
    }

    fn ensure_waveform_resource(&mut self, sample_key: &str, samples_per_bucket: u32) -> bool {
        let key = (sample_key.to_string(), samples_per_bucket);
        if self.waveform_buffers.contains_key(&key) {
            return true;
        }
        let Some(gpu) = self.gpu.as_ref() else {
            return false;
        };
        let Some(sample) = get_registered_sample(sample_key) else {
            return false;
        };
        let Some(level) = sample
            .levels()
            .iter()
            .find(|level| level.samples_per_bucket as u32 == samples_per_bucket)
        else {
            return false;
        };
        let flattened = level.flattened_pairs();
        let buffer = gpu
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("eseqlisp waveform buckets"),
                contents: bytemuck::cast_slice(&flattened),
                usage: wgpu::BufferUsages::STORAGE,
            });
        let bind_group = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("eseqlisp waveform bind group"),
            layout: &gpu.storage1_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: buffer.as_entire_binding(),
            }],
        });
        self.waveform_buffers.insert(
            key,
            WaveformGpuResource {
                bucket_count: level.buckets.len() as u32,
                bind_group,
            },
        );
        true
    }

    fn ensure_wavetable_resource(
        &mut self,
        bank_key: &str,
        revision: u64,
        data: &Arc<Vec<f32>>,
    ) -> bool {
        if let Some(resource) = self.wavetable_buffers.get(bank_key)
            && resource.revision == revision
        {
            return true;
        }
        let Some(gpu) = self.gpu.as_ref() else {
            return false;
        };
        let buffer = gpu
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("eseqlisp wavetable bank"),
                contents: bytemuck::cast_slice(data.as_slice()),
                usage: wgpu::BufferUsages::STORAGE,
            });
        let bind_group = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("eseqlisp wavetable bind group"),
            layout: &gpu.storage1_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: buffer.as_entire_binding(),
            }],
        });
        self.wavetable_buffers.insert(
            bank_key.to_string(),
            WavetableGpuResource {
                revision,
                bind_group,
            },
        );
        true
    }

    fn ensure_live_spectrogram_resource(&mut self, data_key: &str) -> bool {
        let Some(frame) = live_audio::spectrogram_frame(data_key) else {
            return false;
        };
        let needs_upload = self
            .live_spectrogram_buffers
            .get(data_key)
            .map(|resource| {
                resource.revision != frame.revision
                    || resource.bins != frame.bins
                    || resource.time_slices != frame.time_slices
            })
            .unwrap_or(true);
        if needs_upload {
            let Some(gpu) = self.gpu.as_ref() else {
                return false;
            };
            let waterfall = gpu
                .device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("eseqlisp spectrogram waterfall"),
                    contents: bytemuck::cast_slice(frame.waterfall.as_slice()),
                    usage: wgpu::BufferUsages::STORAGE,
                });
            let smoothed = gpu
                .device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("eseqlisp spectrogram smoothed"),
                    contents: bytemuck::cast_slice(frame.smoothed.as_slice()),
                    usage: wgpu::BufferUsages::STORAGE,
                });
            let bind_group = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("eseqlisp spectrogram bind group"),
                layout: &gpu.storage2_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: waterfall.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: smoothed.as_entire_binding(),
                    },
                ],
            });
            self.live_spectrogram_buffers.insert(
                data_key.to_string(),
                LiveSpectrogramGpuResource {
                    revision: frame.revision,
                    bins: frame.bins,
                    time_slices: frame.time_slices,
                    write_head: frame.write_head,
                    sample_rate: frame.sample_rate,
                    bind_group,
                },
            );
        } else if let Some(resource) = self.live_spectrogram_buffers.get_mut(data_key) {
            resource.write_head = frame.write_head;
            resource.sample_rate = frame.sample_rate;
        }
        true
    }

    // ── Draw planning ────────────────────────────────────────────────────────

    fn vertex_buffer<T: bytemuck::Pod>(device: &wgpu::Device, data: &[T]) -> wgpu::Buffer {
        device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("eseqlisp frame geometry"),
            contents: bytemuck::cast_slice(data),
            usage: wgpu::BufferUsages::VERTEX,
        })
    }

    fn plan_vertices(
        plan: &mut Vec<DrawCmd>,
        device: &wgpu::Device,
        scissor: ScissorRect,
        pipeline: PipelineRef,
        verts: &[Vertex],
    ) {
        if verts.is_empty() {
            return;
        }
        plan.push(DrawCmd {
            scissor,
            pipeline,
            buffer: Self::vertex_buffer(device, verts),
            kind: DrawKind::Vertices(verts.len() as u32),
        });
    }

    fn plan_widget_instances(
        plan: &mut Vec<DrawCmd>,
        device: &wgpu::Device,
        scissor: ScissorRect,
        widget_type: &str,
        instances: &[WidgetInstance],
    ) {
        if instances.is_empty() {
            return;
        }
        plan.push(DrawCmd {
            scissor,
            pipeline: PipelineRef::Widget(widget_type.to_string()),
            buffer: Self::vertex_buffer(device, instances),
            kind: DrawKind::Instanced(instances.len() as u32),
        });
    }

    fn plan_patch_cables(
        plan: &mut Vec<DrawCmd>,
        device: &wgpu::Device,
        cables: &[PatchCableDrawInstance],
    ) {
        let mut run_start = 0;
        while run_start < cables.len() {
            let clip = cables[run_start].clip;
            let mut run_end = run_start + 1;
            while run_end < cables.len() && cables[run_end].clip == clip {
                run_end += 1;
            }
            let run: Vec<_> = cables[run_start..run_end]
                .iter()
                .map(|draw| draw.instance)
                .collect();
            plan.push(DrawCmd {
                scissor: clip,
                pipeline: PipelineRef::Cable,
                buffer: Self::vertex_buffer(device, &run),
                kind: DrawKind::Instanced(run.len() as u32),
            });
            run_start = run_end;
        }
    }

    /// The per-segment dynamic dispatcher: mirrors the Metal
    /// `draw_dynamic_segment_all` draw order exactly.
    #[allow(clippy::too_many_arguments)]
    fn plan_dynamic_segment(
        &mut self,
        plan: &mut Vec<DrawCmd>,
        seg_scissor: ScissorRect,
        seg_prims: &[widget_render::GpuPrimitive],
        cell_w: f32,
        cell_h: f32,
        vp_w: f32,
        vp_h: f32,
        image_load_budget: &mut usize,
        render_time_seconds: f32,
    ) {
        let z_layers = gpu_scene::z_ordered_primitive_layers(seg_prims);
        for seg_prims in &z_layers {
            let (bg_runs, fg_runs) = gpu_scene::partition_widget_instance_runs(seg_prims);
            {
                let gpu = self.gpu.as_ref().expect("gpu initialized");
                for (widget_type, instances) in &bg_runs {
                    if gpu.widget_pipelines.contains_key(widget_type) {
                        Self::plan_widget_instances(
                            plan,
                            &gpu.device,
                            seg_scissor,
                            widget_type,
                            instances,
                        );
                    }
                }
            }

            // Images
            let images = gpu_scene::collect_image_primitives(seg_prims);
            for image in &images {
                if !gpu_scene::image_intersects_scissor(image, seg_scissor, cell_w, cell_h) {
                    continue;
                }
                let Some(path) = self.ensure_image_texture(&image.src, image_load_budget) else {
                    continue;
                };
                let Some(resource) = self.image_textures.get(&path) else {
                    continue;
                };
                let (image_w, image_h) = (resource.width, resource.height);
                let rotation = self.effective_image_rotation(image, render_time_seconds);
                let verts = gpu_scene::image_vertices(
                    image, image_w, image_h, cell_w, cell_h, vp_w, vp_h, rotation,
                );
                if verts.is_empty() {
                    continue;
                }
                let gpu = self.gpu.as_ref().expect("gpu initialized");
                plan.push(DrawCmd {
                    scissor: seg_scissor,
                    pipeline: PipelineRef::Image(path),
                    buffer: Self::vertex_buffer(&gpu.device, &verts),
                    kind: DrawKind::Vertices(verts.len() as u32),
                });
            }

            // Solid rects/quads/triangles + monospace glyph runs
            {
                let Some(atlas) = self.atlas.as_mut() else {
                    return;
                };
                let prim_quads =
                    gpu_scene::build_widget_primitive_quads(seg_prims, atlas, vp_w, vp_h);
                let gpu = self.gpu.as_ref().expect("gpu initialized");
                Self::plan_vertices(
                    plan,
                    &gpu.device,
                    seg_scissor,
                    PipelineRef::Text { zoomed: false },
                    &prim_quads,
                );
            }

            // Patch cables
            let cables = gpu_scene::collect_patch_cable_primitives(
                seg_prims,
                seg_scissor,
                cell_w,
                cell_h,
                vp_w,
                vp_h,
            );
            {
                let gpu = self.gpu.as_ref().expect("gpu initialized");
                Self::plan_patch_cables(plan, &gpu.device, &cables);
            }

            // Waveforms
            for primitive in gpu_scene::collect_waveform_primitives(seg_prims) {
                if !self.ensure_waveform_resource(&primitive.sample_key, primitive.samples_per_bucket)
                {
                    continue;
                }
                let key = (primitive.sample_key.clone(), primitive.samples_per_bucket);
                let bucket_count = self.waveform_buffers[&key].bucket_count;
                let ndc_min = [
                    (primitive.rect.col * cell_w / vp_w) * 2.0 - 1.0,
                    1.0 - ((primitive.rect.row + primitive.rect.height) * cell_h / vp_h) * 2.0,
                ];
                let ndc_max = [
                    ((primitive.rect.col + primitive.rect.width) * cell_w / vp_w) * 2.0 - 1.0,
                    1.0 - (primitive.rect.row * cell_h / vp_h) * 2.0,
                ];
                let instance = WaveformInstance {
                    ndc_min,
                    ndc_max,
                    sample_start: primitive.sample_start,
                    sample_end: primitive.sample_end,
                    bucket_count: primitive.bucket_count.min(bucket_count),
                    aspect_ratio: (primitive.rect.width * cell_w
                        / (primitive.rect.height * cell_h))
                        .max(0.0001),
                    selection_start: primitive.selection_start,
                    selection_end: primitive.selection_end,
                    show_selection_start: primitive.show_selection_start as i32,
                    show_selection_end: primitive.show_selection_end as i32,
                    playhead_position: primitive.playhead_position,
                    show_playhead: primitive.show_playhead as i32,
                    waveform_color: primitive.waveform_color.to_rgba(),
                    inactive_waveform_color: primitive.inactive_waveform_color.to_rgba(),
                    marker_color: primitive.marker_color.to_rgba(),
                    active_marker_color: primitive.active_marker_color.to_rgba(),
                    active_selection_start: primitive.active_selection_start as i32,
                    active_selection_end: primitive.active_selection_end as i32,
                    selection_color: primitive.selection_color.to_rgba(),
                    bg_color: theme::BG().to_rgba(),
                    border_color: theme::BORDER_INACTIVE().to_rgba(),
                };
                let gpu = self.gpu.as_ref().expect("gpu initialized");
                plan.push(DrawCmd {
                    scissor: seg_scissor,
                    pipeline: PipelineRef::Waveform(key),
                    buffer: Self::vertex_buffer(&gpu.device, std::slice::from_ref(&instance)),
                    kind: DrawKind::Instanced(1),
                });
            }

            // Wavetables
            for primitive in gpu_scene::collect_wavetable_primitives(seg_prims) {
                let expected = (primitive.set_base + primitive.waves_in_set) as usize
                    * primitive.frame_len as usize;
                if primitive.frame_len < 2
                    || primitive.waves_in_set == 0
                    || primitive.data.len() < expected
                {
                    continue;
                }
                if !self.ensure_wavetable_resource(
                    &primitive.bank_key,
                    primitive.data_revision,
                    &primitive.data,
                ) {
                    continue;
                }
                let ndc_min = [
                    (primitive.rect.col * cell_w / vp_w) * 2.0 - 1.0,
                    1.0 - ((primitive.rect.row + primitive.rect.height) * cell_h / vp_h) * 2.0,
                ];
                let ndc_max = [
                    ((primitive.rect.col + primitive.rect.width) * cell_w / vp_w) * 2.0 - 1.0,
                    1.0 - (primitive.rect.row * cell_h / vp_h) * 2.0,
                ];
                let instance = WavetableInstance {
                    ndc_min,
                    ndc_max,
                    widget_px_w: primitive.rect.width * cell_w,
                    widget_px_h: primitive.rect.height * cell_h,
                    frame_len: primitive.frame_len,
                    set_base: primitive.set_base,
                    waves_in_set: primitive.waves_in_set,
                    wave_pos: primitive.wave_pos,
                    warp: primitive.warp,
                    fold: primitive.fold,
                    domain: primitive.domain,
                    selected_color: primitive.selected_color.to_rgba(),
                    inactive_color: primitive.inactive_color.to_rgba(),
                    bg_color: primitive.bg_color.to_rgba(),
                };
                let gpu = self.gpu.as_ref().expect("gpu initialized");
                plan.push(DrawCmd {
                    scissor: seg_scissor,
                    pipeline: PipelineRef::Wavetable(primitive.bank_key.clone()),
                    buffer: Self::vertex_buffer(&gpu.device, std::slice::from_ref(&instance)),
                    kind: DrawKind::Instanced(1),
                });
            }

            // Live spectrograms
            for primitive in gpu_scene::collect_live_spectrogram_primitives(seg_prims) {
                if !self.ensure_live_spectrogram_resource(&primitive.data_key) {
                    continue;
                }
                let resource = &self.live_spectrogram_buffers[&primitive.data_key];
                if resource.bins < 2 || resource.time_slices == 0 {
                    continue;
                }
                let ndc_min = [
                    (primitive.rect.col * cell_w / vp_w) * 2.0 - 1.0,
                    1.0 - ((primitive.rect.row + primitive.rect.height) * cell_h / vp_h) * 2.0,
                ];
                let ndc_max = [
                    ((primitive.rect.col + primitive.rect.width) * cell_w / vp_w) * 2.0 - 1.0,
                    1.0 - (primitive.rect.row * cell_h / vp_h) * 2.0,
                ];
                let instance = LiveSpectrogramInstance {
                    ndc_min,
                    ndc_max,
                    widget_px_w: primitive.rect.width * cell_w,
                    widget_px_h: primitive.rect.height * cell_h,
                    bins: resource.bins,
                    time_slices: resource.time_slices,
                    write_head: resource.write_head,
                    mode: primitive.mode,
                    freq_scale: primitive.freq_scale,
                    sample_rate: resource.sample_rate,
                    display_hz: [primitive.min_hz, primitive.max_hz],
                    display_hz_padding: [0.0, 0.0],
                    min_color: primitive.min_color.to_rgba(),
                    mid_color: primitive.mid_color.to_rgba(),
                    max_color: primitive.max_color.to_rgba(),
                    eq_line_color: primitive.eq_line_color.to_rgba(),
                    eq_fill_color: primitive.eq_fill_color.to_rgba(),
                    background_color: primitive.background_color.to_rgba(),
                };
                let pipeline = if primitive.mode == 1 {
                    PipelineRef::SpectrogramEq(primitive.data_key.clone())
                } else {
                    PipelineRef::SpectrogramWaterfall(primitive.data_key.clone())
                };
                let gpu = self.gpu.as_ref().expect("gpu initialized");
                plan.push(DrawCmd {
                    scissor: seg_scissor,
                    pipeline,
                    buffer: Self::vertex_buffer(&gpu.device, std::slice::from_ref(&instance)),
                    kind: DrawKind::Instanced(1),
                });
            }

            {
                let gpu = self.gpu.as_ref().expect("gpu initialized");
                for (widget_type, instances) in &fg_runs {
                    if gpu.widget_pipelines.contains_key(widget_type) {
                        Self::plan_widget_instances(
                            plan,
                            &gpu.device,
                            seg_scissor,
                            widget_type,
                            instances,
                        );
                    }
                }

                let circle_quads =
                    gpu_scene::build_circle_quads(seg_prims, cell_w, cell_h, vp_w, vp_h);
                Self::plan_vertices(
                    plan,
                    &gpu.device,
                    seg_scissor,
                    PipelineRef::Text { zoomed: false },
                    &circle_quads,
                );

                let foreground_rect_quads =
                    gpu_scene::build_foreground_rect_quads(seg_prims, cell_w, cell_h, vp_w, vp_h);
                Self::plan_vertices(
                    plan,
                    &gpu.device,
                    seg_scissor,
                    PipelineRef::Text { zoomed: false },
                    &foreground_rect_quads,
                );
            }

            // Proportional text
            if let Some(prop_atlas) = self.prop_atlas.as_mut() {
                let prop_verts = gpu_scene::build_proportional_text_quads(
                    seg_prims,
                    &mut prop_atlas.atlas,
                    &mut self.prop_text_layout_cache,
                    cell_w,
                    cell_h,
                    vp_w,
                    vp_h,
                );
                let gpu = self.gpu.as_ref().expect("gpu initialized");
                Self::plan_vertices(plan, &gpu.device, seg_scissor, PipelineRef::Prop, &prop_verts);
            }
        }
    }

    /// Render a tiled frame with per-tile scissor clipping. Direct port of the
    /// Metal `render_tiled` flow.
    pub fn render_tiled(
        &mut self,
        tiled: &TiledRenderFrame,
    ) -> Result<TiledRenderStatus, BackendError> {
        widget_render::sdf_widget::set_sdf_time_seconds(self.elapsed_time_seconds());
        self.compile_pending_sdf_pipelines();
        self.agent_instrument_stub_animation_visible = false;
        let render_time_seconds = self.elapsed_time_seconds();
        self.sync_window_theme();
        let mut image_load_budget = 1usize;
        self.drain_decoded_images(2);

        let Some(window) = self.window.clone() else {
            return Ok(TiledRenderStatus::NotPresented);
        };
        let size = window.inner_size();
        if size.width == 0 || size.height == 0 {
            return Ok(TiledRenderStatus::NotPresented);
        }
        {
            let Some(gpu) = self.gpu.as_mut() else {
                return Ok(TiledRenderStatus::NotPresented);
            };
            let max_dim = gpu.device.limits().max_texture_dimension_2d.max(1);
            let target = (size.width.clamp(1, max_dim), size.height.clamp(1, max_dim));
            if (gpu.config.width, gpu.config.height) != target {
                gpu.config.width = target.0;
                gpu.config.height = target.1;
                gpu.surface.configure(&gpu.device, &gpu.config);
            }
        }
        let (cell_w, cell_h) = match self.atlas.as_ref() {
            Some(atlas) => (atlas.cell_w as f32, atlas.cell_h as f32),
            None => return Ok(TiledRenderStatus::NotPresented),
        };
        let (vp_w, vp_h) = (size.width as f32, size.height as f32);
        // Raw-px design constants (tile corner radii, autocomplete radii) are
        // authored against the 2x macOS reference; scale them to this window.
        let ui_px_scale = widget_render::ui_px_scale();
        let to_rgba = |c: Color| [c.r, c.g, c.b, c.a];
        let has_multiple_tiles = tiled.tiles.len() > 1;
        let mut mod_patch_ports = Vec::new();
        let mut global_overlay_prims = Vec::new();
        let mut deferred_inspect_overlay: Option<(f32, f32, f32, f32, Color, Color)> = None;
        self.prop_text_layout_cache.begin_frame();

        let mut plan: Vec<DrawCmd> = Vec::new();
        let full_scissor = gpu_scene::full_viewport_scissor(vp_w, vp_h);

        // Profiling window for this frame. `widget_scene` accumulates across
        // tiles inside the loop below; the phase boundaries are the same ones
        // the aggregate reports. Kept unconditional and branch-free: the two
        // `Instant::now()` pairs per frame are far below the noise floor of the
        // work they bracket, and a disabled `WgpuFrameStats` drops the sample.
        let scroll_offsets = tiled.tiles.iter().fold((0.0f32, 0.0f32), |acc, tile| {
            (
                acc.0 + tile.frame.widget_layout_scroll_left,
                acc.1 + tile.frame.text_scroll_top as f32 + tile.frame.widget_scroll_top,
            )
        });
        let scrolled = self.frame_stats.note_scroll_offsets(scroll_offsets);
        let mut sample = FrameSample {
            scrolled,
            ..FrameSample::default()
        };
        let plan_start = Instant::now();

        // ── Per-tile planning ────────────────────────────────────────────────
        for tile in &tiled.tiles {
            let frame_left_px = tile.rect.col * cell_w;
            let frame_top_px = tile.rect.row * cell_h;
            let frame_width_px = tile.rect.width * cell_w;
            let frame_height_px = tile.rect.height * cell_h;
            let tile_left_px = tile.body_rect.col * cell_w;
            let tile_top_px = tile.body_rect.row * cell_h;
            let tile_width_px = tile.body_rect.width * cell_w;
            let tile_height_px = tile.body_rect.height * cell_h;
            let border_inset_px = if tile.show_border {
                widget_render::ui_design_px(tile.border_width_px)
                    .max(0.0)
                    .min(tile_width_px * 0.5)
                    .min(tile_height_px * 0.5)
            } else {
                0.0
            };
            let content_left_px = tile_left_px + border_inset_px;
            let content_top_px = tile_top_px + border_inset_px;
            let content_right_px =
                (tile_left_px + tile_width_px - border_inset_px).max(content_left_px);
            let content_bottom_px = if tile.show_status {
                (tile_top_px + tile_height_px - border_inset_px - cell_h).max(content_top_px)
            } else {
                (tile_top_px + tile_height_px - border_inset_px).max(content_top_px)
            };
            let content_col = content_left_px / cell_w;
            let content_row = content_top_px / cell_h;

            let tile_scissor_left = frame_left_px.floor().max(0.0);
            let tile_scissor_top = frame_top_px.floor().max(0.0);
            let tile_scissor_right = (frame_left_px + frame_width_px)
                .ceil()
                .max(tile_scissor_left);
            let tile_scissor_bottom = (frame_top_px + frame_height_px)
                .ceil()
                .max(tile_scissor_top);
            let tile_scissor = ScissorRect {
                x: tile_scissor_left.min(u32::MAX as f32) as u32,
                y: tile_scissor_top.min(u32::MAX as f32) as u32,
                width: (tile_scissor_right - tile_scissor_left).min(u32::MAX as f32) as u32,
                height: (tile_scissor_bottom - tile_scissor_top).min(u32::MAX as f32) as u32,
            };

            let scissor_left = content_left_px.floor().max(0.0);
            let scissor_top = content_top_px.floor().max(0.0);
            let scissor_right = content_right_px.ceil().max(scissor_left);
            let scissor_bottom = content_bottom_px.ceil().max(scissor_top);
            let content_scissor = ScissorRect {
                x: scissor_left.min(u32::MAX as f32) as u32,
                y: scissor_top.min(u32::MAX as f32) as u32,
                width: (scissor_right - scissor_left).min(u32::MAX as f32) as u32,
                height: (scissor_bottom - scissor_top).min(u32::MAX as f32) as u32,
            };

            let tile_bg = tile
                .background_color_name
                .as_deref()
                .and_then(theme::named_color)
                .or(tile.background_color)
                .unwrap_or(theme::BG());

            // Tile chrome background
            {
                let gpu = self.gpu.as_ref().expect("gpu initialized");
                let chrome = gpu
                    .widget_pipelines
                    .contains_key("tile-chrome")
                    .then(|| {
                        gpu_scene::tile_chrome_instance_px(
                            frame_left_px,
                            frame_top_px,
                            frame_width_px,
                            frame_height_px,
                            tile.border_radius_px * ui_px_scale,
                            0.0,
                            tile_bg,
                            Color::rgba(0.0, 0.0, 0.0, 0.0),
                            vp_w,
                            vp_h,
                        )
                    })
                    .flatten();
                if let Some(instance) = chrome {
                    Self::plan_widget_instances(
                        &mut plan,
                        &gpu.device,
                        tile_scissor,
                        "tile-chrome",
                        std::slice::from_ref(&instance),
                    );
                } else {
                    let mut tile_bg_verts = Vec::new();
                    gpu_scene::push_rounded_rect_fill_px(
                        &mut tile_bg_verts,
                        frame_left_px,
                        frame_top_px,
                        frame_width_px,
                        frame_height_px,
                        tile.border_radius_px * ui_px_scale,
                        tile_bg,
                        vp_w,
                        vp_h,
                    );
                    Self::plan_vertices(
                        &mut plan,
                        &gpu.device,
                        tile_scissor,
                        PipelineRef::Text { zoomed: false },
                        &tile_bg_verts,
                    );
                }
            }

            // ── Text content (shifted by horizontal scroll) ──────────────────
            let hscroll = tile.frame.widget_scroll_left;
            let offset = TextOffset {
                origin_col: content_col,
                origin_row: content_row,
                scroll_left: hscroll,
            };
            {
                let zoomed = self.text_atlas.is_some();
                let text_atlas = self.text_atlas.as_mut().or(self.atlas.as_mut());
                if let Some(text_atlas) = text_atlas {
                    let text_verts = gpu_scene::build_tile_text_quads(
                        &tile.frame,
                        &mut text_atlas.atlas,
                        cell_w,
                        cell_h,
                        vp_w,
                        vp_h,
                        offset,
                        tile_bg,
                    );
                    let gpu = self.gpu.as_ref().expect("gpu initialized");
                    Self::plan_vertices(
                        &mut plan,
                        &gpu.device,
                        content_scissor,
                        PipelineRef::Text { zoomed },
                        &text_verts,
                    );
                }
            }

            // ── Widget primitives ────────────────────────────────────────────
            if let Some(ref layout) = tile.frame.widget_layout {
                if gpu_scene::layout_contains_agent_instrument_stub_animation(layout) {
                    self.agent_instrument_stub_animation_visible = true;
                }
                let time_seconds = self.elapsed_time_seconds();
                let inner_rows_exact = ((content_bottom_px - content_top_px) / cell_h).max(0.0);
                let inner_rows = inner_rows_exact.floor() as u16;
                let text_scroll = tile.frame.text_scroll_top as f32;
                let widget_scroll = tile.frame.widget_scroll_top;
                let combined_scroll = text_scroll + widget_scroll;

                let viewport = WidgetViewport {
                    cell_w,
                    cell_h,
                    vp_w,
                    vp_h,
                    time_seconds,
                    focused_widget_id: tile.frame.focused_widget_id,
                    focused_branch: false,
                    overlay_viewport_bottom: vp_h / cell_h - content_row,
                    scroll_top: combined_scroll,
                    scroll_left: tile.frame.widget_layout_scroll_left,
                    inherited_hover: false,
                };
                let widget_col_off = content_col - tile.frame.widget_layout_scroll_left;
                let widget_row_off = content_row - combined_scroll;
                gpu_scene::collect_mod_patch_ports(
                    layout,
                    widget_col_off,
                    widget_row_off,
                    cell_w,
                    cell_h,
                    content_scissor,
                    &mut mod_patch_ports,
                );
                let content_width_cells = ((content_right_px - content_left_px) / cell_w).max(0.0);
                let fill_extra_cols = (content_width_cells - layout.rect.width).max(0.0);

                let widget_scene_start = Instant::now();
                let (primitives, overlay_prims) = widget_render::collect_gpu_primitives(
                    layout,
                    viewport,
                    combined_scroll,
                    inner_rows,
                );
                sample.widget_primitives += primitives.len() as u64;
                let offset_prims: Vec<_> = primitives
                    .into_iter()
                    .map(|p| {
                        gpu_scene::offset_primitive(
                            gpu_scene::extend_right_edge_primitive(
                                p,
                                layout.rect.width,
                                fill_extra_cols,
                                cell_w,
                                vp_w,
                            ),
                            widget_col_off,
                            widget_row_off,
                            cell_w,
                            cell_h,
                            vp_w,
                            vp_h,
                        )
                    })
                    .collect();
                if gpu_scene::contains_agent_instrument_stub_animation(&offset_prims) {
                    self.agent_instrument_stub_animation_visible = true;
                }

                let segments = gpu_scene::split_prim_segment_ranges(
                    &offset_prims,
                    content_scissor,
                    cell_w,
                    cell_h,
                );
                // Everything above is scene rebuild: collection, offsetting,
                // and segment splitting. Buffer creation below belongs to the
                // plan total, not to the scene, so the two costs stay separable.
                sample.widget_scene += widget_scene_start.elapsed();
                for (seg_scissor, seg_range) in &segments {
                    self.plan_dynamic_segment(
                        &mut plan,
                        *seg_scissor,
                        &offset_prims[seg_range.clone()],
                        cell_w,
                        cell_h,
                        vp_w,
                        vp_h,
                        &mut image_load_budget,
                        render_time_seconds,
                    );
                }

                // ── Overlay collection (dropdown menus, etc.) ────────────────
                if !overlay_prims.is_empty() {
                    let overlay_col_off = content_col;
                    let overlay_row_off = content_row;
                    let offset_overlay: Vec<_> = overlay_prims
                        .into_iter()
                        .map(|p| {
                            gpu_scene::offset_primitive(
                                p,
                                overlay_col_off,
                                overlay_row_off,
                                cell_w,
                                cell_h,
                                vp_w,
                                vp_h,
                            )
                        })
                        .collect();
                    if gpu_scene::contains_agent_instrument_stub_animation(&offset_overlay) {
                        self.agent_instrument_stub_animation_visible = true;
                    }
                    global_overlay_prims.extend(offset_overlay);
                }
            }

            if let Some(overlay) = tile.inspect_overlay {
                let text_scroll = tile.frame.text_scroll_top as f32;
                let overlay_x = (content_col + overlay.rect.col
                    - tile.frame.widget_layout_scroll_left)
                    * cell_w;
                let overlay_y = (content_row + overlay.rect.row
                    - text_scroll
                    - tile.frame.widget_scroll_top)
                    * cell_h;
                let overlay_w = overlay.rect.width * cell_w;
                let overlay_h = overlay.rect.height * cell_h;
                if overlay_w > 0.0 && overlay_h > 0.0 {
                    let modal_overlay_active = widget_render::topmost_overlay()
                        .is_some_and(|entry| entry.kind == widget_render::OverlayKind::Modal);
                    if modal_overlay_active {
                        deferred_inspect_overlay = Some((
                            overlay_x,
                            overlay_y,
                            overlay_w,
                            overlay_h,
                            overlay.fill,
                            overlay.border,
                        ));
                    } else {
                        let mut inspect_verts = Vec::new();
                        gpu_scene::push_rect_px(
                            &mut inspect_verts,
                            overlay_x,
                            overlay_y,
                            overlay_w,
                            overlay_h,
                            overlay.fill,
                            vp_w,
                            vp_h,
                        );
                        gpu_scene::push_rounded_rect_border_px(
                            &mut inspect_verts,
                            overlay_x,
                            overlay_y,
                            overlay_w,
                            overlay_h,
                            1.5,
                            3.0,
                            overlay.border,
                            vp_w,
                            vp_h,
                        );
                        let gpu = self.gpu.as_ref().expect("gpu initialized");
                        Self::plan_vertices(
                            &mut plan,
                            &gpu.device,
                            content_scissor,
                            PipelineRef::Text { zoomed: false },
                            &inspect_verts,
                        );
                    }
                }
            }

            // ── Per-tile status bar ──────────────────────────────────────────
            if tile.show_status {
                let status_left_px = content_left_px;
                let status_right_px = content_right_px;
                let status_top_px =
                    (tile_top_px + tile_height_px - border_inset_px - cell_h).max(content_top_px);
                let status_bottom_px =
                    (tile_top_px + tile_height_px - border_inset_px).max(status_top_px);
                let status_scissor = ScissorRect {
                    x: status_left_px.floor().max(0.0) as u32,
                    y: status_top_px.floor().max(0.0) as u32,
                    width: (status_right_px.ceil() - status_left_px.floor()).max(0.0) as u32,
                    height: (status_bottom_px.ceil() - status_top_px.floor()).max(0.0) as u32,
                };
                if let Some(atlas) = self.atlas.as_mut() {
                    let status_verts = gpu_scene::build_status_row_quads(
                        &tile.frame.status_cells,
                        &mut atlas.atlas,
                        status_left_px,
                        status_top_px,
                        status_right_px,
                        status_bottom_px,
                        cell_w,
                        cell_h,
                        vp_w,
                        vp_h,
                    );
                    let gpu = self.gpu.as_ref().expect("gpu initialized");
                    Self::plan_vertices(
                        &mut plan,
                        &gpu.device,
                        status_scissor,
                        PipelineRef::Text { zoomed: false },
                        &status_verts,
                    );
                }
            }

            // ── Thin pixel borders (on top of content) ───────────────────────
            if has_multiple_tiles && tile.show_border {
                let border_color = if tile.is_active {
                    theme::BORDER_ACTIVE()
                } else {
                    tile_bg
                };
                let gpu = self.gpu.as_ref().expect("gpu initialized");
                let chrome = gpu
                    .widget_pipelines
                    .contains_key("tile-chrome")
                    .then(|| {
                        gpu_scene::tile_chrome_instance_px(
                            frame_left_px,
                            frame_top_px,
                            frame_width_px,
                            frame_height_px,
                            tile.border_radius_px * ui_px_scale,
                            tile.border_width_px * ui_px_scale,
                            Color::rgba(0.0, 0.0, 0.0, 0.0),
                            border_color,
                            vp_w,
                            vp_h,
                        )
                    })
                    .flatten();
                if let Some(instance) = chrome {
                    Self::plan_widget_instances(
                        &mut plan,
                        &gpu.device,
                        tile_scissor,
                        "tile-chrome",
                        std::slice::from_ref(&instance),
                    );
                } else {
                    let mut bverts = Vec::new();
                    gpu_scene::push_rounded_rect_border_px(
                        &mut bverts,
                        frame_left_px,
                        frame_top_px,
                        frame_width_px,
                        frame_height_px,
                        tile.border_width_px * ui_px_scale,
                        tile.border_radius_px * ui_px_scale,
                        border_color,
                        vp_w,
                        vp_h,
                    );
                    Self::plan_vertices(
                        &mut plan,
                        &gpu.device,
                        tile_scissor,
                        PipelineRef::Text { zoomed: false },
                        &bverts,
                    );
                }
            }
        }

        // ── Tile tabs ────────────────────────────────────────────────────────
        let mut tab_instances = Vec::new();
        let mut tab_text_prims = Vec::new();
        for tile in &tiled.tiles {
            if tile.tabs.is_empty() {
                continue;
            }
            let Some(first_tab) = tile.tabs.first() else {
                continue;
            };
            let Some(last_tab) = tile.tabs.last() else {
                continue;
            };
            let group_left = first_tab.rect.col;
            let group_top = first_tab.rect.row;
            let group_right = last_tab.rect.col + last_tab.rect.width;
            let group_width = (group_right - group_left).max(0.0);
            let group_height = first_tab.rect.height;
            let group_height_px = group_height * cell_h;
            let group_inset_y = 0.0;
            let group_visual_top = group_top + group_inset_y / cell_h.max(1.0);
            let group_visual_height_px = (group_height_px - group_inset_y * 2.0).max(1.0);
            let group_visual_height = group_visual_height_px / cell_h.max(1.0);
            let group_bg = theme::BUFFER_TAB_BAR_BG();
            let selected_tab_bg = theme::BUFFER_TAB_SELECTED_BG();
            gpu_scene::push_tile_tab_instance_cells(
                &mut tab_instances,
                group_left,
                group_visual_top,
                group_width,
                group_visual_height,
                group_bg,
                Color::rgba(0.0, 0.0, 0.0, 0.0),
                Color::rgba(1.0, 1.0, 1.0, 0.04),
                Color::rgba(0.0, 0.0, 0.0, 0.10),
                0.0,
                0.95,
                cell_w,
                cell_h,
                vp_w,
                vp_h,
            );
            for tab in &tile.tabs {
                if tab.selected {
                    let selected_inset_px = 1.0;
                    let selected_x = tab.rect.col + selected_inset_px / cell_w.max(1.0);
                    let selected_y = group_visual_top + selected_inset_px / cell_h.max(1.0);
                    let selected_w =
                        (tab.rect.width - selected_inset_px * 2.0 / cell_w.max(1.0)).max(0.1);
                    let selected_h =
                        (group_visual_height - selected_inset_px * 2.0 / cell_h.max(1.0)).max(0.1);
                    gpu_scene::push_tile_tab_instance_cells(
                        &mut tab_instances,
                        selected_x,
                        selected_y,
                        selected_w,
                        selected_h,
                        selected_tab_bg,
                        theme::BUFFER_TAB_SELECTED_BORDER(),
                        theme::BUFFER_TAB_SELECTED_HIGHLIGHT(),
                        theme::BUFFER_TAB_SELECTED_SHADOW(),
                        1.0,
                        0.95,
                        cell_w,
                        cell_h,
                        vp_w,
                        vp_h,
                    );
                }
                let fg = if tab.selected {
                    theme::BUFFER_TAB_SELECTED_FG()
                } else {
                    theme::BUFFER_TAB_FG()
                };
                tab_text_prims.push(widget_render::GpuPrimitive::ProportionalText(
                    widget_render::GpuProportionalTextPrimitive {
                        row: tab.label_rect.row + (tab.label_rect.height - 1.0) * 0.5,
                        col: tab.label_rect.col,
                        align_width: tab.label_rect.width.max(0.0),
                        h_align: 0.5,
                        text: tab.label.clone(),
                        font_size: 10.5,
                        scale: 1.0,
                        fg,
                        bg: if tab.selected {
                            selected_tab_bg
                        } else {
                            group_bg
                        },
                    },
                ));
                if tab.close_visible
                    && let Some(close_rect) = tab.close_rect
                {
                    tab_text_prims.push(widget_render::GpuPrimitive::ProportionalText(
                        widget_render::GpuProportionalTextPrimitive {
                            row: close_rect.row + (close_rect.height - 1.0) * 0.5 - 0.08,
                            col: close_rect.col,
                            align_width: close_rect.width.max(0.0),
                            h_align: 0.5,
                            text: "×".to_string(),
                            font_size: 14.0,
                            scale: 1.0,
                            fg,
                            bg: if tab.selected {
                                selected_tab_bg
                            } else {
                                group_bg
                            },
                        },
                    ));
                }
            }
        }
        {
            let gpu = self.gpu.as_ref().expect("gpu initialized");
            let tab_pipeline_key = if gpu.widget_pipelines.contains_key("tile-tab") {
                Some("tile-tab")
            } else if gpu.widget_pipelines.contains_key("box") {
                Some("box")
            } else {
                None
            };
            if let Some(key) = tab_pipeline_key {
                Self::plan_widget_instances(&mut plan, &gpu.device, full_scissor, key, &tab_instances);
            }
        }
        if let Some(prop_atlas) = self.prop_atlas.as_mut() {
            let prop_verts = gpu_scene::build_proportional_text_quads(
                &tab_text_prims,
                &mut prop_atlas.atlas,
                &mut self.prop_text_layout_cache,
                cell_w,
                cell_h,
                vp_w,
                vp_h,
            );
            let gpu = self.gpu.as_ref().expect("gpu initialized");
            Self::plan_vertices(&mut plan, &gpu.device, full_scissor, PipelineRef::Prop, &prop_verts);
        }

        // ── Global patch cables (no tile scissor) ────────────────────────────
        if !mod_patch_ports.is_empty() {
            let cursor_px = (self.cursor_pos.0 * cell_w, self.cursor_pos.1 * cell_h);
            let cables =
                gpu_scene::build_mod_patch_cables(&mod_patch_ports, vp_w, vp_h, cursor_px);
            let highlight =
                gpu_scene::build_mod_patch_drag_highlight(&mod_patch_ports, cursor_px, vp_w, vp_h);
            let gpu = self.gpu.as_ref().expect("gpu initialized");
            Self::plan_patch_cables(&mut plan, &gpu.device, &cables);
            if let Some((highlight_verts, highlight_clip)) = highlight
                && !highlight_verts.is_empty()
            {
                Self::plan_vertices(
                    &mut plan,
                    &gpu.device,
                    highlight_clip,
                    PipelineRef::Text { zoomed: false },
                    &highlight_verts,
                );
            }
        }

        // ── Global overlay pass (dropdown menus, etc.) ───────────────────────
        if !global_overlay_prims.is_empty() {
            let overlay_prims = std::mem::take(&mut global_overlay_prims);
            let segments = gpu_scene::split_prim_segment_ranges(
                &overlay_prims,
                full_scissor,
                cell_w,
                cell_h,
            );
            for (seg_scissor, seg_range) in &segments {
                self.plan_dynamic_segment(
                    &mut plan,
                    *seg_scissor,
                    &overlay_prims[seg_range.clone()],
                    cell_w,
                    cell_h,
                    vp_w,
                    vp_h,
                    &mut image_load_budget,
                    render_time_seconds,
                );
            }
        }

        // ── Deferred inspect highlight (modal-hosted hover) ──────────────────
        if let Some((overlay_x, overlay_y, overlay_w, overlay_h, fill, border)) =
            deferred_inspect_overlay
        {
            let mut inspect_verts = Vec::new();
            gpu_scene::push_rect_px(
                &mut inspect_verts,
                overlay_x,
                overlay_y,
                overlay_w,
                overlay_h,
                fill,
                vp_w,
                vp_h,
            );
            gpu_scene::push_rounded_rect_border_px(
                &mut inspect_verts,
                overlay_x,
                overlay_y,
                overlay_w,
                overlay_h,
                1.5,
                3.0,
                border,
                vp_w,
                vp_h,
            );
            let gpu = self.gpu.as_ref().expect("gpu initialized");
            Self::plan_vertices(
                &mut plan,
                &gpu.device,
                full_scissor,
                PipelineRef::Text { zoomed: false },
                &inspect_verts,
            );
        }

        // ── Completion popup (on top of everything) ──────────────────────────
        if let Some(comp) = &tiled.completion
            && let Some(tile) = tiled.tiles.iter().find(|t| t.is_active)
        {
            let col_off = tile.body_rect.col.round() as usize;
            let row_off = tile.body_rect.row.round() as usize;
            let sel_bg = theme::COMP_SELECTED_BG();
            let pop_cell_w = (cell_w
                * comp.text_cell_width_scale.max(0.001)
                * AUTOCOMPLETE_TEXT_CELL_SCALE)
                .max(1.0);
            let pop_cell_h = (cell_h
                * comp.text_cell_height_scale.max(0.001)
                * AUTOCOMPLETE_TEXT_CELL_SCALE)
                .max(1.0);
            let anchor_x_px =
                (col_off as f32 + comp.anchor.1 as f32 * comp.text_cell_width_scale) * cell_w;
            let anchor_top_px =
                (row_off as f32 + comp.anchor.0 as f32 * comp.text_cell_height_scale) * cell_h;
            let anchor_bottom_px = anchor_top_px + cell_h * comp.text_cell_height_scale.max(0.001);
            let popup_col = (anchor_x_px / pop_cell_w).floor().max(0.0) as usize;
            let anchor_row = (anchor_top_px / pop_cell_h).floor().max(0.0) as usize;
            let popup_row = ((anchor_bottom_px + AUTOCOMPLETE_ANCHOR_GAP_PX * ui_px_scale)
                / pop_cell_h)
                .ceil()
                .max(0.0) as usize;
            let total_cols = (vp_w / pop_cell_w).floor().max(1.0) as usize;
            let total_rows = (vp_h / pop_cell_h).floor().max(1.0) as usize;
            let label_w = comp
                .entries
                .iter()
                .map(|e| e.label.len())
                .max()
                .unwrap_or(0)
                .max(12)
                .min(34);
            let panel_columns =
                completion_panel_columns(popup_col, total_cols, label_w, comp.doc.is_some());
            let popup_col = panel_columns.popup_col;
            let pane_w = panel_columns.pane_width;
            let show_doc = panel_columns.show_doc;
            let doc_gap = usize::from(show_doc);
            let doc_col = popup_col + pane_w + doc_gap;
            let doc_pad_x = 3usize;
            let doc_pad_top = 1usize;
            let doc_text_w = pane_w.saturating_sub(doc_pad_x * 2);
            let doc_body = comp
                .doc
                .as_ref()
                .map(|(_, body)| gpu_scene::wrap_completion_doc_lines(body, doc_text_w));
            let doc_content_h = if show_doc {
                doc_pad_top + 3 + doc_body.as_ref().map(|body| body.len()).unwrap_or(0)
            } else {
                0
            };
            let row_step = 1usize;
            let list_pad_top = 1usize;
            let list_visible_h = list_pad_top + comp.entries.len() * row_step;
            let desired_h = list_visible_h.max(8).max(doc_content_h).min(16);
            let rows_below = total_rows.saturating_sub(popup_row + 1);
            let panel_h = desired_h.min(rows_below.max(1));
            let panel_row = if panel_h < desired_h && anchor_row > desired_h {
                anchor_row.saturating_sub(desired_h)
            } else {
                popup_row
            };
            let mut popup_verts = Vec::new();
            let panel_bg = theme::COMP_UNSELECTED_BG();
            let panel_border = theme::COMP_BORDER();
            let muted_fg = to_rgba(theme::COMP_CATEGORY_FG());
            let doc_bg = theme::COMP_DOC_BG();
            let doc_border = theme::COMP_DOC_BORDER();
            let mut rounded = Vec::new();
            gpu_scene::push_rounded_instance_cells(
                &mut rounded,
                popup_col as f32,
                panel_row as f32,
                pane_w as f32,
                panel_h as f32,
                panel_bg,
                AUTOCOMPLETE_PANEL_CORNER_RADIUS_PX * ui_px_scale,
                pop_cell_w,
                pop_cell_h,
                vp_w,
                vp_h,
            );
            if show_doc {
                gpu_scene::push_rounded_instance_cells(
                    &mut rounded,
                    doc_col as f32,
                    panel_row as f32,
                    pane_w as f32,
                    panel_h as f32,
                    doc_bg,
                    AUTOCOMPLETE_PANEL_CORNER_RADIUS_PX * ui_px_scale,
                    pop_cell_w,
                    pop_cell_h,
                    vp_w,
                    vp_h,
                );
            }
            {
                let gpu = self.gpu.as_ref().expect("gpu initialized");
                if gpu.widget_pipelines.contains_key("dropdown") {
                    Self::plan_widget_instances(
                        &mut plan,
                        &gpu.device,
                        full_scissor,
                        "dropdown",
                        &rounded,
                    );
                }
            }
            gpu_scene::push_rounded_rect_border_px(
                &mut popup_verts,
                popup_col as f32 * pop_cell_w,
                panel_row as f32 * pop_cell_h,
                pane_w as f32 * pop_cell_w,
                panel_h as f32 * pop_cell_h,
                AUTOCOMPLETE_PANEL_BORDER_WIDTH_PX,
                AUTOCOMPLETE_PANEL_CORNER_RADIUS_PX * ui_px_scale,
                panel_border,
                vp_w,
                vp_h,
            );
            if show_doc {
                gpu_scene::push_rounded_rect_border_px(
                    &mut popup_verts,
                    doc_col as f32 * pop_cell_w,
                    panel_row as f32 * pop_cell_h,
                    pane_w as f32 * pop_cell_w,
                    panel_h as f32 * pop_cell_h,
                    AUTOCOMPLETE_PANEL_BORDER_WIDTH_PX,
                    AUTOCOMPLETE_PANEL_CORNER_RADIUS_PX * ui_px_scale,
                    doc_border,
                    vp_w,
                    vp_h,
                );
            }
            if let Some(atlas) = self.atlas.as_mut() {
                for (i, entry) in comp.entries.iter().enumerate() {
                    let row = panel_row + list_pad_top + i * row_step;
                    if row >= panel_row + panel_h {
                        break;
                    }
                    if entry.selected {
                        let mut selected = Vec::new();
                        gpu_scene::push_rounded_instance_cells(
                            &mut selected,
                            popup_col as f32 + 1.0,
                            row as f32 - 0.15,
                            pane_w.saturating_sub(2) as f32,
                            1.18,
                            sel_bg,
                            AUTOCOMPLETE_ROW_CORNER_RADIUS_PX * ui_px_scale,
                            pop_cell_w,
                            pop_cell_h,
                            vp_w,
                            vp_h,
                        );
                        let gpu = self.gpu.as_ref().expect("gpu initialized");
                        if gpu.widget_pipelines.contains_key("dropdown") {
                            Self::plan_widget_instances(
                                &mut plan,
                                &gpu.device,
                                full_scissor,
                                "dropdown",
                                &selected,
                            );
                        }
                    }
                    let entry_fg = if entry.selected {
                        to_rgba(theme::COMP_SELECTED_FG())
                    } else {
                        to_rgba(theme::COMP_FG())
                    };
                    let entry_bg = if entry.selected {
                        to_rgba(sel_bg)
                    } else {
                        to_rgba(panel_bg)
                    };
                    gpu_scene::push_text_cells(
                        &mut popup_verts,
                        &mut atlas.atlas,
                        &entry.label,
                        popup_col + 3,
                        row,
                        pane_w.saturating_sub(6),
                        entry_fg,
                        entry_bg,
                        pop_cell_w,
                        pop_cell_h,
                        vp_w,
                        vp_h,
                    );
                    if let Some(category) = &entry.category {
                        let category_width = category.chars().count();
                        let content_width = pane_w.saturating_sub(6);
                        if entry.label.chars().count() + category_width + 2 <= content_width {
                            gpu_scene::push_text_cells(
                                &mut popup_verts,
                                &mut atlas.atlas,
                                category,
                                popup_col + pane_w - 3 - category_width,
                                row,
                                category_width,
                                to_rgba(theme::COMP_CATEGORY_FG()),
                                entry_bg,
                                pop_cell_w,
                                pop_cell_h,
                                vp_w,
                                vp_h,
                            );
                        }
                    }
                }
                if show_doc && let Some((title, _)) = &comp.doc {
                    let title_fg = to_rgba(theme::COMP_DOC_TITLE_FG());
                    let doc_fg = to_rgba(theme::COMP_DOC_FG());
                    let doc_bg_rgba = to_rgba(doc_bg);
                    gpu_scene::push_text_cells(
                        &mut popup_verts,
                        &mut atlas.atlas,
                        title,
                        doc_col + doc_pad_x,
                        panel_row + doc_pad_top,
                        doc_text_w,
                        title_fg,
                        doc_bg_rgba,
                        pop_cell_w,
                        pop_cell_h,
                        vp_w,
                        vp_h,
                    );
                    gpu_scene::push_rect_px(
                        &mut popup_verts,
                        (doc_col + doc_pad_x) as f32 * pop_cell_w,
                        (panel_row + doc_pad_top + 2) as f32 * pop_cell_h - 2.0,
                        doc_text_w as f32 * pop_cell_w,
                        1.0,
                        doc_border,
                        vp_w,
                        vp_h,
                    );
                    if let Some(lines) = &doc_body {
                        for (li, line) in lines.iter().enumerate() {
                            let row = panel_row + doc_pad_top + 3 + li;
                            if row >= panel_row + panel_h {
                                break;
                            }
                            gpu_scene::push_text_cells(
                                &mut popup_verts,
                                &mut atlas.atlas,
                                line,
                                doc_col + doc_pad_x,
                                row,
                                doc_text_w,
                                doc_fg,
                                doc_bg_rgba,
                                pop_cell_w,
                                pop_cell_h,
                                vp_w,
                                vp_h,
                            );
                        }
                    }
                    if doc_content_h == 0 {
                        gpu_scene::push_text_cells(
                            &mut popup_verts,
                            &mut atlas.atlas,
                            "No documentation.",
                            doc_col + doc_pad_x,
                            panel_row + doc_pad_top + 3,
                            doc_text_w,
                            muted_fg,
                            doc_bg_rgba,
                            pop_cell_w,
                            pop_cell_h,
                            vp_w,
                            vp_h,
                        );
                    }
                }
            }
            let gpu = self.gpu.as_ref().expect("gpu initialized");
            Self::plan_vertices(
                &mut plan,
                &gpu.device,
                full_scissor,
                PipelineRef::Text { zoomed: false },
                &popup_verts,
            );
        }

        // ── Encode ───────────────────────────────────────────────────────────
        // Every glyph the plan phase rasterized is in the CPU bitmaps now;
        // upload dirty atlases before sampling them.
        {
            let gpu = self.gpu.as_ref().expect("gpu initialized");
            if let Some(atlas) = self.atlas.as_mut() {
                atlas.sync_texture(&gpu.queue);
            }
            if let Some(atlas) = self.text_atlas.as_mut() {
                atlas.sync_texture(&gpu.queue);
            }
            if let Some(atlas) = self.prop_atlas.as_mut() {
                atlas.sync_texture(&gpu.queue);
            }
        }

        sample.plan = plan_start.elapsed();
        sample.draw_commands = plan.len() as u64;
        // The shell creates exactly one vertex/instance buffer per draw command
        // and throws it away at end of frame. Reporting the count and the bytes
        // makes that allocation load visible instead of implied.
        sample.buffers_created = plan.len() as u64;
        sample.buffer_bytes = plan.iter().map(|cmd| cmd.buffer.size() as usize).sum();

        let gpu = self.gpu.as_ref().expect("gpu initialized");
        // Under `Fifo` this call blocks until a swapchain image frees up. It is
        // measured on its own so present backpressure is never mistaken for CPU
        // frame cost.
        let acquire_start = Instant::now();
        let frame = match gpu.surface.get_current_texture() {
            Ok(frame) => frame,
            Err(wgpu::SurfaceError::Timeout) => return Ok(TiledRenderStatus::NotPresented),
            Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => {
                gpu.surface.configure(&gpu.device, &gpu.config);
                return Ok(TiledRenderStatus::NotPresented);
            }
            Err(_) => return Err(BackendError::MetalError),
        };
        sample.acquire = acquire_start.elapsed();
        let encode_start = Instant::now();
        let view = frame.texture.create_view(&Default::default());
        let mut encoder = gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("eseqlisp tiled frame"),
            });
        {
            let bg = theme::BG();
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("eseqlisp tiled pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: bg.r as f64,
                            g: bg.g as f64,
                            b: bg.b as f64,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            let atlas_group = self.atlas.as_ref().map(|a| &a.bind_group);
            let text_atlas_group = self.text_atlas.as_ref().map(|a| &a.bind_group);
            let prop_atlas_group = self.prop_atlas.as_ref().map(|a| &a.bind_group);
            for cmd in &plan {
                // wgpu validates the scissor against the surface; clamp the
                // cell-derived rects the way Metal implicitly did.
                let clamped = cmd.scissor.intersect(ScissorRect::full(
                    gpu.config.width,
                    gpu.config.height,
                ));
                if clamped.width == 0 || clamped.height == 0 {
                    continue;
                }
                match &cmd.pipeline {
                    PipelineRef::Text { zoomed } => {
                        let group = if *zoomed {
                            text_atlas_group.or(atlas_group)
                        } else {
                            atlas_group
                        };
                        let Some(group) = group else { continue };
                        pass.set_pipeline(&gpu.text_pipeline);
                        pass.set_bind_group(0, group, &[]);
                    }
                    PipelineRef::Prop => {
                        let Some(group) = prop_atlas_group else { continue };
                        pass.set_pipeline(&gpu.prop_pipeline);
                        pass.set_bind_group(0, group, &[]);
                    }
                    PipelineRef::Cable => {
                        pass.set_pipeline(&gpu.cable_pipeline);
                    }
                    PipelineRef::Widget(name) => {
                        let Some(pipeline) = gpu.widget_pipelines.get(name) else {
                            continue;
                        };
                        pass.set_pipeline(pipeline);
                    }
                    PipelineRef::Image(path) => {
                        let Some(resource) = self.image_textures.get(path) else {
                            continue;
                        };
                        pass.set_pipeline(&gpu.image_pipeline);
                        pass.set_bind_group(0, &resource.bind_group, &[]);
                    }
                    PipelineRef::Waveform(key) => {
                        let Some(resource) = self.waveform_buffers.get(key) else {
                            continue;
                        };
                        pass.set_pipeline(&gpu.waveform_pipeline);
                        pass.set_bind_group(0, &resource.bind_group, &[]);
                    }
                    PipelineRef::Wavetable(key) => {
                        let Some(resource) = self.wavetable_buffers.get(key) else {
                            continue;
                        };
                        pass.set_pipeline(&gpu.wavetable_pipeline);
                        pass.set_bind_group(0, &resource.bind_group, &[]);
                    }
                    PipelineRef::SpectrogramWaterfall(key) => {
                        let Some(resource) = self.live_spectrogram_buffers.get(key) else {
                            continue;
                        };
                        pass.set_pipeline(&gpu.spectrogram_pipelines.waterfall);
                        pass.set_bind_group(0, &resource.bind_group, &[]);
                    }
                    PipelineRef::SpectrogramEq(key) => {
                        let Some(resource) = self.live_spectrogram_buffers.get(key) else {
                            continue;
                        };
                        pass.set_pipeline(&gpu.spectrogram_pipelines.eq);
                        pass.set_bind_group(0, &resource.bind_group, &[]);
                    }
                }
                pass.set_scissor_rect(clamped.x, clamped.y, clamped.width, clamped.height);
                pass.set_vertex_buffer(0, cmd.buffer.slice(..));
                match cmd.kind {
                    DrawKind::Vertices(count) => pass.draw(0..count, 0..1),
                    DrawKind::Instanced(count) => pass.draw(0..6, 0..count),
                }
            }
        }
        gpu.queue.submit(Some(encoder.finish()));
        frame.present();
        sample.encode = encode_start.elapsed();
        self.frame_stats.end_frame(sample);
        Ok(TiledRenderStatus::Presented)
    }
}

#[cfg(target_os = "linux")]
fn create_event_loop() -> Result<EventLoop<()>, winit::error::EventLoopError> {
    let mut builder = winit::event_loop::EventLoopBuilder::new();

    // Production deliberately leaves backend selection to winit: a Wayland
    // session must get a native Wayland surface so fractional-scale events are
    // authoritative, while an actual X11 session naturally selects X11. Winit
    // 0.29 does not support file drops on Wayland; forcing the whole app onto
    // XWayland to obtain XDND breaks that scale contract and is not acceptable.
    //
    // Test binaries create event loops off the main thread, so explicitly pick
    // the available backend there and opt only the test loop into that mode.
    #[cfg(test)]
    if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        use winit::platform::wayland::EventLoopBuilderExtWayland;
        builder.with_wayland().with_any_thread(true);
    } else {
        use winit::platform::x11::EventLoopBuilderExtX11;
        builder.with_x11().with_any_thread(true);
    }

    builder.build()
}

#[cfg(not(target_os = "linux"))]
fn create_event_loop() -> Result<EventLoop<()>, winit::error::EventLoopError> {
    EventLoop::new()
}

impl Backend for WgpuAppBackend {
    fn initialize(&mut self) -> Result<(), BackendError> {
        // ── Window ───────────────────────────────────────────────────────────
        let event_loop = create_event_loop().map_err(|_| BackendError::MetalError)?;
        let window = winit::window::WindowBuilder::new()
            .with_title("eseq")
            .with_inner_size(self.initial_window_size)
            .with_visible(self.initial_window_visible)
            .build(&event_loop)
            .map_err(|_| BackendError::MetalError)?;
        let window = Arc::new(window);
        let phys = window.inner_size();

        // ── Device / surface ─────────────────────────────────────────────────
        let instance = wgpu::Instance::default();
        let surface = instance
            .create_surface(window.clone())
            .map_err(|_| BackendError::MetalError)?;
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: Some(&surface),
            force_fallback_adapter: false,
        }))
        .ok_or(BackendError::MetalError)?;
        // Report and assert the selection before any device work: a silent
        // fallback to OpenGL or a software rasterizer renders correct pixels at
        // a completely different cost, so it must not be discovered later from
        // a performance number.
        if !self.reported_adapter {
            self.reported_adapter = true;
            let policy = gpu_adapter::AdapterPolicy::from_env();
            if let Err(message) = gpu_adapter::report_and_check(&adapter.get_info(), &policy) {
                eprintln!("{message}");
                return Err(BackendError::MetalError);
            }
        }
        // `downlevel_defaults` caps textures at 2048px, smaller than a
        // fullscreened window on a HiDPI display; take the adapter's real
        // resolution limits so the swapchain can match the window.
        let (device, queue) = pollster::block_on(adapter.request_device(
            &wgpu::DeviceDescriptor {
                label: Some("eseq wgpu device"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::downlevel_defaults()
                    .using_resolution(adapter.limits()),
                memory_hints: wgpu::MemoryHints::Performance,
            },
            None,
        ))
        .map_err(|_| BackendError::MetalError)?;

        let capabilities = surface.get_capabilities(&adapter);
        // Prefer a non-sRGB format so authored colors match Metal's
        // BGRA8Unorm drawable — see `wgpu_backend::preferred_surface_format`.
        let format = capabilities
            .formats
            .iter()
            .copied()
            .find(|format| !format.is_srgb())
            .or_else(|| capabilities.formats.first().copied())
            .ok_or(BackendError::MetalError)?;
        let alpha_mode = capabilities
            .alpha_modes
            .iter()
            .copied()
            .find(|mode| *mode == wgpu::CompositeAlphaMode::Opaque)
            .unwrap_or(capabilities.alpha_modes[0]);
        let max_dim = device.limits().max_texture_dimension_2d.max(1);
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: phys.width.clamp(1, max_dim),
            height: phys.height.clamp(1, max_dim),
            present_mode: wgpu::PresentMode::Fifo,
            desired_maximum_frame_latency: 2,
            alpha_mode,
            view_formats: vec![],
        };
        surface.configure(&device, &config);

        // ── Pipelines ────────────────────────────────────────────────────────
        let atlas_layout = pipelines::texture_bind_group_layout(&device, "eseq atlas");
        let storage1_layout = pipelines::storage_bind_group_layout(&device, "eseq data", 1);
        let storage2_layout = pipelines::storage_bind_group_layout(&device, "eseq data2", 2);
        let (text_pipeline, prop_pipeline) =
            pipelines::text_pipelines(&device, &atlas_layout, format);
        let image_pipeline = pipelines::image_pipeline(&device, &atlas_layout, format);
        let cable_pipeline = pipelines::patch_cable_pipeline(&device, format);
        let waveform_pipeline = pipelines::waveform_pipeline(&device, &storage1_layout, format);
        let wavetable_pipeline = pipelines::wavetable_pipeline(&device, &storage1_layout, format);
        let spectrogram_pipelines =
            pipelines::live_spectrogram_pipelines(&device, &storage2_layout, format);

        let sampler = |filter: wgpu::FilterMode, label: &str| {
            device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some(label),
                address_mode_u: wgpu::AddressMode::ClampToEdge,
                address_mode_v: wgpu::AddressMode::ClampToEdge,
                address_mode_w: wgpu::AddressMode::ClampToEdge,
                mag_filter: filter,
                min_filter: filter,
                mipmap_filter: wgpu::FilterMode::Nearest,
                ..Default::default()
            })
        };
        let nearest_sampler = sampler(wgpu::FilterMode::Nearest, "eseq nearest sampler");
        let linear_sampler = sampler(wgpu::FilterMode::Linear, "eseq linear sampler");

        let mut gpu = GpuState {
            surface,
            device,
            queue,
            config,
            text_pipeline,
            prop_pipeline,
            image_pipeline,
            cable_pipeline,
            waveform_pipeline,
            wavetable_pipeline,
            spectrogram_pipelines,
            widget_pipelines: HashMap::new(),
            atlas_layout,
            storage1_layout,
            storage2_layout,
            nearest_sampler,
            linear_sampler,
        };

        // Built-in widget fragment shaders (ported WGSL bodies).
        for (widget_type, vertex_src, fragment_src) in
            widget_render::widget_shader_sources(widget_render::ShaderBackend::Wgsl)
        {
            if let Some(pipeline) =
                gpu.compile_widget_pipeline(widget_type, vertex_src, fragment_src)
            {
                gpu.widget_pipelines.insert(widget_type.to_string(), pipeline);
            }
        }
        // The shared button surface doubles as the number-picker fill, exactly
        // as the Metal backend compiles shaders/button_surface.metal for both.
        for widget_type in ["button", "number-picker"] {
            if gpu.widget_pipelines.contains_key(widget_type) {
                continue;
            }
            if let Some(pipeline) = gpu.compile_widget_pipeline(
                widget_type,
                None,
                wgsl_shaders::BUTTON_SURFACE_WGSL,
            ) {
                gpu.widget_pipelines.insert(widget_type.to_string(), pipeline);
            }
        }

        // ── Glyph atlases ────────────────────────────────────────────────────
        // On Wayland this is 1.0 until the surface maps to an output; the
        // real factor arrives as ScaleFactorChanged and is applied by
        // `apply_scale_factor`.
        let scale = window.scale_factor();
        self.atlas_scale_factor = scale;
        self.prop_text_scale_bits
            .store(scale.to_bits(), Ordering::Relaxed);
        widget_render::set_ui_scale_factor(scale as f32);
        let atlas = WgpuGlyphAtlas::new(
            &gpu.device,
            &gpu.atlas_layout,
            &gpu.nearest_sampler,
            MONOSPACE_FONT_NAME,
            self.monospace_font_size_pt * scale,
        );
        let text_zoom = if self.text_atlas_zoom.is_finite() && self.text_atlas_zoom > 0.0 {
            self.text_atlas_zoom
        } else {
            1.0
        };
        let text_atlas = WgpuGlyphAtlas::new(
            &gpu.device,
            &gpu.atlas_layout,
            &gpu.nearest_sampler,
            MONOSPACE_FONT_NAME,
            self.monospace_font_size_pt * text_zoom as f64 * scale,
        );
        let prop_atlas = WgpuPropGlyphAtlas::new(
            &gpu.device,
            &gpu.atlas_layout,
            &gpu.linear_sampler,
            scale,
        );

        self.atlas = atlas;
        self.text_atlas = text_atlas;
        self.text_atlas_zoom = text_zoom;
        self.prop_atlas = prop_atlas;
        self.prop_text_layout_cache = PropTextLayoutCache::new();
        self.gpu = Some(gpu);
        self.event_loop = Some(event_loop);
        self.window = Some(window);
        self.sync_window_theme();
        // User-content SDF widget shaders registered before the window came up
        // (the editor and its UI load first) compile now.
        self.compile_pending_sdf_pipelines();
        Ok(())
    }

    fn teardown(&mut self) -> Result<(), BackendError> {
        self.window = None;
        self.event_loop = None;
        Ok(())
    }

    fn viewport_size(&self) -> (usize, usize) {
        let Some(window) = &self.window else {
            return (80, 24);
        };
        let size = window.inner_size();
        let (cell_w, cell_h) = self
            .atlas
            .as_ref()
            .map(|a| (a.cell_w.max(1), a.cell_h.max(1)))
            .unwrap_or((8, 16));
        let cols = (size.width as usize / cell_w).max(1);
        let rows = (size.height as usize / cell_h).max(1);
        (cols, rows)
    }

    fn poll_backend_event(&mut self, timeout: Duration) -> Option<BackendEvent> {
        if std::mem::take(&mut self.close_requested) {
            return Some(BackendEvent::Quit);
        }
        if let Some(paths) = self.pending_file_drops.pop_front() {
            return Some(BackendEvent::FileDrop(paths));
        }
        let terminal_event = self.poll_event(timeout);
        if std::mem::take(&mut self.close_requested) {
            Some(BackendEvent::Quit)
        } else {
            terminal_event.map(BackendEvent::Terminal)
        }
    }

    fn poll_event(&mut self, timeout: Duration) -> Option<Event> {
        if let Some(ev) = self.pending.pop_front() {
            if matches!(ev, Event::Mouse(_)) {
                self.last_precise_mouse = Some(self.cursor_pos);
            }
            return Some(ev);
        }
        if let Some(ev) = self.pending_drag.take() {
            self.last_precise_mouse = Some(self.cursor_pos);
            return Some(ev);
        }
        if let Some(ev) = self.pending_move.take() {
            self.last_precise_mouse = Some(self.cursor_pos);
            return Some(ev);
        }
        let Some(event_loop) = &mut self.event_loop else {
            return None;
        };
        let pending = &mut self.pending;
        let pending_drag = &mut self.pending_drag;
        let pending_move = &mut self.pending_move;
        let pending_magnify = &mut self.pending_magnify;
        let pending_scroll = &mut self.pending_scroll;
        let pending_resize = &mut self.pending_resize;
        let pending_scale_factor = &mut self.pending_scale_factor;
        let close_requested = &mut self.close_requested;
        let suppress_scroll_until = &mut self.suppress_scroll_until;
        let modifiers = &mut self.modifiers;
        let pressed_mouse_button = &mut self.pressed_mouse_button;
        let cursor_cell = &mut self.cursor_cell;
        let cursor_pos = &mut self.cursor_pos;
        let window_ref = self.window.as_deref();
        let cell_size = self
            .atlas
            .as_ref()
            .map(|a| (a.cell_w.max(1) as f64, a.cell_h.max(1) as f64))
            .unwrap_or((8.0, 16.0));
        let wake_at = Instant::now() + timeout;
        let mut dropped_paths = Vec::new();
        event_loop.pump_events(Some(timeout), |event, elwt| {
            elwt.set_control_flow(if timeout.is_zero() {
                ControlFlow::Poll
            } else {
                ControlFlow::WaitUntil(wake_at)
            });
            let WEvent::WindowEvent { event, .. } = event else {
                return;
            };
            match event {
                WindowEvent::CloseRequested => {
                    *close_requested = true;
                }
                WindowEvent::Resized(new_size) => {
                    // The surface reconfigures lazily at render time from the
                    // window's inner size; just wake the redraw path.
                    *pending_resize = true;
                    if let Some(w) = window_ref {
                        w.request_redraw();
                    }
                    pending.push_back(Event::Resize(
                        new_size.width as u16,
                        new_size.height as u16,
                    ));
                }
                WindowEvent::RedrawRequested => {
                    pending.push_back(Event::Resize(0, 0));
                }
                WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                    // Applied after the pump returns: the atlas rebuild needs
                    // `&mut self`, which the closure cannot borrow.
                    *pending_scale_factor = Some(scale_factor);
                    *pending_resize = true;
                    if let Some(w) = window_ref {
                        w.request_redraw();
                    }
                }
                WindowEvent::DroppedFile(path) => {
                    dropped_paths.push(path);
                }
                WindowEvent::ModifiersChanged(mods) => {
                    *modifiers = winit_mods_to_crossterm(mods.state());
                }
                WindowEvent::KeyboardInput { event: kev, .. } => {
                    if let Some(ev) = translate_key_with_state(
                        &kev.logical_key,
                        &kev.physical_key,
                        *modifiers,
                        kev.state,
                    ) {
                        pending.push_back(ev);
                    }
                }
                WindowEvent::CursorMoved { position, .. } => {
                    let exact_col = (position.x / cell_size.0).max(0.0) as f32;
                    let exact_row = (position.y / cell_size.1).max(0.0) as f32;
                    let col = exact_col.floor() as u16;
                    let row = exact_row.floor() as u16;
                    *cursor_pos = (exact_col, exact_row);
                    *cursor_cell = (col, row);
                    if let Some(button) = pressed_mouse_button {
                        *pending_drag = Some(Event::Mouse(MouseEvent {
                            kind: MouseEventKind::Drag(*button),
                            column: col,
                            row,
                            modifiers: *modifiers,
                        }));
                    } else {
                        // Coalesce Moved events — only keep the latest for
                        // hover detection.
                        *pending_move = Some(Event::Mouse(MouseEvent {
                            kind: MouseEventKind::Moved,
                            column: col,
                            row,
                            modifiers: *modifiers,
                        }));
                    }
                }
                WindowEvent::MouseInput { state, button, .. } => {
                    let Some(button) = translate_mouse_button(button) else {
                        return;
                    };
                    match state {
                        ElementState::Pressed => {
                            *pressed_mouse_button = Some(button);
                            pending.push_back(Event::Mouse(MouseEvent {
                                kind: MouseEventKind::Down(button),
                                column: cursor_cell.0,
                                row: cursor_cell.1,
                                modifiers: *modifiers,
                            }));
                        }
                        ElementState::Released => {
                            let release = Event::Mouse(MouseEvent {
                                kind: MouseEventKind::Up(button),
                                column: cursor_cell.0,
                                row: cursor_cell.1,
                                modifiers: *modifiers,
                            });
                            // Cursor movement is coalesced while a button is
                            // held; flush the final drag before the release so
                            // consumers observe the complete gesture.
                            if let Some(drag) = pending_drag.take() {
                                pending.push_back(drag);
                            }
                            pending.push_back(release);
                            if pressed_mouse_button.as_ref() == Some(&button) {
                                *pressed_mouse_button = None;
                            }
                        }
                    }
                }
                WindowEvent::MouseWheel { delta, phase, .. } => {
                    if matches!(phase, TouchPhase::Ended | TouchPhase::Cancelled) {
                        return;
                    }
                    if let Some(until) = *suppress_scroll_until {
                        if Instant::now() < until {
                            return;
                        }
                        *suppress_scroll_until = None;
                    }
                    let delta = crate::ui::pointer_input::scroll_delta_pixels(
                        delta,
                        cell_size.1 as f32,
                    );
                    pending_scroll.push_back((delta, *cursor_pos));
                }
                WindowEvent::TouchpadMagnify { delta, phase, .. } => {
                    if matches!(phase, TouchPhase::Ended | TouchPhase::Cancelled) {
                        return;
                    }
                    pending_scroll.clear();
                    *suppress_scroll_until = Some(Instant::now() + Duration::from_millis(120));
                    pending_magnify.push_back((delta, *cursor_pos));
                }
                _ => {}
            }
        });
        if !dropped_paths.is_empty() {
            self.pending_file_drops.push_back(dropped_paths);
        }
        if let Some(scale) = self.pending_scale_factor.take() {
            self.apply_scale_factor(scale);
        }
        if let Some(ev) = self.pending.pop_front() {
            if matches!(ev, Event::Mouse(_)) {
                self.last_precise_mouse = Some(self.cursor_pos);
            }
            Some(ev)
        } else if let Some(ev) = self.pending_drag.take() {
            self.last_precise_mouse = Some(self.cursor_pos);
            Some(ev)
        } else {
            None
        }
    }

    fn render(&mut self, _frame: &RenderFrame) -> Result<(), BackendError> {
        // The app shell renders exclusively through `render_tiled`; the
        // single-frame trait entry point only backs the macOS capture paths.
        Ok(())
    }
}

// ── winit → crossterm translation ────────────────────────────────────────────

fn winit_mods_to_crossterm(mods: winit::keyboard::ModifiersState) -> KeyModifiers {
    let mut out = KeyModifiers::NONE;
    if mods.shift_key() {
        out |= KeyModifiers::SHIFT;
    }
    if mods.control_key() {
        out |= KeyModifiers::CONTROL;
    }
    if mods.alt_key() {
        out |= KeyModifiers::ALT;
    }
    if mods.super_key() {
        out |= KeyModifiers::SUPER;
    }
    out
}

fn translate_key_with_state(
    key: &Key,
    physical_key: &PhysicalKey,
    mods: KeyModifiers,
    state: ElementState,
) -> Option<Event> {
    let code = if mods.intersects(KeyModifiers::ALT | KeyModifiers::CONTROL | KeyModifiers::SUPER)
    {
        translate_physical_shortcut_key(physical_key).or_else(|| translate_logical_key(key))?
    } else {
        translate_logical_key(key)?
    };
    let kind = match state {
        ElementState::Pressed => KeyEventKind::Press,
        ElementState::Released => KeyEventKind::Release,
    };
    Some(Event::Key(KeyEvent {
        code,
        modifiers: mods,
        kind,
        state: crossterm::event::KeyEventState::NONE,
    }))
}

fn translate_logical_key(key: &Key) -> Option<KeyCode> {
    let code = match key {
        Key::Named(named) => match named {
            NamedKey::Enter => KeyCode::Enter,
            NamedKey::Escape => KeyCode::Esc,
            NamedKey::Backspace => KeyCode::Backspace,
            NamedKey::Delete => KeyCode::Delete,
            NamedKey::Tab => KeyCode::Tab,
            NamedKey::Space => KeyCode::Char(' '),
            NamedKey::ArrowUp => KeyCode::Up,
            NamedKey::ArrowDown => KeyCode::Down,
            NamedKey::ArrowLeft => KeyCode::Left,
            NamedKey::ArrowRight => KeyCode::Right,
            NamedKey::Home => KeyCode::Home,
            NamedKey::End => KeyCode::End,
            NamedKey::PageUp => KeyCode::PageUp,
            NamedKey::PageDown => KeyCode::PageDown,
            _ => return None,
        },
        Key::Character(s) => KeyCode::Char(s.chars().next()?),
        _ => return None,
    };
    Some(code)
}

fn translate_physical_shortcut_key(key: &PhysicalKey) -> Option<KeyCode> {
    let PhysicalKey::Code(code) = key else {
        return None;
    };
    let code = match code {
        WinitKeyCode::KeyA => KeyCode::Char('a'),
        WinitKeyCode::KeyB => KeyCode::Char('b'),
        WinitKeyCode::KeyC => KeyCode::Char('c'),
        WinitKeyCode::KeyD => KeyCode::Char('d'),
        WinitKeyCode::KeyE => KeyCode::Char('e'),
        WinitKeyCode::KeyF => KeyCode::Char('f'),
        WinitKeyCode::KeyG => KeyCode::Char('g'),
        WinitKeyCode::KeyH => KeyCode::Char('h'),
        WinitKeyCode::KeyI => KeyCode::Char('i'),
        WinitKeyCode::KeyJ => KeyCode::Char('j'),
        WinitKeyCode::KeyK => KeyCode::Char('k'),
        WinitKeyCode::KeyL => KeyCode::Char('l'),
        WinitKeyCode::KeyM => KeyCode::Char('m'),
        WinitKeyCode::KeyN => KeyCode::Char('n'),
        WinitKeyCode::KeyO => KeyCode::Char('o'),
        WinitKeyCode::KeyP => KeyCode::Char('p'),
        WinitKeyCode::KeyQ => KeyCode::Char('q'),
        WinitKeyCode::KeyR => KeyCode::Char('r'),
        WinitKeyCode::KeyS => KeyCode::Char('s'),
        WinitKeyCode::KeyT => KeyCode::Char('t'),
        WinitKeyCode::KeyU => KeyCode::Char('u'),
        WinitKeyCode::KeyV => KeyCode::Char('v'),
        WinitKeyCode::KeyW => KeyCode::Char('w'),
        WinitKeyCode::KeyX => KeyCode::Char('x'),
        WinitKeyCode::KeyY => KeyCode::Char('y'),
        WinitKeyCode::KeyZ => KeyCode::Char('z'),
        WinitKeyCode::ArrowUp => KeyCode::Up,
        WinitKeyCode::ArrowDown => KeyCode::Down,
        WinitKeyCode::ArrowLeft => KeyCode::Left,
        WinitKeyCode::ArrowRight => KeyCode::Right,
        WinitKeyCode::Space => KeyCode::Char(' '),
        WinitKeyCode::Tab => KeyCode::Tab,
        WinitKeyCode::Backspace => KeyCode::Backspace,
        WinitKeyCode::Delete => KeyCode::Delete,
        WinitKeyCode::Enter => KeyCode::Enter,
        WinitKeyCode::Home => KeyCode::Home,
        WinitKeyCode::End => KeyCode::End,
        WinitKeyCode::PageUp => KeyCode::PageUp,
        WinitKeyCode::PageDown => KeyCode::PageDown,
        _ => return None,
    };
    Some(code)
}

fn translate_mouse_button(button: WMouseButton) -> Option<MouseButton> {
    match button {
        WMouseButton::Left => Some(MouseButton::Left),
        WMouseButton::Right => Some(MouseButton::Right),
        WMouseButton::Middle => Some(MouseButton::Middle),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "linux")]
    #[test]
    fn event_loop_uses_the_native_linux_session_backend() {
        use winit::platform::wayland::EventLoopWindowTargetExtWayland;
        use winit::platform::x11::EventLoopWindowTargetExtX11;

        if std::env::var_os("WAYLAND_DISPLAY").is_some() {
            let event_loop = create_event_loop().expect("Wayland event loop should initialize");
            assert!(event_loop.is_wayland());
        } else if std::env::var_os("DISPLAY").is_some() {
            let event_loop = create_event_loop().expect("X11 event loop should initialize");
            assert!(event_loop.is_x11());
        }
    }

    #[test]
    fn close_request_is_reported_as_one_quit_event() {
        let Ok(mut backend) = WgpuAppBackend::new() else {
            panic!("backend construction should succeed");
        };
        backend.close_requested = true;

        assert_eq!(
            backend.poll_backend_event(Duration::ZERO),
            Some(BackendEvent::Quit)
        );
        assert_eq!(backend.poll_backend_event(Duration::ZERO), None);
    }
}
