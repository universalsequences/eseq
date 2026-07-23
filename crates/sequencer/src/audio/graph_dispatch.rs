#[allow(unused_imports)]
use super::*;

pub(super) unsafe fn push_param_span(lg: *mut LiveGraph, logical_id: u64, idx: u64, span: u32, value: f32) {
    for lane in 0..span.max(1) as u64 {
        params_push_wrapper(
            lg,
            ParamMsg {
                idx: idx + lane,
                logical_id,
                fvalue: value,
            },
        );
    }
}

pub(super) fn next_block_event_sequence(data: &mut AudioCallbackData) -> u32 {
    next_event_sequence_from(&mut data.event_seq)
}

pub(super) fn next_event_sequence_from(event_seq: &mut u64) -> u32 {
    let seq = *event_seq as u32;
    *event_seq = event_seq.wrapping_add(1);
    seq
}

pub(super) unsafe fn push_graph_block_event(
    lg: *mut LiveGraph,
    logical_id: u64,
    frame_offset: u32,
    sequence: u32,
    kind: u32,
    aux: &[f32],
) -> bool {
    let mut event = GraphBlockEvent {
        logical_id,
        frame_offset,
        sequence,
        kind,
        aux_count: aux.len().min(GBE_AUX_CAP) as u32,
        aux: [0.0; GBE_AUX_CAP],
    };
    let aux_count = event.aux_count as usize;
    event.aux[..aux_count].copy_from_slice(&aux[..aux_count]);
    push_block_event(lg, event)
}

#[derive(Clone, Copy, Debug)]
pub(super) struct HostTransportClock {
    pub(super) bar_phase: f32,
    pub(super) bar_phase_increment: f32,
}

pub(super) unsafe fn dispatch_voice_modulator_bpm(lg: *mut LiveGraph, modulator_id: u64, bpm: f32) {
    if modulator_id == 0 {
        return;
    }
    params_push_wrapper(
        lg,
        ParamMsg {
            idx: crate::voice_modulator::PARAM_BPM as u64,
            logical_id: modulator_id,
            fvalue: bpm.clamp(20.0, 400.0),
        },
    );
}

pub(super) unsafe fn dispatch_voice_modulator_transport_clock(
    lg: *mut LiveGraph,
    modulator_id: u64,
    clock: HostTransportClock,
) {
    if modulator_id == 0 {
        return;
    }
    params_push_wrapper(
        lg,
        ParamMsg {
            idx: crate::voice_modulator::PARAM_TRANSPORT_BAR_PHASE as u64,
            logical_id: modulator_id,
            fvalue: clock.bar_phase,
        },
    );
    params_push_wrapper(
        lg,
        ParamMsg {
            idx: crate::voice_modulator::PARAM_TRANSPORT_BAR_PHASE_INC as u64,
            logical_id: modulator_id,
            fvalue: clock.bar_phase_increment,
        },
    );
}

pub(super) unsafe fn dispatch_transport_phase(
    lg: *mut LiveGraph,
    logical_id: u64,
    param_idx: u32,
    beat_phase: f32,
) {
    if logical_id == 0 {
        return;
    }
    params_push_wrapper(
        lg,
        ParamMsg {
            idx: param_idx as u64,
            logical_id,
            fvalue: beat_phase,
        },
    );
}

/// Send a trigger to the sampler with the given per-step params, gate length, and explicit transpose.
pub(super) unsafe fn send_trigger(
    lg: *mut LiveGraph,
    lid: u64,
    frame_offset: u32,
    sequence: u32,
    velocity: f32,
    speed: f32,
    gate_samples: f32,
    attack_samples: f32,
    release_samples: f32,
    gate_mode: f32,
    transpose: f32,
    start_point: f32,
    end_point: f32,
    enabled: f32,
    reverse: f32,
    loop_mode: f32,
    loop_xfade_samples: f32,
    sr_hz: f32,
    warp_enabled: f32,
    warp_mode: f32,
    warp_ratio: f32,
    warp_sample_bpm: f32,
    warp_project_bpm: f32,
    warp_ptr_lo: f32,
    warp_ptr_hi: f32,
    warp_preserve: f32,
    warp_seg_loop_mode: f32,
    warp_seg_envelope: f32,
    scrub_offset: f32,
) {
    let mut aux = [0.0f32; SAMPLER_EVENT_AUX_NOTE_ON_COUNT];
    aux[SAMPLER_EVENT_AUX_ENABLED] = enabled;
    aux[SAMPLER_EVENT_AUX_VELOCITY] = velocity;
    aux[SAMPLER_EVENT_AUX_SPEED] = speed;
    aux[SAMPLER_EVENT_AUX_GATE_SAMPLES] = gate_samples;
    aux[SAMPLER_EVENT_AUX_TRANSPOSE] = transpose;
    aux[SAMPLER_EVENT_AUX_ATTACK_SAMPLES] = attack_samples;
    aux[SAMPLER_EVENT_AUX_RELEASE_SAMPLES] = release_samples;
    aux[SAMPLER_EVENT_AUX_GATE_MODE] = gate_mode;
    aux[SAMPLER_EVENT_AUX_START_POINT] = start_point;
    aux[SAMPLER_EVENT_AUX_END_POINT] = end_point;
    aux[SAMPLER_EVENT_AUX_REVERSE] = reverse;
    aux[SAMPLER_EVENT_AUX_LOOP_MODE] = loop_mode;
    aux[SAMPLER_EVENT_AUX_LOOP_XFADE_SAMPLES] = loop_xfade_samples;
    aux[SAMPLER_EVENT_AUX_SR_HZ] = sr_hz;
    aux[SAMPLER_EVENT_AUX_WARP_ENABLED] = warp_enabled;
    aux[SAMPLER_EVENT_AUX_WARP_MODE] = warp_mode;
    aux[SAMPLER_EVENT_AUX_WARP_RATIO] = warp_ratio;
    aux[SAMPLER_EVENT_AUX_WARP_SAMPLE_BPM] = warp_sample_bpm;
    aux[SAMPLER_EVENT_AUX_WARP_PROJECT_BPM] = warp_project_bpm;
    aux[SAMPLER_EVENT_AUX_WARP_PTR_LO] = warp_ptr_lo;
    aux[SAMPLER_EVENT_AUX_WARP_PTR_HI] = warp_ptr_hi;
    aux[SAMPLER_EVENT_AUX_SCRUB_OFFSET] = scrub_offset;
    aux[SAMPLER_EVENT_AUX_WARP_PRESERVE] = warp_preserve;
    aux[SAMPLER_EVENT_AUX_WARP_SEG_LOOP_MODE] = warp_seg_loop_mode;
    aux[SAMPLER_EVENT_AUX_WARP_SEG_ENVELOPE] = warp_seg_envelope;
    push_graph_block_event(lg, lid, frame_offset, sequence, GBE_NOTE_ON, &aux);
}

/// Send a keyboard trigger directly to a voice (no step data lookup).
pub(super) unsafe fn send_keyboard_trigger(
    lg: *mut LiveGraph,
    lid: u64,
    frame_offset: u32,
    sequence: u32,
    transpose: f32,
    velocity: f32,
    speed: f32,
    attack_samples: f32,
    release_samples: f32,
    gate_mode: f32,
    start_point: f32,
    end_point: f32,
    enabled: f32,
    reverse: f32,
    loop_mode: f32,
    loop_xfade_samples: f32,
    sr_hz: f32,
    warp_enabled: f32,
    warp_mode: f32,
    warp_ratio: f32,
    warp_sample_bpm: f32,
    warp_project_bpm: f32,
    warp_ptr_lo: f32,
    warp_ptr_hi: f32,
    warp_preserve: f32,
    warp_seg_loop_mode: f32,
    warp_seg_envelope: f32,
    scrub_offset: f32,
) {
    send_trigger(
        lg,
        lid,
        frame_offset,
        sequence,
        velocity,
        speed,
        f32::MAX,
        attack_samples,
        release_samples,
        gate_mode,
        transpose,
        start_point,
        end_point,
        enabled,
        reverse,
        loop_mode,
        loop_xfade_samples,
        sr_hz,
        warp_enabled,
        warp_mode,
        warp_ratio,
        warp_sample_bpm,
        warp_project_bpm,
        warp_ptr_lo,
        warp_ptr_hi,
        warp_preserve,
        warp_seg_loop_mode,
        warp_seg_envelope,
        scrub_offset,
    );
}

/// Send a gate-on trigger to a GatePitch node with pitch in Hz and normalized velocity.
pub(super) unsafe fn send_custom_trigger(
    lg: *mut LiveGraph,
    gatepitch_lid: u64,
    frame_offset: u32,
    sequence: u32,
    pitch_hz: f32,
    velocity: f32,
) {
    push_graph_block_event(
        lg,
        gatepitch_lid,
        frame_offset,
        sequence,
        GBE_NOTE_ON,
        &[pitch_hz, velocity],
    );
}

/// Send a gate-off to a GatePitch node.
pub(super) unsafe fn send_custom_note_off(
    lg: *mut LiveGraph,
    gatepitch_lid: u64,
    frame_offset: u32,
    sequence: u32,
) {
    push_graph_block_event(lg, gatepitch_lid, frame_offset, sequence, GBE_GATE_OFF, &[]);
}

pub(super) unsafe fn send_sampler_note_off(
    lg: *mut LiveGraph,
    sampler_lid: u64,
    frame_offset: u32,
    sequence: u32,
) {
    push_graph_block_event(lg, sampler_lid, frame_offset, sequence, GBE_GATE_OFF, &[]);
}

pub(super) unsafe fn set_modulator_gate(lg: *mut LiveGraph, modulator_lid: u64, gate: f32) {
    params_push_wrapper(
        lg,
        ParamMsg {
            idx: crate::track_modulator::PARAM_GATE,
            logical_id: modulator_lid,
            fvalue: gate.clamp(0.0, 1.0),
        },
    );
}

pub(super) unsafe fn trigger_modulator_pulse(
    lg: *mut LiveGraph,
    modulator_lid: u64,
    frame_offset: u32,
    sequence: u32,
    pulse_samples: f32,
    pulse_level: f32,
) {
    push_graph_block_event(
        lg,
        modulator_lid,
        frame_offset,
        sequence,
        GBE_PULSE,
        &[pulse_samples.max(1.0), pulse_level.clamp(0.0, 1.0)],
    );
}

pub(super) unsafe fn dispatch_modulator_params(
    lg: *mut LiveGraph,
    modulator_lid: u64,
    instrument_params: &[ScheduledInstrumentParam],
) {
    for param in instrument_params {
        if param.target != ScheduledInstrumentParamTarget::Synth {
            continue;
        }
        push_param_span(lg, modulator_lid, param.idx, param.span, param.value);
    }
}

pub(super) unsafe fn dispatch_effect_chain_for_track(
    lg: *mut LiveGraph,
    effect_params: &mut [ScheduledEffectParam],
) {
    effect_params.sort_by_key(|param| (param.logical_id, param.idx));
    for param in effect_params {
        params_push_wrapper(
            lg,
            ParamMsg {
                idx: param.idx,
                logical_id: param.logical_id,
                fvalue: param.value,
            },
        );
    }
}

pub(super) fn for_each_custom_route_lid(
    state: &SequencerState,
    engine_id: usize,
    voice_idx: usize,
    route_idx: usize,
    mut visit: impl FnMut(u64),
) {
    let (lid_l, lid_r, ext_lids): (u64, u64, [u64; crate::sequencer::EXT_MOD_INPUT_COUNT]) =
        if route_idx < MAX_TRACKS {
            (
                state.runtime.engine_route_lids[engine_id][voice_idx][route_idx]
                    .load(Ordering::Relaxed),
                state.runtime.engine_route_lids_r[engine_id][voice_idx][route_idx]
                    .load(Ordering::Relaxed),
                std::array::from_fn(|input| {
                    state.runtime.engine_ext_route_lids[engine_id][voice_idx][route_idx][input]
                        .load(Ordering::Relaxed)
                }),
            )
        } else if route_idx < crate::sequencer::MAX_SAMPLER_POOLS
            && state.runtime.rack_engine_route_engine_ids[route_idx].load(Ordering::Relaxed)
                == engine_id as u32
        {
            (
                state.runtime.rack_engine_route_lids[route_idx][voice_idx].load(Ordering::Relaxed),
                state.runtime.rack_engine_route_lids_r[route_idx][voice_idx]
                    .load(Ordering::Relaxed),
                std::array::from_fn(|input| {
                    state.runtime.rack_engine_ext_route_lids[route_idx][voice_idx][input]
                        .load(Ordering::Relaxed)
                }),
            )
        } else {
            return;
        };
    for lid in [lid_l, lid_r] {
        if lid != 0 {
            visit(lid);
        }
    }
    for ext_lid in ext_lids {
        if ext_lid != 0 {
            visit(ext_lid);
        }
    }
}

pub(super) fn for_each_custom_voice_route_update(
    state: &SequencerState,
    engine_id: usize,
    voice_idx: usize,
    previous_route: Option<usize>,
    route_idx: usize,
    mut visit: impl FnMut(u64, f32),
) {
    let mut set_route = |target_route: usize, value: f32| {
        for_each_custom_route_lid(state, engine_id, voice_idx, target_route, |lid| {
            visit(lid, value)
        });
    };
    if let Some(previous_route) = previous_route {
        if previous_route != route_idx {
            set_route(previous_route, 0.0);
        }
    } else {
        // A pool/topology reset forgets voice ownership, but route gain nodes
        // can retain their previous values. Re-establish exclusivity before
        // opening the new route so a stale rack route cannot mirror the voice.
        for stale_route in 0..MAX_TRACKS {
            if stale_route != route_idx {
                set_route(stale_route, 0.0);
            }
        }
        for stale_route in MAX_TRACKS..crate::sequencer::MAX_SAMPLER_POOLS {
            if stale_route != route_idx
                && state.runtime.rack_engine_route_engine_ids[stale_route].load(Ordering::Relaxed)
                    == engine_id as u32
            {
                set_route(stale_route, 0.0);
            }
        }
    }
    set_route(route_idx, 1.0);
}

pub(super) unsafe fn route_custom_voice_to_consumer(
    lg: *mut LiveGraph,
    state: &SequencerState,
    engine_id: usize,
    voice_idx: usize,
    previous_route: Option<usize>,
    route_idx: usize,
) {
    for_each_custom_voice_route_update(
        state,
        engine_id,
        voice_idx,
        previous_route,
        route_idx,
        |logical_id, fvalue| {
            params_push_wrapper(
                lg,
                ParamMsg {
                    idx: 0,
                    logical_id,
                    fvalue,
                },
            );
        },
    );
}

/// Dispatch instrument param values (with p-lock support) to a selected synth node.
pub(super) unsafe fn dispatch_instrument_params_to_voice(
    lg: *mut LiveGraph,
    synth_id: u64,
    modulator_id: u64,
    instrument_params: &[ScheduledInstrumentParam],
) {
    for param in instrument_params {
        let (logical_id, idx) = match param.target {
            ScheduledInstrumentParamTarget::Synth => (synth_id, param.idx),
            ScheduledInstrumentParamTarget::Modulator => (modulator_id, param.idx),
        };
        push_param_span(lg, logical_id, idx, param.span, param.value);
    }
}

pub(super) unsafe fn dispatch_instrument_tensor_params_to_voice(
    lg: *mut LiveGraph,
    synth_id: u64,
    instrument_tensor_params: &[ScheduledInstrumentTensorParam],
) {
    if synth_id == 0 {
        return;
    }
    for tensor in instrument_tensor_params {
        crate::lisp_host::queue_tensor_write(
            lg,
            synth_id as i32,
            tensor.cell_offset,
            &tensor.values,
        );
    }
}

pub(super) unsafe fn dispatch_instrument_defaults_to_voice(
    lg: *mut LiveGraph,
    state: &SequencerState,
    track_idx: usize,
    synth_id: u64,
    modulator_id: u64,
) {
    let slot = &state.pattern.instrument_slots[track_idx];
    let num_params = slot.num_params.load(Ordering::Relaxed) as usize;
    let mut param_indices: ArrayVec<usize, MAX_SLOT_PARAMS> = ArrayVec::new();
    for param_idx in 0..num_params.min(MAX_SLOT_PARAMS) {
        param_indices.push(param_idx);
    }
    param_indices.sort_by_key(|param_idx| slot.resolve_node_idx(*param_idx));
    for param_idx in param_indices {
        let idx = slot.resolve_node_idx(param_idx);
        let is_mod_param = idx as u32 >= crate::voice_modulator::MOD_PARAM_BASE;
        let logical_id = if is_mod_param { modulator_id } else { synth_id };
        let resolved_idx = if is_mod_param {
            idx - crate::voice_modulator::MOD_PARAM_BASE as u64
        } else {
            idx
        };
        push_param_span(
            lg,
            logical_id,
            resolved_idx,
            slot.resolve_node_span(param_idx),
            slot.defaults.get(param_idx),
        );
    }
    let num_tensors = slot.tensor_params.num_params();
    for tensor_idx in 0..num_tensors {
        let Some(cell_offset) = slot.tensor_params.tensor_cell_offset(tensor_idx) else {
            continue;
        };
        let Some(values) = slot.tensor_params.default_values(tensor_idx) else {
            continue;
        };
        crate::lisp_host::queue_tensor_write(lg, synth_id as i32, cell_offset, &values);
    }
}

pub(super) unsafe fn dispatch_sampler_modulator_params_to_voice(
    lg: *mut LiveGraph,
    modulator_id: u64,
    instrument_params: &[ScheduledInstrumentParam],
) {
    if modulator_id == 0 {
        return;
    }
    for param in instrument_params {
        if param.target != ScheduledInstrumentParamTarget::Modulator {
            continue;
        }
        push_param_span(lg, modulator_id, param.idx, param.span, param.value);
    }
}

pub(super) unsafe fn dispatch_sampler_extra_params_to_voice(
    lg: *mut LiveGraph,
    sampler_lid: u64,
    instrument_params: &[ScheduledInstrumentParam],
) {
    for param in instrument_params {
        if param.target != ScheduledInstrumentParamTarget::Synth {
            continue;
        }
        if param.idx < crate::sampler::PARAM_SCRUB_OFFSET {
            continue;
        }
        push_param_span(lg, sampler_lid, param.idx, param.span, param.value);
    }
}

pub(super) fn sampler_live_param_value(idx: u64, value: f32, sample_rate: f64) -> f32 {
    if idx == PARAM_ATTACK_SAMPLES
        || idx == PARAM_RELEASE_SAMPLES
        || idx == PARAM_LOOP_XFADE_SAMPLES
    {
        // Sampler p-lock values for these UI params are stored in ms; the DSP
        // node consumes samples.
        value * sample_rate as f32 / 1000.0
    } else {
        value
    }
}

pub(super) unsafe fn dispatch_sampler_live_params_to_voice(
    lg: *mut LiveGraph,
    sampler_lid: u64,
    modulator_id: u64,
    instrument_params: &[ScheduledInstrumentParam],
    sample_rate: f64,
) {
    for param in instrument_params {
        match param.target {
            ScheduledInstrumentParamTarget::Synth => {
                push_param_span(
                    lg,
                    sampler_lid,
                    param.idx,
                    param.span,
                    sampler_live_param_value(param.idx, param.value, sample_rate),
                );
            }
            ScheduledInstrumentParamTarget::Modulator => {
                if modulator_id != 0 {
                    push_param_span(lg, modulator_id, param.idx, param.span, param.value);
                }
            }
        }
    }
}

pub(super) fn dispatch_instrument_params_to_active_voices(
    data: &mut AudioCallbackData,
    track_idx: usize,
    instrument_params: &[ScheduledInstrumentParam],
) {
    if instrument_params.is_empty() {
        return;
    }
    if let Some(engine_id) = track_engine_id(&data.state, track_idx) {
        let pool = &mut data.custom_engine_pools[engine_id];
        let free_patch =
            track_custom_run_mode(&data.state, track_idx) == CustomInstrumentRunMode::FreePatch;
        for voice_idx in 0..pool.num_voices {
            let targets_voice = if free_patch {
                voice_idx == 0
            } else {
                pool.voices[voice_idx].active
                    && pool.voices[voice_idx].assigned_track == Some(track_idx)
            };
            if !targets_voice {
                continue;
            }
            let synth_id = data.state.runtime.engine_synth_node_ids[engine_id][voice_idx]
                .load(Ordering::Relaxed);
            let modulator_id = data.state.runtime.engine_modulator_node_ids[engine_id][voice_idx]
                .load(Ordering::Relaxed);
            if synth_id == 0 {
                continue;
            }
            unsafe {
                dispatch_instrument_params_to_voice(
                    data.lg.0,
                    synth_id as u64,
                    modulator_id as u64,
                    instrument_params,
                );
            }
            // Force re-resolve on the next trigger because live p-locks can
            // diverge the active voice from descriptor defaults.
            pool.voices[voice_idx].fingerprint = 0;
        }
    } else {
        let pool = &data.voice_pools[track_idx];
        for voice in pool.voices[..pool.num_voices]
            .iter()
            .filter(|voice| voice.active && voice.logical_id != 0)
        {
            unsafe {
                dispatch_sampler_live_params_to_voice(
                    data.lg.0,
                    voice.logical_id,
                    voice.modulator_id as u64,
                    instrument_params,
                    data.sample_rate,
                );
            }
        }
    }
}

pub(super) fn dispatch_instrument_tensor_params_to_active_voices(
    data: &mut AudioCallbackData,
    track_idx: usize,
    instrument_tensor_params: &[ScheduledInstrumentTensorParam],
) {
    if instrument_tensor_params.is_empty() {
        return;
    }
    let Some(engine_id) = track_engine_id(&data.state, track_idx) else {
        return;
    };
    let pool = &mut data.custom_engine_pools[engine_id];
    let free_patch =
        track_custom_run_mode(&data.state, track_idx) == CustomInstrumentRunMode::FreePatch;
    for voice_idx in 0..pool.num_voices {
        let targets_voice = if free_patch {
            voice_idx == 0
        } else {
            pool.voices[voice_idx].active
                && pool.voices[voice_idx].assigned_track == Some(track_idx)
        };
        if !targets_voice {
            continue;
        }
        let synth_id =
            data.state.runtime.engine_synth_node_ids[engine_id][voice_idx].load(Ordering::Relaxed);
        if synth_id == 0 {
            continue;
        }
        unsafe {
            dispatch_instrument_tensor_params_to_voice(
                data.lg.0,
                synth_id as u64,
                instrument_tensor_params,
            );
        }
        pool.voices[voice_idx].fingerprint = 0;
    }
}

pub(super) unsafe fn dispatch_sampler_modulator_defaults_to_voice(
    lg: *mut LiveGraph,
    state: &SequencerState,
    track_idx: usize,
    modulator_id: u64,
) {
    if modulator_id == 0 {
        return;
    }
    let slot = &state.pattern.instrument_slots[track_idx];
    let num_params = slot.num_params.load(Ordering::Relaxed) as usize;
    for param_idx in 0..num_params {
        let idx = slot.resolve_node_idx(param_idx);
        if (idx as u32) < crate::voice_modulator::MOD_PARAM_BASE {
            continue;
        }
        params_push_wrapper(
            lg,
            ParamMsg {
                idx: idx - crate::voice_modulator::MOD_PARAM_BASE as u64,
                logical_id: modulator_id,
                fvalue: slot.defaults.get(param_idx),
            },
        );
    }
}

pub(super) unsafe fn dispatch_sampler_extra_defaults_to_voice(
    lg: *mut LiveGraph,
    state: &SequencerState,
    track_idx: usize,
    sampler_lid: u64,
) {
    let slot = &state.pattern.instrument_slots[track_idx];
    let num_params = slot.num_params.load(Ordering::Relaxed) as usize;
    for param_idx in 0..num_params {
        let idx = slot.resolve_node_idx(param_idx);
        if idx < crate::sampler::PARAM_SCRUB_OFFSET
            || idx as u32 >= crate::voice_modulator::MOD_PARAM_BASE
        {
            continue;
        }
        params_push_wrapper(
            lg,
            ParamMsg {
                idx,
                logical_id: sampler_lid,
                fvalue: slot.defaults.get(param_idx),
            },
        );
    }
}
