/*!
Note firing: turning a resolved trigger into sound.

`fire_resolved` is the heart of the module (~700 lines): given a resolved
step/keyboard/network trigger it allocates or steals voices, resolves and
pushes parameter bundles, emits note-on graph events for sampler and custom
engines, and registers gate-offs and chops. `dispatch_chop_event` retriggers
voices for chop/roll playback. Rack-specific variants live in `rack`.
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

    let chop = resolved.chop.round() as u32;
    let chop = chop.max(1);

    let total_gate = (resolved.duration as f64 * samples_per_step) as f32;
    let chop_gate = total_gate / chop as f32;

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
            if scheduled_source { "scheduled" } else { "fallback" },
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
                chop_gate,
                resolved.velocity,
            );
        }
        if chop > 1 {
            schedule_chop_events(
                data,
                track_idx,
                frame_offset,
                chop_gate as f64,
                chop_gate as f64,
                chop - 1,
                step,
                chop_gate,
            );
        } else {
            cancel_chops_for_track(
                &mut data.countdown_events,
                &mut data.block_events,
                track_idx,
            );
        }
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

    // Check chord data: if chord has notes, trigger each note on its own voice
    let chord_count = chord.count;
    if chord_count > 0 {
        for n in 0..chord_count {
            let note_duration = chord.durations[n].max(0.0);
            let note_total_gate = if note_duration > 0.0 {
                (note_duration as f64 * samples_per_step) as f32
            } else {
                total_gate
            };
            let note_chop_gate = note_total_gate / chop as f32;
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
                let voice =
                    data.voice_pools[track_idx].allocate_voice_retriggering_same_note(transpose);
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
                            custom_pitch_hz(transpose + base_note_offset, 0.0),
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
                        note_chop_gate,
                        attack_samples,
                        release_samples,
                        gate_mode,
                        transpose + base_note_offset,
                        start_point,
                        end_point,
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
            let voice =
                data.voice_pools[track_idx].allocate_voice_retriggering_same_note(transpose);
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
                        custom_pitch_hz(transpose + base_note_offset, 0.0),
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
                    chop_gate,
                    attack_samples,
                    release_samples,
                    gate_mode,
                    transpose + base_note_offset,
                    start_point,
                    end_point,
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

    // Setup chop re-triggers (sampler only — custom instruments handle gate duration internally)
    if !is_custom && chop > 1 {
        schedule_chop_events(
            data,
            track_idx,
            frame_offset,
            samples_per_step / chop as f64,
            samples_per_step / chop as f64,
            chop - 1,
            step,
            chop_gate,
        );
    } else {
        cancel_chops_for_track(
            &mut data.countdown_events,
            &mut data.block_events,
            track_idx,
        );
    }
}

pub(super) fn dispatch_chop_event(data: &mut AudioCallbackData, event: ChopEvent, frame_offset: u32) {
    let track_idx = event.track_idx;
    if track_idx >= data.state.active_track_count() {
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
            trigger_modulator_pulse(data.lg.0, lid, frame_offset, seq, event.chop_gate, velocity);
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
                custom_pitch_hz(transpose + chop_base_note_offset, 0.0),
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
            event.chop_gate,
            attack_samples,
            release_samples,
            gate_mode,
            transpose + chop_base_note_offset,
            chop_start,
            chop_end,
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
