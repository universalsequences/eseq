/*!
Snapshot-side parameter, plock, default, sampler, and MIDI-FX clock resolution.
*/

#[allow(unused_imports)]
use super::*;

pub(super) fn scheduled_instrument_params_from_vec(
    params: Vec<ScheduledInstrumentParam>,
) -> ScheduledInstrumentParams {
    params.into_iter().collect::<ScheduledInstrumentParams>()
}

pub(super) fn scheduled_instrument_tensor_params_from_vec(
    params: Vec<ScheduledInstrumentTensorParam>,
) -> ScheduledInstrumentTensorParams {
    params
        .into_iter()
        .collect::<ScheduledInstrumentTensorParams>()
}

pub(super) fn resolve_track_send_params(
    snapshot: &SequencerSnapshot,
    track_idx: usize,
    step_idx: usize,
) -> Vec<ScheduledEffectParam> {
    let Some(track) = snapshot.tracks.get(track_idx) else {
        return Vec::new();
    };
    let step_locks = track.steps.get(step_idx)
        .map(|step| step.track_send_plocks.as_slice())
        .unwrap_or_default();
    let mut params = Vec::with_capacity(track.track_send_runtime_targets.len() * 2);
    for target in &track.track_send_runtime_targets {
        let snapshot_baseline = track.params.sends.iter()
            .find(|send| send.destination == target.destination)
            .map(|send| send.amount)
            .unwrap_or(0.0);
        let step_lock = step_locks.iter()
            .find(|send| send.destination == target.destination)
            .map(|send| send.amount.clamp(0.0, 1.0));
        let live_baseline = if step_lock.is_none() {
            track.track_send_live_baselines.iter()
                .find(|(destination, _)| *destination == target.destination)
                .map(|(_, baseline)| Arc::clone(baseline))
        } else {
            None
        };
        let value = step_lock.unwrap_or_else(|| {
            live_baseline
                .as_ref()
                .map(|baseline| baseline.load())
                .unwrap_or(snapshot_baseline)
        });
        let live_value = live_baseline.map(LiveScheduledEffectValue::new);
        params.push(ScheduledEffectParam {
            logical_id: target.left_id,
            idx: 0,
            value,
            live_value: live_value.clone(),
        });
        params.push(ScheduledEffectParam {
            logical_id: target.right_id,
            idx: 0,
            value,
            live_value,
        });
    }
    params
}

pub(super) fn debug_routing_enabled() -> bool {
    std::env::var_os("TINYSEQ_DEBUG_ROUTING").is_some()
}

pub(super) fn event_source_label(source: &EventSource) -> &'static str {
    match source {
        EventSource::Step { .. } => "step",
        EventSource::Network { .. } => "network",
    }
}

pub(super) fn upsert_instrument_params(
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

pub(super) fn upsert_instrument_tensor_params(
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

pub(super) fn upsert_effect_params(
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


pub(super) fn slot_param_identity(node_id: u32, modulator_node_id: u32, raw_idx: u32) -> Option<ParamNodeId> {
    if raw_idx == u32::MAX {
        return None;
    }
    if raw_idx >= crate::instruments::voice_modulator::MOD_PARAM_BASE {
        if modulator_node_id == 0 {
            return None;
        }
        Some(ParamNodeId {
            logical_id: modulator_node_id as u64,
            node_param_idx: raw_idx - crate::instruments::voice_modulator::MOD_PARAM_BASE,
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

pub(super) fn plock_identity_matches(
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

pub(super) fn resolved_slot_param_value(
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
    let Some(raw_idx) = slot.node_param_idx(param_idx) else {
        return default_value;
    };
    let expected_id = slot_param_identity(slot.node_id, slot.modulator_node_id, raw_idx);
    if plock_identity_matches(&slot.plock_param_ids, step_idx, param_idx, expected_id) {
        plock
    } else {
        default_value
    }
}

fn resolved_sampler_host_param_value(
    slot: &crate::effects::EffectSlotSnapshot,
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

fn slot_has_explicit_plock(
    slot: &crate::effects::EffectSlotSnapshot,
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
        slot_param_identity(slot.node_id, slot.modulator_node_id, raw_idx),
    )
}

pub(super) fn slot_param_index_by_node_idx(
    slot: &crate::effects::EffectSlotSnapshot,
    node_param_idx: u32,
) -> Option<usize> {
    let num_params = slot.num_params as usize;
    (0..num_params).find(|&param_idx| slot.node_param_idx(param_idx) == Some(node_param_idx))
}

pub(super) fn resolved_slot_node_param_value(
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

pub(super) fn default_slot_node_param_value(
    slot: &crate::effects::EffectSlotSnapshot,
    node_param_idx: u32,
    default: f32,
) -> f32 {
    let Some(param_idx) = slot_param_index_by_node_idx(slot, node_param_idx) else {
        return default;
    };
    slot.defaults.get(param_idx).copied().unwrap_or(default)
}

pub(super) fn delayed_step_sample_time(
    base_sample_time: u64,
    step_params: &[f32],
    samples_per_step: f32,
) -> u64 {
    base_sample_time.saturating_add(step_delay_samples(step_params, samples_per_step))
}

pub(super) fn resolve_effect_params(
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
            let Some(raw_idx) = slot.node_param_idx(param_idx) else {
                continue;
            };
            if raw_idx == u32::MAX {
                continue;
            }
            let (logical_id, idx) = if raw_idx >= crate::instruments::voice_modulator::MOD_PARAM_BASE {
                if slot.modulator_node_id == 0 {
                    continue;
                }
                (
                    slot.modulator_node_id as u64,
                    raw_idx as u64 - crate::instruments::voice_modulator::MOD_PARAM_BASE as u64,
                )
            } else {
                (slot.node_id as u64, raw_idx as u64)
            };
            let value = resolved_slot_param_value(slot, step_idx, param_idx, 0.0);
            if !value.is_finite() {
                continue;
            }
            params.push(ScheduledEffectParam::fixed(logical_id, idx, value));
        }
    }
    params.sort_by_key(|param| (param.logical_id, param.idx));
    params
}

pub(super) fn resolve_instrument_params(
    snapshot: &SequencerSnapshot,
    track_idx: usize,
    step_idx: usize,
) -> ScheduledInstrumentParams {
    let slot = &snapshot.tracks[track_idx].instrument_slot;
    let num_params = slot.num_params as usize;
    let mut params = ScheduledInstrumentParams::new();
    for param_idx in 0..num_params {
        let Some(raw_idx) = slot.node_param_idx(param_idx) else {
            continue;
        };
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

pub(super) fn resolve_instrument_defaults(
    snapshot: &SequencerSnapshot,
    track_idx: usize,
) -> ScheduledInstrumentParams {
    let slot = &snapshot.tracks[track_idx].instrument_slot;
    let num_params = slot.num_params as usize;
    let mut params = ScheduledInstrumentParams::new();
    for param_idx in 0..num_params {
        let Some(raw_idx) = slot.node_param_idx(param_idx) else {
            continue;
        };
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

pub(super) fn resolve_instrument_tensor_params(
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

pub(super) fn resolve_instrument_tensor_defaults(
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

pub(super) fn resolve_instrument_tensor_plocks(
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

pub(super) fn resolve_effect_defaults(
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
            let Some(raw_idx) = slot.node_param_idx(param_idx) else {
                continue;
            };
            if raw_idx == u32::MAX {
                continue;
            }
            let (logical_id, idx) = if raw_idx >= crate::instruments::voice_modulator::MOD_PARAM_BASE {
                if slot.modulator_node_id == 0 {
                    continue;
                }
                (
                    slot.modulator_node_id as u64,
                    raw_idx as u64 - crate::instruments::voice_modulator::MOD_PARAM_BASE as u64,
                )
            } else {
                (slot.node_id as u64, raw_idx as u64)
            };
            let value = slot.defaults.get(param_idx).copied().unwrap_or(0.0);
            if !value.is_finite() {
                continue;
            }
            params.push(ScheduledEffectParam::fixed(logical_id, idx, value));
        }
    }
    params.sort_by_key(|param| (param.logical_id, param.idx));
    params
}

pub(super) fn resolve_instrument_plocks(
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
        let Some(raw_idx) = slot.node_param_idx(param_idx) else {
            continue;
        };
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

pub(super) fn enqueue_instrument_param_change<const QUEUE_CAP: usize>(
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

pub(super) fn resolve_midi_fx_slot_param(
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

pub(super) fn midi_fx_slot_param_value(
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

pub(super) const MIDI_FX_CLOCK_RATE_ROLE: &str = "clock-rate";
pub(super) const MIDI_FX_QUANTIZE_GRID_ROLE: &str = "quantize-grid";

#[derive(Clone, Copy)]
pub(super) struct MidiFxClockParam {
    slot_idx: usize,
    param_idx: usize,
}

pub(super) fn midi_fx_param_has_role(param: &crate::effects::ParamDescriptor, role: &str) -> bool {
    param
        .ui_metadata
        .as_ref()
        .and_then(|metadata| metadata.role.as_deref())
        .is_some_and(|param_role| param_role.eq_ignore_ascii_case(role))
}

pub(super) fn midi_fx_chain_clock_param(
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

pub(super) fn midi_fx_clock_tick_beats(
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

pub(super) fn midi_fx_timebase_param_beats(
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

pub(super) fn midi_fx_timebase_param_beats_from_slot(
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

pub(super) fn midi_fx_quantizer_grid_param(descriptor: &EffectDescriptor) -> Option<usize> {
    descriptor
        .params
        .iter()
        .position(|param| midi_fx_param_has_role(param, MIDI_FX_QUANTIZE_GRID_ROLE))
}

pub(super) fn instrument_sound_fingerprint(
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

pub(super) fn resolve_sampler_params(
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
