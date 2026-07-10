//! Magnitude-response display for a Roar stage filter.
//!
//! Draws the selected filter type's magnitude response (type / freq / res
//! props) on a log-frequency axis, 20 Hz .. 20.48 kHz, ±24 dB. The response
//! math mirrors the SVF / comb / resample / dispersion kernels in
//! `sequencer::roar` closely enough for display purposes.

use std::collections::HashMap;

use super::{
    CellBuffer, MetalPrimitive, WidgetDefinition, WidgetInstance, WidgetViewport, ndc_bounds,
    resolve_named_color, styled_cell,
};
use crate::backend::Color;
use crate::layout::{Constraints, LayoutNode, MeasureCtx, Size, f64_to_f32, get_prop_num};
use crate::theme;
use crate::vm::Value;

pub struct RoarFilterWidget;

pub static ROAR_FILTER_WIDGET: RoarFilterWidget = RoarFilterWidget;

const FREQ_MIN_HZ: f32 = 20.0;
const FREQ_SPAN_OCT: f32 = 10.0;

const STAGE_COLORS: [Color; 3] = [
    Color::rgba(1.0, 0.62, 0.25, 1.0),
    Color::rgba(0.45, 0.78, 0.95, 1.0),
    Color::rgba(0.95, 0.45, 0.62, 1.0),
];

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

/// Res 0..1 → Q 0.5..~12; mirror of `sequencer::roar::res_to_q`.
fn res_to_q(res: f32) -> f32 {
    0.5 * (24.0_f32).powf(res.clamp(0.0, 1.0))
}

/// Display magnitude (linear) at `freq` for the stage filter.
pub fn filter_magnitude(filter: usize, cutoff: f32, res: f32, freq: f32) -> f32 {
    let cutoff = cutoff.clamp(20.0, 16_000.0);
    let res = res.clamp(0.0, 1.0);
    let omega = freq / cutoff;
    let k = 1.0 / res_to_q(res);
    let denom = ((1.0 - omega * omega).powi(2) + (k * omega).powi(2)).sqrt().max(1.0e-6);
    match filter {
        1 => omega / denom,                       // bp (peaks at Q)
        2 => omega * omega / denom,               // hp
        3 => (1.0 - omega * omega).abs() / denom, // notch
        4 => (1.0 + omega * omega) / denom,       // peak
        5 => {
            // Morph: fixed BP-leaning LP/BP/HP blend.
            let re = 0.3 * (1.0 - omega * omega);
            let im = 0.4 * k * omega;
            (re * re + im * im).sqrt() / denom
        }
        6 => {
            // Comb: y = 0.5 (w + z w), w = x + fb z w with delay 1/cutoff.
            let theta = std::f32::consts::TAU * freq / cutoff;
            let fb = res * 0.9;
            let num = 0.5 * (2.0 + 2.0 * theta.cos()).max(0.0).sqrt();
            let den = (1.0 + fb * fb - 2.0 * fb * theta.cos()).max(1.0e-6).sqrt();
            num / den
        }
        7 => {
            // Resample: zero-order hold sinc roll-off toward the hold rate.
            if cutoff >= 15_900.0 {
                1.0
            } else {
                let t = (std::f32::consts::PI * freq / cutoff).min(std::f32::consts::PI);
                if t < 1.0e-3 { 1.0 } else { (t.sin() / t).abs() }
            }
        }
        8 => 1.0, // dispersion: allpass, flat magnitude
        _ => 1.0 / denom, // lp
    }
}

struct Display {
    filter: usize,
    freq: f32,
    res: f32,
    stage: usize,
}

fn display_from_props(props: &HashMap<String, Value>) -> Display {
    Display {
        filter: prop_num(props, "filter", 0.0).round().clamp(0.0, 8.0) as usize,
        freq: prop_num(props, "freq", 16_000.0).clamp(20.0, 16_000.0),
        res: prop_num(props, "res", 0.1).clamp(0.0, 1.0),
        stage: prop_num(props, "stage", 0.0).round().clamp(0.0, 2.0) as usize,
    }
}

impl WidgetDefinition for RoarFilterWidget {
    fn names(&self) -> &'static [&'static str] {
        &["roar-filter"]
    }

    fn size_affecting_props(&self) -> &'static [&'static str] {
        &["width", "height"]
    }

    fn bindable_props(&self) -> &'static [&'static str] {
        &["filter", "freq", "res", "stage"]
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
        let display = display_from_props(props);
        let width = rect.width.round().max(1.0) as usize;
        let height = rect.height.round().max(1.0) as usize;
        let col_start = rect.col.round() as u16;
        let row_start = rect.row.round() as u16;
        for col in 0..width {
            let t = col as f32 / (width.saturating_sub(1)).max(1) as f32;
            let freq = FREQ_MIN_HZ * (t * FREQ_SPAN_OCT).exp2();
            let mag = filter_magnitude(display.filter, display.freq, display.res, freq);
            let db = (20.0 * mag.max(1.0e-6).log10()).clamp(-24.0, 24.0);
            let row =
                (((24.0 - db) / 48.0) * (height.saturating_sub(1)) as f32).round() as usize;
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
        Some(ROAR_FILTER_SHADER)
    }

    #[cfg(target_os = "macos")]
    fn build_metal_primitives(
        &self,
        widget_type: &str,
        node: &LayoutNode,
        viewport: WidgetViewport,
    ) -> Vec<MetalPrimitive> {
        let display = display_from_props(&node.props);
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
        let (ndc_min, ndc_max) = ndc_bounds(node.rect, viewport);
        let px_w = node.rect.width * viewport.cell_w;
        let px_h = node.rect.height * viewport.cell_h;
        vec![MetalPrimitive::WidgetInstance {
            widget_type: widget_type.to_string(),
            instance: WidgetInstance {
                ndc_min,
                ndc_max,
                value_t: display.filter as f32,
                orientation: 0.0,
                itime: viewport.time_seconds,
                uniform_a: [display.freq, display.res, 0.0, 0.0],
                uniform_b: [0.0; 4],
                uniform_c: [0.0; 4],
                uniform_d: [0.0; 4],
                color_a: curve_color.to_rgba(),
                color_b: bg_color.to_rgba(),
                color_c: grid_color.to_rgba(),
                color_d: curve_color.to_rgba(),
                corner_radius: 0.0,
                pixel_aspect: if px_h > 0.0 { px_w / px_h } else { 1.0 },
            },
            is_background: false,
        }]
    }
}

// Dual-maintained with `filter_magnitude` above.
#[cfg(target_os = "macos")]
const ROAR_FILTER_SHADER: &str = r#"
float roarFilterMagnitude(int filterType, float cutoff, float res, float freq)
{
    float omega = freq / max(cutoff, 20.0);
    float q = 0.5 * pow(24.0, clamp(res, 0.0, 1.0));
    float k = 1.0 / q;
    float one = 1.0 - omega * omega;
    float denom = max(sqrt(one * one + k * k * omega * omega), 1.0e-6);
    if (filterType == 1) { return omega / denom; }
    if (filterType == 2) { return omega * omega / denom; }
    if (filterType == 3) { return abs(one) / denom; }
    if (filterType == 4) { return (1.0 + omega * omega) / denom; }
    if (filterType == 5) {
        float re = 0.3 * one;
        float im = 0.4 * k * omega;
        return sqrt(re * re + im * im) / denom;
    }
    if (filterType == 6) {
        float theta = 6.28318530718 * freq / max(cutoff, 20.0);
        float fb = res * 0.9;
        float num = 0.5 * sqrt(max(2.0 + 2.0 * cos(theta), 0.0));
        float den = sqrt(max(1.0 + fb * fb - 2.0 * fb * cos(theta), 1.0e-6));
        return num / den;
    }
    if (filterType == 7) {
        if (cutoff >= 15900.0) { return 1.0; }
        float t = min(3.14159265 * freq / cutoff, 3.14159265);
        return (t < 1.0e-3) ? 1.0 : abs(sin(t) / t);
    }
    if (filterType == 8) { return 1.0; }
    return 1.0 / denom;
}

fragment float4 widget_frag(WidgetVaryings in [[stage_in]])
{
    float2 uv = in.uv;
    int filterType = clamp(int(round(in.value_t)), 0, 8);
    float cutoff = clamp(in.uniform_a.x, 20.0, 16000.0);
    float res = clamp(in.uniform_a.y, 0.0, 1.0);

    float4 col = in.color_b;

    // Octave grid + 0 dB line.
    float gridT = fract(uv.x * 10.0);
    float gridDist = min(gridT, 1.0 - gridT) / 10.0;
    float aa = max(fwidth(uv.x), 0.0008);
    float gridMask = smoothstep(aa * 1.6, 0.0, gridDist) * 0.5;
    float zeroLine = smoothstep(aa * 1.6, 0.0, abs(uv.y - 0.5)) * 0.7;
    col.rgb = mix(col.rgb, in.color_c.rgb, max(gridMask, zeroLine) * in.color_c.a);

    // Response curve: -24..+24 dB across the height.
    float freq = 20.0 * exp2(uv.x * 10.0);
    float mag = roarFilterMagnitude(filterType, cutoff, res, freq);
    float db = clamp(20.0 * log10(max(mag, 1.0e-6)), -24.0, 24.0);
    float curveV = (24.0 - db) / 48.0;
    float aaY = max(fwidth(uv.y), 0.0015);
    float line = smoothstep(aaY * 2.6, aaY * 0.5, abs(uv.y - curveV));
    col.rgb = mix(col.rgb, in.color_a.rgb, line * 0.95);
    // Soft fill below the curve.
    float fill = step(curveV, uv.y) * 0.10;
    col.rgb = mix(col.rgb, in.color_a.rgb, fill);
    return col;
}
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lp_response_attenuates_above_cutoff() {
        let pass = filter_magnitude(0, 1000.0, 0.1, 100.0);
        let stop = filter_magnitude(0, 1000.0, 0.1, 8000.0);
        assert!(pass > 0.9, "passband should be near unity: {pass}");
        assert!(stop < 0.05, "stopband should be attenuated: {stop}");
    }

    #[test]
    fn bp_peaks_at_center_with_resonance() {
        let center = filter_magnitude(1, 1000.0, 0.8, 1000.0);
        let off = filter_magnitude(1, 1000.0, 0.8, 4000.0);
        assert!(center > 2.0, "resonant BP should peak: {center}");
        assert!(off < center * 0.3);
    }

    #[test]
    fn comb_notches_between_harmonics() {
        let peak = filter_magnitude(6, 1000.0, 0.5, 1000.0);
        let notch = filter_magnitude(6, 1000.0, 0.5, 1500.0);
        assert!(peak > notch * 3.0, "comb should alternate: {peak} vs {notch}");
    }

    #[test]
    fn dispersion_is_flat() {
        for freq in [50.0, 500.0, 5000.0] {
            assert_eq!(filter_magnitude(8, 1000.0, 0.5, freq), 1.0);
        }
    }
}
