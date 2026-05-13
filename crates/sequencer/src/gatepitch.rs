use std::os::raw::{c_int, c_void};

use crate::audiograph::NodeVTable;

// State layout: [gate, pitch_hz, velocity, trigger]
pub const GATEPITCH_STATE_SIZE: usize = 4;
pub const PARAM_GATE: u64 = 0;
pub const PARAM_PITCH: u64 = 1;
pub const PARAM_VELOCITY: u64 = 2;
pub const PARAM_TRIGGER: u64 = 3;

unsafe extern "C" fn gatepitch_init(
    state: *mut c_void,
    _sample_rate: c_int,
    _max_block: c_int,
    _initial_state: *const c_void,
) {
    let s = state as *mut f32;
    *s.add(0) = 0.0; // gate off
    *s.add(1) = 440.0; // default pitch
    *s.add(2) = 1.0; // default velocity
    *s.add(3) = 0.0; // trigger pulse
}

unsafe extern "C" fn gatepitch_process(
    _inp: *const *mut f32,
    out: *const *mut f32,
    nframes: c_int,
    state: *mut c_void,
    _buffers: *mut c_void,
) {
    let s = state as *const f32;
    let gate = *s.add(0);
    let pitch = *s.add(1);
    let velocity = *s.add(2);
    let trigger = *s.add(3);
    let nf = nframes as usize;
    let out0 = *out.add(0); // gate output
    let out1 = *out.add(1); // pitch output
    let out2 = *out.add(2); // velocity output
    let out3 = *out.add(3); // trigger output
    for i in 0..nf {
        *out0.add(i) = gate;
        *out1.add(i) = pitch;
        *out2.add(i) = velocity;
        *out3.add(i) = if i == 0 { trigger } else { 0.0 };
    }
    *(state as *mut f32).add(3) = 0.0;
}

pub fn gatepitch_vtable() -> NodeVTable {
    NodeVTable {
        process: Some(gatepitch_process),
        init: Some(gatepitch_init),
        reset: None,
        migrate: None,
    }
}
