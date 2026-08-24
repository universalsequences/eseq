//! wgpu render pipelines for the ported core shaders.
//!
//! One builder per MSL pipeline in [`crate::ui::metal_backend`], using the WGSL
//! in [`crate::ui::wgsl_shaders`]. The vertex buffer layouts here are the
//! contract between the `#[repr(C)]` uploads in
//! [`crate::ui::gpu_geometry`] and the shader-side attribute declarations:
//! every offset below is the Rust struct's field offset, so the same bytes the
//! Metal backend hands to `device const T*` are read unchanged.
//!
//! Instance data goes through vertex buffers rather than storage buffers on
//! purpose. WGSL's storage layout rules would force 16-byte alignment on every
//! `vec4<f32>` member and change the shared structs; vertex attributes only
//! require 4-byte offsets, which the packed layouts already satisfy.

use wgpu::VertexAttribute;

use crate::ui::gpu_geometry::{
    ImageVertex, LiveSpectrogramInstance, PatchCableInstance, Vertex, WaveformInstance,
    WavetableInstance,
};
use crate::ui::wgsl_shaders;
use crate::widget_render::WidgetInstance;

/// Every core pipeline blends the same way the Metal backend does:
/// `srcAlpha, 1-srcAlpha` for color and `one, 1-srcAlpha` for alpha.
const BLEND: wgpu::BlendState = wgpu::BlendState::ALPHA_BLENDING;

const fn attribute(offset: u64, location: u32, format: wgpu::VertexFormat) -> VertexAttribute {
    VertexAttribute {
        format,
        offset,
        shader_location: location,
    }
}

/// [`Vertex`] — position, uv, fg, bg, stepped per vertex.
pub const TEXT_VERTEX_ATTRIBUTES: [VertexAttribute; 4] = [
    attribute(0, 0, wgpu::VertexFormat::Float32x2),
    attribute(8, 1, wgpu::VertexFormat::Float32x2),
    attribute(16, 2, wgpu::VertexFormat::Float32x4),
    attribute(32, 3, wgpu::VertexFormat::Float32x4),
];

/// [`ImageVertex`], stepped per vertex.
pub const IMAGE_VERTEX_ATTRIBUTES: [VertexAttribute; 8] = [
    attribute(0, 0, wgpu::VertexFormat::Float32x2),
    attribute(8, 1, wgpu::VertexFormat::Float32x2),
    attribute(16, 2, wgpu::VertexFormat::Float32),
    attribute(20, 3, wgpu::VertexFormat::Float32x2),
    attribute(28, 4, wgpu::VertexFormat::Float32x2),
    attribute(36, 5, wgpu::VertexFormat::Float32),
    attribute(40, 6, wgpu::VertexFormat::Float32),
    attribute(44, 7, wgpu::VertexFormat::Float32),
];

/// [`PatchCableInstance`], stepped per instance. Adjacent `[f32; 2]` pairs are
/// fetched as one `Float32x4` to keep the attribute count down.
pub const PATCH_CABLE_ATTRIBUTES: [VertexAttribute; 6] = [
    attribute(0, 0, wgpu::VertexFormat::Float32x4),
    attribute(16, 1, wgpu::VertexFormat::Float32x4),
    attribute(32, 2, wgpu::VertexFormat::Float32x4),
    attribute(48, 3, wgpu::VertexFormat::Float32x4),
    attribute(64, 4, wgpu::VertexFormat::Float32x4),
    attribute(80, 5, wgpu::VertexFormat::Float32x4),
];

/// [`WidgetInstance`], stepped per instance. One attribute per field: fifteen
/// still fits under WebGPU's sixteen-attribute floor, and the 1:1 mapping keeps
/// the shared preamble readable next to its MSL original.
pub const WIDGET_ATTRIBUTES: [VertexAttribute; 15] = [
    attribute(0, 0, wgpu::VertexFormat::Float32x2),
    attribute(8, 1, wgpu::VertexFormat::Float32x2),
    attribute(16, 2, wgpu::VertexFormat::Float32),
    attribute(20, 3, wgpu::VertexFormat::Float32),
    attribute(24, 4, wgpu::VertexFormat::Float32),
    attribute(28, 5, wgpu::VertexFormat::Float32x4),
    attribute(44, 6, wgpu::VertexFormat::Float32x4),
    attribute(60, 7, wgpu::VertexFormat::Float32x4),
    attribute(76, 8, wgpu::VertexFormat::Float32x4),
    attribute(92, 9, wgpu::VertexFormat::Float32x4),
    attribute(108, 10, wgpu::VertexFormat::Float32x4),
    attribute(124, 11, wgpu::VertexFormat::Float32x4),
    attribute(140, 12, wgpu::VertexFormat::Float32x4),
    attribute(156, 13, wgpu::VertexFormat::Float32),
    attribute(160, 14, wgpu::VertexFormat::Float32),
];

/// [`WavetableInstance`], stepped per instance.
pub const WAVETABLE_ATTRIBUTES: [VertexAttribute; 10] = [
    attribute(0, 0, wgpu::VertexFormat::Float32x4),
    attribute(16, 1, wgpu::VertexFormat::Float32x2),
    attribute(24, 2, wgpu::VertexFormat::Uint32),
    attribute(28, 3, wgpu::VertexFormat::Uint32),
    attribute(32, 4, wgpu::VertexFormat::Uint32),
    attribute(36, 5, wgpu::VertexFormat::Float32x3),
    attribute(48, 6, wgpu::VertexFormat::Uint32),
    attribute(52, 7, wgpu::VertexFormat::Float32x4),
    attribute(68, 8, wgpu::VertexFormat::Float32x4),
    attribute(84, 9, wgpu::VertexFormat::Float32x4),
];

/// [`WaveformInstance`], stepped per instance.
pub const WAVEFORM_ATTRIBUTES: [VertexAttribute; 15] = [
    attribute(0, 0, wgpu::VertexFormat::Float32x4),
    attribute(16, 1, wgpu::VertexFormat::Float32x2),
    attribute(24, 2, wgpu::VertexFormat::Uint32),
    attribute(28, 3, wgpu::VertexFormat::Float32x3),
    attribute(40, 4, wgpu::VertexFormat::Sint32x2),
    attribute(48, 5, wgpu::VertexFormat::Float32),
    attribute(52, 6, wgpu::VertexFormat::Sint32),
    attribute(56, 7, wgpu::VertexFormat::Float32x4),
    attribute(72, 8, wgpu::VertexFormat::Float32x4),
    attribute(88, 9, wgpu::VertexFormat::Float32x4),
    attribute(104, 10, wgpu::VertexFormat::Float32x4),
    attribute(120, 11, wgpu::VertexFormat::Sint32x2),
    attribute(128, 12, wgpu::VertexFormat::Float32x4),
    attribute(144, 13, wgpu::VertexFormat::Float32x4),
    attribute(160, 14, wgpu::VertexFormat::Float32x4),
];

/// [`LiveSpectrogramInstance`], stepped per instance.
pub const LIVE_SPECTROGRAM_ATTRIBUTES: [VertexAttribute; 12] = [
    attribute(0, 0, wgpu::VertexFormat::Float32x4),
    attribute(16, 1, wgpu::VertexFormat::Float32x2),
    attribute(24, 2, wgpu::VertexFormat::Uint32x4),
    attribute(40, 3, wgpu::VertexFormat::Uint32),
    attribute(44, 4, wgpu::VertexFormat::Float32),
    attribute(48, 5, wgpu::VertexFormat::Float32x2),
    attribute(64, 6, wgpu::VertexFormat::Float32x4),
    attribute(80, 7, wgpu::VertexFormat::Float32x4),
    attribute(96, 8, wgpu::VertexFormat::Float32x4),
    attribute(112, 9, wgpu::VertexFormat::Float32x4),
    attribute(128, 10, wgpu::VertexFormat::Float32x4),
    attribute(144, 11, wgpu::VertexFormat::Float32x4),
];

fn buffer_layout<T>(
    step_mode: wgpu::VertexStepMode,
    attributes: &[VertexAttribute],
) -> wgpu::VertexBufferLayout<'_> {
    wgpu::VertexBufferLayout {
        array_stride: std::mem::size_of::<T>() as u64,
        step_mode,
        attributes,
    }
}

/// Bind group layout for the two text pipelines and the image pipeline: a
/// sampled texture plus its sampler, replacing MSL's inline `constexpr sampler`.
pub fn texture_bind_group_layout(device: &wgpu::Device, label: &str) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some(label),
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
    })
}

/// Bind group layout for the sample-data pipelines: `count` read-only storage
/// buffers, one per MSL `device const float*` argument.
pub fn storage_bind_group_layout(
    device: &wgpu::Device,
    label: &str,
    count: u32,
) -> wgpu::BindGroupLayout {
    let entries: Vec<_> = (0..count)
        .map(|binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only: true },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        })
        .collect();
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some(label),
        entries: &entries,
    })
}

/// Shared pipeline construction. `bind_group_layouts` is empty for the
/// pipelines whose MSL took no textures or data buffers.
#[allow(clippy::too_many_arguments)]
fn build(
    device: &wgpu::Device,
    label: &str,
    vertex_module: &wgpu::ShaderModule,
    vertex_entry: &str,
    fragment_module: &wgpu::ShaderModule,
    fragment_entry: &str,
    buffers: &[wgpu::VertexBufferLayout<'_>],
    bind_group_layouts: &[&wgpu::BindGroupLayout],
    format: wgpu::TextureFormat,
) -> wgpu::RenderPipeline {
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some(label),
        bind_group_layouts,
        push_constant_ranges: &[],
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: vertex_module,
            entry_point: Some(vertex_entry),
            compilation_options: Default::default(),
            buffers,
        },
        fragment: Some(wgpu::FragmentState {
            module: fragment_module,
            entry_point: Some(fragment_entry),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend: Some(BLEND),
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
    })
}

fn module(device: &wgpu::Device, label: &str, source: &str) -> wgpu::ShaderModule {
    device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some(label),
        source: wgpu::ShaderSource::Wgsl(source.into()),
    })
}

/// The monospace and proportional text pipelines. Both use `text_vert`; they
/// differ in fragment stage and in the sampler the caller binds — nearest for
/// monospace cells, linear for sub-pixel-positioned proportional glyphs.
pub fn text_pipelines(
    device: &wgpu::Device,
    atlas_layout: &wgpu::BindGroupLayout,
    format: wgpu::TextureFormat,
) -> (wgpu::RenderPipeline, wgpu::RenderPipeline) {
    let text = module(device, "eseqlisp text shader", wgsl_shaders::TEXT_SHADER_WGSL);
    let prop = module(
        device,
        "eseqlisp proportional text shader",
        wgsl_shaders::PROP_TEXT_SHADER_WGSL,
    );
    let buffers = [buffer_layout::<Vertex>(
        wgpu::VertexStepMode::Vertex,
        &TEXT_VERTEX_ATTRIBUTES,
    )];
    (
        build(
            device,
            "eseqlisp text pipeline",
            &text,
            "text_vert",
            &text,
            "text_frag",
            &buffers,
            &[atlas_layout],
            format,
        ),
        build(
            device,
            "eseqlisp proportional text pipeline",
            &text,
            "text_vert",
            &prop,
            "prop_text_frag",
            &buffers,
            &[atlas_layout],
            format,
        ),
    )
}

pub fn image_pipeline(
    device: &wgpu::Device,
    image_layout: &wgpu::BindGroupLayout,
    format: wgpu::TextureFormat,
) -> wgpu::RenderPipeline {
    let shader = module(device, "eseqlisp image shader", wgsl_shaders::IMAGE_SHADER_WGSL);
    build(
        device,
        "eseqlisp image pipeline",
        &shader,
        "image_vert",
        &shader,
        "image_frag",
        &[buffer_layout::<ImageVertex>(
            wgpu::VertexStepMode::Vertex,
            &IMAGE_VERTEX_ATTRIBUTES,
        )],
        &[image_layout],
        format,
    )
}

pub fn patch_cable_pipeline(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
) -> wgpu::RenderPipeline {
    let shader = module(
        device,
        "eseqlisp patch cable shader",
        wgsl_shaders::PATCH_CABLE_SHADER_WGSL,
    );
    build(
        device,
        "eseqlisp patch cable pipeline",
        &shader,
        "patch_cable_vert",
        &shader,
        "patch_cable_frag",
        &[buffer_layout::<PatchCableInstance>(
            wgpu::VertexStepMode::Instance,
            &PATCH_CABLE_ATTRIBUTES,
        )],
        &[],
        format,
    )
}

/// One widget pipeline, assembled the way the Metal backend assembles its MSL:
/// shared preamble, then the widget's vertex stage (or the default one), then
/// its fragment stage.
pub fn widget_pipeline(
    device: &wgpu::Device,
    label: &str,
    vertex_source: Option<&str>,
    fragment_source: &str,
    format: wgpu::TextureFormat,
) -> wgpu::RenderPipeline {
    let source = wgsl_shaders::widget_shader_module(vertex_source, fragment_source);
    let shader = module(device, label, &source);
    build(
        device,
        label,
        &shader,
        "widget_vert",
        &shader,
        "widget_frag",
        &[buffer_layout::<WidgetInstance>(
            wgpu::VertexStepMode::Instance,
            &WIDGET_ATTRIBUTES,
        )],
        &[],
        format,
    )
}

pub fn wavetable_pipeline(
    device: &wgpu::Device,
    bank_layout: &wgpu::BindGroupLayout,
    format: wgpu::TextureFormat,
) -> wgpu::RenderPipeline {
    let shader = module(
        device,
        "eseqlisp wavetable shader",
        wgsl_shaders::WAVETABLE_SHADER_WGSL,
    );
    build(
        device,
        "eseqlisp wavetable pipeline",
        &shader,
        "wavetable_vert",
        &shader,
        "wavetable_frag",
        &[buffer_layout::<WavetableInstance>(
            wgpu::VertexStepMode::Instance,
            &WAVETABLE_ATTRIBUTES,
        )],
        &[bank_layout],
        format,
    )
}

pub fn waveform_pipeline(
    device: &wgpu::Device,
    data_layout: &wgpu::BindGroupLayout,
    format: wgpu::TextureFormat,
) -> wgpu::RenderPipeline {
    let shader = module(
        device,
        "eseqlisp waveform shader",
        wgsl_shaders::WAVEFORM_SHADER_WGSL,
    );
    build(
        device,
        "eseqlisp waveform pipeline",
        &shader,
        "waveform_vert",
        &shader,
        "waveform_frag",
        &[buffer_layout::<WaveformInstance>(
            wgpu::VertexStepMode::Instance,
            &WAVEFORM_ATTRIBUTES,
        )],
        &[data_layout],
        format,
    )
}

pub struct LiveSpectrogramPipelines {
    pub waterfall: wgpu::RenderPipeline,
    pub eq: wgpu::RenderPipeline,
}

pub fn live_spectrogram_pipelines(
    device: &wgpu::Device,
    data_layout: &wgpu::BindGroupLayout,
    format: wgpu::TextureFormat,
) -> LiveSpectrogramPipelines {
    let shader = module(
        device,
        "eseqlisp live spectrogram shader",
        wgsl_shaders::LIVE_SPECTROGRAM_SHADER_WGSL,
    );
    let build_mode = |label, fragment_entry| {
        build(
            device,
            label,
            &shader,
            "live_spectrogram_vert",
            &shader,
            fragment_entry,
            &[buffer_layout::<LiveSpectrogramInstance>(
                wgpu::VertexStepMode::Instance,
                &LIVE_SPECTROGRAM_ATTRIBUTES,
            )],
            &[data_layout],
            format,
        )
    };
    LiveSpectrogramPipelines {
        waterfall: build_mode(
            "eseqlisp live spectrogram waterfall pipeline",
            "live_spectrogram_waterfall_frag",
        ),
        eq: build_mode(
            "eseqlisp live spectrogram EQ pipeline",
            "live_spectrogram_eq_frag",
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every attribute must sit inside its struct and start on a 4-byte
    /// boundary, or wgpu rejects the layout at pipeline creation. Checking it
    /// here names the offending offset instead of failing inside a driver.
    fn assert_layout_fits(label: &str, stride: usize, attributes: &[VertexAttribute]) {
        assert_eq!(stride % 4, 0, "{label}: stride {stride} is not 4-byte sized");
        for attribute in attributes {
            let size = attribute.format.size() as usize;
            let offset = attribute.offset as usize;
            assert_eq!(offset % 4, 0, "{label}: offset {offset} is misaligned");
            assert!(
                offset + size <= stride,
                "{label}: attribute at {offset} (+{size}) runs past the {stride}-byte struct"
            );
        }
    }

    #[test]
    fn vertex_layouts_match_their_rust_structs() {
        assert_layout_fits(
            "Vertex",
            std::mem::size_of::<Vertex>(),
            &TEXT_VERTEX_ATTRIBUTES,
        );
        assert_layout_fits(
            "ImageVertex",
            std::mem::size_of::<ImageVertex>(),
            &IMAGE_VERTEX_ATTRIBUTES,
        );
        assert_layout_fits(
            "PatchCableInstance",
            std::mem::size_of::<PatchCableInstance>(),
            &PATCH_CABLE_ATTRIBUTES,
        );
        assert_layout_fits(
            "WidgetInstance",
            std::mem::size_of::<WidgetInstance>(),
            &WIDGET_ATTRIBUTES,
        );
        assert_layout_fits(
            "WavetableInstance",
            std::mem::size_of::<WavetableInstance>(),
            &WAVETABLE_ATTRIBUTES,
        );
        assert_layout_fits(
            "WaveformInstance",
            std::mem::size_of::<WaveformInstance>(),
            &WAVEFORM_ATTRIBUTES,
        );
        assert_layout_fits(
            "LiveSpectrogramInstance",
            std::mem::size_of::<LiveSpectrogramInstance>(),
            &LIVE_SPECTROGRAM_ATTRIBUTES,
        );
    }

    /// The last attribute of each layout must land exactly on the struct's last
    /// field, which is what proves the offsets were not transcribed from a
    /// stale field order.
    #[test]
    fn attribute_offsets_end_on_the_last_field() {
        for (label, stride, attributes) in [
            (
                "Vertex",
                std::mem::size_of::<Vertex>(),
                TEXT_VERTEX_ATTRIBUTES.as_slice(),
            ),
            (
                "ImageVertex",
                std::mem::size_of::<ImageVertex>(),
                IMAGE_VERTEX_ATTRIBUTES.as_slice(),
            ),
            (
                "PatchCableInstance",
                std::mem::size_of::<PatchCableInstance>(),
                PATCH_CABLE_ATTRIBUTES.as_slice(),
            ),
            (
                "WidgetInstance",
                std::mem::size_of::<WidgetInstance>(),
                WIDGET_ATTRIBUTES.as_slice(),
            ),
            (
                "WavetableInstance",
                std::mem::size_of::<WavetableInstance>(),
                WAVETABLE_ATTRIBUTES.as_slice(),
            ),
            (
                "WaveformInstance",
                std::mem::size_of::<WaveformInstance>(),
                WAVEFORM_ATTRIBUTES.as_slice(),
            ),
            (
                "LiveSpectrogramInstance",
                std::mem::size_of::<LiveSpectrogramInstance>(),
                LIVE_SPECTROGRAM_ATTRIBUTES.as_slice(),
            ),
        ] {
            let last = attributes.last().expect("layout has attributes");
            assert_eq!(
                last.offset as usize + last.format.size() as usize,
                stride,
                "{label}: layout stops short of the struct's end"
            );
        }
    }
}
