use std::os::raw::{c_int, c_void};

use crate::audiograph::NodeVTable;

// State layout: [gate, pitch_hz, velocity, trigger, clock_phase, clock_inc]
pub const GATEPITCH_STATE_SIZE: usize = 6;
pub const OUTPUT_COUNT: usize = 5;
pub const PARAM_GATE: u64 = 0;
pub const PARAM_PITCH: u64 = 1;
pub const PARAM_VELOCITY: u64 = 2;
pub const PARAM_TRIGGER: u64 = 3;
pub const PARAM_CLOCK_PHASE: u64 = 4;
pub const PARAM_CLOCK_INC: u64 = 5;

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
    *s.add(4) = 0.0; // transport bar phase
    *s.add(5) = 0.0; // per-sample clock increment
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
    let mut clock_phase = *s.add(4);
    let clock_inc = *s.add(5);
    let nf = nframes as usize;
    let out0 = *out.add(0); // gate output
    let out1 = *out.add(1); // pitch output
    let out2 = *out.add(2); // velocity output
    let out3 = *out.add(3); // trigger output
    let out4 = *out.add(4); // clock output
    for i in 0..nf {
        *out0.add(i) = gate;
        *out1.add(i) = pitch;
        *out2.add(i) = velocity;
        *out3.add(i) = if i == 0 { trigger } else { 0.0 };
        *out4.add(i) = clock_phase;
        clock_phase += clock_inc;
        if clock_phase >= 1.0 {
            clock_phase -= clock_phase.floor();
        }
    }
    *(state as *mut f32).add(3) = 0.0;
    *(state as *mut f32).add(4) = clock_phase;
}

pub fn gatepitch_vtable() -> NodeVTable {
    NodeVTable {
        process: Some(gatepitch_process),
        init: Some(gatepitch_init),
        reset: None,
        migrate: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gatepitch_clock_outputs_wrapping_bar_phase() {
        let mut state = [0.0_f32; GATEPITCH_STATE_SIZE];
        unsafe {
            gatepitch_init(state.as_mut_ptr().cast(), 48_000, 64, std::ptr::null());
        }
        state[PARAM_CLOCK_PHASE as usize] = 0.75;
        state[PARAM_CLOCK_INC as usize] = 0.125;

        let mut gate = [0.0; 4];
        let mut pitch = [0.0; 4];
        let mut velocity = [0.0; 4];
        let mut trigger = [0.0; 4];
        let mut clock = [0.0; 4];
        let outputs = [
            gate.as_mut_ptr(),
            pitch.as_mut_ptr(),
            velocity.as_mut_ptr(),
            trigger.as_mut_ptr(),
            clock.as_mut_ptr(),
        ];

        unsafe {
            gatepitch_process(
                std::ptr::null(),
                outputs.as_ptr(),
                4,
                state.as_mut_ptr().cast(),
                std::ptr::null_mut(),
            );
        }

        assert_eq!(clock, [0.75, 0.875, 0.0, 0.125]);
        assert_eq!(state[PARAM_CLOCK_PHASE as usize], 0.25);
    }
}
