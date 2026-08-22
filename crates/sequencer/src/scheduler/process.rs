/*!
Process-runtime target overlays, inlet writes, cascades, and network-trigger overrides.
*/

#[allow(unused_imports)]
use super::*;

#[derive(Clone, Debug)]
pub(super) struct ProcessTargetOverlay {
    pub(super) effect_params: Vec<ScheduledEffectParam>,
    pub(super) instrument_params: ScheduledInstrumentParams,
    pub(super) midi_fx_params: Vec<ProcessMidiFxParamOverride>,
    pub(super) rack_macro_values: [Option<f32>; crate::sequencer::RACK_MACRO_COUNT],
}

impl Default for ProcessTargetOverlay {
    fn default() -> Self {
        Self {
            effect_params: Vec::new(),
            instrument_params: ScheduledInstrumentParams::new(),
            midi_fx_params: Vec::new(),
            rack_macro_values: [None; crate::sequencer::RACK_MACRO_COUNT],
        }
    }
}

pub(super) fn process_trace(snapshot: &SequencerSnapshot, message: impl FnOnce() -> String) {
    if snapshot.process_trace {
        eprintln!("[process-trace] {}", message());
    }
}

pub(super) fn process_target_op_label(op: crate::process::ProcessTargetOp) -> &'static str {
    match op {
        crate::process::ProcessTargetOp::Set => "set",
        crate::process::ProcessTargetOp::Add => "add",
    }
}

pub(super) type StepProcessInletWrites =
    BTreeMap<usize, BTreeMap<String, Vec<crate::process::ProcessInletWrite>>>;

#[derive(Clone, Debug)]
pub(super) struct DeferredProcessInletWrite {
    pub(super) track: usize,
    pub(super) instance_id: crate::process::ProcessInstanceId,
    pub(super) inlet: String,
    pub(super) write: crate::process::ProcessInletWrite,
}

pub(super) struct ProcessInletWriteContext<'a> {
    pub(super) chain: &'a crate::process::TrackProcessChain,
    pub(super) current_slot_index: Option<usize>,
    pub(super) current_fire_writes: &'a mut StepProcessInletWrites,
    pub(super) deferred_writes: &'a mut Vec<DeferredProcessInletWrite>,
}

pub(super) fn process_target_label(target: &crate::process::ParamTarget) -> String {
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
        crate::process::ParamTarget::RackMacroParam { macro_id } => {
            format!("rack-macro:{}", macro_id + 1)
        }
    }
}

pub(super) fn process_step_param_from_name(name: &str) -> Option<StepParam> {
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

pub(super) fn resolved_step_param(resolved: &ResolvedStep, param: StepParam) -> f32 {
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

pub(super) fn set_resolved_step_param(resolved: &mut ResolvedStep, param: StepParam, value: f32) {
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

pub(super) fn process_apply_step_param_write(
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

pub(super) fn process_param_index_by_tag_or_name(
    descriptor: &EffectDescriptor,
    tag_or_name: &str,
) -> Option<usize> {
    descriptor
        .params
        .iter()
        .position(|param| param.has_tag_or_name(tag_or_name))
}

pub(super) fn process_slot_param_identity(
    slot: &crate::effects::EffectSlotSnapshot,
    param_idx: usize,
) -> Option<ParamNodeId> {
    let raw_idx = slot.node_param_idx(param_idx)?;
    slot_param_identity(slot.node_id, slot.modulator_node_id, raw_idx)
}

pub(super) fn process_scheduled_instrument_param(
    slot: &crate::effects::EffectSlotSnapshot,
    param_idx: usize,
    value: f32,
) -> Option<ScheduledInstrumentParam> {
    if param_idx >= slot.num_params as usize || !value.is_finite() {
        return None;
    }
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
    let (target, idx) = if raw_idx >= crate::instruments::voice_modulator::MOD_PARAM_BASE {
        (
            ScheduledInstrumentParamTarget::Modulator,
            (raw_idx - crate::instruments::voice_modulator::MOD_PARAM_BASE) as u64,
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

pub(super) fn process_scheduled_effect_param(
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

pub(super) fn process_device_write_value(
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

pub(super) fn process_instrument_overlay_value(
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

pub(super) fn process_effect_overlay_value(
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

pub(super) fn process_midi_fx_overlay_value(
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

pub(super) fn process_apply_instrument_write(
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

pub(super) fn process_apply_effect_write(
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

pub(super) fn process_apply_midi_fx_write(
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

pub(super) fn process_resolve_hint_to_target(
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
        crate::process::ProcessTargetHint::RackMacroParam { macro_id } => ((*macro_id as usize)
            < crate::sequencer::RACK_MACRO_COUNT)
            .then_some(crate::process::ParamTarget::RackMacroParam {
                macro_id: *macro_id,
            }),
    }
}

pub(super) fn resolve_process_inlet_target(
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

pub(super) fn process_apply_inlet_write(
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

pub(super) fn process_apply_concrete_target_write(
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
        crate::process::ParamTarget::RackMacroParam { macro_id } => {
            let index = *macro_id as usize;
            let Some(rack_macro) = snapshot
                .tracks
                .get(track)
                .and_then(|track| track.rack_track.as_ref())
                .and_then(|rack| rack.macros.get(index))
            else {
                return;
            };
            let base =
                overlay.rack_macro_values[index].unwrap_or_else(|| rack_macro.value_at(step));
            overlay.rack_macro_values[index] = Some(
                match write.op {
                    crate::process::ProcessTargetOp::Set => write.value,
                    crate::process::ProcessTargetOp::Add => base + write.value,
                }
                .clamp(0.0, 1.0),
            );
        }
    }
}

pub(super) fn apply_process_target_writes(
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

pub(super) fn step_event_with_process_overlay(
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
    let mut event = step_event_from_resolved(
        snapshot,
        track,
        step,
        samples_per_step,
        resolved,
        step_chord_data(snapshot, track, step),
        effect_params,
        instrument_params,
        resolve_instrument_tensor_params(snapshot, track, step),
    );
    event.rack_macro_values = overlay.rack_macro_values;
    event
}

pub(super) fn clamp_ratchet_event(
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
pub(super) fn materialize_process_ratchet(
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
pub(super) fn apply_step_process_commands(
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
            crate::process::ProcessRunCommand::Graph(_) => {
                // Graph commands are applied by the scheduler-owned graph runtime
                // alongside this step-local command pass.
            }
        }
    }
}

pub(super) fn apply_process_midi_fx_overrides_to_slot(
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

pub(super) fn invoke_process_cascade<F>(
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
        if invocation.reads.conductor_observe_tracks.is_empty() {
            invocation.reads = process_runtime.read_snapshot(invocation_beat);
        }
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

pub(super) fn invoke_conductor_invocations(
    scratch_runtime: &mut Option<lisp_host::ScratchControlRuntime>,
    process_runtime: &mut crate::process::ProcessRuntime,
    graph_runtimes: &mut [crate::graph::GraphRuntime],
    invocations: Vec<crate::process::ProcessRunInvocation>,
    debug_accum: bool,
) -> bool {
    for invocation in invocations {
        if !invoke_process_cascade(
            scratch_runtime,
            process_runtime,
            invocation,
            debug_accum,
            |_scratch, _runtime, runtime_id, commands| {
                apply_graph_process_commands(graph_runtimes, commands);
                if commands.iter().any(|command| {
                    !matches!(command, crate::process::ProcessRunCommand::Graph(_))
                }) && (debug_accum || debug_routing_enabled()) {
                    eprintln!(
                        "[process] conductor {} produced unsupported step commands",
                        runtime_id
                    );
                }
            },
        ) {
            return false;
        }
    }
    true
}

pub(super) fn process_step_event_value(
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

pub(super) fn normalize_network_event_destination(
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

pub(super) fn resolve_sampler_defaults(
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
        slice_mode: value(crate::instruments::sampler::SLOT_PARAM_SLICE_MODE, 0.0),
        slice_sensitivity: value(
            crate::instruments::sampler::SLOT_PARAM_SLICE_SENSITIVITY, 0.5,
        ),
        slice_base: value(crate::instruments::sampler::SLOT_PARAM_SLICE_BASE, 0.0),
        slice_division: value(
            crate::instruments::sampler::SLOT_PARAM_SLICE_DIVISION,
            crate::instruments::sampler::SLICE_DIVISION_DEFAULT,
        ),
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

#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct NeuronParameterEvents {
    pub(super) instrument: Vec<(usize, ScheduledInstrumentParams)>,
    pub(super) effects: Vec<(usize, Vec<ScheduledEffectParam>)>,
}

pub(super) fn push_target_instrument_param(
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

pub(super) fn push_target_effect_param(
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

pub(super) fn resolve_neuron_instrument_override(
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
    let (target, idx) = if raw_idx >= crate::instruments::voice_modulator::MOD_PARAM_BASE {
        (
            ScheduledInstrumentParamTarget::Modulator,
            (raw_idx - crate::instruments::voice_modulator::MOD_PARAM_BASE) as u64,
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

pub(super) fn resolve_neuron_effect_override(
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
    let (logical_id, idx) = if raw_idx >= crate::instruments::voice_modulator::MOD_PARAM_BASE {
        if slot.modulator_node_id == 0 {
            return None;
        }
        (
            slot.modulator_node_id as u64,
            raw_idx as u64 - crate::instruments::voice_modulator::MOD_PARAM_BASE as u64,
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

pub(super) fn apply_neuron_output_overrides(
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

pub(super) fn apply_sampler_descriptor_param_override(
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
        idx if idx == crate::instruments::sampler::SLOT_PARAM_SLICE_MODE as u64 => {
            params.slice_mode = value;
        }
        idx if idx == crate::instruments::sampler::SLOT_PARAM_SLICE_SENSITIVITY as u64 => {
            params.slice_sensitivity = value;
        }
        idx if idx == crate::instruments::sampler::SLOT_PARAM_SLICE_BASE as u64 => {
            params.slice_base = value;
        }
        idx if idx == crate::instruments::sampler::SLOT_PARAM_SLICE_DIVISION as u64 => {
            params.slice_division = value;
        }
        _ => return false,
    }
    true
}

pub(super) fn apply_sampler_state_param_override(
    params: &mut ScheduledSamplerParams,
    node_param_idx: u64,
    value: f32,
) -> bool {
    match node_param_idx {
        idx if idx == crate::instruments::sampler::PARAM_ATTACK_SAMPLES => params.attack_ms = value,
        idx if idx == crate::instruments::sampler::PARAM_RELEASE_SAMPLES => params.release_ms = value,
        idx if idx == crate::instruments::sampler::PARAM_START_POINT => params.start_point = value,
        idx if idx == crate::instruments::sampler::PARAM_END_POINT => params.end_point = value,
        idx if idx == crate::instruments::sampler::PARAM_ENABLED => params.instrument_enabled = value,
        idx if idx == crate::instruments::sampler::PARAM_REVERSE => params.reverse = value,
        idx if idx == crate::instruments::sampler::PARAM_LOOP_MODE => params.loop_mode = value,
        idx if idx == crate::instruments::sampler::PARAM_LOOP_XFADE_SAMPLES => params.loop_xfade_ms = value,
        idx if idx == crate::instruments::sampler::PARAM_SR_HZ => params.sr_hz = value,
        idx if idx == crate::instruments::sampler::PARAM_WARP_ENABLED => params.warp_enabled = value,
        idx if idx == crate::instruments::sampler::PARAM_WARP_MODE => params.warp_mode = value,
        idx if idx == crate::instruments::sampler::PARAM_WARP_SAMPLE_BPM => params.sample_bpm = value,
        idx if idx == crate::instruments::sampler::PARAM_SPEED => params.playback_speed = value,
        idx if idx == crate::instruments::sampler::PARAM_SCRUB_OFFSET => params.scrub = value,
        idx if idx == crate::instruments::sampler::PARAM_WARP_PRESERVE => params.warp_preserve = value,
        idx if idx == crate::instruments::sampler::PARAM_WARP_SEG_LOOP_MODE => {
            params.warp_seg_loop_mode = value;
        }
        idx if idx == crate::instruments::sampler::PARAM_WARP_SEG_ENVELOPE => {
            params.warp_seg_envelope = value;
        }
        _ => return false,
    }
    true
}

pub(super) fn sampler_descriptor_param_index_for_scheduled_param(
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

pub(super) fn apply_sampler_instrument_param_overrides(
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

pub(super) fn apply_fit_to_scale_to_trigger(
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

pub(super) fn apply_global_transpose_to_resolved(
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

pub(super) fn enqueue_network_trigger<const QUEUE_CAP: usize>(
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
    rack_macro_values: [Option<f32>; crate::sequencer::RACK_MACRO_COUNT],
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
                            rack_macro_values,
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
                rack_macro_values,
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
