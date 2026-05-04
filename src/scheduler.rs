use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use crate::accumulator::{
    apply_limit_mode, AccumMode, AccumulatorRuntimeState, ResolvedStep, StepAction,
    ACCUMULATOR_REGISTRY,
};
use crate::lisp_effect::{self, AccumulatorNoteSpan};
use crate::scheduled_event::{
    ScheduledChordData, ScheduledEffectParam, ScheduledEvent, ScheduledEventKind,
    ScheduledEventQueue, ScheduledInstrumentParam, ScheduledInstrumentParamTarget,
};
use crate::sequencer::{
    sync_beats, SequencerSnapshot, SequencerState, StepParam, SwingResolution, MAX_STEPS,
    MAX_TRACKS,
};
use crate::voice::MAX_VOICES;

fn ceil_to_grid(value: f64, grid: f64) -> f64 {
    let rem = value % grid;
    if rem > 1e-9 {
        value + (grid - rem)
    } else {
        value
    }
}

#[derive(Clone, Copy)]
struct SnapshotTrigger {
    track: usize,
    step: usize,
    offset: usize,
    cycle_start_beats: f64,
    samples_per_step: f32,
}

struct SnapshotTrackClockState {
    last_local_step: u32,
    boundaries: [f64; MAX_STEPS + 1],
    step_ends: [f64; MAX_STEPS],
    cycle_beats: f64,
}

struct SnapshotSequencerClock {
    sample_rate: f64,
    total_beats: f64,
    track_clocks: Vec<SnapshotTrackClockState>,
    was_playing: bool,
}

impl SnapshotSequencerClock {
    fn new(sample_rate: u32) -> Self {
        let track_clocks = (0..MAX_TRACKS)
            .map(|_| SnapshotTrackClockState {
                last_local_step: u32::MAX,
                boundaries: [0.0; MAX_STEPS + 1],
                step_ends: [0.0; MAX_STEPS],
                cycle_beats: 4.0,
            })
            .collect();
        Self {
            sample_rate: sample_rate as f64,
            total_beats: 0.0,
            track_clocks,
            was_playing: false,
        }
    }

    fn reset(&mut self) {
        self.total_beats = 0.0;
        self.was_playing = false;
        for track in &mut self.track_clocks {
            track.last_local_step = u32::MAX;
        }
    }

    fn seek_to_rendered_position(
        &mut self,
        snapshot: &SequencerSnapshot,
        rendered_sample: u64,
        scheduled_until_sample: u64,
    ) {
        let bpm = snapshot.transport.bpm as f64;
        let beats_per_sample = bpm / (self.sample_rate * 60.0);
        let ahead_samples = scheduled_until_sample.saturating_sub(rendered_sample) as f64;
        self.total_beats = (self.total_beats - ahead_samples * beats_per_sample).max(0.0);
        self.was_playing = snapshot.transport.playing;

        let num_tracks = snapshot.transport.num_tracks;
        for t in 0..num_tracks {
            self.precompute_boundaries(snapshot, t);
            let ns = snapshot.tracks[t].params.num_steps;
            let tc = &self.track_clocks[t];
            let pos_in_cycle = self.total_beats % tc.cycle_beats;
            self.track_clocks[t].last_local_step = Self::derive_local_step(tc, pos_in_cycle, ns)
                .map(|step| step as u32)
                .unwrap_or(u32::MAX);
        }
        for t in num_tracks..MAX_TRACKS {
            self.track_clocks[t].last_local_step = u32::MAX;
        }
    }

    fn precompute_boundaries(&mut self, snapshot: &SequencerSnapshot, track: usize) {
        const EPS: f64 = 1e-9;
        let track_snapshot = &snapshot.tracks[track];
        let ns = track_snapshot.params.num_steps;
        let default_tb = track_snapshot.params.timebase;
        let tc = &mut self.track_clocks[track];

        let mut accum = 0.0;
        for s in 0..ns {
            let tb = track_snapshot.steps[s]
                .timebase_override
                .unwrap_or(default_tb);
            let step_dur = tb.step_beats(ns);

            let sync_b = sync_beats(track_snapshot.steps[s].params[StepParam::Sync.index()]);
            if sync_b > EPS {
                accum = ceil_to_grid(accum, sync_b);
            }

            tc.boundaries[s] = accum;
            tc.step_ends[s] = accum + step_dur;
            accum += step_dur;
        }
        tc.boundaries[ns] = accum;

        let sync0_b = sync_beats(track_snapshot.steps[0].params[StepParam::Sync.index()]);
        tc.cycle_beats = if sync0_b > EPS {
            ceil_to_grid(accum, sync0_b).max(EPS)
        } else {
            accum.max(EPS)
        };
    }

    fn derive_local_step(
        tc: &SnapshotTrackClockState,
        pos_in_cycle: f64,
        num_steps: usize,
    ) -> Option<usize> {
        if pos_in_cycle >= tc.boundaries[num_steps] {
            return None;
        }
        let idx = tc.boundaries[..num_steps + 1].partition_point(|&b| b <= pos_in_cycle);
        let s = if idx > 0 { idx - 1 } else { 0 };
        if pos_in_cycle < tc.step_ends[s] {
            Some(s)
        } else {
            None
        }
    }

    fn process_chunk(
        &mut self,
        nframes: usize,
        snapshot: &SequencerSnapshot,
        state: &SequencerState,
    ) -> Vec<SnapshotTrigger> {
        if !snapshot.transport.playing {
            self.reset();
            return Vec::new();
        }

        let bpm = snapshot.transport.bpm as f64;
        let beats_per_sample = bpm / (self.sample_rate * 60.0);
        let samples_per_quarter = self.sample_rate * 60.0 / bpm;
        let num_tracks = snapshot.transport.num_tracks;

        if !self.was_playing {
            self.was_playing = true;
            self.total_beats = 0.0;
            for t in 0..MAX_TRACKS {
                self.track_clocks[t].last_local_step = u32::MAX;
            }
        }

        for t in 0..num_tracks {
            self.precompute_boundaries(snapshot, t);
        }

        let mut triggers = Vec::new();
        let mut last_global_16th = (self.total_beats / 0.25) as u32;
        let mut last_bar = (self.total_beats / 4.0) as u32;
        for offset in 0..nframes {
            self.total_beats += beats_per_sample;

            let global_16th = (self.total_beats / 0.25) as u32;
            if global_16th != last_global_16th {
                state
                    .transport
                    .playhead
                    .store(global_16th, Ordering::Relaxed);
                last_global_16th = global_16th;
            }

            let bar = (self.total_beats / 4.0) as u32;
            if bar != last_bar {
                last_bar = bar;
                if state
                    .transport
                    .pending_mod_resync
                    .swap(false, Ordering::Relaxed)
                {
                    state
                        .transport
                        .mod_reset_counter
                        .fetch_add(1, Ordering::Relaxed);
                }
            }

            for t in 0..num_tracks {
                let track = &snapshot.tracks[t];
                let ns = track.params.num_steps;
                let tc = &self.track_clocks[t];
                let cycle = tc.cycle_beats;
                if cycle <= 0.0 {
                    continue;
                }
                let pos_in_cycle = self.total_beats % cycle;
                match Self::derive_local_step(tc, pos_in_cycle, ns) {
                    Some(step) => {
                        let step_u32 = step as u32;
                        if step_u32 != self.track_clocks[t].last_local_step {
                            let tc = &mut self.track_clocks[t];
                            tc.last_local_step = step_u32;
                            let tb = track.steps[step]
                                .timebase_override
                                .unwrap_or(track.params.timebase);
                            let samples_per_step = (tb.step_beats(ns) * samples_per_quarter) as f32;
                            triggers.push(SnapshotTrigger {
                                track: t,
                                step,
                                offset,
                                cycle_start_beats: tc.boundaries[step],
                                samples_per_step,
                            });
                            state.transport.track_playheads[t].store(step_u32, Ordering::Relaxed);
                        }
                    }
                    None => {
                        self.track_clocks[t].last_local_step = u32::MAX;
                    }
                }
            }
        }

        let phase_16th = (self.total_beats / 0.25).fract() as f32;
        state
            .transport
            .playhead_phase
            .store(phase_16th.to_bits(), Ordering::Relaxed);

        triggers
    }
}

fn swing_bucket_index(cycle_start_beats: f64, resolution: SwingResolution) -> u64 {
    const EPS: f64 = 1e-9;
    ((cycle_start_beats + EPS) / resolution.step_beats()).floor() as u64
}

fn swing_delay_samples(
    sample_rate: f64,
    bpm: f64,
    swing_pct: f32,
    resolution: SwingResolution,
) -> f64 {
    let samples_per_quarter = sample_rate * 60.0 / bpm;
    let resolution_samples = resolution.step_beats() * samples_per_quarter;
    ((swing_pct as f64 / 100.0) - 0.5) * 2.0 * resolution_samples
}

fn resolve_effect_params(
    snapshot: &SequencerSnapshot,
    track_idx: usize,
    step_idx: usize,
) -> Vec<ScheduledEffectParam> {
    let mut params = Vec::new();
    for slot in &snapshot.tracks[track_idx].effect_slots {
        if slot.node_id == 0 {
            continue;
        }
        let num_params = slot.num_params as usize;
        for param_idx in 0..num_params {
            let idx = slot
                .param_node_indices
                .get(param_idx)
                .copied()
                .unwrap_or(param_idx as u32) as u64;
            if idx == u32::MAX as u64 {
                continue;
            }
            let value = slot
                .plocks
                .get(step_idx)
                .and_then(|step| step.get(param_idx))
                .copied()
                .flatten()
                .unwrap_or_else(|| slot.defaults.get(param_idx).copied().unwrap_or(0.0));
            if !value.is_finite() {
                continue;
            }
            params.push(ScheduledEffectParam {
                logical_id: slot.node_id as u64,
                idx,
                value,
            });
        }
    }
    params
}

fn resolve_instrument_params(
    snapshot: &SequencerSnapshot,
    track_idx: usize,
    step_idx: usize,
) -> Vec<ScheduledInstrumentParam> {
    let slot = &snapshot.tracks[track_idx].instrument_slot;
    let num_params = slot.num_params as usize;
    let mut params = Vec::with_capacity(num_params);
    for param_idx in 0..num_params {
        let raw_idx = slot
            .param_node_indices
            .get(param_idx)
            .copied()
            .unwrap_or(param_idx as u32);
        let (target, idx) = if raw_idx >= crate::voice_modulator::MOD_PARAM_BASE {
            (
                ScheduledInstrumentParamTarget::Modulator,
                (raw_idx - crate::voice_modulator::MOD_PARAM_BASE) as u64,
            )
        } else {
            (ScheduledInstrumentParamTarget::Synth, raw_idx as u64)
        };
        let value = slot
            .plocks
            .get(step_idx)
            .and_then(|step| step.get(param_idx))
            .copied()
            .flatten()
            .unwrap_or_else(|| slot.defaults.get(param_idx).copied().unwrap_or(0.0));
        if !value.is_finite() {
            continue;
        }
        params.push(ScheduledInstrumentParam { target, idx, value });
    }
    params
}

fn instrument_sound_fingerprint(
    snapshot: &SequencerSnapshot,
    track_idx: usize,
    instrument_params: &[ScheduledInstrumentParam],
) -> u64 {
    let track = &snapshot.tracks[track_idx];
    let mut hasher = DefaultHasher::new();
    track.engine_id.hash(&mut hasher);
    track
        .instrument_base_note_offset
        .to_bits()
        .hash(&mut hasher);
    for param in instrument_params {
        param.target.hash(&mut hasher);
        param.idx.hash(&mut hasher);
        param.value.to_bits().hash(&mut hasher);
    }
    hasher.finish()
}

fn chord_data_from_parts(
    notes: &[f32],
    durations: &[f32],
    fallback_duration: f32,
    step_transpose: f32,
) -> ScheduledChordData {
    let mut chord = ScheduledChordData {
        count: notes.len().min(MAX_VOICES),
        notes: [0.0; MAX_VOICES],
        durations: [0.0; MAX_VOICES],
        step_transpose,
    };
    for (idx, note) in notes.iter().take(MAX_VOICES).enumerate() {
        chord.notes[idx] = *note;
        chord.durations[idx] = durations
            .get(idx)
            .copied()
            .filter(|duration| *duration > 0.0)
            .unwrap_or(fallback_duration);
    }
    chord
}

fn step_chord_data(
    snapshot: &SequencerSnapshot,
    track_idx: usize,
    step_idx: usize,
) -> ScheduledChordData {
    let step = &snapshot.tracks[track_idx].steps[step_idx];
    chord_data_from_parts(
        &step.chord,
        &step.chord_durations,
        step.params[StepParam::Duration.index()],
        step.params[StepParam::Transpose.index()],
    )
}

fn track_step_boundaries(track: &crate::sequencer::SequencerTrackSnapshot) -> Vec<f32> {
    const EPS: f64 = 1e-9;
    let ns = track.params.num_steps;
    let mut boundaries = vec![0.0_f32; ns + 1];
    let mut accum = 0.0_f64;
    for step in 0..ns {
        let tb = track.steps[step]
            .timebase_override
            .unwrap_or(track.params.timebase);
        let sync_b = sync_beats(track.steps[step].params[StepParam::Sync.index()]);
        if sync_b > EPS {
            accum = ceil_to_grid(accum, sync_b);
        }
        boundaries[step] = accum as f32;
        accum += tb.step_beats(ns);
    }
    boundaries[ns] = accum as f32;
    boundaries
}

fn track_note_spans_for_trigger(
    snapshot: &SequencerSnapshot,
    track_idx: usize,
    step_idx: usize,
) -> Vec<AccumulatorNoteSpan> {
    const EPS: f32 = 1e-5;
    let Some(track) = snapshot.tracks.get(track_idx) else {
        return Vec::new();
    };
    let ns = track.params.num_steps;
    if step_idx >= ns {
        return Vec::new();
    }
    let boundaries = track_step_boundaries(track);
    let trigger_start = boundaries[step_idx];
    let mut candidates = Vec::new();

    for step in 0..ns {
        let step_snapshot = &track.steps[step];
        if !step_snapshot.active {
            continue;
        }
        let step_start = boundaries[step];
        let step_beats = step_snapshot
            .timebase_override
            .unwrap_or(track.params.timebase)
            .step_beats(ns) as f32;
        if step_beats <= 0.0 {
            continue;
        }
        let fallback_duration = step_snapshot.params[StepParam::Duration.index()].max(0.0);
        if step_snapshot.chord.is_empty() {
            candidates.push(AccumulatorNoteSpan {
                transpose: step_snapshot.params[StepParam::Transpose.index()],
                start_beats: step_start,
                end_beats: step_start + fallback_duration * step_beats,
            });
        } else {
            for (idx, note) in step_snapshot.chord.iter().enumerate() {
                let duration = step_snapshot
                    .chord_durations
                    .get(idx)
                    .copied()
                    .filter(|duration| *duration > 0.0)
                    .unwrap_or(fallback_duration)
                    .max(0.0);
                candidates.push(AccumulatorNoteSpan {
                    transpose: *note,
                    start_beats: step_start,
                    end_beats: step_start + duration * step_beats,
                });
            }
        }
    }

    candidates.sort_by(|a, b| {
        a.start_beats
            .partial_cmp(&b.start_beats)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    if candidates
        .iter()
        .any(|note| note.start_beats < trigger_start - EPS && note.end_beats > trigger_start + EPS)
    {
        return Vec::new();
    }

    let mut group_end = candidates
        .iter()
        .filter(|note| (note.start_beats - trigger_start).abs() <= EPS)
        .map(|note| note.end_beats)
        .fold(trigger_start, f32::max);
    if group_end <= trigger_start + EPS {
        return Vec::new();
    }

    let mut out = Vec::new();
    for note in candidates {
        if note.start_beats < trigger_start - EPS {
            continue;
        }
        if note.start_beats > trigger_start + EPS && note.start_beats >= group_end - EPS {
            break;
        }
        if note.end_beats <= trigger_start + EPS {
            continue;
        }
        group_end = group_end.max(note.end_beats);
        out.push(AccumulatorNoteSpan {
            transpose: note.transpose,
            start_beats: (note.start_beats - trigger_start).max(0.0),
            end_beats: (note.end_beats - trigger_start).max(0.0),
        });
    }
    out
}

fn enqueue_resolved_trigger(
    queue: &ScheduledEventQueue<4096>,
    snapshot: &SequencerSnapshot,
    pattern_epoch: u64,
    sample_time: u64,
    track_idx: usize,
    step_idx: usize,
    samples_per_step: f32,
    resolved: ResolvedStep,
    chord: ScheduledChordData,
    effect_params: Vec<ScheduledEffectParam>,
    instrument_params: Vec<ScheduledInstrumentParam>,
) -> bool {
    let instrument_fingerprint =
        instrument_sound_fingerprint(snapshot, track_idx, &instrument_params);
    queue
        .push(ScheduledEvent {
            pattern_epoch,
            sample_time,
            kind: ScheduledEventKind::ResolvedTrigger {
                track: track_idx,
                step: step_idx,
                samples_per_step,
                resolved,
                chord,
                effect_params,
                instrument_params,
                instrument_fingerprint,
            },
        })
        .is_ok()
}

pub fn spawn_scheduler_thread(
    state: Arc<SequencerState>,
    sample_rate: u32,
    scheduler_block_size: usize,
    rendered_samples: Arc<AtomicU64>,
    queue: Arc<ScheduledEventQueue<4096>>,
) {
    let _ = thread::Builder::new()
        .name("sequencer-scheduler".to_string())
        .spawn(move || {
            let mut clock = SnapshotSequencerClock::new(sample_rate);
            let mut scheduled_until_sample = 0u64;
            let mut last_pattern = usize::MAX;
            let mut last_pattern_epoch = u64::MAX;
            let mut last_topology_epoch = u64::MAX;
            let mut last_playing = false;
            let lookahead_target_samples = (scheduler_block_size.max(1) * 4) as u64;
            let mut accumulator_states = [AccumulatorRuntimeState::default(); MAX_TRACKS];
            let mut pending_accum_reset = [false; MAX_TRACKS];
            let mut scratch_source_version = u64::MAX;
            let mut scratch_runtime = None;
            let debug_accum = std::env::var_os("TINYSEQ_DEBUG_ACCUM").is_some();
            let mut debug_accum_invocations = 0_u64;

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

                if latest_scratch_source_version != scratch_source_version {
                    let source = state.scratch_source();
                    if debug_accum {
                        eprintln!(
                            "[accum] scratch source version {} -> {} bytes={}",
                            scratch_source_version,
                            latest_scratch_source_version,
                            source.len()
                        );
                    }
                    if source.trim().is_empty() {
                        if debug_accum {
                            eprintln!("[accum] scratch source empty; clearing runtime");
                        }
                        scratch_runtime = None;
                    } else {
                        let mut runtime =
                            lisp_effect::scratch_runtime_with_fallbacks(Arc::clone(&state), 0, 0);
                        match runtime.eval(&source) {
                            Ok(_) => {
                                if debug_accum {
                                    let status = runtime.take_status_message();
                                    eprintln!(
                                        "[accum] scratch eval ok names={:?} status={:?}",
                                        runtime.accumulator_names(),
                                        status
                                    );
                                    for track_idx in 0..state.active_track_count().min(MAX_TRACKS) {
                                        let params = &state.pattern.track_params[track_idx];
                                        eprintln!(
                                            "[accum] state track={} acc_idx={} script={:?}",
                                            track_idx,
                                            params.get_accumulator_idx(),
                                            params.script_accumulator_name()
                                        );
                                    }
                                }
                                scratch_runtime = Some(runtime);
                            }
                            Err(err) => {
                                if debug_accum {
                                    let status = runtime.take_status_message();
                                    eprintln!(
                                        "[accum] scratch eval err={} status={:?}",
                                        err, status
                                    );
                                }
                                scratch_runtime = None;
                            }
                        }
                    }
                    scratch_source_version = latest_scratch_source_version;
                }

                if !playing {
                    queue.clear();
                    clock.reset();
                    scheduled_until_sample = rendered;
                    last_playing = false;
                    last_pattern = pattern;
                    last_pattern_epoch = pattern_epoch;
                    last_topology_epoch = topology_epoch;
                    pending_accum_reset = [false; MAX_TRACKS];
                    accumulator_states = [AccumulatorRuntimeState::default(); MAX_TRACKS];
                    thread::sleep(Duration::from_millis(2));
                    continue;
                }

                if topology_edit_in_flight {
                    queue.clear();
                    // Freeze future scheduling while the topology edit is in
                    // flight, but preserve the clock's current musical phase
                    // so resuming after the edit does not jump backwards.
                    scheduled_until_sample = rendered;
                    pending_accum_reset = [true; MAX_TRACKS];
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
                    for track_idx in 0..MAX_TRACKS {
                        pending_accum_reset[track_idx] = false;
                        if let Some(def) = ACCUMULATOR_REGISTRY.get(
                            snapshot
                                .tracks
                                .get(track_idx)
                                .map(|t| t.params.accumulator_idx)
                                .unwrap_or(0),
                        ) {
                            accumulator_states[track_idx] = AccumulatorRuntimeState {
                                value: def.reset_value,
                                reversed: false,
                            };
                        } else {
                            accumulator_states[track_idx] = AccumulatorRuntimeState::default();
                        }
                    }
                }
                for track_idx in 0..MAX_TRACKS {
                    if !reset_tracks[track_idx] {
                        continue;
                    }
                    pending_accum_reset[track_idx] = false;
                    if let Some(def) = ACCUMULATOR_REGISTRY.get(
                        snapshot
                            .tracks
                            .get(track_idx)
                            .map(|t| t.params.accumulator_idx)
                            .unwrap_or(0),
                    ) {
                        accumulator_states[track_idx] = AccumulatorRuntimeState {
                            value: def.reset_value,
                            reversed: false,
                        };
                    } else {
                        accumulator_states[track_idx] = AccumulatorRuntimeState::default();
                    }
                }

                if !last_playing {
                    queue.clear();
                    clock.reset();
                    scheduled_until_sample = rendered;
                    pending_accum_reset = [true; MAX_TRACKS];
                } else if last_topology_epoch != topology_epoch {
                    let previous_scheduled_until = scheduled_until_sample;
                    queue.clear();
                    clock.seek_to_rendered_position(&snapshot, rendered, previous_scheduled_until);
                    scheduled_until_sample = rendered;
                    pending_accum_reset = [true; MAX_TRACKS];
                } else if last_pattern_epoch != pattern_epoch {
                    // Track topology edits bump pattern_epoch without changing the
                    // pattern index. Rebuild the scheduler horizon immediately so
                    // future triggers target the compacted live track layout.
                    let previous_scheduled_until = scheduled_until_sample;
                    queue.clear();
                    clock.seek_to_rendered_position(&snapshot, rendered, previous_scheduled_until);
                    scheduled_until_sample = rendered;
                    pending_accum_reset = [true; MAX_TRACKS];
                } else if last_pattern != pattern {
                    // Pattern switches should replace future scheduled content without
                    // disturbing the current musical phase.
                    let previous_scheduled_until = scheduled_until_sample;
                    queue.clear();
                    clock.seek_to_rendered_position(&snapshot, rendered, previous_scheduled_until);
                    scheduled_until_sample = rendered;
                    pending_accum_reset = [true; MAX_TRACKS];
                }

                while scheduled_until_sample < rendered.saturating_add(lookahead_target_samples) {
                    let triggers = clock.process_chunk(scheduler_block_size, &snapshot, &state);
                    let mut chunk_enqueued = true;
                    for trigger in triggers {
                        if !snapshot.tracks[trigger.track].steps[trigger.step].active {
                            continue;
                        }
                        let track = &snapshot.tracks[trigger.track];
                        if trigger.step == 0 && pending_accum_reset[trigger.track] {
                            pending_accum_reset[trigger.track] = false;
                            if let Some(def) =
                                ACCUMULATOR_REGISTRY.get(track.params.accumulator_idx)
                            {
                                accumulator_states[trigger.track] = AccumulatorRuntimeState {
                                    value: def.reset_value,
                                    reversed: false,
                                };
                            } else {
                                accumulator_states[trigger.track] =
                                    AccumulatorRuntimeState::default();
                            }
                        }
                        let step_snapshot = &track.steps[trigger.step];
                        let swing_pct = step_snapshot.swing_override.unwrap_or(track.params.swing);
                        let swing_resolution = step_snapshot
                            .swing_resolution_override
                            .unwrap_or(track.params.swing_resolution);
                        let swing_step =
                            swing_bucket_index(trigger.cycle_start_beats, swing_resolution);
                        let is_odd_step = swing_step % 2 == 1;
                        let mut sample_time = scheduled_until_sample + trigger.offset as u64;
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

                        let resolved = ResolvedStep {
                            duration: step_snapshot.params[StepParam::Duration.index()],
                            velocity: step_snapshot.params[StepParam::Velocity.index()],
                            speed: step_snapshot.params[StepParam::Speed.index()],
                            aux_a: step_snapshot.params[StepParam::AuxA.index()],
                            aux_b: step_snapshot.params[StepParam::AuxB.index()],
                            transpose: step_snapshot.params[StepParam::Transpose.index()],
                            pan: step_snapshot.params[StepParam::Pan.index()],
                            chop: step_snapshot.params[StepParam::Chop.index()],
                        };
                        let rs = &mut accumulator_states[trigger.track];
                        let builtin_count = ACCUMULATOR_REGISTRY.len();
                        let actions = if let Some(def) =
                            ACCUMULATOR_REGISTRY.get(track.params.accumulator_idx)
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
                            let effect_params =
                                resolve_effect_params(&snapshot, trigger.track, trigger.step);
                            let instrument_params =
                                resolve_instrument_params(&snapshot, trigger.track, trigger.step);
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
                                let debug_note_spans = track_note_spans_for_trigger(
                                    &snapshot,
                                    trigger.track,
                                    trigger.step,
                                );
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
                            if let (Some(runtime), Some(script_idx)) =
                                (scratch_runtime.as_mut(), script_idx)
                            {
                                let note_spans = track_note_spans_for_trigger(
                                    &snapshot,
                                    trigger.track,
                                    trigger.step,
                                );
                                runtime.set_position(trigger.track, trigger.step);
                                match runtime.invoke_accumulator(
                                    script_idx,
                                    trigger.step,
                                    rs.value,
                                    resolved,
                                    step_snapshot.chord.clone(),
                                    step_snapshot.chord_durations.clone(),
                                    step_snapshot.params[StepParam::Transpose.index()],
                                    Some(note_spans),
                                    trigger.samples_per_step
                                        / (sample_rate as f32 * 60.0
                                            / snapshot.transport.bpm as f32),
                                    track.params.num_steps,
                                    track.effect_slots.clone(),
                                    track.instrument_slot.clone(),
                                    effect_params,
                                    instrument_params,
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
                                            for (idx, emitted) in
                                                output.emitted.iter().take(12).enumerate()
                                            {
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
                                        debug_accum_invocations =
                                            debug_accum_invocations.saturating_add(1);
                                        if !output.suppressed {
                                            let chord =
                                                step_chord_data(&snapshot, trigger.track, trigger.step);
                                            if !enqueue_resolved_trigger(
                                                &queue,
                                                &snapshot,
                                                pattern_epoch,
                                                sample_time,
                                                trigger.track,
                                                trigger.step,
                                                trigger.samples_per_step,
                                                output.resolved,
                                                chord,
                                                output.effect_params.clone(),
                                                output.instrument_params.clone(),
                                            ) {
                                                chunk_enqueued = false;
                                            }
                                        }
                                        let samples_per_quarter =
                                            sample_rate as f32 * 60.0 / snapshot.transport.bpm as f32;
                                        for emitted in output.emitted {
                                            let target_track = emitted.track.unwrap_or(trigger.track);
                                            if target_track >= snapshot.tracks.len() {
                                                continue;
                                            }
                                            let emitted_sample_time = sample_time.saturating_add(
                                                (emitted.offset_beats.max(0.0) * samples_per_quarter)
                                                    .round()
                                                    as u64,
                                            );
                                            let chord = chord_data_from_parts(
                                                &emitted.chord,
                                                &emitted.chord_durations,
                                                emitted.resolved.duration,
                                                emitted.chord_step_transpose,
                                            );
                                            if !enqueue_resolved_trigger(
                                                &queue,
                                                &snapshot,
                                                pattern_epoch,
                                                emitted_sample_time,
                                                target_track,
                                                trigger.step,
                                                trigger.samples_per_step,
                                                emitted.resolved,
                                                chord,
                                                emitted.effect_params,
                                                emitted.instrument_params,
                                            ) {
                                                chunk_enqueued = false;
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
                                                trigger.track,
                                                trigger.step,
                                                script_idx,
                                                err
                                            );
                                        }
                                        debug_accum_invocations =
                                            debug_accum_invocations.saturating_add(1);
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
                                debug_accum_invocations =
                                    debug_accum_invocations.saturating_add(1);
                            }
                            crate::accumulator::ActionBuffer::just(StepAction::Play(resolved))
                        } else {
                            crate::accumulator::ActionBuffer::just(StepAction::Play(resolved))
                        };

                        for action in actions.iter() {
                            let (target_track, resolved) = match *action {
                                StepAction::Play(resolved) => (trigger.track, resolved),
                                StepAction::SendToTrack { track, resolved } => (track, resolved),
                                StepAction::Silence => continue,
                            };
                            if target_track >= snapshot.tracks.len() {
                                continue;
                            }
                            let effect_params =
                                resolve_effect_params(&snapshot, target_track, trigger.step);
                            let instrument_params =
                                resolve_instrument_params(&snapshot, target_track, trigger.step);
                            let chord = step_chord_data(&snapshot, target_track, trigger.step);
                            if !enqueue_resolved_trigger(
                                &queue,
                                &snapshot,
                                pattern_epoch,
                                sample_time,
                                target_track,
                                trigger.step,
                                trigger.samples_per_step,
                                resolved,
                                chord,
                                effect_params,
                                instrument_params,
                            ) {
                                chunk_enqueued = false;
                                break;
                            }
                        }
                        if !chunk_enqueued {
                            break;
                        }
                    }
                    if !chunk_enqueued {
                        break;
                    }
                    scheduled_until_sample =
                        scheduled_until_sample.saturating_add(scheduler_block_size as u64);
                }

                last_playing = playing;
                last_pattern = pattern;
                last_pattern_epoch = pattern_epoch;
                last_topology_epoch = topology_epoch;
                thread::sleep(Duration::from_millis(1));
            }
        });
}

#[cfg(test)]
mod tests {
    use super::{track_note_spans_for_trigger, SnapshotSequencerClock};
    use crate::sequencer::{default_empty_effect_chain, SequencerState, StepParam};

    #[test]
    fn snapshot_clock_emits_triggers_for_active_steps() {
        let state = SequencerState::new(1, vec![default_empty_effect_chain()]);
        state.toggle_play();
        state.toggle_step_and_clear_plocks(0, 0);
        let snapshot = state.latest_scheduler_snapshot();
        let mut clock = SnapshotSequencerClock::new(48_000);

        let triggers = clock.process_chunk(12_000, &snapshot, &state);
        assert!(!triggers.is_empty());
        assert_eq!(triggers[0].track, 0);
        assert_eq!(triggers[0].step, 0);
    }

    #[test]
    fn track_note_spans_fold_later_notes_into_running_group() {
        let state = SequencerState::new(1, vec![default_empty_effect_chain()]);
        let track = 0;

        state.pattern.patterns[track].set_step_active(0, true);
        state.pattern.chord_data[track].add_note_with_duration(0, 0.0, 8.0);
        state.pattern.chord_data[track].add_note_with_duration(0, 4.0, 8.0);
        state.pattern.step_data[track].set(0, StepParam::Duration, 8.0);

        state.pattern.patterns[track].set_step_active(4, true);
        state.pattern.step_data[track].set(4, StepParam::Transpose, 7.0);
        state.pattern.step_data[track].set(4, StepParam::Duration, 4.0);

        let snapshot = state.publish_scheduler_snapshot();
        let first_group = track_note_spans_for_trigger(&snapshot, track, 0);
        assert_eq!(first_group.len(), 3);
        assert_eq!(first_group[0].transpose, 0.0);
        assert_eq!(first_group[1].transpose, 4.0);
        assert_eq!(first_group[2].transpose, 7.0);
        assert_eq!(first_group[2].start_beats, 1.0);
        assert_eq!(first_group[2].end_beats, 2.0);

        let later_group = track_note_spans_for_trigger(&snapshot, track, 4);
        assert!(later_group.is_empty());
    }

    #[test]
    fn scheduler_note_grouping_follows_staggered_piano_roll_pattern() {
        let state = SequencerState::new(1, vec![default_empty_effect_chain()]);
        let track = 0;

        state.pattern.patterns[track].set_step_active(0, true);
        state.pattern.chord_data[track].add_note_with_duration(0, 0.0, 12.0);
        state.pattern.chord_data[track].add_note_with_duration(0, 7.0, 4.0);
        state.pattern.step_data[track].set(0, StepParam::Duration, 12.0);

        state.pattern.patterns[track].set_step_active(4, true);
        state.pattern.step_data[track].set(4, StepParam::Transpose, 12.0);
        state.pattern.step_data[track].set(4, StepParam::Duration, 8.0);

        state.pattern.patterns[track].set_step_active(8, true);
        state.pattern.step_data[track].set(8, StepParam::Transpose, 19.0);
        state.pattern.step_data[track].set(8, StepParam::Duration, 2.0);

        state.pattern.patterns[track].set_step_active(12, true);
        state.pattern.step_data[track].set(12, StepParam::Transpose, 24.0);
        state.pattern.step_data[track].set(12, StepParam::Duration, 4.0);

        state.toggle_play();
        let snapshot = state.publish_scheduler_snapshot();
        let mut clock = SnapshotSequencerClock::new(48_000);
        let triggers = clock.process_chunk(84_000, &snapshot, &state);
        let active_trigger_steps = triggers
            .iter()
            .filter(|trigger| snapshot.tracks[trigger.track].steps[trigger.step].active)
            .map(|trigger| trigger.step)
            .collect::<Vec<_>>();
        assert_eq!(active_trigger_steps, vec![0, 4, 8, 12]);

        let first_group = track_note_spans_for_trigger(&snapshot, track, 0);
        let first_transposes = first_group
            .iter()
            .map(|note| note.transpose)
            .collect::<Vec<_>>();
        let first_starts = first_group
            .iter()
            .map(|note| note.start_beats)
            .collect::<Vec<_>>();
        let first_ends = first_group
            .iter()
            .map(|note| note.end_beats)
            .collect::<Vec<_>>();
        assert_eq!(first_transposes, vec![0.0, 7.0, 12.0, 19.0]);
        assert_eq!(first_starts, vec![0.0, 0.0, 1.0, 2.0]);
        assert_eq!(first_ends, vec![3.0, 1.0, 3.0, 2.5]);

        assert!(track_note_spans_for_trigger(&snapshot, track, 4).is_empty());
        assert!(track_note_spans_for_trigger(&snapshot, track, 8).is_empty());

        let next_group = track_note_spans_for_trigger(&snapshot, track, 12);
        assert_eq!(next_group.len(), 1);
        assert_eq!(next_group[0].transpose, 24.0);
        assert_eq!(next_group[0].start_beats, 0.0);
        assert_eq!(next_group[0].end_beats, 1.0);
    }
}
