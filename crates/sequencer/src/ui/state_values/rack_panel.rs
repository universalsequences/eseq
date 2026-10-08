use super::*;

pub(crate) fn rack_slot_param_value(
    rack: &sequencer::sequencer::RackTrackSnapshot,
    slot_idx: usize,
    slot: &sequencer::sequencer::RackSlotSnapshot,
    desc: &sequencer::effects::EffectDescriptor,
    param_idx: usize,
    selected_step: Option<usize>,
) -> f32 {
    let default = desc
        .params
        .get(param_idx)
        .map(|param| param.default)
        .unwrap_or_default();
    rack_slot_instrument_param_display(rack, slot_idx, slot, default, param_idx, selected_step).0
}

/// What param `param_idx` of rack slot `slot_idx`'s instrument shows at
/// `step` (stored units), and whether a p-lock supplies it: the step's
/// p-lock, else a rack macro mapped onto the param, else the slot's own
/// value (`default` past its values). Shared by the legacy rack panel
/// fields and the host kinds' rack slot params.
pub(crate) fn rack_slot_instrument_param_display(
    rack: &sequencer::sequencer::RackTrackSnapshot,
    slot_idx: usize,
    slot: &sequencer::sequencer::RackSlotSnapshot,
    default: f32,
    param_idx: usize,
    step: Option<usize>,
) -> (f32, bool) {
    if let Some(step) = step {
        if let Some(value) = slot
            .instrument_slot
            .plocks
            .get(step)
            .and_then(|step_plocks| step_plocks.get(param_idx))
            .copied()
            .flatten()
        {
            return (value, true);
        }
    }
    if let Some(value) = rack_macro_mapped_value(rack, step, |target| {
        matches!(
            target,
            sequencer::sequencer::RackMacroTarget::SlotInstrumentParam {
                slot,
                param_index,
                ..
            } if *slot == slot_idx && *param_index == param_idx
        )
    }) {
        return (value, false);
    }
    let base = slot.instrument_slot.defaults.get(param_idx).copied();
    (base.unwrap_or(default), false)
}

pub(super) fn rack_macro_mapped_value(
    rack: &sequencer::sequencer::RackTrackSnapshot,
    selected_step: Option<usize>,
    target_matches: impl Fn(&sequencer::sequencer::RackMacroTarget) -> bool,
) -> Option<f32> {
    rack.macros.iter().find_map(|rack_macro| {
        rack_macro.mappings.iter().find_map(|mapping| {
            if !target_matches(&mapping.target) {
                return None;
            }
            let macro_value = selected_step
                .map(|step| rack_macro.value_at(step))
                .unwrap_or(rack_macro.value);
            let curved = match mapping.curve {
                sequencer::sequencer::RackMacroCurve::Linear => macro_value,
                sequencer::sequencer::RackMacroCurve::Exp => macro_value * macro_value,
                sequencer::sequencer::RackMacroCurve::Log => macro_value.sqrt(),
            };
            Some(mapping.range_min + (mapping.range_max - mapping.range_min) * curved)
        })
    })
}

pub(super) fn rack_effect_param_value(
    rack: &sequencer::sequencer::RackTrackSnapshot,
    rack_slot: usize,
    effect_slot: usize,
    snapshot: &sequencer::effects::EffectSlotSnapshot,
    descriptor: &sequencer::effects::EffectDescriptor,
    param_idx: usize,
    selected_step: Option<usize>,
) -> f32 {
    let default = descriptor.params[param_idx].default;
    rack_effect_param_display(
        rack,
        rack_slot,
        effect_slot,
        snapshot,
        default,
        param_idx,
        selected_step,
    )
    .0
}

/// What param `param_idx` of effect `effect_slot` of rack slot `rack_slot`
/// shows at `step` (stored units), and whether a p-lock supplies it, as
/// [`rack_slot_instrument_param_display`] for a rack slot's effect.
pub(crate) fn rack_effect_param_display(
    rack: &sequencer::sequencer::RackTrackSnapshot,
    rack_slot: usize,
    effect_slot: usize,
    snapshot: &sequencer::effects::EffectSlotSnapshot,
    default: f32,
    param_idx: usize,
    step: Option<usize>,
) -> (f32, bool) {
    if let Some(step) = step {
        if let Some(value) = snapshot
            .plocks
            .get(step)
            .and_then(|step_plocks| step_plocks.get(param_idx))
            .copied()
            .flatten()
        {
            return (value, true);
        }
    }
    let mapped = rack_macro_mapped_value(rack, step, |target| {
        matches!(
            target,
            sequencer::sequencer::RackMacroTarget::SlotEffectParam {
                slot,
                effect_slot: target_effect_slot,
                param_index,
                ..
            } if *slot == rack_slot
                && *target_effect_slot == effect_slot
                && *param_index == param_idx
        )
    });
    let base = || snapshot.defaults.get(param_idx).copied().unwrap_or(default);
    (mapped.unwrap_or_else(base), false)
}

pub(crate) fn rack_macro_mapping_display_metadata(
    app: &app::App,
    rack: &sequencer::sequencer::RackTrackSnapshot,
    mapping: &sequencer::sequencer::RackMacroMapping,
) -> (String, String, f32, f32, f32, f32, f32, u8, String) {
    let (slot_idx, descriptor, param_idx) = match &mapping.target {
        sequencer::sequencer::RackMacroTarget::SlotInstrumentParam {
            slot, param_index, ..
        } => (
            *slot,
            rack.slots
                .get(*slot)
                .and_then(|slot| app.rack_slot_instrument_descriptor(slot)),
            *param_index,
        ),
        sequencer::sequencer::RackMacroTarget::SlotEffectParam {
            slot,
            effect_slot,
            param_index,
            ..
        } => (
            *slot,
            rack.slots
                .get(*slot)
                .and_then(|slot| slot.effect_descriptors.get(*effect_slot))
                .cloned(),
            *param_index,
        ),
        sequencer::sequencer::RackMacroTarget::SlotParam { slot, param } => {
            return (
                format!("Layer {}", slot + 1),
                param.clone(),
                mapping.range_min,
                mapping.range_max,
                mapping.range_min,
                mapping.range_max,
                1.0,
                2,
                String::new(),
            );
        }
    };
    let Some(descriptor) = descriptor else {
        return (
            format!("Layer {}", slot_idx + 1),
            match &mapping.target {
                sequencer::sequencer::RackMacroTarget::SlotInstrumentParam { param, .. }
                | sequencer::sequencer::RackMacroTarget::SlotEffectParam { param, .. } => {
                    param.clone()
                }
                sequencer::sequencer::RackMacroTarget::SlotParam { param, .. } => param.clone(),
            },
            mapping.range_min,
            mapping.range_max,
            mapping.range_min,
            mapping.range_max,
            1.0,
            2,
            String::new(),
        );
    };
    let Some(param) = descriptor.params.get(param_idx) else {
        return (
            format!("Layer {} · {}", slot_idx + 1, descriptor.name),
            "missing parameter".to_string(),
            mapping.range_min,
            mapping.range_max,
            mapping.range_min,
            mapping.range_max,
            1.0,
            2,
            String::new(),
        );
    };
    let scale = if param.is_percent() { 100.0 } else { 1.0 };
    let (decimals, unit) = match &param.kind {
        sequencer::effects::ParamKind::Boolean | sequencer::effects::ParamKind::Enum { .. } => {
            (0, String::new())
        }
        sequencer::effects::ParamKind::Continuous { unit } => (
            if unit.as_deref() == Some("%") { 1 } else { 2 },
            unit.clone().unwrap_or_default(),
        ),
    };
    (
        format!("Layer {} · {}", slot_idx + 1, descriptor.name),
        param.name.clone(),
        param.stored_to_user(mapping.range_min),
        param.stored_to_user(mapping.range_max),
        param.stored_to_user(param.min),
        param.stored_to_user(param.max),
        scale,
        decimals,
        unit,
    )
}

