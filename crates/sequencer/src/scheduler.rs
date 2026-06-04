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
use crate::neural::{NeuralOutput, NeuralRuntime, ParamNodeId};
use crate::scheduled_event::{
    resolved_chord_transpose, EventSource, ScheduledChordData, ScheduledEffectParam,
    ScheduledEvent, ScheduledEventKind, ScheduledEventQueue, ScheduledInstrumentParam,
    ScheduledInstrumentParamTarget, ScheduledInstrumentParams, ScheduledSamplerParams, StepEvent,
};
use crate::sequencer::{
    sync_beats, KeyboardTrigger, MidiFxPosition, SequencerSnapshot, SequencerState, StepParam,
    SwingResolution, MAX_STEPS, MAX_TRACKS,
};
use crate::voice::MAX_VOICES;

fn scheduled_instrument_params_from_vec(
    params: Vec<ScheduledInstrumentParam>,
) -> ScheduledInstrumentParams {
    params.into_iter().collect::<ScheduledInstrumentParams>()
}

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
    swing_delay_samples_from_quarter(samples_per_quarter, swing_pct, resolution)
}

fn swing_delay_samples_from_quarter(
    samples_per_quarter: f64,
    swing_pct: f32,
    resolution: SwingResolution,
) -> f64 {
    let resolution_samples = resolution.step_beats() * samples_per_quarter;
    ((swing_pct as f64 / 100.0) - 0.5) * 2.0 * resolution_samples
}

fn swung_network_sample_time(
    snapshot: &SequencerSnapshot,
    event: &StepEvent,
    sample_time: u64,
    event_beats: f64,
    samples_per_quarter: f64,
) -> u64 {
    let Some(track) = snapshot.tracks.get(event.track) else {
        return sample_time;
    };
    let swing_pct = track.params.swing;
    if swing_pct <= 50.0 {
        return sample_time;
    }
    let swing_step = swing_bucket_index(event_beats, track.params.swing_resolution);
    if swing_step % 2 == 0 {
        return sample_time;
    }
    let swing_delay = swing_delay_samples_from_quarter(
        samples_per_quarter,
        swing_pct,
        track.params.swing_resolution,
    )
    .round();
    sample_time.saturating_add(swing_delay.max(0.0) as u64)
}

fn step_delay_samples(step_params: &[f32], samples_per_step: f32) -> u64 {
    let delay = step_params
        .get(StepParam::Delay.index())
        .copied()
        .unwrap_or_else(|| StepParam::Delay.default_value())
        .clamp(StepParam::Delay.min(), StepParam::Delay.max());
    (delay as f64 * samples_per_step.max(0.0) as f64).round() as u64
}

fn slot_param_identity(node_id: u32, modulator_node_id: u32, raw_idx: u32) -> Option<ParamNodeId> {
    if raw_idx == u32::MAX {
        return None;
    }
    if raw_idx >= crate::voice_modulator::MOD_PARAM_BASE {
        if modulator_node_id == 0 {
            return None;
        }
        Some(ParamNodeId {
            logical_id: modulator_node_id as u64,
            node_param_idx: raw_idx - crate::voice_modulator::MOD_PARAM_BASE,
        })
    } else {
        if node_id == 0 {
            return None;
        }
        Some(ParamNodeId {
            logical_id: node_id as u64,
            node_param_idx: raw_idx,
        })
    }
}

fn plock_identity_matches(
    plock_ids: &[Vec<Option<ParamNodeId>>],
    step_idx: usize,
    param_idx: usize,
    expected: Option<ParamNodeId>,
) -> bool {
    let Some(expected) = expected else {
        return false;
    };
    plock_ids
        .get(step_idx)
        .and_then(|step| step.get(param_idx))
        .copied()
        .flatten()
        == Some(expected)
}

fn resolved_slot_param_value(
    slot: &crate::effects::EffectSlotSnapshot,
    step_idx: usize,
    param_idx: usize,
    default: f32,
) -> f32 {
    let default_value = slot.defaults.get(param_idx).copied().unwrap_or(default);
    let Some(plock) = slot
        .plocks
        .get(step_idx)
        .and_then(|step| step.get(param_idx))
        .copied()
        .flatten()
    else {
        return default_value;
    };
    let raw_idx = slot
        .param_node_indices
        .get(param_idx)
        .copied()
        .unwrap_or(param_idx as u32);
    let expected_id = slot_param_identity(slot.node_id, slot.modulator_node_id, raw_idx);
    if plock_identity_matches(&slot.plock_param_ids, step_idx, param_idx, expected_id) {
        plock
    } else {
        default_value
    }
}

fn delayed_step_sample_time(
    base_sample_time: u64,
    step_params: &[f32],
    samples_per_step: f32,
) -> u64 {
    base_sample_time.saturating_add(step_delay_samples(step_params, samples_per_step))
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
            let raw_idx = slot
                .param_node_indices
                .get(param_idx)
                .copied()
                .unwrap_or(param_idx as u32);
            if raw_idx == u32::MAX {
                continue;
            }
            let (logical_id, idx) = if raw_idx >= crate::voice_modulator::MOD_PARAM_BASE {
                if slot.modulator_node_id == 0 {
                    continue;
                }
                (
                    slot.modulator_node_id as u64,
                    raw_idx as u64 - crate::voice_modulator::MOD_PARAM_BASE as u64,
                )
            } else {
                (slot.node_id as u64, raw_idx as u64)
            };
            let value = resolved_slot_param_value(slot, step_idx, param_idx, 0.0);
            if !value.is_finite() {
                continue;
            }
            params.push(ScheduledEffectParam {
                logical_id,
                idx,
                value,
            });
        }
    }
    params.sort_by_key(|param| (param.logical_id, param.idx));
    params
}

fn resolve_instrument_params(
    snapshot: &SequencerSnapshot,
    track_idx: usize,
    step_idx: usize,
) -> ScheduledInstrumentParams {
    let slot = &snapshot.tracks[track_idx].instrument_slot;
    let num_params = slot.num_params as usize;
    let mut params = ScheduledInstrumentParams::new();
    for param_idx in 0..num_params {
        let raw_idx = slot
            .param_node_indices
            .get(param_idx)
            .copied()
            .unwrap_or(param_idx as u32);
        let span = slot
            .param_node_spans
            .get(param_idx)
            .copied()
            .unwrap_or(1)
            .max(1);
        let (target, idx) = if raw_idx >= crate::voice_modulator::MOD_PARAM_BASE {
            (
                ScheduledInstrumentParamTarget::Modulator,
                (raw_idx - crate::voice_modulator::MOD_PARAM_BASE) as u64,
            )
        } else {
            (ScheduledInstrumentParamTarget::Synth, raw_idx as u64)
        };
        let value = resolved_slot_param_value(slot, step_idx, param_idx, 0.0);
        if !value.is_finite() {
            continue;
        }
        params.push(ScheduledInstrumentParam {
            target,
            idx,
            span,
            value,
        });
    }
    params.sort_by_key(|param| match param.target {
        ScheduledInstrumentParamTarget::Synth => (0_u8, param.idx),
        ScheduledInstrumentParamTarget::Modulator => (1_u8, param.idx),
    });
    params
}

fn resolve_instrument_defaults(
    snapshot: &SequencerSnapshot,
    track_idx: usize,
) -> ScheduledInstrumentParams {
    let slot = &snapshot.tracks[track_idx].instrument_slot;
    let num_params = slot.num_params as usize;
    let mut params = ScheduledInstrumentParams::new();
    for param_idx in 0..num_params {
        let raw_idx = slot
            .param_node_indices
            .get(param_idx)
            .copied()
            .unwrap_or(param_idx as u32);
        let span = slot
            .param_node_spans
            .get(param_idx)
            .copied()
            .unwrap_or(1)
            .max(1);
        let (target, idx) = if raw_idx >= crate::voice_modulator::MOD_PARAM_BASE {
            (
                ScheduledInstrumentParamTarget::Modulator,
                (raw_idx - crate::voice_modulator::MOD_PARAM_BASE) as u64,
            )
        } else {
            (ScheduledInstrumentParamTarget::Synth, raw_idx as u64)
        };
        let value = slot.defaults.get(param_idx).copied().unwrap_or(0.0);
        if !value.is_finite() {
            continue;
        }
        params.push(ScheduledInstrumentParam {
            target,
            idx,
            span,
            value,
        });
    }
    params.sort_by_key(|param| match param.target {
        ScheduledInstrumentParamTarget::Synth => (0_u8, param.idx),
        ScheduledInstrumentParamTarget::Modulator => (1_u8, param.idx),
    });
    params
}

fn resolve_effect_defaults(
    snapshot: &SequencerSnapshot,
    track_idx: usize,
) -> Vec<ScheduledEffectParam> {
    let mut params = Vec::new();
    for slot in &snapshot.tracks[track_idx].effect_slots {
        if slot.node_id == 0 {
            continue;
        }
        let num_params = slot.num_params as usize;
        for param_idx in 0..num_params {
            let raw_idx = slot
                .param_node_indices
                .get(param_idx)
                .copied()
                .unwrap_or(param_idx as u32);
            if raw_idx == u32::MAX {
                continue;
            }
            let (logical_id, idx) = if raw_idx >= crate::voice_modulator::MOD_PARAM_BASE {
                if slot.modulator_node_id == 0 {
                    continue;
                }
                (
                    slot.modulator_node_id as u64,
                    raw_idx as u64 - crate::voice_modulator::MOD_PARAM_BASE as u64,
                )
            } else {
                (slot.node_id as u64, raw_idx as u64)
            };
            let value = slot.defaults.get(param_idx).copied().unwrap_or(0.0);
            if !value.is_finite() {
                continue;
            }
            params.push(ScheduledEffectParam {
                logical_id,
                idx,
                value,
            });
        }
    }
    params.sort_by_key(|param| (param.logical_id, param.idx));
    params
}

fn resolve_instrument_plocks(
    snapshot: &SequencerSnapshot,
    track_idx: usize,
    step_idx: usize,
) -> ScheduledInstrumentParams {
    let slot = &snapshot.tracks[track_idx].instrument_slot;
    let num_params = slot.num_params as usize;
    let Some(step_plocks) = slot.plocks.get(step_idx) else {
        return ScheduledInstrumentParams::new();
    };
    let mut params = ScheduledInstrumentParams::new();
    for param_idx in 0..num_params {
        let Some(value) = step_plocks.get(param_idx).copied().flatten() else {
            continue;
        };
        if !value.is_finite() {
            continue;
        }
        let raw_idx = slot
            .param_node_indices
            .get(param_idx)
            .copied()
            .unwrap_or(param_idx as u32);
        let span = slot
            .param_node_spans
            .get(param_idx)
            .copied()
            .unwrap_or(1)
            .max(1);
        let (target, idx) = if raw_idx >= crate::voice_modulator::MOD_PARAM_BASE {
            (
                ScheduledInstrumentParamTarget::Modulator,
                (raw_idx - crate::voice_modulator::MOD_PARAM_BASE) as u64,
            )
        } else {
            (ScheduledInstrumentParamTarget::Synth, raw_idx as u64)
        };
        let expected_id = slot_param_identity(slot.node_id, slot.modulator_node_id, raw_idx);
        if !plock_identity_matches(&slot.plock_param_ids, step_idx, param_idx, expected_id) {
            continue;
        }
        params.push(ScheduledInstrumentParam {
            target,
            idx,
            span,
            value,
        });
    }
    params.sort_by_key(|param| match param.target {
        ScheduledInstrumentParamTarget::Synth => (0_u8, param.idx),
        ScheduledInstrumentParamTarget::Modulator => (1_u8, param.idx),
    });
    params
}

fn enqueue_instrument_param_change(
    queue: &ScheduledEventQueue<4096>,
    pattern_epoch: u64,
    sample_time: u64,
    track_idx: usize,
    instrument_params: ScheduledInstrumentParams,
) -> bool {
    if instrument_params.is_empty() {
        return true;
    }
    queue
        .push(ScheduledEvent {
            pattern_epoch,
            sample_time,
            kind: ScheduledEventKind::InstrumentParams {
                track: track_idx,
                instrument_params,
            },
        })
        .is_ok()
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
    Some(resolved_slot_param_value(slot, step_idx, param_idx, 0.0))
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

fn resolve_sampler_params(
    snapshot: &SequencerSnapshot,
    track_idx: usize,
    step_idx: usize,
) -> ScheduledSamplerParams {
    let Some(slot) = snapshot
        .tracks
        .get(track_idx)
        .map(|track| &track.instrument_slot)
    else {
        return ScheduledSamplerParams::default();
    };
    let value = |param_idx: usize, default: f32| {
        resolved_slot_param_value(slot, step_idx, param_idx, default)
    };
    ScheduledSamplerParams {
        attack_ms: value(0, 0.0),
        release_ms: value(1, 0.0),
        start_point: value(2, 0.0),
        end_point: value(3, 1.0),
        instrument_enabled: value(4, 1.0),
        reverse: value(5, 0.0),
        loop_mode: value(6, 0.0),
        loop_xfade_ms: value(7, 0.0),
        sr_hz: value(8, 0.0),
        warp_enabled: value(9, 0.0),
        warp_mode: value(10, 0.0),
        sample_bpm: value(11, 120.0),
        playback_speed: value(12, 1.0),
        scrub: value(13, 0.0),
    }
}

/// Seed every graph-mode runtime from a step-sequencer trigger (spec §4): each node
/// whose resolved `seed-from` includes the event's track receives a fire carrying the
/// step's note/velocity, after that node's delay. Mirrors the `neural` seed sites.
fn seed_graph_runtimes(
    graphs: &mut [crate::graph::GraphRuntime],
    event: &StepEvent,
    seed_beats: f64,
) {
    if graphs.is_empty() {
        return;
    }
    let payload = crate::graph::GraphPayload {
        note: event.resolved.transpose,
        velocity: event.resolved.velocity,
    };
    let debug_graph = std::env::var_os("TINYSEQ_DEBUG_GRAPH").is_some();
    for (graph_idx, graph) in graphs.iter_mut().enumerate() {
        let seeded = graph.seed(event.track, seed_beats, payload);
        if debug_graph {
            eprintln!(
                "[graph-seed] graph={} track={} beat={:.6} seeded={} node0_mask={:#x} node0_pending={}",
                graph_idx,
                event.track,
                seed_beats,
                seeded,
                graph.seed_track_mask_for_node(0).unwrap_or(0),
                graph.pending_count_for_node(0).unwrap_or(0)
            );
        }
    }
}

fn graph_overrides_for_manifest<'a>(
    manifest: &crate::graph::GraphManifest,
    overrides: &'a [crate::graph::ProjectGraphOverrides],
) -> Option<&'a crate::graph::ProjectGraphOverrides> {
    overrides.iter().find(|overrides| {
        overrides.sequencer_id == manifest.id || overrides.sequencer_name == manifest.name
    })
}

fn reconcile_graph_runtimes(
    manifests: Vec<crate::graph::GraphManifest>,
    overrides: &[crate::graph::ProjectGraphOverrides],
    runtimes: &mut Vec<crate::graph::GraphRuntime>,
    stored_manifests: &mut Vec<crate::graph::GraphManifest>,
    total_beats: f64,
) {
    let mut existing = std::mem::take(runtimes);
    let mut next_runtimes = Vec::with_capacity(manifests.len());
    for manifest in &manifests {
        let graph_overrides = graph_overrides_for_manifest(manifest, overrides);
        let config = manifest.runtime_config_with_overrides(graph_overrides);
        let next_runtime = if let Some(pos) = existing
            .iter()
            .position(|runtime| runtime.id == manifest.id)
        {
            let mut runtime = existing.swap_remove(pos);
            if runtime.config_compatible(&config) {
                runtime.apply_config_preserving_state(config, total_beats);
                runtime
            } else {
                let mut runtime = crate::graph::GraphRuntime::new_from_config(config);
                runtime.realign(total_beats);
                runtime
            }
        } else {
            let mut runtime = crate::graph::GraphRuntime::new_from_config(config);
            runtime.realign(total_beats);
            runtime
        };
        next_runtimes.push(next_runtime);
    }
    *runtimes = next_runtimes;
    *stored_manifests = manifests;
}

fn chord_data_from_parts(
    notes: &[f32],
    durations: &[f32],
    delays: &[f32],
    fallback_duration: f32,
    step_transpose: f32,
) -> ScheduledChordData {
    let mut chord = ScheduledChordData {
        count: notes.len().min(MAX_VOICES),
        notes: [0.0; MAX_VOICES],
        durations: [0.0; MAX_VOICES],
        delays: [0.0; MAX_VOICES],
        step_transpose,
    };
    for (idx, note) in notes.iter().take(MAX_VOICES).enumerate() {
        chord.notes[idx] = *note;
        chord.durations[idx] = durations
            .get(idx)
            .copied()
            .filter(|duration| *duration > 0.0)
            .unwrap_or(fallback_duration);
        chord.delays[idx] = delays
            .get(idx)
            .copied()
            .unwrap_or(0.0)
            .clamp(StepParam::Delay.min(), StepParam::Delay.max());
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
        &step.chord_delays,
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

fn delayed_step_start_beats(
    track: &crate::sequencer::SequencerTrackSnapshot,
    step: usize,
    boundaries: &[f32],
) -> f32 {
    let step_beats = track.steps[step]
        .timebase_override
        .unwrap_or(track.params.timebase)
        .step_beats(track.params.num_steps) as f32;
    let delay = track.steps[step].params[StepParam::Delay.index()]
        .clamp(StepParam::Delay.min(), StepParam::Delay.max());
    boundaries[step] + delay * step_beats.max(0.0)
}

fn explicit_note_delay_beats(
    step_snapshot: &crate::sequencer::SequencerStepSnapshot,
    note_idx: usize,
    step_beats: f32,
) -> f32 {
    step_snapshot
        .chord_delays
        .get(note_idx)
        .copied()
        .unwrap_or(0.0)
        .clamp(StepParam::Delay.min(), StepParam::Delay.max())
        * step_beats.max(0.0)
}

fn step_trigger_start_beats(
    track: &crate::sequencer::SequencerTrackSnapshot,
    step: usize,
    boundaries: &[f32],
) -> f32 {
    if track.steps[step].chord.is_empty() {
        delayed_step_start_beats(track, step, boundaries)
    } else {
        boundaries[step]
    }
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
    let trigger_start = step_trigger_start_beats(track, step_idx, &boundaries);
    let mut candidates = Vec::new();

    for step in 0..ns {
        let step_snapshot = &track.steps[step];
        if !step_snapshot.active {
            continue;
        }
        let step_beats = step_snapshot
            .timebase_override
            .unwrap_or(track.params.timebase)
            .step_beats(ns) as f32;
        if step_beats <= 0.0 {
            continue;
        }
        let fallback_duration = step_snapshot.params[StepParam::Duration.index()].max(0.0);
        if step_snapshot.chord.is_empty() {
            let step_start = delayed_step_start_beats(track, step, &boundaries);
            candidates.push(AccumulatorNoteSpan {
                transpose: step_snapshot.params[StepParam::Transpose.index()],
                start_beats: step_start,
                end_beats: step_start + fallback_duration * step_beats,
            });
        } else {
            for (idx, note) in step_snapshot.chord.iter().enumerate() {
                let step_start =
                    boundaries[step] + explicit_note_delay_beats(step_snapshot, idx, step_beats);
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

    let Some(group_anchor) = candidates
        .iter()
        .filter(|note| note.start_beats >= trigger_start - EPS)
        .map(|note| note.start_beats)
        .next()
    else {
        return Vec::new();
    };

    if candidates
        .iter()
        .any(|note| note.start_beats < group_anchor - EPS && note.end_beats > group_anchor + EPS)
    {
        return Vec::new();
    }

    let mut group_end = candidates
        .iter()
        .filter(|note| (note.start_beats - group_anchor).abs() <= EPS)
        .map(|note| note.end_beats)
        .fold(group_anchor, f32::max);
    if group_end <= group_anchor + EPS {
        return Vec::new();
    }

    let mut out = Vec::new();
    for note in candidates {
        if note.start_beats < trigger_start - EPS {
            continue;
        }
        if note.start_beats > group_anchor + EPS && note.start_beats >= group_end - EPS {
            break;
        }
        if note.end_beats <= group_anchor + EPS {
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
            let step_beats = step_snapshot
                .timebase_override
                .unwrap_or(track.params.timebase)
                .step_beats(ns) as f32;
            if step_beats <= 0.0 {
                continue;
            }
            let fallback_duration = step_snapshot.params[StepParam::Duration.index()].max(0.0);
            if step_snapshot.chord.is_empty() {
                let step_start = delayed_step_start_beats(track, step, &boundaries) + cycle_offset;
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
                    let step_start = boundaries[step]
                        + explicit_note_delay_beats(step_snapshot, idx, step_beats)
                        + cycle_offset;
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

fn enqueue_resolved_trigger<const QUEUE_CAP: usize>(
    queue: &ScheduledEventQueue<QUEUE_CAP>,
    snapshot: &SequencerSnapshot,
    pattern_epoch: u64,
    sample_time: u64,
    track_idx: usize,
    step_idx: usize,
    samples_per_step: f32,
    resolved: ResolvedStep,
    chord: ScheduledChordData,
    effect_params: Vec<ScheduledEffectParam>,
    instrument_params: ScheduledInstrumentParams,
) -> bool {
    let (resolved, chord) = apply_fit_to_scale_to_trigger(snapshot, track_idx, resolved, chord);
    let instrument_fingerprint =
        instrument_sound_fingerprint(snapshot, track_idx, &instrument_params);
    if chord.count > 0 {
        let max_delay = chord.delays[..chord.count]
            .iter()
            .copied()
            .fold(0.0_f32, f32::max);
        if max_delay > 1e-6 {
            let mut ok = true;
            for note_idx in 0..chord.count {
                let note_delay =
                    chord.delays[note_idx].clamp(StepParam::Delay.min(), StepParam::Delay.max());
                let mut note_chord = ScheduledChordData {
                    count: 1,
                    notes: [0.0; MAX_VOICES],
                    durations: [0.0; MAX_VOICES],
                    delays: [0.0; MAX_VOICES],
                    step_transpose: chord.step_transpose,
                };
                note_chord.notes[0] = chord.notes[note_idx];
                note_chord.durations[0] = chord.durations[note_idx];
                let note_sample_time = sample_time.saturating_add(
                    (note_delay as f64 * samples_per_step.max(0.0) as f64).round() as u64,
                );
                if queue
                    .push(ScheduledEvent {
                        pattern_epoch,
                        sample_time: note_sample_time,
                        kind: ScheduledEventKind::ResolvedTrigger {
                            track: track_idx,
                            step: step_idx,
                            samples_per_step,
                            resolved,
                            chord: note_chord,
                            effect_params: effect_params.clone(),
                            instrument_params: instrument_params.clone(),
                            instrument_fingerprint,
                        },
                    })
                    .is_err()
                {
                    ok = false;
                    break;
                }
            }
            return ok;
        }
    }
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

fn step_event_from_resolved(
    snapshot: &SequencerSnapshot,
    track_idx: usize,
    step_idx: usize,
    samples_per_step: f32,
    resolved: ResolvedStep,
    chord: ScheduledChordData,
    effect_params: Vec<ScheduledEffectParam>,
    instrument_params: ScheduledInstrumentParams,
) -> StepEvent {
    let instrument_fingerprint =
        instrument_sound_fingerprint(snapshot, track_idx, &instrument_params);
    StepEvent {
        track: track_idx,
        samples_per_step,
        resolved,
        chord,
        effect_params,
        instrument_params,
        sampler_params: resolve_sampler_params(snapshot, track_idx, step_idx),
        source: EventSource::Step {
            track: track_idx,
            step: step_idx,
            instrument_fingerprint,
        },
    }
}

fn enqueue_step_event<const QUEUE_CAP: usize>(
    queue: &ScheduledEventQueue<QUEUE_CAP>,
    snapshot: &SequencerSnapshot,
    pattern_epoch: u64,
    sample_time: u64,
    mut event: StepEvent,
) -> bool {
    match event.source.clone() {
        EventSource::Step { step, .. } => enqueue_resolved_trigger(
            queue,
            snapshot,
            pattern_epoch,
            sample_time,
            event.track,
            step,
            event.samples_per_step,
            event.resolved,
            event.chord,
            event.effect_params,
            event.instrument_params,
        ),
        EventSource::Network { seed, neuron, .. } => {
            normalize_network_event_destination(snapshot, neuron, seed, &mut event);
            let instrument_fingerprint =
                instrument_sound_fingerprint(snapshot, event.track, &event.instrument_params);
            enqueue_network_trigger(
                queue,
                snapshot,
                pattern_epoch,
                sample_time,
                event.track,
                neuron,
                seed,
                event.samples_per_step,
                event.resolved,
                event.chord,
                event.effect_params,
                event.instrument_params,
                event.sampler_params,
                instrument_fingerprint,
            )
        }
    }
}

fn midi_fx_step_for_step_event(snapshot: &SequencerSnapshot, event: &StepEvent) -> usize {
    let step = match event.source {
        EventSource::Step { step, .. } => step,
        EventSource::Network {
            seed: Some((_, step)),
            ..
        } => step,
        EventSource::Network { .. } => 0,
    };
    midi_fx_event_step_for_track(snapshot, event.track, step)
}

fn enqueue_step_event_with_midi_fx<const QUEUE_CAP: usize>(
    queue: &ScheduledEventQueue<QUEUE_CAP>,
    snapshot: &SequencerSnapshot,
    runtime: Option<&mut lisp_effect::ScratchControlRuntime>,
    pattern_epoch: u64,
    sample_time: u64,
    samples_per_quarter: f32,
    arp_phase_beats: f32,
    mut event: StepEvent,
    debug_accum: bool,
) -> bool {
    if event.track >= snapshot.tracks.len() {
        return false;
    }
    let run_midi_fx = snapshot.tracks[event.track].params.midi_fx_position
        == MidiFxPosition::PostAccumulator
        && !snapshot.tracks[event.track].params.midi_fx_chain.is_empty();
    let Some(runtime) = runtime else {
        return enqueue_step_event(queue, snapshot, pattern_epoch, sample_time, event);
    };
    if !run_midi_fx {
        return enqueue_step_event(queue, snapshot, pattern_epoch, sample_time, event);
    }
    if let EventSource::Network { seed, neuron, .. } = event.source.clone() {
        normalize_network_event_destination(snapshot, neuron, seed, &mut event);
    }

    let step = midi_fx_step_for_step_event(snapshot, &event);
    let step_beats = if samples_per_quarter > 0.0 {
        event.samples_per_step / samples_per_quarter
    } else {
        0.0
    };
    let event =
        midi_fx_event_from_step_event(snapshot, event, step, step_beats, 0.0, arp_phase_beats);
    let events =
        run_midi_fx_chain_for_track(runtime, snapshot, event.track, vec![event], 0, debug_accum);
    enqueue_midi_fx_events(
        queue,
        snapshot,
        pattern_epoch,
        sample_time,
        samples_per_quarter,
        events,
    )
}

fn enqueue_neuron_parameter_events<const QUEUE_CAP: usize>(
    queue: &ScheduledEventQueue<QUEUE_CAP>,
    pattern_epoch: u64,
    sample_time: u64,
    parameter_events: NeuronParameterEvents,
) -> bool {
    let mut ok = true;
    for (track, instrument_params) in parameter_events.instrument {
        if instrument_params.is_empty() {
            continue;
        }
        if queue
            .push(ScheduledEvent {
                pattern_epoch,
                sample_time,
                kind: ScheduledEventKind::InstrumentParams {
                    track,
                    instrument_params,
                },
            })
            .is_err()
        {
            ok = false;
            break;
        }
    }
    if ok {
        for (track, effect_params) in parameter_events.effects {
            if effect_params.is_empty() {
                continue;
            }
            if queue
                .push(ScheduledEvent {
                    pattern_epoch,
                    sample_time,
                    kind: ScheduledEventKind::EffectParams {
                        track,
                        effect_params,
                    },
                })
                .is_err()
            {
                ok = false;
                break;
            }
        }
    }
    ok
}

fn enqueue_neural_output_with_midi_fx<const QUEUE_CAP: usize>(
    queue: &ScheduledEventQueue<QUEUE_CAP>,
    snapshot: &SequencerSnapshot,
    runtime: Option<&mut lisp_effect::ScratchControlRuntime>,
    pattern_epoch: u64,
    sample_time: u64,
    samples_per_quarter: f32,
    arp_phase_beats: f32,
    output: NeuralOutput,
    debug_accum: bool,
) -> bool {
    let mut event = output.event;
    let (seed, neuron) = match event.source.clone() {
        EventSource::Network { seed, neuron, .. } => (seed, neuron),
        EventSource::Step { .. } => {
            return output.emit_trigger
                && enqueue_step_event_with_midi_fx(
                    queue,
                    snapshot,
                    runtime,
                    pattern_epoch,
                    sample_time,
                    samples_per_quarter,
                    arp_phase_beats,
                    event,
                    debug_accum,
                );
        }
    };
    if output.emit_trigger {
        normalize_network_event_destination(snapshot, neuron, seed, &mut event);
    }
    let trigger_track = output.emit_trigger.then_some(event.track);
    let parameter_events =
        apply_neuron_output_overrides(snapshot, neuron, trigger_track, &mut event);
    if !enqueue_neuron_parameter_events(queue, pattern_epoch, sample_time, parameter_events) {
        return false;
    }
    if !output.emit_trigger {
        return true;
    }
    enqueue_step_event_with_midi_fx(
        queue,
        snapshot,
        runtime,
        pattern_epoch,
        sample_time,
        samples_per_quarter,
        arp_phase_beats,
        event,
        debug_accum,
    )
}

fn normalize_network_event_destination(
    snapshot: &SequencerSnapshot,
    neuron_idx: usize,
    seed: Option<(usize, usize)>,
    event: &mut StepEvent,
) {
    if seed.map(|(track, _)| track != event.track).unwrap_or(true) {
        event.effect_params = resolve_effect_defaults(snapshot, event.track);
        event.instrument_params = resolve_instrument_defaults(snapshot, event.track);
        event.sampler_params = resolve_sampler_defaults(snapshot, event.track);
    }
}

fn resolve_sampler_defaults(
    snapshot: &SequencerSnapshot,
    track_idx: usize,
) -> ScheduledSamplerParams {
    let Some(slot) = snapshot
        .tracks
        .get(track_idx)
        .map(|track| &track.instrument_slot)
    else {
        return ScheduledSamplerParams::default();
    };
    let value =
        |param_idx: usize, default: f32| slot.defaults.get(param_idx).copied().unwrap_or(default);
    ScheduledSamplerParams {
        attack_ms: value(0, 0.0),
        release_ms: value(1, 0.0),
        start_point: value(2, 0.0),
        end_point: value(3, 1.0),
        instrument_enabled: value(4, 1.0),
        reverse: value(5, 0.0),
        loop_mode: value(6, 0.0),
        loop_xfade_ms: value(7, 0.0),
        sr_hz: value(8, 0.0),
        warp_enabled: value(9, 0.0),
        warp_mode: value(10, 0.0),
        sample_bpm: value(11, 120.0),
        playback_speed: value(12, 1.0),
        scrub: value(13, 0.0),
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
struct NeuronParameterEvents {
    instrument: Vec<(usize, ScheduledInstrumentParams)>,
    effects: Vec<(usize, Vec<ScheduledEffectParam>)>,
}

fn push_target_instrument_param(
    events: &mut Vec<(usize, ScheduledInstrumentParams)>,
    track: usize,
    param: ScheduledInstrumentParam,
) {
    if let Some((_, params)) = events
        .iter_mut()
        .find(|(event_track, _)| *event_track == track)
    {
        if let Some(existing) = params
            .iter_mut()
            .find(|existing| existing.target == param.target && existing.idx == param.idx)
        {
            *existing = param;
        } else if !params.is_full() {
            params.push(param);
        }
        return;
    }
    let mut params = ScheduledInstrumentParams::new();
    params.push(param);
    events.push((track, params));
}

fn push_target_effect_param(
    events: &mut Vec<(usize, Vec<ScheduledEffectParam>)>,
    track: usize,
    param: ScheduledEffectParam,
) {
    if let Some((_, params)) = events
        .iter_mut()
        .find(|(event_track, _)| *event_track == track)
    {
        if let Some(existing) = params
            .iter_mut()
            .find(|existing| existing.logical_id == param.logical_id && existing.idx == param.idx)
        {
            *existing = param;
        } else {
            params.push(param);
        }
        return;
    }
    events.push((track, vec![param]));
}

fn resolve_neuron_instrument_override(
    snapshot: &SequencerSnapshot,
    override_param: &crate::neural::ProjectParamOverride,
) -> Option<(ScheduledInstrumentParam, u64)> {
    let track = snapshot.tracks.get(override_param.target_track)?;
    let param_idx = override_param.param_index;
    let raw_idx = track
        .instrument_slot
        .param_node_indices
        .get(param_idx)
        .copied()?;
    let expected_id = slot_param_identity(
        track.instrument_slot.node_id,
        track.instrument_slot.modulator_node_id,
        raw_idx,
    )?;
    if expected_id != override_param.param_id {
        return None;
    }
    let span = track
        .instrument_slot
        .param_node_spans
        .get(param_idx)
        .copied()
        .unwrap_or(1)
        .max(1);
    let (target, idx) = if raw_idx >= crate::voice_modulator::MOD_PARAM_BASE {
        (
            ScheduledInstrumentParamTarget::Modulator,
            (raw_idx - crate::voice_modulator::MOD_PARAM_BASE) as u64,
        )
    } else {
        (ScheduledInstrumentParamTarget::Synth, raw_idx as u64)
    };
    Some((
        ScheduledInstrumentParam {
            target,
            idx,
            span,
            value: override_param.value,
        },
        param_idx as u64,
    ))
}

fn resolve_neuron_effect_override(
    snapshot: &SequencerSnapshot,
    override_param: &crate::neural::ProjectEffectParamOverride,
) -> Option<ScheduledEffectParam> {
    let track = snapshot.tracks.get(override_param.target_track)?;
    let slot = track.effect_slots.get(override_param.slot_index)?;
    let raw_idx = slot
        .param_node_indices
        .get(override_param.param_index)
        .copied()?;
    let expected_id = slot_param_identity(slot.node_id, slot.modulator_node_id, raw_idx)?;
    if expected_id != override_param.param_id {
        return None;
    }
    let (logical_id, idx) = if raw_idx >= crate::voice_modulator::MOD_PARAM_BASE {
        if slot.modulator_node_id == 0 {
            return None;
        }
        (
            slot.modulator_node_id as u64,
            raw_idx as u64 - crate::voice_modulator::MOD_PARAM_BASE as u64,
        )
    } else {
        (slot.node_id as u64, raw_idx as u64)
    };
    if logical_id != override_param.param_id.logical_id {
        return None;
    }
    Some(ScheduledEffectParam {
        logical_id,
        idx,
        value: override_param.value,
    })
}

fn apply_neuron_output_overrides(
    snapshot: &SequencerSnapshot,
    neuron_idx: usize,
    trigger_track: Option<usize>,
    event: &mut StepEvent,
) -> NeuronParameterEvents {
    let Some(network) = snapshot
        .neural_networks
        .iter()
        .find(|network| network.enabled && neuron_idx < network.neurons.len())
    else {
        return NeuronParameterEvents::default();
    };
    let Some(neuron) = network.neurons.get(neuron_idx) else {
        return NeuronParameterEvents::default();
    };

    let mut parameter_events = NeuronParameterEvents::default();
    for override_param in &neuron.output_overrides.instrument {
        let Some((param, param_idx)) = resolve_neuron_instrument_override(snapshot, override_param)
        else {
            continue;
        };
        if Some(override_param.target_track) == trigger_track {
            if let Some(existing) = event
                .instrument_params
                .iter_mut()
                .find(|existing| existing.target == param.target && existing.idx == param.idx)
            {
                *existing = param.clone();
            } else if !event.instrument_params.is_full() {
                event.instrument_params.push(param.clone());
            }
            if matches!(param.target, ScheduledInstrumentParamTarget::Synth) {
                apply_sampler_param_override(&mut event.sampler_params, param_idx, param.value);
            }
        } else {
            push_target_instrument_param(
                &mut parameter_events.instrument,
                override_param.target_track,
                param,
            );
        }
    }
    event
        .instrument_params
        .sort_by_key(|param| match param.target {
            ScheduledInstrumentParamTarget::Synth => (0_u8, param.idx),
            ScheduledInstrumentParamTarget::Modulator => (1_u8, param.idx),
        });

    for override_param in &neuron.output_overrides.effects {
        let Some(param) = resolve_neuron_effect_override(snapshot, override_param) else {
            continue;
        };
        if Some(override_param.target_track) == trigger_track {
            if let Some(existing) = event.effect_params.iter_mut().find(|existing| {
                existing.logical_id == param.logical_id && existing.idx == param.idx
            }) {
                *existing = param;
            } else {
                event.effect_params.push(param);
            }
        } else {
            push_target_effect_param(
                &mut parameter_events.effects,
                override_param.target_track,
                param,
            );
        }
    }
    event
        .effect_params
        .sort_by_key(|param| (param.logical_id, param.idx));
    for (_, params) in &mut parameter_events.instrument {
        params.sort_by_key(|param| match param.target {
            ScheduledInstrumentParamTarget::Synth => (0_u8, param.idx),
            ScheduledInstrumentParamTarget::Modulator => (1_u8, param.idx),
        });
    }
    for (_, params) in &mut parameter_events.effects {
        params.sort_by_key(|param| (param.logical_id, param.idx));
    }
    parameter_events
}

fn apply_sampler_param_override(params: &mut ScheduledSamplerParams, idx: u64, value: f32) {
    match idx {
        0 => params.attack_ms = value,
        1 => params.release_ms = value,
        2 => params.start_point = value,
        3 => params.end_point = value,
        4 => params.instrument_enabled = value,
        5 => params.reverse = value,
        6 => params.loop_mode = value,
        7 => params.loop_xfade_ms = value,
        8 => params.sr_hz = value,
        9 => params.warp_enabled = value,
        10 => params.warp_mode = value,
        11 => params.sample_bpm = value,
        12 => params.playback_speed = value,
        13 => params.scrub = value,
        _ => {}
    }
}

fn apply_fit_to_scale_to_trigger(
    snapshot: &SequencerSnapshot,
    track_idx: usize,
    mut resolved: ResolvedStep,
    mut chord: ScheduledChordData,
) -> (ResolvedStep, ScheduledChordData) {
    let Some(track) = snapshot.tracks.get(track_idx) else {
        return (resolved, chord);
    };
    let scale_idx = track.params.fts_scale;
    if scale_idx == 0 {
        return (resolved, chord);
    }

    let pre_fts_transpose = resolved.transpose;
    resolved.transpose = crate::scale::quantize_transpose(pre_fts_transpose, scale_idx);
    for note_idx in 0..chord.count.min(MAX_VOICES) {
        let raw = resolved_chord_transpose(
            chord.notes[note_idx],
            chord.step_transpose,
            pre_fts_transpose,
        );
        let quantized = crate::scale::quantize_transpose(raw, scale_idx);
        chord.notes[note_idx] = quantized - (resolved.transpose - chord.step_transpose);
    }

    (resolved, chord)
}

fn enqueue_network_trigger<const QUEUE_CAP: usize>(
    queue: &ScheduledEventQueue<QUEUE_CAP>,
    snapshot: &SequencerSnapshot,
    pattern_epoch: u64,
    sample_time: u64,
    track_idx: usize,
    source_neuron: usize,
    seed: Option<(usize, usize)>,
    samples_per_step: f32,
    resolved: ResolvedStep,
    chord: ScheduledChordData,
    effect_params: Vec<ScheduledEffectParam>,
    instrument_params: ScheduledInstrumentParams,
    sampler_params: ScheduledSamplerParams,
    instrument_fingerprint: u64,
) -> bool {
    let (resolved, chord) = apply_fit_to_scale_to_trigger(snapshot, track_idx, resolved, chord);
    if chord.count > 0 {
        let max_delay = chord.delays[..chord.count]
            .iter()
            .copied()
            .fold(0.0_f32, f32::max);
        if max_delay > 1e-6 {
            let mut ok = true;
            for note_idx in 0..chord.count {
                let note_delay =
                    chord.delays[note_idx].clamp(StepParam::Delay.min(), StepParam::Delay.max());
                let mut note_chord = ScheduledChordData {
                    count: 1,
                    notes: [0.0; MAX_VOICES],
                    durations: [0.0; MAX_VOICES],
                    delays: [0.0; MAX_VOICES],
                    step_transpose: chord.step_transpose,
                };
                note_chord.notes[0] = chord.notes[note_idx];
                note_chord.durations[0] = chord.durations[note_idx];
                let note_sample_time = sample_time.saturating_add(
                    (note_delay as f64 * samples_per_step.max(0.0) as f64).round() as u64,
                );
                if queue
                    .push(ScheduledEvent {
                        pattern_epoch,
                        sample_time: note_sample_time,
                        kind: ScheduledEventKind::NetworkTrigger {
                            track: track_idx,
                            source_neuron,
                            seed,
                            samples_per_step,
                            resolved,
                            chord: note_chord,
                            effect_params: effect_params.clone(),
                            instrument_params: instrument_params.clone(),
                            sampler_params,
                            instrument_fingerprint,
                        },
                    })
                    .is_err()
                {
                    ok = false;
                    break;
                }
            }
            return ok;
        }
    }
    queue
        .push(ScheduledEvent {
            pattern_epoch,
            sample_time,
            kind: ScheduledEventKind::NetworkTrigger {
                track: track_idx,
                source_neuron,
                seed,
                samples_per_step,
                resolved,
                chord,
                effect_params,
                instrument_params,
                sampler_params,
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
    chord_delays: Vec<f32>,
    chord_step_transpose: f32,
    note_spans: Option<Vec<AccumulatorNoteSpan>>,
    arp_phase_beats: f32,
    effect_params: Vec<ScheduledEffectParam>,
    instrument_params: ScheduledInstrumentParams,
    sampler_params: ScheduledSamplerParams,
    source: EventSource,
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
    instrument_params: ScheduledInstrumentParams,
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
        chord_delays: step.chord_delays.clone(),
        chord_step_transpose: step.params[StepParam::Transpose.index()],
        note_spans: Some(track_note_spans_for_trigger(snapshot, track_idx, step_idx)),
        arp_phase_beats,
        effect_params,
        instrument_params,
        sampler_params: resolve_sampler_params(snapshot, track_idx, step_idx),
        source: EventSource::Step {
            track: track_idx,
            step: step_idx,
            instrument_fingerprint: 0,
        },
    }
}

fn midi_fx_event_step_for_track(
    snapshot: &SequencerSnapshot,
    track_idx: usize,
    step_idx: usize,
) -> usize {
    let step_count = snapshot
        .tracks
        .get(track_idx)
        .map(|track| track.steps.len().min(track.params.num_steps.max(1)))
        .unwrap_or(1)
        .max(1);
    step_idx.min(step_count.saturating_sub(1))
}

fn midi_fx_event_from_step_event(
    snapshot: &SequencerSnapshot,
    mut event: StepEvent,
    step_idx: usize,
    step_beats: f32,
    offset_beats: f32,
    arp_phase_beats: f32,
) -> MidiFxEvent {
    let step_idx = midi_fx_event_step_for_track(snapshot, event.track, step_idx);
    let chord = event.chord.notes[..event.chord.count].to_vec();
    let chord_durations = event.chord.durations[..event.chord.count].to_vec();
    let chord_delays = event.chord.delays[..event.chord.count].to_vec();
    event.sampler_params = match &event.source {
        EventSource::Network { .. } => event.sampler_params,
        EventSource::Step { .. } => resolve_sampler_params(snapshot, event.track, step_idx),
    };
    MidiFxEvent {
        offset_beats,
        track: event.track,
        step: step_idx,
        samples_per_step: event.samples_per_step,
        step_beats,
        resolved: event.resolved,
        chord,
        chord_durations,
        chord_delays,
        chord_step_transpose: event.chord.step_transpose,
        note_spans: None,
        arp_phase_beats,
        effect_params: event.effect_params,
        instrument_params: event.instrument_params,
        sampler_params: event.sampler_params,
        source: event.source,
    }
}

fn rebind_midi_fx_event_to_track(
    snapshot: &SequencerSnapshot,
    mut event: MidiFxEvent,
    target_track: usize,
) -> Option<MidiFxEvent> {
    if target_track >= snapshot.tracks.len() {
        return None;
    }
    if event.track == target_track {
        return Some(event);
    }
    let target_step = midi_fx_event_step_for_track(snapshot, target_track, event.step);
    event.track = target_track;
    event.step = target_step;
    event.effect_params = resolve_effect_params(snapshot, target_track, target_step);
    event.instrument_params = resolve_instrument_params(snapshot, target_track, target_step);
    event.sampler_params = resolve_sampler_params(snapshot, target_track, target_step);
    event.source = match event.source {
        EventSource::Network { seed, neuron, .. } => EventSource::Network {
            seed,
            neuron,
            instrument_fingerprint: 0,
        },
        EventSource::Step { .. } => EventSource::Step {
            track: target_track,
            step: target_step,
            instrument_fingerprint: 0,
        },
    };
    Some(event)
}

fn midi_fx_window_events_from_step(
    snapshot: &SequencerSnapshot,
    track_idx: usize,
    step_idx: usize,
    samples_per_step: f32,
    step_beats: f32,
    samples_per_quarter: f32,
    arp_phase_beats: f32,
    resolved: ResolvedStep,
    effect_params: Vec<ScheduledEffectParam>,
    instrument_params: ScheduledInstrumentParams,
) -> Vec<MidiFxEvent> {
    const EPS: f32 = 1e-5;
    const MAX_WINDOWS: usize = 1024;

    let note_spans = track_note_spans_for_trigger(snapshot, track_idx, step_idx);
    if note_spans.is_empty() {
        return vec![midi_fx_event_from_step(
            snapshot,
            track_idx,
            step_idx,
            samples_per_step,
            step_beats,
            arp_phase_beats,
            resolved,
            effect_params,
            instrument_params,
        )];
    }

    let window_beats = live_midi_fx_tick_beats(snapshot, track_idx, step_idx).max(EPS);
    let window_samples = (samples_per_quarter * window_beats).round().max(1.0);
    let end_beats = note_spans
        .iter()
        .map(|span| span.end_beats)
        .fold(0.0_f32, f32::max);
    if end_beats <= EPS {
        return Vec::new();
    }

    let window_count = ((end_beats / window_beats).ceil() as usize).min(MAX_WINDOWS);
    let mut events = Vec::with_capacity(window_count);
    for window_idx in 0..window_count {
        let window_start = window_idx as f32 * window_beats;
        let window_end = window_start + window_beats;
        let window_spans = note_spans
            .iter()
            .filter(|span| {
                span.end_beats > window_start + EPS && span.start_beats < window_end - EPS
            })
            .map(|span| AccumulatorNoteSpan {
                transpose: span.transpose,
                start_beats: (span.start_beats - window_start).max(0.0),
                end_beats: (span.end_beats - window_start).min(window_beats).max(0.0),
            })
            .filter(|span| span.end_beats > span.start_beats + EPS)
            .collect::<Vec<_>>();
        if window_spans.is_empty() {
            continue;
        }

        let chord = window_spans
            .iter()
            .map(|span| span.transpose)
            .collect::<Vec<_>>();
        let first_transpose = chord.first().copied().unwrap_or(resolved.transpose);
        let mut window_resolved = resolved;
        window_resolved.duration = 1.0;
        window_resolved.transpose = first_transpose;

        events.push(MidiFxEvent {
            offset_beats: window_start,
            track: track_idx,
            step: step_idx,
            samples_per_step: window_samples,
            step_beats: window_beats,
            resolved: window_resolved,
            chord_durations: vec![1.0; chord.len()],
            chord_delays: vec![0.0; chord.len()],
            chord,
            chord_step_transpose: 0.0,
            note_spans: Some(window_spans),
            arp_phase_beats: arp_phase_beats + window_start,
            effect_params: effect_params.clone(),
            instrument_params: instrument_params.clone(),
            sampler_params: resolve_sampler_params(snapshot, track_idx, step_idx),
            source: EventSource::Step {
                track: track_idx,
                step: step_idx,
                instrument_fingerprint: 0,
            },
        });
    }

    events
}

fn run_midi_fx_chain_for_track(
    runtime: &mut lisp_effect::ScratchControlRuntime,
    snapshot: &SequencerSnapshot,
    source_track: usize,
    events: Vec<MidiFxEvent>,
    depth: usize,
    debug_accum: bool,
) -> Vec<MidiFxEvent> {
    run_midi_fx_chain_for_track_inner(
        runtime,
        snapshot,
        source_track,
        events,
        depth,
        [false; MAX_TRACKS],
        debug_accum,
    )
}

fn run_midi_fx_chain_for_track_inner(
    runtime: &mut lisp_effect::ScratchControlRuntime,
    snapshot: &SequencerSnapshot,
    source_track: usize,
    events: Vec<MidiFxEvent>,
    depth: usize,
    mut visited_tracks: [bool; MAX_TRACKS],
    debug_accum: bool,
) -> Vec<MidiFxEvent> {
    if source_track >= snapshot.tracks.len() || depth >= MAX_TRACKS {
        return Vec::new();
    }
    if visited_tracks.get(source_track).copied().unwrap_or(true) {
        if debug_accum {
            eprintln!("[midi-fx] dropped recursive route into track={source_track}");
        }
        return Vec::new();
    }
    visited_tracks[source_track] = true;
    let chain = snapshot.tracks[source_track].params.midi_fx_chain.clone();
    if chain.is_empty() {
        return events;
    }
    let names = runtime.midi_fx_names();
    let descriptors = runtime.midi_fx_descriptors();
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
                if visited_tracks.get(event.track).copied().unwrap_or(true) {
                    if debug_accum {
                        eprintln!(
                            "[midi-fx] dropped recursive pending event into track={}",
                            event.track
                        );
                    }
                    continue;
                }
                next.extend(run_midi_fx_chain_for_track_inner(
                    runtime,
                    snapshot,
                    event.track,
                    vec![event],
                    depth + 1,
                    visited_tracks,
                    debug_accum,
                ));
                continue;
            }
            let slot_snapshot = snapshot.tracks[event.track]
                .midi_fx_slots
                .get(stage_idx)
                .cloned()
                .unwrap_or_else(crate::effects::EffectSlotSnapshot::new_empty);
            let enabled = descriptors
                .get(fx_idx)
                .and_then(|desc| {
                    desc.params
                        .iter()
                        .position(|param| param.name.eq_ignore_ascii_case("enabled"))
                })
                .and_then(|param_idx| {
                    slot_snapshot
                        .plocks
                        .get(event.step)
                        .and_then(|step| step.get(param_idx))
                        .copied()
                        .flatten()
                        .or_else(|| slot_snapshot.defaults.get(param_idx).copied())
                })
                .unwrap_or(1.0);
            if enabled <= 0.5 {
                next.push(event);
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
                slot_snapshot,
                event.arp_phase_beats,
                event.step_beats,
                snapshot.tracks[event.track].params.num_steps,
                snapshot.tracks[event.track].effect_slots.clone(),
                snapshot.tracks[event.track].instrument_slot.clone(),
                event.effect_params.clone(),
                event.instrument_params.to_vec(),
            ) {
                Ok(output) => {
                    if !output.suppressed {
                        let mut passthrough = event.clone();
                        passthrough.resolved = output.resolved;
                        passthrough.effect_params = output.effect_params.clone();
                        passthrough.instrument_params =
                            scheduled_instrument_params_from_vec(output.instrument_params.clone());
                        next.push(passthrough);
                    }
                    for emitted in output.emitted {
                        let target_track = emitted.track.unwrap_or(event.track);
                        if target_track >= snapshot.tracks.len() {
                            continue;
                        }
                        let chord_len = emitted.chord.len();
                        let routed = MidiFxEvent {
                            offset_beats: event.offset_beats + emitted.offset_beats,
                            track: event.track,
                            step: event.step,
                            samples_per_step: event.samples_per_step,
                            step_beats: event.step_beats,
                            resolved: emitted.resolved,
                            chord: emitted.chord,
                            chord_durations: emitted.chord_durations,
                            chord_delays: vec![0.0; chord_len],
                            chord_step_transpose: emitted.chord_step_transpose,
                            note_spans: None,
                            arp_phase_beats: event.arp_phase_beats,
                            effect_params: emitted.effect_params,
                            instrument_params: scheduled_instrument_params_from_vec(
                                emitted.instrument_params,
                            ),
                            sampler_params: event.sampler_params,
                            source: event.source.clone(),
                        };
                        if target_track == source_track {
                            next.push(routed);
                        } else if visited_tracks.get(target_track).copied().unwrap_or(true) {
                            if debug_accum {
                                eprintln!(
                                    "[midi-fx] dropped recursive emit track={source_track} target={target_track}"
                                );
                            }
                        } else if let Some(routed) =
                            rebind_midi_fx_event_to_track(snapshot, routed, target_track)
                        {
                            next.extend(run_midi_fx_chain_for_track_inner(
                                runtime,
                                snapshot,
                                target_track,
                                vec![routed],
                                depth + 1,
                                visited_tracks,
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

fn enqueue_midi_fx_events<const QUEUE_CAP: usize>(
    queue: &ScheduledEventQueue<QUEUE_CAP>,
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
            &event.chord_delays,
            event.resolved.duration,
            event.chord_step_transpose,
        );
        let instrument_fingerprint =
            instrument_sound_fingerprint(snapshot, event.track, &event.instrument_params);
        let enqueued = match event.source {
            EventSource::Network { seed, neuron, .. } => enqueue_network_trigger(
                queue,
                snapshot,
                pattern_epoch,
                sample_time,
                event.track,
                neuron,
                seed,
                event.samples_per_step,
                event.resolved,
                chord,
                event.effect_params,
                event.instrument_params,
                event.sampler_params,
                instrument_fingerprint,
            ),
            EventSource::Step { .. } => enqueue_resolved_trigger(
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
            ),
        };
        if !enqueued {
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
            let chord_delays = vec![0.0; chord.len()];
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
                chord_delays,
                chord_step_transpose: 0.0,
                note_spans: Some(note_spans),
                arp_phase_beats: (rendered_total_beats + tick_offset_beats) as f32,
                effect_params: resolve_effect_params(snapshot, track_idx, step),
                instrument_params: resolve_instrument_params(snapshot, track_idx, step),
                sampler_params: resolve_sampler_params(snapshot, track_idx, step),
                source: EventSource::Step {
                    track: track_idx,
                    step,
                    instrument_fingerprint: 0,
                },
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

fn sample_time_to_beats(
    chunk_start_beats: f64,
    chunk_start_sample: u64,
    sample_time: u64,
    samples_per_quarter: f64,
) -> f64 {
    let sample_delta = sample_time.saturating_sub(chunk_start_sample) as f64;
    chunk_start_beats + sample_delta / samples_per_quarter.max(1.0)
}

fn process_neural_boundaries_until(
    neural_runtime: &mut NeuralRuntime,
    cursor_beats: &mut f64,
    cursor_sample: &mut u64,
    target_beats: f64,
    target_sample: u64,
    samples_per_quarter: f64,
    out: &mut Vec<NeuralOutput>,
) {
    if target_beats <= *cursor_beats + 1e-9 {
        return;
    }
    neural_runtime.process_boundaries_with_outputs(
        *cursor_beats,
        target_beats,
        *cursor_sample,
        samples_per_quarter,
        out,
    );
    *cursor_beats = target_beats;
    *cursor_sample = target_sample;
}

fn should_reload_neural_runtime(
    loaded_networks: &Option<Vec<crate::neural::ProjectNeuralNetwork>>,
    snapshot_networks: &[crate::neural::ProjectNeuralNetwork],
    last_pattern: usize,
    pattern: usize,
) -> bool {
    last_pattern != pattern
        || loaded_networks
            .as_deref()
            .map(|networks| networks != snapshot_networks)
            .unwrap_or(true)
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
            let mut neural_runtime = NeuralRuntime::default();
            let mut generator_runtime = crate::generator::GeneratorRuntime::default();
            // Graph-mode sequencers: parallel vecs (manifest + live runtime), reconciled
            // by id from the published-sequencer channel. Held alongside the generator
            // runtime; both are additive layers over the neural/step output.
            let mut graph_manifests: Vec<crate::graph::GraphManifest> = Vec::new();
            let mut graph_runtimes: Vec<crate::graph::GraphRuntime> = Vec::new();
            let mut loaded_graph_overrides: Option<Vec<crate::graph::ProjectGraphOverrides>> = None;
            let mut loaded_neural_networks: Option<Vec<crate::neural::ProjectNeuralNetwork>> = None;
            let mut last_live_midi_fx_active = false;
            let mut scratch_source_version = u64::MAX;
            let mut published_sequencers_version = u64::MAX;
            let mut scratch_runtime = None;
            let debug_accum = std::env::var_os("TINYSEQ_DEBUG_ACCUM").is_some();
            let debug_graph = std::env::var_os("TINYSEQ_DEBUG_GRAPH").is_some();
            let mut debug_graph_drive_chunks = 0_u32;
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

                let latest_published_sequencers_version = state.published_sequencers_version();
                if latest_scratch_source_version != scratch_source_version
                    || latest_published_sequencers_version != published_sequencers_version
                {
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
                    // Register UI-authored generators (def-sequencer evaluated in any
                    // editor file, published via SequencerState). These need a runtime
                    // to live in even when there is no scratch/midi-fx source.
                    let published = state.published_sequencers();
                    if !published.is_empty() {
                        let runtime = scratch_runtime.get_or_insert_with(|| {
                            lisp_effect::scratch_runtime_with_fallbacks(Arc::clone(&state), 0, 0)
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
                    let generator_defs = scratch_runtime
                        .as_ref()
                        .map(|runtime| runtime.sequencer_defs())
                        .unwrap_or_default();
                    generator_runtime.sync_definitions(&generator_defs, clock.total_beats);

                    let new_manifests: Vec<crate::graph::GraphManifest> =
                        published.iter().filter_map(|s| s.graph.clone()).collect();
                    reconcile_graph_runtimes(
                        new_manifests,
                        &snapshot.graph_overrides,
                        &mut graph_runtimes,
                        &mut graph_manifests,
                        clock.total_beats,
                    );
                    if debug_graph {
                        eprintln!(
                            "[graph-reconcile] published={} graph_manifests={} runtimes={} overrides={}",
                            published.len(),
                            graph_manifests.len(),
                            graph_runtimes.len(),
                            snapshot.graph_overrides.len()
                        );
                    }
                    loaded_graph_overrides = Some(snapshot.graph_overrides.clone());
                }

                if loaded_graph_overrides.as_ref() != Some(&snapshot.graph_overrides) {
                    reconcile_graph_runtimes(
                        graph_manifests.clone(),
                        &snapshot.graph_overrides,
                        &mut graph_runtimes,
                        &mut graph_manifests,
                        clock.total_beats,
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
                if should_reload_neural_runtime(
                    &loaded_neural_networks,
                    &snapshot.neural_networks,
                    last_pattern,
                    pattern,
                )
                {
                    neural_runtime.load_from_networks(&snapshot.neural_networks, clock.total_beats);
                    loaded_neural_networks = Some(snapshot.neural_networks.clone());
                    state.set_neural_visualization(neural_runtime.visualization_snapshot());
                }
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
                    neural_runtime.reset_state(0.0);
                    generator_runtime.reset(0.0);
                    for graph in &mut graph_runtimes {
                        graph.reset(0.0);
                    }
                    state.set_neural_visualization(neural_runtime.visualization_snapshot());
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
                    neural_runtime.reset_state(clock.total_beats);
                    state.set_neural_visualization(neural_runtime.visualization_snapshot());
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
                    neural_runtime.reset_state(clock.total_beats);
                    state.set_neural_visualization(neural_runtime.visualization_snapshot());
                } else if last_topology_epoch != topology_epoch {
                    let previous_scheduled_until = scheduled_until_sample;
                    queue.clear();
                    clock.seek_to_rendered_position(&snapshot, rendered, previous_scheduled_until);
                    scheduled_until_sample = rendered;
                    pending_accum_reset = [true; MAX_TRACKS];
                    neural_runtime.reset_state(clock.total_beats);
                    state.set_neural_visualization(neural_runtime.visualization_snapshot());
                } else if last_pattern_epoch != pattern_epoch {
                    // Track topology edits bump pattern_epoch without changing the
                    // pattern index. Rebuild the scheduler horizon immediately so
                    // future triggers target the compacted live track layout.
                    let previous_scheduled_until = scheduled_until_sample;
                    queue.clear();
                    clock.seek_to_rendered_position(&snapshot, rendered, previous_scheduled_until);
                    scheduled_until_sample = rendered;
                    pending_accum_reset = [true; MAX_TRACKS];
                    neural_runtime.reset_state(clock.total_beats);
                    state.set_neural_visualization(neural_runtime.visualization_snapshot());
                } else if last_pattern != pattern {
                    // Pattern switches should replace future scheduled content without
                    // disturbing the current musical phase.
                    let previous_scheduled_until = scheduled_until_sample;
                    queue.clear();
                    clock.seek_to_rendered_position(&snapshot, rendered, previous_scheduled_until);
                    scheduled_until_sample = rendered;
                    pending_accum_reset = [true; MAX_TRACKS];
                    neural_runtime.reset_state(clock.total_beats);
                    state.set_neural_visualization(neural_runtime.visualization_snapshot());
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
                    let chunk_start_beats = clock.total_beats;
                    let triggers = clock.process_chunk(scheduler_block_size, &snapshot, &state);
                    let chunk_end_beats = clock.total_beats;
                    let mut neural_events = Vec::new();
                    let mut neural_cursor_beats = chunk_start_beats;
                    let mut neural_cursor_sample = scheduled_until_sample;
                    let mut chunk_enqueued = true;
                    let mut neural_reset_groups: Vec<(usize, f64)> = Vec::new();
                    for trigger in &triggers {
                        let step = &snapshot.tracks[trigger.track].steps[trigger.step];
                        if !step.active || !step.neural_reset {
                            continue;
                        }
                        let is_new_group =
                            neural_reset_groups.last().map_or(true, |(offset, beats)| {
                                *offset != trigger.offset
                                    || (*beats - trigger.absolute_beats).abs() > 1e-9
                            });
                        if is_new_group {
                            neural_reset_groups.push((trigger.offset, trigger.absolute_beats));
                        }
                    }
                    let mut neural_reset_group_idx = 0;
                    for trigger in triggers {
                        let trigger_sample_time = scheduled_until_sample + trigger.offset as u64;
                        process_neural_boundaries_until(
                            &mut neural_runtime,
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
                            chunk_enqueued &= enqueue_instrument_param_change(
                                &queue,
                                pattern_epoch,
                                sample_time,
                                trigger.track,
                                resolve_instrument_plocks(&snapshot, trigger.track, trigger.step),
                            );
                            if !chunk_enqueued {
                                break;
                            }
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
                        let step_boundary_sample_time =
                            scheduled_until_sample + trigger.offset as u64;
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
                                                chord_delays: step_snapshot.chord_delays.clone(),
                                                chord_step_transpose: step_snapshot.params
                                                    [StepParam::Transpose.index()],
                                                note_spans: Some(note_spans.clone()),
                                                arp_phase_beats: trigger.absolute_beats as f32,
                                                effect_params: output.effect_params.clone(),
                                                instrument_params:
                                                    scheduled_instrument_params_from_vec(
                                                        output.instrument_params.clone(),
                                                    ),
                                                sampler_params: resolve_sampler_params(
                                                    &snapshot,
                                                    trigger.track,
                                                    trigger.step,
                                                ),
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
                                                effect_params: emitted.effect_params,
                                                instrument_params:
                                                    scheduled_instrument_params_from_vec(
                                                        emitted.instrument_params,
                                                    ),
                                                sampler_params: resolve_sampler_params(
                                                    &snapshot,
                                                    trigger.track,
                                                    trigger.step,
                                                ),
                                                source: EventSource::Step {
                                                    track: trigger.track,
                                                    step: trigger.step,
                                                    instrument_fingerprint: 0,
                                                },
                                            };
                                            if let Some(event) = rebind_midi_fx_event_to_track(
                                                &snapshot,
                                                event,
                                                target_track,
                                            ) {
                                                accumulator_events.push(event);
                                            }
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
                                    let seed_chord =
                                        step_chord_data(&snapshot, target_track, trigger.step);
                                    let seed_event = step_event_from_resolved(
                                        &snapshot,
                                        target_track,
                                        trigger.step,
                                        trigger.samples_per_step,
                                        resolved,
                                        seed_chord,
                                        effect_params.clone(),
                                        instrument_params.clone(),
                                    );
                                    let events = midi_fx_window_events_from_step(
                                        &snapshot,
                                        target_track,
                                        trigger.step,
                                        trigger.samples_per_step,
                                        trigger.samples_per_step / samples_per_quarter,
                                        samples_per_quarter,
                                        trigger.absolute_beats as f32,
                                        resolved,
                                        effect_params,
                                        instrument_params,
                                    );
                                    let events = run_midi_fx_chain_for_track(
                                        runtime,
                                        &snapshot,
                                        target_track,
                                        events,
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
                                    let seed_beats = trigger.absolute_beats;
                                    neural_runtime.process_seed_at(&seed_event, seed_beats);
                                    seed_graph_runtimes(&mut graph_runtimes, &seed_event, seed_beats);
                                } else {
                                    let chord =
                                        step_chord_data(&snapshot, target_track, trigger.step);
                                    let step_event = step_event_from_resolved(
                                        &snapshot,
                                        target_track,
                                        trigger.step,
                                        trigger.samples_per_step,
                                        resolved,
                                        chord,
                                        effect_params,
                                        instrument_params,
                                    );
                                    let ok = enqueue_step_event(
                                        &queue,
                                        &snapshot,
                                        pattern_epoch,
                                        sample_time,
                                        step_event.clone(),
                                    );
                                    let seed_beats = trigger.absolute_beats;
                                    neural_runtime.process_seed_at(&step_event, seed_beats);
                                    seed_graph_runtimes(&mut graph_runtimes, &step_event, seed_beats);
                                    if !ok {
                                        chunk_enqueued = false;
                                        break;
                                    }
                                }
                            } else {
                                let chord = step_chord_data(&snapshot, target_track, trigger.step);
                                let step_event = step_event_from_resolved(
                                    &snapshot,
                                    target_track,
                                    trigger.step,
                                    trigger.samples_per_step,
                                    resolved,
                                    chord,
                                    effect_params,
                                    instrument_params,
                                );
                                let ok = enqueue_step_event(
                                    &queue,
                                    &snapshot,
                                    pattern_epoch,
                                    sample_time,
                                    step_event.clone(),
                                );
                                let seed_beats = trigger.absolute_beats;
                                neural_runtime.process_seed_at(&step_event, seed_beats);
                                seed_graph_runtimes(&mut graph_runtimes, &step_event, seed_beats);
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
                            &snapshot,
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
                    let mut merged_neural_events: Vec<NeuralOutput> = Vec::new();
                    for output in neural_events {
                        if output.emit_trigger {
                            if let Some(last_output) = merged_neural_events.last_mut() {
                                if last_output.emit_trigger
                                    && last_output.sample_time == output.sample_time
                                    && last_output.event.track == output.event.track
                                {
                                    last_output.event.resolved.velocity =
                                        (last_output.event.resolved.velocity
                                            + output.event.resolved.velocity)
                                        .min(1.0);
                                    continue;
                                }
                            }
                        }
                        merged_neural_events.push(output);
                    }
                    for output in merged_neural_events {
                        let sample_time = output.sample_time;
                        let event_beats = sample_time_to_beats(
                            chunk_start_beats,
                            scheduled_until_sample,
                            sample_time,
                            samples_per_quarter,
                        ) as f32;
                        if !enqueue_neural_output_with_midi_fx(
                            &queue,
                            &snapshot,
                            scratch_runtime.as_mut(),
                            pattern_epoch,
                            sample_time,
                            samples_per_quarter as f32,
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
                            generator_runtime.process_block(
                                chunk_start_beats,
                                chunk_end_beats,
                                scheduled_until_sample,
                                samples_per_quarter,
                                |input| {
                                    let generator_index = input.generator_index;
                                    let random_state = input.random_state;
                                    let fallback_state = input.state.clone();
                                    scratch.invoke_sequencer_tick(generator_index, input).unwrap_or(
                                        crate::generator::GeneratorTickResult {
                                            emitted: Vec::new(),
                                            random_state,
                                            state: fallback_state,
                                        },
                                    )
                                },
                                &mut generator_emissions,
                            );
                        }
                        // Velocity-merge coincident hits on the same track (accent, not
                        // polyphony) — same policy as the neural layer.
                        let mut merged_generator_emissions: Vec<crate::generator::GeneratorEmission> =
                            Vec::new();
                        for emission in generator_emissions {
                            if let Some(last) = merged_generator_emissions.last_mut() {
                                if last.sample_time == emission.sample_time
                                    && last.event.track == emission.event.track
                                {
                                    last.event.resolved.velocity = (last.event.resolved.velocity
                                        + emission.event.resolved.velocity)
                                        .min(1.0);
                                    continue;
                                }
                            }
                            merged_generator_emissions.push(emission);
                        }
                        for emission in merged_generator_emissions {
                            let track_idx = emission.event.track.unwrap_or(0);
                            if track_idx >= snapshot.tracks.len() {
                                continue;
                            }
                            let chord = chord_data_from_parts(
                                &emission.event.chord,
                                &emission.event.chord_durations,
                                &[],
                                emission.event.resolved.duration,
                                emission.event.chord_step_transpose,
                            );
                            if !enqueue_network_trigger(
                                &queue,
                                &snapshot,
                                pattern_epoch,
                                emission.sample_time,
                                track_idx,
                                emission.generator_index,
                                None,
                                samples_per_quarter as f32,
                                emission.event.resolved,
                                chord,
                                emission.event.effect_params,
                                scheduled_instrument_params_from_vec(emission.event.instrument_params),
                                resolve_sampler_params(&snapshot, track_idx, 0),
                                0,
                            ) {
                                chunk_enqueued = false;
                                break;
                            }
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
                            eprintln!(
                                "[graph-drive]   runtime[{i}] is_empty={}",
                                rt.is_empty()
                            );
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
                                                    manifest.name,
                                                    eval.node_index,
                                                    eval.beat,
                                                    error
                                                );
                                            }
                                            crate::graph::NodeFire::default()
                                        }
                                    }
                                },
                                &mut graph_emissions,
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
                        // Velocity-merge coincident hits on the same track (accent).
                        let mut merged_graph_emissions: Vec<crate::graph::GraphEmission> = Vec::new();
                        for emission in graph_emissions {
                            if let Some(last) = merged_graph_emissions.last_mut() {
                                if last.sample_time == emission.sample_time
                                    && last.event.track == emission.event.track
                                {
                                    last.event.resolved.velocity = (last.event.resolved.velocity
                                        + emission.event.resolved.velocity)
                                        .min(1.0);
                                    continue;
                                }
                            }
                            merged_graph_emissions.push(emission);
                        }
                        for emission in merged_graph_emissions {
                            let track_idx = emission.event.track.unwrap_or(0);
                            if track_idx >= snapshot.tracks.len() {
                                continue;
                            }
                            let chord = chord_data_from_parts(
                                &emission.event.chord,
                                &emission.event.chord_durations,
                                &[],
                                emission.event.resolved.duration,
                                emission.event.chord_step_transpose,
                            );
                            if !enqueue_network_trigger(
                                &queue,
                                &snapshot,
                                pattern_epoch,
                                emission.sample_time,
                                track_idx,
                                emission.node_index,
                                None,
                                samples_per_quarter as f32,
                                emission.event.resolved,
                                chord,
                                emission.event.effect_params,
                                scheduled_instrument_params_from_vec(
                                    emission.event.instrument_params,
                                ),
                                resolve_sampler_params(&snapshot, track_idx, 0),
                                0,
                            ) {
                                chunk_enqueued = false;
                                break;
                            }
                        }
                        if !chunk_enqueued {
                            break;
                        }
                    }
                    if log_graph_drive_chunk {
                        debug_graph_drive_chunks += 1;
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
        apply_fit_to_scale_to_trigger, apply_neuron_output_overrides, delayed_step_sample_time,
        enqueue_resolved_trigger, enqueue_step_event_with_midi_fx, midi_fx_window_events_from_step,
        quantized_live_tick_sample, reconcile_graph_runtimes, resolve_effect_params,
        resolve_instrument_plocks, resolve_sampler_params, run_midi_fx_chain_for_track,
        sample_time_to_beats, should_reload_neural_runtime, swung_network_sample_time,
        track_active_note_spans_at_beat, track_note_spans_for_trigger, MidiFxEvent,
        SnapshotSequencerClock,
    };
    use crate::accumulator::ResolvedStep;
    use crate::effects::{EffectDescriptor, ParamDescriptor, ParamKind, ParamScaling};
    use crate::graph::{
        EdgeSetSpec, GraphEmission, GraphManifest, GraphPayload, NodeEval, NodeFire, NodeProto,
        ParamSpec, ProjectGraphNodeIntrinsicOverride, ProjectGraphOverrides,
        ProjectGraphRouteOverride, SeedFrom, ShapeSpec, Topology,
    };
    use crate::lisp_effect;
    use crate::neural::{
        NeuralMaxPolySelection, NeuralOutput, ParamNodeId, ProjectEffectParamOverride,
        ProjectNeuralNetwork, ProjectNeuron, ProjectParamOverride,
    };
    use crate::scheduled_event::{
        resolved_chord_transpose, EventSource, ScheduledChordData, ScheduledEffectParam,
        ScheduledEventKind, ScheduledEventQueue, ScheduledInstrumentParam,
        ScheduledInstrumentParamTarget, ScheduledInstrumentParams, ScheduledSamplerParams,
        StepEvent,
    };
    use crate::sequencer::{
        default_empty_effect_chain, SequencerState, StepParam, SwingResolution, Timebase,
    };
    use std::sync::Arc;

    fn test_resolved_step() -> ResolvedStep {
        ResolvedStep {
            duration: 1.0,
            velocity: 1.0,
            speed: 1.0,
            aux_a: 0.0,
            aux_b: 0.0,
            transpose: 0.0,
            pan: 0.0,
            chop: 1.0,
        }
    }

    fn graph_manifest(id: u64, name: &str, shape: ShapeSpec) -> GraphManifest {
        GraphManifest {
            id,
            name: name.into(),
            shape,
            energy_decay: 1.0,
            reset_every_beats: 0.0,
            seed_on_reset: 0.0,
            max_poly: 0,
            max_poly_selection: NeuralMaxPolySelection::Deterministic,
            node: NodeProto {
                name: "n".into(),
                resolution: Timebase::Quarter,
                route: Some(0),
                seed_from: SeedFrom::Route,
                ..NodeProto::default()
            },
            edge_sets: vec![EdgeSetSpec {
                from: "n".into(),
                to: "n".into(),
                topology: Topology::AllToAll,
                gather_source: None,
                params: vec![ParamSpec {
                    name: "weight".into(),
                    min: 0.0,
                    max: 1.0,
                    default: 1.0,
                    is_int: false,
                }],
            }],
        }
    }

    fn graph_route_override(
        sequencer_id: u64,
        sequencer_name: &str,
        node_index: usize,
        route: usize,
    ) -> ProjectGraphOverrides {
        ProjectGraphOverrides {
            sequencer_id,
            sequencer_name: sequencer_name.into(),
            node_intrinsics: vec![ProjectGraphNodeIntrinsicOverride {
                group: "n".into(),
                instance: node_index,
                resolution: None,
                delay_steps: None,
                quantize: None,
                route: Some(ProjectGraphRouteOverride::Track(route)),
                seed_from: None,
            }],
            node_params: Vec::new(),
            edge_params: Vec::new(),
            reset_every_beats: None,
            max_poly: None,
        }
    }

    fn process_graph(
        runtime: &mut crate::graph::GraphRuntime,
        start_beats: f64,
        end_beats: f64,
    ) -> Vec<GraphEmission> {
        let mut out = Vec::new();
        runtime.process_block(
            start_beats,
            end_beats,
            0,
            48_000.0,
            0,
            |eval: &NodeEval| NodeFire {
                fired: eval.energy >= 1.0,
                ..NodeFire::default()
            },
            &mut out,
        );
        out
    }

    #[test]
    fn graph_override_reconcile_preserves_pending_runtime_state() {
        let manifest = graph_manifest(1, "g", ShapeSpec::Line(1));
        let mut manifests = Vec::new();
        let mut runtimes = Vec::new();
        reconcile_graph_runtimes(
            vec![manifest.clone()],
            &[],
            &mut runtimes,
            &mut manifests,
            0.0,
        );
        runtimes[0].seed(0, 0.0, GraphPayload::default());

        reconcile_graph_runtimes(
            vec![manifest],
            &[graph_route_override(1, "g", 0, 2)],
            &mut runtimes,
            &mut manifests,
            0.0,
        );

        let out = process_graph(&mut runtimes[0], 0.0, 1.0);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].event.track, Some(2));
    }

    #[test]
    fn graph_shape_change_rebuilds_and_clears_pending_state() {
        let mut manifests = Vec::new();
        let mut runtimes = Vec::new();
        reconcile_graph_runtimes(
            vec![graph_manifest(1, "g", ShapeSpec::Line(1))],
            &[],
            &mut runtimes,
            &mut manifests,
            0.0,
        );
        runtimes[0].seed(0, 0.0, GraphPayload::default());

        reconcile_graph_runtimes(
            vec![graph_manifest(1, "g", ShapeSpec::Line(2))],
            &[],
            &mut runtimes,
            &mut manifests,
            0.0,
        );

        assert_eq!(runtimes[0].num_nodes(), 2);
        let out = process_graph(&mut runtimes[0], 0.0, 1.0);
        assert!(out.is_empty());
    }

    #[test]
    fn graph_reconcile_tracks_multiple_graphs_by_id() {
        let mut manifests = Vec::new();
        let mut runtimes = Vec::new();
        let graph_a = graph_manifest(1, "a", ShapeSpec::Line(1));
        let graph_b = graph_manifest(2, "b", ShapeSpec::Line(1));
        reconcile_graph_runtimes(
            vec![graph_a.clone(), graph_b.clone()],
            &[],
            &mut runtimes,
            &mut manifests,
            0.0,
        );
        runtimes[0].seed(0, 0.0, GraphPayload::default());
        runtimes[1].seed(0, 0.0, GraphPayload::default());

        reconcile_graph_runtimes(
            vec![graph_a, graph_b],
            &[graph_route_override(1, "a", 0, 3)],
            &mut runtimes,
            &mut manifests,
            0.0,
        );

        let out_a = process_graph(&mut runtimes[0], 0.0, 1.0);
        let out_b = process_graph(&mut runtimes[1], 0.0, 1.0);
        assert_eq!(out_a.len(), 1);
        assert_eq!(out_b.len(), 1);
        assert_eq!(out_a[0].event.track, Some(3));
        assert_eq!(out_b[0].event.track, Some(0));
    }

    #[test]
    fn graph_reset_boundary_preserves_seed_from_snapshot_clock_trigger() {
        let state = SequencerState::new(
            2,
            vec![default_empty_effect_chain(), default_empty_effect_chain()],
        );
        state.toggle_play();
        state.toggle_step_and_clear_plocks(0, 0);
        let snapshot = state.latest_scheduler_snapshot();
        let mut clock = SnapshotSequencerClock::new(48_000);
        let samples_per_quarter = 48_000.0 * 60.0 / snapshot.transport.bpm as f64;

        let mut seed_node = crate::graph::GraphNode {
            resolution: Timebase::Sixteenth,
            seed_track_mask: crate::graph::seed_track_mask(&[0]),
            ..crate::graph::GraphNode::default()
        };
        seed_node.route = Some(0);
        let routed_node = crate::graph::GraphNode {
            resolution: Timebase::Sixteenth,
            route: Some(1),
            ..crate::graph::GraphNode::default()
        };
        let mut runtime = crate::graph::GraphRuntime::new(
            1,
            "g".into(),
            vec![seed_node, routed_node],
            vec![crate::graph::GraphEdge::new(0, 1, 1.0)],
            1.0,
            4.0,
        );

        let mut scheduled_until_sample = 0_u64;
        let mut emitted = Vec::new();
        while clock.total_beats < 4.5 {
            let chunk_start_beats = clock.total_beats;
            let triggers = clock.process_chunk(512, &snapshot, &state);
            let chunk_end_beats = clock.total_beats;
            for trigger in triggers {
                if trigger.track == 0 && trigger.step == 0 {
                    let seed_beats = trigger.absolute_beats;
                    runtime.seed(
                        trigger.track,
                        seed_beats,
                        crate::graph::GraphPayload::default(),
                    );
                }
            }
            runtime.process_block(
                chunk_start_beats,
                chunk_end_beats,
                scheduled_until_sample,
                samples_per_quarter,
                0,
                |eval: &NodeEval| NodeFire {
                    fired: eval.energy >= 1.0,
                    ..NodeFire::default()
                },
                &mut emitted,
            );
            scheduled_until_sample = scheduled_until_sample.saturating_add(512);
        }

        assert!(
            emitted
                .iter()
                .any(|emission| emission.event.track == Some(1) && emission.sample_time > 96_000),
            "bar-start seed should survive the one-bar reset and re-drive the graph: {emitted:?}"
        );
    }

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
    fn delayed_step_sample_time_offsets_by_fraction_of_step() {
        let mut params = [0.0; crate::sequencer::NUM_PARAMS];
        params[StepParam::Delay.index()] = 0.5;

        assert_eq!(delayed_step_sample_time(1_000, &params, 6_000.0), 4_000);
    }

    #[test]
    fn enqueue_resolved_trigger_splits_note_delays() {
        let state = SequencerState::new(1, vec![default_empty_effect_chain()]);
        let snapshot = state.publish_scheduler_snapshot();
        let queue = ScheduledEventQueue::<8>::new();
        let resolved = ResolvedStep {
            duration: 1.0,
            velocity: 1.0,
            speed: 1.0,
            aux_a: 0.0,
            aux_b: 0.0,
            transpose: 0.0,
            pan: 0.0,
            chop: 1.0,
        };
        let mut chord = ScheduledChordData {
            count: 2,
            notes: [0.0; crate::voice::MAX_VOICES],
            durations: [1.0; crate::voice::MAX_VOICES],
            delays: [0.0; crate::voice::MAX_VOICES],
            step_transpose: 0.0,
        };
        chord.notes[1] = 7.0;
        chord.delays[1] = 0.5;

        assert!(enqueue_resolved_trigger(
            &queue,
            &snapshot,
            0,
            1_000,
            0,
            0,
            6_000.0,
            resolved,
            chord,
            Vec::new(),
            ScheduledInstrumentParams::new(),
        ));

        let first = queue.pop().expect("first note event");
        let second = queue.pop().expect("second note event");
        assert_eq!(first.sample_time, 1_000);
        assert_eq!(second.sample_time, 4_000);
        assert!(queue.pop().is_none());
    }

    #[test]
    fn network_trigger_uses_target_track_swing() {
        let state = SequencerState::new(1, vec![default_empty_effect_chain()]);
        state.pattern.track_params[0].set_swing(75.0);
        state.pattern.track_params[0].set_swing_resolution(SwingResolution::Sixteenth);
        let snapshot = state.publish_scheduler_snapshot();
        let event = StepEvent {
            track: 0,
            samples_per_step: 12_000.0,
            resolved: test_resolved_step(),
            chord: ScheduledChordData {
                count: 0,
                notes: [0.0; crate::voice::MAX_VOICES],
                durations: [0.0; crate::voice::MAX_VOICES],
                delays: [0.0; crate::voice::MAX_VOICES],
                step_transpose: 0.0,
            },
            effect_params: Vec::new(),
            instrument_params: ScheduledInstrumentParams::new(),
            sampler_params: ScheduledSamplerParams::default(),
            source: EventSource::Network {
                seed: Some((0, 0)),
                neuron: 0,
                instrument_fingerprint: 0,
            },
        };

        assert_eq!(
            swung_network_sample_time(&snapshot, &event, 12_000, 0.25, 48_000.0),
            18_000
        );
        assert_eq!(
            swung_network_sample_time(&snapshot, &event, 24_000, 0.5, 48_000.0),
            24_000
        );
    }

    #[test]
    fn network_trigger_enqueue_runs_target_midi_fx_chain() {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        state.pattern.track_params[0].set_midi_fx_chain(vec!["octave".to_string()]);
        let snapshot = state.publish_scheduler_snapshot();
        let mut runtime = lisp_effect::ScratchControlRuntime::new(
            Arc::clone(&state),
            vec![Vec::new()],
            vec![EffectDescriptor::builtin_sampler()],
            0,
            0,
        );
        runtime
            .eval(
                r#"
                (def-midi-fx "octave"
                  (do
                    (fx-suppress)
                    (fx-emit 0 :transpose 12)))
                "#,
            )
            .unwrap();
        let queue = ScheduledEventQueue::<8>::new();
        let event = StepEvent {
            track: 0,
            samples_per_step: 12_000.0,
            resolved: test_resolved_step(),
            chord: ScheduledChordData {
                count: 0,
                notes: [0.0; crate::voice::MAX_VOICES],
                durations: [0.0; crate::voice::MAX_VOICES],
                delays: [0.0; crate::voice::MAX_VOICES],
                step_transpose: 0.0,
            },
            effect_params: Vec::new(),
            instrument_params: ScheduledInstrumentParams::new(),
            sampler_params: ScheduledSamplerParams::default(),
            source: EventSource::Network {
                seed: None,
                neuron: 0,
                instrument_fingerprint: 0,
            },
        };

        assert!(enqueue_step_event_with_midi_fx(
            &queue,
            &snapshot,
            Some(&mut runtime),
            0,
            1_000,
            48_000.0,
            0.0,
            event,
            false,
        ));
        let scheduled = queue.pop().expect("MIDI FX output event");
        match scheduled.kind {
            ScheduledEventKind::NetworkTrigger {
                track,
                resolved,
                source_neuron,
                ..
            } => {
                assert_eq!(track, 0);
                assert_eq!(source_neuron, 0);
                assert_eq!(resolved.transpose, 12.0);
            }
            other => panic!("expected network trigger, got {other:?}"),
        }
        assert!(queue.pop().is_none());
    }

    #[test]
    fn network_trigger_enqueue_applies_target_track_fit_to_scale() {
        let state = SequencerState::new(
            2,
            vec![default_empty_effect_chain(), default_empty_effect_chain()],
        );
        state.pattern.track_params[1].set_fts_scale(1);
        let snapshot = state.publish_scheduler_snapshot();
        let queue = ScheduledEventQueue::<8>::new();
        let mut event = StepEvent {
            track: 1,
            samples_per_step: 12_000.0,
            resolved: test_resolved_step(),
            chord: ScheduledChordData {
                count: 0,
                notes: [0.0; crate::voice::MAX_VOICES],
                durations: [0.0; crate::voice::MAX_VOICES],
                delays: [0.0; crate::voice::MAX_VOICES],
                step_transpose: 0.0,
            },
            effect_params: Vec::new(),
            instrument_params: ScheduledInstrumentParams::new(),
            sampler_params: ScheduledSamplerParams::default(),
            source: EventSource::Network {
                seed: Some((0, 0)),
                neuron: 0,
                instrument_fingerprint: 0,
            },
        };
        event.resolved.transpose = 3.2;

        assert!(super::enqueue_neural_output_with_midi_fx(
            &queue,
            &snapshot,
            None,
            0,
            1_000,
            48_000.0,
            0.0,
            NeuralOutput {
                sample_time: 1_000,
                event,
                emit_trigger: true,
            },
            false,
        ));
        let scheduled = queue.pop().expect("network trigger");
        match scheduled.kind {
            ScheduledEventKind::NetworkTrigger {
                track, resolved, ..
            } => {
                assert_eq!(track, 1);
                assert_eq!(resolved.transpose, 4.0);
            }
            other => panic!("expected network trigger, got {other:?}"),
        }
        assert!(queue.pop().is_none());
    }

    #[test]
    fn midi_fx_track_send_applies_destination_fit_to_scale() {
        let state = Arc::new(SequencerState::new(
            2,
            vec![default_empty_effect_chain(), default_empty_effect_chain()],
        ));
        state.pattern.track_params[0].set_midi_fx_chain(vec!["send".to_string()]);
        state.pattern.track_params[1].set_fts_scale(1);
        let snapshot = state.publish_scheduler_snapshot();
        let mut runtime = lisp_effect::ScratchControlRuntime::new(
            Arc::clone(&state),
            vec![Vec::new(), Vec::new()],
            vec![
                EffectDescriptor::builtin_sampler(),
                EffectDescriptor::builtin_sampler(),
            ],
            0,
            0,
        );
        runtime
            .eval(
                r#"
                (def-midi-fx "send"
                  (do
                    (fx-suppress)
                    (fx-emit 0 :track 1 :transpose 3.2)))
                "#,
            )
            .unwrap();
        let queue = ScheduledEventQueue::<8>::new();
        let event = StepEvent {
            track: 0,
            samples_per_step: 12_000.0,
            resolved: test_resolved_step(),
            chord: ScheduledChordData {
                count: 0,
                notes: [0.0; crate::voice::MAX_VOICES],
                durations: [0.0; crate::voice::MAX_VOICES],
                delays: [0.0; crate::voice::MAX_VOICES],
                step_transpose: 0.0,
            },
            effect_params: Vec::new(),
            instrument_params: ScheduledInstrumentParams::new(),
            sampler_params: ScheduledSamplerParams::default(),
            source: EventSource::Network {
                seed: Some((0, 0)),
                neuron: 0,
                instrument_fingerprint: 0,
            },
        };

        assert!(enqueue_step_event_with_midi_fx(
            &queue,
            &snapshot,
            Some(&mut runtime),
            0,
            1_000,
            48_000.0,
            0.0,
            event,
            false,
        ));
        let scheduled = queue.pop().expect("routed network trigger");
        match scheduled.kind {
            ScheduledEventKind::NetworkTrigger {
                track, resolved, ..
            } => {
                assert_eq!(track, 1);
                assert_eq!(resolved.transpose, 4.0);
            }
            other => panic!("expected network trigger, got {other:?}"),
        }
        assert!(queue.pop().is_none());
    }

    #[test]
    fn trigger_to_track_midi_fx_copies_one_network_trigger_once() {
        let state = Arc::new(SequencerState::new(
            2,
            vec![default_empty_effect_chain(), default_empty_effect_chain()],
        ));
        state.pattern.track_params[0].set_midi_fx_chain(vec!["copy-to-track-2".to_string()]);
        let snapshot = state.publish_scheduler_snapshot();
        let mut runtime = lisp_effect::ScratchControlRuntime::new(
            Arc::clone(&state),
            vec![Vec::new(), Vec::new()],
            vec![
                EffectDescriptor::builtin_sampler(),
                EffectDescriptor::builtin_sampler(),
            ],
            0,
            0,
        );
        runtime
            .eval(
                r#"
                (def-midi-fx "copy-to-track-2"
                  (fx-emit 0 :track 1))
                "#,
            )
            .unwrap();
        let queue = ScheduledEventQueue::<8>::new();
        let event = StepEvent {
            track: 0,
            samples_per_step: 12_000.0,
            resolved: test_resolved_step(),
            chord: ScheduledChordData {
                count: 0,
                notes: [0.0; crate::voice::MAX_VOICES],
                durations: [0.0; crate::voice::MAX_VOICES],
                delays: [0.0; crate::voice::MAX_VOICES],
                step_transpose: 0.0,
            },
            effect_params: Vec::new(),
            instrument_params: ScheduledInstrumentParams::new(),
            sampler_params: ScheduledSamplerParams::default(),
            source: EventSource::Network {
                seed: Some((0, 0)),
                neuron: 0,
                instrument_fingerprint: 0,
            },
        };

        assert!(enqueue_step_event_with_midi_fx(
            &queue,
            &snapshot,
            Some(&mut runtime),
            0,
            1_000,
            48_000.0,
            0.0,
            event,
            false,
        ));

        let first = queue.pop().expect("source network trigger");
        let second = queue.pop().expect("target network trigger");
        let mut tracks = [usize::MAX; 2];
        for (idx, scheduled) in [first, second].into_iter().enumerate() {
            assert_eq!(scheduled.sample_time, 1_000);
            match scheduled.kind {
                ScheduledEventKind::NetworkTrigger { track, .. } => tracks[idx] = track,
                other => panic!("expected network trigger, got {other:?}"),
            }
        }
        tracks.sort();
        assert_eq!(tracks, [0, 1]);
        assert!(queue.pop().is_none());
    }

    #[test]
    fn trigger_to_track_midi_fx_drops_recursive_route_cycles() {
        let state = Arc::new(SequencerState::new(
            2,
            vec![default_empty_effect_chain(), default_empty_effect_chain()],
        ));
        state.pattern.track_params[0].set_midi_fx_chain(vec!["copy-to-track-2".to_string()]);
        state.pattern.track_params[1].set_midi_fx_chain(vec!["copy-to-track-1".to_string()]);
        let snapshot = state.publish_scheduler_snapshot();
        let mut runtime = lisp_effect::ScratchControlRuntime::new(
            Arc::clone(&state),
            vec![Vec::new(), Vec::new()],
            vec![
                EffectDescriptor::builtin_sampler(),
                EffectDescriptor::builtin_sampler(),
            ],
            0,
            0,
        );
        runtime
            .eval(
                r#"
                (def-midi-fx "copy-to-track-2"
                  (fx-emit 0 :track 1))

                (def-midi-fx "copy-to-track-1"
                  (fx-emit 0 :track 0))
                "#,
            )
            .unwrap();
        let event = MidiFxEvent {
            offset_beats: 0.0,
            track: 0,
            step: 0,
            samples_per_step: 12_000.0,
            step_beats: 0.25,
            resolved: test_resolved_step(),
            chord: Vec::new(),
            chord_durations: Vec::new(),
            chord_delays: Vec::new(),
            chord_step_transpose: 0.0,
            note_spans: None,
            arp_phase_beats: 0.0,
            effect_params: Vec::new(),
            instrument_params: ScheduledInstrumentParams::new(),
            sampler_params: ScheduledSamplerParams::default(),
            source: EventSource::Network {
                seed: Some((0, 0)),
                neuron: 0,
                instrument_fingerprint: 0,
            },
        };

        let events = run_midi_fx_chain_for_track(&mut runtime, &snapshot, 0, vec![event], 0, false);
        let tracks = events.iter().map(|event| event.track).collect::<Vec<_>>();

        assert_eq!(tracks, vec![0, 1]);
    }

    #[test]
    fn fit_to_scale_preserves_chord_accumulator_offset_after_scheduler_quantize() {
        let state = SequencerState::new(1, vec![default_empty_effect_chain()]);
        state.pattern.track_params[0].set_fts_scale(1);
        let snapshot = state.publish_scheduler_snapshot();
        let mut resolved = test_resolved_step();
        resolved.transpose = 3.2;
        let mut chord = ScheduledChordData {
            count: 2,
            notes: [0.0; crate::voice::MAX_VOICES],
            durations: [1.0; crate::voice::MAX_VOICES],
            delays: [0.0; crate::voice::MAX_VOICES],
            step_transpose: 2.0,
        };
        chord.notes[0] = 3.0;
        chord.notes[1] = 6.0;

        let (resolved, chord) = apply_fit_to_scale_to_trigger(&snapshot, 0, resolved, chord);

        assert_eq!(resolved.transpose, 4.0);
        assert_eq!(
            resolved_chord_transpose(chord.notes[0], chord.step_transpose, resolved.transpose),
            4.0
        );
        assert_eq!(
            resolved_chord_transpose(chord.notes[1], chord.step_transpose, resolved.transpose),
            7.0
        );
    }

    #[test]
    fn midi_fx_track_send_rebinds_target_params_before_target_chain() {
        let state = Arc::new(SequencerState::new(
            2,
            vec![default_empty_effect_chain(), default_empty_effect_chain()],
        ));
        state.pattern.track_params[0].set_midi_fx_chain(vec!["send".to_string()]);
        state.pattern.track_params[1].set_midi_fx_chain(vec!["octave".to_string()]);
        state.pattern.instrument_slots[1]
            .apply_descriptor(&EffectDescriptor::builtin_sampler(), 77);
        state.pattern.instrument_slots[1].defaults.set(12, 2.5);
        let snapshot = state.publish_scheduler_snapshot();
        let mut runtime = lisp_effect::ScratchControlRuntime::new(
            Arc::clone(&state),
            vec![Vec::new(), Vec::new()],
            vec![
                EffectDescriptor::builtin_sampler(),
                EffectDescriptor::builtin_sampler(),
            ],
            0,
            0,
        );
        runtime
            .eval(
                r#"
                (def-midi-fx "send"
                  (fx-emit 0 :track 1))

                (def-midi-fx "octave"
                  (do
                    (fx-suppress)
                    (fx-emit 0 :transpose 12)))
                "#,
            )
            .unwrap();
        let event = MidiFxEvent {
            offset_beats: 0.0,
            track: 0,
            step: 0,
            samples_per_step: 12_000.0,
            step_beats: 0.25,
            resolved: test_resolved_step(),
            chord: Vec::new(),
            chord_durations: Vec::new(),
            chord_delays: Vec::new(),
            chord_step_transpose: 0.0,
            note_spans: None,
            arp_phase_beats: 0.0,
            effect_params: Vec::new(),
            instrument_params: ScheduledInstrumentParams::new(),
            sampler_params: ScheduledSamplerParams::default(),
            source: EventSource::Step {
                track: 0,
                step: 0,
                instrument_fingerprint: 0,
            },
        };

        let events = run_midi_fx_chain_for_track(&mut runtime, &snapshot, 0, vec![event], 0, false);
        let target = events
            .iter()
            .find(|event| event.track == 1)
            .expect("routed target event");
        assert_eq!(target.resolved.transpose, 12.0);
        assert!(target
            .instrument_params
            .iter()
            .any(|param| param.idx == crate::sampler::PARAM_SPEED as u64 && param.value == 2.5));
    }

    #[test]
    fn neural_runtime_reload_ignores_non_network_snapshot_edits() {
        let mut network = ProjectNeuralNetwork::default();
        network.id = 1;
        network.num_neurons = 1;
        network.neurons.truncate(1);
        network.weights = vec![vec![0.0]];

        let loaded = Some(vec![network.clone()]);
        assert!(!should_reload_neural_runtime(
            &loaded,
            &[network.clone()],
            0,
            0
        ));
        assert!(should_reload_neural_runtime(
            &loaded,
            &[network.clone()],
            0,
            1
        ));

        let mut edited_network = network;
        edited_network.neurons[0].threshold = 0.5;
        assert!(should_reload_neural_runtime(
            &loaded,
            &[edited_network],
            0,
            0
        ));
    }

    #[test]
    fn resolve_instrument_plocks_returns_only_plocked_params_on_inactive_steps() {
        let state = SequencerState::new(1, vec![default_empty_effect_chain()]);
        let track = 0;
        let step = 3;
        state.pattern.instrument_slots[track]
            .apply_descriptor(&EffectDescriptor::builtin_sampler(), 12);
        state.pattern.instrument_slots[track].set_plock(step, 12, 2.0);
        state.pattern.instrument_slots[track].set_plock(step, 13, 0.25);

        let snapshot = state.publish_scheduler_snapshot();
        assert!(!snapshot.tracks[track].steps[step].active);

        let params = resolve_instrument_plocks(&snapshot, track, step);

        assert_eq!(
            params.as_slice(),
            vec![
                ScheduledInstrumentParam {
                    target: ScheduledInstrumentParamTarget::Synth,
                    idx: crate::sampler::PARAM_SPEED,
                    span: 1,
                    value: 2.0,
                },
                ScheduledInstrumentParam {
                    target: ScheduledInstrumentParamTarget::Synth,
                    idx: crate::sampler::PARAM_SCRUB_OFFSET,
                    span: 1,
                    value: 0.25,
                },
            ]
        );
    }

    #[test]
    fn resolve_instrument_plocks_drops_stale_identity() {
        let state = SequencerState::new(1, vec![default_empty_effect_chain()]);
        let track = 0;
        let step = 3;
        state.pattern.instrument_slots[track]
            .apply_descriptor(&EffectDescriptor::builtin_sampler(), 12);
        state.pattern.instrument_slots[track]
            .plocks
            .set(step, 12, 2.0);

        let snapshot = state.publish_scheduler_snapshot();
        let params = resolve_instrument_plocks(&snapshot, track, step);

        assert!(params.is_empty());
    }

    #[test]
    fn resolve_sampler_params_drops_stale_identity() {
        let state = SequencerState::new(1, vec![default_empty_effect_chain()]);
        let track = 0;
        let step = 3;
        state.pattern.instrument_slots[track]
            .apply_descriptor(&EffectDescriptor::builtin_sampler(), 12);
        state.pattern.instrument_slots[track]
            .plocks
            .set(step, 12, 2.0);

        let snapshot = state.publish_scheduler_snapshot();
        let params = resolve_sampler_params(&snapshot, track, step);

        assert_eq!(params.playback_speed, 1.0);
    }

    #[test]
    fn resolve_effect_params_routes_modulator_params_to_effect_bank() {
        let state = SequencerState::new(1, vec![default_empty_effect_chain()]);
        let track = 0;
        let step = 3;
        let desc = EffectDescriptor {
            name: "modded effect".to_string(),
            input_channels: 6,
            output_channels: 2,
            instrument_modulators: Vec::new(),
            instrument_modulation_targets: Vec::new(),
            params: vec![
                ParamDescriptor {
                    name: "gain".to_string(),
                    min: 0.0,
                    max: 1.0,
                    default: 0.5,
                    kind: ParamKind::Continuous { unit: None },
                    scaling: ParamScaling::Linear,
                    node_param_idx: 12,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
                ParamDescriptor {
                    name: "mod1_source".to_string(),
                    min: 0.0,
                    max: 8.0,
                    default: 0.0,
                    kind: ParamKind::Enum {
                        labels: vec!["off".to_string(), "lfo".to_string()],
                    },
                    scaling: ParamScaling::Linear,
                    node_param_idx: crate::voice_modulator::MOD_PARAM_BASE
                        + crate::voice_modulator::PARAM_SLOT_SOURCE as u32,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
            ],
        };
        state.pattern.effect_chains[track][0].apply_descriptor_with_modulator(&desc, 42, 77);
        state.pattern.effect_chains[track][0].set_plock(step, 0, 0.75);
        state.pattern.effect_chains[track][0].set_plock(step, 1, 1.0);

        let snapshot = state.publish_scheduler_snapshot();
        let params = resolve_effect_params(&snapshot, track, step);

        assert_eq!(
            params,
            vec![
                ScheduledEffectParam {
                    logical_id: 42,
                    idx: 12,
                    value: 0.75,
                },
                ScheduledEffectParam {
                    logical_id: 77,
                    idx: crate::voice_modulator::PARAM_SLOT_SOURCE as u64,
                    value: 1.0,
                },
            ]
        );
    }

    #[test]
    fn neuron_output_overrides_apply_only_with_matching_identity() {
        let state = SequencerState::new(1, vec![default_empty_effect_chain()]);
        let track = 0;
        state.pattern.instrument_slots[track]
            .apply_descriptor(&EffectDescriptor::builtin_sampler(), 12);
        let mut snapshot = (*state.publish_scheduler_snapshot()).clone();
        let mut neuron = ProjectNeuron::default();
        neuron
            .output_overrides
            .instrument
            .push(ProjectParamOverride {
                target_track: track,
                param_id: ParamNodeId {
                    logical_id: 12,
                    node_param_idx: crate::sampler::PARAM_SPEED as u32,
                },
                param_index: 12,
                value: 2.5,
            });
        snapshot.neural_networks = vec![ProjectNeuralNetwork {
            id: 1,
            name: "test".to_string(),
            enabled: true,
            num_neurons: 1,
            weights: vec![vec![0.0]],
            neurons: vec![neuron],
            ..ProjectNeuralNetwork::default()
        }];
        let mut event = StepEvent {
            track,
            samples_per_step: 1.0,
            resolved: test_resolved_step(),
            chord: ScheduledChordData {
                count: 0,
                notes: [0.0; crate::voice::MAX_VOICES],
                durations: [0.0; crate::voice::MAX_VOICES],
                delays: [0.0; crate::voice::MAX_VOICES],
                step_transpose: 0.0,
            },
            effect_params: Vec::new(),
            instrument_params: ScheduledInstrumentParams::new(),
            sampler_params: ScheduledSamplerParams::default(),
            source: EventSource::Network {
                seed: None,
                neuron: 0,
                instrument_fingerprint: 0,
            },
        };

        let parameter_events =
            apply_neuron_output_overrides(&snapshot, 0, Some(event.track), &mut event);
        assert!(parameter_events.instrument.is_empty());
        assert!(parameter_events.effects.is_empty());

        assert_eq!(event.sampler_params.playback_speed, 2.5);
        assert_eq!(
            event.instrument_params.as_slice(),
            &[ScheduledInstrumentParam {
                target: ScheduledInstrumentParamTarget::Synth,
                idx: crate::sampler::PARAM_SPEED,
                span: 1,
                value: 2.5,
            }]
        );

        let mut stale_snapshot = snapshot.clone();
        stale_snapshot.neural_networks[0].neurons[0]
            .output_overrides
            .instrument[0]
            .param_id = ParamNodeId {
            logical_id: 99,
            node_param_idx: crate::sampler::PARAM_SPEED as u32,
        };
        let mut stale_event = event.clone();
        stale_event.instrument_params.clear();
        stale_event.sampler_params = ScheduledSamplerParams::default();

        let parameter_events = apply_neuron_output_overrides(
            &stale_snapshot,
            0,
            Some(stale_event.track),
            &mut stale_event,
        );
        assert!(parameter_events.instrument.is_empty());
        assert!(parameter_events.effects.is_empty());

        assert!(stale_event.instrument_params.is_empty());
        assert_eq!(stale_event.sampler_params.playback_speed, 1.0);
    }

    #[test]
    fn neuron_effect_output_overrides_match_modulator_identity() {
        let state = SequencerState::new(1, vec![default_empty_effect_chain()]);
        let track = 0;
        let desc = EffectDescriptor {
            name: "modded effect".to_string(),
            input_channels: 6,
            output_channels: 2,
            instrument_modulators: Vec::new(),
            instrument_modulation_targets: Vec::new(),
            params: vec![ParamDescriptor {
                name: "mod1_source".to_string(),
                min: 0.0,
                max: 8.0,
                default: 0.0,
                kind: ParamKind::Enum {
                    labels: vec!["off".to_string(), "lfo".to_string()],
                },
                scaling: ParamScaling::Linear,
                node_param_idx: crate::voice_modulator::MOD_PARAM_BASE
                    + crate::voice_modulator::PARAM_SLOT_SOURCE as u32,
                node_param_span: 1,
                host_control: None,
                ui_metadata: None,
            }],
        };
        state.pattern.effect_chains[track][0].apply_descriptor_with_modulator(&desc, 42, 77);
        let mut snapshot = (*state.publish_scheduler_snapshot()).clone();
        let mut neuron = ProjectNeuron::default();
        neuron
            .output_overrides
            .effects
            .push(ProjectEffectParamOverride {
                target_track: track,
                slot_index: 0,
                param_id: ParamNodeId {
                    logical_id: 77,
                    node_param_idx: crate::voice_modulator::PARAM_SLOT_SOURCE as u32,
                },
                param_index: 0,
                value: 1.0,
            });
        snapshot.neural_networks = vec![ProjectNeuralNetwork {
            id: 1,
            name: "test".to_string(),
            enabled: true,
            num_neurons: 1,
            weights: vec![vec![0.0]],
            neurons: vec![neuron],
            ..ProjectNeuralNetwork::default()
        }];
        let mut event = StepEvent {
            track,
            samples_per_step: 1.0,
            resolved: test_resolved_step(),
            chord: ScheduledChordData {
                count: 0,
                notes: [0.0; crate::voice::MAX_VOICES],
                durations: [0.0; crate::voice::MAX_VOICES],
                delays: [0.0; crate::voice::MAX_VOICES],
                step_transpose: 0.0,
            },
            effect_params: Vec::new(),
            instrument_params: ScheduledInstrumentParams::new(),
            sampler_params: ScheduledSamplerParams::default(),
            source: EventSource::Network {
                seed: None,
                neuron: 0,
                instrument_fingerprint: 0,
            },
        };

        let parameter_events =
            apply_neuron_output_overrides(&snapshot, 0, Some(event.track), &mut event);
        assert!(parameter_events.instrument.is_empty());
        assert!(parameter_events.effects.is_empty());

        assert_eq!(
            event.effect_params,
            vec![ScheduledEffectParam {
                logical_id: 77,
                idx: crate::voice_modulator::PARAM_SLOT_SOURCE as u64,
                value: 1.0,
            }]
        );

        let mut stale_snapshot = snapshot.clone();
        stale_snapshot.neural_networks[0].neurons[0]
            .output_overrides
            .effects[0]
            .param_id = ParamNodeId {
            logical_id: 42,
            node_param_idx: crate::voice_modulator::PARAM_SLOT_SOURCE as u32,
        };
        event.effect_params.clear();

        let parameter_events =
            apply_neuron_output_overrides(&stale_snapshot, 0, Some(event.track), &mut event);
        assert!(parameter_events.instrument.is_empty());
        assert!(parameter_events.effects.is_empty());

        assert!(event.effect_params.is_empty());
    }

    #[test]
    fn hidden_neuron_emits_target_parameter_events_without_network_trigger() {
        let state = SequencerState::new(
            2,
            vec![default_empty_effect_chain(), default_empty_effect_chain()],
        );
        let sampler_desc = EffectDescriptor::builtin_sampler();
        let sampler_speed_param_idx = sampler_desc
            .params
            .iter()
            .position(|param| param.name == "speed")
            .expect("sampler speed param");
        let sampler_speed_node_param_idx =
            sampler_desc.params[sampler_speed_param_idx].node_param_idx;
        state.pattern.instrument_slots[1].apply_descriptor(&sampler_desc, 12);
        state.pattern.effect_chains[1][0].apply_descriptor(&EffectDescriptor::builtin_filter(), 42);
        let mut snapshot = (*state.publish_scheduler_snapshot()).clone();
        let filter_param_idx = 0;
        let filter_node_param_idx =
            EffectDescriptor::builtin_filter().params[filter_param_idx].node_param_idx;
        let mut neuron = ProjectNeuron::default();
        neuron.route = None;
        neuron
            .output_overrides
            .instrument
            .push(ProjectParamOverride {
                target_track: 1,
                param_id: ParamNodeId {
                    logical_id: 12,
                    node_param_idx: sampler_speed_node_param_idx,
                },
                param_index: sampler_speed_param_idx,
                value: 1.75,
            });
        neuron
            .output_overrides
            .effects
            .push(ProjectEffectParamOverride {
                target_track: 1,
                slot_index: 0,
                param_id: ParamNodeId {
                    logical_id: 42,
                    node_param_idx: filter_node_param_idx,
                },
                param_index: filter_param_idx,
                value: 640.0,
            });
        snapshot.neural_networks = vec![ProjectNeuralNetwork {
            id: 1,
            name: "hidden".to_string(),
            enabled: true,
            num_neurons: 1,
            weights: vec![vec![0.0]],
            neurons: vec![neuron],
            ..ProjectNeuralNetwork::default()
        }];
        let event = StepEvent {
            track: 0,
            samples_per_step: 1.0,
            resolved: test_resolved_step(),
            chord: ScheduledChordData {
                count: 0,
                notes: [0.0; crate::voice::MAX_VOICES],
                durations: [0.0; crate::voice::MAX_VOICES],
                delays: [0.0; crate::voice::MAX_VOICES],
                step_transpose: 0.0,
            },
            effect_params: Vec::new(),
            instrument_params: ScheduledInstrumentParams::new(),
            sampler_params: ScheduledSamplerParams::default(),
            source: EventSource::Network {
                seed: None,
                neuron: 0,
                instrument_fingerprint: 0,
            },
        };
        let queue = ScheduledEventQueue::<8>::new();

        assert!(super::enqueue_neural_output_with_midi_fx(
            &queue,
            &snapshot,
            None,
            7,
            1234,
            48_000.0,
            0.0,
            NeuralOutput {
                sample_time: 1234,
                event,
                emit_trigger: false,
            },
            false,
        ));

        let first = queue.pop().expect("instrument parameter event");
        assert_eq!(first.pattern_epoch, 7);
        assert_eq!(first.sample_time, 1234);
        match first.kind {
            ScheduledEventKind::InstrumentParams {
                track,
                instrument_params,
            } => {
                assert_eq!(track, 1);
                assert_eq!(
                    instrument_params.as_slice(),
                    &[ScheduledInstrumentParam {
                        target: ScheduledInstrumentParamTarget::Synth,
                        idx: sampler_speed_node_param_idx as u64,
                        span: 1,
                        value: 1.75,
                    }]
                );
            }
            other => panic!("expected instrument params, got {other:?}"),
        }

        let second = queue.pop().expect("effect parameter event");
        assert_eq!(second.pattern_epoch, 7);
        assert_eq!(second.sample_time, 1234);
        match second.kind {
            ScheduledEventKind::EffectParams {
                track,
                effect_params,
            } => {
                assert_eq!(track, 1);
                assert_eq!(
                    effect_params,
                    vec![ScheduledEffectParam {
                        logical_id: 42,
                        idx: filter_node_param_idx as u64,
                        value: 640.0,
                    }]
                );
            }
            other => panic!("expected effect params, got {other:?}"),
        }
        assert!(queue.pop().is_none());
    }

    #[test]
    fn routed_neuron_emits_cross_track_parameter_event_before_own_trigger() {
        let state = SequencerState::new(
            2,
            vec![default_empty_effect_chain(), default_empty_effect_chain()],
        );
        state.pattern.effect_chains[1][0].apply_descriptor(&EffectDescriptor::builtin_filter(), 42);
        let mut snapshot = (*state.publish_scheduler_snapshot()).clone();
        let filter_param_idx = 0;
        let filter_node_param_idx =
            EffectDescriptor::builtin_filter().params[filter_param_idx].node_param_idx;
        let mut neuron = ProjectNeuron::default();
        neuron.route = Some(0);
        neuron
            .output_overrides
            .effects
            .push(ProjectEffectParamOverride {
                target_track: 1,
                slot_index: 0,
                param_id: ParamNodeId {
                    logical_id: 42,
                    node_param_idx: filter_node_param_idx,
                },
                param_index: filter_param_idx,
                value: 900.0,
            });
        snapshot.neural_networks = vec![ProjectNeuralNetwork {
            id: 1,
            name: "cross".to_string(),
            enabled: true,
            num_neurons: 1,
            weights: vec![vec![0.0]],
            neurons: vec![neuron],
            ..ProjectNeuralNetwork::default()
        }];
        let event = StepEvent {
            track: 0,
            samples_per_step: 1.0,
            resolved: test_resolved_step(),
            chord: ScheduledChordData {
                count: 0,
                notes: [0.0; crate::voice::MAX_VOICES],
                durations: [0.0; crate::voice::MAX_VOICES],
                delays: [0.0; crate::voice::MAX_VOICES],
                step_transpose: 0.0,
            },
            effect_params: Vec::new(),
            instrument_params: ScheduledInstrumentParams::new(),
            sampler_params: ScheduledSamplerParams::default(),
            source: EventSource::Network {
                seed: Some((0, 0)),
                neuron: 0,
                instrument_fingerprint: 0,
            },
        };
        let queue = ScheduledEventQueue::<8>::new();

        assert!(super::enqueue_neural_output_with_midi_fx(
            &queue,
            &snapshot,
            None,
            7,
            1234,
            48_000.0,
            0.0,
            NeuralOutput {
                sample_time: 1234,
                event,
                emit_trigger: true,
            },
            false,
        ));

        let first = queue.pop().expect("cross-track effect parameter event");
        match first.kind {
            ScheduledEventKind::EffectParams {
                track,
                effect_params,
            } => {
                assert_eq!(track, 1);
                assert_eq!(
                    effect_params,
                    vec![ScheduledEffectParam {
                        logical_id: 42,
                        idx: filter_node_param_idx as u64,
                        value: 900.0,
                    }]
                );
            }
            other => panic!("expected cross-track effect params, got {other:?}"),
        }

        let second = queue.pop().expect("routed network trigger");
        match second.kind {
            ScheduledEventKind::NetworkTrigger {
                track,
                effect_params,
                source_neuron,
                ..
            } => {
                assert_eq!(track, 0);
                assert_eq!(source_neuron, 0);
                assert!(effect_params.is_empty());
            }
            other => panic!("expected routed network trigger, got {other:?}"),
        }
        assert!(queue.pop().is_none());
    }

    #[test]
    fn resolve_instrument_plocks_preserves_param_node_spans() {
        let state = SequencerState::new(1, vec![default_empty_effect_chain()]);
        let track = 0;
        let step = 3;
        let desc = EffectDescriptor {
            name: "custom".to_string(),
            input_channels: 0,
            output_channels: 1,
            instrument_modulators: Vec::new(),
            instrument_modulation_targets: Vec::new(),
            params: vec![
                ParamDescriptor {
                    name: "cutoff".to_string(),
                    min: 80.0,
                    max: 12_000.0,
                    default: 7200.0,
                    kind: ParamKind::Continuous {
                        unit: Some("Hz".to_string()),
                    },
                    scaling: ParamScaling::Linear,
                    node_param_idx: 105,
                    node_param_span: 4,
                    host_control: None,
                    ui_metadata: None,
                },
                ParamDescriptor {
                    name: "__dgen_mod_active__cutoff".to_string(),
                    min: 0.0,
                    max: 1.0,
                    default: 0.0,
                    kind: ParamKind::Boolean,
                    scaling: ParamScaling::Linear,
                    node_param_idx: 109,
                    node_param_span: 1,
                    host_control: None,
                    ui_metadata: None,
                },
            ],
        };
        state.pattern.instrument_slots[track].apply_descriptor(&desc, 12);
        state.pattern.instrument_slots[track].set_plock(step, 0, 9155.0);
        state.pattern.instrument_slots[track].set_plock(step, 1, 1.0);

        let snapshot = state.publish_scheduler_snapshot();
        let params = resolve_instrument_plocks(&snapshot, track, step);

        assert_eq!(
            params.as_slice(),
            vec![
                ScheduledInstrumentParam {
                    target: ScheduledInstrumentParamTarget::Synth,
                    idx: 105,
                    span: 4,
                    value: 9155.0,
                },
                ScheduledInstrumentParam {
                    target: ScheduledInstrumentParamTarget::Synth,
                    idx: 109,
                    span: 1,
                    value: 1.0,
                },
            ]
        );
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
    fn track_note_spans_include_step_delay_in_start_time() {
        let state = SequencerState::new(1, vec![default_empty_effect_chain()]);
        let track = 0;

        state.pattern.patterns[track].set_step_active(0, true);
        state.pattern.step_data[track].set(0, StepParam::Delay, 0.5);
        state.pattern.step_data[track].set(0, StepParam::Duration, 0.5);

        state.pattern.patterns[track].set_step_active(1, true);
        state.pattern.step_data[track].set(1, StepParam::Transpose, 7.0);
        state.pattern.step_data[track].set(1, StepParam::Delay, 0.25);
        state.pattern.step_data[track].set(1, StepParam::Duration, 1.0);

        let snapshot = state.publish_scheduler_snapshot();
        let first_group = track_note_spans_for_trigger(&snapshot, track, 0);
        assert_eq!(first_group.len(), 1);
        assert_eq!(first_group[0].start_beats, 0.0);
        assert_eq!(first_group[0].end_beats, 0.125);

        let later_group = track_note_spans_for_trigger(&snapshot, track, 1);
        assert_eq!(later_group.len(), 1);
        assert_eq!(later_group[0].transpose, 7.0);
        assert_eq!(later_group[0].start_beats, 0.0);
        assert_eq!(later_group[0].end_beats, 0.25);
    }

    #[test]
    fn track_note_spans_include_per_note_delays_for_strums() {
        let state = SequencerState::new(1, vec![default_empty_effect_chain()]);
        let track = 0;

        state.pattern.patterns[track].set_step_active(0, true);
        state.pattern.chord_data[track].add_note_with_timing(0, 0.0, 1.0, 0.0);
        state.pattern.chord_data[track].add_note_with_timing(0, 4.0, 1.0, 0.25);
        state.pattern.chord_data[track].add_note_with_timing(0, 7.0, 1.0, 0.5);

        let snapshot = state.publish_scheduler_snapshot();
        let spans = track_note_spans_for_trigger(&snapshot, track, 0);

        assert_eq!(spans.len(), 3);
        assert_eq!(spans[0].transpose, 0.0);
        assert_eq!(spans[0].start_beats, 0.0);
        assert_eq!(spans[1].transpose, 4.0);
        assert_eq!(spans[1].start_beats, 0.0625);
        assert_eq!(spans[2].transpose, 7.0);
        assert_eq!(spans[2].start_beats, 0.125);
    }

    #[test]
    fn track_note_spans_include_strums_with_no_gridline_note() {
        let state = SequencerState::new(1, vec![default_empty_effect_chain()]);
        let track = 0;

        state.pattern.patterns[track].set_step_active(0, true);
        state.pattern.chord_data[track].add_note_with_timing(0, 0.0, 1.0, 0.25);
        state.pattern.chord_data[track].add_note_with_timing(0, 4.0, 1.0, 0.5);

        let snapshot = state.publish_scheduler_snapshot();
        let spans = track_note_spans_for_trigger(&snapshot, track, 0);

        assert_eq!(spans.len(), 2);
        assert_eq!(spans[0].start_beats, 0.0625);
        assert_eq!(spans[1].start_beats, 0.125);
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
    fn midi_fx_window_events_clip_recorded_notes_to_tick_windows() {
        let state = SequencerState::new(1, vec![default_empty_effect_chain()]);
        let track = 0;

        state.pattern.patterns[track].set_step_active(0, true);
        state.pattern.chord_data[track].add_note_with_duration(0, 0.0, 8.0);
        state.pattern.chord_data[track].add_note_with_duration(0, 4.0, 8.0);
        state.pattern.chord_data[track].add_note_with_duration(0, 7.0, 8.0);
        state.pattern.step_data[track].set(0, StepParam::Duration, 8.0);

        let snapshot = state.publish_scheduler_snapshot();
        let events = midi_fx_window_events_from_step(
            &snapshot,
            track,
            0,
            6_000.0,
            0.25,
            24_000.0,
            0.0,
            ResolvedStep {
                duration: 8.0,
                velocity: 0.8,
                speed: 1.0,
                aux_a: 0.0,
                aux_b: 0.0,
                transpose: 0.0,
                pan: 0.0,
                chop: 1.0,
            },
            Vec::new(),
            ScheduledInstrumentParams::new(),
        );

        assert_eq!(events.len(), 8);
        for (idx, event) in events.iter().enumerate() {
            assert_eq!(event.offset_beats, idx as f32 * 0.25);
            assert_eq!(event.samples_per_step, 6_000.0);
            assert_eq!(event.step_beats, 0.25);
            assert_eq!(event.resolved.duration, 1.0);
            assert_eq!(event.chord, vec![0.0, 4.0, 7.0]);
            let spans = event.note_spans.as_ref().expect("window spans");
            assert_eq!(spans.len(), 3);
            assert!(spans.iter().all(|span| span.start_beats == 0.0));
            assert!(spans.iter().all(|span| span.end_beats <= 0.25));
            assert_eq!(event.arp_phase_beats, idx as f32 * 0.25);
        }
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
