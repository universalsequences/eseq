/*!
Rack-slot processing: routing, macros, choke groups, and rack note firing.

Routes triggers to rack slots, applies rack macro curves and per-slot gain
mutes at a step, collects and dispatches
slot note-offs and choke-group releases, and fires notes into rack slots —
`fire_rack_slot_note`/`fire_rack_resolved` for sequenced triggers and
`fire_live_keyboard_rack_note` for live keyboard input.
*/

#[allow(unused_imports)]
use super::*;

mod updates;
use updates::RackParams;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct ResolvedRackSlotParams {
    pub(super) base_note_offset: f32,
    pub(super) gain: f32,
    pub(super) pan: f32,
    pub(super) max_polyphony: usize,
    pub(super) mute: bool,
    pub(super) solo: bool,
}

pub(super) fn resolve_rack_slot_params(slot: &RackSlotSnapshot, step: usize) -> ResolvedRackSlotParams {
    resolve_rack_slot_params_for_update(slot, Some(step))
}

fn resolve_rack_slot_params_for_update(slot: &RackSlotSnapshot, step: Option<usize>) -> ResolvedRackSlotParams {
    let value = |param: RackSlotParam| param.clamp(
        step.map_or_else(|| slot.param_default(param), |step| slot.param_value_at_step(param, step))
    );
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

/// Resolve every rack macro at `step` and push the mapped values into the
/// rack's slots. Precedence per macro: a held print latch (the knob the
/// performer is turning right now, `print_values`), then a process overlay,
/// then the step's p-lock / live default.
#[cfg(test)]
pub(super) fn apply_rack_macros_at_step(
    rack: &mut RackTrackSnapshot,
    step: usize,
    process_values: [Option<f32>; crate::sequencer::RACK_MACRO_COUNT],
    print_values: [Option<f32>; crate::sequencer::RACK_MACRO_COUNT],
) {
    apply_rack_macros_at_step_masked(
        rack,
        step,
        process_values,
        print_values,
        [true; crate::sequencer::RACK_MACRO_COUNT],
    );
}

/// `apply_rack_macros_at_step` restricted to the macros whose `mask` entry is
/// set. Off-step application uses this so only the macros that are p-locked
/// at the step (or held by a print latch) touch the rack.
#[cfg(test)]
fn apply_rack_macros_at_step_masked(
    rack: &mut RackTrackSnapshot,
    step: usize,
    process_values: [Option<f32>; crate::sequencer::RACK_MACRO_COUNT],
    print_values: [Option<f32>; crate::sequencer::RACK_MACRO_COUNT],
    mask: [bool; crate::sequencer::RACK_MACRO_COUNT],
) {
    let macros = rack.macros.clone();
    for rack_macro in macros {
        let index = rack_macro.id.index();
        if !mask.get(index).copied().unwrap_or(false) {
            continue;
        }
        let normalized = print_values
            .get(index)
            .and_then(|value| *value)
            .or_else(|| process_values.get(index).and_then(|value| *value))
            .unwrap_or_else(|| {
                rack.runtime_macro_value_at(rack_macro.id, step)
                    .unwrap_or_else(|| rack_macro.value_at(step))
            });
        apply_rack_macro_mappings(rack, &rack_macro, normalized, Some(step));
    }
}

/// Live (un-sequenced) rack notes: apply every macro at its current knob
/// position — the runtime default the control thread keeps up to date on
/// each turn, or a held print latch — with no step and so no p-locks. A
/// live note allocates a fresh voice and stamps it in full, so without this
/// the stamp carries pre-knob slot defaults while the voice that was
/// sounding during the turn already received the live push: two voices,
/// two different sounds for one knob position.
#[cfg(test)]
pub(super) fn apply_rack_macros_live(
    rack: &mut RackTrackSnapshot,
    print_values: [Option<f32>; crate::sequencer::RACK_MACRO_COUNT],
) {
    let macros = rack.macros.clone();
    for rack_macro in macros {
        let index = rack_macro.id.index();
        let normalized = print_values
            .get(index)
            .and_then(|value| *value)
            .or_else(|| rack.runtime_macro_default(rack_macro.id))
            .unwrap_or(rack_macro.value)
            .clamp(0.0, 1.0);
        apply_rack_macro_mappings(rack, &rack_macro, normalized, None);
    }
}

/// Push one macro's mapped values into the rack's slots. With a `step`, a
/// target that carries its own p-lock at that step keeps the lock; with
/// `None` (live notes) every target follows the macro.
#[cfg(test)]
fn apply_rack_macro_mappings(
    rack: &mut RackTrackSnapshot,
    rack_macro: &crate::sequencer::RackMacro,
    normalized: f32,
    step: Option<usize>,
) {
    {
        for mapping in rack_macro.mappings.iter().cloned() {
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
                    if step.is_some_and(|step| slot.param_plocks.get(step, target).is_some()) {
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
                    let locked = step.is_some_and(|step| {
                        slot.instrument_slot
                            .plocks
                            .get(step)
                            .and_then(|row| row.get(param_index))
                            .and_then(|value| *value)
                            .is_some()
                    });
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
                    let locked = step.is_some_and(|step| {
                        effect
                            .plocks
                            .get(step)
                            .and_then(|row| row.get(param_index))
                            .and_then(|value| *value)
                            .is_some()
                    });
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
) -> ArrayVec<RackSlotNoteOff, { MAX_VOICES * 2 }> {
    let mut note_offs = ArrayVec::new();
    match slot.instrument_type {
        InstrumentType::Sampler => {
            let Some(pool_id) = rack_slot_pool_index(track_idx, slot_idx) else {
                return note_offs;
            };
            if pool_id >= voice_pools.len() {
                return note_offs;
            }
            let active: ArrayVec<(u64, i32), MAX_VOICES> = voice_pools[pool_id].voices
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
            let lids: ArrayVec<u64, MAX_VOICES> = custom_engine_pools[engine_id].voices
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
        InstrumentType::Empty | InstrumentType::Modulator | InstrumentType::Rack => {}
    }
    for note_off in &note_offs {
        let logical_id = match *note_off {
            RackSlotNoteOff::Custom { logical_id } | RackSlotNoteOff::Sampler { logical_id } => logical_id,
        };
        cancel_retrigs_for_voice(countdown_events, block_events, logical_id);
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
) -> ArrayVec<RackSlotNoteOff, { MAX_RACK_SLOTS * MAX_VOICES * 2 }> {
    let mut note_offs = ArrayVec::new();
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

/// Drum rack v2 choke, the per-member-track counterpart of
/// [`collect_rack_slot_active_voice_releases`] (docs/drum-rack-v2-spec.md,
/// "Trigger routing"). A rack pad is a real track, so choke releases a whole
/// track's sounding voices instead of a slot's. Modulator and v1 rack tracks
/// are skipped, exactly as the per-slot version skips those slot types.
#[allow(clippy::too_many_arguments)]
pub(super) fn collect_track_active_voice_releases(
    state: &SequencerState,
    voice_pools: &mut [VoicePool],
    custom_engine_pools: &mut [CustomEnginePool],
    countdown_events: &mut Vec<CountdownEvent>,
    block_events: &mut Vec<BlockEvent>,
    active_keyboard_notes: &mut [[Option<ActiveKeyboardNote>; MAX_VOICES]],
    track_idx: usize,
    release_sample: u64,
    note_offs: &mut Vec<RackSlotNoteOff>,
) {
    let instrument_type = InstrumentType::from_runtime_flag(
        state.runtime.instrument_type_flags[track_idx].load(Ordering::Relaxed),
    );
    match instrument_type {
        InstrumentType::Sampler => {
            if track_idx >= voice_pools.len() {
                return;
            }
            cancel_retrigs_for_track(countdown_events, block_events, track_idx);
            // The choke recycles this track's voice lids, so any live-key
            // entry here is stale: leaving it would make the eventual key
            // release note-off a lid the sequencer has already re-triggered.
            active_keyboard_notes[track_idx] = [None; MAX_VOICES];
            let num_voices = voice_pools[track_idx].num_voices;
            for voice_idx in 0..num_voices {
                let voice = &voice_pools[track_idx].voices[voice_idx];
                if !voice.active || voice.logical_id == 0 {
                    continue;
                }
                let (lid, gatepitch_id) = (voice.logical_id, voice.gatepitch_id);
                voice_pools[track_idx].release_voice_by_logical_id(lid);
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
            let Some(engine_id) = track_engine_id(state, track_idx) else {
                return;
            };
            if engine_id >= custom_engine_pools.len() {
                return;
            }
            cancel_retrigs_for_track(countdown_events, block_events, track_idx);
            active_keyboard_notes[track_idx] = [None; MAX_VOICES];
            let free_patch =
                track_custom_run_mode(state, track_idx) == CustomInstrumentRunMode::FreePatch;
            let num_voices = custom_engine_pools[engine_id].num_voices;
            for voice_idx in 0..num_voices {
                let voice = &custom_engine_pools[engine_id].voices[voice_idx];
                if !voice.active || voice.assigned_track != Some(track_idx) {
                    continue;
                }
                let lid = voice.logical_id;
                if free_patch {
                    custom_engine_pools[engine_id].release_free_patch_voice_by_logical_id(lid);
                } else {
                    custom_engine_pools[engine_id].release_voice_by_logical_id(lid, release_sample);
                }
                cancel_gate_off_for_lid(countdown_events, block_events, lid);
                note_offs.push(RackSlotNoteOff::Custom { logical_id: lid });
            }
        }
        InstrumentType::Empty | InstrumentType::Modulator | InstrumentType::Rack => {}
    }
}

/// Collects the note-offs for a drum rack v2 choke: the sounding voices of
/// every *other* member track sharing the triggering track's choke group.
/// Membership is not walked here — the control thread flattens the pad map
/// into one key per track (`App::publish_rack_choke_runtime`), so two tracks
/// choke each other iff their keys match and are non-zero. A track that fired
/// at this very sample is left alone, so two pads of one choke group landing
/// on the same frame (closed + open hat on one step) do not cut each other's
/// brand-new voice.
#[allow(clippy::too_many_arguments)]
pub(super) fn collect_rack_choke_group_track_releases(
    state: &SequencerState,
    voice_pools: &mut [VoicePool],
    custom_engine_pools: &mut [CustomEnginePool],
    countdown_events: &mut Vec<CountdownEvent>,
    block_events: &mut Vec<BlockEvent>,
    active_keyboard_notes: &mut [[Option<ActiveKeyboardNote>; MAX_VOICES]],
    last_trigger: &[u64; MAX_TRACKS],
    triggering_track: usize,
    release_sample: u64,
    note_offs: &mut Vec<RackSlotNoteOff>,
) {
    let num_tracks = state.active_track_count().min(MAX_TRACKS);
    if triggering_track >= num_tracks {
        return;
    }
    let key = state.runtime.rack_choke_keys[triggering_track].load(Ordering::Acquire);
    if key == 0 {
        return;
    }
    for track_idx in 0..num_tracks {
        if track_idx == triggering_track || last_trigger[track_idx] == release_sample {
            continue;
        }
        if state.runtime.rack_choke_keys[track_idx].load(Ordering::Acquire) != key {
            continue;
        }
        collect_track_active_voice_releases(
            state,
            voice_pools,
            custom_engine_pools,
            countdown_events,
            block_events,
            active_keyboard_notes,
            track_idx,
            release_sample,
            note_offs,
        );
    }
}

/// Runs the drum rack v2 choke pass for a pad that just triggered, live or
/// sequenced. Real-time safe: the note-off scratch buffer lives on the
/// callback data, so a steady-state block allocates nothing.
pub(super) fn release_rack_choke_group_track_voices(
    data: &mut AudioCallbackData,
    triggering_track: usize,
    release_sample: u64,
    frame_offset: u32,
) {
    if triggering_track >= MAX_TRACKS
        || data.state.runtime.rack_choke_keys[triggering_track].load(Ordering::Acquire) == 0
    {
        return;
    }
    data.rack_choke_last_trigger[triggering_track] = release_sample;
    let mut note_offs = std::mem::take(&mut data.rack_choke_note_offs);
    note_offs.clear();
    collect_rack_choke_group_track_releases(
        &data.state,
        &mut data.voice_pools,
        &mut data.custom_engine_pools,
        &mut data.countdown_events,
        &mut data.block_events,
        &mut data.active_keyboard_notes,
        &data.rack_choke_last_trigger,
        triggering_track,
        release_sample,
        &mut note_offs,
    );
    for note_off in note_offs.iter().copied() {
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
    data.rack_choke_note_offs = note_offs;
}

pub(super) fn dispatch_rack_slot_note_offs(
    data: &mut AudioCallbackData,
    frame_offset: u32,
    note_offs: impl IntoIterator<Item = RackSlotNoteOff>,
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

/// A disabled slot's FX chain must not run on silence, so push every
/// effect's `enabled` node param to 0. The enabled branch of the trigger loop
/// pushes the effect's own stored value back, so re-enabling restores whatever
/// bypass state the user had authored on each effect.
pub(super) unsafe fn push_rack_slot_effects_bypassed(lg: *mut LiveGraph, slot: &RackSlotSnapshot) {
    for (effect, descriptor) in slot.effect_slots.iter().zip(&slot.effect_descriptors) {
        if effect.node_id == 0 {
            continue;
        }
        let Some(param_idx) = descriptor.enabled_param_idx() else {
            continue;
        };
        let Some(idx) = effect.node_param_idx(param_idx) else {
            continue;
        };
        if idx == u32::MAX || idx >= crate::instruments::voice_modulator::MOD_PARAM_BASE {
            continue;
        }
        params_push_wrapper(
            lg,
            ParamMsg {
                idx: idx as u64,
                logical_id: effect.node_id as u64,
                fvalue: 0.0,
            },
        );
    }
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

/// Whether live keys on this rack should run through the per-track mono
/// held-note stack (`AudioCallbackData::mono_held`), like a mono Custom track.
///
/// Rack polyphony is per slot, and the held stack is per track, so the rack
/// only qualifies when every slot is a gated Custom instrument at one voice.
/// A sampler or free-patch slot allocates its own voice per key; the stack's
/// "buried key releases nothing" rule would leave that voice hanging, so any
/// such slot keeps the rack on the plain per-key path.
pub(super) fn rack_live_keys_play_mono(rack: &RackTrackSnapshot) -> bool {
    !rack.slots.is_empty()
        && rack.slots.iter().all(|slot| {
            slot.instrument_type == InstrumentType::Custom
                && slot.instrument_run_mode != CustomInstrumentRunMode::FreePatch
                && slot.max_polyphony <= 1
        })
}

pub(super) fn fire_live_keyboard_rack_note(
    data: &mut AudioCallbackData,
    parent_track_idx: usize,
    trigger: &KeyboardTrigger,
    transpose: f32,
    rack: &RackTrackSnapshot,
) -> bool {
    let print_values = data.state.rack_macro_values_for_track(parent_track_idx);
    let update = RackParams::live(rack, print_values);
    let gate_mode = if data.state.pattern.track_params[parent_track_idx].is_gate_on() {
        1.0
    } else {
        0.0
    };
    let mut active_voices = [ActiveKeyboardVoice::default(); MAX_RACK_SLOTS];
    let mut active_voice_count = 0;
    // The held-note stack already chose which key sounds, so the allocator
    // must not apply priority a second time (it could reject the note).
    let voice_priority = if !rack.slots.is_empty() && rack.slots.iter().enumerate().all(|(idx, slot)| {
        slot.instrument_type == InstrumentType::Custom
            && slot.instrument_run_mode != CustomInstrumentRunMode::FreePatch
            && update.slot_params(idx).max_polyphony <= 1
    }) {
        crate::sequencer::VoicePriority::Last
    } else {
        data.state.pattern.track_params[parent_track_idx].get_voice_priority()
    };

    for (slot_idx, slot) in rack.slots.iter().enumerate() {
        if !slot.enabled {
            continue;
        }
        let slot_params = update.slot_params(slot_idx);
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
        let instrument_params = update.instrument_params(slot_idx);
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
                let mut sampler_params = update.sampler_params(slot_idx);
                let mut trigger_transpose = transpose;
                if resolve_slice(
                    &data.state,
                    pool_id,
                    &mut sampler_params,
                    &mut trigger_transpose,
                ) == SliceTriggerVerdict::Ignore
                {
                    continue;
                }
                // `resolve_slice` consumes the note to pick the slice and zeroes the
                // transpose, so adding the base-note offset unconditionally leaves
                // classic mode untouched and makes `base` the pitch offset that every
                // slice plays at.
                trigger_transpose += slot_params.base_note_offset;
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
                let (voice_lid, gatepitch_id, modulator_id) = {
                    let voice = data.voice_pools[pool_id]
                        .allocate_voice_retriggering_same_note_with_limit(
                            transpose,
                            slot_params.max_polyphony,
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
                                trigger_transpose,
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
                        trigger_transpose,
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
                            transpose,
                        )
                    else {
                        continue;
                    };
                    allocation
                } else {
                    let Some(allocation) = data.custom_engine_pools[engine_id].allocate_voice_with_priority(
                        parent_track_idx,
                        rack_slot_pool_index(parent_track_idx, slot_idx)
                            .expect("validated rack slot must have a route identity"),
                        transpose,
                        slot_params.max_polyphony > 1,
                        slot_params.max_polyphony,
                        voice_priority,
                        trigger.origin(),
                    ) else { continue; };
                    allocation
                };
                let legato = !free_patch && allocation.continues_mono_note(
                    rack_slot_pool_index(parent_track_idx, slot_idx).expect("validated rack route"),
                    slot_params.max_polyphony > 1, slot_params.max_polyphony,
                    data.state.pattern.track_params[parent_track_idx].get_mono_trigger(),
                );
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
                    transpose,
                    slot_params.base_note_offset,
                    None,
                    &instrument_params,
                );
                let instrument_fingerprint = rack_slot_sound_fingerprint(
                    slot,
                    &key_locked_instrument_params,
                    slot_params.base_note_offset,
                );
                let pitch_hz =
                    custom_pitch_hz(transpose, slot_params.base_note_offset);
                cancel_gate_off_for_lid(
                    &mut data.countdown_events,
                    &mut data.block_events,
                    voice_lid,
                );
                data.legato_holds.clear_lid(voice_lid);
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
                    if !legato && (allocation.stole_active_voice || slot_params.max_polyphony <= 1 || free_patch) {
                        let off_seq = next_event_sequence_from(&mut data.event_seq);
                        send_custom_note_off(data.lg.0, voice_lid, 0, off_seq);
                    }
                    let on_seq = next_event_sequence_from(&mut data.event_seq);
                    send_custom_note_on(
                        data.lg.0,
                        voice_lid,
                        0,
                        on_seq,
                        pitch_hz,
                        trigger.velocity,
                        legato,
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
            InstrumentType::Empty | InstrumentType::Modulator | InstrumentType::Rack => {}
        }
    }

    if active_voice_count == 0 {
        return false;
    }
    store_active_keyboard_note(
        &mut data.active_keyboard_notes,
        parent_track_idx,
        trigger.transpose,
        trigger.source,
        midi_note_from_transpose(
            transpose,
            f32::from_bits(
                data.state.pattern.instrument_base_note_offsets[parent_track_idx]
                    .load(Ordering::Relaxed),
            ),
        ),
        trigger.velocity,
        &active_voices[..active_voice_count],
    );
    true
}

/// Captured sampler hit: repeats reuse its voice and resolved slice rather
/// than reading a later step or allocating another layer voice.
#[derive(Clone, Copy, Debug)]
pub(super) struct RackSamplerRetrig {
    pool_id: usize,
    logical_id: u64,
    gatepitch_id: i32,
    modulator_active: bool,
    transpose: f32,
    velocity: f32,
    speed: f32,
    gate_mode: f32,
    params: ScheduledSamplerParams,
}

pub(super) fn dispatch_rack_sampler_retrig(
    data: &mut AudioCallbackData,
    hit: RackSamplerRetrig,
    frame_offset: u32,
    gate_samples: f32,
) {
    // A short gate can have released this pool entry before the next repeat.
    // Reopening the DSP gate must also restore the sampler's activity mask.
    if let Some(pool) = data.voice_pools.get_mut(hit.pool_id) {
        if let Some(voice) = pool.voices[..pool.num_voices].iter_mut()
            .find(|voice| voice.logical_id == hit.logical_id)
        {
            voice.active = true;
        }
    }
    let params = hit.params;
    let (warp_enabled, warp_mode, warp_ratio, warp_sample_bpm,
        warp_project_bpm, warp_ptr_lo, warp_ptr_hi) = rack_sampler_warp_runtime(
            &data.state, params.warp_enabled, params.warp_mode, params.sample_bpm,
        );
    if hit.modulator_active {
        let seq = next_event_sequence_from(&mut data.event_seq);
        unsafe {
            send_custom_trigger(
                data.lg.0, hit.gatepitch_id as u64, frame_offset, seq,
                custom_pitch_hz(hit.transpose, 0.0), hit.velocity,
            );
        }
    }
    let seq = next_event_sequence_from(&mut data.event_seq);
    unsafe {
        send_trigger(
            data.lg.0, hit.logical_id, frame_offset, seq, hit.velocity,
            hit.speed * params.playback_speed, gate_samples,
            params.attack_ms * data.sample_rate as f32 / 1000.0,
            params.release_ms * data.sample_rate as f32 / 1000.0,
            hit.gate_mode, hit.transpose, params.start_point, params.end_point,
            params.instrument_enabled, params.reverse, params.loop_mode,
            params.loop_xfade_ms * data.sample_rate as f32 / 1000.0,
            params.sr_hz, warp_enabled, warp_mode, warp_ratio, warp_sample_bpm,
            warp_project_bpm, warp_ptr_lo, warp_ptr_hi, params.warp_preserve,
            params.warp_seg_loop_mode, params.warp_seg_envelope, params.scrub,
        );
    }
    if hit.gate_mode > 0.5 {
        schedule_gate_off_event(
            data, hit.pool_id, hit.logical_id, frame_offset, gate_samples as f64,
            GateOffTarget::Sampler { gatepitch_id: hit.gatepitch_id },
        );
    }
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
    origin: Option<crate::sequencer::LiveNoteOrigin>,
) -> Option<RetrigTarget> {
    match slot.instrument_type {
        InstrumentType::Sampler => {
            let pool_id = rack_slot_pool_index(parent_track_idx, slot_idx)?;
            if pool_id >= data.voice_pools.len() {
                return None;
            }
            let sampler_lid = data.state.runtime.sampler_lids[pool_id].load(Ordering::Acquire);
            if sampler_lid == 0 {
                return None;
            }
            let mut sampler_params = sampler_params.unwrap_or_default();
            let mut trigger_transpose = transpose;
            if resolve_slice(
                &data.state,
                pool_id,
                &mut sampler_params,
                &mut trigger_transpose,
            ) == SliceTriggerVerdict::Ignore
            {
                return None;
            }
            // `resolve_slice` consumes the note to pick the slice and zeroes the
            // transpose, so adding the base-note offset unconditionally leaves
            // classic mode untouched and makes `base` the pitch offset that every
            // slice plays at.
            trigger_transpose += slot_params.base_note_offset;
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
            let hit = RackSamplerRetrig {
                pool_id,
                logical_id: lid,
                gatepitch_id: voice.gatepitch_id,
                modulator_active: voice.modulator_id > 0,
                transpose: trigger_transpose,
                velocity,
                speed,
                gate_mode,
                params: sampler_params,
            };
            unsafe {
                if hit.modulator_active {
                    dispatch_sampler_modulator_params_to_voice(
                        data.lg.0, voice.modulator_id as u64, instrument_params,
                    );
                }
                dispatch_sampler_extra_params_to_voice(data.lg.0, lid, instrument_params);
            }
            dispatch_rack_sampler_retrig(data, hit, frame_offset, gate_samples);
            return Some(RetrigTarget::RackSampler(hit));
        }
        InstrumentType::Custom => {
            let engine_id = slot.track_sound_state.engine_id?;
            if engine_id >= data.custom_engine_pools.len() {
                return None;
            }
            let free_patch = slot.instrument_run_mode == CustomInstrumentRunMode::FreePatch;
            let allocation = if free_patch {
                let allocation = data.custom_engine_pools[engine_id]
                    .allocate_free_patch_voice(
                        parent_track_idx,
                        rack_slot_pool_index(parent_track_idx, slot_idx)
                            .expect("validated rack slot must have a route identity"),
                        transpose,
                    )?;
                allocation
            } else {
                let Some(allocation) = data.custom_engine_pools[engine_id].allocate_voice_with_priority(
                    parent_track_idx,
                    rack_slot_pool_index(parent_track_idx, slot_idx)
                        .expect("validated rack slot must have a route identity"),
                    transpose,
                    slot_params.max_polyphony > 1,
                    slot_params.max_polyphony,
                    data.state.pattern.track_params[parent_track_idx].get_voice_priority(),
                    origin,
                ) else { return None; };
                allocation
            };
            let legato = !free_patch && allocation.continues_mono_note(
                rack_slot_pool_index(parent_track_idx, slot_idx).expect("validated rack route"),
                slot_params.max_polyphony > 1, slot_params.max_polyphony,
                data.state.pattern.track_params[parent_track_idx].get_mono_trigger(),
            );
            let voice_idx = allocation.voice_idx;
            data.custom_engine_pools[engine_id].note_voice_allocated(engine_id, voice_idx);
            let lid = allocation.logical_id;
            let synth_id = data.state.runtime.engine_synth_node_ids[engine_id][voice_idx]
                .load(Ordering::Relaxed);
            let modulator_id = data.state.runtime.engine_modulator_node_ids[engine_id][voice_idx]
                .load(Ordering::Relaxed);
            if lid == 0 || synth_id == 0 || modulator_id == 0 {
                return None;
            }
            let pitch_hz = custom_pitch_hz(transpose, slot_params.base_note_offset);
            if data.trace_audio {
                eprintln!(
                    "audio-trace: scheduled rack note-on track={parent_track_idx} slot={slot_idx} engine={engine_id} voice={voice_idx} lid={lid} max_poly={} stolen={} legato={legato}",
                    slot_params.max_polyphony, allocation.stole_active_voice,
                );
            }
            cancel_gate_off_for_lid(&mut data.countdown_events, &mut data.block_events, lid);
            unsafe {
                if !legato && (allocation.stole_active_voice || slot_params.max_polyphony <= 1 || free_patch) {
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
                send_custom_note_on(data.lg.0, lid, frame_offset, on_seq, pitch_hz, velocity, legato);
                super::pressure::dispatch_voice_expression(data, engine_id, voice_idx, frame_offset);
            }
            if gate_mode > 0.5 {
                let target = GateOffTarget::Custom { engine_id, free_patch };
                schedule_gate_off_event(
                    data,
                    parent_track_idx,
                    lid,
                    frame_offset,
                    gate_samples as f64,
                    target,
                );
                record_sequenced_legato_note(
                    data, parent_track_idx, lid, target, pitch_hz, velocity, legato,
                    frame_offset, gate_samples as f64,
                );
            } else {
                data.legato_holds.clear_lid(lid);
            }
            let mut voices = [RetrigCustomVoice::default(); MAX_VOICES];
            voices[0] = RetrigCustomVoice { logical_id: lid, pitch_hz, velocity };
            return Some(RetrigTarget::Custom {
                voices, count: 1, engine_id, free_patch, gated: gate_mode > 0.5,
            });
        }
        InstrumentType::Empty | InstrumentType::Modulator | InstrumentType::Rack => {}
    }
    None
}

// ── Off-step p-locks ────────────────────────────────────────────────────────
//
// A regular track applies an inactive step's p-locks to its sounding voices
// (`InstrumentParams` / `EffectParams` events). These are the rack analog,
// driven by one `RackParams` event per locked off step: only what is locked
// at the step — or held by a rack-macro print latch — is pushed, so nothing
// the performer is holding gets reset by an unlocked step.

/// Macros that are live at an off step: p-locked there or print-latched.
#[cfg(test)]
pub(super) fn off_step_macro_mask(
    rack: &RackTrackSnapshot,
    step: usize,
    print_values: &[Option<f32>; crate::sequencer::RACK_MACRO_COUNT],
) -> [bool; crate::sequencer::RACK_MACRO_COUNT] {
    let mut mask = [false; crate::sequencer::RACK_MACRO_COUNT];
    for rack_macro in &rack.macros {
        let index = rack_macro.id.index();
        if index >= mask.len() {
            continue;
        }
        mask[index] = print_values[index].is_some()
            || rack_macro.plocks.get(step).copied().flatten().is_some();
    }
    mask
}

/// Every mapping target driven by a live macro (see `off_step_macro_mask`);
/// the params these name are pushed even without their own p-lock.
#[cfg(test)]
pub(super) fn off_step_macro_targets(
    rack: &RackTrackSnapshot,
    mask: &[bool; crate::sequencer::RACK_MACRO_COUNT],
) -> Vec<crate::sequencer::RackMacroTarget> {
    rack.macros
        .iter()
        .filter(|rack_macro| mask.get(rack_macro.id.index()).copied().unwrap_or(false))
        .flat_map(|rack_macro| rack_macro.mappings.iter().map(|mapping| mapping.target.clone()))
        .collect()
}


/// Whether an off-step event at `step` can change any slot's solo state:
/// some slot carries a Solo (or Mute) p-lock there, or a live macro drives
/// one. `MUTED_BY_SOLO` is a cross-slot value (computed from every slot's
/// solo), so when this is true every slot's panner needs it re-pushed, not
/// only the slots that own a lock.
fn off_step_solo_state_changed<'a>(
    rack: &RackTrackSnapshot,
    step: Option<usize>,
    targets: impl IntoIterator<Item = &'a crate::sequencer::RackMacroTarget>,
) -> bool {
    let solo_idx = RackSlotParam::Solo.index();
    let mute_idx = RackSlotParam::Mute.index();
    rack.slots.iter().any(|slot| {
        step.and_then(|step| slot.param_plocks.rows.get(step)).is_some_and(|row| {
            row.get(solo_idx).copied().flatten().is_some()
                || row.get(mute_idx).copied().flatten().is_some()
        })
    }) || targets.into_iter().any(|target| {
        matches!(
            target,
            crate::sequencer::RackMacroTarget::SlotParam { param, .. }
                if matches!(RackSlotParam::from_name(param), Some(RackSlotParam::Solo | RackSlotParam::Mute))
        )
    })
}



/// The slot instrument params to push at an off step: those with an explicit
/// p-lock at `step` plus those `extra` names (macro-driven). Values are
/// resolved the way a trigger resolves them, so a macro that already wrote
/// the slot's default is picked up.
pub(super) fn resolve_rack_slot_instrument_plocks(
    slot: &EffectSlotSnapshot,
    step: usize,
    extra: impl Fn(usize) -> bool,
) -> ScheduledInstrumentParams {
    resolve_rack_slot_instrument_updates(slot, Some(step), |param| {
        extra(param).then(|| slot.defaults.get(param).copied().unwrap_or(0.0))
    })
}

fn resolve_rack_slot_instrument_updates(
    slot: &EffectSlotSnapshot,
    step: Option<usize>,
    macro_value: impl Fn(usize) -> Option<f32>,
) -> ScheduledInstrumentParams {
    let mut params = ScheduledInstrumentParams::new();
    for param_idx in 0..(slot.num_params as usize).min(MAX_SLOT_PARAMS) {
        let macro_value = macro_value(param_idx);
        if !(step.is_some_and(|step| slot_has_explicit_plock(slot, step, param_idx)) || macro_value.is_some()) {
            continue;
        }
        let Some(raw_idx) = slot.node_param_idx(param_idx) else {
            continue;
        };
        if raw_idx == u32::MAX {
            continue;
        }
        let span = slot
            .param_node_spans
            .get(param_idx)
            .copied()
            .unwrap_or(1)
            .max(1);
        let (target, idx) = if raw_idx >= crate::instruments::voice_modulator::MOD_PARAM_BASE {
            (
                ScheduledInstrumentParamTarget::Modulator,
                (raw_idx - crate::instruments::voice_modulator::MOD_PARAM_BASE) as u64,
            )
        } else {
            (ScheduledInstrumentParamTarget::Synth, raw_idx as u64)
        };
        let value = updates::resolved_device_value(slot, step, param_idx, macro_value);
        if !value.is_finite() {
            continue;
        }
        params.push(ScheduledInstrumentParam {
            target,
            idx,
            span,
            value,
        });
    }
    params
}

/// Push the p-locked (or macro-driven) params of a slot's effect chain at
/// `step` straight to the graph nodes; chains are per-slot, not per-voice,
/// so the values stick until the next lock or trigger stamp.
unsafe fn dispatch_rack_slot_effect_plocks_at_step(
    lg: *mut LiveGraph,
    update: &RackParams<'_>,
    slot_idx: usize,
) {
    let step = update.step;
    for (effect_slot_idx, slot) in update.rack.slots[slot_idx].effect_slots.iter().enumerate() {
        let device = update.effect(slot_idx, effect_slot_idx);
        if slot.node_id == 0 {
            continue;
        }
        for param_idx in 0..(slot.num_params as usize).min(MAX_SLOT_PARAMS) {
            let macro_value = device.macro_value(param_idx);
            if !(step.is_some_and(|step| slot_has_explicit_plock(slot, step, param_idx)) || macro_value.is_some())
            {
                continue;
            }
            let Some(idx) = slot.node_param_idx(param_idx) else {
                continue;
            };
            if idx == u32::MAX || param_idx >= slot.defaults.len() {
                continue;
            }
            let (logical_id, idx) = if idx >= crate::instruments::voice_modulator::MOD_PARAM_BASE {
                if slot.modulator_node_id == 0 {
                    continue;
                }
                (
                    slot.modulator_node_id as u64,
                    (idx - crate::instruments::voice_modulator::MOD_PARAM_BASE) as u64,
                )
            } else {
                (slot.node_id as u64, idx as u64)
            };
            let value = updates::resolved_device_value(slot, step, param_idx, macro_value);
            if !value.is_finite() {
                continue;
            }
            let span = slot
                .param_node_spans
                .get(param_idx)
                .copied()
                .unwrap_or(1)
                .max(1);
            push_param_span(lg, logical_id, idx, span, value);
        }
    }
}

/// Fan `params` out to every voice of a rack slot that is sounding right
/// now — the rack analog of `dispatch_instrument_params_to_active_voices`.
fn dispatch_rack_slot_params_to_active_voices(
    data: &mut AudioCallbackData,
    track_idx: usize,
    slot_idx: usize,
    slot: &RackSlotSnapshot,
    params: &[ScheduledInstrumentParam],
) {
    if params.is_empty() {
        return;
    }
    let Some(route_idx) = rack_slot_pool_index(track_idx, slot_idx) else {
        return;
    };
    match slot.instrument_type {
        InstrumentType::Custom => {
            let Some(engine_id) = slot.track_sound_state.engine_id else {
                return;
            };
            if engine_id >= data.custom_engine_pools.len() {
                return;
            }
            let free_patch = slot.instrument_run_mode == CustomInstrumentRunMode::FreePatch;
            let pool = &mut data.custom_engine_pools[engine_id];
            for voice_idx in 0..pool.num_voices {
                let targets_voice = if free_patch {
                    voice_idx == 0
                } else {
                    pool.voices[voice_idx].active
                        && pool.voices[voice_idx].assigned_route == Some(route_idx)
                };
                if !targets_voice {
                    continue;
                }
                let synth_id = data.state.runtime.engine_synth_node_ids[engine_id][voice_idx]
                    .load(Ordering::Relaxed);
                let modulator_id = data.state.runtime.engine_modulator_node_ids[engine_id]
                    [voice_idx]
                    .load(Ordering::Relaxed);
                if synth_id == 0 {
                    continue;
                }
                unsafe {
                    dispatch_instrument_params_to_voice(
                        data.lg.0,
                        synth_id as u64,
                        modulator_id as u64,
                        params,
                    );
                }
                // The voice now diverges from its last full stamp.
                pool.voices[voice_idx].fingerprint = 0;
            }
        }
        InstrumentType::Sampler => {
            if route_idx >= data.voice_pools.len() {
                return;
            }
            let pool = &data.voice_pools[route_idx];
            for voice in pool.voices[..pool.num_voices]
                .iter()
                .filter(|voice| voice.active && voice.logical_id != 0)
            {
                unsafe {
                    dispatch_sampler_live_params_to_voice(
                        data.lg.0,
                        voice.logical_id,
                        voice.modulator_id as u64,
                        params,
                        data.sample_rate,
                    );
                }
            }
        }
        InstrumentType::Empty | InstrumentType::Modulator | InstrumentType::Rack => {}
    }
}

/// Pending takes have no sequenced boundaries of their own yet. Consume
/// their controller mailbox on the audio thread so held notes respond even
/// on an empty arrangement lane. Live updates carry no pattern step:
/// only the latched macro targets are sent, never source-pattern locks.
pub(super) fn apply_take_rack_macro_updates(data: &mut AudioCallbackData) {
    let mut tracks = data.state.take_rack_macro_override.take_changed_tracks();
    while tracks != 0 {
        let track = tracks.trailing_zeros() as usize;
        tracks &= tracks - 1;
        apply_rack_param_updates(data, track, None);
    }
}

/// Apply an Instrument Rack's p-locks at an inactive `step` to whatever is
/// sounding (`ScheduledEventKind::RackParams`). Macros p-locked at the step
/// or held by the print latch are applied first (they may rewrite slot
/// defaults), then per slot: slot params to the panner, effect p-locks to
/// the slot chain, instrument p-locks to the slot's live voices.
pub(super) fn apply_rack_params_off_step(
    data: &mut AudioCallbackData,
    track_idx: usize,
    step: usize,
) {
    apply_rack_param_updates(data, track_idx, Some(step));
}

fn apply_rack_param_updates(data: &mut AudioCallbackData, track_idx: usize, step: Option<usize>) {
    // Retain the immutable snapshot while mutating callback-owned voice state.
    // Cloning the rack itself copies every p-lock row and tensor on the audio
    // thread, even when this event changes a single scalar parameter.
    let snapshot = Arc::clone(&data.scheduler_snapshot);
    let Some(rack) = snapshot
        .tracks
        .get(track_idx)
        .and_then(|track| track.rack_track.as_ref())
    else {
        return;
    };
    let print_values = data.state.rack_macro_values_for_track(track_idx);
    let update = RackParams::for_update(rack, step, print_values);
    dispatch_rack_param_updates(data, track_idx, &update);
}

#[cfg(test)]
fn apply_rack_macros_for_update(
    rack: &mut RackTrackSnapshot,
    step: Option<usize>,
    print_values: [Option<f32>; crate::sequencer::RACK_MACRO_COUNT],
) -> [bool; crate::sequencer::RACK_MACRO_COUNT] {
    if let Some(step) = step {
        let mask = off_step_macro_mask(rack, step, &print_values);
        apply_rack_macros_at_step_masked(
            rack, step, [None; crate::sequencer::RACK_MACRO_COUNT], print_values, mask,
        );
        mask
    } else {
        let macros = rack.macros.clone();
        for rack_macro in &macros {
            if let Some(value) = print_values[rack_macro.id.index()] {
                apply_rack_macro_mappings(rack, rack_macro, value, None);
            }
        }
        print_values.map(|value| value.is_some())
    }
}

fn dispatch_rack_param_updates(
    data: &mut AudioCallbackData,
    track_idx: usize,
    update: &RackParams<'_>,
) {
    let rack = update.rack;
    let step = update.step;
    let has_solo = (0..rack.slots.len()).any(|slot| update.slot_params(slot).solo);
    let solo_state_changed = off_step_solo_state_changed(rack, step, update.targets());
    for (slot_idx, slot) in rack.slots.iter().enumerate() {
        let slot_param_locked = step
            .and_then(|step| slot.param_plocks.rows.get(step))
            .is_some_and(|row| row.iter().any(Option::is_some));
        {
            let slot_params = update.slot_params(slot_idx);
            let muted_by_solo = has_solo && !slot_params.solo;
            let slot_pan_lid = data.state.runtime.rack_slot_pan_lids[track_idx][slot_idx]
                .load(Ordering::Acquire);
            if slot_param_locked || update.targets_slot_param(slot_idx) {
                unsafe {
                    push_rack_slot_panner_params(
                        data.lg.0,
                        slot_pan_lid,
                        slot_params,
                        muted_by_solo,
                    );
                }
            } else if solo_state_changed && slot_pan_lid != 0 {
                // Another slot's Solo/Mute lock changed the cross-slot solo
                // state. Push only MUTED_BY_SOLO: this slot has no lock of
                // its own, so its gain/pan/mute stay live (not snapshot).
                unsafe {
                    params_push_wrapper(
                        data.lg.0,
                        ParamMsg {
                            idx: crate::effects::stereo_panner::STEREO_PANNER_PARAM_MUTED_BY_SOLO,
                            logical_id: slot_pan_lid,
                            fvalue: if muted_by_solo { 1.0 } else { 0.0 },
                        },
                    );
                }
            }
        }
        if slot.enabled {
            unsafe { dispatch_rack_slot_effect_plocks_at_step(data.lg.0, update, slot_idx); }
        } else {
            unsafe { push_rack_slot_effects_bypassed(data.lg.0, slot) };
        }
        let device = update.instrument(slot_idx);
        let params = resolve_rack_slot_instrument_updates(&slot.instrument_slot, step, |param| device.macro_value(param));
        dispatch_rack_slot_params_to_active_voices(data, track_idx, slot_idx, slot, &params);
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
    rack: &RackTrackSnapshot,
    rack_macro_values: [Option<f32>; crate::sequencer::RACK_MACRO_COUNT],
) {
    let print_values = data.state.rack_macro_values_for_track(track_idx);
    let update = RackParams::at_step(rack, step, rack_macro_values, print_values);
    let (track_pan, track_send, gate_mode) = {
        let tp = &data.state.pattern.track_params[track_idx];
        (
            tp.get_pan(),
            tp.get_send(),
            if tp.is_gate_on() { 1.0 } else { 0.0 },
        )
    };
    cancel_retrigs_for_track(
        &mut data.countdown_events, &mut data.block_events, track_idx,
    );
    let repeats = retrig_repeats_from_resolved(&resolved);
    let interval = retrig_interval_samples(
        &resolved, data.sample_rate, data.scheduler_snapshot.transport.bpm as f64,
    );
    let total_gate = (resolved.duration as f64 * samples_per_step) as f32;
    let rack_gate = retrig_hit_gate(total_gate, repeats, interval);

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

    let has_solo = (0..rack.slots.len()).any(|slot| update.slot_params(slot).solo);
    for (slot_idx, slot) in rack.slots.iter().enumerate() {
        let slot_params = update.slot_params(slot_idx);
        let muted_by_solo = has_solo && !slot_params.solo;
        let slot_pan_lid =
            data.state.runtime.rack_slot_pan_lids[track_idx][slot_idx].load(Ordering::Acquire);
        unsafe {
            push_rack_slot_panner_params(data.lg.0, slot_pan_lid, slot_params, muted_by_solo);
        }
        if !slot.enabled {
            // eseq-bw9v: a disabled slot gets no note, so its engine idles
            // down to zero voices, and its FX chain is bypassed so nothing
            // downstream of the silent instrument burns a block either.
            unsafe { push_rack_slot_effects_bypassed(data.lg.0, slot) };
            continue;
        }
        for (effect_idx, effect) in slot.effect_slots.iter().enumerate() {
            let device = update.effect(slot_idx, effect_idx);
            unsafe { dispatch_snapshot_effect_slot_params_with_values(data.lg.0, effect,
                |param| device.value(param, 0.0)); }
        }
        let instrument_params = update.instrument_params(slot_idx);
        let sampler_params = if slot.instrument_type == InstrumentType::Sampler {
            Some(update.sampler_params(slot_idx))
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
                let note_gate = retrig_hit_gate(note_total_gate, repeats, interval);
                let transpose = resolved_chord_transpose(
                    chord.notes[n],
                    chord.step_transpose,
                    resolved.transpose,
                );
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
                let note_instrument_params = if slot.instrument_type == InstrumentType::Custom {
                    key_locked_snapshot_instrument_params(
                        &slot.instrument_slot,
                        transpose,
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
                let target = fire_rack_slot_note(
                    data,
                    frame_offset,
                    track_idx,
                    slot_idx,
                    slot,
                    slot_params,
                    transpose,
                    resolved.velocity,
                    resolved.speed,
                    note_gate,
                    gate_mode,
                    &note_instrument_params,
                    sampler_params,
                    instrument_fingerprint,
                    chord.live_origins[n],
                );
                arm_rack_voice_retrig(
                    data, track_idx, frame_offset, step, repeats, interval, note_gate, target,
                );
            }
        } else {
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
            let note_instrument_params = if slot.instrument_type == InstrumentType::Custom {
                key_locked_snapshot_instrument_params(
                    &slot.instrument_slot,
                    resolved.transpose,
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
            let target = fire_rack_slot_note(
                data,
                frame_offset,
                track_idx,
                slot_idx,
                slot,
                slot_params,
                resolved.transpose,
                resolved.velocity,
                resolved.speed,
                rack_gate,
                gate_mode,
                &note_instrument_params,
                sampler_params,
                instrument_fingerprint,
                chord.live_origins[0],
            );
            arm_rack_voice_retrig(
                data, track_idx, frame_offset, step, repeats, interval, rack_gate, target,
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
    data.state.transport.trigger_flash[track_idx].store(255, Ordering::Relaxed);
}

/// The parent hit cancels the previous burst once. Each allocated rack voice
/// then owns its repeat cadence and gate; arming a layer must not cancel the
/// repeats already armed for the other layers or chord notes.
fn arm_rack_voice_retrig(
    data: &mut AudioCallbackData,
    track_idx: usize,
    frame_offset: u32,
    step: usize,
    repeats: u32,
    interval: f64,
    gate: f32,
    target: Option<RetrigTarget>,
) {
    let Some(target) = target else { return; };
    match target {
        RetrigTarget::RackSampler(hit) => cancel_retrigs_for_voice(
            &mut data.countdown_events, &mut data.block_events, hit.logical_id,
        ),
        RetrigTarget::Custom { voices, count, .. } => {
            for voice in voices.iter().take(count) {
                cancel_retrigs_for_voice(
                    &mut data.countdown_events, &mut data.block_events, voice.logical_id,
                );
            }
        }
        RetrigTarget::Step { .. } => unreachable!("rack notes carry concrete voice targets"),
    }
    if repeats == 0 || !interval.is_finite() {
        return;
    }
    schedule_countdown_or_block_event(
        data, frame_offset as f64 + interval, interval, repeats,
        data.scheduler_snapshot.transport.pattern_epoch,
        CountdownEventKind::Retrig(RetrigEvent { audition_generation: data.current_audition_generation,
            track_idx, step, gate, target }),
    );
}

/// A choked or reassigned voice must not be resurrected by its old burst.
fn cancel_retrigs_for_voice(
    countdown_events: &mut Vec<CountdownEvent>,
    block_events: &mut Vec<BlockEvent>,
    logical_id: u64,
) {
    let retain = |target: &mut RetrigTarget| match target {
        RetrigTarget::RackSampler(hit) => hit.logical_id != logical_id,
        RetrigTarget::Custom { voices, count, .. } => {
            let mut kept = 0;
            for index in 0..*count {
                if voices[index].logical_id != logical_id {
                    voices[kept] = voices[index];
                    kept += 1;
                }
            }
            *count = kept;
            kept > 0
        }
        RetrigTarget::Step { .. } => true,
    };
    countdown_events.retain_mut(|event| match &mut event.kind {
        CountdownEventKind::Retrig(hit) => retain(&mut hit.target),
        _ => true,
    });
    block_events.retain_mut(|event| match &mut event.kind {
        BlockEventKind::Retrig(hit) => retain(&mut hit.target),
        _ => true,
    });
}

#[cfg(test)]
#[path = "rack_retrig_tests.rs"]
mod retrig_tests;

#[cfg(test)]
mod updates_tests;

#[cfg(test)]
mod off_step_solo_tests {
    use super::*;
    use crate::effects::{EffectDescriptor, EffectSlotSnapshot};
    use crate::sequencer::{
        default_rack_macros, CustomInstrumentRunMode, RackMacroCurve, RackMacroMapping,
        RackMacroTarget, RackSlotParamPlocks, RackSlotSnapshot, RackTrackSnapshot,
        TrackSoundState,
    };

    pub(super) fn slot() -> RackSlotSnapshot {
        RackSlotSnapshot {
            instrument_type: InstrumentType::Sampler,
            instrument_run_mode: CustomInstrumentRunMode::Instrument,
            instrument_base_note_offset: 0.0,
            choke_group: None,
            gain: 1.0,
            pan: 0.0,
            mute: false,
            solo: false,
            enabled: true,
            max_polyphony: 1,
            param_plocks: RackSlotParamPlocks::new(),
            instrument_slot: EffectSlotSnapshot::new_empty(),
            effect_slots: RackSlotSnapshot::empty_effect_slots(),
            effect_descriptors: EffectDescriptor::default_full_chain(),
            custom_effect_names: RackSlotSnapshot::empty_effect_names(),
            track_sound_state: TrackSoundState::default(),
            sample_id: None,
        }
    }

    #[test]
    fn take_macro_monitor_ignores_source_plocks_and_preserves_untouched_envelope() {
        let descriptor = EffectDescriptor::builtin_filter();
        let mut voice = slot();
        voice.instrument_slot = EffectSlotSnapshot::new_default(&descriptor, 46);
        voice.instrument_slot.set_plock(0, 0, 999.0);
        voice.instrument_slot.set_plock(0, 1, 0.0);
        voice.instrument_slot.sync_to_descriptor(&descriptor, 47);
        let mut rack = RackTrackSnapshot::new(vec![voice], default_rack_macros());
        for (id, param) in [(0, "cutoff"), (1, "envelope")] {
            rack.macros[id].value = 0.9;
            rack.macros[id].mappings.push(RackMacroMapping {
                target: RackMacroTarget::SlotInstrumentParam {
                    slot: 0, param: param.to_string(), param_index: id,
                },
                range_min: if id == 0 { 70.0 } else { 0.0 },
                range_max: if id == 0 { 600.0 } else { 8.0 },
                curve: RackMacroCurve::Linear,
            });
        }
        let state = SequencerState::new(2, Vec::new());
        let id = crate::sequencer::RackMacroId::from_index(0).unwrap();
        state.take_rack_macro_override.set(0, id, 0.3);
        state.take_rack_macro_override.set(1, id, 0.7);
        state.rack_macro_print_override.clear(); // unrelated mouse-up
        assert_eq!(state.take_rack_macro_override.take_changed_tracks(), 3);
        assert_eq!(state.take_rack_macro_override.take_changed_tracks(), 0);
        assert_eq!(state.rack_macro_values_for_track(1)[0], Some(0.7));
        let values = state.rack_macro_values_for_track(0);
        let mut onset = rack.clone();
        apply_rack_macros_live(&mut onset, values);
        assert_eq!(onset.slots[0].instrument_slot.defaults[1], 7.2);
        let update = RackParams::for_update(&rack, None, values);
        let device = update.instrument(0);
        let params = resolve_rack_slot_instrument_updates(device.slot, None, |param| device.macro_value(param));
        assert_eq!(params.len(), 1, "no envelope reset or source-pattern lock push");
        assert_eq!(params[0].idx, descriptor.params[0].node_param_idx as u64);
        assert_eq!(params[0].value, onset.slots[0].instrument_slot.defaults[0]);
        state.take_rack_macro_override.clear();
        assert!(state.rack_macro_values_for_track(0).iter().all(Option::is_none));
        assert!(state.rack_macro_values_for_track(1).iter().all(Option::is_none));
    }

    /// `MUTED_BY_SOLO` is cross-slot: a Solo p-lock on slot 1 at an inactive
    /// step must re-push it to slot 0 too, even though slot 0 owns no lock.
    /// A gain lock alone must not (slot 0's live gain/pan would be clobbered
    /// by the snapshot for no reason).
    #[test]
    fn off_step_solo_lock_on_one_slot_repushes_every_slot() {
        let mut rack = RackTrackSnapshot::new(vec![slot(), slot()], default_rack_macros());
        assert!(!off_step_solo_state_changed(&rack, Some(4), &[]));

        rack.slots[1].param_plocks.rows[4][RackSlotParam::Gain.index()] = Some(0.5);
        assert!(!off_step_solo_state_changed(&rack, Some(4), &[]), "gain lock is slot-local");

        rack.slots[1].param_plocks.rows[4][RackSlotParam::Solo.index()] = Some(0.0);
        assert!(off_step_solo_state_changed(&rack, Some(4), &[]), "solo lock (even off) is cross-slot");
        assert!(!off_step_solo_state_changed(&rack, Some(5), &[]));

        rack.slots[1].param_plocks.rows[4][RackSlotParam::Solo.index()] = None;
        rack.slots[1].param_plocks.rows[4][RackSlotParam::Mute.index()] = Some(1.0);
        assert!(off_step_solo_state_changed(&rack, Some(4), &[]), "mute lock is cross-slot too");
        assert!(!off_step_solo_state_changed(&rack, None, &[]), "live updates ignore source locks");

        // A live macro driving a slot's solo counts the same way.
        let mapping = |param: &str| RackMacroMapping {
            target: RackMacroTarget::SlotParam {
                slot: 0,
                param: param.to_string(),
            },
            range_min: 0.0,
            range_max: 1.0,
            curve: RackMacroCurve::Linear,
        };
        let empty = RackTrackSnapshot::new(vec![slot(), slot()], default_rack_macros());
        assert!(!off_step_solo_state_changed(&empty, Some(4), &[mapping("gain").target]));
        assert!(off_step_solo_state_changed(&empty, Some(4), &[mapping("solo").target]));
    }
}
