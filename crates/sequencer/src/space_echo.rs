//! Roland RE-201 Space Echo emulation.
//!
//! Architecture: a mono echo loop (the real unit sums its inputs into a single
//! record head) feeding a fixed-rate-write tape buffer with three playback
//! heads read at speed-dependent delays. The REPEAT RATE control changes the
//! virtual tape speed, so head delays are `head_distance / speed`; smoothing
//! the speed glides all read positions through the buffer, which repitches
//! material already "on tape" exactly like the hardware's motor swoop.
//!
//! Inside the feedback loop: germanium-ish preamp clip on the input, a record
//! stage at 2× oversampling (HF pre-emphasis → tanh with bias asymmetry →
//! de-emphasis) so highs saturate first and self-oscillation blooms dark, a
//! playback loss lowpass + head-bump shelf, and bass/treble tone shelves.
//! Wow/flutter (flutter deepens with loop energy — motor strain), a periodic
//! tape-splice gain dip + thump, hiss and head crosstalk scale with "age".
//!
//! A two-tank dispersive spring reverb (see `crate::spring`: stretched
//! allpasses inside parallel feedback delays, tuned offline against a real
//! spring IR) hangs off the echo bus per the RE-201 mode selector.

use crate::audiograph::NodeVTable;
use crate::spring::{spring_tank_process, SpringCoeffs, SpringParams, SPRING_TANK_STATE_LEN};
use std::os::raw::{c_int, c_void};

const TAPE_BUF_LEN: usize = 131072;

// Head delays at nominal speed (speed_ratio = 1.0). Equidistant heads, so the
// repeats land at a 1:2:3 ratio like the hardware.
const HEAD_MS: [f32; 3] = [69.0, 138.0, 207.0];
// Tape speed range. At max speed head 1 reads ~50 ms, at min speed head 3
// reads ~510 ms — the documented RE-201 per-mode ranges.
const MIN_SPEED: f32 = 0.406;
const MAX_SPEED: f32 = 1.38;
// Virtual splice loop: 5.4 s of tape at nominal speed.
const SPLICE_LOOP_S: f32 = 5.4;
// Splice dip length in seconds-of-tape (4 ms at nominal speed).
const SPLICE_DIP_S: f32 = 0.004;

// ── Parameter slots ──
const STATE_ENABLED: usize = 0;
const STATE_MODE: usize = 1; // 0..11 mode selector
const STATE_RATE: usize = 2; // repeat rate knob 0..1 (free mode)
const STATE_SYNC: usize = 3;
const STATE_SYNC_DIV: usize = 4;
const STATE_SYNC_OFFSET: usize = 5;
const STATE_INTENSITY: usize = 6;
const STATE_BASS: usize = 7; // -1..1
const STATE_TREBLE: usize = 8; // -1..1
const STATE_ECHO_VOL: usize = 9;
const STATE_REVERB_VOL: usize = 10;
const STATE_DRY: usize = 11;
const STATE_INPUT_DB: usize = 12;
const STATE_WOW_FLUTTER: usize = 13;
const STATE_AGE: usize = 14;
const STATE_BPM: usize = 15;
const STATE_SAMPLE_RATE: usize = 16;

// Host-modulation depth slots (4 modulator slots × 4 targets).
const STATE_MOD_RATE_DEPTH_1: usize = 17;
const STATE_MOD_INTENSITY_DEPTH_1: usize = 21;
const STATE_MOD_ECHO_DEPTH_1: usize = 25;
const STATE_MOD_REVERB_DEPTH_1: usize = 29;

// ── Runtime state ──
const STATE_SMOOTH_SPEED: usize = 33;
const STATE_SMOOTH_INTENSITY: usize = 34;
const STATE_SMOOTH_ECHO: usize = 35;
const STATE_SMOOTH_REVERB: usize = 36;
const STATE_SMOOTH_BASS: usize = 37;
const STATE_SMOOTH_TREBLE: usize = 38;
const STATE_SMOOTH_DRY: usize = 39;
const STATE_WRITE_POS: usize = 40;
const STATE_WOW_PHASE: usize = 41;
const STATE_FLUT_PHASE1: usize = 42;
const STATE_FLUT_PHASE2: usize = 43;
const STATE_LOOP_ENV: usize = 44;
const STATE_SPLICE_PHASE: usize = 45;
const STATE_THUMP: usize = 46;
const STATE_NOISE_COUNTER: usize = 47;
const STATE_HISS_LP: usize = 48;
const STATE_PREAMP_DC_X1: usize = 49;
const STATE_PREAMP_DC_Y1: usize = 50;
const STATE_PRE_EMPH_LP: usize = 51;
const STATE_DE_EMPH_LP: usize = 52;
// 2× oversampling biquads (one up, one down), z1+z2 each.
const STATE_OS_UP_Z1: usize = 53;
const STATE_OS_UP_Z2: usize = 54;
const STATE_OS_DOWN_Z1: usize = 55;
const STATE_OS_DOWN_Z2: usize = 56;
const STATE_LOSS_Z1: usize = 57;
const STATE_LOSS_Z2: usize = 58;
const STATE_BUMP_LP: usize = 59;
const STATE_BUMP_SUB_LP: usize = 60;
const STATE_BASS_LP: usize = 61;
const STATE_TREBLE_LP: usize = 62;
const STATE_FB_PREV: usize = 63;
const STATE_LOOP_DC_X1: usize = 64;
const STATE_LOOP_DC_Y1: usize = 65;

// Spring tension macro (0..1, 0.5 = the tuned re201 fit) + its smoother.
const STATE_TENSION: usize = 66;
const STATE_SMOOTH_TENSION: usize = 67;

// Spring tank state: flat blocks owned by `crate::spring` (scalars + delay
// buffers per tank).
const STATE_SPRING_A: usize = 68;
const STATE_SPRING_B: usize = STATE_SPRING_A + SPRING_TANK_STATE_LEN;

const STATE_TAPE_OFFSET: usize = STATE_SPRING_B + SPRING_TANK_STATE_LEN;
const STATE_END: usize = STATE_TAPE_OFFSET + TAPE_BUF_LEN;

pub const SPACE_ECHO_STATE_SIZE: usize = STATE_END;

pub const SPACE_ECHO_PARAM_ENABLED: u64 = STATE_ENABLED as u64;
pub const SPACE_ECHO_PARAM_MODE: u64 = STATE_MODE as u64;
pub const SPACE_ECHO_PARAM_RATE: u64 = STATE_RATE as u64;
pub const SPACE_ECHO_PARAM_SYNC: u64 = STATE_SYNC as u64;
pub const SPACE_ECHO_PARAM_SYNC_DIV: u64 = STATE_SYNC_DIV as u64;
pub const SPACE_ECHO_PARAM_SYNC_OFFSET: u64 = STATE_SYNC_OFFSET as u64;
pub const SPACE_ECHO_PARAM_INTENSITY: u64 = STATE_INTENSITY as u64;
pub const SPACE_ECHO_PARAM_BASS: u64 = STATE_BASS as u64;
pub const SPACE_ECHO_PARAM_TREBLE: u64 = STATE_TREBLE as u64;
pub const SPACE_ECHO_PARAM_ECHO_VOL: u64 = STATE_ECHO_VOL as u64;
pub const SPACE_ECHO_PARAM_REVERB_VOL: u64 = STATE_REVERB_VOL as u64;
pub const SPACE_ECHO_PARAM_DRY: u64 = STATE_DRY as u64;
pub const SPACE_ECHO_PARAM_INPUT_DB: u64 = STATE_INPUT_DB as u64;
pub const SPACE_ECHO_PARAM_WOW_FLUTTER: u64 = STATE_WOW_FLUTTER as u64;
pub const SPACE_ECHO_PARAM_AGE: u64 = STATE_AGE as u64;
pub const SPACE_ECHO_PARAM_BPM: u64 = STATE_BPM as u64;
pub const SPACE_ECHO_PARAM_TENSION: u64 = STATE_TENSION as u64;
pub const SPACE_ECHO_PARAM_MOD_RATE_DEPTH_1: u64 = STATE_MOD_RATE_DEPTH_1 as u64;
pub const SPACE_ECHO_PARAM_MOD_RATE_DEPTH_2: u64 = STATE_MOD_RATE_DEPTH_1 as u64 + 1;
pub const SPACE_ECHO_PARAM_MOD_RATE_DEPTH_3: u64 = STATE_MOD_RATE_DEPTH_1 as u64 + 2;
pub const SPACE_ECHO_PARAM_MOD_RATE_DEPTH_4: u64 = STATE_MOD_RATE_DEPTH_1 as u64 + 3;
pub const SPACE_ECHO_PARAM_MOD_INTENSITY_DEPTH_1: u64 = STATE_MOD_INTENSITY_DEPTH_1 as u64;
pub const SPACE_ECHO_PARAM_MOD_INTENSITY_DEPTH_2: u64 = STATE_MOD_INTENSITY_DEPTH_1 as u64 + 1;
pub const SPACE_ECHO_PARAM_MOD_INTENSITY_DEPTH_3: u64 = STATE_MOD_INTENSITY_DEPTH_1 as u64 + 2;
pub const SPACE_ECHO_PARAM_MOD_INTENSITY_DEPTH_4: u64 = STATE_MOD_INTENSITY_DEPTH_1 as u64 + 3;
pub const SPACE_ECHO_PARAM_MOD_ECHO_DEPTH_1: u64 = STATE_MOD_ECHO_DEPTH_1 as u64;
pub const SPACE_ECHO_PARAM_MOD_ECHO_DEPTH_2: u64 = STATE_MOD_ECHO_DEPTH_1 as u64 + 1;
pub const SPACE_ECHO_PARAM_MOD_ECHO_DEPTH_3: u64 = STATE_MOD_ECHO_DEPTH_1 as u64 + 2;
pub const SPACE_ECHO_PARAM_MOD_ECHO_DEPTH_4: u64 = STATE_MOD_ECHO_DEPTH_1 as u64 + 3;
pub const SPACE_ECHO_PARAM_MOD_REVERB_DEPTH_1: u64 = STATE_MOD_REVERB_DEPTH_1 as u64;
pub const SPACE_ECHO_PARAM_MOD_REVERB_DEPTH_2: u64 = STATE_MOD_REVERB_DEPTH_1 as u64 + 1;
pub const SPACE_ECHO_PARAM_MOD_REVERB_DEPTH_3: u64 = STATE_MOD_REVERB_DEPTH_1 as u64 + 2;
pub const SPACE_ECHO_PARAM_MOD_REVERB_DEPTH_4: u64 = STATE_MOD_REVERB_DEPTH_1 as u64 + 3;

// Same division table as Str8 Delay so the UI labels match.
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

// Mode selector: (active head bitmask, reverb on). Positions 1-6 of the
// hardware are echo-only, 7-11 add reverb, 12 is reverb only.
const MODES: [(u8, bool); 12] = [
    (0b001, false),
    (0b010, false),
    (0b100, false),
    (0b011, false),
    (0b110, false),
    (0b111, false),
    (0b001, true),
    (0b010, true),
    (0b100, true),
    (0b011, true),
    (0b110, true),
    (0b000, true),
];

#[inline]
fn db_to_amp(db: f32) -> f32 {
    (10.0_f32).powf(db / 20.0)
}

#[inline]
fn synced_ms(div_idx: f32, offset: f32, bpm: f32) -> f32 {
    let idx = (div_idx.round() as usize).min(SYNC_BEATS.len() - 1);
    let beats = SYNC_BEATS[idx];
    let base_ms = beats * 60.0 / bpm.max(20.0) * 1000.0;
    base_ms * (1.0 + offset.clamp(-0.5, 0.5))
}

// Target tape speed ratio. Free mode maps the knob exponentially across the
// motor range (knob up = faster motor = shorter repeats). Sync mode picks the
// speed that puts head 2 on the requested division, octave-folded into the
// motor's reachable range.
fn target_speed(sync: f32, knob: f32, div: f32, offset: f32, bpm: f32) -> f32 {
    if sync > 0.5 {
        let ms = synced_ms(div, offset, bpm).max(1.0);
        let mut ratio = HEAD_MS[1] / ms;
        while ratio > MAX_SPEED {
            ratio *= 0.5;
        }
        while ratio < MIN_SPEED {
            ratio *= 2.0;
        }
        ratio.clamp(MIN_SPEED, MAX_SPEED)
    } else {
        MIN_SPEED * (MAX_SPEED / MIN_SPEED).powf(knob.clamp(0.0, 1.0))
    }
}

// INTENSITY knob → loop gain. Most of the throw stays below unity (long but
// decaying repeat trails); the loop only crosses into self-oscillation near
// the top (~0.9 knob) and peaks at a gentle 1.08, so runaway builds over
// seconds and hovers in the saturator's soft region instead of slamming
// straight into distortion.
#[inline]
fn intensity_gain(knob: f32) -> f32 {
    let k = knob.clamp(0.0, 1.0);
    if k < 0.75 {
        k / 0.75 * 0.72
    } else {
        0.72 + (k - 0.75) / 0.25 * 0.36
    }
}

// Germanium-flavoured preamp: soft asymmetric clip with a mild even-harmonic
// term. The DC introduced by the asymmetry is blocked downstream.
#[inline]
fn preamp_clip(x: f32) -> f32 {
    let x = x + 0.04;
    x * (1.0 + 0.12 * x * x) / (1.0 + 0.4 * x.abs() + x * x)
}

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
    let b0 = (1.0 - cos) * 0.5;
    let b1 = 1.0 - cos;
    let b2 = (1.0 - cos) * 0.5;
    let a0 = 1.0 + alpha;
    BiquadCoeffs {
        b0: b0 / a0,
        b1: b1 / a0,
        b2: b2 / a0,
        a1: (-2.0 * cos) / a0,
        a2: (1.0 - alpha) / a0,
    }
}

#[inline]
fn one_pole_coef(freq: f32, sr: f32) -> f32 {
    1.0 - (-std::f32::consts::TAU * freq / sr.max(1.0)).exp()
}

// 4-point Hermite read — the read heads glide continuously, so linear
// interpolation would dull the repeats noticeably during speed swoops.
#[inline]
unsafe fn tape_read(buf: *const f32, wpos: usize, delay: f32) -> f32 {
    let d = delay.clamp(2.0, (TAPE_BUF_LEN - 4) as f32);
    let read = wpos as f32 - d + TAPE_BUF_LEN as f32;
    let base = read.floor();
    let frac = read - base;
    let i0 = (base as usize + TAPE_BUF_LEN - 1) % TAPE_BUF_LEN;
    let xm1 = *buf.add(i0);
    let x0 = *buf.add((i0 + 1) % TAPE_BUF_LEN);
    let x1 = *buf.add((i0 + 2) % TAPE_BUF_LEN);
    let x2 = *buf.add((i0 + 3) % TAPE_BUF_LEN);
    let c1 = 0.5 * (x1 - xm1);
    let c2 = xm1 - 2.5 * x0 + 2.0 * x1 - 0.5 * x2;
    let c3 = 0.5 * (x2 - xm1) + 1.5 * (x0 - x1);
    ((c3 * frac + c2) * frac + c1) * frac + x0
}

// Counter-based white noise (Wellons lowbias32 finalizer) — see tape.rs.
#[inline]
fn hash_noise(mut x: u32) -> f32 {
    x ^= x >> 16;
    x = x.wrapping_mul(0x7feb_352d);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846c_a68b);
    x ^= x >> 16;
    ((x >> 8) as f32 / (1u32 << 24) as f32) * 2.0 - 1.0
}

const NOISE_COUNTER_WRAP: f32 = 8_388_608.0;

// ── Spring tanks ──
//
// The dispersive spring lives in `crate::spring` (stretched-allpass loops,
// parameters fitted offline to spring_reverb_impulse.wav). Tank B is a
// detuned copy of tank A so the cross-mixed stereo output decorrelates.
const SPRING_B_DETUNE: f32 = 1.035;
// Output calibration so the new tanks sit at the same loudness as the old
// spring did at REVERB_VOL = 0.5 (impulse-response RMS match over 3 s).
const SPRING_OUT_GAIN: f32 = 0.788;

fn spring_tank_b_params(base: &SpringParams) -> SpringParams {
    let mut p = base.clone();
    for d in p.d_loop.iter_mut() {
        *d *= SPRING_B_DETUNE;
    }
    p.d_df1 *= 0.97;
    p
}

unsafe extern "C" fn space_echo_init(
    state: *mut c_void,
    sample_rate: c_int,
    _max_block: c_int,
    _initial_state: *const c_void,
) {
    let s = state as *mut f32;
    std::ptr::write_bytes(s, 0, SPACE_ECHO_STATE_SIZE);
    *s.add(STATE_ENABLED) = 1.0;
    *s.add(STATE_MODE) = 7.0; // head 2 + reverb
    *s.add(STATE_RATE) = 0.5;
    *s.add(STATE_SYNC) = 0.0;
    *s.add(STATE_SYNC_DIV) = 6.0;
    *s.add(STATE_SYNC_OFFSET) = 0.0;
    *s.add(STATE_INTENSITY) = 0.45;
    *s.add(STATE_BASS) = 0.0;
    *s.add(STATE_TREBLE) = 0.0;
    *s.add(STATE_ECHO_VOL) = 0.8;
    *s.add(STATE_REVERB_VOL) = 0.5;
    *s.add(STATE_DRY) = 1.0;
    *s.add(STATE_INPUT_DB) = 0.0;
    *s.add(STATE_WOW_FLUTTER) = 0.35;
    *s.add(STATE_AGE) = 0.3;
    *s.add(STATE_BPM) = 120.0;
    *s.add(STATE_SAMPLE_RATE) = sample_rate as f32;
    *s.add(STATE_TENSION) = 0.5;
    *s.add(STATE_SMOOTH_TENSION) = 0.5;
    let speed = target_speed(0.0, 0.5, 6.0, 0.0, 120.0);
    *s.add(STATE_SMOOTH_SPEED) = speed;
    *s.add(STATE_SMOOTH_INTENSITY) = intensity_gain(0.45);
    *s.add(STATE_SMOOTH_ECHO) = 0.8;
    *s.add(STATE_SMOOTH_REVERB) = 0.5;
    *s.add(STATE_SMOOTH_DRY) = 1.0;
}

unsafe extern "C" fn space_echo_process(
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
    let bpm = *s.add(STATE_BPM);
    let mode_idx = (*s.add(STATE_MODE)).round().clamp(0.0, 11.0) as usize;
    let (head_mask, reverb_on) = MODES[mode_idx];
    let rate_knob = (*s.add(STATE_RATE)).clamp(0.0, 1.0);
    let sync = *s.add(STATE_SYNC);
    let sync_div = *s.add(STATE_SYNC_DIV);
    let sync_offset = *s.add(STATE_SYNC_OFFSET);
    let intensity_knob = (*s.add(STATE_INTENSITY)).clamp(0.0, 1.0);
    let bass = (*s.add(STATE_BASS)).clamp(-1.0, 1.0);
    let treble = (*s.add(STATE_TREBLE)).clamp(-1.0, 1.0);
    let echo_vol = (*s.add(STATE_ECHO_VOL)).clamp(0.0, 1.5);
    let reverb_vol = if reverb_on {
        (*s.add(STATE_REVERB_VOL)).clamp(0.0, 1.5)
    } else {
        0.0
    };
    let dry_level = (*s.add(STATE_DRY)).clamp(0.0, 1.0);
    let input_gain = db_to_amp((*s.add(STATE_INPUT_DB)).clamp(-12.0, 24.0));
    let wf_amt = (*s.add(STATE_WOW_FLUTTER)).clamp(0.0, 1.0);
    let age = (*s.add(STATE_AGE)).clamp(0.0, 1.0);

    let mod_rate_depths = [
        *s.add(STATE_MOD_RATE_DEPTH_1),
        *s.add(STATE_MOD_RATE_DEPTH_1 + 1),
        *s.add(STATE_MOD_RATE_DEPTH_1 + 2),
        *s.add(STATE_MOD_RATE_DEPTH_1 + 3),
    ];
    let mod_intensity_depths = [
        *s.add(STATE_MOD_INTENSITY_DEPTH_1),
        *s.add(STATE_MOD_INTENSITY_DEPTH_1 + 1),
        *s.add(STATE_MOD_INTENSITY_DEPTH_1 + 2),
        *s.add(STATE_MOD_INTENSITY_DEPTH_1 + 3),
    ];
    let mod_echo_depths = [
        *s.add(STATE_MOD_ECHO_DEPTH_1),
        *s.add(STATE_MOD_ECHO_DEPTH_1 + 1),
        *s.add(STATE_MOD_ECHO_DEPTH_1 + 2),
        *s.add(STATE_MOD_ECHO_DEPTH_1 + 3),
    ];
    let mod_reverb_depths = [
        *s.add(STATE_MOD_REVERB_DEPTH_1),
        *s.add(STATE_MOD_REVERB_DEPTH_1 + 1),
        *s.add(STATE_MOD_REVERB_DEPTH_1 + 2),
        *s.add(STATE_MOD_REVERB_DEPTH_1 + 3),
    ];
    let rate_mod_active = mod_rate_depths.iter().any(|d| d.abs() > 1e-6);

    // Per-block coefficients. The loss cutoff tracks tape speed: slower tape
    // loses more highs, so cranking REPEAT RATE down also darkens the repeats.
    let mut smooth_speed = (*s.add(STATE_SMOOTH_SPEED)).clamp(MIN_SPEED, MAX_SPEED);
    let base_target_speed = target_speed(sync, rate_knob, sync_div, sync_offset, bpm);
    let loss_lp = lowpass_coeffs(6500.0 * smooth_speed.powf(0.7), sr);
    let fs_os = sr * 2.0;
    let os_lp = lowpass_coeffs(sr * 0.45, fs_os);
    // First-order shelves built from one-pole lowpasses.
    let emph_coef = one_pole_coef(3000.0, fs_os);
    let bump_coef = one_pole_coef(70.0, sr);
    let bump_sub_coef = one_pole_coef(25.0, sr);
    let bump_gain = db_to_amp(2.0) - 1.0;
    let bass_coef = one_pole_coef(250.0, sr);
    let bass_gain = db_to_amp(bass * 8.0) - 1.0;
    let treble_coef = one_pole_coef(2200.0, sr);
    let treble_gain = db_to_amp(treble * 8.0) - 1.0;

    // Motor inertia (~120 ms) for the speed glide; 20 Hz for level params.
    let speed_smooth = 1.0 - (-1.0 / (0.12 * sr)).exp();
    let param_smooth = one_pole_coef(20.0, sr);
    let env_coef = one_pole_coef(5.0, sr);

    // Wow/flutter (transport modulation, shared by all heads).
    let wow_depth = wf_amt * 3.0 * 0.001 * sr;
    let flutter_depth_base = wf_amt * 0.3 * 0.001 * sr;
    let wow_inc = 0.55 * smooth_speed / sr;
    let flut_inc1 = 5.8 / sr;
    let flut_inc2 = 10.4 / sr;

    // Splice geometry in tape-time.
    let splice_inc_per_speed = 1.0 / (SPLICE_LOOP_S * sr);
    let splice_dip_width = SPLICE_DIP_S / SPLICE_LOOP_S;
    let splice_depth = 0.15 + 0.5 * age;
    let thump_amp = 0.03 * age;
    let thump_decay = 1.0 - one_pole_coef(120.0, sr);

    let hiss_gain = age * age * 0.015;
    let hiss_coef = one_pole_coef(9000.0, sr);
    let crosstalk = 0.01 * (0.3 + 0.7 * age);
    let dc_r = (-std::f32::consts::TAU * 10.0 / sr).exp();

    // Spring coefficients (per-block: a handful of transcendentals). The
    // tension knob is smoothed across blocks so coefficient steps stay small.
    let tension_knob = (*s.add(STATE_TENSION)).clamp(0.0, 1.0);
    let mut smooth_tension = *s.add(STATE_SMOOTH_TENSION);
    smooth_tension += 0.1 * (tension_knob - smooth_tension);
    *s.add(STATE_SMOOTH_TENSION) = smooth_tension;
    let spring_params = SpringParams::re201().with_tension(smooth_tension);
    let spring_coef_a = SpringCoeffs::new(&spring_params, sr);
    let spring_coef_b = SpringCoeffs::new(&spring_tank_b_params(&spring_params), sr);

    let mut smooth_intensity = *s.add(STATE_SMOOTH_INTENSITY);
    let mut smooth_echo = *s.add(STATE_SMOOTH_ECHO);
    let mut smooth_reverb = *s.add(STATE_SMOOTH_REVERB);
    let mut smooth_bass = *s.add(STATE_SMOOTH_BASS);
    let mut smooth_treble = *s.add(STATE_SMOOTH_TREBLE);
    let mut smooth_dry = *s.add(STATE_SMOOTH_DRY);
    let mut wpos = (*s.add(STATE_WRITE_POS)) as usize % TAPE_BUF_LEN;
    let mut wow_phase = *s.add(STATE_WOW_PHASE);
    let mut flut_phase1 = *s.add(STATE_FLUT_PHASE1);
    let mut flut_phase2 = *s.add(STATE_FLUT_PHASE2);
    let mut loop_env = *s.add(STATE_LOOP_ENV);
    let mut splice_phase = *s.add(STATE_SPLICE_PHASE);
    let mut thump = *s.add(STATE_THUMP);
    let mut noise_counter = *s.add(STATE_NOISE_COUNTER);
    let mut hiss_lp = *s.add(STATE_HISS_LP);
    let mut preamp_dc_x1 = *s.add(STATE_PREAMP_DC_X1);
    let mut preamp_dc_y1 = *s.add(STATE_PREAMP_DC_Y1);
    let mut pre_emph_lp = *s.add(STATE_PRE_EMPH_LP);
    let mut de_emph_lp = *s.add(STATE_DE_EMPH_LP);
    let mut os_up_z1 = *s.add(STATE_OS_UP_Z1);
    let mut os_up_z2 = *s.add(STATE_OS_UP_Z2);
    let mut os_down_z1 = *s.add(STATE_OS_DOWN_Z1);
    let mut os_down_z2 = *s.add(STATE_OS_DOWN_Z2);
    let mut loss_z1 = *s.add(STATE_LOSS_Z1);
    let mut loss_z2 = *s.add(STATE_LOSS_Z2);
    let mut bump_lp = *s.add(STATE_BUMP_LP);
    let mut bump_sub_lp = *s.add(STATE_BUMP_SUB_LP);
    let mut bass_lp = *s.add(STATE_BASS_LP);
    let mut treble_lp = *s.add(STATE_TREBLE_LP);
    let mut fb_prev = *s.add(STATE_FB_PREV);
    let mut loop_dc_x1 = *s.add(STATE_LOOP_DC_X1);
    let mut loop_dc_y1 = *s.add(STATE_LOOP_DC_Y1);

    let tape = s.add(STATE_TAPE_OFFSET);
    let spring_a_state = std::slice::from_raw_parts_mut(s.add(STATE_SPRING_A), SPRING_TANK_STATE_LEN);
    let spring_b_state = std::slice::from_raw_parts_mut(s.add(STATE_SPRING_B), SPRING_TANK_STATE_LEN);

    let target_intensity = intensity_gain(intensity_knob);
    let fb_norm = 1.0 / (head_mask.count_ones().max(1)) as f32;

    for i in 0..nf {
        // ── Parameter smoothing + host modulation ──
        let intensity_mod = mod_inputs
            .iter()
            .zip(mod_intensity_depths)
            .map(|(input, depth)| (*input.add(i)).clamp(0.0, 1.0) * depth)
            .sum::<f32>();
        let echo_mod = mod_inputs
            .iter()
            .zip(mod_echo_depths)
            .map(|(input, depth)| (*input.add(i)).clamp(0.0, 1.0) * depth)
            .sum::<f32>();
        let reverb_mod = mod_inputs
            .iter()
            .zip(mod_reverb_depths)
            .map(|(input, depth)| (*input.add(i)).clamp(0.0, 1.0) * depth)
            .sum::<f32>();
        smooth_intensity += param_smooth * (target_intensity - smooth_intensity);
        smooth_echo += param_smooth * (echo_vol - smooth_echo);
        smooth_reverb += param_smooth * (reverb_vol - smooth_reverb);
        smooth_bass += param_smooth * (bass_gain - smooth_bass);
        smooth_treble += param_smooth * (treble_gain - smooth_treble);
        smooth_dry += param_smooth * (dry_level - smooth_dry);
        let loop_gain =
            (smooth_intensity + intensity_mod.clamp(-1.0, 1.0) * 1.15).clamp(0.0, 1.15);

        // Rate modulation goes through the same motor smoother, so modulating
        // it produces tape-style pitch warble rather than zipper noise.
        let speed_target = if rate_mod_active {
            let rate_mod = mod_inputs
                .iter()
                .zip(mod_rate_depths)
                .map(|(input, depth)| (*input.add(i)).clamp(0.0, 1.0) * depth)
                .sum::<f32>();
            target_speed(sync, rate_knob + rate_mod, sync_div, sync_offset, bpm)
        } else {
            base_target_speed
        };
        smooth_speed += speed_smooth * (speed_target - smooth_speed);

        // ── Input path ──
        let dry_l = *in0.add(i);
        let dry_r = *in1.add(i);
        let mono = 0.5 * (dry_l + dry_r) * input_gain;
        let pre = preamp_clip(mono);
        // DC block (the asymmetric clip rides on a small offset).
        let pre = {
            let y = pre - preamp_dc_x1 + dc_r * preamp_dc_y1;
            preamp_dc_x1 = pre;
            preamp_dc_y1 = y;
            y
        };

        // Hiss is injected into the loop so it regenerates with intensity.
        let cbits = noise_counter as u32;
        let noise = hash_noise(cbits ^ 0x5EAC_E101);
        hiss_lp += hiss_coef * (noise - hiss_lp);
        noise_counter += 1.0;
        if noise_counter >= NOISE_COUNTER_WRAP {
            noise_counter = 0.0;
        }

        // Splice thump (decaying low-frequency bump injected into the loop).
        thump *= thump_decay;
        let loop_in = pre + loop_gain * fb_prev + hiss_lp * hiss_gain + thump;

        // ── Record stage at 2× oversampling ──
        // Pre-emphasis (+6 dB above 3 kHz) → tanh with even-order asymmetry →
        // de-emphasis. HF saturates first, so runaway feedback blooms dark.
        let mut rec = 0.0;
        for k_os in 0..2 {
            let stuffed = if k_os == 0 { loop_in * 2.0 } else { 0.0 };
            let up = biquad_sample(stuffed, os_lp, &mut os_up_z1, &mut os_up_z2);
            pre_emph_lp += emph_coef * (up - pre_emph_lp);
            let emphasized = up + 1.0 * (up - pre_emph_lp); // +6 dB high shelf
            let shaped = emphasized + 0.10 * emphasized * emphasized;
            let saturated = (shaped * 1.1).tanh() / 1.1;
            de_emph_lp += emph_coef * (saturated - de_emph_lp);
            let deemphasized = saturated - 0.5 * (saturated - de_emph_lp); // -6 dB shelf
            rec = biquad_sample(deemphasized, os_lp, &mut os_down_z1, &mut os_down_z2);
        }
        // In-loop DC block keeps the asymmetric term from accumulating.
        let rec = {
            let y = rec - loop_dc_x1 + dc_r * loop_dc_y1;
            loop_dc_x1 = rec;
            loop_dc_y1 = y;
            y
        };

        // ── Splice gain dip ──
        splice_phase += splice_inc_per_speed * smooth_speed;
        if splice_phase >= 1.0 {
            splice_phase -= 1.0;
            thump += thump_amp;
        }
        let rec = if splice_phase < splice_dip_width {
            let w = splice_phase / splice_dip_width;
            let dip = 0.5 * (1.0 - (std::f32::consts::TAU * w).cos());
            rec * (1.0 - splice_depth * dip)
        } else {
            rec
        };

        *tape.add(wpos) = rec;

        // ── Playback heads ──
        loop_env += env_coef * (fb_prev.abs() - loop_env);
        let wow_lfo = (std::f32::consts::TAU * wow_phase).sin();
        let flut_lfo = 0.6 * (std::f32::consts::TAU * flut_phase1).sin()
            + 0.4 * (std::f32::consts::TAU * flut_phase2).sin();
        let flutter_depth = flutter_depth_base * (1.0 + 2.0 * loop_env.min(1.0));
        let transport_mod = wow_depth * wow_lfo + flutter_depth * flut_lfo;
        wow_phase = (wow_phase + wow_inc).fract();
        flut_phase1 = (flut_phase1 + flut_inc1).fract();
        flut_phase2 = (flut_phase2 + flut_inc2).fract();

        let mut heads = [0.0f32; 3];
        for (h, head) in heads.iter_mut().enumerate() {
            if head_mask & (1 << h) != 0 {
                let delay = HEAD_MS[h] / smooth_speed * 0.001 * sr + transport_mod;
                *head = tape_read(tape as *const f32, wpos, delay);
            }
        }
        // Adjacent-head crosstalk (active heads bleed into each other's sum
        // only matters audibly when one neighbor is muted, so read it cheap:
        // bleed from the tape even for inactive neighbors).
        let mut echo_sum = 0.0;
        for h in 0..3 {
            if head_mask & (1 << h) != 0 {
                let mut v = heads[h];
                if h > 0 {
                    let delay = HEAD_MS[h - 1] / smooth_speed * 0.001 * sr + transport_mod;
                    v += crosstalk * tape_read(tape as *const f32, wpos, delay);
                }
                if h < 2 {
                    let delay = HEAD_MS[h + 1] / smooth_speed * 0.001 * sr + transport_mod;
                    v += crosstalk * tape_read(tape as *const f32, wpos, delay);
                }
                echo_sum += v;
            }
        }

        // ── Playback EQ + tone (inside the loop: repeats darken cumulatively) ──
        let after_loss = biquad_sample(echo_sum, loss_lp, &mut loss_z1, &mut loss_z2);
        bump_lp += bump_coef * (after_loss - bump_lp);
        bump_sub_lp += bump_sub_coef * (bump_lp - bump_sub_lp);
        let after_bump = after_loss + bump_gain * (bump_lp - bump_sub_lp);
        bass_lp += bass_coef * (after_bump - bass_lp);
        let after_bass = after_bump + smooth_bass * bass_lp;
        treble_lp += treble_coef * (after_bass - treble_lp);
        let echo = after_bass + smooth_treble * (after_bass - treble_lp);
        // Normalize the feedback tap by head count: the output keeps the full
        // multi-head sum, but the loop sees unity total gain, so 1+2 modes
        // regenerate at the same rate as a single head instead of 2× hotter.
        fb_prev = echo * fb_norm;

        // ── Spring reverb (fed from preamp + echo, per the RE-201 bus) ──
        let (tank_a, tank_b) = if reverb_on {
            let spring_in = (pre + echo) * 0.7;
            (
                SPRING_OUT_GAIN * spring_tank_process(spring_in, &spring_coef_a, spring_a_state),
                SPRING_OUT_GAIN * spring_tank_process(spring_in, &spring_coef_b, spring_b_state),
            )
        } else {
            (0.0, 0.0)
        };

        let echo_out = echo * (smooth_echo + echo_mod).clamp(0.0, 1.5);
        let rev_gain = (smooth_reverb + reverb_mod).clamp(0.0, 1.5);
        *out0.add(i) = dry_l * smooth_dry + echo_out + (0.85 * tank_a + 0.35 * tank_b) * rev_gain;
        *out1.add(i) = dry_r * smooth_dry + echo_out + (0.35 * tank_a + 0.85 * tank_b) * rev_gain;

        wpos = (wpos + 1) % TAPE_BUF_LEN;
    }

    *s.add(STATE_SMOOTH_SPEED) = smooth_speed;
    *s.add(STATE_SMOOTH_INTENSITY) = smooth_intensity;
    *s.add(STATE_SMOOTH_ECHO) = smooth_echo;
    *s.add(STATE_SMOOTH_REVERB) = smooth_reverb;
    *s.add(STATE_SMOOTH_BASS) = smooth_bass;
    *s.add(STATE_SMOOTH_TREBLE) = smooth_treble;
    *s.add(STATE_SMOOTH_DRY) = smooth_dry;
    *s.add(STATE_WRITE_POS) = wpos as f32;
    *s.add(STATE_WOW_PHASE) = wow_phase;
    *s.add(STATE_FLUT_PHASE1) = flut_phase1;
    *s.add(STATE_FLUT_PHASE2) = flut_phase2;
    *s.add(STATE_LOOP_ENV) = loop_env;
    *s.add(STATE_SPLICE_PHASE) = splice_phase;
    *s.add(STATE_THUMP) = thump;
    *s.add(STATE_NOISE_COUNTER) = noise_counter;
    *s.add(STATE_HISS_LP) = hiss_lp;
    *s.add(STATE_PREAMP_DC_X1) = preamp_dc_x1;
    *s.add(STATE_PREAMP_DC_Y1) = preamp_dc_y1;
    *s.add(STATE_PRE_EMPH_LP) = pre_emph_lp;
    *s.add(STATE_DE_EMPH_LP) = de_emph_lp;
    *s.add(STATE_OS_UP_Z1) = os_up_z1;
    *s.add(STATE_OS_UP_Z2) = os_up_z2;
    *s.add(STATE_OS_DOWN_Z1) = os_down_z1;
    *s.add(STATE_OS_DOWN_Z2) = os_down_z2;
    *s.add(STATE_LOSS_Z1) = loss_z1;
    *s.add(STATE_LOSS_Z2) = loss_z2;
    *s.add(STATE_BUMP_LP) = bump_lp;
    *s.add(STATE_BUMP_SUB_LP) = bump_sub_lp;
    *s.add(STATE_BASS_LP) = bass_lp;
    *s.add(STATE_TREBLE_LP) = treble_lp;
    *s.add(STATE_FB_PREV) = fb_prev;
    *s.add(STATE_LOOP_DC_X1) = loop_dc_x1;
    *s.add(STATE_LOOP_DC_Y1) = loop_dc_y1;
}

pub fn space_echo_vtable() -> NodeVTable {
    NodeVTable {
        process: Some(space_echo_process),
        init: Some(space_echo_init),
        reset: None,
        migrate: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: i32 = 48_000;

    fn init_state() -> Vec<f32> {
        let mut state = vec![0.0f32; SPACE_ECHO_STATE_SIZE];
        unsafe {
            space_echo_init(
                state.as_mut_ptr().cast::<c_void>(),
                SR,
                512,
                std::ptr::null(),
            );
        }
        state
    }

    fn render_with_mods(
        state: &mut [f32],
        left: &[f32],
        right: &[f32],
        mod1: f32,
    ) -> (Vec<f32>, Vec<f32>) {
        let frames = left.len();
        let mut in_l = left.to_vec();
        let mut in_r = right.to_vec();
        let mut m1 = vec![mod1; frames];
        let mut m2 = vec![0.0; frames];
        let mut m3 = vec![0.0; frames];
        let mut m4 = vec![0.0; frames];
        let inputs = [
            in_l.as_mut_ptr(),
            in_r.as_mut_ptr(),
            m1.as_mut_ptr(),
            m2.as_mut_ptr(),
            m3.as_mut_ptr(),
            m4.as_mut_ptr(),
        ];
        let mut out_l = vec![0.0; frames];
        let mut out_r = vec![0.0; frames];
        let outputs = [out_l.as_mut_ptr(), out_r.as_mut_ptr()];
        unsafe {
            space_echo_process(
                inputs.as_ptr(),
                outputs.as_ptr(),
                frames as c_int,
                state.as_mut_ptr().cast::<c_void>(),
                std::ptr::null_mut(),
            );
        }
        (out_l, out_r)
    }

    fn render(state: &mut [f32], left: &[f32], right: &[f32]) -> (Vec<f32>, Vec<f32>) {
        render_with_mods(state, left, right, 0.0)
    }

    fn rms(samples: &[f32]) -> f32 {
        (samples.iter().map(|x| x * x).sum::<f32>() / samples.len() as f32).sqrt()
    }

    // Quiet-test baseline: zero out character noise sources.
    fn clean_state() -> Vec<f32> {
        let mut state = init_state();
        state[STATE_AGE] = 0.0;
        state[STATE_WOW_FLUTTER] = 0.0;
        state
    }

    fn impulse(frames: usize) -> Vec<f32> {
        let mut v = vec![0.0; frames];
        v[0] = 1.0;
        v
    }

    fn sine(freq: f32, amp: f32, frames: usize) -> Vec<f32> {
        (0..frames)
            .map(|i| amp * (std::f32::consts::TAU * freq * i as f32 / SR as f32).sin())
            .collect()
    }

    // Local peaks above a threshold, at least `min_gap` apart.
    fn echo_peaks(samples: &[f32], threshold: f32, min_gap: usize) -> Vec<usize> {
        let mut peaks = Vec::new();
        let mut i = 1;
        while i < samples.len() - 1 {
            let a = samples[i].abs();
            if a > threshold && a >= samples[i - 1].abs() && a >= samples[i + 1].abs() {
                if peaks.last().map_or(true, |&p: &usize| i - p > min_gap) {
                    peaks.push(i);
                    i += min_gap;
                    continue;
                }
            }
            i += 1;
        }
        peaks
    }

    #[test]
    fn bypass_copies_input_exactly() {
        let mut state = init_state();
        state[STATE_ENABLED] = 0.0;
        let left = vec![0.1, -0.2, 0.3, -0.4];
        let right = vec![-0.1, 0.2, -0.3, 0.4];
        let (out_l, out_r) = render(&mut state, &left, &right);
        assert_eq!(out_l, left);
        assert_eq!(out_r, right);
    }

    #[test]
    fn head_delays_follow_the_1_2_3_ratio() {
        // Mode 6 (all heads, echo only is index 5), no feedback, dry muted.
        let mut state = clean_state();
        state[STATE_MODE] = 5.0;
        state[STATE_INTENSITY] = 0.0;
        state[STATE_SMOOTH_INTENSITY] = 0.0;
        state[STATE_DRY] = 0.0;
        state[STATE_SMOOTH_DRY] = 0.0;
        // Pin speed to nominal: knob for ratio 1.0 in the exponential map.
        let knob = (1.0 / MIN_SPEED).ln() / (MAX_SPEED / MIN_SPEED).ln();
        state[STATE_RATE] = knob;
        state[STATE_SMOOTH_SPEED] = 1.0;

        let frames = SR as usize / 2 + 4096;
        let (out_l, _) = render(&mut state, &impulse(frames), &impulse(frames));
        let peaks = echo_peaks(&out_l, 0.02, 800);
        assert!(peaks.len() >= 3, "expected 3 head echoes, got {peaks:?}");
        let expected = [69.0, 138.0, 207.0];
        for (peak, ms) in peaks.iter().zip(expected) {
            let got_ms = *peak as f32 * 1000.0 / SR as f32;
            assert!(
                (got_ms - ms).abs() < 4.0,
                "head echo at {got_ms} ms, expected {ms} ms (peaks {peaks:?})"
            );
        }
    }

    #[test]
    fn repeat_rate_scales_all_heads_together() {
        let run = |knob: f32| {
            let mut state = clean_state();
            state[STATE_MODE] = 5.0;
            state[STATE_INTENSITY] = 0.0;
            state[STATE_SMOOTH_INTENSITY] = 0.0;
            state[STATE_DRY] = 0.0;
            state[STATE_SMOOTH_DRY] = 0.0;
            state[STATE_RATE] = knob;
            state[STATE_SMOOTH_SPEED] = target_speed(0.0, knob, 6.0, 0.0, 120.0);
            let frames = SR as usize + 8192;
            let (out_l, _) = render(&mut state, &impulse(frames), &impulse(frames));
            echo_peaks(&out_l, 0.02, 800)
        };
        let slow = run(0.1);
        let fast = run(0.9);
        assert!(slow.len() >= 3 && fast.len() >= 3);
        let ratio = slow[0] as f32 / fast[0] as f32;
        for h in 0..3 {
            let r = slow[h] as f32 / fast[h] as f32;
            assert!(
                (r / ratio - 1.0).abs() < 0.05,
                "head {h} scaled by {r}, expected ~{ratio}"
            );
        }
        assert!(ratio > 1.5, "slow/fast delay ratio {ratio} too small");
    }

    #[test]
    fn sync_puts_head_two_on_the_division() {
        // 1/4 @ 120 BPM = 500 ms. Head 2 (138 ms) needs speed 0.276, folded
        // up one octave to 0.552 → head-2 delay 250 ms (the folded division).
        let mut state = clean_state();
        state[STATE_MODE] = 1.0; // head 2 only
        state[STATE_INTENSITY] = 0.0;
        state[STATE_SMOOTH_INTENSITY] = 0.0;
        state[STATE_DRY] = 0.0;
        state[STATE_SMOOTH_DRY] = 0.0;
        state[STATE_SYNC] = 1.0;
        state[STATE_SYNC_DIV] = 6.0; // 1/4
        let expected_speed = target_speed(1.0, 0.5, 6.0, 0.0, 120.0);
        state[STATE_SMOOTH_SPEED] = expected_speed;
        let expected_ms = HEAD_MS[1] / expected_speed;

        let frames = SR as usize;
        let (out_l, _) = render(&mut state, &impulse(frames), &impulse(frames));
        let peaks = echo_peaks(&out_l, 0.02, 800);
        assert!(!peaks.is_empty(), "no echo found");
        let got_ms = peaks[0] as f32 * 1000.0 / SR as f32;
        assert!(
            (got_ms - expected_ms).abs() / expected_ms < 0.01,
            "synced echo at {got_ms} ms, expected {expected_ms} ms"
        );
        // And the folded division must be beat-related: 500 / 2^k.
        let division_ms = 500.0 / 2.0_f32.powf((500.0 / expected_ms).log2().round());
        assert!(
            (expected_ms - division_ms).abs() < 1.0,
            "folded delay {expected_ms} not on a /2 grid of 500 ms"
        );
    }

    #[test]
    fn self_oscillation_sustains_and_stays_bounded() {
        let mut state = init_state();
        state[STATE_MODE] = 1.0;
        state[STATE_INTENSITY] = 1.0;
        state[STATE_DRY] = 0.0;

        // Excite with a short burst, then run on silence.
        let burst = sine(660.0, 0.8, 4096);
        let _ = render(&mut state, &burst, &burst);
        let silence = vec![0.0; SR as usize * 5];
        let (out_l, out_r) = render(&mut state, &silence, &silence);
        for v in out_l.iter().chain(out_r.iter()) {
            assert!(v.is_finite(), "non-finite sample: {v}");
            assert!(v.abs() < 4.0, "runaway sample: {v}");
        }
        let tail = &out_l[out_l.len() - SR as usize..];
        assert!(
            rms(tail) > 0.01,
            "self-oscillation died out, tail rms {}",
            rms(tail)
        );
    }

    #[test]
    fn multi_head_modes_decay_like_single_head_modes() {
        // Below the self-oscillation threshold every mode must decay; before
        // feedback normalization the 1+2 / 2+3 modes regenerated 2× hotter
        // and ran away where single heads decayed.
        let tail_rms = |mode: f32| {
            let mut state = clean_state();
            state[STATE_MODE] = mode;
            state[STATE_INTENSITY] = 0.8; // loop gain ~0.97, decaying
            state[STATE_SMOOTH_INTENSITY] = intensity_gain(0.8);
            state[STATE_DRY] = 0.0;
            state[STATE_SMOOTH_DRY] = 0.0;
            let burst = sine(660.0, 0.5, 4096);
            let _ = render(&mut state, &burst, &burst);
            let silence = vec![0.0; SR as usize * 4];
            let (out_l, _) = render(&mut state, &silence, &silence);
            for v in &out_l {
                assert!(v.is_finite(), "mode {mode}: non-finite sample");
                assert!(v.abs() < 4.0, "mode {mode}: runaway sample {v}");
            }
            rms(&out_l[out_l.len() - SR as usize..])
        };
        let single = tail_rms(0.0);
        let pair = tail_rms(3.0);
        let all = tail_rms(5.0);
        assert!(single < 0.05, "single head should decay, tail {single}");
        assert!(pair < 0.05, "1+2 mode should decay, tail {pair}");
        assert!(all < 0.05, "all-heads mode should decay, tail {all}");
    }

    #[test]
    fn repeats_get_progressively_darker() {
        // Broadband click through head 1 with feedback; later repeats should
        // have less high-frequency content relative to lows.
        let mut state = clean_state();
        state[STATE_MODE] = 0.0;
        state[STATE_INTENSITY] = 0.6;
        state[STATE_SMOOTH_INTENSITY] = intensity_gain(0.6);
        state[STATE_DRY] = 0.0;
        state[STATE_SMOOTH_DRY] = 0.0;
        state[STATE_SMOOTH_SPEED] = 1.0;
        let knob = (1.0 / MIN_SPEED).ln() / (MAX_SPEED / MIN_SPEED).ln();
        state[STATE_RATE] = knob;

        let frames = SR as usize;
        let (out_l, _) = render(&mut state, &impulse(frames), &impulse(frames));

        // Repeat windows around 69 ms and 3×69 ms.
        let window = |center_ms: f32| {
            let c = (center_ms * SR as f32 / 1000.0) as usize;
            &out_l[c - 600..c + 600]
        };
        let tilt = |w: &[f32]| {
            // Crude spectral tilt: RMS of the first difference (HF proxy)
            // over RMS of the signal (broadband).
            let d: Vec<f32> = w.windows(2).map(|p| p[1] - p[0]).collect();
            rms(&d) / rms(w).max(1e-9)
        };
        let first = tilt(window(69.0));
        let third = tilt(window(207.0));
        assert!(
            third < first * 0.97,
            "third repeat tilt {third} should be darker than first {first}"
        );
    }

    #[test]
    fn speed_change_glides_instead_of_jumping() {
        // Echoes of a sine should pass through intermediate delays when the
        // rate knob moves — the read heads glide, so the echo of a constant
        // tone shifts pitch rather than crossfading.
        let mut state = clean_state();
        state[STATE_MODE] = 0.0;
        state[STATE_INTENSITY] = 0.0;
        state[STATE_SMOOTH_INTENSITY] = 0.0;
        state[STATE_DRY] = 0.0;
        state[STATE_SMOOTH_DRY] = 0.0;
        state[STATE_RATE] = 0.8;
        state[STATE_SMOOTH_SPEED] = target_speed(0.0, 0.8, 6.0, 0.0, 120.0);

        // Feed a steady tone so the tape is full of it.
        let tone = sine(1000.0, 0.5, SR as usize);
        let _ = render(&mut state, &tone, &tone);
        // Drop the speed sharply and keep feeding the tone; during the glide
        // the echo is read faster/slower than written → frequency shifts.
        state[STATE_RATE] = 0.2;
        let tone2 = sine(1000.0, 0.5, SR as usize / 2);
        let (out_l, _) = render(&mut state, &tone2, &tone2);

        // Measure dominant frequency early in the glide via zero crossings.
        let seg = &out_l[2000..10000];
        let crossings = seg.windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0).count();
        let freq = crossings as f32 * SR as f32 / seg.len() as f32;
        assert!(
            (freq - 1000.0).abs() > 30.0,
            "echo frequency {freq} should be shifted during the speed glide"
        );
    }

    #[test]
    fn spring_reverb_rings_disperses_and_decays() {
        let mut state = clean_state();
        state[STATE_MODE] = 11.0; // reverb only
        state[STATE_DRY] = 0.0;
        state[STATE_SMOOTH_DRY] = 0.0;
        state[STATE_REVERB_VOL] = 1.0;
        state[STATE_SMOOTH_REVERB] = 1.0;

        let frames = SR as usize * 5;
        let (out_l, _) = render(&mut state, &impulse(frames), &impulse(frames));

        let early = rms(&out_l[2400..4800]); // ~50-100 ms
        let late = rms(&out_l[SR as usize..SR as usize + 4800]); // ~1 s
        assert!(early > 1.0e-4, "spring produced no early energy: {early}");
        assert!(late < early, "spring should decay: early {early} late {late}");
        let tail = rms(&out_l[frames - 4800..]);
        assert!(tail < 1.0e-3, "spring tail did not die out: {tail}");

        // Dispersion: a low-frequency band's energy should arrive later than
        // a high band's. Compare centroid-in-time of band-filtered outputs.
        let band_centroid = |freq: f32| {
            let mut z1 = 0.0;
            let mut z2 = 0.0;
            let coefs = lowpass_coeffs(freq, SR as f32);
            let mut num = 0.0;
            let mut den = 0.0;
            for (n, &x) in out_l[..SR as usize / 4].iter().enumerate() {
                let y = biquad_sample(x, coefs, &mut z1, &mut z2);
                let e = y * y;
                num += n as f32 * e;
                den += e;
            }
            num / den.max(1e-12)
        };
        let low_centroid = band_centroid(400.0);
        let broadband_centroid = band_centroid(8000.0);
        assert!(
            low_centroid > broadband_centroid,
            "spring chirp: low band centroid {low_centroid} should lag broadband {broadband_centroid}"
        );
    }

    #[test]
    fn reverb_is_muted_in_echo_only_modes() {
        let mut state = clean_state();
        state[STATE_MODE] = 0.0; // head 1, no reverb
        state[STATE_DRY] = 0.0;
        state[STATE_SMOOTH_DRY] = 0.0;
        state[STATE_ECHO_VOL] = 0.0;
        state[STATE_SMOOTH_ECHO] = 0.0;
        state[STATE_REVERB_VOL] = 1.5;
        state[STATE_SMOOTH_REVERB] = 0.0;
        let frames = SR as usize;
        let (out_l, _) = render(&mut state, &impulse(frames), &impulse(frames));
        assert!(
            rms(&out_l) < 1.0e-5,
            "echo-only mode leaked reverb: rms {}",
            rms(&out_l)
        );
    }

    #[test]
    fn splice_thump_is_periodic_with_age() {
        let mut state = init_state();
        state[STATE_MODE] = 0.0;
        state[STATE_AGE] = 1.0;
        state[STATE_WOW_FLUTTER] = 0.0;
        state[STATE_INTENSITY] = 0.0;
        state[STATE_DRY] = 0.0;
        state[STATE_ECHO_VOL] = 1.5;
        state[STATE_SMOOTH_SPEED] = 1.0;
        let knob = (1.0 / MIN_SPEED).ln() / (MAX_SPEED / MIN_SPEED).ln();
        state[STATE_RATE] = knob;

        // Silence in: only hiss + thumps come out. The thump is low-frequency,
        // the hiss broadband — isolate it with a 120 Hz lowpass before taking
        // the envelope, and skip the startup transient. The thump period at
        // speed 1.0 is 5.4 s (wraps land at ~5.4 s and ~10.8 s).
        let frames = SR as usize * 12;
        let silence = vec![0.0; frames];
        let (out_l, _) = render(&mut state, &silence, &silence);
        let lp = lowpass_coeffs(120.0, SR as f32);
        let mut z1 = 0.0;
        let mut z2 = 0.0;
        let mut env = 0.0;
        let coef = one_pole_coef(30.0, SR as f32);
        let envelope: Vec<f32> = out_l
            .iter()
            .map(|x| {
                let y = biquad_sample(*x, lp, &mut z1, &mut z2);
                env += coef * (y.abs() - env);
                env
            })
            .collect();
        // Thumps wrap at 5.4 s and 10.8 s (plus the ~69 ms head delay). Find
        // the envelope argmax in a window around each and check the spacing.
        let argmax_in = |from_s: f32, to_s: f32| {
            let a = (from_s * SR as f32) as usize;
            let b = (to_s * SR as f32) as usize;
            let (idx, peak) = envelope[a..b]
                .iter()
                .enumerate()
                .max_by(|x, y| x.1.total_cmp(y.1))
                .unwrap();
            (a + idx, *peak)
        };
        let (first, first_peak) = argmax_in(4.0, 7.0);
        let (second, second_peak) = argmax_in(9.4, 12.0);
        let noise_floor = envelope[(2.0 * SR as f32) as usize..(4.0 * SR as f32) as usize]
            .iter()
            .cloned()
            .fold(0.0, f32::max);
        assert!(
            first_peak > noise_floor * 1.5 && second_peak > noise_floor * 1.5,
            "thumps ({first_peak}, {second_peak}) should clear the hiss floor {noise_floor}"
        );
        let period_s = (second - first) as f32 / SR as f32;
        assert!(
            (period_s - 5.4).abs() < 0.3,
            "splice period {period_s} s, expected ~5.4 s"
        );
    }

    #[test]
    fn modulation_inputs_affect_intensity_and_volumes() {
        // Intensity modulation adds repeat energy.
        let mut state = clean_state();
        state[STATE_MODE] = 0.0;
        state[STATE_INTENSITY] = 0.0;
        state[STATE_SMOOTH_INTENSITY] = 0.0;
        state[STATE_DRY] = 0.0;
        state[STATE_SMOOTH_DRY] = 0.0;
        state[STATE_MOD_INTENSITY_DEPTH_1] = 0.8;
        let frames = SR as usize;
        let (base, _) = render(&mut state.clone(), &impulse(frames), &impulse(frames));
        let (modded, _) = render_with_mods(&mut state, &impulse(frames), &impulse(frames), 1.0);
        let tail_start = SR as usize / 4;
        let base_tail: f32 = base[tail_start..].iter().map(|x| x.abs()).sum();
        let mod_tail: f32 = modded[tail_start..].iter().map(|x| x.abs()).sum();
        assert!(
            mod_tail > base_tail + 0.05,
            "intensity modulation should add repeats: {mod_tail} vs {base_tail}"
        );

        // Echo volume modulation changes output level.
        let mut state = clean_state();
        state[STATE_MODE] = 0.0;
        state[STATE_INTENSITY] = 0.0;
        state[STATE_SMOOTH_INTENSITY] = 0.0;
        state[STATE_DRY] = 0.0;
        state[STATE_SMOOTH_DRY] = 0.0;
        state[STATE_ECHO_VOL] = 0.2;
        state[STATE_SMOOTH_ECHO] = 0.2;
        state[STATE_MOD_ECHO_DEPTH_1] = 1.0;
        let (base, _) = render(&mut state.clone(), &impulse(frames), &impulse(frames));
        let (modded, _) = render_with_mods(&mut state, &impulse(frames), &impulse(frames), 1.0);
        assert!(
            rms(&modded) > rms(&base) * 2.0,
            "echo volume modulation should boost output"
        );

        // Rate modulation moves the echo earlier (mod pushes knob up = faster).
        let mut state = clean_state();
        state[STATE_MODE] = 0.0;
        state[STATE_INTENSITY] = 0.0;
        state[STATE_SMOOTH_INTENSITY] = 0.0;
        state[STATE_DRY] = 0.0;
        state[STATE_SMOOTH_DRY] = 0.0;
        state[STATE_RATE] = 0.3;
        state[STATE_SMOOTH_SPEED] = target_speed(0.0, 0.3, 6.0, 0.0, 120.0);
        state[STATE_MOD_RATE_DEPTH_1] = 0.6;
        let (base, _) = render(&mut state.clone(), &impulse(frames), &impulse(frames));
        let mut state2 = state.clone();
        // With rate mod the smoothed speed glides up, so the echo lands sooner.
        state2[STATE_SMOOTH_SPEED] = target_speed(0.0, 0.9, 6.0, 0.0, 120.0);
        let (modded, _) = render_with_mods(&mut state2, &impulse(frames), &impulse(frames), 1.0);
        let first_peak = |buf: &[f32]| echo_peaks(buf, 0.02, 800).first().copied().unwrap_or(0);
        assert!(
            first_peak(&modded) + 1000 < first_peak(&base),
            "rate modulation should shorten the echo delay"
        );
    }

    #[test]
    fn everything_maxed_stays_finite() {
        let mut state = init_state();
        state[STATE_MODE] = 9.0; // heads 1+2 + reverb
        state[STATE_INTENSITY] = 1.0;
        state[STATE_INPUT_DB] = 24.0;
        state[STATE_WOW_FLUTTER] = 1.0;
        state[STATE_AGE] = 1.0;
        state[STATE_ECHO_VOL] = 1.5;
        state[STATE_REVERB_VOL] = 1.5;
        state[STATE_BASS] = 1.0;
        state[STATE_TREBLE] = 1.0;
        let drive = sine(660.0, 1.0, SR as usize * 2);
        let (out_l, out_r) = render(&mut state, &drive, &drive);
        for v in out_l.iter().chain(out_r.iter()) {
            assert!(v.is_finite(), "non-finite sample: {v}");
            assert!(v.abs() < 16.0, "runaway sample: {v}");
        }
    }
}
