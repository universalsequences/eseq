use crate::audiograph::NodeVTable;
use std::os::raw::{c_int, c_void};

const STATE_VOLUME: usize = 0;
const STATE_PAN: usize = 1;
const STATE_SMOOTH_L: usize = 2;
const STATE_SMOOTH_R: usize = 3;
const STATE_SAMPLE_RATE: usize = 4;
pub const STATE_PEAK_L: usize = 5;
pub const STATE_PEAK_R: usize = 6;
const STATE_MUTE: usize = 7;
const STATE_MUTED_BY_SOLO: usize = 8;

pub const STEREO_PANNER_STATE_SIZE: usize = 9;

pub const STEREO_PANNER_PARAM_VOLUME: u64 = STATE_VOLUME as u64;
pub const STEREO_PANNER_PARAM_PAN: u64 = STATE_PAN as u64;
pub const STEREO_PANNER_PARAM_MUTE: u64 = STATE_MUTE as u64;
pub const STEREO_PANNER_PARAM_MUTED_BY_SOLO: u64 = STATE_MUTED_BY_SOLO as u64;

fn gains_for(volume: f32, pan: f32) -> (f32, f32) {
    let angle = (pan.clamp(-1.0, 1.0) + 1.0) * 0.25 * std::f32::consts::PI;
    (volume.max(0.0) * angle.cos(), volume.max(0.0) * angle.sin())
}

fn balance_gains_for(volume: f32, pan: f32) -> (f32, f32) {
    let pan = pan.clamp(-1.0, 1.0);
    if pan >= 0.0 {
        (volume.max(0.0) * (1.0 - pan), volume.max(0.0))
    } else {
        (volume.max(0.0), volume.max(0.0) * (1.0 + pan))
    }
}

unsafe extern "C" fn stereo_panner_init(
    state: *mut c_void,
    sample_rate: c_int,
    _max_block: c_int,
    _initial_state: *const c_void,
) {
    let s = state as *mut f32;
    *s.add(STATE_VOLUME) = 1.0;
    *s.add(STATE_PAN) = 0.0;
    *s.add(STATE_SAMPLE_RATE) = sample_rate as f32;
    *s.add(STATE_MUTE) = 0.0;
    *s.add(STATE_MUTED_BY_SOLO) = 0.0;
    let (gain_l, gain_r) = gains_for(1.0, 0.0);
    *s.add(STATE_SMOOTH_L) = gain_l;
    *s.add(STATE_SMOOTH_R) = gain_r;
    *s.add(STATE_PEAK_L) = 0.0;
    *s.add(STATE_PEAK_R) = 0.0;
}

unsafe extern "C" fn stereo_panner_process(
    inp: *const *mut f32,
    out: *const *mut f32,
    nframes: c_int,
    state: *mut c_void,
    _buffers: *mut c_void,
) {
    let s = state as *mut f32;
    let volume = *s.add(STATE_VOLUME);
    let pan = *s.add(STATE_PAN);
    let muted = *s.add(STATE_MUTE) >= 0.5 || *s.add(STATE_MUTED_BY_SOLO) >= 0.5;
    let sample_rate = (*s.add(STATE_SAMPLE_RATE)).max(1.0);
    let mut smooth_l = *s.add(STATE_SMOOTH_L);
    let mut smooth_r = *s.add(STATE_SMOOTH_R);
    let prev_peak_l = *s.add(STATE_PEAK_L);
    let prev_peak_r = *s.add(STATE_PEAK_R);
    let smooth_coeff = 1.0 - (-2.0 * std::f32::consts::PI * 60.0 / sample_rate).exp();
    let mut peak_l = 0.0f32;
    let mut peak_r = 0.0f32;

    let in0 = *inp.add(0);
    let in1 = *inp.add(1);
    let out0 = *out.add(0);
    let out1 = *out.add(1);

    let (target_l, target_r) = if muted {
        (0.0, 0.0)
    } else {
        balance_gains_for(volume, pan)
    };

    for i in 0..nframes as usize {
        smooth_l += smooth_coeff * (target_l - smooth_l);
        smooth_r += smooth_coeff * (target_r - smooth_r);
        let sample_l = *in0.add(i) * smooth_l;
        let sample_r = *in1.add(i) * smooth_r;
        *out0.add(i) = sample_l;
        *out1.add(i) = sample_r;
        peak_l = peak_l.max(sample_l.abs());
        peak_r = peak_r.max(sample_r.abs());
    }

    *s.add(STATE_SMOOTH_L) = smooth_l;
    *s.add(STATE_SMOOTH_R) = smooth_r;
    *s.add(STATE_PEAK_L) = peak_l.max(prev_peak_l * 0.92);
    *s.add(STATE_PEAK_R) = peak_r.max(prev_peak_r * 0.92);
}

pub fn stereo_panner_vtable() -> NodeVTable {
    NodeVTable {
        process: Some(stereo_panner_process),
        init: Some(stereo_panner_init),
        reset: None,
        migrate: None,
        ..NodeVTable::default()
    }
}
