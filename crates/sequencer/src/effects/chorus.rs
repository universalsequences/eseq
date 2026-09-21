//! Feed-forward stereo chorus: mono wet excitation, a shared delay ring with
//! two phase-related triangle reads, and independent wet-path filtering.
//! Delay modulation is the full excursion above the minimum/base delay.
//! All continuous controls accept the host's four sample-rate modulation lanes.

use super::{
    EffectDescriptor, InstrumentModulationTarget, InstrumentModulatorDescriptor, ModulationMode,
    ParamDescriptor, ParamKind, ParamScaling,
};
use crate::audiograph::NodeVTable;
use crate::instruments::voice_modulator;
use std::os::raw::{c_int, c_void};

const SLOTS: usize = voice_modulator::SLOT_COUNT;
const COUNT: usize = 9;
const RATE: usize = 0;
const DEPTH: usize = 1;
const BASE: usize = 2;
const PHASE: usize = 3;
const HP: usize = 4;
const LP: usize = 5;
const WET_LP: usize = 6;
const MIX: usize = 7;
const OUTPUT: usize = 8;
const DEPTH_BASE: usize = 1 + COUNT;

struct Control {
    name: &'static str,
    min: f32,
    max: f32,
    default: f32,
    unit: &'static str,
    logarithmic: bool,
}

// One source of truth for UI ranges, DSP clamps, defaults and modulation depth.
const CONTROLS: [Control; COUNT] = [
    Control {
        name: "rate",
        min: 0.02,
        max: 15.0,
        default: 0.6,
        unit: "Hz",
        logarithmic: true,
    },
    Control {
        name: "delay modulation",
        min: 0.0,
        max: 20.0,
        default: 3.0,
        unit: "ms",
        logarithmic: false,
    },
    Control {
        name: "base delay",
        min: 0.5,
        max: 30.0,
        default: 5.0,
        unit: "ms",
        logarithmic: true,
    },
    Control {
        name: "stereo phase",
        min: 0.0,
        max: 360.0,
        default: 180.0,
        unit: "°",
        logarithmic: false,
    },
    Control {
        name: "input highpass",
        min: 20.0,
        max: 20000.0,
        default: 80.0,
        unit: "Hz",
        logarithmic: true,
    },
    Control {
        name: "input lowpass",
        min: 20.0,
        max: 20000.0,
        default: 12000.0,
        unit: "Hz",
        logarithmic: true,
    },
    Control {
        name: "wet lowpass",
        min: 20.0,
        max: 20000.0,
        default: 6000.0,
        unit: "Hz",
        logarithmic: true,
    },
    Control {
        name: "mix",
        min: 0.0,
        max: 1.0,
        default: 0.35,
        unit: "%",
        logarithmic: false,
    },
    Control {
        name: "output",
        min: -24.0,
        max: 12.0,
        default: 0.0,
        unit: "dB",
        logarithmic: false,
    },
];

pub fn descriptor() -> EffectDescriptor {
    let mut params = vec![EffectDescriptor::enabled_param(0, 1.0)];
    for (i, c) in CONTROLS.iter().enumerate() {
        params.push(ParamDescriptor {
            name: c.name.to_string(),
            min: c.min,
            max: c.max,
            default: c.default,
            kind: ParamKind::Continuous {
                unit: Some(c.unit.to_string()),
            },
            scaling: if c.logarithmic {
                ParamScaling::Exponential
            } else {
                ParamScaling::Linear
            },
            node_param_idx: (1 + i) as u32,
            node_param_span: 1,
            host_control: None,
            ui_metadata: Some(super::modulatable_ui_metadata()),
        });
    }
    params.extend(voice_modulator::effect_param_descriptors());
    let mut targets = Vec::new();
    for (i, c) in CONTROLS.iter().enumerate() {
        let range = c.max - c.min;
        for slot in 0..SLOTS {
            let depth_param_idx = params.len();
            params.push(ParamDescriptor {
                name: format!("mod {} slot {} amt", c.name, slot + 1),
                min: -range,
                max: range,
                default: 0.0,
                kind: ParamKind::Continuous { unit: None },
                scaling: ParamScaling::Linear,
                node_param_idx: (DEPTH_BASE + i * SLOTS + slot) as u32,
                node_param_span: 1,
                host_control: None,
                ui_metadata: None,
            });
            targets.push(InstrumentModulationTarget {
                base_param_idx: 1 + i,
                source_param_idx: None,
                modulator_slot: slot + 1,
                depth_param_idx,
                active_param_idx: None,
                depth_min: -range,
                depth_max: range,
                depth_unit: Some(c.unit.to_string()),
                mod_mode: ModulationMode::Additive,
            });
        }
    }
    EffectDescriptor {
        name: "Chorus".to_string(),
        params,
        tensor_params: Vec::new(),
        input_channels: 2 + SLOTS,
        output_channels: 2,
        instrument_modulators: (1..=SLOTS)
            .map(|slot| InstrumentModulatorDescriptor {
                slot,
                label: voice_modulator::modulator_slot_label(slot, ""),
            })
            .collect(),
        instrument_modulation_targets: targets,
        declared_latency_samples: None,
    }
}

/// Standard topology-preserving state-variable filter, Butterworth Q.
/// The response editor uses the same 12 dB/octave, fixed-Q response.
#[derive(Clone, Copy, Default)]
struct Filter {
    ic1: f64,
    ic2: f64,
}
impl Filter {
    fn tick(&mut self, x: f64, g: f64, highpass: bool) -> f64 {
        let k = std::f64::consts::SQRT_2;
        let a = 1.0 / (1.0 + g * (g + k));
        let v1 = a * (self.ic1 + g * (x - self.ic2));
        let v2 = self.ic2 + g * v1;
        self.ic1 = flush(2.0 * v1 - self.ic1);
        self.ic2 = flush(2.0 * v2 - self.ic2);
        if highpass {
            x - k * v1 - v2
        } else {
            v2
        }
    }
}
fn flush(x: f64) -> f64 {
    if x.abs() < 1e-25 {
        0.0
    } else {
        x
    }
}
fn bounded(x: f32, lo: f32, hi: f32, default: f32) -> f32 {
    if x.is_finite() {
        x.clamp(lo, hi)
    } else {
        default
    }
}
fn slew(x: f64, target: f64, alpha: f64) -> f64 {
    let y = x + alpha * (target - x);
    if (y - target).abs() < 1e-10 {
        target
    } else {
        y
    }
}
fn triangle(phase: f64) -> f64 {
    1.0 - (2.0 * phase - 1.0).abs()
}

// The graph's default parameter setter addresses f32 cells at the beginning.
// Runtime state follows; no pointers/heap ownership are stored in the header.
#[repr(C)]
struct State {
    enabled: f32,
    params: [f32; COUNT],
    depths: [[f32; SLOTS]; COUNT],
    sample_rate: f32,
    capacity: usize,
    write: usize,
    phase: f64,
    smooth: [f64; COUNT],
    wet: f64,
    gain: f64,
    first: bool,
    filters: [Filter; 4],
}

fn capacity(sample_rate: f32) -> usize {
    let seconds = (CONTROLS[BASE].max + CONTROLS[DEPTH].max) * 0.001;
    ((super::safe_sample_rate(sample_rate) * seconds).ceil() as usize + 4).next_power_of_two()
}
pub fn state_size(sample_rate: f32) -> usize {
    std::mem::size_of::<State>() + capacity(sample_rate) * std::mem::size_of::<f32>()
}
unsafe fn ring(state: *mut c_void) -> *mut f32 {
    state.cast::<u8>().add(std::mem::size_of::<State>()).cast()
}

impl State {
    fn effective(&self, sources: [f32; SLOTS]) -> [f64; COUNT] {
        std::array::from_fn(|i| {
            let c = &CONTROLS[i];
            let mut value = bounded(self.params[i], c.min, c.max, c.default);
            for slot in 0..SLOTS {
                value += bounded(sources[slot], -1.0, 1.0, 0.0)
                    * bounded(self.depths[i][slot], c.min - c.max, c.max - c.min, 0.0);
            }
            value.clamp(c.min, c.max) as f64
        })
    }
    /// Four-point cubic interpolation; the minimum two-sample read ensures
    /// all four neighbors are already written. No extra dry-path latency.
    unsafe fn read(&self, buffer: *const f32, delay: f64) -> f64 {
        let pos = (self.write as f64 - delay.max(2.0)).rem_euclid(self.capacity as f64);
        let index = pos.floor() as usize;
        let t = pos - index as f64;
        let mask = self.capacity - 1;
        let a = *buffer.add(index.wrapping_sub(1) & mask) as f64;
        let b = *buffer.add(index) as f64;
        let c = *buffer.add((index + 1) & mask) as f64;
        let d = *buffer.add((index + 2) & mask) as f64;
        b + 0.5 * t * (c - a + t * (2.0 * a - 5.0 * b + 4.0 * c - d + t * (3.0 * (b - c) + d - a)))
    }
}

unsafe extern "C" fn init(state: *mut c_void, sample_rate: c_int, _: c_int, _: *const c_void) {
    let sample_rate = super::safe_sample_rate(sample_rate as f32);
    std::ptr::write(
        state.cast::<State>(),
        State {
            enabled: 1.0,
            params: std::array::from_fn(|i| CONTROLS[i].default),
            depths: [[0.0; SLOTS]; COUNT],
            sample_rate,
            capacity: capacity(sample_rate),
            write: 0,
            phase: 0.0,
            smooth: [0.0; COUNT],
            wet: 0.0,
            gain: 1.0,
            first: true,
            filters: [Filter::default(); 4],
        },
    );
    reset(state);
}
unsafe extern "C" fn reset(state: *mut c_void) {
    let s = &mut *state.cast::<State>();
    s.write = 0;
    s.phase = 0.0;
    s.first = true;
    s.filters = [Filter::default(); 4];
    std::ptr::write_bytes(ring(state), 0, s.capacity);
}
unsafe extern "C" fn migrate(new: *mut c_void, old: *const c_void) {
    let src = &*old.cast::<State>();
    let dst = &mut *new.cast::<State>();
    if src.sample_rate == dst.sample_rate && src.capacity == dst.capacity {
        std::ptr::copy_nonoverlapping(
            old.cast::<u8>(),
            new.cast::<u8>(),
            state_size(src.sample_rate),
        );
    } else {
        dst.enabled = src.enabled;
        dst.params = src.params;
        dst.depths = src.depths;
        reset(new);
    }
}
unsafe extern "C" fn process(
    inp: *const *mut f32,
    out: *const *mut f32,
    frames: c_int,
    state: *mut c_void,
    _: *mut c_void,
) {
    if frames <= 0 {
        return;
    }
    let s = &mut *state.cast::<State>();
    let buffer = ring(state);
    let sr = s.sample_rate as f64;
    let alpha = 1.0 - (-1.0 / (0.01 * sr)).exp();
    let enabled = bounded(s.enabled, 0.0, 1.0, 1.0) >= 0.5;
    for frame in 0..frames as usize {
        let dry = [*(*inp).add(frame), *(*inp.add(1)).add(frame)];
        let dry = dry.map(|x| if x.is_finite() { x } else { 0.0 });
        let sources = std::array::from_fn(|slot| *(*inp.add(2 + slot)).add(frame));
        let targets = s.effective(sources);
        for i in 0..COUNT {
            s.smooth[i] = if s.first {
                targets[i]
            } else {
                slew(s.smooth[i], targets[i], alpha)
            };
        }
        let wet = if enabled { s.smooth[MIX] } else { 0.0 };
        let gain = if enabled {
            10.0_f64.powf(s.smooth[OUTPUT] / 20.0)
        } else {
            1.0
        };
        s.wet = if s.first {
            wet
        } else {
            slew(s.wet, wet, alpha)
        };
        s.gain = if s.first {
            gain
        } else {
            slew(s.gain, gain, alpha)
        };
        s.first = false;
        let coefficient = |hz: f64| (std::f64::consts::PI * hz.min(sr * 0.45) / sr).tan();
        let mono = 0.5 * (dry[0] as f64 + dry[1] as f64);
        let high = s.filters[0].tick(mono, coefficient(s.smooth[HP]), true);
        *buffer.add(s.write) = s.filters[1].tick(high, coefficient(s.smooth[LP]), false) as f32;
        let wet_g = coefficient(s.smooth[WET_LP]);
        for ch in 0..2 {
            let offset = (s.smooth[PHASE] / 360.0).fract();
            let phase = (s.phase + ch as f64 * offset).fract();
            let delay = (s.smooth[BASE] + s.smooth[DEPTH] * triangle(phase)) * 0.001 * sr;
            let tap = s.read(buffer, delay);
            let wet = s.filters[2 + ch].tick(tap, wet_g, false);
            *(*out.add(ch)).add(frame) = if s.wet == 0.0 && s.gain == 1.0 {
                dry[ch]
            } else {
                ((dry[ch] as f64 * (1.0 - s.wet) + wet * s.wet) * s.gain) as f32
            };
        }
        s.phase = (s.phase + s.smooth[RATE] / sr).fract();
        s.write = (s.write + 1) & (s.capacity - 1);
    }
}

pub fn vtable() -> NodeVTable {
    NodeVTable {
        init: Some(init),
        process: Some(process),
        reset: Some(reset),
        migrate: Some(migrate),
        ..NodeVTable::default()
    }
}

#[cfg(test)]
mod tests;
