//! Roland SDD-320 "Dimension D"–style chorus.
//!
//! What makes the Dimension lush where a textbook chorus sounds cheap:
//!
//! * **No feedback.** The wet path is a single pass through two short BBD
//!   delay lines — there is no regeneration, so no flanger comb resonance.
//! * **Antiphase dual lines + inverted crossmix.** One triangle LFO drives
//!   both lines 180° apart, and each wet signal is mixed into the *opposite*
//!   output with inverted polarity: `L = dry + wetA − w·wetB`,
//!   `R = dry + wetB − w·wetA`. At full width the wet cancels exactly in the
//!   mono sum — all the motion lives in the side channel, so you get width
//!   without audible vibrato and perfect mono compatibility.
//! * **Tiny pitch deviation.** Mode-dependent rate/depth pairs keep the peak
//!   deviation in the single-digit-cents range; it never reads as pitch.
//! * **BBD voicing.** The wet path is band-limited (input HPF, 4th-order
//!   output LPF) and runs through an NE570-style compander (2:1 compress in,
//!   1:2 expand out) around a soft tanh saturator — program-dependent warmth
//!   and noise gating. The dry path is untouched.
//!
//! The four DIMENSION MODE buttons combine like the hardware's paralleled
//! timing resistors: pressing several averages their rate/depth pairs. The
//! DYNAMIC COLOR switch revoices the compander/saturation (SMOOTH → LF SAT 2
//! lets progressively more low end into the saturating wet path), and the
//! LFO shape can be overridden beyond the original's rounded triangle.

use crate::audiograph::NodeVTable;
use std::os::raw::{c_int, c_void};

// Power of two so reads can wrap with a mask. 4096 samples ≈ 21 ms at 192 kHz,
// comfortably above the ~9 ms worst-case read (base 5.4 ms + 2× depth).
const DELAY_BUF_LEN: usize = 4096;
const DELAY_BUF_MASK: usize = DELAY_BUF_LEN - 1;

// Static BBD lengths. Slightly different per line so the two wet signals
// never null against each other identically.
const BASE_DELAY_MS: [f32; 2] = [4.2, 5.4];

// Per-button (rate Hz, depth ms). Intensity rises 1→4 while the peak pitch
// deviation (≈ 4·depth·rate for a triangle) stays in the 2.5–7.5 cent range.
const MODE_RATE_HZ: [f32; 4] = [0.22, 0.45, 0.9, 1.8];
const MODE_DEPTH_MS: [f32; 4] = [1.7, 1.2, 0.85, 0.6];

// Dynamic color voicings: wet-path HPF cutoff, saturation drive, compander
// attack/release. LF SAT positions open the highpass so lows reach the
// saturator; SMOOTH slows and lightens the compander.
const COLOR_HPF_HZ: [f32; 4] = [260.0, 220.0, 110.0, 55.0];
const COLOR_DRIVE: [f32; 4] = [0.9, 1.3, 1.9, 2.6];
const COLOR_ATTACK_MS: [f32; 4] = [6.0, 3.0, 3.0, 2.5];
const COLOR_RELEASE_MS: [f32; 4] = [80.0, 40.0, 40.0, 30.0];

// NE570-style compander reference level: comp gain = sqrt(REF/env),
// expander gain = env/REF, so matched envelopes give unity through-gain.
const COMPANDER_REF: f32 = 0.25;

// ── Parameter slots ──
const STATE_ENABLED: usize = 0;
const STATE_BTN1: usize = 1;
const STATE_BTN2: usize = 2;
const STATE_BTN3: usize = 3;
const STATE_BTN4: usize = 4;
const STATE_COLOR: usize = 5; // 0 smooth, 1 default, 2 lf sat 1, 3 lf sat 2
const STATE_SHAPE: usize = 6; // 0 default (rounded tri), 1 sine, 2 ramp, 3 square, 4 triangle
const STATE_RATE: usize = 7; // rate multiplier 0.25..4
const STATE_DEPTH: usize = 8; // depth multiplier 0..2
const STATE_WIDTH: usize = 9; // 0 dual-mono .. 1 full inverted crossmix
const STATE_TONE: usize = 10; // wet LPF Hz
const STATE_MIX: usize = 11; // wet level
const STATE_SAMPLE_RATE: usize = 12;

// Host-modulation depth slots (4 modulator slots × 2 targets).
const STATE_MOD_DEPTH_DEPTH_1: usize = 13;
const STATE_MOD_MIX_DEPTH_1: usize = 17;

// ── Runtime state ──
const STATE_LFO_PHASE: usize = 21;
const STATE_LFO_LP_A: usize = 22;
const STATE_LFO_LP_B: usize = 23;
const STATE_SMOOTH_RATE: usize = 24;
const STATE_SMOOTH_DEPTH_MS: usize = 25;
const STATE_SMOOTH_WIDTH: usize = 26;
const STATE_SMOOTH_MIX: usize = 27;
const STATE_SMOOTH_WET_ON: usize = 28;
const STATE_COMP_ENV_A: usize = 29;
const STATE_COMP_ENV_B: usize = 30;
const STATE_EXP_ENV_A: usize = 31;
const STATE_EXP_ENV_B: usize = 32;
const STATE_HPF_A_Z1: usize = 33;
const STATE_HPF_A_Z2: usize = 34;
const STATE_HPF_B_Z1: usize = 35;
const STATE_HPF_B_Z2: usize = 36;
const STATE_LPF_A1_Z1: usize = 37;
const STATE_LPF_A1_Z2: usize = 38;
const STATE_LPF_A2_Z1: usize = 39;
const STATE_LPF_A2_Z2: usize = 40;
const STATE_LPF_B1_Z1: usize = 41;
const STATE_LPF_B1_Z2: usize = 42;
const STATE_LPF_B2_Z1: usize = 43;
const STATE_LPF_B2_Z2: usize = 44;
const STATE_WRITE_POS: usize = 45;
const STATE_SMOOTH_TONE: usize = 46;
const STATE_SMOOTH_DRIVE: usize = 47;
const STATE_SMOOTH_HPF: usize = 48;

const STATE_BUF_A: usize = 49;
const STATE_BUF_B: usize = STATE_BUF_A + DELAY_BUF_LEN;

pub const DIMENSION_STATE_SIZE: usize = STATE_BUF_B + DELAY_BUF_LEN;

pub const DIMENSION_PARAM_ENABLED: u64 = STATE_ENABLED as u64;
pub const DIMENSION_PARAM_BTN1: u64 = STATE_BTN1 as u64;
pub const DIMENSION_PARAM_BTN2: u64 = STATE_BTN2 as u64;
pub const DIMENSION_PARAM_BTN3: u64 = STATE_BTN3 as u64;
pub const DIMENSION_PARAM_BTN4: u64 = STATE_BTN4 as u64;
pub const DIMENSION_PARAM_COLOR: u64 = STATE_COLOR as u64;
pub const DIMENSION_PARAM_SHAPE: u64 = STATE_SHAPE as u64;
pub const DIMENSION_PARAM_RATE: u64 = STATE_RATE as u64;
pub const DIMENSION_PARAM_DEPTH: u64 = STATE_DEPTH as u64;
pub const DIMENSION_PARAM_WIDTH: u64 = STATE_WIDTH as u64;
pub const DIMENSION_PARAM_TONE: u64 = STATE_TONE as u64;
pub const DIMENSION_PARAM_MIX: u64 = STATE_MIX as u64;
pub const DIMENSION_PARAM_MOD_DEPTH_DEPTH_1: u64 = STATE_MOD_DEPTH_DEPTH_1 as u64;
pub const DIMENSION_PARAM_MOD_DEPTH_DEPTH_2: u64 = STATE_MOD_DEPTH_DEPTH_1 as u64 + 1;
pub const DIMENSION_PARAM_MOD_DEPTH_DEPTH_3: u64 = STATE_MOD_DEPTH_DEPTH_1 as u64 + 2;
pub const DIMENSION_PARAM_MOD_DEPTH_DEPTH_4: u64 = STATE_MOD_DEPTH_DEPTH_1 as u64 + 3;
pub const DIMENSION_PARAM_MOD_MIX_DEPTH_1: u64 = STATE_MOD_MIX_DEPTH_1 as u64;
pub const DIMENSION_PARAM_MOD_MIX_DEPTH_2: u64 = STATE_MOD_MIX_DEPTH_1 as u64 + 1;
pub const DIMENSION_PARAM_MOD_MIX_DEPTH_3: u64 = STATE_MOD_MIX_DEPTH_1 as u64 + 2;
pub const DIMENSION_PARAM_MOD_MIX_DEPTH_4: u64 = STATE_MOD_MIX_DEPTH_1 as u64 + 3;

#[derive(Clone, Copy)]
struct BiquadCoeffs {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
}

#[inline]
fn biquad_sample(input: f32, coeffs: BiquadCoeffs, z1: &mut f32, z2: &mut f32) -> f32 {
    let out = coeffs.b0 * input + *z1;
    *z1 = coeffs.b1 * input - coeffs.a1 * out + *z2;
    *z2 = coeffs.b2 * input - coeffs.a2 * out;
    out
}

fn lowpass_coeffs(freq: f32, sr: f32) -> BiquadCoeffs {
    let omega = std::f32::consts::TAU * freq.clamp(20.0, sr * 0.49) / sr.max(1.0);
    let sin = omega.sin();
    let cos = omega.cos();
    let alpha = sin * std::f32::consts::FRAC_1_SQRT_2;
    let a0 = 1.0 + alpha;
    BiquadCoeffs {
        b0: (1.0 - cos) * 0.5 / a0,
        b1: (1.0 - cos) / a0,
        b2: (1.0 - cos) * 0.5 / a0,
        a1: (-2.0 * cos) / a0,
        a2: (1.0 - alpha) / a0,
    }
}

fn highpass_coeffs(freq: f32, sr: f32) -> BiquadCoeffs {
    let omega = std::f32::consts::TAU * freq.clamp(10.0, sr * 0.49) / sr.max(1.0);
    let sin = omega.sin();
    let cos = omega.cos();
    let alpha = sin * std::f32::consts::FRAC_1_SQRT_2;
    let a0 = 1.0 + alpha;
    BiquadCoeffs {
        b0: (1.0 + cos) * 0.5 / a0,
        b1: -(1.0 + cos) / a0,
        b2: (1.0 + cos) * 0.5 / a0,
        a1: (-2.0 * cos) / a0,
        a2: (1.0 - alpha) / a0,
    }
}

#[inline]
fn one_pole_coef(freq: f32, sr: f32) -> f32 {
    1.0 - (-std::f32::consts::TAU * freq / sr.max(1.0)).exp()
}

#[inline]
fn time_coef(ms: f32, sr: f32) -> f32 {
    1.0 - (-1.0 / (ms.max(0.01) * 0.001 * sr.max(1.0))).exp()
}

// Raw LFO shapes, all in -1..1 over phase 0..1. The hardware's BBD clock
// circuit rounds the triangle; we emulate that with a one-pole smoother whose
// cutoff depends on the selected shape (heavy for "default", light otherwise
// so ramps/squares keep their character without clicking).
#[inline]
fn lfo_raw(shape: usize, phase: f32) -> f32 {
    match shape {
        1 => (std::f32::consts::TAU * phase).sin(),
        2 => 2.0 * phase - 1.0,
        3 => {
            if phase < 0.5 {
                1.0
            } else {
                -1.0
            }
        }
        // 0 (default) and 4 are both triangle at the source.
        _ => {
            if phase < 0.5 {
                4.0 * phase - 1.0
            } else {
                3.0 - 4.0 * phase
            }
        }
    }
}

// 4-point Hermite read of a modulated line — linear interpolation dulls the
// wet path audibly while the read head glides.
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

unsafe extern "C" fn dimension_init(
    state: *mut c_void,
    sample_rate: c_int,
    _max_block: c_int,
    _initial_state: *const c_void,
) {
    let s = state as *mut f32;
    std::ptr::write_bytes(s, 0, DIMENSION_STATE_SIZE);
    *s.add(STATE_ENABLED) = 1.0;
    *s.add(STATE_BTN2) = 1.0;
    *s.add(STATE_COLOR) = 1.0;
    *s.add(STATE_SHAPE) = 0.0;
    *s.add(STATE_RATE) = 1.0;
    *s.add(STATE_DEPTH) = 1.0;
    *s.add(STATE_WIDTH) = 1.0;
    *s.add(STATE_TONE) = 7200.0;
    *s.add(STATE_MIX) = 0.7;
    *s.add(STATE_SAMPLE_RATE) = sample_rate as f32;
    *s.add(STATE_SMOOTH_RATE) = MODE_RATE_HZ[1];
    *s.add(STATE_SMOOTH_DEPTH_MS) = MODE_DEPTH_MS[1];
    *s.add(STATE_SMOOTH_WIDTH) = 1.0;
    *s.add(STATE_SMOOTH_MIX) = 0.7;
    *s.add(STATE_SMOOTH_WET_ON) = 1.0;
    *s.add(STATE_SMOOTH_TONE) = 7200.0;
    *s.add(STATE_SMOOTH_DRIVE) = COLOR_DRIVE[1];
    *s.add(STATE_SMOOTH_HPF) = COLOR_HPF_HZ[1];
}

unsafe extern "C" fn dimension_process(
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

    let sr = (*s.add(STATE_SAMPLE_RATE)).max(1.0);

    // Mode buttons combine by averaging their rate/depth pairs (the hardware
    // parallels timing resistors when several are latched).
    let mut rate_sum = 0.0_f32;
    let mut depth_sum = 0.0_f32;
    let mut pressed = 0.0_f32;
    for (i, slot) in [STATE_BTN1, STATE_BTN2, STATE_BTN3, STATE_BTN4]
        .into_iter()
        .enumerate()
    {
        if *s.add(slot) > 0.5 {
            rate_sum += MODE_RATE_HZ[i];
            depth_sum += MODE_DEPTH_MS[i];
            pressed += 1.0;
        }
    }
    let wet_on_target = if pressed > 0.0 { 1.0 } else { 0.0 };
    let rate_target = if pressed > 0.0 {
        (rate_sum / pressed) * (*s.add(STATE_RATE)).clamp(0.25, 4.0)
    } else {
        *s.add(STATE_SMOOTH_RATE)
    };
    let depth_knob = (*s.add(STATE_DEPTH)).clamp(0.0, 2.0);
    let mode_depth_ms = if pressed > 0.0 {
        depth_sum / pressed
    } else {
        *s.add(STATE_SMOOTH_DEPTH_MS)
    };

    let color = (*s.add(STATE_COLOR)).round().clamp(0.0, 3.0) as usize;
    let shape = (*s.add(STATE_SHAPE)).round().clamp(0.0, 4.0) as usize;
    let width_target = (*s.add(STATE_WIDTH)).clamp(0.0, 1.0);
    let tone_target = (*s.add(STATE_TONE)).clamp(2000.0, 16000.0);
    let mix_knob = (*s.add(STATE_MIX)).clamp(0.0, 1.5);

    // Knob smoothing (~20 Hz) so plocks and mode stabs never zipper; the LFO
    // smoother is shape-dependent (the rounded-triangle "BBD clock" feel).
    let knob_coef = one_pole_coef(20.0, sr);
    let lfo_coef = one_pole_coef(if shape == 0 { 0.8 } else { 6.0 }, sr);
    let attack = time_coef(COLOR_ATTACK_MS[color], sr);
    let release = time_coef(COLOR_RELEASE_MS[color], sr);

    // Color voicing smoothed per block (switch flips are rare and small).
    let drive_target = COLOR_DRIVE[color];
    let hpf_target = COLOR_HPF_HZ[color];
    let smooth_tone = *s.add(STATE_SMOOTH_TONE) + 0.3 * (tone_target - *s.add(STATE_SMOOTH_TONE));
    let smooth_drive =
        *s.add(STATE_SMOOTH_DRIVE) + 0.3 * (drive_target - *s.add(STATE_SMOOTH_DRIVE));
    let smooth_hpf = *s.add(STATE_SMOOTH_HPF) + 0.3 * (hpf_target - *s.add(STATE_SMOOTH_HPF));
    *s.add(STATE_SMOOTH_TONE) = smooth_tone;
    *s.add(STATE_SMOOTH_DRIVE) = smooth_drive;
    *s.add(STATE_SMOOTH_HPF) = smooth_hpf;
    let hpf = highpass_coeffs(smooth_hpf, sr);
    let lpf = lowpass_coeffs(smooth_tone, sr);
    let drive = smooth_drive.max(0.1);
    let inv_drive = 1.0 / drive;

    let buf_a = s.add(STATE_BUF_A);
    let buf_b = s.add(STATE_BUF_B);

    let mut phase = *s.add(STATE_LFO_PHASE);
    let mut lfo_a = *s.add(STATE_LFO_LP_A);
    let mut lfo_b = *s.add(STATE_LFO_LP_B);
    let mut sm_rate = *s.add(STATE_SMOOTH_RATE);
    let mut sm_depth_ms = *s.add(STATE_SMOOTH_DEPTH_MS);
    let mut sm_width = *s.add(STATE_SMOOTH_WIDTH);
    let mut sm_mix = *s.add(STATE_SMOOTH_MIX);
    let mut sm_wet_on = *s.add(STATE_SMOOTH_WET_ON);
    let mut comp_env_a = *s.add(STATE_COMP_ENV_A);
    let mut comp_env_b = *s.add(STATE_COMP_ENV_B);
    let mut exp_env_a = *s.add(STATE_EXP_ENV_A);
    let mut exp_env_b = *s.add(STATE_EXP_ENV_B);
    let mut hpf_a_z1 = *s.add(STATE_HPF_A_Z1);
    let mut hpf_a_z2 = *s.add(STATE_HPF_A_Z2);
    let mut hpf_b_z1 = *s.add(STATE_HPF_B_Z1);
    let mut hpf_b_z2 = *s.add(STATE_HPF_B_Z2);
    let mut lpf_a1_z1 = *s.add(STATE_LPF_A1_Z1);
    let mut lpf_a1_z2 = *s.add(STATE_LPF_A1_Z2);
    let mut lpf_a2_z1 = *s.add(STATE_LPF_A2_Z1);
    let mut lpf_a2_z2 = *s.add(STATE_LPF_A2_Z2);
    let mut lpf_b1_z1 = *s.add(STATE_LPF_B1_Z1);
    let mut lpf_b1_z2 = *s.add(STATE_LPF_B1_Z2);
    let mut lpf_b2_z1 = *s.add(STATE_LPF_B2_Z1);
    let mut lpf_b2_z2 = *s.add(STATE_LPF_B2_Z2);
    let mut wpos = (*s.add(STATE_WRITE_POS)) as usize & DELAY_BUF_MASK;

    let depth_mod_amt = [
        *s.add(STATE_MOD_DEPTH_DEPTH_1),
        *s.add(STATE_MOD_DEPTH_DEPTH_1 + 1),
        *s.add(STATE_MOD_DEPTH_DEPTH_1 + 2),
        *s.add(STATE_MOD_DEPTH_DEPTH_1 + 3),
    ];
    let mix_mod_amt = [
        *s.add(STATE_MOD_MIX_DEPTH_1),
        *s.add(STATE_MOD_MIX_DEPTH_1 + 1),
        *s.add(STATE_MOD_MIX_DEPTH_1 + 2),
        *s.add(STATE_MOD_MIX_DEPTH_1 + 3),
    ];
    let base_samples = [BASE_DELAY_MS[0] * 0.001 * sr, BASE_DELAY_MS[1] * 0.001 * sr];

    for i in 0..nf {
        let dry_l = *in0.add(i);
        let dry_r = *in1.add(i);

        let mut depth_mod = 0.0_f32;
        let mut mix_mod = 0.0_f32;
        for slot in 0..4 {
            let m = *mod_inputs[slot].add(i);
            depth_mod += m * depth_mod_amt[slot];
            mix_mod += m * mix_mod_amt[slot];
        }

        sm_rate += knob_coef * (rate_target - sm_rate);
        sm_depth_ms += knob_coef * (mode_depth_ms - sm_depth_ms);
        sm_width += knob_coef * (width_target - sm_width);
        sm_mix += knob_coef * ((mix_knob + mix_mod).clamp(0.0, 1.5) - sm_mix);
        sm_wet_on += knob_coef * (wet_on_target - sm_wet_on);

        phase += sm_rate / sr;
        if phase >= 1.0 {
            phase -= 1.0;
        }
        let mut phase_b = phase + 0.5;
        if phase_b >= 1.0 {
            phase_b -= 1.0;
        }
        lfo_a += lfo_coef * (lfo_raw(shape, phase) - lfo_a);
        lfo_b += lfo_coef * (lfo_raw(shape, phase_b) - lfo_b);

        let depth_eff = (depth_knob + depth_mod).clamp(0.0, 2.0);
        let depth_samples = sm_depth_ms * depth_eff * 0.001 * sr;

        // ── Wet path, line A (fed by L) / line B (fed by R) ──
        // HPF → 2:1 compress → tanh → delay write.
        let xa = biquad_sample(dry_l, hpf, &mut hpf_a_z1, &mut hpf_a_z2);
        let xb = biquad_sample(dry_r, hpf, &mut hpf_b_z1, &mut hpf_b_z2);

        let coef_a = if xa.abs() > comp_env_a {
            attack
        } else {
            release
        };
        comp_env_a += coef_a * (xa.abs() - comp_env_a);
        let coef_b = if xb.abs() > comp_env_b {
            attack
        } else {
            release
        };
        comp_env_b += coef_b * (xb.abs() - comp_env_b);
        let comp_gain_a = (COMPANDER_REF / comp_env_a.max(1.0e-4)).sqrt().min(8.0);
        let comp_gain_b = (COMPANDER_REF / comp_env_b.max(1.0e-4)).sqrt().min(8.0);

        let wa = (xa * comp_gain_a * drive).tanh() * inv_drive;
        let wb = (xb * comp_gain_b * drive).tanh() * inv_drive;
        *buf_a.add(wpos) = wa;
        *buf_b.add(wpos) = wb;

        // Modulated reads (antiphase), then 4th-order LPF → 1:2 expand.
        let delay_a = base_samples[0] + depth_samples * lfo_a;
        let delay_b = base_samples[1] + depth_samples * lfo_b;
        let ra = line_read(buf_a, wpos, delay_a);
        let rb = line_read(buf_b, wpos, delay_b);

        let mut ya = biquad_sample(ra, lpf, &mut lpf_a1_z1, &mut lpf_a1_z2);
        ya = biquad_sample(ya, lpf, &mut lpf_a2_z1, &mut lpf_a2_z2);
        let mut yb = biquad_sample(rb, lpf, &mut lpf_b1_z1, &mut lpf_b1_z2);
        yb = biquad_sample(yb, lpf, &mut lpf_b2_z1, &mut lpf_b2_z2);

        let coef_ea = if ya.abs() > exp_env_a {
            attack
        } else {
            release
        };
        exp_env_a += coef_ea * (ya.abs() - exp_env_a);
        let coef_eb = if yb.abs() > exp_env_b {
            attack
        } else {
            release
        };
        exp_env_b += coef_eb * (yb.abs() - exp_env_b);
        let wet_a = ya * (exp_env_a / COMPANDER_REF).min(4.0);
        let wet_b = yb * (exp_env_b / COMPANDER_REF).min(4.0);

        // The Dimension crossmix: each wet into the opposite channel with
        // inverted polarity. At width 1 the wet cancels exactly in mono.
        let g = sm_mix * sm_wet_on;
        *out0.add(i) = dry_l + g * (wet_a - sm_width * wet_b);
        *out1.add(i) = dry_r + g * (wet_b - sm_width * wet_a);

        wpos = (wpos + 1) & DELAY_BUF_MASK;
    }

    *s.add(STATE_LFO_PHASE) = phase;
    *s.add(STATE_LFO_LP_A) = lfo_a;
    *s.add(STATE_LFO_LP_B) = lfo_b;
    *s.add(STATE_SMOOTH_RATE) = sm_rate;
    *s.add(STATE_SMOOTH_DEPTH_MS) = sm_depth_ms;
    *s.add(STATE_SMOOTH_WIDTH) = sm_width;
    *s.add(STATE_SMOOTH_MIX) = sm_mix;
    *s.add(STATE_SMOOTH_WET_ON) = sm_wet_on;
    *s.add(STATE_COMP_ENV_A) = comp_env_a;
    *s.add(STATE_COMP_ENV_B) = comp_env_b;
    *s.add(STATE_EXP_ENV_A) = exp_env_a;
    *s.add(STATE_EXP_ENV_B) = exp_env_b;
    *s.add(STATE_HPF_A_Z1) = hpf_a_z1;
    *s.add(STATE_HPF_A_Z2) = hpf_a_z2;
    *s.add(STATE_HPF_B_Z1) = hpf_b_z1;
    *s.add(STATE_HPF_B_Z2) = hpf_b_z2;
    *s.add(STATE_LPF_A1_Z1) = lpf_a1_z1;
    *s.add(STATE_LPF_A1_Z2) = lpf_a1_z2;
    *s.add(STATE_LPF_A2_Z1) = lpf_a2_z1;
    *s.add(STATE_LPF_A2_Z2) = lpf_a2_z2;
    *s.add(STATE_LPF_B1_Z1) = lpf_b1_z1;
    *s.add(STATE_LPF_B1_Z2) = lpf_b1_z2;
    *s.add(STATE_LPF_B2_Z1) = lpf_b2_z1;
    *s.add(STATE_LPF_B2_Z2) = lpf_b2_z2;
    *s.add(STATE_WRITE_POS) = wpos as f32;
}

pub fn dimension_vtable() -> NodeVTable {
    NodeVTable {
        process: Some(dimension_process),
        init: Some(dimension_init),
        reset: None,
        migrate: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: usize = 48000;
    const BLOCK: usize = 128;

    struct Render {
        in_l: Vec<f32>,
        in_r: Vec<f32>,
        out_l: Vec<f32>,
        out_r: Vec<f32>,
    }

    fn render(params: &[(usize, f32)], input: impl Fn(usize) -> (f32, f32), n: usize) -> Render {
        let mut state = vec![0.0_f32; DIMENSION_STATE_SIZE];
        unsafe {
            dimension_init(
                state.as_mut_ptr() as *mut c_void,
                SR as c_int,
                BLOCK as c_int,
                std::ptr::null(),
            );
        }
        // Mode buttons are exclusive in these tests: clear the default.
        state[STATE_BTN2] = 0.0;
        for &(slot, value) in params {
            state[slot] = value;
        }
        // Pre-seed smoothers at their targets so short renders measure the
        // steady state rather than the 20 Hz parameter glide.
        state[STATE_SMOOTH_WIDTH] = state[STATE_WIDTH];
        state[STATE_SMOOTH_MIX] = state[STATE_MIX];
        let mut pressed = 0.0;
        let mut rate = 0.0;
        let mut depth = 0.0;
        for i in 0..4 {
            if state[STATE_BTN1 + i] > 0.5 {
                pressed += 1.0;
                rate += MODE_RATE_HZ[i];
                depth += MODE_DEPTH_MS[i];
            }
        }
        if pressed > 0.0 {
            state[STATE_SMOOTH_RATE] = rate / pressed * state[STATE_RATE];
            state[STATE_SMOOTH_DEPTH_MS] = depth / pressed;
            state[STATE_SMOOTH_WET_ON] = 1.0;
        } else {
            state[STATE_SMOOTH_WET_ON] = 0.0;
        }

        let mut r = Render {
            in_l: vec![0.0; n],
            in_r: vec![0.0; n],
            out_l: vec![0.0; n],
            out_r: vec![0.0; n],
        };
        let mut in_l = vec![0.0_f32; BLOCK];
        let mut in_r = vec![0.0_f32; BLOCK];
        let mut zeros1 = vec![0.0_f32; BLOCK];
        let mut zeros2 = vec![0.0_f32; BLOCK];
        let mut zeros3 = vec![0.0_f32; BLOCK];
        let mut zeros4 = vec![0.0_f32; BLOCK];
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
                zeros1.as_mut_ptr(),
                zeros2.as_mut_ptr(),
                zeros3.as_mut_ptr(),
                zeros4.as_mut_ptr(),
            ];
            let outputs = [out_l.as_mut_ptr(), out_r.as_mut_ptr()];
            unsafe {
                dimension_process(
                    inputs.as_ptr(),
                    outputs.as_ptr(),
                    frames as c_int,
                    state.as_mut_ptr() as *mut c_void,
                    std::ptr::null_mut(),
                );
            }
            for i in 0..frames {
                r.in_l[pos + i] = in_l[i];
                r.in_r[pos + i] = in_r[i];
                r.out_l[pos + i] = out_l[i];
                r.out_r[pos + i] = out_r[i];
            }
            pos += frames;
        }
        r
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

    /// The defining Dimension property: at full width the wet signal cancels
    /// exactly in the mono sum — width without mono pitch wobble.
    #[test]
    fn wet_cancels_in_mono_at_full_width() {
        let n = SR * 3;
        let r = render(
            &[(STATE_BTN2, 1.0), (STATE_WIDTH, 1.0), (STATE_MIX, 0.9)],
            sine(440.0, 0.5),
            n,
        );
        let tail = SR;
        let wet_l: Vec<f32> = (n - tail..n).map(|i| r.out_l[i] - r.in_l[i]).collect();
        let wet_r: Vec<f32> = (n - tail..n).map(|i| r.out_r[i] - r.in_r[i]).collect();
        let mono: Vec<f32> = wet_l.iter().zip(&wet_r).map(|(a, b)| a + b).collect();
        let side: Vec<f32> = wet_l.iter().zip(&wet_r).map(|(a, b)| a - b).collect();
        let side_rms = rms(&side);
        assert!(
            side_rms > 0.01,
            "wet path should be audible, side rms {side_rms}"
        );
        assert!(
            rms(&mono) < 0.02 * side_rms,
            "mono wet should cancel: mono {} side {}",
            rms(&mono),
            side_rms
        );
    }

    /// Pitch deviation of a single line stays in the gentle single-digit-cent
    /// range even on the most intense mode — chorus depth without vibrato.
    #[test]
    fn pitch_deviation_is_subtle() {
        let n = SR * 6;
        // width 0 so wet L is line A alone; mode 4 = fastest/steepest.
        let r = render(
            &[(STATE_BTN4, 1.0), (STATE_WIDTH, 0.0), (STATE_MIX, 1.0)],
            sine(440.0, 0.5),
            n,
        );
        let warm = SR * 2;
        let wet: Vec<f32> = (warm..n).map(|i| r.out_l[i] - r.in_l[i]).collect();
        // Positive-going zero crossings with linear interpolation.
        let mut crossings = Vec::new();
        for i in 1..wet.len() {
            if wet[i - 1] <= 0.0 && wet[i] > 0.0 {
                let frac = -wet[i - 1] / (wet[i] - wet[i - 1]);
                crossings.push((i - 1) as f32 + frac);
            }
        }
        assert!(
            crossings.len() > 1000,
            "expected a steady tone in the wet path"
        );
        // Average frequency over 24-period windows, track cents vs 440.
        let win = 24;
        let mut max_cents: f32 = 0.0;
        let mut sum_freq = 0.0;
        let mut count = 0;
        for w in crossings.windows(win + 1).step_by(win / 2) {
            let freq = win as f32 * SR as f32 / (w[win] - w[0]);
            let cents = 1200.0 * (freq / 440.0).log2();
            max_cents = max_cents.max(cents.abs());
            sum_freq += freq;
            count += 1;
        }
        let mean_freq = sum_freq / count as f32;
        let mean_cents = 1200.0 * (mean_freq / 440.0_f32).log2();
        assert!(
            mean_cents.abs() < 1.5,
            "average pitch should stay put, drifted {mean_cents} cents"
        );
        assert!(
            (2.0..15.0).contains(&max_cents),
            "peak deviation should be subtle but present, got {max_cents} cents"
        );
    }

    /// The wet path is band-limited like a BBD: lows stay dry/mono, highs
    /// stay unmodulated.
    #[test]
    fn wet_path_is_band_limited() {
        let n = SR * 3;
        let params = [(STATE_BTN2, 1.0), (STATE_WIDTH, 0.0), (STATE_MIX, 1.0)];
        let gain_at = |freq: f32| {
            let r = render(&params, sine(freq, 0.25), n);
            let tail = SR;
            let wet: Vec<f32> = (n - tail..n).map(|i| r.out_l[i] - r.in_l[i]).collect();
            let input: Vec<f32> = (n - tail..n).map(|i| r.in_l[i]).collect();
            rms(&wet) / rms(&input)
        };
        let low = gain_at(30.0);
        let mid = gain_at(1000.0);
        let high = gain_at(12000.0);
        assert!(mid > 0.3, "mid band should pass, gain {mid}");
        assert!(
            low < mid * 0.25,
            "30 Hz should be filtered from the wet path: low {low} mid {mid}"
        );
        assert!(
            high < mid * 0.25,
            "12 kHz should be filtered from the wet path: high {high} mid {mid}"
        );
    }

    /// The compander gates the wet path below the compressor's gain-clamp
    /// knee, like a real NE570 expander ducking BBD noise. (Above the knee a
    /// matched 2:1/1:2 compander is gain-transparent for steady tones — the
    /// level dependence lives in the floor and the transient response.)
    #[test]
    fn compander_expands_quiet_signals_down() {
        let n = SR * 3;
        let params = [(STATE_BTN2, 1.0), (STATE_WIDTH, 0.0), (STATE_MIX, 1.0)];
        let wet_gain = |amp: f32| {
            let r = render(&params, sine(1000.0, amp), n);
            let tail = SR;
            let wet: Vec<f32> = (n - tail..n).map(|i| r.out_l[i] - r.in_l[i]).collect();
            let input: Vec<f32> = (n - tail..n).map(|i| r.in_l[i]).collect();
            rms(&wet) / rms(&input)
        };
        let loud = wet_gain(0.4);
        let quiet = wet_gain(0.0004);
        assert!(
            quiet < loud * 0.5,
            "quiet input should expand down: quiet gain {quiet}, loud gain {loud}"
        );
    }

    /// No buttons latched = wet muted (the red "0" button); disabled = bypass.
    #[test]
    fn no_mode_buttons_means_dry() {
        let n = SR * 2;
        let r = render(&[(STATE_WIDTH, 1.0), (STATE_MIX, 1.0)], sine(440.0, 0.5), n);
        let tail = SR;
        let wet: Vec<f32> = (n - tail..n).map(|i| r.out_l[i] - r.in_l[i]).collect();
        assert!(
            rms(&wet) < 1.0e-4,
            "wet should be silent with no mode, rms {}",
            rms(&wet)
        );
    }
}
