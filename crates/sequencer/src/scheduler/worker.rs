/*!
Scheduler worker lifecycle and synchronization with live sequencer state.
*/

#[allow(unused_imports)]
use super::*;

pub fn spawn_scheduler_thread(
    state: Arc<SequencerState>,
    sample_rate: u32,
    scheduler_block_size: usize,
    rendered_samples: Arc<AtomicU64>,
    queue: Arc<ScheduledEventQueue<4096>>,
    live_keyboard_rx: mpsc::Receiver<KeyboardTrigger>,
) {
    let _ = thread::Builder::new()
        .name("sequencer-scheduler".to_string())
        .stack_size(SCHEDULER_THREAD_STACK_SIZE)
        .spawn(move || {
            let mut lookahead_state = SchedulerLookaheadState::new(sample_rate);
            let mut scheduled_until_sample = 0u64;
            let mut last_pattern = usize::MAX;
            let mut last_pattern_epoch = u64::MAX;
            let mut last_topology_epoch = u64::MAX;
            let mut last_playing = false;
            let lookahead_target_samples = (scheduler_block_size.max(1) * 4) as u64;
            let mut live_midi_fx_tracks: [LiveMidiFxTrackState; MAX_TRACKS] =
                std::array::from_fn(|_| LiveMidiFxTrackState::default());
            // Graph-mode sequencers: parallel vecs (manifest + live runtime), reconciled
            // by id from the published-sequencer channel. Held alongside the generator
            // runtime; both are additive layers over the neural/step output.
            let mut loaded_graph_overrides: Option<Vec<crate::graph::ProjectGraphOverrides>> = None;
            let mut loaded_neural_networks: Option<Vec<crate::neural::ProjectNeuralNetwork>> = None;
            let mut last_live_midi_fx_active = false;
            let mut scratch_source_version = u64::MAX;
            let mut published_sequencers_version = u64::MAX;
            let mut published_process_authoring_version = u64::MAX;
            let mut scratch_runtime = None;
            let debug_accum = std::env::var_os("TINYSEQ_DEBUG_ACCUM").is_some();
            let debug_graph = std::env::var_os("TINYSEQ_DEBUG_GRAPH").is_some();

            loop {
                let snapshot = state.latest_scheduler_snapshot();
                let playing = snapshot.transport.playing;
                let pattern = snapshot.transport.current_pattern;
                let pattern_epoch = snapshot.transport.pattern_epoch;
                let topology_epoch = snapshot.transport.topology_epoch;
                let rendered = rendered_samples.load(Ordering::Acquire);
                let latest_scratch_source_version = state.scratch_source_version();
                let (reset_all, reset_tracks) = state.take_accumulator_reset_requests();
                let requested_edit = state
                    .transport
                    .topology_edit_request_id
                    .load(Ordering::Acquire);
                let ready_edit = state
                    .transport
                    .topology_edit_ready_id
                    .load(Ordering::Acquire);
                let applied_edit = state
                    .transport
                    .topology_edit_applied_id
                    .load(Ordering::Acquire);
                let topology_edit_in_flight = state.topology_edit_in_flight();

                // Song playback commands (docs/song-mode-spec.md 10.2): the
                // control thread hands over a preflighted immutable song; the
                // scheduler owns row boundaries from here. Installing a song
                // rebuilds the horizon so scheduling starts from the song's
                // row snapshots immediately.
                for command in state.song_playback().drain_commands() {
                    match command {
                        crate::sequencer::SongPlaybackCommand::Start {
                            song,
                            start_beat,
                            open_ended,
                        } => {
                            let samples_per_quarter =
                                sample_rate as f64 * 60.0 / snapshot.transport.bpm.max(1) as f64;
                            match crate::sequencer::SongPlaybackRuntime::new(
                                song,
                                start_beat,
                                samples_per_quarter,
                            ) {
                                Ok(mut runtime) => {
                                    runtime.set_open_ended(open_ended);
                                    lookahead_state.song = Some(runtime);
                                    queue.clear();
                                    lookahead_state.clock.reset();
                                    lookahead_state.midi_fx_quantizer_state.reset();
                                    scheduled_until_sample = rendered;
                                    lookahead_state.pending_accum_reset = [true; MAX_TRACKS];
                                    lookahead_state.neural_runtime.reset_state(0.0);
                                    lookahead_state.generator_runtime.reset(0.0);
                                    lookahead_state.process_runtime.reset_transport(0.0);
                                    for graph in &mut lookahead_state.graph_runtimes {
                                        graph.reset(0.0);
                                    }
                                }
                                Err(error) => {
                                    state.song_playback().push_notice(
                                        crate::sequencer::SongPlaybackNotice::StartFailed {
                                            error,
                                        },
                                    );
                                }
                            }
                        }
                        // Edit-through (takes spec 16.7): rows the playhead
                        // has not reached pick up the edit; the queue and
                        // clock are untouched, so nothing already scheduled
                        // is disturbed.
                        crate::sequencer::SongPlaybackCommand::Refresh { song } => {
                            if let Some(runtime) = lookahead_state.song.as_mut() {
                                runtime.replace_song_in_place(song);
                            }
                        }
                        crate::sequencer::SongPlaybackCommand::Rebuild { song } => {
                            let clock_beats = lookahead_state.clock.total_beats;
                            if let Some(runtime) = lookahead_state.song.as_mut() {
                                runtime.rebuild_song(song, clock_beats);
                            }
                        }
                        crate::sequencer::SongPlaybackCommand::Stop => {
                            lookahead_state.song = None;
                            state.song_playback().clear_position();
                        }
                    }
                }
                let song_playback_active = lookahead_state.song.is_some();

                let latest_published_sequencers_version = state.published_sequencers_version();
                let latest_published_process_authoring_version =
                    state.published_process_authoring_version();
                if latest_scratch_source_version != scratch_source_version
                    || latest_published_sequencers_version != published_sequencers_version
                    || latest_published_process_authoring_version
                        != published_process_authoring_version
                {
                    let user_source = state.scratch_source();
                    if debug_accum {
                        eprintln!(
                            "[accum] scratch source version {} -> {} bytes={}",
                            scratch_source_version,
                            latest_scratch_source_version,
                            user_source.len()
                        );
                    }
                    scratch_runtime =
                        build_scheduler_scratch_runtime(Arc::clone(&state), &user_source, debug_accum);
                    if debug_accum {
                        if let Some(runtime) = scratch_runtime.as_ref() {
                            for track_idx in 0..state.active_track_count().min(MAX_TRACKS) {
                                let params = &state.pattern.track_params[track_idx];
                                eprintln!(
                                    "[accum] state track={} acc_idx={} script={:?}",
                                    track_idx,
                                    params.get_accumulator_idx(),
                                    params.script_accumulator_name()
                                );
                            }
                            eprintln!(
                                "[accum] scheduler runtime ready accumulators={:?} midi_fx={:?}",
                                runtime.accumulator_names(),
                                runtime.midi_fx_names()
                            );
                        } else {
                            eprintln!("[accum] scheduler runtime empty; clearing runtime");
                        }
                    }
                    // Register UI-authored generators (def-sequencer evaluated in any
                    // editor file, published via SequencerState). These need a runtime
                    // to live in even when there is no scratch/midi-fx source.
                    let published = state.published_sequencers();
                    let published_process_authoring = state.published_process_authoring();
                    if !published.is_empty() || !published_process_authoring.is_empty() {
                        let runtime = scratch_runtime.get_or_insert_with(|| {
                            build_scheduler_scratch_runtime(Arc::clone(&state), "", debug_accum)
                                .unwrap_or_else(|| {
                                    lisp_host::scheduler_scratch_runtime_with_fallbacks(
                                        Arc::clone(&state),
                                        0,
                                        0,
                                    )
                                })
                        });
                        for seq in &published {
                            if seq.graph.is_some() {
                                continue; // graph-mode entries reconcile below, not as ticks
                            }
                            runtime.register_published_sequencer(
                                seq.id,
                                seq.name.clone(),
                                crate::sequencer::Timebase::from_index(seq.resolution as u32),
                                seq.tick_source.clone(),
                            );
                        }
                    }
                    scratch_source_version = latest_scratch_source_version;
                    published_sequencers_version = latest_published_sequencers_version;
                    published_process_authoring_version =
                        latest_published_process_authoring_version;
                    let generator_defs = scratch_runtime
                        .as_ref()
                        .map(|runtime| runtime.sequencer_defs())
                        .unwrap_or_default();
                    lookahead_state.generator_runtime.sync_definitions(&generator_defs, lookahead_state.clock.total_beats);
                    let process_authoring = scratch_runtime
                        .as_ref()
                        .map(|runtime| runtime.process_authoring_snapshot())
                        .unwrap_or_default();
                    let process_authoring = crate::process::merge_authoring_snapshots(
                        process_authoring,
                        published_process_authoring.to_runtime(),
                    );
                    lookahead_state
                        .process_runtime
                        .sync_authoring(process_authoring, lookahead_state.clock.total_beats);

                    let new_manifests: Vec<crate::graph::GraphManifest> =
                        published.iter().filter_map(|s| s.graph.clone()).collect();
                    reconcile_graph_runtimes(
                        new_manifests,
                        &snapshot.graph_overrides,
                        &mut lookahead_state.graph_runtimes,
                        &mut lookahead_state.graph_manifests,
                        lookahead_state.clock.total_beats,
                    );
                    publish_graph_visualizations(
                        &state,
                        &lookahead_state.graph_runtimes,
                        lookahead_state.clock.total_beats,
                    );
                    if debug_graph {
                        eprintln!(
                            "[graph-reconcile] published={} graph_manifests={} runtimes={} overrides={}",
                            published.len(),
                            lookahead_state.graph_manifests.len(),
                            lookahead_state.graph_runtimes.len(),
                            snapshot.graph_overrides.len()
                        );
                    }
                    loaded_graph_overrides = Some(snapshot.graph_overrides.clone());
                }

                if loaded_graph_overrides.as_ref() != Some(&snapshot.graph_overrides) {
                    reconcile_graph_runtimes(
                        lookahead_state.graph_manifests.clone(),
                        &snapshot.graph_overrides,
                        &mut lookahead_state.graph_runtimes,
                        &mut lookahead_state.graph_manifests,
                        lookahead_state.clock.total_beats,
                    );
                    publish_graph_visualizations(
                        &state,
                        &lookahead_state.graph_runtimes,
                        lookahead_state.clock.total_beats,
                    );
                    loaded_graph_overrides = Some(snapshot.graph_overrides.clone());
                }

                if !playing
                    && (last_playing
                        || last_pattern != pattern
                        || last_pattern_epoch != pattern_epoch
                        || last_topology_epoch != topology_epoch)
                {
                    queue.clear();
                    lookahead_state.midi_fx_quantizer_state.reset();
                    lookahead_state.process_runtime.clear_scene_pending();
                }

                drain_live_keyboard_inputs(
                    &live_keyboard_rx,
                    &snapshot,
                    rendered,
                    &mut live_midi_fx_tracks,
                );
                let live_midi_fx_active = any_live_midi_fx_notes(&live_midi_fx_tracks);
                if live_midi_fx_active != last_live_midi_fx_active {
                    let previous_scheduled_until = scheduled_until_sample;
                    queue.clear();
                    lookahead_state.midi_fx_quantizer_state.reset();
                    scheduled_until_sample = rendered;
                    if playing {
                        lookahead_state.clock.seek_to_rendered_position(
                            &snapshot,
                            rendered,
                            previous_scheduled_until,
                        );
                        lookahead_state
                            .process_runtime
                            .reset_transport(lookahead_state.clock.total_beats);
                    } else {
                        lookahead_state.process_runtime.reset_transport(0.0);
                    }
                    last_live_midi_fx_active = live_midi_fx_active;
                }
                let samples_per_quarter =
                    sample_rate as f64 * 60.0 / snapshot.transport.bpm.max(1) as f64;
                if should_reload_neural_runtime(
                    &loaded_neural_networks,
                    &snapshot.neural_networks,
                    last_pattern,
                    pattern,
                )
                {
                    lookahead_state.neural_runtime.load_from_networks(&snapshot.neural_networks, lookahead_state.clock.total_beats);
                    loaded_neural_networks = Some(snapshot.neural_networks.clone());
                    state.set_neural_visualization(lookahead_state.neural_runtime.visualization_snapshot());
                }
                let scheduled_ahead_beats =
                    scheduled_until_sample.saturating_sub(rendered) as f64 / samples_per_quarter;
                let rendered_total_beats = (lookahead_state.clock.total_beats - scheduled_ahead_beats).max(0.0);
                // Publish the launch-deadline beat clock so the control
                // thread can stamp immediate launches with a
                // scheduler-derived audible beat (song capture, spec 8.2).
                state.set_scheduler_rendered_beats(rendered_total_beats);
                state.quantized_launches().process_scheduler(
                    &mut lookahead_state.quantized_launches,
                    rendered_total_beats,
                    playing,
                );
                if !playing {
                    let live_active = schedule_live_midi_fx(
                        scratch_runtime.as_mut(),
                        &state,
                        &snapshot,
                        &queue,
                        pattern_epoch,
                        rendered,
                        rendered_total_beats,
                        lookahead_target_samples,
                        sample_rate,
                        &mut live_midi_fx_tracks,
                        debug_accum,
                    );
                    lookahead_state.clock.reset();
                    // Keep an installed song but rewind it to its start so a
                    // later Play begins the song from its start position.
                    if let Some(song) = lookahead_state.song.as_mut() {
                        song.reset();
                    }
                    scheduled_until_sample = rendered;
                    last_playing = false;
                    last_pattern = pattern;
                    last_pattern_epoch = pattern_epoch;
                    last_topology_epoch = topology_epoch;
                    lookahead_state.pending_accum_reset = [false; MAX_TRACKS];
                    lookahead_state.accumulator_states = [AccumulatorRuntimeState::default(); MAX_TRACKS];
                    lookahead_state.midi_fx_quantizer_state.reset();
                    lookahead_state.neural_runtime.reset_state(0.0);
                    lookahead_state.generator_runtime.reset(0.0);
                    lookahead_state.process_runtime.reset_transport(0.0);
                    for graph in &mut lookahead_state.graph_runtimes {
                        graph.reset(0.0);
                    }
                    publish_graph_visualizations(&state, &lookahead_state.graph_runtimes, 0.0);
                    state.set_neural_visualization(lookahead_state.neural_runtime.visualization_snapshot());
                    thread::sleep(Duration::from_millis(if live_active { 1 } else { 2 }));
                    continue;
                }

                if topology_edit_in_flight {
                    queue.clear();
                    lookahead_state.midi_fx_quantizer_state.reset();
                    lookahead_state.process_runtime.clear_scene_pending();
                    // Freeze future scheduling while the topology edit is in
                    // flight, but preserve the clock's current musical phase
                    // so resuming after the edit does not jump backwards.
                    scheduled_until_sample = rendered;
                    lookahead_state.pending_accum_reset = [true; MAX_TRACKS];
                    lookahead_state.neural_runtime.reset_state(lookahead_state.clock.total_beats);
                    state.set_neural_visualization(lookahead_state.neural_runtime.visualization_snapshot());
                    if ready_edit < requested_edit {
                        state
                            .transport
                            .topology_edit_ready_id
                            .store(requested_edit, Ordering::Release);
                    }
                    if applied_edit < requested_edit {
                        thread::sleep(Duration::from_millis(1));
                        continue;
                    }
                }

                if reset_all {
                    lookahead_state.midi_fx_quantizer_state.reset();
                    for track_idx in 0..MAX_TRACKS {
                        lookahead_state.pending_accum_reset[track_idx] = false;
                        if let Some(def) = ACCUMULATOR_REGISTRY.get(
                            snapshot
                                .tracks
                                .get(track_idx)
                                .map(|t| t.params.accumulator_idx)
                                .unwrap_or(0),
                        ) {
                            lookahead_state.accumulator_states[track_idx] = AccumulatorRuntimeState {
                                value: def.reset_value,
                                reversed: false,
                            };
                        } else {
                            lookahead_state.accumulator_states[track_idx] = AccumulatorRuntimeState::default();
                        }
                    }
                }
                if reset_tracks.iter().any(|reset| *reset) {
                    lookahead_state.midi_fx_quantizer_state.reset();
                }
                for track_idx in 0..MAX_TRACKS {
                    if !reset_tracks[track_idx] {
                        continue;
                    }
                    lookahead_state.pending_accum_reset[track_idx] = false;
                    if let Some(def) = ACCUMULATOR_REGISTRY.get(
                        snapshot
                            .tracks
                            .get(track_idx)
                            .map(|t| t.params.accumulator_idx)
                            .unwrap_or(0),
                    ) {
                        lookahead_state.accumulator_states[track_idx] = AccumulatorRuntimeState {
                            value: def.reset_value,
                            reversed: false,
                        };
                    } else {
                        lookahead_state.accumulator_states[track_idx] = AccumulatorRuntimeState::default();
                    }
                }

                if !last_playing {
                    queue.clear();
                    lookahead_state.clock.reset();
                    lookahead_state.midi_fx_quantizer_state.reset();
                    scheduled_until_sample = rendered;
                    lookahead_state.pending_accum_reset = [true; MAX_TRACKS];
                    lookahead_state.neural_runtime.reset_state(lookahead_state.clock.total_beats);
                    lookahead_state.process_runtime.reset_transport(0.0);
                    state.set_neural_visualization(lookahead_state.neural_runtime.visualization_snapshot());
                } else if last_topology_epoch != topology_epoch {
                    let previous_scheduled_until = scheduled_until_sample;
                    queue.clear();
                    lookahead_state.midi_fx_quantizer_state.reset();
                    lookahead_state.clock.seek_to_rendered_position(&snapshot, rendered, previous_scheduled_until);
                    scheduled_until_sample = rendered;
                    lookahead_state.pending_accum_reset = [true; MAX_TRACKS];
                    lookahead_state.neural_runtime.reset_state(lookahead_state.clock.total_beats);
                    lookahead_state
                        .process_runtime
                        .reset_transport(lookahead_state.clock.total_beats);
                    state.set_neural_visualization(lookahead_state.neural_runtime.visualization_snapshot());
                } else if !song_playback_active && last_pattern_epoch != pattern_epoch {
                    // Track topology edits bump pattern_epoch without changing the
                    // pattern index. Rebuild the scheduler horizon immediately so
                    // future triggers target the compacted live track layout.
                    // During song playback the scheduler is the launch
                    // authority: the control-side apply_song_row mirror keeps
                    // UI state in sync without invalidating the split
                    // schedule, so pattern/scene resyncs are suppressed here.
                    let previous_scheduled_until = scheduled_until_sample;
                    queue.clear();
                    lookahead_state.midi_fx_quantizer_state.reset();
                    lookahead_state.clock.seek_to_rendered_position(&snapshot, rendered, previous_scheduled_until);
                    scheduled_until_sample = rendered;
                    lookahead_state.pending_accum_reset = [true; MAX_TRACKS];
                    lookahead_state.neural_runtime.reset_state(lookahead_state.clock.total_beats);
                    lookahead_state
                        .process_runtime
                        .reset_transport(lookahead_state.clock.total_beats);
                    state.set_neural_visualization(lookahead_state.neural_runtime.visualization_snapshot());
                } else if !song_playback_active && last_pattern != pattern {
                    // Pattern switches should replace future scheduled content without
                    // disturbing the current musical phase.
                    let previous_scheduled_until = scheduled_until_sample;
                    queue.clear();
                    lookahead_state.midi_fx_quantizer_state.reset();
                    lookahead_state.clock.seek_to_rendered_position(&snapshot, rendered, previous_scheduled_until);
                    scheduled_until_sample = rendered;
                    lookahead_state.pending_accum_reset = [true; MAX_TRACKS];
                    lookahead_state.neural_runtime.reset_state(lookahead_state.clock.total_beats);
                    lookahead_state
                        .process_runtime
                        .reset_transport(lookahead_state.clock.total_beats);
                    state.set_neural_visualization(lookahead_state.neural_runtime.visualization_snapshot());
                }

                schedule_live_midi_fx(
                    scratch_runtime.as_mut(),
                    &state,
                    &snapshot,
                    &queue,
                    pattern_epoch,
                    rendered,
                    rendered_total_beats,
                    lookahead_target_samples,
                    sample_rate,
                    &mut live_midi_fx_tracks,
                    debug_accum,
                );
                let lookahead_result = schedule_playing_lookahead(
                    &mut lookahead_state,
                    &state,
                    &snapshot,
                    &queue,
                    &mut scratch_runtime,
                    &live_midi_fx_tracks,
                    pattern_epoch,
                    rendered,
                    lookahead_target_samples,
                    sample_rate,
                    scheduler_block_size,
                    samples_per_quarter,
                    scheduled_until_sample,
                    debug_accum,
                    debug_graph,
                );
                scheduled_until_sample = lookahead_result.scheduled_until_sample;

                last_playing = playing;
                last_pattern = pattern;
                last_pattern_epoch = pattern_epoch;
                last_topology_epoch = topology_epoch;
                thread::sleep(Duration::from_millis(1));
            }
        });
}
