use std::cell::RefCell;
use std::collections::{hash_map::DefaultHasher, BTreeMap, HashMap};
use std::hash::{Hash, Hasher};
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use crate::accumulator::{
    apply_limit_mode, AccumMode, AccumulatorRuntimeState, ResolvedStep, StepAction,
    ACCUMULATOR_REGISTRY,
};
use crate::effects::EffectDescriptor;
use crate::lisp_host::{self, AccumulatorNoteSpan};
use crate::neural::{NeuralOutput, NeuralRuntime, ParamNodeId};
use crate::process::ProcessMidiFxParamOverride;
use crate::scheduled_event::{
    resolved_chord_transpose, EventSource, ScheduledChordData, ScheduledEffectParam,
    ScheduledEvent, ScheduledEventKind, ScheduledEventQueue, ScheduledInstrumentParam,
    ScheduledInstrumentParamTarget, ScheduledInstrumentParams, ScheduledInstrumentTensorParam,
    ScheduledInstrumentTensorParams, ScheduledSamplerParams, StepEvent,
};
use crate::sequencer::{
    sync_beats, InstrumentType, KeyboardTrigger, MidiFxPosition, SequencerSnapshot, SequencerState,
    StepParam, SwingResolution, TrackOutputEvent, MAX_STEPS, MAX_TRACKS,
};
use crate::voice::MAX_VOICES;

// The scheduler lookahead pass carries sizeable event values in debug builds, and
// tests call the same extracted production pass. Keep the stack budget explicit
// instead of depending on platform thread defaults.
const SCHEDULER_THREAD_STACK_SIZE: usize = 16 * 1024 * 1024;
const PROCESS_EVENT_CASCADE_LIMIT: usize = 1024;

fn scheduled_instrument_params_from_vec(
    params: Vec<ScheduledInstrumentParam>,
) -> ScheduledInstrumentParams {
    params.into_iter().collect::<ScheduledInstrumentParams>()
}

fn scheduled_instrument_tensor_params_from_vec(
    params: Vec<ScheduledInstrumentTensorParam>,
) -> ScheduledInstrumentTensorParams {
    params
        .into_iter()
        .collect::<ScheduledInstrumentTensorParams>()
}

fn debug_routing_enabled() -> bool {
    std::env::var_os("TINYSEQ_DEBUG_ROUTING").is_some()
}

fn event_source_label(source: &EventSource) -> &'static str {
    match source {
        EventSource::Step { .. } => "step",
        EventSource::Network { .. } => "network",
    }
}

fn upsert_instrument_params(
    params: &mut ScheduledInstrumentParams,
    overrides: impl IntoIterator<Item = ScheduledInstrumentParam>,
) {
    for override_param in overrides {
        if let Some(existing) = params.iter_mut().find(|existing| {
            existing.target == override_param.target && existing.idx == override_param.idx
        }) {
            *existing = override_param;
        } else if !params.is_full() {
            params.push(override_param);
        }
    }
    params.sort_by_key(|param| match param.target {
        ScheduledInstrumentParamTarget::Synth => (0_u8, param.idx),
        ScheduledInstrumentParamTarget::Modulator => (1_u8, param.idx),
    });
}

fn upsert_instrument_tensor_params(
    params: &mut ScheduledInstrumentTensorParams,
    overrides: impl IntoIterator<Item = ScheduledInstrumentTensorParam>,
) {
    for override_param in overrides {
        if let Some(existing) = params
            .iter_mut()
            .find(|existing| existing.cell_offset == override_param.cell_offset)
        {
            *existing = override_param;
        } else if !params.is_full() {
            params.push(override_param);
        }
    }
    params.sort_by_key(|param| param.cell_offset);
}

fn upsert_effect_params(
    params: &mut Vec<ScheduledEffectParam>,
    overrides: impl IntoIterator<Item = ScheduledEffectParam>,
) {
    for override_param in overrides {
        if let Some(existing) = params.iter_mut().find(|existing| {
            existing.logical_id == override_param.logical_id && existing.idx == override_param.idx
        }) {
            *existing = override_param;
        } else {
            params.push(override_param);
        }
    }
    params.sort_by_key(|param| (param.logical_id, param.idx));
}

fn ceil_to_grid(value: f64, grid: f64) -> f64 {
    let rem = value % grid;
    if rem > 1e-9 {
        value + (grid - rem)
    } else {
        value
    }
}

fn snap_near_grid_down(value: f64, grid: f64, tolerance: f64) -> f64 {
    let rem = value.rem_euclid(grid);
    if rem <= tolerance {
        value - rem
    } else {
        value
    }
}

#[derive(Clone, Copy)]
struct SnapshotTrigger {
    track: usize,
    step: usize,
    offset: usize,
    cycle: u64,
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
                            if !track.scene_silenced {
                                triggers.push(SnapshotTrigger {
                                    track: t,
                                    step,
                                    offset,
                                    cycle: (self.total_beats / cycle).floor().max(0.0) as u64,
                                    cycle_start_beats: tc.boundaries[step],
                                    absolute_beats: self.total_beats,
                                    samples_per_step,
                                });
                            }
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

fn slot_param_index_by_node_idx(
    slot: &crate::effects::EffectSlotSnapshot,
    node_param_idx: u32,
) -> Option<usize> {
    let num_params = slot.num_params as usize;
    (0..num_params).find(|&param_idx| {
        slot.param_node_indices
            .get(param_idx)
            .copied()
            .unwrap_or(param_idx as u32)
            == node_param_idx
    })
}

fn resolved_slot_node_param_value(
    slot: &crate::effects::EffectSlotSnapshot,
    step_idx: usize,
    node_param_idx: u32,
    default: f32,
) -> f32 {
    let Some(param_idx) = slot_param_index_by_node_idx(slot, node_param_idx) else {
        return default;
    };
    resolved_slot_param_value(slot, step_idx, param_idx, default)
}

fn default_slot_node_param_value(
    slot: &crate::effects::EffectSlotSnapshot,
    node_param_idx: u32,
    default: f32,
) -> f32 {
    let Some(param_idx) = slot_param_index_by_node_idx(slot, node_param_idx) else {
        return default;
    };
    slot.defaults.get(param_idx).copied().unwrap_or(default)
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

fn resolve_instrument_tensor_params(
    snapshot: &SequencerSnapshot,
    track_idx: usize,
    step_idx: usize,
) -> ScheduledInstrumentTensorParams {
    let slot = &snapshot.tracks[track_idx].instrument_slot;
    let mut params = ScheduledInstrumentTensorParams::new();
    for tensor in &slot.tensor_params {
        let values = tensor
            .plocks
            .get(step_idx)
            .and_then(|values| values.as_ref())
            .unwrap_or(&tensor.default);
        if values.len() != tensor.default.len() || values.iter().any(|value| !value.is_finite()) {
            continue;
        }
        if params.is_full() {
            break;
        }
        params.push(ScheduledInstrumentTensorParam {
            cell_offset: tensor.cell_offset,
            values: values.clone(),
        });
    }
    params.sort_by_key(|param| param.cell_offset);
    params
}

fn resolve_instrument_tensor_defaults(
    snapshot: &SequencerSnapshot,
    track_idx: usize,
) -> ScheduledInstrumentTensorParams {
    let slot = &snapshot.tracks[track_idx].instrument_slot;
    let mut params = ScheduledInstrumentTensorParams::new();
    for tensor in &slot.tensor_params {
        if tensor.default.iter().any(|value| !value.is_finite()) {
            continue;
        }
        if params.is_full() {
            break;
        }
        params.push(ScheduledInstrumentTensorParam {
            cell_offset: tensor.cell_offset,
            values: tensor.default.clone(),
        });
    }
    params.sort_by_key(|param| param.cell_offset);
    params
}

fn resolve_instrument_tensor_plocks(
    snapshot: &SequencerSnapshot,
    track_idx: usize,
    step_idx: usize,
) -> ScheduledInstrumentTensorParams {
    let slot = &snapshot.tracks[track_idx].instrument_slot;
    let mut params = ScheduledInstrumentTensorParams::new();
    for tensor in &slot.tensor_params {
        let Some(values) = tensor
            .plocks
            .get(step_idx)
            .and_then(|values| values.as_ref())
        else {
            continue;
        };
        if values.len() != tensor.default.len() || values.iter().any(|value| !value.is_finite()) {
            continue;
        }
        if params.is_full() {
            break;
        }
        params.push(ScheduledInstrumentTensorParam {
            cell_offset: tensor.cell_offset,
            values: values.clone(),
        });
    }
    params.sort_by_key(|param| param.cell_offset);
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

fn enqueue_instrument_param_change<const QUEUE_CAP: usize>(
    queue: &ScheduledEventQueue<QUEUE_CAP>,
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
                instrument_tensor_params: ScheduledInstrumentTensorParams::new(),
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
    Some(midi_fx_slot_param_value(slot, step_idx, param_idx, 0.0))
}

fn midi_fx_slot_param_value(
    slot: &crate::effects::EffectSlotSnapshot,
    step_idx: usize,
    param_idx: usize,
    default: f32,
) -> f32 {
    slot.plocks
        .get(step_idx)
        .and_then(|step| step.get(param_idx))
        .copied()
        .flatten()
        .or_else(|| slot.defaults.get(param_idx).copied())
        .unwrap_or(default)
}

const MIDI_FX_CLOCK_RATE_ROLE: &str = "clock-rate";
const MIDI_FX_QUANTIZE_GRID_ROLE: &str = "quantize-grid";

#[derive(Clone, Copy)]
struct MidiFxClockParam {
    slot_idx: usize,
    param_idx: usize,
}

fn midi_fx_param_has_role(param: &crate::effects::ParamDescriptor, role: &str) -> bool {
    param
        .ui_metadata
        .as_ref()
        .and_then(|metadata| metadata.role.as_deref())
        .is_some_and(|param_role| param_role.eq_ignore_ascii_case(role))
}

fn midi_fx_chain_clock_param(
    snapshot: &SequencerSnapshot,
    descriptors: &[EffectDescriptor],
    track_idx: usize,
) -> Option<MidiFxClockParam> {
    let track = snapshot.tracks.get(track_idx)?;
    track
        .params
        .midi_fx_chain
        .iter()
        .enumerate()
        .find_map(|(slot_idx, fx_name)| {
            descriptors
                .iter()
                .find(|desc| desc.name.eq_ignore_ascii_case(fx_name))
                .and_then(|desc| {
                    desc.params
                        .iter()
                        .position(|param| midi_fx_param_has_role(param, MIDI_FX_CLOCK_RATE_ROLE))
                })
                .map(|param_idx| MidiFxClockParam {
                    slot_idx,
                    param_idx,
                })
        })
}

fn midi_fx_clock_tick_beats(
    snapshot: &SequencerSnapshot,
    descriptors: &[EffectDescriptor],
    track_idx: usize,
    step_idx: usize,
) -> Option<f32> {
    let Some(clock_param) = midi_fx_chain_clock_param(snapshot, descriptors, track_idx) else {
        if debug_routing_enabled() {
            let chain = snapshot
                .tracks
                .get(track_idx)
                .map(|track| track.params.midi_fx_chain.as_slice())
                .unwrap_or(&[]);
            eprintln!(
                "[midi-fx-clock] none track={} step={} chain={:?} descriptors={:?}",
                track_idx,
                step_idx,
                chain,
                descriptors
                    .iter()
                    .map(|desc| desc.name.as_str())
                    .collect::<Vec<_>>()
            );
        }
        return None;
    };
    let Some(raw_idx) = resolve_midi_fx_slot_param(
        snapshot,
        track_idx,
        clock_param.slot_idx,
        clock_param.param_idx,
        step_idx,
    ) else {
        if debug_routing_enabled() {
            eprintln!(
                "[midi-fx-clock] missing-param-value track={} step={} slot={} param={}",
                track_idx, step_idx, clock_param.slot_idx, clock_param.param_idx
            );
        }
        return None;
    };
    let timebase_idx = raw_idx.round().max(0.0) as usize;
    let Some(timebase) = crate::sequencer::Timebase::ALL.get(timebase_idx).copied() else {
        if debug_routing_enabled() {
            eprintln!(
                "[midi-fx-clock] invalid-timebase track={} step={} raw_idx={} rounded_idx={} all_count={}",
                track_idx,
                step_idx,
                raw_idx,
                timebase_idx,
                crate::sequencer::Timebase::ALL.len()
            );
        }
        return None;
    };
    let beats = timebase.step_beats(snapshot.tracks[track_idx].params.num_steps) as f32;
    if beats <= 0.0 {
        if debug_routing_enabled() {
            eprintln!(
                "[midi-fx-clock] nonpositive track={} step={} raw_idx={} beats={}",
                track_idx, step_idx, raw_idx, beats
            );
        }
        return None;
    }
    if debug_routing_enabled() {
        eprintln!(
            "[midi-fx-clock] track={} step={} slot={} param={} raw_idx={} beats={}",
            track_idx, step_idx, clock_param.slot_idx, clock_param.param_idx, raw_idx, beats
        );
    }
    Some(beats)
}

fn midi_fx_timebase_param_beats(
    snapshot: &SequencerSnapshot,
    track_idx: usize,
    slot_idx: usize,
    param_idx: usize,
    step_idx: usize,
) -> Option<f32> {
    let raw_idx = resolve_midi_fx_slot_param(snapshot, track_idx, slot_idx, param_idx, step_idx)?;
    let timebase_idx = raw_idx.round().max(0.0) as usize;
    let timebase = crate::sequencer::Timebase::ALL.get(timebase_idx).copied()?;
    let beats = timebase.step_beats(snapshot.tracks[track_idx].params.num_steps) as f32;
    (beats > 0.0).then_some(beats)
}

fn midi_fx_timebase_param_beats_from_slot(
    snapshot: &SequencerSnapshot,
    track_idx: usize,
    slot: &crate::effects::EffectSlotSnapshot,
    param_idx: usize,
    step_idx: usize,
) -> Option<f32> {
    if param_idx >= slot.num_params as usize {
        return None;
    }
    let raw_idx = midi_fx_slot_param_value(slot, step_idx, param_idx, 0.0);
    let timebase_idx = raw_idx.round().max(0.0) as usize;
    let timebase = crate::sequencer::Timebase::ALL.get(timebase_idx).copied()?;
    let beats = timebase.step_beats(snapshot.tracks[track_idx].params.num_steps) as f32;
    (beats > 0.0).then_some(beats)
}

fn midi_fx_quantizer_grid_param(descriptor: &EffectDescriptor) -> Option<usize> {
    descriptor
        .params
        .iter()
        .position(|param| midi_fx_param_has_role(param, MIDI_FX_QUANTIZE_GRID_ROLE))
}

fn instrument_sound_fingerprint(
    snapshot: &SequencerSnapshot,
    track_idx: usize,
    instrument_params: &[ScheduledInstrumentParam],
    instrument_tensor_params: &[ScheduledInstrumentTensorParam],
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
    for tensor in instrument_tensor_params {
        tensor.cell_offset.hash(&mut hasher);
        for value in &tensor.values {
            value.to_bits().hash(&mut hasher);
        }
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
        warp_preserve: resolved_slot_node_param_value(
            slot,
            step_idx,
            crate::sampler::PARAM_WARP_PRESERVE as u32,
            crate::sampler::WARP_PRESERVE_DEFAULT as f32,
        ),
        warp_seg_loop_mode: resolved_slot_node_param_value(
            slot,
            step_idx,
            crate::sampler::PARAM_WARP_SEG_LOOP_MODE as u32,
            crate::sampler::WARP_SEG_LOOP_MODE_DEFAULT as f32,
        ),
        warp_seg_envelope: resolved_slot_node_param_value(
            slot,
            step_idx,
            crate::sampler::PARAM_WARP_SEG_ENVELOPE as u32,
            crate::sampler::WARP_SEG_ENVELOPE_DEFAULT,
        ),
    }
}

/// Seed every graph-mode runtime from a step-sequencer trigger (spec §4): each node
/// whose resolved `seed-from` includes the event's track receives a fire carrying the
/// step's note/velocity, after that node's delay. Mirrors the `neural` seed sites.
fn seed_graph_runtimes(
    graphs: &mut [crate::graph::GraphRuntime],
    event: &StepEvent,
    seed_beats: f64,
    samples_per_quarter: f64,
) {
    if graphs.is_empty() {
        return;
    }
    let duration_beats = if samples_per_quarter.is_finite() && samples_per_quarter > 0.0 {
        let value =
            event.resolved.duration as f64 * event.samples_per_step as f64 / samples_per_quarter;
        if value.is_finite() {
            value.max(0.0) as f32
        } else {
            0.0
        }
    } else {
        0.0
    };
    let payload = crate::graph::GraphPayload {
        note: event.resolved.transpose,
        velocity: event.resolved.velocity,
        duration_beats,
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

fn publish_graph_visualizations(
    state: &SequencerState,
    graphs: &[crate::graph::GraphRuntime],
    current_beat: f64,
) {
    state.set_graph_visualizations(
        graphs
            .iter()
            .map(|graph| graph.visualization_snapshot_at(current_beat))
            .collect(),
    );
}

fn same_coincident_note(
    existing_sample_time: u64,
    existing_track: Option<usize>,
    existing_transpose: f32,
    incoming_sample_time: u64,
    incoming_track: Option<usize>,
    incoming_transpose: f32,
) -> bool {
    existing_sample_time == incoming_sample_time
        && existing_track == incoming_track
        && existing_transpose == incoming_transpose
}

fn record_track_output_event(
    events: &mut Vec<TrackOutputEvent>,
    track: usize,
    sample_time: u64,
    beat: f64,
    resolved: ResolvedStep,
) {
    events.push(TrackOutputEvent {
        track,
        sample_time,
        beat,
        transpose: resolved.transpose,
        velocity: resolved.velocity,
    });
}

fn neural_outputs_are_same_accent(existing: &NeuralOutput, incoming: &NeuralOutput) -> bool {
    same_coincident_note(
        existing.sample_time,
        Some(existing.event.track),
        existing.event.resolved.transpose,
        incoming.sample_time,
        Some(incoming.event.track),
        incoming.event.resolved.transpose,
    )
}

fn merge_neural_output_accents(neural_events: Vec<NeuralOutput>) -> Vec<NeuralOutput> {
    let mut merged: Vec<NeuralOutput> = Vec::with_capacity(neural_events.len());
    for output in neural_events {
        if output.emit_trigger {
            if let Some(existing) = merged.iter_mut().find(|existing| {
                existing.emit_trigger && neural_outputs_are_same_accent(existing, &output)
            }) {
                existing.event.resolved.velocity =
                    (existing.event.resolved.velocity + output.event.resolved.velocity).min(1.0);
                continue;
            }
        }
        merged.push(output);
    }
    merged
}

fn generator_emissions_are_same_accent(
    existing: &crate::generator::GeneratorEmission,
    incoming: &crate::generator::GeneratorEmission,
) -> bool {
    same_coincident_note(
        existing.sample_time,
        existing.event.track,
        existing.event.resolved.transpose,
        incoming.sample_time,
        incoming.event.track,
        incoming.event.resolved.transpose,
    )
}

fn merge_generator_emission_accents(
    generator_emissions: Vec<crate::generator::GeneratorEmission>,
) -> Vec<crate::generator::GeneratorEmission> {
    let mut merged: Vec<crate::generator::GeneratorEmission> =
        Vec::with_capacity(generator_emissions.len());
    for emission in generator_emissions {
        if let Some(existing) = merged
            .iter_mut()
            .find(|existing| generator_emissions_are_same_accent(existing, &emission))
        {
            existing.event.resolved.velocity =
                (existing.event.resolved.velocity + emission.event.resolved.velocity).min(1.0);
            continue;
        }
        merged.push(emission);
    }
    merged
}

fn graph_emissions_are_same_accent(
    existing: &crate::graph::GraphEmission,
    incoming: &crate::graph::GraphEmission,
) -> bool {
    same_coincident_note(
        existing.sample_time,
        existing.event.track,
        existing.event.resolved.transpose,
        incoming.sample_time,
        incoming.event.track,
        incoming.event.resolved.transpose,
    )
}

fn merge_graph_emission_accents(
    graph_emissions: Vec<crate::graph::GraphEmission>,
) -> Vec<crate::graph::GraphEmission> {
    let mut merged: Vec<crate::graph::GraphEmission> = Vec::with_capacity(graph_emissions.len());
    for emission in graph_emissions {
        if let Some(existing) = merged
            .iter_mut()
            .find(|existing| graph_emissions_are_same_accent(existing, &emission))
        {
            existing.event.resolved.velocity =
                (existing.event.resolved.velocity + emission.event.resolved.velocity).min(1.0);
            continue;
        }
        merged.push(emission);
    }
    merged
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
    track_output_events: &mut Vec<TrackOutputEvent>,
    pattern_epoch: u64,
    sample_time: u64,
    event_beat: f64,
    samples_per_quarter: f32,
    global_transpose: f32,
    track_idx: usize,
    step_idx: usize,
    samples_per_step: f32,
    resolved: ResolvedStep,
    chord: ScheduledChordData,
    effect_params: Vec<ScheduledEffectParam>,
    instrument_params: ScheduledInstrumentParams,
    instrument_tensor_params: ScheduledInstrumentTensorParams,
    mut sampler_params: ScheduledSamplerParams,
) -> bool {
    let (resolved, chord) = apply_fit_to_scale_to_trigger(snapshot, track_idx, resolved, chord);
    let resolved =
        apply_global_transpose_to_resolved(snapshot, track_idx, global_transpose, resolved);
    apply_sampler_instrument_param_overrides(
        snapshot,
        track_idx,
        &mut sampler_params,
        &instrument_params,
    );
    process_trace(snapshot, || {
        format!(
            "enqueue kind=step track={} step={} inst_params={} sampler.attack={} sampler.release={} sampler.speed={}",
            track_idx + 1,
            step_idx,
            instrument_params.len(),
            sampler_params.attack_ms,
            sampler_params.release_ms,
            sampler_params.playback_speed
        )
    });
    let instrument_fingerprint = instrument_sound_fingerprint(
        snapshot,
        track_idx,
        &instrument_params,
        &instrument_tensor_params,
    );
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
                            instrument_tensor_params: instrument_tensor_params.clone(),
                            sampler_params,
                            instrument_fingerprint,
                        },
                    })
                    .is_err()
                {
                    ok = false;
                    break;
                }
                let note_beat = event_beat
                    + (note_sample_time.saturating_sub(sample_time) as f64)
                        / samples_per_quarter.max(1.0) as f64;
                record_track_output_event(
                    track_output_events,
                    track_idx,
                    note_sample_time,
                    note_beat,
                    resolved,
                );
            }
            return ok;
        }
    }
    let enqueued = queue
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
                instrument_tensor_params,
                sampler_params,
                instrument_fingerprint,
            },
        })
        .is_ok();
    if enqueued {
        record_track_output_event(
            track_output_events,
            track_idx,
            sample_time,
            event_beat,
            resolved,
        );
    }
    enqueued
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
    instrument_tensor_params: ScheduledInstrumentTensorParams,
) -> StepEvent {
    let instrument_fingerprint = instrument_sound_fingerprint(
        snapshot,
        track_idx,
        &instrument_params,
        &instrument_tensor_params,
    );
    StepEvent {
        track: track_idx,
        samples_per_step,
        resolved,
        chord,
        effect_params,
        instrument_params,
        instrument_tensor_params,
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
    track_output_events: &mut Vec<TrackOutputEvent>,
    pattern_epoch: u64,
    sample_time: u64,
    event_beat: f64,
    samples_per_quarter: f32,
    global_transpose: f32,
    mut event: StepEvent,
) -> bool {
    match event.source.clone() {
        EventSource::Step { step, .. } => enqueue_resolved_trigger(
            queue,
            snapshot,
            track_output_events,
            pattern_epoch,
            sample_time,
            event_beat,
            samples_per_quarter,
            global_transpose,
            event.track,
            step,
            event.samples_per_step,
            event.resolved,
            event.chord,
            event.effect_params,
            event.instrument_params,
            event.instrument_tensor_params,
            event.sampler_params,
        ),
        EventSource::Network { seed, neuron, .. } => {
            normalize_network_event_destination(snapshot, neuron, seed, &mut event);
            let instrument_fingerprint = instrument_sound_fingerprint(
                snapshot,
                event.track,
                &event.instrument_params,
                &event.instrument_tensor_params,
            );
            enqueue_network_trigger(
                queue,
                snapshot,
                track_output_events,
                pattern_epoch,
                sample_time,
                event_beat,
                samples_per_quarter,
                global_transpose,
                event.track,
                neuron,
                seed,
                event.samples_per_step,
                event.resolved,
                event.chord,
                event.effect_params,
                event.instrument_params,
                event.instrument_tensor_params,
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
    track_output_events: &mut Vec<TrackOutputEvent>,
    runtime: Option<&mut lisp_host::ScratchControlRuntime>,
    quantizer_state: Option<&mut MidiFxQuantizerState>,
    pattern_epoch: u64,
    sample_time: u64,
    event_beat: f64,
    samples_per_quarter: f32,
    global_transpose: f32,
    arp_phase_beats: f32,
    mut event: StepEvent,
    midi_fx_params: Vec<ProcessMidiFxParamOverride>,
    debug_accum: bool,
) -> bool {
    if event.track >= snapshot.tracks.len() {
        if debug_routing_enabled() {
            eprintln!(
                "[routing] skip enqueue_step_event_with_midi_fx reason=track-out-of-range track={} tracks={} source={}",
                event.track,
                snapshot.tracks.len(),
                event_source_label(&event.source)
            );
        }
        return false;
    }
    let run_midi_fx = snapshot.tracks[event.track].params.midi_fx_position
        == MidiFxPosition::PostAccumulator
        && !snapshot.tracks[event.track].params.midi_fx_chain.is_empty();
    let Some(runtime) = runtime else {
        if debug_routing_enabled() {
            eprintln!(
                "[routing] skip midi-fx reason=no-scratch-runtime track={} sample={} source={} chain={:?}",
                event.track,
                sample_time,
                event_source_label(&event.source),
                snapshot.tracks[event.track].params.midi_fx_chain
            );
        }
        return enqueue_step_event(
            queue,
            snapshot,
            track_output_events,
            pattern_epoch,
            sample_time,
            event_beat,
            samples_per_quarter,
            global_transpose,
            event,
        );
    };
    if !run_midi_fx {
        if debug_routing_enabled() {
            eprintln!(
                "[routing] skip midi-fx reason=no-post-accumulator-chain track={} sample={} source={} position={:?} chain={:?}",
                event.track,
                sample_time,
                event_source_label(&event.source),
                snapshot.tracks[event.track].params.midi_fx_position,
                snapshot.tracks[event.track].params.midi_fx_chain
            );
        }
        return enqueue_step_event(
            queue,
            snapshot,
            track_output_events,
            pattern_epoch,
            sample_time,
            event_beat,
            samples_per_quarter,
            global_transpose,
            event,
        );
    }
    if let EventSource::Network { seed, neuron, .. } = event.source.clone() {
        normalize_network_event_destination(snapshot, neuron, seed, &mut event);
    }
    if debug_routing_enabled() {
        eprintln!(
            "[routing] enter midi-fx track={} step={} sample={} source={} chain={:?} transpose={} vel={} inst_params={} sampler_speed={}",
            event.track,
            midi_fx_step_for_step_event(snapshot, &event),
            sample_time,
            event_source_label(&event.source),
            snapshot.tracks[event.track].params.midi_fx_chain,
            event.resolved.transpose,
            event.resolved.velocity,
            event.instrument_params.len(),
            event.sampler_params.playback_speed
        );
    }

    let step = midi_fx_step_for_step_event(snapshot, &event);
    let step_beats = if samples_per_quarter > 0.0 {
        event.samples_per_step / samples_per_quarter
    } else {
        0.0
    };
    let event = midi_fx_event_from_step_event(
        snapshot,
        event,
        step,
        step_beats,
        0.0,
        arp_phase_beats,
        midi_fx_params,
    );
    let events = run_midi_fx_chain_for_track(
        runtime,
        snapshot,
        event.track,
        vec![event],
        quantizer_state,
        0,
        debug_accum,
    );
    if debug_routing_enabled() {
        eprintln!(
            "[routing] midi-fx result count={} base_sample={} samples_per_quarter={}",
            events.len(),
            sample_time,
            samples_per_quarter
        );
    }
    enqueue_midi_fx_events(
        queue,
        snapshot,
        track_output_events,
        pattern_epoch,
        sample_time,
        event_beat,
        samples_per_quarter,
        global_transpose,
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
                    instrument_tensor_params: ScheduledInstrumentTensorParams::new(),
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
    track_output_events: &mut Vec<TrackOutputEvent>,
    runtime: Option<&mut lisp_host::ScratchControlRuntime>,
    mut quantizer_state: Option<&mut MidiFxQuantizerState>,
    pattern_epoch: u64,
    sample_time: u64,
    samples_per_quarter: f32,
    global_transpose: f32,
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
                    track_output_events,
                    runtime,
                    quantizer_state.as_deref_mut(),
                    pattern_epoch,
                    sample_time,
                    arp_phase_beats as f64,
                    samples_per_quarter,
                    global_transpose,
                    arp_phase_beats,
                    event,
                    Vec::new(),
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
        track_output_events,
        runtime,
        quantizer_state,
        pattern_epoch,
        sample_time,
        arp_phase_beats as f64,
        samples_per_quarter,
        global_transpose,
        arp_phase_beats,
        event,
        Vec::new(),
        debug_accum,
    )
}

#[derive(Clone, Copy, Debug)]
enum EmittedNetworkEventSource {
    Generator {
        index: usize,
    },
    Process {
        runtime_id: u64,
    },
    Graph {
        graph_index: usize,
        node_index: usize,
    },
}

impl EmittedNetworkEventSource {
    fn event_source_index(self) -> usize {
        match self {
            Self::Generator { index } => index,
            Self::Process { runtime_id } => runtime_id as usize,
            Self::Graph { node_index, .. } => node_index,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Generator { .. } => "generator",
            Self::Process { .. } => "process",
            Self::Graph { .. } => "graph",
        }
    }

    fn owner_index(self) -> usize {
        match self {
            Self::Generator { index } => index,
            Self::Process { runtime_id } => runtime_id as usize,
            Self::Graph {
                graph_index: index, ..
            } => index,
        }
    }

    fn resolve_track(self, emitted_track: Option<usize>) -> Option<usize> {
        match self {
            Self::Generator { .. } | Self::Process { .. } => emitted_track.or(Some(0)),
            Self::Graph { .. } => emitted_track,
        }
    }
}

fn enqueue_emitted_network_event_with_midi_fx<const QUEUE_CAP: usize>(
    queue: &ScheduledEventQueue<QUEUE_CAP>,
    snapshot: &SequencerSnapshot,
    track_output_events: &mut Vec<TrackOutputEvent>,
    runtime: Option<&mut lisp_host::ScratchControlRuntime>,
    quantizer_state: Option<&mut MidiFxQuantizerState>,
    pattern_epoch: u64,
    sample_time: u64,
    samples_per_quarter: f32,
    arp_phase_beats: f32,
    global_transpose: f32,
    source: EmittedNetworkEventSource,
    emitted: lisp_host::EmittedAccumulatorEvent,
    debug_accum: bool,
) -> bool {
    let Some(track_idx) = source.resolve_track(emitted.track) else {
        if debug_routing_enabled() {
            eprintln!(
                "[routing] skip emitted-network reason=route-off source={} owner_index={} source_index={} sample={}",
                source.label(),
                source.owner_index(),
                source.event_source_index(),
                sample_time
            );
        }
        return true;
    };
    if track_idx >= snapshot.tracks.len() {
        if debug_routing_enabled() {
            eprintln!(
                "[routing] skip emitted-network reason=track-out-of-range source={} owner_index={} source_index={} track={:?} tracks={} sample={}",
                source.label(),
                source.owner_index(),
                source.event_source_index(),
                emitted.track,
                snapshot.tracks.len(),
                sample_time
            );
        }
        return true;
    }
    if debug_routing_enabled() {
        eprintln!(
            "[routing] emitted-network source={} owner_index={} source_index={} track={} sample={} event_beats={} chain={:?} transpose={} vel={} emitted_fx_params={} emitted_inst_params={}",
            source.label(),
            source.owner_index(),
            source.event_source_index(),
            track_idx,
            sample_time,
            arp_phase_beats,
            snapshot.tracks[track_idx].params.midi_fx_chain,
            emitted.resolved.transpose,
            emitted.resolved.velocity,
            emitted.effect_params.len(),
            emitted.instrument_params.len()
        );
    }

    let chord = chord_data_from_parts(
        &emitted.chord,
        &emitted.chord_durations,
        &[],
        emitted.resolved.duration,
        emitted.chord_step_transpose,
    );
    let mut event = StepEvent {
        track: track_idx,
        samples_per_step: samples_per_quarter,
        resolved: emitted.resolved,
        chord,
        effect_params: resolve_effect_defaults(snapshot, track_idx),
        instrument_params: resolve_instrument_defaults(snapshot, track_idx),
        instrument_tensor_params: resolve_instrument_tensor_defaults(snapshot, track_idx),
        sampler_params: resolve_sampler_defaults(snapshot, track_idx),
        source: EventSource::Network {
            seed: None,
            neuron: source.event_source_index(),
            instrument_fingerprint: 0,
        },
    };
    upsert_effect_params(&mut event.effect_params, emitted.effect_params);
    upsert_instrument_params(
        &mut event.instrument_params,
        scheduled_instrument_params_from_vec(emitted.instrument_params),
    );

    enqueue_step_event_with_midi_fx(
        queue,
        snapshot,
        track_output_events,
        runtime,
        quantizer_state,
        pattern_epoch,
        sample_time,
        arp_phase_beats as f64,
        samples_per_quarter,
        global_transpose,
        arp_phase_beats,
        event,
        Vec::new(),
        debug_accum,
    )
}

fn enqueue_due_process_emissions<const QUEUE_CAP: usize>(
    queue: &ScheduledEventQueue<QUEUE_CAP>,
    snapshot: &SequencerSnapshot,
    track_output_events: &mut Vec<TrackOutputEvent>,
    scratch_runtime: &mut Option<lisp_host::ScratchControlRuntime>,
    quantizer_state: &mut MidiFxQuantizerState,
    process_runtime: &mut crate::process::ProcessRuntime,
    pattern_epoch: u64,
    chunk_start_beats: f64,
    chunk_start_sample: u64,
    up_to_beat: f64,
    samples_per_quarter: f64,
    debug_accum: bool,
) -> bool {
    for item in process_runtime.take_due_events(up_to_beat) {
        let sample_time = chunk_start_sample.saturating_add(
            ((item.beat - chunk_start_beats).max(0.0) * samples_per_quarter).round() as u64,
        );
        match item.event {
            crate::process::ProcessScheduledEvent::Emission(event) => {
                if debug_routing_enabled() {
                    eprintln!(
                        "[routing] process-emission process={} track={:?} sample={} beat={:.6} transpose={} vel={}",
                        item.process_runtime_id,
                        event.track,
                        sample_time,
                        item.beat,
                        event.resolved.transpose,
                        event.resolved.velocity
                    );
                }
                if !enqueue_emitted_network_event_with_midi_fx(
                    queue,
                    snapshot,
                    track_output_events,
                    scratch_runtime.as_mut(),
                    Some(&mut *quantizer_state),
                    pattern_epoch,
                    sample_time,
                    samples_per_quarter as f32,
                    item.beat as f32,
                    process_runtime.global_transpose(),
                    EmittedNetworkEventSource::Process {
                        runtime_id: item.process_runtime_id,
                    },
                    event,
                    debug_accum,
                ) {
                    return false;
                }
            }
            crate::process::ProcessScheduledEvent::Step(spawned) => {
                if debug_routing_enabled() {
                    eprintln!(
                        "[routing] process-step process={} track={} sample={} beat={:.6} transpose={} vel={} midi_fx_overrides={}",
                        item.process_runtime_id,
                        spawned.event.track,
                        sample_time,
                        item.beat,
                        spawned.event.resolved.transpose,
                        spawned.event.resolved.velocity,
                        spawned.midi_fx_params.len()
                    );
                }
                if !enqueue_step_event_with_midi_fx(
                    queue,
                    snapshot,
                    track_output_events,
                    scratch_runtime.as_mut(),
                    Some(&mut *quantizer_state),
                    pattern_epoch,
                    sample_time,
                    item.beat,
                    samples_per_quarter as f32,
                    process_runtime.global_transpose(),
                    item.beat as f32,
                    spawned.event,
                    spawned.midi_fx_params,
                    debug_accum,
                ) {
                    return false;
                }
            }
        }
    }
    true
}

#[derive(Clone, Debug)]
struct ProcessTargetOverlay {
    effect_params: Vec<ScheduledEffectParam>,
    instrument_params: ScheduledInstrumentParams,
    midi_fx_params: Vec<ProcessMidiFxParamOverride>,
}

impl Default for ProcessTargetOverlay {
    fn default() -> Self {
        Self {
            effect_params: Vec::new(),
            instrument_params: ScheduledInstrumentParams::new(),
            midi_fx_params: Vec::new(),
        }
    }
}

fn process_trace(snapshot: &SequencerSnapshot, message: impl FnOnce() -> String) {
    if snapshot.process_trace {
        eprintln!("[process-trace] {}", message());
    }
}

fn process_target_op_label(op: crate::process::ProcessTargetOp) -> &'static str {
    match op {
        crate::process::ProcessTargetOp::Set => "set",
        crate::process::ProcessTargetOp::Add => "add",
    }
}

type StepProcessInletWrites =
    BTreeMap<usize, BTreeMap<String, Vec<crate::process::ProcessInletWrite>>>;

#[derive(Clone, Debug)]
struct DeferredProcessInletWrite {
    track: usize,
    instance_id: crate::process::ProcessInstanceId,
    inlet: String,
    write: crate::process::ProcessInletWrite,
}

struct ProcessInletWriteContext<'a> {
    chain: &'a crate::process::TrackProcessChain,
    current_slot_index: Option<usize>,
    current_fire_writes: &'a mut StepProcessInletWrites,
    deferred_writes: &'a mut Vec<DeferredProcessInletWrite>,
}

fn process_target_label(target: &crate::process::ParamTarget) -> String {
    match target {
        crate::process::ParamTarget::StepParam { param } => format!("step-param:{param}"),
        crate::process::ParamTarget::InstrumentParam { param, .. } => {
            format!("instrument:{param}")
        }
        crate::process::ParamTarget::EffectParam {
            slot,
            effect,
            param,
            ..
        } => format!("effect{}:{effect}:{param}", slot + 1),
        crate::process::ParamTarget::MidiFxParam { slot, fx, param } => {
            format!("midi-fx{}:{fx}:{param}", slot + 1)
        }
        crate::process::ParamTarget::ProcessInlet {
            process,
            inlet,
            instance_id,
        } => instance_id
            .map(|id| format!("process-inlet:{process}#{}:{inlet}", id.0))
            .unwrap_or_else(|| format!("process-inlet:{process}:{inlet}")),
        crate::process::ParamTarget::RackSlotParam { slot, param } => {
            format!("rack{}:{param}", slot + 1)
        }
        crate::process::ParamTarget::RackSlotInstrumentParam { slot, param, .. } => {
            format!("rack{}:instrument:{param}", slot + 1)
        }
    }
}

fn process_step_param_from_name(name: &str) -> Option<StepParam> {
    let normalized = name
        .trim_start_matches(':')
        .replace('_', "-")
        .to_ascii_lowercase();
    [
        StepParam::Duration,
        StepParam::Velocity,
        StepParam::Speed,
        StepParam::AuxA,
        StepParam::AuxB,
        StepParam::Transpose,
        StepParam::Pan,
        StepParam::Chop,
    ]
    .into_iter()
    .find(|param| {
        param.short_label().eq_ignore_ascii_case(&normalized)
            || param
                .label()
                .replace(' ', "-")
                .eq_ignore_ascii_case(&normalized)
            || match param {
                StepParam::Duration => normalized == "duration",
                StepParam::Velocity => normalized == "velocity",
                StepParam::Speed => normalized == "speed",
                StepParam::AuxA => normalized == "aux-a",
                StepParam::AuxB => normalized == "aux-b",
                StepParam::Transpose => normalized == "transpose",
                StepParam::Pan => normalized == "pan",
                StepParam::Chop => normalized == "chop",
                StepParam::Sync | StepParam::Delay => false,
            }
    })
}

fn resolved_step_param(resolved: &ResolvedStep, param: StepParam) -> f32 {
    match param {
        StepParam::Duration => resolved.duration,
        StepParam::Velocity => resolved.velocity,
        StepParam::Speed => resolved.speed,
        StepParam::AuxA => resolved.aux_a,
        StepParam::AuxB => resolved.aux_b,
        StepParam::Transpose => resolved.transpose,
        StepParam::Pan => resolved.pan,
        StepParam::Chop => resolved.chop,
        StepParam::Sync => 0.0,
        StepParam::Delay => 0.0,
    }
}

fn set_resolved_step_param(resolved: &mut ResolvedStep, param: StepParam, value: f32) {
    let value = value.clamp(param.min(), param.max());
    match param {
        StepParam::Duration => resolved.duration = value,
        StepParam::Velocity => resolved.velocity = value,
        StepParam::Speed => resolved.speed = value,
        StepParam::AuxA => resolved.aux_a = value,
        StepParam::AuxB => resolved.aux_b = value,
        StepParam::Transpose => resolved.transpose = value,
        StepParam::Pan => resolved.pan = value,
        StepParam::Chop => resolved.chop = value,
        StepParam::Sync | StepParam::Delay => unreachable!("unsupported process step param"),
    }
}

fn process_apply_step_param_write(
    resolved: &mut ResolvedStep,
    param_name: &str,
    op: crate::process::ProcessTargetOp,
    value: f32,
) -> Option<(StepParam, f32)> {
    let param = process_step_param_from_name(param_name)?;
    let next = match op {
        crate::process::ProcessTargetOp::Set => value,
        crate::process::ProcessTargetOp::Add => resolved_step_param(resolved, param) + value,
    };
    set_resolved_step_param(resolved, param, next);
    Some((param, next))
}

fn process_param_index_by_tag_or_name(
    descriptor: &EffectDescriptor,
    tag_or_name: &str,
) -> Option<usize> {
    descriptor
        .params
        .iter()
        .position(|param| param.has_tag_or_name(tag_or_name))
}

fn process_slot_param_identity(
    slot: &crate::effects::EffectSlotSnapshot,
    param_idx: usize,
) -> Option<ParamNodeId> {
    let raw_idx = slot
        .param_node_indices
        .get(param_idx)
        .copied()
        .unwrap_or(param_idx as u32);
    slot_param_identity(slot.node_id, slot.modulator_node_id, raw_idx)
}

fn process_scheduled_instrument_param(
    slot: &crate::effects::EffectSlotSnapshot,
    param_idx: usize,
    value: f32,
) -> Option<ScheduledInstrumentParam> {
    if param_idx >= slot.num_params as usize || !value.is_finite() {
        return None;
    }
    let raw_idx = slot
        .param_node_indices
        .get(param_idx)
        .copied()
        .unwrap_or(param_idx as u32);
    if raw_idx == u32::MAX {
        return None;
    }
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
    Some(ScheduledInstrumentParam {
        target,
        idx,
        span,
        value,
    })
}

fn process_scheduled_effect_param(
    slot: &crate::effects::EffectSlotSnapshot,
    param_idx: usize,
    value: f32,
) -> Option<ScheduledEffectParam> {
    let identity = process_slot_param_identity(slot, param_idx)?;
    value.is_finite().then_some(ScheduledEffectParam {
        logical_id: identity.logical_id,
        idx: identity.node_param_idx as u64,
        value,
    })
}

fn process_device_write_value(
    descriptor: &crate::effects::ParamDescriptor,
    current_stored: f32,
    op: crate::process::ProcessTargetOp,
    value: f32,
) -> f32 {
    match op {
        crate::process::ProcessTargetOp::Set => descriptor.denormalize(value),
        crate::process::ProcessTargetOp::Add => {
            descriptor.denormalize((descriptor.normalize(current_stored) + value).clamp(0.0, 1.0))
        }
    }
}

fn process_instrument_overlay_value(
    overlay: &ProcessTargetOverlay,
    param: &ScheduledInstrumentParam,
    fallback: f32,
) -> f32 {
    overlay
        .instrument_params
        .iter()
        .find(|existing| existing.target == param.target && existing.idx == param.idx)
        .map(|existing| existing.value)
        .unwrap_or(fallback)
}

fn process_effect_overlay_value(
    overlay: &ProcessTargetOverlay,
    param: &ScheduledEffectParam,
    fallback: f32,
) -> f32 {
    overlay
        .effect_params
        .iter()
        .find(|existing| existing.logical_id == param.logical_id && existing.idx == param.idx)
        .map(|existing| existing.value)
        .unwrap_or(fallback)
}

fn process_midi_fx_overlay_value(
    overlay: &ProcessTargetOverlay,
    slot: usize,
    fx: &str,
    param_idx: usize,
    fallback: f32,
) -> f32 {
    overlay
        .midi_fx_params
        .iter()
        .rev()
        .find(|existing| {
            existing.slot == slot
                && existing.param_idx == param_idx
                && existing.fx.eq_ignore_ascii_case(fx)
        })
        .map(|existing| existing.value)
        .unwrap_or(fallback)
}

fn process_apply_instrument_write(
    snapshot: &SequencerSnapshot,
    track: usize,
    step: usize,
    param_idx: usize,
    op: crate::process::ProcessTargetOp,
    value: f32,
    overlay: &mut ProcessTargetOverlay,
) -> Option<ScheduledInstrumentParam> {
    let Some(track_snapshot) = snapshot.tracks.get(track) else {
        return None;
    };
    let Some(param_desc) = track_snapshot.instrument_descriptor.params.get(param_idx) else {
        return None;
    };
    let current = resolved_slot_param_value(&track_snapshot.instrument_slot, step, param_idx, 0.0);
    let Some(mut scheduled) =
        process_scheduled_instrument_param(&track_snapshot.instrument_slot, param_idx, current)
    else {
        return None;
    };
    let current = process_instrument_overlay_value(overlay, &scheduled, current);
    scheduled.value = process_device_write_value(param_desc, current, op, value);
    upsert_instrument_params(&mut overlay.instrument_params, [scheduled.clone()]);
    Some(scheduled)
}

fn process_apply_effect_write(
    snapshot: &SequencerSnapshot,
    track: usize,
    step: usize,
    slot_idx: usize,
    param_idx: usize,
    op: crate::process::ProcessTargetOp,
    value: f32,
    overlay: &mut ProcessTargetOverlay,
) -> Option<ScheduledEffectParam> {
    let Some(track_snapshot) = snapshot.tracks.get(track) else {
        return None;
    };
    let Some(slot) = track_snapshot.effect_slots.get(slot_idx) else {
        return None;
    };
    let Some(param_desc) = track_snapshot
        .effect_descriptors
        .get(slot_idx)
        .and_then(|desc| desc.params.get(param_idx))
    else {
        return None;
    };
    let current = resolved_slot_param_value(slot, step, param_idx, 0.0);
    let Some(mut scheduled) = process_scheduled_effect_param(slot, param_idx, current) else {
        return None;
    };
    let current = process_effect_overlay_value(overlay, &scheduled, current);
    scheduled.value = process_device_write_value(param_desc, current, op, value);
    upsert_effect_params(&mut overlay.effect_params, [scheduled.clone()]);
    Some(scheduled)
}

fn process_apply_midi_fx_write(
    snapshot: &SequencerSnapshot,
    midi_fx_descriptors: &[EffectDescriptor],
    track: usize,
    step: usize,
    slot_idx: usize,
    fx: &str,
    param_idx: usize,
    op: crate::process::ProcessTargetOp,
    value: f32,
    overlay: &mut ProcessTargetOverlay,
) -> Option<ProcessMidiFxParamOverride> {
    let Some(track_snapshot) = snapshot.tracks.get(track) else {
        return None;
    };
    let Some(chain_fx) = track_snapshot.params.midi_fx_chain.get(slot_idx) else {
        return None;
    };
    if !chain_fx.eq_ignore_ascii_case(fx) {
        return None;
    }
    let Some(desc) = midi_fx_descriptors
        .iter()
        .find(|desc| desc.name.eq_ignore_ascii_case(fx))
    else {
        return None;
    };
    let Some(param_desc) = desc.params.get(param_idx) else {
        return None;
    };
    let Some(slot) = track_snapshot.midi_fx_slots.get(slot_idx) else {
        return None;
    };
    let current = resolved_slot_param_value(slot, step, param_idx, 0.0);
    let current = process_midi_fx_overlay_value(overlay, slot_idx, fx, param_idx, current);
    let value = process_device_write_value(param_desc, current, op, value);
    let next = ProcessMidiFxParamOverride {
        slot: slot_idx,
        fx: chain_fx.clone(),
        param: param_desc.name.clone(),
        param_idx,
        value,
    };
    if let Some(existing) = overlay.midi_fx_params.iter_mut().find(|existing| {
        existing.slot == slot_idx
            && existing.param_idx == param_idx
            && existing.fx.eq_ignore_ascii_case(fx)
    }) {
        existing.value = value;
        existing.param = param_desc.name.clone();
    } else {
        overlay.midi_fx_params.push(next.clone());
    }
    Some(next)
}

fn process_resolve_hint_to_target(
    snapshot: &SequencerSnapshot,
    midi_fx_descriptors: &[EffectDescriptor],
    track: usize,
    hint: &crate::process::ProcessTargetHint,
) -> Option<crate::process::ParamTarget> {
    match hint {
        crate::process::ProcessTargetHint::StepParam { param } => {
            process_step_param_from_name(param)?;
            Some(crate::process::ParamTarget::StepParam {
                param: param.clone(),
            })
        }
        crate::process::ProcessTargetHint::InstrumentParam { param } => {
            let track_snapshot = snapshot.tracks.get(track)?;
            let param_idx =
                process_param_index_by_tag_or_name(&track_snapshot.instrument_descriptor, param)?;
            Some(crate::process::ParamTarget::InstrumentParam {
                param: track_snapshot.instrument_descriptor.params[param_idx]
                    .name
                    .clone(),
                param_id: process_slot_param_identity(&track_snapshot.instrument_slot, param_idx),
            })
        }
        crate::process::ProcessTargetHint::EffectParam { effect, param } => {
            let track_snapshot = snapshot.tracks.get(track)?;
            for (slot_idx, desc) in track_snapshot.effect_descriptors.iter().enumerate() {
                if !desc.name.eq_ignore_ascii_case(effect) {
                    continue;
                }
                let param_idx = process_param_index_by_tag_or_name(desc, param)?;
                let slot = track_snapshot.effect_slots.get(slot_idx)?;
                return Some(crate::process::ParamTarget::EffectParam {
                    slot: slot_idx,
                    effect: desc.name.clone(),
                    param: desc.params[param_idx].name.clone(),
                    param_id: process_slot_param_identity(slot, param_idx),
                });
            }
            None
        }
        crate::process::ProcessTargetHint::MidiFxParam { fx, param } => {
            let track_snapshot = snapshot.tracks.get(track)?;
            for (slot_idx, chain_fx) in track_snapshot.params.midi_fx_chain.iter().enumerate() {
                if !chain_fx.eq_ignore_ascii_case(fx) {
                    continue;
                }
                let desc = midi_fx_descriptors
                    .iter()
                    .find(|desc| desc.name.eq_ignore_ascii_case(chain_fx))?;
                let param_idx = process_param_index_by_tag_or_name(desc, param)?;
                return Some(crate::process::ParamTarget::MidiFxParam {
                    slot: slot_idx,
                    fx: chain_fx.clone(),
                    param: desc.params[param_idx].name.clone(),
                });
            }
            None
        }
        crate::process::ProcessTargetHint::ParamTag { tag } => {
            let track_snapshot = snapshot.tracks.get(track)?;
            if let Some(param_idx) =
                process_param_index_by_tag_or_name(&track_snapshot.instrument_descriptor, tag)
            {
                return Some(crate::process::ParamTarget::InstrumentParam {
                    param: track_snapshot.instrument_descriptor.params[param_idx]
                        .name
                        .clone(),
                    param_id: process_slot_param_identity(
                        &track_snapshot.instrument_slot,
                        param_idx,
                    ),
                });
            }
            for (slot_idx, desc) in track_snapshot.effect_descriptors.iter().enumerate() {
                if let Some(param_idx) = process_param_index_by_tag_or_name(desc, tag) {
                    let slot = track_snapshot.effect_slots.get(slot_idx)?;
                    return Some(crate::process::ParamTarget::EffectParam {
                        slot: slot_idx,
                        effect: desc.name.clone(),
                        param: desc.params[param_idx].name.clone(),
                        param_id: process_slot_param_identity(slot, param_idx),
                    });
                }
            }
            for (slot_idx, chain_fx) in track_snapshot.params.midi_fx_chain.iter().enumerate() {
                let Some(desc) = midi_fx_descriptors
                    .iter()
                    .find(|desc| desc.name.eq_ignore_ascii_case(chain_fx))
                else {
                    continue;
                };
                if let Some(param_idx) = process_param_index_by_tag_or_name(desc, tag) {
                    return Some(crate::process::ParamTarget::MidiFxParam {
                        slot: slot_idx,
                        fx: chain_fx.clone(),
                        param: desc.params[param_idx].name.clone(),
                    });
                }
            }
            None
        }
    }
}

fn resolve_process_inlet_target(
    chain: &crate::process::TrackProcessChain,
    source_project_layer: Option<bool>,
    target: &crate::process::ParamTarget,
) -> Option<(usize, crate::process::ProcessInstanceId, String)> {
    let crate::process::ParamTarget::ProcessInlet {
        process,
        inlet,
        instance_id,
    } = target
    else {
        return None;
    };
    // Wiring stays within a layer: a project slot only drives project slots,
    // a track slot only track slots (cross-layer traffic is channels' job).
    let same_layer = |slot: &crate::process::TrackProcessSlot| {
        source_project_layer.is_none_or(|layer| slot.project_layer == layer)
    };
    let slot_idx = match instance_id {
        Some(instance_id) => chain.slots.iter().position(|slot| {
            slot.instance_id == *instance_id && slot.class_name == *process && same_layer(slot)
        }),
        None => chain
            .slots
            .iter()
            .position(|slot| slot.class_name == *process && same_layer(slot)),
    }?;
    let slot = &chain.slots[slot_idx];
    Some((slot_idx, slot.instance_id, inlet.clone()))
}

fn process_apply_inlet_write(
    snapshot: &SequencerSnapshot,
    track: usize,
    step: usize,
    target: &crate::process::ParamTarget,
    write: &crate::process::ProcessTargetWrite,
    context: Option<&mut ProcessInletWriteContext<'_>>,
) {
    let Some(context) = context else {
        process_trace(snapshot, || {
            format!(
                "skip track={} step={} port={} target={} reason=process-inlet-context-missing",
                track + 1,
                step,
                write.port,
                process_target_label(target)
            )
        });
        return;
    };
    let source_project_layer = context
        .current_slot_index
        .and_then(|index| context.chain.slots.get(index))
        .map(|slot| slot.project_layer);
    let Some((target_slot_index, instance_id, inlet)) =
        resolve_process_inlet_target(context.chain, source_project_layer, target)
    else {
        process_trace(snapshot, || {
            format!(
                "skip track={} step={} port={} target={} reason=process-inlet-target-not-found",
                track + 1,
                step,
                write.port,
                process_target_label(target)
            )
        });
        return;
    };
    let inlet_write = crate::process::ProcessInletWrite {
        op: write.op,
        value: write.value,
    };
    if context
        .current_slot_index
        .is_some_and(|current_slot_index| target_slot_index > current_slot_index)
    {
        context
            .current_fire_writes
            .entry(target_slot_index)
            .or_default()
            .entry(inlet.clone())
            .or_default()
            .push(inlet_write);
        process_trace(snapshot, || {
            format!(
                "apply track={} step={} port={} target={} op={} value={} -> slot={} inlet={} timing=current-fire",
                track + 1,
                step,
                write.port,
                process_target_label(target),
                process_target_op_label(write.op),
                write.value,
                target_slot_index,
                inlet
            )
        });
    } else {
        context.deferred_writes.push(DeferredProcessInletWrite {
            track,
            instance_id,
            inlet: inlet.clone(),
            write: inlet_write,
        });
        process_trace(snapshot, || {
            format!(
                "defer track={} step={} port={} target={} op={} value={} -> instance={} inlet={} timing=next-fire",
                track + 1,
                step,
                write.port,
                process_target_label(target),
                process_target_op_label(write.op),
                write.value,
                instance_id.0,
                inlet
            )
        });
    }
}

fn process_apply_concrete_target_write(
    snapshot: &SequencerSnapshot,
    midi_fx_descriptors: &[EffectDescriptor],
    track: usize,
    step: usize,
    resolved: &mut ResolvedStep,
    overlay: &mut ProcessTargetOverlay,
    target: &crate::process::ParamTarget,
    write: &crate::process::ProcessTargetWrite,
) {
    match target {
        crate::process::ParamTarget::StepParam { param } => {
            match process_apply_step_param_write(resolved, param, write.op, write.value) {
                Some((step_param, applied)) => process_trace(snapshot, || {
                    format!(
                        "apply track={} step={} port={} target={} op={} value={} -> {:?}={}",
                        track + 1,
                        step,
                        write.port,
                        process_target_label(target),
                        process_target_op_label(write.op),
                        write.value,
                        step_param,
                        applied
                    )
                }),
                None => process_trace(snapshot, || {
                    format!(
                        "skip track={} step={} port={} target={} reason=unknown-step-param",
                        track + 1,
                        step,
                        write.port,
                        process_target_label(target)
                    )
                }),
            }
        }
        crate::process::ParamTarget::InstrumentParam { param, param_id } => {
            let Some(track_snapshot) = snapshot.tracks.get(track) else {
                process_trace(snapshot, || {
                    format!(
                        "skip track={} step={} port={} target={} reason=missing-track",
                        track + 1,
                        step,
                        write.port,
                        process_target_label(target)
                    )
                });
                return;
            };
            let Some(param_idx) =
                process_param_index_by_tag_or_name(&track_snapshot.instrument_descriptor, param)
            else {
                process_trace(snapshot, || {
                    format!(
                        "skip track={} step={} port={} target={} reason=instrument-param-not-found descriptor={}",
                        track + 1,
                        step,
                        write.port,
                        process_target_label(target),
                        track_snapshot.instrument_descriptor.name
                    )
                });
                return;
            };
            if let Some(expected) = param_id {
                let actual =
                    process_slot_param_identity(&track_snapshot.instrument_slot, param_idx);
                if actual != Some(*expected) {
                    process_trace(snapshot, || {
                        format!(
                            "skip track={} step={} port={} target={} reason=instrument-param-identity-mismatch expected={expected:?} actual={actual:?}",
                            track + 1,
                            step,
                            write.port,
                            process_target_label(target)
                        )
                    });
                    return;
                }
            }
            match process_apply_instrument_write(
                snapshot,
                track,
                step,
                param_idx,
                write.op,
                write.value,
                overlay,
            ) {
                Some(applied) => process_trace(snapshot, || {
                    format!(
                        "apply track={} step={} port={} target={} op={} normalized={} param-idx={} node={:?}:{} raw={}",
                        track + 1,
                        step,
                        write.port,
                        process_target_label(target),
                        process_target_op_label(write.op),
                        write.value,
                        param_idx,
                        applied.target,
                        applied.idx,
                        applied.value
                    )
                }),
                None => process_trace(snapshot, || {
                    format!(
                        "skip track={} step={} port={} target={} reason=instrument-param-not-schedulable param-idx={}",
                        track + 1,
                        step,
                        write.port,
                        process_target_label(target),
                        param_idx
                    )
                }),
            }
        }
        crate::process::ParamTarget::EffectParam {
            slot,
            effect,
            param,
            param_id,
        } => {
            let Some(track_snapshot) = snapshot.tracks.get(track) else {
                process_trace(snapshot, || {
                    format!(
                        "skip track={} step={} port={} target={} reason=missing-track",
                        track + 1,
                        step,
                        write.port,
                        process_target_label(target)
                    )
                });
                return;
            };
            let Some(desc) = track_snapshot.effect_descriptors.get(*slot) else {
                process_trace(snapshot, || {
                    format!(
                        "skip track={} step={} port={} target={} reason=missing-effect-slot slot={}",
                        track + 1,
                        step,
                        write.port,
                        process_target_label(target),
                        slot + 1
                    )
                });
                return;
            };
            if !desc.name.eq_ignore_ascii_case(effect) {
                process_trace(snapshot, || {
                    format!(
                        "skip track={} step={} port={} target={} reason=effect-name-mismatch expected={} actual={}",
                        track + 1,
                        step,
                        write.port,
                        process_target_label(target),
                        effect,
                        desc.name
                    )
                });
                return;
            }
            let Some(param_idx) = process_param_index_by_tag_or_name(desc, param) else {
                process_trace(snapshot, || {
                    format!(
                        "skip track={} step={} port={} target={} reason=effect-param-not-found descriptor={}",
                        track + 1,
                        step,
                        write.port,
                        process_target_label(target),
                        desc.name
                    )
                });
                return;
            };
            let Some(slot_snapshot) = track_snapshot.effect_slots.get(*slot) else {
                process_trace(snapshot, || {
                    format!(
                        "skip track={} step={} port={} target={} reason=missing-effect-slot-state slot={}",
                        track + 1,
                        step,
                        write.port,
                        process_target_label(target),
                        slot + 1
                    )
                });
                return;
            };
            if let Some(expected) = param_id {
                let actual = process_slot_param_identity(slot_snapshot, param_idx);
                if actual != Some(*expected) {
                    process_trace(snapshot, || {
                        format!(
                            "skip track={} step={} port={} target={} reason=effect-param-identity-mismatch expected={expected:?} actual={actual:?}",
                            track + 1,
                            step,
                            write.port,
                            process_target_label(target)
                        )
                    });
                    return;
                }
            }
            match process_apply_effect_write(
                snapshot,
                track,
                step,
                *slot,
                param_idx,
                write.op,
                write.value,
                overlay,
            ) {
                Some(applied) => process_trace(snapshot, || {
                    format!(
                        "apply track={} step={} port={} target={} op={} normalized={} param-idx={} node={}:{} raw={}",
                        track + 1,
                        step,
                        write.port,
                        process_target_label(target),
                        process_target_op_label(write.op),
                        write.value,
                        param_idx,
                        applied.logical_id,
                        applied.idx,
                        applied.value
                    )
                }),
                None => process_trace(snapshot, || {
                    format!(
                        "skip track={} step={} port={} target={} reason=effect-param-not-schedulable param-idx={}",
                        track + 1,
                        step,
                        write.port,
                        process_target_label(target),
                        param_idx
                    )
                }),
            }
        }
        crate::process::ParamTarget::MidiFxParam { slot, fx, param } => {
            let Some(desc) = midi_fx_descriptors
                .iter()
                .find(|desc| desc.name.eq_ignore_ascii_case(fx))
            else {
                process_trace(snapshot, || {
                    format!(
                        "skip track={} step={} port={} target={} reason=midi-fx-descriptor-not-loaded",
                        track + 1,
                        step,
                        write.port,
                        process_target_label(target)
                    )
                });
                return;
            };
            let Some(param_idx) = process_param_index_by_tag_or_name(desc, param) else {
                process_trace(snapshot, || {
                    format!(
                        "skip track={} step={} port={} target={} reason=midi-fx-param-not-found descriptor={}",
                        track + 1,
                        step,
                        write.port,
                        process_target_label(target),
                        desc.name
                    )
                });
                return;
            };
            match process_apply_midi_fx_write(
                snapshot,
                midi_fx_descriptors,
                track,
                step,
                *slot,
                fx,
                param_idx,
                write.op,
                write.value,
                overlay,
            ) {
                Some(applied) => process_trace(snapshot, || {
                    format!(
                        "apply track={} step={} port={} target={} op={} normalized={} slot={} param-idx={} raw={}",
                        track + 1,
                        step,
                        write.port,
                        process_target_label(target),
                        process_target_op_label(write.op),
                        write.value,
                        applied.slot + 1,
                        applied.param_idx,
                        applied.value
                    )
                }),
                None => process_trace(snapshot, || {
                    format!(
                        "skip track={} step={} port={} target={} reason=midi-fx-slot-mismatch-or-not-schedulable slot={}",
                        track + 1,
                        step,
                        write.port,
                        process_target_label(target),
                        slot + 1
                    )
                }),
            }
        }
        crate::process::ParamTarget::ProcessInlet { .. } => {
            process_trace(snapshot, || {
                format!(
                    "skip track={} step={} port={} target={} reason=process-inlet-context-missing",
                    track + 1,
                    step,
                    write.port,
                    process_target_label(target)
                )
            });
        }
        crate::process::ParamTarget::RackSlotParam { .. }
        | crate::process::ParamTarget::RackSlotInstrumentParam { .. } => {
            process_trace(snapshot, || {
                format!(
                    "skip track={} step={} port={} target={} reason=rack-target-not-supported",
                    track + 1,
                    step,
                    write.port,
                    process_target_label(target)
                )
            });
        }
    }
}

fn apply_process_target_writes(
    snapshot: &SequencerSnapshot,
    midi_fx_descriptors: &[EffectDescriptor],
    track: usize,
    step: usize,
    resolved: &mut ResolvedStep,
    overlay: &mut ProcessTargetOverlay,
    slot: Option<&crate::process::TrackProcessSlot>,
    writes: &[crate::process::ProcessTargetWrite],
    mut process_inlet_context: Option<&mut ProcessInletWriteContext<'_>>,
) {
    for write in writes {
        process_trace(snapshot, || {
            let slot_label = slot
                .map(|slot| format!("{}#{}", slot.class_name, slot.instance_id.0))
                .unwrap_or_else(|| "track-fire".to_string());
            let binding_label = match slot.and_then(|slot| slot.bindings.get(&write.port)) {
                Some(Some(_)) => "manual",
                Some(None) => "default",
                None if write.target.is_some() => "default",
                None => "unbound",
            };
            format!(
                "write track={} step={} slot={} port={} op={} value={} binding={} default_hint={:?}",
                track + 1,
                step,
                slot_label,
                write.port,
                process_target_op_label(write.op),
                write.value,
                binding_label,
                write.target
            )
        });
        let target = slot
            .and_then(|slot| slot.bindings.get(&write.port))
            .and_then(|binding| binding.as_ref().cloned())
            .or_else(|| {
                write.target.as_ref().and_then(|hint| {
                    process_resolve_hint_to_target(snapshot, midi_fx_descriptors, track, hint)
                })
            });
        let Some(target) = target else {
            process_trace(snapshot, || {
                format!(
                    "skip track={} step={} port={} reason=unresolved-target hint={:?}",
                    track + 1,
                    step,
                    write.port,
                    write.target
                )
            });
            continue;
        };
        process_trace(snapshot, || {
            format!(
                "resolve track={} step={} port={} -> {}",
                track + 1,
                step,
                write.port,
                process_target_label(&target)
            )
        });
        if matches!(target, crate::process::ParamTarget::ProcessInlet { .. }) {
            process_apply_inlet_write(
                snapshot,
                track,
                step,
                &target,
                write,
                process_inlet_context.as_deref_mut(),
            );
        } else {
            process_apply_concrete_target_write(
                snapshot,
                midi_fx_descriptors,
                track,
                step,
                resolved,
                overlay,
                &target,
                write,
            );
        }
    }
}

fn step_event_with_process_overlay(
    snapshot: &SequencerSnapshot,
    track: usize,
    step: usize,
    samples_per_step: f32,
    resolved: ResolvedStep,
    overlay: &ProcessTargetOverlay,
) -> StepEvent {
    let mut effect_params = resolve_effect_params(snapshot, track, step);
    let mut instrument_params = resolve_instrument_params(snapshot, track, step);
    upsert_effect_params(&mut effect_params, overlay.effect_params.clone());
    upsert_instrument_params(&mut instrument_params, overlay.instrument_params.clone());
    step_event_from_resolved(
        snapshot,
        track,
        step,
        samples_per_step,
        resolved,
        step_chord_data(snapshot, track, step),
        effect_params,
        instrument_params,
        resolve_instrument_tensor_params(snapshot, track, step),
    )
}

fn clamp_ratchet_event(
    mut event: crate::process::ProcessRatchetEvent,
) -> crate::process::ProcessRatchetEvent {
    event.offset_beats = event.offset_beats.max(0.0);
    let resolved = event.resolved;
    set_resolved_step_param(&mut event.resolved, StepParam::Duration, resolved.duration);
    set_resolved_step_param(&mut event.resolved, StepParam::Velocity, resolved.velocity);
    set_resolved_step_param(&mut event.resolved, StepParam::Speed, resolved.speed);
    set_resolved_step_param(&mut event.resolved, StepParam::AuxA, resolved.aux_a);
    set_resolved_step_param(&mut event.resolved, StepParam::AuxB, resolved.aux_b);
    set_resolved_step_param(
        &mut event.resolved,
        StepParam::Transpose,
        resolved.transpose,
    );
    set_resolved_step_param(&mut event.resolved, StepParam::Pan, resolved.pan);
    set_resolved_step_param(&mut event.resolved, StepParam::Chop, resolved.chop);
    event
}

#[allow(clippy::too_many_arguments)]
fn materialize_process_ratchet(
    scratch: &mut lisp_host::ScratchControlRuntime,
    process_runtime: &mut crate::process::ProcessRuntime,
    process_runtime_id: u64,
    snapshot: &SequencerSnapshot,
    track: usize,
    step: usize,
    absolute_beats: f64,
    samples_per_step: f32,
    base_resolved: ResolvedStep,
    overlay: &ProcessTargetOverlay,
    request: &crate::process::ProcessRatchetRequest,
) -> Result<(), String> {
    if request.times == 0 {
        return Ok(());
    }
    let step_beats = request.shape_context.step_context.step_beats.max(0.0);
    let span_beats = request.span_beats.unwrap_or(step_beats).max(0.0);
    let subdivided_span = if request.times > 0 {
        span_beats / request.times as f32
    } else {
        0.0
    };
    let mut shape_context = request.shape_context.clone();
    let mut scheduled_events = Vec::with_capacity(request.times as usize);
    for index in 0..request.times {
        let mut resolved = base_resolved;
        let offset_beats = match request.mode {
            crate::process::ProcessRatchetMode::Subdivide => {
                if step_beats > 0.0 {
                    set_resolved_step_param(
                        &mut resolved,
                        StepParam::Duration,
                        subdivided_span / step_beats,
                    );
                }
                index as f32 * subdivided_span
            }
            crate::process::ProcessRatchetMode::Repeat => index as f32 * span_beats,
        };
        let mut event = crate::process::ProcessRatchetEvent {
            offset_beats,
            resolved,
        };
        if let Some(shape) = request.shape.as_ref() {
            event = scratch
                .invoke_process_ratchet_shape(&mut shape_context, shape, index, event)
                .map_err(|err| {
                    format!(
                        "ratchet shape error process={} track={} step={} index={} err={}",
                        process_runtime_id, track, step, index, err
                    )
                })?;
        }
        let event = clamp_ratchet_event(event);
        let step_event = step_event_with_process_overlay(
            snapshot,
            track,
            step,
            samples_per_step,
            event.resolved,
            overlay,
        );
        scheduled_events.push((
            absolute_beats + event.offset_beats as f64,
            crate::process::ProcessScheduledStepEvent {
                event: step_event,
                midi_fx_params: overlay.midi_fx_params.clone(),
            },
        ));
    }
    for (beat, event) in scheduled_events {
        process_runtime.schedule_step_event_at(process_runtime_id, beat, event);
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn apply_step_process_commands(
    scratch: &mut lisp_host::ScratchControlRuntime,
    process_runtime: &mut crate::process::ProcessRuntime,
    process_runtime_id: u64,
    snapshot: &SequencerSnapshot,
    midi_fx_descriptors: &[EffectDescriptor],
    track: usize,
    step: usize,
    absolute_beats: f64,
    samples_per_step: f32,
    slot: Option<&crate::process::TrackProcessSlot>,
    resolved: &mut ResolvedStep,
    overlay: &mut ProcessTargetOverlay,
    process_base_alive: &mut bool,
    commands: &[crate::process::ProcessRunCommand],
    mut process_inlet_context: Option<&mut ProcessInletWriteContext<'_>>,
    debug_accum: bool,
) {
    for command in commands {
        match command {
            crate::process::ProcessRunCommand::TargetWrite(write) => {
                apply_process_target_writes(
                    snapshot,
                    midi_fx_descriptors,
                    track,
                    step,
                    resolved,
                    overlay,
                    slot,
                    std::slice::from_ref(write),
                    process_inlet_context.as_deref_mut(),
                );
            }
            crate::process::ProcessRunCommand::VetoBaseEvent => {
                *process_base_alive = false;
                process_trace(snapshot, || {
                    format!(
                        "veto track={} step={} process={}",
                        track + 1,
                        step,
                        process_runtime_id
                    )
                });
            }
            crate::process::ProcessRunCommand::Ratchet(request) => {
                if let Err(err) = materialize_process_ratchet(
                    scratch,
                    process_runtime,
                    process_runtime_id,
                    snapshot,
                    track,
                    step,
                    absolute_beats,
                    samples_per_step,
                    *resolved,
                    overlay,
                    request,
                ) {
                    if debug_accum || debug_routing_enabled() {
                        eprintln!("[process] {err}");
                    }
                }
            }
        }
    }
}

fn apply_process_midi_fx_overrides_to_slot(
    slot: &mut crate::effects::EffectSlotSnapshot,
    step: usize,
    stage_idx: usize,
    fx_name: &str,
    descriptor: &EffectDescriptor,
    overrides: &[ProcessMidiFxParamOverride],
) {
    for override_param in overrides {
        if override_param.slot != stage_idx
            || override_param.param_idx >= slot.num_params as usize
            || !override_param.fx.eq_ignore_ascii_case(fx_name)
            || !override_param.value.is_finite()
        {
            continue;
        }
        let Some(desc_param) = descriptor.params.get(override_param.param_idx) else {
            continue;
        };
        if !desc_param.name.eq_ignore_ascii_case(&override_param.param) {
            continue;
        }
        slot.set_plock(step, override_param.param_idx, override_param.value);
    }
}

fn invoke_process_cascade<F>(
    scratch_runtime: &mut Option<lisp_host::ScratchControlRuntime>,
    process_runtime: &mut crate::process::ProcessRuntime,
    initial: crate::process::ProcessRunInvocation,
    debug_accum: bool,
    mut apply_commands: F,
) -> bool
where
    F: FnMut(
        &mut lisp_host::ScratchControlRuntime,
        &mut crate::process::ProcessRuntime,
        u64,
        &[crate::process::ProcessRunCommand],
    ),
{
    let mut pending_invocations = vec![initial];
    let mut processed_invocations = 0usize;
    while let Some(mut invocation) = pending_invocations.pop() {
        processed_invocations += 1;
        if processed_invocations > PROCESS_EVENT_CASCADE_LIMIT {
            if debug_accum || debug_routing_enabled() {
                eprintln!(
                    "[process] listener cascade limit exceeded limit={}",
                    PROCESS_EVENT_CASCADE_LIMIT
                );
            }
            return false;
        }
        let invocation_beat = invocation.beat;
        let process_runtime_id = invocation.runtime_id;
        invocation.reads = process_runtime.read_snapshot(invocation_beat);
        let Some(scratch) = scratch_runtime.as_mut() else {
            return true;
        };
        match scratch.invoke_process_run(invocation) {
            Ok(result) => {
                let runtime_id = result.runtime_id;
                apply_commands(scratch, process_runtime, runtime_id, &result.commands);
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
    }
    true
}

fn process_step_event_value(
    track: usize,
    step: usize,
    cycle: u64,
    beat: f64,
    sample_time: u64,
    resolved: ResolvedStep,
    step_beats: f32,
) -> eseqlisp::vm::Value {
    fn number(value: impl Into<f64>) -> Rc<RefCell<eseqlisp::vm::Value>> {
        Rc::new(RefCell::new(eseqlisp::vm::Value::Number(value.into())))
    }

    let mut map = HashMap::new();
    map.insert("track".to_string(), number(track as f64));
    map.insert("step".to_string(), number(step as f64));
    map.insert("cycle".to_string(), number(cycle as f64));
    map.insert("beat".to_string(), number(beat));
    map.insert("sample-time".to_string(), number(sample_time as f64));
    map.insert("step-length".to_string(), number(step_beats as f64));
    map.insert("duration".to_string(), number(resolved.duration as f64));
    map.insert("velocity".to_string(), number(resolved.velocity as f64));
    map.insert("speed".to_string(), number(resolved.speed as f64));
    map.insert("aux-a".to_string(), number(resolved.aux_a as f64));
    map.insert("aux-b".to_string(), number(resolved.aux_b as f64));
    map.insert("transpose".to_string(), number(resolved.transpose as f64));
    map.insert("pan".to_string(), number(resolved.pan as f64));
    map.insert("chop".to_string(), number(resolved.chop as f64));
    eseqlisp::vm::Value::Map(map)
}

fn normalize_network_event_destination(
    snapshot: &SequencerSnapshot,
    neuron_idx: usize,
    seed: Option<(usize, usize)>,
    event: &mut StepEvent,
) {
    if seed.map(|(track, _)| track != event.track).unwrap_or(true) {
        let event_effect_params = seed
            .is_none()
            .then(|| std::mem::take(&mut event.effect_params))
            .unwrap_or_default();
        let event_instrument_params = if seed.is_none() {
            std::mem::replace(
                &mut event.instrument_params,
                ScheduledInstrumentParams::new(),
            )
        } else {
            ScheduledInstrumentParams::new()
        };
        let event_instrument_tensor_params = if seed.is_none() {
            std::mem::replace(
                &mut event.instrument_tensor_params,
                ScheduledInstrumentTensorParams::new(),
            )
        } else {
            ScheduledInstrumentTensorParams::new()
        };
        event.effect_params = resolve_effect_defaults(snapshot, event.track);
        event.instrument_params = resolve_instrument_defaults(snapshot, event.track);
        event.instrument_tensor_params = resolve_instrument_tensor_defaults(snapshot, event.track);
        event.sampler_params = resolve_sampler_defaults(snapshot, event.track);
        upsert_effect_params(&mut event.effect_params, event_effect_params);
        upsert_instrument_params(&mut event.instrument_params, event_instrument_params);
        upsert_instrument_tensor_params(
            &mut event.instrument_tensor_params,
            event_instrument_tensor_params,
        );
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
        warp_preserve: default_slot_node_param_value(
            slot,
            crate::sampler::PARAM_WARP_PRESERVE as u32,
            crate::sampler::WARP_PRESERVE_DEFAULT as f32,
        ),
        warp_seg_loop_mode: default_slot_node_param_value(
            slot,
            crate::sampler::PARAM_WARP_SEG_LOOP_MODE as u32,
            crate::sampler::WARP_SEG_LOOP_MODE_DEFAULT as f32,
        ),
        warp_seg_envelope: default_slot_node_param_value(
            slot,
            crate::sampler::PARAM_WARP_SEG_ENVELOPE as u32,
            crate::sampler::WARP_SEG_ENVELOPE_DEFAULT,
        ),
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
                apply_sampler_descriptor_param_override(
                    &mut event.sampler_params,
                    param_idx,
                    param.value,
                );
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

fn apply_sampler_descriptor_param_override(
    params: &mut ScheduledSamplerParams,
    param_idx: u64,
    value: f32,
) -> bool {
    match param_idx {
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
        _ => return false,
    }
    true
}

fn apply_sampler_state_param_override(
    params: &mut ScheduledSamplerParams,
    node_param_idx: u64,
    value: f32,
) -> bool {
    match node_param_idx {
        idx if idx == crate::sampler::PARAM_ATTACK_SAMPLES => params.attack_ms = value,
        idx if idx == crate::sampler::PARAM_RELEASE_SAMPLES => params.release_ms = value,
        idx if idx == crate::sampler::PARAM_START_POINT => params.start_point = value,
        idx if idx == crate::sampler::PARAM_END_POINT => params.end_point = value,
        idx if idx == crate::sampler::PARAM_ENABLED => params.instrument_enabled = value,
        idx if idx == crate::sampler::PARAM_REVERSE => params.reverse = value,
        idx if idx == crate::sampler::PARAM_LOOP_MODE => params.loop_mode = value,
        idx if idx == crate::sampler::PARAM_LOOP_XFADE_SAMPLES => params.loop_xfade_ms = value,
        idx if idx == crate::sampler::PARAM_SR_HZ => params.sr_hz = value,
        idx if idx == crate::sampler::PARAM_WARP_ENABLED => params.warp_enabled = value,
        idx if idx == crate::sampler::PARAM_WARP_MODE => params.warp_mode = value,
        idx if idx == crate::sampler::PARAM_WARP_SAMPLE_BPM => params.sample_bpm = value,
        idx if idx == crate::sampler::PARAM_SPEED => params.playback_speed = value,
        idx if idx == crate::sampler::PARAM_SCRUB_OFFSET => params.scrub = value,
        idx if idx == crate::sampler::PARAM_WARP_PRESERVE => params.warp_preserve = value,
        idx if idx == crate::sampler::PARAM_WARP_SEG_LOOP_MODE => {
            params.warp_seg_loop_mode = value;
        }
        idx if idx == crate::sampler::PARAM_WARP_SEG_ENVELOPE => {
            params.warp_seg_envelope = value;
        }
        _ => return false,
    }
    true
}

fn sampler_descriptor_param_index_for_scheduled_param(
    slot: &crate::effects::EffectSlotSnapshot,
    param: &ScheduledInstrumentParam,
) -> Option<usize> {
    if !matches!(param.target, ScheduledInstrumentParamTarget::Synth) {
        return None;
    }
    slot.param_node_indices
        .iter()
        .take(slot.num_params as usize)
        .position(|raw_idx| *raw_idx as u64 == param.idx)
}

fn apply_sampler_instrument_param_overrides(
    snapshot: &SequencerSnapshot,
    track_idx: usize,
    sampler_params: &mut ScheduledSamplerParams,
    instrument_params: &ScheduledInstrumentParams,
) {
    if !matches!(
        snapshot
            .tracks
            .get(track_idx)
            .map(|track| track.instrument_type),
        Some(InstrumentType::Sampler)
    ) {
        return;
    }
    let slot = &snapshot.tracks[track_idx].instrument_slot;
    for param in instrument_params {
        if matches!(param.target, ScheduledInstrumentParamTarget::Synth) {
            let applied = sampler_descriptor_param_index_for_scheduled_param(slot, param)
                .map(|param_idx| {
                    apply_sampler_descriptor_param_override(
                        sampler_params,
                        param_idx as u64,
                        param.value,
                    )
                })
                .unwrap_or(false);
            if !applied {
                apply_sampler_state_param_override(sampler_params, param.idx, param.value);
            }
        }
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

fn apply_global_transpose_to_resolved(
    snapshot: &SequencerSnapshot,
    track_idx: usize,
    global_transpose: f32,
    mut resolved: ResolvedStep,
) -> ResolvedStep {
    if global_transpose.abs() > f32::EPSILON
        && snapshot
            .tracks
            .get(track_idx)
            .map(|track| track.params.global_transpose)
            .unwrap_or(false)
    {
        resolved.transpose += global_transpose;
    }
    resolved
}

fn enqueue_network_trigger<const QUEUE_CAP: usize>(
    queue: &ScheduledEventQueue<QUEUE_CAP>,
    snapshot: &SequencerSnapshot,
    track_output_events: &mut Vec<TrackOutputEvent>,
    pattern_epoch: u64,
    sample_time: u64,
    event_beat: f64,
    samples_per_quarter: f32,
    global_transpose: f32,
    track_idx: usize,
    source_neuron: usize,
    seed: Option<(usize, usize)>,
    samples_per_step: f32,
    resolved: ResolvedStep,
    chord: ScheduledChordData,
    effect_params: Vec<ScheduledEffectParam>,
    instrument_params: ScheduledInstrumentParams,
    instrument_tensor_params: ScheduledInstrumentTensorParams,
    mut sampler_params: ScheduledSamplerParams,
    instrument_fingerprint: u64,
) -> bool {
    let (resolved, chord) = apply_fit_to_scale_to_trigger(snapshot, track_idx, resolved, chord);
    let resolved =
        apply_global_transpose_to_resolved(snapshot, track_idx, global_transpose, resolved);
    apply_sampler_instrument_param_overrides(
        snapshot,
        track_idx,
        &mut sampler_params,
        &instrument_params,
    );
    process_trace(snapshot, || {
        format!(
            "enqueue kind=network track={} source_neuron={} seed={:?} inst_params={} sampler.attack={} sampler.release={} sampler.speed={}",
            track_idx + 1,
            source_neuron,
            seed,
            instrument_params.len(),
            sampler_params.attack_ms,
            sampler_params.release_ms,
            sampler_params.playback_speed
        )
    });
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
                            instrument_tensor_params: instrument_tensor_params.clone(),
                            sampler_params,
                            instrument_fingerprint,
                        },
                    })
                    .is_err()
                {
                    ok = false;
                    break;
                }
                let note_beat = event_beat
                    + (note_sample_time.saturating_sub(sample_time) as f64)
                        / samples_per_quarter.max(1.0) as f64;
                record_track_output_event(
                    track_output_events,
                    track_idx,
                    note_sample_time,
                    note_beat,
                    resolved,
                );
            }
            return ok;
        }
    }
    let enqueued = queue
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
                instrument_tensor_params,
                sampler_params,
                instrument_fingerprint,
            },
        })
        .is_ok();
    if enqueued {
        record_track_output_event(
            track_output_events,
            track_idx,
            sample_time,
            event_beat,
            resolved,
        );
    }
    enqueued
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
    midi_fx_params: Vec<ProcessMidiFxParamOverride>,
    effect_params: Vec<ScheduledEffectParam>,
    instrument_params: ScheduledInstrumentParams,
    instrument_tensor_params: ScheduledInstrumentTensorParams,
    sampler_params: ScheduledSamplerParams,
    source: EventSource,
}

#[derive(Clone)]
struct PendingQuantizedMidiFxEvent {
    source_track: usize,
    stage_idx: usize,
    resume_stage_idx: usize,
    deadline_beats: f64,
    event: MidiFxEvent,
}

#[derive(Default)]
struct MidiFxQuantizerState {
    pending: Vec<PendingQuantizedMidiFxEvent>,
}

impl MidiFxQuantizerState {
    fn reset(&mut self) {
        self.pending.clear();
    }

    fn push_or_replace(
        &mut self,
        source_track: usize,
        stage_idx: usize,
        resume_stage_idx: usize,
        deadline_beats: f64,
        mut event: MidiFxEvent,
    ) {
        event.offset_beats = 0.0;
        event.arp_phase_beats = deadline_beats as f32;
        let existing = self.pending.iter_mut().find(|pending| {
            pending.source_track == source_track
                && pending.stage_idx == stage_idx
                && (pending.deadline_beats - deadline_beats).abs() <= 1e-9
        });
        if let Some(pending) = existing {
            if event.resolved.velocity > pending.event.resolved.velocity {
                pending.resume_stage_idx = resume_stage_idx;
                pending.event = event;
            }
        } else {
            self.pending.push(PendingQuantizedMidiFxEvent {
                source_track,
                stage_idx,
                resume_stage_idx,
                deadline_beats,
                event,
            });
        }
    }

    fn drain_due(&mut self, up_to_beats: f64) -> Vec<PendingQuantizedMidiFxEvent> {
        let mut due = Vec::new();
        let mut idx = 0;
        while idx < self.pending.len() {
            if self.pending[idx].deadline_beats <= up_to_beats + 1e-9 {
                due.push(self.pending.swap_remove(idx));
            } else {
                idx += 1;
            }
        }
        due.sort_by(|a, b| {
            a.deadline_beats
                .partial_cmp(&b.deadline_beats)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        due
    }
}

#[derive(Clone, Copy)]
struct LiveMidiFxNote {
    transpose: f32,
    velocity: f32,
    pending_event: bool,
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
    instrument_tensor_params: ScheduledInstrumentTensorParams,
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
        midi_fx_params: Vec::new(),
        effect_params,
        instrument_params,
        instrument_tensor_params,
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
    midi_fx_params: Vec<ProcessMidiFxParamOverride>,
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
        midi_fx_params,
        effect_params: event.effect_params,
        instrument_params: event.instrument_params,
        instrument_tensor_params: event.instrument_tensor_params,
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
        if debug_routing_enabled() {
            eprintln!(
                "[routing] rebind drop reason=target-out-of-range from_track={} target_track={} tracks={} source={} step={}",
                event.track,
                target_track,
                snapshot.tracks.len(),
                event_source_label(&event.source),
                event.step
            );
        }
        return None;
    }
    if event.track == target_track {
        if debug_routing_enabled() {
            eprintln!(
                "[routing] rebind noop track={} source={} step={}",
                event.track,
                event_source_label(&event.source),
                event.step
            );
        }
        return Some(event);
    }
    if debug_routing_enabled() {
        eprintln!(
            "[routing] rebind from_track={} target_track={} source={} step={} transpose={} explicit_fx_params={} explicit_inst_params={}",
            event.track,
            target_track,
            event_source_label(&event.source),
            event.step,
            event.resolved.transpose,
            event.effect_params.len(),
            event.instrument_params.len()
        );
    }
    let explicit_effect_params = std::mem::take(&mut event.effect_params);
    let explicit_instrument_params = std::mem::replace(
        &mut event.instrument_params,
        ScheduledInstrumentParams::new(),
    );
    let explicit_instrument_tensor_params = std::mem::replace(
        &mut event.instrument_tensor_params,
        ScheduledInstrumentTensorParams::new(),
    );
    let target_step = midi_fx_event_step_for_track(snapshot, target_track, event.step);
    event.track = target_track;
    event.step = target_step;
    event.midi_fx_params.clear();
    event.effect_params = resolve_effect_params(snapshot, target_track, target_step);
    event.instrument_params = resolve_instrument_params(snapshot, target_track, target_step);
    event.instrument_tensor_params =
        resolve_instrument_tensor_params(snapshot, target_track, target_step);
    event.sampler_params = resolve_sampler_params(snapshot, target_track, target_step);
    upsert_effect_params(&mut event.effect_params, explicit_effect_params);
    upsert_instrument_params(&mut event.instrument_params, explicit_instrument_params);
    upsert_instrument_tensor_params(
        &mut event.instrument_tensor_params,
        explicit_instrument_tensor_params,
    );
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
    if debug_routing_enabled() {
        eprintln!(
            "[routing] rebind result target_track={} target_step={} fx_params={} inst_params={} sampler_speed={}",
            event.track,
            event.step,
            event.effect_params.len(),
            event.instrument_params.len(),
            event.sampler_params.playback_speed
        );
    }
    Some(event)
}

fn midi_fx_window_events_from_step(
    snapshot: &SequencerSnapshot,
    midi_fx_descriptors: &[EffectDescriptor],
    track_idx: usize,
    step_idx: usize,
    samples_per_step: f32,
    step_beats: f32,
    samples_per_quarter: f32,
    arp_phase_beats: f32,
    resolved: ResolvedStep,
    effect_params: Vec<ScheduledEffectParam>,
    instrument_params: ScheduledInstrumentParams,
    instrument_tensor_params: ScheduledInstrumentTensorParams,
) -> Vec<MidiFxEvent> {
    const EPS: f32 = 1e-5;
    const MAX_WINDOWS: usize = 1024;

    let note_spans = track_note_spans_for_trigger(snapshot, track_idx, step_idx);
    if note_spans.is_empty() {
        if debug_routing_enabled() {
            eprintln!(
                "[midi-fx-window] no note spans track={} step={} -> single event",
                track_idx, step_idx
            );
        }
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
            instrument_tensor_params,
        )];
    }

    let Some(window_beats) =
        midi_fx_clock_tick_beats(snapshot, midi_fx_descriptors, track_idx, step_idx)
    else {
        if debug_routing_enabled() {
            eprintln!(
                "[midi-fx-window] no clock-rate role track={} step={} chain={:?} descriptors={:?} -> single event",
                track_idx,
                step_idx,
                snapshot.tracks[track_idx].params.midi_fx_chain,
                midi_fx_descriptors
                    .iter()
                    .map(|desc| desc.name.as_str())
                    .collect::<Vec<_>>()
            );
        }
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
            instrument_tensor_params,
        )];
    };
    let window_beats = window_beats.max(EPS);
    let window_samples = (samples_per_quarter * window_beats).round().max(1.0);
    let end_beats = note_spans
        .iter()
        .map(|span| span.end_beats)
        .fold(0.0_f32, f32::max);
    if end_beats <= EPS {
        return Vec::new();
    }

    let window_count = ((end_beats / window_beats).ceil() as usize).min(MAX_WINDOWS);
    if debug_routing_enabled() {
        eprintln!(
            "[midi-fx-window] clocked track={} step={} window_beats={} window_samples={} end_beats={} windows={} note_spans={}",
            track_idx,
            step_idx,
            window_beats,
            window_samples,
            end_beats,
            window_count,
            note_spans.len()
        );
    }
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
            midi_fx_params: Vec::new(),
            effect_params: effect_params.clone(),
            instrument_params: instrument_params.clone(),
            instrument_tensor_params: instrument_tensor_params.clone(),
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
    runtime: &mut lisp_host::ScratchControlRuntime,
    snapshot: &SequencerSnapshot,
    source_track: usize,
    events: Vec<MidiFxEvent>,
    quantizer_state: Option<&mut MidiFxQuantizerState>,
    depth: usize,
    debug_accum: bool,
) -> Vec<MidiFxEvent> {
    run_midi_fx_chain_for_track_inner(
        runtime,
        snapshot,
        source_track,
        events,
        quantizer_state,
        0,
        depth,
        [false; MAX_TRACKS],
        debug_accum,
    )
}

fn run_midi_fx_chain_for_track_inner(
    runtime: &mut lisp_host::ScratchControlRuntime,
    snapshot: &SequencerSnapshot,
    source_track: usize,
    events: Vec<MidiFxEvent>,
    mut quantizer_state: Option<&mut MidiFxQuantizerState>,
    start_stage_idx: usize,
    depth: usize,
    mut visited_tracks: [bool; MAX_TRACKS],
    debug_accum: bool,
) -> Vec<MidiFxEvent> {
    if source_track >= snapshot.tracks.len() || depth >= MAX_TRACKS {
        if debug_routing_enabled() {
            eprintln!(
                "[midi-fx] skip chain reason=invalid-source-or-depth source_track={} tracks={} depth={}",
                source_track,
                snapshot.tracks.len(),
                depth
            );
        }
        return Vec::new();
    }
    if visited_tracks.get(source_track).copied().unwrap_or(true) {
        if debug_accum || debug_routing_enabled() {
            eprintln!("[midi-fx] dropped recursive route into track={source_track}");
        }
        return Vec::new();
    }
    visited_tracks[source_track] = true;
    let chain = snapshot.tracks[source_track].params.midi_fx_chain.clone();
    if chain.is_empty() {
        if debug_routing_enabled() {
            eprintln!(
                "[midi-fx] skip chain reason=empty-chain source_track={} events={}",
                source_track,
                events.len()
            );
        }
        return events;
    }
    let names = runtime.midi_fx_names();
    let descriptors = runtime.midi_fx_descriptors();
    if debug_routing_enabled() {
        eprintln!(
            "[midi-fx] chain start source_track={} depth={} events={} chain={:?} registered={:?}",
            source_track,
            depth,
            events.len(),
            chain,
            names
        );
    }
    let mut current = events;
    for (stage_idx, fx_name) in chain.into_iter().enumerate().skip(start_stage_idx) {
        let Some(fx_idx) = names
            .iter()
            .position(|name| name.eq_ignore_ascii_case(&fx_name))
        else {
            if debug_accum || debug_routing_enabled() {
                eprintln!("[midi-fx] missing fx name={fx_name:?} track={source_track}");
            }
            continue;
        };
        let mut next = Vec::new();
        for event in current {
            if event.track != source_track {
                if visited_tracks.get(event.track).copied().unwrap_or(true) {
                    if debug_accum || debug_routing_enabled() {
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
                    quantizer_state.as_deref_mut(),
                    0,
                    depth + 1,
                    visited_tracks,
                    debug_accum,
                ));
                continue;
            }
            let mut slot_snapshot = snapshot.tracks[event.track]
                .midi_fx_slots
                .get(stage_idx)
                .cloned()
                .unwrap_or_else(crate::effects::EffectSlotSnapshot::new_empty);
            if let Some(desc) = descriptors.get(fx_idx) {
                if slot_snapshot.num_params == 0 && !desc.params.is_empty() {
                    slot_snapshot = crate::effects::EffectSlotSnapshot::new_default(desc, 0);
                }
                apply_process_midi_fx_overrides_to_slot(
                    &mut slot_snapshot,
                    event.step,
                    stage_idx,
                    &fx_name,
                    desc,
                    &event.midi_fx_params,
                );
            }
            let enabled = descriptors
                .get(fx_idx)
                .and_then(|desc| {
                    desc.params
                        .iter()
                        .position(|param| param.name.eq_ignore_ascii_case("enabled"))
                })
                .and_then(|param_idx| {
                    Some(midi_fx_slot_param_value(
                        &slot_snapshot,
                        event.step,
                        param_idx,
                        1.0,
                    ))
                })
                .unwrap_or(1.0);
            if enabled <= 0.5 {
                if debug_routing_enabled() {
                    eprintln!(
                        "[midi-fx] stage skip reason=disabled track={} step={} fx={} stage={} source={} transpose={}",
                        event.track,
                        event.step,
                        fx_name,
                        stage_idx,
                        event_source_label(&event.source),
                        event.resolved.transpose
                    );
                }
                next.push(event);
                continue;
            }
            if let Some(grid_param_idx) = descriptors
                .get(fx_idx)
                .and_then(midi_fx_quantizer_grid_param)
            {
                if let Some(state) = quantizer_state.as_deref_mut() {
                    let grid_beats = midi_fx_timebase_param_beats_from_slot(
                        snapshot,
                        event.track,
                        &slot_snapshot,
                        grid_param_idx,
                        event.step,
                    )
                    .unwrap_or(event.step_beats.max(1.0 / 1024.0));
                    let samples_per_quarter = if event.step_beats > 0.0 {
                        event.samples_per_step as f64 / event.step_beats as f64
                    } else {
                        0.0
                    };
                    let boundary_tolerance_beats = if samples_per_quarter > 0.0 {
                        1.5 / samples_per_quarter
                    } else {
                        1e-9
                    };
                    let event_beats = snap_near_grid_down(
                        event.arp_phase_beats as f64 + event.offset_beats as f64,
                        grid_beats as f64,
                        boundary_tolerance_beats,
                    );
                    let deadline = ceil_to_grid(event_beats, grid_beats as f64);
                    state.push_or_replace(source_track, stage_idx, stage_idx + 1, deadline, event);
                    continue;
                }
                next.push(event);
                continue;
            }
            if debug_routing_enabled() {
                eprintln!(
                    "[midi-fx] invoke track={} step={} fx={} stage={} source={} chord={} note_spans={} offset={} step_beats={} transpose={} vel={} fx_params={} inst_params={} sampler_speed={}",
                    event.track,
                    event.step,
                    fx_name,
                    stage_idx,
                    event_source_label(&event.source),
                    event.chord.len(),
                    event
                        .note_spans
                        .as_ref()
                        .map(|spans| spans.len())
                        .unwrap_or(0),
                    event.offset_beats,
                    event.step_beats,
                    event.resolved.transpose,
                    event.resolved.velocity,
                    event.effect_params.len(),
                    event.instrument_params.len(),
                    event.sampler_params.playback_speed
                );
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
                    if debug_routing_enabled() {
                        eprintln!(
                            "[midi-fx] output track={} step={} fx={} suppressed={} emitted={} resolved_transpose={} resolved_vel={} fx_params={} inst_params={}",
                            event.track,
                            event.step,
                            fx_name,
                            output.suppressed,
                            output.emitted.len(),
                            output.resolved.transpose,
                            output.resolved.velocity,
                            output.effect_params.len(),
                            output.instrument_params.len()
                        );
                    }
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
                            if debug_routing_enabled() {
                                eprintln!(
                                    "[midi-fx] emitted drop reason=target-out-of-range fx={} source_track={} target_track={} tracks={}",
                                    fx_name,
                                    event.track,
                                    target_track,
                                    snapshot.tracks.len()
                                );
                            }
                            continue;
                        }
                        if debug_routing_enabled() {
                            eprintln!(
                                "[midi-fx] emitted fx={} from_track={} target_track={} offset={} transpose={} vel={} emitted_fx_params={} emitted_inst_params={}",
                                fx_name,
                                event.track,
                                target_track,
                                emitted.offset_beats,
                                emitted.resolved.transpose,
                                emitted.resolved.velocity,
                                emitted.effect_params.len(),
                                emitted.instrument_params.len()
                            );
                        }
                        let chord_len = emitted.chord.len();
                        let mut effect_params = emitted.effect_params;
                        let mut instrument_params =
                            scheduled_instrument_params_from_vec(emitted.instrument_params);
                        if target_track == event.track {
                            let explicit_effect_params = effect_params;
                            let explicit_instrument_params = instrument_params;
                            effect_params = event.effect_params.clone();
                            instrument_params = event.instrument_params.clone();
                            upsert_effect_params(&mut effect_params, explicit_effect_params);
                            upsert_instrument_params(
                                &mut instrument_params,
                                explicit_instrument_params,
                            );
                        }
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
                            midi_fx_params: event.midi_fx_params.clone(),
                            effect_params,
                            instrument_params,
                            instrument_tensor_params: event.instrument_tensor_params.clone(),
                            sampler_params: event.sampler_params,
                            source: event.source.clone(),
                        };
                        if target_track == source_track {
                            next.push(routed);
                        } else if visited_tracks.get(target_track).copied().unwrap_or(true) {
                            if debug_accum || debug_routing_enabled() {
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
                                quantizer_state.as_deref_mut(),
                                0,
                                depth + 1,
                                visited_tracks,
                                debug_accum,
                            ));
                        }
                    }
                }
                Err(err) => {
                    if debug_accum || debug_routing_enabled() {
                        eprintln!(
                            "[midi-fx] invoke err track={} step={} fx={} err={}",
                            event.track, event.step, fx_name, err
                        );
                    }
                    next.push(event);
                }
            }
            if next.len() > 1024 {
                if debug_routing_enabled() {
                    eprintln!(
                        "[midi-fx] truncate stage={} source_track={} len={} max=1024",
                        stage_idx,
                        source_track,
                        next.len()
                    );
                }
                next.truncate(1024);
                break;
            }
        }
        if debug_routing_enabled() {
            eprintln!(
                "[midi-fx] stage done source_track={} stage={} fx={} output_events={}",
                source_track,
                stage_idx,
                fx_name,
                next.len()
            );
        }
        current = next;
    }
    if debug_routing_enabled() {
        eprintln!(
            "[midi-fx] chain done source_track={} depth={} output_events={}",
            source_track,
            depth,
            current.len()
        );
    }
    current
}

fn enqueue_midi_fx_events<const QUEUE_CAP: usize>(
    queue: &ScheduledEventQueue<QUEUE_CAP>,
    snapshot: &SequencerSnapshot,
    track_output_events: &mut Vec<TrackOutputEvent>,
    pattern_epoch: u64,
    base_sample_time: u64,
    base_beat: f64,
    samples_per_quarter: f32,
    global_transpose: f32,
    events: Vec<MidiFxEvent>,
) -> bool {
    let mut ok = true;
    for event in events {
        let sample_time = base_sample_time
            .saturating_add((event.offset_beats.max(0.0) * samples_per_quarter).round() as u64);
        let enqueue_track = event.track;
        let enqueue_sample_time = sample_time;
        let chord = chord_data_from_parts(
            &event.chord,
            &event.chord_durations,
            &event.chord_delays,
            event.resolved.duration,
            event.chord_step_transpose,
        );
        let instrument_fingerprint = instrument_sound_fingerprint(
            snapshot,
            event.track,
            &event.instrument_params,
            &event.instrument_tensor_params,
        );
        if debug_routing_enabled() {
            eprintln!(
                "[routing] enqueue source={} track={} step={} sample={} offset={} transpose={} vel={} chord={} fx_params={} inst_params={} sampler_speed={} fingerprint={}",
                event_source_label(&event.source),
                event.track,
                event.step,
                sample_time,
                event.offset_beats,
                event.resolved.transpose,
                event.resolved.velocity,
                chord.count,
                event.effect_params.len(),
                event.instrument_params.len(),
                event.sampler_params.playback_speed,
                instrument_fingerprint
            );
        }
        let enqueued = match event.source {
            EventSource::Network { seed, neuron, .. } => enqueue_network_trigger(
                queue,
                snapshot,
                track_output_events,
                pattern_epoch,
                sample_time,
                base_beat + event.offset_beats as f64,
                samples_per_quarter,
                global_transpose,
                event.track,
                neuron,
                seed,
                event.samples_per_step,
                event.resolved,
                chord,
                event.effect_params,
                event.instrument_params,
                event.instrument_tensor_params,
                event.sampler_params,
                instrument_fingerprint,
            ),
            EventSource::Step { .. } => enqueue_resolved_trigger(
                queue,
                snapshot,
                track_output_events,
                pattern_epoch,
                sample_time,
                base_beat + event.offset_beats as f64,
                samples_per_quarter,
                global_transpose,
                event.track,
                event.step,
                event.samples_per_step,
                event.resolved,
                chord,
                event.effect_params,
                event.instrument_params,
                event.instrument_tensor_params,
                event.sampler_params,
            ),
        };
        if !enqueued {
            if debug_routing_enabled() {
                eprintln!(
                    "[routing] enqueue failed track={} sample={} queue_capacity={}",
                    enqueue_track, enqueue_sample_time, QUEUE_CAP
                );
            }
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
            note.pending_event = true;
        } else {
            track_state.notes.push(LiveMidiFxNote {
                transpose: trigger.transpose,
                velocity: trigger.velocity,
                pending_event: true,
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
    midi_fx_descriptors: &[EffectDescriptor],
    track_idx: usize,
) -> bool {
    let has_live_midi_fx_notes = track_idx < MAX_TRACKS
        && track_idx < snapshot.tracks.len()
        && !live_tracks[track_idx].notes.is_empty()
        && !snapshot.tracks[track_idx].params.midi_fx_chain.is_empty()
        && snapshot.tracks[track_idx].params.midi_fx_position == MidiFxPosition::PostAccumulator
        && midi_fx_chain_clock_param(snapshot, midi_fx_descriptors, track_idx).is_some();
    if has_live_midi_fx_notes && debug_routing_enabled() {
        eprintln!(
            "[routing] live-midi-fx owns track={} notes={} chain={:?}",
            track_idx,
            live_tracks[track_idx].notes.len(),
            snapshot.tracks[track_idx].params.midi_fx_chain
        );
    }
    has_live_midi_fx_notes
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

fn schedule_live_midi_fx<const QUEUE_CAP: usize>(
    runtime: Option<&mut lisp_host::ScratchControlRuntime>,
    state: &SequencerState,
    snapshot: &SequencerSnapshot,
    queue: &ScheduledEventQueue<QUEUE_CAP>,
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
        if live_active && debug_routing_enabled() {
            eprintln!("[routing] skip live-midi-fx reason=no-scratch-runtime");
        }
        return live_active;
    };
    if snapshot.transport.bpm == 0 {
        return live_active;
    }
    let samples_per_quarter = sample_rate as f32 * 60.0 / snapshot.transport.bpm as f32;
    let horizon = rendered_sample.saturating_add(lookahead_samples);
    let midi_fx_descriptors = runtime.midi_fx_descriptors();
    let mut track_output_events = Vec::new();

    for track_idx in 0..snapshot.tracks.len().min(MAX_TRACKS) {
        if live_tracks[track_idx].notes.is_empty()
            || snapshot.tracks[track_idx].params.midi_fx_chain.is_empty()
            || snapshot.tracks[track_idx].params.midi_fx_position != MidiFxPosition::PostAccumulator
        {
            continue;
        }
        let num_steps = snapshot.tracks[track_idx].params.num_steps.max(1);
        let step = (state.transport.track_playheads[track_idx].load(Ordering::Relaxed) as usize)
            % num_steps;
        let Some(live_tick_beats) =
            midi_fx_clock_tick_beats(snapshot, &midi_fx_descriptors, track_idx, step)
        else {
            let pending_notes = live_tracks[track_idx]
                .notes
                .iter_mut()
                .filter_map(|note| {
                    if note.pending_event {
                        note.pending_event = false;
                        Some(*note)
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>();
            if pending_notes.is_empty() {
                continue;
            }
            let step_beats = snapshot.tracks[track_idx].steps[step]
                .timebase_override
                .unwrap_or(snapshot.tracks[track_idx].params.timebase)
                .step_beats(num_steps) as f32;
            let step_beats = step_beats.max(1.0 / 1024.0);
            let samples_per_step = (samples_per_quarter * step_beats).round().max(1.0);
            let chord = pending_notes
                .iter()
                .map(|note| note.transpose)
                .collect::<Vec<_>>();
            let chord_durations = vec![1.0; chord.len()];
            let chord_delays = vec![0.0; chord.len()];
            let note_spans = pending_notes
                .iter()
                .map(|note| AccumulatorNoteSpan {
                    transpose: note.transpose,
                    start_beats: 0.0,
                    end_beats: step_beats,
                })
                .collect::<Vec<_>>();
            let velocity = pending_notes
                .iter()
                .map(|note| note.velocity)
                .fold(0.0_f32, f32::max)
                .clamp(0.0, 1.0);
            let resolved = ResolvedStep {
                duration: 1.0,
                velocity,
                speed: 1.0,
                aux_a: 0.0,
                aux_b: 0.0,
                transpose: chord[0],
                pan: 0.0,
                chop: 1.0,
            };
            let event = MidiFxEvent {
                offset_beats: 0.0,
                track: track_idx,
                step,
                samples_per_step,
                step_beats,
                resolved,
                chord,
                chord_durations,
                chord_delays,
                chord_step_transpose: 0.0,
                note_spans: Some(note_spans),
                arp_phase_beats: rendered_total_beats as f32,
                midi_fx_params: Vec::new(),
                effect_params: resolve_effect_params(snapshot, track_idx, step),
                instrument_params: resolve_instrument_params(snapshot, track_idx, step),
                instrument_tensor_params: resolve_instrument_tensor_params(
                    snapshot, track_idx, step,
                ),
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
                None,
                0,
                debug_accum,
            );
            if !enqueue_midi_fx_events(
                queue,
                snapshot,
                &mut track_output_events,
                pattern_epoch,
                rendered_sample,
                rendered_total_beats,
                samples_per_quarter,
                0.0,
                events,
            ) {
                break;
            }
            continue;
        };
        for note in &mut live_tracks[track_idx].notes {
            note.pending_event = false;
        }
        if live_tracks[track_idx].next_tick_sample < rendered_sample {
            live_tracks[track_idx].next_tick_sample = rendered_sample;
        }
        while live_tracks[track_idx].next_tick_sample < horizon {
            let notes = live_tracks[track_idx].notes.clone();
            if notes.is_empty() {
                break;
            }
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
                midi_fx_params: Vec::new(),
                effect_params: resolve_effect_params(snapshot, track_idx, step),
                instrument_params: resolve_instrument_params(snapshot, track_idx, step),
                instrument_tensor_params: resolve_instrument_tensor_params(
                    snapshot, track_idx, step,
                ),
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
                None,
                0,
                debug_accum,
            );
            if !enqueue_midi_fx_events(
                queue,
                snapshot,
                &mut track_output_events,
                pattern_epoch,
                live_tracks[track_idx].next_tick_sample,
                rendered_total_beats + tick_offset_beats,
                samples_per_quarter,
                0.0,
                events,
            ) {
                break;
            }
            live_tracks[track_idx].next_tick_sample = live_tracks[track_idx]
                .next_tick_sample
                .saturating_add(live_tick_samples);
        }
    }

    state.set_track_output_current_beat(rendered_total_beats);
    state.append_track_output_events(track_output_events);
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

struct SchedulerLookaheadState {
    clock: SnapshotSequencerClock,
    accumulator_states: [AccumulatorRuntimeState; MAX_TRACKS],
    pending_accum_reset: [bool; MAX_TRACKS],
    midi_fx_quantizer_state: MidiFxQuantizerState,
    neural_runtime: NeuralRuntime,
    generator_runtime: crate::generator::GeneratorRuntime,
    process_runtime: crate::process::ProcessRuntime,
    resolved_read_pattern_epoch: Option<u64>,
    graph_manifests: Vec<crate::graph::GraphManifest>,
    graph_runtimes: Vec<crate::graph::GraphRuntime>,
    debug_graph_drive_chunks: u32,
    debug_accum_invocations: u64,
}

impl SchedulerLookaheadState {
    fn new(sample_rate: u32) -> Self {
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
        }
    }
}

fn build_scheduler_scratch_runtime(
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
struct SchedulerLookaheadResult {
    scheduled_until_sample: u64,
}

fn schedule_playing_lookahead<const QUEUE_CAP: usize>(
    scheduler: &mut SchedulerLookaheadState,
    state: &Arc<SequencerState>,
    snapshot: &SequencerSnapshot,
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
    let mut debug_graph_drive_chunks = scheduler.debug_graph_drive_chunks;
    let mut debug_accum_invocations = scheduler.debug_accum_invocations;
    let mut track_output_events = Vec::new();

    let resolved_read_bases = vec![
        std::array::from_fn(|index| StepParam::ALL[index].default_value());
        snapshot.tracks.len()
    ];
    if scheduler.resolved_read_pattern_epoch != Some(pattern_epoch) {
        process_runtime.reset_resolved_track_history(&resolved_read_bases);
        scheduler.resolved_read_pattern_epoch = Some(pattern_epoch);
    } else {
        process_runtime.ensure_resolved_track_bases(&resolved_read_bases);
    }

    let midi_fx_descriptors_for_scheduling = scratch_runtime
        .as_ref()
        .map(|runtime| runtime.midi_fx_descriptors())
        .unwrap_or_default();

    while scheduled_until_sample < rendered.saturating_add(lookahead_target_samples) {
        let chunk_start_beats = clock.total_beats;
        let triggers = clock.process_chunk(scheduler_block_size, snapshot, state);
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
                            )
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
                        )
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
                                    crate::process::resolved_values_from_step(
                                        output.resolved,
                                        &step_snapshot.params,
                                    ),
                                );
                                let mut event_effect_params = output.effect_params.clone();
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
                        let chord = step_chord_data(snapshot, target_track, trigger.step);
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
                    let chord = step_chord_data(snapshot, target_track, trigger.step);
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
                let invocations = process_runtime.process_block(
                    chunk_start_beats,
                    chunk_end_beats,
                    scheduled_until_sample,
                    samples_per_quarter,
                );
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

        scheduled_until_sample = scheduled_until_sample.saturating_add(scheduler_block_size as u64);
    }

    scheduler.debug_graph_drive_chunks = debug_graph_drive_chunks;
    scheduler.debug_accum_invocations = debug_accum_invocations;
    state.set_track_output_current_beat(scheduler.clock.total_beats);
    state.append_track_output_events(track_output_events);
    SchedulerLookaheadResult {
        scheduled_until_sample,
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
                } else if last_pattern_epoch != pattern_epoch {
                    // Track topology edits bump pattern_epoch without changing the
                    // pattern index. Rebuild the scheduler horizon immediately so
                    // future triggers target the compacted live track layout.
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
                } else if last_pattern != pattern {
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

#[cfg(test)]
mod tests {
    use super::{
        apply_fit_to_scale_to_trigger, apply_neuron_output_overrides, delayed_step_sample_time,
        enqueue_resolved_trigger, enqueue_step_event_with_midi_fx, midi_fx_window_events_from_step,
        quantized_live_tick_sample, reconcile_graph_runtimes, resolve_effect_params,
        resolve_instrument_plocks, resolve_sampler_params, run_midi_fx_chain_for_track,
        schedule_playing_lookahead, should_reload_neural_runtime, swung_network_sample_time,
        track_active_note_spans_at_beat, track_note_spans_for_trigger, EmittedNetworkEventSource,
        LiveMidiFxTrackState, MidiFxEvent, MidiFxQuantizerState, SchedulerLookaheadState,
        SnapshotSequencerClock,
    };
    use crate::accumulator::ResolvedStep;
    use crate::effects::{
        EffectDescriptor, ParamDescriptor, ParamKind, ParamScaling, TensorParamDescriptor,
    };
    use crate::graph::{
        EdgeSetSpec, GraphDurationSpec, GraphEdge, GraphEmission, GraphManifest, GraphNode,
        GraphPayload, GraphRuntime, NodeEval, NodeFire, NodeProto, ParamSpec,
        ProjectGraphEdgeParamOverride, ProjectGraphNodeIntrinsicOverride,
        ProjectGraphNodeParamOverride, ProjectGraphOverrides, ProjectGraphRouteOverride,
        ProjectGraphSeedFrom, SeedFrom, ShapeSpec, Topology,
    };
    use crate::lisp_host;
    use crate::neural::{
        NeuralMaxPolySelection, NeuralOutput, ParamNodeId, ProjectEffectParamOverride,
        ProjectNeuralNetwork, ProjectNeuron, ProjectParamOverride,
    };
    use crate::scheduled_event::{
        resolved_chord_transpose, EventSource, ScheduledChordData, ScheduledEffectParam,
        ScheduledEventKind, ScheduledEventQueue, ScheduledInstrumentParam,
        ScheduledInstrumentParamTarget, ScheduledInstrumentParams, ScheduledInstrumentTensorParams,
        ScheduledSamplerParams, StepEvent,
    };
    use crate::sequencer::{
        default_empty_effect_chain, SequencerState, StepParam, SwingResolution, Timebase,
        MAX_TRACKS,
    };
    use eseqlisp::vm::Value;
    use eseqlisp::Runtime;
    use std::sync::atomic::Ordering;
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

    fn graph_emission(
        sample_time: u64,
        node_index: usize,
        track: Option<usize>,
        transpose: f32,
        velocity: f32,
    ) -> GraphEmission {
        let mut resolved = test_resolved_step();
        resolved.transpose = transpose;
        resolved.velocity = velocity;
        GraphEmission {
            sample_time,
            node_index,
            event: lisp_host::EmittedAccumulatorEvent {
                offset_beats: 0.0,
                track,
                resolved,
                chord: Vec::new(),
                chord_durations: Vec::new(),
                chord_step_transpose: 0.0,
                effect_params: Vec::new(),
                instrument_params: Vec::new(),
            },
        }
    }

    fn generator_emission(
        sample_time: u64,
        generator_index: usize,
        track: Option<usize>,
        transpose: f32,
        velocity: f32,
    ) -> crate::generator::GeneratorEmission {
        let mut resolved = test_resolved_step();
        resolved.transpose = transpose;
        resolved.velocity = velocity;
        crate::generator::GeneratorEmission {
            sample_time,
            generator_index,
            event: lisp_host::EmittedAccumulatorEvent {
                offset_beats: 0.0,
                track,
                resolved,
                chord: Vec::new(),
                chord_durations: Vec::new(),
                chord_step_transpose: 0.0,
                effect_params: Vec::new(),
                instrument_params: Vec::new(),
            },
        }
    }

    fn neural_output(
        sample_time: u64,
        track: usize,
        neuron: usize,
        transpose: f32,
        velocity: f32,
    ) -> NeuralOutput {
        let mut resolved = test_resolved_step();
        resolved.transpose = transpose;
        resolved.velocity = velocity;
        NeuralOutput {
            sample_time,
            event: StepEvent {
                track,
                samples_per_step: 12_000.0,
                resolved,
                chord: ScheduledChordData {
                    count: 0,
                    notes: [0.0; crate::voice::MAX_VOICES],
                    durations: [0.0; crate::voice::MAX_VOICES],
                    delays: [0.0; crate::voice::MAX_VOICES],
                    step_transpose: 0.0,
                },
                effect_params: Vec::new(),
                instrument_params: ScheduledInstrumentParams::new(),
                instrument_tensor_params: ScheduledInstrumentTensorParams::new(),
                sampler_params: ScheduledSamplerParams::default(),
                source: EventSource::Network {
                    seed: Some((0, 0)),
                    neuron,
                    instrument_fingerprint: 0,
                },
            },
            emit_trigger: true,
        }
    }

    #[test]
    fn neural_accent_merge_keeps_coincident_distinct_notes_polyphonic() {
        let merged = super::merge_neural_output_accents(vec![
            neural_output(1_000, 2, 0, 0.0, 0.5),
            neural_output(1_000, 2, 1, 7.0, 0.25),
        ]);

        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].event.resolved.transpose, 0.0);
        assert_eq!(merged[1].event.resolved.transpose, 7.0);
    }

    #[test]
    fn generator_accent_merge_keeps_coincident_distinct_notes_polyphonic() {
        let merged = super::merge_generator_emission_accents(vec![
            generator_emission(1_000, 0, Some(2), 0.0, 0.5),
            generator_emission(1_000, 1, Some(2), 7.0, 0.25),
        ]);

        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].event.resolved.transpose, 0.0);
        assert_eq!(merged[1].event.resolved.transpose, 7.0);
    }

    #[test]
    fn graph_accent_merge_keeps_coincident_distinct_notes_polyphonic() {
        let merged = super::merge_graph_emission_accents(vec![
            graph_emission(1_000, 0, Some(2), 0.0, 0.5),
            graph_emission(1_000, 1, Some(2), 7.0, 0.25),
        ]);

        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].event.resolved.transpose, 0.0);
        assert_eq!(merged[0].event.resolved.velocity, 0.5);
        assert_eq!(merged[1].event.resolved.transpose, 7.0);
        assert_eq!(merged[1].event.resolved.velocity, 0.25);
    }

    #[test]
    fn graph_accent_merge_sums_only_matching_notes() {
        let merged = super::merge_graph_emission_accents(vec![
            graph_emission(1_000, 0, Some(2), 0.0, 0.5),
            graph_emission(1_000, 1, Some(2), 7.0, 0.25),
            graph_emission(1_000, 2, Some(2), 0.0, 0.75),
        ]);

        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].event.resolved.transpose, 0.0);
        assert_eq!(merged[0].event.resolved.velocity, 1.0);
        assert_eq!(merged[1].event.resolved.transpose, 7.0);
        assert_eq!(merged[1].event.resolved.velocity, 0.25);
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
            duration: crate::graph::GraphDurationSpec::default(),
            swing: crate::graph::GraphSwingSpec::default(),
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
                distribution: crate::graph::EdgeDistribution::BroadcastWeighted,
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
                seed_on_reset: None,
                duration: None,
                swing: None,
            }],
            node_params: Vec::new(),
            edge_params: Vec::new(),
            reset_every_beats: None,
            max_poly: None,
            max_poly_selection: None,
            node_count: None,
        }
    }

    #[test]
    fn graph_seed_duration_uses_source_step_duration_and_step_size() {
        let mut source = GraphNode::default();
        source.seed_track_mask = crate::graph::seed_track_mask(&[0]);
        let target = GraphNode {
            duration: GraphDurationSpec::Seed,
            ..GraphNode::default()
        };
        let graph = GraphRuntime::new(
            1,
            "g".into(),
            vec![source, target],
            vec![GraphEdge::new(0, 1, 1.0)],
            1.0,
            0.0,
        );
        let mut graphs = vec![graph];
        let event = StepEvent {
            track: 0,
            samples_per_step: 24_000.0,
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
            instrument_tensor_params: ScheduledInstrumentTensorParams::new(),
            sampler_params: ScheduledSamplerParams::default(),
            source: EventSource::Step {
                track: 0,
                step: 0,
                instrument_fingerprint: 0,
            },
        };

        super::seed_graph_runtimes(&mut graphs, &event, 0.0, 48_000.0);
        let mut out = Vec::new();
        graphs[0].process_block(
            0.0,
            1.0,
            0,
            48_000.0,
            0,
            |eval| NodeFire {
                fired: eval.input > 0.0,
                ..NodeFire::default()
            },
            &mut out,
        );

        assert_eq!(out.len(), 1);
        assert_eq!(out[0].node_index, 1);
        assert_eq!(out[0].event.resolved.duration, 0.5);
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
    fn graph_node_count_override_rebuilds_runtime_and_preserves_overrides() {
        let mut manifests = Vec::new();
        let mut runtimes = Vec::new();
        let manifest = graph_manifest(
            1,
            "g",
            ShapeSpec::VariableLine {
                default: 8,
                min: 1,
                max: 16,
            },
        );
        reconcile_graph_runtimes(
            vec![manifest.clone()],
            &[],
            &mut runtimes,
            &mut manifests,
            0.0,
        );
        assert_eq!(runtimes[0].num_nodes(), 8);
        runtimes[0].seed(0, 0.0, GraphPayload::default());

        let overrides = vec![ProjectGraphOverrides {
            sequencer_id: 1,
            sequencer_name: "g".into(),
            node_count: Some(12),
            node_params: vec![ProjectGraphNodeParamOverride {
                group: "n".into(),
                instance: 14,
                param: "threshold".into(),
                value: 0.25,
            }],
            edge_params: vec![ProjectGraphEdgeParamOverride {
                group: "n->n".into(),
                from: 14,
                to: 3,
                param: "weight".into(),
                value: 0.5,
            }],
            ..ProjectGraphOverrides::default()
        }];
        reconcile_graph_runtimes(
            vec![manifest.clone()],
            &overrides,
            &mut runtimes,
            &mut manifests,
            0.0,
        );

        assert_eq!(runtimes[0].num_nodes(), 12);
        let out = process_graph(&mut runtimes[0], 0.0, 1.0);
        assert!(
            out.is_empty(),
            "node-count change must clear pending seed state"
        );
        assert_eq!(overrides[0].node_count, Some(12));
        assert_eq!(overrides[0].node_params[0].instance, 14);
        assert_eq!(overrides[0].edge_params[0].from, 14);

        let shrunk = manifest.runtime_config_with_overrides(Some(&overrides[0]));
        assert_eq!(
            shrunk.nodes.len(),
            12,
            "storage remains dormant until node_count grows"
        );
        let mut restored_overrides = overrides[0].clone();
        restored_overrides.node_count = Some(16);
        let restored = manifest.runtime_config_with_overrides(Some(&restored_overrides));
        assert_eq!(restored.nodes.len(), 16);
        assert_eq!(restored.node_params[14]["threshold"], 0.25);
        assert_eq!(
            restored
                .edges
                .iter()
                .find(|edge| edge.from == 14 && edge.to == 3)
                .expect("restored dormant edge")
                .weight,
            0.5
        );
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
    fn snapshot_clock_suppresses_triggers_for_scene_silenced_tracks() {
        let state = SequencerState::new(1, vec![default_empty_effect_chain()]);
        state.toggle_play();
        state.toggle_step_and_clear_plocks(0, 0);
        let mut snapshot = (*state.latest_scheduler_snapshot()).clone();
        snapshot.tracks[0].scene_silenced = true;
        let mut clock = SnapshotSequencerClock::new(48_000);

        let triggers = clock.process_chunk(12_000, &snapshot, &state);

        assert!(triggers.is_empty());
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
        let mut track_output_events = Vec::new();

        assert!(enqueue_resolved_trigger(
            &queue,
            &snapshot,
            &mut track_output_events,
            0,
            1_000,
            0.0,
            48_000.0,
            0.0,
            0,
            0,
            6_000.0,
            resolved,
            chord,
            Vec::new(),
            ScheduledInstrumentParams::new(),
            ScheduledInstrumentTensorParams::new(),
            ScheduledSamplerParams::default(),
        ));

        let first = queue.pop().expect("first note event");
        let second = queue.pop().expect("second note event");
        assert_eq!(first.sample_time, 1_000);
        assert_eq!(second.sample_time, 4_000);
        assert_eq!(track_output_events.len(), 2);
        assert_eq!(track_output_events[0].beat, 0.0);
        assert_eq!(track_output_events[1].beat, 0.0625);
        assert!(queue.pop().is_none());
    }

    #[test]
    fn enqueue_resolved_trigger_applies_global_transpose_for_opted_in_track() {
        let state = SequencerState::new(1, vec![default_empty_effect_chain()]);
        let snapshot = state.publish_scheduler_snapshot();
        let queue = ScheduledEventQueue::<8>::new();
        let mut track_output_events = Vec::new();

        assert!(enqueue_resolved_trigger(
            &queue,
            &snapshot,
            &mut track_output_events,
            0,
            1_000,
            0.0,
            48_000.0,
            5.0,
            0,
            0,
            6_000.0,
            test_resolved_step(),
            ScheduledChordData {
                count: 0,
                notes: [0.0; crate::voice::MAX_VOICES],
                durations: [0.0; crate::voice::MAX_VOICES],
                delays: [0.0; crate::voice::MAX_VOICES],
                step_transpose: 0.0,
            },
            Vec::new(),
            ScheduledInstrumentParams::new(),
            ScheduledInstrumentTensorParams::new(),
            ScheduledSamplerParams::default(),
        ));

        let event = queue.pop().expect("global-transposed event");
        match event.kind {
            ScheduledEventKind::ResolvedTrigger { resolved, .. } => {
                assert_eq!(resolved.transpose, 5.0);
            }
            other => panic!("unexpected event kind: {other:?}"),
        }
        assert_eq!(track_output_events[0].transpose, 5.0);
    }

    #[test]
    fn enqueue_resolved_trigger_respects_global_transpose_opt_out() {
        let state = SequencerState::new(1, vec![default_empty_effect_chain()]);
        state.pattern.track_params[0].set_global_transpose(false);
        let snapshot = state.publish_scheduler_snapshot();
        let queue = ScheduledEventQueue::<8>::new();
        let mut track_output_events = Vec::new();

        assert!(enqueue_resolved_trigger(
            &queue,
            &snapshot,
            &mut track_output_events,
            0,
            1_000,
            0.0,
            48_000.0,
            5.0,
            0,
            0,
            6_000.0,
            test_resolved_step(),
            ScheduledChordData {
                count: 0,
                notes: [0.0; crate::voice::MAX_VOICES],
                durations: [0.0; crate::voice::MAX_VOICES],
                delays: [0.0; crate::voice::MAX_VOICES],
                step_transpose: 0.0,
            },
            Vec::new(),
            ScheduledInstrumentParams::new(),
            ScheduledInstrumentTensorParams::new(),
            ScheduledSamplerParams::default(),
        ));

        let event = queue.pop().expect("opted-out event");
        match event.kind {
            ScheduledEventKind::ResolvedTrigger { resolved, .. } => {
                assert_eq!(resolved.transpose, 0.0);
            }
            other => panic!("unexpected event kind: {other:?}"),
        }
        assert_eq!(track_output_events[0].transpose, 0.0);
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
            instrument_tensor_params: ScheduledInstrumentTensorParams::new(),
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
        let mut runtime = lisp_host::ScratchControlRuntime::new(
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
            instrument_tensor_params: ScheduledInstrumentTensorParams::new(),
            sampler_params: ScheduledSamplerParams::default(),
            source: EventSource::Network {
                seed: None,
                neuron: 0,
                instrument_fingerprint: 0,
            },
        };
        let mut track_output_events = Vec::new();

        assert!(enqueue_step_event_with_midi_fx(
            &queue,
            &snapshot,
            &mut track_output_events,
            Some(&mut runtime),
            None,
            0,
            1_000,
            0.0,
            48_000.0,
            0.0,
            0.0,
            event,
            Vec::new(),
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
        assert_eq!(track_output_events.len(), 1);
        assert_eq!(track_output_events[0].track, 0);
        assert_eq!(track_output_events[0].transpose, 12.0);
        assert!(queue.pop().is_none());
    }

    #[test]
    fn emitted_network_event_runs_midi_fx_and_keeps_target_instrument_defaults() {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        state.pattern.track_params[0].set_midi_fx_chain(vec!["octave".to_string()]);
        state.pattern.instrument_slots[0]
            .apply_descriptor(&EffectDescriptor::builtin_sampler(), 77);
        state.pattern.instrument_slots[0].defaults.set(12, 2.5);
        let snapshot = state.publish_scheduler_snapshot();
        let mut runtime = lisp_host::ScratchControlRuntime::new(
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
        let mut track_output_events = Vec::new();

        assert!(super::enqueue_emitted_network_event_with_midi_fx(
            &queue,
            &snapshot,
            &mut track_output_events,
            Some(&mut runtime),
            None,
            0,
            1_000,
            48_000.0,
            0.0,
            0.0,
            EmittedNetworkEventSource::Generator { index: 0 },
            lisp_host::EmittedAccumulatorEvent {
                offset_beats: 0.0,
                track: Some(0),
                resolved: test_resolved_step(),
                chord: Vec::new(),
                chord_durations: Vec::new(),
                chord_step_transpose: 0.0,
                effect_params: Vec::new(),
                instrument_params: Vec::new(),
            },
            false,
        ));

        let scheduled = queue.pop().expect("MIDI FX output event");
        match scheduled.kind {
            ScheduledEventKind::NetworkTrigger {
                track,
                resolved,
                instrument_params,
                sampler_params,
                ..
            } => {
                assert_eq!(track, 0);
                assert_eq!(resolved.transpose, 12.0);
                assert_eq!(sampler_params.playback_speed, 2.5);
                assert!(instrument_params.iter().any(|param| {
                    param.target == ScheduledInstrumentParamTarget::Synth
                        && param.idx == crate::sampler::PARAM_SPEED
                        && param.value == 2.5
                }));
            }
            other => panic!("expected network trigger, got {other:?}"),
        }
        assert!(queue.pop().is_none());
        assert_eq!(track_output_events.len(), 1);
        assert_eq!(track_output_events[0].transpose, 12.0);
    }

    #[test]
    fn emitted_network_event_trigger_to_track_copies_to_selected_target_track() {
        let state = Arc::new(SequencerState::new(
            5,
            vec![
                default_empty_effect_chain(),
                default_empty_effect_chain(),
                default_empty_effect_chain(),
                default_empty_effect_chain(),
                default_empty_effect_chain(),
            ],
        ));
        state.pattern.track_params[0].set_midi_fx_chain(vec!["trigger-to-track".to_string()]);
        let trigger_desc = lisp_host::load_midi_fx_descriptor("trigger-to-track")
            .expect("trigger-to-track descriptor");
        state.pattern.midi_fx_slots[0][0].apply_descriptor(&trigger_desc, 0);
        state.pattern.midi_fx_slots[0][0].defaults.set(0, 5.0);
        let snapshot = state.publish_scheduler_snapshot();
        let mut runtime = lisp_host::ScratchControlRuntime::new(
            Arc::clone(&state),
            vec![Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new()],
            vec![
                EffectDescriptor::builtin_sampler(),
                EffectDescriptor::builtin_sampler(),
                EffectDescriptor::builtin_sampler(),
                EffectDescriptor::builtin_sampler(),
                EffectDescriptor::builtin_sampler(),
            ],
            0,
            0,
        );
        runtime
            .eval(&lisp_host::load_midi_fx_library_source())
            .unwrap();
        let queue = ScheduledEventQueue::<8>::new();
        let mut track_output_events = Vec::new();

        assert!(super::enqueue_emitted_network_event_with_midi_fx(
            &queue,
            &snapshot,
            &mut track_output_events,
            Some(&mut runtime),
            None,
            0,
            1_000,
            48_000.0,
            0.0,
            0.0,
            EmittedNetworkEventSource::Generator { index: 0 },
            lisp_host::EmittedAccumulatorEvent {
                offset_beats: 0.0,
                track: Some(0),
                resolved: ResolvedStep {
                    transpose: 7.0,
                    ..test_resolved_step()
                },
                chord: Vec::new(),
                chord_durations: Vec::new(),
                chord_step_transpose: 0.0,
                effect_params: Vec::new(),
                instrument_params: Vec::new(),
            },
            false,
        ));

        let mut events = Vec::new();
        while let Some(event) = queue.pop() {
            events.push(event);
        }
        assert_eq!(
            events.len(),
            2,
            "expected source plus copied target trigger"
        );
        let mut tracks_and_transposes = events
            .into_iter()
            .map(|scheduled| {
                assert_eq!(scheduled.sample_time, 1_000);
                match scheduled.kind {
                    ScheduledEventKind::NetworkTrigger {
                        track, resolved, ..
                    } => (track, resolved.transpose),
                    other => panic!("expected network trigger, got {other:?}"),
                }
            })
            .collect::<Vec<_>>();
        tracks_and_transposes.sort_by_key(|(track, _)| *track);
        assert_eq!(tracks_and_transposes, vec![(0, 7.0), (4, 7.0)]);
        let mut telemetry_tracks = track_output_events
            .iter()
            .map(|event| event.track)
            .collect::<Vec<_>>();
        telemetry_tracks.sort_unstable();
        assert_eq!(telemetry_tracks, vec![0, 4]);
    }

    #[test]
    fn quantizer_midi_fx_holds_until_next_grid_and_keeps_highest_velocity() {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        state.pattern.track_params[0].set_midi_fx_chain(vec!["quantizer".to_string()]);

        let mut runtime = lisp_host::ScratchControlRuntime::new(
            Arc::clone(&state),
            vec![Vec::new()],
            vec![EffectDescriptor::builtin_sampler()],
            0,
            0,
        );
        runtime
            .eval(&lisp_host::load_midi_fx_library_source())
            .unwrap();
        let quantizer_desc = runtime
            .midi_fx_descriptors()
            .into_iter()
            .find(|desc| desc.name == "quantizer")
            .expect("quantizer descriptor");
        assert_eq!(
            quantizer_desc.params[0]
                .ui_metadata
                .as_ref()
                .and_then(|metadata| metadata.role.as_deref()),
            Some("quantize-grid")
        );
        state.pattern.midi_fx_slots[0][0].apply_descriptor(&quantizer_desc, 0);
        state.pattern.midi_fx_slots[0][0].defaults.set(0, 2.0);
        let snapshot = state.publish_scheduler_snapshot();

        let event = |beat: f32, velocity: f32, transpose: f32| MidiFxEvent {
            offset_beats: 0.0,
            track: 0,
            step: 0,
            samples_per_step: 12_000.0,
            step_beats: 0.25,
            resolved: ResolvedStep {
                duration: 1.0,
                velocity,
                speed: 1.0,
                aux_a: 0.0,
                aux_b: 0.0,
                transpose,
                pan: 0.0,
                chop: 1.0,
            },
            chord: vec![transpose],
            chord_durations: vec![1.0],
            chord_delays: vec![0.0],
            chord_step_transpose: 0.0,
            note_spans: None,
            arp_phase_beats: beat,
            midi_fx_params: Vec::new(),
            effect_params: Vec::new(),
            instrument_params: ScheduledInstrumentParams::new(),
            instrument_tensor_params: ScheduledInstrumentTensorParams::new(),
            sampler_params: resolve_sampler_params(&snapshot, 0, 0),
            source: EventSource::Step {
                track: 0,
                step: 0,
                instrument_fingerprint: 0,
            },
        };

        let mut quantizer_state = MidiFxQuantizerState::default();
        assert!(run_midi_fx_chain_for_track(
            &mut runtime,
            &snapshot,
            0,
            vec![event(0.10, 0.3, 1.0)],
            Some(&mut quantizer_state),
            0,
            false,
        )
        .is_empty());
        assert!(run_midi_fx_chain_for_track(
            &mut runtime,
            &snapshot,
            0,
            vec![event(0.25, 0.8, 7.0)],
            Some(&mut quantizer_state),
            0,
            false,
        )
        .is_empty());
        assert!(run_midi_fx_chain_for_track(
            &mut runtime,
            &snapshot,
            0,
            vec![event(0.40, 0.6, 12.0)],
            Some(&mut quantizer_state),
            0,
            false,
        )
        .is_empty());

        let due = quantizer_state.drain_due(1.0);
        assert_eq!(due.len(), 1);
        assert!((due[0].deadline_beats - 1.0).abs() < 1e-9);
        assert_eq!(due[0].event.resolved.transpose, 7.0);
        assert!((due[0].event.resolved.velocity - 0.8).abs() < 1e-6);
    }

    #[test]
    fn scheduler_lookahead_quantizer_keeps_first_on_grid_trigger() {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        state.pattern.track_params[0].set_midi_fx_chain(vec!["quantizer".to_string()]);

        let mut runtime = lisp_host::ScratchControlRuntime::new(
            Arc::clone(&state),
            vec![Vec::new()],
            vec![EffectDescriptor::builtin_sampler()],
            0,
            0,
        );
        runtime
            .eval(&lisp_host::load_midi_fx_library_source())
            .unwrap();
        let quantizer_desc = runtime
            .midi_fx_descriptors()
            .into_iter()
            .find(|desc| desc.name == "quantizer")
            .expect("quantizer descriptor");
        state.pattern.midi_fx_slots[0][0].apply_descriptor(&quantizer_desc, 0);
        state.pattern.midi_fx_slots[0][0].defaults.set(0, 2.0);

        state.pattern.patterns[0].set_step_active(0, true);
        state.pattern.step_data[0].set(0, StepParam::Velocity, 0.7);
        state.pattern.step_data[0].set(0, StepParam::Transpose, 5.0);

        state.transport.playing.store(true, Ordering::Relaxed);
        let snapshot = state.publish_scheduler_snapshot();
        let queue = ScheduledEventQueue::<16>::new();
        let mut scheduler = SchedulerLookaheadState::new(48_000);
        let live_midi_fx_tracks: [LiveMidiFxTrackState; MAX_TRACKS] =
            std::array::from_fn(|_| LiveMidiFxTrackState::default());
        let mut scratch_runtime = Some(runtime);

        schedule_playing_lookahead(
            &mut scheduler,
            &state,
            &snapshot,
            &queue,
            &mut scratch_runtime,
            &live_midi_fx_tracks,
            snapshot.transport.pattern_epoch,
            0,
            6_000,
            48_000,
            6_000,
            24_000.0,
            0,
            false,
            false,
        );

        let scheduled = queue.pop().expect("on-grid quantized event");
        let ScheduledEventKind::ResolvedTrigger {
            track, resolved, ..
        } = scheduled.kind
        else {
            panic!("expected resolved trigger");
        };
        assert_eq!(scheduled.sample_time, 0);
        assert_eq!(track, 0);
        assert_eq!(resolved.transpose, 5.0);
        assert!((resolved.velocity - 0.7).abs() < 1e-6);
    }

    #[test]
    fn scheduler_lookahead_flushes_quantizer_without_trigger_on_grid() {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        state.pattern.track_params[0].set_midi_fx_chain(vec!["quantizer".to_string()]);

        let mut runtime = lisp_host::ScratchControlRuntime::new(
            Arc::clone(&state),
            vec![Vec::new()],
            vec![EffectDescriptor::builtin_sampler()],
            0,
            0,
        );
        runtime
            .eval(&lisp_host::load_midi_fx_library_source())
            .unwrap();
        let quantizer_desc = runtime
            .midi_fx_descriptors()
            .into_iter()
            .find(|desc| desc.name == "quantizer")
            .expect("quantizer descriptor");
        state.pattern.midi_fx_slots[0][0].apply_descriptor(&quantizer_desc, 0);
        state.pattern.midi_fx_slots[0][0].defaults.set(0, 2.0);

        state.pattern.patterns[0].set_step_active(1, true);
        state.pattern.step_data[0].set(1, StepParam::Velocity, 0.9);
        state.pattern.step_data[0].set(1, StepParam::Transpose, 7.0);
        state.pattern.patterns[0].set_step_active(2, true);
        state.pattern.step_data[0].set(2, StepParam::Velocity, 0.4);
        state.pattern.step_data[0].set(2, StepParam::Transpose, 12.0);

        state.transport.playing.store(true, Ordering::Relaxed);
        let snapshot = state.publish_scheduler_snapshot();
        let queue = ScheduledEventQueue::<16>::new();
        let mut scheduler = SchedulerLookaheadState::new(48_000);
        let live_midi_fx_tracks: [LiveMidiFxTrackState; MAX_TRACKS] =
            std::array::from_fn(|_| LiveMidiFxTrackState::default());
        let mut scratch_runtime = Some(runtime);

        schedule_playing_lookahead(
            &mut scheduler,
            &state,
            &snapshot,
            &queue,
            &mut scratch_runtime,
            &live_midi_fx_tracks,
            snapshot.transport.pattern_epoch,
            0,
            36_000,
            48_000,
            6_000,
            24_000.0,
            0,
            false,
            false,
        );

        let scheduled = queue.pop().expect("quantized event");
        let ScheduledEventKind::ResolvedTrigger {
            track, resolved, ..
        } = scheduled.kind
        else {
            panic!("expected resolved trigger");
        };
        assert_eq!(scheduled.sample_time, 24_000);
        assert_eq!(track, 0);
        assert_eq!(resolved.transpose, 7.0);
        assert!((resolved.velocity - 0.9).abs() < 1e-6);
        assert!(queue.pop().is_none());
    }

    #[test]
    fn graph_runtime_emission_runs_target_track_midi_fx_chain() {
        let state = Arc::new(SequencerState::new(
            5,
            vec![
                default_empty_effect_chain(),
                default_empty_effect_chain(),
                default_empty_effect_chain(),
                default_empty_effect_chain(),
                default_empty_effect_chain(),
            ],
        ));
        state.pattern.track_params[1].set_midi_fx_chain(vec!["trigger-to-track".to_string()]);
        let trigger_desc = lisp_host::load_midi_fx_descriptor("trigger-to-track")
            .expect("trigger-to-track descriptor");
        state.pattern.midi_fx_slots[1][0].apply_descriptor(&trigger_desc, 0);
        state.pattern.midi_fx_slots[1][0].defaults.set(0, 5.0);
        let snapshot = state.publish_scheduler_snapshot();

        let mut n0 = GraphNode::default();
        n0.seed_track_mask = 1 << 0;
        n0.route = Some(1);
        let mut graph_runtime = GraphRuntime::new(
            1,
            "g".to_string(),
            vec![n0],
            vec![GraphEdge::new(0, 0, 1.0)],
            1.0,
            0.0,
        );
        graph_runtime.seed(
            0,
            0.0,
            GraphPayload {
                note: 7.0,
                velocity: 0.9,
                duration_beats: 0.25,
            },
        );
        let mut graph_emissions = Vec::new();
        graph_runtime.process_block(
            0.0,
            1.0,
            1_000,
            48_000.0,
            0,
            |_eval| NodeFire {
                fired: true,
                ..NodeFire::default()
            },
            &mut graph_emissions,
        );
        assert!(!graph_emissions.is_empty());
        assert_eq!(graph_emissions[0].event.track, Some(1));

        let mut runtime = lisp_host::ScratchControlRuntime::new(
            Arc::clone(&state),
            vec![Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new()],
            vec![
                EffectDescriptor::builtin_sampler(),
                EffectDescriptor::builtin_sampler(),
                EffectDescriptor::builtin_sampler(),
                EffectDescriptor::builtin_sampler(),
                EffectDescriptor::builtin_sampler(),
            ],
            0,
            0,
        );
        runtime
            .eval(&lisp_host::load_midi_fx_library_source())
            .unwrap();
        let queue = ScheduledEventQueue::<8>::new();
        let mut track_output_events = Vec::new();

        assert!(super::enqueue_emitted_network_event_with_midi_fx(
            &queue,
            &snapshot,
            &mut track_output_events,
            Some(&mut runtime),
            None,
            0,
            graph_emissions[0].sample_time,
            48_000.0,
            1.0,
            0.0,
            EmittedNetworkEventSource::Graph {
                graph_index: 0,
                node_index: graph_emissions[0].node_index,
            },
            graph_emissions.remove(0).event,
            false,
        ));

        let mut tracks_and_transposes = Vec::new();
        while let Some(event) = queue.pop() {
            match event.kind {
                ScheduledEventKind::NetworkTrigger {
                    track, resolved, ..
                } => tracks_and_transposes.push((track, resolved.transpose)),
                other => panic!("expected network trigger, got {other:?}"),
            }
        }
        tracks_and_transposes.sort_by_key(|(track, _)| *track);
        assert_eq!(tracks_and_transposes, vec![(1, 7.0), (4, 7.0)]);
        let mut telemetry_tracks = track_output_events
            .iter()
            .map(|event| event.track)
            .collect::<Vec<_>>();
        telemetry_tracks.sort_unstable();
        assert_eq!(telemetry_tracks, vec![1, 4]);
    }

    #[test]
    fn graph_route_off_emission_does_not_fall_back_to_track_zero() {
        let state = Arc::new(SequencerState::new(
            2,
            vec![default_empty_effect_chain(), default_empty_effect_chain()],
        ));
        state.pattern.track_params[0].set_midi_fx_chain(vec!["trigger-to-track".to_string()]);
        let trigger_desc = lisp_host::load_midi_fx_descriptor("trigger-to-track")
            .expect("trigger-to-track descriptor");
        state.pattern.midi_fx_slots[0][0].apply_descriptor(&trigger_desc, 0);
        state.pattern.midi_fx_slots[0][0].defaults.set(0, 2.0);
        let snapshot = state.publish_scheduler_snapshot();
        let queue = ScheduledEventQueue::<8>::new();
        let mut track_output_events = Vec::new();

        assert!(super::enqueue_emitted_network_event_with_midi_fx(
            &queue,
            &snapshot,
            &mut track_output_events,
            None,
            None,
            0,
            1_000,
            48_000.0,
            0.0,
            0.0,
            EmittedNetworkEventSource::Graph {
                graph_index: 0,
                node_index: 0,
            },
            lisp_host::EmittedAccumulatorEvent {
                offset_beats: 0.0,
                track: None,
                resolved: ResolvedStep {
                    transpose: 7.0,
                    ..test_resolved_step()
                },
                chord: Vec::new(),
                chord_durations: Vec::new(),
                chord_step_transpose: 0.0,
                effect_params: Vec::new(),
                instrument_params: Vec::new(),
            },
            false,
        ));

        assert!(
            queue.pop().is_none(),
            "graph route Off must not enqueue a source-track event or run source-track MIDI FX"
        );
        assert!(track_output_events.is_empty());
    }

    #[test]
    fn graph_runtime_emission_runs_arp_midi_fx_chain() {
        let state = Arc::new(SequencerState::new(
            2,
            vec![default_empty_effect_chain(), default_empty_effect_chain()],
        ));
        state.pattern.track_params[1].set_midi_fx_chain(vec!["arp".to_string()]);
        let arp_desc = lisp_host::load_midi_fx_descriptor("arp").expect("arp descriptor");
        state.pattern.midi_fx_slots[1][0].apply_descriptor(&arp_desc, 0);
        state.pattern.midi_fx_slots[1][0].defaults.set(0, 4.0);
        let snapshot = state.publish_scheduler_snapshot();

        let mut runtime = lisp_host::ScratchControlRuntime::new(
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
            .eval(&lisp_host::load_midi_fx_library_source())
            .unwrap();
        let queue = ScheduledEventQueue::<16>::new();
        let mut track_output_events = Vec::new();

        assert!(super::enqueue_emitted_network_event_with_midi_fx(
            &queue,
            &snapshot,
            &mut track_output_events,
            Some(&mut runtime),
            None,
            0,
            1_000,
            48_000.0,
            0.0,
            0.0,
            EmittedNetworkEventSource::Graph {
                graph_index: 0,
                node_index: 0,
            },
            lisp_host::EmittedAccumulatorEvent {
                offset_beats: 0.0,
                track: Some(1),
                resolved: ResolvedStep {
                    transpose: 7.0,
                    ..test_resolved_step()
                },
                chord: Vec::new(),
                chord_durations: Vec::new(),
                chord_step_transpose: 0.0,
                effect_params: Vec::new(),
                instrument_params: Vec::new(),
            },
            false,
        ));

        let mut scheduled = Vec::new();
        while let Some(event) = queue.pop() {
            match event.kind {
                ScheduledEventKind::NetworkTrigger {
                    track, resolved, ..
                } => scheduled.push((event.sample_time, track, resolved.transpose)),
                other => panic!("expected network trigger, got {other:?}"),
            }
        }
        assert_eq!(scheduled.len(), 4);
        assert!(scheduled
            .iter()
            .all(|(_, track, note)| *track == 1 && *note == 7.0));
        assert_eq!(
            scheduled
                .iter()
                .map(|(sample_time, _, _)| *sample_time)
                .collect::<Vec<_>>(),
            vec![1_000, 13_000, 25_000, 37_000]
        );
    }

    fn publish_test_graph_sequencer(state: Arc<SequencerState>, source: &str) {
        let mut authoring = Runtime::new();
        let publish_state = Arc::clone(&state);
        authoring.register_native("def-sequencer", move |args, _ctx| {
            let published = lisp_host::published_sequencer_from_def_args(&args)?;
            let name = published.name.clone();
            publish_state.publish_sequencer(published);
            Ok(Value::String(name))
        });
        authoring
            .eval_str(source)
            .expect("evaluate test graph sequencer");
    }

    #[test]
    fn scheduler_runtime_keeps_builtin_midi_fx_when_project_scratch_fails() {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        let runtime = super::build_scheduler_scratch_runtime(
            Arc::clone(&state),
            r#"(def-sequencer "graph-scratch" :shape (line 1))"#,
            false,
        )
        .expect("builtin MIDI FX should keep scheduler runtime alive");
        let names = runtime.midi_fx_names();
        assert!(
            names.iter().any(|name| name == "arp"),
            "scheduler runtime should keep builtin arp after scratch eval failure: {names:?}"
        );
        assert!(
            names.iter().any(|name| name == "trigger-to-track"),
            "scheduler runtime should keep builtin trigger-to-track after scratch eval failure: {names:?}"
        );
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum ScheduledTriggerKind {
        Step,
        Network,
    }

    #[derive(Clone, Debug, PartialEq)]
    struct ObservedTrigger {
        kind: ScheduledTriggerKind,
        track: usize,
        sample_time: u64,
        transpose: f32,
        duration: f32,
        sampler_speed: Option<f32>,
        has_speed_param: bool,
    }

    fn observed_triggers<const QUEUE_CAP: usize>(
        queue: &ScheduledEventQueue<QUEUE_CAP>,
    ) -> Vec<ObservedTrigger> {
        let mut out = Vec::new();
        while let Some(event) = queue.pop() {
            match event.kind {
                ScheduledEventKind::ResolvedTrigger {
                    track,
                    resolved,
                    instrument_params,
                    ..
                } => out.push(ObservedTrigger {
                    kind: ScheduledTriggerKind::Step,
                    track,
                    sample_time: event.sample_time,
                    transpose: resolved.transpose,
                    duration: resolved.duration,
                    sampler_speed: None,
                    has_speed_param: instrument_params.iter().any(|param| {
                        param.target == ScheduledInstrumentParamTarget::Synth
                            && param.idx == crate::sampler::PARAM_SPEED
                            && param.value == 2.5
                    }),
                }),
                ScheduledEventKind::NetworkTrigger {
                    track,
                    resolved,
                    instrument_params,
                    sampler_params,
                    ..
                } => out.push(ObservedTrigger {
                    kind: ScheduledTriggerKind::Network,
                    track,
                    sample_time: event.sample_time,
                    transpose: resolved.transpose,
                    duration: resolved.duration,
                    sampler_speed: Some(sampler_params.playback_speed),
                    has_speed_param: instrument_params.iter().any(|param| {
                        param.target == ScheduledInstrumentParamTarget::Synth
                            && param.idx == crate::sampler::PARAM_SPEED
                            && param.value == 2.5
                    }),
                }),
                ScheduledEventKind::EffectParams { .. }
                | ScheduledEventKind::InstrumentParams { .. } => {}
            }
        }
        out.sort_by_key(|event| {
            (
                event.sample_time,
                match event.kind {
                    ScheduledTriggerKind::Step => 0_u8,
                    ScheduledTriggerKind::Network => 1_u8,
                },
                event.track,
            )
        });
        out
    }

    fn run_sparse_process_accumulator_fixture() -> (Arc<SequencerState>, Vec<ObservedTrigger>) {
        run_sparse_process_accumulator_fixture_impl(false)
    }

    fn run_sparse_process_accumulator_fixture_via_lisp_attach(
    ) -> (Arc<SequencerState>, Vec<ObservedTrigger>) {
        run_sparse_process_accumulator_fixture_impl(true)
    }

    fn run_sparse_process_accumulator_fixture_impl(
        attach_via_lisp: bool,
    ) -> (Arc<SequencerState>, Vec<ObservedTrigger>) {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        state.pattern.track_params[0].set_num_steps(8);
        for step in 0..8 {
            state.pattern.patterns[0].set_step_active(step, true);
        }

        let mut scratch = lisp_host::ScratchControlRuntime::new(
            Arc::clone(&state),
            vec![Vec::new()],
            vec![EffectDescriptor::builtin_sampler()],
            0,
            0,
        );
        scratch
            .eval(
                r#"
                (def-accumulator sparse-transpose
                  :target (step-param :transpose)
                  :amount (amount :lane true :default 0)
                  :range (-128 128)
                  :mode :clip)
                "#,
            )
            .expect("define process accumulator");

        if attach_via_lisp {
            scratch
                .eval(
                    r#"
                    (processes :track 0
                      (sparse-transpose :amount (lane 0 1 0 0 1 0 0 0)))
                    "#,
                )
                .expect("attach process chain via lisp");
            let chain = state.track_process_chain(0).expect("track 0 process chain");
            assert_eq!(chain.slots.len(), 1);
            assert_eq!(chain.slots[0].class_name, "sparse-transpose");
        } else {
            assert!(state.set_track_process_chain(
                0,
                crate::process::TrackProcessChain {
                    slots: vec![crate::process::TrackProcessSlot {
                        instance_id: crate::process::ProcessInstanceId(1),
                        instance_name: None,
                        class_name: "sparse-transpose".to_string(),
                        enabled: true,
                        project_layer: false,
                        inlets: std::collections::BTreeMap::new(),
                        lanes: std::collections::BTreeMap::from([(
                            "amount".to_string(),
                            crate::process::ProcessLane {
                                values: vec![0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0],
                            },
                        )]),
                        bindings: std::collections::BTreeMap::new(),
                    }],
                },
            ));
        }

        state.transport.playing.store(true, Ordering::Relaxed);
        let snapshot = state.publish_scheduler_snapshot();
        let queue = ScheduledEventQueue::<32>::new();
        let mut scheduler = SchedulerLookaheadState::new(48_000);
        scheduler
            .process_runtime
            .sync_authoring(scratch.process_authoring_snapshot(), 0.0);
        let live_midi_fx_tracks: [LiveMidiFxTrackState; MAX_TRACKS] =
            std::array::from_fn(|_| LiveMidiFxTrackState::default());
        let mut scratch_runtime = Some(scratch);

        schedule_playing_lookahead(
            &mut scheduler,
            &state,
            &snapshot,
            &queue,
            &mut scratch_runtime,
            &live_midi_fx_tracks,
            snapshot.transport.pattern_epoch,
            0,
            102_000,
            48_000,
            6_000,
            24_000.0,
            0,
            false,
            false,
        );

        (state, observed_triggers(&queue))
    }

    fn run_default_inert_process_accumulator_fixture(
        attach_default_process: bool,
    ) -> (Arc<SequencerState>, Vec<ObservedTrigger>) {
        let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
        state.pattern.track_params[0].set_num_steps(8);
        let base_transposes = [0.0, 7.0, -3.0, 12.0, 0.0, 5.0, -5.0, 2.0];
        let base_durations = [1.0, 0.5, 2.0, 1.5, 0.75, 1.25, 0.5, 2.0];
        for step in 0..8 {
            state.pattern.patterns[0].set_step_active(step, true);
            state.set_step_param(0, step, StepParam::Transpose, base_transposes[step]);
            state.set_step_param(0, step, StepParam::Duration, base_durations[step]);
        }

        let mut scratch = lisp_host::ScratchControlRuntime::new(
            Arc::clone(&state),
            vec![Vec::new()],
            vec![EffectDescriptor::builtin_sampler()],
            0,
            0,
        );
        scratch
            .eval(
                r#"
                (def-accumulator default-transpose
                  :target (step-param :transpose)
                  :amount (amount :lane true :default 0)
                  :range (-128 128)
                  :mode :clip)
                "#,
            )
            .expect("define default-inert process accumulator");
        if attach_default_process {
            scratch
                .eval("(processes :track 0 (default-transpose))")
                .expect("attach default process accumulator");
            let chain = state.track_process_chain(0).expect("track 0 process chain");
            assert_eq!(chain.slots.len(), 1);
            assert_eq!(chain.slots[0].class_name, "default-transpose");
            assert!(
                chain.slots[0].lanes.is_empty(),
                "default attachment should not persist any lane overrides"
            );
        }

        state.transport.playing.store(true, Ordering::Relaxed);
        let snapshot = state.publish_scheduler_snapshot();
        let queue = ScheduledEventQueue::<32>::new();
        let mut scheduler = SchedulerLookaheadState::new(48_000);
        scheduler
            .process_runtime
            .sync_authoring(scratch.process_authoring_snapshot(), 0.0);
        let live_midi_fx_tracks: [LiveMidiFxTrackState; MAX_TRACKS] =
            std::array::from_fn(|_| LiveMidiFxTrackState::default());
        let mut scratch_runtime = Some(scratch);

        schedule_playing_lookahead(
            &mut scheduler,
            &state,
            &snapshot,
            &queue,
            &mut scratch_runtime,
            &live_midi_fx_tracks,
            snapshot.transport.pattern_epoch,
            0,
            102_000,
            48_000,
            6_000,
            24_000.0,
            0,
            false,
            false,
        );

        (state, observed_triggers(&queue))
    }

    fn run_with_scheduler_stack<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
        std::thread::Builder::new()
            .name("scheduler-process-accumulator-harness".to_string())
            .stack_size(super::SCHEDULER_THREAD_STACK_SIZE)
            .spawn(f)
            .expect("spawn scheduler process accumulator harness")
            .join()
            .expect("scheduler process accumulator harness panicked")
    }

    fn schedule_process_fixture(
        state: &Arc<SequencerState>,
        scratch: lisp_host::ScratchControlRuntime,
    ) -> Vec<ScheduledEventKind> {
        state.transport.playing.store(true, Ordering::Relaxed);
        let snapshot = state.publish_scheduler_snapshot();
        let queue = ScheduledEventQueue::<64>::new();
        let mut scheduler = SchedulerLookaheadState::new(48_000);
        scheduler
            .process_runtime
            .sync_authoring(scratch.process_authoring_snapshot(), 0.0);
        let live_midi_fx_tracks: [LiveMidiFxTrackState; MAX_TRACKS] =
            std::array::from_fn(|_| LiveMidiFxTrackState::default());
        let mut scratch_runtime = Some(scratch);

        schedule_playing_lookahead(
            &mut scheduler,
            state,
            &snapshot,
            &queue,
            &mut scratch_runtime,
            &live_midi_fx_tracks,
            snapshot.transport.pattern_epoch,
            0,
            24_000,
            48_000,
            6_000,
            24_000.0,
            0,
            false,
            false,
        );

        let mut events = Vec::new();
        while let Some(event) = queue.pop() {
            events.push(event.kind);
        }
        events
    }

    fn schedule_process_observed_fixture(
        state: &Arc<SequencerState>,
        scratch: lisp_host::ScratchControlRuntime,
        lookahead_target_samples: u64,
    ) -> Vec<ObservedTrigger> {
        state.transport.playing.store(true, Ordering::Relaxed);
        let snapshot = state.publish_scheduler_snapshot();
        let queue = ScheduledEventQueue::<64>::new();
        let mut scheduler = SchedulerLookaheadState::new(48_000);
        scheduler
            .process_runtime
            .sync_authoring(scratch.process_authoring_snapshot(), 0.0);
        let live_midi_fx_tracks: [LiveMidiFxTrackState; MAX_TRACKS] =
            std::array::from_fn(|_| LiveMidiFxTrackState::default());
        let mut scratch_runtime = Some(scratch);

        schedule_playing_lookahead(
            &mut scheduler,
            state,
            &snapshot,
            &queue,
            &mut scratch_runtime,
            &live_midi_fx_tracks,
            snapshot.transport.pattern_epoch,
            0,
            lookahead_target_samples,
            48_000,
            6_000,
            24_000.0,
            0,
            false,
            false,
        );

        observed_triggers(&queue)
    }

    fn first_resolved_trigger(events: &[ScheduledEventKind]) -> &ScheduledEventKind {
        events
            .iter()
            .find(|event| matches!(event, ScheduledEventKind::ResolvedTrigger { .. }))
            .expect("resolved trigger event")
    }

    #[test]
    fn scheduler_process_accumulator_folds_sparse_lane_into_transpose() {
        let (_state, events) = run_with_scheduler_stack(run_sparse_process_accumulator_fixture);
        let transposes = events
            .iter()
            .take(8)
            .map(|event| event.transpose)
            .collect::<Vec<_>>();
        assert_eq!(transposes, vec![0.0, 1.0, 1.0, 1.0, 2.0, 2.0, 2.0, 2.0]);
    }

    #[test]
    fn scheduler_project_layer_runs_on_every_track_with_independent_state() {
        let events = run_with_scheduler_stack(|| {
            let state = Arc::new(SequencerState::new(
                2,
                vec![default_empty_effect_chain(), default_empty_effect_chain()],
            ));
            for track in 0..2 {
                state.pattern.track_params[track].set_num_steps(8);
                for step in 0..8 {
                    state.pattern.patterns[track].set_step_active(step, true);
                }
            }

            let mut scratch = lisp_host::ScratchControlRuntime::new(
                Arc::clone(&state),
                vec![Vec::new(), Vec::new()],
                vec![
                    EffectDescriptor::builtin_sampler(),
                    EffectDescriptor::builtin_sampler(),
                ],
                0,
                0,
            );
            scratch
                .eval(
                    r#"
                    (def-process count-up
                      :target (step-param :transpose)
                      :state ((acc 0))
                      :run (do
                        (set! acc (+ acc 1))
                        (target-add! acc)))

                    (def-accumulator sparse-transpose
                      :target (step-param :transpose)
                      :amount (amount :lane true :default 0)
                      :range (-128 128)
                      :mode :clip)

                    (processes :project (count-up))
                    (processes :track 1
                      (sparse-transpose :amount (lane 10 0 0 0 0 0 0 0)))
                    "#,
                )
                .expect("attach project layer and track chain");

            let project_chain = state.project_process_chain();
            assert_eq!(project_chain.slots.len(), 1);
            assert!(project_chain.slots[0].project_layer);

            schedule_process_observed_fixture(&state, scratch, 102_000)
        });

        let track_transposes = |track: usize| {
            events
                .iter()
                .filter(|event| event.track == track)
                .take(8)
                .map(|event| event.transpose)
                .collect::<Vec<_>>()
        };
        // The project counter runs on both tracks with independent state:
        // shared configuration, per-(instance, track) runtime state. A shared
        // state cell would interleave to 1..16 across the two tracks.
        assert_eq!(
            track_transposes(0),
            vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0],
            "track 0 runs its own copy of the project counter"
        );
        // Track 1 composes its own chain after the project layer: the sparse
        // accumulator holds +10 from step 0 onward on top of the counter.
        assert_eq!(
            track_transposes(1),
            vec![11.0, 12.0, 13.0, 14.0, 15.0, 16.0, 17.0, 18.0],
            "track 1 = project counter + its own accumulator"
        );
    }

    #[test]
    fn scheduler_resolved_track_read_uses_previous_tick_not_trigger_visit_order() {
        let events = run_with_scheduler_stack(|| {
            let state = Arc::new(SequencerState::new(
                2,
                vec![default_empty_effect_chain(), default_empty_effect_chain()],
            ));
            for track in 0..2 {
                state.pattern.track_params[track].set_num_steps(4);
                for step in 0..4 {
                    state.pattern.patterns[track].set_step_active(step, true);
                }
            }
            for (step, transpose) in [2.0, 4.0, 6.0, 8.0].into_iter().enumerate() {
                state.set_step_param(0, step, StepParam::Transpose, transpose);
            }

            let mut scratch = lisp_host::ScratchControlRuntime::new(
                Arc::clone(&state),
                vec![Vec::new(), Vec::new()],
                vec![
                    EffectDescriptor::builtin_sampler(),
                    EffectDescriptor::builtin_sampler(),
                ],
                0,
                0,
            );
            scratch
                .eval(
                    r#"
                    (def-process follow-previous-source
                      :target (step-param :transpose)
                      :run (target-add! (read (track 0 :transpose))))
                    (processes :track 1 (follow-previous-source))
                    "#,
                )
                .expect("define previous-tick track reader");

            schedule_process_observed_fixture(&state, scratch, 54_000)
        });

        let track_transposes = |track: usize| {
            events
                .iter()
                .filter(|event| event.track == track)
                .take(4)
                .map(|event| event.transpose)
                .collect::<Vec<_>>()
        };
        assert_eq!(track_transposes(0), vec![2.0, 4.0, 6.0, 8.0]);
        assert_eq!(
            track_transposes(1),
            vec![0.0, 2.0, 4.0, 6.0],
            "track 1 must not observe track 0's same-boundary value even though track 0 sorts first"
        );
    }

    #[test]
    fn scheduler_resolved_track_read_repeats_across_pattern_cycles() {
        let events = run_with_scheduler_stack(|| {
            let state = Arc::new(SequencerState::new(
                2,
                vec![default_empty_effect_chain(), default_empty_effect_chain()],
            ));
            for track in 0..2 {
                state.pattern.track_params[track].set_num_steps(8);
            }
            state.pattern.patterns[0].set_step_active(0, true);
            state.set_step_param(0, 0, StepParam::Transpose, 7.0);
            state.pattern.patterns[1].set_step_active(4, true);

            let mut scratch = lisp_host::ScratchControlRuntime::new(
                Arc::clone(&state),
                vec![Vec::new(), Vec::new()],
                vec![
                    EffectDescriptor::builtin_sampler(),
                    EffectDescriptor::builtin_sampler(),
                ],
                0,
                0,
            );
            scratch
                .eval(
                    r#"
                    (def-process repeat-current-source
                      :target (step-param :transpose)
                      :run (target-add! (read (track 0 :transpose))))
                    (processes :track 1 (repeat-current-source))
                    "#,
                )
                .expect("define repeating current-value reader");

            schedule_process_observed_fixture(&state, scratch, 108_000)
        });

        let reader = events
            .iter()
            .filter(|event| event.track == 1)
            .map(|event| event.transpose)
            .collect::<Vec<_>>();
        assert_eq!(reader, vec![7.0, 7.0], "reader must repeat every cycle");
    }

    #[test]
    fn phase7_demo_trigger_history_reader_repeats_across_pattern_cycles() {
        let events = run_with_scheduler_stack(|| {
            let state = Arc::new(SequencerState::new(
                2,
                vec![default_empty_effect_chain(), default_empty_effect_chain()],
            ));
            for track in 0..2 {
                state.pattern.track_params[track].set_num_steps(8);
            }
            state.pattern.patterns[0].set_step_active(0, true);
            state.set_step_param(0, 0, StepParam::Transpose, 7.0);
            // UI step #3 is zero-based scheduler step 2, where the demo's
            // `:trigs-ago 1` amount lane is active.
            state.pattern.patterns[1].set_step_active(2, true);

            let mut scratch = lisp_host::ScratchControlRuntime::new(
                Arc::clone(&state),
                vec![Vec::new(), Vec::new()],
                vec![
                    EffectDescriptor::builtin_sampler(),
                    EffectDescriptor::builtin_sampler(),
                ],
                0,
                0,
            );
            let script_path = format!(
                "{}/scripts/process-phase7-reads-demo.lisp",
                env!("CARGO_MANIFEST_DIR")
            );
            let source = std::fs::read_to_string(&script_path).expect("read Phase 7 reads demo");
            scratch
                .eval_source_at_path(script_path, &source)
                .expect("evaluate Phase 7 reads demo");

            schedule_process_observed_fixture(&state, scratch, 108_000)
        });

        let reader = events
            .iter()
            .filter(|event| event.track == 1)
            .map(|event| event.transpose)
            .collect::<Vec<_>>();
        assert_eq!(
            reader,
            vec![7.0, 7.0],
            "the demo's UI step #3 trigger-history reader must repeat every cycle"
        );
    }

    #[test]
    fn scheduler_process_accumulator_lisp_attach_matches_manual_chain() {
        let (_state, events) =
            run_with_scheduler_stack(run_sparse_process_accumulator_fixture_via_lisp_attach);
        let transposes = events
            .iter()
            .take(8)
            .map(|event| event.transpose)
            .collect::<Vec<_>>();
        assert_eq!(transposes, vec![0.0, 1.0, 1.0, 1.0, 2.0, 2.0, 2.0, 2.0]);
    }

    #[test]
    fn scheduler_process_accumulator_carries_across_pattern_cycles() {
        let (_state, events) = run_with_scheduler_stack(run_sparse_process_accumulator_fixture);
        let transposes = events
            .iter()
            .take(10)
            .map(|event| event.transpose)
            .collect::<Vec<_>>();
        assert_eq!(
            transposes,
            vec![0.0, 1.0, 1.0, 1.0, 2.0, 2.0, 2.0, 2.0, 2.0, 3.0]
        );
    }

    #[test]
    fn scheduler_process_accumulator_replay_does_not_double_advance_fold() {
        let (_first_state, first) =
            run_with_scheduler_stack(run_sparse_process_accumulator_fixture);
        let (_second_state, second) =
            run_with_scheduler_stack(run_sparse_process_accumulator_fixture);
        let first_transposes = first
            .iter()
            .take(8)
            .map(|event| event.transpose)
            .collect::<Vec<_>>();
        let second_transposes = second
            .iter()
            .take(8)
            .map(|event| event.transpose)
            .collect::<Vec<_>>();
        assert_eq!(second_transposes, first_transposes);
    }

    #[test]
    fn scheduler_process_chain_defaults_are_audibly_inert() {
        let (_base_state, baseline) =
            run_with_scheduler_stack(|| run_default_inert_process_accumulator_fixture(false));
        let (_process_state, default_attached) =
            run_with_scheduler_stack(|| run_default_inert_process_accumulator_fixture(true));
        assert_eq!(
            default_attached.iter().take(8).cloned().collect::<Vec<_>>(),
            baseline.iter().take(8).cloned().collect::<Vec<_>>(),
            "attaching a process at defaults must not alter scheduled note timing, transpose, duration, or sampler params"
        );
    }

    #[test]
    fn scheduler_process_target_writes_are_transient_step_param_writes() {
        let (state, events) = run_with_scheduler_stack(run_sparse_process_accumulator_fixture);
        assert_eq!(events[4].transpose, 2.0);
        for step in 0..8 {
            assert_eq!(
                state.pattern.step_data[0].get(step, StepParam::Transpose),
                StepParam::Transpose.default_value()
            );
        }
        assert!(state
            .pattern
            .plock_variant_registries
            .lock()
            .unwrap()
            .iter()
            .all(|registry| registry == &crate::plock_variants::PlockVariantRegistry::default()));
        assert!(state
            .pattern
            .key_lock_variant_registries
            .lock()
            .unwrap()
            .iter()
            .all(|registry| registry == &crate::plock_variants::PlockVariantRegistry::default()));
        assert!(!state.pattern.instrument_slots[0].key_locks.has_any_lock());
    }

    #[test]
    fn scheduler_process_named_ports_accumulate_ordered_step_writes() {
        run_with_scheduler_stack(|| {
            let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
            state.pattern.track_params[0].set_num_steps(1);
            state.pattern.patterns[0].set_step_active(0, true);
            let mut scratch = lisp_host::ScratchControlRuntime::new(
                Arc::clone(&state),
                vec![Vec::new()],
                vec![EffectDescriptor::builtin_sampler()],
                0,
                0,
            );
            scratch
                .eval(
                    r#"
                    (def-process pitch-a
                      :targets '((pitch (step-param :transpose)))
                      :run (target-add! :pitch 3))
                    (def-process pitch-b
                      :target (step-param :transpose)
                      :run (target-add! 4))
                    (processes :track 0 (pitch-a) (pitch-b))
                    "#,
                )
                .expect("define process chain");

            let events = schedule_process_fixture(&state, scratch);
            match first_resolved_trigger(&events) {
                ScheduledEventKind::ResolvedTrigger { resolved, .. } => {
                    assert_eq!(resolved.transpose, 7.0);
                }
                other => panic!("expected resolved trigger, got {other:?}"),
            }
            assert_eq!(
                state.pattern.step_data[0].get(0, StepParam::Transpose),
                StepParam::Transpose.default_value()
            );
        });
    }

    #[test]
    fn scheduler_process_inlet_writes_compose_with_downstream_lane_this_fire() {
        run_with_scheduler_stack(|| {
            let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
            state.pattern.track_params[0].set_num_steps(1);
            state.pattern.patterns[0].set_step_active(0, true);
            let mut scratch = lisp_host::ScratchControlRuntime::new(
                Arc::clone(&state),
                vec![Vec::new()],
                vec![EffectDescriptor::builtin_sampler()],
                0,
                0,
            );
            scratch
                .eval(
                    r#"
                    (def-process process-inlet-setter
                      :targets ((out :process-inlet))
                      :run (target-set! :out 3))
                    (def-process process-inlet-adder
                      :targets ((out :process-inlet))
                      :run (target-add! :out 2))
                    (def-process inlet-driven-pitch
                      :in ((amount :float -12 12 :default 0 :lane true))
                      :target (step-param :transpose)
                      :run (target-add! (in :amount)))

                    (def setter (process-inlet-setter))
                    (def adder (process-inlet-adder))
                    (def pitch (inlet-driven-pitch :amount (lane 1)))
                    (processes :track 0 setter adder pitch)
                    (connect! setter :out (inlet pitch :amount))
                    (connect! adder :out (inlet pitch :amount))
                    "#,
                )
                .expect("define process-inlet chain");

            let events = schedule_process_fixture(&state, scratch);
            match first_resolved_trigger(&events) {
                ScheduledEventKind::ResolvedTrigger { resolved, .. } => {
                    assert_eq!(resolved.transpose, 5.0);
                }
                other => panic!("expected resolved trigger, got {other:?}"),
            }
            assert_eq!(
                state.pattern.step_data[0].get(0, StepParam::Transpose),
                StepParam::Transpose.default_value()
            );
            let chain = state.track_process_chain(0).expect("track 0 chain");
            assert_eq!(
                chain.slots[2]
                    .lanes
                    .get("amount")
                    .map(|lane| lane.values.as_slice()),
                Some(&[1.0][..])
            );
        });
    }

    #[test]
    fn scheduler_process_inlet_write_to_earlier_slot_arrives_next_fire() {
        run_with_scheduler_stack(|| {
            let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
            state.pattern.track_params[0].set_num_steps(16);
            state.pattern.patterns[0].set_step_active(0, true);
            state.pattern.patterns[0].set_step_active(1, true);
            let mut scratch = lisp_host::ScratchControlRuntime::new(
                Arc::clone(&state),
                vec![Vec::new()],
                vec![EffectDescriptor::builtin_sampler()],
                0,
                0,
            );
            scratch
                .eval(
                    r#"
                    (def-process earlier-pitch
                      :in ((amount :float -12 12 :default 0 :lane true))
                      :target (step-param :transpose)
                      :run (target-add! (in :amount)))
                    (def-process late-writer
                      :targets ((out :process-inlet))
                      :run (target-set! :out 7))

                    (def pitch (earlier-pitch))
                    (def writer (late-writer))
                    (processes :track 0 pitch writer)
                    (connect! writer :out (inlet pitch :amount))
                    "#,
                )
                .expect("define upstream process-inlet chain");

            let events = schedule_process_observed_fixture(&state, scratch, 12_000);
            let transposes = events
                .iter()
                .take(2)
                .map(|event| event.transpose)
                .collect::<Vec<_>>();
            assert_eq!(transposes, vec![0.0, 7.0], "{events:?}");
        });
    }

    #[test]
    fn scheduler_process_veto_suppresses_base_event_but_continues_chain() {
        run_with_scheduler_stack(|| {
            let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
            state.pattern.track_params[0].set_num_steps(16);
            state.pattern.patterns[0].set_step_active(0, true);
            let mut scratch = lisp_host::ScratchControlRuntime::new(
                Arc::clone(&state),
                vec![Vec::new()],
                vec![EffectDescriptor::builtin_sampler()],
                0,
                0,
            );
            scratch
                .eval(
                    r#"
                    (def-process kill-base
                      :run (veto!))
                    (def-process clone-after-veto
                      :target (step-param :transpose)
                      :run (do
                        (target-add! 7)
                        (ratchet! :times 1 :mode :repeat :span 0)))
                    (processes :track 0 (kill-base) (clone-after-veto))
                    "#,
                )
                .expect("define veto chain fixture");

            let events = schedule_process_observed_fixture(&state, scratch, 6_000);
            assert_eq!(events.len(), 1, "{events:?}");
            assert_eq!(events[0].sample_time, 1);
            assert_eq!(events[0].transpose, 7.0);
        });
    }

    #[test]
    fn scheduler_process_commands_apply_target_writes_in_authored_order() {
        run_with_scheduler_stack(|| {
            let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
            state.pattern.track_params[0].set_num_steps(16);
            state.pattern.patterns[0].set_step_active(0, true);
            let mut scratch = lisp_host::ScratchControlRuntime::new(
                Arc::clone(&state),
                vec![Vec::new()],
                vec![EffectDescriptor::builtin_sampler()],
                0,
                0,
            );
            scratch
                .eval(
                    r#"
                    (def-process ordered-command-stream
                      :target (step-param :transpose)
                      :run (do
                        (ratchet! :times 1 :mode :repeat :span 0)
                        (target-add! 7)
                        (veto!)))
                    (processes :track 0 (ordered-command-stream))
                    "#,
                )
                .expect("define ordered command fixture");

            let events = schedule_process_observed_fixture(&state, scratch, 6_000);
            assert_eq!(events.len(), 1, "{events:?}");
            assert_eq!(events[0].transpose, 0.0, "{events:?}");
        });
    }

    #[test]
    fn scheduler_process_ratchet_subdivide_offsets_and_scales_duration() {
        run_with_scheduler_stack(|| {
            let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
            state.pattern.track_params[0].set_num_steps(16);
            state.pattern.patterns[0].set_step_active(0, true);
            let mut scratch = lisp_host::ScratchControlRuntime::new(
                Arc::clone(&state),
                vec![Vec::new()],
                vec![EffectDescriptor::builtin_sampler()],
                0,
                0,
            );
            scratch
                .eval(
                    r#"
                    (def-process subdivide-burst
                      :run (do
                        (veto!)
                        (ratchet! :times 4 :mode :subdivide :span 0.25)))
                    (processes :track 0 (subdivide-burst))
                    "#,
                )
                .expect("define subdivide ratchet fixture");

            let events = schedule_process_observed_fixture(&state, scratch, 6_000);
            let sample_times = events
                .iter()
                .map(|event| event.sample_time)
                .collect::<Vec<_>>();
            assert_eq!(sample_times, vec![1, 1_501, 3_001, 4_501], "{events:?}");
            assert!(events
                .iter()
                .all(|event| (event.duration - 0.25).abs() < 1e-6));
        });
    }

    #[test]
    fn scheduler_process_ratchet_repeat_keeps_duration_for_ring_through_overlap() {
        run_with_scheduler_stack(|| {
            let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
            state.pattern.track_params[0].set_num_steps(16);
            state.pattern.patterns[0].set_step_active(0, true);
            state.pattern.step_data[0].set(0, StepParam::Duration, 1.0);
            let mut scratch = lisp_host::ScratchControlRuntime::new(
                Arc::clone(&state),
                vec![Vec::new()],
                vec![EffectDescriptor::builtin_sampler()],
                0,
                0,
            );
            scratch
                .eval(
                    r#"
                    (def-process repeat-burst
                      :run (do
                        (veto!)
                        (ratchet! :times 3 :mode :repeat :span 0.125)))
                    (processes :track 0 (repeat-burst))
                    "#,
                )
                .expect("define repeat ratchet fixture");

            let events = schedule_process_observed_fixture(&state, scratch, 12_000);
            let sample_times = events
                .iter()
                .map(|event| event.sample_time)
                .collect::<Vec<_>>();
            assert_eq!(sample_times, vec![1, 3_001, 6_001], "{events:?}");
            assert!(events
                .iter()
                .all(|event| (event.duration - 1.0).abs() < 1e-6));
        });
    }

    #[test]
    fn scheduler_process_ratchet_shape_error_drops_burst_without_aborting_lookahead() {
        run_with_scheduler_stack(|| {
            let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
            state.pattern.track_params[0].set_num_steps(16);
            state.pattern.patterns[0].set_step_active(0, true);
            state.pattern.patterns[0].set_step_active(1, true);
            let mut scratch = lisp_host::ScratchControlRuntime::new(
                Arc::clone(&state),
                vec![Vec::new()],
                vec![EffectDescriptor::builtin_sampler()],
                0,
                0,
            );
            scratch
                .eval(
                    r#"
                    (def-process broken-ratchet-shape
                      :run (ratchet! :times 3
                                      :mode :subdivide
                                      :span 0.25
                                      :shape (lambda (i ev)
                                               (if (= i 1)
                                                 (vel! ev "not-a-number")
                                                 ev))))
                    (processes :track 0 (broken-ratchet-shape))
                    "#,
                )
                .expect("define broken ratchet shape fixture");

            let events = schedule_process_observed_fixture(&state, scratch, 12_000);
            let sample_times = events
                .iter()
                .map(|event| event.sample_time)
                .collect::<Vec<_>>();
            assert_eq!(
                sample_times,
                vec![0, 6_000],
                "a bad shape should drop each burst atomically while base scheduling continues: {events:?}"
            );
        });
    }

    #[test]
    fn scheduler_process_stale_midi_fx_target_is_noop_without_blocking_other_ports() {
        run_with_scheduler_stack(|| {
            let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
            state.pattern.track_params[0].set_num_steps(1);
            state.pattern.patterns[0].set_step_active(0, true);
            let mut scratch = lisp_host::ScratchControlRuntime::new(
                Arc::clone(&state),
                vec![Vec::new()],
                vec![EffectDescriptor::builtin_sampler()],
                0,
                0,
            );
            scratch
                .eval(
                    r#"
                    (def-process stale-port
                      :targets '((pitch (step-param :transpose))
                                 (gate (fx-param :beat-repeat :gate)))
                      :run (do
                        (target-add! :pitch 5)
                        (target-set! :gate 0)))
                    (processes :track 0 (stale-port))
                    "#,
                )
                .expect("define stale-port process");

            let events = schedule_process_fixture(&state, scratch);
            match first_resolved_trigger(&events) {
                ScheduledEventKind::ResolvedTrigger { resolved, .. } => {
                    assert_eq!(resolved.transpose, 5.0);
                }
                other => panic!("expected resolved trigger, got {other:?}"),
            }
        });
    }

    #[test]
    fn scheduler_process_midi_fx_param_write_applies_to_temporary_slot_snapshot() {
        run_with_scheduler_stack(|| {
            let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
            state.pattern.track_params[0].set_num_steps(1);
            state.pattern.patterns[0].set_step_active(0, true);
            let mut scratch = lisp_host::ScratchControlRuntime::new(
                Arc::clone(&state),
                vec![Vec::new()],
                vec![EffectDescriptor::builtin_sampler()],
                0,
                0,
            );
            let source = format!(
                "{}\n{}",
                lisp_host::load_midi_fx_library_source(),
                r#"
                (def-process close-repeat-gate
                  :target (fx-param :beat-repeat :gate)
                  :run (target-set! 0.0))
                (seq-use-midi-fx 0 "beat-repeat")
                (processes :track 0 (close-repeat-gate))
                "#
            );
            scratch
                .eval(&source)
                .expect("define MIDI FX process fixture");

            let gate_idx = scratch
                .midi_fx_descriptors()
                .iter()
                .find(|desc| desc.name == "beat-repeat")
                .and_then(|desc| {
                    desc.params
                        .iter()
                        .position(|param| param.name.eq_ignore_ascii_case("gate"))
                })
                .expect("beat-repeat gate param");
            let stored_default = state.pattern.midi_fx_slots[0][0].defaults.get(gate_idx);
            assert!((stored_default - 0.90).abs() < 1e-6);

            let events = schedule_process_fixture(&state, scratch);
            match first_resolved_trigger(&events) {
                ScheduledEventKind::ResolvedTrigger { resolved, .. } => {
                    assert!((resolved.duration - 0.05).abs() < 1e-6, "{resolved:?}");
                }
                other => panic!("expected resolved trigger, got {other:?}"),
            }
            assert_eq!(
                state.pattern.midi_fx_slots[0][0].plocks.get(0, gate_idx),
                None
            );
            assert!((state.pattern.midi_fx_slots[0][0].defaults.get(gate_idx) - 0.90).abs() < 1e-6);
        });
    }

    #[test]
    fn scheduler_process_device_param_writes_upsert_transient_event_payloads() {
        run_with_scheduler_stack(|| {
            let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
            state.pattern.track_params[0].set_num_steps(1);
            state.pattern.patterns[0].set_step_active(0, true);

            let sampler_desc = EffectDescriptor::builtin_sampler();
            let filter_desc = EffectDescriptor::builtin_filter();
            let speed_param_idx = sampler_desc
                .params
                .iter()
                .position(|param| param.name == "speed")
                .expect("sampler speed param");
            let release_param_idx = sampler_desc
                .params
                .iter()
                .position(|param| param.name == "release")
                .expect("sampler release param");
            let filter_mode_param_idx = filter_desc
                .params
                .iter()
                .position(|param| param.name == "mode")
                .expect("filter mode param");

            state.pattern.instrument_slots[0].apply_descriptor(&sampler_desc, 12);
            state.pattern.instrument_slots[0].set_plock(0, speed_param_idx, 0.0);
            state.pattern.effect_chains[0][0].apply_descriptor(&filter_desc, 42);
            state.pattern.effect_chains[0][0].set_plock(0, filter_mode_param_idx, 1.0);
            let mut effect_descriptors = EffectDescriptor::default_full_chain();
            effect_descriptors[0] = filter_desc.clone();
            state.set_scratch_runtime_descriptors(
                vec![effect_descriptors.clone()],
                vec![sampler_desc.clone()],
            );

            let mut scratch = lisp_host::ScratchControlRuntime::new(
                Arc::clone(&state),
                vec![effect_descriptors],
                vec![sampler_desc],
                0,
                0,
            );
            scratch
                .eval(
                    r#"
                    (def-process device-writes
                      :targets '((inst (instrument-param :speed))
                                 (release (instrument-param :release))
                                 (mode (effect-param "Filter" :mode)))
                      :run (do
                        (target-set! :inst 1.0)
                        (target-set! :release 1.0)
                        (target-set! :mode 1.0)))
                    (processes :track 0 (device-writes))
                    "#,
                )
                .expect("define device process fixture");

            let events = schedule_process_fixture(&state, scratch);
            match first_resolved_trigger(&events) {
                ScheduledEventKind::ResolvedTrigger {
                    effect_params,
                    instrument_params,
                    sampler_params,
                    ..
                } => {
                    assert!(instrument_params.iter().any(|param| {
                        param.target == ScheduledInstrumentParamTarget::Synth
                            && param.idx == crate::sampler::PARAM_SPEED
                            && (param.value - 4.0).abs() < 1e-6
                    }));
                    assert!(
                        (sampler_params.playback_speed - 4.0).abs() < 1e-6,
                        "{sampler_params:?}"
                    );
                    assert!(
                        (sampler_params.release_ms - 2000.0).abs() < 1e-6,
                        "{sampler_params:?}"
                    );
                    assert!(effect_params.iter().any(|param| {
                        param.logical_id == 42
                            && param.idx == crate::filter::FILTER_PARAM_MODE as u64
                            && (param.value - 3.0).abs() < 1e-6
                    }));
                }
                other => panic!("expected resolved trigger, got {other:?}"),
            }
            assert_eq!(
                state.pattern.instrument_slots[0]
                    .plocks
                    .get(0, speed_param_idx),
                Some(0.0)
            );
            assert_eq!(
                state.pattern.instrument_slots[0]
                    .plocks
                    .get(0, release_param_idx),
                None
            );
            assert!(
                (state.pattern.instrument_slots[0]
                    .defaults
                    .get(release_param_idx))
                .abs()
                    < 1e-6
            );
            assert_eq!(
                state.pattern.effect_chains[0][0]
                    .plocks
                    .get(0, filter_mode_param_idx),
                Some(1.0)
            );
        });
    }

    #[test]
    fn scheduler_phase3a_demo_live_edits_drive_pitch_and_sampler_speed() {
        run_with_scheduler_stack(|| {
            let state = Arc::new(SequencerState::new(1, vec![default_empty_effect_chain()]));
            state.pattern.track_params[0].set_num_steps(1);
            state.pattern.patterns[0].set_step_active(0, true);

            let sampler_desc = EffectDescriptor::builtin_sampler();
            let speed_param_idx = sampler_desc
                .params
                .iter()
                .position(|param| param.name == "speed")
                .expect("sampler speed param");
            state.pattern.instrument_slots[0].apply_descriptor(&sampler_desc, 12);
            state.set_scratch_runtime_descriptors(vec![Vec::new()], vec![sampler_desc.clone()]);

            let mut scratch = lisp_host::ScratchControlRuntime::new(
                Arc::clone(&state),
                vec![Vec::new()],
                vec![sampler_desc],
                0,
                0,
            );
            let script_path = format!(
                "{}/scripts/process-phase3a-ports-demo.lisp",
                env!("CARGO_MANIFEST_DIR")
            );
            let source = std::fs::read_to_string(&script_path).expect("read Phase 3A process demo");
            scratch
                .eval_source_at_path(script_path, &source)
                .expect("load Phase 3A process demo");
            scratch
                .eval("(phase3a-port-writer-h :pitch 4)")
                .expect("live edit pitch inlet");
            scratch
                .eval("(phase3a-port-writer-h :speed 0.75)")
                .expect("live edit speed inlet");

            let events = schedule_process_fixture(&state, scratch);
            match first_resolved_trigger(&events) {
                ScheduledEventKind::ResolvedTrigger {
                    resolved,
                    instrument_params,
                    sampler_params,
                    ..
                } => {
                    assert!((resolved.transpose - 4.0).abs() < 1e-6, "{resolved:?}");
                    assert!(instrument_params.iter().any(|param| {
                        param.target == ScheduledInstrumentParamTarget::Synth
                            && param.idx == crate::sampler::PARAM_SPEED
                            && (param.value - 2.0).abs() < 1e-6
                    }));
                    assert!(
                        (sampler_params.playback_speed - 2.0).abs() < 1e-6,
                        "{sampler_params:?}"
                    );
                }
                other => panic!("expected resolved trigger, got {other:?}"),
            }

            assert_eq!(
                state.pattern.step_data[0].get(0, StepParam::Transpose),
                StepParam::Transpose.default_value()
            );
            assert_eq!(
                state.pattern.instrument_slots[0]
                    .plocks
                    .get(0, speed_param_idx),
                None
            );
            assert!(
                (state.pattern.instrument_slots[0]
                    .defaults
                    .get(speed_param_idx)
                    - 1.0)
                    .abs()
                    < 1e-6
            );
        });
    }

    #[test]
    fn scheduler_lookahead_routes_lisp_graph_seed_and_propagation_through_midi_fx() {
        std::thread::Builder::new()
            .name("scheduler-routing-harness".to_string())
            .stack_size(super::SCHEDULER_THREAD_STACK_SIZE)
            .spawn(scheduler_lookahead_routes_lisp_graph_seed_and_propagation_through_midi_fx_body)
            .expect("spawn scheduler routing harness")
            .join()
            .expect("scheduler routing harness panicked");
    }

    fn scheduler_lookahead_routes_lisp_graph_seed_and_propagation_through_midi_fx_body() {
        let state = Arc::new(SequencerState::new(
            5,
            (0..5).map(|_| default_empty_effect_chain()).collect(),
        ));
        state.toggle_play();
        state.toggle_step_and_clear_plocks(0, 0);
        state.toggle_step_and_clear_plocks(0, 4);
        state.set_step_param(0, 0, StepParam::Transpose, 7.0);
        state.set_step_param(0, 4, StepParam::Transpose, 7.0);
        state.pattern.track_params[0].set_midi_fx_chain(vec!["trigger-to-track".to_string()]);
        let trigger_desc = lisp_host::load_midi_fx_descriptor("trigger-to-track")
            .expect("trigger-to-track descriptor");
        state.pattern.midi_fx_slots[0][0].apply_descriptor(&trigger_desc, 0);
        state.pattern.midi_fx_slots[0][0].defaults.set(0, 3.0);
        state.pattern.instrument_slots[2]
            .apply_descriptor(&EffectDescriptor::builtin_sampler(), 77);
        state.pattern.instrument_slots[2].defaults.set(12, 2.5);

        publish_test_graph_sequencer(
            Arc::clone(&state),
            r#"
            (def-sequencer "routing-harness-graph"
              :shape (line 2)
              :energy-decay 1
              :reset-every 0
              :seed-on-reset 0
              :max-poly 8
              :max-poly-selection :deterministic
              :duration (steps 1)

              (def-node nrn
                :resolution :16
                :delay 1
                :quantize :16
                :route 0
                :seed-from 0
                :reduce :sum
                :params ((threshold :float 0 4 :default 0.5))
	                :state ((energy :leak (per-step :energy-decay)))
	                :update (if (>= (energy) (param :threshold))
	                          (emit :note (in-note) :vel (in-vel))
	                          false))

	              (edges
	                :from nrn
	                :to nrn
	                :topology (all-to-all)
	                :gather (edge :weight)
	                :params ((weight :float -1 1 :default 0))))
            "#,
        );

        let published_graph = state
            .published_sequencers()
            .into_iter()
            .find(|seq| seq.name == "routing-harness-graph")
            .expect("published graph sequencer");
        let manifest = published_graph.graph.as_ref().expect("graph manifest");
        let edge_group = crate::graph::edge_set_group_id(&manifest.edge_sets[0]);
        state
            .edit_current_graph_overrides(|graphs| {
                graphs.push(ProjectGraphOverrides {
                    sequencer_id: published_graph.id,
                    sequencer_name: published_graph.name.clone(),
                    node_intrinsics: vec![ProjectGraphNodeIntrinsicOverride {
                        group: "nrn".to_string(),
                        instance: 1,
                        resolution: None,
                        delay_steps: None,
                        quantize: None,
                        route: None,
                        seed_from: Some(ProjectGraphSeedFrom::Tracks(Vec::new())),
                        seed_on_reset: None,
                        duration: None,
                        swing: None,
                    }],
                    node_params: Vec::new(),
                    edge_params: vec![ProjectGraphEdgeParamOverride {
                        group: edge_group,
                        from: 0,
                        to: 1,
                        param: "weight".to_string(),
                        value: 1.0,
                    }],
                    reset_every_beats: None,
                    max_poly: None,
                    max_poly_selection: None,
                    node_count: None,
                });
                Ok(())
            })
            .expect("install graph routing overrides");
        let snapshot = state.publish_scheduler_snapshot();
        let mut scheduler = SchedulerLookaheadState::new(48_000);
        let manifests = state
            .published_sequencers()
            .into_iter()
            .filter_map(|seq| seq.graph)
            .collect::<Vec<_>>();
        reconcile_graph_runtimes(
            manifests,
            &snapshot.graph_overrides,
            &mut scheduler.graph_runtimes,
            &mut scheduler.graph_manifests,
            scheduler.clock.total_beats,
        );
        assert_eq!(scheduler.graph_runtimes.len(), 1);

        let mut scratch_runtime = Some(lisp_host::scratch_runtime_with_fallbacks(
            Arc::clone(&state),
            0,
            0,
        ));
        scratch_runtime
            .as_mut()
            .expect("scratch runtime")
            .eval(&lisp_host::load_midi_fx_library_source())
            .expect("load MIDI FX library");
        let queue = ScheduledEventQueue::<64>::new();
        let live_midi_fx_tracks: [LiveMidiFxTrackState; MAX_TRACKS] =
            std::array::from_fn(|_| LiveMidiFxTrackState::default());
        let samples_per_quarter = 48_000.0 * 60.0 / snapshot.transport.bpm as f64;

        let result = schedule_playing_lookahead(
            &mut scheduler,
            &state,
            &snapshot,
            &queue,
            &mut scratch_runtime,
            &live_midi_fx_tracks,
            snapshot.transport.pattern_epoch,
            0,
            48_000,
            48_000,
            12_000,
            samples_per_quarter,
            0,
            false,
            false,
        );
        assert_eq!(result.scheduled_until_sample, 48_000);

        let events = observed_triggers(&queue);
        assert!(
            events.iter().any(|event| {
                event.kind == ScheduledTriggerKind::Step
                    && event.track == 0
                    && event.sample_time == 0
                    && event.transpose == 7.0
            }),
            "source seed step should be scheduled: {events:#?}"
        );
        assert!(
            events.iter().any(|event| {
                event.kind == ScheduledTriggerKind::Step
                    && event.track == 2
                    && event.sample_time == 0
                    && event.transpose == 7.0
                    && event.has_speed_param
            }),
            "seed step should route through trigger-to-track to target track with target params: {events:#?}"
        );
        let target_networks = events
            .iter()
            .filter(|event| event.kind == ScheduledTriggerKind::Network && event.track == 2)
            .collect::<Vec<_>>();
        assert!(
            target_networks.len() >= 2,
            "graph propagation should route multiple network events to the target track: {events:#?}"
        );
        assert!(
            target_networks.iter().all(|event| {
                event.transpose == 7.0
                    && event.duration == 0.25
                    && event.sampler_speed == Some(2.5)
                    && event.has_speed_param
            }),
            "routed graph events should carry the target track instrument/sampler params: {events:#?}"
        );
        let source_network_samples = events
            .iter()
            .filter(|event| event.kind == ScheduledTriggerKind::Network && event.track == 0)
            .map(|event| event.sample_time)
            .collect::<Vec<_>>();
        let target_network_samples = target_networks
            .iter()
            .map(|event| event.sample_time)
            .collect::<Vec<_>>();
        assert_eq!(
            target_network_samples,
            vec![6_000, 30_000],
            "graph propagation should produce the expected finite routed target events"
        );
        assert_eq!(
            target_network_samples, source_network_samples,
            "trigger-to-track should copy every graph network event to the target track"
        );
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
            instrument_tensor_params: ScheduledInstrumentTensorParams::new(),
            sampler_params: ScheduledSamplerParams::default(),
            source: EventSource::Network {
                seed: Some((0, 0)),
                neuron: 0,
                instrument_fingerprint: 0,
            },
        };
        event.resolved.transpose = 3.2;
        let mut track_output_events = Vec::new();

        assert!(super::enqueue_neural_output_with_midi_fx(
            &queue,
            &snapshot,
            &mut track_output_events,
            None,
            None,
            0,
            1_000,
            48_000.0,
            0.0,
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
        let mut runtime = lisp_host::ScratchControlRuntime::new(
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
            instrument_tensor_params: ScheduledInstrumentTensorParams::new(),
            sampler_params: ScheduledSamplerParams::default(),
            source: EventSource::Network {
                seed: Some((0, 0)),
                neuron: 0,
                instrument_fingerprint: 0,
            },
        };
        let mut track_output_events = Vec::new();

        assert!(enqueue_step_event_with_midi_fx(
            &queue,
            &snapshot,
            &mut track_output_events,
            Some(&mut runtime),
            None,
            0,
            1_000,
            0.0,
            48_000.0,
            0.0,
            0.0,
            event,
            Vec::new(),
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
        let mut runtime = lisp_host::ScratchControlRuntime::new(
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
            instrument_tensor_params: ScheduledInstrumentTensorParams::new(),
            sampler_params: ScheduledSamplerParams::default(),
            source: EventSource::Network {
                seed: Some((0, 0)),
                neuron: 0,
                instrument_fingerprint: 0,
            },
        };
        let mut track_output_events = Vec::new();

        assert!(enqueue_step_event_with_midi_fx(
            &queue,
            &snapshot,
            &mut track_output_events,
            Some(&mut runtime),
            None,
            0,
            1_000,
            0.0,
            48_000.0,
            0.0,
            0.0,
            event,
            Vec::new(),
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
        let mut runtime = lisp_host::ScratchControlRuntime::new(
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
            midi_fx_params: Vec::new(),
            effect_params: Vec::new(),
            instrument_params: ScheduledInstrumentParams::new(),
            instrument_tensor_params: ScheduledInstrumentTensorParams::new(),
            sampler_params: ScheduledSamplerParams::default(),
            source: EventSource::Network {
                seed: Some((0, 0)),
                neuron: 0,
                instrument_fingerprint: 0,
            },
        };

        let events =
            run_midi_fx_chain_for_track(&mut runtime, &snapshot, 0, vec![event], None, 0, false);
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
        let mut runtime = lisp_host::ScratchControlRuntime::new(
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
            midi_fx_params: Vec::new(),
            effect_params: Vec::new(),
            instrument_params: ScheduledInstrumentParams::new(),
            instrument_tensor_params: ScheduledInstrumentTensorParams::new(),
            sampler_params: ScheduledSamplerParams::default(),
            source: EventSource::Step {
                track: 0,
                step: 0,
                instrument_fingerprint: 0,
            },
        };

        let events =
            run_midi_fx_chain_for_track(&mut runtime, &snapshot, 0, vec![event], None, 0, false);
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
    fn resolve_sampler_params_carries_beats_warp_controls_by_node_param() {
        let state = SequencerState::new(1, vec![default_empty_effect_chain()]);
        let track = 0;
        let step = 3;
        let desc = EffectDescriptor::builtin_sampler();
        let param_idx = |node_idx: u64| {
            desc.params
                .iter()
                .position(|param| param.node_param_idx == node_idx as u32)
                .expect("sampler param should exist")
        };
        let preserve_idx = param_idx(crate::sampler::PARAM_WARP_PRESERVE);
        let fill_idx = param_idx(crate::sampler::PARAM_WARP_SEG_LOOP_MODE);
        let decay_idx = param_idx(crate::sampler::PARAM_WARP_SEG_ENVELOPE);
        let slot = &state.pattern.instrument_slots[track];
        slot.apply_descriptor(&desc, 12);
        slot.defaults
            .set(preserve_idx, crate::warp_grid::PRESERVE_1_8 as f32);
        slot.defaults
            .set(fill_idx, crate::sampler::SEG_LOOP_PINGPONG as f32);
        slot.defaults.set(decay_idx, 0.25);

        let snapshot = state.publish_scheduler_snapshot();
        let params = resolve_sampler_params(&snapshot, track, step);
        assert_eq!(params.warp_preserve, crate::warp_grid::PRESERVE_1_8 as f32);
        assert_eq!(
            params.warp_seg_loop_mode,
            crate::sampler::SEG_LOOP_PINGPONG as f32
        );
        assert!((params.warp_seg_envelope - 0.25).abs() < 0.0001);

        slot.set_plock(step, preserve_idx, crate::warp_grid::PRESERVE_1_16 as f32);
        slot.set_plock(step, fill_idx, crate::sampler::SEG_LOOP_OFF as f32);
        slot.set_plock(step, decay_idx, 0.75);

        let snapshot = state.publish_scheduler_snapshot();
        let params = resolve_sampler_params(&snapshot, track, step);
        assert_eq!(params.warp_preserve, crate::warp_grid::PRESERVE_1_16 as f32);
        assert_eq!(
            params.warp_seg_loop_mode,
            crate::sampler::SEG_LOOP_OFF as f32
        );
        assert!((params.warp_seg_envelope - 0.75).abs() < 0.0001);
    }

    #[test]
    fn enqueue_step_event_step_source_carries_sampler_params() {
        let state = SequencerState::new(1, vec![default_empty_effect_chain()]);
        let snapshot = state.publish_scheduler_snapshot();
        let queue = ScheduledEventQueue::<4>::new();
        let mut track_output_events = Vec::new();
        let mut sampler_params = ScheduledSamplerParams::default();
        sampler_params.warp_enabled = 1.0;
        sampler_params.warp_mode = crate::sampler::WARP_MODE_BEATS as f32;
        sampler_params.sample_bpm = 174.0;
        sampler_params.warp_preserve = crate::warp_grid::PRESERVE_1_16 as f32;
        sampler_params.warp_seg_loop_mode = crate::sampler::SEG_LOOP_PINGPONG as f32;
        sampler_params.warp_seg_envelope = 0.5;

        let event = StepEvent {
            track: 0,
            samples_per_step: 6_000.0,
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
            instrument_tensor_params: ScheduledInstrumentTensorParams::new(),
            sampler_params,
            source: EventSource::Step {
                track: 0,
                step: 0,
                instrument_fingerprint: 0,
            },
        };

        assert!(super::enqueue_step_event(
            &queue,
            &snapshot,
            &mut track_output_events,
            0,
            1_000,
            0.0,
            48_000.0,
            0.0,
            event,
        ));

        let scheduled = queue.pop().expect("scheduled step trigger");
        let ScheduledEventKind::ResolvedTrigger { sampler_params, .. } = scheduled.kind else {
            panic!("expected resolved trigger");
        };
        assert_eq!(
            sampler_params.warp_preserve,
            crate::warp_grid::PRESERVE_1_16 as f32
        );
        assert_eq!(
            sampler_params.warp_seg_loop_mode,
            crate::sampler::SEG_LOOP_PINGPONG as f32
        );
        assert!((sampler_params.warp_seg_envelope - 0.5).abs() < 0.0001);
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
            tensor_params: Vec::new(),
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
            instrument_tensor_params: ScheduledInstrumentTensorParams::new(),
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
            tensor_params: Vec::new(),
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
            instrument_tensor_params: ScheduledInstrumentTensorParams::new(),
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
            instrument_tensor_params: ScheduledInstrumentTensorParams::new(),
            sampler_params: ScheduledSamplerParams::default(),
            source: EventSource::Network {
                seed: None,
                neuron: 0,
                instrument_fingerprint: 0,
            },
        };
        let queue = ScheduledEventQueue::<8>::new();
        let mut track_output_events = Vec::new();

        assert!(super::enqueue_neural_output_with_midi_fx(
            &queue,
            &snapshot,
            &mut track_output_events,
            None,
            None,
            7,
            1234,
            48_000.0,
            0.0,
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
                instrument_tensor_params,
            } => {
                assert_eq!(track, 1);
                assert!(instrument_tensor_params.is_empty());
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
            instrument_tensor_params: ScheduledInstrumentTensorParams::new(),
            sampler_params: ScheduledSamplerParams::default(),
            source: EventSource::Network {
                seed: Some((0, 0)),
                neuron: 0,
                instrument_fingerprint: 0,
            },
        };
        let queue = ScheduledEventQueue::<8>::new();
        let mut track_output_events = Vec::new();

        assert!(super::enqueue_neural_output_with_midi_fx(
            &queue,
            &snapshot,
            &mut track_output_events,
            None,
            None,
            7,
            1234,
            48_000.0,
            0.0,
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
            tensor_params: Vec::new(),
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
    fn resolve_instrument_tensor_params_uses_default_and_step_plocked_matrix() {
        let state = SequencerState::new(1, vec![default_empty_effect_chain()]);
        let track = 0;
        let step = 9;
        let desc = EffectDescriptor {
            name: "tensor instrument".to_string(),
            input_channels: 0,
            output_channels: 2,
            instrument_modulators: Vec::new(),
            instrument_modulation_targets: Vec::new(),
            tensor_params: vec![TensorParamDescriptor {
                name: "strike_mask".to_string(),
                shape: vec![2, 2],
                cell_offset: 64,
                default: vec![0.1, 0.2, 0.3, 0.4],
                min: 0.0,
                max: 1.0,
            }],
            params: Vec::new(),
        };
        state.pattern.instrument_slots[track].apply_descriptor(&desc, 12);
        state.pattern.instrument_slots[track]
            .tensor_params
            .set_plock_cell(step, 0, 1, 0.95)
            .expect("tensor p-lock edit");

        let snapshot = state.publish_scheduler_snapshot();
        let defaults = super::resolve_instrument_tensor_params(&snapshot, track, 0);
        let plocked = super::resolve_instrument_tensor_params(&snapshot, track, step);
        let explicit_plocks = super::resolve_instrument_tensor_plocks(&snapshot, track, step);
        let default_only = super::resolve_instrument_tensor_defaults(&snapshot, track);

        assert_eq!(defaults.len(), 1);
        assert_eq!(defaults[0].cell_offset, 64);
        assert_eq!(defaults[0].values, vec![0.1, 0.2, 0.3, 0.4]);
        assert_eq!(default_only.as_slice(), defaults.as_slice());
        assert_eq!(plocked.len(), 1);
        assert_eq!(plocked[0].values, vec![0.1, 0.95, 0.3, 0.4]);
        assert_eq!(explicit_plocks.as_slice(), plocked.as_slice());
        assert_ne!(
            super::instrument_sound_fingerprint(
                &snapshot,
                track,
                ScheduledInstrumentParams::new().as_slice(),
                defaults.as_slice(),
            ),
            super::instrument_sound_fingerprint(
                &snapshot,
                track,
                ScheduledInstrumentParams::new().as_slice(),
                plocked.as_slice(),
            )
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
        state.pattern.track_params[track].set_midi_fx_chain(vec!["arp".to_string()]);
        let midi_fx_descriptors = lisp_host::load_midi_fx_descriptors();
        let arp_desc = midi_fx_descriptors
            .iter()
            .find(|desc| desc.name == "arp")
            .expect("arp descriptor");
        state.pattern.midi_fx_slots[track][0].apply_descriptor(arp_desc, 0);
        state.pattern.midi_fx_slots[track][0].defaults.set(0, 4.0);

        state.pattern.patterns[track].set_step_active(0, true);
        state.pattern.chord_data[track].add_note_with_duration(0, 0.0, 8.0);
        state.pattern.chord_data[track].add_note_with_duration(0, 4.0, 8.0);
        state.pattern.chord_data[track].add_note_with_duration(0, 7.0, 8.0);
        state.pattern.step_data[track].set(0, StepParam::Duration, 8.0);

        let snapshot = state.publish_scheduler_snapshot();
        let events = midi_fx_window_events_from_step(
            &snapshot,
            &midi_fx_descriptors,
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
            ScheduledInstrumentTensorParams::new(),
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
    fn midi_fx_window_events_do_not_treat_event_param_as_tick_rate() {
        let state = SequencerState::new(1, vec![default_empty_effect_chain()]);
        let track = 0;
        state.pattern.track_params[track].set_midi_fx_chain(vec!["trigger-to-track".to_string()]);
        let midi_fx_descriptors = lisp_host::load_midi_fx_descriptors();
        let trigger_desc = midi_fx_descriptors
            .iter()
            .find(|desc| desc.name == "trigger-to-track")
            .expect("trigger-to-track descriptor");
        state.pattern.midi_fx_slots[track][0].apply_descriptor(trigger_desc, 0);
        state.pattern.midi_fx_slots[track][0].defaults.set(0, 6.0);

        state.pattern.patterns[track].set_step_active(0, true);
        state.pattern.chord_data[track].add_note_with_duration(0, 0.0, 8.0);
        state.pattern.chord_data[track].add_note_with_duration(0, 7.0, 8.0);
        state.pattern.step_data[track].set(0, StepParam::Duration, 8.0);

        let snapshot = state.publish_scheduler_snapshot();
        assert!(
            super::midi_fx_clock_tick_beats(&snapshot, &midi_fx_descriptors, track, 0).is_none()
        );

        let events = midi_fx_window_events_from_step(
            &snapshot,
            &midi_fx_descriptors,
            track,
            0,
            48_000.0,
            2.0,
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
            ScheduledInstrumentTensorParams::new(),
        );

        assert_eq!(events.len(), 1);
        assert_eq!(events[0].samples_per_step, 48_000.0);
        assert_eq!(events[0].step_beats, 2.0);
        assert_eq!(
            events[0]
                .note_spans
                .as_ref()
                .expect("source note spans")
                .len(),
            2
        );
    }

    #[test]
    fn midi_fx_window_events_do_not_clock_spatial_harmonic_delay() {
        let state = SequencerState::new(1, vec![default_empty_effect_chain()]);
        let track = 0;
        state.pattern.track_params[track]
            .set_midi_fx_chain(vec!["spatial-harmonic-delay".to_string()]);
        let midi_fx_descriptors = lisp_host::load_midi_fx_descriptors();
        let spatial_desc = midi_fx_descriptors
            .iter()
            .find(|desc| desc.name == "spatial-harmonic-delay")
            .expect("spatial-harmonic-delay descriptor");
        state.pattern.midi_fx_slots[track][0].apply_descriptor(spatial_desc, 0);
        state.pattern.midi_fx_slots[track][0].defaults.set(0, 4.0);

        state.pattern.patterns[track].set_step_active(0, true);
        state.pattern.chord_data[track].add_note_with_duration(0, 0.0, 4.0);
        state.pattern.step_data[track].set(0, StepParam::Duration, 4.0);

        let snapshot = state.publish_scheduler_snapshot();
        assert!(
            super::midi_fx_clock_tick_beats(&snapshot, &midi_fx_descriptors, track, 0).is_none()
        );

        let events = midi_fx_window_events_from_step(
            &snapshot,
            &midi_fx_descriptors,
            track,
            0,
            48_000.0,
            1.0,
            48_000.0,
            0.0,
            ResolvedStep {
                duration: 4.0,
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
            ScheduledInstrumentTensorParams::new(),
        );

        assert_eq!(events.len(), 1);
        assert_eq!(events[0].samples_per_step, 48_000.0);
        assert_eq!(events[0].step_beats, 1.0);
        let spans = events[0].note_spans.as_ref().expect("source note spans");
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].start_beats, 0.0);
        assert_eq!(spans[0].end_beats, 1.0);
    }

    #[test]
    fn midi_fx_window_events_clock_beat_repeat_over_source_duration() {
        let state = SequencerState::new(1, vec![default_empty_effect_chain()]);
        let track = 0;
        state.pattern.track_params[track].set_midi_fx_chain(vec!["beat-repeat".to_string()]);
        let midi_fx_descriptors = lisp_host::load_midi_fx_descriptors();
        let repeat_desc = midi_fx_descriptors
            .iter()
            .find(|desc| desc.name == "beat-repeat")
            .expect("beat-repeat descriptor");
        state.pattern.midi_fx_slots[track][0].apply_descriptor(repeat_desc, 0);
        state.pattern.midi_fx_slots[track][0].defaults.set(0, 4.0);

        state.pattern.patterns[track].set_step_active(0, true);
        state.pattern.chord_data[track].add_note_with_duration(0, 0.0, 4.0);
        state.pattern.step_data[track].set(0, StepParam::Duration, 4.0);

        let snapshot = state.publish_scheduler_snapshot();
        assert_eq!(
            super::midi_fx_clock_tick_beats(&snapshot, &midi_fx_descriptors, track, 0),
            Some(0.25)
        );

        let events = midi_fx_window_events_from_step(
            &snapshot,
            &midi_fx_descriptors,
            track,
            0,
            48_000.0,
            1.0,
            48_000.0,
            0.0,
            ResolvedStep {
                duration: 4.0,
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
            ScheduledInstrumentTensorParams::new(),
        );

        assert_eq!(events.len(), 4);
        for (idx, event) in events.iter().enumerate() {
            assert_eq!(event.offset_beats, idx as f32 * 0.25);
            assert_eq!(event.samples_per_step, 12_000.0);
            assert_eq!(event.step_beats, 0.25);
            assert_eq!(event.resolved.duration, 1.0);
            assert_eq!(event.chord, vec![0.0]);
        }
    }

    #[test]
    fn event_driven_live_midi_fx_processes_pending_note_once() {
        let state = Arc::new(SequencerState::new(
            2,
            vec![default_empty_effect_chain(), default_empty_effect_chain()],
        ));
        state.pattern.track_params[0].set_midi_fx_chain(vec!["trigger-to-track".to_string()]);
        let trigger_desc = lisp_host::load_midi_fx_descriptor("trigger-to-track")
            .expect("trigger-to-track descriptor");
        state.pattern.midi_fx_slots[0][0].apply_descriptor(&trigger_desc, 0);
        state.pattern.midi_fx_slots[0][0].defaults.set(0, 2.0);
        let snapshot = state.publish_scheduler_snapshot();
        let mut runtime = lisp_host::ScratchControlRuntime::new(
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
            .eval(&lisp_host::load_midi_fx_library_source())
            .unwrap();
        let queue = ScheduledEventQueue::<8>::new();
        let mut live_tracks: [super::LiveMidiFxTrackState; MAX_TRACKS] =
            std::array::from_fn(|_| super::LiveMidiFxTrackState::default());
        live_tracks[0].notes.push(super::LiveMidiFxNote {
            transpose: 7.0,
            velocity: 0.8,
            pending_event: true,
        });

        assert!(super::schedule_live_midi_fx(
            Some(&mut runtime),
            &state,
            &snapshot,
            &queue,
            0,
            1_000,
            0.0,
            512,
            48_000,
            &mut live_tracks,
            false,
        ));

        let mut tracks = Vec::new();
        while let Some(event) = queue.pop() {
            match event.kind {
                ScheduledEventKind::ResolvedTrigger {
                    track, resolved, ..
                } => {
                    assert_eq!(event.sample_time, 1_000);
                    assert_eq!(resolved.transpose, 7.0);
                    tracks.push(track);
                }
                other => panic!("expected resolved trigger, got {other:?}"),
            }
        }
        tracks.sort_unstable();
        assert_eq!(tracks, vec![0, 1]);

        assert!(super::schedule_live_midi_fx(
            Some(&mut runtime),
            &state,
            &snapshot,
            &queue,
            0,
            1_256,
            0.0,
            512,
            48_000,
            &mut live_tracks,
            false,
        ));
        assert!(queue.pop().is_none());
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
