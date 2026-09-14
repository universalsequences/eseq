//! Resolve rack parameters without modifying the published snapshot.
//!
//! Only eight optional macro values are retained. Mappings and pattern data
//! remain borrowed, and each destination is resolved when it is dispatched.
use super::*;
use crate::sequencer::{RackMacroTarget, RACK_MACRO_COUNT};

pub(super) struct RackParams<'a> {
    pub(super) rack: &'a RackTrackSnapshot,
    pub(super) step: Option<usize>,
    values: [Option<f32>; RACK_MACRO_COUNT],
}

impl<'a> RackParams<'a> {
    pub(super) fn at_step(
        rack: &'a RackTrackSnapshot,
        step: usize,
        process_values: [Option<f32>; RACK_MACRO_COUNT],
        print_values: [Option<f32>; RACK_MACRO_COUNT],
    ) -> Self {
        let mut values = [None; RACK_MACRO_COUNT];
        for rack_macro in &rack.macros {
            let index = rack_macro.id.index();
            values[index] = Some(print_values[index].or(process_values[index])
                .unwrap_or_else(|| rack.runtime_macro_value_at(rack_macro.id, step)
                    .unwrap_or_else(|| rack_macro.value_at(step))));
        }
        Self { rack, step: Some(step), values }
    }

    pub(super) fn live(rack: &'a RackTrackSnapshot, print_values: [Option<f32>; RACK_MACRO_COUNT]) -> Self {
        let mut values = [None; RACK_MACRO_COUNT];
        for rack_macro in &rack.macros {
            let index = rack_macro.id.index();
            values[index] = Some(print_values[index]
                .or_else(|| rack.runtime_macro_default(rack_macro.id))
                .unwrap_or(rack_macro.value).clamp(0.0, 1.0));
        }
        Self { rack, step: None, values }
    }

    pub(super) fn for_update(
        rack: &'a RackTrackSnapshot,
        step: Option<usize>,
        print_values: [Option<f32>; RACK_MACRO_COUNT],
    ) -> Self {
        let mut values = [None; RACK_MACRO_COUNT];
        for rack_macro in &rack.macros {
            let index = rack_macro.id.index();
            values[index] = print_values[index].or_else(|| {
                let step = step?;
                // Off-step events apply only locked macros. An untouched
                // macro's default must not overwrite a live device control.
                rack_macro.plocks.get(step).copied().flatten()?;
                Some(rack.runtime_macro_value_at(rack_macro.id, step)
                    .unwrap_or_else(|| rack_macro.value_at(step)))
            });
        }
        Self { rack, step, values }
    }

    fn mappings(&self) -> impl Iterator<Item = (&RackMacroTarget, f32)> {
        self.rack.macros.iter().filter_map(|rack_macro| {
            self.values[rack_macro.id.index()].map(|value| (rack_macro, value))
        }).flat_map(|(rack_macro, value)| {
            rack_macro.mappings.iter().map(move |mapping| {
                let value = mapping.range_min + (mapping.range_max - mapping.range_min)
                    * rack_macro_curve_value(mapping.curve, value);
                (&mapping.target, value)
            })
        })
    }

    pub(super) fn targets(&self) -> impl Iterator<Item = &RackMacroTarget> {
        self.mappings().map(|(target, _)| target)
    }

    pub(super) fn targets_slot_param(&self, slot_idx: usize) -> bool {
        self.targets().any(|target| {
            matches!(target, RackMacroTarget::SlotParam { slot, .. } if *slot == slot_idx)
        })
    }

    pub(super) fn instrument(&self, slot_idx: usize) -> DeviceParams<'_> {
        DeviceParams::new(&self.rack.slots[slot_idx].instrument_slot, self.step,
            self.mappings().filter_map(|(target, value)| match target {
                RackMacroTarget::SlotInstrumentParam { slot, param_index, .. } if *slot == slot_idx =>
                    Some((*param_index, value)),
                _ => None,
            }))
    }

    pub(super) fn effect(&self, slot_idx: usize, effect_idx: usize) -> DeviceParams<'_> {
        DeviceParams::new(&self.rack.slots[slot_idx].effect_slots[effect_idx], self.step,
            self.mappings().filter_map(|(target, value)| match target {
                RackMacroTarget::SlotEffectParam { slot, effect_slot, param_index, .. }
                    if *slot == slot_idx && *effect_slot == effect_idx => Some((*param_index, value)),
                _ => None,
            }))
    }

    pub(super) fn instrument_params(&self, slot_idx: usize) -> ScheduledInstrumentParams {
        let device = self.instrument(slot_idx);
        resolve_rack_slot_instrument_values(device.slot, |param| device.value(param, 0.0))
    }

    pub(super) fn sampler_params(&self, slot_idx: usize) -> ScheduledSamplerParams {
        let device = self.instrument(slot_idx);
        let slot = device.slot;
        let value = |param, fallback| device.value(param, fallback);
        // Host sampler controls have no DSP-node identity. Their authored
        // locks are read directly, as in the ordinary sampler resolver.
        let host_value = |param, fallback| {
            self.step.and_then(|step| stored_lock(slot, step, param))
                .unwrap_or_else(|| resolved_device_value_or(slot, None, param,
                    device.macro_value(param), fallback))
        };
        resolve_rack_slot_sampler_values(slot, self.step, value, host_value)
    }

    pub(super) fn slot_params(&self, slot_idx: usize) -> ResolvedRackSlotParams {
        let slot = &self.rack.slots[slot_idx];
        let mut result = resolve_rack_slot_params_for_update(slot, self.step);
        for (target, value) in self.mappings() {
            let RackMacroTarget::SlotParam { slot: target_slot, param } = target else { continue; };
            if *target_slot != slot_idx { continue; }
            let Some(param) = mapped_slot_param(param) else { continue; };
            if self.step.is_some_and(|step| slot.param_plocks.get(step, param).is_some()) {
                continue;
            }
            match param {
                RackSlotParam::BaseNote => result.base_note_offset = param.clamp(value),
                RackSlotParam::Gain => result.gain = param.clamp(value),
                RackSlotParam::Pan => result.pan = param.clamp(value),
                RackSlotParam::MaxPolyphony => result.max_polyphony = param.clamp(value).round() as usize,
                RackSlotParam::Mute => result.mute = value >= 0.5,
                RackSlotParam::Solo => result.solo = value >= 0.5,
            }
        }
        result
    }
}

/// A bounded scalar overlay for one device. Build it once per dispatch, so
/// resolving N parameters does not rescan all authored mappings N times.
pub(super) struct DeviceParams<'a> {
    pub(super) slot: &'a EffectSlotSnapshot,
    step: Option<usize>,
    values: [Option<f32>; MAX_SLOT_PARAMS],
}

impl<'a> DeviceParams<'a> {
    fn new(slot: &'a EffectSlotSnapshot, step: Option<usize>, mappings: impl Iterator<Item = (usize, f32)>) -> Self {
        let mut values = [None; MAX_SLOT_PARAMS];
        for (param, value) in mappings {
            if let Some(destination) = values.get_mut(param) { *destination = Some(value); }
        }
        Self { slot, step, values }
    }

    pub(super) fn macro_value(&self, param: usize) -> Option<f32> {
        self.values.get(param).copied().flatten()
    }

    pub(super) fn value(&self, param: usize, fallback: f32) -> f32 {
        resolved_device_value_or(self.slot, self.step, param, self.macro_value(param), fallback)
    }
}

// Match the authoring spelling accepted by rack macros without allocating
// normalized strings in the audio callback.
fn mapped_slot_param(name: &str) -> Option<RackSlotParam> {
    let matches = |expected: &str| name.trim_start_matches(':').bytes()
        .map(|byte| if byte == b'_' { b'-' } else { byte.to_ascii_lowercase() })
        .eq(expected.bytes());
    RackSlotParam::ALL.into_iter().find(|param| matches(param.name()))
        .or_else(|| matches("transpose").then_some(RackSlotParam::BaseNote))
        .or_else(|| matches("polyphony").then_some(RackSlotParam::MaxPolyphony))
}

pub(super) fn resolved_device_value(
    slot: &EffectSlotSnapshot, step: Option<usize>, param_idx: usize, macro_value: Option<f32>,
) -> f32 {
    resolved_device_value_or(slot, step, param_idx, macro_value, 0.0)
}

fn stored_lock(slot: &EffectSlotSnapshot, step: usize, param: usize) -> Option<f32> {
    slot.plocks.get(step).and_then(|row| row.get(param)).copied().flatten()
}

fn resolved_device_value_or(
    slot: &EffectSlotSnapshot, step: Option<usize>, param_idx: usize,
    macro_value: Option<f32>, fallback: f32,
) -> f32 {
    let default = slot.defaults.get(param_idx).copied().unwrap_or(fallback);
    if let Some(step) = step {
        // Preserve the macro/target-lock precedence used for note-on stamps.
        // A stale lock still resolves through the slot's identity check.
        if stored_lock(slot, step, param_idx).is_some() {
            return resolved_slot_param_value(slot, step, param_idx, default);
        }
    }
    macro_value.filter(|_| param_idx < slot.defaults.len()).unwrap_or(default)
}
