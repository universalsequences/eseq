use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Instant;


use crate::effects::{EffectDescriptor, BUILTIN_SLOT_COUNT};
use crate::lisp_host;
use crate::macro_engine::{MacroParamKey, ResolvedMacroTarget};
use crate::neural::ParamNodeId;
use crate::process::ParamTarget;
use crate::project::{
    self, chord_snapshot_from_steps_durations_and_delays, project_file_version, ProjectBusChannel,
    ProjectBusPatternSnapshot, ProjectFile, ProjectMacro, ProjectPattern, ProjectReverbState,
    ProjectScratchState, ProjectTrack, ProjectTrackKind,
};
use crate::sequencer::{
    BusGateSequence, BusId, BusPatternSnapshot, CustomInstrumentRunMode, InstrumentType,
    PatternSnapshot, RackTrackSnapshot, TrackOutput, MAX_STEPS, TRACK_PATTERN_WORDS,
};

use super::fx_chain::{FxChainLocator, FxGraphEditBatch};
use super::graph::{
    RackCustomBuildSpec, RackSamplerBuildSpec, RackSlotBuildSpec, RackSlotInstrumentBuildSpec,
};
use super::{
    App, BusChannelState, InputMode, ProjectSampleAsset, Region, SidebarMode, SidebarTab,
};

pub(super) fn resolve_live_macro_target(
    state: &crate::sequencer::SequencerState,
    effect_descriptors: &[Vec<EffectDescriptor>],
    instrument_descriptors: &[EffectDescriptor],
    buses: &[super::BusChannelState],
    scope: crate::macro_engine::ParamScope,
    target: &ParamTarget,
) -> Option<ResolvedMacroTarget> {
    if let crate::macro_engine::ParamScope::Bus(bus_id) = scope {
        let ParamTarget::EffectParam {
            slot,
            effect,
            param,
            param_id: previous_param_id,
        } = target
        else {
            return None;
        };
        let bus = buses.iter().find(|bus| bus.id == bus_id)?;
        let descriptor = bus.effect_descriptors.get(*slot)?;
        if !descriptor.name.eq_ignore_ascii_case(effect) {
            return None;
        }
        let param_idx = descriptor
            .params
            .iter()
            .position(|descriptor| descriptor.has_tag_or_name(param))?;
        let live_slot = bus.effect_slots.get(*slot)?;
        let raw_idx = live_slot
            .param_node_indices
            .get(param_idx)
            .copied()
            .unwrap_or(param_idx as u32);
        let param_id =
            ParamNodeId::from_slot_param(live_slot.node_id, live_slot.modulator_node_id, raw_idx);
        if previous_param_id.is_some() && param_id.is_none() {
            return None;
        }
        return Some(ResolvedMacroTarget {
            target: ParamTarget::EffectParam {
                slot: *slot,
                effect: effect.clone(),
                param: param.clone(),
                param_id,
            },
            key: MacroParamKey::for_bus_effect(bus_id, *slot, param_idx, param_id),
        });
    }
    let crate::macro_engine::ParamScope::Track(track) = scope else {
        unreachable!()
    };
    match target {
        ParamTarget::EffectParam {
            slot,
            effect,
            param,
            param_id: previous_param_id,
        } => {
            let descriptor = effect_descriptors.get(track)?.get(*slot)?;
            if !descriptor.name.eq_ignore_ascii_case(effect) {
                return None;
            }
            let param_idx = descriptor
                .params
                .iter()
                .position(|descriptor| descriptor.has_tag_or_name(param))?;
            let live_slot = state.pattern.effect_chains.get(track)?.get(*slot)?;
            if param_idx >= live_slot.num_params.load(Ordering::Relaxed) as usize {
                return None;
            }
            let raw_idx = live_slot.resolve_node_idx(param_idx) as u32;
            let param_id = ParamNodeId::from_slot_param(
                live_slot.node_id.load(Ordering::Relaxed),
                live_slot.modulator_node_id.load(Ordering::Relaxed),
                raw_idx,
            );
            if previous_param_id.is_some() && param_id.is_none() {
                return None;
            }
            Some(ResolvedMacroTarget {
                target: ParamTarget::EffectParam {
                    slot: *slot,
                    effect: effect.clone(),
                    param: param.clone(),
                    param_id,
                },
                key: MacroParamKey::for_effect(track, *slot, param_idx, param_id),
            })
        }
        ParamTarget::InstrumentParam {
            param,
            param_id: previous_param_id,
        } => {
            let descriptor = instrument_descriptors.get(track)?;
            let param_idx = descriptor
                .params
                .iter()
                .position(|descriptor| descriptor.has_tag_or_name(param))?;
            let live_slot = state.pattern.instrument_slots.get(track)?;
            if param_idx >= live_slot.num_params.load(Ordering::Relaxed) as usize {
                return None;
            }
            let raw_idx = live_slot.resolve_node_idx(param_idx) as u32;
            let param_id = ParamNodeId::from_slot_param(
                live_slot.node_id.load(Ordering::Relaxed),
                live_slot.modulator_node_id.load(Ordering::Relaxed),
                raw_idx,
            );
            if previous_param_id.is_some() && param_id.is_none() {
                return None;
            }
            Some(ResolvedMacroTarget {
                target: ParamTarget::InstrumentParam {
                    param: param.clone(),
                    param_id,
                },
                key: MacroParamKey::for_instrument(track, param_idx, param_id),
            })
        }
        ParamTarget::StepParam { .. } | ParamTarget::ProcessInlet { .. } => None,
        _ => Some(ResolvedMacroTarget {
            target: target.clone(),
            key: MacroParamKey::from_target(scope, target, None)?,
        }),
    }
}

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
    Some(if raw_idx >= crate::instruments::voice_modulator::MOD_PARAM_BASE {
        raw_idx - crate::instruments::voice_modulator::MOD_PARAM_BASE
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
        table: None,
    }
}

fn project_builtin_effect_name_for_save(name: &str) -> Option<String> {
    let trimmed = name.trim();
    crate::effects::EffectDescriptor::builtin_insert_project_name(trimmed).or_else(|| {
        crate::effects::dgen_builtin::find(trimmed).map(|builtin| {
            format!(
                "{}{}",
                crate::effects::EffectDescriptor::BUILTIN_INSERT_PREFIX,
                builtin.name
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
    crate::effects::dgen_builtin::find(stripped)
        .map(|builtin| builtin.name.to_string())
}

fn migrate_dgen_builtin_effect_names(project: &mut ProjectFile) {
    fn migrate_name(name: &mut Option<String>) {
        let Some(raw_name) = name.as_deref() else {
            return;
        };
        if crate::effects::dgen_builtin::contains(raw_name.trim()) {
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

/// Version-1 project files were saved when dgenlisp node state used a 6-slot
/// header. Version 2 grew the header to 10 slots (the node-owned process
/// identity refactor), which shifted every dgen param's node-state index by
/// +4. The migration below rewrites saved indices for dgen-hosted slots so
/// they address the same memory cells under the new layout. Native builtin
/// effects (filter, OTT, DJ mixer, EQ8, …) and MIDI FX own their layouts,
/// which did not change, and are left untouched.
const LEGACY_DGEN_HEADER_SLOTS: u32 = 6;

fn legacy_dgen_header_delta() -> u32 {
    crate::lisp_host::HEADER_SLOTS as u32 - LEGACY_DGEN_HEADER_SLOTS
}

fn shifted_legacy_dgen_node_index(idx: u32) -> Option<u32> {
    (idx >= LEGACY_DGEN_HEADER_SLOTS && idx < crate::instruments::voice_modulator::LEGACY_FIXED_MOD_PARAM_BASE)
        .then(|| idx + legacy_dgen_header_delta())
}

/// Shift a legacy dgen slot's saved node indices to the 10-slot header
/// layout. Returns the set of pre-migration indices that were shifted so
/// callers can rebase saved `ParamNodeId`s referencing this slot.
fn migrate_legacy_dgen_effect_slot(
    slot: &mut project::ProjectEffectSlot,
) -> std::collections::BTreeSet<u32> {
    let shifted_sources: std::collections::BTreeSet<u32> = slot
        .param_node_indices
        .iter()
        .copied()
        .filter(|&idx| shifted_legacy_dgen_node_index(idx).is_some())
        .collect();
    for idx in &mut slot.param_node_indices {
        if let Some(shifted) = shifted_legacy_dgen_node_index(*idx) {
            *idx = shifted;
        }
    }
    fn migrate_param_id(
        shifted_sources: &std::collections::BTreeSet<u32>,
        param_id: &mut Option<ParamNodeId>,
    ) {
        if let Some(param_id) = param_id {
            if shifted_sources.contains(&param_id.node_param_idx) {
                param_id.node_param_idx += legacy_dgen_header_delta();
            }
        }
    }
    for row in &mut slot.plock_param_ids {
        for param_id in row {
            migrate_param_id(&shifted_sources, param_id);
        }
    }
    for row in slot.key_lock_param_ids.values_mut() {
        for param_id in row {
            migrate_param_id(&shifted_sources, param_id);
        }
    }
    shifted_sources
}

/// True when a saved effect-chain slot name refers to a dgenlisp-hosted
/// effect: any custom (non-builtin) effect, or a dgenlisp-backed builtin.
fn project_effect_name_is_dgen_hosted(name: Option<&str>) -> bool {
    let Some(name) = name else {
        return false;
    };
    match crate::effects::EffectDescriptor::strip_builtin_insert_project_name(name) {
        Some(builtin) => crate::effects::dgen_builtin::contains(builtin),
        None => true,
    }
}

fn migrate_legacy_dgen_param_node_indices(project: &mut ProjectFile) {
    let custom_instrument_tracks: Vec<bool> = project
        .tracks
        .iter()
        .map(|track| matches!(track.kind, project::ProjectTrackKind::Custom { .. }))
        .collect();
    let track_effect_names = project.custom_effects.clone();
    let bus_effect_names: std::collections::HashMap<u64, Vec<Option<String>>> = project
        .buses
        .iter()
        .map(|bus| (bus.id, bus.custom_effects.clone()))
        .collect();
    let slot_name_is_dgen = |names: Option<&Vec<Option<String>>>, slot_idx: usize| {
        project_effect_name_is_dgen_hosted(
            names
                .and_then(|names| names.get(slot_idx))
                .and_then(|name| name.as_deref()),
        )
    };

    for bus in &mut project.buses {
        let names = bus.custom_effects.clone();
        for (slot_idx, slot) in bus.effect_slots.iter_mut().enumerate() {
            if slot_name_is_dgen(Some(&names), slot_idx) {
                migrate_legacy_dgen_effect_slot(slot);
            }
        }
    }

    for pattern in &mut project.patterns {
        let mut instrument_shifted: std::collections::HashMap<
            usize,
            std::collections::BTreeSet<u32>,
        > = std::collections::HashMap::new();
        let mut effect_shifted: std::collections::HashMap<
            (usize, usize),
            std::collections::BTreeSet<u32>,
        > = std::collections::HashMap::new();

        for (track, slot) in pattern.instrument_slots.iter_mut().enumerate() {
            if custom_instrument_tracks
                .get(track)
                .copied()
                .unwrap_or(false)
            {
                instrument_shifted.insert(track, migrate_legacy_dgen_effect_slot(slot));
            }
        }
        for (track, slots) in pattern.effect_slots.iter_mut().enumerate() {
            for (slot_idx, slot) in slots.iter_mut().enumerate() {
                if slot_name_is_dgen(track_effect_names.get(track), slot_idx) {
                    effect_shifted.insert((track, slot_idx), migrate_legacy_dgen_effect_slot(slot));
                }
            }
        }
        for bus_pattern in &mut pattern.bus_patterns {
            let names = bus_effect_names.get(&bus_pattern.id);
            for (slot_idx, slot) in bus_pattern.effect_slots.iter_mut().enumerate() {
                if slot_name_is_dgen(names, slot_idx) {
                    migrate_legacy_dgen_effect_slot(slot);
                }
            }
        }
        for rack in pattern.rack_tracks.iter_mut().flatten() {
            for slot in &mut rack.slots {
                if matches!(slot.instrument_type, project::ProjectInstrumentType::Custom) {
                    migrate_legacy_dgen_effect_slot(&mut slot.instrument_slot);
                }
                let names = slot.custom_effects.clone();
                for (fx_idx, fx_slot) in slot.effect_slots.iter_mut().enumerate() {
                    if slot_name_is_dgen(Some(&names), fx_idx) {
                        migrate_legacy_dgen_effect_slot(fx_slot);
                    }
                }
            }
        }

        for network in &mut pattern.neural_networks {
            for neuron in &mut network.neurons {
                for override_param in &mut neuron.output_overrides.instrument {
                    if let Some(shifted_sources) =
                        instrument_shifted.get(&override_param.target_track)
                    {
                        if shifted_sources.contains(&override_param.param_id.node_param_idx) {
                            override_param.param_id.node_param_idx += legacy_dgen_header_delta();
                        }
                    }
                }
                for override_param in &mut neuron.output_overrides.effects {
                    if let Some(shifted_sources) = effect_shifted
                        .get(&(override_param.target_track, override_param.slot_index))
                    {
                        if shifted_sources.contains(&override_param.param_id.node_param_idx) {
                            override_param.param_id.node_param_idx += legacy_dgen_header_delta();
                        }
                    }
                }
            }
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
        node_idx >= crate::instruments::voice_modulator::LEGACY_FIXED_MOD_PARAM_BASE
            && node_idx < crate::instruments::voice_modulator::LEGACY_FIXED_MOD_PARAM_BASE_END
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
            && param.node_param_idx >= crate::instruments::voice_modulator::MOD_PARAM_BASE
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
                    && param.node_param_idx < crate::instruments::voice_modulator::MOD_PARAM_BASE
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
        table: slot.table.clone(),
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
                    table: None,
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
            output: value.output,
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
        bus.output = value.output;
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
    /// Drop every piece of state whose identity belongs to the current
    /// arrangement before the project's track topology is torn down.
    ///
    /// This must run before `clear_all_tracks`: new-track registration
    /// reconciles committed arrangement lanes against the live topology and
    /// must never see lanes from the project being replaced.
    fn clear_project_arrangement_state(&mut self) {
        self.state.clear_committed_arrangement();
        self.song_capture_armed = false;
        self.recording_kind = None;
        // The manual-override latch now survives transport stop; a project
        // switch is the one boundary it must never cross.
        self.state.clear_song_manual_latch();
        self.song_clip_selection = None;
        self.song_region_selection = None;
        self.song_edit_error = None;
        // In-flight capture/take state is keyed by the OUTGOING project's
        // track indices and pattern pools; carrying it across a load would let
        // the next stop-commit splice stale chunks into the new project.
        self.discard_song_capture_take();
        self.active_runtime_song = None;
        self.active_song_start_beat = None;
        self.song_mirrored_row = None;
        self.song_transport_mode = crate::app::song_transport::SongTransportMode::Stopped;
    }

    pub fn start_new_project(&mut self) {
        self.editor.pending_project_load = None;
        self.groups.clear();
        self.publish_rack_choke_runtime();
        self.clear_project_arrangement_state();

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
                        idx: crate::effects::stereo_panner::STEREO_PANNER_PARAM_VOLUME,
                        logical_id: nodes.volume_id as u64,
                        fvalue: crate::mixer_volume::fader_to_gain(bus.volume),
                    },
                );
                crate::audiograph::params_push_wrapper(
                    self.graph.lg.0,
                    crate::audiograph::ParamMsg {
                        idx: crate::effects::stereo_panner::STEREO_PANNER_PARAM_MUTE,
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
        self.set_reverb_param_unrecorded(0, 0.2);
        self.set_reverb_param_unrecorded(1, 0.8);
        self.set_reverb_param_unrecorded(2, 0.3);

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
        self.ui.recording = false;
        self.recording_history = None;

        self.history.reset();
        self.device_registry.clear();

        // Empty-arrangement spec 4.3: the arrangement always exists. The
        // teardown above cleared it to `None` (its lanes were indexed by the
        // outgoing project's tracks); with the new topology in place, install
        // the empty arrangement the project starts on.
        if let Err(error) = self
            .state
            .set_committed_arrangement(Some(self.empty_arrangement()))
        {
            debug_assert!(false, "empty arrangement failed to install: {error}");
        }

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

    pub(crate) fn add_bus_channel_with_id(
        &mut self,
        id: BusId,
        name: impl Into<String>,
    ) -> Result<usize, String> {
        if self.buses.iter().any(|bus| bus.id == id) {
            return Err(format!("Bus {:?} already exists", id));
        }
        self.buses.push(BusChannelState::new(id, name));
        let index = self.buses.len() - 1;
        let bus = self.buses[index].clone();
        self.graph_controller().ensure_bus_graph_node(bus.id, &bus.name);
        self.publish_bus_gate_runtime();
        Ok(index)
    }

    pub fn delete_bus_channel(&mut self, id: BusId) -> bool {
        if id == BusId::MIX {
            return false;
        }

        let Some(bus_idx) = self.buses.iter().position(|bus| bus.id == id) else {
            return false;
        };

        let batch = FxGraphEditBatch::new(self.graph.lg.0);
        if let Err(err) = self
            .editor
            .effect_chain_leases
            .retire_host(FxChainLocator::Bus(id), batch.serial)
        {
            self.editor.status_message = Some((format!("Error: {err}"), Instant::now()));
            return false;
        }

        if let Some(bus) = self.buses.get(bus_idx) {
            for slot in &bus.effect_slots {
                if slot.node_id != 0 {
                    unsafe {
                        crate::audiograph::delete_node(self.graph.lg.0, slot.node_id as i32);
                    }
                    crate::effects::dgen_builtin::clear_instance(slot.node_id as i32);
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

        // Anything chained into this bus loses its destination; drop those
        // back to the master mix before the node pair disappears, or their
        // audio would vanish with it (docs/drum-rack-v2-spec.md).
        let orphans = self
            .buses
            .iter_mut()
            .filter(|bus| bus.output.destination() == Some(id.0))
            .map(|bus| {
                bus.output = crate::project::BusOutput::Mix;
                bus.id
            })
            .collect::<Vec<_>>();
        for orphan in orphans {
            self.graph_controller().apply_bus_output_routing(orphan);
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
    #[doc(hidden)]
    pub fn set_track_output_all_scenes_unrecorded(
        &mut self,
        track: usize,
        output: TrackOutput,
    ) -> bool {
        if track >= self.state.pattern.track_params.len() {
            return false;
        }
        let live_changed = self.state.pattern.track_params[track].output() != output;
        self.state.pattern.track_params[track].set_output(output.clone());
        let stored_changed = self.state
            .set_track_output_in_all_track_patterns(track, output);
        self.graph_controller().apply_track_output_routing(track);
        live_changed || stored_changed
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
        if project.version > project::project_file_version() {
            return Err(format!("Unsupported project version {}", project.version));
        }
        migrate_dgen_builtin_effect_names(&mut project);
        if project.version < 2 {
            migrate_legacy_dgen_param_node_indices(&mut project);
        }
        project.normalize_device_instances()?;

        self.editor.pending_project_load = Some(super::PendingProjectLoad {
            name: name.to_string(),
            tick: 0,
            project,
            sample_assets: std::collections::HashMap::new(),
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

    fn load_project_sample_asset(
        &mut self,
        sample_assets: &mut std::collections::HashMap<PathBuf, ProjectSampleAsset>,
        wav_path: &Path,
    ) -> Result<ProjectSampleAsset, String> {
        // Canonical paths collapse relative, absolute, and symlinked references
        // before any decode, graph allocation, or analyzer submission occurs.
        let canonical_path = wav_path.canonicalize().map_err(|error| {
            format!(
                "Failed to resolve sample '{}': {error}",
                wav_path.display()
            )
        })?;
        if let Some(asset) = sample_assets.get(&canonical_path) {
            return Ok(asset.clone());
        }

        let loaded =
            crate::instruments::sampler::load_wav_buffer(self.graph.lg.0, &canonical_path)?;
        self.submit_sample_analysis(&loaded);
        let asset = ProjectSampleAsset {
            buffer_id: loaded.buffer_id,
            sample_rate: loaded.sample_rate,
            decoded_name: loaded.name,
        };
        sample_assets.insert(canonical_path, asset.clone());
        Ok(asset)
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
        super::edit::finish_active_gesture(self);
        let project = self.capture_project(project_name)?;
        project::save_project(project_name, &project).map_err(|error| error.to_string())?;
        self.history.mark_saved();
        Ok(())
    }

    fn capture_track_as_container_preset(
        &mut self,
        track: usize,
        name: &str,
        tags: Vec<String>,
        author: String,
    ) -> Result<crate::project::ProjectSoundPreset, String> {
        if track >= self.tracks.len() {
            return Err(format!("Invalid track index {}", track + 1));
        }
        let project = self.capture_project(name)?;
        let source_track = project.tracks[track].clone();
        let pattern = project
            .patterns
            .get(project.current_pattern)
            .ok_or_else(|| "Current pattern is missing while saving Sound".to_string())?;
        let ProjectTrack {
            id,
            color,
            collapsed,
            kind,
        } = source_track;
        let (track_payload, rack_payload) = match kind {
            ProjectTrackKind::Rack { .. } => {
                let rack = pattern
                    .rack_tracks
                    .get(track)
                    .cloned()
                    .flatten()
                    .ok_or_else(|| "Rack pattern data is missing".to_string())?;
                (project.tracks[track].clone(), rack)
            }
            ProjectTrackKind::Sampler { sample_path } => {
                let sample_name = pattern.sample_names.get(track).cloned();
                let slot_source = crate::project::ProjectRackTrackSlot {
                    instrument_type: crate::project::ProjectInstrumentType::Sampler,
                    sample_path: Some(sample_path.clone()),
                    sample_name: sample_name.clone(),
                    instrument_name: None,
                };
                let slot = crate::project::ProjectRackSlotPattern {
                    instrument_type: crate::project::ProjectInstrumentType::Sampler,
                    instrument_run_mode: crate::project::ProjectCustomInstrumentRunMode::Instrument,
                    instrument_base_note_offset: pattern
                        .instrument_base_note_offsets
                        .get(track)
                        .copied()
                        .unwrap_or(0.0),
                    choke_group: None,
                    gain: 1.0,
                    pan: 0.0,
                    mute: false,
                    solo: false,
                    max_polyphony: crate::audio::MAX_VOICES,
                    param_plocks: Vec::new(),
                    instrument_slot: pattern.instrument_slots[track].clone(),
                    effect_slots: pattern.effect_slots[track].clone(),
                    custom_effects: project.custom_effects[track].clone(),
                    track_sound_state: pattern.track_sound_states[track].clone(),
                    sample_path: Some(sample_path),
                    sample_name,
                };
                (
                    ProjectTrack {
                        id,
                        color,
                        collapsed,
                        kind: ProjectTrackKind::Rack {
                            routing: crate::project::ProjectRackRouting::Broadcast,
                            slots: vec![slot_source],
                        },
                    },
                    crate::project::ProjectRackTrackPattern {
                        routing: crate::project::ProjectRackRouting::Broadcast,
                        slots: vec![slot],
                        macros: crate::project::default_project_rack_macros(),
                    },
                )
            }
            ProjectTrackKind::Custom { instrument_name } => {
                let slot_source = crate::project::ProjectRackTrackSlot {
                    instrument_type: crate::project::ProjectInstrumentType::Custom,
                    sample_path: None,
                    sample_name: None,
                    instrument_name: Some(instrument_name.clone()),
                };
                let slot = crate::project::ProjectRackSlotPattern {
                    instrument_type: crate::project::ProjectInstrumentType::Custom,
                    instrument_run_mode: pattern
                        .instrument_run_modes
                        .get(track)
                        .copied()
                        .unwrap_or_default(),
                    instrument_base_note_offset: pattern
                        .instrument_base_note_offsets
                        .get(track)
                        .copied()
                        .unwrap_or(0.0),
                    choke_group: None,
                    gain: 1.0,
                    pan: 0.0,
                    mute: false,
                    solo: false,
                    max_polyphony: crate::audio::MAX_VOICES,
                    param_plocks: Vec::new(),
                    instrument_slot: pattern.instrument_slots[track].clone(),
                    effect_slots: pattern.effect_slots[track].clone(),
                    custom_effects: project.custom_effects[track].clone(),
                    track_sound_state: pattern.track_sound_states[track].clone(),
                    sample_path: None,
                    sample_name: None,
                };
                (
                    ProjectTrack {
                        id,
                        color,
                        collapsed,
                        kind: ProjectTrackKind::Rack {
                            routing: crate::project::ProjectRackRouting::Broadcast,
                            slots: vec![slot_source],
                        },
                    },
                    crate::project::ProjectRackTrackPattern {
                        routing: crate::project::ProjectRackRouting::Broadcast,
                        slots: vec![slot],
                        macros: crate::project::default_project_rack_macros(),
                    },
                )
            }
            ProjectTrackKind::Modulator => {
                return Err("Modulator tracks cannot be saved as Sounds".to_string())
            }
        };
        let sound = crate::project::ProjectSoundPreset {
            version: crate::project::project_file_version(),
            metadata: crate::project::ProjectSoundMetadata {
                name: name.trim().to_string(),
                tags,
                author,
            },
            track: track_payload,
            rack: rack_payload,
        };
        Ok(sound)
    }

    pub fn load_sound_onto_track(&mut self, track: usize, path: &Path) -> Result<(), String> {
        let sound = crate::project::load_sound_preset(path).map_err(|error| error.to_string())?;
        let fallback_name = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or("Sound");
        self.load_sound_preset_onto_track(track, sound, fallback_name)
    }

    fn load_sound_preset_onto_track(
        &mut self,
        track: usize,
        sound: crate::project::ProjectSoundPreset,
        fallback_name: &str,
    ) -> Result<(), String> {
        if track >= self.tracks.len() {
            return Err(format!("Invalid track index {}", track + 1));
        }
        let track_id = self.track_registry.id_at(track)
            .ok_or_else(|| format!("Track {} has no stable identity", track + 1))?;
        self.apply_recorded_instrument_binding_mutation(track, "Load Sound", |app| {
            app.load_container_preset_onto_track(track, sound, fallback_name)?;
            app.device_registry.clear_rack_track(track_id);
            Ok(())
        })
    }

    pub fn add_track_from_sound(&mut self, path: &Path) -> Result<usize, String> {
        // Parse and validate before changing topology. Loading samples and engines
        // still happens after the shell exists, so roll that shell back on error.
        let sound = crate::project::load_sound_preset(path).map_err(|error| error.to_string())?;
        let fallback_name = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or("Sound");
        self.add_track_from_sound_preset(sound, fallback_name)
    }

    /// Track-creating half of [`App::add_track_from_sound`], for callers that
    /// already hold the parsed Sound (a kit rebuilds one member track per pad
    /// from Sounds it carries inline rather than from files).
    pub fn add_track_from_sound_preset(
        &mut self,
        sound: crate::project::ProjectSoundPreset,
        fallback_name: &str,
    ) -> Result<usize, String> {
        let track = self.graph_controller().add_blank_sampler_track()?;
        if let Err(error) = self.load_container_preset_onto_track(track, sound, fallback_name) {
            let rollback = self.graph_controller().delete_track(track);
            return match rollback {
                Ok(_) => Err(error),
                Err(rollback_error) => Err(format!(
                    "{error}; failed to roll back new track: {rollback_error}"
                )),
            };
        }
        Ok(track)
    }

    fn load_container_preset_onto_track(
        &mut self,
        track: usize,
        sound: crate::project::ProjectSoundPreset,
        fallback_name: &str,
    ) -> Result<(), String> {
        let source_slots = match &sound.track.kind {
            ProjectTrackKind::Rack { slots, .. } => slots.clone(),
            _ => return Err("Container preset does not contain a rack".to_string()),
        };
        if source_slots.len() != sound.rack.slots.len() {
            return Err("Container preset source/state slot counts do not match".to_string());
        }
        let mut rack = RackTrackSnapshot::from(sound.rack);
        for (slot_idx, (source, slot)) in source_slots.iter().zip(&mut rack.slots).enumerate() {
            match source.instrument_type {
                crate::project::ProjectInstrumentType::Sampler => {
                    let sample_path = source
                        .sample_path
                        .as_deref()
                        .ok_or_else(|| format!("Sound slot {} has no sample path", slot_idx + 1))?;
                    let loaded =
                        crate::instruments::sampler::load_wav_buffer(self.graph.lg.0, Path::new(sample_path))
                            .map_err(|error| {
                                format!(
                                    "Failed to load Sound sample '{}' for slot {}: {error}",
                                    sample_path,
                                    slot_idx + 1
                                )
                            })?;
                    self.submit_sample_analysis(&loaded);
                    let sample_name = source
                        .sample_name
                        .clone()
                        .unwrap_or_else(|| loaded.name.clone());
                    self.register_loaded_sample_path(
                        &sample_name,
                        loaded.buffer_id,
                        PathBuf::from(sample_path),
                    );
                    slot.instrument_type = InstrumentType::Sampler;
                    slot.sample_id = Some((loaded.buffer_id, sample_name, loaded.sample_rate));
                    slot.track_sound_state.engine_id = None;
                }
                crate::project::ProjectInstrumentType::Custom => {
                    let instrument_name = source.instrument_name.as_deref().ok_or_else(|| {
                        format!("Sound slot {} has no instrument name", slot_idx + 1)
                    })?;
                    let prepared =
                        self.prepare_saved_instrument_for_rack_slot_sync(instrument_name)?;
                    slot.instrument_type = InstrumentType::Custom;
                    slot.instrument_run_mode = prepared.run_mode;
                    slot.track_sound_state.engine_id = Some(prepared.engine_id);
                    slot.sample_id = None;
                }
                crate::project::ProjectInstrumentType::Modulator
                | crate::project::ProjectInstrumentType::Rack => {
                    return Err(format!(
                        "Sound slot {} has unsupported instrument type",
                        slot_idx + 1
                    ));
                }
            }
        }
        let saved_effects = rack
            .slots
            .iter()
            .enumerate()
            .flat_map(|(rack_slot, slot)| {
                slot.custom_effect_names.iter().enumerate().filter_map(
                    move |(effect_slot, name)| {
                        let name = name.as_ref()?.trim();
                        if name.is_empty() {
                            return None;
                        }
                        Some((
                            rack_slot,
                            effect_slot,
                            name.to_string(),
                            slot.effect_slots[effect_slot].clone(),
                        ))
                    },
                )
            })
            .collect::<Vec<_>>();
        for slot in &mut rack.slots {
            for effect in &mut slot.effect_slots {
                effect.node_id = 0;
                effect.modulator_node_id = 0;
            }
        }
        let display_name = sound.metadata.name.trim();
        let display_name = if display_name.is_empty() {
            fallback_name
        } else {
            display_name
        };
        self.graph_controller()
            .replace_track_instrument_container_with_rack(track, rack, display_name)?;
        for (rack_slot, effect_slot, name, saved) in saved_effects {
            if let Some(builtin_name) = project_builtin_effect_name_for_load(&name) {
                self.load_builtin_rack_slot_effect_to_slot_sync(
                    track,
                    rack_slot,
                    effect_slot,
                    &builtin_name,
                )?;
            } else {
                self.load_rack_slot_effect_to_slot_sync(track, rack_slot, effect_slot, &name)?;
            }
            let live = self
                .state
                .pattern
                .rack_tracks
                .lock()
                .unwrap()
                .get(track)
                .and_then(Option::as_ref)
                .and_then(|rack| rack.slots.get(rack_slot))
                .cloned()
                .ok_or_else(|| "Loaded Sound rack slot disappeared".to_string())?;
            let mut restored = saved;
            restored.node_id = live.effect_slots[effect_slot].node_id;
            restored.modulator_node_id = live.effect_slots[effect_slot].modulator_node_id;
            restored.sync_to_descriptor_with_modulator(
                &live.effect_descriptors[effect_slot],
                restored.node_id,
                restored.modulator_node_id,
            );
            if !self
                .state
                .update_rack_slot_in_all_pattern_snapshots(track, rack_slot, |slot| {
                    slot.effect_slots[effect_slot] = restored.clone()
                })
            {
                return Err("Failed to restore Sound effect parameters".to_string());
            }
            self.push_rack_slot_effect_defaults(track, rack_slot, effect_slot);
        }
        self.push_all_restored_defaults();
        Ok(())
    }

    pub fn save_rack_preset(
        &mut self,
        track: usize,
        name: &str,
        overwrite: bool,
    ) -> Result<PathBuf, String> {
        if self.graph.track_instrument_types.get(track) != Some(&InstrumentType::Rack) {
            return Err("Current track is not an instrument rack".to_string());
        }
        if crate::project::rack_preset_path(name).exists() && !overwrite {
            return Err(format!("Preset '{name}' already exists"));
        }
        let preset =
            self.capture_track_as_container_preset(track, name, Vec::new(), String::new())?;
        let path =
            crate::project::save_rack_preset(name, &preset).map_err(|error| error.to_string())?;
        self.set_track_sound_state(track, None, Some(name.to_string()), false);
        Ok(path)
    }

    /// Captures a drum rack as a **kit** (`docs/drum-rack-v2-spec.md`,
    /// "Polish"): the rack's identity plus one Sound per pad, in pad order.
    /// Patterns are deliberately left behind — see [`ProjectKitPreset`].
    ///
    /// [`ProjectKitPreset`]: crate::project::ProjectKitPreset
    pub fn capture_rack_as_kit(
        &mut self,
        group_id: u64,
        name: &str,
        tags: Vec<String>,
        author: String,
    ) -> Result<crate::project::ProjectKitPreset, String> {
        let group = self
            .groups
            .iter()
            .find(|group| group.id == group_id)
            .ok_or_else(|| format!("Track group {group_id} does not exist"))?;
        let rack = group
            .rack
            .as_ref()
            .ok_or_else(|| format!("Track group {group_id} is not a drum rack"))?;
        let color = group.color;
        // Resolve every pad against the member list up front: capturing a pad
        // borrows `self` mutably, and a capture must not see a half-built kit.
        let pads: Vec<(i32, Option<u8>, usize)> = rack
            .pads
            .iter()
            .enumerate()
            .filter_map(|(pad_index, pad)| {
                let track = group.members.get(pad.member).copied()?;
                Some((pad.pad_note, rack.choke_group(pad_index), track))
            })
            .collect();
        if pads.is_empty() {
            return Err("A kit needs at least one pad with a sound on it".to_string());
        }
        let mut kit_pads = Vec::with_capacity(pads.len());
        for (pad_note, choke_group, track) in pads {
            let track_name = self
                .tracks
                .get(track)
                .cloned()
                .ok_or_else(|| format!("Kit pad {pad_note} has no member track"))?;
            let sound = self.capture_track_as_container_preset(
                track,
                &track_name,
                Vec::new(),
                String::new(),
            )?;
            kit_pads.push(crate::project::ProjectKitPad {
                pad_note,
                choke_group,
                name: track_name,
                sound,
            });
        }
        Ok(crate::project::ProjectKitPreset {
            version: crate::project::project_file_version(),
            metadata: crate::project::ProjectSoundMetadata {
                name: name.trim().to_string(),
                tags,
                author,
            },
            color,
            pads: kit_pads,
        })
    }

    /// Saves a drum rack to the kit browser. `overwrite` guards an existing
    /// kit of the same name.
    pub fn save_rack_as_kit(
        &mut self,
        group_id: u64,
        name: &str,
        overwrite: bool,
    ) -> Result<PathBuf, String> {
        if name.trim().is_empty() {
            return Err("A kit needs a name".to_string());
        }
        if crate::project::kit_preset_path(name).exists() && !overwrite {
            return Err(format!("Kit '{name}' already exists"));
        }
        let kit = self.capture_rack_as_kit(group_id, name, Vec::new(), String::new())?;
        crate::project::save_kit_preset(name, &kit).map_err(|error| error.to_string())
    }

    /// Rebuilds a saved kit as a fresh drum rack: the group and its bus, then
    /// one member track per pad loaded from the pad's Sound, then the pad map
    /// and choke groups. Returns the new group id along with a message per pad
    /// that failed to load — a kit missing one sample still loads the rest, and
    /// every step it took is an ordinary undo entry.
    pub fn load_kit_as_rack(&mut self, path: &Path) -> Result<(u64, Vec<String>), String> {
        let kit = crate::project::load_kit_preset(path).map_err(|error| error.to_string())?;
        let fallback_name = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or("Kit");
        let kit_name = if kit.metadata.name.trim().is_empty() {
            fallback_name.to_string()
        } else {
            kit.metadata.name.trim().to_string()
        };
        let (group_id, _) = self.create_drum_rack_recorded(Some(kit_name))?;
        if let Some(group) = self.groups.iter_mut().find(|group| group.id == group_id) {
            group.color = kit.color;
        }
        let mut failures = Vec::new();
        for pad in kit.pads {
            let pad_name = if pad.name.trim().is_empty() {
                format!("Pad {}", pad.pad_note)
            } else {
                pad.name.trim().to_string()
            };
            let track = match self.add_track_from_sound_preset(pad.sound, &pad_name) {
                Ok(track) => track,
                Err(error) => {
                    failures.push(format!("{pad_name}: {error}"));
                    continue;
                }
            };
            if let Err(error) = self.attach_track_to_group(track, group_id, Some(pad.pad_note)) {
                // The track exists but has no pad; drop it rather than leave a
                // loose member of nothing.
                let _ = self.graph_controller().delete_track(track);
                failures.push(format!("{pad_name}: {error}"));
                continue;
            }
            if let Err(error) = self.commit_created_track(track, "Load kit pad") {
                failures.push(format!("{pad_name}: {error}"));
                continue;
            }
            if let Some(choke) = pad.choke_group {
                if let Err(error) =
                    self.set_rack_pad_choke_group_recorded(group_id, pad.pad_note, Some(choke))
                {
                    failures.push(format!("{pad_name}: {error}"));
                }
            }
        }
        Ok((group_id, failures))
    }

    /// Auditions a kit into an existing drum rack as one atomic authoring edit.
    /// Existing lanes are reused by pad note, so their patterns stay put while
    /// the sounds behind them change; pads absent from the new kit disappear
    /// and new notes receive new, empty member lanes. Undo restores the exact
    /// previous rack topology and every member instrument.
    pub fn load_kit_onto_rack(&mut self, group_id: u64, path: &Path) -> Result<String, String> {
        let kit = crate::project::load_kit_preset(path).map_err(|error| error.to_string())?;
        let fallback_name = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or("Kit");
        let kit_name = if kit.metadata.name.trim().is_empty() {
            fallback_name.to_string()
        } else {
            kit.metadata.name.trim().to_string()
        };
        let group = self.groups.iter().find(|group| group.id == group_id)
            .ok_or_else(|| format!("Track group {group_id} does not exist"))?;
        let rack = group.rack.as_ref()
            .ok_or_else(|| format!("Track group {group_id} is not a drum rack"))?;
        let old_ids = group.members.iter().map(|track| {
            self.track_registry.id_at(*track)
                .ok_or_else(|| format!("Drum rack member {} has no stable identity", track + 1))
        }).collect::<Result<Vec<_>, String>>()?;
        let old_by_note = rack.pads.iter().filter_map(|pad| {
            group.members.get(pad.member)
                .and_then(|track| self.track_registry.id_at(*track))
                .map(|id| (pad.pad_note, id))
        }).collect::<std::collections::HashMap<_, _>>();

        let mut seen_notes = std::collections::HashSet::new();
        for pad in &kit.pads {
            if !(crate::sequencer::DRUM_RACK_FIRST_PAD_NOTE..=crate::sequencer::DRUM_RACK_LAST_PAD_NOTE)
                .contains(&pad.pad_note)
            {
                return Err(format!("Kit pad note {} is outside the drum rack range", pad.pad_note));
            }
            if !seen_notes.insert(pad.pad_note) {
                return Err(format!("Kit contains duplicate pad note {}", pad.pad_note));
            }
        }
        let resulting_track_count = self.tracks.len() - old_ids.len() + kit.pads.len();
        if resulting_track_count == 0 {
            return Err("A kit cannot remove the last remaining track".to_string());
        }

        let history_checkpoint = self.history.clone();
        let history_len = self.history.undo_len();
        let result = (|| {
            let mut desired = Vec::with_capacity(kit.pads.len());
            for pad in kit.pads {
                let pad_name = if pad.name.trim().is_empty() {
                    format!("Pad {}", pad.pad_note)
                } else {
                    pad.name.trim().to_string()
                };
                let track_id = if let Some(track_id) = old_by_note.get(&pad.pad_note).copied() {
                    let track = self.track_registry.index_of(track_id)
                        .ok_or_else(|| format!("Kit pad {pad_name} lost its member track"))?;
                    self.load_sound_preset_onto_track(track, pad.sound, &pad_name)?;
                    track_id
                } else {
                    let track = self.add_track_from_sound_preset(pad.sound, &pad_name)?;
                    if let Err(error) = self.commit_created_track(track, "Load kit pad") {
                        let rollback = self.graph_controller().delete_track(track);
                        return match rollback {
                            Ok(_) => Err(error),
                            Err(rollback_error) => Err(format!(
                                "{error}; failed to remove the uncommitted kit member: {rollback_error}"
                            )),
                        };
                    }
                    self.track_registry.id_at(track)
                        .ok_or_else(|| format!("Kit pad {pad_name} has no stable identity"))?
                };
                desired.push((pad.pad_note, pad.choke_group, track_id));
            }

            let desired_ids = desired.iter().map(|(_, _, id)| *id)
                .collect::<std::collections::HashSet<_>>();
            let desired_for_group = desired.clone();
            let kit_name_for_group = kit_name.clone();
            let kit_color = kit.color;
            self.apply_recorded_bus_group_structure_mutation("Load kit into drum rack", |app| {
                let group_index = app.groups.iter().position(|group| group.id == group_id)
                    .ok_or_else(|| format!("Track group {group_id} does not exist"))?;
                let bus = BusId(app.groups[group_index].bus_id);
                let mut members = desired_for_group.iter().map(|(_, _, id)| {
                    app.track_registry.index_of(*id)
                        .ok_or_else(|| "A loaded kit member disappeared".to_string())
                }).collect::<Result<Vec<_>, String>>()?;
                members.sort_unstable();
                members.dedup();
                for track in &members {
                    app.set_track_output_all_scenes_unrecorded(*track, TrackOutput::Bus(bus));
                }
                for old_id in &old_ids {
                    if !desired_ids.contains(old_id) {
                        if let Some(track) = app.track_registry.index_of(*old_id) {
                            app.set_track_output_all_scenes_unrecorded(track, TrackOutput::Mix);
                        }
                    }
                }
                let pads = desired_for_group.iter().map(|(note, _, id)| {
                    let track = app.track_registry.index_of(*id)
                        .ok_or_else(|| "A loaded kit member disappeared".to_string())?;
                    let member = members.binary_search(&track)
                        .map_err(|_| "A loaded kit member was not assigned to the rack".to_string())?;
                    Ok(crate::project::ProjectRackPad { pad_note: *note, member })
                }).collect::<Result<Vec<_>, String>>()?;
                let group = &mut app.groups[group_index];
                group.name.clone_from(&kit_name_for_group);
                group.color = kit_color;
                group.members = members;
                group.rack = Some(crate::project::ProjectRackConfig {
                    pads,
                    choke_groups: desired_for_group.iter().map(|(_, choke, _)| *choke).collect(),
                });
                Ok(())
            })?;

            let mut removed = old_ids.iter().filter(|id| !desired_ids.contains(id))
                .filter_map(|id| self.track_registry.index_of(*id)).collect::<Vec<_>>();
            removed.sort_unstable_by(|a, b| b.cmp(a));
            for track in removed {
                self.delete_track_recorded(track)?;
            }
            crate::app::edit::squash_history_since(self, history_len, "Audition kit on drum rack");
            Ok(kit_name.clone())
        })();
        match result {
            Ok(name) => Ok(name),
            Err(error) => match crate::app::edit::rollback_history_to(self, history_checkpoint) {
                Ok(()) => Err(error),
                Err(rollback_error) => Err(format!(
                    "Kit audition failed ({error}); restoring the rack also failed ({rollback_error:?})"
                )),
            },
        }
    }

    /// Replaces a selected drum rack with one Sound while preserving the
    /// rack's first lane as the resulting track. This is the rack equivalent
    /// of swapping a track instrument: the lane's patterns remain, nested
    /// parent-group membership is transferred, and one undo restores the rack.
    pub fn replace_rack_with_sound(&mut self, group_id: u64, path: &Path) -> Result<usize, String> {
        let sound = crate::project::load_sound_preset(path).map_err(|error| error.to_string())?;
        let fallback_name = path.file_stem().and_then(|stem| stem.to_str()).unwrap_or("Sound");
        let group = self.groups.iter().find(|group| group.id == group_id)
            .ok_or_else(|| format!("Track group {group_id} does not exist"))?;
        if !group.is_rack() {
            return Err(format!("Track group {group_id} is not a drum rack"));
        }
        let member_ids = group.members.iter().map(|track| {
            self.track_registry.id_at(*track)
                .ok_or_else(|| format!("Drum rack member {} has no stable identity", track + 1))
        }).collect::<Result<Vec<_>, String>>()?;
        let parent_id = self.groups.iter()
            .find(|parent| parent.rack_members.contains(&group_id))
            .map(|parent| parent.id);

        let history_checkpoint = self.history.clone();
        let history_len = self.history.undo_len();
        let result = (|| {
            let replacement_id = if let Some(id) = member_ids.first().copied() {
                let track = self.track_registry.index_of(id)
                    .ok_or_else(|| "The rack's first member disappeared".to_string())?;
                self.load_sound_preset_onto_track(track, sound, fallback_name)?;
                id
            } else {
                let track = self.add_track_from_sound_preset(sound, fallback_name)?;
                if let Err(error) = self.commit_created_track(track, "Create Sound track") {
                    let rollback = self.graph_controller().delete_track(track);
                    return match rollback {
                        Ok(_) => Err(error),
                        Err(rollback_error) => Err(format!(
                            "{error}; failed to remove the uncommitted Sound track: {rollback_error}"
                        )),
                    };
                }
                self.track_registry.id_at(track)
                    .ok_or_else(|| "The replacement Sound track has no stable identity".to_string())?
            };

            self.apply_recorded_bus_group_structure_mutation("Replace drum rack with Sound", |app| {
                let replacement = app.track_registry.index_of(replacement_id)
                    .ok_or_else(|| "The replacement Sound track disappeared".to_string())?;
                let rack_index = app.groups.iter().position(|group| group.id == group_id)
                    .ok_or_else(|| format!("Track group {group_id} does not exist"))?;
                if let Some(position) = app.groups[rack_index].members.iter()
                    .position(|track| *track == replacement)
                {
                    app.groups[rack_index].members.remove(position);
                    app.groups[rack_index].rack.as_mut()
                        .expect("validated rack")
                        .remap_after_member_removed(position);
                }
                if let Some(parent_id) = parent_id {
                    let parent = app.groups.iter().position(|group| group.id == parent_id)
                        .ok_or_else(|| "The rack's parent group disappeared".to_string())?;
                    app.groups[parent].rack_members.retain(|id| *id != group_id);
                    let bus = BusId(app.groups[parent].bus_id);
                    let position = app.groups[parent].members.partition_point(|track| *track < replacement);
                    app.groups[parent].members.insert(position, replacement);
                    app.set_track_output_all_scenes_unrecorded(replacement, TrackOutput::Bus(bus));
                } else {
                    app.set_track_output_all_scenes_unrecorded(replacement, TrackOutput::Mix);
                }
                Ok(())
            })?;

            let mut removed = member_ids.iter().skip(1)
                .filter_map(|id| self.track_registry.index_of(*id)).collect::<Vec<_>>();
            removed.sort_unstable_by(|a, b| b.cmp(a));
            for track in removed {
                self.delete_track_recorded(track)?;
            }
            self.delete_group_recorded(group_id)?;
            crate::app::edit::squash_history_since(self, history_len, "Audition Sound on drum rack");
            self.track_registry.index_of(replacement_id)
                .ok_or_else(|| "The replacement Sound track disappeared".to_string())
        })();
        match result {
            Ok(track) => Ok(track),
            Err(error) => match crate::app::edit::rollback_history_to(self, history_checkpoint) {
                Ok(()) => Err(error),
                Err(rollback_error) => Err(format!(
                    "Sound audition failed ({error}); restoring the rack also failed ({rollback_error:?})"
                )),
            },
        }
    }

    pub fn load_rack_preset_onto_track(&mut self, track: usize, name: &str) -> Result<(), String> {
        if self.graph.track_instrument_types.get(track) != Some(&InstrumentType::Rack) {
            return Err("Rack presets can only be loaded onto an instrument rack".to_string());
        }
        let preset = crate::project::load_rack_preset(name).map_err(|error| error.to_string())?;
        let track_id = self.track_registry.id_at(track)
            .ok_or_else(|| format!("Track {} has no stable identity", track + 1))?;
        let preset_name = name.to_string();
        self.apply_recorded_instrument_binding_mutation(track, "Load rack preset", |app| {
            app.load_container_preset_onto_track(track, preset, &preset_name)?;
            app.set_track_sound_state(track, None, Some(preset_name), false);
            app.device_registry.clear_rack_track(track_id);
            Ok(())
        })
    }

    pub fn promote_preset_to_sound(
        &mut self,
        track: usize,
        preset_name: &str,
    ) -> Result<PathBuf, String> {
        let mut sound = match self.graph.track_instrument_types.get(track).copied() {
            Some(InstrumentType::Rack) => {
                crate::project::load_rack_preset(preset_name).map_err(|error| error.to_string())?
            }
            Some(InstrumentType::Custom) => {
                let engine_id = self
                    .graph
                    .track_engine_ids
                    .get(track)
                    .and_then(|id| *id)
                    .ok_or_else(|| "Current custom instrument engine is unavailable".to_string())?;
                let instrument_name = self
                    .editor
                    .engine_registry
                    .get(engine_id)
                    .map(|engine| engine.name.clone())
                    .ok_or_else(|| "Current custom instrument is unavailable".to_string())?;
                let preset = crate::lisp_host::load_instrument_presets(&instrument_name)
                    .map_err(|error| error.to_string())?
                    .into_iter()
                    .find(|preset| preset.name == preset_name)
                    .ok_or_else(|| format!("Preset '{preset_name}' not found"))?;
                let descriptor = self
                    .graph
                    .instrument_descriptors
                    .get(track)
                    .cloned()
                    .ok_or_else(|| "Instrument descriptor unavailable".to_string())?;
                let mut sound = self.capture_track_as_container_preset(
                    track,
                    preset_name,
                    Vec::new(),
                    String::new(),
                )?;
                let slot = sound
                    .rack
                    .slots
                    .first_mut()
                    .ok_or_else(|| "Captured instrument preset has no rack slot".to_string())?;
                apply_instrument_preset_to_container_slot(
                    &mut slot.instrument_slot,
                    &mut slot.instrument_base_note_offset,
                    &descriptor,
                    &preset,
                );
                // Track insert FX are not part of an ordinary instrument preset.
                // Only container presets (racks) carry their saved slot chains.
                for effect in &mut slot.effect_slots {
                    *effect = crate::project::ProjectEffectSlot::default();
                }
                slot.custom_effects.fill(None);
                slot.track_sound_state.loaded_preset = Some(preset.name.clone());
                slot.track_sound_state.dirty = false;
                sound
            }
            Some(InstrumentType::Sampler) => {
                return Err("Sampler tracks do not have instrument presets".to_string())
            }
            _ => return Err("Current track cannot promote presets to Sounds".to_string()),
        };
        sound.metadata.name = preset_name.to_string();
        crate::project::save_sound_preset(preset_name, &sound).map_err(|error| error.to_string())
    }

    pub(super) fn capture_project(&mut self, project_name: &str) -> Result<ProjectFile, String> {
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
        self.state
            .reconcile_committed_arrangement_track_lanes()
            .map_err(|error| {
                format!("Could not reconcile arrangement tracks before save: {error}")
            })?;
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
        let device_instances = self.capture_project_device_instances(&tracks, &custom_effects)?;
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

        // §17.4 prune-on-save: drop orphaned entities from the LIVE pools
        // before capturing. This is the one seam the spec sanctions, and it
        // is safe here because nothing holds Patch/Mix ids into the live
        // pools from outside `ProjectScenes` — undo lane snapshots clone
        // pools wholesale, so history entries stay self-consistent.
        self.state.prune_unreferenced_sounds();
        // Takes spec 6.1/11.1: record per-scene cell presence (the dense
        // `patterns` bank cannot encode a bare lane) and serialize each
        // track's take pool with its chunk patterns inline — chunks are in
        // no scene cell, so the pattern bank never carries them.
        let scenes_for_takes = self.state.capture_project_scenes();
        let scene_cell_presence: Vec<Vec<bool>> = scenes_for_takes
            .scenes
            .iter()
            .map(|scene| {
                (0..num_tracks)
                    .map(|track| scene.cells.get(track).copied().flatten().is_some())
                    .collect()
            })
            .collect();
        // Skip the field entirely when every cell is present, keeping files
        // byte-identical to the pre-takes format.
        let scene_cell_presence = if scene_cell_presence
            .iter()
            .all(|mask| mask.iter().all(|present| *present))
        {
            Vec::new()
        } else {
            scene_cell_presence
        };
        let mut take_pools = Vec::new();
        for (track, takes) in scenes_for_takes.take_pools.iter().enumerate() {
            let mut file_takes = Vec::new();
            for take in &takes.takes {
                let mut chunks = Vec::with_capacity(take.chunks.len());
                for chunk_id in &take.chunks {
                    let data = scenes_for_takes
                        .track_pools
                        .get(track)
                        .and_then(|pool| pool.get(*chunk_id))
                        .ok_or_else(|| {
                            format!(
                                "Track {} take '{}' references chunk pattern {} which is \
                                 not in the track's pattern pool",
                                track + 1,
                                take.name,
                                chunk_id.0
                            )
                        })?;
                    let mut snapshot = PatternSnapshot::new_default(num_tracks, &[]);
                    snapshot.set_track_pattern_data(track, data);
                    let mut sample_paths = vec![None; num_tracks];
                    let mut sample_names = vec![String::new(); num_tracks];
                    let (buffer_id, sample_name, _) = snapshot.sample_ids[track].clone();
                    if snapshot
                        .instrument_types
                        .get(track)
                        .copied()
                        .unwrap_or(InstrumentType::Sampler)
                        == InstrumentType::Sampler
                        && !sample_name.is_empty()
                    {
                        // `usize::MAX` bypasses the current-scene live-path
                        // shortcut; chunks resolve through the registries.
                        sample_paths[track] = self
                            .resolve_sample_path_for_snapshot(
                                usize::MAX,
                                track,
                                buffer_id,
                                &sample_name,
                            )?
                            .map(|path| path.to_string_lossy().to_string());
                    }
                    sample_names[track] = sample_name;
                    chunks.push(ProjectPattern::from_snapshot(
                        &snapshot,
                        sample_paths,
                        sample_names,
                        Vec::new(),
                    ));
                }
                file_takes.push(crate::project::ProjectTake {
                    id: take.id.0,
                    name: take.name.clone(),
                    total_len_steps: take.total_len_steps,
                    chunks,
                });
            }
            take_pools.push(crate::project::ProjectTrackTakePool {
                takes: file_takes,
                next_take_id: takes.next_take_id,
            });
        }
        // Skip the field when no track holds any take.
        let take_pools = if take_pools.iter().all(|pool| pool.takes.is_empty()) {
            Vec::new()
        } else {
            take_pools
        };

        // Sound ref structure (takes spec 18.1 step 5): per track, which
        // entity each scene cell, cell pattern, and take references. Entity
        // ids are the live per-track PatchId/MixId values — unique within
        // the track, meaningful only within this file. Content stays in the
        // dense bank / take chunks, except entities referenced ONLY by bare
        // cells (no pattern to carry them), which serialize as
        // `orphan_sounds` carriers. Unreferenced entities are pruned simply
        // by never being named here (§17.4 prune-on-save).
        let mut track_sounds: Vec<crate::project::ProjectTrackSounds> =
            Vec::with_capacity(num_tracks);
        for track in 0..num_tracks {
            let pool = scenes_for_takes.track_pools.get(track);
            let to_refs = |refs: crate::sequencer::SoundRefs| crate::project::ProjectSoundRefs {
                patch: refs.patch.0,
                mix: refs.mix.0,
            };
            let cells: Vec<crate::project::ProjectSoundRefs> = scenes_for_takes
                .scenes
                .iter()
                .map(|scene| {
                    scene
                        .cell_sounds
                        .get(track)
                        .copied()
                        .map(to_refs)
                        .unwrap_or(crate::project::ProjectSoundRefs {
                            patch: u64::MAX,
                            mix: u64::MAX,
                        })
                })
                .collect();
            let patterns: Vec<Option<crate::project::ProjectSoundRefs>> = scenes_for_takes
                .scenes
                .iter()
                .map(|scene| {
                    scene
                        .cells
                        .get(track)
                        .copied()
                        .flatten()
                        .and_then(|id| pool.and_then(|pool| pool.refs(id)))
                        .map(to_refs)
                })
                .collect();
            let takes: Vec<crate::project::ProjectSoundRefs> = scenes_for_takes
                .take_pools
                .get(track)
                .map(|takes| takes.takes.iter().map(|take| to_refs(take.sound)).collect())
                .unwrap_or_default();
            // Which entity ids the file already carries content for: cell
            // patterns ride the dense bank, take chunks their take entry.
            let mut carried_patches: std::collections::HashSet<u64> =
                std::collections::HashSet::new();
            let mut carried_mixes: std::collections::HashSet<u64> =
                std::collections::HashSet::new();
            for refs in patterns.iter().flatten() {
                carried_patches.insert(refs.patch);
                carried_mixes.insert(refs.mix);
            }
            for refs in &takes {
                carried_patches.insert(refs.patch);
                carried_mixes.insert(refs.mix);
            }
            // The track sound (track-sound spec §2.1) serializes its refs
            // like a cell does; content rides a carrier below when nothing
            // else carries the pair.
            let track_refs = scenes_for_takes
                .track_sound_refs(track)
                .map(to_refs);
            // Any cell (or the track sound) naming an uncarried entity gets
            // a content carrier for its pair (one per distinct pair — bare
            // cells left behind by a pattern delete can share). A pair whose
            // carried half is duplicated into the carrier is harmless: the
            // loader seeds only ids that content-carrying referents didn't
            // already claim.
            let mut orphan_sounds = Vec::new();
            let mut emitted: std::collections::HashSet<(u64, u64)> =
                std::collections::HashSet::new();
            for refs in cells.iter().chain(track_refs.iter()) {
                if refs.patch == u64::MAX || refs.mix == u64::MAX {
                    continue;
                }
                if carried_patches.contains(&refs.patch) && carried_mixes.contains(&refs.mix) {
                    continue;
                }
                if !emitted.insert((refs.patch, refs.mix)) {
                    continue;
                }
                let data = pool.and_then(|pool| {
                    pool.compose_bare_sound(crate::sequencer::SoundRefs {
                        patch: crate::sequencer::PatchId(refs.patch),
                        mix: crate::sequencer::MixId(refs.mix),
                    })
                });
                let Some(data) = data else {
                    // Dangling refs (always-resolves invariant violated): the
                    // content is already unrecoverable; keep the save usable.
                    debug_assert!(false, "bare cell refs do not resolve on track {track}");
                    continue;
                };
                let mut snapshot = PatternSnapshot::new_default(num_tracks, &[]);
                snapshot.set_track_pattern_data(track, data);
                let mut sample_paths = vec![None; num_tracks];
                let mut sample_names = vec![String::new(); num_tracks];
                let (buffer_id, sample_name, _) = snapshot.sample_ids[track].clone();
                if snapshot
                    .instrument_types
                    .get(track)
                    .copied()
                    .unwrap_or(InstrumentType::Sampler)
                    == InstrumentType::Sampler
                    && !sample_name.is_empty()
                {
                    sample_paths[track] = self
                        .resolve_sample_path_for_snapshot(
                            usize::MAX,
                            track,
                            buffer_id,
                            &sample_name,
                        )?
                        .map(|path| path.to_string_lossy().to_string());
                }
                sample_names[track] = sample_name;
                orphan_sounds.push(crate::project::ProjectOrphanSound {
                    patch: refs.patch,
                    mix: refs.mix,
                    data: ProjectPattern::from_snapshot(
                        &snapshot,
                        sample_paths,
                        sample_names,
                        Vec::new(),
                    ),
                });
            }
            // §17.11 display metadata for every entity this file names —
            // pruned entities are never named, so their meta drops with them.
            let mut named_patches: std::collections::HashSet<u64> = carried_patches;
            let mut named_mixes: std::collections::HashSet<u64> = carried_mixes;
            for refs in cells.iter().chain(track_refs.iter()) {
                if refs.patch != u64::MAX {
                    named_patches.insert(refs.patch);
                }
                if refs.mix != u64::MAX {
                    named_mixes.insert(refs.mix);
                }
            }
            let meta_entry = |id: u64, meta: Option<&crate::sequencer::SoundEntityMeta>| {
                meta.map(|meta| crate::project::ProjectSoundEntityMeta {
                    id,
                    name: meta.name.clone(),
                    color: meta.color.map(i32::from).unwrap_or(-1),
                })
            };
            let mut patch_meta: Vec<crate::project::ProjectSoundEntityMeta> = named_patches
                .iter()
                .filter_map(|id| {
                    meta_entry(
                        *id,
                        pool.and_then(|pool| {
                            pool.sounds.patch_meta.get(&crate::sequencer::PatchId(*id))
                        }),
                    )
                })
                .collect();
            patch_meta.sort_by_key(|meta| meta.id);
            let mut mix_meta: Vec<crate::project::ProjectSoundEntityMeta> = named_mixes
                .iter()
                .filter_map(|id| {
                    meta_entry(
                        *id,
                        pool.and_then(|pool| {
                            pool.sounds.mix_meta.get(&crate::sequencer::MixId(*id))
                        }),
                    )
                })
                .collect();
            mix_meta.sort_by_key(|meta| meta.id);
            track_sounds.push(crate::project::ProjectTrackSounds {
                cells,
                patterns,
                takes,
                track: track_refs,
                orphan_sounds,
                patch_meta,
                mix_meta,
            });
        }

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
            device_instances,
            scratch: ProjectScratchState {
                buffer: self.editor.scratch_buffer.clone(),
                cursor_row: self.editor.scratch_cursor.0,
                cursor_col: self.editor.scratch_cursor.1,
            },
            patterns,
            groups: self.groups.clone(),
            // The arrangement always exists (empty-arrangement spec 7):
            // every save writes one, the empty arrangement included.
            // Serialization maps live pattern-pool ids into the
            // deterministic ids the loader rebuilds from scene cells; a
            // clip referencing an unpersistable pattern fails the save
            // naming the clip and its track instead of silently dropping
            // the reference (docs/arrangement-lane-model-spec.md 10).
            arrangement: Some(crate::sequencer::arrangement_for_serialization(
                &self
                    .state
                    .committed_arrangement()
                    .unwrap_or_else(|| self.empty_arrangement()),
                &self.state.capture_project_scenes(),
            )?),
            macros: self
                .macro_engine
                .macros()
                .iter()
                .map(ProjectMacro::from)
                .collect(),
            next_macro_id: self.macro_engine.next_id(),
            // Vestigial since docs/unified-transport-spec.md 7: kept on the
            // wire for parse tolerance, never read back.
            use_arrangement: true,
            record_armed: if self.graph.record_armed.iter().any(|armed| *armed) {
                self.graph.record_armed.clone()
            } else {
                Vec::new()
            },
            scene_cell_presence,
            take_pools,
            track_sounds,
        })
    }

    fn capture_project_tracks(&self) -> Result<Vec<ProjectTrack>, String> {
        self.tracks
            .iter()
            .enumerate()
            .map(|(track_idx, name)| {
                let id = self
                    .track_registry
                    .id_at(track_idx)
                    .ok_or_else(|| format!("Missing stable id for track {}", track_idx + 1))?;
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
                    Ok(ProjectTrack {
                        id,
                        color,
                        collapsed,
                        kind: ProjectTrackKind::Rack {
                            routing: crate::project::ProjectRackRouting::Broadcast,
                            slots,
                        },
                    })
                } else if self.is_sampler_track(track_idx) {
                    let path = self
                        .sampler_path_for_track(track_idx)
                        .or_else(|| self.resolve_sample_path_by_name(name));
                    let Some(path) = path else {
                        return Err(format!("Couldn't resolve sample path for '{}'", name));
                    };
                    Ok(ProjectTrack {
                        id,
                        color,
                        collapsed,
                        kind: ProjectTrackKind::Sampler {
                            sample_path: path.to_string_lossy().to_string(),
                        },
                    })
                } else if self.graph.track_instrument_types.get(track_idx)
                    == Some(&InstrumentType::Modulator)
                {
                    Ok(ProjectTrack {
                        id,
                        color,
                        collapsed,
                        kind: ProjectTrackKind::Modulator,
                    })
                } else {
                    let instrument_name = self
                        .graph
                        .track_engine_ids
                        .get(track_idx)
                        .and_then(|engine_id| *engine_id)
                        .and_then(|engine_id| self.editor.engine_registry.get(engine_id))
                        .map(|engine| engine.name.clone())
                        .unwrap_or_else(|| name.clone());
                    Ok(ProjectTrack {
                        id,
                        color,
                        collapsed,
                        kind: ProjectTrackKind::Custom { instrument_name },
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

    fn capture_project_device_instances(
        &mut self,
        tracks: &[ProjectTrack],
        custom_effects: &[Vec<Option<String>>],
    ) -> Result<crate::project::ProjectDeviceInstances, String> {
        let mut result = crate::project::ProjectDeviceInstances::default();
        for (track, project_track) in tracks.iter().enumerate() {
            let sources = custom_effects.get(track).cloned().unwrap_or_default();
            let active_sources = sources.into_iter().take_while(Option::is_some)
                .flatten().collect::<Vec<_>>();
            let ids = self.device_registry.audio_effect_chain(
                project_track.id,
                (0..active_sources.len()).map(|offset| BUILTIN_SLOT_COUNT + offset),
            );
            self.device_registry.bind_audio_effect_chain(
                project_track.id,
                BUILTIN_SLOT_COUNT,
                &ids,
            )?;
            result.track_effects.push(crate::project::ProjectTrackEffectChain {
                track_id: project_track.id.0,
                instances: ids.into_iter().zip(active_sources).map(|(id, name)| {
                    crate::project::ProjectEffectInstance {
                        id: id.0,
                        source: crate::project::ProjectEffectSource::from_project_name(&name),
                    }
                }).collect(),
            });

            let midi_names = self.state.pattern.track_params.get(track)
                .map(|params| params.midi_fx_chain())
                .unwrap_or_default();
            let midi_ids = self.device_registry.midi_effect_chain(project_track.id, midi_names.len());
            self.device_registry.bind_midi_effect_chain(project_track.id, &midi_ids)?;
            result.midi_effects.push(crate::project::ProjectMidiEffectChain {
                track_id: project_track.id.0,
                instances: midi_ids.into_iter().zip(midi_names).map(|(id, name)| {
                    crate::project::ProjectMidiEffectInstance { id: id.0, name }
                }).collect(),
            });
        }
        for bus in &self.buses {
            let names = bus.custom_effect_names.iter().take_while(|name| name.is_some())
                .filter_map(|name| name.clone()).collect::<Vec<_>>();
            let ids = (0..names.len()).map(|slot| {
                self.device_registry.bus_audio_effect(bus.id, slot)
            }).collect::<Vec<_>>();
            self.device_registry.bind_bus_audio_effect_chain(bus.id, &ids)?;
            result.bus_effects.push(crate::project::ProjectBusEffectChain {
                bus_id: bus.id.0,
                instances: ids.into_iter().zip(names).map(|(id, name)| {
                    crate::project::ProjectEffectInstance {
                        id: id.0,
                        source: crate::project::ProjectEffectSource::from_project_name(&name),
                    }
                }).collect(),
            });
        }
        let racks = self.state.pattern.rack_tracks.lock().unwrap().clone();
        for (track, rack) in racks.into_iter().enumerate() {
            let (Some(project_track), Some(rack)) = (tracks.get(track), rack) else { continue };
            for (slot_index, slot) in rack.slots.into_iter().enumerate() {
                let rack_slot_id = self.device_registry.rack_slot(project_track.id, slot_index);
                let names = slot.custom_effect_names.into_iter().take_while(Option::is_some)
                    .flatten().collect::<Vec<_>>();
                let ids = (0..names.len()).map(|effect_slot| {
                    self.device_registry.rack_audio_effect(rack_slot_id, effect_slot)
                }).collect::<Vec<_>>();
                self.device_registry.bind_rack_audio_effect_chain(rack_slot_id, &ids)?;
                result.rack_effects.push(crate::project::ProjectRackEffectChain {
                    track_id: project_track.id.0,
                    slot_index,
                    rack_slot_id: rack_slot_id.0,
                    instances: ids.into_iter().zip(names).map(|(id, name)| {
                        crate::project::ProjectEffectInstance {
                            id: id.0,
                            source: crate::project::ProjectEffectSource::from_project_name(&name),
                        }
                    }).collect(),
                });
            }
        }
        Ok(result)
    }

    fn restore_project_device_instances(
        &mut self,
        instances: &crate::project::ProjectDeviceInstances,
    ) -> Result<(), String> {
        let mut seen = std::collections::HashSet::new();
        let mut observe = |id: u64, label: &str| -> Result<(), String> {
            if id == 0 {
                return Err(format!("{label} has an invalid zero identity"));
            }
            if !seen.insert(id) {
                return Err(format!("project device identity {id} is duplicated"));
            }
            Ok(())
        };
        for chain in &instances.track_effects {
            for instance in &chain.instances { observe(instance.id, "track effect")?; }
        }
        for chain in &instances.midi_effects {
            for instance in &chain.instances { observe(instance.id, "MIDI effect")?; }
        }
        for chain in &instances.bus_effects {
            for instance in &chain.instances { observe(instance.id, "bus effect")?; }
        }
        for chain in &instances.rack_effects {
            observe(chain.rack_slot_id, "rack slot")?;
            for instance in &chain.instances { observe(instance.id, "rack-slot effect")?; }
        }

        let expected_tracks = self.track_registry.ids().iter()
            .map(|id| id.0).collect::<std::collections::HashSet<_>>();
        let track_effect_owners = instances.track_effects.iter()
            .map(|chain| chain.track_id).collect::<std::collections::HashSet<_>>();
        if track_effect_owners.len() != instances.track_effects.len()
            || track_effect_owners != expected_tracks
        {
            return Err("project track-effect records do not cover every track exactly once".to_string());
        }
        let midi_effect_owners = instances.midi_effects.iter()
            .map(|chain| chain.track_id).collect::<std::collections::HashSet<_>>();
        if midi_effect_owners.len() != instances.midi_effects.len()
            || midi_effect_owners != expected_tracks
        {
            return Err("project MIDI-effect records do not cover every track exactly once".to_string());
        }
        let expected_buses = self.buses.iter()
            .map(|bus| bus.id.0).collect::<std::collections::HashSet<_>>();
        let bus_effect_owners = instances.bus_effects.iter()
            .map(|chain| chain.bus_id).collect::<std::collections::HashSet<_>>();
        if bus_effect_owners.len() != instances.bus_effects.len()
            || bus_effect_owners != expected_buses
        {
            return Err("project bus-effect records do not cover every bus exactly once".to_string());
        }
        let racks = self.state.pattern.rack_tracks.lock().unwrap().clone();
        let mut expected_rack_slots = std::collections::HashSet::new();
        for (track, rack) in racks.iter().enumerate() {
            let Some(rack) = rack else { continue };
            let track_id = self.track_registry.id_at(track)
                .ok_or_else(|| format!("rack at track {} has no stable track identity", track + 1))?;
            expected_rack_slots.extend(
                (0..rack.slots.len()).map(|slot_index| (track_id.0, slot_index)),
            );
        }
        let rack_effect_owners = instances.rack_effects.iter()
            .map(|chain| (chain.track_id, chain.slot_index))
            .collect::<std::collections::HashSet<_>>();
        if rack_effect_owners.len() != instances.rack_effects.len()
            || rack_effect_owners != expected_rack_slots
        {
            return Err("project rack-effect records do not cover every rack slot exactly once".to_string());
        }

        for chain in &instances.track_effects {
            let track_id = crate::sequencer::TrackId(chain.track_id);
            let track = self.track_registry.index_of(track_id)
                .ok_or_else(|| format!("effect chain references missing track {}", chain.track_id))?;
            let live_count = self.state.pattern.effect_chains[track]
                .iter().skip(BUILTIN_SLOT_COUNT)
                .take_while(|slot| slot.node_id.load(Ordering::Relaxed) != 0)
                .count();
            if live_count != chain.instances.len() {
                return Err(format!("track {} effect-instance count does not match the live chain", track + 1));
            }
        }
        for chain in &instances.midi_effects {
            let track_id = crate::sequencer::TrackId(chain.track_id);
            let track = self.track_registry.index_of(track_id)
                .ok_or_else(|| format!("MIDI chain references missing track {}", chain.track_id))?;
            if self.state.pattern.track_params[track].midi_fx_chain().len() != chain.instances.len() {
                return Err(format!("track {} MIDI-instance count does not match the live chain", track + 1));
            }
        }
        for chain in &instances.bus_effects {
            let bus_id = BusId(chain.bus_id);
            let bus = self.buses.iter().position(|bus| bus.id == bus_id)
                .ok_or_else(|| format!("effect chain references missing bus {}", chain.bus_id))?;
            let live_count = self.buses[bus].effect_slots.iter()
                .take_while(|slot| slot.node_id != 0).count();
            if live_count != chain.instances.len() {
                return Err(format!("bus {} effect-instance count does not match the live chain", chain.bus_id));
            }
        }
        for chain in &instances.rack_effects {
            let track_id = crate::sequencer::TrackId(chain.track_id);
            let track = self.track_registry.index_of(track_id)
                .ok_or_else(|| format!("rack chain references missing track {}", chain.track_id))?;
            let rack_slot_id = crate::sequencer::RackSlotId(chain.rack_slot_id);
            let slot = self.state.pattern.rack_tracks.lock().unwrap()
                .get(track).and_then(Option::as_ref)
                .and_then(|rack| rack.slots.get(chain.slot_index)).cloned()
                .ok_or_else(|| format!("rack chain references missing track {} slot {}", track + 1, chain.slot_index + 1))?;
            let live_count = slot.effect_slots.iter().take_while(|slot| slot.node_id != 0).count();
            if live_count != chain.instances.len() {
                return Err(format!("rack-slot effect-instance count does not match the live chain"));
            }
        }

        self.device_registry.clear();
        for chain in &instances.track_effects {
            let track_id = crate::sequencer::TrackId(chain.track_id);
            let ids = chain.instances.iter()
                .map(|instance| crate::sequencer::EffectInstanceId(instance.id))
                .collect::<Vec<_>>();
            self.device_registry.bind_audio_effect_chain(track_id, BUILTIN_SLOT_COUNT, &ids)?;
        }
        for chain in &instances.midi_effects {
            let track_id = crate::sequencer::TrackId(chain.track_id);
            let ids = chain.instances.iter()
                .map(|instance| crate::sequencer::MidiFxInstanceId(instance.id))
                .collect::<Vec<_>>();
            self.device_registry.bind_midi_effect_chain(track_id, &ids)?;
        }
        for chain in &instances.bus_effects {
            let bus_id = BusId(chain.bus_id);
            let ids = chain.instances.iter()
                .map(|instance| crate::sequencer::EffectInstanceId(instance.id))
                .collect::<Vec<_>>();
            self.device_registry.bind_bus_audio_effect_chain(bus_id, &ids)?;
        }
        for chain in &instances.rack_effects {
            let track_id = crate::sequencer::TrackId(chain.track_id);
            let rack_slot_id = crate::sequencer::RackSlotId(chain.rack_slot_id);
            self.device_registry.bind_rack_slot(track_id, chain.slot_index, rack_slot_id)?;
            let ids = chain.instances.iter()
                .map(|instance| crate::sequencer::EffectInstanceId(instance.id))
                .collect::<Vec<_>>();
            self.device_registry.bind_rack_audio_effect_chain(rack_slot_id, &ids)?;
        }
        Ok(())
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

    pub fn resolve_sample_path_by_name(&self, sample_name: &str) -> Option<PathBuf> {
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
        if ir_ref == crate::effects::conv_reverb::DEFAULT_IR_REF {
            crate::effects::conv_reverb::default_ir_path()
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
        if ir_ref.is_empty() || ir_ref == crate::effects::conv_reverb::DEFAULT_IR_REF {
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

    fn restore_filter_table_track(
        &mut self,
        track: usize,
        slot_idx: usize,
        table_ref: Option<&str>,
    ) {
        let Some(table_ref) = table_ref else { return };
        // Saved references embed the analysis mode and optionally the engine;
        // the sample resolves by its decoded name while the bare reference
        // keeps the analysis deterministic. `fltab:` references resolve to
        // baked asset files instead of samples. The engine restores first so
        // the table lands on the node that will keep it.
        let (table_ref, engine) = crate::effects::filter_table::split_engine_ref(table_ref);
        if engine != crate::effects::filter_table::TableEngine::default() {
            if let Err(error) = self.set_track_filter_table_engine(track, slot_idx, engine) {
                eprintln!(
                    "project-load: Filter Table engine '{}' not restored: {error}",
                    engine.tag()
                );
            }
        }
        let (sample_name, _mode) = crate::effects::filter_table::decode_table_ref(table_ref);
        if table_ref.is_empty() || sample_name == crate::effects::filter_table::DEFAULT_TABLE_REF {
            return;
        }
        if let Some(path) = self.resolve_filter_table_source_path(sample_name) {
            if let Err(error) = self.set_filter_table_source(track, slot_idx, &path, table_ref) {
                eprintln!("project-load: Filter Table '{table_ref}' not restored: {error}");
            }
        } else {
            eprintln!("project-load: Filter Table '{table_ref}' could not be resolved");
        }
    }

    /// Resolve a decoded Filter Table reference to a file on disk: an asset
    /// stem to a `.fltab` asset, anything else to a sample by name.
    pub fn resolve_filter_table_source_path(&self, sample_name: &str) -> Option<PathBuf> {
        match crate::effects::filter_table_asset::decode_asset_ref(sample_name) {
            Some(stem) => crate::effects::filter_table_asset::resolve_asset_path(stem),
            None => self.resolve_sample_path_by_name(sample_name),
        }
    }

    fn restore_filter_table_bus(
        &mut self,
        bus_idx: usize,
        slot_idx: usize,
        table_ref: Option<&str>,
    ) {
        let Some(table_ref) = table_ref else { return };
        let (table_ref, engine) = crate::effects::filter_table::split_engine_ref(table_ref);
        if engine != crate::effects::filter_table::TableEngine::default() {
            if let Err(error) = self.set_bus_filter_table_engine(bus_idx, slot_idx, engine) {
                eprintln!(
                    "project-load: bus Filter Table engine '{}' not restored: {error}",
                    engine.tag()
                );
            }
        }
        let (sample_name, _mode) = crate::effects::filter_table::decode_table_ref(table_ref);
        if table_ref.is_empty() || sample_name == crate::effects::filter_table::DEFAULT_TABLE_REF {
            return;
        }
        if let Some(path) = self.resolve_filter_table_source_path(sample_name) {
            if let Err(error) =
                self.set_filter_table_source_bus(bus_idx, slot_idx, &path, table_ref)
            {
                eprintln!("project-load: bus Filter Table '{table_ref}' not restored: {error}");
            }
        } else {
            eprintln!("project-load: bus Filter Table '{table_ref}' could not be resolved");
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
        if ir_ref.is_empty() || ir_ref == crate::effects::conv_reverb::DEFAULT_IR_REF {
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
                self.clear_project_arrangement_state();
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
                    self.track_registry = crate::sequencer::TrackRegistry::from_ids(
                        pending.project.tracks.iter().map(|track| track.id),
                    )
                    .map_err(|error| format!("Invalid project track ids: {error:?}"))?;
                    pending.phase = super::PendingProjectLoadPhase::AddEffect {
                        track_idx: 0,
                        offset: 0,
                    };
                } else {
                    let saved_color = pending.project.tracks[track_idx].color();
                    let saved_collapsed = pending.project.tracks[track_idx].collapsed();
                    match &pending.project.tracks[track_idx].kind {
                        ProjectTrackKind::Sampler { sample_path } => {
                            eprintln!(
                                "project-load: add sampler track index={} path={}",
                                track_idx, sample_path
                            );
                            let sample_path = Path::new(sample_path);
                            let asset = self
                                .load_project_sample_asset(
                                    &mut pending.sample_assets,
                                    sample_path,
                                )
                                .map_err(|error| {
                                    format!(
                                        "Failed to load sample '{}': {error}",
                                        sample_path.display()
                                    )
                                })?;
                            let track_name =
                                crate::sample_db::display_title_for_sample_path(sample_path)
                                    .unwrap_or_else(|| asset.decoded_name.clone());
                            self.graph_controller().add_track_from_sample(
                                sample_path,
                                asset.buffer_id,
                                asset.sample_rate,
                                track_name,
                            )?;
                        }
                        ProjectTrackKind::Custom { instrument_name } => {
                            eprintln!(
                                "project-load: add custom track index={} instrument={}",
                                track_idx, instrument_name
                            );
                            self.add_saved_instrument_track_sync(instrument_name)?;
                        }
                        ProjectTrackKind::Modulator => {
                            eprintln!("project-load: add modulator track index={track_idx}");
                            self.graph_controller().add_modulator_track()?;
                        }
                        ProjectTrackKind::Rack { routing, slots } => {
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
                            // Rack slot topology and effects are instance state
                            // (one rack_effects chain per slot regardless of
                            // scene), but the snapshot is stored per pattern and
                            // scenes without a presence cell for this track save
                            // it as None. Fall back to any pattern that has one
                            // so loading with a bare current scene doesn't drop
                            // the slot state and fail instance validation.
                            let rack_pattern = pending
                                .project
                                .patterns
                                .get(current_pattern_idx)
                                .and_then(|pattern| pattern.rack_tracks.get(track_idx))
                                .and_then(|rack| rack.as_ref())
                                .cloned()
                                .or_else(|| {
                                    pending.project.patterns.iter().find_map(|pattern| {
                                        pattern
                                            .rack_tracks
                                            .get(track_idx)
                                            .and_then(|rack| rack.as_ref())
                                            .cloned()
                                    })
                                });
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
                                        let asset = self
                                            .load_project_sample_asset(
                                                &mut pending.sample_assets,
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
                                        let sample_name = slot
                                            .sample_name
                                            .clone()
                                            .or_else(|| {
                                                saved_pattern_slot
                                                    .and_then(|slot| slot.sample_name.clone())
                                            })
                                            .unwrap_or_else(|| asset.decoded_name.clone());
                                        self.register_loaded_sample_path(
                                            &sample_name,
                                            asset.buffer_id,
                                            PathBuf::from(sample_path),
                                        );
                                        prepared_sources.push(PreparedRackSlotSource::Sampler(
                                            RackSamplerBuildSpec {
                                                buffer_id: asset.buffer_id,
                                                sample_rate: asset.sample_rate,
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
                                        .unwrap_or(crate::audio::MAX_VOICES),
                                    param_plocks: saved_slot
                                        .as_ref()
                                        .map(|slot| slot.param_plocks.clone()),
                                    instrument_slot: saved_slot
                                        .as_ref()
                                        .map(|slot| slot.instrument_slot.clone()),
                                    effect_slots: saved_slot
                                        .as_ref()
                                        .map(|slot| slot.effect_slots.clone()),
                                    effect_descriptors: saved_slot
                                        .as_ref()
                                        .map(|slot| slot.effect_descriptors.clone()),
                                    custom_effect_names: saved_slot
                                        .as_ref()
                                        .map(|slot| slot.custom_effect_names.clone()),
                                    track_sound_state: saved_slot
                                        .as_ref()
                                        .map(|slot| slot.track_sound_state.clone()),
                                });
                            }
                            // Legacy `by_pitch` racks load as layering racks:
                            // drum racks are track groups now
                            // (docs/drum-rack-v2-spec.md).
                            self.graph_controller()
                                .add_rack_track("Layer Rack", build_specs)?;
                            let saved_rack_effects = rack_pattern
                                .as_ref()
                                .into_iter()
                                .flat_map(|rack| rack.slots.iter().enumerate())
                                .flat_map(|(rack_slot, slot)| {
                                    slot.custom_effects.iter().enumerate().filter_map(
                                        move |(effect_slot, name)| {
                                            let name = name.as_ref()?.trim();
                                            if name.is_empty() {
                                                return None;
                                            }
                                            let saved = slot.effect_slots.get(effect_slot)?.clone();
                                            Some((rack_slot, effect_slot, name.to_string(), saved))
                                        },
                                    )
                                })
                                .collect::<Vec<_>>();
                            for (rack_slot, effect_slot, name, saved) in saved_rack_effects {
                                if let Some(builtin_name) =
                                    project_builtin_effect_name_for_load(&name)
                                {
                                    self.load_builtin_rack_slot_effect_to_slot_sync(
                                        track_idx,
                                        rack_slot,
                                        effect_slot,
                                        &builtin_name,
                                    )?;
                                } else {
                                    self.load_rack_slot_effect_to_slot_sync(
                                        track_idx,
                                        rack_slot,
                                        effect_slot,
                                        &name,
                                    )?;
                                }
                                let graph_slot = self
                                    .state
                                    .pattern
                                    .rack_tracks
                                    .lock()
                                    .unwrap()
                                    .get(track_idx)
                                    .and_then(Option::as_ref)
                                    .and_then(|rack| rack.slots.get(rack_slot))
                                    .cloned()
                                    .ok_or_else(|| "Loaded rack slot disappeared".to_string())?;
                                let descriptor = graph_slot.effect_descriptors[effect_slot].clone();
                                let live_effect = &graph_slot.effect_slots[effect_slot];
                                let restored = project_slot_into_synced_snapshot_with_modulator(
                                    saved,
                                    &descriptor,
                                    live_effect.node_id,
                                    live_effect.modulator_node_id,
                                );
                                if !self.state.update_rack_slot_in_current_pattern(
                                    track_idx,
                                    rack_slot,
                                    |slot| slot.effect_slots[effect_slot] = restored.clone(),
                                ) {
                                    return Err("Failed to restore rack-slot FX state".to_string());
                                }
                                self.push_rack_slot_effect_defaults(
                                    track_idx,
                                    rack_slot,
                                    effect_slot,
                                );
                            }
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
                        let saved_table = pending.project.patterns.iter().find_map(|pattern| {
                            pattern.effect_slots
                                .get(track_idx)
                                .and_then(|slots| slots.get(offset))
                                .and_then(|slot| slot.table.clone())
                        });
                        self.restore_filter_table_track(
                            track_idx,
                            BUILTIN_SLOT_COUNT + offset,
                            saved_table.as_deref(),
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
                            &mut pending.sample_assets,
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

    fn finish_project_load(
        &mut self,
        mut pending: super::PendingProjectLoad,
    ) -> Result<(), String> {
        eprintln!(
            "project-load: finish start name={} tracks={} built_patterns={} fallback_samples={}",
            pending.name,
            self.tracks.len(),
            pending.built_patterns.len(),
            pending.fallback_samples
        );
        let ProjectFile {
            version: file_version,
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
            device_instances,
            patterns: _,
            groups,
            arrangement,
            macros,
            next_macro_id,
            use_arrangement: _,
            record_armed,
            scene_cell_presence,
            take_pools,
            track_sounds,
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
        // Takes spec 6.1/11.1: re-apply per-scene cell absence (bare lanes)
        // and rebuild each track's take pool. Chunk patterns convert through
        // the same snapshot path as scene patterns (sample resolution
        // included) and are inserted into the freshly rebuilt pools; take
        // ids are restored verbatim so serialized song overrides stay valid.
        let mut loaded_take_pools = Vec::with_capacity(take_pools.len());
        for (track, pool) in take_pools.into_iter().enumerate() {
            let mut takes = Vec::with_capacity(pool.takes.len());
            for take in pool.takes {
                let mut chunk_data = Vec::with_capacity(take.chunks.len());
                for chunk in take.chunks {
                    let (snapshot, _, fallback_count) =
                        self.project_pattern_into_snapshot(
                            chunk,
                            &mut pending.sample_assets,
                        )?;
                    let data = snapshot.track_pattern_data(track).ok_or_else(|| {
                        format!(
                            "Take '{}' chunk is missing lane data for track {}",
                            take.name,
                            track + 1
                        )
                    })?;
                    pending.fallback_samples += fallback_count;
                    chunk_data.push(data);
                }
                takes.push((take.id, take.name, take.total_len_steps, chunk_data));
            }
            loaded_take_pools.push((pool.next_take_id, takes));
        }
        self.state
            .install_project_arrangement(&scene_cell_presence, loaded_take_pools);
        // v7 sound ref structure (takes spec 18.1 step 5): re-link referents
        // that shared an entity when the file was saved. Legacy files carry
        // no structure — the canonical migration above (private entities per
        // pattern, take chunks collapsed) is already the correct shape.
        // Orphan-sound carriers (bare-cell content) convert through the same
        // snapshot path as take chunks, sample resolution included.
        let mut sound_model = Vec::with_capacity(track_sounds.len());
        for (track, sounds) in track_sounds.into_iter().enumerate() {
            let mut carriers = Vec::with_capacity(sounds.orphan_sounds.len());
            for carrier in sounds.orphan_sounds {
                let (snapshot, _, fallback_count) = self
                    .project_pattern_into_snapshot(carrier.data, &mut pending.sample_assets)?;
                let data = snapshot.track_pattern_data(track).ok_or_else(|| {
                    format!(
                        "Sound carrier is missing lane data for track {}",
                        track + 1
                    )
                })?;
                pending.fallback_samples += fallback_count;
                carriers.push((carrier.patch, carrier.mix, data));
            }
            sound_model.push((
                sounds
                    .cells
                    .into_iter()
                    .map(|refs| (refs.patch, refs.mix))
                    .collect::<Vec<_>>(),
                sounds
                    .patterns
                    .into_iter()
                    .map(|refs| refs.map(|refs| (refs.patch, refs.mix)))
                    .collect::<Vec<_>>(),
                sounds
                    .takes
                    .into_iter()
                    .map(|refs| (refs.patch, refs.mix))
                    .collect::<Vec<_>>(),
                carriers,
                sounds
                    .patch_meta
                    .into_iter()
                    .map(|meta| (meta.id, meta.name, meta.color))
                    .collect::<Vec<_>>(),
                sounds
                    .mix_meta
                    .into_iter()
                    .map(|meta| (meta.id, meta.name, meta.color))
                    .collect::<Vec<_>>(),
                sounds.track.map(|refs| (refs.patch, refs.mix)),
            ));
        }
        self.state.apply_project_sound_model(&sound_model);
        // Relinking abandons the privately minted entities of every referent
        // that adopted a canonical one; drop those orphans now instead of
        // carrying them until the first save prunes them.
        self.state.prune_unreferenced_sounds();
        // Per-track record-arm flags (takes spec 8.1), persisted like
        // mute/solo. The UI-shared arm vector syncs FROM `graph.record_armed`
        // on the next tick via `record_arm_sync_pending`.
        let mut loaded_armed = record_armed;
        loaded_armed.resize(self.tracks.len(), false);
        self.graph.record_armed = loaded_armed;
        self.record_arm_sync_pending = true;
        // The serialized arrangement was structurally validated at
        // deserialize time against `SerializedSongContext`, which knows only
        // the id domain — it can see neither scene cells nor timebases, so a
        // compile against it would silently produce no scene-backdrop phase
        // overrides at all. Compile here instead: `replace_pattern_repository`
        // and `install_project_arrangement` above have just installed the
        // rebuilt pattern pools, scene cells, and take pools, so the live
        // scenes are exactly the context the compiler needs.
        // `set_committed_arrangement` validates, compiles, and installs the
        // arrangement together with its compiled song, and installs nothing
        // if the arrangement no longer fits the project.
        //
        // Version <= 5 files were authored under the retired backdrop model,
        // where a lane GAP played the governing scene's cell. Under the
        // current model a gap is silence, so loading such a file untouched
        // would silently gut the arrangement. Freeze what every gap sounded
        // like into real clips first (lane spec 10, v5 -> v6): the project
        // sounds identical, and the file saves back as version 6.
        let arrangement = match arrangement {
            Some(arrangement) if file_version < 6 => {
                let scenes = self.state.capture_project_scenes();
                Some(
                    crate::sequencer::migrate_legacy_backdrops(&arrangement, &scenes)
                        .map_err(|error| format!("Project arrangement failed to load: {error}"))?,
                )
            }
            other => other,
        };
        // A file with no arrangement (any version) loads the empty one:
        // the arrangement always exists (empty-arrangement spec 7).
        let arrangement = Some(arrangement.unwrap_or_else(|| self.empty_arrangement()));
        self.state
            .set_committed_arrangement(arrangement)
            .map_err(|error| format!("Project arrangement failed to load: {error}"))?;
        self.state
            .transport
            .pattern_epoch
            .fetch_add(1, Ordering::Relaxed);
        self.state.transport.bpm.store(bpm, Ordering::Relaxed);
        self.state
            .transport
            .master_volume
            .store(master_volume.clamp(0.0, 2.0).to_bits(), Ordering::Relaxed);
        let mut buses = buses;
        // Bus chaining is only structurally acyclic while the nesting rule
        // holds; a file is not a proof, so repair dangling/cyclic outputs
        // before they reach the graph (docs/drum-rack-v2-spec.md).
        if crate::project::sanitize_bus_outputs(&mut buses) {
            eprintln!("Project load: reset an invalid bus output chain to the master mix");
        }
        self.buses = if buses.is_empty() {
            BusChannelState::default_buses()
        } else {
            buses.into_iter().map(BusChannelState::from).collect()
        };
        if let Some(mix) = self.buses.iter_mut().find(|bus| bus.id == BusId::MIX) {
            mix.output = crate::project::BusOutput::Mix;
        }
        if !self.buses.iter().any(|bus| bus.id == BusId::MIX) {
            self.buses
                .insert(0, BusChannelState::new(BusId::MIX, "Mix"));
        }
        self.graph_controller().reconcile_bus_graph_nodes()?;
        // Defensively drop dangling groups: every backing bus must resolve and
        // every member index must be in range (track count is known here).
        let group_track_count = self.tracks.len();
        self.groups = groups
            .into_iter()
            .filter(|group| {
                self.buses.iter().any(|bus| bus.id.0 == group.bus_id)
                    // A drum rack with zero members is legal: its pads are
                    // lazy, and a plain group is legal while it holds a rack.
                    && (group.is_rack()
                        || !group.members.is_empty()
                        || !group.rack_members.is_empty())
                    && group.members.iter().all(|&m| m < group_track_count)
            })
            .map(|mut group| {
                // Enforce the rack invariants on load: pads point at live
                // members, pad notes are unique, one pad per member.
                let member_count = group.members.len();
                let group_name = group.name.clone();
                if let Some(rack) = group.rack.as_mut() {
                    rack.sanitize(member_count);
                    // Repair projects saved before every attach path mapped a
                    // pad: a member with no pad is unreachable from the grid,
                    // so give it the next free note in member order.
                    let repaired = rack.map_unmapped_members(member_count);
                    if repaired > 0 {
                        eprintln!(
                            "Project load: mapped {repaired} padless member(s) of drum rack '{group_name}' onto free pads"
                        );
                    }
                }
                group
            })
            .collect();
        // Nesting can only be judged once the surviving group set is known: a
        // child rack dropped above must not leave a stale parent reference.
        if crate::project::sanitize_group_nesting(&mut self.groups) {
            eprintln!("Project load: dropped group nesting that breaks the rack nesting rule");
        }
        self.reconcile_rack_group_bus_outputs();
        self.publish_rack_choke_runtime();
        // Reconcile group routing: a group's members must reach its backing bus
        // in every scene. Output is stored per-scene, so older saves (or any
        // pre-fix grouping) can have members still pointing at Mix in some/all
        // scenes — repair that here so the group actually submixes on load.
        for group in self.groups.clone() {
            let output = TrackOutput::Bus(BusId(group.bus_id));
            for &member in &group.members {
                self.set_track_output_all_scenes_unrecorded(member, output.clone());
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
            let saved_table = saved_slot.table.clone();
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
            self.restore_filter_table_bus(bus_idx, slot_idx, saved_table.as_deref());
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
                        idx: crate::effects::stereo_panner::STEREO_PANNER_PARAM_VOLUME,
                        logical_id: nodes.volume_id as u64,
                        fvalue: crate::mixer_volume::fader_to_gain(bus.volume),
                    },
                );
                crate::audiograph::params_push_wrapper(
                    self.graph.lg.0,
                    crate::audiograph::ParamMsg {
                        idx: crate::effects::stereo_panner::STEREO_PANNER_PARAM_MUTE,
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

        // Track-sound spec §5.3.1: the state's ownership context is a mirror
        // of the App's view flag. A load can hand us a fresh `SequencerState`
        // (default `false`), so re-assert before anything reads the masks.
        self.state
            .set_arrangement_context(self.arrangement_view_visible);
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
        self.set_reverb_param_unrecorded(0, reverb.size);
        self.set_reverb_param_unrecorded(1, reverb.brightness);
        self.set_reverb_param_unrecorded(2, reverb.replace);
        let mut macro_engine = crate::macro_engine::MacroEngine::default();
        let mut restored_values = Vec::with_capacity(macros.len());
        for project_macro in macros {
            let macro_definition = crate::macro_engine::Macro::try_from(project_macro)
                .map_err(|error| format!("invalid persisted macro: {error:?}"))?;
            restored_values.push((macro_definition.id, macro_definition.value));
            macro_engine
                .insert_macro(macro_definition)
                .map_err(|error| format!("invalid persisted macro id: {error:?}"))?;
        }
        macro_engine.ensure_next_id_at_least(next_macro_id);
        for (id, value) in restored_values {
            macro_engine.set_value(id, value);
        }
        self.macro_engine = macro_engine;
        self.restore_project_device_instances(&device_instances)?;
        self.push_all_restored_defaults();
        self.push_all_delay_bpm();

        if !self.tracks.is_empty() {
            self.clamp_cursor_to_steps();
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
        // Effect slots restore before the full track list settles, so any
        // FxSidechain enum (Compressor, Filterbank FM/AM sources) may still
        // hold its "off"-only placeholder labels — re-patch with the loaded
        // track names.
        self.refresh_effect_sidechain_labels();
        self.ui.recording = false;
        self.recording_history = None;
        self.history.reset();
        self.device_registry.clear();
        self.history.mark_saved();
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
                    slot.effect_descriptors = graph_slot.effect_descriptors.clone();
                    slot.custom_effect_names = graph_slot.custom_effect_names.clone();
                    slot.effect_slots.resize_with(
                        crate::lisp_host::MAX_CUSTOM_FX,
                        crate::effects::EffectSlotSnapshot::new_empty,
                    );
                    for effect_idx in 0..crate::lisp_host::MAX_CUSTOM_FX {
                        let graph_effect = &graph_slot.effect_slots[effect_idx];
                        let descriptor = &graph_slot.effect_descriptors[effect_idx];
                        if graph_effect.node_id == 0 {
                            slot.effect_slots[effect_idx] =
                                crate::effects::EffectSlotSnapshot::new_empty();
                        } else {
                            slot.effect_slots[effect_idx].sync_to_descriptor_with_modulator(
                                descriptor,
                                graph_effect.node_id,
                                graph_effect.modulator_node_id,
                            );
                        }
                    }
                    rebound_slots.push(slot);
                }
                saved_rack.slots = rebound_slots;
                Some(saved_rack)
            })
            .collect()
    }

    pub(super) fn project_pattern_into_snapshot(
        &mut self,
        pattern: ProjectPattern,
        sample_assets: &mut std::collections::HashMap<PathBuf, ProjectSampleAsset>,
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

                // Full-width take chunks only populate their owning lane. Empty
                // sampler lanes are intentionally unbound, not missing assets
                // to be replaced with an arbitrary file from the sample tree.
                if saved_path.is_none() && saved_name.trim().is_empty() {
                    sample_ids.push((-1, String::new(), self.graph.sample_rate));
                    continue;
                }

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
                    });

                // A moved or renamed asset must never make the project
                // unopenable: leave the lane unbound (as an empty lane is) and
                // count it so the load reports the substitution.
                let Some(path_buf) = resolved_path else {
                    let reference = saved_path
                        .as_ref()
                        .map(|path| format!("path '{}'", path.display()))
                        .unwrap_or_else(|| format!("name '{saved_name}'"));
                    eprintln!(
                        "project-load: couldn't resolve saved sample {reference} for track {}; leaving lane unbound",
                        track_idx + 1,
                    );
                    fallback_count += 1;
                    sample_ids.push((-1, String::new(), self.graph.sample_rate));
                    continue;
                };
                let asset = self
                    .load_project_sample_asset(sample_assets, &path_buf)
                    .map_err(|error| {
                        format!(
                            "Failed to load sample '{}' for track {}: {}",
                            path_buf.display(),
                            track_idx + 1,
                            error
                        )
                    })?;
                let buffer_id = asset.buffer_id;
                let sample_rate = asset.sample_rate;
                let sample_name = crate::sample_db::display_title_for_sample_path(&path_buf)
                    .or_else(|| {
                        let saved_name = saved_name.trim();
                        (!saved_name.is_empty()).then(|| saved_name.to_string())
                    })
                    .unwrap_or(asset.decoded_name);
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
            track_send_plock_snapshots,
            bus_patterns,
            mod_connections,
            neural_networks,
            graph_overrides,
            instrument_types: _,
            instrument_run_modes,
            sample_paths: _,
            sample_names: _,
            rack_tracks,
            process_chains,
            project_process_lane_overrides,
            project_process_chain,
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
                                    table: None,
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
                                table: None,
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
                                table: None,
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
            track_send_plock_snapshots: track_send_plock_snapshots
                .into_iter()
                .map(|steps| steps.into_iter().take(MAX_STEPS).map(|sends| {
                    sends.into_iter().map(Into::into).collect()
                }).chain(std::iter::repeat_with(Vec::new)).take(MAX_STEPS).collect())
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
            process_chains,
            project_process_lane_overrides,
            project_process_chain,
            plock_variant_registries,
            key_lock_variant_registries,
        };
        snapshot.normalize_track_count(num_tracks, &self.graph.effect_descriptors);
        snapshot.refresh_process_binding_param_ids(
            &self.graph.effect_descriptors,
            &self.graph.instrument_descriptors,
        );
        refresh_neural_output_override_param_ids(&mut snapshot);

        Ok((snapshot, bus_patterns, fallback_count))
    }

    pub fn push_all_restored_defaults(&mut self) {
        let state = Arc::clone(&self.state);
        let effect_descriptors = &self.graph.effect_descriptors;
        let instrument_descriptors = &self.graph.instrument_descriptors;
        let buses = &self.buses;
        self.macro_engine.revalidate_mappings(|scope, target| {
            resolve_live_macro_target(
                &state,
                effect_descriptors,
                instrument_descriptors,
                buses,
                scope,
                target,
            )
        });
        self.state
            .publish_macro_overrides(self.macro_engine.override_snapshot());

        self.push_master_volume();
        for track_idx in 0..self.tracks.len() {
            self.push_track_volume(track_idx);
            self.push_track_pan(track_idx);
            self.push_track_mute(track_idx);
            self.push_send_gain(track_idx);
            self.graph_controller().apply_track_bus_sends(track_idx);
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
                        self.send_effective_slot_param(track_idx, slot_idx, param_idx);
                    }
                }
            }
        }
        self.push_track_solo_mutes();
        self.push_all_restored_instrument_defaults();
        self.state.publish_scheduler_snapshot();
    }
}

fn apply_instrument_preset_to_container_slot(
    slot: &mut crate::project::ProjectEffectSlot,
    base_note_offset: &mut f32,
    descriptor: &crate::effects::EffectDescriptor,
    preset: &crate::lisp_host::InstrumentPreset,
) {
    *base_note_offset = preset.base_note_offset;
    for (index, param) in descriptor.params.iter().enumerate() {
        if index < slot.defaults.len() {
            slot.defaults[index] = param.clamp(
                preset
                    .params
                    .get(&param.name)
                    .copied()
                    .unwrap_or(param.default),
            );
        }
    }
    slot.key_locks.clear();
    slot.key_lock_param_ids.clear();
    for (&note, locks) in &preset.key_locks {
        let mut row = vec![None; descriptor.params.len()];
        for (param_name, value) in locks {
            let mut matches = descriptor
                .params
                .iter()
                .enumerate()
                .filter(|(_, param)| param.name == *param_name);
            let Some((index, param)) = matches.next() else {
                continue;
            };
            if matches.next().is_none() && value.is_finite() {
                row[index] = Some(param.clamp(*value));
            }
        }
        if row.iter().any(Option::is_some) {
            slot.key_locks.insert(note, row);
        }
    }
}

/// Canonical persisted project state used by undo/redo tests.
///
/// Serialization deliberately excludes runtime graph identities. The project
/// name, active scene/track, and scratch editor are normalized because they
/// are session or text-editor state rather than sequencer authoring state.
#[cfg(test)]
#[derive(Clone, PartialEq, Eq)]
pub(super) struct AuthoringStateSnapshot(Vec<u8>);

#[cfg(test)]
impl std::fmt::Debug for AuthoringStateSnapshot {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AuthoringStateSnapshot")
            .field("serialized_bytes", &self.0.len())
            .finish()
    }
}

#[cfg(test)]
impl AuthoringStateSnapshot {
    pub(super) fn first_difference(&self, other: &Self) -> Option<(usize, String, String)> {
        let index = self
            .0
            .iter()
            .zip(&other.0)
            .position(|(left, right)| left != right)
            .or_else(|| {
                (self.0.len() != other.0.len()).then_some(self.0.len().min(other.0.len()))
            })?;
        let start = index.saturating_sub(48);
        let left_end = (index + 48).min(self.0.len());
        let right_end = (index + 48).min(other.0.len());
        Some((
            index,
            String::from_utf8_lossy(&self.0[start..left_end]).into_owned(),
            String::from_utf8_lossy(&other.0[start..right_end]).into_owned(),
        ))
    }
}

#[cfg(test)]
impl App {
    pub(super) fn capture_authoring_state_snapshot(
        &mut self,
    ) -> Result<AuthoringStateSnapshot, String> {
        let mut project = self.capture_project("__authoring_state_snapshot__")?;
        project.name.clear();
        project.current_pattern = 0;
        project.current_track = None;
        project.scratch.buffer.clear();
        project.scratch.cursor_row = 0;
        project.scratch.cursor_col = 0;
        serde_json::to_vec(&project)
            .map(AuthoringStateSnapshot)
            .map_err(|error| format!("could not serialize authoring-state snapshot: {error}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_dgen_slot_migration_shifts_only_header_relative_indices() {
        let mut slot = project::ProjectEffectSlot {
            num_params: 4,
            defaults: vec![1.0, 0.5, 0.25, 0.7],
            plocks: vec![vec![None; 4]; 2],
            plock_param_ids: vec![
                vec![
                    None,
                    Some(ParamNodeId {
                        logical_id: 11,
                        node_param_idx: 6,
                    }),
                    None,
                    // Modulator-relative index that happens to be small but
                    // does not match any effect-node index in this slot.
                    Some(ParamNodeId {
                        logical_id: 12,
                        node_param_idx: 64,
                    }),
                ],
                vec![None; 4],
            ],
            key_locks: std::collections::BTreeMap::new(),
            key_lock_param_ids: std::collections::BTreeMap::from([(
                60,
                vec![
                    Some(ParamNodeId {
                        logical_id: 11,
                        node_param_idx: 9,
                    }),
                    None,
                    None,
                    None,
                ],
            )]),
            tensor_params: Vec::new(),
            // enabled param (4), two dgen cells (6, 9), one modulator param.
            param_node_indices: vec![4, 6, 9, crate::instruments::voice_modulator::MOD_PARAM_BASE + 3],
            param_node_spans: vec![1; 4],
            ir: None,
            table: None,
        };

        migrate_legacy_dgen_effect_slot(&mut slot);

        assert_eq!(
            slot.param_node_indices,
            vec![4, 10, 13, crate::instruments::voice_modulator::MOD_PARAM_BASE + 3]
        );
        assert_eq!(
            slot.plock_param_ids[0][1],
            Some(ParamNodeId {
                logical_id: 11,
                node_param_idx: 10,
            })
        );
        assert_eq!(
            slot.plock_param_ids[0][3],
            Some(ParamNodeId {
                logical_id: 12,
                node_param_idx: 64,
            }),
            "modulator-relative param id must not shift"
        );
        assert_eq!(
            slot.key_lock_param_ids[&60][0],
            Some(ParamNodeId {
                logical_id: 11,
                node_param_idx: 13,
            })
        );
    }

    #[test]
    fn dgen_hosted_effect_name_classification() {
        assert!(project_effect_name_is_dgen_hosted(Some("my-custom-fx")));
        assert!(project_effect_name_is_dgen_hosted(Some(
            "builtin:Convolution Reverb"
        )));
        assert!(project_effect_name_is_dgen_hosted(Some(
            "builtin:Filter Table"
        )));
        assert!(!project_effect_name_is_dgen_hosted(Some("builtin:EQ8")));
        assert!(!project_effect_name_is_dgen_hosted(Some("builtin:OTT")));
        assert!(!project_effect_name_is_dgen_hosted(None));
    }

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
    fn instrument_preset_promotion_uses_saved_values_instead_of_captured_live_defaults() {
        let mut descriptor = crate::effects::EffectDescriptor::builtin_filter();
        descriptor.params = vec![
            test_param("cutoff", 0.25, 0),
            test_param("resonance", 0.1, 1),
        ];
        let mut params = std::collections::BTreeMap::new();
        params.insert("cutoff".to_string(), 0.8);
        let mut note_locks = std::collections::BTreeMap::new();
        note_locks.insert("resonance".to_string(), 0.65);
        let mut key_locks = std::collections::BTreeMap::new();
        key_locks.insert(60, note_locks);
        let preset = crate::lisp_host::InstrumentPreset {
            id: "saved".to_string(),
            name: "Saved".to_string(),
            base_note_offset: 7.0,
            params,
            key_locks,
        };
        let mut container_slot = crate::project::ProjectEffectSlot {
            num_params: 2,
            defaults: vec![0.05, 0.95],
            ..crate::project::ProjectEffectSlot::default()
        };
        let mut base_note_offset = -12.0;

        apply_instrument_preset_to_container_slot(
            &mut container_slot,
            &mut base_note_offset,
            &descriptor,
            &preset,
        );

        assert_eq!(container_slot.defaults, vec![0.8, 0.1]);
        assert_eq!(container_slot.key_locks[&60], vec![None, Some(0.65)]);
        assert_eq!(base_note_offset, 7.0);
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
            table: None,
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
                    - crate::instruments::voice_modulator::MOD_PARAM_BASE,
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
            table: None,
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
    fn dgen_builtin_project_names_are_treated_as_builtin() {
        for name in [
            crate::effects::conv_reverb::NAME,
            crate::effects::filter_table::NAME,
        ] {
            let project_name = format!(
                "{}{}",
                EffectDescriptor::BUILTIN_INSERT_PREFIX,
                name,
            );
            assert_eq!(
                project_builtin_effect_name_for_save(name),
                Some(project_name.clone()),
            );
            assert_eq!(
                project_builtin_effect_name_for_load(&project_name),
                Some(name.to_string()),
            );
        }

        let project_name = project_builtin_effect_name_for_save(
            crate::effects::conv_reverb::NAME,
        ).unwrap();
        let mut project = minimal_project_with_effect_slots(
            vec![Some(crate::effects::conv_reverb::NAME.to_string())],
            Vec::new(),
        );
        project.buses = project::default_project_buses();
        project.buses[1].custom_effects.resize(1, None);
        project.buses[1].custom_effects[0] = Some(crate::effects::conv_reverb::NAME.to_string());
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
            arrangement: None,
            reverb: ProjectReverbState {
                size: 0.2,
                brightness: 0.8,
                replace: 0.3,
            },
            buses: Vec::new(),
            groups: Vec::new(),
            tracks: vec![ProjectTrack {
                id: crate::sequencer::TrackId(1),
                color: None,
                collapsed: false,
                kind: ProjectTrackKind::Sampler {
                    sample_path: "samples/kick.wav".to_string(),
                },
            }],
            custom_effects: vec![custom_effects],
            device_instances: crate::project::ProjectDeviceInstances::default(),
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
                track_send_plock_snapshots: Vec::new(),
                bus_patterns: Vec::new(),
                instrument_types: Vec::new(),
                mod_connections: Vec::new(),
                neural_networks: Vec::new(),
                graph_overrides: Vec::new(),
                sample_paths: Vec::new(),
                sample_names: Vec::new(),
                rack_tracks: Vec::new(),
                process_chains: Vec::new(),
                project_process_lane_overrides: Vec::new(),
                project_process_chain: crate::process::TrackProcessChain::default(),
                plock_variant_registries: Vec::new(),
                key_lock_variant_registries: Vec::new(),
            }],
            macros: Vec::new(),
            next_macro_id: 1,
            use_arrangement: false,
            record_armed: Vec::new(),
            scene_cell_presence: Vec::new(),
            take_pools: Vec::new(),
            track_sounds: Vec::new(),
        }
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
    fn project_with_track_and_bus_effects_roundtrips_bit_identically() {
        let desc = EffectDescriptor::builtin_insert("Filter").expect("filter descriptor");
        let mut track_slot = default_project_effect_slot(&desc);
        track_slot.defaults[0] = 0.25;
        track_slot.plocks[2][0] = Some(0.75);
        let mut bus_slot = default_project_effect_slot(&desc);
        bus_slot.defaults[0] = 0.5;
        bus_slot.plocks[3][0] = Some(0.125);

        let project_name = EffectDescriptor::builtin_insert_project_name("Filter")
            .expect("filter should have a persisted builtin name");
        let mut project =
            minimal_project_with_effect_slots(vec![Some(project_name.clone())], vec![track_slot]);
        project.buses = project::default_project_buses();
        project.buses[1].custom_effects = vec![Some(project_name)];
        project.buses[1].effect_slots = vec![bus_slot];
        project.normalize_device_instances().unwrap();

        let before = serde_json::to_string_pretty(&project).expect("serialize project");
        let restored: ProjectFile = serde_json::from_str(&before).expect("deserialize project");
        let after = serde_json::to_string_pretty(&restored).expect("reserialize project");

        assert_eq!(after, before);
    }

    #[test]
    fn project_device_instance_records_roundtrip_stable_ids_and_sources() {
        let mut project = minimal_project_with_effect_slots(Vec::new(), Vec::new());
        project.buses = project::default_project_buses();
        project.device_instances = crate::project::ProjectDeviceInstances {
            track_effects: vec![crate::project::ProjectTrackEffectChain {
                track_id: 1,
                instances: vec![crate::project::ProjectEffectInstance {
                    id: 10,
                    source: crate::project::ProjectEffectSource::Builtin {
                        name: "Filter".to_string(),
                    },
                }],
            }],
            midi_effects: vec![crate::project::ProjectMidiEffectChain {
                track_id: 1,
                instances: vec![crate::project::ProjectMidiEffectInstance {
                    id: 11,
                    name: "arp".to_string(),
                }],
            }],
            bus_effects: vec![crate::project::ProjectBusEffectChain {
                bus_id: BusId::DEFAULT_A.0,
                instances: vec![crate::project::ProjectEffectInstance {
                    id: 12,
                    source: crate::project::ProjectEffectSource::Saved {
                        name: "stereo-tremolo".to_string(),
                    },
                }],
            }],
            rack_effects: vec![crate::project::ProjectRackEffectChain {
                track_id: 1,
                slot_index: 0,
                rack_slot_id: 13,
                instances: vec![crate::project::ProjectEffectInstance {
                    id: 14,
                    source: crate::project::ProjectEffectSource::Builtin {
                        name: "OTT".to_string(),
                    },
                }],
            }],
        };

        let json = serde_json::to_string_pretty(&project).expect("serialize instance records");
        assert!(json.contains("\"device_instances\""));
        assert!(!json.contains("\"midi_fx_chain\""));
        let restored: ProjectFile = serde_json::from_str(&json).expect("restore instance records");
        assert_eq!(restored.device_instances, project.device_instances);
        assert_eq!(restored.custom_effects[0][0].as_deref(), Some("builtin:Filter"));
        assert_eq!(
            restored.buses.iter().find(|bus| bus.id == BusId::DEFAULT_A.0)
                .unwrap().custom_effects[0].as_deref(),
            Some("stereo-tremolo")
        );
    }

    #[test]
    fn device_normalization_keeps_modern_dense_custom_slots() {
        let shimmer_values = vec![
            1.6437798, 7.0884132, 53.366623, 230.13315, 0.82464963, 5579.7046, 0.85,
            0.7110942, 0.38275215, 1.0, 1.0,
        ];
        let shimmer_slot = project::ProjectEffectSlot {
            num_params: shimmer_values.len() as u32,
            defaults: shimmer_values.clone(),
            plocks: vec![vec![None; shimmer_values.len()]; MAX_STEPS],
            plock_param_ids: vec![vec![None; shimmer_values.len()]; MAX_STEPS],
            key_locks: std::collections::BTreeMap::new(),
            key_lock_param_ids: std::collections::BTreeMap::new(),
            param_node_indices: vec![6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 4],
            param_node_spans: vec![1; shimmer_values.len()],
            tensor_params: Vec::new(),
            ir: None,
            table: None,
        };
        let mut slots = vec![shimmer_slot];
        slots.resize_with(crate::lisp_host::MAX_CUSTOM_FX, Default::default);
        let mut project = minimal_project_with_effect_slots(
            vec![Some("shimmerpitch".to_string())],
            slots,
        );
        project.version = 1;

        project.normalize_device_instances().unwrap();

        assert_eq!(
            project.custom_effects[0],
            vec![Some("shimmerpitch".to_string())]
        );
        assert_eq!(
            project.patterns[0].effect_slots[0][0].defaults,
            shimmer_values
        );
        assert_eq!(
            project.patterns[0].effect_slots[0].len(),
            crate::lisp_host::MAX_CUSTOM_FX
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
                crate::instruments::voice_modulator::MOD_PARAM_BASE,
                crate::instruments::voice_modulator::MOD_PARAM_BASE + 1,
            ],
            param_node_spans: vec![1, 1, 1, 1],
            tensor_params: Vec::new(),
            ir: None,
            table: None,
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
                test_param("lfo rate", 0.03, crate::instruments::voice_modulator::MOD_PARAM_BASE),
                test_param(
                    "lfo depth",
                    0.04,
                    crate::instruments::voice_modulator::MOD_PARAM_BASE + 1,
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
                crate::instruments::voice_modulator::MOD_PARAM_BASE,
                crate::instruments::voice_modulator::MOD_PARAM_BASE + 1,
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
                crate::instruments::voice_modulator::LEGACY_FIXED_MOD_PARAM_BASE,
                crate::instruments::voice_modulator::LEGACY_FIXED_MOD_PARAM_BASE + 1,
            ],
            param_node_spans: vec![1, 1, 1, 1],
            tensor_params: Vec::new(),
            ir: None,
            table: None,
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
                test_param("mod 1 source", 1.0, crate::instruments::voice_modulator::MOD_PARAM_BASE),
                test_param(
                    "mod 1 lfo rate",
                    5.0,
                    crate::instruments::voice_modulator::MOD_PARAM_BASE + 1,
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
                crate::instruments::voice_modulator::MOD_PARAM_BASE,
                crate::instruments::voice_modulator::MOD_PARAM_BASE + 1,
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
            table: None,
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
        let key_locks = std::collections::BTreeMap::from([(
            69,
            vec![None, None, Some(0.73), None, None, Some(-0.31), None, None],
        )]);
        let key_lock_param_ids = std::collections::BTreeMap::from([(
            69,
            vec![
                None,
                None,
                Some(ParamNodeId {
                    logical_id: 7,
                    node_param_idx: 18,
                }),
                None,
                None,
                Some(ParamNodeId {
                    logical_id: 7,
                    node_param_idx: 30,
                }),
                None,
                None,
            ],
        )]);

        let saved_slot = project::ProjectEffectSlot {
            num_params: desc.params.len() as u32,
            defaults: vec![0.12, 2.0, 0.31, 0.34, 4.0, -0.27, 0.56, 0.78],
            plocks,
            plock_param_ids: vec![vec![None; desc.params.len()]; MAX_STEPS],
            key_locks,
            key_lock_param_ids,
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
            table: None,
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
        assert_eq!(restored.key_locks[&69][2], Some(0.73));
        assert_eq!(restored.key_locks[&69][5], Some(-0.31));
        assert_eq!(
            restored.key_lock_param_ids[&69][2],
            Some(ParamNodeId {
                logical_id: 42,
                node_param_idx: 18,
            })
        );
        assert_eq!(
            restored.key_lock_param_ids[&69][5],
            Some(ParamNodeId {
                logical_id: 42,
                node_param_idx: 30,
            })
        );
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
            table: None,
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
            table: None,
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
