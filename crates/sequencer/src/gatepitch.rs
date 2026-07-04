use std::os::raw::{c_int, c_void};

use crate::audiograph::{GraphBlockEvent, NodeVTable, GBE_GATE_OFF, GBE_NOTE_ON};

const TIMELINE_EVENT_WIDTH: usize = 4;
const TIMELINE_FRAME: usize = 0;
const TIMELINE_KIND: usize = 1;
const TIMELINE_PITCH: usize = 2;
const TIMELINE_VELOCITY: usize = 3;
pub const GATEPITCH_TIMELINE_CAPACITY: usize = crate::sequencer::MAX_STEPS;
const PARAM_TIMELINE_COUNT: usize = 6;
const PARAM_TIMELINE_BASE: usize = 7;

// State layout starts with the public ParamMsg slots, then a fixed per-slice
// event timeline: [count, event(frame, kind, pitch, velocity) * MAX_STEPS].
pub const GATEPITCH_STATE_SIZE: usize =
    PARAM_TIMELINE_BASE + GATEPITCH_TIMELINE_CAPACITY * TIMELINE_EVENT_WIDTH;
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
    *s.add(PARAM_TIMELINE_COUNT) = 0.0;
}

unsafe extern "C" fn gatepitch_begin_event_slice(
    state: *mut c_void,
    _block_serial: u64,
    _slice_start: c_int,
    _slice_nframes: c_int,
) {
    *(state as *mut f32).add(PARAM_TIMELINE_COUNT) = 0.0;
}

unsafe extern "C" fn gatepitch_schedule_event(
    state: *mut c_void,
    event: *const GraphBlockEvent,
) -> bool {
    if event.is_null() {
        return false;
    }
    let event = &*event;
    let s = state as *mut f32;
    let count = (*s.add(PARAM_TIMELINE_COUNT)).max(0.0) as usize;
    if count >= GATEPITCH_TIMELINE_CAPACITY {
        return false;
    }
    let (pitch, velocity) = match event.kind {
        GBE_NOTE_ON => {
            if event.aux_count < 2 {
                return false;
            }
            (event.aux[0].max(0.0), event.aux[1].clamp(0.0, 1.0))
        }
        GBE_GATE_OFF => (0.0, 0.0),
        _ => return false,
    };

    let base = PARAM_TIMELINE_BASE + count * TIMELINE_EVENT_WIDTH;
    *s.add(base + TIMELINE_FRAME) = event.frame_offset as f32;
    *s.add(base + TIMELINE_KIND) = event.kind as f32;
    *s.add(base + TIMELINE_PITCH) = pitch;
    *s.add(base + TIMELINE_VELOCITY) = velocity;
    *s.add(PARAM_TIMELINE_COUNT) = (count + 1) as f32;
    true
}

unsafe extern "C" fn gatepitch_process(
    _inp: *const *mut f32,
    out: *const *mut f32,
    nframes: c_int,
    state: *mut c_void,
    _buffers: *mut c_void,
) {
    let s = state as *mut f32;
    let mut gate = *s.add(PARAM_GATE as usize);
    let mut pitch = *s.add(PARAM_PITCH as usize);
    let mut velocity = *s.add(PARAM_VELOCITY as usize);
    let mut clock_phase = *s.add(PARAM_CLOCK_PHASE as usize);
    let clock_inc = *s.add(PARAM_CLOCK_INC as usize);
    let event_count = (*s.add(PARAM_TIMELINE_COUNT)).max(0.0) as usize;
    let event_count = event_count.min(GATEPITCH_TIMELINE_CAPACITY);
    let mut event_index = 0usize;
    let nf = nframes as usize;
    let out0 = *out.add(0); // gate output
    let out1 = *out.add(1); // pitch output
    let out2 = *out.add(2); // velocity output
    let out3 = *out.add(3); // trigger output
    let out4 = *out.add(4); // clock output
    for i in 0..nf {
        let mut trigger = 0.0;
        while event_index < event_count {
            let base = PARAM_TIMELINE_BASE + event_index * TIMELINE_EVENT_WIDTH;
            let frame = (*s.add(base + TIMELINE_FRAME)).max(0.0) as usize;
            if frame != i {
                break;
            }
            let kind = *s.add(base + TIMELINE_KIND) as u32;
            if kind == GBE_NOTE_ON {
                pitch = *s.add(base + TIMELINE_PITCH);
                velocity = *s.add(base + TIMELINE_VELOCITY);
                gate = 1.0;
                trigger = 1.0;
            } else if kind == GBE_GATE_OFF {
                gate = 0.0;
            }
            event_index += 1;
        }
        *out0.add(i) = gate;
        *out1.add(i) = pitch;
        *out2.add(i) = velocity;
        *out3.add(i) = trigger;
        *out4.add(i) = clock_phase;
        clock_phase += clock_inc;
        if clock_phase >= 1.0 {
            clock_phase -= clock_phase.floor();
        }
    }
    *s.add(PARAM_GATE as usize) = gate;
    *s.add(PARAM_PITCH as usize) = pitch;
    *s.add(PARAM_VELOCITY as usize) = velocity;
    *s.add(PARAM_TRIGGER as usize) = 0.0;
    *s.add(PARAM_CLOCK_PHASE as usize) = clock_phase;
}

pub fn gatepitch_vtable() -> NodeVTable {
    NodeVTable {
        process: Some(gatepitch_process),
        init: Some(gatepitch_init),
        reset: None,
        migrate: None,
        begin_event_slice: Some(gatepitch_begin_event_slice),
        schedule_event: Some(gatepitch_schedule_event),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audiograph::{GBE_AUX_CAP, GBE_GATE_OFF, GBE_NOTE_ON};

    fn event(frame_offset: u32, kind: u32, aux: &[f32]) -> GraphBlockEvent {
        let mut event = GraphBlockEvent {
            logical_id: 1,
            frame_offset,
            sequence: 0,
            kind,
            aux_count: aux.len() as u32,
            aux: [0.0; GBE_AUX_CAP],
        };
        event.aux[..aux.len()].copy_from_slice(aux);
        event
    }

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

    #[test]
    fn gatepitch_events_fire_at_scheduled_frames() {
        let mut state = [0.0_f32; GATEPITCH_STATE_SIZE];
        unsafe {
            gatepitch_init(state.as_mut_ptr().cast(), 48_000, 64, std::ptr::null());
            gatepitch_begin_event_slice(state.as_mut_ptr().cast(), 1, 0, 8);
            let note_a = event(2, GBE_NOTE_ON, &[220.0, 0.5]);
            let off = event(5, GBE_GATE_OFF, &[]);
            let note_b = event(5, GBE_NOTE_ON, &[330.0, 0.75]);
            assert!(gatepitch_schedule_event(state.as_mut_ptr().cast(), &note_a));
            assert!(gatepitch_schedule_event(state.as_mut_ptr().cast(), &off));
            assert!(gatepitch_schedule_event(state.as_mut_ptr().cast(), &note_b));
        }

        let mut gate = [0.0; 8];
        let mut pitch = [0.0; 8];
        let mut velocity = [0.0; 8];
        let mut trigger = [0.0; 8];
        let mut clock = [0.0; 8];
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
                8,
                state.as_mut_ptr().cast(),
                std::ptr::null_mut(),
            );
        }

        assert_eq!(trigger, [0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
        assert_eq!(gate, [0.0, 0.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0]);
        assert_eq!(pitch[2], 220.0);
        assert_eq!(pitch[5], 330.0);
        assert_eq!(velocity[2], 0.5);
        assert_eq!(velocity[5], 0.75);
    }

    #[test]
    fn gatepitch_begin_event_slice_clears_stale_timeline() {
        let mut state = [0.0_f32; GATEPITCH_STATE_SIZE];
        unsafe {
            gatepitch_init(state.as_mut_ptr().cast(), 48_000, 64, std::ptr::null());
            gatepitch_begin_event_slice(state.as_mut_ptr().cast(), 1, 0, 4);
            let note = event(0, GBE_NOTE_ON, &[220.0, 1.0]);
            assert!(gatepitch_schedule_event(state.as_mut_ptr().cast(), &note));
            gatepitch_begin_event_slice(state.as_mut_ptr().cast(), 1, 4, 4);
        }

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

        assert_eq!(trigger, [0.0; 4]);
        assert_eq!(gate, [0.0; 4]);
    }
}
