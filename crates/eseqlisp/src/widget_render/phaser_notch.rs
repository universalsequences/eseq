//! Display strip for the Phaser-Flanger builtin effect (Ableton-style).
//!
//! Phaser mode draws one vertical marker per notch on a log-frequency axis
//! with a dim band showing how far the LFO sweep (amount) can carry it.
//! Flanger/Doubler modes draw the two channel dots (L orange, R cyan) on a
//! log-time axis with the sweep range behind them.
//!
//! The notch layout math is dual-maintained with
//! `sequencer::phaser_flanger::notch_frequencies` — keep the two in sync.

use std::collections::HashMap;

#[cfg(target_os = "macos")]
use super::MetalLiveSpectrogramPrimitive;
use super::{
    CellBuffer, MetalPrimitive, WidgetDefinition, WidgetInstance, WidgetViewport, ndc_bounds,
    resolve_named_color, styled_cell,
};
use crate::backend::Color;
use crate::layout::{Constraints, LayoutNode, MeasureCtx, Size, f64_to_f32, get_prop_num};
use crate::theme;
use crate::vm::Value;

pub struct PhaserNotchWidget;

pub static PHASER_NOTCH_WIDGET: PhaserNotchWidget = PhaserNotchWidget;

const MAX_NOTCHES: usize = 12;

// Mirror of the DSP constants in sequencer::phaser_flanger.
const PHASER_SWEEP_OCT: f32 = 2.5;
const FLANGER_SWEEP_OCT: f32 = 1.0;
const DOUBLER_SWEEP_OCT: f32 = 0.35;
const SPREAD_OCT_PER_NOTCH: f32 = 1.2;
const SPREAD_LIN_PER_NOTCH: f32 = 1.6;
const SYNC_BEATS: [f32; 11] = [
    0.125,
    0.25,
    1.0 / 6.0,
    0.5,
    1.0 / 3.0,
    0.75,
    1.0,
    2.0 / 3.0,
    1.5,
    2.0,
    4.0,
];

// Display axes: frequency 20 Hz..20.48 kHz (10 octaves), time 0.1..409.6 ms
// (12 octaves), both log2.
const FREQ_MIN_HZ: f32 = 20.0;
const FREQ_SPAN_OCT: f32 = 10.0;
const TIME_MIN_MS: f32 = 0.1;
const TIME_SPAN_OCT: f32 = 12.0;

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

/// Static notch layout — mirror of `phaser_flanger::notch_frequencies`.
fn notch_frequencies(notches: usize, center: f32, spread: f32, blend: f32) -> Vec<f32> {
    let n = notches.clamp(1, MAX_NOTCHES);
    let center = center.clamp(20.0, 20000.0);
    let spread = spread.clamp(0.0, 1.0);
    let blend = blend.clamp(0.0, 1.0);
    let mid = (n as f32 - 1.0) * 0.5;
    (0..n)
        .map(|k| {
            let off = k as f32 - mid;
            let exp_f = center * (spread * off * SPREAD_OCT_PER_NOTCH).exp2();
            let lin_f = center * (1.0 + spread * off * SPREAD_LIN_PER_NOTCH).max(0.1);
            let f = exp_f.powf(1.0 - blend) * lin_f.powf(blend);
            f.clamp(20.0, 20000.0)
        })
        .collect()
}

fn freq_to_x(freq: f32) -> f32 {
    ((freq / FREQ_MIN_HZ).max(1.0e-3).log2() / FREQ_SPAN_OCT).clamp(0.0, 1.0)
}

fn time_to_x(ms: f32) -> f32 {
    ((ms / TIME_MIN_MS).max(1.0e-3).log2() / TIME_SPAN_OCT).clamp(0.0, 1.0)
}

struct Display {
    mode: usize,
    // Marker x positions (normalized 0..1 on the log axis); count entries.
    xs: Vec<f32>,
    // Sweep half-width in normalized axis units.
    sweep: f32,
    rate_hz: f32,
    shape: usize,
    stereo_phase: f32,
    anchor_x: f32,
    spread: f32,
    blend: f32,
}

fn display_from_props(props: &HashMap<String, Value>) -> Display {
    let mode = prop_num(props, "mode", 0.0).round().clamp(0.0, 2.0) as usize;
    let amount = prop_num(props, "amount", 0.25).clamp(0.0, 1.0);
    let sync = prop_num(props, "sync", 0.0) > 0.5;
    let rate_hz = if sync {
        let div = prop_num(props, "sync-div", 6.0)
            .round()
            .clamp(0.0, (SYNC_BEATS.len() - 1) as f32) as usize;
        prop_num(props, "bpm", 120.0).clamp(20.0, 999.0) / (60.0 * SYNC_BEATS[div])
    } else {
        prop_num(props, "rate", 0.15).clamp(0.01, 20.0)
    };
    let shape = prop_num(props, "lfo-shape", 0.0).round().clamp(0.0, 3.0) as usize;
    let stereo_phase = prop_num(props, "stereo", 20.0).clamp(0.0, 180.0) / 360.0;
    match mode {
        0 => {
            let notches = prop_num(props, "notches", 4.0).round().clamp(1.0, 12.0) as usize;
            let center = prop_num(props, "center", 400.0);
            let spread = prop_num(props, "spread", 0.35).clamp(0.0, 1.0);
            let blend = prop_num(props, "blend", 0.0).clamp(0.0, 1.0);
            let xs = notch_frequencies(notches, center, spread, blend)
                .into_iter()
                .map(freq_to_x)
                .collect();
            Display {
                mode,
                xs,
                sweep: amount * PHASER_SWEEP_OCT / FREQ_SPAN_OCT,
                rate_hz,
                shape,
                stereo_phase,
                anchor_x: freq_to_x(center.clamp(20.0, 20_000.0)),
                spread,
                blend,
            }
        }
        1 => {
            let anchor_x = time_to_x(prop_num(props, "flanger-time", 2.5).clamp(0.1, 20.0));
            Display {
                mode,
                xs: vec![anchor_x],
                sweep: amount * FLANGER_SWEEP_OCT / TIME_SPAN_OCT,
                rate_hz,
                shape,
                stereo_phase,
                anchor_x,
                spread: 0.0,
                blend: 0.0,
            }
        }
        _ => {
            let anchor_x = time_to_x(prop_num(props, "doubler-time", 80.0).clamp(2.0, 100.0));
            Display {
                mode,
                xs: vec![anchor_x],
                sweep: amount * DOUBLER_SWEEP_OCT / TIME_SPAN_OCT,
                rate_hz,
                shape,
                stereo_phase,
                anchor_x,
                spread: 0.0,
                blend: 0.0,
            }
        }
    }
}

impl WidgetDefinition for PhaserNotchWidget {
    fn names(&self) -> &'static [&'static str] {
        &["phaser-notch"]
    }

    fn size_affecting_props(&self) -> &'static [&'static str] {
        &["width", "height"]
    }

    fn bindable_props(&self) -> &'static [&'static str] {
        &[
            "mode",
            "notches",
            "center",
            "spread",
            "blend",
            "flanger-time",
            "doubler-time",
            "amount",
            "stereo",
            "sync",
            "rate",
            "sync-div",
            "lfo-shape",
            "bpm",
            "fft-size",
            "time-slices",
            "min-db",
            "max-db",
            "smoothing",
        ]
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
        let row = (rect.row + rect.height * 0.5).round() as u16;
        let col_start = rect.col.round() as u16;
        let mut cells = vec!['·'; width];
        for &x in &display.xs {
            let col = ((x * (width.saturating_sub(1)) as f32).round() as usize).min(width - 1);
            cells[col] = '|';
        }
        for (i, ch) in cells.into_iter().enumerate() {
            buf.set(
                row,
                col_start + i as u16,
                styled_cell(ch, theme::FG_MUTED(), None),
            );
        }
    }

    fn wants_animation_frames(&self, node: &LayoutNode) -> bool {
        prop_num(&node.props, "amount", 0.25) > 0.0001
    }

    fn metal_shader_uses_time(&self) -> bool {
        true
    }

    #[cfg(target_os = "macos")]
    fn metal_fragment_shader(&self, _widget_type: &str) -> Option<&'static str> {
        Some(PHASER_NOTCH_SHADER)
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
            Color::rgba(0.045, 0.048, 0.052, 0.72),
        );
        let grid_color = resolve_named_color(
            &node.props,
            "grid-color",
            Color::rgba(0.36, 0.36, 0.38, 0.30),
        );
        let left_color =
            resolve_named_color(&node.props, "left-color", Color::rgba(1.0, 0.62, 0.25, 1.0));
        let right_color = resolve_named_color(
            &node.props,
            "right-color",
            Color::rgba(0.45, 0.78, 0.95, 1.0),
        );
        let (ndc_min, ndc_max) = ndc_bounds(node.rect, viewport);
        let px_w = node.rect.width * viewport.cell_w;
        let px_h = node.rect.height * viewport.cell_h;

        let mut primitives = Vec::with_capacity(2);
        if display.mode == 0 {
            let spectrum = super::spectrogram::request_from_props(&node.props);
            primitives.push(MetalPrimitive::LiveSpectrogram(
                MetalLiveSpectrogramPrimitive {
                    rect: node.rect,
                    data_key: spectrum.data_key,
                    mode: 1,
                    freq_scale: 0,
                    min_hz: FREQ_MIN_HZ,
                    max_hz: 20_000.0,
                    min_color: Color::rgba(0.02, 0.025, 0.03, 0.28),
                    mid_color: Color::rgba(0.12, 0.30, 0.34, 0.18),
                    max_color: Color::rgba(0.35, 0.68, 0.72, 0.30),
                    eq_line_color: Color::rgba(0.42, 0.76, 0.78, 0.28),
                    eq_fill_color: Color::rgba(0.10, 0.36, 0.38, 0.12),
                    background_color: Color::rgba(0.02, 0.025, 0.03, 1.0),
                },
            ));
        }
        primitives.push(MetalPrimitive::WidgetInstance {
            widget_type: widget_type.to_string(),
            instance: WidgetInstance {
                ndc_min,
                ndc_max,
                value_t: display.shape as f32,
                orientation: 0.0,
                itime: viewport.time_seconds,
                uniform_a: [
                    display.mode as f32,
                    display.xs.len() as f32,
                    display.sweep,
                    display.rate_hz,
                ],
                uniform_b: [
                    display.anchor_x,
                    display.spread,
                    display.blend,
                    display.stereo_phase,
                ],
                uniform_c: [0.0; 4],
                uniform_d: [0.0; 4],
                color_a: left_color.to_rgba(),
                color_b: bg_color.to_rgba(),
                color_c: grid_color.to_rgba(),
                color_d: right_color.to_rgba(),
                corner_radius: 0.0,
                pixel_aspect: if px_h > 0.0 { px_w / px_h } else { 1.0 },
            },
            is_background: false,
        });
        primitives
    }
}

#[cfg(target_os = "macos")]
const PHASER_NOTCH_SHADER: &str = r#"
float phaserFlangerLfo(int shape, float phase)
{
    phase = fract(phase);
    if (shape == 1) {
        return (phase < 0.5) ? (4.0 * phase - 1.0) : (3.0 - 4.0 * phase);
    }
    if (shape == 2) { return 2.0 * phase - 1.0; }
    if (shape == 3) { return (phase < 0.5) ? 1.0 : -1.0; }
    return sin(6.28318530718 * phase);
}

fragment float4 widget_frag(WidgetVaryings in [[stage_in]])
{
    float2 uv = in.uv;
    float aspect = max(in.aspect, 0.0001);
    int mode = int(round(in.uniform_a.x));
    int count = clamp(int(round(in.uniform_a.y)), 0, 12);
    float sweep = clamp(in.uniform_a.z, 0.0, 0.5);
    float rateHz = max(in.uniform_a.w, 0.0001);
    int shape = clamp(int(round(in.value_t)), 0, 3);
    float phaseL = fract(in.itime * rateHz);
    float phaseR = fract(phaseL + clamp(in.uniform_b.w, 0.0, 0.5));
    float sweepL = sweep * phaserFlangerLfo(shape, phaseL);
    float sweepR = sweep * phaserFlangerLfo(shape, phaseR);
    float anchorX = in.uniform_b.x;
    float spread = clamp(in.uniform_b.y, 0.0, 1.0);
    float blend = clamp(in.uniform_b.z, 0.0, 1.0);

    float4 col = in.color_b;

    // Octave grid ticks along the log axis.
    float axisSpan = (mode == 0) ? 10.0 : 12.0;
    float gridT = fract(uv.x * axisSpan);
    float gridDist = min(gridT, 1.0 - gridT) / axisSpan;
    float gridAA = max(fwidth(uv.x), 0.0008);
    float gridMask = smoothstep(gridAA * 1.6, 0.0, gridDist) * 0.6;
    col.rgb = mix(col.rgb, in.color_c.rgb, gridMask * in.color_c.a);

    float aa = max(fwidth(uv.x), 0.0008);
    if (mode == 0) {
        // One vertical marker per notch with the LFO sweep band behind it.
        float yPad = smoothstep(0.04, 0.14, uv.y) * smoothstep(0.96, 0.86, uv.y);
        for (int i = 0; i < 12; i++) {
            if (i >= count) { break; }
            float offset = float(i) - (float(count) - 1.0) * 0.5;
            float exponentialX = anchorX + spread * offset * 1.2 / 10.0;
            float linearFactor = max(0.1, 1.0 + spread * offset * 1.6);
            float linearX = anchorX + log2(linearFactor) / 10.0;
            float baseX = clamp(mix(exponentialX, linearX, blend), 0.0, 1.0);
            float band = step(baseX - sweep, uv.x) * step(uv.x, baseX + sweep);
            col.rgb = mix(col.rgb, in.color_a.rgb, band * 0.10 * yPad);
            float lineL = smoothstep(aa * 2.2, aa * 0.4, abs(uv.x - (baseX + sweepL)));
            float lineR = smoothstep(aa * 2.2, aa * 0.4, abs(uv.x - (baseX + sweepR)));
            col.rgb = mix(col.rgb, in.color_a.rgb, lineL * 0.92 * yPad);
            col.rgb = mix(col.rgb, in.color_d.rgb, lineR * 0.88 * yPad);
        }
    } else {
        // Two channel dots on the log-time axis with the sweep range behind.
        float baseX = anchorX;
        float2 dotL = float2(baseX + sweepL, 0.38);
        float2 dotR = float2(baseX + sweepR, 0.62);
        float band = step(baseX - sweep, uv.x) * step(uv.x, baseX + sweep);
        float yBand = smoothstep(0.26, 0.36, uv.y) * smoothstep(0.74, 0.64, uv.y);
        col.rgb = mix(col.rgb, in.color_a.rgb, band * 0.07 * yBand);

        float dL = length(float2((uv.x - dotL.x) * aspect, uv.y - dotL.y));
        float dR = length(float2((uv.x - dotR.x) * aspect, uv.y - dotR.y));
        float rAA = max(fwidth(dL), 0.002);
        float maskL = smoothstep(0.045 + rAA, 0.045 - rAA, dL);
        float maskR = smoothstep(0.045 + rAA, 0.045 - rAA, dR);
        col.rgb = mix(col.rgb, in.color_a.rgb, maskL);
        col.rgb = mix(col.rgb, in.color_d.rgb, maskR);
    }
    return col;
}
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::AtomicU64;

    #[test]
    fn freq_axis_maps_edges() {
        assert!(freq_to_x(20.0) < 1.0e-4);
        assert!((freq_to_x(20480.0) - 1.0).abs() < 1.0e-4);
    }

    #[test]
    fn phaser_display_uses_notch_count() {
        let mut props = HashMap::new();
        props.insert("mode".to_string(), Value::Number(0.0));
        props.insert("notches".to_string(), Value::Number(6.0));
        props.insert("center".to_string(), Value::Number(400.0));
        let d = display_from_props(&props);
        assert_eq!(d.xs.len(), 6);
    }

    #[test]
    fn delay_modes_show_single_marker() {
        for (mode, key, val) in [(1.0, "flanger-time", 2.5), (2.0, "doubler-time", 80.0)] {
            let mut props = HashMap::new();
            props.insert("mode".to_string(), Value::Number(mode));
            props.insert(key.to_string(), Value::Number(val));
            let d = display_from_props(&props);
            assert_eq!(d.xs.len(), 1);
        }
    }

    #[test]
    fn display_reads_updated_reactive_parameter_slots_without_rebuild() {
        let center = Arc::new(AtomicU64::new((400.0_f64).to_bits()));
        let mut props = HashMap::from([(
            "center".to_string(),
            Value::ReactiveRef {
                namespace: "SEQ".to_string(),
                field: "phaser-center".to_string(),
                index: None,
                kind: crate::vm::BindingKind::Float,
                slot: Arc::clone(&center),
            },
        )]);
        props.insert("notches".to_string(), Value::Number(1.0));

        let before = display_from_props(&props).xs[0];
        center.store((1600.0_f64).to_bits(), std::sync::atomic::Ordering::Release);
        let after = display_from_props(&props).xs[0];

        assert!(
            after > before,
            "center binding should move the notch marker"
        );
    }
}
