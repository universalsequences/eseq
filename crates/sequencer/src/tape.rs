//! Analog tape emulation built around the Jiles–Atherton hysteresis model.
//!
//! The audio path per channel is:
//!
//!   dry → drive trim → 4× upsample → J-A hysteresis step at the oversampled
//!   rate → 4× downsample → magnetic-loss filter (LP + low-shelf "head bump")
//!   → output trim → dry/wet blend.
//!
//! Oversampling is a simple zero-stuff + cascaded 4-pole Butterworth, with the
//! same filter used for both anti-image and anti-alias. This is the textbook
//! naive oversampler — easy to read, enough rolloff for the audio band at the
//! drive levels this effect lives at.
//!
//! References:
//!   - Chowdhury, "Real-Time Physical Modelling for Analog Tape Machines",
//!     DAFx 2019.
//!   - Jatin Chowdhury, CHOWTape (https://github.com/jatinchowdhury18/AnalogTapeModel).
//!   - Jiles & Atherton, "Theory of ferromagnetic hysteresis", JMMM 1986.

use crate::audiograph::NodeVTable;
use std::os::raw::{c_int, c_void};

const OVERSAMPLE: usize = 4;

// Parameter slots.
const STATE_ENABLED: usize = 0;
const STATE_DRIVE_DB: usize = 1;
const STATE_BIAS: usize = 2; // 0..1 — soft to hard saturation knee
const STATE_SPEED: usize = 3; // 0=7.5 ips, 1=15 ips, 2=30 ips
const STATE_OUTPUT_DB: usize = 4;
const STATE_MIX: usize = 5;
const STATE_SAMPLE_RATE: usize = 6;

// Per-channel hysteresis state: magnetization M and previous H (for ΔH).
const STATE_M_L: usize = 7;
const STATE_H_PREV_L: usize = 8;
const STATE_M_R: usize = 9;
const STATE_H_PREV_R: usize = 10;

// Oversampling filter state. Two cascaded biquads per stage (up + down) per
// channel. Each biquad has 4 floats (x1, x2, y1, y2). 2 biquads × 2 stages × 2
// channels × 4 floats = 32 floats.
const STATE_OS_BLOCK_OFFSET: usize = 11;
const STATE_OS_BLOCK_SIZE: usize = 32;

// Loss-filter state: one 2-pole LP biquad + one low-shelf biquad per channel.
// 2 biquads × 2 channels × 4 floats = 16 floats.
const STATE_LOSS_BLOCK_OFFSET: usize = STATE_OS_BLOCK_OFFSET + STATE_OS_BLOCK_SIZE;
const STATE_LOSS_BLOCK_SIZE: usize = 16;

pub const TAPE_STATE_SIZE: usize = STATE_LOSS_BLOCK_OFFSET + STATE_LOSS_BLOCK_SIZE;

pub const TAPE_PARAM_ENABLED: u64 = STATE_ENABLED as u64;
pub const TAPE_PARAM_DRIVE_DB: u64 = STATE_DRIVE_DB as u64;
pub const TAPE_PARAM_BIAS: u64 = STATE_BIAS as u64;
pub const TAPE_PARAM_SPEED: u64 = STATE_SPEED as u64;
pub const TAPE_PARAM_OUTPUT_DB: u64 = STATE_OUTPUT_DB as u64;
pub const TAPE_PARAM_MIX: u64 = STATE_MIX as u64;

#[inline]
fn db_to_amp(db: f32) -> f32 {
    (10.0_f32).powf(db / 20.0)
}

// ── Langevin function and derivative ──
//
//   L(x) = coth(x) − 1/x
//   L'(x) = 1/x² − csch²(x) = 1/x² − 1/sinh²(x)
//
// Both have a removable singularity at x = 0. We fall back to a Taylor series
// for small |x| to keep them well-behaved.

// Both `langevin` and `langevin_deriv` are reformulated to avoid the
// catastrophic cancellation in the naïve forms (`coth(x) − 1/x` and
// `1/x² − 1/sinh²(x)`). In f32 those subtract two large nearly-equal numbers
// over the audio range and lose most of their precision — the symptom is
// gritty/bit-crushy output even when the rest of the model is healthy.
//
// Series expansions:
//   x·cosh(x) − sinh(x) = x³/3 · (1 + x²/10 + …)
//   sinh²(x) − x²       = x⁴/3 · (1 + 2x²/15 + …)
//
// dividing through gives stable closed forms for both functions.

#[inline]
fn langevin(x: f32) -> f32 {
    if x.abs() < 1e-4 {
        x * (1.0 / 3.0 - x * x / 45.0)
    } else {
        let s = x.sinh();
        let x2 = x * x;
        x2 * (1.0 + x2 / 10.0) / (3.0 * s)
    }
}

#[inline]
fn langevin_deriv(x: f32) -> f32 {
    if x.abs() < 1e-4 {
        1.0 / 3.0 - x * x / 15.0
    } else {
        let s = x.sinh();
        let x2 = x * x;
        x2 * (1.0 + 2.0 * x2 / 15.0) / (3.0 * s * s)
    }
}

// ── Jiles–Atherton hysteresis step ──
//
// Material constants (in normalized units — Ms = 1):
//   a     — Langevin shape parameter (smaller = harder saturation)
//   alpha — interdomain coupling (small positive, ~1e-3)
//   k     — pinning loss (irreversibility)
//   c     — reversibility fraction (0 = fully irreversible)
//
// Returns the next magnetization M given the new H, previous H, current M, and
// the material constants. Forward Euler over the H increment.

#[allow(clippy::too_many_arguments)]
#[inline]
fn ja_step(h: f32, h_prev: f32, m: f32, a: f32, alpha: f32, k: f32, c: f32) -> f32 {
    let dh = h - h_prev;
    if dh == 0.0 {
        return m;
    }
    let delta = if dh > 0.0 { 1.0 } else { -1.0 };

    let he = h + alpha * m;
    let q = he / a.max(1e-6);

    let m_an = langevin(q);
    let dman_dhe = langevin_deriv(q) / a.max(1e-6);

    // dMirr/dH from the J-A relation. Two guards:
    //   - The "wing correction": if the field is moving in the opposite
    //     direction from (m_an − m), pin dMirr/dH to 0. Without this the
    //     irreversible magnetization can drift the wrong way at signal turning
    //     points, which sounds like crackle.
    //   - Skip near-zero denominator (we're on the anhysteretic curve).
    let diff = m_an - m;
    let denom_irr = delta * k * (1.0 - c) - alpha * diff;
    let dmirr_dh = if diff * delta <= 0.0 || denom_irr.abs() < 1e-9 {
        0.0
    } else {
        diff / denom_irr
    };

    let num = (1.0 - c) * dmirr_dh + c * dman_dhe;
    let den = 1.0 - alpha * num;
    let dm_dh = if den.abs() < 1e-9 { num } else { num / den };

    // Forward Euler — cheap, fine at 4× oversampling for our drive range.
    let m_new = m + dm_dh * dh;
    // Clamp to ±2 to keep things sane if the integrator goes wild.
    m_new.clamp(-2.0, 2.0)
}

// ── Biquad helper ──
//
// Direct-form-1 biquad. State is `[x1, x2, y1, y2]`.

#[inline]
unsafe fn biquad_process(coefs: &BiquadCoefs, state: *mut f32, x: f32) -> f32 {
    let x1 = *state.add(0);
    let x2 = *state.add(1);
    let y1 = *state.add(2);
    let y2 = *state.add(3);
    let y = coefs.b0 * x + coefs.b1 * x1 + coefs.b2 * x2 - coefs.a1 * y1 - coefs.a2 * y2;
    *state.add(0) = x;
    *state.add(1) = x1;
    *state.add(2) = y;
    *state.add(3) = y1;
    y
}

#[derive(Clone, Copy)]
struct BiquadCoefs {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
}

// 2nd-order Butterworth LP. fs is the sample rate the biquad will run at.
fn butterworth_lp(cutoff_hz: f32, fs: f32) -> BiquadCoefs {
    let omega = std::f32::consts::TAU * cutoff_hz.clamp(10.0, fs * 0.49) / fs;
    let cos_w = omega.cos();
    let sin_w = omega.sin();
    // Q = 1/√2 for Butterworth.
    let alpha = sin_w / (2.0 * std::f32::consts::FRAC_1_SQRT_2 * 2.0_f32.sqrt() * 0.5);
    // Equivalent to alpha = sin(ω)/√2, classic Butterworth.
    let alpha = sin_w * std::f32::consts::FRAC_1_SQRT_2;
    let _ = alpha; // silence unused-var warning from the intermediate
    let alpha = sin_w * std::f32::consts::FRAC_1_SQRT_2;
    let b0 = (1.0 - cos_w) * 0.5;
    let b1 = 1.0 - cos_w;
    let b2 = (1.0 - cos_w) * 0.5;
    let a0 = 1.0 + alpha;
    let a1 = -2.0 * cos_w;
    let a2 = 1.0 - alpha;
    BiquadCoefs {
        b0: b0 / a0,
        b1: b1 / a0,
        b2: b2 / a0,
        a1: a1 / a0,
        a2: a2 / a0,
    }
}

// Low-shelf biquad for the "head bump" — boost below cutoff by `gain_db`.
fn low_shelf(cutoff_hz: f32, gain_db: f32, fs: f32) -> BiquadCoefs {
    let a = (10.0_f32).powf(gain_db / 40.0);
    let omega = std::f32::consts::TAU * cutoff_hz.clamp(10.0, fs * 0.49) / fs;
    let cos_w = omega.cos();
    let sin_w = omega.sin();
    let s = 1.0; // shelf slope
    let alpha = sin_w * 0.5 * ((a + 1.0 / a) * (1.0 / s - 1.0) + 2.0).sqrt();
    let two_sqrt_a_alpha = 2.0 * a.sqrt() * alpha;
    let b0 = a * ((a + 1.0) - (a - 1.0) * cos_w + two_sqrt_a_alpha);
    let b1 = 2.0 * a * ((a - 1.0) - (a + 1.0) * cos_w);
    let b2 = a * ((a + 1.0) - (a - 1.0) * cos_w - two_sqrt_a_alpha);
    let a0 = (a + 1.0) + (a - 1.0) * cos_w + two_sqrt_a_alpha;
    let a1 = -2.0 * ((a - 1.0) + (a + 1.0) * cos_w);
    let a2 = (a + 1.0) + (a - 1.0) * cos_w - two_sqrt_a_alpha;
    BiquadCoefs {
        b0: b0 / a0,
        b1: b1 / a0,
        b2: b2 / a0,
        a1: a1 / a0,
        a2: a2 / a0,
    }
}

// Per-speed loss-filter characteristics (LP cutoff Hz, head-bump cutoff Hz,
// head-bump gain dB). Slower tape = darker + bigger low-mid resonance.
fn loss_params(speed_idx: usize) -> (f32, f32, f32) {
    match speed_idx.min(2) {
        0 => (14_000.0, 80.0, 1.4),  // 7.5 ips
        1 => (18_000.0, 50.0, 0.7),  // 15 ips
        _ => (21_000.0, 30.0, 0.3),  // 30 ips
    }
}

// `bias` models a tape machine's HF bias current. A real bias signal pushes
// the audio into the linear region of the magnetization curve, so:
//
//   under-biased (low)  → brighter, more distortion, edgier, less headroom
//   calibrated (~0.5)   → balanced
//   over-biased (high)  → darker, smoother, cleaner, compressed highs
//
// We capture the two audible dimensions: the saturation curve (here) and the
// HF brightness (see `bias_brightness`, applied to the loss-filter cutoff).
fn material_constants(bias: f32) -> (f32, f32, f32, f32) {
    let bias = bias.clamp(0.0, 1.0);
    // a: Langevin shape — smaller bends the curve sooner/harder. Under-biasing
    // gives a harder knee (distorts earlier, more harmonics).
    let a = 0.35 + bias * 0.85;
    // k: hysteresis loop width. Under-biasing widens the loop → more low-level
    // nonlinearity and "edge".
    let k = 0.9 - bias * 0.55;
    let alpha = 1.6e-3;
    let c = 0.17;
    (a, alpha, k, c)
}

// HF brightness multiplier applied to the loss-filter cutoff. Under-biasing
// leaves the highs intact (>1); over-biasing erases them (<1). This is the
// "back off the bias to brighten it up" behaviour of a real deck.
fn bias_brightness(bias: f32) -> f32 {
    1.3 - bias.clamp(0.0, 1.0) * 0.6
}

unsafe extern "C" fn tape_init(
    state: *mut c_void,
    sample_rate: c_int,
    _max_block: c_int,
    _initial_state: *const c_void,
) {
    let s = state as *mut f32;
    *s.add(STATE_ENABLED) = 1.0;
    *s.add(STATE_DRIVE_DB) = 0.0;
    *s.add(STATE_BIAS) = 0.5;
    *s.add(STATE_SPEED) = 1.0; // 15 ips default
    *s.add(STATE_OUTPUT_DB) = 0.0;
    *s.add(STATE_MIX) = 1.0;
    *s.add(STATE_SAMPLE_RATE) = sample_rate as f32;
    *s.add(STATE_M_L) = 0.0;
    *s.add(STATE_H_PREV_L) = 0.0;
    *s.add(STATE_M_R) = 0.0;
    *s.add(STATE_H_PREV_R) = 0.0;
    for i in 0..(STATE_OS_BLOCK_SIZE + STATE_LOSS_BLOCK_SIZE) {
        *s.add(STATE_OS_BLOCK_OFFSET + i) = 0.0;
    }
}

// Layout inside the OS block, per channel (16 floats each):
//   0..4:  upsample biquad 1 state
//   4..8:  upsample biquad 2 state
//   8..12: downsample biquad 1 state
//   12..16: downsample biquad 2 state
#[inline]
unsafe fn os_state_ptr(s: *mut f32, channel: usize) -> *mut f32 {
    s.add(STATE_OS_BLOCK_OFFSET + channel * 16)
}

// Loss block, per channel (8 floats each):
//   0..4:  LP biquad
//   4..8:  low-shelf biquad
#[inline]
unsafe fn loss_state_ptr(s: *mut f32, channel: usize) -> *mut f32 {
    s.add(STATE_LOSS_BLOCK_OFFSET + channel * 8)
}

#[allow(clippy::too_many_arguments)]
#[inline]
unsafe fn process_channel_sample(
    input: f32,
    drive: f32,
    a: f32,
    alpha: f32,
    k: f32,
    c: f32,
    os_lp: &BiquadCoefs,
    loss_lp: &BiquadCoefs,
    loss_shelf: &BiquadCoefs,
    m: &mut f32,
    h_prev: &mut f32,
    os_state: *mut f32,
    loss_state: *mut f32,
) -> f32 {
    // Drive into the magnetic field.
    let h_in = input * drive;

    // Upsample 4×: zero-stuff and cascade two anti-image LPs (gain ×4 to
    // restore the energy lost to the zero samples).
    let mut last = 0.0;
    for k_os in 0..OVERSAMPLE {
        let stuffed = if k_os == 0 { h_in * OVERSAMPLE as f32 } else { 0.0 };
        let y1 = biquad_process(os_lp, os_state.add(0), stuffed);
        let h_os = biquad_process(os_lp, os_state.add(4), y1);

        // Hysteresis step at the oversampled rate.
        *m = ja_step(h_os, *h_prev, *m, a, alpha, k, c);
        *h_prev = h_os;

        // Anti-alias downsample (the same biquads cascade), then only keep the
        // last sample of the 4-sample block.
        let d1 = biquad_process(os_lp, os_state.add(8), *m);
        let d2 = biquad_process(os_lp, os_state.add(12), d1);
        last = d2;
    }

    // Loss filter: HF rolloff + low-shelf head bump.
    let after_lp = biquad_process(loss_lp, loss_state.add(0), last);
    biquad_process(loss_shelf, loss_state.add(4), after_lp)
}

unsafe extern "C" fn tape_process(
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
    let out0 = *out.add(0);
    let out1 = *out.add(1);

    if *s.add(STATE_ENABLED) <= 0.5 {
        std::ptr::copy_nonoverlapping(in0 as *const f32, out0, nf);
        std::ptr::copy_nonoverlapping(in1 as *const f32, out1, nf);
        return;
    }

    let sr = (*s.add(STATE_SAMPLE_RATE)).max(1.0);
    let fs_os = sr * OVERSAMPLE as f32;
    let drive = db_to_amp((*s.add(STATE_DRIVE_DB)).clamp(-12.0, 24.0));
    let output = db_to_amp((*s.add(STATE_OUTPUT_DB)).clamp(-24.0, 12.0));
    let bias = (*s.add(STATE_BIAS)).clamp(0.0, 1.0);
    let mix = (*s.add(STATE_MIX)).clamp(0.0, 1.0);
    let speed_idx = (*s.add(STATE_SPEED)).round().clamp(0.0, 2.0) as usize;

    let (a, alpha, k, c) = material_constants(bias);
    let (loss_cut_hz, bump_hz, bump_db) = loss_params(speed_idx);

    // Anti-image/anti-alias cutoff at the base Nyquist (slightly under).
    let os_lp = butterworth_lp(sr * 0.45, fs_os);
    let loss_lp = butterworth_lp(loss_cut_hz * bias_brightness(bias), sr);
    let loss_shelf = low_shelf(bump_hz, bump_db, sr);

    let mut m_l = *s.add(STATE_M_L);
    let mut h_prev_l = *s.add(STATE_H_PREV_L);
    let mut m_r = *s.add(STATE_M_R);
    let mut h_prev_r = *s.add(STATE_H_PREV_R);
    let os_l = os_state_ptr(s, 0);
    let os_r = os_state_ptr(s, 1);
    let loss_l = loss_state_ptr(s, 0);
    let loss_r = loss_state_ptr(s, 1);

    for i in 0..nf {
        let dry_l = *in0.add(i);
        let dry_r = *in1.add(i);

        let wet_l = process_channel_sample(
            dry_l,
            drive,
            a,
            alpha,
            k,
            c,
            &os_lp,
            &loss_lp,
            &loss_shelf,
            &mut m_l,
            &mut h_prev_l,
            os_l,
            loss_l,
        ) * output;
        let wet_r = process_channel_sample(
            dry_r,
            drive,
            a,
            alpha,
            k,
            c,
            &os_lp,
            &loss_lp,
            &loss_shelf,
            &mut m_r,
            &mut h_prev_r,
            os_r,
            loss_r,
        ) * output;

        // Dry blend uses raw input — `drive` is purely a wet-path knob.
        *out0.add(i) = dry_l + (wet_l - dry_l) * mix;
        *out1.add(i) = dry_r + (wet_r - dry_r) * mix;
    }

    *s.add(STATE_M_L) = m_l;
    *s.add(STATE_H_PREV_L) = h_prev_l;
    *s.add(STATE_M_R) = m_r;
    *s.add(STATE_H_PREV_R) = h_prev_r;
}

pub fn tape_vtable() -> NodeVTable {
    NodeVTable {
        process: Some(tape_process),
        init: Some(tape_init),
        reset: None,
        migrate: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn init_state() -> [f32; TAPE_STATE_SIZE] {
        let mut state = [0.0; TAPE_STATE_SIZE];
        unsafe {
            tape_init(
                state.as_mut_ptr() as *mut c_void,
                48_000,
                512,
                std::ptr::null(),
            );
        }
        state
    }

    fn process_block(
        state: &mut [f32; TAPE_STATE_SIZE],
        left: &[f32],
        right: &[f32],
    ) -> (Vec<f32>, Vec<f32>) {
        let mut in_l = left.to_vec();
        let mut in_r = right.to_vec();
        let mut out_l = vec![0.0; left.len()];
        let mut out_r = vec![0.0; right.len()];
        let inputs = [in_l.as_mut_ptr(), in_r.as_mut_ptr()];
        let outputs = [out_l.as_mut_ptr(), out_r.as_mut_ptr()];
        unsafe {
            tape_process(
                inputs.as_ptr(),
                outputs.as_ptr(),
                left.len() as c_int,
                state.as_mut_ptr() as *mut c_void,
                std::ptr::null_mut(),
            );
        }
        (out_l, out_r)
    }

    fn rms(samples: &[f32]) -> f32 {
        (samples.iter().map(|x| x * x).sum::<f32>() / samples.len() as f32).sqrt()
    }

    #[test]
    fn bypass_copies_input_exactly() {
        let mut state = init_state();
        state[STATE_ENABLED] = 0.0;
        let left = vec![0.1, -0.2, 0.3, -0.4];
        let right = vec![-0.1, 0.2, -0.3, 0.4];
        let (out_l, out_r) = process_block(&mut state, &left, &right);
        assert_eq!(out_l, left);
        assert_eq!(out_r, right);
    }

    #[test]
    fn output_stays_finite_and_bounded_under_extreme_drive() {
        let mut state = init_state();
        state[STATE_DRIVE_DB] = 24.0;
        // bias=0 is the under-biased / hardest-knee end — most likely to blow up.
        state[STATE_BIAS] = 0.0;
        let sr = 48_000.0;
        let left: Vec<f32> = (0..2048)
            .map(|i| (std::f32::consts::TAU * 440.0 * i as f32 / sr).sin())
            .collect();
        let right = left.clone();
        let (out_l, out_r) = process_block(&mut state, &left, &right);
        for s in out_l.iter().chain(out_r.iter()) {
            assert!(s.is_finite(), "non-finite sample: {s}");
            assert!(s.abs() < 4.0, "runaway sample: {s}");
        }
    }

    #[test]
    fn hysteresis_introduces_harmonics_on_pure_sine() {
        // A linear effect can't add harmonic content. Compare the difference
        // between input and output — for a clean sine + nonlinear tape, the
        // residual should be non-trivial.
        let mut state = init_state();
        state[STATE_DRIVE_DB] = 12.0;
        let sr = 48_000.0;
        let left: Vec<f32> = (0..4096)
            .map(|i| (std::f32::consts::TAU * 220.0 * i as f32 / sr).sin() * 0.5)
            .collect();
        let right = left.clone();
        let (out_l, _) = process_block(&mut state, &left, &right);
        let residual: Vec<f32> = out_l[2048..]
            .iter()
            .zip(&left[2048..])
            .map(|(o, i)| o - i)
            .collect();
        let r = rms(&residual);
        assert!(r > 1.0e-3, "residual rms was {r}, expected harmonic content");
    }

    #[test]
    fn under_bias_passes_more_treble_than_over_bias() {
        // Backing off the bias should brighten the top end.
        let sr = 48_000.0;
        let left: Vec<f32> = (0..4096)
            .map(|i| (std::f32::consts::TAU * 10_000.0 * i as f32 / sr).sin() * 0.3)
            .collect();

        let mut under = init_state();
        under[STATE_BIAS] = 0.0;
        let (out_under, _) = process_block(&mut under, &left, &left);

        let mut over = init_state();
        over[STATE_BIAS] = 1.0;
        let (out_over, _) = process_block(&mut over, &left, &left);

        let r_under = rms(&out_under[2048..]);
        let r_over = rms(&out_over[2048..]);
        assert!(
            r_under > r_over,
            "under-biased rms {r_under} should exceed over-biased rms {r_over}"
        );
    }

    #[test]
    fn high_speed_passes_more_treble_than_low_speed() {
        // 30 ips should be brighter than 7.5 ips. Feed an 8 kHz sine through
        // both and check the slower setting attenuates more.
        let sr = 48_000.0;
        let left: Vec<f32> = (0..4096)
            .map(|i| (std::f32::consts::TAU * 8000.0 * i as f32 / sr).sin() * 0.3)
            .collect();

        let mut slow = init_state();
        slow[STATE_SPEED] = 0.0;
        let (out_slow, _) = process_block(&mut slow, &left, &left);

        let mut fast = init_state();
        fast[STATE_SPEED] = 2.0;
        let (out_fast, _) = process_block(&mut fast, &left, &left);

        let r_slow = rms(&out_slow[2048..]);
        let r_fast = rms(&out_fast[2048..]);
        assert!(
            r_fast > r_slow,
            "fast rms {r_fast} should exceed slow rms {r_slow}"
        );
    }
}
