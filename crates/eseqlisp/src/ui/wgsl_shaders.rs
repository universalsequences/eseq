//! WGSL ports of the core Metal pipelines.
//!
//! Each constant here is the portable counterpart of one MSL source in
//! [`crate::ui::metal_backend`], kept 1:1 with it so the two can be diffed by
//! eye. The translation rules applied throughout:
//!
//! * `[[vertex_id]]`/`[[instance_id]]` become `@builtin(vertex_index)` /
//!   `@builtin(instance_index)`; `[[stage_in]]` becomes a typed struct
//!   parameter; `[[buffer(n)]]` becomes either an instance-step vertex buffer
//!   (per-instance structs) or a `@group(0) @binding(n)` storage buffer
//!   (variable-length sample data).
//! * MSL's `device const T*` instance pointers are fed as vertex attributes
//!   rather than storage buffers. Vertex attributes only require 4-byte offset
//!   alignment, so the existing tightly packed `#[repr(C)]` uploads work
//!   unchanged, whereas WGSL's storage/uniform layout rules would force
//!   16-byte alignment on every `vec4<f32>` and change the Rust structs.
//! * WGSL has no implicit numeric conversions and no `?:`, so every mixed
//!   int/float expression is explicitly converted and every ternary becomes
//!   `select(false_value, true_value, condition)`. `select` evaluates both
//!   arms, so it is only used where both arms are side-effect free and cheap.
//! * Derivative builtins (`fwidth`) must be reached from uniform control flow.
//!   Where MSL took a derivative inside a data-dependent branch, the branch now
//!   computes the value into a `var` and the derivative is taken after it.
//! * Swizzle assignment (`col.rgb = ...`) does not exist in WGSL; those become
//!   whole-vector rebuilds.
//!
//! Inter-stage varyings are packed into `vec2`/`vec3`/`vec4` locations rather
//! than one location per scalar: WebGPU caps inter-stage components, and the
//! per-scalar shape used by MSL would push the waveform pipeline over the top.
//! Each fragment entry point unpacks them back into MSL-named locals on entry
//! so the ported bodies still read like their originals.

/// Monospace text quad — port of `SHADER_SRC`.
///
/// Vertex data is [`crate::ui::gpu_geometry::Vertex`], stepped per vertex.
/// Nearest-neighbour filtering lives in the sampler the pipeline binds, not in
/// the shader as it did with MSL's `constexpr sampler`.
pub const TEXT_SHADER_WGSL: &str = r#"
struct TextVaryings {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) fg: vec4<f32>,
    @location(2) bg: vec4<f32>,
};

@group(0) @binding(0) var atlas: texture_2d<f32>;
@group(0) @binding(1) var atlas_sampler: sampler;

@vertex
fn text_vert(
    @location(0) position: vec2<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) fg: vec4<f32>,
    @location(3) bg: vec4<f32>,
) -> TextVaryings {
    var out: TextVaryings;
    out.position = vec4<f32>(position, 0.0, 1.0);
    out.uv = uv;
    out.fg = fg;
    out.bg = bg;
    return out;
}

@fragment
fn text_frag(input: TextVaryings) -> @location(0) vec4<f32> {
    let coverage = textureSample(atlas, atlas_sampler, input.uv).r;
    return mix(input.bg, input.fg, coverage);
}
"#;

/// Proportional text fragment — port of `PROP_FRAG_SRC`.
///
/// Pairs with `text_vert` above exactly as the MSL pipeline pairs `prop_frag`
/// with `vert`. The linear-vs-nearest difference is a sampler difference, so
/// this module is otherwise identical to the monospace fragment apart from
/// emitting coverage as alpha for ordinary alpha blending.
pub const PROP_TEXT_SHADER_WGSL: &str = r#"
struct TextVaryings {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) fg: vec4<f32>,
    @location(2) bg: vec4<f32>,
};

@group(0) @binding(0) var atlas: texture_2d<f32>;
@group(0) @binding(1) var atlas_sampler: sampler;

@fragment
fn prop_text_frag(input: TextVaryings) -> @location(0) vec4<f32> {
    let coverage = textureSample(atlas, atlas_sampler, input.uv).r;
    // Output foreground color with coverage as alpha. The pipeline uses
    // standard alpha blending (srcAlpha, 1-srcAlpha) so glyphs composite over
    // the background rect without clipping neighbors.
    return vec4<f32>(input.fg.rgb, coverage);
}
"#;

/// Image quad — port of `IMAGE_SHADER_SRC`.
pub const IMAGE_SHADER_WGSL: &str = r#"
struct ImageVertex {
    @location(0) position: vec2<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) opacity: f32,
    @location(3) local_pos: vec2<f32>,
    @location(4) half_size: vec2<f32>,
    @location(5) radius: f32,
    @location(6) rotation: f32,
    @location(7) clip_circle: f32,
};

struct ImageVaryings {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) opacity: f32,
    @location(2) local_pos: vec2<f32>,
    @location(3) half_size: vec2<f32>,
    @location(4) radius: f32,
    @location(5) rotation: f32,
    @location(6) clip_circle: f32,
};

@group(0) @binding(0) var image_tex: texture_2d<f32>;
@group(0) @binding(1) var image_sampler: sampler;

@vertex
fn image_vert(v: ImageVertex) -> ImageVaryings {
    var out: ImageVaryings;
    out.position = vec4<f32>(v.position, 0.0, 1.0);
    out.uv = v.uv;
    out.opacity = v.opacity;
    out.local_pos = v.local_pos;
    out.half_size = v.half_size;
    out.radius = v.radius;
    out.rotation = v.rotation;
    out.clip_circle = v.clip_circle;
    return out;
}

fn image_rounded_rect_sdf(p: vec2<f32>, half_size: vec2<f32>, radius: f32) -> f32 {
    let q = abs(p) - half_size + radius;
    return length(max(q, vec2<f32>(0.0))) + min(max(q.x, q.y), 0.0) - radius;
}

@fragment
fn image_frag(input: ImageVaryings) -> @location(0) vec4<f32> {
    var uv = input.uv;
    if (abs(input.rotation) > 0.0001) {
        let c = cos(input.rotation);
        let sn = sin(input.rotation);
        let p = uv - vec2<f32>(0.5, 0.5);
        uv = vec2<f32>(c * p.x - sn * p.y, sn * p.x + c * p.y) + vec2<f32>(0.5, 0.5);
    }
    var color = textureSample(image_tex, image_sampler, uv);

    // MSL took fwidth(d) inside the clip branches. WGSL needs derivatives in
    // uniform control flow, so the branch only picks the distance and the
    // derivative is taken afterwards.
    var d = 0.0;
    var clipped = false;
    if (input.clip_circle > 0.5) {
        d = length(input.local_pos) - min(input.half_size.x, input.half_size.y);
        clipped = true;
    } else if (input.radius > 0.0) {
        let radius = min(input.radius, min(input.half_size.x, input.half_size.y));
        d = image_rounded_rect_sdf(input.local_pos, input.half_size, radius);
        clipped = true;
    }
    let aa = max(fwidth(d), 0.001);
    if (clipped) {
        color.a = color.a * smoothstep(aa, -aa, d);
    }
    color.a = color.a * input.opacity;
    return color;
}
"#;

/// Patch cable — port of `PATCH_CABLE_SHADER_SRC`.
///
/// `PatchCableInstance` arrives as instance-step vertex attributes; the
/// `packed_float2` pairs are fetched as `vec4<f32>` where they are adjacent so
/// the attribute count stays small.
pub const PATCH_CABLE_SHADER_WGSL: &str = r#"
struct PatchCableInstance {
    // ndc_min.xy, ndc_max.xy
    @location(0) ndc: vec4<f32>,
    // bounds_min.xy, bounds_max.xy
    @location(1) bounds: vec4<f32>,
    // start.xy, control1.xy
    @location(2) start_control1: vec4<f32>,
    // control2.xy, end.xy
    @location(3) control2_end: vec4<f32>,
    @location(4) color: vec4<f32>,
    // radius_px, is_segmented, segment_y_px, corner_radius_px
    @location(5) params: vec4<f32>,
};

struct PatchCableVaryings {
    @builtin(position) position: vec4<f32>,
    @location(0) pixel_pos: vec2<f32>,
    @location(1) @interpolate(flat) start_control1: vec4<f32>,
    @location(2) @interpolate(flat) control2_end: vec4<f32>,
    @location(3) @interpolate(flat) color: vec4<f32>,
    @location(4) @interpolate(flat) params: vec4<f32>,
};

fn patch_cable_quad_corner(vid: u32) -> vec2<f32> {
    switch vid {
        case 0u: { return vec2<f32>(0.0, 0.0); }
        case 1u: { return vec2<f32>(0.0, 1.0); }
        case 2u: { return vec2<f32>(1.0, 0.0); }
        case 3u: { return vec2<f32>(1.0, 0.0); }
        case 4u: { return vec2<f32>(0.0, 1.0); }
        default: { return vec2<f32>(1.0, 1.0); }
    }
}

@vertex
fn patch_cable_vert(
    @builtin(vertex_index) vid: u32,
    cable: PatchCableInstance,
) -> PatchCableVaryings {
    let uv = patch_cable_quad_corner(vid);
    var out: PatchCableVaryings;
    let ndc_pos = mix(cable.ndc.xy, cable.ndc.zw, uv);
    out.position = vec4<f32>(ndc_pos, 0.0, 1.0);
    out.pixel_pos = mix(cable.bounds.xy, cable.bounds.zw, uv);
    out.start_control1 = cable.start_control1;
    out.control2_end = cable.control2_end;
    out.color = cable.color;
    out.params = cable.params;
    return out;
}

fn patch_cable_bezier(p0: vec2<f32>, p1: vec2<f32>, p2: vec2<f32>, p3: vec2<f32>, t: f32) -> vec2<f32> {
    let u = 1.0 - t;
    let tt = t * t;
    let uu = u * u;
    return (uu * u) * p0
        + (3.0 * uu * t) * p1
        + (3.0 * u * tt) * p2
        + (tt * t) * p3;
}

fn patch_cable_curve_distance(
    p: vec2<f32>,
    p0: vec2<f32>,
    p1: vec2<f32>,
    p2: vec2<f32>,
    p3: vec2<f32>,
) -> f32 {
    var min_dist = 1.0e6;
    for (var i = 0; i < 24; i = i + 1) {
        let t1 = f32(i) / 24.0;
        let t2 = f32(i + 1) / 24.0;
        let seg_start = patch_cable_bezier(p0, p1, p2, p3, t1);
        let seg_end = patch_cable_bezier(p0, p1, p2, p3, t2);
        let seg_vec = seg_end - seg_start;
        let seg_len_sq = max(dot(seg_vec, seg_vec), 0.0001);
        let t = clamp(dot(p - seg_start, seg_vec) / seg_len_sq, 0.0, 1.0);
        min_dist = min(min_dist, length(p - (seg_start + t * seg_vec)));
    }
    return min_dist;
}

fn patch_cable_segment_distance(point: vec2<f32>, seg_start: vec2<f32>, seg_end: vec2<f32>) -> f32 {
    let seg_vec = seg_end - seg_start;
    let seg_len_sq = dot(seg_vec, seg_vec);
    if (seg_len_sq < 0.00001) {
        return length(point - seg_start);
    }
    let t = clamp(dot(point - seg_start, seg_vec) / seg_len_sq, 0.0, 1.0);
    return length(point - (seg_start + t * seg_vec));
}

fn patch_cable_arc_distance(point: vec2<f32>, center: vec2<f32>, radius: f32, corner: vec2<f32>) -> f32 {
    let to_corner = corner - center;
    let to_point = point - center;
    let valid_x = select(to_point.x <= 0.0, to_point.x >= 0.0, to_corner.x > 0.0);
    let valid_y = select(to_point.y <= 0.0, to_point.y >= 0.0, to_corner.y > 0.0);
    if (valid_x && valid_y) {
        return abs(length(to_point) - radius);
    }
    return 1000000.0;
}

fn patch_cable_segmented_distance_y_up(
    p: vec2<f32>,
    start: vec2<f32>,
    end: vec2<f32>,
    segment_y: f32,
    corner_radius: f32,
) -> f32 {
    let needs_five = end.y > segment_y;
    if (!needs_five) {
        let going_down1 = start.y > segment_y;
        let going_right = end.x > start.x;
        let going_down2 = end.y < segment_y;
        let corner1 = vec2<f32>(start.x, segment_y);
        let corner2 = vec2<f32>(end.x, segment_y);
        var corner1_center: vec2<f32>;
        var corner2_center: vec2<f32>;
        if (going_down1) {
            corner1_center = select(
                vec2<f32>(start.x - corner_radius, segment_y + corner_radius),
                vec2<f32>(start.x + corner_radius, segment_y + corner_radius),
                going_right);
        } else {
            corner1_center = select(
                vec2<f32>(start.x - corner_radius, segment_y - corner_radius),
                vec2<f32>(start.x + corner_radius, segment_y - corner_radius),
                going_right);
        }
        if (going_down2) {
            corner2_center = select(
                vec2<f32>(end.x + corner_radius, segment_y - corner_radius),
                vec2<f32>(end.x - corner_radius, segment_y - corner_radius),
                going_right);
        } else {
            corner2_center = select(
                vec2<f32>(end.x + corner_radius, segment_y + corner_radius),
                vec2<f32>(end.x - corner_radius, segment_y + corner_radius),
                going_right);
        }
        let seg1_end = vec2<f32>(start.x, select(segment_y - corner_radius, segment_y + corner_radius, going_down1));
        let seg3_start = vec2<f32>(end.x, select(segment_y + corner_radius, segment_y - corner_radius, going_down2));
        let seg2_start = vec2<f32>(select(start.x - corner_radius, start.x + corner_radius, going_right), segment_y);
        let seg2_end = vec2<f32>(select(end.x + corner_radius, end.x - corner_radius, going_right), segment_y);
        return min(
            min(min(patch_cable_segment_distance(p, start, seg1_end), patch_cable_segment_distance(p, seg2_start, seg2_end)),
                min(patch_cable_segment_distance(p, seg3_start, end), patch_cable_arc_distance(p, corner1_center, corner_radius, corner1))),
            patch_cable_arc_distance(p, corner2_center, corner_radius, corner2));
    }

    let going_right = end.x > start.x;
    let clearance = corner_radius * 2.0;
    let turnaround_y = end.y + clearance;
    let turnaround_x = end.x - clearance;
    let seg4_going_right = end.x > turnaround_x;
    let corner1 = vec2<f32>(start.x, segment_y);
    let corner1_center = select(
        vec2<f32>(start.x - corner_radius, segment_y + corner_radius),
        vec2<f32>(start.x + corner_radius, segment_y + corner_radius),
        going_right);
    let seg1_end = vec2<f32>(start.x, segment_y + corner_radius);
    let corner2 = vec2<f32>(turnaround_x, segment_y);
    let corner2_center = select(
        vec2<f32>(turnaround_x + corner_radius, segment_y + corner_radius),
        vec2<f32>(turnaround_x - corner_radius, segment_y + corner_radius),
        going_right);
    let seg2_start = vec2<f32>(select(start.x - corner_radius, start.x + corner_radius, going_right), segment_y);
    let seg2_end = vec2<f32>(select(turnaround_x + corner_radius, turnaround_x - corner_radius, going_right), segment_y);
    let corner3 = vec2<f32>(turnaround_x, turnaround_y);
    let corner3_center = select(
        vec2<f32>(turnaround_x - corner_radius, turnaround_y - corner_radius),
        vec2<f32>(turnaround_x + corner_radius, turnaround_y - corner_radius),
        seg4_going_right);
    let seg3_start = vec2<f32>(turnaround_x, segment_y + corner_radius);
    let seg3_end = vec2<f32>(turnaround_x, turnaround_y - corner_radius);
    let corner4 = vec2<f32>(end.x, turnaround_y);
    let corner4_center = select(
        vec2<f32>(end.x + corner_radius, turnaround_y - corner_radius),
        vec2<f32>(end.x - corner_radius, turnaround_y - corner_radius),
        seg4_going_right);
    let seg4_start = vec2<f32>(select(turnaround_x - corner_radius, turnaround_x + corner_radius, seg4_going_right), turnaround_y);
    let seg4_end = vec2<f32>(select(end.x + corner_radius, end.x - corner_radius, seg4_going_right), turnaround_y);
    let seg5_start = vec2<f32>(end.x, turnaround_y - corner_radius);
    let min_seg = min(
        min(min(patch_cable_segment_distance(p, start, seg1_end), patch_cable_segment_distance(p, seg2_start, seg2_end)),
            min(patch_cable_segment_distance(p, seg3_start, seg3_end), patch_cable_segment_distance(p, seg4_start, seg4_end))),
        patch_cable_segment_distance(p, seg5_start, end));
    let min_corner = min(
        min(patch_cable_arc_distance(p, corner1_center, corner_radius, corner1),
            patch_cable_arc_distance(p, corner2_center, corner_radius, corner2)),
        min(patch_cable_arc_distance(p, corner3_center, corner_radius, corner3),
            patch_cable_arc_distance(p, corner4_center, corner_radius, corner4)));
    return min(min_seg, min_corner);
}

fn patch_cable_segmented_distance(
    p: vec2<f32>,
    start: vec2<f32>,
    end: vec2<f32>,
    segment_y_px: f32,
    corner_radius: f32,
) -> f32 {
    return patch_cable_segmented_distance_y_up(
        vec2<f32>(p.x, -p.y),
        vec2<f32>(start.x, -start.y),
        vec2<f32>(end.x, -end.y),
        -segment_y_px,
        corner_radius);
}

@fragment
fn patch_cable_frag(input: PatchCableVaryings) -> @location(0) vec4<f32> {
    let start = input.start_control1.xy;
    let control1 = input.start_control1.zw;
    let control2 = input.control2_end.xy;
    let end = input.control2_end.zw;
    let radius_px = input.params.x;
    let is_segmented = input.params.y;
    let segment_y_px = input.params.z;
    let corner_radius_px = input.params.w;

    // A `select` here would evaluate both distance fields; branch instead and
    // take the derivative below, in uniform control flow.
    var min_dist_to_line: f32;
    if (is_segmented > 0.5) {
        min_dist_to_line = patch_cable_segmented_distance(
            input.pixel_pos,
            start,
            end,
            segment_y_px,
            corner_radius_px);
    } else {
        min_dist_to_line = patch_cable_curve_distance(
            input.pixel_pos,
            start,
            control1,
            control2,
            end);
    }

    let sdf = min_dist_to_line - radius_px;
    let derivative = max(fwidth(sdf), 0.0001);
    let alpha = smoothstep(derivative * 0.5, -derivative * 0.5, sdf);

    if (alpha <= 0.0) {
        discard;
    }

    let core_radius = radius_px * 0.48;
    let core_blend = 1.0 - smoothstep(
        max(core_radius - derivative, 0.0),
        core_radius + derivative,
        min_dist_to_line);
    let edge_color = input.color.rgb * 0.58;
    let core_color = mix(input.color.rgb, vec3<f32>(1.0, 1.0, 1.0), 0.78);
    let cable_color = mix(edge_color, core_color, core_blend);
    let edge_alpha_scale = 0.62;
    let alpha_scale = mix(edge_alpha_scale, 1.0, core_blend);

    return vec4<f32>(cable_color, input.color.a * alpha * alpha_scale);
}
"#;

/// Shared widget preamble — port of `WIDGET_SHADER_PREAMBLE`.
///
/// Every widget pipeline is assembled as
/// `WIDGET_SHADER_PREAMBLE_WGSL + vertex + fragment`, exactly as the Metal
/// backend concatenates its MSL counterpart. The preamble owns the three
/// shared declarations so no per-widget fragment has to repeat them:
///
/// * `WidgetInstance` — the vertex-stage view of
///   [`crate::widget_render::WidgetInstance`], one attribute per `#[repr(C)]`
///   field so the tightly packed upload needs no re-layout. Fifteen attributes
///   fits under WebGPU's 16-attribute floor.
/// * `WidgetVaryings` — the fragment-stage input. Its layout is fixed by
///   [`crate::lang::sdf_codegen`], which emits `widget_frag` bodies against
///   exactly these names and locations.
/// * `WidgetVertexOutput` — the vertex-stage output. WGSL cannot nest an IO
///   struct, so this repeats `WidgetVaryings`' locations with
///   `@builtin(position)` in front; the two are matched by location, not by
///   type.
pub const WIDGET_SHADER_PREAMBLE_WGSL: &str = r#"
struct WidgetInstance {
    @location(0) ndc_min: vec2<f32>,
    @location(1) ndc_max: vec2<f32>,
    @location(2) value_t: f32,
    @location(3) orientation: f32,
    @location(4) itime: f32,
    @location(5) uniform_a: vec4<f32>,
    @location(6) uniform_b: vec4<f32>,
    @location(7) uniform_c: vec4<f32>,
    @location(8) uniform_d: vec4<f32>,
    @location(9) color_a: vec4<f32>,
    @location(10) color_b: vec4<f32>,
    @location(11) color_c: vec4<f32>,
    @location(12) color_d: vec4<f32>,
    @location(13) corner_radius: f32,
    @location(14) pixel_aspect: f32,
};

struct WidgetVaryings {
    @location(0) uv: vec2<f32>,
    @location(1) @interpolate(flat) value_t: f32,
    @location(2) @interpolate(flat) itime: f32,
    @location(3) @interpolate(flat) uniform_a: vec4<f32>,
    @location(4) @interpolate(flat) uniform_b: vec4<f32>,
    @location(5) @interpolate(flat) uniform_c: vec4<f32>,
    @location(6) @interpolate(flat) uniform_d: vec4<f32>,
    @location(7) @interpolate(flat) color_a: vec4<f32>,
    @location(8) @interpolate(flat) color_b: vec4<f32>,
    @location(9) @interpolate(flat) color_c: vec4<f32>,
    @location(10) @interpolate(flat) color_d: vec4<f32>,
    @location(11) @interpolate(flat) aspect: f32,
    @location(12) @interpolate(flat) corner_radius: f32,
};

struct WidgetVertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) @interpolate(flat) value_t: f32,
    @location(2) @interpolate(flat) itime: f32,
    @location(3) @interpolate(flat) uniform_a: vec4<f32>,
    @location(4) @interpolate(flat) uniform_b: vec4<f32>,
    @location(5) @interpolate(flat) uniform_c: vec4<f32>,
    @location(6) @interpolate(flat) uniform_d: vec4<f32>,
    @location(7) @interpolate(flat) color_a: vec4<f32>,
    @location(8) @interpolate(flat) color_b: vec4<f32>,
    @location(9) @interpolate(flat) color_c: vec4<f32>,
    @location(10) @interpolate(flat) color_d: vec4<f32>,
    @location(11) @interpolate(flat) aspect: f32,
    @location(12) @interpolate(flat) corner_radius: f32,
};

fn sdf_rounded_rect(p: vec2<f32>, half_size: vec2<f32>, radius: f32) -> f32 {
    let d = abs(p) - half_size + radius;
    return length(max(d, vec2<f32>(0.0))) + min(max(d.x, d.y), 0.0) - radius;
}

// MSL passed the outer mask back through a `thread float&` out-parameter;
// WGSL's function-address-space pointer is the direct equivalent. Call it as
//   var outer_mask: f32;
//   let border = compute_border_mask(p, size, r, 1.0, &outer_mask);
fn compute_border_mask(
    localPos: vec2<f32>,
    outerSize: vec2<f32>,
    cornerRadius: f32,
    borderPixels: f32,
    outerMask: ptr<function, f32>,
) -> f32 {
    let outerDist = sdf_rounded_rect(localPos, outerSize, cornerRadius);
    let outerDeriv = max(fwidth(outerDist), 0.001);
    let borderThickness = borderPixels * outerDeriv;
    let innerSize = outerSize - vec2<f32>(borderThickness);
    let innerDist = sdf_rounded_rect(localPos, innerSize, max(cornerRadius - borderThickness, 0.0));
    let innerDeriv = max(fwidth(innerDist), 0.001);
    *outerMask = smoothstep(outerDeriv, -outerDeriv, outerDist);
    let innerMask = smoothstep(innerDeriv, -innerDeriv, innerDist);
    return *outerMask * (1.0 - innerMask);
}
"#;

/// Default widget vertex stage — port of `DEFAULT_WIDGET_VERTEX_SHADER`.
pub const DEFAULT_WIDGET_VERTEX_SHADER_WGSL: &str = r#"
@vertex
fn widget_vert(
    @builtin(vertex_index) vid: u32,
    inst: WidgetInstance,
) -> WidgetVertexOutput {
    let corners = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0), vec2<f32>(0.0, 1.0), vec2<f32>(1.0, 0.0),
        vec2<f32>(1.0, 0.0), vec2<f32>(0.0, 1.0), vec2<f32>(1.0, 1.0),
    );
    let corner = corners[vid];
    let ndc = mix(inst.ndc_min, inst.ndc_max, corner);

    var out: WidgetVertexOutput;
    out.position = vec4<f32>(ndc, 0.0, 1.0);
    out.uv = corner;
    out.value_t = inst.value_t;
    out.itime = inst.itime;
    out.uniform_a = inst.uniform_a;
    out.uniform_b = inst.uniform_b;
    out.uniform_c = inst.uniform_c;
    out.uniform_d = inst.uniform_d;
    out.color_a = inst.color_a;
    out.color_b = inst.color_b;
    out.color_c = inst.color_c;
    out.color_d = inst.color_d;
    out.aspect = inst.pixel_aspect;
    out.corner_radius = inst.corner_radius;
    return out;
}
"#;

/// Wavetable scope — port of `WAVETABLE_SHADER_SRC`.
///
/// The MSL vertex stage read `instances[0]`; here the same single struct is
/// bound as an instance-step vertex buffer and drawn with one instance.
pub const WAVETABLE_SHADER_WGSL: &str = r#"
struct WavetableInstance {
    // ndc_min.xy, ndc_max.xy
    @location(0) ndc: vec4<f32>,
    @location(1) widget_px: vec2<f32>,
    @location(2) frame_len: u32,
    @location(3) set_base: u32,
    @location(4) waves_in_set: u32,
    // wave_pos, warp, fold
    @location(5) morph: vec3<f32>,
    @location(6) domain: u32,
    @location(7) selected_color: vec4<f32>,
    @location(8) inactive_color: vec4<f32>,
    @location(9) bg_color: vec4<f32>,
};

struct WavetableVaryings {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) @interpolate(flat) widget_px: vec2<f32>,
    // frame_len, set_base, waves_in_set, domain
    @location(2) @interpolate(flat) counts: vec4<u32>,
    @location(3) @interpolate(flat) morph: vec3<f32>,
    @location(4) @interpolate(flat) selected_color: vec4<f32>,
    @location(5) @interpolate(flat) inactive_color: vec4<f32>,
    @location(6) @interpolate(flat) bg_color: vec4<f32>,
};

@group(0) @binding(0) var<storage, read> bank: array<f32>;

@vertex
fn wavetable_vert(
    @builtin(vertex_index) vid: u32,
    inst: WavetableInstance,
) -> WavetableVaryings {
    let corners = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0), vec2<f32>(0.0, 1.0), vec2<f32>(1.0, 0.0),
        vec2<f32>(1.0, 0.0), vec2<f32>(0.0, 1.0), vec2<f32>(1.0, 1.0),
    );
    let corner = corners[vid];
    let ndc = mix(inst.ndc.xy, inst.ndc.zw, corner);

    var out: WavetableVaryings;
    out.position = vec4<f32>(ndc, 0.0, 1.0);
    out.uv = corner; // uv.y is up: (0,0) = bottom-left of the widget
    out.widget_px = inst.widget_px;
    out.counts = vec4<u32>(inst.frame_len, inst.set_base, inst.waves_in_set, inst.domain);
    out.morph = inst.morph;
    out.selected_color = inst.selected_color;
    out.inactive_color = inst.inactive_color;
    out.bg_color = inst.bg_color;
    return out;
}

// Mobius phase bend — keep in sync with the wavetable dsp.lisp warp math.
fn wt_warp_phase(p: f32, warp: f32) -> f32 {
    let k = 1.0 + 6.0 * warp;
    return (k * p) / (1.0 + (k - 1.0) * p);
}

// Triangle wavefolder — keep in sync with the wavetable dsp.lisp fold math.
fn wt_fold(y: f32, fold: f32) -> f32 {
    let g = 1.0 + 6.0 * fold;
    var v = (y * g + 1.0) % 4.0;
    if (v < 0.0) { v = v + 4.0; }
    return 1.0 - abs(v - 2.0);
}

// Reads the module-scope `bank` storage buffer: WGSL cannot take a storage
// pointer as a function parameter the way MSL's `device const float*` did.
fn wt_sample(base: u32, frame_len: u32, phase: f32, warp: f32, fold: f32, domain: u32) -> f32 {
    let clamped = clamp(phase, 0.0, 1.0);
    let p = select(wt_warp_phase(clamped, warp), clamped, domain == 1u);
    let pos = p * f32(frame_len - select(0u, 1u, domain == 1u));
    let i0 = min(u32(pos), frame_len - 1u);
    let i1 = select((i0 + 1u) % frame_len, min(i0 + 1u, frame_len - 1u), domain == 1u);
    let y = mix(bank[base + i0], bank[base + i1], pos - f32(i0));
    return select(wt_fold(y, fold), max(y, 0.0), domain == 1u);
}

@fragment
fn wavetable_frag(input: WavetableVaryings) -> @location(0) vec4<f32> {
    let PAD_X = 0.03;
    let PAD_Y = 0.10;

    let frame_len = input.counts.x;
    let set_base = input.counts.y;
    let waves_in_set = input.counts.z;
    let domain = input.counts.w;
    let wave_pos = input.morph.x;
    let warp = input.morph.y;
    let fold = input.morph.z;
    let widget_px_w = input.widget_px.x;
    let widget_px_h = input.widget_px.y;

    let n = max(waves_in_set, 1u);
    let display_n = min(n, 16u);
    let plot_h = 1.0 - PAD_Y * 2.0;
    let amp_half = plot_h / f32(max(display_n, 2u)) * 0.85;
    let plot_px_w = widget_px_w * (1.0 - PAD_X * 2.0);

    let u_raw = (input.uv.x - PAD_X) / (1.0 - PAD_X * 2.0);
    let in_plot = u_raw >= 0.0 && u_raw <= 1.0;
    let u = clamp(u_raw, 0.0, 1.0);
    let du = max(fwidth(u), 1e-5);

    var col = input.bg_color;

    if (in_plot && frame_len >= 2u) {
        // ── inactive waves: wave 0 at the bottom, last at the top ──
        var inactive_acc = 0.0;
        for (var row = 0u; row < display_n; row = row + 1u) {
            let t = select(0.5, f32(row) / f32(max(display_n - 1u, 1u)), display_n > 1u);
            let w = select(0u, u32(round(t * f32(n - 1u))), n > 1u);
            let rowc = PAD_Y + plot_h * t;
            let base = (set_base + w) * frame_len;
            let s0 = wt_sample(base, frame_len, u, warp, fold, domain);
            let s1 = wt_sample(base, frame_len, u + du, warp, fold, domain);
            let y0_px = (rowc + s0 * amp_half) * widget_px_h;
            let dy_px = input.uv.y * widget_px_h - y0_px;
            let slope = (s1 - s0) * amp_half * widget_px_h / (du * plot_px_w);
            let d = abs(dy_px) / sqrt(1.0 + slope * slope);
            inactive_acc = max(inactive_acc, 1.0 - smoothstep(0.45, 1.35, d));
        }
        col = vec4<f32>(
            mix(col.rgb, input.inactive_color.rgb, inactive_acc * input.inactive_color.a),
            max(col.a, inactive_acc * input.inactive_color.a));

        // ── selected wave: morph-interpolated at the fractional position ──
        let pos = clamp(wave_pos, 0.0, f32(n - 1u));
        let w0 = u32(pos);
        let w1 = min(w0 + 1u, n - 1u);
        let ft = pos - f32(w0);
        let t = select(0.5, pos / f32(max(n - 1u, 1u)), n > 1u);
        let rowc = PAD_Y + plot_h * t;
        let b0 = (set_base + w0) * frame_len;
        let b1 = (set_base + w1) * frame_len;
        let s0 = mix(wt_sample(b0, frame_len, u, warp, fold, domain),
                     wt_sample(b1, frame_len, u, warp, fold, domain), ft);
        let s1 = mix(wt_sample(b0, frame_len, u + du, warp, fold, domain),
                     wt_sample(b1, frame_len, u + du, warp, fold, domain), ft);
        let y0_px = (rowc + s0 * amp_half) * widget_px_h;
        let dy_px = input.uv.y * widget_px_h - y0_px;
        let slope = (s1 - s0) * amp_half * widget_px_h / (du * plot_px_w);
        let d = abs(dy_px) / sqrt(1.0 + slope * slope);
        // soft dark halo behind the selected line so it reads over the grays
        let halo = 1.0 - smoothstep(1.2, 3.2, d);
        let haloed = mix(col.rgb, input.bg_color.rgb, halo * 0.65);
        let line = 1.0 - smoothstep(0.85, 2.0, d);
        col = vec4<f32>(
            mix(haloed, input.selected_color.rgb, line * input.selected_color.a),
            max(col.a, line * input.selected_color.a));
    }
    return col;
}
"#;

/// Sample waveform — port of `WAVEFORM_SHADER_SRC`.
pub const WAVEFORM_SHADER_WGSL: &str = r#"
struct WaveformInstance {
    // ndc_min.xy, ndc_max.xy
    @location(0) ndc: vec4<f32>,
    // sample_start, sample_end
    @location(1) sample_range: vec2<f32>,
    @location(2) bucket_count: u32,
    // aspect_ratio, selection_start, selection_end
    @location(3) aspect_and_selection: vec3<f32>,
    @location(4) show_selection: vec2<i32>,
    @location(5) playhead_position: f32,
    @location(6) show_playhead: i32,
    @location(7) waveform_color: vec4<f32>,
    @location(8) inactive_waveform_color: vec4<f32>,
    @location(9) marker_color: vec4<f32>,
    @location(10) active_marker_color: vec4<f32>,
    @location(11) active_selection: vec2<i32>,
    @location(12) selection_color: vec4<f32>,
    @location(13) bg_color: vec4<f32>,
    @location(14) border_color: vec4<f32>,
};

struct WaveformVaryings {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    // sample_start, sample_end, selection_start, selection_end
    @location(1) @interpolate(flat) ranges: vec4<f32>,
    // aspect_ratio, playhead_position
    @location(2) @interpolate(flat) misc: vec2<f32>,
    @location(3) @interpolate(flat) bucket_count: u32,
    // show_selection_start, show_selection_end, show_playhead, active_selection_start
    @location(4) @interpolate(flat) flags: vec4<i32>,
    @location(5) @interpolate(flat) active_selection_end: i32,
    @location(6) @interpolate(flat) waveform_color: vec4<f32>,
    @location(7) @interpolate(flat) inactive_waveform_color: vec4<f32>,
    @location(8) @interpolate(flat) marker_color: vec4<f32>,
    @location(9) @interpolate(flat) active_marker_color: vec4<f32>,
    @location(10) @interpolate(flat) selection_color: vec4<f32>,
    @location(11) @interpolate(flat) bg_color: vec4<f32>,
    @location(12) @interpolate(flat) border_color: vec4<f32>,
};

@group(0) @binding(0) var<storage, read> waveform_data: array<f32>;

@vertex
fn waveform_vert(
    @builtin(vertex_index) vid: u32,
    inst: WaveformInstance,
) -> WaveformVaryings {
    let corners = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0), vec2<f32>(0.0, 1.0), vec2<f32>(1.0, 0.0),
        vec2<f32>(1.0, 0.0), vec2<f32>(0.0, 1.0), vec2<f32>(1.0, 1.0),
    );
    let corner = corners[vid];
    let ndc = mix(inst.ndc.xy, inst.ndc.zw, corner);

    var out: WaveformVaryings;
    out.position = vec4<f32>(ndc, 0.0, 1.0);
    out.uv = corner;
    out.ranges = vec4<f32>(
        inst.sample_range.x,
        inst.sample_range.y,
        inst.aspect_and_selection.y,
        inst.aspect_and_selection.z);
    out.misc = vec2<f32>(inst.aspect_and_selection.x, inst.playhead_position);
    out.bucket_count = inst.bucket_count;
    out.flags = vec4<i32>(
        inst.show_selection.x,
        inst.show_selection.y,
        inst.show_playhead,
        inst.active_selection.x);
    out.active_selection_end = inst.active_selection.y;
    out.waveform_color = inst.waveform_color;
    out.inactive_waveform_color = inst.inactive_waveform_color;
    out.marker_color = inst.marker_color;
    out.active_marker_color = inst.active_marker_color;
    out.selection_color = inst.selection_color;
    out.bg_color = inst.bg_color;
    out.border_color = inst.border_color;
    return out;
}

@fragment
fn waveform_frag(input: WaveformVaryings) -> @location(0) vec4<f32> {
    let sample_start = input.ranges.x;
    let sample_end = input.ranges.y;
    let selection_start = input.ranges.z;
    let selection_end = input.ranges.w;
    let aspect_ratio = input.misc.x;
    let playhead_position = input.misc.y;
    let bucket_count = input.bucket_count;
    let show_selection_start = input.flags.x;
    let show_selection_end = input.flags.y;
    let show_playhead = input.flags.z;
    let active_selection_start = input.flags.w;
    let active_selection_end = input.active_selection_end;

    if (bucket_count < 2u) {
        discard;
    }

    let content_uv = input.uv;
    // Hoisted so the playhead's antialiasing width is not a derivative taken
    // inside `if (show_playhead == 1)`.
    let d_uv_x = fwidth(content_uv.x);
    let d_uv_y = fwidth(content_uv.y);

    var rgb = input.bg_color.rgb;
    var alpha = 0.0;

    let has_selection = selection_end > selection_start + 0.001;
    if (has_selection &&
        content_uv.x >= selection_start &&
        content_uv.x <= selection_end) {
        rgb = mix(rgb, input.selection_color.rgb, 0.30);
        alpha = max(alpha, 0.06);
    }

    let center_line = 1.0 - smoothstep(0.0, 0.004, abs(content_uv.y - 0.5));
    let center_rgb = mix(input.bg_color.rgb, input.border_color.rgb, 0.5);
    rgb = mix(rgb, center_rgb, center_line * 0.20);
    alpha = max(alpha, center_line * 0.18);

    let boundary_width = max(d_uv_x * 0.9, 0.0015);
    let boundary_aa = max(d_uv_x * 0.75, 0.00075);
    let start_dist = abs(content_uv.x - selection_start);
    let end_dist = abs(content_uv.x - selection_end);
    let start_boundary = select(
        0.0,
        1.0 - smoothstep(boundary_width, boundary_width + boundary_aa, start_dist),
        has_selection && show_selection_start == 1);
    let end_boundary = select(
        0.0,
        1.0 - smoothstep(boundary_width, boundary_width + boundary_aa, end_dist),
        has_selection && show_selection_end == 1);
    let start_marker_rgb = select(
        input.marker_color.rgb,
        input.active_marker_color.rgb,
        active_selection_start == 1);
    let end_marker_rgb = select(
        input.marker_color.rgb,
        input.active_marker_color.rgb,
        active_selection_end == 1);
    rgb = mix(rgb, start_marker_rgb, start_boundary * 0.85);
    rgb = mix(rgb, end_marker_rgb, end_boundary * 0.85);
    let boundary_mask = max(start_boundary, end_boundary);
    alpha = max(alpha, boundary_mask * 0.75);

    let flag_height = 0.1575;
    let flag_width = flag_height / max(aspect_ratio, 0.0001);
    let flag_y = 1.0 - content_uv.y;
    let flag_taper = 1.0 - clamp(flag_y / flag_height, 0.0, 1.0);
    let start_flag_dx = content_uv.x - selection_start;
    let start_flag = select(0.0, 1.0,
        has_selection && show_selection_start == 1 && flag_y <= flag_height &&
        start_flag_dx >= 0.0 && start_flag_dx <= flag_width * flag_taper);
    let end_flag_dx = selection_end - content_uv.x;
    let end_flag = select(0.0, 1.0,
        has_selection && show_selection_end == 1 && flag_y <= flag_height &&
        end_flag_dx >= 0.0 && end_flag_dx <= flag_width * flag_taper);
    let flag_mask = max(start_flag, end_flag);
    rgb = mix(rgb, start_marker_rgb, start_flag);
    rgb = mix(rgb, end_marker_rgb, end_flag);
    alpha = max(alpha, flag_mask);

    let sample_t = clamp(mix(sample_start, sample_end, content_uv.x), 0.0, 1.0);
    let exact_idx = sample_t * f32(bucket_count - 1u);
    let pixel_span = max(fwidth(exact_idx), 1.0);
    let idx_left = clamp(exact_idx - pixel_span * 0.5, 0.0, f32(bucket_count - 1u));
    let idx_right = clamp(exact_idx + pixel_span * 0.5, 0.0, f32(bucket_count - 1u));

    let last_bucket = i32(bucket_count - 1u);
    let idx_a = clamp(i32(floor(exact_idx)), 0, last_bucket);
    let idx_b = min(idx_a + 1, last_bucket);
    let idx_la = clamp(i32(floor(idx_left)), 0, last_bucket);
    let idx_lb = min(idx_la + 1, last_bucket);
    let idx_ra = clamp(i32(floor(idx_right)), 0, last_bucket);
    let idx_rb = min(idx_ra + 1, last_bucket);

    let frac = fract(exact_idx);
    let frac_left = fract(idx_left);
    let frac_right = fract(idx_right);

    let min_center = mix(waveform_data[idx_a * 2], waveform_data[idx_b * 2], frac);
    let max_center = mix(waveform_data[idx_a * 2 + 1], waveform_data[idx_b * 2 + 1], frac);
    let min_left = mix(waveform_data[idx_la * 2], waveform_data[idx_lb * 2], frac_left);
    let max_left = mix(waveform_data[idx_la * 2 + 1], waveform_data[idx_lb * 2 + 1], frac_left);
    let min_right = mix(waveform_data[idx_ra * 2], waveform_data[idx_rb * 2], frac_right);
    let max_right = mix(waveform_data[idx_ra * 2 + 1], waveform_data[idx_rb * 2 + 1], frac_right);

    let min_val = min(min_center, min(min_left, min_right));
    let max_val = max(max_center, max(max_left, max_right));
    let amplitude = clamp(max(abs(min_val), abs(max_val)), 0.0, 1.0);
    var y_min = 0.5 - amplitude * 0.5;
    var y_max = 0.5 + amplitude * 0.5;

    let min_thickness = 0.010;
    if (y_max - y_min < min_thickness) {
        let center = (y_min + y_max) * 0.5;
        y_min = center - min_thickness * 0.5;
        y_max = center + min_thickness * 0.5;
    }

    let edge_aa = max(length(vec2<f32>(d_uv_x, d_uv_y)) * 1.5, 0.002);
    let above_min = smoothstep(y_min - edge_aa, y_min + edge_aa, content_uv.y);
    let below_max = smoothstep(y_max + edge_aa, y_max - edge_aa, content_uv.y);
    let fill_alpha = above_min * below_max;

    let upper_edge = 1.0 - smoothstep(0.0, edge_aa * 1.5, abs(content_uv.y - y_max));
    let lower_edge = 1.0 - smoothstep(0.0, edge_aa * 1.5, abs(content_uv.y - y_min));
    let edge_alpha = max(upper_edge, lower_edge);

    let in_selection = has_selection &&
        content_uv.x >= selection_start &&
        content_uv.x <= selection_end;
    let wave_color = select(input.inactive_waveform_color.rgb, input.waveform_color.rgb, in_selection);
    let fill_color = mix(rgb, wave_color, 0.88);
    let edge_color = mix(wave_color, vec3<f32>(1.0, 1.0, 1.0), 0.15);
    rgb = mix(rgb, fill_color, fill_alpha);
    rgb = mix(rgb, edge_color, edge_alpha * 0.9);
    alpha = max(alpha, fill_alpha);
    alpha = max(alpha, edge_alpha * 0.9);

    if (show_playhead == 1) {
        let playhead_dist = abs(content_uv.x - playhead_position);
        let playhead_width = 0.003;
        let playhead_aa = max(d_uv_x * 1.5, 0.001);
        let playhead_alpha = 1.0 - smoothstep(playhead_width - playhead_aa, playhead_width + playhead_aa, playhead_dist);
        let playhead_overlaps_selection_boundary =
            has_selection && (playhead_dist <= boundary_width + boundary_aa) &&
            ((show_selection_start == 1 &&
              abs(playhead_position - selection_start) <= boundary_width + boundary_aa) ||
             (show_selection_end == 1 &&
              abs(playhead_position - selection_end) <= boundary_width + boundary_aa));
        let playhead_color = select(
            vec3<f32>(0.2, 0.9, 1.0),
            input.selection_color.rgb,
            playhead_overlaps_selection_boundary);
        let playhead_mix = select(
            playhead_alpha * 0.95,
            max(playhead_alpha, boundary_mask),
            playhead_overlaps_selection_boundary);
        rgb = mix(rgb, playhead_color, playhead_mix);
        alpha = max(alpha, playhead_mix);
    }

    let border = min(min(content_uv.x, 1.0 - content_uv.x), min(content_uv.y, 1.0 - content_uv.y));
    let border_mask = 1.0 - smoothstep(0.0, 0.004, border);
    rgb = mix(rgb, input.border_color.rgb, border_mask * 0.8);
    alpha = max(alpha, border_mask * 0.7);

    if (alpha < 0.001) {
        discard;
    }
    return vec4<f32>(rgb, alpha);
}
"#;

/// Live spectrogram / EQ spectrum — port of `LIVE_SPECTROGRAM_SHADER_SRC`.
///
/// MSL passed `waterfall` and `smoothed` into the sampling helpers as
/// `device const float*` parameters. WGSL forbids storage pointers in function
/// parameters, so both buffers are module-scope and each helper is specialised
/// to the one it was only ever called with: `sample_bin`/`sample_bin_range`
/// read the waterfall history, `sample_bin_cubic` reads the smoothed spectrum.
pub const LIVE_SPECTROGRAM_SHADER_WGSL: &str = r#"
struct LiveSpectrogramInstance {
    // ndc_min.xy, ndc_max.xy
    @location(0) ndc: vec4<f32>,
    @location(1) widget_px: vec2<f32>,
    // bins, time_slices, write_head, mode
    @location(2) counts: vec4<u32>,
    @location(3) freq_scale: u32,
    @location(4) sample_rate: f32,
    @location(5) display_hz: vec2<f32>,
    @location(6) min_color: vec4<f32>,
    @location(7) mid_color: vec4<f32>,
    @location(8) max_color: vec4<f32>,
    @location(9) eq_line_color: vec4<f32>,
    @location(10) eq_fill_color: vec4<f32>,
    @location(11) background_color: vec4<f32>,
};

struct LiveSpectrogramVaryings {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) @interpolate(flat) widget_px: vec2<f32>,
    // bins, time_slices, write_head, mode
    @location(2) @interpolate(flat) counts: vec4<u32>,
    @location(3) @interpolate(flat) freq_scale: u32,
    // sample_rate, min_hz, max_hz
    @location(4) @interpolate(flat) rates: vec3<f32>,
    @location(5) @interpolate(flat) min_color: vec4<f32>,
    @location(6) @interpolate(flat) mid_color: vec4<f32>,
    @location(7) @interpolate(flat) max_color: vec4<f32>,
    @location(8) @interpolate(flat) eq_line_color: vec4<f32>,
    @location(9) @interpolate(flat) eq_fill_color: vec4<f32>,
    @location(10) @interpolate(flat) background_color: vec4<f32>,
};

@group(0) @binding(0) var<storage, read> waterfall: array<f32>;
@group(0) @binding(1) var<storage, read> smoothed: array<f32>;

@vertex
fn live_spectrogram_vert(
    @builtin(vertex_index) vid: u32,
    inst: LiveSpectrogramInstance,
) -> LiveSpectrogramVaryings {
    let corners = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0), vec2<f32>(0.0, 1.0), vec2<f32>(1.0, 0.0),
        vec2<f32>(1.0, 0.0), vec2<f32>(0.0, 1.0), vec2<f32>(1.0, 1.0),
    );
    let corner = corners[vid];
    let ndc = mix(inst.ndc.xy, inst.ndc.zw, corner);

    var out: LiveSpectrogramVaryings;
    out.position = vec4<f32>(ndc, 0.0, 1.0);
    out.uv = corner;
    out.widget_px = inst.widget_px;
    out.counts = inst.counts;
    out.freq_scale = inst.freq_scale;
    out.rates = vec3<f32>(inst.sample_rate, inst.display_hz.x, inst.display_hz.y);
    out.min_color = inst.min_color;
    out.mid_color = inst.mid_color;
    out.max_color = inst.max_color;
    out.eq_line_color = inst.eq_line_color;
    out.eq_fill_color = inst.eq_fill_color;
    out.background_color = inst.background_color;
    return out;
}

fn spectrogram_bin_for_uv(
    freq_t: f32,
    freq_scale: u32,
    bins: u32,
    sample_rate: f32,
    min_hz: f32,
    max_hz: f32,
) -> f32 {
    let max_bin = f32(max(bins, 1u) - 1u);
    if (freq_scale == 1u || sample_rate <= 1.0 || bins < 2u) {
        return clamp(freq_t, 0.0, 1.0) * max_bin;
    }
    let nyquist = max(sample_rate * 0.5, 160.0);
    let hi_hz = clamp(max_hz, 2.0, nyquist);
    let lo_hz = clamp(min_hz, 1.0, hi_hz * 0.5);
    let hz = lo_hz * exp2(log2(hi_hz / lo_hz) * clamp(freq_t, 0.0, 1.0));
    return clamp(hz / nyquist, 0.0, 1.0) * max_bin;
}

fn sample_bin(bins: u32, row: u32, bin: f32) -> f32 {
    let lo = u32(floor(clamp(bin, 0.0, f32(bins - 1u))));
    let hi = min(lo + 1u, bins - 1u);
    let t = fract(bin);
    return mix(waterfall[row * bins + lo], waterfall[row * bins + hi], t);
}

fn sample_bin_cubic(bins: u32, row: u32, bin: f32) -> f32 {
    let max_bin = f32(bins - 1u);
    let x = clamp(bin, 0.0, max_bin);
    let i1 = i32(floor(x));
    let i0 = max(i1 - 1, 0);
    let i2 = min(i1 + 1, i32(bins - 1u));
    let i3 = min(i1 + 2, i32(bins - 1u));
    let t = x - f32(i1);
    let t2 = t * t;
    let t3 = t2 * t;
    let p0 = smoothed[row * bins + u32(i0)];
    let p1 = smoothed[row * bins + u32(i1)];
    let p2 = smoothed[row * bins + u32(i2)];
    let p3 = smoothed[row * bins + u32(i3)];
    let value = 0.5 * (
        2.0 * p1
        + (-p0 + p2) * t
        + (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3) * t2
        + (-p0 + 3.0 * p1 - 3.0 * p2 + p3) * t3);
    return clamp(value, 0.0, 1.0);
}

fn sample_bin_range(bins: u32, row: u32, bin: f32, bin_width: f32) -> f32 {
    let max_bin = f32(bins - 1u);
    let half_width = max(bin_width * 0.5, 0.5);
    let lo = i32(floor(clamp(bin - half_width, 0.0, max_bin)));
    let hi = i32(ceil(clamp(bin + half_width, 0.0, max_bin)));
    let span = max(hi - lo + 1, 1);

    if (span <= 1) {
        return sample_bin(bins, row, bin);
    }

    var peak = 0.0;
    var sum = 0.0;
    var sample_count = 0;
    if (span <= 64) {
        for (var idx = lo; idx <= hi; idx = idx + 1) {
            let v = waterfall[row * bins + u32(idx)];
            peak = max(peak, v);
            sum = sum + v;
            sample_count = sample_count + 1;
        }
    } else {
        for (var i = 0; i < 64; i = i + 1) {
            let idx = lo + i32(round(f32(i) * f32(span - 1) / 63.0));
            let v = waterfall[row * bins + u32(idx)];
            peak = max(peak, v);
            sum = sum + v;
            sample_count = sample_count + 1;
        }
    }
    let average = sum / f32(max(sample_count, 1));
    return max(peak, average);
}

fn spectrogram_display_value(value: f32) -> f32 {
    let v = clamp(value, 0.0, 1.0);
    let noise_floor = 0.025;
    if (v <= noise_floor) {
        return 0.0;
    }
    return pow((v - noise_floor) / (1.0 - noise_floor), 0.68);
}

fn eq_spectrum_display_value(value: f32) -> f32 {
    let v = clamp(value, 0.0, 1.0);
    return pow(smoothstep(0.0, 1.0, v), 0.72);
}

fn sample_eq_spectrum_curve(
    bins: u32,
    freq_t: f32,
    freq_scale: u32,
    sample_rate: f32,
    min_hz: f32,
    max_hz: f32,
    widget_px_w: f32,
) -> f32 {
    let radius = 6;
    let px = 1.0 / max(widget_px_w, 1.0);
    var weighted = 0.0;
    var peak = 0.0;
    var total_weight = 0.0;
    for (var i = -radius; i <= radius; i = i + 1) {
        let offset_px = f32(i);
        let t = clamp(freq_t + offset_px * px, 0.0, 1.0);
        let bin = spectrogram_bin_for_uv(t, freq_scale, bins, sample_rate, min_hz, max_hz);
        let value = sample_bin_cubic(bins, 0u, bin);
        let weight = exp(-0.5 * (offset_px * offset_px) / 6.25);
        weighted = weighted + value * weight;
        peak = max(peak, value);
        total_weight = total_weight + weight;
    }
    let averaged = weighted / max(total_weight, 0.0001);
    return eq_spectrum_display_value(mix(averaged, peak, 0.18));
}

fn heat_color(value: f32, low: vec3<f32>, mid: vec3<f32>, high: vec3<f32>) -> vec3<f32> {
    let v = clamp(value, 0.0, 1.0);
    let lower = mix(low, mid, smoothstep(0.0, 0.55, v));
    let upper = mix(mid, high, smoothstep(0.45, 1.0, v));
    return select(upper, lower, v < 0.55);
}

@fragment
fn live_spectrogram_frag(input: LiveSpectrogramVaryings) -> @location(0) vec4<f32> {
    let bins = input.counts.x;
    let time_slices = input.counts.y;
    let write_head = input.counts.z;
    let mode = input.counts.w;
    let freq_scale = input.freq_scale;
    let sample_rate = input.rates.x;
    let min_hz = input.rates.y;
    let max_hz = input.rates.z;
    let widget_px_w = input.widget_px.x;
    let widget_px_h = input.widget_px.y;

    if (bins < 2u || time_slices < 1u) {
        discard;
    }

    let uv = clamp(input.uv, vec2<f32>(0.0), vec2<f32>(1.0));
    let bin = spectrogram_bin_for_uv(
        uv.y,
        freq_scale,
        bins,
        sample_rate,
        min_hz,
        max_hz);
    let border = min(min(uv.x, 1.0 - uv.x), min(uv.y, 1.0 - uv.y));
    let border_mask = 1.0 - smoothstep(0.0, max(fwidth(border) * 1.5, 0.002), border);

    // Both modes' derivative inputs are computed here, in uniform control flow.
    // WGSL cannot take fwidth inside the `mode` branch, and the EQ curve has to
    // exist before its own derivative can be taken, so the curve is evaluated
    // for both modes and only the compositing below is branched.
    let eq_value = sample_eq_spectrum_curve(
        bins,
        uv.x,
        freq_scale,
        sample_rate,
        min_hz,
        max_hz,
        widget_px_w);
    let eq_y = clamp(eq_value, 0.0, 1.0);
    let eq_deriv = fwidth(uv.y - eq_y);
    let bin_width = max(fwidth(bin), 1.0);

    if (mode == 1u) {
        let fill = smoothstep(-0.004, 0.010, eq_y - uv.y);
        let line_width = max(1.35 / max(widget_px_h, 1.0), 0.003);
        let aa = max(eq_deriv, line_width * 0.65);
        let line = 1.0 - smoothstep(line_width, line_width + aa, abs(uv.y - eq_y));
        var rgb = input.background_color.rgb;
        rgb = mix(rgb, input.eq_fill_color.rgb, fill * input.eq_fill_color.a);
        rgb = mix(rgb, input.eq_line_color.rgb, line * input.eq_line_color.a);
        rgb = mix(rgb, input.eq_line_color.rgb, border_mask * 0.45);
        let alpha = max(max(fill * input.eq_fill_color.a, line * input.eq_line_color.a), border_mask * 0.35);
        return vec4<f32>(rgb, alpha);
    }

    let x = clamp(uv.x, 0.0, 0.999999);
    let time_offset = u32(floor(x * f32(time_slices)));
    let row = (write_head + time_offset) % time_slices;
    let value = spectrogram_display_value(sample_bin_range(bins, row, bin, bin_width));
    let heat = heat_color(value, input.min_color.rgb, input.mid_color.rgb, input.max_color.rgb);
    var rgb = mix(input.background_color.rgb, heat, smoothstep(0.02, 0.95, value));
    rgb = mix(rgb, input.max_color.rgb, border_mask * 0.28);
    let alpha = max(smoothstep(0.005, 0.12, value), border_mask * 0.30);
    return vec4<f32>(rgb, alpha);
}
"#;

/// The editable button-surface override, in WGSL. Counterpart of
/// `shaders/button_surface.metal`; see [`widget_shader_module`] for how it is
/// assembled into a complete module.
pub const BUTTON_SURFACE_WGSL: &str = include_str!("../../shaders/button_surface.wgsl");

/// Assemble one complete widget shader module, mirroring the Metal backend's
/// `WIDGET_SHADER_PREAMBLE + vertex + fragment` concatenation.
///
/// `vertex` is `None` for the widgets that use the shared vertex stage.
pub fn widget_shader_module(vertex: Option<&str>, fragment: &str) -> String {
    format!(
        "{}{}{}",
        WIDGET_SHADER_PREAMBLE_WGSL,
        vertex.unwrap_or(DEFAULT_WIDGET_VERTEX_SHADER_WGSL),
        fragment
    )
}

/// Every standalone WGSL module in this file, paired with the MSL constant it
/// was ported from. Widget shaders are not standalone — see
/// [`widget_shader_module`].
pub const STANDALONE_SHADER_MODULES: &[(&str, &str)] = &[
    ("SHADER_SRC", TEXT_SHADER_WGSL),
    ("PROP_FRAG_SRC", PROP_TEXT_SHADER_WGSL),
    ("IMAGE_SHADER_SRC", IMAGE_SHADER_WGSL),
    ("PATCH_CABLE_SHADER_SRC", PATCH_CABLE_SHADER_WGSL),
    ("WAVETABLE_SHADER_SRC", WAVETABLE_SHADER_WGSL),
    ("WAVEFORM_SHADER_SRC", WAVEFORM_SHADER_WGSL),
    ("LIVE_SPECTROGRAM_SHADER_SRC", LIVE_SPECTROGRAM_SHADER_WGSL),
];

#[cfg(test)]
mod tests {
    use super::*;

    /// Parse and validate one WGSL module the same way `wgpu` will, so a
    /// translation slip fails here rather than at pipeline-creation time.
    pub(crate) fn validate_wgsl(label: &str, source: &str) {
        let module = naga::front::wgsl::parse_str(source).unwrap_or_else(|error| {
            panic!("{label}: WGSL parse failed:\n{}", error.emit_to_string(source))
        });
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .unwrap_or_else(|error| panic!("{label}: WGSL validation failed: {error:#?}\n\n{source}"));
    }

    #[test]
    fn standalone_pipelines_are_valid_wgsl() {
        for (label, source) in STANDALONE_SHADER_MODULES {
            validate_wgsl(label, source);
        }
    }

    #[test]
    fn widget_preamble_assembles_with_the_default_vertex_stage() {
        // The preamble alone is not a module (no entry point); it becomes one
        // only once a vertex and fragment stage are concatenated on.
        validate_wgsl(
            "button_surface.wgsl",
            &widget_shader_module(None, BUTTON_SURFACE_WGSL),
        );
    }

    /// The generated SDF fragments are written against the preamble's
    /// `WidgetVaryings`, so they must assemble with it rather than declaring
    /// their own copy.
    #[test]
    fn generated_sdf_fragments_assemble_onto_the_preamble() {
        let source = "(let ((shape (- (length (vec2 x y)) 0.5)))
                        (sdf/layer
                          (sdf/fill shape
                            (material
                              :color (mix :accent :white (smoothstep -0.1 0.03 d))))))";
        let tokens = crate::parser::Parser::new(source.to_string()).parse().unwrap();
        let expression = crate::parser::ASTParser::new(tokens)
            .parse()
            .unwrap()
            .into_iter()
            .next()
            .unwrap();
        let generated = crate::lang::sdf_codegen::compile_sdf_to_wgsl(&expression).unwrap();
        validate_wgsl(
            "generated sdf widget",
            &widget_shader_module(None, &generated.shader_source),
        );
    }
}
