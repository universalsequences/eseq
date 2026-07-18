//! Ableton-style bread-and-butter compressor with external sidechain.
//!
//! Detection models: Peak, RMS, and Expand (downward expander). Soft knee,
//! program-dependent auto release, linear/log envelope smoothing, 0/1/10 ms
//! lookahead, auto makeup, dry/wet, and a sidechain section (external input
//! on channel 2, gain, filter, listen).
//!
//! The node also records a fine-grained meter ring (output level + gain
//! reduction every `METER_STRIDE` samples) in its state so the UI's activity
//! display gets Ableton-grade trace resolution independent of the UI poll
//! rate. The ring layout is read by `ui::live_audio_analyzer`.

use crate::audiograph::NodeVTable;
use std::os::raw::{c_int, c_void};

const STATE_ENABLED: usize = 0;
const STATE_THRESHOLD_DB: usize = 1;
const STATE_RATIO: usize = 2;
const STATE_ATTACK_MS: usize = 3;
const STATE_RELEASE_MS: usize = 4;
const STATE_AUTO_RELEASE: usize = 5;
const STATE_MODEL: usize = 6;
const STATE_KNEE_DB: usize = 7;
const STATE_LOOKAHEAD_MODE: usize = 8;
const STATE_ENV_MODE: usize = 9;
const STATE_OUT_DB: usize = 10;
const STATE_AUTO_MAKEUP: usize = 11;
const STATE_DRY_WET: usize = 12;
const STATE_SC_ON: usize = 13;
const STATE_SC_GAIN_DB: usize = 14;
const STATE_SC_FILTER_ON: usize = 15;
const STATE_SC_FILTER_TYPE: usize = 16;
const STATE_SC_FREQ: usize = 17;
const STATE_SC_Q: usize = 18;
const STATE_SC_LISTEN: usize = 19;
pub const STATE_SAMPLE_RATE: usize = 20;
const STATE_ENV: usize = 21;
const STATE_GAIN_FAST_DB: usize = 22;
const STATE_GAIN_SLOW_DB: usize = 23;
const STATE_RMS_MS: usize = 24;
const STATE_BQ_X1: usize = 25;
const STATE_BQ_X2: usize = 26;
const STATE_BQ_Y1: usize = 27;
const STATE_BQ_Y2: usize = 28;
pub const STATE_METER_IN_DB: usize = 29;
pub const STATE_METER_GR_DB: usize = 30;
pub const STATE_METER_OUT_DB: usize = 31;
pub const STATE_RING_WRITE: usize = 32;
const STATE_ACC_OUT: usize = 33;
const STATE_ACC_GR_DB: usize = 34;
const STATE_ACC_COUNT: usize = 35;
const STATE_DELAY_POS: usize = 36;
const STATE_DELAY_RING: usize = 37;
/// Covers 10 ms of lookahead up to 102.4 kHz.
const DELAY_RING_FRAMES: usize = 1024;
pub const STATE_METER_RING: usize = STATE_DELAY_RING + DELAY_RING_FRAMES * 2;
/// Meter ring entries are (output dB, gain-reduction dB) pairs written every
/// `METER_STRIDE` samples: ~4 s of history at 48 kHz.
pub const METER_RING_LEN: usize = 1536;
pub const METER_STRIDE: usize = 128;
pub const COMPRESSOR_STATE_SIZE: usize = STATE_METER_RING + METER_RING_LEN * 2;

pub const COMPRESSOR_PARAM_ENABLED: u64 = STATE_ENABLED as u64;
pub const COMPRESSOR_PARAM_THRESHOLD_DB: u64 = STATE_THRESHOLD_DB as u64;
pub const COMPRESSOR_PARAM_RATIO: u64 = STATE_RATIO as u64;
pub const COMPRESSOR_PARAM_ATTACK_MS: u64 = STATE_ATTACK_MS as u64;
pub const COMPRESSOR_PARAM_RELEASE_MS: u64 = STATE_RELEASE_MS as u64;
pub const COMPRESSOR_PARAM_AUTO_RELEASE: u64 = STATE_AUTO_RELEASE as u64;
pub const COMPRESSOR_PARAM_MODEL: u64 = STATE_MODEL as u64;
pub const COMPRESSOR_PARAM_KNEE_DB: u64 = STATE_KNEE_DB as u64;
pub const COMPRESSOR_PARAM_LOOKAHEAD: u64 = STATE_LOOKAHEAD_MODE as u64;
pub const COMPRESSOR_PARAM_ENV_MODE: u64 = STATE_ENV_MODE as u64;
pub const COMPRESSOR_PARAM_OUT_DB: u64 = STATE_OUT_DB as u64;
pub const COMPRESSOR_PARAM_AUTO_MAKEUP: u64 = STATE_AUTO_MAKEUP as u64;
pub const COMPRESSOR_PARAM_DRY_WET: u64 = STATE_DRY_WET as u64;
pub const COMPRESSOR_PARAM_SC_ON: u64 = STATE_SC_ON as u64;
pub const COMPRESSOR_PARAM_SC_GAIN_DB: u64 = STATE_SC_GAIN_DB as u64;
pub const COMPRESSOR_PARAM_SC_FILTER_ON: u64 = STATE_SC_FILTER_ON as u64;
pub const COMPRESSOR_PARAM_SC_FILTER_TYPE: u64 = STATE_SC_FILTER_TYPE as u64;
pub const COMPRESSOR_PARAM_SC_FREQ: u64 = STATE_SC_FREQ as u64;
pub const COMPRESSOR_PARAM_SC_Q: u64 = STATE_SC_Q as u64;
pub const COMPRESSOR_PARAM_SC_LISTEN: u64 = STATE_SC_LISTEN as u64;

/// External sidechain audio arrives on this input channel (mono).
pub const SIDECHAIN_INPUT_CHANNEL: usize = 2;

pub const MODEL_PEAK: f32 = 0.0;
pub const MODEL_RMS: f32 = 1.0;
pub const MODEL_EXPAND: f32 = 2.0;

/// Deepest reduction the expander will apply, so silence doesn't collapse
/// to -inf dB.
const EXPAND_FLOOR_DB: f32 = -40.0;
const GR_FLOOR_DB: f32 = -60.0;
const LEVEL_FLOOR_DB: f32 = -70.0;

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

/// Static gain curve: desired gain change (dB, <= 0) for a detector level.
/// Compress bends above the threshold, expand bends below it; both use a
/// quadratic soft knee of `knee_db` width centered on the threshold.
pub fn static_gain_db(
    detector_db: f32,
    threshold_db: f32,
    ratio: f32,
    knee_db: f32,
    expand: bool,
) -> f32 {
    let ratio = ratio.max(1.0);
    let over = detector_db - threshold_db;
    let half_knee = knee_db.max(0.0) * 0.5;
    let gain = if expand {
        // Downward expander: reduce below threshold by (ratio - 1).
        let slope = ratio - 1.0;
        if over >= half_knee {
            0.0
        } else if over > -half_knee && knee_db > 0.0 {
            -slope * (half_knee - over) * (half_knee - over) / (2.0 * knee_db)
        } else {
            slope * over
        }
        .max(EXPAND_FLOOR_DB)
    } else {
        let slope = 1.0 / ratio - 1.0;
        if over <= -half_knee {
            0.0
        } else if over < half_knee && knee_db > 0.0 {
            slope * (over + half_knee) * (over + half_knee) / (2.0 * knee_db)
        } else {
            slope * over
        }
    };
    gain.clamp(GR_FLOOR_DB, 0.0)
}

/// Auto-makeup gain: full compensation of the static curve at 0 dBFS.
pub fn auto_makeup_db(threshold_db: f32, ratio: f32, knee_db: f32, expand: bool) -> f32 {
    if expand {
        0.0
    } else {
        -static_gain_db(0.0, threshold_db, ratio, knee_db, false)
    }
}

/// RBJ biquad coefficients for the sidechain filter.
/// `filter_type`: 0 lowpass, 1 highpass, 2 bandpass, 3 notch.
fn sc_filter_coefs(filter_type: usize, freq: f32, q: f32, sample_rate: f32) -> [f32; 5] {
    let sr = sample_rate.max(1.0);
    let freq = freq.clamp(10.0, sr * 0.45);
    let q = q.clamp(0.05, 12.0);
    let w0 = 2.0 * std::f32::consts::PI * freq / sr;
    let (sin_w0, cos_w0) = w0.sin_cos();
    let alpha = sin_w0 / (2.0 * q);
    let (b0, b1, b2, a0, a1, a2) = match filter_type {
        1 => {
            let b1 = -(1.0 + cos_w0);
            (
                (1.0 + cos_w0) / 2.0,
                b1,
                (1.0 + cos_w0) / 2.0,
                1.0 + alpha,
                -2.0 * cos_w0,
                1.0 - alpha,
            )
        }
        2 => (
            alpha,
            0.0,
            -alpha,
            1.0 + alpha,
            -2.0 * cos_w0,
            1.0 - alpha,
        ),
        3 => (
            1.0,
            -2.0 * cos_w0,
            1.0,
            1.0 + alpha,
            -2.0 * cos_w0,
            1.0 - alpha,
        ),
        _ => {
            let b1 = 1.0 - cos_w0;
            (
                (1.0 - cos_w0) / 2.0,
                b1,
                (1.0 - cos_w0) / 2.0,
                1.0 + alpha,
                -2.0 * cos_w0,
                1.0 - alpha,
            )
        }
    };
    [b0 / a0, b1 / a0, b2 / a0, a1 / a0, a2 / a0]
}

/// Lookahead delay in samples for the mode selector (0 -> 0 ms, 1 -> 1 ms,
/// 2 -> 10 ms), clamped to the state ring.
fn lookahead_samples(mode: f32, sample_rate: f32) -> usize {
    let ms = match mode.round() as i32 {
        1 => 1.0,
        2 => 10.0,
        _ => 0.0,
    };
    ((ms * 0.001 * sample_rate) as usize).min(DELAY_RING_FRAMES - 1)
}

unsafe extern "C" fn compressor_init(
    state: *mut c_void,
    sample_rate: c_int,
    _max_block: c_int,
    _initial_state: *const c_void,
) {
    let s = state as *mut f32;
    std::ptr::write_bytes(s, 0, COMPRESSOR_STATE_SIZE);
    *s.add(STATE_ENABLED) = 1.0;
    *s.add(STATE_THRESHOLD_DB) = -18.0;
    *s.add(STATE_RATIO) = 4.0;
    *s.add(STATE_ATTACK_MS) = 1.0;
    *s.add(STATE_RELEASE_MS) = 30.0;
    *s.add(STATE_MODEL) = MODEL_RMS;
    *s.add(STATE_KNEE_DB) = 6.0;
    *s.add(STATE_ENV_MODE) = 1.0;
    *s.add(STATE_DRY_WET) = 1.0;
    *s.add(STATE_SC_GAIN_DB) = 0.0;
    *s.add(STATE_SC_FILTER_TYPE) = 0.0;
    *s.add(STATE_SC_FREQ) = 80.0;
    *s.add(STATE_SC_Q) = 0.71;
    *s.add(STATE_SAMPLE_RATE) = sample_rate as f32;
    *s.add(STATE_METER_IN_DB) = LEVEL_FLOOR_DB;
    *s.add(STATE_METER_GR_DB) = 0.0;
    *s.add(STATE_METER_OUT_DB) = LEVEL_FLOOR_DB;
    for i in 0..METER_RING_LEN {
        *s.add(STATE_METER_RING + i * 2) = LEVEL_FLOOR_DB;
        *s.add(STATE_METER_RING + i * 2 + 1) = 0.0;
    }
}

unsafe extern "C" fn compressor_process(
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
    let sc_in = *inp.add(SIDECHAIN_INPUT_CHANNEL);
    let out0 = *out.add(0);
    let out1 = *out.add(1);

    if *s.add(STATE_ENABLED) <= 0.5 {
        std::ptr::copy_nonoverlapping(in0 as *const f32, out0, nf);
        std::ptr::copy_nonoverlapping(in1 as *const f32, out1, nf);
        *s.add(STATE_ENV) = 0.0;
        *s.add(STATE_GAIN_FAST_DB) = 0.0;
        *s.add(STATE_GAIN_SLOW_DB) = 0.0;
        *s.add(STATE_RMS_MS) = 0.0;
        *s.add(STATE_METER_GR_DB) = 0.0;
        return;
    }

    let sr = (*s.add(STATE_SAMPLE_RATE)).max(1.0);
    let threshold = (*s.add(STATE_THRESHOLD_DB)).clamp(-70.0, 6.0);
    let ratio = (*s.add(STATE_RATIO)).clamp(1.0, 40.0);
    let knee = (*s.add(STATE_KNEE_DB)).clamp(0.0, 18.0);
    let model = (*s.add(STATE_MODEL)).round();
    let expand = model == MODEL_EXPAND;
    let rms = model == MODEL_RMS;
    let auto_release = *s.add(STATE_AUTO_RELEASE) > 0.5;
    let log_env = *s.add(STATE_ENV_MODE) > 0.5;
    let attack = time_coef((*s.add(STATE_ATTACK_MS)).clamp(0.01, 1000.0), sr);
    let release_ms = (*s.add(STATE_RELEASE_MS)).clamp(1.0, 3000.0);
    let release_fast = time_coef(if auto_release { 60.0 } else { release_ms }, sr);
    let release_slow = time_coef(1000.0, sr);
    // ~5 ms RMS integration window.
    let rms_coef = time_coef(5.0, sr);
    let dry_wet = (*s.add(STATE_DRY_WET)).clamp(0.0, 1.0);
    let sc_on = *s.add(STATE_SC_ON) > 0.5;
    let sc_gain = db_to_amp((*s.add(STATE_SC_GAIN_DB)).clamp(-24.0, 24.0));
    let sc_filter_on = *s.add(STATE_SC_FILTER_ON) > 0.5;
    let sc_listen = *s.add(STATE_SC_LISTEN) > 0.5;
    let makeup_db = (*s.add(STATE_OUT_DB)).clamp(-36.0, 36.0)
        + if *s.add(STATE_AUTO_MAKEUP) > 0.5 {
            auto_makeup_db(threshold, ratio, knee, expand)
        } else {
            0.0
        };
    let makeup = db_to_amp(makeup_db);
    let lookahead = lookahead_samples(*s.add(STATE_LOOKAHEAD_MODE), sr);

    let coefs = sc_filter_coefs(
        (*s.add(STATE_SC_FILTER_TYPE)).round().max(0.0) as usize,
        *s.add(STATE_SC_FREQ),
        *s.add(STATE_SC_Q),
        sr,
    );
    let mut bq_x1 = *s.add(STATE_BQ_X1);
    let mut bq_x2 = *s.add(STATE_BQ_X2);
    let mut bq_y1 = *s.add(STATE_BQ_Y1);
    let mut bq_y2 = *s.add(STATE_BQ_Y2);

    let mut env = *s.add(STATE_ENV);
    let mut gain_fast_db = *s.add(STATE_GAIN_FAST_DB);
    let mut gain_slow_db = *s.add(STATE_GAIN_SLOW_DB);
    let mut rms_ms = *s.add(STATE_RMS_MS);
    let mut delay_pos = *s.add(STATE_DELAY_POS) as usize % DELAY_RING_FRAMES;
    let mut ring_write = *s.add(STATE_RING_WRITE);
    let mut acc_out = *s.add(STATE_ACC_OUT);
    let mut acc_gr_db = *s.add(STATE_ACC_GR_DB);
    let mut acc_count = *s.add(STATE_ACC_COUNT);
    let mut block_in_peak = 0.0f32;
    let mut block_out_peak = 0.0f32;
    let mut block_gr_db = 0.0f32;

    for i in 0..nf {
        let input_l = *in0.add(i);
        let input_r = *in1.add(i);

        // Detector source: external sidechain (mono) or the internal mix,
        // optionally shaped by the sidechain filter.
        let mut det_sig = if sc_on {
            *sc_in.add(i) * sc_gain
        } else {
            0.5 * (input_l + input_r)
        };
        if sc_filter_on {
            let y = coefs[0] * det_sig + coefs[1] * bq_x1 + coefs[2] * bq_x2
                - coefs[3] * bq_y1
                - coefs[4] * bq_y2;
            bq_x2 = bq_x1;
            bq_x1 = det_sig;
            bq_y2 = bq_y1;
            bq_y1 = y;
            det_sig = y;
        }

        let detector = if rms {
            rms_ms += rms_coef * (det_sig * det_sig - rms_ms);
            rms_ms.max(0.0).sqrt()
        } else {
            let rect = det_sig.abs();
            let coef = if rect > env { attack } else { release_fast };
            env += coef * (rect - env);
            env
        };
        block_in_peak = block_in_peak.max(detector);

        let target_db = static_gain_db(amp_to_db(detector), threshold, ratio, knee, expand);
        // Gain smoothing: log mode works in dB, lin mode in amplitude. Auto
        // release blends a fast and a slow smoother (program dependent).
        if log_env {
            let coef = if target_db < gain_fast_db {
                attack
            } else {
                release_fast
            };
            gain_fast_db += coef * (target_db - gain_fast_db);
            if auto_release {
                let coef_slow = if target_db < gain_slow_db {
                    attack
                } else {
                    release_slow
                };
                gain_slow_db += coef_slow * (target_db - gain_slow_db);
            }
        } else {
            let target = db_to_amp(target_db);
            let fast = db_to_amp(gain_fast_db);
            let coef = if target < fast { attack } else { release_fast };
            gain_fast_db = amp_to_db(fast + coef * (target - fast));
            if auto_release {
                let slow = db_to_amp(gain_slow_db);
                let coef_slow = if target < slow { attack } else { release_slow };
                gain_slow_db = amp_to_db(slow + coef_slow * (target - slow));
            }
        }
        let gain_db = if auto_release {
            0.5 * (gain_fast_db + gain_slow_db)
        } else {
            gain_fast_db
        };
        block_gr_db = block_gr_db.min(gain_db);

        // Lookahead: the audio path (dry and wet alike) is delayed while the
        // detector runs on the undelayed signal.
        let (audio_l, audio_r) = if lookahead > 0 {
            let read = (delay_pos + DELAY_RING_FRAMES - lookahead) % DELAY_RING_FRAMES;
            let dl = *s.add(STATE_DELAY_RING + read);
            let dr = *s.add(STATE_DELAY_RING + DELAY_RING_FRAMES + read);
            *s.add(STATE_DELAY_RING + delay_pos) = input_l;
            *s.add(STATE_DELAY_RING + DELAY_RING_FRAMES + delay_pos) = input_r;
            delay_pos = (delay_pos + 1) % DELAY_RING_FRAMES;
            (dl, dr)
        } else {
            (input_l, input_r)
        };

        let gain = db_to_amp(gain_db) * makeup;
        let (mut l, mut r) = (
            audio_l + (audio_l * gain - audio_l) * dry_wet,
            audio_r + (audio_r * gain - audio_r) * dry_wet,
        );
        if sc_listen {
            l = det_sig;
            r = det_sig;
        }
        *out0.add(i) = l;
        *out1.add(i) = r;

        let out_peak = l.abs().max(r.abs());
        block_out_peak = block_out_peak.max(out_peak);
        acc_out = acc_out.max(out_peak);
        acc_gr_db = acc_gr_db.min(gain_db);
        acc_count += 1.0;
        if acc_count >= METER_STRIDE as f32 {
            let entry = (ring_write as usize) % METER_RING_LEN;
            *s.add(STATE_METER_RING + entry * 2) = amp_to_db(acc_out).max(LEVEL_FLOOR_DB);
            *s.add(STATE_METER_RING + entry * 2 + 1) = acc_gr_db;
            ring_write += 1.0;
            if ring_write >= 1.0e7 {
                ring_write -= 1.0e7 - (1.0e7 % METER_RING_LEN as f32);
            }
            acc_out = 0.0;
            acc_gr_db = 0.0;
            acc_count = 0.0;
        }
    }

    *s.add(STATE_ENV) = env;
    *s.add(STATE_GAIN_FAST_DB) = gain_fast_db;
    *s.add(STATE_GAIN_SLOW_DB) = gain_slow_db;
    *s.add(STATE_RMS_MS) = rms_ms;
    *s.add(STATE_BQ_X1) = if bq_x1.is_finite() { bq_x1 } else { 0.0 };
    *s.add(STATE_BQ_X2) = if bq_x2.is_finite() { bq_x2 } else { 0.0 };
    *s.add(STATE_BQ_Y1) = if bq_y1.is_finite() { bq_y1 } else { 0.0 };
    *s.add(STATE_BQ_Y2) = if bq_y2.is_finite() { bq_y2 } else { 0.0 };
    *s.add(STATE_DELAY_POS) = delay_pos as f32;
    *s.add(STATE_RING_WRITE) = ring_write;
    *s.add(STATE_ACC_OUT) = acc_out;
    *s.add(STATE_ACC_GR_DB) = acc_gr_db;
    *s.add(STATE_ACC_COUNT) = acc_count;
    *s.add(STATE_METER_IN_DB) = amp_to_db(block_in_peak).max(LEVEL_FLOOR_DB);
    *s.add(STATE_METER_GR_DB) = block_gr_db;
    *s.add(STATE_METER_OUT_DB) = amp_to_db(block_out_peak).max(LEVEL_FLOOR_DB);
}

pub fn compressor_vtable() -> NodeVTable {
    NodeVTable {
        process: Some(compressor_process),
        init: Some(compressor_init),
        reset: None,
        migrate: None,
        ..NodeVTable::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run_block(
        state: &mut [f32],
        input: &[Vec<f32>; 3],
        nframes: usize,
    ) -> (Vec<f32>, Vec<f32>) {
        let mut in0 = input[0].clone();
        let mut in1 = input[1].clone();
        let mut in2 = input[2].clone();
        let mut out0 = vec![0.0f32; nframes];
        let mut out1 = vec![0.0f32; nframes];
        let inputs = [in0.as_mut_ptr(), in1.as_mut_ptr(), in2.as_mut_ptr()];
        let outputs = [out0.as_mut_ptr(), out1.as_mut_ptr()];
        unsafe {
            compressor_process(
                inputs.as_ptr(),
                outputs.as_ptr(),
                nframes as c_int,
                state.as_mut_ptr().cast(),
                std::ptr::null_mut(),
            );
        }
        (out0, out1)
    }

    fn init_state() -> Vec<f32> {
        let mut state = vec![0.0f32; COMPRESSOR_STATE_SIZE];
        unsafe {
            compressor_init(state.as_mut_ptr().cast(), 48_000, 512, std::ptr::null());
        }
        state
    }

    #[test]
    fn static_curve_is_transparent_below_threshold_and_bends_above() {
        assert_eq!(static_gain_db(-40.0, -20.0, 4.0, 0.0, false), 0.0);
        let gr = static_gain_db(-8.0, -20.0, 4.0, 0.0, false);
        // 12 dB over at 4:1 -> 9 dB of reduction.
        assert!((gr + 9.0).abs() < 1.0e-4, "{gr}");
        // Soft knee: halfway into the knee reduces less than the hard curve.
        let soft = static_gain_db(-20.0, -20.0, 4.0, 12.0, false);
        assert!(soft < 0.0 && soft > -4.5, "{soft}");
    }

    #[test]
    fn expand_curve_reduces_below_threshold_with_floor() {
        assert_eq!(static_gain_db(-10.0, -20.0, 4.0, 0.0, true), 0.0);
        let gr = static_gain_db(-25.0, -20.0, 2.0, 0.0, true);
        assert!((gr + 5.0).abs() < 1.0e-4, "{gr}");
        assert_eq!(static_gain_db(-90.0, -20.0, 10.0, 0.0, true), EXPAND_FLOOR_DB);
    }

    #[test]
    fn auto_makeup_compensates_static_curve_at_zero_db() {
        let makeup = auto_makeup_db(-20.0, 4.0, 0.0, false);
        assert!((makeup - 15.0).abs() < 1.0e-4, "{makeup}");
        assert_eq!(auto_makeup_db(-20.0, 4.0, 0.0, true), 0.0);
    }

    #[test]
    fn loud_input_is_reduced_and_meters_move() {
        let mut state = init_state();
        state[STATE_THRESHOLD_DB] = -20.0;
        state[STATE_RATIO] = 10.0;
        state[STATE_ATTACK_MS] = 0.05;
        state[STATE_MODEL] = MODEL_PEAK;
        let n = 4096;
        let loud = vec![0.9f32; n];
        let inputs = [loud.clone(), loud, vec![0.0; n]];
        let (out0, _) = run_block(&mut state, &inputs, n);
        assert!(
            out0[n - 1].abs() < 0.5,
            "expected heavy reduction, got {}",
            out0[n - 1]
        );
        assert!(state[STATE_METER_GR_DB] < -6.0, "{}", state[STATE_METER_GR_DB]);
        assert!(state[STATE_RING_WRITE] as usize >= n / METER_STRIDE);
        let entry = (state[STATE_RING_WRITE] as usize - 1) % METER_RING_LEN;
        assert!(state[STATE_METER_RING + entry * 2] > LEVEL_FLOOR_DB);
        assert!(state[STATE_METER_RING + entry * 2 + 1] < -6.0);
    }

    #[test]
    fn external_sidechain_ducks_the_main_signal() {
        let mut state = init_state();
        state[STATE_THRESHOLD_DB] = -30.0;
        state[STATE_RATIO] = 20.0;
        state[STATE_ATTACK_MS] = 0.05;
        state[STATE_MODEL] = MODEL_PEAK;
        state[STATE_SC_ON] = 1.0;
        let n = 4096;
        let quiet_main = vec![0.1f32; n];
        // Without sidechain signal: main passes at threshold-transparent level.
        let inputs = [quiet_main.clone(), quiet_main.clone(), vec![0.0; n]];
        let (idle, _) = run_block(&mut state, &inputs, n);
        // Loud sidechain signal ducks the quiet main signal.
        let inputs = [quiet_main.clone(), quiet_main, vec![0.9f32; n]];
        let (ducked, _) = run_block(&mut state, &inputs, n);
        assert!(
            ducked[n - 1].abs() < idle[n - 1].abs() * 0.5,
            "idle {} ducked {}",
            idle[n - 1],
            ducked[n - 1]
        );
    }

    #[test]
    fn sidechain_listen_outputs_the_detector_signal() {
        let mut state = init_state();
        state[STATE_SC_ON] = 1.0;
        state[STATE_SC_LISTEN] = 1.0;
        let n = 256;
        let inputs = [vec![0.5f32; n], vec![0.5f32; n], vec![0.25f32; n]];
        let (out0, out1) = run_block(&mut state, &inputs, n);
        assert!((out0[n - 1] - 0.25).abs() < 1.0e-5, "{}", out0[n - 1]);
        assert_eq!(out0[n - 1], out1[n - 1]);
    }

    #[test]
    fn lookahead_delays_the_audio_path() {
        let mut state = init_state();
        state[STATE_LOOKAHEAD_MODE] = 1.0; // 1 ms = 48 samples at 48 kHz
        state[STATE_THRESHOLD_DB] = 6.0; // transparent gain
        let n = 256;
        let mut impulse = vec![0.0f32; n];
        impulse[0] = 1.0;
        let inputs = [impulse.clone(), impulse, vec![0.0; n]];
        let (out0, _) = run_block(&mut state, &inputs, n);
        assert!(out0[0].abs() < 1.0e-6);
        let peak_at = out0
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.abs().total_cmp(&b.1.abs()))
            .map(|(i, _)| i)
            .unwrap();
        assert_eq!(peak_at, 48);
    }

    #[test]
    fn disabled_node_passes_audio_through() {
        let mut state = init_state();
        state[STATE_ENABLED] = 0.0;
        let n = 64;
        let inputs = [vec![0.7f32; n], vec![-0.7f32; n], vec![0.9f32; n]];
        let (out0, out1) = run_block(&mut state, &inputs, n);
        assert_eq!(out0[n - 1], 0.7);
        assert_eq!(out1[n - 1], -0.7);
    }
}
