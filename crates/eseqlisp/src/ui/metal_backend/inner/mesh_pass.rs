//! The offscreen pass of mesh widgets (widget_render/mesh_widget.rs). Each
//! instance's draw job is rendered with its widget's `mesh_vert` and
//! `mesh_frag` into a 4x multisampled colour target with a depth buffer,
//! resolved into a texture; `draw_image_primitives` then composites that
//! texture into the widget's rect through the image pipeline, so ordinary
//! widgets render exactly as before.
//!
//! The job renders in its own command buffer, committed while the frame is
//! still being encoded: the queue runs command buffers in commit order, so
//! the texture is finished before the frame that samples it.

use super::*;
use crate::widget_render::mesh_widget::{self, MeshDrawJob, MeshUniforms};
use objc2_metal::{
    MTLCompareFunction, MTLCullMode, MTLDepthStencilDescriptor, MTLDepthStencilState,
    MTLTextureType,
};

const MESH_SAMPLE_COUNT: usize = 4;
const MESH_DEPTH_FORMAT: MTLPixelFormat = MTLPixelFormat::Depth32Float;
/// The most `setVertexBytes` / `setFragmentBytes` may pass.
const SET_BYTES_LIMIT: usize = 4096;
/// Frames a target or geometry may go unused before it is dropped.
const MESH_IDLE_FRAMES: u64 = 600;

type Buffer = Retained<ProtocolObject<dyn MTLBuffer>>;
type Texture = Retained<ProtocolObject<dyn MTLTexture>>;
type Pipeline = Retained<ProtocolObject<dyn MTLRenderPipelineState>>;

#[derive(Default)]
pub(super) struct MeshPassState {
    /// Widget -> the source its pipeline was built from (None: it failed).
    pipelines: HashMap<String, (String, Option<Pipeline>)>,
    depth_state: Option<Retained<ProtocolObject<dyn MTLDepthStencilState>>>,
    geometry: HashMap<u64, MeshGpuGeometry>,
    targets: HashMap<u64, MeshTarget>,
    frame: u64,
}

/// One geometry's static buffers, uploaded once.
#[derive(Clone)]
struct MeshGpuGeometry {
    indices: Buffer,
    positions: Buffer,
    normals: Buffer,
    table: Buffer,
    index_count: usize,
    last_used_frame: u64,
}

/// One instance's offscreen target.
#[derive(Clone)]
pub(super) struct MeshTarget {
    color: Texture,
    depth: Texture,
    pub(super) resolved: Texture,
    width: u32,
    height: u32,
    rendered_revision: Option<u64>,
    rendered_time: f32,
    last_used_frame: u64,
}

/// `data` in a new shared buffer; an empty slice gets a small zeroed one,
/// since a shader may still bind it.
fn shared_buffer<T: Copy>(device: &ProtocolObject<dyn MTLDevice>, data: &[T]) -> Option<Buffer> {
    let bytes = std::mem::size_of_val(data);
    if bytes == 0 {
        return device.newBufferWithLength_options(16, MTLResourceOptions(0));
    }
    unsafe {
        device.newBufferWithBytes_length_options(
            NonNull::new(data.as_ptr() as *mut core::ffi::c_void)?,
            bytes,
            MTLResourceOptions(0),
        )
    }
}

impl MetalBackend {
    /// Start a frame of the mesh pass: drop targets, geometry and jobs that
    /// have gone unused.
    pub(super) fn begin_mesh_frame(&mut self) {
        let state = &mut self.mesh;
        state.frame += 1;
        let frame = state.frame;
        let before = state.targets.len();
        state.targets.retain(|_, target| frame - target.last_used_frame <= MESH_IDLE_FRAMES);
        state.geometry.retain(|_, geometry| frame - geometry.last_used_frame <= MESH_IDLE_FRAMES);
        if state.targets.len() != before {
            let targets = &state.targets;
            mesh_widget::retain_mesh_draw_jobs(|widget_id| targets.contains_key(&widget_id));
            // A job is only made when its widget's scene is built, so a cached
            // scene replaying a dropped job's image would draw nothing.
            self.clear_widget_scene_caches();
        }
    }

    fn mesh_pipeline(&mut self, widget_type: &str) -> Option<Pipeline> {
        let def = mesh_widget::mesh_widget_def(widget_type)?;
        if let Some((source, pipeline)) = self.mesh.pipelines.get(widget_type)
            && *source == def.shader_source
        {
            return pipeline.clone();
        }
        let pipeline = self
            .compile_mesh_pipeline(&def.shader_source)
            .map_err(|error| eprintln!("mesh widget '{widget_type}': {error}"))
            .ok();
        self.mesh
            .pipelines
            .insert(widget_type.to_string(), (def.shader_source.clone(), pipeline.clone()));
        pipeline
    }

    pub(super) fn compile_mesh_pipeline(&self, source: &str) -> Result<Pipeline, String> {
        let library = self
            .device
            .newLibraryWithSource_options_error(&NSString::from_str(source), None)
            .map_err(|error| format!("shader compilation failed: {error:?}"))?;
        let function = |name: &str| {
            library
                .newFunctionWithName(&NSString::from_str(name))
                .ok_or_else(|| format!("no {name} in the shader"))
        };
        let desc = MTLRenderPipelineDescriptor::new();
        let (vertex, fragment) = (function("mesh_vert")?, function("mesh_frag")?);
        desc.setVertexFunction(Some(&vertex));
        desc.setFragmentFunction(Some(&fragment));
        desc.setRasterSampleCount(MESH_SAMPLE_COUNT);
        desc.setDepthAttachmentPixelFormat(MESH_DEPTH_FORMAT);
        let attach = unsafe { desc.colorAttachments().objectAtIndexedSubscript(0) };
        attach.setPixelFormat(MTLPixelFormat::BGRA8Unorm);
        attach.setBlendingEnabled(true);
        attach.setSourceRGBBlendFactor(MTLBlendFactor::SourceAlpha);
        attach.setDestinationRGBBlendFactor(MTLBlendFactor::OneMinusSourceAlpha);
        attach.setSourceAlphaBlendFactor(MTLBlendFactor::One);
        attach.setDestinationAlphaBlendFactor(MTLBlendFactor::OneMinusSourceAlpha);
        self.device
            .newRenderPipelineStateWithDescriptor_error(&desc)
            .map_err(|error| format!("pipeline creation failed: {error:?}"))
    }

    fn mesh_depth_state(&mut self) -> Option<Retained<ProtocolObject<dyn MTLDepthStencilState>>> {
        if self.mesh.depth_state.is_none() {
            let desc = MTLDepthStencilDescriptor::new();
            desc.setDepthCompareFunction(MTLCompareFunction::Less);
            desc.setDepthWriteEnabled(true);
            self.mesh.depth_state = self.device.newDepthStencilStateWithDescriptor(&desc);
        }
        self.mesh.depth_state.clone()
    }

    fn mesh_geometry_buffers(&mut self, id: u64) -> Option<MeshGpuGeometry> {
        let frame = self.mesh.frame;
        if let Some(buffers) = self.mesh.geometry.get_mut(&id) {
            buffers.last_used_frame = frame;
            return Some(buffers.clone());
        }
        let geometry = mesh_widget::geometry(id)?;
        let buffers = MeshGpuGeometry {
            indices: shared_buffer(&self.device, &geometry.indices)?,
            positions: shared_buffer(&self.device, &geometry.positions)?,
            normals: shared_buffer(&self.device, &geometry.normals)?,
            table: shared_buffer(&self.device, &geometry.table)?,
            index_count: geometry.indices.len(),
            last_used_frame: frame,
        };
        self.mesh.geometry.insert(id, buffers.clone());
        Some(buffers)
    }

    /// A target of `width` x `height` pixels; `storage` is the resolved
    /// texture's (Private to composite, Shared to read back in tests).
    pub(super) fn new_mesh_target(&self, width: u32, height: u32, storage: MTLStorageMode) -> Option<MeshTarget> {
        let texture = |format, samples: usize, usage, storage| {
            let desc = MTLTextureDescriptor::new();
            desc.setPixelFormat(format);
            unsafe {
                desc.setWidth(width as usize);
                desc.setHeight(height as usize);
                if samples > 1 {
                    desc.setTextureType(MTLTextureType::Type2DMultisample);
                    desc.setSampleCount(samples);
                }
            }
            desc.setStorageMode(storage);
            desc.setUsage(usage);
            self.device.newTextureWithDescriptor(&desc)
        };
        let render = MTLTextureUsage::RenderTarget;
        Some(MeshTarget {
            color: texture(MTLPixelFormat::BGRA8Unorm, MESH_SAMPLE_COUNT, render, MTLStorageMode::Private)?,
            depth: texture(MESH_DEPTH_FORMAT, MESH_SAMPLE_COUNT, render, MTLStorageMode::Private)?,
            resolved: texture(MTLPixelFormat::BGRA8Unorm, 1, render | MTLTextureUsage::ShaderRead, storage)?,
            width,
            height,
            rendered_revision: None,
            rendered_time: 0.0,
            last_used_frame: self.mesh.frame,
        })
    }

    /// Encode `job` into `target` on `command_buffer`; `itime` replaces the
    /// job's shader time (an animating widget's frame time). False when its
    /// widget, pipeline or geometry is missing.
    pub(super) fn encode_mesh_job(
        &mut self,
        command_buffer: &ProtocolObject<dyn MTLCommandBuffer>,
        job: &MeshDrawJob,
        target: &MeshTarget,
        itime: Option<f32>,
    ) -> bool {
        let (Some(pipeline), Some(depth_state), Some(geometry)) = (
            self.mesh_pipeline(&job.widget_type),
            self.mesh_depth_state(),
            self.mesh_geometry_buffers(job.geometry_id),
        ) else {
            return false;
        };
        let mut uniforms: MeshUniforms = job.uniforms;
        if let Some(itime) = itime {
            uniforms.view[1] = itime;
        }
        // Arrays past setBytes' 4 KB limit go in a buffer of their own.
        let array_bytes = std::mem::size_of_val(job.arrays.as_slice());
        let array_buffer = if array_bytes > SET_BYTES_LIMIT {
            let Some(buffer) = shared_buffer(&self.device, &job.arrays) else {
                return false;
            };
            Some(buffer)
        } else {
            None
        };

        let pass = MTLRenderPassDescriptor::new();
        let color = unsafe { pass.colorAttachments().objectAtIndexedSubscript(0) };
        color.setTexture(Some(&target.color));
        color.setResolveTexture(Some(&target.resolved));
        color.setLoadAction(MTLLoadAction::Clear);
        let [red, green, blue, alpha] = job.clear_color.map(f64::from);
        color.setClearColor(MTLClearColor { red, green, blue, alpha });
        color.setStoreAction(MTLStoreAction::MultisampleResolve);
        let depth = pass.depthAttachment();
        depth.setTexture(Some(&target.depth));
        depth.setLoadAction(MTLLoadAction::Clear);
        depth.setClearDepth(1.0);
        depth.setStoreAction(MTLStoreAction::DontCare);

        let Some(enc) = command_buffer.renderCommandEncoderWithDescriptor(&pass) else {
            return false;
        };
        enc.setRenderPipelineState(&pipeline);
        enc.setDepthStencilState(Some(&depth_state));
        enc.setCullMode(MTLCullMode::None);
        // An empty array block still binds something the shader may name.
        let no_arrays = [0.0_f32; 4];
        let arrays: &[f32] = if job.arrays.is_empty() { &no_arrays } else { &job.arrays };
        unsafe {
            for (index, buffer) in
                [&geometry.indices, &geometry.positions, &geometry.normals, &geometry.table]
                    .into_iter()
                    .enumerate()
            {
                enc.setVertexBuffer_offset_atIndex(Some(buffer), 0, index);
            }
            let uniforms_ptr = NonNull::from(&uniforms).cast();
            let uniforms_len = std::mem::size_of::<MeshUniforms>();
            enc.setVertexBytes_length_atIndex(uniforms_ptr, uniforms_len, 4);
            enc.setFragmentBytes_length_atIndex(uniforms_ptr, uniforms_len, 0);
            match &array_buffer {
                Some(buffer) => {
                    enc.setVertexBuffer_offset_atIndex(Some(buffer), 0, 5);
                    enc.setFragmentBuffer_offset_atIndex(Some(buffer), 0, 1);
                }
                None => {
                    let arrays_ptr = NonNull::new(arrays.as_ptr() as *mut core::ffi::c_void).unwrap();
                    enc.setVertexBytes_length_atIndex(arrays_ptr, std::mem::size_of_val(arrays), 5);
                    enc.setFragmentBytes_length_atIndex(arrays_ptr, std::mem::size_of_val(arrays), 1);
                }
            }
            enc.drawPrimitives_vertexStart_vertexCount(MTLPrimitiveType::Triangle, 0, geometry.index_count);
        }
        enc.endEncoding();
        self.stats.note_draw_command();
        true
    }

    /// The texture instance `widget_id` composites, and its size: its
    /// target, rendered first when its job changed (or, for a widget that
    /// animates, at each new frame time).
    pub(super) fn mesh_target_texture(&mut self, widget_id: u64, time_seconds: f32) -> Option<(Texture, u32, u32)> {
        let job = mesh_widget::mesh_draw_job(widget_id)?;
        let fits = self
            .mesh
            .targets
            .get(&widget_id)
            .is_some_and(|target| target.width == job.width_px && target.height == job.height_px);
        if !fits {
            let target = self.new_mesh_target(job.width_px, job.height_px, MTLStorageMode::Private)?;
            self.mesh.targets.insert(widget_id, target);
        }
        let frame = self.mesh.frame;
        let target = self.mesh.targets.get_mut(&widget_id)?;
        target.last_used_frame = frame;
        let stale = target.rendered_revision != Some(job.revision)
            || (job.animates && target.rendered_time != time_seconds);
        let result = (target.resolved.clone(), target.width, target.height);
        if stale {
            let target = target.clone();
            let command_buffer = self.command_queue.commandBuffer()?;
            let itime = job.animates.then_some(time_seconds);
            let encoded = self.encode_mesh_job(&command_buffer, &job, &target, itime);
            command_buffer.commit();
            if !encoded {
                return None;
            }
            let target = self.mesh.targets.get_mut(&widget_id)?;
            target.rendered_revision = Some(job.revision);
            target.rendered_time = time_seconds;
        }
        Some(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::widget_render::mesh_widget::{MeshGeometry, MeshWidgetDef, register_geometry, register_mesh_widget};

    fn parse(src: &str) -> crate::parser::Expression {
        let tokens = crate::parser::Parser::new(src.to_string()).parse().unwrap();
        crate::parser::ASTParser::new(tokens).parse().unwrap().into_iter().next().unwrap()
    }

    /// BGRA pixel at (`x`, `y`) of a `width`-wide readback.
    fn pixel(pixels: &[u8], width: usize, x: usize, y: usize) -> [u8; 4] {
        let i = (y * width + x) * 4;
        [pixels[i], pixels[i + 1], pixels[i + 2], pixels[i + 3]]
    }

    #[test]
    fn offscreen_mesh_pass_draws_depth_tested_triangles() {
        let Ok(mut backend) = MetalBackend::new_capture(64, 64) else {
            return; // no Metal device
        };
        // Clip position straight from the vertex; red where z < 0.5, blue beyond.
        let vertex = parse("(out :position (vec4 position 1.0) :varyings ((z (dot position (vec3 0 0 1)))))");
        let fragment = parse("(if (< z 0.5) (rgba 1 0 0 1) (rgba 0 0 1 1))");
        let output = crate::lang::sdf_codegen::compile_mesh_to_metal(&vertex, &fragment, &[], &[]).unwrap();
        register_mesh_widget(MeshWidgetDef {
            name: "mesh-pass-test".into(),
            shader_source: output.shader_source,
            vertex_expr: vertex,
            fragment_expr: fragment,
            state_uniforms: Vec::new(),
            arrays: Vec::new(),
            animates: false,
        });
        // A near quad (z 0.25) over the middle, then a far one (z 0.75)
        // reaching the lower left: drawn second, the far quad would cover
        // the near one without the depth test.
        let quad = |x0: f32, y0: f32, x1: f32, y1: f32, z: f32| [x0, y0, z, x1, y0, z, x1, y1, z, x0, y1, z];
        let positions: Vec<f32> = quad(-0.6, -0.6, 0.6, 0.6, 0.25)
            .into_iter()
            .chain(quad(-0.9, -0.9, 0.3, 0.3, 0.75))
            .collect();
        let indices = vec![0, 1, 2, 0, 2, 3, 4, 5, 6, 4, 6, 7];
        let geometry = MeshGeometry::new(positions, None, indices, Vec::new(), 0, Vec::new(), None).unwrap();
        let mut uniforms = MeshUniforms::default();
        uniforms.target_size = [64.0, 64.0, 0.0, 0.0];
        let job = MeshDrawJob {
            widget_type: "mesh-pass-test".into(),
            geometry_id: register_geometry(geometry),
            width_px: 64,
            height_px: 64,
            uniforms,
            arrays: Vec::new(),
            clear_color: [0.0; 4],
            revision: 1,
            animates: false,
        };
        let target = backend.new_mesh_target(64, 64, MTLStorageMode::Shared).expect("mesh target");
        let command_buffer = backend.command_queue.commandBuffer().expect("command buffer");
        assert!(backend.encode_mesh_job(&command_buffer, &job, &target, None));
        command_buffer.commit();
        command_buffer.waitUntilCompleted();
        assert_ne!(command_buffer.status(), MTLCommandBufferStatus::Error);

        let mut pixels = vec![0u8; 64 * 64 * 4];
        unsafe {
            target.resolved.getBytes_bytesPerRow_fromRegion_mipmapLevel(
                NonNull::new(pixels.as_mut_ptr().cast()).unwrap(),
                64 * 4,
                MTLRegion {
                    origin: MTLOrigin { x: 0, y: 0, z: 0 },
                    size: MTLSize { width: 64, height: 64, depth: 1 },
                },
                0,
            );
        }
        // NDC y is up, rows run down: NDC (x, y) is pixel ((x+1)*32, (1-y)*32).
        let red = [0, 0, 255, 255];
        let blue = [255, 0, 0, 255];
        assert_eq!(pixel(&pixels, 64, 32, 32), red, "the near quad wins where both cover");
        assert_eq!(pixel(&pixels, 64, 6, 58), blue, "the far quad shows where it alone covers");
        assert_eq!(pixel(&pixels, 64, 60, 4), [0; 4], "uncovered pixels keep the clear colour");
        let covered = pixels.chunks_exact(4).filter(|p| p[3] > 0).count();
        assert!(covered > 64 * 64 / 3, "{covered} covered pixels");
        // Edges are multisampled: some pixels are partly covered.
        assert!(pixels.chunks_exact(4).any(|p| p[3] > 0 && p[3] < 255), "no antialiased edge pixels");
    }
}
