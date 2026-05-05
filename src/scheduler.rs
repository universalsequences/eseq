use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
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
    sync_beats, KeyboardTrigger, MidiFxPosition, SequencerSnapshot, SequencerState, StepParam,
    SwingResolution, MAX_STEPS, MAX_TRACKS,
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
    absolute_beats: f64,
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
                                absolute_beats: self.total_beats,
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

fn resolve_midi_fx_slot_param(
    snapshot: &SequencerSnapshot,
    track_idx: usize,
    slot_idx: usize,
    param_idx: usize,
    step_idx: usize,
) -> Option<f32> {
    let slot = snapshot
        .tracks
        .get(track_idx)?
        .midi_fx_slots
        .get(slot_idx)?;
    if param_idx >= slot.num_params as usize {
        return None;
    }
    Some(
        slot.plocks
            .get(step_idx)
            .and_then(|step| step.get(param_idx))
            .copied()
            .flatten()
            .unwrap_or_else(|| slot.defaults.get(param_idx).copied().unwrap_or(0.0)),
    )
}

fn live_midi_fx_tick_beats(snapshot: &SequencerSnapshot, track_idx: usize, step_idx: usize) -> f32 {
    resolve_midi_fx_slot_param(snapshot, track_idx, 0, 0, step_idx)
        .and_then(|idx| {
            crate::sequencer::Timebase::ALL
                .get(idx.round().max(0.0) as usize)
                .copied()
        })
        .map(|timebase| timebase.step_beats(snapshot.tracks[track_idx].params.num_steps) as f32)
        .filter(|beats| *beats > 0.0)
        .unwrap_or(0.25)
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

fn track_active_note_spans_at_beat(
    snapshot: &SequencerSnapshot,
    track_idx: usize,
    position_beats: f32,
    window_beats: f32,
) -> Vec<AccumulatorNoteSpan> {
    const EPS: f32 = 1e-5;
    let Some(track) = snapshot.tracks.get(track_idx) else {
        return Vec::new();
    };
    if window_beats <= 0.0 {
        return Vec::new();
    }
    let ns = track.params.num_steps;
    let boundaries = track_step_boundaries(track);
    let cycle_beats = boundaries.get(ns).copied().unwrap_or(0.0).max(EPS);
    let position = position_beats.rem_euclid(cycle_beats);
    let window_end = position + window_beats;
    let mut spans = Vec::new();

    for cycle_offset in [0.0, cycle_beats] {
        for step in 0..ns {
            let step_snapshot = &track.steps[step];
            if !step_snapshot.active {
                continue;
            }
            let step_start = boundaries[step] + cycle_offset;
            let step_beats = step_snapshot
                .timebase_override
                .unwrap_or(track.params.timebase)
                .step_beats(ns) as f32;
            if step_beats <= 0.0 {
                continue;
            }
            let fallback_duration = step_snapshot.params[StepParam::Duration.index()].max(0.0);
            if step_snapshot.chord.is_empty() {
                let note_end = step_start + fallback_duration * step_beats;
                if note_end > position + EPS && step_start < window_end - EPS {
                    spans.push(AccumulatorNoteSpan {
                        transpose: step_snapshot.params[StepParam::Transpose.index()],
                        start_beats: (step_start - position).max(0.0),
                        end_beats: (note_end - position).min(window_beats).max(0.0),
                    });
                }
            } else {
                for (idx, note) in step_snapshot.chord.iter().enumerate() {
                    let duration = step_snapshot
                        .chord_durations
                        .get(idx)
                        .copied()
                        .filter(|duration| *duration > 0.0)
                        .unwrap_or(fallback_duration)
                        .max(0.0);
                    let note_end = step_start + duration * step_beats;
                    if note_end > position + EPS && step_start < window_end - EPS {
                        spans.push(AccumulatorNoteSpan {
                            transpose: *note,
                            start_beats: (step_start - position).max(0.0),
                            end_beats: (note_end - position).min(window_beats).max(0.0),
                        });
                    }
                }
            }
        }
    }

    spans
        .into_iter()
        .filter(|span| span.end_beats > span.start_beats + EPS)
        .collect()
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

#[derive(Clone)]
struct MidiFxEvent {
    offset_beats: f32,
    track: usize,
    step: usize,
    samples_per_step: f32,
    step_beats: f32,
    resolved: ResolvedStep,
    chord: Vec<f32>,
    chord_durations: Vec<f32>,
    chord_step_transpose: f32,
    note_spans: Option<Vec<AccumulatorNoteSpan>>,
    arp_phase_beats: f32,
    effect_params: Vec<ScheduledEffectParam>,
    instrument_params: Vec<ScheduledInstrumentParam>,
}

#[derive(Clone, Copy)]
struct LiveMidiFxNote {
    transpose: f32,
    velocity: f32,
}

#[derive(Clone, Default)]
struct LiveMidiFxTrackState {
    notes: Vec<LiveMidiFxNote>,
    next_tick_sample: u64,
    quantize_next_tick: bool,
}

fn midi_fx_event_from_step(
    snapshot: &SequencerSnapshot,
    track_idx: usize,
    step_idx: usize,
    samples_per_step: f32,
    step_beats: f32,
    arp_phase_beats: f32,
    resolved: ResolvedStep,
    effect_params: Vec<ScheduledEffectParam>,
    instrument_params: Vec<ScheduledInstrumentParam>,
) -> MidiFxEvent {
    let step = &snapshot.tracks[track_idx].steps[step_idx];
    MidiFxEvent {
        offset_beats: 0.0,
        track: track_idx,
        step: step_idx,
        samples_per_step,
        step_beats,
        resolved,
        chord: step.chord.clone(),
        chord_durations: step.chord_durations.clone(),
        chord_step_transpose: step.params[StepParam::Transpose.index()],
        note_spans: Some(track_note_spans_for_trigger(snapshot, track_idx, step_idx)),
        arp_phase_beats,
        effect_params,
        instrument_params,
    }
}

fn run_midi_fx_chain_for_track(
    runtime: &mut lisp_effect::ScratchControlRuntime,
    snapshot: &SequencerSnapshot,
    source_track: usize,
    events: Vec<MidiFxEvent>,
    depth: usize,
    debug_accum: bool,
) -> Vec<MidiFxEvent> {
    if depth > 4 || source_track >= snapshot.tracks.len() {
        return events;
    }
    let chain = snapshot.tracks[source_track].params.midi_fx_chain.clone();
    if chain.is_empty() {
        return events;
    }
    let names = runtime.midi_fx_names();
    let mut current = events;
    for (stage_idx, fx_name) in chain.into_iter().enumerate() {
        let Some(fx_idx) = names
            .iter()
            .position(|name| name.eq_ignore_ascii_case(&fx_name))
        else {
            if debug_accum {
                eprintln!("[midi-fx] missing fx name={fx_name:?} track={source_track}");
            }
            continue;
        };
        let mut next = Vec::new();
        for event in current {
            if event.track != source_track {
                next.extend(run_midi_fx_chain_for_track(
                    runtime,
                    snapshot,
                    event.track,
                    vec![event],
                    depth + 1,
                    debug_accum,
                ));
                continue;
            }
            runtime.set_position(event.track, event.step);
            match runtime.invoke_midi_fx_with_arp_phase_beats(
                fx_idx,
                event.track,
                event.step,
                0.0,
                event.resolved,
                event.chord.clone(),
                event.chord_durations.clone(),
                event.chord_step_transpose,
                event.note_spans.clone(),
                snapshot.tracks[event.track]
                    .midi_fx_slots
                    .get(stage_idx)
                    .cloned()
                    .unwrap_or_else(crate::effects::EffectSlotSnapshot::new_empty),
                event.arp_phase_beats,
                event.step_beats,
                snapshot.tracks[event.track].params.num_steps,
                snapshot.tracks[event.track].effect_slots.clone(),
                snapshot.tracks[event.track].instrument_slot.clone(),
                event.effect_params.clone(),
                event.instrument_params.clone(),
            ) {
                Ok(output) => {
                    if !output.suppressed {
                        let mut passthrough = event.clone();
                        passthrough.resolved = output.resolved;
                        passthrough.effect_params = output.effect_params.clone();
                        passthrough.instrument_params = output.instrument_params.clone();
                        next.push(passthrough);
                    }
                    for emitted in output.emitted {
                        let target_track = emitted.track.unwrap_or(event.track);
                        if target_track >= snapshot.tracks.len() {
                            continue;
                        }
                        let routed = MidiFxEvent {
                            offset_beats: event.offset_beats + emitted.offset_beats,
                            track: target_track,
                            step: event.step,
                            samples_per_step: event.samples_per_step,
                            step_beats: event.step_beats,
                            resolved: emitted.resolved,
                            chord: emitted.chord,
                            chord_durations: emitted.chord_durations,
                            chord_step_transpose: emitted.chord_step_transpose,
                            note_spans: None,
                            arp_phase_beats: event.arp_phase_beats,
                            effect_params: emitted.effect_params,
                            instrument_params: emitted.instrument_params,
                        };
                        if target_track == source_track {
                            next.push(routed);
                        } else {
                            next.extend(run_midi_fx_chain_for_track(
                                runtime,
                                snapshot,
                                target_track,
                                vec![routed],
                                depth + 1,
                                debug_accum,
                            ));
                        }
                    }
                }
                Err(err) => {
                    if debug_accum {
                        eprintln!(
                            "[midi-fx] invoke err track={} step={} fx={} err={}",
                            event.track, event.step, fx_name, err
                        );
                    }
                    next.push(event);
                }
            }
            if next.len() > 1024 {
                next.truncate(1024);
                break;
            }
        }
        current = next;
    }
    current
}

fn enqueue_midi_fx_events(
    queue: &ScheduledEventQueue<4096>,
    snapshot: &SequencerSnapshot,
    pattern_epoch: u64,
    base_sample_time: u64,
    samples_per_quarter: f32,
    events: Vec<MidiFxEvent>,
) -> bool {
    let mut ok = true;
    for event in events {
        let sample_time = base_sample_time
            .saturating_add((event.offset_beats.max(0.0) * samples_per_quarter).round() as u64);
        let chord = chord_data_from_parts(
            &event.chord,
            &event.chord_durations,
            event.resolved.duration,
            event.chord_step_transpose,
        );
        if !enqueue_resolved_trigger(
            queue,
            snapshot,
            pattern_epoch,
            sample_time,
            event.track,
            event.step,
            event.samples_per_step,
            event.resolved,
            chord,
            event.effect_params,
            event.instrument_params,
        ) {
            ok = false;
            break;
        }
    }
    ok
}

fn drain_live_keyboard_inputs(
    live_keyboard_rx: &mpsc::Receiver<KeyboardTrigger>,
    snapshot: &SequencerSnapshot,
    rendered_sample: u64,
    live_tracks: &mut [LiveMidiFxTrackState; MAX_TRACKS],
) {
    while let Ok(trigger) = live_keyboard_rx.try_recv() {
        if trigger.track >= snapshot.tracks.len() || trigger.track >= MAX_TRACKS {
            continue;
        }
        let track_state = &mut live_tracks[trigger.track];
        if trigger.note_off {
            track_state
                .notes
                .retain(|note| note.transpose != trigger.transpose);
            if track_state.notes.is_empty() {
                track_state.next_tick_sample = 0;
                track_state.quantize_next_tick = false;
            }
            continue;
        }
        let was_empty = track_state.notes.is_empty();
        if let Some(note) = track_state
            .notes
            .iter_mut()
            .find(|note| note.transpose == trigger.transpose)
        {
            note.velocity = trigger.velocity;
        } else {
            track_state.notes.push(LiveMidiFxNote {
                transpose: trigger.transpose,
                velocity: trigger.velocity,
            });
        }
        if was_empty || track_state.next_tick_sample == 0 {
            track_state.next_tick_sample = rendered_sample;
            track_state.quantize_next_tick = true;
        }
    }
}

fn any_live_midi_fx_notes(live_tracks: &[LiveMidiFxTrackState; MAX_TRACKS]) -> bool {
    live_tracks.iter().any(|track| !track.notes.is_empty())
}

fn track_has_live_midi_fx_notes(
    live_tracks: &[LiveMidiFxTrackState; MAX_TRACKS],
    snapshot: &SequencerSnapshot,
    track_idx: usize,
) -> bool {
    track_idx < MAX_TRACKS
        && track_idx < snapshot.tracks.len()
        && !live_tracks[track_idx].notes.is_empty()
        && !snapshot.tracks[track_idx].params.midi_fx_chain.is_empty()
        && snapshot.tracks[track_idx].params.midi_fx_position == MidiFxPosition::PostAccumulator
}

fn quantized_live_tick_sample(
    rendered_sample: u64,
    rendered_total_beats: f64,
    live_tick_beats: f32,
    samples_per_quarter: f32,
) -> u64 {
    let beat_phase = rendered_total_beats.rem_euclid(live_tick_beats as f64);
    let beats_to_next_tick = if beat_phase <= 1e-6 {
        0.0
    } else {
        live_tick_beats as f64 - beat_phase
    };
    rendered_sample.saturating_add((beats_to_next_tick * samples_per_quarter as f64).round() as u64)
}

fn schedule_live_midi_fx(
    runtime: Option<&mut lisp_effect::ScratchControlRuntime>,
    state: &SequencerState,
    snapshot: &SequencerSnapshot,
    queue: &ScheduledEventQueue<4096>,
    pattern_epoch: u64,
    rendered_sample: u64,
    rendered_total_beats: f64,
    lookahead_samples: u64,
    sample_rate: u32,
    live_tracks: &mut [LiveMidiFxTrackState; MAX_TRACKS],
    debug_accum: bool,
) -> bool {
    let live_active = any_live_midi_fx_notes(live_tracks);
    let Some(runtime) = runtime else {
        return live_active;
    };
    if snapshot.transport.bpm == 0 {
        return live_active;
    }
    let samples_per_quarter = sample_rate as f32 * 60.0 / snapshot.transport.bpm as f32;
    let horizon = rendered_sample.saturating_add(lookahead_samples);

    for track_idx in 0..snapshot.tracks.len().min(MAX_TRACKS) {
        if live_tracks[track_idx].notes.is_empty()
            || snapshot.tracks[track_idx].params.midi_fx_chain.is_empty()
            || snapshot.tracks[track_idx].params.midi_fx_position != MidiFxPosition::PostAccumulator
        {
            continue;
        }
        if live_tracks[track_idx].next_tick_sample < rendered_sample {
            live_tracks[track_idx].next_tick_sample = rendered_sample;
        }
        while live_tracks[track_idx].next_tick_sample < horizon {
            let notes = live_tracks[track_idx].notes.clone();
            if notes.is_empty() {
                break;
            }
            let num_steps = snapshot.tracks[track_idx].params.num_steps.max(1);
            let step = (state.transport.track_playheads[track_idx].load(Ordering::Relaxed)
                as usize)
                % num_steps;
            let live_tick_beats = live_midi_fx_tick_beats(snapshot, track_idx, step);
            let live_tick_samples = (samples_per_quarter * live_tick_beats).round().max(1.0) as u64;
            if live_tracks[track_idx].quantize_next_tick {
                live_tracks[track_idx].next_tick_sample = quantized_live_tick_sample(
                    rendered_sample,
                    rendered_total_beats,
                    live_tick_beats,
                    samples_per_quarter,
                );
                live_tracks[track_idx].quantize_next_tick = false;
            }
            let track_boundaries = track_step_boundaries(&snapshot.tracks[track_idx]);
            let cycle_beats = track_boundaries
                .get(snapshot.tracks[track_idx].params.num_steps)
                .copied()
                .unwrap_or(live_tick_beats)
                .max(live_tick_beats) as f64;
            let tick_offset_beats = live_tracks[track_idx]
                .next_tick_sample
                .saturating_sub(rendered_sample) as f64
                / samples_per_quarter as f64;
            let track_position_beats =
                ((rendered_total_beats + tick_offset_beats) % cycle_beats) as f32;
            let velocity = notes
                .iter()
                .map(|note| note.velocity)
                .fold(0.0_f32, f32::max)
                .clamp(0.0, 1.0);
            let mut note_spans = track_active_note_spans_at_beat(
                snapshot,
                track_idx,
                track_position_beats,
                live_tick_beats,
            );
            let mut chord = note_spans
                .iter()
                .map(|note| note.transpose)
                .collect::<Vec<_>>();
            let live_spans = notes
                .iter()
                .map(|note| AccumulatorNoteSpan {
                    transpose: note.transpose,
                    start_beats: 0.0,
                    end_beats: live_tick_beats,
                })
                .collect::<Vec<_>>();
            chord.extend(live_spans.iter().map(|note| note.transpose));
            note_spans.extend(live_spans);
            if chord.is_empty() {
                break;
            }
            let chord_durations = vec![1.0; chord.len()];
            let first_transpose = chord[0];
            let resolved = ResolvedStep {
                duration: 1.0,
                velocity,
                speed: 1.0,
                aux_a: 0.0,
                aux_b: 0.0,
                transpose: first_transpose,
                pan: 0.0,
                chop: 1.0,
            };
            let event = MidiFxEvent {
                offset_beats: 0.0,
                track: track_idx,
                step,
                samples_per_step: live_tick_samples as f32,
                step_beats: live_tick_beats,
                resolved,
                chord,
                chord_durations,
                chord_step_transpose: 0.0,
                note_spans: Some(note_spans),
                arp_phase_beats: (rendered_total_beats + tick_offset_beats) as f32,
                effect_params: resolve_effect_params(snapshot, track_idx, step),
                instrument_params: resolve_instrument_params(snapshot, track_idx, step),
            };
            let events = run_midi_fx_chain_for_track(
                runtime,
                snapshot,
                track_idx,
                vec![event],
                0,
                debug_accum,
            );
            if !enqueue_midi_fx_events(
                queue,
                snapshot,
                pattern_epoch,
                live_tracks[track_idx].next_tick_sample,
                samples_per_quarter,
                events,
            ) {
                break;
            }
            live_tracks[track_idx].next_tick_sample = live_tracks[track_idx]
                .next_tick_sample
                .saturating_add(live_tick_samples);
        }
    }

    live_active
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
            let mut live_midi_fx_tracks: [LiveMidiFxTrackState; MAX_TRACKS] =
                std::array::from_fn(|_| LiveMidiFxTrackState::default());
            let mut last_live_midi_fx_active = false;
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
                    let source =
                        lisp_effect::midi_fx_library_source_with_user_source(&state.scratch_source());
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
                                        "[accum] scratch eval ok names={:?} midi_fx={:?} status={:?}",
                                        runtime.accumulator_names(),
                                        runtime.midi_fx_names(),
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

                if !playing
                    && (last_playing
                        || last_pattern != pattern
                        || last_pattern_epoch != pattern_epoch
                        || last_topology_epoch != topology_epoch)
                {
                    queue.clear();
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
                    scheduled_until_sample = rendered;
                    if playing {
                        clock.seek_to_rendered_position(
                            &snapshot,
                            rendered,
                            previous_scheduled_until,
                        );
                    }
                    last_live_midi_fx_active = live_midi_fx_active;
                }
                let samples_per_quarter =
                    sample_rate as f64 * 60.0 / snapshot.transport.bpm.max(1) as f64;
                let scheduled_ahead_beats =
                    scheduled_until_sample.saturating_sub(rendered) as f64 / samples_per_quarter;
                let rendered_total_beats = (clock.total_beats - scheduled_ahead_beats).max(0.0);
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
                    clock.reset();
                    scheduled_until_sample = rendered;
                    last_playing = false;
                    last_pattern = pattern;
                    last_pattern_epoch = pattern_epoch;
                    last_topology_epoch = topology_epoch;
                    pending_accum_reset = [false; MAX_TRACKS];
                    accumulator_states = [AccumulatorRuntimeState::default(); MAX_TRACKS];
                    thread::sleep(Duration::from_millis(if live_active { 1 } else { 2 }));
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

                while scheduled_until_sample < rendered.saturating_add(lookahead_target_samples) {
                    let triggers = clock.process_chunk(scheduler_block_size, &snapshot, &state);
                    let mut chunk_enqueued = true;
                    for trigger in triggers {
                        if !snapshot.tracks[trigger.track].steps[trigger.step].active {
                            continue;
                        }
                        if track_has_live_midi_fx_notes(
                            &live_midi_fx_tracks,
                            &snapshot,
                            trigger.track,
                        ) {
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
                                    Some(note_spans.clone()),
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
                                        let samples_per_quarter =
                                            sample_rate as f32 * 60.0 / snapshot.transport.bpm as f32;
                                        let step_beats =
                                            trigger.samples_per_step / samples_per_quarter;
                                        let mut accumulator_events = Vec::new();
                                        if !output.suppressed {
                                            accumulator_events.push(MidiFxEvent {
                                                offset_beats: 0.0,
                                                track: trigger.track,
                                                step: trigger.step,
                                                samples_per_step: trigger.samples_per_step,
                                                step_beats,
                                                resolved: output.resolved,
                                                chord: step_snapshot.chord.clone(),
                                                chord_durations: step_snapshot.chord_durations.clone(),
                                                chord_step_transpose: step_snapshot.params
                                                    [StepParam::Transpose.index()],
                                                note_spans: Some(note_spans.clone()),
                                                arp_phase_beats: trigger.absolute_beats as f32,
                                                effect_params: output.effect_params.clone(),
                                                instrument_params: output.instrument_params.clone(),
                                            });
                                        }
                                        for emitted in output.emitted {
                                            let target_track = emitted.track.unwrap_or(trigger.track);
                                            if target_track >= snapshot.tracks.len() {
                                                continue;
                                            }
                                            accumulator_events.push(MidiFxEvent {
                                                offset_beats: emitted.offset_beats,
                                                track: target_track,
                                                step: trigger.step,
                                                samples_per_step: trigger.samples_per_step,
                                                step_beats,
                                                resolved: emitted.resolved,
                                                chord: emitted.chord,
                                                chord_durations: emitted.chord_durations,
                                                chord_step_transpose: emitted.chord_step_transpose,
                                                note_spans: None,
                                                arp_phase_beats: trigger.absolute_beats as f32,
                                                effect_params: emitted.effect_params,
                                                instrument_params: emitted.instrument_params,
                                            });
                                        }
                                        for event in accumulator_events {
                                            if track_has_live_midi_fx_notes(
                                                &live_midi_fx_tracks,
                                                &snapshot,
                                                event.track,
                                            ) {
                                                continue;
                                            }
                                            let final_events = if snapshot.tracks[event.track]
                                                .params
                                                .midi_fx_position
                                                == MidiFxPosition::PostAccumulator
                                                && !snapshot.tracks[event.track]
                                                    .params
                                                    .midi_fx_chain
                                                    .is_empty()
                                            {
                                                run_midi_fx_chain_for_track(
                                                    runtime,
                                                    &snapshot,
                                                    event.track,
                                                    vec![event],
                                                    0,
                                                    debug_accum,
                                                )
                                            } else {
                                                vec![event]
                                            };
                                            if !enqueue_midi_fx_events(
                                                &queue,
                                                &snapshot,
                                                pattern_epoch,
                                                sample_time,
                                                samples_per_quarter,
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
                            if track_has_live_midi_fx_notes(
                                &live_midi_fx_tracks,
                                &snapshot,
                                target_track,
                            ) {
                                continue;
                            }
                            let effect_params =
                                resolve_effect_params(&snapshot, target_track, trigger.step);
                            let instrument_params =
                                resolve_instrument_params(&snapshot, target_track, trigger.step);
                            let samples_per_quarter =
                                sample_rate as f32 * 60.0 / snapshot.transport.bpm as f32;
                            if snapshot.tracks[target_track].params.midi_fx_position
                                == MidiFxPosition::PostAccumulator
                                && !snapshot.tracks[target_track].params.midi_fx_chain.is_empty()
                            {
                                if let Some(runtime) = scratch_runtime.as_mut() {
                                    let event = midi_fx_event_from_step(
                                        &snapshot,
                                        target_track,
                                        trigger.step,
                                        trigger.samples_per_step,
                                        trigger.samples_per_step / samples_per_quarter,
                                        trigger.absolute_beats as f32,
                                        resolved,
                                        effect_params,
                                        instrument_params,
                                    );
                                    let events = run_midi_fx_chain_for_track(
                                        runtime,
                                        &snapshot,
                                        target_track,
                                        vec![event],
                                        0,
                                        debug_accum,
                                    );
                                    if !enqueue_midi_fx_events(
                                        &queue,
                                        &snapshot,
                                        pattern_epoch,
                                        sample_time,
                                        samples_per_quarter,
                                        events,
                                    ) {
                                        chunk_enqueued = false;
                                        break;
                                    }
                                } else {
                                    let chord =
                                        step_chord_data(&snapshot, target_track, trigger.step);
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
                            } else {
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
    use super::{
        quantized_live_tick_sample, track_active_note_spans_at_beat, track_note_spans_for_trigger,
        SnapshotSequencerClock,
    };
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

    #[test]
    fn active_note_spans_at_beat_exposes_current_sequenced_pool_for_live_join() {
        let state = SequencerState::new(1, vec![default_empty_effect_chain()]);
        let track = 0;

        state.pattern.patterns[track].set_step_active(0, true);
        state.pattern.chord_data[track].add_note_with_duration(0, 0.0, 8.0);
        state.pattern.chord_data[track].add_note_with_duration(0, 7.0, 4.0);
        state.pattern.step_data[track].set(0, StepParam::Duration, 8.0);

        state.pattern.patterns[track].set_step_active(4, true);
        state.pattern.step_data[track].set(4, StepParam::Transpose, 12.0);
        state.pattern.step_data[track].set(4, StepParam::Duration, 4.0);

        let snapshot = state.publish_scheduler_snapshot();
        let spans = track_active_note_spans_at_beat(&snapshot, track, 1.0, 0.25);
        let transposes = spans.iter().map(|span| span.transpose).collect::<Vec<_>>();

        assert_eq!(transposes, vec![0.0, 12.0]);
        assert!(spans.iter().all(|span| span.start_beats == 0.0));
        assert!(spans.iter().all(|span| span.end_beats <= 0.25));
    }

    #[test]
    fn live_midi_fx_start_quantizes_to_next_tick() {
        let rendered_sample = 48_000;
        let samples_per_quarter = 24_000.0;

        assert_eq!(
            quantized_live_tick_sample(rendered_sample, 1.25, 0.25, samples_per_quarter),
            rendered_sample
        );
        assert_eq!(
            quantized_live_tick_sample(rendered_sample, 1.30, 0.25, samples_per_quarter),
            rendered_sample + 4_800
        );
    }
}
