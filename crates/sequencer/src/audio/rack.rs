/*!
Rack-slot processing: routing, macros, choke groups, and rack note firing.

Decides which slots of a rack track accept a trigger (routing, key ranges,
mute/solo), applies rack macro curves at a step, collects and dispatches
slot note-offs and choke-group releases, and fires notes into rack slots —
`fire_rack_slot_note`/`fire_rack_resolved` for sequenced triggers and
`fire_live_keyboard_rack_note` for live keyboard input.
*/

#[allow(unused_imports)]
use super::*;

pub(super) fn rack_slot_accepts_trigger(slot: &RackSlotSnapshot, has_solo: bool) -> bool {
    if has_solo {
        slot.solo && !slot.mute
    } else {
        !slot.mute
    }
}

#[derive(Clone, Copy, Debug)]
pub(super) struct ResolvedRackSlotParams {
    pub(super) base_note_offset: f32,
    pub(super) gain: f32,
    pub(super) pan: f32,
    pub(super) max_polyphony: usize,
    pub(super) mute: bool,
    pub(super) solo: bool,
}

pub(super) fn resolve_rack_slot_params(slot: &RackSlotSnapshot, step: usize) -> ResolvedRackSlotParams {
    let value = |param: RackSlotParam| param.clamp(slot.param_value_at_step(param, step));
    let max_polyphony = value(RackSlotParam::MaxPolyphony)
        .round()
        .clamp(1.0, MAX_VOICES as f32) as usize;
    ResolvedRackSlotParams {
        base_note_offset: value(RackSlotParam::BaseNote),
        gain: value(RackSlotParam::Gain),
        pan: value(RackSlotParam::Pan),
        max_polyphony,
        mute: value(RackSlotParam::Mute) > 0.5,
        solo: value(RackSlotParam::Solo) > 0.5,
    }
}

pub(super) fn rack_macro_curve_value(curve: crate::sequencer::RackMacroCurve, value: f32) -> f32 {
    match curve {
        crate::sequencer::RackMacroCurve::Linear => value,
        crate::sequencer::RackMacroCurve::Exp => value * value,
        crate::sequencer::RackMacroCurve::Log => value.sqrt(),
    }
}

pub(super) fn apply_rack_macros_at_step(
    rack: &mut RackTrackSnapshot,
    step: usize,
    process_values: [Option<f32>; crate::sequencer::RACK_MACRO_COUNT],
) {
    let macros = rack.macros.clone();
    for rack_macro in macros {
        let normalized = process_values
            .get(rack_macro.id.index())
            .and_then(|value| *value)
            .unwrap_or_else(|| {
                rack.runtime_macro_value_at(rack_macro.id, step)
                    .unwrap_or_else(|| rack_macro.value_at(step))
            });
        for mapping in rack_macro.mappings {
            let value = mapping.range_min
                + (mapping.range_max - mapping.range_min)
                    * rack_macro_curve_value(mapping.curve, normalized);
            match mapping.target {
                crate::sequencer::RackMacroTarget::SlotParam { slot, param } => {
                    let Some(slot) = rack.slots.get_mut(slot) else {
                        continue;
                    };
                    let normalized = param
                        .trim_start_matches(':')
                        .replace('_', "-")
                        .to_ascii_lowercase();
                    let target = match normalized.as_str() {
                        "base-note" | "transpose" => Some(RackSlotParam::BaseNote),
                        "gain" => Some(RackSlotParam::Gain),
                        "pan" => Some(RackSlotParam::Pan),
                        "max-polyphony" | "polyphony" => Some(RackSlotParam::MaxPolyphony),
                        "mute" => Some(RackSlotParam::Mute),
                        "solo" => Some(RackSlotParam::Solo),
                        _ => None,
                    };
                    let Some(target) = target else {
                        continue;
                    };
                    if slot.param_plocks.get(step, target).is_some() {
                        continue;
                    }
                    match target {
                        RackSlotParam::BaseNote => {
                            slot.instrument_base_note_offset = target.clamp(value)
                        }
                        RackSlotParam::Gain => slot.gain = target.clamp(value),
                        RackSlotParam::Pan => slot.pan = target.clamp(value),
                        RackSlotParam::MaxPolyphony => {
                            slot.max_polyphony = target.clamp(value).round() as usize
                        }
                        RackSlotParam::Mute => slot.mute = value >= 0.5,
                        RackSlotParam::Solo => slot.solo = value >= 0.5,
                    }
                }
                crate::sequencer::RackMacroTarget::SlotInstrumentParam {
                    slot,
                    param_index,
                    ..
                } => {
                    let Some(slot) = rack.slots.get_mut(slot) else {
                        continue;
                    };
                    let locked = slot
                        .instrument_slot
                        .plocks
                        .get(step)
                        .and_then(|row| row.get(param_index))
                        .and_then(|value| *value)
                        .is_some();
                    if !locked {
                        if let Some(default) = slot.instrument_slot.defaults.get_mut(param_index) {
                            *default = value;
                        }
                    }
                }
                crate::sequencer::RackMacroTarget::SlotEffectParam {
                    slot,
                    effect_slot,
                    param_index,
                    ..
                } => {
                    let Some(effect) = rack
                        .slots
                        .get_mut(slot)
                        .and_then(|slot| slot.effect_slots.get_mut(effect_slot))
                    else {
                        continue;
                    };
                    let locked = effect
                        .plocks
                        .get(step)
                        .and_then(|row| row.get(param_index))
                        .and_then(|value| *value)
                        .is_some();
                    if !locked {
                        if let Some(default) = effect.defaults.get_mut(param_index) {
                            *default = value;
                        }
                    }
                }
            }
        }
    }
}

pub(super) fn rack_slot_accepts_resolved(params: ResolvedRackSlotParams, has_solo: bool) -> bool {
    if has_solo {
        params.solo && !params.mute
    } else {
        !params.mute
    }
}

pub(super) fn rack_slot_matches_routing(
    slot: &RackSlotSnapshot,
    routing: RackRouting,
    transpose: f32,
) -> bool {
    match routing {
        RackRouting::Broadcast => true,
        RackRouting::ByPitch => slot.pad_note == Some(transpose.round() as i32),
    }
}

pub(super) fn rack_slot_playback_transpose(routing: RackRouting, transpose: f32) -> f32 {
    match routing {
        RackRouting::Broadcast => transpose,
        RackRouting::ByPitch => 0.0,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum RackSlotNoteOff {
    Custom { logical_id: u64 },
    Sampler { logical_id: u64 },
}

pub(super) fn collect_rack_slot_active_voice_releases(
    voice_pools: &mut [VoicePool],
    custom_engine_pools: &mut [CustomEnginePool],
    countdown_events: &mut Vec<CountdownEvent>,
    block_events: &mut Vec<BlockEvent>,
    track_idx: usize,
    slot_idx: usize,
    slot: &RackSlotSnapshot,
    release_sample: u64,
) -> Vec<RackSlotNoteOff> {
    let mut note_offs = Vec::new();
    match slot.instrument_type {
        InstrumentType::Sampler => {
            let Some(pool_id) = rack_slot_pool_index(track_idx, slot_idx) else {
                return note_offs;
            };
            if pool_id >= voice_pools.len() {
                return note_offs;
            }
            let active: Vec<(u64, i32)> = voice_pools[pool_id].voices
                [..voice_pools[pool_id].num_voices]
                .iter()
                .filter(|voice| voice.active && voice.logical_id != 0)
                .map(|voice| (voice.logical_id, voice.gatepitch_id))
                .collect();
            for (lid, gatepitch_id) in active {
                voice_pools[pool_id].release_voice_by_logical_id(lid);
                cancel_gate_off_for_lid(countdown_events, block_events, lid);
                if gatepitch_id > 0 {
                    note_offs.push(RackSlotNoteOff::Custom {
                        logical_id: gatepitch_id as u64,
                    });
                }
                note_offs.push(RackSlotNoteOff::Sampler { logical_id: lid });
            }
        }
        InstrumentType::Custom => {
            let Some(engine_id) = slot.track_sound_state.engine_id else {
                return note_offs;
            };
            if engine_id >= custom_engine_pools.len() {
                return note_offs;
            }
            let free_patch = slot.instrument_run_mode == CustomInstrumentRunMode::FreePatch;
            let route_idx = rack_slot_pool_index(track_idx, slot_idx)
                .expect("validated rack slot must have a route identity");
            let lids: Vec<u64> = custom_engine_pools[engine_id].voices
                [..custom_engine_pools[engine_id].num_voices]
                .iter()
                .filter(|voice| voice.active && voice.assigned_route == Some(route_idx))
                .map(|voice| voice.logical_id)
                .collect();
            for lid in lids {
                if free_patch {
                    custom_engine_pools[engine_id].release_free_patch_voice_by_logical_id(lid);
                } else {
                    custom_engine_pools[engine_id].release_voice_by_logical_id(lid, release_sample);
                }
                cancel_gate_off_for_lid(countdown_events, block_events, lid);
                note_offs.push(RackSlotNoteOff::Custom { logical_id: lid });
            }
        }
        InstrumentType::Modulator | InstrumentType::Rack => {}
    }
    note_offs
}

pub(super) fn collect_rack_choke_group_voice_releases(
    voice_pools: &mut [VoicePool],
    custom_engine_pools: &mut [CustomEnginePool],
    countdown_events: &mut Vec<CountdownEvent>,
    block_events: &mut Vec<BlockEvent>,
    parent_track_idx: usize,
    rack: &RackTrackSnapshot,
    triggering_slot_idx: usize,
    choke_group: u8,
    release_sample: u64,
) -> Vec<RackSlotNoteOff> {
    let mut note_offs = Vec::new();
    for (slot_idx, slot) in rack.slots.iter().enumerate() {
        if slot_idx == triggering_slot_idx || slot.choke_group != Some(choke_group) {
            continue;
        }
        note_offs.extend(collect_rack_slot_active_voice_releases(
            voice_pools,
            custom_engine_pools,
            countdown_events,
            block_events,
            parent_track_idx,
            slot_idx,
            slot,
            release_sample,
        ));
    }
    note_offs
}

pub(super) fn dispatch_rack_slot_note_offs(
    data: &mut AudioCallbackData,
    frame_offset: u32,
    note_offs: Vec<RackSlotNoteOff>,
) {
    for note_off in note_offs {
        let seq = next_block_event_sequence(data);
        unsafe {
            match note_off {
                RackSlotNoteOff::Custom { logical_id } => {
                    send_custom_note_off(data.lg.0, logical_id, frame_offset, seq);
                }
                RackSlotNoteOff::Sampler { logical_id } => {
                    send_sampler_note_off(data.lg.0, logical_id, frame_offset, seq);
                }
            }
        }
    }
}

pub(super) fn release_rack_choke_group_voices(
    data: &mut AudioCallbackData,
    parent_track_idx: usize,
    rack: &RackTrackSnapshot,
    triggering_slot_idx: usize,
    choke_group: u8,
    frame_offset: u32,
) {
    let release_sample = data.rendered_samples.load(Ordering::Acquire) + frame_offset as u64;
    let note_offs = collect_rack_choke_group_voice_releases(
        &mut data.voice_pools,
        &mut data.custom_engine_pools,
        &mut data.countdown_events,
        &mut data.block_events,
        parent_track_idx,
        rack,
        triggering_slot_idx,
        choke_group,
        release_sample,
    );
    dispatch_rack_slot_note_offs(data, frame_offset, note_offs);
}

pub(super) unsafe fn push_rack_slot_panner_params(
    lg: *mut LiveGraph,
    slot_pan_lid: u64,
    params: ResolvedRackSlotParams,
    muted_by_solo: bool,
) {
    if slot_pan_lid == 0 {
        return;
    }
    params_push_wrapper(
        lg,
        ParamMsg {
            idx: crate::effects::stereo_panner::STEREO_PANNER_PARAM_VOLUME,
            logical_id: slot_pan_lid,
            fvalue: params.gain,
        },
    );
    params_push_wrapper(
        lg,
        ParamMsg {
            idx: crate::effects::stereo_panner::STEREO_PANNER_PARAM_PAN,
            logical_id: slot_pan_lid,
            fvalue: params.pan,
        },
    );
    params_push_wrapper(
        lg,
        ParamMsg {
            idx: crate::effects::stereo_panner::STEREO_PANNER_PARAM_MUTE,
            logical_id: slot_pan_lid,
            fvalue: if params.mute { 1.0 } else { 0.0 },
        },
    );
    params_push_wrapper(
        lg,
        ParamMsg {
            idx: crate::effects::stereo_panner::STEREO_PANNER_PARAM_MUTED_BY_SOLO,
            logical_id: slot_pan_lid,
            fvalue: if muted_by_solo { 1.0 } else { 0.0 },
        },
    );
}

pub(super) fn rack_sampler_warp_runtime(
    state: &SequencerState,
    warp_enabled: f32,
    warp_mode: f32,
    sample_bpm: f32,
) -> (f32, f32, f32, f32, f32, f32, f32) {
    let project_bpm = state.transport.bpm.load(Ordering::Relaxed).max(1) as f32;
    let sample_bpm = sample_bpm.clamp(20.0, 400.0);
    if warp_enabled <= 0.5 {
        return (0.0, warp_mode, 1.0, sample_bpm, project_bpm, 0.0, 0.0);
    }
    // All warp modes run without analysis now (Beats falls back to the pure
    // beat grid when no onset table is present), so racks support every mode.
    let ratio = (project_bpm / sample_bpm).clamp(0.01, 32.0);
    (1.0, warp_mode, ratio, sample_bpm, project_bpm, 0.0, 0.0)
}

pub(super) fn fire_live_keyboard_rack_note(
    data: &mut AudioCallbackData,
    parent_track_idx: usize,
    trigger: &KeyboardTrigger,
    transpose: f32,
    rack: RackTrackSnapshot,
) -> bool {
    let gate_mode = if data.state.pattern.track_params[parent_track_idx].is_gate_on() {
        1.0
    } else {
        0.0
    };
    let has_solo = rack.slots.iter().any(|slot| slot.solo);
    let mut active_voices = [ActiveKeyboardVoice::default(); MAX_RACK_SLOTS];
    let mut active_voice_count = 0;

    for (slot_idx, slot) in rack.slots.iter().enumerate() {
        if !rack_slot_matches_routing(slot, rack.routing, transpose) {
            continue;
        }
        if !rack_slot_accepts_trigger(slot, has_solo) {
            continue;
        }
        if let Some(choke_group) = slot.choke_group {
            release_rack_choke_group_voices(
                data,
                parent_track_idx,
                &rack,
                slot_idx,
                choke_group,
                0,
            );
        }
        let playback_transpose = rack_slot_playback_transpose(rack.routing, transpose);
        let instrument_params = resolve_rack_slot_instrument_defaults(&slot.instrument_slot);
        match slot.instrument_type {
            InstrumentType::Sampler => {
                let Some(pool_id) = rack_slot_pool_index(parent_track_idx, slot_idx) else {
                    continue;
                };
                if pool_id >= data.voice_pools.len() {
                    continue;
                }
                let sampler_lid = data.state.runtime.sampler_lids[pool_id].load(Ordering::Acquire);
                if sampler_lid == 0 {
                    continue;
                }
                let sampler_params = resolve_rack_slot_sampler_defaults(&slot.instrument_slot);
                let attack_samples = sampler_params.attack_ms * data.sample_rate as f32 / 1000.0;
                let release_samples = sampler_params.release_ms * data.sample_rate as f32 / 1000.0;
                let loop_xfade_samples =
                    sampler_params.loop_xfade_ms * data.sample_rate as f32 / 1000.0;
                let (
                    warp_enabled,
                    warp_mode,
                    warp_ratio,
                    warp_sample_bpm,
                    warp_project_bpm,
                    warp_ptr_lo,
                    warp_ptr_hi,
                ) = rack_sampler_warp_runtime(
                    &data.state,
                    sampler_params.warp_enabled,
                    sampler_params.warp_mode,
                    sampler_params.sample_bpm,
                );
                data.voice_pools[pool_id].polyphonic = slot.max_polyphony > 1;
                let (voice_lid, gatepitch_id, modulator_id) = {
                    let voice = data.voice_pools[pool_id]
                        .allocate_voice_retriggering_same_note_with_limit(
                            playback_transpose,
                            slot.max_polyphony,
                        );
                    (voice.logical_id, voice.gatepitch_id, voice.modulator_id)
                };
                if voice_lid == 0 {
                    continue;
                }
                if modulator_id > 0 {
                    let gatepitch_seq = next_event_sequence_from(&mut data.event_seq);
                    unsafe {
                        dispatch_sampler_modulator_params_to_voice(
                            data.lg.0,
                            modulator_id as u64,
                            &instrument_params,
                        );
                        send_custom_trigger(
                            data.lg.0,
                            gatepitch_id as u64,
                            0,
                            gatepitch_seq,
                            custom_pitch_hz(
                                playback_transpose + slot.instrument_base_note_offset,
                                0.0,
                            ),
                            trigger.velocity,
                        );
                    }
                }
                let sampler_seq = next_event_sequence_from(&mut data.event_seq);
                unsafe {
                    send_keyboard_trigger(
                        data.lg.0,
                        voice_lid,
                        0,
                        sampler_seq,
                        playback_transpose + slot.instrument_base_note_offset,
                        trigger.velocity,
                        sampler_params.playback_speed,
                        attack_samples,
                        release_samples,
                        gate_mode,
                        sampler_params.start_point,
                        sampler_params.end_point,
                        sampler_params.instrument_enabled,
                        sampler_params.reverse,
                        sampler_params.loop_mode,
                        loop_xfade_samples,
                        sampler_params.sr_hz,
                        warp_enabled,
                        warp_mode,
                        warp_ratio,
                        warp_sample_bpm,
                        warp_project_bpm,
                        warp_ptr_lo,
                        warp_ptr_hi,
                        sampler_params.warp_preserve,
                        sampler_params.warp_seg_loop_mode,
                        sampler_params.warp_seg_envelope,
                        sampler_params.scrub,
                    );
                    dispatch_sampler_extra_params_to_voice(
                        data.lg.0,
                        voice_lid,
                        &instrument_params,
                    );
                }
                push_active_keyboard_voice(
                    &mut active_voices,
                    &mut active_voice_count,
                    ActiveKeyboardVoice {
                        logical_id: voice_lid,
                        gatepitch_id,
                        target: ActiveKeyboardVoiceTarget::Sampler { pool_id },
                    },
                );
            }
            InstrumentType::Custom => {
                let Some(engine_id) = slot.track_sound_state.engine_id else {
                    continue;
                };
                if engine_id >= data.custom_engine_pools.len() {
                    continue;
                }
                let free_patch = slot.instrument_run_mode == CustomInstrumentRunMode::FreePatch;
                let allocation = if free_patch {
                    let Some(allocation) = data.custom_engine_pools[engine_id]
                        .allocate_free_patch_voice(
                            parent_track_idx,
                            rack_slot_pool_index(parent_track_idx, slot_idx)
                                .expect("validated rack slot must have a route identity"),
                            playback_transpose,
                        )
                    else {
                        continue;
                    };
                    allocation
                } else {
                    data.custom_engine_pools[engine_id].allocate_voice(
                        parent_track_idx,
                        rack_slot_pool_index(parent_track_idx, slot_idx)
                            .expect("validated rack slot must have a route identity"),
                        playback_transpose,
                        slot.max_polyphony > 1,
                        slot.max_polyphony,
                    )
                };
                let voice_idx = allocation.voice_idx;
                data.custom_engine_pools[engine_id].note_voice_allocated(engine_id, voice_idx);
                let voice_lid = allocation.logical_id;
                let synth_id = data.state.runtime.engine_synth_node_ids[engine_id][voice_idx]
                    .load(Ordering::Relaxed);
                let modulator_id = data.state.runtime.engine_modulator_node_ids[engine_id]
                    [voice_idx]
                    .load(Ordering::Relaxed);
                if voice_lid == 0 || synth_id == 0 || modulator_id == 0 {
                    continue;
                }
                let key_locked_instrument_params = key_locked_snapshot_instrument_params(
                    &slot.instrument_slot,
                    playback_transpose,
                    slot.instrument_base_note_offset,
                    None,
                    &instrument_params,
                );
                let instrument_fingerprint = rack_slot_sound_fingerprint(
                    slot,
                    &key_locked_instrument_params,
                    slot.instrument_base_note_offset,
                );
                let pitch_hz =
                    custom_pitch_hz(playback_transpose, slot.instrument_base_note_offset);
                cancel_gate_off_for_lid(
                    &mut data.countdown_events,
                    &mut data.block_events,
                    voice_lid,
                );
                unsafe {
                    route_custom_voice_to_consumer(
                        data.lg.0,
                        &data.state,
                        engine_id,
                        voice_idx,
                        allocation.previous_route,
                        rack_slot_pool_index(parent_track_idx, slot_idx)
                            .expect("validated rack slot must have a route identity"),
                    );
                    if data.custom_engine_pools[engine_id].voices[voice_idx].fingerprint
                        != instrument_fingerprint
                    {
                        dispatch_instrument_params_to_voice(
                            data.lg.0,
                            synth_id as u64,
                            modulator_id as u64,
                            &key_locked_instrument_params,
                        );
                    }
                    if allocation.stole_active_voice || slot.max_polyphony <= 1 || free_patch {
                        let off_seq = next_event_sequence_from(&mut data.event_seq);
                        send_custom_note_off(data.lg.0, voice_lid, 0, off_seq);
                    }
                    let on_seq = next_event_sequence_from(&mut data.event_seq);
                    send_custom_trigger(
                        data.lg.0,
                        voice_lid,
                        0,
                        on_seq,
                        pitch_hz,
                        trigger.velocity,
                    );
                }
                data.custom_engine_pools[engine_id].voices[voice_idx].fingerprint =
                    instrument_fingerprint;
                push_active_keyboard_voice(
                    &mut active_voices,
                    &mut active_voice_count,
                    ActiveKeyboardVoice {
                        logical_id: voice_lid,
                        gatepitch_id: 0,
                        target: ActiveKeyboardVoiceTarget::Custom {
                            engine_id,
                            free_patch,
                        },
                    },
                );
            }
            InstrumentType::Modulator | InstrumentType::Rack => {}
        }
    }

    if active_voice_count == 0 {
        return false;
    }
    store_active_keyboard_note(
        &mut data.active_keyboard_notes,
        parent_track_idx,
        trigger.transpose,
        midi_note_from_transpose(
            transpose,
            f32::from_bits(
                data.state.pattern.instrument_base_note_offsets[parent_track_idx]
                    .load(Ordering::Relaxed),
            ),
        ),
        &active_voices[..active_voice_count],
    );
    true
}

#[allow(clippy::too_many_arguments)]
pub(super) fn fire_rack_slot_note(
    data: &mut AudioCallbackData,
    frame_offset: u32,
    parent_track_idx: usize,
    slot_idx: usize,
    slot: &RackSlotSnapshot,
    slot_params: ResolvedRackSlotParams,
    transpose: f32,
    velocity: f32,
    speed: f32,
    gate_samples: f32,
    gate_mode: f32,
    instrument_params: &ScheduledInstrumentParams,
    sampler_params: Option<ScheduledSamplerParams>,
    instrument_fingerprint: u64,
) {
    match slot.instrument_type {
        InstrumentType::Sampler => {
            let Some(pool_id) = rack_slot_pool_index(parent_track_idx, slot_idx) else {
                return;
            };
            if pool_id >= data.voice_pools.len() {
                return;
            }
            let sampler_lid = data.state.runtime.sampler_lids[pool_id].load(Ordering::Acquire);
            if sampler_lid == 0 {
                return;
            }
            let sampler_params = sampler_params.unwrap_or_default();
            let attack_samples = sampler_params.attack_ms * data.sample_rate as f32 / 1000.0;
            let release_samples = sampler_params.release_ms * data.sample_rate as f32 / 1000.0;
            let loop_xfade_samples =
                sampler_params.loop_xfade_ms * data.sample_rate as f32 / 1000.0;
            let (
                warp_enabled,
                warp_mode,
                warp_ratio,
                warp_sample_bpm,
                warp_project_bpm,
                warp_ptr_lo,
                warp_ptr_hi,
            ) = rack_sampler_warp_runtime(
                &data.state,
                sampler_params.warp_enabled,
                sampler_params.warp_mode,
                sampler_params.sample_bpm,
            );
            data.voice_pools[pool_id].polyphonic = slot_params.max_polyphony > 1;
            let voice = data.voice_pools[pool_id].allocate_voice_retriggering_same_note_with_limit(
                transpose,
                slot_params.max_polyphony,
            );
            let voice_lid = voice.logical_id;
            let lid = if voice_lid != 0 {
                voice_lid
            } else {
                sampler_lid
            };
            let gatepitch_id = voice.gatepitch_id;
            if voice.modulator_id > 0 {
                let gatepitch_seq = next_event_sequence_from(&mut data.event_seq);
                unsafe {
                    dispatch_sampler_modulator_params_to_voice(
                        data.lg.0,
                        voice.modulator_id as u64,
                        instrument_params,
                    );
                    send_custom_trigger(
                        data.lg.0,
                        voice.gatepitch_id as u64,
                        frame_offset,
                        gatepitch_seq,
                        custom_pitch_hz(transpose + slot_params.base_note_offset, 0.0),
                        velocity,
                    );
                }
            }
            let sampler_seq = next_event_sequence_from(&mut data.event_seq);
            unsafe {
                dispatch_sampler_extra_params_to_voice(data.lg.0, lid, instrument_params);
                send_trigger(
                    data.lg.0,
                    lid,
                    frame_offset,
                    sampler_seq,
                    velocity,
                    speed * sampler_params.playback_speed,
                    gate_samples,
                    attack_samples,
                    release_samples,
                    gate_mode,
                    transpose + slot_params.base_note_offset,
                    sampler_params.start_point,
                    sampler_params.end_point,
                    sampler_params.instrument_enabled,
                    sampler_params.reverse,
                    sampler_params.loop_mode,
                    loop_xfade_samples,
                    sampler_params.sr_hz,
                    warp_enabled,
                    warp_mode,
                    warp_ratio,
                    warp_sample_bpm,
                    warp_project_bpm,
                    warp_ptr_lo,
                    warp_ptr_hi,
                    sampler_params.warp_preserve,
                    sampler_params.warp_seg_loop_mode,
                    sampler_params.warp_seg_envelope,
                    sampler_params.scrub,
                );
            }
            if gate_mode > 0.5 {
                schedule_gate_off_event(
                    data,
                    pool_id,
                    lid,
                    frame_offset,
                    gate_samples as f64,
                    GateOffTarget::Sampler { gatepitch_id },
                );
            }
        }
        InstrumentType::Custom => {
            let Some(engine_id) = slot.track_sound_state.engine_id else {
                return;
            };
            if engine_id >= data.custom_engine_pools.len() {
                return;
            }
            let free_patch = slot.instrument_run_mode == CustomInstrumentRunMode::FreePatch;
            let allocation = if free_patch {
                let Some(allocation) = data.custom_engine_pools[engine_id]
                    .allocate_free_patch_voice(
                        parent_track_idx,
                        rack_slot_pool_index(parent_track_idx, slot_idx)
                            .expect("validated rack slot must have a route identity"),
                        transpose,
                    )
                else {
                    return;
                };
                allocation
            } else {
                data.custom_engine_pools[engine_id].allocate_voice(
                    parent_track_idx,
                    rack_slot_pool_index(parent_track_idx, slot_idx)
                        .expect("validated rack slot must have a route identity"),
                    transpose,
                    slot_params.max_polyphony > 1,
                    slot_params.max_polyphony,
                )
            };
            let voice_idx = allocation.voice_idx;
            data.custom_engine_pools[engine_id].note_voice_allocated(engine_id, voice_idx);
            let lid = allocation.logical_id;
            let synth_id = data.state.runtime.engine_synth_node_ids[engine_id][voice_idx]
                .load(Ordering::Relaxed);
            let modulator_id = data.state.runtime.engine_modulator_node_ids[engine_id][voice_idx]
                .load(Ordering::Relaxed);
            if lid == 0 || synth_id == 0 || modulator_id == 0 {
                return;
            }
            let pitch_hz = custom_pitch_hz(transpose, slot_params.base_note_offset);
            cancel_gate_off_for_lid(&mut data.countdown_events, &mut data.block_events, lid);
            unsafe {
                if allocation.stole_active_voice || slot_params.max_polyphony <= 1 || free_patch {
                    let off_seq = next_event_sequence_from(&mut data.event_seq);
                    send_custom_note_off(data.lg.0, lid, frame_offset, off_seq);
                }
                route_custom_voice_to_consumer(
                    data.lg.0,
                    &data.state,
                    engine_id,
                    voice_idx,
                    allocation.previous_route,
                    rack_slot_pool_index(parent_track_idx, slot_idx)
                        .expect("validated rack slot must have a route identity"),
                );
                if data.custom_engine_pools[engine_id].voices[voice_idx].fingerprint
                    != instrument_fingerprint
                {
                    dispatch_instrument_params_to_voice(
                        data.lg.0,
                        synth_id as u64,
                        modulator_id as u64,
                        instrument_params,
                    );
                }
            }
            data.custom_engine_pools[engine_id].voices[voice_idx].fingerprint =
                instrument_fingerprint;
            let on_seq = next_event_sequence_from(&mut data.event_seq);
            unsafe {
                send_custom_trigger(data.lg.0, lid, frame_offset, on_seq, pitch_hz, velocity);
            }
            if gate_mode > 0.5 {
                schedule_gate_off_event(
                    data,
                    parent_track_idx,
                    lid,
                    frame_offset,
                    gate_samples as f64,
                    GateOffTarget::Custom {
                        engine_id,
                        free_patch,
                    },
                );
            }
        }
        InstrumentType::Modulator | InstrumentType::Rack => {}
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn fire_rack_resolved(
    data: &mut AudioCallbackData,
    frame_offset: u32,
    track_idx: usize,
    step: usize,
    key_lock_plock_step: Option<usize>,
    samples_per_step: f64,
    resolved: crate::accumulator::ResolvedStep,
    chord: crate::scheduled_event::ScheduledChordData,
    mut rack: RackTrackSnapshot,
    rack_macro_values: [Option<f32>; crate::sequencer::RACK_MACRO_COUNT],
) {
    apply_rack_macros_at_step(&mut rack, step, rack_macro_values);
    let (track_pan, track_send, gate_mode) = {
        let tp = &data.state.pattern.track_params[track_idx];
        (
            tp.get_pan(),
            tp.get_send(),
            if tp.is_gate_on() { 1.0 } else { 0.0 },
        )
    };
    let chop = (resolved.chop.round() as u32).max(1);
    let total_gate = (resolved.duration as f64 * samples_per_step) as f32;
    let rack_gate = total_gate / chop as f32;

    let pan_lid = data.state.runtime.pan_lids[track_idx].load(Ordering::Acquire);
    if pan_lid != 0 {
        let effective_pan = (track_pan + resolved.pan).clamp(-1.0, 1.0);
        unsafe {
            crate::audiograph::params_push_wrapper(
                data.lg.0,
                crate::audiograph::ParamMsg {
                    idx: crate::effects::stereo_panner::STEREO_PANNER_PARAM_PAN,
                    logical_id: pan_lid,
                    fvalue: effective_pan,
                },
            );
        }
    }

    let resolved_slot_params: Vec<ResolvedRackSlotParams> = rack
        .slots
        .iter()
        .map(|slot| resolve_rack_slot_params(slot, step))
        .collect();
    let has_solo = resolved_slot_params.iter().any(|params| params.solo);
    for (slot_idx, slot) in rack.slots.iter().enumerate() {
        let Some(slot_params) = resolved_slot_params.get(slot_idx).copied() else {
            continue;
        };
        let muted_by_solo = has_solo && !slot_params.solo;
        let slot_pan_lid =
            data.state.runtime.rack_slot_pan_lids[track_idx][slot_idx].load(Ordering::Acquire);
        unsafe {
            push_rack_slot_panner_params(data.lg.0, slot_pan_lid, slot_params, muted_by_solo);
        }
        if !rack_slot_accepts_resolved(slot_params, has_solo) {
            continue;
        }
        let receives_trigger = if chord.count > 0 {
            (0..chord.count).any(|note_idx| {
                let transpose = resolved_chord_transpose(
                    chord.notes[note_idx],
                    chord.step_transpose,
                    resolved.transpose,
                );
                rack_slot_matches_routing(slot, rack.routing, transpose)
            })
        } else {
            rack_slot_matches_routing(slot, rack.routing, resolved.transpose)
        };
        if !receives_trigger {
            continue;
        }
        unsafe {
            dispatch_snapshot_effect_params_at_step(data.lg.0, &slot.effect_slots, step);
        }
        let instrument_params = resolve_rack_slot_instrument_params(&slot.instrument_slot, step);
        let sampler_params = if slot.instrument_type == InstrumentType::Sampler {
            Some(resolve_rack_slot_sampler_params(
                &slot.instrument_slot,
                step,
            ))
        } else {
            None
        };

        if chord.count > 0 {
            for n in 0..chord.count {
                let note_duration = chord.durations[n].max(0.0);
                let note_total_gate = if note_duration > 0.0 {
                    (note_duration as f64 * samples_per_step) as f32
                } else {
                    total_gate
                };
                let note_gate = note_total_gate / chop as f32;
                let transpose = resolved_chord_transpose(
                    chord.notes[n],
                    chord.step_transpose,
                    resolved.transpose,
                );
                if !rack_slot_matches_routing(slot, rack.routing, transpose) {
                    continue;
                }
                if let Some(choke_group) = slot.choke_group {
                    release_rack_choke_group_voices(
                        data,
                        track_idx,
                        &rack,
                        slot_idx,
                        choke_group,
                        frame_offset,
                    );
                }
                let playback_transpose = rack_slot_playback_transpose(rack.routing, transpose);
                let note_instrument_params = if slot.instrument_type == InstrumentType::Custom {
                    key_locked_snapshot_instrument_params(
                        &slot.instrument_slot,
                        playback_transpose,
                        slot_params.base_note_offset,
                        key_lock_plock_step,
                        &instrument_params,
                    )
                } else {
                    instrument_params.clone()
                };
                let instrument_fingerprint = rack_slot_sound_fingerprint(
                    slot,
                    &note_instrument_params,
                    slot_params.base_note_offset,
                );
                fire_rack_slot_note(
                    data,
                    frame_offset,
                    track_idx,
                    slot_idx,
                    slot,
                    slot_params,
                    playback_transpose,
                    resolved.velocity,
                    resolved.speed,
                    note_gate,
                    gate_mode,
                    &note_instrument_params,
                    sampler_params,
                    instrument_fingerprint,
                );
            }
        } else {
            if !rack_slot_matches_routing(slot, rack.routing, resolved.transpose) {
                continue;
            }
            if let Some(choke_group) = slot.choke_group {
                release_rack_choke_group_voices(
                    data,
                    track_idx,
                    &rack,
                    slot_idx,
                    choke_group,
                    frame_offset,
                );
            }
            let playback_transpose = rack_slot_playback_transpose(rack.routing, resolved.transpose);
            let note_instrument_params = if slot.instrument_type == InstrumentType::Custom {
                key_locked_snapshot_instrument_params(
                    &slot.instrument_slot,
                    playback_transpose,
                    slot_params.base_note_offset,
                    key_lock_plock_step,
                    &instrument_params,
                )
            } else {
                instrument_params.clone()
            };
            let instrument_fingerprint = rack_slot_sound_fingerprint(
                slot,
                &note_instrument_params,
                slot_params.base_note_offset,
            );
            fire_rack_slot_note(
                data,
                frame_offset,
                track_idx,
                slot_idx,
                slot,
                slot_params,
                playback_transpose,
                resolved.velocity,
                resolved.speed,
                rack_gate,
                gate_mode,
                &note_instrument_params,
                sampler_params,
                instrument_fingerprint,
            );
        }
    }

    let send_lid = data.state.runtime.send_lids[track_idx].load(Ordering::Acquire);
    if send_lid != 0 {
        unsafe {
            params_push_wrapper(
                data.lg.0,
                ParamMsg {
                    idx: 0,
                    logical_id: send_lid,
                    fvalue: track_send,
                },
            );
        }
    }
    cancel_chops_for_track(
        &mut data.countdown_events,
        &mut data.block_events,
        track_idx,
    );
    data.state.transport.trigger_flash[track_idx].store(255, Ordering::Relaxed);
}
