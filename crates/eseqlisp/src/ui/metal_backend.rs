/// Metal GPU backend for eseqlisp.
#[cfg(target_os = "macos")]
mod inner {
    use std::collections::hash_map::DefaultHasher;
    use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
    use std::fs;
    use std::hash::{Hash, Hasher};
    use std::ops::Range;
    use std::path::PathBuf;
    use std::ptr::NonNull;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    use crossterm::event::{
        Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent,
        MouseEventKind,
    };
    use objc2::rc::Retained;
    use objc2::runtime::ProtocolObject;
    use objc2_app_kit::NSView;
    use objc2_core_foundation::CGSize;
    use objc2_foundation::NSString;
    use objc2_metal::{
        MTLBlendFactor, MTLBuffer, MTLClearColor, MTLCommandBuffer, MTLCommandBufferStatus,
        MTLCommandEncoder, MTLCommandQueue, MTLCreateSystemDefaultDevice, MTLDevice, MTLLibrary,
        MTLLoadAction, MTLOrigin, MTLPixelFormat, MTLPrimitiveType, MTLRegion,
        MTLRenderCommandEncoder, MTLRenderPassDescriptor, MTLRenderPipelineDescriptor,
        MTLRenderPipelineState, MTLResourceOptions, MTLScissorRect, MTLSize, MTLStorageMode,
        MTLStoreAction, MTLTexture, MTLTextureDescriptor, MTLTextureUsage,
    };
    use objc2_quartz_core::{CAMetalDrawable, CAMetalLayer};
    use winit::{
        dpi::LogicalSize,
        event::{
            ElementState, Event as WEvent, MouseButton as WMouseButton, MouseScrollDelta,
            TouchPhase, WindowEvent,
        },
        event_loop::{ControlFlow, EventLoop},
        keyboard::{Key, KeyCode as WinitKeyCode, NamedKey, PhysicalKey},
        platform::pump_events::EventLoopExtPumpEvents,
        raw_window_handle::{HasWindowHandle, RawWindowHandle},
        window::{CursorIcon, Window},
    };

    use crate::audio::sample::get_registered_sample;
    use crate::backend::{Backend, BackendError, Color, RenderFrame, TiledRenderFrame};
    use crate::glyph_atlas::{GlyphAtlas, ProportionalGlyphAtlas, SizedFontCache};
    use crate::layout::{LayoutNode, Rect, TextMeasurer};
    use crate::theme;
    use crate::vm::Value;
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

    const IMAGE_SHADER_SRC: &str = r#"
#include <metal_stdlib>
using namespace metal;

struct ImageVertex {
    packed_float2 position;
    packed_float2 uv;
    float  opacity;
    packed_float2 local_pos;
    packed_float2 half_size;
    float  radius;
    float  rotation;
    float  clip_circle;
};

struct ImageVaryings {
    float4 position [[position]];
    float2 uv;
    float  opacity;
    float2 local_pos;
    float2 half_size;
    float  radius;
    float  rotation;
    float  clip_circle;
};

vertex ImageVaryings image_vert(
    uint vid [[vertex_id]],
    device const ImageVertex* verts [[buffer(0)]])
{
    ImageVertex v = verts[vid];
    ImageVaryings out;
    out.position = float4(v.position, 0.0, 1.0);
    out.uv = v.uv;
    out.opacity = v.opacity;
    out.local_pos = v.local_pos;
    out.half_size = v.half_size;
    out.radius = v.radius;
    out.rotation = v.rotation;
    out.clip_circle = v.clip_circle;
    return out;
}

float image_rounded_rect_sdf(float2 p, float2 half_size, float radius) {
    float2 q = abs(p) - half_size + radius;
    return length(max(q, 0.0)) + min(max(q.x, q.y), 0.0) - radius;
}

fragment float4 image_frag(
    ImageVaryings in [[stage_in]],
    texture2d<float> image_tex [[texture(0)]])
{
    constexpr sampler s(address::clamp_to_edge, filter::linear);
    float2 uv = in.uv;
    if (abs(in.rotation) > 0.0001) {
        float c = cos(in.rotation);
        float sn = sin(in.rotation);
        float2 p = uv - float2(0.5, 0.5);
        uv = float2(c * p.x - sn * p.y, sn * p.x + c * p.y) + float2(0.5, 0.5);
    }
    float4 color = image_tex.sample(s, uv);
    if (in.clip_circle > 0.5) {
        float d = length(in.local_pos) - min(in.half_size.x, in.half_size.y);
        float aa = max(fwidth(d), 0.001);
        color.a *= smoothstep(aa, -aa, d);
    } else if (in.radius > 0.0) {
        float radius = min(in.radius, min(in.half_size.x, in.half_size.y));
        float d = image_rounded_rect_sdf(in.local_pos, in.half_size, radius);
        float aa = max(fwidth(d), 0.001);
        color.a *= smoothstep(aa, -aa, d);
    }
    color.a *= in.opacity;
    return color;
}
"#;

    const PATCH_CABLE_SHADER_SRC: &str = r#"
#include <metal_stdlib>
using namespace metal;

struct PatchCableInstance {
    packed_float2 ndc_min;
    packed_float2 ndc_max;
    packed_float2 bounds_min;
    packed_float2 bounds_max;
    packed_float2 start;
    packed_float2 control1;
    packed_float2 control2;
    packed_float2 end;
    float4 color;
    float radius_px;
    float is_segmented;
    float segment_y_px;
    float corner_radius_px;
};

struct PatchCableVaryings {
    float4 position [[position]];
    float2 pixel_pos;
    float2 start;
    float2 control1;
    float2 control2;
    float2 end;
    float4 color;
    float radius_px;
    float is_segmented;
    float segment_y_px;
    float corner_radius_px;
};

float2 patch_cable_quad_corner(uint vid) {
    switch (vid) {
        case 0: return float2(0.0, 0.0);
        case 1: return float2(0.0, 1.0);
        case 2: return float2(1.0, 0.0);
        case 3: return float2(1.0, 0.0);
        case 4: return float2(0.0, 1.0);
        default: return float2(1.0, 1.0);
    }
}

vertex PatchCableVaryings patch_cable_vert(
    uint vid [[vertex_id]],
    uint iid [[instance_id]],
    device const PatchCableInstance* instances [[buffer(0)]])
{
    PatchCableInstance cable = instances[iid];
    float2 uv = patch_cable_quad_corner(vid);
    PatchCableVaryings out;
    float2 ndc_pos = mix(float2(cable.ndc_min), float2(cable.ndc_max), uv);
    out.position = float4(ndc_pos, 0.0, 1.0);
    out.pixel_pos = mix(float2(cable.bounds_min), float2(cable.bounds_max), uv);
    out.start = float2(cable.start);
    out.control1 = float2(cable.control1);
    out.control2 = float2(cable.control2);
    out.end = float2(cable.end);
    out.color = cable.color;
    out.radius_px = cable.radius_px;
    out.is_segmented = cable.is_segmented;
    out.segment_y_px = cable.segment_y_px;
    out.corner_radius_px = cable.corner_radius_px;
    return out;
}

float2 patch_cable_bezier(float2 p0, float2 p1, float2 p2, float2 p3, float t) {
    float u = 1.0 - t;
    float tt = t * t;
    float uu = u * u;
    return (uu * u) * p0
        + (3.0 * uu * t) * p1
        + (3.0 * u * tt) * p2
        + (tt * t) * p3;
}

float patch_cable_curve_distance(
    float2 p,
    float2 p0,
    float2 p1,
    float2 p2,
    float2 p3)
{
    float min_dist = 1.0e6;
    for (int i = 0; i < 24; i++) {
        float t1 = float(i) / 24.0;
        float t2 = float(i + 1) / 24.0;
        float2 seg_start = patch_cable_bezier(p0, p1, p2, p3, t1);
        float2 seg_end = patch_cable_bezier(p0, p1, p2, p3, t2);
        float2 seg_vec = seg_end - seg_start;
        float seg_len_sq = max(dot(seg_vec, seg_vec), 0.0001);
        float t = clamp(dot(p - seg_start, seg_vec) / seg_len_sq, 0.0, 1.0);
        min_dist = min(min_dist, length(p - (seg_start + t * seg_vec)));
    }
    return min_dist;
}

float patch_cable_segment_distance(float2 point, float2 seg_start, float2 seg_end) {
    float2 seg_vec = seg_end - seg_start;
    float seg_len_sq = dot(seg_vec, seg_vec);
    if (seg_len_sq < 0.00001) {
        return length(point - seg_start);
    }
    float t = clamp(dot(point - seg_start, seg_vec) / seg_len_sq, 0.0, 1.0);
    return length(point - (seg_start + t * seg_vec));
}

float patch_cable_arc_distance(float2 point, float2 center, float radius, float2 corner) {
    float2 to_corner = corner - center;
    float2 to_point = point - center;
    bool valid_x = (to_corner.x > 0.0) ? (to_point.x >= 0.0) : (to_point.x <= 0.0);
    bool valid_y = (to_corner.y > 0.0) ? (to_point.y >= 0.0) : (to_point.y <= 0.0);
    if (valid_x && valid_y) {
        return abs(length(to_point) - radius);
    }
    return 1000000.0;
}

float patch_cable_segmented_distance_y_up(
    float2 p,
    float2 start,
    float2 end,
    float segment_y,
    float corner_radius)
{
    bool needs_five = end.y > segment_y;
    if (!needs_five) {
        bool going_down1 = start.y > segment_y;
        bool going_right = end.x > start.x;
        bool going_down2 = end.y < segment_y;
        float2 corner1 = float2(start.x, segment_y);
        float2 corner2 = float2(end.x, segment_y);
        float2 corner1_center;
        float2 corner2_center;
        if (going_down1) {
            corner1_center = going_right
                ? float2(start.x + corner_radius, segment_y + corner_radius)
                : float2(start.x - corner_radius, segment_y + corner_radius);
        } else {
            corner1_center = going_right
                ? float2(start.x + corner_radius, segment_y - corner_radius)
                : float2(start.x - corner_radius, segment_y - corner_radius);
        }
        if (going_down2) {
            corner2_center = going_right
                ? float2(end.x - corner_radius, segment_y - corner_radius)
                : float2(end.x + corner_radius, segment_y - corner_radius);
        } else {
            corner2_center = going_right
                ? float2(end.x - corner_radius, segment_y + corner_radius)
                : float2(end.x + corner_radius, segment_y + corner_radius);
        }
        float2 seg1_end = float2(start.x, going_down1 ? segment_y + corner_radius : segment_y - corner_radius);
        float2 seg3_start = float2(end.x, going_down2 ? segment_y - corner_radius : segment_y + corner_radius);
        float2 seg2_start = float2(going_right ? start.x + corner_radius : start.x - corner_radius, segment_y);
        float2 seg2_end = float2(going_right ? end.x - corner_radius : end.x + corner_radius, segment_y);
        return min(
            min(min(patch_cable_segment_distance(p, start, seg1_end), patch_cable_segment_distance(p, seg2_start, seg2_end)),
                min(patch_cable_segment_distance(p, seg3_start, end), patch_cable_arc_distance(p, corner1_center, corner_radius, corner1))),
            patch_cable_arc_distance(p, corner2_center, corner_radius, corner2));
    }

    bool going_right = end.x > start.x;
    float clearance = corner_radius * 2.0;
    float turnaround_y = end.y + clearance;
    float turnaround_x = end.x - clearance;
    bool seg4_going_right = end.x > turnaround_x;
    float2 corner1 = float2(start.x, segment_y);
    float2 corner1_center = going_right
        ? float2(start.x + corner_radius, segment_y + corner_radius)
        : float2(start.x - corner_radius, segment_y + corner_radius);
    float2 seg1_end = float2(start.x, segment_y + corner_radius);
    float2 corner2 = float2(turnaround_x, segment_y);
    float2 corner2_center = going_right
        ? float2(turnaround_x - corner_radius, segment_y + corner_radius)
        : float2(turnaround_x + corner_radius, segment_y + corner_radius);
    float2 seg2_start = float2(going_right ? start.x + corner_radius : start.x - corner_radius, segment_y);
    float2 seg2_end = float2(going_right ? turnaround_x - corner_radius : turnaround_x + corner_radius, segment_y);
    float2 corner3 = float2(turnaround_x, turnaround_y);
    float2 corner3_center = seg4_going_right
        ? float2(turnaround_x + corner_radius, turnaround_y - corner_radius)
        : float2(turnaround_x - corner_radius, turnaround_y - corner_radius);
    float2 seg3_start = float2(turnaround_x, segment_y + corner_radius);
    float2 seg3_end = float2(turnaround_x, turnaround_y - corner_radius);
    float2 corner4 = float2(end.x, turnaround_y);
    float2 corner4_center = seg4_going_right
        ? float2(end.x - corner_radius, turnaround_y - corner_radius)
        : float2(end.x + corner_radius, turnaround_y - corner_radius);
    float2 seg4_start = float2(seg4_going_right ? turnaround_x + corner_radius : turnaround_x - corner_radius, turnaround_y);
    float2 seg4_end = float2(seg4_going_right ? end.x - corner_radius : end.x + corner_radius, turnaround_y);
    float2 seg5_start = float2(end.x, turnaround_y - corner_radius);
    float min_seg = min(
        min(min(patch_cable_segment_distance(p, start, seg1_end), patch_cable_segment_distance(p, seg2_start, seg2_end)),
            min(patch_cable_segment_distance(p, seg3_start, seg3_end), patch_cable_segment_distance(p, seg4_start, seg4_end))),
        patch_cable_segment_distance(p, seg5_start, end));
    float min_corner = min(
        min(patch_cable_arc_distance(p, corner1_center, corner_radius, corner1),
            patch_cable_arc_distance(p, corner2_center, corner_radius, corner2)),
        min(patch_cable_arc_distance(p, corner3_center, corner_radius, corner3),
            patch_cable_arc_distance(p, corner4_center, corner_radius, corner4)));
    return min(min_seg, min_corner);
}

float patch_cable_segmented_distance(
    float2 p,
    float2 start,
    float2 end,
    float segment_y_px,
    float corner_radius)
{
    return patch_cable_segmented_distance_y_up(
        float2(p.x, -p.y),
        float2(start.x, -start.y),
        float2(end.x, -end.y),
        -segment_y_px,
        corner_radius);
}

fragment float4 patch_cable_frag(PatchCableVaryings in [[stage_in]])
{
    float min_dist_to_line = in.is_segmented > 0.5
        ? patch_cable_segmented_distance(
            in.pixel_pos,
            in.start,
            in.end,
            in.segment_y_px,
            in.corner_radius_px)
        : patch_cable_curve_distance(
            in.pixel_pos,
            in.start,
            in.control1,
            in.control2,
            in.end);

    float sdf = min_dist_to_line - in.radius_px;
    float derivative = max(fwidth(sdf), 0.0001);
    float alpha = smoothstep(derivative * 0.5, -derivative * 0.5, sdf);

    if (alpha <= 0.0) {
        discard_fragment();
    }

    float core_radius = in.radius_px * 0.48;
    float core_blend = 1.0 - smoothstep(
        max(core_radius - derivative, 0.0),
        core_radius + derivative,
        min_dist_to_line);
    float3 edge_color = in.color.rgb * 0.58;
    float3 core_color = mix(in.color.rgb, float3(1.0, 1.0, 1.0), 0.78);
    float3 cable_color = mix(edge_color, core_color, core_blend);
    float edge_alpha_scale = 0.62;
    float alpha_scale = mix(edge_alpha_scale, 1.0, core_blend);

    return float4(cable_color, in.color.a * alpha * alpha_scale);
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
    float  value_t       [[flat]];
    float  itime         [[flat]];
    float4 uniform_a     [[flat]];
    float4 uniform_b     [[flat]];
    float4 color_a       [[flat]];
    float4 color_b       [[flat]];
    float4 color_c       [[flat]];
    float4 color_d       [[flat]];
    float  aspect        [[flat]];
    float  corner_radius [[flat]];
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
    out.corner_radius = inst.corner_radius;
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

        fn compile_widget_shader_source_with_metal(
            vertex_src: Option<&str>,
            fragment_src: &str,
        ) -> Result<(), String> {
            let device = MTLCreateSystemDefaultDevice().ok_or("no Metal device".to_string())?;
            let full_src = format!(
                "{}{}{}",
                WIDGET_SHADER_PREAMBLE,
                vertex_src.unwrap_or(DEFAULT_WIDGET_VERTEX_SHADER),
                fragment_src
            );
            let src_ns = NSString::from_str(&full_src);
            device
                .newLibraryWithSource_options_error(&src_ns, None)
                .map(|_| ())
                .map_err(|err| format!("{:?}", err))
        }

        fn compile_widget_shader_with_metal(shader_src: &str) -> Result<(), String> {
            compile_widget_shader_source_with_metal(None, shader_src)
        }

        fn prop_text_run(text: &str) -> widget_render::MetalProportionalTextPrimitive {
            widget_render::MetalProportionalTextPrimitive {
                row: 2.0,
                col: 3.0,
                align_width: 8.0,
                h_align: 0.5,
                text: text.to_string(),
                font_size: 12.0,
                scale: 1.0,
                fg: Color::rgba(0.8, 0.7, 0.6, 1.0),
                bg: Color::rgba(0.0, 0.0, 0.0, 0.0),
            }
        }

        #[test]
        fn proportional_text_vertex_key_reuses_unchanged_label() {
            let first = prop_text_run("track");
            let second = prop_text_run("track");

            assert_eq!(
                ProportionalTextVertexKey::new(&first, 10.0, 20.0, 1000.0, 700.0),
                ProportionalTextVertexKey::new(&second, 10.0, 20.0, 1000.0, 700.0)
            );
        }

        #[test]
        fn proportional_text_vertex_key_invalidates_when_text_changes() {
            let first = prop_text_run("1");
            let second = prop_text_run("17");

            assert_ne!(
                ProportionalTextVertexKey::new(&first, 10.0, 20.0, 1000.0, 700.0),
                ProportionalTextVertexKey::new(&second, 10.0, 20.0, 1000.0, 700.0)
            );
        }

        #[test]
        fn proportional_text_vertex_key_invalidates_render_inputs() {
            let base = prop_text_run("track");
            let base_key = ProportionalTextVertexKey::new(&base, 10.0, 20.0, 1000.0, 700.0);

            let mut moved = base.clone();
            moved.col = 4.0;
            assert_ne!(
                base_key,
                ProportionalTextVertexKey::new(&moved, 10.0, 20.0, 1000.0, 700.0)
            );

            let mut resized = base.clone();
            resized.font_size = 13.0;
            assert_ne!(
                base_key,
                ProportionalTextVertexKey::new(&resized, 10.0, 20.0, 1000.0, 700.0)
            );

            let mut recolored = base.clone();
            recolored.fg = Color::rgba(0.2, 0.7, 0.6, 1.0);
            assert_ne!(
                base_key,
                ProportionalTextVertexKey::new(&recolored, 10.0, 20.0, 1000.0, 700.0)
            );

            assert_ne!(
                base_key,
                ProportionalTextVertexKey::new(&base, 11.0, 20.0, 1000.0, 700.0)
            );
        }

        #[test]
        fn registered_widget_shaders_compile_in_metal() {
            for (widget_type, vertex_src, fragment_src) in widget_render::widget_shader_sources() {
                compile_widget_shader_source_with_metal(vertex_src, fragment_src)
                    .unwrap_or_else(|err| panic!("{widget_type} widget shader failed: {err}"));
            }
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
    int show_selection_start;
    int show_selection_end;
    float playhead_position;
    int show_playhead;
    packed_float4 waveform_color;
    packed_float4 inactive_waveform_color;
    packed_float4 marker_color;
    packed_float4 active_marker_color;
    int active_selection_start;
    int active_selection_end;
    packed_float4 selection_color;
    packed_float4 bg_color;
    packed_float4 border_color;
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
    int show_selection_start [[flat]];
    int show_selection_end [[flat]];
    float playhead_position [[flat]];
    int show_playhead [[flat]];
    float4 waveform_color [[flat]];
    float4 inactive_waveform_color [[flat]];
    float4 marker_color [[flat]];
    float4 active_marker_color [[flat]];
    int active_selection_start [[flat]];
    int active_selection_end [[flat]];
    float4 selection_color [[flat]];
    float4 bg_color [[flat]];
    float4 border_color [[flat]];
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
    out.show_selection_start = inst.show_selection_start;
    out.show_selection_end = inst.show_selection_end;
    out.playhead_position = inst.playhead_position;
    out.show_playhead = inst.show_playhead;
    out.waveform_color = inst.waveform_color;
    out.inactive_waveform_color = inst.inactive_waveform_color;
    out.marker_color = inst.marker_color;
    out.active_marker_color = inst.active_marker_color;
    out.active_selection_start = inst.active_selection_start;
    out.active_selection_end = inst.active_selection_end;
    out.selection_color = inst.selection_color;
    out.bg_color = inst.bg_color;
    out.border_color = inst.border_color;
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
    float3 rgb = in.bg_color.rgb;
    float alpha = 0.0;

    bool has_selection = in.selection_end > in.selection_start + 0.001;
    if (has_selection &&
        content_uv.x >= in.selection_start &&
        content_uv.x <= in.selection_end) {
        rgb = mix(rgb, in.selection_color.rgb, 0.30);
        alpha = max(alpha, 0.06);
    }

    float center_line = 1.0 - smoothstep(0.0, 0.004, abs(content_uv.y - 0.5));
    float3 center_rgb = mix(in.bg_color.rgb, in.border_color.rgb, 0.5);
    rgb = mix(rgb, center_rgb, center_line * 0.20);
    alpha = max(alpha, center_line * 0.18);

    float boundary_width = max(fwidth(content_uv.x) * 0.9, 0.0015);
    float boundary_aa = max(fwidth(content_uv.x) * 0.75, 0.00075);
    float start_dist = abs(content_uv.x - in.selection_start);
    float end_dist = abs(content_uv.x - in.selection_end);
    float start_boundary = has_selection && in.show_selection_start == 1
        ? 1.0 - smoothstep(boundary_width, boundary_width + boundary_aa, start_dist)
        : 0.0;
    float end_boundary = has_selection && in.show_selection_end == 1
        ? 1.0 - smoothstep(boundary_width, boundary_width + boundary_aa, end_dist)
        : 0.0;
    float3 start_marker_rgb = in.active_selection_start == 1
        ? in.active_marker_color.rgb
        : in.marker_color.rgb;
    float3 end_marker_rgb = in.active_selection_end == 1
        ? in.active_marker_color.rgb
        : in.marker_color.rgb;
    rgb = mix(rgb, start_marker_rgb, start_boundary * 0.85);
    rgb = mix(rgb, end_marker_rgb, end_boundary * 0.85);
    float boundary_mask = max(start_boundary, end_boundary);
    alpha = max(alpha, boundary_mask * 0.75);

    float flag_height = 0.1575;
    float flag_width = flag_height / max(in.aspect_ratio, 0.0001);
    float flag_y = 1.0 - content_uv.y;
    float flag_taper = 1.0 - clamp(flag_y / flag_height, 0.0, 1.0);
    float start_flag_dx = content_uv.x - in.selection_start;
    float start_flag = has_selection && in.show_selection_start == 1 && flag_y <= flag_height &&
        start_flag_dx >= 0.0 && start_flag_dx <= flag_width * flag_taper
        ? 1.0
        : 0.0;
    float end_flag_dx = in.selection_end - content_uv.x;
    float end_flag = has_selection && in.show_selection_end == 1 && flag_y <= flag_height &&
        end_flag_dx >= 0.0 && end_flag_dx <= flag_width * flag_taper
        ? 1.0
        : 0.0;
    float flag_mask = max(start_flag, end_flag);
    rgb = mix(rgb, start_marker_rgb, start_flag);
    rgb = mix(rgb, end_marker_rgb, end_flag);
    alpha = max(alpha, flag_mask);

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

    bool in_selection = has_selection &&
        content_uv.x >= in.selection_start &&
        content_uv.x <= in.selection_end;
    float3 wave_color = in_selection ? in.waveform_color.rgb : in.inactive_waveform_color.rgb;
    float3 fill_color = mix(rgb, wave_color, 0.88);
    float3 edge_color = mix(wave_color, float3(1.0, 1.0, 1.0), 0.15);
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
            ((in.show_selection_start == 1 &&
              abs(in.playhead_position - in.selection_start) <= boundary_width + boundary_aa) ||
             (in.show_selection_end == 1 &&
              abs(in.playhead_position - in.selection_end) <= boundary_width + boundary_aa));
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
    rgb = mix(rgb, in.border_color.rgb, border_mask * 0.8);
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

    struct UploadedBufferSlice {
        buffer: Retained<ProtocolObject<dyn MTLBuffer>>,
        offset: usize,
    }

    struct GpuUploadFrame {
        buffer: Option<Retained<ProtocolObject<dyn MTLBuffer>>>,
        capacity: usize,
        cursor: usize,
        in_flight: Option<Retained<ProtocolObject<dyn MTLCommandBuffer>>>,
    }

    impl GpuUploadFrame {
        fn new() -> Self {
            Self {
                buffer: None,
                capacity: 0,
                cursor: 0,
                in_flight: None,
            }
        }

        fn is_available(&self) -> bool {
            self.in_flight.as_ref().is_none_or(|cmd| {
                matches!(
                    cmd.status(),
                    MTLCommandBufferStatus::Completed | MTLCommandBufferStatus::Error
                )
            })
        }
    }

    struct GpuUploadArena {
        frames: Vec<GpuUploadFrame>,
        current: usize,
    }

    impl GpuUploadArena {
        const INITIAL_FRAMES: usize = 3;
        const INITIAL_CAPACITY: usize = 1024 * 1024;
        const ALIGNMENT: usize = 256;

        fn new() -> Self {
            Self {
                frames: (0..Self::INITIAL_FRAMES)
                    .map(|_| GpuUploadFrame::new())
                    .collect(),
                current: 0,
            }
        }

        fn begin_frame(&mut self, stats: &mut RenderStats) {
            let start = (self.current + 1) % self.frames.len();
            let mut selected = None;
            for offset in 0..self.frames.len() {
                let idx = (start + offset) % self.frames.len();
                if self.frames[idx].is_available() {
                    selected = Some(idx);
                    break;
                }
            }

            let idx = if let Some(idx) = selected {
                idx
            } else {
                self.frames.push(GpuUploadFrame::new());
                stats.note_upload_frame_grow();
                self.frames.len() - 1
            };

            self.current = idx;
            let frame = &mut self.frames[self.current];
            frame.cursor = 0;
            frame.in_flight = None;
        }

        fn finish_frame(&mut self, command_buffer: Retained<ProtocolObject<dyn MTLCommandBuffer>>) {
            self.frames[self.current].in_flight = Some(command_buffer);
        }

        fn upload_slice<T>(
            &mut self,
            device: &ProtocolObject<dyn MTLDevice>,
            data: &[T],
            stats: &mut RenderStats,
        ) -> Option<UploadedBufferSlice> {
            let byte_len = std::mem::size_of_val(data);
            if byte_len == 0 {
                return None;
            }
            let frame = &mut self.frames[self.current];
            let offset = align_up(frame.cursor, Self::ALIGNMENT);
            let required = offset.checked_add(byte_len)?;
            if required > frame.capacity {
                let capacity = next_power_of_two_at_least(required.max(Self::INITIAL_CAPACITY));
                let options = MTLResourceOptions::StorageModeShared
                    | MTLResourceOptions::CPUCacheModeWriteCombined;
                frame.buffer = device.newBufferWithLength_options(capacity, options);
                frame.capacity = if frame.buffer.is_some() { capacity } else { 0 };
                stats.note_upload_buffer_allocation(capacity);
            }
            let buffer = frame.buffer.as_ref()?.clone();
            unsafe {
                let dst = buffer.contents().as_ptr().cast::<u8>().add(offset);
                std::ptr::copy_nonoverlapping(data.as_ptr().cast::<u8>(), dst, byte_len);
            }
            frame.cursor = required;
            stats.note_upload_bytes(byte_len);
            Some(UploadedBufferSlice { buffer, offset })
        }

        fn upload_one<T>(
            &mut self,
            device: &ProtocolObject<dyn MTLDevice>,
            value: &T,
            stats: &mut RenderStats,
        ) -> Option<UploadedBufferSlice> {
            self.upload_slice(device, std::slice::from_ref(value), stats)
        }
    }

    fn align_up(value: usize, alignment: usize) -> usize {
        debug_assert!(alignment.is_power_of_two());
        (value + alignment - 1) & !(alignment - 1)
    }

    fn next_power_of_two_at_least(value: usize) -> usize {
        value.checked_next_power_of_two().unwrap_or(value)
    }

    #[derive(Clone, Hash, PartialEq, Eq)]
    struct ProportionalTextLayoutKey {
        text: String,
        size_tenths: u16,
    }

    #[derive(Clone, Copy)]
    struct CachedGlyphPlacement {
        pen_x: f32,
        advance: f32,
        raster_w: usize,
        raster_h: usize,
        uv_min: [f32; 2],
        uv_max: [f32; 2],
    }

    struct CachedProportionalTextLayout {
        text_width_px: f32,
        line_height_px: f32,
        glyphs: Vec<CachedGlyphPlacement>,
        last_used_frame: u64,
    }

    #[derive(Clone, Debug, Hash, PartialEq, Eq)]
    struct ProportionalTextVertexKey {
        text: String,
        size_tenths: u16,
        row_bits: u32,
        col_bits: u32,
        align_width_bits: u32,
        h_align_bits: u32,
        scale_bits: u32,
        fg_bits: [u32; 4],
        mono_cell_w_bits: u32,
        mono_cell_h_bits: u32,
        vp_w_bits: u32,
        vp_h_bits: u32,
    }

    impl ProportionalTextVertexKey {
        fn new(
            run: &widget_render::MetalProportionalTextPrimitive,
            mono_cell_w: f32,
            mono_cell_h: f32,
            vp_w: f32,
            vp_h: f32,
        ) -> Self {
            let fg = run.fg.to_rgba();
            Self {
                text: run.text.clone(),
                size_tenths: (run.font_size * 10.0).round() as u16,
                row_bits: run.row.to_bits(),
                col_bits: run.col.to_bits(),
                align_width_bits: run.align_width.to_bits(),
                h_align_bits: run.h_align.to_bits(),
                scale_bits: run.scale.to_bits(),
                fg_bits: [
                    fg[0].to_bits(),
                    fg[1].to_bits(),
                    fg[2].to_bits(),
                    fg[3].to_bits(),
                ],
                mono_cell_w_bits: mono_cell_w.to_bits(),
                mono_cell_h_bits: mono_cell_h.to_bits(),
                vp_w_bits: vp_w.to_bits(),
                vp_h_bits: vp_h.to_bits(),
            }
        }
    }

    struct CachedProportionalTextVertices {
        vertices: Vec<Vertex>,
        glyph_count: usize,
        quad_count: usize,
        last_used_frame: u64,
    }

    struct ProportionalTextLayoutCache {
        layouts: HashMap<ProportionalTextLayoutKey, CachedProportionalTextLayout>,
        vertex_runs: HashMap<ProportionalTextVertexKey, CachedProportionalTextVertices>,
        frame_index: u64,
    }

    impl ProportionalTextLayoutCache {
        const MAX_ENTRIES: usize = 8192;
        const MAX_UNUSED_FRAMES: u64 = 600;

        fn new() -> Self {
            Self {
                layouts: HashMap::new(),
                vertex_runs: HashMap::new(),
                frame_index: 0,
            }
        }

        fn begin_frame(&mut self) {
            self.frame_index = self.frame_index.wrapping_add(1);
            if self.layouts.len() > Self::MAX_ENTRIES {
                let cutoff = self.frame_index.saturating_sub(Self::MAX_UNUSED_FRAMES);
                self.layouts
                    .retain(|_, layout| layout.last_used_frame >= cutoff);
                if self.layouts.len() > Self::MAX_ENTRIES {
                    self.layouts.clear();
                }
            }
            if self.vertex_runs.len() > Self::MAX_ENTRIES {
                let cutoff = self.frame_index.saturating_sub(Self::MAX_UNUSED_FRAMES);
                self.vertex_runs
                    .retain(|_, run| run.last_used_frame >= cutoff);
                if self.vertex_runs.len() > Self::MAX_ENTRIES {
                    self.vertex_runs.clear();
                }
            }
        }

        fn layout_for_run(
            &mut self,
            run: &widget_render::MetalProportionalTextPrimitive,
            prop_atlas: &mut ProportionalGlyphAtlas,
        ) -> Option<&CachedProportionalTextLayout> {
            let key = ProportionalTextLayoutKey {
                text: run.text.clone(),
                size_tenths: (run.font_size * 10.0).round() as u16,
            };
            if !self.layouts.contains_key(&key) {
                let mut pen_x = 0.0_f32;
                let mut glyphs = Vec::new();
                for ch in key.text.chars() {
                    let Some(entry) = prop_atlas.get_or_rasterize(ch, key.size_tenths) else {
                        continue;
                    };
                    glyphs.push(CachedGlyphPlacement {
                        pen_x,
                        advance: entry.advance,
                        raster_w: entry.raster_w,
                        raster_h: entry.raster_h,
                        uv_min: entry.uv_min,
                        uv_max: entry.uv_max,
                    });
                    pen_x += entry.advance;
                }
                let layout = CachedProportionalTextLayout {
                    text_width_px: pen_x,
                    line_height_px: prop_atlas.line_height(key.size_tenths),
                    glyphs,
                    last_used_frame: self.frame_index,
                };
                self.layouts.insert(key.clone(), layout);
            }

            let frame = self.frame_index;
            let layout = self.layouts.get_mut(&key)?;
            layout.last_used_frame = frame;
            Some(layout)
        }
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum WidgetRunCommandPhase {
        BackgroundInstances,
        MainVertices,
        ForegroundInstances,
        CircleVertices,
        ForegroundRectVertices,
        ProportionalTextVertices,
    }

    #[derive(Clone, Debug, Hash, PartialEq, Eq)]
    struct WidgetRunCacheKey {
        widget_id: u64,
        widget_type: String,
        primitive_signature: u64,
        theme_generation: u64,
        mono_atlas_generation: u64,
        prop_atlas_generation: u64,
        cell_w_bits: u32,
        cell_h_bits: u32,
        vp_w_bits: u32,
        vp_h_bits: u32,
    }

    #[derive(Clone)]
    enum CompiledWidgetRunPipeline {
        MainText,
        ProportionalText,
        Widget(String),
    }

    #[derive(Clone)]
    struct CompiledWidgetRunCommand {
        phase: WidgetRunCommandPhase,
        pipeline: CompiledWidgetRunPipeline,
        buffer: Retained<ProtocolObject<dyn MTLBuffer>>,
        count: usize,
    }

    #[derive(Clone)]
    struct CompiledWidgetRun {
        commands: Vec<CompiledWidgetRunCommand>,
        last_used_frame: u64,
    }

    struct OffsetMetalPrimitiveRun {
        widget_id: u64,
        widget_type: String,
        ancestor_widget_ids: Vec<u64>,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct ImageVertex {
        position: [f32; 2],
        uv: [f32; 2],
        opacity: f32,
        local_pos: [f32; 2],
        half_size: [f32; 2],
        radius: f32,
        rotation: f32,
        clip_circle: f32,
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
        show_selection_start: i32,
        show_selection_end: i32,
        playhead_position: f32,
        show_playhead: i32,
        waveform_color: [f32; 4],
        inactive_waveform_color: [f32; 4],
        marker_color: [f32; 4],
        active_marker_color: [f32; 4],
        active_selection_start: i32,
        active_selection_end: i32,
        selection_color: [f32; 4],
        bg_color: [f32; 4],
        border_color: [f32; 4],
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct PatchCableInstance {
        ndc_min: [f32; 2],
        ndc_max: [f32; 2],
        bounds_min: [f32; 2],
        bounds_max: [f32; 2],
        start: [f32; 2],
        control1: [f32; 2],
        control2: [f32; 2],
        end: [f32; 2],
        color: [f32; 4],
        radius_px: f32,
        is_segmented: f32,
        segment_y_px: f32,
        corner_radius_px: f32,
    }

    #[derive(Clone, Copy)]
    struct PatchCableDrawInstance {
        instance: PatchCableInstance,
        clip: MTLScissorRect,
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
        col: f32,
        row: f32,
    }

    #[derive(Clone)]
    struct CachedWidgetScene {
        primitives: Vec<widget_render::MetalPrimitive>,
    }

    #[derive(Clone)]
    struct CachedWidgetRunScene {
        runs: Vec<widget_render::MetalPrimitiveRun>,
        run_indices: HashMap<widget_render::MetalPrimitiveRunKey, usize>,
    }

    struct ImageTextureResource {
        texture: Retained<ProtocolObject<dyn MTLTexture>>,
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
        bgra: Vec<u8>,
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
        let mut bgra = rgba.into_raw();
        for px in bgra.chunks_exact_mut(4) {
            px.swap(0, 2);
        }
        Some(DecodedImageData {
            width,
            height,
            bgra,
        })
    }

    #[derive(Clone, Copy, Hash, PartialEq, Eq)]
    struct WidgetSceneCacheKey {
        owner_frame_key: u64,
        layout_identity: usize,
        layout_cache_key: u64,
        widget_state_generation: u64,
        theme_generation: u64,
        focused_widget_id: Option<u64>,
        scroll_top_bits: u32,
        max_rows: u16,
        cell_w_bits: u32,
        cell_h_bits: u32,
        vp_w_bits: u32,
        vp_h_bits: u32,
        tile_content_rows_bits: u32,
    }

    const AGENT_INSTRUMENT_STUB_ANIMATION_WIDGET: &str = "agent-instrument-stub-bg";
    const AGENT_INSTRUMENT_STUB_ANIMATION_WIDGET_SUFFIX: &str = "__agent-instrument-stub-bg";
    const AGENT_INSTRUMENT_STUB_ANIMATION_WIDGET_SAFE_SUFFIX: &str = "__agent_instrument_stub_bg";
    const AGENT_INSTRUMENT_STUB_SKELETON_DEBUG_NAME: &str = "agent-instrument-stub-skeleton";
    const DEFAULT_MONOSPACE_FONT_SIZE_PT: f64 = 16.0;

    fn simple_widget_run_cacheable(widget_type: &str) -> bool {
        matches!(
            widget_type,
            "label"
                | "button"
                | "badge"
                | "slider"
                | "hslider"
                | "vslider"
                | "toggle"
                | "knob"
                | "tabs"
                | "box"
                | "number-label"
        )
    }

    fn primitive_run_supported_for_cache(primitives: &[widget_render::MetalPrimitive]) -> bool {
        !primitives.is_empty()
            && primitives.iter().all(|primitive| {
                matches!(
                    widget_render::innermost_primitive(primitive),
                    widget_render::MetalPrimitive::Rect(_)
                        | widget_render::MetalPrimitive::ForegroundRect(_)
                        | widget_render::MetalPrimitive::Quad(_)
                        | widget_render::MetalPrimitive::Triangle(_)
                        | widget_render::MetalPrimitive::GlyphRun(_)
                        | widget_render::MetalPrimitive::ProportionalText(_)
                        | widget_render::MetalPrimitive::Circle(_)
                        | widget_render::MetalPrimitive::WidgetInstance { .. }
                )
            })
    }

    fn widget_run_or_ancestor_dirty(
        run: &OffsetMetalPrimitiveRun,
        dirty_widget_ids: &[u64],
    ) -> bool {
        dirty_widget_ids.contains(&run.widget_id)
            || run
                .ancestor_widget_ids
                .iter()
                .any(|widget_id| dirty_widget_ids.contains(widget_id))
    }

    fn hash_f32(value: f32, hasher: &mut DefaultHasher) {
        value.to_bits().hash(hasher);
    }

    fn hash_color(color: Color, hasher: &mut DefaultHasher) {
        hash_f32(color.r, hasher);
        hash_f32(color.g, hasher);
        hash_f32(color.b, hasher);
        hash_f32(color.a, hasher);
    }

    fn hash_rect(rect: Rect, hasher: &mut DefaultHasher) {
        hash_f32(rect.row, hasher);
        hash_f32(rect.col, hasher);
        hash_f32(rect.width, hasher);
        hash_f32(rect.height, hasher);
    }

    fn hash_f32_array<const N: usize>(values: [f32; N], hasher: &mut DefaultHasher) {
        for value in values {
            hash_f32(value, hasher);
        }
    }

    fn hash_widget_instance(
        widget_type: &str,
        instance: &WidgetInstance,
        hasher: &mut DefaultHasher,
    ) {
        hash_f32_array(instance.ndc_min, hasher);
        hash_f32_array(instance.ndc_max, hasher);
        hash_f32(instance.value_t, hasher);
        hash_f32(instance.orientation, hasher);
        if widget_instance_shader_uses_time(widget_type) {
            hash_f32(instance.itime, hasher);
        }
        hash_f32_array(instance.uniform_a, hasher);
        hash_f32_array(instance.uniform_b, hasher);
        hash_f32_array(instance.color_a, hasher);
        hash_f32_array(instance.color_b, hasher);
        hash_f32_array(instance.color_c, hasher);
        hash_f32_array(instance.color_d, hasher);
        hash_f32(instance.corner_radius, hasher);
        hash_f32(instance.pixel_aspect, hasher);
    }

    fn widget_instance_shader_uses_time(widget_type: &str) -> bool {
        widget_render::sdf_widget::sdf_widget_def(widget_type)
            .is_some_and(|definition| definition.animates)
    }

    fn hash_metal_primitive(primitive: &widget_render::MetalPrimitive, hasher: &mut DefaultHasher) {
        match primitive {
            widget_render::MetalPrimitive::ZLayer { z_index, primitive } => {
                0u8.hash(hasher);
                z_index.hash(hasher);
                hash_metal_primitive(primitive, hasher);
            }
            widget_render::MetalPrimitive::Rect(rect) => {
                1u8.hash(hasher);
                hash_rect(rect.rect, hasher);
                hash_color(rect.color, hasher);
            }
            widget_render::MetalPrimitive::ForegroundRect(rect) => {
                2u8.hash(hasher);
                hash_rect(rect.rect, hasher);
                hash_color(rect.color, hasher);
            }
            widget_render::MetalPrimitive::Quad(quad) => {
                3u8.hash(hasher);
                hash_f32(quad.x, hasher);
                hash_f32(quad.y, hasher);
                hash_f32(quad.width, hasher);
                hash_f32(quad.height, hasher);
                hash_color(quad.color, hasher);
            }
            widget_render::MetalPrimitive::Triangle(triangle) => {
                4u8.hash(hasher);
                for point in triangle.points {
                    hash_f32_array(point, hasher);
                }
                hash_color(triangle.color, hasher);
            }
            widget_render::MetalPrimitive::GlyphRun(run) => {
                5u8.hash(hasher);
                hash_f32(run.row, hasher);
                run.col.hash(hasher);
                run.text.hash(hasher);
                hash_color(run.fg, hasher);
                hash_color(run.bg, hasher);
            }
            widget_render::MetalPrimitive::ProportionalText(run) => {
                6u8.hash(hasher);
                hash_f32(run.row, hasher);
                hash_f32(run.col, hasher);
                hash_f32(run.align_width, hasher);
                hash_f32(run.h_align, hasher);
                run.text.hash(hasher);
                hash_f32(run.font_size, hasher);
                hash_f32(run.scale, hasher);
                hash_color(run.fg, hasher);
                hash_color(run.bg, hasher);
            }
            widget_render::MetalPrimitive::Circle(circle) => {
                7u8.hash(hasher);
                hash_f32_array(circle.center, hasher);
                hash_f32(circle.radius_px, hasher);
                hash_color(circle.color, hasher);
                std::mem::discriminant(&circle.visible_half).hash(hasher);
            }
            widget_render::MetalPrimitive::WidgetInstance {
                widget_type,
                instance,
                is_background,
            } => {
                8u8.hash(hasher);
                widget_type.hash(hasher);
                is_background.hash(hasher);
                hash_widget_instance(widget_type, instance, hasher);
            }
            widget_render::MetalPrimitive::PatchCable(_)
            | widget_render::MetalPrimitive::Waveform(_)
            | widget_render::MetalPrimitive::Image(_)
            | widget_render::MetalPrimitive::PushClipRect(_)
            | widget_render::MetalPrimitive::PopClipRect => {
                255u8.hash(hasher);
            }
        }
    }

    fn widget_run_cache_key(
        widget_id: u64,
        widget_type: &str,
        primitives: &[widget_render::MetalPrimitive],
        cell_w: f32,
        cell_h: f32,
        vp_w: f32,
        vp_h: f32,
        mono_atlas_generation: u64,
        prop_atlas_generation: u64,
    ) -> WidgetRunCacheKey {
        let mut hasher = DefaultHasher::new();
        primitives.len().hash(&mut hasher);
        for primitive in primitives {
            hash_metal_primitive(primitive, &mut hasher);
        }
        WidgetRunCacheKey {
            widget_id,
            widget_type: widget_type.to_string(),
            primitive_signature: hasher.finish(),
            theme_generation: theme::generation(),
            mono_atlas_generation,
            prop_atlas_generation,
            cell_w_bits: cell_w.to_bits(),
            cell_h_bits: cell_h.to_bits(),
            vp_w_bits: vp_w.to_bits(),
            vp_h_bits: vp_h.to_bits(),
        }
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
        sdf_widget_pipeline_registry_generation: u64,
        waveform_pipeline: Option<Retained<ProtocolObject<dyn MTLRenderPipelineState>>>,
        image_pipeline: Option<Retained<ProtocolObject<dyn MTLRenderPipelineState>>>,
        patch_cable_pipeline: Option<Retained<ProtocolObject<dyn MTLRenderPipelineState>>>,
        waveform_buffers: HashMap<(String, u32), WaveformGpuResource>,
        image_textures: HashMap<PathBuf, ImageTextureResource>,
        image_decode_tx: mpsc::Sender<ImageDecodeJob>,
        image_decode_rx: mpsc::Receiver<ImageDecodeResult>,
        image_decode_in_flight: HashSet<PathBuf>,
        pending_image_loads: bool,
        image_load_suspended_until: Option<Instant>,
        image_last_decode_at: Option<Instant>,
        image_decode_min_interval: Duration,
        // Glyph atlases
        atlas: Option<GlyphAtlas>,
        prop_atlas: Option<ProportionalGlyphAtlas>,
        cached_text_key: Option<u64>,
        cached_text_quads: Vec<Vertex>,
        cached_text_buffer: Option<Retained<ProtocolObject<dyn MTLBuffer>>>,
        cached_text_vertex_count: usize,
        upload_arena: GpuUploadArena,
        prop_text_layout_cache: ProportionalTextLayoutCache,
        mono_atlas_generation: u64,
        prop_atlas_generation: u64,
        compiled_widget_runs: HashMap<WidgetRunCacheKey, CompiledWidgetRun>,
        compiled_widget_run_frame: u64,
        cached_widget_scenes: HashMap<u64, CachedWidgetScene>,
        cached_widget_run_scenes: HashMap<u64, CachedWidgetRunScene>,
        widget_scene_last_keys: HashMap<usize, WidgetSceneCacheKey>,
        image_rotation_states: HashMap<u64, ImageRotationState>,
        stats: RenderStats,
        agent_instrument_stub_animation_visible: bool,
        // Winit
        event_loop: Option<EventLoop<()>>,
        window: Option<Window>,
        pending: VecDeque<Event>,
        pending_drag: Option<Event>,
        pending_move: Option<Event>,
        pending_magnify: VecDeque<(f64, (f32, f32))>,
        pending_scroll: VecDeque<((f32, f32), (f32, f32))>,
        suppress_scroll_until: Option<Instant>,
        modifiers: KeyModifiers,
        pressed_mouse_button: Option<MouseButton>,
        cursor_cell: (u16, u16),
        cursor_pos: (f32, f32),
        last_precise_mouse: Option<(f32, f32)>,
        last_window_bg: Option<Color>,
        start_time: Instant,
        initial_window_size: LogicalSize<f64>,
        initial_window_visible: bool,
        monospace_font_size_pt: f64,
    }

    impl MetalBackend {
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
            Self::new_with_size_font_size_and_visibility(
                width,
                height,
                monospace_font_size_pt,
                true,
            )
        }

        pub fn new_capture(width: u32, height: u32) -> Result<Self, BackendError> {
            Self::new_with_size_font_size_and_visibility(
                width,
                height,
                DEFAULT_MONOSPACE_FONT_SIZE_PT,
                false,
            )
        }

        fn new_with_size_font_size_and_visibility(
            width: u32,
            height: u32,
            monospace_font_size_pt: f64,
            visible: bool,
        ) -> Result<Self, BackendError> {
            if !monospace_font_size_pt.is_finite() || monospace_font_size_pt <= 0.0 {
                return Err(BackendError::MetalError);
            }
            let device = MTLCreateSystemDefaultDevice().ok_or(BackendError::MetalError)?;
            let command_queue = device.newCommandQueue().ok_or(BackendError::MetalError)?;
            let layer = CAMetalLayer::new();
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
                sdf_widget_pipeline_registry_generation: 0,
                waveform_pipeline: None,
                image_pipeline: None,
                patch_cable_pipeline: None,
                waveform_buffers: HashMap::new(),
                image_textures: HashMap::new(),
                image_decode_tx,
                image_decode_rx,
                image_decode_in_flight: HashSet::new(),
                pending_image_loads: false,
                image_load_suspended_until: None,
                image_last_decode_at: None,
                image_decode_min_interval: Duration::ZERO,
                atlas: None,
                prop_atlas: None,
                cached_text_key: None,
                cached_text_quads: Vec::new(),
                cached_text_buffer: None,
                cached_text_vertex_count: 0,
                upload_arena: GpuUploadArena::new(),
                prop_text_layout_cache: ProportionalTextLayoutCache::new(),
                mono_atlas_generation: 0,
                prop_atlas_generation: 0,
                compiled_widget_runs: HashMap::new(),
                compiled_widget_run_frame: 0,
                cached_widget_scenes: HashMap::new(),
                cached_widget_run_scenes: HashMap::new(),
                widget_scene_last_keys: HashMap::new(),
                image_rotation_states: HashMap::new(),
                stats: RenderStats::new(),
                agent_instrument_stub_animation_visible: false,
                event_loop: None,
                window: None,
                pending: VecDeque::new(),
                pending_drag: None,
                pending_move: None,
                pending_magnify: VecDeque::new(),
                pending_scroll: VecDeque::new(),
                suppress_scroll_until: None,
                modifiers: KeyModifiers::NONE,
                pressed_mouse_button: None,
                cursor_cell: (0, 0),
                cursor_pos: (0.0, 0.0),
                last_precise_mouse: None,
                last_window_bg: None,
                start_time: Instant::now(),
                initial_window_size: LogicalSize::new(width as f64, height as f64),
                initial_window_visible: visible,
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

        fn note_agent_instrument_stub_animation_detected(&mut self) {
            self.agent_instrument_stub_animation_visible = true;
        }

        fn widget_scene_cache_parts(
            &self,
            owner_frame_key: u64,
            layout: &crate::layout::LayoutNode,
            layout_cache_key: u64,
            viewport: WidgetViewport,
            scroll_top: f32,
            max_rows: u16,
        ) -> WidgetSceneCacheKey {
            WidgetSceneCacheKey {
                owner_frame_key,
                layout_identity: layout as *const crate::layout::LayoutNode as usize,
                layout_cache_key,
                widget_state_generation: widget_render::widget_state_generation(),
                theme_generation: theme::generation(),
                focused_widget_id: viewport.focused_widget_id,
                scroll_top_bits: scroll_top.to_bits(),
                max_rows,
                cell_w_bits: viewport.cell_w.to_bits(),
                cell_h_bits: viewport.cell_h.to_bits(),
                vp_w_bits: viewport.vp_w.to_bits(),
                vp_h_bits: viewport.vp_h.to_bits(),
                tile_content_rows_bits: viewport.tile_content_rows.to_bits(),
            }
        }

        fn widget_scene_cache_key(&self, key: WidgetSceneCacheKey) -> u64 {
            let mut hasher = DefaultHasher::new();
            key.hash(&mut hasher);
            hasher.finish()
        }

        fn refresh_widget_scene_time(
            primitives: &mut [widget_render::MetalPrimitive],
            time_seconds: f32,
        ) {
            for primitive in primitives {
                if let widget_render::MetalPrimitive::ZLayer { primitive, .. } = primitive {
                    Self::refresh_widget_scene_time(std::slice::from_mut(primitive), time_seconds);
                    continue;
                }
                if let widget_render::MetalPrimitive::WidgetInstance {
                    widget_type,
                    instance,
                    ..
                } = primitive
                {
                    instance.itime = time_seconds;
                    if instance.color_d[3] < 0.0 {
                        let duration = -instance.color_d[3];
                        let scale = if duration <= 0.0001 {
                            instance.color_d[1]
                        } else {
                            let t =
                                ((time_seconds - instance.color_d[2]) / duration).clamp(0.0, 1.0);
                            let ease = instance.color_b[2];
                            let eased = if (ease - 2.0).abs() < 0.5 {
                                t * t * (3.0 - 2.0 * t)
                            } else if (ease - 3.0).abs() < 0.5 {
                                t
                            } else {
                                1.0 - (1.0 - t).powi(3)
                            };
                            instance.color_d[0]
                                + (instance.color_d[1] - instance.color_d[0]) * eased
                        };
                        if widget_type == "save-icon" && duration > 0.0001 {
                            let elapsed = time_seconds - instance.color_d[2];
                            let phase = if elapsed < 0.0 {
                                "before"
                            } else if elapsed < duration {
                                "active"
                            } else {
                                "done"
                            };
                            eprintln!(
                                "[anim-draw] t={:.3} widget={} phase={} elapsed={:.3}/{:.3} scale={:.3}",
                                time_seconds, widget_type, phase, elapsed, duration, scale
                            );
                        }
                        let center = [
                            (instance.ndc_min[0] + instance.ndc_max[0]) * 0.5,
                            (instance.ndc_min[1] + instance.ndc_max[1]) * 0.5,
                        ];
                        instance.ndc_min[0] = center[0] + (instance.ndc_min[0] - center[0]) * scale;
                        instance.ndc_max[0] = center[0] + (instance.ndc_max[0] - center[0]) * scale;
                        instance.ndc_min[1] = center[1] + (instance.ndc_min[1] - center[1]) * scale;
                        instance.ndc_max[1] = center[1] + (instance.ndc_max[1] - center[1]) * scale;
                        instance.color_d = [0.0; 4];
                    }
                }
            }
        }

        fn widget_scene_for_layout(
            &mut self,
            owner_frame_key: u64,
            layout_cache_key: u64,
            layout: &crate::layout::LayoutNode,
            dirty_widget_ids: &[u64],
            viewport: WidgetViewport,
            scroll_top: f32,
            max_rows: u16,
        ) -> Vec<widget_render::MetalPrimitive> {
            if widget_render::overlay_widget_id().is_some() {
                self.stats.note_widget_scene_overlay_bypass();
                let (mut primitives, _overlay) =
                    widget_render::collect_metal_primitives(layout, viewport, scroll_top, max_rows);
                Self::refresh_widget_scene_time(&mut primitives, viewport.time_seconds);
                self.stats.note_widget_primitives(primitives.len());
                return primitives;
            }
            if widget_render::layout_wants_animation_frames(layout) {
                let (mut primitives, _overlay) =
                    widget_render::collect_metal_primitives(layout, viewport, scroll_top, max_rows);
                Self::refresh_widget_scene_time(&mut primitives, viewport.time_seconds);
                self.stats.note_widget_primitives(primitives.len());
                return primitives;
            }

            let cache_parts = self.widget_scene_cache_parts(
                owner_frame_key,
                layout,
                layout_cache_key,
                viewport,
                scroll_top,
                max_rows,
            );
            let layout_identity = cache_parts.layout_identity;
            let cache_key = self.widget_scene_cache_key(cache_parts);
            if dirty_widget_ids.is_empty()
                && let Some(scene) = self.cached_widget_scenes.get(&cache_key)
            {
                self.stats.note_widget_scene_cache_hit();
                let mut primitives = scene.primitives.clone();
                Self::refresh_widget_scene_time(&mut primitives, viewport.time_seconds);
                self.stats.note_widget_primitives(primitives.len());
                return primitives;
            }
            if dirty_widget_ids.is_empty()
                && let Some(scene) = self.cached_widget_run_scenes.get(&cache_key)
            {
                self.stats.note_widget_scene_cache_hit();
                let mut primitives = widget_render::flatten_metal_primitive_runs(&scene.runs);
                if self.cached_widget_scenes.len() >= 128 {
                    self.cached_widget_scenes.clear();
                    self.cached_widget_run_scenes.clear();
                    self.widget_scene_last_keys.clear();
                    self.stats.note_widget_scene_cache_clear();
                } else {
                    self.cached_widget_scenes.insert(
                        cache_key,
                        CachedWidgetScene {
                            primitives: primitives.clone(),
                        },
                    );
                }
                Self::refresh_widget_scene_time(&mut primitives, viewport.time_seconds);
                self.stats.note_widget_primitives(primitives.len());
                return primitives;
            }

            if dirty_widget_ids.is_empty() {
                let previous = self
                    .widget_scene_last_keys
                    .insert(layout_identity, cache_parts);
                self.stats
                    .note_widget_scene_cache_miss(previous, cache_parts);
            } else {
                self.widget_scene_last_keys
                    .insert(layout_identity, cache_parts);
                self.stats
                    .note_widget_scene_dirty_bypass(dirty_widget_ids.len());
            }

            let (runs, _overlay) =
                widget_render::collect_metal_primitive_runs(layout, viewport, scroll_top, max_rows);
            let mut primitives = widget_render::flatten_metal_primitive_runs(&runs);
            if self.cached_widget_scenes.len() >= 128 {
                self.cached_widget_scenes.clear();
                self.cached_widget_run_scenes.clear();
                self.widget_scene_last_keys.clear();
                self.stats.note_widget_scene_cache_clear();
            }
            self.cached_widget_scenes.insert(
                cache_key,
                CachedWidgetScene {
                    primitives: primitives.clone(),
                },
            );
            let run_indices = widget_render::build_metal_primitive_run_index(&runs);
            self.cached_widget_run_scenes
                .insert(cache_key, CachedWidgetRunScene { runs, run_indices });
            Self::refresh_widget_scene_time(&mut primitives, viewport.time_seconds);
            self.stats.note_widget_primitives(primitives.len());
            primitives
        }

        fn update_widget_scene_cache_from_primitive_runs(
            &mut self,
            owner_frame_key: u64,
            layout_cache_key: u64,
            layout: &crate::layout::LayoutNode,
            viewport: WidgetViewport,
            scroll_top: f32,
            max_rows: u16,
            runs: &[widget_render::MetalPrimitiveRun],
        ) {
            if widget_render::overlay_widget_id().is_some()
                || widget_render::layout_wants_animation_frames(layout)
            {
                return;
            }
            let cache_parts = self.widget_scene_cache_parts(
                owner_frame_key,
                layout,
                layout_cache_key,
                viewport,
                scroll_top,
                max_rows,
            );
            let layout_identity = cache_parts.layout_identity;
            let cache_key = self.widget_scene_cache_key(cache_parts);
            self.widget_scene_last_keys
                .insert(layout_identity, cache_parts);
            if self.cached_widget_scenes.len() >= 128 {
                self.cached_widget_scenes.clear();
                self.cached_widget_run_scenes.clear();
                self.widget_scene_last_keys.clear();
                self.stats.note_widget_scene_cache_clear();
            }
            let primitives = widget_render::flatten_metal_primitive_runs(runs);
            self.cached_widget_scenes
                .insert(cache_key, CachedWidgetScene { primitives });
            let run_indices = widget_render::build_metal_primitive_run_index(runs);
            self.cached_widget_run_scenes.insert(
                cache_key,
                CachedWidgetRunScene {
                    runs: runs.to_vec(),
                    run_indices,
                },
            );
        }

        fn refresh_widget_run_scene_for_dirty_layout(
            &mut self,
            owner_frame_key: u64,
            layout_cache_key: u64,
            layout: &crate::layout::LayoutNode,
            dirty_widget_ids: &[u64],
            viewport: WidgetViewport,
            scroll_top: f32,
            max_rows: u16,
        ) -> (u64, Vec<widget_render::MetalPrimitive>) {
            let cache_parts = self.widget_scene_cache_parts(
                owner_frame_key,
                layout,
                layout_cache_key,
                viewport,
                scroll_top,
                max_rows,
            );
            self.widget_scene_last_keys
                .insert(cache_parts.layout_identity, cache_parts);
            self.stats
                .note_widget_scene_dirty_bypass(dirty_widget_ids.len());

            let cache_key = self.widget_scene_cache_key(cache_parts);
            self.cached_widget_scenes.remove(&cache_key);
            if let Some(scene) = self.cached_widget_run_scenes.get_mut(&cache_key) {
                let (overlay, retained_stats) =
                    widget_render::refresh_metal_primitive_runs_retained_in_place(
                        layout,
                        viewport,
                        scroll_top,
                        max_rows,
                        &mut scene.runs,
                        &scene.run_indices,
                        dirty_widget_ids,
                    );
                let should_rebuild_full = retained_stats.missing_previous_runs > 0
                    || retained_stats.invalid_previous_runs > 0;
                self.stats.note_widget_retained_run_collection(
                    retained_stats.reused_runs,
                    retained_stats.rebuilt_runs,
                    retained_stats.missing_previous_runs,
                    retained_stats.invalid_previous_runs,
                );
                if should_rebuild_full {
                    let (runs, overlay) = widget_render::collect_metal_primitive_runs(
                        layout, viewport, scroll_top, max_rows,
                    );
                    scene.run_indices = widget_render::build_metal_primitive_run_index(&runs);
                    scene.runs = runs;
                    return (cache_key, overlay);
                }
                return (cache_key, overlay);
            }

            self.stats.note_widget_retained_run_collection_miss();
            let (runs, overlay) =
                widget_render::collect_metal_primitive_runs(layout, viewport, scroll_top, max_rows);
            let run_indices = widget_render::build_metal_primitive_run_index(&runs);
            if self.cached_widget_run_scenes.len() >= 128 {
                self.cached_widget_scenes.clear();
                self.cached_widget_run_scenes.clear();
                self.widget_scene_last_keys.clear();
                self.stats.note_widget_scene_cache_clear();
            }
            self.cached_widget_run_scenes
                .insert(cache_key, CachedWidgetRunScene { runs, run_indices });
            (cache_key, overlay)
        }

        fn begin_compiled_widget_run_frame(&mut self) {
            self.compiled_widget_run_frame = self.compiled_widget_run_frame.wrapping_add(1);
            if self.compiled_widget_runs.len() > 8192 {
                let cutoff = self.compiled_widget_run_frame.saturating_sub(600);
                self.compiled_widget_runs
                    .retain(|_, run| run.last_used_frame >= cutoff);
                if self.compiled_widget_runs.len() > 8192 {
                    self.compiled_widget_runs.clear();
                    self.stats.note_widget_run_cache_clear();
                }
            }
        }

        fn new_static_buffer<T>(
            &mut self,
            data: &[T],
        ) -> Option<Retained<ProtocolObject<dyn MTLBuffer>>> {
            let byte_len = std::mem::size_of_val(data);
            if byte_len == 0 {
                return None;
            }
            let buffer = unsafe {
                self.device.newBufferWithBytes_length_options(
                    NonNull::new(data.as_ptr() as *mut _)?,
                    byte_len,
                    MTLResourceOptions::StorageModeShared,
                )
            };
            if buffer.is_some() {
                self.stats.note_widget_run_static_allocation(byte_len);
            }
            buffer
        }

        fn compile_simple_widget_run(
            &mut self,
            primitives: &[widget_render::MetalPrimitive],
            cell_w: f32,
            cell_h: f32,
            vp_w: f32,
            vp_h: f32,
        ) -> Option<CompiledWidgetRun> {
            if !primitive_run_supported_for_cache(primitives) {
                return None;
            }

            let mut commands = Vec::new();
            let (bg_runs, fg_runs) = partition_widget_instance_runs(primitives);
            for (widget_type, instances) in bg_runs {
                let buffer = self.new_static_buffer(instances.as_slice())?;
                commands.push(CompiledWidgetRunCommand {
                    phase: WidgetRunCommandPhase::BackgroundInstances,
                    pipeline: CompiledWidgetRunPipeline::Widget(widget_type),
                    buffer,
                    count: instances.len(),
                });
            }

            let main_vertices = {
                let atlas = self.atlas.as_mut()?;
                build_widget_primitive_quads(primitives, atlas, vp_w, vp_h)
            };
            if !main_vertices.is_empty() {
                let buffer = self.new_static_buffer(main_vertices.as_slice())?;
                commands.push(CompiledWidgetRunCommand {
                    phase: WidgetRunCommandPhase::MainVertices,
                    pipeline: CompiledWidgetRunPipeline::MainText,
                    buffer,
                    count: main_vertices.len(),
                });
            }

            for (widget_type, instances) in fg_runs {
                let buffer = self.new_static_buffer(instances.as_slice())?;
                commands.push(CompiledWidgetRunCommand {
                    phase: WidgetRunCommandPhase::ForegroundInstances,
                    pipeline: CompiledWidgetRunPipeline::Widget(widget_type),
                    buffer,
                    count: instances.len(),
                });
            }

            let circle_vertices = build_circle_quads(primitives, cell_w, cell_h, vp_w, vp_h);
            if !circle_vertices.is_empty() {
                let buffer = self.new_static_buffer(circle_vertices.as_slice())?;
                commands.push(CompiledWidgetRunCommand {
                    phase: WidgetRunCommandPhase::CircleVertices,
                    pipeline: CompiledWidgetRunPipeline::MainText,
                    buffer,
                    count: circle_vertices.len(),
                });
            }

            let foreground_rect_vertices =
                build_foreground_rect_quads(primitives, cell_w, cell_h, vp_w, vp_h);
            if !foreground_rect_vertices.is_empty() {
                let buffer = self.new_static_buffer(foreground_rect_vertices.as_slice())?;
                commands.push(CompiledWidgetRunCommand {
                    phase: WidgetRunCommandPhase::ForegroundRectVertices,
                    pipeline: CompiledWidgetRunPipeline::MainText,
                    buffer,
                    count: foreground_rect_vertices.len(),
                });
            }

            if self.prop_pipeline.is_some() {
                let prop_vertices = if let Some(prop_atlas) = self.prop_atlas.as_mut() {
                    build_proportional_text_quads_cached(
                        primitives,
                        prop_atlas,
                        &mut self.prop_text_layout_cache,
                        &mut self.stats,
                        cell_w,
                        cell_h,
                        vp_w,
                        vp_h,
                    )
                } else {
                    Vec::new()
                };
                if !prop_vertices.is_empty() {
                    let buffer = self.new_static_buffer(prop_vertices.as_slice())?;
                    commands.push(CompiledWidgetRunCommand {
                        phase: WidgetRunCommandPhase::ProportionalTextVertices,
                        pipeline: CompiledWidgetRunPipeline::ProportionalText,
                        buffer,
                        count: prop_vertices.len(),
                    });
                }
            }

            if commands.is_empty() {
                return None;
            }

            Some(CompiledWidgetRun {
                commands,
                last_used_frame: self.compiled_widget_run_frame,
            })
        }

        fn compiled_simple_widget_run(
            &mut self,
            widget_id: u64,
            widget_type: &str,
            primitives: &[widget_render::MetalPrimitive],
            dirty_widget_ids: &[u64],
            cell_w: f32,
            cell_h: f32,
            vp_w: f32,
            vp_h: f32,
        ) -> Option<CompiledWidgetRun> {
            if !simple_widget_run_cacheable(widget_type) {
                self.stats.note_widget_run_cache_bypass_unsupported();
                return None;
            }
            if dirty_widget_ids.contains(&widget_id) {
                self.stats.note_widget_run_cache_bypass_dirty();
                return None;
            }
            if !primitive_run_supported_for_cache(primitives) {
                self.stats.note_widget_run_cache_bypass_complex();
                return None;
            }

            let key = widget_run_cache_key(
                widget_id,
                widget_type,
                primitives,
                cell_w,
                cell_h,
                vp_w,
                vp_h,
                self.mono_atlas_generation,
                self.prop_atlas_generation,
            );
            if let Some(compiled) = self.compiled_widget_runs.get_mut(&key) {
                compiled.last_used_frame = self.compiled_widget_run_frame;
                self.stats.note_widget_run_cache_hit();
                return Some(compiled.clone());
            }

            self.stats.note_widget_run_cache_miss();
            let compiled =
                self.compile_simple_widget_run(primitives, cell_w, cell_h, vp_w, vp_h)?;
            self.compiled_widget_runs.insert(key, compiled.clone());
            Some(compiled)
        }

        fn draw_compiled_widget_run_phase(
            &mut self,
            enc: &ProtocolObject<dyn MTLRenderCommandEncoder>,
            compiled: &CompiledWidgetRun,
            phase: WidgetRunCommandPhase,
            atlas_texture: &ProtocolObject<dyn MTLTexture>,
            prop_atlas_texture: Option<&ProtocolObject<dyn MTLTexture>>,
        ) {
            for command in compiled
                .commands
                .iter()
                .filter(|command| command.phase == phase)
            {
                match &command.pipeline {
                    CompiledWidgetRunPipeline::MainText => {
                        let Some(pipeline) = self.pipeline.as_ref() else {
                            continue;
                        };
                        enc.setRenderPipelineState(pipeline);
                        unsafe {
                            enc.setVertexBuffer_offset_atIndex(Some(&command.buffer), 0, 0);
                            enc.setFragmentTexture_atIndex(Some(atlas_texture), 0);
                            enc.drawPrimitives_vertexStart_vertexCount(
                                MTLPrimitiveType::Triangle,
                                0,
                                command.count as _,
                            );
                        }
                        self.stats.note_draw_command();
                    }
                    CompiledWidgetRunPipeline::ProportionalText => {
                        let (Some(pipeline), Some(texture)) =
                            (self.prop_pipeline.as_ref(), prop_atlas_texture)
                        else {
                            continue;
                        };
                        enc.setRenderPipelineState(pipeline);
                        unsafe {
                            enc.setVertexBuffer_offset_atIndex(Some(&command.buffer), 0, 0);
                            enc.setFragmentTexture_atIndex(Some(texture), 0);
                            enc.drawPrimitives_vertexStart_vertexCount(
                                MTLPrimitiveType::Triangle,
                                0,
                                command.count as _,
                            );
                        }
                        self.stats.note_draw_command();
                    }
                    CompiledWidgetRunPipeline::Widget(widget_type) => {
                        let Some(pipeline) = self.widget_pipelines.get(widget_type) else {
                            continue;
                        };
                        enc.setRenderPipelineState(pipeline);
                        unsafe {
                            enc.setVertexBuffer_offset_atIndex(Some(&command.buffer), 0, 0);
                            enc.drawPrimitives_vertexStart_vertexCount_instanceCount(
                                MTLPrimitiveType::Triangle,
                                0,
                                6,
                                command.count as _,
                            );
                        }
                        self.stats.note_draw_command();
                    }
                }
            }
        }

        fn draw_dynamic_widget_run_phase(
            &mut self,
            enc: &ProtocolObject<dyn MTLRenderCommandEncoder>,
            primitives: &[widget_render::MetalPrimitive],
            phase: WidgetRunCommandPhase,
            atlas_texture: &ProtocolObject<dyn MTLTexture>,
            prop_atlas_texture: Option<&ProtocolObject<dyn MTLTexture>>,
            cell_w: f32,
            cell_h: f32,
            vp_w: f32,
            vp_h: f32,
        ) {
            match phase {
                WidgetRunCommandPhase::BackgroundInstances => {
                    let (bg_runs, _) = partition_widget_instance_runs(primitives);
                    for (widget_type, instances) in bg_runs {
                        let Some(wpipe) = self.widget_pipelines.get(&widget_type) else {
                            continue;
                        };
                        draw_widget_instances(
                            enc,
                            &self.device,
                            &mut self.upload_arena,
                            &mut self.stats,
                            wpipe,
                            instances.as_slice(),
                        );
                    }
                }
                WidgetRunCommandPhase::MainVertices => {
                    let Some(atlas) = self.atlas.as_mut() else {
                        return;
                    };
                    let vertices = build_widget_primitive_quads(primitives, atlas, vp_w, vp_h);
                    let Some(pipeline) = self.pipeline.as_ref() else {
                        return;
                    };
                    draw_vertices(
                        enc,
                        &self.device,
                        &mut self.upload_arena,
                        &mut self.stats,
                        pipeline,
                        atlas_texture,
                        vertices.as_slice(),
                    );
                }
                WidgetRunCommandPhase::ForegroundInstances => {
                    let (_, fg_runs) = partition_widget_instance_runs(primitives);
                    for (widget_type, instances) in fg_runs {
                        let Some(wpipe) = self.widget_pipelines.get(&widget_type) else {
                            continue;
                        };
                        draw_widget_instances(
                            enc,
                            &self.device,
                            &mut self.upload_arena,
                            &mut self.stats,
                            wpipe,
                            instances.as_slice(),
                        );
                    }
                }
                WidgetRunCommandPhase::CircleVertices => {
                    let vertices = build_circle_quads(primitives, cell_w, cell_h, vp_w, vp_h);
                    let Some(pipeline) = self.pipeline.as_ref() else {
                        return;
                    };
                    draw_vertices(
                        enc,
                        &self.device,
                        &mut self.upload_arena,
                        &mut self.stats,
                        pipeline,
                        atlas_texture,
                        vertices.as_slice(),
                    );
                }
                WidgetRunCommandPhase::ForegroundRectVertices => {
                    let vertices =
                        build_foreground_rect_quads(primitives, cell_w, cell_h, vp_w, vp_h);
                    let Some(pipeline) = self.pipeline.as_ref() else {
                        return;
                    };
                    draw_vertices(
                        enc,
                        &self.device,
                        &mut self.upload_arena,
                        &mut self.stats,
                        pipeline,
                        atlas_texture,
                        vertices.as_slice(),
                    );
                }
                WidgetRunCommandPhase::ProportionalTextVertices => {
                    let (Some(prop_atlas), Some(prop_pipe), Some(prop_texture)) = (
                        self.prop_atlas.as_mut(),
                        self.prop_pipeline.as_ref(),
                        prop_atlas_texture,
                    ) else {
                        return;
                    };
                    let vertices = build_proportional_text_quads_cached(
                        primitives,
                        prop_atlas,
                        &mut self.prop_text_layout_cache,
                        &mut self.stats,
                        cell_w,
                        cell_h,
                        vp_w,
                        vp_h,
                    );
                    draw_vertices(
                        enc,
                        &self.device,
                        &mut self.upload_arena,
                        &mut self.stats,
                        prop_pipe,
                        prop_texture,
                        vertices.as_slice(),
                    );
                }
            }
        }

        fn draw_dynamic_segment_all(
            &mut self,
            enc: &ProtocolObject<dyn MTLRenderCommandEncoder>,
            seg_scissor: MTLScissorRect,
            seg_prims: &[widget_render::MetalPrimitive],
            atlas_texture: &ProtocolObject<dyn MTLTexture>,
            cell_w: f32,
            cell_h: f32,
            vp_w: f32,
            vp_h: f32,
            image_load_budget: &mut usize,
            render_time_seconds: f32,
        ) -> Duration {
            let mut metal_prep_time = Duration::ZERO;
            let z_layers = z_ordered_primitive_layers(seg_prims);
            for seg_prims in &z_layers {
                let prep_started = Instant::now();
                let (bg_runs, fg_runs) = partition_widget_instance_runs(seg_prims);
                for (widget_type, instances) in &bg_runs {
                    let Some(wpipe) = self.widget_pipelines.get(widget_type) else {
                        continue;
                    };
                    if instances.is_empty() {
                        continue;
                    }
                    draw_widget_instances(
                        enc,
                        &self.device,
                        &mut self.upload_arena,
                        &mut self.stats,
                        wpipe,
                        instances.as_slice(),
                    );
                }

                if let Some(image_pipeline) = self.image_pipeline.clone() {
                    let images = collect_image_primitives(seg_prims);
                    self.draw_image_primitives(
                        enc,
                        &image_pipeline,
                        &images,
                        Some(seg_scissor),
                        image_load_budget,
                        cell_w,
                        cell_h,
                        vp_w,
                        vp_h,
                        render_time_seconds,
                    );
                }

                let prim_quads = {
                    let Some(atlas) = self.atlas.as_mut() else {
                        return metal_prep_time;
                    };
                    build_widget_primitive_quads(seg_prims, atlas, vp_w, vp_h)
                };
                metal_prep_time += prep_started.elapsed();
                if let Some(pipeline) = self.pipeline.as_ref() {
                    draw_vertices(
                        enc,
                        &self.device,
                        &mut self.upload_arena,
                        &mut self.stats,
                        pipeline,
                        atlas_texture,
                        &prim_quads,
                    );
                }

                if let Some(cable_pipeline) = self.patch_cable_pipeline.clone() {
                    let cables = collect_patch_cable_primitives(
                        seg_prims,
                        seg_scissor,
                        cell_w,
                        cell_h,
                        vp_w,
                        vp_h,
                    );
                    draw_patch_cable_instances(
                        enc,
                        &self.device,
                        &mut self.upload_arena,
                        &mut self.stats,
                        &cable_pipeline,
                        &cables,
                    );
                    enc.setScissorRect(seg_scissor);
                }

                if let Some(waveform_pipeline) = self.waveform_pipeline.clone() {
                    let waveforms = collect_waveform_primitives(seg_prims);
                    self.draw_waveform_primitives(
                        enc,
                        &waveform_pipeline,
                        &waveforms,
                        cell_w,
                        cell_h,
                        vp_w,
                        vp_h,
                    );
                }

                for (widget_type, instances) in &fg_runs {
                    let Some(wpipe) = self.widget_pipelines.get(widget_type) else {
                        continue;
                    };
                    if instances.is_empty() {
                        continue;
                    }
                    draw_widget_instances(
                        enc,
                        &self.device,
                        &mut self.upload_arena,
                        &mut self.stats,
                        wpipe,
                        instances.as_slice(),
                    );
                }

                let circle_quads = build_circle_quads(seg_prims, cell_w, cell_h, vp_w, vp_h);
                if let Some(pipeline) = self.pipeline.as_ref() {
                    draw_vertices(
                        enc,
                        &self.device,
                        &mut self.upload_arena,
                        &mut self.stats,
                        pipeline,
                        atlas_texture,
                        &circle_quads,
                    );
                }

                let foreground_rect_quads =
                    build_foreground_rect_quads(seg_prims, cell_w, cell_h, vp_w, vp_h);
                if let Some(pipeline) = self.pipeline.as_ref() {
                    draw_vertices(
                        enc,
                        &self.device,
                        &mut self.upload_arena,
                        &mut self.stats,
                        pipeline,
                        atlas_texture,
                        &foreground_rect_quads,
                    );
                }

                if let (Some(prop_atlas), Some(prop_pipe)) =
                    (self.prop_atlas.as_mut(), self.prop_pipeline.as_ref())
                {
                    let prop_started = Instant::now();
                    let prop_verts = build_proportional_text_quads_cached(
                        seg_prims,
                        prop_atlas,
                        &mut self.prop_text_layout_cache,
                        &mut self.stats,
                        cell_w,
                        cell_h,
                        vp_w,
                        vp_h,
                    );
                    metal_prep_time += prop_started.elapsed();
                    let prop_tex = prop_atlas.texture.clone();
                    draw_vertices(
                        enc,
                        &self.device,
                        &mut self.upload_arena,
                        &mut self.stats,
                        prop_pipe,
                        &prop_tex,
                        &prop_verts,
                    );
                }
            }
            metal_prep_time
        }

        fn draw_widget_run_cached_segment(
            &mut self,
            enc: &ProtocolObject<dyn MTLRenderCommandEncoder>,
            seg_scissor: MTLScissorRect,
            segment_range: Range<usize>,
            offset_prims: &[widget_render::MetalPrimitive],
            run_indices: &[usize],
            offset_runs: &[OffsetMetalPrimitiveRun],
            dirty_widget_ids: &[u64],
            atlas_texture: &ProtocolObject<dyn MTLTexture>,
            cell_w: f32,
            cell_h: f32,
            vp_w: f32,
            vp_h: f32,
            image_load_budget: &mut usize,
            render_time_seconds: f32,
        ) -> Duration {
            if offset_prims[segment_range.clone()]
                .iter()
                .any(|primitive| matches!(primitive, widget_render::MetalPrimitive::ZLayer { .. }))
            {
                self.stats.note_widget_run_cache_bypass_complex();
                return self.draw_dynamic_segment_all(
                    enc,
                    seg_scissor,
                    &offset_prims[segment_range],
                    atlas_texture,
                    cell_w,
                    cell_h,
                    vp_w,
                    vp_h,
                    image_load_budget,
                    render_time_seconds,
                );
            }

            let prop_atlas_texture = self.prop_atlas.as_ref().map(|atlas| atlas.texture.clone());
            let mut groups: Vec<CompiledWidgetRun> = Vec::new();
            let mut cursor = segment_range.start;
            while cursor < segment_range.end {
                let run_index = run_indices[cursor];
                let start = cursor;
                cursor += 1;
                while cursor < segment_range.end && run_indices[cursor] == run_index {
                    cursor += 1;
                }
                let run = &offset_runs[run_index];
                let primitives = &offset_prims[start..cursor];
                if !simple_widget_run_cacheable(&run.widget_type) {
                    self.stats.note_widget_run_cache_bypass_unsupported();
                    return self.draw_dynamic_segment_all(
                        enc,
                        seg_scissor,
                        &offset_prims[segment_range],
                        atlas_texture,
                        cell_w,
                        cell_h,
                        vp_w,
                        vp_h,
                        image_load_budget,
                        render_time_seconds,
                    );
                }
                if widget_run_or_ancestor_dirty(run, dirty_widget_ids) {
                    self.stats.note_widget_run_cache_bypass_dirty();
                    return self.draw_dynamic_segment_all(
                        enc,
                        seg_scissor,
                        &offset_prims[segment_range],
                        atlas_texture,
                        cell_w,
                        cell_h,
                        vp_w,
                        vp_h,
                        image_load_budget,
                        render_time_seconds,
                    );
                }
                if !primitive_run_supported_for_cache(primitives) {
                    self.stats.note_widget_run_cache_bypass_complex();
                    return self.draw_dynamic_segment_all(
                        enc,
                        seg_scissor,
                        &offset_prims[segment_range],
                        atlas_texture,
                        cell_w,
                        cell_h,
                        vp_w,
                        vp_h,
                        image_load_budget,
                        render_time_seconds,
                    );
                }
                let Some(compiled) = self.compiled_simple_widget_run(
                    run.widget_id,
                    &run.widget_type,
                    primitives,
                    dirty_widget_ids,
                    cell_w,
                    cell_h,
                    vp_w,
                    vp_h,
                ) else {
                    self.stats.note_widget_run_cache_bypass_complex();
                    return self.draw_dynamic_segment_all(
                        enc,
                        seg_scissor,
                        &offset_prims[segment_range],
                        atlas_texture,
                        cell_w,
                        cell_h,
                        vp_w,
                        vp_h,
                        image_load_budget,
                        render_time_seconds,
                    );
                };
                self.stats.note_widget_run_cached_draw();
                groups.push(compiled);
            }

            const PHASES: [WidgetRunCommandPhase; 6] = [
                WidgetRunCommandPhase::BackgroundInstances,
                WidgetRunCommandPhase::MainVertices,
                WidgetRunCommandPhase::ForegroundInstances,
                WidgetRunCommandPhase::CircleVertices,
                WidgetRunCommandPhase::ForegroundRectVertices,
                WidgetRunCommandPhase::ProportionalTextVertices,
            ];

            for (phase_idx, phase) in PHASES.iter().copied().enumerate() {
                if phase_idx == 1 {
                    if let Some(image_pipeline) = self.image_pipeline.clone() {
                        let images = collect_image_primitives(&offset_prims[segment_range.clone()]);
                        self.draw_image_primitives(
                            enc,
                            &image_pipeline,
                            &images,
                            Some(seg_scissor),
                            image_load_budget,
                            cell_w,
                            cell_h,
                            vp_w,
                            vp_h,
                            render_time_seconds,
                        );
                    }
                }
                if phase_idx == 2 {
                    if let Some(cable_pipeline) = self.patch_cable_pipeline.clone() {
                        let cables = collect_patch_cable_primitives(
                            &offset_prims[segment_range.clone()],
                            seg_scissor,
                            cell_w,
                            cell_h,
                            vp_w,
                            vp_h,
                        );
                        draw_patch_cable_instances(
                            enc,
                            &self.device,
                            &mut self.upload_arena,
                            &mut self.stats,
                            &cable_pipeline,
                            &cables,
                        );
                        enc.setScissorRect(seg_scissor);
                    }

                    if let Some(waveform_pipeline) = self.waveform_pipeline.clone() {
                        let waveforms =
                            collect_waveform_primitives(&offset_prims[segment_range.clone()]);
                        self.draw_waveform_primitives(
                            enc,
                            &waveform_pipeline,
                            &waveforms,
                            cell_w,
                            cell_h,
                            vp_w,
                            vp_h,
                        );
                    }
                }

                for compiled in &groups {
                    self.draw_compiled_widget_run_phase(
                        enc,
                        compiled,
                        phase,
                        atlas_texture,
                        prop_atlas_texture.as_deref(),
                    );
                }
            }

            Duration::ZERO
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

        pub fn set_widget_cursor(&self, cursor: crate::widget_render::WidgetCursor) {
            let Some(window) = &self.window else {
                return;
            };
            let icon = match cursor {
                crate::widget_render::WidgetCursor::Default => CursorIcon::Default,
                crate::widget_render::WidgetCursor::EwResize => CursorIcon::EwResize,
                crate::widget_render::WidgetCursor::DragCopy => CursorIcon::Copy,
                crate::widget_render::WidgetCursor::DragNotAllowed => CursorIcon::NotAllowed,
            };
            window.set_cursor_icon(icon);
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

        pub fn render_frame_to_png<P: AsRef<std::path::Path>>(
            &mut self,
            frame: &RenderFrame,
            width_px: u32,
            height_px: u32,
            path: P,
        ) -> Result<(), BackendError> {
            if width_px == 0 || height_px == 0 {
                return Err(BackendError::MetalError);
            }
            crate::widget_render::sdf_widget::set_sdf_time_seconds(self.elapsed_time_seconds());
            self.compile_pending_sdf_pipelines();
            self.drain_decoded_images(usize::MAX);

            let desc = MTLTextureDescriptor::new();
            desc.setPixelFormat(MTLPixelFormat::BGRA8Unorm);
            unsafe {
                desc.setWidth(width_px as usize);
                desc.setHeight(height_px as usize);
            }
            desc.setStorageMode(MTLStorageMode::Shared);
            desc.setUsage(MTLTextureUsage::RenderTarget | MTLTextureUsage::ShaderRead);
            let Some(texture) = self.device.newTextureWithDescriptor(&desc) else {
                return Err(BackendError::MetalError);
            };

            self.render_frame_into_texture(frame, &texture)?;

            let bytes_per_pixel = 4usize;
            let bytes_per_row = width_px as usize * bytes_per_pixel;
            let mut bgra = vec![0u8; bytes_per_row * height_px as usize];
            unsafe {
                texture.getBytes_bytesPerRow_fromRegion_mipmapLevel(
                    NonNull::new(bgra.as_mut_ptr().cast()).ok_or(BackendError::MetalError)?,
                    bytes_per_row,
                    MTLRegion {
                        origin: MTLOrigin { x: 0, y: 0, z: 0 },
                        size: MTLSize {
                            width: width_px as usize,
                            height: height_px as usize,
                            depth: 1,
                        },
                    },
                    0,
                );
            }
            let mut rgba = Vec::with_capacity(bgra.len());
            for px in bgra.chunks_exact(4) {
                rgba.extend_from_slice(&[px[2], px[1], px[0], px[3]]);
            }
            image::save_buffer_with_format(
                path,
                &rgba,
                width_px,
                height_px,
                image::ColorType::Rgba8,
                image::ImageFormat::Png,
            )
            .map_err(|_| BackendError::MetalError)
        }

        fn render_frame_into_texture(
            &mut self,
            frame: &RenderFrame,
            texture: &ProtocolObject<dyn MTLTexture>,
        ) -> Result<(), BackendError> {
            let time_seconds = self.elapsed_time_seconds();
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
            let vp_w = texture.width() as f32;
            let vp_h = texture.height() as f32;
            self.upload_arena.begin_frame(&mut self.stats);
            self.prop_text_layout_cache.begin_frame();
            self.begin_compiled_widget_run_frame();
            let max_rows_exact = (vp_h / cell_h - 1.0).max(0.0);
            let max_rows = max_rows_exact.floor() as u16;

            let primitive_scene = frame
                .widget_layout
                .as_ref()
                .map(|layout| {
                    self.widget_scene_for_layout(
                        frame.widget_content_cache_key,
                        frame.widget_layout_cache_key,
                        layout,
                        &frame.dirty_widget_ids,
                        WidgetViewport {
                            cell_w,
                            cell_h,
                            vp_w,
                            vp_h,
                            time_seconds,
                            focused_widget_id: frame.focused_widget_id,
                            focused_branch: false,
                            tile_content_rows: max_rows_exact,
                            scroll_top: frame.widget_scroll_top,
                            scroll_left: frame.widget_scroll_left,
                            inherited_hover: false,
                        },
                        frame.widget_scroll_top,
                        max_rows,
                    )
                })
                .unwrap_or_default();

            let Some(atlas) = &mut self.atlas else {
                return Ok(());
            };
            let text_quads = build_text_quads(frame, atlas, vp_w, vp_h);
            let primitive_quads = build_widget_primitive_quads(&primitive_scene, atlas, vp_w, vp_h);
            let (primitive_bg_runs, primitive_fg_runs) =
                partition_widget_instance_runs(&primitive_scene);

            let render_desc = MTLRenderPassDescriptor::new();
            let attach = unsafe { render_desc.colorAttachments().objectAtIndexedSubscript(0) };
            attach.setTexture(Some(texture));
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
                .renderCommandEncoderWithDescriptor(&render_desc)
                .ok_or(BackendError::MetalError)?;
            enc.setScissorRect(MTLScissorRect {
                x: 0,
                y: 0,
                width: texture.width(),
                height: texture.height(),
            });

            for (widget_type, instances) in &primitive_bg_runs {
                let Some(wpipe) = self.widget_pipelines.get(widget_type) else {
                    continue;
                };
                draw_widget_instances(
                    &enc,
                    &self.device,
                    &mut self.upload_arena,
                    &mut self.stats,
                    wpipe,
                    instances.as_slice(),
                );
            }

            if let Some(image_pipeline) = self.image_pipeline.clone() {
                let mut image_load_budget = usize::MAX;
                let images = collect_image_primitives(&primitive_scene);
                self.draw_image_primitives(
                    &enc,
                    &image_pipeline,
                    &images,
                    None,
                    &mut image_load_budget,
                    cell_w,
                    cell_h,
                    vp_w,
                    vp_h,
                    time_seconds,
                );
            }

            draw_vertices(
                &enc,
                &self.device,
                &mut self.upload_arena,
                &mut self.stats,
                &pipeline,
                &atlas_texture,
                text_quads.as_slice(),
            );
            draw_vertices(
                &enc,
                &self.device,
                &mut self.upload_arena,
                &mut self.stats,
                &pipeline,
                &atlas_texture,
                primitive_quads.as_slice(),
            );

            if let Some(cable_pipeline) = self.patch_cable_pipeline.clone() {
                let clip = MTLScissorRect {
                    x: 0,
                    y: 0,
                    width: texture.width(),
                    height: texture.height(),
                };
                let cables = collect_patch_cable_primitives(
                    &primitive_scene,
                    clip,
                    cell_w,
                    cell_h,
                    vp_w,
                    vp_h,
                );
                draw_patch_cable_instances(
                    &enc,
                    &self.device,
                    &mut self.upload_arena,
                    &mut self.stats,
                    &cable_pipeline,
                    &cables,
                );
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

            for (widget_type, instances) in &primitive_fg_runs {
                let Some(wpipe) = self.widget_pipelines.get(widget_type) else {
                    continue;
                };
                draw_widget_instances(
                    &enc,
                    &self.device,
                    &mut self.upload_arena,
                    &mut self.stats,
                    wpipe,
                    instances.as_slice(),
                );
            }

            let circle_quads = build_circle_quads(&primitive_scene, cell_w, cell_h, vp_w, vp_h);
            draw_vertices(
                &enc,
                &self.device,
                &mut self.upload_arena,
                &mut self.stats,
                &pipeline,
                &atlas_texture,
                circle_quads.as_slice(),
            );

            let foreground_rect_quads =
                build_foreground_rect_quads(&primitive_scene, cell_w, cell_h, vp_w, vp_h);
            draw_vertices(
                &enc,
                &self.device,
                &mut self.upload_arena,
                &mut self.stats,
                &pipeline,
                &atlas_texture,
                foreground_rect_quads.as_slice(),
            );

            if let (Some(prop_atlas), Some(prop_pipe)) =
                (self.prop_atlas.as_mut(), self.prop_pipeline.as_ref())
            {
                let prop_verts = build_proportional_text_quads_cached(
                    &primitive_scene,
                    prop_atlas,
                    &mut self.prop_text_layout_cache,
                    &mut self.stats,
                    cell_w,
                    cell_h,
                    vp_w,
                    vp_h,
                );
                let prop_tex = prop_atlas.texture.clone();
                draw_vertices(
                    &enc,
                    &self.device,
                    &mut self.upload_arena,
                    &mut self.stats,
                    prop_pipe,
                    &prop_tex,
                    prop_verts.as_slice(),
                );
            }

            enc.endEncoding();
            cmdbuf.commit();
            self.upload_arena.finish_frame(cmdbuf.clone());
            cmdbuf.waitUntilCompleted();
            Ok(())
        }

        pub fn take_pending_image_loads(&mut self) -> bool {
            let pending = self.pending_image_loads;
            self.pending_image_loads = false;
            pending
        }

        pub fn suspend_image_loading_for(&mut self, duration: Duration) {
            self.image_load_suspended_until = Some(Instant::now() + duration);
            self.pending_image_loads = true;
        }

        pub fn set_image_decode_min_interval(&mut self, interval: Duration) {
            self.image_decode_min_interval = interval;
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

        fn image_path_and_modified(src: &str) -> Option<(PathBuf, Option<std::time::SystemTime>)> {
            if src.is_empty() {
                return None;
            }
            let mut path = PathBuf::from(src);
            if !path.is_absolute() {
                path = std::env::current_dir().ok()?.join(path);
            }
            let metadata = fs::metadata(&path).ok()?;
            let modified = metadata.modified().ok();
            Some((path, modified))
        }

        fn ensure_image_texture(
            &mut self,
            src: &str,
            load_budget: &mut usize,
        ) -> Option<&ImageTextureResource> {
            let (path, modified) = Self::image_path_and_modified(src)?;
            let should_reload = self
                .image_textures
                .get(&path)
                .map(|cached| cached.modified != modified)
                .unwrap_or(true);
            if should_reload {
                if self
                    .image_load_suspended_until
                    .is_some_and(|until| Instant::now() < until)
                {
                    self.pending_image_loads = true;
                    return None;
                }
                if self.image_decode_in_flight.contains(&path) {
                    self.pending_image_loads = true;
                    return None;
                }
                if *load_budget == 0 {
                    self.pending_image_loads = true;
                    return None;
                }
                if self
                    .image_last_decode_at
                    .is_some_and(|last| last.elapsed() < self.image_decode_min_interval)
                {
                    self.pending_image_loads = true;
                    return None;
                }
                *load_budget = load_budget.saturating_sub(1);
                self.image_last_decode_at = Some(Instant::now());
                if self.image_decode_in_flight.insert(path.clone()) {
                    if self
                        .image_decode_tx
                        .send(ImageDecodeJob {
                            path: path.clone(),
                            modified,
                        })
                        .is_err()
                    {
                        self.image_decode_in_flight.remove(&path);
                    }
                }
                self.pending_image_loads = true;
            }
            self.image_textures.get(&path)
        }

        fn drain_decoded_images(&mut self, mut upload_budget: usize) {
            while upload_budget > 0 {
                let Ok(result) = self.image_decode_rx.try_recv() else {
                    break;
                };
                self.image_decode_in_flight.remove(&result.path);
                let Some(decoded) = result.image else {
                    self.pending_image_loads = true;
                    continue;
                };

                let desc = MTLTextureDescriptor::new();
                unsafe {
                    desc.setPixelFormat(MTLPixelFormat::BGRA8Unorm);
                    desc.setWidth(decoded.width as usize);
                    desc.setHeight(decoded.height as usize);
                }
                let Some(texture) = self.device.newTextureWithDescriptor(&desc) else {
                    self.pending_image_loads = true;
                    continue;
                };
                unsafe {
                    texture.replaceRegion_mipmapLevel_withBytes_bytesPerRow(
                        MTLRegion {
                            origin: MTLOrigin { x: 0, y: 0, z: 0 },
                            size: MTLSize {
                                width: decoded.width as usize,
                                height: decoded.height as usize,
                                depth: 1,
                            },
                        },
                        0,
                        NonNull::new(decoded.bgra.as_ptr() as *mut core::ffi::c_void).unwrap(),
                        decoded.width as usize * 4,
                    );
                }
                self.image_textures.insert(
                    result.path,
                    ImageTextureResource {
                        texture,
                        width: decoded.width,
                        height: decoded.height,
                        modified: result.modified,
                    },
                );
                self.pending_image_loads = true;
                upload_budget -= 1;
            }
        }

        /// Compile Metal pipelines for any SDF widgets that have been registered
        /// since the last render. This enables lazy compilation of defwidget shaders.
        fn compile_pending_sdf_pipelines(&mut self) {
            use crate::widget_render::sdf_widget;
            let generation = sdf_widget::sdf_widget_registry_generation();
            if self.sdf_widget_pipeline_registry_generation == generation {
                return;
            }
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
            self.sdf_widget_pipeline_registry_generation = generation;
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
                    show_selection_start: if primitive.show_selection_start { 1 } else { 0 },
                    show_selection_end: if primitive.show_selection_end { 1 } else { 0 },
                    playhead_position: primitive.playhead_position,
                    show_playhead: if primitive.show_playhead { 1 } else { 0 },
                    waveform_color: primitive.waveform_color.to_rgba(),
                    inactive_waveform_color: primitive.inactive_waveform_color.to_rgba(),
                    marker_color: primitive.marker_color.to_rgba(),
                    active_marker_color: primitive.active_marker_color.to_rgba(),
                    active_selection_start: if primitive.active_selection_start {
                        1
                    } else {
                        0
                    },
                    active_selection_end: if primitive.active_selection_end { 1 } else { 0 },
                    selection_color: primitive.selection_color.to_rgba(),
                    bg_color: theme::BG().to_rgba(),
                    border_color: theme::BORDER_INACTIVE().to_rgba(),
                };
                let Some(instance_upload) =
                    self.upload_arena
                        .upload_one(&self.device, &instance, &mut self.stats)
                else {
                    continue;
                };
                enc.setRenderPipelineState(pipeline);
                unsafe {
                    enc.setVertexBuffer_offset_atIndex(
                        Some(&instance_upload.buffer),
                        instance_upload.offset,
                        0,
                    );
                    enc.setFragmentBuffer_offset_atIndex(Some(&waveform_buffer), 0, 1);
                    enc.drawPrimitives_vertexStart_vertexCount(MTLPrimitiveType::Triangle, 0, 6);
                }
                self.stats.note_draw_command();
            }
        }

        fn draw_image_primitives(
            &mut self,
            enc: &ProtocolObject<dyn MTLRenderCommandEncoder>,
            pipeline: &ProtocolObject<dyn MTLRenderPipelineState>,
            images: &[widget_render::MetalImagePrimitive],
            scissor: Option<MTLScissorRect>,
            load_budget: &mut usize,
            cell_w: f32,
            cell_h: f32,
            vp_w: f32,
            vp_h: f32,
            time_seconds: f32,
        ) {
            for image in images {
                if let Some(scissor) = scissor {
                    if !image_intersects_scissor(image, scissor, cell_w, cell_h) {
                        continue;
                    }
                }
                let Some(resource) = self.ensure_image_texture(&image.src, load_budget) else {
                    continue;
                };
                let texture = resource.texture.clone();
                let image_w = resource.width;
                let image_h = resource.height;
                let rotation = self.effective_image_rotation(image, time_seconds);
                let verts = image_vertices(
                    image, image_w, image_h, cell_w, cell_h, vp_w, vp_h, rotation,
                );
                if verts.is_empty() {
                    continue;
                }
                let Some(upload) =
                    self.upload_arena
                        .upload_slice(&self.device, verts.as_slice(), &mut self.stats)
                else {
                    continue;
                };
                enc.setRenderPipelineState(pipeline);
                unsafe {
                    enc.setVertexBuffer_offset_atIndex(Some(&upload.buffer), upload.offset, 0);
                    enc.setFragmentTexture_atIndex(Some(&texture), 0);
                    enc.drawPrimitives_vertexStart_vertexCount(
                        MTLPrimitiveType::Triangle,
                        0,
                        verts.len() as _,
                    );
                }
                self.stats.note_draw_command();
            }
        }

        fn effective_image_rotation(
            &mut self,
            image: &widget_render::MetalImagePrimitive,
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
                if angular_distance(predicted, base_angle) < SEEK_SNAP_THRESHOLD_RADIANS {
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

        /// Render a tiled frame with per-tile scissor clipping.
        pub fn render_tiled(&mut self, tiled: &TiledRenderFrame) -> Result<(), BackendError> {
            crate::widget_render::sdf_widget::set_sdf_time_seconds(self.elapsed_time_seconds());
            self.agent_instrument_stub_animation_visible = false;
            let render_time_seconds = self.elapsed_time_seconds();
            self.sync_window_theme();
            let mut widget_scene_build_time = Duration::ZERO;
            let mut metal_prep_time = Duration::ZERO;
            let mut image_load_budget = 1usize;
            self.drain_decoded_images(2);

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
            let mut mod_patch_ports = Vec::new();
            let mut global_overlay_prims = Vec::new();

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
            self.upload_arena.begin_frame(&mut self.stats);
            self.prop_text_layout_cache.begin_frame();
            self.begin_compiled_widget_run_frame();

            // ── Per-tile rendering with scissor rect ─────────────────────────
            for tile in &tiled.tiles {
                let tile_left_px = tile.rect.col * cell_w;
                let tile_top_px = tile.rect.row * cell_h;
                let tile_width_px = tile.rect.width * cell_w;
                let tile_height_px = tile.rect.height * cell_h;
                let border_inset_px = if tile.show_border {
                    tile.border_width_px
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

                let tile_scissor_left = tile_left_px.floor().max(0.0);
                let tile_scissor_top = tile_top_px.floor().max(0.0);
                let tile_scissor_right =
                    (tile_left_px + tile_width_px).ceil().max(tile_scissor_left);
                let tile_scissor_bottom =
                    (tile_top_px + tile_height_px).ceil().max(tile_scissor_top);
                let tile_scissor = MTLScissorRect {
                    x: tile_scissor_left as usize,
                    y: tile_scissor_top as usize,
                    width: (tile_scissor_right - tile_scissor_left) as usize,
                    height: (tile_scissor_bottom - tile_scissor_top) as usize,
                };

                // Set scissor rect to clip to tile content area (exclude border and status row)
                let scissor_left = content_left_px.floor().max(0.0);
                let scissor_top = content_top_px.floor().max(0.0);
                let scissor_right = content_right_px.ceil().max(scissor_left);
                let scissor_bottom = content_bottom_px.ceil().max(scissor_top);
                let content_scissor = MTLScissorRect {
                    x: scissor_left as usize,
                    y: scissor_top as usize,
                    width: (scissor_right - scissor_left) as usize,
                    height: (scissor_bottom - scissor_top) as usize,
                };

                let tile_bg = tile
                    .background_color_name
                    .as_deref()
                    .and_then(theme::named_color)
                    .or(tile.background_color)
                    .unwrap_or(theme::BG());
                let mut tile_bg_verts = Vec::new();
                enc.setScissorRect(tile_scissor);
                push_rounded_rect_fill_px(
                    &mut tile_bg_verts,
                    tile_left_px,
                    tile_top_px,
                    tile_width_px,
                    tile_height_px,
                    tile.border_radius_px,
                    tile_bg,
                    vp_w,
                    vp_h,
                );
                draw_vertices(
                    &enc,
                    &self.device,
                    &mut self.upload_arena,
                    &mut self.stats,
                    &pipeline,
                    &atlas_texture,
                    &tile_bg_verts,
                );

                enc.setScissorRect(content_scissor);

                // ── Text content (shifted by horizontal scroll) ──────────────
                let hscroll = tile.frame.widget_scroll_left;
                let offset = TileOffset {
                    col: content_col - hscroll,
                    row: content_row,
                };
                let text_verts = {
                    let atlas = self.atlas.as_mut().ok_or(BackendError::MetalError)?;
                    build_text_quads_offset(&tile.frame, atlas, vp_w, vp_h, offset, tile_bg)
                };
                draw_vertices(
                    &enc,
                    &self.device,
                    &mut self.upload_arena,
                    &mut self.stats,
                    &pipeline,
                    &atlas_texture,
                    &text_verts,
                );

                // ── Widget primitives (clipped to content area, above status) ─
                // Collect with LOCAL coords (no offset) so scroll/clip logic works,
                // then offset the resulting primitives to screen position.
                if let Some(ref layout) = tile.frame.widget_layout {
                    if layout_contains_agent_instrument_stub_animation(layout) {
                        self.note_agent_instrument_stub_animation_detected();
                    }
                    let time_seconds = self.elapsed_time_seconds();
                    let inner_rows_exact = ((content_bottom_px - content_top_px) / cell_h).max(0.0);
                    let inner_rows = inner_rows_exact.floor() as u16;

                    let viewport = WidgetViewport {
                        cell_w,
                        cell_h,
                        vp_w,
                        vp_h,
                        time_seconds,
                        focused_widget_id: tile.frame.focused_widget_id,
                        focused_branch: false,
                        tile_content_rows: inner_rows_exact,
                        scroll_top: tile.frame.widget_scroll_top,
                        scroll_left: tile.frame.widget_scroll_left,
                        inherited_hover: false,
                    };
                    // Offset primitives to tile's screen position,
                    // shifted by both text scroll (vertical) and hscroll (horizontal)
                    // so widgets move with the text.
                    let text_scroll = tile.frame.text_scroll_top as f32;
                    let widget_scroll = tile.frame.widget_scroll_top;
                    let widget_col_off = content_col - tile.frame.widget_scroll_left;
                    let widget_row_off = content_row - text_scroll - widget_scroll;
                    collect_mod_patch_ports(
                        layout,
                        widget_col_off,
                        widget_row_off,
                        cell_w,
                        cell_h,
                        content_scissor,
                        &mut mod_patch_ports,
                    );
                    let content_width_cells =
                        ((content_right_px - content_left_px) / cell_w).max(0.0);
                    let fill_extra_cols = (content_width_cells - layout.rect.width).max(0.0);
                    let use_widget_run_cache = !tile.frame.dirty_widget_ids.is_empty()
                        && widget_render::overlay_widget_id().is_none()
                        && !widget_render::layout_wants_animation_frames(layout);
                    let scene_started = Instant::now();
                    let (
                        offset_prims,
                        offset_run_indices,
                        offset_runs,
                        overlay_prims,
                        use_widget_run_cache,
                    ) = if use_widget_run_cache {
                        let (run_scene_key, overlay) = self
                            .refresh_widget_run_scene_for_dirty_layout(
                                tile.frame.widget_content_cache_key,
                                tile.frame.widget_layout_cache_key,
                                layout,
                                &tile.frame.dirty_widget_ids,
                                viewport,
                                tile.frame.widget_scroll_top,
                                inner_rows,
                            );
                        let primitive_runs = self
                            .cached_widget_run_scenes
                            .get(&run_scene_key)
                            .map(|scene| scene.runs.as_slice())
                            .unwrap_or(&[]);
                        let mut offset_prims = Vec::new();
                        let mut offset_run_indices = Vec::new();
                        let mut offset_runs = Vec::new();
                        for run in primitive_runs {
                            let mut run_primitives = run.primitives.clone();
                            Self::refresh_widget_scene_time(
                                &mut run_primitives,
                                viewport.time_seconds,
                            );
                            let run_index = offset_runs.len();
                            for primitive in run_primitives {
                                let offset = offset_primitive(
                                    extend_right_edge_primitive(
                                        primitive,
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
                                );
                                offset_run_indices.push(run_index);
                                offset_prims.push(offset);
                            }
                            offset_runs.push(OffsetMetalPrimitiveRun {
                                widget_id: run.widget_id,
                                widget_type: run.widget_type.clone(),
                                ancestor_widget_ids: run.ancestor_widget_ids.clone(),
                            });
                        }
                        self.stats.note_widget_primitives(offset_prims.len());
                        (offset_prims, offset_run_indices, offset_runs, overlay, true)
                    } else {
                        let primitives = self.widget_scene_for_layout(
                            tile.frame.widget_content_cache_key,
                            tile.frame.widget_layout_cache_key,
                            layout,
                            &tile.frame.dirty_widget_ids,
                            viewport,
                            tile.frame.widget_scroll_top,
                            inner_rows,
                        );
                        let overlay = if widget_render::overlay_widget_id().is_some() {
                            let (_, overlay) = widget_render::collect_metal_primitives(
                                layout,
                                viewport,
                                tile.frame.widget_scroll_top,
                                inner_rows,
                            );
                            overlay
                        } else {
                            Vec::new()
                        };
                        let offset_prims: Vec<_> = primitives
                            .into_iter()
                            .map(|p| {
                                offset_primitive(
                                    extend_right_edge_primitive(
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
                        (offset_prims, Vec::new(), Vec::new(), overlay, false)
                    };
                    widget_scene_build_time += scene_started.elapsed();
                    if contains_agent_instrument_stub_animation(&offset_prims) {
                        self.note_agent_instrument_stub_animation_detected();
                    }
                    // Split primitives into segments at clip rect boundaries.
                    // Each segment gets its own scissor rect for proper scroll clipping.
                    let segments =
                        split_prim_segment_ranges(&offset_prims, content_scissor, cell_w, cell_h);
                    self.stats.note_widget_segments(segments.len());

                    self.compile_pending_sdf_pipelines();
                    for (seg_scissor, seg_range) in &segments {
                        enc.setScissorRect(*seg_scissor);
                        metal_prep_time += if use_widget_run_cache {
                            self.draw_widget_run_cached_segment(
                                &enc,
                                *seg_scissor,
                                seg_range.clone(),
                                &offset_prims,
                                &offset_run_indices,
                                &offset_runs,
                                &tile.frame.dirty_widget_ids,
                                &atlas_texture,
                                cell_w,
                                cell_h,
                                vp_w,
                                vp_h,
                                &mut image_load_budget,
                                render_time_seconds,
                            )
                        } else {
                            self.draw_dynamic_segment_all(
                                &enc,
                                *seg_scissor,
                                &offset_prims[seg_range.clone()],
                                &atlas_texture,
                                cell_w,
                                cell_h,
                                vp_w,
                                vp_h,
                                &mut image_load_budget,
                                render_time_seconds,
                            )
                        };
                    }
                    // Restore tile scissor after segments
                    enc.setScissorRect(content_scissor);

                    // ── Overlay collection (dropdown menus, etc.) ───────────
                    // Defer drawing until after global passes such as patch
                    // cables so overlays remain the topmost widget layer.
                    if !overlay_prims.is_empty() {
                        // Overlay primitives are already in post-scroll tile-local
                        // coordinates; only the tile origin offset still needs to
                        // be applied before drawing.
                        let overlay_col_off = content_col;
                        let overlay_row_off = content_row;
                        let offset_overlay: Vec<_> = overlay_prims
                            .into_iter()
                            .map(|p| {
                                offset_primitive(
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
                        if contains_agent_instrument_stub_animation(&offset_overlay) {
                            self.note_agent_instrument_stub_animation_detected();
                        }
                        global_overlay_prims.extend(offset_overlay);
                    }
                }

                // ── Per-tile status bar (drawn ON TOP of widgets with full-tile scissor)
                if tile.show_status {
                    let status_left_px = content_left_px;
                    let status_right_px = content_right_px;
                    let status_top_px = (tile_top_px + tile_height_px - border_inset_px - cell_h)
                        .max(content_top_px);
                    let status_bottom_px =
                        (tile_top_px + tile_height_px - border_inset_px).max(status_top_px);
                    enc.setScissorRect(MTLScissorRect {
                        x: status_left_px.floor().max(0.0) as usize,
                        y: status_top_px.floor().max(0.0) as usize,
                        width: (status_right_px.ceil() - status_left_px.floor()).max(0.0) as usize,
                        height: (status_bottom_px.ceil() - status_top_px.floor()).max(0.0) as usize,
                    });
                    let mut status_verts = Vec::new();
                    let status_col = status_left_px / cell_w;
                    let status_row = status_top_px / cell_h;
                    let status_width_px = (status_right_px - status_left_px).max(0.0);
                    let status_bg = to_rgba(theme::STATUS_BG());
                    let sx0 = ndc_x(status_left_px);
                    let sx1 = ndc_x(status_right_px);
                    let sy0 = ndc_y(status_top_px);
                    let sy1 = ndc_y(status_bottom_px);
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
                    for (i, cell) in tile.frame.status_cells.iter().enumerate() {
                        let ch_col = status_col + i as f32;
                        if (ch_col + 1.0) * cell_w > status_right_px {
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
                                (ch_col, status_row),
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
                    // Draw the edge lines AFTER cell backgrounds so they render on top
                    push_horizontal_rule(
                        &mut status_verts,
                        status_left_px,
                        status_top_px,
                        status_width_px,
                        1.0,
                        theme::STATUS_EDGE(),
                        vp_w,
                        vp_h,
                    );
                    push_horizontal_rule(
                        &mut status_verts,
                        status_left_px,
                        status_bottom_px - 1.0,
                        status_width_px,
                        1.0,
                        theme::STATUS_EDGE(),
                        vp_w,
                        vp_h,
                    );
                    draw_vertices(
                        &enc,
                        &self.device,
                        &mut self.upload_arena,
                        &mut self.stats,
                        &pipeline,
                        &atlas_texture,
                        &status_verts,
                    );
                }

                // ── Thin pixel borders (drawn AFTER content, on top) ─────────
                if has_multiple_tiles && tile.show_border {
                    enc.setScissorRect(tile_scissor);
                    let border_color = if tile.is_active {
                        theme::BORDER_ACTIVE()
                    } else {
                        tile_bg
                    };
                    let mut bverts = Vec::new();
                    push_rounded_rect_border_px(
                        &mut bverts,
                        tile_left_px,
                        tile_top_px,
                        tile_width_px,
                        tile_height_px,
                        tile.border_width_px,
                        tile.border_radius_px,
                        border_color,
                        vp_w,
                        vp_h,
                    );
                    draw_vertices(
                        &enc,
                        &self.device,
                        &mut self.upload_arena,
                        &mut self.stats,
                        &pipeline,
                        &atlas_texture,
                        &bverts,
                    );
                }
            }

            // ── Global patch cables (no tile scissor) ───────────────────────
            // These are collected from visible mod-port widget rects and drawn
            // after tiles so cables can cross channel strips and tile bounds.
            if !mod_patch_ports.is_empty() {
                enc.setScissorRect(MTLScissorRect {
                    x: 0,
                    y: 0,
                    width: vp_w as usize,
                    height: vp_h as usize,
                });
                if let Some(cable_pipeline) = self.patch_cable_pipeline.clone() {
                    let cursor_px = (self.cursor_pos.0 * cell_w, self.cursor_pos.1 * cell_h);
                    let cables = build_mod_patch_cables(&mod_patch_ports, vp_w, vp_h, cursor_px);
                    draw_patch_cable_instances(
                        &enc,
                        &self.device,
                        &mut self.upload_arena,
                        &mut self.stats,
                        &cable_pipeline,
                        &cables,
                    );
                    let highlight =
                        build_mod_patch_drag_highlight(&mod_patch_ports, cursor_px, vp_w, vp_h);
                    if let Some((highlight_verts, highlight_clip)) = highlight
                        && !highlight_verts.is_empty()
                    {
                        enc.setScissorRect(highlight_clip);
                        draw_vertices(
                            &enc,
                            &self.device,
                            &mut self.upload_arena,
                            &mut self.stats,
                            &pipeline,
                            &atlas_texture,
                            &highlight_verts,
                        );
                    }
                }
            }

            // ── Global overlay pass (dropdown menus, etc.) ──────────────────
            // Drawn after tiles and patch cables so open dropdowns always sit
            // above interactive wiring and tile chrome.
            if !global_overlay_prims.is_empty() {
                enc.setScissorRect(MTLScissorRect {
                    x: 0,
                    y: 0,
                    width: vp_w as usize,
                    height: vp_h as usize,
                });

                let (bg_runs, fg_runs) = partition_widget_instance_runs(&global_overlay_prims);
                for (widget_type, instances) in &bg_runs {
                    let Some(wpipe) = self.widget_pipelines.get(widget_type) else {
                        continue;
                    };
                    if instances.is_empty() {
                        continue;
                    }
                    draw_widget_instances(
                        &enc,
                        &self.device,
                        &mut self.upload_arena,
                        &mut self.stats,
                        wpipe,
                        instances.as_slice(),
                    );
                }

                if let Some(image_pipeline) = self.image_pipeline.clone() {
                    let images = collect_image_primitives(&global_overlay_prims);
                    self.draw_image_primitives(
                        &enc,
                        &image_pipeline,
                        &images,
                        None,
                        &mut image_load_budget,
                        cell_w,
                        cell_h,
                        vp_w,
                        vp_h,
                        render_time_seconds,
                    );
                }

                let prim_quads = {
                    let atlas = self.atlas.as_mut().ok_or(BackendError::MetalError)?;
                    build_widget_primitive_quads(&global_overlay_prims, atlas, vp_w, vp_h)
                };
                draw_vertices(
                    &enc,
                    &self.device,
                    &mut self.upload_arena,
                    &mut self.stats,
                    &pipeline,
                    &atlas_texture,
                    &prim_quads,
                );

                if let (Some(prop_atlas), Some(prop_pipe)) =
                    (self.prop_atlas.as_mut(), self.prop_pipeline.as_ref())
                {
                    let prop_verts = build_proportional_text_quads_cached(
                        &global_overlay_prims,
                        prop_atlas,
                        &mut self.prop_text_layout_cache,
                        &mut self.stats,
                        cell_w,
                        cell_h,
                        vp_w,
                        vp_h,
                    );
                    let prop_tex = prop_atlas.texture.clone();
                    draw_vertices(
                        &enc,
                        &self.device,
                        &mut self.upload_arena,
                        &mut self.stats,
                        prop_pipe,
                        &prop_tex,
                        &prop_verts,
                    );
                }

                for (widget_type, instances) in &fg_runs {
                    let Some(wpipe) = self.widget_pipelines.get(widget_type) else {
                        continue;
                    };
                    if instances.is_empty() {
                        continue;
                    }
                    draw_widget_instances(
                        &enc,
                        &self.device,
                        &mut self.upload_arena,
                        &mut self.stats,
                        wpipe,
                        instances.as_slice(),
                    );
                }

                let foreground_rect_quads =
                    build_foreground_rect_quads(&global_overlay_prims, cell_w, cell_h, vp_w, vp_h);
                draw_vertices(
                    &enc,
                    &self.device,
                    &mut self.upload_arena,
                    &mut self.stats,
                    &pipeline,
                    &atlas_texture,
                    &foreground_rect_quads,
                );
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
                    let anchor_row = row_off + comp.anchor.0;
                    let popup_row = anchor_row + 1;
                    let total_cols = (vp_w / cell_w).floor().max(1.0) as usize;
                    let total_rows = (vp_h / cell_h).floor().max(1.0) as usize;
                    let label_w = comp
                        .entries
                        .iter()
                        .map(|e| e.label.len())
                        .max()
                        .unwrap_or(0)
                        .max(12)
                        .min(34);
                    let has_doc = comp.doc.is_some();
                    let doc_gap = if has_doc { 1 } else { 0 };
                    let desired_pane_w = if has_doc { 54 } else { label_w + 4 };
                    let max_pane_w = if has_doc {
                        total_cols
                            .saturating_sub(popup_col + doc_gap + 2)
                            .saturating_div(2)
                            .max(label_w + 4)
                    } else {
                        total_cols.saturating_sub(popup_col + 2)
                    };
                    let pane_w = desired_pane_w.min(max_pane_w).max(label_w + 4).min(64);
                    let show_doc = has_doc && pane_w >= 26;
                    let total_panel_w = if show_doc {
                        pane_w * 2 + doc_gap
                    } else {
                        pane_w
                    };
                    let popup_col = if popup_col + total_panel_w + 1 > total_cols {
                        total_cols.saturating_sub(total_panel_w + 1)
                    } else {
                        popup_col
                    };
                    let doc_col = popup_col + pane_w + doc_gap;
                    let doc_pad_x = 3usize;
                    let doc_pad_top = 1usize;
                    let doc_text_w = pane_w.saturating_sub(doc_pad_x * 2);
                    let doc_body = comp
                        .doc
                        .as_ref()
                        .map(|(_, body)| wrap_completion_doc_lines(body, doc_text_w));
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
                    let panel_bg = Color::rgba(0.105, 0.115, 0.135, 0.96);
                    let panel_border = Color::rgba(0.24, 0.26, 0.30, 1.0);
                    let shadow = Color::rgba(0.0, 0.0, 0.0, 0.38);
                    let muted_fg = to_rgba(Color::rgba(0.58, 0.59, 0.62, 1.0));
                    let doc_bg = Color::rgba(0.085, 0.09, 0.105, 0.98);
                    let mut rounded = Vec::new();
                    push_rounded_instance_cells(
                        &mut rounded,
                        popup_col as f32 + 0.18,
                        panel_row as f32 + 0.20,
                        pane_w as f32,
                        panel_h as f32,
                        shadow,
                        10.0,
                        cell_w,
                        cell_h,
                        vp_w,
                        vp_h,
                    );
                    push_rounded_instance_cells(
                        &mut rounded,
                        popup_col as f32,
                        panel_row as f32,
                        pane_w as f32,
                        panel_h as f32,
                        panel_bg,
                        8.0,
                        cell_w,
                        cell_h,
                        vp_w,
                        vp_h,
                    );
                    if show_doc {
                        push_rounded_instance_cells(
                            &mut rounded,
                            doc_col as f32 + 0.18,
                            panel_row as f32 + 0.20,
                            pane_w as f32,
                            panel_h as f32,
                            shadow,
                            10.0,
                            cell_w,
                            cell_h,
                            vp_w,
                            vp_h,
                        );
                        push_rounded_instance_cells(
                            &mut rounded,
                            doc_col as f32,
                            panel_row as f32,
                            pane_w as f32,
                            panel_h as f32,
                            doc_bg,
                            8.0,
                            cell_w,
                            cell_h,
                            vp_w,
                            vp_h,
                        );
                    }
                    if let Some(wpipe) = self.widget_pipelines.get("dropdown") {
                        draw_widget_instances(
                            &enc,
                            &self.device,
                            &mut self.upload_arena,
                            &mut self.stats,
                            wpipe,
                            rounded.as_slice(),
                        );
                    }
                    for (i, entry) in comp.entries.iter().enumerate() {
                        let row = panel_row + list_pad_top + i * row_step;
                        if row >= panel_row + panel_h {
                            break;
                        }
                        let bg = if entry.selected { sel_bg } else { unsel_bg };
                        let mut selected = Vec::new();
                        push_rounded_instance_cells_rgba(
                            &mut selected,
                            popup_col as f32 + 1.0,
                            row as f32 - 0.15,
                            pane_w.saturating_sub(2) as f32,
                            1.18,
                            bg,
                            cell_w,
                            cell_h,
                            vp_w,
                            vp_h,
                        );
                        if entry.selected
                            && let Some(wpipe) = self.widget_pipelines.get("dropdown")
                        {
                            draw_widget_instances(
                                &enc,
                                &self.device,
                                &mut self.upload_arena,
                                &mut self.stats,
                                wpipe,
                                selected.as_slice(),
                            );
                        }
                        let atlas = self.atlas.as_mut().ok_or(BackendError::MetalError)?;
                        push_text_cells(
                            &mut popup_verts,
                            atlas,
                            &entry.label,
                            popup_col + 3,
                            row,
                            pane_w.saturating_sub(6),
                            pop_fg,
                            to_rgba(panel_bg),
                            cell_w,
                            cell_h,
                            vp_w,
                            vp_h,
                        );
                    }
                    if show_doc {
                        if let Some((title, _)) = &comp.doc {
                            let title_fg = to_rgba(theme::COMP_DOC_TITLE_FG());
                            let doc_fg = to_rgba(theme::COMP_DOC_FG());
                            let doc_bg_rgba = to_rgba(doc_bg);
                            let atlas = self.atlas.as_mut().ok_or(BackendError::MetalError)?;
                            push_text_cells(
                                &mut popup_verts,
                                atlas,
                                title,
                                doc_col + doc_pad_x,
                                panel_row + doc_pad_top,
                                doc_text_w,
                                title_fg,
                                doc_bg_rgba,
                                cell_w,
                                cell_h,
                                vp_w,
                                vp_h,
                            );
                            push_rect_px(
                                &mut popup_verts,
                                (doc_col + doc_pad_x) as f32 * cell_w,
                                (panel_row + doc_pad_top + 2) as f32 * cell_h - 2.0,
                                doc_text_w as f32 * cell_w,
                                1.0,
                                panel_border,
                                vp_w,
                                vp_h,
                            );
                            if let Some(lines) = &doc_body {
                                for (li, line) in lines.iter().enumerate() {
                                    let row = panel_row + doc_pad_top + 3 + li;
                                    if row >= panel_row + panel_h {
                                        break;
                                    }
                                    push_text_cells(
                                        &mut popup_verts,
                                        atlas,
                                        line,
                                        doc_col + doc_pad_x,
                                        row,
                                        doc_text_w,
                                        doc_fg,
                                        doc_bg_rgba,
                                        cell_w,
                                        cell_h,
                                        vp_w,
                                        vp_h,
                                    );
                                }
                            }
                            if doc_content_h == 0 {
                                push_text_cells(
                                    &mut popup_verts,
                                    atlas,
                                    "No documentation.",
                                    doc_col + doc_pad_x,
                                    panel_row + doc_pad_top + 3,
                                    doc_text_w,
                                    muted_fg,
                                    doc_bg_rgba,
                                    cell_w,
                                    cell_h,
                                    vp_w,
                                    vp_h,
                                );
                            }
                        }
                    }
                    draw_vertices(
                        &enc,
                        &self.device,
                        &mut self.upload_arena,
                        &mut self.stats,
                        &pipeline,
                        &atlas_texture,
                        &popup_verts,
                    );
                }
            }

            enc.endEncoding();
            cmdbuf.presentDrawable(objc2::runtime::ProtocolObject::from_ref(&*drawable));
            cmdbuf.commit();
            self.upload_arena.finish_frame(cmdbuf.clone());
            self.stats
                .note_frame(0, 0, 0, widget_scene_build_time, metal_prep_time);
            Ok(())
        }
    }

    impl Backend for MetalBackend {
        fn initialize(&mut self) -> Result<(), BackendError> {
            // ── Window ───────────────────────────────────────────────────────
            let event_loop = EventLoop::new().map_err(|_| BackendError::MetalError)?;
            let window = winit::window::WindowBuilder::new()
                .with_title("eseqlisp")
                .with_inner_size(self.initial_window_size)
                .with_visible(self.initial_window_visible)
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
            self.atlas = GlyphAtlas::new(
                &self.device,
                "JetBrainsMono-Regular",
                self.monospace_font_size_pt * scale,
            );
            self.prop_atlas = ProportionalGlyphAtlas::new(
                &self.device,
                DEFAULT_MONOSPACE_FONT_SIZE_PT * scale,
                scale,
            );
            self.mono_atlas_generation = self.mono_atlas_generation.wrapping_add(1);
            self.prop_atlas_generation = self.prop_atlas_generation.wrapping_add(1);
            self.compiled_widget_runs.clear();
            self.prop_text_layout_cache = ProportionalTextLayoutCache::new();

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
            attach.setBlendingEnabled(true);
            attach.setSourceRGBBlendFactor(MTLBlendFactor::SourceAlpha);
            attach.setDestinationRGBBlendFactor(MTLBlendFactor::OneMinusSourceAlpha);
            attach.setSourceAlphaBlendFactor(MTLBlendFactor::One);
            attach.setDestinationAlphaBlendFactor(MTLBlendFactor::OneMinusSourceAlpha);

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

            // ── Image pipeline ───────────────────────────────────────────────
            {
                let image_lib = self
                    .device
                    .newLibraryWithSource_options_error(&NSString::from_str(IMAGE_SHADER_SRC), None)
                    .map_err(|_| BackendError::MetalError)?;
                let image_vert = image_lib
                    .newFunctionWithName(&NSString::from_str("image_vert"))
                    .ok_or(BackendError::MetalError)?;
                let image_frag = image_lib
                    .newFunctionWithName(&NSString::from_str("image_frag"))
                    .ok_or(BackendError::MetalError)?;
                let image_desc = MTLRenderPipelineDescriptor::new();
                image_desc.setVertexFunction(Some(&image_vert));
                image_desc.setFragmentFunction(Some(&image_frag));
                let image_attach =
                    unsafe { image_desc.colorAttachments().objectAtIndexedSubscript(0) };
                image_attach.setPixelFormat(MTLPixelFormat::BGRA8Unorm);
                image_attach.setBlendingEnabled(true);
                image_attach.setSourceRGBBlendFactor(MTLBlendFactor::SourceAlpha);
                image_attach.setDestinationRGBBlendFactor(MTLBlendFactor::OneMinusSourceAlpha);
                image_attach.setSourceAlphaBlendFactor(MTLBlendFactor::One);
                image_attach.setDestinationAlphaBlendFactor(MTLBlendFactor::OneMinusSourceAlpha);
                self.image_pipeline = Some(
                    self.device
                        .newRenderPipelineStateWithDescriptor_error(&image_desc)
                        .map_err(|_| BackendError::MetalError)?,
                );
            }

            // ── Patch cable pipeline ────────────────────────────────────────
            {
                let cable_lib = self
                    .device
                    .newLibraryWithSource_options_error(
                        &NSString::from_str(PATCH_CABLE_SHADER_SRC),
                        None,
                    )
                    .map_err(|err| {
                        eprintln!("Metal patch cable shader compile failed: {err:?}");
                        BackendError::MetalError
                    })?;
                let cable_vert = cable_lib
                    .newFunctionWithName(&NSString::from_str("patch_cable_vert"))
                    .ok_or(BackendError::MetalError)?;
                let cable_frag = cable_lib
                    .newFunctionWithName(&NSString::from_str("patch_cable_frag"))
                    .ok_or(BackendError::MetalError)?;
                let cable_desc = MTLRenderPipelineDescriptor::new();
                cable_desc.setVertexFunction(Some(&cable_vert));
                cable_desc.setFragmentFunction(Some(&cable_frag));
                let cable_attach =
                    unsafe { cable_desc.colorAttachments().objectAtIndexedSubscript(0) };
                cable_attach.setPixelFormat(MTLPixelFormat::BGRA8Unorm);
                cable_attach.setBlendingEnabled(true);
                cable_attach.setSourceRGBBlendFactor(MTLBlendFactor::SourceAlpha);
                cable_attach.setDestinationRGBBlendFactor(MTLBlendFactor::OneMinusSourceAlpha);
                cable_attach.setSourceAlphaBlendFactor(MTLBlendFactor::One);
                cable_attach.setDestinationAlphaBlendFactor(MTLBlendFactor::OneMinusSourceAlpha);
                self.patch_cable_pipeline = Some(
                    self.device
                        .newRenderPipelineStateWithDescriptor_error(&cable_desc)
                        .map_err(|err| {
                            eprintln!("Metal patch cable pipeline creation failed: {err:?}");
                            BackendError::MetalError
                        })?,
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
                    .map_err(|err| {
                        eprintln!("Metal widget shader compile failed for {widget_type}: {err:?}");
                        BackendError::MetalError
                    })?;

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
                    .map_err(|err| {
                        eprintln!(
                            "Metal widget pipeline creation failed for {widget_type}: {err:?}"
                        );
                        BackendError::MetalError
                    })?;
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
            let suppress_scroll_until = &mut self.suppress_scroll_until;
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
            let wake_at = Instant::now() + timeout;
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
                        match kev.state {
                            ElementState::Pressed => {
                                if let Some(ev) =
                                    translate_key(&kev.logical_key, &kev.physical_key, *modifiers)
                                {
                                    pending.push_back(ev);
                                }
                            }
                            ElementState::Released => {
                                // Emit Release events for note-off handling in sequencer
                                if let Some(ev) = translate_key_with_state(
                                    &kev.logical_key,
                                    &kev.physical_key,
                                    *modifiers,
                                    kev.state,
                                ) {
                                    pending.push_back(ev);
                                }
                            }
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
                                // Clear stale drag so it doesn't fire after the up
                                *pending_drag = None;
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
                        if let Some(until) = *suppress_scroll_until {
                            if Instant::now() < until {
                                return;
                            }
                            *suppress_scroll_until = None;
                        }
                        match delta {
                            MouseScrollDelta::LineDelta(x, y) => {
                                // Convert line deltas to pixel deltas and route through
                                // the unified pending_scroll path. This allows lib.rs to
                                // apply smooth sub-cell scrolling in UI mode, while still
                                // quantizing to cell steps in text mode.
                                let line_h = (cell_size.1 as f32).max(20.0);
                                pending_scroll.push_back(((x * line_h, y * line_h), *cursor_pos));
                            }
                            MouseScrollDelta::PixelDelta(delta) => {
                                pending_scroll
                                    .push_back(((delta.x as f32, delta.y as f32), *cursor_pos));
                            }
                        };
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
            let mut widget_scene_build_time = Duration::ZERO;
            let mut metal_prep_time = Duration::ZERO;
            let mut image_load_budget = 1usize;
            self.drain_decoded_images(2);

            self.compile_pending_sdf_pipelines();

            let Some(pipeline) = self.pipeline.clone() else {
                return Ok(());
            };
            let Some((cell_w, cell_h)) = self
                .atlas
                .as_ref()
                .map(|atlas| (atlas.cell_w as f32, atlas.cell_h as f32))
            else {
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
            self.upload_arena.begin_frame(&mut self.stats);
            self.prop_text_layout_cache.begin_frame();
            self.begin_compiled_widget_run_frame();
            // ── Build/cached text vertex data ───────────────────────────────
            let mut text_upload_bytes = 0;
            let max_rows_exact = (vp_h / cell_h - 1.0).max(0.0);
            let max_rows = max_rows_exact.floor() as u16;
            let primitive_scene = frame
                .widget_layout
                .as_ref()
                .map(|layout| {
                    let started = Instant::now();
                    let scene = self.widget_scene_for_layout(
                        frame.widget_content_cache_key,
                        frame.widget_layout_cache_key,
                        layout,
                        &frame.dirty_widget_ids,
                        WidgetViewport {
                            cell_w,
                            cell_h,
                            vp_w,
                            vp_h,
                            time_seconds,
                            focused_widget_id: frame.focused_widget_id,
                            focused_branch: false,
                            tile_content_rows: max_rows_exact,
                            scroll_top: frame.widget_scroll_top,
                            scroll_left: frame.widget_scroll_left,
                            inherited_hover: false,
                        },
                        frame.widget_scroll_top,
                        max_rows,
                    );
                    widget_scene_build_time += started.elapsed();
                    scene
                })
                .unwrap_or_default();

            let Some(atlas) = &mut self.atlas else {
                return Ok(());
            };
            let atlas_texture = atlas.texture.clone();
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
            let prep_started = Instant::now();
            let primitive_quads = build_widget_primitive_quads(&primitive_scene, atlas, vp_w, vp_h);
            let (primitive_bg_runs, primitive_instance_runs) =
                partition_widget_instance_runs(&primitive_scene);
            metal_prep_time += prep_started.elapsed();
            let _ = atlas;

            // ── Vertex buffer ────────────────────────────────────────────────
            let text_vbuf = self.cached_text_buffer.clone();
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

            // Background SDF widgets (behind text)
            for (widget_type, instances) in &primitive_bg_runs {
                let Some(wpipe) = self.widget_pipelines.get(widget_type) else {
                    continue;
                };
                if instances.is_empty() {
                    continue;
                }
                draw_widget_instances(
                    &enc,
                    &self.device,
                    &mut self.upload_arena,
                    &mut self.stats,
                    wpipe,
                    instances.as_slice(),
                );
            }

            if let Some(image_pipeline) = self.image_pipeline.clone() {
                let images = collect_image_primitives(&primitive_scene);
                self.draw_image_primitives(
                    &enc,
                    &image_pipeline,
                    &images,
                    None,
                    &mut image_load_budget,
                    cell_w,
                    cell_h,
                    vp_w,
                    vp_h,
                    time_seconds,
                );
            }

            if let Some(vbuf) = &text_vbuf {
                enc.setRenderPipelineState(&pipeline);
                unsafe {
                    enc.setVertexBuffer_offset_atIndex(Some(vbuf), 0, 0);
                    enc.setFragmentTexture_atIndex(Some(&atlas_texture), 0);
                    enc.drawPrimitives_vertexStart_vertexCount(
                        MTLPrimitiveType::Triangle,
                        0,
                        self.cached_text_vertex_count as _,
                    );
                }
            }

            draw_vertices(
                &enc,
                &self.device,
                &mut self.upload_arena,
                &mut self.stats,
                &pipeline,
                &atlas_texture,
                primitive_quads.as_slice(),
            );

            if let Some(cable_pipeline) = self.patch_cable_pipeline.clone() {
                let clip = MTLScissorRect {
                    x: 0,
                    y: 0,
                    width: texture.width(),
                    height: texture.height(),
                };
                let cables = collect_patch_cable_primitives(
                    &primitive_scene,
                    clip,
                    cell_w,
                    cell_h,
                    vp_w,
                    vp_h,
                );
                draw_patch_cable_instances(
                    &enc,
                    &self.device,
                    &mut self.upload_arena,
                    &mut self.stats,
                    &cable_pipeline,
                    &cables,
                );
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
                draw_widget_instances(
                    &enc,
                    &self.device,
                    &mut self.upload_arena,
                    &mut self.stats,
                    wpipe,
                    instances.as_slice(),
                );
            }

            let circle_quads = build_circle_quads(&primitive_scene, cell_w, cell_h, vp_w, vp_h);
            draw_vertices(
                &enc,
                &self.device,
                &mut self.upload_arena,
                &mut self.stats,
                &pipeline,
                &atlas_texture,
                circle_quads.as_slice(),
            );

            let foreground_rect_quads =
                build_foreground_rect_quads(&primitive_scene, cell_w, cell_h, vp_w, vp_h);
            draw_vertices(
                &enc,
                &self.device,
                &mut self.upload_arena,
                &mut self.stats,
                &pipeline,
                &atlas_texture,
                foreground_rect_quads.as_slice(),
            );

            // Proportional text: separate atlas + linear-filtering pipeline.
            if let (Some(prop_atlas), Some(prop_pipe)) =
                (self.prop_atlas.as_mut(), self.prop_pipeline.as_ref())
            {
                let prop_started = Instant::now();
                let prop_verts = build_proportional_text_quads_cached(
                    &primitive_scene,
                    prop_atlas,
                    &mut self.prop_text_layout_cache,
                    &mut self.stats,
                    cell_w,
                    cell_h,
                    vp_w,
                    vp_h,
                );
                metal_prep_time += prop_started.elapsed();
                draw_vertices(
                    &enc,
                    &self.device,
                    &mut self.upload_arena,
                    &mut self.stats,
                    prop_pipe,
                    &prop_atlas.texture,
                    prop_verts.as_slice(),
                );
            }

            let text_bytes = text_upload_bytes;
            let label_bytes = primitive_quads.len() * std::mem::size_of::<Vertex>();
            let widget_bytes = widget_upload_bytes;

            enc.endEncoding();
            buf.presentDrawable(objc2::runtime::ProtocolObject::from_ref(&*drawable));
            buf.commit();
            self.upload_arena.finish_frame(buf.clone());
            crate::widget_render::sdf_widget::note_sdf_frame_presented(time_seconds);
            if crate::widget_render::sdf_widget::sdf_visual_animations_active(time_seconds)
                && let Some(window) = self.window.as_ref()
            {
                window.request_redraw();
            }
            self.stats.note_frame(
                text_bytes,
                label_bytes,
                widget_bytes,
                widget_scene_build_time,
                metal_prep_time,
            );
            Ok(())
        }
    }

    struct RenderStats {
        enabled: bool,
        window_start: Instant,
        frames: u64,
        text_bytes: usize,
        label_bytes: usize,
        widget_bytes: usize,
        widget_scene_build: Duration,
        metal_prep: Duration,
        widget_scene_cache_hits: u64,
        widget_scene_cache_misses: u64,
        widget_scene_cache_dirty_bypasses: u64,
        widget_scene_cache_overlay_bypasses: u64,
        widget_scene_cache_clears: u64,
        widget_scene_dirty_widget_ids: u64,
        widget_scene_miss_cold: u64,
        widget_scene_miss_content: u64,
        widget_scene_miss_layout: u64,
        widget_scene_miss_widget_state: u64,
        widget_scene_miss_theme: u64,
        widget_scene_miss_focus: u64,
        widget_scene_miss_scroll: u64,
        widget_scene_miss_viewport: u64,
        prop_text_runs: u64,
        prop_text_glyphs: u64,
        prop_text_quads: u64,
        prop_text_cache_hits: u64,
        prop_text_cache_misses: u64,
        widget_primitives: u64,
        widget_segments: u64,
        draw_commands: u64,
        upload_bytes: usize,
        upload_buffer_allocations: u64,
        upload_buffer_allocated_bytes: usize,
        upload_frame_grows: u64,
        widget_run_cache_hits: u64,
        widget_run_cache_misses: u64,
        widget_run_cache_dirty_bypasses: u64,
        widget_run_cache_unsupported_bypasses: u64,
        widget_run_cache_complex_bypasses: u64,
        widget_run_cache_clears: u64,
        widget_run_cached_draws: u64,
        widget_run_dynamic_draws: u64,
        widget_run_static_allocations: u64,
        widget_run_static_allocated_bytes: usize,
        retained_run_collection_misses: u64,
        retained_run_reuses: u64,
        retained_run_rebuilds: u64,
        retained_run_missing_previous: u64,
        retained_run_invalid_previous: u64,
    }

    impl RenderStats {
        fn new() -> Self {
            Self {
                enabled: std::env::var_os("ESEQLISP_PROFILE_UI").is_some(),
                window_start: Instant::now(),
                frames: 0,
                text_bytes: 0,
                label_bytes: 0,
                widget_bytes: 0,
                widget_scene_build: Duration::ZERO,
                metal_prep: Duration::ZERO,
                widget_scene_cache_hits: 0,
                widget_scene_cache_misses: 0,
                widget_scene_cache_dirty_bypasses: 0,
                widget_scene_cache_overlay_bypasses: 0,
                widget_scene_cache_clears: 0,
                widget_scene_dirty_widget_ids: 0,
                widget_scene_miss_cold: 0,
                widget_scene_miss_content: 0,
                widget_scene_miss_layout: 0,
                widget_scene_miss_widget_state: 0,
                widget_scene_miss_theme: 0,
                widget_scene_miss_focus: 0,
                widget_scene_miss_scroll: 0,
                widget_scene_miss_viewport: 0,
                prop_text_runs: 0,
                prop_text_glyphs: 0,
                prop_text_quads: 0,
                prop_text_cache_hits: 0,
                prop_text_cache_misses: 0,
                widget_primitives: 0,
                widget_segments: 0,
                draw_commands: 0,
                upload_bytes: 0,
                upload_buffer_allocations: 0,
                upload_buffer_allocated_bytes: 0,
                upload_frame_grows: 0,
                widget_run_cache_hits: 0,
                widget_run_cache_misses: 0,
                widget_run_cache_dirty_bypasses: 0,
                widget_run_cache_unsupported_bypasses: 0,
                widget_run_cache_complex_bypasses: 0,
                widget_run_cache_clears: 0,
                widget_run_cached_draws: 0,
                widget_run_dynamic_draws: 0,
                widget_run_static_allocations: 0,
                widget_run_static_allocated_bytes: 0,
                retained_run_collection_misses: 0,
                retained_run_reuses: 0,
                retained_run_rebuilds: 0,
                retained_run_missing_previous: 0,
                retained_run_invalid_previous: 0,
            }
        }

        fn note_widget_scene_cache_hit(&mut self) {
            self.widget_scene_cache_hits += 1;
        }

        fn note_widget_scene_cache_miss(
            &mut self,
            previous: Option<WidgetSceneCacheKey>,
            current: WidgetSceneCacheKey,
        ) {
            self.widget_scene_cache_misses += 1;
            let Some(previous) = previous else {
                self.widget_scene_miss_cold += 1;
                return;
            };

            if previous.owner_frame_key != current.owner_frame_key {
                self.widget_scene_miss_content += 1;
            } else if previous.layout_cache_key != current.layout_cache_key
                || previous.layout_identity != current.layout_identity
            {
                self.widget_scene_miss_layout += 1;
            } else if previous.widget_state_generation != current.widget_state_generation {
                self.widget_scene_miss_widget_state += 1;
            } else if previous.theme_generation != current.theme_generation {
                self.widget_scene_miss_theme += 1;
            } else if previous.focused_widget_id != current.focused_widget_id {
                self.widget_scene_miss_focus += 1;
            } else if previous.scroll_top_bits != current.scroll_top_bits {
                self.widget_scene_miss_scroll += 1;
            } else if previous.max_rows != current.max_rows
                || previous.cell_w_bits != current.cell_w_bits
                || previous.cell_h_bits != current.cell_h_bits
                || previous.vp_w_bits != current.vp_w_bits
                || previous.vp_h_bits != current.vp_h_bits
                || previous.tile_content_rows_bits != current.tile_content_rows_bits
            {
                self.widget_scene_miss_viewport += 1;
            }
        }

        fn note_widget_scene_dirty_bypass(&mut self, dirty_widget_id_count: usize) {
            self.widget_scene_cache_dirty_bypasses += 1;
            self.widget_scene_dirty_widget_ids += dirty_widget_id_count as u64;
        }

        fn note_widget_scene_overlay_bypass(&mut self) {
            self.widget_scene_cache_overlay_bypasses += 1;
        }

        fn note_widget_scene_cache_clear(&mut self) {
            self.widget_scene_cache_clears += 1;
        }

        fn note_prop_text_cache_hit(&mut self) {
            self.prop_text_cache_hits += 1;
        }

        fn note_prop_text_cache_miss(&mut self) {
            self.prop_text_cache_misses += 1;
        }

        fn note_prop_text_run(&mut self, glyphs: usize, quads: usize) {
            self.prop_text_runs += 1;
            self.prop_text_glyphs += glyphs as u64;
            self.prop_text_quads += quads as u64;
        }

        fn note_widget_primitives(&mut self, count: usize) {
            self.widget_primitives += count as u64;
        }

        fn note_widget_segments(&mut self, count: usize) {
            self.widget_segments += count as u64;
        }

        fn note_draw_command(&mut self) {
            self.draw_commands += 1;
        }

        fn note_upload_bytes(&mut self, bytes: usize) {
            self.upload_bytes += bytes;
        }

        fn note_upload_buffer_allocation(&mut self, bytes: usize) {
            self.upload_buffer_allocations += 1;
            self.upload_buffer_allocated_bytes += bytes;
        }

        fn note_upload_frame_grow(&mut self) {
            self.upload_frame_grows += 1;
        }

        fn note_widget_run_cache_hit(&mut self) {
            self.widget_run_cache_hits += 1;
        }

        fn note_widget_run_cache_miss(&mut self) {
            self.widget_run_cache_misses += 1;
        }

        fn note_widget_run_cache_bypass_dirty(&mut self) {
            self.widget_run_cache_dirty_bypasses += 1;
        }

        fn note_widget_run_cache_bypass_unsupported(&mut self) {
            self.widget_run_cache_unsupported_bypasses += 1;
        }

        fn note_widget_run_cache_bypass_complex(&mut self) {
            self.widget_run_cache_complex_bypasses += 1;
        }

        fn note_widget_run_cache_clear(&mut self) {
            self.widget_run_cache_clears += 1;
        }

        fn note_widget_run_cached_draw(&mut self) {
            self.widget_run_cached_draws += 1;
        }

        fn note_widget_run_dynamic_draw(&mut self) {
            self.widget_run_dynamic_draws += 1;
        }

        fn note_widget_run_static_allocation(&mut self, bytes: usize) {
            self.widget_run_static_allocations += 1;
            self.widget_run_static_allocated_bytes += bytes;
        }

        fn note_widget_retained_run_collection(
            &mut self,
            reused_runs: usize,
            rebuilt_runs: usize,
            missing_previous_runs: usize,
            invalid_previous_runs: usize,
        ) {
            self.retained_run_reuses += reused_runs as u64;
            self.retained_run_rebuilds += rebuilt_runs as u64;
            self.retained_run_missing_previous += missing_previous_runs as u64;
            self.retained_run_invalid_previous += invalid_previous_runs as u64;
        }

        fn note_widget_retained_run_collection_miss(&mut self) {
            self.retained_run_collection_misses += 1;
        }

        fn note_frame(
            &mut self,
            text_bytes: usize,
            label_bytes: usize,
            widget_bytes: usize,
            widget_scene_build: Duration,
            metal_prep: Duration,
        ) {
            self.frames += 1;
            self.text_bytes += text_bytes;
            self.label_bytes += label_bytes;
            self.widget_bytes += widget_bytes;
            self.widget_scene_build += widget_scene_build;
            self.metal_prep += metal_prep;

            let elapsed = self.window_start.elapsed();
            if elapsed.as_secs_f64() < 1.0 {
                return;
            }

            let secs = elapsed.as_secs_f64();
            let fps = self.frames as f64 / secs;
            let total_mb =
                (self.text_bytes + self.label_bytes + self.widget_bytes) as f64 / (1024.0 * 1024.0);
            let mbps = total_mb / secs;
            if self.enabled {
                let scene_cache_attempts =
                    self.widget_scene_cache_hits + self.widget_scene_cache_misses;
                let scene_cache_hit_pct = if scene_cache_attempts == 0 {
                    0.0
                } else {
                    self.widget_scene_cache_hits as f64 * 100.0 / scene_cache_attempts as f64
                };
                eprintln!(
                    "[ui-profile][metal] fps={fps:.1} scene_avg={:.2}ms prep_avg={:.2}ms upload={mbps:.2}MB/s text={:.2}MB/s labels={:.2}MB/s widgets={:.2}MB/s arena_upload={:.2}MB/s arena_allocs:{} arena_alloc_mb:{:.2} arena_frame_grows:{} draws:{} prims:{} segments:{} run_cache=hit:{}/miss:{} cached:{} dynamic:{} bypass_dirty:{} bypass_unsupported:{} bypass_complex:{} clear:{} static_allocs:{} static_alloc_mb:{:.2} retained_runs=reuse:{} rebuild:{} miss:{} missing_prev:{} invalid_prev:{} prop_runs:{} prop_glyphs:{} prop_quads:{} prop_cache=hit:{}/miss:{} scene_cache=hit:{}/miss:{}({scene_cache_hit_pct:.1}%) dirty:{} dirty_ids:{} overlay:{} clear:{} miss_reason=cold:{} content:{} layout:{} widget_state:{} theme:{} focus:{} scroll:{} viewport:{}",
                    self.widget_scene_build.as_secs_f64() * 1000.0 / self.frames as f64,
                    self.metal_prep.as_secs_f64() * 1000.0 / self.frames as f64,
                    self.text_bytes as f64 / (1024.0 * 1024.0) / secs,
                    self.label_bytes as f64 / (1024.0 * 1024.0) / secs,
                    self.widget_bytes as f64 / (1024.0 * 1024.0) / secs,
                    self.upload_bytes as f64 / (1024.0 * 1024.0) / secs,
                    self.upload_buffer_allocations,
                    self.upload_buffer_allocated_bytes as f64 / (1024.0 * 1024.0),
                    self.upload_frame_grows,
                    self.draw_commands,
                    self.widget_primitives,
                    self.widget_segments,
                    self.widget_run_cache_hits,
                    self.widget_run_cache_misses,
                    self.widget_run_cached_draws,
                    self.widget_run_dynamic_draws,
                    self.widget_run_cache_dirty_bypasses,
                    self.widget_run_cache_unsupported_bypasses,
                    self.widget_run_cache_complex_bypasses,
                    self.widget_run_cache_clears,
                    self.widget_run_static_allocations,
                    self.widget_run_static_allocated_bytes as f64 / (1024.0 * 1024.0),
                    self.retained_run_reuses,
                    self.retained_run_rebuilds,
                    self.retained_run_collection_misses,
                    self.retained_run_missing_previous,
                    self.retained_run_invalid_previous,
                    self.prop_text_runs,
                    self.prop_text_glyphs,
                    self.prop_text_quads,
                    self.prop_text_cache_hits,
                    self.prop_text_cache_misses,
                    self.widget_scene_cache_hits,
                    self.widget_scene_cache_misses,
                    self.widget_scene_cache_dirty_bypasses,
                    self.widget_scene_dirty_widget_ids,
                    self.widget_scene_cache_overlay_bypasses,
                    self.widget_scene_cache_clears,
                    self.widget_scene_miss_cold,
                    self.widget_scene_miss_content,
                    self.widget_scene_miss_layout,
                    self.widget_scene_miss_widget_state,
                    self.widget_scene_miss_theme,
                    self.widget_scene_miss_focus,
                    self.widget_scene_miss_scroll,
                    self.widget_scene_miss_viewport,
                );
            }

            self.window_start = Instant::now();
            self.frames = 0;
            self.text_bytes = 0;
            self.label_bytes = 0;
            self.widget_bytes = 0;
            self.widget_scene_build = Duration::ZERO;
            self.metal_prep = Duration::ZERO;
            self.widget_scene_cache_hits = 0;
            self.widget_scene_cache_misses = 0;
            self.widget_scene_cache_dirty_bypasses = 0;
            self.widget_scene_cache_overlay_bypasses = 0;
            self.widget_scene_cache_clears = 0;
            self.widget_scene_dirty_widget_ids = 0;
            self.widget_scene_miss_cold = 0;
            self.widget_scene_miss_content = 0;
            self.widget_scene_miss_layout = 0;
            self.widget_scene_miss_widget_state = 0;
            self.widget_scene_miss_theme = 0;
            self.widget_scene_miss_focus = 0;
            self.widget_scene_miss_scroll = 0;
            self.widget_scene_miss_viewport = 0;
            self.prop_text_runs = 0;
            self.prop_text_glyphs = 0;
            self.prop_text_quads = 0;
            self.prop_text_cache_hits = 0;
            self.prop_text_cache_misses = 0;
            self.widget_primitives = 0;
            self.widget_segments = 0;
            self.draw_commands = 0;
            self.upload_bytes = 0;
            self.upload_buffer_allocations = 0;
            self.upload_buffer_allocated_bytes = 0;
            self.upload_frame_grows = 0;
            self.widget_run_cache_hits = 0;
            self.widget_run_cache_misses = 0;
            self.widget_run_cache_dirty_bypasses = 0;
            self.widget_run_cache_unsupported_bypasses = 0;
            self.widget_run_cache_complex_bypasses = 0;
            self.widget_run_cache_clears = 0;
            self.widget_run_cached_draws = 0;
            self.widget_run_dynamic_draws = 0;
            self.widget_run_static_allocations = 0;
            self.widget_run_static_allocated_bytes = 0;
            self.retained_run_collection_misses = 0;
            self.retained_run_reuses = 0;
            self.retained_run_rebuilds = 0;
            self.retained_run_missing_previous = 0;
            self.retained_run_invalid_previous = 0;
        }
    }

    fn rasterize_char(
        atlas: &mut GlyphAtlas,
        ch: char,
        (col, row): (f32, f32),
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
        let x0 = ndc_x(col * ctx.cell_w);
        let x1 = ndc_x((col + 1.0) * ctx.cell_w);
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
    fn build_proportional_text_quads_cached(
        primitives: &[widget_render::MetalPrimitive],
        prop_atlas: &mut ProportionalGlyphAtlas,
        layout_cache: &mut ProportionalTextLayoutCache,
        stats: &mut RenderStats,
        mono_cell_w: f32,
        mono_cell_h: f32,
        vp_w: f32,
        vp_h: f32,
    ) -> Vec<Vertex> {
        let mut verts = Vec::new();
        let ndc_x = |px: f32| px / vp_w * 2.0 - 1.0;
        let ndc_y = |px: f32| 1.0 - px / vp_h * 2.0;

        for prim in primitives {
            let widget_render::MetalPrimitive::ProportionalText(run) =
                widget_render::innermost_primitive(prim)
            else {
                continue;
            };

            let scale = run.scale.max(0.001);
            let fg = run.fg.to_rgba();
            let bg = [0.0, 0.0, 0.0, 0.0]; // Transparent — alpha blending handles bg
            let vertex_key =
                ProportionalTextVertexKey::new(run, mono_cell_w, mono_cell_h, vp_w, vp_h);
            if let Some(cached) = layout_cache.vertex_runs.get_mut(&vertex_key) {
                cached.last_used_frame = layout_cache.frame_index;
                verts.extend(cached.vertices.iter().cloned());
                stats.note_prop_text_cache_hit();
                stats.note_prop_text_run(cached.glyph_count, cached.quad_count);
                continue;
            }
            stats.note_prop_text_cache_miss();

            let Some(layout) = layout_cache.layout_for_run(run, prop_atlas) else {
                continue;
            };
            let run_start = verts.len();

            let text_width_px = layout.text_width_px * scale;
            let align_extra_px = if run.align_width > 0.0 {
                (run.align_width * mono_cell_w - text_width_px).max(0.0)
                    * run.h_align.clamp(0.0, 1.0)
            } else {
                0.0
            };
            let base_x_px = run.col * mono_cell_w + align_extra_px;
            let base_y_px = run.row * mono_cell_h;

            // Vertical centering: offset glyph bitmap so it's centered within
            // one mono cell height (widgets center text assuming 1.0 cell units).
            let y_offset = (mono_cell_h - layout.line_height_px) * 0.5 * scale;
            let mut run_quads = 0;

            for glyph in &layout.glyphs {
                if glyph.raster_w == 0 || glyph.raster_h == 0 {
                    continue;
                }

                let [u0, v0] = glyph.uv_min;
                let [u1, v1] = glyph.uv_max;

                // Glyph bitmap starts 2px before pen (padding), spans full line height.
                let gx0 = base_x_px + glyph.pen_x * scale - 2.0 * scale;
                let gy0 = base_y_px + y_offset;
                let gx1 = gx0 + glyph.raster_w as f32 * scale;
                let gy1 = gy0 + glyph.raster_h as f32 * scale;

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
                run_quads += 1;
            }
            let glyph_count = layout.glyphs.len();
            stats.note_prop_text_run(glyph_count, run_quads);
            layout_cache.vertex_runs.insert(
                vertex_key,
                CachedProportionalTextVertices {
                    vertices: verts[run_start..].to_vec(),
                    glyph_count,
                    quad_count: run_quads,
                    last_used_frame: layout_cache.frame_index,
                },
            );
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
        build_text_quads_offset(frame, atlas, vp_w, vp_h, TileOffset::default(), theme::BG())
    }

    fn build_text_quads_offset(
        frame: &RenderFrame,
        atlas: &mut GlyphAtlas,
        vp_w: f32,
        vp_h: f32,
        offset: TileOffset,
        default_bg: Color,
    ) -> Vec<Vertex> {
        let cell_w = atlas.cell_w as f32;
        let cell_h = atlas.cell_h as f32;
        let mut verts = Vec::with_capacity(frame.lines.len() * 80 * 6);

        let ndc_x = |px: f32| px / vp_w * 2.0 - 1.0;
        let ndc_y = |px: f32| 1.0 - px / vp_h * 2.0;
        let to_rgba = |c: Color| [c.r, c.g, c.b, c.a];

        for (row, line) in frame.lines.iter().enumerate() {
            for (col, cell) in line.iter().enumerate() {
                let abs_col = col as f32 + offset.col;
                let abs_row = row as f32 + offset.row;
                let is_cursor = frame.cursor == Some((row, col));

                let x0 = ndc_x(abs_col * cell_w);
                let x1 = ndc_x((abs_col + 1.0) * cell_w);
                let y0 = ndc_y(abs_row * cell_h);
                let y1 = ndc_y((abs_row + 1.0) * cell_h);

                // Use a dedicated cursor fill so it stays legible over selection and syntax colors.
                let (fg, bg) = if is_cursor {
                    let cursor_bg = theme::CURSOR();
                    let cursor_fg = if cursor_bg.luma() >= 0.55 {
                        theme::BG()
                    } else {
                        theme::FG()
                    };
                    (to_rgba(cursor_fg), to_rgba(cursor_bg))
                } else {
                    (
                        to_rgba(cell.style.fg),
                        to_rgba(cell.style.bg.unwrap_or(default_bg)),
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
                    (abs_col, abs_row),
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
        let status_row = if offset.col == 0.0 && offset.row == 0.0 {
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
                        (ch_col as f32, ch_row as f32),
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
                            ((doc_col + j) as f32, title_row as f32),
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
                            ((doc_col + j) as f32, doc_row as f32),
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
                (col as f32, status_row as f32),
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
            match widget_render::innermost_primitive(primitive) {
                widget_render::MetalPrimitive::Rect(rect) => {
                    push_solid_rect_vertices(
                        rect.rect, rect.color, cell_w, cell_h, vp_w, vp_h, &mut verts,
                    );
                }
                widget_render::MetalPrimitive::ForegroundRect(_) => {}
                widget_render::MetalPrimitive::Quad(quad) => {
                    push_solid_quad_vertices(*quad, cell_w, cell_h, vp_w, vp_h, &mut verts);
                }
                widget_render::MetalPrimitive::Triangle(triangle) => {
                    push_solid_triangle_vertices(*triangle, cell_w, cell_h, vp_w, vp_h, &mut verts);
                }
                widget_render::MetalPrimitive::GlyphRun(run) => {
                    for (idx, ch) in run.text.chars().enumerate() {
                        if ch == ' ' {
                            continue;
                        }
                        rasterize_char(
                            atlas,
                            ch,
                            ((run.col + idx as i32) as f32, run.row),
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
                widget_render::MetalPrimitive::PatchCable(_) => {}
                widget_render::MetalPrimitive::Circle(_) => {}
                widget_render::MetalPrimitive::Waveform(_) => {}
                widget_render::MetalPrimitive::Image(_) => {}
                widget_render::MetalPrimitive::WidgetInstance { .. } => {}
                widget_render::MetalPrimitive::PushClipRect(_)
                | widget_render::MetalPrimitive::PopClipRect
                | widget_render::MetalPrimitive::ZLayer { .. } => {}
            }
        }
        verts
    }

    fn build_foreground_rect_quads(
        primitives: &[widget_render::MetalPrimitive],
        cell_w: f32,
        cell_h: f32,
        vp_w: f32,
        vp_h: f32,
    ) -> Vec<Vertex> {
        let mut verts = Vec::new();
        for primitive in primitives {
            let widget_render::MetalPrimitive::ForegroundRect(rect) =
                widget_render::innermost_primitive(primitive)
            else {
                continue;
            };
            push_solid_rect_vertices(
                rect.rect, rect.color, cell_w, cell_h, vp_w, vp_h, &mut verts,
            );
        }
        verts
    }

    fn build_circle_quads(
        primitives: &[widget_render::MetalPrimitive],
        cell_w: f32,
        cell_h: f32,
        vp_w: f32,
        vp_h: f32,
    ) -> Vec<Vertex> {
        let mut verts = Vec::new();
        for primitive in primitives {
            let widget_render::MetalPrimitive::Circle(circle) =
                widget_render::innermost_primitive(primitive)
            else {
                continue;
            };
            push_circle_fill_px(
                &mut verts,
                circle.center[0] * cell_w,
                circle.center[1] * cell_h,
                circle.radius_px,
                circle.color,
                circle.visible_half,
                vp_w,
                vp_h,
            );
        }
        verts
    }

    fn collect_image_primitives(
        primitives: &[widget_render::MetalPrimitive],
    ) -> Vec<widget_render::MetalImagePrimitive> {
        primitives
            .iter()
            .filter_map(
                |primitive| match widget_render::innermost_primitive(primitive) {
                    widget_render::MetalPrimitive::Image(image) => Some(image.clone()),
                    _ => None,
                },
            )
            .collect()
    }

    fn collect_waveform_primitives(
        primitives: &[widget_render::MetalPrimitive],
    ) -> Vec<widget_render::MetalWaveformPrimitive> {
        primitives
            .iter()
            .filter_map(
                |primitive| match widget_render::innermost_primitive(primitive) {
                    widget_render::MetalPrimitive::Waveform(waveform) => Some(waveform.clone()),
                    _ => None,
                },
            )
            .collect()
    }

    fn collect_patch_cable_primitives(
        primitives: &[widget_render::MetalPrimitive],
        clip: MTLScissorRect,
        cell_w: f32,
        cell_h: f32,
        vp_w: f32,
        vp_h: f32,
    ) -> Vec<PatchCableDrawInstance> {
        primitives
            .iter()
            .filter_map(
                |primitive| match widget_render::innermost_primitive(primitive) {
                    widget_render::MetalPrimitive::PatchCable(cable) => {
                        patch_cable_draw_instance_from_primitive(
                            cable, clip, cell_w, cell_h, vp_w, vp_h,
                        )
                    }
                    _ => None,
                },
            )
            .collect()
    }

    fn patch_cable_draw_instance_from_primitive(
        cable: &widget_render::MetalPatchCablePrimitive,
        clip: MTLScissorRect,
        cell_w: f32,
        cell_h: f32,
        vp_w: f32,
        vp_h: f32,
    ) -> Option<PatchCableDrawInstance> {
        let start = (cable.start[0] * cell_w, cable.start[1] * cell_h);
        let c1 = (cable.control1[0] * cell_w, cable.control1[1] * cell_h);
        let c2 = (cable.control2[0] * cell_w, cable.control2[1] * cell_h);
        let end = (cable.end[0] * cell_w, cable.end[1] * cell_h);
        let segment_y_px = cable.segment_row * cell_h;
        let corner_radius_px = cable.corner_radius_cells * cell_w.min(cell_h);
        let padding = cable.radius_px + 16.0;
        let (min_x, max_x, min_y, max_y) = if cable.is_segmented {
            let needs_five = end.1 < segment_y_px;
            if needs_five {
                let clearance = corner_radius_px * 2.0;
                let turnaround_y = end.1 - clearance;
                let turnaround_x = end.0 - clearance;
                (
                    start.0.min(end.0).min(turnaround_x) - padding,
                    start.0.max(end.0).max(turnaround_x) + padding,
                    start.1.min(end.1).min(segment_y_px).min(turnaround_y) - padding,
                    start.1.max(end.1).max(segment_y_px).max(turnaround_y) + padding,
                )
            } else {
                (
                    start.0.min(end.0) - padding,
                    start.0.max(end.0) + padding,
                    start.1.min(end.1).min(segment_y_px) - padding,
                    start.1.max(end.1).max(segment_y_px) + padding,
                )
            }
        } else {
            (
                start.0.min(end.0).min(c1.0).min(c2.0) - padding,
                start.0.max(end.0).max(c1.0).max(c2.0) + padding,
                start.1.min(end.1).min(c1.1).min(c2.1) - padding,
                start.1.max(end.1).max(c1.1).max(c2.1) + padding,
            )
        };
        if min_x >= vp_w || max_x <= 0.0 || min_y >= vp_h || max_y <= 0.0 {
            return None;
        }
        let ndc_x = |px: f32| px / vp_w * 2.0 - 1.0;
        let ndc_y = |px: f32| 1.0 - px / vp_h * 2.0;
        Some(PatchCableDrawInstance {
            instance: PatchCableInstance {
                ndc_min: [ndc_x(min_x), ndc_y(min_y)],
                ndc_max: [ndc_x(max_x), ndc_y(max_y)],
                bounds_min: [min_x, min_y],
                bounds_max: [max_x, max_y],
                start: [start.0, start.1],
                control1: [c1.0, c1.1],
                control2: [c2.0, c2.1],
                end: [end.0, end.1],
                color: cable.color.to_rgba(),
                radius_px: cable.radius_px,
                is_segmented: if cable.is_segmented { 1.0 } else { 0.0 },
                segment_y_px,
                corner_radius_px,
            },
            clip,
        })
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum ModPatchPortDirection {
        In,
        Out,
    }

    #[derive(Clone, Debug)]
    struct ModPatchPort {
        direction: ModPatchPortDirection,
        track: usize,
        input: usize,
        active: bool,
        pending: bool,
        center_px: (f32, f32),
        clip: MTLScissorRect,
        connected_sources: Vec<usize>,
        selected_sources: Vec<usize>,
    }

    fn collect_mod_patch_ports(
        node: &LayoutNode,
        col_off: f32,
        row_off: f32,
        cell_w: f32,
        cell_h: f32,
        visible_scissor: MTLScissorRect,
        out: &mut Vec<ModPatchPort>,
    ) {
        if layout_node_bool_prop(node, "patch-port") {
            if let (Some(direction), Some(track)) = (
                mod_patch_port_direction(node),
                layout_node_usize_prop(node, "track"),
            ) {
                let center_col = col_off + node.rect.col + node.rect.width * 0.5;
                let center_row = row_off + node.rect.row + node.rect.height * 0.5;
                let center_px = (center_col * cell_w, center_row * cell_h);
                if center_px.0.is_finite() && center_px.1.is_finite() {
                    out.push(ModPatchPort {
                        direction,
                        track,
                        input: layout_node_usize_prop(node, "input").unwrap_or(0),
                        active: layout_node_bool_prop(node, "active"),
                        pending: layout_node_bool_prop(node, "pending"),
                        center_px,
                        clip: visible_scissor,
                        connected_sources: layout_node_usize_list_prop(node, "connected-sources"),
                        selected_sources: layout_node_usize_list_prop(node, "selected-sources"),
                    });
                }
            }
        }

        for child in &node.children {
            collect_mod_patch_ports(
                child,
                col_off,
                row_off,
                cell_w,
                cell_h,
                visible_scissor,
                out,
            );
        }
    }

    fn mod_patch_port_direction(node: &LayoutNode) -> Option<ModPatchPortDirection> {
        match node.props.get("direction") {
            Some(Value::Keyword(value)) | Some(Value::String(value)) if value == "in" => {
                Some(ModPatchPortDirection::In)
            }
            Some(Value::Keyword(value)) | Some(Value::String(value)) if value == "out" => {
                Some(ModPatchPortDirection::Out)
            }
            _ => None,
        }
    }

    fn layout_node_usize_prop(node: &LayoutNode, key: &str) -> Option<usize> {
        match node.props.get(key) {
            Some(Value::Number(value)) if value.is_finite() && *value >= 0.0 => {
                Some(*value as usize)
            }
            _ => None,
        }
    }

    fn layout_node_bool_prop(node: &LayoutNode, key: &str) -> bool {
        matches!(node.props.get(key), Some(Value::Bool(true)))
    }

    fn layout_node_usize_list_prop(node: &LayoutNode, key: &str) -> Vec<usize> {
        let Some(Value::List(values)) = node.props.get(key) else {
            return Vec::new();
        };
        values
            .iter()
            .filter_map(|value| match &*value.borrow() {
                Value::Number(n) if n.is_finite() && *n >= 0.0 => Some(*n as usize),
                _ => None,
            })
            .collect()
    }

    fn build_mod_patch_cables(
        ports: &[ModPatchPort],
        vp_w: f32,
        vp_h: f32,
        cursor_px: (f32, f32),
    ) -> Vec<PatchCableDrawInstance> {
        let mut outputs = HashMap::new();
        for port in ports {
            if port.direction == ModPatchPortDirection::Out && port.active {
                outputs.insert(port.track, (port.center_px, port.clip));
            }
        }

        let mut cables = Vec::new();
        let base_color = Color::rgba(0.10, 0.58, 1.0, 0.92);
        let highlight_color = Color::rgba(0.78, 0.94, 1.0, 0.38);
        let shadow_color = Color::rgba(0.0, 0.0, 0.0, 0.34);
        let selected_color = Color::rgba(1.0, 0.16, 0.10, 0.96);
        let selected_highlight_color = Color::rgba(1.0, 0.66, 0.58, 0.42);
        let preview_color = Color::rgba(0.42, 0.84, 1.0, 0.84);
        let preview_highlight_color = Color::rgba(0.88, 0.98, 1.0, 0.36);
        let tension = 0.30;
        for port in ports {
            if port.direction != ModPatchPortDirection::In {
                continue;
            }
            for source in &port.connected_sources {
                let Some((start, source_clip)) = outputs.get(source).copied() else {
                    continue;
                };
                let cable_clip = shared_endpoint_clip(source_clip, port.clip, vp_w, vp_h);
                push_mod_patch_cable_instance(
                    (start.0 + 1.4, start.1 + 2.2),
                    (port.center_px.0 + 1.4, port.center_px.1 + 2.2),
                    3.6,
                    shadow_color,
                    cable_clip,
                    &mut cables,
                    vp_w,
                    vp_h,
                    tension,
                );
                let lane_tint = (port.input as f32 * 0.08).min(0.24);
                let is_selected = port
                    .selected_sources
                    .iter()
                    .any(|selected| selected == source);
                let color = if is_selected {
                    selected_color
                } else {
                    Color {
                        r: (base_color.r + lane_tint).min(1.0),
                        g: (base_color.g + lane_tint * 0.35).min(1.0),
                        b: base_color.b,
                        a: base_color.a,
                    }
                };
                push_mod_patch_cable_instance(
                    start,
                    port.center_px,
                    1.85,
                    color,
                    cable_clip,
                    &mut cables,
                    vp_w,
                    vp_h,
                    tension,
                );
                push_mod_patch_cable_instance(
                    (start.0, start.1 - 0.7),
                    (port.center_px.0, port.center_px.1 - 0.7),
                    0.55,
                    if is_selected {
                        selected_highlight_color
                    } else {
                        highlight_color
                    },
                    cable_clip,
                    &mut cables,
                    vp_w,
                    vp_h,
                    tension,
                );
            }
        }
        if let Some(source_port) = ports.iter().find(|port| {
            port.direction == ModPatchPortDirection::Out && port.active && port.pending
        }) {
            push_mod_patch_cable_instance(
                (source_port.center_px.0 + 1.4, source_port.center_px.1 + 2.2),
                (cursor_px.0 + 1.4, cursor_px.1 + 2.2),
                3.6,
                shadow_color,
                source_port.clip,
                &mut cables,
                vp_w,
                vp_h,
                tension,
            );
            push_mod_patch_cable_instance(
                source_port.center_px,
                cursor_px,
                1.85,
                preview_color,
                source_port.clip,
                &mut cables,
                vp_w,
                vp_h,
                tension,
            );
            push_mod_patch_cable_instance(
                (source_port.center_px.0, source_port.center_px.1 - 0.7),
                (cursor_px.0, cursor_px.1 - 0.7),
                0.55,
                preview_highlight_color,
                source_port.clip,
                &mut cables,
                vp_w,
                vp_h,
                tension,
            );
        }
        cables
    }

    fn full_viewport_scissor(vp_w: f32, vp_h: f32) -> MTLScissorRect {
        MTLScissorRect {
            x: 0,
            y: 0,
            width: vp_w.ceil().max(0.0) as usize,
            height: vp_h.ceil().max(0.0) as usize,
        }
    }

    fn same_scissor(a: MTLScissorRect, b: MTLScissorRect) -> bool {
        a.x == b.x && a.y == b.y && a.width == b.width && a.height == b.height
    }

    fn shared_endpoint_clip(
        source_clip: MTLScissorRect,
        dest_clip: MTLScissorRect,
        vp_w: f32,
        vp_h: f32,
    ) -> MTLScissorRect {
        if same_scissor(source_clip, dest_clip) {
            source_clip
        } else {
            full_viewport_scissor(vp_w, vp_h)
        }
    }

    fn nearest_mod_input_port<'a>(
        ports: &'a [ModPatchPort],
        source_track: usize,
        cursor_px: (f32, f32),
    ) -> Option<&'a ModPatchPort> {
        ports
            .iter()
            .filter(|port| {
                port.direction == ModPatchPortDirection::In
                    && port.active
                    && port.track != source_track
            })
            .min_by(|a, b| {
                let da = squared_distance_px(a.center_px, cursor_px);
                let db = squared_distance_px(b.center_px, cursor_px);
                da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
            })
    }

    fn squared_distance_px(a: (f32, f32), b: (f32, f32)) -> f32 {
        let dx = a.0 - b.0;
        let dy = a.1 - b.1;
        dx * dx + dy * dy
    }

    fn build_mod_patch_drag_highlight(
        ports: &[ModPatchPort],
        cursor_px: (f32, f32),
        vp_w: f32,
        vp_h: f32,
    ) -> Option<(Vec<Vertex>, MTLScissorRect)> {
        let Some(source_port) = ports.iter().find(|port| {
            port.direction == ModPatchPortDirection::Out && port.active && port.pending
        }) else {
            return None;
        };
        let Some(input_port) = nearest_mod_input_port(ports, source_port.track, cursor_px) else {
            return None;
        };
        let mut verts = Vec::new();
        let size = 13.5;
        let x = input_port.center_px.0 - size * 0.5;
        let y = input_port.center_px.1 - size * 0.5;
        push_rounded_rect_fill_px(
            &mut verts,
            x,
            y,
            size,
            size,
            size * 0.5,
            Color::rgba(0.30, 0.76, 1.0, 0.14),
            vp_w,
            vp_h,
        );
        push_rounded_rect_border_px(
            &mut verts,
            x,
            y,
            size,
            size,
            2.0,
            size * 0.5,
            Color::rgba(0.72, 0.95, 1.0, 0.92),
            vp_w,
            vp_h,
        );
        Some((verts, input_port.clip))
    }

    fn push_mod_patch_cable_instance(
        start: (f32, f32),
        end: (f32, f32),
        radius_px: f32,
        color: Color,
        clip: MTLScissorRect,
        cables: &mut Vec<PatchCableDrawInstance>,
        vp_w: f32,
        vp_h: f32,
        tension: f32,
    ) {
        let dx = end.0 - start.0;
        let dy = end.1 - start.1;
        let distance = (dx * dx + dy * dy).sqrt();
        let horizontal = dx.abs();
        let slack = (1.0 - tension).clamp(0.0, 1.0);
        let sag = ((28.0 + distance * 0.22) * slack).clamp(18.0, 98.0);
        let handle_x = horizontal.clamp(42.0, 190.0) * (0.30 + 0.14 * slack);
        let direction = if dx >= 0.0 { 1.0 } else { -1.0 };
        let c1 = (start.0 + handle_x * direction, start.1 + sag);
        let c2 = (end.0 - handle_x * direction, end.1 + sag);
        let padding = radius_px + sag * 0.12 + 8.0;
        let min_x = start.0.min(end.0).min(c1.0).min(c2.0) - padding;
        let max_x = start.0.max(end.0).max(c1.0).max(c2.0) + padding;
        let min_y = start.1.min(end.1).min(c1.1).min(c2.1) - padding;
        let max_y = start.1.max(end.1).max(c1.1).max(c2.1) + padding;
        if min_x >= vp_w || max_x <= 0.0 || min_y >= vp_h || max_y <= 0.0 {
            return;
        }
        let ndc_x = |px: f32| px / vp_w * 2.0 - 1.0;
        let ndc_y = |px: f32| 1.0 - px / vp_h * 2.0;
        cables.push(PatchCableDrawInstance {
            instance: PatchCableInstance {
                ndc_min: [ndc_x(min_x), ndc_y(min_y)],
                ndc_max: [ndc_x(max_x), ndc_y(max_y)],
                bounds_min: [min_x, min_y],
                bounds_max: [max_x, max_y],
                start: [start.0, start.1],
                control1: [c1.0, c1.1],
                control2: [c2.0, c2.1],
                end: [end.0, end.1],
                color: color.to_rgba(),
                radius_px,
                is_segmented: 0.0,
                segment_y_px: 0.0,
                corner_radius_px: 0.0,
            },
            clip,
        });
    }

    fn image_vertices(
        image: &widget_render::MetalImagePrimitive,
        image_w: u32,
        image_h: u32,
        cell_w: f32,
        cell_h: f32,
        vp_w: f32,
        vp_h: f32,
        rotation: f32,
    ) -> Vec<ImageVertex> {
        if image.rect.width <= 0.0 || image.rect.height <= 0.0 || image_w == 0 || image_h == 0 {
            return Vec::new();
        }
        let ndc_x = |px: f32| px / vp_w * 2.0 - 1.0;
        let ndc_y = |px: f32| 1.0 - px / vp_h * 2.0;
        let x0 = ndc_x(image.rect.col * cell_w);
        let x1 = ndc_x((image.rect.col + image.rect.width) * cell_w);
        let y0 = ndc_y(image.rect.row * cell_h);
        let y1 = ndc_y((image.rect.row + image.rect.height) * cell_h);

        let dst_aspect = (image.rect.width * cell_w) / (image.rect.height * cell_h).max(0.001);
        let src_aspect = image_w as f32 / (image_h as f32).max(1.0);
        let (mut u0, mut v0, mut u1, mut v1) = (0.0, 0.0, 1.0, 1.0);
        match image.fit {
            widget_render::ImageFit::Cover => {
                if src_aspect > dst_aspect {
                    let visible = dst_aspect / src_aspect;
                    u0 = (1.0 - visible) * 0.5;
                    u1 = 1.0 - u0;
                } else {
                    let visible = src_aspect / dst_aspect;
                    v0 = (1.0 - visible) * 0.5;
                    v1 = 1.0 - v0;
                }
            }
            widget_render::ImageFit::Contain | widget_render::ImageFit::Stretch => {}
        }

        if matches!(image.fit, widget_render::ImageFit::Contain) {
            let mut dx0 = x0;
            let mut dx1 = x1;
            let mut dy0 = y0;
            let mut dy1 = y1;
            if src_aspect > dst_aspect {
                let target_h = (image.rect.width * cell_w / src_aspect) / vp_h * 2.0;
                let mid = (y0 + y1) * 0.5;
                dy0 = mid + target_h * 0.5;
                dy1 = mid - target_h * 0.5;
            } else {
                let target_w = (image.rect.height * cell_h * src_aspect) / vp_w * 2.0;
                let mid = (x0 + x1) * 0.5;
                dx0 = mid - target_w * 0.5;
                dx1 = mid + target_w * 0.5;
            }
            return image_vertex_quad(
                dx0,
                dx1,
                dy0,
                dy1,
                u0,
                u1,
                v0,
                v1,
                image.opacity,
                image.radius_px,
                rotation,
                image.clip_circle,
                image.rect.width * cell_w,
                image.rect.height * cell_h,
            );
        }

        image_vertex_quad(
            x0,
            x1,
            y0,
            y1,
            u0,
            u1,
            v0,
            v1,
            image.opacity,
            image.radius_px,
            rotation,
            image.clip_circle,
            image.rect.width * cell_w,
            image.rect.height * cell_h,
        )
    }

    fn image_intersects_scissor(
        image: &widget_render::MetalImagePrimitive,
        scissor: MTLScissorRect,
        cell_w: f32,
        cell_h: f32,
    ) -> bool {
        let x0 = (image.rect.col * cell_w).floor() as isize;
        let y0 = (image.rect.row * cell_h).floor() as isize;
        let x1 = ((image.rect.col + image.rect.width) * cell_w).ceil() as isize;
        let y1 = ((image.rect.row + image.rect.height) * cell_h).ceil() as isize;
        let sx0 = scissor.x as isize;
        let sy0 = scissor.y as isize;
        let sx1 = (scissor.x + scissor.width) as isize;
        let sy1 = (scissor.y + scissor.height) as isize;
        x1 > sx0 && x0 < sx1 && y1 > sy0 && y0 < sy1
    }

    fn angular_distance(a: f32, b: f32) -> f32 {
        let tau = std::f32::consts::TAU;
        let mut d = (a - b).rem_euclid(tau);
        if d > std::f32::consts::PI {
            d = tau - d;
        }
        d.abs()
    }

    fn image_vertex_quad(
        x0: f32,
        x1: f32,
        y0: f32,
        y1: f32,
        u0: f32,
        u1: f32,
        v0: f32,
        v1: f32,
        opacity: f32,
        radius: f32,
        rotation: f32,
        clip_circle: bool,
        width_px: f32,
        height_px: f32,
    ) -> Vec<ImageVertex> {
        let half_size = [width_px.max(0.0) * 0.5, height_px.max(0.0) * 0.5];
        let clip_circle = if clip_circle { 1.0 } else { 0.0 };
        let v = |position, uv, local_pos| ImageVertex {
            position,
            uv,
            opacity,
            local_pos,
            half_size,
            radius,
            rotation,
            clip_circle,
        };
        vec![
            v([x0, y0], [u0, v0], [-half_size[0], -half_size[1]]),
            v([x0, y1], [u0, v1], [-half_size[0], half_size[1]]),
            v([x1, y0], [u1, v0], [half_size[0], -half_size[1]]),
            v([x1, y0], [u1, v0], [half_size[0], -half_size[1]]),
            v([x0, y1], [u0, v1], [-half_size[0], half_size[1]]),
            v([x1, y1], [u1, v1], [half_size[0], half_size[1]]),
        ]
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

    fn push_solid_triangle_vertices(
        triangle: widget_render::MetalTrianglePrimitive,
        cell_w: f32,
        cell_h: f32,
        vp_w: f32,
        vp_h: f32,
        verts: &mut Vec<Vertex>,
    ) {
        let ndc_x = |px: f32| px / vp_w * 2.0 - 1.0;
        let ndc_y = |px: f32| 1.0 - px / vp_h * 2.0;
        let rgba = triangle.color.to_rgba();
        let v = |point: [f32; 2]| Vertex {
            position: [ndc_x(point[0] * cell_w), ndc_y(point[1] * cell_h)],
            uv: [0.0, 0.0],
            fg: rgba,
            bg: rgba,
        };
        verts.extend_from_slice(&[
            v(triangle.points[0]),
            v(triangle.points[1]),
            v(triangle.points[2]),
        ]);
    }

    fn push_rect_px(
        verts: &mut Vec<Vertex>,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        color: Color,
        vp_w: f32,
        vp_h: f32,
    ) {
        push_rect_px_rgba(verts, x, y, width, height, color.to_rgba(), vp_w, vp_h);
    }

    fn rounded_rect_points_px(
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        radius: f32,
        segments_per_corner: usize,
    ) -> Vec<(f32, f32)> {
        if width <= 0.0 || height <= 0.0 {
            return Vec::new();
        }
        let radius = radius.clamp(0.0, width.min(height) * 0.5);
        if radius <= 0.0 {
            return vec![
                (x, y),
                (x + width, y),
                (x + width, y + height),
                (x, y + height),
            ];
        }

        let segments = segments_per_corner.max(2);
        let corners = [
            (x + width - radius, y + radius, -90.0f32, 0.0f32),
            (x + width - radius, y + height - radius, 0.0f32, 90.0f32),
            (x + radius, y + height - radius, 90.0f32, 180.0f32),
            (x + radius, y + radius, 180.0f32, 270.0f32),
        ];
        let mut points = Vec::with_capacity(corners.len() * (segments + 1));
        for (cx, cy, start_deg, end_deg) in corners {
            for i in 0..=segments {
                let t = i as f32 / segments as f32;
                let angle = (start_deg + (end_deg - start_deg) * t).to_radians();
                points.push((cx + angle.cos() * radius, cy + angle.sin() * radius));
            }
        }
        points
    }

    fn push_rounded_rect_fill_px(
        verts: &mut Vec<Vertex>,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        radius: f32,
        color: Color,
        vp_w: f32,
        vp_h: f32,
    ) {
        if width <= 0.0 || height <= 0.0 {
            return;
        }
        let points = rounded_rect_points_px(x, y, width, height, radius, 8);
        if points.len() < 3 {
            return;
        }

        let ndc_x = |px: f32| px / vp_w * 2.0 - 1.0;
        let ndc_y = |px: f32| 1.0 - px / vp_h * 2.0;
        let rgba = color.to_rgba();
        let center = (x + width * 0.5, y + height * 0.5);
        let v = |point: (f32, f32)| Vertex {
            position: [ndc_x(point.0), ndc_y(point.1)],
            uv: [0.0, 0.0],
            fg: rgba,
            bg: rgba,
        };

        for i in 0..points.len() {
            let j = (i + 1) % points.len();
            verts.extend_from_slice(&[v(center), v(points[i]), v(points[j])]);
        }
    }

    fn push_circle_fill_px(
        verts: &mut Vec<Vertex>,
        cx: f32,
        cy: f32,
        radius: f32,
        color: Color,
        visible_half: widget_render::MetalCircleVisibleHalf,
        vp_w: f32,
        vp_h: f32,
    ) {
        if radius <= 0.0 {
            return;
        }
        let segments = match visible_half {
            widget_render::MetalCircleVisibleHalf::Full => 32usize,
            widget_render::MetalCircleVisibleHalf::Top
            | widget_render::MetalCircleVisibleHalf::Bottom => 16usize,
        };
        let ndc_x = |px: f32| px / vp_w * 2.0 - 1.0;
        let ndc_y = |px: f32| 1.0 - px / vp_h * 2.0;
        let rgba = color.to_rgba();
        let v = |px: f32, py: f32| Vertex {
            position: [ndc_x(px), ndc_y(py)],
            uv: [0.0, 0.0],
            fg: rgba,
            bg: rgba,
        };
        for i in 0..segments {
            let (start_angle, sweep) = match visible_half {
                widget_render::MetalCircleVisibleHalf::Full => (0.0, std::f32::consts::TAU),
                widget_render::MetalCircleVisibleHalf::Top => {
                    (std::f32::consts::PI, std::f32::consts::PI)
                }
                widget_render::MetalCircleVisibleHalf::Bottom => (0.0, std::f32::consts::PI),
            };
            let a0 = start_angle + (i as f32 / segments as f32) * sweep;
            let a1 = start_angle + ((i + 1) as f32 / segments as f32) * sweep;
            verts.extend_from_slice(&[
                v(cx, cy),
                v(cx + a0.cos() * radius, cy + a0.sin() * radius),
                v(cx + a1.cos() * radius, cy + a1.sin() * radius),
            ]);
        }
    }

    fn push_rounded_rect_border_px(
        verts: &mut Vec<Vertex>,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        border_width: f32,
        radius: f32,
        color: Color,
        vp_w: f32,
        vp_h: f32,
    ) {
        if width <= 0.0 || height <= 0.0 || border_width <= 0.0 {
            return;
        }
        let inset = border_width.min(width * 0.5).min(height * 0.5);
        let radius = radius.clamp(0.0, width.min(height) * 0.5);
        let outer = rounded_rect_points_px(x, y, width, height, radius, 8);
        let inner = rounded_rect_points_px(
            x + inset,
            y + inset,
            (width - inset * 2.0).max(0.0),
            (height - inset * 2.0).max(0.0),
            (radius - inset).max(0.0),
            8,
        );
        if outer.len() < 3 || outer.len() != inner.len() {
            return;
        }

        let ndc_x = |px: f32| px / vp_w * 2.0 - 1.0;
        let ndc_y = |px: f32| 1.0 - px / vp_h * 2.0;
        let rgba = color.to_rgba();
        let v = |point: (f32, f32)| Vertex {
            position: [ndc_x(point.0), ndc_y(point.1)],
            uv: [0.0, 0.0],
            fg: rgba,
            bg: rgba,
        };

        for i in 0..outer.len() {
            let j = (i + 1) % outer.len();
            verts.extend_from_slice(&[
                v(outer[i]),
                v(inner[i]),
                v(outer[j]),
                v(outer[j]),
                v(inner[i]),
                v(inner[j]),
            ]);
        }
    }

    fn push_rect_px_rgba(
        verts: &mut Vec<Vertex>,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        rgba: [f32; 4],
        vp_w: f32,
        vp_h: f32,
    ) {
        if width <= 0.0 || height <= 0.0 {
            return;
        }
        let ndc_x = |px: f32| px / vp_w * 2.0 - 1.0;
        let ndc_y = |px: f32| 1.0 - px / vp_h * 2.0;
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

    fn push_text_cells(
        verts: &mut Vec<Vertex>,
        atlas: &mut GlyphAtlas,
        text: &str,
        col: usize,
        row: usize,
        max_cols: usize,
        fg: [f32; 4],
        bg: [f32; 4],
        cell_w: f32,
        cell_h: f32,
        vp_w: f32,
        vp_h: f32,
    ) {
        for (j, ch) in text.chars().take(max_cols).enumerate() {
            if ch == ' ' {
                continue;
            }
            rasterize_char(
                atlas,
                ch,
                ((col + j) as f32, row as f32),
                &CharCtx {
                    cell_w,
                    cell_h,
                    vp_w,
                    vp_h,
                    fg,
                    bg,
                },
                verts,
            );
        }
    }

    fn push_rounded_instance_cells(
        instances: &mut Vec<WidgetInstance>,
        col: f32,
        row: f32,
        width: f32,
        height: f32,
        color: Color,
        radius_px: f32,
        cell_w: f32,
        cell_h: f32,
        vp_w: f32,
        vp_h: f32,
    ) {
        push_rounded_instance_cells_rgba(
            instances,
            col,
            row,
            width,
            height,
            color.to_rgba(),
            cell_w,
            cell_h,
            vp_w,
            vp_h,
        );
        if let Some(instance) = instances.last_mut() {
            instance.corner_radius = normalized_overlay_radius(height, cell_h, radius_px);
        }
    }

    fn push_rounded_instance_cells_rgba(
        instances: &mut Vec<WidgetInstance>,
        col: f32,
        row: f32,
        width: f32,
        height: f32,
        rgba: [f32; 4],
        cell_w: f32,
        cell_h: f32,
        vp_w: f32,
        vp_h: f32,
    ) {
        if width <= 0.0 || height <= 0.0 {
            return;
        }
        let ndc_x = |px: f32| px / vp_w * 2.0 - 1.0;
        let ndc_y = |px: f32| 1.0 - px / vp_h * 2.0;
        let x0 = ndc_x(col * cell_w);
        let x1 = ndc_x((col + width) * cell_w);
        let y0 = ndc_y(row * cell_h);
        let y1 = ndc_y((row + height) * cell_h);
        instances.push(WidgetInstance {
            ndc_min: [x0.min(x1), y1.min(y0)],
            ndc_max: [x0.max(x1), y1.max(y0)],
            value_t: 0.0,
            orientation: 0.0,
            itime: 0.0,
            uniform_a: [0.0; 4],
            uniform_b: [0.0; 4],
            color_a: rgba,
            color_b: rgba,
            color_c: rgba,
            color_d: rgba,
            corner_radius: normalized_overlay_radius(height, cell_h, 7.0),
            pixel_aspect: if height > 0.0 {
                (width * cell_w) / (height * cell_h)
            } else {
                1.0
            },
        });
    }

    fn normalized_overlay_radius(height_cells: f32, cell_h: f32, radius_px: f32) -> f32 {
        if radius_px <= 0.0 || height_cells <= 0.0 || cell_h <= 0.0 {
            return 0.0;
        }
        ((radius_px * 2.0) / (height_cells * cell_h)).clamp(0.001, 0.5)
    }

    fn draw_widget_instances(
        enc: &ProtocolObject<dyn MTLRenderCommandEncoder>,
        device: &ProtocolObject<dyn MTLDevice>,
        upload_arena: &mut GpuUploadArena,
        stats: &mut RenderStats,
        pipeline: &ProtocolObject<dyn MTLRenderPipelineState>,
        instances: &[WidgetInstance],
    ) {
        if instances.is_empty() {
            return;
        }
        let Some(upload) = upload_arena.upload_slice(device, instances, stats) else {
            return;
        };
        enc.setRenderPipelineState(pipeline);
        unsafe {
            enc.setVertexBuffer_offset_atIndex(Some(&upload.buffer), upload.offset, 0);
            enc.drawPrimitives_vertexStart_vertexCount_instanceCount(
                MTLPrimitiveType::Triangle,
                0,
                6,
                instances.len() as _,
            );
        }
        stats.note_draw_command();
    }

    fn draw_vertices(
        enc: &ProtocolObject<dyn MTLRenderCommandEncoder>,
        device: &ProtocolObject<dyn MTLDevice>,
        upload_arena: &mut GpuUploadArena,
        stats: &mut RenderStats,
        pipeline: &ProtocolObject<dyn MTLRenderPipelineState>,
        texture: &ProtocolObject<dyn MTLTexture>,
        verts: &[Vertex],
    ) {
        if verts.is_empty() {
            return;
        }
        let Some(upload) = upload_arena.upload_slice(device, verts, stats) else {
            return;
        };
        enc.setRenderPipelineState(pipeline);
        unsafe {
            enc.setVertexBuffer_offset_atIndex(Some(&upload.buffer), upload.offset, 0);
            enc.setFragmentTexture_atIndex(Some(texture), 0);
            enc.drawPrimitives_vertexStart_vertexCount(
                MTLPrimitiveType::Triangle,
                0,
                verts.len() as _,
            );
        }
        stats.note_draw_command();
    }

    fn draw_patch_cable_instances(
        enc: &ProtocolObject<dyn MTLRenderCommandEncoder>,
        device: &ProtocolObject<dyn MTLDevice>,
        upload_arena: &mut GpuUploadArena,
        stats: &mut RenderStats,
        pipeline: &ProtocolObject<dyn MTLRenderPipelineState>,
        instances: &[PatchCableDrawInstance],
    ) {
        if instances.is_empty() {
            return;
        }
        enc.setRenderPipelineState(pipeline);

        let mut run_start = 0;
        while run_start < instances.len() {
            let clip = instances[run_start].clip;
            let mut run_end = run_start + 1;
            while run_end < instances.len() && same_scissor(instances[run_end].clip, clip) {
                run_end += 1;
            }

            let run: Vec<PatchCableInstance> = instances[run_start..run_end]
                .iter()
                .map(|draw| draw.instance)
                .collect();
            let Some(upload) = upload_arena.upload_slice(device, run.as_slice(), stats) else {
                run_start = run_end;
                continue;
            };
            enc.setScissorRect(clip);
            unsafe {
                enc.setVertexBuffer_offset_atIndex(Some(&upload.buffer), upload.offset, 0);
                enc.drawPrimitives_vertexStart_vertexCount_instanceCount(
                    MTLPrimitiveType::Triangle,
                    0,
                    6,
                    run.len() as _,
                );
            }
            stats.note_draw_command();
            run_start = run_end;
        }
    }

    fn wrap_completion_doc_lines(lines: &[String], width: usize) -> Vec<String> {
        let width = width.max(8);
        let mut out = Vec::new();
        for line in lines {
            let mut current = String::new();
            for word in line.split_whitespace() {
                let current_len = current.chars().count();
                let word_len = word.chars().count();
                if current_len == 0 {
                    current.push_str(word);
                } else if current_len + 1 + word_len <= width {
                    current.push(' ');
                    current.push_str(word);
                } else {
                    out.push(current);
                    current = word.to_string();
                }
            }
            if !current.is_empty() {
                out.push(current);
            } else if line.trim().is_empty() {
                out.push(String::new());
            }
        }
        out
    }

    /// Split a flat list of primitives into segments separated by PushClipRect/PopClipRect.
    /// Each segment gets an associated scissor rect. Clip rects are intersected with the
    /// current scissor (stacked) so nested scroll containers work correctly.
    fn split_prim_segments<'a>(
        primitives: &'a [widget_render::MetalPrimitive],
        base_scissor: MTLScissorRect,
        cell_w: f32,
        cell_h: f32,
    ) -> Vec<(MTLScissorRect, &'a [widget_render::MetalPrimitive])> {
        // Fast path: no clip rects at all
        let has_clips = primitives.iter().any(|p| {
            matches!(
                p,
                widget_render::MetalPrimitive::PushClipRect(_)
                    | widget_render::MetalPrimitive::PopClipRect
            )
        });
        if !has_clips {
            return vec![(base_scissor, primitives)];
        }

        let mut segments = Vec::new();
        let mut scissor_stack: Vec<MTLScissorRect> = vec![base_scissor];
        let mut seg_start = 0;

        for (i, prim) in primitives.iter().enumerate() {
            match prim {
                widget_render::MetalPrimitive::PushClipRect(rect) => {
                    // Flush the current segment (excluding this marker)
                    if i > seg_start {
                        segments.push((*scissor_stack.last().unwrap(), &primitives[seg_start..i]));
                    }
                    // Compute new scissor = intersection of current and clip rect
                    let clip_x = (rect.col * cell_w).max(0.0) as usize;
                    let clip_y = (rect.row * cell_h).max(0.0) as usize;
                    let clip_w = (rect.width * cell_w).max(0.0) as usize;
                    let clip_h = (rect.height * cell_h).max(0.0) as usize;
                    let current = scissor_stack.last().unwrap();
                    let new_scissor = intersect_scissor_rects(
                        *current,
                        MTLScissorRect {
                            x: clip_x,
                            y: clip_y,
                            width: clip_w,
                            height: clip_h,
                        },
                    );
                    scissor_stack.push(new_scissor);
                    seg_start = i + 1;
                }
                widget_render::MetalPrimitive::PopClipRect => {
                    // Flush the current segment
                    if i > seg_start {
                        segments.push((*scissor_stack.last().unwrap(), &primitives[seg_start..i]));
                    }
                    scissor_stack.pop();
                    seg_start = i + 1;
                }
                _ => {}
            }
        }
        // Final segment
        if seg_start < primitives.len() {
            segments.push((*scissor_stack.last().unwrap(), &primitives[seg_start..]));
        }
        segments
    }

    fn split_prim_segment_ranges(
        primitives: &[widget_render::MetalPrimitive],
        base_scissor: MTLScissorRect,
        cell_w: f32,
        cell_h: f32,
    ) -> Vec<(MTLScissorRect, Range<usize>)> {
        if !primitives.iter().any(|primitive| {
            matches!(
                primitive,
                widget_render::MetalPrimitive::PushClipRect(_)
                    | widget_render::MetalPrimitive::PopClipRect
            )
        }) {
            return vec![(base_scissor, 0..primitives.len())];
        }

        let mut segments = Vec::new();
        let mut scissor_stack: Vec<MTLScissorRect> = vec![base_scissor];
        let mut seg_start = 0;

        for (i, prim) in primitives.iter().enumerate() {
            match prim {
                widget_render::MetalPrimitive::PushClipRect(rect) => {
                    if i > seg_start {
                        segments.push((*scissor_stack.last().unwrap(), seg_start..i));
                    }
                    let clip_x = (rect.col * cell_w).max(0.0) as usize;
                    let clip_y = (rect.row * cell_h).max(0.0) as usize;
                    let clip_w = (rect.width * cell_w).max(0.0) as usize;
                    let clip_h = (rect.height * cell_h).max(0.0) as usize;
                    let current = scissor_stack.last().unwrap();
                    let new_scissor = intersect_scissor_rects(
                        *current,
                        MTLScissorRect {
                            x: clip_x,
                            y: clip_y,
                            width: clip_w,
                            height: clip_h,
                        },
                    );
                    scissor_stack.push(new_scissor);
                    seg_start = i + 1;
                }
                widget_render::MetalPrimitive::PopClipRect => {
                    if i > seg_start {
                        segments.push((*scissor_stack.last().unwrap(), seg_start..i));
                    }
                    scissor_stack.pop();
                    seg_start = i + 1;
                }
                _ => {}
            }
        }
        if seg_start < primitives.len() {
            segments.push((*scissor_stack.last().unwrap(), seg_start..primitives.len()));
        }
        segments
    }

    fn z_ordered_primitive_layers(
        primitives: &[widget_render::MetalPrimitive],
    ) -> Vec<Vec<widget_render::MetalPrimitive>> {
        let has_layers = primitives
            .iter()
            .any(|primitive| matches!(primitive, widget_render::MetalPrimitive::ZLayer { .. }));
        if !has_layers {
            return vec![primitives.to_vec()];
        }
        let mut buckets: BTreeMap<i32, Vec<widget_render::MetalPrimitive>> = BTreeMap::new();
        for primitive in primitives {
            match primitive {
                widget_render::MetalPrimitive::ZLayer { z_index, primitive } => {
                    buckets
                        .entry(*z_index)
                        .or_default()
                        .push((**primitive).clone());
                }
                primitive => {
                    buckets.entry(0).or_default().push(primitive.clone());
                }
            }
        }
        buckets.into_values().collect()
    }

    fn intersect_scissor_rects(a: MTLScissorRect, b: MTLScissorRect) -> MTLScissorRect {
        let x1 = a.x.max(b.x);
        let y1 = a.y.max(b.y);
        let x2 = (a.x + a.width).min(b.x + b.width);
        let y2 = (a.y + a.height).min(b.y + b.height);
        MTLScissorRect {
            x: x1,
            y: y1,
            width: if x2 > x1 { x2 - x1 } else { 0 },
            height: if y2 > y1 { y2 - y1 } else { 0 },
        }
    }

    /// Partition widget instances into background and foreground runs in a single pass.
    fn partition_widget_instance_runs(
        primitives: &[widget_render::MetalPrimitive],
    ) -> (
        Vec<(String, Vec<WidgetInstance>)>,
        Vec<(String, Vec<WidgetInstance>)>,
    ) {
        let mut bg_runs: Vec<(String, Vec<WidgetInstance>)> = Vec::new();
        let mut fg_runs: Vec<(String, Vec<WidgetInstance>)> = Vec::new();
        for primitive in primitives {
            if let widget_render::MetalPrimitive::WidgetInstance {
                widget_type,
                instance,
                is_background,
            } = widget_render::innermost_primitive(primitive)
            {
                let runs = if *is_background {
                    &mut bg_runs
                } else {
                    &mut fg_runs
                };
                if let Some((run_type, instances)) = runs.last_mut()
                    && run_type == widget_type
                {
                    instances.push(*instance);
                } else {
                    runs.push((widget_type.clone(), vec![*instance]));
                }
            }
        }
        (bg_runs, fg_runs)
    }

    fn contains_agent_instrument_stub_animation(
        primitives: &[widget_render::MetalPrimitive],
    ) -> bool {
        primitives.iter().any(|primitive| {
            matches!(
                widget_render::innermost_primitive(primitive),
                widget_render::MetalPrimitive::WidgetInstance { widget_type, .. }
                    if is_agent_instrument_stub_animation_widget_type(widget_type)
            )
        })
    }

    fn layout_contains_agent_instrument_stub_animation(layout: &crate::layout::LayoutNode) -> bool {
        is_agent_instrument_stub_animation_widget_type(&layout.widget_type)
            || layout_debug_name(layout) == Some(AGENT_INSTRUMENT_STUB_SKELETON_DEBUG_NAME)
            || layout
                .children
                .iter()
                .any(layout_contains_agent_instrument_stub_animation)
    }

    fn is_agent_instrument_stub_animation_widget_type(widget_type: &str) -> bool {
        widget_type == AGENT_INSTRUMENT_STUB_ANIMATION_WIDGET
            || widget_type.ends_with(AGENT_INSTRUMENT_STUB_ANIMATION_WIDGET_SUFFIX)
            || widget_type.ends_with(AGENT_INSTRUMENT_STUB_ANIMATION_WIDGET_SAFE_SUFFIX)
    }

    fn layout_debug_name(layout: &crate::layout::LayoutNode) -> Option<&str> {
        let value = layout.props.get("debug-name")?;
        let crate::vm::Value::String(debug_name) = value else {
            return None;
        };
        Some(debug_name.as_str())
    }

    fn extend_right_edge_primitive(
        prim: widget_render::MetalPrimitive,
        layout_width: f32,
        extra_cols: f32,
        cell_w: f32,
        vp_w: f32,
    ) -> widget_render::MetalPrimitive {
        if extra_cols <= 0.001 || layout_width <= 0.0 {
            return prim;
        }
        let reaches_right = |right: f32| (right - layout_width).abs() <= 0.01;
        match prim {
            widget_render::MetalPrimitive::ZLayer { z_index, primitive } => {
                widget_render::MetalPrimitive::ZLayer {
                    z_index,
                    primitive: Box::new(extend_right_edge_primitive(
                        *primitive,
                        layout_width,
                        extra_cols,
                        cell_w,
                        vp_w,
                    )),
                }
            }
            widget_render::MetalPrimitive::Rect(mut r) => {
                if reaches_right(r.rect.col + r.rect.width) {
                    r.rect.width += extra_cols;
                }
                widget_render::MetalPrimitive::Rect(r)
            }
            widget_render::MetalPrimitive::ForegroundRect(mut r) => {
                if reaches_right(r.rect.col + r.rect.width) {
                    r.rect.width += extra_cols;
                }
                widget_render::MetalPrimitive::ForegroundRect(r)
            }
            widget_render::MetalPrimitive::Quad(mut q) => {
                if reaches_right(q.x + q.width) {
                    q.width += extra_cols;
                }
                widget_render::MetalPrimitive::Quad(q)
            }
            widget_render::MetalPrimitive::Triangle(mut t) => {
                for point in &mut t.points {
                    if reaches_right(point[0]) {
                        point[0] += extra_cols;
                    }
                }
                widget_render::MetalPrimitive::Triangle(t)
            }
            widget_render::MetalPrimitive::ProportionalText(mut p) => {
                if p.align_width > 0.0 && reaches_right(p.col + p.align_width) {
                    p.align_width += extra_cols;
                }
                widget_render::MetalPrimitive::ProportionalText(p)
            }
            widget_render::MetalPrimitive::PatchCable(mut c) => {
                if reaches_right(c.end[0]) {
                    c.end[0] += extra_cols;
                    c.control2[0] += extra_cols;
                }
                widget_render::MetalPrimitive::PatchCable(c)
            }
            widget_render::MetalPrimitive::Circle(mut c) => {
                if reaches_right(c.center[0]) {
                    c.center[0] += extra_cols;
                }
                widget_render::MetalPrimitive::Circle(c)
            }
            widget_render::MetalPrimitive::Waveform(mut w) => {
                if reaches_right(w.rect.col + w.rect.width) {
                    w.rect.width += extra_cols;
                }
                widget_render::MetalPrimitive::Waveform(w)
            }
            widget_render::MetalPrimitive::Image(mut i) => {
                if reaches_right(i.rect.col + i.rect.width) {
                    i.rect.width += extra_cols;
                }
                widget_render::MetalPrimitive::Image(i)
            }
            widget_render::MetalPrimitive::WidgetInstance {
                widget_type,
                mut instance,
                is_background,
            } => {
                let local_right_ndc = -1.0 + (layout_width * cell_w / vp_w) * 2.0;
                if (instance.ndc_max[0] - local_right_ndc).abs() <= 0.002 {
                    let old_width = instance.ndc_max[0] - instance.ndc_min[0];
                    instance.ndc_max[0] += (extra_cols * cell_w / vp_w) * 2.0;
                    let new_width = instance.ndc_max[0] - instance.ndc_min[0];
                    if old_width > 0.0 {
                        instance.pixel_aspect *= new_width / old_width;
                    }
                }
                widget_render::MetalPrimitive::WidgetInstance {
                    widget_type,
                    instance,
                    is_background,
                }
            }
            widget_render::MetalPrimitive::PushClipRect(mut r) => {
                if reaches_right(r.col + r.width) {
                    r.width += extra_cols;
                }
                widget_render::MetalPrimitive::PushClipRect(r)
            }
            other => other,
        }
    }

    /// Offset a MetalPrimitive by (col_off, row_off) cells.
    /// For Rect/Quad/GlyphRun: shift cell coordinates.
    /// For WidgetInstance: shift NDC bounds using the pixel conversion.
    /// Offset a MetalPrimitive by (col_off, row_off) cells (signed for scroll).
    fn offset_primitive(
        prim: widget_render::MetalPrimitive,
        col_off: f32,
        row_off: f32,
        cell_w: f32,
        cell_h: f32,
        vp_w: f32,
        vp_h: f32,
    ) -> widget_render::MetalPrimitive {
        match prim {
            widget_render::MetalPrimitive::ZLayer { z_index, primitive } => {
                widget_render::MetalPrimitive::ZLayer {
                    z_index,
                    primitive: Box::new(offset_primitive(
                        *primitive, col_off, row_off, cell_w, cell_h, vp_w, vp_h,
                    )),
                }
            }
            widget_render::MetalPrimitive::Rect(mut r) => {
                r.rect.col += col_off;
                r.rect.row += row_off;
                widget_render::MetalPrimitive::Rect(r)
            }
            widget_render::MetalPrimitive::ForegroundRect(mut r) => {
                r.rect.col += col_off;
                r.rect.row += row_off;
                widget_render::MetalPrimitive::ForegroundRect(r)
            }
            widget_render::MetalPrimitive::Quad(mut q) => {
                q.x += col_off;
                q.y += row_off;
                widget_render::MetalPrimitive::Quad(q)
            }
            widget_render::MetalPrimitive::Triangle(mut t) => {
                for point in &mut t.points {
                    point[0] += col_off;
                    point[1] += row_off;
                }
                widget_render::MetalPrimitive::Triangle(t)
            }
            widget_render::MetalPrimitive::GlyphRun(mut g) => {
                g.col += col_off.round() as i32;
                g.row += row_off;
                widget_render::MetalPrimitive::GlyphRun(g)
            }
            widget_render::MetalPrimitive::ProportionalText(mut p) => {
                p.col += col_off;
                p.row += row_off;
                widget_render::MetalPrimitive::ProportionalText(p)
            }
            widget_render::MetalPrimitive::PatchCable(mut c) => {
                c.start[0] += col_off;
                c.start[1] += row_off;
                c.control1[0] += col_off;
                c.control1[1] += row_off;
                c.control2[0] += col_off;
                c.control2[1] += row_off;
                c.end[0] += col_off;
                c.end[1] += row_off;
                c.segment_row += row_off;
                widget_render::MetalPrimitive::PatchCable(c)
            }
            widget_render::MetalPrimitive::Circle(mut c) => {
                c.center[0] += col_off;
                c.center[1] += row_off;
                widget_render::MetalPrimitive::Circle(c)
            }
            widget_render::MetalPrimitive::Waveform(mut w) => {
                w.rect.col += col_off;
                w.rect.row += row_off;
                widget_render::MetalPrimitive::Waveform(w)
            }
            widget_render::MetalPrimitive::Image(mut i) => {
                i.rect.col += col_off;
                i.rect.row += row_off;
                widget_render::MetalPrimitive::Image(i)
            }
            widget_render::MetalPrimitive::WidgetInstance {
                widget_type,
                mut instance,
                is_background,
            } => {
                let ndc_dx = (col_off * cell_w / vp_w) * 2.0;
                let ndc_dy = -(row_off * cell_h / vp_h) * 2.0;
                instance.ndc_min[0] += ndc_dx;
                instance.ndc_max[0] += ndc_dx;
                instance.ndc_min[1] += ndc_dy;
                instance.ndc_max[1] += ndc_dy;
                widget_render::MetalPrimitive::WidgetInstance {
                    widget_type,
                    instance,
                    is_background,
                }
            }
            widget_render::MetalPrimitive::PushClipRect(mut r) => {
                r.col += col_off;
                r.row += row_off;
                widget_render::MetalPrimitive::PushClipRect(r)
            }
            widget_render::MetalPrimitive::PopClipRect => {
                widget_render::MetalPrimitive::PopClipRect
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
        if mods.super_key() {
            out |= KeyModifiers::SUPER;
        }
        out
    }

    fn translate_key(key: &Key, physical_key: &PhysicalKey, mods: KeyModifiers) -> Option<Event> {
        let code = if mods
            .intersects(KeyModifiers::ALT | KeyModifiers::CONTROL | KeyModifiers::SUPER)
        {
            translate_physical_shortcut_key(physical_key).or_else(|| translate_logical_key(key))?
        } else {
            translate_logical_key(key)?
        };
        Some(Event::Key(KeyEvent::new(code, mods)))
    }

    fn translate_key_with_state(
        key: &Key,
        physical_key: &PhysicalKey,
        mods: KeyModifiers,
        state: ElementState,
    ) -> Option<Event> {
        let code = if mods
            .intersects(KeyModifiers::ALT | KeyModifiers::CONTROL | KeyModifiers::SUPER)
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
    mod render_dispatch_tests {
        use super::*;
        use crate::layout::LayoutNode;
        use crate::vm::Value;
        use crate::widget_render::{MetalPrimitive, WidgetViewport};

        fn prop_string(value: &str) -> Value {
            Value::String(value.to_string())
        }

        fn prop_number(value: f64) -> Value {
            Value::Number(value)
        }

        fn prop_keyword(value: &str) -> Value {
            Value::Keyword(value.to_string())
        }

        fn layout_node(widget_type: &str, props: HashMap<String, Value>) -> LayoutNode {
            LayoutNode {
                widget_id: 1,
                stable_widget_id: None,
                subtree_root_id: None,
                parent_subtree_root_id: None,
                stable_key: None,
                widget_type: widget_type.to_string(),
                rect: Rect {
                    col: 0.0,
                    row: 0.0,
                    width: 10.0,
                    height: 4.0,
                },
                props,
                children: Vec::new(),
                focusable: false,
            }
        }

        fn rect_contains(outer: Rect, inner: Rect) -> bool {
            inner.col >= outer.col
                && inner.row >= outer.row
                && inner.col + inner.width <= outer.col + outer.width
                && inner.row + inner.height <= outer.row + outer.height
        }

        fn widget_instance_rect(instance: &WidgetInstance, viewport: WidgetViewport) -> Rect {
            let ndc_x_to_col = |x: f32| ((x + 1.0) * 0.5 * viewport.vp_w) / viewport.cell_w;
            let ndc_y_to_row = |y: f32| ((1.0 - y) * 0.5 * viewport.vp_h) / viewport.cell_h;
            let left = ndc_x_to_col(instance.ndc_min[0]);
            let right = ndc_x_to_col(instance.ndc_max[0]);
            let top = ndc_y_to_row(instance.ndc_min[1]);
            let bottom = ndc_y_to_row(instance.ndc_max[1]);
            Rect {
                col: left.min(right),
                row: top.min(bottom),
                width: (right - left).abs(),
                height: (bottom - top).abs(),
            }
        }

        fn test_widget_run_cache_key(
            primitives: &[MetalPrimitive],
            cell_w: f32,
            cell_h: f32,
            vp_w: f32,
            vp_h: f32,
            mono_atlas_generation: u64,
            prop_atlas_generation: u64,
        ) -> WidgetRunCacheKey {
            widget_run_cache_key(
                7,
                "label",
                primitives,
                cell_w,
                cell_h,
                vp_w,
                vp_h,
                mono_atlas_generation,
                prop_atlas_generation,
            )
        }

        fn test_widget_instance_primitive(widget_type: &str, itime: f32) -> MetalPrimitive {
            MetalPrimitive::WidgetInstance {
                widget_type: widget_type.to_string(),
                instance: WidgetInstance {
                    ndc_min: [-0.5, -0.5],
                    ndc_max: [0.5, 0.5],
                    value_t: 0.25,
                    orientation: 0.0,
                    itime,
                    uniform_a: [0.0; 4],
                    uniform_b: [0.0; 4],
                    color_a: [0.2, 0.3, 0.4, 1.0],
                    color_b: [0.5, 0.6, 0.7, 1.0],
                    color_c: [0.0; 4],
                    color_d: [0.0; 4],
                    corner_radius: 0.2,
                    pixel_aspect: 1.0,
                },
                is_background: false,
            }
        }

        #[test]
        fn metal_button_background_is_not_followed_by_covering_box_rect() {
            let viewport = WidgetViewport {
                cell_w: 10.0,
                cell_h: 10.0,
                vp_w: 400.0,
                vp_h: 240.0,
                time_seconds: 0.0,
                focused_widget_id: None,
                focused_branch: false,
                tile_content_rows: 24.0,
                scroll_top: 0.0,
                scroll_left: 0.0,
                inherited_hover: false,
            };

            let button = LayoutNode {
                widget_id: 2,
                stable_widget_id: None,
                subtree_root_id: None,
                parent_subtree_root_id: None,
                stable_key: None,
                widget_type: "button".to_string(),
                rect: Rect {
                    col: 2.0,
                    row: 6.0,
                    width: 3.0,
                    height: 1.0,
                },
                props: HashMap::from([
                    ("text".to_string(), prop_string("A")),
                    ("width".to_string(), prop_number(3.0)),
                    ("height".to_string(), prop_number(1.0)),
                    ("padding".to_string(), prop_number(0.0)),
                    ("font-size".to_string(), prop_number(10.0)),
                    ("background-color".to_string(), prop_keyword("orange")),
                    ("color".to_string(), prop_keyword("black")),
                ]),
                children: Vec::new(),
                focusable: true,
            };

            let strip = LayoutNode {
                widget_id: 1,
                stable_widget_id: None,
                subtree_root_id: None,
                parent_subtree_root_id: None,
                stable_key: None,
                widget_type: "box".to_string(),
                rect: Rect {
                    col: 0.0,
                    row: 0.0,
                    width: 8.0,
                    height: 12.0,
                },
                props: HashMap::from([(
                    "background-color".to_string(),
                    Value::List(vec![
                        std::rc::Rc::new(std::cell::RefCell::new(Value::Number(0.13))),
                        std::rc::Rc::new(std::cell::RefCell::new(Value::Number(0.13))),
                        std::rc::Rc::new(std::cell::RefCell::new(Value::Number(0.14))),
                        std::rc::Rc::new(std::cell::RefCell::new(Value::Number(1.0))),
                    ]),
                )]),
                children: vec![button],
                focusable: false,
            };

            let (primitives, _) =
                widget_render::collect_metal_primitives(&strip, viewport, 0.0, 24);
            let button_bg_rect = primitives
                .iter()
                .find_map(|primitive| match primitive {
                    MetalPrimitive::WidgetInstance {
                        widget_type,
                        instance,
                        is_background: true,
                    } if widget_type == "button" => Some(widget_instance_rect(instance, viewport)),
                    _ => None,
                })
                .expect("button should emit a background instance");

            let covering_rect_dispatched_after_button_background =
                primitives.iter().find(|primitive| match primitive {
                    MetalPrimitive::Rect(rect) => rect_contains(rect.rect, button_bg_rect),
                    _ => false,
                });

            assert!(
                covering_rect_dispatched_after_button_background.is_none(),
                "the backend dispatches widget backgrounds before Rect primitives; this covering Rect paints over the button chrome"
            );
        }

        #[test]
        fn agent_stub_animation_detection_matches_namespaced_custom_widget() {
            let namespaced = layout_node(
                "custom_ui_agent_draft___agent_instrument_stub_bg",
                HashMap::new(),
            );
            assert!(layout_contains_agent_instrument_stub_animation(&namespaced));
            assert!(contains_agent_instrument_stub_animation(&[
                MetalPrimitive::WidgetInstance {
                    widget_type: "custom_ui_agent_draft___agent_instrument_stub_bg".to_string(),
                    instance: WidgetInstance {
                        ndc_min: [-1.0, -1.0],
                        ndc_max: [1.0, 1.0],
                        value_t: 0.0,
                        orientation: 0.0,
                        itime: 0.0,
                        uniform_a: [0.0; 4],
                        uniform_b: [0.0; 4],
                        color_a: [0.0; 4],
                        color_b: [0.0; 4],
                        color_c: [0.0; 4],
                        color_d: [0.0; 4],
                        corner_radius: 0.0,
                        pixel_aspect: 1.0,
                    },
                    is_background: false,
                }
            ]));
        }

        #[test]
        fn agent_stub_animation_detection_matches_raw_defwidget_name() {
            let raw = layout_node("agent-instrument-stub-bg", HashMap::new());
            assert!(layout_contains_agent_instrument_stub_animation(&raw));
        }

        #[test]
        fn agent_stub_animation_detection_matches_skeleton_debug_name() {
            let skeleton = layout_node(
                "box",
                HashMap::from([(
                    "debug-name".to_string(),
                    prop_string("agent-instrument-stub-skeleton"),
                )]),
            );
            assert!(layout_contains_agent_instrument_stub_animation(&skeleton));
        }

        #[test]
        fn widget_run_cache_key_reuses_unchanged_label_primitives() {
            let primitives = vec![MetalPrimitive::ProportionalText(
                widget_render::MetalProportionalTextPrimitive {
                    row: 1.0,
                    col: 2.0,
                    align_width: 6.0,
                    h_align: 0.0,
                    text: "1".to_string(),
                    font_size: 12.0,
                    scale: 1.0,
                    fg: theme::FG(),
                    bg: theme::BG(),
                },
            )];

            let a = test_widget_run_cache_key(&primitives, 8.0, 16.0, 800.0, 600.0, 1, 1);
            let b = test_widget_run_cache_key(&primitives, 8.0, 16.0, 800.0, 600.0, 1, 1);

            assert_eq!(a, b);
        }

        #[test]
        fn widget_run_cache_key_invalidates_changed_text() {
            let mut primitives = vec![MetalPrimitive::ProportionalText(
                widget_render::MetalProportionalTextPrimitive {
                    row: 1.0,
                    col: 2.0,
                    align_width: 6.0,
                    h_align: 0.0,
                    text: "1".to_string(),
                    font_size: 12.0,
                    scale: 1.0,
                    fg: theme::FG(),
                    bg: theme::BG(),
                },
            )];

            let before = test_widget_run_cache_key(&primitives, 8.0, 16.0, 800.0, 600.0, 1, 1);
            if let MetalPrimitive::ProportionalText(text) = &mut primitives[0] {
                text.text = "17".to_string();
            }
            let after = test_widget_run_cache_key(&primitives, 8.0, 16.0, 800.0, 600.0, 1, 1);

            assert_ne!(before, after);
        }

        #[test]
        fn widget_run_cache_key_invalidates_text_style_and_view_metrics() {
            let mut primitives = vec![MetalPrimitive::ProportionalText(
                widget_render::MetalProportionalTextPrimitive {
                    row: 1.0,
                    col: 2.0,
                    align_width: 6.0,
                    h_align: 0.0,
                    text: "Tempo".to_string(),
                    font_size: 12.0,
                    scale: 1.0,
                    fg: theme::FG(),
                    bg: theme::BG(),
                },
            )];

            let base = test_widget_run_cache_key(&primitives, 8.0, 16.0, 800.0, 600.0, 1, 1);
            if let MetalPrimitive::ProportionalText(text) = &mut primitives[0] {
                text.font_size = 14.0;
            }
            assert_ne!(
                base,
                test_widget_run_cache_key(&primitives, 8.0, 16.0, 800.0, 600.0, 1, 1)
            );

            if let MetalPrimitive::ProportionalText(text) = &mut primitives[0] {
                text.font_size = 12.0;
                text.h_align = 1.0;
            }
            assert_ne!(
                base,
                test_widget_run_cache_key(&primitives, 8.0, 16.0, 800.0, 600.0, 1, 1)
            );

            if let MetalPrimitive::ProportionalText(text) = &mut primitives[0] {
                text.h_align = 0.0;
                text.fg = theme::ACCENT();
            }
            assert_ne!(
                base,
                test_widget_run_cache_key(&primitives, 8.0, 16.0, 800.0, 600.0, 1, 1)
            );

            if let MetalPrimitive::ProportionalText(text) = &mut primitives[0] {
                text.fg = theme::FG();
            }
            assert_ne!(
                base,
                test_widget_run_cache_key(&primitives, 10.0, 16.0, 800.0, 600.0, 1, 1)
            );
        }

        #[test]
        fn widget_run_cache_key_invalidates_atlas_recreation() {
            let primitives = vec![MetalPrimitive::ProportionalText(
                widget_render::MetalProportionalTextPrimitive {
                    row: 1.0,
                    col: 2.0,
                    align_width: 6.0,
                    h_align: 0.0,
                    text: "Tempo".to_string(),
                    font_size: 12.0,
                    scale: 1.0,
                    fg: theme::FG(),
                    bg: theme::BG(),
                },
            )];

            let base = test_widget_run_cache_key(&primitives, 8.0, 16.0, 800.0, 600.0, 1, 1);
            assert_ne!(
                base,
                test_widget_run_cache_key(&primitives, 8.0, 16.0, 800.0, 600.0, 2, 1)
            );
            assert_ne!(
                base,
                test_widget_run_cache_key(&primitives, 8.0, 16.0, 800.0, 600.0, 1, 2)
            );
        }

        #[test]
        fn widget_run_cache_key_ignores_itime_for_non_animated_widget_instances() {
            let mut primitives = vec![test_widget_instance_primitive("button", 1.0)];
            let base = test_widget_run_cache_key(&primitives, 8.0, 16.0, 800.0, 600.0, 1, 1);
            if let MetalPrimitive::WidgetInstance { instance, .. } = &mut primitives[0] {
                instance.itime = 42.0;
            }

            assert_eq!(
                base,
                test_widget_run_cache_key(&primitives, 8.0, 16.0, 800.0, 600.0, 1, 1)
            );
        }

        #[test]
        fn widget_run_cache_key_keeps_itime_for_animated_widget_instances() {
            widget_render::sdf_widget::register_sdf_widget(
                widget_render::sdf_widget::SdfWidgetDef {
                    name: "test-cache-itime-animated".to_string(),
                    shader_source: String::new(),
                    sdf_expr: crate::parser::Expression::Number(0.0),
                    state_uniforms: Vec::new(),
                    bindable_props: Vec::new(),
                    region_count: 0,
                    width: 1.0,
                    height: 1.0,
                    paint_margin: 0.0,
                    animates: true,
                },
            );

            let mut primitives = vec![test_widget_instance_primitive(
                "test-cache-itime-animated",
                1.0,
            )];
            let base = test_widget_run_cache_key(&primitives, 8.0, 16.0, 800.0, 600.0, 1, 1);
            if let MetalPrimitive::WidgetInstance { instance, .. } = &mut primitives[0] {
                instance.itime = 42.0;
            }

            assert_ne!(
                base,
                test_widget_run_cache_key(&primitives, 8.0, 16.0, 800.0, 600.0, 1, 1)
            );
        }
    }
}

#[cfg(target_os = "macos")]
pub use inner::MetalBackend;
