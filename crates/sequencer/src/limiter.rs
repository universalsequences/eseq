use crate::audiograph::NodeVTable;
use std::os::raw::{c_int, c_void};

const MAX_LOOKAHEAD_SAMPLES: usize = 960;

const STATE_ENABLED: usize = 0;
const STATE_INPUT_GAIN_DB: usize = 1;
const STATE_CEILING_DB: usize = 2;
const STATE_RELEASE_MS: usize = 3;
const STATE_LOOKAHEAD_MS: usize = 4;
const STATE_SAMPLE_RATE: usize = 5;
const STATE_GAIN_DB: usize = 6;
const STATE_WRITE_POS: usize = 7;
const STATE_BUF_OFFSET: usize = 8;
pub const LIMITER_STATE_SIZE: usize = STATE_BUF_OFFSET + MAX_LOOKAHEAD_SAMPLES * 2;

pub const LIMITER_PARAM_ENABLED: u64 = STATE_ENABLED as u64;
pub const LIMITER_PARAM_INPUT_GAIN_DB: u64 = STATE_INPUT_GAIN_DB as u64;
pub const LIMITER_PARAM_CEILING_DB: u64 = STATE_CEILING_DB as u64;
pub const LIMITER_PARAM_RELEASE_MS: u64 = STATE_RELEASE_MS as u64;
pub const LIMITER_PARAM_LOOKAHEAD_MS: u64 = STATE_LOOKAHEAD_MS as u64;

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

unsafe extern "C" fn limiter_init(
    state: *mut c_void,
    sample_rate: c_int,
    _max_block: c_int,
    _initial_state: *const c_void,
) {
    let s = state as *mut f32;
    *s.add(STATE_ENABLED) = 1.0;
    *s.add(STATE_INPUT_GAIN_DB) = 0.0;
    *s.add(STATE_CEILING_DB) = -0.3;
    *s.add(STATE_RELEASE_MS) = 100.0;
    *s.add(STATE_LOOKAHEAD_MS) = 3.0;
    *s.add(STATE_SAMPLE_RATE) = sample_rate as f32;
    *s.add(STATE_GAIN_DB) = 0.0;
    *s.add(STATE_WRITE_POS) = 0.0;
    for i in 0..(MAX_LOOKAHEAD_SAMPLES * 2) {
        *s.add(STATE_BUF_OFFSET + i) = 0.0;
    }
}

unsafe extern "C" fn limiter_process(
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
        *s.add(STATE_GAIN_DB) = 0.0;
        return;
    }

    let sr = (*s.add(STATE_SAMPLE_RATE)).max(1.0);
    let input_gain = db_to_amp((*s.add(STATE_INPUT_GAIN_DB)).clamp(-24.0, 24.0));
    let ceiling_db = (*s.add(STATE_CEILING_DB)).clamp(-24.0, 0.0);
    let ceiling_amp = db_to_amp(ceiling_db);
    let release = time_coef((*s.add(STATE_RELEASE_MS)).clamp(1.0, 2000.0), sr);
    let lookahead_samples = ((*s.add(STATE_LOOKAHEAD_MS)).clamp(0.0, 20.0) * sr / 1000.0)
        .round()
        .clamp(0.0, (MAX_LOOKAHEAD_SAMPLES - 1) as f32) as usize;
    let attack = 1.0;
    let mut gain_db = *s.add(STATE_GAIN_DB);
    let mut write_pos = (*s.add(STATE_WRITE_POS) as usize).min(MAX_LOOKAHEAD_SAMPLES - 1);
    let buf_l = s.add(STATE_BUF_OFFSET);
    let buf_r = s.add(STATE_BUF_OFFSET + MAX_LOOKAHEAD_SAMPLES);

    for i in 0..nf {
        let hot_l = *in0.add(i) * input_gain;
        let hot_r = *in1.add(i) * input_gain;
        *buf_l.add(write_pos) = hot_l;
        *buf_r.add(write_pos) = hot_r;

        let detector = hot_l.abs().max(hot_r.abs());
        let needed_gain_db = if detector > ceiling_amp {
            ceiling_db - amp_to_db(detector)
        } else {
            0.0
        };
        let coef = if needed_gain_db < gain_db {
            attack
        } else {
            release
        };
        gain_db += coef * (needed_gain_db - gain_db);

        let read_pos =
            (write_pos + MAX_LOOKAHEAD_SAMPLES - lookahead_samples) % MAX_LOOKAHEAD_SAMPLES;
        let gain = db_to_amp(gain_db);
        let limited_l = *buf_l.add(read_pos) * gain;
        let limited_r = *buf_r.add(read_pos) * gain;
        *out0.add(i) = limited_l.clamp(-ceiling_amp, ceiling_amp);
        *out1.add(i) = limited_r.clamp(-ceiling_amp, ceiling_amp);
        write_pos = (write_pos + 1) % MAX_LOOKAHEAD_SAMPLES;
    }

    *s.add(STATE_GAIN_DB) = gain_db;
    *s.add(STATE_WRITE_POS) = write_pos as f32;
}

pub fn limiter_vtable() -> NodeVTable {
    NodeVTable {
        process: Some(limiter_process),
        init: Some(limiter_init),
        reset: None,
        migrate: None,
    }
}
