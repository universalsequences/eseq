use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::time::Instant;

use crossterm::event::KeyCode;

use crate::effects::{EffectDescriptor, BUILTIN_SLOT_COUNT};
use crate::lisp_host;
use crate::neural::ParamNodeId;
use crate::project::{
    self, chord_snapshot_from_steps_durations_and_delays, project_file_version, ProjectBusChannel,
    ProjectBusPatternSnapshot, ProjectFile, ProjectPattern, ProjectReverbState,
    ProjectScratchState, ProjectTrack,
};
use crate::sequencer::{
    BusGateSequence, BusId, BusPatternSnapshot, CustomInstrumentRunMode, InstrumentType,
    PatternSnapshot, RackRouting, RackTrackSnapshot, TrackOutput, MAX_STEPS, TRACK_PATTERN_WORDS,
};

use super::graph::{
    RackCustomBuildSpec, RackSamplerBuildSpec, RackSlotBuildSpec, RackSlotInstrumentBuildSpec,
};
use super::{App, BusChannelState, InputMode, Region, SidebarMode, SidebarTab};

fn project_slot_into_synced_snapshot(
    slot: project::ProjectEffectSlot,
    desc: &EffectDescriptor,
    node_id: u32,
) -> crate::effects::EffectSlotSnapshot {
    project_slot_into_synced_snapshot_with_modulator(slot, desc, node_id, 0)
}

fn project_slot_into_synced_snapshot_with_modulator(
    slot: project::ProjectEffectSlot,
    desc: &EffectDescriptor,
    node_id: u32,
    modulator_node_id: u32,
) -> crate::effects::EffectSlotSnapshot {
    let mut snapshot = slot.into_snapshot_with_node_ids(node_id, modulator_node_id);
    snapshot.sync_to_descriptor_with_modulator(desc, node_id, modulator_node_id);
    snapshot
}

fn project_track_effect_slot_into_synced_snapshot(
    slot: project::ProjectEffectSlot,
    desc: &EffectDescriptor,
    live_slot: &crate::effects::EffectSlotState,
) -> crate::effects::EffectSlotSnapshot {
    let node_id = live_slot.node_id.load(Ordering::Relaxed);
    let modulator_node_id = live_slot.modulator_node_id.load(Ordering::Relaxed);
    project_slot_into_synced_snapshot_with_modulator(slot, desc, node_id, modulator_node_id)
}

fn project_midi_fx_slot_into_synced_snapshot(
    slot: project::ProjectEffectSlot,
    fx_name: Option<&str>,
) -> crate::effects::EffectSlotSnapshot {
    if let Some(desc) = fx_name.and_then(crate::lisp_host::load_midi_fx_descriptor) {
        project_slot_into_synced_snapshot(slot, &desc, 0)
    } else {
        slot.into_snapshot_with_node_id(0)
    }
}

fn restore_saved_bus_effect_slot_runtime_ids(
    slot: &mut crate::effects::EffectSlotSnapshot,
    saved_slot: crate::effects::EffectSlotSnapshot,
) {
    let live_node_id = slot.node_id;
    let live_modulator_node_id = slot.modulator_node_id;
    *slot = saved_slot;
    slot.node_id = live_node_id;
    slot.modulator_node_id = live_modulator_node_id;
}

fn slot_param_node_relative_idx(raw_idx: u32) -> Option<u32> {
    if raw_idx == u32::MAX {
        return None;
    }
    Some(if raw_idx >= crate::voice_modulator::MOD_PARAM_BASE {
        raw_idx - crate::voice_modulator::MOD_PARAM_BASE
    } else {
        raw_idx
    })
}

fn refreshed_slot_param_id(
    slot: &crate::effects::EffectSlotSnapshot,
    saved_param_idx: usize,
    saved_param_id: ParamNodeId,
) -> Option<(usize, ParamNodeId)> {
    let live_param_id = |param_idx: usize| {
        let raw_idx = slot.param_node_indices.get(param_idx).copied()?;
        let node_relative_idx = slot_param_node_relative_idx(raw_idx)?;
        (node_relative_idx == saved_param_id.node_param_idx)
            .then(|| ParamNodeId::from_slot_param(slot.node_id, slot.modulator_node_id, raw_idx))?
    };

    if let Some(param_id) = live_param_id(saved_param_idx) {
        return Some((saved_param_idx, param_id));
    }

    let matches = slot
        .param_node_indices
        .iter()
        .enumerate()
        .filter_map(|(param_idx, raw_idx)| {
            let node_relative_idx = slot_param_node_relative_idx(*raw_idx)?;
            (node_relative_idx == saved_param_id.node_param_idx).then(|| {
                ParamNodeId::from_slot_param(slot.node_id, slot.modulator_node_id, *raw_idx)
                    .map(|param_id| (param_idx, param_id))
            })?
        })
        .collect::<Vec<_>>();

    (matches.len() == 1).then(|| matches[0])
}

fn refresh_neural_output_override_param_ids(snapshot: &mut PatternSnapshot) {
    for network in &mut snapshot.neural_networks {
        for neuron in &mut network.neurons {
            for override_param in &mut neuron.output_overrides.instrument {
                let Some(slot) = snapshot.instrument_slots.get(override_param.target_track) else {
                    continue;
                };
                if let Some((param_index, param_id)) = refreshed_slot_param_id(
                    slot,
                    override_param.param_index,
                    override_param.param_id,
                ) {
                    override_param.param_index = param_index;
                    override_param.param_id = param_id;
                }
            }

            for override_param in &mut neuron.output_overrides.effects {
                let Some(slot) = snapshot
                    .effect_slots
                    .get(override_param.target_track)
                    .and_then(|slots| slots.get(override_param.slot_index))
                else {
                    continue;
                };
                if let Some((param_index, param_id)) = refreshed_slot_param_id(
                    slot,
                    override_param.param_index,
                    override_param.param_id,
                ) {
                    override_param.param_index = param_index;
                    override_param.param_id = param_id;
                }
            }
        }
    }
}

fn default_project_effect_slot(desc: &EffectDescriptor) -> project::ProjectEffectSlot {
    let num_params = desc.params.len();
    project::ProjectEffectSlot {
        num_params: num_params as u32,
        defaults: desc.params.iter().map(|param| param.default).collect(),
        plocks: (0..MAX_STEPS).map(|_| vec![None; num_params]).collect(),
        plock_param_ids: (0..MAX_STEPS).map(|_| vec![None; num_params]).collect(),
        key_locks: std::collections::BTreeMap::new(),
        key_lock_param_ids: std::collections::BTreeMap::new(),
        tensor_params: desc
            .tensor_params
            .iter()
            .map(|tensor| crate::effects::TensorParamSnapshot {
                name: tensor.name.clone(),
                shape: tensor.shape.clone(),
                cell_offset: tensor.cell_offset,
                default: tensor.default.clone(),
                plocks: Vec::new(),
            })
            .collect(),
        param_node_indices: desc
            .params
            .iter()
            .map(|param| param.node_param_idx)
            .collect(),
        param_node_spans: desc
            .params
            .iter()
            .map(|param| param.node_param_span.max(1))
            .collect(),
        ir: None,
    }
}

fn legacy_builtin_slot_has_edits(
    slot: &project::ProjectEffectSlot,
    desc: &EffectDescriptor,
) -> bool {
    if slot.num_params == 0 {
        return false;
    }
    let num_params = (slot.num_params as usize).min(desc.params.len());
    for param_idx in 0..num_params {
        let saved = slot.defaults.get(param_idx).copied().unwrap_or(0.0);
        if (saved - desc.params[param_idx].default).abs() > 0.0001 {
            return true;
        }
    }
    slot.plocks
        .iter()
        .any(|row| row.iter().take(num_params).any(Option::is_some))
}

fn migrate_legacy_default_track_effects(project: &mut ProjectFile) {
    let mut filter_desc = EffectDescriptor::builtin_filter();
    let mut delay_desc = EffectDescriptor::builtin_delay();
    if let Some(enabled) = filter_desc
        .params
        .iter_mut()
        .find(|param| param.name == "enabled")
    {
        enabled.default = 0.0;
    }
    if let Some(enabled) = delay_desc
        .params
        .iter_mut()
        .find(|param| param.name == "enabled")
    {
        enabled.default = 0.0;
    }
    let legacy_descs = [&filter_desc, &delay_desc];
    let legacy_names = ["Filter", "Delay"];
    let max_slots = crate::lisp_host::MAX_CUSTOM_FX;

    for track_idx in 0..project.tracks.len() {
        let old_custom_len = project
            .custom_effects
            .get(track_idx)
            .map(Vec::len)
            .unwrap_or_default();
        let has_legacy_layout = project.patterns.iter().any(|pattern| {
            pattern
                .effect_slots
                .get(track_idx)
                .map(|slots| slots.len() >= 2 && slots.len() > old_custom_len)
                .unwrap_or(false)
        });
        if !has_legacy_layout {
            continue;
        }

        let mut preserve_legacy = [false; 2];
        for pattern in &project.patterns {
            let Some(slots) = pattern.effect_slots.get(track_idx) else {
                continue;
            };
            for legacy_idx in 0..2 {
                if let Some(slot) = slots.get(legacy_idx) {
                    preserve_legacy[legacy_idx] |=
                        legacy_builtin_slot_has_edits(slot, legacy_descs[legacy_idx]);
                }
            }
        }

        let old_names = project
            .custom_effects
            .get(track_idx)
            .cloned()
            .unwrap_or_default();
        let mut migrated_names = Vec::new();
        for legacy_idx in 0..2 {
            if preserve_legacy[legacy_idx] {
                migrated_names.push(EffectDescriptor::builtin_insert_project_name(
                    legacy_names[legacy_idx],
                ));
            }
        }
        migrated_names.extend(old_names);
        migrated_names.truncate(max_slots);
        while project.custom_effects.len() <= track_idx {
            project.custom_effects.push(Vec::new());
        }
        project.custom_effects[track_idx] = migrated_names;

        for pattern in &mut project.patterns {
            let old_slots = pattern
                .effect_slots
                .get(track_idx)
                .cloned()
                .unwrap_or_default();
            let mut migrated_slots = Vec::new();
            for legacy_idx in 0..2 {
                if preserve_legacy[legacy_idx] {
                    migrated_slots.push(
                        old_slots.get(legacy_idx).cloned().unwrap_or_else(|| {
                            default_project_effect_slot(legacy_descs[legacy_idx])
                        }),
                    );
                }
            }
            if old_slots.len() > 2 {
                migrated_slots.extend(old_slots.into_iter().skip(2));
            }
            migrated_slots.truncate(max_slots);
            while pattern.effect_slots.len() <= track_idx {
                pattern.effect_slots.push(Vec::new());
            }
            pattern.effect_slots[track_idx] = migrated_slots;
        }
    }
}

fn project_builtin_effect_name_for_save(name: &str) -> Option<String> {
    let trimmed = name.trim();
    crate::effects::EffectDescriptor::builtin_insert_project_name(trimmed).or_else(|| {
        crate::conv_reverb::is_dgen_builtin(trimmed).then(|| {
            format!(
                "{}{}",
                crate::effects::EffectDescriptor::BUILTIN_INSERT_PREFIX,
                crate::conv_reverb::NAME
            )
        })
    })
}

fn project_builtin_effect_name_for_load(name: &str) -> Option<String> {
    if let Some(builtin_name) =
        crate::effects::EffectDescriptor::strip_builtin_insert_project_name(name)
    {
        return Some(builtin_name.to_string());
    }
    let stripped = name
        .trim()
        .strip_prefix(crate::effects::EffectDescriptor::BUILTIN_INSERT_PREFIX)?
        .trim();
    crate::conv_reverb::is_dgen_builtin(stripped).then(|| crate::conv_reverb::NAME.to_string())
}

fn migrate_dgen_builtin_effect_names(project: &mut ProjectFile) {
    fn migrate_name(name: &mut Option<String>) {
        let Some(raw_name) = name.as_deref() else {
            return;
        };
        if crate::conv_reverb::is_dgen_builtin(raw_name.trim()) {
            *name = project_builtin_effect_name_for_save(raw_name);
        }
    }

    for track_effects in &mut project.custom_effects {
        for name in track_effects {
            migrate_name(name);
        }
    }
    for bus in &mut project.buses {
        for name in &mut bus.custom_effects {
            migrate_name(name);
        }
    }
}

fn resolve_project_current_track(
    saved_current_track: Option<usize>,
    track_count: usize,
    current_pattern_track_bits: Option<&[[u64; TRACK_PATTERN_WORDS]]>,
) -> usize {
    if track_count == 0 {
        return 0;
    }
    saved_current_track
        .or_else(|| {
            current_pattern_track_bits.and_then(|track_bits| {
                track_bits
                    .iter()
                    .position(|bits| bits.iter().any(|word| *word != 0))
            })
        })
        .unwrap_or(0)
        .min(track_count - 1)
}

fn project_custom_instrument_slot_into_synced_snapshot(
    slot: project::ProjectEffectSlot,
    desc: &crate::effects::EffectDescriptor,
    node_id: u32,
    modulator_node_id: u32,
) -> crate::effects::EffectSlotSnapshot {
    if project_slot_matches_descriptor_param_layout(&slot, desc) {
        let mut snapshot = project_slot_into_synced_snapshot_with_modulator(
            slot,
            desc,
            node_id,
            modulator_node_id,
        );
        snapshot.recompute_modulation_active_params(desc);
        return snapshot;
    }

    let new_np = desc.params.len();
    let has_legacy_fixed_voice_mod_params = slot.param_node_indices.iter().any(|&node_idx| {
        node_idx >= crate::voice_modulator::LEGACY_FIXED_MOD_PARAM_BASE
            && node_idx < crate::voice_modulator::LEGACY_FIXED_MOD_PARAM_BASE_END
    });
    let has_generated_mod_params = desc
        .params
        .iter()
        .any(|param| is_generated_mod_runtime_param_name(&param.name));
    let inserted_enabled = desc
        .params
        .iter()
        .position(|param| param.name.eq_ignore_ascii_case("enabled"))
        .filter(|_| {
            let comparable_new_params = desc
                .params
                .iter()
                .filter(|param| !is_generated_mod_runtime_param_name(&param.name))
                .count();
            slot.num_params as usize + 1 == comparable_new_params
        });

    let old_non_generated_idx_for_new_idx = |new_idx: usize| {
        desc.params[..new_idx]
            .iter()
            .filter(|param| !is_generated_mod_runtime_param_name(&param.name))
            .count()
    };

    let find_old_idx_by_node = |target: u32| {
        slot.param_node_indices
            .iter()
            .position(|&saved| saved == target)
    };

    let old_idx_for = |new_idx: usize, param: &crate::effects::ParamDescriptor| -> Option<usize> {
        if has_legacy_fixed_voice_mod_params
            && param.node_param_idx >= crate::voice_modulator::MOD_PARAM_BASE
        {
            return None;
        }

        if is_generated_mod_runtime_param_name(&param.name) {
            return None;
        }

        if inserted_enabled == Some(new_idx) {
            return None;
        }

        if let Some(enabled_idx) = inserted_enabled {
            if !has_generated_mod_params {
                if param.node_param_idx >= crate::lisp_host::HEADER_SLOTS as u32
                    && param.node_param_idx < crate::voice_modulator::MOD_PARAM_BASE
                {
                    if let Some(old_idx) = find_old_idx_by_node(param.node_param_idx - 1) {
                        return Some(old_idx);
                    }
                } else if let Some(old_idx) = find_old_idx_by_node(param.node_param_idx) {
                    return Some(old_idx);
                }
            }

            return Some(if new_idx > enabled_idx {
                old_non_generated_idx_for_new_idx(new_idx) - 1
            } else {
                old_non_generated_idx_for_new_idx(new_idx)
            })
            .filter(|&old_idx| old_idx < slot.defaults.len());
        }

        if !has_generated_mod_params {
            find_old_idx_by_node(param.node_param_idx)
                .or_else(|| (new_idx < slot.defaults.len()).then_some(new_idx))
        } else {
            Some(old_non_generated_idx_for_new_idx(new_idx))
                .filter(|&old_idx| old_idx < slot.defaults.len())
        }
    };

    let mut defaults = desc
        .params
        .iter()
        .map(|param| param.default)
        .collect::<Vec<_>>();
    for (new_idx, param) in desc.params.iter().enumerate() {
        if let Some(old_idx) = old_idx_for(new_idx, param) {
            if let Some(value) = slot.defaults.get(old_idx).copied() {
                defaults[new_idx] = value;
            }
        }
    }

    let mut plocks = (0..MAX_STEPS)
        .map(|_| vec![None; new_np])
        .collect::<Vec<_>>();
    let mut plock_param_ids = (0..MAX_STEPS)
        .map(|_| vec![None; new_np])
        .collect::<Vec<_>>();
    let mut key_locks = std::collections::BTreeMap::new();
    let mut key_lock_param_ids = std::collections::BTreeMap::new();
    for step in 0..MAX_STEPS {
        for (new_idx, param) in desc.params.iter().enumerate() {
            let Some(old_idx) = old_idx_for(new_idx, param) else {
                continue;
            };
            if let Some(value) = slot
                .plocks
                .get(step)
                .and_then(|row| row.get(old_idx))
                .copied()
                .flatten()
            {
                plocks[step][new_idx] = Some(value);
                plock_param_ids[step][new_idx] =
                    ParamNodeId::from_slot_param(node_id, modulator_node_id, param.node_param_idx);
            }
        }
    }
    for (&note, saved_row) in &slot.key_locks {
        for (new_idx, param) in desc.params.iter().enumerate() {
            let Some(old_idx) = old_idx_for(new_idx, param) else {
                continue;
            };
            if let Some(value) = saved_row.get(old_idx).copied().flatten() {
                key_locks.entry(note).or_insert_with(|| vec![None; new_np])[new_idx] = Some(value);
                key_lock_param_ids
                    .entry(note)
                    .or_insert_with(|| vec![None; new_np])[new_idx] =
                    ParamNodeId::from_slot_param(node_id, modulator_node_id, param.node_param_idx);
            }
        }
    }

    let mut snapshot = crate::effects::EffectSlotSnapshot {
        node_id,
        modulator_node_id,
        num_params: new_np as u32,
        defaults,
        plocks,
        plock_param_ids,
        key_locks,
        key_lock_param_ids,
        tensor_params: slot.tensor_params.clone(),
        param_node_indices: desc.params.iter().map(|p| p.node_param_idx).collect(),
        param_node_spans: desc
            .params
            .iter()
            .map(|p| p.node_param_span.max(1))
            .collect(),
        transport_phase_param_idx: desc
            .transport_phase_param_idx()
            .unwrap_or(crate::effects::NO_TRANSPORT_PHASE_PARAM),
        ir: slot.ir.clone(),
    };
    snapshot.recompute_modulation_active_params(desc);
    snapshot
}

fn project_slot_matches_descriptor_param_layout(
    slot: &project::ProjectEffectSlot,
    desc: &crate::effects::EffectDescriptor,
) -> bool {
    let num_params = desc.params.len();
    slot.num_params as usize == num_params
        && slot.defaults.len() >= num_params
        && slot.param_node_indices.len() >= num_params
        && slot.param_node_spans.len() >= num_params
        && desc
            .params
            .iter()
            .zip(slot.param_node_indices.iter())
            .zip(slot.param_node_spans.iter())
            .all(|((param, saved_node_idx), saved_node_span)| {
                param.node_param_idx == *saved_node_idx
                    && param.node_param_span.max(1) == (*saved_node_span).max(1)
            })
}

fn is_generated_mod_runtime_param_name(name: &str) -> bool {
    name.starts_with("__host_mod__")
        || name.starts_with("__dgen_mod_active__")
        || (name.starts_with("mod ") && name.contains(" slot ") && name.ends_with(" amt"))
}

fn project_bus_gate_sequence_from_ui(
    sequence: &BusGateSequence,
) -> project::ProjectBusGateSequence {
    project::ProjectBusGateSequence {
        steps: sequence.steps.to_vec(),
        velocities: sequence.velocities.to_vec(),
        durations: sequence.durations.to_vec(),
        syncs: sequence.syncs.to_vec(),
        num_steps: sequence.num_steps,
        timebase: sequence.timebase as u8,
        swing: sequence.swing,
        swing_resolution: sequence.swing_resolution as u8,
        timebase_plocks: sequence
            .timebase_plocks
            .iter()
            .map(|value| value.map(|timebase| timebase as u32))
            .collect(),
        swing_plocks: sequence.swing_plocks.to_vec(),
        swing_resolution_plocks: sequence
            .swing_resolution_plocks
            .iter()
            .map(|value| value.map(|resolution| resolution as u32))
            .collect(),
    }
}

fn project_bus_gate_sequence_to_ui(sequence: project::ProjectBusGateSequence) -> BusGateSequence {
    let mut restored = BusGateSequence::default();
    for (idx, value) in sequence.steps.into_iter().take(MAX_STEPS).enumerate() {
        restored.steps[idx] = value;
    }
    for (idx, value) in sequence.velocities.into_iter().take(MAX_STEPS).enumerate() {
        restored.velocities[idx] = value.clamp(0.0, 1.0);
    }
    for (idx, value) in sequence.durations.into_iter().take(MAX_STEPS).enumerate() {
        restored.durations[idx] = value.clamp(0.1, 2.0);
    }
    for (idx, value) in sequence.syncs.into_iter().take(MAX_STEPS).enumerate() {
        restored.set_step_sync(idx, value);
    }
    restored.set_num_steps(sequence.num_steps);
    restored.timebase = crate::sequencer::Timebase::from_index(sequence.timebase as u32);
    restored.swing = sequence.swing.clamp(50.0, 75.0);
    restored.swing_resolution =
        crate::sequencer::SwingResolution::from_index(sequence.swing_resolution as u32);
    for (idx, value) in sequence
        .timebase_plocks
        .into_iter()
        .take(MAX_STEPS)
        .enumerate()
    {
        restored.timebase_plocks[idx] = value.map(crate::sequencer::Timebase::from_index);
    }
    for (idx, value) in sequence
        .swing_plocks
        .into_iter()
        .take(MAX_STEPS)
        .enumerate()
    {
        restored.swing_plocks[idx] = value.map(|swing| swing.clamp(50.0, 75.0));
    }
    for (idx, value) in sequence
        .swing_resolution_plocks
        .into_iter()
        .take(MAX_STEPS)
        .enumerate()
    {
        restored.swing_resolution_plocks[idx] =
            value.map(crate::sequencer::SwingResolution::from_index);
    }
    restored
}

fn project_bus_pattern_snapshot_from_ui(
    snapshot: &BusPatternSnapshot,
) -> ProjectBusPatternSnapshot {
    ProjectBusPatternSnapshot {
        id: snapshot.id.0,
        gate_sequence: project_bus_gate_sequence_from_ui(&snapshot.gate_sequence),
        effect_slots: snapshot
            .effect_plocks
            .iter()
            .enumerate()
            .map(|(slot_idx, plocks)| {
                let defaults = snapshot
                    .effect_defaults
                    .get(slot_idx)
                    .cloned()
                    .unwrap_or_default();
                let num_params = plocks
                    .iter()
                    .map(Vec::len)
                    .max()
                    .unwrap_or(0)
                    .max(defaults.len()) as u32;
                crate::project::ProjectEffectSlot {
                    num_params,
                    defaults,
                    plocks: plocks.clone(),
                    plock_param_ids: (0..MAX_STEPS).map(|_| Vec::new()).collect(),
                    key_locks: std::collections::BTreeMap::new(),
                    key_lock_param_ids: std::collections::BTreeMap::new(),
                    tensor_params: Vec::new(),
                    param_node_indices: Vec::new(),
                    param_node_spans: Vec::new(),
                    ir: None,
                }
            })
            .collect(),
    }
}

fn project_bus_pattern_snapshot_to_ui(snapshot: ProjectBusPatternSnapshot) -> BusPatternSnapshot {
    BusPatternSnapshot {
        id: BusId(snapshot.id),
        gate_sequence: project_bus_gate_sequence_to_ui(snapshot.gate_sequence),
        effect_defaults: snapshot
            .effect_slots
            .iter()
            .map(|slot| slot.defaults.clone())
            .collect(),
        effect_plocks: snapshot
            .effect_slots
            .into_iter()
            .map(|slot| slot.plocks)
            .collect(),
    }
}

impl From<BusChannelState> for ProjectBusChannel {
    fn from(value: BusChannelState) -> Self {
        Self {
            id: value.id.0,
            name: value.name,
            volume: value.volume,
            mute: value.mute,
            solo: value.solo,
            gate_sequence: project_bus_gate_sequence_from_ui(&value.gate_sequence),
            custom_effects: value
                .custom_effect_names
                .into_iter()
                .map(|name| {
                    name.and_then(|name| {
                        project_builtin_effect_name_for_save(&name).or_else(|| Some(name))
                    })
                })
                .collect(),
            effect_slots: value
                .effect_slots
                .iter()
                .map(crate::project::ProjectEffectSlot::from)
                .collect(),
        }
    }
}

impl From<ProjectBusChannel> for BusChannelState {
    fn from(value: ProjectBusChannel) -> Self {
        let mut bus = Self::new(BusId(value.id), value.name);
        bus.volume = value.volume.clamp(0.0, 1.0);
        bus.mute = value.mute;
        bus.solo = value.solo;
        bus.gate_sequence = project_bus_gate_sequence_to_ui(value.gate_sequence);
        for (idx, name) in value.custom_effects.into_iter().enumerate() {
            if idx < bus.custom_effect_names.len() {
                bus.custom_effect_names[idx] = name;
            }
        }
        for (idx, slot) in value.effect_slots.into_iter().enumerate() {
            if idx < bus.effect_slots.len() {
                bus.effect_slots[idx] = slot.into_snapshot_with_node_id(0);
            }
        }
        bus
    }
}

impl App {
    pub fn start_new_project(&mut self) {
        self.editor.pending_project_load = None;
        self.groups.clear();

        {
            let mut graph = self.graph_controller();
            graph.clear_all_tracks();
            graph.clear_all_bus_effect_chains();
        }

        let removable_bus_ids = self
            .buses
            .iter()
            .map(|bus| bus.id)
            .filter(|id| *id != BusId::MIX && *id != BusId::DEFAULT_A && *id != BusId::DEFAULT_B)
            .collect::<Vec<_>>();
        for id in removable_bus_ids {
            self.delete_bus_channel(id);
        }

        self.buses = BusChannelState::default_buses();
        for bus in self.buses.clone() {
            self.graph_controller()
                .ensure_bus_graph_node(bus.id, &bus.name);
        }
        let default_bus_snapshot = self.capture_bus_pattern_snapshot();
        self.state
            .replace_bus_pattern_repository(Vec::new(), &default_bus_snapshot);
        self.restore_bus_pattern_snapshot(&default_bus_snapshot);
        for bus in &self.buses {
            let Some(nodes) = self
                .graph
                .bus_node_ids
                .iter()
                .find(|nodes| nodes.id == bus.id)
            else {
                continue;
            };
            unsafe {
                crate::audiograph::params_push_wrapper(
                    self.graph.lg.0,
                    crate::audiograph::ParamMsg {
                        idx: crate::stereo_panner::STEREO_PANNER_PARAM_VOLUME,
                        logical_id: nodes.volume_id as u64,
                        fvalue: crate::mixer_volume::fader_to_gain(bus.volume),
                    },
                );
                crate::audiograph::params_push_wrapper(
                    self.graph.lg.0,
                    crate::audiograph::ParamMsg {
                        idx: crate::stereo_panner::STEREO_PANNER_PARAM_MUTE,
                        logical_id: nodes.volume_id as u64,
                        fvalue: if bus.mute { 1.0 } else { 0.0 },
                    },
                );
            }
        }

        self.state
            .transport
            .bpm
            .store(crate::sequencer::DEFAULT_BPM, Ordering::Relaxed);
        self.state
            .transport
            .master_volume
            .store(1.0_f32.to_bits(), Ordering::Relaxed);
        self.push_master_volume();
        self.set_reverb_param(0, 0.2);
        self.set_reverb_param(1, 0.8);
        self.set_reverb_param(2, 0.3);

        self.current_project_name = None;
        self.editor.scratch_buffer.clear();
        self.editor.scratch_cursor = (0, 0);
        self.editor.scratch_runtime = None;
        self.state.set_scratch_source(String::new());
        self.clear_control_hooks();

        self.ui.value_buffer.clear();
        self.ui.input_mode = InputMode::Normal;
        self.ui.cursor_track = 0;
        self.ui.cursor_step = 0;
        self.ui.pattern_page = 0;
        self.ui.focused_region = Region::Sidebar;
        self.ui.sidebar_tab = SidebarTab::Sounds;
        self.ui.sidebar_mode = SidebarMode::InstrumentPicker;
        self.ui.sidebar_search_focused = false;
        self.ui.selection_anchor = None;
        self.ui.track_selection_anchor = None;
        self.ui.visual_steps.clear();

        self.editor.status_message = Some(("New project".to_string(), Instant::now()));
    }

    pub fn add_bus_channel(&mut self, name: impl Into<String>) -> BusId {
        let next_id = self
            .buses
            .iter()
            .map(|bus| bus.id.0)
            .max()
            .unwrap_or(crate::sequencer::DEFAULT_BUS_B_ID)
            .saturating_add(1);
        let id = BusId(next_id.max(crate::sequencer::DEFAULT_BUS_B_ID + 1));
        self.buses.push(BusChannelState::new(id, name));
        if let Some(bus) = self.buses.iter().find(|bus| bus.id == id).cloned() {
            self.graph_controller()
                .ensure_bus_graph_node(bus.id, &bus.name);
        }
        self.publish_bus_gate_runtime();
        id
    }

    pub fn delete_bus_channel(&mut self, id: BusId) -> bool {
        if id == BusId::MIX {
            return false;
        }

        let Some(bus_idx) = self.buses.iter().position(|bus| bus.id == id) else {
            return false;
        };

        if let Some(bus) = self.buses.get(bus_idx) {
            for slot in &bus.effect_slots {
                if slot.node_id != 0 {
                    unsafe {
                        crate::audiograph::delete_node(self.graph.lg.0, slot.node_id as i32);
                    }
                    crate::conv_reverb::clear_instance(slot.node_id as i32);
                }
                if slot.modulator_node_id != 0 {
                    unsafe {
                        crate::audiograph::delete_node(
                            self.graph.lg.0,
                            slot.modulator_node_id as i32,
                        );
                    }
                }
            }
        }

        self.buses.remove(bus_idx);
        if bus_idx < self.editor.bus_effect_leases.len() {
            self.editor.bus_effect_leases.remove(bus_idx);
        }

        self.remove_bus_references_from_live_pattern(id);
        {
            let track_count = self.tracks.len();
            let mut graph = self.graph_controller();
            for track_idx in 0..track_count {
                graph.apply_track_output_routing(track_idx);
                graph.apply_track_bus_sends(track_idx);
            }
        }
        self.graph_controller().delete_bus_graph_node(id);
        self.publish_bus_gate_runtime();
        self.state.remove_bus_references_from_all_track_patterns(id);
        true
    }

    /// Route a track's output to `output` in the live pattern AND every stored
    /// scene, then apply graph routing. Group membership is global, so its
    /// members must keep this routing across every scene — see
    /// `SequencerState::set_track_output_in_all_track_patterns`.
    pub fn set_track_output_all_scenes(&mut self, track: usize, output: TrackOutput) {
        if track >= self.state.pattern.track_params.len() {
            return;
        }
        self.state.pattern.track_params[track].set_output(output.clone());
        self.state
            .set_track_output_in_all_track_patterns(track, output);
        self.graph_controller().apply_track_output_routing(track);
    }

    fn remove_bus_references_from_live_pattern(&self, id: BusId) {
        for track in 0..self.state.active_track_count() {
            let params = &self.state.pattern.track_params[track];
            if params.output() == TrackOutput::Bus(id) {
                params.set_output(TrackOutput::Mix);
            }
            let sends = params
                .sends()
                .into_iter()
                .filter(|send| send.destination != id)
                .collect();
            params.set_sends(sends);
        }
    }

    pub fn save_project_with_name(
        &mut self,
        requested_name: Option<&str>,
    ) -> Result<String, String> {
        let requested_name = requested_name
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(str::to_string)
            .or_else(|| self.current_project_name.clone())
            .ok_or_else(|| "Project name must contain letters or numbers".to_string())?;

        let save_name = project::sanitize_project_name(&requested_name);
        if save_name.is_empty() {
            return Err("Project name must contain letters or numbers".to_string());
        }

        self.save_project_named(&save_name)?;
        self.current_project_name = Some(save_name.clone());
        Ok(save_name)
    }

    pub fn queue_project_load_named(&mut self, name: &str) -> Result<(), String> {
        eprintln!("project-load: queue requested name={name}");
        let mut project = project::load_project(name).map_err(|error| error.to_string())?;
        eprintln!(
            "project-load: file loaded name={} version={} tracks={} patterns={} custom_effect_tracks={}",
            name,
            project.version,
            project.tracks.len(),
            project.patterns.len(),
            project.custom_effects.len()
        );
        if project.version != project::project_file_version() {
            return Err(format!("Unsupported project version {}", project.version));
        }
        migrate_legacy_default_track_effects(&mut project);
        migrate_dgen_builtin_effect_names(&mut project);

        self.editor.pending_project_load = Some(super::PendingProjectLoad {
            name: name.to_string(),
            tick: 0,
            project,
            built_patterns: Vec::new(),
            built_bus_patterns: Vec::new(),
            fallback_samples: 0,
            phase: super::PendingProjectLoadPhase::ClearExisting,
        });
        Ok(())
    }

    pub fn has_pending_project_load(&self) -> bool {
        self.editor.pending_project_load.is_some()
    }

    pub fn advance_pending_project_load(&mut self) -> Result<(), String> {
        let result = self.advance_project_load();
        if result.is_err() {
            self.editor.pending_project_load = None;
        }
        result
    }

    pub(super) fn open_project_name_prompt(&mut self) {
        self.ui.value_buffer = self.current_project_name.clone().unwrap_or_default();
        self.ui.input_mode = InputMode::ProjectNameEntry;
    }

    pub(super) fn open_project_picker(&mut self) {
        match project::list_project_names() {
            Ok(items) => {
                self.editor.picker_items = items;
                self.editor.picker_cursor = 0;
                self.editor.picker_filter.clear();
                self.ui.input_mode = InputMode::ProjectPicker;
            }
            Err(error) => {
                self.editor.status_message = Some((format!("Error: {error}"), Instant::now()));
            }
        }
    }

    pub(super) fn handle_project_name_entry(&mut self, code: KeyCode) {
        match code {
            KeyCode::Char(c) => self.ui.value_buffer.push(c),
            KeyCode::Backspace => {
                self.ui.value_buffer.pop();
            }
            KeyCode::Enter => {
                let requested_name = self.ui.value_buffer.trim().to_string();
                self.ui.value_buffer.clear();
                self.ui.input_mode = InputMode::Normal;
                if requested_name.is_empty() {
                    return;
                }
                match self.save_project_with_name(Some(&requested_name)) {
                    Ok(save_name) => {
                        self.editor.status_message =
                            Some((format!("Saved project '{}'", save_name), Instant::now()));
                    }
                    Err(error) => {
                        self.editor.status_message =
                            Some((format!("Error: {error}"), Instant::now()));
                    }
                }
            }
            KeyCode::Esc => {
                self.ui.value_buffer.clear();
                self.ui.input_mode = InputMode::Normal;
            }
            _ => {}
        }
    }

    pub(super) fn handle_project_picker(&mut self, code: KeyCode) {
        match code {
            KeyCode::Char(c) => {
                self.editor.picker_filter.push(c);
                self.editor.picker_cursor = 0;
            }
            KeyCode::Backspace => {
                self.editor.picker_filter.pop();
                self.editor.picker_cursor = 0;
            }
            KeyCode::Up => {
                if self.editor.picker_cursor > 0 {
                    self.editor.picker_cursor -= 1;
                }
            }
            KeyCode::Down => {
                if self.editor.picker_cursor + 1 < self.filtered_project_items().len() {
                    self.editor.picker_cursor += 1;
                }
            }
            KeyCode::Enter => {
                let Some(name) = self
                    .filtered_project_items()
                    .get(self.editor.picker_cursor)
                    .cloned()
                else {
                    return;
                };
                self.ui.input_mode = InputMode::Normal;
                match self.queue_project_load_named(&name) {
                    Ok(()) => {}
                    Err(error) => {
                        self.editor.status_message =
                            Some((format!("Error: {error}"), Instant::now()));
                    }
                }
            }
            KeyCode::Esc => {
                self.editor.picker_filter.clear();
                self.editor.picker_cursor = 0;
                self.ui.input_mode = InputMode::Normal;
            }
            _ => {}
        }
    }

    pub(super) fn filtered_project_items(&self) -> Vec<String> {
        if self.editor.picker_filter.is_empty() {
            return self.editor.picker_items.clone();
        }
        let filter = self.editor.picker_filter.to_lowercase();
        self.editor
            .picker_items
            .iter()
            .filter(|item| item.to_lowercase().contains(&filter))
            .cloned()
            .collect()
    }

    pub(super) fn save_project_named(&mut self, project_name: &str) -> Result<(), String> {
        let project = self.capture_project(project_name)?;
        project::save_project(project_name, &project).map_err(|error| error.to_string())?;
        Ok(())
    }

    fn capture_project(&mut self, project_name: &str) -> Result<ProjectFile, String> {
        let num_tracks = self.tracks.len();
        let current_pattern = self.state.current_scene_index();
        let current_track = if num_tracks == 0 {
            0
        } else {
            self.ui.cursor_track.min(num_tracks - 1)
        };

        self.state.save_current_pattern_snapshot(
            num_tracks,
            &self.graph.track_buffer_ids,
            &self.graph.track_sample_rates,
            &self.tracks,
            &self.graph.track_instrument_types,
        );
        self.save_current_bus_pattern();

        let bank = self.state.export_pattern_repository();
        let default_bus_snapshot = self.capture_bus_pattern_snapshot();
        self.state
            .ensure_bus_pattern_repository_len(bank.len(), &default_bus_snapshot);
        let bus_pattern_bank = self
            .state
            .export_bus_pattern_repository(&default_bus_snapshot);
        let tracks = self.capture_project_tracks()?;
        let custom_effects = self.capture_custom_effects();
        let patterns = bank
            .iter()
            .enumerate()
            .map(|(pattern_idx, snapshot)| {
                let mut sample_paths = Vec::with_capacity(num_tracks);
                let mut sample_names = Vec::with_capacity(num_tracks);
                for track_idx in 0..num_tracks {
                    let (sample_buffer_id, sample_name) = snapshot
                        .sample_ids
                        .get(track_idx)
                        .map(|(buffer_id, name, _)| (*buffer_id, name.clone()))
                        .unwrap_or((-1, String::new()));
                    let sample_path = if snapshot
                        .instrument_types
                        .get(track_idx)
                        .copied()
                        .unwrap_or(InstrumentType::Sampler)
                        == InstrumentType::Sampler
                        && !sample_name.is_empty()
                    {
                        self.resolve_sample_path_for_snapshot(
                            pattern_idx,
                            track_idx,
                            sample_buffer_id,
                            &sample_name,
                        )?
                        .map(|path| path.to_string_lossy().to_string())
                    } else {
                        None
                    };
                    sample_paths.push(sample_path);
                    sample_names.push(sample_name);
                }
                Ok(ProjectPattern::from_snapshot(
                    snapshot,
                    sample_paths,
                    sample_names,
                    bus_pattern_bank
                        .get(pattern_idx)
                        .cloned()
                        .unwrap_or_else(|| self.capture_bus_pattern_snapshot())
                        .iter()
                        .map(project_bus_pattern_snapshot_from_ui)
                        .collect(),
                ))
            })
            .collect::<Result<Vec<_>, String>>()?;

        Ok(ProjectFile {
            version: project_file_version(),
            name: project_name.to_string(),
            bpm: self.state.transport.bpm.load(Ordering::Relaxed),
            master_volume: f32::from_bits(
                self.state.transport.master_volume.load(Ordering::Relaxed),
            ),
            current_pattern,
            current_track: Some(current_track),
            reverb: ProjectReverbState {
                size: self.ui.reverb_size,
                brightness: self.ui.reverb_brightness,
                replace: self.ui.reverb_replace,
            },
            buses: self
                .buses
                .iter()
                .cloned()
                .map(ProjectBusChannel::from)
                .collect(),
            tracks,
            custom_effects,
            scratch: ProjectScratchState {
                buffer: self.editor.scratch_buffer.clone(),
                cursor_row: self.editor.scratch_cursor.0,
                cursor_col: self.editor.scratch_cursor.1,
            },
            patterns,
            groups: self.groups.clone(),
        })
    }

    fn capture_project_tracks(&self) -> Result<Vec<ProjectTrack>, String> {
        self.tracks
            .iter()
            .enumerate()
            .map(|(track_idx, name)| {
                let color = self.track_colors.get(track_idx).copied();
                let collapsed = self
                    .track_collapsed
                    .get(track_idx)
                    .copied()
                    .unwrap_or(false);
                if self.graph.track_instrument_types.get(track_idx) == Some(&InstrumentType::Rack)
                {
                    let rack = self
                        .state
                        .pattern
                        .rack_tracks
                        .lock()
                        .unwrap()
                        .get(track_idx)
                        .cloned()
                        .flatten()
                        .ok_or_else(|| {
                            format!("Rack track '{}' has no rack metadata", name)
                        })?;
                    let mut slots = Vec::with_capacity(rack.slots.len());
                    for (slot_idx, slot) in rack.slots.iter().enumerate() {
                        match slot.instrument_type {
                            InstrumentType::Sampler => {
                                let sample_name = slot
                                    .sample_id
                                    .as_ref()
                                    .map(|(_, name, _)| name.clone())
                                    .unwrap_or_default();
                                let path = self
                                    .sample_path_registry
                                    .get(&sample_name)
                                    .cloned()
                                    .or_else(|| self.resolve_sample_path_by_name(&sample_name))
                                    .ok_or_else(|| {
                                        format!(
                                            "Couldn't resolve sample path for rack track '{}' slot {}",
                                            name,
                                            slot_idx + 1
                                        )
                                    })?;
                                slots.push(crate::project::ProjectRackTrackSlot {
                                    instrument_type: crate::project::ProjectInstrumentType::Sampler,
                                    sample_path: Some(path.to_string_lossy().to_string()),
                                    sample_name: (!sample_name.is_empty()).then_some(sample_name),
                                    instrument_name: None,
                                });
                            }
                            InstrumentType::Custom => {
                                let engine_id = slot.track_sound_state.engine_id.ok_or_else(|| {
                                    format!(
                                        "Rack track '{}' slot {} has no custom engine binding",
                                        name,
                                        slot_idx + 1
                                    )
                                })?;
                                let instrument_name = self
                                    .editor
                                    .engine_registry
                                    .get(engine_id)
                                    .map(|engine| engine.name.clone())
                                    .ok_or_else(|| {
                                        format!(
                                            "Rack track '{}' slot {} engine {} is missing from registry",
                                            name,
                                            slot_idx + 1,
                                            engine_id
                                        )
                                    })?;
                                slots.push(crate::project::ProjectRackTrackSlot {
                                    instrument_type: crate::project::ProjectInstrumentType::Custom,
                                    sample_path: None,
                                    sample_name: None,
                                    instrument_name: Some(instrument_name),
                                });
                            }
                            InstrumentType::Modulator | InstrumentType::Rack => {
                                return Err(format!(
                                    "Rack track '{}' slot {} has unsupported instrument type",
                                    name,
                                    slot_idx + 1
                                ));
                            }
                        }
                    }
                    Ok(ProjectTrack::Rack {
                        routing: crate::project::ProjectRackRouting::from(rack.routing),
                        slots,
                        color,
                        collapsed,
                    })
                } else if self.is_sampler_track(track_idx) {
                    let path = self
                        .sampler_path_for_track(track_idx)
                        .or_else(|| self.resolve_sample_path_by_name(name));
                    let Some(path) = path else {
                        return Err(format!("Couldn't resolve sample path for '{}'", name));
                    };
                    Ok(ProjectTrack::Sampler {
                        sample_path: path.to_string_lossy().to_string(),
                        color,
                        collapsed,
                    })
                } else if self.graph.track_instrument_types.get(track_idx)
                    == Some(&InstrumentType::Modulator)
                {
                    Ok(ProjectTrack::Modulator { color, collapsed })
                } else {
                    let instrument_name = self
                        .graph
                        .track_engine_ids
                        .get(track_idx)
                        .and_then(|engine_id| *engine_id)
                        .and_then(|engine_id| self.editor.engine_registry.get(engine_id))
                        .map(|engine| engine.name.clone())
                        .unwrap_or_else(|| name.clone());
                    Ok(ProjectTrack::Custom {
                        instrument_name,
                        color,
                        collapsed,
                    })
                }
            })
            .collect()
    }

    fn capture_custom_effects(&self) -> Vec<Vec<Option<String>>> {
        self.tracks
            .iter()
            .enumerate()
            .map(|(track_idx, _)| {
                (BUILTIN_SLOT_COUNT..self.graph.effect_descriptors[track_idx].len())
                    .map(|slot_idx| {
                        let slot = &self.state.pattern.effect_chains[track_idx][slot_idx];
                        if slot.node_id.load(Ordering::Relaxed) == 0 {
                            None
                        } else {
                            let name = self.graph.effect_descriptors[track_idx][slot_idx]
                                .name
                                .trim()
                                .to_string();
                            if name.is_empty() {
                                None
                            } else if let Some(project_name) =
                                project_builtin_effect_name_for_save(&name)
                            {
                                Some(project_name)
                            } else {
                                Some(name)
                            }
                        }
                    })
                    .collect()
            })
            .collect()
    }

    fn resolve_sample_path_for_snapshot(
        &self,
        pattern_idx: usize,
        track_idx: usize,
        buffer_id: i32,
        sample_name: &str,
    ) -> Result<Option<PathBuf>, String> {
        if self.state.current_scene_index() == pattern_idx {
            if let Some(path) = self.sampler_path_for_track(track_idx) {
                return Ok(Some(path));
            }
        }
        if let Some(path) = self.sample_buffer_path_registry.get(&buffer_id) {
            return Ok(Some(path.clone()));
        }
        if let Some(path) = self.sample_path_registry.get(sample_name) {
            return Ok(Some(path.clone()));
        }
        let resolved = self.resolve_sample_path_by_name(sample_name);
        if resolved.is_none() {
            return Err(format!(
                "Couldn't resolve sample path for '{}'",
                sample_name
            ));
        }
        Ok(resolved)
    }

    fn resolve_sample_path_by_name(&self, sample_name: &str) -> Option<PathBuf> {
        fn walk(dir: &Path, sample_name: &str) -> Option<PathBuf> {
            let entries = std::fs::read_dir(dir).ok()?;
            for entry in entries {
                let entry = entry.ok()?;
                let path = entry.path();
                if path.is_dir() {
                    if let Some(found) = walk(&path, sample_name) {
                        return Some(found);
                    }
                    continue;
                }
                let stem = path.file_stem().and_then(|stem| stem.to_str())?;
                if stem == sample_name {
                    return Some(path);
                }
            }
            None
        }

        walk(Path::new("samples"), sample_name)
    }

    /// Resolve a saved Convolution Reverb IR reference to an absolute path: the
    /// bundled default, otherwise a sample looked up by stem under `samples/`.
    fn resolve_conv_reverb_ir_path(&self, ir_ref: &str) -> Option<PathBuf> {
        if ir_ref == crate::conv_reverb::DEFAULT_IR_REF {
            crate::conv_reverb::default_ir_path()
        } else {
            self.resolve_sample_path_by_name(ir_ref)
        }
    }

    /// Re-apply a saved IR to a freshly-created track Convolution Reverb. The
    /// default IR is loaded on create, so only a non-default reference needs work.
    fn restore_conv_reverb_ir_track(
        &mut self,
        track: usize,
        slot_idx: usize,
        ir_ref: Option<&str>,
    ) {
        let Some(ir_ref) = ir_ref else { return };
        if ir_ref.is_empty() || ir_ref == crate::conv_reverb::DEFAULT_IR_REF {
            return;
        }
        if let Some(path) = self.resolve_conv_reverb_ir_path(ir_ref) {
            if let Err(e) = self.set_conv_reverb_ir(track, slot_idx, &path, ir_ref) {
                eprintln!("project-load: conv reverb IR '{ir_ref}' not restored: {e}");
            }
        } else {
            eprintln!("project-load: conv reverb IR '{ir_ref}' could not be resolved");
        }
    }

    /// Bus counterpart of `restore_conv_reverb_ir_track`.
    fn restore_conv_reverb_ir_bus(
        &mut self,
        bus_idx: usize,
        slot_idx: usize,
        ir_ref: Option<&str>,
    ) {
        let Some(ir_ref) = ir_ref else { return };
        if ir_ref.is_empty() || ir_ref == crate::conv_reverb::DEFAULT_IR_REF {
            return;
        }
        if let Some(path) = self.resolve_conv_reverb_ir_path(ir_ref) {
            if let Err(e) = self.set_conv_reverb_ir_bus(bus_idx, slot_idx, &path, ir_ref) {
                eprintln!("project-load: conv reverb bus IR '{ir_ref}' not restored: {e}");
            }
        } else {
            eprintln!("project-load: conv reverb bus IR '{ir_ref}' could not be resolved");
        }
    }

    pub(super) fn advance_project_load(&mut self) -> Result<(), String> {
        let Some(mut pending) = self.editor.pending_project_load.take() else {
            return Ok(());
        };
        pending.tick += 1;

        match pending.phase {
            super::PendingProjectLoadPhase::ClearExisting => {
                eprintln!(
                    "project-load: tick={} phase=ClearExisting existing_tracks={}",
                    pending.tick,
                    self.tracks.len()
                );
                {
                    let mut graph = self.graph_controller();
                    graph.clear_all_tracks();
                    graph.clear_all_bus_effect_chains();
                }
                pending.phase = super::PendingProjectLoadPhase::AddTrack(0);
            }
            super::PendingProjectLoadPhase::AddTrack(track_idx) => {
                eprintln!(
                    "project-load: tick={} phase=AddTrack index={} total={}",
                    pending.tick,
                    track_idx,
                    pending.project.tracks.len()
                );
                if track_idx >= pending.project.tracks.len() {
                    pending.phase = super::PendingProjectLoadPhase::AddEffect {
                        track_idx: 0,
                        offset: 0,
                    };
                } else {
                    let saved_color = pending.project.tracks[track_idx].color();
                    let saved_collapsed = pending.project.tracks[track_idx].collapsed();
                    match &pending.project.tracks[track_idx] {
                        ProjectTrack::Sampler { sample_path, .. } => {
                            eprintln!(
                                "project-load: add sampler track index={} path={}",
                                track_idx, sample_path
                            );
                            self.graph_controller()
                                .add_track(Path::new(sample_path))
                                .map_err(|error| {
                                    format!("Failed to load sample '{}': {error}", sample_path)
                                })?;
                        }
                        ProjectTrack::Custom {
                            instrument_name, ..
                        } => {
                            eprintln!(
                                "project-load: add custom track index={} instrument={}",
                                track_idx, instrument_name
                            );
                            self.add_saved_instrument_track_sync(instrument_name)?;
                        }
                        ProjectTrack::Modulator { .. } => {
                            eprintln!("project-load: add modulator track index={track_idx}");
                            self.graph_controller().add_modulator_track()?;
                        }
                        ProjectTrack::Rack { routing, slots, .. } => {
                            eprintln!(
                                "project-load: add rack track index={} slots={}",
                                track_idx,
                                slots.len()
                            );
                            enum PreparedRackSlotSource {
                                Sampler(RackSamplerBuildSpec),
                                Custom(usize),
                            }
                            let current_pattern_idx = pending
                                .project
                                .current_pattern
                                .min(pending.project.patterns.len().saturating_sub(1));
                            let rack_pattern = pending
                                .project
                                .patterns
                                .get(current_pattern_idx)
                                .and_then(|pattern| pattern.rack_tracks.get(track_idx))
                                .and_then(|rack| rack.as_ref())
                                .cloned();
                            let mut prepared_customs = Vec::new();
                            let mut prepared_sources = Vec::with_capacity(slots.len());
                            for (slot_idx, slot) in slots.iter().enumerate() {
                                match slot.instrument_type {
                                    crate::project::ProjectInstrumentType::Sampler => {
                                        let saved_pattern_slot = rack_pattern
                                            .as_ref()
                                            .and_then(|rack| rack.slots.get(slot_idx));
                                        let sample_path = slot
                                            .sample_path
                                            .as_ref()
                                            .or_else(|| {
                                                saved_pattern_slot
                                                    .and_then(|slot| slot.sample_path.as_ref())
                                            })
                                            .ok_or_else(|| {
                                                format!(
                                                    "Rack track {} slot {} is a sampler but has no sample_path",
                                                    track_idx + 1,
                                                    slot_idx + 1
                                                )
                                            })?;
                                        let loaded = crate::sampler::load_wav_buffer(
                                            self.graph.lg.0,
                                            Path::new(sample_path),
                                        )
                                        .map_err(|error| {
                                            format!(
                                                "Failed to load rack sample '{}' for track {} slot {}: {}",
                                                sample_path,
                                                track_idx + 1,
                                                slot_idx + 1,
                                                error
                                            )
                                        })?;
                                        self.submit_sample_analysis(&loaded);
                                        let sample_name = slot
                                            .sample_name
                                            .clone()
                                            .or_else(|| {
                                                saved_pattern_slot
                                                    .and_then(|slot| slot.sample_name.clone())
                                            })
                                            .unwrap_or_else(|| loaded.name.clone());
                                        self.register_loaded_sample_path(
                                            &sample_name,
                                            loaded.buffer_id,
                                            PathBuf::from(sample_path),
                                        );
                                        prepared_sources.push(PreparedRackSlotSource::Sampler(
                                            RackSamplerBuildSpec {
                                                buffer_id: loaded.buffer_id,
                                                sample_rate: loaded.sample_rate,
                                                sample_name,
                                            },
                                        ));
                                    }
                                    crate::project::ProjectInstrumentType::Custom => {
                                        let instrument_name = slot
                                            .instrument_name
                                            .as_deref()
                                            .ok_or_else(|| {
                                                format!(
                                                    "Rack track {} slot {} is custom but has no instrument_name",
                                                    track_idx + 1,
                                                    slot_idx + 1
                                                )
                                            })?;
                                        let prepared = self
                                            .prepare_saved_instrument_for_rack_slot_sync(
                                                instrument_name,
                                            )?;
                                        prepared_customs.push(prepared);
                                        prepared_sources.push(PreparedRackSlotSource::Custom(
                                            prepared_customs.len() - 1,
                                        ));
                                    }
                                    crate::project::ProjectInstrumentType::Modulator
                                    | crate::project::ProjectInstrumentType::Rack => {
                                        return Err(format!(
                                            "Rack track {} slot {} has unsupported instrument type",
                                            track_idx + 1,
                                            slot_idx + 1
                                        ));
                                    }
                                }
                            }
                            let mut build_specs = Vec::with_capacity(prepared_sources.len());
                            for (slot_idx, source) in prepared_sources.iter().enumerate() {
                                let saved_slot = rack_pattern
                                    .as_ref()
                                    .and_then(|rack| rack.slots.get(slot_idx))
                                    .cloned()
                                    .map(crate::sequencer::RackSlotSnapshot::from);
                                let instrument = match source {
                                    PreparedRackSlotSource::Sampler(sampler) => {
                                        RackSlotInstrumentBuildSpec::Sampler(sampler.clone())
                                    }
                                    PreparedRackSlotSource::Custom(prepared_idx) => {
                                        let prepared = &prepared_customs[*prepared_idx];
                                        let lib_ptr: *const lisp_host::LoadedDGenLib =
                                            &self.editor.instrument_libs[prepared.lib_index];
                                        RackSlotInstrumentBuildSpec::Custom(RackCustomBuildSpec {
                                            instrument_name: &prepared.name,
                                            engine_id: prepared.engine_id,
                                            manifest: &prepared.manifest,
                                            lib: unsafe { &*lib_ptr },
                                            run_mode: prepared.run_mode,
                                        })
                                    }
                                };
                                build_specs.push(RackSlotBuildSpec {
                                    instrument,
                                    instrument_base_note_offset: saved_slot
                                        .as_ref()
                                        .map(|slot| slot.instrument_base_note_offset)
                                        .unwrap_or(0.0),
                                    pad_note: saved_slot.as_ref().and_then(|slot| slot.pad_note),
                                    choke_group: saved_slot
                                        .as_ref()
                                        .and_then(|slot| slot.choke_group),
                                    gain: saved_slot.as_ref().map(|slot| slot.gain).unwrap_or(1.0),
                                    pan: saved_slot.as_ref().map(|slot| slot.pan).unwrap_or(0.0),
                                    mute: saved_slot
                                        .as_ref()
                                        .map(|slot| slot.mute)
                                        .unwrap_or(false),
                                    solo: saved_slot
                                        .as_ref()
                                        .map(|slot| slot.solo)
                                        .unwrap_or(false),
                                    max_polyphony: saved_slot
                                        .as_ref()
                                        .map(|slot| slot.max_polyphony)
                                        .unwrap_or(crate::voice::MAX_VOICES),
                                    param_plocks: saved_slot
                                        .as_ref()
                                        .map(|slot| slot.param_plocks.clone()),
                                    instrument_slot: saved_slot
                                        .as_ref()
                                        .map(|slot| slot.instrument_slot.clone()),
                                    track_sound_state: saved_slot
                                        .as_ref()
                                        .map(|slot| slot.track_sound_state.clone()),
                                });
                            }
                            let routing = RackRouting::from(*routing);
                            let rack_name = match routing {
                                RackRouting::Broadcast => "Layer Rack",
                                RackRouting::ByPitch => "Drum Rack",
                            };
                            self.graph_controller().add_rack_track(
                                rack_name,
                                routing,
                                build_specs,
                            )?;
                        }
                    }
                    if let Some(color) = saved_color {
                        self.set_track_color(track_idx, color);
                    }
                    self.set_track_collapsed(track_idx, saved_collapsed);
                    pending.phase = super::PendingProjectLoadPhase::AddTrack(track_idx + 1);
                }
            }
            super::PendingProjectLoadPhase::AddEffect { track_idx, offset } => {
                eprintln!(
                    "project-load: tick={} phase=AddEffect track={} offset={}",
                    pending.tick, track_idx, offset
                );
                if track_idx >= pending.project.custom_effects.len() {
                    pending.phase = super::PendingProjectLoadPhase::BuildPattern(0);
                } else if offset >= pending.project.custom_effects[track_idx].len() {
                    pending.phase = super::PendingProjectLoadPhase::AddEffect {
                        track_idx: track_idx + 1,
                        offset: 0,
                    };
                } else {
                    if let Some(effect_name) = pending.project.custom_effects[track_idx][offset]
                        .as_ref()
                        .map(|name| name.trim())
                        .filter(|name| !name.is_empty())
                    {
                        eprintln!(
                            "project-load: load effect track={} slot={} name={}",
                            track_idx,
                            BUILTIN_SLOT_COUNT + offset,
                            effect_name
                        );
                        if let Some(builtin_name) =
                            project_builtin_effect_name_for_load(effect_name)
                        {
                            self.load_builtin_effect_to_slot_sync(
                                track_idx,
                                BUILTIN_SLOT_COUNT + offset,
                                &builtin_name,
                            )?;
                        } else {
                            self.load_saved_effect_to_slot_sync(
                                track_idx,
                                BUILTIN_SLOT_COUNT + offset,
                                effect_name,
                            )?;
                        }
                        // Restore a saved Convolution Reverb IR for this slot. The
                        // ref is the same across patterns (instance state), so take
                        // the first pattern that recorded one.
                        let saved_ir = pending.project.patterns.iter().find_map(|p| {
                            p.effect_slots
                                .get(track_idx)
                                .and_then(|slots| slots.get(offset))
                                .and_then(|slot| slot.ir.clone())
                        });
                        self.restore_conv_reverb_ir_track(
                            track_idx,
                            BUILTIN_SLOT_COUNT + offset,
                            saved_ir.as_deref(),
                        );
                    }
                    pending.phase = super::PendingProjectLoadPhase::AddEffect {
                        track_idx,
                        offset: offset + 1,
                    };
                }
            }
            super::PendingProjectLoadPhase::BuildPattern(pattern_idx) => {
                eprintln!(
                    "project-load: tick={} phase=BuildPattern index={} total={}",
                    pending.tick,
                    pattern_idx,
                    pending.project.patterns.len()
                );
                if pattern_idx >= pending.project.patterns.len() {
                    pending.phase = super::PendingProjectLoadPhase::Finalize;
                } else {
                    let (snapshot, bus_patterns, fallback_count) = self
                        .project_pattern_into_snapshot(
                            pending.project.patterns[pattern_idx].clone(),
                        )?;
                    pending.built_patterns.push(snapshot);
                    pending.built_bus_patterns.push(bus_patterns);
                    pending.fallback_samples += fallback_count;
                    pending.phase = super::PendingProjectLoadPhase::BuildPattern(pattern_idx + 1);
                }
            }
            super::PendingProjectLoadPhase::Finalize => {
                eprintln!(
                    "project-load: tick={} phase=Finalize built_patterns={} fallback_samples={}",
                    pending.tick,
                    pending.built_patterns.len(),
                    pending.fallback_samples
                );
                self.finish_project_load(pending)?;
                return Ok(());
            }
        }

        self.editor.pending_project_load = Some(pending);
        Ok(())
    }

    fn finish_project_load(&mut self, pending: super::PendingProjectLoad) -> Result<(), String> {
        eprintln!(
            "project-load: finish start name={} tracks={} built_patterns={} fallback_samples={}",
            pending.name,
            self.tracks.len(),
            pending.built_patterns.len(),
            pending.fallback_samples
        );
        let ProjectFile {
            version: _,
            name: _,
            bpm,
            master_volume,
            current_pattern: saved_current_pattern,
            current_track: saved_current_track,
            reverb,
            buses,
            scratch,
            tracks: _,
            custom_effects: _,
            patterns: _,
            groups,
        } = pending.project;
        let bank = pending.built_patterns;
        let bus_pattern_bank = pending.built_bus_patterns;
        let current_pattern = saved_current_pattern.min(bank.len().saturating_sub(1));
        let current_track = resolve_project_current_track(
            saved_current_track,
            self.tracks.len(),
            bank.get(current_pattern)
                .map(|snapshot| snapshot.track_bits.as_slice()),
        );
        self.normalize_track_colors();
        self.normalize_track_collapsed();

        let pattern_repository = if bank.is_empty() {
            vec![PatternSnapshot::new_default(
                self.tracks.len(),
                &self.graph.effect_descriptors,
            )]
        } else {
            bank
        };
        self.state
            .replace_pattern_repository(pattern_repository, current_pattern);
        self.state
            .transport
            .pattern_epoch
            .fetch_add(1, Ordering::Relaxed);
        self.state.transport.bpm.store(bpm, Ordering::Relaxed);
        self.state
            .transport
            .master_volume
            .store(master_volume.clamp(0.0, 2.0).to_bits(), Ordering::Relaxed);
        self.buses = if buses.is_empty() {
            BusChannelState::default_buses()
        } else {
            buses.into_iter().map(BusChannelState::from).collect()
        };
        if !self.buses.iter().any(|bus| bus.id == BusId::MIX) {
            self.buses
                .insert(0, BusChannelState::new(BusId::MIX, "Mix"));
        }
        for bus in self.buses.clone() {
            self.graph_controller()
                .ensure_bus_graph_node(bus.id, &bus.name);
        }
        // Defensively drop dangling groups: every backing bus must resolve and
        // every member index must be in range (track count is known here).
        let group_track_count = self.tracks.len();
        self.groups = groups
            .into_iter()
            .filter(|group| {
                self.buses.iter().any(|bus| bus.id.0 == group.bus_id)
                    && !group.members.is_empty()
                    && group.members.iter().all(|&m| m < group_track_count)
            })
            .collect();
        // Reconcile group routing: a group's members must reach its backing bus
        // in every scene. Output is stored per-scene, so older saves (or any
        // pre-fix grouping) can have members still pointing at Mix in some/all
        // scenes — repair that here so the group actually submixes on load.
        for group in self.groups.clone() {
            let output = TrackOutput::Bus(BusId(group.bus_id));
            for &member in &group.members {
                self.set_track_output_all_scenes(member, output.clone());
            }
        }
        let saved_bus_effects: Vec<(usize, usize, String, crate::effects::EffectSlotSnapshot)> =
            self.buses
                .iter()
                .enumerate()
                .flat_map(|(bus_idx, bus)| {
                    bus.custom_effect_names.iter().enumerate().filter_map(
                        move |(slot_idx, name)| {
                            let name = name.as_ref()?.trim();
                            if name.is_empty() {
                                return None;
                            }
                            let slot = bus
                                .effect_slots
                                .get(slot_idx)
                                .cloned()
                                .unwrap_or_else(crate::effects::EffectSlotSnapshot::new_empty);
                            Some((bus_idx, slot_idx, name.to_string(), slot))
                        },
                    )
                })
                .collect();
        for (bus_idx, slot_idx, name, saved_slot) in saved_bus_effects {
            let saved_ir = saved_slot.ir.clone();
            if let Some(builtin_name) = project_builtin_effect_name_for_load(&name) {
                self.load_builtin_bus_effect_to_slot_sync(bus_idx, slot_idx, &builtin_name)?;
            } else {
                self.load_bus_effect_to_slot_sync(bus_idx, slot_idx, &name)?;
            }
            if let Some(slot) = self
                .buses
                .get_mut(bus_idx)
                .and_then(|bus| bus.effect_slots.get_mut(slot_idx))
            {
                restore_saved_bus_effect_slot_runtime_ids(slot, saved_slot);
            }
            self.push_bus_effect_slot_defaults(bus_idx, slot_idx);
            // Restore a saved Convolution Reverb IR (the default was auto-loaded
            // on create, so only override for a non-default reference).
            self.restore_conv_reverb_ir_bus(bus_idx, slot_idx, saved_ir.as_deref());
        }
        let default_bus_snapshot = self.capture_bus_pattern_snapshot();
        self.state
            .replace_bus_pattern_repository(bus_pattern_bank, &default_bus_snapshot);
        let current_bus_snapshot = self
            .state
            .bus_pattern_snapshot_or_default(current_pattern, &default_bus_snapshot);
        self.restore_bus_pattern_snapshot(&current_bus_snapshot);
        for bus in &self.buses {
            let Some(nodes) = self
                .graph
                .bus_node_ids
                .iter()
                .find(|nodes| nodes.id == bus.id)
            else {
                continue;
            };
            unsafe {
                crate::audiograph::params_push_wrapper(
                    self.graph.lg.0,
                    crate::audiograph::ParamMsg {
                        idx: crate::stereo_panner::STEREO_PANNER_PARAM_VOLUME,
                        logical_id: nodes.volume_id as u64,
                        fvalue: crate::mixer_volume::fader_to_gain(bus.volume),
                    },
                );
                crate::audiograph::params_push_wrapper(
                    self.graph.lg.0,
                    crate::audiograph::ParamMsg {
                        idx: crate::stereo_panner::STEREO_PANNER_PARAM_MUTE,
                        logical_id: nodes.volume_id as u64,
                        fvalue: if bus.mute { 1.0 } else { 0.0 },
                    },
                );
            }
        }
        self.publish_bus_gate_runtime();

        self.ui.cursor_track = current_track;
        self.ui.cursor_step = 0;
        self.ui.pattern_page = current_pattern / 10;
        self.ui.focused_region = if self.tracks.is_empty() {
            Region::Sidebar
        } else {
            Region::Cirklon
        };
        self.ui.sidebar_tab = if self.tracks.is_empty() {
            SidebarTab::Sounds
        } else {
            SidebarTab::Tools
        };
        self.ui.sidebar_mode = if self.tracks.is_empty() {
            SidebarMode::InstrumentPicker
        } else {
            self.effective_sidebar_mode()
        };

        let current_sample_ids = self
            .state
            .restore_current_pattern_from_repository()
            .unwrap_or_default();
        {
            let mut graph = self.graph_controller();
            graph.sync_track_instrument_run_modes_from_live_state()?;
        }
        eprintln!(
            "project-load: restored current_pattern={} sample_ids={} graph_tracks={}",
            current_pattern,
            current_sample_ids.len(),
            self.graph.track_node_ids.len()
        );
        self.state.publish_scheduler_snapshot();
        {
            let mut graph = self.graph_controller();
            graph.apply_sample_ids(&current_sample_ids);
            graph.sync_current_pattern_mod_routes();
        }
        {
            let track_count = self.tracks.len();
            let mut graph = self.graph_controller();
            for track_idx in 0..track_count {
                graph.apply_track_output_routing(track_idx);
                graph.apply_track_bus_sends(track_idx);
            }
        }
        self.set_reverb_param(0, reverb.size);
        self.set_reverb_param(1, reverb.brightness);
        self.set_reverb_param(2, reverb.replace);
        self.push_all_restored_defaults();
        self.push_all_delay_bpm();

        if !self.tracks.is_empty() {
            self.clamp_cursor_to_steps();
            self.browser.sync_to_track(
                &self.tracks,
                self.ui.cursor_track,
                self.is_sampler_track(self.ui.cursor_track),
                &self.ui,
            );
        }

        self.current_project_name = Some(pending.name.clone());
        self.editor.scratch_buffer = scratch.buffer;
        self.editor.scratch_cursor = (scratch.cursor_row, scratch.cursor_col);
        self.editor.scratch_runtime = None;
        self.state
            .set_scratch_source(self.editor.scratch_buffer.clone());
        self.clear_control_hooks();
        let repaired_sidechains = self.repair_stale_sidechain_effect_slots()?;
        let status = if pending.fallback_samples > 0 {
            format!(
                "Opened project '{}' with {} fallback sample{}",
                pending.name,
                pending.fallback_samples,
                if pending.fallback_samples == 1 {
                    ""
                } else {
                    "s"
                }
            )
        } else {
            format!("Opened project '{}'", pending.name)
        };
        let status = if repaired_sidechains > 0 {
            format!("{status}; repaired {repaired_sidechains} sidechain effect route")
        } else {
            status
        };
        eprintln!("project-load: finish complete status={status}");
        self.editor.status_message = Some((status, Instant::now()));
        self.editor.pending_project_load = None;
        Ok(())
    }

    fn rebind_project_rack_tracks_to_graph(
        &self,
        rack_tracks: Vec<Option<crate::project::ProjectRackTrackPattern>>,
        num_tracks: usize,
    ) -> Vec<Option<RackTrackSnapshot>> {
        let live_rack_tracks = self.state.pattern.rack_tracks.lock().unwrap();
        (0..num_tracks)
            .map(|track_idx| {
                if self.graph.track_instrument_types.get(track_idx) != Some(&InstrumentType::Rack) {
                    return None;
                }
                let graph_rack = live_rack_tracks.get(track_idx).cloned().flatten()?;
                let mut saved_rack = rack_tracks
                    .get(track_idx)
                    .cloned()
                    .flatten()
                    .map(RackTrackSnapshot::from)
                    .unwrap_or_else(|| graph_rack.clone());
                saved_rack.routing = graph_rack.routing;
                let mut rebound_slots = Vec::with_capacity(graph_rack.slots.len());
                for (slot_idx, graph_slot) in graph_rack.slots.iter().enumerate() {
                    let mut slot = saved_rack
                        .slots
                        .get(slot_idx)
                        .cloned()
                        .unwrap_or_else(|| graph_slot.clone());
                    slot.instrument_type = graph_slot.instrument_type;
                    slot.track_sound_state.engine_id = graph_slot.track_sound_state.engine_id;
                    slot.sample_id = graph_slot.sample_id.clone();
                    match graph_slot.instrument_type {
                        InstrumentType::Sampler => {
                            let desc = EffectDescriptor::builtin_sampler();
                            slot.instrument_slot.sync_to_descriptor_with_modulator(
                                &desc,
                                graph_slot.instrument_slot.node_id,
                                graph_slot.instrument_slot.modulator_node_id,
                            );
                        }
                        InstrumentType::Custom => {
                            slot.instrument_run_mode = graph_slot.instrument_run_mode;
                            slot.instrument_slot.node_id = graph_slot.instrument_slot.node_id;
                            slot.instrument_slot.modulator_node_id =
                                graph_slot.instrument_slot.modulator_node_id;
                            slot.instrument_slot.param_node_indices =
                                graph_slot.instrument_slot.param_node_indices.clone();
                            slot.instrument_slot.param_node_spans =
                                graph_slot.instrument_slot.param_node_spans.clone();
                            slot.instrument_slot.transport_phase_param_idx =
                                graph_slot.instrument_slot.transport_phase_param_idx;
                            slot.instrument_slot.num_params = graph_slot.instrument_slot.num_params;
                            if slot.instrument_slot.defaults.len()
                                != graph_slot.instrument_slot.defaults.len()
                            {
                                slot.instrument_slot.defaults =
                                    graph_slot.instrument_slot.defaults.clone();
                            }
                            let num_params = slot.instrument_slot.num_params as usize;
                            slot.instrument_slot.plocks.resize_with(MAX_STEPS, Vec::new);
                            slot.instrument_slot
                                .plock_param_ids
                                .resize_with(MAX_STEPS, Vec::new);
                            for step in 0..MAX_STEPS {
                                slot.instrument_slot.plocks[step].resize(num_params, None);
                                slot.instrument_slot.plock_param_ids[step].resize(num_params, None);
                                for param_idx in 0..num_params {
                                    if slot.instrument_slot.plocks[step][param_idx].is_none() {
                                        slot.instrument_slot.plock_param_ids[step][param_idx] =
                                            None;
                                        continue;
                                    }
                                    let raw_idx = slot
                                        .instrument_slot
                                        .param_node_indices
                                        .get(param_idx)
                                        .copied()
                                        .unwrap_or(param_idx as u32);
                                    slot.instrument_slot.plock_param_ids[step][param_idx] =
                                        ParamNodeId::from_slot_param(
                                            slot.instrument_slot.node_id,
                                            slot.instrument_slot.modulator_node_id,
                                            raw_idx,
                                        );
                                }
                            }
                        }
                        InstrumentType::Modulator | InstrumentType::Rack => {}
                    }
                    rebound_slots.push(slot);
                }
                saved_rack.slots = rebound_slots;
                Some(saved_rack)
            })
            .collect()
    }

    fn project_pattern_into_snapshot(
        &mut self,
        pattern: ProjectPattern,
    ) -> Result<(PatternSnapshot, Vec<BusPatternSnapshot>, usize), String> {
        let num_tracks = self.tracks.len();
        let mut sample_ids = Vec::with_capacity(num_tracks);
        let mut fallback_count = 0;
        for track_idx in 0..num_tracks {
            if self.is_sampler_track(track_idx) {
                let saved_path = pattern
                    .sample_paths
                    .get(track_idx)
                    .and_then(|path| path.as_ref())
                    .map(PathBuf::from);
                let saved_name = pattern
                    .sample_names
                    .get(track_idx)
                    .cloned()
                    .unwrap_or_default();

                let resolved_path = saved_path
                    .as_ref()
                    .filter(|path| path.exists())
                    .cloned()
                    .or_else(|| {
                        if saved_name.is_empty() {
                            None
                        } else {
                            self.sample_path_registry
                                .get(&saved_name)
                                .cloned()
                                .or_else(|| self.resolve_sample_path_by_name(&saved_name))
                        }
                    })
                    .or_else(|| self.first_available_sample_path());

                let Some(path_buf) = resolved_path else {
                    return Err(format!(
                        "Couldn't recover sample for track {} and no fallback samples exist",
                        track_idx + 1
                    ));
                };
                let loaded = crate::sampler::load_wav_buffer(self.graph.lg.0, &path_buf).map_err(
                    |error| {
                        format!(
                            "Failed to load sample '{}' for track {}: {}",
                            path_buf.display(),
                            track_idx + 1,
                            error
                        )
                    },
                )?;
                self.submit_sample_analysis(&loaded);
                let buffer_id = loaded.buffer_id;
                let sample_rate = loaded.sample_rate;
                let sample_name = crate::sample_db::display_title_for_sample_path(&path_buf)
                    .or_else(|| {
                        let saved_name = saved_name.trim();
                        (!saved_name.is_empty()).then(|| saved_name.to_string())
                    })
                    .unwrap_or(loaded.name);
                if saved_path.as_ref() != Some(&path_buf) {
                    fallback_count += 1;
                }
                self.register_loaded_sample_path(&sample_name, buffer_id, path_buf);
                sample_ids.push((buffer_id, sample_name, sample_rate));
            } else {
                sample_ids.push((-1, String::new(), self.graph.sample_rate));
            }
        }

        let crate::project::ProjectPattern {
            track_bits,
            neural_reset_bits,
            step_data,
            track_params,
            effect_slots,
            midi_fx_slots,
            instrument_slots,
            instrument_base_note_offsets,
            track_sound_states,
            chord_snapshots,
            chord_duration_snapshots,
            chord_delay_snapshots,
            timebase_plock_snapshots,
            swing_plock_snapshots,
            swing_resolution_plock_snapshots,
            bus_patterns,
            mod_connections,
            neural_networks,
            graph_overrides,
            instrument_types: _,
            instrument_run_modes,
            sample_paths: _,
            sample_names: _,
            rack_tracks,
            plock_variant_registries,
            key_lock_variant_registries,
        } = pattern;
        let bus_patterns = bus_patterns
            .into_iter()
            .map(project_bus_pattern_snapshot_to_ui)
            .collect();

        // Pre-extract attack/release for sampler instrument slot migration
        // before track_params is consumed by into_iter().
        let sampler_attack_release: Vec<(f32, f32)> = track_params
            .iter()
            .map(|tp| (tp.attack_ms, tp.release_ms))
            .collect();
        let midi_fx_chains: Vec<Vec<String>> = track_params
            .iter()
            .map(|tp| tp.midi_fx_chain.clone())
            .collect();

        let mut snapshot = PatternSnapshot {
            track_bits,
            neural_reset_bits,
            step_data,
            track_params: track_params.into_iter().map(Into::into).collect(),
            effect_slots: effect_slots
                .into_iter()
                .enumerate()
                .map(|(track_idx, slots)| {
                    slots
                        .into_iter()
                        .enumerate()
                        .map(|(slot_idx, slot)| {
                            if let Some(desc) = self
                                .graph
                                .effect_descriptors
                                .get(track_idx)
                                .and_then(|descs| descs.get(slot_idx))
                            {
                                project_track_effect_slot_into_synced_snapshot(
                                    slot,
                                    desc,
                                    &self.state.pattern.effect_chains[track_idx][slot_idx],
                                )
                            } else {
                                let node_id = self.state.pattern.effect_chains[track_idx][slot_idx]
                                    .node_id
                                    .load(Ordering::Relaxed);
                                slot.into_snapshot_with_node_id(node_id)
                            }
                        })
                        .collect()
                })
                .collect(),
            midi_fx_slots: (0..num_tracks)
                .map(|track_idx| {
                    midi_fx_slots
                        .get(track_idx)
                        .cloned()
                        .unwrap_or_default()
                        .into_iter()
                        .enumerate()
                        .map(|(slot_idx, slot)| {
                            project_midi_fx_slot_into_synced_snapshot(
                                slot,
                                midi_fx_chains
                                    .get(track_idx)
                                    .and_then(|chain| chain.get(slot_idx))
                                    .map(String::as_str),
                            )
                        })
                        .collect()
                })
                .collect(),
            instrument_slots: (0..num_tracks)
                .map(|track_idx| {
                    let node_id = self.state.pattern.instrument_slots[track_idx]
                        .node_id
                        .load(Ordering::Relaxed);
                    let modulator_node_id = self.state.pattern.instrument_slots[track_idx]
                        .modulator_node_id
                        .load(Ordering::Relaxed);
                    if self.graph.track_instrument_types.get(track_idx)
                        == Some(&InstrumentType::Rack)
                    {
                        crate::effects::EffectSlotSnapshot::new_empty()
                    } else if self.is_sampler_track(track_idx) {
                        let saved_slot =
                            instrument_slots.get(track_idx).cloned().unwrap_or_else(|| {
                                crate::project::ProjectEffectSlot {
                                    num_params: 0,
                                    defaults: Vec::new(),
                                    plocks: vec![Vec::new(); MAX_STEPS],
                                    plock_param_ids: vec![Vec::new(); MAX_STEPS],
                                    key_locks: std::collections::BTreeMap::new(),
                                    key_lock_param_ids: std::collections::BTreeMap::new(),
                                    tensor_params: Vec::new(),
                                    param_node_indices: Vec::new(),
                                    param_node_spans: Vec::new(),
                                    ir: None,
                                }
                            });
                        if saved_slot.num_params >= 4 {
                            // Sampler params already saved; sync to pick up params added after
                            // the project was written, such as enabled.
                            let sampler_desc = crate::effects::EffectDescriptor::builtin_sampler();
                            project_slot_into_synced_snapshot_with_modulator(
                                saved_slot,
                                &sampler_desc,
                                node_id,
                                modulator_node_id,
                            )
                        } else {
                            // Old project: migrate attack/release from TrackParams
                            let sampler_desc = crate::effects::EffectDescriptor::builtin_sampler();
                            let mut defaults: Vec<f32> =
                                sampler_desc.params.iter().map(|p| p.default).collect();
                            let (attack, release) = sampler_attack_release
                                .get(track_idx)
                                .copied()
                                .unwrap_or((0.0, 0.0));
                            defaults[0] = attack;
                            defaults[1] = release;
                            // start=0.0, end=1.0 already set from defaults
                            crate::effects::EffectSlotSnapshot {
                                node_id,
                                modulator_node_id,
                                num_params: defaults.len() as u32,
                                defaults,
                                plocks: vec![Vec::new(); MAX_STEPS],
                                plock_param_ids: vec![Vec::new(); MAX_STEPS],
                                key_locks: std::collections::BTreeMap::new(),
                                key_lock_param_ids: std::collections::BTreeMap::new(),
                                tensor_params: Vec::new(),
                                param_node_indices: sampler_desc
                                    .params
                                    .iter()
                                    .map(|p| p.node_param_idx)
                                    .collect(),
                                param_node_spans: sampler_desc
                                    .params
                                    .iter()
                                    .map(|p| p.node_param_span.max(1))
                                    .collect(),
                                transport_phase_param_idx: sampler_desc
                                    .transport_phase_param_idx()
                                    .unwrap_or(crate::effects::NO_TRANSPORT_PHASE_PARAM),
                                ir: None,
                            }
                        }
                    } else {
                        let desc = self.graph.instrument_descriptors[track_idx].clone();
                        let slot = instrument_slots.get(track_idx).cloned().unwrap_or_else(|| {
                            crate::project::ProjectEffectSlot {
                                num_params: 0,
                                defaults: Vec::new(),
                                plocks: vec![Vec::new(); MAX_STEPS],
                                plock_param_ids: vec![Vec::new(); MAX_STEPS],
                                key_locks: std::collections::BTreeMap::new(),
                                key_lock_param_ids: std::collections::BTreeMap::new(),
                                tensor_params: Vec::new(),
                                param_node_indices: Vec::new(),
                                param_node_spans: Vec::new(),
                                ir: None,
                            }
                        });
                        if slot.num_params == 0 {
                            crate::effects::EffectSlotSnapshot::capture(
                                &self.state.pattern.instrument_slots[track_idx],
                            )
                        } else {
                            // Sync against the current descriptor so old projects inherit params
                            // added after save, while remapping custom-instrument params across
                            // the inserted enabled slot.
                            project_custom_instrument_slot_into_synced_snapshot(
                                slot,
                                &desc,
                                node_id,
                                modulator_node_id,
                            )
                        }
                    }
                })
                .collect(),
            instrument_base_note_offsets,
            track_sound_states: track_sound_states
                .into_iter()
                .enumerate()
                .map(|(track_idx, sound)| {
                    let engine_id = self
                        .graph
                        .track_engine_ids
                        .get(track_idx)
                        .and_then(|id| *id);
                    sound.into_track_sound_state(engine_id)
                })
                .collect(),
            sample_ids,
            chord_snapshots: chord_snapshots
                .into_iter()
                .zip(
                    chord_duration_snapshots
                        .into_iter()
                        .chain(std::iter::repeat_with(|| vec![Vec::new(); MAX_STEPS])),
                )
                .zip(
                    chord_delay_snapshots
                        .into_iter()
                        .chain(std::iter::repeat_with(|| vec![Vec::new(); MAX_STEPS])),
                )
                .map(|((steps, durations), delays)| {
                    chord_snapshot_from_steps_durations_and_delays(steps, durations, delays)
                })
                .collect(),
            timebase_plock_snapshots: timebase_plock_snapshots
                .into_iter()
                .map(|steps| {
                    let mut snapshot = [None; MAX_STEPS];
                    for (idx, value) in steps.into_iter().take(MAX_STEPS).enumerate() {
                        snapshot[idx] = value;
                    }
                    snapshot
                })
                .collect(),
            swing_plock_snapshots: swing_plock_snapshots
                .into_iter()
                .map(|steps| {
                    let mut snapshot = [None; MAX_STEPS];
                    for (idx, value) in steps.into_iter().take(MAX_STEPS).enumerate() {
                        snapshot[idx] = value;
                    }
                    snapshot
                })
                .collect(),
            swing_resolution_plock_snapshots: swing_resolution_plock_snapshots
                .into_iter()
                .map(|steps| {
                    let mut snapshot = [None; MAX_STEPS];
                    for (idx, value) in steps.into_iter().take(MAX_STEPS).enumerate() {
                        snapshot[idx] = value;
                    }
                    snapshot
                })
                .collect(),
            instrument_types: self.graph.track_instrument_types.clone(),
            instrument_run_modes: instrument_run_modes
                .into_iter()
                .map(CustomInstrumentRunMode::from)
                .collect(),
            mod_connections: mod_connections.into_iter().map(Into::into).collect(),
            neural_networks,
            graph_overrides,
            rack_tracks: self.rebind_project_rack_tracks_to_graph(rack_tracks, num_tracks),
            plock_variant_registries,
            key_lock_variant_registries,
        };
        snapshot.normalize_track_count(num_tracks, &self.graph.effect_descriptors);
        refresh_neural_output_override_param_ids(&mut snapshot);

        Ok((snapshot, bus_patterns, fallback_count))
    }

    fn first_available_sample_path(&self) -> Option<PathBuf> {
        fn walk(dir: &Path) -> Option<PathBuf> {
            let mut entries: Vec<_> = std::fs::read_dir(dir)
                .ok()?
                .filter_map(Result::ok)
                .collect();
            entries.sort_by_key(|entry| entry.file_name());
            for entry in entries {
                let path = entry.path();
                if path.is_dir() {
                    if let Some(found) = walk(&path) {
                        return Some(found);
                    }
                } else if path
                    .extension()
                    .map(|ext| ext.to_ascii_lowercase() == "wav")
                    .unwrap_or(false)
                {
                    return Some(path);
                }
            }
            None
        }

        walk(Path::new("samples"))
    }

    pub fn push_all_restored_defaults(&self) {
        self.push_master_volume();
        for track_idx in 0..self.tracks.len() {
            self.push_track_volume(track_idx);
            self.push_track_pan(track_idx);
            self.push_track_mute(track_idx);
            self.push_send_gain(track_idx);
            for slot_idx in 0..self.state.pattern.effect_chains[track_idx].len() {
                let slot = &self.state.pattern.effect_chains[track_idx][slot_idx];
                let num_params = slot.num_params.load(Ordering::Relaxed) as usize;
                for param_idx in 0..num_params {
                    let value = slot.defaults.get(param_idx);
                    let host_control = self
                        .graph
                        .effect_descriptors
                        .get(track_idx)
                        .and_then(|slots| slots.get(slot_idx))
                        .and_then(|desc| desc.params.get(param_idx))
                        .and_then(|param| param.host_control.as_ref());
                    if matches!(
                        host_control,
                        Some(crate::effects::HostControl::FxSidechain { .. })
                    ) {
                        self.apply_effect_sidechain_selection(
                            track_idx,
                            slot_idx,
                            param_idx,
                            value.round().max(0.0) as usize,
                        );
                    } else {
                        self.send_slot_param(track_idx, slot_idx, param_idx, value);
                    }
                }
            }
        }
        self.push_track_solo_mutes();
        self.push_all_restored_instrument_defaults();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_param(
        name: &str,
        default: f32,
        node_param_idx: u32,
    ) -> crate::effects::ParamDescriptor {
        crate::effects::ParamDescriptor {
            name: name.to_string(),
            min: 0.0,
            max: 1.0,
            default,
            kind: crate::effects::ParamKind::Continuous { unit: None },
            scaling: crate::effects::ParamScaling::Linear,
            node_param_idx,
            node_param_span: 1,
            host_control: None,
            ui_metadata: None,
        }
    }

    #[test]
    fn missing_project_current_track_uses_first_non_empty_track() {
        let mut track_bits = vec![[0; TRACK_PATTERN_WORDS]; 3];
        track_bits[2][0] = 1;

        assert_eq!(resolve_project_current_track(None, 3, Some(&track_bits)), 2);
        assert_eq!(
            resolve_project_current_track(Some(1), 3, Some(&track_bits)),
            1
        );
        assert_eq!(
            resolve_project_current_track(Some(99), 3, Some(&track_bits)),
            2
        );
        assert_eq!(resolve_project_current_track(None, 3, None), 0);
        assert_eq!(resolve_project_current_track(None, 0, Some(&track_bits)), 0);
    }

    #[test]
    fn project_restore_syncs_midi_fx_slot_to_chain_descriptor() {
        let saved_slot = project::ProjectEffectSlot {
            num_params: 1,
            defaults: vec![5.0],
            plocks: (0..MAX_STEPS).map(|_| vec![None]).collect(),
            plock_param_ids: (0..MAX_STEPS).map(|_| vec![None]).collect(),
            key_locks: std::collections::BTreeMap::new(),
            key_lock_param_ids: std::collections::BTreeMap::new(),
            param_node_indices: vec![0],
            param_node_spans: vec![1],
            tensor_params: Vec::new(),
            ir: None,
        };

        let restored =
            project_midi_fx_slot_into_synced_snapshot(saved_slot, Some("trigger-to-track"));

        assert_eq!(restored.num_params, 2);
        assert_eq!(restored.defaults[0], 5.0);
        assert_eq!(restored.defaults[1], 1.0);
        assert_eq!(restored.param_node_indices, vec![0, 1]);
        assert_eq!(restored.param_node_spans, vec![1, 1]);
    }

    #[test]
    fn project_restore_preserves_track_effect_modulator_node_id() {
        let desc = EffectDescriptor::builtin_filter();
        let mut saved_slot = default_project_effect_slot(&desc);
        let source_idx = desc
            .params
            .iter()
            .position(|param| param.name == "mod1_source")
            .expect("filter should expose Mod 1 source");
        let depth_idx = desc
            .params
            .iter()
            .position(|param| param.name == "mod cutoff slot 1 amt")
            .expect("filter should expose Mod 1 cutoff depth");
        saved_slot.defaults[source_idx] = 5.0;
        saved_slot.defaults[depth_idx] = -2.0;

        let live_slot = crate::effects::EffectSlotState::new(&desc, 101);
        live_slot.apply_descriptor_with_modulator(&desc, 101, 202);

        let restored =
            project_track_effect_slot_into_synced_snapshot(saved_slot, &desc, &live_slot);

        assert_eq!(restored.node_id, 101);
        assert_eq!(restored.modulator_node_id, 202);
        assert_eq!(restored.defaults[source_idx], 5.0);
        assert_eq!(restored.defaults[depth_idx], -2.0);
        assert_eq!(
            ParamNodeId::from_slot_param(
                restored.node_id,
                restored.modulator_node_id,
                restored.param_node_indices[source_idx],
            ),
            Some(ParamNodeId {
                logical_id: 202,
                node_param_idx: restored.param_node_indices[source_idx]
                    - crate::voice_modulator::MOD_PARAM_BASE,
            })
        );
        assert_eq!(
            ParamNodeId::from_slot_param(
                restored.node_id,
                restored.modulator_node_id,
                restored.param_node_indices[depth_idx],
            ),
            Some(ParamNodeId {
                logical_id: 101,
                node_param_idx: restored.param_node_indices[depth_idx],
            })
        );
    }

    fn test_slot_snapshot(
        node_id: u32,
        modulator_node_id: u32,
        param_node_indices: Vec<u32>,
    ) -> crate::effects::EffectSlotSnapshot {
        let num_params = param_node_indices.len();
        crate::effects::EffectSlotSnapshot {
            node_id,
            modulator_node_id,
            num_params: num_params as u32,
            defaults: vec![0.0; num_params],
            plocks: (0..MAX_STEPS).map(|_| vec![None; num_params]).collect(),
            plock_param_ids: (0..MAX_STEPS).map(|_| vec![None; num_params]).collect(),
            key_locks: std::collections::BTreeMap::new(),
            key_lock_param_ids: std::collections::BTreeMap::new(),
            param_node_indices,
            param_node_spans: vec![1; num_params],
            transport_phase_param_idx: crate::effects::NO_TRANSPORT_PHASE_PARAM,
            tensor_params: Vec::new(),
            ir: None,
        }
    }

    #[test]
    fn project_load_refreshes_neuron_plock_ids_to_live_slot_nodes() {
        let mut snapshot = PatternSnapshot::new_default(2, &[Vec::new(), Vec::new()]);
        snapshot.instrument_slots[1] = test_slot_snapshot(200, 0, vec![6, 15]);
        snapshot.effect_slots[1] = vec![test_slot_snapshot(300, 0, vec![2])];

        let mut network = crate::neural::ProjectNeuralNetwork {
            id: 1,
            num_neurons: 1,
            neurons: vec![crate::neural::ProjectNeuron::default()],
            ..crate::neural::ProjectNeuralNetwork::default()
        };
        network.neurons[0].output_overrides.instrument =
            vec![crate::neural::ProjectParamOverride {
                target_track: 1,
                param_id: ParamNodeId {
                    logical_id: 10,
                    node_param_idx: 15,
                },
                param_index: 1,
                value: 0.75,
            }];
        network.neurons[0].output_overrides.effects =
            vec![crate::neural::ProjectEffectParamOverride {
                target_track: 1,
                slot_index: 0,
                param_id: ParamNodeId {
                    logical_id: 11,
                    node_param_idx: 2,
                },
                param_index: 0,
                value: 0.25,
            }];
        snapshot.neural_networks = vec![network];

        refresh_neural_output_override_param_ids(&mut snapshot);

        let neuron = &snapshot.neural_networks[0].neurons[0];
        assert_eq!(
            neuron.output_overrides.instrument[0].param_id,
            ParamNodeId {
                logical_id: 200,
                node_param_idx: 15,
            }
        );
        assert_eq!(
            neuron.output_overrides.effects[0].param_id,
            ParamNodeId {
                logical_id: 300,
                node_param_idx: 2,
            }
        );
    }

    #[test]
    fn project_load_remaps_neuron_plock_param_index_by_node_identity() {
        let mut snapshot = PatternSnapshot::new_default(1, &[Vec::new()]);
        snapshot.instrument_slots[0] = test_slot_snapshot(200, 0, vec![6, 7, 15]);
        let mut network = crate::neural::ProjectNeuralNetwork {
            id: 1,
            num_neurons: 1,
            neurons: vec![crate::neural::ProjectNeuron::default()],
            ..crate::neural::ProjectNeuralNetwork::default()
        };
        network.neurons[0].output_overrides.instrument =
            vec![crate::neural::ProjectParamOverride {
                target_track: 0,
                param_id: ParamNodeId {
                    logical_id: 10,
                    node_param_idx: 15,
                },
                param_index: 1,
                value: 0.75,
            }];
        snapshot.neural_networks = vec![network];

        refresh_neural_output_override_param_ids(&mut snapshot);

        let override_param = &snapshot.neural_networks[0].neurons[0]
            .output_overrides
            .instrument[0];
        assert_eq!(override_param.param_index, 2);
        assert_eq!(
            override_param.param_id,
            ParamNodeId {
                logical_id: 200,
                node_param_idx: 15,
            }
        );
    }

    #[test]
    fn convolution_reverb_project_names_are_treated_as_builtin() {
        let project_name = format!(
            "{}{}",
            EffectDescriptor::BUILTIN_INSERT_PREFIX,
            crate::conv_reverb::NAME
        );
        assert_eq!(
            project_builtin_effect_name_for_save(crate::conv_reverb::NAME),
            Some(project_name.clone())
        );
        assert_eq!(
            project_builtin_effect_name_for_load(&project_name),
            Some(crate::conv_reverb::NAME.to_string())
        );

        let mut project = minimal_project_with_effect_slots(
            vec![Some(crate::conv_reverb::NAME.to_string())],
            Vec::new(),
        );
        project.buses = project::default_project_buses();
        project.buses[1].custom_effects.resize(1, None);
        project.buses[1].custom_effects[0] = Some(crate::conv_reverb::NAME.to_string());
        migrate_dgen_builtin_effect_names(&mut project);

        assert_eq!(project.custom_effects[0][0], Some(project_name.clone()));
        assert_eq!(project.buses[1].custom_effects[0], Some(project_name));
    }

    fn minimal_project_with_effect_slots(
        custom_effects: Vec<Option<String>>,
        effect_slots: Vec<project::ProjectEffectSlot>,
    ) -> ProjectFile {
        ProjectFile {
            version: project::project_file_version(),
            name: "test".to_string(),
            bpm: 120,
            master_volume: 1.0,
            current_pattern: 0,
            current_track: Some(0),
            reverb: ProjectReverbState {
                size: 0.2,
                brightness: 0.8,
                replace: 0.3,
            },
            buses: Vec::new(),
            groups: Vec::new(),
            tracks: vec![ProjectTrack::Sampler {
                sample_path: "samples/kick.wav".to_string(),
                color: None,
                collapsed: false,
            }],
            custom_effects: vec![custom_effects],
            scratch: ProjectScratchState::default(),
            patterns: vec![ProjectPattern {
                track_bits: Vec::new(),
                neural_reset_bits: Vec::new(),
                step_data: Vec::new(),
                track_params: Vec::new(),
                effect_slots: vec![effect_slots],
                midi_fx_slots: Vec::new(),
                instrument_slots: Vec::new(),
                instrument_base_note_offsets: Vec::new(),
                instrument_run_modes: Vec::new(),
                track_sound_states: Vec::new(),
                chord_snapshots: Vec::new(),
                chord_duration_snapshots: Vec::new(),
                chord_delay_snapshots: Vec::new(),
                timebase_plock_snapshots: Vec::new(),
                swing_plock_snapshots: Vec::new(),
                swing_resolution_plock_snapshots: Vec::new(),
                bus_patterns: Vec::new(),
                instrument_types: Vec::new(),
                mod_connections: Vec::new(),
                neural_networks: Vec::new(),
                graph_overrides: Vec::new(),
                sample_paths: Vec::new(),
                sample_names: Vec::new(),
                rack_tracks: Vec::new(),
                plock_variant_registries: Vec::new(),
                key_lock_variant_registries: Vec::new(),
            }],
        }
    }

    fn legacy_default_project_effect_slot(
        mut desc: EffectDescriptor,
    ) -> project::ProjectEffectSlot {
        if let Some(enabled) = desc.params.iter_mut().find(|param| param.name == "enabled") {
            enabled.default = 0.0;
        }
        default_project_effect_slot(&desc)
    }

    #[test]
    fn bus_effect_project_restore_preserves_live_modulator_node_id() {
        let desc = EffectDescriptor::builtin_insert("Filter").expect("filter descriptor");
        let mut live_slot =
            crate::effects::EffectSlotSnapshot::new_default_with_modulator(&desc, 101, 202);
        let mut saved_slot =
            crate::effects::EffectSlotSnapshot::new_default_with_modulator(&desc, 0, 0);
        saved_slot.defaults[0] = 0.0;
        saved_slot.defaults[1] = 1.0;
        saved_slot.plocks[3][1] = Some(0.0);

        restore_saved_bus_effect_slot_runtime_ids(&mut live_slot, saved_slot);

        assert_eq!(live_slot.node_id, 101);
        assert_eq!(live_slot.modulator_node_id, 202);
        assert_eq!(live_slot.defaults[0], 0.0);
        assert_eq!(live_slot.defaults[1], 1.0);
        assert_eq!(live_slot.plocks[3][1], Some(0.0));
    }

    #[test]
    fn legacy_default_filter_delay_migration_drops_untouched_slots() {
        let custom_slot = project::ProjectEffectSlot {
            num_params: 1,
            defaults: vec![0.42],
            plocks: vec![vec![None]; MAX_STEPS],
            plock_param_ids: vec![vec![None]; MAX_STEPS],
            key_locks: std::collections::BTreeMap::new(),
            key_lock_param_ids: std::collections::BTreeMap::new(),
            param_node_indices: vec![9],
            param_node_spans: vec![1],
            tensor_params: Vec::new(),
            ir: None,
        };
        let mut project = minimal_project_with_effect_slots(
            vec![Some("custom-fx".to_string())],
            vec![
                legacy_default_project_effect_slot(EffectDescriptor::builtin_filter()),
                legacy_default_project_effect_slot(EffectDescriptor::builtin_delay()),
                custom_slot.clone(),
            ],
        );

        migrate_legacy_default_track_effects(&mut project);

        assert_eq!(
            project.custom_effects[0],
            vec![Some("custom-fx".to_string())]
        );
        assert_eq!(project.patterns[0].effect_slots[0].len(), 1);
        assert_eq!(
            project.patterns[0].effect_slots[0][0].defaults,
            custom_slot.defaults
        );
    }

    #[test]
    fn legacy_default_filter_delay_migration_preserves_edited_slots() {
        let mut filter_slot =
            legacy_default_project_effect_slot(EffectDescriptor::builtin_filter());
        filter_slot.defaults[2] = 880.0;
        let mut delay_slot = legacy_default_project_effect_slot(EffectDescriptor::builtin_delay());
        delay_slot.plocks[3][0] = Some(0.0);
        let custom_slot = project::ProjectEffectSlot {
            num_params: 1,
            defaults: vec![0.24],
            plocks: vec![vec![None]; MAX_STEPS],
            plock_param_ids: vec![vec![None]; MAX_STEPS],
            key_locks: std::collections::BTreeMap::new(),
            key_lock_param_ids: std::collections::BTreeMap::new(),
            param_node_indices: vec![11],
            param_node_spans: vec![1],
            tensor_params: Vec::new(),
            ir: None,
        };
        let mut project = minimal_project_with_effect_slots(
            vec![Some("custom-fx".to_string())],
            vec![filter_slot, delay_slot, custom_slot.clone()],
        );

        migrate_legacy_default_track_effects(&mut project);

        assert_eq!(
            project.custom_effects[0],
            vec![
                EffectDescriptor::builtin_insert_project_name("Filter"),
                EffectDescriptor::builtin_insert_project_name("Delay"),
                Some("custom-fx".to_string()),
            ]
        );
        assert_eq!(project.patterns[0].effect_slots[0][0].defaults[2], 880.0);
        assert_eq!(
            project.patterns[0].effect_slots[0][1].plocks[3][0],
            Some(0.0)
        );
        assert_eq!(
            project.patterns[0].effect_slots[0][2].defaults,
            custom_slot.defaults
        );
    }

    #[test]
    fn custom_instrument_project_restore_remaps_params_across_inserted_enabled() {
        let mut plocks = vec![vec![None; 4]; MAX_STEPS];
        plocks[7][2] = Some(0.33);

        let old_slot = project::ProjectEffectSlot {
            num_params: 4,
            defaults: vec![0.12, 0.34, 0.56, 0.78],
            plocks,
            plock_param_ids: vec![vec![None; 4]; MAX_STEPS],
            key_locks: std::collections::BTreeMap::new(),
            key_lock_param_ids: std::collections::BTreeMap::new(),
            param_node_indices: vec![
                (crate::lisp_host::HEADER_SLOTS - 1) as u32,
                crate::lisp_host::HEADER_SLOTS as u32,
                crate::voice_modulator::MOD_PARAM_BASE,
                crate::voice_modulator::MOD_PARAM_BASE + 1,
            ],
            param_node_spans: vec![1, 1, 1, 1],
            tensor_params: Vec::new(),
            ir: None,
        };

        let desc = crate::effects::EffectDescriptor {
            name: "test".to_string(),
            input_channels: 0,
            output_channels: 2,
            instrument_modulators: Vec::new(),
            instrument_modulation_targets: Vec::new(),
            tensor_params: Vec::new(),
            params: vec![
                test_param("attack", 0.01, crate::lisp_host::HEADER_SLOTS as u32),
                test_param("tone", 0.02, crate::lisp_host::HEADER_SLOTS as u32 + 1),
                crate::effects::EffectDescriptor::enabled_param(
                    crate::lisp_host::DGEN_ENABLED_PARAM_IDX as u32,
                    1.0,
                ),
                test_param("lfo rate", 0.03, crate::voice_modulator::MOD_PARAM_BASE),
                test_param(
                    "lfo depth",
                    0.04,
                    crate::voice_modulator::MOD_PARAM_BASE + 1,
                ),
            ],
        };

        let restored = project_custom_instrument_slot_into_synced_snapshot(old_slot, &desc, 42, 0);

        assert_eq!(restored.node_id, 42);
        assert_eq!(restored.num_params, 5);
        assert_eq!(restored.defaults, vec![0.12, 0.34, 1.0, 0.56, 0.78]);
        assert_eq!(restored.plocks[7][3], Some(0.33));
        assert_eq!(restored.plocks[7][2], None);
        assert_eq!(
            restored.param_node_indices,
            vec![
                crate::lisp_host::HEADER_SLOTS as u32,
                crate::lisp_host::HEADER_SLOTS as u32 + 1,
                crate::lisp_host::DGEN_ENABLED_PARAM_IDX as u32,
                crate::voice_modulator::MOD_PARAM_BASE,
                crate::voice_modulator::MOD_PARAM_BASE + 1,
            ]
        );
    }

    #[test]
    fn custom_instrument_project_restore_resets_legacy_fixed_voice_mod_params() {
        let mut plocks = vec![vec![None; 4]; MAX_STEPS];
        plocks[7][2] = Some(0.33);

        let old_slot = project::ProjectEffectSlot {
            num_params: 4,
            defaults: vec![0.12, 0.34, 8.0, 9.0],
            plocks,
            plock_param_ids: vec![vec![None; 4]; MAX_STEPS],
            key_locks: std::collections::BTreeMap::new(),
            key_lock_param_ids: std::collections::BTreeMap::new(),
            param_node_indices: vec![
                crate::lisp_host::HEADER_SLOTS as u32,
                crate::lisp_host::HEADER_SLOTS as u32 + 1,
                crate::voice_modulator::LEGACY_FIXED_MOD_PARAM_BASE,
                crate::voice_modulator::LEGACY_FIXED_MOD_PARAM_BASE + 1,
            ],
            param_node_spans: vec![1, 1, 1, 1],
            tensor_params: Vec::new(),
            ir: None,
        };

        let desc = crate::effects::EffectDescriptor {
            name: "test".to_string(),
            input_channels: 0,
            output_channels: 2,
            instrument_modulators: Vec::new(),
            instrument_modulation_targets: Vec::new(),
            tensor_params: Vec::new(),
            params: vec![
                test_param("attack", 0.01, crate::lisp_host::HEADER_SLOTS as u32),
                test_param("tone", 0.02, crate::lisp_host::HEADER_SLOTS as u32 + 1),
                test_param("mod 1 source", 1.0, crate::voice_modulator::MOD_PARAM_BASE),
                test_param(
                    "mod 1 lfo rate",
                    5.0,
                    crate::voice_modulator::MOD_PARAM_BASE + 1,
                ),
            ],
        };

        let restored = project_custom_instrument_slot_into_synced_snapshot(old_slot, &desc, 42, 0);

        assert_eq!(restored.defaults, vec![0.12, 0.34, 1.0, 5.0]);
        assert_eq!(restored.plocks[7][2], None);
        assert_eq!(restored.plocks[7][3], None);
        assert_eq!(
            restored.param_node_indices,
            vec![
                crate::lisp_host::HEADER_SLOTS as u32,
                crate::lisp_host::HEADER_SLOTS as u32 + 1,
                crate::voice_modulator::MOD_PARAM_BASE,
                crate::voice_modulator::MOD_PARAM_BASE + 1,
            ]
        );
    }

    #[test]
    fn custom_instrument_project_restore_skips_generated_host_mod_lanes() {
        let mut plocks = vec![vec![None; 4]; MAX_STEPS];
        plocks[3][2] = Some(0.91);

        let old_slot = project::ProjectEffectSlot {
            num_params: 4,
            defaults: vec![0.12, 0.34, 0.56, 0.78],
            plocks,
            plock_param_ids: vec![vec![None; 4]; MAX_STEPS],
            key_locks: std::collections::BTreeMap::new(),
            key_lock_param_ids: std::collections::BTreeMap::new(),
            param_node_indices: vec![10, 14, 18, 22],
            param_node_spans: vec![1, 1, 1, 1],
            tensor_params: Vec::new(),
            ir: None,
        };

        let desc = crate::effects::EffectDescriptor {
            name: "test".to_string(),
            input_channels: 0,
            output_channels: 2,
            instrument_modulators: Vec::new(),
            instrument_modulation_targets: Vec::new(),
            tensor_params: Vec::new(),
            params: vec![
                test_param("attack", 0.01, 10),
                test_param("__host_mod__attack__lane2__source", 0.0, 14),
                test_param("__host_mod__attack__lane2__depth", 0.0, 18),
                test_param("tone", 0.02, 22),
                test_param("__host_mod__tone__lane2__source", 0.0, 26),
                test_param("__host_mod__tone__lane2__depth", 0.0, 30),
                test_param("cutoff", 0.03, 34),
                test_param("gain", 0.04, 38),
            ],
        };

        let restored = project_custom_instrument_slot_into_synced_snapshot(old_slot, &desc, 42, 0);

        assert_eq!(restored.node_id, 42);
        assert_eq!(restored.num_params, 8);
        assert_eq!(
            restored.defaults,
            vec![0.12, 0.0, 0.0, 0.34, 0.0, 0.0, 0.56, 0.78]
        );
        assert_eq!(restored.plocks[3][6], Some(0.91));
        assert_eq!(restored.plocks[3][1], None);
        assert_eq!(restored.plocks[3][2], None);
    }

    #[test]
    fn custom_instrument_project_restore_preserves_saved_generated_host_mod_lanes() {
        let desc = crate::effects::EffectDescriptor {
            name: "test".to_string(),
            input_channels: 0,
            output_channels: 2,
            instrument_modulators: Vec::new(),
            instrument_modulation_targets: Vec::new(),
            tensor_params: Vec::new(),
            params: vec![
                test_param("attack", 0.01, 10),
                test_param("__host_mod__attack__lane2__source", 0.0, 14),
                test_param("__host_mod__attack__lane2__depth", 0.0, 18),
                test_param("tone", 0.02, 22),
                test_param("__host_mod__tone__lane2__source", 0.0, 26),
                test_param("__host_mod__tone__lane2__depth", 0.0, 30),
                test_param("cutoff", 0.03, 34),
                test_param("gain", 0.04, 38),
            ],
        };
        let mut plocks = vec![vec![None; desc.params.len()]; MAX_STEPS];
        plocks[3][2] = Some(0.91);
        plocks[4][5] = Some(-0.42);

        let saved_slot = project::ProjectEffectSlot {
            num_params: desc.params.len() as u32,
            defaults: vec![0.12, 2.0, 0.31, 0.34, 4.0, -0.27, 0.56, 0.78],
            plocks,
            plock_param_ids: vec![vec![None; desc.params.len()]; MAX_STEPS],
            key_locks: std::collections::BTreeMap::new(),
            key_lock_param_ids: std::collections::BTreeMap::new(),
            param_node_indices: desc
                .params
                .iter()
                .map(|param| param.node_param_idx)
                .collect(),
            param_node_spans: desc
                .params
                .iter()
                .map(|param| param.node_param_span.max(1))
                .collect(),
            tensor_params: Vec::new(),
            ir: None,
        };

        let restored =
            project_custom_instrument_slot_into_synced_snapshot(saved_slot, &desc, 42, 0);

        assert_eq!(restored.node_id, 42);
        assert_eq!(restored.num_params, desc.params.len() as u32);
        assert_eq!(
            restored.defaults,
            vec![0.12, 2.0, 0.31, 0.34, 4.0, -0.27, 0.56, 0.78]
        );
        assert_eq!(restored.plocks[3][2], Some(0.91));
        assert_eq!(restored.plocks[4][5], Some(-0.42));
        assert_eq!(
            restored.param_node_indices,
            desc.params
                .iter()
                .map(|param| param.node_param_idx)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn custom_instrument_project_restore_refreshes_stale_param_spans() {
        let desc = crate::effects::EffectDescriptor {
            name: "test".to_string(),
            input_channels: 0,
            output_channels: 2,
            instrument_modulators: Vec::new(),
            instrument_modulation_targets: Vec::new(),
            tensor_params: Vec::new(),
            params: vec![
                test_param("wave", 0.12, 10),
                test_param("cutoff", 7200.0, 14),
                test_param("__dgen_mod_active__cutoff", 0.0, 15),
                test_param("mod cutoff slot 1 amt", 0.0, 16),
            ],
        };
        let saved_slot = project::ProjectEffectSlot {
            num_params: desc.params.len() as u32,
            defaults: vec![0.5, 7400.0, 0.0, 0.0],
            plocks: vec![vec![None; desc.params.len()]; MAX_STEPS],
            plock_param_ids: vec![vec![None; desc.params.len()]; MAX_STEPS],
            key_locks: std::collections::BTreeMap::new(),
            key_lock_param_ids: std::collections::BTreeMap::new(),
            param_node_indices: desc
                .params
                .iter()
                .map(|param| param.node_param_idx)
                .collect(),
            param_node_spans: vec![1, 4, 1, 1],
            tensor_params: Vec::new(),
            ir: None,
        };

        let restored =
            project_custom_instrument_slot_into_synced_snapshot(saved_slot, &desc, 42, 0);

        assert_eq!(restored.defaults[1], 7400.0);
        assert_eq!(restored.param_node_indices, vec![10, 14, 15, 16]);
        assert_eq!(restored.param_node_spans, vec![1, 1, 1, 1]);
    }

    #[test]
    fn custom_instrument_project_restore_preserves_plocks_by_node_idx_when_lane_names_change() {
        let desc = crate::effects::EffectDescriptor {
            name: "test".to_string(),
            input_channels: 0,
            output_channels: 2,
            instrument_modulators: Vec::new(),
            instrument_modulation_targets: Vec::new(),
            tensor_params: Vec::new(),
            params: vec![
                test_param("cutoff", 0.01, 10),
                test_param("mod cutoff lane 1 src", 1.0, 14),
                test_param("mod cutoff lane 1 amt", 0.25, 18),
                test_param("mod cutoff lane 2 src", 2.0, 22),
                test_param("mod cutoff lane 2 amt", -0.5, 26),
            ],
        };
        let mut plocks = vec![vec![None; desc.params.len()]; MAX_STEPS];
        plocks[3][2] = Some(0.75);
        plocks[4][4] = Some(-0.25);

        let saved_slot = project::ProjectEffectSlot {
            num_params: desc.params.len() as u32,
            defaults: vec![0.12, 1.0, 0.31, 2.0, -0.27],
            plocks,
            plock_param_ids: vec![vec![None; desc.params.len()]; MAX_STEPS],
            key_locks: std::collections::BTreeMap::new(),
            key_lock_param_ids: std::collections::BTreeMap::new(),
            param_node_indices: desc
                .params
                .iter()
                .map(|param| param.node_param_idx)
                .collect(),
            param_node_spans: desc
                .params
                .iter()
                .map(|param| param.node_param_span.max(1))
                .collect(),
            tensor_params: Vec::new(),
            ir: None,
        };

        let renamed_desc = crate::effects::EffectDescriptor {
            params: vec![
                test_param("cutoff", 0.01, 10),
                test_param("mod filter cutoff lane 1 src", 1.0, 14),
                test_param("mod filter cutoff lane 1 amt", 0.25, 18),
                test_param("mod filter cutoff lane 2 src", 2.0, 22),
                test_param("mod filter cutoff lane 2 amt", -0.5, 26),
            ],
            ..desc
        };

        let restored =
            project_custom_instrument_slot_into_synced_snapshot(saved_slot, &renamed_desc, 42, 0);

        assert_eq!(restored.node_id, 42);
        assert_eq!(restored.defaults, vec![0.12, 1.0, 0.31, 2.0, -0.27]);
        assert_eq!(restored.plocks[3][2], Some(0.75));
        assert_eq!(restored.plocks[4][4], Some(-0.25));
        assert_eq!(
            restored.param_node_indices,
            renamed_desc
                .params
                .iter()
                .map(|param| param.node_param_idx)
                .collect::<Vec<_>>()
        );
    }
}
