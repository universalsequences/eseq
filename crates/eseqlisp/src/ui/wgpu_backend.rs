//! Portable wgpu renderer for the backend-neutral GPU primitive display list.
//!
//! This first renderer intentionally supports the common solid rectangle and
//! quad path. Text and specialized widget pipelines are layered onto the same
//! primitive-run walk by subsequent ports.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::sync::Arc;

use wgpu::util::DeviceExt;
use winit::dpi::PhysicalSize;
use winit::window::Window;

use crate::backend::Color;
use crate::ui::gpu_geometry::{
    self, ClipStack, PrimitiveRunGeometry, PrimitiveRunOp, ScissorRect, SolidQuadInstance,
};
use crate::widget_render::{GpuPrimitiveRun, GpuPrimitiveRunKey};

const SOLID_QUAD_SHADER: &str = r#"
struct QuadInstance {
    ndc_bounds: vec4<f32>,
    color: vec4<f32>,
};

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) color: vec4<f32>,
};

@vertex
fn solid_quad_vertex(
    @builtin(vertex_index) vertex_index: u32,
    @location(0) ndc_bounds: vec4<f32>,
    @location(1) color: vec4<f32>,
) -> VertexOutput {
    let corners = array<vec2<f32>, 6>(
        vec2<f32>(ndc_bounds.x, ndc_bounds.y),
        vec2<f32>(ndc_bounds.x, ndc_bounds.w),
        vec2<f32>(ndc_bounds.z, ndc_bounds.y),
        vec2<f32>(ndc_bounds.z, ndc_bounds.y),
        vec2<f32>(ndc_bounds.x, ndc_bounds.w),
        vec2<f32>(ndc_bounds.z, ndc_bounds.w),
    );
    var output: VertexOutput;
    output.position = vec4<f32>(corners[vertex_index], 0.0, 1.0);
    output.color = color;
    return output;
}

@fragment
fn solid_quad_fragment(input: VertexOutput) -> @location(0) vec4<f32> {
    return input.color;
}
"#;

#[derive(Debug)]
pub enum WgpuBackendError {
    CreateSurface(wgpu::CreateSurfaceError),
    NoAdapter,
    RequestDevice(wgpu::RequestDeviceError),
    Surface(wgpu::SurfaceError),
}

impl fmt::Display for WgpuBackendError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CreateSurface(error) => write!(f, "failed to create wgpu surface: {error}"),
            Self::NoAdapter => write!(f, "no wgpu adapter supports the window surface"),
            Self::RequestDevice(error) => write!(f, "failed to request wgpu device: {error}"),
            Self::Surface(error) => write!(f, "wgpu surface error: {error}"),
        }
    }
}

impl std::error::Error for WgpuBackendError {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WgpuRenderStatus {
    Presented,
    SurfaceUnavailable,
}

struct CachedPrimitiveRun {
    geometry: PrimitiveRunGeometry,
    instance_buffer: Option<wgpu::Buffer>,
    geometry_key: GeometryKey,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct GeometryKey {
    cell_w: u32,
    cell_h: u32,
    viewport_w: u32,
    viewport_h: u32,
}

/// Pick the swapchain format.
///
/// `Color` components are display-encoded 0..1 values (`Color::from_hex` is a
/// plain `channel / 255.0`), and the Metal backend writes them straight into a
/// `BGRA8Unorm` drawable. An sRGB swapchain format would make the GPU apply a
/// linear-to-sRGB encode on write, so identical primitives would come out
/// visibly lighter here than on Metal. Prefer a non-sRGB format to keep the two
/// backends showing the same colors.
fn preferred_surface_format(formats: &[wgpu::TextureFormat]) -> Option<wgpu::TextureFormat> {
    formats
        .iter()
        .copied()
        .find(|format| !format.is_srgb())
        .or_else(|| formats.first().copied())
}

/// Device-side state for the solid rect/quad pipeline plus the retained
/// per-run instance-buffer cache. Split out of [`WgpuBackend`] so the same draw
/// walk can be replayed into an offscreen texture by tests.
struct SolidQuadRenderer {
    pipeline: wgpu::RenderPipeline,
    run_cache: HashMap<GpuPrimitiveRunKey, CachedPrimitiveRun>,
}

impl SolidQuadRenderer {
    fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("eseqlisp solid quad shader"),
            source: wgpu::ShaderSource::Wgsl(SOLID_QUAD_SHADER.into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("eseqlisp solid quad pipeline layout"),
            bind_group_layouts: &[],
            push_constant_ranges: &[],
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("eseqlisp solid quad pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("solid_quad_vertex"),
                compilation_options: Default::default(),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<SolidQuadInstance>() as u64,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &[
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x4,
                            offset: 0,
                            shader_location: 0,
                        },
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x4,
                            offset: 16,
                            shader_location: 1,
                        },
                    ],
                }],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("solid_quad_fragment"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: Default::default(),
            multiview: None,
            cache: None,
        });
        Self {
            pipeline,
            run_cache: HashMap::new(),
        }
    }

    /// Rebuild geometry for every run that cannot reuse its cached instances,
    /// and drop cache entries for runs that are no longer in the display list.
    fn prepare(
        &mut self,
        device: &wgpu::Device,
        runs: &[GpuPrimitiveRun],
        cell_w: f32,
        cell_h: f32,
        viewport_w: u32,
        viewport_h: u32,
    ) {
        let geometry_key = GeometryKey {
            cell_w: cell_w.to_bits(),
            cell_h: cell_h.to_bits(),
            viewport_w: (viewport_w as f32).to_bits(),
            viewport_h: (viewport_h as f32).to_bits(),
        };
        let active_keys: HashSet<GpuPrimitiveRunKey> = runs.iter().map(run_key).collect();
        self.run_cache.retain(|key, _| active_keys.contains(key));

        for run in runs {
            let key = run_key(run);
            let can_reuse = run.reused_from_previous
                && self
                    .run_cache
                    .get(&key)
                    .is_some_and(|cached| cached.geometry_key == geometry_key);
            if can_reuse {
                continue;
            }
            let geometry = gpu_geometry::build_primitive_run_geometry(
                run,
                cell_w,
                cell_h,
                viewport_w as f32,
                viewport_h as f32,
            );
            let instance_buffer = (!geometry.instances.is_empty()).then(|| {
                device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("eseqlisp retained primitive run"),
                    contents: bytemuck::cast_slice(&geometry.instances),
                    usage: wgpu::BufferUsages::VERTEX,
                })
            });
            self.run_cache.insert(
                key,
                CachedPrimitiveRun {
                    geometry,
                    instance_buffer,
                    geometry_key,
                },
            );
        }
    }

    /// Record one clear-and-draw pass into `view`. `prepare` must have run for
    /// the same `runs` and viewport first.
    fn encode(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        runs: &[GpuPrimitiveRun],
        cell_w: f32,
        cell_h: f32,
        viewport_w: u32,
        viewport_h: u32,
        clear: Color,
    ) {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("eseqlisp solid primitive pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: clear.r as f64,
                        g: clear.g as f64,
                        b: clear.b as f64,
                        a: clear.a as f64,
                    }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        pass.set_pipeline(&self.pipeline);
        let mut clips = ClipStack::new(ScissorRect::full(viewport_w, viewport_h));
        for run in runs {
            let cached = self
                .run_cache
                .get(&run_key(run))
                .expect("active run was prepared");
            for op in &cached.geometry.ops {
                match op {
                    PrimitiveRunOp::Draw(range) => {
                        let clip = clips.current();
                        if clip.width == 0 || clip.height == 0 || range.is_empty() {
                            continue;
                        }
                        let Some(buffer) = &cached.instance_buffer else {
                            continue;
                        };
                        pass.set_scissor_rect(clip.x, clip.y, clip.width, clip.height);
                        pass.set_vertex_buffer(0, buffer.slice(..));
                        pass.draw(0..6, range.clone());
                    }
                    PrimitiveRunOp::PushClip(rect) => clips.push_cells(*rect, cell_w, cell_h),
                    PrimitiveRunOp::PopClip => clips.pop(),
                }
            }
        }
    }
}

fn run_key(run: &GpuPrimitiveRun) -> GpuPrimitiveRunKey {
    GpuPrimitiveRunKey {
        widget_id: run.widget_id,
        ordinal: run.ordinal,
    }
}

/// A surface-backed renderer. The `Arc<Window>` gives the surface a sound
/// `'static` lifetime without self-referential storage or leaked windows.
pub struct WgpuBackend {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    renderer: SolidQuadRenderer,
}

impl WgpuBackend {
    pub async fn new(window: Arc<Window>) -> Result<Self, WgpuBackendError> {
        let instance = wgpu::Instance::default();
        let surface = instance
            .create_surface(window.clone())
            .map_err(WgpuBackendError::CreateSurface)?;
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
            })
            .await
            .ok_or(WgpuBackendError::NoAdapter)?;
        let (device, queue) = adapter
            .request_device(
                &wgpu::DeviceDescriptor {
                    label: Some("eseqlisp wgpu device"),
                    required_features: wgpu::Features::empty(),
                    required_limits: wgpu::Limits::downlevel_defaults(),
                    memory_hints: wgpu::MemoryHints::Performance,
                },
                None,
            )
            .await
            .map_err(WgpuBackendError::RequestDevice)?;

        let size = window.inner_size();
        let capabilities = surface.get_capabilities(&adapter);
        let format =
            preferred_surface_format(&capabilities.formats).ok_or(WgpuBackendError::NoAdapter)?;
        let alpha_mode = capabilities
            .alpha_modes
            .iter()
            .copied()
            .find(|mode| *mode == wgpu::CompositeAlphaMode::Opaque)
            .unwrap_or(capabilities.alpha_modes[0]);
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode: wgpu::PresentMode::Fifo,
            desired_maximum_frame_latency: 2,
            alpha_mode,
            view_formats: vec![],
        };
        surface.configure(&device, &config);

        let renderer = SolidQuadRenderer::new(&device, format);

        Ok(Self {
            window,
            surface,
            device,
            queue,
            config,
            renderer,
        })
    }

    pub fn window(&self) -> &Window {
        &self.window
    }

    pub fn resize(&mut self, size: PhysicalSize<u32>) {
        if size.width == 0 || size.height == 0 {
            return;
        }
        self.config.width = size.width;
        self.config.height = size.height;
        self.surface.configure(&self.device, &self.config);
    }

    pub fn render_primitive_runs(
        &mut self,
        runs: &[GpuPrimitiveRun],
        cell_w: f32,
        cell_h: f32,
        clear: Color,
    ) -> Result<WgpuRenderStatus, WgpuBackendError> {
        let size = self.window.inner_size();
        if size.width == 0 || size.height == 0 {
            return Ok(WgpuRenderStatus::SurfaceUnavailable);
        }
        if self.config.width != size.width || self.config.height != size.height {
            self.resize(size);
        }

        self.renderer.prepare(
            &self.device,
            runs,
            cell_w,
            cell_h,
            self.config.width,
            self.config.height,
        );

        let frame = match self.surface.get_current_texture() {
            Ok(frame) => frame,
            Err(wgpu::SurfaceError::Timeout) => return Ok(WgpuRenderStatus::SurfaceUnavailable),
            Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => {
                self.surface.configure(&self.device, &self.config);
                return Ok(WgpuRenderStatus::SurfaceUnavailable);
            }
            Err(error) => return Err(WgpuBackendError::Surface(error)),
        };
        let view = frame.texture.create_view(&Default::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("eseqlisp wgpu frame"),
            });
        self.renderer.encode(
            &mut encoder,
            &view,
            runs,
            cell_w,
            cell_h,
            self.config.width,
            self.config.height,
            clear,
        );
        self.queue.submit(Some(encoder.finish()));
        frame.present();
        Ok(WgpuRenderStatus::Presented)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::layout::{LayoutNode, Rect};
    use crate::vm::Value;
    use crate::widget_render::{WidgetViewport, collect_gpu_primitive_runs};

    const VIEWPORT: u32 = 256;
    const CELL: f32 = 8.0;
    /// A deliberate mid-tone: pure primaries sit on the fixed points of the
    /// sRGB transfer curve, so they cannot tell a linear target apart from an
    /// sRGB-encoding one.
    const SCOPE_HEX: &str = "#336699";
    const SCOPE_RGBA: [u8; 4] = [0x33, 0x66, 0x99, 0xff];

    #[test]
    fn surface_format_choice_matches_the_metal_drawable_encoding() {
        // Vulkan on Linux typically reports the sRGB variant first; picking it
        // would brighten every color relative to Metal's BGRA8Unorm drawable.
        assert_eq!(
            preferred_surface_format(&[
                wgpu::TextureFormat::Bgra8UnormSrgb,
                wgpu::TextureFormat::Bgra8Unorm,
            ]),
            Some(wgpu::TextureFormat::Bgra8Unorm)
        );
        assert_eq!(
            preferred_surface_format(&[wgpu::TextureFormat::Rgba16Float]),
            Some(wgpu::TextureFormat::Rgba16Float)
        );
        assert_eq!(preferred_surface_format(&[]), None);
    }

    /// A `box` with a `background` prop is the production source of
    /// `PushClipRect`/`PopClipRect`; a `scope` child is a production source of
    /// solid `Rect` primitives. The scope deliberately overhangs the box on the
    /// right, so the clip has something to cut.
    fn clipping_widget_tree() -> LayoutNode {
        let scope = LayoutNode {
            widget_id: 2,
            stable_widget_id: None,
            subtree_root_id: None,
            parent_subtree_root_id: None,
            stable_key: None,
            widget_type: "scope".into(),
            rect: Rect {
                row: 8.0,
                col: 8.0,
                width: 16.0,
                height: 8.0,
            },
            props: HashMap::from([
                ("background-color".into(), Value::String(SCOPE_HEX.into())),
                ("waveform-color".into(), Value::String(SCOPE_HEX.into())),
            ]),
            children: vec![],
            focusable: false,
            animation: Default::default(),
        };
        LayoutNode {
            widget_id: 1,
            stable_widget_id: None,
            subtree_root_id: None,
            parent_subtree_root_id: None,
            stable_key: None,
            widget_type: "box".into(),
            rect: Rect {
                row: 4.0,
                col: 4.0,
                width: 16.0,
                height: 16.0,
            },
            props: HashMap::from([("background".into(), Value::String("panel".into()))]),
            children: vec![scope],
            focusable: false,
            animation: Default::default(),
        }
    }

    fn test_viewport() -> WidgetViewport {
        WidgetViewport {
            cell_w: CELL,
            cell_h: CELL,
            vp_w: VIEWPORT as f32,
            vp_h: VIEWPORT as f32,
            time_seconds: 0.0,
            focused_widget_id: None,
            focused_branch: false,
            overlay_viewport_bottom: VIEWPORT as f32 / CELL,
            scroll_top: 0.0,
            scroll_left: 0.0,
            inherited_hover: false,
        }
    }

    /// Render `runs` into an offscreen `Rgba8Unorm` texture and read it back.
    /// Returns `None` when the machine exposes no wgpu adapter at all.
    fn render_offscreen(runs: &[GpuPrimitiveRun], clear: Color) -> Option<Vec<u8>> {
        let instance = wgpu::Instance::default();
        let adapter =
            pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))?;
        let (device, queue) = pollster::block_on(adapter.request_device(
            &wgpu::DeviceDescriptor {
                label: Some("eseqlisp wgpu test device"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::downlevel_defaults(),
                memory_hints: wgpu::MemoryHints::Performance,
            },
            None,
        ))
        .expect("wgpu adapter refused a downlevel-defaults device");

        let format = wgpu::TextureFormat::Rgba8Unorm;
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("eseqlisp wgpu test target"),
            size: wgpu::Extent3d {
                width: VIEWPORT,
                height: VIEWPORT,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());

        // 256 px * 4 bytes is already a multiple of COPY_BYTES_PER_ROW_ALIGNMENT.
        let bytes_per_row = VIEWPORT * 4;
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("eseqlisp wgpu test readback"),
            size: (bytes_per_row * VIEWPORT) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        let mut renderer = SolidQuadRenderer::new(&device, format);
        renderer.prepare(&device, runs, CELL, CELL, VIEWPORT, VIEWPORT);
        let mut encoder = device.create_command_encoder(&Default::default());
        renderer.encode(
            &mut encoder,
            &view,
            runs,
            CELL,
            CELL,
            VIEWPORT,
            VIEWPORT,
            clear,
        );
        encoder.copy_texture_to_buffer(
            texture.as_image_copy(),
            wgpu::ImageCopyBuffer {
                buffer: &readback,
                layout: wgpu::ImageDataLayout {
                    offset: 0,
                    bytes_per_row: Some(bytes_per_row),
                    rows_per_image: Some(VIEWPORT),
                },
            },
            wgpu::Extent3d {
                width: VIEWPORT,
                height: VIEWPORT,
                depth_or_array_layers: 1,
            },
        );
        queue.submit(Some(encoder.finish()));

        readback.slice(..).map_async(wgpu::MapMode::Read, |_| {});
        device.poll(wgpu::Maintain::Wait);
        let pixels = readback.slice(..).get_mapped_range().to_vec();
        readback.unmap();
        Some(pixels)
    }

    fn pixel(pixels: &[u8], x: u32, y: u32) -> [u8; 4] {
        let offset = ((y * VIEWPORT + x) * 4) as usize;
        [
            pixels[offset],
            pixels[offset + 1],
            pixels[offset + 2],
            pixels[offset + 3],
        ]
    }

    #[test]
    fn widget_tree_renders_rects_with_production_colors_and_clipping() {
        let tree = clipping_widget_tree();
        let (runs, _) = collect_gpu_primitive_runs(
            &tree,
            test_viewport(),
            0.0,
            (VIEWPORT as f32 / CELL) as u16,
        );
        assert!(
            runs.iter().any(|run| run.widget_type == "scope"),
            "the tree must produce a run of solid rects to render"
        );

        let clear = Color::rgb(0.0, 0.0, 0.0);
        let Some(pixels) = render_offscreen(&runs, clear) else {
            eprintln!("SKIPPED: no wgpu adapter available on this machine");
            return;
        };

        // The box clips to cells (4,4)..(20,20) => px (32,32)..(160,160); the
        // scope's background rect covers cells (8,8)..(24,16) => px
        // (64,64)..(192,128). Only the intersection may be painted.
        assert_eq!(
            pixel(&pixels, 100, 100),
            SCOPE_RGBA,
            "inside both the scope rect and the clip: the authored color, unshifted"
        );
        assert_eq!(
            pixel(&pixels, 176, 100),
            [0, 0, 0, 255],
            "inside the scope rect but right of the clip: must be scissored away"
        );
        assert_eq!(
            pixel(&pixels, 100, 150),
            [0, 0, 0, 255],
            "inside the clip but below the scope rect: nothing to draw"
        );
        assert_eq!(
            pixel(&pixels, 40, 40),
            [0, 0, 0, 255],
            "inside the clip but left of the scope rect: nothing to draw"
        );
    }
}
