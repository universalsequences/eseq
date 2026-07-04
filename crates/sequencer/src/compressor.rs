use crate::audiograph::NodeVTable;
use std::os::raw::{c_int, c_void};

const STATE_ENABLED: usize = 0;
const STATE_THRESHOLD_DB: usize = 1;
const STATE_RATIO: usize = 2;
const STATE_ATTACK_MS: usize = 3;
const STATE_RELEASE_MS: usize = 4;
const STATE_MAKEUP_DB: usize = 5;
const STATE_MIX: usize = 6;
const STATE_SAMPLE_RATE: usize = 7;
const STATE_ENV: usize = 8;
const STATE_GAIN_DB: usize = 9;
pub const COMPRESSOR_STATE_SIZE: usize = 10;

pub const COMPRESSOR_PARAM_ENABLED: u64 = STATE_ENABLED as u64;
pub const COMPRESSOR_PARAM_THRESHOLD_DB: u64 = STATE_THRESHOLD_DB as u64;
pub const COMPRESSOR_PARAM_RATIO: u64 = STATE_RATIO as u64;
pub const COMPRESSOR_PARAM_ATTACK_MS: u64 = STATE_ATTACK_MS as u64;
pub const COMPRESSOR_PARAM_RELEASE_MS: u64 = STATE_RELEASE_MS as u64;
pub const COMPRESSOR_PARAM_MAKEUP_DB: u64 = STATE_MAKEUP_DB as u64;
pub const COMPRESSOR_PARAM_MIX: u64 = STATE_MIX as u64;

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

#[inline]
fn gain_reduction_db(input_db: f32, threshold_db: f32, ratio: f32) -> f32 {
    let over = input_db - threshold_db;
    if over <= 0.0 {
        0.0
    } else {
        -over * (1.0 - 1.0 / ratio.max(1.0))
    }
}

unsafe extern "C" fn compressor_init(
    state: *mut c_void,
    sample_rate: c_int,
    _max_block: c_int,
    _initial_state: *const c_void,
) {
    let s = state as *mut f32;
    *s.add(STATE_ENABLED) = 1.0;
    *s.add(STATE_THRESHOLD_DB) = -18.0;
    *s.add(STATE_RATIO) = 4.0;
    *s.add(STATE_ATTACK_MS) = 10.0;
    *s.add(STATE_RELEASE_MS) = 120.0;
    *s.add(STATE_MAKEUP_DB) = 0.0;
    *s.add(STATE_MIX) = 1.0;
    *s.add(STATE_SAMPLE_RATE) = sample_rate as f32;
    *s.add(STATE_ENV) = 0.0;
    *s.add(STATE_GAIN_DB) = 0.0;
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
    let out0 = *out.add(0);
    let out1 = *out.add(1);

    if *s.add(STATE_ENABLED) <= 0.5 {
        std::ptr::copy_nonoverlapping(in0 as *const f32, out0, nf);
        std::ptr::copy_nonoverlapping(in1 as *const f32, out1, nf);
        *s.add(STATE_ENV) = 0.0;
        *s.add(STATE_GAIN_DB) = 0.0;
        return;
    }

    let threshold = (*s.add(STATE_THRESHOLD_DB)).clamp(-60.0, 0.0);
    let ratio = (*s.add(STATE_RATIO)).clamp(1.0, 20.0);
    let attack = time_coef(
        (*s.add(STATE_ATTACK_MS)).clamp(0.1, 200.0),
        *s.add(STATE_SAMPLE_RATE),
    );
    let release = time_coef(
        (*s.add(STATE_RELEASE_MS)).clamp(5.0, 2000.0),
        *s.add(STATE_SAMPLE_RATE),
    );
    let makeup = db_to_amp((*s.add(STATE_MAKEUP_DB)).clamp(-24.0, 24.0));
    let mix = (*s.add(STATE_MIX)).clamp(0.0, 1.0);
    let gain_smooth = time_coef(2.0, *s.add(STATE_SAMPLE_RATE));
    let mut env = *s.add(STATE_ENV);
    let mut gain_db = *s.add(STATE_GAIN_DB);

    for i in 0..nf {
        let input_l = *in0.add(i);
        let input_r = *in1.add(i);
        let detector = input_l.abs().max(input_r.abs());
        let coef = if detector > env { attack } else { release };
        env += coef * (detector - env);
        let target_gain_db = gain_reduction_db(amp_to_db(env), threshold, ratio);
        gain_db += gain_smooth * (target_gain_db - gain_db);
        let gain = db_to_amp(gain_db) * makeup;
        let wet_l = input_l * gain;
        let wet_r = input_r * gain;
        *out0.add(i) = input_l + (wet_l - input_l) * mix;
        *out1.add(i) = input_r + (wet_r - input_r) * mix;
    }

    *s.add(STATE_ENV) = env;
    *s.add(STATE_GAIN_DB) = gain_db;
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
