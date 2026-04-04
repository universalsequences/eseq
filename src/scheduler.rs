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
use crate::lisp_effect;
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
            let mut last_playing = false;
            let lookahead_target_samples = (scheduler_block_size.max(1) * 4) as u64;
            let mut accumulator_states = [AccumulatorRuntimeState::default(); MAX_TRACKS];
            let mut pending_accum_reset = [false; MAX_TRACKS];
            let mut scratch_source_version = u64::MAX;
            let mut scratch_runtime = None;

            loop {
                let snapshot = state.latest_scheduler_snapshot();
                let playing = snapshot.transport.playing;
                let pattern = snapshot.transport.current_pattern;
                let pattern_epoch = snapshot.transport.pattern_epoch;
                let rendered = rendered_samples.load(Ordering::Acquire);
                let latest_scratch_source_version = state.scratch_source_version();
                let (reset_all, reset_tracks) = state.take_accumulator_reset_requests();

                if latest_scratch_source_version != scratch_source_version {
                    let source = state.scratch_source();
                    if source.trim().is_empty() {
                        scratch_runtime = None;
                    } else {
                        let mut runtime =
                            lisp_effect::scratch_runtime_with_fallbacks(Arc::clone(&state), 0, 0);
                        if runtime.eval(&source).is_ok() {
                            scratch_runtime = Some(runtime);
                        } else {
                            scratch_runtime = None;
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
                    pending_accum_reset = [false; MAX_TRACKS];
                    accumulator_states = [AccumulatorRuntimeState::default(); MAX_TRACKS];
                    thread::sleep(Duration::from_millis(2));
                    continue;
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
                        let swing_pct = step_snapshot
                            .swing_override
                            .unwrap_or(track.params.swing);
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
                            if let (Some(runtime), Some(script_idx)) =
                                (scratch_runtime.as_mut(), script_idx)
                            {
                                runtime.set_position(trigger.track, trigger.step);
                                if let Ok(output) = runtime.invoke_accumulator(
                                    script_idx,
                                    trigger.step,
                                    rs.value,
                                    resolved,
                                    track.effect_slots.clone(),
                                    track.instrument_slot.clone(),
                                    effect_params,
                                    instrument_params,
                                ) {
                                    resolved = output.resolved;
                                    let actions = crate::accumulator::ActionBuffer::just(
                                        StepAction::Play(resolved),
                                    );
                                    // Stash script-mutated params in the resolved event path below.
                                    let script_effect_params = output.effect_params;
                                    let script_instrument_params = output.instrument_params;
                                    for action in actions.iter() {
                                        let (target_track, resolved) = match *action {
                                            StepAction::Play(resolved) => (trigger.track, resolved),
                                            StepAction::SendToTrack { track, resolved } => {
                                                (track, resolved)
                                            }
                                            StepAction::Silence => continue,
                                        };
                                        if target_track >= snapshot.tracks.len() {
                                            continue;
                                        }
                                        let target_step =
                                            &snapshot.tracks[target_track].steps[trigger.step];
                                        let instrument_fingerprint = instrument_sound_fingerprint(
                                            &snapshot,
                                            target_track,
                                            &script_instrument_params,
                                        );
                                        let mut chord = ScheduledChordData {
                                            count: target_step.chord.len().min(MAX_VOICES),
                                            notes: [0.0; MAX_VOICES],
                                            step_transpose: target_step.params
                                                [StepParam::Transpose.index()],
                                        };
                                        for (idx, note) in
                                            target_step.chord.iter().take(MAX_VOICES).enumerate()
                                        {
                                            chord.notes[idx] = *note;
                                        }
                                        if queue
                                            .push(ScheduledEvent {
                                                pattern_epoch,
                                                sample_time,
                                                kind: ScheduledEventKind::ResolvedTrigger {
                                                    track: target_track,
                                                    step: trigger.step,
                                                    samples_per_step: trigger.samples_per_step,
                                                    resolved,
                                                    chord,
                                                    effect_params: script_effect_params.clone(),
                                                    instrument_params: script_instrument_params
                                                        .clone(),
                                                    instrument_fingerprint,
                                                },
                                            })
                                            .is_err()
                                        {
                                            chunk_enqueued = false;
                                        }
                                    }
                                    if !chunk_enqueued {
                                        break;
                                    }
                                    continue;
                                }
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
                            let target_step = &snapshot.tracks[target_track].steps[trigger.step];
                            let effect_params =
                                resolve_effect_params(&snapshot, target_track, trigger.step);
                            let instrument_params =
                                resolve_instrument_params(&snapshot, target_track, trigger.step);
                            let instrument_fingerprint = instrument_sound_fingerprint(
                                &snapshot,
                                target_track,
                                &instrument_params,
                            );
                            let mut chord = ScheduledChordData {
                                count: target_step.chord.len().min(MAX_VOICES),
                                notes: [0.0; MAX_VOICES],
                                step_transpose: target_step.params[StepParam::Transpose.index()],
                            };
                            for (idx, note) in target_step.chord.iter().take(MAX_VOICES).enumerate()
                            {
                                chord.notes[idx] = *note;
                            }

                            if queue
                                .push(ScheduledEvent {
                                    pattern_epoch,
                                    sample_time,
                                    kind: ScheduledEventKind::ResolvedTrigger {
                                        track: target_track,
                                        step: trigger.step,
                                        samples_per_step: trigger.samples_per_step,
                                        resolved,
                                        chord,
                                        effect_params,
                                        instrument_params,
                                        instrument_fingerprint,
                                    },
                                })
                                .is_err()
                            {
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
                thread::sleep(Duration::from_millis(1));
            }
        });
}

#[cfg(test)]
mod tests {
    use super::SnapshotSequencerClock;
    use crate::sequencer::{default_empty_effect_chain, SequencerState};

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
}
