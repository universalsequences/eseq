/*!
Scheduler worker lifecycle and synchronization with live sequencer state.
*/

#[allow(unused_imports)]
use super::*;

/// With roll mode on, the first lookahead extension after Play is held this
/// long so a roll key pressed "together with" Play — which reaches this
/// thread a UI frame or two later — still lands before beat zero is
/// scheduled and rolls from the first grid line, sample-exact. The whole
/// transport simply starts this much later; nothing is skipped or doubled.
const ROLL_PLAY_START_HOLD: Duration = Duration::from_millis(50);

/// Every process body the scheduler VM may compile into its run cache for
/// this authoring snapshot: class and instance `:run` bodies and listener
/// handlers.
fn process_run_sources(
    authoring: &crate::process::ProcessAuthoringSnapshot,
) -> std::collections::HashSet<String> {
    let defs = authoring.defs.iter().flat_map(|def| {
        def.run_source
            .iter()
            .chain(def.listens.iter().map(|listen| &listen.handler_source))
    });
    let instances = authoring
        .instances
        .iter()
        .filter_map(|instance| instance.run_source.as_ref());
    defs.chain(instances).cloned().collect()
}

/// Reconcile a topology publication without throwing away valid lookahead
/// when queued event meaning is unchanged. Track appends join at the current
/// frontier and rack slot changes are resolved from the latest audio snapshot;
/// destructive changes still rebuild from the render head.
pub(super) fn reconcile_playing_topology_change<const CAPACITY: usize>(
    scheduler: &mut SchedulerLookaheadState,
    state: &Arc<SequencerState>,
    snapshot: &SequencerSnapshot,
    queue: &ScheduledEventQueue<CAPACITY>,
    rendered: u64,
    scheduled_until_sample: &mut u64,
    previous_num_tracks: usize,
    previous_pattern_epoch: u64,
) {
    let event_compatible = snapshot.transport.num_tracks >= previous_num_tracks
        && snapshot.transport.pattern_epoch == previous_pattern_epoch;
    if event_compatible {
        for track in previous_num_tracks..snapshot.transport.num_tracks.min(MAX_TRACKS) {
            scheduler.pending_accum_reset[track] = true;
        }
        return;
    }

    let previous_scheduled_until = *scheduled_until_sample;
    queue.clear();
    scheduler.midi_fx_quantizer_state.reset();
    scheduler
        .clock
        .seek_to_rendered_position(snapshot, rendered, previous_scheduled_until);
    *scheduled_until_sample = rendered;
    scheduler.pending_accum_reset = [true; MAX_TRACKS];
    scheduler
        .neural_runtime
        .reset_state(scheduler.clock.total_beats);
    scheduler
        .process_runtime
        .reset_transport(scheduler.clock.total_beats);
    state.set_neural_visualization(scheduler.neural_runtime.visualization_snapshot());
}

pub(super) fn topology_edit_frontier_drained(
    rendered_sample: u64,
    scheduled_until_sample: u64,
) -> bool {
    rendered_sample >= scheduled_until_sample
}

/// The driver owns scheduler state; the caller owns pacing and the render cursor.
/// No sleeping or implicit wall-clock reads occur during advancement.
pub(crate) struct SchedulerDriver {
    state: Arc<SequencerState>,
    queue: Arc<ScheduledEventQueue<4096>>,
    sample_rate: u32,
    scheduler_block_size: usize,
    lookahead_state: SchedulerLookaheadState,
    scheduled_until_sample: u64,
    last_pattern: usize,
    last_pattern_epoch: u64,
    last_topology_epoch: u64,
    last_num_tracks: usize,
    last_playing: bool,
    roll_play_hold: Option<std::time::Instant>,
    audition: audition::AuditionPlayer,
    live_midi_fx_tracks: [LiveMidiFxTrackState; MAX_TRACKS],
    loaded_graph_overrides: Option<Vec<crate::graph::ProjectGraphOverrides>>,
    loaded_neural_networks: Option<Vec<crate::neural::ProjectNeuralNetwork>>,
    last_live_midi_fx_active: bool,
    scratch_source_version: u64,
    published_sequencers_version: u64,
    published_process_authoring_version: u64,
    scratch_runtime: Option<lisp_host::ScratchControlRuntime>,
    /// Process body sources (run bodies and listener handlers) in the
    /// authoring snapshot last synced into `process_runtime`. The scheduler
    /// VM caches one compiled callback per distinct body; when a republish
    /// drops a body from this set its callback is stale and the cache is
    /// dropped, so live-edited bodies do not accumulate for the session.
    process_run_sources: std::collections::HashSet<String>,
    /// How many times `advance` rebuilt the scheduler VM.
    #[cfg(test)]
    pub(super) scratch_runtime_builds: u32,
    /// How many process-only republishes dropped the VM's run cache.
    #[cfg(test)]
    pub(super) process_run_cache_clears: u32,
    debug_accum: bool,
    debug_graph: bool,
    runtime_errors: Vec<String>,
}

#[derive(Clone, Copy)]
pub(crate) enum SchedulerInput<'a> {
    Live {
        keyboard: &'a mpsc::Receiver<KeyboardTrigger>,
        clock: &'a dyn Fn() -> std::time::Instant,
    },
    Offline,
}

impl SchedulerInput<'_> {
    fn now(self) -> Option<std::time::Instant> {
        match self { Self::Live { clock, .. } => Some(clock()), Self::Offline => None }
    }
}

pub(crate) struct SchedulerAdvance {
    pub scheduled_until_sample: u64,
    pub poll_after: Duration,
    pub queue_rejections: u64,
}

impl SchedulerDriver {
    pub(crate) fn new(
        state: Arc<SequencerState>,
        sample_rate: u32,
        scheduler_block_size: usize,
        queue: Arc<ScheduledEventQueue<4096>>,
    ) -> Self {
        assert!(sample_rate > 0 && scheduler_block_size > 0);
        Self {
            state, queue, sample_rate, scheduler_block_size,
            lookahead_state: SchedulerLookaheadState::new(sample_rate),
            scheduled_until_sample: 0,
            last_pattern: usize::MAX,
            last_pattern_epoch: u64::MAX,
            last_topology_epoch: u64::MAX,
            last_num_tracks: usize::MAX,
            last_playing: false,
            roll_play_hold: None,
            audition: audition::AuditionPlayer::default(),
            live_midi_fx_tracks: std::array::from_fn(|_| LiveMidiFxTrackState::default()),
            loaded_graph_overrides: None,
            loaded_neural_networks: None,
            last_live_midi_fx_active: false,
            scratch_source_version: u64::MAX,
            published_sequencers_version: u64::MAX,
            published_process_authoring_version: u64::MAX,
            scratch_runtime: None,
            process_run_sources: std::collections::HashSet::new(),
            #[cfg(test)]
            scratch_runtime_builds: 0,
            #[cfg(test)]
            process_run_cache_clears: 0,
            runtime_errors: Vec::new(),
            debug_accum: std::env::var_os("TINYSEQ_DEBUG_ACCUM").is_some(),
            debug_graph: std::env::var_os("TINYSEQ_DEBUG_GRAPH").is_some(),
        }
    }

    pub(crate) fn take_runtime_errors(&mut self) -> Vec<String> {
        if let Some(error) = self.scratch_runtime.as_mut().and_then(|runtime| runtime.take_invocation_error()) {
            self.runtime_errors.push(error);
        }
        std::mem::take(&mut self.runtime_errors)
    }

    fn poll_after(&self, millis: u64) -> SchedulerAdvance {
        SchedulerAdvance {
            scheduled_until_sample: self.scheduled_until_sample,
            poll_after: Duration::from_millis(millis),
            queue_rejections: self.queue.rejected_events(),
        }
    }

    /// Complete lookahead up to the requested absolute frontier, unless the
    /// transport is stopped, the song ends, or a live topology/roll handoff
    /// needs another poll. Callers must inspect the returned frontier before
    /// rendering: a retry delay is advisory and never sleeps this thread.
    pub(crate) fn advance(
        &mut self,
        rendered: u64,
        horizon: u64,
        input: SchedulerInput<'_>,
    ) -> SchedulerAdvance {
        self.runtime_errors.clear();
        if let Some(runtime) = self.scratch_runtime.as_mut() { runtime.take_invocation_error(); }
        let state = &self.state;
        let queue = &self.queue;
        let sample_rate = self.sample_rate;
        let scheduler_block_size = self.scheduler_block_size;
        let lookahead_target_samples = horizon.saturating_sub(rendered);
        let debug_accum = self.debug_accum;
        let debug_graph = self.debug_graph;
        // Free whatever the audio callback retired instead of dropping
        // on its own thread (bead eseq-sj01). This loop runs every
        // 1-2 ms, so the retire ring stays shallow even when nothing
        // publishes.
        state.drain_retired_scheduler_snapshots();
        let snapshot = state.latest_scheduler_snapshot();
        let playing = snapshot.transport.playing;
        let pattern = snapshot.transport.current_pattern;
        let pattern_epoch = snapshot.transport.pattern_epoch;
        let topology_epoch = snapshot.transport.topology_epoch;
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
                            self.lookahead_state.song = Some(runtime);
                            queue.clear();
                            self.lookahead_state.clock.reset();
                            self.lookahead_state.midi_fx_quantizer_state.reset();
                            self.scheduled_until_sample = rendered;
                            self.lookahead_state.pending_accum_reset = [true; MAX_TRACKS];
                            self.lookahead_state.neural_runtime.reset_state(0.0);
                            self.lookahead_state.generator_runtime.reset(0.0);
                            self.lookahead_state.process_runtime.reset_transport(0.0);
                            self.lookahead_state.process_runtime.lock_holds.clear();
                            for graph in &mut self.lookahead_state.graph_runtimes {
                                graph.reset_transport(0.0);
                            }
                        }
                        Err(error) => {
                            self.runtime_errors.push(format!("song start: {error}"));
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
                    if let Some(runtime) = self.lookahead_state.song.as_mut() {
                        runtime.replace_song_in_place(song);
                    }
                }
                crate::sequencer::SongPlaybackCommand::Rebuild { song } => {
                    let clock_beats = self.lookahead_state.clock.total_beats;
                    if let Some(runtime) = self.lookahead_state.song.as_mut() {
                        runtime.rebuild_song(song, clock_beats);
                    }
                }
                crate::sequencer::SongPlaybackCommand::Stop => {
                    self.lookahead_state.song = None;
                    state.song_playback().clear_position();
                }
            }
        }
        let song_playback_active = self.lookahead_state.song.is_some();

        let latest_published_sequencers_version = state.published_sequencers_version();
        let latest_published_process_authoring_version =
            state.published_process_authoring_version();
        let process_authoring_changed = latest_published_process_authoring_version
            != self.published_process_authoring_version;
        // Only the scratch source and the published generators live in the
        // scheduler VM. Process bodies ship as source in the authoring
        // snapshot and compile lazily into the VM's run cache, so a
        // process-only republish (a panel re-declaring a track's chain on
        // every slider tick) keeps the VM and its cache: rebuilding it
        // re-evaluates the builtin libraries on this thread, which outlasts
        // the lookahead and plays the next hits late.
        let rebuild_runtime = latest_scratch_source_version != self.scratch_source_version
            || latest_published_sequencers_version != self.published_sequencers_version
            || (process_authoring_changed && self.scratch_runtime.is_none());
        if rebuild_runtime {
            #[cfg(test)]
            {
                self.scratch_runtime_builds += 1;
            }
            let user_source = state.scratch_source();
            if debug_accum {
                eprintln!(
                    "[accum] scratch source version {} -> {} bytes={}",
                    self.scratch_source_version,
                    latest_scratch_source_version,
                    user_source.len()
                );
            }
            let (runtime, errors) =
                build_scheduler_scratch_runtime(Arc::clone(state), &user_source, debug_accum);
            self.scratch_runtime = runtime;
            self.runtime_errors.extend(errors);
            if debug_accum {
                if let Some(runtime) = self.scratch_runtime.as_ref() {
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
                let runtime = self.scratch_runtime.get_or_insert_with(|| {
                    build_scheduler_scratch_runtime(Arc::clone(state), "", debug_accum).0
                        .unwrap_or_else(|| {
                            lisp_host::scheduler_scratch_runtime_with_fallbacks(
                                Arc::clone(state),
                                0,
                                0,
                            )
                        })
                });
                self.lookahead_state.generator_owner_racks = published
                    .iter()
                    .filter(|seq| seq.graph.is_none())
                    .filter_map(|seq| seq.owner_rack.map(|group_id| (seq.id, group_id)))
                    .collect();
                self.lookahead_state.generator_before_steps = published
                    .iter()
                    .filter(|seq| seq.graph.is_none() && seq.before_steps)
                    .map(|seq| seq.id)
                    .collect();
                for seq in &published {
                    if seq.graph.is_some() {
                        continue; // graph-mode entries reconcile below, not as ticks
                    }
                    if let Err(error) = runtime.register_published_sequencer(
                        seq.id,
                        seq.name.clone(),
                        crate::sequencer::Timebase::from_index(seq.resolution as u32),
                        seq.tick_source.clone(),
                        &seq.requires,
                    ) {
                        self.runtime_errors.push(format!("sequencer {:?} ({}): {error}", seq.name, seq.id));
                        eprintln!(
                            "failed to register published sequencer {:?} ({}): {error}",
                            seq.name, seq.id
                        );
                    } else if let Err(error) =
                        runtime.set_sequencer_trigger(seq.id, seq.trigger_source.as_deref())
                    {
                        self.runtime_errors.push(format!("sequencer {:?} ({}): {error}", seq.name, seq.id));
                        eprintln!("failed to register sequencer trigger {:?} ({}): {error}", seq.name, seq.id);
                        state.report_generator_tick_error(seq.id, seq.name.clone(), error);
                    }
                }
            }
            self.scratch_source_version = latest_scratch_source_version;
            self.published_sequencers_version = latest_published_sequencers_version;
            let generator_defs = self.scratch_runtime
                .as_ref()
                .map(|runtime| runtime.sequencer_defs())
                .unwrap_or_default();
            self.lookahead_state.generator_runtime.sync_definitions(&generator_defs, self.lookahead_state.clock.total_beats);
            self.lookahead_state
                .process_runtime
                .lock_holds
                .retain_generators(|id| generator_defs.iter().any(|def| def.id == id));
            // Edited/republished definitions get a fresh tick attempt
            // (eseq-85a.5): the parked-on-error set is scoped to one
            // runtime build.
            self.lookahead_state.parked_generators.clear();
            self.lookahead_state.gate_conflicts_reported.clear();

            let new_manifests: Vec<crate::graph::GraphManifest> =
                published.iter().filter_map(|s| s.graph.clone()).collect();
            reconcile_graph_runtimes(
                new_manifests,
                &snapshot.graph_overrides,
                &snapshot.rack_memberships,
                &mut self.lookahead_state.graph_runtimes,
                &mut self.lookahead_state.graph_manifests,
                self.lookahead_state.clock.total_beats,
            );
            publish_graph_visualizations(
                state,
                &self.lookahead_state.graph_runtimes,
                self.lookahead_state.clock.total_beats,
            );
            if debug_graph {
                eprintln!(
                    "[graph-reconcile] published={} graph_manifests={} runtimes={} overrides={}",
                    published.len(),
                    self.lookahead_state.graph_manifests.len(),
                    self.lookahead_state.graph_runtimes.len(),
                    snapshot.graph_overrides.len()
                );
            }
            self.loaded_graph_overrides = Some(snapshot.graph_overrides.clone());
            self.lookahead_state.graph_overrides_applied = Some(snapshot.graph_overrides.clone());
        }
        if rebuild_runtime || process_authoring_changed {
            self.published_process_authoring_version =
                latest_published_process_authoring_version;
            let process_authoring = self.scratch_runtime
                .as_ref()
                .map(|runtime| runtime.process_authoring_snapshot())
                .unwrap_or_default();
            let process_authoring = crate::process::merge_authoring_snapshots(
                process_authoring,
                state.published_process_authoring().to_runtime(),
            );
            let run_sources = process_run_sources(&process_authoring);
            // A rebuilt VM starts with an empty run cache. On a kept VM, a
            // body that left the snapshot (an edited def-process, a replaced
            // expr class) would otherwise stay compiled forever. Unchanged
            // bodies, as on a panel's per-tick chain republish, keep the
            // cache warm.
            if !rebuild_runtime
                && !self.process_run_sources.is_subset(&run_sources)
            {
                if let Some(runtime) = self.scratch_runtime.as_mut() {
                    // `eval` drops every cached process callback before it
                    // evaluates; `nil` itself does nothing.
                    if let Err(error) = runtime.eval("nil") {
                        self.runtime_errors
                            .push(format!("process run cache reset: {error}"));
                    }
                    #[cfg(test)]
                    {
                        self.process_run_cache_clears += 1;
                    }
                }
            }
            self.process_run_sources = run_sources;
            self.lookahead_state
                .process_runtime
                .sync_authoring(process_authoring, self.lookahead_state.clock.total_beats);
        }

        if self.loaded_graph_overrides.as_ref() != Some(&snapshot.graph_overrides) {
            reconcile_graph_runtimes(
                self.lookahead_state.graph_manifests.clone(),
                &snapshot.graph_overrides,
                &snapshot.rack_memberships,
                &mut self.lookahead_state.graph_runtimes,
                &mut self.lookahead_state.graph_manifests,
                self.lookahead_state.clock.total_beats,
            );
            publish_graph_visualizations(
                state,
                &self.lookahead_state.graph_runtimes,
                self.lookahead_state.clock.total_beats,
            );
            self.loaded_graph_overrides = Some(snapshot.graph_overrides.clone());
            self.lookahead_state.graph_overrides_applied = Some(snapshot.graph_overrides.clone());
        }

        let graph_control_commands = state.drain_graph_control_commands();
        if !graph_control_commands.is_empty() {
            apply_graph_control_commands(
                &mut self.lookahead_state.graph_runtimes,
                &graph_control_commands,
            );
            publish_graph_visualizations(
                state,
                &self.lookahead_state.graph_runtimes,
                self.lookahead_state.clock.total_beats,
            );
        }

        if !playing
            && (self.last_playing
                || self.last_pattern != pattern
                || self.last_pattern_epoch != pattern_epoch
                || self.last_topology_epoch != topology_epoch)
        {
            queue.clear();
            self.lookahead_state.midi_fx_quantizer_state.reset();
            self.lookahead_state.process_runtime.clear_scene_pending();
        }

        if let SchedulerInput::Live { keyboard, .. } = input {
            drain_live_keyboard_inputs(
                keyboard, &snapshot, rendered, &mut self.live_midi_fx_tracks,
            );
        }
        // Roll commands are applied before the lookahead pass extends
        // the schedule, so a NoteOff cancels every roll hit not yet
        // inside the horizon (docs/rolling-core-spec.md 4.2, F3).
        let roll_commands = state.drain_roll_commands();
        if !roll_commands.is_empty() {
            // Manual sequence-roll commands and ClearAll win over a running
            // process roll (`roll!`).
            self.lookahead_state
                .roll
                .cancel_process_roll_for_commands(&roll_commands, state);
            // The clock frontier leads the render head by the scheduled
            // lookahead (plus any early-groove discovery lead); a captured
            // sequence-roll window anchors on what is audible now.
            let frontier_lag_beats = if playing {
                self.scheduled_until_sample.saturating_sub(rendered) as f64
                    / (sample_rate as f64 * 60.0 / snapshot.transport.bpm.max(1) as f64)
            } else {
                0.0
            };
            self.lookahead_state.roll.apply_commands_with_clock_lagged(
                &roll_commands,
                &mut self.lookahead_state.clock,
                &snapshot,
                frontier_lag_beats,
            );
            let grid = self.lookahead_state.roll.active_grid_beats(state);
            self.lookahead_state.roll.publish_windows(state, grid);
        }
        if let Err(error) = self.audition.advance(state, &snapshot, rendered, horizon,
            sample_rate, scheduler_block_size, self.scratch_runtime.as_mut()) {
            state.note_audition.report_error(error.clone());
            self.runtime_errors.push(error);
        }
        let live_midi_fx_active = any_live_midi_fx_notes(&self.live_midi_fx_tracks);
        if live_midi_fx_active != self.last_live_midi_fx_active {
            let previous_scheduled_until = self.scheduled_until_sample;
            queue.clear();
            self.lookahead_state.midi_fx_quantizer_state.reset();
            self.scheduled_until_sample = rendered;
            if playing {
                self.lookahead_state.clock.seek_to_rendered_position(
                    &snapshot,
                    rendered,
                    previous_scheduled_until,
                );
                self.lookahead_state
                    .process_runtime
                    .reset_transport(self.lookahead_state.clock.total_beats);
            } else {
                self.lookahead_state.process_runtime.reset_transport(0.0);
            }
            self.last_live_midi_fx_active = live_midi_fx_active;
        }
        let samples_per_quarter =
            sample_rate as f64 * 60.0 / snapshot.transport.bpm.max(1) as f64;
        if should_reload_neural_runtime(
            &self.loaded_neural_networks,
            &snapshot.neural_networks,
            self.last_pattern,
            pattern,
        )
        {
            self.lookahead_state.neural_runtime.load_from_networks(&snapshot.neural_networks, self.lookahead_state.clock.total_beats);
            self.loaded_neural_networks = Some(snapshot.neural_networks.clone());
            state.set_neural_visualization(self.lookahead_state.neural_runtime.visualization_snapshot());
        }
        let scheduled_ahead_beats =
            self.scheduled_until_sample.saturating_sub(rendered) as f64 / samples_per_quarter;
        let rendered_total_beats = (self.lookahead_state.clock.total_beats - scheduled_ahead_beats).max(0.0);
        // Publish the launch-deadline beat clock so the control
        // thread can stamp immediate launches with a
        // scheduler-derived audible beat (song capture, spec 8.2).
        state.set_scheduler_rendered_beats(rendered_total_beats);
        state.quantized_launches().process_scheduler(
            &mut self.lookahead_state.quantized_launches,
            rendered_total_beats,
            // Boundary-launch deadlines quantize against the
            // scheduling frontier so the chunk split always lands on
            // a not-yet-scheduled boundary.
            self.lookahead_state.clock.total_beats,
            playing,
            song_playback_active,
        );
        if !playing {
            let clear_graph_deltas = self.last_playing
                || self.last_pattern != pattern
                || self.last_pattern_epoch != pattern_epoch
                || self.last_topology_epoch != topology_epoch;
            let live_active = schedule_live_midi_fx(
                self.scratch_runtime.as_mut(),
                state,
                &snapshot,
                queue,
                pattern_epoch,
                rendered,
                rendered_total_beats,
                lookahead_target_samples,
                sample_rate,
                &mut self.live_midi_fx_tracks,
                debug_accum,
            );
            self.lookahead_state.clock.reset();
            // Keep an installed song but rewind it to its start so a
            // later Play begins the song from its start position.
            if let Some(song) = self.lookahead_state.song.as_mut() {
                song.reset();
            }
            self.scheduled_until_sample = rendered;
            // Transport STOP clears held rolls (rolling-core-spec 7)
            // — on the playing→stopped transition only. While parked,
            // roll keys pressed with roll mode on stay armed, so a
            // press-then-play starts rolling exactly on beat one.
            if self.last_playing {
                self.lookahead_state.roll.cancel_process_roll(state, true);
                // Stop queues ClearAll at the input boundary. A new manual
                // hold after it may already have been applied above; keep
                // that fresh gesture armed for the next Play.
                if !state.has_sequence_roll_holds() {
                    self.lookahead_state.roll.clear_all();
                }
            }
            self.roll_play_hold = None;
            self.last_playing = false;
            self.last_pattern = pattern;
            self.last_pattern_epoch = pattern_epoch;
            self.last_topology_epoch = topology_epoch;
            self.last_num_tracks = snapshot.transport.num_tracks;
            self.lookahead_state.pending_accum_reset = [false; MAX_TRACKS];
            self.lookahead_state.accumulator_states = [AccumulatorRuntimeState::default(); MAX_TRACKS];
            self.lookahead_state.midi_fx_quantizer_state.reset();
            self.lookahead_state.neural_runtime.reset_state(0.0);
            self.lookahead_state.generator_runtime.reset(0.0);
            self.lookahead_state.process_runtime.reset_transport(0.0);
            self.lookahead_state.process_runtime.lock_holds.clear();
            for graph in &mut self.lookahead_state.graph_runtimes {
                if clear_graph_deltas {
                    graph.reset_transport(0.0);
                } else {
                    graph.rewind(0.0);
                }
            }
            publish_graph_visualizations(state, &self.lookahead_state.graph_runtimes, 0.0);
            state.set_neural_visualization(self.lookahead_state.neural_runtime.visualization_snapshot());
            return self.poll_after(if live_active { 1 } else { 2 });
        }

        // Roll-armed play start: pin the scheduling frontier to the
        // render head for ROLL_PLAY_START_HOLD so roll commands racing
        // the Play toggle are drained before the first chunk maps out
        // beat zero. Commands, song-playback handover and launch
        // deadlines above keep running every held iteration.
        if !self.last_playing
            && self.roll_play_hold.is_none()
            && input.now().is_some()
            && state.transport.roll_mode.load(Ordering::Relaxed)
        {
            self.roll_play_hold = input.now().map(|now| now + ROLL_PLAY_START_HOLD);
        }
        if let Some(hold_until) = self.roll_play_hold {
            if input.now().is_some_and(|now| now < hold_until) {
                self.scheduled_until_sample = rendered;
                return self.poll_after(1);
            }
            self.roll_play_hold = None;
        }

        if topology_edit_in_flight && applied_edit < requested_edit {
            // Stop extending lookahead, but let the audio callback
            // consume everything already scheduled. The edit becomes
            // safe exactly at that frontier: no stale track indices
            // remain to clear, and playback reaches the handoff without
            // a scheduler-created hole.
            self.lookahead_state.midi_fx_quantizer_state.reset();
            self.lookahead_state.process_runtime.clear_scene_pending();
            self.lookahead_state.pending_accum_reset = [true; MAX_TRACKS];
            if !topology_edit_frontier_drained(rendered, self.scheduled_until_sample) {
                return self.poll_after(1);
            }
            if ready_edit < requested_edit {
                state
                    .transport
                    .topology_edit_ready_id
                    .store(requested_edit, Ordering::Release);
            }
            return self.poll_after(1);
        }

        if reset_all {
            self.lookahead_state.midi_fx_quantizer_state.reset();
            self.lookahead_state.process_runtime.reset_step_process_states();
            for track_idx in 0..MAX_TRACKS {
                self.lookahead_state.pending_accum_reset[track_idx] = false;
                if let Some(def) = ACCUMULATOR_REGISTRY.get(
                    snapshot
                        .tracks
                        .get(track_idx)
                        .map(|t| t.params.accumulator_idx)
                        .unwrap_or(0),
                ) {
                    self.lookahead_state.accumulator_states[track_idx] = AccumulatorRuntimeState {
                        value: def.reset_value,
                        reversed: false,
                    };
                } else {
                    self.lookahead_state.accumulator_states[track_idx] = AccumulatorRuntimeState::default();
                }
            }
        }
        if reset_tracks.iter().any(|reset| *reset) {
            self.lookahead_state.midi_fx_quantizer_state.reset();
        }
        for track_idx in 0..MAX_TRACKS {
            if !reset_tracks[track_idx] {
                continue;
            }
            self.lookahead_state.pending_accum_reset[track_idx] = false;
            if let Some(def) = ACCUMULATOR_REGISTRY.get(
                snapshot
                    .tracks
                    .get(track_idx)
                    .map(|t| t.params.accumulator_idx)
                    .unwrap_or(0),
            ) {
                self.lookahead_state.accumulator_states[track_idx] = AccumulatorRuntimeState {
                    value: def.reset_value,
                    reversed: false,
                };
            } else {
                self.lookahead_state.accumulator_states[track_idx] = AccumulatorRuntimeState::default();
            }
        }

        // A published pattern change is observed exactly once, before
        // the resync chain below and independently of which branch it
        // takes: a boundary launch the scheduler already applied is
        // adopted here (its mirror must not trigger a resync), and any
        // other switch voids the pending adoptions. The scheduler
        // cannot key this off the mailbox ack — the control thread
        // publishes the mirrored snapshot before it acks, and the ack
        // is drained above, a whole iteration before this comparison.
        let pattern_adopted = self.last_pattern != pattern
            && self.lookahead_state
                .quantized_launches
                .observe_pattern_switch(pattern);
        if crate::scheduled_event::launch_trace_enabled()
            && (self.last_pattern != pattern
                || self.last_pattern_epoch != pattern_epoch
                || self.last_topology_epoch != topology_epoch)
        {
            let resync = if !self.last_playing {
                "play-start"
            } else if self.last_topology_epoch != topology_epoch {
                "topology"
            } else if !song_playback_active && self.last_pattern_epoch != pattern_epoch {
                "pattern-epoch"
            } else if !song_playback_active && self.last_pattern != pattern && !pattern_adopted {
                "pattern-switch"
            } else {
                "none"
            };
            eprintln!(
                "[launch] sched-observe pattern {}->{pattern} pattern_epoch {}->{pattern_epoch} topology_epoch {}->{topology_epoch} adopted={pattern_adopted} resync={resync} rendered={rendered} scheduled_until={}",
                self.last_pattern, self.last_pattern_epoch, self.last_topology_epoch,
                self.scheduled_until_sample
            );
        }

        if !self.last_playing {
            queue.clear();
            self.lookahead_state.clock.reset();
            self.lookahead_state.midi_fx_quantizer_state.reset();
            self.scheduled_until_sample = rendered;
            self.lookahead_state.pending_accum_reset = [true; MAX_TRACKS];
            self.lookahead_state.neural_runtime.reset_state(self.lookahead_state.clock.total_beats);
            self.lookahead_state.process_runtime.reset_transport(0.0);
            // Play from stopped restarts every lane/process accumulator,
            // like the legacy per-track accumulator above.
            self.lookahead_state.process_runtime.reset_step_process_states();
            state.set_neural_visualization(self.lookahead_state.neural_runtime.visualization_snapshot());
        } else if self.last_topology_epoch != topology_epoch {
            reconcile_playing_topology_change(
                &mut self.lookahead_state,
                state,
                &snapshot,
                queue,
                rendered,
                &mut self.scheduled_until_sample,
                self.last_num_tracks,
                self.last_pattern_epoch,
            );
        } else if !song_playback_active && self.last_pattern_epoch != pattern_epoch {
            // Destructive topology edits bump pattern_epoch without
            // changing the pattern index. Rebuild the scheduler horizon
            // immediately so future triggers target the rewritten layout.
            // During song playback the scheduler is the launch
            // authority: the control-side apply_song_row mirror keeps
            // UI state in sync without invalidating the split
            // schedule, so pattern/scene resyncs are suppressed here.
            let previous_scheduled_until = self.scheduled_until_sample;
            queue.clear();
            self.lookahead_state.midi_fx_quantizer_state.reset();
            self.lookahead_state.clock.seek_to_rendered_position(&snapshot, rendered, previous_scheduled_until);
            self.scheduled_until_sample = rendered;
            self.lookahead_state.pending_accum_reset = [true; MAX_TRACKS];
            self.lookahead_state.neural_runtime.reset_state(self.lookahead_state.clock.total_beats);
            self.lookahead_state
                .process_runtime
                .reset_transport(self.lookahead_state.clock.total_beats);
            state.set_neural_visualization(self.lookahead_state.neural_runtime.visualization_snapshot());
        } else if !song_playback_active && self.last_pattern != pattern && !pattern_adopted {
            // Pattern switches should replace future scheduled content without
            // disturbing the current musical phase.
            //
            // A pattern index matching an installed boundary launch is
            // excluded above: the scheduler already switched audibly
            // at the boundary via the chunk split and this publish is
            // the control-side mirror — resyncing here would clear
            // the queue and mark the boundary step as already played
            // (the skipped-first-trigger bug).
            let previous_scheduled_until = self.scheduled_until_sample;
            queue.clear();
            // The new pattern's authored lengths govern (`length!`).
            self.lookahead_state.clock.clear_pattern_lengths();
            self.lookahead_state.midi_fx_quantizer_state.reset();
            self.lookahead_state.clock.seek_to_rendered_position(&snapshot, rendered, previous_scheduled_until);
            self.scheduled_until_sample = rendered;
            self.lookahead_state.pending_accum_reset = [true; MAX_TRACKS];
            self.lookahead_state.neural_runtime.reset_state(self.lookahead_state.clock.total_beats);
            self.lookahead_state
                .process_runtime
                .reset_transport(self.lookahead_state.clock.total_beats);
            state.set_neural_visualization(self.lookahead_state.neural_runtime.visualization_snapshot());
        }

        schedule_live_midi_fx(
            self.scratch_runtime.as_mut(),
            state,
            &snapshot,
            queue,
            pattern_epoch,
            rendered,
            rendered_total_beats,
            lookahead_target_samples,
            sample_rate,
            &mut self.live_midi_fx_tracks,
            debug_accum,
        );
        let lookahead_result = schedule_playing_lookahead(
            &mut self.lookahead_state,
            state,
            &snapshot,
            queue,
            &mut self.scratch_runtime,
            &self.live_midi_fx_tracks,
            pattern_epoch,
            rendered,
            lookahead_target_samples,
            sample_rate,
            scheduler_block_size,
            samples_per_quarter,
            self.scheduled_until_sample,
            debug_accum,
            debug_graph,
        );
        self.scheduled_until_sample = lookahead_result.scheduled_until_sample;

        self.last_playing = playing;
        self.last_pattern = pattern;
        self.last_pattern_epoch = pattern_epoch;
        self.last_topology_epoch = topology_epoch;
        self.last_num_tracks = snapshot.transport.num_tracks;
        self.poll_after(1)
    }
}

/// The scheduler has a soft deadline (the lookahead, ~43 ms at 512-frame
/// blocks) but ran at default QoS, so ordinary machine load parked it for
/// tens to hundreds of ms and events reached the callback late. Mark it
/// user-interactive like the audio workers' helper threads; it is not a
/// time-constraint thread because Lisp ticks allocate and may overrun.
/// `ESEQ_SCHED_QOS=0` keeps the default for A/B measurement.
fn raise_scheduler_thread_qos() {
    #[cfg(target_os = "macos")]
    if std::env::var("ESEQ_SCHED_QOS").as_deref() != Ok("0") {
        unsafe {
            libc::pthread_set_qos_class_self_np(libc::qos_class_t::QOS_CLASS_USER_INTERACTIVE, 0);
        }
    }
}

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
            let lookahead_samples = (scheduler_block_size.max(1) * 4) as u64;
            let mut driver = SchedulerDriver::new(state, sample_rate, scheduler_block_size, queue);
            raise_scheduler_thread_qos();
            let mut meter = super::load_meter::LoadMeter::new(sample_rate);
            loop {
                let rendered = rendered_samples.load(Ordering::Acquire);
                meter.begin_pass(rendered, driver.scheduled_until_sample, driver.last_playing);
                let advanced = driver.advance(
                    rendered,
                    rendered.saturating_add(lookahead_samples),
                    SchedulerInput::Live {
                        keyboard: &live_keyboard_rx,
                        clock: &std::time::Instant::now,
                    },
                );
                meter.end_pass(advanced.poll_after);
                thread::sleep(advanced.poll_after);
            }
        });
}

#[cfg(test)]
mod process_run_cache_tests {
    use super::*;
    use crate::sequencer::{default_empty_effect_chain, SequencerState};

    fn step_transposes<const CAPACITY: usize>(
        queue: &ScheduledEventQueue<CAPACITY>,
    ) -> Vec<f32> {
        let mut out = Vec::new();
        while let Some(event) = queue.pop_owned() {
            if let ScheduledEventKind::ResolvedTrigger { resolved, .. } = event.kind {
                out.push(resolved.transpose);
            }
        }
        out
    }

    #[test]
    fn edited_process_body_drops_the_kept_vm_run_cache() {
        std::thread::Builder::new()
            .stack_size(SCHEDULER_THREAD_STACK_SIZE)
            .spawn(|| {
                let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
                state.pattern.track_params[0].set_num_steps(4);
                for step in 0..4 {
                    state.pattern.patterns[0].set_step_active(step, true);
                }
                let mut ui = lisp_host::scratch_runtime_with_fallbacks(Arc::clone(&state), 0, 0);
                let define = |ui: &mut lisp_host::ScratchControlRuntime, run: &str| {
                    ui.eval(&format!(
                        "(def-process shift-test
                           :target (step-param :transpose)
                           :in ((by :float -24 24 :default 0))
                           :run {run})"
                    ))
                    .expect("define process");
                };
                let declare = |ui: &mut lisp_host::ScratchControlRuntime, by: i32| {
                    ui.eval(&format!("(processes :track 0 (shift-test :by {by}))"))
                        .expect("declare chain");
                    state.publish_process_authoring(
                        ui.process_authoring_snapshot().to_published().expect("publishable"),
                    );
                    state.publish_scheduler_snapshot();
                };
                define(&mut ui, "(target-add! (in :by))");
                state.transport.playing.store(true, Ordering::Relaxed);
                declare(&mut ui, 3);

                let queue = Arc::new(ScheduledEventQueue::<4096>::new());
                let mut driver = SchedulerDriver::new(state.clone(), 48_000, 512, queue.clone());
                let input = SchedulerInput::Offline;
                driver.advance(0, 24_000, input);
                let first = step_transposes(&queue);
                assert!(!first.is_empty());
                assert!(first.iter().all(|t| *t == 3.0), "{first:?}");

                // Port-only republishes keep every body: the cache stays.
                declare(&mut ui, 5);
                driver.advance(24_000, 48_000, input);
                let ported = step_transposes(&queue);
                assert!(!ported.is_empty());
                assert!(ported.iter().all(|t| *t == 5.0), "{ported:?}");
                assert_eq!(driver.process_run_cache_clears, 0);

                // A live-edited body replaces the old one: its callback goes.
                define(&mut ui, "(target-add! (+ 1 (in :by)))");
                declare(&mut ui, 5);
                driver.advance(48_000, 72_000, input);
                let edited = step_transposes(&queue);
                assert!(!edited.is_empty());
                assert!(edited.iter().all(|t| *t == 6.0), "{edited:?}");
                assert_eq!(driver.process_run_cache_clears, 1);
                assert_eq!(
                    driver.scratch_runtime_builds, 1,
                    "the edit must still not rebuild the scheduler VM"
                );
            })
            .expect("spawn")
            .join()
            .expect("test panicked");
    }
}
