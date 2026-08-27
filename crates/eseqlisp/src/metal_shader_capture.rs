//! Deterministic offscreen captures of the production MSL pipelines.
//!
//! This is the Metal reference half of the shader-port comparison
//! (`eseq-linux.25`). It renders the scene set in [`crate::capture_scenes`] —
//! the same buffers, the same draw order, the same clear — through the MSL
//! shader sources [`crate::ui::metal_backend`] actually compiles, and writes
//! the same `<scene>.png` + `manifest.json` layout the WGSL capture in
//! [`crate::shader_capture`] writes. Diffing the two directories therefore
//! isolates the shader language, not the harness.
//!
//! Two deliberate deviations from the live Metal backend, both so the PNG
//! bytes are directly comparable with the wgpu capture:
//!
//! * the color attachment is `RGBA8Unorm` rather than the swapchain's
//!   `BGRA8Unorm`, so no channel swizzle sits between the shader and the file;
//! * there is no window, layer, or drawable — the pass targets a `Shared`
//!   texture that is read straight back with `getBytes`.
//!
//! Neither changes any fragment math: blending, clear value, viewport and
//! 8-bit rounding are identical either way.

use std::collections::BTreeMap;
use std::path::Path;
use std::ptr::NonNull;

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_foundation::NSString;
use objc2_metal::{
    MTLBlendFactor, MTLBuffer, MTLClearColor, MTLCommandBuffer, MTLCommandEncoder, MTLCommandQueue,
    MTLCreateSystemDefaultDevice, MTLDevice, MTLLibrary, MTLLoadAction, MTLOrigin, MTLPixelFormat,
    MTLPrimitiveType, MTLRegion, MTLRenderCommandEncoder, MTLRenderPassDescriptor,
    MTLRenderPipelineDescriptor, MTLRenderPipelineState, MTLResourceOptions, MTLSize,
    MTLStorageMode, MTLStoreAction, MTLTexture, MTLTextureDescriptor, MTLTextureUsage,
};

use crate::capture_scenes::{
    HEIGHT, SCENES, WIDTH, glyph_atlas_pixels, image_pixels, image_vertices,
    live_spectrogram_instances, patch_cable_instances, sha256_hex, spectrogram_smoothed,
    spectrogram_waterfall, text_vertices, waveform_buckets, waveform_instance, wavetable_bank,
    wavetable_instance, widget_instances_for_scene, widget_scene_shader,
};
use crate::ui::metal_backend::{
    DEFAULT_WIDGET_VERTEX_SHADER, IMAGE_SHADER_SRC, LIVE_SPECTROGRAM_SHADER_SRC,
    PATCH_CABLE_SHADER_SRC, PROP_FRAG_SRC, SHADER_SRC, WAVEFORM_SHADER_SRC, WAVETABLE_SHADER_SRC,
    WIDGET_SHADER_PREAMBLE,
};
use crate::widget_render::ShaderBackend;

/// Matches [`crate::shader_capture::SCHEMA_VERSION`]: the two backends write
/// the same manifest schema so one reader handles both.
pub const SCHEMA_VERSION: u32 = 3;

/// The scene list, which is by construction the one
/// [`crate::shader_capture`] renders: both harnesses read
/// [`crate::capture_scenes::SCENES`].
pub fn scene_names() -> &'static [&'static str] {
    SCENES
}

const FORMAT: MTLPixelFormat = MTLPixelFormat::RGBA8Unorm;

/// The same dark, non-neutral clear the WGSL capture uses.
const CLEAR: MTLClearColor = MTLClearColor {
    red: 0.04,
    green: 0.05,
    blue: 0.07,
    alpha: 1.0,
};

type Device = Retained<ProtocolObject<dyn MTLDevice>>;
type Queue = Retained<ProtocolObject<dyn MTLCommandQueue>>;
type Pipeline = Retained<ProtocolObject<dyn MTLRenderPipelineState>>;
type Buffer = Retained<ProtocolObject<dyn MTLBuffer>>;
type Texture = Retained<ProtocolObject<dyn MTLTexture>>;

/// A Metal device plus the readback plumbing shared by every scene.
pub struct MetalCaptureRenderer {
    device: Device,
    queue: Queue,
    device_name: String,
}

impl MetalCaptureRenderer {
    /// Returns `None` when the machine exposes no Metal device, so callers can
    /// skip rather than fail.
    pub fn new() -> Option<Self> {
        let device = MTLCreateSystemDefaultDevice()?;
        let queue = device.newCommandQueue()?;
        let device_name = device.name().to_string();
        Some(Self {
            device,
            queue,
            device_name,
        })
    }

    pub fn device_name(&self) -> &str {
        &self.device_name
    }

    fn library(&self, source: &str) -> Result<Retained<ProtocolObject<dyn MTLLibrary>>, String> {
        self.device
            .newLibraryWithSource_options_error(&NSString::from_str(source), None)
            .map_err(|error| format!("MSL compile failed: {error:?}"))
    }

    /// One pipeline from `vertex`/`fragment` entry points, blending exactly the
    /// way every pipeline in the Metal backend blends.
    fn pipeline(&self, source: &str, vertex: &str, fragment: &str) -> Result<Pipeline, String> {
        let library = self.library(source)?;
        self.pipeline_from(&library, &library, vertex, fragment)
    }

    fn pipeline_from(
        &self,
        vertex_library: &ProtocolObject<dyn MTLLibrary>,
        fragment_library: &ProtocolObject<dyn MTLLibrary>,
        vertex: &str,
        fragment: &str,
    ) -> Result<Pipeline, String> {
        let vertex_fn = vertex_library
            .newFunctionWithName(&NSString::from_str(vertex))
            .ok_or_else(|| format!("missing vertex function {vertex}"))?;
        let fragment_fn = fragment_library
            .newFunctionWithName(&NSString::from_str(fragment))
            .ok_or_else(|| format!("missing fragment function {fragment}"))?;

        let desc = MTLRenderPipelineDescriptor::new();
        desc.setVertexFunction(Some(&vertex_fn));
        desc.setFragmentFunction(Some(&fragment_fn));
        let attach = unsafe { desc.colorAttachments().objectAtIndexedSubscript(0) };
        attach.setPixelFormat(FORMAT);
        attach.setBlendingEnabled(true);
        attach.setSourceRGBBlendFactor(MTLBlendFactor::SourceAlpha);
        attach.setDestinationRGBBlendFactor(MTLBlendFactor::OneMinusSourceAlpha);
        attach.setSourceAlphaBlendFactor(MTLBlendFactor::One);
        attach.setDestinationAlphaBlendFactor(MTLBlendFactor::OneMinusSourceAlpha);
        self.device
            .newRenderPipelineStateWithDescriptor_error(&desc)
            .map_err(|error| format!("pipeline creation failed: {error:?}"))
    }

    /// The widget pipeline for one `widget-<name>` scene, assembled from the
    /// shared preamble the way `compile_widget_pipeline_source` assembles it.
    fn widget_pipeline(&self, vertex: Option<&str>, fragment: &str) -> Result<Pipeline, String> {
        let source = format!(
            "{}{}{}",
            WIDGET_SHADER_PREAMBLE,
            vertex.unwrap_or(DEFAULT_WIDGET_VERTEX_SHADER),
            fragment
        );
        self.pipeline(&source, "widget_vert", "widget_frag")
    }

    fn buffer<T>(&self, data: &[T]) -> Result<Buffer, String> {
        let byte_len = std::mem::size_of_val(data);
        let pointer = NonNull::new(data.as_ptr() as *mut std::ffi::c_void)
            .ok_or_else(|| "empty capture buffer".to_string())?;
        unsafe {
            self.device.newBufferWithBytes_length_options(
                pointer,
                byte_len,
                MTLResourceOptions::StorageModeShared,
            )
        }
        .ok_or_else(|| "buffer allocation failed".to_string())
    }

    /// Upload one sampled texture. The MSL shaders declare their samplers as
    /// `constexpr sampler`, so filtering is fixed in the shader and there is
    /// nothing to bind alongside the texture.
    fn texture(
        &self,
        width: usize,
        height: usize,
        format: MTLPixelFormat,
        bytes_per_pixel: usize,
        pixels: &[u8],
    ) -> Result<Texture, String> {
        let desc = MTLTextureDescriptor::new();
        unsafe {
            desc.setPixelFormat(format);
            desc.setWidth(width);
            desc.setHeight(height);
        }
        desc.setStorageMode(MTLStorageMode::Shared);
        desc.setUsage(MTLTextureUsage::ShaderRead);
        let texture = self
            .device
            .newTextureWithDescriptor(&desc)
            .ok_or_else(|| "texture allocation failed".to_string())?;
        unsafe {
            texture.replaceRegion_mipmapLevel_withBytes_bytesPerRow(
                MTLRegion {
                    origin: MTLOrigin { x: 0, y: 0, z: 0 },
                    size: MTLSize {
                        width,
                        height,
                        depth: 1,
                    },
                },
                0,
                NonNull::new(pixels.as_ptr() as *mut std::ffi::c_void)
                    .ok_or_else(|| "empty texture upload".to_string())?,
                width * bytes_per_pixel,
            );
        }
        Ok(texture)
    }

    /// Render one named scene and return its RGBA8 pixels, row-major from the
    /// top-left — the same shape [`crate::shader_capture::CaptureRenderer`]
    /// returns.
    pub fn render(&self, scene: &str) -> Result<Vec<u8>, String> {
        let target_desc = MTLTextureDescriptor::new();
        unsafe {
            target_desc.setPixelFormat(FORMAT);
            target_desc.setWidth(WIDTH as usize);
            target_desc.setHeight(HEIGHT as usize);
        }
        target_desc.setStorageMode(MTLStorageMode::Shared);
        target_desc.setUsage(MTLTextureUsage::RenderTarget | MTLTextureUsage::ShaderRead);
        let target = self
            .device
            .newTextureWithDescriptor(&target_desc)
            .ok_or_else(|| "capture target allocation failed".to_string())?;

        let pass = MTLRenderPassDescriptor::new();
        let attach = unsafe { pass.colorAttachments().objectAtIndexedSubscript(0) };
        attach.setTexture(Some(&target));
        attach.setLoadAction(MTLLoadAction::Clear);
        attach.setClearColor(CLEAR);
        attach.setStoreAction(MTLStoreAction::Store);

        let command_buffer = self
            .queue
            .commandBuffer()
            .ok_or_else(|| "command buffer allocation failed".to_string())?;
        let encoder = command_buffer
            .renderCommandEncoderWithDescriptor(&pass)
            .ok_or_else(|| "render encoder creation failed".to_string())?;

        self.encode_scene(scene, &encoder)?;

        encoder.endEncoding();
        command_buffer.commit();
        command_buffer.waitUntilCompleted();

        let bytes_per_row = WIDTH as usize * 4;
        let mut pixels = vec![0u8; bytes_per_row * HEIGHT as usize];
        unsafe {
            target.getBytes_bytesPerRow_fromRegion_mipmapLevel(
                NonNull::new(pixels.as_mut_ptr().cast())
                    .ok_or_else(|| "readback buffer is null".to_string())?,
                bytes_per_row,
                MTLRegion {
                    origin: MTLOrigin { x: 0, y: 0, z: 0 },
                    size: MTLSize {
                        width: WIDTH as usize,
                        height: HEIGHT as usize,
                        depth: 1,
                    },
                },
                0,
            );
        }
        Ok(pixels)
    }

    /// The per-scene draw. Buffers are bound to the same indices the MSL entry
    /// points declare: instances at `buffer(0)`, storage data from `buffer(1)`.
    fn encode_scene(
        &self,
        scene: &str,
        encoder: &ProtocolObject<dyn MTLRenderCommandEncoder>,
    ) -> Result<(), String> {
        // Every encoder call below is an `unsafe` Objective-C message in
        // objc2-metal; the buffers and textures they reference are all owned
        // by this function and outlive the encoder.
        unsafe {
            match scene {
                "text" | "proportional-text" => {
                    let vertices = text_vertices();
                    let buffer = self.buffer(&vertices)?;
                    let atlas =
                        self.texture(64, 64, MTLPixelFormat::R8Unorm, 1, &glyph_atlas_pixels())?;
                    let pipeline = if scene == "text" {
                        self.pipeline(SHADER_SRC, "vert", "frag")?
                    } else {
                        // The proportional fragment is its own library; the vertex
                        // function comes from the shared text library, exactly as
                        // the backend pairs them.
                        let text_library = self.library(SHADER_SRC)?;
                        let prop_library = self.library(PROP_FRAG_SRC)?;
                        self.pipeline_from(&text_library, &prop_library, "vert", "prop_frag")?
                    };
                    encoder.setRenderPipelineState(&pipeline);
                    encoder.setVertexBuffer_offset_atIndex(Some(&buffer), 0, 0);
                    encoder.setFragmentTexture_atIndex(Some(&atlas), 0);
                    encoder.drawPrimitives_vertexStart_vertexCount(
                        MTLPrimitiveType::Triangle,
                        0,
                        vertices.len(),
                    );
                }
                "image" => {
                    let vertices = image_vertices();
                    let buffer = self.buffer(&vertices)?;
                    let texture =
                        self.texture(64, 64, MTLPixelFormat::RGBA8Unorm, 4, &image_pixels())?;
                    let pipeline = self.pipeline(IMAGE_SHADER_SRC, "image_vert", "image_frag")?;
                    encoder.setRenderPipelineState(&pipeline);
                    encoder.setVertexBuffer_offset_atIndex(Some(&buffer), 0, 0);
                    encoder.setFragmentTexture_atIndex(Some(&texture), 0);
                    encoder.drawPrimitives_vertexStart_vertexCount(
                        MTLPrimitiveType::Triangle,
                        0,
                        vertices.len(),
                    );
                }
                "patch-cable" => {
                    let cables = patch_cable_instances();
                    let buffer = self.buffer(&cables)?;
                    let pipeline = self.pipeline(
                        PATCH_CABLE_SHADER_SRC,
                        "patch_cable_vert",
                        "patch_cable_frag",
                    )?;
                    encoder.setRenderPipelineState(&pipeline);
                    encoder.setVertexBuffer_offset_atIndex(Some(&buffer), 0, 0);
                    encoder.drawPrimitives_vertexStart_vertexCount_instanceCount(
                        MTLPrimitiveType::Triangle,
                        0,
                        6,
                        cables.len(),
                    );
                }
                "wavetable" => {
                    let instances = [wavetable_instance()];
                    let buffer = self.buffer(&instances)?;
                    let bank = self.buffer(&wavetable_bank())?;
                    let pipeline =
                        self.pipeline(WAVETABLE_SHADER_SRC, "wavetable_vert", "wavetable_frag")?;
                    encoder.setRenderPipelineState(&pipeline);
                    encoder.setVertexBuffer_offset_atIndex(Some(&buffer), 0, 0);
                    encoder.setFragmentBuffer_offset_atIndex(Some(&bank), 0, 1);
                    encoder.drawPrimitives_vertexStart_vertexCount(
                        MTLPrimitiveType::Triangle,
                        0,
                        6,
                    );
                }
                "waveform" => {
                    let instances = [waveform_instance()];
                    let buffer = self.buffer(&instances)?;
                    let buckets = self.buffer(&waveform_buckets())?;
                    let pipeline =
                        self.pipeline(WAVEFORM_SHADER_SRC, "waveform_vert", "waveform_frag")?;
                    encoder.setRenderPipelineState(&pipeline);
                    encoder.setVertexBuffer_offset_atIndex(Some(&buffer), 0, 0);
                    encoder.setFragmentBuffer_offset_atIndex(Some(&buckets), 0, 1);
                    encoder.drawPrimitives_vertexStart_vertexCount(
                        MTLPrimitiveType::Triangle,
                        0,
                        6,
                    );
                }
                "live-spectrogram" => {
                    // One MSL fragment serves both modes and branches on
                    // `in.mode`, where the WGSL port needed a pipeline per entry
                    // point. The vertex function reads `instances[0]`, so each mode
                    // is still its own draw with its own one-instance buffer, and
                    // the draw order matches the WGSL capture: waterfall, then EQ.
                    let instances = live_spectrogram_instances();
                    let waterfall_data = self.buffer(&spectrogram_waterfall())?;
                    let smoothed_data = self.buffer(&spectrogram_smoothed())?;
                    let pipeline = self.pipeline(
                        LIVE_SPECTROGRAM_SHADER_SRC,
                        "live_spectrogram_vert",
                        "live_spectrogram_frag",
                    )?;
                    encoder.setRenderPipelineState(&pipeline);
                    encoder.setFragmentBuffer_offset_atIndex(Some(&waterfall_data), 0, 1);
                    encoder.setFragmentBuffer_offset_atIndex(Some(&smoothed_data), 0, 2);
                    let mut ordered: Vec<_> = instances.iter().filter(|i| i.mode != 1).collect();
                    ordered.extend(instances.iter().filter(|i| i.mode == 1));
                    for instance in ordered {
                        let buffer = self.buffer(std::slice::from_ref(instance))?;
                        encoder.setVertexBuffer_offset_atIndex(Some(&buffer), 0, 0);
                        encoder.drawPrimitives_vertexStart_vertexCount(
                            MTLPrimitiveType::Triangle,
                            0,
                            6,
                        );
                    }
                }
                widget_scene => {
                    let (vertex, fragment) = widget_scene_shader(widget_scene, ShaderBackend::Msl)
                        .ok_or_else(|| format!("unknown capture scene {widget_scene:?}"))?;
                    let widgets = widget_instances_for_scene(widget_scene);
                    let buffer = self.buffer(&widgets)?;
                    let pipeline = self.widget_pipeline(vertex, fragment)?;
                    encoder.setRenderPipelineState(&pipeline);
                    encoder.setVertexBuffer_offset_atIndex(Some(&buffer), 0, 0);
                    encoder.drawPrimitives_vertexStart_vertexCount_instanceCount(
                        MTLPrimitiveType::Triangle,
                        0,
                        6,
                        widgets.len(),
                    );
                }
            }
        }
        Ok(())
    }
}

/// Render every scene and write `<output_dir>/<name>/{<scene>.png, manifest.json}`.
pub fn write_capture(
    renderer: &MetalCaptureRenderer,
    output_dir: &Path,
    name: &str,
) -> Result<(), String> {
    let dir = output_dir.join(name);
    std::fs::create_dir_all(&dir).map_err(|error| error.to_string())?;

    let mut digests = BTreeMap::new();
    for scene in SCENES {
        let pixels = renderer.render(scene)?;
        let image = image::RgbaImage::from_raw(WIDTH, HEIGHT, pixels)
            .ok_or_else(|| format!("{scene}: readback is not WIDTH * HEIGHT RGBA pixels"))?;
        let path = dir.join(format!("{scene}.png"));
        image.save(&path).map_err(|error| error.to_string())?;
        let bytes = std::fs::read(&path).map_err(|error| error.to_string())?;
        digests.insert((*scene).to_string(), sha256_hex(&bytes));
    }

    let manifest = serde_json::json!({
        "schema_version": SCHEMA_VERSION,
        "capture_name": name,
        "backend": "msl",
        "width": WIDTH,
        "height": HEIGHT,
        "adapter": renderer.device_name(),
        "adapter_backend": "Metal",
        "scenes": SCENES,
        "png_sha256": digests,
    });
    std::fs::write(
        dir.join("manifest.json"),
        format!(
            "{}\n",
            serde_json::to_string_pretty(&manifest).map_err(|error| error.to_string())?
        ),
    )
    .map_err(|error| error.to_string())
}
