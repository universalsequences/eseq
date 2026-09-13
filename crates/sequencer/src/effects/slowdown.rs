//! Rolling-buffer varispeed insert. The read head falls behind live input at
//! `1 - speed` samples/frame, then restarts through a complementary crossfade.
//! Length/smooth changes latch at cycle boundaries; speed, tone and wet gain
//! slew continuously.
//!
//! `mode` selects how the head falls behind. Varispeed plays the history at
//! `speed`, so pitch drops with it. Stretch plays at unity pitch and instead
//! steps the head back by `slice * (1 - speed)` at every slice boundary, the
//! slice-repeat time stretch of an SP-303-class sampler: each step restarts the
//! waveform at a new phase, and through a few-millisecond raised-cosine
//! crossfade that reads as a level wobble at the slice rate. Stretch+Pitch
//! plays at the `pitch` ratio and lets the slice steps make up the difference
//! to `speed`, in either direction. In beat-sync mode the host feeds the transport's beat
//! phase every block and restarts land on the song's beat grid; while the
//! transport is stopped (or in ms mode) the cycle free-runs from tempo alone.
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
/// Params appended after the transport input (see `PARAM_MODE`); their
/// descriptor entries come last so every earlier saved index stays put.
const EXTRA_COUNT: usize = 4;
const EXTRA_MODE: usize = 0;
const EXTRA_SLICE: usize = 1;
const EXTRA_XFADE: usize = 2;
const EXTRA_PITCH: usize = 3;
const EXTRA_DEFAULTS: [f32; EXTRA_COUNT] = [MODE_VARISPEED, 55.0, 8.0, -12.0];
pub const MODE_VARISPEED: f32 = 0.0;
pub const MODE_STRETCH: f32 = 1.0;
pub const MODE_STRETCH_PITCH: f32 = 2.0;
const SLICE_MS_MIN: f32 = 20.0;
const SLICE_MS_MAX: f32 = 200.0;
const XFADE_MS_MIN: f32 = 0.5;
const XFADE_MS_MAX: f32 = 30.0;
const PITCH_MIN: f32 = -24.0;
const PITCH_MAX: f32 = 0.0;
const TAPS: usize = 32;
const PHASES: usize = 256;
const BASE_DELAY: f64 = 17.0;
// Four beats at the supported tempo floor of 20 BPM, plus the outgoing fade.
const MAX_HISTORY_SECONDS: f32 = 12.2;
const MOD_SLOTS: usize = voice_modulator::SLOT_COUNT;
const DEPTH_BASE: usize = PARAM_COUNT;
/// Hidden host input: transport beat phase in `[0, TRANSPORT_CYCLE_BEATS)`,
/// pushed once per block by the audio callback (see `sync_dj_mixer_transport_phase`).
/// It sits directly after the depth block so every saved index stays put.
pub const PARAM_TRANSPORT_BEAT_PHASE: u64 = (DEPTH_BASE + 6 * MOD_SLOTS) as u64;
/// The host wraps beat phase at eight beats (the DJ Mixer's longest division).
const TRANSPORT_CYCLE_BEATS: f64 = 8.0;
/// Memory slots of the appended params, directly after the transport input.
pub const PARAM_MODE: u64 = PARAM_TRANSPORT_BEAT_PHASE + 1;
pub const PARAM_SLICE: u64 = PARAM_MODE + 1;
pub const PARAM_XFADE: u64 = PARAM_MODE + 2;
pub const PARAM_PITCH: u64 = PARAM_MODE + 3;

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
    /// `PARAM_TRANSPORT_BEAT_PHASE`; must directly follow `mod_depths`.
    transport_phase: f32,
    /// `PARAM_MODE..=PARAM_PITCH`; must directly follow `transport_phase`.
    extra: [f32; EXTRA_COUNT],
    /// Last block-start phase the host pushed. A repeated value means the
    /// transport is stopped (or nobody drives the input), so the cycle free-runs.
    transport_seen: f32,
    transport_driven: bool,
    /// Cycle index `floor(phase / beats)` at the previous frame; -1 = unknown.
    transport_cycle: i64,
    /// Beat phase advanced per frame within the current block.
    local_phase: f64,
    sample_rate: f32,
    capacity: usize,
    write: usize,
    age: usize,
    period: usize,
    fade_length: usize,
    fade_age: usize,
    /// Raised-cosine (slice step) rather than linear (cycle restart) fade.
    fade_cosine: bool,
    /// Slice length in samples, latched with the cycle; 0 = no slicing.
    slice: usize,
    /// Slice crossfade in samples, latched with the cycle.
    slice_fade: usize,
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
    // Appended last: saved slots index params by descriptor position.
    for (slot, name, min, max, kind, scaling) in [
        (
            PARAM_MODE,
            "mode",
            0.0,
            2.0,
            ParamKind::Enum {
                labels: vec![
                    "Varispeed".to_string(),
                    "Stretch".to_string(),
                    "Stretch+Pitch".to_string(),
                ],
            },
            ParamScaling::Linear,
        ),
        (
            PARAM_SLICE,
            "slice",
            SLICE_MS_MIN,
            SLICE_MS_MAX,
            ParamKind::Continuous {
                unit: Some("ms".to_string()),
            },
            ParamScaling::Exponential,
        ),
        (
            PARAM_XFADE,
            "xfade",
            XFADE_MS_MIN,
            XFADE_MS_MAX,
            ParamKind::Continuous {
                unit: Some("ms".to_string()),
            },
            ParamScaling::Exponential,
        ),
        (
            PARAM_PITCH,
            "pitch",
            PITCH_MIN,
            PITCH_MAX,
            ParamKind::Continuous {
                unit: Some("st".to_string()),
            },
            ParamScaling::Linear,
        ),
    ] {
        params.push(ParamDescriptor {
            name: name.to_string(),
            min,
            max,
            default: EXTRA_DEFAULTS[(slot - PARAM_MODE) as usize],
            kind,
            scaling,
            node_param_idx: slot as u32,
            node_param_span: 1,
            host_control: None,
            ui_metadata: None,
        });
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
        self.fade_cosine = false;
        self.slice = if self.mode() == MODE_VARISPEED {
            0
        } else {
            let slice_ms = finite(self.extra[EXTRA_SLICE], SLICE_MS_MIN, SLICE_MS_MAX, 55.0);
            ((slice_ms * 0.001 * self.sample_rate).round() as usize).max(2)
        };
        let xfade_ms = finite(self.extra[EXTRA_XFADE], XFADE_MS_MIN, XFADE_MS_MAX, 8.0);
        self.slice_fade = ((xfade_ms * 0.001 * self.sample_rate).round() as usize)
            .clamp(1, self.slice.max(2) / 2);
    }

    fn mode(&self) -> f32 {
        finite(
            self.extra[EXTRA_MODE],
            MODE_VARISPEED,
            MODE_STRETCH_PITCH,
            MODE_VARISPEED,
        )
        .round()
    }

    /// Playback rate of the read head: pitch ratio, not the overall slowdown.
    fn rate(&self, params: &[f32; PARAM_COUNT]) -> f64 {
        match self.mode() {
            m if m == MODE_STRETCH => 1.0,
            m if m == MODE_STRETCH_PITCH => {
                let semitones = finite(self.extra[EXTRA_PITCH], PITCH_MIN, PITCH_MAX, -12.0);
                (semitones as f64 / 12.0).exp2()
            }
            _ => params[SPEED] as f64,
        }
    }

    /// One slice-repeat step: the head jumps back so that, on average over a
    /// slice, it falls behind at `1 - speed` regardless of the playback rate.
    fn slice_step(&mut self, params: &[f32; PARAM_COUNT]) {
        let step = self.slice as f64 * (self.rate(params) - params[SPEED] as f64);
        self.outgoing_delay = self.delay;
        self.delay = (self.delay + step).max(BASE_DELAY);
        self.fade_length = self.slice_fade;
        self.fade_cosine = true;
        self.fade_age = 0;
    }

    fn reset_runtime(&mut self) {
        self.write = 0;
        self.age = 0;
        self.delay = BASE_DELAY;
        self.outgoing_delay = BASE_DELAY;
        self.speed = self.rate(&self.effective_params([0.0; MOD_SLOTS]));
        // Fade in wet processing on startup/reset; dry remains sample-exact.
        self.wet = 0.0;
        self.tone = finite(self.params[TONE], 200.0, 20000.0, 20000.0) as f64;
        self.lowpass = [0.0; 2];
        self.transport_seen = self.transport_phase;
        self.transport_driven = false;
        self.transport_cycle = -1;
        self.local_phase = 0.0;
        self.latch_cycle(&self.effective_params([0.0; MOD_SLOTS]));
        self.fade_age = self.fade_length;
    }

    fn restart(&mut self, params: &[f32; PARAM_COUNT]) {
        self.outgoing_delay = self.delay;
        self.delay = BASE_DELAY;
        self.age = 0;
        self.latch_cycle(params);
        self.fade_age = 0;
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
            transport_phase: 0.0,
            extra: EXTRA_DEFAULTS,
            transport_seen: 0.0,
            transport_driven: false,
            transport_cycle: -1,
            local_phase: 0.0,
            sample_rate: sr,
            capacity: capacity(sr),
            write: 0,
            age: 0,
            period: 0,
            fade_length: 0,
            fade_age: 0,
            fade_cosine: false,
            slice: 0,
            slice_fade: 1,
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
        dst.extra = src.extra;
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
    // A fresh block-start phase means the transport advanced since the last
    // block: lock restarts to its beat grid. An unchanged one means stopped
    // (or undriven), and the cycle keeps free-running from tempo alone.
    let pushed = finite(s.transport_phase, 0.0, TRANSPORT_CYCLE_BEATS as f32, 0.0);
    if pushed.to_bits() != s.transport_seen.to_bits() {
        s.transport_seen = pushed;
        s.transport_driven = true;
        s.local_phase = pushed as f64;
    } else if s.transport_driven {
        s.transport_driven = false;
        s.transport_cycle = -1;
    }
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
        let synced = finite(params[SYNC], 0.0, 1.0, 1.0) >= 0.5;
        if s.transport_driven && synced {
            let beats = finite(params[BEATS], 0.125, 4.0, 1.0) as f64;
            let cycle = (s.local_phase / beats).floor() as i64;
            if cycle != s.transport_cycle {
                s.transport_cycle = cycle;
                s.restart(&params);
            }
            let bpm = finite(params[PARAM_BPM as usize], 20.0, 400.0, 120.0) as f64;
            s.local_phase =
                (s.local_phase + bpm / (60.0 * s.sample_rate as f64)) % TRANSPORT_CYCLE_BEATS;
        } else if s.age >= s.period {
            s.restart(&params);
        }
        if s.slice > 0 && s.age > 0 && s.age % s.slice == 0 {
            s.slice_step(&params);
        }
        let wet = if finite(params[ENABLED], 0.0, 1.0, 1.0) >= 0.5 {
            params[MIX]
        } else {
            0.0
        };
        let rate = s.rate(&params);
        s.speed = slew(s.speed, rate, coefficient);
        s.wet = slew(s.wet, wet as f64, coefficient);
        s.tone = slew(s.tone, params[TONE] as f64, coefficient);
        let mut sample = s.read(buffer, s.delay);
        if s.fade_age < s.fade_length {
            let previous = s.read(buffer, s.outgoing_delay);
            let linear = s.fade_age as f32 / s.fade_length as f32;
            let a = if s.fade_cosine {
                0.5 - 0.5 * (std::f32::consts::PI * linear).cos()
            } else {
                linear
            };
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
