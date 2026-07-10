//! Transfer-curve display for the Roar builtin effect's selected stage.
//!
//! Draws the stage's shaper curve in the stage color, a dashed vertical bias
//! marker, and a translucent live drive-region overlay: the horizontal span
//! of the curve currently exercised by the input, from the per-stage
//! pre-shaper min/max meters the host publishes as `roar-meter:` keyed
//! `BandMeterFrame`s (level_db\[stage\] carries the linear min/max pair).
//!
//! The curve formulas are dual-maintained with
//! `sequencer::roar::shaper_transfer` — keep the two in sync.

use std::collections::HashMap;

use super::live_audio::{LiveAudioSourceSelector, source_from_props};
use super::{
    CellBuffer, MetalPrimitive, WidgetDefinition, WidgetInstance, WidgetViewport, ndc_bounds,
    resolve_named_color, styled_cell,
};
use crate::backend::Color;
use crate::layout::{Constraints, LayoutNode, MeasureCtx, Size, f64_to_f32, get_prop_num};
use crate::live_audio::BandMeterFrame;
use crate::theme;
use crate::vm::Value;

pub struct RoarShaperWidget;

pub static ROAR_SHAPER_WIDGET: RoarShaperWidget = RoarShaperWidget;

// Curve domain drawn along x.
const X_SPAN: f32 = 2.0;

// Default stage colors (stage 1 orange, stage 2 cyan, stage 3 pink).
const STAGE_COLORS: [Color; 3] = [
    Color::rgba(1.0, 0.62, 0.25, 1.0),
    Color::rgba(0.45, 0.78, 0.95, 1.0),
    Color::rgba(0.95, 0.45, 0.62, 1.0),
];

#[derive(Clone, Debug, PartialEq)]
pub struct RoarMeterRequest {
    pub data_key: String,
    pub source: LiveAudioSourceSelector,
}

pub fn request_from_props(props: &HashMap<String, Value>) -> RoarMeterRequest {
    let source = source_from_props(props);
    let data_key = format!("roar-meter:{}", source.key_fragment());
    RoarMeterRequest { data_key, source }
}

/// Collects the live meter requests for every visible roar-shaper widget so
/// the host can watch the effect nodes behind them.
pub fn collect_roar_meter_requests(layout: &LayoutNode) -> Vec<RoarMeterRequest> {
    let mut requests = Vec::new();
    collect_roar_meter_requests_into(layout, &mut requests);
    requests
}

fn collect_roar_meter_requests_into(layout: &LayoutNode, requests: &mut Vec<RoarMeterRequest>) {
    if layout.widget_type == "roar-shaper" && layout.rect.width > 0.0 && layout.rect.height > 0.0 {
        requests.push(request_from_props(&layout.props));
    }
    for child in &layout.children {
        collect_roar_meter_requests_into(child, requests);
    }
}

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

/// Mirror of `sequencer::roar::shaper_transfer` (stateless part).
pub fn shaper_transfer(shaper: usize, amount: f32, x: f32) -> f32 {
    let a = amount.clamp(0.0, 1.0);
    let soft_sine = |x: f32| {
        x.clamp(-std::f32::consts::FRAC_PI_2, std::f32::consts::FRAC_PI_2)
            .sin()
    };
    match shaper {
        1 => x.clamp(-1.0, 1.0),
        2 => {
            let levels = 64.0 + (2.0 - 64.0) * a;
            (x * levels).round() / levels
        }
        3 => {
            if x >= 0.0 {
                let t = 0.35;
                if x <= t {
                    x
                } else {
                    t + (1.0 - (-(x - t) * 3.0).exp()) / 3.0
                }
            } else {
                1.2 * (x / 1.2).tanh()
            }
        }
        4 => {
            let u = x.max(-2.4);
            (u + 0.2 * u * u).tanh()
        }
        5 => 2.0 * x.max(0.0),
        6 => x.abs(),
        7 => {
            let cheb = (3.0 * x - 4.0 * x * x * x) / 3.0;
            ((1.0 - a) * x + a * cheb).clamp(-1.0, 1.0)
        }
        8 => {
            let mut y = x;
            for k in [1.9_f32, 1.5, 1.2] {
                y = (k * y).sin() / k;
            }
            y
        }
        9 => {
            let t = ((x + 1.0) * 0.25).rem_euclid(1.0);
            1.0 - 4.0 * (t - 0.5).abs()
        }
        _ => soft_sine(x),
    }
}

struct Display {
    shaper: usize,
    amount: f32,
    bias: f32,
    stage: usize,
    // Live drive region in curve-x units, already clamped to the domain.
    drive_min: f32,
    drive_max: f32,
}

fn display_from_props(props: &HashMap<String, Value>, frame: Option<BandMeterFrame>) -> Display {
    let shaper = prop_num(props, "shaper", 0.0).round().clamp(0.0, 11.0) as usize;
    let amount = prop_num(props, "amount", 0.0).clamp(0.0, 1.0);
    let bias = prop_num(props, "bias", 0.0).clamp(-1.0, 1.0);
    let stage = prop_num(props, "stage", 0.0).round().clamp(0.0, 2.0) as usize;
    let (drive_min, drive_max) = frame
        .map(|frame| {
            let pair = frame.level_db[stage];
            (
                pair[0].clamp(-X_SPAN, X_SPAN),
                pair[1].clamp(-X_SPAN, X_SPAN),
            )
        })
        .unwrap_or((0.0, 0.0));
    Display {
        shaper,
        amount,
        bias,
        stage,
        drive_min,
        drive_max,
    }
}

impl WidgetDefinition for RoarShaperWidget {
    fn names(&self) -> &'static [&'static str] {
        &["roar-shaper"]
    }

    fn size_affecting_props(&self) -> &'static [&'static str] {
        &["width", "height"]
    }

    fn bindable_props(&self) -> &'static [&'static str] {
        &["shaper", "amount", "bias", "stage"]
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
            .clamp(4.0, constraints.max_width.max(4.0));
        let height = get_prop_num(node, "height")
            .map(f64_to_f32)
            .unwrap_or(3.0)
            .max(1.5);
        Some(Size { width, height })
    }

    fn tui_render(
        &self,
        props: &HashMap<String, Value>,
        rect: crate::layout::Rect,
        buf: &mut CellBuffer,
    ) {
        let display = display_from_props(props, None);
        let width = rect.width.round().max(1.0) as usize;
        let height = rect.height.round().max(1.0) as usize;
        let col_start = rect.col.round() as u16;
        let row_start = rect.row.round() as u16;
        for col in 0..width {
            let x = (col as f32 / (width.saturating_sub(1)).max(1) as f32) * 2.0 * X_SPAN - X_SPAN;
            let y = shaper_transfer(display.shaper, display.amount, x).clamp(-1.2, 1.2);
            let row = ((1.0 - (y / 1.2 + 1.0) * 0.5) * (height.saturating_sub(1)) as f32).round()
                as usize;
            buf.set(
                row_start + row.min(height - 1) as u16,
                col_start + col as u16,
                styled_cell('·', theme::FG_MUTED(), None),
            );
        }
    }

    fn metal_shader_uses_time(&self) -> bool {
        false
    }

    #[cfg(target_os = "macos")]
    fn metal_fragment_shader(&self, _widget_type: &str) -> Option<&'static str> {
        Some(ROAR_SHAPER_SHADER)
    }

    #[cfg(target_os = "macos")]
    fn build_metal_primitives(
        &self,
        widget_type: &str,
        node: &LayoutNode,
        viewport: WidgetViewport,
    ) -> Vec<MetalPrimitive> {
        let request = request_from_props(&node.props);
        let frame = crate::live_audio::band_meter_frame(&request.data_key);
        let display = display_from_props(&node.props, frame);
        let bg_color = resolve_named_color(
            &node.props,
            "background-color",
            Color::rgba(0.045, 0.048, 0.052, 0.9),
        );
        let grid_color = resolve_named_color(
            &node.props,
            "grid-color",
            Color::rgba(0.36, 0.36, 0.38, 0.30),
        );
        let curve_color = resolve_named_color(
            &node.props,
            "curve-color",
            STAGE_COLORS[display.stage.min(2)],
        );
        let bias_color = resolve_named_color(
            &node.props,
            "bias-color",
            Color::rgba(0.92, 0.92, 0.94, 0.85),
        );
        let (ndc_min, ndc_max) = ndc_bounds(node.rect, viewport);
        let px_w = node.rect.width * viewport.cell_w;
        let px_h = node.rect.height * viewport.cell_h;
        vec![MetalPrimitive::WidgetInstance {
            widget_type: widget_type.to_string(),
            instance: WidgetInstance {
                ndc_min,
                ndc_max,
                value_t: display.shaper as f32,
                orientation: 0.0,
                itime: viewport.time_seconds,
                uniform_a: [
                    display.amount,
                    display.bias,
                    display.drive_min,
                    display.drive_max,
                ],
                uniform_b: [0.0; 4],
                uniform_c: [0.0; 4],
                uniform_d: [0.0; 4],
                color_a: curve_color.to_rgba(),
                color_b: bg_color.to_rgba(),
                color_c: grid_color.to_rgba(),
                color_d: bias_color.to_rgba(),
                corner_radius: 0.0,
                pixel_aspect: if px_h > 0.0 { px_w / px_h } else { 1.0 },
            },
            is_background: false,
        }]
    }
}

// Dual-maintained with `sequencer::roar::shaper_transfer` and the Rust
// mirror above.
#[cfg(target_os = "macos")]
const ROAR_SHAPER_SHADER: &str = r#"
float roarShaperCurve(int shaper, float a, float x)
{
    if (shaper == 1) { return clamp(x, -1.0, 1.0); }
    if (shaper == 2) {
        float levels = 64.0 + (2.0 - 64.0) * a;
        return round(x * levels) / levels;
    }
    if (shaper == 3) {
        if (x >= 0.0) {
            float t = 0.35;
            return (x <= t) ? x : t + (1.0 - exp(-(x - t) * 3.0)) / 3.0;
        }
        return 1.2 * tanh(x / 1.2);
    }
    if (shaper == 4) {
        float u = max(x, -2.4);
        return tanh(u + 0.2 * u * u);
    }
    if (shaper == 5) { return 2.0 * max(x, 0.0); }
    if (shaper == 6) { return abs(x); }
    if (shaper == 7) {
        float cheb = (3.0 * x - 4.0 * x * x * x) / 3.0;
        return clamp((1.0 - a) * x + a * cheb, -1.0, 1.0);
    }
    if (shaper == 8) {
        float y = x;
        y = sin(1.9 * y) / 1.9;
        y = sin(1.5 * y) / 1.5;
        y = sin(1.2 * y) / 1.2;
        return y;
    }
    if (shaper == 9) {
        float t = fract((x + 1.0) * 0.25);
        return 1.0 - 4.0 * abs(t - 0.5);
    }
    return sin(clamp(x, -1.5707963, 1.5707963));
}

fragment float4 widget_frag(WidgetVaryings in [[stage_in]])
{
    float2 uv = in.uv;
    int shaper = clamp(int(round(in.value_t)), 0, 11);
    float amount = clamp(in.uniform_a.x, 0.0, 1.0);
    float bias = clamp(in.uniform_a.y, -1.0, 1.0);
    float driveMin = clamp(in.uniform_a.z, -2.0, 2.0);
    float driveMax = clamp(in.uniform_a.w, -2.0, 2.0);

    float x = (uv.x * 2.0 - 1.0) * 2.0; // curve domain -2..2
    float4 col = in.color_b;

    // Axis grid: center lines and ±1 ticks.
    float aa = max(fwidth(uv.x), 0.0008);
    float axisX = smoothstep(aa * 1.8, 0.0, abs(uv.x - 0.5));
    float axisY = smoothstep(aa * 1.8, 0.0, abs(uv.y - 0.5));
    float tick1 = smoothstep(aa * 1.4, 0.0, abs(abs(x) - 1.0));
    float grid = max(max(axisX, axisY) * 0.8, tick1 * 0.4);
    col.rgb = mix(col.rgb, in.color_c.rgb, grid * in.color_c.a);

    // Live drive region: the input span currently exercising the curve.
    if (driveMax > driveMin + 0.001) {
        float band = step(driveMin, x) * step(x, driveMax);
        col.rgb = mix(col.rgb, in.color_a.rgb, band * 0.14);
    }

    // Dashed bias marker.
    float biasU = (bias * 0.5 + 2.0) / 4.0;
    float dash = step(0.5, fract(uv.y * 9.0));
    float biasLine = smoothstep(aa * 1.8, aa * 0.3, abs(uv.x - biasU)) * dash;
    col.rgb = mix(col.rgb, in.color_d.rgb, biasLine * in.color_d.a);

    // Transfer curve, y in -1.4..1.4 of widget height.
    float y = roarShaperCurve(shaper, amount, x);
    float curveV = 0.5 - y / 2.8;
    float aaY = max(fwidth(uv.y), 0.0015);
    float line = smoothstep(aaY * 2.6, aaY * 0.5, abs(uv.y - curveV));
    col.rgb = mix(col.rgb, in.color_a.rgb, line * 0.95);
    return col;
}
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_key_uses_roar_meter_prefix() {
        let props = HashMap::new();
        let request = request_from_props(&props);
        assert!(request.data_key.starts_with("roar-meter:"));
    }

    #[test]
    fn drive_region_follows_selected_stage() {
        let mut props = HashMap::new();
        props.insert("stage".to_string(), Value::Number(1.0));
        let frame = BandMeterFrame {
            revision: 1,
            level_db: [[0.0, 0.0], [-0.4, 1.3], [0.0, 0.0]],
            gain_db: [0.0; 3],
        };
        let display = display_from_props(&props, Some(frame));
        assert!((display.drive_min + 0.4).abs() < 1.0e-6);
        assert!((display.drive_max - 1.3).abs() < 1.0e-6);
    }

    #[test]
    fn curve_mirror_matches_slope_normalization() {
        for shaper in [0usize, 1, 3, 4, 7, 8, 9] {
            let h = 1.0e-3;
            let slope =
                (shaper_transfer(shaper, 0.5, h) - shaper_transfer(shaper, 0.5, -h)) / (2.0 * h);
            assert!(
                (slope - 1.0).abs() < 0.15,
                "shaper {shaper} origin slope {slope}"
            );
        }
    }
}
