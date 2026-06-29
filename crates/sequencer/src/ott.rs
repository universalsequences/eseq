// OTT-style 3-band upward+downward compressor.
//
// Signal path: input gain -> LR4 crossover split (low / mid / high, with the
// low band allpass-compensated at the upper crossover so the bands sum flat)
// -> per-band up/down compression toward a fixed target level -> per-band
// gain -> sum -> output gain, mixed against the dry input by `depth`.
use crate::audiograph::NodeVTable;
use std::os::raw::{c_int, c_void};

const STATE_ENABLED: usize = 0;
const STATE_DEPTH: usize = 1;
const STATE_TIME: usize = 2;
const STATE_INPUT_DB: usize = 3;
const STATE_OUTPUT_DB: usize = 4;
const STATE_UPWARD: usize = 5;
const STATE_DOWNWARD: usize = 6;
const STATE_LOW_GAIN_DB: usize = 7;
const STATE_MID_GAIN_DB: usize = 8;
const STATE_HIGH_GAIN_DB: usize = 9;
const STATE_XOVER_LOW_HZ: usize = 10;
const STATE_XOVER_HIGH_HZ: usize = 11;
const STATE_SAMPLE_RATE: usize = 12;
const STATE_ENV: usize = 13; // 3 slots (linked L/R envelope per band)
const STATE_GAIN_DB: usize = 16; // 3 slots (smoothed gain per band)
const STATE_FILTERS: usize = 19; // 2 channels x 10 biquads x 2 = 40 slots
pub const OTT_STATE_SIZE: usize = 59;

pub const OTT_PARAM_ENABLED: u64 = STATE_ENABLED as u64;
pub const OTT_PARAM_DEPTH: u64 = STATE_DEPTH as u64;
pub const OTT_PARAM_TIME: u64 = STATE_TIME as u64;
pub const OTT_PARAM_INPUT_DB: u64 = STATE_INPUT_DB as u64;
pub const OTT_PARAM_OUTPUT_DB: u64 = STATE_OUTPUT_DB as u64;
pub const OTT_PARAM_UPWARD: u64 = STATE_UPWARD as u64;
pub const OTT_PARAM_DOWNWARD: u64 = STATE_DOWNWARD as u64;
pub const OTT_PARAM_LOW_GAIN_DB: u64 = STATE_LOW_GAIN_DB as u64;
pub const OTT_PARAM_MID_GAIN_DB: u64 = STATE_MID_GAIN_DB as u64;
pub const OTT_PARAM_HIGH_GAIN_DB: u64 = STATE_HIGH_GAIN_DB as u64;
pub const OTT_PARAM_XOVER_LOW_HZ: u64 = STATE_XOVER_LOW_HZ as u64;
pub const OTT_PARAM_XOVER_HIGH_HZ: u64 = STATE_XOVER_HIGH_HZ as u64;

// Companding pivot per band; signals are squeezed toward this level.
const TARGET_DB: f32 = -30.0;
// Downward slope at full strength (ratio 8:1).
const DOWN_SLOPE: f32 = 0.875;
// Upward slope at full strength and the cap on upward gain.
const UP_SLOPE: f32 = 0.6;
const UP_MAX_DB: f32 = 24.0;
// Per-band detector times in ms at time = 1.0: (attack, release).
const BAND_TIMES: [(f32, f32); 3] = [(40.0, 300.0), (20.0, 200.0), (12.0, 120.0)];

#[inline]
fn db_to_amp(db: f32) -> f32 {
    (10.0_f32).powf(db / 20.0)
}

#[inline]
fn amp_to_db(amp: f32) -> f32 {
    20.0 * amp.max(1.0e-9).log10()
}

#[inline]
fn time_coef(ms: f32, sample_rate: f32) -> f32 {
    1.0 - (-1.0 / (ms.max(0.01) * 0.001 * sample_rate.max(1.0))).exp()
}

/// Biquad coefficients (a0-normalized) processed in transposed direct form II.
#[derive(Clone, Copy)]
struct Coefs {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
}

fn butterworth(freq: f32, sample_rate: f32, kind: u8) -> Coefs {
    let q = std::f32::consts::FRAC_1_SQRT_2;
    let w0 = 2.0 * std::f32::consts::PI * (freq / sample_rate).clamp(1.0e-5, 0.49);
    let (sin_w0, cos_w0) = w0.sin_cos();
    let alpha = sin_w0 / (2.0 * q);
    let a0 = 1.0 + alpha;
    let (b0, b1, b2) = match kind {
        0 => ((1.0 - cos_w0) * 0.5, 1.0 - cos_w0, (1.0 - cos_w0) * 0.5), // lowpass
        1 => ((1.0 + cos_w0) * 0.5, -(1.0 + cos_w0), (1.0 + cos_w0) * 0.5), // highpass
        _ => (1.0 - alpha, -2.0 * cos_w0, 1.0 + alpha),                  // allpass
    };
    Coefs {
        b0: b0 / a0,
        b1: b1 / a0,
        b2: b2 / a0,
        a1: -2.0 * cos_w0 / a0,
        a2: (1.0 - alpha) / a0,
    }
}

#[inline]
unsafe fn biquad(x: f32, c: &Coefs, z: *mut f32) -> f32 {
    let y = c.b0 * x + *z;
    *z = c.b1 * x - c.a1 * y + *z.add(1);
    *z.add(1) = c.b2 * x - c.a2 * y;
    y
}

/// Split one sample into (low, mid, high) bands. `z` points at this
/// channel's 10 biquads (20 f32): lp1 x2, ap2 x2, hp1 x2, lp2 x2, hp2 x2.
#[inline]
unsafe fn split_bands(x: f32, c: &[Coefs; 5], z: *mut f32) -> (f32, f32, f32) {
    let lo = biquad(biquad(x, &c[0], z), &c[0], z.add(2));
    let lo = biquad(biquad(lo, &c[4], z.add(4)), &c[4], z.add(6));
    let hi1 = biquad(biquad(x, &c[1], z.add(8)), &c[1], z.add(10));
    let mid = biquad(biquad(hi1, &c[2], z.add(12)), &c[2], z.add(14));
    let hi = biquad(biquad(hi1, &c[3], z.add(16)), &c[3], z.add(18));
    (lo, mid, hi)
}

unsafe extern "C" fn ott_init(
    state: *mut c_void,
    sample_rate: c_int,
    _max_block: c_int,
    _initial_state: *const c_void,
) {
    let s = state as *mut f32;
    std::ptr::write_bytes(s, 0, OTT_STATE_SIZE);
    *s.add(STATE_ENABLED) = 1.0;
    *s.add(STATE_DEPTH) = 0.5;
    *s.add(STATE_TIME) = 1.0;
    *s.add(STATE_INPUT_DB) = 0.0;
    *s.add(STATE_OUTPUT_DB) = 0.0;
    *s.add(STATE_UPWARD) = 1.0;
    *s.add(STATE_DOWNWARD) = 1.0;
    *s.add(STATE_LOW_GAIN_DB) = 0.0;
    *s.add(STATE_MID_GAIN_DB) = 0.0;
    *s.add(STATE_HIGH_GAIN_DB) = 0.0;
    *s.add(STATE_XOVER_LOW_HZ) = 100.0;
    *s.add(STATE_XOVER_HIGH_HZ) = 2500.0;
    *s.add(STATE_SAMPLE_RATE) = sample_rate as f32;
}

unsafe extern "C" fn ott_process(
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
        std::ptr::write_bytes(s.add(STATE_ENV), 0, OTT_STATE_SIZE - STATE_ENV);
        return;
    }

    let sr = *s.add(STATE_SAMPLE_RATE);
    let depth = (*s.add(STATE_DEPTH)).clamp(0.0, 1.0);
    let time = (*s.add(STATE_TIME)).clamp(0.1, 2.5);
    let input_gain = db_to_amp((*s.add(STATE_INPUT_DB)).clamp(-24.0, 24.0));
    let output_gain = db_to_amp((*s.add(STATE_OUTPUT_DB)).clamp(-24.0, 24.0));
    let upward = (*s.add(STATE_UPWARD)).clamp(0.0, 1.0);
    let downward = (*s.add(STATE_DOWNWARD)).clamp(0.0, 1.0);
    let band_gain = [
        db_to_amp((*s.add(STATE_LOW_GAIN_DB)).clamp(-24.0, 24.0)),
        db_to_amp((*s.add(STATE_MID_GAIN_DB)).clamp(-24.0, 24.0)),
        db_to_amp((*s.add(STATE_HIGH_GAIN_DB)).clamp(-24.0, 24.0)),
    ];
    let f_low = (*s.add(STATE_XOVER_LOW_HZ)).clamp(40.0, 400.0);
    let f_high = (*s.add(STATE_XOVER_HIGH_HZ))
        .clamp(1000.0, 8000.0)
        .max(f_low * 2.0);

    let coefs = [
        butterworth(f_low, sr, 0),  // lp @ low xover
        butterworth(f_low, sr, 1),  // hp @ low xover
        butterworth(f_high, sr, 0), // lp @ high xover
        butterworth(f_high, sr, 1), // hp @ high xover
        butterworth(f_high, sr, 2), // ap @ high xover (low-band phase comp)
    ];

    let mut attack = [0.0f32; 3];
    let mut release = [0.0f32; 3];
    for b in 0..3 {
        attack[b] = time_coef(BAND_TIMES[b].0 * time, sr);
        release[b] = time_coef(BAND_TIMES[b].1 * time, sr);
    }
    let gain_smooth = time_coef(8.0, sr);
    let down_strength = downward * DOWN_SLOPE;
    let up_strength = upward * UP_SLOPE;

    let mut env = [
        *s.add(STATE_ENV),
        *s.add(STATE_ENV + 1),
        *s.add(STATE_ENV + 2),
    ];
    let mut gain_db = [
        *s.add(STATE_GAIN_DB),
        *s.add(STATE_GAIN_DB + 1),
        *s.add(STATE_GAIN_DB + 2),
    ];
    let z0 = s.add(STATE_FILTERS);
    let z1 = s.add(STATE_FILTERS + 20);

    for i in 0..nf {
        let dry_l = *in0.add(i);
        let dry_r = *in1.add(i);
        let bands_l = split_bands(dry_l * input_gain, &coefs, z0);
        let bands_r = split_bands(dry_r * input_gain, &coefs, z1);
        let bands_l = [bands_l.0, bands_l.1, bands_l.2];
        let bands_r = [bands_r.0, bands_r.1, bands_r.2];

        let mut wet_l = 0.0;
        let mut wet_r = 0.0;
        for b in 0..3 {
            let detector = bands_l[b].abs().max(bands_r[b].abs());
            let coef = if detector > env[b] {
                attack[b]
            } else {
                release[b]
            };
            env[b] += coef * (detector - env[b]);
            let over_db = amp_to_db(env[b]) - TARGET_DB;
            let target_gain_db = if over_db > 0.0 {
                -over_db * down_strength
            } else {
                (-over_db * up_strength).min(UP_MAX_DB)
            };
            gain_db[b] += gain_smooth * (target_gain_db - gain_db[b]);
            let g = db_to_amp(gain_db[b]) * band_gain[b];
            wet_l += bands_l[b] * g;
            wet_r += bands_r[b] * g;
        }
        wet_l *= output_gain;
        wet_r *= output_gain;
        *out0.add(i) = dry_l + (wet_l - dry_l) * depth;
        *out1.add(i) = dry_r + (wet_r - dry_r) * depth;
    }

    for b in 0..3 {
        *s.add(STATE_ENV + b) = env[b];
        *s.add(STATE_GAIN_DB + b) = gain_db[b];
    }
}

pub fn ott_vtable() -> NodeVTable {
    NodeVTable {
        process: Some(ott_process),
        init: Some(ott_init),
        reset: None,
        migrate: None,
        ..NodeVTable::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: c_int = 48000;

    fn run(state: &mut [f32; OTT_STATE_SIZE], l: &mut [f32], r: &mut [f32]) {
        let nf = l.len();
        let mut out_l = vec![0.0f32; nf];
        let mut out_r = vec![0.0f32; nf];
        let inputs = [l.as_mut_ptr(), r.as_mut_ptr()];
        let outputs = [out_l.as_mut_ptr(), out_r.as_mut_ptr()];
        unsafe {
            ott_process(
                inputs.as_ptr(),
                outputs.as_ptr(),
                nf as c_int,
                state.as_mut_ptr() as *mut c_void,
                std::ptr::null_mut(),
            );
        }
        l.copy_from_slice(&out_l);
        r.copy_from_slice(&out_r);
    }

    fn init_state() -> [f32; OTT_STATE_SIZE] {
        let mut state = [0.0f32; OTT_STATE_SIZE];
        unsafe {
            ott_init(state.as_mut_ptr() as *mut c_void, SR, 512, std::ptr::null());
        }
        state
    }

    fn sine(freq: f32, amp: f32, n: usize) -> Vec<f32> {
        (0..n)
            .map(|i| amp * (2.0 * std::f32::consts::PI * freq * i as f32 / SR as f32).sin())
            .collect()
    }

    fn peak(buf: &[f32]) -> f32 {
        buf.iter().fold(0.0f32, |m, &x| m.max(x.abs()))
    }

    #[test]
    fn bypass_is_transparent() {
        let mut state = init_state();
        state[STATE_ENABLED] = 0.0;
        let src = sine(220.0, 0.5, 4096);
        let mut l = src.clone();
        let mut r = src.clone();
        run(&mut state, &mut l, &mut r);
        assert_eq!(l, src);
        assert_eq!(r, src);
    }

    #[test]
    fn quiet_signal_is_boosted_and_loud_signal_is_reduced() {
        // Quiet sine (-44 dB, below the -30 dB target) gets upward compression.
        let mut state = init_state();
        state[STATE_DEPTH] = 1.0;
        let quiet_amp = db_to_amp(-44.0);
        let mut l = sine(500.0, quiet_amp, SR as usize);
        let mut r = l.clone();
        run(&mut state, &mut l, &mut r);
        let tail = &l[l.len() - 4096..];
        assert!(
            peak(tail) > quiet_amp * 2.0,
            "expected upward boost, peak {} vs input {}",
            peak(tail),
            quiet_amp
        );

        // Loud sine (-6 dB, above target) gets downward compression.
        let mut state = init_state();
        state[STATE_DEPTH] = 1.0;
        let loud_amp = db_to_amp(-6.0);
        let mut l = sine(500.0, loud_amp, SR as usize);
        let mut r = l.clone();
        run(&mut state, &mut l, &mut r);
        let tail = &l[l.len() - 4096..];
        assert!(
            peak(tail) < loud_amp * 0.5,
            "expected downward reduction, peak {} vs input {}",
            peak(tail),
            loud_amp
        );
    }

    #[test]
    fn crossover_sums_flat_at_zero_depth_equivalent() {
        // With up/down at 0 the band gains are unity, so the wet path is just
        // the crossover split-and-sum; it should reconstruct the input closely.
        let mut state = init_state();
        state[STATE_DEPTH] = 1.0;
        state[STATE_UPWARD] = 0.0;
        state[STATE_DOWNWARD] = 0.0;
        for freq in [60.0, 250.0, 1000.0, 4000.0, 10000.0] {
            let src = sine(freq, 0.5, 16384);
            let mut l = src.clone();
            let mut r = src.clone();
            run(&mut state, &mut l, &mut r);
            let p = peak(&l[8192..]);
            assert!(
                (p - 0.5).abs() < 0.06,
                "crossover not flat at {freq} Hz: peak {p}"
            );
        }
    }
}
