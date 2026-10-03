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

/// The step an emitted hit (generator, process, graph, cross-track neural
/// fire) lands on in its destination track's step grid. The hit stamps that
/// step's device params — base values, the step's p-locks, the live
/// device-print latch — exactly like an ON trigger there, so p-locks recorded
/// on an untriggered pattern ride the hits another sequencer plays. Stamping
/// at the onset is what per-hit instruments (params latched at note-on) need:
/// the off-step automation event reaches the voice after it has latched.
#[derive(Clone)]
pub(super) struct StepLanding {
    pub(super) step: usize,
    pub(super) print_overrides: Option<crate::sequencer::DeviceParamPrintValues>,
}

impl StepLanding {
    /// Where a hit at straight (pre-groove) transport `beats` lands on
    /// `track`, with the track's live print latch.
    pub(super) fn resolve(
        clock: &SnapshotSequencerClock,
        state: &SequencerState,
        snapshot: &SequencerSnapshot,
        track: usize,
        beats: f64,
        samples_per_quarter: f64,
    ) -> Option<Self> {
        // One sample of slack: emitters round their beats to samples.
        let tolerance = if samples_per_quarter > 0.0 {
            1.0 / samples_per_quarter
        } else {
            0.0
        };
        let step = clock.track_step_at_beats(snapshot, track, beats, tolerance)?;
        Some(Self {
            step,
            print_overrides: state.device_print_override.values_for_track(track),
        })
    }

    /// Replace `event`'s device params with this landing step's full stamp.
    /// Callers upsert any params the emitter set explicitly afterwards.
    pub(super) fn stamp(&self, snapshot: &SequencerSnapshot, event: &mut StepEvent) {
        let track = event.track;
        let print_overrides = self.print_overrides.as_ref();
        event.effect_params = resolve_effect_params(snapshot, track, self.step, print_overrides);
        event
            .effect_params
            .extend(resolve_track_send_params(snapshot, track, self.step));
        event.instrument_params =
            resolve_instrument_params(snapshot, track, self.step, print_overrides);
        event.instrument_tensor_params =
            resolve_instrument_tensor_params(snapshot, track, self.step);
        event.sampler_params = resolve_sampler_params(snapshot, track, self.step);
    }
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

/// A named value clamped to its descriptor's range (params store and
/// schedule raw, knob-unit values, not normalized ones).
fn named_param_value(desc: &crate::effects::ParamDescriptor, value: f32) -> f32 {
    let (lo, hi) = if desc.min <= desc.max {
        (desc.min, desc.max)
    } else {
        (desc.max, desc.min)
    };
    value.clamp(lo, hi)
}

/// Fold `seq-emit :params` (docs/jaki-plock-spec.md §3) into `event` after
/// its landing stamp, resolving each label BY NAME against `event.track`, so
/// a per-hit value beats the step's base, its stored p-lock and the print
/// latch. Values are in the param's own units, clamped to its range. A name
/// the destination does not have drops just that param; the note plays.
///
/// Applied: instrument, effect (slotted or first-by-name), MIDI FX (via
/// `midi_fx_params`, needs `midi_fx`), step params, and on a slot-based
/// Instrument Rack track its macros and `rack<N>:…` slot params (the slot's
/// own mixer params and its instrument's params, via
/// `event.rack_slot_params`). Skipped: `send:*` (spec §7).
pub(super) fn apply_named_params(
    snapshot: &SequencerSnapshot,
    midi_fx: Option<&lisp_host::MidiFxDescriptorSource>,
    named: &[(crate::process::ParamRef, f32)],
    event: &mut StepEvent,
    midi_fx_params: &mut Vec<ProcessMidiFxParamOverride>,
) {
    use crate::process::ParamRef;
    let Some(track) = snapshot.tracks.get(event.track) else {
        return;
    };
    for (param_ref, value) in named {
        let value = *value;
        if !value.is_finite() {
            continue;
        }
        match param_ref {
            ParamRef::Instrument { param } => {
                let desc = &track.instrument_descriptor;
                let Some(param_idx) = process_param_index_by_tag_or_name(desc, param) else {
                    continue;
                };
                let value = named_param_value(&desc.params[param_idx], value);
                if let Some(scheduled) =
                    process_scheduled_instrument_param(&track.instrument_slot, param_idx, value)
                {
                    upsert_instrument_params(&mut event.instrument_params, [scheduled]);
                }
            }
            ParamRef::Effect {
                slot,
                effect,
                param,
            } => {
                let slot_idx = match slot {
                    Some(slot) => track
                        .effect_descriptors
                        .get(*slot)
                        .is_some_and(|desc| desc.name.eq_ignore_ascii_case(effect))
                        .then_some(*slot),
                    None => track
                        .effect_descriptors
                        .iter()
                        .position(|desc| desc.name.eq_ignore_ascii_case(effect)),
                };
                let Some(slot_idx) = slot_idx else {
                    continue;
                };
                let desc = &track.effect_descriptors[slot_idx];
                let (Some(param_idx), Some(slot_snapshot)) = (
                    process_param_index_by_tag_or_name(desc, param),
                    track.effect_slots.get(slot_idx),
                ) else {
                    continue;
                };
                let value = named_param_value(&desc.params[param_idx], value);
                if let Some(scheduled) = process_scheduled_effect_param(slot_snapshot, param_idx, value)
                {
                    upsert_effect_params(&mut event.effect_params, [scheduled]);
                }
            }
            ParamRef::MidiFx { slot, fx, param } => {
                let chain = &track.params.midi_fx_chain;
                let slot_idx = match slot {
                    Some(slot) => chain
                        .get(*slot)
                        .is_some_and(|name| name.eq_ignore_ascii_case(fx))
                        .then_some(*slot),
                    None => chain.iter().position(|name| name.eq_ignore_ascii_case(fx)),
                };
                let (Some(slot_idx), Some(source)) = (slot_idx, midi_fx) else {
                    continue;
                };
                let Some(desc) = source.descriptor(&chain[slot_idx]) else {
                    continue;
                };
                let Some(param_idx) = process_param_index_by_tag_or_name(&desc, param) else {
                    continue;
                };
                let param_desc = &desc.params[param_idx];
                let value = named_param_value(param_desc, value);
                match midi_fx_params.iter_mut().find(|existing| {
                    existing.slot == slot_idx && existing.param_idx == param_idx
                }) {
                    Some(existing) => existing.value = value,
                    None => midi_fx_params.push(ProcessMidiFxParamOverride {
                        slot: slot_idx,
                        fx: chain[slot_idx].clone(),
                        param: param_desc.name.clone(),
                        param_idx,
                        value,
                    }),
                }
            }
            ParamRef::Step { param } => {
                if matches!(param, StepParam::Sync | StepParam::Delay) {
                    continue;
                }
                set_resolved_step_param(&mut event.resolved, *param, value);
            }
            ParamRef::RackMacro { macro_idx } => {
                let has_macro = track
                    .rack_track
                    .as_ref()
                    .is_some_and(|rack| rack.macros.get(*macro_idx).is_some());
                if has_macro {
                    if let Some(slot) = event.rack_macro_values.get_mut(*macro_idx) {
                        *slot = Some(value.clamp(0.0, 1.0));
                    }
                }
            }
            ParamRef::RackSlot { slot, param } => {
                let has_slot = track
                    .rack_track
                    .as_ref()
                    .is_some_and(|rack| rack.slots.get(*slot).is_some());
                let Some(param) = crate::sequencer::RackSlotParam::from_label(param).filter(|_| has_slot) else {
                    continue;
                };
                scheduled_event::upsert_rack_slot_param(
                    &mut event.rack_slot_params,
                    ScheduledRackSlotParam {
                        slot: *slot,
                        target: ScheduledRackSlotTarget::Slot(param),
                        value: param.clamp(value),
                    },
                );
            }
            ParamRef::RackSlotInstrument { slot, param } => {
                let Some(rack_slot) = track
                    .rack_track
                    .as_ref()
                    .and_then(|rack| rack.slots.get(*slot))
                else {
                    continue;
                };
                let Some(desc) = snapshot.rack_slot_instrument_descriptor(rack_slot) else {
                    continue;
                };
                let Some(param_idx) = process_param_index_by_tag_or_name(desc, param)
                    .filter(|idx| *idx < rack_slot.instrument_slot.defaults.len())
                else {
                    continue;
                };
                scheduled_event::upsert_rack_slot_param(
                    &mut event.rack_slot_params,
                    ScheduledRackSlotParam {
                        slot: *slot,
                        target: ScheduledRackSlotTarget::Instrument { param_idx },
                        value: named_param_value(&desc.params[param_idx], value),
                    },
                );
            }
            // Bus sends are a v1 non-goal (spec §7).
            ParamRef::Send { .. } => {}
        }
    }
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
    slot.explicit_plock_value(step_idx, param_idx).is_some()
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

/// The latched print value for one device target, if the live device-param
/// print latch (bead eseq-prm) holds it. Substituted at resolution time —
/// mirroring `StepPrintOverride` — because every trigger stamps all device
/// params from the snapshot, so without this a held printing knob is actively
/// reset to its stale snapshot value on each passing step and the gesture is
/// only heard one loop later.
fn device_print_value(
    overrides: Option<&crate::sequencer::DeviceParamPrintValues>,
    target: crate::sequencer::DeviceParamPrintTarget,
) -> Option<f32> {
    overrides?
        .entries
        .iter()
        .find(|(entry, _)| *entry == target)
        .map(|(_, value)| *value)
}

pub(super) fn resolve_effect_params(
    snapshot: &SequencerSnapshot,
    track_idx: usize,
    step_idx: usize,
    print_overrides: Option<&crate::sequencer::DeviceParamPrintValues>,
) -> Vec<ScheduledEffectParam> {
    let mut params = Vec::new();
    for (slot_idx, slot) in snapshot.tracks[track_idx].effect_slots.iter().enumerate() {
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
            let value = device_print_value(
                print_overrides,
                crate::sequencer::DeviceParamPrintTarget::Effect { slot_idx, param_idx },
            )
            .unwrap_or_else(|| resolved_slot_param_value(slot, step_idx, param_idx, 0.0));
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
    print_overrides: Option<&crate::sequencer::DeviceParamPrintValues>,
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
        let value = device_print_value(
            print_overrides,
            crate::sequencer::DeviceParamPrintTarget::Instrument { param_idx },
        )
        .unwrap_or_else(|| resolved_slot_param_value(slot, step_idx, param_idx, 0.0));
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

/// Plock-only effect resolution for inactive steps: an off-step effect p-lock
/// applies to the track's per-track effect chain at that step boundary, the
/// effect analog of the off-step instrument p-lock path. Untouched params emit
/// nothing, so values hold until the next p-lock or the next ON trigger's full
/// stamp. `print_overrides` (the live device-print latch) beats stored p-locks
/// and also emits for latched params with no stored p-lock yet, so a held
/// printing knob is audible on off-step boundaries too.
pub(super) fn resolve_effect_plocks(
    snapshot: &SequencerSnapshot,
    track_idx: usize,
    step_idx: usize,
    print_overrides: Option<&crate::sequencer::DeviceParamPrintValues>,
) -> Vec<ScheduledEffectParam> {
    let mut params = Vec::new();
    for (slot_idx, slot) in snapshot.tracks[track_idx].effect_slots.iter().enumerate() {
        if slot.node_id == 0 {
            continue;
        }
        let num_params = slot.num_params as usize;
        for param_idx in 0..num_params {
            let printed = device_print_value(
                print_overrides,
                crate::sequencer::DeviceParamPrintTarget::Effect { slot_idx, param_idx },
            );
            let value = match printed {
                Some(value) => value,
                None => {
                    let Some(value) = slot
                        .plocks
                        .get(step_idx)
                        .and_then(|step| step.get(param_idx))
                        .copied()
                        .flatten()
                    else {
                        continue;
                    };
                    if !slot_has_explicit_plock(slot, step_idx, param_idx) {
                        continue;
                    }
                    value
                }
            };
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
    print_overrides: Option<&crate::sequencer::DeviceParamPrintValues>,
) -> ScheduledInstrumentParams {
    let slot = &snapshot.tracks[track_idx].instrument_slot;
    let num_params = slot.num_params as usize;
    let step_plocks = slot.plocks.get(step_idx);
    let mut params = ScheduledInstrumentParams::new();
    for param_idx in 0..num_params {
        let printed = device_print_value(
            print_overrides,
            crate::sequencer::DeviceParamPrintTarget::Instrument { param_idx },
        );
        let Some(value) = printed.or_else(|| {
            step_plocks.and_then(|step| step.get(param_idx).copied().flatten())
        }) else {
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
        // A latched print value carries no stored p-lock id — only stored
        // p-locks are subject to the stale-device identity check.
        if printed.is_none() {
            let expected_id =
                slot_param_identity(slot.node_id, slot.modulator_node_id, raw_idx);
            if !plock_identity_matches(&slot.plock_param_ids, step_idx, param_idx, expected_id) {
                continue;
            }
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
            audition_generation: 0,
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

#[cfg(test)]
mod rack_slot_named_param_tests {
    use super::*;
    use crate::effects::EffectSlotSnapshot;
    use crate::process::ParamRef;
    use crate::sequencer::{
        default_empty_effect_chain, default_rack_macros, CustomInstrumentRunMode, InstrumentType,
        RackSlotParam, RackSlotParamPlocks, RackSlotSnapshot, RackTrackSnapshot, SequencerState,
        TrackSoundState,
    };

    fn slot(instrument_type: InstrumentType, desc: &EffectDescriptor) -> RackSlotSnapshot {
        RackSlotSnapshot {
            instrument_type,
            instrument_run_mode: CustomInstrumentRunMode::Instrument,
            instrument_base_note_offset: 0.0,
            choke_group: None,
            gain: 1.0,
            pan: 0.0,
            mute: false,
            solo: false,
            enabled: true,
            max_polyphony: 2,
            param_plocks: RackSlotParamPlocks::new(),
            instrument_slot: EffectSlotSnapshot::new_default(desc, 9),
            effect_slots: RackSlotSnapshot::empty_effect_slots(),
            effect_descriptors: EffectDescriptor::default_full_chain(),
            custom_effect_names: RackSlotSnapshot::empty_effect_names(),
            track_sound_state: TrackSoundState::default(),
            sample_id: None,
        }
    }

    fn event() -> StepEvent {
        StepEvent {
            track: 0,
            samples_per_step: 6_000.0,
            resolved: ResolvedStep {
                duration: 1.0,
                velocity: 1.0,
                speed: 1.0,
                aux_a: 0.0,
                aux_b: 0.0,
                transpose: 0.0,
                pan: 0.0,
                chop: 1.0,
                retrig: StepParam::Retrig.default_value(),
                retrig_rate: StepParam::RetrigRate.default_value(),
            },
            chord: ScheduledChordData {
                live_origins: [None; MAX_VOICES],
                count: 0,
                notes: [0.0; MAX_VOICES],
                durations: [0.0; MAX_VOICES],
                delays: [0.0; MAX_VOICES],
                step_transpose: 0.0,
            },
            effect_params: Vec::new(),
            instrument_params: ScheduledInstrumentParams::new(),
            instrument_tensor_params: ScheduledInstrumentTensorParams::new(),
            sampler_params: ScheduledSamplerParams::default(),
            rack_macro_values: [None; crate::sequencer::RACK_MACRO_COUNT],
            rack_slot_params: Default::default(),
            source: EventSource::Step {
                track: 0,
                step: 0,
                instrument_fingerprint: 0,
            },
        }
    }

    #[test]
    fn rack_slot_params_resolve_by_name_against_each_slots_instrument() {
        let state = SequencerState::new(1, vec![default_empty_effect_chain()]);
        let filter = EffectDescriptor::builtin_filter();
        let sampler = EffectDescriptor::builtin_sampler();
        let mut custom = slot(InstrumentType::Custom, &filter);
        custom.track_sound_state.engine_id = Some(0);
        state.set_rack_track_for_all_pattern_snapshots(
            0,
            RackTrackSnapshot::new(
                vec![custom, slot(InstrumentType::Sampler, &sampler)],
                default_rack_macros(),
            ),
        );
        state.sync_engine_instrument_descriptors(1, || vec![filter.clone()]);
        let snapshot = state.publish_scheduler_snapshot();

        let named: Vec<(ParamRef, f32)> = [
            // Filter mode is 0..3: 7 clamps.
            ("rack1:instrument:mode", 7.0),
            ("rack2:instrument:speed", 2.0),
            ("rack2:gain", 5.0),
            ("rack1:pan", -0.25),
            // Each of these drops alone: no such param, no slot 3.
            ("rack1:instrument:nosuch", 1.0),
            ("rack1:nosuch", 1.0),
            ("rack3:gain", 1.0),
        ]
        .into_iter()
        .map(|(label, value)| (ParamRef::parse(label).expect(label), value))
        .collect();
        let mut event = event();
        apply_named_params(&snapshot, None, &named, &mut event, &mut Vec::new());

        let index = |desc: &EffectDescriptor, name: &str| {
            desc.params.iter().position(|param| param.name == name).unwrap()
        };
        assert_eq!(
            event.rack_slot_params.as_slice(),
            &[
                ScheduledRackSlotParam {
                    slot: 0,
                    target: ScheduledRackSlotTarget::Instrument {
                        param_idx: index(&filter, "mode"),
                    },
                    value: 3.0,
                },
                ScheduledRackSlotParam {
                    slot: 1,
                    target: ScheduledRackSlotTarget::Instrument {
                        param_idx: index(&sampler, "speed"),
                    },
                    value: 2.0,
                },
                ScheduledRackSlotParam {
                    slot: 1,
                    target: ScheduledRackSlotTarget::Slot(RackSlotParam::Gain),
                    value: 2.0,
                },
                ScheduledRackSlotParam {
                    slot: 0,
                    target: ScheduledRackSlotTarget::Slot(RackSlotParam::Pan),
                    value: -0.25,
                },
            ]
        );
        // A track-level instrument param is untouched by slot names.
        assert!(event.instrument_params.is_empty());
    }

    #[test]
    fn rack_slot_instrument_params_need_the_engine_descriptor() {
        let state = SequencerState::new(1, vec![default_empty_effect_chain()]);
        let filter = EffectDescriptor::builtin_filter();
        let mut custom = slot(InstrumentType::Custom, &filter);
        custom.track_sound_state.engine_id = Some(0);
        state.set_rack_track_for_all_pattern_snapshots(
            0,
            RackTrackSnapshot::new(vec![custom], default_rack_macros()),
        );
        let snapshot = state.publish_scheduler_snapshot();
        let named = vec![(ParamRef::parse("rack1:instrument:mode").unwrap(), 1.0)];
        let mut event = event();
        apply_named_params(&snapshot, None, &named, &mut event, &mut Vec::new());
        assert!(event.rack_slot_params.is_empty(), "unknown engine: dropped");
    }
}
