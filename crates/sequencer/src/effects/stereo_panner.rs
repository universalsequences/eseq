use crate::audiograph::{GraphBlockEvent, NodeVTable, GBE_AUX_CAP, GBE_MIXER_PARAM};
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

const STATE_EVENT_COUNT: usize = 9;
const STATE_SLICE_FRAMES: usize = 10;
const STATE_EVENTS: usize = 11;
const EVENT_WIDTH: usize = 3;
pub const STEREO_PANNER_TIMELINE_CAPACITY: usize = 64;
const STATE_RENDERED: usize = STATE_EVENTS + EVENT_WIDTH * STEREO_PANNER_TIMELINE_CAPACITY;
pub const STEREO_PANNER_STATE_SIZE: usize = STATE_RENDERED + 1;

pub const STEREO_PANNER_PARAM_VOLUME: u64 = STATE_VOLUME as u64;
pub const STEREO_PANNER_PARAM_PAN: u64 = STATE_PAN as u64;
pub const STEREO_PANNER_PARAM_MUTE: u64 = STATE_MUTE as u64;
pub const STEREO_PANNER_PARAM_MUTED_BY_SOLO: u64 = STATE_MUTED_BY_SOLO as u64;

/// Construct a sample-timed mixer change for the graph's existing block-event
/// path. Only authored mixer parameters are writable, not smoothing or meter
/// state. A rejected event must fail scheduling, never become a partial mix.
pub fn mixer_param_event(
    logical_id: u64,
    frame_offset: u32,
    sequence: u32,
    parameter: u64,
    value: f32,
) -> Result<GraphBlockEvent, &'static str> {
    if !matches!(parameter, STEREO_PANNER_PARAM_VOLUME | STEREO_PANNER_PARAM_PAN
        | STEREO_PANNER_PARAM_MUTE | STEREO_PANNER_PARAM_MUTED_BY_SOLO) {
        return Err("unsupported timed mixer parameter");
    }
    if !value.is_finite() { return Err("non-finite timed mixer parameter"); }
    let mut aux = [0.0; GBE_AUX_CAP];
    aux[0] = parameter as f32;
    aux[1] = value;
    Ok(GraphBlockEvent { logical_id, frame_offset, sequence, kind: GBE_MIXER_PARAM, aux_count: 2, aux })
}

unsafe extern "C" fn stereo_panner_begin_event_slice(
    state: *mut c_void,
    _block_serial: u64,
    _slice_start: c_int,
    slice_nframes: c_int,
) {
    let s = state as *mut f32;
    *s.add(STATE_EVENT_COUNT) = 0.0;
    *s.add(STATE_SLICE_FRAMES) = slice_nframes.max(0) as f32;
}

unsafe extern "C" fn stereo_panner_schedule_event(
    state: *mut c_void,
    event: *const GraphBlockEvent,
) -> bool {
    if state.is_null() || event.is_null() { return false; }
    let event = &*event;
    if event.kind != GBE_MIXER_PARAM || event.aux_count != 2 || !event.aux[1].is_finite() {
        return false;
    }
    let parameter = event.aux[0];
    if ![STATE_VOLUME, STATE_PAN, STATE_MUTE, STATE_MUTED_BY_SOLO]
        .iter().any(|index| parameter == *index as f32) {
        return false;
    }
    let s = state as *mut f32;
    let count = *s.add(STATE_EVENT_COUNT) as usize;
    if count >= STEREO_PANNER_TIMELINE_CAPACITY
        || event.frame_offset >= *s.add(STATE_SLICE_FRAMES) as u32 {
        return false;
    }
    // The graph delivers stable (frame, sequence) order. Reject malformed
    // direct callers instead of consuming a later event ahead of an earlier one.
    if count > 0 && event.frame_offset < *s.add(STATE_EVENTS + (count - 1) * EVENT_WIDTH) as u32 {
        return false;
    }
    let base = STATE_EVENTS + count * EVENT_WIDTH;
    *s.add(base) = event.frame_offset as f32;
    *s.add(base + 1) = parameter;
    *s.add(base + 2) = event.aux[1];
    *s.add(STATE_EVENT_COUNT) = (count + 1) as f32;
    true
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
    *s.add(STATE_SMOOTH_L) = 0.0;
    *s.add(STATE_SMOOTH_R) = 0.0;
    *s.add(STATE_RENDERED) = 0.0;
    *s.add(STATE_PEAK_L) = 0.0;
    *s.add(STATE_PEAK_R) = 0.0;
    *s.add(STATE_EVENT_COUNT) = 0.0;
    *s.add(STATE_SLICE_FRAMES) = 0.0;
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
    let mut rendered = *s.add(STATE_RENDERED) != 0.0;
    let prev_peak_l = *s.add(STATE_PEAK_L);
    let prev_peak_r = *s.add(STATE_PEAK_R);
    let smooth_coeff = 1.0 - (-2.0 * std::f32::consts::PI * 60.0 / sample_rate).exp();
    let mut peak_l = 0.0f32;
    let mut peak_r = 0.0f32;

    let in0 = *inp.add(0);
    let in1 = *inp.add(1);
    let out0 = *out.add(0);
    let out1 = *out.add(1);

    let (mut target_l, mut target_r) = if muted {
        (0.0, 0.0)
    } else {
        balance_gains_for(volume, pan)
    };

    let event_count = (*s.add(STATE_EVENT_COUNT) as usize).min(STEREO_PANNER_TIMELINE_CAPACITY);
    let mut next_event = 0;
    for i in 0..nframes as usize {
        let first_event = next_event;
        while next_event < event_count {
            let base = STATE_EVENTS + next_event * EVENT_WIDTH;
            if *s.add(base) as usize != i { break; }
            let parameter = *s.add(base + 1) as usize;
            *s.add(parameter) = *s.add(base + 2);
            next_event += 1;
        }
        if next_event != first_event {
            (target_l, target_r) = if *s.add(STATE_MUTE) >= 0.5 || *s.add(STATE_MUTED_BY_SOLO) >= 0.5 {
                (0.0, 0.0)
            } else {
                balance_gains_for(*s.add(STATE_VOLUME), *s.add(STATE_PAN))
            };
        }
        if !rendered {
            // Host parameters arrive after node initialization. Seed from the
            // first sample's target, including frame-zero timeline events, so
            // saved mute/gain/pan settings do not ramp from unrelated defaults.
            // Once any sample has been processed, all changes remain smoothed.
            smooth_l = target_l;
            smooth_r = target_r;
            rendered = true;
        } else {
            smooth_l += smooth_coeff * (target_l - smooth_l);
            smooth_r += smooth_coeff * (target_r - smooth_r);
        }
        let sample_l = *in0.add(i) * smooth_l;
        let sample_r = *in1.add(i) * smooth_r;
        *out0.add(i) = sample_l;
        *out1.add(i) = sample_r;
        peak_l = peak_l.max(sample_l.abs());
        peak_r = peak_r.max(sample_r.abs());
    }

    *s.add(STATE_SMOOTH_L) = smooth_l;
    *s.add(STATE_SMOOTH_R) = smooth_r;
    *s.add(STATE_RENDERED) = if rendered { 1.0 } else { 0.0 };
    *s.add(STATE_PEAK_L) = peak_l.max(prev_peak_l * 0.92);
    *s.add(STATE_PEAK_R) = peak_r.max(prev_peak_r * 0.92);
    *s.add(STATE_EVENT_COUNT) = 0.0;
}

pub fn stereo_panner_vtable() -> NodeVTable {
    NodeVTable {
        process: Some(stereo_panner_process),
        init: Some(stereo_panner_init),
        reset: None,
        migrate: None,
        begin_event_slice: Some(stereo_panner_begin_event_slice),
        schedule_event: Some(stereo_panner_schedule_event),
        ..NodeVTable::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audiograph as graph;

    fn state() -> Vec<f32> {
        let mut state = vec![0.0; STEREO_PANNER_STATE_SIZE];
        unsafe { stereo_panner_init(state.as_mut_ptr().cast(), 48000, 128, std::ptr::null()); }
        state
    }

    fn render(state: &mut [f32], frames: usize) -> Vec<[f32; 2]> {
        let mut input = vec![1.0f32; frames];
        let mut left = vec![0.0f32; frames];
        let mut right = vec![0.0f32; frames];
        let inputs = [input.as_mut_ptr(), input.as_mut_ptr()];
        let outputs = [left.as_mut_ptr(), right.as_mut_ptr()];
        unsafe {
            stereo_panner_process(inputs.as_ptr(), outputs.as_ptr(), frames as c_int,
                state.as_mut_ptr().cast(), std::ptr::null_mut());
        }
        left.into_iter().zip(right).map(|(l, r)| [l, r]).collect()
    }

    #[test]
    fn initial_settings_apply_exactly_without_a_startup_ramp() {
        for (volume, pan, mute, solo_mute, expected) in [
            (1.0, 0.0, 0.0, 0.0, [1.0, 1.0]),
            (0.25, 0.5, 0.0, 0.0, [0.125, 0.25]),
            (0.5, -0.75, 0.0, 0.0, [0.5, 0.125]),
            (1.0, 0.0, 1.0, 0.0, [0.0, 0.0]),
            (1.0, 0.0, 0.0, 1.0, [0.0, 0.0]),
        ] {
            let mut state = state();
            // A zero-frame call must not finalize initialization before the
            // host has supplied the authored parameters.
            assert!(render(&mut state, 0).is_empty());
            state[STATE_VOLUME] = volume;
            state[STATE_PAN] = pan;
            state[STATE_MUTE] = mute;
            state[STATE_MUTED_BY_SOLO] = solo_mute;
            assert_eq!(render(&mut state, 32), vec![expected; 32]);
        }
    }

    #[test]
    fn frame_zero_mute_initializes_silently_but_later_changes_still_smooth() {
        let mut state = state();
        unsafe {
            stereo_panner_begin_event_slice(state.as_mut_ptr().cast(), 1, 0, 32);
            let event = mixer_param_event(0, 0, 0, STEREO_PANNER_PARAM_MUTE, 1.0).unwrap();
            assert!(stereo_panner_schedule_event(state.as_mut_ptr().cast(), &event));
        }
        assert_eq!(render(&mut state, 32), vec![[0.0, 0.0]; 32]);
        state[STATE_MUTE] = 0.0;
        let coeff = 1.0 - (-2.0 * std::f32::consts::PI * 60.0 / 48_000.0).exp();
        assert_eq!(render(&mut state, 1), vec![[coeff, coeff]]);
        let next = coeff + coeff * (1.0 - coeff);
        assert_eq!(render(&mut state, 1), vec![[next, next]], "must not reinitialize per call");
        state[STATE_MUTE] = 1.0;
        let muted = next + coeff * -next;
        assert_eq!(render(&mut state, 1), vec![[muted, muted]], "live mute still fades");
    }

    #[test]
    fn timed_mixer_changes_match_sample_by_sample_application() {
        let mut timed = state();
        let mut reference = state();
        let changes = [
            (0, STEREO_PANNER_PARAM_VOLUME, 1.3),
            (5, STEREO_PANNER_PARAM_MUTE, 1.0),
            (11, STEREO_PANNER_PARAM_MUTE, 0.0),
            (11, STEREO_PANNER_PARAM_MUTED_BY_SOLO, 1.0),
            (19, STEREO_PANNER_PARAM_MUTED_BY_SOLO, 0.0),
            (28, STEREO_PANNER_PARAM_PAN, 0.5),
        ];
        unsafe {
            stereo_panner_begin_event_slice(timed.as_mut_ptr().cast(), 1, 0, 32);
            for (sequence, &(frame, parameter, value)) in changes.iter().enumerate() {
                let event = mixer_param_event(0, frame, sequence as u32, parameter, value).unwrap();
                assert!(stereo_panner_schedule_event(timed.as_mut_ptr().cast(), &event));
            }
        }
        let actual = render(&mut timed, 32);
        let mut expected = Vec::new();
        for frame in 0..32 {
            for &(_, parameter, value) in changes.iter().filter(|change| change.0 == frame) {
                reference[parameter as usize] = value;
            }
            expected.extend(render(&mut reference, 1));
        }
        assert_eq!(actual, expected, "control timing and smoothing must be bit-identical");
        assert_eq!(timed[STATE_MUTE], 0.0);
        assert_eq!(timed[STATE_MUTED_BY_SOLO], 0.0);
        assert_eq!(timed[STATE_PAN], 0.5);
        assert_eq!(render(&mut timed, 32), render(&mut reference, 32), "no stale events in the next block");
    }

    #[test]
    fn mixer_timeline_rejects_invalid_events_and_capacity_overflow() {
        let mut state = state();
        let pointer = state.as_mut_ptr().cast();
        assert!(mixer_param_event(0, 0, 0, STATE_SMOOTH_L as u64, 0.0).is_err());
        assert!(mixer_param_event(0, 0, 0, STEREO_PANNER_PARAM_VOLUME, f32::NAN).is_err());
        unsafe {
            stereo_panner_begin_event_slice(pointer, 1, 0, 128);
            let event = mixer_param_event(0, 128, 0, STEREO_PANNER_PARAM_MUTE, 1.0).unwrap();
            assert!(!stereo_panner_schedule_event(pointer, &event));
            let mut event = mixer_param_event(0, 0, 0, STEREO_PANNER_PARAM_MUTE, 1.0).unwrap();
            event.aux[0] = 7.5;
            assert!(!stereo_panner_schedule_event(pointer, &event));
            event.aux[0] = STATE_MUTE as f32;
            event.aux[1] = f32::INFINITY;
            assert!(!stereo_panner_schedule_event(pointer, &event));
            assert_eq!(state[STATE_EVENT_COUNT], 0.0);
            let event = mixer_param_event(0, 5, 0, STEREO_PANNER_PARAM_MUTE, 1.0).unwrap();
            for _ in 0..STEREO_PANNER_TIMELINE_CAPACITY {
                assert!(stereo_panner_schedule_event(pointer, &event));
            }
            assert!(!stereo_panner_schedule_event(pointer, &event));
            stereo_panner_begin_event_slice(pointer, 2, 128, 128);
            assert!(stereo_panner_schedule_event(pointer, &event));
            let earlier = mixer_param_event(0, 4, 1, STEREO_PANNER_PARAM_MUTE, 0.0).unwrap();
            assert!(!stereo_panner_schedule_event(pointer, &earlier));
            stereo_panner_begin_event_slice(pointer, 3, 256, 128);
        }
        assert_eq!(render(&mut state, 128), render(&mut self::state(), 128), "starting a new slice clears old events");
    }

    unsafe extern "C" fn constant_input(
        _inputs: *const *mut f32, outputs: *const *mut f32, frames: c_int,
        _state: *mut c_void, _buffers: *mut c_void,
    ) {
        std::slice::from_raw_parts_mut(*outputs, frames as usize).fill(1.0);
    }

    struct Graph(*mut graph::LiveGraph);
    impl Drop for Graph {
        fn drop(&mut self) { unsafe { graph::destroy_live_graph(self.0); } }
    }

    #[test]
    fn production_graph_delivers_mixer_events_at_their_frame_offsets() {
        graph::initialize_engine_for_test(128, 48000);
        unsafe {
            let graph = Graph(graph::create_live_graph(32, 128, c"timed-mixer-test".as_ptr(), 2));
            assert!(!graph.0.is_null());
            let source = graph::add_node(graph.0, NodeVTable {
                process: Some(constant_input), ..NodeVTable::default()
            }, 4, c"constant".as_ptr(), 0, 1, std::ptr::null(), 0);
            let mixer = graph::add_node(graph.0, stereo_panner_vtable(),
                STEREO_PANNER_STATE_SIZE * std::mem::size_of::<f32>(), c"mixer".as_ptr(),
                2, 2, std::ptr::null(), 0);
            assert!(source > 0 && mixer > 0);
            assert!(graph::graph_connect(graph.0, source, 0, mixer, 0));
            assert!(graph::graph_connect(graph.0, source, 0, mixer, 1));
            assert!(graph::graph_connect(graph.0, mixer, 0, 0, 0));
            assert!(graph::graph_connect(graph.0, mixer, 1, 0, 1));
            let mut output = [0.0f32; 256];
            graph::process_next_block(graph.0, output.as_mut_ptr(), 128);
            let mut reference = state();
            let warm = render(&mut reference, 128);
            assert_eq!(output.to_vec(), warm.into_iter().flatten().collect::<Vec<_>>());
            // Deliberately arrive out of order; the graph owns the stable
            // ordering before handing each node its local slice timeline.
            let changes = [(91, 2, 0.0), (7, 0, 1.0), (91, 1, 1.0)];
            for (frame, sequence, value) in changes {
                assert!(graph::push_block_event(graph.0,
                    mixer_param_event(mixer as u64, frame, sequence, STEREO_PANNER_PARAM_MUTE, value).unwrap()));
            }
            graph::process_next_block(graph.0, output.as_mut_ptr(), 128);
            let mut expected = Vec::new();
            for frame in 0..128 {
                if frame == 7 { reference[STATE_MUTE] = 1.0; }
                if frame == 91 { reference[STATE_MUTE] = 0.0; }
                expected.extend(render(&mut reference, 1).into_iter().flatten());
            }
            assert_eq!(output.to_vec(), expected);
            assert_eq!(graph::graph_block_event_delivery_failures(graph.0), 0);

            for sequence in 0..=STEREO_PANNER_TIMELINE_CAPACITY {
                assert!(graph::push_block_event(graph.0, mixer_param_event(mixer as u64,
                    0, sequence as u32, STEREO_PANNER_PARAM_MUTE, 1.0).unwrap()));
            }
            graph::process_next_block(graph.0, output.as_mut_ptr(), 128);
            assert_eq!(graph::graph_block_event_delivery_failures(graph.0), 1,
                "a full node timeline must be detectable by the render job");
            assert!(graph::push_block_event(graph.0, mixer_param_event(mixer as u64,
                128, 0, STEREO_PANNER_PARAM_MUTE, 1.0).unwrap()));
            graph::process_next_block(graph.0, output.as_mut_ptr(), 128);
            assert_eq!(graph::graph_block_event_delivery_failures(graph.0), 2,
                "an event outside the callback must also fail strict rendering");
        }
    }
}
