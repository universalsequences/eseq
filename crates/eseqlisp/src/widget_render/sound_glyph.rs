//! `sound-glyph`: GPU SDF renderer for cohort-relative delta glyph frames.

use std::collections::HashMap;

use super::{CellBuffer, WidgetDefinition};
use crate::layout::{f64_to_f32, get_prop_num, Constraints, MeasureCtx, Size};
use crate::vm::Value;

#[cfg(target_os = "macos")]
use super::{metal_widget_instance, ndc_bounds, MetalPrimitive, WidgetInstance, WidgetViewport};
#[cfg(target_os = "macos")]
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
#[cfg(target_os = "macos")]
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

#[cfg(target_os = "macos")]
fn tint_channel(props: &HashMap<String, Value>, name: &str, default: f32) -> f32 {
    match props.get(name) {
        Some(Value::Number(value)) => (*value as f32).clamp(0.0, 1.0),
        _ => default,
    }
}

#[cfg(target_os = "macos")]
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

impl WidgetDefinition for SoundGlyphWidget {
    fn names(&self) -> &'static [&'static str] { &["sound-glyph"] }
    fn size_affecting_props(&self) -> &'static [&'static str] { &["width", "height"] }

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
        let height = get_prop_num(node, "height").map(f64_to_f32).unwrap_or(6.0).max(0.4);
        Some(Size { width, height })
    }

    fn tui_render(&self, _props: &HashMap<String, Value>, _rect: crate::layout::Rect, _buf: &mut CellBuffer) {}

    #[cfg(target_os = "macos")]
    fn metal_fragment_shader(&self, _widget_type: &str) -> Option<&'static str> {
        Some(DELTA_GLYPH_SHADER)
    }

    #[cfg(target_os = "macos")]
    fn build_metal_primitives(
        &self,
        widget_type: &str,
        node: &LayoutNode,
        viewport: WidgetViewport,
    ) -> Vec<MetalPrimitive> {
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
        let play = match node.props.get("play") {
            Some(Value::Number(value)) => *value as f32,
            _ => {
                if crate::sound_glyph_data::sound_glyph_playing(&source) {
                    1.0
                } else {
                    0.0
                }
            }
        };

        // Pack only exact 24-bit integers into float uniforms; depending on NaN
        // payload bits would be unsafe across GPU drivers.
        //   words 0..4  — substrate, five 4-bit slot radii each (25 slots)
        //   words 5..9  — up to MAX_PIECES accent piece records, 18 bits each
        let mut packed = [0.0f32; 18];
        for (word, slots) in frame.substrate.chunks(5).take(5).enumerate() {
            packed[word] = slots.iter().enumerate().fold(0u32, |bits, (offset, radius)| {
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
        if side < 4.0 { return Vec::new(); }
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
        metal_widget_instance(widget_type, WidgetInstance {
            ndc_min,
            ndc_max,
            value_t: packed[16],
            orientation: 0.0,
            itime: packed[17],
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
                if play > 0.5 { 1.0 } else { 0.0 },
            ],
            color_b: tune[8..12].try_into().unwrap(),
            color_c: tune[12..16].try_into().unwrap(),
            color_d: [frame.cols as f32, frame.rows as f32, tune[7], frame.anchor as u8 as f32],
            corner_radius: frame.incompatible as u8 as f32,
            pixel_aspect: 1.0,
        })
    }
}

#[cfg(target_os = "macos")]
const DELTA_GLYPH_SHADER: &str = r#"
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

// Exact signed distance to the play triangle (right-pointing, same vertices
// the mixer's cell background used to draw): negative inside, so the edge
// anti-aliases at true pixel width via fwidth.
float dg_play_triangle(float2 p) {
    float2 p0 = float2(-0.26, -0.36);
    float2 p1 = float2(-0.26, 0.36);
    float2 p2 = float2(0.36, 0.0);
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
    float fit = dg_fit(in);
    float4 color = float4(0.0);

    float edge = max(dg_tune(in, 15), 0.0005) * fit;
    float substrate = dg_substrate_field(p, in);
    // color_a.w is the play flag, NOT the tint alpha — rebuild alpha as 1.0.
    float4 baseTint = float4(in.color_a.rgb, 1.0);
    color = dg_compose(color,
                       dg_material(p, dg_normal_substrate(p, in), baseTint, false, in),
                       substrate, fit, baseTint.rgb, in);

    // One layer per lit parameter, all anchored into the SHARED lattice so they
    // interpenetrate — the original's second tier of richness, which rev 2's
    // per-slot layer encoding made structurally impossible.
    float unionSoFar = substrate;
    for (int index = 0; index < DG_MAX_PIECES; ++index) {
        uint record = dg_piece(in, index);
        if (((record >> 17) & 1u) == 0u) continue;
        float sdf = dg_piece_field(p, in, record);
        float3 n = dg_normal_piece(p, in, record);

        float3 hue = DG_HUES[min((record >> 9) & 7u, 6u)];
        float magnitude = float((record >> 12) & 7u) / 7.0;
        // Sign rides hue temperature, not position: a positional offset large
        // enough to read is larger than the entire fusion budget (spec §6.2).
        hue *= ((record >> 16) & 1u) != 0u ? float3(0.92, 1.00, 1.10)
                                           : float3(1.08, 1.00, 0.92);
        float4 tint = float4(clamp(hue * (0.5 + 0.5 * magnitude), 0.0, 1.0), 1.0);

        color = dg_compose(color, dg_material(p, n, tint, false, in),
                           sdf, fit, tint.rgb, in);
        float intersection = max(unionSoFar, sdf + 0.05 * fit);
        color = mix(color, dg_material(p, n, tint, true, in),
                    1.0 - smoothstep(0.0, edge, intersection + 0.001 * fit));
        unionSoFar = min(unionSoFar, sdf);
    }
    color.rgb = clamp(color.rgb, 0.0, 1.0);

    // The anchor tile carries no accents by definition; ring it so it reads as
    // the zero point rather than as an empty patch.
    if (in.color_d.w > 0.5) {
        float ring = 1.0 - smoothstep(0.008, 0.017, abs(length(p) - 0.055));
        color = mix(color, float4(0.85, 0.87, 0.85, 0.8), ring);
    }
    if (in.corner_radius > 0.5) {
        float ring = 1.0 - smoothstep(0.010, 0.022, abs(length(p) - 0.91));
        color = mix(color, float4(0.96, 0.50, 0.24, 0.9), ring);
    }
    // Play indicator ON TOP of the glyph (color_a.w > 0.5 = playing): opaque
    // green triangle with ~1px coverage AA, over a soft dark ring so the edge
    // stays legible against bright accent pieces.
    if (in.color_a.w > 0.5) {
        float d = dg_play_triangle(p);
        float aa = max(fwidth(d), 0.002);
        float halo = (1.0 - smoothstep(0.0, 0.10, d)) * smoothstep(-aa, aa, d);
        color.rgb = mix(color.rgb, float3(0.01, 0.03, 0.015), 0.62 * halo);
        color.a = max(color.a, 0.62 * halo);
        float tri = 1.0 - smoothstep(-aa, aa, d);
        color.rgb = mix(color.rgb, float3(0.1, 0.95, 0.38), tri);
        color.a = max(color.a, tri);
    }
    if (color.a <= 0.002) discard_fragment();
    return color;
}
"#;
