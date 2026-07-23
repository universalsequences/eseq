pub mod audiograph;
pub mod engine;
mod device;
#[allow(unused_imports)]
use device::*;
mod graph_dispatch;
#[allow(unused_imports)]
use graph_dispatch::*;
mod state;
#[allow(unused_imports)]
use state::*;
mod voice_pool;
#[allow(unused_imports)]
use voice_pool::*;

use arrayvec::ArrayVec;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::Stream;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use crate::audiograph::*;
use crate::effects::gatepitch;
use crate::effects::{EffectSlotSnapshot, EffectSlotState, MAX_SLOT_PARAMS};
use crate::recorder::MasterRecorder;
use crate::sampler::{
    PARAM_ATTACK_SAMPLES, PARAM_LOOP_XFADE_SAMPLES, PARAM_RELEASE_SAMPLES, PARAM_WARP_PROJECT_BPM,
    SAMPLER_EVENT_AUX_ATTACK_SAMPLES, SAMPLER_EVENT_AUX_ENABLED, SAMPLER_EVENT_AUX_END_POINT,
    SAMPLER_EVENT_AUX_GATE_MODE, SAMPLER_EVENT_AUX_GATE_SAMPLES, SAMPLER_EVENT_AUX_LOOP_MODE,
    SAMPLER_EVENT_AUX_LOOP_XFADE_SAMPLES, SAMPLER_EVENT_AUX_NOTE_ON_COUNT,
    SAMPLER_EVENT_AUX_RELEASE_SAMPLES, SAMPLER_EVENT_AUX_REVERSE, SAMPLER_EVENT_AUX_SCRUB_OFFSET,
    SAMPLER_EVENT_AUX_SPEED, SAMPLER_EVENT_AUX_SR_HZ, SAMPLER_EVENT_AUX_START_POINT,
    SAMPLER_EVENT_AUX_TRANSPOSE, SAMPLER_EVENT_AUX_VELOCITY, SAMPLER_EVENT_AUX_WARP_ENABLED,
    SAMPLER_EVENT_AUX_WARP_MODE, SAMPLER_EVENT_AUX_WARP_PRESERVE,
    SAMPLER_EVENT_AUX_WARP_PROJECT_BPM, SAMPLER_EVENT_AUX_WARP_PTR_HI,
    SAMPLER_EVENT_AUX_WARP_PTR_LO, SAMPLER_EVENT_AUX_WARP_RATIO, SAMPLER_EVENT_AUX_WARP_SAMPLE_BPM,
    SAMPLER_EVENT_AUX_WARP_SEG_ENVELOPE, SAMPLER_EVENT_AUX_WARP_SEG_LOOP_MODE,
};
use crate::scheduled_event::{
    resolved_chord_transpose, ScheduledEffectParam, ScheduledEvent, ScheduledEventKind,
    ScheduledEventQueue, ScheduledInstrumentParam, ScheduledInstrumentParamTarget,
    ScheduledInstrumentParams, ScheduledInstrumentTensorParam, ScheduledInstrumentTensorParams,
    ScheduledSamplerParams,
};
use crate::sequencer::{
    rack_slot_pool_index, sync_beats, BusId, CustomInstrumentRunMode, InstrumentType,
    KeyboardTrigger, RackRouting, RackSlotParam, RackSlotSnapshot, RackTrackSnapshot,
    SequencerSnapshot, SequencerState, StepParam, SwingResolution, MAX_INSTRUMENT_ENGINES,
    MAX_RACK_SLOTS, MAX_SAMPLER_POOLS, MAX_TRACKS,
};
use crate::app::BusGateRuntimeState;
use crate::voice::{VoicePool, MAX_VOICES};

pub const FALLBACK_SAMPLE_RATE: u32 = 44_100;
const CUSTOM_ENGINE_RELEASE_TAIL_SECONDS: f64 = 20.0;
const SCHEDULED_EVENT_QUEUE_CAPACITY: usize = 4096;
const SCHEDULED_COUNTDOWN_CAPACITY: usize =
    SCHEDULED_EVENT_QUEUE_CAPACITY + MAX_TRACKS * MAX_VOICES * 2 + MAX_TRACKS;
const SCHEDULED_BLOCK_SCRATCH_CAPACITY: usize =
    SCHEDULED_EVENT_QUEUE_CAPACITY + MAX_TRACKS * MAX_VOICES * 2 + MAX_TRACKS;

#[derive(Clone, Copy, Debug)]
enum GateOffTarget {
    Custom { engine_id: usize, free_patch: bool },
    Sampler { gatepitch_id: i32 },
}

#[derive(Clone, Copy, Debug)]
struct GateOffEvent {
    track_idx: usize,
    logical_id: u64,
    target: GateOffTarget,
}

#[derive(Clone, Copy, Debug)]
struct ChopEvent {
    track_idx: usize,
    step: usize,
    chop_gate: f32,
}

#[derive(Debug)]
enum CountdownEventKind {
    Scheduled(ScheduledEvent),
    GateOff(GateOffEvent),
    Chop(ChopEvent),
}

#[derive(Debug)]
struct CountdownEvent {
    remaining_samples: f64,
    period_samples: f64,
    repeats: u32,
    pattern_epoch: u64,
    seq: u64,
    kind: CountdownEventKind,
}

#[derive(Debug)]
enum BlockEventKind {
    Scheduled(ScheduledEvent),
    GateOff(GateOffEvent),
    Chop(ChopEvent),
}

#[derive(Debug)]
struct BlockEvent {
    frame_offset: u32,
    seq: u64,
    kind: BlockEventKind,
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

fn cancel_gate_off_for_lid(
    countdown_events: &mut Vec<CountdownEvent>,
    block_events: &mut Vec<BlockEvent>,
    lid: u64,
) {
    countdown_events.retain(|event| {
        !matches!(
            event.kind,
            CountdownEventKind::GateOff(GateOffEvent { logical_id, .. }) if logical_id == lid
        )
    });
    block_events.retain(|event| {
        !matches!(
            event.kind,
            BlockEventKind::GateOff(GateOffEvent { logical_id, .. }) if logical_id == lid
        )
    });
}

fn cancel_chops_for_track(
    countdown_events: &mut Vec<CountdownEvent>,
    block_events: &mut Vec<BlockEvent>,
    track_idx: usize,
) {
    countdown_events.retain(|event| {
        !matches!(
            event.kind,
            CountdownEventKind::Chop(ChopEvent { track_idx: event_track, .. }) if event_track == track_idx
        )
    });
    block_events.retain(|event| {
        !matches!(
            event.kind,
            BlockEventKind::Chop(ChopEvent { track_idx: event_track, .. }) if event_track == track_idx
        )
    });
}

fn schedule_gate_off_event(
    data: &mut AudioCallbackData,
    track_idx: usize,
    logical_id: u64,
    source_frame_offset: u32,
    delay_samples: f64,
    target: GateOffTarget,
) {
    cancel_gate_off_for_lid(
        &mut data.countdown_events,
        &mut data.block_events,
        logical_id,
    );
    let event_offset = source_frame_offset as f64 + delay_samples.max(0.0);
    schedule_countdown_or_block_event(
        data,
        event_offset,
        0.0,
        1,
        data.state.transport.pattern_epoch.load(Ordering::Relaxed),
        CountdownEventKind::GateOff(GateOffEvent {
            track_idx,
            logical_id,
            target,
        }),
    );
}

fn schedule_chop_events(
    data: &mut AudioCallbackData,
    track_idx: usize,
    source_frame_offset: u32,
    first_delay_samples: f64,
    interval_samples: f64,
    repeats: u32,
    step: usize,
    chop_gate: f32,
) {
    cancel_chops_for_track(
        &mut data.countdown_events,
        &mut data.block_events,
        track_idx,
    );
    if repeats == 0 {
        return;
    }
    schedule_countdown_or_block_event(
        data,
        source_frame_offset as f64 + first_delay_samples.max(0.0),
        interval_samples.max(1.0),
        repeats,
        data.state.transport.pattern_epoch.load(Ordering::Relaxed),
        CountdownEventKind::Chop(ChopEvent {
            track_idx,
            step,
            chop_gate,
        }),
    );
}

fn dispatch_gate_off_event(
    data: &mut AudioCallbackData,
    event: GateOffEvent,
    frame_offset: u32,
    block_start_sample: u64,
) {
    match event.target {
        GateOffTarget::Custom {
            engine_id,
            free_patch,
        } => {
            if engine_id >= data.custom_engine_pools.len() {
                return;
            }
            if free_patch {
                data.custom_engine_pools[engine_id]
                    .release_free_patch_voice_by_logical_id(event.logical_id);
            } else {
                data.custom_engine_pools[engine_id].release_voice_by_logical_id(
                    event.logical_id,
                    block_start_sample + frame_offset as u64,
                );
            }
            let seq = next_block_event_sequence(data);
            unsafe {
                send_custom_note_off(data.lg.0, event.logical_id, frame_offset, seq);
            }
        }
        GateOffTarget::Sampler { gatepitch_id } => {
            if gatepitch_id > 0 {
                let seq = next_block_event_sequence(data);
                unsafe {
                    send_custom_note_off(data.lg.0, gatepitch_id as u64, frame_offset, seq);
                }
            }
            if event.track_idx >= data.voice_pools.len() {
                return;
            }
            data.voice_pools[event.track_idx].release_voice_by_logical_id(event.logical_id);
            let seq = next_block_event_sequence(data);
            unsafe {
                send_sampler_note_off(data.lg.0, event.logical_id, frame_offset, seq);
            }
        }
    }
}

fn custom_pitch_hz(transpose: f32, base_note_offset: f32) -> f32 {
    440.0 * 2f32.powf((transpose + base_note_offset) / 12.0)
}

fn custom_pitch_midi_note(transpose: f32, base_note_offset: f32) -> u8 {
    (transpose + base_note_offset + 60.0)
        .round()
        .clamp(0.0, 127.0) as u8
}

fn track_accepts_scheduled_trigger(state: &SequencerState, track_idx: usize) -> bool {
    let Some(track_params) = state.pattern.track_params.get(track_idx) else {
        return false;
    };
    if track_params.is_muted() {
        return false;
    }
    let has_solo = state
        .pattern
        .track_params
        .iter()
        .take(state.active_track_count())
        .any(|params| params.is_solo());
    !has_solo || track_params.is_solo()
}

fn resolve_live_keyboard_transpose(
    state: &SequencerState,
    accumulator_state: crate::accumulator::AccumulatorRuntimeState,
    track_idx: usize,
    raw_transpose: f32,
) -> f32 {
    let tp = &state.pattern.track_params[track_idx];
    let accum_idx = tp.get_accumulator_idx();
    let with_accumulator = match crate::accumulator::ACCUMULATOR_REGISTRY.get(accum_idx) {
        Some(def) if def.name == "TransposeRamp" => raw_transpose + accumulator_state.value,
        _ => raw_transpose,
    };
    let fts = tp.get_fts_scale();
    if fts > 0 {
        crate::scale::quantize_transpose(with_accumulator, fts)
    } else {
        with_accumulator
    }
}

fn track_engine_id(state: &SequencerState, track_idx: usize) -> Option<usize> {
    let engine_id = state.runtime.track_engine_ids[track_idx].load(Ordering::Relaxed);
    if engine_id == u32::MAX {
        None
    } else {
        Some(engine_id as usize)
    }
}

fn track_custom_run_mode(state: &SequencerState, track_idx: usize) -> CustomInstrumentRunMode {
    CustomInstrumentRunMode::from_runtime_flag(
        state.runtime.instrument_run_mode_flags[track_idx].load(Ordering::Relaxed),
    )
}

fn sampler_warp_runtime(
    state: &SequencerState,
    track_idx: usize,
    warp_enabled: f32,
    warp_mode: f32,
    sample_bpm: f32,
) -> (f32, f32, f32, f32, f32, f32, f32) {
    let project_bpm = state.transport.bpm.load(Ordering::Relaxed).max(1) as f32;
    let sample_bpm = sample_bpm.clamp(20.0, 400.0);
    if warp_enabled <= 0.5 {
        return (0.0, warp_mode, 1.0, sample_bpm, project_bpm, 0.0, 0.0);
    }
    let ratio = (project_bpm / sample_bpm).clamp(0.01, 32.0);
    if warp_mode.round() != 0.0 {
        // Non-onset warp modes (re-pitch family) need no analysis or onset table.
        return (1.0, warp_mode, ratio, sample_bpm, project_bpm, 0.0, 0.0);
    }
    // Beats runs off the beat grid (bpm-only); the onset table is attached
    // when analysis is ready so Preserve=Transients can snap to it, but its
    // absence no longer disables warp.
    let status = state.runtime.sampler_analysis_status[track_idx].load(Ordering::Acquire);
    let (ptr_lo, ptr_hi) = if status == 2 {
        (
            f32::from_bits(state.runtime.sampler_onset_ptr_lo[track_idx].load(Ordering::Acquire)),
            f32::from_bits(state.runtime.sampler_onset_ptr_hi[track_idx].load(Ordering::Acquire)),
        )
    } else {
        (0.0, 0.0)
    };
    (
        1.0,
        warp_mode,
        ratio,
        sample_bpm,
        project_bpm,
        ptr_lo,
        ptr_hi,
    )
}

fn instrument_sound_fingerprint(
    state: &SequencerState,
    track_idx: usize,
    engine_id: usize,
    step: Option<usize>,
) -> u64 {
    let mut hasher = DefaultHasher::new();
    engine_id.hash(&mut hasher);
    state.pattern.instrument_base_note_offsets[track_idx]
        .load(Ordering::Relaxed)
        .hash(&mut hasher);

    let slot = &state.pattern.instrument_slots[track_idx];
    let num_params = slot.num_params.load(Ordering::Relaxed) as usize;
    for param_idx in 0..num_params {
        let value_bits = if let Some(step_idx) = step {
            slot.plocks
                .get(step_idx, param_idx)
                .unwrap_or_else(|| slot.defaults.get(param_idx))
                .to_bits()
        } else {
            slot.defaults.get(param_idx).to_bits()
        };
        value_bits.hash(&mut hasher);
    }
    let num_tensors = slot.tensor_params.num_params();
    for tensor_idx in 0..num_tensors {
        if let Some(cell_offset) = slot.tensor_params.tensor_cell_offset(tensor_idx) {
            cell_offset.hash(&mut hasher);
        }
        if let Some(values) = step
            .and_then(|step_idx| slot.tensor_params.plock_values(step_idx, tensor_idx))
            .or_else(|| slot.tensor_params.default_values(tensor_idx))
        {
            for value in values {
                value.to_bits().hash(&mut hasher);
            }
        }
    }

    hasher.finish()
}

fn slot_param_identity(
    node_id: u32,
    modulator_node_id: u32,
    raw_idx: u32,
) -> Option<crate::neural::ParamNodeId> {
    if raw_idx == u32::MAX {
        return None;
    }
    if raw_idx >= crate::voice_modulator::MOD_PARAM_BASE {
        if modulator_node_id == 0 {
            return None;
        }
        Some(crate::neural::ParamNodeId {
            logical_id: modulator_node_id as u64,
            node_param_idx: raw_idx - crate::voice_modulator::MOD_PARAM_BASE,
        })
    } else {
        if node_id == 0 {
            return None;
        }
        Some(crate::neural::ParamNodeId {
            logical_id: node_id as u64,
            node_param_idx: raw_idx,
        })
    }
}

fn plock_identity_matches(
    plock_ids: &[Vec<Option<crate::neural::ParamNodeId>>],
    step_idx: usize,
    param_idx: usize,
    expected: Option<crate::neural::ParamNodeId>,
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
    slot: &EffectSlotSnapshot,
    step_idx: usize,
    param_idx: usize,
    default: f32,
) -> f32 {
    slot.resolved_param_value(step_idx, param_idx, default)
}

fn snapshot_slot_param_index_by_node_idx(
    slot: &EffectSlotSnapshot,
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
    slot: &EffectSlotSnapshot,
    step_idx: usize,
    node_param_idx: u32,
    default: f32,
) -> f32 {
    let Some(param_idx) = snapshot_slot_param_index_by_node_idx(slot, node_param_idx) else {
        return default;
    };
    resolved_slot_param_value(slot, step_idx, param_idx, default)
}

fn default_slot_node_param_value(
    slot: &EffectSlotSnapshot,
    node_param_idx: u32,
    default: f32,
) -> f32 {
    let Some(param_idx) = snapshot_slot_param_index_by_node_idx(slot, node_param_idx) else {
        return default;
    };
    slot.defaults.get(param_idx).copied().unwrap_or(default)
}

fn live_slot_param_index_by_node_idx(slot: &EffectSlotState, node_param_idx: u64) -> Option<usize> {
    let num_params = slot.num_params.load(Ordering::Relaxed) as usize;
    (0..num_params).find(|&param_idx| slot.resolve_node_idx(param_idx) == node_param_idx)
}

fn live_slot_resolved_param_value(
    slot: &EffectSlotState,
    step_idx: usize,
    param_idx: usize,
    default: f32,
) -> f32 {
    let num_params = slot.num_params.load(Ordering::Relaxed) as usize;
    if param_idx >= num_params {
        return default;
    }
    let default_value = slot.defaults.get(param_idx);
    let Some(plock) = slot.plocks.get(step_idx, param_idx) else {
        return default_value;
    };
    let expected_id = slot.param_node_id(param_idx);
    if expected_id.is_some() && slot.plocks.get_id(step_idx, param_idx) == expected_id {
        plock
    } else {
        default_value
    }
}

fn live_slot_resolved_node_param_value(
    slot: &EffectSlotState,
    step_idx: usize,
    node_param_idx: u64,
    default: f32,
) -> f32 {
    let Some(param_idx) = live_slot_param_index_by_node_idx(slot, node_param_idx) else {
        return default;
    };
    live_slot_resolved_param_value(slot, step_idx, param_idx, default)
}

fn live_slot_default_node_param_value(
    slot: &EffectSlotState,
    node_param_idx: u64,
    default: f32,
) -> f32 {
    let Some(param_idx) = live_slot_param_index_by_node_idx(slot, node_param_idx) else {
        return default;
    };
    slot.defaults.get(param_idx)
}

fn snapshot_slot_default_node_param_value(
    slot: &EffectSlotSnapshot,
    node_param_idx: u64,
    default: f32,
) -> f32 {
    let Some(param_idx) = slot
        .param_node_indices
        .iter()
        .position(|idx| u64::from(*idx) == node_param_idx)
    else {
        return default;
    };
    slot.defaults.get(param_idx).copied().unwrap_or(default)
}

fn key_lock_identity_matches(
    key_lock_ids: &std::collections::BTreeMap<u8, Vec<Option<crate::neural::ParamNodeId>>>,
    note: u8,
    param_idx: usize,
    expected: Option<crate::neural::ParamNodeId>,
) -> bool {
    let Some(expected) = expected else {
        return false;
    };
    key_lock_ids
        .get(&note)
        .and_then(|row| row.get(param_idx))
        .copied()
        .flatten()
        == Some(expected)
}

fn live_param_route(
    slot: &EffectSlotState,
    param_idx: usize,
) -> Option<(ScheduledInstrumentParamTarget, u64, u32)> {
    let raw_idx = slot.resolve_node_idx(param_idx);
    if raw_idx == u32::MAX as u64 {
        return None;
    }
    let span = slot.resolve_node_span(param_idx);
    if raw_idx >= crate::voice_modulator::MOD_PARAM_BASE as u64 {
        Some((
            ScheduledInstrumentParamTarget::Modulator,
            raw_idx - crate::voice_modulator::MOD_PARAM_BASE as u64,
            span,
        ))
    } else {
        Some((ScheduledInstrumentParamTarget::Synth, raw_idx, span))
    }
}

fn snapshot_param_route(
    slot: &EffectSlotSnapshot,
    param_idx: usize,
) -> Option<(ScheduledInstrumentParamTarget, u64, u32)> {
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
    if raw_idx >= crate::voice_modulator::MOD_PARAM_BASE {
        Some((
            ScheduledInstrumentParamTarget::Modulator,
            (raw_idx - crate::voice_modulator::MOD_PARAM_BASE) as u64,
            span,
        ))
    } else {
        Some((ScheduledInstrumentParamTarget::Synth, raw_idx as u64, span))
    }
}

fn live_step_has_valid_plock(
    slot: &EffectSlotState,
    step_idx: Option<usize>,
    param_idx: usize,
) -> bool {
    let Some(step_idx) = step_idx else {
        return false;
    };
    if slot.plocks.get(step_idx, param_idx).is_none() {
        return false;
    }
    let expected_id = slot.param_node_id(param_idx);
    expected_id.is_some() && slot.plocks.get_id(step_idx, param_idx) == expected_id
}

fn snapshot_step_has_valid_plock(
    slot: &EffectSlotSnapshot,
    step_idx: Option<usize>,
    param_idx: usize,
) -> bool {
    let Some(step_idx) = step_idx else {
        return false;
    };
    if slot
        .plocks
        .get(step_idx)
        .and_then(|row| row.get(param_idx))
        .copied()
        .flatten()
        .is_none()
    {
        return false;
    }
    let raw_idx = slot
        .param_node_indices
        .get(param_idx)
        .copied()
        .unwrap_or(param_idx as u32);
    let expected_id = slot_param_identity(slot.node_id, slot.modulator_node_id, raw_idx);
    plock_identity_matches(&slot.plock_param_ids, step_idx, param_idx, expected_id)
}

fn upsert_instrument_param(
    params: &mut ScheduledInstrumentParams,
    target: ScheduledInstrumentParamTarget,
    idx: u64,
    span: u32,
    value: f32,
) {
    if let Some(existing) = params
        .iter_mut()
        .find(|param| param.target == target && param.idx == idx)
    {
        existing.span = span;
        existing.value = value;
        return;
    }
    if params.is_full() {
        return;
    }
    params.push(ScheduledInstrumentParam {
        target,
        idx,
        span,
        value,
    });
    params.sort_by_key(|param| match param.target {
        ScheduledInstrumentParamTarget::Synth => (0_u8, param.idx),
        ScheduledInstrumentParamTarget::Modulator => (1_u8, param.idx),
    });
}

fn key_locked_live_instrument_params(
    state: &SequencerState,
    track_idx: usize,
    transpose: f32,
    base_note_offset: f32,
    step_idx: Option<usize>,
    base_params: &ScheduledInstrumentParams,
) -> ScheduledInstrumentParams {
    let Some(slot) = state.pattern.instrument_slots.get(track_idx) else {
        return base_params.clone();
    };
    let note = custom_pitch_midi_note(transpose, base_note_offset);
    if !slot
        .key_locks
        .note_has_any_lock(note, slot.num_params.load(Ordering::Relaxed) as usize)
    {
        return base_params.clone();
    }

    let mut params = base_params.clone();
    let num_params = slot.num_params.load(Ordering::Relaxed) as usize;
    for param_idx in 0..num_params.min(MAX_SLOT_PARAMS) {
        if live_step_has_valid_plock(slot, step_idx, param_idx) {
            continue;
        }
        let Some(value) = slot.key_locks.get(note, param_idx) else {
            continue;
        };
        if !value.is_finite()
            || slot.key_locks.get_id(note, param_idx) != slot.param_node_id(param_idx)
        {
            continue;
        }
        let Some((target, idx, span)) = live_param_route(slot, param_idx) else {
            continue;
        };
        upsert_instrument_param(&mut params, target, idx, span, value);
    }
    params
}

fn key_locked_snapshot_instrument_params(
    slot: &EffectSlotSnapshot,
    transpose: f32,
    base_note_offset: f32,
    step_idx: Option<usize>,
    base_params: &ScheduledInstrumentParams,
) -> ScheduledInstrumentParams {
    let note = custom_pitch_midi_note(transpose, base_note_offset);
    let Some(row) = slot.key_locks.get(&note) else {
        return base_params.clone();
    };

    let mut params = base_params.clone();
    let num_params = (slot.num_params as usize).min(MAX_SLOT_PARAMS);
    for param_idx in 0..num_params.min(row.len()) {
        if snapshot_step_has_valid_plock(slot, step_idx, param_idx) {
            continue;
        }
        let Some(value) = row[param_idx] else {
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
        let expected_id = slot_param_identity(slot.node_id, slot.modulator_node_id, raw_idx);
        if !key_lock_identity_matches(&slot.key_lock_param_ids, note, param_idx, expected_id) {
            continue;
        }
        let Some((target, idx, span)) = snapshot_param_route(slot, param_idx) else {
            continue;
        };
        upsert_instrument_param(&mut params, target, idx, span, value);
    }
    params
}

fn resolve_live_instrument_defaults(
    state: &SequencerState,
    track_idx: usize,
) -> ScheduledInstrumentParams {
    let Some(slot) = state.pattern.instrument_slots.get(track_idx) else {
        return ScheduledInstrumentParams::new();
    };
    let mut params = ScheduledInstrumentParams::new();
    let num_params = slot.num_params.load(Ordering::Relaxed) as usize;
    for param_idx in 0..num_params.min(MAX_SLOT_PARAMS) {
        let Some((target, idx, span)) = live_param_route(slot, param_idx) else {
            continue;
        };
        let value = slot.defaults.get(param_idx);
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

fn resolve_snapshot_instrument_defaults(
    snapshot: &SequencerSnapshot,
    track_idx: usize,
) -> ScheduledInstrumentParams {
    let Some(slot) = snapshot
        .tracks
        .get(track_idx)
        .map(|track| &track.instrument_slot)
    else {
        return ScheduledInstrumentParams::new();
    };
    let mut params = ScheduledInstrumentParams::new();
    let num_params = (slot.num_params as usize).min(slot.defaults.len());
    for param_idx in 0..num_params {
        let Some((target, idx, span)) = snapshot_param_route(slot, param_idx) else {
            continue;
        };
        let value = slot.defaults[param_idx];
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

fn resolve_live_instrument_tensor_defaults(
    state: &SequencerState,
    track_idx: usize,
) -> ScheduledInstrumentTensorParams {
    let Some(slot) = state.pattern.instrument_slots.get(track_idx) else {
        return ScheduledInstrumentTensorParams::new();
    };
    let mut params = ScheduledInstrumentTensorParams::new();
    let num_tensors = slot.tensor_params.num_params();
    for tensor_idx in 0..num_tensors {
        let Some(cell_offset) = slot.tensor_params.tensor_cell_offset(tensor_idx) else {
            continue;
        };
        let Some(values) = slot.tensor_params.default_values(tensor_idx) else {
            continue;
        };
        if values.iter().any(|value| !value.is_finite()) {
            continue;
        }
        if params.is_full() {
            break;
        }
        params.push(ScheduledInstrumentTensorParam {
            cell_offset,
            values,
        });
    }
    params.sort_by_key(|param| param.cell_offset);
    params
}

fn instrument_param_bundle_fingerprint(
    engine_id: usize,
    base_note_offset: f32,
    instrument_params: &[ScheduledInstrumentParam],
    instrument_tensor_params: &[ScheduledInstrumentTensorParam],
) -> u64 {
    let mut hasher = DefaultHasher::new();
    engine_id.hash(&mut hasher);
    base_note_offset.to_bits().hash(&mut hasher);
    for param in instrument_params {
        param.target.hash(&mut hasher);
        param.idx.hash(&mut hasher);
        param.span.hash(&mut hasher);
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

fn resolve_rack_slot_instrument_params(
    slot: &EffectSlotSnapshot,
    step_idx: usize,
) -> ScheduledInstrumentParams {
    let num_params = slot.num_params as usize;
    let mut params = ScheduledInstrumentParams::new();
    for param_idx in 0..num_params {
        let raw_idx = slot
            .param_node_indices
            .get(param_idx)
            .copied()
            .unwrap_or(param_idx as u32);
        if raw_idx == u32::MAX {
            continue;
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

fn resolve_rack_slot_instrument_defaults(slot: &EffectSlotSnapshot) -> ScheduledInstrumentParams {
    let num_params = slot.num_params as usize;
    let mut params = ScheduledInstrumentParams::new();
    for param_idx in 0..num_params {
        let raw_idx = slot
            .param_node_indices
            .get(param_idx)
            .copied()
            .unwrap_or(param_idx as u32);
        if raw_idx == u32::MAX {
            continue;
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

fn resolve_rack_slot_sampler_params(
    slot: &EffectSlotSnapshot,
    step_idx: usize,
) -> ScheduledSamplerParams {
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

fn resolve_rack_slot_sampler_defaults(slot: &EffectSlotSnapshot) -> ScheduledSamplerParams {
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

fn rack_slot_sound_fingerprint(
    slot: &RackSlotSnapshot,
    instrument_params: &[ScheduledInstrumentParam],
    base_note_offset: f32,
) -> u64 {
    let mut hasher = DefaultHasher::new();
    slot.track_sound_state.engine_id.hash(&mut hasher);
    base_note_offset.to_bits().hash(&mut hasher);
    for param in instrument_params {
        param.target.hash(&mut hasher);
        param.idx.hash(&mut hasher);
        param.value.to_bits().hash(&mut hasher);
    }
    hasher.finish()
}

fn dispatch_scheduled_step(
    data: &mut AudioCallbackData,
    frame_offset: u32,
    track_idx: usize,
    step: usize,
    samples_per_step: f32,
    resolved: crate::accumulator::ResolvedStep,
    chord: crate::scheduled_event::ScheduledChordData,
    mut effect_params: Vec<ScheduledEffectParam>,
    instrument_params: ScheduledInstrumentParams,
    instrument_tensor_params: ScheduledInstrumentTensorParams,
    sampler_params: ScheduledSamplerParams,
    instrument_fingerprint: u64,
    rack_macro_values: [Option<f32>; crate::sequencer::RACK_MACRO_COUNT],
) {
    unsafe {
        dispatch_effect_chain_for_track(data.lg.0, &mut effect_params);
    }
    fire_resolved(
        data,
        frame_offset,
        track_idx,
        step,
        Some(step),
        samples_per_step as f64,
        resolved,
        chord,
        instrument_params,
        instrument_tensor_params,
        instrument_fingerprint,
        Some(sampler_params),
        rack_macro_values,
    );
}

fn dispatch_scheduled_network_step(
    data: &mut AudioCallbackData,
    frame_offset: u32,
    track_idx: usize,
    key_lock_plock_step: Option<usize>,
    samples_per_step: f32,
    resolved: crate::accumulator::ResolvedStep,
    chord: crate::scheduled_event::ScheduledChordData,
    mut effect_params: Vec<ScheduledEffectParam>,
    instrument_params: ScheduledInstrumentParams,
    instrument_tensor_params: ScheduledInstrumentTensorParams,
    sampler_params: ScheduledSamplerParams,
    instrument_fingerprint: u64,
    rack_macro_values: [Option<f32>; crate::sequencer::RACK_MACRO_COUNT],
) {
    unsafe {
        dispatch_effect_chain_for_track(data.lg.0, &mut effect_params);
    }
    fire_resolved(
        data,
        frame_offset,
        track_idx,
        0,
        key_lock_plock_step,
        samples_per_step as f64,
        resolved,
        chord,
        instrument_params,
        instrument_tensor_params,
        instrument_fingerprint,
        Some(sampler_params),
        rack_macro_values,
    );
}

fn dispatch_scheduled_event(
    data: &mut AudioCallbackData,
    event: ScheduledEvent,
    frame_offset: u32,
) {
    match event.kind {
        ScheduledEventKind::ResolvedTrigger {
            track,
            step,
            samples_per_step,
            resolved,
            chord,
            effect_params,
            instrument_params,
            instrument_tensor_params,
            sampler_params,
            instrument_fingerprint,
            rack_macro_values,
        } => {
            dispatch_scheduled_step(
                data,
                frame_offset,
                track,
                step,
                samples_per_step,
                resolved,
                chord,
                effect_params,
                instrument_params,
                instrument_tensor_params,
                sampler_params,
                instrument_fingerprint,
                rack_macro_values,
            );
        }
        ScheduledEventKind::InstrumentParams {
            track,
            instrument_params,
            instrument_tensor_params,
        } => {
            dispatch_instrument_params_to_active_voices(data, track, &instrument_params);
            dispatch_instrument_tensor_params_to_active_voices(
                data,
                track,
                &instrument_tensor_params,
            );
        }
        ScheduledEventKind::EffectParams {
            mut effect_params, ..
        } => unsafe {
            dispatch_effect_chain_for_track(data.lg.0, &mut effect_params);
        },
        ScheduledEventKind::NetworkTrigger {
            track,
            samples_per_step,
            resolved,
            chord,
            effect_params,
            instrument_params,
            instrument_tensor_params,
            sampler_params,
            instrument_fingerprint,
            rack_macro_values,
            seed,
            ..
        } => {
            dispatch_scheduled_network_step(
                data,
                frame_offset,
                track,
                seed.map(|(_, step)| step),
                samples_per_step,
                resolved,
                chord,
                effect_params,
                instrument_params,
                instrument_tensor_params,
                sampler_params,
                instrument_fingerprint,
                rack_macro_values,
            );
        }
    }
}

fn scheduled_trigger_track(event: &ScheduledEvent) -> Option<usize> {
    match &event.kind {
        ScheduledEventKind::ResolvedTrigger { track, .. }
        | ScheduledEventKind::NetworkTrigger { track, .. } => Some(*track),
        ScheduledEventKind::InstrumentParams { .. } | ScheduledEventKind::EffectParams { .. } => {
            None
        }
    }
}

fn frame_offset_from_remaining(remaining_samples: f64, nframes: usize) -> u32 {
    remaining_samples
        .floor()
        .max(0.0)
        .min(nframes.saturating_sub(1) as f64) as u32
}

fn block_event_priority(kind: &BlockEventKind) -> u8 {
    match kind {
        BlockEventKind::GateOff(_) => 0,
        BlockEventKind::Scheduled(ScheduledEvent {
            kind:
                ScheduledEventKind::InstrumentParams { .. } | ScheduledEventKind::EffectParams { .. },
            ..
        }) => 1,
        BlockEventKind::Scheduled(_) | BlockEventKind::Chop(_) => 2,
    }
}

fn try_push_block_event(
    data: &mut AudioCallbackData,
    frame_offset: u32,
    seq: u64,
    kind: BlockEventKind,
) {
    if data.block_events.len() >= SCHEDULED_BLOCK_SCRATCH_CAPACITY {
        data.dropped_scheduled_events = data.dropped_scheduled_events.saturating_add(1);
        if data.trace_audio {
            eprintln!(
                "audio-trace: dropped countdown event; block scratch full capacity={SCHEDULED_BLOCK_SCRATCH_CAPACITY}"
            );
        }
        return;
    }
    data.block_events.push(BlockEvent {
        frame_offset,
        seq,
        kind,
    });
    data.block_events_need_sort = true;
}

fn try_push_countdown_event(
    data: &mut AudioCallbackData,
    remaining_samples: f64,
    period_samples: f64,
    repeats: u32,
    pattern_epoch: u64,
    seq: u64,
    kind: CountdownEventKind,
) {
    if data.countdown_events.len() >= SCHEDULED_COUNTDOWN_CAPACITY {
        data.dropped_scheduled_events = data.dropped_scheduled_events.saturating_add(1);
        if data.trace_audio {
            eprintln!(
                "audio-trace: dropped countdown event; countdown pool full capacity={SCHEDULED_COUNTDOWN_CAPACITY}"
            );
        }
        return;
    }
    data.countdown_events.push(CountdownEvent {
        remaining_samples,
        period_samples,
        repeats,
        pattern_epoch,
        seq,
        kind,
    });
}

fn schedule_countdown_or_block_event(
    data: &mut AudioCallbackData,
    event_offset: f64,
    period_samples: f64,
    repeats: u32,
    pattern_epoch: u64,
    kind: CountdownEventKind,
) {
    if repeats == 0 {
        return;
    }
    let nframes = data.current_callback_nframes;
    match kind {
        CountdownEventKind::Scheduled(event) => {
            let seq = data.event_seq;
            data.event_seq = data.event_seq.wrapping_add(1);
            if event_offset < nframes as f64 {
                let frame_offset = frame_offset_from_remaining(event_offset, nframes);
                try_push_block_event(data, frame_offset, seq, BlockEventKind::Scheduled(event));
            } else {
                try_push_countdown_event(
                    data,
                    event_offset - nframes as f64,
                    0.0,
                    1,
                    pattern_epoch,
                    seq,
                    CountdownEventKind::Scheduled(event),
                );
            }
        }
        CountdownEventKind::GateOff(event) => {
            let seq = data.event_seq;
            data.event_seq = data.event_seq.wrapping_add(1);
            if event_offset < nframes as f64 {
                let frame_offset = frame_offset_from_remaining(event_offset, nframes);
                try_push_block_event(data, frame_offset, seq, BlockEventKind::GateOff(event));
            } else {
                try_push_countdown_event(
                    data,
                    event_offset - nframes as f64,
                    0.0,
                    1,
                    pattern_epoch,
                    seq,
                    CountdownEventKind::GateOff(event),
                );
            }
        }
        CountdownEventKind::Chop(event) => {
            let mut next_offset = event_offset;
            let mut remaining_repeats = repeats;
            while remaining_repeats > 0 && next_offset < nframes as f64 {
                let seq = data.event_seq;
                data.event_seq = data.event_seq.wrapping_add(1);
                let frame_offset = frame_offset_from_remaining(next_offset, nframes);
                try_push_block_event(data, frame_offset, seq, BlockEventKind::Chop(event));
                remaining_repeats -= 1;
                next_offset += period_samples.max(1.0);
            }
            if remaining_repeats > 0 {
                let seq = data.event_seq;
                data.event_seq = data.event_seq.wrapping_add(1);
                try_push_countdown_event(
                    data,
                    next_offset - nframes as f64,
                    period_samples.max(1.0),
                    remaining_repeats,
                    pattern_epoch,
                    seq,
                    CountdownEventKind::Chop(event),
                );
            }
        }
    }
}

fn enqueue_scheduled_event_for_callback(
    data: &mut AudioCallbackData,
    event: ScheduledEvent,
    block_start_sample: u64,
    nframes: usize,
    current_pattern_epoch: u64,
) {
    if event.pattern_epoch != current_pattern_epoch {
        return;
    }
    let seq = data.event_seq;
    data.event_seq = data.event_seq.wrapping_add(1);
    let remaining_samples = if event.sample_time >= block_start_sample {
        (event.sample_time - block_start_sample) as f64
    } else {
        data.late_scheduled_events = data.late_scheduled_events.saturating_add(1);
        0.0
    };
    if remaining_samples < nframes as f64 {
        let frame_offset = frame_offset_from_remaining(remaining_samples, nframes);
        try_push_block_event(data, frame_offset, seq, BlockEventKind::Scheduled(event));
    } else {
        try_push_countdown_event(
            data,
            remaining_samples - nframes as f64,
            0.0,
            1,
            event.pattern_epoch,
            seq,
            CountdownEventKind::Scheduled(event),
        );
    }
}

fn drain_scheduled_events_for_callback(
    data: &mut AudioCallbackData,
    block_start_sample: u64,
    nframes: usize,
    current_pattern_epoch: u64,
) {
    while let Some(event) = data.scheduled_events.pop() {
        enqueue_scheduled_event_for_callback(
            data,
            event,
            block_start_sample,
            nframes,
            current_pattern_epoch,
        );
    }
}

fn collect_due_countdown_events(
    data: &mut AudioCallbackData,
    nframes: usize,
    current_pattern_epoch: u64,
) {
    let block_len = nframes as f64;
    let mut i = 0usize;
    while i < data.countdown_events.len() {
        let stale = match data.countdown_events[i].kind {
            CountdownEventKind::GateOff(_) => false,
            CountdownEventKind::Scheduled(_) | CountdownEventKind::Chop(_) => {
                data.countdown_events[i].pattern_epoch != current_pattern_epoch
            }
        };
        if stale {
            data.countdown_events.swap_remove(i);
            continue;
        }
        if data.countdown_events[i].remaining_samples < block_len {
            let mut due = data.countdown_events.swap_remove(i);
            match due.kind {
                CountdownEventKind::Chop(event) => {
                    while due.repeats > 0 && due.remaining_samples < block_len {
                        let frame_offset =
                            frame_offset_from_remaining(due.remaining_samples, nframes);
                        try_push_block_event(
                            data,
                            frame_offset,
                            due.seq,
                            BlockEventKind::Chop(event),
                        );
                        due.repeats -= 1;
                        due.seq = data.event_seq;
                        data.event_seq = data.event_seq.wrapping_add(1);
                        due.remaining_samples += due.period_samples;
                    }
                    if due.repeats > 0 {
                        due.remaining_samples -= block_len;
                        data.countdown_events.push(due);
                    }
                }
                CountdownEventKind::Scheduled(event) => {
                    let frame_offset = frame_offset_from_remaining(due.remaining_samples, nframes);
                    try_push_block_event(
                        data,
                        frame_offset,
                        due.seq,
                        BlockEventKind::Scheduled(event),
                    );
                }
                CountdownEventKind::GateOff(event) => {
                    let frame_offset = frame_offset_from_remaining(due.remaining_samples, nframes);
                    try_push_block_event(
                        data,
                        frame_offset,
                        due.seq,
                        BlockEventKind::GateOff(event),
                    );
                }
            }
            continue;
        }
        data.countdown_events[i].remaining_samples -= block_len;
        i += 1;
    }
}

fn clear_countdown_events(data: &mut AudioCallbackData) {
    data.countdown_events.clear();
    data.block_events.clear();
    data.block_events_need_sort = false;
}

fn clear_transport_countdown_events(data: &mut AudioCallbackData) {
    data.countdown_events
        .retain(|event| matches!(event.kind, CountdownEventKind::GateOff(_)));
    data.block_events
        .retain(|event| matches!(event.kind, BlockEventKind::GateOff(_)));
    data.block_events_need_sort = true;
}

fn mute_group_winner_for_block_events(
    track: usize,
    group: u8,
    batch: &[BlockEvent],
    track_mute_groups: impl Fn(usize) -> u8,
) -> usize {
    batch
        .iter()
        .filter_map(|event| match &event.kind {
            BlockEventKind::Scheduled(scheduled) => scheduled_trigger_track(scheduled),
            BlockEventKind::GateOff(_) | BlockEventKind::Chop(_) => None,
        })
        .filter(|&candidate| track_mute_groups(candidate) == group)
        .max()
        .unwrap_or(track)
}

fn dispatch_block_events(data: &mut AudioCallbackData, block_start_sample: u64) {
    while !data.block_events.is_empty() {
        if data.block_events_need_sort {
            data.block_events.sort_unstable_by(|a, b| {
                (b.frame_offset, block_event_priority(&b.kind), b.seq).cmp(&(
                    a.frame_offset,
                    block_event_priority(&a.kind),
                    a.seq,
                ))
            });
            data.block_events_need_sort = false;
        }

        let Some(frame_offset) = data.block_events.last().map(|event| event.frame_offset) else {
            break;
        };
        let mut group_start = data.block_events.len();
        while group_start > 0 && data.block_events[group_start - 1].frame_offset == frame_offset {
            group_start -= 1;
        }

        let mut winning_group_tracks = [false; MAX_TRACKS];
        {
            let group = &data.block_events[group_start..];
            for event in group {
                let Some(track) = (match &event.kind {
                    BlockEventKind::Scheduled(scheduled) => scheduled_trigger_track(scheduled),
                    BlockEventKind::GateOff(_) | BlockEventKind::Chop(_) => None,
                }) else {
                    continue;
                };
                if track >= data.state.active_track_count() {
                    continue;
                }
                let group_id = data.state.pattern.track_params[track].get_mute_group();
                if group_id == 0 {
                    continue;
                }
                let winner =
                    mute_group_winner_for_block_events(track, group_id, group, |candidate| {
                        data.state
                            .pattern
                            .track_params
                            .get(candidate)
                            .map(|params| params.get_mute_group())
                            .unwrap_or(0)
                    });
                if winner < MAX_TRACKS {
                    winning_group_tracks[winner] = true;
                }
            }
        }

        let release_sample = block_start_sample + frame_offset as u64;
        for (track, is_winner) in winning_group_tracks.iter().copied().enumerate() {
            if is_winner {
                enforce_mute_group_for_winning_track(data, track, release_sample, frame_offset);
            }
        }

        while data
            .block_events
            .last()
            .is_some_and(|event| event.frame_offset == frame_offset)
        {
            let event = data.block_events.pop().unwrap();
            match event.kind {
                BlockEventKind::Scheduled(scheduled) => {
                    let dispatch = match scheduled_trigger_track(&scheduled) {
                        Some(track) if track < data.state.active_track_count() => {
                            let group = data.state.pattern.track_params[track].get_mute_group();
                            group == 0 || winning_group_tracks[track]
                        }
                        Some(_) => false,
                        None => true,
                    };
                    if dispatch {
                        dispatch_scheduled_event(data, scheduled, frame_offset);
                    }
                }
                BlockEventKind::GateOff(gate_off) => {
                    dispatch_gate_off_event(data, gate_off, frame_offset, block_start_sample);
                }
                BlockEventKind::Chop(chop) => {
                    dispatch_chop_event(data, chop, frame_offset);
                }
            }
        }
    }
}

fn render_chunk(data: &mut AudioCallbackData, output: &mut [f32]) {
    if output.is_empty() {
        return;
    }
    let nframes = output.len() / data.num_channels;
    if nframes == 0 {
        return;
    }
    publish_sampler_modulator_activity(data);
    unsafe {
        data.lg
            .process_next_block(output.as_mut_ptr(), nframes as i32);
    }
}

/// Mix the transport click after recorder capture. This intentionally keeps
/// exported master WAVs and all upstream per-track capture paths click-free.
fn mix_metronome(
    metronome: &mut MetronomeState,
    output: &mut [f32],
    num_channels: usize,
    sample_rate: f64,
    block_start_beats: f64,
    bpm: f64,
) {
    if output.is_empty() || num_channels == 0 || bpm <= 0.0 {
        return;
    }
    let nframes = output.len() / num_channels;
    let beats_per_sample = bpm / (sample_rate * 60.0);
    let block_end_beats = block_start_beats + nframes as f64 * beats_per_sample;
    let first_quarter = (block_start_beats - 1.0e-9).ceil().max(0.0) as u64;
    let mut next_quarter = first_quarter;

    for frame in 0..nframes {
        let beat = block_start_beats + frame as f64 * beats_per_sample;
        while (next_quarter as f64) <= beat + 1.0e-9
            && (next_quarter as f64) < block_end_beats + 1.0e-9
        {
            metronome.trigger(sample_rate, next_quarter % 4 == 0);
            next_quarter += 1;
        }
        let click = metronome.sample(sample_rate);
        if click != 0.0 {
            for channel in 0..num_channels {
                output[frame * num_channels + channel] += click;
            }
        }
    }
}

fn publish_sampler_modulator_activity(data: &AudioCallbackData) {
    // Covers both per-track pools (0..MAX_TRACKS) and per-rack-slot pools
    // (rack_slot_pool_index, >= MAX_TRACKS) — previously capped at
    // MAX_TRACKS, which left every rack slot's mask permanently zero and
    // forced its voice_modulator nodes through an O(nframes) gate-timeline
    // scan every block instead of the O(1) active-mask check.
    for (pool_id, pool) in data.voice_pools.iter().enumerate() {
        if pool.num_voices == 0 {
            continue;
        }
        let mut mask = 0u64;
        for voice_idx in 0..pool.num_voices.min(MAX_VOICES) {
            if pool.voices[voice_idx].active {
                mask |= 1u64 << voice_idx;
            }
        }
        crate::voice_modulator::set_sampler_active_mask(pool_id, mask);
    }
}

fn bus_gate_state_at(
    sequence: &crate::sequencer::BusGateSequence,
    total_beats: f64,
) -> (f32, usize) {
    const EPS: f64 = 1e-9;
    let ns = sequence.num_steps.clamp(1, crate::sequencer::MAX_STEPS);
    let mut starts = [0.0f64; crate::sequencer::MAX_STEPS];
    let mut durations = [0.0f64; crate::sequencer::MAX_STEPS];
    let mut accum = 0.0f64;
    for step in 0..ns {
        let timebase = sequence.timebase_plocks[step].unwrap_or(sequence.timebase);
        let duration = timebase.step_beats(ns).max(EPS);
        let sync = sync_beats(sequence.syncs[step]);
        if sync > EPS {
            accum = ceil_to_grid(accum, sync);
        }
        starts[step] = accum;
        durations[step] = duration;
        accum += duration;
    }
    let sync0 = sync_beats(sequence.syncs[0]);
    if sync0 > EPS {
        accum = ceil_to_grid(accum, sync0).max(EPS);
    }
    if accum <= EPS {
        return (1.0, 0);
    }

    let pos = total_beats.rem_euclid(accum);
    let mut active_step = None;
    for idx in 0..ns {
        if pos + EPS >= starts[idx] && pos < starts[idx] + durations[idx] {
            active_step = Some(idx);
            break;
        }
    }
    let step = active_step.unwrap_or_else(|| {
        let idx = starts[..ns].partition_point(|&start| start <= pos);
        idx.saturating_sub(1).min(ns - 1)
    });
    if active_step.is_none() {
        return (0.0, step);
    }

    if !sequence.steps[step] {
        return (0.0, step);
    }
    let local = pos - starts[step];
    let gate_duration = durations[step] * sequence.durations[step].clamp(0.0, 1.0) as f64;
    if local <= gate_duration + EPS {
        (sequence.velocities[step].clamp(0.0, 1.0), step)
    } else {
        (0.0, step)
    }
}

fn bus_gate_target_at(sequence: &crate::sequencer::BusGateSequence, total_beats: f64) -> f32 {
    bus_gate_state_at(sequence, total_beats).0
}

fn ceil_to_grid(value: f64, grid: f64) -> f64 {
    let rem = value % grid;
    if rem > 1e-9 {
        value + (grid - rem)
    } else {
        value
    }
}

unsafe fn dispatch_snapshot_effect_params_at_step(
    lg: *mut LiveGraph,
    effect_slots: &[EffectSlotSnapshot],
    step: usize,
) {
    for slot in effect_slots {
        if slot.node_id == 0 {
            continue;
        }
        let num_params = slot.num_params as usize;
        let mut param_indices: ArrayVec<usize, MAX_SLOT_PARAMS> = ArrayVec::new();
        for param_idx in 0..num_params.min(MAX_SLOT_PARAMS) {
            param_indices.push(param_idx);
        }
        param_indices.sort_by_key(|param_idx| {
            slot.param_node_indices
                .get(*param_idx)
                .copied()
                .unwrap_or(*param_idx as u32)
        });
        for param_idx in param_indices {
            let idx = slot
                .param_node_indices
                .get(param_idx)
                .copied()
                .unwrap_or(param_idx as u32);
            if idx == u32::MAX || param_idx >= slot.defaults.len() {
                continue;
            }
            let (logical_id, idx) = if idx >= crate::voice_modulator::MOD_PARAM_BASE {
                if slot.modulator_node_id == 0 {
                    continue;
                }
                (
                    slot.modulator_node_id as u64,
                    (idx - crate::voice_modulator::MOD_PARAM_BASE) as u64,
                )
            } else {
                (slot.node_id as u64, idx as u64)
            };
            let value = resolved_slot_param_value(slot, step, param_idx, slot.defaults[param_idx]);
            if !value.is_finite() {
                continue;
            }
            let span = slot
                .param_node_spans
                .get(param_idx)
                .copied()
                .unwrap_or(1)
                .max(1);
            push_param_span(lg, logical_id, idx, span, value);
        }
    }
}

fn sync_bus_gate_params(data: &mut AudioCallbackData, block_start_sample: u64) {
    let playing = data.state.transport.playing.load(Ordering::Relaxed);
    let bpm = data.state.transport.bpm.load(Ordering::Relaxed).max(1) as f64;
    if playing && !data.bus_gate_was_playing {
        data.bus_gate_play_start_sample = block_start_sample;
        for clock in &mut data.bus_gate_clocks {
            clock.last_target = f32::NAN;
            clock.last_step = None;
        }
    }
    if !playing && data.bus_gate_was_playing {
        for clock in &mut data.bus_gate_clocks {
            clock.last_target = f32::NAN;
            clock.last_step = None;
        }
    }
    data.bus_gate_was_playing = playing;

    let elapsed_samples = block_start_sample.saturating_sub(data.bus_gate_play_start_sample);
    let total_beats = elapsed_samples as f64 * bpm / (data.sample_rate * 60.0);
    let Ok(gates) = data.bus_gate_runtime.try_lock() else {
        return;
    };
    let gates = gates.clone();
    let mut playheads = Vec::with_capacity(gates.len());

    data.bus_gate_clocks
        .retain(|clock| gates.iter().any(|gate| gate.id == clock.id));

    for gate in gates {
        if gate.gate_id <= 0 {
            continue;
        }
        let (target, step) = if playing {
            bus_gate_state_at(&gate.sequence, total_beats)
        } else {
            (1.0, 0)
        };
        playheads.push((gate.id, step));
        let clock_idx = data
            .bus_gate_clocks
            .iter()
            .position(|clock| clock.id == gate.id)
            .unwrap_or_else(|| {
                data.bus_gate_clocks.push(BusGateClock {
                    id: gate.id,
                    last_target: f32::NAN,
                    last_step: None,
                });
                data.bus_gate_clocks.len() - 1
            });
        let clock = &mut data.bus_gate_clocks[clock_idx];
        if clock.last_step != Some(step) {
            clock.last_step = Some(step);
            unsafe {
                dispatch_snapshot_effect_params_at_step(data.lg.0, &gate.effect_slots, step);
            }
        }
        if (clock.last_target - target).abs() <= 0.0001 {
            continue;
        }
        clock.last_target = target;
        unsafe {
            crate::audiograph::params_push_wrapper(
                data.lg.0,
                crate::audiograph::ParamMsg {
                    idx: crate::effects::stereo_panner::STEREO_PANNER_PARAM_VOLUME,
                    logical_id: gate.gate_id as u64,
                    fvalue: target,
                },
            );
        }
    }
    if let Ok(mut shared_playheads) = data.bus_gate_playheads.try_lock() {
        *shared_playheads = playheads;
    }
}

fn compute_host_transport_clock(
    data: &mut AudioCallbackData,
    block_start_sample: u64,
) -> HostTransportClock {
    let playing = data.state.transport.playing.load(Ordering::Relaxed);
    if playing && !data.host_clock_was_playing {
        data.host_clock_play_start_sample = block_start_sample;
    }
    if !playing && data.host_clock_was_playing {
        data.host_clock_play_start_sample = block_start_sample;
    }
    data.host_clock_was_playing = playing;

    let bpm = data.state.transport.bpm.load(Ordering::Relaxed).max(1) as f64;
    let samples_per_bar = data.sample_rate * 240.0 / bpm;
    let elapsed_samples = block_start_sample.saturating_sub(data.host_clock_play_start_sample);
    let bar_phase = (elapsed_samples as f64 / samples_per_bar).fract() as f32;
    let bar_phase_increment = (1.0 / samples_per_bar) as f32;

    HostTransportClock {
        bar_phase,
        bar_phase_increment,
    }
}

fn sync_instrument_host_clock_params(data: &mut AudioCallbackData, clock: HostTransportClock) {
    for engine_id in 0..data.state.runtime.engine_voice_counts.len() {
        let voice_count =
            data.state.runtime.engine_voice_counts[engine_id].load(Ordering::Acquire) as usize;
        for voice_idx in 0..voice_count.min(MAX_VOICES) {
            let lid =
                data.state.runtime.engine_voice_lids[engine_id][voice_idx].load(Ordering::Acquire);
            if lid == 0 {
                continue;
            }
            unsafe {
                params_push_wrapper(
                    data.lg.0,
                    ParamMsg {
                        idx: gatepitch::PARAM_CLOCK_PHASE,
                        logical_id: lid,
                        fvalue: clock.bar_phase,
                    },
                );
                params_push_wrapper(
                    data.lg.0,
                    ParamMsg {
                        idx: gatepitch::PARAM_CLOCK_INC,
                        logical_id: lid,
                        fvalue: clock.bar_phase_increment,
                    },
                );
            }
        }
    }

    for pool_id in 0..data.state.runtime.voice_counts.len() {
        let voice_count = data.state.runtime.voice_counts[pool_id].load(Ordering::Acquire) as usize;
        for voice_idx in 0..voice_count.min(MAX_VOICES) {
            let gatepitch_id = data.state.runtime.sampler_gatepitch_node_ids[pool_id][voice_idx]
                .load(Ordering::Acquire);
            if gatepitch_id == 0 {
                continue;
            }
            unsafe {
                params_push_wrapper(
                    data.lg.0,
                    ParamMsg {
                        idx: gatepitch::PARAM_CLOCK_PHASE,
                        logical_id: gatepitch_id as u64,
                        fvalue: clock.bar_phase,
                    },
                );
                params_push_wrapper(
                    data.lg.0,
                    ParamMsg {
                        idx: gatepitch::PARAM_CLOCK_INC,
                        logical_id: gatepitch_id as u64,
                        fvalue: clock.bar_phase_increment,
                    },
                );
            }
        }
    }
}

fn sync_effect_modulator_transport_clock_params(
    data: &mut AudioCallbackData,
    clock: HostTransportClock,
) {
    for chain in &data.state.pattern.effect_chains {
        for slot in chain {
            let modulator_id = slot.modulator_node_id.load(Ordering::Relaxed);
            if modulator_id == 0 {
                continue;
            }
            unsafe {
                dispatch_voice_modulator_transport_clock(data.lg.0, modulator_id as u64, clock);
            }
        }
    }

    for track in &data.scheduler_snapshot.tracks {
        let Some(rack) = &track.rack_track else {
            continue;
        };
        for rack_slot in &rack.slots {
            for slot in &rack_slot.effect_slots {
                if slot.modulator_node_id == 0 {
                    continue;
                }
                unsafe {
                    dispatch_voice_modulator_transport_clock(
                        data.lg.0,
                        slot.modulator_node_id as u64,
                        clock,
                    );
                }
            }
        }
    }

    let Ok(gates) = data.bus_gate_runtime.try_lock() else {
        return;
    };
    for gate in gates.iter() {
        for slot in &gate.effect_slots {
            if slot.modulator_node_id == 0 {
                continue;
            }
            unsafe {
                dispatch_voice_modulator_transport_clock(
                    data.lg.0,
                    slot.modulator_node_id as u64,
                    clock,
                );
            }
        }
    }
}

fn sync_dj_mixer_transport_phase(data: &mut AudioCallbackData, block_start_sample: u64) {
    let playing = data.state.transport.playing.load(Ordering::Relaxed);
    let bpm = data.state.transport.bpm.load(Ordering::Relaxed).max(1) as f64;
    let total_beats = if playing {
        block_start_sample as f64 * bpm / (data.sample_rate * 60.0)
    } else {
        0.0
    };
    let beat_phase = crate::effects::dj_mixer::transport_beat_phase(total_beats);

    for chain in &data.state.pattern.effect_chains {
        for slot in chain {
            let param_idx = slot.transport_phase_param_idx.load(Ordering::Relaxed);
            if param_idx == crate::effects::NO_TRANSPORT_PHASE_PARAM {
                continue;
            }
            let node_id = slot.node_id.load(Ordering::Relaxed);
            if node_id == 0 {
                continue;
            }
            unsafe {
                dispatch_transport_phase(data.lg.0, node_id as u64, param_idx, beat_phase);
            }
        }
    }
    for track in &data.scheduler_snapshot.tracks {
        let Some(rack) = &track.rack_track else {
            continue;
        };
        for rack_slot in &rack.slots {
            for slot in &rack_slot.effect_slots {
                if slot.transport_phase_param_idx == crate::effects::NO_TRANSPORT_PHASE_PARAM
                    || slot.node_id == 0
                {
                    continue;
                }
                unsafe {
                    dispatch_transport_phase(
                        data.lg.0,
                        slot.node_id as u64,
                        slot.transport_phase_param_idx,
                        beat_phase,
                    );
                }
            }
        }
    }

    let Ok(gates) = data.bus_gate_runtime.try_lock() else {
        return;
    };
    for gate in gates.iter() {
        for slot in &gate.effect_slots {
            let param_idx = slot.transport_phase_param_idx;
            if param_idx == crate::effects::NO_TRANSPORT_PHASE_PARAM || slot.node_id == 0 {
                continue;
            }
            unsafe {
                dispatch_transport_phase(data.lg.0, slot.node_id as u64, param_idx, beat_phase);
            }
        }
    }
}

fn interleaved_peak(output: &[f32], num_channels: usize) -> (f32, f32) {
    let mut peak_l = 0.0f32;
    let mut peak_r = 0.0f32;
    if num_channels == 0 {
        return (peak_l, peak_r);
    }
    let nframes = output.len() / num_channels;
    for i in 0..nframes {
        let l = output[i * num_channels].abs();
        if l > peak_l {
            peak_l = l;
        }
        if num_channels > 1 {
            let r = output[i * num_channels + 1].abs();
            if r > peak_r {
                peak_r = r;
            }
        }
    }
    (peak_l, peak_r)
}

fn zero_output_frames(output: &mut [f32], start_frame: usize, num_channels: usize) {
    let start = start_frame.saturating_mul(num_channels);
    if start < output.len() {
        output[start..].fill(0.0);
    }
}

fn rack_slot_accepts_trigger(slot: &RackSlotSnapshot, has_solo: bool) -> bool {
    if has_solo {
        slot.solo && !slot.mute
    } else {
        !slot.mute
    }
}

#[derive(Clone, Copy, Debug)]
struct ResolvedRackSlotParams {
    base_note_offset: f32,
    gain: f32,
    pan: f32,
    max_polyphony: usize,
    mute: bool,
    solo: bool,
}

fn resolve_rack_slot_params(slot: &RackSlotSnapshot, step: usize) -> ResolvedRackSlotParams {
    let value = |param: RackSlotParam| param.clamp(slot.param_value_at_step(param, step));
    let max_polyphony = value(RackSlotParam::MaxPolyphony)
        .round()
        .clamp(1.0, MAX_VOICES as f32) as usize;
    ResolvedRackSlotParams {
        base_note_offset: value(RackSlotParam::BaseNote),
        gain: value(RackSlotParam::Gain),
        pan: value(RackSlotParam::Pan),
        max_polyphony,
        mute: value(RackSlotParam::Mute) > 0.5,
        solo: value(RackSlotParam::Solo) > 0.5,
    }
}

fn rack_macro_curve_value(curve: crate::sequencer::RackMacroCurve, value: f32) -> f32 {
    match curve {
        crate::sequencer::RackMacroCurve::Linear => value,
        crate::sequencer::RackMacroCurve::Exp => value * value,
        crate::sequencer::RackMacroCurve::Log => value.sqrt(),
    }
}

fn apply_rack_macros_at_step(
    rack: &mut RackTrackSnapshot,
    step: usize,
    process_values: [Option<f32>; crate::sequencer::RACK_MACRO_COUNT],
) {
    let macros = rack.macros.clone();
    for rack_macro in macros {
        let normalized = process_values
            .get(rack_macro.id.index())
            .and_then(|value| *value)
            .unwrap_or_else(|| {
                rack.runtime_macro_value_at(rack_macro.id, step)
                    .unwrap_or_else(|| rack_macro.value_at(step))
            });
        for mapping in rack_macro.mappings {
            let value = mapping.range_min
                + (mapping.range_max - mapping.range_min)
                    * rack_macro_curve_value(mapping.curve, normalized);
            match mapping.target {
                crate::sequencer::RackMacroTarget::SlotParam { slot, param } => {
                    let Some(slot) = rack.slots.get_mut(slot) else {
                        continue;
                    };
                    let normalized = param
                        .trim_start_matches(':')
                        .replace('_', "-")
                        .to_ascii_lowercase();
                    let target = match normalized.as_str() {
                        "base-note" | "transpose" => Some(RackSlotParam::BaseNote),
                        "gain" => Some(RackSlotParam::Gain),
                        "pan" => Some(RackSlotParam::Pan),
                        "max-polyphony" | "polyphony" => Some(RackSlotParam::MaxPolyphony),
                        "mute" => Some(RackSlotParam::Mute),
                        "solo" => Some(RackSlotParam::Solo),
                        _ => None,
                    };
                    let Some(target) = target else {
                        continue;
                    };
                    if slot.param_plocks.get(step, target).is_some() {
                        continue;
                    }
                    match target {
                        RackSlotParam::BaseNote => {
                            slot.instrument_base_note_offset = target.clamp(value)
                        }
                        RackSlotParam::Gain => slot.gain = target.clamp(value),
                        RackSlotParam::Pan => slot.pan = target.clamp(value),
                        RackSlotParam::MaxPolyphony => {
                            slot.max_polyphony = target.clamp(value).round() as usize
                        }
                        RackSlotParam::Mute => slot.mute = value >= 0.5,
                        RackSlotParam::Solo => slot.solo = value >= 0.5,
                    }
                }
                crate::sequencer::RackMacroTarget::SlotInstrumentParam {
                    slot,
                    param_index,
                    ..
                } => {
                    let Some(slot) = rack.slots.get_mut(slot) else {
                        continue;
                    };
                    let locked = slot
                        .instrument_slot
                        .plocks
                        .get(step)
                        .and_then(|row| row.get(param_index))
                        .and_then(|value| *value)
                        .is_some();
                    if !locked {
                        if let Some(default) = slot.instrument_slot.defaults.get_mut(param_index) {
                            *default = value;
                        }
                    }
                }
                crate::sequencer::RackMacroTarget::SlotEffectParam {
                    slot,
                    effect_slot,
                    param_index,
                    ..
                } => {
                    let Some(effect) = rack
                        .slots
                        .get_mut(slot)
                        .and_then(|slot| slot.effect_slots.get_mut(effect_slot))
                    else {
                        continue;
                    };
                    let locked = effect
                        .plocks
                        .get(step)
                        .and_then(|row| row.get(param_index))
                        .and_then(|value| *value)
                        .is_some();
                    if !locked {
                        if let Some(default) = effect.defaults.get_mut(param_index) {
                            *default = value;
                        }
                    }
                }
            }
        }
    }
}

fn rack_slot_accepts_resolved(params: ResolvedRackSlotParams, has_solo: bool) -> bool {
    if has_solo {
        params.solo && !params.mute
    } else {
        !params.mute
    }
}

fn rack_slot_matches_routing(
    slot: &RackSlotSnapshot,
    routing: RackRouting,
    transpose: f32,
) -> bool {
    match routing {
        RackRouting::Broadcast => true,
        RackRouting::ByPitch => slot.pad_note == Some(transpose.round() as i32),
    }
}

fn rack_slot_playback_transpose(routing: RackRouting, transpose: f32) -> f32 {
    match routing {
        RackRouting::Broadcast => transpose,
        RackRouting::ByPitch => 0.0,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RackSlotNoteOff {
    Custom { logical_id: u64 },
    Sampler { logical_id: u64 },
}

fn collect_rack_slot_active_voice_releases(
    voice_pools: &mut [VoicePool],
    custom_engine_pools: &mut [CustomEnginePool],
    countdown_events: &mut Vec<CountdownEvent>,
    block_events: &mut Vec<BlockEvent>,
    track_idx: usize,
    slot_idx: usize,
    slot: &RackSlotSnapshot,
    release_sample: u64,
) -> Vec<RackSlotNoteOff> {
    let mut note_offs = Vec::new();
    match slot.instrument_type {
        InstrumentType::Sampler => {
            let Some(pool_id) = rack_slot_pool_index(track_idx, slot_idx) else {
                return note_offs;
            };
            if pool_id >= voice_pools.len() {
                return note_offs;
            }
            let active: Vec<(u64, i32)> = voice_pools[pool_id].voices
                [..voice_pools[pool_id].num_voices]
                .iter()
                .filter(|voice| voice.active && voice.logical_id != 0)
                .map(|voice| (voice.logical_id, voice.gatepitch_id))
                .collect();
            for (lid, gatepitch_id) in active {
                voice_pools[pool_id].release_voice_by_logical_id(lid);
                cancel_gate_off_for_lid(countdown_events, block_events, lid);
                if gatepitch_id > 0 {
                    note_offs.push(RackSlotNoteOff::Custom {
                        logical_id: gatepitch_id as u64,
                    });
                }
                note_offs.push(RackSlotNoteOff::Sampler { logical_id: lid });
            }
        }
        InstrumentType::Custom => {
            let Some(engine_id) = slot.track_sound_state.engine_id else {
                return note_offs;
            };
            if engine_id >= custom_engine_pools.len() {
                return note_offs;
            }
            let free_patch = slot.instrument_run_mode == CustomInstrumentRunMode::FreePatch;
            let route_idx = rack_slot_pool_index(track_idx, slot_idx)
                .expect("validated rack slot must have a route identity");
            let lids: Vec<u64> = custom_engine_pools[engine_id].voices
                [..custom_engine_pools[engine_id].num_voices]
                .iter()
                .filter(|voice| voice.active && voice.assigned_route == Some(route_idx))
                .map(|voice| voice.logical_id)
                .collect();
            for lid in lids {
                if free_patch {
                    custom_engine_pools[engine_id].release_free_patch_voice_by_logical_id(lid);
                } else {
                    custom_engine_pools[engine_id].release_voice_by_logical_id(lid, release_sample);
                }
                cancel_gate_off_for_lid(countdown_events, block_events, lid);
                note_offs.push(RackSlotNoteOff::Custom { logical_id: lid });
            }
        }
        InstrumentType::Modulator | InstrumentType::Rack => {}
    }
    note_offs
}

fn collect_rack_choke_group_voice_releases(
    voice_pools: &mut [VoicePool],
    custom_engine_pools: &mut [CustomEnginePool],
    countdown_events: &mut Vec<CountdownEvent>,
    block_events: &mut Vec<BlockEvent>,
    parent_track_idx: usize,
    rack: &RackTrackSnapshot,
    triggering_slot_idx: usize,
    choke_group: u8,
    release_sample: u64,
) -> Vec<RackSlotNoteOff> {
    let mut note_offs = Vec::new();
    for (slot_idx, slot) in rack.slots.iter().enumerate() {
        if slot_idx == triggering_slot_idx || slot.choke_group != Some(choke_group) {
            continue;
        }
        note_offs.extend(collect_rack_slot_active_voice_releases(
            voice_pools,
            custom_engine_pools,
            countdown_events,
            block_events,
            parent_track_idx,
            slot_idx,
            slot,
            release_sample,
        ));
    }
    note_offs
}

fn dispatch_rack_slot_note_offs(
    data: &mut AudioCallbackData,
    frame_offset: u32,
    note_offs: Vec<RackSlotNoteOff>,
) {
    for note_off in note_offs {
        let seq = next_block_event_sequence(data);
        unsafe {
            match note_off {
                RackSlotNoteOff::Custom { logical_id } => {
                    send_custom_note_off(data.lg.0, logical_id, frame_offset, seq);
                }
                RackSlotNoteOff::Sampler { logical_id } => {
                    send_sampler_note_off(data.lg.0, logical_id, frame_offset, seq);
                }
            }
        }
    }
}

fn release_rack_choke_group_voices(
    data: &mut AudioCallbackData,
    parent_track_idx: usize,
    rack: &RackTrackSnapshot,
    triggering_slot_idx: usize,
    choke_group: u8,
    frame_offset: u32,
) {
    let release_sample = data.rendered_samples.load(Ordering::Acquire) + frame_offset as u64;
    let note_offs = collect_rack_choke_group_voice_releases(
        &mut data.voice_pools,
        &mut data.custom_engine_pools,
        &mut data.countdown_events,
        &mut data.block_events,
        parent_track_idx,
        rack,
        triggering_slot_idx,
        choke_group,
        release_sample,
    );
    dispatch_rack_slot_note_offs(data, frame_offset, note_offs);
}

unsafe fn push_rack_slot_panner_params(
    lg: *mut LiveGraph,
    slot_pan_lid: u64,
    params: ResolvedRackSlotParams,
    muted_by_solo: bool,
) {
    if slot_pan_lid == 0 {
        return;
    }
    params_push_wrapper(
        lg,
        ParamMsg {
            idx: crate::effects::stereo_panner::STEREO_PANNER_PARAM_VOLUME,
            logical_id: slot_pan_lid,
            fvalue: params.gain,
        },
    );
    params_push_wrapper(
        lg,
        ParamMsg {
            idx: crate::effects::stereo_panner::STEREO_PANNER_PARAM_PAN,
            logical_id: slot_pan_lid,
            fvalue: params.pan,
        },
    );
    params_push_wrapper(
        lg,
        ParamMsg {
            idx: crate::effects::stereo_panner::STEREO_PANNER_PARAM_MUTE,
            logical_id: slot_pan_lid,
            fvalue: if params.mute { 1.0 } else { 0.0 },
        },
    );
    params_push_wrapper(
        lg,
        ParamMsg {
            idx: crate::effects::stereo_panner::STEREO_PANNER_PARAM_MUTED_BY_SOLO,
            logical_id: slot_pan_lid,
            fvalue: if muted_by_solo { 1.0 } else { 0.0 },
        },
    );
}

fn rack_sampler_warp_runtime(
    state: &SequencerState,
    warp_enabled: f32,
    warp_mode: f32,
    sample_bpm: f32,
) -> (f32, f32, f32, f32, f32, f32, f32) {
    let project_bpm = state.transport.bpm.load(Ordering::Relaxed).max(1) as f32;
    let sample_bpm = sample_bpm.clamp(20.0, 400.0);
    if warp_enabled <= 0.5 {
        return (0.0, warp_mode, 1.0, sample_bpm, project_bpm, 0.0, 0.0);
    }
    // All warp modes run without analysis now (Beats falls back to the pure
    // beat grid when no onset table is present), so racks support every mode.
    let ratio = (project_bpm / sample_bpm).clamp(0.01, 32.0);
    (1.0, warp_mode, ratio, sample_bpm, project_bpm, 0.0, 0.0)
}

fn push_active_keyboard_voice(
    voices: &mut [ActiveKeyboardVoice; MAX_RACK_SLOTS],
    voice_count: &mut usize,
    voice: ActiveKeyboardVoice,
) {
    if *voice_count >= MAX_RACK_SLOTS || voice.logical_id == 0 {
        return;
    }
    voices[*voice_count] = voice;
    *voice_count += 1;
}

fn fire_live_keyboard_rack_note(
    data: &mut AudioCallbackData,
    parent_track_idx: usize,
    trigger: &KeyboardTrigger,
    transpose: f32,
    rack: RackTrackSnapshot,
) -> bool {
    let gate_mode = if data.state.pattern.track_params[parent_track_idx].is_gate_on() {
        1.0
    } else {
        0.0
    };
    let has_solo = rack.slots.iter().any(|slot| slot.solo);
    let mut active_voices = [ActiveKeyboardVoice::default(); MAX_RACK_SLOTS];
    let mut active_voice_count = 0;

    for (slot_idx, slot) in rack.slots.iter().enumerate() {
        if !rack_slot_matches_routing(slot, rack.routing, transpose) {
            continue;
        }
        if !rack_slot_accepts_trigger(slot, has_solo) {
            continue;
        }
        if let Some(choke_group) = slot.choke_group {
            release_rack_choke_group_voices(
                data,
                parent_track_idx,
                &rack,
                slot_idx,
                choke_group,
                0,
            );
        }
        let playback_transpose = rack_slot_playback_transpose(rack.routing, transpose);
        let instrument_params = resolve_rack_slot_instrument_defaults(&slot.instrument_slot);
        match slot.instrument_type {
            InstrumentType::Sampler => {
                let Some(pool_id) = rack_slot_pool_index(parent_track_idx, slot_idx) else {
                    continue;
                };
                if pool_id >= data.voice_pools.len() {
                    continue;
                }
                let sampler_lid = data.state.runtime.sampler_lids[pool_id].load(Ordering::Acquire);
                if sampler_lid == 0 {
                    continue;
                }
                let sampler_params = resolve_rack_slot_sampler_defaults(&slot.instrument_slot);
                let attack_samples = sampler_params.attack_ms * data.sample_rate as f32 / 1000.0;
                let release_samples = sampler_params.release_ms * data.sample_rate as f32 / 1000.0;
                let loop_xfade_samples =
                    sampler_params.loop_xfade_ms * data.sample_rate as f32 / 1000.0;
                let (
                    warp_enabled,
                    warp_mode,
                    warp_ratio,
                    warp_sample_bpm,
                    warp_project_bpm,
                    warp_ptr_lo,
                    warp_ptr_hi,
                ) = rack_sampler_warp_runtime(
                    &data.state,
                    sampler_params.warp_enabled,
                    sampler_params.warp_mode,
                    sampler_params.sample_bpm,
                );
                data.voice_pools[pool_id].polyphonic = slot.max_polyphony > 1;
                let (voice_lid, gatepitch_id, modulator_id) = {
                    let voice = data.voice_pools[pool_id]
                        .allocate_voice_retriggering_same_note_with_limit(
                            playback_transpose,
                            slot.max_polyphony,
                        );
                    (voice.logical_id, voice.gatepitch_id, voice.modulator_id)
                };
                if voice_lid == 0 {
                    continue;
                }
                if modulator_id > 0 {
                    let gatepitch_seq = next_event_sequence_from(&mut data.event_seq);
                    unsafe {
                        dispatch_sampler_modulator_params_to_voice(
                            data.lg.0,
                            modulator_id as u64,
                            &instrument_params,
                        );
                        send_custom_trigger(
                            data.lg.0,
                            gatepitch_id as u64,
                            0,
                            gatepitch_seq,
                            custom_pitch_hz(
                                playback_transpose + slot.instrument_base_note_offset,
                                0.0,
                            ),
                            trigger.velocity,
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
                        playback_transpose + slot.instrument_base_note_offset,
                        trigger.velocity,
                        sampler_params.playback_speed,
                        attack_samples,
                        release_samples,
                        gate_mode,
                        sampler_params.start_point,
                        sampler_params.end_point,
                        sampler_params.instrument_enabled,
                        sampler_params.reverse,
                        sampler_params.loop_mode,
                        loop_xfade_samples,
                        sampler_params.sr_hz,
                        warp_enabled,
                        warp_mode,
                        warp_ratio,
                        warp_sample_bpm,
                        warp_project_bpm,
                        warp_ptr_lo,
                        warp_ptr_hi,
                        sampler_params.warp_preserve,
                        sampler_params.warp_seg_loop_mode,
                        sampler_params.warp_seg_envelope,
                        sampler_params.scrub,
                    );
                    dispatch_sampler_extra_params_to_voice(
                        data.lg.0,
                        voice_lid,
                        &instrument_params,
                    );
                }
                push_active_keyboard_voice(
                    &mut active_voices,
                    &mut active_voice_count,
                    ActiveKeyboardVoice {
                        logical_id: voice_lid,
                        gatepitch_id,
                        target: ActiveKeyboardVoiceTarget::Sampler { pool_id },
                    },
                );
            }
            InstrumentType::Custom => {
                let Some(engine_id) = slot.track_sound_state.engine_id else {
                    continue;
                };
                if engine_id >= data.custom_engine_pools.len() {
                    continue;
                }
                let free_patch = slot.instrument_run_mode == CustomInstrumentRunMode::FreePatch;
                let allocation = if free_patch {
                    let Some(allocation) = data.custom_engine_pools[engine_id]
                        .allocate_free_patch_voice(
                            parent_track_idx,
                            rack_slot_pool_index(parent_track_idx, slot_idx)
                                .expect("validated rack slot must have a route identity"),
                            playback_transpose,
                        )
                    else {
                        continue;
                    };
                    allocation
                } else {
                    data.custom_engine_pools[engine_id].allocate_voice(
                        parent_track_idx,
                        rack_slot_pool_index(parent_track_idx, slot_idx)
                            .expect("validated rack slot must have a route identity"),
                        playback_transpose,
                        slot.max_polyphony > 1,
                        slot.max_polyphony,
                    )
                };
                let voice_idx = allocation.voice_idx;
                data.custom_engine_pools[engine_id].note_voice_allocated(engine_id, voice_idx);
                let voice_lid = allocation.logical_id;
                let synth_id = data.state.runtime.engine_synth_node_ids[engine_id][voice_idx]
                    .load(Ordering::Relaxed);
                let modulator_id = data.state.runtime.engine_modulator_node_ids[engine_id]
                    [voice_idx]
                    .load(Ordering::Relaxed);
                if voice_lid == 0 || synth_id == 0 || modulator_id == 0 {
                    continue;
                }
                let key_locked_instrument_params = key_locked_snapshot_instrument_params(
                    &slot.instrument_slot,
                    playback_transpose,
                    slot.instrument_base_note_offset,
                    None,
                    &instrument_params,
                );
                let instrument_fingerprint = rack_slot_sound_fingerprint(
                    slot,
                    &key_locked_instrument_params,
                    slot.instrument_base_note_offset,
                );
                let pitch_hz =
                    custom_pitch_hz(playback_transpose, slot.instrument_base_note_offset);
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
                        rack_slot_pool_index(parent_track_idx, slot_idx)
                            .expect("validated rack slot must have a route identity"),
                    );
                    if data.custom_engine_pools[engine_id].voices[voice_idx].fingerprint
                        != instrument_fingerprint
                    {
                        dispatch_instrument_params_to_voice(
                            data.lg.0,
                            synth_id as u64,
                            modulator_id as u64,
                            &key_locked_instrument_params,
                        );
                    }
                    if allocation.stole_active_voice || slot.max_polyphony <= 1 || free_patch {
                        let off_seq = next_event_sequence_from(&mut data.event_seq);
                        send_custom_note_off(data.lg.0, voice_lid, 0, off_seq);
                    }
                    let on_seq = next_event_sequence_from(&mut data.event_seq);
                    send_custom_trigger(
                        data.lg.0,
                        voice_lid,
                        0,
                        on_seq,
                        pitch_hz,
                        trigger.velocity,
                    );
                }
                data.custom_engine_pools[engine_id].voices[voice_idx].fingerprint =
                    instrument_fingerprint;
                push_active_keyboard_voice(
                    &mut active_voices,
                    &mut active_voice_count,
                    ActiveKeyboardVoice {
                        logical_id: voice_lid,
                        gatepitch_id: 0,
                        target: ActiveKeyboardVoiceTarget::Custom {
                            engine_id,
                            free_patch,
                        },
                    },
                );
            }
            InstrumentType::Modulator | InstrumentType::Rack => {}
        }
    }

    if active_voice_count == 0 {
        return false;
    }
    store_active_keyboard_note(
        &mut data.active_keyboard_notes,
        parent_track_idx,
        trigger.transpose,
        midi_note_from_transpose(
            transpose,
            f32::from_bits(
                data.state.pattern.instrument_base_note_offsets[parent_track_idx]
                    .load(Ordering::Relaxed),
            ),
        ),
        &active_voices[..active_voice_count],
    );
    true
}

#[allow(clippy::too_many_arguments)]
fn fire_rack_slot_note(
    data: &mut AudioCallbackData,
    frame_offset: u32,
    parent_track_idx: usize,
    slot_idx: usize,
    slot: &RackSlotSnapshot,
    slot_params: ResolvedRackSlotParams,
    transpose: f32,
    velocity: f32,
    speed: f32,
    gate_samples: f32,
    gate_mode: f32,
    instrument_params: &ScheduledInstrumentParams,
    sampler_params: Option<ScheduledSamplerParams>,
    instrument_fingerprint: u64,
) {
    match slot.instrument_type {
        InstrumentType::Sampler => {
            let Some(pool_id) = rack_slot_pool_index(parent_track_idx, slot_idx) else {
                return;
            };
            if pool_id >= data.voice_pools.len() {
                return;
            }
            let sampler_lid = data.state.runtime.sampler_lids[pool_id].load(Ordering::Acquire);
            if sampler_lid == 0 {
                return;
            }
            let sampler_params = sampler_params.unwrap_or_default();
            let attack_samples = sampler_params.attack_ms * data.sample_rate as f32 / 1000.0;
            let release_samples = sampler_params.release_ms * data.sample_rate as f32 / 1000.0;
            let loop_xfade_samples =
                sampler_params.loop_xfade_ms * data.sample_rate as f32 / 1000.0;
            let (
                warp_enabled,
                warp_mode,
                warp_ratio,
                warp_sample_bpm,
                warp_project_bpm,
                warp_ptr_lo,
                warp_ptr_hi,
            ) = rack_sampler_warp_runtime(
                &data.state,
                sampler_params.warp_enabled,
                sampler_params.warp_mode,
                sampler_params.sample_bpm,
            );
            data.voice_pools[pool_id].polyphonic = slot_params.max_polyphony > 1;
            let voice = data.voice_pools[pool_id].allocate_voice_retriggering_same_note_with_limit(
                transpose,
                slot_params.max_polyphony,
            );
            let voice_lid = voice.logical_id;
            let lid = if voice_lid != 0 {
                voice_lid
            } else {
                sampler_lid
            };
            let gatepitch_id = voice.gatepitch_id;
            if voice.modulator_id > 0 {
                let gatepitch_seq = next_event_sequence_from(&mut data.event_seq);
                unsafe {
                    dispatch_sampler_modulator_params_to_voice(
                        data.lg.0,
                        voice.modulator_id as u64,
                        instrument_params,
                    );
                    send_custom_trigger(
                        data.lg.0,
                        voice.gatepitch_id as u64,
                        frame_offset,
                        gatepitch_seq,
                        custom_pitch_hz(transpose + slot_params.base_note_offset, 0.0),
                        velocity,
                    );
                }
            }
            let sampler_seq = next_event_sequence_from(&mut data.event_seq);
            unsafe {
                dispatch_sampler_extra_params_to_voice(data.lg.0, lid, instrument_params);
                send_trigger(
                    data.lg.0,
                    lid,
                    frame_offset,
                    sampler_seq,
                    velocity,
                    speed * sampler_params.playback_speed,
                    gate_samples,
                    attack_samples,
                    release_samples,
                    gate_mode,
                    transpose + slot_params.base_note_offset,
                    sampler_params.start_point,
                    sampler_params.end_point,
                    sampler_params.instrument_enabled,
                    sampler_params.reverse,
                    sampler_params.loop_mode,
                    loop_xfade_samples,
                    sampler_params.sr_hz,
                    warp_enabled,
                    warp_mode,
                    warp_ratio,
                    warp_sample_bpm,
                    warp_project_bpm,
                    warp_ptr_lo,
                    warp_ptr_hi,
                    sampler_params.warp_preserve,
                    sampler_params.warp_seg_loop_mode,
                    sampler_params.warp_seg_envelope,
                    sampler_params.scrub,
                );
            }
            if gate_mode > 0.5 {
                schedule_gate_off_event(
                    data,
                    pool_id,
                    lid,
                    frame_offset,
                    gate_samples as f64,
                    GateOffTarget::Sampler { gatepitch_id },
                );
            }
        }
        InstrumentType::Custom => {
            let Some(engine_id) = slot.track_sound_state.engine_id else {
                return;
            };
            if engine_id >= data.custom_engine_pools.len() {
                return;
            }
            let free_patch = slot.instrument_run_mode == CustomInstrumentRunMode::FreePatch;
            let allocation = if free_patch {
                let Some(allocation) = data.custom_engine_pools[engine_id]
                    .allocate_free_patch_voice(
                        parent_track_idx,
                        rack_slot_pool_index(parent_track_idx, slot_idx)
                            .expect("validated rack slot must have a route identity"),
                        transpose,
                    )
                else {
                    return;
                };
                allocation
            } else {
                data.custom_engine_pools[engine_id].allocate_voice(
                    parent_track_idx,
                    rack_slot_pool_index(parent_track_idx, slot_idx)
                        .expect("validated rack slot must have a route identity"),
                    transpose,
                    slot_params.max_polyphony > 1,
                    slot_params.max_polyphony,
                )
            };
            let voice_idx = allocation.voice_idx;
            data.custom_engine_pools[engine_id].note_voice_allocated(engine_id, voice_idx);
            let lid = allocation.logical_id;
            let synth_id = data.state.runtime.engine_synth_node_ids[engine_id][voice_idx]
                .load(Ordering::Relaxed);
            let modulator_id = data.state.runtime.engine_modulator_node_ids[engine_id][voice_idx]
                .load(Ordering::Relaxed);
            if lid == 0 || synth_id == 0 || modulator_id == 0 {
                return;
            }
            let pitch_hz = custom_pitch_hz(transpose, slot_params.base_note_offset);
            cancel_gate_off_for_lid(&mut data.countdown_events, &mut data.block_events, lid);
            unsafe {
                if allocation.stole_active_voice || slot_params.max_polyphony <= 1 || free_patch {
                    let off_seq = next_event_sequence_from(&mut data.event_seq);
                    send_custom_note_off(data.lg.0, lid, frame_offset, off_seq);
                }
                route_custom_voice_to_consumer(
                    data.lg.0,
                    &data.state,
                    engine_id,
                    voice_idx,
                    allocation.previous_route,
                    rack_slot_pool_index(parent_track_idx, slot_idx)
                        .expect("validated rack slot must have a route identity"),
                );
                if data.custom_engine_pools[engine_id].voices[voice_idx].fingerprint
                    != instrument_fingerprint
                {
                    dispatch_instrument_params_to_voice(
                        data.lg.0,
                        synth_id as u64,
                        modulator_id as u64,
                        instrument_params,
                    );
                }
            }
            data.custom_engine_pools[engine_id].voices[voice_idx].fingerprint =
                instrument_fingerprint;
            let on_seq = next_event_sequence_from(&mut data.event_seq);
            unsafe {
                send_custom_trigger(data.lg.0, lid, frame_offset, on_seq, pitch_hz, velocity);
            }
            if gate_mode > 0.5 {
                schedule_gate_off_event(
                    data,
                    parent_track_idx,
                    lid,
                    frame_offset,
                    gate_samples as f64,
                    GateOffTarget::Custom {
                        engine_id,
                        free_patch,
                    },
                );
            }
        }
        InstrumentType::Modulator | InstrumentType::Rack => {}
    }
}

#[allow(clippy::too_many_arguments)]
fn fire_rack_resolved(
    data: &mut AudioCallbackData,
    frame_offset: u32,
    track_idx: usize,
    step: usize,
    key_lock_plock_step: Option<usize>,
    samples_per_step: f64,
    resolved: crate::accumulator::ResolvedStep,
    chord: crate::scheduled_event::ScheduledChordData,
    mut rack: RackTrackSnapshot,
    rack_macro_values: [Option<f32>; crate::sequencer::RACK_MACRO_COUNT],
) {
    apply_rack_macros_at_step(&mut rack, step, rack_macro_values);
    let (track_pan, track_send, gate_mode) = {
        let tp = &data.state.pattern.track_params[track_idx];
        (
            tp.get_pan(),
            tp.get_send(),
            if tp.is_gate_on() { 1.0 } else { 0.0 },
        )
    };
    let chop = (resolved.chop.round() as u32).max(1);
    let total_gate = (resolved.duration as f64 * samples_per_step) as f32;
    let rack_gate = total_gate / chop as f32;

    let pan_lid = data.state.runtime.pan_lids[track_idx].load(Ordering::Acquire);
    if pan_lid != 0 {
        let effective_pan = (track_pan + resolved.pan).clamp(-1.0, 1.0);
        unsafe {
            crate::audiograph::params_push_wrapper(
                data.lg.0,
                crate::audiograph::ParamMsg {
                    idx: crate::effects::stereo_panner::STEREO_PANNER_PARAM_PAN,
                    logical_id: pan_lid,
                    fvalue: effective_pan,
                },
            );
        }
    }

    let resolved_slot_params: Vec<ResolvedRackSlotParams> = rack
        .slots
        .iter()
        .map(|slot| resolve_rack_slot_params(slot, step))
        .collect();
    let has_solo = resolved_slot_params.iter().any(|params| params.solo);
    for (slot_idx, slot) in rack.slots.iter().enumerate() {
        let Some(slot_params) = resolved_slot_params.get(slot_idx).copied() else {
            continue;
        };
        let muted_by_solo = has_solo && !slot_params.solo;
        let slot_pan_lid =
            data.state.runtime.rack_slot_pan_lids[track_idx][slot_idx].load(Ordering::Acquire);
        unsafe {
            push_rack_slot_panner_params(data.lg.0, slot_pan_lid, slot_params, muted_by_solo);
        }
        if !rack_slot_accepts_resolved(slot_params, has_solo) {
            continue;
        }
        let receives_trigger = if chord.count > 0 {
            (0..chord.count).any(|note_idx| {
                let transpose = resolved_chord_transpose(
                    chord.notes[note_idx],
                    chord.step_transpose,
                    resolved.transpose,
                );
                rack_slot_matches_routing(slot, rack.routing, transpose)
            })
        } else {
            rack_slot_matches_routing(slot, rack.routing, resolved.transpose)
        };
        if !receives_trigger {
            continue;
        }
        unsafe {
            dispatch_snapshot_effect_params_at_step(data.lg.0, &slot.effect_slots, step);
        }
        let instrument_params = resolve_rack_slot_instrument_params(&slot.instrument_slot, step);
        let sampler_params = if slot.instrument_type == InstrumentType::Sampler {
            Some(resolve_rack_slot_sampler_params(
                &slot.instrument_slot,
                step,
            ))
        } else {
            None
        };

        if chord.count > 0 {
            for n in 0..chord.count {
                let note_duration = chord.durations[n].max(0.0);
                let note_total_gate = if note_duration > 0.0 {
                    (note_duration as f64 * samples_per_step) as f32
                } else {
                    total_gate
                };
                let note_gate = note_total_gate / chop as f32;
                let transpose = resolved_chord_transpose(
                    chord.notes[n],
                    chord.step_transpose,
                    resolved.transpose,
                );
                if !rack_slot_matches_routing(slot, rack.routing, transpose) {
                    continue;
                }
                if let Some(choke_group) = slot.choke_group {
                    release_rack_choke_group_voices(
                        data,
                        track_idx,
                        &rack,
                        slot_idx,
                        choke_group,
                        frame_offset,
                    );
                }
                let playback_transpose = rack_slot_playback_transpose(rack.routing, transpose);
                let note_instrument_params = if slot.instrument_type == InstrumentType::Custom {
                    key_locked_snapshot_instrument_params(
                        &slot.instrument_slot,
                        playback_transpose,
                        slot_params.base_note_offset,
                        key_lock_plock_step,
                        &instrument_params,
                    )
                } else {
                    instrument_params.clone()
                };
                let instrument_fingerprint = rack_slot_sound_fingerprint(
                    slot,
                    &note_instrument_params,
                    slot_params.base_note_offset,
                );
                fire_rack_slot_note(
                    data,
                    frame_offset,
                    track_idx,
                    slot_idx,
                    slot,
                    slot_params,
                    playback_transpose,
                    resolved.velocity,
                    resolved.speed,
                    note_gate,
                    gate_mode,
                    &note_instrument_params,
                    sampler_params,
                    instrument_fingerprint,
                );
            }
        } else {
            if !rack_slot_matches_routing(slot, rack.routing, resolved.transpose) {
                continue;
            }
            if let Some(choke_group) = slot.choke_group {
                release_rack_choke_group_voices(
                    data,
                    track_idx,
                    &rack,
                    slot_idx,
                    choke_group,
                    frame_offset,
                );
            }
            let playback_transpose = rack_slot_playback_transpose(rack.routing, resolved.transpose);
            let note_instrument_params = if slot.instrument_type == InstrumentType::Custom {
                key_locked_snapshot_instrument_params(
                    &slot.instrument_slot,
                    playback_transpose,
                    slot_params.base_note_offset,
                    key_lock_plock_step,
                    &instrument_params,
                )
            } else {
                instrument_params.clone()
            };
            let instrument_fingerprint = rack_slot_sound_fingerprint(
                slot,
                &note_instrument_params,
                slot_params.base_note_offset,
            );
            fire_rack_slot_note(
                data,
                frame_offset,
                track_idx,
                slot_idx,
                slot,
                slot_params,
                playback_transpose,
                resolved.velocity,
                resolved.speed,
                rack_gate,
                gate_mode,
                &note_instrument_params,
                sampler_params,
                instrument_fingerprint,
            );
        }
    }

    let send_lid = data.state.runtime.send_lids[track_idx].load(Ordering::Acquire);
    if send_lid != 0 {
        unsafe {
            params_push_wrapper(
                data.lg.0,
                ParamMsg {
                    idx: 0,
                    logical_id: send_lid,
                    fvalue: track_send,
                },
            );
        }
    }
    cancel_chops_for_track(
        &mut data.countdown_events,
        &mut data.block_events,
        track_idx,
    );
    data.state.transport.trigger_flash[track_idx].store(255, Ordering::Relaxed);
}

/// Fire a resolved step trigger for a track (handles gate, chop setup, envelope params).
/// Uses voice pool allocation for polyphonic playback.
fn midi_note_from_transpose(transpose: f32, base_note_offset: f32) -> Option<u8> {
    let note = (60.0 + transpose + base_note_offset).round();
    (0.0..=127.0).contains(&note).then_some(note as u8)
}

fn mark_resolved_note_activity(
    data: &AudioCallbackData,
    frame_offset: u32,
    track_idx: usize,
    samples_per_step: f64,
    resolved: crate::accumulator::ResolvedStep,
    chord: crate::scheduled_event::ScheduledChordData,
) {
    let base_note_offset = f32::from_bits(
        data.state.pattern.instrument_base_note_offsets[track_idx].load(Ordering::Relaxed),
    );
    let start_sample = data.rendered_samples.load(Ordering::Acquire) + frame_offset as u64;
    let mark = |transpose: f32, duration_steps: f32| {
        let Some(note) = midi_note_from_transpose(transpose, base_note_offset) else {
            return;
        };
        let gate_samples = (duration_steps.max(0.0) as f64 * samples_per_step.max(0.0))
            .round()
            .max(1.0) as u64;
        data.state.mark_scheduled_note_active_until(
            track_idx,
            note,
            start_sample.saturating_add(gate_samples),
        );
    };

    if chord.count > 0 {
        for idx in 0..chord.count.min(MAX_VOICES) {
            let duration = if chord.durations[idx] > 0.0 {
                chord.durations[idx]
            } else {
                resolved.duration
            };
            mark(
                crate::scheduled_event::resolved_chord_transpose(
                    chord.notes[idx],
                    chord.step_transpose,
                    resolved.transpose,
                ),
                duration,
            );
        }
    } else {
        mark(resolved.transpose, resolved.duration);
    }
}

fn fire_resolved(
    data: &mut AudioCallbackData,
    frame_offset: u32,
    track_idx: usize,
    step: usize,
    key_lock_plock_step: Option<usize>,
    samples_per_step: f64,
    resolved: crate::accumulator::ResolvedStep,
    chord: crate::scheduled_event::ScheduledChordData,
    instrument_params: ScheduledInstrumentParams,
    instrument_tensor_params: ScheduledInstrumentTensorParams,
    instrument_fingerprint: u64,
    scheduled_sampler_params: Option<ScheduledSamplerParams>,
    rack_macro_values: [Option<f32>; crate::sequencer::RACK_MACRO_COUNT],
) {
    if !track_accepts_scheduled_trigger(&data.state, track_idx) {
        return;
    }
    let tp = &data.state.pattern.track_params[track_idx];
    let instrument_type = InstrumentType::from_runtime_flag(
        data.state.runtime.instrument_type_flags[track_idx].load(Ordering::Relaxed),
    );
    mark_resolved_note_activity(
        data,
        frame_offset,
        track_idx,
        samples_per_step,
        resolved,
        chord,
    );
    if instrument_type == InstrumentType::Rack {
        let rack = data
            .scheduler_snapshot
            .tracks
            .get(track_idx)
            .and_then(|track| track.rack_track.clone());
        if let Some(rack) = rack {
            fire_rack_resolved(
                data,
                frame_offset,
                track_idx,
                step,
                key_lock_plock_step,
                samples_per_step,
                resolved,
                chord,
                rack,
                rack_macro_values,
            );
        }
        return;
    }
    let is_custom = instrument_type == InstrumentType::Custom;
    let is_modulator = instrument_type == InstrumentType::Modulator;
    let sampler_lid = data.state.runtime.sampler_lids[track_idx].load(Ordering::Acquire);
    if !is_custom && !is_modulator && sampler_lid == 0 {
        return;
    }

    let chop = resolved.chop.round() as u32;
    let chop = chop.max(1);

    let total_gate = (resolved.duration as f64 * samples_per_step) as f32;
    let chop_gate = total_gate / chop as f32;

    let fallback_sampler_params = || {
        let inst_slot = &data.state.pattern.instrument_slots[track_idx];
        ScheduledSamplerParams {
            attack_ms: inst_slot
                .plocks
                .get(step, 0)
                .unwrap_or_else(|| inst_slot.defaults.get(0)),
            release_ms: inst_slot
                .plocks
                .get(step, 1)
                .unwrap_or_else(|| inst_slot.defaults.get(1)),
            start_point: inst_slot
                .plocks
                .get(step, 2)
                .unwrap_or_else(|| inst_slot.defaults.get(2)),
            end_point: inst_slot
                .plocks
                .get(step, 3)
                .unwrap_or_else(|| inst_slot.defaults.get(3)),
            instrument_enabled: inst_slot
                .plocks
                .get(step, 4)
                .unwrap_or_else(|| inst_slot.defaults.get(4)),
            reverse: inst_slot
                .plocks
                .get(step, 5)
                .unwrap_or_else(|| inst_slot.defaults.get(5)),
            loop_mode: inst_slot
                .plocks
                .get(step, 6)
                .unwrap_or_else(|| inst_slot.defaults.get(6)),
            loop_xfade_ms: inst_slot
                .plocks
                .get(step, 7)
                .unwrap_or_else(|| inst_slot.defaults.get(7)),
            sr_hz: inst_slot
                .plocks
                .get(step, 8)
                .unwrap_or_else(|| inst_slot.defaults.get(8)),
            warp_enabled: inst_slot
                .plocks
                .get(step, 9)
                .unwrap_or_else(|| inst_slot.defaults.get(9)),
            warp_mode: inst_slot
                .plocks
                .get(step, 10)
                .unwrap_or_else(|| inst_slot.defaults.get(10)),
            sample_bpm: inst_slot
                .plocks
                .get(step, 11)
                .unwrap_or_else(|| inst_slot.defaults.get(11)),
            playback_speed: inst_slot
                .plocks
                .get(step, 12)
                .unwrap_or_else(|| inst_slot.defaults.get(12)),
            scrub: inst_slot
                .plocks
                .get(step, 13)
                .unwrap_or_else(|| inst_slot.defaults.get(13)),
            warp_preserve: live_slot_resolved_node_param_value(
                inst_slot,
                step,
                crate::sampler::PARAM_WARP_PRESERVE,
                crate::sampler::WARP_PRESERVE_DEFAULT as f32,
            ),
            warp_seg_loop_mode: live_slot_resolved_node_param_value(
                inst_slot,
                step,
                crate::sampler::PARAM_WARP_SEG_LOOP_MODE,
                crate::sampler::WARP_SEG_LOOP_MODE_DEFAULT as f32,
            ),
            warp_seg_envelope: live_slot_resolved_node_param_value(
                inst_slot,
                step,
                crate::sampler::PARAM_WARP_SEG_ENVELOPE,
                crate::sampler::WARP_SEG_ENVELOPE_DEFAULT,
            ),
        }
    };
    let scheduled_source = scheduled_sampler_params.is_some();
    let sampler_params = scheduled_sampler_params.unwrap_or_else(fallback_sampler_params);
    if crate::sampler::srange_debug_enabled() {
        eprintln!(
            "[srange] trigger dispatch track={} step={} source={} start={} end={}",
            track_idx,
            step,
            if scheduled_source { "scheduled" } else { "fallback" },
            sampler_params.start_point,
            sampler_params.end_point,
        );
    }
    let attack_ms = sampler_params.attack_ms;
    let release_ms = sampler_params.release_ms;
    let attack_samples = attack_ms * data.sample_rate as f32 / 1000.0;
    let release_samples = release_ms * data.sample_rate as f32 / 1000.0;
    let gate_mode = if tp.is_gate_on() { 1.0 } else { 0.0 };
    let track_send = tp.get_send();
    let start_point = sampler_params.start_point;
    let end_point = sampler_params.end_point;
    let instrument_enabled = sampler_params.instrument_enabled;
    let reverse = sampler_params.reverse;
    let loop_mode = sampler_params.loop_mode;
    let loop_xfade_samples = sampler_params.loop_xfade_ms * data.sample_rate as f32 / 1000.0;
    let sr_hz = sampler_params.sr_hz;
    let warp_enabled = sampler_params.warp_enabled;
    let warp_mode = sampler_params.warp_mode;
    let sample_bpm = sampler_params.sample_bpm;
    let playback_speed = sampler_params.playback_speed;
    let scrub = sampler_params.scrub;
    let warp_preserve = sampler_params.warp_preserve;
    let warp_seg_loop_mode = sampler_params.warp_seg_loop_mode;
    let warp_seg_envelope = sampler_params.warp_seg_envelope;
    let (
        warp_enabled,
        warp_mode,
        warp_ratio,
        warp_sample_bpm,
        warp_project_bpm,
        warp_ptr_lo,
        warp_ptr_hi,
    ) = sampler_warp_runtime(&data.state, track_idx, warp_enabled, warp_mode, sample_bpm);
    let velocity = resolved.velocity;
    let base_note_offset = f32::from_bits(
        data.state.pattern.instrument_base_note_offsets[track_idx].load(Ordering::Relaxed),
    );
    let step_transpose = chord.step_transpose;
    let pan_lid = data.state.runtime.pan_lids[track_idx].load(Ordering::Acquire);
    if pan_lid != 0 {
        let effective_pan = (tp.get_pan() + resolved.pan).clamp(-1.0, 1.0);
        unsafe {
            crate::audiograph::params_push_wrapper(
                data.lg.0,
                crate::audiograph::ParamMsg {
                    idx: crate::effects::stereo_panner::STEREO_PANNER_PARAM_PAN,
                    logical_id: pan_lid,
                    fvalue: effective_pan,
                },
            );
        }
    }

    if is_modulator {
        let lid = data.state.runtime.modulator_lids[track_idx].load(Ordering::Acquire);
        if lid == 0 {
            return;
        }
        let seq = next_block_event_sequence(data);
        unsafe {
            dispatch_modulator_params(data.lg.0, lid, &instrument_params);
            trigger_modulator_pulse(
                data.lg.0,
                lid,
                frame_offset,
                seq,
                chop_gate,
                resolved.velocity,
            );
        }
        if chop > 1 {
            schedule_chop_events(
                data,
                track_idx,
                frame_offset,
                chop_gate as f64,
                chop_gate as f64,
                chop - 1,
                step,
                chop_gate,
            );
        } else {
            cancel_chops_for_track(
                &mut data.countdown_events,
                &mut data.block_events,
                track_idx,
            );
        }
        data.state.transport.trigger_flash[track_idx].store(255, Ordering::Relaxed);
        return;
    }

    // Sync polyphonic setting from track params
    let track_polyphonic = tp.is_polyphonic();
    let track_max_polyphony = tp.get_max_polyphony();
    data.voice_pools[track_idx].polyphonic = track_polyphonic;
    let engine_id = if is_custom {
        track_engine_id(&data.state, track_idx)
    } else {
        None
    };
    let free_patch = is_custom
        && track_custom_run_mode(&data.state, track_idx) == CustomInstrumentRunMode::FreePatch;

    // Check chord data: if chord has notes, trigger each note on its own voice
    let chord_count = chord.count;
    if chord_count > 0 {
        for n in 0..chord_count {
            let note_duration = chord.durations[n].max(0.0);
            let note_total_gate = if note_duration > 0.0 {
                (note_duration as f64 * samples_per_step) as f32
            } else {
                total_gate
            };
            let note_chop_gate = note_total_gate / chop as f32;
            let transpose =
                resolved_chord_transpose(chord.notes[n], step_transpose, resolved.transpose);
            if is_custom {
                let Some(engine_id) = engine_id else {
                    continue;
                };
                let allocation = if free_patch {
                    let Some(allocation) = data.custom_engine_pools[engine_id]
                        .allocate_free_patch_voice(track_idx, track_idx, transpose)
                    else {
                        continue;
                    };
                    allocation
                } else {
                    data.custom_engine_pools[engine_id].allocate_voice(
                        track_idx,
                        track_idx,
                        transpose,
                        track_polyphonic,
                        track_max_polyphony,
                    )
                };
                let voice_idx = allocation.voice_idx;
                data.custom_engine_pools[engine_id].note_voice_allocated(engine_id, voice_idx);
                let lid = allocation.logical_id;
                let synth_id = data.state.runtime.engine_synth_node_ids[engine_id][voice_idx]
                    .load(Ordering::Relaxed);
                let modulator_id = data.state.runtime.engine_modulator_node_ids[engine_id]
                    [voice_idx]
                    .load(Ordering::Relaxed);
                if lid == 0 || synth_id == 0 || modulator_id == 0 {
                    continue;
                }
                if data.trace_audio {
                    let enabled = data.custom_engine_pools[engine_id].enabled_voice_count;
                    eprintln!(
                        "audio-trace: scheduled custom note-on track={track_idx} engine={engine_id} voice={voice_idx} lid={lid} synth={synth_id} mod={modulator_id} chord_note={n} enabled_voices={enabled} poly={track_polyphonic} stolen={}",
                        allocation.stole_active_voice,
                    );
                    data.trace_render_probe_blocks = data.trace_render_probe_blocks.max(12);
                }
                let pitch_hz = custom_pitch_hz(transpose, base_note_offset);
                let key_locked_params = key_locked_live_instrument_params(
                    &data.state,
                    track_idx,
                    transpose,
                    base_note_offset,
                    key_lock_plock_step,
                    &instrument_params,
                );
                let note_fingerprint = instrument_param_bundle_fingerprint(
                    engine_id,
                    base_note_offset,
                    &key_locked_params,
                    &instrument_tensor_params,
                );
                cancel_gate_off_for_lid(&mut data.countdown_events, &mut data.block_events, lid);
                if allocation.stole_active_voice || !track_polyphonic || free_patch {
                    let off_seq = next_event_sequence_from(&mut data.event_seq);
                    unsafe {
                        send_custom_note_off(data.lg.0, lid, frame_offset, off_seq);
                        route_custom_voice_to_consumer(
                            data.lg.0,
                            &data.state,
                            engine_id,
                            voice_idx,
                            allocation.previous_route,
                            track_idx,
                        );
                        if data.custom_engine_pools[engine_id].voices[voice_idx].fingerprint
                            != note_fingerprint
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
                                &instrument_tensor_params,
                            );
                        }
                    }
                } else {
                    unsafe {
                        route_custom_voice_to_consumer(
                            data.lg.0,
                            &data.state,
                            engine_id,
                            voice_idx,
                            allocation.previous_route,
                            track_idx,
                        );
                        if data.custom_engine_pools[engine_id].voices[voice_idx].fingerprint
                            != note_fingerprint
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
                                &instrument_tensor_params,
                            );
                        }
                    }
                }
                data.custom_engine_pools[engine_id].voices[voice_idx].fingerprint =
                    note_fingerprint;
                let on_seq = next_event_sequence_from(&mut data.event_seq);
                unsafe {
                    send_custom_trigger(data.lg.0, lid, frame_offset, on_seq, pitch_hz, velocity);
                }
                if gate_mode > 0.5 {
                    schedule_gate_off_event(
                        data,
                        track_idx,
                        lid,
                        frame_offset,
                        note_total_gate as f64,
                        GateOffTarget::Custom {
                            engine_id,
                            free_patch,
                        },
                    );
                }
            } else {
                let voice =
                    data.voice_pools[track_idx].allocate_voice_retriggering_same_note(transpose);
                let voice_lid = voice.logical_id;
                let lid = if voice_lid != 0 {
                    voice_lid
                } else {
                    sampler_lid
                };
                let gatepitch_id = voice.gatepitch_id;
                if voice.modulator_id > 0 {
                    let gatepitch_seq = next_event_sequence_from(&mut data.event_seq);
                    unsafe {
                        dispatch_sampler_modulator_params_to_voice(
                            data.lg.0,
                            voice.modulator_id as u64,
                            &instrument_params,
                        );
                        send_custom_trigger(
                            data.lg.0,
                            voice.gatepitch_id as u64,
                            frame_offset,
                            gatepitch_seq,
                            custom_pitch_hz(transpose + base_note_offset, 0.0),
                            velocity,
                        );
                    }
                }
                let sampler_seq = next_event_sequence_from(&mut data.event_seq);
                unsafe {
                    dispatch_sampler_extra_params_to_voice(data.lg.0, lid, &instrument_params);
                    send_trigger(
                        data.lg.0,
                        lid,
                        frame_offset,
                        sampler_seq,
                        velocity,
                        resolved.speed * playback_speed,
                        note_chop_gate,
                        attack_samples,
                        release_samples,
                        gate_mode,
                        transpose + base_note_offset,
                        start_point,
                        end_point,
                        instrument_enabled,
                        reverse,
                        loop_mode,
                        loop_xfade_samples,
                        sr_hz,
                        warp_enabled,
                        warp_mode,
                        warp_ratio,
                        warp_sample_bpm,
                        warp_project_bpm,
                        warp_ptr_lo,
                        warp_ptr_hi,
                        warp_preserve,
                        warp_seg_loop_mode,
                        warp_seg_envelope,
                        scrub,
                    );
                }
                if gate_mode > 0.5 {
                    schedule_gate_off_event(
                        data,
                        track_idx,
                        lid,
                        frame_offset,
                        note_total_gate as f64,
                        GateOffTarget::Sampler { gatepitch_id },
                    );
                }
            }
        }
    } else {
        // Single-note mode: use resolved transpose
        let transpose = resolved.transpose;
        if is_custom {
            let Some(engine_id) = engine_id else {
                return;
            };
            let allocation = if free_patch {
                let Some(allocation) = data.custom_engine_pools[engine_id]
                    .allocate_free_patch_voice(track_idx, track_idx, transpose)
                else {
                    return;
                };
                allocation
            } else {
                data.custom_engine_pools[engine_id].allocate_voice(
                    track_idx,
                    track_idx,
                    transpose,
                    track_polyphonic,
                    track_max_polyphony,
                )
            };
            let voice_idx = allocation.voice_idx;
            data.custom_engine_pools[engine_id].note_voice_allocated(engine_id, voice_idx);
            let lid = allocation.logical_id;
            let synth_id = data.state.runtime.engine_synth_node_ids[engine_id][voice_idx]
                .load(Ordering::Relaxed);
            let modulator_id = data.state.runtime.engine_modulator_node_ids[engine_id][voice_idx]
                .load(Ordering::Relaxed);
            if lid == 0 || synth_id == 0 || modulator_id == 0 {
                return;
            }
            if data.trace_audio {
                let enabled = data.custom_engine_pools[engine_id].enabled_voice_count;
                eprintln!(
                    "audio-trace: scheduled custom note-on track={track_idx} engine={engine_id} voice={voice_idx} lid={lid} synth={synth_id} mod={modulator_id} enabled_voices={enabled} poly={track_polyphonic} stolen={}",
                    allocation.stole_active_voice,
                );
                data.trace_render_probe_blocks = data.trace_render_probe_blocks.max(12);
            }
            let pitch_hz = custom_pitch_hz(transpose, base_note_offset);
            let key_locked_params = key_locked_live_instrument_params(
                &data.state,
                track_idx,
                transpose,
                base_note_offset,
                key_lock_plock_step,
                &instrument_params,
            );
            let note_fingerprint = instrument_param_bundle_fingerprint(
                engine_id,
                base_note_offset,
                &key_locked_params,
                &instrument_tensor_params,
            );
            cancel_gate_off_for_lid(&mut data.countdown_events, &mut data.block_events, lid);
            if allocation.stole_active_voice || !track_polyphonic || free_patch {
                let off_seq = next_event_sequence_from(&mut data.event_seq);
                unsafe {
                    send_custom_note_off(data.lg.0, lid, frame_offset, off_seq);
                    route_custom_voice_to_consumer(
                        data.lg.0,
                        &data.state,
                        engine_id,
                        voice_idx,
                        allocation.previous_route,
                        track_idx,
                    );
                    if data.custom_engine_pools[engine_id].voices[voice_idx].fingerprint
                        != note_fingerprint
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
                            &instrument_tensor_params,
                        );
                    }
                }
            } else {
                unsafe {
                    route_custom_voice_to_consumer(
                        data.lg.0,
                        &data.state,
                        engine_id,
                        voice_idx,
                        allocation.previous_route,
                        track_idx,
                    );
                    if data.custom_engine_pools[engine_id].voices[voice_idx].fingerprint
                        != note_fingerprint
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
                            &instrument_tensor_params,
                        );
                    }
                }
            }
            data.custom_engine_pools[engine_id].voices[voice_idx].fingerprint = note_fingerprint;
            let on_seq = next_event_sequence_from(&mut data.event_seq);
            unsafe {
                send_custom_trigger(data.lg.0, lid, frame_offset, on_seq, pitch_hz, velocity);
            }
            if gate_mode > 0.5 {
                schedule_gate_off_event(
                    data,
                    track_idx,
                    lid,
                    frame_offset,
                    total_gate as f64,
                    GateOffTarget::Custom {
                        engine_id,
                        free_patch,
                    },
                );
            }
        } else {
            let voice =
                data.voice_pools[track_idx].allocate_voice_retriggering_same_note(transpose);
            let voice_lid = voice.logical_id;
            let lid = if voice_lid != 0 {
                voice_lid
            } else {
                sampler_lid
            };
            let gatepitch_id = voice.gatepitch_id;
            if voice.modulator_id > 0 {
                let gatepitch_seq = next_event_sequence_from(&mut data.event_seq);
                unsafe {
                    dispatch_sampler_modulator_params_to_voice(
                        data.lg.0,
                        voice.modulator_id as u64,
                        &instrument_params,
                    );
                    send_custom_trigger(
                        data.lg.0,
                        voice.gatepitch_id as u64,
                        frame_offset,
                        gatepitch_seq,
                        custom_pitch_hz(transpose + base_note_offset, 0.0),
                        velocity,
                    );
                }
            }
            let sampler_seq = next_event_sequence_from(&mut data.event_seq);
            unsafe {
                dispatch_sampler_extra_params_to_voice(data.lg.0, lid, &instrument_params);
                send_trigger(
                    data.lg.0,
                    lid,
                    frame_offset,
                    sampler_seq,
                    velocity,
                    resolved.speed * playback_speed,
                    chop_gate,
                    attack_samples,
                    release_samples,
                    gate_mode,
                    transpose + base_note_offset,
                    start_point,
                    end_point,
                    instrument_enabled,
                    reverse,
                    loop_mode,
                    loop_xfade_samples,
                    sr_hz,
                    warp_enabled,
                    warp_mode,
                    warp_ratio,
                    warp_sample_bpm,
                    warp_project_bpm,
                    warp_ptr_lo,
                    warp_ptr_hi,
                    warp_preserve,
                    warp_seg_loop_mode,
                    warp_seg_envelope,
                    scrub,
                );
            }
            if gate_mode > 0.5 {
                schedule_gate_off_event(
                    data,
                    track_idx,
                    lid,
                    frame_offset,
                    total_gate as f64,
                    GateOffTarget::Sampler { gatepitch_id },
                );
            }
        }
    }

    // Update send gain (reverb send amount from track-level param)
    let send_lid = data.state.runtime.send_lids[track_idx].load(Ordering::Acquire);
    if send_lid != 0 {
        unsafe {
            params_push_wrapper(
                data.lg.0,
                ParamMsg {
                    idx: 0,
                    logical_id: send_lid,
                    fvalue: track_send,
                },
            );
        }
    }

    data.state.transport.trigger_flash[track_idx].store(255, Ordering::Relaxed);

    // Setup chop re-triggers (sampler only — custom instruments handle gate duration internally)
    if !is_custom && chop > 1 {
        schedule_chop_events(
            data,
            track_idx,
            frame_offset,
            samples_per_step / chop as f64,
            samples_per_step / chop as f64,
            chop - 1,
            step,
            chop_gate,
        );
    } else {
        cancel_chops_for_track(
            &mut data.countdown_events,
            &mut data.block_events,
            track_idx,
        );
    }
}

fn dispatch_chop_event(data: &mut AudioCallbackData, event: ChopEvent, frame_offset: u32) {
    let track_idx = event.track_idx;
    if track_idx >= data.state.active_track_count() {
        return;
    }
    if InstrumentType::from_runtime_flag(
        data.state.runtime.instrument_type_flags[track_idx].load(Ordering::Relaxed),
    ) == InstrumentType::Modulator
    {
        let lid = data.state.runtime.modulator_lids[track_idx].load(Ordering::Acquire);
        if lid == 0 {
            return;
        }
        let slot = &data.state.pattern.instrument_slots[track_idx];
        let rise = slot
            .plocks
            .get(event.step, 0)
            .unwrap_or_else(|| slot.defaults.get(0));
        let fall = slot
            .plocks
            .get(event.step, 1)
            .unwrap_or_else(|| slot.defaults.get(1));
        let velocity = data.state.pattern.step_data[track_idx].get(event.step, StepParam::Velocity);
        let seq = next_event_sequence_from(&mut data.event_seq);
        unsafe {
            params_push_wrapper(
                data.lg.0,
                ParamMsg {
                    idx: crate::track_modulator::PARAM_RISE_MS,
                    logical_id: lid,
                    fvalue: rise,
                },
            );
            params_push_wrapper(
                data.lg.0,
                ParamMsg {
                    idx: crate::track_modulator::PARAM_FALL_MS,
                    logical_id: lid,
                    fvalue: fall,
                },
            );
            trigger_modulator_pulse(data.lg.0, lid, frame_offset, seq, event.chop_gate, velocity);
        }
        data.state.transport.trigger_flash[track_idx].store(255, Ordering::Relaxed);
        return;
    }

    let tp = &data.state.pattern.track_params[track_idx];
    let gate_mode = if tp.is_gate_on() { 1.0 } else { 0.0 };
    let chop_inst_slot = &data.state.pattern.instrument_slots[track_idx];
    let attack_samples = chop_inst_slot
        .plocks
        .get(event.step, 0)
        .unwrap_or_else(|| chop_inst_slot.defaults.get(0))
        * data.sample_rate as f32
        / 1000.0;
    let release_samples = chop_inst_slot
        .plocks
        .get(event.step, 1)
        .unwrap_or_else(|| chop_inst_slot.defaults.get(1))
        * data.sample_rate as f32
        / 1000.0;
    let chop_start = chop_inst_slot
        .plocks
        .get(event.step, 2)
        .unwrap_or_else(|| chop_inst_slot.defaults.get(2));
    let chop_end = chop_inst_slot
        .plocks
        .get(event.step, 3)
        .unwrap_or_else(|| chop_inst_slot.defaults.get(3));
    let chop_reverse = chop_inst_slot
        .plocks
        .get(event.step, 5)
        .unwrap_or_else(|| chop_inst_slot.defaults.get(5));
    let chop_loop_mode = chop_inst_slot
        .plocks
        .get(event.step, 6)
        .unwrap_or_else(|| chop_inst_slot.defaults.get(6));
    let chop_loop_xfade_samples = chop_inst_slot
        .plocks
        .get(event.step, 7)
        .unwrap_or_else(|| chop_inst_slot.defaults.get(7))
        * data.sample_rate as f32
        / 1000.0;
    let chop_sr_hz = chop_inst_slot
        .plocks
        .get(event.step, 8)
        .unwrap_or_else(|| chop_inst_slot.defaults.get(8));
    let chop_warp_enabled = chop_inst_slot
        .plocks
        .get(event.step, 9)
        .unwrap_or_else(|| chop_inst_slot.defaults.get(9));
    let chop_warp_mode = chop_inst_slot
        .plocks
        .get(event.step, 10)
        .unwrap_or_else(|| chop_inst_slot.defaults.get(10));
    let chop_sample_bpm = chop_inst_slot
        .plocks
        .get(event.step, 11)
        .unwrap_or_else(|| chop_inst_slot.defaults.get(11));
    let chop_playback_speed = chop_inst_slot
        .plocks
        .get(event.step, 12)
        .unwrap_or_else(|| chop_inst_slot.defaults.get(12));
    let chop_scrub = chop_inst_slot
        .plocks
        .get(event.step, 13)
        .unwrap_or_else(|| chop_inst_slot.defaults.get(13));
    let chop_warp_preserve = live_slot_resolved_node_param_value(
        chop_inst_slot,
        event.step,
        crate::sampler::PARAM_WARP_PRESERVE,
        crate::sampler::WARP_PRESERVE_DEFAULT as f32,
    );
    let chop_warp_seg_loop_mode = live_slot_resolved_node_param_value(
        chop_inst_slot,
        event.step,
        crate::sampler::PARAM_WARP_SEG_LOOP_MODE,
        crate::sampler::WARP_SEG_LOOP_MODE_DEFAULT as f32,
    );
    let chop_warp_seg_envelope = live_slot_resolved_node_param_value(
        chop_inst_slot,
        event.step,
        crate::sampler::PARAM_WARP_SEG_ENVELOPE,
        crate::sampler::WARP_SEG_ENVELOPE_DEFAULT,
    );
    let (
        chop_warp_enabled,
        chop_warp_mode,
        chop_warp_ratio,
        chop_warp_sample_bpm,
        chop_warp_project_bpm,
        chop_warp_ptr_lo,
        chop_warp_ptr_hi,
    ) = sampler_warp_runtime(
        &data.state,
        track_idx,
        chop_warp_enabled,
        chop_warp_mode,
        chop_sample_bpm,
    );
    let chop_base_note_offset = f32::from_bits(
        data.state.pattern.instrument_base_note_offsets[track_idx].load(Ordering::Relaxed),
    );
    let sd = &data.state.pattern.step_data[track_idx];
    let transpose = sd.get(event.step, StepParam::Transpose);
    let voice = data.voice_pools[track_idx].allocate_voice_retriggering_same_note(transpose);
    let voice_lid = voice.logical_id;
    let sampler_lid = data.state.runtime.sampler_lids[track_idx].load(Ordering::Acquire);
    let lid = if voice_lid != 0 {
        voice_lid
    } else {
        sampler_lid
    };
    if lid == 0 {
        return;
    }
    if voice.modulator_id > 0 {
        let gatepitch_seq = next_event_sequence_from(&mut data.event_seq);
        unsafe {
            dispatch_sampler_modulator_defaults_to_voice(
                data.lg.0,
                &data.state,
                track_idx,
                voice.modulator_id as u64,
            );
            send_custom_trigger(
                data.lg.0,
                voice.gatepitch_id as u64,
                frame_offset,
                gatepitch_seq,
                custom_pitch_hz(transpose + chop_base_note_offset, 0.0),
                sd.get(event.step, StepParam::Velocity),
            );
        }
    }
    let sampler_seq = next_event_sequence_from(&mut data.event_seq);
    unsafe {
        dispatch_sampler_extra_defaults_to_voice(data.lg.0, &data.state, track_idx, lid);
        send_trigger(
            data.lg.0,
            lid,
            frame_offset,
            sampler_seq,
            sd.get(event.step, StepParam::Velocity),
            sd.get(event.step, StepParam::Speed) * chop_playback_speed,
            event.chop_gate,
            attack_samples,
            release_samples,
            gate_mode,
            transpose + chop_base_note_offset,
            chop_start,
            chop_end,
            chop_inst_slot
                .plocks
                .get(event.step, 4)
                .unwrap_or_else(|| chop_inst_slot.defaults.get(4)),
            chop_reverse,
            chop_loop_mode,
            chop_loop_xfade_samples,
            chop_sr_hz,
            chop_warp_enabled,
            chop_warp_mode,
            chop_warp_ratio,
            chop_warp_sample_bpm,
            chop_warp_project_bpm,
            chop_warp_ptr_lo,
            chop_warp_ptr_hi,
            chop_warp_preserve,
            chop_warp_seg_loop_mode,
            chop_warp_seg_envelope,
            chop_scrub,
        );
    }
    data.state.transport.trigger_flash[track_idx].store(255, Ordering::Relaxed);
}

fn audio_callback(data: &mut AudioCallbackData, output: &mut [f32]) {
    let callback_start = Instant::now();
    let nframes = output.len() / data.num_channels;
    data.current_callback_nframes = nframes;
    data.trace_callback_counter = data.trace_callback_counter.wrapping_add(1);
    let num_tracks = data.state.active_track_count();
    let topology_epoch = data.state.transport.topology_epoch.load(Ordering::Relaxed);
    if num_tracks != data.last_num_tracks || topology_epoch != data.last_topology_epoch {
        if data.trace_audio {
            eprintln!(
                "audio-trace: topology reset tracks {}->{} epoch {}->{} rendered_samples={}",
                data.last_num_tracks,
                num_tracks,
                data.last_topology_epoch,
                topology_epoch,
                data.rendered_samples.load(Ordering::Acquire),
            );
            data.trace_render_probe_blocks = data.trace_render_probe_blocks.max(12);
        }
        reset_audio_runtime_for_track_topology(data, num_tracks);
    }
    if data.state.topology_edit_in_flight() {
        data.scheduled_events.clear();
        clear_countdown_events(data);
        data.event_seq = 0;
    }
    let scheduler_snapshot_version = data.state.scheduler_snapshot_version();
    if scheduler_snapshot_version != data.scheduler_snapshot_version {
        data.scheduler_snapshot = data.state.latest_scheduler_snapshot();
        data.scheduler_snapshot_version = scheduler_snapshot_version;
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
    sync_bus_gate_params(data, block_start_sample);
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
                release_active_keyboard_note(data, active_note, 0, block_end_sample);
            }
        } else {
            // Note-on: allocate voice and trigger
            enforce_mute_group_for_winning_track(data, kt.track, block_start_sample, 0);
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
                let voice = data.voice_pools[kt.track]
                    .allocate_voice_retriggering_same_note(resolved_transpose);
                let voice_lid = voice.logical_id;
                if voice_lid == 0 {
                    continue;
                }
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
                let kb_instrument_params =
                    resolve_snapshot_instrument_defaults(&data.scheduler_snapshot, kt.track);
                let attack_samples = kb_default(0) * data.sample_rate as f32 / 1000.0;
                let release_samples = kb_default(1) * data.sample_rate as f32 / 1000.0;
                let gate_mode = if tp.is_gate_on() { 1.0 } else { 0.0 };
                let kb_start = kb_default(2);
                let kb_end = kb_default(3);
                let kb_enabled = kb_default(4);
                let kb_reverse = kb_default(5);
                let kb_loop_mode = kb_default(6);
                let kb_loop_xfade_samples = kb_default(7) * data.sample_rate as f32 / 1000.0;
                let kb_sr_hz = kb_default(8);
                let kb_playback_speed = kb_default(12);
                let kb_warp_preserve = snapshot_slot_default_node_param_value(
                    kb_inst_slot,
                    crate::sampler::PARAM_WARP_PRESERVE,
                    crate::sampler::WARP_PRESERVE_DEFAULT as f32,
                );
                let kb_warp_seg_loop_mode = snapshot_slot_default_node_param_value(
                    kb_inst_slot,
                    crate::sampler::PARAM_WARP_SEG_LOOP_MODE,
                    crate::sampler::WARP_SEG_LOOP_MODE_DEFAULT as f32,
                );
                let kb_warp_seg_envelope = snapshot_slot_default_node_param_value(
                    kb_inst_slot,
                    crate::sampler::PARAM_WARP_SEG_ENVELOPE,
                    crate::sampler::WARP_SEG_ENVELOPE_DEFAULT,
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
                            custom_pitch_hz(resolved_transpose + base_note_offset, 0.0),
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
                        resolved_transpose + base_note_offset,
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
                    &[ActiveKeyboardVoice {
                        logical_id: voice_lid,
                        gatepitch_id: voice.gatepitch_id,
                        target: ActiveKeyboardVoiceTarget::Sampler { pool_id: kt.track },
                    }],
                );
            }
            data.state.transport.trigger_flash[kt.track].store(255, Ordering::Relaxed);
        }
    }
    for track in 0..num_tracks {
        data.state.replace_live_notes(
            track,
            data.active_keyboard_notes[track]
                .iter()
                .filter_map(|note| note.and_then(|note| note.midi_note)),
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
                                idx: crate::voice_modulator::PARAM_RESET_COUNTER as u64,
                                logical_id: logical_id as u64,
                                fvalue: mod_reset_counter as f32,
                            },
                        );
                    }
                }
            }
        }
    }

    let current_pattern_epoch = data.state.transport.pattern_epoch.load(Ordering::Relaxed);
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
            let mod_stats = crate::voice_modulator::take_process_stats();
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

/// Build a cpal output stream that drives the audiograph.
pub fn build_output_stream(
    lg: *mut LiveGraph,
    state: Arc<SequencerState>,
    sample_rate: u32,
    num_channels: usize,
    block_size: usize,
    master_recorder: Arc<MasterRecorder>,
    keyboard_rx: std::sync::mpsc::Receiver<KeyboardTrigger>,
    bus_gate_runtime: Arc<Mutex<Vec<BusGateRuntimeState>>>,
    bus_gate_playheads: Arc<Mutex<Vec<(BusId, usize)>>>,
) -> Result<Stream, String> {
    // CPAL does not expose portable output latency. Use the configured output
    // block as the sensible default; users can tune this transport value when
    // their device/OS path has additional latency.
    state.transport.record_latency_seconds.store(
        (block_size as f32 / sample_rate.max(1) as f32).to_bits(),
        Ordering::Release,
    );
    // Initialize voice pools from state
    let mut voice_pools: Vec<VoicePool> =
        (0..MAX_SAMPLER_POOLS).map(|_| VoicePool::new()).collect();
    let mut custom_engine_pools: Vec<CustomEnginePool> = (0..MAX_INSTRUMENT_ENGINES)
        .map(|_| CustomEnginePool::new())
        .collect();

    // Pre-populate voice pools for any existing tracks
    let num_tracks = state.active_track_count();
    for t in 0..num_tracks {
        sync_sampler_voice_pool(&state, t, &mut voice_pools[t]);

        if let Some(engine_id) = track_engine_id(&state, t) {
            sync_custom_engine_pool(&state, engine_id, &mut custom_engine_pools[engine_id]);
        }
    }

    let scheduled_events = Arc::new(ScheduledEventQueue::new());
    let rendered_samples = Arc::new(AtomicU64::new(0));
    let (audio_keyboard_tx, audio_keyboard_rx) = std::sync::mpsc::channel();
    let (live_keyboard_tx, live_keyboard_rx) = std::sync::mpsc::channel();
    {
        let state_for_keyboard_router = Arc::clone(&state);
        let _ = std::thread::Builder::new()
            .name("keyboard-midi-fx-router".to_string())
            .spawn(move || {
                while let Ok(trigger) = keyboard_rx.recv() {
                    if trigger.note_off {
                        let _ = live_keyboard_tx.send(trigger);
                        let _ = audio_keyboard_tx.send(trigger);
                        continue;
                    }
                    let use_midi_fx = trigger.track
                        < state_for_keyboard_router.active_track_count()
                        && !state_for_keyboard_router.pattern.track_params[trigger.track]
                            .midi_fx_chain()
                            .is_empty();
                    if use_midi_fx {
                        let _ = live_keyboard_tx.send(trigger);
                    } else {
                        let _ = audio_keyboard_tx.send(trigger);
                    }
                }
            });
    }
    let initial_topology_epoch = state.transport.topology_epoch.load(Ordering::Relaxed);
    let initial_scheduler_snapshot_version = state.scheduler_snapshot_version();
    let initial_scheduler_snapshot = state.latest_scheduler_snapshot();
    let trace_audio = env_flag("TINYSEQ_AUDIO_TRACE", false);
    crate::voice_modulator::set_process_stats_enabled(trace_audio);
    if trace_audio {
        eprintln!("audio-trace: enabled");
    }

    let mut cb_data = AudioCallbackData {
        lg: LiveGraphPtr(lg),
        state,
        num_channels,
        sample_rate: sample_rate as f64,
        last_bpm: 0,
        last_mod_reset_counter: 0,
        voice_pools,
        custom_engine_pools,
        scheduler_snapshot: initial_scheduler_snapshot,
        scheduler_snapshot_version: initial_scheduler_snapshot_version,
        active_keyboard_notes: [[None; MAX_VOICES]; MAX_TRACKS],
        keyboard_rx: audio_keyboard_rx,
        master_recorder,
        accumulator_states: [crate::accumulator::AccumulatorRuntimeState::default(); MAX_TRACKS],
        last_playing: false,
        last_pattern: u32::MAX,
        last_num_tracks: num_tracks,
        last_topology_epoch: initial_topology_epoch,
        host_clock_was_playing: false,
        host_clock_play_start_sample: 0,
        free_patch_transport_routes: [FreePatchTransportRouteState::default(); MAX_TRACKS],
        pending_accum_reset: [false; MAX_TRACKS],
        scheduled_events: Arc::clone(&scheduled_events),
        countdown_events: Vec::with_capacity(SCHEDULED_COUNTDOWN_CAPACITY),
        block_events: Vec::with_capacity(SCHEDULED_BLOCK_SCRATCH_CAPACITY),
        block_events_need_sort: false,
        current_callback_nframes: block_size,
        rendered_samples: Arc::clone(&rendered_samples),
        bus_gate_runtime,
        bus_gate_playheads,
        bus_gate_clocks: Vec::new(),
        bus_gate_was_playing: false,
        bus_gate_play_start_sample: 0,
        dropped_scheduled_events: 0,
        late_scheduled_events: 0,
        event_seq: 0,
        trace_audio,
        trace_callback_counter: 0,
        trace_render_probe_blocks: 0,
        trace_silent_active_callbacks: 0,
        transport_beats: 0.0,
        transport_was_playing: false,
        metronome: MetronomeState::default(),
    };
    crate::scheduler::spawn_scheduler_thread(
        Arc::clone(&cb_data.state),
        sample_rate,
        block_size,
        rendered_samples,
        scheduled_events,
        live_keyboard_rx,
    );

    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or("No output device available")?;

    let config = cpal::StreamConfig {
        channels: num_channels as u16,
        sample_rate: cpal::SampleRate(sample_rate),
        buffer_size: cpal::BufferSize::Fixed(block_size as u32),
    };

    let stream = device
        .build_output_stream(
            &config,
            move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
                audio_callback(&mut cb_data, data);
            },
            |err| eprintln!("Audio stream error: {err}"),
            None,
        )
        .map_err(|e| format!("Failed to build output stream: {e}"))?;

    stream
        .play()
        .map_err(|e| format!("Failed to play stream: {e}"))?;

    Ok(stream)
}

/// Query the default output device, preserving the system sample rate when possible.
pub fn query_device_config() -> Result<(u32, u16), String> {
    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or("No output device available")?;
    let default_config = device
        .default_output_config()
        .map_err(|e| format!("Failed to get default config: {e}"))?;
    let ranges: Vec<OutputFormatRange> = device
        .supported_output_configs()
        .map_err(|e| format!("Failed to query supported output configs: {e}"))?
        .map(|range| OutputFormatRange {
            channels: range.channels(),
            min_sample_rate: range.min_sample_rate().0,
            max_sample_rate: range.max_sample_rate().0,
            supports_f32: range.sample_format() == cpal::SampleFormat::F32,
        })
        .collect();
    let selected = select_output_config(
        default_config.sample_rate().0,
        default_config.channels(),
        ranges,
    )
    .ok_or_else(|| {
        let device_name = device
            .name()
            .unwrap_or_else(|_| "default output device".to_string());
        format!(
            "{device_name} does not support f32 output at either {} Hz or its default {} Hz rate",
            FALLBACK_SAMPLE_RATE,
            default_config.sample_rate().0
        )
    })?;

    Ok((selected.sample_rate, selected.channels))
}

#[cfg(test)]
mod tests;
