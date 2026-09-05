//! Digi Drift's phase-aligned, pre-filter source preview.
//!
//! Dual-maintained with `content/instruments/Synths/Digi Drift/dsp.lisp`:
//! morph-osc, basic-osc, osc-frequencies and source-mixer. This is a cycle
//! diagram, not an oscilloscope: free-running phase, analog drift and internal
//! voice envelopes / matrix are not simulated. Host modulation uses the same
//! live offsets as the knobs. One base-pitch cycle includes both octave ratios
//! and detune. PolyBLEP uses the diagram's sampling rate (512 samples per base
//! cycle), not the audio device's current pitch/sample rate.
//! Noise uses a fixed realization so repainting never animates a static patch.

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use super::{
    CellBuffer, GpuPrimitive, GpuRectPrimitive, WidgetDefinition, WidgetViewport,
    resolve_named_color, styled_cell,
};
use crate::layout::{Constraints, LayoutNode, MeasureCtx, Rect, Size, f64_to_f32, get_prop_num};
use crate::theme;
use crate::vm::Value;

pub struct DriftWaveformWidget;
pub static DRIFT_WAVEFORM_WIDGET: DriftWaveformWidget = DriftWaveformWidget;
const SAMPLES: usize = 513;
// Bounded value cache, not an owner-id registry: deleted/rebuilt widgets cannot
// strand entries. Identical patches share samples; no ui_epoch/time dependency.
const CACHE_CAPACITY: usize = 128;
const PARAMS: [(&str, f32, f32, f32); 11] = [
    ("osc1-wave", 4.0, 0.0, 6.0),
    ("osc1-shape", 0.5, 0.0, 1.0),
    ("osc1-octave", 0.0, -3.0, 3.0),
    ("osc1-on", 1.0, 0.0, 1.0),
    ("osc1-gain-db", -6.0, -36.0, 12.0),
    ("osc2-wave", 3.0, 0.0, 4.0),
    ("osc2-octave", -1.0, -3.0, 3.0),
    ("osc2-detune", 0.0, -24.0, 24.0),
    ("osc2-on", 1.0, 0.0, 1.0),
    ("osc2-gain-db", -6.0, -36.0, 12.0),
    ("noise-gain-db", -60.0, -60.0, 12.0),
];

fn number(value: &Value) -> Option<f32> {
    let value = match value {
        Value::Number(n) => *n as f32,
        Value::ReactiveRef { slot, .. } => crate::reactive::read_float_slot(slot) as f32,
        _ => return None,
    };
    value.is_finite().then_some(value)
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Inputs([f32; 11]);
impl Inputs {
    fn from_props(props: &HashMap<String, Value>) -> Self {
        Self(std::array::from_fn(|i| {
            let (name, default, min, max) = PARAMS[i];
            let base = props.get(name).and_then(number).unwrap_or(default);
            let offset = props
                .get(&format!("{name}-mod"))
                .and_then(number)
                .unwrap_or(0.0);
            (base + offset).clamp(min, max)
        }))
    }
}

fn asym_triangle(phase: f32, peak: f32) -> f32 {
    if phase < peak {
        2.0 * phase / peak - 1.0
    } else {
        2.0 * (1.0 - phase) / (1.0 - peak) - 1.0
    }
}
// DGen's polyblep / polyblep_saw / polyblep_pulse library macros.
fn polyblep(phase: f32, dt: f32) -> f32 {
    let dt = dt.clamp(0.000001, 0.5);
    if phase < dt {
        let x = phase / dt;
        2.0 * x - x * x - 1.0
    } else if phase > 1.0 - dt {
        let x = (phase - 1.0) / dt;
        x * x + 2.0 * x + 1.0
    } else {
        0.0
    }
}
fn pulse(phase: f32, width: f32, dt: f32) -> f32 {
    let raw = if phase < width { 1.0 } else { -1.0 };
    raw + polyblep(phase, dt) - polyblep((phase - width).rem_euclid(1.0), dt)
}
fn driven_saw(saw: f32, drive: f32) -> f32 {
    (saw * drive).tanh() / drive.tanh()
}
fn morph(phase: f32, wave: f32, shape: f32, dt: f32) -> f32 {
    let saw = 2.0 * phase - 1.0 - polyblep(phase, dt);
    let tri = asym_triangle(phase, 0.05 + shape * 0.9);
    match wave.round() as u32 {
        0 => (phase * std::f32::consts::TAU).sin(),
        1 => tri,
        2 => (1.0 - shape) * saw + shape * tri,
        3 => driven_saw(saw, 1.0 + shape * 5.0),
        4 => driven_saw(saw, 1.0 + shape * 1.5),
        5 => pulse(phase, 0.05 + shape * 0.9, dt),
        _ => pulse(phase, 0.5 + (shape - 0.5) * 0.6, dt),
    }
}
fn basic(phase: f32, wave: f32, dt: f32) -> f32 {
    let saw = 2.0 * phase - 1.0 - polyblep(phase, dt);
    match wave.round() as u32 {
        0 => (phase * std::f32::consts::TAU).sin(),
        // DGen's (triangle ph) is UNIPOLAR; unlike morph-osc's asym triangle.
        1 => 1.0 - (2.0 * phase - 1.0).abs(),
        2 => driven_saw(saw, 3.0),
        3 => saw,
        _ => pulse(phase, 0.5, dt),
    }
}
fn db_amp(db: f32) -> f32 {
    (0.1151292546 * db).exp()
}

fn evaluate(inputs: Inputs) -> Vec<f32> {
    let [
        wave1,
        shape,
        octave1,
        on1,
        db1,
        wave2,
        octave2,
        detune,
        on2,
        db2,
        noise_db,
    ] = inputs.0;
    let ratio1 = octave1.round().exp2();
    let ratio2 = (octave2.round() + detune / 12.0).exp2();
    let dt1 = ratio1 / (SAMPLES - 1) as f32;
    let dt2 = ratio2 / (SAMPLES - 1) as f32;
    let gain1 = if on1 >= 0.5 { db_amp(db1) } else { 0.0 };
    let gain2 = if on2 >= 0.5 { db_amp(db2) } else { 0.0 };
    let noise_gain = if noise_db > -59.5 {
        db_amp(noise_db)
    } else {
        0.0
    };
    let mut random = 0x6d2b79f5u32;
    (0..SAMPLES)
        .map(|i| {
            random ^= random << 13;
            random ^= random >> 17;
            random ^= random << 5;
            let noise = random as f64 / u32::MAX as f64 * 2.0 - 1.0;
            let phase = i as f32 / (SAMPLES - 1) as f32;
            gain1 * morph((phase * ratio1).fract(), wave1, shape, dt1)
                + gain2 * basic((phase * ratio2).fract(), wave2, dt2)
                + noise_gain * noise as f32
        })
        .collect()
}

#[derive(Default)]
struct SampleCache {
    entries: VecDeque<(Inputs, Arc<Vec<f32>>)>,
    evaluations: u64,
}
impl SampleCache {
    fn samples(&mut self, inputs: Inputs) -> Arc<Vec<f32>> {
        if let Some(index) = self.entries.iter().position(|(key, _)| *key == inputs) {
            let entry = self.entries.remove(index).unwrap();
            let samples = entry.1.clone();
            self.entries.push_back(entry);
            return samples;
        }
        let samples = Arc::new(evaluate(inputs));
        self.evaluations += 1;
        if self.entries.len() == CACHE_CAPACITY {
            self.entries.pop_front();
        }
        self.entries.push_back((inputs, samples.clone()));
        samples
    }
}
thread_local! {
    static CACHE: RefCell<SampleCache> = RefCell::new(SampleCache::default());
}

/// Diagnostic counter for retained-frame / perf probes, on the UI thread.
pub fn evaluation_count() -> u64 {
    CACHE.with(|cache| cache.borrow().evaluations)
}

impl WidgetDefinition for DriftWaveformWidget {
    fn names(&self) -> &'static [&'static str] {
        &["drift-waveform"]
    }
    fn size_affecting_props(&self) -> &'static [&'static str] {
        &["width", "height"]
    }
    fn bindable_props(&self) -> &'static [&'static str] {
        &[
            "osc1-wave",
            "osc1-shape",
            "osc1-octave",
            "osc1-on",
            "osc1-gain-db",
            "osc2-wave",
            "osc2-octave",
            "osc2-detune",
            "osc2-on",
            "osc2-gain-db",
            "noise-gain-db",
            "osc1-shape-mod",
            "osc1-gain-db-mod",
            "osc2-detune-mod",
            "osc2-gain-db-mod",
            "noise-gain-db-mod",
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
        Some(Size {
            width: get_prop_num(node, "width")
                .map(f64_to_f32)
                .unwrap_or(constraints.max_width)
                .min(constraints.max_width),
            height: get_prop_num(node, "height")
                .map(f64_to_f32)
                .unwrap_or(3.0)
                .max(1.0),
        })
    }
    fn tui_render(&self, props: &HashMap<String, Value>, rect: Rect, buf: &mut CellBuffer) {
        let samples = CACHE.with(|cache| cache.borrow_mut().samples(Inputs::from_props(props)));
        let range = display_range(&samples);
        for x in 0..rect.width.max(0.0) as usize {
            let i = x * (SAMPLES - 1) / (rect.width as usize).max(1);
            let y = (0.5 - samples[i] / (2.0 * range)) * (rect.height - 1.0).max(0.0);
            buf.set(
                (rect.row + y).round() as u16,
                rect.col as u16 + x as u16,
                styled_cell('·', theme::FG(), None),
            );
        }
    }
    fn build_primitives(
        &self,
        _widget_type: &str,
        node: &LayoutNode,
        viewport: WidgetViewport,
    ) -> Vec<GpuPrimitive> {
        let samples =
            CACHE.with(|cache| cache.borrow_mut().samples(Inputs::from_props(&node.props)));
        let range = display_range(&samples);
        let color = resolve_named_color(&node.props, "wave-color", theme::FG());
        let background = resolve_named_color(&node.props, "background-color", theme::BG());
        let mut primitives = vec![GpuPrimitive::Rect(GpuRectPrimitive {
            rect: node.rect,
            color: background,
        })];
        let points: Vec<_> = samples
            .iter()
            .enumerate()
            .map(|(i, y)| {
                [
                    node.rect.col
                        + 0.2
                        + (node.rect.width - 0.4).max(0.0) * i as f32 / (SAMPLES - 1) as f32,
                    node.rect.row + node.rect.height * (0.5 - 0.44 * y / range),
                ]
            })
            .collect();
        let mut mesh = super::stroke::ShadedMesh::new();
        mesh.push_polyline(&points, color, viewport, 0.75);
        mesh.push_into(&mut primitives);
        primitives
    }
}

// Preserve level changes at ordinary gains, expanding headroom only when the
// sum exceeds +/-2. Never clip or normalize each oscillator independently.
fn display_range(samples: &[f32]) -> f32 {
    samples
        .iter()
        .fold(2.0f32, |range, sample| range.max(sample.abs()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_math_and_mix_match_drift() {
        assert_eq!(asym_triangle(0.0, 0.3), -1.0);
        assert!((asym_triangle(0.3, 0.3) - 1.0).abs() < 1e-6);
        let dt = 1.0 / 512.0;
        assert_eq!(basic(0.0, 1.0, dt), 0.0);
        assert_eq!(basic(0.5, 1.0, dt), 1.0);
        assert_eq!(basic(0.0, 3.0, dt), 0.0); // corrected saw discontinuity
        assert_eq!(pulse(0.0, 0.5, dt), 0.0);
        assert_eq!(pulse(0.5, 0.5, dt), 0.0);
        assert_eq!(morph(0.2, 5.0, 0.0, dt), -1.0);
        assert_eq!(morph(0.2, 5.0, 1.0, dt), 1.0);
        assert!((morph(0.25, 3.0, 0.4, dt) - driven_saw(-0.5, 3.0)).abs() < 1e-6);
        let mut inputs = Inputs::from_props(&HashMap::new());
        inputs.0[3] = 0.0;
        inputs.0[8] = 0.0;
        assert!(evaluate(inputs).iter().all(|y| *y == 0.0));
        inputs.0[10] = -59.5;
        assert!(evaluate(inputs).iter().all(|y| *y == 0.0));
        inputs.0[10] = 0.0;
        let noise = evaluate(inputs);
        assert_eq!(noise, evaluate(inputs));
        assert!(noise.iter().any(|y| y.abs() > 0.5));
    }

    #[test]
    fn cache_ignores_unrelated_props_and_reuses_idle_samples() {
        let mut props = HashMap::new();
        let mut cache = SampleCache::default();
        let first = cache.samples(Inputs::from_props(&props));
        for i in 0..120 {
            props.insert("ui_epoch".into(), Value::Number(i as f64));
            props.insert("lp_freq".into(), Value::Number(i as f64 * 100.0));
            assert!(Arc::ptr_eq(
                &first,
                &cache.samples(Inputs::from_props(&props))
            ));
        }
        assert_eq!(cache.evaluations, 1);
        for (name, value) in [
            ("osc1-shape", 0.8),
            ("osc2-detune", 3.0),
            ("osc1-gain-db", 0.0),
            ("osc2-gain-db", -12.0),
            ("noise-gain-db", -6.0),
            ("osc1-wave", 1.0),
            ("osc2-wave", 0.0),
            ("osc1-octave", 1.0),
            ("osc2-octave", 0.0),
            ("osc1-on", 0.0),
            ("osc2-on", 0.0),
        ] {
            let changed = HashMap::from([(name.into(), Value::Number(value))]);
            assert_ne!(
                *first,
                *cache.samples(Inputs::from_props(&changed)),
                "{name}"
            );
        }
    }
}
