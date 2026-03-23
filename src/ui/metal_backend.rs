/// Metal GPU backend for eseqlisp.
#[cfg(target_os = "macos")]
mod inner {
    use std::collections::{HashMap, VecDeque};
    use std::ptr::NonNull;
    use std::time::{Duration, Instant};

    use crossterm::event::{
        Event, KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
    };
    use objc2::rc::Retained;
    use objc2::runtime::ProtocolObject;
    use objc2_app_kit::NSView;
    use objc2_core_foundation::CGSize;
    use objc2_foundation::NSString;
    use objc2_metal::{
        MTLBlendFactor, MTLBuffer, MTLClearColor, MTLCommandBuffer, MTLCommandEncoder,
        MTLCommandQueue, MTLCreateSystemDefaultDevice, MTLDevice, MTLLibrary, MTLLoadAction,
        MTLPixelFormat, MTLPrimitiveType, MTLRenderCommandEncoder, MTLRenderPassDescriptor,
        MTLRenderPipelineDescriptor, MTLRenderPipelineState, MTLResourceOptions, MTLScissorRect,
        MTLStoreAction, MTLTexture,
    };
    use objc2_quartz_core::{CAMetalDrawable, CAMetalLayer};
    use winit::{
        dpi::PhysicalSize,
        event::{
            ElementState, Event as WEvent, MouseButton as WMouseButton, MouseScrollDelta,
            TouchPhase, WindowEvent,
        },
        event_loop::{ControlFlow, EventLoop},
        keyboard::{Key, KeyCode as WinitKeyCode, NamedKey, PhysicalKey},
        platform::pump_events::EventLoopExtPumpEvents,
        raw_window_handle::{HasWindowHandle, RawWindowHandle},
        window::Window,
    };

    use crate::audio::sample::get_registered_sample;
    use crate::backend::{Backend, BackendError, Color, RenderFrame, TiledRenderFrame};
    use crate::glyph_atlas::{GlyphAtlas, ProportionalGlyphAtlas, SizedFontCache};
    use crate::layout::{Rect, TextMeasurer};
    use crate::theme;
    use crate::widget_render::{self, WidgetInstance, WidgetViewport};

    /// Lightweight TextMeasurer that delegates to `SizedFontCache` for font
    /// metrics without needing a GPU atlas. Used by the layout engine.
    pub(crate) struct PropTextMeasurer {
        fonts: std::cell::RefCell<SizedFontCache>,
    }

    impl PropTextMeasurer {
        pub(crate) fn new(base_font_size: f64, scale: f64) -> Option<Self> {
            let fonts = SizedFontCache::new(base_font_size, scale)?;
            Some(Self {
                fonts: std::cell::RefCell::new(fonts),
            })
        }
    }

    impl TextMeasurer for PropTextMeasurer {
        fn measure_text_px(&self, text: &str, font_size: f32) -> f32 {
            if text.is_empty() {
                return 0.0;
            }
            let size_tenths = (font_size * 10.0).round() as u16;
            self.fonts.borrow_mut().measure_text(text, size_tenths)
        }

        fn line_height_px(&self, font_size: f32) -> f32 {
            let size_tenths = (font_size * 10.0).round() as u16;
            self.fonts.borrow_mut().line_height(size_tenths)
        }
    }

    // ── Shader source ─────────────────────────────────────────────────────────
    //
    // Buffer-based vertex input: no vertex descriptor needed.
    // UV.v is flipped in the fragment shader because CoreText rasterizes Y-up
    // but Metal textures are Y-down.
    const SHADER_SRC: &str = r#"
#include <metal_stdlib>
using namespace metal;

struct Vertex {
    float2 position;
    float2 uv;
    float4 fg;
    float4 bg;
};

struct Varyings {
    float4 position [[position]];
    float2 uv;
    float4 fg;
    float4 bg;
};

vertex Varyings vert(
    uint                    vid   [[vertex_id]],
    device const Vertex*    verts [[buffer(0)]])
{
    Vertex v = verts[vid];
    Varyings out;
    out.position = float4(v.position, 0.0, 1.0);
    out.uv  = v.uv;
    out.fg  = v.fg;
    out.bg  = v.bg;
    return out;
}

fragment float4 frag(
    Varyings              in    [[stage_in]],
    texture2d<float>      atlas [[texture(0)]])
{
    constexpr sampler s(filter::nearest);
    float coverage = atlas.sample(s, in.uv).r;
    return mix(in.bg, in.fg, coverage);
}
"#;

    /// Fragment shader for proportional text — identical to the main text shader
    /// but uses bilinear filtering instead of nearest-neighbor. Proportional glyphs
    /// land on sub-pixel boundaries, so linear filtering produces smooth edges.
    /// Fragment shader for proportional text. Uses linear filtering and alpha
    /// blending so that glyph quads overlay each other without clipping.
    /// The background rect is drawn separately; glyphs are composited on top.
    const PROP_FRAG_SRC: &str = r#"
#include <metal_stdlib>
using namespace metal;

struct Varyings {
    float4 position [[position]];
    float2 uv;
    float4 fg;
    float4 bg;
};

fragment float4 prop_frag(
    Varyings              in    [[stage_in]],
    texture2d<float>      atlas [[texture(0)]])
{
    constexpr sampler s(filter::linear);
    float coverage = atlas.sample(s, in.uv).r;
    // Output foreground color with coverage as alpha.
    // The pipeline uses standard alpha blending (srcAlpha, 1-srcAlpha)
    // so glyphs composite over the background rect without clipping neighbors.
    return float4(in.fg.rgb, coverage);
}
"#;

    // ── Widget shader source ────────────────────────────────────────────────
    //
    // SDF-based rendering for sliders and toggles. Each widget is one instanced
    // quad (6 vertices from vertex_id). The fragment shader decides color per
    // pixel using UV coordinates and per-instance data.
    // ── Shared shader preamble (instance struct, varyings, SDF utils) ──────

    const WIDGET_SHADER_PREAMBLE: &str = r#"
#include <metal_stdlib>
using namespace metal;

// Packed types match Rust's #[repr(C)] layout (4-byte alignment).
struct WidgetInstance {
    packed_float2 ndc_min;
    packed_float2 ndc_max;
    float         value_t;
    float         orientation;
    float         itime;
    packed_float4 uniform_a;
    packed_float4 uniform_b;
    packed_float4 color_a;
    packed_float4 color_b;
    packed_float4 color_c;
    packed_float4 color_d;
    float         corner_radius;
    float         pixel_aspect;
};

struct WidgetVaryings {
    float4 position [[position]];
    float2 uv;
    float  value_t    [[flat]];
    float  itime      [[flat]];
    float4 uniform_a  [[flat]];
    float4 uniform_b  [[flat]];
    float4 color_a    [[flat]];
    float4 color_b    [[flat]];
    float4 color_c    [[flat]];
    float4 color_d    [[flat]];
    float  aspect     [[flat]];
};

float sdf_rounded_rect(float2 p, float2 half_size, float radius) {
    float2 d = abs(p) - half_size + radius;
    return length(max(d, 0.0)) + min(max(d.x, d.y), 0.0) - radius;
}

float compute_border_mask(float2 localPos, float2 outerSize, float cornerRadius,
                          float borderPixels, thread float& outerMask) {
    float outerDist = sdf_rounded_rect(localPos, outerSize, cornerRadius);
    float outerDeriv = max(fwidth(outerDist), 0.001);
    float borderThickness = borderPixels * outerDeriv;
    float2 innerSize = outerSize - float2(borderThickness);
    float innerDist = sdf_rounded_rect(localPos, innerSize, max(cornerRadius - borderThickness, 0.0));
    float innerDeriv = max(fwidth(innerDist), 0.001);
    outerMask = smoothstep(outerDeriv, -outerDeriv, outerDist);
    float innerMask = smoothstep(innerDeriv, -innerDeriv, innerDist);
    return outerMask * (1.0 - innerMask);
}
"#;

    const DEFAULT_WIDGET_VERTEX_SHADER: &str = r#"
vertex WidgetVaryings widget_vert(
    uint vid [[vertex_id]],
    uint iid [[instance_id]],
    device const WidgetInstance* instances [[buffer(0)]])
{
    float2 corners[6] = {
        float2(0, 0), float2(0, 1), float2(1, 0),
        float2(1, 0), float2(0, 1), float2(1, 1)
    };
    float2 corner = corners[vid];
    WidgetInstance inst = instances[iid];
    float2 ndc = mix(inst.ndc_min, inst.ndc_max, corner);

    WidgetVaryings out;
    out.position = float4(ndc, 0.0, 1.0);
    out.uv = corner;
    out.value_t = inst.value_t;
    out.itime = inst.itime;
    out.uniform_a = inst.uniform_a;
    out.uniform_b = inst.uniform_b;
    out.color_a = inst.color_a;
    out.color_b = inst.color_b;
    out.color_c = inst.color_c;
    out.color_d = inst.color_d;
    out.aspect = inst.pixel_aspect;
    return out;
}
"#;

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::lang::sdf_codegen::compile_sdf_to_metal;

        fn parse_one_expr(src: &str) -> crate::parser::Expression {
            let tokens = crate::parser::Parser::new(src.to_string()).parse().unwrap();
            let mut ast = crate::parser::ASTParser::new(tokens);
            ast.parse().unwrap().into_iter().next().unwrap()
        }

        fn compile_widget_shader_with_metal(shader_src: &str) -> Result<(), String> {
            let device = MTLCreateSystemDefaultDevice().ok_or("no Metal device".to_string())?;
            let full_src = format!(
                "{}{}{}",
                WIDGET_SHADER_PREAMBLE, DEFAULT_WIDGET_VERTEX_SHADER, shader_src
            );
            let src_ns = NSString::from_str(&full_src);
            device
                .newLibraryWithSource_options_error(&src_ns, None)
                .map(|_| ())
                .map_err(|err| format!("{:?}", err))
        }

        #[test]
        fn generated_top_level_let_layer_shader_compiles_in_metal() {
            let output = compile_sdf_to_metal(&parse_one_expr(
                "(let ((shape (- (length (vec2 x y)) 0.5)))
                   (sdf/layer
                     (sdf/fill shape
                       (material
                         :color (mix :accent :white (smoothstep -0.1 0.03 d))))))",
            ))
            .unwrap();
            compile_widget_shader_with_metal(&output.shader_source).unwrap();
        }

        #[test]
        fn generated_shadow_material_shader_compiles_in_metal() {
            let output = compile_sdf_to_metal(&parse_one_expr(
                "(let ((shape (- (length (vec2 x y)) 0.5)))
                   (sdf/layer
                     (sdf/fill shape
                       (material
                         :color :accent
                         :shadow (shadow :color (rgba 0 0 0 0.2)
                                         :blur 0.18
                                         :offset (vec2 0 0.05))))))",
            ))
            .unwrap();
            compile_widget_shader_with_metal(&output.shader_source).unwrap();
        }
    }

    const WAVEFORM_SHADER_SRC: &str = r#"
#include <metal_stdlib>
using namespace metal;

struct WaveformInstance {
    packed_float2 ndc_min;
    packed_float2 ndc_max;
    float sample_start;
    float sample_end;
    uint bucket_count;
    float aspect_ratio;
    float selection_start;
    float selection_end;
    float playhead_position;
    int show_playhead;
    packed_float4 waveform_color;
    packed_float4 selection_color;
};

struct WaveformVaryings {
    float4 position [[position]];
    float2 uv;
    float sample_start [[flat]];
    float sample_end [[flat]];
    uint bucket_count [[flat]];
    float aspect_ratio [[flat]];
    float selection_start [[flat]];
    float selection_end [[flat]];
    float playhead_position [[flat]];
    int show_playhead [[flat]];
    float4 waveform_color [[flat]];
    float4 selection_color [[flat]];
};

vertex WaveformVaryings waveform_vert(
    uint vid [[vertex_id]],
    device const WaveformInstance* instances [[buffer(0)]])
{
    float2 corners[6] = {
        float2(0, 0), float2(0, 1), float2(1, 0),
        float2(1, 0), float2(0, 1), float2(1, 1)
    };
    float2 corner = corners[vid];
    WaveformInstance inst = instances[0];
    float2 ndc = mix(inst.ndc_min, inst.ndc_max, corner);

    WaveformVaryings out;
    out.position = float4(ndc, 0.0, 1.0);
    out.uv = corner;
    out.sample_start = inst.sample_start;
    out.sample_end = inst.sample_end;
    out.bucket_count = inst.bucket_count;
    out.aspect_ratio = inst.aspect_ratio;
    out.selection_start = inst.selection_start;
    out.selection_end = inst.selection_end;
    out.playhead_position = inst.playhead_position;
    out.show_playhead = inst.show_playhead;
    out.waveform_color = inst.waveform_color;
    out.selection_color = inst.selection_color;
    return out;
}

fragment float4 waveform_frag(
    WaveformVaryings in [[stage_in]],
    device const float* waveform_data [[buffer(1)]])
{
    if (in.bucket_count < 2) {
        discard_fragment();
    }

    float2 uv = in.uv;
    float2 content_uv = uv;
    float3 rgb = float3(0.08, 0.08, 0.10);
    float alpha = 0.0;

    bool has_selection = in.selection_end > in.selection_start + 0.001;
    if (has_selection &&
        content_uv.x >= in.selection_start &&
        content_uv.x <= in.selection_end) {
        rgb = mix(rgb, in.selection_color.rgb, 0.30);
        alpha = max(alpha, 0.22);
    }

    float center_line = 1.0 - smoothstep(0.0, 0.004, abs(content_uv.y - 0.5));
    rgb = mix(rgb, float3(0.3, 0.3, 0.35), center_line * 0.20);
    alpha = max(alpha, center_line * 0.18);

    float boundary_width = max(fwidth(content_uv.x) * 0.9, 0.0015);
    float boundary_aa = max(fwidth(content_uv.x) * 0.75, 0.00075);
    float start_dist = abs(content_uv.x - in.selection_start);
    float end_dist = abs(content_uv.x - in.selection_end);
    float start_boundary = has_selection
        ? 1.0 - smoothstep(boundary_width, boundary_width + boundary_aa, start_dist)
        : 0.0;
    float end_boundary = has_selection
        ? 1.0 - smoothstep(boundary_width, boundary_width + boundary_aa, end_dist)
        : 0.0;
    float boundary_mask = max(start_boundary, end_boundary);
    rgb = mix(rgb, in.selection_color.rgb, boundary_mask * 0.85);
    alpha = max(alpha, boundary_mask * 0.75);

    float sample_t = clamp(mix(in.sample_start, in.sample_end, content_uv.x), 0.0, 1.0);
    float exact_idx = sample_t * float(in.bucket_count - 1);
    float pixel_span = max(fwidth(exact_idx), 1.0);
    float idx_left = clamp(exact_idx - pixel_span * 0.5, 0.0, float(in.bucket_count - 1));
    float idx_right = clamp(exact_idx + pixel_span * 0.5, 0.0, float(in.bucket_count - 1));

    int idx_a = clamp(int(floor(exact_idx)), 0, int(in.bucket_count - 1));
    int idx_b = min(idx_a + 1, int(in.bucket_count - 1));
    int idx_la = clamp(int(floor(idx_left)), 0, int(in.bucket_count - 1));
    int idx_lb = min(idx_la + 1, int(in.bucket_count - 1));
    int idx_ra = clamp(int(floor(idx_right)), 0, int(in.bucket_count - 1));
    int idx_rb = min(idx_ra + 1, int(in.bucket_count - 1));

    float frac = fract(exact_idx);
    float frac_left = fract(idx_left);
    float frac_right = fract(idx_right);

    float min_center = mix(waveform_data[idx_a * 2], waveform_data[idx_b * 2], frac);
    float max_center = mix(waveform_data[idx_a * 2 + 1], waveform_data[idx_b * 2 + 1], frac);
    float min_left = mix(waveform_data[idx_la * 2], waveform_data[idx_lb * 2], frac_left);
    float max_left = mix(waveform_data[idx_la * 2 + 1], waveform_data[idx_lb * 2 + 1], frac_left);
    float min_right = mix(waveform_data[idx_ra * 2], waveform_data[idx_rb * 2], frac_right);
    float max_right = mix(waveform_data[idx_ra * 2 + 1], waveform_data[idx_rb * 2 + 1], frac_right);

    float min_val = min(min_center, min(min_left, min_right));
    float max_val = max(max_center, max(max_left, max_right));
    float amplitude = max(abs(min_val), abs(max_val));
    amplitude = clamp(amplitude, 0.0, 1.0);
    float y_min = 0.5 - amplitude * 0.5;
    float y_max = 0.5 + amplitude * 0.5;

    float min_thickness = 0.010;
    if (y_max - y_min < min_thickness) {
        float center = (y_min + y_max) * 0.5;
        y_min = center - min_thickness * 0.5;
        y_max = center + min_thickness * 0.5;
    }

    float edge_aa = max(length(float2(fwidth(content_uv.x), fwidth(content_uv.y))) * 1.5, 0.002);
    float above_min = smoothstep(y_min - edge_aa, y_min + edge_aa, content_uv.y);
    float below_max = smoothstep(y_max + edge_aa, y_max - edge_aa, content_uv.y);
    float fill_alpha = above_min * below_max;

    float upper_edge = 1.0 - smoothstep(0.0, edge_aa * 1.5, abs(content_uv.y - y_max));
    float lower_edge = 1.0 - smoothstep(0.0, edge_aa * 1.5, abs(content_uv.y - y_min));
    float edge_alpha = max(upper_edge, lower_edge);

    float3 fill_color = mix(rgb, in.waveform_color.rgb, 0.88);
    float3 edge_color = mix(in.waveform_color.rgb, float3(1.0, 1.0, 1.0), 0.15);
    rgb = mix(rgb, fill_color, fill_alpha);
    rgb = mix(rgb, edge_color, edge_alpha * 0.9);
    alpha = max(alpha, fill_alpha);
    alpha = max(alpha, edge_alpha * 0.9);

    if (in.show_playhead == 1) {
        float playhead_dist = abs(content_uv.x - in.playhead_position);
        float playhead_width = 0.003;
        float playhead_aa = max(fwidth(content_uv.x) * 1.5, 0.001);
        float playhead_alpha = 1.0 - smoothstep(playhead_width - playhead_aa, playhead_width + playhead_aa, playhead_dist);
        bool playhead_overlaps_selection_boundary =
            has_selection && (playhead_dist <= boundary_width + boundary_aa) &&
            (abs(in.playhead_position - in.selection_start) <= boundary_width + boundary_aa ||
             abs(in.playhead_position - in.selection_end) <= boundary_width + boundary_aa);
        float3 playhead_color = playhead_overlaps_selection_boundary
            ? in.selection_color.rgb
            : float3(0.2, 0.9, 1.0);
        float playhead_mix = playhead_overlaps_selection_boundary
            ? max(playhead_alpha, boundary_mask)
            : playhead_alpha * 0.95;
        rgb = mix(rgb, playhead_color, playhead_mix);
        alpha = max(alpha, playhead_mix);
    }

    float border = min(min(content_uv.x, 1.0 - content_uv.x), min(content_uv.y, 1.0 - content_uv.y));
    float border_mask = 1.0 - smoothstep(0.0, 0.004, border);
    rgb = mix(rgb, float3(0.22, 0.22, 0.25), border_mask * 0.8);
    alpha = max(alpha, border_mask * 0.7);

    if (alpha < 0.001) {
        discard_fragment();
    }
    return float4(rgb, alpha);
}
"#;

    // ── Vertex type ───────────────────────────────────────────────────────────

    /// One vertex of a cell quad.  Two triangles (6 vertices) form each cell.
    #[repr(C)]
    #[derive(Clone)]
    pub struct Vertex {
        /// NDC position: X in [-1, +1], Y in [-1, +1] (Y+ = up).
        pub position: [f32; 2],
        /// Atlas UV: (0,0) = top-left of atlas texture.
        pub uv: [f32; 2],
        /// Foreground colour (RGBA linear 0..1).
        pub fg: [f32; 4],
        /// Background colour (RGBA linear 0..1).
        pub bg: [f32; 4],
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct WaveformInstance {
        ndc_min: [f32; 2],
        ndc_max: [f32; 2],
        sample_start: f32,
        sample_end: f32,
        bucket_count: u32,
        aspect_ratio: f32,
        selection_start: f32,
        selection_end: f32,
        playhead_position: f32,
        show_playhead: i32,
        waveform_color: [f32; 4],
        selection_color: [f32; 4],
    }

    struct WaveformGpuResource {
        bucket_count: u32,
        buffer: Retained<ProtocolObject<dyn MTLBuffer>>,
    }

    /// Layout + colour context threaded into `rasterize_char`.
    struct CharCtx {
        cell_w: f32,
        cell_h: f32,
        vp_w: f32,
        vp_h: f32,
        fg: [f32; 4],
        bg: [f32; 4],
    }

    /// Cell offset for placing tile content at the right screen position.
    /// Signed to support negative offsets from horizontal scrolling.
    #[derive(Clone, Copy, Default)]
    struct TileOffset {
        col: i32,
        row: i32,
    }

    // ── Backend ───────────────────────────────────────────────────────────────

    pub struct MetalBackend {
        // Metal state
        device: Retained<ProtocolObject<dyn MTLDevice>>,
        command_queue: Retained<ProtocolObject<dyn MTLCommandQueue>>,
        layer: Retained<CAMetalLayer>,
        // Text render pipeline (compiled from SHADER_SRC, nearest-neighbor filtering)
        pipeline: Option<Retained<ProtocolObject<dyn MTLRenderPipelineState>>>,
        // Proportional text pipeline (linear filtering for sub-pixel positioned glyphs)
        prop_pipeline: Option<Retained<ProtocolObject<dyn MTLRenderPipelineState>>>,
        // Per-widget-type GPU pipelines (hslider, vslider, toggle)
        widget_pipelines: HashMap<String, Retained<ProtocolObject<dyn MTLRenderPipelineState>>>,
        sdf_widget_pipeline_sources: HashMap<String, String>,
        waveform_pipeline: Option<Retained<ProtocolObject<dyn MTLRenderPipelineState>>>,
        waveform_buffers: HashMap<(String, u32), WaveformGpuResource>,
        // Glyph atlases
        atlas: Option<GlyphAtlas>,
        prop_atlas: Option<ProportionalGlyphAtlas>,
        cached_text_key: Option<u64>,
        cached_text_quads: Vec<Vertex>,
        cached_text_buffer: Option<Retained<ProtocolObject<dyn MTLBuffer>>>,
        cached_text_vertex_count: usize,
        stats: RenderStats,
        // Winit
        event_loop: Option<EventLoop<()>>,
        window: Option<Window>,
        pending: VecDeque<Event>,
        pending_drag: Option<Event>,
        pending_move: Option<Event>,
        pending_magnify: VecDeque<(f64, (f32, f32))>,
        pending_scroll: VecDeque<((f32, f32), (f32, f32))>,
        modifiers: KeyModifiers,
        pressed_mouse_button: Option<MouseButton>,
        cursor_cell: (u16, u16),
        cursor_pos: (f32, f32),
        last_precise_mouse: Option<(f32, f32)>,
        last_window_bg: Option<Color>,
        start_time: Instant,
    }

    impl MetalBackend {
        pub fn new() -> Result<Self, BackendError> {
            let device = MTLCreateSystemDefaultDevice().ok_or(BackendError::MetalError)?;
            let command_queue = device.newCommandQueue().ok_or(BackendError::MetalError)?;
            let layer = CAMetalLayer::new();
            layer.setDevice(Some(&device));
            layer.setPixelFormat(MTLPixelFormat::BGRA8Unorm);
            layer.setFramebufferOnly(false); // atlas upload needs non-framebuffer-only
            Ok(Self {
                device,
                command_queue,
                layer,
                pipeline: None,
                prop_pipeline: None,
                widget_pipelines: HashMap::new(),
                sdf_widget_pipeline_sources: HashMap::new(),
                waveform_pipeline: None,
                waveform_buffers: HashMap::new(),
                atlas: None,
                prop_atlas: None,
                cached_text_key: None,
                cached_text_quads: Vec::new(),
                cached_text_buffer: None,
                cached_text_vertex_count: 0,
                stats: RenderStats::new(),
                event_loop: None,
                window: None,
                pending: VecDeque::new(),
                pending_drag: None,
                pending_move: None,
                pending_magnify: VecDeque::new(),
                pending_scroll: VecDeque::new(),
                modifiers: KeyModifiers::NONE,
                pressed_mouse_button: None,
                cursor_cell: (0, 0),
                cursor_pos: (0.0, 0.0),
                last_precise_mouse: None,
                last_window_bg: None,
                start_time: Instant::now(),
            })
        }

        fn elapsed_time_seconds(&self) -> f32 {
            self.start_time.elapsed().as_secs_f32()
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

        /// Create a TextMeasurer for the proportional font. Called once after
        /// atlas initialization to hand off to the layout engine.
        pub fn create_text_measurer(&self) -> Option<Box<dyn TextMeasurer>> {
            let scale = self
                .window
                .as_ref()
                .map(|w| w.scale_factor())
                .unwrap_or(1.0);
            let measurer = PropTextMeasurer::new(14.0 * scale, scale)?;
            Some(Box::new(measurer))
        }

        pub fn cell_dimensions(&self) -> (f32, f32) {
            self.atlas
                .as_ref()
                .map(|a| (a.cell_w.max(1) as f32, a.cell_h.max(1) as f32))
                .unwrap_or((8.0, 16.0))
        }

        fn sync_window_theme(&mut self) {
            let Some(window) = self.window.as_ref() else {
                return;
            };

            let bg = theme::BG();
            if self.last_window_bg == Some(bg) {
                return;
            }
            self.last_window_bg = Some(bg);

            if let Ok(handle) = window.window_handle()
                && let RawWindowHandle::AppKit(appkit) = handle.as_raw()
            {
                unsafe {
                    use objc2_app_kit::{NSAppearance, NSAppearanceCustomization, NSColor};

                    let ns_view = appkit.ns_view.as_ptr() as *mut NSView;
                    let ns_view = &*ns_view;
                    let ns_window = ns_view.window().expect("view must have a window");
                    let color = NSColor::colorWithRed_green_blue_alpha(
                        bg.r as f64,
                        bg.g as f64,
                        bg.b as f64,
                        1.0,
                    );
                    ns_window.setBackgroundColor(Some(&color));
                    ns_window.setTitlebarAppearsTransparent(true);

                    let appearance_name = if bg.luma() > 0.55 {
                        "NSAppearanceNameVibrantLight"
                    } else {
                        "NSAppearanceNameVibrantDark"
                    };
                    if let Some(appearance) =
                        NSAppearance::appearanceNamed(&NSString::from_str(appearance_name))
                    {
                        ns_window.setAppearance(Some(&appearance));
                    }
                }
            }
        }

        fn ensure_waveform_buffer(
            &mut self,
            sample_key: &str,
            samples_per_bucket: u32,
        ) -> Option<&WaveformGpuResource> {
            let key = (sample_key.to_string(), samples_per_bucket);
            if !self.waveform_buffers.contains_key(&key) {
                let sample = get_registered_sample(sample_key)?;
                let level = sample
                    .levels()
                    .iter()
                    .find(|level| level.samples_per_bucket as u32 == samples_per_bucket)?;
                let flattened = level.flattened_pairs();
                let buffer = unsafe {
                    self.device.newBufferWithBytes_length_options(
                        NonNull::new(flattened.as_ptr() as *mut _)?,
                        std::mem::size_of_val(flattened.as_slice()),
                        MTLResourceOptions(0),
                    )
                }?;
                self.waveform_buffers.insert(
                    key.clone(),
                    WaveformGpuResource {
                        bucket_count: level.buckets.len() as u32,
                        buffer,
                    },
                );
            }
            self.waveform_buffers.get(&key)
        }

        /// Compile Metal pipelines for any SDF widgets that have been registered
        /// since the last render. This enables lazy compilation of defwidget shaders.
        fn compile_pending_sdf_pipelines(&mut self) {
            use crate::widget_render::sdf_widget;
            for (name, shader_src) in sdf_widget::sdf_widget_shader_sources() {
                if self
                    .sdf_widget_pipeline_sources
                    .get(&name)
                    .is_some_and(|current| current == &shader_src)
                {
                    continue;
                }
                let full_src = format!(
                    "{}{}{}",
                    WIDGET_SHADER_PREAMBLE, DEFAULT_WIDGET_VERTEX_SHADER, shader_src
                );
                let src_ns = NSString::from_str(&full_src);
                let wlib = match self
                    .device
                    .newLibraryWithSource_options_error(&src_ns, None)
                {
                    Ok(lib) => lib,
                    Err(err) => {
                        eprintln!(
                            "SDF widget '{}': shader compilation failed: {:?}",
                            name, err
                        );
                        self.widget_pipelines.remove(&name);
                        self.sdf_widget_pipeline_sources.insert(name, shader_src);
                        continue;
                    }
                };
                let (Some(wvert), Some(wfrag)) = (
                    wlib.newFunctionWithName(&NSString::from_str("widget_vert")),
                    wlib.newFunctionWithName(&NSString::from_str("widget_frag")),
                ) else {
                    continue;
                };
                let wdesc = MTLRenderPipelineDescriptor::new();
                wdesc.setVertexFunction(Some(&wvert));
                wdesc.setFragmentFunction(Some(&wfrag));
                let wattach = unsafe { wdesc.colorAttachments().objectAtIndexedSubscript(0) };
                wattach.setPixelFormat(MTLPixelFormat::BGRA8Unorm);
                wattach.setBlendingEnabled(true);
                {
                    use objc2_metal::{MTLBlendFactor, MTLBlendOperation};
                    wattach.setSourceRGBBlendFactor(MTLBlendFactor::SourceAlpha);
                    wattach.setDestinationRGBBlendFactor(MTLBlendFactor::OneMinusSourceAlpha);
                    wattach.setRgbBlendOperation(MTLBlendOperation::Add);
                    wattach.setSourceAlphaBlendFactor(MTLBlendFactor::One);
                    wattach.setDestinationAlphaBlendFactor(MTLBlendFactor::OneMinusSourceAlpha);
                    wattach.setAlphaBlendOperation(MTLBlendOperation::Add);
                }
                if let Ok(pipeline_state) = self
                    .device
                    .newRenderPipelineStateWithDescriptor_error(&wdesc)
                {
                    self.sdf_widget_pipeline_sources
                        .insert(name.clone(), shader_src);
                    self.widget_pipelines.insert(name, pipeline_state);
                }
            }
        }

        fn draw_waveform_primitives(
            &mut self,
            enc: &ProtocolObject<dyn MTLRenderCommandEncoder>,
            pipeline: &ProtocolObject<dyn MTLRenderPipelineState>,
            primitives: &[widget_render::MetalWaveformPrimitive],
            cell_w: f32,
            cell_h: f32,
            vp_w: f32,
            vp_h: f32,
        ) {
            for primitive in primitives {
                let Some((waveform_buffer, bucket_count)) = self
                    .ensure_waveform_buffer(&primitive.sample_key, primitive.samples_per_bucket)
                    .map(|resource| (resource.buffer.clone(), resource.bucket_count))
                else {
                    continue;
                };
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
                    playhead_position: primitive.playhead_position,
                    show_playhead: if primitive.show_playhead { 1 } else { 0 },
                    waveform_color: primitive.waveform_color.to_rgba(),
                    selection_color: primitive.selection_color.to_rgba(),
                };
                let Some(instance_buffer) = (unsafe {
                    self.device.newBufferWithBytes_length_options(
                        NonNull::new((&instance as *const WaveformInstance).cast_mut().cast())
                            .unwrap(),
                        std::mem::size_of::<WaveformInstance>(),
                        MTLResourceOptions(0),
                    )
                }) else {
                    continue;
                };
                enc.setRenderPipelineState(pipeline);
                unsafe {
                    enc.setVertexBuffer_offset_atIndex(Some(&instance_buffer), 0, 0);
                    enc.setFragmentBuffer_offset_atIndex(Some(&waveform_buffer), 0, 1);
                    enc.drawPrimitives_vertexStart_vertexCount(MTLPrimitiveType::Triangle, 0, 6);
                }
            }
        }

        /// Render a tiled frame with per-tile scissor clipping.
        pub fn render_tiled(&mut self, tiled: &TiledRenderFrame) -> Result<(), BackendError> {
            crate::widget_render::sdf_widget::set_sdf_time_seconds(self.elapsed_time_seconds());
            self.sync_window_theme();

            let Some(pipeline) = self.pipeline.clone() else {
                return Ok(());
            };
            let Some((cell_w, cell_h, atlas_texture)) = self.atlas.as_ref().map(|atlas| {
                (
                    atlas.cell_w as f32,
                    atlas.cell_h as f32,
                    atlas.texture.clone(),
                )
            }) else {
                return Ok(());
            };
            let Some(drawable) = self.layer.nextDrawable() else {
                return Ok(());
            };
            let texture = drawable.texture();
            let vp_w = texture.width() as f32;
            let vp_h = texture.height() as f32;
            let ndc_x = |px: f32| px / vp_w * 2.0 - 1.0;
            let ndc_y = |px: f32| 1.0 - px / vp_h * 2.0;
            let to_rgba = |c: Color| [c.r, c.g, c.b, c.a];
            let has_multiple_tiles = tiled.tiles.len() > 1;
            let border_px = 2.0f32;

            // ── Render pass setup ────────────────────────────────────────────
            let desc = MTLRenderPassDescriptor::new();
            let attach = unsafe { desc.colorAttachments().objectAtIndexedSubscript(0) };
            attach.setTexture(Some(&texture));
            attach.setLoadAction(MTLLoadAction::Clear);
            attach.setClearColor(MTLClearColor {
                red: theme::BG().r as f64,
                green: theme::BG().g as f64,
                blue: theme::BG().b as f64,
                alpha: 1.0,
            });
            attach.setStoreAction(MTLStoreAction::Store);

            let cmdbuf = self
                .command_queue
                .commandBuffer()
                .ok_or(BackendError::MetalError)?;
            let enc = cmdbuf
                .renderCommandEncoderWithDescriptor(&desc)
                .ok_or(BackendError::MetalError)?;

            // Helper: upload verts to a buffer and draw with text pipeline
            let draw_text_verts = |enc: &ProtocolObject<dyn MTLRenderCommandEncoder>,
                                   device: &ProtocolObject<dyn MTLDevice>,
                                   pipeline: &ProtocolObject<dyn MTLRenderPipelineState>,
                                   atlas_tex: &ProtocolObject<dyn MTLTexture>,
                                   verts: &[Vertex]| {
                if verts.is_empty() {
                    return;
                }
                let byte_len = std::mem::size_of_val(verts);
                let Some(vbuf) = (unsafe {
                    device.newBufferWithBytes_length_options(
                        NonNull::new(verts.as_ptr() as *mut _).unwrap(),
                        byte_len,
                        MTLResourceOptions(0),
                    )
                }) else {
                    return;
                };
                enc.setRenderPipelineState(pipeline);
                unsafe {
                    enc.setVertexBuffer_offset_atIndex(Some(&vbuf), 0, 0);
                    enc.setFragmentTexture_atIndex(Some(atlas_tex), 0);
                    enc.drawPrimitives_vertexStart_vertexCount(
                        MTLPrimitiveType::Triangle,
                        0,
                        verts.len() as _,
                    );
                }
            };

            // ── Per-tile rendering with scissor rect ─────────────────────────
            for tile in &tiled.tiles {
                let col_off = tile.rect.col.round() as usize;
                let row_off = tile.rect.row.round() as usize;
                let tile_w = tile.rect.width.round() as usize;
                let tile_h = tile.rect.height.round() as usize;

                // Set scissor rect to clip to tile content area (exclude status row)
                let scissor_rows = if tile.show_status {
                    tile_h.saturating_sub(1)
                } else {
                    tile_h
                };
                let scissor = MTLScissorRect {
                    x: (col_off as f32 * cell_w) as usize,
                    y: (row_off as f32 * cell_h) as usize,
                    width: (tile_w as f32 * cell_w) as usize,
                    height: (scissor_rows as f32 * cell_h) as usize,
                };
                enc.setScissorRect(scissor);

                // ── Text content (shifted by horizontal scroll) ──────────────
                let hscroll = tile.frame.widget_scroll_left as i32;
                let offset = TileOffset {
                    col: col_off as i32 - hscroll,
                    row: row_off as i32,
                };
                let text_verts = {
                    let atlas = self.atlas.as_mut().ok_or(BackendError::MetalError)?;
                    build_text_quads_offset(&tile.frame, atlas, vp_w, vp_h, offset)
                };
                draw_text_verts(&enc, &self.device, &pipeline, &atlas_texture, &text_verts);

                // ── Widget primitives (clipped to content area, above status) ─
                // Collect with LOCAL coords (no offset) so scroll/clip logic works,
                // then offset the resulting primitives to screen position.
                if let Some(ref layout) = tile.frame.widget_layout {
                    let time_seconds = self.elapsed_time_seconds();
                    let inner_rows = (tile.rect.height - if tile.show_status { 1.0 } else { 0.0 })
                        .max(0.0)
                        .round() as u16;
                    let primitives = widget_render::collect_metal_primitives(
                        layout,
                        WidgetViewport {
                            cell_w,
                            cell_h,
                            vp_w,
                            vp_h,
                            time_seconds,
                            focused_widget_id: tile.frame.focused_widget_id,
                            focused_branch: false,
                        },
                        tile.frame.widget_scroll_top,
                        inner_rows,
                    );
                    // Offset primitives to tile's screen position,
                    // shifted by both text scroll (vertical) and hscroll (horizontal)
                    // so widgets move with the text.
                    let text_scroll = tile.frame.text_scroll_top as i32;
                    let widget_scroll = tile.frame.widget_scroll_top as i32;
                    let widget_col_off = col_off as i32 - hscroll;
                    let widget_row_off = row_off as i32 - text_scroll - widget_scroll;
                    let offset_prims: Vec<_> = primitives
                        .into_iter()
                        .map(|p| {
                            offset_primitive(
                                p,
                                widget_col_off,
                                widget_row_off,
                                cell_w,
                                cell_h,
                                vp_w,
                                vp_h,
                            )
                        })
                        .collect();
                    // Rect/Quad/GlyphRun primitives
                    let prim_quads = {
                        let atlas = self.atlas.as_mut().ok_or(BackendError::MetalError)?;
                        build_widget_primitive_quads(&offset_prims, atlas, vp_w, vp_h)
                    };
                    draw_text_verts(&enc, &self.device, &pipeline, &atlas_texture, &prim_quads);

                    // Proportional text: separate atlas + linear-filtering pipeline.
                    if let (Some(prop_atlas), Some(prop_pipe)) =
                        (self.prop_atlas.as_mut(), self.prop_pipeline.as_ref())
                    {
                        let prop_verts = build_proportional_text_quads(
                            &offset_prims,
                            prop_atlas,
                            cell_w,
                            cell_h,
                            vp_w,
                            vp_h,
                        );
                        let prop_tex = prop_atlas.texture.clone();
                        draw_text_verts(&enc, &self.device, prop_pipe, &prop_tex, &prop_verts);
                    }

                    if let Some(waveform_pipeline) = self.waveform_pipeline.clone() {
                        let waveforms = collect_waveform_primitives(&offset_prims);
                        self.draw_waveform_primitives(
                            &enc,
                            &waveform_pipeline,
                            &waveforms,
                            cell_w,
                            cell_h,
                            vp_w,
                            vp_h,
                        );
                    }
                    // Widget instances (sliders, toggles, knobs, SDF widgets)
                    self.compile_pending_sdf_pipelines();
                    let widget_runs = collect_widget_instance_runs(&offset_prims);
                    for (widget_type, instances) in &widget_runs {
                        let Some(wpipe) = self.widget_pipelines.get(widget_type) else {
                            continue;
                        };
                        if instances.is_empty() {
                            continue;
                        }
                        let byte_len = std::mem::size_of_val(instances.as_slice());
                        let Some(wbuf) = (unsafe {
                            self.device.newBufferWithBytes_length_options(
                                NonNull::new(instances.as_ptr() as *mut _).unwrap(),
                                byte_len,
                                MTLResourceOptions(0),
                            )
                        }) else {
                            continue;
                        };
                        enc.setRenderPipelineState(wpipe);
                        unsafe {
                            enc.setVertexBuffer_offset_atIndex(Some(&wbuf), 0, 0);
                            enc.drawPrimitives_vertexStart_vertexCount_instanceCount(
                                MTLPrimitiveType::Triangle,
                                0,
                                6,
                                instances.len() as _,
                            );
                        }
                    }
                }

                // ── Per-tile status bar (drawn ON TOP of widgets with full-tile scissor)
                if tile.show_status {
                    enc.setScissorRect(MTLScissorRect {
                        x: (col_off as f32 * cell_w) as usize,
                        y: (row_off as f32 * cell_h) as usize,
                        width: (tile_w as f32 * cell_w) as usize,
                        height: (tile_h as f32 * cell_h) as usize,
                    });
                    let mut status_verts = Vec::new();
                    let status_row = row_off + tile_h.saturating_sub(1);
                    let status_bg = to_rgba(theme::STATUS_BG());
                    let sx0 = ndc_x(col_off as f32 * cell_w);
                    let sx1 = ndc_x((col_off + tile_w) as f32 * cell_w);
                    let sy0 = ndc_y(status_row as f32 * cell_h);
                    let sy1 = ndc_y((status_row + 1) as f32 * cell_h);
                    let sb = |px, py| Vertex {
                        position: [px, py],
                        uv: [0.0, 0.0],
                        fg: status_bg,
                        bg: status_bg,
                    };
                    status_verts.extend_from_slice(&[
                        sb(sx0, sy0),
                        sb(sx0, sy1),
                        sb(sx1, sy0),
                        sb(sx1, sy0),
                        sb(sx0, sy1),
                        sb(sx1, sy1),
                    ]);
                    push_horizontal_rule(
                        &mut status_verts,
                        col_off as f32 * cell_w,
                        status_row as f32 * cell_h,
                        tile_w as f32 * cell_w,
                        1.0,
                        theme::STATUS_EDGE(),
                        vp_w,
                        vp_h,
                    );
                    for (i, cell) in tile.frame.status_cells.iter().enumerate() {
                        let ch_col = col_off + i;
                        if ch_col >= col_off + tile_w {
                            continue;
                        }
                        let fg = to_rgba(cell.style.fg);
                        let bg = to_rgba(cell.style.bg.unwrap_or(theme::STATUS_BG()));
                        let ch = cell.ch;
                        if ch == ' ' {
                            continue;
                        }
                        {
                            let atlas = self.atlas.as_mut().ok_or(BackendError::MetalError)?;
                            rasterize_char(
                                atlas,
                                ch,
                                (ch_col as i32, status_row as f32),
                                &CharCtx {
                                    cell_w,
                                    cell_h,
                                    vp_w,
                                    vp_h,
                                    fg,
                                    bg,
                                },
                                &mut status_verts,
                            );
                        }
                    }
                    draw_text_verts(&enc, &self.device, &pipeline, &atlas_texture, &status_verts);
                }

                // ── Thin pixel borders (drawn AFTER content, on top) ─────────
                if has_multiple_tiles {
                    let border_color = if tile.is_active {
                        theme::PURPLE()
                    } else {
                        Color::DARK_GRAY
                    };
                    let bc = to_rgba(border_color);
                    let bv = |px, py| Vertex {
                        position: [px, py],
                        uv: [0.0, 0.0],
                        fg: bc,
                        bg: bc,
                    };
                    let left_px = col_off as f32 * cell_w;
                    let right_px = (col_off + tile_w) as f32 * cell_w;
                    let top_px = row_off as f32 * cell_h;
                    let bottom_px = (row_off + tile_h) as f32 * cell_h;
                    let mut bverts = Vec::new();
                    // Right edge
                    let (rx0, rx1) = (ndc_x(right_px - border_px), ndc_x(right_px));
                    let (ry0, ry1) = (ndc_y(top_px), ndc_y(bottom_px));
                    bverts.extend_from_slice(&[
                        bv(rx0, ry0),
                        bv(rx0, ry1),
                        bv(rx1, ry0),
                        bv(rx1, ry0),
                        bv(rx0, ry1),
                        bv(rx1, ry1),
                    ]);
                    // Left edge
                    let (lx0, lx1) = (ndc_x(left_px), ndc_x(left_px + border_px));
                    bverts.extend_from_slice(&[
                        bv(lx0, ry0),
                        bv(lx0, ry1),
                        bv(lx1, ry0),
                        bv(lx1, ry0),
                        bv(lx0, ry1),
                        bv(lx1, ry1),
                    ]);
                    // Top edge
                    let (tx0, tx1) = (ndc_x(left_px), ndc_x(right_px));
                    let (ty0, ty1) = (ndc_y(top_px), ndc_y(top_px + border_px));
                    bverts.extend_from_slice(&[
                        bv(tx0, ty0),
                        bv(tx0, ty1),
                        bv(tx1, ty0),
                        bv(tx1, ty0),
                        bv(tx0, ty1),
                        bv(tx1, ty1),
                    ]);
                    // Bottom edge
                    let (by0, by1) = (ndc_y(bottom_px - border_px), ndc_y(bottom_px));
                    bverts.extend_from_slice(&[
                        bv(tx0, by0),
                        bv(tx0, by1),
                        bv(tx1, by0),
                        bv(tx1, by0),
                        bv(tx0, by1),
                        bv(tx1, by1),
                    ]);
                    draw_text_verts(&enc, &self.device, &pipeline, &atlas_texture, &bverts);
                }
            }

            // ── Completion popup (no scissor — drawn on top of everything) ───
            if let Some(comp) = &tiled.completion {
                // Reset scissor to full viewport
                {
                    enc.setScissorRect(MTLScissorRect {
                        x: 0,
                        y: 0,
                        width: vp_w as usize,
                        height: vp_h as usize,
                    });
                }
                if let Some(tile) = tiled.tiles.iter().find(|t| t.is_active) {
                    let col_off = tile.rect.col.round() as usize;
                    let row_off = tile.rect.row.round() as usize;
                    let sel_bg = to_rgba(theme::COMP_SELECTED_BG());
                    let unsel_bg = to_rgba(theme::COMP_UNSELECTED_BG());
                    let pop_fg = to_rgba(theme::COMP_FG());
                    let popup_col = col_off + comp.anchor.1;
                    let popup_row = row_off + comp.anchor.0 + 1;
                    let label_w = comp
                        .entries
                        .iter()
                        .map(|e| e.label.len())
                        .max()
                        .unwrap_or(0)
                        .max(12);
                    let mut popup_verts = Vec::new();
                    let x0 = ndc_x(popup_col as f32 * cell_w);
                    let x1 = ndc_x((popup_col + label_w) as f32 * cell_w);
                    for (i, entry) in comp.entries.iter().enumerate() {
                        let row = popup_row + i;
                        let y0 = ndc_y(row as f32 * cell_h);
                        let y1 = ndc_y((row + 1) as f32 * cell_h);
                        let bg = if entry.selected { sel_bg } else { unsel_bg };
                        let gv = |px, py| Vertex {
                            position: [px, py],
                            uv: [0.0, 0.0],
                            fg: pop_fg,
                            bg,
                        };
                        popup_verts.extend_from_slice(&[
                            gv(x0, y0),
                            gv(x0, y1),
                            gv(x1, y0),
                            gv(x1, y0),
                            gv(x0, y1),
                            gv(x1, y1),
                        ]);
                        for (j, ch) in entry.label.chars().enumerate() {
                            if ch == ' ' {
                                continue;
                            }
                            {
                                let atlas = self.atlas.as_mut().ok_or(BackendError::MetalError)?;
                                rasterize_char(
                                    atlas,
                                    ch,
                                    ((popup_col + j) as i32, row as f32),
                                    &CharCtx {
                                        cell_w,
                                        cell_h,
                                        vp_w,
                                        vp_h,
                                        fg: pop_fg,
                                        bg,
                                    },
                                    &mut popup_verts,
                                );
                            }
                        }
                    }
                    draw_text_verts(&enc, &self.device, &pipeline, &atlas_texture, &popup_verts);
                }
            }

            enc.endEncoding();
            cmdbuf.presentDrawable(objc2::runtime::ProtocolObject::from_ref(&*drawable));
            cmdbuf.commit();
            Ok(())
        }
    }

    impl Backend for MetalBackend {
        fn initialize(&mut self) -> Result<(), BackendError> {
            // ── Window ───────────────────────────────────────────────────────
            let event_loop = EventLoop::new().map_err(|_| BackendError::MetalError)?;
            let window = winit::window::WindowBuilder::new()
                .with_title("eseqlisp")
                .with_inner_size(PhysicalSize::new(1200u32, 800u32))
                .build(&event_loop)
                .map_err(|_| BackendError::MetalError)?;

            let phys = window.inner_size();
            if let Ok(handle) = window.window_handle()
                && let RawWindowHandle::AppKit(appkit) = handle.as_raw()
            {
                unsafe {
                    let ns_view = appkit.ns_view.as_ptr() as *mut NSView;
                    let ns_view = &*ns_view;
                    ns_view.setWantsLayer(true);
                    ns_view.setLayer(Some(&self.layer));
                }
            }
            // Set drawableSize to physical pixels so the Metal texture is full-res
            // on HiDPI/Retina displays (layer bounds default to logical pixels).
            self.layer.setDrawableSize(CGSize {
                width: phys.width as f64,
                height: phys.height as f64,
            });
            self.event_loop = Some(event_loop);
            self.window = Some(window);
            self.sync_window_theme();

            // ── Glyph atlas ──────────────────────────────────────────────────
            let scale = self
                .window
                .as_ref()
                .map(|w| w.scale_factor())
                .unwrap_or(1.0);
            self.atlas = GlyphAtlas::new(&self.device, "JetBrainsMono-Regular", 14.0 * scale);
            self.prop_atlas = ProportionalGlyphAtlas::new(&self.device, 14.0 * scale, scale);

            // ── Render pipeline ──────────────────────────────────────────────
            let src = NSString::from_str(SHADER_SRC);
            let library = self
                .device
                .newLibraryWithSource_options_error(&src, None)
                .map_err(|_| BackendError::MetalError)?;

            let vert_fn = library
                .newFunctionWithName(&NSString::from_str("vert"))
                .ok_or(BackendError::MetalError)?;
            let frag_fn = library
                .newFunctionWithName(&NSString::from_str("frag"))
                .ok_or(BackendError::MetalError)?;

            let desc = MTLRenderPipelineDescriptor::new();
            desc.setVertexFunction(Some(&vert_fn));
            desc.setFragmentFunction(Some(&frag_fn));
            let attach = unsafe { desc.colorAttachments().objectAtIndexedSubscript(0) };
            attach.setPixelFormat(MTLPixelFormat::BGRA8Unorm);

            self.pipeline = Some(
                self.device
                    .newRenderPipelineStateWithDescriptor_error(&desc)
                    .map_err(|_| BackendError::MetalError)?,
            );

            // ── Proportional text pipeline (linear filtering) ────────────────
            {
                // Compile the proportional fragment shader alongside the shared
                // vertex shader (reuse from the main library).
                // Compile PROP_FRAG_SRC as its own library,
                // reuse the vertex function from the main library.
                let prop_lib = self
                    .device
                    .newLibraryWithSource_options_error(&NSString::from_str(PROP_FRAG_SRC), None)
                    .map_err(|_| BackendError::MetalError)?;
                let prop_frag_fn = prop_lib
                    .newFunctionWithName(&NSString::from_str("prop_frag"))
                    .ok_or(BackendError::MetalError)?;

                let prop_desc = MTLRenderPipelineDescriptor::new();
                prop_desc.setVertexFunction(Some(&vert_fn));
                prop_desc.setFragmentFunction(Some(&prop_frag_fn));
                let prop_attach =
                    unsafe { prop_desc.colorAttachments().objectAtIndexedSubscript(0) };
                prop_attach.setPixelFormat(MTLPixelFormat::BGRA8Unorm);
                // Enable alpha blending: src * srcAlpha + dst * (1 - srcAlpha).
                // This lets glyph quads overlap without clipping each other.
                prop_attach.setBlendingEnabled(true);
                prop_attach.setSourceRGBBlendFactor(MTLBlendFactor::SourceAlpha);
                prop_attach.setDestinationRGBBlendFactor(MTLBlendFactor::OneMinusSourceAlpha);
                prop_attach.setSourceAlphaBlendFactor(MTLBlendFactor::One);
                prop_attach.setDestinationAlphaBlendFactor(MTLBlendFactor::OneMinusSourceAlpha);

                self.prop_pipeline = Some(
                    self.device
                        .newRenderPipelineStateWithDescriptor_error(&prop_desc)
                        .map_err(|_| BackendError::MetalError)?,
                );
            }

            // ── Widget render pipelines (one per widget type) ────────────────
            // Each widget gets its own fragment shader but shares the vertex
            // shader and SDF utilities from the preamble.
            for (widget_type, vertex_src, fragment_src) in widget_render::widget_shader_sources() {
                let full_src = format!(
                    "{}{}{}",
                    WIDGET_SHADER_PREAMBLE,
                    vertex_src.unwrap_or(DEFAULT_WIDGET_VERTEX_SHADER),
                    fragment_src
                );
                let src_ns = NSString::from_str(&full_src);
                let wlib = self
                    .device
                    .newLibraryWithSource_options_error(&src_ns, None)
                    .map_err(|_| BackendError::MetalError)?;

                let wvert = wlib
                    .newFunctionWithName(&NSString::from_str("widget_vert"))
                    .ok_or(BackendError::MetalError)?;
                let wfrag = wlib
                    .newFunctionWithName(&NSString::from_str("widget_frag"))
                    .ok_or(BackendError::MetalError)?;

                let wdesc = MTLRenderPipelineDescriptor::new();
                wdesc.setVertexFunction(Some(&wvert));
                wdesc.setFragmentFunction(Some(&wfrag));
                let wattach = unsafe { wdesc.colorAttachments().objectAtIndexedSubscript(0) };
                wattach.setPixelFormat(MTLPixelFormat::BGRA8Unorm);
                wattach.setBlendingEnabled(true);
                {
                    use objc2_metal::{MTLBlendFactor, MTLBlendOperation};
                    wattach.setSourceRGBBlendFactor(MTLBlendFactor::SourceAlpha);
                    wattach.setDestinationRGBBlendFactor(MTLBlendFactor::OneMinusSourceAlpha);
                    wattach.setRgbBlendOperation(MTLBlendOperation::Add);
                    wattach.setSourceAlphaBlendFactor(MTLBlendFactor::One);
                    wattach.setDestinationAlphaBlendFactor(MTLBlendFactor::OneMinusSourceAlpha);
                    wattach.setAlphaBlendOperation(MTLBlendOperation::Add);
                }

                let pipeline_state = self
                    .device
                    .newRenderPipelineStateWithDescriptor_error(&wdesc)
                    .map_err(|_| BackendError::MetalError)?;
                self.widget_pipelines
                    .insert(widget_type.to_string(), pipeline_state);
            }

            let waveform_src = NSString::from_str(WAVEFORM_SHADER_SRC);
            let waveform_lib = self
                .device
                .newLibraryWithSource_options_error(&waveform_src, None)
                .map_err(|_| BackendError::MetalError)?;
            let waveform_vert = waveform_lib
                .newFunctionWithName(&NSString::from_str("waveform_vert"))
                .ok_or(BackendError::MetalError)?;
            let waveform_frag = waveform_lib
                .newFunctionWithName(&NSString::from_str("waveform_frag"))
                .ok_or(BackendError::MetalError)?;
            let waveform_desc = MTLRenderPipelineDescriptor::new();
            waveform_desc.setVertexFunction(Some(&waveform_vert));
            waveform_desc.setFragmentFunction(Some(&waveform_frag));
            let waveform_attach =
                unsafe { waveform_desc.colorAttachments().objectAtIndexedSubscript(0) };
            waveform_attach.setPixelFormat(MTLPixelFormat::BGRA8Unorm);
            waveform_attach.setBlendingEnabled(true);
            {
                use objc2_metal::{MTLBlendFactor, MTLBlendOperation};
                waveform_attach.setSourceRGBBlendFactor(MTLBlendFactor::SourceAlpha);
                waveform_attach.setDestinationRGBBlendFactor(MTLBlendFactor::OneMinusSourceAlpha);
                waveform_attach.setRgbBlendOperation(MTLBlendOperation::Add);
                waveform_attach.setSourceAlphaBlendFactor(MTLBlendFactor::One);
                waveform_attach.setDestinationAlphaBlendFactor(MTLBlendFactor::OneMinusSourceAlpha);
                waveform_attach.setAlphaBlendOperation(MTLBlendOperation::Add);
            }
            self.waveform_pipeline = Some(
                self.device
                    .newRenderPipelineStateWithDescriptor_error(&waveform_desc)
                    .map_err(|_| BackendError::MetalError)?,
            );

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
            let modifiers = &mut self.modifiers;
            let pressed_mouse_button = &mut self.pressed_mouse_button;
            let cursor_cell = &mut self.cursor_cell;
            let cursor_pos = &mut self.cursor_pos;
            let layer_ref = &self.layer;
            let window_ref = self.window.as_ref();
            let cell_size = self
                .atlas
                .as_ref()
                .map(|a| (a.cell_w.max(1) as f64, a.cell_h.max(1) as f64))
                .unwrap_or((8.0, 16.0));
            event_loop.pump_events(Some(timeout), |event, elwt| {
                elwt.set_control_flow(ControlFlow::Wait);
                let WEvent::WindowEvent { event, .. } = event else {
                    return;
                };
                match event {
                    WindowEvent::CloseRequested => {
                        pending.push_back(Event::Key(KeyEvent::new(
                            KeyCode::Char('c'),
                            KeyModifiers::CONTROL,
                        )));
                    }
                    WindowEvent::Resized(new_size) => {
                        layer_ref.setDrawableSize(CGSize {
                            width: new_size.width as f64,
                            height: new_size.height as f64,
                        });
                        // Ask macOS to send RedrawRequested during the modal drag loop.
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
                    WindowEvent::ModifiersChanged(mods) => {
                        *modifiers = winit_mods_to_crossterm(mods.state());
                    }
                    WindowEvent::KeyboardInput { event: kev, .. } => {
                        if kev.state != ElementState::Pressed {
                            return;
                        }
                        if let Some(ev) =
                            translate_key(&kev.logical_key, &kev.physical_key, *modifiers)
                        {
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
                            // Coalesce Moved events — only keep the latest for hover detection
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
                                pending.push_back(Event::Mouse(MouseEvent {
                                    kind: MouseEventKind::Up(button),
                                    column: cursor_cell.0,
                                    row: cursor_cell.1,
                                    modifiers: *modifiers,
                                }));
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
                        let kind = match delta {
                            MouseScrollDelta::LineDelta(x, y) => {
                                if y > 0.0 {
                                    Some(MouseEventKind::ScrollUp)
                                } else if y < 0.0 {
                                    Some(MouseEventKind::ScrollDown)
                                } else if x > 0.0 {
                                    Some(MouseEventKind::ScrollLeft)
                                } else if x < 0.0 {
                                    Some(MouseEventKind::ScrollRight)
                                } else {
                                    None
                                }
                            }
                            MouseScrollDelta::PixelDelta(delta) => {
                                pending_scroll
                                    .push_back(((delta.x as f32, delta.y as f32), *cursor_pos));
                                None
                            }
                        };
                        if let Some(kind) = kind {
                            pending.push_back(Event::Mouse(MouseEvent {
                                kind,
                                column: cursor_cell.0,
                                row: cursor_cell.1,
                                modifiers: *modifiers,
                            }));
                        }
                    }
                    WindowEvent::TouchpadMagnify { delta, phase, .. } => {
                        if matches!(phase, TouchPhase::Ended | TouchPhase::Cancelled) {
                            return;
                        }
                        pending_magnify.push_back((delta, *cursor_pos));
                    }
                    _ => {}
                }
            });
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

        fn render(&mut self, frame: &RenderFrame) -> Result<(), BackendError> {
            crate::widget_render::sdf_widget::set_sdf_time_seconds(self.elapsed_time_seconds());
            self.sync_window_theme();
            let time_seconds = self.elapsed_time_seconds();

            let (Some(pipeline), Some(atlas)) = (&self.pipeline, &mut self.atlas) else {
                return Ok(());
            };

            // ── Draw ─────────────────────────────────────────────────────────
            // Get the drawable first so we know the exact texture dimensions.
            let Some(drawable) = self.layer.nextDrawable() else {
                return Ok(());
            };
            let texture = drawable.texture();
            let vp_w = texture.width() as f32;
            let vp_h = texture.height() as f32;
            let cell_w = atlas.cell_w as f32;
            let cell_h = atlas.cell_h as f32;

            // ── Build/cached text vertex data ───────────────────────────────
            let mut text_upload_bytes = 0;
            if self.cached_text_key != Some(frame.text_cache_key) {
                self.cached_text_quads = build_text_quads(frame, atlas, vp_w, vp_h);
                self.cached_text_key = Some(frame.text_cache_key);
                self.cached_text_vertex_count = self.cached_text_quads.len();
                self.cached_text_buffer = if self.cached_text_quads.is_empty() {
                    None
                } else {
                    let byte_len = std::mem::size_of_val(self.cached_text_quads.as_slice());
                    text_upload_bytes = byte_len;
                    unsafe {
                        self.device.newBufferWithBytes_length_options(
                            NonNull::new(self.cached_text_quads.as_ptr() as *mut _).unwrap(),
                            byte_len,
                            MTLResourceOptions(0),
                        )
                    }
                };
            }
            let max_rows = (vp_h / atlas.cell_h as f32).floor() as u16 - 1;
            let primitive_scene = frame
                .widget_layout
                .as_ref()
                .map(|layout| {
                    widget_render::collect_metal_primitives(
                        layout,
                        WidgetViewport {
                            cell_w: atlas.cell_w as f32,
                            cell_h: atlas.cell_h as f32,
                            vp_w,
                            vp_h,
                            time_seconds,
                            focused_widget_id: frame.focused_widget_id,
                            focused_branch: false,
                        },
                        frame.widget_scroll_top,
                        max_rows,
                    )
                })
                .unwrap_or_default();
            let primitive_quads = build_widget_primitive_quads(&primitive_scene, atlas, vp_w, vp_h);
            let primitive_instance_runs = collect_widget_instance_runs(&primitive_scene);

            // ── Vertex buffer ────────────────────────────────────────────────
            let text_vbuf = self.cached_text_buffer.as_ref();
            let label_vbuf = if primitive_quads.is_empty() {
                None
            } else {
                let byte_len = std::mem::size_of_val(primitive_quads.as_slice());
                unsafe {
                    self.device.newBufferWithBytes_length_options(
                        NonNull::new(primitive_quads.as_ptr() as *mut _).unwrap(),
                        byte_len,
                        MTLResourceOptions(0),
                    )
                }
            };

            let desc = MTLRenderPassDescriptor::new();
            let attach = unsafe { desc.colorAttachments().objectAtIndexedSubscript(0) };
            attach.setTexture(Some(&texture));
            attach.setLoadAction(MTLLoadAction::Clear);
            attach.setClearColor(MTLClearColor {
                red: theme::BG().r as f64,
                green: theme::BG().g as f64,
                blue: theme::BG().b as f64,
                alpha: 1.0,
            });
            attach.setStoreAction(MTLStoreAction::Store);

            let buf = self
                .command_queue
                .commandBuffer()
                .ok_or(BackendError::MetalError)?;
            let enc = buf
                .renderCommandEncoderWithDescriptor(&desc)
                .ok_or(BackendError::MetalError)?;

            if let Some(vbuf) = &text_vbuf {
                enc.setRenderPipelineState(pipeline);
                unsafe {
                    enc.setVertexBuffer_offset_atIndex(Some(vbuf), 0, 0);
                    enc.setFragmentTexture_atIndex(Some(&atlas.texture), 0);
                    enc.drawPrimitives_vertexStart_vertexCount(
                        MTLPrimitiveType::Triangle,
                        0,
                        self.cached_text_vertex_count as _,
                    );
                }
            }

            if let Some(vbuf) = &label_vbuf {
                enc.setRenderPipelineState(pipeline);
                unsafe {
                    enc.setVertexBuffer_offset_atIndex(Some(vbuf), 0, 0);
                    enc.setFragmentTexture_atIndex(Some(&atlas.texture), 0);
                    enc.drawPrimitives_vertexStart_vertexCount(
                        MTLPrimitiveType::Triangle,
                        0,
                        primitive_quads.len() as _,
                    );
                }
            }

            // Proportional text: separate atlas + linear-filtering pipeline.
            if let (Some(prop_atlas), Some(prop_pipe)) =
                (self.prop_atlas.as_mut(), self.prop_pipeline.as_ref())
            {
                let prop_verts = build_proportional_text_quads(
                    &primitive_scene,
                    prop_atlas,
                    cell_w,
                    cell_h,
                    vp_w,
                    vp_h,
                );
                if !prop_verts.is_empty() {
                    let byte_len = std::mem::size_of_val(prop_verts.as_slice());
                    if let Some(pvbuf) = unsafe {
                        self.device.newBufferWithBytes_length_options(
                            NonNull::new(prop_verts.as_ptr() as *mut _).unwrap(),
                            byte_len,
                            MTLResourceOptions(0),
                        )
                    } {
                        enc.setRenderPipelineState(prop_pipe);
                        unsafe {
                            enc.setVertexBuffer_offset_atIndex(Some(&pvbuf), 0, 0);
                            enc.setFragmentTexture_atIndex(Some(&prop_atlas.texture), 0);
                            enc.drawPrimitives_vertexStart_vertexCount(
                                MTLPrimitiveType::Triangle,
                                0,
                                prop_verts.len() as _,
                            );
                        }
                    }
                }
            }

            if let Some(waveform_pipeline) = self.waveform_pipeline.clone() {
                let waveform_primitives = collect_waveform_primitives(&primitive_scene);
                self.draw_waveform_primitives(
                    &enc,
                    &waveform_pipeline,
                    &waveform_primitives,
                    cell_w,
                    cell_h,
                    vp_w,
                    vp_h,
                );
            }

            self.compile_pending_sdf_pipelines();
            let mut widget_upload_bytes = 0;
            for (widget_type, instances) in &primitive_instance_runs {
                let Some(wpipe) = self.widget_pipelines.get(widget_type) else {
                    continue;
                };
                if instances.is_empty() {
                    continue;
                }
                let byte_len = std::mem::size_of_val(instances.as_slice());
                widget_upload_bytes += byte_len;
                let Some(wbuf) = (unsafe {
                    self.device.newBufferWithBytes_length_options(
                        NonNull::new(instances.as_ptr() as *mut _).unwrap(),
                        byte_len,
                        MTLResourceOptions(0),
                    )
                }) else {
                    continue;
                };
                enc.setRenderPipelineState(wpipe);
                unsafe {
                    enc.setVertexBuffer_offset_atIndex(Some(&wbuf), 0, 0);
                    enc.drawPrimitives_vertexStart_vertexCount_instanceCount(
                        MTLPrimitiveType::Triangle,
                        0,
                        6,
                        instances.len() as _,
                    );
                }
            }

            let text_bytes = text_upload_bytes;
            let label_bytes = primitive_quads.len() * std::mem::size_of::<Vertex>();
            let widget_bytes = widget_upload_bytes;

            enc.endEncoding();
            buf.presentDrawable(objc2::runtime::ProtocolObject::from_ref(&*drawable));
            buf.commit();
            self.stats.note_frame(text_bytes, label_bytes, widget_bytes);
            Ok(())
        }
    }

    struct RenderStats {
        window_start: Instant,
        frames: u64,
        text_bytes: usize,
        label_bytes: usize,
        widget_bytes: usize,
    }

    impl RenderStats {
        fn new() -> Self {
            Self {
                window_start: Instant::now(),
                frames: 0,
                text_bytes: 0,
                label_bytes: 0,
                widget_bytes: 0,
            }
        }

        fn note_frame(&mut self, text_bytes: usize, label_bytes: usize, widget_bytes: usize) {
            self.frames += 1;
            self.text_bytes += text_bytes;
            self.label_bytes += label_bytes;
            self.widget_bytes += widget_bytes;

            let elapsed = self.window_start.elapsed();
            if elapsed.as_secs_f64() < 1.0 {
                return;
            }

            let secs = elapsed.as_secs_f64();
            let fps = self.frames as f64 / secs;
            let total_mb =
                (self.text_bytes + self.label_bytes + self.widget_bytes) as f64 / (1024.0 * 1024.0);
            let mbps = total_mb / secs;
            eprintln!(
                "[metal-stats] fps={fps:.1} upload={mbps:.2}MB/s text={:.2}MB/s labels={:.2}MB/s widgets={:.2}MB/s",
                self.text_bytes as f64 / (1024.0 * 1024.0) / secs,
                self.label_bytes as f64 / (1024.0 * 1024.0) / secs,
                self.widget_bytes as f64 / (1024.0 * 1024.0) / secs,
            );

            self.window_start = Instant::now();
            self.frames = 0;
            self.text_bytes = 0;
            self.label_bytes = 0;
            self.widget_bytes = 0;
        }
    }

    fn rasterize_char(
        atlas: &mut GlyphAtlas,
        ch: char,
        (col, row): (i32, f32),
        ctx: &CharCtx,
        out: &mut Vec<Vertex>,
    ) {
        let Some(entry) = atlas.get_or_rasterize(ch) else {
            return;
        };
        let [u0, v0] = entry.uv_min;
        let [u1, v1] = entry.uv_max;

        let ndc_x = |px: f32| px / ctx.vp_w * 2.0 - 1.0;
        let ndc_y = |px: f32| 1.0 - px / ctx.vp_h * 2.0;
        let x0 = ndc_x(col as f32 * ctx.cell_w);
        let x1 = ndc_x((col + 1) as f32 * ctx.cell_w);
        let y0 = ndc_y(row * ctx.cell_h);
        let y1 = ndc_y((row + 1.0) * ctx.cell_h);

        let gv = |px, py, u, v| Vertex {
            position: [px, py],
            uv: [u, v],
            fg: ctx.fg,
            bg: ctx.bg,
        };
        out.extend_from_slice(&[
            gv(x0, y0, u0, v0),
            gv(x0, y1, u0, v1),
            gv(x1, y0, u1, v0),
            gv(x1, y0, u1, v0),
            gv(x0, y1, u0, v1),
            gv(x1, y1, u1, v1),
        ]);
    }

    /// Build vertices for proportional text primitives.
    /// Each glyph is rendered as a separate quad with alpha blending.
    fn build_proportional_text_quads(
        primitives: &[widget_render::MetalPrimitive],
        prop_atlas: &mut ProportionalGlyphAtlas,
        mono_cell_w: f32,
        mono_cell_h: f32,
        vp_w: f32,
        vp_h: f32,
    ) -> Vec<Vertex> {
        let mut verts = Vec::new();
        let ndc_x = |px: f32| px / vp_w * 2.0 - 1.0;
        let ndc_y = |px: f32| 1.0 - px / vp_h * 2.0;

        for prim in primitives {
            let widget_render::MetalPrimitive::ProportionalText(run) = prim else {
                continue;
            };

            let size_tenths = (run.font_size * 10.0).round() as u16;
            let fg = run.fg.to_rgba();
            let bg = [0.0, 0.0, 0.0, 0.0]; // Transparent — alpha blending handles bg

            let base_x_px = run.col * mono_cell_w;
            let base_y_px = run.row * mono_cell_h;
            let mut pen_x = base_x_px;

            for ch in run.text.chars() {
                let Some(entry) = prop_atlas.get_or_rasterize(ch, size_tenths) else {
                    continue;
                };
                let advance = entry.advance;

                if entry.raster_w == 0 || entry.raster_h == 0 {
                    pen_x += advance;
                    continue;
                }

                let [u0, v0] = entry.uv_min;
                let [u1, v1] = entry.uv_max;

                // Glyph bitmap starts 2px before pen (padding), spans full line height.
                let gx0 = pen_x - 2.0;
                let gy0 = base_y_px;
                let gx1 = gx0 + entry.raster_w as f32;
                let gy1 = gy0 + entry.raster_h as f32;

                let x0 = ndc_x(gx0);
                let x1 = ndc_x(gx1);
                let y0 = ndc_y(gy0);
                let y1 = ndc_y(gy1);

                let gv = |px, py, u, v| Vertex {
                    position: [px, py],
                    uv: [u, v],
                    fg,
                    bg,
                };
                verts.extend_from_slice(&[
                    gv(x0, y0, u0, v0),
                    gv(x0, y1, u0, v1),
                    gv(x1, y0, u1, v0),
                    gv(x1, y0, u1, v0),
                    gv(x0, y1, u0, v1),
                    gv(x1, y1, u1, v1),
                ]);

                pen_x += advance;
            }
        }
        verts
    }

    // ── Quad builder ──────────────────────────────────────────────────────────

    /// Convert a `RenderFrame` into a flat list of triangle vertices.
    ///
    /// Each cell becomes 6 vertices (2 triangles).
    /// Coordinate system:
    ///   - Screen pixel (0, 0) = top-left of window.
    ///   - Metal NDC: X ∈ [-1, +1] left→right, Y ∈ [-1, +1] bottom→top.
    ///   - Conversion: ndc_x = (px_x / vp_w) * 2 - 1
    ///                 ndc_y = 1 - (px_y / vp_h) * 2
    fn build_text_quads(
        frame: &RenderFrame,
        atlas: &mut GlyphAtlas,
        vp_w: f32,
        vp_h: f32,
    ) -> Vec<Vertex> {
        build_text_quads_offset(frame, atlas, vp_w, vp_h, TileOffset::default())
    }

    fn build_text_quads_offset(
        frame: &RenderFrame,
        atlas: &mut GlyphAtlas,
        vp_w: f32,
        vp_h: f32,
        offset: TileOffset,
    ) -> Vec<Vertex> {
        let cell_w = atlas.cell_w as f32;
        let cell_h = atlas.cell_h as f32;
        let mut verts = Vec::with_capacity(frame.lines.len() * 80 * 6);

        let ndc_x = |px: f32| px / vp_w * 2.0 - 1.0;
        let ndc_y = |px: f32| 1.0 - px / vp_h * 2.0;
        let to_rgba = |c: Color| [c.r, c.g, c.b, c.a];

        for (row, line) in frame.lines.iter().enumerate() {
            for (col, cell) in line.iter().enumerate() {
                let abs_col = col as i32 + offset.col;
                let abs_row = row as i32 + offset.row;
                let is_cursor = frame.cursor == Some((row, col));

                let x0 = ndc_x(abs_col as f32 * cell_w);
                let x1 = ndc_x((abs_col + 1) as f32 * cell_w);
                let y0 = ndc_y(abs_row as f32 * cell_h);
                let y1 = ndc_y((abs_row + 1) as f32 * cell_h);

                // Cursor inverts fg/bg; otherwise use cell style.
                let (fg, bg) = if is_cursor {
                    let cell_fg = cell.style.fg;
                    let cell_bg = cell.style.bg.unwrap_or(theme::BG());
                    (to_rgba(cell_bg), to_rgba(cell_fg))
                } else {
                    (
                        to_rgba(cell.style.fg),
                        to_rgba(cell.style.bg.unwrap_or(theme::BG())),
                    )
                };

                // Background quad — solid color, zero coverage glyph UV.
                let bg_v = |px, py| Vertex {
                    position: [px, py],
                    uv: [0.0, 0.0],
                    fg: bg,
                    bg,
                };
                verts.extend_from_slice(&[
                    bg_v(x0, y0),
                    bg_v(x0, y1),
                    bg_v(x1, y0),
                    bg_v(x1, y0),
                    bg_v(x0, y1),
                    bg_v(x1, y1),
                ]);

                // Glyph quad — skip spaces (cursor on space is handled by bg inversion above).
                if cell.ch == ' ' {
                    continue;
                }

                rasterize_char(
                    atlas,
                    cell.ch,
                    (abs_col, abs_row as f32),
                    &CharCtx {
                        cell_w,
                        cell_h,
                        vp_w,
                        vp_h,
                        fg,
                        bg,
                    },
                    &mut verts,
                );
            }
        }

        // ── Status bar (placed at bottom of tile region) ─────────────────────
        let total_rows = (vp_h / cell_h).floor() as usize;
        let status_row = if offset.col == 0 && offset.row == 0 {
            total_rows.saturating_sub(1) // legacy single-tile: bottom of screen
        } else {
            // Skip status bar for offset tiles — handled by tiled renderer
            return verts;
        };
        let status_bg = to_rgba(theme::STATUS_BG());

        // ── Completion popup ─────────────────────────────────────────────────
        if let Some(comp) = &frame.completion {
            let label_w = comp
                .entries
                .iter()
                .map(|e| e.label.len())
                .max()
                .unwrap_or(0)
                .max(12);
            let popup_col = comp.anchor.1;
            let popup_row = comp.anchor.0 + 1; // one row below the cursor

            let sel_bg = to_rgba(theme::COMP_SELECTED_BG());
            let unsel_bg = to_rgba(theme::COMP_UNSELECTED_BG());
            let pop_fg = to_rgba(theme::COMP_FG());

            let x0 = ndc_x(popup_col as f32 * cell_w);
            let x1 = ndc_x((popup_col + label_w) as f32 * cell_w);
            for (i, entry) in comp.entries.iter().enumerate() {
                let row = popup_row + i;
                if row >= status_row {
                    break;
                }
                let y0 = ndc_y(row as f32 * cell_h); // top (larger NDC Y)
                let y1 = ndc_y((row + 1) as f32 * cell_h); // bottom
                let bg = if entry.selected { sel_bg } else { unsel_bg };
                let gv = |px, py, u, v| Vertex {
                    position: [px, py],
                    uv: [u, v],
                    fg: pop_fg,
                    bg,
                };

                verts.extend_from_slice(&[
                    gv(x0, y0, 0.0, 0.0),
                    gv(x0, y1, 0.0, 0.0),
                    gv(x1, y0, 0.0, 0.0),
                    gv(x1, y0, 0.0, 0.0),
                    gv(x0, y1, 0.0, 0.0),
                    gv(x1, y1, 0.0, 0.0),
                ]);

                for (j, ch) in entry.label.chars().enumerate() {
                    let ch_row = row;
                    let ch_col = popup_col + j;

                    rasterize_char(
                        atlas,
                        ch,
                        (ch_col as i32, ch_row as f32),
                        &CharCtx {
                            cell_w,
                            cell_h,
                            vp_w,
                            vp_h,
                            fg: pop_fg,
                            bg,
                        },
                        &mut verts,
                    );
                }
            }

            // ── Doc panel (right of the list) ────────────────────────────────
            if let Some((title, body)) = &comp.doc {
                let doc_col = popup_col + label_w + 1;
                let doc_w: usize = 44;
                let doc_h = comp.entries.len().max(4);
                let doc_bg = to_rgba(theme::COMP_DOC_BG());
                let doc_fg = to_rgba(theme::COMP_DOC_FG());
                let title_fg = to_rgba(theme::COMP_DOC_TITLE_FG());

                // Background for the whole panel.
                let dx0 = ndc_x(doc_col as f32 * cell_w);
                let dx1 = ndc_x((doc_col + doc_w) as f32 * cell_w);
                let dy0 = ndc_y(popup_row as f32 * cell_h);
                let dy1 = ndc_y((popup_row + doc_h) as f32 * cell_h);
                let db = |px, py| Vertex {
                    position: [px, py],
                    uv: [0.0, 0.0],
                    fg: doc_bg,
                    bg: doc_bg,
                };
                verts.extend_from_slice(&[
                    db(dx0, dy0),
                    db(dx0, dy1),
                    db(dx1, dy0),
                    db(dx1, dy0),
                    db(dx0, dy1),
                    db(dx1, dy1),
                ]);

                // Title on row 0.
                let title_row = popup_row;
                if title_row < status_row {
                    for (j, ch) in title.chars().take(doc_w).enumerate() {
                        if ch == ' ' {
                            continue;
                        }
                        rasterize_char(
                            atlas,
                            ch,
                            ((doc_col + j) as i32, title_row as f32),
                            &CharCtx {
                                cell_w,
                                cell_h,
                                vp_w,
                                vp_h,
                                fg: title_fg,
                                bg: doc_bg,
                            },
                            &mut verts,
                        );
                    }
                }

                // Body lines starting at row 2 (row 1 is the blank separator).
                for (li, line) in body.iter().enumerate() {
                    let doc_row = popup_row + 2 + li;
                    if doc_row >= popup_row + doc_h || doc_row >= status_row {
                        break;
                    }
                    for (j, ch) in line.chars().take(doc_w).enumerate() {
                        if ch == ' ' {
                            continue;
                        }
                        rasterize_char(
                            atlas,
                            ch,
                            ((doc_col + j) as i32, doc_row as f32),
                            &CharCtx {
                                cell_w,
                                cell_h,
                                vp_w,
                                vp_h,
                                fg: doc_fg,
                                bg: doc_bg,
                            },
                            &mut verts,
                        );
                    }
                }
            }
        }

        // Fill the whole status row with background first.
        let total_cols = (vp_w / cell_w).floor() as usize;
        let sx0 = ndc_x(0.0);
        let sx1 = ndc_x(total_cols as f32 * cell_w);
        let sy0 = ndc_y(status_row as f32 * cell_h);
        let sy1 = ndc_y((status_row + 1) as f32 * cell_h);
        let sb = |px, py| Vertex {
            position: [px, py],
            uv: [0.0, 0.0],
            fg: status_bg,
            bg: status_bg,
        };
        verts.extend_from_slice(&[
            sb(sx0, sy0),
            sb(sx0, sy1),
            sb(sx1, sy0),
            sb(sx1, sy0),
            sb(sx0, sy1),
            sb(sx1, sy1),
        ]);
        push_horizontal_rule(
            &mut verts,
            0.0,
            status_row as f32 * cell_h,
            total_cols as f32 * cell_w,
            1.0,
            theme::STATUS_EDGE(),
            vp_w,
            vp_h,
        );

        // Render each styled character in the status row.
        for (col, cell) in frame.status_cells.iter().enumerate() {
            if col >= total_cols {
                break;
            }
            let ch = cell.ch;
            if ch == ' ' {
                continue;
            }
            let fg = to_rgba(cell.style.fg);
            let bg = to_rgba(cell.style.bg.unwrap_or(theme::STATUS_BG()));

            rasterize_char(
                atlas,
                ch,
                (col as i32, status_row as f32),
                &CharCtx {
                    cell_w,
                    cell_h,
                    vp_w,
                    vp_h,
                    fg,
                    bg,
                },
                &mut verts,
            );
        }

        verts
    }

    fn build_widget_primitive_quads(
        primitives: &[widget_render::MetalPrimitive],
        atlas: &mut GlyphAtlas,
        vp_w: f32,
        vp_h: f32,
    ) -> Vec<Vertex> {
        let cell_w = atlas.cell_w as f32;
        let cell_h = atlas.cell_h as f32;
        let mut verts = Vec::new();
        for primitive in primitives {
            match primitive {
                widget_render::MetalPrimitive::Rect(rect) => {
                    push_solid_rect_vertices(
                        rect.rect, rect.color, cell_w, cell_h, vp_w, vp_h, &mut verts,
                    );
                }
                widget_render::MetalPrimitive::Quad(quad) => {
                    push_solid_quad_vertices(*quad, cell_w, cell_h, vp_w, vp_h, &mut verts);
                }
                widget_render::MetalPrimitive::GlyphRun(run) => {
                    for (idx, ch) in run.text.chars().enumerate() {
                        if ch == ' ' {
                            continue;
                        }
                        rasterize_char(
                            atlas,
                            ch,
                            (run.col + idx as i32, run.row),
                            &CharCtx {
                                cell_w,
                                cell_h: atlas.cell_h as f32,
                                vp_w,
                                vp_h,
                                fg: run.fg.to_rgba(),
                                bg: run.bg.to_rgba(),
                            },
                            &mut verts,
                        );
                    }
                }
                // Proportional text is rendered in a separate pass with its own atlas.
                widget_render::MetalPrimitive::ProportionalText(_) => {}
                widget_render::MetalPrimitive::Waveform(_) => {}
                widget_render::MetalPrimitive::WidgetInstance { .. } => {}
            }
        }
        verts
    }

    fn collect_waveform_primitives(
        primitives: &[widget_render::MetalPrimitive],
    ) -> Vec<widget_render::MetalWaveformPrimitive> {
        primitives
            .iter()
            .filter_map(|primitive| match primitive {
                widget_render::MetalPrimitive::Waveform(waveform) => Some(waveform.clone()),
                _ => None,
            })
            .collect()
    }

    fn push_solid_rect_vertices(
        rect: Rect,
        color: Color,
        cell_w: f32,
        cell_h: f32,
        vp_w: f32,
        vp_h: f32,
        verts: &mut Vec<Vertex>,
    ) {
        let ndc_x = |px: f32| px / vp_w * 2.0 - 1.0;
        let ndc_y = |px: f32| 1.0 - px / vp_h * 2.0;
        let x0 = ndc_x(rect.col * cell_w);
        let x1 = ndc_x((rect.col + rect.width) * cell_w);
        let y0 = ndc_y(rect.row * cell_h);
        let y1 = ndc_y((rect.row + rect.height) * cell_h);
        let rgba = color.to_rgba();
        let v = |px, py| Vertex {
            position: [px, py],
            uv: [0.0, 0.0],
            fg: rgba,
            bg: rgba,
        };
        verts.extend_from_slice(&[
            v(x0, y0),
            v(x0, y1),
            v(x1, y0),
            v(x1, y0),
            v(x0, y1),
            v(x1, y1),
        ]);
    }

    fn push_horizontal_rule(
        verts: &mut Vec<Vertex>,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        color: Color,
        vp_w: f32,
        vp_h: f32,
    ) {
        let ndc_x = |px: f32| px / vp_w * 2.0 - 1.0;
        let ndc_y = |px: f32| 1.0 - px / vp_h * 2.0;
        let rgba = color.to_rgba();
        let x0 = ndc_x(x);
        let x1 = ndc_x(x + width);
        let y0 = ndc_y(y);
        let y1 = ndc_y(y + height);
        let v = |px, py| Vertex {
            position: [px, py],
            uv: [0.0, 0.0],
            fg: rgba,
            bg: rgba,
        };
        verts.extend_from_slice(&[
            v(x0, y0),
            v(x0, y1),
            v(x1, y0),
            v(x1, y0),
            v(x0, y1),
            v(x1, y1),
        ]);
    }

    fn push_solid_quad_vertices(
        quad: widget_render::MetalQuadPrimitive,
        cell_w: f32,
        cell_h: f32,
        vp_w: f32,
        vp_h: f32,
        verts: &mut Vec<Vertex>,
    ) {
        let ndc_x = |px: f32| px / vp_w * 2.0 - 1.0;
        let ndc_y = |px: f32| 1.0 - px / vp_h * 2.0;
        let x0 = ndc_x(quad.x * cell_w);
        let x1 = ndc_x((quad.x + quad.width) * cell_w);
        let y0 = ndc_y(quad.y * cell_h);
        let y1 = ndc_y((quad.y + quad.height) * cell_h);
        let rgba = quad.color.to_rgba();
        let v = |px, py| Vertex {
            position: [px, py],
            uv: [0.0, 0.0],
            fg: rgba,
            bg: rgba,
        };
        verts.extend_from_slice(&[
            v(x0, y0),
            v(x0, y1),
            v(x1, y0),
            v(x1, y0),
            v(x0, y1),
            v(x1, y1),
        ]);
    }

    fn collect_widget_instance_runs(
        primitives: &[widget_render::MetalPrimitive],
    ) -> Vec<(String, Vec<WidgetInstance>)> {
        let mut runs: Vec<(String, Vec<WidgetInstance>)> = Vec::new();
        for primitive in primitives {
            if let widget_render::MetalPrimitive::WidgetInstance {
                widget_type,
                instance,
            } = primitive
            {
                if let Some((run_type, instances)) = runs.last_mut()
                    && run_type == widget_type
                {
                    instances.push(*instance);
                } else {
                    runs.push((widget_type.clone(), vec![*instance]));
                }
            }
        }
        runs
    }

    /// Offset a MetalPrimitive by (col_off, row_off) cells.
    /// For Rect/Quad/GlyphRun: shift cell coordinates.
    /// For WidgetInstance: shift NDC bounds using the pixel conversion.
    /// Offset a MetalPrimitive by (col_off, row_off) cells (signed for scroll).
    fn offset_primitive(
        prim: widget_render::MetalPrimitive,
        col_off: i32,
        row_off: i32,
        cell_w: f32,
        cell_h: f32,
        vp_w: f32,
        vp_h: f32,
    ) -> widget_render::MetalPrimitive {
        match prim {
            widget_render::MetalPrimitive::Rect(mut r) => {
                r.rect.col += col_off as f32;
                r.rect.row += row_off as f32;
                widget_render::MetalPrimitive::Rect(r)
            }
            widget_render::MetalPrimitive::Quad(mut q) => {
                q.x += col_off as f32;
                q.y += row_off as f32;
                widget_render::MetalPrimitive::Quad(q)
            }
            widget_render::MetalPrimitive::GlyphRun(mut g) => {
                g.col += col_off;
                g.row += row_off as f32;
                widget_render::MetalPrimitive::GlyphRun(g)
            }
            widget_render::MetalPrimitive::ProportionalText(mut p) => {
                p.col += col_off as f32;
                p.row += row_off as f32;
                widget_render::MetalPrimitive::ProportionalText(p)
            }
            widget_render::MetalPrimitive::Waveform(mut w) => {
                w.rect.col += col_off as f32;
                w.rect.row += row_off as f32;
                widget_render::MetalPrimitive::Waveform(w)
            }
            widget_render::MetalPrimitive::WidgetInstance {
                widget_type,
                mut instance,
            } => {
                let ndc_dx = (col_off as f32 * cell_w / vp_w) * 2.0;
                let ndc_dy = -(row_off as f32 * cell_h / vp_h) * 2.0;
                instance.ndc_min[0] += ndc_dx;
                instance.ndc_max[0] += ndc_dx;
                instance.ndc_min[1] += ndc_dy;
                instance.ndc_max[1] += ndc_dy;
                widget_render::MetalPrimitive::WidgetInstance {
                    widget_type,
                    instance,
                }
            }
        }
    }

    // ── Helpers ───────────────────────────────────────────────────────────────

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
        out
    }

    fn translate_key(key: &Key, physical_key: &PhysicalKey, mods: KeyModifiers) -> Option<Event> {
        let code = if mods.intersects(KeyModifiers::ALT | KeyModifiers::CONTROL) {
            translate_physical_shortcut_key(physical_key).or_else(|| translate_logical_key(key))?
        } else {
            translate_logical_key(key)?
        };
        Some(Event::Key(KeyEvent::new(code, mods)))
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
}

#[cfg(target_os = "macos")]
pub use inner::MetalBackend;
