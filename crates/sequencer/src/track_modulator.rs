use std::os::raw::{c_int, c_void};

use crate::audiograph::NodeVTable;
use crate::effects::{EffectDescriptor, ParamDescriptor, ParamKind, ParamScaling};

const STATE_VALUE: usize = 0;
const STATE_SAMPLE_RATE: usize = 1;
const STATE_GATE: usize = 2;
const STATE_RISE_MS: usize = 3;
const STATE_FALL_MS: usize = 4;
const STATE_PULSE_SAMPLES: usize = 5;
const STATE_TRIGGER: usize = 6;
const STATE_PULSE_REMAINING: usize = 7;
const STATE_PULSE_LEVEL: usize = 8;

pub const PARAM_GATE: u64 = STATE_GATE as u64;
pub const PARAM_RISE_MS: u64 = STATE_RISE_MS as u64;
pub const PARAM_FALL_MS: u64 = STATE_FALL_MS as u64;
pub const PARAM_PULSE_SAMPLES: u64 = STATE_PULSE_SAMPLES as u64;
pub const PARAM_TRIGGER: u64 = STATE_TRIGGER as u64;
pub const PARAM_PULSE_LEVEL: u64 = STATE_PULSE_LEVEL as u64;

pub const MODULATOR_ENVELOPE_STATE_SIZE: usize = 9;
pub const MOD_IN_CLIP_STATE_SIZE: usize = 0;

unsafe extern "C" fn modulator_envelope_init(
    state: *mut c_void,
    sample_rate: c_int,
    _max_block: c_int,
    _init_msg: *const c_void,
) {
    let s = state as *mut f32;
    *s.add(STATE_VALUE) = 0.0;
    *s.add(STATE_SAMPLE_RATE) = (sample_rate as f32).max(1.0);
    *s.add(STATE_GATE) = 0.0;
    *s.add(STATE_RISE_MS) = 1.0;
    *s.add(STATE_FALL_MS) = 1.0;
    *s.add(STATE_PULSE_SAMPLES) = 1.0;
    *s.add(STATE_TRIGGER) = 0.0;
    *s.add(STATE_PULSE_REMAINING) = 0.0;
    *s.add(STATE_PULSE_LEVEL) = 1.0;
}

fn slew_coeff(time_ms: f32, sample_rate: f32) -> f32 {
    let samples = (time_ms.max(0.0) * 0.001 * sample_rate).max(1.0);
    (1.0 / samples).clamp(0.0, 1.0)
}

unsafe extern "C" fn modulator_envelope_process(
    _inp: *const *mut f32,
    out: *const *mut f32,
    nframes: c_int,
    state: *mut c_void,
    _buffers: *mut c_void,
) {
    let nf = nframes as usize;
    let s = state as *mut f32;
    let out0 = *out.add(0);
    let mut value = *s.add(STATE_VALUE);
    let sample_rate = (*s.add(STATE_SAMPLE_RATE)).max(1.0);
    let rise = (*s.add(STATE_RISE_MS)).clamp(0.0, 60_000.0);
    let fall = (*s.add(STATE_FALL_MS)).clamp(0.0, 60_000.0);
    let pulse_samples = (*s.add(STATE_PULSE_SAMPLES)).max(1.0);
    let pulse_level = (*s.add(STATE_PULSE_LEVEL)).clamp(0.0, 1.0);
    let mut pulse_remaining = (*s.add(STATE_PULSE_REMAINING)).max(0.0);
    if *s.add(STATE_TRIGGER) > 0.0 {
        pulse_remaining = pulse_samples;
        *s.add(STATE_TRIGGER) = 0.0;
    }

    let up_coeff = slew_coeff(rise, sample_rate);
    let down_coeff = slew_coeff(fall, sample_rate);
    for i in 0..nf {
        let gate = if pulse_remaining > 0.0 {
            pulse_level
        } else {
            (*s.add(STATE_GATE)).clamp(0.0, 1.0)
        };
        let coeff = if gate >= value { up_coeff } else { down_coeff };
        value += (gate - value) * coeff;
        value = value.clamp(0.0, 1.0);
        *out0.add(i) = value;
        pulse_remaining = (pulse_remaining - 1.0).max(0.0);
    }
    *s.add(STATE_VALUE) = value;
    *s.add(STATE_PULSE_REMAINING) = pulse_remaining;
}

unsafe extern "C" fn mod_in_clip_process(
    inp: *const *mut f32,
    out: *const *mut f32,
    nframes: c_int,
    _state: *mut c_void,
    _buffers: *mut c_void,
) {
    let input = *inp.add(0);
    let output = *out.add(0);
    for i in 0..nframes as usize {
        *output.add(i) = (*input.add(i)).clamp(0.0, 1.0);
    }
}

pub fn modulator_envelope_vtable() -> NodeVTable {
    NodeVTable {
        process: Some(modulator_envelope_process),
        init: Some(modulator_envelope_init),
        reset: None,
        migrate: None,
    }
}

pub fn mod_in_clip_vtable() -> NodeVTable {
    NodeVTable {
        process: Some(mod_in_clip_process),
        init: None,
        reset: None,
        migrate: None,
    }
}

pub fn descriptor() -> EffectDescriptor {
    EffectDescriptor {
        name: "Modulator".to_string(),
        input_channels: 0,
        output_channels: 0,
        params: vec![
            ParamDescriptor {
                name: "rise".to_string(),
                min: 0.0,
                max: 5000.0,
                default: 1.0,
                kind: ParamKind::Continuous {
                    unit: Some("ms".to_string()),
                },
                scaling: ParamScaling::Exponential,
                node_param_idx: PARAM_RISE_MS as u32,
                node_param_span: 1,
                host_control: None,
            },
            ParamDescriptor {
                name: "fall".to_string(),
                min: 0.0,
                max: 5000.0,
                default: 1.0,
                kind: ParamKind::Continuous {
                    unit: Some("ms".to_string()),
                },
                scaling: ParamScaling::Exponential,
                node_param_idx: PARAM_FALL_MS as u32,
                node_param_span: 1,
                host_control: None,
            },
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_rise_and_fall_generates_finite_pulse() {
        let mut state = [0.0f32; MODULATOR_ENVELOPE_STATE_SIZE];
        unsafe {
            modulator_envelope_init(state.as_mut_ptr().cast(), 48_000, 16, std::ptr::null());
        }
        state[PARAM_RISE_MS as usize] = 0.0;
        state[PARAM_FALL_MS as usize] = 0.0;
        state[PARAM_PULSE_SAMPLES as usize] = 4.0;
        state[PARAM_PULSE_LEVEL as usize] = 0.5;
        state[PARAM_TRIGGER as usize] = 1.0;

        let mut out = [0.0f32; 8];
        let out_ptrs = [out.as_mut_ptr()];
        unsafe {
            modulator_envelope_process(
                std::ptr::null(),
                out_ptrs.as_ptr(),
                out.len() as c_int,
                state.as_mut_ptr().cast(),
                std::ptr::null_mut(),
            );
        }

        assert_eq!(&out[..], &[0.5, 0.5, 0.5, 0.5, 0.0, 0.0, 0.0, 0.0]);
    }
}
