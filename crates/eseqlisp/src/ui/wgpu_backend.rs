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

/// A surface-backed renderer. The `Arc<Window>` gives the surface a sound
/// `'static` lifetime without self-referential storage or leaked windows.
pub struct WgpuBackend {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    pipeline: wgpu::RenderPipeline,
    run_cache: HashMap<GpuPrimitiveRunKey, CachedPrimitiveRun>,
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
        let format = capabilities
            .formats
            .iter()
            .copied()
            .find(wgpu::TextureFormat::is_srgb)
            .unwrap_or(capabilities.formats[0]);
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

        Ok(Self {
            window,
            surface,
            device,
            queue,
            config,
            pipeline,
            run_cache: HashMap::new(),
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

        let geometry_key = GeometryKey {
            cell_w: cell_w.to_bits(),
            cell_h: cell_h.to_bits(),
            viewport_w: (self.config.width as f32).to_bits(),
            viewport_h: (self.config.height as f32).to_bits(),
        };
        let active_keys: HashSet<GpuPrimitiveRunKey> = runs
            .iter()
            .map(|run| GpuPrimitiveRunKey {
                widget_id: run.widget_id,
                ordinal: run.ordinal,
            })
            .collect();
        self.run_cache.retain(|key, _| active_keys.contains(key));

        for run in runs {
            let key = GpuPrimitiveRunKey {
                widget_id: run.widget_id,
                ordinal: run.ordinal,
            };
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
                self.config.width as f32,
                self.config.height as f32,
            );
            let instance_buffer = (!geometry.instances.is_empty()).then(|| {
                self.device
                    .create_buffer_init(&wgpu::util::BufferInitDescriptor {
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
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("eseqlisp solid primitive pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
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
            let mut clips =
                ClipStack::new(ScissorRect::full(self.config.width, self.config.height));
            for run in runs {
                let key = GpuPrimitiveRunKey {
                    widget_id: run.widget_id,
                    ordinal: run.ordinal,
                };
                let cached = self.run_cache.get(&key).expect("active run was prepared");
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
                        PrimitiveRunOp::PushClip(rect) => {
                            clips.push_cells(*rect, cell_w, cell_h);
                        }
                        PrimitiveRunOp::PopClip => clips.pop(),
                    }
                }
            }
        }
        self.queue.submit(Some(encoder.finish()));
        frame.present();
        Ok(WgpuRenderStatus::Presented)
    }
}
