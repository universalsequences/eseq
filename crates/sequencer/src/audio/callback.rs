/*!
The CPAL audio callback.

`audio_callback` runs once per output buffer and orchestrates the whole
module: topology resets, snapshot refresh, transport advance, pool and
route sync, live keyboard trigger handling, scheduled/countdown/block event
draining, rendering via `render_chunk`, master-recorder capture, metronome
mix, metering, and CPU-load accounting.
*/

#[allow(unused_imports)]
use super::*;

/// Flush subnormal floats to zero for all math on the calling thread.
///
/// The intrinsics are deprecated because changing MXCSR behind the compiler is
/// formally outside the default floating-point environment; setting FTZ/DAZ on
/// dedicated DSP threads is nonetheless the standard audio-engine practice this
/// codebase relies on, and nothing on these threads depends on subnormal
/// precision (f32 subnormals sit below roughly -750 dBFS).
#[allow(deprecated)]
pub(super) fn enable_flush_denormals_to_zero() {
    #[cfg(target_arch = "x86_64")]
    unsafe {
        use std::arch::x86_64::{_mm_getcsr, _mm_setcsr};
        const MXCSR_FLUSH_TO_ZERO: u32 = 1 << 15;
        const MXCSR_DENORMALS_ARE_ZERO: u32 = 1 << 6;
        _mm_setcsr(_mm_getcsr() | MXCSR_FLUSH_TO_ZERO | MXCSR_DENORMALS_ARE_ZERO);
    }
}

pub(super) fn audio_callback(data: &mut AudioCallbackData, output: &mut [f32]) {
    if !data.callback_thread_initialized {
        data.callback_thread_initialized = true;
        // FTZ/DAZ are per-thread MXCSR state and this thread executes graph
        // nodes, so it needs the same subnormal-flush mode the audiograph
        // workers set in worker_main (see flush_denormals_to_zero there for
        // why silence is otherwise more expensive than sound on x86).
        enable_flush_denormals_to_zero();
        // cpal's ALSA backend spawns the callback thread with default
        // scheduling and no spawn hook, so promote it here on first entry.
        // The callback thread actively helps drain the graph; leaving it
        // SCHED_OTHER while the workers run SCHED_FIFO inverts priorities in
        // the audio path.
        #[cfg(target_os = "linux")]
        {
            unsafe { promote_current_thread_rt() };
            // Direct FIFO promotion is final immediately. An rtkit request is
            // still pending here; its non-RT helper prints the achieved RR (or
            // normal-priority) result after the D-Bus reply instead.
            //
            // Either way the printing happens on the helper thread: formatting
            // the status allocates and `eprintln!` takes the process stderr
            // lock, neither of which belongs on the callback thread. This is a
            // single atomic store.
            let status = unsafe { audiograph::rt_status() };
            if status.callback_reported != 0 {
                super::rtkit::request_status_print();
            }
        }
    }
    let callback_start = Instant::now();
    // Always the graph block size: `FixedOutputBlocks` in stream.rs absorbs the
    // device's actual request, which on ALSA/PipeWire is neither fixed nor
    // hop-compatible (eseq-linux.73).
    let nframes = output.len() / data.num_channels;
    data.current_callback_nframes = nframes;
    data.trace_callback_counter = data.trace_callback_counter.wrapping_add(1);
    // The immutable scheduler snapshot is the commit point for topology.
    // Reading live num_tracks/epochs independently can observe the middle of
    // an add-track publication and clear valid events before the scheduler can
    // possibly have produced replacements.
    //
    // Both halves of this refresh are realtime-safe (bead eseq-sj01): the
    // snapshot arrives through a bounded lock-free ring rather than
    // `latest_scheduler_snapshot()`'s `std::sync::Mutex` (no priority
    // inheritance, so a publish landing here could futex-wait the audio thread
    // behind the UI thread), and the outgoing `Arc` is handed to the reclaimer
    // instead of being dropped here. When this thread held the last reference,
    // that drop freed the whole deep structure — per-step chord `Vec`s,
    // per-step effect p-locks, `String`-bearing effect descriptors, order tens
    // of thousands of frees — inside the block budget.
    data.state.snapshot_handoff().refresh(
        &mut data.scheduler_snapshot,
        &mut data.scheduler_snapshot_version,
    );
    let num_tracks = data.scheduler_snapshot.transport.num_tracks;
    let topology_epoch = data.scheduler_snapshot.transport.topology_epoch;
    if num_tracks != data.last_num_tracks || topology_epoch != data.last_topology_epoch {
        // A non-compacting topology keeps existing track indices stable. Even
        // when one track's instrument changed (and therefore its pattern epoch
        // advanced), callback runtime for every unaffected track remains valid.
        let non_compacting = num_tracks >= data.last_num_tracks;
        if data.trace_audio {
            eprintln!(
                "audio-trace: topology {} tracks {}->{} epoch {}->{} rendered_samples={}",
                if non_compacting { "reconcile" } else { "reset" },
                data.last_num_tracks,
                num_tracks,
                data.last_topology_epoch,
                topology_epoch,
                data.rendered_samples.load(Ordering::Acquire),
            );
            data.trace_render_probe_blocks = data.trace_render_probe_blocks.max(12);
        }
        if non_compacting {
            reconcile_event_compatible_topology(data, num_tracks, topology_epoch);
        } else {
            let deleted_track = (data.last_num_tracks.checked_sub(num_tracks) == Some(1))
                .then(|| data.pending_topology_delete_track.take())
                .flatten();
            reset_audio_runtime_for_track_topology(
                data,
                num_tracks,
                topology_epoch,
                deleted_track,
            );
        }
    }
    if data.state.topology_edit_in_flight() {
        let track = data
            .state
            .transport
            .topology_edit_track
            .load(Ordering::Acquire) as usize;
        if track < MAX_TRACKS {
            data.pending_topology_delete_track = Some(track);
        }
    }
    let block_start_sample = data.rendered_samples.load(Ordering::Acquire);
    let block_end_sample = block_start_sample + nframes as u64;
    let transport_playing = data.state.transport.playing.load(Ordering::Relaxed);
    if transport_playing && !data.transport_was_playing {
        data.transport_beats = 0.0;
        data.metronome = MetronomeState::default();
    }
    if transport_playing {
        // Timestamp at callback entry, before graph work, so the UI can
        // interpolate between blocks rather than reading a stale playhead.
        data.state
            .transport
            .record_clock
            .publish(data.transport_beats, callback_start);
    } else {
        data.transport_beats = 0.0;
        data.metronome = MetronomeState::default();
    }
    data.transport_was_playing = transport_playing;
    let host_transport_clock = compute_host_transport_clock(data, block_start_sample);
    sync_instrument_host_clock_params(data, host_transport_clock);
    sync_effect_modulator_transport_clock_params(data, host_transport_clock);
    sync_dj_mixer_transport_phase(data, block_start_sample);

    // Sync voice pools against current runtime bindings. Project loads can
    // replace tracks in-place, so growth-only sync leaves dead logical IDs.
    for t in 0..num_tracks {
        sync_sampler_voice_pool(&data.state, t, &mut data.voice_pools[t]);

        if let Some(engine_id) = track_engine_id(&data.state, t) {
            sync_custom_engine_pool(
                &data.state,
                engine_id,
                &mut data.custom_engine_pools[engine_id],
            );
        }
    }
    sync_rack_voice_pools(data, num_tracks);
    sync_free_patch_transport_routes(data, num_tracks);

    // Process keyboard triggers
    let mut processed_keyboard_trigger = false;
    while let Ok(kt) = data.keyboard_rx.try_recv() {
        processed_keyboard_trigger = true;
        if kt.track >= num_tracks {
            continue;
        }
        let instrument_type = InstrumentType::from_runtime_flag(
            data.state.runtime.instrument_type_flags[kt.track].load(Ordering::Relaxed),
        );
        let is_custom = instrument_type == InstrumentType::Custom;
        let track_polyphonic = data.state.pattern.track_params[kt.track].is_polyphonic();
        let track_max_polyphony = data.state.pattern.track_params[kt.track].get_max_polyphony();
        data.voice_pools[kt.track].polyphonic = track_polyphonic;
        let base_note_offset = f32::from_bits(
            data.state.pattern.instrument_base_note_offsets[kt.track].load(Ordering::Relaxed),
        );

        if kt.note_off {
            if let Some(active_note) =
                take_active_keyboard_note(&mut data.active_keyboard_notes, kt.track, kt.transpose)
            {
                if live_key_release_cuts_voice(&data.state, kt.track) {
                    release_active_keyboard_note(data, active_note, 0, block_end_sample);
                }
            }
        } else {
            // Note-on: allocate voice and trigger
            enforce_mute_group_for_winning_track(data, kt.track, block_start_sample, 0);
            release_rack_choke_group_track_voices(data, kt.track, block_start_sample, 0);
            let resolved_transpose = resolve_live_keyboard_transpose(
                &data.state,
                data.accumulator_states[kt.track],
                kt.track,
                kt.transpose,
            );
            if instrument_type == InstrumentType::Rack {
                let rack = data
                    .scheduler_snapshot
                    .tracks
                    .get(kt.track)
                    .and_then(|track| track.rack_track.clone());
                if let Some(rack) = rack {
                    if !fire_live_keyboard_rack_note(data, kt.track, &kt, resolved_transpose, rack)
                    {
                        continue;
                    }
                } else {
                    continue;
                }
            } else if is_custom {
                let Some(engine_id) = track_engine_id(&data.state, kt.track) else {
                    continue;
                };
                let free_patch = track_custom_run_mode(&data.state, kt.track)
                    == CustomInstrumentRunMode::FreePatch;
                let allocation = if free_patch {
                    let Some(allocation) = data.custom_engine_pools[engine_id]
                        .allocate_free_patch_voice(kt.track, kt.track, resolved_transpose)
                    else {
                        continue;
                    };
                    allocation
                } else {
                    data.custom_engine_pools[engine_id].allocate_voice(
                        kt.track,
                        kt.track,
                        resolved_transpose,
                        track_polyphonic,
                        track_max_polyphony,
                    )
                };
                let voice_idx = allocation.voice_idx;
                data.custom_engine_pools[engine_id].note_voice_allocated(engine_id, voice_idx);
                let voice_lid = allocation.logical_id;
                let default_params =
                    resolve_snapshot_instrument_defaults(&data.scheduler_snapshot, kt.track);
                let default_tensor_params =
                    resolve_live_instrument_tensor_defaults(&data.state, kt.track);
                let key_locked_params = key_locked_live_instrument_params(
                    &data.state,
                    kt.track,
                    resolved_transpose,
                    base_note_offset,
                    None,
                    &default_params,
                );
                let fingerprint = instrument_param_bundle_fingerprint(
                    engine_id,
                    base_note_offset,
                    &key_locked_params,
                    &default_tensor_params,
                );
                let synth_id = data.state.runtime.engine_synth_node_ids[engine_id][voice_idx]
                    .load(Ordering::Relaxed);
                let modulator_id = data.state.runtime.engine_modulator_node_ids[engine_id]
                    [voice_idx]
                    .load(Ordering::Relaxed);
                if voice_lid == 0 || synth_id == 0 || modulator_id == 0 {
                    continue;
                }
                if data.trace_audio {
                    let enabled = data.custom_engine_pools[engine_id].enabled_voice_count;
                    eprintln!(
                        "audio-trace: keyboard custom note-on track={} engine={engine_id} voice={voice_idx} lid={voice_lid} synth={synth_id} mod={modulator_id} enabled_voices={enabled} poly={track_polyphonic} stolen={}",
                        kt.track, allocation.stole_active_voice,
                    );
                    data.trace_render_probe_blocks = data.trace_render_probe_blocks.max(12);
                }
                let pitch_hz = custom_pitch_hz(resolved_transpose, base_note_offset);
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
                        kt.track,
                    );
                    if data.custom_engine_pools[engine_id].voices[voice_idx].fingerprint
                        != fingerprint
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
                            &default_tensor_params,
                        );
                    }
                }
                data.custom_engine_pools[engine_id].voices[voice_idx].fingerprint = fingerprint;
                if allocation.stole_active_voice || !track_polyphonic || free_patch {
                    let off_seq = next_block_event_sequence(data);
                    unsafe {
                        send_custom_note_off(data.lg.0, voice_lid, 0, off_seq);
                    }
                }
                let on_seq = next_block_event_sequence(data);
                unsafe {
                    send_custom_trigger(data.lg.0, voice_lid, 0, on_seq, pitch_hz, kt.velocity);
                }
                store_active_keyboard_note(
                    &mut data.active_keyboard_notes,
                    kt.track,
                    kt.transpose,
                    midi_note_from_transpose(resolved_transpose, base_note_offset),
                    kt.velocity,
                    &[ActiveKeyboardVoice {
                        logical_id: voice_lid,
                        gatepitch_id: 0,
                        target: ActiveKeyboardVoiceTarget::Custom {
                            engine_id,
                            free_patch,
                        },
                    }],
                );
            } else {
                let tp = &data.state.pattern.track_params[kt.track];
                let Some(kb_inst_slot) = data
                    .scheduler_snapshot
                    .tracks
                    .get(kt.track)
                    .map(|track| &track.instrument_slot)
                else {
                    continue;
                };
                let kb_default =
                    |param_idx: usize| kb_inst_slot.defaults.get(param_idx).copied().unwrap_or(0.0);
                let mut kb_sampler_params = resolve_rack_slot_sampler_defaults(kb_inst_slot);
                let mut trigger_transpose = resolved_transpose;
                if resolve_slice(
                    &data.state,
                    kt.track,
                    &mut kb_sampler_params,
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
                let voice = data.voice_pools[kt.track]
                    .allocate_voice_retriggering_same_note(resolved_transpose);
                let voice_lid = voice.logical_id;
                if voice_lid == 0 {
                    continue;
                }
                let kb_instrument_params =
                    resolve_snapshot_instrument_defaults(&data.scheduler_snapshot, kt.track);
                let attack_samples = kb_default(0) * data.sample_rate as f32 / 1000.0;
                let release_samples = kb_default(1) * data.sample_rate as f32 / 1000.0;
                let gate_mode = if tp.is_gate_on() { 1.0 } else { 0.0 };
                let kb_start = kb_sampler_params.start_point;
                let kb_end = kb_sampler_params.end_point;
                let kb_enabled = kb_default(4);
                let kb_reverse = kb_default(5);
                let kb_loop_mode = kb_default(6);
                let kb_loop_xfade_samples = kb_default(7) * data.sample_rate as f32 / 1000.0;
                let kb_sr_hz = kb_default(8);
                let kb_playback_speed = kb_default(12);
                let kb_warp_preserve = snapshot_slot_default_node_param_value(
                    kb_inst_slot,
                    crate::instruments::sampler::PARAM_WARP_PRESERVE,
                    crate::instruments::sampler::WARP_PRESERVE_DEFAULT as f32,
                );
                let kb_warp_seg_loop_mode = snapshot_slot_default_node_param_value(
                    kb_inst_slot,
                    crate::instruments::sampler::PARAM_WARP_SEG_LOOP_MODE,
                    crate::instruments::sampler::WARP_SEG_LOOP_MODE_DEFAULT as f32,
                );
                let kb_warp_seg_envelope = snapshot_slot_default_node_param_value(
                    kb_inst_slot,
                    crate::instruments::sampler::PARAM_WARP_SEG_ENVELOPE,
                    crate::instruments::sampler::WARP_SEG_ENVELOPE_DEFAULT,
                );
                let (
                    kb_warp_enabled,
                    kb_warp_mode,
                    kb_warp_ratio,
                    kb_warp_sample_bpm,
                    kb_warp_project_bpm,
                    kb_warp_ptr_lo,
                    kb_warp_ptr_hi,
                ) = sampler_warp_runtime(
                    &data.state,
                    kt.track,
                    kb_default(9),
                    kb_default(10),
                    kb_default(11),
                );
                if voice.modulator_id > 0 {
                    let gatepitch_seq = next_event_sequence_from(&mut data.event_seq);
                    unsafe {
                        dispatch_sampler_modulator_params_to_voice(
                            data.lg.0,
                            voice.modulator_id as u64,
                            &kb_instrument_params,
                        );
                        send_custom_trigger(
                            data.lg.0,
                            voice.gatepitch_id as u64,
                            0,
                            gatepitch_seq,
                            custom_pitch_hz(trigger_transpose, 0.0),
                            kt.velocity,
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
                        kt.velocity,
                        kb_playback_speed,
                        attack_samples,
                        release_samples,
                        gate_mode,
                        kb_start,
                        kb_end,
                        kb_enabled,
                        kb_reverse,
                        kb_loop_mode,
                        kb_loop_xfade_samples,
                        kb_sr_hz,
                        kb_warp_enabled,
                        kb_warp_mode,
                        kb_warp_ratio,
                        kb_warp_sample_bpm,
                        kb_warp_project_bpm,
                        kb_warp_ptr_lo,
                        kb_warp_ptr_hi,
                        kb_warp_preserve,
                        kb_warp_seg_loop_mode,
                        kb_warp_seg_envelope,
                        kb_default(13),
                    );
                    dispatch_sampler_extra_params_to_voice(
                        data.lg.0,
                        voice_lid,
                        &kb_instrument_params,
                    );
                }
                store_active_keyboard_note(
                    &mut data.active_keyboard_notes,
                    kt.track,
                    kt.transpose,
                    midi_note_from_transpose(resolved_transpose, base_note_offset),
                    kt.velocity,
                    &[ActiveKeyboardVoice {
                        logical_id: voice_lid,
                        gatepitch_id: voice.gatepitch_id,
                        target: ActiveKeyboardVoiceTarget::Sampler { pool_id: kt.track },
                    }],
                );
            }
            if let Some(note) = midi_note_from_transpose(resolved_transpose, base_note_offset) {
                data.state.mark_live_note_trigger(kt.track, note);
            }
            data.state.transport.trigger_flash[kt.track].store(255, Ordering::Relaxed);
        }
    }
    for track in 0..num_tracks {
        data.state.replace_live_notes(
            track,
            data.active_keyboard_notes[track]
                .iter()
                .filter_map(|note| {
                    note.and_then(|note| {
                        note.midi_note.map(|midi_note| (midi_note, note.velocity))
                    })
                }),
        );
    }
    if processed_keyboard_trigger {
        sync_free_patch_transport_routes(data, num_tracks);
    }

    // Schedule accumulator reset on play-start or pattern change; consumed at next step 0.
    {
        let playing = data.state.transport.playing.load(Ordering::Relaxed);
        let pattern = data.state.current_scene_index() as u32;
        if (!data.last_playing && playing) || data.last_pattern != pattern {
            // Pattern changes and fresh playback should always reapply custom instrument params
            // even if a voice slot is being reused from an older sound state.
            for pool in &mut data.custom_engine_pools {
                pool.invalidate_sound_cache();
            }
            data.pending_accum_reset = [true; MAX_TRACKS];
        }
        if !playing && data.last_playing {
            data.scheduled_events.clear();
            clear_transport_countdown_events(data);
        }
        data.last_playing = playing;
        data.last_pattern = pattern;
    }

    // Push BPM to per-voice modulators when it changes. Track Filter/Delay
    // inserts are descriptor-managed on the control side.
    let bpm = data.state.transport.bpm.load(Ordering::Relaxed);
    if bpm != data.last_bpm {
        data.last_bpm = bpm;
        let bpm_f = bpm as f32;
        for engine in &data.state.runtime.engine_modulator_node_ids {
            for node in engine {
                let logical_id = node.load(Ordering::Relaxed);
                if logical_id != 0 {
                    unsafe {
                        dispatch_voice_modulator_bpm(data.lg.0, logical_id as u64, bpm_f);
                    }
                }
            }
        }
        for pool in &data.voice_pools {
            for voice in pool.voices.iter().take(pool.num_voices) {
                if voice.modulator_id > 0 {
                    unsafe {
                        dispatch_voice_modulator_bpm(data.lg.0, voice.modulator_id as u64, bpm_f);
                    }
                }
                if voice.logical_id != 0 {
                    unsafe {
                        params_push_wrapper(
                            data.lg.0,
                            ParamMsg {
                                idx: PARAM_WARP_PROJECT_BPM,
                                logical_id: voice.logical_id,
                                fvalue: bpm_f,
                            },
                        );
                    }
                }
            }
        }
    }

    let mod_reset_counter = data
        .state
        .transport
        .mod_reset_counter
        .load(Ordering::Relaxed);
    if mod_reset_counter != data.last_mod_reset_counter {
        data.last_mod_reset_counter = mod_reset_counter;
        for engine in &data.state.runtime.engine_modulator_node_ids {
            for node in engine {
                let logical_id = node.load(Ordering::Relaxed);
                if logical_id != 0 {
                    unsafe {
                        params_push_wrapper(
                            data.lg.0,
                            ParamMsg {
                                idx: crate::instruments::voice_modulator::PARAM_RESET_COUNTER as u64,
                                logical_id: logical_id as u64,
                                fvalue: mod_reset_counter as f32,
                            },
                        );
                    }
                }
            }
        }
    }

    let current_pattern_epoch = data.scheduler_snapshot.transport.pattern_epoch;
    collect_due_countdown_events(data, nframes, current_pattern_epoch);
    drain_scheduled_events_for_callback(data, block_start_sample, nframes, current_pattern_epoch);
    dispatch_block_events(data, block_start_sample);

    let custom_release_tail_samples =
        (CUSTOM_ENGINE_RELEASE_TAIL_SECONDS * data.sample_rate).round() as u64;
    for engine_id in 0..data.state.runtime.engine_voice_counts.len() {
        if data.state.runtime.engine_voice_counts[engine_id].load(Ordering::Acquire) == 0 {
            continue;
        }
        let minimum_enabled_voices = usize::from(custom_engine_requires_idle_voice(
            data, engine_id, num_tracks,
        ));
        data.custom_engine_pools[engine_id].shrink_released_voices(
            engine_id,
            block_end_sample,
            custom_release_tail_samples,
            minimum_enabled_voices,
        );
    }

    let probe_render = data.trace_audio && data.trace_render_probe_blocks > 0;
    if probe_render {
        eprintln!(
            "audio-trace: render-start callback={} nframes={nframes} tracks={num_tracks} countdown_len={} rendered_samples={block_start_sample}",
            data.trace_callback_counter,
            data.countdown_events.len(),
        );
    }
    let render_start = Instant::now();
    render_chunk(data, output);
    let render_elapsed = render_start.elapsed();
    if probe_render {
        let (chunk_peak_l, chunk_peak_r) = interleaved_peak(output, data.num_channels);
        eprintln!(
            "audio-trace: render-done callback={} nframes={nframes} elapsed_us={} peak_l={chunk_peak_l:.6} peak_r={chunk_peak_r:.6}",
            data.trace_callback_counter,
            render_elapsed.as_micros(),
        );
        data.trace_render_probe_blocks -= 1;
    }
    if render_elapsed.as_millis() >= 10 {
        eprintln!(
            "audio: slow render_chunk; nframes={nframes} elapsed_ms={} countdown_len={} block_start_sample={block_start_sample}",
            render_elapsed.as_millis(),
            data.countdown_events.len(),
        );
    }
    data.rendered_samples
        .store(block_end_sample, Ordering::Release);
    data.state.set_audio_rendered_sample(block_end_sample);

    data.master_recorder.capture(output);

    preview::mix_preview(
        &mut data.preview,
        output,
        data.num_channels,
        data.sample_rate,
    );

    if transport_playing
        && data
            .state
            .transport
            .metronome_enabled
            .load(Ordering::Relaxed)
    {
        let bpm = data.state.transport.bpm.load(Ordering::Relaxed) as f64;
        mix_metronome(
            &mut data.metronome,
            output,
            data.num_channels,
            data.sample_rate,
            data.transport_beats,
            bpm,
        );
    }
    if transport_playing {
        let bpm = data.state.transport.bpm.load(Ordering::Relaxed) as f64;
        data.transport_beats += nframes as f64 * bpm / (data.sample_rate * 60.0);
    }

    // Scan interleaved output for peak levels
    let (peak_l, peak_r) = interleaved_peak(output, data.num_channels);
    data.state
        .transport
        .peak_l
        .store(peak_l.to_bits(), Ordering::Relaxed);
    data.state
        .transport
        .peak_r
        .store(peak_r.to_bits(), Ordering::Relaxed);

    if data.trace_audio {
        let active_custom_voices: usize = data
            .custom_engine_pools
            .iter()
            .map(|pool| {
                pool.voices
                    .iter()
                    .take(pool.num_voices)
                    .filter(|v| v.active)
                    .count()
            })
            .sum();
        let active_sampler_voices: usize = data
            .voice_pools
            .iter()
            .map(|pool| {
                pool.voices
                    .iter()
                    .take(pool.num_voices)
                    .filter(|v| v.active)
                    .count()
            })
            .sum();
        let active_voices = active_custom_voices + active_sampler_voices;
        if active_voices > 0 && peak_l <= 0.000001 && peak_r <= 0.000001 {
            data.trace_silent_active_callbacks =
                data.trace_silent_active_callbacks.saturating_add(1);
            if data.trace_silent_active_callbacks == 16
                || data.trace_silent_active_callbacks % 128 == 0
            {
                eprintln!(
                    "audio-trace: silent while voices active callbacks={} streak={} tracks={num_tracks} custom_active={active_custom_voices} sampler_active={active_sampler_voices} rendered_samples={} topology_epoch={} playing={} countdown_len={} late_events={} dropped_events={}",
                    data.trace_callback_counter,
                    data.trace_silent_active_callbacks,
                    data.rendered_samples.load(Ordering::Acquire),
                    topology_epoch,
                    data.state.transport.playing.load(Ordering::Relaxed),
                    data.countdown_events.len(),
                    data.late_scheduled_events,
                    data.dropped_scheduled_events,
                );
            }
        } else {
            data.trace_silent_active_callbacks = 0;
        }

        let sample_rate = data.sample_rate.max(1.0) as u64;
        let callbacks_per_second = (sample_rate / nframes.max(1) as u64).max(1);
        if data.trace_callback_counter % callbacks_per_second == 0 {
            eprintln!(
                "audio-trace: heartbeat callbacks={} rendered_samples={} tracks={num_tracks} active_custom={active_custom_voices} active_sampler={active_sampler_voices} peak_l={peak_l:.6} peak_r={peak_r:.6} topology_epoch={} cpu_load_pct={:.1}",
                data.trace_callback_counter,
                data.rendered_samples.load(Ordering::Acquire),
                topology_epoch,
                f32::from_bits(data.state.transport.cpu_load_pct.load(Ordering::Relaxed)),
            );
            let mod_stats = crate::instruments::voice_modulator::take_process_stats();
            if mod_stats.calls > 0 {
                eprintln!(
                    "audio-trace: modulator-stats calls={} rendered={} disabled_custom={} disabled_sampler={} all_slots_off={} unbound_rendered={} rendered_frames={} disabled_frames={} all_slots_off_frames={}",
                    mod_stats.calls,
                    mod_stats.rendered_calls,
                    mod_stats.disabled_custom_skips,
                    mod_stats.disabled_sampler_skips,
                    mod_stats.all_slots_off_calls,
                    mod_stats.unbound_rendered_calls,
                    mod_stats.rendered_frames,
                    mod_stats.disabled_frames,
                    mod_stats.all_slots_off_frames,
                );
                for stats in mod_stats.engines {
                    eprintln!(
                        "audio-trace: modulator-engine engine={} enabled={} calls={} rendered={} disabled={} rendered_frames={} disabled_frames={}",
                        stats.engine_id,
                        stats.enabled_voices,
                        stats.calls,
                        stats.rendered_calls,
                        stats.disabled_skips,
                        stats.rendered_frames,
                        stats.disabled_frames,
                    );
                }
                for stats in mod_stats.sampler_tracks {
                    eprintln!(
                        "audio-trace: modulator-sampler track={} active_mask=0x{:03x} calls={} rendered={} disabled={} rendered_frames={} disabled_frames={}",
                        stats.track_idx,
                        stats.active_mask,
                        stats.calls,
                        stats.rendered_calls,
                        stats.disabled_skips,
                        stats.rendered_frames,
                        stats.disabled_frames,
                    );
                }
            }
        }
    }

    publish_active_voice_counts(data, num_tracks);

    if nframes > 0 {
        let elapsed_secs = callback_start.elapsed().as_secs_f32();
        let block_budget_secs = nframes as f32 / data.sample_rate as f32;
        let raw_load_pct = if block_budget_secs > 0.0 {
            (elapsed_secs / block_budget_secs) * 100.0
        } else {
            0.0
        };
        let prev_load_pct =
            f32::from_bits(data.state.transport.cpu_load_pct.load(Ordering::Relaxed));
        let smoothed_load_pct = if prev_load_pct <= 0.0 {
            raw_load_pct
        } else {
            prev_load_pct * 0.97 + raw_load_pct * 0.03
        };
        data.state
            .transport
            .cpu_load_pct
            .store(smoothed_load_pct.to_bits(), Ordering::Relaxed);
    }
}
