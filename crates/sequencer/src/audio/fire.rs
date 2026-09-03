/*!
Note firing: turning a resolved trigger into sound.

`fire_resolved` is the heart of the module (~700 lines): given a resolved
step/keyboard/network trigger it allocates or steals voices, resolves and
pushes parameter bundles, emits note-on graph events for sampler and custom
engines, and registers gate-offs and retrigs. `dispatch_retrig_event` re-fires
voices for retrig/roll playback (see `docs/step-retrig-spec.md`). Rack-specific variants live in `rack`.
*/

#[allow(unused_imports)]
use super::*;

pub(super) fn push_active_keyboard_voice(
    voices: &mut [ActiveKeyboardVoice; MAX_RACK_SLOTS],
    voice_count: &mut usize,
    voice: ActiveKeyboardVoice,
) {
    if *voice_count >= MAX_RACK_SLOTS || voice.logical_id == 0 {
        return;
    }
    voices[*voice_count] = voice;
    *voice_count += 1;
}

/// Fire a resolved step trigger for a track (handles gate, chop setup, envelope params).
/// Uses voice pool allocation for polyphonic playback.
pub(super) fn midi_note_from_transpose(transpose: f32, base_note_offset: f32) -> Option<u8> {
    let note = (60.0 + transpose + base_note_offset).round();
    (0.0..=127.0).contains(&note).then_some(note as u8)
}

pub(super) fn mark_resolved_note_activity(
    data: &AudioCallbackData,
    frame_offset: u32,
    track_idx: usize,
    samples_per_step: f64,
    resolved: crate::accumulator::ResolvedStep,
    chord: crate::scheduled_event::ScheduledChordData,
) {
    let base_note_offset = f32::from_bits(
        data.state.pattern.instrument_base_note_offsets[track_idx].load(Ordering::Relaxed),
    );
    let start_sample = data.rendered_samples.load(Ordering::Acquire) + frame_offset as u64;
    let mark = |transpose: f32, duration_steps: f32| {
        let Some(note) = midi_note_from_transpose(transpose, base_note_offset) else {
            return;
        };
        let gate_samples = (duration_steps.max(0.0) as f64 * samples_per_step.max(0.0))
            .round()
            .max(1.0) as u64;
        data.state.mark_scheduled_note_active_until(
            track_idx,
            note,
            start_sample.saturating_add(gate_samples),
            resolved.velocity,
        );
    };

    if chord.count > 0 {
        for idx in 0..chord.count.min(MAX_VOICES) {
            let duration = if chord.durations[idx] > 0.0 {
                chord.durations[idx]
            } else {
                resolved.duration
            };
            mark(
                crate::scheduled_event::resolved_chord_transpose(
                    chord.notes[idx],
                    chord.step_transpose,
                    resolved.transpose,
                ),
                duration,
            );
        }
    } else {
        mark(resolved.transpose, resolved.duration);
    }
}

pub(super) fn fire_resolved(
    data: &mut AudioCallbackData,
    frame_offset: u32,
    track_idx: usize,
    step: usize,
    key_lock_plock_step: Option<usize>,
    samples_per_step: f64,
    resolved: crate::accumulator::ResolvedStep,
    chord: crate::scheduled_event::ScheduledChordData,
    instrument_params: ScheduledInstrumentParams,
    instrument_tensor_params: ScheduledInstrumentTensorParams,
    instrument_fingerprint: u64,
    scheduled_sampler_params: Option<ScheduledSamplerParams>,
    rack_macro_values: [Option<f32>; crate::sequencer::RACK_MACRO_COUNT],
) {
    if !track_accepts_scheduled_trigger(&data.state, track_idx) {
        return;
    }
    // Drum rack v2 choke: a sequenced pad hit releases the other member tracks
    // in its choke group before its own note goes out. Every sequenced trigger
    // path (block events and countdown events alike) lands here.
    let choke_release_sample = data.rendered_samples.load(Ordering::Acquire) + frame_offset as u64;
    release_rack_choke_group_track_voices(data, track_idx, choke_release_sample, frame_offset);
    let tp = &data.state.pattern.track_params[track_idx];
    let instrument_type = InstrumentType::from_runtime_flag(
        data.state.runtime.instrument_type_flags[track_idx].load(Ordering::Relaxed),
    );
    mark_resolved_note_activity(
        data,
        frame_offset,
        track_idx,
        samples_per_step,
        resolved,
        chord,
    );
    if instrument_type == InstrumentType::Rack {
        let rack = data
            .scheduler_snapshot
            .tracks
            .get(track_idx)
            .and_then(|track| track.rack_track.clone());
        if let Some(rack) = rack {
            fire_rack_resolved(
                data,
                frame_offset,
                track_idx,
                step,
                key_lock_plock_step,
                samples_per_step,
                resolved,
                chord,
                rack,
                rack_macro_values,
            );
        }
        return;
    }
    let is_custom = instrument_type == InstrumentType::Custom;
    let is_modulator = instrument_type == InstrumentType::Modulator;
    let sampler_lid = data.state.runtime.sampler_lids[track_idx].load(Ordering::Acquire);
    if !is_custom && !is_modulator && sampler_lid == 0 {
        return;
    }

    // Machinedrum RTRG/RTIM (docs/step-retrig-spec.md). `Chop` is no longer
    // consulted here: it fought retrig for the same countdown slot and only
    // ever covered samplers. The burst is owned by the voice, so it keeps
    // rolling past the step until the next trig on this track cancels it.
    let retrig_repeats = retrig_repeats_from_resolved(&resolved);
    let retrig_interval_samples = retrig_interval_samples(
        &resolved,
        data.sample_rate,
        data.scheduler_snapshot.transport.bpm as f64,
    );

    let total_gate = (resolved.duration as f64 * samples_per_step) as f32;
    // Each hit is gated to the interval when a repeat is due before the step's
    // own duration runs out, so hits butt together instead of overlapping.
    let hit_gate = retrig_hit_gate(total_gate, retrig_repeats, retrig_interval_samples);

    let fallback_sampler_params = || {
        let inst_slot = &data.state.pattern.instrument_slots[track_idx];
        ScheduledSamplerParams {
            attack_ms: inst_slot
                .plocks
                .get(step, 0)
                .unwrap_or_else(|| inst_slot.defaults.get(0)),
            release_ms: inst_slot
                .plocks
                .get(step, 1)
                .unwrap_or_else(|| inst_slot.defaults.get(1)),
            start_point: inst_slot
                .plocks
                .get(step, 2)
                .unwrap_or_else(|| inst_slot.defaults.get(2)),
            end_point: inst_slot
                .plocks
                .get(step, 3)
                .unwrap_or_else(|| inst_slot.defaults.get(3)),
            instrument_enabled: inst_slot
                .plocks
                .get(step, 4)
                .unwrap_or_else(|| inst_slot.defaults.get(4)),
            reverse: inst_slot
                .plocks
                .get(step, 5)
                .unwrap_or_else(|| inst_slot.defaults.get(5)),
            loop_mode: inst_slot
                .plocks
                .get(step, 6)
                .unwrap_or_else(|| inst_slot.defaults.get(6)),
            loop_xfade_ms: inst_slot
                .plocks
                .get(step, 7)
                .unwrap_or_else(|| inst_slot.defaults.get(7)),
            sr_hz: inst_slot
                .plocks
                .get(step, 8)
                .unwrap_or_else(|| inst_slot.defaults.get(8)),
            warp_enabled: inst_slot
                .plocks
                .get(step, 9)
                .unwrap_or_else(|| inst_slot.defaults.get(9)),
            warp_mode: inst_slot
                .plocks
                .get(step, 10)
                .unwrap_or_else(|| inst_slot.defaults.get(10)),
            sample_bpm: inst_slot
                .plocks
                .get(step, 11)
                .unwrap_or_else(|| inst_slot.defaults.get(11)),
            playback_speed: inst_slot
                .plocks
                .get(step, 12)
                .unwrap_or_else(|| inst_slot.defaults.get(12)),
            scrub: inst_slot
                .plocks
                .get(step, 13)
                .unwrap_or_else(|| inst_slot.defaults.get(13)),
            slice_mode: inst_slot
                .plocks
                .get(step, crate::instruments::sampler::SLOT_PARAM_SLICE_MODE)
                .unwrap_or_else(|| {
                    inst_slot
                        .defaults
                        .get(crate::instruments::sampler::SLOT_PARAM_SLICE_MODE)
                }),
            slice_sensitivity: inst_slot
                .plocks
                .get(
                    step,
                    crate::instruments::sampler::SLOT_PARAM_SLICE_SENSITIVITY,
                )
                .unwrap_or_else(|| {
                    inst_slot
                        .defaults
                        .get(crate::instruments::sampler::SLOT_PARAM_SLICE_SENSITIVITY)
                }),
            slice_base: inst_slot
                .plocks
                .get(step, crate::instruments::sampler::SLOT_PARAM_SLICE_BASE)
                .unwrap_or_else(|| {
                    inst_slot
                        .defaults
                        .get(crate::instruments::sampler::SLOT_PARAM_SLICE_BASE)
                }),
            start_point_locked: inst_slot.plocks.get(step, 2).is_some(),
            end_point_locked: inst_slot.plocks.get(step, 3).is_some(),
            warp_preserve: live_slot_resolved_node_param_value(
                inst_slot,
                step,
                crate::instruments::sampler::PARAM_WARP_PRESERVE,
                crate::instruments::sampler::WARP_PRESERVE_DEFAULT as f32,
            ),
            warp_seg_loop_mode: live_slot_resolved_node_param_value(
                inst_slot,
                step,
                crate::instruments::sampler::PARAM_WARP_SEG_LOOP_MODE,
                crate::instruments::sampler::WARP_SEG_LOOP_MODE_DEFAULT as f32,
            ),
            warp_seg_envelope: live_slot_resolved_node_param_value(
                inst_slot,
                step,
                crate::instruments::sampler::PARAM_WARP_SEG_ENVELOPE,
                crate::instruments::sampler::WARP_SEG_ENVELOPE_DEFAULT,
            ),
        }
    };
    let scheduled_source = scheduled_sampler_params.is_some();
    let sampler_params = scheduled_sampler_params.unwrap_or_else(fallback_sampler_params);
    if crate::instruments::sampler::srange_debug_enabled() {
        eprintln!(
            "[srange] trigger dispatch track={} step={} source={} start={} end={}",
            track_idx,
            step,
            if scheduled_source {
                "scheduled"
            } else {
                "fallback"
            },
            sampler_params.start_point,
            sampler_params.end_point,
        );
    }
    let attack_ms = sampler_params.attack_ms;
    let release_ms = sampler_params.release_ms;
    let attack_samples = attack_ms * data.sample_rate as f32 / 1000.0;
    let release_samples = release_ms * data.sample_rate as f32 / 1000.0;
    let gate_mode = if tp.is_gate_on() { 1.0 } else { 0.0 };
    let track_send = tp.get_send();
    let start_point = sampler_params.start_point;
    let end_point = sampler_params.end_point;
    let instrument_enabled = sampler_params.instrument_enabled;
    let reverse = sampler_params.reverse;
    let loop_mode = sampler_params.loop_mode;
    let loop_xfade_samples = sampler_params.loop_xfade_ms * data.sample_rate as f32 / 1000.0;
    let sr_hz = sampler_params.sr_hz;
    let warp_enabled = sampler_params.warp_enabled;
    let warp_mode = sampler_params.warp_mode;
    let sample_bpm = sampler_params.sample_bpm;
    let playback_speed = sampler_params.playback_speed;
    let scrub = sampler_params.scrub;
    let warp_preserve = sampler_params.warp_preserve;
    let warp_seg_loop_mode = sampler_params.warp_seg_loop_mode;
    let warp_seg_envelope = sampler_params.warp_seg_envelope;
    let (
        warp_enabled,
        warp_mode,
        warp_ratio,
        warp_sample_bpm,
        warp_project_bpm,
        warp_ptr_lo,
        warp_ptr_hi,
    ) = sampler_warp_runtime(&data.state, track_idx, warp_enabled, warp_mode, sample_bpm);
    let velocity = resolved.velocity;
    let base_note_offset = f32::from_bits(
        data.state.pattern.instrument_base_note_offsets[track_idx].load(Ordering::Relaxed),
    );
    let step_transpose = chord.step_transpose;
    let pan_lid = data.state.runtime.pan_lids[track_idx].load(Ordering::Acquire);
    if pan_lid != 0 {
        let effective_pan = (tp.get_pan() + resolved.pan).clamp(-1.0, 1.0);
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

    if is_modulator {
        let lid = data.state.runtime.modulator_lids[track_idx].load(Ordering::Acquire);
        if lid == 0 {
            return;
        }
        let seq = next_block_event_sequence(data);
        unsafe {
            dispatch_modulator_params(data.lg.0, lid, &instrument_params);
            trigger_modulator_pulse(
                data.lg.0,
                lid,
                frame_offset,
                seq,
                hit_gate,
                resolved.velocity,
            );
        }
        arm_step_retrig(
            data,
            track_idx,
            frame_offset,
            step,
            retrig_repeats,
            retrig_interval_samples,
            hit_gate,
            RetrigTarget::Step,
        );
        data.state.transport.trigger_flash[track_idx].store(255, Ordering::Relaxed);
        return;
    }

    // Sync polyphonic setting from track params
    let track_polyphonic = tp.is_polyphonic();
    let track_max_polyphony = tp.get_max_polyphony();
    data.voice_pools[track_idx].polyphonic = track_polyphonic;
    let engine_id = if is_custom {
        track_engine_id(&data.state, track_idx)
    } else {
        None
    };
    let free_patch = is_custom
        && track_custom_run_mode(&data.state, track_idx) == CustomInstrumentRunMode::FreePatch;

    // Custom (dgen) voices a retrig burst must re-excite. Collected as the
    // initial hit allocates them so each repeat re-fires the SAME logical
    // voice with the gate held, rather than stealing a fresh one.
    let mut retrig_custom_voices = [RetrigCustomVoice::default(); MAX_VOICES];
    let mut retrig_custom_count = 0usize;

    // Check chord data: if chord has notes, trigger each note on its own voice
    let mut sampler_voice_fired = false;
    let chord_count = chord.count;
    if chord_count > 0 {
        for n in 0..chord_count {
            let note_duration = chord.durations[n].max(0.0);
            let note_total_gate = if note_duration > 0.0 {
                (note_duration as f64 * samples_per_step) as f32
            } else {
                total_gate
            };
            let note_hit_gate =
                retrig_hit_gate(note_total_gate, retrig_repeats, retrig_interval_samples);
            let transpose =
                resolved_chord_transpose(chord.notes[n], step_transpose, resolved.transpose);
            if is_custom {
                let Some(engine_id) = engine_id else {
                    continue;
                };
                let allocation = if free_patch {
                    let Some(allocation) = data.custom_engine_pools[engine_id]
                        .allocate_free_patch_voice(track_idx, track_idx, transpose)
                    else {
                        continue;
                    };
                    allocation
                } else {
                    data.custom_engine_pools[engine_id].allocate_voice(
                        track_idx,
                        track_idx,
                        transpose,
                        track_polyphonic,
                        track_max_polyphony,
                    )
                };
                let voice_idx = allocation.voice_idx;
                data.custom_engine_pools[engine_id].note_voice_allocated(engine_id, voice_idx);
                let lid = allocation.logical_id;
                let synth_id = data.state.runtime.engine_synth_node_ids[engine_id][voice_idx]
                    .load(Ordering::Relaxed);
                let modulator_id = data.state.runtime.engine_modulator_node_ids[engine_id]
                    [voice_idx]
                    .load(Ordering::Relaxed);
                if lid == 0 || synth_id == 0 || modulator_id == 0 {
                    continue;
                }
                if data.trace_audio {
                    let enabled = data.custom_engine_pools[engine_id].enabled_voice_count;
                    eprintln!(
                        "audio-trace: scheduled custom note-on track={track_idx} engine={engine_id} voice={voice_idx} lid={lid} synth={synth_id} mod={modulator_id} chord_note={n} enabled_voices={enabled} poly={track_polyphonic} stolen={}",
                        allocation.stole_active_voice,
                    );
                    data.trace_render_probe_blocks = data.trace_render_probe_blocks.max(12);
                }
                let pitch_hz = custom_pitch_hz(transpose, base_note_offset);
                let key_locked_params = key_locked_live_instrument_params(
                    &data.state,
                    track_idx,
                    transpose,
                    base_note_offset,
                    key_lock_plock_step,
                    &instrument_params,
                );
                let note_fingerprint = instrument_param_bundle_fingerprint(
                    engine_id,
                    base_note_offset,
                    &key_locked_params,
                    &instrument_tensor_params,
                );
                cancel_gate_off_for_lid(&mut data.countdown_events, &mut data.block_events, lid);
                if allocation.stole_active_voice || !track_polyphonic || free_patch {
                    let off_seq = next_event_sequence_from(&mut data.event_seq);
                    unsafe {
                        send_custom_note_off(data.lg.0, lid, frame_offset, off_seq);
                        route_custom_voice_to_consumer(
                            data.lg.0,
                            &data.state,
                            engine_id,
                            voice_idx,
                            allocation.previous_route,
                            track_idx,
                        );
                        if data.custom_engine_pools[engine_id].voices[voice_idx].fingerprint
                            != note_fingerprint
                        {
                            dispatch_instrument_params_to_voice(
                                data.lg.0,
                                synth_id as u64,
                                modulator_id as u64,
                                &key_locked_params,
                            );
                            dispatch_instrument_tensor_params_to_voice(
                                data.lg.0,
                                synth_id as u64,
                                &instrument_tensor_params,
                            );
                        }
                    }
                } else {
                    unsafe {
                        route_custom_voice_to_consumer(
                            data.lg.0,
                            &data.state,
                            engine_id,
                            voice_idx,
                            allocation.previous_route,
                            track_idx,
                        );
                        if data.custom_engine_pools[engine_id].voices[voice_idx].fingerprint
                            != note_fingerprint
                        {
                            dispatch_instrument_params_to_voice(
                                data.lg.0,
                                synth_id as u64,
                                modulator_id as u64,
                                &key_locked_params,
                            );
                            dispatch_instrument_tensor_params_to_voice(
                                data.lg.0,
                                synth_id as u64,
                                &instrument_tensor_params,
                            );
                        }
                    }
                }
                data.custom_engine_pools[engine_id].voices[voice_idx].fingerprint =
                    note_fingerprint;
                let on_seq = next_event_sequence_from(&mut data.event_seq);
                unsafe {
                    send_custom_trigger(data.lg.0, lid, frame_offset, on_seq, pitch_hz, velocity);
                }
                if retrig_custom_count < MAX_VOICES {
                    retrig_custom_voices[retrig_custom_count] = RetrigCustomVoice {
                        logical_id: lid,
                        pitch_hz,
                        velocity,
                    };
                    retrig_custom_count += 1;
                }
                if gate_mode > 0.5 {
                    schedule_gate_off_event(
                        data,
                        track_idx,
                        lid,
                        frame_offset,
                        note_total_gate as f64,
                        GateOffTarget::Custom {
                            engine_id,
                            free_patch,
                        },
                    );
                }
            } else {
                let selector_transpose = transpose;
                let mut trigger_transpose = transpose;
                let mut trigger_params = sampler_params;
                if resolve_slice(
                    &data.state,
                    track_idx,
                    &mut trigger_params,
                    &mut trigger_transpose,
                ) == SliceTriggerVerdict::Ignore
                {
                    continue;
                }
                // `resolve_slice` consumes the note to pick the slice and zeroes the
                // transpose, so adding the base-note offset unconditionally leaves
                // classic mode untouched and makes `base` the pitch offset that every
                // slice plays at.
                trigger_transpose += base_note_offset;
                let voice = data.voice_pools[track_idx]
                    .allocate_voice_retriggering_same_note(selector_transpose);
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
                            &instrument_params,
                        );
                        send_custom_trigger(
                            data.lg.0,
                            voice.gatepitch_id as u64,
                            frame_offset,
                            gatepitch_seq,
                            custom_pitch_hz(trigger_transpose, 0.0),
                            velocity,
                        );
                    }
                }
                let sampler_seq = next_event_sequence_from(&mut data.event_seq);
                unsafe {
                    dispatch_sampler_extra_params_to_voice(data.lg.0, lid, &instrument_params);
                    send_trigger(
                        data.lg.0,
                        lid,
                        frame_offset,
                        sampler_seq,
                        velocity,
                        resolved.speed * playback_speed,
                        note_hit_gate,
                        attack_samples,
                        release_samples,
                        gate_mode,
                        trigger_transpose,
                        trigger_params.start_point,
                        trigger_params.end_point,
                        instrument_enabled,
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
                        scrub,
                    );
                }
                sampler_voice_fired = true;
                if gate_mode > 0.5 {
                    schedule_gate_off_event(
                        data,
                        track_idx,
                        lid,
                        frame_offset,
                        note_total_gate as f64,
                        GateOffTarget::Sampler { gatepitch_id },
                    );
                }
            }
        }
    } else {
        // Single-note mode: use resolved transpose
        let transpose = resolved.transpose;
        if is_custom {
            let Some(engine_id) = engine_id else {
                return;
            };
            let allocation = if free_patch {
                let Some(allocation) = data.custom_engine_pools[engine_id]
                    .allocate_free_patch_voice(track_idx, track_idx, transpose)
                else {
                    return;
                };
                allocation
            } else {
                data.custom_engine_pools[engine_id].allocate_voice(
                    track_idx,
                    track_idx,
                    transpose,
                    track_polyphonic,
                    track_max_polyphony,
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
            if data.trace_audio {
                let enabled = data.custom_engine_pools[engine_id].enabled_voice_count;
                eprintln!(
                    "audio-trace: scheduled custom note-on track={track_idx} engine={engine_id} voice={voice_idx} lid={lid} synth={synth_id} mod={modulator_id} enabled_voices={enabled} poly={track_polyphonic} stolen={}",
                    allocation.stole_active_voice,
                );
                data.trace_render_probe_blocks = data.trace_render_probe_blocks.max(12);
            }
            let pitch_hz = custom_pitch_hz(transpose, base_note_offset);
            let key_locked_params = key_locked_live_instrument_params(
                &data.state,
                track_idx,
                transpose,
                base_note_offset,
                key_lock_plock_step,
                &instrument_params,
            );
            let note_fingerprint = instrument_param_bundle_fingerprint(
                engine_id,
                base_note_offset,
                &key_locked_params,
                &instrument_tensor_params,
            );
            cancel_gate_off_for_lid(&mut data.countdown_events, &mut data.block_events, lid);
            if allocation.stole_active_voice || !track_polyphonic || free_patch {
                let off_seq = next_event_sequence_from(&mut data.event_seq);
                unsafe {
                    send_custom_note_off(data.lg.0, lid, frame_offset, off_seq);
                    route_custom_voice_to_consumer(
                        data.lg.0,
                        &data.state,
                        engine_id,
                        voice_idx,
                        allocation.previous_route,
                        track_idx,
                    );
                    if data.custom_engine_pools[engine_id].voices[voice_idx].fingerprint
                        != note_fingerprint
                    {
                        dispatch_instrument_params_to_voice(
                            data.lg.0,
                            synth_id as u64,
                            modulator_id as u64,
                            &key_locked_params,
                        );
                        dispatch_instrument_tensor_params_to_voice(
                            data.lg.0,
                            synth_id as u64,
                            &instrument_tensor_params,
                        );
                    }
                }
            } else {
                unsafe {
                    route_custom_voice_to_consumer(
                        data.lg.0,
                        &data.state,
                        engine_id,
                        voice_idx,
                        allocation.previous_route,
                        track_idx,
                    );
                    if data.custom_engine_pools[engine_id].voices[voice_idx].fingerprint
                        != note_fingerprint
                    {
                        dispatch_instrument_params_to_voice(
                            data.lg.0,
                            synth_id as u64,
                            modulator_id as u64,
                            &key_locked_params,
                        );
                        dispatch_instrument_tensor_params_to_voice(
                            data.lg.0,
                            synth_id as u64,
                            &instrument_tensor_params,
                        );
                    }
                }
            }
            data.custom_engine_pools[engine_id].voices[voice_idx].fingerprint = note_fingerprint;
            let on_seq = next_event_sequence_from(&mut data.event_seq);
            unsafe {
                send_custom_trigger(data.lg.0, lid, frame_offset, on_seq, pitch_hz, velocity);
            }
            if retrig_custom_count < MAX_VOICES {
                retrig_custom_voices[retrig_custom_count] = RetrigCustomVoice {
                    logical_id: lid,
                    pitch_hz,
                    velocity,
                };
                retrig_custom_count += 1;
            }
            if gate_mode > 0.5 {
                schedule_gate_off_event(
                    data,
                    track_idx,
                    lid,
                    frame_offset,
                    total_gate as f64,
                    GateOffTarget::Custom {
                        engine_id,
                        free_patch,
                    },
                );
            }
        } else {
            let selector_transpose = transpose;
            let mut trigger_transpose = transpose;
            let mut trigger_params = sampler_params;
            if resolve_slice(
                &data.state,
                track_idx,
                &mut trigger_params,
                &mut trigger_transpose,
            ) == SliceTriggerVerdict::Ignore
            {
                return;
            }
            // `resolve_slice` consumes the note to pick the slice and zeroes the
            // transpose, so adding the base-note offset unconditionally leaves
            // classic mode untouched and makes `base` the pitch offset that every
            // slice plays at.
            trigger_transpose += base_note_offset;
            let voice = data.voice_pools[track_idx]
                .allocate_voice_retriggering_same_note(selector_transpose);
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
                        &instrument_params,
                    );
                    send_custom_trigger(
                        data.lg.0,
                        voice.gatepitch_id as u64,
                        frame_offset,
                        gatepitch_seq,
                        custom_pitch_hz(trigger_transpose, 0.0),
                        velocity,
                    );
                }
            }
            let sampler_seq = next_event_sequence_from(&mut data.event_seq);
            unsafe {
                dispatch_sampler_extra_params_to_voice(data.lg.0, lid, &instrument_params);
                send_trigger(
                    data.lg.0,
                    lid,
                    frame_offset,
                    sampler_seq,
                    velocity,
                    resolved.speed * playback_speed,
                    hit_gate,
                    attack_samples,
                    release_samples,
                    gate_mode,
                    trigger_transpose,
                    trigger_params.start_point,
                    trigger_params.end_point,
                    instrument_enabled,
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
                    scrub,
                );
            }
            sampler_voice_fired = true;
            if gate_mode > 0.5 {
                schedule_gate_off_event(
                    data,
                    track_idx,
                    lid,
                    frame_offset,
                    total_gate as f64,
                    GateOffTarget::Sampler { gatepitch_id },
                );
            }
        }
    }

    if !is_custom && !sampler_voice_fired {
        return;
    }

    // Update send gain (reverb send amount from track-level param)
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

    // Arm the retrig burst. Unlike the retired chop path this runs for custom
    // (dgen) tracks too: `retrig_target` carries the logical voices the initial
    // hit allocated so each repeat re-excites the *same* voice.
    let retrig_target = if let (true, Some(engine_id)) = (is_custom, engine_id) {
        RetrigTarget::Custom {
            voices: retrig_custom_voices,
            count: retrig_custom_count,
            engine_id,
            free_patch,
            gated: gate_mode > 0.5,
        }
    } else if is_custom {
        // No engine resolved: nothing was allocated above, so nothing to re-fire.
        RetrigTarget::Custom {
            voices: retrig_custom_voices,
            count: 0,
            engine_id: 0,
            free_patch,
            gated: gate_mode > 0.5,
        }
    } else {
        RetrigTarget::Step
    };
    let retrig_repeats = armed_retrig_repeats(retrig_repeats, &retrig_target);
    arm_step_retrig(
        data,
        track_idx,
        frame_offset,
        step,
        retrig_repeats,
        retrig_interval_samples,
        hit_gate,
        retrig_target,
    );
}

/// Repeats after the initial hit. `RETRIG_INFINITE` rolls until the next trig
/// on the track cancels the burst.
pub(super) fn retrig_repeats_from_resolved(resolved: &crate::accumulator::ResolvedStep) -> u32 {
    if resolved.retrig >= crate::sequencer::RETRIG_INFINITE {
        u32::MAX
    } else {
        resolved.retrig.max(0.0).round() as u32
    }
}

/// Samples between two retrigs: the rate is retrigs *per beat*, so the interval
/// is tempo-relative exactly like the Machinedrum's RTIM.
pub(super) fn retrig_interval_samples(
    resolved: &crate::accumulator::ResolvedStep,
    sample_rate: f64,
    bpm: f64,
) -> f64 {
    let rate = resolved.retrig_rate as f64;
    if !(rate > 0.0) || !(bpm > 0.0) {
        return f64::INFINITY;
    }
    (sample_rate * 60.0 / bpm) / rate
}

/// Gate for one hit of a burst: the step's own gate, shortened to the retrig
/// interval when a repeat is due before it ends.
pub(super) fn retrig_hit_gate(total_gate: f32, repeats: u32, interval_samples: f64) -> f32 {
    if repeats == 0 || !interval_samples.is_finite() {
        return total_gate;
    }
    total_gate.min(interval_samples as f32)
}

/// Repeats actually armed for a fired step. A custom track that allocated no
/// voice (pool exhausted, missing graph nodes) has nothing to re-excite, so its
/// burst stays cancelled rather than firing into a dead logical id.
pub(super) fn armed_retrig_repeats(repeats: u32, target: &RetrigTarget) -> u32 {
    match target {
        RetrigTarget::Custom { count: 0, .. } => 0,
        _ => repeats,
    }
}

/// Arm (or, with no repeats, clear) the track's retrig burst.
fn arm_step_retrig(
    data: &mut AudioCallbackData,
    track_idx: usize,
    frame_offset: u32,
    step: usize,
    repeats: u32,
    interval_samples: f64,
    gate: f32,
    target: RetrigTarget,
) {
    if repeats == 0 || !interval_samples.is_finite() {
        cancel_retrigs_for_track(
            &mut data.countdown_events,
            &mut data.block_events,
            track_idx,
        );
        return;
    }
    schedule_retrig_events(
        data,
        track_idx,
        frame_offset,
        interval_samples,
        interval_samples,
        repeats,
        step,
        gate,
        target,
    );
}

pub(super) fn dispatch_retrig_event(
    data: &mut AudioCallbackData,
    event: RetrigEvent,
    frame_offset: u32,
) {
    let track_idx = event.track_idx;
    if track_idx >= data.state.active_track_count() {
        return;
    }
    if let RetrigTarget::Custom {
        voices,
        count,
        engine_id,
        free_patch,
        gated,
    } = event.target
    {
        // Custom voices are re-excited in place: gatepitch turns each trigger
        // into a fresh pulse while the gate stays held, so envelopes keyed on
        // `(max gate_rising trigger)` restart and one-shots re-fire.
        for voice in voices.iter().take(count) {
            if voice.logical_id == 0 {
                continue;
            }
            let seq = next_event_sequence_from(&mut data.event_seq);
            unsafe {
                send_custom_trigger(
                    data.lg.0,
                    voice.logical_id,
                    frame_offset,
                    seq,
                    voice.pitch_hz,
                    voice.velocity,
                );
            }
            if gated {
                // A repeat that lands after the step's own gate-off re-opens
                // the gate (the note-on sets gate high in gatepitch), so it
                // must bring its own gate-off or the voice hangs open once the
                // burst ends. `schedule_gate_off_event` also cancels the
                // previous pending gate-off for this lid, which is what makes
                // consecutive hits butt together.
                schedule_gate_off_event(
                    data,
                    track_idx,
                    voice.logical_id,
                    frame_offset,
                    event.gate as f64,
                    GateOffTarget::Custom {
                        engine_id,
                        free_patch,
                    },
                );
            }
        }
        data.state.transport.trigger_flash[track_idx].store(255, Ordering::Relaxed);
        return;
    }
    if InstrumentType::from_runtime_flag(
        data.state.runtime.instrument_type_flags[track_idx].load(Ordering::Relaxed),
    ) == InstrumentType::Modulator
    {
        let lid = data.state.runtime.modulator_lids[track_idx].load(Ordering::Acquire);
        if lid == 0 {
            return;
        }
        let slot = &data.state.pattern.instrument_slots[track_idx];
        let rise = slot
            .plocks
            .get(event.step, 0)
            .unwrap_or_else(|| slot.defaults.get(0));
        let fall = slot
            .plocks
            .get(event.step, 1)
            .unwrap_or_else(|| slot.defaults.get(1));
        let velocity = data.state.pattern.step_data[track_idx].get(event.step, StepParam::Velocity);
        let seq = next_event_sequence_from(&mut data.event_seq);
        unsafe {
            params_push_wrapper(
                data.lg.0,
                ParamMsg {
                    idx: crate::instruments::track_modulator::PARAM_RISE_MS,
                    logical_id: lid,
                    fvalue: rise,
                },
            );
            params_push_wrapper(
                data.lg.0,
                ParamMsg {
                    idx: crate::instruments::track_modulator::PARAM_FALL_MS,
                    logical_id: lid,
                    fvalue: fall,
                },
            );
            trigger_modulator_pulse(data.lg.0, lid, frame_offset, seq, event.gate, velocity);
        }
        data.state.transport.trigger_flash[track_idx].store(255, Ordering::Relaxed);
        return;
    }

    let tp = &data.state.pattern.track_params[track_idx];
    let gate_mode = if tp.is_gate_on() { 1.0 } else { 0.0 };
    let chop_inst_slot = &data.state.pattern.instrument_slots[track_idx];
    let attack_samples = chop_inst_slot
        .plocks
        .get(event.step, 0)
        .unwrap_or_else(|| chop_inst_slot.defaults.get(0))
        * data.sample_rate as f32
        / 1000.0;
    let release_samples = chop_inst_slot
        .plocks
        .get(event.step, 1)
        .unwrap_or_else(|| chop_inst_slot.defaults.get(1))
        * data.sample_rate as f32
        / 1000.0;
    let chop_start = chop_inst_slot
        .plocks
        .get(event.step, 2)
        .unwrap_or_else(|| chop_inst_slot.defaults.get(2));
    let chop_end = chop_inst_slot
        .plocks
        .get(event.step, 3)
        .unwrap_or_else(|| chop_inst_slot.defaults.get(3));
    let chop_reverse = chop_inst_slot
        .plocks
        .get(event.step, 5)
        .unwrap_or_else(|| chop_inst_slot.defaults.get(5));
    let chop_loop_mode = chop_inst_slot
        .plocks
        .get(event.step, 6)
        .unwrap_or_else(|| chop_inst_slot.defaults.get(6));
    let chop_loop_xfade_samples = chop_inst_slot
        .plocks
        .get(event.step, 7)
        .unwrap_or_else(|| chop_inst_slot.defaults.get(7))
        * data.sample_rate as f32
        / 1000.0;
    let chop_sr_hz = chop_inst_slot
        .plocks
        .get(event.step, 8)
        .unwrap_or_else(|| chop_inst_slot.defaults.get(8));
    let chop_warp_enabled = chop_inst_slot
        .plocks
        .get(event.step, 9)
        .unwrap_or_else(|| chop_inst_slot.defaults.get(9));
    let chop_warp_mode = chop_inst_slot
        .plocks
        .get(event.step, 10)
        .unwrap_or_else(|| chop_inst_slot.defaults.get(10));
    let chop_sample_bpm = chop_inst_slot
        .plocks
        .get(event.step, 11)
        .unwrap_or_else(|| chop_inst_slot.defaults.get(11));
    let chop_playback_speed = chop_inst_slot
        .plocks
        .get(event.step, 12)
        .unwrap_or_else(|| chop_inst_slot.defaults.get(12));
    let chop_scrub = chop_inst_slot
        .plocks
        .get(event.step, 13)
        .unwrap_or_else(|| chop_inst_slot.defaults.get(13));
    let chop_warp_preserve = live_slot_resolved_node_param_value(
        chop_inst_slot,
        event.step,
        crate::instruments::sampler::PARAM_WARP_PRESERVE,
        crate::instruments::sampler::WARP_PRESERVE_DEFAULT as f32,
    );
    let chop_warp_seg_loop_mode = live_slot_resolved_node_param_value(
        chop_inst_slot,
        event.step,
        crate::instruments::sampler::PARAM_WARP_SEG_LOOP_MODE,
        crate::instruments::sampler::WARP_SEG_LOOP_MODE_DEFAULT as f32,
    );
    let chop_warp_seg_envelope = live_slot_resolved_node_param_value(
        chop_inst_slot,
        event.step,
        crate::instruments::sampler::PARAM_WARP_SEG_ENVELOPE,
        crate::instruments::sampler::WARP_SEG_ENVELOPE_DEFAULT,
    );
    let (
        chop_warp_enabled,
        chop_warp_mode,
        chop_warp_ratio,
        chop_warp_sample_bpm,
        chop_warp_project_bpm,
        chop_warp_ptr_lo,
        chop_warp_ptr_hi,
    ) = sampler_warp_runtime(
        &data.state,
        track_idx,
        chop_warp_enabled,
        chop_warp_mode,
        chop_sample_bpm,
    );
    let chop_base_note_offset = f32::from_bits(
        data.state.pattern.instrument_base_note_offsets[track_idx].load(Ordering::Relaxed),
    );
    let sd = &data.state.pattern.step_data[track_idx];
    let transpose = sd.get(event.step, StepParam::Transpose);
    let host_value = |param_idx: usize, default: f32| {
        if param_idx >= chop_inst_slot.num_params.load(Ordering::Relaxed) as usize {
            return default;
        }
        chop_inst_slot
            .plocks
            .get(event.step, param_idx)
            .unwrap_or_else(|| chop_inst_slot.defaults.get(param_idx))
    };
    let mut trigger_params = ScheduledSamplerParams {
        start_point: chop_start,
        end_point: chop_end,
        slice_mode: host_value(crate::instruments::sampler::SLOT_PARAM_SLICE_MODE, 0.0),
        slice_sensitivity: host_value(
            crate::instruments::sampler::SLOT_PARAM_SLICE_SENSITIVITY,
            0.5,
        ),
        slice_base: host_value(crate::instruments::sampler::SLOT_PARAM_SLICE_BASE, 0.0),
        start_point_locked: chop_inst_slot.plocks.get(event.step, 2).is_some(),
        end_point_locked: chop_inst_slot.plocks.get(event.step, 3).is_some(),
        ..ScheduledSamplerParams::default()
    };
    let mut trigger_transpose = transpose;
    if resolve_slice(
        &data.state,
        track_idx,
        &mut trigger_params,
        &mut trigger_transpose,
    ) == SliceTriggerVerdict::Ignore
    {
        return;
    }
    // `resolve_slice` consumes the note to pick the slice and zeroes the
    // transpose, so adding the base-note offset unconditionally leaves
    // classic mode untouched and makes `base` the pitch offset that every
    // slice plays at.
    trigger_transpose += chop_base_note_offset;
    let voice = data.voice_pools[track_idx].allocate_voice_retriggering_same_note(transpose);
    let voice_lid = voice.logical_id;
    let sampler_lid = data.state.runtime.sampler_lids[track_idx].load(Ordering::Acquire);
    let lid = if voice_lid != 0 {
        voice_lid
    } else {
        sampler_lid
    };
    if lid == 0 {
        return;
    }
    if voice.modulator_id > 0 {
        let gatepitch_seq = next_event_sequence_from(&mut data.event_seq);
        unsafe {
            dispatch_sampler_modulator_defaults_to_voice(
                data.lg.0,
                &data.state,
                track_idx,
                voice.modulator_id as u64,
            );
            send_custom_trigger(
                data.lg.0,
                voice.gatepitch_id as u64,
                frame_offset,
                gatepitch_seq,
                custom_pitch_hz(trigger_transpose, 0.0),
                sd.get(event.step, StepParam::Velocity),
            );
        }
    }
    let sampler_seq = next_event_sequence_from(&mut data.event_seq);
    unsafe {
        dispatch_sampler_extra_defaults_to_voice(data.lg.0, &data.state, track_idx, lid);
        send_trigger(
            data.lg.0,
            lid,
            frame_offset,
            sampler_seq,
            sd.get(event.step, StepParam::Velocity),
            sd.get(event.step, StepParam::Speed) * chop_playback_speed,
            event.gate,
            attack_samples,
            release_samples,
            gate_mode,
            trigger_transpose,
            trigger_params.start_point,
            trigger_params.end_point,
            chop_inst_slot
                .plocks
                .get(event.step, 4)
                .unwrap_or_else(|| chop_inst_slot.defaults.get(4)),
            chop_reverse,
            chop_loop_mode,
            chop_loop_xfade_samples,
            chop_sr_hz,
            chop_warp_enabled,
            chop_warp_mode,
            chop_warp_ratio,
            chop_warp_sample_bpm,
            chop_warp_project_bpm,
            chop_warp_ptr_lo,
            chop_warp_ptr_hi,
            chop_warp_preserve,
            chop_warp_seg_loop_mode,
            chop_warp_seg_envelope,
            chop_scrub,
        );
    }
    data.state.transport.trigger_flash[track_idx].store(255, Ordering::Relaxed);
}
