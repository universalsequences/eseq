//! `sound-glyph`: GPU SDF renderer for cohort-relative delta glyph frames.

use std::collections::HashMap;

use super::{CellBuffer, WidgetDefinition};
use crate::layout::{Constraints, MeasureCtx, Size, f64_to_f32, get_prop_num};
use crate::vm::Value;

use super::{
    GpuPrimitive, WidgetInstance, WidgetViewport, get_f32_prop, gpu_widget_instance,
    ndc_bounds, resolve_named_color,
};
use crate::backend::Color;
use crate::layout::{LayoutNode, Rect};

pub struct SoundGlyphWidget;
pub static SOUND_GLYPH_WIDGET: SoundGlyphWidget = SoundGlyphWidget;

/// Accent pieces per glyph. Each is its own shaded layer with its own normal
/// estimate, so this bounds shader cost; mirrors `delta_glyph::MAX_LIT`.
const MAX_PIECES: usize = 5;
/// First uniform word holding a piece record; words 0..4 are the substrate.
const SHADER_PIECE_WORD: usize = 5;

pub fn source_key(props: &HashMap<String, Value>) -> Option<String> {
    match props.get("source") {
        Some(Value::String(source)) if !source.is_empty() => Some(source.clone()),
        _ => None,
    }
}

/// Shader tuning props: `(name, default)`, packed in order into the uniform
/// words the glyph payload leaves free (words 10..17, then color_b/color_c).
/// The defaults ARE the tuned house style (ear^H^H eye-tuned in the sound
/// palette, 2026-08-03) so every glyph surface — palette cards, mixer
/// pattern cells — shares one look with zero props; any prop still overrides
/// per call site for live tuning sessions.
const TUNING_PROPS: [(&str, f32); 16] = [
    // Height profile: the smoothstep window + power curve that turns the SDF
    // into the height field the finite-difference normals sample.
    ("height-amp", 3.8),
    ("height-pow", 2.0),
    ("height-in", -0.005),
    ("height-out", 0.148),
    // Finite-difference epsilon for the normal estimate (bigger = softer bevel).
    ("normal-eps", 0.001),
    // Coverage anti-aliasing width at the silhouette.
    ("edge-soft", 0.020),
    // Achromatic specular weight and tinted diffuse weight (dg_material).
    ("white-damp", 0.35),
    ("diffuse", 0.75),
    // Multiplier on both specular exponents; crease darkening multiplier.
    ("spec-pow", 10.0),
    ("crease-scale", 8.0),
    // Neon rim just inside the silhouette (0 gain = off).
    ("rim-width", 0.01),
    ("rim-gain", 0.38),
    // Soft halo outside the silhouette (0 gain = off).
    ("glow-width", 0.12),
    ("glow-gain", 0.23295),
    // SDF-depth interior shading: darkens toward the middle of a mass so the
    // fill is not one static color (0 shade = off).
    ("interior-shade", 0.18),
    ("interior-width", 0.22),
];

fn tint_channel(props: &HashMap<String, Value>, name: &str, default: f32) -> f32 {
    match props.get(name) {
        Some(Value::Number(value)) => (*value as f32).clamp(0.0, 1.0),
        _ => default,
    }
}

fn tuning(props: &HashMap<String, Value>) -> [f32; 16] {
    let mut packed = [0.0f32; 16];
    for (slot, (name, default)) in TUNING_PROPS.iter().enumerate() {
        packed[slot] = match props.get(*name) {
            Some(Value::Number(value)) => *value as f32,
            _ => *default,
        };
    }
    packed
}

/// Pack an RGB color into an exactly representable 24-bit integer carried by
/// one float uniform. Sound-glyph already uses this representation for its
/// lattice payload; unlike `itime`, this field participates in the retained
/// primitive cache token, so live style changes cannot reuse stale colors.
fn pack_rgb24(color: Color) -> f32 {
    let channel = |value: f32| (value.clamp(0.0, 1.0) * 255.0).round() as u32;
    (channel(color.r) | (channel(color.g) << 8) | (channel(color.b) << 16)) as f32
}

impl WidgetDefinition for SoundGlyphWidget {
    fn names(&self) -> &'static [&'static str] {
        &["sound-glyph"]
    }
    fn size_affecting_props(&self) -> &'static [&'static str] {
        &["width", "height"]
    }
    fn bindable_props(&self) -> &'static [&'static str] {
        &["play", "play-glyph-padding", "play-glyph-opacity"]
    }

    fn measure(
        &self,
        node: &Value,
        _children: &[Value],
        constraints: Constraints,
        _ctx: &MeasureCtx<'_>,
        _measure_child: &mut dyn FnMut(&Value, Constraints) -> Option<Size>,
    ) -> Option<Size> {
        // Minima stay tiny: mixer pattern cells embed sub-cell glyphs.
        let width = get_prop_num(node, "width")
            .map(f64_to_f32)
            .unwrap_or(constraints.max_width)
            .clamp(0.4, constraints.max_width.max(0.4));
        let height = get_prop_num(node, "height")
            .map(f64_to_f32)
            .unwrap_or(6.0)
            .max(0.4);
        Some(Size { width, height })
    }

    fn tui_render(
        &self,
        _props: &HashMap<String, Value>,
        _rect: crate::layout::Rect,
        _buf: &mut CellBuffer,
    ) {
    }

    fn fragment_shader(
        &self,
        _widget_type: &str,
        backend: super::ShaderBackend,
    ) -> Option<&'static str> {
        DELTA_GLYPH_SHADER.source(backend)
    }

    fn build_primitives(
        &self,
        widget_type: &str,
        node: &LayoutNode,
        viewport: WidgetViewport,
    ) -> Vec<GpuPrimitive> {
        let Some(source) = source_key(&node.props) else {
            return Vec::new();
        };
        let Some(frame) = crate::sound_glyph_data::sound_glyph_frame(&source) else {
            return Vec::new();
        };
        // Play indicator (mixer pattern cells): `:play` prop overrides, else
        // the host-published play-key store drives it. The store bumps the
        // widget-state generation on launch changes, and color_a is part of
        // the primitive cache token, so this stays live without itime.
        let play = if node.props.contains_key("play") {
            get_f32_prop(&node.props, "play", 0.0) > 0.5
        } else {
            crate::sound_glyph_data::sound_glyph_playing(&source)
        };
        // These are deliberately play-state overrides rather than global glyph
        // styling. The host play-key store is the launch-state authority, so
        // applying them in the renderer guarantees that the glyph shrinks and
        // dims in the same frame that the triangle appears.
        let play_glyph_padding =
            get_f32_prop(&node.props, "play-glyph-padding", 0.0).clamp(0.0, 0.45);
        let play_glyph_opacity =
            get_f32_prop(&node.props, "play-glyph-opacity", 1.0).clamp(0.0, 1.0);
        let play_color = resolve_named_color(
            &node.props,
            "play-color",
            Color::rgba(0.10, 0.95, 0.38, 1.0),
        );

        // Pack only exact 24-bit integers into float uniforms; depending on NaN
        // payload bits would be unsafe across GPU drivers.
        //   words 0..4  — substrate, five 4-bit slot radii each (25 slots)
        //   words 5..9  — up to MAX_PIECES accent piece records, 18 bits each
        let mut packed = [0.0f32; 18];
        for (word, slots) in frame.substrate.chunks(5).take(5).enumerate() {
            packed[word] = slots
                .iter()
                .enumerate()
                .fold(0u32, |bits, (offset, radius)| {
                    bits | ((*radius as u32 & 15) << (offset * 4))
                }) as f32;
        }
        for (index, piece) in frame.pieces.iter().take(MAX_PIECES).enumerate() {
            packed[SHADER_PIECE_WORD + index] = (piece.slot.min(24) as u32
                | ((piece.piece.min(14) as u32) << 5)
                | ((piece.hue.min(6) as u32) << 9)
                | ((piece.magnitude.min(7) as u32) << 12)
                | ((piece.mirror as u32) << 15)
                | ((piece.negative as u32) << 16)
                | (1 << 17)) as f32; // present bit
        }

        let px_w = node.rect.width * viewport.cell_w;
        let px_h = node.rect.height * viewport.cell_h;
        let side = px_w.min(px_h);
        if side < 4.0 {
            return Vec::new();
        }
        let square = Rect {
            col: node.rect.col + (px_w - side) * 0.5 / viewport.cell_w,
            row: node.rect.row + (px_h - side) * 0.5 / viewport.cell_h,
            width: side / viewport.cell_w,
            height: side / viewport.cell_h,
        };
        // Words 10..16 carry the first seven tuning props; diffuse rides
        // color_d.z (NOT itime — the run cache deliberately skips hashing
        // itime for non-animated widgets, which would make :diffuse inert);
        // the remaining eight ride color_b/color_c.
        let tune = tuning(&node.props);
        packed[10..16].copy_from_slice(&tune[0..6]);
        packed[16] = tune[6];

        let (ndc_min, ndc_max) = ndc_bounds(square, viewport);
        // `:pixelate` — virtual-resolution quantization, expressed as the cell
        // count across the glyph (0 = off, the hi-def look). The shader snaps
        // its sample coordinate to this grid before evaluating the SDF, so the
        // whole render pixelates with no extra pass. Not one of the 16 packed
        // TUNING_PROPS (those words are full); it rides the spare bits of the
        // color_d.w flag word instead.
        let pixelate = get_f32_prop(&node.props, "pixelate", 0.0)
            .clamp(0.0, 255.0)
            .round() as u32;
        let frame_flags =
            u32::from(frame.anchor) | (u32::from(frame.incompatible) << 1) | (pixelate << 2);
        gpu_widget_instance(
            widget_type,
            WidgetInstance {
                ndc_min,
                ndc_max,
                value_t: packed[16],
                // The shared vertex shader exposes `itime`, but retained-run cache
                // tokens intentionally exclude it for non-animated widgets. Mirror
                // padding into cache-visible orientation so reactive changes still
                // invalidate this primitive correctly.
                orientation: play_glyph_padding,
                itime: play_glyph_padding,
                uniform_a: packed[0..4].try_into().unwrap(),
                uniform_b: packed[4..8].try_into().unwrap(),
                uniform_c: packed[8..12].try_into().unwrap(),
                uniform_d: packed[12..16].try_into().unwrap(),
                // Substrate tint (overridable per call site — mixer cells tint the
                // body with the track color); accent hues live in the shader's
                // DG_HUES palette so a per-piece 3-bit index costs no uniform slots.
                // color_a.w carries the play flag (the shader rebuilds the tint's
                // alpha as 1.0 itself).
                color_a: [
                    tint_channel(&node.props, "tint-r", 0.20),
                    tint_channel(&node.props, "tint-g", 0.34),
                    tint_channel(&node.props, "tint-b", 0.38),
                    play as u8 as f32,
                ],
                color_b: tune[8..12].try_into().unwrap(),
                color_c: tune[12..16].try_into().unwrap(),
                // color_d.w is a two-bit flag word: bit 0 = anchor, bit 1 =
                // incompatible. Keeping both flags here frees corner_radius for the
                // cache-visible packed play color without expanding WidgetInstance.
                color_d: [
                    frame.cols as f32,
                    frame.rows as f32,
                    tune[7],
                    frame_flags as f32,
                ],
                corner_radius: pack_rgb24(play_color),
                pixel_aspect: play_glyph_opacity,
            },
        )
    }
}

const DELTA_GLYPH_SHADER: super::ShaderSources = super::ShaderSources::both(r#"
// Geometry constants mirror sequencer::delta_glyph (spec §6). Radius and k are
// FIXED: magnitude rides occupancy (which cells a piece claims) and luminance.
// Two equal discs weld iff their surface gap is within 0.6452*k, and all three
// lattice adjacencies (0.3672 / 0.40572 / 0.40896) clear that at R = 0.18.
constant float DG_K = 0.155;
constant float DG_R = 0.18;
constant float DG_STEP_X = 0.3672;
constant float DG_STEP_Y = 0.3636;
constant float DG_STAGGER = 0.09;
constant float DG_SUB_MIN = 0.155;
constant float DG_SUB_MAX = 0.185;
constant int DG_PIECE_WORD = 5;
constant int DG_MAX_PIECES = 5;

// Dual-maintained with delta_glyph::GROUP_PALETTE. Index 6 is the deliberately
// un-hued "unclassified" tone.
constant float3 DG_HUES[7] = {
    float3(0.98, 0.72, 0.22), float3(0.95, 0.30, 0.62), float3(0.55, 0.95, 0.30),
    float3(0.25, 0.80, 0.95), float3(0.62, 0.45, 0.98), float3(0.96, 0.46, 0.28),
    float3(0.56, 0.57, 0.60),
};

float dg_packed(WidgetVaryings in, int index) {
    switch (index) {
        case 0: return in.uniform_a.x; case 1: return in.uniform_a.y;
        case 2: return in.uniform_a.z; case 3: return in.uniform_a.w;
        case 4: return in.uniform_b.x; case 5: return in.uniform_b.y;
        case 6: return in.uniform_b.z; case 7: return in.uniform_b.w;
        case 8: return in.uniform_c.x; case 9: return in.uniform_c.y;
        case 10: return in.uniform_c.z; case 11: return in.uniform_c.w;
        case 12: return in.uniform_d.x; case 13: return in.uniform_d.y;
        case 14: return in.uniform_d.z; case 15: return in.uniform_d.w;
        case 16: return in.value_t; default: return in.itime;
    }
}

int dg_cols(WidgetVaryings in) { return clamp(int(round(in.color_d.x)), 1, 5); }
int dg_rows(WidgetVaryings in) { return clamp(int(round(in.color_d.y)), 1, 5); }
bool dg_anchor(WidgetVaryings in) { return (uint(round(in.color_d.w)) & 1u) != 0u; }
bool dg_incompatible(WidgetVaryings in) { return (uint(round(in.color_d.w)) & 2u) != 0u; }
// Virtual pixel count across the glyph (bits 2..9 of the flag word); 0 = off.
float dg_pixelate(WidgetVaryings in) {
    return float((uint(round(in.color_d.w)) >> 2) & 255u);
}

float3 dg_play_color(WidgetVaryings in) {
    uint rgb = uint(round(in.corner_radius));
    return float3(float(rgb & 255u),
                  float((rgb >> 8) & 255u),
                  float((rgb >> 16) & 255u)) / 255.0;
}

// 0 = unassigned slot, else 1..15 across the substrate radius band.
uint dg_substrate(WidgetVaryings in, int slot) {
    uint word = uint(round(dg_packed(in, slot / 5)));
    return (word >> (4 * (slot % 5))) & 15u;
}

uint dg_piece(WidgetVaryings in, int index) {
    return uint(round(dg_packed(in, DG_PIECE_WORD + index)));
}

float dg_smin(float a, float b, float k) {
    float h = max(k - abs(a - b), 0.0) / max(k, 0.0001);
    return min(a, b) - pow(h, 1.55) * 0.5 * k / 1.55;
}

// Plain column-major: slot = col*rows + row. Rev 2 reversed odd columns, which
// broke horizontal adjacency and therefore every piece built from it.
float2 dg_center(int col, int row, int cols, int rows) {
    float x = (float(col) - 0.5 * float(cols - 1)) * DG_STEP_X
            + ((row & 1) == 1 ? DG_STAGGER : -DG_STAGGER);
    float y = (float(row) - 0.5 * float(rows - 1)) * DG_STEP_Y;
    return float2(x, y);
}

float dg_fit(WidgetVaryings in) {
    float extentX = float(dg_cols(in) - 1) * DG_STEP_X + 2.0 * DG_STAGGER + 2.0 * DG_R + 0.06;
    float extentY = float(dg_rows(in) - 1) * DG_STEP_Y + 2.0 * DG_R + 0.06;
    return 2.0 / max(extentX, extentY);
}

// The substrate: one disc per assigned slot, radius from the patch's ABSOLUTE
// parameter values, over a band that lies entirely inside the fusion zone — so
// this is always one molten mass whose silhouette varies with the whole vector.
float dg_substrate_field(float2 p, WidgetVaryings in) {
    int cols = dg_cols(in), rows = dg_rows(in);
    float fit = dg_fit(in);
    p /= fit;
    float scene = 1000.0;
    for (int slot = 0; slot < 25; ++slot) {
        if (slot >= cols * rows) break;
        uint level = dg_substrate(in, slot);
        if (level == 0u) continue;
        float radius = mix(DG_SUB_MIN, DG_SUB_MAX, float(level - 1u) / 14.0);
        float2 c = dg_center(slot / rows, slot % rows, cols, rows);
        scene = dg_smin(scene, length(p - c) - radius, DG_K);
    }
    return scene * fit;
}

// One accent piece: a contiguous polyomino of 1..5 cells, welded at fixed radius.
// The table mirrors delta_glyph::PIECES (tier*3 + variant).
float dg_piece_field(float2 p, WidgetVaryings in, uint record) {
    int cols = dg_cols(in), rows = dg_rows(in);
    float fit = dg_fit(in);
    p /= fit;
    int slot = int(record & 31u);
    int id = int((record >> 5) & 15u);
    float mirror = ((record >> 15) & 1u) != 0u ? -1.0 : 1.0;
    int anchorCol = slot / rows;
    int anchorRow = slot % rows;

    float scene = 1000.0;
    for (int prim = 0; prim < 5; ++prim) {
        int dcol = 0, drow = 0;
        bool capsule = false, present = false;
        // tier 0: 1 cell
        if (id <= 2) {
            present = prim == 0;
        } else if (id == 3) {                 // capsule
            present = prim == 0; capsule = true;
        } else if (id == 4) {                 // vertical pair
            present = prim < 2; drow = prim;
        } else if (id == 5) {                 // diagonal pair
            present = prim < 2; dcol = prim; drow = prim;
        } else if (id == 6) {                 // capsule + disc
            present = prim < 2; capsule = prim == 0; drow = prim;
        } else if (id == 7) {                 // L
            present = prim < 3; drow = min(prim, 1); dcol = prim == 2 ? 1 : 0;
        } else if (id == 8) {                 // vertical run
            present = prim < 3; drow = prim;
        } else if (id == 9) {                 // stacked capsules
            present = prim < 2; capsule = true; drow = prim;
        } else if (id == 10) {                // 2x2
            present = prim < 4; dcol = prim / 2; drow = prim % 2;
        } else if (id == 11) {                // capsule + 2 discs
            present = prim < 3; capsule = prim == 0;
            drow = prim == 0 ? 0 : 1; dcol = prim == 2 ? 1 : 0;
        } else if (id == 12) {                // 2 capsules + disc
            present = prim < 3; capsule = prim < 2; drow = prim;
        } else if (id == 13) {                // P-pentomino
            present = prim < 5; dcol = prim / 3; drow = prim % 3;
        } else {                              // capsule + 3 discs
            present = prim < 4; capsule = prim == 0;
            dcol = prim >= 2 ? 1 : 0; drow = prim == 0 ? 0 : (prim == 1 ? 1 : prim - 1);
        }
        if (!present) continue;

        int col = anchorCol + int(mirror) * dcol;
        int row = anchorRow + drow;
        if (col < 0 || col >= cols || row < 0 || row >= rows) continue;
        float2 c = dg_center(col, row, cols, rows);
        float sdf;
        if (capsule) {
            // A stadium welding this cell to its horizontal neighbour: the
            // two-cell pair as one CONVEX primitive, no waist. This is where the
            // elongated lobes come from.
            int farCol = col + int(mirror);
            if (farCol < 0 || farCol >= cols) {
                sdf = length(p - c) - DG_R;
            } else {
                float2 far = dg_center(farCol, row, cols, rows);
                float2 seg = far - c;
                float t = clamp(dot(p - c, seg) / max(dot(seg, seg), 0.0001), 0.0, 1.0);
                sdf = length(p - c - seg * t) - DG_R;
            }
        } else {
            sdf = length(p - c) - DG_R;
        }
        scene = dg_smin(scene, sdf, DG_K);
    }
    return scene * fit;
}

// ── tuning uniforms ──
// Live-tunable from lisp via widget props (see TUNING_PROPS on the Rust side).
// Words 10..17 and color_b/color_c; every default reproduces the shipped look.
float dg_tune(WidgetVaryings in, int word) { return dg_packed(in, word); }

// The fake-3D height profile: a smoothstep window over the SDF raised to a
// power. :height-in/:height-out move the window, :height-pow shapes the
// shoulder, :height-amp scales the relief the normals read.
float dg_height(float sdf, float fit, WidgetVaryings in) {
    float lo = dg_tune(in, 12) * fit;
    float hi = max(dg_tune(in, 13) * fit, lo + 1e-5);
    return dg_tune(in, 10) * pow(smoothstep(lo, hi, sdf), max(dg_tune(in, 11), 0.01));
}

float3 dg_normal_substrate(float2 p, WidgetVaryings in) {
    float fit = dg_fit(in);
    float e = max(dg_tune(in, 14), 1e-6) * fit;
    float x = dg_height(dg_substrate_field(p + float2(e, 0.0), in), fit, in)
            - dg_height(dg_substrate_field(p - float2(e, 0.0), in), fit, in);
    float y = dg_height(dg_substrate_field(p + float2(0.0, e), in), fit, in)
            - dg_height(dg_substrate_field(p - float2(0.0, e), in), fit, in);
    return normalize(float3(x, y, 2.0 * e));
}

float3 dg_normal_piece(float2 p, WidgetVaryings in, uint record) {
    float fit = dg_fit(in);
    float e = max(dg_tune(in, 14), 1e-6) * fit;
    float x = dg_height(dg_piece_field(p + float2(e, 0.0), in, record), fit, in)
            - dg_height(dg_piece_field(p - float2(e, 0.0), in, record), fit, in);
    float y = dg_height(dg_piece_field(p + float2(0.0, e), in, record), fit, in)
            - dg_height(dg_piece_field(p - float2(0.0, e), in, record), fit, in);
    return normalize(float3(x, y, 2.0 * e));
}

float4 dg_material(float2 p, float3 n, float4 tint, bool crease, WidgetVaryings in) {
    float3 l1 = float3(-0.11, -0.8138, 0.3);
    float3 l2 = float3(-0.5238, 0.3, 1.4);
    float specPow = max(in.color_b.x, 0.05);
    float3 viewer = float3(p, 1.0) - float3(-0.81891595, 1.39159394, 0.87441919);
    float spec1 = pow(max(0.0, 0.99 * dot(n, normalize(l1 + viewer))), 24.0 * specPow);
    float spec2 = pow(max(0.0, 0.969 * dot(n, normalize(l2 + viewer))), 22.0 * specPow);
    float scale = crease ? 0.321513593 * max(in.color_b.y, 0.0) : 0.51513593;
    // The original adds this achromatically (a precedence artifact there — see
    // docs/sdf-blob-glyph-algorithm.md §6.4). Damped: at full strength it
    // desaturates every cell toward white, and hue is this glyph's group legend.
    float white = dg_tune(in, 16) * (scale * spec1 + spec2 + 0.293913139 * dot(l1, n));
    // Diffuse rides color_d.z — itime is excluded from the run-cache hash
    // for non-animated widgets, so data there would never invalidate.
    float4 color = float4(white) + in.color_d.z * dot(l2, n) * tint;
    color.rgb = clamp(color.rgb, 0.0, 1.0);
    color.a = tint.a;
    return color;
}

// Composite one layer over the running color: interior shading inside the
// surface, coverage AA at the silhouette, then the optional neon rim hugging
// the inside edge and the optional emissive halo falling off outside it.
float4 dg_compose(float4 color, float4 material, float sdf, float fit, float3 tint,
                  WidgetVaryings in) {
    float edge = max(dg_tune(in, 15), 0.0005) * fit;
    float shade = clamp(in.color_c.z, 0.0, 1.0);
    if (shade > 0.0) {
        // SDF-depth shading: the fill darkens toward the middle of the mass, so
        // it reads as lit volume rather than one static color.
        float width = max(in.color_c.w, 0.01) * fit;
        material.rgb *= 1.0 - shade * smoothstep(0.0, width, -sdf);
    }
    color = mix(color, material, 1.0 - smoothstep(0.0, edge, sdf));
    float rimGain = in.color_b.w;
    if (rimGain > 0.0) {
        float band = max(in.color_b.z, 0.005) * fit;
        float rim = rimGain * (1.0 - smoothstep(0.0, band, abs(sdf + 0.5 * band)));
        color.rgb += rim * mix(tint, float3(1.0), 0.6);
        color.a = max(color.a, min(rim, 1.0));
    }
    float glowGain = in.color_c.y;
    if (glowGain > 0.0 && sdf > 0.0) {
        float glow = glowGain * exp(-sdf / max(in.color_c.x * fit, 0.001));
        color.rgb += glow * tint;
        color.a = max(color.a, min(glow, 1.0));
    }
    return color;
}

// Exact signed distance to the play triangle (right-pointing, 2x the vertices
// the mixer's cell background used to draw): negative inside, so the edge
// anti-aliases at true pixel width via fwidth.
float dg_play_triangle(float2 p) {
    float2 p0 = float2(-0.52, -0.72);
    float2 p1 = float2(-0.52, 0.72);
    float2 p2 = float2(0.72, 0.0);
    float2 e0 = p1 - p0, e1 = p2 - p1, e2 = p0 - p2;
    float2 v0 = p - p0, v1 = p - p1, v2 = p - p2;
    float2 pq0 = v0 - e0 * clamp(dot(v0, e0) / dot(e0, e0), 0.0, 1.0);
    float2 pq1 = v1 - e1 * clamp(dot(v1, e1) / dot(e1, e1), 0.0, 1.0);
    float2 pq2 = v2 - e2 * clamp(dot(v2, e2) / dot(e2, e2), 0.0, 1.0);
    float s = sign(e0.x * e2.y - e0.y * e2.x);
    float2 d = min(min(float2(dot(pq0, pq0), s * (v0.x * e0.y - v0.y * e0.x)),
                       float2(dot(pq1, pq1), s * (v1.x * e1.y - v1.y * e1.x))),
                   float2(dot(pq2, pq2), s * (v2.x * e2.y - v2.y * e2.x)));
    return -sqrt(d.x) * sign(d.y);
}

fragment float4 widget_frag(WidgetVaryings in [[stage_in]]) {
    // Centered uv, +y upward, as required by the delta-glyph lattice.
    float2 p = float2(in.uv.x * 2.0 - 1.0, 1.0 - in.uv.y * 2.0);
    bool play = in.color_a.w > 0.5;
    // Padding is a fraction of the glyph's half-extent on every side. It and
    // opacity apply only while the play indicator is present; the triangle
    // remains full-size and fully opaque above the quieter identity glyph.
    float glyphScale = play ? 1.0 - 2.0 * clamp(in.itime, 0.0, 0.45) : 1.0;
    float glyphOpacity = play ? clamp(in.aspect, 0.0, 1.0) : 1.0;
    float2 glyphP = p / max(glyphScale, 0.10);
    // Pixelation: snap the sample coordinate to an N-cell virtual grid before
    // any field evaluation. Everything downstream (normals, lighting, rim,
    // glow, interior shade) evaluates at the cell center, so the whole glyph
    // quantizes coherently; the silhouette smoothstep then gives boundary
    // cells partial coverage, which reads as a cleanly downsampled image. The
    // play triangle deliberately stays on the raw coordinate (crisp on top).
    float pixelCells = dg_pixelate(in);
    if (pixelCells > 0.5) {
        glyphP = (floor((glyphP * 0.5 + 0.5) * pixelCells) + 0.5) / pixelCells * 2.0 - 1.0;
    }
    float fit = dg_fit(in);
    float4 color = float4(0.0);

    float edge = max(dg_tune(in, 15), 0.0005) * fit;
    float substrate = dg_substrate_field(glyphP, in);
    // color_a.w is the play flag, NOT the tint alpha — rebuild alpha as 1.0.
    float4 baseTint = float4(in.color_a.rgb, 1.0);
    color = dg_compose(color,
                       dg_material(glyphP, dg_normal_substrate(glyphP, in), baseTint, false, in),
                       substrate, fit, baseTint.rgb, in);

    // One layer per lit parameter, all anchored into the SHARED lattice so they
    // interpenetrate — the original's second tier of richness, which rev 2's
    // per-slot layer encoding made structurally impossible.
    float unionSoFar = substrate;
    for (int index = 0; index < DG_MAX_PIECES; ++index) {
        uint record = dg_piece(in, index);
        if (((record >> 17) & 1u) == 0u) continue;
        float sdf = dg_piece_field(glyphP, in, record);
        float3 n = dg_normal_piece(glyphP, in, record);

        float3 hue = DG_HUES[min((record >> 9) & 7u, 6u)];
        float magnitude = float((record >> 12) & 7u) / 7.0;
        // Sign rides hue temperature, not position: a positional offset large
        // enough to read is larger than the entire fusion budget (spec §6.2).
        hue *= ((record >> 16) & 1u) != 0u ? float3(0.92, 1.00, 1.10)
                                           : float3(1.08, 1.00, 0.92);
        float4 tint = float4(clamp(hue * (0.5 + 0.5 * magnitude), 0.0, 1.0), 1.0);

        color = dg_compose(color, dg_material(glyphP, n, tint, false, in),
                           sdf, fit, tint.rgb, in);
        float intersection = max(unionSoFar, sdf + 0.05 * fit);
        color = mix(color, dg_material(glyphP, n, tint, true, in),
                    1.0 - smoothstep(0.0, edge, intersection + 0.001 * fit));
        unionSoFar = min(unionSoFar, sdf);
    }
    color.rgb = clamp(color.rgb, 0.0, 1.0);

    // The anchor tile carries no accents by definition; ring it so it reads as
    // the zero point rather than as an empty patch.
    if (dg_anchor(in)) {
        float ring = 1.0 - smoothstep(0.008, 0.017, abs(length(glyphP) - 0.055));
        color = mix(color, float4(0.85, 0.87, 0.85, 0.8), ring);
    }
    if (dg_incompatible(in)) {
        float ring = 1.0 - smoothstep(0.010, 0.022, abs(length(glyphP) - 0.91));
        color = mix(color, float4(0.96, 0.50, 0.24, 0.9), ring);
    }
    color.a *= glyphOpacity;
    // Play indicator ON TOP of the glyph (color_a.w > 0.5 = playing): caller-
    // colored triangle with ~1px coverage AA, over a soft dark ring so the
    // edge stays legible against bright accent pieces.
    if (play) {
        float d = dg_play_triangle(p);
        float aa = max(fwidth(d), 0.002);
        float halo = (1.0 - smoothstep(0.0, 0.10, d)) * smoothstep(-aa, aa, d);
        color.rgb = mix(color.rgb, float3(0.01, 0.03, 0.015), 0.62 * halo);
        color.a = max(color.a, 0.62 * halo);
        float tri = 1.0 - smoothstep(-aa, aa, d);
        color.rgb = mix(color.rgb, dg_play_color(in), tri);
        color.a = max(color.a, tri);
    }
    if (color.a <= 0.002) discard_fragment();
    return color;
}
"#, super::wgsl::DELTA_GLYPH_SHADER);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn play_style_numeric_props_accept_reactive_bindings() {
        assert_eq!(
            SOUND_GLYPH_WIDGET.bindable_props(),
            &["play", "play-glyph-padding", "play-glyph-opacity"]
        );
    }

    #[test]
    fn play_style_resolves_live_numeric_props_and_color_into_shader_uniforms() {
        use std::cell::RefCell;
        use std::rc::Rc;

        let slots = crate::reactive::ReactiveBindingStore::default();
        slots.write_float("SOUND_GLYPH_TEST", "play", 1.0);
        slots.write_float("SOUND_GLYPH_TEST", "padding", 0.14);
        slots.write_float("SOUND_GLYPH_TEST", "opacity", 0.4);
        let reactive = |field: &str| Value::ReactiveRef {
            namespace: "SOUND_GLYPH_TEST".to_string(),
            field: field.to_string(),
            index: None,
            kind: crate::vm::BindingKind::Float,
            slot: slots.slot("SOUND_GLYPH_TEST", field),
        };
        let play_color = Value::List(
            [0.2, 0.4, 0.8]
                .into_iter()
                .map(|value| Rc::new(RefCell::new(Value::Number(value))))
                .collect(),
        );
        let source = "sound-glyph-play-style-test";
        crate::sound_glyph_data::publish_sound_glyph_frame(
            source,
            crate::sound_glyph_data::SoundGlyphFrame {
                revision: 1,
                cols: 1,
                rows: 1,
                substrate: vec![8],
                pieces: Vec::new(),
                anchor: true,
                incompatible: true,
            },
        );
        let node = LayoutNode {
            widget_id: 1,
            stable_widget_id: None,
            subtree_root_id: None,
            parent_subtree_root_id: None,
            stable_key: None,
            widget_type: "sound-glyph".to_string(),
            rect: Rect {
                row: 0.0,
                col: 0.0,
                width: 4.0,
                height: 4.0,
            },
            props: HashMap::from([
                ("source".to_string(), Value::String(source.to_string())),
                ("play".to_string(), reactive("play")),
                ("play-glyph-padding".to_string(), reactive("padding")),
                ("play-glyph-opacity".to_string(), reactive("opacity")),
                ("play-color".to_string(), play_color),
            ]),
            children: Vec::new(),
            focusable: false,
            animation: Default::default(),
        };
        let viewport = WidgetViewport {
            cell_w: 10.0,
            cell_h: 10.0,
            vp_w: 100.0,
            vp_h: 100.0,
            time_seconds: 0.0,
            focused_widget_id: None,
            focused_branch: false,
            overlay_viewport_bottom: 10.0,
            scroll_top: 0.0,
            scroll_left: 0.0,
            inherited_hover: false,
        };
        let instance = |primitives: Vec<GpuPrimitive>| {
            primitives
                .into_iter()
                .find_map(|primitive| match primitive {
                    GpuPrimitive::WidgetInstance { instance, .. } => Some(instance),
                    _ => None,
                })
                .expect("sound glyph shader instance")
        };

        let playing =
            instance(SOUND_GLYPH_WIDGET.build_primitives("sound-glyph", &node, viewport));
        assert!((playing.orientation - 0.14).abs() < 0.0001);
        assert!((playing.itime - 0.14).abs() < 0.0001);
        assert!((playing.pixel_aspect - 0.4).abs() < 0.0001);
        assert_eq!(playing.color_a[3], 1.0);
        assert_eq!(playing.color_d[3], 3.0);
        assert_eq!(
            playing.corner_radius,
            (51 | (102 << 8) | (204 << 16)) as f32
        );

        slots.write_float("SOUND_GLYPH_TEST", "play", 0.0);
        slots.write_float("SOUND_GLYPH_TEST", "padding", 0.25);
        slots.write_float("SOUND_GLYPH_TEST", "opacity", 0.7);
        let stopped =
            instance(SOUND_GLYPH_WIDGET.build_primitives("sound-glyph", &node, viewport));
        assert_eq!(stopped.color_a[3], 0.0);
        assert!((stopped.orientation - 0.25).abs() < 0.0001);
        assert!((stopped.pixel_aspect - 0.7).abs() < 0.0001);
    }
}
