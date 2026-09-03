//! `lfo-curve`: one cycle of a voice-modulator LFO shape with a live phase
//! marker. The source editor draws the waveform the user is designing (shape,
//! pulse width, phase offset) and rides the marker on the most recently
//! triggered voice's effective phase, published by the host as a reactive
//! field. Colors and geometry follow `modulator-curve`.

use std::collections::HashMap;

use super::{
    CellBuffer, GpuPrimitive, WidgetDefinition, WidgetInstance, WidgetViewport, ndc_bounds,
    resolve_named_color, styled_cell,
};
use crate::backend::Color;
use crate::layout::{Constraints, LayoutNode, MeasureCtx, Size, f64_to_f32, get_prop_num};
use crate::theme;
use crate::vm::Value;

pub struct LfoCurveWidget;

pub static LFO_CURVE_WIDGET: LfoCurveWidget = LfoCurveWidget;

/// Shape indices, dual-maintained with the voice modulator's
/// `shape_labels()` order (`triangle`, `sine`, `pulse`, `sawtooth`).
pub const SHAPE_TRIANGLE: f32 = 0.0;
pub const SHAPE_SINE: f32 = 1.0;
pub const SHAPE_PULSE: f32 = 2.0;
pub const SHAPE_SAW: f32 = 3.0;

fn value_num(value: &Value) -> Option<f32> {
    match value {
        Value::Number(n) => Some(*n as f32),
        Value::ReactiveRef { slot, .. } => Some(crate::reactive::read_float_slot(slot) as f32),
        _ => None,
    }
}

fn prop_num(props: &HashMap<String, Value>, key: &str, default: f32) -> f32 {
    props.get(key).and_then(value_num).unwrap_or(default)
}

/// Bipolar shape value at `phase` (0..1); the same math as the voice
/// modulator's `shape_value`, kept here so the TUI fallback and tests can
/// describe the curve without a GPU.
pub fn shape_value(shape: f32, phase: f32, pulse_width: f32) -> f32 {
    let phase = phase.rem_euclid(1.0);
    match shape.round() as i32 {
        1 => (std::f32::consts::TAU * phase).sin(),
        2 => {
            if phase < pulse_width.clamp(0.05, 0.95) {
                1.0
            } else {
                -1.0
            }
        }
        3 => phase * 2.0 - 1.0,
        _ => {
            // Skewed triangle: `pulse_width` is the peak position.
            let peak = pulse_width.clamp(0.05, 0.95);
            if phase < peak {
                -1.0 + 2.0 * phase / peak
            } else {
                1.0 - 2.0 * (phase - peak) / (1.0 - peak)
            }
        }
    }
}

impl WidgetDefinition for LfoCurveWidget {
    fn names(&self) -> &'static [&'static str] {
        &["lfo-curve"]
    }

    fn size_affecting_props(&self) -> &'static [&'static str] {
        &["width", "height"]
    }

    fn bindable_props(&self) -> &'static [&'static str] {
        &["shape", "pw", "phase-offset", "phase"]
    }

    fn measure(
        &self,
        node: &Value,
        _children: &[Value],
        constraints: Constraints,
        _ctx: &MeasureCtx<'_>,
        _measure_child: &mut dyn FnMut(&Value, Constraints) -> Option<Size>,
    ) -> Option<Size> {
        let width = get_prop_num(node, "width")
            .map(f64_to_f32)
            .unwrap_or(constraints.max_width)
            .clamp(6.0, constraints.max_width.max(6.0));
        let height = get_prop_num(node, "height")
            .map(f64_to_f32)
            .unwrap_or(4.0)
            .max(2.0);
        Some(Size { width, height })
    }

    fn tui_render(
        &self,
        props: &HashMap<String, Value>,
        rect: crate::layout::Rect,
        buf: &mut CellBuffer,
    ) {
        let shape = prop_num(props, "shape", SHAPE_TRIANGLE);
        let pw = prop_num(props, "pw", 0.5);
        let width = rect.width.round().max(1.0) as usize;
        let row = rect.row.round() as u16;
        let col_start = rect.col.round() as u16;
        for i in 0..width {
            let phase = i as f32 / width.max(1) as f32;
            let ch = if shape_value(shape, phase, pw) >= 0.0 {
                '▀'
            } else {
                '▄'
            };
            buf.set(
                row,
                col_start + i as u16,
                styled_cell(ch, theme::FG_MUTED(), None),
            );
        }
    }

    fn fragment_shader(
        &self,
        _widget_type: &str,
        backend: super::ShaderBackend,
    ) -> Option<&'static str> {
        LFO_CURVE_SHADER.source(backend)
    }

    fn build_primitives(
        &self,
        widget_type: &str,
        node: &LayoutNode,
        viewport: WidgetViewport,
    ) -> Vec<GpuPrimitive> {
        let shape = prop_num(&node.props, "shape", SHAPE_TRIANGLE).clamp(0.0, 3.0);
        let pw = prop_num(&node.props, "pw", 0.5).clamp(0.05, 0.95);
        // Degrees, like the `modN_lfo_phase` param; the shader works in cycles.
        let phase_offset = prop_num(&node.props, "phase-offset", 0.0) / 360.0;
        let phase = prop_num(&node.props, "phase", -1.0);
        let bg_color = resolve_named_color(
            &node.props,
            "background-color",
            Color::rgba(0.055, 0.058, 0.06, 1.0),
        );
        let grid_color = resolve_named_color(
            &node.props,
            "grid-color",
            Color::rgba(0.36, 0.36, 0.38, 0.45),
        );
        let curve_color = resolve_named_color(
            &node.props,
            "curve-color",
            Color::rgba(1.0, 0.72, 0.24, 1.0),
        );
        let fill_color = resolve_named_color(
            &node.props,
            "fill-color",
            Color::rgba(1.0, 0.48, 0.18, 0.18),
        );
        let (ndc_min, ndc_max) = ndc_bounds(node.rect, viewport);
        let px_w = node.rect.width * viewport.cell_w;
        let px_h = node.rect.height * viewport.cell_h;

        vec![GpuPrimitive::WidgetInstance {
            widget_type: widget_type.to_string(),
            instance: WidgetInstance {
                ndc_min,
                ndc_max,
                value_t: 0.0,
                orientation: 0.0,
                itime: viewport.time_seconds,
                uniform_a: [shape, pw, phase_offset, phase],
                uniform_b: [0.0; 4],
                uniform_c: [0.0; 4],
                uniform_d: [0.0; 4],
                color_a: curve_color.to_rgba(),
                color_b: bg_color.to_rgba(),
                color_c: grid_color.to_rgba(),
                color_d: fill_color.to_rgba(),
                corner_radius: 0.0,
                pixel_aspect: if px_h > 0.0 { px_w / px_h } else { 1.0 },
            },
            is_background: false,
        }]
    }
}

// uniform_a: shape index, pulse width, phase offset (cycles), marker phase
// (effective cycles as the host publishes it, < 0 hides the marker). The x
// axis is one cycle of the *running* phase: the curve is `shape(x + offset)`,
// so turning the phase knob visibly slides the waveform, and the marker sits at
// `phase - offset` so it stays on the drawn curve.
const LFO_CURVE_SHADER: super::ShaderSources = super::ShaderSources::both(
    r#"
float lc_sdSegment(float2 p, float2 a, float2 b) {
    float2 pa = p - a;
    float2 ba = b - a;
    float h = clamp(dot(pa, ba) / max(dot(ba, ba), 1e-6), 0.0, 1.0);
    return length(pa - ba * h);
}

float2 lc_plot(float2 data) {
    float2 pad = float2(0.04, 0.14);
    return float2(
        pad.x + data.x * (1.0 - pad.x * 2.0),
        pad.y + (1.0 - (data.y * 0.5 + 0.5)) * (1.0 - pad.y * 2.0));
}

float lc_shape(int shape, float x, float pw) {
    float phase = x - floor(x);
    if (shape == 1) {
        return sin(6.28318530718 * phase);
    } else if (shape == 2) {
        return phase < clamp(pw, 0.05, 0.95) ? 1.0 : -1.0;
    } else if (shape == 3) {
        return phase * 2.0 - 1.0;
    }
    float peak = clamp(pw, 0.05, 0.95);
    return phase < peak ? -1.0 + 2.0 * phase / peak : 1.0 - 2.0 * (phase - peak) / (1.0 - peak);
}

fragment float4 widget_frag(WidgetVaryings in [[stage_in]])
{
    float2 uv = in.uv;
    float aspect = max(in.aspect, 0.0001);
    int shape = int(round(clamp(in.uniform_a.x, 0.0, 3.0)));
    float pw = in.uniform_a.y;
    float offset = in.uniform_a.z;
    float markerPhase = in.uniform_a.w;

    float4 col = in.color_b;

    // A single faint zero line; no other grid.
    float2 zero = lc_plot(float2(0.0, 0.0));
    float zeroDist = abs(uv.y - zero.y);
    float zeroMask = smoothstep(0.006, 0.002, zeroDist);
    col.rgb = mix(col.rgb, in.color_c.rgb, zeroMask * in.color_c.a * 0.5);

    // Fill between the zero line and the curve.
    float dataX = clamp((uv.x - 0.04) / 0.92, 0.0, 1.0);
    float curveAtX = lc_shape(shape, dataX + offset, pw);
    float curveY = lc_plot(float2(dataX, curveAtX)).y;
    float between = step(min(curveY, zero.y) - 0.002, uv.y) * step(uv.y, max(curveY, zero.y) + 0.002);
    col.rgb = mix(col.rgb, in.color_d.rgb, between * in.color_d.a);

    float lineMask = 0.0;
    const int steps = 96;
    float prevX = 0.0;
    float prevY = lc_shape(shape, offset, pw);
    for (int i = 1; i <= steps; i++) {
        float x = float(i) / float(steps);
        float y = lc_shape(shape, x + offset, pw);
        float2 a = lc_plot(float2(prevX, prevY));
        float2 b = lc_plot(float2(x, y));
        float d = lc_sdSegment(float2(uv.x * aspect, uv.y), float2(a.x * aspect, a.y), float2(b.x * aspect, b.y));
        float aa = max(fwidth(d), 0.001);
        lineMask = max(lineMask, smoothstep(0.009 + aa, 0.003, d));
        prevX = x;
        prevY = y;
    }

    col.rgb = mix(col.rgb, in.color_a.rgb, lineMask * in.color_a.a);
    col.a = max(col.a, max(lineMask * in.color_a.a, in.color_b.a));

    if (markerPhase >= 0.0) {
        float running = markerPhase - offset;
        float markerX = clamp(running - floor(running), 0.0, 1.0);
        float2 marker = lc_plot(float2(markerX, lc_shape(shape, markerX + offset, pw)));
        float markerDist = length(float2((uv.x - marker.x) * aspect, uv.y - marker.y));
        float outer = smoothstep(0.058, 0.042, markerDist);
        float inner = smoothstep(0.036, 0.022, markerDist);
        col.rgb = mix(col.rgb, float3(0.02, 0.025, 0.03), outer);
        col.rgb = mix(col.rgb, in.color_a.rgb, inner);
        col.a = max(col.a, outer);
    }
    return col;
}
"#,
    super::wgsl::LFO_CURVE_SHADER,
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shapes_span_the_bipolar_range() {
        for shape in [SHAPE_TRIANGLE, SHAPE_SINE, SHAPE_PULSE, SHAPE_SAW] {
            let samples: Vec<f32> = (0..64)
                .map(|i| shape_value(shape, i as f32 / 64.0, 0.5))
                .collect();
            let max = samples.iter().cloned().fold(f32::MIN, f32::max);
            let min = samples.iter().cloned().fold(f32::MAX, f32::min);
            assert!(max > 0.9, "shape {shape} max {max}");
            assert!(min < -0.9, "shape {shape} min {min}");
        }
    }

    #[test]
    fn pulse_width_sets_the_high_fraction() {
        let high = (0..100)
            .filter(|i| shape_value(SHAPE_PULSE, *i as f32 / 100.0, 0.25) > 0.0)
            .count();
        assert_eq!(high, 25);
    }
}
