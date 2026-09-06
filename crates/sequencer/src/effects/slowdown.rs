//! Rolling-buffer varispeed insert. The read head falls behind live input at
//! `1 - speed` samples/frame, then restarts through a complementary crossfade.
//! Length/smooth changes latch at cycle boundaries; speed, tone and wet gain
//! slew continuously. Timing is tempo-relative, not song-position locked.
//!
//! All storage (including a normalized windowed-sinc interpolation table) lives
//! in the graph-owned state allocation. Processing never allocates or locks.

use super::{
    EffectDescriptor, InstrumentModulationTarget, InstrumentModulatorDescriptor, ModulationMode,
    ParamDescriptor, ParamKind, ParamScaling,
};
use crate::audiograph::NodeVTable;
use crate::instruments::voice_modulator;
use std::os::raw::{c_int, c_void};

const ENABLED: usize = 0;
const SPEED: usize = 1;
const SYNC: usize = 2;
const TIME: usize = 3;
const BEATS: usize = 4;
const SMOOTH: usize = 5;
const TONE: usize = 6;
const MIX: usize = 7;
pub const PARAM_BPM: u64 = 8;
const PARAM_COUNT: usize = 9;
const DEFAULTS: [f32; PARAM_COUNT] = [1.0, 0.5, 1.0, 500.0, 1.0, 20.0, 20000.0, 1.0, 120.0];
const TAPS: usize = 32;
const PHASES: usize = 256;
const BASE_DELAY: f64 = 17.0;
// Four beats at the supported tempo floor of 20 BPM, plus the outgoing fade.
const MAX_HISTORY_SECONDS: f32 = 12.2;
const MOD_SLOTS: usize = voice_modulator::SLOT_COUNT;
const DEPTH_BASE: usize = PARAM_COUNT;

struct ModTarget {
    param: usize,
    min: f32,
    max: f32,
    depth: f32,
    unit: &'static str,
}

// Depths are offsets in each destination's own units. This single contract
// drives the DSP, depth controls, modulation rings and effective-value telemetry.
const MOD_TARGETS: [ModTarget; 6] = [
    ModTarget {
        param: SPEED,
        min: 0.25,
        max: 1.0,
        depth: 0.75,
        unit: "×",
    },
    ModTarget {
        param: TIME,
        min: 40.0,
        max: 4000.0,
        depth: 4000.0,
        unit: "ms",
    },
    ModTarget {
        param: BEATS,
        min: 0.125,
        max: 4.0,
        depth: 4.0,
        unit: "beats",
    },
    ModTarget {
        param: SMOOTH,
        min: 1.0,
        max: 100.0,
        depth: 100.0,
        unit: "ms",
    },
    ModTarget {
        param: TONE,
        min: 200.0,
        max: 20000.0,
        depth: 20000.0,
        unit: "Hz",
    },
    ModTarget {
        param: MIX,
        min: 0.0,
        max: 1.0,
        depth: 1.0,
        unit: "%",
    },
];

#[repr(C)]
struct State {
    params: [f32; PARAM_COUNT],
    mod_depths: [[f32; MOD_SLOTS]; MOD_TARGETS.len()],
    sample_rate: f32,
    capacity: usize,
    write: usize,
    age: usize,
    period: usize,
    fade_length: usize,
    fade_age: usize,
    delay: f64,
    outgoing_delay: f64,
    speed: f64,
    wet: f64,
    tone: f64,
    lowpass: [f32; 2],
    table: [[f32; TAPS]; PHASES + 1],
}

fn capacity(sample_rate: f32) -> usize {
    ((super::safe_sample_rate(sample_rate) * MAX_HISTORY_SECONDS).ceil() as usize + TAPS * 2)
        .next_power_of_two()
}

/// Byte size, unlike the older builtins' float-count constants. Both allocation
/// and init must use the graph sample rate; higher rates retain the same history.
pub fn state_size(sample_rate: f32) -> usize {
    std::mem::size_of::<State>() + 2 * capacity(sample_rate) * std::mem::size_of::<f32>()
}

pub fn descriptor() -> EffectDescriptor {
    let mut params = vec![EffectDescriptor::enabled_param(
        ENABLED as u32,
        DEFAULTS[ENABLED],
    )];
    for (index, name, min, max, unit, scaling) in [
        (SPEED, "speed", 0.25, 1.0, Some("×"), ParamScaling::Linear),
        (SYNC, "sync", 0.0, 1.0, None, ParamScaling::Linear),
        (
            TIME,
            "time",
            40.0,
            4000.0,
            Some("ms"),
            ParamScaling::Exponential,
        ),
        (
            BEATS,
            "beats",
            0.125,
            4.0,
            Some("beats"),
            ParamScaling::Exponential,
        ),
        (
            SMOOTH,
            "smooth",
            1.0,
            100.0,
            Some("ms"),
            ParamScaling::Exponential,
        ),
        (
            TONE,
            "tone",
            200.0,
            20000.0,
            Some("Hz"),
            ParamScaling::Exponential,
        ),
        (MIX, "mix", 0.0, 1.0, Some("%"), ParamScaling::Linear),
    ] {
        params.push(ParamDescriptor {
            name: name.to_string(),
            min,
            max,
            default: DEFAULTS[index],
            kind: if index == SYNC {
                ParamKind::Boolean
            } else {
                ParamKind::Continuous {
                    unit: unit.map(str::to_string),
                }
            },
            scaling,
            node_param_idx: index as u32,
            node_param_span: 1,
            host_control: None,
            ui_metadata: (index != SYNC).then(super::modulatable_ui_metadata),
        });
    }
    params.extend(voice_modulator::effect_param_descriptors());
    let mut targets = Vec::new();
    for (target_idx, target) in MOD_TARGETS.iter().enumerate() {
        let name = params[target.param].name.clone();
        for slot in 0..MOD_SLOTS {
            let depth_param_idx = params.len();
            params.push(ParamDescriptor {
                name: format!("mod {name} slot {} amt", slot + 1),
                min: -target.depth,
                max: target.depth,
                default: 0.0,
                // Depth cells stay in native DSP units. Display units live
                // on the target; marking this cell '%' would make the host
                // scale its value independently of its native depth bounds.
                kind: ParamKind::Continuous { unit: None },
                scaling: ParamScaling::Linear,
                node_param_idx: (DEPTH_BASE + target_idx * MOD_SLOTS + slot) as u32,
                node_param_span: 1,
                host_control: None,
                ui_metadata: None,
            });
            targets.push(InstrumentModulationTarget {
                base_param_idx: target.param,
                source_param_idx: None,
                modulator_slot: slot + 1,
                depth_param_idx,
                active_param_idx: None,
                depth_min: -target.depth,
                depth_max: target.depth,
                depth_unit: Some(target.unit.to_string()),
                mod_mode: ModulationMode::Additive,
            });
        }
    }
    EffectDescriptor {
        name: "Slowdown".to_string(),
        params,
        tensor_params: Vec::new(),
        input_channels: 2 + MOD_SLOTS,
        output_channels: 2,
        instrument_modulators: (1..=MOD_SLOTS)
            .map(|slot| InstrumentModulatorDescriptor {
                slot,
                label: voice_modulator::modulator_slot_label(slot, ""),
            })
            .collect(),
        instrument_modulation_targets: targets,
    }
}

fn finite(value: f32, min: f32, max: f32, default: f32) -> f32 {
    if value.is_finite() {
        value.clamp(min, max)
    } else {
        default
    }
}

// Double precision keeps a long speed ramp from stalling short of unity and
// accumulating an unintended pitch shift. Settle exact endpoints below 1e-9.
fn slew(value: f64, target: f64, coefficient: f64) -> f64 {
    let next = value + coefficient * (target - value);
    if next == value || (next - target).abs() < 1e-9 {
        target
    } else {
        next
    }
}

impl State {
    fn effective_params(&self, sources: [f32; MOD_SLOTS]) -> [f32; PARAM_COUNT] {
        let mut params = self.params;
        for (idx, target) in MOD_TARGETS.iter().enumerate() {
            let amount: f32 = (0..MOD_SLOTS)
                .map(|slot| {
                    finite(sources[slot], -1.0, 1.0, 0.0)
                        * finite(self.mod_depths[idx][slot], -target.depth, target.depth, 0.0)
                })
                .sum();
            let base = finite(
                params[target.param],
                target.min,
                target.max,
                DEFAULTS[target.param],
            );
            params[target.param] = (base + amount).clamp(target.min, target.max);
        }
        params
    }

    fn latch_cycle(&mut self, params: &[f32; PARAM_COUNT]) {
        let seconds = if finite(params[SYNC], 0.0, 1.0, 1.0) >= 0.5 {
            let bpm = finite(params[PARAM_BPM as usize], 20.0, 400.0, 120.0);
            60.0 / bpm * params[BEATS]
        } else {
            0.001 * params[TIME]
        };
        self.period = (seconds * self.sample_rate).round().max(2.0) as usize;
        self.fade_length = ((params[SMOOTH] * 0.001 * self.sample_rate).round() as usize)
            .clamp(1, self.period / 2);
    }

    fn reset_runtime(&mut self) {
        self.write = 0;
        self.age = 0;
        self.delay = BASE_DELAY;
        self.outgoing_delay = BASE_DELAY;
        self.speed = finite(self.params[SPEED], 0.25, 1.0, 0.5) as f64;
        // Fade in wet processing on startup/reset; dry remains sample-exact.
        self.wet = 0.0;
        self.tone = finite(self.params[TONE], 200.0, 20000.0, 20000.0) as f64;
        self.lowpass = [0.0; 2];
        self.latch_cycle(&self.effective_params([0.0; MOD_SLOTS]));
        self.fade_age = self.fade_length;
    }

    unsafe fn read(&self, ring: *const f32, delay: f64) -> [f32; 2] {
        let position = self.write as f64 - delay;
        let integer = position.floor() as isize;
        let phase = ((position - position.floor()) * PHASES as f64) as f32;
        let index = (phase as usize).min(PHASES - 1);
        let fraction = phase - index as f32;
        let mut result = [0.0; 2];
        for tap in 0..TAPS {
            let weight = self.table[index][tap]
                + fraction * (self.table[index + 1][tap] - self.table[index][tap]);
            let pos = (integer + tap as isize - 15) as usize & (self.capacity - 1);
            result[0] += weight * *ring.add(pos);
            result[1] += weight * *ring.add(self.capacity + pos);
        }
        result
    }
}

unsafe fn ring(state: *mut c_void) -> *mut f32 {
    state.cast::<u8>().add(std::mem::size_of::<State>()).cast()
}

unsafe extern "C" fn init(state: *mut c_void, sample_rate: c_int, _: c_int, _: *const c_void) {
    let sr = super::safe_sample_rate(sample_rate as f32);
    let mut table = [[0.0; TAPS]; PHASES + 1];
    for (phase, weights) in table.iter_mut().enumerate() {
        let fraction = phase as f64 / PHASES as f64;
        let mut sum = 0.0;
        for (tap, weight) in weights.iter_mut().enumerate() {
            let x = tap as f64 - 15.0 - fraction;
            let sinc = if x.abs() < 1e-12 {
                1.0
            } else {
                (std::f64::consts::PI * x).sin() / (std::f64::consts::PI * x)
            };
            let window = 0.5 + 0.5 * (std::f64::consts::PI * x / 16.0).cos();
            *weight = (sinc * window) as f32;
            sum += *weight;
        }
        for weight in weights {
            *weight /= sum;
        }
    }
    std::ptr::write(
        state.cast::<State>(),
        State {
            params: DEFAULTS,
            mod_depths: [[0.0; MOD_SLOTS]; MOD_TARGETS.len()],
            sample_rate: sr,
            capacity: capacity(sr),
            write: 0,
            age: 0,
            period: 0,
            fade_length: 0,
            fade_age: 0,
            delay: BASE_DELAY,
            outgoing_delay: BASE_DELAY,
            speed: 0.5,
            wet: 0.0,
            tone: 20000.0,
            lowpass: [0.0; 2],
            table,
        },
    );
    reset(state);
}

unsafe extern "C" fn reset(state: *mut c_void) {
    let s = &mut *state.cast::<State>();
    s.reset_runtime();
    std::ptr::write_bytes(ring(state), 0, 2 * s.capacity);
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
        // A rate change cannot reuse sample-domain history or coefficients.
        dst.params = src.params;
        dst.mod_depths = src.mod_depths;
        reset(new);
    }
}

unsafe extern "C" fn process(
    inp: *const *mut f32,
    out: *const *mut f32,
    nframes: c_int,
    state: *mut c_void,
    _: *mut c_void,
) {
    if nframes <= 0 {
        return;
    }
    let s = &mut *state.cast::<State>();
    let buffer = ring(state);
    let inputs = [*inp, *inp.add(1)];
    let outputs = [*out, *out.add(1)];
    let mod_inputs: [*mut f32; MOD_SLOTS] = std::array::from_fn(|slot| *inp.add(2 + slot));
    let coefficient = 1.0 - (-1.0 / (0.005 * s.sample_rate as f64)).exp();
    for i in 0..nframes as usize {
        let dry = [*inputs[0].add(i), *inputs[1].add(i)];
        for ch in 0..2 {
            // Do not let a nonfinite upstream sample poison the history forever.
            *buffer.add(ch * s.capacity + s.write) =
                if dry[ch].is_finite() { dry[ch] } else { 0.0 };
        }
        let sources = std::array::from_fn(|slot| *mod_inputs[slot].add(i));
        let params = s.effective_params(sources);
        if s.age == 0 {
            s.latch_cycle(&params);
        }
        if s.age >= s.period {
            s.outgoing_delay = s.delay;
            s.delay = BASE_DELAY;
            s.age = 0;
            s.latch_cycle(&params);
            s.fade_age = 0;
        }
        let wet = if finite(params[ENABLED], 0.0, 1.0, 1.0) >= 0.5 {
            params[MIX]
        } else {
            0.0
        };
        s.speed = slew(s.speed, params[SPEED] as f64, coefficient);
        s.wet = slew(s.wet, wet as f64, coefficient);
        s.tone = slew(s.tone, params[TONE] as f64, coefficient);
        let mut sample = s.read(buffer, s.delay);
        if s.fade_age < s.fade_length {
            let previous = s.read(buffer, s.outgoing_delay);
            let a = s.fade_age as f32 / s.fade_length as f32;
            for ch in 0..2 {
                sample[ch] = previous[ch] + a * (sample[ch] - previous[ch]);
            }
            s.fade_age += 1;
        }
        let coef = 1.0
            - (-std::f32::consts::TAU * (s.tone as f32).min(s.sample_rate * 0.45) / s.sample_rate)
                .exp();
        let wet = s.wet as f32;
        for ch in 0..2 {
            s.lowpass[ch] += coef * (sample[ch] - s.lowpass[ch]);
            if s.lowpass[ch].abs() < 1e-20 {
                s.lowpass[ch] = 0.0;
            }
            *outputs[ch].add(i) = if s.wet == 0.0 {
                dry[ch]
            } else {
                dry[ch] * (1.0 - wet) + s.lowpass[ch] * wet
            };
        }
        let advance = 1.0 - s.speed;
        s.delay += advance;
        s.outgoing_delay += advance;
        s.age += 1;
        s.write = (s.write + 1) & (s.capacity - 1);
    }
}

#[cfg(test)]
mod tests;

pub fn vtable() -> NodeVTable {
    NodeVTable {
        init: Some(init),
        process: Some(process),
        reset: Some(reset),
        migrate: Some(migrate),
        ..NodeVTable::default()
    }
}
