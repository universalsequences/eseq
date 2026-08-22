/*!
Scheduler lookahead state and the deterministic production scheduling pass.
*/

#[allow(unused_imports)]
use super::*;

pub(super) struct SchedulerLookaheadState {
    pub(super) clock: SnapshotSequencerClock,
    pub(super) accumulator_states: [AccumulatorRuntimeState; MAX_TRACKS],
    pub(super) pending_accum_reset: [bool; MAX_TRACKS],
    pub(super) midi_fx_quantizer_state: MidiFxQuantizerState,
    pub(super) neural_runtime: NeuralRuntime,
    pub(super) generator_runtime: crate::generator::GeneratorRuntime,
    pub(super) process_runtime: crate::process::ProcessRuntime,
    pub(super) resolved_read_pattern_epoch: Option<u64>,
    pub(super) graph_manifests: Vec<crate::graph::GraphManifest>,
    pub(super) graph_runtimes: Vec<crate::graph::GraphRuntime>,
    pub(super) debug_graph_drive_chunks: u32,
    pub(super) debug_accum_invocations: u64,
    pub(super) quantized_launches: crate::quantized_launch::PendingQuantizedLaunches,
    /// Scheduler-owned song playback cursor (docs/song-mode-spec.md 10.2).
    /// While `Some`, the lookahead pass clamps every chunk to the next song
    /// row boundary and schedules from the row's prebuilt snapshot.
    pub(super) song: Option<crate::sequencer::SongPlaybackRuntime>,
    /// Track-roll held notes (docs/rolling-core-spec.md 3), fed by the
    /// `RollCommand` channel drained in the worker loop.
    pub(super) roll: RollState,
}

impl SchedulerLookaheadState {
    pub(super) fn new(sample_rate: u32) -> Self {
        Self {
            clock: SnapshotSequencerClock::new(sample_rate),
            accumulator_states: [AccumulatorRuntimeState::default(); MAX_TRACKS],
            pending_accum_reset: [false; MAX_TRACKS],
            midi_fx_quantizer_state: MidiFxQuantizerState::default(),
            neural_runtime: NeuralRuntime::default(),
            generator_runtime: crate::generator::GeneratorRuntime::default(),
            process_runtime: crate::process::ProcessRuntime::default(),
            resolved_read_pattern_epoch: None,
            graph_manifests: Vec::new(),
            graph_runtimes: Vec::new(),
            debug_graph_drive_chunks: 0,
            debug_accum_invocations: 0,
            quantized_launches: crate::quantized_launch::PendingQuantizedLaunches::default(),
            song: None,
            roll: RollState::new(),
        }
    }
}

/// Flag accumulator resets for exactly the tracks whose resolved SOURCE
/// changed across a song row boundary. Tracks playing the same source
/// through the boundary keep their accumulator state, so a row split made to
/// edit one track's clip is audibly transparent to every other track.
/// Source identity is take-aware (takes spec 7.3): a take lane's identity is
/// its `TakeId`, so the synthetic rows a chunk boundary introduces are NOT a
/// source change — no reset, a take is one continuous clip. Existing pending
/// flags are preserved (marking is additive).
pub(super) fn mark_song_row_accum_resets(
    prev: &crate::sequencer::RuntimeSongRow,
    next: &crate::sequencer::RuntimeSongRow,
    resets: &mut [bool; MAX_TRACKS],
) {
    for (track, reset) in resets.iter_mut().enumerate() {
        if prev.resolved_sources.get(track) != next.resolved_sources.get(track) {
            *reset = true;
        }
    }
}

pub(super) fn build_scheduler_scratch_runtime(
    state: Arc<SequencerState>,
    user_source: &str,
    debug_accum: bool,
) -> Option<lisp_host::ScratchControlRuntime> {
    let midi_fx_source = lisp_host::load_midi_fx_library_source();
    let process_source = lisp_host::load_process_library_source();
    if midi_fx_source.trim().is_empty()
        && process_source.trim().is_empty()
        && user_source.trim().is_empty()
    {
        return None;
    }

    let mut runtime = lisp_host::scheduler_scratch_runtime_with_fallbacks(state, 0, 0);
    let mut keep_runtime = false;
    if !midi_fx_source.trim().is_empty() {
        match runtime.eval(&midi_fx_source) {
            Ok(_) => {
                keep_runtime = true;
                if debug_accum || debug_routing_enabled() {
                    eprintln!(
                        "[scheduler-runtime] builtin midi-fx eval ok midi_fx={:?}",
                        runtime.midi_fx_names()
                    );
                }
            }
            Err(err) => {
                if debug_accum || debug_routing_enabled() {
                    let status = runtime.take_status_message();
                    eprintln!(
                        "[scheduler-runtime] builtin midi-fx eval err={} status={:?}",
                        err, status
                    );
                }
            }
        }
    }

    if !process_source.trim().is_empty() {
        match runtime.eval(&process_source) {
            Ok(_) => {
                keep_runtime = true;
                if debug_accum || debug_routing_enabled() {
                    let names = runtime
                        .process_authoring_snapshot()
                        .defs
                        .iter()
                        .map(|def| def.name.clone())
                        .collect::<Vec<_>>();
                    eprintln!("[scheduler-runtime] builtin process eval ok processes={names:?}");
                }
            }
            Err(err) => {
                if debug_accum || debug_routing_enabled() {
                    let status = runtime.take_status_message();
                    eprintln!(
                        "[scheduler-runtime] builtin process eval err={} status={:?}",
                        err, status
                    );
                }
            }
        }
    }

    if !user_source.trim().is_empty() {
        match runtime.eval_source_at_path(crate::paths::project_scratch_source_path(), user_source)
        {
            Ok(_) => {
                keep_runtime = true;
                if debug_accum {
                    let status = runtime.take_status_message();
                    eprintln!(
                        "[accum] scratch eval ok names={:?} midi_fx={:?} status={:?}",
                        runtime.accumulator_names(),
                        runtime.midi_fx_names(),
                        status
                    );
                }
            }
            Err(err) => {
                if debug_accum || debug_routing_enabled() {
                    let status = runtime.take_status_message();
                    eprintln!(
                        "[accum] scratch eval err={} status={:?}; keeping runtime with midi_fx={:?}",
                        err,
                        status,
                        runtime.midi_fx_names()
                    );
                }
            }
        }
    }

    keep_runtime.then_some(runtime)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct SchedulerLookaheadResult {
    pub(super) scheduled_until_sample: u64,
}

/// Live step-param printing (bead eseq-jc9): chord-backed steps own their
/// sounding durations per note (`chord.durations[idx] > 0.0` beats
/// `resolved.duration` at fire time), and the write-behind stamp moves them
/// by the base-param delta (`set_step_param_no_publish`). The audible
/// substitution has to carry the same per-note delta onto the scheduled
/// chord, or a duration print on a chord-backed step is only heard one loop
/// later.
fn shift_chord_durations_for_print(chord: &mut ScheduledChordData, delta: Option<f32>) {
    let Some(delta) = delta else {
        return;
    };
    for idx in 0..chord.count {
        if chord.durations[idx] > 0.0 {
            chord.durations[idx] = (chord.durations[idx] + delta)
                .clamp(StepParam::Duration.min(), StepParam::Duration.max());
        }
    }
}

pub(super) fn schedule_playing_lookahead<const QUEUE_CAP: usize>(
    scheduler: &mut SchedulerLookaheadState,
    state: &Arc<SequencerState>,
    base_snapshot: &SequencerSnapshot,
    queue: &ScheduledEventQueue<QUEUE_CAP>,
    scratch_runtime: &mut Option<lisp_host::ScratchControlRuntime>,
    live_midi_fx_tracks: &[LiveMidiFxTrackState; MAX_TRACKS],
    pattern_epoch: u64,
    rendered: u64,
    lookahead_target_samples: u64,
    sample_rate: u32,
    scheduler_block_size: usize,
    samples_per_quarter: f64,
    mut scheduled_until_sample: u64,
    debug_accum: bool,
    debug_graph: bool,
) -> SchedulerLookaheadResult {
    let clock = &mut scheduler.clock;
    let accumulator_states = &mut scheduler.accumulator_states;
    let pending_accum_reset = &mut scheduler.pending_accum_reset;
    let midi_fx_quantizer_state = &mut scheduler.midi_fx_quantizer_state;
    let neural_runtime = &mut scheduler.neural_runtime;
    let generator_runtime = &mut scheduler.generator_runtime;
    let process_runtime = &mut scheduler.process_runtime;
    let graph_manifests = &mut scheduler.graph_manifests;
    let graph_runtimes = &mut scheduler.graph_runtimes;
    let session_launches = &mut scheduler.quantized_launches;
    let mut debug_graph_drive_chunks = scheduler.debug_graph_drive_chunks;
    let mut debug_accum_invocations = scheduler.debug_accum_invocations;
    let song_playback = &mut scheduler.song;
    let mut track_output_events = Vec::new();

    process_runtime.sync_step_process_aliases(
        base_snapshot
            .tracks
            .iter()
            .enumerate()
            .map(|(track, snapshot)| (track, &snapshot.process_chain)),
    );

    let resolved_read_bases = vec![
        std::array::from_fn(|index| StepParam::ALL[index].default_value());
        base_snapshot.tracks.len()
    ];
    if scheduler.resolved_read_pattern_epoch != Some(pattern_epoch) {
        process_runtime.reset_resolved_track_history(&resolved_read_bases);
        for graph in graph_runtimes.iter_mut() {
            graph.clear_deltas();
        }
        scheduler.resolved_read_pattern_epoch = Some(pattern_epoch);
    } else {
        process_runtime.ensure_resolved_track_bases(&resolved_read_bases);
    }

    let midi_fx_descriptors_for_scheduling = scratch_runtime
        .as_ref()
        .map(|runtime| runtime.midi_fx_descriptors())
        .unwrap_or_default();

    while scheduled_until_sample < rendered.saturating_add(lookahead_target_samples) {
        // Song playback: clamp this chunk to the next row boundary and
        // schedule it from the current row's prebuilt snapshot. A boundary
        // inside a block therefore splits scheduling exactly at its sample:
        // events strictly before it come from the old row, events at/after
        // from the new row (docs/song-mode-spec.md 10.2). The snapshot switch
        // is an `Arc` handoff prepared at preflight — no mutexes, no pattern
        // cloning, no asset loading on this path (spec 9).
        let mut chunk_frames = scheduler_block_size;
        let mut song_row_snapshot: Option<Arc<SequencerSnapshot>> = None;
        if song_playback.is_none() {
            // A song that just stopped must not leave stale per-lane phase
            // anchors behind: session playback free-runs (anchor 0/0).
            clock.clear_track_anchors();
            // Session-mode quantized launches: clamp the chunk to the next
            // pending boundary and switch the chunk snapshot exactly at the
            // boundary sample — the song-row chunk split (docs/song-mode-spec
            // 10.2) applied to manual launches. No queue clear, no epoch
            // bump, no clock seek: the boundary step's triggers come from
            // the launched snapshot at the exact boundary sample.
            let (frames, install) = session_launches.next_session_chunk(
                clock.total_beats,
                samples_per_quarter,
                scheduler_block_size,
            );
            chunk_frames = frames;
            match install {
                crate::quantized_launch::SessionLaunchInstall::None => {}
                crate::quantized_launch::SessionLaunchInstall::AllTracks => {
                    // Scene launches restart accumulator evolution on every
                    // track, matching the control-side launch path.
                    *pending_accum_reset = [true; MAX_TRACKS];
                    for graph in graph_runtimes.iter_mut() {
                        graph.clear_deltas();
                    }
                }
                crate::quantized_launch::SessionLaunchInstall::Tracks(tracks) => {
                    for track in tracks {
                        if track < MAX_TRACKS {
                            pending_accum_reset[track] = true;
                        }
                    }
                }
            }
        }
        // While a boundary launch awaits its control-side mirror, chunks
        // schedule from its snapshot override (full scene) or a base+mask
        // merge (track launches).
        let session_launch_snapshot: Option<Arc<SequencerSnapshot>> = if song_playback.is_none() {
            session_launches.session_snapshot(base_snapshot)
        } else {
            None
        };
        if let Some(song) = song_playback.as_mut() {
            let prev_row = song.current_row();
            match song.next_chunk(
                scheduled_until_sample,
                clock.total_beats,
                scheduler_block_size,
                state.song_playback(),
            ) {
                crate::sequencer::SongChunkPlan::Ended => break,
                crate::sequencer::SongChunkPlan::Schedule {
                    frames,
                    row,
                    row_changed,
                    wrapped,
                } => {
                    chunk_frames = frames;
                    song_row_snapshot = Some(song.row_snapshot(row));
                    if row_changed {
                        for graph in graph_runtimes.iter_mut() {
                            graph.clear_deltas();
                        }
                        if wrapped {
                            // A loop wrap rewinds the clock and every
                            // self-clocked runtime below; accumulators restart
                            // with them on every track.
                            *pending_accum_reset = [true; MAX_TRACKS];
                        } else {
                            // Diff-aware row transition: a row split made to
                            // edit one track's clip must not restart the
                            // accumulator evolution of tracks whose resolved
                            // pattern is unchanged across the boundary.
                            let (previous, current) =
                                song.transition_rows(prev_row, row);
                            mark_song_row_accum_resets(
                                previous,
                                current,
                                pending_accum_reset,
                            );
                            // A source change is also a fresh TRIGGER
                            // domain: drop the step-dedup memory so the new
                            // clip's first step fires even when the
                            // previous row's clock wrapped into the same
                            // step index just before the boundary (silenced
                            // lanes update the memory too; fractional
                            // captured row starts make that wrap common).
                            // Latched lanes free-run and keep their memory.
                            let latch = state.song_manual_latch_mask();
                            for track in 0..MAX_TRACKS.min(64) {
                                if latch >> track & 1 == 1 {
                                    continue;
                                }
                                if previous.resolved_sources.get(track)
                                    != current.resolved_sources.get(track)
                                {
                                    clock.reset_track_step_memory(track);
                                }
                            }
                        }
                    }
                    if wrapped {
                        // Loop wrap: song beat zero again. Rewind the clock
                        // and every self-clocked runtime so row zero replays
                        // from its start without stale state; the wrap chunk
                        // begins exactly at the end-beat sample, so the edge
                        // trigger fires exactly once.
                        clock.reset();
                        midi_fx_quantizer_state.reset();
                        neural_runtime.reset_state(0.0);
                        generator_runtime.reset(0.0);
                        process_runtime.reset_transport(0.0);
                        for graph in graph_runtimes.iter_mut() {
                            graph.reset_transport(0.0);
                        }
                    }
                    // Anchored per-lane phase (takes spec 7.3): every chunk
                    // schedules with the governing row's clip anchors, so
                    // each track's step position is projected from its own
                    // clip (`start_beat` + offset) instead of the shared
                    // free-running clock. Installed after any wrap reset so
                    // the anchors survive it.
                    let (anchor_beat, lane_offsets) = song.row_clock_anchor(row);
                    clock.set_song_row_anchors(anchor_beat, lane_offsets);
                    // Manual-override latch (takes spec 10): latched tracks
                    // suspend the song's launch authority — they schedule
                    // from the LIVE session snapshot, free-running (anchor
                    // cleared), and row boundaries neither swap their
                    // content nor reset their accumulators.
                    let latch = state.song_manual_latch_mask();
                    let row_track_count = song_row_snapshot
                        .as_deref()
                        .map(|snapshot| snapshot.tracks.len())
                        .expect("row snapshot set above");
                    let live_track_count = base_snapshot.tracks.len().min(MAX_TRACKS);
                    if latch != 0 || live_track_count > row_track_count {
                        let mut merged =
                            (*song_row_snapshot.take().expect("row snapshot set above")).clone();
                        let track_count = merged
                            .tracks
                            .len()
                            .min(base_snapshot.tracks.len())
                            .min(64);
                        for track in 0..track_count {
                            if latch >> track & 1 == 1 {
                                merged.tracks[track] =
                                    Arc::clone(&base_snapshot.tracks[track]);
                                clock.clear_track_anchor(track);
                                if !wrapped {
                                    pending_accum_reset[track] = false;
                                }
                            }
                        }
                        // Tracks created after the song preflight are unknown
                        // to every prebuilt row snapshot, and the clock only
                        // steps `0..num_tracks` of the chunk snapshot — without
                        // this they neither trigger nor publish a playhead
                        // until the next Play. They are latched at creation
                        // (`latch_track_created_during_song_playback`), so
                        // schedule them from the live session lanes,
                        // free-running like any latched lane.
                        for track in merged.tracks.len()..live_track_count {
                            merged.tracks.push(Arc::clone(&base_snapshot.tracks[track]));
                            clock.clear_track_anchor(track);
                            if !wrapped {
                                pending_accum_reset[track] = false;
                            }
                        }
                        merged.transport.num_tracks =
                            merged.transport.num_tracks.max(merged.tracks.len());
                        song_row_snapshot = Some(Arc::new(merged));
                    }
                }
            }
        }
        let snapshot: &SequencerSnapshot = song_row_snapshot
            .as_deref()
            .or(session_launch_snapshot.as_deref())
            .unwrap_or(base_snapshot);
        let chunk_start_beats = clock.total_beats;
        // Control-thread channel writes land on the chunk boundary, in order,
        // with a defined beat (docs/jaki-live-channel-widgets-spec.md 7). This
        // has to precede the `chan-get` snapshot published below so a tick in
        // this chunk observes the write.
        let mut channel_write_invocations = Vec::new();
        for (name, literal) in state.take_process_channel_writes() {
            channel_write_invocations.extend(process_runtime.send_channel_at(
                &name,
                literal.to_value(),
                chunk_start_beats,
                scheduled_until_sample,
            ));
        }
        let roll_grid = crate::sequencer::Timebase::from_index(
            state.transport.roll_rate.load(Ordering::Relaxed),
        )
        .step_beats(MAX_STEPS);
        let triggers = clock.process_chunk_with_roll(
            chunk_frames,
            snapshot,
            state,
            Some(&mut scheduler.roll.window_start),
            roll_grid,
        );
        scheduler.roll.publish_windows(state, roll_grid);
        let chunk_end_beats = clock.total_beats;
        let mut neural_events = Vec::new();
        let mut neural_cursor_beats = chunk_start_beats;
        let mut neural_cursor_sample = scheduled_until_sample;
        let mut chunk_enqueued = true;
        let mut neural_reset_groups: Vec<(usize, f64)> = Vec::new();
        for trigger in &triggers {
            process_runtime.record_track_step_boundary(trigger.track, trigger.absolute_beats);
            let step = &snapshot.tracks[trigger.track].steps[trigger.step];
            if !step.active || !step.neural_reset {
                continue;
            }
            let is_new_group = neural_reset_groups.last().map_or(true, |(offset, beats)| {
                *offset != trigger.offset || (*beats - trigger.absolute_beats).abs() > 1e-9
            });
            if is_new_group {
                neural_reset_groups.push((trigger.offset, trigger.absolute_beats));
            }
        }
        let mut neural_reset_group_idx = 0;
        for trigger in triggers {
            let trigger_sample_time = scheduled_until_sample + trigger.offset as u64;
            let conductor_invocations =
                process_runtime.take_conductor_invocations_before(trigger.absolute_beats);
            if !invoke_conductor_invocations(
                scratch_runtime,
                process_runtime,
                graph_runtimes,
                conductor_invocations,
                debug_accum,
            ) || !enqueue_due_process_emissions(
                queue,
                snapshot,
                &mut track_output_events,
                scratch_runtime,
                midi_fx_quantizer_state,
                process_runtime,
                pattern_epoch,
                chunk_start_beats,
                scheduled_until_sample,
                trigger.absolute_beats,
                samples_per_quarter,
                debug_accum,
            ) {
                chunk_enqueued = false;
                break;
            }
            process_neural_boundaries_until(
                neural_runtime,
                &mut neural_cursor_beats,
                &mut neural_cursor_sample,
                trigger.absolute_beats,
                trigger_sample_time,
                samples_per_quarter,
                &mut neural_events,
            );
            if let Some((reset_offset, reset_beats)) =
                neural_reset_groups.get(neural_reset_group_idx).copied()
            {
                if reset_offset == trigger.offset
                    && (reset_beats - trigger.absolute_beats).abs() <= 1e-9
                {
                    neural_runtime.reset_state(reset_beats);
                    neural_cursor_beats = reset_beats;
                    neural_cursor_sample = trigger_sample_time;
                    neural_reset_group_idx += 1;
                }
            }
            if !snapshot.tracks[trigger.track].steps[trigger.step].active {
                let sample_time = scheduled_until_sample + trigger.offset as u64;
                let send_params = resolve_track_send_params(snapshot, trigger.track, trigger.step);
                if !send_params.is_empty() {
                    chunk_enqueued &= queue.push(ScheduledEvent {
                        pattern_epoch,
                        sample_time,
                        kind: ScheduledEventKind::EffectParams {
                            track: trigger.track,
                            effect_params: send_params,
                        },
                    }).is_ok();
                }
                chunk_enqueued &= enqueue_instrument_param_change(
                    queue,
                    pattern_epoch,
                    sample_time,
                    trigger.track,
                    resolve_instrument_plocks(snapshot, trigger.track, trigger.step),
                );
                if !chunk_enqueued {
                    break;
                }
                continue;
            }
            if track_has_live_midi_fx_notes(
                live_midi_fx_tracks,
                snapshot,
                &midi_fx_descriptors_for_scheduling,
                trigger.track,
            ) {
                continue;
            }
            let track = &snapshot.tracks[trigger.track];
            if trigger.step == 0 && pending_accum_reset[trigger.track] {
                pending_accum_reset[trigger.track] = false;
                if let Some(def) = ACCUMULATOR_REGISTRY.get(track.params.accumulator_idx) {
                    accumulator_states[trigger.track] = AccumulatorRuntimeState {
                        value: def.reset_value,
                        reversed: false,
                    };
                } else {
                    accumulator_states[trigger.track] = AccumulatorRuntimeState::default();
                }
            }
            let step_snapshot = &track.steps[trigger.step];
            let swing_pct = step_snapshot.swing_override.unwrap_or(track.params.swing);
            let swing_resolution = step_snapshot
                .swing_resolution_override
                .unwrap_or(track.params.swing_resolution);
            let swing_step = swing_bucket_index(trigger.cycle_start_beats, swing_resolution);
            let is_odd_step = swing_step % 2 == 1;
            let step_boundary_sample_time = scheduled_until_sample + trigger.offset as u64;
            let mut sample_time = if step_snapshot.chord.is_empty() {
                delayed_step_sample_time(
                    step_boundary_sample_time,
                    &step_snapshot.params,
                    trigger.samples_per_step,
                )
            } else {
                step_boundary_sample_time
            };
            if is_odd_step && swing_pct > 50.0 {
                let swing_delay = swing_delay_samples(
                    sample_rate as f64,
                    snapshot.transport.bpm as f64,
                    swing_pct,
                    swing_resolution,
                )
                .round();
                sample_time = sample_time.saturating_add(swing_delay.max(0.0) as u64);
            }

            let mut resolved = ResolvedStep {
                duration: step_snapshot.params[StepParam::Duration.index()],
                velocity: step_snapshot.params[StepParam::Velocity.index()],
                speed: step_snapshot.params[StepParam::Speed.index()],
                aux_a: step_snapshot.params[StepParam::AuxA.index()],
                aux_b: step_snapshot.params[StepParam::AuxB.index()],
                transpose: step_snapshot.params[StepParam::Transpose.index()],
                pan: step_snapshot.params[StepParam::Pan.index()],
                chop: step_snapshot.params[StepParam::Chop.index()],
            };
            // Live step-param printing (bead eseq-jc9): while the *step*
            // panel's print latch is armed for this track, the latched values
            // are what must be HEARD now — the pattern write lands behind the
            // playhead (each step is stamped after it was already scheduled),
            // so without this substitution a printed value would only become
            // audible one loop later. Substituting here mirrors exactly what
            // the stamped step_data plays back on the next pass. Chord-backed
            // steps own their sounding durations per note (they beat
            // `resolved.duration` at fire time), and the stamp moves them by
            // the base-param delta (`set_step_param_no_publish`) — so the
            // audible substitution carries the same delta onto the scheduled
            // chord below. Transpose needs no chord handling: playback
            // already applies `resolved.transpose - step_transpose` as a
            // delta per chord note (`resolved_chord_transpose`).
            let mut print_chord_duration_delta: Option<f32> = None;
            {
                let (velocity, duration, transpose) = state
                    .step_print_override
                    .values_for_track(trigger.track);
                if let Some(value) = velocity {
                    resolved.velocity = value;
                }
                if let Some(value) = duration {
                    print_chord_duration_delta = Some(value - resolved.duration);
                    resolved.duration = value;
                }
                if let Some(value) = transpose {
                    resolved.transpose = value;
                }
            }
            let mut process_overlay = ProcessTargetOverlay::default();
            let mut process_base_alive = true;
            let step_beats = trigger.samples_per_step / samples_per_quarter as f32;
            let process_chain = &track.process_chain;
            let mut process_inlet_writes =
                process_runtime.take_step_process_inlet_writes(trigger.track, process_chain);
            let mut deferred_process_inlet_writes = Vec::new();
            for (slot_index, slot) in process_chain.slots.iter().enumerate() {
                if !slot.enabled {
                    continue;
                }
                let slot_inlet_writes =
                    process_inlet_writes.remove(&slot_index).unwrap_or_default();
                let writes = process_runtime.step_process_writes_with_inlet_writes(
                    slot,
                    trigger.step,
                    trigger.cycle,
                    track.params.num_steps,
                    Some(&slot_inlet_writes),
                );
                {
                    let mut inlet_context = ProcessInletWriteContext {
                        chain: process_chain,
                        current_slot_index: Some(slot_index),
                        current_fire_writes: &mut process_inlet_writes,
                        deferred_writes: &mut deferred_process_inlet_writes,
                    };
                    apply_process_target_writes(
                        snapshot,
                        &midi_fx_descriptors_for_scheduling,
                        trigger.track,
                        trigger.step,
                        &mut resolved,
                        &mut process_overlay,
                        Some(slot),
                        &writes,
                        Some(&mut inlet_context),
                    );
                }
                let event = process_step_event_value(
                    trigger.track,
                    trigger.step,
                    trigger.cycle,
                    trigger.absolute_beats,
                    sample_time,
                    resolved,
                    step_beats,
                );
                if let Some(invocation) = process_runtime.step_process_invocation_with_inlet_writes(
                    slot,
                    crate::process::ProcessStepRunContext {
                        track: trigger.track,
                        step: trigger.step,
                        cycle: trigger.cycle,
                        beat: trigger.absolute_beats,
                        sample_time,
                        step_beats,
                        resolved,
                        event,
                    },
                    Some(&slot_inlet_writes),
                ) {
                    if !invoke_process_cascade(
                        scratch_runtime,
                        process_runtime,
                        invocation,
                        debug_accum,
                        |scratch, process_runtime, runtime_id, commands| {
                            let mut inlet_context = ProcessInletWriteContext {
                                chain: process_chain,
                                current_slot_index: Some(slot_index),
                                current_fire_writes: &mut process_inlet_writes,
                                deferred_writes: &mut deferred_process_inlet_writes,
                            };
                            apply_step_process_commands(
                                scratch,
                                process_runtime,
                                runtime_id,
                                snapshot,
                                &midi_fx_descriptors_for_scheduling,
                                trigger.track,
                                trigger.step,
                                trigger.absolute_beats,
                                trigger.samples_per_step,
                                Some(slot),
                                &mut resolved,
                                &mut process_overlay,
                                &mut process_base_alive,
                                commands,
                                Some(&mut inlet_context),
                                debug_accum,
                            );
                            apply_graph_process_commands(graph_runtimes, commands);
                        },
                    ) {
                        chunk_enqueued = false;
                        break;
                    }
                }
            }
            if !chunk_enqueued {
                break;
            }
            for deferred in deferred_process_inlet_writes.drain(..) {
                process_runtime.defer_step_process_inlet_write(
                    deferred.track,
                    deferred.instance_id,
                    deferred.inlet,
                    deferred.write,
                );
            }
            let track_fire_event = process_step_event_value(
                trigger.track,
                trigger.step,
                trigger.cycle,
                trigger.absolute_beats,
                sample_time,
                resolved,
                step_beats,
            );
            let track_fire_step_context = crate::process::ProcessStepEventContext {
                track: trigger.track,
                step: trigger.step,
                cycle: trigger.cycle,
                beat: trigger.absolute_beats,
                sample_time,
                step_beats,
                resolved,
            };
            for invocation in process_runtime.track_fires_at(
                trigger.track,
                track_fire_event.clone(),
                trigger.absolute_beats,
                sample_time,
                track_fire_step_context.clone(),
            ) {
                if !invoke_process_cascade(
                    scratch_runtime,
                    process_runtime,
                    invocation,
                    debug_accum,
                    |scratch, process_runtime, runtime_id, commands| {
                        apply_step_process_commands(
                            scratch,
                            process_runtime,
                            runtime_id,
                            snapshot,
                            &midi_fx_descriptors_for_scheduling,
                            trigger.track,
                            trigger.step,
                            trigger.absolute_beats,
                            trigger.samples_per_step,
                            None,
                            &mut resolved,
                            &mut process_overlay,
                            &mut process_base_alive,
                            commands,
                            None,
                            debug_accum,
                        );
                        apply_graph_process_commands(graph_runtimes, commands);
                    },
                ) {
                    chunk_enqueued = false;
                    break;
                }
            }
            if !chunk_enqueued
                || !enqueue_due_process_emissions(
                    queue,
                    snapshot,
                    &mut track_output_events,
                    scratch_runtime,
                    midi_fx_quantizer_state,
                    process_runtime,
                    pattern_epoch,
                    chunk_start_beats,
                    scheduled_until_sample,
                    trigger.absolute_beats,
                    samples_per_quarter,
                    debug_accum,
                )
            {
                chunk_enqueued = false;
                break;
            }
            let rs = &mut accumulator_states[trigger.track];
            let builtin_count = ACCUMULATOR_REGISTRY.len();
            let actions = if let Some(def) = ACCUMULATOR_REGISTRY.get(track.params.accumulator_idx)
            {
                let (actions, raw_new) =
                    (def.func)(resolved, resolved.aux_a, rs.value, rs.reversed);
                rs.value = apply_limit_mode(
                    raw_new,
                    track.params.accum_limit,
                    AccumMode::from_u32(track.params.accum_mode),
                    &mut rs.reversed,
                );
                actions
            } else if track.params.accumulator_idx >= builtin_count {
                let delta = if rs.reversed {
                    -resolved.aux_a
                } else {
                    resolved.aux_a
                };
                let raw_new = rs.value + delta;
                rs.value = apply_limit_mode(
                    raw_new,
                    track.params.accum_limit,
                    AccumMode::from_u32(track.params.accum_mode),
                    &mut rs.reversed,
                );
                let mut effect_params =
                    resolve_effect_params(snapshot, trigger.track, trigger.step);
                effect_params.extend(resolve_track_send_params(
                    snapshot,
                    trigger.track,
                    trigger.step,
                ));
                let mut instrument_params =
                    resolve_instrument_params(snapshot, trigger.track, trigger.step);
                upsert_effect_params(&mut effect_params, process_overlay.effect_params.clone());
                upsert_instrument_params(
                    &mut instrument_params,
                    process_overlay.instrument_params.clone(),
                );
                let script_idx = if let Some(runtime) = scratch_runtime.as_ref() {
                    if let Some(name) = track.params.script_accumulator_name.as_ref() {
                        runtime
                            .accumulator_names()
                            .iter()
                            .position(|entry| entry == name)
                    } else {
                        track.params.accumulator_idx.checked_sub(builtin_count)
                    }
                } else {
                    None
                };
                if debug_accum && debug_accum_invocations < 200 {
                    let debug_note_spans =
                        track_note_spans_for_trigger(snapshot, trigger.track, trigger.step);
                    eprintln!(
                        "[accum] trigger track={} step={} acc_idx={} script_name={:?} runtime={} script_idx={:?} chord={:?} chord_durs={:?} dur={} note_spans={:?}",
                        trigger.track,
                        trigger.step,
                        track.params.accumulator_idx,
                        track.params.script_accumulator_name,
                        scratch_runtime.is_some(),
                        script_idx,
                        step_snapshot.chord,
                        step_snapshot.chord_durations,
                        resolved.duration,
                        debug_note_spans,
                    );
                }
                if let (Some(runtime), Some(script_idx)) = (scratch_runtime.as_mut(), script_idx) {
                    let note_spans =
                        track_note_spans_for_trigger(snapshot, trigger.track, trigger.step);
                    runtime.set_position(trigger.track, trigger.step);
                    match runtime.invoke_accumulator(
                        script_idx,
                        trigger.step,
                        rs.value,
                        resolved,
                        step_snapshot.chord.clone(),
                        step_snapshot.chord_durations.clone(),
                        step_snapshot.params[StepParam::Transpose.index()],
                        Some(note_spans.clone()),
                        trigger.samples_per_step
                            / (sample_rate as f32 * 60.0 / snapshot.transport.bpm as f32),
                        track.params.num_steps,
                        track.effect_slots.clone(),
                        track.instrument_slot.clone(),
                        effect_params,
                        instrument_params.to_vec(),
                    ) {
                        Ok(output) => {
                            if debug_accum && debug_accum_invocations < 200 {
                                eprintln!(
                                    "[accum] invoke ok track={} step={} suppressed={} emitted={} resolved={:?}",
                                    trigger.track,
                                    trigger.step,
                                    output.suppressed,
                                    output.emitted.len(),
                                    output.resolved,
                                );
                                for (idx, emitted) in output.emitted.iter().take(12).enumerate() {
                                    eprintln!(
                                        "[accum] emitted[{}] offset={} note={} dur={} vel={} chord={:?}",
                                        idx,
                                        emitted.offset_beats,
                                        emitted.resolved.transpose,
                                        emitted.resolved.duration,
                                        emitted.resolved.velocity,
                                        emitted.chord,
                                    );
                                }
                            }
                            debug_accum_invocations = debug_accum_invocations.saturating_add(1);
                            let samples_per_quarter =
                                sample_rate as f32 * 60.0 / snapshot.transport.bpm as f32;
                            let step_beats = trigger.samples_per_step / samples_per_quarter;
                            let mut accumulator_events = Vec::new();
                            if !output.suppressed && process_base_alive {
                                process_runtime.record_track_fire(
                                    trigger.track,
                                    trigger.absolute_beats,
                                    sample_time,
                                    crate::process::resolved_values_from_step(
                                        output.resolved,
                                        &step_snapshot.params,
                                    ),
                                );
                                let mut event_effect_params = output.effect_params.clone();
                                event_effect_params.extend(resolve_track_send_params(
                                    snapshot,
                                    trigger.track,
                                    trigger.step,
                                ));
                                let mut event_instrument_params =
                                    scheduled_instrument_params_from_vec(
                                        output.instrument_params.clone(),
                                    );
                                upsert_effect_params(
                                    &mut event_effect_params,
                                    process_overlay.effect_params.clone(),
                                );
                                upsert_instrument_params(
                                    &mut event_instrument_params,
                                    process_overlay.instrument_params.clone(),
                                );
                                accumulator_events.push(MidiFxEvent {
                                    offset_beats: 0.0,
                                    track: trigger.track,
                                    step: trigger.step,
                                    samples_per_step: trigger.samples_per_step,
                                    step_beats,
                                    resolved: output.resolved,
                                    chord: step_snapshot.chord.clone(),
                                    chord_durations: step_snapshot.chord_durations.clone(),
                                    chord_delays: step_snapshot.chord_delays.clone(),
                                    chord_step_transpose: step_snapshot.params
                                        [StepParam::Transpose.index()],
                                    note_spans: Some(note_spans.clone()),
                                    arp_phase_beats: trigger.absolute_beats as f32,
                                    midi_fx_params: process_overlay.midi_fx_params.clone(),
                                    effect_params: event_effect_params,
                                    instrument_params: event_instrument_params,
                                    instrument_tensor_params: resolve_instrument_tensor_params(
                                        snapshot,
                                        trigger.track,
                                        trigger.step,
                                    ),
                                    sampler_params: resolve_sampler_params(
                                        snapshot,
                                        trigger.track,
                                        trigger.step,
                                    ),
                                    rack_macro_values: process_overlay.rack_macro_values,
                                    source: EventSource::Step {
                                        track: trigger.track,
                                        step: trigger.step,
                                        instrument_fingerprint: 0,
                                    },
                                });
                            }
                            for emitted in output.emitted {
                                let target_track = emitted.track.unwrap_or(trigger.track);
                                if target_track >= snapshot.tracks.len() {
                                    continue;
                                }
                                let chord_len = emitted.chord.len();
                                let mut event_effect_params = emitted.effect_params;
                                event_effect_params.extend(resolve_track_send_params(
                                    snapshot,
                                    target_track,
                                    trigger.step,
                                ));
                                let mut event_instrument_params =
                                    scheduled_instrument_params_from_vec(emitted.instrument_params);
                                if target_track == trigger.track {
                                    upsert_effect_params(
                                        &mut event_effect_params,
                                        process_overlay.effect_params.clone(),
                                    );
                                    upsert_instrument_params(
                                        &mut event_instrument_params,
                                        process_overlay.instrument_params.clone(),
                                    );
                                }
                                let event = MidiFxEvent {
                                    offset_beats: emitted.offset_beats,
                                    track: trigger.track,
                                    step: trigger.step,
                                    samples_per_step: trigger.samples_per_step,
                                    step_beats,
                                    resolved: emitted.resolved,
                                    chord: emitted.chord,
                                    chord_durations: emitted.chord_durations,
                                    chord_delays: vec![0.0; chord_len],
                                    chord_step_transpose: emitted.chord_step_transpose,
                                    note_spans: None,
                                    arp_phase_beats: trigger.absolute_beats as f32,
                                    midi_fx_params: process_overlay.midi_fx_params.clone(),
                                    effect_params: event_effect_params,
                                    instrument_params: event_instrument_params,
                                    instrument_tensor_params: resolve_instrument_tensor_defaults(
                                        snapshot,
                                        target_track,
                                    ),
                                    sampler_params: resolve_sampler_params(
                                        snapshot,
                                        trigger.track,
                                        trigger.step,
                                    ),
                                    rack_macro_values: process_overlay.rack_macro_values,
                                    source: EventSource::Step {
                                        track: trigger.track,
                                        step: trigger.step,
                                        instrument_fingerprint: 0,
                                    },
                                };
                                if let Some(event) =
                                    rebind_midi_fx_event_to_track(snapshot, event, target_track)
                                {
                                    accumulator_events.push(event);
                                }
                            }
                            for event in accumulator_events {
                                if track_has_live_midi_fx_notes(
                                    live_midi_fx_tracks,
                                    snapshot,
                                    &midi_fx_descriptors_for_scheduling,
                                    event.track,
                                ) {
                                    continue;
                                }
                                let final_events = if snapshot.tracks[event.track]
                                    .params
                                    .midi_fx_position
                                    == MidiFxPosition::PostAccumulator
                                    && !snapshot.tracks[event.track].params.midi_fx_chain.is_empty()
                                {
                                    run_midi_fx_chain_for_track(
                                        runtime,
                                        snapshot,
                                        event.track,
                                        vec![event],
                                        Some(&mut *midi_fx_quantizer_state),
                                        0,
                                        debug_accum,
                                    )
                                } else {
                                    vec![event]
                                };
                                if !enqueue_midi_fx_events(
                                    queue,
                                    snapshot,
                                    &mut track_output_events,
                                    pattern_epoch,
                                    sample_time,
                                    sample_time_to_beats(
                                        chunk_start_beats,
                                        scheduled_until_sample,
                                        sample_time,
                                        samples_per_quarter.into(),
                                    ),
                                    samples_per_quarter,
                                    process_runtime.global_transpose(),
                                    final_events,
                                ) {
                                    chunk_enqueued = false;
                                    break;
                                }
                            }
                            if !chunk_enqueued {
                                break;
                            }
                            continue;
                        }
                        Err(err) => {
                            if debug_accum && debug_accum_invocations < 200 {
                                eprintln!(
                                    "[accum] invoke err track={} step={} script_idx={} err={}",
                                    trigger.track, trigger.step, script_idx, err
                                );
                            }
                            debug_accum_invocations = debug_accum_invocations.saturating_add(1);
                        }
                    }
                } else if debug_accum && debug_accum_invocations < 200 {
                    eprintln!(
                        "[accum] no script runtime/index track={} step={} runtime={} script_idx={:?}",
                        trigger.track,
                        trigger.step,
                        scratch_runtime.is_some(),
                        script_idx
                    );
                    debug_accum_invocations = debug_accum_invocations.saturating_add(1);
                }
                crate::accumulator::ActionBuffer::just(StepAction::Play(resolved))
            } else {
                crate::accumulator::ActionBuffer::just(StepAction::Play(resolved))
            };

            let mut recorded_track_fire = false;
            for action in actions.iter() {
                if !process_base_alive {
                    continue;
                }
                let (target_track, resolved) = match *action {
                    StepAction::Play(resolved) => (trigger.track, resolved),
                    StepAction::SendToTrack { track, resolved } => (track, resolved),
                    StepAction::Silence => continue,
                };
                if !recorded_track_fire {
                    process_runtime.record_track_fire(
                        trigger.track,
                        trigger.absolute_beats,
                        sample_time,
                        crate::process::resolved_values_from_step(resolved, &step_snapshot.params),
                    );
                    recorded_track_fire = true;
                }
                if target_track >= snapshot.tracks.len() {
                    continue;
                }
                if track_has_live_midi_fx_notes(
                    live_midi_fx_tracks,
                    snapshot,
                    &midi_fx_descriptors_for_scheduling,
                    target_track,
                ) {
                    continue;
                }
                let same_track_process_targets = target_track == trigger.track;
                let mut effect_params = resolve_effect_params(snapshot, target_track, trigger.step);
                effect_params.extend(resolve_track_send_params(snapshot, target_track, trigger.step));
                let mut instrument_params =
                    resolve_instrument_params(snapshot, target_track, trigger.step);
                let midi_fx_params = if same_track_process_targets {
                    upsert_effect_params(&mut effect_params, process_overlay.effect_params.clone());
                    upsert_instrument_params(
                        &mut instrument_params,
                        process_overlay.instrument_params.clone(),
                    );
                    process_overlay.midi_fx_params.clone()
                } else {
                    Vec::new()
                };
                let instrument_tensor_params =
                    resolve_instrument_tensor_params(snapshot, target_track, trigger.step);
                let samples_per_quarter = sample_rate as f32 * 60.0 / snapshot.transport.bpm as f32;
                if snapshot.tracks[target_track].params.midi_fx_position
                    == MidiFxPosition::PostAccumulator
                    && !snapshot.tracks[target_track]
                        .params
                        .midi_fx_chain
                        .is_empty()
                {
                    if let Some(runtime) = scratch_runtime.as_mut() {
                        let seed_chord = step_chord_data(snapshot, target_track, trigger.step);
                        let seed_event = step_event_from_resolved(
                            snapshot,
                            target_track,
                            trigger.step,
                            trigger.samples_per_step,
                            resolved,
                            seed_chord,
                            effect_params.clone(),
                            instrument_params.clone(),
                            instrument_tensor_params.clone(),
                        );
                        let mut events = midi_fx_window_events_from_step(
                            snapshot,
                            &midi_fx_descriptors_for_scheduling,
                            target_track,
                            trigger.step,
                            trigger.samples_per_step,
                            trigger.samples_per_step / samples_per_quarter,
                            samples_per_quarter.into(),
                            trigger.absolute_beats as f32,
                            resolved,
                            effect_params,
                            instrument_params,
                            instrument_tensor_params,
                        );
                        for event in &mut events {
                            event.midi_fx_params = midi_fx_params.clone();
                        }
                        let events = run_midi_fx_chain_for_track(
                            runtime,
                            snapshot,
                            target_track,
                            events,
                            Some(&mut *midi_fx_quantizer_state),
                            0,
                            debug_accum,
                        );
                        if !enqueue_midi_fx_events(
                            queue,
                            snapshot,
                            &mut track_output_events,
                            pattern_epoch,
                            sample_time,
                            sample_time_to_beats(
                                chunk_start_beats,
                                scheduled_until_sample,
                                sample_time,
                                samples_per_quarter.into(),
                            ),
                            samples_per_quarter.into(),
                            process_runtime.global_transpose(),
                            events,
                        ) {
                            chunk_enqueued = false;
                            break;
                        }
                        let seed_beats = trigger.absolute_beats;
                        neural_runtime.process_seed_at(&seed_event, seed_beats);
                        seed_graph_runtimes(
                            graph_runtimes,
                            &seed_event,
                            seed_beats,
                            samples_per_quarter.into(),
                        );
                    } else {
                        let mut chord = step_chord_data(snapshot, target_track, trigger.step);
                        if target_track == trigger.track {
                            shift_chord_durations_for_print(
                                &mut chord,
                                print_chord_duration_delta,
                            );
                        }
                        let step_event = step_event_from_resolved(
                            snapshot,
                            target_track,
                            trigger.step,
                            trigger.samples_per_step,
                            resolved,
                            chord,
                            effect_params,
                            instrument_params,
                            instrument_tensor_params,
                        );
                        let ok = enqueue_step_event(
                            queue,
                            snapshot,
                            &mut track_output_events,
                            pattern_epoch,
                            sample_time,
                            sample_time_to_beats(
                                chunk_start_beats,
                                scheduled_until_sample,
                                sample_time,
                                samples_per_quarter.into(),
                            ),
                            samples_per_quarter,
                            process_runtime.global_transpose(),
                            step_event.clone(),
                        );
                        let seed_beats = trigger.absolute_beats;
                        neural_runtime.process_seed_at(&step_event, seed_beats);
                        seed_graph_runtimes(
                            graph_runtimes,
                            &step_event,
                            seed_beats,
                            samples_per_quarter.into(),
                        );
                        if !ok {
                            chunk_enqueued = false;
                            break;
                        }
                    }
                } else {
                    let mut chord = step_chord_data(snapshot, target_track, trigger.step);
                    if target_track == trigger.track {
                        shift_chord_durations_for_print(&mut chord, print_chord_duration_delta);
                    }
                    let step_event = step_event_from_resolved(
                        snapshot,
                        target_track,
                        trigger.step,
                        trigger.samples_per_step,
                        resolved,
                        chord,
                        effect_params,
                        instrument_params,
                        instrument_tensor_params,
                    );
                    let ok = enqueue_step_event(
                        queue,
                        snapshot,
                        &mut track_output_events,
                        pattern_epoch,
                        sample_time,
                        sample_time_to_beats(
                            chunk_start_beats,
                            scheduled_until_sample,
                            sample_time,
                            samples_per_quarter.into(),
                        ),
                        samples_per_quarter,
                        process_runtime.global_transpose(),
                        step_event.clone(),
                    );
                    let seed_beats = trigger.absolute_beats;
                    neural_runtime.process_seed_at(&step_event, seed_beats);
                    seed_graph_runtimes(
                        graph_runtimes,
                        &step_event,
                        seed_beats,
                        samples_per_quarter.into(),
                    );
                    if !ok {
                        chunk_enqueued = false;
                        break;
                    }
                }
            }
            if !chunk_enqueued {
                break;
            }
        }
        if chunk_enqueued {
            let conductor_invocations =
                process_runtime.take_conductor_invocations_through(chunk_end_beats);
            chunk_enqueued = invoke_conductor_invocations(
                scratch_runtime,
                process_runtime,
                graph_runtimes,
                conductor_invocations,
                debug_accum,
            ) && enqueue_due_process_emissions(
                queue,
                snapshot,
                &mut track_output_events,
                scratch_runtime,
                midi_fx_quantizer_state,
                process_runtime,
                pattern_epoch,
                chunk_start_beats,
                scheduled_until_sample,
                chunk_end_beats,
                samples_per_quarter,
                debug_accum,
            );
        }
        if !chunk_enqueued {
            break;
        }
        neural_runtime.process_boundaries_with_outputs(
            neural_cursor_beats,
            chunk_end_beats,
            neural_cursor_sample,
            samples_per_quarter,
            &mut neural_events,
        );
        state.set_neural_visualization(neural_runtime.visualization_snapshot());
        for output in &mut neural_events {
            if !output.emit_trigger {
                continue;
            }
            let event_beats = sample_time_to_beats(
                chunk_start_beats,
                scheduled_until_sample,
                output.sample_time,
                samples_per_quarter,
            );
            output.sample_time = swung_network_sample_time(
                snapshot,
                &output.event,
                output.sample_time,
                event_beats,
                samples_per_quarter,
            );
        }
        neural_events.sort_by_key(|output| {
            let neuron = match output.event.source {
                EventSource::Network { neuron, .. } => neuron,
                EventSource::Step { .. } => 0,
            };
            (output.sample_time, output.event.track, neuron)
        });
        for output in merge_neural_output_accents(neural_events) {
            let sample_time = output.sample_time;
            let event_beats = sample_time_to_beats(
                chunk_start_beats,
                scheduled_until_sample,
                sample_time,
                samples_per_quarter,
            ) as f32;
            if !enqueue_neural_output_with_midi_fx(
                queue,
                snapshot,
                &mut track_output_events,
                scratch_runtime.as_mut(),
                Some(&mut *midi_fx_quantizer_state),
                pattern_epoch,
                sample_time,
                samples_per_quarter as f32,
                process_runtime.global_transpose(),
                event_beats,
                output,
                debug_accum,
            ) {
                chunk_enqueued = false;
                break;
            }
        }
        if !chunk_enqueued {
            break;
        }

        // Lisp-defined generators: self-clocked over this chunk, additive
        // (like the neural layer). Each boundary invokes the generator's
        // :tick on the scheduler-side VM; seq-emit output is resolved to a
        // NetworkTrigger here.
        if !generator_runtime.is_empty() {
            let mut generator_emissions = Vec::new();
            if let Some(scratch) = scratch_runtime.as_mut() {
                // Channel snapshot for chan-get: ticks in this chunk observe
                // process-channel writes from earlier chunks (processes run
                // after generators within a chunk).
                scratch.set_generator_channel_values(
                    process_runtime.payload_epoch(),
                    process_runtime.channel_values(),
                );
                generator_runtime.process_block(
                    chunk_start_beats,
                    chunk_end_beats,
                    scheduled_until_sample,
                    samples_per_quarter,
                    |input| {
                        let generator_index = input.generator_index;
                        let random_state = input.random_state;
                        let fallback_state = input.state.clone();
                        scratch
                            .invoke_sequencer_tick(generator_index, input)
                            .unwrap_or(crate::generator::GeneratorTickResult {
                                emitted: Vec::new(),
                                random_state,
                                state: fallback_state,
                            })
                    },
                    &mut generator_emissions,
                );
            } else if debug_routing_enabled() {
                eprintln!(
                    "[routing] skip generator-block reason=no-scratch-runtime chunk=({:.6}..{:.6})",
                    chunk_start_beats, chunk_end_beats
                );
            }
            // Velocity-merge coincident hits only when they are the same note.
            // Different notes at the same sample/track are polyphony.
            for emission in merge_generator_emission_accents(generator_emissions) {
                let event_beats = sample_time_to_beats(
                    chunk_start_beats,
                    scheduled_until_sample,
                    emission.sample_time,
                    samples_per_quarter,
                ) as f32;
                if debug_routing_enabled() {
                    eprintln!(
                        "[routing] generator-emission generator={} track={:?} sample={} beats={:.6} chain={:?} transpose={} vel={}",
                        emission.generator_index,
                        emission.event.track,
                        emission.sample_time,
                        event_beats,
                        emission
                            .event
                            .track
                            .and_then(|track| snapshot.tracks.get(track))
                            .map(|track| track.params.midi_fx_chain.as_slice())
                            .unwrap_or(&[]),
                        emission.event.resolved.transpose,
                        emission.event.resolved.velocity
                    );
                }
                if !enqueue_emitted_network_event_with_midi_fx(
                    queue,
                    snapshot,
                    &mut track_output_events,
                    scratch_runtime.as_mut(),
                    Some(&mut *midi_fx_quantizer_state),
                    pattern_epoch,
                    emission.sample_time,
                    samples_per_quarter as f32,
                    event_beats,
                    process_runtime.global_transpose(),
                    EmittedNetworkEventSource::Generator {
                        index: emission.generator_index,
                    },
                    emission.event,
                    debug_accum,
                ) {
                    chunk_enqueued = false;
                    break;
                }
            }
            if !chunk_enqueued {
                break;
            }
        }

        // Scheduler-owned processes: self-clocked like generators, but with
        // named inlets/outlets/channels and a pending store for future emits.
        if !process_runtime.is_empty() {
            if scratch_runtime.is_some() {
                // Listeners woken by this chunk's control-thread channel
                // writes run before the clocked processes, matching the beat
                // the writes were applied at.
                let mut invocations = std::mem::take(&mut channel_write_invocations);
                invocations.extend(process_runtime.process_block(
                    chunk_start_beats,
                    chunk_end_beats,
                    scheduled_until_sample,
                    samples_per_quarter,
                ));
                for invocation in invocations {
                    let mut pending_invocations = vec![invocation];
                    let mut processed_invocations = 0usize;
                    while let Some(invocation) = pending_invocations.pop() {
                        processed_invocations += 1;
                        if processed_invocations > PROCESS_EVENT_CASCADE_LIMIT {
                            if debug_accum || debug_routing_enabled() {
                                eprintln!(
                                    "[process] listener cascade limit exceeded limit={}",
                                    PROCESS_EVENT_CASCADE_LIMIT
                                );
                            }
                            chunk_enqueued = false;
                            break;
                        }
                        let invocation_beat = invocation.beat;
                        let process_runtime_id = invocation.runtime_id;
                        let Some(scratch) = scratch_runtime.as_mut() else {
                            break;
                        };
                        match scratch.invoke_process_run(invocation) {
                            Ok(result) => {
                                apply_graph_process_commands(graph_runtimes, &result.commands);
                                let mut followups = process_runtime.apply_run_result(result);
                                followups.reverse();
                                pending_invocations.extend(followups);
                            }
                            Err(err) => {
                                if debug_accum || debug_routing_enabled() {
                                    eprintln!(
                                        "[process] run error process={} beat={:.6} err={}",
                                        process_runtime_id, invocation_beat, err
                                    );
                                }
                            }
                        }
                        if !enqueue_due_process_emissions(
                            queue,
                            snapshot,
                            &mut track_output_events,
                            scratch_runtime,
                            midi_fx_quantizer_state,
                            process_runtime,
                            pattern_epoch,
                            chunk_start_beats,
                            scheduled_until_sample,
                            invocation_beat,
                            samples_per_quarter,
                            debug_accum,
                        ) {
                            chunk_enqueued = false;
                            break;
                        }
                    }
                    if !chunk_enqueued {
                        break;
                    }
                }
                if chunk_enqueued
                    && !enqueue_due_process_emissions(
                        queue,
                        snapshot,
                        &mut track_output_events,
                        scratch_runtime,
                        midi_fx_quantizer_state,
                        process_runtime,
                        pattern_epoch,
                        chunk_start_beats,
                        scheduled_until_sample,
                        chunk_end_beats,
                        samples_per_quarter,
                        debug_accum,
                    )
                {
                    chunk_enqueued = false;
                }
            } else if debug_routing_enabled() {
                eprintln!(
                    "[routing] skip process-block reason=no-scratch-runtime chunk=({:.6}..{:.6})",
                    chunk_start_beats, chunk_end_beats
                );
            }
            if !chunk_enqueued {
                break;
            }
        }

        // Graph-mode sequencers: native gather/scatter over this chunk. Each
        // fired node's :update predicate runs on the scheduler VM; firings
        // resolve to NetworkTriggers (velocity-merged + max_poly), additive
        // like the neural/generator layers.
        let log_graph_drive_chunk = debug_graph && debug_graph_drive_chunks < 60;
        if log_graph_drive_chunk {
            eprintln!(
                "[graph-drive] runtimes={} scratch={} chunk=({:.3}..{:.3})",
                graph_runtimes.len(),
                scratch_runtime.is_some(),
                chunk_start_beats,
                chunk_end_beats
            );
            for (i, rt) in graph_runtimes.iter().enumerate() {
                eprintln!("[graph-drive]   runtime[{i}] is_empty={}", rt.is_empty());
            }
        }
        for graph_index in 0..graph_runtimes.len() {
            if graph_runtimes[graph_index].is_empty() {
                continue;
            }
            let mut graph_emissions = Vec::new();
            let mut graph_eval_count = 0_usize;
            if let Some(scratch) = scratch_runtime.as_mut() {
                let manifest = &graph_manifests[graph_index];
                // Resolved (override-or-manifest) cap, carried on the runtime.
                let max_poly = graph_runtimes[graph_index].max_poly();
                graph_runtimes[graph_index].process_block(
                    chunk_start_beats,
                    chunk_end_beats,
                    scheduled_until_sample,
                    samples_per_quarter,
                    max_poly,
                    |eval| {
                        graph_eval_count += 1;
                        match scratch.invoke_graph_update(manifest, eval) {
                            Ok(decision) => decision,
                            Err(error) => {
                                if debug_graph {
                                    eprintln!(
                                        "[graph-update-error] graph={} node={} beat={:.6} error={}",
                                        manifest.name, eval.node_index, eval.beat, error
                                    );
                                }
                                crate::graph::NodeFire::default()
                            }
                        }
                    },
                    &mut graph_emissions,
                );
            } else if debug_routing_enabled() {
                eprintln!(
                    "[routing] skip graph-block reason=no-scratch-runtime graph_index={} chunk=({:.6}..{:.6})",
                    graph_index, chunk_start_beats, chunk_end_beats
                );
            }
            if log_graph_drive_chunk {
                eprintln!(
                    "[graph-drive]   runtime[{graph_index}] evals={} emissions={} node0_pending={}",
                    graph_eval_count,
                    graph_emissions.len(),
                    graph_runtimes[graph_index]
                        .pending_count_for_node(0)
                        .unwrap_or(0)
                );
            }
            // Velocity-merge coincident hits only when they are the same note.
            // Different notes at the same sample/track are polyphony.
            for emission in merge_graph_emission_accents(graph_emissions) {
                let event_beats = sample_time_to_beats(
                    chunk_start_beats,
                    scheduled_until_sample,
                    emission.sample_time,
                    samples_per_quarter,
                ) as f32;
                if debug_routing_enabled() {
                    eprintln!(
                        "[routing] graph-emission graph={} node={} track={:?} sample={} beats={:.6} chain={:?} transpose={} vel={}",
                        graph_index,
                        emission.node_index,
                        emission.event.track,
                        emission.sample_time,
                        event_beats,
                        emission
                            .event
                            .track
                            .and_then(|track| snapshot.tracks.get(track))
                            .map(|track| track.params.midi_fx_chain.as_slice())
                            .unwrap_or(&[]),
                        emission.event.resolved.transpose,
                        emission.event.resolved.velocity
                    );
                }
                if !enqueue_emitted_network_event_with_midi_fx(
                    queue,
                    snapshot,
                    &mut track_output_events,
                    scratch_runtime.as_mut(),
                    Some(&mut *midi_fx_quantizer_state),
                    pattern_epoch,
                    emission.sample_time,
                    samples_per_quarter as f32,
                    event_beats,
                    process_runtime.global_transpose(),
                    EmittedNetworkEventSource::Graph {
                        graph_index,
                        node_index: emission.node_index,
                    },
                    emission.event,
                    debug_accum,
                ) {
                    chunk_enqueued = false;
                    break;
                }
            }
            if !chunk_enqueued {
                break;
            }
        }
        publish_graph_visualizations(state, &graph_runtimes, chunk_end_beats);
        if log_graph_drive_chunk {
            debug_graph_drive_chunks += 1;
        }
        if !chunk_enqueued {
            break;
        }

        if let Some(runtime) = scratch_runtime.as_mut() {
            for pending in midi_fx_quantizer_state.drain_due(chunk_end_beats) {
                let deadline_sample = scheduled_until_sample.saturating_add(
                    ((pending.deadline_beats - chunk_start_beats).max(0.0) * samples_per_quarter)
                        .round() as u64,
                );
                let events = run_midi_fx_chain_for_track_inner(
                    runtime,
                    snapshot,
                    pending.source_track,
                    vec![pending.event],
                    Some(&mut *midi_fx_quantizer_state),
                    pending.resume_stage_idx,
                    0,
                    [false; MAX_TRACKS],
                    debug_accum,
                );
                if !enqueue_midi_fx_events(
                    queue,
                    snapshot,
                    &mut track_output_events,
                    pattern_epoch,
                    deadline_sample,
                    pending.deadline_beats,
                    samples_per_quarter as f32,
                    process_runtime.global_transpose(),
                    events,
                ) {
                    chunk_enqueued = false;
                    break;
                }
            }
        }
        if !chunk_enqueued {
            break;
        }

        // Track rolling (docs/rolling-core-spec.md 4.2): emit held-note roll
        // hits on every roll-grid boundary inside this chunk, layered on top
        // of pattern playback. The rate is re-read from the transport atomics
        // every chunk (F2) so mid-hold rate switches take effect at the next
        // boundary; note-offs drained before this pass cancel every hit not
        // yet inside the lookahead horizon (F3).
        if state.transport.roll_mode.load(Ordering::Relaxed) && scheduler.roll.any_held() {
            let roll_grid = crate::sequencer::Timebase::from_index(
                state.transport.roll_rate.load(Ordering::Relaxed),
            )
            .step_beats(MAX_STEPS);
            if !schedule_roll_hits(
                queue,
                snapshot,
                &mut track_output_events,
                state,
                &*clock,
                &mut scheduler.roll,
                roll_grid,
                chunk_start_beats,
                chunk_end_beats,
                scheduled_until_sample,
                rendered,
                samples_per_quarter,
                pattern_epoch,
                process_runtime.global_transpose(),
            ) {
                break;
            }
        }

        scheduled_until_sample = scheduled_until_sample.saturating_add(chunk_frames as u64);
    }

    scheduler.debug_graph_drive_chunks = debug_graph_drive_chunks;
    scheduler.debug_accum_invocations = debug_accum_invocations;
    state.set_track_output_current_beat(scheduler.clock.total_beats);
    state.append_track_output_events(track_output_events);
    SchedulerLookaheadResult {
        scheduled_until_sample,
    }
}
