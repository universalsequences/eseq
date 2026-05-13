use crate::audiograph::NodeVTable;
use std::os::raw::{c_int, c_void};

const MAX_DELAY_SAMPLES: usize = 96000;

const STATE_ENABLED: usize = 0;
const STATE_WET: usize = 1;
const STATE_FEEDBACK: usize = 2;
const STATE_LEFT_SYNC: usize = 3;
const STATE_LEFT_DIV: usize = 4;
const STATE_LEFT_OFFSET: usize = 5;
const STATE_LEFT_TIME_MS: usize = 6;
const STATE_RIGHT_SYNC: usize = 7;
const STATE_RIGHT_DIV: usize = 8;
const STATE_RIGHT_OFFSET: usize = 9;
const STATE_RIGHT_TIME_MS: usize = 10;
const STATE_FILTER_FREQ: usize = 11;
const STATE_FILTER_Q: usize = 12;
const STATE_MOD_RATE: usize = 13;
const STATE_MOD_AMOUNT: usize = 14;
const STATE_MOD_PHASE: usize = 15;
const STATE_BPM: usize = 16;
const STATE_SAMPLE_RATE: usize = 17;
const STATE_SMOOTH_WET: usize = 18;
const STATE_SMOOTH_FEEDBACK: usize = 19;
const STATE_SMOOTH_LEFT_SAMPLES: usize = 20;
const STATE_SMOOTH_RIGHT_SAMPLES: usize = 21;
const STATE_SMOOTH_FILTER_FREQ: usize = 22;
const STATE_SMOOTH_FILTER_Q: usize = 23;
const STATE_SMOOTH_MOD_AMOUNT: usize = 24;
const STATE_WRITE_POS_L: usize = 25;
const STATE_WRITE_POS_R: usize = 26;
const STATE_LFO_PHASE: usize = 27;
const STATE_HP_Z1_L: usize = 28;
const STATE_HP_Z2_L: usize = 29;
const STATE_LP_Z1_L: usize = 30;
const STATE_LP_Z2_L: usize = 31;
const STATE_HP_Z1_R: usize = 32;
const STATE_HP_Z2_R: usize = 33;
const STATE_LP_Z1_R: usize = 34;
const STATE_LP_Z2_R: usize = 35;
const STATE_BUF_OFFSET: usize = 36;
const STATE_BUF_R_OFFSET: usize = STATE_BUF_OFFSET + MAX_DELAY_SAMPLES;
const STATE_END: usize = STATE_BUF_OFFSET + MAX_DELAY_SAMPLES * 2;

pub const STR8_DELAY_STATE_SIZE: usize = STATE_END;

pub const STR8_DELAY_PARAM_ENABLED: u64 = STATE_ENABLED as u64;
pub const STR8_DELAY_PARAM_WET: u64 = STATE_WET as u64;
pub const STR8_DELAY_PARAM_FEEDBACK: u64 = STATE_FEEDBACK as u64;
pub const STR8_DELAY_PARAM_LEFT_SYNC: u64 = STATE_LEFT_SYNC as u64;
pub const STR8_DELAY_PARAM_LEFT_DIV: u64 = STATE_LEFT_DIV as u64;
pub const STR8_DELAY_PARAM_LEFT_OFFSET: u64 = STATE_LEFT_OFFSET as u64;
pub const STR8_DELAY_PARAM_LEFT_TIME_MS: u64 = STATE_LEFT_TIME_MS as u64;
pub const STR8_DELAY_PARAM_RIGHT_SYNC: u64 = STATE_RIGHT_SYNC as u64;
pub const STR8_DELAY_PARAM_RIGHT_DIV: u64 = STATE_RIGHT_DIV as u64;
pub const STR8_DELAY_PARAM_RIGHT_OFFSET: u64 = STATE_RIGHT_OFFSET as u64;
pub const STR8_DELAY_PARAM_RIGHT_TIME_MS: u64 = STATE_RIGHT_TIME_MS as u64;
pub const STR8_DELAY_PARAM_FILTER_FREQ: u64 = STATE_FILTER_FREQ as u64;
pub const STR8_DELAY_PARAM_FILTER_Q: u64 = STATE_FILTER_Q as u64;
pub const STR8_DELAY_PARAM_MOD_RATE: u64 = STATE_MOD_RATE as u64;
pub const STR8_DELAY_PARAM_MOD_AMOUNT: u64 = STATE_MOD_AMOUNT as u64;
pub const STR8_DELAY_PARAM_MOD_PHASE: u64 = STATE_MOD_PHASE as u64;
pub const STR8_DELAY_PARAM_BPM: u64 = STATE_BPM as u64;

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

#[inline]
fn synced_ms(div_idx: f32, offset: f32, bpm: f32) -> f32 {
    let idx = (div_idx.round() as usize).min(SYNC_BEATS.len() - 1);
    let beats = SYNC_BEATS[idx];
    let base_ms = beats * 60.0 / bpm.max(20.0) * 1000.0;
    base_ms * (1.0 + offset.clamp(-0.5, 0.5))
}

#[inline]
fn target_samples(sync: f32, div: f32, offset: f32, free_ms: f32, bpm: f32, sr: f32) -> f32 {
    let ms = if sync > 0.5 {
        synced_ms(div, offset, bpm)
    } else {
        free_ms
    };
    (ms * sr / 1000.0).clamp(1.0, (MAX_DELAY_SAMPLES - 1) as f32)
}

#[inline]
fn read_delay(buf: *const f32, write_pos: usize, delay_samples: f32) -> f32 {
    let read_pos =
        (write_pos as f32 - delay_samples + MAX_DELAY_SAMPLES as f32) % MAX_DELAY_SAMPLES as f32;
    let idx = read_pos as usize;
    let frac = read_pos - idx as f32;
    unsafe {
        let s0 = *buf.add(idx % MAX_DELAY_SAMPLES);
        let s1 = *buf.add((idx + 1) % MAX_DELAY_SAMPLES);
        s0 + frac * (s1 - s0)
    }
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

#[inline]
fn lowpass_coeffs(freq: f32, sr: f32) -> BiquadCoeffs {
    let omega = std::f32::consts::TAU * freq.clamp(20.0, sr * 0.45) / sr.max(1.0);
    let sin = omega.sin();
    let cos = omega.cos();
    let alpha = sin / (2.0 * std::f32::consts::FRAC_1_SQRT_2);
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
fn highpass_coeffs(freq: f32, sr: f32) -> BiquadCoeffs {
    let omega = std::f32::consts::TAU * freq.clamp(20.0, sr * 0.45) / sr.max(1.0);
    let sin = omega.sin();
    let cos = omega.cos();
    let alpha = sin / (2.0 * std::f32::consts::FRAC_1_SQRT_2);
    let b0 = (1.0 + cos) * 0.5;
    let b1 = -(1.0 + cos);
    let b2 = (1.0 + cos) * 0.5;
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
fn filter_edges(center: f32, width_octaves: f32, sr: f32) -> (f32, f32) {
    let center = center.clamp(20.0, 20_000.0);
    let spread = 2.0_f32.powf(width_octaves.clamp(0.25, 6.0) * 0.5);
    let low = (center / spread).clamp(20.0, sr * 0.42);
    let high = (center * spread).clamp((low * 1.05).max(21.0), sr * 0.45);
    (low, high)
}

#[inline]
fn passband_sample(
    input: f32,
    center: f32,
    width_octaves: f32,
    sr: f32,
    hp_z1: &mut f32,
    hp_z2: &mut f32,
    lp_z1: &mut f32,
    lp_z2: &mut f32,
) -> f32 {
    let (low, high) = filter_edges(center, width_octaves, sr);
    let highpassed = biquad_sample(input, highpass_coeffs(low, sr), hp_z1, hp_z2);
    biquad_sample(highpassed, lowpass_coeffs(high, sr), lp_z1, lp_z2)
}

unsafe extern "C" fn str8_delay_init(
    state: *mut c_void,
    sample_rate: c_int,
    _max_block: c_int,
    _initial_state: *const c_void,
) {
    let s = state as *mut f32;
    *s.add(STATE_ENABLED) = 0.0;
    *s.add(STATE_WET) = 0.5;
    *s.add(STATE_FEEDBACK) = 0.5;
    *s.add(STATE_LEFT_SYNC) = 1.0;
    *s.add(STATE_LEFT_DIV) = 6.0;
    *s.add(STATE_LEFT_OFFSET) = 0.0;
    *s.add(STATE_LEFT_TIME_MS) = 250.0;
    *s.add(STATE_RIGHT_SYNC) = 1.0;
    *s.add(STATE_RIGHT_DIV) = 6.0;
    *s.add(STATE_RIGHT_OFFSET) = 0.0;
    *s.add(STATE_RIGHT_TIME_MS) = 250.0;
    *s.add(STATE_FILTER_FREQ) = 1140.0;
    *s.add(STATE_FILTER_Q) = 4.5;
    *s.add(STATE_MOD_RATE) = 0.5;
    *s.add(STATE_MOD_AMOUNT) = 0.0;
    *s.add(STATE_MOD_PHASE) = 0.5;
    *s.add(STATE_BPM) = 120.0;
    *s.add(STATE_SAMPLE_RATE) = sample_rate as f32;
    *s.add(STATE_SMOOTH_WET) = 0.5;
    *s.add(STATE_SMOOTH_FEEDBACK) = 0.5;
    *s.add(STATE_SMOOTH_LEFT_SAMPLES) =
        target_samples(1.0, 6.0, 0.0, 250.0, 120.0, sample_rate as f32);
    *s.add(STATE_SMOOTH_RIGHT_SAMPLES) =
        target_samples(1.0, 6.0, 0.0, 250.0, 120.0, sample_rate as f32);
    *s.add(STATE_SMOOTH_FILTER_FREQ) = 1140.0;
    *s.add(STATE_SMOOTH_FILTER_Q) = 4.5;
    *s.add(STATE_SMOOTH_MOD_AMOUNT) = 0.0;
    *s.add(STATE_WRITE_POS_L) = 0.0;
    *s.add(STATE_WRITE_POS_R) = 0.0;
    *s.add(STATE_LFO_PHASE) = 0.0;
    *s.add(STATE_HP_Z1_L) = 0.0;
    *s.add(STATE_HP_Z2_L) = 0.0;
    *s.add(STATE_LP_Z1_L) = 0.0;
    *s.add(STATE_LP_Z2_L) = 0.0;
    *s.add(STATE_HP_Z1_R) = 0.0;
    *s.add(STATE_HP_Z2_R) = 0.0;
    *s.add(STATE_LP_Z1_R) = 0.0;
    *s.add(STATE_LP_Z2_R) = 0.0;
    for i in STATE_BUF_OFFSET..STATE_END {
        *s.add(i) = 0.0;
    }
}

unsafe extern "C" fn str8_delay_process(
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
        *s.add(STATE_HP_Z1_L) = 0.0;
        *s.add(STATE_HP_Z2_L) = 0.0;
        *s.add(STATE_LP_Z1_L) = 0.0;
        *s.add(STATE_LP_Z2_L) = 0.0;
        *s.add(STATE_HP_Z1_R) = 0.0;
        *s.add(STATE_HP_Z2_R) = 0.0;
        *s.add(STATE_LP_Z1_R) = 0.0;
        *s.add(STATE_LP_Z2_R) = 0.0;
        return;
    }

    let sr = (*s.add(STATE_SAMPLE_RATE)).max(1.0);
    let bpm = *s.add(STATE_BPM);
    let target_wet = (*s.add(STATE_WET)).clamp(0.0, 1.0);
    let target_feedback = (*s.add(STATE_FEEDBACK)).clamp(0.0, 0.95);
    let target_left = target_samples(
        *s.add(STATE_LEFT_SYNC),
        *s.add(STATE_LEFT_DIV),
        *s.add(STATE_LEFT_OFFSET),
        *s.add(STATE_LEFT_TIME_MS),
        bpm,
        sr,
    );
    let target_right = target_samples(
        *s.add(STATE_RIGHT_SYNC),
        *s.add(STATE_RIGHT_DIV),
        *s.add(STATE_RIGHT_OFFSET),
        *s.add(STATE_RIGHT_TIME_MS),
        bpm,
        sr,
    );
    let target_filter_freq = (*s.add(STATE_FILTER_FREQ)).clamp(20.0, 20_000.0);
    let target_filter_width = (*s.add(STATE_FILTER_Q)).clamp(0.25, 6.0);
    let mod_rate = (*s.add(STATE_MOD_RATE)).clamp(0.01, 20.0);
    let mod_phase_offset = *s.add(STATE_MOD_PHASE);

    let mut smooth_wet = *s.add(STATE_SMOOTH_WET);
    let mut smooth_feedback = *s.add(STATE_SMOOTH_FEEDBACK);
    let mut smooth_left = *s.add(STATE_SMOOTH_LEFT_SAMPLES);
    let mut smooth_right = *s.add(STATE_SMOOTH_RIGHT_SAMPLES);
    let mut smooth_filter_freq = *s.add(STATE_SMOOTH_FILTER_FREQ);
    let mut smooth_filter_width = *s.add(STATE_SMOOTH_FILTER_Q);
    let mut smooth_mod_amount = *s.add(STATE_SMOOTH_MOD_AMOUNT);
    let mut write_pos_l = (*s.add(STATE_WRITE_POS_L)) as usize;
    let mut write_pos_r = (*s.add(STATE_WRITE_POS_R)) as usize;
    let mut lfo_phase = *s.add(STATE_LFO_PHASE);
    let mut hp_z1_l = *s.add(STATE_HP_Z1_L);
    let mut hp_z2_l = *s.add(STATE_HP_Z2_L);
    let mut lp_z1_l = *s.add(STATE_LP_Z1_L);
    let mut lp_z2_l = *s.add(STATE_LP_Z2_L);
    let mut hp_z1_r = *s.add(STATE_HP_Z1_R);
    let mut hp_z2_r = *s.add(STATE_HP_Z2_R);
    let mut lp_z1_r = *s.add(STATE_LP_Z1_R);
    let mut lp_z2_r = *s.add(STATE_LP_Z2_R);

    let buf_l = s.add(STATE_BUF_OFFSET);
    let buf_r = s.add(STATE_BUF_R_OFFSET);
    let smooth_coeff = 1.0 - (-2.0 * std::f32::consts::PI * 20.0 / sr).exp();
    let lfo_inc = mod_rate / sr;

    for i in 0..nf {
        smooth_wet += smooth_coeff * (target_wet - smooth_wet);
        smooth_feedback += smooth_coeff * (target_feedback - smooth_feedback);
        smooth_left += smooth_coeff * (target_left - smooth_left);
        smooth_right += smooth_coeff * (target_right - smooth_right);
        smooth_filter_freq += smooth_coeff * (target_filter_freq - smooth_filter_freq);
        smooth_filter_width += smooth_coeff * (target_filter_width - smooth_filter_width);
        smooth_mod_amount +=
            smooth_coeff * ((*s.add(STATE_MOD_AMOUNT)).clamp(0.0, 1.0) - smooth_mod_amount);

        lfo_phase = (lfo_phase + lfo_inc).fract();
        let lfo_l = (lfo_phase * std::f32::consts::TAU).sin();
        let lfo_r = ((lfo_phase + mod_phase_offset).fract() * std::f32::consts::TAU).sin();
        let depth = smooth_mod_amount.clamp(0.0, 1.0) * 0.25;
        let delay_l =
            (smooth_left * (1.0 + lfo_l * depth)).clamp(1.0, (MAX_DELAY_SAMPLES - 1) as f32);
        let delay_r =
            (smooth_right * (1.0 + lfo_r * depth)).clamp(1.0, (MAX_DELAY_SAMPLES - 1) as f32);

        let delayed_l = read_delay(buf_l as *const f32, write_pos_l, delay_l);
        let delayed_r = read_delay(buf_r as *const f32, write_pos_r, delay_r);
        let filtered_l = passband_sample(
            delayed_l,
            smooth_filter_freq,
            smooth_filter_width,
            sr,
            &mut hp_z1_l,
            &mut hp_z2_l,
            &mut lp_z1_l,
            &mut lp_z2_l,
        );
        let filtered_r = passband_sample(
            delayed_r,
            smooth_filter_freq,
            smooth_filter_width,
            sr,
            &mut hp_z1_r,
            &mut hp_z2_r,
            &mut lp_z1_r,
            &mut lp_z2_r,
        );

        let input_l = *in0.add(i);
        let input_r = *in1.add(i);
        *buf_l.add(write_pos_l) = input_l + filtered_l * smooth_feedback;
        *buf_r.add(write_pos_r) = input_r + filtered_r * smooth_feedback;

        let wet = smooth_wet.clamp(0.0, 1.0);
        *out0.add(i) = input_l * (1.0 - wet) + filtered_l * wet;
        *out1.add(i) = input_r * (1.0 - wet) + filtered_r * wet;

        write_pos_l = (write_pos_l + 1) % MAX_DELAY_SAMPLES;
        write_pos_r = (write_pos_r + 1) % MAX_DELAY_SAMPLES;
    }

    *s.add(STATE_SMOOTH_WET) = smooth_wet;
    *s.add(STATE_SMOOTH_FEEDBACK) = smooth_feedback;
    *s.add(STATE_SMOOTH_LEFT_SAMPLES) = smooth_left;
    *s.add(STATE_SMOOTH_RIGHT_SAMPLES) = smooth_right;
    *s.add(STATE_SMOOTH_FILTER_FREQ) = smooth_filter_freq;
    *s.add(STATE_SMOOTH_FILTER_Q) = smooth_filter_width;
    *s.add(STATE_SMOOTH_MOD_AMOUNT) = smooth_mod_amount;
    *s.add(STATE_WRITE_POS_L) = write_pos_l as f32;
    *s.add(STATE_WRITE_POS_R) = write_pos_r as f32;
    *s.add(STATE_LFO_PHASE) = lfo_phase;
    *s.add(STATE_HP_Z1_L) = hp_z1_l;
    *s.add(STATE_HP_Z2_L) = hp_z2_l;
    *s.add(STATE_LP_Z1_L) = lp_z1_l;
    *s.add(STATE_LP_Z2_L) = lp_z2_l;
    *s.add(STATE_HP_Z1_R) = hp_z1_r;
    *s.add(STATE_HP_Z2_R) = hp_z2_r;
    *s.add(STATE_LP_Z1_R) = lp_z1_r;
    *s.add(STATE_LP_Z2_R) = lp_z2_r;
}

pub fn str8_delay_vtable() -> NodeVTable {
    NodeVTable {
        process: Some(str8_delay_process),
        init: Some(str8_delay_init),
        reset: None,
        migrate: None,
    }
}
