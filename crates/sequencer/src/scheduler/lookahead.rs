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
    /// Last process payload epoch copied to SequencerState for UI polling.
    /// This keeps the scheduler → UI mirror off unchanged lookahead chunks.
    pub(super) published_process_channel_epoch: Option<u32>,
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
    /// Generators whose `:tick` failed (eseq-85a.5). The first failure is
    /// reported through `SequencerState::report_generator_tick_error`; a
    /// parked generator is skipped instead of re-erroring every boundary.
    /// Cleared when the worker re-syncs generator definitions, so an edited
    /// body gets a fresh attempt.
    pub(super) parked_generators: std::collections::HashSet<u64>,
    /// Rack owner of each rack-owned generator (a kind instance on a rack,
    /// docs/jaki-kind-spec.md §4): its emissions' `:track` is a member index.
    /// Re-filled by the worker whenever it re-registers published sequencers.
    pub(super) generator_owner_racks: std::collections::HashMap<u64, u64>,
    /// Last line emitted by the ESEQ_DEBUG_SCENE_SLOTS trace, so the seam
    /// reports changes rather than one line per chunk boundary.
    pub(super) last_scene_slot_debug: Option<(usize, usize, usize, String)>,
    /// Graph emissions enqueued ahead of the audio, kept (pre-groove) until
    /// they can no longer sound, so a mid-play resync that clears the queue
    /// can replay them: graph runtimes are not rewound, and already ran their
    /// boundaries up to the old frontier (eseq-groove.8).
    pub(super) graph_replay: Vec<RetainedGraphEmission>,
    /// The last prebuilt chunk snapshot patched with the live rack config
    /// ([`with_live_rack_config`]), so a song row that plays for many
    /// chunks after a groove or member edit is cloned once, not per chunk.
    pub(super) live_rack_config_chunk: Option<LiveRackConfigChunk>,
}

/// A prebuilt chunk snapshot (`source`) re-pointed at the live rack config
/// (groove table `grooves`, plus the live member lists): `patched`.
pub(super) struct LiveRackConfigChunk {
    source: Arc<SequencerSnapshot>,
    grooves: Arc<Vec<Option<crate::groove::TrackGrooveSnapshot>>>,
    patched: Arc<SequencerSnapshot>,
}

/// `prebuilt` (a song row's snapshot, a quantized launch's, or a merge of
/// either) with the LIVE rack config published with `base`: the rack groove
/// table of ITS scene, and the rack member lists.
///
/// Both are project-level rack config, not frozen scene content, but every
/// prebuilt snapshot copied them at preflight — and Play always preflights
/// (the song runtime, plus the auto-latched launch merged over its rows).
/// Scheduling from the frozen copy made a groove pick, an amount drag or Off
/// inaudible until the next Play, and a rack-owned generator (a jaki
/// instance, whose routes are member indices) could not reach a pad added
/// mid-play: its member resolved to nothing.
/// The groove table comes from `base.scene_track_grooves` for the prebuilt
/// chunk's scene (`transport.current_pattern`), so a clip launch plays the
/// new clip's groove from exactly the boundary its patterns start on. The
/// live table is also what the early/late leads, the resync recovery and the
/// record unwind read, so every groove reader agrees. Config that has not
/// changed since preflight is the same `Arc` / an equal short list, so this
/// is free until an edit.
pub(super) fn with_live_rack_config(
    cache: &mut Option<LiveRackConfigChunk>,
    mut prebuilt: Arc<SequencerSnapshot>,
    base: &SequencerSnapshot,
) -> Arc<SequencerSnapshot> {
    let live = base.scene_groove_table(prebuilt.transport.current_pattern);
    if Arc::ptr_eq(&prebuilt.track_grooves, live)
        && prebuilt.rack_memberships == base.rack_memberships
    {
        return prebuilt;
    }
    // A merge built for this chunk alone: patch it in place.
    if let Some(unique) = Arc::get_mut(&mut prebuilt) {
        unique.track_grooves = Arc::clone(live);
        unique.rack_memberships.clone_from(&base.rack_memberships);
        return prebuilt;
    }
    if let Some(cached) = cache.as_ref() {
        if Arc::ptr_eq(&cached.source, &prebuilt)
            && Arc::ptr_eq(&cached.grooves, live)
            && cached.patched.rack_memberships == base.rack_memberships
        {
            return Arc::clone(&cached.patched);
        }
    }
    let mut patched = (*prebuilt).clone();
    patched.track_grooves = Arc::clone(live);
    patched.rack_memberships.clone_from(&base.rack_memberships);
    let patched = Arc::new(patched);
    *cache = Some(LiveRackConfigChunk {
        source: prebuilt,
        grooves: Arc::clone(live),
        patched: Arc::clone(&patched),
    });
    patched
}

/// One enqueued graph emission, as the graph produced it (before the rack
/// groove), for a resync replay (`replay_retained_graph_emissions`).
#[derive(Clone, Debug)]
pub(super) struct RetainedGraphEmission {
    /// `SnapshotSequencerClock::seek_generation` when it was emitted.
    generation: u64,
    graph_id: u64,
    /// The node's route when it fired: a replay re-resolves a routed fire
    /// through the node's CURRENT route, so a track move or delete that
    /// remapped the graph also moves (or drops) its replayed fires.
    route: Option<usize>,
    /// The sample the emission was enqueued at (after the groove).
    enqueued_sample: u64,
    emission: crate::graph::GraphEmission,
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
            published_process_channel_epoch: None,
            resolved_read_pattern_epoch: None,
            graph_manifests: Vec::new(),
            graph_runtimes: Vec::new(),
            debug_graph_drive_chunks: 0,
            debug_accum_invocations: 0,
            quantized_launches: crate::quantized_launch::PendingQuantizedLaunches::default(),
            song: None,
            roll: RollState::new(),
            parked_generators: std::collections::HashSet::new(),
            generator_owner_racks: std::collections::HashMap::new(),
            last_scene_slot_debug: None,
            graph_replay: Vec::new(),
            live_rack_config_chunk: None,
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

/// The scene-slot overrides a chunk's generators must observe.
///
/// `chunk` is whichever snapshot governs this chunk: the live base snapshot,
/// a song row's, or a quantized launch's. The latter two are PREBUILT — their
/// `scene_slots` froze at preflight, so a slot written while the song plays
/// would never reach a shipped tick (a row preflighted before the first write
/// carries an empty store, and the fallback to the declaration default makes
/// the override look silently inert).
///
/// The chunk still decides WHICH scene is playing; only the values come from
/// the live table published with `base_snapshot`. For the base snapshot this
/// is an identity — its table entry for its own scene is `scene_slots` — so
/// one rule covers all three sources. The frozen copy is the fallback for a
/// scene index the live table no longer has (a scene deleted mid-song).
pub(super) fn scene_slot_debug_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("ESEQ_DEBUG_SCENE_SLOTS").is_some())
}

pub(super) fn scene_slots_for_chunk(
    base_snapshot: &SequencerSnapshot,
    chunk: &SequencerSnapshot,
) -> Arc<crate::sequencer::SceneSlotStore> {
    base_snapshot
        .scene_slot_table
        .get(chunk.transport.current_pattern)
        .cloned()
        .unwrap_or_else(|| Arc::new(chunk.scene_slots.clone()))
}

pub(super) fn build_scheduler_scratch_runtime(
    state: Arc<SequencerState>,
    user_source: &str,
    debug_accum: bool,
) -> (Option<lisp_host::ScratchControlRuntime>, Vec<String>) {
    let mut errors = Vec::new();
    let midi_fx_source = lisp_host::load_midi_fx_library_source();
    let process_source = lisp_host::load_process_library_source();
    if midi_fx_source.trim().is_empty()
        && process_source.trim().is_empty()
        && user_source.trim().is_empty()
    {
        return (None, errors);
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
                errors.push(format!("builtin MIDI FX: {err}"));
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
                errors.push(format!("builtin processes: {err}"));
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
                errors.push(format!("project scratch: {err}"));
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

    (keep_runtime.then_some(runtime), errors)
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

/// Forget retained graph emissions that can no longer sound (or belong to
/// an older seek generation): only those still ahead of `rendered` can need
/// a resync replay.
fn retain_live_graph_emissions(
    graph_replay: &mut Vec<RetainedGraphEmission>,
    clock: &SnapshotSequencerClock,
    base_snapshot: &SequencerSnapshot,
    samples_per_quarter: f64,
    rendered: u64,
) {
    let late_reach = (base_snapshot.groove_late_lead_beats() * samples_per_quarter).ceil();
    let late_reach = if late_reach.is_finite() && late_reach > 0.0 {
        late_reach as u64
    } else {
        0
    };
    let generation = clock.seek_generation;
    graph_replay.retain(|retained| {
        retained.generation == generation
            && retained
                .enqueued_sample
                .max(retained.emission.sample_time.saturating_add(late_reach))
                >= rendered
    });
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
    let published_process_channel_epoch = &mut scheduler.published_process_channel_epoch;
    let graph_manifests = &mut scheduler.graph_manifests;
    let graph_runtimes = &mut scheduler.graph_runtimes;
    let session_launches = &mut scheduler.quantized_launches;
    let mut debug_graph_drive_chunks = scheduler.debug_graph_drive_chunks;
    let mut debug_accum_invocations = scheduler.debug_accum_invocations;
    let song_playback = &mut scheduler.song;
    let parked_generators = &mut scheduler.parked_generators;
    let generator_owner_racks = &scheduler.generator_owner_racks;
    let graph_replay = &mut scheduler.graph_replay;
    let mut track_output_events = Vec::new();
    // How many of `track_output_events` the process reads have seen; see
    // `feed_track_output_reads`.
    let mut track_output_read_cursor = 0_usize;

    // Rack groove early hits (docs/rack-groove-spec.md §Early hits): a
    // negative offset sounds a trig up to `E` beats BEFORE its straight
    // boundary, so every source must be discovered at least `E` ahead of the
    // audio block it lands in. Extending the horizon by `E` does that for all
    // of them at once: the step clock's search window, every self-clocked
    // runtime (graphs run their boundaries sooner, in the same order) and the
    // process/neural/generator layers. The frontier itself is the dedupe:
    // the next call starts where this one stopped, so no boundary is handled
    // twice. Each site floors an early trig at `rendered`, so even the first
    // chunk after a seek never enqueues at a sample the audio has passed.
    // A mid-play resync (queue cleared, clock rewound to `rendered`) breaks
    // the frontier dedupe for early hits that already SOUNDED before
    // `rendered`: the clock finds their boundaries again, so the floor drops
    // them instead of clamping them (`GrooveFloor::replayed_until`).
    // `E` is zero unless a groove can move a trig early, which keeps
    // late-only and ungrooved scheduling bit-identical.
    let early_lead_samples = {
        let lead = (base_snapshot.groove_early_lead_beats() * samples_per_quarter).ceil();
        if lead.is_finite() && lead > 0.0 {
            lead as u64
        } else {
            0
        }
    };
    let horizon = rendered
        .saturating_add(lookahead_target_samples)
        .saturating_add(early_lead_samples);
    // The worker polls every ~1 ms against a render head that moves one
    // audio block at a time, so most calls find the frontier already at the
    // horizon. Those only keep the render-head bookkeeping every call does;
    // the per-call setup below (process aliases, resolved-read bases, the
    // chunk loop) waits for a call that schedules.
    if scheduled_until_sample >= horizon && !clock.graph_replay_pending() {
        clock.forget_sounded_step_hits(rendered);
        retain_live_graph_emissions(graph_replay, clock, base_snapshot, samples_per_quarter, rendered);
        state.set_track_output_current_beat(clock.total_beats);
        return SchedulerLookaheadResult {
            scheduled_until_sample,
        };
    }

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

    // Built on first use: only active step trigs read them, and building
    // clones every registered midi-fx's params.
    let midi_fx_descriptor_source = scratch_runtime
        .as_ref()
        .map(|runtime| runtime.midi_fx_descriptor_source());
    let midi_fx_descriptors_cell = std::cell::OnceCell::new();
    let midi_fx_descriptors_for_scheduling = || -> &Vec<EffectDescriptor> {
        midi_fx_descriptors_cell.get_or_init(|| {
            midi_fx_descriptor_source
                .as_ref()
                .map(|source| source.descriptors())
                .unwrap_or_default()
        })
    };

    let groove_floor = clock.groove_floor(rendered);
    clock.forget_sounded_step_hits(rendered);
    // Graph emissions a mid-play resync cleared from the queue: the graph
    // runtimes already consumed those boundaries, so re-enqueue what they
    // emitted instead (eseq-groove.8). Then forget what can no longer sound.
    if let Some(generation) = clock.take_graph_replay_generation() {
        replay_retained_graph_emissions(
            graph_replay,
            generation,
            clock,
            state,
            graph_runtimes,
            queue,
            base_snapshot,
            &mut track_output_events,
            scratch_runtime,
            midi_fx_quantizer_state,
            process_runtime.global_transpose(),
            pattern_epoch,
            rendered,
            scheduled_until_sample,
            samples_per_quarter,
            debug_accum,
        );
    }
    retain_live_graph_emissions(graph_replay, clock, base_snapshot, samples_per_quarter, rendered);
    while scheduled_until_sample < horizon {
        let max_chunk_frames = (horizon - scheduled_until_sample)
            .min(scheduler_block_size as u64) as usize;
        // Song playback: clamp this chunk to the next row boundary and
        // schedule it from the current row's prebuilt snapshot. A boundary
        // inside a block therefore splits scheduling exactly at its sample:
        // events strictly before it come from the old row, events at/after
        // from the new row (docs/song-mode-spec.md 10.2). The snapshot switch
        // is an `Arc` handoff prepared at preflight — no mutexes, no pattern
        // cloning, no asset loading on this path (spec 9).
        let mut chunk_frames = max_chunk_frames;
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
                max_chunk_frames,
            );
            chunk_frames = frames;
            match install {
                crate::quantized_launch::SessionLaunchInstall::None => {}
                crate::quantized_launch::SessionLaunchInstall::AllTracks => {
                    // Scene launches restart accumulator evolution on every
                    // track, matching the control-side launch path.
                    *pending_accum_reset = [true; MAX_TRACKS];
                    // The launched patterns' authored lengths govern.
                    clock.clear_pattern_lengths();
                    for graph in graph_runtimes.iter_mut() {
                        graph.clear_deltas();
                    }
                }
                crate::quantized_launch::SessionLaunchInstall::Tracks(tracks) => {
                    for track in tracks {
                        if track < MAX_TRACKS {
                            pending_accum_reset[track] = true;
                            clock.clear_pattern_length(track);
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
                max_chunk_frames,
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
                        // Hand the reads the previous run's tail first, so
                        // the reset drops it instead of the next chunk's
                        // feed replaying it into the new run.
                        feed_track_output_reads(
                            process_runtime,
                            &track_output_events,
                            &mut track_output_read_cursor,
                        );
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
                    if row_changed && !wrapped {
                        let (previous, current) = song.transition_rows(prev_row, row);
                        let latch = state.song_manual_latch_mask();
                        for track in 0..current.scheduler_snapshot.tracks.len().min(MAX_TRACKS).min(64) {
                            if latch >> track & 1 == 0
                                && previous.resolved_sources.get(track) != current.resolved_sources.get(track)
                            {
                                clock.adopt_track_source(track, &current.scheduler_snapshot);
                            }
                        }
                    }
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
        // Scene-latched manual launch (takes spec 10): the scene latch
        // suspends the song's SCENE-LEVEL authority, so scene-keyed reads —
        // the defscene slot store this chunk's shipped ticks resolve against
        // via `scene_slots_for_chunk` — must follow the SESSION's current
        // scene, not the governing row's. Without this, a performer's
        // launched scene keeps playing the row's slot values (and live slot
        // writes look inert) until the transport stops.
        if state.song_scene_latch() {
            if let Some(row) = song_row_snapshot.as_deref() {
                if row.transport.current_pattern != base_snapshot.transport.current_pattern {
                    let mut adjusted = row.clone();
                    adjusted.transport.current_pattern =
                        base_snapshot.transport.current_pattern;
                    song_row_snapshot = Some(Arc::new(adjusted));
                }
            }
        }
        // Prebuilt snapshots play with the live rack grooves and members
        // (see `with_live_rack_config`).
        let prebuilt_snapshot = song_row_snapshot
            .or(session_launch_snapshot)
            .map(|prebuilt| {
                with_live_rack_config(&mut scheduler.live_rack_config_chunk, prebuilt, base_snapshot)
            });
        let snapshot: &SequencerSnapshot = prebuilt_snapshot.as_deref().unwrap_or(base_snapshot);
        // Process-driven pattern length (`length!`): a change due at this
        // chunk's start lands before the clock steps, no chunk straddles the
        // next pending boundary, and the chunk snapshot carries each track's
        // effective `num_steps` so every reader below agrees.
        clock.apply_due_pattern_lengths(snapshot);
        for track in 0..snapshot.tracks.len().min(MAX_TRACKS) {
            let marker = clock.pattern_length_marker(track).unwrap_or(0) as u32;
            state.transport.track_process_lengths[track].store(marker, Ordering::Relaxed);
        }
        if let Some(at_beats) = clock.next_pattern_length_boundary(snapshot.tracks.len()) {
            // The tolerance keeps float noise in `remaining * spq` from
            // ceiling one frame past the boundary: that frame would fire the
            // boundary step under the old length.
            let remaining_beats = at_beats - clock.total_beats - super::clock::PATTERN_LENGTH_EPS_BEATS;
            if remaining_beats > 0.0 {
                let remaining_frames =
                    (remaining_beats * samples_per_quarter).ceil().max(1.0) as usize;
                chunk_frames = chunk_frames.min(remaining_frames);
            }
        }
        let length_snapshot = clock.patch_pattern_lengths(snapshot);
        let snapshot: &SequencerSnapshot = length_snapshot.as_deref().unwrap_or(snapshot);
        let chunk_slots = scene_slots_for_chunk(base_snapshot, snapshot);
        process_runtime.set_scene_transpose(&chunk_slots);
        if let Some(scratch) = scratch_runtime.as_ref() {
            // ESEQ_DEBUG_SCENE_SLOTS: one line per change at the seam where a
            // chunk selects the scene its shipped ticks read.
            if scene_slot_debug_enabled() {
                let fingerprint = (
                    snapshot.transport.current_pattern,
                    base_snapshot.transport.current_pattern,
                    base_snapshot.scene_slot_table.len(),
                    format!("{:?}", chunk_slots.values()),
                );
                if scheduler.last_scene_slot_debug.as_ref() != Some(&fingerprint) {
                    eprintln!(
                        "[scene-slots] chunk_scene={} base_scene={} table_len={} values={}",
                        fingerprint.0, fingerprint.1, fingerprint.2, fingerprint.3
                    );
                    scheduler.last_scene_slot_debug = Some(fingerprint);
                }
            }
            scratch.set_scene_slot_snapshot(chunk_slots);
        }
        // A process roll (`roll!`) releases on its deadline beat exactly:
        // never let a chunk straddle it, so the next chunk starts on the
        // release beat and reads the pattern normally from there.
        if let Some(roll) = scheduler.roll.process_roll {
            let remaining_beats = roll.until_beats - clock.total_beats;
            if remaining_beats > 0.0 {
                let remaining_frames =
                    (remaining_beats * samples_per_quarter).ceil().max(1.0) as usize;
                chunk_frames = chunk_frames.min(remaining_frames);
            }
        }
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
        // A process roll (`roll!`) releases itself once the frontier reaches
        // its deadline and owns the remap grid while it runs; otherwise the
        // grid is the transport rate, read fresh every chunk (F2).
        scheduler.roll.release_process_roll_if_due(chunk_start_beats, state);
        let roll_grid = scheduler.roll.active_grid_beats(state);
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
        // The key each track's pattern implies, computed once per chunk per
        // track that steps, for `:key :pattern` reads.
        let mut key_masks: Vec<Option<u16>> = vec![None; snapshot.tracks.len()];
        for trigger in &triggers {
            if trigger.recovered_lag.is_some() {
                // A late groove hit recovered after a resync: its boundary
                // was recorded (and any neural reset applied) before it.
                continue;
            }
            let source = &snapshot.tracks[trigger.track];
            let step = &source.steps[trigger.step];
            let key_mask = *key_masks[trigger.track].get_or_insert_with(|| {
                crate::runtime::harmony::pattern_pitch_class_mask(
                    &source.steps,
                    source.params.num_steps,
                )
            });
            // Pattern data rides along so a `:pattern` read from any track's
            // process later in this chunk sees the step the source is on,
            // same tick (the grab lane's Cirklon semantics).
            process_runtime.record_track_step_boundary_with_pattern(
                trigger.track,
                trigger.absolute_beats,
                Some(crate::process::ProcessStepPattern::from_step_snapshot(
                    step,
                    source.bar_transposes[crate::sequencer::bar_of_step(trigger.step)],
                    key_mask,
                )),
            );
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
        // `roll!` requests from this chunk's process runs, applied after the
        // trigger loop: the remap only touches later chunks, and the roll
        // state is borrowed by the clock pass above.
        let mut pending_process_rolls: Vec<super::roll::PendingProcessRoll> = Vec::new();
        // `length!` requests (track, steps, firing beat), stamped with the
        // track's next cycle boundary once the trigger loop is done.
        let mut pending_pattern_lengths: Vec<(usize, usize, f64)> = Vec::new();
        for trigger in triggers {
            if trigger.recovered_lag.is_some() {
                // A grooved step whose straight boundary a mid-play resync
                // already passed (eseq-groove.8): it is scheduled again only
                // when it had not sounded yet, i.e. a LATE hit the cleared
                // queue still held (scheduled at or after `rendered` under
                // the groove it was scheduled with, which a groove change
                // since may differ from), and the current groove still puts
                // it there. Anything else about the step (off-step p-locks,
                // early or straight hits) happened before the resync, so it
                // is skipped before any side effect.
                let straight = trigger.straight_sample(scheduled_until_sample);
                let still_due = snapshot.tracks[trigger.track].steps[trigger.step].active
                    && clock.step_hit_was_queued(trigger.track, trigger.step, straight, rendered)
                    && step_trigger_sample_time(
                        snapshot,
                        &trigger,
                        straight,
                        sample_rate,
                        samples_per_quarter,
                        groove_floor,
                    )
                    .is_some_and(|sample| sample >= rendered);
                if !still_due {
                    continue;
                }
            }
            let trigger_sample_time = scheduled_until_sample + trigger.offset as u64;
            feed_track_output_reads(
                process_runtime,
                &track_output_events,
                &mut track_output_read_cursor,
            );
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
                groove_floor,
                Some(&|track, beats| {
                    StepLanding::resolve(&*clock, state, snapshot, track, beats, samples_per_quarter)
                }),
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
                // Off-step boundary: p-locks on an inactive step apply to the
                // whole track at that boundary — instrument p-locks to every
                // active voice, effect p-locks to the per-track chain — and
                // then hold (voice/node params are sticky) until the next
                // p-lock or the next ON trigger's full param stamp. The live
                // device-print latch substitutes here too, so a held printing
                // knob is heard on sparse patterns, not just on triggers.
                let sample_time = scheduled_until_sample + trigger.offset as u64;
                let print_overrides =
                    state.device_print_override.values_for_track(trigger.track);
                let mut off_step_effect_params =
                    resolve_track_send_params(snapshot, trigger.track, trigger.step);
                off_step_effect_params.extend(resolve_effect_plocks(
                    snapshot,
                    trigger.track,
                    trigger.step,
                    print_overrides.as_ref(),
                ));
                if !off_step_effect_params.is_empty() {
                    chunk_enqueued &= queue.push(ScheduledEvent {
                        audition_generation: 0,
                        pattern_epoch,
                        sample_time,
                        kind: ScheduledEventKind::EffectParams {
                            track: trigger.track,
                            effect_params: off_step_effect_params,
                        },
                    }).is_ok();
                }
                chunk_enqueued &= enqueue_instrument_param_change(
                    queue,
                    pattern_epoch,
                    sample_time,
                    trigger.track,
                    resolve_instrument_plocks(
                        snapshot,
                        trigger.track,
                        trigger.step,
                        print_overrides.as_ref(),
                    ),
                );
                // Instrument Rack tracks keep their p-locks inside the rack
                // (macros, slot params, slot devices), which the two
                // track-level resolvers above never see. One event per
                // locked (or currently printing) off step lets the audio
                // side apply them to the rack's sounding voices.
                if rack_off_step_has_params(state, snapshot, trigger.track, trigger.step) {
                    chunk_enqueued &= queue
                        .push(ScheduledEvent {
                            audition_generation: 0,
                            pattern_epoch,
                            sample_time,
                            kind: ScheduledEventKind::RackParams {
                                track: trigger.track,
                                step: trigger.step,
                            },
                        })
                        .is_ok();
                }
                if !chunk_enqueued {
                    break;
                }
                continue;
            }
            if track_has_live_midi_fx_notes(
                live_midi_fx_tracks,
                snapshot,
                midi_fx_descriptors_for_scheduling(),
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
            let step_boundary_sample_time = trigger.straight_sample(scheduled_until_sample);
            // Step Delay, then the feel: the member's rack groove replaces
            // track/step swing when it has one (rack groove spec §Sites 1).
            let Some(sample_time) = step_trigger_sample_time(
                snapshot,
                &trigger,
                step_boundary_sample_time,
                sample_rate,
                samples_per_quarter,
                groove_floor,
            ) else {
                // An early groove hit that already sounded before a mid-play
                // resync: the whole step was handled then (rack groove spec
                // §Early hits), so it is not handled again.
                continue;
            };
            clock.record_queued_step_hit(
                trigger.track,
                trigger.step,
                step_boundary_sample_time,
                sample_time,
            );

            let mut resolved = ResolvedStep {
                duration: step_snapshot.params[StepParam::Duration.index()],
                velocity: step_snapshot.params[StepParam::Velocity.index()],
                speed: step_snapshot.params[StepParam::Speed.index()],
                aux_a: step_snapshot.params[StepParam::AuxA.index()],
                aux_b: step_snapshot.params[StepParam::AuxB.index()],
                transpose: step_snapshot.params[StepParam::Transpose.index()],
                pan: step_snapshot.params[StepParam::Pan.index()],
                chop: step_snapshot.params[StepParam::Chop.index()],
                retrig: step_snapshot.params[StepParam::Retrig.index()],
                retrig_rate: step_snapshot.params[StepParam::RetrigRate.index()],
            };
            let bar_transpose =
                track.bar_transposes[crate::sequencer::bar_of_step(trigger.step)];
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
                let overrides = state
                    .step_print_override
                    .values_for_track(trigger.track);
                if let Some(value) = overrides.get(StepParam::Velocity) {
                    resolved.velocity = value;
                }
                if let Some(value) = overrides.get(StepParam::Duration) {
                    print_chord_duration_delta = Some(value - resolved.duration);
                    resolved.duration = value;
                }
                if let Some(value) = overrides.get(StepParam::Transpose) {
                    resolved.transpose = value;
                }
                // Retrig/Rate are the roll knobs: without the pre-echo a drag
                // is inaudible until the loop wraps, which defeats the whole
                // performance gesture (docs/step-retrig-spec.md).
                if let Some(value) = overrides.get(StepParam::Retrig) {
                    resolved.retrig = value;
                }
                if let Some(value) = overrides.get(StepParam::RetrigRate) {
                    resolved.retrig_rate = value;
                }
                // Pan is printable too; without the substitution the latched
                // knob is only heard once the loop wraps.
                if let Some(value) = overrides.get(StepParam::Pan) {
                    resolved.pan = value;
                }
            }
            // Bar transpose (Cirklon P3 bar XPOSE, eseq-m14x): one value per
            // 16-step page, applied to every note the bar plays. Folded in
            // BEFORE the process chain runs, so `(current-note)` and every
            // process read already carry it and a note grab's replace
            // formula keeps it. It lands AFTER the print-override
            // substitution above because that substitution stands in for the
            // step_data write behind the playhead — which the next pass
            // plays back with the bar transpose on top, exactly like this.
            // Chord steps move with it for free: playback applies
            // `resolved.transpose - step_transpose` as a delta per chord
            // note. Scene transpose is applied later and stacks on top.
            resolved.transpose += bar_transpose;
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
                        midi_fx_descriptors_for_scheduling(),
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
                        note: crate::process::step_authored_note(
                            &step_snapshot.chord,
                            &step_snapshot.params,
                        ),
                        event,
                        fire_seed: None,
                        after_reset: false,
                        delay_offset_steps: 0.0,
                    },
                    Some(&slot_inlet_writes),
                ) {
                    if !invoke_process_cascade(
                        scratch_runtime,
                        process_runtime,
                        invocation,
                        debug_accum,
                        |scratch, process_runtime, runtime_id, commands| {
                            super::roll::collect_process_roll_requests(
                                commands,
                                trigger.absolute_beats,
                                step_beats,
                                resolved.duration,
                                &mut pending_process_rolls,
                            );
                            collect_pattern_length_requests(
                                commands,
                                trigger.track,
                                trigger.absolute_beats,
                                &mut pending_pattern_lengths,
                            );
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
                                midi_fx_descriptors_for_scheduling(),
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
            if !process_overlay.instrument_effective.is_empty() {
                state.publish_process_effective_params(
                    trigger.track,
                    &process_overlay.instrument_effective,
                );
            }
            if !process_overlay.send_effective.is_empty() {
                state.publish_process_effective_sends(
                    trigger.track,
                    &process_overlay.send_effective,
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
                note: crate::process::step_authored_note(
                    &step_snapshot.chord,
                    &step_snapshot.params,
                ),
                written_inlets: Vec::new(),
                after_reset: false,
                delay_offset_steps: 0.0,
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
                        super::roll::collect_process_roll_requests(
                            commands,
                            trigger.absolute_beats,
                            step_beats,
                            resolved.duration,
                            &mut pending_process_rolls,
                        );
                        collect_pattern_length_requests(
                            commands,
                            trigger.track,
                            trigger.absolute_beats,
                            &mut pending_pattern_lengths,
                        );
                        apply_step_process_commands(
                            scratch,
                            process_runtime,
                            runtime_id,
                            snapshot,
                            midi_fx_descriptors_for_scheduling(),
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
                    groove_floor,
                    Some(&|track, beats| {
                        StepLanding::resolve(&*clock, state, snapshot, track, beats, samples_per_quarter)
                    }),
                    debug_accum,
                )
            {
                chunk_enqueued = false;
                break;
            }
            // Groove accent (rack groove spec §Application, eseq-groove.5):
            // the member's groove scales the base trig's resolved velocity at
            // the same straight transport beat that keyed its timing. It lands
            // AFTER the process chain, exactly once per sounding event: the
            // chain reads the straight velocity (as it reads straight timing),
            // and everything the chain spawns (ratchets, `emit`s) is a process
            // event that `enqueue_due_process_emissions` grooves at its own
            // beat. Grooving here first would scale those spawned events twice.
            // The accumulator below builds on the grooved base like any other
            // base-event consumer.
            resolved.velocity = grooved_velocity(
                snapshot,
                Some(trigger.track),
                resolved.velocity,
                trigger.boundary_beats,
            );
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
                let print_overrides =
                    state.device_print_override.values_for_track(trigger.track);
                let mut effect_params = resolve_effect_params(
                    snapshot,
                    trigger.track,
                    trigger.step,
                    print_overrides.as_ref(),
                );
                effect_params.extend(resolve_track_send_params(
                    snapshot,
                    trigger.track,
                    trigger.step,
                ));
                let mut instrument_params = resolve_instrument_params(
                    snapshot,
                    trigger.track,
                    trigger.step,
                    print_overrides.as_ref(),
                );
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
                                    live_origins: Vec::new(),
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
                                    live_origins: Vec::new(),
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
                                if let Some(event) = rebind_midi_fx_event_to_track(
                                    snapshot,
                                    event,
                                    target_track,
                                    state
                                        .device_print_override
                                        .values_for_track(target_track)
                                        .as_ref(),
                                ) {
                                    accumulator_events.push(event);
                                }
                            }
                            for event in accumulator_events {
                                if track_has_live_midi_fx_notes(
                                    live_midi_fx_tracks,
                                    snapshot,
                                    midi_fx_descriptors_for_scheduling(),
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
                    midi_fx_descriptors_for_scheduling(),
                    target_track,
                ) {
                    continue;
                }
                let same_track_process_targets = target_track == trigger.track;
                let print_overrides =
                    state.device_print_override.values_for_track(target_track);
                let mut effect_params = resolve_effect_params(
                    snapshot,
                    target_track,
                    trigger.step,
                    print_overrides.as_ref(),
                );
                effect_params.extend(resolve_track_send_params(
                    snapshot,
                    target_track,
                    trigger.step,
                ));
                let mut instrument_params = resolve_instrument_params(
                    snapshot,
                    target_track,
                    trigger.step,
                    print_overrides.as_ref(),
                );
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
                            midi_fx_descriptors_for_scheduling(),
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
                        if clock.graph_seed_is_new(seed_beats, samples_per_quarter.into()) {
                            seed_graph_runtimes(
                                graph_runtimes,
                                &seed_event,
                                seed_beats,
                                samples_per_quarter.into(),
                            );
                        }
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
                        if clock.graph_seed_is_new(seed_beats, samples_per_quarter.into()) {
                            seed_graph_runtimes(
                                graph_runtimes,
                                &step_event,
                                seed_beats,
                                samples_per_quarter.into(),
                            );
                        }
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
                    // A step the clock re-found below the last resync's old
                    // frontier already seeded the graphs, which were not
                    // rewound (their emissions are replayed instead).
                    if clock.graph_seed_is_new(seed_beats, samples_per_quarter.into()) {
                        seed_graph_runtimes(
                            graph_runtimes,
                            &step_event,
                            seed_beats,
                            samples_per_quarter.into(),
                        );
                    }
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
                groove_floor,
                Some(&|track, beats| {
                    StepLanding::resolve(&*clock, state, snapshot, track, beats, samples_per_quarter)
                }),
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
        neural_events.retain_mut(|output| {
            if !output.emit_trigger {
                return true;
            }
            let event_beats = sample_time_to_beats(
                chunk_start_beats,
                scheduled_until_sample,
                output.sample_time,
                samples_per_quarter,
            );
            // A fire into another track lands on that track's step grid at
            // its straight beat and takes that step's device p-locks.
            if let Some(landing) = StepLanding::resolve(
                &*clock,
                state,
                snapshot,
                output.event.track,
                event_beats,
                samples_per_quarter,
            ) {
                land_network_event(snapshot, &landing, &mut output.event);
            }
            // `None`: an early groove hit that already sounded before a
            // mid-play resync.
            let Some(sample_time) = grooved_or_swung_network_sample_time(
                snapshot,
                &output.event,
                output.sample_time,
                event_beats,
                samples_per_quarter,
                groove_floor,
            ) else {
                return false;
            };
            output.sample_time = sample_time;
            output.event.resolved.velocity = grooved_velocity(
                snapshot,
                Some(output.event.track),
                output.event.resolved.velocity,
                event_beats,
            );
            true
        });
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
        // Generators a graph node gates run in a second pass after the
        // graph stage, so a fire reaches the generator's tick on the same
        // boundary (docs/jaki-trig-modes-spec.md §4.1).
        let gated_generators: std::collections::HashSet<u64> = graph_runtimes
            .iter()
            .filter(|graph| !graph.is_empty())
            .flat_map(|graph| graph.gate_targets())
            .collect();
        if !generator_runtime.is_empty()
            && !run_generator_stage(
                generator_runtime,
                scratch_runtime,
                process_runtime,
                parked_generators,
                generator_owner_racks,
                clock,
                state,
                snapshot,
                queue,
                &mut track_output_events,
                midi_fx_quantizer_state,
                pattern_epoch,
                chunk_start_beats,
                chunk_end_beats,
                scheduled_until_sample,
                samples_per_quarter,
                groove_floor,
                debug_accum,
                |id| !gated_generators.contains(&id),
            )
        {
            break;
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
                            groove_floor,
                            Some(&|track, beats| {
                                StepLanding::resolve(&*clock, state, snapshot, track, beats, samples_per_quarter)
                            }),
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
                        groove_floor,
                        Some(&|track, beats| {
                            StepLanding::resolve(&*clock, state, snapshot, track, beats, samples_per_quarter)
                        }),
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
        }

        // Publish after the process cascade so UI polling sees values written
        // by processes in this chunk, including contention with a live widget.
        // Replacing the complete map also removes channels dropped by a later
        // authoring sync. Unchanged chunks do no allocation or locking here.
        let channel_epoch = process_runtime.payload_epoch();
        if *published_process_channel_epoch != Some(channel_epoch) {
            state.publish_process_channel_values(process_runtime.channel_value_literals());
            *published_process_channel_epoch = Some(channel_epoch);
        }
        if let Some(scopes) = process_runtime.take_step_process_scopes_if_changed() {
            state.publish_process_scope_values(scopes);
        }
        if let Some(errors) = process_runtime.take_run_errors_if_changed() {
            state.publish_process_run_errors(errors);
        }
        if !chunk_enqueued {
            break;
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
            // Graphs earlier in the order, and everything the trigger loop
            // enqueued, are what this graph's `:output` reads can see.
            feed_track_output_reads(
                process_runtime,
                &track_output_events,
                &mut track_output_read_cursor,
            );
            let mut graph_emissions = Vec::new();
            let mut graph_eval_count = 0_usize;
            if scratch_runtime.is_some() {
                let manifest = &graph_manifests[graph_index];
                // Resolved (override-or-manifest) cap, carried on the runtime.
                let max_poly = graph_runtimes[graph_index].max_poly();
                // The driver pairs the node-rule predicate with the emit-time
                // process-patch hook (scheduler/node_process.rs).
                let mut driver = super::node_process::SchedulerGraphDriver {
                    scratch_runtime,
                    process_runtime,
                    snapshot,
                    manifest,
                    debug_graph,
                    debug_accum,
                    eval_count: 0,
                    last_delay_offset_steps: 0.0,
                    pending_resets: Vec::new(),
                };
                graph_runtimes[graph_index].process_block_with_driver(
                    chunk_start_beats,
                    chunk_end_beats,
                    scheduled_until_sample,
                    samples_per_quarter,
                    max_poly,
                    &mut driver,
                    &mut graph_emissions,
                );
                graph_eval_count += driver.eval_count;
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
            // A node routed to a generator gates (or restarts) it instead of
            // sounding a note: its fire becomes a gate trigger for the generator's
            // second pass below, and is neither enqueued nor retained for
            // replay (docs/jaki-trig-modes-spec.md §4, §6). Split before the
            // accent merge, which would fold two gate targets' coincident
            // same-note fires (both track `None`) into one.
            graph_emissions.retain(|emission| {
                let Some(target) =
                    graph_runtimes[graph_index].node_gate_target(emission.node_index)
                else {
                    return true;
                };
                generator_runtime.push_gate_trigger(
                    target.id,
                    crate::generator::GateTrigger {
                        kind: if target.restart {
                            crate::generator::GateTriggerKind::Restart
                        } else {
                            crate::generator::GateTriggerKind::Play
                        },
                        beat: emission.grid_beats,
                        duration_beats: emission.event.resolved.duration as f64,
                        note: emission.event.resolved.transpose as f32,
                        velocity: emission.event.resolved.velocity as f32,
                    },
                );
                false
            });
            // Velocity-merge coincident hits only when they are the same note.
            // Different notes at the same sample/track are polyphony.
            for mut emission in merge_graph_emission_accents(graph_emissions) {
                let straight_emission = emission.clone();
                let graph_source = EmittedNetworkEventSource::Graph {
                    graph_index,
                    node_index: emission.node_index,
                };
                // The step the fire lands on, at its straight beat, supplies
                // its device p-locks.
                let landing = graph_source.resolve_track(emission.event.track).and_then(|track| {
                    StepLanding::resolve(
                        &*clock,
                        state,
                        snapshot,
                        track,
                        emission.grid_beats,
                        samples_per_quarter,
                    )
                });
                // A fire aimed at a rack member plays through the rack's
                // groove, keyed on its straight (quantized) beat (rack
                // groove spec §Sites 2). No groove: unchanged.
                // `None`: an early hit that already sounded before a
                // mid-play resync.
                let Some(sample_time) = grooved_emission_sample_time(
                    snapshot,
                    emission.event.track,
                    emission.sample_time,
                    emission.grid_beats,
                    samples_per_quarter,
                    groove_floor,
                ) else {
                    continue;
                };
                emission.sample_time = sample_time;
                emission.event.resolved.velocity = grooved_velocity(
                    snapshot,
                    emission.event.track,
                    emission.event.resolved.velocity,
                    emission.grid_beats,
                );
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
                    graph_source,
                    emission.event,
                    landing,
                    debug_accum,
                ) {
                    chunk_enqueued = false;
                    break;
                }
                graph_replay.push(RetainedGraphEmission {
                    generation: clock.seek_generation,
                    graph_id: graph_runtimes[graph_index].id,
                    route: graph_runtimes[graph_index].node_route(straight_emission.node_index),
                    enqueued_sample: emission.sample_time,
                    emission: straight_emission,
                });
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
        if !gated_generators.is_empty()
            && !run_generator_stage(
                generator_runtime,
                scratch_runtime,
                process_runtime,
                parked_generators,
                generator_owner_racks,
                clock,
                state,
                snapshot,
                queue,
                &mut track_output_events,
                midi_fx_quantizer_state,
                pattern_epoch,
                chunk_start_beats,
                chunk_end_beats,
                scheduled_until_sample,
                samples_per_quarter,
                groove_floor,
                debug_accum,
                |id| gated_generators.contains(&id),
            )
        {
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
        // Graph node patches ran above; their run errors (expr cards) reach
        // the UI with this chunk rather than the next.
        if let Some(errors) = process_runtime.take_run_errors_if_changed() {
            state.publish_process_run_errors(errors);
        }
        if !chunk_enqueued {
            break;
        }
        for request in pending_process_rolls.drain(..) {
            if scheduler.roll.engage_process_roll(request, clock, snapshot, state) {
                scheduler.roll.publish_windows(state, request.grid_beats);
            }
        }
        for (track, steps, fired_beats) in pending_pattern_lengths.drain(..) {
            clock.request_pattern_length(snapshot, track, steps, fired_beats);
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

/// Collect the `length!` commands of one process run on `track`.
fn collect_pattern_length_requests(
    commands: &[crate::process::ProcessRunCommand],
    track: usize,
    fired_beats: f64,
    pending: &mut Vec<(usize, usize, f64)>,
) {
    for command in commands {
        if let crate::process::ProcessRunCommand::PatternLength(steps) = command {
            pending.push((track, *steps, fired_beats));
        }
    }
}

/// Whether an inactive step on `track` needs a `RackParams` event: the track
/// is an Instrument Rack and either something in the rack is p-locked at
/// `step` or a rack-macro print latch is held for the track (so a printing
/// knob is heard on sparse patterns, like the device-print latch is).
pub(super) fn rack_off_step_has_params(
    state: &SequencerState,
    snapshot: &SequencerSnapshot,
    track: usize,
    step: usize,
) -> bool {
    let Some(rack) = snapshot
        .tracks
        .get(track)
        .and_then(|track| track.rack_track.as_ref())
    else {
        return false;
    };
    rack.step_has_plocks(step)
        || state
            .rack_macro_print_override
            .values_for_track(track)
            .iter()
            .any(Option::is_some)
}

/// Hand the process reads every output event enqueued since the last call, so
/// `(read (track n :chord|:key :output))` follows what each instrument is
/// actually sent. Reads see output enqueued before them: a follower firing on
/// the same boundary as its source sees the source's new notes only when the
/// source was enqueued first (an earlier graph, or the trigger loop).
fn feed_track_output_reads(
    process_runtime: &mut crate::process::ProcessRuntime,
    events: &[TrackOutputEvent],
    cursor: &mut usize,
) {
    for event in events.get(*cursor..).unwrap_or(&[]) {
        process_runtime.record_track_output(
            event.track,
            event.beat,
            event.harmony.end_beat,
            event.harmony.pitches(),
        );
    }
    *cursor = events.len();
}

/// Re-enqueue the graph emissions a mid-play resync cleared with the queue
/// (docs/rack-groove-spec.md §Early hits, eseq-groove.8). The resync rewinds
/// the step clock to `rendered` but not the graph runtimes, which already
/// evaluated every boundary up to the old frontier: re-running them would
/// fire those boundaries twice, and skipping them loses what they emitted.
/// So what they emitted is replayed through the CURRENT snapshot's groove,
/// routing and MIDI fx, exactly like a fresh emission, keeping only what
/// has not sounded: it must have been enqueued at or after `rendered` (so
/// it was still in the cleared queue) and its re-grooved sample must be at
/// or after `rendered` too (an early move that lands before it is dropped). Replayed emissions stay
/// retained under the current generation, for a later resync.
/// One pass of the Lisp-generator stage over a chunk: tick the generators
/// `include` accepts, resolve their `seq-emit` output to network triggers and
/// enqueue them. False when the queue filled (the chunk stops there).
#[allow(clippy::too_many_arguments)]
fn run_generator_stage<const QUEUE_CAP: usize>(
    generator_runtime: &mut crate::generator::GeneratorRuntime,
    scratch_runtime: &mut Option<lisp_host::ScratchControlRuntime>,
    process_runtime: &mut crate::process::ProcessRuntime,
    parked_generators: &mut std::collections::HashSet<u64>,
    generator_owner_racks: &std::collections::HashMap<u64, u64>,
    clock: &SnapshotSequencerClock,
    state: &SequencerState,
    snapshot: &SequencerSnapshot,
    queue: &ScheduledEventQueue<QUEUE_CAP>,
    track_output_events: &mut Vec<TrackOutputEvent>,
    midi_fx_quantizer_state: &mut MidiFxQuantizerState,
    pattern_epoch: u64,
    chunk_start_beats: f64,
    chunk_end_beats: f64,
    scheduled_until_sample: u64,
    samples_per_quarter: f64,
    groove_floor: crate::groove::GrooveFloor,
    debug_accum: bool,
    include: impl Fn(u64) -> bool,
) -> bool {
    let mut chunk_enqueued = true;
    let mut generator_emissions = Vec::new();
    let mut generator_control_emissions = Vec::new();
    if let Some(scratch) = scratch_runtime.as_mut() {
        // Channel snapshot for chan-get: ticks in this chunk observe
        // process-channel writes from earlier chunks (processes run
        // after generators within a chunk).
        scratch.set_generator_channel_values(
            process_runtime.payload_epoch(),
            process_runtime.channel_values(),
        );
        // Naming a generator means locking the definition registry
        // and cloning every name; failures are rare, so collect ids
        // here and resolve names once, after the block.
        let mut tick_failures: Vec<(u64, String)> = Vec::new();
        generator_runtime.process_block_selected(
            chunk_start_beats,
            chunk_end_beats,
            scheduled_until_sample,
            samples_per_quarter,
            include,
            |input| {
                let generator_index = input.generator_index;
                let generator_id = input.id;
                let random_state = input.random_state;
                let fallback_state = input.state.clone();
                let empty = crate::generator::GeneratorTickResult {
                    emitted: Vec::new(),
                    controls: Vec::new(),
                    random_state,
                    state: fallback_state,
                };
                if parked_generators.contains(&generator_id) {
                    return empty;
                }
                match scratch.invoke_sequencer_tick(generator_index, input) {
                    Ok(mut result) => {
                        if let Some(group_id) = generator_owner_racks.get(&generator_id) {
                            map_rack_member_emissions(
                                &mut result.emitted,
                                crate::graph::rack_members(
                                    &snapshot.rack_memberships,
                                    *group_id,
                                )
                                .unwrap_or(&[]),
                            );
                        }
                        result
                    }
                    Err(error) => {
                        // Report the first failure and park: a broken
                        // tick must be one loud notice, not silence
                        // re-erroring every boundary. The park clears
                        // when definitions re-sync.
                        tick_failures.push((generator_id, error));
                        parked_generators.insert(generator_id);
                        empty
                    }
                }
            },
            &mut generator_emissions,
            &mut generator_control_emissions,
        );
        if !tick_failures.is_empty() {
            let generator_names: std::collections::HashMap<u64, String> = scratch
                .sequencer_defs()
                .iter()
                .map(|definition| (definition.id, definition.name.clone()))
                .collect();
            for (generator_id, error) in tick_failures {
                let name = generator_names
                    .get(&generator_id)
                    .cloned()
                    .unwrap_or_else(|| format!("generator {generator_id}"));
                eprintln!("sequencer tick failed for {name} ({generator_id}): {error}");
                state.report_generator_tick_error(generator_id, name, error);
            }
        }
        // Mixer-control holds ride to the app thread through the
        // mailbox; the frame drain applies due ones
        // (docs/jaki-mixer-control-routes-spec.md).
        for emission in generator_control_emissions.drain(..) {
            state.scheduled_mixer_controls().push(
                emission.engage_sample,
                emission.release_sample,
                emission.generator_index,
                emission.control.op,
                emission.control.target,
            );
        }
    } else if debug_routing_enabled() {
        eprintln!(
            "[routing] skip generator-block reason=no-scratch-runtime chunk=({:.6}..{:.6})",
            chunk_start_beats, chunk_end_beats
        );
    }
    // Velocity-merge coincident hits only when they are the same note.
    // Different notes at the same sample/track are polyphony.
    for mut emission in merge_generator_emission_accents(generator_emissions) {
        let generator_source = EmittedNetworkEventSource::Generator {
            index: emission.generator_index,
        };
        // The step the hit lands on, at its straight beat (before
        // the groove moves it), supplies its device p-locks.
        let landing = generator_source.resolve_track(emission.event.track).and_then(|track| {
            StepLanding::resolve(
                &*clock,
                state,
                snapshot,
                track,
                sample_time_to_beats(
                    chunk_start_beats,
                    scheduled_until_sample,
                    emission.sample_time,
                    samples_per_quarter,
                ),
                samples_per_quarter,
            )
        });
        // Generator hits aimed at a rack member play through its
        // groove like every other trig source; their sample time is
        // their straight beat.
        if let Some(track) = emission.event.track {
            if snapshot.track_groove(track).is_some() {
                let straight_beats = sample_time_to_beats(
                    chunk_start_beats,
                    scheduled_until_sample,
                    emission.sample_time,
                    samples_per_quarter,
                );
                // `None`: an early hit that already sounded before
                // a mid-play resync.
                let Some(sample_time) = grooved_emission_sample_time(
                    snapshot,
                    Some(track),
                    emission.sample_time,
                    straight_beats,
                    samples_per_quarter,
                    groove_floor,
                ) else {
                    continue;
                };
                emission.sample_time = sample_time;
                emission.event.resolved.velocity = grooved_velocity(
                    snapshot,
                    Some(track),
                    emission.event.resolved.velocity,
                    straight_beats,
                );
            }
        }
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
            track_output_events,
            scratch_runtime.as_mut(),
            Some(&mut *midi_fx_quantizer_state),
            pattern_epoch,
            emission.sample_time,
            samples_per_quarter as f32,
            event_beats,
            process_runtime.global_transpose(),
            generator_source,
            emission.event,
            landing,
            debug_accum,
        ) {
            chunk_enqueued = false;
            break;
        }
    }
    if !chunk_enqueued {
        return false;
    }
    true
}

#[allow(clippy::too_many_arguments)]
fn replay_retained_graph_emissions<const QUEUE_CAP: usize>(
    retained: &mut Vec<RetainedGraphEmission>,
    generation: u64,
    clock: &SnapshotSequencerClock,
    state: &SequencerState,
    graph_runtimes: &[crate::graph::GraphRuntime],
    queue: &ScheduledEventQueue<QUEUE_CAP>,
    snapshot: &SequencerSnapshot,
    track_output_events: &mut Vec<TrackOutputEvent>,
    scratch_runtime: &mut Option<lisp_host::ScratchControlRuntime>,
    midi_fx_quantizer_state: &mut MidiFxQuantizerState,
    global_transpose: f32,
    pattern_epoch: u64,
    rendered: u64,
    chunk_start_sample: u64,
    samples_per_quarter: f64,
    debug_accum: bool,
) {
    let floor = crate::groove::GrooveFloor {
        not_before: rendered,
        // Every retained emission was discovered before the resync.
        replayed_until: u64::MAX,
    };
    let chunk_start_beats = clock.total_beats;
    let mut entries = std::mem::take(retained);
    entries.retain(|entry| entry.generation == generation);
    entries.sort_by_key(|entry| entry.emission.sample_time);
    for mut entry in entries {
        // The cleared queue only held samples at or after `rendered`: an
        // entry enqueued before it already sounded, whatever a groove
        // change since would now move it to.
        if entry.enqueued_sample < rendered {
            continue;
        }
        let Some(graph_index) = graph_runtimes
            .iter()
            .position(|graph| graph.id == entry.graph_id)
        else {
            continue;
        };
        let route_now = graph_runtimes[graph_index].node_route(entry.emission.node_index);
        if entry.emission.event.track == entry.route && route_now != entry.route {
            entry.emission.event.track = route_now;
            entry.route = route_now;
        }
        if entry
            .emission
            .event
            .track
            .is_some_and(|track| track >= snapshot.tracks.len())
        {
            continue;
        }
        let mut emission = entry.emission.clone();
        let Some(sample_time) = grooved_emission_sample_time(
            snapshot,
            emission.event.track,
            emission.sample_time,
            emission.grid_beats,
            samples_per_quarter,
            floor,
        ) else {
            continue;
        };
        if sample_time < rendered {
            continue;
        }
        emission.sample_time = sample_time;
        emission.event.resolved.velocity = grooved_velocity(
            snapshot,
            emission.event.track,
            emission.event.resolved.velocity,
            emission.grid_beats,
        );
        let event_beats = sample_time_to_beats(
            chunk_start_beats,
            chunk_start_sample,
            sample_time,
            samples_per_quarter,
        ) as f32;
        let source = EmittedNetworkEventSource::Graph {
            graph_index,
            node_index: emission.node_index,
        };
        let landing = source.resolve_track(emission.event.track).and_then(|track| {
            StepLanding::resolve(
                clock,
                state,
                snapshot,
                track,
                emission.grid_beats,
                samples_per_quarter,
            )
        });
        if !enqueue_emitted_network_event_with_midi_fx(
            queue,
            snapshot,
            track_output_events,
            scratch_runtime.as_mut(),
            Some(&mut *midi_fx_quantizer_state),
            pattern_epoch,
            sample_time,
            samples_per_quarter as f32,
            event_beats,
            global_transpose,
            source,
            emission.event,
            landing,
            debug_accum,
        ) {
            break;
        }
        entry.generation = clock.seek_generation;
        entry.enqueued_sample = sample_time;
        retained.push(entry);
    }
}

/// A rack-owned generator addresses its rack's members (docs/jaki-kind-spec.md
/// §4): emission `:track` n is member n, the same member-relative rule graph
/// routes use, so the instance plays its own pads wherever the rack sits. A
/// member index past the rack's size (or an untracked emission) drops.
fn map_rack_member_emissions(
    emitted: &mut Vec<crate::lisp_host::EmittedAccumulatorEvent>,
    members: &[usize],
) {
    emitted.retain_mut(|event| match event.track {
        Some(member) => match members.get(member) {
            Some(track) => {
                event.track = Some(*track);
                true
            }
            None => false,
        },
        None => false,
    });
}
