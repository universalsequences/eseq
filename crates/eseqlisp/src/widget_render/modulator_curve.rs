use std::collections::HashMap;

use super::{
    CellBuffer, GpuPrimitive, WidgetDefinition, WidgetInstance, WidgetViewport, ndc_bounds,
    resolve_named_color, styled_cell,
};
use crate::backend::Color;
use crate::layout::{Constraints, LayoutNode, MeasureCtx, Size, f64_to_f32, get_prop_num};
use crate::theme;
use crate::vm::Value;

pub struct ModulatorCurveWidget;

pub static MODULATOR_CURVE_WIDGET: ModulatorCurveWidget = ModulatorCurveWidget;

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

fn normalized_time_ms(value: f32, max: f32) -> f32 {
    if max <= 0.0 {
        0.0
    } else {
        (value.max(0.0) / max).clamp(0.0, 1.0).sqrt()
    }
}

impl WidgetDefinition for ModulatorCurveWidget {
    fn names(&self) -> &'static [&'static str] {
        &["modulator-curve"]
    }

    fn size_affecting_props(&self) -> &'static [&'static str] {
        &["width", "height"]
    }

    fn bindable_props(&self) -> &'static [&'static str] {
        &["rise", "fall", "phase", "level"]
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
            .clamp(8.0, constraints.max_width.max(8.0));
        let height = get_prop_num(node, "height")
            .map(f64_to_f32)
            .unwrap_or(4.0)
            .max(2.5);
        Some(Size { width, height })
    }

    fn tui_render(
        &self,
        props: &HashMap<String, Value>,
        rect: crate::layout::Rect,
        buf: &mut CellBuffer,
    ) {
        let rise = prop_num(props, "rise", 0.0);
        let fall = prop_num(props, "fall", 0.0);
        let label = format!("rise {:.0}ms  fall {:.0}ms", rise, fall);
        let row = rect.row.round() as u16;
        let col_start = rect.col.round() as u16;
        for (i, ch) in label.chars().enumerate() {
            if i >= rect.width.round() as usize {
                break;
            }
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
        MODULATOR_CURVE_SHADER.source(backend)
    }

    fn build_primitives(
        &self,
        widget_type: &str,
        node: &LayoutNode,
        viewport: WidgetViewport,
    ) -> Vec<GpuPrimitive> {
        let max_ms = prop_num(&node.props, "max-ms", 5000.0).max(1.0);
        let rise_t = normalized_time_ms(prop_num(&node.props, "rise", 0.0), max_ms);
        let fall_t = normalized_time_ms(prop_num(&node.props, "fall", 0.0), max_ms);
        let phase = prop_num(&node.props, "phase", -1.0);
        let level = prop_num(&node.props, "level", 1.0).clamp(0.0, 1.0);
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
                uniform_a: [rise_t, fall_t, max_ms, 0.0],
                uniform_b: [phase, level, 0.0, 0.0],
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

const MODULATOR_CURVE_SHADER: super::ShaderSources = super::ShaderSources::both(r#"
float mc_sdSegment(float2 p, float2 a, float2 b) {
    float2 pa = p - a;
    float2 ba = b - a;
    float h = clamp(dot(pa, ba) / max(dot(ba, ba), 1e-6), 0.0, 1.0);
    return length(pa - ba * h);
}

float2 mc_plot(float2 data) {
    float2 pad = float2(0.055, 0.12);
    return float2(
        pad.x + data.x * (1.0 - pad.x * 2.0),
        pad.y + (1.0 - data.y) * (1.0 - pad.y * 2.0));
}

float mc_curve_y(float x, float riseT, float fallT, float level) {
    float startX = 0.18;
    float endX = 0.82;
    float riseW = mix(0.004, 0.26, riseT);
    float fallW = mix(0.004, 0.26, fallT);

    if (x < startX) {
        return 0.0;
    }
    float raw = 0.0;
    if (x < startX + riseW) {
        float t = (x - startX) / max(riseW, 0.0001);
        raw = 1.0 - exp(-t * mix(18.0, 4.2, riseT));
    } else if (x < endX) {
        raw = 1.0;
    } else if (x < endX + fallW) {
        float t = (x - endX) / max(fallW, 0.0001);
        raw = exp(-t * mix(18.0, 4.2, fallT));
    } else {
        raw = 0.0;
    }
    return raw * clamp(level, 0.0, 1.0);
}

fragment float4 widget_frag(WidgetVaryings in [[stage_in]])
{
    float2 uv = in.uv;
    float aspect = max(in.aspect, 0.0001);
    float riseT = clamp(in.uniform_a.x, 0.0, 1.0);
    float fallT = clamp(in.uniform_a.y, 0.0, 1.0);
    float markerPhase = in.uniform_b.x;
    float level = clamp(in.uniform_b.y, 0.0, 1.0);

    float4 col = in.color_b;

    float dataX = clamp((uv.x - 0.055) / 0.89, 0.0, 1.0);
    float dataY = clamp((0.88 - uv.y) / 0.76, 0.0, 1.0);
    float curveAtX = mc_curve_y(dataX, riseT, fallT, level);
    float fill = smoothstep(curveAtX + 0.008, curveAtX - 0.008, dataY);
    col.rgb = mix(col.rgb, in.color_d.rgb, fill * in.color_d.a);

    float lineMask = 0.0;
    const int steps = 112;
    float prevX = 0.0;
    float prevY = mc_curve_y(0.0, riseT, fallT, level);
    for (int i = 1; i <= steps; i++) {
        float x = float(i) / float(steps);
        float y = mc_curve_y(x, riseT, fallT, level);
        float2 a = mc_plot(float2(prevX, prevY));
        float2 b = mc_plot(float2(x, y));
        float d = mc_sdSegment(float2(uv.x * aspect, uv.y), float2(a.x * aspect, a.y), float2(b.x * aspect, b.y));
        float aa = max(fwidth(d), 0.001);
        lineMask = max(lineMask, smoothstep(0.008 + aa, 0.0025, d));
        prevX = x;
        prevY = y;
    }

    col.rgb = mix(col.rgb, in.color_a.rgb, lineMask * in.color_a.a);
    col.a = max(col.a, max(lineMask * in.color_a.a, in.color_b.a));

    if (markerPhase >= 0.0) {
        float markerX = clamp(markerPhase, 0.0, 1.0);
        float2 marker = mc_plot(float2(markerX, mc_curve_y(markerX, riseT, fallT, level)));
        float markerDist = length(float2((uv.x - marker.x) * aspect, uv.y - marker.y));
        float outer = smoothstep(0.052, 0.038, markerDist);
        float inner = smoothstep(0.032, 0.020, markerDist);
        col.rgb = mix(col.rgb, float3(0.02, 0.025, 0.03), outer);
        col.rgb = mix(col.rgb, in.color_a.rgb, inner);
        col.a = max(col.a, outer);
    }
    return col;
}
"#, super::wgsl::MODULATOR_CURVE_SHADER);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalized_time_is_zero_for_zero_ms() {
        assert_eq!(normalized_time_ms(0.0, 5000.0), 0.0);
    }

    #[test]
    fn normalized_time_clamps_to_one() {
        assert_eq!(normalized_time_ms(6000.0, 5000.0), 1.0);
    }
}
