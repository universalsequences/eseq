use std::os::raw::{c_int, c_void};

use crate::audiograph::{GraphBlockEvent, NodeVTable, GBE_PULSE};
use crate::effects::{EffectDescriptor, ParamDescriptor, ParamKind, ParamScaling};

pub const STATE_VALUE: usize = 0;
const STATE_SAMPLE_RATE: usize = 1;
const STATE_GATE: usize = 2;
const STATE_RISE_MS: usize = 3;
const STATE_FALL_MS: usize = 4;
const STATE_PULSE_SAMPLES: usize = 5;
const STATE_TRIGGER: usize = 6;
const STATE_PULSE_REMAINING: usize = 7;
const STATE_PULSE_LEVEL: usize = 8;
const STATE_PULSE_ELAPSED: usize = 9;
const STATE_FALL_ELAPSED: usize = 10;
const STATE_FALL_SAMPLES: usize = 11;
pub const STATE_DISPLAY_PHASE: usize = 12;
const STATE_ENABLED: usize = 13;
const STATE_TIMELINE_COUNT: usize = 14;
const STATE_TIMELINE_BASE: usize = 15;
const TIMELINE_EVENT_WIDTH: usize = 3;
const TIMELINE_FRAME: usize = 0;
const TIMELINE_PULSE_SAMPLES: usize = 1;
const TIMELINE_PULSE_LEVEL: usize = 2;
pub const MODULATOR_TIMELINE_CAPACITY: usize = crate::sequencer::MAX_STEPS;

pub const PARAM_GATE: u64 = STATE_GATE as u64;
pub const PARAM_RISE_MS: u64 = STATE_RISE_MS as u64;
pub const PARAM_FALL_MS: u64 = STATE_FALL_MS as u64;
pub const PARAM_PULSE_SAMPLES: u64 = STATE_PULSE_SAMPLES as u64;
pub const PARAM_TRIGGER: u64 = STATE_TRIGGER as u64;
pub const PARAM_PULSE_LEVEL: u64 = STATE_PULSE_LEVEL as u64;
pub const PARAM_ENABLED: u64 = STATE_ENABLED as u64;

pub const MODULATOR_ENVELOPE_STATE_SIZE: usize =
    STATE_TIMELINE_BASE + MODULATOR_TIMELINE_CAPACITY * TIMELINE_EVENT_WIDTH;
pub const MOD_IN_CLIP_STATE_SIZE: usize = 0;

const DISPLAY_START_X: f32 = 0.18;
const DISPLAY_END_X: f32 = 0.82;
const DISPLAY_MAX_X: f32 = 0.90;

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
    *s.add(STATE_PULSE_ELAPSED) = 0.0;
    *s.add(STATE_FALL_ELAPSED) = 0.0;
    *s.add(STATE_FALL_SAMPLES) = 0.0;
    *s.add(STATE_DISPLAY_PHASE) = DISPLAY_START_X;
    *s.add(STATE_ENABLED) = 1.0;
    *s.add(STATE_TIMELINE_COUNT) = 0.0;
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
    let enabled = (*s.add(STATE_ENABLED)).clamp(0.0, 1.0);
    let pulse_samples = (*s.add(STATE_PULSE_SAMPLES)).max(1.0);
    let mut pulse_remaining = (*s.add(STATE_PULSE_REMAINING)).max(0.0);
    let mut pulse_elapsed = (*s.add(STATE_PULSE_ELAPSED)).max(0.0);
    let mut fall_elapsed = (*s.add(STATE_FALL_ELAPSED)).max(0.0);
    let mut fall_samples = (*s.add(STATE_FALL_SAMPLES)).max(0.0);
    let mut display_phase = (*s.add(STATE_DISPLAY_PHASE)).clamp(0.0, 1.0);
    if enabled <= 0.5 {
        for i in 0..nf {
            *out0.add(i) = 0.0;
        }
        *s.add(STATE_VALUE) = 0.0;
        *s.add(STATE_TRIGGER) = 0.0;
        *s.add(STATE_PULSE_REMAINING) = 0.0;
        *s.add(STATE_PULSE_ELAPSED) = 0.0;
        *s.add(STATE_FALL_ELAPSED) = 0.0;
        *s.add(STATE_FALL_SAMPLES) = 0.0;
        *s.add(STATE_DISPLAY_PHASE) = DISPLAY_START_X;
        return;
    }
    if *s.add(STATE_TRIGGER) > 0.0 {
        pulse_remaining = pulse_samples;
        pulse_elapsed = 0.0;
        fall_elapsed = 0.0;
        fall_samples = 0.0;
        display_phase = DISPLAY_START_X;
        *s.add(STATE_TRIGGER) = 0.0;
    }

    let up_coeff = slew_coeff(rise, sample_rate);
    let down_coeff = slew_coeff(fall, sample_rate);
    let event_count = (*s.add(STATE_TIMELINE_COUNT))
        .max(0.0)
        .min(MODULATOR_TIMELINE_CAPACITY as f32) as usize;
    let mut event_index = 0usize;
    for i in 0..nf {
        while event_index < event_count {
            let base = STATE_TIMELINE_BASE + event_index * TIMELINE_EVENT_WIDTH;
            let frame = (*s.add(base + TIMELINE_FRAME)).max(0.0) as usize;
            if frame != i {
                break;
            }
            pulse_remaining = (*s.add(base + TIMELINE_PULSE_SAMPLES)).max(1.0);
            pulse_elapsed = 0.0;
            fall_elapsed = 0.0;
            fall_samples = 0.0;
            display_phase = DISPLAY_START_X;
            *s.add(STATE_PULSE_LEVEL) = (*s.add(base + TIMELINE_PULSE_LEVEL)).clamp(0.0, 1.0);
            event_index += 1;
        }
        let was_pulsing = pulse_remaining > 0.0;
        let gate = if pulse_remaining > 0.0 {
            (*s.add(STATE_PULSE_LEVEL)).clamp(0.0, 1.0)
        } else {
            (*s.add(STATE_GATE)).clamp(0.0, 1.0)
        };
        let coeff = if gate >= value { up_coeff } else { down_coeff };
        value += (gate - value) * coeff;
        value = value.clamp(0.0, 1.0);
        *out0.add(i) = value;

        if was_pulsing {
            let t = (pulse_elapsed / pulse_samples).clamp(0.0, 1.0);
            display_phase = DISPLAY_START_X + t * (DISPLAY_END_X - DISPLAY_START_X);
            pulse_elapsed += 1.0;
            pulse_remaining = (pulse_remaining - 1.0).max(0.0);
            if pulse_remaining <= 0.0 {
                fall_elapsed = 0.0;
                fall_samples = (fall * 0.001 * sample_rate).max(1.0);
                if fall <= 0.0 {
                    display_phase = DISPLAY_MAX_X;
                }
            }
        } else if fall_samples > 0.0 && fall_elapsed < fall_samples {
            let t = (fall_elapsed / fall_samples).clamp(0.0, 1.0);
            display_phase = DISPLAY_END_X + t * (DISPLAY_MAX_X - DISPLAY_END_X);
            fall_elapsed += 1.0;
            if fall_elapsed >= fall_samples {
                display_phase = DISPLAY_MAX_X;
            }
        } else if gate > 0.0 {
            display_phase = DISPLAY_END_X;
        } else if value <= 0.0001 {
            display_phase = DISPLAY_START_X;
        }
    }
    *s.add(STATE_VALUE) = value;
    *s.add(STATE_PULSE_REMAINING) = pulse_remaining;
    *s.add(STATE_PULSE_ELAPSED) = pulse_elapsed;
    *s.add(STATE_FALL_ELAPSED) = fall_elapsed;
    *s.add(STATE_FALL_SAMPLES) = fall_samples;
    *s.add(STATE_DISPLAY_PHASE) = display_phase;
}

unsafe extern "C" fn modulator_envelope_begin_event_slice(
    state: *mut c_void,
    _block_serial: u64,
    _slice_start: c_int,
    _slice_nframes: c_int,
) {
    *(state as *mut f32).add(STATE_TIMELINE_COUNT) = 0.0;
}

unsafe extern "C" fn modulator_envelope_schedule_event(
    state: *mut c_void,
    event: *const GraphBlockEvent,
) -> bool {
    if event.is_null() {
        return false;
    }
    let event = &*event;
    if event.kind != GBE_PULSE || event.aux_count < 2 {
        return false;
    }
    let s = state as *mut f32;
    let count = (*s.add(STATE_TIMELINE_COUNT)).max(0.0) as usize;
    if count >= MODULATOR_TIMELINE_CAPACITY {
        return false;
    }
    let base = STATE_TIMELINE_BASE + count * TIMELINE_EVENT_WIDTH;
    *s.add(base + TIMELINE_FRAME) = event.frame_offset as f32;
    *s.add(base + TIMELINE_PULSE_SAMPLES) = event.aux[0].max(1.0);
    *s.add(base + TIMELINE_PULSE_LEVEL) = event.aux[1].clamp(0.0, 1.0);
    *s.add(STATE_TIMELINE_COUNT) = (count + 1) as f32;
    true
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
        begin_event_slice: Some(modulator_envelope_begin_event_slice),
        schedule_event: Some(modulator_envelope_schedule_event),
    }
}

pub fn mod_in_clip_vtable() -> NodeVTable {
    NodeVTable {
        process: Some(mod_in_clip_process),
        init: None,
        reset: None,
        migrate: None,
        ..NodeVTable::default()
    }
}

pub fn descriptor() -> EffectDescriptor {
    EffectDescriptor {
        name: "Modulator".to_string(),
        input_channels: 0,
        output_channels: 0,
        instrument_modulators: Vec::new(),
        instrument_modulation_targets: Vec::new(),
        params: vec![
            EffectDescriptor::enabled_param(PARAM_ENABLED as u32, 1.0),
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
                ui_metadata: None,
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
                ui_metadata: None,
            },
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audiograph::{GraphBlockEvent, GBE_AUX_CAP, GBE_PULSE};

    fn pulse_event(frame_offset: u32, samples: f32, level: f32) -> GraphBlockEvent {
        let mut event = GraphBlockEvent {
            logical_id: 1,
            frame_offset,
            sequence: 0,
            kind: GBE_PULSE,
            aux_count: 2,
            aux: [0.0; GBE_AUX_CAP],
        };
        event.aux[0] = samples;
        event.aux[1] = level;
        event
    }

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

    #[test]
    fn disabled_modulator_outputs_silence_and_resets_display_phase() {
        let mut state = [0.0f32; MODULATOR_ENVELOPE_STATE_SIZE];
        unsafe {
            modulator_envelope_init(state.as_mut_ptr().cast(), 48_000, 16, std::ptr::null());
        }
        state[PARAM_ENABLED as usize] = 0.0;
        state[PARAM_RISE_MS as usize] = 0.0;
        state[PARAM_FALL_MS as usize] = 0.0;
        state[PARAM_PULSE_SAMPLES as usize] = 4.0;
        state[PARAM_PULSE_LEVEL as usize] = 1.0;
        state[PARAM_TRIGGER as usize] = 1.0;

        let mut out = [1.0f32; 8];
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

        assert_eq!(&out[..], &[0.0; 8]);
        assert_eq!(state[STATE_VALUE], 0.0);
        assert_eq!(state[PARAM_TRIGGER as usize], 0.0);
        assert_eq!(state[STATE_DISPLAY_PHASE], DISPLAY_START_X);
    }

    #[test]
    fn display_phase_advances_across_zero_slew_pulse_duration() {
        let mut state = [0.0f32; MODULATOR_ENVELOPE_STATE_SIZE];
        unsafe {
            modulator_envelope_init(state.as_mut_ptr().cast(), 48_000, 16, std::ptr::null());
        }
        state[PARAM_RISE_MS as usize] = 0.0;
        state[PARAM_FALL_MS as usize] = 0.0;
        state[PARAM_PULSE_SAMPLES as usize] = 4.0;
        state[PARAM_PULSE_LEVEL as usize] = 1.0;
        state[PARAM_TRIGGER as usize] = 1.0;

        let mut out = [0.0f32; 2];
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

        let phase = state[STATE_DISPLAY_PHASE];
        assert!(
            phase > DISPLAY_START_X && phase < DISPLAY_END_X,
            "phase should advance through the gate plateau, got {phase}"
        );
    }

    #[test]
    fn scheduled_pulse_starts_at_local_frame() {
        let mut state = [0.0f32; MODULATOR_ENVELOPE_STATE_SIZE];
        unsafe {
            modulator_envelope_init(state.as_mut_ptr().cast(), 48_000, 16, std::ptr::null());
        }
        state[PARAM_RISE_MS as usize] = 0.0;
        state[PARAM_FALL_MS as usize] = 0.0;

        unsafe {
            modulator_envelope_begin_event_slice(state.as_mut_ptr().cast(), 1, 0, 8);
            let event = pulse_event(3, 2.0, 0.5);
            assert!(modulator_envelope_schedule_event(
                state.as_mut_ptr().cast(),
                &event
            ));
        }

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

        assert_eq!(out, [0.0, 0.0, 0.0, 0.5, 0.5, 0.0, 0.0, 0.0]);
    }
}
