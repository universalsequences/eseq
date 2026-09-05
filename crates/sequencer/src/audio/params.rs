/*!
Parameter resolution: from sequencer state to concrete per-voice values.

Answers "what value does this parameter have for this trigger?" across every
source: p-locks (per-step), key locks (per-note), slot defaults, and live
overrides — in paired `live_*` (current `SequencerState`) and `snapshot_*`
(scheduler snapshot) variants that must stay in sync. Builds the
`ScheduledInstrumentParams`/`ScheduledSamplerParams` bundles consumed by the
fire path, computes sound fingerprints used for voice-reuse decisions, and
resolves rack-slot param sets.
*/

#[allow(unused_imports)]
use super::*;

/// Hz handed to a custom (DGenLisp) instrument's `pitch` input.
///
/// Transpose 0 is MIDI 60 (C4 = 261.63 Hz) everywhere else in the app: the
/// piano-roll labels, the MIDI-keyboard seam (`note - 60`), the sampler root,
/// and the audition/probe path. This must agree with them; the original
/// `440 * 2^(t/12)` form played A4 at transpose 0 and left every dgen synth
/// nine semitones sharp against its note name.
pub(super) fn custom_pitch_hz(transpose: f32, base_note_offset: f32) -> f32 {
    440.0 * 2f32.powf((transpose + base_note_offset - 9.0) / 12.0)
}

pub(super) fn custom_pitch_midi_note(transpose: f32, base_note_offset: f32) -> u8 {
    (transpose + base_note_offset + 60.0)
        .round()
        .clamp(0.0, 127.0) as u8
}

pub(super) fn track_accepts_scheduled_trigger(state: &SequencerState, track_idx: usize) -> bool {
    // Mute and solo are post-FX gain controls. Internal triggers must keep
    // advancing voices, envelopes, accumulators, and choke arbitration while
    // their output is inaudible; only an invalid track is rejected here.
    track_idx < state.active_track_count()
}

pub(super) fn resolve_live_keyboard_transpose(
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

pub(super) fn track_engine_id(state: &SequencerState, track_idx: usize) -> Option<usize> {
    let engine_id = state.runtime.track_engine_ids[track_idx].load(Ordering::Relaxed);
    if engine_id == u32::MAX {
        None
    } else {
        Some(engine_id as usize)
    }
}

pub(super) fn track_custom_run_mode(state: &SequencerState, track_idx: usize) -> CustomInstrumentRunMode {
    CustomInstrumentRunMode::from_runtime_flag(
        state.runtime.instrument_run_mode_flags[track_idx].load(Ordering::Relaxed),
    )
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum SliceTriggerVerdict {
    Fire,
    Ignore,
}

/// Resolve one note selector into the transient slice bounds for a sampler
/// runtime pool. This performs no allocation and is shared by sequenced,
/// live-keyboard, and rack-member trigger paths.
pub(super) fn resolve_slice(
    state: &SequencerState,
    pool_idx: usize,
    params: &mut ScheduledSamplerParams,
    transpose: &mut f32,
) -> SliceTriggerVerdict {
    let mode = params.slice_mode.round();
    if mode == 0.0 {
        return SliceTriggerVerdict::Fire;
    }
    if mode != 1.0 {
        return SliceTriggerVerdict::Ignore;
    }
    let Some(status) = state.runtime.sampler_analysis_status.get(pool_idx) else {
        return SliceTriggerVerdict::Ignore;
    };
    if status.load(Ordering::Acquire) != 2 {
        return SliceTriggerVerdict::Ignore;
    }
    let ptr = crate::analysis::unpack_ptr(
        f32::from_bits(state.runtime.sampler_onset_ptr_lo[pool_idx].load(Ordering::Acquire)),
        f32::from_bits(state.runtime.sampler_onset_ptr_hi[pool_idx].load(Ordering::Acquire)),
    );
    if ptr.is_null() {
        return SliceTriggerVerdict::Ignore;
    }
    // The analysis cache keeps every published table alive for the app
    // lifetime (replaced tables are retired, never freed), so this pointer
    // stays valid even if the buffer id is recycled and re-analyzed.
    let table = unsafe { &*ptr };
    let selector = transpose.round() as i32 - params.slice_base.round() as i32;
    if selector < 0 {
        return SliceTriggerVerdict::Ignore;
    }
    let selector = selector as usize;
    let bounds = {
        let mut starts = table.slice_starts(params.slice_sensitivity);
        starts.nth(selector).map(|start| {
            let end = starts.next().unwrap_or(table.sample_len_frames);
            (start, end)
        })
    };
    let Some((start_frame, end_frame)) = bounds else {
        return SliceTriggerVerdict::Ignore;
    };
    let sample_len = table.sample_len_frames.max(1) as f32;
    if !params.start_point_locked {
        params.start_point = start_frame as f32 / sample_len;
    }
    if !params.end_point_locked {
        params.end_point = end_frame as f32 / sample_len;
    }
    if crate::instruments::sampler::srange_debug_enabled() {
        eprintln!(
            "[srange] slice note={} idx={} start={} end={}",
            *transpose, selector, params.start_point, params.end_point,
        );
    }
    *transpose = 0.0;
    SliceTriggerVerdict::Fire
}

pub(super) fn sampler_warp_runtime(
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

pub(super) fn instrument_sound_fingerprint(
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

pub(super) fn slot_param_identity(
    node_id: u32,
    modulator_node_id: u32,
    raw_idx: u32,
) -> Option<crate::neural::ParamNodeId> {
    if raw_idx == u32::MAX {
        return None;
    }
    if raw_idx >= crate::instruments::voice_modulator::MOD_PARAM_BASE {
        if modulator_node_id == 0 {
            return None;
        }
        Some(crate::neural::ParamNodeId {
            logical_id: modulator_node_id as u64,
            node_param_idx: raw_idx - crate::instruments::voice_modulator::MOD_PARAM_BASE,
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

pub(super) fn plock_identity_matches(
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

pub(super) fn resolved_slot_param_value(
    slot: &EffectSlotSnapshot,
    step_idx: usize,
    param_idx: usize,
    default: f32,
) -> f32 {
    slot.resolved_param_value(step_idx, param_idx, default)
}

fn resolved_sampler_host_param_value(
    slot: &EffectSlotSnapshot,
    step_idx: usize,
    param_idx: usize,
    default: f32,
) -> f32 {
    slot.plocks
        .get(step_idx)
        .and_then(|row| row.get(param_idx))
        .copied()
        .flatten()
        .unwrap_or_else(|| slot.defaults.get(param_idx).copied().unwrap_or(default))
}

pub(super) fn slot_has_explicit_plock(
    slot: &EffectSlotSnapshot,
    step_idx: usize,
    param_idx: usize,
) -> bool {
    let Some(raw_idx) = slot.node_param_idx(param_idx) else {
        return false;
    };
    plock_identity_matches(
        &slot.plock_param_ids,
        step_idx,
        param_idx,
        crate::neural::ParamNodeId::from_slot_param(slot.node_id, slot.modulator_node_id, raw_idx),
    )
}

pub(super) fn snapshot_slot_param_index_by_node_idx(
    slot: &EffectSlotSnapshot,
    node_param_idx: u32,
) -> Option<usize> {
    let num_params = slot.num_params as usize;
    (0..num_params).find(|&param_idx| slot.node_param_idx(param_idx) == Some(node_param_idx))
}

pub(super) fn resolved_slot_node_param_value(
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

pub(super) fn default_slot_node_param_value(
    slot: &EffectSlotSnapshot,
    node_param_idx: u32,
    default: f32,
) -> f32 {
    let Some(param_idx) = snapshot_slot_param_index_by_node_idx(slot, node_param_idx) else {
        return default;
    };
    slot.defaults.get(param_idx).copied().unwrap_or(default)
}

pub(super) fn live_slot_param_index_by_node_idx(slot: &EffectSlotState, node_param_idx: u64) -> Option<usize> {
    let num_params = slot.num_params.load(Ordering::Relaxed) as usize;
    (0..num_params).find(|&param_idx| slot.resolve_node_idx(param_idx) == node_param_idx)
}

pub(super) fn live_slot_resolved_param_value(
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

pub(super) fn live_slot_resolved_node_param_value(
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

pub(super) fn live_slot_default_node_param_value(
    slot: &EffectSlotState,
    node_param_idx: u64,
    default: f32,
) -> f32 {
    let Some(param_idx) = live_slot_param_index_by_node_idx(slot, node_param_idx) else {
        return default;
    };
    slot.defaults.get(param_idx)
}

pub(super) fn snapshot_slot_default_node_param_value(
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

pub(super) fn key_lock_identity_matches(
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

pub(super) fn live_param_route(
    slot: &EffectSlotState,
    param_idx: usize,
) -> Option<(ScheduledInstrumentParamTarget, u64, u32)> {
    let raw_idx = slot.resolve_node_idx(param_idx);
    if raw_idx == u32::MAX as u64 {
        return None;
    }
    let span = slot.resolve_node_span(param_idx);
    if raw_idx >= crate::instruments::voice_modulator::MOD_PARAM_BASE as u64 {
        Some((
            ScheduledInstrumentParamTarget::Modulator,
            raw_idx - crate::instruments::voice_modulator::MOD_PARAM_BASE as u64,
            span,
        ))
    } else {
        Some((ScheduledInstrumentParamTarget::Synth, raw_idx, span))
    }
}

pub(super) fn snapshot_param_route(
    slot: &EffectSlotSnapshot,
    param_idx: usize,
) -> Option<(ScheduledInstrumentParamTarget, u64, u32)> {
    let raw_idx = slot.node_param_idx(param_idx)?;
    if raw_idx == u32::MAX {
        return None;
    }
    let span = slot
        .param_node_spans
        .get(param_idx)
        .copied()
        .unwrap_or(1)
        .max(1);
    if raw_idx >= crate::instruments::voice_modulator::MOD_PARAM_BASE {
        Some((
            ScheduledInstrumentParamTarget::Modulator,
            (raw_idx - crate::instruments::voice_modulator::MOD_PARAM_BASE) as u64,
            span,
        ))
    } else {
        Some((ScheduledInstrumentParamTarget::Synth, raw_idx as u64, span))
    }
}

pub(super) fn live_step_has_valid_plock(
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

pub(super) fn snapshot_step_has_valid_plock(
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
    let Some(raw_idx) = slot.node_param_idx(param_idx) else {
        return false;
    };
    let expected_id = slot_param_identity(slot.node_id, slot.modulator_node_id, raw_idx);
    plock_identity_matches(&slot.plock_param_ids, step_idx, param_idx, expected_id)
}

pub(super) fn upsert_instrument_param(
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

pub(super) fn key_locked_live_instrument_params(
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

pub(super) fn key_locked_snapshot_instrument_params(
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
        let Some(raw_idx) = slot.node_param_idx(param_idx) else {
            continue;
        };
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

pub(super) fn resolve_live_instrument_defaults(
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

pub(super) fn resolve_snapshot_instrument_defaults(
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

pub(super) fn resolve_live_instrument_tensor_defaults(
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

pub(super) fn instrument_param_bundle_fingerprint(
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

pub(super) fn resolve_rack_slot_instrument_params(
    slot: &EffectSlotSnapshot,
    step_idx: usize,
) -> ScheduledInstrumentParams {
    let num_params = slot.num_params as usize;
    let mut params = ScheduledInstrumentParams::new();
    for param_idx in 0..num_params {
        let Some(raw_idx) = slot.node_param_idx(param_idx) else {
            continue;
        };
        if raw_idx == u32::MAX {
            continue;
        }
        let span = slot
            .param_node_spans
            .get(param_idx)
            .copied()
            .unwrap_or(1)
            .max(1);
        let (target, idx) = if raw_idx >= crate::instruments::voice_modulator::MOD_PARAM_BASE {
            (
                ScheduledInstrumentParamTarget::Modulator,
                (raw_idx - crate::instruments::voice_modulator::MOD_PARAM_BASE) as u64,
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

pub(super) fn resolve_rack_slot_instrument_defaults(slot: &EffectSlotSnapshot) -> ScheduledInstrumentParams {
    let num_params = slot.num_params as usize;
    let mut params = ScheduledInstrumentParams::new();
    for param_idx in 0..num_params {
        let Some(raw_idx) = slot.node_param_idx(param_idx) else {
            continue;
        };
        if raw_idx == u32::MAX {
            continue;
        }
        let span = slot
            .param_node_spans
            .get(param_idx)
            .copied()
            .unwrap_or(1)
            .max(1);
        let (target, idx) = if raw_idx >= crate::instruments::voice_modulator::MOD_PARAM_BASE {
            (
                ScheduledInstrumentParamTarget::Modulator,
                (raw_idx - crate::instruments::voice_modulator::MOD_PARAM_BASE) as u64,
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

pub(super) fn resolve_rack_slot_sampler_params(
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
        slice_mode: resolved_sampler_host_param_value(
            slot, step_idx, crate::instruments::sampler::SLOT_PARAM_SLICE_MODE, 0.0,
        ),
        slice_sensitivity: resolved_sampler_host_param_value(
            slot, step_idx, crate::instruments::sampler::SLOT_PARAM_SLICE_SENSITIVITY, 0.5,
        ),
        slice_base: resolved_sampler_host_param_value(
            slot, step_idx, crate::instruments::sampler::SLOT_PARAM_SLICE_BASE, 0.0,
        ),
        start_point_locked: slot_has_explicit_plock(slot, step_idx, 2),
        end_point_locked: slot_has_explicit_plock(slot, step_idx, 3),
        warp_preserve: resolved_slot_node_param_value(
            slot,
            step_idx,
            crate::instruments::sampler::PARAM_WARP_PRESERVE as u32,
            crate::instruments::sampler::WARP_PRESERVE_DEFAULT as f32,
        ),
        warp_seg_loop_mode: resolved_slot_node_param_value(
            slot,
            step_idx,
            crate::instruments::sampler::PARAM_WARP_SEG_LOOP_MODE as u32,
            crate::instruments::sampler::WARP_SEG_LOOP_MODE_DEFAULT as f32,
        ),
        warp_seg_envelope: resolved_slot_node_param_value(
            slot,
            step_idx,
            crate::instruments::sampler::PARAM_WARP_SEG_ENVELOPE as u32,
            crate::instruments::sampler::WARP_SEG_ENVELOPE_DEFAULT,
        ),
    }
}

pub(super) fn resolve_rack_slot_sampler_defaults(slot: &EffectSlotSnapshot) -> ScheduledSamplerParams {
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
        slice_mode: value(crate::instruments::sampler::SLOT_PARAM_SLICE_MODE, 0.0),
        slice_sensitivity: value(
            crate::instruments::sampler::SLOT_PARAM_SLICE_SENSITIVITY, 0.5,
        ),
        slice_base: value(crate::instruments::sampler::SLOT_PARAM_SLICE_BASE, 0.0),
        start_point_locked: false,
        end_point_locked: false,
        warp_preserve: default_slot_node_param_value(
            slot,
            crate::instruments::sampler::PARAM_WARP_PRESERVE as u32,
            crate::instruments::sampler::WARP_PRESERVE_DEFAULT as f32,
        ),
        warp_seg_loop_mode: default_slot_node_param_value(
            slot,
            crate::instruments::sampler::PARAM_WARP_SEG_LOOP_MODE as u32,
            crate::instruments::sampler::WARP_SEG_LOOP_MODE_DEFAULT as f32,
        ),
        warp_seg_envelope: default_slot_node_param_value(
            slot,
            crate::instruments::sampler::PARAM_WARP_SEG_ENVELOPE as u32,
            crate::instruments::sampler::WARP_SEG_ENVELOPE_DEFAULT,
        ),
    }
}

pub(super) fn rack_slot_sound_fingerprint(
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

#[cfg(test)]
mod pitch_tests {
    use super::{custom_pitch_hz, custom_pitch_midi_note};

    #[test]
    fn transpose_zero_is_middle_c_for_custom_instruments() {
        assert!((custom_pitch_hz(0.0, 0.0) - 261.6256).abs() < 0.01);
        assert!((custom_pitch_hz(9.0, 0.0) - 440.0).abs() < 0.01);
        assert!((custom_pitch_hz(0.0, 9.0) - 440.0).abs() < 0.01);
        assert!((custom_pitch_hz(-12.0, 0.0) - 130.8128).abs() < 0.01);
    }

    #[test]
    fn custom_hz_and_midi_note_agree_on_the_same_transpose() {
        for transpose in [-24.0f32, -9.0, 0.0, 7.0, 12.0, 31.0] {
            let note = custom_pitch_midi_note(transpose, 0.0) as f32;
            let expected = 440.0 * 2f32.powf((note - 69.0) / 12.0);
            assert!((custom_pitch_hz(transpose, 0.0) - expected).abs() < 0.01);
        }
    }
}
