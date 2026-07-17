use crate::audiograph::NodeVTable;
use std::os::raw::{c_int, c_void};

// Max delay: 2 seconds at 48kHz = 96000 samples per channel
const MAX_DELAY_SAMPLES: usize = 96000;

// State layout indices (f32 slots)
const STATE_WET: usize = 0;
const STATE_SYNCED: usize = 1;
const STATE_DELAY_TIME: usize = 2; // ms (or sync division index when synced)
const STATE_FEEDBACK: usize = 3;
const STATE_DAMPENING: usize = 4;
const STATE_STEREO_WIDTH: usize = 5;
const STATE_BPM: usize = 6;
const STATE_SAMPLE_RATE: usize = 7;
const STATE_SMOOTH_WET: usize = 8;
const STATE_SMOOTH_TIME: usize = 9;
const STATE_SMOOTH_FB: usize = 10;
const STATE_SMOOTH_DAMP: usize = 11;
const STATE_SMOOTH_WIDTH: usize = 12;
const STATE_WRITE_POS_L: usize = 13;
const STATE_WRITE_POS_R: usize = 14;
const STATE_DAMP_STATE_L: usize = 15;
const STATE_DAMP_STATE_R: usize = 16;
const STATE_BUF_OFFSET: usize = 17;
const STATE_ENABLED: usize = STATE_BUF_OFFSET + MAX_DELAY_SAMPLES * 2;
// L buffer: STATE_BUF_OFFSET .. STATE_BUF_OFFSET + MAX_DELAY_SAMPLES
// R buffer: STATE_BUF_OFFSET + MAX_DELAY_SAMPLES .. STATE_BUF_OFFSET + 2*MAX_DELAY_SAMPLES

pub const DELAY_STATE_SIZE: usize = STATE_ENABLED + 1;

// Param indices for external control
#[allow(dead_code)]
pub const DELAY_PARAM_WET: u64 = STATE_WET as u64;
#[allow(dead_code)]
pub const DELAY_PARAM_SYNCED: u64 = STATE_SYNCED as u64;
#[allow(dead_code)]
pub const DELAY_PARAM_DELAY_TIME: u64 = STATE_DELAY_TIME as u64;
#[allow(dead_code)]
pub const DELAY_PARAM_FEEDBACK: u64 = STATE_FEEDBACK as u64;
#[allow(dead_code)]
pub const DELAY_PARAM_DAMPENING: u64 = STATE_DAMPENING as u64;
#[allow(dead_code)]
pub const DELAY_PARAM_STEREO_WIDTH: u64 = STATE_STEREO_WIDTH as u64;
pub const DELAY_PARAM_BPM: u64 = STATE_BPM as u64;
pub const DELAY_PARAM_ENABLED: u64 = STATE_ENABLED as u64;

unsafe extern "C" fn delay_init(
    state: *mut c_void,
    sample_rate: c_int,
    _max_block: c_int,
    _initial_state: *const c_void,
) {
    let s = state as *mut f32;
    *s.add(STATE_WET) = 0.0;
    *s.add(STATE_SYNCED) = 0.0;
    *s.add(STATE_DELAY_TIME) = 250.0;
    *s.add(STATE_FEEDBACK) = 0.3;
    *s.add(STATE_DAMPENING) = 0.5;
    *s.add(STATE_STEREO_WIDTH) = 1.0;
    *s.add(STATE_BPM) = 120.0;
    *s.add(STATE_SAMPLE_RATE) = sample_rate as f32;
    *s.add(STATE_SMOOTH_WET) = 0.0;
    *s.add(STATE_SMOOTH_TIME) = 250.0;
    *s.add(STATE_SMOOTH_FB) = 0.3;
    *s.add(STATE_SMOOTH_DAMP) = 0.5;
    *s.add(STATE_SMOOTH_WIDTH) = 1.0;
    *s.add(STATE_WRITE_POS_L) = 0.0;
    *s.add(STATE_WRITE_POS_R) = 0.0;
    *s.add(STATE_DAMP_STATE_L) = 0.0;
    *s.add(STATE_DAMP_STATE_R) = 0.0;

    // Zero delay buffers
    for i in 0..(MAX_DELAY_SAMPLES * 2) {
        *s.add(STATE_BUF_OFFSET + i) = 0.0;
    }
    *s.add(STATE_ENABLED) = 0.0;
}

unsafe extern "C" fn delay_process(
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
    let out0 = *out.add(0); // L output
    let out1 = *out.add(1); // R output

    if *s.add(STATE_ENABLED) <= 0.5 {
        std::ptr::copy_nonoverlapping(in0 as *const f32, out0, nf);
        std::ptr::copy_nonoverlapping(in1 as *const f32, out1, nf);
        return;
    }

    let target_wet = *s.add(STATE_WET);
    let synced = *s.add(STATE_SYNCED);
    let target_time = *s.add(STATE_DELAY_TIME);
    let target_fb = *s.add(STATE_FEEDBACK);
    let target_damp = *s.add(STATE_DAMPENING);
    let target_width = *s.add(STATE_STEREO_WIDTH);
    let bpm = *s.add(STATE_BPM);
    let sr = *s.add(STATE_SAMPLE_RATE);

    let mut smooth_wet = *s.add(STATE_SMOOTH_WET);
    let mut smooth_time = *s.add(STATE_SMOOTH_TIME);
    let mut smooth_fb = *s.add(STATE_SMOOTH_FB);
    let mut smooth_damp = *s.add(STATE_SMOOTH_DAMP);
    let mut smooth_width = *s.add(STATE_SMOOTH_WIDTH);
    let mut write_pos_l = (*s.add(STATE_WRITE_POS_L)) as usize;
    let mut write_pos_r = (*s.add(STATE_WRITE_POS_R)) as usize;
    let mut damp_state_l = *s.add(STATE_DAMP_STATE_L);
    let mut damp_state_r = *s.add(STATE_DAMP_STATE_R);

    let buf_l = s.add(STATE_BUF_OFFSET);
    let buf_r = s.add(STATE_BUF_OFFSET + MAX_DELAY_SAMPLES);

    // One-pole smoothing (~20Hz)
    let smooth_coeff = 1.0 - (-2.0 * std::f32::consts::PI * 20.0 / sr).exp();

    // Compute delay in ms from target_time
    let delay_ms = if synced > 0.5 {
        // target_time is sync division index — convert to beats then to ms
        const SYNC_BEATS: [f32; 11] = [
            0.125,     // 1/32
            0.25,      // 1/16
            1.0 / 6.0, // 1/16t
            0.5,       // 1/8
            1.0 / 3.0, // 1/8t
            0.75,      // 1/8.
            1.0,       // 1/4
            2.0 / 3.0, // 1/4t
            1.5,       // 1/4.
            2.0,       // 1/2
            4.0,       // 1
        ];
        let idx = (target_time.round() as usize).min(SYNC_BEATS.len() - 1);
        let beats = SYNC_BEATS[idx];
        let bpm_safe = bpm.max(20.0);
        beats * 60.0 / bpm_safe * 1000.0
    } else {
        target_time
    };
    let target_delay_samples = (delay_ms * sr / 1000.0).clamp(1.0, (MAX_DELAY_SAMPLES - 1) as f32);

    for i in 0..nf {
        smooth_wet += smooth_coeff * (target_wet - smooth_wet);
        smooth_time += smooth_coeff * (target_delay_samples - smooth_time);
        smooth_fb += smooth_coeff * (target_fb - smooth_fb);
        smooth_damp += smooth_coeff * (target_damp - smooth_damp);
        smooth_width += smooth_coeff * (target_width - smooth_width);

        let input_l = *in0.add(i);
        let input_r = *in1.add(i);
        let delay_l = smooth_time;
        let delay_r = (smooth_time * smooth_width).clamp(1.0, (MAX_DELAY_SAMPLES - 1) as f32);

        // Read from circular buffer with linear interpolation — L
        let read_pos_l =
            (write_pos_l as f32 - delay_l + MAX_DELAY_SAMPLES as f32) % MAX_DELAY_SAMPLES as f32;
        let idx_l = read_pos_l as usize;
        let frac_l = read_pos_l - idx_l as f32;
        let s0_l = *buf_l.add(idx_l % MAX_DELAY_SAMPLES);
        let s1_l = *buf_l.add((idx_l + 1) % MAX_DELAY_SAMPLES);
        let delayed_l = s0_l + frac_l * (s1_l - s0_l);

        // Read from circular buffer — R
        let read_pos_r =
            (write_pos_r as f32 - delay_r + MAX_DELAY_SAMPLES as f32) % MAX_DELAY_SAMPLES as f32;
        let idx_r = read_pos_r as usize;
        let frac_r = read_pos_r - idx_r as f32;
        let s0_r = *buf_r.add(idx_r % MAX_DELAY_SAMPLES);
        let s1_r = *buf_r.add((idx_r + 1) % MAX_DELAY_SAMPLES);
        let delayed_r = s0_r + frac_r * (s1_r - s0_r);

        // Dampening: one-pole LP on feedback path
        damp_state_l += smooth_damp * (delayed_l - damp_state_l);
        damp_state_r += smooth_damp * (delayed_r - damp_state_r);

        // Write to buffer
        *buf_l.add(write_pos_l) = input_l + smooth_fb * damp_state_l;
        *buf_r.add(write_pos_r) = input_r + smooth_fb * damp_state_r;

        // Output: dry/wet mix
        let dry = 1.0 - smooth_wet;
        *out0.add(i) = input_l * dry + delayed_l * smooth_wet;
        *out1.add(i) = input_r * dry + delayed_r * smooth_wet;

        write_pos_l = (write_pos_l + 1) % MAX_DELAY_SAMPLES;
        write_pos_r = (write_pos_r + 1) % MAX_DELAY_SAMPLES;
    }

    *s.add(STATE_SMOOTH_WET) = smooth_wet;
    *s.add(STATE_SMOOTH_TIME) = smooth_time;
    *s.add(STATE_SMOOTH_FB) = smooth_fb;
    *s.add(STATE_SMOOTH_DAMP) = smooth_damp;
    *s.add(STATE_SMOOTH_WIDTH) = smooth_width;
    *s.add(STATE_WRITE_POS_L) = write_pos_l as f32;
    *s.add(STATE_WRITE_POS_R) = write_pos_r as f32;
    *s.add(STATE_DAMP_STATE_L) = damp_state_l;
    *s.add(STATE_DAMP_STATE_R) = damp_state_r;
}

pub fn delay_vtable() -> NodeVTable {
    NodeVTable {
        process: Some(delay_process),
        init: Some(delay_init),
        reset: None,
        migrate: None,
        ..NodeVTable::default()
    }
}
