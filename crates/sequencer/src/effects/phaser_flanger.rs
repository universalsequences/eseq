//! Ableton-style Phaser-Flanger: one device, three sweep engines.
//!
//! * **Phaser** — up to 12 second-order allpass sections with two circuits:
//!   Stack preserves the original per-stage notch sums and feedback character;
//!   Classic feeds back the un-mixed cascade and performs one dry/allpass sum
//!   at the output for high-Q regenerative sweeps.
//! * **Flanger** — a short Hermite-read delay (TIME, 0.1–20 ms) summed with
//!   the input; the LFO sweeps the delay in log-time so the sweep reads as a
//!   tape flange rather than a siren.
//! * **Doubler** — the same delay core at doubling lengths (2–100 ms), wet
//!   only, feedback disabled, with independent filtered-random drift stacked
//!   on the LFO so the voice never sits still or audibly repeats.
//!
//! Shared frame: one LFO (sine/tri/ramp/square, free Hz or beat-synced, the
//! right channel reading STEREO degrees later), a ±feedback path conditioned
//! by a circuit-specific DC blocker and tanh so full regeneration blooms
//! instead of blowing up, WARMTH (compensated tanh drive plus low-frequency weight on
//! the effect path only), equal-power dry/wet, and an output trim.

use crate::audiograph::NodeVTable;
use std::os::raw::{c_int, c_void};

// Power of two so reads wrap with a mask. Worst case is the doubler: 100 ms
// base pushed up to ~1.27× by LFO+drift at 384 kHz ≈ 48.9 k samples.
const DELAY_BUF_LEN: usize = 65536;
const DELAY_BUF_MASK: usize = DELAY_BUF_LEN - 1;

pub const MAX_NOTCHES: usize = 12;

// Sweep ranges in octaves at AMOUNT = 100 % (multiplicative, so the phaser
// sweeps center frequency and the delay modes sweep time in log domain).
const PHASER_SWEEP_OCT: f32 = 2.5;
const PHASER_SPREAD_SWEEP: f32 = 0.5;
const FLANGER_SWEEP_OCT: f32 = 1.0;
const DOUBLER_SWEEP_OCT: f32 = 0.35;

// Octaves between adjacent allpass centers at SPREAD = 100 %.
const SPREAD_OCT_PER_NOTCH: f32 = 1.2;
// The Stack circuit preserves the original exponential-to-linear layout.
const STACK_SPREAD_LIN_PER_NOTCH: f32 = 1.6;

const ALLPASS_Q: f32 = 0.7;
// Full feedback must get close enough to unity for the cascade's phase
// crossings to become high-Q resonances, while remaining asymptotically
// stable when the in-loop saturator is in its linear region.
const FEEDBACK_MAX: f32 = 0.995;
const STACK_FEEDBACK_MAX: f32 = 0.95;
// This is DC protection, not a user-facing bass cut. Keep it below the useful
// phaser range; a higher Safe Bass control should be modeled separately.
const FEEDBACK_DC_HZ: f32 = 5.0;
const STACK_FEEDBACK_DC_HZ: f32 = 30.0;

const PHASER_CIRCUIT_STACK: usize = 0;
const PHASER_CIRCUIT_CLASSIC: usize = 1;

// Doubler drift uses independent sample-and-hold noise per channel, then a
// slow one-pole filter. The 24-bit generator state is exactly representable
// in the effect's f32 state buffer, which keeps renders deterministic without
// type-punning audio-thread memory.
const DRIFT_UPDATE_HZ: f32 = 4.0;
const DRIFT_SMOOTH_HZ: f32 = 0.35;

// Same division table as Str8 Delay / Space Echo so the UI labels match.
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

// ── Parameter slots ──
const STATE_ENABLED: usize = 0;
const STATE_MODE: usize = 1; // 0 phaser, 1 flanger, 2 doubler
const STATE_NOTCHES: usize = 2; // 1..12
const STATE_CENTER: usize = 3; // Hz
const STATE_SPREAD: usize = 4; // 0..1
const STATE_BLEND: usize = 5; // LFO routing: 0 center .. 1 spread
const STATE_FLANGER_TIME: usize = 6; // ms
const STATE_DOUBLER_TIME: usize = 7; // ms
const STATE_SYNC: usize = 8;
const STATE_RATE: usize = 9; // Hz (free mode)
const STATE_SYNC_DIV: usize = 10;
const STATE_SHAPE: usize = 11; // 0 sine, 1 triangle, 2 ramp, 3 square
const STATE_AMOUNT: usize = 12; // 0..1
const STATE_FEEDBACK: usize = 13; // 0..1
const STATE_FB_INVERT: usize = 14;
const STATE_STEREO: usize = 15; // LFO phase offset, degrees 0..180
const STATE_WARMTH: usize = 16; // 0..1
const STATE_MIX: usize = 17; // 0..1
const STATE_OUTPUT: usize = 18; // dB
const STATE_SAMPLE_RATE: usize = 19;
const STATE_BPM: usize = 20;

// Host-modulation depth slots (4 modulator slots × 4 targets).
const STATE_MOD_AMOUNT_DEPTH_1: usize = 21;
const STATE_MOD_CENTER_DEPTH_1: usize = 25;
const STATE_MOD_FEEDBACK_DEPTH_1: usize = 29;
const STATE_MOD_MIX_DEPTH_1: usize = 33;

// ── Runtime state ──
const STATE_LFO_PHASE: usize = 37;
const STATE_DRIFT_RNG_L: usize = 38;
const STATE_DRIFT_RNG_R: usize = 39;
const STATE_DRIFT_TARGET_L: usize = 40;
const STATE_DRIFT_TARGET_R: usize = 41;
const STATE_DRIFT_L: usize = 42;
const STATE_DRIFT_R: usize = 43;
const STATE_DRIFT_COUNTDOWN: usize = 44;
const STATE_SM_CENTER_L2: usize = 45; // smoothed in log2(Hz)
const STATE_SM_AMOUNT: usize = 46;
const STATE_SM_FEEDBACK: usize = 47; // signed (invert folds in here)
const STATE_SM_MIX: usize = 48;
const STATE_SM_OUTPUT: usize = 49; // linear amp
const STATE_SM_WARMTH: usize = 50;
const STATE_SM_STEREO: usize = 51; // fraction of a cycle
const STATE_SM_RATE: usize = 52; // Hz
const STATE_SM_TIME_FL_L2: usize = 53; // log2(ms)
const STATE_SM_TIME_DB_L2: usize = 54; // log2(ms)
const STATE_SM_SPREAD: usize = 55;
const STATE_SM_BLEND: usize = 56;
const STATE_FB_L: usize = 57;
const STATE_FB_R: usize = 58;
const STATE_DC_L_X: usize = 59;
const STATE_DC_L_Y: usize = 60;
const STATE_DC_R_X: usize = 61;
const STATE_DC_R_Y: usize = 62;
const STATE_WARM_LP_L: usize = 63;
const STATE_WARM_LP_R: usize = 64;
const STATE_WRITE_POS: usize = 65;
// 12 sections × 2 z per biquad × 2 channels.
const STATE_AP_Z: usize = 66;
const AP_Z_LEN: usize = MAX_NOTCHES * 2 * 2;
const BIQUAD_COEFF_COUNT: usize = 5;
const AP_COEFF_LEN: usize = MAX_NOTCHES * 2 * BIQUAD_COEFF_COUNT;
const STATE_AP_COEFFS: usize = STATE_AP_Z + AP_Z_LEN;
const STATE_AP_COEFF_STEPS: usize = STATE_AP_COEFFS + AP_COEFF_LEN;
const STATE_AP_CONTROL_REMAINING: usize = STATE_AP_COEFF_STEPS + AP_COEFF_LEN;
const STATE_AP_ACTIVE: usize = STATE_AP_CONTROL_REMAINING + 1;
const STATE_AP_NOTCHES: usize = STATE_AP_ACTIVE + 1;

const STATE_BUF_L: usize = STATE_AP_NOTCHES + 1;
const STATE_BUF_R: usize = STATE_BUF_L + DELAY_BUF_LEN;
// Appended cells preserve every established parameter/runtime/buffer index.
// Parameter cells need not be contiguous in the node state arena.
const STATE_PHASER_CIRCUIT: usize = STATE_BUF_R + DELAY_BUF_LEN;
const STATE_AP_CIRCUIT: usize = STATE_PHASER_CIRCUIT + 1;

pub const PHASER_FLANGER_STATE_SIZE: usize = STATE_AP_CIRCUIT + 1;

pub const PHASER_FLANGER_PARAM_ENABLED: u64 = STATE_ENABLED as u64;
pub const PHASER_FLANGER_PARAM_MODE: u64 = STATE_MODE as u64;
pub const PHASER_FLANGER_PARAM_NOTCHES: u64 = STATE_NOTCHES as u64;
pub const PHASER_FLANGER_PARAM_CENTER: u64 = STATE_CENTER as u64;
pub const PHASER_FLANGER_PARAM_SPREAD: u64 = STATE_SPREAD as u64;
pub const PHASER_FLANGER_PARAM_BLEND: u64 = STATE_BLEND as u64;
pub const PHASER_FLANGER_PARAM_FLANGER_TIME: u64 = STATE_FLANGER_TIME as u64;
pub const PHASER_FLANGER_PARAM_DOUBLER_TIME: u64 = STATE_DOUBLER_TIME as u64;
pub const PHASER_FLANGER_PARAM_SYNC: u64 = STATE_SYNC as u64;
pub const PHASER_FLANGER_PARAM_RATE: u64 = STATE_RATE as u64;
pub const PHASER_FLANGER_PARAM_SYNC_DIV: u64 = STATE_SYNC_DIV as u64;
pub const PHASER_FLANGER_PARAM_SHAPE: u64 = STATE_SHAPE as u64;
pub const PHASER_FLANGER_PARAM_AMOUNT: u64 = STATE_AMOUNT as u64;
pub const PHASER_FLANGER_PARAM_FEEDBACK: u64 = STATE_FEEDBACK as u64;
pub const PHASER_FLANGER_PARAM_FB_INVERT: u64 = STATE_FB_INVERT as u64;
pub const PHASER_FLANGER_PARAM_STEREO: u64 = STATE_STEREO as u64;
pub const PHASER_FLANGER_PARAM_WARMTH: u64 = STATE_WARMTH as u64;
pub const PHASER_FLANGER_PARAM_MIX: u64 = STATE_MIX as u64;
pub const PHASER_FLANGER_PARAM_OUTPUT: u64 = STATE_OUTPUT as u64;
pub const PHASER_FLANGER_PARAM_BPM: u64 = STATE_BPM as u64;
pub const PHASER_FLANGER_PARAM_PHASER_CIRCUIT: u64 = STATE_PHASER_CIRCUIT as u64;
pub const PHASER_FLANGER_PARAM_MOD_AMOUNT_DEPTH_1: u64 = STATE_MOD_AMOUNT_DEPTH_1 as u64;
pub const PHASER_FLANGER_PARAM_MOD_CENTER_DEPTH_1: u64 = STATE_MOD_CENTER_DEPTH_1 as u64;
pub const PHASER_FLANGER_PARAM_MOD_FEEDBACK_DEPTH_1: u64 = STATE_MOD_FEEDBACK_DEPTH_1 as u64;
pub const PHASER_FLANGER_PARAM_MOD_MIX_DEPTH_1: u64 = STATE_MOD_MIX_DEPTH_1 as u64;

#[derive(Clone, Copy, Default)]
struct BiquadCoeffs {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
}

impl BiquadCoeffs {
    fn is_finite(self) -> bool {
        self.b0.is_finite()
            && self.b1.is_finite()
            && self.b2.is_finite()
            && self.a1.is_finite()
            && self.a2.is_finite()
    }

    fn step_toward(self, target: Self, samples: f32) -> Self {
        let scale = 1.0 / samples.max(1.0);
        Self {
            b0: (target.b0 - self.b0) * scale,
            b1: (target.b1 - self.b1) * scale,
            b2: (target.b2 - self.b2) * scale,
            a1: (target.a1 - self.a1) * scale,
            a2: (target.a2 - self.a2) * scale,
        }
    }

    fn advance(&mut self, step: Self) {
        self.b0 += step.b0;
        self.b1 += step.b1;
        self.b2 += step.b2;
        self.a1 += step.a1;
        self.a2 += step.a2;
    }
}

#[inline]
unsafe fn load_coeff(state: *const f32, index: usize) -> BiquadCoeffs {
    let base = state.add(index * BIQUAD_COEFF_COUNT);
    BiquadCoeffs {
        b0: *base,
        b1: *base.add(1),
        b2: *base.add(2),
        a1: *base.add(3),
        a2: *base.add(4),
    }
}

#[inline]
unsafe fn store_coeff(state: *mut f32, index: usize, coeff: BiquadCoeffs) {
    let base = state.add(index * BIQUAD_COEFF_COUNT);
    *base = coeff.b0;
    *base.add(1) = coeff.b1;
    *base.add(2) = coeff.b2;
    *base.add(3) = coeff.a1;
    *base.add(4) = coeff.a2;
}

#[inline]
fn allpass_coeffs(freq: f32, sr: f32) -> BiquadCoeffs {
    let w0 = std::f32::consts::TAU * (freq / sr.max(1.0)).clamp(1.0e-5, 0.49);
    let (sin_w0, cos_w0) = w0.sin_cos();
    let alpha = sin_w0 / (2.0 * ALLPASS_Q);
    let a0 = 1.0 + alpha;
    BiquadCoeffs {
        b0: (1.0 - alpha) / a0,
        b1: -2.0 * cos_w0 / a0,
        b2: 1.0,
        a1: -2.0 * cos_w0 / a0,
        a2: (1.0 - alpha) / a0,
    }
}

#[inline]
unsafe fn biquad_sample(input: f32, c: BiquadCoeffs, z: *mut f32) -> f32 {
    let out = c.b0 * input + *z;
    *z = c.b1 * input - c.a1 * out + *z.add(1);
    *z.add(1) = c.b2 * input - c.a2 * out;
    out
}

#[inline]
fn one_pole_coef(freq: f32, sr: f32) -> f32 {
    1.0 - (-std::f32::consts::TAU * freq / sr.max(1.0)).exp()
}

#[inline]
fn db_to_amp(db: f32) -> f32 {
    (10.0_f32).powf(db / 20.0)
}

#[inline]
fn finite_or(value: f32, fallback: f32) -> f32 {
    if value.is_finite() {
        value
    } else {
        fallback
    }
}

#[inline]
fn finite_clamp(value: f32, min: f32, max: f32, fallback: f32) -> f32 {
    finite_or(value, fallback).clamp(min, max)
}

/// Advance a deterministic 24-bit LCG and return bipolar noise. Every state
/// value remains exactly representable in f32, including across host saves.
#[inline]
fn next_drift_noise(state: &mut f32) -> f32 {
    const MASK: u32 = (1 << 24) - 1;
    let seed = (*state as u32) & MASK;
    let next = seed.wrapping_mul(1_140_671_485).wrapping_add(12_820_163) & MASK;
    *state = next as f32;
    next as f32 * (2.0 / MASK as f32) - 1.0
}

// Raw LFO shapes, -1..1 over phase 0..1.
#[inline]
fn lfo_raw(shape: usize, phase: f32) -> f32 {
    match shape {
        1 => {
            if phase < 0.5 {
                4.0 * phase - 1.0
            } else {
                3.0 - 4.0 * phase
            }
        }
        2 => 2.0 * phase - 1.0,
        3 => {
            if phase < 0.5 {
                1.0
            } else {
                -1.0
            }
        }
        _ => (std::f32::consts::TAU * phase).sin(),
    }
}

// 4-point Hermite read of a modulated line (same shape as dimension.rs).
#[inline]
unsafe fn line_read(buf: *const f32, wpos: usize, delay: f32) -> f32 {
    let d = delay.clamp(2.0, (DELAY_BUF_LEN - 4) as f32);
    let read = wpos as f32 - d + DELAY_BUF_LEN as f32;
    let base = read.floor();
    let frac = read - base;
    let i0 = (base as usize + DELAY_BUF_LEN - 1) & DELAY_BUF_MASK;
    let xm1 = *buf.add(i0);
    let x0 = *buf.add((i0 + 1) & DELAY_BUF_MASK);
    let x1 = *buf.add((i0 + 2) & DELAY_BUF_MASK);
    let x2 = *buf.add((i0 + 3) & DELAY_BUF_MASK);
    let c1 = 0.5 * (x1 - xm1);
    let c2 = xm1 - 2.5 * x0 + 2.0 * x1 - 0.5 * x2;
    let c3 = 0.5 * (x2 - xm1) + 1.5 * (x0 - x1);
    ((c3 * frac + c2) * frac + c1) * frac + x0
}

/// Allpass center-frequency layout for the phaser, before modulation. The
/// resulting global dry/allpass sum has one notch near each center. This math
/// is dual-maintained with the `phaser-notch` UI widget.
pub fn notch_frequencies(notches: usize, center: f32, spread: f32, sr: f32) -> Vec<f32> {
    let n = notches.clamp(1, MAX_NOTCHES);
    let center = super::nyquist_clamp(center, sr, 20.0, 0.45);
    let spread = spread.clamp(0.0, 1.0);
    let mid = (n as f32 - 1.0) * 0.5;
    (0..n)
        .map(|k| {
            let off = k as f32 - mid;
            let f = center * (spread * off * SPREAD_OCT_PER_NOTCH).exp2();
            super::nyquist_clamp(f, sr, 20.0, 0.45)
        })
        .collect()
}

/// Original Stack-circuit layout. Unlike Classic BLEND, Stack BLEND is a
/// static morph from octave spacing to linear-Hz spacing.
fn stack_notch_frequencies(
    notches: usize,
    center: f32,
    spread: f32,
    blend: f32,
    sr: f32,
) -> Vec<f32> {
    let n = notches.clamp(1, MAX_NOTCHES);
    let center = super::nyquist_clamp(center, sr, 20.0, 0.45);
    let spread = spread.clamp(0.0, 1.0);
    let blend = blend.clamp(0.0, 1.0);
    let mid = (n as f32 - 1.0) * 0.5;
    (0..n)
        .map(|k| {
            let off = k as f32 - mid;
            let exp_f = center * (spread * off * SPREAD_OCT_PER_NOTCH).exp2();
            let lin_f = center * (1.0 + spread * off * STACK_SPREAD_LIN_PER_NOTCH).max(0.1);
            let f = exp_f.powf(1.0 - blend) * lin_f.powf(blend);
            super::nyquist_clamp(f, sr, 20.0, 0.45)
        })
        .collect()
}

/// Apply Phaser BLEND semantics to the two parameters the LFO can move.
/// Keeping this as an explicit mapping makes CENTER-only and SPREAD-only
/// modulation independently testable.
#[inline]
fn modulated_phaser_layout(
    center_l2: f32,
    spread: f32,
    blend: f32,
    amount: f32,
    lfo: f32,
) -> (f32, f32) {
    let blend = blend.clamp(0.0, 1.0);
    let center = (center_l2 + amount * lfo * PHASER_SWEEP_OCT * (1.0 - blend)).exp2();
    let spread = (spread + amount * lfo * PHASER_SPREAD_SWEEP * blend).clamp(0.0, 1.0);
    (center, spread)
}

unsafe extern "C" fn phaser_flanger_init(
    state: *mut c_void,
    sample_rate: c_int,
    _max_block: c_int,
    _initial_state: *const c_void,
) {
    let s = state as *mut f32;
    std::ptr::write_bytes(s, 0, PHASER_FLANGER_STATE_SIZE);
    *s.add(STATE_ENABLED) = 1.0;
    *s.add(STATE_MODE) = 0.0;
    *s.add(STATE_NOTCHES) = 4.0;
    *s.add(STATE_CENTER) = 400.0;
    *s.add(STATE_SPREAD) = 0.35;
    *s.add(STATE_BLEND) = 0.0;
    *s.add(STATE_FLANGER_TIME) = 2.5;
    *s.add(STATE_DOUBLER_TIME) = 80.0;
    *s.add(STATE_SYNC) = 0.0;
    *s.add(STATE_RATE) = 0.15;
    *s.add(STATE_SYNC_DIV) = 6.0; // "1/4"
    *s.add(STATE_SHAPE) = 0.0;
    *s.add(STATE_AMOUNT) = 0.25;
    *s.add(STATE_FEEDBACK) = 0.0;
    *s.add(STATE_FB_INVERT) = 0.0;
    *s.add(STATE_STEREO) = 20.0;
    *s.add(STATE_WARMTH) = 0.0;
    *s.add(STATE_MIX) = 0.5;
    *s.add(STATE_OUTPUT) = 0.0;
    *s.add(STATE_SAMPLE_RATE) = sample_rate as f32;
    *s.add(STATE_BPM) = 120.0;
    *s.add(STATE_PHASER_CIRCUIT) = PHASER_CIRCUIT_CLASSIC as f32;
    *s.add(STATE_DRIFT_RNG_L) = 0x51_7c_c1 as f32;
    *s.add(STATE_DRIFT_RNG_R) = 0xa3_42_19 as f32;
    *s.add(STATE_SM_CENTER_L2) = 400.0_f32.log2();
    *s.add(STATE_SM_AMOUNT) = 0.25;
    *s.add(STATE_SM_MIX) = 0.5;
    *s.add(STATE_SM_OUTPUT) = 1.0;
    *s.add(STATE_SM_STEREO) = 20.0 / 360.0;
    *s.add(STATE_SM_RATE) = 0.15;
    *s.add(STATE_SM_TIME_FL_L2) = 2.5_f32.log2();
    *s.add(STATE_SM_TIME_DB_L2) = 80.0_f32.log2();
    *s.add(STATE_SM_SPREAD) = 0.35;
    *s.add(STATE_AP_NOTCHES) = 4.0;
    *s.add(STATE_AP_CIRCUIT) = PHASER_CIRCUIT_CLASSIC as f32;
}

unsafe extern "C" fn phaser_flanger_process(
    inp: *const *mut f32,
    out: *const *mut f32,
    nframes: c_int,
    state: *mut c_void,
    _buffers: *mut c_void,
) {
    let s = state as *mut f32;
    let nf = nframes as usize;
    let in0 = *inp.add(0);
    let in1 = *inp.add(1);
    let mod_inputs = [*inp.add(2), *inp.add(3), *inp.add(4), *inp.add(5)];
    let out0 = *out.add(0);
    let out1 = *out.add(1);

    if *s.add(STATE_ENABLED) <= 0.5 {
        std::ptr::copy_nonoverlapping(in0 as *const f32, out0, nf);
        std::ptr::copy_nonoverlapping(in1 as *const f32, out1, nf);
        return;
    }

    let sr = finite_clamp(*s.add(STATE_SAMPLE_RATE), 1_000.0, 384_000.0, 48_000.0);
    let mode = finite_clamp(*s.add(STATE_MODE), 0.0, 2.0, 0.0).round() as usize;
    let phaser_circuit = finite_clamp(
        *s.add(STATE_PHASER_CIRCUIT),
        PHASER_CIRCUIT_STACK as f32,
        PHASER_CIRCUIT_CLASSIC as f32,
        PHASER_CIRCUIT_CLASSIC as f32,
    )
    .round() as usize;
    let notches =
        finite_clamp(*s.add(STATE_NOTCHES), 1.0, MAX_NOTCHES as f32, 4.0).round() as usize;
    let shape = finite_clamp(*s.add(STATE_SHAPE), 0.0, 3.0, 0.0).round() as usize;

    // Rate: free Hz, or one LFO cycle per synced division.
    let rate_target = if *s.add(STATE_SYNC) > 0.5 {
        let idx = (finite_clamp(*s.add(STATE_SYNC_DIV), 0.0, 10.0, 6.0).round() as usize)
            .min(SYNC_BEATS.len() - 1);
        let bpm = finite_clamp(*s.add(STATE_BPM), 20.0, 999.0, 120.0);
        bpm / (60.0 * SYNC_BEATS[idx])
    } else {
        finite_clamp(*s.add(STATE_RATE), 0.01, 20.0, 0.15)
    };

    let center_l2_target = finite_clamp(*s.add(STATE_CENTER), 20.0, sr * 0.45, 400.0).log2();
    let spread_target = finite_clamp(*s.add(STATE_SPREAD), 0.0, 1.0, 0.35);
    let blend_target = finite_clamp(*s.add(STATE_BLEND), 0.0, 1.0, 0.0);
    let time_fl_l2_target = finite_clamp(*s.add(STATE_FLANGER_TIME), 0.1, 20.0, 2.5).log2();
    let time_db_l2_target = finite_clamp(*s.add(STATE_DOUBLER_TIME), 2.0, 100.0, 80.0).log2();
    let amount_knob = finite_clamp(*s.add(STATE_AMOUNT), 0.0, 1.0, 0.25);
    let fb_sign = if *s.add(STATE_FB_INVERT) > 0.5 {
        -1.0
    } else {
        1.0
    };
    let feedback_knob = finite_clamp(*s.add(STATE_FEEDBACK), 0.0, 1.0, 0.0);
    let feedback_max = if mode == 0 && phaser_circuit == PHASER_CIRCUIT_STACK {
        STACK_FEEDBACK_MAX
    } else {
        FEEDBACK_MAX
    };
    let stereo_target = finite_clamp(*s.add(STATE_STEREO), 0.0, 180.0, 20.0) / 360.0;
    let warmth_target = finite_clamp(*s.add(STATE_WARMTH), 0.0, 1.0, 0.0);
    let mix_knob = finite_clamp(*s.add(STATE_MIX), 0.0, 1.0, 0.5);
    let output_target = db_to_amp(finite_clamp(*s.add(STATE_OUTPUT), -12.0, 12.0, 0.0));

    let knob_coef = one_pole_coef(20.0, sr);
    let warm_lp_coef = one_pole_coef(200.0, sr);
    let drift_coef = one_pole_coef(DRIFT_SMOOTH_HZ, sr);
    let dc_r = if mode == 0 && phaser_circuit == PHASER_CIRCUIT_STACK {
        // Preserve the original Stack circuit exactly, including its
        // approximate 30 Hz pole placement.
        1.0 - (std::f32::consts::TAU * STACK_FEEDBACK_DC_HZ / sr).min(0.5)
    } else {
        // The Classic circuit uses an accurately placed sub-audio DC blocker.
        (-std::f32::consts::TAU * FEEDBACK_DC_HZ / sr).exp()
    };

    let amount_mod_amt = [
        finite_clamp(*s.add(STATE_MOD_AMOUNT_DEPTH_1), -1.0, 1.0, 0.0),
        finite_clamp(*s.add(STATE_MOD_AMOUNT_DEPTH_1 + 1), -1.0, 1.0, 0.0),
        finite_clamp(*s.add(STATE_MOD_AMOUNT_DEPTH_1 + 2), -1.0, 1.0, 0.0),
        finite_clamp(*s.add(STATE_MOD_AMOUNT_DEPTH_1 + 3), -1.0, 1.0, 0.0),
    ];
    let center_mod_amt = [
        finite_clamp(*s.add(STATE_MOD_CENTER_DEPTH_1), -4.0, 4.0, 0.0),
        finite_clamp(*s.add(STATE_MOD_CENTER_DEPTH_1 + 1), -4.0, 4.0, 0.0),
        finite_clamp(*s.add(STATE_MOD_CENTER_DEPTH_1 + 2), -4.0, 4.0, 0.0),
        finite_clamp(*s.add(STATE_MOD_CENTER_DEPTH_1 + 3), -4.0, 4.0, 0.0),
    ];
    let feedback_mod_amt = [
        finite_clamp(*s.add(STATE_MOD_FEEDBACK_DEPTH_1), -1.0, 1.0, 0.0),
        finite_clamp(*s.add(STATE_MOD_FEEDBACK_DEPTH_1 + 1), -1.0, 1.0, 0.0),
        finite_clamp(*s.add(STATE_MOD_FEEDBACK_DEPTH_1 + 2), -1.0, 1.0, 0.0),
        finite_clamp(*s.add(STATE_MOD_FEEDBACK_DEPTH_1 + 3), -1.0, 1.0, 0.0),
    ];
    let mix_mod_amt = [
        finite_clamp(*s.add(STATE_MOD_MIX_DEPTH_1), -1.0, 1.0, 0.0),
        finite_clamp(*s.add(STATE_MOD_MIX_DEPTH_1 + 1), -1.0, 1.0, 0.0),
        finite_clamp(*s.add(STATE_MOD_MIX_DEPTH_1 + 2), -1.0, 1.0, 0.0),
        finite_clamp(*s.add(STATE_MOD_MIX_DEPTH_1 + 3), -1.0, 1.0, 0.0),
    ];

    let buf_l = s.add(STATE_BUF_L);
    let buf_r = s.add(STATE_BUF_R);
    let ap_z = s.add(STATE_AP_Z);

    let mut phase = finite_clamp(*s.add(STATE_LFO_PHASE), 0.0, 1.0, 0.0);
    let mut drift_rng_l = finite_or(*s.add(STATE_DRIFT_RNG_L), 0x51_7c_c1 as f32);
    let mut drift_rng_r = finite_or(*s.add(STATE_DRIFT_RNG_R), 0xa3_42_19 as f32);
    let mut drift_target_l = finite_clamp(*s.add(STATE_DRIFT_TARGET_L), -1.0, 1.0, 0.0);
    let mut drift_target_r = finite_clamp(*s.add(STATE_DRIFT_TARGET_R), -1.0, 1.0, 0.0);
    let mut drift_l = finite_clamp(*s.add(STATE_DRIFT_L), -1.0, 1.0, 0.0);
    let mut drift_r = finite_clamp(*s.add(STATE_DRIFT_R), -1.0, 1.0, 0.0);
    let mut drift_countdown = finite_or(*s.add(STATE_DRIFT_COUNTDOWN), 0.0);
    let mut sm_center_l2 = finite_clamp(
        *s.add(STATE_SM_CENTER_L2),
        20.0_f32.log2(),
        (sr * 0.45).log2(),
        center_l2_target,
    );
    let mut sm_amount = finite_clamp(*s.add(STATE_SM_AMOUNT), 0.0, 1.0, amount_knob);
    let mut sm_feedback = finite_clamp(*s.add(STATE_SM_FEEDBACK), -feedback_max, feedback_max, 0.0);
    let mut sm_mix = finite_clamp(*s.add(STATE_SM_MIX), 0.0, 1.0, mix_knob);
    let mut sm_output = finite_clamp(*s.add(STATE_SM_OUTPUT), 0.25, 4.0, output_target);
    let mut sm_warmth = finite_clamp(*s.add(STATE_SM_WARMTH), 0.0, 1.0, warmth_target);
    let mut sm_stereo = finite_clamp(*s.add(STATE_SM_STEREO), 0.0, 0.5, stereo_target);
    let mut sm_rate = finite_clamp(*s.add(STATE_SM_RATE), 0.01, 160.0, rate_target);
    let mut sm_time_fl_l2 = finite_clamp(
        *s.add(STATE_SM_TIME_FL_L2),
        0.1_f32.log2(),
        20.0_f32.log2(),
        time_fl_l2_target,
    );
    let mut sm_time_db_l2 = finite_clamp(
        *s.add(STATE_SM_TIME_DB_L2),
        2.0_f32.log2(),
        100.0_f32.log2(),
        time_db_l2_target,
    );
    let mut sm_spread = finite_clamp(*s.add(STATE_SM_SPREAD), 0.0, 1.0, spread_target);
    let mut sm_blend = finite_clamp(*s.add(STATE_SM_BLEND), 0.0, 1.0, blend_target);
    let mut fb_l = finite_or(*s.add(STATE_FB_L), 0.0);
    let mut fb_r = finite_or(*s.add(STATE_FB_R), 0.0);
    let mut dc_l_x = finite_or(*s.add(STATE_DC_L_X), 0.0);
    let mut dc_l_y = finite_or(*s.add(STATE_DC_L_Y), 0.0);
    let mut dc_r_x = finite_or(*s.add(STATE_DC_R_X), 0.0);
    let mut dc_r_y = finite_or(*s.add(STATE_DC_R_Y), 0.0);
    let mut warm_lp_l = finite_or(*s.add(STATE_WARM_LP_L), 0.0);
    let mut warm_lp_r = finite_or(*s.add(STATE_WARM_LP_R), 0.0);
    let mut wpos = (*s.add(STATE_WRITE_POS)) as usize & DELAY_BUF_MASK;

    // Phaser allpass targets are refreshed every 32 samples and linearly
    // interpolated between refreshes. Coefficients and ramp progress live in
    // node state so arbitrary host block sizes cannot reintroduce zippering.
    let mut coeffs_l: [BiquadCoeffs; MAX_NOTCHES] =
        std::array::from_fn(|k| load_coeff(s.add(STATE_AP_COEFFS), k * 2));
    let mut coeffs_r: [BiquadCoeffs; MAX_NOTCHES] =
        std::array::from_fn(|k| load_coeff(s.add(STATE_AP_COEFFS), k * 2 + 1));
    let mut coeff_steps_l: [BiquadCoeffs; MAX_NOTCHES] =
        std::array::from_fn(|k| load_coeff(s.add(STATE_AP_COEFF_STEPS), k * 2));
    let mut coeff_steps_r: [BiquadCoeffs; MAX_NOTCHES] =
        std::array::from_fn(|k| load_coeff(s.add(STATE_AP_COEFF_STEPS), k * 2 + 1));
    let mut ap_control_remaining = *s.add(STATE_AP_CONTROL_REMAINING) as usize;
    let mut ap_active = *s.add(STATE_AP_ACTIVE) > 0.5;
    let mut ap_notches = (*s.add(STATE_AP_NOTCHES)).round() as usize;
    let mut ap_circuit = finite_clamp(
        *s.add(STATE_AP_CIRCUIT),
        PHASER_CIRCUIT_STACK as f32,
        PHASER_CIRCUIT_CLASSIC as f32,
        PHASER_CIRCUIT_CLASSIC as f32,
    )
    .round() as usize;
    if coeffs_l
        .iter()
        .chain(&coeffs_r)
        .any(|coeff| !coeff.is_finite())
        || coeff_steps_l
            .iter()
            .chain(&coeff_steps_r)
            .any(|coeff| !coeff.is_finite())
    {
        ap_active = false;
        ap_control_remaining = 0;
    }

    for i in 0..nf {
        let dry_l = *in0.add(i);
        let dry_r = *in1.add(i);

        let mut amount_mod = 0.0_f32;
        let mut center_mod = 0.0_f32;
        let mut feedback_mod = 0.0_f32;
        let mut mix_mod = 0.0_f32;
        for slot in 0..4 {
            let m = finite_or(*mod_inputs[slot].add(i), 0.0);
            amount_mod += m * amount_mod_amt[slot];
            center_mod += m * center_mod_amt[slot];
            feedback_mod += m * feedback_mod_amt[slot];
            mix_mod += m * mix_mod_amt[slot];
        }

        sm_center_l2 +=
            knob_coef * ((center_l2_target + center_mod).clamp(4.32, 14.5) - sm_center_l2);
        sm_amount += knob_coef * ((amount_knob + amount_mod).clamp(0.0, 1.0) - sm_amount);
        sm_feedback += knob_coef
            * ((feedback_knob + feedback_mod).clamp(0.0, 1.0) * feedback_max * fb_sign
                - sm_feedback);
        sm_mix += knob_coef * ((mix_knob + mix_mod).clamp(0.0, 1.0) - sm_mix);
        sm_output += knob_coef * (output_target - sm_output);
        sm_warmth += knob_coef * (warmth_target - sm_warmth);
        sm_stereo += knob_coef * (stereo_target - sm_stereo);
        sm_rate += knob_coef * (rate_target - sm_rate);
        sm_time_fl_l2 += knob_coef * (time_fl_l2_target - sm_time_fl_l2);
        sm_time_db_l2 += knob_coef * (time_db_l2_target - sm_time_db_l2);
        sm_spread += knob_coef * (spread_target - sm_spread);
        sm_blend += knob_coef * (blend_target - sm_blend);

        phase += sm_rate / sr;
        if phase >= 1.0 {
            phase -= 1.0;
        }
        let mut phase_r = phase + sm_stereo;
        if phase_r >= 1.0 {
            phase_r -= 1.0;
        }
        let lfo_l = lfo_raw(shape, phase);
        let lfo_r = lfo_raw(shape, phase_r);

        if drift_countdown <= 0.0 {
            drift_target_l = next_drift_noise(&mut drift_rng_l);
            drift_target_r = next_drift_noise(&mut drift_rng_r);
            drift_countdown += sr / DRIFT_UPDATE_HZ;
        }
        drift_countdown -= 1.0;
        drift_l += drift_coef * (drift_target_l - drift_l);
        drift_r += drift_coef * (drift_target_r - drift_r);

        // Warmth: compensated tanh drive plus a touch of low-frequency
        // weight, on the effect path only — the dry mix leg stays clean.
        let drive = 1.0 + 3.0 * sm_warmth;
        let normalization = 1.0 / drive.tanh().max(1.0e-6);
        let saturated_l = (dry_l * drive).tanh() * normalization;
        let saturated_r = (dry_r * drive).tanh() * normalization;
        // WARMTH=0 must be a true identity pre-stage; blending into the
        // normalized driven signal avoids the always-on tanh that would
        // otherwise color the effect even when the control is off.
        let mut xw_l = dry_l + sm_warmth * (saturated_l - dry_l);
        let mut xw_r = dry_r + sm_warmth * (saturated_r - dry_r);
        warm_lp_l += warm_lp_coef * (xw_l - warm_lp_l);
        warm_lp_r += warm_lp_coef * (xw_r - warm_lp_r);
        xw_l += sm_warmth * 0.5 * warm_lp_l;
        xw_r += sm_warmth * 0.5 * warm_lp_r;

        let (wet_l, wet_r, fb_src_l, fb_src_r) = match mode {
            0 => {
                let structural_change = !ap_active || ap_notches != notches;
                let circuit_change = ap_circuit != phaser_circuit;
                if structural_change || circuit_change || ap_control_remaining == 0 {
                    let (freqs_l, freqs_r) = if phaser_circuit == PHASER_CIRCUIT_STACK {
                        let center = sm_center_l2.exp2();
                        let center_l = center * (sm_amount * lfo_l * PHASER_SWEEP_OCT).exp2();
                        let center_r = center * (sm_amount * lfo_r * PHASER_SWEEP_OCT).exp2();
                        (
                            stack_notch_frequencies(notches, center_l, sm_spread, sm_blend, sr),
                            stack_notch_frequencies(notches, center_r, sm_spread, sm_blend, sr),
                        )
                    } else {
                        let (center_l, spread_l) = modulated_phaser_layout(
                            sm_center_l2,
                            sm_spread,
                            sm_blend,
                            sm_amount,
                            lfo_l,
                        );
                        let (center_r, spread_r) = modulated_phaser_layout(
                            sm_center_l2,
                            sm_spread,
                            sm_blend,
                            sm_amount,
                            lfo_r,
                        );
                        (
                            notch_frequencies(notches, center_l, spread_l, sr),
                            notch_frequencies(notches, center_r, spread_r, sr),
                        )
                    };
                    for k in 0..notches {
                        let target_l = allpass_coeffs(freqs_l[k], sr);
                        let target_r = allpass_coeffs(freqs_r[k], sr);
                        if structural_change {
                            coeffs_l[k] = target_l;
                            coeffs_r[k] = target_r;
                            coeff_steps_l[k] = BiquadCoeffs::default();
                            coeff_steps_r[k] = BiquadCoeffs::default();
                        } else {
                            coeff_steps_l[k] = coeffs_l[k].step_toward(target_l, 32.0);
                            coeff_steps_r[k] = coeffs_r[k].step_toward(target_r, 32.0);
                        }
                    }
                    if structural_change {
                        // A changed cascade topology cannot safely reuse the
                        // old section histories. This is a deliberate reset
                        // at an explicit mode/notch-count edit, not a hidden
                        // audio-block boundary discontinuity.
                        std::ptr::write_bytes(ap_z, 0, AP_Z_LEN);
                    }
                    ap_active = true;
                    ap_notches = notches;
                    ap_circuit = phaser_circuit;
                    ap_control_remaining = 32;
                }
                if phaser_circuit == PHASER_CIRCUIT_STACK {
                    // Original circuit: every stage becomes a notch before
                    // feeding the next, and the already-notched result closes
                    // the feedback loop.
                    let mut y_l = xw_l + sm_feedback * fb_l;
                    let mut y_r = xw_r + sm_feedback * fb_r;
                    for k in 0..notches {
                        y_l = 0.5 * (y_l + biquad_sample(y_l, coeffs_l[k], ap_z.add(k * 4)));
                        y_r = 0.5 * (y_r + biquad_sample(y_r, coeffs_r[k], ap_z.add(k * 4 + 2)));
                        coeffs_l[k].advance(coeff_steps_l[k]);
                        coeffs_r[k].advance(coeff_steps_r[k]);
                    }
                    ap_control_remaining = ap_control_remaining.saturating_sub(1);
                    (y_l, y_r, y_l, y_r)
                } else {
                    // Classic circuit: the unity-magnitude allpass cascade is
                    // the feedback signal, with a single dry/allpass sum at
                    // the output so every phase crossing can regenerate.
                    let mut ap_l = xw_l + sm_feedback * fb_l;
                    let mut ap_r = xw_r + sm_feedback * fb_r;
                    for k in 0..notches {
                        ap_l = biquad_sample(ap_l, coeffs_l[k], ap_z.add(k * 4));
                        ap_r = biquad_sample(ap_r, coeffs_r[k], ap_z.add(k * 4 + 2));
                        coeffs_l[k].advance(coeff_steps_l[k]);
                        coeffs_r[k].advance(coeff_steps_r[k]);
                    }
                    ap_control_remaining = ap_control_remaining.saturating_sub(1);
                    (0.5 * (xw_l + ap_l), 0.5 * (xw_r + ap_r), ap_l, ap_r)
                }
            }
            1 => {
                ap_active = false;
                ap_control_remaining = 0;
                let base_l = sm_time_fl_l2 + sm_amount * lfo_l * FLANGER_SWEEP_OCT;
                let base_r = sm_time_fl_l2 + sm_amount * lfo_r * FLANGER_SWEEP_OCT;
                let d_l = base_l.exp2() * 0.001 * sr;
                let d_r = base_r.exp2() * 0.001 * sr;
                let rd_l = line_read(buf_l, wpos, d_l);
                let rd_r = line_read(buf_r, wpos, d_r);
                *buf_l.add(wpos) = xw_l + sm_feedback * fb_l;
                *buf_r.add(wpos) = xw_r + sm_feedback * fb_r;
                (0.5 * (xw_l + rd_l), 0.5 * (xw_r + rd_r), rd_l, rd_r)
            }
            _ => {
                ap_active = false;
                ap_control_remaining = 0;
                let base_l =
                    sm_time_db_l2 + sm_amount * (lfo_l * 0.7 + drift_l * 0.3) * DOUBLER_SWEEP_OCT;
                let base_r =
                    sm_time_db_l2 + sm_amount * (lfo_r * 0.7 + drift_r * 0.3) * DOUBLER_SWEEP_OCT;
                let d_l = base_l.exp2() * 0.001 * sr;
                let d_r = base_r.exp2() * 0.001 * sr;
                *buf_l.add(wpos) = xw_l;
                *buf_r.add(wpos) = xw_r;
                let rd_l = line_read(buf_l, wpos, d_l);
                let rd_r = line_read(buf_r, wpos, d_r);
                (rd_l, rd_r, 0.0, 0.0)
            }
        };

        // Condition the feedback source: sub-audio DC block, then tanh so full
        // regeneration saturates instead of running away.
        dc_l_y = fb_src_l - dc_l_x + dc_r * dc_l_y;
        dc_l_x = fb_src_l;
        dc_r_y = fb_src_r - dc_r_x + dc_r * dc_r_y;
        dc_r_x = fb_src_r;
        fb_l = dc_l_y.tanh();
        fb_r = dc_r_y.tanh();

        let g_wet = (sm_mix * std::f32::consts::FRAC_PI_2).sin();
        let g_dry = (sm_mix * std::f32::consts::FRAC_PI_2).cos();
        *out0.add(i) = (dry_l * g_dry + wet_l * g_wet) * sm_output;
        *out1.add(i) = (dry_r * g_dry + wet_r * g_wet) * sm_output;

        wpos = (wpos + 1) & DELAY_BUF_MASK;
    }

    *s.add(STATE_LFO_PHASE) = phase;
    *s.add(STATE_DRIFT_RNG_L) = drift_rng_l;
    *s.add(STATE_DRIFT_RNG_R) = drift_rng_r;
    *s.add(STATE_DRIFT_TARGET_L) = drift_target_l;
    *s.add(STATE_DRIFT_TARGET_R) = drift_target_r;
    *s.add(STATE_DRIFT_L) = drift_l;
    *s.add(STATE_DRIFT_R) = drift_r;
    *s.add(STATE_DRIFT_COUNTDOWN) = drift_countdown;
    *s.add(STATE_SM_CENTER_L2) = sm_center_l2;
    *s.add(STATE_SM_AMOUNT) = sm_amount;
    *s.add(STATE_SM_FEEDBACK) = sm_feedback;
    *s.add(STATE_SM_MIX) = sm_mix;
    *s.add(STATE_SM_OUTPUT) = sm_output;
    *s.add(STATE_SM_WARMTH) = sm_warmth;
    *s.add(STATE_SM_STEREO) = sm_stereo;
    *s.add(STATE_SM_RATE) = sm_rate;
    *s.add(STATE_SM_TIME_FL_L2) = sm_time_fl_l2;
    *s.add(STATE_SM_TIME_DB_L2) = sm_time_db_l2;
    *s.add(STATE_SM_SPREAD) = sm_spread;
    *s.add(STATE_SM_BLEND) = sm_blend;
    *s.add(STATE_FB_L) = fb_l;
    *s.add(STATE_FB_R) = fb_r;
    *s.add(STATE_DC_L_X) = dc_l_x;
    *s.add(STATE_DC_L_Y) = dc_l_y;
    *s.add(STATE_DC_R_X) = dc_r_x;
    *s.add(STATE_DC_R_Y) = dc_r_y;
    *s.add(STATE_WARM_LP_L) = warm_lp_l;
    *s.add(STATE_WARM_LP_R) = warm_lp_r;
    *s.add(STATE_WRITE_POS) = wpos as f32;
    *s.add(STATE_AP_CONTROL_REMAINING) = ap_control_remaining as f32;
    *s.add(STATE_AP_ACTIVE) = ap_active as u8 as f32;
    *s.add(STATE_AP_NOTCHES) = ap_notches as f32;
    *s.add(STATE_AP_CIRCUIT) = ap_circuit as f32;
    for k in 0..MAX_NOTCHES {
        store_coeff(s.add(STATE_AP_COEFFS), k * 2, coeffs_l[k]);
        store_coeff(s.add(STATE_AP_COEFFS), k * 2 + 1, coeffs_r[k]);
        store_coeff(s.add(STATE_AP_COEFF_STEPS), k * 2, coeff_steps_l[k]);
        store_coeff(s.add(STATE_AP_COEFF_STEPS), k * 2 + 1, coeff_steps_r[k]);
    }
}

pub fn phaser_flanger_vtable() -> NodeVTable {
    NodeVTable {
        process: Some(phaser_flanger_process),
        init: Some(phaser_flanger_init),
        reset: None,
        migrate: None,
        ..NodeVTable::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: usize = 48000;
    const BLOCK: usize = 128;

    struct Render {
        in_l: Vec<f32>,
        out_l: Vec<f32>,
    }

    fn render_with_mods(
        params: &[(usize, f32)],
        mods: [f32; 4],
        input: impl Fn(usize) -> (f32, f32),
        n: usize,
    ) -> Render {
        let mut state = vec![0.0_f32; PHASER_FLANGER_STATE_SIZE];
        unsafe {
            phaser_flanger_init(
                state.as_mut_ptr() as *mut c_void,
                SR as c_int,
                BLOCK as c_int,
                std::ptr::null(),
            );
        }
        for &(slot, value) in params {
            state[slot] = value;
        }
        // Pre-seed smoothers at their targets so short renders measure the
        // steady state rather than the 20 Hz parameter glide.
        state[STATE_SM_CENTER_L2] = state[STATE_CENTER].clamp(20.0, SR as f32 * 0.45).log2();
        state[STATE_SM_AMOUNT] = state[STATE_AMOUNT];
        let feedback_max = if state[STATE_MODE].round() as usize == 0
            && state[STATE_PHASER_CIRCUIT].round() as usize == PHASER_CIRCUIT_STACK
        {
            STACK_FEEDBACK_MAX
        } else {
            FEEDBACK_MAX
        };
        state[STATE_SM_FEEDBACK] = state[STATE_FEEDBACK]
            * feedback_max
            * if state[STATE_FB_INVERT] > 0.5 {
                -1.0
            } else {
                1.0
            };
        state[STATE_SM_MIX] = state[STATE_MIX];
        state[STATE_SM_OUTPUT] = db_to_amp(state[STATE_OUTPUT]);
        state[STATE_SM_WARMTH] = state[STATE_WARMTH];
        state[STATE_SM_STEREO] = state[STATE_STEREO] / 360.0;
        state[STATE_SM_TIME_FL_L2] = state[STATE_FLANGER_TIME].clamp(0.1, 20.0).log2();
        state[STATE_SM_TIME_DB_L2] = state[STATE_DOUBLER_TIME].clamp(2.0, 100.0).log2();
        state[STATE_SM_SPREAD] = state[STATE_SPREAD];
        state[STATE_SM_BLEND] = state[STATE_BLEND];
        state[STATE_SM_RATE] = if state[STATE_SYNC] > 0.5 {
            let idx = (state[STATE_SYNC_DIV].round() as usize).min(SYNC_BEATS.len() - 1);
            state[STATE_BPM].max(20.0) / (60.0 * SYNC_BEATS[idx])
        } else {
            state[STATE_RATE].clamp(0.01, 20.0)
        };

        let mut r = Render {
            in_l: vec![0.0; n],
            out_l: vec![0.0; n],
        };
        let mut in_l = vec![0.0_f32; BLOCK];
        let mut in_r = vec![0.0_f32; BLOCK];
        let mut mod1 = vec![mods[0]; BLOCK];
        let mut mod2 = vec![mods[1]; BLOCK];
        let mut mod3 = vec![mods[2]; BLOCK];
        let mut mod4 = vec![mods[3]; BLOCK];
        let mut out_l = vec![0.0_f32; BLOCK];
        let mut out_r = vec![0.0_f32; BLOCK];
        let mut pos = 0;
        while pos < n {
            let frames = BLOCK.min(n - pos);
            for i in 0..frames {
                let (l, rr) = input(pos + i);
                in_l[i] = l;
                in_r[i] = rr;
            }
            let inputs = [
                in_l.as_mut_ptr(),
                in_r.as_mut_ptr(),
                mod1.as_mut_ptr(),
                mod2.as_mut_ptr(),
                mod3.as_mut_ptr(),
                mod4.as_mut_ptr(),
            ];
            let outputs = [out_l.as_mut_ptr(), out_r.as_mut_ptr()];
            unsafe {
                phaser_flanger_process(
                    inputs.as_ptr(),
                    outputs.as_ptr(),
                    frames as c_int,
                    state.as_mut_ptr() as *mut c_void,
                    std::ptr::null_mut(),
                );
            }
            for i in 0..frames {
                r.in_l[pos + i] = in_l[i];
                r.out_l[pos + i] = out_l[i];
            }
            pos += frames;
        }
        // Stash final state for phase inspection.
        LAST_LFO_PHASE.with(|p| p.set(state[STATE_LFO_PHASE]));
        r
    }

    thread_local! {
        static LAST_LFO_PHASE: std::cell::Cell<f32> = std::cell::Cell::new(0.0);
    }

    fn render(params: &[(usize, f32)], input: impl Fn(usize) -> (f32, f32), n: usize) -> Render {
        render_with_mods(params, [0.0; 4], input, n)
    }

    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|v| v * v).sum::<f32>() / x.len().max(1) as f32).sqrt()
    }

    fn sine(freq: f32, amp: f32) -> impl Fn(usize) -> (f32, f32) {
        move |i| {
            let v = amp * (std::f32::consts::TAU * freq * i as f32 / SR as f32).sin();
            (v, v)
        }
    }

    /// Steady-state gain at one frequency: full-wet render, no sweep.
    fn gain_at_with(extra: &[(usize, f32)], freq: f32, amp: f32, n: usize) -> f32 {
        let mut params = vec![(STATE_AMOUNT, 0.0), (STATE_STEREO, 0.0), (STATE_MIX, 1.0)];
        params.extend_from_slice(extra);
        let r = render(&params, sine(freq, amp), n);
        let tail = (SR / 2).min(n);
        rms(&r.out_l[n - tail..]) / rms(&r.in_l[n - tail..])
    }

    fn gain_at(extra: &[(usize, f32)], freq: f32) -> f32 {
        gain_at_with(extra, freq, 0.25, SR)
    }

    #[derive(Clone, Copy)]
    struct Complex {
        re: f32,
        im: f32,
    }

    impl Complex {
        const ONE: Self = Self { re: 1.0, im: 0.0 };

        fn add(self, rhs: Self) -> Self {
            Self {
                re: self.re + rhs.re,
                im: self.im + rhs.im,
            }
        }

        fn sub(self, rhs: Self) -> Self {
            Self {
                re: self.re - rhs.re,
                im: self.im - rhs.im,
            }
        }

        fn mul(self, rhs: Self) -> Self {
            Self {
                re: self.re * rhs.re - self.im * rhs.im,
                im: self.re * rhs.im + self.im * rhs.re,
            }
        }

        fn scale(self, value: f32) -> Self {
            Self {
                re: self.re * value,
                im: self.im * value,
            }
        }

        fn div(self, rhs: Self) -> Self {
            let denominator = rhs.re * rhs.re + rhs.im * rhs.im;
            Self {
                re: (self.re * rhs.re + self.im * rhs.im) / denominator,
                im: (self.im * rhs.re - self.re * rhs.im) / denominator,
            }
        }

        fn magnitude(self) -> f32 {
            self.re.hypot(self.im)
        }
    }

    fn static_allpass_cascade(freq: f32, notches: usize, center: f32, spread: f32) -> Complex {
        let phase = std::f32::consts::TAU * freq / SR as f32;
        let z1 = Complex {
            re: phase.cos(),
            im: -phase.sin(),
        };
        let z2 = z1.mul(z1);
        notch_frequencies(notches, center, spread, SR as f32)
            .into_iter()
            .fold(Complex::ONE, |cascade, section_freq| {
                let c = allpass_coeffs(section_freq, SR as f32);
                let numerator = Complex::ONE
                    .scale(c.b0)
                    .add(z1.scale(c.b1))
                    .add(z2.scale(c.b2));
                let denominator = Complex::ONE.add(z1.scale(c.a1)).add(z2.scale(c.a2));
                cascade.mul(numerator.div(denominator))
            })
    }

    /// Linearized frozen response of the implemented Phaser topology. This is
    /// used only to locate its narrow peaks accurately enough for process-level
    /// sine tests; the assertions below measure the actual renderer.
    fn static_phaser_gain(freq: f32, feedback: f32) -> f32 {
        let cascade = static_allpass_cascade(freq, 4, 400.0, 0.35);
        let phase = std::f32::consts::TAU * freq / SR as f32;
        let z1 = Complex {
            re: phase.cos(),
            im: -phase.sin(),
        };
        let dc_r = (-std::f32::consts::TAU * FEEDBACK_DC_HZ / SR as f32).exp();
        let dc_block = Complex::ONE.sub(z1).div(Complex::ONE.sub(z1.scale(dc_r)));
        let loop_response = cascade.mul(z1).mul(dc_block);
        let allpass_branch = cascade.div(Complex::ONE.sub(loop_response.scale(feedback)));
        Complex::ONE.add(allpass_branch).scale(0.5).magnitude()
    }

    fn static_phaser_extrema(feedback: f32, maxima: bool) -> Vec<f32> {
        const POINTS: usize = 40_000;
        let mut frequencies = Vec::with_capacity(POINTS);
        let mut gains = Vec::with_capacity(POINTS);
        for i in 0..POINTS {
            let t = i as f32 / (POINTS - 1) as f32;
            let freq = 20.0_f32 * (16_000.0_f32 / 20.0).powf(t);
            frequencies.push(freq);
            gains.push(static_phaser_gain(freq, feedback));
        }
        (1..POINTS - 1)
            .filter_map(|i| {
                let is_extremum = if maxima {
                    gains[i] > gains[i - 1] && gains[i] >= gains[i + 1] && gains[i] > 3.0
                } else {
                    gains[i] < gains[i - 1] && gains[i] <= gains[i + 1] && gains[i] < 0.02
                };
                is_extremum.then_some(frequencies[i])
            })
            .collect()
    }

    /// Stack is the original implementation, including per-stage notch sums,
    /// its exponential-to-linear BLEND layout, and feedback around the final
    /// already-notched signal.
    #[test]
    fn phaser_stack_preserves_original_per_stage_notches() {
        for &(spread, blend) in &[(0.35_f32, 0.0_f32), (0.5, 1.0)] {
            let base = [
                (STATE_MODE, 0.0),
                (STATE_PHASER_CIRCUIT, PHASER_CIRCUIT_STACK as f32),
                (STATE_NOTCHES, 2.0),
                (STATE_CENTER, 400.0),
                (STATE_SPREAD, spread),
                (STATE_BLEND, blend),
                (STATE_FEEDBACK, 1.0),
            ];
            let frequencies = stack_notch_frequencies(2, 400.0, spread, blend, SR as f32);
            for freq in frequencies {
                let notch = gain_at(&base, freq);
                let shoulder = gain_at(&base, freq * 2.0);
                assert!(
                    notch < shoulder * 0.35,
                    "Stack spread {spread} blend {blend}: expected notch at {freq} Hz, gain {notch} vs shoulder {shoulder}"
                );
            }
        }
    }

    /// The global dry/allpass sum must retain one deep notch per section when
    /// feedback is off.
    #[test]
    fn phaser_global_mix_has_one_notch_per_allpass_section() {
        let notches = static_phaser_extrema(0.0, false);
        assert_eq!(
            notches.len(),
            4,
            "expected four analytical notches: {notches:?}"
        );
        let base = [
            (STATE_MODE, 0.0),
            (STATE_NOTCHES, 4.0),
            (STATE_CENTER, 400.0),
            (STATE_SPREAD, 0.35),
            (STATE_FEEDBACK, 0.0),
        ];
        for freq in notches {
            let gain = gain_at(&base, freq);
            assert!(gain < 0.08, "expected deep notch at {freq} Hz, gain {gain}");
        }
    }

    /// Full positive feedback creates a high-Q resonance at every in-phase
    /// crossing of the allpass cascade. This is the characteristic response
    /// lost when dry/allpass mixing is performed inside every stage.
    #[test]
    fn phaser_full_feedback_regenerates_multiple_resonant_bands() {
        let resonances = static_phaser_extrema(FEEDBACK_MAX, true);
        assert_eq!(
            resonances.len(),
            4,
            "expected one positive-feedback resonance per section: {resonances:?}"
        );
        for freq in resonances {
            let without = gain_at_with(&[(STATE_MODE, 0.0)], freq, 1.0e-4, SR * 3);
            let with = gain_at_with(
                &[(STATE_MODE, 0.0), (STATE_FEEDBACK, 1.0)],
                freq,
                1.0e-4,
                SR * 3,
            );
            assert!(
                with > without * 12.0,
                "feedback failed to regenerate {freq} Hz: {with} vs {without}"
            );
        }
    }

    #[test]
    fn phaser_feedback_invert_moves_the_resonant_bands() {
        let inverted_resonances = static_phaser_extrema(-FEEDBACK_MAX, true);
        assert_eq!(inverted_resonances.len(), 4);
        let freq = inverted_resonances[1];
        let positive = gain_at_with(
            &[(STATE_MODE, 0.0), (STATE_FEEDBACK, 1.0)],
            freq,
            1.0e-4,
            SR * 3,
        );
        let inverted = gain_at_with(
            &[
                (STATE_MODE, 0.0),
                (STATE_FEEDBACK, 1.0),
                (STATE_FB_INVERT, 1.0),
            ],
            freq,
            1.0e-4,
            SR * 3,
        );
        assert!(
            inverted > positive * 12.0,
            "inverted feedback should select the opposite phase crossing at {freq} Hz: {inverted} vs {positive}"
        );
    }

    #[test]
    fn phaser_blend_routes_lfo_between_center_and_spread() {
        let center_l2 = 400.0_f32.log2();
        let (center_low, spread_low) = modulated_phaser_layout(center_l2, 0.5, 0.0, 1.0, -1.0);
        let (center_high, spread_high) = modulated_phaser_layout(center_l2, 0.5, 0.0, 1.0, 1.0);
        assert!(center_high > center_low * 16.0);
        assert!((spread_low - 0.5).abs() < 1.0e-6);
        assert!((spread_high - 0.5).abs() < 1.0e-6);

        let (center_narrow, spread_narrow) =
            modulated_phaser_layout(center_l2, 0.5, 1.0, 1.0, -1.0);
        let (center_wide, spread_wide) = modulated_phaser_layout(center_l2, 0.5, 1.0, 1.0, 1.0);
        assert!((center_narrow - 400.0).abs() < 1.0e-3);
        assert!((center_wide - 400.0).abs() < 1.0e-3);
        assert!(spread_narrow < 1.0e-6);
        assert!((spread_wide - 1.0).abs() < 1.0e-6);
    }

    /// Flanger's first comb notch sits at 1/(2·time).
    #[test]
    fn flanger_first_notch_at_half_period() {
        let base = [(STATE_MODE, 1.0), (STATE_FLANGER_TIME, 2.5)];
        let notch_hz = 1000.0 / (2.0 * 2.5); // 200 Hz
        let notch = gain_at(&base, notch_hz);
        let peak = gain_at(&base, notch_hz * 2.0);
        assert!(
            notch < peak * 0.2,
            "expected comb notch at {notch_hz} Hz: {notch} vs peak {peak}"
        );
    }

    /// Full regeneration in either polarity stays bounded (tanh + DC block).
    #[test]
    fn full_feedback_is_stable() {
        for circuit in [PHASER_CIRCUIT_STACK, PHASER_CIRCUIT_CLASSIC] {
            for invert in [0.0, 1.0] {
                for mode in [0.0, 1.0] {
                    let n = SR * 5;
                    let r = render(
                        &[
                            (STATE_MODE, mode),
                            (STATE_PHASER_CIRCUIT, circuit as f32),
                            (STATE_NOTCHES, 12.0),
                            (STATE_FEEDBACK, 1.0),
                            (STATE_FB_INVERT, invert),
                            (STATE_AMOUNT, 1.0),
                            (STATE_MIX, 1.0),
                            (STATE_RATE, 20.0),
                        ],
                        |i| {
                            if i < SR / 10 {
                                let v = 0.8
                                    * (std::f32::consts::TAU * 220.0 * i as f32 / SR as f32).sin();
                                (v, v)
                            } else {
                                (0.0, 0.0)
                            }
                        },
                        n,
                    );
                    let peak = r.out_l.iter().fold(0.0_f32, |a, v| a.max(v.abs()));
                    // This effect deliberately permits resonant gain above 0 dB;
                    // it is not an output limiter. The generous float-headroom
                    // ceiling catches a runaway loop while allowing the extreme
                    // 12-section/20 Hz stress case to bloom naturally.
                    assert!(
                    peak.is_finite() && peak < 32.0,
                    "circuit {circuit} mode {mode} invert {invert}: unbounded output, peak {peak}"
                );
                }
            }
        }
    }

    /// The doubler has no regeneration: feedback setting changes nothing.
    #[test]
    fn doubler_ignores_feedback() {
        let base = [(STATE_MODE, 2.0), (STATE_DOUBLER_TIME, 30.0)];
        let no_fb = gain_at(&base, 440.0);
        let with_fb = gain_at(
            &[
                (STATE_MODE, 2.0),
                (STATE_DOUBLER_TIME, 30.0),
                (STATE_FEEDBACK, 1.0),
            ],
            440.0,
        );
        assert!(
            (no_fb - with_fb).abs() < 0.02,
            "doubler gain moved with feedback: {no_fb} vs {with_fb}"
        );
    }

    /// Synced LFO completes one cycle per division at the pushed BPM.
    #[test]
    fn synced_rate_follows_bpm() {
        // Div index 10 = 4 beats; at 120 BPM the LFO runs 2 s/cycle = 0.5 Hz.
        // Render exactly 1 s and expect the phase to land near 0.5.
        render(
            &[
                (STATE_MODE, 1.0),
                (STATE_SYNC, 1.0),
                (STATE_SYNC_DIV, 10.0),
                (STATE_BPM, 120.0),
            ],
            |_| (0.0, 0.0),
            SR,
        );
        let phase = LAST_LFO_PHASE.with(|p| p.get());
        assert!(
            (phase - 0.5).abs() < 0.01,
            "expected ~0.5 cycles after 1 s at 120 BPM/4 beats, got {phase}"
        );
    }

    /// Text entry and project loading are allowed to transiently present raw
    /// host values outside the descriptor range. Those values must clamp at
    /// the DSP boundary and must never poison persistent smoother state.
    #[test]
    fn out_of_range_or_nan_amount_cannot_kill_audio() {
        for amount in [50.0, f32::NAN] {
            let r = render(
                &[(STATE_MODE, 0.0), (STATE_AMOUNT, amount), (STATE_MIX, 0.5)],
                sine(440.0, 0.25),
                SR,
            );
            let tail = &r.out_l[SR / 2..];
            let peak = tail.iter().fold(0.0_f32, |acc, sample| {
                assert!(sample.is_finite(), "amount {amount:?} produced {sample:?}");
                acc.max(sample.abs())
            });
            assert!(peak > 0.01, "amount {amount:?} silenced the signal");
        }
    }

    /// Dry/wet at zero is transparent (warmth and sweep act on the wet leg).
    #[test]
    fn dry_at_zero_mix_is_untouched() {
        let n = SR;
        let r = render(
            &[
                (STATE_MODE, 0.0),
                (STATE_MIX, 0.0),
                (STATE_WARMTH, 1.0),
                (STATE_AMOUNT, 1.0),
            ],
            sine(440.0, 0.5),
            n,
        );
        let tail = SR / 2;
        let diff: Vec<f32> = (n - tail..n).map(|i| r.out_l[i] - r.in_l[i]).collect();
        assert!(
            rms(&diff) < 1.0e-3,
            "mix 0 should pass dry untouched, diff rms {}",
            rms(&diff)
        );
    }

    /// Disabled node bypasses exactly.
    #[test]
    fn disabled_is_exact_bypass() {
        let n = SR / 2;
        let r = render(&[(STATE_ENABLED, 0.0)], sine(440.0, 0.5), n);
        for i in 0..n {
            assert_eq!(r.out_l[i], r.in_l[i]);
        }
    }

    /// A host modulator writing into a mix depth slot opens the wet path.
    #[test]
    fn mod_input_drives_mix() {
        let n = SR;
        let base = [
            (STATE_MODE, 1.0),
            (STATE_FLANGER_TIME, 2.5),
            (STATE_MIX, 0.0),
            (STATE_AMOUNT, 0.0),
        ];
        let dry = render(&base, sine(200.0, 0.25), n);
        let mut with_depth = base.to_vec();
        with_depth.push((STATE_MOD_MIX_DEPTH_1, 1.0));
        let modded = render_with_mods(&with_depth, [1.0, 0.0, 0.0, 0.0], sine(200.0, 0.25), n);
        let tail = SR / 2;
        let dry_rms = rms(&dry.out_l[n - tail..]);
        let mod_rms = rms(&modded.out_l[n - tail..]);
        // 200 Hz is the comb notch: full wet should carve it way down.
        assert!(
            mod_rms < dry_rms * 0.35,
            "mod-driven mix should engage the flanger notch: {mod_rms} vs {dry_rms}"
        );
    }
}
