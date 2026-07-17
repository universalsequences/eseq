// OTT: 3-band multiband dynamics (Ableton Multiband Dynamics-style).
//
// Signal path: LR4 crossover split (low / mid / high, with the low band
// allpass-compensated at the upper crossover so the bands sum flat) ->
// per-band input gain -> per-band above/below gain computer (threshold +
// ratio each way, soft knee, peak or RMS detector, per-band attack/release
// scaled by the global time knob) -> per-band output gain -> band on/solo
// routing -> sum -> global output gain. `amount` scales the computed
// dynamics gain (100% = full compression, 0% = crossover only).
//
// The tail of the state array carries display meters (per-band L/R level dB
// and applied gain dB) that the UI polls via the node-state watchlist.
use crate::audiograph::NodeVTable;
use std::os::raw::{c_int, c_void};

const STATE_ENABLED: usize = 0;
const STATE_AMOUNT: usize = 1;
const STATE_TIME: usize = 2;
const STATE_OUTPUT_DB: usize = 3;
const STATE_SOFT_KNEE: usize = 4;
const STATE_RMS: usize = 5;
const STATE_SPLIT_LOW: usize = 6;
const STATE_SPLIT_HIGH: usize = 7;
const STATE_XOVER_LOW_HZ: usize = 8;
const STATE_XOVER_HIGH_HZ: usize = 9;

// Per-band parameter block: bands are low = 0, mid = 1, high = 2.
const STATE_BAND_BASE: usize = 10;
const BAND_STRIDE: usize = 10;
const BAND_BELOW_THR_DB: usize = 0;
const BAND_BELOW_RATIO: usize = 1;
const BAND_ABOVE_THR_DB: usize = 2;
const BAND_ABOVE_RATIO: usize = 3;
const BAND_ATTACK_MS: usize = 4;
const BAND_RELEASE_MS: usize = 5;
const BAND_INPUT_DB: usize = 6;
const BAND_OUTPUT_DB: usize = 7;
const BAND_ON: usize = 8;
const BAND_SOLO: usize = 9;

const STATE_SAMPLE_RATE: usize = 40;
const STATE_ENV: usize = 41; // 3 slots (linked L/R detector per band)
const STATE_GAIN_DB: usize = 44; // 3 slots (smoothed dynamics gain per band)
const STATE_FILTERS: usize = 47; // 2 channels x 10 biquads x 2 = 40 slots
pub const STATE_METER_LEVEL_DB: usize = 87; // 6 slots: (L, R) per band, low..high
pub const STATE_METER_GAIN_DB: usize = 93; // 3 slots: applied gain per band
pub const OTT_STATE_SIZE: usize = 96;

pub const OTT_PARAM_ENABLED: u64 = STATE_ENABLED as u64;
pub const OTT_PARAM_AMOUNT: u64 = STATE_AMOUNT as u64;
pub const OTT_PARAM_TIME: u64 = STATE_TIME as u64;
pub const OTT_PARAM_OUTPUT_DB: u64 = STATE_OUTPUT_DB as u64;
pub const OTT_PARAM_SOFT_KNEE: u64 = STATE_SOFT_KNEE as u64;
pub const OTT_PARAM_RMS: u64 = STATE_RMS as u64;
pub const OTT_PARAM_SPLIT_LOW: u64 = STATE_SPLIT_LOW as u64;
pub const OTT_PARAM_SPLIT_HIGH: u64 = STATE_SPLIT_HIGH as u64;
pub const OTT_PARAM_XOVER_LOW_HZ: u64 = STATE_XOVER_LOW_HZ as u64;
pub const OTT_PARAM_XOVER_HIGH_HZ: u64 = STATE_XOVER_HIGH_HZ as u64;

pub const fn ott_band_param(band: usize, field: OttBandField) -> u64 {
    (STATE_BAND_BASE + band * BAND_STRIDE + field as usize) as u64
}

#[derive(Clone, Copy)]
pub enum OttBandField {
    BelowThreshold = BAND_BELOW_THR_DB as isize,
    BelowRatio = BAND_BELOW_RATIO as isize,
    AboveThreshold = BAND_ABOVE_THR_DB as isize,
    AboveRatio = BAND_ABOVE_RATIO as isize,
    Attack = BAND_ATTACK_MS as isize,
    Release = BAND_RELEASE_MS as isize,
    Input = BAND_INPUT_DB as isize,
    Output = BAND_OUTPUT_DB as isize,
    On = BAND_ON as isize,
    Solo = BAND_SOLO as isize,
}

// Detector floor and dynamics gain limits. The floor keeps upward ratios
// from computing unbounded boost on digital silence; the boost cap matches
// the Ableton device's ceiling.
const LEVEL_FLOOR_DB: f32 = -80.0;
const GAIN_MIN_DB: f32 = -60.0;
const GAIN_MAX_DB: f32 = 36.0;
const KNEE_WIDTH_DB: f32 = 6.0;

// Per-band defaults (low, mid, high) matching the Ableton device.
pub const BAND_DEFAULT_ATTACK_MS: [f32; 3] = [50.0, 10.0, 5.0];
pub const BAND_DEFAULT_RELEASE_MS: [f32; 3] = [300.0, 200.0, 100.0];
pub const DEFAULT_BELOW_THR_DB: f32 = -60.0;
pub const DEFAULT_ABOVE_THR_DB: f32 = -20.0;

// Display meter ballistics (amplitude domain, fixed).
const METER_ATTACK_MS: f32 = 5.0;
const METER_RELEASE_MS: f32 = 250.0;

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

/// One side of the gain computer. `over` is how far the level has crossed
/// the threshold into the active region (positive = active); the return is
/// `over * slope` outside the knee with a quadratic transition inside it.
#[inline]
fn knee_gain(over: f32, slope: f32, knee: f32) -> f32 {
    if knee <= 0.0 {
        return if over > 0.0 { over * slope } else { 0.0 };
    }
    let half = knee * 0.5;
    if over <= -half {
        0.0
    } else if over >= half {
        over * slope
    } else {
        let t = over + half;
        slope * t * t / (2.0 * knee)
    }
}

/// Dynamics gain in dB for one band given the detector level in dB.
/// Above: level > threshold compresses/expands with slope (1/R - 1).
/// Below: level < threshold boosts/cuts with slope (1 - 1/R).
#[inline]
fn dynamics_gain_db(
    level_db: f32,
    below_thr: f32,
    below_ratio: f32,
    above_thr: f32,
    above_ratio: f32,
    knee: f32,
) -> f32 {
    let above = knee_gain(level_db - above_thr, 1.0 / above_ratio - 1.0, knee);
    let below = knee_gain(below_thr - level_db, 1.0 - 1.0 / below_ratio, knee);
    (above + below).clamp(GAIN_MIN_DB, GAIN_MAX_DB)
}

/// Biquad coefficients (a0-normalized) processed in transposed direct form II.
#[derive(Clone, Copy)]
pub(crate) struct Coefs {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
}

pub(crate) fn butterworth(freq: f32, sample_rate: f32, kind: u8) -> Coefs {
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
pub(crate) unsafe fn biquad(x: f32, c: &Coefs, z: *mut f32) -> f32 {
    let y = c.b0 * x + *z;
    *z = c.b1 * x - c.a1 * y + *z.add(1);
    *z.add(1) = c.b2 * x - c.a2 * y;
    y
}

/// Split one sample into (low, mid, high) bands. `z` points at this
/// channel's 10 biquads (20 f32): lp1 x2, ap2 x2, hp1 x2, lp2 x2, hp2 x2.
#[inline]
pub(crate) unsafe fn split_bands(x: f32, c: &[Coefs; 5], z: *mut f32) -> (f32, f32, f32) {
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
    *s.add(STATE_AMOUNT) = 1.0;
    *s.add(STATE_TIME) = 1.0;
    *s.add(STATE_OUTPUT_DB) = 0.0;
    *s.add(STATE_SOFT_KNEE) = 1.0;
    *s.add(STATE_RMS) = 0.0;
    *s.add(STATE_SPLIT_LOW) = 1.0;
    *s.add(STATE_SPLIT_HIGH) = 1.0;
    *s.add(STATE_XOVER_LOW_HZ) = 120.0;
    *s.add(STATE_XOVER_HIGH_HZ) = 2500.0;
    for band in 0..3 {
        let base = STATE_BAND_BASE + band * BAND_STRIDE;
        *s.add(base + BAND_BELOW_THR_DB) = DEFAULT_BELOW_THR_DB;
        *s.add(base + BAND_BELOW_RATIO) = 1.0;
        *s.add(base + BAND_ABOVE_THR_DB) = DEFAULT_ABOVE_THR_DB;
        *s.add(base + BAND_ABOVE_RATIO) = 1.0;
        *s.add(base + BAND_ATTACK_MS) = BAND_DEFAULT_ATTACK_MS[band];
        *s.add(base + BAND_RELEASE_MS) = BAND_DEFAULT_RELEASE_MS[band];
        *s.add(base + BAND_INPUT_DB) = 0.0;
        *s.add(base + BAND_OUTPUT_DB) = 0.0;
        *s.add(base + BAND_ON) = 1.0;
        *s.add(base + BAND_SOLO) = 0.0;
    }
    *s.add(STATE_SAMPLE_RATE) = sample_rate as f32;
    for i in 0..6 {
        *s.add(STATE_METER_LEVEL_DB + i) = LEVEL_FLOOR_DB;
    }
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
        for i in 0..6 {
            *s.add(STATE_METER_LEVEL_DB + i) = LEVEL_FLOOR_DB;
        }
        return;
    }

    let sr = *s.add(STATE_SAMPLE_RATE);
    let amount = (*s.add(STATE_AMOUNT)).clamp(0.0, 1.0);
    let time = (*s.add(STATE_TIME)).clamp(0.1, 10.0);
    let output_gain = db_to_amp((*s.add(STATE_OUTPUT_DB)).clamp(-24.0, 24.0));
    let knee = if *s.add(STATE_SOFT_KNEE) > 0.5 {
        KNEE_WIDTH_DB
    } else {
        0.0
    };
    let rms = *s.add(STATE_RMS) > 0.5;
    let split_low = *s.add(STATE_SPLIT_LOW) > 0.5;
    let split_high = *s.add(STATE_SPLIT_HIGH) > 0.5;
    let f_low = (*s.add(STATE_XOVER_LOW_HZ)).clamp(20.0, 2000.0);
    let f_high = (*s.add(STATE_XOVER_HIGH_HZ))
        .clamp(200.0, 18000.0)
        .max(f_low);

    let mut below_thr = [0.0f32; 3];
    let mut below_ratio = [1.0f32; 3];
    let mut above_thr = [0.0f32; 3];
    let mut above_ratio = [1.0f32; 3];
    let mut attack = [0.0f32; 3];
    let mut release = [0.0f32; 3];
    let mut input_gain = [1.0f32; 3];
    let mut band_out_gain = [1.0f32; 3];
    let mut band_on = [true; 3];
    let mut band_solo = [false; 3];
    for band in 0..3 {
        let base = STATE_BAND_BASE + band * BAND_STRIDE;
        below_thr[band] = (*s.add(base + BAND_BELOW_THR_DB)).clamp(LEVEL_FLOOR_DB, 0.0);
        below_ratio[band] = (*s.add(base + BAND_BELOW_RATIO)).clamp(0.1, 100.0);
        above_thr[band] = (*s.add(base + BAND_ABOVE_THR_DB)).clamp(LEVEL_FLOOR_DB, 0.0);
        above_ratio[band] = (*s.add(base + BAND_ABOVE_RATIO)).clamp(0.1, 100.0);
        attack[band] = time_coef(
            (*s.add(base + BAND_ATTACK_MS)).clamp(0.1, 1000.0) * time,
            sr,
        );
        release[band] = time_coef(
            (*s.add(base + BAND_RELEASE_MS)).clamp(1.0, 3000.0) * time,
            sr,
        );
        input_gain[band] = db_to_amp((*s.add(base + BAND_INPUT_DB)).clamp(-24.0, 24.0));
        band_out_gain[band] = db_to_amp((*s.add(base + BAND_OUTPUT_DB)).clamp(-24.0, 24.0));
        band_on[band] = *s.add(base + BAND_ON) > 0.5;
        band_solo[band] = *s.add(base + BAND_SOLO) > 0.5;
    }

    // Disabled splits merge outward into the mid band, matching the device's
    // High / Low split buttons; the crossover always runs so the sum stays
    // allpass-flat regardless of split state.
    let band_active = [split_low, true, split_high];
    let any_solo = (0..3).any(|band| band_active[band] && band_solo[band]);
    let mut band_audible = [true; 3];
    for band in 0..3 {
        band_audible[band] = band_active[band] && (!any_solo || band_solo[band]);
    }

    let coefs = [
        butterworth(f_low, sr, 0),  // lp @ low xover
        butterworth(f_low, sr, 1),  // hp @ low xover
        butterworth(f_high, sr, 0), // lp @ high xover
        butterworth(f_high, sr, 1), // hp @ high xover
        butterworth(f_high, sr, 2), // ap @ high xover (low-band phase comp)
    ];

    let gain_smooth = time_coef(8.0, sr);
    let meter_attack = time_coef(METER_ATTACK_MS, sr);
    let meter_release = time_coef(METER_RELEASE_MS, sr);

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
    let mut meter_env = [0.0f32; 6];
    for (i, slot) in meter_env.iter_mut().enumerate() {
        *slot = db_to_amp(*s.add(STATE_METER_LEVEL_DB + i)).max(0.0);
    }
    let z0 = s.add(STATE_FILTERS);
    let z1 = s.add(STATE_FILTERS + 20);

    for i in 0..nf {
        let x_l = *in0.add(i);
        let x_r = *in1.add(i);
        let (lo_l, mid_l, hi_l) = split_bands(x_l, &coefs, z0);
        let (lo_r, mid_r, hi_r) = split_bands(x_r, &coefs, z1);
        let mut bands_l = [lo_l, mid_l, hi_l];
        let mut bands_r = [lo_r, mid_r, hi_r];
        if !split_high {
            bands_l[1] += bands_l[2];
            bands_r[1] += bands_r[2];
            bands_l[2] = 0.0;
            bands_r[2] = 0.0;
        }
        if !split_low {
            bands_l[1] += bands_l[0];
            bands_r[1] += bands_r[0];
            bands_l[0] = 0.0;
            bands_r[0] = 0.0;
        }

        let mut wet_l = 0.0;
        let mut wet_r = 0.0;
        for b in 0..3 {
            let l = bands_l[b] * input_gain[b];
            let r = bands_r[b] * input_gain[b];

            let peak = l.abs().max(r.abs());
            let detector = if rms { peak * peak } else { peak };
            let coef = if detector > env[b] {
                attack[b]
            } else {
                release[b]
            };
            env[b] += coef * (detector - env[b]);
            let level_db = if rms {
                (10.0 * env[b].max(1.0e-18).log10()).max(LEVEL_FLOOR_DB)
            } else {
                amp_to_db(env[b]).max(LEVEL_FLOOR_DB)
            };
            let target_gain_db = if band_on[b] {
                dynamics_gain_db(
                    level_db,
                    below_thr[b],
                    below_ratio[b],
                    above_thr[b],
                    above_ratio[b],
                    knee,
                )
            } else {
                0.0
            };
            gain_db[b] += gain_smooth * (target_gain_db - gain_db[b]);

            if band_audible[b] {
                let g = db_to_amp(gain_db[b] * amount) * band_out_gain[b];
                wet_l += l * g;
                wet_r += r * g;
            }

            for (ch, sample) in [l, r].into_iter().enumerate() {
                let m = &mut meter_env[b * 2 + ch];
                let mag = sample.abs();
                let mcoef = if mag > *m {
                    meter_attack
                } else {
                    meter_release
                };
                *m += mcoef * (mag - *m);
            }
        }
        *out0.add(i) = wet_l * output_gain;
        *out1.add(i) = wet_r * output_gain;
    }

    for b in 0..3 {
        *s.add(STATE_ENV + b) = env[b];
        *s.add(STATE_GAIN_DB + b) = gain_db[b];
        *s.add(STATE_METER_GAIN_DB + b) = if band_active[b] {
            gain_db[b] * amount
        } else {
            0.0
        };
    }
    for (i, m) in meter_env.iter().enumerate() {
        let band = i / 2;
        *s.add(STATE_METER_LEVEL_DB + i) = if band_active[band] {
            amp_to_db(*m).max(LEVEL_FLOOR_DB)
        } else {
            LEVEL_FLOOR_DB
        };
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

    fn set_band(state: &mut [f32; OTT_STATE_SIZE], band: usize, field: OttBandField, value: f32) {
        state[ott_band_param(band, field) as usize] = value;
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
    fn crossover_sums_flat_at_unity_ratios() {
        // With 1:1 ratios everywhere the gain computer is unity, so the wet
        // path is just the crossover split-and-sum.
        let mut state = init_state();
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

    #[test]
    fn split_toggles_keep_the_sum_flat() {
        for (split_low, split_high) in [(0.0, 1.0), (1.0, 0.0), (0.0, 0.0)] {
            let mut state = init_state();
            state[STATE_SPLIT_LOW] = split_low;
            state[STATE_SPLIT_HIGH] = split_high;
            for freq in [60.0, 1000.0, 8000.0] {
                let src = sine(freq, 0.5, 16384);
                let mut l = src.clone();
                let mut r = src.clone();
                run(&mut state, &mut l, &mut r);
                let p = peak(&l[8192..]);
                assert!(
                    (p - 0.5).abs() < 0.06,
                    "merged bands not flat at {freq} Hz (splits {split_low}/{split_high}): peak {p}"
                );
            }
        }
    }

    #[test]
    fn above_threshold_compression_reduces_loud_signal() {
        let mut state = init_state();
        set_band(&mut state, 1, OttBandField::AboveThreshold, -30.0);
        set_band(&mut state, 1, OttBandField::AboveRatio, 8.0);
        let loud_amp = db_to_amp(-6.0);
        let mut l = sine(1000.0, loud_amp, SR as usize);
        let mut r = l.clone();
        run(&mut state, &mut l, &mut r);
        let tail = &l[l.len() - 4096..];
        // -6 dB into a -30 dB threshold at 8:1 is 21 dB of reduction.
        let p = amp_to_db(peak(tail));
        assert!(
            p < -20.0,
            "expected heavy downward compression, tail peak {p} dB"
        );
    }

    #[test]
    fn below_threshold_ratio_boosts_quiet_signal() {
        let mut state = init_state();
        set_band(&mut state, 1, OttBandField::BelowThreshold, -30.0);
        set_band(&mut state, 1, OttBandField::BelowRatio, 4.0);
        let quiet_amp = db_to_amp(-46.0);
        let mut l = sine(1000.0, quiet_amp, SR as usize);
        let mut r = l.clone();
        run(&mut state, &mut l, &mut r);
        let tail = &l[l.len() - 4096..];
        // 16 dB under the threshold at 1:4 recovers 12 dB.
        let p = amp_to_db(peak(tail));
        assert!(
            p > -40.0,
            "expected upward compression boost, tail peak {p} dB"
        );
    }

    #[test]
    fn below_threshold_expansion_gates_quiet_signal() {
        let mut state = init_state();
        set_band(&mut state, 1, OttBandField::BelowThreshold, -30.0);
        set_band(&mut state, 1, OttBandField::BelowRatio, 0.5);
        let quiet_amp = db_to_amp(-46.0);
        let mut l = sine(1000.0, quiet_amp, SR as usize);
        let mut r = l.clone();
        run(&mut state, &mut l, &mut r);
        let tail = &l[l.len() - 4096..];
        let p = amp_to_db(peak(tail));
        assert!(p < -55.0, "expected downward expansion, tail peak {p} dB");
    }

    #[test]
    fn amount_scales_the_dynamics_gain() {
        let mut settings = init_state();
        set_band(&mut settings, 1, OttBandField::AboveThreshold, -30.0);
        set_band(&mut settings, 1, OttBandField::AboveRatio, 8.0);
        let loud_amp = db_to_amp(-6.0);
        let mut peaks = Vec::new();
        for amount in [0.0f32, 1.0] {
            let mut state = settings;
            state[STATE_AMOUNT] = amount;
            let mut l = sine(1000.0, loud_amp, SR as usize);
            let mut r = l.clone();
            run(&mut state, &mut l, &mut r);
            peaks.push(peak(&l[l.len() - 4096..]));
        }
        assert!(
            (amp_to_db(peaks[0]) + 6.0).abs() < 1.0,
            "amount 0 should be transparent, got {} dB",
            amp_to_db(peaks[0])
        );
        assert!(
            amp_to_db(peaks[1]) < amp_to_db(peaks[0]) - 12.0,
            "amount 1 should compress hard"
        );
    }

    #[test]
    fn band_solo_mutes_other_bands() {
        let mut state = init_state();
        set_band(&mut state, 0, OttBandField::Solo, 1.0);
        let src = sine(4000.0, 0.5, 16384); // mid-band content
        let mut l = src.clone();
        let mut r = src.clone();
        run(&mut state, &mut l, &mut r);
        assert!(
            peak(&l[8192..]) < 0.02,
            "soloing the low band should mute mid content"
        );
    }

    #[test]
    fn band_off_bypasses_dynamics_but_passes_audio() {
        let mut state = init_state();
        set_band(&mut state, 1, OttBandField::AboveThreshold, -30.0);
        set_band(&mut state, 1, OttBandField::AboveRatio, 8.0);
        set_band(&mut state, 1, OttBandField::On, 0.0);
        let loud_amp = db_to_amp(-6.0);
        let mut l = sine(1000.0, loud_amp, SR as usize);
        let mut r = l.clone();
        run(&mut state, &mut l, &mut r);
        let p = amp_to_db(peak(&l[l.len() - 4096..]));
        assert!(
            (p + 6.0).abs() < 1.0,
            "band off should pass audio unprocessed, got {p} dB"
        );
    }

    #[test]
    fn meters_report_level_and_gain() {
        let mut state = init_state();
        set_band(&mut state, 1, OttBandField::AboveThreshold, -30.0);
        set_band(&mut state, 1, OttBandField::AboveRatio, 8.0);
        let loud_amp = db_to_amp(-6.0);
        let mut l = sine(1000.0, loud_amp, SR as usize);
        let mut r = l.clone();
        run(&mut state, &mut l, &mut r);
        let mid_level_l = state[STATE_METER_LEVEL_DB + 2];
        let mid_gain = state[STATE_METER_GAIN_DB + 1];
        assert!(
            mid_level_l > -12.0 && mid_level_l < 0.0,
            "mid band level meter should track the sine, got {mid_level_l} dB"
        );
        assert!(
            mid_gain < -12.0,
            "mid band gain meter should show reduction, got {mid_gain} dB"
        );
        let low_level = state[STATE_METER_LEVEL_DB];
        assert!(
            low_level < -30.0,
            "low band should be quiet for a 1 kHz sine, got {low_level} dB"
        );
    }
}
