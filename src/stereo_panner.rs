use crate::audiograph::NodeVTable;
use std::os::raw::{c_int, c_void};

const STATE_VOLUME: usize = 0;
const STATE_PAN: usize = 1;
const STATE_SMOOTH_L: usize = 2;
const STATE_SMOOTH_R: usize = 3;
const STATE_SAMPLE_RATE: usize = 4;

pub const STEREO_PANNER_STATE_SIZE: usize = 5;

pub const STEREO_PANNER_PARAM_VOLUME: u64 = STATE_VOLUME as u64;
pub const STEREO_PANNER_PARAM_PAN: u64 = STATE_PAN as u64;

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
    let (gain_l, gain_r) = gains_for(1.0, 0.0);
    *s.add(STATE_SMOOTH_L) = gain_l;
    *s.add(STATE_SMOOTH_R) = gain_r;
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
    let sample_rate = (*s.add(STATE_SAMPLE_RATE)).max(1.0);
    let mut smooth_l = *s.add(STATE_SMOOTH_L);
    let mut smooth_r = *s.add(STATE_SMOOTH_R);
    let smooth_coeff = 1.0 - (-2.0 * std::f32::consts::PI * 60.0 / sample_rate).exp();

    let in0 = *inp.add(0);
    let in1 = *inp.add(1);
    let out0 = *out.add(0);
    let out1 = *out.add(1);

    let (target_l, target_r) = balance_gains_for(volume, pan);

    for i in 0..nframes as usize {
        smooth_l += smooth_coeff * (target_l - smooth_l);
        smooth_r += smooth_coeff * (target_r - smooth_r);
        *out0.add(i) = *in0.add(i) * smooth_l;
        *out1.add(i) = *in1.add(i) * smooth_r;
    }

    *s.add(STATE_SMOOTH_L) = smooth_l;
    *s.add(STATE_SMOOTH_R) = smooth_r;
}

pub fn stereo_panner_vtable() -> NodeVTable {
    NodeVTable {
        process: Some(stereo_panner_process),
        init: Some(stereo_panner_init),
        reset: None,
        migrate: None,
    }
}
