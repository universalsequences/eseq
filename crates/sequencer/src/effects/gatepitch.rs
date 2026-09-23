use std::os::raw::{c_int, c_void};

use crate::audiograph::{GraphBlockEvent, NodeVTable, GBE_GATE_OFF, GBE_NOTE_ON, GBE_PRESSURE, GBE_EXPRESSION};

const TIMELINE_EVENT_WIDTH: usize = 8;
const TIMELINE_FRAME: usize = 0;
const TIMELINE_KIND: usize = 1;
const TIMELINE_PITCH: usize = 2;
const TIMELINE_VELOCITY: usize = 3;
const TIMELINE_LEGATO: usize = 4;
const TIMELINE_PRESSURE: usize = 5;
pub const GATEPITCH_TIMELINE_CAPACITY: usize = crate::sequencer::MAX_STEPS;
const TIMELINE_PITCH_BEND: usize = 6;
const TIMELINE_MOD_WHEEL: usize = 7;
const PARAM_TIMELINE_COUNT: usize = 9;
const PARAM_TIMELINE_BASE: usize = 10;

// State layout starts with the public ParamMsg slots, then a fixed per-slice
// event timeline: [count, event(frame, kind, pitch, velocity, legato, pressure, bend, wheel) * MAX_STEPS].
// After the timeline: the constant-lane cache (`effects::output_lanes`).
const OUTPUT_LANE_CACHE: usize =
    PARAM_TIMELINE_BASE + GATEPITCH_TIMELINE_CAPACITY * TIMELINE_EVENT_WIDTH;
/// Every output except the clock ramp is constant between events.
const CACHED_LANES: [usize; 10] = [
    0, 1, 2, 3, 5, OUTPUT_NOTE_ON, OUTPUT_LEGATO, OUTPUT_PRESSURE, OUTPUT_PITCH_BEND, OUTPUT_MOD_WHEEL,
];
pub const GATEPITCH_STATE_SIZE: usize =
    OUTPUT_LANE_CACHE + super::output_lanes::state_cells(CACHED_LANES.len());
pub const OUTPUT_NOTE_ON: usize = 6;
pub const OUTPUT_LEGATO: usize = 7;
pub const OUTPUT_PRESSURE: usize = 8;
pub const OUTPUT_PITCH_BEND: usize = 9;
pub const OUTPUT_MOD_WHEEL: usize = 10;
pub const OUTPUT_COUNT: usize = 11;
pub const PARAM_GATE: u64 = 0;
pub const PARAM_PITCH: u64 = 1;
pub const PARAM_VELOCITY: u64 = 2;
pub const PARAM_TRIGGER: u64 = 3;
pub const PARAM_CLOCK_PHASE: u64 = 4;
pub const PARAM_CLOCK_INC: u64 = 5;
pub const PARAM_PRESSURE: u64 = 6;
pub const PARAM_PITCH_BEND: u64 = 7;
pub const PARAM_MOD_WHEEL: u64 = 8;

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
    *s.add(PARAM_PRESSURE as usize) = 0.0;
    *s.add(PARAM_PITCH_BEND as usize) = 0.0;
    *s.add(PARAM_MOD_WHEEL as usize) = 0.0;
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
        GBE_EXPRESSION if event.aux_count >= 3 && event.aux[..3].iter().all(|v| v.is_finite()) => (0.0, 0.0),
        GBE_PRESSURE if event.aux_count >= 1 && event.aux[0].is_finite() => (0.0, 0.0),
        _ => return false,
    };

    let base = PARAM_TIMELINE_BASE + count * TIMELINE_EVENT_WIDTH;
    *s.add(base + TIMELINE_FRAME) = event.frame_offset as f32;
    *s.add(base + TIMELINE_KIND) = event.kind as f32;
    *s.add(base + TIMELINE_PITCH) = pitch;
    *s.add(base + TIMELINE_VELOCITY) = velocity;
    // A request, not proof that a previous note remains held. Resolve it at
    // the event's sample, after any earlier gate-off in this slice.
    *s.add(base + TIMELINE_LEGATO) =
        if event.kind == GBE_NOTE_ON && event.aux_count >= 3 && event.aux[2] > 0.5 {
            1.0
        } else {
            0.0
        };
    *s.add(PARAM_TIMELINE_COUNT) = (count + 1) as f32;
    *s.add(base + TIMELINE_PRESSURE) = match event.kind {
        GBE_PRESSURE | GBE_EXPRESSION => event.aux[0].clamp(0.0, 1.0),
        GBE_NOTE_ON if event.aux_count >= 4 && event.aux[3].is_finite() => event.aux[3].clamp(0.0, 1.0),
        _ => 0.0,
    };
    *s.add(base + TIMELINE_PITCH_BEND) = if event.kind == GBE_EXPRESSION {
        event.aux[1].clamp(-1.0, 1.0)
    } else { 0.0 };
    *s.add(base + TIMELINE_MOD_WHEEL) = if event.kind == GBE_EXPRESSION {
        event.aux[2].clamp(0.0, 1.0)
    } else { 0.0 };
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
    let mut pressure = *s.add(PARAM_PRESSURE as usize);
    let mut pitch_bend = *s.add(PARAM_PITCH_BEND as usize);
    let mut mod_wheel = *s.add(PARAM_MOD_WHEEL as usize);
    let mut clock_phase = *s.add(PARAM_CLOCK_PHASE as usize);
    let clock_inc = *s.add(PARAM_CLOCK_INC as usize);
    let event_count = (*s.add(PARAM_TIMELINE_COUNT)).max(0.0) as usize;
    let event_count = event_count.min(GATEPITCH_TIMELINE_CAPACITY);
    let mut event_index = 0usize;
    let nf = nframes as usize;
    let lane = |output: usize| std::slice::from_raw_parts_mut(*out.add(output), nf);
    let (gate_out, pitch_out, velocity_out, trigger_out, clock_out, clock_inc_out) =
        (lane(0), lane(1), lane(2), lane(3), lane(4), lane(5));
    let (note_on_out, legato_out) = (lane(OUTPUT_NOTE_ON), lane(OUTPUT_LEGATO));
    let (pressure_out, pitch_bend_out, mod_wheel_out) =
        (lane(OUTPUT_PRESSURE), lane(OUTPUT_PITCH_BEND), lane(OUTPUT_MOD_WHEEL));
    let event_frame =
        |index: usize| (*s.add(PARAM_TIMELINE_BASE + index * TIMELINE_EVENT_WIDTH + TIMELINE_FRAME)).max(0.0) as usize;
    let cache = super::output_lanes::OutputLanes::begin(s.add(OUTPUT_LANE_CACHE));

    // Every lane but the clock only changes on event frames, so render one
    // constant segment per event run instead of eleven stores per frame.
    let mut i = 0usize;
    while i < nf {
        let mut trigger = 0.0;
        let mut note_on = 0.0;
        let mut legato = 0.0;
        while event_index < event_count {
            let base = PARAM_TIMELINE_BASE + event_index * TIMELINE_EVENT_WIDTH;
            if event_frame(event_index) != i {
                break;
            }
            let kind = *s.add(base + TIMELINE_KIND) as u32;
            if kind == GBE_NOTE_ON {
                pressure = *s.add(base + TIMELINE_PRESSURE);
                pitch_bend = 0.0;
                mod_wheel = 0.0;
                pitch = *s.add(base + TIMELINE_PITCH);
                velocity = *s.add(base + TIMELINE_VELOCITY);
                let continues_note = gate > 0.5 && *s.add(base + TIMELINE_LEGATO) > 0.5;
                note_on = 1.0;
                if continues_note {
                    legato = 1.0;
                } else {
                    trigger = 1.0;
                }
                gate = 1.0;
            } else if kind == GBE_EXPRESSION {
                pressure = *s.add(base + TIMELINE_PRESSURE);
                pitch_bend = *s.add(base + TIMELINE_PITCH_BEND);
                mod_wheel = *s.add(base + TIMELINE_MOD_WHEEL);
            } else if kind == GBE_PRESSURE {
                pressure = *s.add(base + TIMELINE_PRESSURE);
            } else if kind == GBE_GATE_OFF {
                gate = 0.0;
            }
            event_index += 1;
        }
        // An unconsumed event only ever fires on its own frame; one already
        // behind `i` blocks the rest of the timeline, as in the reference.
        let end = if event_index < event_count {
            let next = event_frame(event_index);
            if next > i { next.min(nf) } else { nf }
        } else {
            nf
        };
        if i == 0 && end == nf && trigger == 0.0 && note_on == 0.0 && legato == 0.0 {
            // One constant segment for the whole call: let the lane cache
            // skip lanes that already hold these values.
            let values = [gate, pitch, velocity, 0.0, clock_inc, 0.0, 0.0, pressure, pitch_bend, mod_wheel];
            let lanes: [&mut [f32]; 10] = [
                &mut *gate_out, &mut *pitch_out, &mut *velocity_out, &mut *trigger_out,
                &mut *clock_inc_out, &mut *note_on_out, &mut *legato_out, &mut *pressure_out,
                &mut *pitch_bend_out, &mut *mod_wheel_out,
            ];
            for (index, (lane, value)) in lanes.into_iter().zip(values).enumerate() {
                cache.fill(index, lane, value);
            }
        } else {
            gate_out[i..end].fill(gate);
            pitch_out[i..end].fill(pitch);
            velocity_out[i..end].fill(velocity);
            clock_inc_out[i..end].fill(clock_inc);
            pressure_out[i..end].fill(pressure);
            pitch_bend_out[i..end].fill(pitch_bend);
            mod_wheel_out[i..end].fill(mod_wheel);
            trigger_out[i..end].fill(0.0);
            note_on_out[i..end].fill(0.0);
            legato_out[i..end].fill(0.0);
            trigger_out[i] = trigger;
            note_on_out[i] = note_on;
            legato_out[i] = legato;
            for index in 0..CACHED_LANES.len() {
                cache.written(index);
            }
        }
        for value in &mut clock_out[i..end] {
            *value = clock_phase;
            clock_phase += clock_inc;
            if clock_phase >= 1.0 {
                clock_phase -= clock_phase.floor();
            }
        }
        i = end;
    }
    *s.add(PARAM_PRESSURE as usize) = pressure;
    *s.add(PARAM_PITCH_BEND as usize) = pitch_bend;
    *s.add(PARAM_MOD_WHEEL as usize) = mod_wheel;
    *s.add(PARAM_GATE as usize) = gate;
    *s.add(PARAM_PITCH as usize) = pitch;
    *s.add(PARAM_VELOCITY as usize) = velocity;
    *s.add(PARAM_TRIGGER as usize) = 0.0;
    *s.add(PARAM_CLOCK_PHASE as usize) = clock_phase;
}

/// Per-sample reference for `gatepitch_process`, pinned bit-for-bit by
/// `segment_renderer_matches_per_sample_reference`.
#[cfg(test)]
unsafe extern "C" fn gatepitch_process_reference(
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
    let mut pressure = *s.add(PARAM_PRESSURE as usize);
    let mut pitch_bend = *s.add(PARAM_PITCH_BEND as usize);
    let mut mod_wheel = *s.add(PARAM_MOD_WHEEL as usize);
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
    let out5 = *out.add(5); // per-sample clock increment
    for i in 0..nf {
        let mut trigger = 0.0;
        let mut note_on = 0.0;
        let mut legato = 0.0;
        while event_index < event_count {
            let base = PARAM_TIMELINE_BASE + event_index * TIMELINE_EVENT_WIDTH;
            let frame = (*s.add(base + TIMELINE_FRAME)).max(0.0) as usize;
            if frame != i {
                break;
            }
            let kind = *s.add(base + TIMELINE_KIND) as u32;
            if kind == GBE_NOTE_ON {
                pressure = *s.add(base + TIMELINE_PRESSURE);
                pitch_bend = 0.0;
                mod_wheel = 0.0;
                pitch = *s.add(base + TIMELINE_PITCH);
                velocity = *s.add(base + TIMELINE_VELOCITY);
                let continues_note = gate > 0.5 && *s.add(base + TIMELINE_LEGATO) > 0.5;
                note_on = 1.0;
                if continues_note {
                    legato = 1.0;
                } else {
                    trigger = 1.0;
                }
                gate = 1.0;
            } else if kind == GBE_EXPRESSION {
                pressure = *s.add(base + TIMELINE_PRESSURE);
                pitch_bend = *s.add(base + TIMELINE_PITCH_BEND);
                mod_wheel = *s.add(base + TIMELINE_MOD_WHEEL);
            } else if kind == GBE_PRESSURE {
                pressure = *s.add(base + TIMELINE_PRESSURE);
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
        *out5.add(i) = clock_inc;
        *(*out.add(OUTPUT_NOTE_ON)).add(i) = note_on;
        *(*out.add(OUTPUT_LEGATO)).add(i) = legato;
        *(*out.add(OUTPUT_PRESSURE)).add(i) = pressure;
        *(*out.add(OUTPUT_PITCH_BEND)).add(i) = pitch_bend;
        *(*out.add(OUTPUT_MOD_WHEEL)).add(i) = mod_wheel;
        clock_phase += clock_inc;
        if clock_phase >= 1.0 {
            clock_phase -= clock_phase.floor();
        }
    }
    *s.add(PARAM_PRESSURE as usize) = pressure;
    *s.add(PARAM_PITCH_BEND as usize) = pitch_bend;
    *s.add(PARAM_MOD_WHEEL as usize) = mod_wheel;
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
    use crate::audiograph::{GBE_AUX_CAP, GBE_GATE_OFF, GBE_NOTE_ON, GBE_PRESSURE, GBE_EXPRESSION};

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
    fn cached_lanes_follow_param_changes_and_rewired_buffers() {
        use crate::audiograph as graph;
        use std::ffi::CString;
        graph::initialize_engine_for_test(64, 48_000);
        let label = CString::new("gatepitch-lane-cache").unwrap();
        let lg = unsafe { graph::create_live_graph(32, 64, label.as_ptr(), 1) };
        assert!(!lg.is_null());
        let name = CString::new("gp").unwrap();
        let gp = unsafe {
            graph::add_node(lg, gatepitch_vtable(), GATEPITCH_STATE_SIZE * std::mem::size_of::<f32>(),
                name.as_ptr(), 0, OUTPUT_COUNT as c_int, std::ptr::null(), 0)
        };
        assert!(gp > 0);
        let set_pitch = |value: f32| unsafe {
            assert!(graph::params_push_wrapper(lg, graph::ParamMsg {
                idx: PARAM_PITCH, logical_id: gp as u64, fvalue: value,
            }));
        };
        let render = || {
            let mut output = vec![f32::NAN; 64];
            unsafe { graph::process_next_block(lg, output.as_mut_ptr(), 64) };
            output
        };
        assert!(unsafe { graph::graph_connect(lg, gp, 1, 0, 0) });
        set_pitch(440.0);
        // Repeated identical blocks take the cached path and must keep
        // delivering the value already in the buffer.
        for _ in 0..3 {
            assert!(render().iter().all(|v| *v == 440.0));
        }
        set_pitch(220.0);
        assert!(render().iter().all(|v| *v == 220.0));
        assert!(render().iter().all(|v| *v == 220.0));
        // A rewire hands the node a fresh zeroed edge buffer; the unchanged
        // value must be written into it, not assumed present.
        assert!(unsafe { graph::graph_disconnect(lg, gp, 1, 0, 0) });
        assert!(render().iter().all(|v| *v == 0.0));
        assert!(unsafe { graph::graph_connect(lg, gp, 1, 0, 0) });
        assert!(render().iter().all(|v| *v == 220.0), "rewired buffer must be refilled");
        unsafe { graph::destroy_live_graph(lg) };
    }

    #[test]
    fn segment_renderer_matches_per_sample_reference() {
        let mut seed = 0x9e37_79b9u64;
        let mut next = move |n: u64| {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (seed >> 33) % n
        };
        for trial in 0..400 {
            let frames = 1 + next(300) as usize;
            let mut state = vec![0.0_f32; GATEPITCH_STATE_SIZE];
            for param in 0..=8 {
                state[param] = next(1000) as f32 / 250.0 - 1.0;
            }
            state[PARAM_CLOCK_PHASE as usize] = next(1000) as f32 / 1000.0;
            state[PARAM_CLOCK_INC as usize] = if trial % 3 == 0 { 0.0 } else { next(1000) as f32 / 20_000.0 };
            let events = next(8) as usize;
            state[PARAM_TIMELINE_COUNT] = events as f32;
            let mut frame = 0u64;
            for event in 0..events {
                let base = PARAM_TIMELINE_BASE + event * TIMELINE_EVENT_WIDTH;
                // Mostly ascending frames, with duplicates, stale (earlier)
                // frames and frames past the block, as a real timeline can.
                frame = match next(6) {
                    0 => frame,
                    1 => frame.saturating_sub(next(5)),
                    2 => frames as u64 + next(4),
                    _ => frame + next(80),
                };
                state[base + TIMELINE_FRAME] = frame as f32;
                state[base + TIMELINE_KIND] =
                    [GBE_NOTE_ON, GBE_GATE_OFF, GBE_EXPRESSION, GBE_PRESSURE, 99][next(5) as usize] as f32;
                for field in 2..TIMELINE_EVENT_WIDTH {
                    state[base + field] = next(1000) as f32 / 500.0;
                }
            }
            let mut reference_state = state.clone();
            let render = |state: &mut Vec<f32>, reference: bool| {
                let mut outputs = vec![vec![f32::NAN; frames]; OUTPUT_COUNT];
                let pointers: Vec<*mut f32> = outputs.iter_mut().map(|lane| lane.as_mut_ptr()).collect();
                unsafe {
                    let process = if reference { gatepitch_process_reference } else { gatepitch_process };
                    process(std::ptr::null(), pointers.as_ptr(), frames as c_int,
                        state.as_mut_ptr().cast(), std::ptr::null_mut());
                }
                outputs
            };
            let segmented = render(&mut state, false);
            let reference = render(&mut reference_state, true);
            for lane in 0..OUTPUT_COUNT {
                for i in 0..frames {
                    assert_eq!(segmented[lane][i].to_bits(), reference[lane][i].to_bits(),
                        "trial {trial} lane {lane} frame {i}");
                }
            }
            // The lane cache after the timeline is the fast path's own bookkeeping.
            for idx in 0..OUTPUT_LANE_CACHE {
                assert_eq!(state[idx].to_bits(), reference_state[idx].to_bits(), "trial {trial} state[{idx}]");
            }
        }
    }

    #[test]
    fn expression_updates_are_atomic_sample_accurate_and_reset_on_reuse() {
        let mut state = [0.0_f32; GATEPITCH_STATE_SIZE];
        let mut outputs = [[0.0_f32; 8]; OUTPUT_COUNT];
        let pointers = outputs.each_mut().map(|output| output.as_mut_ptr());
        unsafe {
            gatepitch_init(state.as_mut_ptr().cast(), 48_000, 8, std::ptr::null());
            gatepitch_begin_event_slice(state.as_mut_ptr().cast(), 1, 0, 8);
            for change in [
                event(0, GBE_NOTE_ON, &[220.0, 1.0]),
                event(0, GBE_EXPRESSION, &[0.2, -0.5, 0.8]),
                event(2, GBE_PRESSURE, &[0.7]),
                event(3, GBE_GATE_OFF, &[]),
                event(4, GBE_EXPRESSION, &[0.4, 0.25, 0.6]),
                event(6, GBE_NOTE_ON, &[330.0, 1.0]),
            ] {
                assert!(gatepitch_schedule_event(state.as_mut_ptr().cast(), &change));
            }
            assert!(!gatepitch_schedule_event(state.as_mut_ptr().cast(),
                &event(7, GBE_EXPRESSION, &[0.0, f32::NAN, 0.0])));
            gatepitch_process(std::ptr::null(), pointers.as_ptr(), 8,
                state.as_mut_ptr().cast(), std::ptr::null_mut());
        }
        assert_eq!(outputs[OUTPUT_PRESSURE], [0.2, 0.2, 0.7, 0.7, 0.4, 0.4, 0.0, 0.0]);
        assert_eq!(outputs[OUTPUT_PITCH_BEND], [-0.5, -0.5, -0.5, -0.5, 0.25, 0.25, 0.0, 0.0]);
        assert_eq!(outputs[OUTPUT_MOD_WHEEL], [0.8, 0.8, 0.8, 0.8, 0.6, 0.6, 0.0, 0.0]);
        assert_eq!(outputs[PARAM_TRIGGER as usize], [1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0]);
    }

    #[test]
    fn pressure_changes_at_event_offset_and_resets_on_voice_reuse() {
        let mut state = [0.0_f32; GATEPITCH_STATE_SIZE];
        let mut outputs = [[0.0_f32; 8]; OUTPUT_COUNT];
        let pointers = outputs.each_mut().map(|output| output.as_mut_ptr());
        unsafe {
            gatepitch_init(state.as_mut_ptr().cast(), 48_000, 8, std::ptr::null());
            gatepitch_begin_event_slice(state.as_mut_ptr().cast(), 1, 0, 8);
            for change in [
                event(0, GBE_NOTE_ON, &[220.0, 0.5, 0.0, 0.25]),
                event(2, GBE_PRESSURE, &[0.75]),
                event(3, GBE_GATE_OFF, &[]),
                event(5, GBE_NOTE_ON, &[330.0, 0.5]),
                event(7, GBE_PRESSURE, &[2.0]),
            ] {
                assert!(gatepitch_schedule_event(state.as_mut_ptr().cast(), &change));
            }
            assert!(!gatepitch_schedule_event(state.as_mut_ptr().cast(),
                &event(7, GBE_PRESSURE, &[f32::NAN])));
            gatepitch_process(std::ptr::null(), pointers.as_ptr(), 8,
                state.as_mut_ptr().cast(), std::ptr::null_mut());
        }
        assert_eq!(outputs[OUTPUT_PRESSURE], [0.25, 0.25, 0.75, 0.75, 0.75, 0.0, 0.0, 1.0]);
        assert_eq!(outputs[PARAM_TRIGGER as usize], [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
        unsafe {
            gatepitch_begin_event_slice(state.as_mut_ptr().cast(), 2, 0, 8);
            gatepitch_process(std::ptr::null(), pointers.as_ptr(), 8,
                state.as_mut_ptr().cast(), std::ptr::null_mut());
        }
        assert_eq!(outputs[OUTPUT_PRESSURE], [1.0; 8]);
        assert_eq!(outputs[PARAM_TRIGGER as usize], [0.0; 8]);
    }

    #[test]
    fn legato_preserves_gate_but_reports_each_note_and_retriggers_after_release() {
        let mut state = [0.0_f32; GATEPITCH_STATE_SIZE];
        let mut outputs = [[0.0_f32; 8]; OUTPUT_COUNT];
        let pointers = outputs.each_mut().map(|output| output.as_mut_ptr());
        unsafe {
            gatepitch_init(state.as_mut_ptr().cast(), 48_000, 8, std::ptr::null());
            gatepitch_begin_event_slice(state.as_mut_ptr().cast(), 1, 0, 8);
            for note in [
                event(0, GBE_NOTE_ON, &[220.0, 0.5, 1.0]),
                event(2, GBE_NOTE_ON, &[330.0, 0.75, 1.0]),
                event(4, GBE_GATE_OFF, &[]),
                event(4, GBE_NOTE_ON, &[440.0, 1.0, 1.0]),
                event(6, GBE_NOTE_ON, &[550.0, 0.25, 0.0]),
                event(7, GBE_GATE_OFF, &[]),
            ] {
                assert!(gatepitch_schedule_event(state.as_mut_ptr().cast(), &note));
            }
            gatepitch_process(
                std::ptr::null(), pointers.as_ptr(), 8,
                state.as_mut_ptr().cast(), std::ptr::null_mut(),
            );
        }
        assert_eq!(outputs[PARAM_GATE as usize], [1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 0.0]);
        assert_eq!(outputs[PARAM_TRIGGER as usize], [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 1.0, 0.0]);
        assert_eq!(outputs[OUTPUT_NOTE_ON], [1.0, 0.0, 1.0, 0.0, 1.0, 0.0, 1.0, 0.0]);
        assert_eq!(outputs[OUTPUT_LEGATO], [0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0]);
        assert_eq!(outputs[PARAM_PITCH as usize], [220.0, 220.0, 330.0, 330.0, 440.0, 440.0, 550.0, 550.0]);
        assert_eq!(outputs[PARAM_VELOCITY as usize][2], 0.75);
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
        let mut clock_inc = [0.0; 4];
        let mut note_on = [0.0; 4];
        let mut legato = [0.0; 4];
        let mut pressure = [0.0; 4];
        let mut pitch_bend = [0.0; 4];
        let mut mod_wheel = [0.0; 4];
        let outputs = [
            gate.as_mut_ptr(),
            pitch.as_mut_ptr(),
            velocity.as_mut_ptr(),
            trigger.as_mut_ptr(),
            clock.as_mut_ptr(),
            clock_inc.as_mut_ptr(),
            note_on.as_mut_ptr(),
            legato.as_mut_ptr(),
            pressure.as_mut_ptr(),
            pitch_bend.as_mut_ptr(),
            mod_wheel.as_mut_ptr(),
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
        assert_eq!(clock_inc, [0.125; 4]);
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
        let mut clock_inc = [0.0; 8];
        let mut note_on = [0.0; 8];
        let mut legato = [0.0; 8];
        let mut pressure = [0.0; 8];
        let mut pitch_bend = [0.0; 8];
        let mut mod_wheel = [0.0; 8];
        let outputs = [
            gate.as_mut_ptr(),
            pitch.as_mut_ptr(),
            velocity.as_mut_ptr(),
            trigger.as_mut_ptr(),
            clock.as_mut_ptr(),
            clock_inc.as_mut_ptr(),
            note_on.as_mut_ptr(),
            legato.as_mut_ptr(),
            pressure.as_mut_ptr(),
            pitch_bend.as_mut_ptr(),
            mod_wheel.as_mut_ptr(),
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
        let mut clock_inc = [0.0; 4];
        let mut note_on = [0.0; 4];
        let mut legato = [0.0; 4];
        let mut pressure = [0.0; 4];
        let mut pitch_bend = [0.0; 4];
        let mut mod_wheel = [0.0; 4];
        let outputs = [
            gate.as_mut_ptr(),
            pitch.as_mut_ptr(),
            velocity.as_mut_ptr(),
            trigger.as_mut_ptr(),
            clock.as_mut_ptr(),
            clock_inc.as_mut_ptr(),
            note_on.as_mut_ptr(),
            legato.as_mut_ptr(),
            pressure.as_mut_ptr(),
            pitch_bend.as_mut_ptr(),
            mod_wheel.as_mut_ptr(),
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
